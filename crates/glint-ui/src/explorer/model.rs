//! The `Explorer` entity: panes, tabs, drives, known folders and the derived getters.
//! Ported from the state, the "Tabs" helpers and the derived getters of src/lib/explorer.ts
//! (`visibleTabs`, `activeTabId`, `tabState`, `currentPath`, `archiveOf`, `remoteOf`, `closedOf`,
//! `recycleBinOf`, `viewKey`, `setView`, `zoomGrid`, `tabName`, `savedScroll`/`saveScroll`, `flashPaths`).
//!
//! The behaviour (navigating, tabs, selection, opening…) is in the sibling files as more `impl Explorer`
//! blocks; this file is the data and the plumbing they share: the settings snapshot, the rows rebuild, the
//! central `changed` hook and the handling of core's events.

use super::memory::Memory;
use super::ops_state::OpsState;
use super::rows::{build_rows, reads_git, RowMarks, RowSpec, Rows};
use super::source::Source;
use super::tab::Tab;
use super::types::*;
use crate::core_bridge::{self, CoreEvent};
use crate::state::UiState;
use glint_model::columns::{folder_kind_of, in_order, ColumnId, FolderKind, FolderPlace};
use glint_model::entries::{FileEntry, Sort};
use glint_model::grid::{zoom_tiles, TileSize};
use glint_model::groups::GroupBy;
use glint_model::paths::{is_gallery, is_recent, is_recycle_bin, same_path, THIS_PC};
use glint_model::store::{FolderViewPatch, Settings, StartFolder};
use gpui_kit::{App, AppContext as _, Context, EventEmitter, Subscription, Task};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Listings with more entries than this are sorted on a background thread rather than on the UI thread.
const ROWS_IN_BACKGROUND: usize = 20_000;
/// How long a flashed path stays highlighted (the TypeScript's 1200 ms).
const FLASH_FOR: Duration = Duration::from_millis(1200);

/// A tab Ctrl+Shift+T can bring back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClosedTab {
    pub path: String,
    pub view: ViewMode,
    pub pane: usize,
    pub index: usize,
}

/// The few settings the explorer reads, taken in one go (so no code path reads a global that a test or an
/// early start hasn't made: without `UiState` the defaults apply).
#[derive(Clone, Debug)]
pub(super) struct Prefs {
    pub folders_first: bool,
    pub show_hidden: bool,
    pub group_by: GroupBy,
    pub per_folder_views: bool,
    pub sort: Sort,
    pub list_columns: Vec<ColumnId>,
    pub restore_tabs: bool,
    pub start_folder: StartFolder,
    pub zoxide: bool,
    pub tile_size: TileSize,
    /// Copies and moves go to Glint's copy queue rather than Windows' dialog.
    pub copy_queue: bool,
    /// The queue reads copies back and compares them with their originals.
    pub verify_copies: bool,
    /// Windows asks before it deletes.
    pub confirm_delete: bool,
    /// Searches by name follow .gitignore files (fd's alone).
    pub find_respect_ignore: bool,
    /// Names and globs a search skips.
    pub find_exclusions: Vec<String>,
    /// The most results a search keeps; none for no limit.
    pub find_max_results: Option<u64>,
    /// Names and globs a search inside files skips.
    pub inside_exclusions: Vec<String>,
    /// The most matching lines a search inside files keeps; none for no limit.
    pub inside_max_results: Option<u64>,
    /// Compare panes goes into the subfolders too (the compare bar's options).
    pub compare_subfolders: bool,
    /// Compare panes reads the files' contents, not only their sizes and dates.
    pub compare_contents: bool,
}

impl Prefs {
    fn of(settings: &Settings) -> Prefs {
        Prefs {
            folders_first: settings.folders_first,
            show_hidden: settings.show_hidden,
            group_by: settings.group_by,
            per_folder_views: settings.per_folder_views,
            sort: settings.sort,
            list_columns: settings.list_columns.clone(),
            restore_tabs: settings.restore_tabs,
            start_folder: settings.start_folder,
            zoxide: settings.zoxide,
            tile_size: settings.tile_size,
            copy_queue: settings.copy_queue,
            verify_copies: settings.verify_copies,
            confirm_delete: settings.confirm_delete,
            find_respect_ignore: settings.find_respect_ignore,
            find_exclusions: settings.find_exclusions.clone(),
            find_max_results: settings.find_max_results,
            inside_exclusions: settings.exclusions.clone(),
            inside_max_results: settings.max_results,
            compare_subfolders: settings.compare_subfolders,
            compare_contents: settings.compare_contents,
        }
    }

