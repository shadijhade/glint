//! Headless tests of This PC: what it draws, what a click and a key do to the explorer, and the Arabic layout.
//! They run on a fake disk behind the explorer's `Source`, so nothing touches the real drives. The drive health
//! model asks core once when the view starts, as the app does; its answer doesn't matter here.

use super::*;
use crate::drops::dispatch;
use crate::explorer::fake::{file, FakeDisk};
use crate::explorer::{Explorer, FileOp, KnownFolder, KnownId};
use crate::platform::dragdrop::{Answer, DropAnswers, DropEvent};
use crate::state;
use crate::testing;
use glint_model::format::format_size;
use glint_model::paths::THIS_PC;
use glint_model::store::StartFolder;
use gpui_kit::test::{ClickOptions, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{ElementId, Entity, MouseButton, TestAppContext, VisualTestContext};
use std::cell::RefCell;

const HOME: &str = "C:\\Users\\me";
const DOWNLOADS: &str = "C:\\Users\\me\\Downloads";

fn drive(path: &str, label: &str, kind: DriveKind, fs: &str, total: u64, free: u64) -> Drive {
    Drive {
        path: path.into(),
        label: label.into(),
        kind,
        fs: fs.into(),
        total,
        free,
    }
}

/// C: named "Windows" with 300 of 500 used, D: with no size (not ready), E: an empty USB drive of 32.
fn standard_drives() -> Vec<Drive> {
    vec![
        drive("C:\\", "Windows", DriveKind::Fixed, "NTFS", 500, 200),
        drive("D:\\", "", DriveKind::Fixed, "", 0, 0),
        drive("E:\\", "", DriveKind::Removable, "FAT32", 32, 8),
    ]
}

/// The explorer on a fake disk with Home and Downloads, opened on This PC in pane 0. The view is the window's
/// content. `rtl` sets Arabic before anything is drawn.
fn setup(
    cx: &mut TestAppContext,
    drives: Vec<Drive>,
    rtl: bool,
) -> (VisualTestContext, Entity<ThisPc>, Entity<Explorer>, FakeDisk) {
    cx.update(testing::init);
    // Start-up shows This PC, not Home (the default start folder).
    cx.update(|cx| state::update(cx, |settings| settings.start_folder = StartFolder::ThisPc));
    testing::set_rtl(rtl);
    let disk = FakeDisk::new();
    disk.folder(HOME, vec![]);
    disk.folder(DOWNLOADS, vec![file(DOWNLOADS, "report.pdf", 10, 1)]);
    disk.folder("C:\\", vec![]);
    disk.folder("E:\\", vec![]);
    disk.set_known(vec![
        KnownFolder {
            id: KnownId::Home,
            name: "Home",
            path: HOME.into(),
        },
        KnownFolder {
            id: KnownId::Downloads,
            name: "Downloads",
            path: DOWNLOADS.into(),
        },
    ]);
    disk.set_drives(drives);
    cx.update(|cx| crate::explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let (mut visual, view) = testing::open_window(cx, |window, cx| cx.new(|cx| ThisPc::new(0, window, cx)));
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx).detach()));
    settle(&mut visual);
    (visual, view, explorer, disk)
}

/// Lets the fake disk's answers and the explorer's tasks run until nothing is left.
fn settle(visual: &mut VisualTestContext) {
    for _ in 0..20 {
        visual.run_until_parked();
    }
}

/// Completes a frame, so queries and key presses see what is drawn now.
fn frame(visual: &mut VisualTestContext) {
    visual.update(|window, cx| window.render_frame(cx));
}

fn has(visual: &mut VisualTestContext, label: &str) -> bool {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).is_some()
    })
}

fn role_of(visual: &mut VisualTestContext, label: &str) -> Option<Role> {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).and_then(|element| element.role())
    })
}

fn click(visual: &mut VisualTestContext, id: impl Into<ElementId>) {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
    settle(visual);
}

fn press(visual: &mut VisualTestContext, key: &str) {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    });
    settle(visual);
}

/// The tab key's job: the first tile takes the focus.
fn focus_first_tile(visual: &mut VisualTestContext) {
    frame(visual);
    visual.update(|window, cx| window.focus_next(cx));
}

/// Navigates the explorer's pane 0 to `target`, and lets the listing arrive.
fn navigate(visual: &mut VisualTestContext, explorer: &Entity<Explorer>, target: &str) {
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.navigate(target, NavOptions::default(), cx)));
    settle(visual);
}

