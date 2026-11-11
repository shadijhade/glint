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
mod diff_marks;

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
    open_with(cx, disk, path, None)
}

/// [`open`], the thumbnail service reading from `pictures` when given.
fn open_with(
    cx: &mut TestAppContext,
    disk: &FakeDisk,
    path: &str,
    pictures: Option<Rc<dyn thumbnail::ThumbSource>>,
) -> Ui {
    open_setting(cx, disk, path, pictures, true)
}

/// [`open_with`], Settings' "pictures in the list" on or off.
fn open_setting(
    cx: &mut TestAppContext,
    disk: &FakeDisk,
    path: &str,
    pictures: Option<Rc<dyn thumbnail::ThumbSource>>,
    list_thumbnails: bool,
) -> Ui {
    cx.update(testing::init);
    cx.update(|cx| state::update(cx, |settings| settings.list_thumbnails = list_thumbnails));
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    if let Some(pictures) = pictures {
        cx.update(|cx| {
            thumbnail::init_with(cx, pictures);
        });
    }
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
fn enter_in_the_rename_box_commits_and_does_not_open_the_item(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(0);
    // Quick Look is what the list's own Enter closes (`open_selected`): it shows whether that Enter ran.
    ui.explorer.update(&mut ui.visual, |e, _| e.quick_look = true);
    ui.rename(Some("C:\\x\\alpha.txt"));
    ui.press("enter");
    assert_eq!(ui.events().len(), 1, "{:?}", ui.events());
    assert!(matches!(ui.events()[0], FileListEvent::CommitRename { .. }));
    assert!(
        ui.explorer.read_with(&ui.visual, |e, _| e.quick_look),
        "the list's Open ran on the old name"
    );
    // The same for Ctrl+Enter and F2, which the list binds too.
    ui.press("ctrl-enter");
    ui.press("f2");
    assert!(ui.explorer.read_with(&ui.visual, |e, _| e.quick_look));
}

#[gpui_kit::test]
fn the_list_takes_the_keyboard_back_when_the_rename_box_closes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    for key in ["escape", "enter"] {
        let mut ui = open(cx, &small_disk(), "C:\\x");
        ui.rename(Some("C:\\x\\alpha.txt"));
        let list = ui.list.clone();
        let list_has_it = |ui: &mut Ui| ui.visual.update(|window, cx| list.read(cx).focus.is_focused(window));
        assert!(!list_has_it(&mut ui), "the box holds the keyboard while it is open");
        ui.press(key);
        // The explorer's rename part clears `Tab::renaming` once it has heard.
        ui.rename(None);
        ui.frame();
        assert!(list_has_it(&mut ui), "after {key} nothing holds the keyboard");
        // A key that is an action of the list reaches it again.
        ui.press("ctrl-a");
        assert!(!ui.selected().is_empty(), "Ctrl+A after {key}");
    }
}

#[gpui_kit::test]
fn a_rename_box_closed_by_going_elsewhere_leaves_the_focus_there(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    let other = ui.visual.update(|_, cx| cx.focus_handle());
    ui.rename(Some("C:\\x\\alpha.txt"));
    ui.visual.update(|window, cx| other.focus(window, cx));
    ui.frame();
    ui.rename(None);
    ui.frame();
    assert!(ui.visual.update(|window, _| other.is_focused(window)));
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

// ---- Follow-ups: the explorer's focus and marquee, cut items, dragging, drops, thumbnails, the keyboard's menu --

use crate::components::file_grid::thumbnail::{Ask, Fut, ThumbSource};
use crate::platform::dragdrop::{Answer, DropAnswers, DropEffect, DropEvent, Under};
use gpui_kit::MouseButton;

impl Ui {
    /// Where row `i` is in the window.
    fn row_at(&mut self, i: usize) -> gpui_kit::Bounds<Pixels> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find(("file-row", i)).bounds()
        })
    }

    /// The list's area under the header, in the window.
    fn area(&mut self) -> gpui_kit::Bounds<Pixels> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("file-list-area").bounds()
        })
    }

    fn down(&mut self, at: Point<Pixels>) {
        self.visual
            .update(|window, cx| window.pointer_down(at, MouseButton::Left, cx));
        self.frame();
    }

    fn drag_to(&mut self, at: Point<Pixels>) {
        self.visual
            .update(|window, cx| window.pointer_move(at, Some(MouseButton::Left), cx));
        self.frame();
    }

    fn up(&mut self, at: Point<Pixels>) {
        self.visual
            .update(|window, cx| window.pointer_up(at, MouseButton::Left, cx));
        self.frame();
    }

    fn anchor(&mut self) -> Option<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.active_tab().and_then(|t| {
                t.selection
                    .anchor()
                    .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
            })
        })
    }

    /// What the last frame was drawn from.
    fn frame_of(&mut self) -> Rc<Frame> {
        self.frame();
        self.list.read_with(&self.visual, |list, _| list.frame.clone())
    }

    fn lane(&mut self) -> String {
        self.explorer
            .read_with(&self.visual, |e, _| e.active_tab_id().to_string())
    }
}

