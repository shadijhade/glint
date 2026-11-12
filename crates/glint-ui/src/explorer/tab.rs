//! One tab's state (`TabState` in explorer.ts) and its selection.
//!
//! A tab is a folder (or This PC, or search results) with its own history, listing, selection and view.
//! Views read it through [`Explorer::tab`](super::Explorer::tab) and never change it: every change goes
//! through an `Explorer` method, which keeps the rows, the watches and the events right.

use super::rows::{RowSpec, Rows};
use super::types::*;
use glint_model::columns::{ColumnId, FolderKind};
use glint_model::entries::{FileEntry, GitMarks, Sort, SortKey};
use glint_model::groups::GroupBy;
use glint_model::paths::THIS_PC;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// The selected paths in the order they were selected, with fast lookup (a row asks "am I selected?" every
/// frame). The TypeScript's `Set<string>` keeps insertion order, and opening several files uses it.
#[derive(Clone, Debug, Default)]
pub struct PathSet {
    order: Vec<Arc<str>>,
    set: HashSet<Arc<str>>,
}

impl PathSet {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.set.contains(path)
    }

    /// In the order they were selected.
    pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        self.order.iter().map(|p| &**p)
    }

    /// Adds a path (nothing if it's there already). Returns whether it was new.
    pub fn insert(&mut self, path: &str) -> bool {
        if self.set.contains(path) {
            return false;
        }
        let path: Arc<str> = Arc::from(path);
        self.order.push(path.clone());
        self.set.insert(path);
        true
    }

    /// Removes a path. Returns whether it was there.
    pub fn remove(&mut self, path: &str) -> bool {
        if !self.set.remove(path) {
            return false;
        }
        self.order.retain(|p| &**p != path);
        true
    }

    /// The last path added.
    pub fn last(&self) -> Option<&str> {
        self.order.last().map(|p| &**p)
    }
}

impl<'a> FromIterator<&'a str> for PathSet {
    fn from_iter<I: IntoIterator<Item = &'a str>>(paths: I) -> Self {
        let mut set = PathSet::default();
        for path in paths {
            set.insert(path);
        }
        set
    }
}

/// What a tab has selected: the paths, the item a Shift-selection grows from (`anchor`) and the one the
/// keyboard is on (`focus`). Clone-on-write: a view holding an earlier `Selection` never sees it change.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    pub(super) paths: Arc<PathSet>,
    pub(super) anchor: Option<String>,
    pub(super) focus: Option<String>,
}

impl Selection {
    /// A selection of these paths, with no anchor or focus.
    pub fn of<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        Selection {
            paths: Arc::new(paths.into_iter().collect()),
            anchor: None,
            focus: None,
        }
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.paths.contains(path)
    }

    /// The selected paths, in the order they were selected.
    pub fn iter(&self) -> impl Iterator<Item = &str> + '_ {
        self.paths.iter()
    }

    /// The selected paths as owned strings (what an operation on the selection takes).
    pub fn to_vec(&self) -> Vec<String> {
        self.iter().map(str::to_string).collect()
    }

    /// The item Shift-selection grows from.
    pub fn anchor(&self) -> Option<&str> {
        self.anchor.as_deref()
    }

    /// The item the keyboard is on.
    pub fn focus(&self) -> Option<&str> {
        self.focus.as_deref()
    }
}

/// Where a tab's list was scrolled, for the folder or search shown then (`scrolls` in explorer.ts).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ScrollMemo {
    pub key: String,
    pub top: f32,
}

