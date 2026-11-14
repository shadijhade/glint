//! Headless tests of the terminal pane: tabs, following the folder, keys to the shell, output on the screen, with
//! a fake backend (no shell is started) and a fake disk behind the explorer. The real shell is in the `#[ignore]`d
//! test at the bottom.

use super::session::fake::{self, Log};
use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::testing;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{ClipboardItem, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::sync::Arc;

/// A pane in a window, the fake shell behind it, and the disk behind the explorer.
struct Env {
    cx: VisualTestContext,
    pane: Entity<TerminalPane>,
    log: Arc<Log>,
    _disk: FakeDisk,
    events: Rc<RefCell<Vec<TerminalPaneEvent>>>,
    _subscription: Subscription,
}

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

fn open(cx: &mut TestAppContext, shells: &[(&str, &str)]) -> Env {
    cx.update(testing::init);
    let disk = disk();
    let (backend, log) = fake::backend(shells);
    cx.update(|cx| {
        crate::explorer::init_with(cx, disk.source());
        session::init_with(cx, backend);
    });
    testing::set_rtl(false);
    let (mut cx, pane) = testing::open_window(cx, |window, cx| cx.new(|cx| TerminalPane::new(window, cx)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let seen = events.clone();
    let subscription = cx.update(|_, cx| {
        cx.subscribe(&pane, move |_, event: &TerminalPaneEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
    });
    // The explorer opens C:\ first.
    cx.update(|_, cx| {
        let explorer = crate::explorer::explorer(cx);
        explorer.update(cx, |explorer, cx| explorer.navigate("C:\\", NavOptions::default(), cx));
    });
    cx.run_until_parked();
    Env {
        cx,
        pane,
        log,
        _disk: disk,
        events,
        _subscription: subscription,
    }
}

/// Draws frames and lets background work come back until `done`.
fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    for _ in 0..600 {
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
        // The view redraws no oftener than a frame; the fake clock moves on.
        cx.executor().advance_clock(std::time::Duration::from_millis(40));
        if done(cx) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    panic!("the terminal never got there");
}

impl Env {
    fn view(&mut self) -> Entity<TerminalView> {
        let pane = self.pane.clone();
        self.cx
            .update(|_, cx| pane.read(cx).active_view().expect("a terminal is open"))
    }

    fn text(&mut self) -> Vec<String> {
        let view = self.view();
        self.cx.update(|_, cx| view.read(cx).visible_text())
    }

    /// Opens a terminal and waits until its shell has been started.
    fn start(&mut self) {
        let pane = self.pane.clone();
        self.cx
            .update(|window, cx| pane.update(cx, |pane, cx| pane.toggle(window, cx)));
        let log = self.log.clone();
        settle(&mut self.cx, move |_| !log.spawned.lock().unwrap().is_empty());
        // The writer thread starts once the session has an id.
        let log = self.log.clone();
        settle(&mut self.cx, move |_| log.outputs.lock().unwrap().len() == 1);
    }

    fn print(&mut self, text: &str) {
        self.log.print(text);
    }

    fn press(&mut self, key: &str) {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.cx.run_until_parked();
    }

    fn input(&mut self, text: &str) {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        });
        self.cx.run_until_parked();
    }

    /// Waits until the fake shell has been written to with exactly this much text.
    fn wait_written(&mut self, expected: &str) {
        let log = self.log.clone();
        let expected = expected.to_string();
        settle(&mut self.cx, move |_| log.written() == expected);
    }

    fn shown_folder(&mut self) -> String {
        self.cx
            .update(|_, cx| crate::explorer::explorer(cx).read(cx).current_path().to_string())
    }

    fn goto(&mut self, path: &str) {
        let path = path.to_string();
        self.cx.update(|_, cx| {
            let explorer = crate::explorer::explorer(cx);
            explorer.update(cx, |explorer, cx| explorer.navigate(path, NavOptions::default(), cx));
        });
        self.cx.run_until_parked();
    }

    fn role_of(&mut self, label: &str) -> Option<Role> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }
}

const SHELLS: [(&str, &str); 3] = [("pwsh", "PowerShell"), ("cmd", "Command Prompt"), ("wsl", "WSL")];

#[gpui_kit::test]
fn the_first_toggle_starts_a_shell_in_the_folder_shown_at_the_size_that_fits(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    assert!(!env.cx.update(|_, cx| env.pane.read(cx).visible()));
    env.start();
    let spawned = env.log.spawned.lock().unwrap().clone();
    assert_eq!(spawned.len(), 1);
    let (shell, cwd, cols, rows) = &spawned[0];
    assert_eq!(
        (shell.as_str(), cwd.as_str()),
        ("pwsh", "C:\\"),
        "the first shell found, in the folder shown"
    );
    assert!(*cols >= 10 && *rows >= 2, "{cols}x{rows}");
    // The grid is the size the element measured.
    let view = env.view();
    let geometry = env
        .cx
        .update(|_, cx| view.read(cx).geometry())
        .expect("it has been drawn");
    assert_eq!((geometry.cols as u16, geometry.rows as u16), (*cols, *rows));
    assert!(env.cx.update(|_, cx| env.pane.read(cx).visible()));
}

#[gpui_kit::test]
fn what_the_shell_prints_is_on_the_screen_and_a_flood_redraws_once_a_frame(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.print("hello\r\nworld");
    settle(&mut env.cx, |_| true);
    assert_eq!(&env.text()[..2], ["hello", "world"]);
    // A hundred chunks are one wake until a frame has passed.
    for n in 0..100 {
        env.print(&format!("\r\nline {n}"));
    }
    let view = env.view();
    let screen = env.cx.update(|_, cx| view.read(cx).screen().clone());
    screen.with(|emu| assert!(emu.visible_text().iter().any(|row| row == "line 99")));
}

#[gpui_kit::test]
fn typed_text_and_keys_reach_the_shell_as_it_expects_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.input("ls");
    env.press("enter");
    env.wait_written("ls\r");
    // Arrows are the plain form until a program asks for application mode.
    env.press("up");
    env.wait_written("ls\r\x1b[A");
    env.print("\x1b[?1h");
    settle(&mut env.cx, |_| true);
    env.press("up");
    env.press("ctrl-right");
    env.press("backspace");
    env.press("tab");
    env.press("ctrl-l");
    env.wait_written("ls\r\x1b[A\x1bOA\x1b[1;5C\x7f\t\x0c");
    // Alt prefixes escape; Unicode text goes through unchanged.
    env.press("alt-b");
    env.input("مرحبا 漢");
    env.wait_written("ls\r\x1b[A\x1bOA\x1b[1;5C\x7f\t\x0c\x1bbمرحبا 漢");
}

