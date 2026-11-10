//! Headless tests of the disk usage map: a fake measurement drawn as a map with its legend and totals, what
//! hovering and clicking do (drilling into a folder, going back out, opening a file), Stop, a failure, the keys, the
//! growth strip, the status bar's line, and the Arabic interface with an Arabic name. The measurement comes from a
//! fake [`Source`], so nothing touches the real core; the explorer runs on a fake disk.

use super::colors::{ColorMode, DAY_MS};
use super::model::{self, Ask, Fut, Source};
use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{self, ExplorerEvent, Notice};
use crate::platform::dragdrop::DropEffect;
use crate::testing;
use glint_model::entries::attr;
use glint_model::paths::join_path;
use glint_model::usage::{UsageGrowth, UsageMap, NO_PARENT, WAS_NOT_THERE};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// A node of a fixture map: name, folder or not, size, the node it is in, and how many days ago it changed.
type Node = (&'static str, bool, f64, u32, f64);

/// A map as `decode_usage` would read it: `nodes` in the index's order (the folder first, each folder's items
/// together after it).
fn build_map(path: &str, engine: &'static str, nodes: &[Node]) -> UsageMap {
    let count = nodes.len();
    let (mut first, mut end) = (vec![0u32; count], vec![0u32; count]);
    let mut paths: Vec<String> = Vec::new();
    for (i, (name, _, _, parent, _)) in nodes.iter().enumerate() {
        if i == 0 {
            paths.push(path.to_string());
            continue;
        }
        let p = *parent as usize;
        paths.push(join_path(&paths[p], name));
        if end[p] == 0 {
            first[p] = i as u32;
        }
        end[p] = i as u32 + 1;
    }
    let now = now_ms() as f64;
    UsageMap {
        path: path.to_string(),
        engine,
        elapsed: 1.5,
        limited: false,
        service_too_old: false,
        growth_too_old: false,
        growth: None,
        count,
        size: nodes.iter().map(|n| n.2).collect(),
        modified: nodes.iter().map(|n| now - n.4 * DAY_MS).collect(),
        attrs: nodes.iter().map(|n| if n.1 { attr::DIRECTORY } else { 0 }).collect(),
        parent: nodes
            .iter()
            .enumerate()
            .map(|(i, n)| if i == 0 { NO_PARENT } else { n.3 })
            .collect(),
        names: nodes.iter().map(|n| n.0.to_string()).collect(),
        paths,
        first,
        end,
    }
}

/// `C:\data`: a folder of pictures, a video and an Arabic-named document.
fn data_map(engine: &'static str) -> UsageMap {
    build_map(
        "C:\\data",
        engine,
        &[
            ("data", true, 1000., NO_PARENT, 0.),
            ("photos", true, 600., 0, 0.),
            ("movie.mp4", false, 250., 0, 40.),
            ("تقرير.pdf", false, 150., 0, 400.),
            ("a.png", false, 350., 1, 0.5),
            ("b.jpg", false, 250., 1, 3.),
        ],
    )
}

/// `C:\data\photos` on its own.
fn photos_map(engine: &'static str) -> UsageMap {
    build_map(
        "C:\\data\\photos",
        engine,
        &[
            ("photos", true, 600., NO_PARENT, 0.),
            ("a.png", false, 350., 0, 0.5),
            ("b.jpg", false, 250., 0, 3.),
        ],
    )
}

// ---- A fake measurement ------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeState {
    maps: HashMap<String, UsageMap>,
    asked: Vec<Ask>,
    cancelled: Vec<String>,
    held: HashSet<String>,
    gates: HashMap<String, (String, async_channel::Sender<bool>)>,
    errors: HashMap<String, String>,
    missing: Vec<String>,
}

/// The fake source: answers with the maps it was given, can hold an answer back, fail it, and sees Stop.
#[derive(Clone, Default)]
struct Fake {
    inner: Arc<Mutex<FakeState>>,
}

fn key(path: &str) -> String {
    path.trim_end_matches('\\').to_lowercase()
}

impl Fake {
    fn with(maps: Vec<UsageMap>) -> Self {
        let fake = Fake::default();
        for map in maps {
            fake.inner.lock().unwrap().maps.insert(key(&map.path), map);
        }
        fake
    }

    fn hold(&self, path: &str) {
        self.inner.lock().unwrap().held.insert(key(path));
    }

    fn release(&self, path: &str) {
        let mut state = self.inner.lock().unwrap();
        state.held.remove(&key(path));
        for (wanted, sender) in state.gates.values() {
            if *wanted == key(path) {
                sender.try_send(true).ok();
            }
        }
    }

    fn fail(&self, path: &str, reason: &str) {
        self.inner.lock().unwrap().errors.insert(key(path), reason.into());
    }

    fn mend(&self, path: &str) {
        self.inner.lock().unwrap().errors.remove(&key(path));
    }

    fn asked(&self) -> Vec<Ask> {
        self.inner.lock().unwrap().asked.clone()
    }

    fn asked_paths(&self) -> Vec<String> {
        self.asked().into_iter().map(|a| a.path).collect()
    }

    fn cancelled(&self) -> Vec<String> {
        self.inner.lock().unwrap().cancelled.clone()
    }

    fn set_missing(&self, paths: &[&str]) {
        self.inner.lock().unwrap().missing = paths.iter().map(|p| p.to_string()).collect();
    }
}

impl Source for Fake {
    fn measure(&self, ask: Ask) -> Fut<Result<UsageMap, String>> {
        let mut state = self.inner.lock().unwrap();
        state.asked.push(ask.clone());
        let wanted = key(&ask.path);
        let held = state.held.contains(&wanted);
        let (sender, receiver) = async_channel::unbounded::<bool>();
        state.gates.insert(ask.lane.clone(), (wanted.clone(), sender));
        let answer = match state.errors.get(&wanted) {
            Some(reason) => Err(reason.clone()),
            None => state
                .maps
                .get(&wanted)
                .cloned()
                .ok_or_else(|| format!("no map for {}", ask.path)),
        };
        Box::pin(async move {
            if held && !matches!(receiver.recv().await, Ok(true)) {
                return Err("cancelled".to_string());
            }
            answer
        })
    }