    pub fn read(cx: &App) -> Prefs {
        match cx.try_global::<UiState>() {
            Some(ui) => Prefs::of(&ui.settings),
            None => Prefs::of(&Settings::default()),
        }
    }
}

/// Everything about what Glint is showing: one or two panes of tabs, the drives and the well-known folders.
///
/// **How views use it.** Keep the `Entity<Explorer>` (from [`super::explorer`]) and
/// - read with `explorer.read(cx)` (all getters are `&self`),
/// - change with `explorer.update(cx, |explorer, cx| explorer.go_back(cx).detach())`,
/// - redraw on any change with `cx.observe(&explorer, ...)`, or on a particular kind of change with
///   `cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| ...)`.
///
/// Functions that read the disk return a `Task` (the TypeScript's promise): `.detach()` it to let it run,
/// or `.await` it to know whether it worked. **Dropping a task cancels it.**
pub struct Explorer {
    pub(super) source: Rc<dyn Source>,
    pub(super) tabs: HashMap<TabId, Tab>,
    /// One pane, or two (the right one keeps its tabs while hidden). Never empty.
    pub(super) panes: Vec<Pane>,
    pub(super) active_pane: usize,
    pub(super) dual: bool,
    /// Highlights unique, newer, older and size-different items across the two dual panes (compare part).
    pub compare_panes: bool,
    /// When comparing panes, hides identical items (compare part).
    pub diff_only: bool,
    pub(super) drives: Arc<[Drive]>,
    pub(super) known: Arc<[KnownFolder]>,
    /// The known folders being read since [`Explorer::read_ahead_for_start`], for [`Explorer::init_explorer`]
    /// to take.
    pub(super) known_reading: Option<super::source::Fut<Vec<KnownFolder>>>,
    /// The Gallery's folders last handed to glint-core ([`Explorer::push_gallery_folders`]).
    pub(super) gallery_sent: Option<Vec<String>>,
    /// Remote connections' names by lower-case id, for [`tab_name`] (the remotes part sets them).
    pub(super) remote_names: HashMap<String, String>,
    /// The address bar is being typed in (reset by anything that changes the folder shown).
    pub editing_address: bool,
    /// The Space-bar preview is open (closed by anything that changes the focused item's tab).
    pub quick_look: bool,
    pub(super) flashed: Arc<HashSet<String>>,
    pub(super) flash_task: Option<Task<()>>,
    pub(super) closed_tabs: Vec<ClosedTab>,
    /// Background tabs opened from one tab go after each other, as in a browser: (from, last opened).
    pub(super) opened_from: Option<(TabId, TabId)>,
    pub(super) tab_seq: u32,
    pub(super) memory: Memory,
    /// The session is saved only once start-up has restored it, so starting can't overwrite it.
    pub(super) restored: bool,
    pub(super) watching: String,
    pub(super) hinted: String,
    pub(super) hint_task: Option<Task<()>>,
    pub(super) reported_folder: String,
    pub(super) first_painted: bool,
    pub(super) prefetched: HashMap<String, super::nav::PrefetchEntry>,
    pub(super) hover_task: Option<Task<()>>,
    /// Tabs refreshed with F5, until what waits for it (Git status in a very big repository) has run.
    pub refreshed_by_hand: HashSet<TabId>,
    /// `init_explorer` has been called (it runs once).
    pub(super) initialized: bool,
    /// The file operations' state: cut items, Undo, slow work, the copy queue, the New menu.
    pub(super) ops: OpsState,
    /// The searches by name: running ones, their numbering and where instant search stands.
    pub(super) searching: super::find::Searching,
    /// Archive work in progress (compressing, extracting, staging copy-outs) and the drag that is ready.
    pub(super) archives: super::archive::ArchiveState,
    /// The menu actions' state: the folder sizes measured, the remotes shown in File Explorer.
    pub(super) actions: super::actions::ActionState,
    /// Compare panes: each pane's answer ([`Explorer::comparison_of`]).
    pub(super) compare: super::compare::CompareState,
    pub(super) _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Explorer {
    /// An explorer reading folders from `source`, with one tab on This PC (not yet opened: call
    /// [`Explorer::init_explorer`]). Follows the live settings and core's "dir-changed" events.
    pub fn new(source: Rc<dyn Source>, cx: &mut Context<Self>) -> Explorer {
        let first = TabId(1);
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_global::<UiState>(|this, cx| this.on_settings_changed(cx)));
        subscriptions.extend(core_bridge::subscribe(cx, |this: &mut Self, event, cx| {
            this.on_core_event(event, cx)
        }));
        let mut tabs = HashMap::new();
        tabs.insert(first, Tab::blank(first, THIS_PC, ViewMode::List, None));
        Explorer {
            source,
            tabs,
            panes: vec![Pane {
                tabs: vec![first],
                active: first,
            }],
            active_pane: 0,
            dual: false,
            compare_panes: false,
            diff_only: false,
            drives: Arc::from(Vec::new()),
            known: Arc::from(Vec::new()),
            known_reading: None,
            gallery_sent: None,
            remote_names: HashMap::new(),
            editing_address: false,
            quick_look: false,
            flashed: Arc::new(HashSet::new()),
            flash_task: None,
            closed_tabs: Vec::new(),
            opened_from: None,
            tab_seq: 1,
            memory: Memory::new(),
            restored: false,
            watching: String::new(),
            hinted: String::new(),
            hint_task: None,
            reported_folder: String::new(),
            first_painted: false,
            prefetched: HashMap::new(),
            hover_task: None,
            refreshed_by_hand: HashSet::new(),
            initialized: false,
            ops: OpsState::new(),
            searching: Default::default(),
            archives: Default::default(),
            actions: Default::default(),
            compare: Default::default(),
            _subscriptions: subscriptions,
        }
    }