#[gpui_kit::test]
fn ctrl_backtick_is_the_apps_and_everything_else_is_the_shells(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    let view = env.view();
    fn took(env: &mut Env, view: &Entity<TerminalView>, key: &'static str) -> bool {
        let view = view.clone();
        let keystroke = gpui_kit::Keystroke::parse(key).expect("a keystroke");
        env.cx
            .update(|window, cx| view.update(cx, |view, cx| view.key(&keystroke, window, cx)))
    }
    assert!(!took(&mut env, &view, "ctrl-`"), "the workspace toggles the pane");
    // Keys the app binds: the terminal takes them.
    for key in ["ctrl-k", "ctrl-w", "f5", "escape", "ctrl-shift-t", "alt-left"] {
        assert!(took(&mut env, &view, key), "{key}");
    }
    // Typed text is the input handler's; a lone modifier is nobody's.
    let letter = gpui_kit::Keystroke {
        key: "a".into(),
        key_char: Some("a".into()),
        modifiers: Default::default(),
    };
    let took_letter = env
        .cx
        .update(|window, cx| view.update(cx, |view, cx| view.key(&letter, window, cx)));
    assert!(!took_letter);
    assert!(!took(&mut env, &view, "shift"));
}

#[gpui_kit::test]
fn copy_paste_and_bracketed_paste(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.print("hello world");
    settle(&mut env.cx, |_| true);
    let view = env.view();
    // Ctrl+C with nothing selected is ^C.
    env.press("ctrl-c");
    env.wait_written("\x03");
    // With a selection it copies (and the shell hears nothing).
    let screen = env.cx.update(|_, cx| view.read(cx).screen().clone());
    screen.with(|emu| {
        emu.start_selection(emu::Pick::Cells, 0, 0, false);
        emu.update_selection(4, 0, true);
    });
    env.press("ctrl-c");
    let copied = env
        .cx
        .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("hello"));
    assert_eq!(env.log.written(), "\x03");
    assert!(
        screen.with(|emu| emu.selection_text()).is_none(),
        "copying clears the selection"
    );
    // Ctrl+V pastes with line breaks as Enter...
    env.cx
        .update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("one\ntwo".into())));
    env.press("ctrl-v");
    env.wait_written("\x03one\rtwo");
    // ...and bracketed when the program asked.
    env.print("\x1b[?2004h");
    settle(&mut env.cx, |_| true);
    env.press("ctrl-shift-v");
    env.wait_written("\x03one\rtwo\x1b[200~one\ntwo\x1b[201~");
}

