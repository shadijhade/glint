//! Headless tests of the status bar: what it draws, and that clicks and keys change the explorer as the TypeScript
//! did. A fake disk stands behind the explorer, and a fake index behind the bar.

use super::index::{Index, Source, State, Volume};
use super::{summary, StatusBar};
use crate::explorer::fake::{file, folder, hidden, FakeDisk};
use crate::explorer::{self, Drive, DriveKind, Explorer, NavOptions};
use crate::kit::prelude::*;
use crate::testing;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the fake service answers now: shared, so a test can change it.
type Answer = Arc<Mutex<Index>>;

/// An index source answering with what `answer` holds, and turning on as `enable` says. Counts the turn-ons.
fn fake_index(answer: Index, enable: Result<Index, String>) -> (Source, Answer, Arc<AtomicUsize>) {
    let shared: Answer = Arc::new(Mutex::new(answer));
    let enables = Arc::new(AtomicUsize::new(0));
    let status_answer = shared.clone();
    let enable_count = enables.clone();
    let source = Source {
        status: Arc::new(move || {
            let now = status_answer
                .lock()
                .map(|guard| guard.clone())
                .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
            Box::pin(async move { Ok(now) })
        }),
        enable: Arc::new(move || {
            enable_count.fetch_add(1, Ordering::SeqCst);
            let answer = enable.clone();
            Box::pin(async move { answer })
        }),
    };
    (source, shared, enables)
}

fn index(state: State, volumes: Vec<Volume>) -> Index {
    Index {
        state,
        version: None,
        volumes,
        error: None,
    }
}

fn scanning(progress: u64, total: u64) -> Volume {
    Volume {
        volume: "C:".into(),
        scanning: true,
        error: false,
        progress,
        progress_total: total,
    }
}

/// `C:\` with folders `a`, `b` and `ar`; `C:\a` with a subfolder, three files and a hidden one; `C:\ar` with an
/// Arabic file name.
fn standard_disk(disk: &FakeDisk) {
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), folder("C:\\", "ar")],
    );
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "sub"),
            file("C:\\a", "file10.txt", 100, 10),
            file("C:\\a", "file2.txt", 300, 30),
            file("C:\\a", "notes.md", 200, 20),
            hidden("C:\\a", ".secret"),
        ],
    );
    disk.folder("C:\\a\\sub", vec![file("C:\\a\\sub", "deep.txt", 1, 1)]);
    disk.folder("C:\\b", vec![]);
    disk.folder("C:\\ar", vec![file("C:\\ar", "تقرير الربع.pdf", 2048, 5)]);
}

fn drive(path: &str, label: &str) -> Drive {
    Drive {
        path: path.into(),
        label: label.into(),
        kind: DriveKind::Fixed,
        fs: "NTFS".into(),
        total: 500,
        free: 200,
    }
}

/// The window's content: the bar at the bottom edge, where the workspace puts it, so its details open above it.
struct Frame {
    bar: Entity<StatusBar>,
}

impl Render for Frame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        col().size_full().justify_end().child(self.bar.clone())
    }
}

struct Setup {
    visual: VisualTestContext,
    bar: Entity<StatusBar>,
    explorer: Entity<Explorer>,
    answer: Answer,
    enables: Arc<AtomicUsize>,
}

/// The skeleton's globals, a fake disk, a fake index answering `answer`, and a window holding the status bar.
fn setup(cx: &mut TestAppContext, answer: Index, enable: Result<Index, String>) -> Setup {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    standard_disk(&disk);
    disk.set_drives(vec![drive("C:\\", "Windows"), drive("D:\\", "Data")]);
    let (source, answer, enables) = fake_index(answer, enable);
    cx.update(|cx| {
        explorer::init_with(cx, disk.source());
        super::init_with(cx, source);
    });
    let (mut visual, frame) = testing::open_window(cx, |window, cx| {
        let bar = cx.new(|cx| StatusBar::new(window, cx));
        cx.new(|_| Frame { bar })
    });
    let bar = visual.update(|_, cx| frame.read(cx).bar.clone());
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let mut setup = Setup {
        visual,
        bar,
        explorer,
        answer,
        enables,
    };
    // This PC's drives come from the explorer's own refresh, as start-up does it.
    let explorer = setup.explorer.clone();
    setup
        .visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.refresh_drives(cx).detach()));
    setup.visual.run_until_parked();
    setup
}