#[gpui_kit::test]
fn ctrl_and_the_arrows_move_the_focus_without_touching_the_selection(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(1);
    ui.press("ctrl-down");
    ui.press("ctrl-down");
    assert_eq!(ui.selected(), ["alpha.txt"], "the selection stays");
    assert_eq!(ui.focused().as_deref(), Some("beta.md"), "the focus moved two rows");
    // Ctrl+Space then adds the focused item.
    ui.press("ctrl-space");
    assert_eq!(ui.selected(), ["alpha.txt", "beta.md"]);
    ui.press("ctrl-up");
    assert_eq!(ui.focused().as_deref(), Some("berry.png"));
    assert_eq!(ui.selected(), ["alpha.txt", "beta.md"]);
}

#[gpui_kit::test]
fn pressing_one_item_of_a_selection_moves_the_focus_and_anchor_and_narrows_on_release(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(1);
    ui.click_with(3, shift());
    assert_eq!(ui.selected(), ["alpha.txt", "berry.png", "beta.md"]);
    let at = ui.row_at(2).center();
    ui.down(at);
    assert_eq!(ui.selected(), ["alpha.txt", "berry.png", "beta.md"], "kept for a drag");
    assert_eq!(ui.focused().as_deref(), Some("berry.png"));
    assert_eq!(ui.anchor().as_deref(), Some("berry.png"));
    ui.up(at);
    assert_eq!(ui.selected(), ["berry.png"]);
}

#[gpui_kit::test]
fn a_drag_on_empty_space_selects_the_rows_it_crosses_and_ctrl_adds(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    let last = ui.row_at(3);
    let below = point(last.center().x, last.bottom() + px(40.));
    let second = ui.row_at(1).center();
    // Nothing shows until the pointer has moved.
    ui.down(below);
    ui.visual
        .update(|window, _| assert!(window.try_find("file-list-marquee").is_none()));
    ui.drag_to(second);
    // The rectangle is drawn from where it began to the pointer.
    ui.visual.update(|window, _| {
        let rect = window.find("file-list-marquee").bounds();
        assert_eq!(rect.origin.y, second.y);
        assert_eq!(rect.bottom(), below.y);
    });
    // From below the last row up to the second: alpha, berry, beta. The first covered is the anchor, the last
    // has the focus.
    assert_eq!(ui.selected(), ["alpha.txt", "berry.png", "beta.md"]);
    assert_eq!(ui.anchor().as_deref(), Some("alpha.txt"));
    assert_eq!(ui.focused().as_deref(), Some("beta.md"));
    // Dragged back down to the third, fewer are covered.
    let third = ui.row_at(2).center();
    ui.drag_to(third);
    assert_eq!(ui.selected(), ["berry.png", "beta.md"]);
    ui.up(third);
    ui.visual
        .update(|window, _| assert!(window.try_find("file-list-marquee").is_none()));
    assert_eq!(
        ui.selected(),
        ["berry.png", "beta.md"],
        "kept when the button is let go"
    );

    // With Ctrl, what was selected stays.
    ui.click(0);
    ui.visual.update(|window, cx| window.change_modifiers(ctrl(), cx));
    ui.down(below);
    ui.drag_to(third);
    ui.up(third);
    ui.visual
        .update(|window, cx| window.change_modifiers(Modifiers::default(), cx));
    assert_eq!(ui.selected(), ["docs", "berry.png", "beta.md"]);
    // A press on empty space that never moves just clears the selection.
    ui.down(below);
    ui.up(below);
    assert!(ui.selected().is_empty());
}

