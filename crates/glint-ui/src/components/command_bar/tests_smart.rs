//! Tests of the island's next step and Paste's words. Card F1 of docs/gpui/aware writes these.
//!
//! The words are plain functions, tested on their own. The window tests open the bar beside a menu host (so the
//! host answers for the next step) and an Aware entity whose clipboard is a fake the test holds.

use super::*;
use crate::components::menus::{MenuHost, Sources as MenuSources};
use crate::explorer::fake::{file, FakeDisk};
use crate::explorer::NavOptions;
use crate::testing;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::atomic::{AtomicUsize, Ordering};

// ---- The words ---------------------------------------------------------------------------------------------

fn files(count: usize, cut: bool, from: Option<&str>, names: &[&str]) -> Clip {
    Clip::Files {
        count,
        cut,
        from: from.map(str::to_string),
        names: names.iter().map(|n| n.to_string()).collect(),
    }
}

#[test]
fn paste_says_how_many_files_it_would_paste_or_move() {
    let _guard = testing::lock();
    let said = |clip: &Clip| smart::paste_words(clip).map(|words| words.to_string());
    assert_eq!(said(&files(3, false, None, &[])).as_deref(), Some("Paste 3 files"));
    assert_eq!(said(&files(1, false, None, &[])).as_deref(), Some("Paste 1 file"));
    assert_eq!(said(&files(3, true, None, &[])).as_deref(), Some("Move 3 files here"));
    assert_eq!(said(&files(1, true, None, &[])).as_deref(), Some("Move 1 file here"));
    assert_eq!(said(&files(1204, false, None, &[])).as_deref(), Some("Paste 1,204 files"));
}

#[test]
fn a_picture_text_or_nothing_on_the_clipboard_has_no_paste_words() {
    let _guard = testing::lock();
    assert!(smart::paste_words(&Clip::Picture).is_none());
    assert!(smart::paste_words(&Clip::Text).is_none());
    assert!(smart::paste_words(&Clip::Empty).is_none());
}

#[test]
fn the_paste_tooltip_names_the_files_and_where_they_are_from() {
    let _guard = testing::lock();
    let tip = |clip: &Clip| smart::paste_tip(clip, "Ctrl+V").to_string();
    let from = Some("C:\\Users\\me\\Downloads");
    assert_eq!(
        tip(&files(2, false, from, &["a.txt", "b.txt"])),
        "Paste (Ctrl+V): a.txt, b.txt, from Downloads"
    );
    // Five files name three, and say how many more.
    assert_eq!(
        tip(&files(5, false, from, &["a", "b", "c", "d", "e"])),
        "Paste (Ctrl+V): a, b, c and 2 more, from Downloads"
    );
    // A cut says it moves.
    assert_eq!(
        tip(&files(2, true, from, &["a.txt", "b.txt"])),
        "Move here (Ctrl+V): a.txt, b.txt, from Downloads"
    );
    // Without a folder, there is nothing to say about where from.
    assert_eq!(tip(&files(1, false, None, &["a.txt"])), "Paste (Ctrl+V): a.txt");
    assert_eq!(tip(&files(1, true, None, &["a.txt"])), "Move here (Ctrl+V): a.txt");
    // A drive's root is named whole.
    assert_eq!(
        tip(&files(1, false, Some("C:\\"), &["a.txt"])),
        "Paste (Ctrl+V): a.txt, from C:\\"
    );
    assert_eq!(tip(&Clip::Empty), "Paste (Ctrl+V)");
    assert_eq!(tip(&Clip::Text), "Paste (Ctrl+V)");
}

// ---- The rig -----------------------------------------------------------------------------------------------

const ZIP: &str = "C:\\Work\\pack.zip";
const A: &str = "C:\\Work\\a.txt";
const B: &str = "C:\\Work\\b.txt";
const WIDE: f32 = 1400.;

/// The window's content: the menu host the bar asks, and the bar in a box of a width the test sets.
struct Shell {
    menus: Entity<MenuHost>,
    bar: Entity<CommandBar>,
    width: Pixels,
}

impl Render for Shell {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(self.width)
            .on_action(cx.listener(|_: &mut Shell, _: &actions::Paste, _, _| {}))
            .on_action(cx.listener(|_: &mut Shell, _: &actions::Delete, _, _| {}))
            .child(self.menus.clone())
            .child(self.bar.clone())
    }
}

