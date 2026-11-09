//! Places and the folder tree (Sidebar.tsx, FolderTree.tsx).
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                    | Here                                                      |
//! |-----------------------------------------------|-----------------------------------------------------------|
//! | `Sidebar`, `Section`, `Place`                 | [`Sidebar`] draws [`rows::Row`]s from [`rows::build_rows`] |
//! | `useTree`, `keyOf`, `revealInTree`, `treeSaw` | [`tree::Tree`] and its functions                          |
//! | `TreeKids`, `TreeRow`, `TreeArrow`            | `Row::Folder` rows, drawn by [`Sidebar::draw_folder`]      |
//! | `onTreeKey`                                   | [`Sidebar::on_key_down`]                                  |
//! | `useCloudSpace`, `useSavedCount`              | [`Sidebar::ask_cloud_space`], [`Sidebar::recount`]         |
//! | `folderMenu` for a place                      | [`menu::build`]                                           |
//!
//! **One flat, virtualised list.** The TypeScript nested a component per place and per folder. A tree of
//! thousands of folders is possible, so the sidebar lists what is on screen as plain rows
//! ([`rows::build_rows`], a pure function, tested without a window) and draws only the visible ones with GPUI's
//! `list` (rows differ in height: a drive has its space bar). The rows are rebuilt only when something they are
//! made from changed, and only the part of the list that differs is replaced, so scrolling and measurements
//! stay.
//!
//! **State.** The view keeps only its own: the keyboard's row, the open folders and what was read of them
//! ([`tree::Tree`]), cloud space and saved-search counts. The folder shown, the drives and the known folders are
//! the explorer's; the pinned folders, saved searches and connections are the saved state's; the folder tree
//! settings are the settings'. The view observes all three.
//!
//! **Not in the TypeScript, and so not here:** sections do not collapse (the TypeScript never let them); the
//! "rail" below the `md` breakpoint (a 48 px icon-only sidebar) is the window width's business in CSS, and the
//! GPUI workspace resizes the sidebar between fixed limits instead.

mod menu;
mod rows;
mod tree;

use crate::core_bridge::{self, Core};
use crate::explorer::{explorer, Explorer, NavOptions};
use crate::kit::prelude::*;
use crate::persistence::Persistence;
use crate::state::UiState;
use crate::{persistence, state, t, theme};
use glint_core::host::Manager as _;
use glint_model::entries::{with_details, FileEntry, RawEntry};
use glint_model::file_types::kind_of;
use glint_model::paths::{same_path, THIS_PC};
use glint_model::store::{RemoteKind, SavedSearch};
use rows::{build_rows, count_key, reads_files, Glyph, Inputs, Label, Place, Row, Space};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::{KindTone, Tokens};

pub use rows::{remote_path, remote_where};
pub use tree::{core_subfolders, key_of, reveal_chain, Kids, Subfolder, SubfolderSource, Tree};

/// How long a cloud's space is reused before its service is asked again (the TypeScript's ten minutes).
const SPACE_FRESH_FOR: Duration = Duration::from_secs(10 * 60);

/// What the sidebar asks its owner to do. The window wires each to the part that does it.
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarEvent {
    /// A saved search was clicked: open its folder in the active tab, put its query in the search box and run it
    /// (`runSavedSearch`). PORT ME(find): the find part does this.
    RunSavedSearch(SavedSearch),
    /// "Add a connection" was clicked (`addRemote`). PORT ME(remotes): open the connection dialog.
    AddConnection,
    /// "Edit the connection…" in a connection's menu, with the connection's id (`editRemote`).
    EditRemote(String),
    /// "Remove the connection…" in a connection's menu, with its id (`removeRemote`).
    RemoveRemote(String),
    /// "Search text" in a folder's menu: switch to text search in this folder (`onSearchText`).
    SearchText(String),
}

/// A cloud's space from its service: its rclone remote's name and the folder in it. Total and free bytes, or
/// `None` where the service does not say.
pub type CloudSpaceSource =
    Arc<dyn Fn(String, String) -> Pin<Box<dyn Future<Output = Result<Option<Space>, String>> + Send>> + Send + Sync>;

/// How many items a saved search finds now, as text ("1,000+"), from instant search: the query and the folders
/// searched. `None` where the index cannot say.
pub type SavedCountSource = Arc<
    dyn Fn(String, Vec<String>) -> Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send>> + Send + Sync,
>;

/// Where the sidebar reads what is not in the explorer or the saved state: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Sources {
    /// The folders in a folder, for the tree.
    pub subfolders: SubfolderSource,
    /// How full a cloud is.
    pub cloud_space: CloudSpaceSource,
    /// What a saved search finds.
    pub saved_count: SavedCountSource,
}

impl Sources {
    /// glint-core.
    pub fn core() -> Sources {
        Sources {
            subfolders: core_subfolders(),
            cloud_space: Arc::new(|remote, root| {
                Box::pin(async move {
                    let found = glint_core::rclone::rclone_about(remote, root).await?;
                    Ok(found.and_then(|s| {
                        let total = s.total.filter(|t| *t != 0)?;
                        let free = s.free.unwrap_or_else(|| total.saturating_sub(s.used.unwrap_or(0)));
                        Some((total, free))
                    }))
                })
            }),
            saved_count: Arc::new(|query, roots| {
                Box::pin(async move {
                    let found = glint_core::index::index_count(query, roots, 1000).await?;
                    Ok(found.map(|(n, more)| {
                        let n = glint_model::format::format_count(usize::try_from(n).unwrap_or(usize::MAX));
                        if more {
                            format!("{n}+")
                        } else {
                            n
                        }
                    }))
                })
            }),
        }
    }
}

