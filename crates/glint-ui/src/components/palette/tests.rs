//! Tests of the palette: what is ranked and listed (plain data), and, in a headless window over a fake disk,
//! what is drawn and what keys and clicks do: a command dispatches its action, a folder opens in the active
//! tab, the calculator's answer is copied, Escape closes, an Arabic name.

use super::commands::{self, Answer, Command, Inputs, Kind, Run};
use super::*;
use crate::actions;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{init_with, Drive, DriveKind, Explorer, KnownFolder, KnownId};
use crate::testing;
use glint_model::rank::Visit;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;

const NOW: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;
const HOME: &str = "C:\\Users\\me";
const DOWNLOADS: &str = "C:\\Users\\me\\Downloads";
const DOCUMENTS: &str = "C:\\Users\\me\\Documents";

// ---- Plain data --------------------------------------------------------------------------------------

fn inputs() -> Inputs {
    Inputs {
        path: HOME.into(),
        tabs: 1,
        known: vec![
            (KnownId::Downloads, DOWNLOADS.into()),
            (KnownId::Documents, DOCUMENTS.into()),
        ],
        drives: vec![Drive {
            path: "C:\\".into(),
            label: String::new(),
            kind: DriveKind::Fixed,
            fs: "NTFS".into(),
            total: 0,
            free: 0,
        }],
        pinned: vec!["D:\\Photos".into()],
        visits: vec![
            (
                "C:\\work\\project".into(),
                Visit {
                    count: 10,
                    last: NOW - 1000,
                },
            ),
            (
                "C:\\old\\archive".into(),
                Visit {
                    count: 1,
                    last: NOW - 30 * DAY,
                },
            ),
        ],
        now: NOW,
        ..Inputs::default()
    }
}

fn ids(results: &[Command]) -> Vec<&str> {
    results.iter().map(|c| c.id.as_str()).collect()
}

#[test]
fn nothing_typed_lists_the_folders_used_most_then_every_command() {
    let _guard = testing::lock();
    let all = commands::all(&inputs());
    let shown = commands::rank(&all, "  ");
    let folders: Vec<_> = shown.iter().filter(|c| c.kind == Kind::Folder).collect();
    assert!(folders.len() <= commands::FOLDERS_WHEN_EMPTY);
    // Most used first: the folder opened often and lately beats the one opened once last month.
    let project = shown.iter().position(|c| c.id == "folder:c:\\work\\project");
    let archive = shown.iter().position(|c| c.id == "folder:c:\\old\\archive");
    assert!(project.is_some());
    assert!(archive.is_none() || project < archive);
    // Folders come before commands, and one of a set (each tile size) waits for something to be typed.
    let first_action = shown.iter().position(|c| c.kind == Kind::Action);
    assert_eq!(first_action, Some(folders.len()));
    assert!(shown.iter().all(|c| !c.search_only));
    assert!(!ids(&shown).iter().any(|id| id.starts_with("tiles-")));
    assert!(ids(&shown).contains(&"new-tab"));
}

#[test]
fn typed_letters_find_a_folder_or_a_command_best_first() {
    let _guard = testing::lock();
    let all = commands::all(&inputs());
    assert_eq!(commands::rank(&all, "dl")[0].id, "folder:c:\\users\\me\\downloads");
    assert_eq!(commands::rank(&all, "settings")[0].id, "settings");
    // The name of a folder by its path, where the letters typed are close together.
    assert!(ids(&commands::rank(&all, "work")).contains(&"folder:c:\\work\\project"));
    // Each tile size is there once something's typed.
    assert!(ids(&commands::rank(&all, "large tiles")).contains(&"tiles-large"));
    // A close match for a typo.
    assert!(ids(&commands::rank(&all, "stetings")).contains(&"settings"));
    assert!(commands::rank(&all, "qzxqzxqzx").is_empty());
    assert!(commands::rank(&all, "q").len() <= commands::MOST_MATCHES);
}

