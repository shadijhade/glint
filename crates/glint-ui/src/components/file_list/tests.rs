//! Headless tests of the details view, on a fake disk behind the explorer: what is drawn (labels, roles), that
//! presses and keys change the explorer as the TypeScript did, an Arabic name, and the Arabic interface.

use super::*;
use crate::explorer::fake::{file, folder, hidden, FakeDisk};
use crate::explorer::NavOptions;
use crate::testing;
use glint_model::columns::ColumnId;
use glint_model::entries::{attr, with_details, RawEntry, SortKey};
use glint_model::groups::GroupBy;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};
use std::cell::RefCell;

/// A window with a list on a folder of a fake disk, and the events the list raised.
struct Ui {
    visual: VisualTestContext,
    list: Entity<FileList>,
    explorer: Entity<Explorer>,
    events: Rc<RefCell<Vec<FileListEvent>>>,
}

/// `C:\x` with a folder, a few files and a hidden one.
fn small_disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\x",
        vec![
            folder("C:\\x", "docs"),
            file("C:\\x", "alpha.txt", 100, 0),
            file("C:\\x", "beta.md", 3000, 0),
            file("C:\\x", "berry.png", 20_000, 0),
            hidden("C:\\x", ".secret"),
        ],
    );
    disk.folder("C:\\x\\docs", vec![file("C:\\x\\docs", "inner.txt", 1, 0)]);
    disk
}

fn open(cx: &mut TestAppContext, disk: &FakeDisk, path: &str) -> Ui {
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let (mut visual, list) = testing::open_window(cx, |window, cx| cx.new(|cx| FileList::new(0, window, cx)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&list, move |_, event: &FileListEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    // Focus events (the rename box leaving focus, a click making a pane active) only come in an active window.
    visual.update(|window, _| window.activate_window());
    let mut ui = Ui {
        visual,
        list,
        explorer,
        events,
    };
    ui.focus();
    ui.frame();
    ui
}

impl Ui {
    /// Lets the explorer's work and a frame finish.
    fn frame(&mut self) {
        self.visual.run_until_parked();
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    fn focus(&mut self) {
        let list = self.list.clone();
        self.visual
            .update(|window, cx| list.update(cx, |list, cx| list.focus(window, cx)));
    }

    fn click(&mut self, i: usize) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("file-row", i), cx);
        });
        self.frame();
    }

    fn click_with(&mut self, i: usize, modifiers: Modifiers) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click_with_modifiers(("file-row", i), modifiers, cx);
        });
        self.frame();
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.frame();
    }

    /// The rows drawn, as a screen reader names them.
    fn labels(&mut self) -> Vec<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::ListBoxOption)
                .iter()
                .map(|row| row.label().unwrap_or_default().to_string())
                .collect()
        })
    }

    /// What the explorer has selected, by name, in the order selected.
    fn selected(&mut self) -> Vec<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.active_tab()
                .map(|t| {
                    t.selection
                        .iter()
                        .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    fn focused(&mut self) -> Option<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.active_tab().and_then(|t| {
                t.selection
                    .focus()
                    .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
            })
        })
    }

    fn path(&mut self) -> String {
        self.explorer
            .read_with(&self.visual, |e, _| e.current_path().to_string())
    }

    fn role_of(&mut self, label: &str) -> Option<Role> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn events(&self) -> Vec<FileListEvent> {
        self.events.borrow().clone()
    }
}

fn ctrl() -> Modifiers {
    Modifiers {
        control: true,
        ..Default::default()
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Default::default()
    }
}

#[gpui_kit::test]
fn the_rows_are_a_named_listbox_of_options_in_the_listings_order(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    // Folders first, then names; the hidden item is left out.
    assert_eq!(ui.labels(), ["docs", "alpha.txt", "berry.png", "beta.md"]);
    assert_eq!(ui.role_of("Files"), Some(Role::ListBox));
    // Each row says what it is for a screen reader beyond its name.
    ui.visual.update(|window, _| {
        let row = window.find(("file-row", 2usize));
        assert_eq!(row.role(), Some(Role::ListBoxOption));
        assert_eq!(row.selected(), Some(false));
        assert_eq!(row.description(), Some("PNG image, 20 KB"));
    });
}

