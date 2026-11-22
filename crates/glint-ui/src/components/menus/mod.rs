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
//!      `None` for `at` puts the menu at the middle of the window (the list passes the focused row's bottom edge
//!      for the keyboard, see [`MenuHost::anchor`]).
//!    - `FileGridEvent::ContextMenu { path, position, shift }` -> `open_list_menu(pane, Some(&path), Some(position), shift, ..)`.
//!    - `FileGridEvent::BackgroundContextMenu { position }` -> `open_background_menu(pane, Some(position), ..)`.
//!    - `ThisPcEvent::ContextMenu { path, position }` -> `open_folder_menu(&path, position, ..)`.
//!    - The sidebar has a menu of its own (`components/sidebar/menu.rs`) with the places' items; the shared one is
//!      `open_folder_menu`, once `SidebarEvent` can ask for it.
//! 4. Keep `MenuHost::update_data` (tags, on-demand connections) current from the parts that own them. What Undo
//!    would take back is read from the explorer (`next_undo`) each time a folder menu opens.

mod data;
mod event;
/// Finding a menu's rows by typing.
pub mod filter;
mod folder;
/// What a menu's head says.
pub mod head;
mod item;
/// What kind of item each command is offered for (also the Ctrl+K palette's conditions).
pub(crate) mod kinds;
/// "Move or copy to": the folders this kind of thing usually goes to.
pub mod send_to;
/// The Suggested section, and every row's key.
pub mod smart;
mod spec;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_content;
#[cfg(test)]
mod tests_host;
#[cfg(test)]
mod tests_send_to;
#[cfg(test)]
mod tests_smart;
#[cfg(test)]
mod tests_view;
mod view;

pub use data::{MenuData, Sources};
pub use event::{CloudPin, CompressFormat, MediaTool, MenuEvent, PdfTool};
pub use folder::{folder_menu, undo_label, Connection, FolderCtx, FolderView};
pub use item::{item_menu, open_in_menu, scripts_menu, shell_menu_fits, ItemCtx};
pub use kinds::on_disk;
pub use spec::{key_text, visible_items, Badge, Command, Entry, Head, HeadPicture, Node, Run, Task, Tone};
pub use view::{MenuView, MenuViewEvent};

use crate::components::git;
use crate::core_bridge::{self, Core};
use crate::explorer::{try_explorer, NavOptions};
use crate::kit::prelude::*;
use crate::{aware, i18n, persistence, state, t};
use glint_core::host::Manager as _;
use glint_model::aware::{Facts, Pick, Probed, Shape};
use glint_model::entries::{is_archived, is_closed, is_recycled, is_remote, FileEntry, GitMark};
use glint_model::paths::{base_name, is_folder_path, is_remote_path, is_root, remote_id_of};
use gpui_kit::{anchored, deferred, Anchor};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long the lists read from glint-core are reused: a menu opened sooner doesn't read them again.
const FRESH_FOR: Duration = Duration::from_secs(10);
/// How long a menu for one item waits for what is still being found out about it (who has it open, where it
/// came from, who signed it), so that what it suggests is there from its first frame and never moves. Two or
/// three frames: nobody sees it, and a menu whose item is already known doesn't wait at all.
const LOOK_FIRST: Duration = Duration::from_millis(40);

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
    /// What the items are (what choosing is counted under).
    shape: Shape,
    /// The items it is for (none for a folder's menu): the head follows what is found out about the one.
    targets: Vec<FileEntry>,
    /// The open folder's head, for the folder's own menu of the folder shown: it follows what is found out about
    /// the folder.
    folder: Option<FolderHead>,
    _subscription: Subscription,
    /// Hears Aware's answers while the menu is open.
    _aware: Option<Subscription>,
}

/// A menu ready to be shown.
struct Shown {
    nodes: Vec<Node>,
    at: Point<Pixels>,
    pane: usize,
    label: SharedString,
    shape: Shape,
    targets: Vec<FileEntry>,
    folder: Option<FolderHead>,
}

/// What a waiting menu is for.
enum Awaited {
    /// The menu of these items (one of them).
    Items(Vec<FileEntry>),
    /// The folder's own menu: the folder that is open.
    Folder(FileEntry),
}

