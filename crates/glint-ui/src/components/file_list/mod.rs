//! The details view: columns, sorting, groups, selection, keyboard, type-ahead (FileList.tsx, VirtualList.tsx,
//! ResizeHandle.tsx, and the list's keys and presses in Explorer.tsx).
//!
//! # What this is
//!
//! [`FileList`] is a GPUI entity that shows one pane's tab as a list of rows with a header of columns. It
//! keeps no copy of what the explorer holds: the rows, the selection, the focus, the sort, the columns and the
//! rename are read from [`crate::explorer`] every frame (`Rows` and `Selection` clone cheaply), and every
//! change goes through an `Explorer` method. What it does keep is view state: the scroll, a column being
//! dragged, the rename box, what was typed for type-ahead, the width the list was laid out for.
//!
//! # How it is built
//!
//! - **Virtualised with `uniform_list`**, not the kit's `DataTable`. A row here is not a table row: it has
//!   group headers between rows, a rename box in the name cell, columns that come and go with the list's width
//!   (index.css's container queries), a size bar, and the explorer's own selection and focus rather than a
//!   table's. `uniform_list` draws only the lines on screen, measures one, and scrolls a million lines as
//!   cheaply as ten, with the kit's scroll bar on its handle. Everything that could be computed once per set of
//!   rows (the biggest size, the focused row's position) is computed when the rows change.
//! - **Accessibility** as the web's: a `ListBox` named "Files" (or "Search results", or "Recycle Bin") whose
//!   `ListBoxOption` rows have the file's name, its kind, size and date as description, their selected state,
//!   their place in the set, and the focused one reports itself as the active descendant. The header's
//!   buttons sort and are named; each column edge is a separator named "<column> column width".
//! - **Right to left**: rows, header and cells are `row_start()`; widths and edges use start and end.
//!
//! # Events
//!
//! The view emits [`FileListEvent`] for what belongs to another part (renaming, menus, dialogs); the
//! coordinator wires them. See each variant. Dragging a selection that is in an archive out of the window
//! emits [`DragFromArchive`].
//!
//! # What the pointer does
//!
//! `pointer.rs`: a press that moves 6 px is a drag out of the window (`drag_out`); a press on the empty part is
//! a marquee that selects the rows it crosses. `incoming.rs`: files dropped onto the list. `thumbs.rs`: pictures
//! in the icon cell.

mod cells;
pub mod diff;
#[cfg(test)]
pub(crate) mod fake_git;
mod header;
mod incoming;
mod layout;
mod pointer;
mod rename;
mod states;
#[cfg(test)]
mod tests;
mod thumbs;
mod typeahead;

pub use cells::{drop_path, lock_reason, size_of, ROW_HEIGHT};
pub use layout::relative_to;
pub use rename::{selection_end, INVALID_NAME_CHARS};

use crate::actions;
use crate::components::file_grid::thumbnail::{self, Thumbnails};
use crate::components::git;
use crate::components::row_facts::{self, Media, RowFacts, Want};
use crate::explorer::{
    self, Comparison, Explorer, ExplorerEvent, FindRun, LoadState, NavOptions, NoAccess, Rows, Selection, TabId,
    ViewMode,
};
use crate::kit::prelude::*;
use crate::theme::{self, Tokens};
use crate::{state, t};
use glint_model::columns::{ColumnId, FolderKind, GridTrack};
use glint_model::entries::{is_archive_file, is_recycled, FileEntry, GitMarks, Sort};
use glint_model::grid::grid_move;
use glint_model::groups::GroupBy;
use glint_model::paths::{is_gallery, is_recent, same_path, THIS_PC};
use glint_model::store::Column;
use incoming::DropAt;
use pointer::{layout_rows, Marquee};
use rename::RenameBox;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use typeahead::TypeAhead;

/// The icon of a kind of file (`kit::kind_icon`), for the cells.
use crate::kit::kind_icon;

/// How long the arrow keys resting on a folder wait before reading it ahead (`setTimeout(…, 120)`).
const PREFETCH_AFTER: Duration = Duration::from_millis(120);
/// More files than this asked to open at once need the person's say-so (`files.length > 10`).
const OPEN_MANY_ASKS_ABOVE: usize = 10;
/// Selections up to this many items have their tags read (the Tags submenu's checks need them, and the rows may be
/// off screen); a bigger one reads only what is shown.
const MOST_SELECTION_READS: usize = 200;
/// The width assumed before the list has been laid out once: wide, so no column is left out of the first frame.
const FIRST_WIDTH: f32 = 1200.;

/// What the list tells the coordinator. Each is something another part of the app does.
#[derive(Clone, Debug, PartialEq)]
pub enum FileListEvent {
    /// Enter (or leaving the box) on the rename box: rename `path` to `name`. The explorer's rename part
    /// validates it, renames, clears `Tab::renaming` and selects the result (`renameEntry`). The list keeps
    /// showing the box until `Tab::renaming` is cleared.
    CommitRename { path: String, name: String },
    /// Escape in the rename box: clear `Tab::renaming` (`updateTab({ renaming: null })`).
    CancelRename,
    /// A right-click, or the menu key (Shift+F10). `target` is the item pressed (already selected, as the web
    /// did), or None on the empty part of the list (selection already cleared) and for the keyboard on a folder
    /// with nothing focused. `at` is where the menu goes, in window coordinates: the pointer's position, or for
    /// the keyboard the focused row's bottom-start corner (a little inside, 1.5 rem, as the web put it), else
    /// near the list's top. `at` is None only if the list has not been laid out yet. `shift` was held:
    /// Windows' own menu at once, as in File Explorer.
    ContextMenu {
        target: Option<String>,
        at: Option<Point<Pixels>>,
        shift: bool,
    },
    /// Enter on more than ten files: ask "Open N files?" and, if yes, `Explorer::open_entry` each.
    ConfirmOpenMany { paths: Vec<String> },
    /// The header's menu chose these columns for the folder: `setColumns` (the folder's own view if folders
    /// keep their own, else Settings' `listColumns`). The explorer has no method for it yet.
    SetColumns { columns: Vec<ColumnId> },
    /// The header's menu: this folder is "like other folders again" (`setFolderView(path, null)`).
    ResetFolderView,
    /// "Open in File Explorer" on a folder Windows keeps closed.
    OpenInFileExplorer { path: String },
    /// "Properties" on a folder Windows keeps closed.
    ShowProperties { path: String },
    /// "Search subfolders too" under a filter that matched nothing: `runFind(query)`.
    SearchSubfolders { query: String },
}

impl EventEmitter<FileListEvent> for FileList {}

