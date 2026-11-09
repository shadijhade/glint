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
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk().source()));
    testing::set_rtl(rtl);
    testing::open_window(cx, |window, cx| cx.new(|cx| TabStrip::new(window, cx)))
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
