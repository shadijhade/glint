//! Headless tests of the frequent folders: the ranking, what is drawn (with Arabic names, in both directions),
//! and what a click, a middle click, a right click and the keys do. They run on a fake disk behind the
//! explorer's `Source`.

use super::*;
use crate::explorer::fake::{file, FakeDisk};
use crate::explorer::{Explorer, Memory};
use crate::state;
use crate::testing;
use glint_model::store::{AppState, StartFolder};
use gpui_kit::test::{ClickOptions, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Entity, MouseButton, TestAppContext, VisualTestContext};
use std::cell::RefCell;

const DOCS: &str = "C:\\Users\\me\\Documents";
const WORK: &str = "D:\\Work";
const ARABIC: &str = "C:\\Users\\me\\التقارير";

fn visit(count: u64, last: i64) -> Visit {
    Visit { count, last }
}

fn visits(list: &[(&str, Visit)]) -> OrderedMap<Visit> {
    let mut map = OrderedMap::new();
    for (path, visit) in list {
        map.insert((*path).to_string(), *visit);
    }
    map
}

const NOW: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;

#[test]
fn the_most_frecent_folders_come_first_and_only_folders_count() {
    let map = visits(&[
        // Seen eight days ago, ten times: 10 x 0.25.
        ("C:\\old", visit(10, NOW - 8 * DAY)),
        // Seen now, once: 1 x 4.
        ("C:\\new", visit(1, NOW)),
        // Seen now, three times: 3 x 4.
        ("C:\\hot", visit(3, NOW)),
        // Not folders on disk: Recent, This PC's own empty path, the Recycle Bin and a remote connection.
        ("shell:Recent", visit(99, NOW)),
        ("", visit(99, NOW)),
        ("shell:RecycleBinFolder", visit(99, NOW)),
        ("remote:nas/", visit(99, NOW)),
    ]);
    assert_eq!(frequent(&map, NOW), ["C:\\hot", "C:\\new", "C:\\old"]);
}

#[test]
fn only_eight_show_and_ties_keep_the_order_the_visits_are_kept_in() {
    let paths: Vec<String> = (0..12).map(|i| format!("C:\\f{i}")).collect();
    let list: Vec<(&str, Visit)> = paths.iter().map(|p| (p.as_str(), visit(2, NOW))).collect();
    let shown = frequent(&visits(&list), NOW);
    assert_eq!(shown.len(), MOST);
    assert_eq!(
        shown,
        paths[..MOST],
        "equal ranks stay in their order, like a stable sort"
    );
    assert!(frequent(&OrderedMap::new(), NOW).is_empty());
}

fn setup(
    cx: &mut TestAppContext,
    visits: OrderedMap<Visit>,
    rtl: bool,
) -> (VisualTestContext, Entity<FrequentFolders>, Entity<Explorer>, FakeDisk) {
    cx.update(testing::init);
    cx.update(|cx| state::update(cx, |settings| settings.start_folder = StartFolder::ThisPc));
    testing::set_rtl(rtl);
    let disk = FakeDisk::new();
    for folder in [DOCS, WORK, ARABIC, "C:\\Users\\me", "C:\\"] {
        disk.folder(folder, vec![file(folder, "a.txt", 1, 1)]);
    }
    cx.update(|cx| crate::explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let (mut visual, view) = testing::open_window(cx, |window, cx| cx.new(|cx| FrequentFolders::new(0, window, cx)));
    visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            let app = AppState {
                visits,
                ..AppState::default()
            };
            e.set_memory(Memory::from_app_state(app), cx);
            e.init_explorer(Vec::new(), cx).detach();
        })
    });
    settle(&mut visual);
    (visual, view, explorer, disk)
}

fn recent_visits() -> OrderedMap<Visit> {
    let now = now_ms();
    visits(&[(DOCS, visit(5, now)), (WORK, visit(2, now)), (ARABIC, visit(9, now))])
}

fn settle(visual: &mut VisualTestContext) {
    for _ in 0..20 {
        visual.run_until_parked();
    }
}

fn frame(visual: &mut VisualTestContext) {
    visual.update(|window, cx| window.render_frame(cx));
}

fn role_of(visual: &mut VisualTestContext, label: &str) -> Option<Role> {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).and_then(|element| element.role())
    })
}

fn path(visual: &mut VisualTestContext, explorer: &Entity<Explorer>) -> String {
    explorer.read_with(&*visual, |e, _| {
        e.pane_tab(0).map(|t| t.path.clone()).unwrap_or_default()
    })
}

fn press(visual: &mut VisualTestContext, key: &str) {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    });
    settle(visual);
}

fn focused_tile(visual: &mut VisualTestContext, view: &Entity<FrequentFolders>) -> Option<usize> {
    visual.update(|window, cx| {
        view.read(cx)
            .tiles
            .iter()
            .position(|tile| tile.focus.is_focused(window))
    })
}

#[gpui_kit::test]
fn the_tiles_are_named_buttons_best_first_with_an_arabic_name(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer, _disk) = setup(cx, recent_visits(), false);
    // The Arabic folder was opened most, then Documents, then Work.
    assert_eq!(view.read_with(&visual, |v, _| v.paths()), [ARABIC, DOCS, WORK]);
    assert_eq!(role_of(&mut visual, "Frequent folders"), Some(Role::Group));
    assert_eq!(role_of(&mut visual, "التقارير"), Some(Role::Button));
    assert_eq!(role_of(&mut visual, "Documents"), Some(Role::Button));
    assert_eq!(role_of(&mut visual, "Work"), Some(Role::Button));
}

