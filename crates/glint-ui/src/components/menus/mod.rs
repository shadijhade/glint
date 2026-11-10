//! The item, background and folder menus, and Windows' own menu.
//! Ported from the menus in src/components/explorer/Explorer.tsx (`itemMenu`, `recycledMenu`, `folderMenu`,
//! `windowsMenu`, `keyboardMenu`), src/components/Menu.tsx and the parts of ActionParts.tsx the menus build on.
//!
//! # How the menus are made
//!
//! | TypeScript                                | Here                                                              |
//! |-------------------------------------------|-------------------------------------------------------------------|
//! | `MenuItem` (Menu.tsx)                     | [`Node`], [`Entry`] in [`spec`]                                   |
//! | `visibleItems`                            | [`visible_items`]                                                 |
//! | `useMenu`'s popover and keys              | [`MenuView`] ([`view`]), shown by [`MenuHost`]                    |
//! | `itemMenu` / `recycledMenu`               | [`item_menu`] ([`item`])                                          |
//! | `folderMenu`                              | [`folder_menu`] ([`folder`])                                      |
//! | `newMenuItems`, `openInItems`, `scriptItems` | `folder::new_items`, [`open_in_menu`], [`scripts_menu`]        |
//! | `onRowContextMenu`, `onFolderContextMenu`, `keyboardMenu`, `onPlaceContextMenu` | [`MenuHost::open_list_menu`], [`MenuHost::open_background_menu`], [`MenuHost::open_folder_menu`] |
//! | `windowsMenu`, `shellMenuFits`            | [`MenuHost::show_windows_menu`], [`shell_menu_fits`]              |
//!
//! A menu is data first ([`spec`]): `item_menu` and `folder_menu` read the items and the facts about their pane
//! and return a list, in the TypeScript's order and with its conditions, and the tests read that list. [`MenuHost`]
//! reads the explorer and the saved state to fill in the facts, shows the list in a [`MenuView`] at a point in
//! the window (an anchored, deferred layer that closes on Escape, a press outside, or a choice), and runs what is
//! chosen.
//!
//! # What choosing an item does
//!
//! - **File operations are dispatched as workspace actions** (Copy, Cut, Paste, Delete, Rename, Undo,
//!   Properties, Copy path, Quick Look, Open, New folder, Select all, Refresh) to what had the keyboard when the
//!   menu opened, so the workspace's own handlers do them, with their undo and confirmations. The key a menu
//!   shows beside an item is the one `actions::BINDINGS` gives that action.
//! - **Navigation is done here**, with the explorer: Open, Open in new tab(s), Open in the other pane, Get
//!   access, Refresh on This PC, and Pin. So are a few shell calls with nothing more to them: Copy path, Open in
//!   Terminal, Open with…, Run as administrator, Properties of a folder, Open in an editor, Open in File Explorer.
//! - **"Show more options"** (and Shift+right-click) shows Windows' own menu through
//!   `glint_core::shellmenu::shell_menu`, posted to the UI thread's message window by `core_bridge::run`, never
//!   inside a GPUI callback. Rename and Delete come back from it and run as Glint's own, with Undo.
//! - **Everything whose part isn't built** (share, compress, inspect, scripts, tags, the dialogs of the tools)
//!   is a [`MenuEvent`] naming what was chosen and for which paths. [`MenuHost`] emits it.
//!
//! # Wiring (the coordinator)
//!
//! 1. Make the host once per window, keep it, and draw it as the **last child of the window's root** so the
//!    menu is above everything: `let menus = cx.new(MenuHost::new);` ... `.child(self.menus.clone())`.
//! 2. Subscribe to it for [`MenuEvent`]s and hand each to its part.
//! 3. Call it from the views' events:
//!    - `FileListEvent::ContextMenu { target, at, shift }` -> `menus.update(cx, |m, cx| m.open_list_menu(pane, target.as_deref(), at, shift, window, cx))`.
//!      `None` for `at` (the keyboard) puts the menu at the middle of the window until the list can say where its
//!      focused row is (see the `PORT ME` in [`MenuHost::anchor`]).
//!    - `FileGridEvent::ContextMenu { path, position }` -> `open_list_menu(pane, Some(&path), Some(position), false, ..)`.
//!    - `FileGridEvent::BackgroundContextMenu { position }` -> `open_background_menu(pane, Some(position), ..)`.
//!    - `ThisPcEvent::ContextMenu { path, position }` -> `open_folder_menu(&path, position, ..)`.
//!    - The sidebar has a menu of its own (`components/sidebar/menu.rs`) with the places' items; the shared one is
//!      `open_folder_menu`, once `SidebarEvent` can ask for it.
//! 4. Keep `MenuHost::update_data` (tags, on-demand connections) current from the parts that own them. What Undo
//!    would take back is read from the explorer (`next_undo`) each time a folder menu opens.