#[gpui_kit::test]
fn the_header_has_a_button_per_column_that_sorts(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    for label in ["Name", "Date modified", "Type", "Size"] {
        assert_eq!(ui.role_of(label), Some(Role::Button), "{label}");
    }
    // The first click on Size puts the biggest first; the next turns it round.
    ui.visual.update(|window, cx| window.click("file-list-sort-Size", cx));
    ui.frame();
    let sort = ui.explorer.read_with(&ui.visual, |e, _| e.active_tab().map(|t| t.sort));
    assert_eq!(sort.map(|s| (s.key, s.desc)), Some((SortKey::Size, true)));
    assert_eq!(ui.labels(), ["docs", "berry.png", "beta.md", "alpha.txt"]);
    ui.visual.update(|window, cx| window.click("file-list-sort-Size", cx));
    ui.frame();
    let sort = ui.explorer.read_with(&ui.visual, |e, _| e.active_tab().map(|t| t.sort));
    assert_eq!(sort.map(|s| (s.key, s.desc)), Some((SortKey::Size, false)));
    // Name sorts ascending first.
    ui.visual.update(|window, cx| window.click("file-list-sort-Name", cx));
    ui.frame();
    let sort = ui.explorer.read_with(&ui.visual, |e, _| e.active_tab().map(|t| t.sort));
    assert_eq!(sort.map(|s| (s.key, s.desc)), Some((SortKey::Name, false)));
}

#[gpui_kit::test]
fn a_click_selects_only_it_ctrl_toggles_shift_extends_and_empty_space_clears(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(1);
    assert_eq!(ui.selected(), ["alpha.txt"]);
    assert_eq!(ui.focused().as_deref(), Some("alpha.txt"));
    ui.click_with(3, ctrl());
    assert_eq!(ui.selected(), ["alpha.txt", "beta.md"]);
    ui.click_with(3, ctrl());
    assert_eq!(
        ui.selected(),
        ["alpha.txt"],
        "Ctrl+click on a selected row takes it out"
    );
    // Shift from the anchor (the last Ctrl+clicked row).
    ui.click(0);
    ui.click_with(2, shift());
    assert_eq!(ui.selected(), ["docs", "alpha.txt", "berry.png"]);
    ui.visual.update(|window, _| {
        assert_eq!(window.find(("file-row", 1usize)).selected(), Some(true));
        assert_eq!(window.find(("file-row", 3usize)).selected(), Some(false));
    });
    // A press below the last row clears the selection.
    ui.visual.update(|window, cx| {
        window.click_at("file-list-area", point(px(300.), px(400.)), cx);
    });
    ui.frame();
    assert!(ui.selected().is_empty());
}

#[gpui_kit::test]
fn a_press_on_a_selection_of_several_keeps_it_until_the_button_is_let_go(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(1);
    ui.click_with(2, shift());
    assert_eq!(ui.selected(), ["alpha.txt", "berry.png"]);
    // The click is a press and a release: the release narrows to the pressed row.
    ui.click(2);
    assert_eq!(ui.selected(), ["berry.png"]);
}

#[gpui_kit::test]
fn arrows_home_end_and_pages_move_the_selection_and_shift_extends(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.press("down");
    assert_eq!(ui.selected(), ["docs"], "from nothing, Down lands on the first");
    ui.press("down");
    ui.press("down");
    assert_eq!(ui.selected(), ["berry.png"]);
    ui.press("up");
    assert_eq!(ui.selected(), ["alpha.txt"]);
    ui.press("end");
    assert_eq!(ui.selected(), ["beta.md"]);
    ui.press("home");
    assert_eq!(ui.selected(), ["docs"]);
    ui.press("shift-down");
    ui.press("shift-down");
    assert_eq!(ui.selected(), ["docs", "alpha.txt", "berry.png"]);
    assert_eq!(ui.focused().as_deref(), Some("berry.png"));
    ui.press("pagedown");
    assert_eq!(ui.selected(), ["beta.md"], "a page past the end stops at the last");
}