impl Awaited {
    /// The one thing whose answers are waited for.
    fn entry(&self) -> Option<&FileEntry> {
        match self {
            Awaited::Items(chosen) => match chosen.as_slice() {
                [only] => Some(only),
                _ => None,
            },
            Awaited::Folder(folder) => Some(folder),
        }
    }
}

/// What the head of the folder's own menu follows while the menu is open.
struct FolderHead {
    /// The open folder, as Aware keeps what it finds out about it.
    entry: FileEntry,
    title: SharedString,
    items: Option<usize>,
    branch: Option<SharedString>,
}

/// A menu asked for that waits its breath ([`LOOK_FIRST`]) for what is being found out about its one item.
struct Waiting {
    /// Which asking it is: the menu opens only if no other was asked for since.
    turn: u64,
    pane: usize,
    what: Awaited,
    at: Point<Pixels>,
    /// Hears Aware's answers, so the menu opens the moment everything it waits for is known.
    _wake: Subscription,
}

/// The window's menu host, kept where the commands' island can ask it what it would suggest.
struct SharedHost(WeakEntity<MenuHost>);

impl Global for SharedHost {}

/// The window's menu host, once it has been made.
/// Whether a menu is on screen or about to be: what is under it (a row, a tile) opens no tooltip then.
pub fn showing(cx: &App) -> bool {
    host(cx).is_some_and(|host| host.read(cx).is_showing())
}

pub fn host(cx: &App) -> Option<Entity<MenuHost>> {
    cx.try_global::<SharedHost>().and_then(|shared| shared.0.upgrade())
}

/// A row put forward for what is selected in a pane (the island's next step).
#[derive(Clone, Debug, PartialEq)]
pub struct Suggested {
    /// The row, made to stand alone, with its reason.
    pub entry: Entry,
    /// Its key (`Run::key`).
    pub key: String,
    /// What the selection is: what choosing it is counted under.
    pub shape: glint_model::aware::Shape,
}

/// Shows the menus of the explorer window and runs what is chosen. One per window, drawn last in its root.
pub struct MenuHost {
    open: Option<Open>,
    data: MenuData,
    sources: Sources,
    read_at: Option<Instant>,
    /// Counts the menus asked for: one that waited a breath opens only if no other was asked for meanwhile.
    opening: u64,
    /// A menu was asked for and waits its breath ([`LOOK_FIRST`]): it is as good as open to what is under it.
    waiting: Option<Waiting>,
    /// Tests: stands in for Windows' own menu, which can't be shown headless.
    #[cfg(test)]
    pub(crate) shell_calls: std::rc::Rc<std::cell::RefCell<Vec<ShellCall>>>,
}