/// The selection was dragged out of the window, and it is in an archive: extract it first (`dragFromArchive(paths)`,
/// the archive part's), then drag the copies out with `platform::dragdrop::drag_out(.., copy_only: true)`. Items on
/// disk are dragged out by the list itself.
///
/// A type of its own, not a [`FileListEvent`] variant, so the workspace's exhaustive `match` on that enum keeps
/// compiling until it subscribes to this one: `cx.subscribe(&list, |.., event: &DragFromArchive, ..| ..)`. (It can
/// be folded into the enum as `FileListEvent::DragFromArchive { paths }` with one new arm.)
#[derive(Clone, Debug, PartialEq)]
pub struct DragFromArchive {
    /// The items dragged, as the explorer names them (inside the archive), in the list's order.
    pub paths: Vec<String>,
}

impl EventEmitter<DragFromArchive> for FileList {}

/// A column being dragged by its edge.
struct Resize {
    name: Column,
    /// Where the pointer was when it started.
    from: Pixels,
    /// The column's width then, in pixels.
    start: f32,
    /// Its width now, once the pointer has moved.
    now: Option<f32>,
}

/// A press on a row of a selection of several: they stay selected until the button is released, so all of
/// them can be dragged; released without a drag, the row alone is selected (`press` in Explorer.tsx).
struct Press {
    narrow_to: Option<String>,
    /// Where the button went down, in window pixels: moving 6 px from here with it held is a drag.
    at: Point<Pixels>,
}

/// Where a track of the row is and how wide.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Track {
    Name,
    Where,
    Column { id: ColumnId, width: Pixels },
}

/// A search's run, as far as the list shows it.
pub(crate) struct FindInfo {
    pub query: String,
    pub scope: String,
    /// The names or the files' contents are still being searched.
    pub running: bool,
    pub error: String,
    pub skipped: Vec<String>,
    pub inside: InsideInfo,
}

/// What a search found inside files, as far as the list says it (the header of that part of the results).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct InsideInfo {
    /// The words looked for inside files ("" when the search looked at names alone).
    pub words: String,
    pub matches: usize,
    pub files: usize,
    pub running: bool,
    pub limited: bool,
    pub error: String,
}

/// What the list reads from its pane's tab each frame. Everything is a cheap clone (a few `Arc`s and short
/// strings), never the entries.
pub(crate) struct TabView {
    pub id: TabId,
    /// What the rows' tags and media facts are kept under: the tab's folder or search (`Tab::view_key`).
    pub facts_key: String,
    /// Where this list's scroll is kept: the folder or search, in the list (`placeKey`).
    pub place: String,
    pub path: String,
    pub rows: Rows,
    pub selection: Selection,
    pub columns: Vec<ColumnId>,
    pub sort: Sort,
    pub group: GroupBy,
    pub kind: FolderKind,
    pub load: LoadState,
    pub loading: bool,
    pub renaming: Option<String>,
    pub filter: String,
    pub find: Option<FindInfo>,
    /// Entries in the listing, before hidden ones and the filter take any out.
    pub entries: usize,
    pub has_listing: bool,
    pub archive_root: bool,
    pub in_archive: bool,
    pub no_access: Option<NoAccess>,
    /// Items from many folders: each says where it is.
    pub located: bool,
    pub recycled: bool,
    pub gallery: bool,
    pub recent: bool,
    /// The folder searched (the folder shown, for a plain listing).
    pub scope: String,
    /// Dual-pane view, and the keyboard is in the other pane.
    pub inactive: bool,
    pub show_hidden: bool,
    /// The folder has a view of its own (columns, sort) that "like other folders again" gives up.
    pub own_view: bool,
    /// The folder's subfolders measured by "Folder sizes" (none for search results), and whether that is still
    /// running.
    pub sizes: Option<Arc<HashMap<String, u64>>>,
    pub measuring: bool,
    /// What Git says of the folder's items (none outside a repository, and for search results).
    pub git: Option<Arc<GitMarks>>,
    /// How the items differ from the other pane's, while the panes are compared (none otherwise).
    pub diffs: Option<Arc<Comparison>>,
    /// "Diff only" is on: the identical items are left out.
    pub diff_only: bool,
}

/// Says where an item is, in a list of items from many folders.
pub(crate) type WhereOf = Box<dyn Fn(&str) -> String>;

/// What `uniform_list` draws a frame from, kept in an `Rc` because its callback outlives `render`.
pub(crate) struct Frame {
    pub rows: Rows,
    pub selected: Selection,
    pub focus_path: Option<String>,
    pub renaming: Option<String>,
    pub tracks: Vec<Track>,
    /// What the rows' tags and media facts are kept under (`TabView::facts_key`).
    pub facts_key: String,
    /// What the shown columns need read of the media files (none unless one of their columns is shown).
    pub media: Media,
    pub inactive: bool,
    pub located: bool,
    pub recycled: bool,
    pub where_of: Option<WhereOf>,
    pub max_size: u64,
    /// The open folder's subfolders measured by "Folder sizes", by name (`sizes`); none for search results.
    pub sizes: Option<Arc<HashMap<String, u64>>>,
    /// The folder's sizes are being measured now: a folder with none yet says "…".
    pub measuring: bool,
    /// What Git says of the folder's items (lowercase name to mark): a pill after the name, ignored ones dimmed.
    pub git: Option<Arc<GitMarks>>,
    /// How the items differ from the other pane's (compare panes): a pill after the name, the same ones dimmed.
    pub diffs: Option<Arc<Comparison>>,
    pub flashed: Arc<HashSet<String>>,
    pub now: i64,
    /// Items cut to the clipboard: dimmed until pasted (`cut`).
    pub cut: Arc<HashSet<String>>,
    /// Items being dragged out of the window: dimmer still (`dragging`).
    pub dragging: Arc<HashSet<String>>,
    /// The folder row files are dragged over: lit (`dropTarget`).
    pub drop_row: Option<String>,
    /// How many lines the list has (rows, a header per group, and the lines under rows).
    pub lines: usize,
    /// The tab shown, and what its search found inside files (the header of that part says it).
    pub tab: TabId,
    pub inside: InsideInfo,
    /// Small pictures in the icon cell (Settings' "list thumbnails"), the tab they are asked for, the square
    /// they fit in (CSS pixels: one rem) and the window's device pixels per CSS pixel.
    pub thumbnails: bool,
    pub lane: String,
    pub box_px: f32,
    pub scale: f32,
}

/// What was last asked of the facts, so a frame that draws the same lines asks nothing again.
struct Asked {
    key: String,
    from: usize,
    to: usize,
    rows: usize,
    want: Want,
}