#[gpui_kit::test]
fn letters_jump_to_the_next_name_that_starts_with_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.press("b");
    assert_eq!(ui.selected(), ["berry.png"]);
    // The same letter again cycles to the next.
    ui.press("b");
    assert_eq!(ui.selected(), ["beta.md"]);
    ui.press("b");
    assert_eq!(ui.selected(), ["berry.png"], "and wraps");
    ui.press("z");
    assert_eq!(ui.selected(), ["berry.png"], "nothing starts with z");
}

#[gpui_kit::test]
fn ctrl_space_toggles_the_focused_item_and_escape_clears(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(1);
    ui.press("ctrl-space");
    assert!(ui.selected().is_empty());
    ui.press("ctrl-space");
    assert_eq!(ui.selected(), ["alpha.txt"]);
    ui.press("escape");
    assert!(ui.selected().is_empty());
}

#[gpui_kit::test]
fn ctrl_a_selects_all_rows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.press("ctrl-a");
    assert_eq!(ui.selected().len(), 4);
}

#[gpui_kit::test]
fn enter_and_a_double_click_open_a_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(0);
    ui.press("enter");
    assert_eq!(ui.path(), "C:\\x\\docs");
    assert_eq!(ui.labels(), ["inner.txt"]);
    // Back, then a double-click on the same folder.
    let back = ui.explorer.update(&mut ui.visual, |e, cx| e.go_back(cx));
    ui.visual.foreground_executor().block_test(back);
    ui.frame();
    assert_eq!(ui.path(), "C:\\x");
    ui.visual.update(|window, cx| {
        window.render_frame(cx);
        window.double_click(("file-row", 0usize), cx);
    });
    ui.frame();
    assert_eq!(ui.path(), "C:\\x\\docs");
}

#[gpui_kit::test]
fn the_context_menu_is_asked_for_with_the_item_selected(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(1);
    ui.visual.update(|window, cx| {
        window.render_frame(cx);
        window.right_click(("file-row", 2usize), cx);
    });
    ui.frame();
    assert_eq!(
        ui.selected(),
        ["berry.png"],
        "a right-click selects what isn't selected"
    );
    let events = ui.events();
    assert!(
        matches!(events.as_slice(), [FileListEvent::ContextMenu { target: Some(path), at: Some(_), shift: false }] if path == "C:\\x\\berry.png"),
        "{events:?}"
    );
    // On the empty part of the list: the folder's menu, nothing selected.
    ui.visual.update(|window, cx| {
        window.right_click("file-list-area", cx);
    });
    ui.frame();
    assert!(ui.selected().is_empty());
    assert!(matches!(
        ui.events().last(),
        Some(FileListEvent::ContextMenu { target: None, .. })
    ));
}

// ---- Rename -------------------------------------------------------------------------------------------------

impl Ui {
    /// Makes the explorer say `path` is being renamed, as its rename part will.
    fn rename(&mut self, path: Option<&str>) {
        let path = path.map(str::to_string);
        self.list.update(&mut self.visual, |list, cx| {
            list.pretend_renaming = path;
            cx.notify();
        });
        // One frame makes the box, the next focuses it.
        self.frame();
        self.frame();
    }
}

#[gpui_kit::test]
fn the_row_being_renamed_shows_a_box_with_the_name_selected_and_enter_commits(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.rename(Some("C:\\x\\alpha.txt"));
    assert_eq!(ui.role_of("New name"), Some(Role::TextInput));
    // "alpha" is selected, ".txt" is not: typing replaces the first.
    ui.visual.update(|window, cx| window.input("gamma", cx));
    ui.press("enter");
    assert_eq!(
        ui.events(),
        [FileListEvent::CommitRename {
            path: "C:\\x\\alpha.txt".into(),
            name: "gamma.txt".into()
        }]
    );
    // The box says it once: leaving it after Enter doesn't commit again.
    ui.click(0);
    assert_eq!(ui.events().len(), 1);
}

#[gpui_kit::test]
fn escape_cancels_the_rename_and_leaving_the_box_commits(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.rename(Some("C:\\x\\beta.md"));
    ui.press("escape");
    assert_eq!(ui.events(), [FileListEvent::CancelRename]);

    // Another box: pressing on another row blurs it, which commits what is there (`onBlur`).
    ui.rename(None);
    ui.rename(Some("C:\\x\\berry.png"));
    ui.click(0);
    assert_eq!(
        ui.events().last(),
        Some(&FileListEvent::CommitRename {
            path: "C:\\x\\berry.png".into(),
            name: "berry.png".into()
        })
    );
}

