//! The head of a pane: what it says of a folder, and that a chip narrows the list to its kind and back.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::NavOptions;
use crate::testing;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\Work",
        vec![
            folder("C:\\Work", "src"),
            folder("C:\\Work", "docs"),
            file("C:\\Work", "a.png", 1000, 0),
            file("C:\\Work", "b.png", 2000, 0),
            file("C:\\Work", "notes.txt", 24, 0),
        ],
    );
    disk.folder("C:\\Work\\src", Vec::new());
    disk
}

/// A window holding the head of pane 0, with `C:\Work` open.
fn open(cx: &mut TestAppContext) -> (Entity<PaneHead>, VisualTestContext) {
    let disk = disk();
    cx.update(|cx| {
        testing::init(cx);
        explorer::init_with(cx, disk.source());
    });
    let (head, visual) = cx.add_window_view(|window, cx| PaneHead::new(0, window, cx));
    let mut visual = visual.clone();
    go(&mut visual, "C:\\Work");
    (head, visual)
}

fn go(visual: &mut VisualTestContext, path: &str) {
    let path = path.to_string();
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx)));
    visual.run_until_parked();
}

#[gpui_kit::test]
fn it_names_the_folder_and_counts_what_is_in_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual) = open(cx);
    let said = visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()));
    let said = said.expect("a folder has a head");
    assert_eq!(said.title.as_ref(), "Work");
    assert_eq!(said.facts.as_ref(), "2 folders · 3 files · 3.0 KB");
    // The kinds, commonest first; two of each of the first two, in the order they came.
    let names: Vec<&str> = said.kinds.iter().map(|kind| kind.group.name).collect();
    assert_eq!(names[..2], ["Folders", "Images"], "{names:?}");
    assert_eq!(said.kinds.iter().map(|kind| kind.count).sum::<usize>(), 5);
}

#[gpui_kit::test]
fn hidden_items_the_list_leaves_out_are_not_counted(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk();
    // A repository: its `.git` is hidden, and with "Show hidden items" off the list shows only the file.
    disk.folder(
        "C:\\Repo",
        vec![
            crate::explorer::fake::hidden("C:\\Repo", ".git"),
            file("C:\\Repo", "a.txt", 10, 0),
        ],
    );
    cx.update(|cx| {
        testing::init(cx);
        explorer::init_with(cx, disk.source());
    });
    let (head, visual) = cx.add_window_view(|window, cx| PaneHead::new(0, window, cx));
    let mut visual = visual.clone();
    go(&mut visual, "C:\\Repo");
    let said = visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()));
    let said = said.expect("a folder has a head");
    assert_eq!(said.facts.as_ref(), "1 file · 10 B");
    assert_eq!(said.kinds.iter().map(|kind| kind.count).sum::<usize>(), 1, "{:?}", said.kinds);
}

#[gpui_kit::test]
fn an_empty_folder_says_so_and_this_pc_and_a_search_have_no_head(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual) = open(cx);
    go(&mut visual, "C:\\Work\\src");
    let said = visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()));
    assert_eq!(said.expect("a head").facts.as_ref(), "Empty");
    // This PC has its own page.
    go(&mut visual, glint_model::paths::THIS_PC);
    assert!(visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).is_none())));
    // A search's results are headed by the search bar.
    go(&mut visual, "C:\\Work");
    visual
        .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.run_find("png", explorer::Wider::default(), cx)))
        .detach();
    visual.run_until_parked();
    assert!(visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).is_none())));
}

#[gpui_kit::test]
fn a_chip_shows_only_its_kind_and_pressed_again_everything(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual) = open(cx);
    let filter = |visual: &mut VisualTestContext| {
        visual.update(|_, cx| {
            let explorer = explorer::explorer(cx).read(cx);
            explorer.active_tab().map(|tab| tab.filter.clone()).unwrap_or_default()
        })
    };
    // The chips are in the kinds' order: Folders, then Images.
    let press = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("pane-head-chip", 1usize), cx);
        });
        visual.run_until_parked();
    };
    let label = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find(("pane-head-chip", 1usize)).label().map(str::to_string)
    });
    assert_eq!(label.as_deref(), Some("Images: 2"));
    press(&mut visual);
    let images = filter(&mut visual);
    assert!(!images.is_empty(), "the kind's filter is in the box");
    let said = visual
        .update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()))
        .expect("a head");
    assert_eq!(said.facts.as_ref(), "2 of 5 shown");
    assert_eq!(said.filter, images);
    // Again: everything.
    press(&mut visual);
    assert_eq!(filter(&mut visual), "");
}

