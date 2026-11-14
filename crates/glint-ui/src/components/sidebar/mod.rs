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
//! | `TagsSection`, `allTags`                      | [`rows::shown_tags`], [`Sidebar::read_tags`], [`Sidebar::draw_tag`] |
//! | `folderMenu` for a place, `onPlaceContextMenu` | [`SidebarRequest::ContextMenu`]: the window's shared menu |
//! | `data-drop`, `dropTarget`                     | [`drop::target_of`], [`Sidebar::drag_over`], [`Sidebar::drop_on`] |
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

mod drop;
mod rows;
mod tree;

use crate::core_bridge;
use crate::dialogs::tags;
use crate::explorer::{explorer, Explorer, NavOptions};
use crate::kit::prelude::*;
use crate::persistence::Persistence;
use crate::platform::dragdrop::{Answer, DropEffect};
use crate::state::UiState;
use crate::{drops, persistence, state, t, theme};
use glint_model::entries::{with_details, FileEntry, RawEntry};
use glint_model::file_types::kind_of;
use glint_model::paths::{same_path, RECYCLE_BIN, THIS_PC};
use glint_model::store::{RemoteConnection, RemoteKind, SavedSearch};
use rows::{build_rows, count_key, reads_files, shown_tags, tag_search, Glyph, Inputs, Label, Place, Row, Space};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::{KindTone, Tokens};

pub use rows::{remote_path, remote_where};
pub use tree::{core_subfolders, key_of, reveal_chain, Kids, Subfolder, SubfolderSource, Tree};

/// Rows the list draws beyond the visible ones, so scrolling doesn't show blanks.
const OVERDRAW: Pixels = px(300.);

/// A tree folder's text (`text-[13px]`): between the theme's `XS` and `SM`.
const TREE_TEXT: Rems = rems(0.8125);

/// Spacing units ([`space`]): how far in a tree folder's icon is below a place (`pl-[38px]`), and a step a level.
const TREE_INDENT: f32 = 9.5;
const TREE_STEP: f32 = 3.5;

/// How long a cloud's space is reused before its service is asked again (the TypeScript's ten minutes).
const SPACE_FRESH_FOR: Duration = Duration::from_secs(10 * 60);

/// What the sidebar asks its owner to do. The window wires each to the part that does it.
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarEvent {
    /// A saved search was clicked: open its folder in the active tab, put its query in the search box and run it
    /// (`runSavedSearch`; the window has the explorer do it). A tag is one too: `tag:"name"` at This PC.
    RunSavedSearch(SavedSearch),
    /// "Add a connection" was clicked (`addRemote`); the window opens the connection dialog.
    AddConnection,
    /// No longer emitted: "Edit the connection…" is the shared menu's `MenuEvent::EditConnection` now. Kept only
    /// until the window's match on this enum drops it.
    EditRemote(String),
    /// No longer emitted: "Remove the connection…" is the shared menu's `MenuEvent::RemoveConnection` now.
    RemoveRemote(String),
    /// No longer emitted: "Search text" is the shared menu's `MenuEvent::SearchText` now.
    SearchText(String),
}

/// What else the sidebar asks its owner for. A second type, so adding to it never breaks the window's exhaustive
/// match on [`SidebarEvent`]; subscribe to both. (The coordinator may fold it into [`SidebarEvent`] when it edits
/// the window.)
#[derive(Clone, Debug, PartialEq)]
pub enum SidebarRequest {
    /// The menu of a place or a tree folder, from a right-click or from the menu key or Shift+F10 on the row the
    /// keyboard is on. `position` is in the window: the pointer, or the row's bottom-start corner (`onPlaceContextMenu`).
    /// The window opens `components::menus::MenuHost::open_folder_menu(&path, position, ..)`.
    ContextMenu { path: String, position: Point<Pixels> },
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

/// Every tag on the drives instant search covers, with how many items have it, most used first (`tags_all`); an
/// error where instant search is off or from before tags.
pub type TagsSource =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Vec<(String, u64)>, String>> + Send>> + Send + Sync>;

/// Where the sidebar reads what is not in the explorer or the saved state: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Sources {
    /// The folders in a folder, for the tree.
    pub subfolders: SubfolderSource,
    /// How full a cloud is.
    pub cloud_space: CloudSpaceSource,
    /// What a saved search finds.
    pub saved_count: SavedCountSource,
    /// The tags on the drives instant search covers.
    pub tags: TagsSource,
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
            tags: Arc::new(|| Box::pin(glint_core::tags::tags_all())),
        }
    }
}