/// What the fake clipboard says, and the count that makes the entity read it.
#[derive(Clone, Default)]
struct Board {
    seq: Rc<Cell<u32>>,
    clip: Rc<RefCell<Clip>>,
}

impl Board {
    fn sources(&self) -> aware::Sources {
        let (seq, clip) = (self.seq.clone(), self.clip.clone());
        aware::Sources {
            probe: Rc::new(|_, _, _| async_channel::unbounded().1),
            clip_seq: Rc::new(move || seq.get()),
            clip: Rc::new(move |_| gpui_kit::Task::ready(clip.borrow().clone())),
            today: Rc::new(|| 20_000),
        }
    }
}

struct Rig {
    cx: VisualTestContext,
    shell: Entity<Shell>,
    bar: Entity<CommandBar>,
    events: Rc<RefCell<Vec<MenuEvent>>>,
    board: Board,
    dir: std::path::PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

static RIGS: AtomicUsize = AtomicUsize::new(0);

fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\Work",
        vec![
            file("C:\\Work", "pack.zip", 5000, 0),
            file("C:\\Work", "a.txt", 120, 0),
            file("C:\\Work", "b.txt", 80, 0),
        ],
    );
    disk.folder("C:\\Other", vec![file("C:\\Other", "x.txt", 7, 0)]);
    disk.archive_folder(
        "C:\\Work\\pack.zip",
        "C:\\Work\\pack.zip",
        vec![file("C:\\Work\\pack.zip", "inner.txt", 4, 0)],
    );
    let many = (0..201).map(|i| file("C:\\many", &format!("f{i}.txt"), 1, 0)).collect();
    disk.folder("C:\\many", many);
    disk
}

/// The bar, a menu host and an Aware entity over a fake disk, opened in `C:\Work`, `width` pixels wide.
fn open(cx: &mut TestAppContext, width: f32) -> Rig {
    cx.update(testing::init);
    testing::set_rtl(false);
    let disk = disk();
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\Work", NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let dir = std::env::temp_dir().join(format!(
        "glint-bar-smart-{}-{}",
        std::process::id(),
        RIGS.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let board = Board::default();
    let (state_file, sources) = (dir.join("state.json"), board.sources());
    cx.update(|cx| {
        crate::persistence::init_at(cx, state_file);
        aware::init_with(cx, sources);
    });
    let (mut visual, shell) = testing::open_window(cx, move |window, cx| {
        let menus = cx.new(|cx| MenuHost::with_sources(MenuSources::empty(), cx));
        let bar = cx.new(|cx| CommandBar::with_sources(window, cx, Sources::empty()));
        cx.new(|_| Shell {
            menus,
            bar,
            width: px(width),
        })
    });
    let (menus, bar) = shell.read_with(&visual, |shell, _| (shell.menus.clone(), shell.bar.clone()));
    let events: Rc<RefCell<Vec<MenuEvent>>> = Rc::default();
    let sink = events.clone();
    visual.update(|window, cx| {
        cx.subscribe(&menus, move |_, event: &MenuEvent, _| sink.borrow_mut().push(event.clone()))
            .detach();
        window.activate_window();
    });
    let mut rig = Rig {
        cx: visual,
        shell,
        bar,
        events,
        board,
        dir,
    };
    rig.settle();
    rig
}

impl Rig {
    /// Draws until the bar stops changing (it measures, folds and draws again).
    fn settle(&mut self) {
        let mut last = None;
        let mut stable = 0;
        for _ in 0..40 {
            self.cx.update(|window, cx| window.render_frame(cx));
            self.cx.run_until_parked();
            let now = self.bar.read_with(&self.cx, |bar, _| {
                let m = &bar.measured;
                (bar.fold(), m.bar.get() as i32, m.left.get() as i32, m.right.get() as i32)
            });
            if last == Some(now) {
                stable += 1;
                if stable >= 2 {
                    return;
                }
            } else {
                stable = 0;
            }
            last = Some(now);
        }
        panic!("the bar never settled: {last:?}");
    }

    fn select(&mut self, paths: &[&str]) {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let explorer = self.cx.update(|_, cx| explorer::explorer(cx));
        explorer.update(&mut self.cx, move |e, cx| e.set_selection(paths, None, None, cx));
        self.cx.run_until_parked();
        self.settle();
    }