#[gpui_kit::test]
fn the_pane_follows_the_folder_shown_and_the_shells_prompt(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    // Busy until its prompt has said where it is: no cd goes in.
    env.goto("C:\\a");
    assert_eq!(env.log.written(), "");
    // The first prompt: the pane went somewhere while the shell was busy, so now the shell follows.
    env.log.cwd("C:\\");
    env.wait_written("Set-Location -LiteralPath 'C:\\a'\r");
    // It is busy until the next prompt, so a move now does not pile up a second cd.
    env.goto("C:\\b");
    assert_eq!(env.log.written(), "Set-Location -LiteralPath 'C:\\a'\r");
    env.log.cwd("C:\\a");
    let view = env.view();
    // The pane was at C:\b while the shell went to C:\a: the explorer follows the shell.
    settle(&mut env.cx, |cx| {
        cx.update(|_, cx| crate::explorer::explorer(cx).read(cx).current_path() == "C:\\a")
    });
    settle(&mut env.cx, |cx| cx.update(|_, cx| view.read(cx).is_idle()));
    // At its prompt, a folder opened in the pane is cd'd to.
    env.goto("C:\\b");
    env.wait_written("Set-Location -LiteralPath 'C:\\a'\rSet-Location -LiteralPath 'C:\\b'\r");
    env.log.cwd("C:\\b");
    settle(&mut env.cx, |cx| cx.update(|_, cx| view.read(cx).is_idle()));
    // The shell went somewhere on its own: the explorer follows.
    env.log.cwd("C:\\a");
    settle(&mut env.cx, |cx| {
        cx.update(|_, cx| crate::explorer::explorer(cx).read(cx).current_path() == "C:\\a")
    });
}

#[gpui_kit::test]
fn following_can_be_switched_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.cx
        .update(|_, cx| state::update(cx, |settings| settings.terminal_follow = false));
    env.goto("C:\\a");
    env.log.cwd("C:\\");
    let view = env.view();
    settle(&mut env.cx, |cx| cx.update(|_, cx| view.read(cx).is_idle()));
    assert_eq!(env.log.written(), "", "not following: nothing is typed");
    env.log.cwd("C:\\b");
    settle(&mut env.cx, |_| true);
    assert_eq!(env.shown_folder(), "C:\\a", "nor does the pane move");
}

