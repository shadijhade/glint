//! Headless tests of the tab strip, on a fake disk behind the explorer.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::testing;
use gpui_kit::test::{ClickOptions, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};

/// A fake disk with a few folders, one of them with an Arabic name.
fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), folder("C:\\", "تقارير")],
    );
    disk.folder("C:\\a", vec![file("C:\\a", "one.txt", 1, 1)]);
    disk.folder("C:\\b", vec![file("C:\\b", "two.txt", 1, 1)]);
    disk.folder("C:\\تقارير", vec![file("C:\\تقارير", "ملف.txt", 1, 1)]);
    disk
}

/// The strip in a window over an explorer on the fake disk, in English or Arabic.
fn open(cx: &mut TestAppContext, rtl: bool) -> (VisualTestContext, Entity<TabStrip>) {
    let (visual, strip, _) = open_on_disk(cx, rtl);
    (visual, strip)
}

/// The same, with the fake disk, to see what was done to it.
fn open_on_disk(cx: &mut TestAppContext, rtl: bool) -> (VisualTestContext, Entity<TabStrip>, FakeDisk) {
    cx.update(testing::init);
    let disk = disk();
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    testing::set_rtl(rtl);
    let (visual, strip) = testing::open_window(cx, |window, cx| cx.new(|cx| TabStrip::new(window, cx)));
    (visual, strip, disk)
}

fn ex(cx: &mut VisualTestContext) -> Entity<Explorer> {
    cx.update(|_, cx| explorer::explorer(cx))
}

/// Runs `f` on the explorer and lets what it started finish.
fn act(cx: &mut VisualTestContext, f: impl FnOnce(&mut Explorer, &mut Context<Explorer>)) {
    let explorer = ex(cx);
    explorer.update(cx, f);
    cx.run_until_parked();
}

/// The names of pane `pane`'s tabs, in order.
fn names(cx: &mut VisualTestContext, pane: usize) -> Vec<String> {
    let explorer = ex(cx);
    cx.update(|_, cx| {
        let e = explorer.read(cx);
        e.panes()[pane].tabs.iter().map(|&id| name_of(e, e.tab(id))).collect()
    })
}

/// The ids of pane `pane`'s tabs, in order.
fn order(cx: &mut VisualTestContext, pane: usize) -> Vec<u32> {
    let explorer = ex(cx);
    cx.update(|_, cx| explorer.read(cx).panes()[pane].tabs.iter().map(|id| id.0).collect())
}

fn active(cx: &mut VisualTestContext) -> TabId {
    let explorer = ex(cx);
    cx.update(|_, cx| explorer.read(cx).active_tab_id())
}

/// Role, selected state and bounds of the element with this label, from a fresh frame.
fn snapshot(cx: &mut VisualTestContext, label: &str) -> Option<(Option<Role>, Option<bool>, Bounds<Pixels>)> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .try_find_by_label(label)
            .map(|e| (e.role(), e.selected(), e.bounds()))
    })
}

fn bounds(cx: &mut VisualTestContext, id: &'static str) -> Bounds<Pixels> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find(id).bounds()
    })
}

fn click(cx: &mut VisualTestContext, id: &'static str) {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    cx.run_until_parked();
}