#[test]
fn only_the_chips_that_fit_are_drawn() {
    let kind = |name: &'static str, count: usize| Category {
        group: glint_model::file_types::TypeGroup { name, hue: 0, sat: 0 },
        count,
        bytes: 0,
        query: String::new(),
    };
    let kinds = [kind("Folders", 6), kind("Code and text", 6), kind("Images", 2)];
    // No room, no chips; room for one; room for all.
    assert_eq!(fitting(&kinds, 0., 16.), 0);
    assert_eq!(fitting(&kinds, 120., 16.), 1);
    assert_eq!(fitting(&kinds, 300., 16.), 2);
    assert_eq!(fitting(&kinds, 2000., 16.), 3);
    // Never more than the commonest few.
    let many: Vec<Category> = (0..12).map(|_| kind("Images", 1)).collect();
    assert_eq!(fitting(&many, 10_000., 16.), CHIPS);
}

#[test]
fn the_band_is_the_whole_width_and_no_part_is_too_thin_to_see() {
    let kind = |count: usize| Category {
        group: FOLDERS,
        count,
        bytes: 0,
        query: String::new(),
    };
    let parts = shares(&[kind(990), kind(9), kind(1)]);
    assert!((parts.iter().sum::<f32>() - 1.).abs() < 0.001, "{parts:?}");
    assert!(parts.iter().all(|share| *share >= 0.014), "{parts:?}");
    assert!(parts[0] > 0.9);
    assert!(shares(&[]).is_empty());
}

// ---- The smart buttons ----------------------------------------------------------------------------------------

use super::lenses::{Lens, MAP_LOOKS};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Folders for the buttons: no pictures, three, twenty, a few with places, and a repository.
fn photo_disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder("C:\\Empty", Vec::new());
    let pictures = |dir: &str, n: usize| {
        (0..n)
            .map(|i| file(dir, &format!("p{i}.jpg"), 1000, 0))
            .collect::<Vec<_>>()
    };
    disk.folder("C:\\Three", pictures("C:\\Three", 3));
    disk.folder("C:\\Twenty", pictures("C:\\Twenty", 20));
    disk.folder("C:\\Trip", pictures("C:\\Trip", 5));
    disk.folder(
        "C:\\Repo",
        vec![file("C:\\Repo", "main.rs", 10, 0), folder("C:\\Repo", "src")],
    );
    disk.folder("C:\\Docs", vec![file("C:\\Docs", "a.txt", 10, 0)]);
    disk
}

/// How many times each probe was asked.
#[derive(Clone, Default)]
struct Asked {
    places: Arc<AtomicUsize>,
    history: Arc<AtomicUsize>,
}

/// Probes that say every picture of "C:\Trip" has a place and that "C:\Repo" has a history, and count the asks.
fn answer(asked: &Asked) -> Probes {
    let (places, history) = (asked.places.clone(), asked.history.clone());
    Probes {
        places: Arc::new(move |files| {
            places.fetch_add(1, Ordering::SeqCst);
            if files.iter().any(|(path, ..)| path.starts_with("C:\\Trip")) {
                files.len()
            } else {
                0
            }
        }),
        history: Arc::new(move |folder| {
            history.fetch_add(1, Ordering::SeqCst);
            folder == "C:\\Repo"
        }),
        ..Probes::quiet()
    }
}