/// The places and the folder tree.
pub struct Sidebar {
    explorer: Entity<Explorer>,
    sources: Sources,
    focus: FocusHandle,
    list: ListState,
    rows: Vec<Row>,
    /// The row the keyboard is on (the TypeScript's DOM focus), drawn with a ring while the sidebar has focus.
    cursor: Option<usize>,
    tree: Tree,
    /// Cloud space by connection id, when it was asked, and who is being asked.
    spaces: HashMap<String, Space>,
    spaces_at: HashMap<String, Instant>,
    asking: HashSet<String>,
    /// Saved searches' counts, by [`count_key`].
    counts: HashMap<String, String>,
    // What the last refresh saw, to do each thing once per change (the TypeScript's effect dependencies).
    last_hidden: bool,
    last_reveal: Option<(bool, bool, String)>,
    last_listing: Option<Arc<[FileEntry]>>,
    last_saved: Vec<SavedSearch>,
    last_drives: Option<Arc<[crate::explorer::Drive]>>,
    /// Scroll to the folder shown once its row exists (after the tree opened down to it).
    scroll_to_current: bool,
    /// The pixels in a rem the rows were measured at.
    measured_at: Pixels,
    folder_tone: Option<KindTone>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

impl Sidebar {
    /// The sidebar, reading from glint-core. Needs the explorer ([`crate::explorer::init`]) to exist. Put it in the
    /// resizable panel at the start edge; it fills it.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_sources(Sources::core(), window, cx)
    }