fn middle_click(cx: &mut VisualTestContext, id: &'static str) {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click_with_options(id, ClickOptions::new().with_button(MouseButton::Middle), cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn draws_the_tabs_named_with_roles_and_marks_the_shown_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    assert_eq!(names(&mut cx, 0), ["This PC", "a", "b"]);
    let (role, selected, _) = snapshot(&mut cx, "b").unwrap();
    assert_eq!((role, selected), (Some(Role::Tab), Some(true)));
    assert_eq!(snapshot(&mut cx, "a").unwrap().1, Some(false));
    assert_eq!(snapshot(&mut cx, "This PC").unwrap().1, Some(false));
    assert_eq!(snapshot(&mut cx, "Tabs").unwrap().0, Some(Role::TabList));
    assert_eq!(snapshot(&mut cx, "New tab").unwrap().0, Some(Role::Button));
    // Every tab has a close button with a name; the pane menu button is for dual-pane view only.
    assert_eq!(snapshot(&mut cx, "Close a").unwrap().0, Some(Role::Button));
    assert!(snapshot(&mut cx, "Pane menu").is_none());
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn clicking_shows_a_tab_the_x_and_a_middle_click_close_it_and_plus_adds_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    click(&mut cx, "tab-1");
    assert_eq!(active(&mut cx), TabId(1));
    click(&mut cx, "tab-2");
    assert_eq!(active(&mut cx), TabId(2));

    // The × closes the tab, and the one beside it takes its place.
    click(&mut cx, "tab-close-2");
    assert_eq!(names(&mut cx, 0), ["This PC", "b"]);
    assert_eq!(active(&mut cx), TabId(3));

    // A middle-click closes too.
    middle_click(&mut cx, "tab-1");
    assert_eq!(names(&mut cx, 0), ["b"]);

    // The last tab has no ×, and a middle-click leaves it.
    assert!(snapshot(&mut cx, "Close b").is_none());
    middle_click(&mut cx, "tab-3");
    assert_eq!(names(&mut cx, 0), ["b"]);

    // + opens a new tab on the open folder.
    click(&mut cx, "new-tab-0");
    assert_eq!(names(&mut cx, 0), ["b", "b"]);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_tab_menu_has_the_typescripts_items(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.right_click("tab-2", cx);
    });
    cx.run_until_parked();
    for item in [
        "Duplicate tab",
        "Open side by side",
        "Close tab",
        "Close other tabs",
        "Reopen closed tab",
    ] {
        assert!(snapshot(&mut cx, item).is_some(), "{item} is in the menu");
    }
    // Moving to the other pane is for two panes.
    assert!(snapshot(&mut cx, "Move to the other pane").is_none());
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn dragging_a_tab_reorders_the_strip(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    // The last tab, dropped on the first one's near edge, goes first.
    let first = bounds(&mut cx, "tab-1");
    let last = bounds(&mut cx, "tab-3");
    let from = last.center();
    let to = point(first.origin.x + px(4.), first.center().y);
    cx.update(|window, cx| window.drag(from, to, cx));
    cx.run_until_parked();
    assert_eq!(order(&mut cx, 0), [3, 1, 2]);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn dual_pane_view_has_a_strip_for_each_pane_and_a_pane_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.toggle_dual(cx));
    assert_eq!(snapshot(&mut cx, "Left pane tabs").unwrap().0, Some(Role::TabList));
    assert_eq!(snapshot(&mut cx, "Right pane tabs").unwrap().0, Some(Role::TabList));
    assert!(snapshot(&mut cx, "Tabs").is_none());

    // A tab dragged into the other pane's strip moves there.
    let from = bounds(&mut cx, "tab-2").center();
    let to = bounds(&mut cx, "tablist-1").center();
    cx.update(|window, cx| window.drag(from, to, cx));
    cx.run_until_parked();
    assert_eq!(names(&mut cx, 0).len() + names(&mut cx, 1).len(), 3);
    assert_ne!(names(&mut cx, 0).len(), 2, "a tab left the left pane");

    // The pane button opens the pane's menu.
    click(&mut cx, "pane-menu-0");
    for item in [
        "Swap panes",
        "Show this folder in the other pane",
        "Even out the panes",
        "Close this pane",
    ] {
        assert!(snapshot(&mut cx, item).is_some(), "{item} is in the pane menu");
    }
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_keyboard_moves_between_tabs(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, strip) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    cx.update(|window, cx| window.render_frame(cx));
    let handle = strip.read_with(&cx, |s, _| s.focus[&TabId(3)].clone());
    cx.update(|window, cx| window.focus(&handle, cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.press("left", cx);
    });
    cx.run_until_parked();
    assert_eq!(active(&mut cx), TabId(2));
    cx.update(|window, cx| window.press("home", cx));
    cx.run_until_parked();
    assert_eq!(active(&mut cx), TabId(1));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn arabic_runs_the_strip_right_to_left_and_names_an_arabic_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, true);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\تقارير"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    // The first tab is on the right, the next to its left.
    let first = bounds(&mut cx, "tab-1");
    let second = bounds(&mut cx, "tab-2");
    let third = bounds(&mut cx, "tab-3");
    assert!(first.origin.x > second.origin.x && second.origin.x > third.origin.x);
    // The Arabic name is the tab's accessible name, unmarked.
    let (role, selected, _) = snapshot(&mut cx, "تقارير").unwrap();
    assert_eq!((role, selected), (Some(Role::Tab), Some(false)));
    assert_eq!(snapshot(&mut cx, "Close تقارير").unwrap().0, Some(Role::Button));
    // Dropping at the right edge of the first tab puts a tab before it.
    let from = third.center();
    let to = point(first.origin.x + first.size.width - px(4.), first.center().y);
    cx.update(|window, cx| window.drag(from, to, cx));
    cx.run_until_parked();
    assert_eq!(order(&mut cx, 0), [3, 1, 2]);
    testing::set_rtl(false);
}