fn open_photos(cx: &mut TestAppContext) -> (Entity<PaneHead>, VisualTestContext, Asked) {
    let disk = photo_disk();
    cx.update(|cx| {
        testing::init(cx);
        explorer::init_with(cx, disk.source());
    });
    let (head, visual) = cx.add_window_view(|window, cx| PaneHead::new(0, window, cx));
    let asked = Asked::default();
    let mut visual = visual.clone();
    visual.update(|_, cx| head.update(cx, |head, _| head.probes = answer(&asked)));
    (head, visual, asked)
}

/// The buttons the head draws now, in order.
fn lenses_of(head: &Entity<PaneHead>, visual: &mut VisualTestContext) -> Vec<Lens> {
    visual.update(|window, cx| {
        window.render_frame(cx);
        head.update(cx, |head, cx| {
            let said = head.head(cx).cloned().expect("a head");
            head.lenses_now(&said, cx).into_iter().map(|(lens, _)| lens).collect()
        })
    })
}

/// Whether the pill of `lens` is on screen.
fn drawn(visual: &mut VisualTestContext, lens: Lens) -> bool {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find((lens.id(), 0usize)).is_some()
    })
}

#[gpui_kit::test]
fn an_empty_folder_and_a_few_pictures_have_no_smart_buttons(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, asked) = open_photos(cx);
    go(&mut visual, "C:\\Empty");
    assert!(lenses_of(&head, &mut visual).is_empty());
    // Nothing to look into: no picture was opened to ask.
    assert_eq!(
        asked.places.load(Ordering::SeqCst),
        0,
        "a folder without pictures does no work"
    );
    go(&mut visual, "C:\\Three");
    assert!(lenses_of(&head, &mut visual).is_empty());
    assert!(!drawn(&mut visual, Lens::Map) && !drawn(&mut visual, Lens::Colors));
    // A folder of files only: no pictures to look into.
    go(&mut visual, "C:\\Docs");
    assert!(lenses_of(&head, &mut visual).is_empty());
    assert_eq!(
        asked.places.load(Ordering::SeqCst),
        1,
        "only the folder with pictures was looked into"
    );
}

#[gpui_kit::test]
fn twenty_pictures_without_places_offer_colors_and_with_places_the_map_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Twenty");
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Colors]);
    assert!(drawn(&mut visual, Lens::Colors) && !drawn(&mut visual, Lens::Map));
    // Five pictures with places: the map, but too few to surf by color.
    go(&mut visual, "C:\\Trip");
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Map]);
    // The pill says how many, and so does its tooltip.
    let label = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find((Lens::Map.id(), 0usize)).label().map(str::to_string)
    });
    assert_eq!(
        label.as_deref(),
        Some("Photo map: 5 photos with a place (Ctrl+Shift+G)")
    );
    assert_eq!(&*Lens::Map.label(5, false), "Map 5");
    assert_eq!(&*Lens::Map.label(MAP_LOOKS, true), "Map 400+");
}

#[gpui_kit::test]
fn a_folder_with_a_history_offers_it_once_known_and_not_before(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, asked) = open_photos(cx);
    go(&mut visual, "C:\\Repo");
    assert_eq!(lenses_of(&head, &mut visual), [Lens::History]);
    // Not known (as in the frames before the answer comes from the background): not shown, and not drawn.
    visual.update(|_, cx| head.update(cx, |head, _| head.history.clear()));
    assert!(lenses_of(&head, &mut visual).is_empty(), "not known yet: not shown");
    assert!(!drawn(&mut visual, Lens::History));
    visual.update(|_, cx| {
        head.update(cx, |head, cx| {
            head.dirty = true;
            head.refresh(cx);
            head.ask(cx);
        })
    });
    visual.run_until_parked();
    assert_eq!(lenses_of(&head, &mut visual), [Lens::History]);
    assert!(drawn(&mut visual, Lens::History));
    // Coming back soon doesn't ask again.
    go(&mut visual, "C:\\Docs");
    go(&mut visual, "C:\\Repo");
    assert_eq!(
        asked.history.load(Ordering::SeqCst),
        3,
        "the repository (twice: its answer was dropped by the test), the other folder once; coming back asks nothing"
    );
    assert_eq!(lenses_of(&head, &mut visual), [Lens::History]);
}