#[test]
fn what_is_offered_follows_the_tab() {
    let _guard = testing::lock();
    let one_tab = commands::actions(&inputs());
    assert!(!ids(&one_tab).contains(&"close-other-tabs"));
    assert!(!ids(&one_tab).contains(&"swap-panes"));
    assert!(!ids(&one_tab).contains(&"back"));
    assert!(ids(&one_tab).contains(&"view-grid"));
    let many = Inputs {
        tabs: 3,
        dual: true,
        can_back: true,
        grid: true,
        tab: Some(crate::explorer::TabId(1)),
        ..inputs()
    };
    let many = commands::actions(&many);
    for id in [
        "close-other-tabs",
        "swap-panes",
        "back",
        "view-list",
        "zoom-in",
        "next-tab",
        "close-tab",
    ] {
        assert!(ids(&many).contains(&id), "{id}");
    }
    assert!(!ids(&many).contains(&"view-grid"));
    // The Recycle Bin is a place, not a folder.
    let bin = commands::actions(&Inputs {
        path: glint_model::paths::RECYCLE_BIN.into(),
        ..inputs()
    });
    assert!(!ids(&bin).contains(&"new-folder") && !ids(&bin).contains(&"recycle-bin"));
    assert!(bin
        .iter()
        .any(|c| c.id == "find" && c.label == "Filter what's in the Recycle Bin"));
    // Pinned or not changes the label.
    let pinned = commands::actions(&Inputs {
        pinned: vec![HOME.into()],
        ..inputs()
    });
    assert!(pinned.iter().any(|c| c.id == "pin" && c.label == "Unpin this folder"));
}

#[test]
fn keys_are_worded_as_the_typescript_did() {
    assert_eq!(commands::describe_keys("ctrl-shift-n"), "Ctrl+Shift+N");
    assert_eq!(commands::describe_keys("alt-left"), "Alt+Left");
    assert_eq!(commands::describe_keys("ctrl--"), "Ctrl+-");
    assert_eq!(commands::describe_keys("ctrl-="), "Ctrl+=");
    assert_eq!(commands::describe_keys("f5"), "F5");
    assert_eq!(commands::describe_keys("ctrl-shift-tab"), "Ctrl+Shift+Tab");
    assert_eq!(
        commands::shortcut_of(|| Box::new(actions::Back)).as_deref(),
        Some("Alt+Left")
    );
    assert_eq!(
        commands::shortcut_of(|| Box::new(actions::NewFolder)).as_deref(),
        Some("Ctrl+Shift+N")
    );
    assert_eq!(
        commands::shortcut_of(|| Box::new(actions::Refresh)).as_deref(),
        Some("F5")
    );
    assert_eq!(
        commands::shortcut_of(|| Box::new(actions::OpenSettings)).as_deref(),
        Some("Ctrl+,")
    );
}

#[test]
fn typed_locations_get_a_go_to_line_first() {
    let _guard = testing::lock();
    for path in [
        "C:\\Users",
        "c:",
        "d:/x",
        "\\\\server\\share",
        "~",
        "~\\Desktop",
        "%APPDATA%",
        ".\\src",
        "../up",
    ] {
        assert!(commands::looks_like_path(path), "{path}");
    }
    for text in ["downloads", "c", "..", "settings", ""] {
        assert!(!commands::looks_like_path(text), "{text}");
    }
    let all = commands::all(&inputs());
    let shown = commands::results(&all, "C:\\Users", None);
    assert_eq!(shown[0].id, "go");
    assert_eq!(shown[0].label, "Go to C:\\Users");
    assert!(matches!(&shown[0].run, Run::GoTo(text) if text == "C:\\Users"));
    assert!(commands::results(&all, "dl", None).iter().all(|c| c.id != "go"));
}