#[gpui_kit::test]
fn a_character_no_name_can_hold_is_refused(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.rename(Some("C:\\x\\alpha.txt"));
    ui.press("?");
    ui.press("enter");
    // Nothing replaced the selected "alpha".
    assert_eq!(
        ui.events(),
        [FileListEvent::CommitRename {
            path: "C:\\x\\alpha.txt".into(),
            name: "alpha.txt".into()
        }]
    );
}

// ---- States -------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn an_empty_folder_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder("C:\\e", vec![]);
    let mut ui = open(cx, &disk, "C:\\e");
    assert!(ui.labels().is_empty());
    assert_eq!(ui.role_of("This folder is empty"), Some(Role::Status));
}

#[gpui_kit::test]
fn a_folder_of_only_hidden_items_offers_to_show_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder("C:\\h", vec![hidden("C:\\h", ".secret")]);
    let mut ui = open(cx, &disk, "C:\\h");
    assert_eq!(ui.role_of("This folder only has hidden items."), Some(Role::Status));
    ui.visual.update(|window, cx| window.click("file-list-show-hidden", cx));
    ui.frame();
    assert_eq!(ui.labels(), [".secret"]);
}

#[gpui_kit::test]
fn a_folder_windows_keeps_closed_says_why_and_offers_to_get_access(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.deny("C:\\locked");
    let mut ui = open(cx, &disk, "C:\\locked");
    assert_eq!(
        ui.role_of("You don't have permission to open “locked”"),
        Some(Role::Group)
    );
    assert_eq!(ui.role_of("Get access"), Some(Role::Button));
    assert_eq!(ui.role_of("Open in File Explorer"), Some(Role::Button));
    // Granting access opens it (the fake disk lets the folder open, and it is empty).
    disk.folder("C:\\locked", vec![file("C:\\locked", "inside.txt", 1, 0)]);
    ui.visual.update(|window, cx| window.click("file-list-get-access", cx));
    for _ in 0..20 {
        ui.frame();
        if !ui.labels().is_empty() {
            break;
        }
    }
    assert_eq!(ui.labels(), ["inside.txt"]);
}

#[gpui_kit::test]
fn a_folder_that_could_not_be_listed_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &FakeDisk::new(), "C:\\nope");
    assert_eq!(ui.role_of("Can't open this folder"), Some(Role::Alert));
}

#[gpui_kit::test]
fn a_filter_that_matches_nothing_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.explorer
        .update(&mut ui.visual, |e, cx| e.set_filter("zzz", None, cx));
    ui.frame();
    assert_eq!(ui.role_of("Nothing in this folder matches “zzz”."), Some(Role::Status));
    // Escape leaves the filter.
    ui.focus();
    ui.press("escape");
    assert_eq!(ui.labels().len(), 4);
}

// ---- Groups, marks, columns ---------------------------------------------------------------------------------

#[gpui_kit::test]
fn group_headers_sit_between_the_rows_and_the_rows_keep_their_places(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.visual
        .update(|_, cx| state::update(cx, |s| s.group_by = GroupBy::Kind));
    ui.frame();
    // The rows are the four items; the headers are named groups with their counts.
    assert_eq!(ui.labels().len(), 4);
    ui.visual.update(|window, _| {
        let headers = window.find_all_by_role(Role::Group);
        assert!(!headers.is_empty());
        let folders = window.find_by_label("Folders");
        assert_eq!(folders.description(), Some("1 item"));
    });
    // Arrow keys step over the headers: Down from the folder lands on the next item.
    ui.press("down");
    assert_eq!(ui.selected().len(), 1);
    let first = ui.selected();
    ui.press("down");
    assert_ne!(ui.selected(), first);
}