#[gpui_kit::test]
fn the_gallery_always_offers_colors(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Three");
    let mut said = visual
        .update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()))
        .expect("a head");
    assert!(!lenses::colors_fit(&said));
    said.is_gallery = true;
    said.is_folder = false;
    assert!(lenses::colors_fit(&said));
}

#[gpui_kit::test]
fn a_panel_that_is_open_keeps_its_pill_lit(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Docs");
    assert!(lenses_of(&head, &mut visual).is_empty());
    let tab = visual.update(|_, cx| explorer::explorer(cx).read(cx).active_tab_id());
    visual.update(|_, cx| {
        crate::components::photo_map::init_with(cx, Arc::new(|_, _| {}));
        crate::components::photo_map::set(cx, tab, true);
        let wheel = crate::components::color_wheel::shared(cx);
        wheel.update(cx, |wheel, cx| wheel.set_shown(tab, true, None, cx));
    });
    visual.run_until_parked();
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Map, Lens::Colors]);
    assert!(drawn(&mut visual, Lens::Map) && drawn(&mut visual, Lens::Colors));
    // Pressing the lit pill closes the panel, and with nothing to show the pill goes.
    visual.update(|window, cx| window.click((Lens::Map.id(), 0usize), cx));
    visual.run_until_parked();
    assert!(!visual.update(|_, cx| crate::components::photo_map::shows(cx, tab)));
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Colors]);
}

#[gpui_kit::test]
fn an_arrival_is_a_different_place_not_a_refresh_or_a_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    let arrivals = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| {
            window.render_frame(cx);
            head.update(cx, |head, _| head.arrivals)
        })
    };
    go(&mut visual, "C:\\Twenty");
    let first = arrivals(&mut visual);
    // A filter changes the explorer but not the place.
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.set_filter("p1", None, cx)));
    visual.run_until_parked();
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.set_filter("", None, cx)));
    visual.run_until_parked();
    assert_eq!(arrivals(&mut visual), first, "a filter is not an arrival");
    // A search hides the head; closing it comes back to the same place without arriving again.
    visual
        .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.run_find("p1", explorer::Wider::default(), cx)))
        .detach();
    visual.run_until_parked();
    assert!(visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).is_none())));
    let tab = visual.update(|_, cx| explorer::explorer(cx).read(cx).active_tab_id());
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.clear_find_in(tab, cx)));
    visual.run_until_parked();
    assert_eq!(arrivals(&mut visual), first, "back from a search is the same place");
    go(&mut visual, "C:\\Three");
    assert_eq!(arrivals(&mut visual), first + 1);
}

#[gpui_kit::test]
fn the_chips_are_there_on_the_first_frame_and_the_buttons_are_mirrored(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Trip");
    // Before anything has been measured the chips already show (the room is the window's, and what doesn't fit
    // wraps out of sight): one frame, and the Images chip is there.
    let chip = visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(("pane-head-chip", 0usize)).is_some()
    });
    assert!(chip, "no pop-in of the chips on the first frame");
    // Mirrored: the buttons come first (at the start of the row, which is the left), and the name after the picture.
    testing::set_rtl(true);
    visual.update(|_, cx| head.update(cx, |_, cx| cx.notify()));
    let (pill, title) = visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find((Lens::Map.id(), 0usize)).bounds(),
            window.find("pane-head-title").bounds(),
        )
    });
    testing::set_rtl(false);
    assert!(
        pill.center().x < title.center().x,
        "in a mirrored head the buttons are on the left: {pill:?} vs {title:?}"
    );
}

// ---- Labels' chips and "marks here are beside the files" -----------------------------------------------------------

use crate::components::row_facts::{self, fake as facts_fake};
use crate::dialogs::tags::{self as tags_hub, TagsChanged};
use glint_core::marks::MarkedIn;
use glint_model::marks::Label;
use std::sync::Mutex;

fn label(name: &str, color: u32, was: &[&str]) -> Label {
    Label {
        name: name.to_string(),
        color,
        was: was.iter().map(|w| w.to_string()).collect(),
    }
}

fn marked(label: Option<&str>, tags: &[&str]) -> Marks {
    Marks {
        label: label.map(String::from),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        comment: None,
    }
}

