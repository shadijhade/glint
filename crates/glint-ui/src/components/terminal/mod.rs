//! The terminal pane: shells in tabs under the files (TerminalPanel.tsx, and how Explorer.tsx placed it).
//!
//! | TypeScript                                          | Here                                                     |
//! |-----------------------------------------------------|----------------------------------------------------------|
//! | `TerminalPanel`, the tab bar, `ResizeHandle`        | [`TerminalPane`]                                         |
//! | `TerminalView` (xterm.js or ghostty-web)            | [`TerminalView`] (`view.rs`) over [`emu`] and [`element`] |
//! | `terminal.ts`: `newTerminal`, `runInTerminal`, `toggleTerminal`, `cdActive`, `closeTerminal` | [`TerminalPane::open`], [`run`](TerminalPane::run), [`toggle`](TerminalPane::toggle), `cd_active`, `close_tab` |
//! | `termShells`, `termSpawn`, `termWrite`, `termResize`, `termClose` | [`session::Backend`] (glint-core's `term.rs`)  |
//! | xterm's key and mouse handling                      | [`keys`]                                                 |
//! | xterm's web-links addon                             | [`links`], Ctrl+click                                    |
//! | `terminalTheme()`                                   | [`emu::Palette`] from the app's theme tokens             |
//!
//! # Wiring (for the workspace)
//!
//! - **Make it.** Once, with the window: `let terminal = cx.new(|cx| TerminalPane::new(window, cx));` and keep the
//!   entity. Observe it (`cx.observe(&terminal, ..)`) so the window re-lays out when it opens, closes or gains a tab.
//! - **Place it.** At the bottom of the column that holds the file panes and the command bar, under them and above the
//!   status bar, `flex_none`: `col().child(files.flex_1().min_h_0()).child(terminal.clone())`. It draws nothing (an
//!   empty `div`) while it is hidden or has no tab, and otherwise draws itself at its saved height
//!   (`settings.terminal_height`, 260 by default) with a draggable edge on top (a double click resets the height),
//!   so the workspace gives it no size. The preview pane stays beside the files, above it, as in the web app.
//! - **Ctrl+`.** The `ToggleTerminal` action calls `terminal.update(cx, |t, cx| t.toggle(window, cx))`. The same call
//!   serves "Terminal here, under the files" in the Tools menu and the palette's `terminal-pane` entry (whose label
//!   is "Hide the terminal" while [`TerminalPane::is_open`]). A hidden pane's shells keep running. The terminal
//!   gets every key but Ctrl+` while it has focus (see below); Ctrl+` itself is the workspace's binding.
//! - **Run a task.** `CommandBarEvent::RunTask { folder, command }` calls
//!   `terminal.update(cx, |t, cx| t.run_task(&folder, &command, window, cx))` (the TypeScript's `runTask`: the tab is
//!   named after the command, which is typed with Enter).
//! - **Run a script.** `explorer::Request::Terminal(job)` calls `t.run_job(&job, window, cx)`
//!   (`runInTerminal(cwd, (shell) => runLine(shell, …), name)`: [`TerminalJob::line`](crate::explorer::TerminalJob::line)
//!   makes the text for the tab's shell; the tab that ran it last is reused when it is back at its prompt in the same
//!   folder). Not in WSL, whose tools aren't Windows'.
//! - **A new tab here.** `t.open(&cwd, window, cx)` (`newTerminal(cwd)`); `t.open_here(window, cx)` uses the folder
//!   shown, or home from This PC (`terminalFolder()`).
//! - **Focus back.** The pane emits [`TerminalPaneEvent::Hidden`] when it closes; the workspace puts the keyboard back
//!   on the file list.
//! - **Keys.** The pane installs one app-wide keystroke interceptor (`cx.intercept_keystrokes`), so while a terminal
//!   has the keyboard its keys reach it before any binding (Ctrl+K, Ctrl+W, F5, Escape…): the TypeScript stopped
//!   propagation the same way. The interceptor lets typed text through to GPUI's input handler (that is how input
//!   methods work) and does nothing while the pane is hidden. No binding needs to change. The view's key context is
//!   `"Terminal"`.
//! - **Follow.** The pane watches the explorer and sends `cd` to the active shell when it is at its prompt, and
//!   navigates the explorer when the shell's prompt says it went somewhere (the "Follow" toggle,
//!   `settings.terminal_follow`), as TerminalPanel.tsx did.
//!
//! # Not ported or different
//!
//! - **The renderer** is not Ghostty's or xterm.js's: it is [`alacritty_terminal`]'s emulator with [`element`]'s own
//!   painting. Box-drawing and block characters are drawn from the font (there is no special drawing for them, so
//!   gaps can show between rows with some fonts); a dotted, dashed or double underline is drawn plain; images
//!   (sixel, kitty) are not drawn; the kitty keyboard protocol is not spoken.
//! - **Right to left.** A terminal draws a letter a cell, so Arabic and Hebrew can look out of order (the same is true
//!   of every terminal). The pane says so, once, with a line under the tabs while such text is on screen.
//! - **Exit message** (`[PowerShell ended.]`) is written in English into the terminal itself, as the TypeScript did.

