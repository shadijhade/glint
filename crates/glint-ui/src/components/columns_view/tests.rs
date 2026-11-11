//! Headless tests of the columns view: what the two side columns list and name, what a click and the keys do to
//! the explorer, hidden items, a stale listing, a folder that cannot be read, and the Arabic layout. They run on a
//! fake disk behind the explorer's `Source`, with Arabic folder and file names.

use super::*;
use crate::explorer::fake::{file, folder, hidden, FakeDisk};
use crate::explorer::Explorer;
use crate::testing;
use glint_model::store::StartFolder;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

const ME: &str = "C:\\Users\\me";
const PROJECTS: &str = "C:\\Users\\me\\Projects";
const REPORTS: &str = "C:\\Users\\me\\Projects\\التقارير";
const SRC: &str = "C:\\Users\\me\\Projects\\src";
const PICTURES: &str = r"C:\Users\me\الصور";
const ARCHIVE: &str = r"C:\Users\me\Projects\التقارير\أرشيف";
const NOTES: &str = "C:\\Users\\me\\Projects\\notes.txt";

#[test]
fn the_folder_above_is_none_at_this_pc_and_at_a_root() {
    assert_eq!(parent_of(THIS_PC), None);
    assert_eq!(parent_of("C:\\"), None);
    assert_eq!(parent_of("C:\\Users"), Some("C:\\".to_string()));
    assert_eq!(parent_of(PROJECTS), Some(ME.to_string()));
    assert_eq!(parent_of(REPORTS), Some(PROJECTS.to_string()));
}

/// A stand-in for the pane's file list.
struct Middle;

impl Render for Middle {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().id("middle").test_support().size_full()
    }
}

struct Rig {
    visual: VisualTestContext,
    view: Entity<ColumnsView>,
    explorer: Entity<Explorer>,
    disk: FakeDisk,
}

fn settle(visual: &mut VisualTestContext) {
    for _ in 0..30 {
        visual.run_until_parked();
    }
}

/// The explorer on a fake disk, opened on `PROJECTS`; the columns view in pane 0 with a stand-in list between.
fn setup(cx: &mut TestAppContext, rtl: bool) -> Rig {
    cx.update(testing::init);
    cx.update(|cx| state::update(cx, |settings| settings.start_folder = StartFolder::ThisPc));
    testing::set_rtl(rtl);
    let disk = FakeDisk::new();
    disk.folder("C:\\", vec![folder("C:\\", "Users")]);
    disk.folder(
        ME,
        vec![
            folder(ME, "Projects"),
            folder(ME, "Downloads"),
            folder(ME, "الصور"),
            file(ME, "ملاحظات.txt", 2_048, 1),
        ],
    );
    disk.folder(
        PROJECTS,
        vec![
            folder(PROJECTS, "التقارير"),
            folder(PROJECTS, "src"),
            file(PROJECTS, "notes.txt", 1_536, 1),
            hidden(PROJECTS, ".secret"),
        ],
    );
    disk.folder(
        REPORTS,
        vec![
            file(REPORTS, "تقرير الربع الأول.pdf", 100, 1),
            file(REPORTS, "ملخص.txt", 10, 1),
            folder(REPORTS, "أرشيف"),
        ],
    );
    disk.folder(SRC, vec![]);
    disk.folder(PICTURES, vec![]);
    disk.folder(ARCHIVE, vec![]);
    cx.update(|cx| crate::explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let source = disk.source();
    let (mut visual, view) = testing::open_window(cx, |window, cx| {
        let columns = cx.new(|cx| ColumnsView::with_source(0, source.clone(), window, cx));
        let middle = cx.new(|_| Middle);
        columns.update(cx, |columns, cx| columns.set_center(middle.into(), cx));
        columns
    });
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx).detach()));
    settle(&mut visual);
    // The pane shows its columns: in any other view they read nothing.
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.set_view(crate::explorer::ViewMode::Columns, None, cx)));
    settle(&mut visual);
    let mut rig = Rig {
        visual,
        view,
        explorer,
        disk,
    };
    rig.navigate(PROJECTS);
    rig
}

impl Rig {
    fn navigate(&mut self, target: &str) {
        self.visual.update(|_, cx| {
            self.explorer
                .update(cx, |e, cx| e.navigate(target, NavOptions::default(), cx))
        });
        settle(&mut self.visual);
    }

    fn focus_item(&mut self, path: &str) {
        self.visual
            .update(|_, cx| self.explorer.update(cx, |e, cx| e.select_only(Some(path), cx)));
        settle(&mut self.visual);
    }