fn item(name: &str, label: Option<&str>, tags: &[&str]) -> MarkedIn {
    MarkedIn {
        name: name.to_string(),
        label: label.map(String::from),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        comment: false,
    }
}

/// What a fake index and a fake drive say, and how often they were asked.
#[derive(Clone, Default)]
struct World {
    /// None: the index doesn't know the folder.
    index: Arc<Mutex<Option<Vec<MarkedIn>>>>,
    index_asks: Arc<AtomicUsize>,
    /// Never answers.
    silent: Arc<Mutex<bool>>,
    beside: Arc<AtomicUsize>,
    beside_asks: Arc<AtomicUsize>,
    moved: Arc<AtomicUsize>,
    changed: Arc<AtomicUsize>,
}

impl World {
    fn probes(&self) -> Probes {
        let (index, asks, silent) = (self.index.clone(), self.index_asks.clone(), self.silent.clone());
        let (beside, beside_asks, moved) = (self.beside.clone(), self.beside_asks.clone(), self.moved.clone());
        Probes {
            marks_in: Arc::new(move |_| {
                asks.fetch_add(1, Ordering::SeqCst);
                let (answer, silent) = (index.lock().map(|i| i.clone()).unwrap_or_default(), silent.lock().is_ok_and(|s| *s));
                Box::pin(async move {
                    if silent {
                        std::future::pending::<()>().await;
                    }
                    Ok(answer)
                })
            }),
            beside: Arc::new(move |_| {
                beside_asks.fetch_add(1, Ordering::SeqCst);
                let n = beside.load(Ordering::SeqCst) as u64;
                Box::pin(async move { Ok(n) })
            }),
            move_in: Arc::new(move |_| {
                let n = moved.fetch_add(1, Ordering::SeqCst) as u64 + 12;
                Box::pin(async move { Ok(n) })
            }),
            ..Probes::quiet()
        }
    }
}

/// `C:\Work` with `n` files named f0000.txt.., the marks backend knowing `marks`, and a head on the fake index.
fn open_marks(
    cx: &mut TestAppContext,
    n: usize,
    marks: &[(&str, Marks)],
    world: &World,
) -> (Entity<PaneHead>, VisualTestContext, Arc<facts_fake::MarksBackend>) {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\Work",
        (0..n).map(|i| file("C:\\Work", &format!("f{i:04}.txt"), 10, 0)).collect(),
    );
    let backend = facts_fake::MarksBackend::with(marks);
    cx.executor().allow_parking();
    cx.update(|cx| {
        testing::init(cx);
        let dyn_backend: Arc<dyn tags_hub::Backend> = backend.clone();
        tags_hub::init_with(cx, dyn_backend);
        row_facts::init_with(cx, facts_fake::Media::with(vec![]));
        explorer::init_with(cx, disk.source());
    });
    let (mut visual, head) = testing::open_window(cx, |window, cx| cx.new(|cx| PaneHead::new(0, window, cx)));
    visual.update(|_, cx| head.update(cx, |head, _| head.probes = world.probes()));
    go(&mut visual, "C:\\Work");
    (head, visual, backend)
}

fn chips_of(head: &Entity<PaneHead>, visual: &mut VisualTestContext) -> Vec<(String, usize)> {
    visual.update(|window, cx| {
        window.render_frame(cx);
        head.update(cx, |head, cx| {
            head.head(cx)
                .map(|h| h.labels.iter().map(|l| (l.name.clone(), l.count)).collect())
                .unwrap_or_default()
        })
    })
}