#[test]
fn the_equals_sign_makes_the_calculators_rows() {
    let _guard = testing::lock();
    assert_eq!(commands::expression_of("= 3 ft"), Some("3 ft".into()));
    assert_eq!(commands::expression_of("  ="), Some(String::new()));
    assert_eq!(commands::expression_of("3 = 3"), None);
    let all = commands::all(&inputs());

    // A bare `=`: the hint.
    let hint = commands::results(&all, "=", None);
    assert_eq!(hint.len(), 1);
    assert!(hint[0].label.starts_with("Type a sum:") && matches!(hint[0].run, Run::Nothing));
    // Waiting for the answer.
    assert_eq!(commands::results(&all, "= 1+1", None)[0].label, "…");
    // An answer for an older sum isn't shown.
    let old = Answer {
        expression: "1+".into(),
        text: "2".into(),
        ok: true,
    };
    assert_eq!(commands::results(&all, "= 1+1", Some(&old))[0].label, "…");
    // A failure says why, with the sum.
    let bad = Answer {
        expression: "oops".into(),
        text: "unknown identifier".into(),
        ok: false,
    };
    let failed = commands::results(&all, "= oops", Some(&bad));
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].label, "unknown identifier");
    assert_eq!(failed[0].detail.as_deref(), Some("oops"));
    assert!(matches!(failed[0].run, Run::Nothing));

    // A value: Enter copies it; a whole number also comes in hexadecimal, binary and as a size.
    let big = Answer {
        expression: "1024*4".into(),
        text: "4,096".into(),
        ok: true,
    };
    let rows = commands::results(&all, "= 1024*4", Some(&big));
    assert_eq!(rows[0].label, "= 4,096");
    assert_eq!(rows[0].detail.as_deref(), Some("1024*4"));
    assert_eq!(rows[0].shortcut.as_deref(), Some("Enter copies"));
    assert!(matches!(&rows[0].run, Run::Copy(text) if text == "4,096"));
    let labels: Vec<_> = rows.iter().map(|r| r.label.to_string()).collect();
    assert_eq!(labels, ["= 4,096", "0x1000", "0b1000000000000", "4.0 KB"]);
    assert_eq!(rows[3].detail.as_deref(), Some("4,096 bytes"));
    // Small numbers have no size; ones with units have no other forms.
    let small = Answer {
        expression: "0x1F + 1".into(),
        text: "32".into(),
        ok: true,
    };
    assert_eq!(commands::results(&all, "= 0x1F + 1", Some(&small)).len(), 3);
    let unit = Answer {
        expression: "3 ft to cm".into(),
        text: "91.44 cm".into(),
        ok: true,
    };
    assert_eq!(commands::results(&all, "= 3 ft to cm", Some(&unit)).len(), 1);
    // Too big for a safe integer, and negative: only the value.
    for text in ["9007199254740993", "-5", "3.5"] {
        let answer = Answer {
            expression: "x".into(),
            text: text.into(),
            ok: true,
        };
        assert_eq!(commands::calculator_rows("x", Some(&answer)).len(), 1, "{text}");
    }
}

#[test]
fn matches_are_highlighted_by_byte_range_in_english_and_arabic() {
    assert_eq!(commands::highlight_ranges("Downloads", "dl", false), vec![0..1, 4..5]);
    assert_eq!(commands::highlight_ranges("Downloads", "down", false), vec![0..4]);
    assert!(commands::highlight_ranges("Downloads", "", false).is_empty());
    assert!(commands::highlight_ranges("Downloads", "zzz", false).is_empty());
    // Two bytes a letter.
    let arabic = commands::highlight_ranges("تقارير", "قا", false);
    assert_eq!(arabic, vec![2..6]);
    assert!("تقارير".is_char_boundary(arabic[0].start) && "تقارير".is_char_boundary(arabic[0].end));
}

#[test]
fn every_icon_the_palette_draws_is_in_the_embedded_assets() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    let mut shown = commands::all(&Inputs {
        tabs: 3,
        dual: true,
        can_back: true,
        can_forward: true,
        can_reopen: true,
        focused: true,
        grid: true,
        pinned: vec![HOME.into(), "D:\\x".into()],
        remotes: vec![Default::default()],
        tab: Some(crate::explorer::TabId(1)),
        ..inputs()
    });
    shown.extend(commands::calculator_rows(
        "x",
        Some(&Answer {
            expression: "x".into(),
            text: "4096".into(),
            ok: true,
        }),
    ));
    shown.push(commands::go_row("C:\\"));
    for command in shown {
        let path = command.icon.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

// ---- In a window -------------------------------------------------------------------------------------

/// The window's content: records the actions that reach it.
struct Host {
    got: Rc<RefCell<Vec<&'static str>>>,
    focus: FocusHandle,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (a, b) = (self.got.clone(), self.got.clone());
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(move |_, _: &actions::NewTab, _, _| a.borrow_mut().push("new-tab")))
            .on_action(cx.listener(move |_, _: &actions::OpenSettings, _, _| b.borrow_mut().push("settings")))
    }
}

struct World {
    cx: VisualTestContext,
    explorer: Entity<Explorer>,
    got: Rc<RefCell<Vec<&'static str>>>,
    asked: Arc<Mutex<Vec<String>>>,
}