#[gpui_kit::test]
fn a_marquee_held_beyond_the_bottom_scrolls_the_list_and_covers_what_comes_into_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &big_disk(300), "C:\\big");
    let area = ui.area();
    // Started the way a press on the empty part would (a long list has none on screen).
    let from = point(area.center().x, area.origin.y + px(40.));
    ui.list
        .update(&mut ui.visual, |list, cx| list.begin_marquee(from, false, cx));
    let held = point(area.center().x, area.bottom() - px(4.));
    ui.drag_to(held);
    // (The last row covered has the focus, which keeps it in view: a few pixels.)
    let before = ui.list.read_with(&ui.visual, |list, _| list.scroll_top());
    assert!(before < 28., "{before}");
    let covered = ui.selected().len();
    for _ in 0..10 {
        let list = ui.list.clone();
        ui.visual.update(|window, cx| {
            list.update(cx, |list, cx| {
                assert!(list.scroll_tick(window, cx));
            })
        });
        ui.frame();
    }
    // Half a row (14 px) a tick, and more as the focus follows the last row covered.
    let after = ui.list.read_with(&ui.visual, |list, _| list.scroll_top());
    assert!(after >= before + 140., "{before} -> {after}");
    assert!(ui.selected().len() > covered, "more rows are covered after the scroll");
    ui.up(held);
    // The ticks stop once the marquee is over.
    let list = ui.list.clone();
    ui.visual.update(|window, cx| {
        list.update(cx, |list, cx| assert!(!list.scroll_tick(window, cx)));
    });
}

#[gpui_kit::test]
fn cut_items_are_dimmed_until_pasted(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    assert!(ui.frame_of().cut.is_empty());
    ui.explorer.update(&mut ui.visual, |e, cx| {
        e.copy_to_clipboard(vec!["C:\\x\\beta.md".into()], true, cx).detach()
    });
    let frame = ui.frame_of();
    assert!(frame.cut.contains("C:\\x\\beta.md"));
    assert_eq!(cells::dimness(true, false), Some(0.55));
    assert_eq!(cells::dimness(false, true), Some(0.45), "dragging is dimmer still");
    assert_eq!(cells::dimness(true, true), Some(0.45));
    assert_eq!(cells::dimness(false, false), None);
}

/// Pictures for the thumbnail service: every ask is answered with a 4 x 2 red picture.
#[derive(Default)]
struct Pictures {
    asked: RefCell<Vec<Ask>>,
}

impl ThumbSource for Rc<Pictures> {
    fn thumbnail(&self, ask: Ask) -> Fut<Result<Vec<u8>, String>> {
        self.asked.borrow_mut().push(ask);
        let pixels = glint_core::thumbs::Pixels {
            width: 4,
            height: 2,
            bgra: [0, 0, 255, 255].iter().copied().cycle().take(4 * 2 * 4).collect(),
        };
        let mut answer = vec![0, 1, 1, 0];
        answer.extend(1.5f32.to_le_bytes());
        answer.extend(glint_core::thumbs::encode_bmp(&pixels));
        Box::pin(async move { Ok(answer) })
    }

    fn cancel(&self, _: &str, _: Option<Vec<String>>, _: u32) {}
}