#[gpui_kit::test]
fn cloud_files_and_locked_archive_items_carry_their_marks(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    let with = |name: &str, attrs: u32| {
        with_details(
            RawEntry {
                name: name.into(),
                size: 10,
                attrs,
                ..Default::default()
            },
            "C:\\c",
        )
    };
    disk.folder(
        "C:\\c",
        vec![
            with("online.txt", attr::OFFLINE),
            with("kept.txt", attr::PINNED),
            with("secret.zip", attr::IN_ARCHIVE | attr::ENCRYPTED),
        ],
    );
    let mut ui = open(cx, &disk, "C:\\c");
    assert_eq!(ui.role_of("Available when online"), Some(Role::Image));
    assert_eq!(ui.role_of("Always available on this device"), Some(Role::Image));
    assert_eq!(ui.role_of("Password-protected"), Some(Role::Image));
}

#[gpui_kit::test]
fn dragging_a_column_edge_resizes_it_and_the_width_is_kept_in_the_settings(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    let (from, to, before) = ui.visual.update(|window, _| {
        let edge = window.find("file-list-edge-size").bounds().center();
        let size = window.find("file-list-sort-Size").bounds().size.width;
        (edge, point(edge.x - px(30.), edge.y), size)
    });
    assert_eq!(f32::from(before), 88., "5.5rem");
    ui.visual.update(|window, cx| window.drag(from, to, cx));
    ui.frame();
    let kept = ui.visual.update(|_, cx| state::settings(cx).columns.size);
    assert_eq!(kept, Some(118.));
    let after = ui
        .visual
        .update(|window, _| window.find("file-list-sort-Size").bounds().size.width);
    assert_eq!(f32::from(after), 118.);
    // A double-click on the edge gives the usual width back.
    ui.visual
        .update(|window, cx| window.double_click("file-list-edge-size", cx));
    ui.frame();
    assert_eq!(ui.visual.update(|_, cx| state::settings(cx).columns.size), None);
}

#[gpui_kit::test]
fn a_narrow_list_leaves_columns_out_as_the_web_did(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    // 1000 px is wide: date, type and size.
    assert!(ui
        .visual
        .update(|window, _| window.try_find("file-list-sort-Type").is_some()));
    ui.visual.simulate_resize(size(px(400.), px(700.)));
    ui.frame();
    ui.frame();
    // 400 px is 25rem: names and sizes only.
    ui.visual.update(|window, _| {
        assert!(window.try_find("file-list-sort-Type").is_none());
        assert!(window.try_find("file-list-sort-Modified").is_none());
        assert!(window.try_find("file-list-sort-Size").is_some());
    });
}

// ---- Scroll, size, direction --------------------------------------------------------------------------------

/// `C:\big` with this many files, named so they sort in order.
fn big_disk(n: usize) -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\big",
        (0..n)
            .map(|i| file("C:\\big", &format!("file{i:06}.txt"), i as u64, 0))
            .collect(),
    );
    disk.folder("C:\\big\\sub", vec![]);
    disk
}

#[gpui_kit::test]
fn a_hundred_thousand_rows_draw_only_the_ones_on_screen(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let started = Instant::now();
    let mut ui = open(cx, &big_disk(100_000), "C:\\big");
    let drawn = ui.labels().len();
    assert!(drawn > 10 && drawn < 80, "{drawn} rows drawn");
    // The last row is a key away, and scrolled to.
    ui.press("end");
    assert_eq!(ui.focused().as_deref(), Some("file099999.txt"));
    ui.frame();
    ui.visual.update(|window, _| {
        let last = window.find(("file-row", 99_999usize));
        assert_eq!(last.selected(), Some(true));
        assert!(last.visible());
    });
    assert!(ui.labels().len() < 80);
    eprintln!(
        "100k rows: opened, drawn and scrolled to the end in {:?}",
        started.elapsed()
    );
}