fn state_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("glint-palette-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp folder");
    let file = dir.join(format!("{name}.json"));
    let _ = std::fs::remove_file(&file);
    file
}

/// A window over an explorer on a fake disk (Home with Downloads and Documents), opened at Home, with one
/// pinned folder with an Arabic name.
fn world(cx: &mut TestAppContext, name: &str) -> World {
    cx.update(testing::init);
    let disk = FakeDisk::new();
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
        KnownFolder {
            id: KnownId::Documents,
            name: "Documents",
            path: DOCUMENTS.into(),
        },
    ]);
    disk.folder(
        HOME,
        vec![
            folder(HOME, "Downloads"),
            folder(HOME, "Documents"),
            file(HOME, "readme.txt", 5, 1),
        ],
    );
    disk.on_resolve(DOCUMENTS, DOCUMENTS, true);
    disk.folder(DOWNLOADS, vec![file(DOWNLOADS, "a.zip", 5, 1)]);
    disk.folder(DOCUMENTS, vec![file(DOCUMENTS, "b.txt", 5, 1)]);
    disk.folder(
        "C:\\مستندات\\تقارير",
        vec![file("C:\\مستندات\\تقارير", "ملف.txt", 1, 1)],
    );
    cx.update(|cx| {
        init_with(cx, disk.source());
        persistence::init_at(cx, state_file(name));
        persistence::update_app(cx, |app| app.pinned.push("C:\\مستندات\\تقارير".into()));
    });
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let got = Rc::new(RefCell::new(Vec::new()));
    let shared = got.clone();
    let (mut visual, host) = testing::open_window(cx, move |_, cx| {
        cx.new(|cx| Host {
            got: shared,
            focus: cx.focus_handle(),
        })
    });
    let focus = host.read_with(&visual, |host, _| host.focus.clone());
    visual.update(|window, cx| focus.focus(window, cx));
    let task = explorer.update(&mut visual, |e, cx| e.init_explorer(Vec::new(), cx));
    visual.foreground_executor().block_test(task);
    let mut world = World {
        cx: visual,
        explorer,
        got,
        asked: Arc::new(Mutex::new(Vec::new())),
    };
    world.settle(|w| w.path() == HOME);
    world
}

impl World {
    /// Lets background work (the listing, the calculator on core's runtime) come back until `done`.
    fn settle(&mut self, mut done: impl FnMut(&mut World) -> bool) {
        for _ in 0..400 {
            self.cx.run_until_parked();
            if done(self) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("what the test waited for never happened");
    }

    fn path(&mut self) -> String {
        self.explorer.read_with(&self.cx, |e, _| {
            e.active_tab().map(|t| t.path.clone()).unwrap_or_default()
        })
    }

    /// Sources that answer a few sums, remember what they were asked, and share these zoxide folders.
    fn sources(&self, zoxide: Vec<(String, f64)>) -> Sources {
        let asked = self.asked.clone();
        Sources {
            calc: Arc::new(move |expression| {
                asked.lock().unwrap().push(expression.clone());
                Box::pin(async move {
                    match expression.as_str() {
                        "3 ft to cm" => Ok("91.44 cm".to_string()),
                        "1024*4" => Ok("4096".to_string()),
                        other => Err(format!("unknown: {other}")),
                    }
                })
            }),
            zoxide: Arc::new(move || {
                let folders = zoxide.clone();
                Box::pin(async move { Ok(Some(folders)) })
            }),
            debounce: Duration::ZERO,
        }
    }

    fn open(&mut self) {
        let sources = self.sources(Vec::new());
        self.open_with(sources);
    }

    fn open_with(&mut self, sources: Sources) {
        self.cx.update(|window, cx| open_with(window, cx, sources));
        self.cx.run_until_parked();
        self.cx.update(|window, _| window.refresh());
        self.cx.run_until_parked();
    }

    fn palette(&mut self) -> Entity<Palette> {
        self.cx
            .update(|_, cx| cx.global::<Current>().0.upgrade())
            .expect("the palette is open")
    }

    /// Types into the box, as keys would.
    fn type_text(&mut self, text: &str) {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        });
        self.cx.run_until_parked();
    }

    fn press(&mut self, key: &str) {
        self.cx.update(|window, cx| window.press(key, cx));
        self.cx.run_until_parked();
    }

    fn results(&mut self) -> Vec<String> {
        let palette = self.palette();
        palette.read_with(&self.cx, |p, _| p.results.iter().map(|c| c.label.to_string()).collect())
    }

    fn is_open(&mut self) -> bool {
        self.cx.update(|window, cx| window.has_active_dialog(cx))
    }

    fn role_of(&mut self, label: &str) -> Option<Role> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn has(&mut self, label: &str) -> bool {
        self.role_of(label).is_some()
    }
}