    fn path(&mut self) -> String {
        self.explorer.read_with(&self.visual, |e, _| {
            e.pane_tab(0).map(|t| t.path.clone()).unwrap_or_default()
        })
    }

    fn names(&self, which: Which) -> Vec<String> {
        self.view.read_with(&self.visual, |v, _| {
            let side = v.side(which);
            (0..side.shown.len())
                .filter_map(|i| side.entry(i).map(|e| e.name.clone()))
                .collect()
        })
    }

    fn index_of(&self, which: Which, name: &str) -> usize {
        self.names(which)
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("{name} is not in the {which:?} column: {:?}", self.names(which)))
    }

    fn frame(&mut self) {
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    fn role(&mut self, label: &str) -> Option<Role> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|e| e.role())
        })
    }

    fn click_row(&mut self, which: Which, name: &str) {
        let index = self.index_of(which, name);
        let id = match which {
            Which::Parent => ("columns-parent-row", index),
            Which::Next => ("columns-next-row", index),
        };
        self.frame();
        self.visual.update(|window, cx| window.click(id, cx));
        settle(&mut self.visual);
    }

    fn focus_column(&mut self, which: Which) {
        self.frame();
        let focus = self.view.read_with(&self.visual, |v, _| v.side(which).focus.clone());
        self.visual.update(|window, cx| focus.focus(window, cx));
        self.frame();
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        settle(&mut self.visual);
    }

    fn cursor(&self, which: Which) -> usize {
        self.view.read_with(&self.visual, |v, _| v.side(which).cursor)
    }
}

#[gpui_kit::test]
fn the_folder_above_lists_its_items_and_marks_the_open_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    // Folders first, as the folder lists, hidden items left out.
    let names = rig.names(Which::Parent);
    assert_eq!(names.len(), 4, "{names:?}");
    assert!(names.contains(&"الصور".to_string()));
    assert_eq!(rig.role("The folder above: me"), Some(Role::Group));
    assert_eq!(rig.role("Items in the folder above"), Some(Role::ListBox));
    assert_eq!(rig.role("الصور, in the folder above"), Some(Role::ListBoxOption));
    assert_eq!(rig.role("Projects, in the folder above"), Some(Role::ListBoxOption));
    // The open folder is the selected option, and the keyboard starts on it.
    let projects = rig.index_of(Which::Parent, "Projects");
    rig.frame();
    let selected: Vec<bool> = rig.visual.update(|window, _| {
        ["Projects", "Downloads", "الصور"]
            .iter()
            .map(|name| {
                window
                    .try_find_by_label(&format!("{name}, in the folder above"))
                    .and_then(|e| e.selected())
                    .unwrap_or(false)
            })
            .collect()
    });
    assert_eq!(selected, [true, false, false]);
    assert_eq!(rig.cursor(Which::Parent), projects);
}

#[gpui_kit::test]
fn there_is_no_folder_above_at_a_root_or_this_pc(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.navigate("C:\\");
    assert_eq!(
        rig.view
            .read_with(&rig.visual, |v, _| v.parent_path().map(str::to_string)),
        None
    );
    assert_eq!(
        rig.role("Items in the folder above"),
        None,
        "the column is not drawn at all"
    );
    rig.navigate(THIS_PC);
    assert_eq!(rig.role("Items in the folder above"), None);
}

#[gpui_kit::test]
fn the_last_column_says_what_to_do_until_something_is_focused(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    assert_eq!(rig.role("What's in the focused folder"), Some(Role::Group));
    assert_eq!(rig.role("← and → move between the columns."), Some(Role::Status));
    assert_eq!(
        rig.view
            .read_with(&rig.visual, |v, _| v.next_path().map(str::to_string)),
        None
    );
}

#[gpui_kit::test]
fn a_focused_folder_lists_its_items_with_a_count_and_arabic_names(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.focus_item(REPORTS);
    assert_eq!(
        rig.view
            .read_with(&rig.visual, |v, _| v.next_path().map(str::to_string)),
        Some(REPORTS.to_string())
    );
    assert_eq!(rig.names(Which::Next).len(), 3);
    assert_eq!(
        rig.role("تقرير الربع الأول.pdf, in the focused folder"),
        Some(Role::ListBoxOption)
    );
    assert_eq!(rig.role("أرشيف, in the focused folder"), Some(Role::ListBoxOption));
    assert_eq!(rig.role("Items in the focused folder"), Some(Role::ListBox));
    // The title carries the count.
    assert_eq!(rig.role("التقارير (3)"), Some(Role::Heading));
}