#[gpui_kit::test]
fn a_task_is_typed_at_the_first_prompt_and_the_tab_is_reused_when_it_is_back_at_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    // The pane shows the task's folder, so following has nothing to add.
    env.goto("C:\\a");
    let pane = env.pane.clone();
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.run_task("C:\\a", "npm run build", window, cx)));
    let log = env.log.clone();
    settle(&mut env.cx, move |_| !log.spawned.lock().unwrap().is_empty());
    let log = env.log.clone();
    settle(&mut env.cx, move |_| log.outputs.lock().unwrap().len() == 1);
    assert_eq!(env.log.spawned.lock().unwrap()[0].1, "C:\\a");
    // Nothing is typed before the prompt is there.
    assert_eq!(env.log.written(), "");
    env.log.cwd("C:\\a");
    env.wait_written("npm run build\r");
    // The tab is named after the task.
    assert_eq!(env.role_of("npm run build"), Some(Role::Tab));
    // Back at its prompt in the same folder: the same tab runs it again.
    env.log.cwd("C:\\a");
    let view = env.view();
    settle(&mut env.cx, |cx| cx.update(|_, cx| view.read(cx).is_idle()));
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.run_task("C:\\a", "npm run build", window, cx)));
    env.wait_written("npm run build\rnpm run build\r");
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).tab_count()), 1);
    // Busy: a third run opens another tab.
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.run_task("C:\\a", "npm run build", window, cx)));
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).tab_count()), 2);
}

#[gpui_kit::test]
fn a_script_job_is_typed_in_the_tabs_shell_quoting(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    let pane = env.pane.clone();
    let job = TerminalJob {
        cwd: "C:\\a".into(),
        title: "Clean".into(),
        program: "C:\\Tools\\clean it.ps1".into(),
        args: vec!["C:\\a\\one's.txt".into()],
    };
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.run_job(&job, window, cx)));
    let log = env.log.clone();
    settle(&mut env.cx, move |_| log.outputs.lock().unwrap().len() == 1);
    env.log.cwd("C:\\a");
    env.wait_written("& 'C:\\Tools\\clean it.ps1' 'C:\\a\\one''s.txt'\r");
}

#[gpui_kit::test]
fn closing_a_tab_ends_its_shell_and_the_last_one_hides_the_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    let pane = env.pane.clone();
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.open("C:\\b", window, cx)));
    let log = env.log.clone();
    settle(&mut env.cx, move |_| log.outputs.lock().unwrap().len() == 2);
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).tab_count()), 2);
    // The second one is on show; closing it shows the first.
    let key = env.cx.update(|_, cx| pane.read(cx).active.expect("a tab"));
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.close_tab(key, window, cx)));
    let log = env.log.clone();
    settle(&mut env.cx, move |_| log.closed.lock().unwrap().len() == 1);
    assert_eq!(*env.log.closed.lock().unwrap(), vec![2]);
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).tab_count()), 1);
    let key = env.cx.update(|_, cx| pane.read(cx).active.expect("a tab"));
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.close_tab(key, window, cx)));
    let log = env.log.clone();
    settle(&mut env.cx, move |_| log.closed.lock().unwrap().len() == 2);
    assert!(!env.cx.update(|_, cx| pane.read(cx).visible()));
    assert_eq!(*env.events.borrow(), vec![TerminalPaneEvent::Hidden]);
}

#[gpui_kit::test]
fn hiding_keeps_the_shells_and_showing_brings_them_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.print("kept");
    settle(&mut env.cx, |_| true);
    let pane = env.pane.clone();
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.toggle(window, cx)));
    assert!(!env.cx.update(|_, cx| pane.read(cx).is_open()));
    assert!(env.log.closed.lock().unwrap().is_empty(), "its shell keeps running");
    env.print(" going");
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.toggle(window, cx)));
    assert!(env.cx.update(|_, cx| pane.read(cx).is_open()));
    settle(&mut env.cx, |_| true);
    assert_eq!(env.text()[0], "kept going");
    assert_eq!(env.log.spawned.lock().unwrap().len(), 1, "one shell, not a new one");
}

