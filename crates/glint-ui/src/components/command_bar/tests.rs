//! Headless tests of the command bar: the buttons' states for a folder, a selection, an archive and the Recycle
//! Bin; what a click does; folding at narrow widths; the menus; and the Arabic interface. A fake disk stands
//! behind the explorer and a fake source behind the project tasks.

use super::*;
use crate::components::menus::{Entry, Task};
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{NavOptions, TabId};
use crate::testing;
use glint_core::projects::{Task as ProjectTask, TaskGroup};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};

/// The window's content: the bar in a box of a width the test sets, listening for every action the bar
/// dispatches (the workspace's handlers, here a log) and for the bar's own events.
struct Host {
    bar: Entity<CommandBar>,
    width: Pixels,
    actions: Rc<RefCell<Vec<String>>>,
    events: Rc<RefCell<Vec<CommandBarEvent>>>,
    _subscription: Subscription,
}

macro_rules! logged {
    ($element:expr, $cx:expr, $($action:ident),* $(,)?) => {
        $element$(.on_action($cx.listener(|this: &mut Host, _: &actions::$action, _, _| {
            this.actions.borrow_mut().push(stringify!($action).to_string())
        })))*
    };
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        logged!(
            div().w(self.width),
            cx,
            NewFolder,
            Cut,
            Copy,
            Paste,
            Rename,
            Delete,
            DeletePermanently,
            BatchRename,
            DiskUsage,
            CommandPalette,
            ToggleHidden,
            TogglePreview,
            DualPane,
            ComparePanes,
            ToggleTerminal,
            CompareFiles,
        )
        .child(self.bar.clone())
    }
}

/// A window with the bar over an explorer on a fake disk.
struct Rig {
    cx: VisualTestContext,
    host: Entity<Host>,
    bar: Entity<CommandBar>,
    disk: FakeDisk,
}

fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\Work",
        vec![
            folder("C:\\Work", "src"),
            file("C:\\Work", "notes.txt", 120, 0),
            file("C:\\Work", "todo.txt", 80, 0),
            file("C:\\Work", "تقرير.txt", 5, 0),
        ],
    );
    disk.folder(
        "C:\\Other",
        vec![
            file("C:\\Other", "notes.txt", 7, 0),
            file("C:\\Other", "other.txt", 9, 0),
        ],
    );
    disk.archive_folder(
        "C:\\Work\\pack.zip",
        "C:\\Work\\pack.zip",
        vec![file("C:\\Work\\pack.zip", "inner.txt", 4, 0)],
    );
    disk.archive_folder(
        "C:\\Work\\pack.zip\\docs",
        "C:\\Work\\pack.zip",
        vec![file("C:\\Work\\pack.zip\\docs", "deep.md", 4, 0)],
    );
    disk
}

fn open(cx: &mut TestAppContext, rtl: bool, width: f32) -> Rig {
    open_with(cx, rtl, width, Sources::empty())
}

fn open_with(cx: &mut TestAppContext, rtl: bool, width: f32, sources: Sources) -> Rig {
    cx.update(testing::init);
    let disk = disk();
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    testing::set_rtl(rtl);
    let (visual, host) = testing::open_window(cx, |window, cx| {
        let bar = cx.new(|cx| CommandBar::with_sources(window, cx, sources));
        cx.new(|cx| {
            let events: Rc<RefCell<Vec<CommandBarEvent>>> = Rc::default();
            let sink = events.clone();
            let subscription = cx.subscribe(&bar, move |_, _, event: &CommandBarEvent, _| {
                sink.borrow_mut().push(event.clone())
            });
            Host {
                bar,
                width: px(width),
                actions: Rc::default(),
                events,
                _subscription: subscription,
            }
        })
    });
    let bar = host.read_with(&visual, |host, _| host.bar.clone());
    let mut rig = Rig {
        cx: visual,
        host,
        bar,
        disk,
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
                (
                    bar.fold(),
                    m.bar.get() as i32,
                    m.left.get() as i32,
                    m.right.get() as i32,
                )
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

    fn fold(&self) -> usize {
        self.bar.read_with(&self.cx, |bar, _| bar.fold())
    }

    fn width(&mut self, width: f32) {
        self.host.update(&mut self.cx, |host, cx| {
            host.width = px(width);
            cx.notify();
        });
        self.settle();
    }

    /// Runs `f` on the explorer and lets what it started finish.
    fn act(&mut self, f: impl FnOnce(&mut Explorer, &mut Context<Explorer>)) {
        let explorer = self.cx.update(|_, cx| explorer::explorer(cx));
        explorer.update(&mut self.cx, f);
        self.cx.run_until_parked();
        self.settle();
    }

    fn go(&mut self, path: &str) {
        let path = path.to_string();
        self.act(move |e, cx| e.navigate(path, NavOptions::default(), cx));
    }

    fn select(&mut self, paths: &[&str]) {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        self.act(move |e, cx| e.set_selection(paths, None, None, cx));
    }

    /// The element with this accessible name: whether it's disabled, its bounds and whether it's pressed.
    fn find(&mut self, label: &str) -> Option<(bool, Bounds<Pixels>, Option<bool>)> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find_by_label(label)
                .map(|e| (e.disabled().unwrap_or(false), e.bounds(), e.checked()))
        })
    }

    fn present(&mut self, label: &str) -> bool {
        self.find(label).is_some()
    }

    fn enabled(&mut self, label: &str) -> bool {
        match self.find(label) {
            Some((disabled, _, _)) => !disabled,
            None => panic!("no control named {label:?}"),
        }
    }

    fn click(&mut self, id: &'static str) {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.cx.run_until_parked();
        self.settle();
    }

    fn press(&mut self, key: &str) {
        self.cx.update(|window, cx| window.press(key, cx));
        self.cx.run_until_parked();
    }

    fn log(&self) -> Vec<String> {
        self.host.read_with(&self.cx, |host, _| host.actions.borrow().clone())
    }

    fn events(&self) -> Vec<CommandBarEvent> {
        self.host.read_with(&self.cx, |host, _| host.events.borrow().clone())
    }

    fn clear(&self) {
        self.host.read_with(&self.cx, |host, _| {
            host.actions.borrow_mut().clear();
            host.events.borrow_mut().clear();
        });
    }

    fn facts(&self) -> Facts {
        self.bar.read_with(&self.cx, |bar, _| bar.facts().clone())
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
}

