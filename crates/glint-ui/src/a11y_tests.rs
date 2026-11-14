//! Accessibility and right-to-left sweep over the main views (wave 17, a11y-rtl): every control a person can reach
//! has a role and a name, and the bars mirror in an Arabic window.

use crate::actions;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{self, Drive, DriveKind, KnownFolder, KnownId, NavOptions};
use crate::testing;
use crate::workspace::Workspace;
use glint_model::paths::THIS_PC;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Bounds, Pixels, Role, TestAppContext, VisualTestContext};
use std::path::PathBuf;

/// The roles a person acts on. Every element with one of these must have a name a screen reader can say.
const INTERACTIVE: &[Role] = &[
    Role::Button,
    Role::CheckBox,
    Role::RadioButton,
    Role::MenuItem,
    Role::MenuItemCheckBox,
    Role::Tab,
    Role::TextInput,
    Role::PasswordInput,
    Role::MultilineTextInput,
    Role::ListBoxOption,
    Role::Link,
    Role::ComboBox,
    Role::Slider,
    Role::Switch,
];

/// What is on screen with an interactive role and no name, each as "Role at x, y (w x h) path". Empty when every
/// control is named.
fn unnamed(cx: &mut VisualTestContext) -> Vec<String> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .elements()
            .into_iter()
            .filter(|element| element.role().is_some_and(|role| INTERACTIVE.contains(&role)))
            .filter(|element| element.label().is_none_or(|label| label.trim().is_empty()))
            .map(|element| {
                let b = element.bounds();
                format!(
                    "{:?} at {:.0},{:.0} ({:.0}x{:.0}) {:?}",
                    element.role(),
                    f32::from(b.origin.x),
                    f32::from(b.origin.y),
                    f32::from(b.size.width),
                    f32::from(b.size.height),
                    element.path()
                )
            })
            .collect()
    })
}

/// How many elements with an interactive role are on screen: a sweep that sees none proves nothing.
fn interactive_count(cx: &mut VisualTestContext) -> usize {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .elements()
            .into_iter()
            .filter(|element| element.role().is_some_and(|role| INTERACTIVE.contains(&role)))
            .count()
    })
}