/// What [`Sidebar::refresh`] builds the rows from, kept to compare against: shared lists by pointer (a new listing
/// or list of drives is a new `Arc`), the rest by value.
#[derive(PartialEq)]
struct Seen {
    path: String,
    known: usize,
    drives: usize,
    listing: Option<usize>,
    folder_tree: bool,
    tree_follows: bool,
    show_hidden: bool,
    pinned: Vec<String>,
    saved: Vec<SavedSearch>,
    remotes: Vec<RemoteConnection>,
}

/// An `Arc`'s address, to tell a new list from the one seen before without comparing what is in it.
fn arc_key<T: ?Sized>(shared: &Arc<T>) -> usize {
    Arc::as_ptr(shared).cast::<()>() as usize
}

/// The active tab's listing, when it is a folder the tree can hold (not in an archive, not on a remote).
fn tree_listing(explorer: &Explorer) -> Option<&crate::explorer::Listed> {
    explorer
        .active_tab()
        .and_then(|t| t.listing.as_ref())
        .filter(|l| l.archive.is_none() && !l.remote)
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
    /// Every tag instant search lists, with its count (empty where it lists none), and how many times it was asked
    /// (an older answer arriving late is dropped).
    indexed_tags: Vec<(String, u64)>,
    tags_asked: u64,
    // What the last refresh saw, to do each thing once per change (the TypeScript's effect dependencies).
    last_hidden: bool,
    last_reveal: Option<(bool, bool, String)>,
    last_listing: Option<Arc<[FileEntry]>>,
    last_saved: Vec<SavedSearch>,
    last_drives: Option<Arc<[crate::explorer::Drive]>>,
    /// What the rows were last built from. The explorer, the saved state and the settings change far more often
    /// than anything the sidebar shows (a selection, a scroll, a sort, a visit): such a change rebuilds nothing.
    seen: Option<Seen>,
    /// How many times the rows were built.
    #[cfg(test)]
    built: usize,
    /// Scroll to the folder shown once its row exists (after the tree opened down to it).
    scroll_to_current: bool,
    /// The pixels in a rem the rows were measured at.
    measured_at: Pixels,
    folder_tone: Option<KindTone>,
    /// The row (its [`Row::key`]) a drag of files is over and would drop into: lit while the drag is.
    drop_hover: Option<String>,
    /// The eject buttons' focus, by drive, so Tab reaches them.
    eject_focus: HashMap<String, FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for Sidebar {}
impl EventEmitter<SidebarRequest> for Sidebar {}

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
            cx.observe(&explorer, |this, _, cx| this.refresh_if_changed(cx)),
            cx.observe_global::<Persistence>(|this, cx| this.refresh_if_changed(cx)),
            cx.observe_global::<UiState>(|this, cx| this.refresh_if_changed(cx)),
            // A tag was set on something: the recent tags changed, and instant search lists what it has now.
            {
                let tags = tags::hub(cx);
                cx.subscribe(&tags, |this, _, _: &tags::TagsChanged, cx| {
                    this.read_tags(cx);
                    // The recent tags changed even if instant search's list did not.
                    this.refresh(cx);
                })
            },
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
            list: ListState::new(0, ListAlignment::Top, OVERDRAW),
            rows: Vec::new(),
            cursor: None,
            tree: Tree::default(),
            spaces: HashMap::new(),
            spaces_at: HashMap::new(),
            asking: HashSet::new(),
            counts: HashMap::new(),
            indexed_tags: Vec::new(),
            tags_asked: 0,
            last_hidden: state::settings(cx).show_hidden,
            last_reveal: None,
            last_listing: None,
            last_saved: Vec::new(),
            last_drives: None,
            seen: None,
            #[cfg(test)]
            built: 0,
            scroll_to_current: false,
            measured_at: theme::rem_size(cx),
            folder_tone: None,
            drop_hover: None,
            eject_focus: HashMap::new(),
            _subscriptions: subscriptions,
        };
        this.read_tags(cx);
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
            let listing = tree_listing(e).map(|l| (l.path.clone(), l.entries.clone()));
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

        // Instant search's tags, else the ones used here lately.
        let shown = shown_tags(&self.indexed_tags, tags::hub(cx).read(cx).recent());
        let built = build_rows(&Inputs {
            path: &path,
            known: &known,
            drives: &drives,
            pinned: &pinned,
            saved: &saved,
            remotes: &remotes,
            tags: &shown,
            folder_tree,
            tree: &self.tree,
            spaces: &self.spaces,
            counts: &self.counts,
        });
        self.set_rows(built, cx);
        #[cfg(test)]
        {
            self.built += 1;
        }
        self.seen = Some(Seen {
            known: arc_key(&known),
            drives: arc_key(&drives),
            listing: listing.as_ref().map(|(_, entries)| arc_key(entries)),
            path,
            folder_tree,
            tree_follows,
            show_hidden,
            pinned,
            saved,
            remotes,
        });
    }

    /// [`Sidebar::refresh`], unless everything the rows are built from is as the last refresh saw it.
    fn refresh_if_changed(&mut self, cx: &mut Context<Self>) {
        if !self.unchanged(cx) {
            self.refresh(cx);
        }
    }

    fn unchanged(&self, cx: &App) -> bool {
        let Some(seen) = &self.seen else {
            return false;
        };
        let e = self.explorer.read(cx);
        let s = state::settings(cx);
        let kept = match persistence::app(cx) {
            Some(app) => seen.pinned == app.pinned && seen.saved == app.saved_searches && seen.remotes == app.remotes,
            None => seen.pinned.is_empty() && seen.saved.is_empty() && seen.remotes.is_empty(),
        };
        kept && seen.path == e.current_path()
            && seen.known == arc_key(e.known_folders())
            && seen.drives == arc_key(e.drives())
            && seen.listing == tree_listing(e).map(|l| arc_key(&l.entries))
            && (seen.folder_tree, seen.tree_follows, seen.show_hidden) == (s.folder_tree, s.tree_follows, s.show_hidden)
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

    /// Asks instant search for every tag it lists, with its count (`allTags`, asked again whenever the recent tags
    /// change). Where it can't (it is off, or from before tags) the recent tags show instead.
    fn read_tags(&mut self, cx: &mut Context<Self>) {
        self.tags_asked += 1;
        let asked = self.tags_asked;
        let reading = (self.sources.tags)();
        cx.spawn(async move |this, cx| {
            let found = core_bridge::run(reading).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.tags_asked != asked {
                    return;
                }
                let listed = found.unwrap_or_default();
                if this.indexed_tags != listed {
                    this.indexed_tags = listed;
                    this.refresh(cx);
                }
            });
        })
        .detach();
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
            Row::Tag(tag) => cx.emit(SidebarEvent::RunSavedSearch(tag_search(&tag.name))),
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

    /// Ejects a drive (a USB drive made safe to remove, a disc) through the explorer, says how it went, and reads
    /// the drives again.
    fn eject(&mut self, path: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let ejecting = self.explorer.update(cx, |e, cx| e.eject(&path, cx));
        cx.spawn_in(window, async move |this, cx| {
            let result = ejecting.await;
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

    // ---- Menus and drops --------------------------------------------------------------------------------

    /// Asks for the shared folder menu on row `ix` (a place or a tree folder) at `position`.
    fn open_menu(&mut self, ix: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(path) = self
            .rows
            .get(ix)
            .filter(|r| matches!(r, Row::Place(_) | Row::Folder(_)))
            .and_then(Row::target)
        {
            cx.emit(SidebarRequest::ContextMenu {
                path: path.to_string(),
                position,
            });
        }
    }

    /// A right-click on row `ix`: the keyboard moves there and the menu is asked for at the pointer.
    fn right_click(&mut self, ix: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        window.focus(&self.focus, cx);
        self.open_menu(ix, position, cx);
        cx.notify();
    }

    /// The menu key or Shift+F10 on the row the keyboard is on: the menu opens at the row's bottom-start corner.
    fn keyboard_menu(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.cursor else {
            return false;
        };
        let bounds = self
            .list
            .bounds_for_item(ix)
            .unwrap_or_else(|| self.list.viewport_bounds());
        let x = if crate::i18n::is_rtl() {
            bounds.right()
        } else {
            bounds.left()
        };
        self.open_menu(ix, point(x, bounds.bottom()), cx);
        true
    }

    /// A drag of files moved over row `ix`: when the pointer is on it, says what a drop here would do (or that it
    /// would do nothing), and lights the row when it would take them (`targetAt`, `tell`).
    fn drag_over(&mut self, ix: usize, event: &DragMoveEvent<ExternalPaths>, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        let key = row.key();
        if !event.bounds.contains(&event.event.position) {
            if self.drop_hover.as_deref() == Some(&key) {
                self.drop_hover = None;
                cx.notify();
            }
            return;
        }
        let dragged = event
            .dragged_item()
            .downcast_ref::<ExternalPaths>()
            .map(drops::paths)
            .unwrap_or_default();
        let name = row_name(row);
        let accepted = drop::under(row, &name, &dragged);
        let hover = accepted.is_some().then_some(key);
        if let Some(under) = accepted {
            drops::answer(cx, Answer::Accept(under));
        }
        if self.drop_hover != hover {
            self.drop_hover = hover;
            cx.notify();
        }
    }

    /// Files dropped on row `ix`: copied or moved into the folder (the Recycle Bin recycles them), or sent to a
    /// paired PC. Done by the explorer.
    fn drop_on(&mut self, ix: usize, files: &ExternalPaths, cx: &mut Context<Self>) {
        self.drop_hover = None;
        cx.notify();
        let Some(target) = self.rows.get(ix).and_then(drop::target_of) else {
            return;
        };
        let paths = drops::paths(files);
        if paths.is_empty() {
            return;
        }
        // A link (Alt) is copied: the explorer makes no shortcuts.
        let moving = drops::effect(cx) == DropEffect::Move;
        match target {
            drop::Target::Folder(dest) => self
                .explorer
                .update(cx, |e, cx| e.drop_files(paths, &dest, moving, cx).detach()),
            drop::Target::Bin => self
                .explorer
                .update(cx, |e, cx| e.drop_files(paths, RECYCLE_BIN, true, cx).detach()),
        }
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
            "menu" => self.keyboard_menu(cx),
            "f10" if m.shift => self.keyboard_menu(cx),
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
        let focused = self.cursor == Some(ix) && self.focus.is_focused(window);
        let dropping = self.drop_hover.as_ref().is_some_and(|key| *key == row.key());
        match row {
            Row::Heading { .. } => self.draw_heading(ix, row, &t, focused, cx),
            Row::Place(p) => {
                // A drive that can be ejected has a button of its own that Tab reaches.
                let eject = p.eject.is_some().then(|| {
                    let handle = self
                        .eject_focus
                        .entry(p.path.to_lowercase())
                        .or_insert_with(|| cx.focus_handle().tab_stop(true))
                        .clone();
                    let on = handle.is_focused(window);
                    (handle, on)
                });
                self.draw_place(ix, p, &t, Look { focused, dropping }, eject, cx)
            }
            Row::Folder(f) => self.draw_folder(ix, f, &t, Look { focused, dropping }, cx),
            Row::Note { depth, reason } => self.draw_note(*depth, reason, &t),
            Row::Search(s) => self.draw_search(ix, s, &t, focused, cx),
            Row::Tag(tag) => self.draw_tag(ix, tag, &t, focused, cx),
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
        wrap.child(div().w(space(0.75)).h(height).rounded_full().bg(t.primary))
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
                RemoteKind::Webdav => IconName::Server,
            },
        }
    }

    /// A place's picture: a folder for a pinned folder, else a chip in the place's own color with its glyph.
    fn glyph_art(&self, glyph: &Glyph, across: Pixels, t: &Tokens, cx: &App) -> Div {
        use crate::explorer::DriveKind;
        use theme::Place;
        let place = match glyph {
            Glyph::Folder => return crate::art::folder(across, self.folder_color(t), None, cx),
            Glyph::Recent => Place::Recent,
            Glyph::Gallery => Place::Gallery,
            Glyph::Known(id) => Place::Known(*id),
            Glyph::Drive(DriveKind::Wsl) => Place::Wsl,
            Glyph::Drive(_) => Place::Drive,
            Glyph::RecycleBin => Place::RecycleBin,
            Glyph::Remote(..) => Place::Remote,
        };
        crate::art::chip(across, theme::place_tone(place, cx), Self::glyph_icon(glyph), cx)
    }

    fn draw_place(
        &self,
        ix: usize,
        p: &Place,
        t: &Tokens,
        look: Look,
        eject: Option<(FocusHandle, bool)>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Look { focused, dropping } = look;
        let label: SharedString = match &p.label {
            Label::Fixed(text) => t!(*text),
            Label::Plain(text) => text.clone(),
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
        let (glass, edge) = (t.glass, t.stroke);
        let (lit, lit_ring) = (t.primary.opacity(0.2), t.primary);
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
            .when(active, move |d| d.bg(glass).border_color(edge).text_color(foreground))
            .when(dropping, move |d| {
                d.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(lit).border_color(lit_ring))
            })
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.open_in_new_tab(ix, cx)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    this.right_click(ix, event.position, window, cx)
                }),
            )
            .on_drag_move::<ExternalPaths>(
                cx.listener(move |this, event: &DragMoveEvent<ExternalPaths>, _, cx| this.drag_over(ix, event, cx)),
            )
            .on_drop::<ExternalPaths>(
                cx.listener(move |this, files: &ExternalPaths, _, cx| this.drop_on(ix, files, cx)),
            )
            .tooltip(tip)
            .when(active, |d| d.child(Self::pill(t, space(4.))))
            .child(self.glyph_art(&p.glyph, theme::rem_size(cx) * 1.25, t, cx))
            .child(text);

        let mut cell = div().group("row").relative().w_full().pb(space(0.25)).child(button);
        if let Some(opens) = p.tree {
            cell = cell.child(self.arrow(ix, opens, space(1.), t, cx));
        }
        if let (Some(name), Some((handle, on))) = (&p.eject, eject) {
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
                            .track_focus(&handle)
                            .p(space(1.))
                            .rounded(theme::size::RADIUS_SM)
                            .border_1()
                            .border_color(ring(t, on))
                            .cursor_pointer()
                            .text_color(t.muted_foreground)
                            .opacity(if on { 1. } else { 0. })
                            .group_hover("row", |s| s.opacity(1.))
                            .hover(move |s| s.bg(hover))
                            .tooltip(tooltip_of(
                                t!("Safely eject {name}", name = name),
                                SharedString::default(),
                            ))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.eject(path.clone(), label.clone(), window, cx)
                            }))
                            // Enter and Space press it (a click); the arrows hand the keyboard back to the list.
                            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "up" | "down" | "home" | "end") {
                                    window.focus(&this.focus, cx);
                                }
                            }))
                            .child(icon(IconName::Unplug).size_3p5()),
                    ),
            );
        }
        cell.into_any_element()
    }

    fn draw_folder(&self, ix: usize, f: &rows::Folder, t: &Tokens, look: Look, cx: &mut Context<Self>) -> AnyElement {
        let Look { focused, dropping } = look;
        // How far in a folder `depth` levels below a place is: past the place's own icon, a step a level.
        let indent = TREE_INDENT + f.depth as f32 * TREE_STEP;
        let (lit, lit_ring) = (t.primary.opacity(0.2), t.primary);
        let tip = tooltip_of(f.name.clone(), SharedString::from(f.path.clone()));
        let (active, accent, foreground) = (f.active, t.accent, t.foreground);
        let (glass, edge) = (t.glass, t.stroke);
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
            .ps(indent)
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_size(TREE_TEXT)
            .text_color(t.foreground.opacity(0.85))
            .hover(move |s| s.bg(accent))
            .when(active, move |d| d.bg(glass).border_color(edge).text_color(foreground))
            .when(dropping, move |d| {
                d.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(lit).border_color(lit_ring))
            })
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.open_in_new_tab(ix, cx)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    this.right_click(ix, event.position, window, cx)
                }),
            )
            .on_drag_move::<ExternalPaths>(
                cx.listener(move |this, event: &DragMoveEvent<ExternalPaths>, _, cx| this.drag_over(ix, event, cx)),
            )
            .on_drop::<ExternalPaths>(
                cx.listener(move |this, files: &ExternalPaths, _, cx| this.drop_on(ix, files, cx)),
            )
            .tooltip(tip)
            .when(active, |d| d.child(Self::pill(t, space(3.5))))
            .child(crate::art::folder(theme::rem_size(cx), self.folder_color(t), None, cx))
            .child(div().flex_1().min_w_0().truncate().child(isolate(f.name.clone())));

        let mut cell = div().group("row").relative().w_full().pb(space(0.25)).child(button);
        if f.has != Some(false) {
            let opens = rows::Opens {
                open: f.open,
                loading: f.loading,
            };
            cell = cell.child(self.arrow(ix, opens, space(indent - 4.5), t, cx));
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
            .ps(TREE_INDENT + depth as f32 * TREE_STEP)
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
            .pb(space(0.25))
            .child(button)
            .child(remove)
            .into_any_element()
    }

    /// A tag: its color's dot, its name, and how many items have it where instant search says. A click finds what
    /// has it, on every drive.
    fn draw_tag(&self, ix: usize, tag: &rows::Tag, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let name = SharedString::from(tag.name.clone());
        let tagged = t!("What's tagged “{tag}”", tag = tag.name.clone());
        let title: SharedString = match tag.count {
            Some(count) => format!("{tagged} ({count})").into(),
            None => tagged,
        };
        let accent = t.accent;
        let button = row_start()
            .id(("tag", ix))
            .role(Role::Button)
            .aria_label(name.clone())
            .aria_description(title.clone())
            .test_support()
            .w_full()
            .gap(space(2.5))
            .px(space(2.))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_color(t.foreground.opacity(0.9))
            .hover(move |s| s.bg(accent))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .tooltip(tooltip_of(title, SharedString::default()))
            .child(
                div()
                    .size(space(4.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(space(2.5)).rounded_full().bg(tags::tag_color(&tag.name))),
            )
            .child(div().flex_1().min_w_0().truncate().child(isolate(name)))
            .children(tag.count.map(|count| {
                div()
                    .flex_none()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(SharedString::from(count.to_string()))
            }));
        div().w_full().pb(space(0.25)).child(button).into_any_element()
    }

    fn draw_add(&self, ix: usize, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let accent = t.accent;
        div()
            .w_full()
            .pb(space(0.25))
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

/// How a row is drawn beyond what it is: the keyboard is on it, and a drag of files that it would take is over it.
#[derive(Clone, Copy)]
struct Look {
    focused: bool,
    dropping: bool,
}

/// The ring a row shows while the keyboard is on it; transparent otherwise (so rows don't move).
fn ring(t: &Tokens, focused: bool) -> Hsla {
    if focused {
        t.ring
    } else {
        transparent_black()
    }
}

/// The tooltip builder for `title` and `detail` (the TypeScript's `title="label\ndetail"`): the kit's tooltip with
/// the detail on a second line.
fn tooltip_of(title: SharedString, detail: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let title = isolate(title);
    let text: SharedString = if detail.is_empty() {
        title
    } else {
        format!(
            "{title}
{detail}"
        )
        .into()
    };
    move |window, cx| tooltip(text.clone(), window, cx)
}

/// What a place or folder row is called (the drag picture's words, `dropName`).
fn row_name(row: &Row) -> String {
    match row {
        Row::Place(p) => match &p.label {
            Label::Fixed(text) => t!(*text).to_string(),
            Label::Plain(text) => text.to_string(),
        },
        Row::Folder(f) => f.name.to_string(),
        _ => String::new(),
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = crate::perf::span("sidebar");
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
            .pt(space(2.))
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