#[gpui_kit::test]
fn where_the_list_is_scrolled_is_kept_for_the_folder_and_comes_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = big_disk(500);
    let mut ui = open(cx, &disk, "C:\\big");
    ui.press("end");
    ui.frame();
    ui.frame();
    let (tab, place) = ui.explorer.read_with(&ui.visual, |e, _| {
        let tab = e.active_tab().map(|t| (t.id, format!("{}:list", t.view_key())));
        tab.unwrap_or((TabId(0), String::new()))
    });
    let kept = ui.explorer.read_with(&ui.visual, |e, _| e.saved_scroll(tab, &place));
    assert!(kept > 1000., "scrolled to the end, kept {kept}");
    // Into a subfolder and back: the list is where it was left.
    let go = ui.explorer.update(&mut ui.visual, |e, cx| {
        e.navigate_task("C:\\big\\sub", NavOptions::default(), cx)
    });
    ui.visual.foreground_executor().block_test(go);
    ui.frame();
    assert!(ui.labels().is_empty());
    let back = ui.explorer.update(&mut ui.visual, |e, cx| e.go_back(cx));
    ui.visual.foreground_executor().block_test(back);
    ui.frame();
    ui.frame();
    let offset = ui.list.read_with(&ui.visual, |l, _| {
        -f32::from(l.scroll.0.borrow().base_handle.offset().y)
    });
    assert!((offset - kept).abs() < 30., "restored {offset}, kept {kept}");
    assert!(ui.labels().iter().any(|l| l == "file000499.txt"));
}

#[gpui_kit::test]
fn the_arrow_keys_resting_on_a_folder_read_it_ahead(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    assert!(!disk.listed().iter().any(|p| p == "C:\\x\\docs"));
    ui.press("down");
    assert_eq!(ui.focused().as_deref(), Some("docs"));
    ui.visual.executor().advance_clock(Duration::from_millis(300));
    ui.frame();
    assert!(disk.listed().iter().any(|p| p == "C:\\x\\docs"), "{:?}", disk.listed());
}

#[gpui_kit::test]
fn more_than_ten_files_ask_before_they_open(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\m",
        (0..12).map(|i| file("C:\\m", &format!("f{i:02}.txt"), 1, 0)).collect(),
    );
    let mut ui = open(cx, &disk, "C:\\m");
    ui.press("ctrl-a");
    ui.press("enter");
    let events = ui.events();
    assert!(
        matches!(events.as_slice(), [FileListEvent::ConfirmOpenMany { paths }] if paths.len() == 12),
        "{events:?}"
    );
}

#[gpui_kit::test]
fn arabic_names_are_named_as_they_are_and_the_arabic_interface_mirrors(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\ar",
        vec![
            folder("C:\\ar", "مجلد"),
            file("C:\\ar", "تقرير الربع 1.pdf", 2048, 0),
            file("C:\\ar", "report.txt", 5, 0),
        ],
    );
    let mut ui = open(cx, &disk, "C:\\ar");
    // The name reaches a screen reader as the file has it, without the layout-only isolates.
    assert_eq!(ui.labels(), ["مجلد", "report.txt", "تقرير الربع 1.pdf"]);
    let left_to_right = ui.visual.update(|window, _| {
        let name = window.find("file-list-sort-Name").bounds().origin.x;
        let size = window.find("file-list-sort-Size").bounds().origin.x;
        name < size
    });
    assert!(left_to_right);

    testing::set_rtl(true);
    ui.list.update(&mut ui.visual, |_, cx| cx.notify());
    ui.frame();
    let mirrored = ui.visual.update(|window, _| {
        let name = window.find("file-list-sort-Name").bounds().origin.x;
        let size = window.find("file-list-sort-Size").bounds().origin.x;
        name > size
    });
    assert!(mirrored, "in Arabic the name is at the right and the size at the left");
    // Keys and presses work the same.
    ui.click(2);
    assert_eq!(ui.selected(), ["تقرير الربع 1.pdf"]);
    ui.press("up");
    assert_eq!(ui.selected(), ["report.txt"]);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn each_pane_has_its_own_list_and_a_click_in_one_makes_it_the_active_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    struct Two {
        left: Entity<FileList>,
        right: Entity<FileList>,
    }
    impl Render for Two {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            row()
                .size_full()
                .child(div().w(px(500.)).h_full().child(self.left.clone()))
                .child(div().w(px(500.)).h_full().child(self.right.clone()))
        }
    }
    let disk = small_disk();
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\x", NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    explorer.update(cx, |e, cx| e.toggle_dual(cx));
    let (mut visual, _) = testing::open_window(cx, |window, cx| {
        let left = cx.new(|cx| FileList::new(0, window, cx));
        let right = cx.new(|cx| FileList::new(1, window, cx));
        cx.new(|_| Two { left, right })
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        explorer.read_with(&visual, |e, _| e.active_pane()),
        1,
        "toggling dual makes the right pane active"
    );
    let rows = visual.update(|window, _| window.find_all_by_role(Role::ListBoxOption).len());
    assert_eq!(rows, 8);
    visual.update(|window, cx| {
        let both = window.find_all(("file-row", 1usize));
        assert_eq!(both.len(), 2);
        let on_the_left = both[0].bounds().center();
        window.pointer_down(on_the_left, MouseButton::Left, cx);
        window.pointer_up(on_the_left, MouseButton::Left, cx);
    });
    visual.run_until_parked();
    assert_eq!(explorer.read_with(&visual, |e, _| e.active_pane()), 0);
    let selected = explorer.read_with(&visual, |e, _| e.pane_tab(0).map(|t| t.selection.len()));
    assert_eq!(selected, Some(1));
    let other = explorer.read_with(&visual, |e, _| e.pane_tab(1).map(|t| t.selection.len()));
    assert_eq!(other, Some(0));
}