/// Fails with the list when something interactive has no name.
#[track_caller]
fn assert_all_named(cx: &mut VisualTestContext, view: &str) {
    let missing = unnamed(cx);
    assert!(
        missing.is_empty(),
        "{view}: {} control(s) without a name:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

/// A 24-bit BMP of one color, the simplest picture to make by hand.
fn bmp(width: u32, height: u32) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let mut out = Vec::new();
    out.extend(b"BM");
    out.extend((54 + row * height).to_le_bytes());
    out.extend([0; 4]);
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend(width.to_le_bytes());
    out.extend(height.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(24u16.to_le_bytes());
    out.extend([0; 4]);
    out.extend((row * height).to_le_bytes());
    out.extend([0; 16]);
    for _ in 0..height {
        for _ in 0..width {
            out.extend([200, 80, 40]);
        }
        out.extend(std::iter::repeat_n(0u8, (row - width * 3) as usize));
    }
    out
}

/// A temp folder with a text file and a picture in it, so the preview has something real to read. Removed on drop.
struct Files {
    dir: PathBuf,
}

impl Files {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("glint-a11y-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("notes.txt"), "Some notes\nA second line\n");
        let _ = std::fs::write(dir.join("pic.bmp"), bmp(8, 8));
        Files { dir }
    }

    fn path(&self) -> String {
        self.dir.to_string_lossy().into_owned()
    }

    fn join(&self, name: &str) -> String {
        self.dir.join(name).to_string_lossy().into_owned()
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The whole window over a fake disk that lists `files`' folder (and a `src` folder), the folder open in the first
/// tab.
fn open_workspace(
    cx: &mut TestAppContext,
    files: &Files,
    rtl: bool,
) -> (VisualTestContext, gpui_kit::Entity<Workspace>) {
    let (visual, workspace, _) = open_workspace_on(cx, files, rtl);
    (visual, workspace)
}

/// [`open_workspace`], with the fake disk too (to say what a search inside files finds).
fn open_workspace_on(
    cx: &mut TestAppContext,
    files: &Files,
    rtl: bool,
) -> (VisualTestContext, gpui_kit::Entity<Workspace>, FakeDisk) {
    let dir = files.path();
    let disk = FakeDisk::new();
    disk.folder(
        &dir,
        vec![
            folder(&dir, "src"),
            file(&dir, "notes.txt", 24, 0),
            file(&dir, "pic.bmp", 246, 0),
            file(&dir, "تقرير.txt", 5, 0),
        ],
    );
    disk.folder(&files.join("src"), vec![file(&files.join("src"), "main.rs", 10, 0)]);
    disk.set_drives(vec![
        Drive {
            path: "C:\\".into(),
            label: "Windows".into(),
            kind: DriveKind::Fixed,
            fs: "NTFS".into(),
            total: 500,
            free: 200,
        },
        Drive {
            path: "E:\\".into(),
            label: String::new(),
            kind: DriveKind::Removable,
            fs: "FAT32".into(),
            total: 32,
            free: 8,
        },
    ]);
    disk.set_known(vec![KnownFolder {
        id: KnownId::Home,
        name: "Home",
        path: files.path(),
    }]);
    cx.update(|cx| {
        testing::init(cx);
        explorer::init_with(cx, disk.source());
    });
    testing::set_rtl(rtl);
    let (mut visual, workspace) = testing::open_window(cx, |window, cx| cx.new(|cx| Workspace::new(window, cx)));
    visual.update(|_, cx| {
        explorer::explorer(cx).update(cx, |e, cx| {
            e.init_explorer(
                vec![explorer::LaunchTarget {
                    path: dir.clone(),
                    select: None,
                }],
                cx,
            )
            .detach()
        })
    });
    visual.run_until_parked();
    (visual, workspace, disk)
}

/// Lets core's threads and the foreground tasks that wait for them finish.
fn settle(cx: &mut VisualTestContext) {
    for _ in 0..3 {
        // The preview waits a moment on a fake clock before it reads the selection.
        cx.executor().advance_clock(std::time::Duration::from_millis(100));
        for _ in 0..20 {
            cx.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(3));
        }
    }
}

fn has(cx: &mut VisualTestContext, id: &'static str) -> bool {
    bounds_of(cx, id).is_some()
}

fn bounds_of(cx: &mut VisualTestContext, id: &'static str) -> Option<Bounds<Pixels>> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).map(|element| element.bounds())
    })
}

/// Selects `names` in the folder and lets the preview read them.
fn select(cx: &mut VisualTestContext, files: &Files, names: &[&str]) {
    let paths: Vec<String> = names.iter().map(|name| files.join(name)).collect();
    cx.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.set_selection(paths, None, None, cx)));
    settle(cx);
}

fn go(cx: &mut VisualTestContext, path: &str) {
    let path = path.to_string();
    cx.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx)));
    settle(cx);
}

/// Sweeps the window as it is, checking first that the sweep sees a window worth sweeping.
#[track_caller]
fn sweep(cx: &mut VisualTestContext, view: &str, at_least: usize) {
    let seen = interactive_count(cx);
    assert!(seen >= at_least, "{view}: the sweep sees the controls, saw only {seen}");
    assert_all_named(cx, view);
}

