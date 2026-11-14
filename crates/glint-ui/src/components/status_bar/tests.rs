//! Headless tests of the status bar: what it draws, and that clicks and keys change the explorer as the TypeScript
//! did. A fake disk stands behind the explorer, and a fake index behind the bar.

use super::index::{Index, Source, State, Volume};
use super::summary::Thumbs;
use super::{summary, StatusBar};
use crate::components::file_grid::thumbnail::{self, Ask, Fut, ThumbSource, Want};
use crate::components::file_list::fake_git::{repo_view, FakeGit};
use crate::explorer::fake::{file, folder, hidden, FakeDisk};
use crate::explorer::{self, Drive, DriveKind, Explorer, NavOptions};
use crate::kit::prelude::*;
use crate::testing;
use glint_model::format::format_duration;
use glint_model::store::ViewMode;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{ElementId, Entity, TestAppContext, VisualTestContext};
use std::rc::Rc;
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
    disk: FakeDisk,
}

/// The skeleton's globals, a fake disk, a fake index answering `answer`, and a window holding the status bar.
fn setup(cx: &mut TestAppContext, answer: Index, enable: Result<Index, String>) -> Setup {
    setup_with(cx, answer, enable, None)
}

/// [`setup`], with the thumbnail service answering from `thumbs` (the file grid's source, in tests).
fn setup_with(
    cx: &mut TestAppContext,
    answer: Index,
    enable: Result<Index, String>,
    thumbs: Option<Rc<dyn ThumbSource>>,
) -> Setup {
    setup_full(cx, answer, enable, thumbs, None)
}