#[gpui_kit::test]
fn pictures_and_videos_get_their_thumbnail_in_the_icon_cell_and_nothing_else_does(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\x",
        vec![
            folder("C:\\x", "docs"),
            file("C:\\x", "alpha.txt", 100, 0),
            file("C:\\x", "berry.png", 20_000, 0),
            file("C:\\x", "clip.mp4", 9_000, 0),
        ],
    );
    let pictures = Rc::new(Pictures::default());
    let mut ui = open_with(cx, &disk, "C:\\x", Some(Rc::new(pictures.clone())));
    // The service is asked for the picture and the video, in the list's one-rem square.
    for _ in 0..200 {
        ui.frame();
        if pictures.asked.borrow().len() >= 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut asked: Vec<String> = pictures.asked.borrow().iter().map(|a| a.path.clone()).collect();
    asked.sort();
    assert_eq!(asked, ["C:\\x\\berry.png", "C:\\x\\clip.mp4"]);
    let lane = ui.lane();
    assert!(pictures.asked.borrow().iter().all(|a| a.lane == lane && a.size >= 16));
    // Once decoded, the cell draws the picture in the icon's place.
    let mut drawn = false;
    for _ in 0..400 {
        ui.visual.run_until_parked();
        let frame = ui.frame_of();
        let picture = ui.list.read_with(&ui.visual, |list, cx| {
            let entry = frame.rows.get(2).cloned();
            entry.and_then(|entry| list.look_of(&frame, &entry, cx))
        });
        if matches!(picture, Some(thumbnail::Look::Picture(_))) {
            drawn = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(drawn, "the picture arrived and the cell shows it");
    let text_has_none = ui.list.read_with(&ui.visual, |list, cx| {
        let frame = list.frame.clone();
        let text = frame.rows.get(1).cloned();
        text.and_then(|entry| list.look_of(&frame, &entry, cx)).is_none()
    });
    assert!(text_has_none, "a text file keeps its icon");
}

#[gpui_kit::test]
fn with_list_thumbnails_off_nothing_is_asked(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let pictures = Rc::new(Pictures::default());
    let mut ui = open_setting(cx, &small_disk(), "C:\\x", Some(Rc::new(pictures.clone())), false);
    ui.frame();
    assert!(pictures.asked.borrow().is_empty());
    assert!(!ui.frame_of().thumbnails);
}

// ---- Dragging out ---------------------------------------------------------------------------------------------

fn started() -> Vec<(Vec<String>, bool)> {
    pointer::STARTED.with(|started| std::mem::take(&mut *started.borrow_mut()))
}

#[gpui_kit::test]
fn a_press_on_the_selection_that_moves_six_pixels_drags_it_out_of_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    started();
    ui.click(1);
    ui.click_with(2, shift());
    let start = ui.row_at(1).center();
    ui.down(start);
    // Less than the threshold is not a drag.
    ui.drag_to(point(start.x + px(3.), start.y + px(3.)));
    assert!(started().is_empty());
    ui.drag_to(point(start.x + px(6.), start.y + px(2.)));
    assert_eq!(
        started(),
        [(
            vec!["C:\\x\\alpha.txt".to_string(), "C:\\x\\berry.png".to_string()],
            false
        )]
    );
    // Further moves don't start it again, and the release doesn't narrow the selection.
    ui.drag_to(point(start.x + px(60.), start.y));
    ui.up(start);
    assert!(started().is_empty());
    assert_eq!(ui.selected(), ["alpha.txt", "berry.png"]);
}

#[gpui_kit::test]
fn the_items_being_dragged_are_dimmed_only_while_the_drag_lasts(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    started();
    ui.click(1);
    let start = ui.row_at(1).center();
    ui.down(start);
    ui.drag_to(point(start.x + px(10.), start.y));
    assert_eq!(started().len(), 1);
    // The test's drag ends at once, as "copy": the dimming is over.
    assert!(ui.frame_of().dragging.is_empty());
    ui.list.update(&mut ui.visual, |list, _| {
        list.dragging = Arc::new(["C:\\x\\alpha.txt".to_string()].into_iter().collect());
    });
    assert!(ui.frame_of().dragging.contains("C:\\x\\alpha.txt"));
}

#[gpui_kit::test]
fn items_in_an_archive_are_extracted_first_and_those_in_the_recycle_bin_cannot_be_dragged(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    let mut inside = file("C:\\x\\pack.zip", "readme.txt", 10, 0);
    inside.attrs |= attr::IN_ARCHIVE;
    disk.folder("C:\\x\\pack.zip", vec![inside]);
    let mut ui = open(cx, &disk, "C:\\x\\pack.zip");
    started();
    let asked = Rc::new(RefCell::new(Vec::new()));
    let sink = asked.clone();
    ui.visual.update(|_, cx| {
        cx.subscribe(&ui.list, move |_, event: &DragFromArchive, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    ui.click(0);
    let start = ui.row_at(0).center();
    ui.down(start);
    ui.drag_to(point(start.x + px(20.), start.y));
    ui.up(start);
    assert_eq!(
        *asked.borrow(),
        [DragFromArchive {
            paths: vec!["C:\\x\\pack.zip\\readme.txt".to_string()]
        }]
    );
    assert!(started().is_empty(), "the archive part drags the extracted copies");

    let disk = FakeDisk::new();
    let mut deleted = file("C:\\$Recycle.Bin", "old.txt", 5, 1_000);
    deleted.attrs |= attr::RECYCLED;
    disk.folder("shell:RecycleBinFolder", vec![deleted]);
    let mut ui = open(cx, &disk, "shell:RecycleBinFolder");
    started();
    ui.click(0);
    let start = ui.row_at(0).center();
    ui.down(start);
    ui.drag_to(point(start.x + px(20.), start.y));
    ui.up(start);
    assert!(started().is_empty());
}

// ---- Drops ----------------------------------------------------------------------------------------------------

impl Ui {
    fn dispatch(&mut self, answers: &DropAnswers, event: DropEvent) {
        self.visual
            .update(|window, cx| crate::drops::dispatch(event, answers, window, cx));
        self.frame();
    }

    /// Files come over `at`, and what the list answered.
    fn over(&mut self, answers: &DropAnswers, paths: &[&str], at: Point<Pixels>, effect: DropEffect) -> Answer {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        // Each drag starts anew: GPUI keeps the files of the first one entered until the drag exits.
        self.dispatch(answers, DropEvent::Leave);
        self.dispatch(
            answers,
            DropEvent::Enter {
                paths,
                x: f32::from(at.x),
                y: f32::from(at.y),
                effect,
            },
        );
        self.dispatch(
            answers,
            DropEvent::Over {
                x: f32::from(at.x),
                y: f32::from(at.y),
                effect,
            },
        );
        answers.get()
    }
}

fn under(name: &str) -> Answer {
    Answer::Accept(Under {
        name: name.into(),
        copy: "Copy to %1".into(),
        moves: "Move to %1".into(),
        always_move: false,
    })
}

#[gpui_kit::test]
fn files_dragged_over_a_folder_row_go_into_it_and_elsewhere_into_the_folder_shown(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    let answers = DropAnswers::default();
    let outside = ["D:\\Work\\report.docx"];
    // Over the folder row: "Copy to docs", and the row is lit.
    let docs = ui.row_at(0).center();
    assert_eq!(ui.over(&answers, &outside, docs, DropEffect::Copy), under("docs"));
    assert_eq!(ui.frame_of().drop_row.as_deref(), Some("C:\\x\\docs"));
    // Over a file row, and the empty part: the folder shown.
    let file_row = ui.row_at(1).center();
    assert_eq!(ui.over(&answers, &outside, file_row, DropEffect::Copy), under("x"));
    assert_eq!(ui.frame_of().drop_row, None);
    let area = ui.area();
    let empty = point(area.center().x, area.bottom() - px(20.));
    assert_eq!(ui.over(&answers, &outside, empty, DropEffect::Copy), under("x"));
    // The header is part of the list too.
    let header = ui
        .visual
        .update(|window, _| window.find("file-list-header").bounds().center());
    assert_eq!(ui.over(&answers, &outside, header, DropEffect::Copy), under("x"));
    ui.dispatch(&answers, DropEvent::Leave);
    assert_eq!(ui.frame_of().drop_row, None, "nothing is lit once the drag has left");
}

#[gpui_kit::test]
fn a_drop_copies_or_moves_into_the_folder_under_the_pointer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    let answers = DropAnswers::default();
    let files = vec!["D:\\Work\\report.docx".to_string(), "D:\\Work\\ملف.txt".to_string()];
    let paths: Vec<&str> = files.iter().map(String::as_str).collect();
    let docs = ui.row_at(0).center();
    ui.over(&answers, &paths, docs, DropEffect::Copy);
    ui.dispatch(
        &answers,
        DropEvent::Drop {
            paths: files.clone(),
            x: f32::from(docs.x),
            y: f32::from(docs.y),
            effect: DropEffect::Copy,
        },
    );
    ui.frame();
    assert!(
        disk.ops()
            .iter()
            .any(|op| matches!(op, crate::explorer::FileOp::Copy { sources, dest }
            if *sources == files && dest == "C:\\x\\docs")),
        "{:?}",
        disk.ops()
    );
    // Shift moves; on a file row it is the folder shown.
    let beta = ui.row_at(2).center();
    ui.over(&answers, &paths, beta, DropEffect::Move);
    ui.dispatch(
        &answers,
        DropEvent::Drop {
            paths: files.clone(),
            x: f32::from(beta.x),
            y: f32::from(beta.y),
            effect: DropEffect::Move,
        },
    );
    ui.frame();
    assert!(
        disk.ops()
            .iter()
            .any(|op| matches!(op, crate::explorer::FileOp::Move { sources, dest }
            if *sources == files && dest == "C:\\x")),
        "{:?}",
        disk.ops()
    );
    // A drop that was refused where it landed never reaches the list.
    let before = disk.ops().len();
    ui.dispatch(
        &answers,
        DropEvent::Drop {
            paths: files.clone(),
            x: f32::from(beta.x),
            y: f32::from(beta.y),
            effect: DropEffect::None,
        },
    );
    assert_eq!(disk.ops().len(), before);
}

#[gpui_kit::test]
fn a_drop_is_refused_where_the_files_already_are_and_into_themselves(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    let answers = DropAnswers::default();
    let docs = ui.row_at(0).center();
    let file_row = ui.row_at(1).center();
    // A folder over itself.
    assert_eq!(
        ui.over(&answers, &["C:\\x\\docs"], docs, DropEffect::Copy),
        Answer::Refuse
    );
    assert_eq!(ui.frame_of().drop_row, None);
    // Files from this folder over this folder.
    assert_eq!(
        ui.over(&answers, &["C:\\x\\alpha.txt"], file_row, DropEffect::Copy),
        Answer::Refuse
    );
    // A folder into one of its own subfolders.
    assert_eq!(
        ui.over(&answers, &["C:\\x"], docs, DropEffect::Copy),
        Answer::Refuse,
        "a folder can't go into one of its own"
    );
    // But a file of this folder into the folder row is fine.
    assert_eq!(
        ui.over(&answers, &["C:\\x\\alpha.txt"], docs, DropEffect::Copy),
        under("docs")
    );
}

#[gpui_kit::test]
fn nothing_is_dropped_into_a_closed_folder_and_a_closed_row_means_the_folder_shown(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let answers = DropAnswers::default();
    let outside = ["D:\\Work\\report.docx"];

    // A folder Windows keeps closed.
    let disk = FakeDisk::new();
    disk.folder("C:\\x", vec![]);
    disk.deny("C:\\x");
    let mut ui = open(cx, &disk, "C:\\x");
    let area = ui.area();
    assert_eq!(
        ui.over(&answers, &outside, area.center(), DropEffect::Copy),
        Answer::Refuse
    );

    // A closed folder's row isn't a place for files; it falls through to the folder shown, as the web's did.
    let disk = FakeDisk::new();
    let mut closed = folder("C:\\x", "locked");
    closed.attrs |= attr::CLOSED;
    disk.folder("C:\\x", vec![closed]);
    let mut ui = open(cx, &disk, "C:\\x");
    let row = ui.row_at(0).center();
    assert_eq!(ui.over(&answers, &outside, row, DropEffect::Copy), under("x"));
}

#[gpui_kit::test]
fn the_recycle_bin_takes_files_to_recycle_them_and_the_words_are_arabic_in_the_arabic_interface(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    let mut deleted = file("C:\\$Recycle.Bin", "old.txt", 5, 1_000);
    deleted.attrs |= attr::RECYCLED;
    disk.folder("shell:RecycleBinFolder", vec![deleted]);
    let mut ui = open(cx, &disk, "shell:RecycleBinFolder");
    let answers = DropAnswers::default();
    let area = ui.area();
    let at = point(area.center().x, area.bottom() - px(20.));
    let got = ui.over(&answers, &["D:\\Work\\report.docx"], at, DropEffect::Copy);
    let Answer::Accept(into) = got else {
        panic!("the Recycle Bin takes files: {got:?}")
    };
    assert!(into.always_move, "a drop there always deletes");

    // The Arabic interface: the words, and a folder with an Arabic name.
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\ar",
        vec![folder("C:\\ar", "مجلد"), file("C:\\ar", "report.txt", 5, 0)],
    );
    let mut ui = open(cx, &disk, "C:\\ar");
    testing::set_rtl(true);
    ui.list.update(&mut ui.visual, |_, cx| cx.notify());
    ui.frame();
    let row = ui.row_at(0).center();
    let got = ui.over(&answers, &["D:\\Work\\ملف.txt"], row, DropEffect::Copy);
    testing::set_rtl(false);
    let Answer::Accept(into) = got else {
        panic!("a folder row takes files: {got:?}")
    };
    assert_eq!(into.name, "مجلد");
    assert_eq!(into.copy, "نسخ إلى %1");
    assert_eq!(into.moves, "نقل إلى %1");
}

// ---- The keyboard's menu --------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_menu_key_asks_for_the_menu_at_the_focused_rows_bottom_corner(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    ui.click(2);
    ui.press("shift-f10");
    let row = ui.row_at(2);
    let area = ui.area();
    let events = ui.events();
    let Some(FileListEvent::ContextMenu { target, at, shift }) = events.last() else {
        panic!("{events:?}")
    };
    assert_eq!(target.as_deref(), Some("C:\\x\\berry.png"));
    assert!(!shift);
    // 1.5 rem in from the start edge, on the row's bottom edge.
    assert_eq!(*at, Some(point(area.origin.x + px(24.), row.bottom())));

    // In the Arabic interface the corner is the other one.
    testing::set_rtl(true);
    ui.list.update(&mut ui.visual, |_, cx| cx.notify());
    ui.frame();
    ui.press("shift-f10");
    let events = ui.events();
    testing::set_rtl(false);
    let Some(FileListEvent::ContextMenu { at: Some(at), .. }) = events.last() else {
        panic!("{events:?}")
    };
    assert_eq!(at.x, area.right() - px(24.));
    assert_eq!(at.y, row.bottom());

    // With nothing focused it is the folder's menu, near the list's top.
    ui.press("escape");
    ui.press("shift-f10");
    let events = ui.events();
    let Some(FileListEvent::ContextMenu {
        target: None,
        at: Some(at),
        ..
    }) = events.last()
    else {
        panic!("{events:?}")
    };
    assert_eq!(at.y, area.origin.y + px(8.));
}

#[gpui_kit::test]
fn the_menu_key_follows_the_focused_row_when_the_list_is_scrolled(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &big_disk(300), "C:\\big");
    ui.click(0);
    ui.press("end");
    ui.press("menu");
    ui.frame();
    let area = ui.area();
    let events = ui.events();
    let Some(FileListEvent::ContextMenu { at: Some(at), .. }) = events.last() else {
        panic!("{events:?}")
    };
    // The last row sits at the bottom of the list.
    assert!(at.y > area.origin.y && at.y <= area.bottom(), "{at:?} in {area:?}");
    assert!(f32::from(area.bottom() - at.y) < 2., "{at:?} in {area:?}");
}

