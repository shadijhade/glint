//! The command bar under the address bar: New, Cut, Copy, Paste, Rename, Delete, the view buttons, Tools...
//! Ported from src/components/explorer/CommandBar.tsx, and from where Explorer.tsx placed and fed it
//! (`ActiveCommandBar`: the active pane's selection and compare pair).
//!
//! # What the bar is
//!
//! A 2.5 rem tall row for **the active tab**: it keeps no copy of what the explorer holds. [`Facts`] (`facts.rs`) is
//! worked out from the explorer, the settings and the disk usage map when one of them changes, so a draw scans
//! nothing. Buttons act through the workspace's actions (New folder, Cut, Copy, Paste, Rename, Delete, Disk usage,
//! Search text, the palette, the view toggles...), through explorer methods (Restore, Extract all, the other pane,
//! the views) or, for what the window owns, through a [`CommandBarEvent`].
//!
//! | Button | Does |
//! |---|---|
//! | New folder, Cut, Copy, Paste, Rename, Delete | the action of that name (the workspace's handler, with Undo) |
//! | New's arrow | the New menu (`menus::new_menu`); a kind chosen is a [`CommandBarEvent::Menu`] |
//! | Copy / Move to the other pane | `Explorer::copy_to_other_pane` |
//! | Compare | [`MenuEvent::Compare`] with the two files shown in its tooltip |
//! | Restore, Delete for good (Recycle Bin) | `Explorer::restore_recycled`; the Delete action (the workspace purges) |
//! | Empty the Recycle Bin | [`MenuEvent::EmptyRecycleBin`] (the window asks first) |
//! | Extract all (archive) | `Explorer::extract_here` |
//! | Tools | the Tools menu: dialogs by [`CommandBarEvent::Menu`], Terminal here by the `ToggleTerminal` action |
//! | Run | the project's tasks (`glint_core::projects`): [`CommandBarEvent::RunTask`] |
//! | List, Grid, Columns, tile sizes | `Explorer::set_view` after taking the disk usage map off; the sizes set `tile_size` |
//! | Disk usage, Search text, the palette | the `DiskUsage`, `TextSearch` and `CommandPalette` actions |
//! | Folder sizes | [`CommandBarEvent::FolderSizes`] |
//! | Hidden items, Preview pane, Dual pane | the `ToggleHidden`, `TogglePreview` and `DualPane` actions |
//! | Compare panes | flips `Explorer::compare_panes` (no handler for its action exists yet) |
//!
//! **Not in this bar.** CommandBar.tsx has no Sort, Group by or Columns menus (those are the list header's menu and
//! Settings → Files, ported with the file list), so none is here.
//!
//! **Why actions are dispatched through [`CommandBar::dispatch`].** A button doesn't take the keyboard when it is
//! pressed, so with nothing focused an action would start at the window's root and never reach the workspace's
//! handlers; the bar takes the keyboard first in that case.
//!
//! # Folding
//!
//! As the bar narrows its labels go, then buttons move into menus (see [`Folding`]). The widths are measured the
//! STANDARDS way: three canvases record the bar's, the left group's and the right group's widths while painting
//! and ask for another frame with `window.on_next_frame` (never `cx.notify` during a draw); the next render hands
//! them to [`Folding::settle`]. A bar that has just been laid out may show one frame at its old fold.
//!
//! # Wiring (the coordinator)
//!
//! 1. Make it once per window, beside the panes: `let bar = cx.new(|cx| CommandBar::new(window, cx));` and
//!    subscribe: `cx.subscribe_in(&bar, window, Workspace::on_command_bar_event)`.
//! 2. **Place it above the panes**, below the toolbar row, inside the column that holds the pane or the two panes,
//!    so it spans the files and not the sidebar or the preview pane (the TypeScript's column): in `body()`, make the
//!    `shown` panel `col().child(self.command_bar.clone()).child(div().flex_1().min_h_0().child(files))`. It is 2.5 rem
//!    (`space(10.)`) tall and as wide as the column. The TypeScript drew it for every place, This PC included (its
//!    folder buttons are greyed there); in text mode there is no files column, so no bar.
//! 3. **Take the temporary Disk usage button out of the toolbar** (`Workspace::disk_usage_button` and its use in
//!    `toolbar()`): the bar's Disk usage button replaces it, with the same action, tooltip and pressed state. Both
//!    have the accessible name "Disk usage", which a window must not have twice.
//! 4. Handle [`CommandBarEvent`]: see its variants.
//! 5. Call [`CommandBar::set_folder_sizes_running`] from the folder sizes part when it starts and ends.

mod facts;
mod menus;
#[cfg(test)]
mod tests;

pub use facts::{Facts, Folding, Pair, MOST};
pub use menus::{keyed, keyed_with, keys_of, Pick};

use crate::actions;
use crate::components::disk_usage;
use crate::components::menus::{visible_items, MenuEvent, MenuView, MenuViewEvent, Node, Run, Task as MenuTask};
use crate::components::preview;
use crate::core_bridge;
use crate::explorer::{self, Explorer};
use crate::kit::prelude::*;
use crate::theme::{self, Tokens};
use crate::{i18n, state, t};
use glint_core::projects::TaskGroup;
use glint_model::grid::{zoom_tiles, TileSize};
use glint_model::store::ViewMode;
use gpui_kit::{anchored, deferred, Anchor, Stateful};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The bar's height: `h-10`.
const HEIGHT: f32 = 10.;
/// The room the bar keeps between its two groups, in rems, so they never touch.
const MIN_GAP_REMS: f32 = 0.5;
/// A press that closed a menu is the press of the button that opened it if it comes this soon after.
const SAME_PRESS: Duration = Duration::from_millis(300);