/// Opens `path` in the active tab, as the person would.
fn go(setup: &mut Setup, path: &str) {
    let explorer = setup.explorer.clone();
    let path = path.to_string();
    setup.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| e.navigate(path.clone(), NavOptions::default(), cx));
    });
    setup.visual.run_until_parked();
}

/// Changes the explorer, and lets what it starts run.
fn with_explorer<R>(setup: &mut Setup, f: impl FnOnce(&mut Explorer, &mut gpui_kit::Context<Explorer>) -> R) -> R {
    let explorer = setup.explorer.clone();
    let result = setup.visual.update(|_, cx| explorer.update(cx, f));
    setup.visual.run_until_parked();
    result
}

/// Clicks the element `id` in the last frame (a frame is rendered first, so a change just made is there).
fn click(setup: &mut Setup, id: impl Into<ElementId>) {
    setup.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    setup.visual.run_until_parked();
}

/// Presses a key where the focus is.
fn press(setup: &mut Setup, key: &str) {
    setup.visual.update(|window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    });
    setup.visual.run_until_parked();
}

/// Completes a frame, then asks the accessibility tree: is an element with this name drawn?
fn has(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).is_some()
    })
}

fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).and_then(|element| element.role())
    })
}

/// Lets the index's answer (on core's runtime, another thread) come back to the foreground until `done`.
fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    for _ in 0..400 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the status bar never settled");
}

fn bar_summary(setup: &Setup) -> summary::Summary {
    setup.bar.read_with(&setup.visual, |bar, _| bar.summary.clone())
}

fn filter_of(setup: &Setup) -> Option<String> {
    setup
        .explorer
        .read_with(&setup.visual, |e, _| e.active_tab().map(|t| t.filter.clone()))
}

fn names_shown(setup: &Setup) -> Vec<String> {
    setup.explorer.read_with(&setup.visual, |e, _| {
        e.active_tab()
            .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
            .unwrap_or_default()
    })
}

#[gpui_kit::test]
fn the_bar_is_a_status_region_and_counts_the_folder_and_hidden_items(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    assert_eq!(role_of(&mut setup.visual, "Status bar"), Some(Role::Status));
    // Hidden items are left out of the count and named apart, as the TypeScript did.
    assert_eq!(role_of(&mut setup.visual, "4 items · 1 hidden"), Some(Role::Status));
    let s = bar_summary(&setup);
    assert_eq!((s.shown, s.hidden), (4, 1));
}

#[gpui_kit::test]
fn selecting_a_file_adds_its_count_and_size(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    with_explorer(&mut setup, |e, cx| e.select_only(Some("C:\\a\\notes.md"), cx));
    assert!(has(&mut setup.visual, "4 items · 1 hidden · 1 selected · 200 B"));
    // A folder selected alone adds no size.
    with_explorer(&mut setup, |e, cx| e.select_only(Some("C:\\a\\sub"), cx));
    assert!(has(&mut setup.visual, "4 items · 1 hidden · 1 selected"));
}

#[gpui_kit::test]
fn several_selected_items_are_counted_and_copy_paths_copies_them_one_to_a_line(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    let rows = setup
        .explorer
        .read_with(&setup.visual, |e, _| e.active_tab().map(|t| t.rows.clone()))
        .unwrap_or_default();
    let paths: Vec<String> = rows.iter().map(|r| r.path.clone()).collect();
    with_explorer(&mut setup, |e, cx| e.select_all(&rows, cx));
    assert!(has(&mut setup.visual, "4 items · 1 hidden · 4 selected · 600 B"));
    click(&mut setup, "status-bar-copy-paths");
    let copied = setup.visual.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some(paths.join("\r\n").as_str()));
}

#[gpui_kit::test]
fn the_details_name_how_the_folder_was_read_and_its_kinds(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    assert!(!has(&mut setup.visual, "What's in this folder"), "closed until asked");
    click(&mut setup, "status-bar-details");
    assert_eq!(role_of(&mut setup.visual, "What's in this folder"), Some(Role::Group));
    // The fake disk reads a folder in 1.5 ms, from the drive.
    assert_eq!(
        role_of(&mut setup.visual, "Folder read · from the drive: 1.5 ms"),
        Some(Role::Group)
    );
    // The folder's kinds: a segment per kind, named with its count.
    assert!(has(&mut setup.visual, "Folders: 1 (click to filter)"));
}