    fn cancel(&self, lane: &str) {
        let mut state = self.inner.lock().unwrap();
        state.cancelled.push(lane.to_string());
        if let Some((_, sender)) = state.gates.get(lane) {
            sender.try_send(false).ok();
        }
    }

    fn missing(&self, paths: Vec<String>) -> Fut<Vec<String>> {
        let known = self.inner.lock().unwrap().missing.clone();
        Box::pin(async move { paths.into_iter().filter(|p| known.contains(p)).collect() })
    }
}

// ---- The window ---------------------------------------------------------------------------------------------

struct Env {
    visual: VisualTestContext,
    view: Entity<DiskUsage>,
    explorer: Entity<Explorer>,
    usage: Entity<Usage>,
    disk: FakeDisk,
    fake: Fake,
    tab: TabId,
    events: Rc<RefCell<Vec<DiskUsageEvent>>>,
    /// Files the explorer tried to open with their app: the fake disk has no app for any, so each one is
    /// reported as "couldn't open" with its path, which is how the tests see it.
    tried: Rc<RefCell<Vec<String>>>,
}

fn setup(cx: &mut TestAppContext, rtl: bool, fake: Fake) -> Env {
    cx.update(testing::init);
    testing::set_rtl(rtl);
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\data",
        vec![
            folder("C:\\data", "photos"),
            file("C:\\data", "movie.mp4", 250, 1),
            file("C:\\data", "تقرير.pdf", 150, 2),
        ],
    );
    disk.folder(
        "C:\\data\\photos",
        vec![
            file("C:\\data\\photos", "a.png", 350, 1),
            file("C:\\data\\photos", "b.jpg", 250, 2),
        ],
    );
    disk.folder("C:\\", vec![folder("C:\\", "data")]);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let source: Rc<dyn Source> = Rc::new(fake.clone());
    cx.update(|cx| model::init_with(cx, source));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\data", NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let (mut visual, view) = testing::open_window(cx, |window, cx| cx.new(|cx| DiskUsage::new(0, window, cx)));
    let usage = cx.update(shared);
    let tab = explorer.read_with(&visual, |e, _| e.pane_tab_id(0));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &DiskUsageEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let tried = Rc::new(RefCell::new(Vec::new()));
    let sink = tried.clone();
    visual.update(|_, cx| {
        cx.subscribe(&explorer, move |_, event: &ExplorerEvent, _| {
            if let ExplorerEvent::Notice(Notice::CouldNotOpenFile { path, .. }) = event {
                sink.borrow_mut().push(path.clone());
            }
        })
        .detach();
    });
    Env {
        visual,
        view,
        explorer,
        usage,
        disk,
        fake,
        tab,
        events,
        tried,
    }
}