#[gpui_kit::test]
fn an_empty_folder_says_so_and_so_does_one_that_cannot_be_read(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.focus_item(SRC);
    assert_eq!(rig.role("It's empty"), Some(Role::Status));
    rig.disk.deny(REPORTS);
    rig.focus_item(REPORTS);
    assert!(
        rig.names(Which::Next).is_empty(),
        "a failed listing leaves the column empty"
    );
    assert_eq!(rig.role("It's empty"), Some(Role::Status));
}

#[gpui_kit::test]
fn a_focused_file_shows_its_name_and_size_not_a_listing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.focus_item(NOTES);
    assert_eq!(
        rig.view
            .read_with(&rig.visual, |v, _| v.next_path().map(str::to_string)),
        None
    );
    assert_eq!(rig.role("notes.txt, details"), Some(Role::Group));
    assert_eq!(rig.role("Items in the focused folder"), None);
}

#[gpui_kit::test]
fn a_click_on_a_folder_in_the_folder_above_opens_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.click_row(Which::Parent, "الصور");
    assert_eq!(rig.path(), "C:\\Users\\me\\الصور");
}

#[gpui_kit::test]
fn a_click_on_a_file_in_the_folder_above_opens_that_folder_with_it_selected(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.click_row(Which::Parent, "ملاحظات.txt");
    assert_eq!(rig.path(), ME);
    let selected = rig.explorer.read_with(&rig.visual, |e, _| {
        e.pane_tab(0).map(|t| t.selection.contains("C:\\Users\\me\\ملاحظات.txt"))
    });
    assert_eq!(selected, Some(true));
}

#[gpui_kit::test]
fn a_click_in_the_last_column_opens_a_folder_or_selects_a_file_in_the_focused_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.focus_item(REPORTS);
    rig.click_row(Which::Next, "ملخص.txt");
    assert_eq!(rig.path(), REPORTS, "the focused folder opens, the file selected");
    let selected = rig.explorer.read_with(&rig.visual, |e, _| {
        e.pane_tab(0)
            .map(|t| t.selection.contains("C:\\Users\\me\\Projects\\التقارير\\ملخص.txt"))
    });
    assert_eq!(selected, Some(true));
}

#[gpui_kit::test]
fn hidden_items_show_only_when_the_setting_says_so_without_listing_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    // From "me", focusing Projects lists it in the last column.
    rig.navigate(ME);
    rig.focus_item(PROJECTS);
    assert!(!rig.names(Which::Next).contains(&".secret".to_string()));
    let listed = rig.disk.listed().len();
    rig.visual
        .update(|_, cx| state::update(cx, |settings| settings.show_hidden = true));
    settle(&mut rig.visual);
    assert!(
        rig.names(Which::Next).contains(&".secret".to_string()),
        "{:?}",
        rig.names(Which::Next)
    );
    assert_eq!(rig.disk.listed().len(), listed, "filtered again, not listed again");
    rig.visual
        .update(|_, cx| state::update(cx, |settings| settings.show_hidden = false));
    settle(&mut rig.visual);
    assert!(!rig.names(Which::Next).contains(&".secret".to_string()));
}

#[gpui_kit::test]
fn a_listing_that_arrives_late_for_another_folder_is_dropped(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    let gate = rig.disk.hold(REPORTS);
    rig.focus_item(REPORTS);
    assert!(rig.names(Which::Next).is_empty(), "still being read");
    // The focus moves on to the other folder before the first listing is in.
    rig.focus_item(SRC);
    gate.send_blocking(()).ok();
    settle(&mut rig.visual);
    assert_eq!(
        rig.view
            .read_with(&rig.visual, |v, _| v.next_path().map(str::to_string)),
        Some(SRC.to_string())
    );
    assert!(
        rig.names(Which::Next).is_empty(),
        "the late listing of the report folder is not shown"
    );
}

#[gpui_kit::test]
fn the_keys_move_the_cursor_and_enter_opens_the_row(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.focus_item(REPORTS);
    rig.focus_column(Which::Next);
    assert_eq!(rig.cursor(Which::Next), 0);
    rig.press("down");
    assert_eq!(rig.cursor(Which::Next), 1);
    rig.press("end");
    assert_eq!(rig.cursor(Which::Next), 2);
    rig.press("down");
    assert_eq!(rig.cursor(Which::Next), 2, "stops at the last row");
    rig.press("home");
    assert_eq!(rig.cursor(Which::Next), 0);
    // The first row is a folder (folders first): Enter opens it.
    let first = rig.names(Which::Next)[0].clone();
    rig.press("enter");
    assert_eq!(rig.path(), format!("{REPORTS}\\{first}"));
}