// ---- Git marks, folder sizes ----------------------------------------------------------------------------------

use super::fake_git::{repo_view, FakeGit};
use glint_model::entries::GitMark;

/// `C:\x` of a repository: a folder, files (one named in Arabic) and one `.gitignore` leaves out.
fn repo_disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\x",
        vec![
            folder("C:\\x", "docs"),
            file("C:\\x", "alpha.txt", 100, 0),
            file("C:\\x", "beta.md", 3000, 0),
            file("C:\\x", "berry.png", 20_000, 0),
            file("C:\\x", "target.log", 5, 0),
            file("C:\\x", "ملف.txt", 7, 0),
        ],
    );
    disk.folder("C:\\x\\docs", vec![file("C:\\x\\docs", "inner.txt", 1, 0)]);
    disk.folder("C:\\plain", vec![file("C:\\plain", "notes.txt", 5, 0)]);
    disk
}

/// What Git says of `repo_disk`'s folder.
fn repo_marks() -> Vec<(&'static str, GitMark)> {
    vec![
        ("docs", GitMark::Conflicted),
        ("alpha.txt", GitMark::Modified),
        ("beta.md", GitMark::Untracked),
        ("berry.png", GitMark::Staged),
        ("target.log", GitMark::Ignored),
        ("ملف.txt", GitMark::Modified),
    ]
}