#[test]
fn the_gap_a_tab_would_take_follows_the_reading_direction() {
    let tab = |x: f32| Bounds {
        origin: point(px(x), px(0.)),
        size: size(px(100.), px(28.)),
    };
    let english = [tab(0.), tab(102.), tab(204.)];
    assert_eq!(slot_at(px(10.), &english, false), 0);
    assert_eq!(slot_at(px(160.), &english, false), 2);
    assert_eq!(slot_at(px(290.), &english, false), 3);
    // Arabic: the first tab is the rightmost.
    let arabic = [tab(204.), tab(102.), tab(0.)];
    assert_eq!(slot_at(px(290.), &arabic, true), 0);
    assert_eq!(slot_at(px(160.), &arabic, true), 1);
    assert_eq!(slot_at(px(10.), &arabic, true), 3);
    assert_eq!(slot_at(px(10.), &[], false), 0);
}

#[test]
fn drives_are_named_in_the_interface_language() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let drive = Drive {
        path: "C:\\".into(),
        label: String::new(),
        kind: DriveKind::Fixed,
        fs: "NTFS".into(),
        total: 0,
        free: 0,
    };
    assert_eq!(drive_title(&drive), "Local Disk (C:)");
    let wsl = Drive {
        path: "\\\\wsl$\\Ubuntu".into(),
        label: "Ubuntu".into(),
        kind: DriveKind::Wsl,
        ..drive
    };
    assert_eq!(drive_title(&wsl), "Ubuntu (WSL)");
}

// ---- Files dragged over tabs ------------------------------------------------------------------------

use crate::drops;
use crate::explorer::FileOp;
use crate::platform::dragdrop::{DropAnswers, DropEvent};
use std::time::Duration;

/// The middle of the element with this id, from a fresh frame.
fn middle(cx: &mut VisualTestContext, id: &'static str) -> (f32, f32) {
    let b = bounds(cx, id);
    (
        f32::from(b.origin.x + b.size.width / 2.),
        f32::from(b.origin.y + b.size.height / 2.),
    )
}

fn drag_event(cx: &mut VisualTestContext, answers: &DropAnswers, event: DropEvent) {
    cx.update(|window, cx| drops::dispatch(event, answers, window, cx));
    cx.run_until_parked();
}