/// Waits (the fakes answer on other threads) until the chips are `want`.
fn wait_for_chips(head: &Entity<PaneHead>, visual: &mut VisualTestContext, want: &[(&str, usize)]) {
    let want: Vec<(String, usize)> = want.iter().map(|(n, c)| (n.to_string(), *c)).collect();
    for _ in 0..400 {
        visual.run_until_parked();
        if chips_of(head, visual) == want {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(chips_of(head, visual), want);
}

fn set_labels(visual: &mut VisualTestContext, labels: Vec<Label>) {
    visual.update(|_, cx| crate::state::update(cx, |s| s.labels = labels));
}

fn quiet_for(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    std::thread::sleep(std::time::Duration::from_millis(30));
    visual.run_until_parked();
}

#[gpui_kit::test]
fn the_index_s_answer_gives_a_chip_a_label_in_the_settings_order_after_the_kinds_and_no_row_is_read(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let world = World::default();
    let items: Vec<MarkedIn> = ["Pending", "Important", "Pending", "Important", "Important", "Important"]
        .iter()
        .enumerate()
        .map(|(i, l)| item(&format!("f{i:04}.txt"), Some(l), &[]))
        .collect();
    *world.index.lock().unwrap() = Some(items);
    let (head, mut visual, backend) = open_marks(cx, 3, &[], &world);
    set_labels(&mut visual, vec![label("Important", 0x3b82f6, &[]), label("Pending", 0xeab308, &[])]);
    wait_for_chips(&head, &mut visual, &[("Important", 4), ("Pending", 2)]);
    let (said, label0, kind0) = visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find(("pane-head-label", 1usize)).label().map(str::to_string),
            window.find(("pane-head-label", 0usize)).bounds(),
            window.find(("pane-head-chip", 0usize)).bounds(),
        )
    });
    assert_eq!(said.as_deref(), Some("Label Pending: 2"));
    assert!(label0.center().x > kind0.center().x, "labels come after the kinds");
    assert_eq!(backend.paths_read(), 0, "ask_all is never asked");
}

#[gpui_kit::test]
fn without_the_index_a_small_folder_is_counted_from_its_rows_and_a_big_one_is_not_read(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let world = World::default();
    let red = marked(Some("Red"), &[]);
    let (head, mut visual, backend) = open_marks(
        cx,
        100,
        &[("C:\\Work\\f0001.txt", red.clone()), ("C:\\Work\\f0002.txt", red.clone()), ("C:\\Work\\f0003.txt", marked(None, &["Blue"]))],
        &world,
    );
    wait_for_chips(&head, &mut visual, &[("Red", 2), ("Blue", 1)]);
    assert!(backend.paths_read() >= 100);
    // Over 5,000 items: nothing is asked and nothing is drawn.
    let world = World::default();
    let (head, mut visual, backend) = open_marks(cx, 5_001, &[("C:\\Work\\f0001.txt", red)], &world);
    quiet_for(&mut visual);
    assert_eq!(backend.paths_read(), 0);
    assert!(chips_of(&head, &mut visual).is_empty());
    assert!(!visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(("pane-head-label", 0usize)).is_some()
    }));
}

#[gpui_kit::test]
fn until_an_answer_is_in_no_label_chip_is_drawn(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let world = World::default();
    *world.silent.lock().unwrap() = true;
    *world.index.lock().unwrap() = Some(vec![item("f0000.txt", Some("Red"), &[])]);
    let (head, mut visual, backend) = open_marks(cx, 3, &[], &world);
    quiet_for(&mut visual);
    assert!(chips_of(&head, &mut visual).is_empty());
    assert!(!visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(("pane-head-label", 0usize)).is_some()
    }));
    assert_eq!(backend.paths_read(), 0, "the rows wait for the index's answer");
}

#[test]
fn a_color_tag_counts_under_its_label_and_a_renamed_label_under_its_name_now() {
    let labels = [label("Red", 0xef4444, &[]), label("Important", 0x3b82f6, &["Pending"])];
    let items = [marked(None, &["Red"]), marked(Some("Red"), &[]), marked(Some("pending"), &[]), marked(Some("Lost"), &[])];
    let chips = label_chips(&labels, &items);
    let said: Vec<(&str, usize)> = chips.iter().map(|c| (c.name.as_str(), c.count)).collect();
    assert_eq!(said, [("Red", 2), ("Important", 1), ("Lost", 1)]);
    assert_eq!(chips[0].color, theme::label_tone(0xef4444), "the label's own color is the dot's");
    assert!(label_chips(&labels, &[marked(None, &["x"])]).is_empty());
}