/// What was worked out when the rows (or the tab) last changed.
#[derive(Default)]
struct Shown {
    tab: Option<TabId>,
    place: String,
    max_size: u64,
    /// The focused row's position, found when the focus or the rows change (not each frame).
    focus: Option<(String, Option<usize>)>,
    /// The row last scrolled into view, so the list scrolls when the focus moves and not otherwise.
    scrolled_to: Option<usize>,
}

/// The details view of one pane. Create it with [`FileList::new`].
pub struct FileList {
    pane: usize,
    explorer: Entity<Explorer>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    shown: Shown,
    /// The rows (or the tab) changed since the derived values were worked out.
    dirty: bool,
    frame: Rc<Frame>,
    /// The width the list was last laid out at, measured by the canvas under it.
    width: Rc<Cell<f32>>,
    /// The folder or search whose rows were drawn last (`TabView::facts_key`), and how many times that has
    /// changed: each new one's rows arrive with a short rise (see `render`).
    entered_for: Option<String>,
    entered: usize,
    resizing: Option<Resize>,
    rename: Option<RenameBox>,
    typeahead: TypeAhead,
    press: Option<Press>,
    /// Pixels scrolled when last saved to the explorer.
    saved_top: f32,
    /// The scroll was just restored: the focused row is not scrolled to (`restoring`).
    restoring: bool,
    prefetch: Option<Task<()>>,
    /// This list, for the listeners the window calls back (a drag that leaves the list).
    this: WeakEntity<Self>,
    /// The thumbnail service: the pictures in the icon cell.
    thumbs: Entity<Thumbnails>,
    /// This list's id as the thumbnail service knows its viewer.
    viewer: EntityId,
    /// A fingerprint of the thumbnails last handed to the service (see `thumbs.rs`).
    thumbs_shown: Rc<Cell<u64>>,
    /// The tags and media facts of the rows (`row_facts`).
    facts: Entity<RowFacts>,
    /// The lines the facts were last asked for; cleared when the rows or the facts change.
    asked: Option<Asked>,
    /// Where the list is on screen (the area under the header), measured by the canvas under it.
    area: Rc<Cell<Bounds<Pixels>>>,
    /// A press or a marquee is under way: the window-level mouse listeners act.
    tracking: Rc<Cell<bool>>,
    /// A rectangle being dragged over the empty part.
    marquee: Option<Marquee>,
    /// The items being dragged out of the window now.
    dragging: Arc<HashSet<String>>,
    /// Where a drop of the files being dragged over the list would land.
    drop_at: Option<DropAt>,
    /// Tests only: an item shown as being renamed, as the explorer's rename part will make `Tab::renaming` say.
    #[cfg(test)]
    pretend_renaming: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl FileList {
    /// A details view of the tab in pane `pane` (0, or 1 in dual-pane view). It follows the pane's tab as it
    /// changes. The coordinator places the entity in the workspace, keyed by pane, and shows it while the
    /// tab's view is the list (`ViewMode::List`, or `Columns` between its two side columns).
    pub fn new(pane: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let focus = cx.focus_handle();
        let thumbs = thumbnail::thumbnails(cx);
        let viewer = cx.entity_id();
        let facts = row_facts::shared(cx);
        let mut subscriptions = vec![
            // Anything the explorer changes redraws; the kinds of change that make the rows different also say so.
            cx.observe(&explorer, |_, _, cx| cx.notify()),
            cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| {
                if matches!(
                    event,
                    ExplorerEvent::RowsChanged(_)
                        | ExplorerEvent::ListingChanged { .. }
                        | ExplorerEvent::TabsChanged
                        | ExplorerEvent::ViewChanged(_)
                        | ExplorerEvent::SizesChanged { .. }
                ) {
                    this.dirty = true;
                    this.asked = None;
                }
                // The selection's tags are read for the Tags submenu, whether or not its rows are on screen.
                if let ExplorerEvent::SelectionChanged(tab) = event {
                    this.ask_selection_tags(*tab, cx);
                }
                // Items were cut to the clipboard, or pasted: they are dimmed, or not.
                if matches!(event, ExplorerEvent::CutChanged) {
                    cx.notify();
                }
            }),
            // A picture arrived, or one was let go of.
            cx.observe(&thumbs, |_, _, cx| cx.notify()),
            // Tags or media facts were read (or must be read again): the dots and cells are drawn, and asked for.
            cx.observe(&facts, |this, _, cx| {
                this.asked = None;
                cx.notify();
            }),
            // Column widths and the text size are settings.
            cx.observe_global::<state::UiState>(|_, cx| cx.notify()),
            // A click or a key in this list makes its pane the one the toolbar and the keys act on.
            cx.on_focus_in(&focus, window, |this, _, cx| {
                let pane = this.pane;
                this.explorer.update(cx, |explorer, cx| explorer.focus_pane(pane, cx));
            }),
        ];
        // Git's marks arrived or changed: the pills after the names, and the faded ignored items, are drawn again.
        if let Some(model) = git::try_shared(cx) {
            subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
        }
        // The thumbnails shown are let go of when the list goes.
        let service = thumbs.clone();
        subscriptions.push(cx.on_release(move |_, cx| {
            service.update(cx, |thumbs, cx| thumbs.release(viewer, cx));
        }));
        Self {
            pane,
            explorer,
            focus,
            scroll: UniformListScrollHandle::new(),
            shown: Shown::default(),
            dirty: true,
            frame: Rc::new(Frame {
                rows: Rows::empty(),
                selected: Selection::default(),
                focus_path: None,
                renaming: None,
                tracks: Vec::new(),
                facts_key: String::new(),
                media: Media::None,
                inactive: false,
                located: false,
                recycled: false,
                where_of: None,
                max_size: 0,
                sizes: None,
                measuring: false,
                git: None,
                diffs: None,
                flashed: Arc::new(HashSet::new()),
                now: 0,
                cut: Arc::new(HashSet::new()),
                dragging: Arc::new(HashSet::new()),
                drop_row: None,
                lines: 0,
                tab: TabId(0),
                inside: InsideInfo::default(),
                thumbnails: false,
                lane: String::new(),
                box_px: 16.,
                scale: 1.,
            }),
            width: Rc::new(Cell::new(0.)),
            entered_for: None,
            entered: 0,
            resizing: None,
            rename: None,
            typeahead: TypeAhead::default(),
            press: None,
            saved_top: 0.,
            restoring: false,
            prefetch: None,
            this: cx.weak_entity(),
            thumbs,
            viewer,
            thumbs_shown: Rc::new(Cell::new(0)),
            facts,
            asked: None,
            area: Rc::new(Cell::new(Bounds::default())),
            tracking: Rc::new(Cell::new(false)),
            marquee: None,
            dragging: Arc::new(HashSet::new()),
            drop_at: None,
            #[cfg(test)]
            pretend_renaming: None,
            _subscriptions: subscriptions,
        }
    }