    /// [`Sidebar::new`] reading from `sources` (tests, tools).
    pub fn with_sources(sources: Sources, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        let focus = cx.focus_handle().tab_stop(true);
        let subscriptions = vec![
            cx.observe(&explorer, |this, _, cx| this.refresh(cx)),
            cx.observe_global::<Persistence>(|this, cx| this.refresh(cx)),
            cx.observe_global::<UiState>(|this, cx| this.refresh(cx)),
            // Coming back to the window: the saved searches' counts are read again.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.recount(cx);
                }
            }),
        ];
        let mut this = Sidebar {
            explorer,
            sources,
            focus,
            list: ListState::new(0, ListAlignment::Top, px(300.)),
            rows: Vec::new(),
            cursor: None,
            tree: Tree::default(),
            spaces: HashMap::new(),
            spaces_at: HashMap::new(),
            asking: HashSet::new(),
            counts: HashMap::new(),
            last_hidden: state::settings(cx).show_hidden,
            last_reveal: None,
            last_listing: None,
            last_saved: Vec::new(),
            last_drives: None,
            scroll_to_current: false,
            measured_at: theme::rem_size(cx),
            folder_tone: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    /// The rows now, top to bottom (for the window's tests).
    #[cfg(test)]
    fn rows(&self) -> &[Row] {
        &self.rows
    }

    // ---- Keeping the list up to date ----------------------------------------------------------------------

    /// Something the list is made from may have changed: does what follows from it once (opens the tree down to
    /// the folder shown, tells the tree what a listing holds, asks for cloud space and counts), then rebuilds
    /// the rows and replaces the part of the list that differs.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (path, known, drives, listing) = {
            let e = self.explorer.read(cx);
            let listing = e
                .active_tab()
                .and_then(|t| t.listing.as_ref())
                .filter(|l| l.archive.is_none() && !l.remote)
                .map(|l| (l.path.clone(), l.entries.clone()));
            (
                e.current_path().to_string(),
                e.known_folders().clone(),
                e.drives().clone(),
                listing,
            )
        };
        let (folder_tree, tree_follows, show_hidden) = {
            let s = state::settings(cx);
            (s.folder_tree, s.tree_follows, s.show_hidden)
        };
        let (pinned, saved, remotes) = persistence::app(cx)
            .map(|a| (a.pinned.clone(), a.saved_searches.clone(), a.remotes.clone()))
            .unwrap_or_default();

        // Hidden folders shown or hidden: what is open is read again.
        if show_hidden != self.last_hidden {
            self.last_hidden = show_hidden;
            for path in self.tree.hidden_changed() {
                self.load(path, cx);
            }
        }

        // The tree opens down to the folder shown, when asked; only when the folder shown (or the settings)
        // change: places coming and going don't open it again.
        let reveal = (folder_tree, tree_follows, path.clone());
        if self.last_reveal.as_ref() != Some(&reveal) {
            self.last_reveal = Some(reveal);
            self.scroll_to_current = false;
            if folder_tree && tree_follows {
                let roots: Vec<String> = known
                    .iter()
                    .map(|k| k.path.clone())
                    .chain(pinned.iter().cloned())
                    .chain(drives.iter().map(|d| d.path.clone()))
                    .collect();
                self.reveal_in_tree(&path, &roots, cx);
            }
        }

        // What the folder shown holds keeps its place in the tree up to date.
        if folder_tree {
            if let Some((listed, entries)) = &listing {
                let new = self
                    .last_listing
                    .as_ref()
                    .is_none_or(|seen| !Arc::ptr_eq(seen, entries));
                if new {
                    self.last_listing = Some(entries.clone());
                    self.tree.saw(listed, entries, show_hidden);
                }
            }
        } else {
            self.last_listing = None;
        }

        // Saved searches' counts: when the searches or the drives change.
        let drives_changed = self.last_drives.as_ref().is_none_or(|d| !Arc::ptr_eq(d, &drives));
        if saved != self.last_saved || drives_changed {
            self.last_saved = saved.clone();
            self.last_drives = Some(drives.clone());
            self.recount(cx);
        }
        for remote in &remotes {
            self.ask_cloud_space(remote, cx);
        }

        let built = build_rows(&Inputs {
            path: &path,
            known: &known,
            drives: &drives,
            pinned: &pinned,
            saved: &saved,
            remotes: &remotes,
            folder_tree,
            tree: &self.tree,
            spaces: &self.spaces,
            counts: &self.counts,
        });
        self.set_rows(built, cx);
    }

    /// Replaces the rows, and the part of the list that differs. The keyboard's row stays on the same thing.
    fn set_rows(&mut self, built: Vec<Row>, cx: &mut Context<Self>) {
        if built != self.rows {
            let old = self.rows.len();
            let new = built.len();
            let same_start = self.rows.iter().zip(&built).take_while(|(a, b)| a == b).count();
            let same_end = self
                .rows
                .iter()
                .rev()
                .zip(built.iter().rev())
                .take(old.min(new) - same_start)
                .take_while(|(a, b)| a == b)
                .count();
            let kept = self
                .cursor
                .and_then(|c| self.rows.get(c))
                .map(|r| (r.key(), self.cursor.unwrap_or(0)));
            self.rows = built;
            self.list
                .splice(same_start..old - same_end, new - same_start - same_end);
            self.cursor = kept.and_then(|(key, at)| {
                (0..self.rows.len())
                    .filter(|&i| self.rows[i].key() == key)
                    .min_by_key(|&i| i.abs_diff(at))
            });
            cx.notify();
        }
        // The folder shown, once the tree has a row for it.
        if self.scroll_to_current {
            if let Some(at) = self.rows.iter().position(|r| matches!(r, Row::Folder(f) if f.active)) {
                self.scroll_to_current = false;
                self.list.scroll_to_reveal_item(at);
            }
        }
    }

    // ---- The folder tree --------------------------------------------------------------------------------

    /// Opens or closes a folder in the tree; opening reads what is in it again (`toggleFolder`).
    fn toggle_folder(&mut self, path: &str, open: Option<bool>, cx: &mut Context<Self>) {
        let open = open.unwrap_or_else(|| !self.tree.is_open(path));
        self.tree.set_open(path, open);
        if open {
            self.load(path.to_string(), cx);
        }
        self.refresh(cx);
    }

    /// Opens the tree down to `path` (File Explorer's "Expand to open folder"), below the closest of `roots` it
    /// is in, and scrolls to it once its row is there (`revealInTree`). The folders are read side by side; the
    /// TypeScript read each before the next, which only made it slower.
    fn reveal_in_tree(&mut self, path: &str, roots: &[String], cx: &mut Context<Self>) {
        let chain = reveal_chain(path, roots);
        if chain.is_empty() {
            return;
        }
        for folder in chain {
            if !self.tree.is_open(&folder) {
                self.tree.set_open(&folder, true);
                self.load(folder, cx);
            }
        }
        self.scroll_to_current = true;
    }

    /// Reads (again) the folders in `path`, on core's runtime (`load`).
    fn load(&mut self, path: String, cx: &mut Context<Self>) {
        let read = self.tree.begin_read(&path);
        let hidden = state::settings(cx).show_hidden;
        let reading = (self.sources.subfolders)(path.clone(), hidden);
        cx.spawn(async move |this, cx| {
            // Two layers: the core task may panic (outer Err), the folder may not list (inner Err).
            let found = core_bridge::run(reading).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.tree.finish_read(&path, read, found) {
                    this.refresh(cx);
                }
            });
        })
        .detach();
    }

    // ---- Clouds and counts ------------------------------------------------------------------------------

    /// How full a cloud of rclone's is, where its service says; asked again after ten minutes (`useCloudSpace`).
    /// An answer of "can't say", or an error, is kept as long as a good one, so a service that refuses is not
    /// asked at every redraw (the TypeScript asked again each time the sidebar mounted).
    fn ask_cloud_space(&mut self, remote: &glint_model::store::RemoteConnection, cx: &mut Context<Self>) {
        let Some(rclone) = remote.rclone.clone().filter(|_| remote.kind == RemoteKind::Rclone) else {
            return;
        };
        if self
            .spaces_at
            .get(&remote.id)
            .is_some_and(|at| at.elapsed() < SPACE_FRESH_FOR)
            || !self.asking.insert(remote.id.clone())
        {
            return;
        }
        let id = remote.id.clone();
        let reading = (self.sources.cloud_space)(rclone, remote.root.clone());
        cx.spawn(async move |this, cx| {
            let found = core_bridge::run(reading).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.asking.remove(&id);
                this.spaces_at.insert(id.clone(), Instant::now());
                match found {
                    Ok(Some(space)) => this.spaces.insert(id, space),
                    _ => this.spaces.remove(&id),
                };
                this.refresh(cx);
            });
        })
        .detach();
    }

    /// Asks instant search how many items each saved search finds now (`useSavedCount`). Searches whose filters
    /// read the files can't be counted by the index and show nothing.
    fn recount(&mut self, cx: &mut Context<Self>) {
        let drives: Vec<String> = self.explorer.read(cx).drives().iter().map(|d| d.path.clone()).collect();
        let saved = persistence::app(cx)
            .map(|a| a.saved_searches.clone())
            .unwrap_or_default();
        for search in saved {
            if reads_files(&search.query) {
                continue;
            }
            let roots = if search.folder == THIS_PC {
                drives.clone()
            } else {
                vec![search.folder.clone()]
            };
            if roots.is_empty() {
                continue;
            }
            let key = count_key(&search);
            let reading = (self.sources.saved_count)(search.query.clone(), roots);
            cx.spawn(async move |this, cx| {
                let found = core_bridge::run(reading).await.and_then(|inner| inner);
                let _ = this.update(cx, |this, cx| {
                    let changed = match found {
                        Ok(Some(text)) => this.counts.insert(key, text.clone()).as_ref() != Some(&text),
                        _ => this.counts.remove(&key).is_some(),
                    };
                    if changed {
                        this.refresh(cx);
                    }
                });
            })
            .detach();
        }
    }

    // ---- Doing things -----------------------------------------------------------------------------------

    /// What a click, Enter or Space on a row does.
    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix).cloned() else {
            return;
        };
        match row {
            Row::Place(_) | Row::Folder(_) | Row::Heading { link: Some(_), .. } => {
                if let Some(target) = row.target() {
                    self.explorer
                        .update(cx, |e, cx| e.navigate(target.to_string(), NavOptions::default(), cx));
                }
            }
            Row::Search(s) => cx.emit(SidebarEvent::RunSavedSearch(SavedSearch {
                query: s.query,
                folder: s.folder,
            })),
            Row::AddConnection => cx.emit(SidebarEvent::AddConnection),
            Row::Heading { .. } | Row::Note { .. } => {}
        }
        let _ = window;
    }

    /// A click: the keyboard moves there, and the row does its thing.
    fn click(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        window.focus(&self.focus, cx);
        self.activate(ix, window, cx);
        cx.notify();
    }

    /// Middle-click on a folder: a new tab for it, as in a browser (`middleClickOpens`).
    fn open_in_new_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(target) = self
            .rows
            .get(ix)
            .filter(|r| matches!(r, Row::Place(_) | Row::Folder(_)))
            .and_then(Row::target)
        {
            let target = target.to_string();
            self.explorer.update(cx, |e, cx| e.open_in_new_tab(&target, cx));
        }
    }

    /// Takes a saved search out of the sidebar.
    fn remove_search(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(Row::Search(s)) = self.rows.get(ix) {
            let search = SavedSearch {
                query: s.query.clone(),
                folder: s.folder.clone(),
            };
            persistence::update_app(cx, |app| app.toggle_saved_search(search));
        }
    }

    /// Ejects a drive (a USB drive made safe to remove, a disc) and reads the drives again.
    fn eject(&mut self, path: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let host = cx.try_global::<Core>().and_then(|core| {
            core.app
                .get_webview_window(crate::workspace::MAIN)
                .or_else(|| core.app.webview_windows().into_values().next())
        });
        let Some(host) = host else {
            window.push_notification(
                Notification::error(t!("Windows didn't answer")).title(t!("Couldn't eject {name}", name = name)),
                cx,
            );
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(glint_core::shell::eject_drive(host, path))
                .await
                .and_then(|inner| inner);
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        window.push_notification(Notification::success(t!("Safely ejected {name}", name = name)), cx);
                        this.explorer.update(cx, |e, cx| e.refresh_drives(cx).detach());
                    }
                    Err(reason) => window.push_notification(
                        Notification::error(reason).title(t!("Couldn't eject {name}", name = name)),
                        cx,
                    ),
                };
            });
        })
        .detach();
    }

    // ---- The keyboard -----------------------------------------------------------------------------------

    /// The sidebar's keys for its places and the tree's folders (`onTreeKey`): Up and Down move between rows,
    /// the key toward the end of the line opens a folder (or goes into it) and the key toward the start closes
    /// it (or goes up to its parent). In the mirrored (Arabic) interface those two swap, as in any mirrored tree.
    /// Beyond the TypeScript, which only moved between places and folders: the arrows also reach saved searches,
    /// the add button and the This PC title, Home and End jump, Enter and Space do what a click does, and Delete
    /// takes a saved search out (the TypeScript's remove button was a Tab stop).
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = event.keystroke.modifiers;
        if m.control || m.alt || m.platform || m.function {
            return;
        }
        let rtl = crate::i18n::is_rtl();
        let handled = match event.keystroke.key.as_str() {
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            "home" => self.jump(false, cx),
            "end" => self.jump(true, cx),
            "right" if !rtl => self.expand(cx),
            "left" if !rtl => self.collapse(cx),
            "left" => self.expand(cx),
            "right" => self.collapse(cx),
            "enter" | "space" => match self.cursor {
                Some(ix) => {
                    self.activate(ix, window, cx);
                    true
                }
                None => false,
            },
            "delete" => match self.cursor {
                Some(ix) if matches!(self.rows.get(ix), Some(Row::Search(_))) => {
                    self.remove_search(ix, cx);
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    /// Moves the keyboard to the row `delta` focusable rows away.
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let stops: Vec<usize> = (0..self.rows.len()).filter(|&i| self.rows[i].focusable()).collect();
        if stops.is_empty() {
            return false;
        }
        let at = self.cursor.and_then(|c| stops.iter().position(|&s| s == c));
        let to = match at {
            None if delta > 0 => 0,
            None => stops.len() - 1,
            Some(at) => (at as isize + delta).clamp(0, stops.len() as isize - 1) as usize,
        };
        self.set_cursor(stops.get(to).copied(), cx);
        true
    }

    /// Moves the keyboard to the first or last focusable row.
    fn jump(&mut self, last: bool, cx: &mut Context<Self>) -> bool {
        let mut stops = (0..self.rows.len()).filter(|&i| self.rows[i].focusable());
        let to = if last { stops.next_back() } else { stops.next() };
        self.set_cursor(to, cx);
        to.is_some()
    }

    fn set_cursor(&mut self, to: Option<usize>, cx: &mut Context<Self>) {
        self.cursor = to;
        if let Some(ix) = to {
            self.list.scroll_to_reveal_item(ix);
        }
        cx.notify();
    }

    /// Opens the folder the keyboard is on, or goes into it when it is open (Right in English).
    fn expand(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.cursor else {
            return false;
        };
        let Some(row) = self.rows.get(ix) else {
            return false;
        };
        let Some(path) = row.tree_path().map(str::to_string) else {
            return false;
        };
        if row.is_leaf() {
            return false;
        }
        if !row.is_open() {
            self.toggle_folder(&path, Some(true), cx);
        } else if matches!(self.rows.get(ix + 1), Some(Row::Folder(f)) if same_path(&f.parent, &path)) {
            self.set_cursor(Some(ix + 1), cx);
        }
        true
    }

    /// Closes the folder the keyboard is on, or goes up to the folder it is in (Left in English).
    fn collapse(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.cursor else {
            return false;
        };
        let Some(row) = self.rows.get(ix) else {
            return false;
        };
        let Some(path) = row.tree_path().map(str::to_string) else {
            return false;
        };
        if row.is_open() {
            self.toggle_folder(&path, Some(false), cx);
        } else if let Row::Folder(f) = row {
            let parent = f.parent.clone();
            let up = self
                .rows
                .iter()
                .position(|r| r.tree_path().is_some_and(|p| same_path(p, &parent)));
            if up.is_some() {
                self.set_cursor(up, cx);
            }
        }
        true
    }

    // ---- Drawing ----------------------------------------------------------------------------------------

    fn draw_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let focused = self.cursor == Some(ix) && self.focus.contains_focused(window, cx);
        match row {
            Row::Heading { .. } => self.draw_heading(ix, row, &t, focused, cx),
            Row::Place(p) => self.draw_place(ix, p, &t, focused, cx),
            Row::Folder(f) => self.draw_folder(ix, f, &t, focused, cx),
            Row::Note { depth, reason } => self.draw_note(*depth, reason, &t),
            Row::Search(s) => self.draw_search(ix, s, &t, focused, cx),
            Row::AddConnection => self.draw_add(ix, &t, focused, cx),
        }
    }

    /// The folder icon's colors (the amber of folders), read once per frame.
    fn folder_color(&self, t: &Tokens) -> Hsla {
        self.folder_tone.map_or(t.muted_foreground, |tone| tone.color)
    }

    fn draw_heading(&self, ix: usize, row: &Row, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let Row::Heading {
            title,
            link,
            active,
            first,
        } = row
        else {
            return div().into_any_element();
        };
        let name = t!(*title);
        let shell = div()
            .w_full()
            .pt(space(if *first { 0. } else { 4. }))
            .pb(space(1.))
            .px(space(2.));
        let text = if link.is_some() {
            div()
                .id(("heading", ix))
                .role(Role::Button)
                .aria_label(name.clone())
                .aria_selected(*active)
                .test_support()
                .rounded(theme::size::RADIUS_SM)
                .border_1()
                .border_color(ring(t, focused))
                .cursor_pointer()
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if *active { t.foreground } else { t.muted_foreground })
                .hover({
                    let color = t.foreground;
                    move |s| s.text_color(color)
                })
                .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
                .child(name)
                .into_any_element()
        } else {
            div()
                .id(("heading", ix))
                .role(Role::Heading)
                .aria_label(name.clone())
                .aria_level(2)
                .test_support()
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted_foreground)
                .child(name)
                .into_any_element()
        };
        shell.child(text).into_any_element()
    }

    /// The pill at the start edge of the row that is the folder shown (Windows 11's navigation selection).
    fn pill(t: &Tokens, height: Rems) -> Div {
        let wrap = div().absolute().top_0().bottom_0().flex().items_center();
        let wrap = if crate::i18n::is_rtl() {
            wrap.right_0()
        } else {
            wrap.left_0()
        };
        wrap.child(div().w(px(3.)).h(height).rounded_full().bg(t.primary))
    }

    /// A tree arrow's wrapper: `offset` from the start edge, centred vertically.
    fn at_start(offset: Rems) -> Div {
        let wrap = div().absolute().top_0().bottom_0().flex().items_center();
        if crate::i18n::is_rtl() {
            wrap.right(offset)
        } else {
            wrap.left(offset)
        }
    }

    /// The arrow by a place or a folder that opens it: seen on hover, and while open (`TreeArrow`). Mouse only:
    /// the keyboard opens folders with the arrow keys, and the row says whether it is expanded.
    fn arrow(&self, ix: usize, opens: rows::Opens, offset: Rems, t: &Tokens, cx: &mut Context<Self>) -> Div {
        let name = if opens.loading {
            IconName::LoaderCircle
        } else if opens.open {
            IconName::ChevronDown
        } else if crate::i18n::is_rtl() {
            IconName::ChevronLeft
        } else {
            IconName::ChevronRight
        };
        let hover = t.background;
        Self::at_start(offset).child(
            div()
                .id(("arrow", ix))
                .test_support()
                .size(space(4.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::size::RADIUS_SM)
                .cursor_pointer()
                .text_color(t.muted_foreground)
                .opacity(if opens.open { 1. } else { 0. })
                .group_hover("row", |s| s.opacity(1.))
                .hover(move |s| s.bg(hover))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(path) = this.rows.get(ix).and_then(Row::tree_path).map(str::to_string) {
                        this.toggle_folder(&path, None, cx);
                    }
                }))
                .child(icon(name).size_3p5()),
        )
    }

    /// The menu of a row, built when it opens from what the row is then.
    fn row_menu(
        &self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let sidebar = cx.entity().downgrade();
        move |menu, _window, cx| {
            let Some(view) = sidebar.upgrade() else {
                return menu;
            };
            let Some(row) = view.read(cx).rows.get(ix).cloned() else {
                return menu;
            };
            let pinned = row
                .target()
                .is_some_and(|path| persistence::app(cx).is_some_and(|a| a.pinned.iter().any(|p| p == path)));
            menu::build(menu, &row, pinned, &sidebar, cx)
        }
    }

    fn glyph_icon(glyph: &Glyph) -> IconName {
        match glyph {
            Glyph::Recent => IconName::Clock,
            Glyph::Gallery => IconName::Images,
            Glyph::Known(id) => known_icon(*id),
            Glyph::Folder => IconName::Folder,
            Glyph::Drive(kind) => drive_icon(*kind),
            Glyph::RecycleBin => IconName::Trash,
            Glyph::Remote(kind, service) => match kind {
                // Services reached over a network protocol rather than a company's cloud: a server's icon.
                RemoteKind::Rclone => {
                    if matches!(
                        service.as_str(),
                        "sftp" | "ftp" | "smb" | "webdav" | "http" | "hdfs" | "seafile"
                    ) {
                        IconName::Server
                    } else {
                        IconName::Cloud
                    }
                }
                RemoteKind::S3 => IconName::Database,
                RemoteKind::Sftp => IconName::SquareTerminal,
                RemoteKind::Device => IconName::Monitor,
                RemoteKind::Webdav => IconName::Server,
            },
        }
    }

    // PORT ME(drag and drop): places and tree folders take dropped files (copy into the folder, Shift moves,
    // a paired PC's connection sends, the Recycle Bin deletes) and show `dropTarget`'s highlight while one is over
    // them; the TypeScript marked them `data-drop`. The drop part answers with `DropAnswers::set` per row.
    fn draw_place(&self, ix: usize, p: &Place, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let label: SharedString = match &p.label {
            Label::Fixed(text) => t!(*text),
            Label::Plain(text) => text.clone(),
        };
        let color = match &p.glyph {
            Glyph::Folder | Glyph::Drive(crate::explorer::DriveKind::Wsl) => self.folder_color(t),
            _ if p.active => t.foreground,
            _ => t.muted_foreground,
        };
        let bar = p
            .capacity
            .and_then(|(total, free)| capacity_bar(total, free, space(1.), cx));
        let text = col()
            .flex_1()
            .min_w_0()
            .child(div().truncate().child(isolate(label.clone())))
            .children(bar.map(|b| div().mt(space(1.)).child(b)));
        let tip = tooltip_of(label.clone(), p.detail.clone());
        let (active, accent, foreground) = (p.active, t.accent, t.foreground);
        let mut button = row_start()
            .id(("place", ix))
            .role(Role::Button)
            .aria_label(label.clone())
            .aria_description(p.detail.clone())
            .aria_selected(p.active);
        if let Some(opens) = p.tree {
            button = button.aria_expanded(opens.open);
        }
        let button = button
            .test_support()
            .relative()
            .w_full()
            .gap(space(2.5))
            .px(space(2.))
            .py(space(1.5))
            .when(p.gutter, |d| d.ps(6.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_color(t.foreground.opacity(0.9))
            .hover(move |s| s.bg(accent))
            .when(active, move |d| d.bg(accent).text_color(foreground))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.open_in_new_tab(ix, cx)),
            )
            .tooltip(tip)
            .when(active, |d| d.child(Self::pill(t, space(4.))))
            .child(icon(Self::glyph_icon(&p.glyph)).size_4().flex_none().text_color(color))
            .child(text)
            .context_menu(self.row_menu(ix, cx));

        let mut cell = div().group("row").relative().w_full().pb(px(1.)).child(button);
        if let Some(opens) = p.tree {
            cell = cell.child(self.arrow(ix, opens, rems(0.25), t, cx));
        }
        if let Some(name) = &p.eject {
            let (path, label) = (p.path.clone(), name.clone());
            let hover = t.background;
            cell = cell.child(
                div()
                    .absolute()
                    .top(space(1.5))
                    .when(crate::i18n::is_rtl(), |d| d.left(space(1.)))
                    .when(!crate::i18n::is_rtl(), |d| d.right(space(1.)))
                    .child(
                        div()
                            .id(("eject", ix))
                            .role(Role::Button)
                            .aria_label(t!("Eject {name}", name = name))
                            .test_support()
                            .p(space(1.))
                            .rounded(theme::size::RADIUS_SM)
                            .cursor_pointer()
                            .text_color(t.muted_foreground)
                            .opacity(0.)
                            .group_hover("row", |s| s.opacity(1.))
                            .hover(move |s| s.bg(hover))
                            .tooltip(tooltip_of(
                                t!("Safely eject {name}", name = name),
                                SharedString::default(),
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.eject(path.clone(), label.clone(), window, cx)
                            }))
                            .child(icon(IconName::Unplug).size_3p5()),
                    ),
            );
        }
        cell.into_any_element()
    }

    fn draw_folder(
        &self,
        ix: usize,
        f: &rows::Folder,
        t: &Tokens,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // How far in a folder `depth` levels below a place is: past the place's own icon, a step a level (px).
        let indent = 38. + f.depth as f32 * 14.;
        let tip = tooltip_of(f.name.clone(), SharedString::from(f.path.clone()));
        let (active, accent, foreground) = (f.active, t.accent, t.foreground);
        let button = row_start()
            .id(("folder", ix))
            .role(Role::TreeItem)
            .aria_label(f.name.clone())
            .aria_description(SharedString::from(f.path.clone()))
            .aria_level(f.depth + 1)
            .aria_selected(f.active)
            .when(f.has != Some(false), |d| d.aria_expanded(f.open))
            .test_support()
            .relative()
            .w_full()
            .gap(space(2.))
            .py(space(1.))
            .pe(2.)
            .ps(indent / 4.)
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_size(rems(0.8125))
            .text_color(t.foreground.opacity(0.85))
            .hover(move |s| s.bg(accent))
            .when(active, move |d| d.bg(accent).text_color(foreground))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.open_in_new_tab(ix, cx)),
            )
            .tooltip(tip)
            .when(active, |d| d.child(Self::pill(t, space(3.5))))
            .child(
                icon(if f.open { IconName::FolderOpen } else { IconName::Folder })
                    .size_4()
                    .flex_none()
                    .text_color(self.folder_color(t)),
            )
            .child(div().flex_1().min_w_0().truncate().child(isolate(f.name.clone())))
            .context_menu(self.row_menu(ix, cx));

        let mut cell = div().group("row").relative().w_full().pb(px(1.)).child(button);
        if f.has != Some(false) {
            let opens = rows::Opens {
                open: f.open,
                loading: f.loading,
            };
            cell = cell.child(self.arrow(ix, opens, rems((indent - 18.) / 16.), t, cx));
        }
        cell.into_any_element()
    }

    /// "Can't look inside": a folder that couldn't be read, with the reason in its tooltip.
    fn draw_note(&self, depth: usize, reason: &str, t: &Tokens) -> AnyElement {
        let reason = SharedString::from(reason.to_string());
        div()
            .id(("note", depth))
            .w_full()
            .py(space(1.))
            .ps((38. + depth as f32 * 14.) / 4.)
            .truncate()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .tooltip(tooltip_of(t!("Can't look inside"), reason))
            .child(t!("Can't look inside"))
            .into_any_element()
    }

    fn draw_search(
        &self,
        ix: usize,
        s: &rows::Search,
        t: &Tokens,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let where_ = if s.folder == THIS_PC {
            t!("All drives")
        } else {
            SharedString::from(glint_model::paths::base_name(&s.folder).to_string())
        };
        let in_ = if s.folder.is_empty() {
            t!("every drive")
        } else {
            SharedString::from(s.folder.clone())
        };
        let query = SharedString::from(s.query.clone());
        let tip = tooltip_of(
            t!("Search for “{query}” in {place}", query = s.query, place = in_),
            SharedString::default(),
        );
        let accent = t.accent;
        let button = row_start()
            .id(("search", ix))
            .role(Role::Button)
            .aria_label(query.clone())
            .aria_description(where_.clone())
            .test_support()
            .w_full()
            .gap(space(2.5))
            .ps(2.)
            .pe(7.)
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_color(t.foreground.opacity(0.9))
            .hover(move |s| s.bg(accent))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .tooltip(tip)
            .child(
                icon(IconName::Search)
                    .size_4()
                    .flex_none()
                    .text_color(t.muted_foreground),
            )
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(isolate(query.clone())))
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(where_)),
                    ),
            )
            .children(s.count.clone().map(|count| {
                div()
                    .id(("search-count", ix))
                    .flex_none()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .group_hover("row", |s| s.invisible())
                    .tooltip(tooltip_of(t!("Found now, by instant search"), SharedString::default()))
                    .child(SharedString::from(count))
            }));

        let hover = t.background;
        let remove = div()
            .absolute()
            .top_0()
            .bottom_0()
            .flex()
            .items_center()
            .when(crate::i18n::is_rtl(), |d| d.left(space(1.)))
            .when(!crate::i18n::is_rtl(), |d| d.right(space(1.)))
            .child(
                div()
                    .id(("search-remove", ix))
                    .role(Role::Button)
                    .aria_label(t!("Remove the saved search “{query}”", query = s.query))
                    .test_support()
                    .p(space(1.))
                    .rounded(theme::size::RADIUS_SM)
                    .cursor_pointer()
                    .text_color(t.muted_foreground)
                    .opacity(0.)
                    .group_hover("row", |s| s.opacity(1.))
                    .hover(move |s| s.bg(hover))
                    .tooltip(tooltip_of(t!("Remove from the sidebar"), SharedString::default()))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_search(ix, cx)))
                    .child(icon(IconName::X).size_3p5()),
            );
        div()
            .group("row")
            .relative()
            .w_full()
            .pb(px(1.))
            .child(button)
            .child(remove)
            .into_any_element()
    }

    fn draw_add(&self, ix: usize, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let accent = t.accent;
        div()
            .w_full()
            .pb(px(1.))
            .child(
                row_start()
                    .id(("add", ix))
                    .role(Role::Button)
                    .aria_label(t!("Add a remote connection"))
                    .test_support()
                    .w_full()
                    .gap(space(2.5))
                    .px(space(2.))
                    .py(space(1.5))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(ring(t, focused))
                    .cursor_pointer()
                    .text_color(t.muted_foreground)
                    .hover(move |s| s.bg(accent))
                    .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
                    .tooltip(tooltip_of(
                        t!("Add a remote connection: a cloud (Google Drive, OneDrive, Dropbox and 60 more), an S3 bucket, a WebDAV server or an SFTP server"),
                        SharedString::default(),
                    ))
                    .child(icon(IconName::Plus).size_4().flex_none())
                    .child(div().truncate().child(t!("Add a connection"))),
            )
            .into_any_element()
    }
}