#[gpui_kit::test]
fn the_main_window_in_list_view_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("list");
    let (mut visual, _) = open_workspace(cx, &files, false);
    settle(&mut visual);
    sweep(&mut visual, "the main window (list view)", 20);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_selection_in_the_list_and_its_text_preview_have_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("text");
    let (mut visual, _) = open_workspace(cx, &files, false);
    select(&mut visual, &files, &["notes.txt"]);
    assert!(has(&mut visual, "text-copy"), "the text file's preview is on screen");
    sweep(&mut visual, "the preview pane with a text file", 20);
    select(&mut visual, &files, &["notes.txt", "pic.bmp"]);
    sweep(&mut visual, "the preview pane with two files", 20);
    select(&mut visual, &files, &["src"]);
    sweep(&mut visual, "the preview pane with a folder", 20);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_preview_of_a_picture_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("picture");
    let (mut visual, _) = open_workspace(cx, &files, false);
    select(&mut visual, &files, &["pic.bmp"]);
    assert!(has(&mut visual, "preview-entry"), "the picture's preview is on screen");
    sweep(&mut visual, "the preview pane with a picture", 20);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_grid_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("grid");
    let (mut visual, _) = open_workspace(cx, &files, false);
    visual.dispatch_action(actions::GridView);
    settle(&mut visual);
    sweep(&mut visual, "the grid", 20);
    select(&mut visual, &files, &["pic.bmp"]);
    sweep(&mut visual, "the grid with a picture selected", 20);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_columns_view_and_dual_panes_have_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("panes");
    let (mut visual, _) = open_workspace(cx, &files, false);
    visual.dispatch_action(actions::ColumnsView);
    settle(&mut visual);
    sweep(&mut visual, "the columns view", 20);
    visual.dispatch_action(actions::ListView);
    visual.dispatch_action(actions::DualPane);
    settle(&mut visual);
    sweep(&mut visual, "two panes", 20);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn this_pc_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("thispc");
    let (mut visual, _) = open_workspace(cx, &files, false);
    go(&mut visual, THIS_PC);
    sweep(&mut visual, "This PC", 20);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_second_tab_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("tabs");
    let (mut visual, _) = open_workspace(cx, &files, false);
    visual.dispatch_action(actions::NewTab);
    settle(&mut visual);
    sweep(&mut visual, "two tabs", 20);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_command_bar_menus_and_the_context_menu_have_every_item_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("menus");
    let (mut visual, _) = open_workspace(cx, &files, false);
    select(&mut visual, &files, &["notes.txt"]);
    // The island folds with the room the dock has: the View menu is there once the toggles have folded into it,
    // the tile sizes until the views have. Whichever of the two this window shows is swept; New and Tools always.
    let mut swept = 0;
    for id in ["new-menu", "view-menu", "tools", "tile-sizes"] {
        if !has(&mut visual, id) {
            assert!(id == "view-menu" || id == "tile-sizes", "{id} is always in the island");
            continue;
        }
        swept += 1;
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        settle(&mut visual);
        let items = visual.update(|window, cx| {
            window.render_frame(cx);
            window.find_all_by_role(Role::MenuItem).len() + window.find_all_by_role(Role::MenuItemCheckBox).len()
        });
        assert!(items > 0, "the {id} menu lists items");
        sweep(&mut visual, id, 20);
        visual.update(|window, cx| window.press("escape", cx));
        settle(&mut visual);
    }
    assert!(
        swept >= 3,
        "New, Tools and at least one of the view menus were swept, got {swept}"
    );
    // The menu of a file.
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.right_click(("file-row", 1_usize), cx);
    });
    settle(&mut visual);
    let items = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find_all_by_role(Role::MenuItem).len()
    });
    assert!(items > 5, "the file's context menu lists its items, saw {items}");
    sweep(&mut visual, "the file's context menu", 20);
    testing::set_rtl(false);
}

/// The left edge of the element with this id, which must be on screen.
#[track_caller]
fn left_of(cx: &mut VisualTestContext, id: &'static str) -> f32 {
    match bounds_of(cx, id) {
        Some(bounds) => f32::from(bounds.origin.x),
        None => panic!("{id} isn't on screen"),
    }
}

/// Each bar lists its first item and its last: the first starts at the start edge, so left of the last in English
/// and right of it in Arabic.
const BARS: &[(&str, &str, &str)] = &[
    // The tabs are a list down the rail: a tab's icon is at its start edge, the list's + at its header's end.
    ("the tabs", "tab-busy-1", "new-tab-0"),
    ("the address bar", "address-back", "address-refresh"),
    ("the command bar", "new-folder", "view-menu"),
    ("the status bar", "status-counts", "status-bar-details"),
    ("the window (sidebar, files)", "sidebar", "content"),
];