pub mod element;
pub mod emu;
pub mod keys;
pub mod links;
pub mod session;
pub mod view;

pub use session::{Backend, Shell};
pub use view::{palette_of, TerminalEvent, TerminalView};

use crate::explorer::{Explorer, NavOptions, TerminalJob};
use crate::kit::prelude::*;
use crate::{state, t, theme};
use glint_model::cd::cd_command;
use glint_model::paths::{base_name, is_folder_path, same_path, THIS_PC};
use gpui_kit::{
    anchored, deferred, Anchor, ClickEvent, DismissEvent, DragMoveEvent, FontWeight, MouseButton, MouseDownEvent, Role,
    Subscription, Task, WeakEntity,
};
use std::rc::Rc;
use std::time::Duration;

/// How tall the pane is until the person drags it (`?? 260`).
pub const DEFAULT_HEIGHT: f32 = 260.;
/// The least it is dragged to (`Math.max(96, …)`); the most leaves 220 px of the window.
const MIN_HEIGHT: f32 = 96.;
const KEEP_FOR_FILES: f32 = 220.;
/// A dragged height is saved this long after the last move.
const SAVE_AFTER: Duration = Duration::from_millis(300);

/// What the pane tells the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalPaneEvent {
    /// The pane was hidden: put the keyboard back on the files.
    Hidden,
}

/// The payload of a drag on the pane's top edge.
struct ResizeDrag;

/// What follows the pointer while the edge is dragged: nothing.
struct NoChip;

impl Render for NoChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// What to run in a new tab (`runInTerminal`).
#[derive(Clone)]
struct Job {
    title: String,
    line: Rc<dyn Fn(&str) -> String>,
}

/// A tab asked for before the shells on this PC were known.
struct Wanted {
    cwd: String,
    shell: Option<String>,
    job: Option<Job>,
}

/// One terminal tab (`TermTab`).
struct Tab {
    key: u64,
    shell: String,
    shell_name: String,
    /// Where its prompt last said it is (where it started, till then).
    cwd: String,
    /// Where the prompt said it was before the newest report: `moved` compares with it.
    said: String,
    /// What it was started to run (a task, a script), which its tab says; a shell's tab says where it is.
    title: Option<String>,
    exited: Option<u32>,
    /// What is typed at its prompt once its shell first says where it is, Enter included.
    run: Option<String>,
    rtl: bool,
    view: Entity<TerminalView>,
    _events: Subscription,
}

impl Tab {
    /// The tab's text: the task's name, else the folder.
    fn label(&self) -> String {
        self.title.clone().unwrap_or_else(|| {
            let name = base_name(&self.cwd);
            if name.is_empty() {
                self.cwd.clone()
            } else {
                name.to_string()
            }
        })
    }
}