mod data;
mod event;
mod folder;
mod item;
/// What kind of item each command is offered for (also the Ctrl+K palette's conditions).
pub(crate) mod kinds;
mod spec;
#[cfg(test)]
mod tests;
mod view;

pub use data::{Device, MenuData, Sources};
pub use event::{CloudPin, CompressFormat, MediaTool, MenuEvent, PdfTool};
pub use folder::{folder_menu, undo_label, Connection, FolderCtx, FolderView};
pub use item::{item_menu, open_in_menu, scripts_menu, shell_menu_fits, ItemCtx};
pub use kinds::on_disk;
pub use spec::{key_text, visible_items, Command, Entry, Node, Run, Task};
pub use view::{MenuView, MenuViewEvent};

use crate::core_bridge::{self, Core};
use crate::explorer::{try_explorer, NavOptions};
use crate::kit::prelude::*;
use crate::{i18n, persistence, state, t};
use glint_core::host::Manager as _;
use glint_model::entries::FileEntry;
use glint_model::paths::{base_name, is_folder_path, is_remote_path, is_root, remote_id_of};
use glint_model::store::RemoteKind;
use gpui_kit::{anchored, deferred, Anchor};
use std::time::{Duration, Instant};

/// How long the lists read from glint-core are reused: a menu opened sooner doesn't read them again.
const FRESH_FOR: Duration = Duration::from_secs(10);

/// A call that would have shown Windows' own menu: the paths and the point (tests).
#[cfg(test)]
pub(crate) type ShellCall = (Vec<String>, Point<Pixels>);

/// A menu on screen: where it is, what it is made of, and what had the keyboard before it.
struct Open {
    view: Entity<MenuView>,
    /// The point it was asked for, in window coordinates.
    at: Point<Pixels>,
    /// The pane it is for, for Open in the other pane.
    pane: usize,
    /// What had the keyboard: the file list, so a chosen command is dispatched to it and the keys return to it.
    focus: Option<FocusHandle>,
    _subscription: Subscription,
}

/// Shows the menus of the explorer window and runs what is chosen. One per window, drawn last in its root.
pub struct MenuHost {
    open: Option<Open>,
    data: MenuData,
    sources: Sources,
    read_at: Option<Instant>,
    /// Tests: stands in for Windows' own menu, which can't be shown headless.
    #[cfg(test)]
    pub(crate) shell_calls: std::rc::Rc<std::cell::RefCell<Vec<ShellCall>>>,
}

impl EventEmitter<MenuEvent> for MenuHost {}