#[gpui_kit::test]
fn files_held_over_a_hidden_tab_open_it_after_a_moment_and_a_drop_goes_into_its_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _, disk) = open_on_disk(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    assert_eq!(active(&mut cx), TabId(3));
    let answers = DropAnswers::default();
    let files = vec!["C:\\a\\one.txt".to_string()];
    let (x, y) = middle(&mut cx, "tab-2");
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Enter {
            paths: files.clone(),
            x,
            y,
            effect: DropEffect::Copy,
        },
    );
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Over {
            x,
            y,
            effect: DropEffect::Copy,
        },
    );
    match answers.get() {
        Answer::Accept(under) => assert_eq!(under.name, "a"),
        other => panic!("the tab under the pointer should take the drop, got {other:?}"),
    }
    assert_eq!(active(&mut cx), TabId(3), "not yet");
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert_eq!(active(&mut cx), TabId(2), "the tab opened under the held files");

    // Dropped on the tab for B (now hidden): the files are copied into its folder.
    let (x, y) = middle(&mut cx, "tab-3");
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Over {
            x,
            y,
            effect: DropEffect::Copy,
        },
    );
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Drop {
            paths: files.clone(),
            x,
            y,
            effect: DropEffect::Copy,
        },
    );
    assert!(
        disk.ops().iter().any(|op| matches!(op, FileOp::Copy { sources, dest }
            if sources == &files && dest == "C:\\b")),
        "the files were copied into B, got {:?}",
        disk.ops()
    );
}

#[gpui_kit::test]
fn files_dropped_on_a_tab_with_an_arabic_folder_move_there_with_the_move_effect_in_arabic(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _, disk) = open_on_disk(cx, true);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\تقارير"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    let answers = DropAnswers::default();
    let files = vec!["C:\\a\\one.txt".to_string()];
    let (x, y) = middle(&mut cx, "tab-2");
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Enter {
            paths: files.clone(),
            x,
            y,
            effect: DropEffect::Move,
        },
    );
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Over {
            x,
            y,
            effect: DropEffect::Move,
        },
    );
    assert!(matches!(answers.get(), Answer::Accept(under) if under.name == "تقارير"));
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Drop {
            paths: files.clone(),
            x,
            y,
            effect: DropEffect::Move,
        },
    );
    assert!(
        disk.ops()
            .iter()
            .any(|op| matches!(op, FileOp::Move { dest, .. } if dest == "C:\\تقارير")),
        "moved into the Arabic folder, got {:?}",
        disk.ops()
    );
}

#[gpui_kit::test]
fn the_first_tab_of_this_pc_refuses_files(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    let answers = DropAnswers::default();
    let (x, y) = middle(&mut cx, "tab-1");
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Enter {
            paths: vec!["C:\\a\\one.txt".into()],
            x,
            y,
            effect: DropEffect::Copy,
        },
    );
    drag_event(
        &mut cx,
        &answers,
        DropEvent::Over {
            x,
            y,
            effect: DropEffect::Copy,
        },
    );
    assert_eq!(answers.get(), Answer::Refuse);
}

#[gpui_kit::test]
fn the_new_tab_button_follows_the_last_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    let last = bounds(&mut cx, "tab-2");
    let plus = snapshot(&mut cx, "New tab").unwrap().2;
    assert!(
        plus.origin.x >= last.origin.x + last.size.width && plus.origin.x - (last.origin.x + last.size.width) < px(40.),
        "the + sits right after the last tab, not at the far end"
    );
}

// ---- A tab pulled out of the window -----------------------------------------------------------------

use crate::platform::secondary::{self, Spawn};
use std::sync::{Arc, Mutex};

/// Windows started by the test's spawner (answering `answer`), in order.
fn record_windows(cx: &mut VisualTestContext, answer: Result<(), String>) -> Arc<Mutex<Vec<Spawn>>> {
    let spawned = Arc::new(Mutex::new(Vec::new()));
    let seen = spawned.clone();
    cx.update(|_, cx| {
        secondary::use_spawner(
            cx,
            Arc::new(move |spawn: &Spawn| {
                seen.lock().unwrap_or_else(|e| e.into_inner()).push(spawn.clone());
                answer.clone()
            }),
        )
    });
    spawned
}