/// The shell picker while it is open.
struct OpenMenu {
    at: Point<Pixels>,
    menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

/// The terminal pane, under the file panes: terminal tabs, each a shell, following the folder shown.
pub struct TerminalPane {
    explorer: Entity<Explorer>,
    tabs: Vec<Tab>,
    active: Option<u64>,
    open: bool,
    next: u64,
    shells: Vec<Shell>,
    shells_ready: bool,
    waiting: Vec<Wanted>,
    /// The height while the edge is being dragged.
    dragging: Option<f32>,
    _save_height: Task<()>,
    /// The folder the explorer showed last, so a cd goes only when it changes.
    last_folder: String,
    menu: Option<OpenMenu>,
    note_dismissed: bool,
    _shells: Task<()>,
    _explorer: Subscription,
    _keys: Subscription,
}

impl EventEmitter<TerminalPaneEvent> for TerminalPane {}

impl TerminalPane {
    /// An empty, hidden pane. It asks glint-core for the shells on this PC in the background.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = session::backend(cx);
        let explorer = crate::explorer::explorer(cx);
        let weak = cx.weak_entity();
        // Keys go to the shell before the app's shortcuts see them (see the module notes).
        let keys = cx.intercept_keystrokes(move |event, window, cx| Self::intercept(&weak, event, window, cx));
        let observed = cx.observe(&explorer, |this, _, cx| this.folder_changed(cx));
        let finding = (backend.shells)();
        let shells = cx.spawn_in(window, async move |this, cx| {
            let found = finding.await;
            this.update_in(cx, |this, window, cx| {
                this.shells = found;
                this.shells_ready = true;
                // Tabs asked for before the shells were known are made now, in the order asked.
                for wanted in std::mem::take(&mut this.waiting) {
                    this.add(wanted, window, cx);
                }
                cx.notify();
            })
            .ok();
        });
        TerminalPane {
            explorer,
            tabs: Vec::new(),
            active: None,
            open: false,
            next: 0,
            shells: Vec::new(),
            shells_ready: false,
            waiting: Vec::new(),
            dragging: None,
            _save_height: Task::ready(()),
            last_folder: THIS_PC.to_string(),
            menu: None,
            note_dismissed: false,
            _shells: shells,
            _explorer: observed,
            _keys: keys,
        }
    }

    /// The keystroke interceptor: the focused terminal's keys are the shell's.
    fn intercept(pane: &WeakEntity<Self>, event: &gpui_kit::KeystrokeEvent, window: &mut Window, cx: &mut App) {
        let Some(pane) = pane.upgrade() else { return };
        let Some(view) = pane.read(cx).active_view() else {
            return;
        };
        if !pane.read(cx).open || !view.read(cx).focus_handle().contains_focused(window, cx) {
            return;
        }
        let keystroke = event.keystroke.clone();
        if view.update(cx, |view, cx| view.key(&keystroke, window, cx)) {
            cx.stop_propagation();
        }
    }

    // ---- What the window asks

    /// Whether the pane is shown (Ctrl+`'s state: `useTerminals.open`).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the pane draws anything: it is open and has a tab.
    pub fn visible(&self) -> bool {
        self.open && !self.tabs.is_empty()
    }

    /// How many terminals there are.
    #[cfg(test)]
    pub fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// The terminal on show.
    pub fn active_view(&self) -> Option<Entity<TerminalView>> {
        let key = self.active?;
        self.tabs.iter().find(|tab| tab.key == key).map(|tab| tab.view.clone())
    }

    /// The folder on disk a terminal starts in or follows: the active pane's, or home from This PC
    /// (`terminalFolder()`).
    pub fn terminal_folder(&self, cx: &App) -> String {
        let explorer = self.explorer.read(cx);
        let path = explorer.current_path();
        if is_folder_path(path) {
            path.to_string()
        } else {
            let home = explorer.home_folder();
            if home.is_empty() {
                "C:\\".to_string()
            } else {
                home.to_string()
            }
        }
    }