impl Env {
    /// Lets the fake disk, the model's background work and the canvas measurement run until things are still.
    fn settle(&mut self) {
        for _ in 0..12 {
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Settles until `done` holds.
    fn settle_until(&mut self, what: &str, mut done: impl FnMut(&mut Env) -> bool) {
        for _ in 0..300 {
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
            if done(self) {
                self.settle();
                return;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        panic!("never happened: {what}");
    }

    /// Turns the tab's map on and waits until the map is drawn.
    fn show_map(&mut self) {
        let tab = self.tab;
        self.visual.update(|_, cx| set(cx, tab, true));
        self.wait_drawn();
    }

    fn wait_drawn(&mut self) {
        self.settle_until("the map is drawn", |env| {
            let view = env.view.clone();
            let drawn_live = view.read_with(&env.visual, |v, cx| v.live(cx).is_some());
            drawn_live
        });
    }

    fn run(&mut self) -> Option<model::Run> {
        let tab = self.tab;
        self.usage.read_with(&self.visual, |u, _| u.run(tab).cloned())
    }

    fn path(&mut self) -> String {
        self.explorer.read_with(&self.visual, |e, _| {
            e.pane_tab(0).map(|t| t.path.clone()).unwrap_or_default()
        })
    }

    fn selection(&mut self) -> Vec<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.pane_tab(0).map(|t| t.selection.to_vec()).unwrap_or_default()
        })
    }

    fn plan(&mut self) -> Arc<Plan> {
        self.view
            .read_with(&self.visual, |v, _| v.drawn.as_ref().map(|d| d.plan.clone()))
            .expect("a plan is drawn")
    }

    fn shown(&mut self) -> Arc<Shown> {
        self.view
            .read_with(&self.visual, |v, _| v.drawn.as_ref().map(|d| d.shown.clone()))
            .expect("a map is drawn")
    }

    fn has(&mut self, label: &str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn has_id(&mut self, id: &'static str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn role_of(&mut self, label: &str) -> Option<Role> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    /// The middle of node `i` in the map's own pixels, mirrored if the plan is.
    fn middle_of(&mut self, node: usize) -> Point<Pixels> {
        let (x, y, w, h) = self.plan().rect(node);
        point(px(x + w / 2.), px(y + h / 2.))
    }

    fn node(&mut self, path: &str) -> usize {
        self.shown()
            .node_of(path)
            .unwrap_or_else(|| panic!("{path} is not in the map"))
    }

    /// A click in the middle of `path`'s tile (`count` is 1 for a click, 2 for a double-click).
    fn click_node(&mut self, path: &str, count: usize) {
        let node = self.node(path);
        let at = self.middle_of(node);
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click_with_options(
                ("disk-usage-map", 0usize),
                gpui_kit::test::ClickOptions::new().with_offset(at).with_count(count),
                cx,
            );
        });
        self.settle();
    }

    /// The pointer over node `path`'s tile.
    fn hover_node(&mut self, path: &str) {
        let node = self.node(path);
        let at = self.middle_of(node);
        let origin = self.view.read_with(&self.visual, |v, _| v.body.get().origin);
        self.visual.update(|window, cx| {
            window.dispatch_event(
                gpui_kit::MouseMoveEvent {
                    position: origin + at,
                    pressed_button: None,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
    }

    fn right_click_node(&mut self, path: &str) {
        let node = self.node(path);
        let at = self.middle_of(node);
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click_with_options(
                ("disk-usage-map", 0usize),
                gpui_kit::test::ClickOptions::new()
                    .with_offset(at)
                    .with_button(MouseButton::Right),
                cx,
            );
        });
        self.settle();
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.settle();
    }

    fn click(&mut self, id: &'static str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.settle();
    }
}

use gpui_kit::InputEvent as _;

// ---- The map and its legend ---------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_measurement_is_drawn_as_a_map_with_its_legend_and_totals(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();

    let run = env.run().expect("the tab shows its map");
    assert!(run.current().is_some() && !run.running && run.error.is_empty());
    assert_eq!(env.fake.asked_paths(), ["C:\\data"]);
    // The index would have been asked for what grew: this many days back.
    let since = env.fake.asked()[0].since.expect("growth is asked for");
    let week = 7. * DAY_MS;
    assert!((now_ms() as f64 - since - week).abs() < 60_000., "7 days by default");

    // The photos folder is a frame around its two pictures; the other two files are tiles.
    let plan = env.plan();
    let shown = env.shown();
    let named = |nodes: &[u32]| -> Vec<String> { nodes.iter().map(|&n| shown.map.names[n as usize].clone()).collect() };
    assert_eq!(named(&plan.frames), ["photos"]);
    let mut blocks = named(&plan.blocks);
    blocks.sort();
    assert_eq!(blocks, ["a.png", "b.jpg", "movie.mp4", "تقرير.pdf"]);

    // The legend: bytes per kind among the files, biggest first.
    let legend: Vec<_> = plan.legend.iter().map(|(g, bytes)| (g.name, *bytes)).collect();
    assert_eq!(
        legend,
        [("Images", 600.), ("Videos", 250.), ("Documents", 150.)],
        "pictures 350 + 250, the video, the Arabic-named PDF"
    );
    assert!(!plan.whole_folders, "no folder is drawn whole");
    for name in ["Images", "Videos", "Documents"] {
        assert_eq!(env.role_of(name), Some(Role::Button), "{name} is a legend button");
    }
    assert!(!env.has("Folders"), "no Folders entry without folders drawn whole");

    // Names and sizes of the biggest items are the map's accessible name; the Arabic name is not isolated there.
    assert_eq!(
        env.role_of("Disk usage of data: photos 600 B, movie.mp4 250 B, تقرير.pdf 150 B, a.png 350 B, b.jpg 250 B"),
        Some(Role::Image)
    );
    // The status bar's line: who mapped it, what size, how long, and how long after the click it was shown.
    let line = env.visual.update(|_, cx| status(cx, env.tab)).expect("a status line");
    assert!(!line.busy);
    assert!(
        line.text.starts_with("pdu · mapped 1000 B in 1.5 ms · shown in "),
        "{}",
        line.text
    );
    assert!(line
        .tooltip
        .expect("a tooltip")
        .starts_with("pdu mapped this folder in 1.5 ms; it was on screen "));
}

#[gpui_kit::test]
fn tiles_fill_the_map_without_overlapping_and_are_hit_where_they_are(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    let plan = env.plan();
    let shown = env.shown();
    for &node in &plan.blocks {
        let (x, y, w, h) = plan.rect(node as usize);
        assert!(w > 0. && h > 0.);
        assert!(x >= 0. && y >= 0. && x + w <= plan.width + 0.5 && y + h <= plan.height + 0.5);
        // A point in the middle of a tile finds that tile, and nothing else.
        let hit = plan
            .hit(&shown, x + w / 2., y + h / 2.)
            .expect("a tile is under its middle");
        assert_eq!((hit.node, hit.rest), (node as usize, false));
    }
    // Outside the map is nothing.
    assert!(plan.hit(&shown, -1., 5.).is_none());
    // The biggest item of the folder (photos, 600 of 1000) has the most room.
    let area = |n: usize| {
        let (_, _, w, h) = plan.rect(n);
        w * h
    };
    assert!(area(1) > area(2) && area(2) > area(3));
}

#[gpui_kit::test]
fn hovering_an_item_explains_it_and_the_pointer_leaving_clears_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.hover_node("C:\\data\\movie.mp4");
    let shown = env.shown();
    let node = env.node("C:\\data\\movie.mp4");
    let lines = words::tooltip_lines(&shown.map, shown.entry(node), node, false, false);
    let label = lines.iter().map(|l| l.text.to_string()).collect::<Vec<_>>().join(". ");
    // Name, path, "size · share · type · exact bytes", when it changed, and what a click does.
    assert_eq!(lines[0].text, "movie.mp4");
    assert_eq!(lines[1].text, "C:\\data\\movie.mp4");
    assert!(lines[2].text.starts_with("250 B · 25% · "), "{}", lines[2].text);
    assert_eq!(lines.last().expect("a hint").text, "Double-click to open");
    assert_eq!(env.role_of(&label), Some(Role::Tooltip));

    // Over a folder's frame the hint says it maps the folder; over a file inside, it goes to the folder.
    env.hover_node("C:\\data\\photos\\a.png");
    let node = env.node("C:\\data\\photos\\a.png");
    let lines = words::tooltip_lines(&shown.map, shown.entry(node), node, false, false);
    assert_eq!(lines.last().expect("a hint").text, "Click to go to its folder");
    assert!(env.has(&lines.iter().map(|l| l.text.to_string()).collect::<Vec<_>>().join(". ")));

    // Leaving the map takes the tooltip away.
    env.view.update(&mut env.visual, |v, cx| v.pointer_left(cx));
    env.settle();
    assert!(!env.has(&label));
}

// ---- Drilling in and out --------------------------------------------------------------------------------------

#[gpui_kit::test]
fn clicking_a_file_in_a_folder_goes_to_that_folder_and_up_comes_back_without_measuring_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu"), photos_map("pdu")]));
    env.show_map();
    assert_eq!(env.path(), "C:\\data");