/// What the window should do for the bar.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandBarEvent {
    /// A menu row chose something a dialog or another part does: hand it to the window's `MenuEvent` handler
    /// (`Workspace::on_menu_event`), which already knows every one the bar sends: the Tools menu's dialogs
    /// (`XRay`, `Locks`, `Timeline`, `History`, `Duplicates`, `Cleanup`, `ConvertPictures`, `Rule`, `SendToPc`,
    /// `ShareOnNetwork`, `ReceiveHere`, `Defender`, `Secrets`, `Yara`), the New menu's kinds (`NewTextDocument`,
    /// `NewOfKind`), `Compare`, `EmptyRecycleBin` and `InstallTool` (a project needs a tool Glint can fetch).
    Menu(MenuEvent),
    /// Folder sizes: measure every folder here (`stop` false), or stop measuring (`stop` true, when the bar said
    /// it was running). The window tells the bar when it starts and ends with
    /// [`CommandBar::set_folder_sizes_running`].
    FolderSizes {
        /// Stop what is running.
        stop: bool,
    },
    /// Run a project's task in the terminal pane, in the project's folder, typed as it'd be typed (`runTask`).
    RunTask {
        /// The project's folder.
        folder: String,
        /// What is typed at the prompt.
        command: String,
    },
}

type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// What the bar reads and starts that isn't the explorer: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Sources {
    /// What a folder and the folders above it can run (`projectTasks`).
    pub tasks: Rc<dyn Fn(String) -> Fut<Vec<TaskGroup>>>,
    /// Opens a terminal window in a folder (`openTerminal`).
    pub terminal: Rc<dyn Fn(String) -> Fut<()>>,
}

impl Sources {
    /// glint-core.
    pub fn core() -> Sources {
        Sources {
            tasks: Rc::new(|folder| Box::pin(glint_core::projects::project_tasks(folder))),
            terminal: Rc::new(|folder| Box::pin(glint_core::shell::open_terminal(folder))),
        }
    }

    /// Nothing runs and no terminal opens (tests).
    pub fn empty() -> Sources {
        Sources {
            tasks: Rc::new(|_| Box::pin(async { Ok(Vec::new()) })),
            terminal: Rc::new(|_| Box::pin(async { Ok(()) })),
        }
    }
}

/// What the bar's three canvases saw the last time it was painted.
#[derive(Default)]
struct Measured {
    bar: Cell<f32>,
    left: Cell<f32>,
    right: Cell<f32>,
    /// The fold and the signature the painted frame was drawn with.
    fold: Cell<usize>,
    signature: Cell<u64>,
}

/// The project tasks of the folder shown.
#[derive(Default)]
struct Tasks {
    /// The folder `groups` are for.
    folder: Option<String>,
    groups: Vec<TaskGroup>,
    /// What each folder could run, as last read (a folder opened again shows its Run menu at once).
    known: HashMap<String, Vec<TaskGroup>>,
    /// Numbers the reads, so one that comes back late is dropped.
    seq: u64,
}

/// A menu on screen.
struct OpenMenu {
    view: Entity<MenuView>,
    at: Point<Pixels>,
    /// Which button opened it.
    by: &'static str,
    /// What had the keyboard before it.
    focus: Option<FocusHandle>,
    _subscription: Subscription,
}

/// The command bar for the active tab.
pub struct CommandBar {
    explorer: Entity<Explorer>,
    sources: Sources,
    focus: FocusHandle,
    facts: Facts,
    /// The explorer, the settings or the map changed since `facts` was made.
    dirty: bool,
    folding: Folding,
    measured: Rc<Measured>,
    /// Where the buttons that open menus were painted, by name.
    anchors: Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>,
    /// Folder sizes are being measured (the window says so).
    sizing: bool,
    tasks: Tasks,
    menu: Option<OpenMenu>,
    /// The button whose menu a press closed last, and when.
    dismissed: Option<(&'static str, Instant)>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<CommandBarEvent> for CommandBar {}

impl CommandBar {
    /// The bar for the explorer's active tab, reading project tasks and opening terminals through glint-core.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_sources(window, cx, Sources::core())
    }

    /// The same, reading from `sources` (tests give fakes).
    pub fn with_sources(_window: &mut Window, cx: &mut Context<Self>, sources: Sources) -> Self {
        let explorer = explorer::explorer(cx);
        let mut subscriptions = vec![
            // Anything the explorer changes may change a button: the facts are made again on the next draw.
            cx.observe(&explorer, |this: &mut Self, _, cx| {
                this.dirty = true;
                cx.notify();
            }),
            // Show hidden items, the preview pane and the tile size are settings.
            cx.observe_global::<state::UiState>(|this: &mut Self, cx| {
                this.dirty = true;
                cx.notify();
            }),
        ];
        // The map's state (a tab shows it or not) lives in disk_usage.
        let usage = disk_usage::shared(cx);
        subscriptions.push(cx.observe(&usage, |this: &mut Self, _, cx| {
            this.dirty = true;
            cx.notify();
        }));
        CommandBar {
            explorer,
            sources,
            focus: cx.focus_handle(),
            facts: Facts::none(),
            dirty: true,
            folding: Folding::default(),
            measured: Rc::new(Measured::default()),
            anchors: Rc::new(RefCell::new(HashMap::new())),
            sizing: false,
            tasks: Tasks::default(),
            menu: None,
            dismissed: None,
            _subscriptions: subscriptions,
        }
    }

    /// What the bar's buttons depend on, as of the last draw (tests).
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    /// How far the bar is folded now: 0 shows every label, 6 is the narrowest.
    pub fn fold(&self) -> usize {
        self.folding.fold()
    }

    /// The project tasks of the folder shown, as last read.
    pub fn tasks(&self) -> &[TaskGroup] {
        &self.tasks.groups
    }

    /// Whether a menu of the bar is open.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// The menu on screen, for tests and screen-reader checks.
    pub fn menu_view(&self) -> Option<&Entity<MenuView>> {
        self.menu.as_ref().map(|open| &open.view)
    }

    /// Folder sizes are being measured (or stopped): the button says Stop while they are.
    pub fn set_folder_sizes_running(&mut self, running: bool, cx: &mut Context<Self>) {
        if self.sizing != running {
            self.sizing = running;
            cx.notify();
        }
    }

    // ---- Reading ---------------------------------------------------------------------------------------------

    /// Makes the facts again, and reads the folder's project tasks if the folder changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.dirty = false;
        let facts = {
            let explorer = self.explorer.read(cx);
            let mapped = explorer
                .active_tab()
                .is_some_and(|tab| tab.find.is_none() && disk_usage::shows(cx, tab.id));
            Facts::read(explorer, mapped, state::settings(cx))
        };
        let wanted = facts.can_run_tasks().then(|| facts.path.clone());
        self.facts = facts;
        if wanted != self.tasks.folder {
            self.read_tasks(wanted, cx);
        }
    }