    /// Shows or hides the pane (Ctrl+`); shown with none, it starts one in the folder shown.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            return self.hide(cx);
        }
        if self.tabs.is_empty() {
            let cwd = self.terminal_folder(cx);
            self.open(&cwd, window, cx);
        } else {
            self.show(window, cx);
        }
    }

    /// Shows the pane (with a tab, if there is one), and gives the keyboard to the terminal.
    pub fn show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            return;
        }
        self.open = true;
        self.focus_active(window, cx);
        self.follow_folder(cx);
        cx.notify();
    }

    /// Gives the keyboard to the terminal on show.
    fn focus_active(&self, window: &mut Window, cx: &mut App) {
        if let Some(view) = self.active_view() {
            let handle = view.read(cx).focus_handle().clone();
            window.focus(&handle, cx);
        }
    }

    /// Hides the pane; its shells keep running.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        self.menu = None;
        cx.emit(TerminalPaneEvent::Hidden);
        cx.notify();
    }

    /// A new terminal tab in `cwd` with the default shell, shown (`newTerminal`).
    pub fn open(&mut self, cwd: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.add(
            Wanted {
                cwd: cwd.to_string(),
                shell: None,
                job: None,
            },
            window,
            cx,
        );
    }

    /// A new terminal in the folder shown (the "+" button).
    pub fn open_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = self.terminal_folder(cx);
        self.open(&cwd, window, cx);
    }

    /// A new tab in `cwd` with the shell called `shell` ("pwsh", "cmd"…).
    pub fn open_with(&mut self, shell: &str, cwd: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.add(
            Wanted {
                cwd: cwd.to_string(),
                shell: Some(shell.to_string()),
                job: None,
            },
            window,
            cx,
        );
    }

    /// Runs the line `line(shell)` in a tab named `title`, shown (`runInTerminal`): in the tab that ran it last if
    /// that is back at its prompt in `cwd`, else in a new one in `cwd`, where it is typed once the shell is up.
    pub fn run(
        &mut self,
        cwd: &str,
        title: &str,
        line: impl Fn(&str) -> String + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line: Rc<dyn Fn(&str) -> String> = Rc::new(line);
        // The newest tab with this title that ended nowhere, is in this folder and is waiting at its prompt.
        let again = self.tabs.iter().rev().find(|tab| {
            tab.title.as_deref() == Some(title)
                && tab.exited.is_none()
                && same_path(&tab.cwd, cwd)
                && tab.view.read(cx).is_idle()
        });
        if let Some(tab) = again {
            let (key, text, view) = (tab.key, line(&tab.shell), tab.view.clone());
            view.update(cx, |view, cx| view.type_text(&text, cx));
            self.select_key(key, window, cx);
            self.open = true;
            cx.notify();
            return;
        }
        self.add(
            Wanted {
                cwd: cwd.to_string(),
                shell: None,
                job: Some(Job {
                    title: title.to_string(),
                    line,
                }),
            },
            window,
            cx,
        );
    }

    /// A script from the explorer's menu (`Request::Terminal`).
    pub fn run_job(&mut self, job: &TerminalJob, window: &mut Window, cx: &mut Context<Self>) {
        let job = job.clone();
        let (cwd, title) = (job.cwd.clone(), job.title.clone());
        self.run(&cwd, &title, move |shell| job.line(shell), window, cx);
    }

    /// A project's task from the command bar (`runTask`): the command, Enter included, in a tab named after it.
    pub fn run_task(&mut self, folder: &str, command: &str, window: &mut Window, cx: &mut Context<Self>) {
        let line = format!("{command}\r");
        self.run(folder, command, move |_| line.clone(), window, cx);
    }

    // ---- Tabs

    /// The shell new terminals use: the one chosen in Settings, else the first found, else cmd (`defaultShell`).
    fn default_shell(&self, cx: &App) -> String {
        let chosen = &state::settings(cx).terminal_shell;
        self.shells
            .iter()
            .find(|shell| &shell.id == chosen)
            .or_else(|| self.shells.first())
            .map_or_else(|| "cmd".to_string(), |shell| shell.id.clone())
    }

    fn add(&mut self, wanted: Wanted, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shells_ready {
            // The shells on this PC aren't known yet: the tab is made when they are (a moment).
            self.waiting.push(wanted);
            return;
        }
        let preferred = wanted.shell.clone().unwrap_or_else(|| self.default_shell(cx));
        // A script's tools are Windows': not WSL's.
        let shell_id = if wanted.job.is_some() && wanted.shell.is_none() && preferred == "wsl" {
            self.shells
                .iter()
                .find(|s| s.id != "wsl")
                .map_or_else(|| "cmd".to_string(), |s| s.id.clone())
        } else {
            preferred
        };
        let shell_name = self
            .shells
            .iter()
            .find(|shell| shell.id == shell_id)
            .map_or_else(|| shell_id.clone(), |shell| shell.name.clone());
        let key = self.next;
        self.next += 1;
        let view = {
            let (id, name, cwd) = (shell_id.clone(), shell_name.clone(), wanted.cwd.clone());
            cx.new(|cx| TerminalView::live(&id, name, &cwd, window, cx))
        };
        let events = cx.subscribe_in(&view, window, move |this, _, event: &TerminalEvent, window, cx| {
            this.on_event(key, event, window, cx)
        });
        let run = wanted.job.as_ref().map(|job| (job.line)(&shell_id));
        self.tabs.push(Tab {
            key,
            shell: shell_id,
            shell_name,
            said: wanted.cwd.clone(),
            cwd: wanted.cwd,
            title: wanted.job.map(|job| job.title),
            exited: None,
            run,
            rtl: false,
            view: view.clone(),
            _events: events,
        });
        self.active = Some(key);
        self.open = true;
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Makes `key` the tab on show.
    fn select_key(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.iter().any(|tab| tab.key == key) {
            self.active = Some(key);
            self.focus_active(window, cx);
            self.follow_folder(cx);
            cx.notify();
        }
    }

    /// Closes a tab, ending its shell (`closeTerminal`): the one after it takes over, or the one before.
    pub fn close_tab(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.tabs.iter().position(|tab| tab.key == key) else {
            return;
        };
        self.tabs.remove(at);
        if self.active == Some(key) {
            self.active = self
                .tabs
                .get(at.min(self.tabs.len().saturating_sub(1)))
                .map(|tab| tab.key);
        }
        if self.tabs.is_empty() {
            self.hide(cx);
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    // ---- Events from the terminals, and following the folder

    fn on_event(&mut self, key: u64, event: &TerminalEvent, _window: &mut Window, cx: &mut Context<Self>) {
        match event {
            TerminalEvent::Cwd(path) => self.on_cwd(key, path, cx),
            TerminalEvent::Exited(code) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.key == key) {
                    tab.exited = Some(*code);
                }
                cx.notify();
            }
            TerminalEvent::Rtl(rtl) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.key == key) {
                    tab.rtl = *rtl;
                }
                cx.notify();
            }
            TerminalEvent::OpenPath(path) => self.open_path(path.clone(), cx),
            TerminalEvent::Title(_) => {}
        }
    }

    /// A shell's prompt said where it is.
    fn on_cwd(&mut self, key: u64, path: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.key == key) else {
            return;
        };
        let moved = !same_path(&tab.said, path);
        tab.said = path.to_string();
        tab.cwd = path.to_string();
        // Its first prompt: what it was opened to run goes in.
        if let Some(run) = tab.run.take() {
            tab.view.update(cx, |view, cx| view.type_text(&run, cx));
        }
        cx.notify();
        if !state::settings(cx).terminal_follow || !self.open || self.active != Some(key) {
            return;
        }
        let shown = self.explorer.read(cx).current_path().to_string();
        if moved && !same_path(&shown, path) {
            // The shell went somewhere: the pane follows.
            let path = path.to_string();
            self.explorer
                .update(cx, |explorer, cx| explorer.navigate(path, NavOptions::default(), cx));
        } else if !moved && is_folder_path(&shown) && !same_path(&shown, path) {
            // The pane went somewhere while the shell was busy: now it follows.
            self.cd_active(&shown, cx);
        }
    }

    /// The folder the active explorer tab shows, if it is a plain folder on disk (This PC otherwise).
    fn folder_shown(&self, cx: &App) -> String {
        match self.explorer.read(cx).active_tab() {
            Some(tab)
                if tab.find.is_none()
                    && tab.listing.as_ref().is_none_or(|listing| listing.archive.is_none())
                    && is_folder_path(&tab.path) =>
            {
                tab.path.clone()
            }
            _ => THIS_PC.to_string(),
        }
    }

    /// The explorer changed: if the folder shown did, the terminal follows it.
    fn folder_changed(&mut self, cx: &mut Context<Self>) {
        let folder = self.folder_shown(cx);
        if folder != self.last_folder {
            self.last_folder = folder;
            self.follow_folder(cx);
        }
    }

    /// Takes the active terminal to the folder shown, when following is on and the pane is up.
    fn follow_folder(&mut self, cx: &mut Context<Self>) {
        let folder = self.folder_shown(cx);
        self.last_folder = folder.clone();
        if self.open && state::settings(cx).terminal_follow && folder != THIS_PC {
            self.cd_active(&folder, cx);
        }
    }

    /// Takes the active terminal to `folder`, if it is at its prompt with nothing typed (`cdActive`).
    fn cd_active(&mut self, folder: &str, cx: &mut Context<Self>) {
        let Some(key) = self.active else { return };
        let Some(tab) = self.tabs.iter().find(|tab| tab.key == key) else {
            return;
        };
        if tab.exited.is_some() || !tab.view.read(cx).is_idle() || same_path(&tab.cwd, folder) {
            return;
        }
        let line = cd_command(&tab.shell, folder);
        tab.view.update(cx, |view, cx| view.type_text(&line, cx));
    }

    /// A path Ctrl+clicked in the output: a folder opens in the pane, a file opens as double-clicking would.
    fn open_path(&mut self, path: String, cx: &mut Context<Self>) {
        let explorer = self.explorer.clone();
        cx.spawn(async move |_, cx| {
            let probe = path.clone();
            let kind = cx
                .background_spawn(async move { std::fs::metadata(&probe).map(|meta| meta.is_dir()) })
                .await;
            let Ok(is_dir) = kind else { return };
            explorer.update(cx, |explorer, cx| {
                if is_dir {
                    explorer.navigate(path, NavOptions::default(), cx);
                } else {
                    explorer.open_file(&path, None, cx).detach();
                }
            });
        })
        .detach();
    }

    // ---- Height

    /// The pane's height now.
    pub fn height(&self, cx: &App) -> f32 {
        self.dragging
            .or(state::settings(cx).terminal_height.map(|h| h as f32))
            .unwrap_or(DEFAULT_HEIGHT)
    }

    fn drag_height(&mut self, bottom: Pixels, pointer: Pixels, window: &Window, cx: &mut Context<Self>) {
        let most = (f32::from(window.viewport_size().height) - KEEP_FOR_FILES).max(MIN_HEIGHT);
        let height = f32::from(bottom - pointer).round().clamp(MIN_HEIGHT, most);
        if self.dragging == Some(height) {
            return;
        }
        self.dragging = Some(height);
        // Saved once the pointer rests, not on every move.
        self._save_height = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            this.update(cx, |this, cx| {
                if let Some(height) = this.dragging.take() {
                    state::update(cx, |settings| settings.terminal_height = Some(f64::from(height)));
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn reset_height(&mut self, cx: &mut Context<Self>) {
        self.dragging = None;
        state::update(cx, |settings| settings.terminal_height = None);
        cx.notify();
    }

    // ---- The shell picker

    fn open_shell_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let pane: WeakEntity<Self> = cx.weak_entity();
        let shells = self.shells.clone();
        let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
            for shell in &shells {
                let (pane, id) = (pane.clone(), shell.id.clone());
                menu = menu.item(
                    PopupMenuItem::new(shell.name.clone())
                        .icon(IconName::SquareTerminal)
                        .on_click(move |_, window, cx| {
                            pane.update(cx, |pane, cx| {
                                let cwd = pane.terminal_folder(cx);
                                pane.open_with(&id, &cwd, window, cx);
                            })
                            .ok();
                        }),
                );
            }
            menu
        });
        window.focus(&menu.focus_handle(cx), cx);
        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.menu = None;
            cx.notify();
        });
        self.menu = Some(OpenMenu {
            at,
            menu,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    // ---- Drawing

    fn tab_strip(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| {
                let key = tab.key;
                let selected = self.active == Some(key);
                let label = tab.label();
                let group = SharedString::from(format!("terminal-tab-group-{key}"));
                let tip = format!(
                    "{}{} · {}",
                    tab.title
                        .as_ref()
                        .map(|title| format!("{title} · "))
                        .unwrap_or_default(),
                    tab.shell_name,
                    tab.cwd
                );
                let close_group = group.clone();
                row_start()
                    .id(("terminal-tab", key))
                    .role(Role::Tab)
                    .aria_label(isolate(label.clone()))
                    .aria_selected(selected)
                    .test_support()
                    .group(group)
                    .h(space(6.))
                    .max_w(space(48.))
                    .flex_shrink_0()
                    .gap(space(1.5))
                    .ps(2.)
                    .pe(0.5)
                    .rounded(theme::size::RADIUS_SM)
                    .text_size(theme::size::XS)
                    .cursor_default()
                    .text_color(if selected { t.foreground } else { t.muted_foreground })
                    .when(selected, |tab| tab.bg(t.background))
                    .when(!selected, |tab| tab.hover(|s| s.bg(t.accent).text_color(t.foreground)))
                    .when(tab.exited.is_some(), |tab| tab.opacity(0.6))
                    .tooltip(move |window, cx| crate::kit::tooltip(tip.clone().into(), window, cx))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.close_tab(key, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.select_key(key, window, cx)))
                    .child(icon(IconName::SquareTerminal).size_3p5().flex_none())
                    .child(div().truncate().child(isolate(label)))
                    .child(
                        div()
                            .flex_none()
                            .opacity(if selected { 1. } else { 0. })
                            .group_hover(close_group, |s| s.opacity(1.))
                            .child(
                                Button::new(("terminal-close", key))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::X)
                                    .accessibility_label(t!("Close this terminal"))
                                    .tooltip(t!("Close (ends its shell)"))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.close_tab(key, window, cx);
                                    })),
                            ),
                    )
            })
            .collect();
        row_start()
            .id("terminal-tabs")
            .role(Role::TabList)
            .aria_label(t!("Terminals"))
            .test_support()
            .min_w_0()
            .gap(space(0.5))
            .overflow_x_scroll()
            .children(tabs)
    }

    fn header(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let follow = state::settings(cx).terminal_follow;
        let follow_tip = if follow {
            t!("Following: a cd in the terminal opens that folder here, and a folder opened here is cd'd to (when the shell is at its prompt). Click to stop.")
        } else {
            t!("Not following: click so the terminal and the folder shown go where the other goes")
        };
        let hide_tip = t!("Hide the terminal (Ctrl+`): its shells keep running");
        row_start()
            .id("terminal-header")
            .h(space(8.))
            .flex_none()
            .w_full()
            .gap(space(0.5))
            .px(space(1.5))
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .bg(theme::chrome_fill(cx).opacity(0.6))
            .text_size(theme::size::XS)
            .child(self.tab_strip(t, cx))
            .child(
                Button::new("terminal-new")
                    .ghost()
                    .small()
                    .icon(IconName::Plus)
                    .accessibility_label(t!("New terminal here"))
                    .tooltip(t!("New terminal here"))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.open_here(window, cx))),
            )
            .when(self.shells.len() > 1, |header| {
                header.child(
                    Button::new("terminal-shells")
                        .ghost()
                        .small()
                        .icon(IconName::ChevronDown)
                        .accessibility_label(t!("New terminal with another shell"))
                        .tooltip(t!("New terminal with another shell"))
                        .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                            let at = event.position() + gpui_kit::point(px(0.), px(16.));
                            this.open_shell_menu(at, window, cx);
                        })),
                )
            })
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("terminal-follow")
                    .ghost()
                    .small()
                    .icon(IconName::Link2)
                    .label(t!("Follow"))
                    .toggled(follow)
                    .accessibility_label(t!("Follow"))
                    .tooltip(follow_tip)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        state::update(cx, |settings| settings.terminal_follow = !follow);
                        this.follow_folder(cx);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("terminal-hide")
                    .ghost()
                    .small()
                    .icon(IconName::ChevronDown)
                    .accessibility_label(hide_tip.clone())
                    .tooltip(hide_tip)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.hide(cx))),
            )
    }

    /// The line about right-to-left text, with its close button.
    fn rtl_note(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let text = t!("Right-to-left text can look out of order here: a terminal draws one letter in each cell.");
        row_start()
            .id("terminal-rtl-note")
            .role(Role::Status)
            .aria_label(text.clone())
            .test_support()
            .flex_none()
            .w_full()
            .gap(space(2.))
            .px(space(3.))
            .py(space(1.))
            .items_center()
            .border_b_1()
            .border_color(t.border)
            .bg(t.muted)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(icon(IconName::Info).size_3p5().flex_none())
            .child(div().flex_1().min_w_0().font_weight(FontWeight::NORMAL).child(text))
            .child(
                Button::new("terminal-rtl-note-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::X)
                    .accessibility_label(t!("Dismiss"))
                    .tooltip(t!("Dismiss"))
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.note_dismissed = true;
                        cx.notify();
                    })),
            )
    }
}