#[gpui_kit::test]
fn a_shell_that_ends_says_so_dimmed_and_takes_no_more_keys(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.log.exit(3);
    settle(&mut env.cx, |env| env.update(|_, _| true));
    let view = env.view();
    settle(&mut env.cx, |cx| cx.update(|_, cx| view.read(cx).exited() == Some(3)));
    assert!(
        env.text().iter().any(|row| row == "[PowerShell ended with 3.]"),
        "{:?}",
        env.text()
    );
    env.input("x");
    assert_eq!(env.log.written(), "");
    // A clean end leaves out the code.
    let pane = env.pane.clone();
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.open("C:\\", window, cx)));
    let log = env.log.clone();
    settle(&mut env.cx, move |_| log.outputs.lock().unwrap().len() == 2);
    env.log.exit(0);
    let view = env.view();
    settle(&mut env.cx, |cx| cx.update(|_, cx| view.read(cx).exited() == Some(0)));
    assert!(env.text().iter().any(|row| row == "[PowerShell ended.]"));
}

#[gpui_kit::test]
fn a_shell_that_cannot_start_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.update(testing::init);
    let disk = disk();
    let (mut backend, _log) = fake::backend(&SHELLS);
    backend.spawn = Rc::new(|_| Box::pin(async { Err("pwsh isn't installed".to_string()) }));
    cx.update(|cx| {
        crate::explorer::init_with(cx, disk.source());
        session::init_with(cx, backend);
    });
    let (mut cx, pane) = testing::open_window(cx, |window, cx| cx.new(|cx| TerminalPane::new(window, cx)));
    cx.update(|window, cx| pane.update(cx, |pane, cx| pane.toggle(window, cx)));
    let view = cx.update(|_, cx| pane.read(cx).active_view().expect("a tab"));
    settle(&mut cx, |cx| {
        cx.update(|_, cx| {
            view.read(cx)
                .visible_text()
                .iter()
                .any(|row| row == "Couldn't start PowerShell: pwsh isn't installed")
        })
    });
}

#[gpui_kit::test]
fn the_pane_is_named_for_screen_readers_and_keyboard_only_use_works(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    assert_eq!(env.role_of("Terminals"), Some(Role::TabList));
    assert_eq!(env.role_of("New terminal here"), Some(Role::Button));
    assert_eq!(env.role_of("New terminal with another shell"), Some(Role::Button));
    assert_eq!(env.role_of("Close this terminal"), Some(Role::Button));
    assert_eq!(env.role_of("Follow"), Some(Role::Button));
    assert_eq!(
        env.role_of("Hide the terminal (Ctrl+`): its shells keep running"),
        Some(Role::Button)
    );
    assert_eq!(env.role_of("Terminal height"), Some(Role::Splitter));
    assert_eq!(env.role_of("Terminal"), Some(Role::Group));
    assert_eq!(env.role_of("PowerShell"), Some(Role::Terminal));
    // The tab is named for the folder, and marked selected.
    let pane = env.pane.clone();
    let name = env.cx.update(|_, cx| pane.read(cx).tabs[0].label());
    let tab = env.cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .try_find_by_label(&name)
            .map(|element| (element.role(), element.selected()))
    });
    assert_eq!(tab, Some((Some(Role::Tab), Some(true))));
    // The terminal holds the keyboard after it opens.
    let view = env.view();
    assert!(env
        .cx
        .update(|window, cx| view.read(cx).focus_handle().is_focused(window)));
}

#[gpui_kit::test]
fn one_shell_means_no_shell_picker(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &[("cmd", "Command Prompt")]);
    env.start();
    assert_eq!(env.role_of("New terminal with another shell"), None);
    let spawned = env.log.spawned.lock().unwrap().clone();
    assert_eq!(spawned[0].0, "cmd");
}