    // a.png is inside photos: the click goes to photos with a.png selected, and photos is mapped.
    env.click_node("C:\\data\\photos\\a.png", 1);
    env.settle_until("photos is mapped", |env| {
        env.run()
            .is_some_and(|r| r.path == "C:\\data\\photos" && r.current().is_some() && !r.running)
    });
    assert_eq!(env.path(), "C:\\data\\photos");
    assert_eq!(env.selection(), ["C:\\data\\photos\\a.png"]);
    assert_eq!(env.fake.asked_paths(), ["C:\\data", "C:\\data\\photos"]);
    env.wait_drawn();
    let map = env.shown().map.clone();
    assert_eq!((map.path.as_str(), map.count), ("C:\\data\\photos", 3));

    // Backspace goes up one folder, as Alt+Up does; the map of data pdu measured before is shown again at once.
    env.visual.update(|window, cx| {
        let focus = env.view.read(cx).focus.clone();
        window.focus(&focus, cx)
    });
    env.press("backspace");
    env.settle_until("back in data", |env| {
        env.run().is_some_and(|r| r.path == "C:\\data" && r.current().is_some())
    });
    assert_eq!(env.path(), "C:\\data");
    assert_eq!(
        env.fake.asked_paths(),
        ["C:\\data", "C:\\data\\photos"],
        "a pdu map measured lately is shown again, not measured again"
    );
}

#[gpui_kit::test]
fn clicking_a_folder_maps_it_while_showing_its_part_of_the_map_meanwhile(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::with(vec![data_map("pdu"), photos_map("pdu")]);
    let mut env = setup(cx, false, fake.clone());
    env.show_map();
    fake.hold("C:\\data\\photos");
    // The folder's frame (not a file in it) is the photos folder itself.
    let photos = env.node("C:\\data\\photos");
    let (x, y, _, _) = env.plan().rect(photos);
    let at = point(px(x + 2.), px(y + 2.));
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click_with_options(
            ("disk-usage-map", 0usize),
            gpui_kit::test::ClickOptions::new().with_offset(at),
            cx,
        );
    });
    env.settle_until("the part of the map shows", |env| env.run().is_some_and(|r| r.preview));
    let run = env.run().expect("a run");
    assert!(
        run.running && run.preview,
        "pdu measures while photos' part of the old map is shown"
    );
    assert_eq!(
        run.shown.as_ref().map(|s| s.map.path.clone()).as_deref(),
        Some("C:\\data\\photos")
    );
    let line = env.visual.update(|_, cx| status(cx, env.tab)).expect("a line");
    assert!(line.busy && line.text == "pdu · measuring…");
    fake.release("C:\\data\\photos");
    env.settle_until("the measurement arrives", |env| env.run().is_some_and(|r| !r.running));
    assert!(!env.run().expect("a run").preview);
}

#[gpui_kit::test]
fn a_file_at_the_top_is_selected_by_a_click_and_opened_by_the_second(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.click_node("C:\\data\\movie.mp4", 1);
    assert_eq!(env.selection(), ["C:\\data\\movie.mp4"]);
    assert!(env.tried.borrow().is_empty(), "one click only selects");
    assert_eq!(env.path(), "C:\\data", "and a file at the top stays in this folder");
    env.click_node("C:\\data\\movie.mp4", 2);
    env.settle();
    assert_eq!(
        *env.tried.borrow(),
        ["C:\\data\\movie.mp4"],
        "the second click opens it with its app"
    );
}

#[gpui_kit::test]
fn the_keys_step_through_the_items_open_and_clear(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    // Give the map the keyboard by clicking its empty root, which also clears the selection.
    env.visual.update(|window, cx| {
        let focus = env.view.read(cx).focus.clone();
        window.focus(&focus, cx)
    });
    env.press("down");
    assert_eq!(env.selection(), ["C:\\data\\photos"], "the biggest first");
    env.press("down");
    assert_eq!(env.selection(), ["C:\\data\\movie.mp4"]);
    env.press("end");
    assert_eq!(env.selection(), ["C:\\data\\تقرير.pdf"]);
    env.press("up");
    assert_eq!(env.selection(), ["C:\\data\\movie.mp4"]);
    env.press("home");
    assert_eq!(env.selection(), ["C:\\data\\photos"]);
    // Escape clears the selection.
    env.press("escape");
    assert!(env.selection().is_empty());
    // Enter opens what is selected: a file opens in its app.
    env.press("right");
    env.press("right");
    assert_eq!(env.selection(), ["C:\\data\\movie.mp4"]);
    env.press("enter");
    assert_eq!(*env.tried.borrow(), ["C:\\data\\movie.mp4"]);
}

// ---- T, F5 and the colors ----------------------------------------------------------------------------------

#[gpui_kit::test]
fn t_goes_round_type_age_and_growth_and_the_legend_follows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.visual.update(|window, cx| {
        let focus = env.view.read(cx).focus.clone();
        window.focus(&focus, cx)
    });
    assert_eq!(env.view.read_with(&env.visual, |v, _| v.color_mode()), ColorMode::Type);
    env.press("t");
    assert_eq!(env.view.read_with(&env.visual, |v, _| v.color_mode()), ColorMode::Age);
    env.settle_until("the age plan", |env| env.plan().mode == ColorMode::Age);
    // a.png changed 12 hours ago, b.jpg 3 days, movie.mp4 40 days, the PDF over a year: four bands, hot to frozen.
    let names: Vec<_> = env.plan().legend.iter().map(|(g, _)| g.name).collect();
    assert_eq!(names, ["Hot (<24h)", "Recent (≤7d)", "Cool (≤6m)", "Frozen (>1y)"]);
    assert_eq!(env.role_of("Hot (<24h)"), Some(Role::Button));
    assert_eq!(env.role_of("Age"), Some(Role::RadioButton));
    env.press("t");
    assert_eq!(
        env.view.read_with(&env.visual, |v, _| v.color_mode()),
        ColorMode::Growth
    );
    // pdu can't tell what grew: types stay, and the strip says where growth comes from.
    env.settle_until("the growth plan", |env| env.plan().mode == ColorMode::Growth);
    assert!(!env.plan().growing);
    assert!(env.has("What grew"));
    env.press("t");
    assert_eq!(env.view.read_with(&env.visual, |v, _| v.color_mode()), ColorMode::Type);
}