#[gpui_kit::test]
fn in_the_columns_view_left_goes_up_and_right_goes_into_the_focused_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    disk.folder("C:\\", vec![folder("C:\\", "x")]);
    let mut ui = open(cx, &disk, "C:\\x");
    ui.explorer
        .update(&mut ui.visual, |e, cx| e.set_view(ViewMode::Columns, None, cx));
    ui.frame();
    ui.press("down");
    assert_eq!(ui.focused().as_deref(), Some("docs"));
    ui.press("right");
    assert_eq!(ui.path(), "C:\\x\\docs");
    ui.press("left");
    assert_eq!(ui.path(), "C:\\x");
    // In the list view they do nothing.
    ui.explorer
        .update(&mut ui.visual, |e, cx| e.set_view(ViewMode::List, None, cx));
    ui.frame();
    ui.press("right");
    assert_eq!(ui.path(), "C:\\x");
}

impl Ui {
    /// Right-clicks the header and clicks the menu's item `n` (counting every item, headings included).
    fn header_menu(&mut self, n: usize) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.right_click("file-list-header", cx);
        });
        self.frame();
        self.visual.update(|window, cx| {
            window.within("popup-menu").click(n, cx);
        });
        self.frame();
    }
}

#[gpui_kit::test]
fn the_headers_menu_chooses_columns_group_by_and_what_to_sort_by(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    // The columns come first, a check by those shown. Date modified is item 1: taking it out asks for the
    // folder's columns without it.
    ui.header_menu(1);
    assert_eq!(
        ui.events().last(),
        Some(&FileListEvent::SetColumns {
            columns: vec![ColumnId::Type, ColumnId::Size]
        })
    );
    // Date taken (item 2) isn't shown: choosing it adds it, in its place in the order.
    ui.header_menu(2);
    assert_eq!(
        ui.events().last(),
        Some(&FileListEvent::SetColumns {
            columns: vec![ColumnId::Modified, ColumnId::Taken, ColumnId::Type, ColumnId::Size]
        })
    );
    // Group by Kind (item 11).
    ui.header_menu(11);
    let group = ui
        .explorer
        .read_with(&ui.visual, |e, _| e.active_tab().map(|t| t.group));
    assert_eq!(group, Some(GroupBy::Kind));
    // Sort by Type (item 20), which isn't a column of the descending kind.
    ui.header_menu(20);
    let sort = ui.explorer.read_with(&ui.visual, |e, _| e.active_tab().map(|t| t.sort));
    assert_eq!(sort.map(|s| (s.key, s.desc)), Some((SortKey::Type, false)));
}