#[gpui_kit::test]
fn the_bars_lay_out_from_the_left_in_english_and_from_the_right_in_arabic(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    for (rtl, label) in [(false, "English"), (true, "Arabic")] {
        let files = Files::new(if rtl { "bars-ar" } else { "bars-en" });
        let (mut visual, _) = open_workspace(cx, &files, rtl);
        select(&mut visual, &files, &["notes.txt"]);
        for (bar, first, last) in BARS {
            let (first_left, last_left) = (left_of(&mut visual, first), left_of(&mut visual, last));
            if rtl {
                assert!(
                    first_left > last_left,
                    "{label}: in {bar}, {first} ({first_left}) should be right of {last} ({last_left})"
                );
            } else {
                assert!(
                    first_left < last_left,
                    "{label}: in {bar}, {first} ({first_left}) should be left of {last} ({last_left})"
                );
            }
        }
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn the_main_window_in_arabic_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let files = Files::new("arabic");
    let (mut visual, _) = open_workspace(cx, &files, true);
    select(&mut visual, &files, &["notes.txt"]);
    sweep(&mut visual, "the main window in Arabic", 20);
    visual.dispatch_action(actions::GridView);
    settle(&mut visual);
    sweep(&mut visual, "the grid in Arabic", 20);
    testing::set_rtl(false);
}

/// Palette sources that answer from nothing, so no tool of this PC is asked.
fn quiet_palette() -> crate::components::palette::Sources {
    use crate::components::palette::{Sources, Tools};
    Sources {
        calc: std::sync::Arc::new(|_| Box::pin(async { Err("no calculator here".to_string()) })),
        zoxide: std::sync::Arc::new(|| Box::pin(async { Ok(None) })),
        tools: std::sync::Arc::new(|_| Box::pin(async { Tools::default() })),
        debounce: std::time::Duration::ZERO,
    }
}

#[gpui_kit::test]
fn the_command_palette_has_every_control_and_result_named(cx: &mut TestAppContext) {
    use crate::components::palette;
    let _guard = testing::lock();
    for rtl in [false, true] {
        let files = Files::new("palette");
        let (mut visual, _) = open_workspace(cx, &files, rtl);
        select(&mut visual, &files, &["notes.txt"]);
        visual.update(|window, cx| palette::open_with(window, cx, quiet_palette()));
        settle(&mut visual);
        let options = visual.update(|window, cx| {
            window.render_frame(cx);
            window.find_all_by_role(Role::ListBoxOption).len()
        });
        assert!(options > 3, "the palette lists commands, saw {options}");
        assert_all_named(&mut visual, "the palette");
        // Typing narrows the list; what is left is named too.
        visual.update(|window, cx| palette::type_query("new", window, cx));
        settle(&mut visual);
        assert_all_named(&mut visual, "the palette with a query");
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn every_section_of_settings_has_every_control_named(cx: &mut TestAppContext) {
    use crate::components::settings::tests::{open_settings, settle as settle_settings, Fake};
    let _guard = testing::lock();
    for rtl in [false, true] {
        let fake = std::sync::Arc::new(Fake::new());
        let (mut visual, view) = open_settings(cx, &fake, None);
        testing::set_rtl(rtl);
        let sections: Vec<&'static str> = view.read_with(&visual, |v, _| v.sections().to_vec());
        assert!(sections.len() >= 8, "Settings has its sections, saw {sections:?}");
        let mut seen = 0;
        for section in sections {
            view.update(&mut visual, |v, cx| v.scroll_to(section, cx));
            settle_settings(&mut visual, |_| true);
            visual.run_until_parked();
            seen += interactive_count(&mut visual);
            assert_all_named(
                &mut visual,
                &format!("Settings, {section}{}", if rtl { " (Arabic)" } else { "" }),
            );
        }
        assert!(seen > 30, "the sweep sees Settings' controls, saw {seen}");
        testing::set_rtl(false);
    }
}

// ---- The dialogs people open most ------------------------------------------------------------------------------

use crate::dialogs::{self, batch_rename, cleanup, compare_files, history, metadata, rules};
use crate::kit::prelude::{div, Context, IntoElement, Render, Styled as _, Window, WindowExt as _};
use std::sync::Arc;

/// An empty window with the dialog layer, for a dialog to open over. English or Arabic from the start.
struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn dialog_window(cx: &mut TestAppContext, rtl: bool) -> VisualTestContext {
    cx.update(testing::init);
    testing::set_rtl(rtl);
    testing::open_window(cx, |_, cx| cx.new(|_| Empty)).0
}

fn language(rtl: bool) -> &'static str {
    if rtl {
        " (Arabic)"
    } else {
        ""
    }
}

/// Sweeps the open dialog and checks one control that must be in it, so an empty or failed dialog can't pass.
#[track_caller]
fn sweep_dialog(cx: &mut VisualTestContext, name: &str, rtl: bool, must_have: &str) {
    settle(cx);
    let present = cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(must_have).is_some()
    });
    if !present {
        let labels: Vec<String> = cx.update(|window, _| {
            window
                .elements()
                .iter()
                .filter_map(|e| e.label().map(|l| format!("{:?} {l}", e.role())))
                .collect()
        });
        panic!(
            "{name}{}: {must_have:?} isn't on screen. On screen:
{}",
            language(rtl),
            labels.join(
                "
"
            )
        );
    }
    sweep(cx, &format!("{name}{}", language(rtl)), 3);
}