#[gpui_kit::test]
fn a_kind_chip_filters_the_folder_and_choosing_it_again_shows_everything(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    click(&mut setup, "status-bar-details");
    let folders = setup
        .bar
        .read_with(&setup.visual, |bar, _| {
            bar.summary.spectrum.iter().position(|c| c.query == "folder:")
        })
        .expect("folders are a kind here");
    click(&mut setup, ("status-chip", folders));
    assert_eq!(filter_of(&setup).as_deref(), Some("folder:"));
    assert_eq!(names_shown(&setup), ["sub"], "only the folder is shown");
    // The filter has closed the details; opening them again offers to show everything, and choosing the chip clears it.
    click(&mut setup, "status-bar-details");
    assert!(has(&mut setup.visual, "Show everything again"));
    click(&mut setup, ("status-chip", folders));
    assert_eq!(filter_of(&setup).as_deref(), Some(""));
    assert_eq!(names_shown(&setup).len(), 4);
}

#[gpui_kit::test]
fn escape_closes_the_details_and_one_press_on_the_button_closes_them_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    click(&mut setup, "status-bar-details");
    assert!(has(&mut setup.visual, "What's in this folder"));
    press(&mut setup, "escape");
    assert!(!has(&mut setup.visual, "What's in this folder"), "Escape closes it");

    // Open again, then press the button that opened it: the press closes it, and its click doesn't reopen it.
    click(&mut setup, "status-bar-details");
    assert!(has(&mut setup.visual, "What's in this folder"));
    click(&mut setup, "status-bar-details");
    assert!(!has(&mut setup.visual, "What's in this folder"), "one press, one close");

    // A click on the counts, outside the details, closes them too.
    click(&mut setup, "status-bar-details");
    click(&mut setup, "status-counts");
    assert!(!has(&mut setup.visual, "What's in this folder"));
}

#[gpui_kit::test]
fn this_pc_counts_drives_and_offers_instant_search_when_it_is_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    settle(&mut setup.visual, |cx| has(cx, "Turn on instant search"));
    assert!(has(&mut setup.visual, "2 drives"));
    assert_eq!(role_of(&mut setup.visual, "Turn on instant search"), Some(Role::Button));
    // Turning it on goes through the source, and its answer replaces the offer.
    click(&mut setup, "status-bar-turn-on");
    assert_eq!(setup.enables.load(Ordering::SeqCst), 1);
    settle(&mut setup.visual, |cx| {
        !has(cx, "Turn on instant search") && !has(cx, "Turning on instant search…")
    });
}

#[gpui_kit::test]
fn a_declined_turn_on_says_nothing_and_keeps_the_offer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Err("cancelled".into()));
    settle(&mut setup.visual, |cx| has(cx, "Turn on instant search"));
    click(&mut setup, "status-bar-turn-on");
    assert_eq!(setup.enables.load(Ordering::SeqCst), 1);
    // The button says it's working until the answer comes back, then offers again: nothing was turned on.
    settle(&mut setup.visual, |cx| has(cx, "Turn on instant search"));
    assert!(!has(&mut setup.visual, "Instant search is on"));
}

#[gpui_kit::test]
fn a_drive_being_read_shows_its_progress(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(
        cx,
        index(State::On, vec![scanning(50, 100)]),
        Ok(index(State::On, vec![])),
    );
    settle(&mut setup.visual, |cx| has(cx, "Indexing C: 50%…"));
    assert_eq!(role_of(&mut setup.visual, "Indexing C: 50%…"), Some(Role::Status));
    // Once the drive is done, the next answer no longer says so and the line goes.
    *setup.answer.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = index(State::On, vec![]);
    setup.bar.update(&mut setup.visual, |bar, cx| {
        bar.index.update(cx, |model, cx| model.refresh(cx))
    });
    settle(&mut setup.visual, |cx| !has(cx, "Indexing C: 50%…"));
}

#[gpui_kit::test]
fn an_arabic_name_and_the_mirrored_bar_render_without_trouble(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    testing::set_rtl(true);
    go(&mut setup, "C:\\ar");
    with_explorer(&mut setup, |e, cx| e.select_only(Some("C:\\ar\\تقرير الربع.pdf"), cx));
    let expected = summary::counts(&bar_summary(&setup));
    assert!(has(&mut setup.visual, &expected), "{expected} is drawn");
    click(&mut setup, "status-bar-details");
    assert_eq!(role_of(&mut setup.visual, "What's in this folder"), Some(Role::Group));
    testing::set_rtl(false);
}