impl MenuHost {
    /// A host that reads the editors, scripts and New kinds from glint-core.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::with_sources(Sources::core(), cx)
    }

    /// A host reading its lists from `sources` (tests give fakes).
    pub fn with_sources(sources: Sources, cx: &mut Context<Self>) -> Self {
        let mut host = MenuHost {
            open: None,
            data: MenuData::default(),
            sources,
            read_at: None,
            #[cfg(test)]
            shell_calls: Default::default(),
        };
        host.read_lists(true, cx);
        host
    }

    /// Whether a menu is on screen.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The menu on screen, for tests and screen-reader checks.
    pub fn view(&self) -> Option<&Entity<MenuView>> {
        self.open.as_ref().map(|o| &o.view)
    }

    /// What the menus read besides the explorer.
    pub fn data(&self) -> &MenuData {
        &self.data
    }

    /// Changes what the menus read (tags, on-demand connections), for the parts that own them.
    pub fn update_data(&mut self, change: impl FnOnce(&mut MenuData)) {
        change(&mut self.data);
    }

    /// Closes the menu without choosing anything.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.emit(MenuEvent::Closed);
            cx.notify();
        }
    }

    /// Reads the editors, scripts and New kinds in the background (at most every ten seconds unless `force`).
    fn read_lists(&mut self, force: bool, cx: &mut Context<Self>) {
        if !force && self.read_at.is_some_and(|at| at.elapsed() < FRESH_FOR) {
            return;
        }
        self.read_at = Some(Instant::now());
        let (openers, scripts, kinds) = (
            (self.sources.openers)(),
            (self.sources.scripts)(),
            (self.sources.new_kinds)(),
        );
        cx.spawn(async move |this, cx| {
            let openers = core_bridge::run(openers).await.and_then(|r| r);
            let scripts = core_bridge::run(scripts).await.and_then(|r| r);
            let kinds = core_bridge::run(kinds).await.and_then(|r| r);
            let _ = this.update(cx, |this, _| {
                // A read that failed leaves the list as it was.
                if let Ok(list) = openers {
                    this.data.openers = list;
                }
                if let Ok(list) = scripts {
                    this.data.scripts = list;
                }
                if let Ok(list) = kinds {
                    this.data.new_kinds = list;
                }
            });
        })
        .detach();
    }

    // ---- Opening ----------------------------------------------------------------------------------------

    /// Where a menu asked for with no point goes: the middle of the window, a third of the way down (the
    /// TypeScript's fallback when there is no list).
    // PORT ME(coordinator): the keyboard's menu belongs at the focused row (`keyboardMenu` put it at the row's
    // bottom edge, 24 px in). The file list and grid can't say where that row is yet; when they can, pass it as
    // `at` and this fallback is only for a window with no list.
    fn anchor(at: Option<Point<Pixels>>, window: &Window) -> Point<Pixels> {
        at.unwrap_or_else(|| {
            let size = window.viewport_size();
            point(size.width * 0.5, size.height / 3.)
        })
    }

    /// The right-click on a row or on the empty part of the list or grid, or the menu key or Shift+F10 (from
    /// `FileListEvent::ContextMenu`; the grid passes its tile and position).
    ///
    /// `target` is the item pressed (selected already) or the one the keyboard is on; `at` is the pointer, or None
    /// for the keyboard. What is selected gets its menu (the item pressed alone when it isn't part of the
    /// selection); with nothing selected the folder's own menu shows. `shift` is Shift+right-click: Windows' own
    /// menu at once, as in File Explorer, when it has these items.
    pub fn open_list_menu(
        &mut self,
        pane: usize,
        target: Option<&str>,
        at: Option<Point<Pixels>>,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = Self::anchor(at, window);
        let chosen = self.chosen(pane, target, cx);
        if chosen.is_empty() {
            return self.open_background_menu(pane, Some(at), window, cx);
        }
        // Shift+right-click: Windows' own menu at once.
        if shift && shell_menu_fits(&chosen) {
            let paths = chosen.iter().map(|e| e.path.clone()).collect();
            return self.show_windows_menu(paths, at, window, cx);
        }
        let mut ctx = self.item_ctx(pane, cx);
        ctx.data.read_tags(cx, chosen.iter().map(|e| e.path.as_str()));
        let nodes = item_menu(&chosen, &ctx);
        self.show(nodes, at, pane, t!("Item menu"), window, cx);
    }

    /// The item menu for `entries` as they are, at `at`: items the tab's rows don't hold, such as the disk usage
    /// map's items deeper than its folder. With none, the folder's own menu.
    pub fn open_entries_menu(
        &mut self,
        pane: usize,
        entries: Vec<FileEntry>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entries.is_empty() {
            return self.open_background_menu(pane, Some(at), window, cx);
        }
        let mut ctx = self.item_ctx(pane, cx);
        ctx.data.read_tags(cx, entries.iter().map(|e| e.path.as_str()));
        let nodes = item_menu(&entries, &ctx);
        self.show(nodes, at, pane, t!("Item menu"), window, cx);
    }

    /// The folder's own menu: right-click on the empty part of the list, the grid, or a folder with nothing
    /// selected for the keyboard (`backgroundMenu`).
    pub fn open_background_menu(
        &mut self,
        pane: usize,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = Self::anchor(at, window);
        let Some(explorer) = try_explorer(cx) else { return };
        let (folder, view) = {
            let ex = explorer.read(cx);
            let Some(tab) = ex.pane_tab(pane) else { return };
            let restore_shown = (tab.is_recycle_bin() && !tab.rows.is_empty()).then(|| {
                let label = if tab.rows.len() == tab.entries().len() {
                    t!("Restore all items")
                } else {
                    t!(
                        "Restore the {n} items shown",
                        n = glint_model::format::format_count(tab.rows.len())
                    )
                };
                (label, tab.rows.iter().map(|e| e.path.clone()).collect())
            });
            (
                tab.path.clone(),
                FolderView {
                    find: tab.find.is_some(),
                    archive: tab.archive().cloned(),
                    no_access: tab.no_access().copied(),
                    from_index: tab.listing.as_ref().is_some_and(|l| l.from_index),
                    tab: Some(tab.id),
                    restore_shown,
                },
            )
        };
        let mut ctx = self.folder_ctx(&folder, cx);
        ctx.view = Some(view);
        let nodes = folder_menu(&folder, true, &ctx);
        self.show(nodes, at, pane, t!("Folder menu"), window, cx);
    }

    /// The menu of a folder that isn't the one shown: a tile of This PC, a place in the sidebar
    /// (`onPlaceContextMenu`).
    pub fn open_folder_menu(&mut self, folder: &str, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let pane = try_explorer(cx).map_or(0, |e| e.read(cx).active_pane());
        let ctx = self.folder_ctx(folder, cx);
        let nodes = folder_menu(folder, false, &ctx);
        self.show(nodes, at, pane, t!("Folder menu"), window, cx);
    }

    /// Windows' own menu for `paths` (all in one folder) at `at` in the window: File Explorer's "Show more
    /// options". The shell draws and runs it, on the UI thread's message loop, outside every GPUI callback;
    /// Rename and Delete come back and run as Glint's own commands.
    pub fn show_windows_menu(
        &mut self,
        paths: Vec<String>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(cx);
        #[cfg(test)]
        self.shell_calls.borrow_mut().push((paths.clone(), at));
        // Headless tests can't show the shell's menu.
        if cfg!(test) {
            return;
        }
        let Some(main) = main_window(cx) else {
            window.push_notification(
                Notification::error(t!("Windows didn't answer")).title(t!("Couldn't show Windows' menu")),
                cx,
            );
            return;
        };
        let focus = window.focused(cx);
        let (x, y) = (f64::from(f32::from(at.x)), f64::from(f32::from(at.y)));
        let many = paths.len() > 1;
        cx.spawn_in(window, async move |_, cx| {
            let verb = core_bridge::run(glint_core::shellmenu::shell_menu(main, paths, x, y))
                .await
                .and_then(|inner| inner);
            let _ = cx.update(|window, cx| match verb {
                // Rename and Delete are Glint's own, with Undo.
                Ok(verb) if verb == "rename" || verb == "delete" => {
                    if let Some(focus) = &focus {
                        focus.focus(window, cx);
                    }
                    let command = match (verb.as_str(), many) {
                        ("rename", true) => Command::BatchRename,
                        ("rename", false) => Command::Rename,
                        _ => Command::Delete,
                    };
                    window.dispatch_action(command.action(), cx);
                }
                Ok(_) => {}
                Err(reason) => {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't show Windows' menu")), cx)
                }
            });
        })
        .detach();
    }

    /// The items to act on: what is selected in `pane`, in the order shown, or the item named when it isn't
    /// part of the selection (`selectedNow()` and the right-click's rule for `targets`).
    fn chosen(&self, pane: usize, target: Option<&str>, cx: &App) -> Vec<FileEntry> {
        let Some(explorer) = try_explorer(cx) else {
            return Vec::new();
        };
        let ex = explorer.read(cx);
        let Some(tab) = ex.pane_tab(pane) else {
            return Vec::new();
        };
        let mut chosen: Vec<FileEntry> = if tab.selection.is_empty() {
            Vec::new()
        } else {
            tab.rows
                .iter()
                .filter(|e| tab.selection.contains(&e.path))
                .cloned()
                .collect()
        };
        if let Some(path) = target {
            if !chosen.iter().any(|e| e.path == path) {
                chosen = tab.rows.iter().find(|e| e.path == path).cloned().into_iter().collect();
            }
        }
        chosen
    }

    /// The facts about `pane` that decide the item menu's items.
    fn item_ctx(&self, pane: usize, cx: &App) -> ItemCtx {
        let mut ctx = ItemCtx {
            pane,
            data: self.data.clone(),
            ..ItemCtx::default()
        };
        let settings = state::settings(cx);
        ctx.editor_path = settings.editor_path.clone();
        ctx.editor_name = editor_name(&settings.editor_path);
        if let Some(app) = persistence::app(cx) {
            ctx.pinned = app.pinned.clone();
            ctx.devices = devices(&app.remotes);
        }
        let Some(explorer) = try_explorer(cx) else { return ctx };
        let ex = explorer.read(cx);
        if let Some(tab) = ex.pane_tab(pane) {
            ctx.folder = tab.path.clone();
            ctx.find = tab.find.is_some();
            ctx.archive = tab.archive().cloned();
        }
        ctx.dual = ex.is_dual();
        if ctx.dual {
            let other = ex
                .panes()
                .get(1usize.saturating_sub(ex.active_pane()).min(1))
                .filter(|_| ex.panes().len() > 1)
                .and_then(|p| ex.tab(p.active));
            if let Some(tab) = other {
                if tab.ready
                    && is_folder_path(&tab.path)
                    && tab.find.is_none()
                    && tab.archive().is_none()
                    && tab.no_access().is_none()
                {
                    let name = ex.tab_name(&tab.path);
                    ctx.other_folder = Some(glint_model::i18n::t(i18n::language(), &name, &[]).into_owned());
                }
                ctx.other_selected = tab
                    .rows
                    .iter()
                    .filter(|e| tab.selection.contains(&e.path) && !e.dir && kinds::on_disk(e))
                    .cloned()
                    .collect();
                if tab.archive().is_none() {
                    ctx.other_listing = tab.shared_entries().map(|entries| (tab.path.clone(), entries));
                }
            }
        }
        ctx
    }

    /// The facts about `folder` that decide the folder menu's items (not its view, which only a pane has).
    fn folder_ctx(&self, folder: &str, cx: &App) -> FolderCtx {
        let mut ctx = FolderCtx {
            data: self.data.clone(),
            ..FolderCtx::default()
        };
        // What Undo would take back, said in the menu's Undo.
        ctx.data.undo = try_explorer(cx).and_then(|e| e.read(cx).next_undo().map(|(_, label)| label.to_string()));
        if let Some(app) = persistence::app(cx) {
            ctx.pinned = app.pinned.clone();
            // The top of a remote connection can be edited and removed.
            if is_remote_path(folder) && is_root(folder) {
                let id = remote_id_of(folder).to_lowercase();
                ctx.remote = app
                    .remotes
                    .iter()
                    .find(|r| r.id.to_lowercase() == id)
                    .map(|r| Connection {
                        id: r.id.clone(),
                        name: r.name.clone(),
                        device: r.kind == RemoteKind::Device,
                    });
            }
        }
        ctx
    }

    /// Shows `nodes` at `at`, taking the keyboard.
    fn show(
        &mut self,
        nodes: Vec<Node>,
        at: Point<Pixels>,
        pane: usize,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if nodes.is_empty() {
            self.close(cx);
            return;
        }
        self.read_lists(false, cx);
        let focus = window.focused(cx);
        let view = cx.new(|cx| MenuView::new(nodes, label, cx));
        view.update(cx, |view, cx| view.focus(window, cx));
        let subscription = cx.subscribe_in(&view, window, |this, _, event: &MenuViewEvent, window, cx| {
            this.on_view(event.clone(), window, cx);
        });
        self.open = Some(Open {
            view,
            at,
            pane,
            focus,
            _subscription: subscription,
        });
        cx.emit(MenuEvent::Shown);
        cx.notify();
    }

    fn on_view(&mut self, event: MenuViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else { return };
        cx.emit(MenuEvent::Closed);
        cx.notify();
        match event {
            MenuViewEvent::Chose(run) => {
                if let Some(focus) = &open.focus {
                    focus.focus(window, cx);
                }
                self.run(run, open.pane, open.at, window, cx);
            }
            MenuViewEvent::Dismissed => {
                // Escape gives the keyboard back; a press outside lets the thing pressed have it, unless that
                // took nothing.
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

    // ---- Doing ------------------------------------------------------------------------------------------

    /// Runs what was chosen.
    pub fn run(&mut self, run: Run, pane: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        match run {
            Run::Command(command) => window.dispatch_action(command.action(), cx),
            Run::Do(task) => self.perform(task, pane, at, window, cx),
            Run::Ask(event) => cx.emit(event),
            Run::Nothing => {}
        }
    }

    fn perform(&mut self, task: Task, _pane: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let explorer = try_explorer(cx);
        match task {
            Task::Navigate(path) => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx));
                }
            }
            Task::OpenInNewTabs(paths) => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| {
                        for path in &paths {
                            e.open_in_new_tab(path, cx);
                        }
                    });
                }
            }
            Task::OpenInOtherPane { path, from } => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| e.open_in_other_pane(&path, Some(from), cx).detach());
                }
            }
            Task::CopyText { text, paths } => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                window.push_notification(
                    Notification::success(if paths == 1 {
                        t!("Copied path")
                    } else {
                        t!("Copied paths")
                    }),
                    cx,
                );
            }
            Task::TogglePin(path) => persistence::update_app(cx, |app| app.toggle_pin(&path)),
            Task::RefreshDrives => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| e.refresh_drives(cx).detach());
                }
            }
            Task::GetAccess(tab) => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| e.get_access(tab, cx).detach());
                }
            }
            Task::WindowsMenu { paths } => self.show_windows_menu(paths, at, window, cx),
            Task::OpenTerminal(path) => {
                let work = glint_core::shell::open_terminal(path);
                Self::report(t!("Couldn't open a terminal"), work, window, cx);
            }
            Task::OpenWith(path) => match main_window(cx) {
                Some(main) => {
                    let work = glint_core::shell::open_with(main, path);
                    Self::report(t!("Couldn't open the app picker"), work, window, cx);
                }
                None => Self::unanswered(t!("Couldn't open the app picker"), window, cx),
            },
            Task::RunAsAdmin(path) => match main_window(cx) {
                Some(main) => {
                    let work = glint_core::shell::run_as_admin(main, path);
                    Self::report(t!("Couldn't run it as administrator"), work, window, cx);
                }
                None => Self::unanswered(t!("Couldn't run it as administrator"), window, cx),
            },
            Task::ShowProperties(path) => match main_window(cx) {
                Some(main) => {
                    let work = glint_core::shell::show_properties(main, path);
                    Self::report(t!("Couldn't show the properties"), work, window, cx);
                }
                None => Self::unanswered(t!("Couldn't show the properties"), window, cx),
            },
            Task::OpenIn { opener, name, path } => {
                let work = glint_core::openers::open_in(opener, path);
                Self::report(t!("Couldn't open it in {name}", name = name), work, window, cx);
            }
            Task::OpenInFileExplorer(path) => {
                // Shown by File Explorer's own "open" verb, which can take a moment: off the UI thread.
                cx.background_spawn(async move { crate::platform::launch::hand_to_explorer(&[path]) })
                    .detach();
            }
        }
    }

    /// Runs a core call without waiting, and tells the person `what` failed if it did.
    fn report(
        what: SharedString,
        work: impl std::future::Future<Output = Result<(), String>> + Send + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |_, cx| {
            let result = core_bridge::run(work).await.and_then(|inner| inner);
            if let Err(reason) = result {
                let _ = cx.update(|window, cx| {
                    window.push_notification(Notification::error(reason).title(what), cx);
                });
            }
        })
        .detach();
    }

    /// Windows' window isn't there to be the owner of a shell dialog.
    fn unanswered(what: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        window.push_notification(Notification::error(t!("Windows didn't answer")).title(what), cx);
    }
}