#[gpui_kit::test]
fn growth_colors_show_what_grew_and_the_days_picker_maps_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mb = f64::from(1u32 << 20);
    let mut map = build_map(
        "C:\\data",
        "index",
        &[
            ("data", true, 10. * mb, NO_PARENT, 0.),
            ("cache", true, 6. * mb, 0, 0.),
            ("old.bin", false, 3. * mb, 0, 40.),
            ("new.log", false, mb, 0, 0.),
        ],
    );
    map.growth = Some(UsageGrowth {
        since: now_ms() as f64 - DAY_MS,
        oldest: now_ms() as f64 - 20. * DAY_MS,
        drive_then: 100. * mb,
        drive_now: 108. * mb,
        previous: vec![6. * mb, 2. * mb, 3. * mb, WAS_NOT_THERE],
    });
    let mut env = setup(cx, false, Fake::with(vec![map]));
    env.show_map();
    env.visual.update(|_, cx| {
        env.view.update(cx, |v, cx| v.set_mode(ColorMode::Growth, cx));
    });
    env.settle_until("the growth plan", |env| env.plan().mode == ColorMode::Growth);
    let plan = env.plan();
    assert!(plan.growing);
    let names: Vec<_> = plan.legend.iter().map(|(g, _)| g.name).collect();
    // new.log is new; cache tripled (+4 MB); old.bin is the same.
    assert_eq!(names, ["New", "Doubled or more", "Same"]);
    assert_eq!(env.role_of("Doubled or more"), Some(Role::Button));
    // The strip: the day choices (7 chosen) and the headline.
    assert_eq!(env.role_of("What grew in the last"), Some(Role::RadioGroup));
    assert_eq!(env.role_of("7 days"), Some(Role::RadioButton));
    let before = env.fake.asked().len();
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(("disk-usage-days", 30usize), cx);
    });
    env.settle();
    assert_eq!(env.visual.update(|_, cx| crate::state::settings(cx).growth_days), 30);
    let asked = env.fake.asked();
    assert_eq!(asked.len(), before + 1, "the folder is mapped again for the new span");
    let since = asked.last().and_then(|a| a.since).expect("since");
    assert!((now_ms() as f64 - since - 30. * DAY_MS).abs() < 60_000.);
    // Growth in words.
    let (shown, headline) = (env.shown(), words::growth_headline("data", 4. * mb, 0.));
    assert!(headline.starts_with("data grew 4.0 MB since "), "{headline}");
    assert_eq!(
        words::growth_line(&shown.map, 3).unwrap_or_default().split(' ').next(),
        Some("New")
    );
}

#[gpui_kit::test]
fn f5_reads_the_folder_again_and_measures_it_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.visual.update(|window, cx| {
        let focus = env.view.read(cx).focus.clone();
        window.focus(&focus, cx)
    });
    let listed = env.disk.listed().len();
    assert_eq!(env.fake.asked().len(), 1);
    env.press("f5");
    env.settle_until("measured again", |env| env.fake.asked().len() == 2);
    assert!(env.disk.listed().len() > listed, "the folder is read again too");
    assert!(env.fake.asked()[1].path == "C:\\data");
}

// ---- Stop, failures, leaving -----------------------------------------------------------------------------------

#[gpui_kit::test]
fn stop_ends_a_slow_measurement_and_measure_again_runs_it_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::with(vec![data_map("pdu")]);
    fake.hold("C:\\data");
    let mut env = setup(cx, false, fake.clone());
    let tab = env.tab;
    env.visual.update(|_, cx| set(cx, tab, true));
    env.settle();
    // pdu is working: the page says so, with a way to stop it, and nothing is drawn yet.
    let run = env.run().expect("a run");
    assert!(run.running && run.shown.is_none());
    assert_eq!(env.role_of("Measuring “data” with pdu…"), Some(Role::Status));
    assert_eq!(env.role_of("Stop"), Some(Role::Button));
    assert_eq!(
        env.visual
            .update(|_, cx| status(cx, tab))
            .map(|l| (l.busy, l.text.to_string())),
        Some((true, "pdu · measuring…".to_string()))
    );
    env.click("disk-usage-stop");
    env.settle_until("stopped", |env| env.run().is_some_and(|r| !r.running));
    assert_eq!(fake.cancelled(), [tab.to_string()]);
    let run = env.run().expect("a run");
    assert!(run.error.is_empty(), "Stop is not a failure");
    assert_eq!(env.role_of("Measuring stopped."), Some(Role::Status));
    assert_eq!(env.role_of("Measure again"), Some(Role::Button));
    // Measure again: it runs once more, and this time it is let through.
    fake.release("C:\\data");
    env.click("disk-usage-measure-again");
    env.wait_drawn();
    assert_eq!(fake.asked().len(), 2);
    assert!(env.run().is_some_and(|r| r.current().is_some()));
}

#[gpui_kit::test]
fn a_failed_measurement_says_why_and_try_again_works(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::with(vec![data_map("pdu")]);
    fake.fail("C:\\data", "Failed to start pdu: not found");
    let mut env = setup(cx, false, fake.clone());
    let tab = env.tab;
    env.visual.update(|_, cx| set(cx, tab, true));
    env.settle_until("the failure", |env| env.run().is_some_and(|r| !r.error.is_empty()));
    assert_eq!(env.role_of("Couldn't map “data”"), Some(Role::Alert));
    assert_eq!(
        env.run().map(|r| r.error).as_deref(),
        Some("Failed to start pdu: not found")
    );
    assert!(
        env.visual.update(|_, cx| status(cx, tab)).is_none(),
        "nothing to say about a map that isn't there"
    );
    fake.mend("C:\\data");
    env.click("disk-usage-retry");
    env.wait_drawn();
    assert!(env.run().is_some_and(|r| r.error.is_empty() && r.current().is_some()));
}

#[gpui_kit::test]
fn an_empty_folder_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let empty = build_map("C:\\data", "pdu", &[("data", true, 0., NO_PARENT, 0.)]);
    let mut env = setup(cx, false, Fake::with(vec![empty]));
    let tab = env.tab;
    env.visual.update(|_, cx| set(cx, tab, true));
    env.settle_until("the map", |env| env.run().is_some_and(|r| r.current().is_some()));
    assert_eq!(env.role_of("This folder is empty."), Some(Role::Status));
}