    /// Which pane this list shows.
    pub fn pane(&self) -> usize {
        self.pane
    }

    /// Gives the list the keyboard (F6 to this pane, a new folder opening).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    /// A press that may turn into a drag was started on a row; a drag from the list calls this so the release
    /// that ends the drag doesn't also narrow the selection.
    pub fn cancel_press(&mut self) {
        self.press = None;
    }

    // ---- Reading the explorer ---------------------------------------------------------------------------

    /// The pane's tab as the list shows it, or None when there is no such pane.
    fn read_tab(&self, cx: &App) -> Option<TabView> {
        let explorer = self.explorer.read(cx);
        let tab = explorer.pane_tab(self.pane)?;
        let find = tab.find.as_ref().map(|run: &FindRun| FindInfo {
            query: run.query.clone(),
            scope: run.scope.clone(),
            running: run.running || run.inside.running,
            error: run.error.clone(),
            skipped: run.skipped.clone(),
            inside: InsideInfo {
                words: if run.inside.running || !run.inside.hits.is_empty() || run.inside.elapsed > 0. {
                    run.inside.words.clone()
                } else {
                    String::new()
                },
                matches: run.inside.matches,
                files: run.inside.files(),
                running: run.inside.running,
                limited: run.inside.limited,
                error: run.inside.error.clone(),
            },
        });
        let plain = find.is_none();
        let recycled = tab.is_recycle_bin() && plain;
        let gallery = is_gallery(&tab.path) && plain;
        let recent = is_recent(&tab.path) && plain;
        let search = !plain || recent || gallery;
        let scope = find.as_ref().map_or_else(|| tab.path.clone(), |run| run.scope.clone());
        let renaming = tab.renaming.clone();
        #[cfg(test)]
        let renaming = renaming.or_else(|| self.pretend_renaming.clone());
        let settings = state::settings(cx);
        // The sizes "Folder sizes" measured for this folder; a search's results come from many.
        let sizes = explorer.size_run(&tab.path).filter(|_| plain);
        let own_view = settings.per_folder_views
            && plain
            && tab.path != THIS_PC
            && explorer.memory().folder_view(&tab.path).is_some();
        Some(TabView {
            id: tab.id,
            facts_key: tab.view_key(),
            // The grid keeps its own place in the same folder, so the key says which view it is for.
            place: format!("{}:list", tab.view_key()),
            path: tab.path.clone(),
            rows: tab.rows.clone(),
            selection: tab.selection.clone(),
            columns: tab.columns.clone(),
            sort: tab.sort,
            group: tab.group,
            kind: tab.kind,
            load: tab.load.clone(),
            loading: tab.loading,
            renaming,
            filter: tab.filter.clone(),
            find,
            entries: tab.entries().len(),
            has_listing: tab.listing.is_some(),
            archive_root: tab.archive().is_some_and(|archive| same_path(&archive.path, &tab.path)),
            in_archive: tab.archive().is_some(),
            no_access: tab.no_access().copied(),
            located: search || recycled,
            recycled,
            gallery,
            recent,
            scope,
            inactive: explorer.is_dual() && explorer.active_pane() != self.pane,
            show_hidden: settings.show_hidden,
            own_view,
            sizes: sizes.map(|run| run.sizes.clone()),
            measuring: sizes.is_some_and(|run| run.running),
            git: marks_in(cx, tab.id, &tab.path).filter(|_| plain),
            diffs: explorer.comparison_of(tab.id).filter(|_| plain),
            diff_only: explorer.diff_only,
        })
    }