/// One tab: a folder (or This PC, or search results) with its own history, selection and search.
pub struct Tab {
    pub id: TabId,
    /// False until its first folder has opened (that one doesn't go into Back history).
    pub ready: bool,
    pub path: String,
    /// The folder's contents, as the engine sent them. `None` for This PC and before the first folder opens.
    pub listing: Option<Listed>,
    /// How much of the folder is there to show.
    pub load: LoadState,
    /// A navigation is under way (a quiet re-read doesn't set this).
    pub loading: bool,
    /// From the click to the new list on screen, ms ([`Explorer::mark_shown`](super::Explorer::mark_shown)).
    pub shown_in: f64,
    pub back: Vec<String>,
    pub forward: Vec<String>,
    /// Text in the search box: filters what's shown; Enter searches subfolders by name (the find part).
    pub filter: String,
    pub find: Option<FindRun>,
    pub selection: Selection,
    /// The item being renamed (the rename part sets it; navigating to another folder clears it).
    pub renaming: Option<String>,
    /// A list with columns, or a grid of thumbnails.
    pub view: ViewMode,
    /// The sort in force here: the folder's own, else its kind's, else Settings' (resolved, not stored).
    pub sort: Sort,
    /// Group by in force (the list only; Settings → Files).
    pub group: GroupBy,
    /// The columns the list shows, in order (resolved like [`Tab::sort`]).
    pub columns: Vec<ColumnId>,
    /// What kind of folder this looks like (pictures, music, Downloads…), which decides its default columns.
    pub kind: FolderKind,
    /// The listing sorted, narrowed and grouped: what the file list draws.
    pub rows: Rows,

    /// What Git says of the folder's items, by lowercase name (`Explorer::set_git_marks`): the `git:` filter and
    /// Group by Git read it. `None` outside a repository.
    pub(super) git: Option<Arc<GitMarks>>,
    /// The folder those marks were given for: once the tab is in another folder they are not its marks.
    pub(super) git_for: String,
    pub(super) rows_spec: Option<RowSpec>,
    /// The entries `rows` were built from, to tell a new listing from a new sort.
    pub(super) rows_for: Option<Arc<[FileEntry]>>,
    pub(super) rows_seq: u64,
    pub(super) scroll: Option<ScrollMemo>,
    /// The newest navigation's number: a result that comes back with an older one is stale and dropped.
    pub(super) nav_seq: u64,
    /// When the navigation that is showing began, until `mark_shown` times it.
    pub(super) nav_started: Option<Instant>,
    /// Re-reading for a change on disk: the folder that changed once more meanwhile, if one did.
    pub(super) rereading: Option<Option<String>>,
    /// The folder was to be read again while a navigation was under way: it is, once that has landed, if
    /// the tab still shows it (see `Explorer::reload`).
    pub(super) reload_waiting: bool,
}

impl Tab {
    /// A tab that opens `path` when it's first shown, as `view`, with `select` selected (`blankTab`).
    pub fn blank(id: TabId, path: &str, view: ViewMode, select: Option<&str>) -> Tab {
        Tab {
            id,
            ready: false,
            path: path.to_string(),
            listing: None,
            load: LoadState::Idle,
            loading: false,
            shown_in: 0.0,
            back: Vec::new(),
            forward: Vec::new(),
            filter: String::new(),
            find: None,
            selection: Selection {
                paths: Arc::new(select.into_iter().collect()),
                anchor: None,
                focus: None,
            },
            renaming: None,
            view,
            sort: Sort {
                key: SortKey::Name,
                desc: false,
            },
            group: GroupBy::None,
            columns: Vec::new(),
            kind: FolderKind::General,
            rows: Rows::empty(),
            git: None,
            git_for: String::new(),
            rows_spec: None,
            rows_for: None,
            rows_seq: 0,
            scroll: None,
            nav_seq: 0,
            nav_started: None,
            rereading: None,
            reload_waiting: false,
        }
    }

    /// What Git says of the folder's items (lowercase name to mark), as the window last told the explorer.
    pub fn git_marks(&self) -> Option<&Arc<GitMarks>> {
        self.git.as_ref()
    }

    /// Whether `rows` were built from exactly these entries.
    pub(super) fn rows_source_is(&self, entries: &Arc<[FileEntry]>) -> bool {
        self.rows_for.as_ref().is_some_and(|r| Arc::ptr_eq(r, entries))
    }