#[gpui_kit::test]
fn toggling_a_tab_off_stops_pdu_and_closing_the_tab_drops_the_map(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::with(vec![data_map("pdu")]);
    fake.hold("C:\\data");
    let mut env = setup(cx, false, fake.clone());
    let tab = env.tab;
    assert!(!env.visual.update(|_, cx| shows(cx, tab)));
    env.visual.update(|_, cx| toggle(cx, tab));
    env.settle();
    assert!(env.visual.update(|_, cx| shows(cx, tab) && visible(cx, tab)));
    env.visual.update(|_, cx| toggle(cx, tab));
    env.settle();
    assert!(!env.visual.update(|_, cx| shows(cx, tab)), "back to the list");
    assert_eq!(fake.cancelled(), [tab.to_string()], "the pdu run that was going stops");

    // A second tab's map goes when the tab closes.
    env.visual
        .update(|_, cx| env.explorer.update(cx, |e, cx| e.new_tab(Some("C:\\data"), cx)));
    env.settle();
    let second = env.explorer.read_with(&env.visual, |e, _| e.active_tab_id());
    assert_ne!(second, tab);
    fake.release("C:\\data");
    env.visual.update(|_, cx| set(cx, second, true));
    env.settle();
    assert!(env.visual.update(|_, cx| shows(cx, second)));
    env.visual
        .update(|_, cx| env.explorer.update(cx, |e, cx| e.close_tab(Some(second), cx)));
    env.settle();
    assert!(
        !env.visual.update(|_, cx| shows(cx, second)),
        "the closed tab took its map along"
    );
}

#[gpui_kit::test]
fn this_pc_and_search_results_keep_their_own_views(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    let tab = env.tab;
    env.visual.update(|_, cx| set(cx, tab, true));
    env.settle();
    assert!(env.visual.update(|_, cx| visible(cx, tab)));
    let task = env.explorer.update(&mut env.visual, |e, cx| {
        e.navigate_task(glint_model::paths::THIS_PC, NavOptions::default(), cx)
    });
    env.visual.foreground_executor().block_test(task);
    env.settle();
    assert!(
        env.visual.update(|_, cx| shows(cx, tab) && !visible(cx, tab)),
        "the run waits; This PC draws This PC"
    );
}

#[gpui_kit::test]
fn renaming_hands_over_to_the_list(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.click_node("C:\\data\\movie.mp4", 1);
    let tab = env.tab;
    env.visual.update(|_, cx| {
        env.explorer
            .update(cx, |e, cx| e.begin_rename(Some("C:\\data\\movie.mp4"), cx))
    });
    env.settle();
    assert!(
        !env.visual.update(|_, cx| shows(cx, tab)),
        "names are edited in the list"
    );
    assert_eq!(
        env.explorer
            .read_with(&env.visual, |e, _| e.pane_tab(0).and_then(|t| t.renaming.clone()))
            .as_deref(),
        Some("C:\\data\\movie.mp4")
    );
}

// ---- Menus and what the bar asks ------------------------------------------------------------------------------------

#[gpui_kit::test]
fn right_click_selects_the_item_and_asks_for_its_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.right_click_node("C:\\data\\photos\\b.jpg");
    assert_eq!(env.selection(), ["C:\\data\\photos\\b.jpg"]);
    let events = env.events.borrow().clone();
    let [DiskUsageEvent::ItemMenu { entries, .. }] = events.as_slice() else {
        panic!("one item menu, got {events:?}")
    };
    assert_eq!(
        entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
        ["C:\\data\\photos\\b.jpg"]
    );
    env.events.borrow_mut().clear();
}

#[gpui_kit::test]
fn right_click_where_no_item_is_asks_for_the_folder_s_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    // The folder holds 1200 but its items add up to 1000: the rest is "smaller items" of the folder itself.
    let mut map = data_map("pdu");
    map.size[0] = 1200.;
    let mut env = setup(cx, false, Fake::with(vec![map]));
    env.show_map();
    let (x, y, w, h) = env.plan().rest_rect(0);
    assert!(w > 0. && h > 0., "the folder has a room for its smaller items");
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click_with_options(
            ("disk-usage-map", 0usize),
            gpui_kit::test::ClickOptions::new()
                .with_offset(point(px(x + w / 2.), px(y + h / 2.)))
                .with_button(MouseButton::Right),
            cx,
        );
    });
    let events = env.events.borrow().clone();
    assert!(
        matches!(events.as_slice(), [DiskUsageEvent::FolderMenu { folder, .. }] if folder == "C:\\data"),
        "{events:?}"
    );
    // A left click there clears the selection rather than mapping anything.
    env.click_node("C:\\data\\movie.mp4", 1);
    assert_eq!(env.selection().len(), 1);
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click_with_options(
            ("disk-usage-map", 0usize),
            gpui_kit::test::ClickOptions::new().with_offset(point(px(x + w / 2.), px(y + h / 2.))),
            cx,
        );
    });
    env.settle();
    assert!(env.selection().is_empty());
    assert_eq!(env.path(), "C:\\data");
}

#[gpui_kit::test]
fn the_menu_key_asks_for_the_selection_s_menu_at_its_corner(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.click_node("C:\\data\\movie.mp4", 1);
    env.view.update(&mut env.visual, |v, cx| v.keyboard_menu(cx));
    let events = env.events.borrow().clone();
    let (x, y, _, h) = env.plan().rect(env.node("C:\\data\\movie.mp4"));
    let origin = env.view.read_with(&env.visual, |v, _| v.body.get().origin);
    let [DiskUsageEvent::ItemMenu { entries, position }] = events.as_slice() else {
        panic!("one item menu, got {events:?}")
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(*position, origin + point(px(x), px(y + h)), "the bottom start corner");
}

#[gpui_kit::test]
fn free_up_space_and_the_instant_search_buttons_ask_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut old = data_map("pdu");
    old.service_too_old = true;
    let mut env = setup(cx, false, Fake::with(vec![old]));
    env.show_map();
    assert_eq!(
        env.role_of("Free up space"),
        Some(Role::Button),
        "the button's name is its label"
    );
    env.click("disk-usage-free-up");
    env.click("disk-usage-update-service");
    let events = env.events.borrow().clone();
    assert_eq!(
        events,
        [
            DiskUsageEvent::FreeUpSpace {
                folder: "C:\\data".into()
            },
            DiskUsageEvent::EnableInstantSearch
        ]
    );
}