    /// What changed since the last frame, worked out once: the scroll place of another folder, the rename
    /// box, the biggest size, the focused row's position and the scroll that keeps it in view.
    fn sync(&mut self, view: &TabView, window: &mut Window, cx: &mut Context<Self>) {
        let moved = self.shown.tab != Some(view.id) || self.shown.place != view.place;
        if moved {
            // Another tab, folder or search: it has its own place in the list, and its own typing.
            let top = self.explorer.read(cx).saved_scroll(view.id, &view.place);
            self.scroll.0.borrow().base_handle.set_offset(point(px(0.), px(-top)));
            self.saved_top = top;
            self.restoring = top > 0.;
            self.typeahead.clear();
            self.press = None;
            self.end_rename(window, cx);
            self.shown.tab = Some(view.id);
            self.shown.place = view.place.clone();
            self.shown.focus = None;
            self.shown.scrolled_to = None;
            self.dirty = true;
            self.marquee = None;
            // The folder changed under this tab: what its thumbnails waited for is dropped, and the burst is
            // timed from here (`timeThumbnails`).
            self.thumbs_shown.set(0);
            if state::settings(cx).list_thumbnails {
                let (lane, place) = (view.id.to_string(), view.place.clone());
                self.thumbs.update(cx, |thumbs, cx| {
                    thumbs.forget_lane(&lane, cx);
                    thumbs.time(&lane, &place, cx);
                });
            }
        }
        if self.dirty {
            self.dirty = false;
            self.shown.max_size = cells::biggest(&view.rows, view.located, view.sizes.as_deref());
            self.shown.focus = None;
            if view.has_listing {
                // Timed from the click to the list on screen, and the first folder of the run for the bench
                // (`markShown`, `firstFolderPainted`): as soon as this frame is drawn.
                let (id, explorer) = (view.id, self.explorer.clone());
                cx.defer(move |cx| {
                    explorer.update(cx, |explorer, cx| {
                        explorer.mark_shown(id, cx);
                        explorer.first_folder_painted();
                    });
                });
            }
        }
        // The focused row's position, found again only when the focus (or the rows) changed.
        let focus_path = view.selection.focus();
        let cached = self.shown.focus.as_ref().map(|(path, _)| path.as_str());
        if cached != focus_path || (focus_path.is_none() && self.shown.focus.is_some()) {
            let index = focus_path.and_then(|path| view.rows.position_of(path));
            self.shown.focus = focus_path.map(|path| (path.to_string(), index));
        }
        let focus_index = self.shown.focus.as_ref().and_then(|(_, index)| *index);

        // Scrolls the focused row into view when it moves; on the frame the scroll was restored, it stays
        // where it was left (the web's `restoring`, which only the first effect after mounting honoured).
        let restoring = std::mem::take(&mut self.restoring);
        if self.shown.scrolled_to != focus_index {
            if !restoring {
                if let Some(index) = focus_index {
                    let line = layout_rows::line_of_row(&view.rows, index);
                    self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
                }
            }
            self.shown.scrolled_to = focus_index;
            // The arrow keys landing on a folder read it ahead: Enter is likely next.
            self.prefetch = focus_index
                .and_then(|index| view.rows.get(index))
                .filter(|entry| entry.dir)
                .map(|entry| entry.path.clone())
                .map(|path| {
                    cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(PREFETCH_AFTER).await;
                        this.update(cx, |this, cx| {
                            this.explorer
                                .update(cx, |explorer, cx| explorer.prefetch_folder(&path, cx));
                        })
                        .ok();
                    })
                });
        }

        // Where the list is scrolled is kept per tab and place, as the web did on every scroll event.
        let top = -f32::from(self.scroll.0.borrow().base_handle.offset().y);
        if (top - self.saved_top).abs() > 0.5 {
            self.saved_top = top;
            let (id, place) = (view.id, view.place.clone());
            self.explorer
                .update(cx, |explorer, _| explorer.save_scroll(id, &place, top));
        }

        // The rename box follows `Tab::renaming`.
        match (&view.renaming, &self.rename) {
            (Some(path), current) if current.as_ref().map(|r| r.path.as_str()) != Some(path.as_str()) => {
                if let Some(entry) = view.rows.iter().find(|e| &e.path == path) {
                    let (path, name, dir) = (entry.path.clone(), entry.name.clone(), entry.dir);
                    self.start_rename(&path, &name, dir, window, cx);
                }
            }
            (None, Some(_)) => self.end_rename(window, cx),
            _ => {}
        }
    }

    /// The tracks of a row for the width the list has now, with their widths in pixels.
    fn tracks(&self, view: &TabView, window: &Window, cx: &App) -> Vec<Track> {
        let rem = f32::from(window.rem_size());
        let width = self.width.get();
        let rems_wide = if width > 0. { width / rem } else { FIRST_WIDTH / rem };
        layout::tracks_for(&view.columns, view.located, rems_wide)
            .into_iter()
            .map(|track| match track {
                GridTrack::Name => Track::Name,
                GridTrack::Where => Track::Where,
                GridTrack::Column(id) => Track::Column {
                    id,
                    width: self.column_width(id.def().width, window, cx),
                },
            })
            .collect()
    }

    // ---- Keyboard ---------------------------------------------------------------------------------------

    /// The pane's rows and where the keyboard is in them.
    fn rows_and_focus(&self, cx: &App) -> Option<(Rows, Option<usize>)> {
        let tab = self.explorer.read(cx).pane_tab(self.pane)?;
        let at = tab.selection.focus().and_then(|path| tab.rows.position_of(path));
        Some((tab.rows.clone(), at))
    }

    /// Rows that fit on screen, less one: how far PageUp and PageDown go.
    fn page(&self, window: &Window) -> usize {
        let viewport = self
            .scroll
            .0
            .borrow()
            .last_item_size
            .map_or(rems(25.).to_pixels(window.rem_size()), |size| size.item.height);
        let row = ROW_HEIGHT.to_pixels(window.rem_size());
        ((f32::from(viewport) / f32::from(row)).floor() as usize)
            .saturating_sub(1)
            .max(1)
    }

    /// The keys that move through the list and choose in it. Everything with a binding in `FileList`'s
    /// context (Enter, Ctrl+A, F2…) is an action instead; Alt keys belong to the workspace.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The rename box has its keys.
        if self.rename.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.alt {
            return;
        }
        let ctrl = modifiers.control || modifiers.platform;
        let key = keystroke.key.as_str();
        // The menu key (Shift+F10 is an action of its own).
        if key == "menu" {
            self.keyboard_menu(window, cx);
            cx.stop_propagation();
            return;
        }
        let Some((rows, at)) = self.rows_and_focus(cx) else {
            return;
        };

        // Arrows, Home, End, PageUp, PageDown: with Shift they extend from the anchor; with Ctrl they only
        // move the cursor; alone they select the row they land on.
        let dom_key = match key {
            "up" => "ArrowUp",
            "down" => "ArrowDown",
            "left" => "ArrowLeft",
            "right" => "ArrowRight",
            "pageup" => "PageUp",
            "pagedown" => "PageDown",
            "home" => "Home",
            "end" => "End",
            _ => "",
        };
        if let Some(to) = grid_move(dom_key, at.map_or(-1, |i| i as i64), rows.len(), 1, self.page(window)) {
            self.go(&rows, to, modifiers.shift, ctrl, cx);
            cx.stop_propagation();
            return;
        }
        // The columns view: Left goes to the folder above (this one selected there), Right into the focused
        // folder. The keys are the arrows themselves, whichever way the interface reads.
        if matches!(key, "left" | "right") && !ctrl && !modifiers.shift {
            let in_columns = self
                .explorer
                .read(cx)
                .pane_tab(self.pane)
                .is_some_and(|tab| tab.view == ViewMode::Columns);
            if in_columns {
                if key == "left" {
                    self.explorer.update(cx, |explorer, cx| explorer.go_up(cx).detach());
                } else if let Some(entry) = at.and_then(|i| rows.get(i)).filter(|entry| entry.dir) {
                    let path = entry.path.clone();
                    self.explorer
                        .update(cx, |explorer, cx| explorer.navigate(path, NavOptions::default(), cx));
                }
                cx.stop_propagation();
                return;
            }
        }
        if ctrl {
            // Ctrl+Space adds the focused item to the selection, or takes it out.
            if key == "space" {
                if let Some(path) = at.and_then(|i| rows.get(i)).map(|entry| entry.path.clone()) {
                    self.explorer
                        .update(cx, |explorer, cx| explorer.toggle_selected(&path, cx));
                    cx.stop_propagation();
                }
            }
            return;
        }
        if key == "escape" {
            let (tab, searching, filtering, selected) = {
                let Some(tab) = self.explorer.read(cx).pane_tab(self.pane) else {
                    return;
                };
                (
                    tab.id,
                    tab.find.is_some(),
                    !tab.filter.is_empty(),
                    !tab.selection.is_empty(),
                )
            };
            if searching || filtering {
                // The filter goes, and a search's results with it (`clearFind`).
                self.explorer.update(cx, |explorer, cx| explorer.clear_find_in(tab, cx));
            } else if selected {
                self.explorer.update(cx, |explorer, cx| explorer.select_only(None, cx));
            } else {
                return;
            }
            cx.stop_propagation();
            return;
        }
        // Type-ahead: a printable key (not Space, which previews) jumps to the next name starting with what
        // was typed.
        let typed = keystroke
            .key_char
            .as_deref()
            .filter(|text| text.chars().count() == 1 && key != "space")
            .map(str::to_lowercase);
        if let Some(letter) = typed {
            let jump = self.typeahead.press(&letter, Instant::now(), at, rows.len(), |i| {
                rows.get(i).map(|e| e.lower.as_str())
            });
            if let Some(path) = jump.and_then(|i| rows.get(i)).map(|entry| entry.path.clone()) {
                self.explorer
                    .update(cx, |explorer, cx| explorer.select_only(Some(&path), cx));
            }
            cx.stop_propagation();
        }
    }

    /// The cursor goes to row `to` (`go`).
    fn go(&mut self, rows: &Rows, to: usize, shift: bool, ctrl: bool, cx: &mut Context<Self>) {
        let Some(path) = rows.get(to).map(|entry| entry.path.clone()) else {
            return;
        };
        self.explorer.update(cx, |explorer, cx| {
            if shift {
                explorer.select_range(rows, to, ctrl, cx);
            } else if ctrl {
                // Ctrl+arrows move the cursor without selecting (`updateTab({ focus })`).
                explorer.set_focus(Some(&path), None, cx);
            } else {
                explorer.select_only(Some(&path), cx);
            }
        });
    }

    /// Enter: opens what is selected (`openSelected`). Several files open one by one; more than ten ask first.
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.explorer.read(cx).pane_tab(self.pane) else {
            return;
        };
        let chosen: Vec<FileEntry> = tab
            .rows
            .iter()
            .filter(|entry| tab.selection.contains(&entry.path))
            .cloned()
            .collect();
        // Enter closes the preview that Space opened.
        let many = chosen.len() > 1;
        let files: Vec<FileEntry> = chosen.iter().filter(|e| !e.dir).cloned().collect();
        self.explorer.update(cx, |explorer, _| explorer.quick_look = false);
        if chosen.is_empty() {
            return;
        }
        if !many || files.is_empty() {
            let first = chosen[0].clone();
            self.explorer
                .update(cx, |explorer, cx| explorer.open_entry(&first, cx).detach());
        } else if files.len() > OPEN_MANY_ASKS_ABOVE {
            cx.emit(FileListEvent::ConfirmOpenMany {
                paths: files.into_iter().map(|e| e.path).collect(),
            });
        } else {
            self.explorer.update(cx, |explorer, cx| {
                for file in &files {
                    explorer.open_entry(file, cx).detach();
                }
            });
        }
    }

    /// Ctrl+Enter: each selected folder (and archive) in a new tab (`openSelectedInTabs`).
    fn open_in_tabs(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.explorer.read(cx).pane_tab(self.pane) else {
            return;
        };
        let paths: Vec<String> = tab
            .rows
            .iter()
            .filter(|e| tab.selection.contains(&e.path) && (e.dir || is_archive_file(e)) && !is_recycled(e))
            .map(|e| e.path.clone())
            .collect();
        self.explorer.update(cx, |explorer, cx| {
            for path in &paths {
                explorer.open_in_new_tab(path, cx);
            }
        });
    }

    /// Ctrl+A.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(rows) = self.explorer.read(cx).pane_tab(self.pane).map(|tab| tab.rows.clone()) else {
            return;
        };
        self.explorer.update(cx, |explorer, cx| explorer.select_all(&rows, cx));
    }

    /// The menu key and Shift+F10: the menu of what is selected, at the focused item, else the folder's
    /// (`keyboardMenu`). The coordinator puts it up where `at` says: the focused row's bottom-start corner,
    /// inset a little (1.5 rem, `left + 24`), else near the list's top (`top + 8`).
    fn keyboard_menu(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some((target, line)) = self.explorer.read(cx).pane_tab(self.pane).map(|tab| {
            let focused = tab
                .selection
                .focus()
                .and_then(|focus| tab.rows.position_of(focus))
                .and_then(|at| tab.rows.get(at).map(|entry| (entry.path.clone(), at)));
            match focused {
                Some((path, at)) => (Some(path), Some(layout_rows::line_of_row(&tab.rows, at))),
                None => (None, None),
            }
        }) else {
            return;
        };
        let area = self.area.get();
        let at = (area.size.width > px(0.)).then(|| {
            let inset = space(6.).to_pixels(window.rem_size());
            let x = if crate::i18n::is_rtl() {
                area.right() - inset
            } else {
                area.origin.x + inset
            };
            let y = match line {
                Some(line) => {
                    let row = ROW_HEIGHT.to_pixels(window.rem_size());
                    // The bottom of the row, kept inside the list.
                    let bottom = area.origin.y + row * (line as f32 + 1.) - px(self.scroll_top());
                    bottom.clamp(area.origin.y, area.bottom())
                }
                None => area.origin.y + space(2.).to_pixels(window.rem_size()),
            };
            point(x, y)
        });
        cx.emit(FileListEvent::ContextMenu {
            target,
            at,
            shift: false,
        });
    }

    // ---- Mouse ------------------------------------------------------------------------------------------

    /// This list takes the keyboard, and its pane becomes the one the keys and the toolbar act on.
    fn take_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus.focus(window, cx);
        let pane = self.pane;
        self.explorer.update(cx, |explorer, cx| explorer.focus_pane(pane, cx));
    }

    /// A button pressed on row `i` (`onRowMouseDown`): Ctrl toggles, Shift selects from the anchor, a plain
    /// press selects only it (but keeps a selection of several until release, so it can be dragged), a
    /// double-click opens, the right button selects it if it isn't and asks for its menu, the middle button
    /// opens a folder in a new tab.
    fn row_down(&mut self, i: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.take_focus(window, cx);
        let Some((rows, selection)) = self
            .explorer
            .read(cx)
            .pane_tab(self.pane)
            .map(|tab| (tab.rows.clone(), tab.selection.clone()))
        else {
            return;
        };
        let Some(entry) = rows.get(i).cloned() else { return };
        let additive = event.modifiers.control || event.modifiers.platform;
        match event.button {
            MouseButton::Middle => {
                if (entry.dir || is_archive_file(&entry)) && !is_recycled(&entry) {
                    self.explorer
                        .update(cx, |explorer, cx| explorer.open_in_new_tab(&entry.path, cx));
                }
            }
            MouseButton::Right => {
                if !selection.contains(&entry.path) {
                    self.explorer
                        .update(cx, |explorer, cx| explorer.select_only(Some(&entry.path), cx));
                }
                cx.emit(FileListEvent::ContextMenu {
                    target: Some(entry.path),
                    at: Some(event.position),
                    shift: event.modifiers.shift,
                });
            }
            MouseButton::Left => {
                // The first click of what may be a double-click reads a folder ahead.
                if entry.dir {
                    self.explorer
                        .update(cx, |explorer, cx| explorer.prefetch_folder(&entry.path, cx));
                }
                let keep =
                    !event.modifiers.shift && !additive && selection.contains(&entry.path) && selection.len() > 1;
                self.explorer.update(cx, |explorer, cx| {
                    if event.modifiers.shift {
                        explorer.select_range(&rows, i, additive, cx);
                    } else if additive {
                        explorer.toggle_selected(&entry.path, cx);
                    } else if keep {
                        // The pressed item takes the focus and the anchor while the selection stays, so all of it
                        // can be dragged; it narrows on release (`updateTab({ focus, anchor })`).
                        explorer.set_focus(Some(&entry.path), Some(&entry.path), cx);
                    } else {
                        explorer.select_only(Some(&entry.path), cx);
                    }
                });
                self.begin_press(keep.then(|| entry.path.clone()), event.position);
                if event.click_count >= 2 {
                    self.press = None;
                    self.explorer
                        .update(cx, |explorer, cx| explorer.open_entry(&entry, cx).detach());
                }
            }
            _ => {}
        }
    }

    /// A press on the empty part of the list (`onBackgroundMouseDown`, `startMarquee`): without Ctrl or Shift it
    /// clears the selection, and dragging from here draws a rectangle that selects the rows it crosses (added to
    /// what was selected with Ctrl), the list scrolling while the pointer is held near its top or bottom.
    fn background_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.take_focus(window, cx);
        match event.button {
            MouseButton::Left => {
                let additive = event.modifiers.control || event.modifiers.platform;
                if !additive && !event.modifiers.shift {
                    self.explorer.update(cx, |explorer, cx| explorer.select_only(None, cx));
                }
                self.begin_marquee(event.position, additive, cx);
            }
            MouseButton::Right => {
                self.explorer.update(cx, |explorer, cx| explorer.select_only(None, cx));
                cx.emit(FileListEvent::ContextMenu {
                    target: None,
                    at: Some(event.position),
                    shift: false,
                });
            }
            _ => {}
        }
    }

    /// The button is let go: a press kept on one item of a selection narrows to it.
    fn release(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.press.take().and_then(|press| press.narrow_to) {
            self.explorer
                .update(cx, |explorer, cx| explorer.select_only(Some(&path), cx));
        }
    }

    // ---- Drawing ----------------------------------------------------------------------------------------

    /// The lines of the list that are on screen.
    fn render_lines(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let frame = self.frame.clone();
        self.show_thumbnails(&frame, &range, window, cx);
        self.ask_facts(&frame, &range, cx);
        let t = theme::tokens(cx).clone();
        range.map(|line| self.line_element(&frame, &t, line, cx)).collect()
    }

    /// Asks for the tags (and, while a column of them is shown, the media facts) of the lines about to be drawn and
    /// a half screen either side, not of the whole folder. Asking for what is read costs lookups, so a frame that
    /// draws the same lines as the last asks nothing.
    fn ask_facts(&mut self, frame: &Frame, range: &Range<usize>, cx: &mut Context<Self>) {
        if frame.rows.is_empty() {
            return;
        }
        let want = Want {
            tags: true,
            media: frame.media,
        };
        let margin = range.len() / 2;
        let (from, to) = (
            range.start.saturating_sub(margin),
            range.end.saturating_add(margin).min(frame.lines),
        );
        let same = self.asked.as_ref().is_some_and(|asked| {
            asked.key == frame.facts_key
                && (asked.from, asked.to, asked.rows, asked.want) == (from, to, frame.rows.len(), want)
        });
        if same || from >= to {
            return;
        }
        self.asked = Some(Asked {
            key: frame.facts_key.clone(),
            from,
            to,
            rows: frame.rows.len(),
            want,
        });
        let entries = cells::entries_on(&frame.rows, from..to);
        self.facts
            .update(cx, |facts, cx| facts.ask(&frame.facts_key, entries, want, cx));
    }

    /// The tags of what tab `tab` has selected are read, if it is this pane's and the selection is of a size worth
    /// reading (the Tags submenu checks them for the whole selection).
    fn ask_selection_tags(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let read = self
            .explorer
            .read(cx)
            .pane_tab(self.pane)
            .filter(|t| t.id == tab)
            .and_then(|tab| {
                let size = tab.selection.len();
                let on_disk = tab.archive().is_none() && !tab.is_recycle_bin();
                (on_disk && (1..=MOST_SELECTION_READS).contains(&size))
                    .then(|| (tab.view_key(), tab.selection.to_vec()))
            });
        if let Some((key, paths)) = read {
            self.facts.update(cx, |facts, cx| {
                facts.ask_tags_of(&key, paths.iter().map(String::as_str), cx)
            });
        }
    }

    /// What the list is called for a screen reader (`aria-label`).
    fn list_label(view: &TabView) -> SharedString {
        if view.find.is_some() {
            t!("Search results")
        } else if view.recycled {
            t!("Recycle Bin")
        } else {
            t!("Files")
        }
    }
}