/// [`open`] on a fake Git that says `C:\x` is in a repository with `marks`.
fn open_repo(cx: &mut TestAppContext, disk: &FakeDisk, marks: &[(&str, GitMark)]) -> Ui {
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let git = FakeGit::default();
    git.put("C:\\x", repo_view(marks));
    cx.update(|cx| git.install(cx));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\x", NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let (mut visual, list) = testing::open_window(cx, |window, cx| cx.new(|cx| FileList::new(0, window, cx)));
    let events = Rc::new(RefCell::new(Vec::new()));
    visual.update(|window, _| window.activate_window());
    let mut ui = Ui {
        visual,
        list,
        explorer,
        events,
    };
    ui.focus();
    ui.settle();
    ui
}

impl Ui {
    /// Frames until the background work (Git's answer, the sizes) has landed.
    fn settle(&mut self) {
        for _ in 0..8 {
            self.frame();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// What the row `i` says besides its name.
    fn said(&mut self, i: usize) -> Option<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find(("file-row", i))
                .and_then(|row| row.description().map(str::to_string))
        })
    }
}

#[gpui_kit::test]
fn a_row_shows_a_pill_after_its_name_for_each_git_mark(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open_repo(cx, &repo_disk(), &repo_marks());
    assert_eq!(ui.role_of("Conflict: docs"), Some(Role::Group));
    assert_eq!(ui.role_of("Changed: alpha.txt"), Some(Role::Group));
    assert_eq!(ui.role_of("New: beta.md"), Some(Role::Group));
    assert_eq!(ui.role_of("Staged: berry.png"), Some(Role::Group));
    // A name in Arabic is told apart as well.
    assert_eq!(ui.role_of("Changed: ملف.txt"), Some(Role::Group));
    // The ignored one has no pill, and the pill says what it says of the item on hover.
    ui.visual.update(|window, _| {
        assert!(window.try_find_by_label("Ignored: target.log").is_none());
        let pill = window.find_by_label("Changed: alpha.txt");
        assert_eq!(pill.description(), Some("Git: changed since it was committed"));
        let folder = window.find_by_label("Conflict: docs");
        assert_eq!(folder.description(), Some("Git: a merge left conflicts in here"));
    });
    // The rows are still the same rows.
    assert_eq!(
        ui.labels(),
        ["docs", "alpha.txt", "berry.png", "beta.md", "target.log", "ملف.txt"]
    );
}