#[gpui_kit::test]
fn the_legend_spotlights_a_kind_on_hover_and_keeps_it_on_click(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    let plan = env.plan();
    let shown = env.shown();
    let images = plan.spotlight(&shown, "Images");
    let mut paths: Vec<_> = images.iter().map(|&n| shown.map.paths[n as usize].clone()).collect();
    paths.sort();
    assert_eq!(paths, ["C:\\data\\photos\\a.png", "C:\\data\\photos\\b.jpg"]);
    assert!(plan.spotlight(&shown, "Folders").is_empty());
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.hover(("disk-usage-legend", 0usize), cx);
    });
    assert_eq!(
        env.view.read_with(&env.visual, |v, _| v.hover_spot.clone()).as_deref(),
        Some("Images")
    );
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(("disk-usage-legend", 1usize), cx);
    });
    assert_eq!(
        env.view.read_with(&env.visual, |v, _| v.pinned_spot.clone()).as_deref(),
        Some("Videos")
    );
    assert_eq!(env.role_of("Videos"), Some(Role::Button));
}

// ---- The index: remaps, and what a delete takes out ------------------------------------------------------------------

#[gpui_kit::test]
fn an_index_map_is_made_again_after_the_folder_is_read_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("index")]));
    env.usage
        .update(&mut env.visual, |u, _| u.set_remap_after(Duration::from_millis(10)));
    env.show_map();
    assert_eq!(env.fake.asked().len(), 1);
    // The folder is read again (a file was pasted, say): a moment later the index map is asked for again.
    let tab = env.tab;
    env.explorer
        .update(&mut env.visual, |e, cx| e.reload(Some(tab), cx).detach());
    env.settle();
    // The wait is on the test clock.
    env.visual.executor().advance_clock(Duration::from_millis(20));
    env.settle_until("the map is made again", |env| env.fake.asked().len() >= 2);
}

#[gpui_kit::test]
fn a_pdu_map_stays_as_measured_when_the_folder_is_read_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.usage
        .update(&mut env.visual, |u, _| u.set_remap_after(Duration::from_millis(10)));
    env.show_map();
    let tab = env.tab;
    env.explorer
        .update(&mut env.visual, |e, cx| e.reload(Some(tab), cx).detach());
    env.settle();
    env.visual.executor().advance_clock(Duration::from_secs(5));
    env.settle();
    assert_eq!(env.fake.asked().len(), 1, "a pdu map stays as measured until F5");
}

#[gpui_kit::test]
fn what_was_selected_and_is_gone_leaves_the_map_at_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    env.click_node("C:\\data\\movie.mp4", 1);
    // The item is deleted: the folder is read again, the selection is gone, the disk says the file is missing.
    env.fake.set_missing(&["C:\\data\\movie.mp4"]);
    let tab = env.tab;
    env.explorer
        .update(&mut env.visual, |e, cx| e.reload(Some(tab), cx).detach());
    env.settle_until("the item is gone from the map", |env| {
        env.run()
            .and_then(|r| r.shown)
            .is_some_and(|s| s.node_of("C:\\data\\movie.mp4").is_none())
    });
    let run = env.run().expect("a run");
    let shown = run.shown.expect("a map");
    assert_eq!(shown.map.count, 5);
    // Folders shrink with it: the folder mapped was 1000, minus the video's 250.
    assert_eq!(shown.map.size[0], 750.);
    assert_eq!(env.fake.asked().len(), 1, "no new measuring: the map was cut");
}