/// [`setup_with`], and a fake Git behind the Git model when given (made before the bar, as start-up does).
fn setup_full(
    cx: &mut TestAppContext,
    answer: Index,
    enable: Result<Index, String>,
    thumbs: Option<Rc<dyn ThumbSource>>,
    git: Option<&FakeGit>,
) -> Setup {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    standard_disk(&disk);
    disk.set_drives(vec![drive("C:\\", "Windows"), drive("D:\\", "Data")]);
    let (source, answer, enables) = fake_index(answer, enable);
    cx.update(|cx| {
        explorer::init_with(cx, disk.source());
        if let Some(git) = git {
            git.install(cx);
        }
        super::init_with(cx, source);
        if let Some(thumbs) = thumbs {
            thumbnail::init_with(cx, thumbs);
        }
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
        disk,
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
fn a_breakdown_segment_takes_the_keyboard_and_enter_filters_to_its_kind(cx: &mut TestAppContext) {
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
    // The segment is a button a person can reach with Tab: it has a focus of its own.
    let focus = setup
        .bar
        .read_with(&setup.visual, |bar, _| bar.segment_focus.get(folders).cloned())
        .expect("the segment has a focus handle");
    setup.visual.update(|window, cx| {
        window.render_frame(cx);
        focus.focus(window, cx);
    });
    press(&mut setup, "enter");
    assert_eq!(
        filter_of(&setup).as_deref(),
        Some("folder:"),
        "Enter filters as the click does"
    );
    assert_eq!(names_shown(&setup), ["sub"]);
}

/// Answers every thumbnail with a picture from core's memory, as core does for a file it has just made.
struct Pictures;

impl ThumbSource for Pictures {
    fn thumbnail(&self, _ask: Ask) -> Fut<Result<glint_core::thumbs::Thumbnail, String>> {
        // Found, from memory, not lasting, and no time spent by the shell.
        let answer = glint_core::thumbs::Thumbnail {
            pixels: Some(std::sync::Arc::new(glint_core::thumbs::Pixels {
                width: 1,
                height: 1,
                bgra: vec![0, 0, 255, 255],
            })),
            lasting: false,
            from: glint_core::thumbs::ThumbFrom::Memory,
            ms: 0.0,
        };
        Box::pin(async move { Ok(answer) })
    }

    fn cancel(&self, _lane: &str, _paths: Option<Vec<String>>, _up_to: u32) {}
}

/// The tab's thumbnail lane and grid scope, as the file grid names them.
fn grid_keys(setup: &Setup) -> (String, String) {
    setup.explorer.read_with(&setup.visual, |e, _| {
        let tab = e.active_tab().expect("a tab is open");
        (tab.id.to_string(), format!("{}:grid", tab.view_key()))
    })
}

/// The thumbnail run of `lane`, if the service has one.
fn run_of(cx: &mut VisualTestContext, lane: &str) -> Option<thumbnail::ThumbRun> {
    cx.update(|_, app| {
        let service = thumbnail::thumbnails(app);
        service.read(app).run(lane).cloned()
    })
}

#[gpui_kit::test]
fn the_thumbnails_of_the_grid_show_in_the_bar_once_they_come(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup_with(
        cx,
        index(State::Off, vec![]),
        Ok(index(State::On, vec![])),
        Some(Rc::new(Pictures)),
    );
    go(&mut setup, "C:\\a");
    with_explorer(&mut setup, |e, cx| e.set_view(ViewMode::Grid, None, cx));
    let (lane, scope) = grid_keys(&setup);
    // The grid asks for its items' thumbnails and times the burst, as FileGrid does on first showing.
    let wants: Vec<Want> = setup.explorer.read_with(&setup.visual, |e, _| {
        e.active_tab()
            .map(|t| {
                t.entries()
                    .iter()
                    .filter_map(|entry| Want::for_entry(entry, 100., 1.))
                    .collect()
            })
            .unwrap_or_default()
    });
    assert!(!wants.is_empty(), "the folder has files a thumbnail can be made of");
    let viewer = setup.bar.entity_id();
    setup.visual.update(|_, cx| {
        let service = thumbnail::thumbnails(cx);
        service.update(cx, |t, cx| {
            t.time(&lane, &scope, cx);
            t.show(viewer, &lane, wants, cx);
        });
    });
    settle(&mut setup.visual, |cx| {
        run_of(cx, &lane).is_some_and(|run| run.found > 0 && run.pending == 0)
    });
    // The line says how many came and how long they took, and the stage in the details says the same.
    let run = run_of(&mut setup.visual, &lane).expect("the burst is timed");
    let th = Thumbs::of(&run);
    let line = summary::thumbs_line(&th);
    assert_eq!(role_of(&mut setup.visual, &line), Some(Role::Status));
    click(&mut setup, "status-bar-details");
    let stage = summary::thumbs_stage(&th).expect("done thumbnails have a stage");
    let row = format!("{} · {}: {}", stage.label, stage.detail, format_duration(stage.ms));
    assert_eq!(role_of(&mut setup.visual, &row), Some(Role::Group));

    // In the list there is no grid, so the bar says nothing about thumbnails.
    click(&mut setup, "status-bar-details");
    with_explorer(&mut setup, |e, cx| e.set_view(ViewMode::List, None, cx));
    assert!(!has(&mut setup.visual, &line), "the list shows no thumbnails line");
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

// ---- Git status and folder sizes ------------------------------------------------------------------------------

/// A label (a status piece, a stage) that starts with `start`, drawn now.
fn label_starting(setup: &mut Setup, start: &str) -> Option<String> {
    setup.visual.update(|window, cx| {
        window.render_frame(cx);
        window
            .elements()
            .iter()
            .filter_map(|element| element.label())
            .find(|label| label.starts_with(start))
            .map(str::to_string)
    })
}

#[gpui_kit::test]
fn the_work_line_says_how_long_git_status_took_in_a_repository_and_nothing_outside_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let git = FakeGit::default();
    git.put(
        "C:\\a",
        repo_view(&[("notes.md", glint_model::entries::GitMark::Modified)]),
    );
    let mut setup = setup_full(
        cx,
        index(State::Off, vec![]),
        Ok(index(State::On, vec![])),
        None,
        Some(&git),
    );
    go(&mut setup, "C:\\a");
    settle(&mut setup.visual, |cx| has(cx, "git · 12 ms"));
    assert_eq!(role_of(&mut setup.visual, "git · 12 ms"), Some(Role::Status));
    // The hover text says what Git found.
    let description = setup
        .visual
        .update(|window, _| window.find_by_label("git · 12 ms").description().map(str::to_string));
    assert_eq!(
        description.as_deref(),
        Some("Git status of this folder in 12 ms: 1 changed in here and below, on main.")
    );
    // Outside a repository there is no Git line.
    go(&mut setup, "C:\\b");
    settle(&mut setup.visual, |cx| !has(cx, "git · 12 ms"));
    assert!(!has(&mut setup.visual, "git · 12 ms"));
}

#[gpui_kit::test]
fn the_details_time_git_status_after_the_folder_sizes_and_before_the_thumbnails(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let git = FakeGit::default();
    git.put("C:\\a", repo_view(&[]));
    let mut setup = setup_full(
        cx,
        index(State::Off, vec![]),
        Ok(index(State::On, vec![])),
        None,
        Some(&git),
    );
    go(&mut setup, "C:\\a");
    settle(&mut setup.visual, |cx| has(cx, "git · 12 ms"));
    with_explorer(&mut setup, |e, cx| e.calc_sizes(cx));
    click(&mut setup, "status-bar-details");
    let stages: Vec<String> = setup.visual.update(|window, cx| {
        window.render_frame(cx);
        window
            .elements()
            .iter()
            .filter_map(|element| element.label())
            .filter(|label| label.contains(" · ") && label.contains(": "))
            .map(str::to_string)
            .collect()
    });
    let at = |what: &str| stages.iter().position(|label| label.starts_with(what));
    let (read, sizes, status) = (at("Folder read"), at("Folder sizes (pdu)"), at("Git status"));
    assert!(
        read.is_some() && read < sizes && sizes < status,
        "folder read, then folder sizes, then Git status: {stages:?}"
    );
    assert!(
        stages.iter().any(|label| label == "Git status · main: 12 ms"),
        "the branch is the detail: {stages:?}"
    );
}

#[gpui_kit::test]
fn the_work_line_says_what_the_folder_sizes_took_once_they_are_measured(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    assert!(
        label_starting(&mut setup, "pdu sized").is_none(),
        "nothing measured yet"
    );
    let gate = setup.disk.hold_sizes();
    with_explorer(&mut setup, |e, cx| e.calc_sizes(cx));
    assert!(
        label_starting(&mut setup, "pdu sized").is_none(),
        "not while it is being measured"
    );
    drop(gate);
    settle(&mut setup.visual, |cx| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .elements()
                .iter()
                .any(|e| e.label().is_some_and(|l| l.starts_with("pdu sized")))
        })
    });
    let said = label_starting(&mut setup, "pdu sized").unwrap_or_default();
    // The files in the folder, and the one inside `sub`; the fake disk measures in 1.5 ms.
    assert_eq!(said, "pdu sized 601 B in 1.5 ms");
    assert_eq!(bar_summary(&setup).sized.map(|s| s.total), Some(601));
    // The breakdown counts the folder's bytes now: `sub` holds one byte.
    let folders = bar_summary(&setup)
        .spectrum
        .into_iter()
        .find(|category| category.group == summary::FOLDERS);
    assert_eq!(folders.map(|category| category.bytes), Some(1));
}