#[gpui_kit::test]
fn pressing_a_label_chip_puts_its_filter_in_the_box_again_takes_it_out_and_a_kind_still_works(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let world = World::default();
    *world.index.lock().unwrap() = Some(vec![item("f0000.txt", Some("Important"), &[])]);
    let (head, mut visual, _) = open_marks(cx, 3, &[], &world);
    set_labels(&mut visual, vec![label("Important", 0x3b82f6, &[])]);
    wait_for_chips(&head, &mut visual, &[("Important", 1)]);
    let filter = |visual: &mut VisualTestContext| {
        visual.update(|_, cx| {
            explorer::explorer(cx).read(cx).active_tab().map(|t| t.filter.clone()).unwrap_or_default()
        })
    };
    let press = |visual: &mut VisualTestContext, id: &'static str| {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click((id, 0usize), cx);
        });
        visual.run_until_parked();
    };
    press(&mut visual, "pane-head-label");
    assert_eq!(filter(&mut visual), "label:\"Important\"");
    let lit = visual.update(|_, cx| head.update(cx, |h, cx| h.head(cx).map(|h| h.filter.clone())));
    assert_eq!(lit.as_deref(), Some("label:\"Important\""));
    press(&mut visual, "pane-head-label");
    assert_eq!(filter(&mut visual), "");
    press(&mut visual, "pane-head-chip");
    assert!(!filter(&mut visual).is_empty(), "a kind's chip beside it still works");
}

#[gpui_kit::test]
fn marks_changed_in_the_folder_ask_again_and_the_count_changes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let world = World::default();
    *world.index.lock().unwrap() = Some(vec![item("f0000.txt", Some("Red"), &[])]);
    let (head, mut visual, _) = open_marks(cx, 3, &[], &world);
    wait_for_chips(&head, &mut visual, &[("Red", 1)]);
    let before = world.index_asks.load(Ordering::SeqCst);
    *world.index.lock().unwrap() = Some(vec![item("f0000.txt", Some("Red"), &[]), item("f0001.txt", Some("Red"), &[])]);
    // A mark elsewhere asks nothing; one in the folder does.
    let hub = visual.update(|_, cx| tags_hub::hub(cx));
    hub.update(&mut visual, |_, cx| cx.emit(TagsChanged { paths: vec!["C:\\Other\\x.txt".into()] }));
    quiet_for(&mut visual);
    assert_eq!(world.index_asks.load(Ordering::SeqCst), before);
    hub.update(&mut visual, |_, cx| cx.emit(TagsChanged { paths: vec!["C:\\Work\\f0001.txt".into()] }));
    wait_for_chips(&head, &mut visual, &[("Red", 2)]);
    assert_eq!(world.index_asks.load(Ordering::SeqCst), before + 1);
}

#[test]
fn in_a_narrow_pane_the_kinds_keep_their_chips_and_the_labels_take_what_is_left() {
    let kind = |name: &'static str, count: usize| Category {
        group: glint_model::file_types::TypeGroup { name, hue: 0, sat: 0 },
        count,
        bytes: 0,
        query: String::new(),
    };
    let kinds = [kind("Folders", 6), kind("Images", 2)];
    let labels: Vec<LabelChip> = ["Important", "Pending", "Red"]
        .iter()
        .map(|n| LabelChip { name: n.to_string(), color: theme::label_tone(0xff0000), count: 3 })
        .collect();
    assert_eq!(split_room(&kinds, &labels, 5_000., 16.), (2, 3));
    assert_eq!(split_room(&kinds, &labels, 0., 16.), (0, 0));
    // Room for the kinds, not for a label: the kinds are whole.
    let kinds_only = fitting(&kinds, 5_000., 16.);
    let (k, l) = split_room(&kinds, &labels, 230., 16.);
    assert_eq!(k, kinds_only.min(k));
    assert!(k == 2 && l < 3 || k < 2 && l == 0, "{k} {l}");
    // Too narrow for every kind: no label.
    let (k, l) = split_room(&kinds, &labels, 100., 16.);
    assert!(k < 2 && l == 0);
}