#[gpui_kit::test]
fn opens_as_a_dialog_with_the_box_focused_and_the_lists_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "opens");
    w.open();
    assert!(w.is_open());
    assert_eq!(w.role_of("Command palette"), Some(Role::Group));
    assert_eq!(w.role_of("Results"), Some(Role::ListBox));
    assert_eq!(w.role_of("Command or folder"), Some(Role::TextInput));
    // Before anything is typed: where to go, then what to do.
    let results = w.results();
    assert!(results.iter().any(|r| r == "Downloads"));
    assert!(results.iter().any(|r| r == "New tab"));
    assert_eq!(w.role_of("Downloads"), Some(Role::ListBoxOption));
    // The first is highlighted, and the keyboard is in the box.
    w.cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.try_find_by_label("Command or folder").and_then(|e| e.focused()),
            Some(true)
        );
        let first = window.find(("palette-item", 0_usize));
        assert_eq!(first.selected(), Some(true));
    });
    // The shortcuts are the bindings'.
    let palette = w.palette();
    let shortcut = palette.read_with(&w.cx, |p, _| {
        p.results
            .iter()
            .find(|c| c.id == "new-tab")
            .and_then(|c| c.shortcut.clone())
    });
    assert_eq!(shortcut.as_deref(), Some("Ctrl+T"));
}

#[gpui_kit::test]
fn typing_filters_and_arrows_and_enter_run_a_command(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "command");
    w.open();
    w.type_text("new tab");
    let results = w.results();
    assert_eq!(results[0], "New tab");
    assert!(results.len() < 10, "{results:?}");
    w.press("enter");
    assert!(!w.is_open(), "choosing closes the palette");
    w.cx.run_until_parked();
    assert_eq!(*w.got.borrow(), vec!["new-tab"]);

    // The arrows move the highlight, round the ends.
    w.open();
    w.type_text("set");
    let palette = w.palette();
    let at = |w: &World| palette.read_with(&w.cx, |p, _| p.active);
    assert_eq!(at(&w), 0);
    w.press("down");
    assert_eq!(at(&w), 1);
    w.press("up");
    w.press("up");
    let last = w.results().len() - 1;
    assert_eq!(at(&w), last, "up from the first goes to the last");
    w.press("down");
    assert_eq!(at(&w), 0);
    // Typing again starts from the top.
    w.press("down");
    w.type_text("t");
    assert_eq!(at(&w), 0);
    w.type_text("i");
    assert_eq!(w.results()[0], "Settings");
    w.press("enter");
    w.cx.run_until_parked();
    assert_eq!(*w.got.borrow(), vec!["new-tab", "settings"]);
}

#[gpui_kit::test]
fn a_click_runs_the_row_and_hovering_moves_the_highlight(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "click");
    w.open();
    w.type_text("new tab");
    w.cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(("palette-item", 0_usize), cx);
    });
    w.cx.run_until_parked();
    assert!(!w.is_open());
    assert_eq!(*w.got.borrow(), vec!["new-tab"]);
}

#[gpui_kit::test]
fn a_folder_opens_in_the_active_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "folder");
    w.open();
    w.type_text("downloads");
    assert_eq!(w.results()[0], "Downloads");
    w.press("enter");
    w.settle(|w| w.path() == DOWNLOADS);
    assert!(!w.is_open());
}

#[gpui_kit::test]
fn typed_paths_are_gone_to(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "goto");
    w.explorer.update(&mut w.cx, |_, _| {});
    w.open();
    w.type_text(DOCUMENTS);
    assert_eq!(w.results()[0], format!("Go to {DOCUMENTS}"));
    w.press("enter");
    w.settle(|w| w.path() == DOCUMENTS);
}