/// The path the pane's tab shows now.
fn path(visual: &mut VisualTestContext, explorer: &Entity<Explorer>) -> String {
    explorer.read_with(&*visual, |e, _| {
        e.pane_tab(0).map(|t| t.path.clone()).unwrap_or_default()
    })
}

/// Every event the view raises from now on.
fn record(visual: &mut VisualTestContext, view: &Entity<ThisPc>) -> Rc<RefCell<Vec<ThisPcEvent>>> {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(view, move |_, event: &ThisPcEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    events
}

#[gpui_kit::test]
fn folders_and_drives_are_named_buttons_and_the_totals_add_up(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, _explorer, _disk) = setup(cx, standard_drives(), false);
    assert_eq!(role_of(&mut visual, "This PC"), Some(Role::Group));
    assert_eq!(role_of(&mut visual, "Folders"), Some(Role::Group));
    assert_eq!(role_of(&mut visual, "Home"), Some(Role::Button));
    assert_eq!(role_of(&mut visual, "Downloads"), Some(Role::Button));
    assert_eq!(role_of(&mut visual, "Windows (C:)"), Some(Role::Button));
    // A drive with no label is called by its kind.
    assert_eq!(role_of(&mut visual, "Local Disk (D:)"), Some(Role::Button));
    assert_eq!(role_of(&mut visual, "USB Drive (E:)"), Some(Role::Button));
    // The totals count only drives with a size: 500 + 32 of capacity, 200 + 8 free, 324 used (61%).
    let line = format!(
        "Total: {} used of {} · {} free · 61%",
        format_size(324),
        format_size(532),
        format_size(208)
    );
    assert_eq!(role_of(&mut visual, &line), Some(Role::Group));
    assert_eq!(role_of(&mut visual, "Devices and drives"), Some(Role::Group));
}

#[gpui_kit::test]
fn a_click_on_a_folder_or_a_drive_opens_it_in_this_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, standard_drives(), false);
    click(&mut visual, ("this-pc-folder", 1_usize));
    assert_eq!(path(&mut visual, &explorer), DOWNLOADS);
    navigate(&mut visual, &explorer, THIS_PC);
    assert_eq!(path(&mut visual, &explorer), THIS_PC);
    // C: is the third tile: after the two folders.
    click(&mut visual, ("this-pc-drive", 2_usize));
    assert_eq!(path(&mut visual, &explorer), "C:\\");
}

#[gpui_kit::test]
fn a_middle_click_opens_the_folder_in_a_new_tab_and_stays_here(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, standard_drives(), false);
    frame(&mut visual);
    visual.update(|window, cx| {
        window.click_with_options(
            ("this-pc-folder", 0_usize),
            ClickOptions::default().with_button(MouseButton::Middle),
            cx,
        );
    });
    settle(&mut visual);
    explorer.read_with(&visual, |e, _| {
        let tabs = &e.panes()[0].tabs;
        assert_eq!(tabs.len(), 2, "one new tab beside This PC");
        assert_eq!(e.tab(tabs[1]).map(|t| t.path.as_str()), Some(HOME));
        assert_eq!(
            e.tab(tabs[0]).map(|t| t.path.as_str()),
            Some(THIS_PC),
            "This PC stays where it was"
        );
    });
}

#[gpui_kit::test]
fn a_right_click_asks_for_the_menu_of_the_tile(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer, _disk) = setup(cx, standard_drives(), false);
    let events = record(&mut visual, &view);
    frame(&mut visual);
    visual.update(|window, cx| window.right_click(("this-pc-folder", 0_usize), cx));
    settle(&mut visual);
    let raised = events.borrow();
    assert!(
        matches!(raised.as_slice(), [ThisPcEvent::ContextMenu { path, .. }] if path == HOME),
        "the folder's menu was asked for"
    );
    // The menu doesn't open the folder.
    assert_eq!(path(&mut visual, &explorer), THIS_PC);
}