#[gpui_kit::test]
fn marks_beside_the_files_show_a_line_and_a_button_that_moves_them_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let world = World::default();
    world.beside.store(12, Ordering::SeqCst);
    let (head, mut visual, _) = open_marks(cx, 3, &[], &world);
    let hub = visual.update(|_, cx| tags_hub::hub(cx));
    let changed = world.changed.clone();
    visual.update(|_, cx| {
        cx.subscribe(&hub, move |_, _: &TagsChanged, _| {
            changed.fetch_add(1, Ordering::SeqCst);
        })
        .detach()
    });
    quiet_for(&mut visual);
    let drawn = |visual: &mut VisualTestContext, id: &'static str| {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    };
    assert!(drawn(&mut visual, "pane-head-beside"));
    let named = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find(("pane-head-move-in", 0usize)).label().map(str::to_string)
    });
    assert_eq!(named.as_deref(), Some("Move them into the files"));
    // The drive has none beside once they are moved.
    world.beside.store(0, Ordering::SeqCst);
    visual.update(|window, cx| window.click(("pane-head-move-in", 0usize), cx));
    for _ in 0..200 {
        visual.run_until_parked();
        if world.changed.load(Ordering::SeqCst) > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(world.moved.load(Ordering::SeqCst), 1);
    assert_eq!(world.changed.load(Ordering::SeqCst), 1, "TagsChanged for the folder's items");
    assert_eq!(visual.update(|window, cx| window.notifications(cx).len()), 1);
    quiet_for(&mut visual);
    assert!(!drawn(&mut visual, "pane-head-beside"), "nothing is beside any more");
    // A search's results never ask.
    let asks = world.beside_asks.load(Ordering::SeqCst);
    visual
        .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.run_find("f00", explorer::Wider::default(), cx)))
        .detach();
    quiet_for(&mut visual);
    assert!(visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).is_none())));
    assert_eq!(world.beside_asks.load(Ordering::SeqCst), asks);
}

#[gpui_kit::test]
fn an_arabic_label_name_mirrors_the_chips_and_nothing_overlaps(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let world = World::default();
    *world.index.lock().unwrap() = Some(vec![
        item("f0000.txt", Some("مهم جدا"), &["وسم"]),
        item("f0001.txt", Some("قيد المراجعة"), &[]),
    ]);
    let (head, mut visual, _) = open_marks(cx, 3, &[], &world);
    wait_for_chips(&head, &mut visual, &[("قيد المراجعة", 1), ("مهم جدا", 1)]);
    testing::set_rtl(true);
    visual.update(|_, cx| head.update(cx, |_, cx| cx.notify()));
    let (a, b, title) = visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find(("pane-head-label", 0usize)).bounds(),
            window.find(("pane-head-label", 1usize)).bounds(),
            window.find("pane-head-title").bounds(),
        )
    });
    testing::set_rtl(false);
    assert!(a.right() <= b.left() || b.right() <= a.left(), "{a:?} {b:?}");
    assert!(a.center().x < title.center().x, "mirrored: the chips are on the left");
}

/// Prints how long the chips take to be worked out once every mark of 5,000 items is in.
#[gpui_kit::test]
#[ignore]
fn five_thousand_items_with_five_hundred_marked_through_has_all(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let world = World::default();
    let marks: Vec<(String, Marks)> = (0..500)
        .map(|i| (format!("C:\\Work\\f{i:04}.txt"), marked(Some(if i % 2 == 0 { "Red" } else { "Blue" }), &["t"])))
        .collect();
    let marks: Vec<(&str, Marks)> = marks.iter().map(|(p, m)| (p.as_str(), m.clone())).collect();
    let (head, mut visual, _) = open_marks(cx, 5_000, &marks, &world);
    wait_for_chips(&head, &mut visual, &[("Red", 250), ("Blue", 250)]);
    let start = Instant::now();
    visual.update(|_, cx| {
        head.update(cx, |head, cx| {
            head.labels_stale = true;
            head.refresh(cx);
        })
    });
    println!("chips of 5,000 items (500 marked) worked out in {:?}", start.elapsed());
}