impl Render for TerminalPane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.active_view().filter(|_| self.open) else {
            // Hidden, or nothing to show: no room taken. The shells keep running.
            return div().id("terminal-pane-hidden").into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let height = self.height(cx);
        let rtl = self
            .tabs
            .iter()
            .find(|tab| Some(tab.key) == self.active)
            .is_some_and(|tab| tab.rtl);
        let menu = self.menu.as_ref().map(|open| {
            deferred(
                anchored()
                    .position(open.at)
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(open.menu.clone()),
            )
            .with_priority(1)
        });
        col()
            .id("terminal-pane")
            .role(Role::Group)
            .aria_label(t!("Terminal"))
            .test_support()
            .relative()
            .flex_none()
            .w_full()
            .h(px(height))
            .border_t_1()
            .border_color(t.border)
            .bg(t.card)
            .text_start()
            // The edge drags the height; the handler is here because the pane's own box is what is measured.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ResizeDrag>, window, cx| {
                this.drag_height(event.bounds.bottom(), event.event.position.y, window, cx);
            }))
            .child(
                div()
                    .id("terminal-resize")
                    .role(Role::Splitter)
                    .aria_label(t!("Terminal height"))
                    .test_support()
                    .absolute()
                    .top(px(-3.))
                    .left_0()
                    .w_full()
                    .h(px(6.))
                    .cursor_row_resize()
                    .on_drag(ResizeDrag, |_, _, _, cx| cx.new(|_| NoChip))
                    .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                        if event.click_count() == 2 {
                            this.reset_height(cx);
                        }
                    })),
            )
            .child(self.header(&t, cx))
            .when(rtl && !self.note_dismissed, |pane| pane.child(self.rtl_note(&t, cx)))
            .child(div().id("terminal-body").flex_1().min_h_0().w_full().child(view))
            .children(menu)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests;