#[gpui_kit::test]
fn the_shell_chosen_in_settings_is_the_default_and_a_script_avoids_wsl(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.cx
        .update(|_, cx| state::update(cx, |settings| settings.terminal_shell = "wsl".into()));
    env.start();
    assert_eq!(env.log.spawned.lock().unwrap()[0].0, "wsl");
    let pane = env.pane.clone();
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.run_task("C:\\", "make", window, cx)));
    let log = env.log.clone();
    settle(&mut env.cx, move |_| log.spawned.lock().unwrap().len() == 2);
    assert_eq!(
        env.log.spawned.lock().unwrap()[1].0,
        "pwsh",
        "a task's tools are Windows', not WSL's"
    );
}

#[gpui_kit::test]
fn dragging_the_edge_sets_the_height_and_resizes_the_shell(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    let before = env.log.spawned.lock().unwrap()[0].3;
    let pane = env.pane.clone();
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).height(cx)), DEFAULT_HEIGHT);
    // The pointer 400 px above the bottom of the pane.
    env.cx.update(|window, cx| {
        pane.update(cx, |pane, cx| pane.drag_height(px(600.), px(200.), window, cx));
    });
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).height(cx)), 400.);
    let log = env.log.clone();
    settle(&mut env.cx, move |_| !log.resizes.lock().unwrap().is_empty());
    let resized = env.log.resizes.lock().unwrap().last().copied().expect("a resize");
    assert!(resized.2 > before, "taller: {} rows, was {before}", resized.2);
    // The height is saved once the pointer rests, and a double click on the edge puts it back.
    let saved = |env: &mut Env| env.cx.update(|_, cx| state::settings(cx).terminal_height);
    assert_eq!(saved(&mut env), None);
    settle(&mut env.cx, |cx| {
        cx.update(|_, cx| state::settings(cx).terminal_height == Some(400.))
    });
    env.cx.update(|_, cx| pane.update(cx, |pane, cx| pane.reset_height(cx)));
    assert_eq!(saved(&mut env), None);
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).height(cx)), DEFAULT_HEIGHT);
    // It never goes below its least.
    env.cx
        .update(|window, cx| pane.update(cx, |pane, cx| pane.drag_height(px(600.), px(590.), window, cx)));
    assert_eq!(env.cx.update(|_, cx| pane.read(cx).height(cx)), MIN_HEIGHT);
}

#[gpui_kit::test]
fn right_to_left_text_gets_a_note_that_can_be_dismissed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    let note = "Right-to-left text can look out of order here: a terminal draws one letter in each cell.";
    assert_eq!(env.role_of(note), None);
    env.print("مرحبا بالعالم");
    settle(&mut env.cx, |cx| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(note).is_some()
        })
    });
    assert_eq!(env.role_of(note), Some(Role::Status));
    assert_eq!(env.text()[0], "مرحبا بالعالم", "the text itself is untouched");
    env.cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("terminal-rtl-note-close", cx);
    });
    env.cx.run_until_parked();
    assert_eq!(env.role_of(note), None);
}

#[gpui_kit::test]
fn an_input_method_composes_at_the_cursor_and_commits_text(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    let view = env.view();
    env.cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.replace_and_mark_text_in_range(None, "ni", None, window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(0..2));
            view.replace_and_mark_text_in_range(None, "你", None, window, cx);
            assert_eq!(view.marked_text_range(window, cx), Some(0..1));
            assert!(
                view.bounds_for_range(0..1, Default::default(), window, cx).is_some(),
                "the candidate window goes under the cursor"
            );
        })
    });
    assert_eq!(env.log.written(), "", "nothing is sent while composing");
    env.cx.update(|window, cx| {
        view.update(cx, |view, cx| view.replace_text_in_range(None, "你好", window, cx));
    });
    env.wait_written("你好");
    assert!(env
        .cx
        .update(|window, cx| view.update(cx, |view, cx| view.marked_text_range(window, cx)))
        .is_none());
}