/// What Git says of the items of `tab`'s folder `path`: not the marks it still holds of the folder the tab was in
/// before, while the new one is being looked at.
pub(crate) fn marks_in(cx: &App, tab: TabId, path: &str) -> Option<Arc<GitMarks>> {
    let run = git::try_shared(cx)?.read(cx).run(tab)?;
    if !same_path(&run.folder, path) {
        return None;
    }
    run.view.as_ref().map(|view| view.marks.clone())
}

impl Frame {
    /// The item is the same as its namesake in the other pane (compare panes).
    pub(crate) fn same_as_other(&self, entry: &FileEntry) -> bool {
        self.diffs
            .as_ref()
            .is_some_and(|diffs| diffs.status(entry) == Some(glint_model::entries::DiffStatus::Same))
    }
}

impl Focusable for FileList {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for FileList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t: Tokens = theme::tokens(cx).clone();
        let root = col()
            .id(("file-list", self.pane))
            .size_full()
            .min_h_0()
            .relative()
            .key_context("FileList");
        let Some(view) = self.read_tab(cx) else {
            return root.track_focus(&self.focus).into_any_element();
        };
        self.sync(&view, window, cx);
        // Another folder (or search) than the one drawn last: its rows arrive. The tab's path changes only when the
        // new listing is in, so this is the frame the rows change; a re-read of the same folder is not an arrival.
        if self.entered_for.as_deref() != Some(view.facts_key.as_str()) {
            if self.entered_for.is_some() {
                self.entered += 1;
            }
            self.entered_for = Some(view.facts_key.clone());
        }