    /// Shows what `folder` could run when it was last read, and reads it again in the background.
    fn read_tasks(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        self.tasks.seq += 1;
        self.tasks.groups = folder
            .as_ref()
            .and_then(|folder| self.tasks.known.get(&folder.to_lowercase()))
            .cloned()
            .unwrap_or_default();
        self.tasks.folder = folder.clone();
        let Some(folder) = folder else { return };
        let (seq, key) = (self.tasks.seq, folder.to_lowercase());
        let reading = (self.sources.tasks)(folder);
        cx.spawn(async move |this, cx| {
            let got = core_bridge::run(reading).await.and_then(|read| read);
            let _ = this.update(cx, |this, cx| {
                // A read that failed leaves the tasks as they were.
                if let (true, Ok(groups)) = (this.tasks.seq == seq, got) {
                    this.tasks.known.insert(key, groups.clone());
                    this.tasks.groups = groups;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// What the measurements depend on: the buttons there are, the text size and the language. When it changes
    /// the bar measures again.
    fn signature(&self, rem: f32) -> u64 {
        let f = &self.facts;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (
            f.dual,
            f.pair.is_some(),
            f.read_only,
            f.closed,
            f.bin,
            f.whole_archive.is_some(),
            !self.tasks.groups.is_empty(),
            self.sizing,
            i18n::is_rtl(),
            (rem * 10.) as u32,
        )
            .hash(&mut hasher);
        hasher.finish()
    }

    // ---- Doing -------------------------------------------------------------------------------------------------

    /// Runs a workspace action as its key would. A button doesn't take the keyboard when pressed, so with
    /// nothing focused the action would start at the window's root and miss the workspace's handlers: the bar
    /// takes the keyboard first then.
    pub fn dispatch(&self, action: Box<dyn Action>, window: &mut Window, cx: &mut App) {
        if window.focused(cx).is_none() {
            self.focus.focus(window, cx);
        }
        window.dispatch_action(action, cx);
    }

    /// List, grid or columns for the active tab (`chooseView`): a map it showed goes first.
    fn choose_view(&self, view: ViewMode, cx: &mut App) {
        let explorer = self.explorer.clone();
        let tab = explorer.read(cx).active_tab_id();
        disk_usage::set(cx, tab, false);
        explorer.update(cx, |e, cx| e.set_view(view, None, cx));
    }

    /// A tile size for the grid, shown as a grid (`pick` in `showSizes`).
    fn choose_tiles(&self, size: TileSize, cx: &mut App) {
        state::update(cx, |settings| settings.tile_size = size);
        self.choose_view(ViewMode::Grid, cx);
    }

    /// Highlight the differences between the two panes, or stop (the explorer computes the comparison).
    fn toggle_compare_panes(&self, cx: &mut App) {
        self.explorer.update(cx, |e, cx| e.toggle_compare_panes(cx));
    }

    /// Restore from the Recycle Bin: the selection goes back where each item was deleted from.
    fn restore(&self, cx: &mut App) {
        let entries = facts::selected_entries(self.explorer.read(cx));
        self.explorer
            .update(cx, |e, cx| e.restore_recycled(&entries, cx).detach());
    }

    /// Does what a row of the bar's own menus asked.
    fn pick(&mut self, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let action = |action: Box<dyn Action>, this: &Self, window: &mut Window, cx: &mut Context<Self>| {
            this.dispatch(action, window, cx)
        };
        match pick {
            Pick::Hidden => action(Box::new(actions::ToggleHidden), self, window, cx),
            Pick::Preview => action(Box::new(actions::TogglePreview), self, window, cx),
            Pick::Dual => action(Box::new(actions::DualPane), self, window, cx),
            Pick::ComparePanes => self.toggle_compare_panes(cx),
            Pick::View(view) => self.choose_view(view, cx),
            Pick::Tiles(size) => self.choose_tiles(size, cx),
            Pick::Larger => {
                let size = zoom_tiles(self.facts.tile_size.as_str(), 1);
                self.choose_tiles(size, cx)
            }
            Pick::Smaller => {
                let size = zoom_tiles(self.facts.tile_size.as_str(), -1);
                self.choose_tiles(size, cx)
            }
            Pick::Palette => action(Box::new(actions::CommandPalette), self, window, cx),
            Pick::DiskUsage => action(Box::new(actions::DiskUsage), self, window, cx),
            Pick::FolderSizes => {
                let stop = self.sizing;
                cx.emit(CommandBarEvent::FolderSizes { stop })
            }
            Pick::SearchText => action(Box::new(actions::TextSearch), self, window, cx),
            Pick::TerminalPane => action(Box::new(actions::ToggleTerminal), self, window, cx),
            Pick::CopyToOther => self
                .explorer
                .update(cx, |e, cx| e.copy_to_other_pane(false, None, cx).detach()),
            Pick::MoveToOther => self
                .explorer
                .update(cx, |e, cx| e.copy_to_other_pane(true, None, cx).detach()),
            Pick::Task { group, task } => {
                let chosen = self
                    .tasks
                    .groups
                    .get(group)
                    .and_then(|g| g.tasks.get(task).map(|task| (g.folder.clone(), task.command.clone())));
                if let Some((folder, command)) = chosen {
                    cx.emit(CommandBarEvent::RunTask { folder, command });
                }
            }
        }
    }

    /// A terminal window in the folder shown ("Open in Terminal"), telling the person if it can't.
    fn open_terminal(&self, folder: String, window: &mut Window, cx: &mut Context<Self>) {
        let opening = (self.sources.terminal)(folder);
        cx.spawn_in(window, async move |_, cx| {
            if let Err(reason) = core_bridge::run(opening).await.and_then(|opened| opened) {
                let _ = cx.update(|window, cx| {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't open a terminal")), cx)
                });
            }
        })
        .detach();
    }

    // ---- Menus -------------------------------------------------------------------------------------------------

    /// Shows `nodes` under the button `by`, taking the keyboard.
    fn open_menu(
        &mut self,
        by: &'static str,
        nodes: Vec<Node>,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The press that closed this button's menu is this click's own: it closes the menu, not opens it again.
        if self
            .dismissed
            .take()
            .is_some_and(|(button, at)| button == by && at.elapsed() < SAME_PRESS)
        {
            return;
        }
        let nodes = visible_items(nodes);
        if nodes.is_empty() {
            return;
        }
        self.close_menu(false, window, cx);
        let anchor = self.anchors.borrow().get(by).copied();
        let rtl = i18n::is_rtl();
        // Under the button, at its start edge: the menu opens toward the end (toward the start in Arabic).
        let at = match anchor {
            Some(bounds) => point(
                if rtl { bounds.right() } else { bounds.left() },
                bounds.bottom() + px(4.),
            ),
            None => point(px(8.), px(48.)),
        };
        let focus = window.focused(cx);
        let view = cx.new(|cx| MenuView::new(nodes, label, cx));
        view.update(cx, |view, cx| view.focus(window, cx));
        let subscription = cx.subscribe_in(&view, window, |this, _, event: &MenuViewEvent, window, cx| {
            this.on_menu_view(event.clone(), window, cx)
        });
        self.menu = Some(OpenMenu {
            view,
            at,
            by,
            focus,
            _subscription: subscription,
        });
        // A native preview draws above GPUI: it steps aside while the menu covers it.
        preview::cover(cx);
        cx.notify();
    }

    /// Closes the menu without choosing anything; `give_back` returns the keyboard to what had it.
    fn close_menu(&mut self, give_back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.menu.take() {
            preview::uncover(cx);
            if let (true, Some(focus)) = (give_back, open.focus) {
                focus.focus(window, cx);
            }
            cx.notify();
        }
    }

    fn on_menu_view(&mut self, event: MenuViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.menu.take() else { return };
        preview::uncover(cx);
        cx.notify();
        match event {
            MenuViewEvent::Chose(run) => {
                if let Some(focus) = &open.focus {
                    focus.focus(window, cx);
                }
                self.run_chosen(run, window, cx);
            }
            MenuViewEvent::Dismissed => {
                // A press on the button that opened it closed the menu: that button's click is the same press.
                let on_its_button = self
                    .anchors
                    .borrow()
                    .get(open.by)
                    .is_some_and(|bounds| bounds.contains(&window.mouse_position()));
                self.dismissed = on_its_button.then(|| (open.by, Instant::now()));
                // Escape gives the keyboard back; a press outside lets the thing pressed have it, unless that took
                // nothing.
                if let Some(focus) = open.focus.clone() {
                    let menu = open.view.read(cx).focus_handle(cx);
                    window.defer(cx, move |window, cx| {
                        if window.focused(cx).is_none_or(|now| now == menu) {
                            focus.focus(window, cx);
                        }
                    });
                }
            }
        }
    }

    /// Does what a chosen row says.
    fn run_chosen(&mut self, run: Run, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pick) = Pick::of(&run) {
            return self.pick(pick, window, cx);
        }
        match run {
            Run::Command(command) => self.dispatch(command.action(), window, cx),
            Run::Do(MenuTask::OpenTerminal(folder)) => self.open_terminal(folder, window, cx),
            // "Folder rules" with no folder: the rules in general.
            Run::Ask(MenuEvent::Rule { folder }) if folder.is_empty() => crate::dialogs::rules::open(window, cx, None),
            Run::Ask(event) => cx.emit(CommandBarEvent::Menu(event)),
            Run::Do(_) | Run::Nothing => {}
        }
    }

    fn show_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kinds = self.explorer.update(cx, |e, cx| e.new_kinds(cx));
        self.open_menu("new", menus::new_menu(&kinds), t!("New"), window, cx);
    }

    fn show_sizes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = menus::sizes_menu(self.facts.tile_size);
        self.open_menu("sizes", nodes, t!("Tile size in the grid"), window, cx);
    }

    fn show_tools(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = facts::selected_entries(self.explorer.read(cx));
        let nodes = menus::tools_menu(&self.facts, &selected, self.folding.fold(), self.sizing);
        self.open_menu("tools", nodes, t!("Tools"), window, cx);
    }

    fn show_tasks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = menus::tasks_menu(&self.tasks.groups);
        self.open_menu("tasks", nodes, t!("Run"), window, cx);
    }

    fn show_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = menus::view_menu(&self.facts, self.folding.fold());
        self.open_menu("view", nodes, t!("View"), window, cx);
    }

    fn show_edits(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = menus::edits_menu(&self.facts);
        self.open_menu("edits", nodes, t!("Cut, copy, paste, rename, delete"), window, cx);
    }
}

// ---- Drawing the buttons ------------------------------------------------------------------------------------

/// A ghost button in the bar's colors: muted until hovered, tinted with the accent while it is pressed.
fn ghost(cx: &App, t: &Tokens, id: impl Into<ElementId>, pressed: bool, labelled: bool) -> Button {
    let clear = hsla(0., 0., 0., 0.);
    Button::new(id).custom(
        ButtonCustomVariant::new(cx)
            .color(if pressed { t.primary.opacity(0.15) } else { clear })
            .foreground(if pressed {
                t.primary
            } else if labelled {
                t.foreground
            } else {
                t.muted_foreground
            })
            .hover(if pressed { t.primary.opacity(0.2) } else { t.accent })
            .active(if pressed { t.primary.opacity(0.25) } else { t.accent }),
    )
}

/// An icon button: its tooltip is its name, as the TypeScript's `Tool` had it. `pressed` makes it a toggle.
fn tool(cx: &App, t: &Tokens, id: &'static str, icon: IconName, name: SharedString, pressed: Option<bool>) -> Button {
    let button = ghost(cx, t, id, pressed == Some(true), false)
        .icon(icon)
        .accessibility_label(name.clone())
        .tooltip(name);
    match pressed {
        Some(on) => button.toggled(on),
        None => button,
    }
}

/// A button with a word beside its icon; the word goes when the bar folds, and stays its accessible name.
#[allow(clippy::too_many_arguments)] // a button's few independent parts, named at the call
fn word(
    cx: &App,
    t: &Tokens,
    id: &'static str,
    icon: IconName,
    label: SharedString,
    shown: bool,
    tip: SharedString,
    pressed: Option<bool>,
) -> Button {
    let button = ghost(cx, t, id, pressed == Some(true), true)
        .icon(icon)
        .accessibility_label(label.clone())
        .tooltip(tip);
    let button = if shown { button.label(label) } else { button };
    match pressed {
        Some(on) => button.toggled(on),
        None => button,
    }
}

/// The little arrow beside a button that opens a menu.
fn arrow(cx: &App, t: &Tokens, id: &'static str, name: SharedString) -> Button {
    ghost(cx, t, id, false, false)
        .icon(IconName::ChevronDown)
        .xsmall()
        .accessibility_label(name.clone())
        .tooltip(name)
}

fn divider(t: &Tokens) -> Div {
    div().mx(space(1.)).h(space(5.)).w(px(1.)).flex_none().bg(t.border)
}

/// Where a button that opens a menu is, for the menu to open under it.
fn anchor_probe(anchors: &Rc<RefCell<HashMap<&'static str, Bounds<Pixels>>>>, name: &'static str) -> impl IntoElement {
    let anchors = anchors.clone();
    canvas(
        move |bounds, _, _| {
            anchors.borrow_mut().insert(name, bounds);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// Records the width of a group (or of the whole bar) while painting, and asks for another frame after this
/// one when it changed. Not `cx.notify` here: GPUI drops a redraw asked for during a draw.
fn width_probe(cell: &Rc<Measured>, which: Width, fold: usize, signature: u64, id: EntityId) -> impl IntoElement {
    let measured = cell.clone();
    canvas(
        move |bounds, window, _| {
            let width = f32::from(bounds.size.width);
            let seen = match which {
                Width::Bar => &measured.bar,
                Width::Left => &measured.left,
                Width::Right => &measured.right,
            };
            let mut changed = (seen.get() - width).abs() > 0.5;
            seen.set(width);
            if which == Width::Bar {
                changed |= measured.fold.get() != fold || measured.signature.get() != signature;
                measured.fold.set(fold);
                measured.signature.set(signature);
            }
            if changed {
                window.on_next_frame(move |_, cx| cx.notify(id));
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Width {
    Bar,
    Left,
    Right,
}

impl CommandBar {
    /// A button that opens a menu, with the probe that tells the menu where it is.
    fn anchored(&self, name: &'static str, button: impl IntoElement) -> Div {
        div()
            .relative()
            .flex_none()
            .child(button)
            .child(anchor_probe(&self.anchors, name))
    }

    /// The group at the start edge: New, the edits, the tools.
    fn left_group(&self, fold: usize, t: &Tokens, cx: &mut Context<Self>) -> Stateful<Div> {
        let f = &self.facts;
        let labels = fold < 1;
        let group = row_start()
            .id("command-bar-start")
            .relative()
            .flex_none()
            .gap(space(0.5));
        if f.bin {
            return group
                .child(
                    word(
                        cx,
                        t,
                        "restore",
                        IconName::RotateCcw,
                        t!("Restore"),
                        labels,
                        t!("Restore: the selection back into the folder each was deleted from"),
                        None,
                    )
                    .disabled(f.selected == 0)
                    .on_click(cx.listener(|this, _, _, cx| this.restore(cx))),
                )
                .child(
                    tool(
                        cx,
                        t,
                        "delete-for-good",
                        IconName::Trash,
                        keyed("Delete for good ({0})", &[&actions::Delete]),
                        None,
                    )
                    .disabled(f.selected == 0)
                    .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::Delete), window, cx))),
                )
                .child(divider(t))
                .child(
                    word(
                        cx,
                        t,
                        "empty-bin",
                        IconName::Trash,
                        t!("Empty the Recycle Bin"),
                        labels,
                        t!("Empty the Recycle Bin: everything in it, on every drive, deleted for good (Windows asks first)"),
                        None,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CommandBarEvent::Menu(MenuEvent::EmptyRecycleBin)))),
                )
                .child(width_probe(&self.measured, Width::Left, fold, 0, cx.entity_id()));
        }
        let new_off = !f.in_folder || f.find || f.read_only;
        let mut group = group
            .child(
                word(
                    cx,
                    t,
                    "new-folder",
                    IconName::FolderPlus,
                    t!("New folder"),
                    labels,
                    keyed("New folder ({0})", &[&actions::NewFolder]),
                    None,
                )
                .disabled(new_off)
                .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::NewFolder), window, cx))),
            )
            .child(
                self.anchored(
                    "new",
                    arrow(
                        cx,
                        t,
                        "new-menu",
                        t!("New: a folder, a text document, or a kind of file your apps make (a Word document, a zip…)"),
                    )
                    .disabled(new_off)
                    // Tucked against New folder, as the TypeScript's -ms-1.
                    .ms(-1.)
                    .on_click(cx.listener(|this, _, window, cx| this.show_new(window, cx))),
                ),
            );
        if fold < MOST {
            group = group.child(divider(t));
            group = self.edit_buttons(group, t, cx);
        } else {
            group = group.child(
                self.anchored(
                    "edits",
                    tool(
                        cx,
                        t,
                        "edits",
                        IconName::Ellipsis,
                        t!("Cut, copy, paste, rename, delete"),
                        None,
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.show_edits(window, cx))),
                ),
            );
        }
        group = group.child(divider(t));
        // In an archive, or a folder Windows keeps closed, where the tools would be (they're for folders on disk
        // you may read): why the rest is greyed out.
        if f.read_only {
            group = group.child(self.read_only_note(labels, t, cx));
            if let Some(archive) = f.whole_archive.clone() {
                group = group.child(
                    word(
                        cx,
                        t,
                        "extract-all",
                        IconName::ArchiveRestore,
                        t!("Extract all"),
                        labels,
                        t!("Extract all: everything into a folder next to the archive"),
                        None,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let archive = archive.clone();
                        this.explorer.update(cx, |e, cx| e.extract_here(&archive, cx).detach());
                    })),
                );
            }
        } else {
            group = group.child(
                self.anchored(
                    "tools",
                    word(
                        cx,
                        t,
                        "tools",
                        IconName::Wrench,
                        t!("Tools"),
                        labels,
                        t!("Tools: Terminal, File X-Ray, what's locking, recent changes, duplicate files"),
                        None,
                    )
                    .dropdown_caret(true)
                    .on_click(cx.listener(|this, _, window, cx| this.show_tools(window, cx))),
                ),
            );
        }
        if !self.tasks.groups.is_empty() && f.can_run_tasks() {
            group = group.child(
                self.anchored(
                    "tasks",
                    word(
                        cx,
                        t,
                        "run",
                        IconName::Play,
                        t!("Run"),
                        labels,
                        t!("Run: this project's scripts, recipes and commands, in the terminal under the files"),
                        None,
                    )
                    .dropdown_caret(true)
                    .on_click(cx.listener(|this, _, window, cx| this.show_tasks(window, cx))),
                ),
            );
        }
        group.child(width_probe(&self.measured, Width::Left, fold, 0, cx.entity_id()))
    }

    /// Cut, Copy, Paste, the other pane's two, Compare, Rename and Delete.
    fn edit_buttons(&self, group: Stateful<Div>, t: &Tokens, cx: &mut Context<Self>) -> Stateful<Div> {
        let f = &self.facts;
        let none = f.selected == 0;
        let mut group = group
            .child(
                tool(
                    cx,
                    t,
                    "cut",
                    IconName::Scissors,
                    keyed("Cut ({0})", &[&actions::Cut]),
                    None,
                )
                .disabled(none || f.read_only)
                .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::Cut), window, cx))),
            )
            .child(
                tool(
                    cx,
                    t,
                    "copy",
                    IconName::Copy,
                    if f.read_only && !f.closed {
                        keyed("Copy ({0}): extracted, to paste anywhere", &[&actions::Copy])
                    } else {
                        keyed("Copy ({0})", &[&actions::Copy])
                    },
                    None,
                )
                .disabled(none)
                .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::Copy), window, cx))),
            )
            .child(
                tool(
                    cx,
                    t,
                    "paste",
                    IconName::ClipboardPaste,
                    keyed("Paste ({0})", &[&actions::Paste]),
                    None,
                )
                .disabled(!f.in_folder || f.find || f.read_only)
                .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::Paste), window, cx))),
            );
        if f.dual {
            group = group
                .child(
                    tool(
                        cx,
                        t,
                        "copy-other",
                        IconName::CopyPlus,
                        t!("Copy to the other pane"),
                        None,
                    )
                    .disabled(none || !f.other)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::CopyToOther, window, cx))),
                )
                .child(
                    tool(
                        cx,
                        t,
                        "move-other",
                        IconName::FolderOutput,
                        t!("Move to the other pane"),
                        None,
                    )
                    .disabled(none || !f.other || f.read_only)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::MoveToOther, window, cx))),
                );
        }
        if f.dual || f.pair.is_some() {
            let tip = match &f.pair {
                Some(pair) => keyed_with(
                    "Compare “{a}” and “{b}” ({0})",
                    &[&actions::CompareFiles],
                    &[("a", pair.left_name.as_str()), ("b", pair.right_name.as_str())],
                ),
                None => keyed(
                    "Compare files: select two files, or one file in each pane ({0})",
                    &[&actions::CompareFiles],
                ),
            };
            let pair = f.pair.clone();
            group = group.child(
                tool(cx, t, "compare", IconName::GitCompareArrows, tip, None)
                    .disabled(f.pair.is_none())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(pair) = &pair {
                            cx.emit(CommandBarEvent::Menu(MenuEvent::Compare {
                                left: pair.left.clone(),
                                right: pair.right.clone(),
                            }));
                        }
                    })),
            );
        }
        group
            .child(
                tool(
                    cx,
                    t,
                    "rename",
                    IconName::PenLine,
                    if f.selected > 1 {
                        keyed_with(
                            "Rename {n} items ({0})",
                            &[&actions::Rename],
                            &[("n", &f.selected.to_string())],
                        )
                    } else {
                        keyed(
                            "Rename ({0}) · {1} renames many",
                            &[&actions::Rename, &actions::BatchRename],
                        )
                    },
                    None,
                )
                .disabled(none || f.read_only)
                .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::Rename), window, cx))),
            )
            .child(
                tool(
                    cx,
                    t,
                    "delete",
                    IconName::Trash,
                    keyed(
                        "Delete ({0}) · {1} deletes permanently",
                        &[&actions::Delete, &actions::DeletePermanently],
                    ),
                    None,
                )
                .disabled(none || f.read_only)
                .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::Delete), window, cx))),
            )
    }

    /// "Read-only archive" or "No access": why the rest is greyed out.
    // NOTE(port): the TypeScript says "Read-only archive" for a remote connection's folder too, though its tooltip
    // speaks of archives; kept as it was.
    fn read_only_note(&self, labels: bool, t: &Tokens, _cx: &mut Context<Self>) -> impl IntoElement {
        let closed = self.facts.closed;
        let (icon_name, words, tip) = if closed {
            (
                IconName::FolderLock,
                t!("No access"),
                t!("Windows keeps this folder closed to your account: nothing in it can be opened or changed here. Get access from the folder itself, or its right-click menu."),
            )
        } else {
            (
                IconName::PencilOff,
                t!("Read-only archive"),
                t!("Glint opens archives read-only. Copy (Ctrl+C), copy to the other pane or drag items to get them out; to change what's in the archive, copy it out first."),
            )
        };
        let name = words.clone();
        let description = tip.clone();
        row()
            .id("read-only-note")
            .test_support()
            .role(Role::Group)
            .aria_label(name)
            .aria_description(description)
            .flex_none()
            .h(space(7.))
            .gap(space(1.))
            .px(space(2.))
            .rounded(theme::size::RADIUS)
            .bg(t.warning.opacity(0.1))
            .text_color(t.warning)
            .text_size(theme::size::XS)
            .font_weight(FontWeight::MEDIUM)
            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
            .child(icon(icon_name).small())
            .when(labels, |note| note.child(div().whitespace_nowrap().child(words)))
    }

    /// List, grid and columns, with the arrow for the grid's tile sizes.
    fn view_switch(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let f = &self.facts;
        // While the tab shows its disk usage map, neither is pressed and each goes back to the files.
        let mapped = f.mapped && !f.find;
        let off = !f.in_folder && !f.find && !f.bin;
        let size_name = glint_model::grid::TILE_SIZES
            .iter()
            .find(|spec| spec.size == f.tile_size)
            .map_or("Medium", |spec| spec.label);
        let sizes = t!(
            "Tile size: {size} (Ctrl+wheel, Ctrl+= and Ctrl+- in the grid)",
            size = glint_model::i18n::translate(i18n::language(), size_name)
        );
        let views = [
            (
                ViewMode::List,
                "view-list",
                IconName::List,
                keyed("List ({0})", &[&actions::ListView]),
            ),
            (
                ViewMode::Grid,
                "view-grid",
                IconName::LayoutGrid,
                keyed("Grid, with thumbnails ({0})", &[&actions::GridView]),
            ),
            (
                ViewMode::Columns,
                "view-columns",
                IconName::Columns3,
                keyed(
                    "Columns: the folder above, this one, and what's in the focused one ({0})",
                    &[&actions::ColumnsView],
                ),
            ),
        ];
        row()
            .id("command-bar-views")
            .test_support()
            .role(Role::Group)
            .aria_label(t!("View"))
            .flex_none()
            .p(space(0.5))
            .rounded(theme::size::RADIUS)
            .bg(t.foreground.opacity(0.05))
            .children(views.into_iter().map(|(view, id, glyph, name)| {
                let chosen = !mapped && f.view == view;
                tool(cx, t, id, glyph, name, Some(chosen))
                    .small()
                    .disabled(off)
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_view(view, cx)))
            }))
            .child(
                self.anchored(
                    "sizes",
                    arrow(cx, t, "tile-sizes", sizes)
                        .disabled(off)
                        .on_click(cx.listener(|this, _, window, cx| this.show_sizes(window, cx))),
                ),
            )
    }

    /// The group at the end edge: the views, the map, sizes and text search, the palette and the toggles.
    fn right_group(&self, fold: usize, t: &Tokens, cx: &mut Context<Self>) -> Stateful<Div> {
        let f = &self.facts;
        let labels = fold < 1;
        let mut group = row_start().id("command-bar-end").relative().flex_none().gap(space(0.5));
        if fold < 5 {
            group = group.child(self.view_switch(t, cx));
        }
        if fold < 4 {
            let off = !f.in_folder || f.find;
            group = group
                .child(
                    word(
                        cx,
                        t,
                        "disk-usage",
                        IconName::LayoutDashboard,
                        t!("Disk usage"),
                        labels,
                        if f.mapped {
                            t!("Back to the file list")
                        } else {
                            t!("Disk usage: a map of what fills this folder, from the instant search index when it has it, otherwise measured with pdu")
                        },
                        Some(f.mapped),
                    )
                    .disabled(off)
                    .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::DiskUsage), window, cx))),
                )
                .child(
                    word(
                        cx,
                        t,
                        "folder-sizes",
                        if self.sizing { IconName::LoaderCircle } else { IconName::HardDrive },
                        if self.sizing { t!("Stop") } else { t!("Folder sizes") },
                        fold < 2,
                        if self.sizing {
                            t!("Stop calculating")
                        } else {
                            t!("Size every folder here: from the instant search index when it has them, otherwise measured with pdu")
                        },
                        None,
                    )
                    .disabled(off)
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::FolderSizes, window, cx))),
                )
                .child(
                    word(
                        cx,
                        t,
                        "search-text",
                        IconName::TextSearch,
                        t!("Search text"),
                        labels,
                        keyed(
                            "Search inside the files in this folder (ripgrep) · {0}",
                            &[&actions::TextSearch],
                        ),
                        None,
                    )
                    .disabled(!f.in_folder || f.read_only)
                    .on_click(cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::TextSearch), window, cx))),
                );
        }
        if fold < 5 {
            group = group
                .child(
                    tool(
                        cx,
                        t,
                        "palette",
                        IconName::Command,
                        keyed("Command palette ({0})", &[&actions::CommandPalette]),
                        None,
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| this.dispatch(Box::new(actions::CommandPalette), window, cx)),
                    ),
                )
                .child(divider(t));
        }
        if fold < 3 {
            group = group
                .child(
                    tool(
                        cx,
                        t,
                        "hidden",
                        if f.show_hidden { IconName::Eye } else { IconName::EyeOff },
                        keyed("Show hidden items ({0})", &[&actions::ToggleHidden]),
                        Some(f.show_hidden),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Hidden, window, cx))),
                )
                .child(
                    tool(
                        cx,
                        t,
                        "preview",
                        IconName::PanelRight,
                        keyed("Preview pane ({0})", &[&actions::TogglePreview]),
                        Some(f.show_preview),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Preview, window, cx))),
                )
                .child(
                    tool(
                        cx,
                        t,
                        "dual",
                        IconName::Columns2,
                        keyed("Dual pane: two folders side by side ({0})", &[&actions::DualPane]),
                        Some(f.dual),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::Dual, window, cx))),
                )
                .child(
                    tool(
                        cx,
                        t,
                        "compare-panes",
                        IconName::GitCompareArrows,
                        keyed(
                            "Compare panes: highlight differences between both panes ({0})",
                            &[&actions::ComparePanes],
                        ),
                        Some(f.compare_panes),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.pick(Pick::ComparePanes, window, cx))),
                );
        } else {
            group = group.child(
                self.anchored(
                    "view",
                    ghost(cx, t, "view-menu", false, true)
                        .icon(IconName::SlidersHorizontal)
                        .dropdown_caret(true)
                        .accessibility_label(if fold >= 5 {
                            t!("View: list or grid, hidden items, preview pane, dual pane, compare panes")
                        } else {
                            t!("View: hidden items, preview pane, dual pane, compare panes")
                        })
                        .tooltip(if fold >= 5 {
                            t!("View: list or grid, hidden items, preview pane, dual pane, compare panes")
                        } else {
                            t!("View: hidden items, preview pane, dual pane, compare panes")
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.show_view(window, cx))),
                ),
            );
        }
        group.child(width_probe(&self.measured, Width::Right, fold, 0, cx.entity_id()))
    }
}