#[gpui_kit::test]
fn the_pills_are_in_the_mirrored_interface_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(true);
    let mut ui = open_repo(cx, &repo_disk(), &repo_marks());
    assert_eq!(ui.role_of("Staged: berry.png"), Some(Role::Group));
    assert_eq!(ui.role_of("Changed: ملف.txt"), Some(Role::Group));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_folder_outside_the_repository_has_no_pills(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open_repo(cx, &repo_disk(), &repo_marks());
    assert!(ui.role_of("Changed: alpha.txt").is_some());
    let explorer = ui.explorer.clone();
    ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.navigate("C:\\plain".to_string(), NavOptions::default(), cx)
        })
    });
    ui.settle();
    assert_eq!(ui.labels(), ["notes.txt"]);
    let pills = ui.visual.update(|window, _| window.find_all_by_role(Role::Group).len());
    assert_eq!(pills, 0);
}

#[gpui_kit::test]
fn the_git_filter_and_group_by_git_follow_the_marks_the_window_hands_over(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open_repo(cx, &repo_disk(), &repo_marks());
    let explorer = ui.explorer.clone();
    let tab = explorer.read_with(&ui.visual, |e, _| e.active_tab_id());
    // The window tells the explorer what Git said (it does so on `GitEvent::Changed`).
    let marks = ui.visual.update(|_, cx| crate::components::git::marks(cx, tab));
    assert!(marks.is_some(), "Git has the folder's marks");
    ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.set_git_marks(tab, marks.clone(), cx);
            e.set_filter("git:modified", None, cx);
        })
    });
    ui.frame();
    assert_eq!(ui.labels(), ["alpha.txt", "ملف.txt"]);
    ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.set_filter("", None, cx);
            e.set_group(GroupBy::Git, cx);
        })
    });
    ui.frame();
    // The rows are grouped under the marks, in this order; the first group's header is drawn.
    let headers: Vec<String> = explorer.read_with(&ui.visual, |e, _| {
        e.tab(tab)
            .and_then(|t| t.rows.groups())
            .map(|g| g.headers.iter().map(|h| h.label.to_string()).collect())
            .unwrap_or_default()
    });
    assert_eq!(headers, ["Conflicts", "Changed", "Staged", "New", "Ignored"]);
    assert_eq!(
        ui.labels(),
        ["docs", "alpha.txt", "ملف.txt", "berry.png", "beta.md", "target.log"]
    );
    assert!(ui.role_of("Conflicts").is_some());
}

/// A folder whose subfolder is measured: `C:\x\docs` holds `inner.txt` (1 byte).
#[gpui_kit::test]
fn a_folder_row_shows_its_measured_size_and_an_ellipsis_while_it_is_measured(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = small_disk();
    let mut ui = open(cx, &disk, "C:\\x");
    // Nothing has measured the folder yet: no size, no ellipsis.
    assert_eq!(ui.said(0).as_deref(), Some("File folder"));
    let gate = disk.hold_sizes();
    let explorer = ui.explorer.clone();
    ui.visual.update(|_, cx| explorer.update(cx, |e, cx| e.calc_sizes(cx)));
    ui.frame();
    assert_eq!(ui.said(0).as_deref(), Some("File folder, …"), "while it is measured");
    // Files keep their sizes meanwhile.
    assert_eq!(ui.said(1).as_deref(), Some("TXT file, 100 B"));
    drop(gate);
    ui.settle();
    assert_eq!(ui.said(0).as_deref(), Some("File folder, 1 B"));
}

mod facts;
mod find_states;