    fn go(&mut self, path: &str) {
        let path = path.to_string();
        let explorer = self.cx.update(|_, cx| explorer::explorer(cx));
        explorer.update(&mut self.cx, move |e, cx| e.navigate(path, NavOptions::default(), cx));
        self.cx.run_until_parked();
        self.settle();
    }

    fn width(&mut self, width: f32) {
        self.shell.update(&mut self.cx, |shell, cx| {
            shell.width = px(width);
            cx.notify();
        });
        self.settle();
    }

    fn fold(&self) -> usize {
        self.bar.read_with(&self.cx, |bar, _| bar.fold())
    }

    fn find(&mut self, label: &str) -> Option<(bool, Bounds<Pixels>)> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find_by_label(label)
                .map(|e| (e.disabled().unwrap_or(false), e.bounds()))
        })
    }

    fn present(&mut self, label: &str) -> bool {
        self.find(label).is_some()
    }

    /// Every accessible name of a button in the bar, in reading order.
    fn buttons(&mut self) -> Vec<String> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::Button)
                .into_iter()
                .filter_map(|e| e.label().map(str::to_string))
                .collect()
        })
    }

    fn turn(&self) -> u64 {
        self.bar.read_with(&self.cx, |bar, _| bar.step_turn)
    }

    fn step(&self) -> Option<String> {
        self.bar
            .read_with(&self.cx, |bar, _| bar.step.as_ref().map(|s| s.key.clone()))
    }

    fn signature(&self) -> u64 {
        self.bar.read_with(&self.cx, |bar, _| bar.signature(16.))
    }

    /// Puts `clip` on the fake clipboard and waits for the Aware entity to read it.
    fn put_on_clipboard(&mut self, clip: Clip) {
        *self.board.clip.borrow_mut() = clip;
        self.board.seq.set(self.board.seq.get() + 1);
        self.cx.executor().advance_clock(std::time::Duration::from_millis(800));
        self.cx.run_until_parked();
        self.settle();
    }

    fn habits(&mut self) -> glint_model::aware::Habits {
        self.cx.update(|_, cx| {
            crate::persistence::app(cx)
                .map(|app| app.habits.clone())
                .unwrap_or_default()
        })
    }
}

// ---- The next step -----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_zip_selected_puts_extract_here_in_the_island_after_delete(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    assert!(!rig.present("Extract here"), "nothing selected, no step");
    rig.select(&[ZIP]);
    assert_eq!(rig.step().as_deref(), Some("do:extract-here"));
    let buttons = rig.buttons();
    let step = buttons.iter().position(|b| b == "Extract here").expect("Extract here");
    let delete = buttons.iter().position(|b| b.starts_with("Delete (")).expect("Delete");
    assert!(step > delete, "after Delete: {buttons:?}");
    // Nothing selected again: it goes.
    rig.select(&[]);
    assert!(!rig.present("Extract here"));
}

#[gpui_kit::test]
fn more_than_two_hundred_selected_offers_no_step(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    rig.go("C:\\many");
    let all: Vec<String> = (0..201).map(|i| format!("C:\\many\\f{i}.txt")).collect();
    let paths: Vec<&str> = all.iter().map(String::as_str).collect();
    rig.select(&paths);
    assert_eq!(rig.bar.read_with(&rig.cx, |bar, _| bar.facts().selected), 201);
    assert!(rig.step().is_none());
    assert!(!rig.present("Extract here"));
    // One fewer is looked at.
    rig.select(&paths[..200]);
    assert!(rig.bar.read_with(&rig.cx, |bar, _| bar.facts().selected) == 200);
}

#[gpui_kit::test]
fn pressing_the_step_runs_the_menus_row_and_counts_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    rig.select(&[ZIP]);
    rig.cx.update(|window, cx| window.click("step", cx));
    rig.cx.run_until_parked();
    assert_eq!(
        *rig.events.borrow(),
        vec![MenuEvent::ExtractHere { path: ZIP.into() }],
        "the host was asked to run the row"
    );
    let habits = rig.habits();
    assert_eq!(
        habits.action_uses(glint_model::aware::Shape::Archive, "do:extract-here"),
        1,
        "{habits:?}"
    );
}

#[gpui_kit::test]
fn with_suggestions_off_the_island_has_no_step(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    rig.cx.update(|_, cx| state::update(cx, |settings| settings.suggest = false));
    rig.select(&[ZIP]);
    assert!(rig.step().is_none());
    assert!(!rig.present("Extract here"));
    // The rest of the island is as it was.
    assert!(rig.buttons().iter().any(|b| b.starts_with("Rename")));
}