// ---- Arabic ------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn in_arabic_the_map_mirrors_and_names_stay_whole(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, true, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    let plan = env.plan();
    assert!(plan.rtl);
    // The biggest item (photos) sits at the top right: its right edge is the map's.
    let (x, y, w, _) = plan.rect(1);
    assert!(
        (x + w - plan.width).abs() < 1.5 && y < 5.,
        "photos at the top right: {x} {y} {w}"
    );
    // A click where the tile is, mirrored, finds it; the unmirrored point finds the other side.
    let shown = env.shown();
    let movie = env.node("C:\\data\\movie.mp4");
    let middle = env.middle_of(movie);
    let node = plan
        .hit(&shown, f32::from(middle.x), f32::from(middle.y))
        .map(|h| h.node);
    assert_eq!(node, Some(movie));
    // The Arabic name is in the accessible name as it is, and the tooltip over it says its own name.
    assert!(env.has("Disk usage of data: photos 600 B, movie.mp4 250 B, تقرير.pdf 150 B, a.png 350 B, b.jpg 250 B"));
    env.click_node("C:\\data\\تقرير.pdf", 1);
    assert_eq!(env.selection(), ["C:\\data\\تقرير.pdf"]);
    env.hover_node("C:\\data\\تقرير.pdf");
    let node = env.node("C:\\data\\تقرير.pdf");
    let lines = words::tooltip_lines(&shown.map, shown.entry(node), node, false, false);
    assert_eq!(lines[0].text, "تقرير.pdf");
    assert!(env.has(&lines.iter().map(|l| l.text.to_string()).collect::<Vec<_>>().join(". ")));
    // The arrows swap sides: Left is the next item in Arabic.
    env.visual.update(|window, cx| {
        let focus = env.view.read(cx).focus.clone();
        window.focus(&focus, cx)
    });
    env.press("home");
    env.press("left");
    assert_eq!(
        env.selection(),
        ["C:\\data\\movie.mp4"],
        "Left goes forward in a right-to-left interface"
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_labels_of_an_arabic_name_are_shaped_and_placed_at_the_start_edge(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, true, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    // The first paint shapes the labels; the cache holds the lines drawn.
    env.settle();
    let nodes = {
        let cache = env.view.read_with(&env.visual, |v, _| v.labels.clone());
        let nodes = cache.borrow().nodes();
        nodes
    };
    assert!(!nodes.is_empty(), "labels are shaped on the first paint");
    let arabic = env.node("C:\\data\\تقرير.pdf") as u32;
    let photos = env.node("C:\\data\\photos") as u32;
    let plan = env.plan();
    // The Arabic file's tile is wide and tall enough for a name; photos' frame has a header.
    assert!(plan.labels.contains(&arabic) && plan.labels.contains(&photos));
    assert!(nodes.contains(&arabic) && nodes.contains(&photos), "{nodes:?}");
    testing::set_rtl(false);
}

// ---- Files dropped on the map ------------------------------------------------------------------------------------

/// The window's drop target's events for files dragged in, as `drops::dispatch` hands them over.
struct Dragging {
    answers: crate::platform::dragdrop::DropAnswers,
    files: Vec<String>,
}

impl Dragging {
    fn new(files: &[&str]) -> Self {
        Dragging {
            answers: Default::default(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    fn over(&self, env: &mut Env, at: Point<Pixels>, first: bool) -> crate::platform::dragdrop::Answer {
        use crate::platform::dragdrop::DropEvent;
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            if first {
                crate::drops::dispatch(DropEvent::Leave, &self.answers, window, cx);
            }
            let event = if first {
                DropEvent::Enter {
                    paths: self.files.clone(),
                    x,
                    y,
                    effect: DropEffect::Copy,
                }
            } else {
                DropEvent::Over {
                    x,
                    y,
                    effect: DropEffect::Copy,
                }
            };
            crate::drops::dispatch(event, &self.answers, window, cx);
            window.render_frame(cx);
        });
        self.answers.get()
    }

    fn drop_at(&self, env: &mut Env, at: Point<Pixels>, effect: DropEffect) {
        use crate::platform::dragdrop::DropEvent;
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        env.visual.update(|window, cx| {
            crate::drops::dispatch(
                DropEvent::Drop {
                    paths: self.files.clone(),
                    x,
                    y,
                    effect,
                },
                &self.answers,
                window,
                cx,
            );
        });
        env.visual.run_until_parked();
    }
}

#[gpui_kit::test]
fn files_dropped_on_the_map_go_into_the_folder_mapped(cx: &mut TestAppContext) {
    use crate::explorer::FileOp;
    use crate::platform::dragdrop::Answer;
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu")]));
    env.show_map();
    let origin = env.view.read_with(&env.visual, |v, _| v.body.get().origin);
    let node = env.node("C:\\data\\movie.mp4");
    let on_a_tile = origin + env.middle_of(node);
    let outside = "C:\\other\\ملف.txt";
    let files = Dragging::new(&[outside]);

    // Over the map (a tile or not, the folder mapped takes them): the drag says "Copy to data", and the map is outlined.
    let Answer::Accept(under) = files.over(&mut env, on_a_tile, true) else {
        panic!("the map takes files")
    };
    assert_eq!(
        (under.name.as_str(), under.copy.as_str(), under.always_move),
        ("data", "Copy to %1", false)
    );
    assert!(env.view.read_with(&env.visual, |v, _| v.drop_over));
    assert!(env.has_id("disk-usage-drop"));
    files.drop_at(&mut env, on_a_tile, DropEffect::Copy);
    assert_eq!(
        env.disk.ops(),
        [FileOp::Copy {
            sources: vec![outside.into()],
            dest: "C:\\data".into()
        }]
    );
    assert!(
        !env.view.read_with(&env.visual, |v, _| v.drop_over),
        "nothing outlined after the drop"
    );

    // Shift moves.
    files.over(&mut env, on_a_tile, true);
    files.drop_at(&mut env, on_a_tile, DropEffect::Move);
    assert_eq!(env.disk.ops().len(), 2);
    assert!(matches!(env.disk.ops()[1], FileOp::Move { .. }));

    // Files already in the folder have nothing to do there; outside the map nothing is answered.
    let here = Dragging::new(&["C:\\data\\movie.mp4"]);
    assert_eq!(here.over(&mut env, on_a_tile, true), Answer::Refuse);
    assert!(!env.view.read_with(&env.visual, |v, _| v.drop_over));
    assert_eq!(files.over(&mut env, point(px(-5.), px(-5.)), true), Answer::Refuse);
}

// ---- Pieces --------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_icons_the_map_draws_are_in_the_embedded_assets(_cx: &mut TestAppContext) {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    for name in [
        IconName::Layers,
        IconName::Flame,
        IconName::TrendingUp,
        IconName::BrushCleaning,
        IconName::Sparkles,
        IconName::Square,
        IconName::TriangleAlert,
        IconName::Zap,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

#[gpui_kit::test]
fn the_pdu_maps_kept_to_show_again_are_the_last_four_one_per_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::default());
    let map_of = |folder: &str| Arc::new(Shown::new(build_map(folder, "pdu", &[("f", true, 1., NO_PARENT, 0.)])));
    env.usage.update(&mut env.visual, |usage, _| {
        for n in 0..6 {
            usage.remember(map_of(&format!("C:\\f{n}")));
        }
        usage.remember(map_of("C:\\F3"));
    });
    let kept: Vec<String> = env.usage.read_with(&env.visual, |usage, _| {
        usage.measured().iter().map(|s| s.map.path.clone()).collect()
    });
    // f0 and f1 were pushed out; f3 came again (any case) and moved to the end.
    assert_eq!(kept, ["C:\\f2", "C:\\f4", "C:\\f5", "C:\\F3"]);
}

#[gpui_kit::test]
fn the_explorer_s_events_reach_the_model_once_a_listing_changes_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, Fake::with(vec![data_map("pdu"), photos_map("pdu")]));
    env.show_map();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let explorer = env.explorer.clone();
    env.visual.update(|_, cx| {
        cx.subscribe(&explorer, move |_, event: &ExplorerEvent, _| {
            if let ExplorerEvent::ListingChanged { .. } = event {
                sink.borrow_mut().push(());
            }
        })
        .detach();
    });
    env.explorer.update(&mut env.visual, |e, cx| {
        e.navigate("C:\\data\\photos", NavOptions::default(), cx)
    });
    env.settle_until("photos is mapped", |env| {
        env.run()
            .is_some_and(|r| r.path == "C:\\data\\photos" && r.current().is_some())
    });
    assert!(!seen.borrow().is_empty());
}