#[gpui_kit::test]
fn batch_rename_has_every_control_named(cx: &mut TestAppContext) {
    use batch_rename::{Item, Source};
    let _guard = testing::lock();
    for rtl in [false, true] {
        let mut visual = dialog_window(cx, rtl);
        let items: Vec<Item> = ["a.txt", "b.txt", "تقرير.txt"]
            .iter()
            .map(|name| Item {
                path: format!("C:\\Work\\{name}"),
                name: (*name).to_string(),
                dir: false,
                modified: 1_700_000_000_000,
            })
            .collect();
        let source = Source {
            rename: Arc::new(|_| Box::pin(async { Err("no renaming in a test".to_string()) })),
            details: Arc::new(|paths| Box::pin(async move { Ok(paths.iter().map(|_| None).collect()) })),
        };
        visual.update(|window, cx| {
            batch_rename::open_with(window, cx, items, Default::default(), source);
        });
        sweep_dialog(&mut visual, "Rename many", rtl, &crate::t!("Cancel"));
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn the_history_dialog_has_every_control_named(cx: &mut TestAppContext) {
    use history::{HistoryAnswer, HistoryEvent, Kind, Source};
    let _guard = testing::lock();
    for rtl in [false, true] {
        let mut visual = dialog_window(cx, rtl);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as i64);
        let events: Vec<HistoryEvent> = [
            (Kind::Renamed, "C:\\Work\\old.txt", Some("C:\\Work\\new.txt")),
            (Kind::Recycled, "C:\\Work\\gone.txt", None),
            (Kind::Deleted, "C:\\Work\\تقرير.txt", None),
            (Kind::Moved, "C:\\Work\\moved.txt", Some("C:\\Other\\moved.txt")),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (kind, path, to))| HistoryEvent {
            time: now - 60_000 * (i as i64 + 1),
            kind,
            dir: false,
            path: path.to_string(),
            to: to.map(str::to_string),
            was: None,
            now: to.map(str::to_string),
            items: 0,
            burst: false,
        })
        .collect();
        let answer = HistoryAnswer {
            events,
            limited: false,
            oldest: now - 600_000,
            elapsed: 3.0,
        };
        let source = Source {
            answer: Arc::new(move |_, _, _| {
                let answer = answer.clone();
                Box::pin(async move { Ok(answer) })
            }),
            enable: Arc::new(|| Box::pin(async { Ok(()) })),
        };
        visual.update(|window, cx| {
            history::open_with(window, cx, source, "");
        });
        sweep_dialog(&mut visual, "History", rtl, &crate::t!("Moved and deleted"));
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn the_cleanup_dialog_has_every_control_named(cx: &mut TestAppContext) {
    use cleanup::{Kind, Progress, Scan, Source, ToolState};
    let _guard = testing::lock();
    for rtl in [false, true] {
        let mut visual = dialog_window(cx, rtl);
        let found = |path: &str, size: u64, kind: &str| glint_core::cleanup::Found {
            path: path.to_string(),
            size,
            date: 1_700_000_000_000,
            kind: kind.to_string(),
            ..Default::default()
        };
        let scan = Scan {
            items: vec![
                found("C:\\Work\\app\\target", 5 << 30, "Rust builds"),
                found("C:\\Work\\web\\node_modules", 2 << 30, "Node modules"),
                found("C:\\Work\\تقرير\\target", 1 << 20, "Rust builds"),
            ],
            total: (7 << 30) + (1 << 20),
            engine: "fd + pdu".to_string(),
            elapsed: 12.0,
            more: 0,
            held: 0,
        };
        let source = Source {
            scan: Arc::new(move |_, _, _| {
                let scan = scan.clone();
                Box::pin(async move { Ok(scan) })
            }),
            cancel: Arc::new(|_| {}),
            rename: Arc::new(|_| Box::pin(async { Err("no renaming in a test".to_string()) })),
            missing: Arc::new(|_| Box::pin(async { Ok(Vec::new()) })),
            tools: Arc::new(|| {
                Box::pin(async {
                    Ok(vec![ToolState {
                        id: "czkawka".into(),
                        name: "Czkawka".into(),
                        installed: false,
                        source: "winget".into(),
                        pinned: "1.0".into(),
                    }])
                })
            }),
            install: Arc::new(|_, _: async_channel::Sender<Progress>| Box::pin(async { Ok(()) })),
            cancel_install: Arc::new(|_| {}),
        };
        visual.update(|window, cx| {
            cleanup::open_with(window, cx, "C:\\Work", Some(Kind::Junk), source);
        });
        sweep_dialog(&mut visual, "Clean up", rtl, &crate::t!("Recycle"));
        testing::set_rtl(false);
    }
}

/// A rules backend that knows nothing and does nothing.
struct NoRules;

impl rules::Backend for NoRules {
    fn preview(&self, _: &glint_model::store::FolderRule) -> rules::Fut<Vec<glint_core::rules::Done>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn apply(&self, _: &glint_model::store::FolderRule) -> rules::Fut<(usize, usize)> {
        Box::pin(async { Ok((0, 0)) })
    }
    fn log(&self) -> rules::Fut<Vec<glint_core::rules::Done>> {
        Box::pin(async {
            Ok(vec![glint_core::rules::Done {
                id: 1,
                at: 1_700_000_000_000,
                rule: "PDFs".into(),
                action: "move".into(),
                from: "C:\\Downloads\\a.pdf".into(),
                to: Some("D:\\Docs\\a.pdf".into()),
                mark: None,
                error: None,
                undone: false,
                summary: None,
            }])
        })
    }
    fn clear_log(&self) -> rules::Fut<()> {
        Box::pin(async { Ok(()) })
    }
    fn undo(&self, _: u64) -> rules::Fut<()> {
        Box::pin(async { Ok(()) })
    }
    fn set_rules(&self, _: Vec<glint_model::store::FolderRule>) -> rules::Fut<()> {
        Box::pin(async { Ok(()) })
    }
    fn pick_folder(&self, _: String, _: Option<String>, _: isize) -> rules::Fut<Option<String>> {
        Box::pin(async { Ok(None) })
    }
}

/// The app's saved state in a temp file, holding a rule, so the rules dialog has something to list.
fn saved_rules(cx: &mut VisualTestContext, tag: &str) {
    let dir = std::env::temp_dir().join(format!("glint-a11y-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("state.json");
    let _ = std::fs::remove_file(&file);
    cx.update(|_, cx| {
        crate::persistence::init_at(cx, file);
        crate::persistence::update_app(cx, |app| {
            app.rules = vec![glint_model::store::FolderRule {
                id: "r1".into(),
                name: "PDFs".into(),
                folder: "C:\\Downloads".into(),
                names: "*.pdf".into(),
                target: "D:\\Docs\\{year}".into(),
                ..Default::default()
            }];
        });
    });
}

#[gpui_kit::test]
fn the_folder_rules_dialog_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    for rtl in [false, true] {
        let mut visual = dialog_window(cx, rtl);
        saved_rules(&mut visual, "rules");
        visual.update(|window, cx| rules::open_with(window, cx, Arc::new(NoRules), None));
        sweep_dialog(&mut visual, "Folder rules", rtl, &crate::t!("Folder rules"));
        // The list of rules, and a new rule's form.
        visual.update(|window, cx| window.close_all_dialogs(cx));
        visual.update(|window, cx| rules::open_with(window, cx, Arc::new(NoRules), Some("C:\\Downloads".to_string())));
        sweep_dialog(&mut visual, "Folder rules, a new rule", rtl, &crate::t!("Folder rules"));
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn comparing_two_files_has_every_control_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    for rtl in [false, true] {
        let files = Files::new(if rtl { "compare-ar" } else { "compare-en" });
        let _ = std::fs::write(files.dir.join("a.txt"), "one\ntwo\nthree\n");
        let _ = std::fs::write(files.dir.join("b.txt"), "one\n2\nthree\nfour\n");
        let mut visual = dialog_window(cx, rtl);
        let (left, right) = (
            compare_files::Side::new(files.join("a.txt")),
            compare_files::Side::new(files.join("b.txt")),
        );
        visual.update(|window, cx| {
            compare_files::open_with(
                window,
                cx,
                Arc::new(compare_files::CoreBackend),
                None,
                left,
                right,
                None,
            );
        });
        // The comparison is read on core's threads: wait for its rows (labelled "left | right").
        let rows_drawn = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window
                    .find_all_by_role(Role::Row)
                    .iter()
                    .filter(|row| row.label().is_some_and(|label| label.contains('|')))
                    .count()
            })
        };
        for _ in 0..400 {
            visual.run_until_parked();
            if rows_drawn(&mut visual) >= 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if rows_drawn(&mut visual) < 3 {
            let labels: Vec<String> = visual.update(|window, _| {
                window
                    .elements()
                    .iter()
                    .filter_map(|e| e.label().map(|l| format!("{:?} {l}", e.role())))
                    .collect()
            });
            panic!(
                "the comparison shows no rows. On screen:
{}",
                labels.join(
                    "
"
                )
            );
        }
        sweep(&mut visual, &format!("Compare files{}", language(rtl)), 3);
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn remove_personal_info_has_every_control_named(cx: &mut TestAppContext) {
    use glint_core::actions::{Output, Progress, Report};
    use glint_core::metadata::{MetaTask, Tag};
    struct Photo;
    impl metadata::Backend for Photo {
        fn read(&self, _: String) -> dialogs::action_run::Fut<Vec<Tag>> {
            Box::pin(async {
                Ok(vec![
                    Tag {
                        group: "GPS".into(),
                        name: "GPSLatitude".into(),
                        value: "51.50722".into(),
                        kind: "place",
                    },
                    Tag {
                        group: "EXIF".into(),
                        name: "Model".into(),
                        value: "Pixel 8".into(),
                        kind: "device",
                    },
                ])
            })
        }
        fn run(
            &self,
            _: Vec<String>,
            _: MetaTask,
            _: Output,
            _: glint_core::host::ipc::Channel<Progress>,
        ) -> dialogs::action_run::Fut<Report> {
            Box::pin(async { Err("nothing runs in a test".to_string()) })
        }
        fn cancel(&self) {}
        fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>> {
            None
        }
    }
    let _guard = testing::lock();
    for rtl in [false, true] {
        let mut visual = dialog_window(cx, rtl);
        visual.update(|window, cx| {
            metadata::open_with(
                window,
                cx,
                Arc::new(Photo),
                vec!["C:\\Photos\\a.jpg".to_string()],
                metadata::Mode::Scrub,
            );
        });
        settle(&mut visual);
        settle(&mut visual);
        sweep(&mut visual, &format!("Remove personal info{}", language(rtl)), 3);
        testing::set_rtl(false);
    }
}

#[gpui_kit::test]
fn a_searchs_results_with_lines_found_inside_files_and_the_disk_map_have_every_control_named(cx: &mut TestAppContext) {
    use crate::explorer::fake::{inside_event, inside_match};
    let _guard = testing::lock();
    let files = Files::new("modes");
    let (mut visual, _, disk) = open_workspace_on(cx, &files, false);
    disk.live_inside();
    // The one search: "notes" is a file's name, and (the test says) a word in a file of the folder below.
    visual.update(|_, cx| {
        explorer::explorer(cx).update(cx, |e, cx| {
            e.set_filter("notes", None, cx);
            e.run_find("notes", Default::default(), cx).detach();
        })
    });
    settle(&mut visual);
    let id = disk
        .inside_searches()
        .last()
        .map(|(_, request)| request.id.clone())
        .expect("the files' contents are searched too");
    let inside = files.join("src\\main.rs");
    for said in [
        glint_core::TextSaid::Matches(vec![inside_match(&inside, "// notes for later\n", 3, 3, 8)]),
        glint_core::TextSaid::Finished,
    ] {
        let event = inside_event(&id, said);
        visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.inside_heard(&event, cx)));
        settle(&mut visual);
    }
    // Both parts are there, each under its header, and the line under its file.
    let named = |visual: &mut VisualTestContext, label: &str| {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    };
    assert!(named(&mut visual, "Names") && named(&mut visual, "Inside files"));
    assert!(named(&mut visual, "Line 3: // notes for later"));
    sweep(&mut visual, "a search's results, by name and inside files", 20);
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.clear_find(cx)));
    settle(&mut visual);
    visual.dispatch_action(actions::DiskUsage);
    settle(&mut visual);
    sweep(&mut visual, "the disk usage map", 10);
    testing::set_rtl(false);
}