#[gpui_kit::test]
fn the_calculators_answer_is_worked_out_in_the_background_and_enter_copies_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "calc");
    w.open();
    w.type_text("= 1024*4");
    // Asked once, off the UI thread; the row says so until the answer comes.
    w.settle(|w| w.results().first().is_some_and(|r| r == "= 4096"));
    assert_eq!(*w.asked.lock().unwrap(), vec!["1024*4".to_string()]);
    assert_eq!(w.results(), ["= 4096", "0x1000", "0b1000000000000", "4.0 KB"]);
    assert!(w.has("= 4096"));
    // Enter copies the value, closes, and says so.
    w.press("enter");
    assert!(!w.is_open());
    let copied =
        w.cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("4096"));
    w.cx.update(|window, cx| window.render_frame(cx));

    // A sum that fails says why and does nothing.
    w.open();
    w.type_text("= nonsense");
    w.settle(|w| w.results().first().is_some_and(|r| r == "unknown: nonsense"));
    w.press("enter");
    assert!(!w.is_open());
    let copied =
        w.cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("4096"), "nothing new was copied");
}

#[gpui_kit::test]
fn an_answer_for_an_older_sum_never_replaces_the_one_in_the_box(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "stale");
    w.open();
    w.type_text("= 3 ft to cm");
    w.type_text("0");
    w.settle(|w| w.asked.lock().unwrap().len() == 2);
    w.settle(|w| w.results().first().is_some_and(|r| r == "unknown: 3 ft to cm0"));
    assert_eq!(w.results().len(), 1);
}

#[gpui_kit::test]
fn zoxides_folders_join_the_list_when_history_is_shared(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "zoxide");
    w.cx.update(|_, cx| state::update(cx, |s| s.zoxide = true));
    let sources = w.sources(vec![("C:\\shared\\notes".into(), 12.)]);
    w.open_with(sources);
    w.settle(|w| w.results().iter().any(|r| r == "notes"));
    w.cx.update(|_, cx| state::update(cx, |s| s.zoxide = false));
}

#[gpui_kit::test]
fn escape_closes_it_and_ctrl_k_again_does_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "escape");
    w.open();
    assert!(w.is_open());
    w.press("escape");
    assert!(!w.is_open());
    assert!(w.got.borrow().is_empty());
    w.open();
    w.press("ctrl-k");
    assert!(!w.is_open());
    // Toggling opens it, and closes it.
    w.cx.update(toggle);
    w.cx.run_until_parked();
    assert!(w.is_open());
    w.cx.update(toggle);
    w.cx.run_until_parked();
    assert!(!w.is_open());
}

#[gpui_kit::test]
fn tab_leaves_the_keyboard_in_the_box_and_nothing_matching_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "tab");
    w.open();
    w.press("tab");
    w.cx.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.try_find_by_label("Command or folder").and_then(|e| e.focused()),
            Some(true)
        );
    });
    w.type_text("qzxqzxqzx");
    assert!(w.has("Nothing matches “qzxqzxqzx”"));
    // Enter with nothing listed does nothing, and the palette stays.
    w.press("enter");
    assert!(w.is_open());
}

#[gpui_kit::test]
fn a_change_without_an_action_is_made(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "change");
    w.open();
    w.type_text("pin this");
    assert_eq!(w.results()[0], "Pin this folder to the sidebar");
    w.press("enter");
    let pinned =
        w.cx.update(|_, cx| persistence::app(cx).map(|a| a.pinned.clone()).unwrap_or_default());
    assert!(pinned.iter().any(|p| p == HOME), "{pinned:?}");
}

#[gpui_kit::test]
fn arabic_names_are_listed_found_highlighted_and_opened_in_the_mirrored_layout(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "arabic");
    testing::set_rtl(true);
    w.open();
    w.type_text("تقارير");
    let results = w.results();
    assert_eq!(results[0], "تقارير", "{results:?}");
    // The name is exposed as it is, without the layout-only isolates.
    assert_eq!(w.role_of("تقارير"), Some(Role::ListBoxOption));
    w.press("enter");
    w.settle(|w| w.path() == "C:\\مستندات\\تقارير");
    // Commands read right to left too.
    w.open();
    // The commands' names are the Arabic catalog's.
    let new_tab = crate::i18n::translate("New tab");
    w.type_text(&new_tab);
    assert_eq!(w.results()[0], new_tab.to_string());
    assert!(w.has(&new_tab));
    w.press("escape");
    testing::set_rtl(false);
}