fn started(spawned: &Arc<Mutex<Vec<Spawn>>>) -> Vec<secondary::SecondaryWindow> {
    spawned
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|spawn| secondary::parse(spawn.args.clone()).ok().flatten())
        .collect()
}

fn settle(cx: &mut VisualTestContext) {
    for _ in 0..40 {
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

#[test]
fn a_release_is_a_tear_off_beyond_the_edge_or_well_away_from_the_drag() {
    assert_eq!(tear_off_for(false, px(0.)), TearOff::Now);
    assert_eq!(tear_off_for(false, px(500.)), TearOff::Now);
    assert_eq!(tear_off_for(true, px(90.)), TearOff::No, "90 is not more than 90");
    assert_eq!(tear_off_for(true, px(91.)), TearOff::IfNotTaken);
    assert_eq!(tear_off_for(true, px(4.)), TearOff::No);
}

#[gpui_kit::test]
fn a_tab_dropped_beyond_the_window_opens_its_folder_in_a_window_there_and_closes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    let spawned = record_windows(&mut cx, Ok(()));
    let from = bounds(&mut cx, "tab-2").center();
    let to = point(px(-60.), px(-30.));
    cx.update(|window, cx| window.drag(from, to, cx));
    settle(&mut cx);
    // The window's corner is up and left of the drop, in screen pixels.
    let (drop, scale) = cx.update(|window, _| (secondary::screen_point(window, to), window.scale_factor()));
    let windows = started(&spawned);
    assert_eq!(windows.len(), 1, "one new window");
    assert_eq!(windows[0].folder, "C:\\a", "the dragged tab's folder");
    assert_eq!(windows[0].at, Some(secondary::corner_for_drop(drop, scale)));
    assert_eq!(windows[0].main, Some(std::process::id()));
    assert_eq!(
        names(&mut cx, 0).len(),
        2,
        "the tab went with its folder: {:?}",
        names(&mut cx, 0)
    );
    assert!(!names(&mut cx, 0).contains(&"a".to_string()));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_last_tab_is_torn_off_but_stays_and_a_window_that_cannot_start_keeps_its_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    // One tab left, on a real folder (the fake disk has no home folder to give This PC's tab).
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.close_tab(Some(TabId(1)), cx));
    assert_eq!(names(&mut cx, 0), ["a"]);
    let spawned = record_windows(&mut cx, Ok(()));
    let from = bounds(&mut cx, "tab-2").center();
    cx.update(|window, cx| window.drag(from, point(px(2000.), px(-40.)), cx));
    settle(&mut cx);
    assert_eq!(started(&spawned).len(), 1, "a window opens on the only tab's folder");
    assert_eq!(names(&mut cx, 0).len(), 1, "but the last tab stays");

    // A second tab, and a window that Windows will not start: the tab stays too, and the person is told.
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    let failing = record_windows(&mut cx, Err("Windows said no".into()));
    let from = bounds(&mut cx, "tab-3").center();
    cx.update(|window, cx| window.drag(from, point(px(-80.), px(300.)), cx));
    settle(&mut cx);
    assert_eq!(failing.lock().unwrap_or_else(|e| e.into_inner()).len(), 1);
    assert_eq!(names(&mut cx, 0).len(), 2);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_tab_dropped_in_the_strip_only_moves(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut cx, _) = open(cx, false);
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    act(&mut cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    let spawned = record_windows(&mut cx, Ok(()));
    let first = bounds(&mut cx, "tab-1");
    let from = bounds(&mut cx, "tab-3").center();
    let to = point(first.origin.x + px(4.), first.center().y);
    cx.update(|window, cx| window.drag(from, to, cx));
    settle(&mut cx);
    assert_eq!(order(&mut cx, 0), [3, 1, 2]);
    assert!(
        started(&spawned).is_empty(),
        "no window for a tab put back in the strip"
    );
    testing::set_rtl(false);
}