#[test]
fn every_icon_the_list_draws_is_in_the_embedded_assets() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    // A missing icon draws nothing and says nothing (kit.rs lists the shared ones; these are the list's own).
    for name in [
        IconName::Cloud,
        IconName::CircleCheck,
        IconName::RefreshCw,
        IconName::KeyRound,
        IconName::FileX,
        IconName::ChevronUp,
        IconName::ChevronDown,
        IconName::FolderLock,
        IconName::FolderOpen,
        IconName::FolderSearch,
        IconName::Trash,
        IconName::Images,
        IconName::Clock,
        IconName::SearchX,
        IconName::ShieldCheck,
        IconName::Info,
        IconName::Search,
        IconName::TriangleAlert,
        IconName::RotateCcw,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

#[gpui_kit::test]
fn the_pointer_resting_on_a_folder_reads_it_ahead_and_leaving_stops_that(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    ui.visual.update(|window, cx| {
        window.render_frame(cx);
        window.hover(("file-row", 0usize), cx);
    });
    ui.visual.executor().advance_clock(Duration::from_millis(1000));
    ui.frame();
    assert!(disk.listed().iter().any(|p| p == "C:\\x\\docs"), "{:?}", disk.listed());
}

#[gpui_kit::test]
fn a_folder_being_read_shows_a_bar_over_the_one_on_screen_and_a_state_when_there_is_none(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    assert_eq!(ui.role_of("Loading…"), None);
    // Into a folder that takes a while: the old list stays, with the bar over it.
    let gate = disk.hold("C:\\x\\docs");
    ui.explorer.update(&mut ui.visual, |e, cx| {
        e.navigate("C:\\x\\docs", NavOptions::default(), cx);
    });
    ui.frame();
    assert_eq!(ui.role_of("Loading…"), Some(Role::ProgressIndicator));
    assert_eq!(ui.labels().len(), 4, "the folder being left is still there");
    gate.send_blocking(()).ok();
    ui.frame();
    ui.frame();
    assert_eq!(ui.role_of("Loading…"), None);
    assert_eq!(ui.labels(), ["inner.txt"]);

    // The very first folder, still being read: a state, and no bar.
    let slow = FakeDisk::new();
    slow.folder("C:\\slow", vec![file("C:\\slow", "a.txt", 1, 0)]);
    let gate = slow.hold("C:\\slow");
    cx.update(|cx| explorer::init_with(cx, slow.source()));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    explorer.update(cx, |e, cx| e.navigate("C:\\slow", NavOptions::default(), cx));
    let (mut visual, _) = testing::open_window(cx, |window, cx| cx.new(|cx| FileList::new(0, window, cx)));
    visual.run_until_parked();
    let role = visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label("Loading…").and_then(|e| e.role())
    });
    assert_eq!(role, Some(Role::Status));
    gate.send_blocking(()).ok();
    visual.run_until_parked();
}

#[gpui_kit::test]
fn the_time_to_show_a_folder_is_reported_to_the_explorer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.frame();
    ui.frame();
    let shown_in = ui
        .explorer
        .read_with(&ui.visual, |e, _| e.active_tab().map(|t| t.shown_in));
    assert!(shown_in.is_some_and(|ms| ms > 0.), "{shown_in:?}");
}

#[gpui_kit::test]
fn the_recycle_bin_says_where_each_item_was_deleted_from(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    let deleted = |name: &str, origin: &str| {
        with_details(
            RawEntry {
                name: name.into(),
                size: 5,
                modified: 1_000,
                attrs: attr::RECYCLED,
                origin: Some(origin.into()),
                ..Default::default()
            },
            "C:\\$Recycle.Bin",
        )
    };
    disk.folder(
        "shell:RecycleBinFolder",
        vec![
            deleted("old.txt", "C:\\Users\\me\\Docs"),
            deleted("older.txt", "D:\\Work"),
        ],
    );
    let mut ui = open(cx, &disk, "shell:RecycleBinFolder");
    assert_eq!(ui.role_of("Recycle Bin"), Some(Role::ListBox));
    assert_eq!(ui.role_of("Deleted from"), Some(Role::ColumnHeader));
    assert_eq!(ui.role_of("Date deleted"), Some(Role::Button));
    assert_eq!(ui.labels(), ["old.txt", "older.txt"]);
    // The name and the place share the free width.
    ui.visual.update(|window, _| {
        let name = window.find("file-list-sort-Name").bounds().size.width;
        let place = window.find("file-list-where").bounds().size.width;
        assert!(place < name && place > name * 0.8, "{place:?} {name:?}");
    });
}