impl Render for CommandBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dirty {
            self.refresh(cx);
        }
        let t = theme::tokens(cx).clone();
        let rem = f32::from(window.rem_size());
        let signature = self.signature(rem);
        // What the last paint measured: the bar's width, and what the two groups need at the fold it was drawn at.
        let m = &self.measured;
        let pad = rem * 0.5 * 2. + rem * MIN_GAP_REMS;
        let measured = (m.left.get() > 0. && m.right.get() > 0.)
            .then(|| (m.signature.get(), m.fold.get(), m.left.get() + m.right.get() + pad));
        let fold = self.folding.settle(signature, m.bar.get(), measured);

        let left = self.left_group(fold, &t, cx);
        let right = self.right_group(fold, &t, cx);
        let id = cx.entity_id();
        let row = row_start()
            .id("command-bar-row")
            .relative()
            .size_full()
            .overflow_hidden()
            .px(space(2.))
            .child(left)
            // What is left over, so the groups sit at the two ends; never less than a little.
            .child(div().flex_1().min_w(rems(MIN_GAP_REMS)))
            .child(right)
            .child(width_probe(&self.measured, Width::Bar, fold, signature, id));
        // The menu is outside the row, which clips: it must not be cut at the bar's edge.
        let menu = self.menu.as_ref().map(|open| {
            let anchor = if i18n::is_rtl() {
                Anchor::TopRight
            } else {
                Anchor::TopLeft
            };
            deferred(
                anchored()
                    .position(open.at)
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(4.))
                    .child(open.view.clone()),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
        });
        div()
            .id("command-bar")
            .role(Role::Toolbar)
            .aria_label(t!("Commands"))
            .test_support()
            .key_context("CommandBar")
            .track_focus(&self.focus)
            .relative()
            .flex_none()
            .w_full()
            .h(space(HEIGHT))
            .bg(theme::chrome_fill(cx))
            .border_b_1()
            .border_color(t.border)
            .child(row)
            .children(menu)
    }
}
