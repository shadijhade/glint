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
//! **Added since the TypeScript:** the Portals section ([`portals`]: the live portals mounted under Glint portals,
//! their menu, and a change notice from glint-core) and the Workspaces button at the top ([`workspaces`]: the
//! window's tabs and panes kept under a name, switched to from its menu or from Ctrl+K).
//!
//! **Compact, since 0.17.** The list's items are [`rows::Line`]s, not rows: Quick access is tiles, two to a line
//! (or a list again, from the switch at its title: a tile has no room for the folder tree); the labels are one
//! line of dots after their title and the tags are chips after theirs; a section folds to its title (kept in
//! `settings.sidebar_folded`); and This PC comes before what finds things, so the drives are in sight without a
//! scroll. A row is still a row to everything that acts on one (the keyboard, a click, a drop, its menu, Hop):
//! only where it is drawn changed. The arrows move by lines, and Left and Right along a line of tiles, dots or
//! chips.
//!
//! **Not in the TypeScript, and so not here:** the "rail" below the `md` breakpoint (a 48 px icon-only sidebar) is
//! the window width's business in CSS, and the GPUI workspace resizes the sidebar between fixed limits instead.

mod drop;
mod portals;
mod rows;
mod tree;
pub mod workspaces;

use crate::components::tab_strip::menu as tab_menu;
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
use glint_model::format::{format_size, plural};
use glint_model::marks::Label as MarkLabel;
use glint_model::paths::{same_path, RECYCLE_BIN, THIS_PC};
use glint_model::store::{RemoteConnection, RemoteKind, SavedSearch};
use portals::{PortalChanges, PortalWork, PortalsSource};
use rows::{
    build_rows, count_key, is_label, label_search, lines as lay_out, reads_files, shown_labels, shown_tags,
    tag_search, Glyph, Inputs, Label, LabelRow, Line, Place, PortalItem, Row, Space,
};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::{KindTone, Tokens};

pub use rows::{remote_path, remote_where};
pub use tree::{core_subfolders, key_of, reveal_chain, Kids, Subfolder, SubfolderSource, Tree};

/// Rows the list draws beyond the visible ones, so scrolling doesn't show blanks.
const OVERDRAW: Pixels = px(300.);

/// A tree folder's text (`text-[13px]`): between the theme's `XS` and `SM`.
const TREE_TEXT: Rems = rems(0.8125);

/// Spacing units above and below a row's words: with them a row is 30 px at the usual text size (it was 36), and
/// the whole rail of a new PC is in sight without a scroll.
pub(super) const ROW_PAD: f32 = 1.;

/// The rail's width, in rems, from which a drive's line has room for how much is free on it after its whole name
/// (at the width the rail starts with, the number would cut "New Volume (D:)" before its letter; the bar says how
/// full it is at any width, and the tooltip the numbers).
const FREE_SHOWN_FROM: f32 = 17.;

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

/// Every label and every tag in use, each with how many items have it (glint-core's `marks_all`: the index's counts
/// with the ledger's), most used first: `(labels, tags)`. An error where marks can't be counted; the tags are asked
/// of [`TagsSource`] then.
pub type MarksSource = Arc<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<(Vec<(String, u64)>, Vec<(String, u64)>), String>> + Send>>
        + Send
        + Sync,
>;

/// The paths that have a tag, on every drive that can be searched (at most [`MOST_TAGGED`]).
pub type TaggedSource =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Vec<String>, String>> + Send>> + Send + Sync>;

/// The most paths a tag's menu works on.
pub const MOST_TAGGED: usize = 100_000;

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
    /// The labels and tags in use, with their counts: asked first, `tags` only when this fails.
    pub marks: MarksSource,
    /// What has a tag (its menu's *Rename everywhere…* and *Take off every file…*).
    pub tagged: TaggedSource,
    /// The live portals mounted now.
    pub portals: PortalsSource,
    /// Told whenever one is mounted or unmounted.
    pub portal_changes: PortalChanges,
    /// Reads a portal's source again (by its id).
    pub portal_refresh: PortalWork,
    /// Unmounts a portal (by its id).
    pub portal_unmount: PortalWork,
}

impl Sources {
    /// glint-core.
    pub fn core() -> Sources {
        let (portals, portal_changes, portal_refresh, portal_unmount) = Sources::core_portals();
        Sources {
            portals,
            portal_changes,
            portal_refresh,
            portal_unmount,
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
            marks: Arc::new(|| {
                Box::pin(async {
                    glint_core::marks::marks_all()
                        .await
                        .map(|counts| (counts.labels, counts.tags))
                })
            }),
            // Needs the app handle, which `Sidebar::new` gives it ([`Sources::core_tagged`]).
            tagged: Arc::new(|_| Box::pin(async { Err("Glint's search isn't ready yet".to_string()) })),
        }
    }

    /// What has a tag, from glint-core's search (`find_files`, as the address bar's): `tag:"name"` on every fixed and
    /// removable drive, at most [`MOST_TAGGED`] paths. An error where the search fails.
    pub fn core_tagged(app: glint_core::host::AppHandle) -> TaggedSource {
        use glint_core::find::{find_files, FindEvent, FindOptions};
        Arc::new(move |tag| {
            let app = app.clone();
            let (sender, receiver) = async_channel::unbounded::<FindEvent>();
            // Runs on core's search thread; the paths are collected below, off the UI thread.
            let channel = glint_core::host::ipc::Channel::new(move |event| {
                sender
                    .try_send(event)
                    .map_err(|_| glint_core::host::Error::from("the search was left"))
            });
            let running = core_bridge::run(async move {
                use glint_core::host::Manager as _;
                let state = app
                    .try_state::<glint_core::AppState>()
                    .ok_or_else(|| "Glint's search isn't ready yet".to_string())?;
                let roots: Vec<String> = glint_core::fsx::list_drives()
                    .await?
                    .into_iter()
                    .filter(|d| matches!(d.kind, "fixed" | "removable"))
                    .map(|d| d.path)
                    .collect();
                let options = FindOptions {
                    hidden: true,
                    respect_ignore: false,
                    exclude: Vec::new(),
                    max_results: Some(MOST_TAGGED),
                    fuzzy: false,
                    rank: false,
                    marks: false,
                };
                let query = tag_search(&tag).query;
                find_files(state, roots, query, options, channel, "sidebar-tag".to_string()).await
            });
            Box::pin(async move {
                running.await.and_then(|started| started)?;
                let mut paths = Vec::new();
                while let Ok(event) = receiver.recv().await {
                    match event {
                        FindEvent::Found(entries) => paths.extend(entries.into_iter().filter_map(|e| e.path)),
                        FindEvent::Error { message } => return Err(message),
                        FindEvent::Finished { .. } => break,
                        FindEvent::Started { .. } | FindEvent::Marked(_) => {}
                    }
                }
                paths.truncate(MOST_TAGGED);
                Ok(paths)
            })
        })
    }
}