#[gpui_kit::test]
fn left_goes_to_the_folder_above_and_right_goes_into_a_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    rig.focus_column(Which::Parent);
    rig.press("left");
    assert_eq!(rig.path(), ME);
    // Now the folder above is the Users folder; its cursor is on "me". Right goes in.
    rig.focus_column(Which::Parent);
    rig.press("right");
    assert_eq!(
        rig.path(),
        ME,
        "the cursor is on the open folder, a folder: it opens (again)"
    );
}

fn check_layout(cx: &mut TestAppContext, rtl: bool) {
    let mut rig = setup(cx, rtl);
    rig.frame();
    let (parent, middle, next) = rig.visual.update(|window, _| {
        (
            window.try_find("columns-parent").map(|e| e.bounds()),
            window.try_find("middle").map(|e| e.bounds()),
            window.try_find("columns-next").map(|e| e.bounds()),
        )
    });
    let (parent, middle, next) = (
        parent.expect("the folder above"),
        middle.expect("the list"),
        next.expect("the last column"),
    );
    if rtl {
        assert!(
            parent.left() >= middle.right() - px(1.),
            "the folder above is at the right: {parent:?} {middle:?}"
        );
        assert!(next.right() <= middle.left() + px(1.), "the last column is at the left");
    } else {
        assert!(
            parent.right() <= middle.left() + px(1.),
            "the folder above is at the left"
        );
        assert!(
            next.left() >= middle.right() - px(1.),
            "the last column is at the right"
        );
    }
    let rem = rig.visual.update(|window, _| f32::from(window.rem_size()));
    assert!((f32::from(parent.size.width) - PARENT_WIDTH * rem).abs() < 1.5, "w-56");
    assert!((f32::from(next.size.width) - NEXT_WIDTH * rem).abs() < 1.5, "w-64");
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_columns_sit_around_the_list(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    check_layout(cx, false);
}

#[gpui_kit::test]
fn in_arabic_the_columns_mirror_around_the_list(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    check_layout(cx, true);
}

#[gpui_kit::test]
fn a_long_folder_is_virtualized(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    let big: Vec<FileEntry> = (0..5_000)
        .map(|i| file(SRC, &format!("ملف {i:05}.txt"), i, 1))
        .collect();
    rig.disk.folder(SRC, big);
    rig.disk.touch(SRC);
    rig.focus_item(PROJECTS);
    rig.focus_item(SRC);
    assert_eq!(rig.names(Which::Next).len(), 5_000);
    rig.frame();
    let drawn = rig.visual.update(|window, _| {
        window
            .find_all_by_role(Role::ListBoxOption)
            .into_iter()
            .filter(|e| e.label().is_some_and(|l| l.ends_with(", in the focused folder")))
            .count()
    });
    assert!(
        drawn > 0 && drawn < 100,
        "only the rows on screen are drawn, not {drawn}"
    );
}

#[test]
fn the_icons_the_columns_draw_are_embedded() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    for name in [
        IconName::Folder,
        IconName::File,
        IconName::ChevronLeft,
        IconName::ChevronRight,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

#[gpui_kit::test]
fn a_pane_in_another_view_reads_nothing_for_its_columns_and_keeps_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = setup(cx, false);
    // Shown: the folder above is listed in the first column.
    let above = rig
        .visual
        .update(|_, cx| rig.view.read(cx).parent_path().map(str::to_string));
    assert_eq!(above.as_deref(), Some(ME));
    // The pane goes to the list: the columns let go of what they listed.
    let explorer = rig.explorer.clone();
    rig.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_view(crate::explorer::ViewMode::List, None, cx)));
    settle(&mut rig.visual);
    let (above, next) = rig.visual.update(|_, cx| {
        let view = rig.view.read(cx);
        (
            view.parent_path().map(str::to_string),
            view.next_path().map(str::to_string),
        )
    });
    assert_eq!((above, next), (None, None));
    // Folders opened and items focused meanwhile are read once, by the list alone.
    let before = rig.disk.listed().len();
    rig.navigate(REPORTS);
    assert_eq!(rig.disk.listed().len(), before + 1, "{:?}", rig.disk.listed());
    // Back in the columns view they are read again.
    rig.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_view(crate::explorer::ViewMode::Columns, None, cx)));
    settle(&mut rig.visual);
    let above = rig
        .visual
        .update(|_, cx| rig.view.read(cx).parent_path().map(str::to_string));
    assert_eq!(above.as_deref(), Some(PROJECTS));
}