        // What this frame's lines are drawn from.
        let tracks = self.tracks(&view, window, cx);
        let explorer = self.explorer.read(cx);
        let (cut, flashed) = (explorer.cut_paths().clone(), explorer.flashed().clone());
        let lines = layout_rows::line_count(&view.rows);
        // Files being dragged over the list light the folder under them; nothing once the drag has left.
        let drop_at = self.drop_at.clone().filter(|_| cx.has_active_drag());
        let folder_drop = drop_at == Some(DropAt::Folder);
        let thumbnails = state::settings(cx).list_thumbnails;
        let frame = Rc::new(Frame {
            rows: view.rows.clone(),
            selected: view.selection.clone(),
            focus_path: view.selection.focus().map(str::to_string),
            renaming: view.renaming.clone(),
            media: if tracks
                .iter()
                .any(|track| matches!(track, Track::Column { id, .. } if id.def().facts))
            {
                Media::All
            } else {
                Media::None
            },
            tracks,
            facts_key: view.facts_key.clone(),
            inactive: view.inactive,
            located: view.located,
            recycled: view.recycled,
            where_of: view
                .located
                .then(|| Box::new(layout::relative_to(&view.scope)) as WhereOf),
            max_size: self.shown.max_size,
            sizes: view.sizes.clone(),
            measuring: view.measuring,
            git: view.git.clone(),
            diffs: view.diffs.clone(),
            flashed,
            now: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
            cut,
            dragging: self.dragging.clone(),
            drop_row: match &drop_at {
                Some(DropAt::Row(path)) => Some(path.clone()),
                _ => None,
            },
            lines,
            tab: view.id,
            inside: view.find.as_ref().map(|find| find.inside.clone()).unwrap_or_default(),
            thumbnails,
            lane: view.id.to_string(),
            box_px: f32::from(window.rem_size()),
            scale: window.scale_factor(),
        });
        self.frame = frame.clone();