    pub(super) fn set_rows_source(&mut self, entries: &Arc<[FileEntry]>) {
        self.rows_for = Some(entries.clone());
    }

    /// The entries of the folder shown (empty for This PC and before the first folder opens).
    pub fn entries(&self) -> &[FileEntry] {
        self.listing.as_ref().map_or(&[], |l| &l.entries[..])
    }

    /// The listing's entries, shared.
    pub fn shared_entries(&self) -> Option<Arc<[FileEntry]>> {
        self.listing.as_ref().map(|l| l.entries.clone())
    }

    /// The archive the folder is in, from its listing (`archiveOf`); `None` for a folder on disk.
    pub fn archive(&self) -> Option<&glint_model::entries::InArchive> {
        self.listing.as_ref().and_then(|l| l.archive.as_ref())
    }

    /// Whether the tab shows a remote connection's folder: read-only, its files downloaded to be read (`remoteOf`).
    pub fn is_remote(&self) -> bool {
        glint_model::paths::is_remote_path(&self.path)
    }

    /// Whether Windows keeps the folder closed to this account (`closedOf`).
    pub fn is_closed(&self) -> bool {
        matches!(self.load, LoadState::NoAccess(_))
    }

    /// Why the folder is closed, if it is.
    pub fn no_access(&self) -> Option<&NoAccess> {
        match &self.load {
            LoadState::NoAccess(no_access) => Some(no_access),
            _ => None,
        }
    }

    /// Whether the tab shows the Recycle Bin (`recycleBinOf`).
    pub fn is_recycle_bin(&self) -> bool {
        glint_model::paths::is_recycle_bin(&self.path)
    }

    /// Whether the tab is on This PC.
    pub fn is_this_pc(&self) -> bool {
        self.path == THIS_PC
    }

    /// What a tab shows: a folder, or a search's results. Scroll positions and thumbnail timings are kept by it
    /// for each view (`viewKey`).
    pub fn view_key(&self) -> String {
        match &self.find {
            Some(find) => format!("find:{}", find.id),
            None => format!("dir:{}", self.path),
        }
    }

    /// The folder on disk to watch for this tab: its own, or the one its archive is in (`watchedFolder`).
    pub fn watched_folder(&self) -> String {
        match self.archive() {
            Some(archive) => glint_model::paths::parent_path(&archive.outer),
            None => self.path.clone(),
        }
    }

    /// Where the list was scrolled the last time this folder or search was shown here (`savedScroll`); 0 for
    /// any other. `key` is [`Tab::view_key`].
    pub fn saved_scroll(&self, key: &str) -> f32 {
        match &self.scroll {
            Some(memo) if memo.key == key => memo.top,
            _ => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_set_keeps_the_order_selected_and_looks_up_fast() {
        let mut set = PathSet::default();
        assert!(set.insert("C:\\b"));
        assert!(set.insert("C:\\a"));
        assert!(!set.insert("C:\\b"), "already there");
        assert_eq!(set.iter().collect::<Vec<_>>(), ["C:\\b", "C:\\a"]);
        assert!(set.contains("C:\\a"));
        assert!(set.remove("C:\\b"));
        assert!(!set.remove("C:\\b"));
        assert_eq!(set.last(), Some("C:\\a"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn a_blank_tab_opens_with_the_asked_for_item_selected_and_no_history() {
        let tab = Tab::blank(TabId(3), "C:\\x", ViewMode::List, Some("C:\\x\\a.txt"));
        assert_eq!(tab.id.to_string(), "tab3");
        assert!(!tab.ready);
        assert!(tab.selection.contains("C:\\x\\a.txt"));
        assert!(tab.back.is_empty() && tab.forward.is_empty());
        assert_eq!(tab.view_key(), "dir:C:\\x");
        assert_eq!(tab.saved_scroll("dir:C:\\x"), 0.0);
    }
}