/// The labels of a menu's rows, headings as "# text", lines as "-".
fn labels(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .map(|node| match node {
            Node::Item(e) => e.label.to_string(),
            Node::Menu { label, .. } => format!("{label}>"),
            Node::Heading(h) => format!("# {h}"),
            Node::Separator => "-".into(),
            Node::Strip(entries) => format!("[{}]", entries.len()),
        })
        .collect()
}

fn entry<'a>(nodes: &'a [Node], label: &str) -> &'a Entry {
    nodes
        .iter()
        .find_map(|node| match node {
            Node::Item(e) if e.label == label => Some(e),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no row {label:?} in {:?}", labels(nodes)))
}

// ---- The buttons' states ---------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_folder_with_nothing_selected_offers_new_and_paste_but_not_cut_copy_rename_or_delete(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1400.);
    rig.go("C:\\Work");
    assert_eq!(rig.fold(), 0, "everything fits at 1400 px");
    let f = rig.facts();
    assert!(f.in_folder && !f.read_only && !f.bin && !f.find && f.selected == 0);

    assert!(rig.enabled("New folder"));
    assert!(rig.enabled("Paste (Ctrl+V)"));
    // What acts on a selection isn't in the island until there is one: nothing greyed out stands in it.
    let on_a_selection = [
        "Cut (Ctrl+X)",
        "Copy (Ctrl+C)",
        "Rename (F2) · Ctrl+Shift+R renames many",
        "Delete (Del) · Shift+Del deletes permanently",
    ];
    for absent in on_a_selection {
        assert!(!rig.present(absent), "{absent} needs a selection");
    }
    rig.select(&["C:\\Work\\notes.txt"]);
    for there in on_a_selection {
        assert!(rig.enabled(there), "{there} arrives with the selection");
    }
    rig.select(&[]);
    assert!(!rig.present("Cut (Ctrl+X)"), "and leaves with it");
    // Always on: the palette, the toggles, the views, the map, folder sizes, text search, the tools.
    for on in [
        "Command palette (Ctrl+K)",
        "Show hidden items (Ctrl+H)",
        "Preview pane (Alt+P)",
        "Dual pane: two folders side by side (Ctrl+Shift+D)",
        "Compare panes: highlight differences between both panes (Ctrl+Shift+M)",
        "List (Ctrl+Shift+6)",
        "Grid, with thumbnails (Ctrl+Shift+2)",
        "Disk usage",
        "Folder sizes",
        "Tools",
        "Run",
    ] {
        if on == "Run" {
            // No project tasks here.
            assert!(!rig.present(on));
        } else {
            assert!(rig.enabled(on), "{on}");
        }
    }
    // A dual pane's buttons and the Recycle Bin's aren't there.
    for absent in [
        "Copy to the other pane",
        "Move to the other pane",
        "Restore",
        "Extract all",
    ] {
        assert!(!rig.present(absent), "{absent}");
    }
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_selection_turns_on_what_acts_on_it_and_names_how_many_rename_takes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    rig.select(&["C:\\Work\\notes.txt"]);
    assert_eq!(rig.facts().selected, 1);
    for on in [
        "Cut (Ctrl+X)",
        "Copy (Ctrl+C)",
        "Rename (F2) · Ctrl+Shift+R renames many",
        "Delete (Del) · Shift+Del deletes permanently",
    ] {
        assert!(rig.enabled(on), "{on}");
    }
    rig.select(&["C:\\Work\\notes.txt", "C:\\Work\\todo.txt"]);
    assert!(rig.enabled("Rename 2 items (F2)"), "{:?}", rig.buttons());
    assert!(!rig.present("Rename (F2) · Ctrl+Shift+R renames many"));
    // Two files in one folder can be compared.
    assert!(rig.facts().pair.is_some());
    assert!(
        rig.enabled("Compare “notes.txt” and “todo.txt” (Ctrl+M)"),
        "{:?}",
        rig.buttons()
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn an_archive_is_read_only_copies_out_and_can_be_extracted_whole_at_its_top(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work\\pack.zip");
    let f = rig.facts();
    assert!(f.read_only && !f.closed && !f.bin);
    assert_eq!(f.whole_archive.as_deref(), Some("C:\\Work\\pack.zip"));
    assert!(rig.present("Read-only archive"));
    assert!(rig.present("Extract all"));
    // The tools are for folders you may read and change: where they would be is the note.
    assert!(!rig.present("Tools"));
    assert!(!rig.enabled("New folder"));
    assert!(!rig.enabled("Paste (Ctrl+V)"));
    rig.select(&["C:\\Work\\pack.zip\\inner.txt"]);
    // Copy works from an archive (extracted), and says so; cut, rename and delete don't.
    assert!(rig.enabled("Copy (Ctrl+C): extracted, to paste anywhere"));
    assert!(!rig.enabled("Cut (Ctrl+X)"));
    assert!(!rig.enabled("Rename (F2) · Ctrl+Shift+R renames many"));
    assert!(!rig.enabled("Delete (Del) · Shift+Del deletes permanently"));

    // A folder inside the archive can't be extracted whole.
    rig.go("C:\\Work\\pack.zip\\docs");
    assert!(rig.facts().whole_archive.is_none());
    assert!(rig.present("Read-only archive") && !rig.present("Extract all"));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn extract_all_extracts_the_archive_beside_itself(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work\\pack.zip");
    rig.click("extract-all");
    for _ in 0..8 {
        rig.cx.run_until_parked();
    }
    assert_eq!(
        rig.disk.extractions().first().map(|(archive, _)| archive.clone()),
        Some("C:\\Work\\pack.zip".to_string())
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_recycle_bin_has_restore_delete_for_good_and_empty_instead_of_the_edits(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    rig.select(&["C:\\Work\\notes.txt"]);
    let explorer = rig.cx.update(|_, cx| explorer::explorer(cx));
    explorer.update(&mut rig.cx, |e, cx| {
        e.delete_paths(vec!["C:\\Work\\notes.txt".into()], false, None, cx)
            .detach()
    });
    rig.cx.run_until_parked();
    rig.go(glint_model::paths::RECYCLE_BIN);
    assert!(rig.facts().bin && rig.facts().read_only);
    assert!(!rig.present("New folder") && !rig.present("Cut (Ctrl+X)") && !rig.present("Tools"));
    assert!(rig.present("Restore") && rig.present("Empty the Recycle Bin"));
    // Nothing chosen: nothing to restore or delete.
    assert!(!rig.enabled("Restore"));
    assert!(!rig.enabled("Delete for good (Del)"));

    let in_bin = explorer.read_with(&rig.cx, |e, _| e.active_tab().map(|t| t.entries()[0].path.clone()));
    let in_bin = in_bin.expect("the deleted file is in the bin");
    rig.select(&[&in_bin]);
    assert!(rig.enabled("Restore") && rig.enabled("Delete for good (Del)"));

    // Restore puts it back where it was; Empty asks the window, Delete for good is the Delete action.
    rig.click("restore");
    for _ in 0..8 {
        rig.cx.run_until_parked();
    }
    assert!(rig.disk.names("C:\\Work").contains(&"notes.txt".to_string()));
    rig.clear();
    rig.click("empty-bin");
    assert_eq!(rig.events(), [CommandBarEvent::Menu(MenuEvent::EmptyRecycleBin)]);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_search_keeps_the_edits_off_what_cant_be_made_and_this_pc_has_no_folder_tools(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    // This PC is not a folder: nothing is made or pasted there, no map, no folder sizes.
    assert!(!rig.facts().in_folder);
    assert!(!rig.enabled("New folder") && !rig.enabled("Paste (Ctrl+V)"));
    assert!(!rig.enabled("Disk usage") && !rig.enabled("Folder sizes"));
    // The palette and the toggles still work.
    assert!(rig.enabled("Command palette (Ctrl+K)") && rig.enabled("Show hidden items (Ctrl+H)"));

    rig.go("C:\\Work");
    rig.act(|e, cx| {
        e.run_find("notes", explorer::Wider::default(), cx).detach();
    });
    for _ in 0..8 {
        rig.cx.run_until_parked();
    }
    let f = rig.facts();
    assert!(f.find, "a search is showing");
    assert!(!rig.enabled("New folder") && !rig.enabled("Paste (Ctrl+V)"));
    assert!(!rig.enabled("Disk usage") && !rig.enabled("Folder sizes"));
    testing::set_rtl(false);
}

// ---- What a click does -----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_edit_buttons_run_the_workspaces_actions(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    rig.select(&["C:\\Work\\notes.txt"]);
    for id in ["new-folder", "cut", "copy", "paste", "rename", "delete"] {
        rig.click(id);
    }
    assert_eq!(rig.log(), ["NewFolder", "Cut", "Copy", "Paste", "Rename", "Delete"]);
    rig.clear();
    for id in ["disk-usage", "palette", "hidden", "preview", "dual"] {
        rig.click(id);
    }
    assert_eq!(
        rig.log(),
        [
            "DiskUsage",
            "CommandPalette",
            "ToggleHidden",
            "TogglePreview",
            "DualPane"
        ]
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_disabled_button_does_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    // This PC is no folder: New folder and Paste are there, greyed out, and pressing them does nothing.
    assert!(!rig.enabled("New folder") && !rig.enabled("Paste (Ctrl+V)"));
    rig.click("new-folder");
    rig.click("paste");
    assert!(rig.log().is_empty());
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_view_buttons_choose_the_tabs_view_and_the_pressed_one_follows_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    let view = |rig: &Rig| rig.facts().view;
    assert_eq!(view(&rig), ViewMode::List);
    let pressed = |rig: &mut Rig, label: &str| rig.find(label).and_then(|(_, _, on)| on);
    assert_eq!(pressed(&mut rig, "List (Ctrl+Shift+6)"), Some(true));
    assert_eq!(pressed(&mut rig, "Grid, with thumbnails (Ctrl+Shift+2)"), Some(false));
    rig.click("view-grid");
    assert_eq!(view(&rig), ViewMode::Grid);
    assert_eq!(pressed(&mut rig, "Grid, with thumbnails (Ctrl+Shift+2)"), Some(true));
    rig.click("view-columns");
    assert_eq!(view(&rig), ViewMode::Columns);
    rig.click("view-list");
    assert_eq!(view(&rig), ViewMode::List);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_disk_usage_map_unpresses_both_views_and_a_view_button_takes_it_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    let tab = rig.cx.update(|_, cx| explorer::explorer(cx).read(cx).active_tab_id());
    rig.cx.update(|_, cx| disk_usage::set(cx, tab, true));
    rig.settle();
    assert!(rig.facts().mapped);
    let pressed = |rig: &mut Rig, label: &str| rig.find(label).and_then(|(_, _, on)| on);
    assert_eq!(pressed(&mut rig, "Disk usage"), Some(true));
    assert_eq!(
        pressed(&mut rig, "List (Ctrl+Shift+6)"),
        Some(false),
        "neither view is pressed"
    );
    rig.click("view-list");
    assert!(!rig.facts().mapped, "back to the files");
    assert_eq!(pressed(&mut rig, "Disk usage"), Some(false));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn in_two_panes_items_go_to_the_other_pane_and_compare_panes_flips(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    rig.act(|e, cx| e.toggle_dual(cx));
    rig.go("C:\\Other");
    rig.act(|e, cx| e.switch_pane(cx));
    let f = rig.facts();
    assert!(f.dual && f.other, "the other pane shows a folder");
    assert!(
        !rig.present("Copy to the other pane"),
        "nothing is selected: nothing to send"
    );
    rig.select(&["C:\\Work\\todo.txt"]);
    assert!(rig.enabled("Copy to the other pane") && rig.enabled("Move to the other pane"));
    // The same name in the other folder is its namesake: one file here compares with it.
    rig.select(&["C:\\Work\\notes.txt"]);
    assert!(
        rig.enabled("Compare “notes.txt” and “notes.txt” (Ctrl+M)"),
        "{:?}",
        rig.buttons()
    );
    assert_eq!(
        rig.facts().pair.map(|p| (p.left, p.right)),
        Some(("C:\\Work\\notes.txt".to_string(), "C:\\Other\\notes.txt".to_string()))
    );
    rig.clear();
    rig.click("compare");
    assert!(matches!(
        rig.events().as_slice(),
        [CommandBarEvent::Menu(MenuEvent::Compare { left, right })]
            if left == "C:\\Work\\notes.txt" && right == "C:\\Other\\notes.txt"
    ));

    // Copy to the other pane copies through the explorer.
    rig.click("copy-other");
    for _ in 0..8 {
        rig.cx.run_until_parked();
    }
    assert!(rig.disk.names("C:\\Other").contains(&"notes.txt".to_string()), "copied");

    // Compare panes is pressed once flipped.
    let pressed = |rig: &mut Rig| {
        rig.find("Compare panes: highlight differences between both panes (Ctrl+Shift+M)")
            .and_then(|(_, _, on)| on)
    };
    assert_eq!(pressed(&mut rig), Some(false));
    rig.click("compare-panes");
    assert_eq!(pressed(&mut rig), Some(true));
    // One pane again: nothing to compare.
    rig.act(|e, cx| e.toggle_dual(cx));
    assert_eq!(pressed(&mut rig), Some(false));
    assert!(!rig.present("Copy to the other pane"));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn folder_sizes_say_stop_while_they_run_and_ask_the_window_either_way(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    rig.click("folder-sizes");
    assert_eq!(rig.events(), [CommandBarEvent::FolderSizes { stop: false }]);
    rig.bar
        .update(&mut rig.cx, |bar, cx| bar.set_folder_sizes_running(true, cx));
    rig.settle();
    assert!(rig.present("Stop") && !rig.present("Folder sizes"));
    rig.clear();
    rig.click("folder-sizes");
    assert_eq!(rig.events(), [CommandBarEvent::FolderSizes { stop: true }]);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_tile_size_menu_sets_the_size_and_shows_the_grid(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    rig.click("tile-sizes");
    assert!(rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    let nodes = rig
        .bar
        .read_with(&rig.cx, |bar, cx| bar.menu_view().map(|v| v.read(cx).nodes().to_vec()))
        .expect("the menu is open");
    assert_eq!(
        labels(&nodes),
        [
            "Larger tiles",
            "Smaller tiles",
            "-",
            "Small",
            "Medium",
            "Large",
            "Extra large",
            "Huge"
        ]
    );
    // Medium is the one chosen; Ctrl+0 resets to it.
    assert_eq!(entry(&nodes, "Medium").check, Some(true));
    assert_eq!(entry(&nodes, "Medium").hint.as_deref(), Some("Ctrl+0"));
    assert_eq!(entry(&nodes, "Larger tiles").hint.as_deref(), Some("Ctrl+="));
    // Choose "Larger tiles" (the first row): the size grows and the grid shows.
    rig.press("enter");
    assert!(!rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    rig.settle();
    let f = rig.facts();
    assert_eq!(f.tile_size, TileSize::Large);
    assert_eq!(f.view, ViewMode::Grid);
    testing::set_rtl(false);
}

// ---- Menus ------------------------------------------------------------------------------------------------------

#[test]
fn the_new_menu_lists_a_folder_a_text_document_and_the_kinds_windows_makes() {
    let _guard = testing::lock();
    let kinds = vec![
        explorer::NewKind {
            ext: ".docx".into(),
            name: "Microsoft Word Document".into(),
            icon: None,
            by_app: true,
        },
        explorer::NewKind {
            ext: ".zip".into(),
            name: "Compressed (zipped) Folder".into(),
            icon: None,
            by_app: false,
        },
    ];
    let nodes = menus::new_menu(&kinds);
    assert_eq!(
        labels(&nodes),
        [
            "Folder",
            "Text document",
            "-",
            "Microsoft Word Document",
            "Compressed (zipped) Folder"
        ]
    );
    assert_eq!(entry(&nodes, "Folder").hint.as_deref(), Some("Ctrl+Shift+N"));
    assert_eq!(
        entry(&nodes, "Microsoft Word Document").run,
        Run::Ask(MenuEvent::NewOfKind { ext: ".docx".into() })
    );
    assert_eq!(labels(&menus::new_menu(&[])), ["Folder", "Text document"]);
}

fn folder_facts() -> Facts {
    let mut f = Facts::none();
    f.tab = Some(TabId(1));
    f.path = "C:\\Work".into();
    f.in_folder = true;
    f
}

#[test]
fn the_tools_menu_grows_as_the_bar_folds_and_is_greyed_for_what_cant_be_done() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let f = folder_facts();
    let at = |fold: usize| labels(&menus::tools_menu(&f, &[], fold, false));
    let plain = at(0);
    assert_eq!(
        plain,
        [
            "Terminal here, under the files",
            "Open in Terminal",
            "# Inspect",
            "Compare two files",
            "File X-Ray: hashes and hidden streams",
            "What's locking this folder?",
            "Recent changes in this folder",
            "Moved and deleted: where did a file go?",
            "# Clean up",
            "Find duplicate files in this folder",
            "Clean up this folder…",
            "Convert the selected pictures…",
            "Folder rules: sort out what comes in…",
            "# Share",
            "Send this folder to another PC…",
            "Share this folder on this network…",
            "Receive from another PC here…",
            "# Check",
            "Scan this folder with Microsoft Defender",
            "Look for passwords and keys left in files",
            "Check with YARA rules",
        ]
    );
    // At 4 the map and folder sizes come in; at 5 the palette too.
    assert_eq!(&at(4)[..3], ["Disk usage", "Folder sizes", "-"]);
    assert_eq!(&at(5)[..2], ["Command palette", "-"]);
    assert_eq!(at(3), plain);

    let nodes = menus::tools_menu(&f, &[], 0, false);
    assert!(entry(&nodes, "Compare two files").disabled, "no pair");
    assert!(
        entry(&nodes, "File X-Ray: hashes and hidden streams").disabled,
        "no file"
    );
    assert!(entry(&nodes, "Convert the selected pictures…").disabled, "no pictures");
    assert!(!entry(&nodes, "Find duplicate files in this folder").disabled);

    // A running measure says Stop; the map says how to get back.
    let mut mapped = f.clone();
    mapped.mapped = true;
    let nodes = menus::tools_menu(&mapped, &[], 4, true);
    assert_eq!(
        &labels(&nodes)[..2],
        ["Back to the file list", "Stop calculating folder sizes"]
    );
}

#[test]
fn the_tools_menu_speaks_of_the_selection_and_offers_its_tools_on_one_file() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let f = folder_facts();
    let one = file("C:\\Work", "photo.jpg", 5, 0);
    let nodes = menus::tools_menu(&f, std::slice::from_ref(&one), 0, false);
    let x_ray = entry(&nodes, "File X-Ray: hashes and hidden streams");
    assert!(!x_ray.disabled);
    assert_eq!(x_ray.run, Run::Ask(MenuEvent::XRay { path: one.path.clone() }));
    assert_eq!(
        entry(&nodes, "What's locking the selection?").run,
        Run::Ask(MenuEvent::Locks {
            paths: vec![one.path.clone()]
        })
    );
    assert!(
        !entry(&nodes, "Convert the selected pictures…").disabled,
        "a jpg converts"
    );
    assert_eq!(
        entry(&nodes, "Send the selection to another PC…").run,
        Run::Ask(MenuEvent::SendToPc { path: one.path.clone() })
    );
    assert_eq!(
        entry(&nodes, "Scan the selection with Microsoft Defender").run,
        Run::Ask(MenuEvent::Defender {
            paths: vec![one.path.clone()]
        })
    );
    // Two items: sending and one-path checks are for one thing.
    let two = [one.clone(), file("C:\\Work", "b.txt", 1, 0)];
    let nodes = menus::tools_menu(&f, &two, 0, false);
    assert!(entry(&nodes, "Share this folder on this network…").disabled);
    assert!(entry(&nodes, "Send this folder to another PC…").disabled);
    assert!(!entry(&nodes, "Scan the selection with Microsoft Defender").disabled);
}

#[test]
fn the_tools_are_greyed_in_a_read_only_place_and_a_remote_has_no_clean_up() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut f = folder_facts();
    f.read_only = true;
    let nodes = menus::tools_menu(&f, &[], 0, false);
    for off in [
        "Clean up this folder…",
        "Send this folder to another PC…",
        "Receive from another PC here…",
        "Scan this folder with Microsoft Defender",
        "Look for passwords and keys left in files",
    ] {
        assert!(entry(&nodes, off).disabled, "{off}");
    }
    // The rules in general still open (an empty folder asks for them all).
    assert_eq!(
        entry(&nodes, "Folder rules: sort out what comes in…").run,
        Run::Ask(MenuEvent::Rule { folder: String::new() })
    );
    f.read_only = false;
    let nodes = menus::tools_menu(&f, &[], 0, false);
    assert_eq!(
        entry(&nodes, "Folder rules: sort out what comes in…").run,
        Run::Ask(MenuEvent::Rule {
            folder: "C:\\Work".into()
        })
    );
}

#[test]
fn the_view_menu_has_the_toggles_and_the_views_only_when_the_bar_is_that_narrow() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut f = folder_facts();
    f.show_hidden = true;
    f.dual = true;
    let narrow = menus::view_menu(&f, 3);
    assert_eq!(
        labels(&narrow),
        ["Show hidden items", "Preview pane", "Dual pane", "Compare panes"]
    );
    assert_eq!(entry(&narrow, "Show hidden items").check, Some(true));
    assert_eq!(entry(&narrow, "Show hidden items").hint.as_deref(), Some("Ctrl+H"));
    assert_eq!(entry(&narrow, "Preview pane").hint.as_deref(), Some("Alt+P"));
    assert_eq!(entry(&narrow, "Dual pane").check, Some(true));
    assert_eq!(entry(&narrow, "Compare panes").check, Some(false));
    let narrower = menus::view_menu(&f, 5);
    assert_eq!(
        labels(&narrower),
        [
            "List",
            "Grid, with thumbnails",
            "Columns",
            "-",
            "Show hidden items",
            "Preview pane",
            "Dual pane",
            "Compare panes"
        ]
    );
    assert_eq!(entry(&narrower, "List").check, Some(true));
    assert_eq!(entry(&narrower, "List").hint.as_deref(), Some("Ctrl+Shift+6"));
    // Where views can't be chosen (This PC) they are greyed; a map unchecks them.
    let mut pc = Facts::none();
    pc.mapped = false;
    assert!(entry(&menus::view_menu(&pc, 5), "List").disabled);
    f.mapped = true;
    assert_eq!(entry(&menus::view_menu(&f, 5), "List").check, Some(false));
}

#[test]
fn the_edits_menu_has_what_the_folded_buttons_had_with_their_conditions() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut f = folder_facts();
    let nodes = menus::edits_menu(&f);
    assert_eq!(labels(&nodes), ["Cut", "Copy", "Paste", "-", "Rename", "Delete"]);
    assert!(entry(&nodes, "Cut").disabled && entry(&nodes, "Delete").disabled);
    assert!(!entry(&nodes, "Paste").disabled);
    assert_eq!(entry(&nodes, "Cut").hint.as_deref(), Some("Ctrl+X"));
    assert_eq!(entry(&nodes, "Rename").hint.as_deref(), Some("F2"));
    assert_eq!(entry(&nodes, "Delete").hint.as_deref(), Some("Del"));
    assert!(entry(&nodes, "Delete").danger);

    f.selected = 3;
    f.dual = true;
    f.other = true;
    f.pair = Some(Pair {
        left: "C:\\a\\x.txt".into(),
        left_name: "x.txt".into(),
        right: "C:\\b\\x.txt".into(),
        right_name: "x.txt".into(),
    });
    let nodes = menus::edits_menu(&f);
    assert_eq!(
        labels(&nodes),
        [
            "Cut",
            "Copy",
            "Paste",
            "Copy to the other pane",
            "Move to the other pane",
            "Compare “x.txt” and “x.txt”",
            "-",
            "Rename 3 items",
            "Delete"
        ]
    );
    assert!(!entry(&nodes, "Copy to the other pane").disabled);
    // Read-only: only Copy (extracted) stays.
    f.read_only = true;
    let nodes = menus::edits_menu(&f);
    assert!(entry(&nodes, "Cut").disabled && entry(&nodes, "Paste").disabled);
    assert!(entry(&nodes, "Delete").disabled && entry(&nodes, "Rename 3 items").disabled);
    assert!(!entry(&nodes, "Copy").disabled);
}

#[test]
fn a_project_menu_lists_tasks_under_the_projects_and_offers_the_tool_a_project_needs() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let groups = vec![
        TaskGroup {
            folder: "C:\\Work\\app".into(),
            source: "npm".into(),
            tasks: vec![
                ProjectTask {
                    name: "build".into(),
                    command: "npm run build".into(),
                    detail: "vite build".into(),
                },
                ProjectTask {
                    name: "test".into(),
                    command: "npm run test".into(),
                    detail: String::new(),
                },
            ],
            missing: None,
        },
        TaskGroup {
            folder: "C:\\Work".into(),
            source: "just".into(),
            tasks: vec![],
            missing: Some("just".into()),
        },
    ];
    let nodes = menus::tasks_menu(&groups);
    assert_eq!(
        labels(&nodes),
        [
            "# app · npm",
            "build: vite build",
            "test",
            "# Work · just",
            "Install just to run these"
        ]
    );
    assert_eq!(
        Pick::of(&entry(&nodes, "test").run),
        Some(Pick::Task { group: 0, task: 1 })
    );
    assert_eq!(
        entry(&nodes, "Install just to run these").run,
        Run::Ask(MenuEvent::InstallTool { tool: "just".into() })
    );
}

#[test]
fn a_pick_survives_being_carried_in_a_run() {
    let picks = [
        Pick::Hidden,
        Pick::Preview,
        Pick::Dual,
        Pick::ComparePanes,
        Pick::View(ViewMode::List),
        Pick::View(ViewMode::Grid),
        Pick::View(ViewMode::Columns),
        Pick::Tiles(TileSize::Small),
        Pick::Tiles(TileSize::Huge),
        Pick::Larger,
        Pick::Smaller,
        Pick::Palette,
        Pick::DiskUsage,
        Pick::FolderSizes,
        Pick::TerminalPane,
        Pick::CopyToOther,
        Pick::MoveToOther,
        Pick::Task { group: 3, task: 12 },
    ];
    for pick in picks {
        assert_eq!(Pick::of(&pick.run()), Some(pick.clone()), "{pick:?}");
    }
    // A folder a menu would open is not a pick.
    assert_eq!(Pick::of(&Run::Do(Task::Navigate("C:\\Work".into()))), None);
    assert_eq!(Pick::of(&Run::Do(Task::Navigate("command-bar:nonsense".into()))), None);
    assert_eq!(Pick::of(&Run::Nothing), None);
}

#[gpui_kit::test]
fn opening_a_menu_and_choosing_a_row_runs_it_and_escape_closes_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    rig.click("new-menu");
    assert!(rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    rig.press("escape");
    assert!(!rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    // Open again and take the first row, Folder: the New folder action.
    // The press that closed a menu belongs to the button that opened it, for a moment.
    std::thread::sleep(SAME_PRESS + Duration::from_millis(50));
    rig.click("new-menu");
    assert!(rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    rig.press("enter");
    assert!(!rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    assert_eq!(rig.log(), ["NewFolder"]);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_tools_row_hands_its_dialog_to_the_window_as_a_menu_event(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go(r"C:\Work");
    // Open the Tools menu and walk down to "Recent changes in this folder" with the keyboard.
    rig.click("tools");
    let nodes = rig
        .bar
        .read_with(&rig.cx, |bar, cx| bar.menu_view().map(|v| v.read(cx).nodes().to_vec()))
        .expect("open");
    let rows: Vec<&Entry> = nodes
        .iter()
        .filter_map(|n| match n {
            Node::Item(e) if !e.disabled => Some(e),
            _ => None,
        })
        .collect();
    let at = rows
        .iter()
        .position(|e| e.label == "Recent changes in this folder")
        .expect("the row");
    for _ in 0..at {
        rig.press("down");
    }
    rig.press("enter");
    assert_eq!(
        rig.events(),
        [CommandBarEvent::Menu(MenuEvent::Timeline {
            folder: r"C:\Work".into()
        })]
    );
    // Folder rules with no folder is the window's dialog, not an event: the bar opens it itself.
    rig.clear();
    assert!(!rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    testing::set_rtl(false);
}

// ---- Project tasks ---------------------------------------------------------------------------------------------

fn tasks_source() -> Sources {
    let mut sources = Sources::empty();
    sources.tasks = Rc::new(|folder| {
        Box::pin(async move {
            Ok(if folder == "C:\\Work" {
                vec![TaskGroup {
                    folder,
                    source: "npm".into(),
                    tasks: vec![ProjectTask {
                        name: "build".into(),
                        command: "npm run build".into(),
                        detail: String::new(),
                    }],
                    missing: None,
                }]
            } else {
                Vec::new()
            })
        })
    });
    sources
}

#[gpui_kit::test]
fn a_run_button_appears_for_a_project_and_a_task_goes_to_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open_with(cx, false, 1000., tasks_source());
    rig.go("C:\\Other");
    assert!(!rig.present("Run"));
    rig.go("C:\\Work");
    for _ in 0..16 {
        rig.cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    rig.settle();
    assert!(rig.present("Run"), "the project's tasks were read");
    rig.click("run");
    assert!(rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    // The first row past the heading is the task.
    rig.press("enter");
    assert_eq!(
        rig.events(),
        [CommandBarEvent::RunTask {
            folder: "C:\\Work".into(),
            command: "npm run build".into()
        }]
    );
    // Back in a folder with nothing to run, it goes again.
    rig.go("C:\\Other");
    assert!(!rig.present("Run"));
    testing::set_rtl(false);
}

// ---- Folding -----------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_bar_folds_as_it_narrows_and_unfolds_as_it_widens(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1400.);
    rig.go("C:\\Work");
    rig.select(&["C:\\Work\\notes.txt"]);
    assert_eq!(rig.fold(), 0);
    let wide_new = rig.find("New folder").expect("New folder").1.size.width;

    let mut folds = Vec::new();
    for width in [1400., 900., 700., 600., 500., 400., 300., 200.] {
        rig.width(width);
        folds.push(rig.fold());
    }
    assert!(
        folds.windows(2).all(|pair| pair[0] <= pair[1]),
        "narrower never unfolds: {folds:?}"
    );
    assert_eq!(folds[0], 0);
    assert_eq!(*folds.last().expect("folds"), MOST, "{folds:?}");
    // Every fold between is used on the way (the bar goes through them as the widths shrink).
    assert!(
        folds.iter().collect::<std::collections::HashSet<_>>().len() >= 4,
        "{folds:?}"
    );

    // Back to wide: every label is back.
    rig.width(1400.);
    assert_eq!(rig.fold(), 0);
    assert_eq!(rig.find("New folder").expect("New folder").1.size.width, wide_new);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn each_fold_hides_what_the_typescript_hid(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1400.);
    rig.go("C:\\Work");
    // With something selected, so Cut, Copy, Rename and Delete are there to fold.
    rig.select(&["C:\\Work\\notes.txt"]);
    // Find a width for each fold by narrowing until the bar reaches it.
    let mut seen: std::collections::BTreeMap<usize, f32> = Default::default();
    let mut width = 1400.;
    while width > 150. {
        rig.width(width);
        seen.entry(rig.fold()).or_insert(width);
        width -= 20.;
    }
    for fold in 0..=MOST {
        assert!(seen.contains_key(&fold), "fold {fold} never reached: {seen:?}");
    }
    let at = |rig: &mut Rig, fold: usize| rig.width(seen[&fold]);

    // Fold 0 shows the words beside the icons: the button is wider than its icon alone.
    at(&mut rig, 0);
    let wide = rig.find("New folder").expect("New folder").1.size.width;
    // Fold 1 and 2 take the labels off but keep the buttons and their names.
    at(&mut rig, 1);
    let icon_only = rig.find("New folder").expect("the name stays").1.size.width;
    assert!(icon_only < wide, "labels go first: {icon_only:?} < {wide:?}");
    assert!(rig.present("Disk usage") && rig.present("Show hidden items (Ctrl+H)"));
    // Fold 3: the four toggles are in the View menu.
    at(&mut rig, 3);
    assert!(!rig.present("Show hidden items (Ctrl+H)") && !rig.present("Preview pane (Alt+P)"));
    assert!(rig.present("View: hidden items, preview pane, dual pane, compare panes"));
    assert!(rig.present("Disk usage"));
    // Fold 4: Disk usage and Folder sizes are in the Tools menu.
    at(&mut rig, 4);
    assert!(!rig.present("Disk usage") && !rig.present("Folder sizes"));
    assert!(rig.present("List (Ctrl+Shift+6)") && rig.present("Command palette (Ctrl+K)"));
    // Fold 5: list and grid are in the View menu, the palette in Tools.
    at(&mut rig, 5);
    assert!(!rig.present("List (Ctrl+Shift+6)") && !rig.present("Command palette (Ctrl+K)"));
    assert!(rig.present("View: list or grid, hidden items, preview pane, dual pane, compare panes"));
    assert!(rig.present("Cut (Ctrl+X)"));
    // Fold 6: the edits are in a "…" menu.
    at(&mut rig, 6);
    assert!(!rig.present("Cut (Ctrl+X)") && !rig.present("Delete (Del) · Shift+Del deletes permanently"));
    assert!(rig.present("Cut, copy, paste, rename, delete"));
    // What was folded is still reachable: the Tools menu at this fold has them.
    rig.click("tools");
    let nodes = rig
        .bar
        .read_with(&rig.cx, |bar, cx| bar.menu_view().map(|v| v.read(cx).nodes().to_vec()))
        .expect("the Tools menu is open");
    let words = labels(&nodes);
    assert_eq!(&words[..5], ["Command palette", "-", "Disk usage", "Folder sizes", "-"]);
    rig.press("escape");
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn what_is_shown_fits_inside_the_bar_at_every_fold_but_the_last(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1400.);
    rig.go("C:\\Work");
    let mut width = 1400.;
    while width > 330. {
        rig.width(width);
        let end = rig.cx.update(|window, cx| {
            window.render_frame(cx);
            window.find("command-bar").bounds().right()
        });
        // The last button at the end edge: the compare panes toggle, or the View menu once they fold into it.
        let last = rig.cx.update(|window, _| {
            window
                .try_find("compare-panes")
                .or_else(|| window.try_find("view-menu"))
                .map(|e| e.bounds().right())
                .expect("the end of the bar")
        });
        if rig.fold() < MOST {
            assert!(
                last <= end + px(1.),
                "at {width} (fold {}): {last:?} past {end:?}",
                rig.fold()
            );
        }
        width -= 50.;
    }
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn text_size_and_the_button_set_are_measured_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1000.);
    rig.go("C:\\Work");
    let before = rig.fold();
    // Dual pane adds buttons: the bar needs more, so it never folds less than before.
    rig.act(|e, cx| e.toggle_dual(cx));
    assert!(rig.fold() >= before);
    rig.width(1400.);
    assert_eq!(rig.fold(), 0);
    testing::set_rtl(false);
}

// ---- Names, keys and tooltips ------------------------------------------------------------------------------------

#[gpui_kit::test]
fn every_control_has_a_name_and_no_two_share_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1400.);
    rig.go("C:\\Work");
    rig.act(|e, cx| e.toggle_dual(cx));
    rig.go("C:\\Other");
    rig.select(&["C:\\Other\\other.txt"]);
    let names = rig.buttons();
    assert!(names.len() >= 20, "{names:?}");
    let mut seen = std::collections::HashSet::new();
    for name in &names {
        assert!(!name.is_empty());
        assert!(seen.insert(name.clone()), "two controls are named {name:?}: {names:?}");
    }
    let (toolbar, views) = rig.cx.update(|window, _| {
        (
            window.try_find_by_label("Commands").map(|e| e.role()),
            window.try_find_by_label("View").map(|e| e.role()),
        )
    });
    assert_eq!(toolbar, Some(Some(Role::Toolbar)));
    assert_eq!(views, Some(Some(Role::Group)));
    testing::set_rtl(false);
}

#[test]
fn the_tooltips_name_the_keys_the_bindings_table_gives_each_action() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    for (sentence, expected) in [
        (
            keyed("New folder ({0})", &[&actions::NewFolder]),
            "New folder (Ctrl+Shift+N)",
        ),
        (keyed("Cut ({0})", &[&actions::Cut]), "Cut (Ctrl+X)"),
        (keyed("Copy ({0})", &[&actions::Copy]), "Copy (Ctrl+C)"),
        (keyed("Paste ({0})", &[&actions::Paste]), "Paste (Ctrl+V)"),
        (
            keyed(
                "Rename ({0}) · {1} renames many",
                &[&actions::Rename, &actions::BatchRename],
            ),
            "Rename (F2) · Ctrl+Shift+R renames many",
        ),
        (
            keyed(
                "Delete ({0}) · {1} deletes permanently",
                &[&actions::Delete, &actions::DeletePermanently],
            ),
            "Delete (Del) · Shift+Del deletes permanently",
        ),
        (
            keyed("Command palette ({0})", &[&actions::CommandPalette]),
            "Command palette (Ctrl+K)",
        ),
        (
            keyed("Show hidden items ({0})", &[&actions::ToggleHidden]),
            "Show hidden items (Ctrl+H)",
        ),
        (
            keyed("Preview pane ({0})", &[&actions::TogglePreview]),
            "Preview pane (Alt+P)",
        ),
        (
            keyed("Dual pane: two folders side by side ({0})", &[&actions::DualPane]),
            "Dual pane: two folders side by side (Ctrl+Shift+D)",
        ),
        (
            keyed(
                "Compare panes: highlight differences between both panes ({0})",
                &[&actions::ComparePanes],
            ),
            "Compare panes: highlight differences between both panes (Ctrl+Shift+M)",
        ),
        (keyed("List ({0})", &[&actions::ListView]), "List (Ctrl+Shift+6)"),
        (
            keyed("Grid, with thumbnails ({0})", &[&actions::GridView]),
            "Grid, with thumbnails (Ctrl+Shift+2)",
        ),
        (
            keyed("Compare files ({0})", &[&actions::CompareFiles]),
            "Compare files (Ctrl+M)",
        ),
    ] {
        assert_eq!(sentence, expected);
    }
    // Every action the bar names a key for is bound.
    for action in [
        &actions::NewFolder as &dyn Action,
        &actions::Cut,
        &actions::Copy,
        &actions::Paste,
        &actions::Rename,
        &actions::BatchRename,
        &actions::Delete,
        &actions::DeletePermanently,
        &actions::CommandPalette,
        &actions::ToggleHidden,
        &actions::TogglePreview,
        &actions::DualPane,
        &actions::ComparePanes,
        &actions::ListView,
        &actions::GridView,
        &actions::GridLarger,
        &actions::GridSmaller,
        &actions::GridReset,
        &actions::CompareFiles,
        &actions::ToggleTerminal,
    ] {
        assert!(keys_of(action).is_some(), "{} has no key", action.name());
    }
}

// ---- The Arabic interface ----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn in_arabic_the_bar_mirrors_and_the_tooltips_are_the_catalogs(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, true, 1200.);
    rig.go("C:\\Work");
    rig.select(&["C:\\Work\\تقرير.txt"]);
    // The start edge is the right: New folder is right of the palette, and the view toggles are at the left end.
    let new = rig.find("مجلد جديد").expect("the catalog's New folder").1;
    let palette = rig.find("لوحة الأوامر (Ctrl+K)").expect("the catalog's palette").1;
    assert!(
        new.left() > palette.left(),
        "New folder {new:?} is at the start (right), the palette {palette:?} at the end"
    );
    // Cut, Paste, hidden items and the preview pane are in the catalog with their keys.
    for name in [
        "قص (Ctrl+X)",
        "لصق (Ctrl+V)",
        "إظهار العناصر المخفية (Ctrl+H)",
        "جزء المعاينة (Alt+P)",
    ] {
        assert!(rig.present(name), "{name}");
    }
    // Cut and Copy keep their order along the reading direction: Cut is first, so it is right of Copy.
    let cut = rig.find("قص (Ctrl+X)").expect("cut").1;
    let paste = rig.find("لصق (Ctrl+V)").expect("paste").1;
    assert!(cut.left() > paste.left());
    // A sentence nobody translated stays English, whatever the direction.
    assert!(rig.present("Rename (F2) · Ctrl+Shift+R renames many"));
    // Narrow, the fold still works mirrored.
    rig.width(300.);
    assert!(rig.fold() > 0);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn an_arabic_name_is_in_the_compare_tooltip_and_the_menu_opens_toward_the_start(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, true, 1000.);
    rig.go("C:\\Work");
    rig.select(&["C:\\Work\\تقرير.txt", "C:\\Work\\notes.txt"]);
    let pair = rig.facts().pair.expect("two files");
    assert_eq!(pair.left_name, "notes.txt");
    assert_eq!(pair.right_name, "تقرير.txt");
    let label = "Compare “notes.txt” and “تقرير.txt” (Ctrl+M)";
    assert!(rig.present(label), "{:?}", rig.buttons());
    // The New menu opens over its button (the island is at the foot of the window), anchored at the button's
    // start edge: the right in Arabic.
    rig.click("new-menu");
    let (at, button) = rig.bar.read_with(&rig.cx, |bar, _| {
        (
            bar.menu.as_ref().map(|m| m.at),
            bar.anchors.borrow().get("new").copied(),
        )
    });
    let (at, button) = (at.expect("open"), button.expect("measured"));
    assert_eq!(at.x, button.right());
    assert_eq!(at.y, button.top() - px(6.));
    rig.press("escape");
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_arabic_bar_in_the_recycle_bin_and_an_archive_names_things_in_arabic_or_english(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, true, 1200.);
    rig.go("C:\\Work\\pack.zip");
    // The catalog has "Restore"; the archive note has no Arabic yet and stays English.
    assert!(rig.present("Read-only archive") && rig.present("Extract all"));
    rig.go(glint_model::paths::RECYCLE_BIN);
    assert!(rig.present("استعادة"), "{:?}", rig.buttons());
    testing::set_rtl(false);
}

// ---- Keyboard and icons -----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_buttons_are_reached_with_tab_and_pressed_with_enter(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = open(cx, false, 1400.);
    rig.go(r"C:\Work");
    // The bar takes the keyboard (as a click on empty space beside the buttons would), then Tab walks the buttons.
    rig.bar
        .update_in(&mut rig.cx, |bar, window, cx| bar.focus.focus(window, cx));
    rig.press("tab");
    rig.press("enter");
    assert_eq!(rig.log(), ["NewFolder"], "the first button is New folder");
    rig.clear();
    // New's arrow is next; Enter opens its menu, which has the keyboard until Escape.
    rig.press("tab");
    rig.press("enter");
    assert!(rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    rig.press("escape");
    assert!(!rig.bar.read_with(&rig.cx, |bar, _| bar.menu_open()));
    testing::set_rtl(false);
}

#[test]
fn every_icon_the_bar_draws_is_in_the_embedded_assets() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    // A missing icon draws nothing and says nothing, so each one the bar uses is checked here (kit.rs's own list
    // is the coordinator's).
    for name in [
        IconName::ArchiveRestore,
        IconName::BrushCleaning,
        IconName::Check,
        IconName::ChevronDown,
        IconName::ClipboardPaste,
        IconName::ClockArrowLeft,
        IconName::Columns2,
        IconName::Columns3,
        IconName::Command,
        IconName::Copy,
        IconName::CopyPlus,
        IconName::Download,
        IconName::Ellipsis,
        IconName::Eye,
        IconName::EyeOff,
        IconName::FileClock,
        IconName::FilePlus,
        IconName::FileText,
        IconName::Files,
        IconName::FingerprintPattern,
        IconName::FolderLock,
        IconName::FolderOutput,
        IconName::FolderPlus,
        IconName::GitCompareArrows,
        IconName::HardDrive,
        IconName::ImageDown,
        IconName::KeyRound,
        IconName::LayoutDashboard,
        IconName::LayoutGrid,
        IconName::List,
        IconName::LoaderCircle,
        IconName::Lock,
        IconName::PanelRight,
        IconName::PencilOff,
        IconName::PenLine,
        IconName::Play,
        IconName::RotateCcw,
        IconName::ScanSearch,
        IconName::Scissors,
        IconName::Send,
        IconName::ShieldCheck,
        IconName::SlidersHorizontal,
        IconName::SquareTerminal,
        IconName::TextSearch,
        IconName::Trash,
        IconName::WandSparkles,
        IconName::Wifi,
        IconName::Wrench,
        IconName::ZoomIn,
        IconName::ZoomOut,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}