// ---- X-Ray, Rename and Zip -------------------------------------------------------------------------------

/// Selects the listed items of the open folder with these names, the first alone and the rest added to it.
fn select_names(setup: &mut Setup, names: &[&str]) {
    let paths: Vec<String> = setup.explorer.read_with(&setup.visual, |e, _| {
        e.active_tab()
            .map(|t| {
                t.rows
                    .iter()
                    .filter(|r| names.contains(&r.name.as_str()))
                    .map(|r| r.path.clone())
                    .collect()
            })
            .unwrap_or_default()
    });
    with_explorer(setup, |e, cx| {
        e.select_only(None, cx);
        for path in &paths {
            e.toggle_selected(path, cx);
        }
    });
}

#[gpui_kit::test]
fn x_ray_is_offered_for_one_selected_file_and_for_nothing_else(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    assert!(!has(&mut setup.visual, "X-Ray"), "nothing selected");
    select_names(&mut setup, &["notes.md"]);
    assert!(has(&mut setup.visual, "X-Ray"));
    assert_eq!(role_of(&mut setup.visual, "X-Ray"), Some(Role::Button));
    // A folder is not a file to look into.
    select_names(&mut setup, &["sub"]);
    assert!(!has(&mut setup.visual, "X-Ray"));
    // Several are renamed and zipped, not looked into.
    select_names(&mut setup, &["notes.md", "file2.txt"]);
    assert!(!has(&mut setup.visual, "X-Ray"));
}