#[gpui_kit::test]
fn a_different_kind_of_selection_turns_the_ring_and_the_same_one_does_not(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    let start = rig.turn();
    rig.select(&[ZIP]);
    let zip = rig.step();
    assert_eq!(rig.turn(), start + 1);
    // The same selection again tells nobody.
    rig.select(&[ZIP]);
    assert_eq!(rig.turn(), start + 1);
    // Two text files that can be compared: another step.
    rig.select(&[A, B]);
    let texts = rig.step();
    assert_ne!(zip, texts, "{texts:?}");
    assert_eq!(rig.turn(), start + 2);
    rig.select(&[B, A]);
    assert_eq!(rig.turn(), start + 2);
}

// ---- Paste -------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn files_on_the_clipboard_give_paste_words_and_keep_its_name(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    rig.go("C:\\Other");
    let clip = files(3, false, Some("C:\\Work"), &["a.txt", "b.txt", "pack.zip"]);
    rig.put_on_clipboard(clip.clone());
    // The words are a fact about the clipboard...
    assert_eq!(smart::paste_words(&clip).as_deref(), Some("Paste 3 files"));
    assert_eq!(rig.bar.read_with(&rig.cx, |bar, _| bar.clip.clone()), clip);
    // ...and the button is still found by the name the end-to-end checks use, and can be pressed.
    let (disabled, _) = rig.find("Paste (Ctrl+V)").expect("Paste keeps its name");
    assert!(!disabled);
    // Without a selection the island holds Paste alone of the edits.
    assert!(!rig.present("Paste 3 files"), "the visible words are not its name");
}

#[gpui_kit::test]
fn in_an_archive_paste_has_no_words(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    rig.go(ZIP);
    rig.put_on_clipboard(files(3, false, None, &["a", "b", "c"]));
    let (disabled, bounds) = rig.find("Paste (Ctrl+V)").expect("Paste");
    assert!(disabled, "an archive is read-only");
    // As wide as the icon button of today: no words made it wider.
    rig.put_on_clipboard(Clip::Empty);
    let (_, plain) = rig.find("Paste (Ctrl+V)").expect("Paste");
    assert_eq!(bounds.size.width, plain.size.width);
}

#[gpui_kit::test]
fn paste_with_words_is_wider_than_without(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    rig.go("C:\\Other");
    let (_, plain) = rig.find("Paste (Ctrl+V)").expect("Paste");
    rig.put_on_clipboard(files(3, false, None, &["a", "b", "c"]));
    let (_, worded) = rig.find("Paste (Ctrl+V)").expect("Paste");
    assert!(worded.size.width > plain.size.width, "{worded:?} vs {plain:?}");
}

// ---- Folding and the signature -----------------------------------------------------------------------------

#[gpui_kit::test]
fn the_step_loses_its_label_at_fold_one_and_is_gone_at_fold_three(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    rig.select(&[ZIP]);
    // Find a width for each fold by narrowing until the bar reaches it.
    let mut seen: std::collections::BTreeMap<usize, f32> = Default::default();
    let mut width = WIDE;
    while width > 150. {
        rig.width(width);
        seen.entry(rig.fold()).or_insert(width);
        width -= 20.;
    }
    assert!(seen.contains_key(&0) && seen.contains_key(&1) && seen.contains_key(&3), "{seen:?}");
    rig.width(seen[&0]);
    let (_, wide) = rig.find("Extract here").expect("the step at fold 0");
    rig.width(seen[&1]);
    let (_, narrow) = rig.find("Extract here").expect("the step at fold 1");
    assert!(narrow.size.width < wide.size.width, "no label at fold 1: {narrow:?} < {wide:?}");
    rig.width(seen[&3]);
    assert!(!rig.present("Extract here"), "not drawn at fold 3");
}

#[gpui_kit::test]
fn the_signature_changes_with_a_step_and_with_files_on_the_clipboard(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, WIDE);
    let none = rig.signature();
    rig.select(&[ZIP]);
    let with_step = rig.signature();
    assert_ne!(none, with_step);
    rig.put_on_clipboard(files(3, false, None, &["a", "b", "c"]));
    let with_clip = rig.signature();
    assert_ne!(with_step, with_clip);
    rig.put_on_clipboard(Clip::Empty);
    assert_eq!(rig.signature(), with_step);
}