/// What [`Sidebar::refresh`] builds the rows from, kept to compare against: shared lists by pointer (a new listing
/// or list of drives is a new `Arc`; holding the ones seen keeps their addresses from being reused), the rest by
/// value.
struct Seen {
    path: String,
    known: Arc<[crate::explorer::KnownFolder]>,
    drives: Arc<[crate::explorer::Drive]>,
    listing: Option<Arc<[FileEntry]>>,
    folder_tree: bool,
    tree_follows: bool,
    show_hidden: bool,
    /// Quick access as tiles, and the sections folded (the settings').
    tiles: bool,
    folded: Vec<String>,
    pinned: Vec<String>,
    saved: Vec<SavedSearch>,
    remotes: Vec<RemoteConnection>,
    portals: Vec<PortalItem>,
    labels: Vec<MarkLabel>,
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
    /// The lines the rows are drawn in: the list's items ([`rows::lines`]).
    lines: Vec<Line>,
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
    /// Every label the index and the ledger count, with its count (empty where they count none).
    indexed_labels: Vec<(String, u64)>,
    tags_asked: u64,
    /// A tag's menu while it is open.
    tag_menu: Option<tab_menu::Popup>,
    // What the last refresh saw, to do each thing once per change (the TypeScript's effect dependencies).
    last_hidden: bool,
    last_reveal: Option<(bool, bool, String)>,
    last_listing: Option<Arc<[FileEntry]>>,
    last_saved: Vec<SavedSearch>,
    last_drives: Option<Arc<[crate::explorer::Drive]>>,
    /// The live portals mounted, as last read, and the menu of one while it is open.
    portals: Vec<PortalItem>,
    portal_menu: Option<crate::components::tab_strip::menu::Popup>,
    /// Whether the list had more below its last row at the last frame: the edge fades while it has.
    fade_below: bool,
    /// Whether the rail was wide enough at the last frame for a drive's line to say how much is free. Read in
    /// `render`, never while the list draws: the list holds its own state then (reading it there panics).
    roomy: bool,
    /// The Workspaces menu while it is open.
    workspace_menu: Option<crate::components::tab_strip::menu::Popup>,
    /// Where the Workspaces trigger sits so mouse and keyboard open the menu flush beneath it.
    workspace_bounds: Rc<Cell<Bounds<Pixels>>>,
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
        let mut sources = Sources::core();
        if let Some(core) = cx.try_global::<core_bridge::Core>() {
            sources.tagged = Sources::core_tagged(core.app.clone());
        }
        Self::with_sources(sources, window, cx)
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
            lines: Vec::new(),
            cursor: None,
            tree: Tree::default(),
            spaces: HashMap::new(),
            spaces_at: HashMap::new(),
            asking: HashSet::new(),
            counts: HashMap::new(),
            indexed_tags: Vec::new(),
            indexed_labels: Vec::new(),
            tags_asked: 0,
            tag_menu: None,
            last_hidden: state::settings(cx).show_hidden,
            last_reveal: None,
            last_listing: None,
            last_saved: Vec::new(),
            last_drives: None,
            portals: Vec::new(),
            portal_menu: None,
            workspace_menu: None,
            workspace_bounds: Rc::default(),
            fade_below: false,
            roomy: true,
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
        // The bottom edge's fade follows the scroll: the view is drawn again only when it flips. The list still
        // holds its state while this handler runs, so it's read once the scroll is done (reading it here panicked
        // inside the window procedure, and Windows closed Glint: 0.16.0).
        let weak = cx.entity().downgrade();
        this.list.set_scroll_handler(move |_, _, cx| {
            let weak = weak.clone();
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| {
                    if this.more_below() != this.fade_below {
                        cx.notify();
                    }
                })
                .ok();
            });
        });
        this.portals = (this.sources.portals)();
        this.watch_portals(cx);
        this.read_tags(cx);
        this.refresh(cx);
        this
    }

    /// Whether the list has rows below the ones in view (as of the last layout).
    fn more_below(&self) -> bool {
        let max = self.list.max_offset_for_scrollbar().y;
        let at = -self.list.scroll_px_offset_for_scrollbar().y;
        max - at > px(1.)
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
        let (folder_tree, tree_follows, show_hidden, tiles, folded) = {
            let s = state::settings(cx);
            (
                s.folder_tree,
                s.tree_follows,
                s.show_hidden,
                s.sidebar_tiles,
                s.sidebar_folded.clone(),
            )
        };
        let labels = state::settings(cx).labels.clone();
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
        // A tag that is a label's name is listed under Labels, so it doesn't take a place among the twelve.
        let indexed: Vec<(String, u64)> = self
            .indexed_tags
            .iter()
            .filter(|(name, _)| !is_label(&labels, name))
            .cloned()
            .collect();
        let recent: Vec<String> = tags::hub(cx)
            .read(cx)
            .recent()
            .iter()
            .filter(|name| !is_label(&labels, name))
            .cloned()
            .collect();
        let shown = shown_tags(&indexed, &recent);
        let label_rows = shown_labels(&labels, &self.indexed_labels, &self.indexed_tags);
        let built = build_rows(&Inputs {
            path: &path,
            known: &known,
            drives: &drives,
            pinned: &pinned,
            saved: &saved,
            remotes: &remotes,
            portals: &self.portals,
            labels: &label_rows,
            tags: &shown,
            folder_tree,
            tiles,
            folded: &folded,
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
            known,
            drives,
            listing: listing.map(|(_, entries)| entries),
            path,
            folder_tree,
            tree_follows,
            show_hidden,
            tiles,
            folded,
            pinned,
            saved,
            remotes,
            portals: self.portals.clone(),
            labels,
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
        let same_listing = match (&seen.listing, tree_listing(e)) {
            (Some(seen), Some(now)) => Arc::ptr_eq(seen, &now.entries),
            (None, None) => true,
            _ => false,
        };
        kept && seen.portals == self.portals
            && same_listing
            && seen.labels == s.labels
            && seen.path == e.current_path()
            && Arc::ptr_eq(&seen.known, e.known_folders())
            && Arc::ptr_eq(&seen.drives, e.drives())
            && (seen.folder_tree, seen.tree_follows, seen.show_hidden) == (s.folder_tree, s.tree_follows, s.show_hidden)
            && seen.tiles == s.sidebar_tiles
            && seen.folded == s.sidebar_folded
    }

    /// Replaces the rows, and the part of the list that differs. The keyboard's row stays on the same thing.
    fn set_rows(&mut self, built: Vec<Row>, cx: &mut Context<Self>) {
        if built != self.rows {
            let laid = lay_out(&built);
            // The list's items are lines. A line is the same while it draws the same rows the same way, wherever
            // in the list those rows now are.
            let (same_start, same_end) = {
                let same = |a: &Line, b: &Line| {
                    let (was, is) = (a.rows(), b.rows());
                    std::mem::discriminant(a) == std::mem::discriminant(b)
                        && was.len() == is.len()
                        && was.iter().zip(&is).all(|(o, n)| self.rows.get(*o) == built.get(*n))
                };
                let (old, new) = (self.lines.len(), laid.len());
                let same_start = self.lines.iter().zip(&laid).take_while(|(a, b)| same(a, b)).count();
                let same_end = self
                    .lines
                    .iter()
                    .rev()
                    .zip(laid.iter().rev())
                    .take(old.min(new) - same_start)
                    .take_while(|(a, b)| same(a, b))
                    .count();
                (same_start, same_end)
            };
            let (old, new) = (self.lines.len(), laid.len());
            let kept = self
                .cursor
                .and_then(|c| self.rows.get(c))
                .map(|r| (r.key(), self.cursor.unwrap_or(0)));
            self.rows = built;
            self.lines = laid;
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
                self.list.scroll_to_reveal_item(self.line_of(at));
            }
        }
    }

    /// The line row `ix` is drawn in.
    fn line_of(&self, ix: usize) -> usize {
        self.lines.iter().position(|line| line.holds(ix)).unwrap_or(0)
    }

    /// Folds the section titled `title` to its title, or opens it again (kept in the settings).
    fn toggle_fold(&mut self, title: &'static str, cx: &mut Context<Self>) {
        state::update(cx, |s| match s.sidebar_folded.iter().position(|f| f == title) {
            Some(at) => {
                s.sidebar_folded.remove(at);
            }
            None => s.sidebar_folded.push(title.to_string()),
        });
    }

    /// Quick access as tiles, or as a list whose places open their folders.
    fn toggle_tiles(&mut self, cx: &mut Context<Self>) {
        state::update(cx, |s| s.sidebar_tiles = !s.sidebar_tiles);
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
        // What the last reveal opened elsewhere closes again.
        for folder in self.tree.reveal(&chain) {
            self.load(folder, cx);
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

    /// Asks for every label and tag in use, with its count (`marks_all`, asked again whenever the recent tags
    /// change). Where marks can't be counted the tags are asked of instant search as before (`allTags`), and where
    /// it can't either (it is off, or from before tags) the recent tags show instead.
    fn read_tags(&mut self, cx: &mut Context<Self>) {
        self.tags_asked += 1;
        let asked = self.tags_asked;
        let marks = (self.sources.marks)();
        let tags = (self.sources.tags)();
        cx.spawn(async move |this, cx| {
            let found = match core_bridge::run(marks).await.and_then(|inner| inner) {
                Ok(counted) => counted,
                Err(_) => (
                    Vec::new(),
                    core_bridge::run(tags).await.and_then(|inner| inner).unwrap_or_default(),
                ),
            };
            let _ = this.update(cx, |this, cx| {
                if this.tags_asked != asked {
                    return;
                }
                let (labels, tags) = found;
                if this.indexed_tags != tags || this.indexed_labels != labels {
                    this.indexed_tags = tags;
                    this.indexed_labels = labels;
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
            Row::Label(label) => cx.emit(SidebarEvent::RunSavedSearch(label_search(&label.name))),
            Row::Tag(tag) => cx.emit(SidebarEvent::RunSavedSearch(tag_search(&tag.name))),
            Row::AddConnection => cx.emit(SidebarEvent::AddConnection),
            Row::Portal(p) => self
                .explorer
                .update(cx, |e, cx| e.navigate(p.path, NavOptions::default(), cx)),
            // A title that opens nothing folds its section, where it folds.
            Row::Heading {
                title, folded: Some(_), ..
            } => self.toggle_fold(title, cx),
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

    /// Hop mode (Alt+J): row `ix` is a target on show, and a hint on it is a click on it.
    fn hop<E: ParentElement>(&self, el: E, ix: usize, name: impl Into<SharedString>, cx: &mut Context<Self>) -> E {
        crate::components::hop::mark(el, || {
            let me = cx.entity().downgrade();
            crate::components::hop::Mark::new(
                crate::components::hop::Kind::Place,
                cx.entity_id().as_u64(),
                name,
                move |window, cx| {
                    me.update(cx, |this, cx| this.click(ix, window, cx)).ok();
                },
            )
        })
    }

    /// Middle-click on a folder: a new tab for it, as in a browser (`middleClickOpens`).
    fn open_in_new_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(target) = self
            .rows
            .get(ix)
            .filter(|r| matches!(r, Row::Place(_) | Row::Folder(_) | Row::Portal(_)))
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
    fn open_menu(&mut self, ix: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.open_portal_menu(ix, position, window, cx) || self.open_tag_menu(ix, position, window, cx) {
            return;
        }
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

    /// Opens row `ix`'s menu at `at`, when it is a tag: *Rename everywhere…* and *Take off every file…*.
    fn open_tag_menu(&mut self, ix: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(Row::Tag(tag)) = self.rows.get(ix) else {
            return false;
        };
        let name = tag.name.clone();
        let nodes = vec![
            tab_menu::row(t!("Rename everywhere…"), IconName::Tag, "rename", name.clone()).node(),
            tab_menu::row(t!("Take off every file…"), IconName::Eraser, "take-off", name).node(),
        ];
        self.portal_menu = None;
        self.workspace_menu = None;
        self.tag_menu = tab_menu::Popup::open(
            nodes.into_iter().flatten().collect(),
            t!("Tag"),
            at,
            window,
            cx,
            |this, chose, window, cx| {
                this.tag_menu = None;
                // The keyboard comes back to the row it was on.
                window.focus(&this.focus, cx);
                if let Some(chose) = chose {
                    this.tag_chose(&chose.tag, chose.arg, window, cx);
                }
                cx.notify();
            },
        );
        cx.notify();
        true
    }

    /// A row of a tag's menu was chosen: finds what has the tag (off the UI thread), and asks, or says that nothing
    /// is found.
    fn tag_chose(&mut self, what: &str, tag: String, window: &mut Window, cx: &mut Context<Self>) {
        let rename = match what {
            "rename" => true,
            "take-off" => false,
            _ => return,
        };
        let finding = (self.sources.tagged)(tag.clone());
        // The dialog opens at once and says it is finding what has the tag (long where the index doesn't cover a
        // drive); what is found fills it in, or closes it with a notice when nothing is.
        let (rename_view, take_off_view) = if rename {
            (Some(open_rename_tag(tag.clone(), window, cx).downgrade()), None)
        } else {
            (None, Some(open_take_off_tag(tag.clone(), window, cx).downgrade()))
        };
        cx.spawn_in(window, async move |this, cx| {
            let found = core_bridge::run(finding).await.and_then(|inner| inner);
            let _ = this.update_in(cx, |_, window, cx| {
                let found = found.ok().filter(|paths| !paths.is_empty());
                // A dialog the person already closed is left alone, and so is whatever opened since.
                let alive = rename_view.as_ref().is_some_and(|v| v.upgrade().is_some())
                    || take_off_view.as_ref().is_some_and(|v| v.upgrade().is_some());
                if !alive {
                    return;
                }
                match found {
                    Some(paths) => {
                        if let Some(view) = rename_view.and_then(|v| v.upgrade()) {
                            view.update(cx, |view, cx| view.found(paths, cx));
                        } else if let Some(view) = take_off_view.and_then(|v| v.upgrade()) {
                            view.update(cx, |view, cx| view.found(paths, cx));
                        }
                    }
                    None => {
                        window.close_dialog(cx);
                        window.push_notification(
                            Notification::info(t!("Nothing has the tag “{tag}” where Glint can search", tag = tag)),
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

    /// A right-click on row `ix`: the keyboard moves there and the menu is asked for at the pointer.
    fn right_click(&mut self, ix: usize, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        window.focus(&self.focus, cx);
        self.open_menu(ix, position, window, cx);
        cx.notify();
    }

    /// The menu key or Shift+F10 on the row the keyboard is on: the menu opens at the row's bottom-start corner.
    fn keyboard_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(ix) = self.cursor else {
            return false;
        };
        let at = self.line_of(ix);
        let bounds = self
            .list
            .bounds_for_item(at)
            .unwrap_or_else(|| self.list.viewport_bounds());
        // The second tile of a line starts at the line's middle.
        let second = matches!(self.lines.get(at), Some(Line::Pair(_, Some(second))) if *second == ix);
        let x = if second {
            bounds.center().x
        } else if crate::i18n::is_rtl() {
            bounds.right()
        } else {
            bounds.left()
        };
        self.open_menu(ix, point(x, bounds.bottom()), window, cx);
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
            // Along a line of tiles, dots or chips; else into and out of the tree.
            "right" if !rtl => self.along(1, cx) || self.expand(cx),
            "left" if !rtl => self.along(-1, cx) || self.collapse(cx),
            "left" => self.along(1, cx) || self.expand(cx),
            "right" => self.along(-1, cx) || self.collapse(cx),
            "enter" | "space" => match self.cursor {
                Some(ix) => {
                    self.activate(ix, window, cx);
                    true
                }
                None => false,
            },
            "menu" => self.keyboard_menu(window, cx),
            "f10" if m.shift => self.keyboard_menu(window, cx),
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

    /// The rows the keyboard can land on, line by line (lines with none left out).
    fn stops(&self) -> Vec<Vec<usize>> {
        self.lines
            .iter()
            .map(|line| {
                let mut cells = line.rows();
                cells.retain(|&ix| self.rows.get(ix).is_some_and(Row::focusable));
                cells
            })
            .filter(|cells| !cells.is_empty())
            .collect()
    }

    /// Where the keyboard is among [`Sidebar::stops`]: the line, and the place in it.
    fn stop_at(&self, stops: &[Vec<usize>]) -> Option<(usize, usize)> {
        let cursor = self.cursor?;
        stops
            .iter()
            .enumerate()
            .find_map(|(line, cells)| cells.iter().position(|&ix| ix == cursor).map(|cell| (line, cell)))
    }

    /// Moves the keyboard `delta` lines away: to the tile in the same half of a line of tiles, else to the line's
    /// first row. Where every row has a line to itself that is the next row, as it always was.
    fn step(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let stops = self.stops();
        let Some(last) = stops.len().checked_sub(1) else {
            return false;
        };
        let tile = |ix: usize| matches!(self.rows.get(ix), Some(Row::Place(p)) if p.tile);
        let to = match self.stop_at(&stops) {
            None if delta > 0 => stops[0][0],
            None => stops[last][0],
            Some((line, cell)) => {
                let next = (line as isize + delta).clamp(0, last as isize) as usize;
                let cells = &stops[next];
                if next == line {
                    stops[line][cell]
                } else if tile(stops[line][cell]) && tile(cells[0]) {
                    cells[cell.min(cells.len() - 1)]
                } else {
                    cells[0]
                }
            }
        };
        self.set_cursor(Some(to), cx);
        true
    }

    /// Moves the keyboard along its line (`delta` 1 is toward the end), where the line has another row that way
    /// to land on. False where it has none: Left and Right then mean what they mean on a line of one (Left on the
    /// tags' title folds them).
    fn along(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let stops = self.stops();
        let Some((line, cell)) = self.stop_at(&stops) else {
            return false;
        };
        let cells = &stops[line];
        let to = (cell as isize + delta).clamp(0, cells.len() as isize - 1) as usize;
        if to == cell {
            return false;
        }
        self.set_cursor(Some(cells[to]), cx);
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
            self.list.scroll_to_reveal_item(self.line_of(ix));
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
        // A folded section's title: its section opens.
        if let Row::Heading { title, folded, .. } = row {
            let (title, folded) = (*title, *folded);
            if folded == Some(true) {
                self.toggle_fold(title, cx);
            }
            return folded.is_some();
        }
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
        // An open section's title: its section folds.
        if let Row::Heading { title, folded, .. } = row {
            let (title, folded) = (*title, *folded);
            if folded == Some(false) {
                self.toggle_fold(title, cx);
            }
            return folded.is_some();
        }
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

    /// One line of the list: a row across the rail, two tiles, the labels' dots or the tags' chips.
    fn draw_line(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(line) = self.lines.get(at).cloned() else {
            return div().into_any_element();
        };
        match line {
            Line::One(ix) => self.draw_row(ix, window, cx),
            Line::Pair(first, second) => {
                let first = self.draw_row(first, window, cx);
                let second = second.map(|ix| self.draw_row(ix, window, cx));
                row_start()
                    .w_full()
                    .gap(space(0.5))
                    .child(div().flex_1().min_w_0().child(first))
                    .child(div().flex_1().min_w_0().children(second))
                    .into_any_element()
            }
            // The title starts the line and what it titles follows it, wrapping where there is more than a line of it.
            Line::Dots { heading, from, to } | Line::Chips { heading, from, to } => {
                let t = theme::tokens(cx).clone();
                let title = self.rows.get(heading).map(|row| self.heading_title(heading, row, &t, false, cx));
                let cells: Vec<AnyElement> = (from..to).map(|ix| self.draw_row(ix, window, cx)).collect();
                // Dots stand close, as one mark of several colors; chips need air between their edges.
                let dots = matches!(line, Line::Dots { .. });
                row_start()
                    .group("heading")
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .gap_x(space(if dots { 0.5 } else { 1. }))
                    .gap_y(space(1.))
                    .pt(space(3.))
                    .pb(space(0.5))
                    .px(space(2.))
                    .children(title.map(|title| div().flex_none().pe(1.).child(title)))
                    .children(cells)
                    .into_any_element()
            }
        }
    }

    fn draw_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        // The ring is for the keyboard: a row that was clicked is plain from its fill, and has none.
        let focused = self.cursor == Some(ix) && self.focus.is_focused(window) && window.last_input_was_keyboard();
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
            Row::Note { depth, reason } => self.draw_note(ix, *depth, reason, &t),
            Row::Search(s) => self.draw_search(ix, s, &t, focused, cx),
            Row::Label(label) => self.draw_label(ix, label, &t, focused, cx),
            Row::Tag(tag) => self.draw_tag(ix, tag, &t, focused, cx),
            Row::Portal(portal) => self.draw_portal(ix, portal, &t, focused, cx),
            Row::AddConnection => self.draw_add(ix, &t, focused, cx),
        }
    }

    /// The folder icon's colors (the amber of folders), read once per frame.
    fn folder_color(&self, t: &Tokens) -> Hsla {
        self.folder_tone.map_or(t.muted_foreground, |tone| tone.color)
    }

    /// A section's title on a line of its own: the title, and at the line's end what the section has to switch
    /// (Quick access: tiles or a list).
    fn draw_heading(&self, ix: usize, row: &Row, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let Row::Heading {
            first, folded, tiles, ..
        } = row
        else {
            return div().into_any_element();
        };
        let switch = tiles.filter(|_| *folded != Some(true)).map(|on| {
            let name = if on {
                t!("Show Quick access as a list")
            } else {
                t!("Show Quick access as tiles")
            };
            let (hover, pressed, strong) = (t.accent, t.pressed, t.foreground);
            div()
                .id("quick-access-view")
                .role(Role::Button)
                .aria_label(name.clone())
                .test_support()
                .p(space(0.5))
                .rounded(theme::size::RADIUS_SM)
                .cursor_pointer()
                .text_color(t.muted_foreground)
                // Quiet until the pointer is on the title's line (Settings → Files has the same switch, for the
                // keyboard), or the keyboard on the title.
                .opacity(if focused { 1. } else { 0. })
                .group_hover("heading", |s| s.opacity(1.))
                .hover(move |s| s.bg(hover).text_color(strong))
                .active(move |s| s.bg(pressed))
                .tooltip(tooltip_of(name, SharedString::default()))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_tiles(cx)))
                .child(icon(if on { IconName::List } else { IconName::LayoutGrid }).size_3p5())
        });
        row_start()
            .group("heading")
            .w_full()
            .items_center()
            .gap(space(1.))
            .pt(space(if *first { 0.5 } else { 3. }))
            .pb(space(0.5))
            .px(space(2.))
            .child(self.heading_title(ix, row, t, focused, cx))
            .child(div().flex_1())
            .children(switch)
            .into_any_element()
    }

    /// A section's title, and after it the arrow that folds the section where it folds. The title opens what it
    /// names where it names a place (This PC); another that folds does so on a press as well, so the whole word is
    /// the target and the arrow is what a screen reader and the Tab key find.
    fn heading_title(&self, ix: usize, row: &Row, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let Row::Heading {
            title,
            link,
            active,
            folded,
            ..
        } = row
        else {
            return div().into_any_element();
        };
        let (title, folded) = (*title, *folded);
        let name = t!(title);
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
                .child(name.clone())
        } else {
            let words = div()
                .id(("heading", ix))
                .role(Role::Heading)
                .aria_label(name.clone())
                .aria_level(2)
                .test_support()
                .rounded(theme::size::RADIUS_SM)
                .border_1()
                .border_color(ring(t, focused))
                .text_size(theme::size::XS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted_foreground)
                .child(name.clone());
            match folded {
                Some(_) => {
                    let strong = t.foreground;
                    words
                        .cursor_pointer()
                        .hover(move |s| s.text_color(strong))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_fold(title, cx)))
                }
                None => words,
            }
        };
        let arrow = folded.map(|folded| {
            let say = if folded {
                t!("Show {section}", section = name.clone())
            } else {
                t!("Fold {section}", section = name.clone())
            };
            let sign = if !folded {
                IconName::ChevronDown
            } else if crate::i18n::is_rtl() {
                IconName::ChevronLeft
            } else {
                IconName::ChevronRight
            };
            let (hover, pressed, strong) = (t.accent, t.pressed, t.foreground);
            div()
                .id(("fold", ix))
                .role(Role::Button)
                .aria_label(say.clone())
                .aria_expanded(!folded)
                .test_support()
                .p(space(0.5))
                .rounded(theme::size::RADIUS_SM)
                .cursor_pointer()
                .text_color(t.muted_foreground)
                // A folded section says so; an open one shows its arrow to the pointer, and while the keyboard is
                // on its title (Left folds it from there, Right opens it).
                .opacity(if folded || focused { 1. } else { 0. })
                .group_hover("heading", |s| s.opacity(1.))
                .hover(move |s| s.bg(hover).text_color(strong))
                .active(move |s| s.bg(pressed))
                .tooltip(tooltip_of(say, SharedString::default()))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_fold(title, cx)))
                .child(icon(sign).size_3())
        });
        row_start()
            .flex_none()
            .items_center()
            .gap(space(0.5))
            .child(text)
            .children(arrow)
            .into_any_element()
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

    /// The arrow by a place or a folder that opens it: softened while collapsed, clear on hover, open, or focus.
    fn arrow(
        &self,
        ix: usize,
        opens: rows::Opens,
        offset: Rems,
        focused: bool,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let name = if opens.open {
            IconName::ChevronDown
        } else if crate::i18n::is_rtl() {
            IconName::ChevronLeft
        } else {
            IconName::ChevronRight
        };
        let (hover, pressed) = (t.accent, t.pressed);
        let arrow_el = if opens.loading {
            Spinner::new().xsmall().into_any_element()
        } else {
            icon(name).size_3p5().into_any_element()
        };
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
                .opacity(if opens.open || opens.loading || focused {
                    1.
                } else {
                    0.38
                })
                .group_hover("row", |s| s.opacity(1.))
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(pressed))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(path) = this.rows.get(ix).and_then(Row::tree_path).map(str::to_string) {
                        this.toggle_folder(&path, None, cx);
                    }
                }))
                .child(arrow_el),
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
        // A drive whose size isn't known this moment (a refresh in between) keeps the bar's place as an empty
        // track, so the row doesn't shrink and grow back.
        let holds_place = p.capacity.is_none()
            && matches!(
                p.glyph,
                Glyph::Drive(crate::explorer::DriveKind::Fixed | crate::explorer::DriveKind::Removable)
            );
        let bar = p
            .capacity
            .and_then(|(total, free)| capacity_bar(total, free, space(1.), true, cx))
            .or_else(|| holds_place.then(|| div().h(space(1.)).w_full().rounded_full().bg(t.pressed)));
        // A drive says how much room it has left at the end of its line, where the rail is wide enough for its
        // name too (one that can be ejected keeps that end for its button).
        let free = p
            .capacity
            .filter(|_| self.roomy && p.eject.is_none() && matches!(p.glyph, Glyph::Drive(_)))
            .map(|(_, free)| {
                div()
                    .flex_none()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("{size} free", size = format_size(free)))
            });
        // A tile's name of two words or more ("Zuletzt verwendet", "مقاطع الفيديو") goes on to a second line rather
        // than lose its end: a tile is half the rail wide, and other languages' names run longer than English's.
        let name = div().flex_1().min_w_0().child(isolate(label.clone()));
        let name = if p.tile && label.trim().contains(' ') {
            name.text_ellipsis().line_clamp(2)
        } else {
            name.truncate()
        };
        let text = col()
            .flex_1()
            .min_w_0()
            .child(row_start().items_center().gap(space(2.)).child(name).children(free))
            .children(bar.map(|b| div().mt(space(0.75)).child(b)));
        let tip = tooltip_of(label.clone(), p.detail.clone());
        let (active, accent) = (p.active, t.accent);
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
        let (here, pressed, foreground) = (t.current, t.pressed, t.foreground);
        let button = button
            .test_support()
            .relative()
            .w_full()
            // A tile is half the rail wide: its picture and its name sit closer, and it keeps no room for an arrow.
            .gap(space(if p.tile { 1.5 } else { 2.5 }))
            .px(space(if p.tile { 1. } else { 2. }))
            .py(space(ROW_PAD))
            .when(p.gutter && !p.tile, |d| d.ps(6.))
            .rounded(theme::size::RADIUS_ROW)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_color(t.foreground.opacity(0.9))
            .hover(move |s| s.bg(accent))
            .active(move |s| s.bg(pressed))
            // The folder shown: a quiet fill and its name in full strength. Nothing else marks it.
            .when(active, move |d| {
                d.bg(here).text_color(foreground).font_weight(FontWeight::MEDIUM)
            })
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
            .child(self.glyph_art(&p.glyph, theme::rem_size(cx) * 1.125, t, cx))
            .child(text);
        let button = self.hop(button, ix, label.clone(), cx);

        let mut cell = div().group("row").relative().w_full().pb(space(0.25)).child(button);
        if let Some(opens) = p.tree {
            cell = cell.child(self.arrow(ix, opens, space(1.), focused, t, cx));
        }
        if let (Some(name), Some((handle, on))) = (&p.eject, eject) {
            let (path, label) = (p.path.clone(), name.clone());
            let (hover, eject_pressed) = (t.accent, t.pressed);
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
                            .active(move |s| s.bg(eject_pressed))
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
        let (active, accent, pressed, foreground) = (f.active, t.accent, t.pressed, t.foreground);
        let here = t.current;
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
            .rounded(theme::size::RADIUS_ROW)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_size(TREE_TEXT)
            .text_color(t.foreground.opacity(0.85))
            .hover(move |s| s.bg(accent))
            .active(move |s| s.bg(pressed))
            // The folder shown: a quiet fill and its name in full strength. Nothing else marks it.
            .when(active, move |d| {
                d.bg(here).text_color(foreground).font_weight(FontWeight::MEDIUM)
            })
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
            .child(crate::art::folder(theme::rem_size(cx), self.folder_color(t), None, cx))
            .child(div().flex_1().min_w_0().truncate().child(isolate(f.name.clone())));
        let button = self.hop(button, ix, f.name.clone(), cx);

        let mut cell = div().group("row").relative().w_full().pb(space(0.25)).child(button);
        if f.has != Some(false) {
            let opens = rows::Opens {
                open: f.open,
                loading: f.loading,
            };
            cell = cell.child(self.arrow(ix, opens, space(indent - 4.5), focused, t, cx));
        }
        cell.into_any_element()
    }

    /// "Can't look inside": a folder that couldn't be read, with the reason in its tooltip.
    fn draw_note(&self, ix: usize, depth: usize, reason: &str, t: &Tokens) -> AnyElement {
        let reason = SharedString::from(reason.to_string());
        div()
            .id(("note", ix))
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
        let (accent, pressed) = (t.accent, t.pressed);
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
            .py(space(ROW_PAD))
            .rounded(theme::size::RADIUS_ROW)
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .text_color(t.foreground.opacity(0.9))
            .hover(move |s| s.bg(accent))
            .active(move |s| s.bg(pressed))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .tooltip(tip)
            .child(
                icon(IconName::Search)
                    .size_4()
                    .flex_none()
                    .text_color(t.muted_foreground),
            )
            // One line: what is searched for, then where, which gives way first in a narrow rail.
            .child(div().flex_1().min_w_0().truncate().child(isolate(query.clone())))
            .child(
                div()
                    .flex_shrink(1.)
                    .min_w_0()
                    .max_w(relative(0.4))
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(isolate(where_)),
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

        let hover = t.accent;
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
                    .active(move |s| s.bg(pressed))
                    .tooltip(tooltip_of(t!("Remove from the sidebar"), SharedString::default()))
                    .on_click(cx.listener(move |this, _, _, cx| this.remove_search(ix, cx)))
                    .child(icon(IconName::X).size_3p5()),
            );
        let button = self.hop(button, ix, query.clone(), cx);
        div()
            .group("row")
            .relative()
            .w_full()
            .pb(space(0.25))
            .child(button)
            .child(remove)
            .into_any_element()
    }

    /// A label: its dot, in the line of them after their title. Its name and how many items have it are its tooltip
    /// and what a screen reader says; a press finds what has it, on every drive.
    fn draw_label(&self, ix: usize, label: &LabelRow, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let named = match label.count {
            Some(n) => t!("Label {name}, {n}", name = label.name.clone(), n = n),
            None => t!("Label {name}", name = label.name.clone()),
        };
        let title = t!("What's labeled “{label}”", label = label.name.clone());
        let how_many: SharedString = match label.count {
            Some(n) => plural(usize::try_from(n).unwrap_or(usize::MAX), "item").into(),
            None => SharedString::default(),
        };
        let (accent, pressed) = (t.accent, t.pressed);
        let dot = div()
            .id(("label", ix))
            .role(Role::Button)
            .aria_label(named)
            .aria_description(title)
            .test_support()
            .flex_none()
            // Seven of them and their title fit the rail's narrowest line.
            .size(space(5.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_1()
            .border_color(ring(t, focused))
            .cursor_pointer()
            .hover(move |s| s.bg(accent))
            .active(move |s| s.bg(pressed))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .tooltip(tooltip_of(label.name.clone().into(), how_many))
            .child(div().size(space(3.)).rounded_full().bg(theme::label_tone(label.color)));
        self.hop(dot, ix, label.name.clone(), cx).into_any_element()
    }

    /// A tag: a chip with its color's dot, its name, and how many items have it where instant search says. A click
    /// finds what has it, on every drive.
    fn draw_tag(&self, ix: usize, tag: &rows::Tag, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let name = SharedString::from(tag.name.clone());
        let tagged = t!("What's tagged “{tag}”", tag = tag.name.clone());
        let title: SharedString = match tag.count {
            Some(count) => format!("{tagged} ({count})").into(),
            None => tagged,
        };
        let (accent, pressed) = (t.accent, t.pressed);
        let chip = row_start()
            .id(("tag", ix))
            .role(Role::Button)
            .aria_label(name.clone())
            .aria_description(title.clone())
            .test_support()
            .flex_none()
            .max_w_full()
            .items_center()
            .gap(space(1.5))
            .px(space(2.))
            .py(space(0.5))
            .rounded_full()
            .border_1()
            .border_color(if focused { t.ring } else { t.stroke })
            .cursor_pointer()
            .text_size(theme::size::XS)
            .text_color(t.foreground.opacity(0.9))
            .hover(move |s| s.bg(accent))
            .active(move |s| s.bg(pressed))
            .on_click(cx.listener(move |this, _, window, cx| this.click(ix, window, cx)))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    this.right_click(ix, event.position, window, cx)
                }),
            )
            .tooltip(tooltip_of(title, SharedString::default()))
            .child(div().size(space(2.)).flex_none().rounded_full().bg(tags::tag_color(&tag.name)))
            .child(div().min_w_0().truncate().child(isolate(name)))
            .children(tag.count.map(|count| {
                div()
                    .flex_none()
                    .text_color(t.muted_foreground)
                    .child(SharedString::from(count.to_string()))
            }));
        self.hop(chip, ix, tag.name.clone(), cx).into_any_element()
    }

    fn draw_add(&self, ix: usize, t: &Tokens, focused: bool, cx: &mut Context<Self>) -> AnyElement {
        let (accent, pressed) = (t.accent, t.pressed);
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
                    .py(space(ROW_PAD))
                    .rounded(theme::size::RADIUS_ROW)
                    .border_1()
                    .border_color(ring(t, focused))
                    .cursor_pointer()
                    .text_color(t.muted_foreground)
                    .hover(move |s| s.bg(accent))
                    .active(move |s| s.bg(pressed))
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
pub(super) fn ring(t: &Tokens, focused: bool) -> Hsla {
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
    crate::components::tab_strip::menu::tip(text)
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

/// *Rename everywhere…*: a small dialog over the items that have the tag (`None` while they are being found).
struct RenameTag {
    tag: String,
    paths: Option<Vec<String>>,
    input: Entity<InputState>,
    said: Option<SharedString>,
    _enter: Subscription,
}

/// Why `new` can't be the name of `old`'s replacement: it is empty, the same (in any case: tags are compared so) or has
/// a line break. `new` is the box as it is, before trimming.
fn refused(old: &str, new: &str) -> Option<SharedString> {
    let trimmed = new.trim();
    (new.contains(['\n', '\r']) || trimmed.is_empty() || trimmed.to_lowercase() == old.to_lowercase())
        .then(|| t!("Give the tag another name"))
}

impl RenameTag {
    fn new(tag: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(tag.clone()));
        // Enter in the box saves, as the button does.
        let enter = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.save(window, cx);
            }
        });
        Self {
            tag,
            paths: None,
            input,
            said: None,
            _enter: enter,
        }
    }

    /// The items that have the tag are known: Rename can act.
    fn found(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.paths = Some(paths);
        cx.notify();
    }

    /// Renames: one change over every item, so one Undo step. A name that is refused is said in the dialog.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(paths) = self.paths.clone() else {
            return;
        };
        let typed = self.input.read(cx).value().to_string();
        if let Some(why) = refused(&self.tag, &typed) {
            self.said = Some(why);
            cx.notify();
            return;
        }
        window.close_dialog(cx);
        tags::set_tags(window, cx, paths, vec![typed.trim().to_string()], vec![self.tag.clone()]);
    }
}

impl Render for RenameTag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let title = t!("Rename the tag “{tag}”", tag = isolate(self.tag.clone()));
        let known = self.paths.is_some();
        crate::dialogs::main_action(cx, known, |this, window, cx| this.save(window, cx));
        let finding = (!known).then(|| t!("Finding what has “{tag}”…", tag = isolate(self.tag.clone())));
        col()
            .id("rename-tag")
            .role(Role::Group)
            .aria_label(title)
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(Input::new(&self.input).aria_label(t!("New name")))
            .children(self.said.clone().map(|said| {
                div()
                    .id("rename-tag-said")
                    .role(Role::Alert)
                    .aria_label(said.clone())
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(t.destructive)
                    .child(said)
            }))
            .children(finding.map(|finding| {
                div()
                    .id("rename-tag-finding")
                    .role(Role::Status)
                    .aria_label(finding.clone())
                    .test_support()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(finding)
            }))
            .child(
                row_start()
                    .justify_end()
                    .gap(space(2.))
                    .child(
                        Button::new("rename-tag-cancel")
                            .ghost()
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("rename-tag-ok")
                            .primary()
                            .label(t!("Rename"))
                            .disabled(!known)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

/// Opens the rename dialog for `tag`; it waits for the items (`RenameTag::found`) to act.
fn open_rename_tag(tag: String, window: &mut Window, cx: &mut App) -> Entity<RenameTag> {
    let title = t!("Rename the tag “{tag}”", tag = isolate(tag.clone()));
    let view = cx.new(|cx| RenameTag::new(tag, window, cx));
    let input = view.read(cx).input.clone();
    crate::dialogs::show(window, cx, title, px(420.), view.clone());
    input.update(cx, |input, cx| input.focus(window, cx));
    view
}

/// *Take off every file…*: asks, with the number, before taking the tag off the items that have it (`None` while
/// they are being found); one change, one Undo step.
struct TakeOffTag {
    tag: String,
    paths: Option<Vec<String>>,
}

impl TakeOffTag {
    /// The items that have the tag are known: the number can be said and the button acts.
    fn found(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.paths = Some(paths);
        cx.notify();
    }

    fn take_off(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(paths) = self.paths.clone() else {
            return;
        };
        window.close_dialog(cx);
        tags::set_tags(window, cx, paths, Vec::new(), vec![self.tag.clone()]);
    }
}

impl Render for TakeOffTag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = isolate(self.tag.clone());
        let title = t!("Take off “{tag}”", tag = shown.clone());
        let message = match &self.paths {
            Some(paths) => t!("Take “{tag}” off {n}?", tag = shown, n = plural(paths.len(), "item")),
            None => t!("Finding what has “{tag}”…", tag = shown),
        };
        let known = self.paths.is_some();
        crate::dialogs::main_action(cx, known, |this, window, cx| this.take_off(window, cx));
        col()
            .id("take-off-tag")
            .role(Role::Group)
            .aria_label(title)
            .aria_description(message.clone())
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .child(
                div()
                    .id("take-off-tag-said")
                    .role(Role::Status)
                    .aria_label(message.clone())
                    .test_support()
                    .child(message),
            )
            .child(
                row_start()
                    .justify_end()
                    .gap(space(2.))
                    .child(
                        Button::new("take-off-tag-cancel")
                            .ghost()
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("take-off-tag-ok")
                            .primary()
                            .label(t!("Take off"))
                            .disabled(!known)
                            .on_click(cx.listener(|this, _, window, cx| this.take_off(window, cx))),
                    ),
            )
    }
}

/// Opens the take-off question for `tag`; it waits for the items (`TakeOffTag::found`) to say the number.
fn open_take_off_tag(tag: String, window: &mut Window, cx: &mut App) -> Entity<TakeOffTag> {
    let title = t!("Take off “{tag}”", tag = isolate(tag.clone()));
    let view = cx.new(|_| TakeOffTag { tag, paths: None });
    crate::dialogs::show(window, cx, title, px(440.), view.clone());
    view
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
        // More rows below the last in view: a delicate hairline over translucent Mica/aurora, or a soft gradient
        // fade on opaque chrome.
        self.fade_below = self.more_below();
        // Before the first layout the list has no width: a drive's line starts with its free space said.
        let wide = self.list.viewport_bounds().size.width;
        self.roomy = wide <= px(0.) || wide >= rem * FREE_SHOWN_FROM;
        let fade = self.fade_below.then(|| {
            if theme::backdrop_on(cx) {
                div()
                    .id("places-fade")
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(px(1.))
                    .bg(t.stroke)
                    .into_any_element()
            } else {
                let layer = div()
                    .id("places-fade")
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .h(space(8.))
                    .bg(gpui_kit::linear_gradient(
                        180.,
                        gpui_kit::linear_color_stop(t.chrome.opacity(0.), 0.),
                        gpui_kit::linear_color_stop(t.chrome, 1.),
                    ));
                if cx.reduce_motion() {
                    layer.into_any_element()
                } else {
                    layer
                        .with_animation("places-fade-in", Animation::new(theme::motion::FAST), |layer, delta| {
                            layer.opacity(delta)
                        })
                        .into_any_element()
                }
            }
        });
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
            .child(self.draw_workspaces(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        list(
                            self.list.clone(),
                            cx.processor(|this, ix, window, cx| this.draw_line(ix, window, cx)),
                        )
                        .size_full(),
                    )
                    .children(fade),
            )
            .children(
                self.portal_menu
                    .as_ref()
                    .map(crate::components::tab_strip::menu::Popup::element),
            )
            .children(
                self.workspace_menu
                    .as_ref()
                    .map(crate::components::tab_strip::menu::Popup::element),
            )
            .children(self.tag_menu.as_ref().map(tab_menu::Popup::element))
    }
}

#[cfg(test)]
mod tests;