/// The folder at `path` as Aware keeps what it finds out about a folder that no list has an entry for: Aware
/// asks about a folder by its path, and so need not know its modified time.
fn folder_entry(path: &str) -> FileEntry {
    let name = base_name(path).to_string();
    FileEntry {
        lower: name.to_lowercase(),
        name,
        path: path.to_string(),
        dir: true,
        size: 0,
        modified: 0,
        attrs: glint_model::entries::attr::DIRECTORY,
        ext_at: glint_model::entries::NO_EXT,
        origin: None,
    }
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
            opening: 0,
            waiting: None,
            #[cfg(test)]
            shell_calls: Default::default(),
        };
        host.read_lists(true, cx);
        cx.set_global(SharedHost(cx.weak_entity()));
        host
    }

    /// The rows Suggested would hold for the selection of `pane`, best first, at most `most`. None for no
    /// selection, or one of more than 200 items.
    pub fn suggestions(&self, pane: usize, most: usize, cx: &App) -> Vec<Suggested> {
        let chosen = self.chosen(pane, None, cx);
        if chosen.is_empty() || chosen.len() > aware::MOST_TO_LOOK_AT {
            return Vec::new();
        }
        let probed = Self::probed_of(&chosen, cx);
        let mut ctx = self.item_ctx(pane, cx);
        ctx.probed = probed.clone();
        let nodes = item_menu(&chosen, &ctx);
        let facts = aware::facts(cx, &chosen);
        Self::picks(&nodes, &facts, probed.as_ref(), most, cx)
            .into_iter()
            .filter_map(|pick| {
                let mut entry = smart::row_for(&nodes, &pick.key)?;
                entry.reason = smart::reason(&pick.why, facts.shape);
                Some(Suggested {
                    entry,
                    key: pick.key,
                    shape: facts.shape,
                })
            })
            .collect()
    }

    /// What is known about the one item, if there is one and Aware has it.
    fn probed_of(chosen: &[FileEntry], cx: &App) -> Option<Probed> {
        let [entry] = chosen else { return None };
        aware::try_shared(cx)?.read(cx).probed(entry).cloned()
    }

    /// The rows to put forward for `facts`: copies of rows `nodes` holds.
    fn picks(nodes: &[Node], facts: &Facts, probed: Option<&Probed>, most: usize, cx: &App) -> Vec<Pick> {
        let keys = smart::keys(nodes);
        let available: Vec<&str> = keys.iter().map(String::as_str).collect();
        aware::suggest(cx, facts, probed, &available, most)
    }

    /// `nodes` with their head and Suggested, when the app has its Aware entity and the menu is more than a row or
    /// two; as they were otherwise.
    fn dressed(
        nodes: Vec<Node>,
        head: impl FnOnce() -> Head,
        facts: &Facts,
        probed: Option<&Probed>,
        extra: Vec<Node>,
        cx: &App,
    ) -> Vec<Node> {
        if aware::try_shared(cx).is_none() || nodes.len() < 4 {
            return nodes;
        }
        // Suggested fills what the top level leaves of 14 rows, at most two.
        let room = 14usize.saturating_sub(smart::top_rows(&nodes)).min(smart::MOST);
        let picks = Self::picks(&nodes, facts, probed, smart::MOST + 4, cx);
        let picks = &picks[..];
        // `extra` is what isn't a copy of a row but is worth putting forward: the usual place.
        smart::dress_with(nodes, head(), picks, facts.shape, extra, room)
    }

    /// What the head needs besides the items and the answers: the time, and Git's word for the one item.
    fn extras(one: Option<&FileEntry>, cx: &App) -> head::Extras {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let git_word = one.and_then(|entry| match git::mark_of(cx, &entry.path) {
            Some(GitMark::Untracked) => Some(t!("New")),
            _ if git::has_changes_view(cx, entry) => Some(t!("Changed")),
            _ => None,
        });
        // What the one file's checksums say about the checksum on the clipboard.
        let checksum = one.and_then(|entry| {
            let aware = aware::try_shared(cx)?;
            let aware = aware.read(cx);
            glint_model::aware::verdict(aware.probed(entry)?.sums.as_ref()?, aware.copied()?)
        });
        head::Extras {
            now: i64::try_from(now).unwrap_or(i64::MAX),
            git: git_word,
            checksum,
        }
    }

    /// The item menu for `chosen`, dressed, and what they are.
    fn item_nodes(chosen: &[FileEntry], ctx: &mut ItemCtx, cx: &mut Context<Self>) -> (Vec<Node>, Shape) {
        // With one item on screen, what isn't known about it is asked for at once; what is, is used now.
        let probed = Self::probed_of(chosen, cx);
        if let (Some(aware), [entry]) = (aware::try_shared(cx), chosen) {
            aware.update(cx, |aware, cx| aware.look_now(std::slice::from_ref(entry), cx));
        }
        ctx.probed = probed.clone();
        // Where this kind of thing has gone before: one extension, or folders only; a mix has no one place.
        let paths: Vec<String> = chosen.iter().map(|e| e.path.clone()).collect();
        let ext = glint_model::aware::shared_ext(chosen);
        let folders_only = chosen.iter().all(|e| e.dir);
        let movable = !chosen.iter().any(|e| is_archived(e) || is_closed(e) || is_remote(e) || is_recycled(e));
        ctx.places = match (&ext, folders_only && movable) {
            (Some(ext), _) if movable => aware::places_for(cx, ext),
            (None, true) => aware::places_for(cx, ""),
            _ => Vec::new(),
        };
        let usual: Vec<Node> = send_to::usual_row(&paths, &ctx.places, ext.as_deref(), folders_only, &ctx.folder)
            .into_iter()
            .collect();
        let nodes = item_menu(chosen, ctx);
        let facts = aware::facts(cx, chosen);
        let one = match chosen {
            [only] => Some(only),
            _ => None,
        };
        let head = || head::head_of(chosen, probed.as_ref(), &Self::extras(one, cx));
        (Self::dressed(nodes, head, &facts, probed.as_ref(), usual, cx), facts.shape)
    }

    /// The folder's own menu, dressed: its name, how many items it shows and its branch in the head.
    fn folder_nodes(
        nodes: Vec<Node>,
        title: SharedString,
        items: Option<usize>,
        branch: Option<SharedString>,
        probed: Option<&Probed>,
        cx: &App,
    ) -> (Vec<Node>, Shape) {
        let facts = aware::facts(cx, &[]);
        let head = || head::folder_head_with(title, items, branch, probed);
        (Self::dressed(nodes, head, &facts, probed, Vec::new(), cx), facts.shape)
    }

    /// A folder's name as the tabs say it, in the language shown.
    fn folder_title(folder: &str, cx: &App) -> SharedString {
        let name = try_explorer(cx).map_or_else(|| base_name(folder).to_string(), |e| e.read(cx).tab_name(folder));
        glint_model::i18n::t(i18n::language(), &name, &[]).into_owned().into()
    }

    /// The menu closed: what was being found out for it is no longer wanted.
    fn rest_aware(cx: &mut Context<Self>) {
        if let Some(aware) = aware::try_shared(cx) {
            aware.update(cx, |aware, _| aware.rest());
        }
    }

    /// Aware found out something: if the menu is for one item, its head says it (nothing else moves).
    fn follow_answers(&mut self, cx: &mut Context<Self>) {
        let Some(open) = &self.open else { return };
        if let Some(folder) = &open.folder {
            let probed = aware::try_shared(cx).and_then(|aware| aware.read(cx).probed(&folder.entry).cloned());
            let head =
                head::folder_head_with(folder.title.clone(), folder.items, folder.branch.clone(), probed.as_ref());
            open.view.update(cx, |view, cx| view.set_head(head, cx));
            return;
        }
        let [entry] = open.targets.as_slice() else { return };
        let probed = Self::probed_of(&open.targets, cx);
        let head = head::head_of(&open.targets, probed.as_ref(), &Self::extras(Some(entry), cx));
        open.view.update(cx, |view, cx| view.set_head(head, cx));
    }

    /// Whether a menu is on screen.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether a menu is on screen or about to be (it waits a breath for what is being found out).
    pub fn is_showing(&self) -> bool {
        self.open.is_some() || self.waiting.is_some()
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
        self.opening = self.opening.wrapping_add(1);
        self.waiting = None;
        if self.open.take().is_some() {
            Self::rest_aware(cx);
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
    // The file list and grid pass the focused row's place as `at` for the keyboard's menu; this fallback is for
    // a window with no list.
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
        // One item about which something is still being found out: it is asked for at once, and the menu waits
        // for it, at most [`LOOK_FIRST`]: it opens the moment the item's answers are in, or when the run that
        // looks for them has ended. What is suggested is decided once, when the menu opens.
        self.opening = self.opening.wrapping_add(1);
        self.waiting = None;
        if let (Some(aware), [entry]) = (aware::try_shared(cx), chosen.as_slice()) {
            aware.update(cx, |aware, cx| aware.look_now(std::slice::from_ref(entry), cx));
            if !aware.read(cx).answered(entry) {
                let turn = self.opening;
                let wake = cx.subscribe_in(&aware, window, move |this, aware, _: &aware::Answered, window, cx| {
                    this.wake(turn, aware, window, cx);
                });
                self.waiting = Some(Waiting {
                    turn,
                    pane,
                    what: Awaited::Items(chosen),
                    at,
                    _wake: wake,
                });
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(LOOK_FIRST).await;
                    this.update_in(cx, |this, window, cx| this.open_waiting(turn, window, cx)).ok();
                })
                .detach();
                return;
            }
        }
        self.open_items(pane, chosen, at, window, cx);
    }

    /// Aware has kept answers or finished a run: the waiting menu opens if what it waits for is all known.
    fn wake(&mut self, turn: u64, aware: &Entity<aware::Aware>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(waiting) = self.waiting.as_ref().filter(|waiting| waiting.turn == turn) else {
            return;
        };
        let Some(entry) = waiting.what.entry() else { return };
        if aware.read(cx).answered(entry) {
            self.open_waiting(turn, window, cx);
        }
    }

    /// The menu that waited is shown, unless another was asked for (or this one closed) since.
    fn open_waiting(&mut self, turn: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening != turn {
            return;
        }
        match self.waiting.take() {
            Some(Waiting { pane, what: Awaited::Items(chosen), at, .. }) => {
                self.open_items(pane, chosen, at, window, cx);
            }
            Some(Waiting { pane, what: Awaited::Folder(_), at, .. }) => self.show_background(pane, at, window, cx),
            None => {}
        }
    }

    /// Shows the item menu for `chosen` (never empty) at `at`.
    fn open_items(
        &mut self,
        pane: usize,
        chosen: Vec<FileEntry>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut ctx = self.item_ctx(pane, cx);
        ctx.data.read_tags(cx, chosen.iter().map(|e| e.path.as_str()));
        let (nodes, shape) = Self::item_nodes(&chosen, &mut ctx, cx);
        let label = t!("Item menu");
        self.show(
            Shown {
                nodes,
                at,
                pane,
                label,
                shape,
                targets: chosen,
                folder: None,
            },
            window,
            cx,
        );
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
        let (nodes, shape) = Self::item_nodes(&entries, &mut ctx, cx);
        let label = t!("Item menu");
        self.show(
            Shown {
                nodes,
                at,
                pane,
                label,
                shape,
                targets: entries,
                folder: None,
            },
            window,
            cx,
        );
    }

    /// The folder's own menu: right-click on the empty part of the list, the grid, or a folder with nothing
    /// selected for the keyboard (`backgroundMenu`).
    ///
    /// For a real folder on a drive, what isn't known about it yet (its size, what a build left in it) is asked
    /// for at once, and the menu waits for it as the item menus do ([`LOOK_FIRST`]), so that its Suggested rows
    /// can offer "Clean up…" in a project folder; what comes in later still goes into its head.
    pub fn open_background_menu(
        &mut self,
        pane: usize,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = Self::anchor(at, window);
        self.opening = self.opening.wrapping_add(1);
        self.waiting = None;
        if let (Some(aware), Some(folder)) = (aware::try_shared(cx), Self::folder_shown(pane, cx)) {
            aware.update(cx, |aware, cx| aware.look_now(std::slice::from_ref(&folder), cx));
            if !aware.read(cx).answered(&folder) {
                let turn = self.opening;
                let wake = cx.subscribe_in(&aware, window, move |this, aware, _: &aware::Answered, window, cx| {
                    this.wake(turn, aware, window, cx);
                });
                self.waiting = Some(Waiting {
                    turn,
                    pane,
                    what: Awaited::Folder(folder),
                    at,
                    _wake: wake,
                });
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(LOOK_FIRST).await;
                    this.update_in(cx, |this, window, cx| this.open_waiting(turn, window, cx)).ok();
                })
                .detach();
                return;
            }
        }
        self.show_background(pane, at, window, cx);
    }

    /// The folder `pane` shows, as Aware keeps what it finds out about it; None when it isn't a plain folder on
    /// a drive (search results, an archive, a folder with no access, the Recycle Bin, This PC).
    fn folder_shown(pane: usize, cx: &App) -> Option<FileEntry> {
        let explorer = try_explorer(cx)?;
        let ex = explorer.read(cx);
        let tab = ex.pane_tab(pane)?;
        let plain = tab.find.is_none() && tab.archive().is_none() && tab.no_access().is_none();
        (plain && !tab.is_recycle_bin() && is_folder_path(&tab.path)).then(|| folder_entry(&tab.path))
    }

    /// Shows the folder's own menu at `at`, with what is known about the folder now.
    fn show_background(&mut self, pane: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(explorer) = try_explorer(cx) else { return };
        let (folder, view, title, rows, tab_id) = {
            let ex = explorer.read(cx);
            let settings = state::settings(cx);
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
            let look = folder::Look {
                view: tab.view,
                sort: tab.sort,
                group: settings.group_by,
                hidden: settings.show_hidden,
                preview: settings.show_preview_pane,
                tile: settings.tile_size,
            };
            (
                tab.path.clone(),
                FolderView {
                    find: tab.find.is_some(),
                    archive: tab.archive().cloned(),
                    no_access: tab.no_access().copied(),
                    from_index: tab.listing.as_ref().is_some_and(|l| l.from_index),
                    tab: Some(tab.id),
                    restore_shown,
                    look,
                },
                Self::folder_title(&tab.path, cx),
                tab.rows.len(),
                tab.id,
            )
        };
        let mut ctx = self.folder_ctx(&folder, cx);
        ctx.view = Some(view);
        let nodes = folder_menu(&folder, true, &ctx);
        let branch = git::view_of(cx, tab_id).and_then(|view| view.branch.clone()).map(SharedString::from);
        let entry = Self::folder_shown(pane, cx);
        let probed = entry
            .as_ref()
            .and_then(|entry| aware::try_shared(cx)?.read(cx).probed(entry).cloned());
        let follow = entry.map(|entry| FolderHead {
            entry,
            title: title.clone(),
            items: Some(rows),
            branch: branch.clone(),
        });
        let (nodes, shape) = Self::folder_nodes(nodes, title, Some(rows), branch, probed.as_ref(), cx);
        let label = t!("Folder menu");
        self.show(
            Shown {
                nodes,
                at,
                pane,
                label,
                shape,
                targets: Vec::new(),
                folder: follow,
            },
            window,
            cx,
        );
    }

    /// The menu of a folder that isn't the one shown: a tile of This PC, a place in the sidebar
    /// (`onPlaceContextMenu`).
    pub fn open_folder_menu(&mut self, folder: &str, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let pane = try_explorer(cx).map_or(0, |e| e.read(cx).active_pane());
        let ctx = self.folder_ctx(folder, cx);
        let nodes = folder_menu(folder, false, &ctx);
        let (nodes, shape) = Self::folder_nodes(nodes, Self::folder_title(folder, cx), None, None, None, cx);
        let label = t!("Folder menu");
        self.show(
            Shown {
                nodes,
                at,
                pane,
                label,
                shape,
                targets: Vec::new(),
                folder: None,
            },
            window,
            cx,
        );
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
        ctx.files_on_clipboard = aware::try_shared(cx)
            .is_some_and(|aware| matches!(aware.read(cx).clip(), glint_model::aware::Clip::Files { .. }));
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
                    });
            }
        }
        ctx
    }

    /// Shows `nodes` at `at`, taking the keyboard.
    fn show(&mut self, shown: Shown, window: &mut Window, cx: &mut Context<Self>) {
        // A menu that was waiting a breath gives way to this one.
        self.opening = self.opening.wrapping_add(1);
        self.waiting = None;
        let Shown {
            nodes,
            at,
            pane,
            label,
            shape,
            targets,
            folder,
        } = shown;
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
        // What is found out while the menu is open goes into its head, and nowhere else.
        let watch = aware::try_shared(cx).map(|aware| cx.observe(&aware, |this, _, cx| this.follow_answers(cx)));
        self.open = Some(Open {
            view,
            at,
            pane,
            focus,
            shape,
            targets,
            folder,
            _subscription: subscription,
            _aware: watch,
        });
        cx.emit(MenuEvent::Shown);
        cx.notify();
    }

    fn on_view(&mut self, event: MenuViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else { return };
        Self::rest_aware(cx);
        cx.emit(MenuEvent::Closed);
        cx.notify();
        match event {
            MenuViewEvent::Chose(run) => {
                if let Some(key) = run.key() {
                    aware::note_action(cx, open.shape, &key);
                }
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
            Task::Navigate(path) | Task::OpenExtracted(path) => {
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
            Task::SortBy(key) => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| {
                        let Some(current) = e.active_tab().map(|tab| tab.sort) else { return };
                        if current.key != key {
                            let desc = glint_model::columns::DESCENDING.contains(&key);
                            e.set_sort(glint_model::entries::Sort { key, desc }, cx);
                        }
                    });
                }
            }
            Task::SortOrder { desc } => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| {
                        let Some(current) = e.active_tab().map(|tab| tab.sort) else { return };
                        if current.desc != desc {
                            e.set_sort(glint_model::entries::Sort { key: current.key, desc }, cx);
                        }
                    });
                }
            }
            Task::GroupBy(by) => {
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| e.set_group(by, cx));
                }
            }
            Task::TileSize(size) => {
                state::update(cx, |settings| settings.tile_size = size);
                if let Some(explorer) = explorer {
                    explorer.update(cx, |e, cx| e.set_view(glint_model::store::ViewMode::Grid, None, cx));
                }
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
        .window(crate::workspace::MAIN)
        .or_else(|| core.app.windows().into_values().next())
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