#[gpui_kit::test]
fn the_drive_actions_say_which_drive_they_are_for_and_leave_it_closed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer, _disk) = setup(cx, standard_drives(), false);
    let events = record(&mut visual, &view);
    // The drives follow the two folders, so C: is the drive at index 2.
    click(&mut visual, ("this-pc-recent", 2_usize));
    click(&mut visual, ("this-pc-duplicates", 2_usize));
    assert_eq!(
        path(&mut visual, &explorer),
        THIS_PC,
        "the actions don't open the drive"
    );
    let raised = events.borrow();
    assert!(
        matches!(
            raised.as_slice(),
            [ThisPcEvent::RecentChanges { path: first }, ThisPcEvent::FindDuplicates { path: second }]
                if first == "C:\\" && second == "C:\\"
        ),
        "the two actions raised their events: {raised:?}"
    );
}

#[gpui_kit::test]
fn map_disk_usage_opens_the_drive_and_asks_for_the_map(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer, _disk) = setup(cx, standard_drives(), false);
    let events = record(&mut visual, &view);
    click(&mut visual, ("this-pc-map", 2_usize));
    assert_eq!(path(&mut visual, &explorer), "C:\\");
    let tab = explorer.read_with(&visual, |e, _| e.pane_tab_id(0));
    let raised = events.borrow();
    assert!(
        matches!(raised.as_slice(), [ThisPcEvent::DiskUsage { tab: shown, path }] if *shown == tab && path == "C:\\"),
        "the usage map was asked for on the drive's tab"
    );
}

#[gpui_kit::test]
fn the_first_tab_stop_is_the_first_folder_and_arrows_move_between_tiles(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, standard_drives(), false);
    focus_first_tile(&mut visual);
    // Right moves to Downloads, and Enter opens the tile with the focus.
    press(&mut visual, "right");
    press(&mut visual, "enter");
    assert_eq!(path(&mut visual, &explorer), DOWNLOADS);
}

#[gpui_kit::test]
fn down_from_the_folders_lands_in_the_drives_and_space_opens_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, standard_drives(), false);
    focus_first_tile(&mut visual);
    // Both folders fit in one row, so Down from Home goes to the first drive, C:.
    press(&mut visual, "down");
    press(&mut visual, "space");
    assert_eq!(path(&mut visual, &explorer), "C:\\");
}

#[gpui_kit::test]
fn end_goes_to_the_last_tile_and_enter_opens_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, standard_drives(), false);
    focus_first_tile(&mut visual);
    press(&mut visual, "end");
    press(&mut visual, "enter");
    assert_eq!(path(&mut visual, &explorer), "E:\\", "the last tile is the USB drive");
}

#[gpui_kit::test]
fn home_goes_back_to_the_first_tile(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, standard_drives(), false);
    focus_first_tile(&mut visual);
    press(&mut visual, "end");
    press(&mut visual, "home");
    press(&mut visual, "enter");
    assert_eq!(path(&mut visual, &explorer), HOME);
}

#[gpui_kit::test]
fn a_removable_drive_without_a_size_still_offers_eject(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let drives = vec![drive("E:\\", "", DriveKind::Removable, "", 0, 0)];
    let (mut visual, _view, _explorer, _disk) = setup(cx, drives, false);
    assert_eq!(role_of(&mut visual, "Safely eject USB Drive (E:)"), Some(Role::Button));
    assert!(!has(&mut visual, "Map disk usage"), "no size, no map");
}

#[gpui_kit::test]
fn a_fixed_drive_without_a_size_has_no_actions(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let drives = vec![drive("D:\\", "", DriveKind::Fixed, "", 0, 0)];
    let (mut visual, _view, _explorer, _disk) = setup(cx, drives, false);
    assert_eq!(role_of(&mut visual, "Local Disk (D:)"), Some(Role::Button));
    assert!(!has(&mut visual, "Safely eject Local Disk (D:)"));
    assert!(!has(&mut visual, "Recent changes"));
    assert!(!has(&mut visual, "Find duplicate files"));
}

#[gpui_kit::test]
fn this_pc_draws_nothing_while_the_pane_shows_a_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, explorer, _disk) = setup(cx, standard_drives(), false);
    assert!(has(&mut visual, "Folders"));
    navigate(&mut visual, &explorer, DOWNLOADS);
    assert!(!has(&mut visual, "Folders"));
    assert!(!has(&mut visual, "Windows (C:)"));
}