#[gpui_kit::test]
fn ctrl_click_opens_the_link_under_the_pointer_and_a_path_goes_to_the_explorer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.print("see https://example.com/x and C:\\a\\one.txt now");
    settle(&mut env.cx, |_| true);
    let view = env.view();
    let opened = Rc::new(RefCell::new(Vec::new()));
    let seen = opened.clone();
    let _sub = env.cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &TerminalEvent, _| {
            seen.borrow_mut().push(event.clone())
        })
    });
    let geometry = env.cx.update(|_, cx| view.read(cx).geometry()).expect("drawn");
    let at_col = |col: usize| geometry.cell_bounds(col, 0, 1).center();
    // Hover with Ctrl lights the link; the path's press emits the event for the pane.
    let ctrl = gpui_kit::Modifiers {
        control: true,
        ..Default::default()
    };
    env.cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let down = |position| gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position,
                modifiers: ctrl,
                click_count: 1,
                first_mouse: false,
            };
            view.mouse_down(&down(at_col(35)), window, cx);
        })
    });
    assert_eq!(*opened.borrow(), vec![TerminalEvent::OpenPath("C:\\a\\one.txt".into())]);
    // Without Ctrl a press starts a selection instead.
    env.cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let down = gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position: at_col(4),
                modifiers: Default::default(),
                click_count: 2,
                first_mouse: false,
            };
            view.mouse_down(&down, window, cx);
        })
    });
    let word = env
        .cx
        .update(|_, cx| view.read(cx).screen().with(|emu| emu.selection_text()));
    // A double click selects up to the characters that end a word, and a colon is one.
    assert_eq!(word.as_deref(), Some("https"));
}

#[gpui_kit::test]
fn the_wheel_scrolls_the_scrollback_and_shift_page_up_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    for n in 0..200 {
        env.print(&format!("row {n}\r\n"));
    }
    settle(&mut env.cx, |_| true);
    let view = env.view();
    let screen = env.cx.update(|_, cx| view.read(cx).screen().clone());
    assert_eq!(screen.with(|emu| emu.scroll_position().0), 0);
    let geometry = env.cx.update(|_, cx| view.read(cx).geometry()).expect("drawn");
    env.cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let wheel = gpui_kit::ScrollEvent {
                position: geometry.bounds.center(),
                delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., 3.)),
                modifiers: Default::default(),
                touch_phase: Default::default(),
                source: Default::default(),
            };
            view.wheel(&wheel, window, cx);
        })
    });
    assert_eq!(screen.with(|emu| emu.scroll_position().0), 3);
    env.press("shift-pageup");
    assert!(screen.with(|emu| emu.scroll_position().0) > 3);
    // Typing brings the view back to the bottom.
    env.input("x");
    assert_eq!(screen.with(|emu| emu.scroll_position().0), 0);
    // Shift+PageUp is the shell's in a full-screen program.
    env.print("\x1b[?1049h");
    settle(&mut env.cx, |_| true);
    env.press("shift-pageup");
    env.wait_written("x\x1b[5;2~");
}

#[gpui_kit::test]
fn the_wheel_in_a_program_that_asked_for_the_mouse_is_reported_to_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.start();
    env.print("\x1b[?1000h\x1b[?1006h");
    settle(&mut env.cx, |_| true);
    let view = env.view();
    let geometry = env.cx.update(|_, cx| view.read(cx).geometry()).expect("drawn");
    env.cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            let wheel = gpui_kit::ScrollEvent {
                position: geometry.cell_bounds(2, 1, 1).center(),
                delta: gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., 1.)),
                modifiers: Default::default(),
                touch_phase: Default::default(),
                source: Default::default(),
            };
            view.wheel(&wheel, window, cx);
            let down = gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position: geometry.cell_bounds(2, 1, 1).center(),
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            };
            view.mouse_down(&down, window, cx);
        })
    });
    env.wait_written("\x1b[<64;3;2M\x1b[<0;3;2M");
}