        let header = self.header_row(&frame, &view, &t, cx);
        let state = self.state_view(&view, &t, cx);

        // The width the list is laid out at decides which columns fit; a canvas under it measures that, and
        // asks for another frame when it changes.
        let measured = self.width.clone();
        let area_bounds = self.area.clone();
        let this = cx.weak_entity();
        let canvas_this = self.tracker();
        let measure = canvas(
            move |bounds, window, _| {
                area_bounds.set(bounds);
                let width = f32::from(bounds.size.width);
                if (measured.get() - width).abs() > 0.5 {
                    measured.set(width);
                    let this = this.clone();
                    window.on_next_frame(move |_, cx| {
                        this.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            },
            // A press or a marquee follows the pointer wherever it goes in the window.
            move |_, _, window, _| canvas_this.track(window),
        )
        .absolute()
        .size_full();

        let list = uniform_list("file-list-lines", lines, cx.processor(Self::render_lines))
            .track_scroll(&self.scroll)
            .size_full();
        // A folder's rows arrive: readable from the first frame, settling upward the last few pixels.
        let list = div().size_full().child(list).with_animation(
            ("file-list-enter", self.entered),
            Animation::new(theme::motion::BASE).with_easing(theme::motion::enter),
            |rows, delta| rows.opacity(0.35 + 0.65 * delta).top(px(10. * (1. - delta))),
        );
        let area = div()
            .id("file-list-area")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(measure)
            .child(
                div()
                    .id("file-list-box")
                    .role(Role::ListBox)
                    .aria_label(Self::list_label(&view))
                    .size_full()
                    .child(list)
                    .test_support(),
            )
            .child(base::Scrollbar::vertical(&self.scroll).viewport_from_layout())
            // The bar says the next folder is being read while this one stays on screen; with nothing to show
            // yet the state says it.
            .when(view.loading && view.has_listing, |this| {
                this.child(
                    div()
                        .id("file-list-progress")
                        .role(Role::ProgressIndicator)
                        .aria_label(t!("Loading…"))
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .child(loading_sweep("file-list-progress-sweep", cx))
                        .test_support(),
                )
            })
            .when_some(state, |this, state| this.child(div().absolute().inset_0().child(state)))
            // The rectangle dragged over the empty part (`startMarquee`'s `mark`).
            .when_some(self.marquee_bounds(), |this, rect| {
                this.child(
                    div()
                        .id("file-list-marquee")
                        .absolute()
                        .left(rect.origin.x)
                        .top(rect.origin.y)
                        .w(rect.size.width)
                        .h(rect.size.height)
                        .rounded(theme::size::RADIUS_SM)
                        .border_1()
                        .border_color(t.primary.opacity(0.8))
                        .bg(t.primary.opacity(0.15))
                        .test_support(),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| this.background_down(event, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| this.background_down(event, window, cx)),
            )
            .test_support();

        root.track_focus(&self.focus)
            // Files dragged over the list: the folder under them takes them (see `incoming.rs`).
            .on_drag_move::<ExternalPaths>(
                cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, window, cx| {
                    this.answer_drag(event, window, cx)
                }),
            )
            .on_drop::<ExternalPaths>(
                cx.listener(|this, dropped: &ExternalPaths, window, cx| this.take_drop(dropped, window, cx)),
            )
            .on_action(cx.listener(|this, _: &actions::OpenItem, _, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &actions::OpenInNewTab, _, cx| this.open_in_tabs(cx)))
            .on_action(cx.listener(|this, _: &actions::SelectAll, _, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &actions::ShowItemMenu, window, cx| this.keyboard_menu(window, cx)))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.release(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| this.release(cx)),
            )
            .child(header)
            .child(area)
            // The folder shown takes the files: a dashed outline round the list.
            .when(folder_drop, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(t.primary.opacity(0.05))
                        .border_2()
                        .border_dashed()
                        .border_color(t.primary.opacity(0.6)),
                )
            })
            // While an edge is held, a layer over the whole list takes the pointer: the drag follows it
            // wherever it goes in the list and ends when the button is let go.
            .when(self.resizing.is_some(), |this| {
                this.child(
                    div()
                        .id("file-list-resizing")
                        .absolute()
                        .inset_0()
                        .cursor_col_resize()
                        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                            this.drag_resize(event.position.x, cx);
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_resize(cx)),
                        )
                        .on_mouse_up_out(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _, cx| this.end_resize(cx)),
                        ),
                )
            })
            .into_any_element()
    }
}