#[gpui_kit::test]
fn rename_and_zip_are_offered_for_several_selected_items_only(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    select_names(&mut setup, &["notes.md"]);
    assert!(!has(&mut setup.visual, "Rename") && !has(&mut setup.visual, "Zip"));
    select_names(&mut setup, &["notes.md", "file2.txt"]);
    assert_eq!(role_of(&mut setup.visual, "Rename"), Some(Role::Button));
    assert_eq!(role_of(&mut setup.visual, "Zip"), Some(Role::Button));
}

#[gpui_kit::test]
fn no_selection_button_shows_in_an_archive_or_on_this_pc(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    setup.disk.archive_folder(
        "C:\\pack.zip",
        "C:\\pack.zip",
        vec![file("C:\\pack.zip", "a.txt", 1, 1), file("C:\\pack.zip", "b.txt", 2, 2)],
    );
    go(&mut setup, "C:\\pack.zip");
    select_names(&mut setup, &["a.txt", "b.txt"]);
    assert!(bar_summary(&setup).selected == 2 && !bar_summary(&setup).writable);
    assert!(!has(&mut setup.visual, "Rename") && !has(&mut setup.visual, "Zip"));
    select_names(&mut setup, &["a.txt"]);
    assert!(!has(&mut setup.visual, "X-Ray"));
    // A folder on disk is writable again.
    go(&mut setup, "C:\\a");
    assert!(bar_summary(&setup).writable);
}

#[gpui_kit::test]
fn the_buttons_wait_for_their_widths(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    select_names(&mut setup, &["notes.md", "file2.txt"]);
    // Rename shows from the medium width (28 rem), Zip from the large (32 rem); a 30 rem window has the first.
    let rem = setup.visual.update(|window, _| f32::from(window.rem_size()));
    setup.visual.simulate_resize(size(px(rem * 30.), px(600.)));
    assert!(has(&mut setup.visual, "Rename"));
    assert!(!has(&mut setup.visual, "Zip"));
    setup.visual.simulate_resize(size(px(rem * 20.), px(600.)));
    assert!(!has(&mut setup.visual, "Rename"));
}

#[gpui_kit::test]
fn rename_opens_the_batch_rename_for_the_selection(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    select_names(&mut setup, &["notes.md", "file2.txt"]);
    click(&mut setup, "status-bar-rename");
    assert!(has(&mut setup.visual, "Rename items"));
}

#[gpui_kit::test]
fn x_ray_opens_the_dialog_for_the_file(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    select_names(&mut setup, &["notes.md"]);
    click(&mut setup, "status-bar-xray");
    assert!(has(&mut setup.visual, "File X-Ray: notes.md"));
}

#[gpui_kit::test]
fn zip_compresses_the_selection_once_the_secrets_check_lets_it_through(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut setup = setup(cx, index(State::Off, vec![]), Ok(index(State::On, vec![])));
    go(&mut setup, "C:\\a");
    select_names(&mut setup, &["notes.md", "file2.txt"]);
    click(&mut setup, "status-bar-zip");
    // No Kingfisher to look with (no core in tests): the check is an offer, not a gate.
    settle(&mut setup.visual, |_| !setup.disk.compressions().is_empty());
    let compressions = setup.disk.compressions();
    assert_eq!(compressions.len(), 1);
    let mut paths = compressions[0].0.paths.clone();
    paths.sort();
    assert_eq!(paths, ["C:\\a\\file2.txt", "C:\\a\\notes.md"]);
    assert_eq!(compressions[0].0.format, explorer::CompressFormat::Zip);
}