    // ---- Reading ---------------------------------------------------------------------------------------

    /// A tab's state; `None` once it's closed (`tabState`).
    pub fn tab(&self, id: TabId) -> Option<&Tab> {
        self.tabs.get(&id)
    }

    /// The tab the keyboard, toolbar and palette act on: the active pane's (`tabState()`).
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(&self.active_tab_id())
    }

    /// The active pane's tab id (`activeTabId`).
    pub fn active_tab_id(&self) -> TabId {
        self.pane_tab_id(self.active_pane)
    }

    /// The tab on screen in `pane` (the active pane's, or the first pane's, when there is no such pane).
    pub fn pane_tab_id(&self, pane: usize) -> TabId {
        self.panes
            .get(pane)
            .or_else(|| self.panes.first())
            .map_or(TabId(0), |p| p.active)
    }

    /// The tab on screen in `pane` (`useTab` in a `PaneProvider`).
    pub fn pane_tab(&self, pane: usize) -> Option<&Tab> {
        self.tabs.get(&self.pane_tab_id(pane))
    }

    /// The panes on screen: the left one, and the right one in dual-pane view (`shownPanes`).
    pub fn shown_panes(&self) -> &[Pane] {
        if self.dual {
            &self.panes
        } else {
            self.panes.get(..1).unwrap_or(&[])
        }
    }

    /// All panes, including the right one while hidden.
    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    /// The tabs on screen, one per visible pane (`visibleTabs`).
    pub fn visible_tabs(&self) -> Vec<TabId> {
        self.shown_panes().iter().map(|p| p.active).collect()
    }

    /// Which pane a tab is in (`paneOf`).
    pub fn pane_of(&self, id: TabId) -> Option<usize> {
        self.panes.iter().position(|p| p.tabs.contains(&id))
    }

    /// The pane the keyboard, toolbar and address bar act on.
    pub fn active_pane(&self) -> usize {
        self.active_pane
    }

    /// Dual-pane view is on.
    pub fn is_dual(&self) -> bool {
        self.dual
    }

    /// The active tab's folder, or This PC (`currentPath`).
    pub fn current_path(&self) -> &str {
        self.active_tab().map_or(THIS_PC, |t| t.path.as_str())
    }

    /// The archive a tab's folder is in (the active tab's by default), from its listing; `None` for a folder
    /// on disk (`archiveOf`).
    pub fn archive_of(&self, tab: Option<TabId>) -> Option<&glint_model::entries::InArchive> {
        self.tab_or_active(tab)?.archive()
    }

    /// Whether a tab (the active tab by default) shows a remote connection's folder (`remoteOf`).
    pub fn remote_of(&self, tab: Option<TabId>) -> bool {
        self.tab_or_active(tab).is_some_and(|t| t.is_remote())
    }

    /// Whether Windows keeps a tab's folder closed to this account (`closedOf`).
    pub fn closed_of(&self, tab: Option<TabId>) -> bool {
        self.tab_or_active(tab).is_some_and(|t| t.is_closed())
    }

    /// Whether a tab shows the Recycle Bin (`recycleBinOf`).
    pub fn recycle_bin_of(&self, tab: Option<TabId>) -> bool {
        self.tab_or_active(tab).is_some_and(|t| t.is_recycle_bin())
    }

    /// What a tab shows: a folder, or a search's results (`viewKey`).
    pub fn view_key(&self, tab: Option<TabId>) -> String {
        self.tab_or_active(tab).map(Tab::view_key).unwrap_or_default()
    }

    fn tab_or_active(&self, tab: Option<TabId>) -> Option<&Tab> {
        self.tabs.get(&tab.unwrap_or_else(|| self.active_tab_id()))
    }

    /// The drives, as last read ([`Explorer::refresh_drives`]).
    pub fn drives(&self) -> &Arc<[Drive]> {
        &self.drives
    }

    /// Home, Desktop… as found at start.
    pub fn known_folders(&self) -> &Arc<[KnownFolder]> {
        &self.known
    }

    /// The path of a known folder, if this PC has it.
    pub fn known_path(&self, id: KnownId) -> Option<&str> {
        self.known.iter().find(|k| k.id == id).map(|k| k.path.as_str())
    }

    /// Which known folder `path` is (Documents itself, not what is in it), for drawing it in its own color.
    pub fn known_id_of(&self, path: &str) -> Option<KnownId> {
        self.known
            .iter()
            .find(|k| k.path.len() == path.len() && k.path.eq_ignore_ascii_case(path))
            .map(|k| k.id)
    }

    /// Home, or This PC when it isn't known (`homeFolder`).
    pub fn home_folder(&self) -> &str {
        self.known_path(KnownId::Home).unwrap_or(THIS_PC)
    }

    /// Paths briefly highlighted because they just arrived (`flashed`).
    pub fn flashed(&self) -> &Arc<HashSet<String>> {
        &self.flashed
    }

    /// Whether Ctrl+Shift+T has a tab to bring back (`canReopenTab`).
    pub fn can_reopen_tab(&self) -> bool {
        !self.closed_tabs.is_empty()
    }

    /// What a path is called in a tab or crumb: "This PC", the folder's name, a drive's name… (`tabName`).
    /// English; translate the fixed names.
    pub fn tab_name(&self, path: &str) -> String {
        tab_name(path, &self.drives, &|p| {
            let id = remote_id(p);
            self.remote_names.get(&id.to_lowercase()).cloned().unwrap_or(id)
        })
    }

    /// What the explorer keeps between runs (see [`Memory`]).
    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    /// Gives the explorer what the app saved. Call once after loading it, before [`Explorer::init_explorer`].
    pub fn set_memory(&mut self, memory: Memory, cx: &mut Context<Self>) {
        self.memory = memory;
        self.on_settings_changed(cx);
    }

    // ---- Changing a tab ---------------------------------------------------------------------------------

    /// Changes a tab's state (`updateTab`). Does not notify: the caller says what changed.
    pub(super) fn update_tab(&mut self, id: TabId, change: impl FnOnce(&mut Tab)) -> bool {
        match self.tabs.get_mut(&id) {
            Some(tab) => {
                change(tab);
                true
            }
            None => false,
        }
    }

    /// Makes a new tab id.
    pub(super) fn new_tab_id(&mut self) -> TabId {
        self.tab_seq += 1;
        TabId(self.tab_seq)
    }

    /// Tells everyone a tab changed, and saves what should be saved. Every mutation ends here.
    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        let _span = crate::perf::span("explorer changed");
        self.save_session(cx);
        self.hint_startup(cx);
        self.report_folder();
        self.refresh_comparison(cx);
        cx.notify();
    }

    /// Tells everyone one tab changed.
    pub(super) fn tab_changed(&mut self, id: TabId, cx: &mut Context<Self>) {
        cx.emit(ExplorerEvent::TabChanged(id));
        self.changed(cx);
    }

    /// Shows a tab's items (the active tab's by default) as a list, or as a grid (`setView`).
    pub fn set_view(&mut self, view: ViewMode, tab: Option<TabId>, cx: &mut Context<Self>) {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        if self.tabs.get(&id).is_none_or(|t| t.view == view) {
            return;
        }
        self.update_tab(id, |t| t.view = view);
        // Only the list has group headers: the rows change with the view.
        self.rebuild_rows(id, cx);
        cx.emit(ExplorerEvent::ViewChanged(id));
        self.tab_changed(id, cx);
    }

    /// Ctrl+wheel, Ctrl+= and Ctrl+- in a grid: bigger tiles (`steps` up) or smaller ones, in every grid; 0 for
    /// medium again (`zoomGrid`).
    pub fn zoom_grid(&mut self, steps: i64, cx: &mut Context<Self>) {
        let current = Prefs::read(cx).tile_size;
        let wanted = if steps != 0 {
            zoom_tiles(current.as_str(), steps)
        } else {
            Settings::default().tile_size
        };
        if wanted != current {
            crate::state::update(cx, |settings| settings.tile_size = wanted);
        }
    }

    /// Sorts the active tab's list by `sort` (a click on a column header). With Settings → "Each folder keeps
    /// its own view" the folder remembers it; otherwise every folder sorts so.
    pub fn set_sort(&mut self, sort: Sort, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let prefs = Prefs::read(cx);
        let own = self
            .tabs
            .get(&id)
            .filter(|t| prefs.per_folder_views && t.find.is_none() && t.path != THIS_PC);
        if let Some(tab) = own {
            let path = tab.path.clone();
            self.memory.set_folder_view(
                &path,
                Some(FolderViewPatch {
                    sort: Some(sort),
                    ..Default::default()
                }),
                now_ms(),
            );
            cx.emit(ExplorerEvent::MemoryChanged);
            self.refresh_views(cx);
        } else {
            crate::state::update(cx, |settings| settings.sort = sort);
            self.refresh_views(cx);
        }
    }

    /// Group by in the list (Settings → Files → Group by).
    pub fn set_group(&mut self, group: GroupBy, cx: &mut Context<Self>) {
        crate::state::update(cx, |settings| settings.group_by = group);
        self.refresh_views(cx);
    }

    /// The text in a tab's search box, which narrows what the list shows (the tab's by default).
    pub fn set_filter(&mut self, text: &str, tab: Option<TabId>, cx: &mut Context<Self>) {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        if self.tabs.get(&id).is_none_or(|t| t.filter == text) {
            return;
        }
        self.update_tab(id, |t| t.filter = text.to_string());
        self.rebuild_rows(id, cx);
        self.tab_changed(id, cx);
    }

    // ---- Flash, scroll, timing --------------------------------------------------------------------------

    /// Briefly highlights items that just arrived: created, pasted, dropped, renamed or revealed
    /// (`flashPaths`). Passing none clears it at once.
    pub fn flash_paths(&mut self, paths: impl IntoIterator<Item = String>, cx: &mut Context<Self>) {
        let next: HashSet<String> = paths.into_iter().collect();
        let any = !next.is_empty();
        self.flashed = Arc::new(next);
        // Replacing the task cancels the older timer, as clearTimeout did.
        self.flash_task = any.then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(FLASH_FOR).await;
                this.update(cx, |this, cx| {
                    this.flashed = Arc::new(HashSet::new());
                    cx.emit(ExplorerEvent::FlashChanged);
                    cx.notify();
                })
                .ok();
            })
        });
        cx.emit(ExplorerEvent::FlashChanged);
        cx.notify();
    }

    /// Remembers where a tab's list was scrolled for the folder or search `key` ([`Tab::view_key`]), to come
    /// back to it (`saveScroll`). Does not redraw anything.
    pub fn save_scroll(&mut self, tab: TabId, key: &str, top: f32) {
        self.update_tab(tab, |t| {
            t.scroll = Some(super::tab::ScrollMemo {
                key: key.to_string(),
                top,
            })
        });
    }

    /// Where a tab's list was scrolled for `key` (`savedScroll`); 0 for any other view.
    pub fn saved_scroll(&self, tab: TabId, key: &str) -> f32 {
        self.tabs.get(&tab).map_or(0.0, |t| t.saved_scroll(key))
    }

    /// Called once a tab's new list is on screen, to time the whole navigation: from the click to the list
    /// painted (`markShown`). Sets [`Tab::shown_in`].
    pub fn mark_shown(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(started) = self.tabs.get_mut(&tab).and_then(|t| t.nav_started.take()) else {
            return;
        };
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        crate::perf::say(|| format!("nav shown at={ms:.2}"));
        self.update_tab(tab, |t| t.shown_in = ms);
        self.tab_changed(tab, cx);
    }

    // ---- Settings and rows ------------------------------------------------------------------------------

    /// The settings (or the folders' own views) changed: sort, group, columns and rows follow.
    pub(super) fn on_settings_changed(&mut self, cx: &mut Context<Self>) {
        self.push_gallery_folders(cx);
        self.refresh_views(cx);
        // Show hidden items changes what is compared.
        self.refresh_comparison(cx);
        cx.notify();
    }

    /// Hands glint-core the Gallery's folders when they change: Pictures and Videos, and the ones added in Settings
    /// (`initGallery`). The Gallery lists the photos and videos below them.
    pub(super) fn push_gallery_folders(&mut self, cx: &mut Context<Self>) {
        let known: Vec<String> = self
            .known
            .iter()
            .filter(|k| matches!(k.id, KnownId::Pictures | KnownId::Videos))
            .map(|k| k.path.clone())
            .collect();
        let added = crate::state::settings(cx).gallery_folders.clone();
        let mut folders = known.clone();
        folders.extend(added.into_iter().filter(|f| !known.iter().any(|k| same_path(k, f))));
        if self.gallery_sent.as_ref() == Some(&folders) {
            return;
        }
        self.gallery_sent = Some(folders.clone());
        // glint-core keeps the handle in a process-wide static: tests, each with its own executor, would drop each
        // other's handles on the wrong thread, so under test the folders are recorded but not handed over.
        if cfg!(test) {
            return;
        }
        if let Some(core) = cx.try_global::<core_bridge::Core>() {
            glint_core::gallery::gallery_set_folders(core.app.clone(), folders);
        }
    }

    /// Resolves every tab's sort, group and columns from the settings and the folders' own views, and rebuilds
    /// the rows of those whose inputs changed.
    pub(super) fn refresh_views(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<TabId> = self.tabs.keys().copied().collect();
        for id in ids {
            if self.resolve_view(id, cx) {
                cx.emit(ExplorerEvent::ViewChanged(id));
            }
            self.rebuild_rows(id, cx);
        }
    }

    /// Works out a tab's effective sort, group and columns (the folder's own view, else its kind's, else
    /// Settings'), as `Explorer.tsx` did on every render. Returns whether anything changed.
    pub(super) fn resolve_view(&mut self, id: TabId, cx: &App) -> bool {
        let prefs = Prefs::read(cx);
        let Some(tab) = self.tabs.get(&id) else {
            return false;
        };
        let find = tab.find.is_some();
        let per_folder = prefs.per_folder_views;
        // The folder's own columns and sort (and list or grid), or, until it has its own, its kind's.
        let own = (per_folder && !find && tab.path != THIS_PC)
            .then(|| self.memory.folder_view(&tab.path))
            .flatten();
        let bin = is_recycle_bin(&tab.path) && !find;
        let columns = if find || bin {
            vec![ColumnId::Modified, ColumnId::Size]
        } else {
            let chosen: Vec<&str> = match own.and_then(|v| v.columns.as_ref()) {
                Some(own) => own.iter().map(|c| c.as_str()).collect(),
                None if per_folder => tab.kind.def().columns.iter().map(|c| c.as_str()).collect(),
                None => prefs.list_columns.iter().map(|c| c.as_str()).collect(),
            };
            in_order(&chosen)
        };
        let sort = (!find && per_folder)
            .then(|| own.and_then(|v| v.sort).or(tab.kind.def().sort))
            .flatten()
            .unwrap_or(prefs.sort);
        let group = prefs.group_by;
        let changed = tab.sort != sort || tab.group != group || tab.columns != columns;
        self.update_tab(id, |t| {
            t.sort = sort;
            t.group = group;
            t.columns = columns;
        });
        changed
    }

    /// What kind of folder a tab shows, which decides its default columns: Recent, Gallery and Downloads by
    /// where they are, else by what's in it (`folderKind`).
    pub(super) fn kind_of(&self, id: TabId) -> FolderKind {
        let _span = crate::perf::span("kind of");
        let Some(tab) = self.tabs.get(&id) else {
            return FolderKind::General;
        };
        let find = tab.find.is_some();
        let downloads = !find
            && self
                .known
                .iter()
                .any(|k| k.id == KnownId::Downloads && same_path(&k.path, &tab.path));
        let place = if is_recent(&tab.path) && !find {
            Some(FolderPlace::Recent)
        } else if is_gallery(&tab.path) && !find {
            Some(FolderPlace::Gallery)
        } else if downloads {
            Some(FolderPlace::Downloads)
        } else {
            None
        };
        // What a big folder mostly holds is read from an even sample of it: every entry of 100,000 took 2 ms
        // on this thread to say what a twentieth of them says as well.
        let entries = tab.entries();
        let step = (entries.len() / KIND_SAMPLE).max(1);
        folder_kind_of(entries.iter().step_by(step), place)
    }

    /// The row spec for a tab under the current settings.
    fn row_spec(&self, id: TabId, prefs: &Prefs) -> Option<RowSpec> {
        let tab = self.tabs.get(&id)?;
        // Only the list has headers: the grid and the columns view show items plain.
        let group = if tab.view == ViewMode::List {
            tab.group
        } else {
            GroupBy::None
        };
        Some(RowSpec {
            sort: tab.sort,
            folders_first: prefs.folders_first,
            show_hidden: prefs.show_hidden,
            filter: tab.filter.clone(),
            group,
            // A listing arrives sorted; a search's results are sorted here.
            presorted: tab.find.is_none(),
            sizes: self.row_sizes(tab),
            diffs: self.diff_filter(id),
            // Search results have no marks (`useGit(...).view.marks` is undefined while a search shows).
            git: tab
                .git
                .clone()
                .filter(|_| tab.find.is_none() && same_path(&tab.git_for, &tab.path) && reads_git(&tab.filter, group))
                .map(|marks| RowMarks { marks }),
            // What the search found inside files: those files stay among the rows, and in the list each file's
            // lines go under its row.
            inside: tab
                .find
                .as_ref()
                .filter(|find| !find.inside.hits.is_empty())
                .map(|find| super::rows::RowInside {
                    only: find.inside.only.clone(),
                    hits: find.inside.hits.clone(),
                    lines: tab.view == ViewMode::List,
                }),
        })
    }

    /// Tells the explorer what Git says of the items in `tab`'s folder (`Git` model's marks, by lowercase name; `None`
    /// outside a repository). The `git:` filter (`git:changed`, `git:new`, `git:staged`, `git:modified`,
    /// `git:conflicted`, `git:ignored`) and Group by Git read them: the tab's rows are rebuilt when it has either,
    /// and not otherwise. They are the marks of the folder the tab is in when they are given: once the tab opens
    /// another folder they are ignored until the window gives it the new ones. Call it whenever Git's view of the tab
    /// changes (`GitEvent::Changed(tab)`):
    ///
    /// ```ignore
    /// let marks = components::git::marks(cx, tab);
    /// explorer.update(cx, |explorer, cx| explorer.set_git_marks(tab, marks, cx));
    /// ```
    pub fn set_git_marks(
        &mut self,
        id: TabId,
        marks: Option<Arc<glint_model::entries::GitMarks>>,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(&id) else {
            return;
        };
        let same = match (&tab.git, &marks) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            _ => false,
        };
        if same && same_path(&tab.git_for, &tab.path) {
            return;
        }
        tab.git = marks;
        tab.git_for = tab.path.clone();
        // Only the rows that read the marks change with them; a rebuild with the same spec is skipped.
        self.rebuild_rows(id, cx);
        cx.notify();
    }

    /// Rebuilds a tab's rows if their inputs changed (the listing, the sort, a setting, the filter). A
    /// listing of more than 20,000 entries that needs real sorting is sorted on a background thread: until it
    /// is done the tab shows the listing in the engine's order.
    pub(super) fn rebuild_rows(&mut self, id: TabId, cx: &mut Context<Self>) {
        let _span = crate::perf::span("rebuild rows");
        let prefs = Prefs::read(cx);
        let Some(spec) = self.row_spec(id, &prefs) else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(&id) else {
            return;
        };
        let source: Arc<[FileEntry]> = match &tab.find {
            Some(find) => find.entries.clone(),
            None => tab.shared_entries().unwrap_or_else(|| Arc::from(Vec::new())),
        };
        if tab.rows_spec.as_ref() == Some(&spec) && tab.rows_source_is(&source) {
            return;
        }
        tab.rows_seq += 1;
        let seq = tab.rows_seq;
        tab.rows_spec = Some(spec.clone());
        let now = now_ms();
        let heavy = source.len() > ROWS_IN_BACKGROUND && !is_plain(&spec);
        if !heavy {
            let building = std::time::Instant::now();
            let old = std::mem::replace(&mut tab.rows, build_rows(&source, &spec, now));
            crate::perf::say(|| {
                format!(
                    "rows built items={} took={:.2} filter={:?}",
                    source.len(),
                    crate::perf::ms(building),
                    spec.filter
                )
            });
            tab.set_rows_source(&source);
            cx.emit(ExplorerEvent::RowsChanged(id));
            // Grouped rows are entries of their own.
            let_go(Some(old.shared().clone()), cx);
            return;
        }
        if !tab.rows_source_is(&source) {
            // A different listing: the old rows no longer fit it. Show it as the engine sent it meanwhile.
            let old = std::mem::replace(&mut tab.rows, Rows::identity(source.clone()));
            tab.set_rows_source(&source);
            cx.emit(ExplorerEvent::RowsChanged(id));
            let_go(Some(old.shared().clone()), cx);
        }
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_spawn(async move { build_rows(&source, &spec, now) })
                .await;
            this.update(cx, |this, cx| {
                // A newer rebuild (or a closed tab) makes this result stale.
                if let Some(tab) = this.tabs.get_mut(&id).filter(|t| t.rows_seq == seq) {
                    let old = std::mem::replace(&mut tab.rows, rows);
                    cx.emit(ExplorerEvent::RowsChanged(id));
                    cx.notify();
                    let_go(Some(old.shared().clone()), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- Core's events ----------------------------------------------------------------------------------

    /// A folder on screen changed on disk ("dir-changed", with the folder's path), or the Gallery's dates were
    /// read ("gallery-dated").
    fn on_core_event(&mut self, event: &CoreEvent, cx: &mut Context<Self>) {
        match event.name.as_str() {
            // What a search inside files found (`inside.rs`).
            "search-event" => {
                if let Ok(raw) = serde_json::from_value::<super::inside::RawEvent>(event.payload.clone()) {
                    self.inside_heard(raw, cx);
                }
            }
            "dir-changed" => {
                if let Some(folder) = event.payload.as_str() {
                    self.reload_folder(folder, cx);
                }
            }
            "gallery-dated" => {
                let galleries: Vec<TabId> = self
                    .tabs
                    .values()
                    .filter(|t| is_gallery(&t.path) && t.find.is_none())
                    .map(|t| t.id)
                    .collect();
                for id in galleries {
                    self.reload(Some(id), cx).detach();
                }
            }
            "copy-queue" => self.on_queue_event(&event.payload, cx),
            _ => {}
        }
    }
}

/// About how many of a big folder's entries are looked at to say what kind of folder it is.
const KIND_SAMPLE: usize = 8_192;

/// A spec the engine's own order already satisfies and that cuts nothing out: building it is O(n) at most,
/// and so never worth a background thread.
fn is_plain(spec: &RowSpec) -> bool {
    use glint_model::entries::SortKey;
    spec.presorted
        && spec.folders_first
        && spec.sort.key == SortKey::Name
        && !spec.sort.desc
        && spec.filter.trim().is_empty()
        && spec.group == GroupBy::None
        && spec.diffs.is_none()
}

/// A listing of more entries than this is let go of on a background thread ([`let_go`]).
const LET_GO_AWAY_ABOVE: usize = 4_000;
/// How long the background holds a listing before letting it go: longer than the views that drew it keep theirs.
const LET_GO_AFTER: std::time::Duration = std::time::Duration::from_millis(500);

/// Lets go of a listing's entries. Each entry owns four strings, and whoever holds the listing last frees them
/// all: 400,000 frees for a folder of 100,000, some 12 ms. Left to chance that was the UI thread, in the middle
/// of showing the next folder. A big listing is held here a moment longer than the views that drew it and freed
/// on a background thread; a small one is simply dropped.
pub(crate) fn let_go(entries: Option<Arc<[FileEntry]>>, cx: &App) {
    let Some(entries) = entries.filter(|entries| entries.len() > LET_GO_AWAY_ABOVE) else {
        return;
    };
    let wait = cx.background_executor().timer(LET_GO_AFTER);
    cx.background_spawn(async move {
        wait.await;
        drop(entries);
    })
    .detach();
}

/// Now, in milliseconds since the Unix epoch.
pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