#[gpui_kit::test]
fn an_arabic_drive_name_is_drawn_and_the_arrows_mirror(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let drives = vec![
        drive("C:\\", "قرص النظام", DriveKind::Fixed, "NTFS", 500, 200),
        drive("E:\\", "", DriveKind::Removable, "FAT32", 32, 8),
    ];
    let (mut visual, _view, explorer, _disk) = setup(cx, drives, true);
    assert_eq!(role_of(&mut visual, "قرص النظام (C:)"), Some(Role::Button));
    focus_first_tile(&mut visual);
    // In Arabic the arrow that points to the left moves forward through the tiles.
    press(&mut visual, "left");
    press(&mut visual, "enter");
    assert_eq!(path(&mut visual, &explorer), DOWNLOADS);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_known_folder_is_named_in_the_interface_language(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.update(testing::init);
    testing::set_rtl(true);
    let disk = FakeDisk::new();
    disk.set_known(vec![KnownFolder {
        id: KnownId::Documents,
        name: "Documents",
        path: "C:\\Users\\me\\Documents".into(),
    }]);
    disk.set_drives(standard_drives());
    cx.update(|cx| crate::explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let (mut visual, _view) = testing::open_window(cx, |window, cx| cx.new(|cx| ThisPc::new(0, window, cx)));
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx).detach()));
    settle(&mut visual);
    // The catalog has Documents in Arabic: the tile says so, and its name is still exposed to screen readers.
    assert_eq!(role_of(&mut visual, "المستندات"), Some(Role::Button));
    testing::set_rtl(false);
}

/// Where tile `index` (folders first) was painted last: its middle, in window pixels, as the drop target sees it.
fn tile_middle(visual: &mut VisualTestContext, view: &Entity<ThisPc>, index: usize) -> (f32, f32) {
    frame(visual);
    let center = view.read_with(&*visual, |v, _| {
        let known = v.known.len();
        let places = if index < known {
            v.folder_bounds.borrow().get(index).cloned()
        } else {
            v.drive_bounds.borrow().get(index - known).cloned()
        };
        places.map(|bounds| bounds.center())
    });
    let center = center.expect("the tile is painted");
    (f32::from(center.x), f32::from(center.y))
}

/// A drag of `paths` over the point (x, y), dropped there with `effect` if the tile takes it. Returns the answer
/// the tile gave while the drag was over the point.
fn drop_at(visual: &mut VisualTestContext, paths: &[&str], (x, y): (f32, f32), effect: DropEffect) -> Answer {
    let files: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
    let answers = DropAnswers::default();
    visual.update(|window, cx| {
        window.render_frame(cx);
        dispatch(
            DropEvent::Enter {
                paths: files.clone(),
                x,
                y,
                effect: DropEffect::Copy,
            },
            &answers,
            window,
            cx,
        );
        dispatch(DropEvent::Over { x, y, effect }, &answers, window, cx);
    });
    let answer = answers.get();
    // What the target sends with the drop: none where the tile refused it.
    let effect = if matches!(answer, Answer::Accept(_)) {
        effect
    } else {
        DropEffect::None
    };
    visual.update(|window, cx| {
        dispatch(
            DropEvent::Drop {
                paths: files.clone(),
                x,
                y,
                effect,
            },
            &answers,
            window,
            cx,
        );
    });
    settle(visual);
    answer
}

#[gpui_kit::test]
fn files_dropped_on_a_folder_tile_are_copied_into_it_and_the_drag_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer, disk) = setup(cx, standard_drives(), false);
    let downloads = tile_middle(&mut visual, &view, 1);
    let source = "C:\\Users\\me\\Desktop\\ملف.txt";
    match drop_at(&mut visual, &[source], downloads, DropEffect::Copy) {
        Answer::Accept(under) => {
            assert_eq!(under.name, "Downloads");
            assert_eq!(under.copy, "Copy to %1", "the words stand for the folder's name");
            assert_eq!(under.moves, "Move to %1");
        }
        other => panic!("the folder takes the drop, got {other:?}"),
    }
    let copied = disk.ops().iter().any(|op| {
        matches!(op, FileOp::Copy { sources, dest }
            if dest == DOWNLOADS && sources.len() == 1 && sources[0] == source)
    });
    assert!(copied, "the file was copied into Downloads: {:?}", disk.ops());
}