/// The ring a row shows while the keyboard is on it; transparent otherwise (so rows don't move).
fn ring(t: &Tokens, focused: bool) -> Hsla {
    if focused {
        t.ring
    } else {
        transparent_black()
    }
}

/// A tooltip with a title line and, where there is one, a second line of detail.
// TODO(coordinator): from kit (Tooltip); this is a private stand-in until the kit exports one.
struct Tip {
    title: SharedString,
    detail: SharedString,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx);
        col()
            .max_w(rems(24.))
            .px(space(2.5))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .text_size(theme::size::XS)
            .child(isolate(self.title.clone()))
            .when(!self.detail.is_empty(), |d| {
                d.child(div().text_color(t.muted_foreground).child(self.detail.clone()))
            })
    }
}

/// The tooltip builder for `title` and `detail` (the TypeScript's `title="label\ndetail"`).
fn tooltip_of(title: SharedString, detail: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_, cx| {
        let (title, detail) = (title.clone(), detail.clone());
        cx.new(|_| Tip { title, detail }).into()
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        // Tabbing in puts the keyboard on the folder shown, else the first place. (Checked here rather than in a
        // focus listener, which GPUI only calls while the window is active.)
        if self.cursor.is_none() && self.focus.is_focused(window) {
            self.cursor = self
                .rows
                .iter()
                .position(|r| matches!(r, Row::Place(p) if p.active))
                .or_else(|| self.rows.iter().position(Row::focusable));
        }
        // Rows are measured in rems: a new text size or zoom means measuring again.
        let rem = theme::rem_size(cx);
        if rem != self.measured_at {
            self.measured_at = rem;
            self.list.remeasure();
        }
        self.folder_tone = {
            static FOLDER: std::sync::OnceLock<FileEntry> = std::sync::OnceLock::new();
            let entry = FOLDER.get_or_init(|| {
                with_details(
                    RawEntry {
                        dir: true,
                        ..Default::default()
                    },
                    "",
                )
            });
            Some(theme::kind_tone(kind_of(entry), cx))
        };
        div()
            .id("places")
            .role(Role::Navigation)
            .aria_label(t!("Places"))
            .test_support()
            .key_context("Sidebar")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .flex()
            .flex_col()
            .px(space(2.))
            .pt(space(3.))
            .text_size(theme::size::SM)
            .text_color(t.foreground)
            .child(
                list(
                    self.list.clone(),
                    cx.processor(|this, ix, window, cx| this.draw_row(ix, window, cx)),
                )
                .flex_1()
                .min_h_0()
                .w_full(),
            )
    }
}

#[cfg(test)]
mod tests;