/// The window shell dialogs use as their owner (`getCurrentWindow()`).
fn main_window(cx: &App) -> Option<glint_core::host::Window> {
    let core = cx.try_global::<Core>()?;
    core.app
        .get_webview_window(crate::workspace::MAIN)
        .or_else(|| core.app.webview_windows().into_values().next())
}

/// An editor's name for "Open with <name>": its file's name without ".exe" (`editorName`).
fn editor_name(editor_path: &str) -> String {
    let name = base_name(editor_path.trim());
    let lower = name.to_lowercase();
    if lower.ends_with(".exe") {
        name.get(..name.len().saturating_sub(4)).unwrap_or(name).to_string()
    } else {
        name.to_string()
    }
}

/// The paired PCs files can be sent to (`pairedDevices()`).
fn devices(remotes: &[glint_model::store::RemoteConnection]) -> Vec<Device> {
    remotes
        .iter()
        .filter(|r| r.kind == RemoteKind::Device && r.host.is_some())
        .map(|r| Device {
            id: r.id.clone(),
            name: r.name.clone(),
        })
        .collect()
}

impl Render for MenuHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The menu is drawn in a deferred layer over the whole window, positioned at its point. In the
        // Arabic interface its end edge meets the point, so it opens toward the start, as Windows' menus do.
        let anchor = if i18n::is_rtl() {
            Anchor::TopRight
        } else {
            Anchor::TopLeft
        };
        div().when_some(self.open.as_ref(), |layer, open| {
            layer.child(
                deferred(
                    anchored()
                        .position(open.at)
                        .anchor(anchor)
                        .snap_to_window_with_margin(px(4.))
                        .child(open.view.clone()),
                )
                .with_priority(gpui_kit::base::POPUP_PRIORITY),
            )
        })
    }
}