#[gpui_kit::test]
fn a_root_is_named_by_its_drive_and_shows_its_whole_path_for_parent(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer, _disk) = setup(cx, visits(&[("C:\\", visit(3, now_ms()))]), false);
    frame(&mut visual);
    assert_eq!(view.read_with(&visual, |v, _| v.paths()), ["C:\\"]);
    assert_eq!(role_of(&mut visual, "C:"), Some(Role::Button));
    let (name, parent) = view.read_with(&visual, |v, _| (v.tiles[0].name.clone(), v.tiles[0].parent.clone()));
    assert_eq!(
        (name.as_ref(), parent.as_ref()),
        ("C:", "C:\\"),
        "a root has no parent folder to name"
    );
}

#[gpui_kit::test]
fn with_no_visits_nothing_is_drawn(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer, _disk) = setup(cx, OrderedMap::new(), false);
    assert!(view.read_with(&visual, |v, _| v.is_empty()));
    assert_eq!(
        role_of(&mut visual, "Frequent folders"),
        None,
        "no group, as `return null`"
    );
}

#[gpui_kit::test]
fn a_click_opens_the_folder_in_this_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, recent_visits(), false);
    frame(&mut visual);
    visual.update(|window, cx| window.click(("frequent-folder", 1_usize), cx));
    settle(&mut visual);
    assert_eq!(path(&mut visual, &explorer), DOCS);
}

#[gpui_kit::test]
fn a_middle_click_opens_a_new_tab_and_stays_here(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, recent_visits(), false);
    frame(&mut visual);
    visual.update(|window, cx| {
        window.click_with_options(
            ("frequent-folder", 2_usize),
            ClickOptions::default().with_button(MouseButton::Middle),
            cx,
        );
    });
    settle(&mut visual);
    explorer.read_with(&visual, |e, _| {
        let tabs = &e.panes()[0].tabs;
        assert_eq!(tabs.len(), 2);
        assert_eq!(e.tab(tabs[1]).map(|t| t.path.as_str()), Some(WORK));
        assert_ne!(
            e.tab(tabs[0]).map(|t| t.path.as_str()),
            Some(WORK),
            "this tab stays where it was"
        );
    });
}

#[gpui_kit::test]
fn a_right_click_asks_for_the_folders_menu_and_opens_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer, _disk) = setup(cx, recent_visits(), false);
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &FrequentFoldersEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let before = path(&mut visual, &explorer);
    frame(&mut visual);
    visual.update(|window, cx| window.right_click(("frequent-folder", 0_usize), cx));
    settle(&mut visual);
    let raised = events.borrow();
    assert!(matches!(raised.as_slice(), [FrequentFoldersEvent::ContextMenu { path, .. }] if path == ARABIC));
    assert_eq!(path(&mut visual, &explorer), before);
}

#[gpui_kit::test]
fn the_arrows_move_between_tiles_and_enter_opens(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer, _disk) = setup(cx, recent_visits(), false);
    frame(&mut visual);
    // Tab puts the keyboard on the first tile.
    visual.update(|window, cx| window.focus_next(cx));
    press(&mut visual, "right");
    press(&mut visual, "right");
    press(&mut visual, "left");
    assert_eq!(
        focused_tile(&mut visual, &view),
        Some(1),
        "right, right, left from the first tile"
    );
    press(&mut visual, "enter");
    assert_eq!(path(&mut visual, &explorer), DOCS);
}

#[gpui_kit::test]
fn in_arabic_the_first_tile_is_at_the_right_and_the_arrows_follow_the_reading_direction(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer, _disk) = setup(cx, recent_visits(), true);
    frame(&mut visual);
    let (first, second) = visual.update(|window, _| {
        (
            window.try_find(("frequent-folder", 0_usize)).map(|e| e.bounds()),
            window.try_find(("frequent-folder", 1_usize)).map(|e| e.bounds()),
        )
    });
    let (first, second) = (first.expect("first tile"), second.expect("second tile"));
    assert!(
        first.origin.x > second.origin.x,
        "the best folder starts at the right edge: {first:?} {second:?}"
    );
    // In Arabic the left arrow is "next".
    visual.update(|window, cx| window.focus_next(cx));
    press(&mut visual, "left");
    assert_eq!(focused_tile(&mut visual, &view), Some(1));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_ranking_is_redone_when_the_visits_change(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer, _disk) = setup(cx, recent_visits(), false);
    assert_eq!(view.read_with(&visual, |v, _| v.paths()).len(), 3);
    visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            let mut app = e.memory().app().clone();
            app.visits.insert(WORK.to_string(), visit(100, now_ms()));
            e.set_memory(Memory::from_app_state(app), cx);
        })
    });
    settle(&mut visual);
    assert_eq!(
        view.read_with(&visual, |v, _| v.paths())[0],
        WORK,
        "Work was opened a hundred times"
    );
}

#[gpui_kit::test]
fn it_is_for_recent_without_a_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, disk) = setup(cx, recent_visits(), false);
    disk.folder("shell:Recent", vec![]);
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.navigate("shell:Recent", NavOptions::default(), cx)));
    settle(&mut visual);
    explorer.read_with(&visual, |e, _| {
        let tab = e.pane_tab(0).expect("a tab");
        assert!(shown_for(tab), "Recent gets them");
    });
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.navigate(DOCS, NavOptions::default(), cx)));
    settle(&mut visual);
    explorer.read_with(&visual, |e, _| assert!(!shown_for(e.pane_tab(0).expect("a tab"))));
}