#[gpui_kit::test]
fn shift_on_a_drop_moves_the_files_and_the_words_are_arabic_in_arabic(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer, disk) = setup(cx, standard_drives(), true);
    let downloads = tile_middle(&mut visual, &view, 1);
    let answer = drop_at(
        &mut visual,
        &["C:\\Users\\me\\Desktop\\ملف.txt"],
        downloads,
        DropEffect::Move,
    );
    assert!(
        matches!(&answer, Answer::Accept(under) if under.copy == "نسخ إلى %1" && under.moves == "نقل إلى %1"),
        "the words are in Arabic: {answer:?}"
    );
    assert!(
        disk.ops().iter().any(|op| matches!(op, FileOp::Move { .. })),
        "Shift moved the file: {:?}",
        disk.ops()
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_cd_drive_and_the_folder_the_files_are_in_refuse_a_drop(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let drives = vec![
        drive("C:\\", "Windows", DriveKind::Fixed, "NTFS", 500, 200),
        drive("D:\\", "", DriveKind::Cdrom, "", 0, 0),
    ];
    let (mut visual, view, _explorer, disk) = setup(cx, drives, false);
    // The drives follow the two folders: the disc is at index 3.
    let disc = tile_middle(&mut visual, &view, 3);
    assert!(
        matches!(
            drop_at(&mut visual, &["C:\\Users\\me\\Desktop\\a.txt"], disc, DropEffect::Copy),
            Answer::Refuse
        ),
        "a disc is read-only"
    );
    let downloads = tile_middle(&mut visual, &view, 1);
    let already_there = format!("{DOWNLOADS}\\notes.txt");
    assert!(
        matches!(
            drop_at(&mut visual, &[already_there.as_str()], downloads, DropEffect::Copy),
            Answer::Refuse
        ),
        "the files are in that folder already"
    );
    assert!(disk.ops().is_empty(), "nothing was copied or moved: {:?}", disk.ops());
}

#[gpui_kit::test]
fn the_tile_under_the_files_lights_up_and_lets_go_when_they_leave(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer, _disk) = setup(cx, standard_drives(), false);
    let (x, y) = tile_middle(&mut visual, &view, 1);
    let answers = DropAnswers::default();
    let files = vec!["C:\\Users\\me\\Desktop\\a.txt".to_string()];
    visual.update(|window, cx| {
        window.render_frame(cx);
        dispatch(
            DropEvent::Enter {
                paths: files.clone(),
                x,
                y,
                effect: DropEffect::Copy,
            },
            &answers,
            window,
            cx,
        );
        dispatch(
            DropEvent::Over {
                x,
                y,
                effect: DropEffect::Copy,
            },
            &answers,
            window,
            cx,
        );
    });
    assert_eq!(
        view.read_with(&visual, |v, _| v.drop_over.get()),
        Some(1),
        "Downloads is lit"
    );
    // The pointer goes to the page's padding, where no tile is: the tile lets go.
    visual.update(|window, cx| {
        dispatch(
            DropEvent::Over {
                x: 5.,
                y: 5.,
                effect: DropEffect::Copy,
            },
            &answers,
            window,
            cx,
        );
    });
    assert_eq!(view.read_with(&visual, |v, _| v.drop_over.get()), None);
}

#[gpui_kit::test]
fn ejecting_a_drive_goes_through_the_explorer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _view, _explorer, disk) = setup(cx, standard_drives(), false);
    // E: is the third drive, after the two folders: index 4.
    click(&mut visual, ("this-pc-eject", 4_usize));
    assert_eq!(disk.ejected(), vec!["E:\\".to_string()]);
}

#[gpui_kit::test]
fn the_arrow_keys_scroll_the_page_to_bring_the_focused_tile_into_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let drives: Vec<Drive> = (b'C'..=b'Z')
        .map(|letter| {
            drive(
                &format!("{}:\\", letter as char),
                "",
                DriveKind::Fixed,
                "NTFS",
                500,
                200,
            )
        })
        .collect();
    let (mut visual, view, _explorer, _disk) = setup(cx, drives, false);
    assert_eq!(
        view.read_with(&visual, |v, _| v.scroll.offset().y),
        px(0.),
        "the page starts at the top"
    );
    focus_first_tile(&mut visual);
    press(&mut visual, "end");
    frame(&mut visual);
    let last = view
        .read_with(&visual, |v, _| v.drive_bounds.borrow().last().cloned())
        .expect("the last drive is painted");
    let page = view.read_with(&visual, |v, _| v.scroll.bounds());
    assert!(
        view.read_with(&visual, |v, _| v.scroll.offset().y) < px(0.),
        "the page scrolled down"
    );
    assert!(
        last.top() >= page.top() && last.bottom() <= page.bottom(),
        "the last tile is wholly in view"
    );
}