#[gpui_kit::test]
fn the_colors_follow_the_theme_in_light_and_dark(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = open(cx, &SHELLS);
    env.cx
        .update(|_, cx| state::update(cx, |settings| settings.theme = glint_model::store::Theme::Dark));
    env.start();
    env.print("\x1b[31mred\x1b[0m");
    settle(&mut env.cx, |_| true);
    let view = env.view();
    let palette = |env: &mut Env| {
        env.cx
            .update(|_, cx| view.read(cx).screen().with(|emu| emu.snapshot().rows[0].runs[0].fg))
    };
    let dark = palette(&mut env);
    env.cx
        .update(|_, cx| state::update(cx, |settings| settings.theme = glint_model::store::Theme::Light));
    settle(&mut env.cx, |_| true);
    let light = palette(&mut env);
    assert_eq!(dark, 0xc50f1f, "Campbell's red");
    assert_eq!(light, 0xe45649, "One Half Light's red");
}

#[gpui_kit::test]
fn the_palette_comes_from_the_themes_tokens(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.update(testing::init);
    cx.update(|cx| {
        let tokens = theme::tokens(cx).clone();
        let palette = palette_of(&tokens);
        assert_eq!(palette.dark, tokens.dark);
        // The card is the background, as `terminalTheme()` has it; the text color is the foreground.
        let rgb = |hsla: gpui_kit::Hsla| {
            let c: gpui_kit::Rgba = hsla.into();
            ((c.r * 255.).round() as u32) << 16 | ((c.g * 255.).round() as u32) << 8 | (c.b * 255.).round() as u32
        };
        assert_eq!(palette.background, rgb(tokens.card));
        assert_eq!(palette.foreground, rgb(tokens.foreground));
        assert_eq!(palette.cursor, rgb(tokens.primary));
    });
}

#[test]
fn the_icons_the_pane_draws_are_embedded() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    for name in [
        IconName::SquareTerminal,
        IconName::Plus,
        IconName::X,
        IconName::ChevronDown,
        IconName::Link2,
        IconName::Info,
        IconName::Play,
        IconName::Pause,
        IconName::RotateCcw,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

/// The real thing: a command prompt in a pseudo console through glint-core, its output parsed into the grid. Slow
/// and it starts a process, so it only runs when asked (`cargo test -- --ignored`).
#[test]
#[ignore = "starts cmd.exe in a pseudo console"]
fn a_real_command_prompt_runs_and_its_output_lands_on_the_screen() {
    use glint_core::host::async_runtime::block_on;
    let (tx, rx) = async_channel::unbounded();
    let waker = session::Waker::new(tx.clone());
    let wake = waker.clone();
    let hooks = emu::Hooks {
        wake: Arc::new(move || wake.wake()),
        ..emu::Hooks::none()
    };
    let (outbox, commands) = session::Outbox::new();
    let writer = outbox.clone();
    let hooks = emu::Hooks {
        write: Arc::new(move |text| writer.write(text)),
        ..hooks
    };
    let screen = emu::Screen::new(100, 30, hooks);
    let channel = session::output_channel(screen.clone(), tx);
    let backend = Backend::core();
    let dir = std::env::temp_dir();
    let id = block_on(async {
        let starting = (backend.spawn)(session::Spawn {
            shell: "cmd".into(),
            cwd: dir.to_string_lossy().into_owned(),
            cols: 100,
            rows: 30,
            output: channel,
        });
        starting.await
    })
    .expect("cmd starts");
    session::start_writer(id, commands, backend);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut typed = false;
    let mut cwd = None;
    while std::time::Instant::now() < deadline {
        while let Ok(msg) = rx.try_recv() {
            if let session::Msg::Session(session::SessionEvent::Cwd(path)) = msg {
                cwd = Some(path);
            }
        }
        let rows = screen.with(|emu| emu.visible_text());
        if !typed && cwd.is_some() {
            outbox.write("echo glint-%COMPUTERNAME:~0,0%works\r");
            typed = true;
        }
        if rows.iter().any(|row| row == "glint-works") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let rows = screen.with(|emu| emu.visible_text());
    outbox.close();
    assert!(rows.iter().any(|row| row == "glint-works"), "{rows:?}");
    assert!(cwd.is_some(), "the prompt said where it is");
}
