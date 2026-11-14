//! The explorer's plain types: ids, load states, drives, events and the notices views turn into toasts.
//! Ported from the types at the top of src/lib/explorer.ts (`KnownFolder`, `NoAccess`, `TabState`'s small
//! parts) and from src/lib/fs.ts (`Drive`, `driveName`, `ListFailure`).
//!
//! Nothing here touches GPUI or the disk, so it is all easy to test.

use glint_model::entries::{FileEntry, InArchive};
use glint_model::paths::{
    base_name, is_gallery, is_recent, is_recycle_bin, is_remote_path, is_root, is_shadow_path, remote_id_of, same_path,
    shadow_name, THIS_PC,
};
use std::fmt;
use std::sync::Arc;

pub use glint_model::store::ViewMode;

// ---- Ids ---------------------------------------------------------------------------------------------

/// A tab's identity. The TypeScript used the strings "tab1", "tab2"…; this is the number ([`fmt::Display`] still
/// prints "tab1"). Ids are never reused within one [`Explorer`](super::Explorer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

impl fmt::Display for TabId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tab{}", self.0)
    }
}

/// One side of the window: its tabs in order, and the one on screen (`PaneState`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub tabs: Vec<TabId>,
    pub active: TabId,
}

/// A folder handed to Glint from outside (File Explorer's "open with Glint", a double-click, the command line):
/// `select` is an item in it to select. Comes from the launch module (`platform::launch`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchTarget {
    pub path: String,
    pub select: Option<String>,
}

// ---- Folders and drives --------------------------------------------------------------------------------

/// Which well-known folder a [`KnownFolder`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KnownId {
    Home,
    Desktop,
    Documents,
    Downloads,
    Pictures,
    Music,
    Videos,
}

impl KnownId {
    /// Every known folder, in the sidebar's order.
    pub const ALL: [KnownId; 7] = [
        KnownId::Home,
        KnownId::Desktop,
        KnownId::Documents,
        KnownId::Downloads,
        KnownId::Pictures,
        KnownId::Music,
        KnownId::Videos,
    ];

    /// The name the sidebar shows, in English (the view translates it).
    pub fn name(self) -> &'static str {
        match self {
            KnownId::Home => "Home",
            KnownId::Desktop => "Desktop",
            KnownId::Documents => "Documents",
            KnownId::Downloads => "Downloads",
            KnownId::Pictures => "Pictures",
            KnownId::Music => "Music",
            KnownId::Videos => "Videos",
        }
    }
}

/// A folder Windows knows by name (Home, Desktop…), with its path (no trailing separator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownFolder {
    pub id: KnownId,
    pub name: &'static str,
    pub path: String,
}

/// What kind of volume a [`Drive`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Cdrom,
    Ram,
    Wsl,
    Unknown,
}

impl DriveKind {
    /// Core's word for it ("fixed", "removable"…); anything else is `Unknown`.
    pub fn parse(kind: &str) -> DriveKind {
        match kind {
            "fixed" => DriveKind::Fixed,
            "removable" => DriveKind::Removable,
            "network" => DriveKind::Network,
            "cdrom" => DriveKind::Cdrom,
            "ram" => DriveKind::Ram,
            "wsl" => DriveKind::Wsl,
            _ => DriveKind::Unknown,
        }
    }

    /// What a drive with no label is called, in English (`DEFAULT_LABEL` in fs.ts).
    fn default_label(self) -> &'static str {
        match self {
            DriveKind::Fixed => "Local Disk",
            DriveKind::Removable => "USB Drive",
            DriveKind::Network => "Network Drive",
            DriveKind::Cdrom => "DVD Drive",
            DriveKind::Ram => "RAM Disk",
            DriveKind::Wsl => "Linux",
            DriveKind::Unknown => "Drive",
        }
    }
}

/// A drive (or WSL distribution), as core's `list_drives` reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// Root, e.g. "C:\" or "\\wsl.localhost\Ubuntu".
    pub path: String,
    pub label: String,
    pub kind: DriveKind,
    pub fs: String,
    pub total: u64,
    pub free: u64,
}

impl From<glint_core::fsx::Drive> for Drive {
    fn from(drive: glint_core::fsx::Drive) -> Self {
        Drive {
            kind: DriveKind::parse(drive.kind),
            path: drive.path,
            label: drive.label,
            fs: drive.fs,
            total: drive.total,
            free: drive.free,
        }
    }
}

/// "Local Disk (C:)", like Explorer, or "Ubuntu (WSL)" for WSL2 distributions (`driveName`). English: the
/// view translates the default labels.
pub fn drive_name(drive: &Drive) -> String {
    let label = if drive.label.is_empty() {
        drive.kind.default_label()
    } else {
        drive.label.as_str()
    };
    if drive.kind == DriveKind::Wsl {
        format!("{label} (WSL)")
    } else {
        format!("{label} ({})", drive.path.strip_suffix('\\').unwrap_or(&drive.path))
    }
}

/// "This PC", "Recycle Bin", a drive's name, or the folder's (`tabName`). English; the view translates the
/// fixed names ("This PC", "Recycle Bin", "Recent", "Gallery"). `remote_name` says what a remote connection's
/// top is called (its connection's name, or its id for one that's gone: `remoteName` in remotes.ts).
pub fn tab_name(path: &str, drives: &[Drive], remote_name: &dyn Fn(&str) -> String) -> String {
    if path == THIS_PC {
        return "This PC".to_string();
    }
    if is_remote_path(path) && is_root(path) {
        return remote_name(path);
    }
    if is_recycle_bin(path) {
        return "Recycle Bin".to_string();
    }
    if is_recent(path) {
        return "Recent".to_string();
    }
    if is_gallery(path) {
        return "Gallery".to_string();
    }
    if is_shadow_path(path) && is_root(path) {
        return shadow_name(path);
    }
    if !is_root(path) {
        return base_name(path).to_string();
    }
    match drives.iter().find(|d| same_path(&d.path, path)) {
        Some(drive) => drive_name(drive),
        None => path.to_string(),
    }
}

/// The id of a remote connection's path, for [`tab_name`]'s fallback (`remoteIdOf`).
pub fn remote_id(path: &str) -> String {
    remote_id_of(path).to_string()
}

// ---- Listings --------------------------------------------------------------------------------------------

/// Why a folder couldn't be listed (`ListFailure` in fs.ts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListFailure {
    pub kind: FailureKind,
    pub message: String,
    /// The folder as asked for, made absolute.
    pub path: String,
    /// On another computer, which decides who may open it.
    pub remote: bool,
}

impl ListFailure {
    /// A failure that is only words.
    pub fn other(path: &str, message: impl Into<String>) -> Self {
        ListFailure {
            kind: FailureKind::Other,
            message: message.into(),
            path: path.to_string(),
            remote: false,
        }
    }
}

/// "denied": Windows doesn't let this account open the folder; anything else is said in the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Denied,
    Other,
}

/// A folder's contents as the engine read them, decoded: what a tab shows (`Listing` in entries.ts, with the
/// entries shared rather than copied, since views, rows and prefetches all hold them).
#[derive(Clone, Debug)]
pub struct Listed {
    pub path: String,
    /// In the order the engine sent them: folders first, natural name order ("presorted").
    pub entries: Arc<[FileEntry]>,
    /// Milliseconds the engine spent enumerating and sorting (or reading the archive's list).
    pub elapsed: f64,
    /// Set for a folder in an archive (read-only).
    pub archive: Option<InArchive>,
    /// Listed from instant search's index: Windows keeps the folder closed to this account.
    pub from_index: bool,
    /// A folder on a remote connection (read-only, downloaded to be read).
    pub remote: bool,
}

impl Listed {
    /// Which engine produced it.
    pub fn engine(&self) -> Engine {
        if let Some(archive) = &self.archive {
            Engine::Archive(archive.format)
        } else if self.from_index {
            Engine::Index
        } else if self.remote {
            Engine::Remote
        } else if is_recycle_bin(&self.path) || is_recent(&self.path) || is_gallery(&self.path) {
            Engine::Place
        } else {
            Engine::Disk
        }
    }
}

impl From<glint_model::entries::Listing> for Listed {
    fn from(listing: glint_model::entries::Listing) -> Self {
        Listed {
            path: listing.path,
            entries: Arc::from(listing.entries),
            elapsed: listing.elapsed,
            archive: listing.archive,
            from_index: listing.from_index,
            remote: listing.remote,
        }
    }
}

/// Who answered for a folder: shown in the status bar beside the time it took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Win32's enumeration of a folder on disk (or on a share).
    Disk,
    /// An archive's own list of entries; the format's label ("zip", "7z"…).
    Archive(&'static str),
    /// Instant search's index (a folder Windows keeps closed to this account).
    Index,
    /// A remote connection.
    Remote,
    /// The Recycle Bin, Recent or the Gallery.
    Place,
    /// This PC: no listing, the drives are shown.
    ThisPc,
}

/// A folder Windows doesn't let this account open: its tab says why, and what can be done about it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoAccess {
    /// On another computer, which decides who may open it: File Explorer can ask for another user name and password.
    pub remote: bool,
    /// Get access was asked for, and even administrators may not change who opens it.
    pub refused: bool,
    /// Get access gave this account permission, yet a rule in the folder's permissions denies it.
    pub still_denied: bool,
}

/// How much of a tab's folder is there to show.
#[derive(Clone, Debug, PartialEq)]
pub enum LoadState {
    /// Never listed: a tab that hasn't been shown yet (a restored tab opens its folder when first shown).
    Idle,
    /// The first listing is being read. (A tab that already shows a folder keeps showing it while the next
    /// one is read; [`Tab::loading`](super::Tab::loading) says so.)
    Loading,
    /// The first listing failed, with the reason. (When a tab already shows a folder, a failed navigation
    /// leaves it as it was and raises [`Notice::CannotOpen`] instead.)
    Failed(String),
    /// Windows keeps the folder closed to this account. The tab may still list what the index has in it.
    NoAccess(NoAccess),
    /// Listed: how long the engine took, and which one it was.
    Ready(Loaded),
}

/// What a finished listing tells about itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loaded {
    /// Milliseconds the engine spent reading the folder (`Listing.elapsed`).
    pub read_ms: f64,
    pub engine: Engine,
}

/// A name search's run, in a tab (`FindRun` in explorer.ts): while it exists the tab shows its `entries`
/// instead of the folder's listing, and navigating away drops it. The methods that make and end it are in
/// `find.rs` ([`Explorer::run_find`](super::Explorer::run_find) and the rest).
#[derive(Clone, Debug, Default)]
pub struct FindRun {
    pub id: u64,
    pub query: String,
    /// Who answers ("index", "fd", "index + fd"); empty until known.
    pub engine: String,
    /// Folder searched (with its subfolders); This PC searches every drive.
    pub scope: String,
    pub entries: Arc<[FileEntry]>,
    pub running: bool,
    pub elapsed: f64,
    pub limited: bool,
    /// Nothing matched as typed: these are close matches.
    pub fuzzy: bool,
    pub error: String,
    /// What it didn't look in.
    pub skipped: Vec<String>,
    /// What the same words found inside files (`inside.rs`): their files are among `entries` too.
    pub inside: Inside,
}

/// One line that matched inside a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The line's number, from 1 (a document's passage has none).
    pub line: Option<u64>,
    /// A document's match: where it is ("p. 3", "slide 2").
    pub label: Option<String>,
    /// The line, trimmed for one row, in plain and matching parts.
    pub parts: Vec<glint_model::text::Segment>,
}

/// Every file's matching lines, by path.
pub type Hits = std::collections::HashMap<String, Arc<[Hit]>>;

/// What a search found inside files (`inside.rs`). The files whose lines matched are rows like any other result:
/// the ones the name search didn't list are added to [`FindRun::entries`] and named in `only`.
#[derive(Clone, Debug, Default)]
pub struct Inside {
    /// The words looked for inside files ("" when the search has none: only filters, a path, a wildcard).
    pub words: String,
    /// The files found by what is in them and not by their names.
    pub only: Arc<std::collections::HashSet<String>>,
    pub hits: Arc<Hits>,
    /// How many lines matched in all.
    pub matches: usize,
    pub running: bool,
    /// It stopped at the result limit.
    pub limited: bool,
    pub error: String,
    pub elapsed: f64,
    /// The contents weren't searched because the place is big (a whole drive): the results' bar offers to.
    pub offered: bool,
}

impl Inside {
    /// How many files have matching lines.
    pub fn files(&self) -> usize {
        self.hits.len()
    }
}

// ---- Options ------------------------------------------------------------------------------------------------

/// How to open a folder (`NavOptions`).
#[derive(Clone, Debug, Default)]
pub struct NavOptions {
    /// Paths to select once the folder is open.
    pub select: Option<Vec<String>>,
    /// Don't record in Back/Forward history (the TypeScript's `history: false`).
    pub no_history: bool,
    /// Keep the current list on screen while loading, as a refresh does.
    pub quiet: bool,
    /// Don't report failures; the caller handles them.
    pub silent: bool,
    /// The tab to open it in (default: the active one).
    pub tab: Option<TabId>,
}

impl NavOptions {
    /// Options for opening in `tab`.
    pub fn in_tab(tab: TabId) -> Self {
        NavOptions {
            tab: Some(tab),
            ..Default::default()
        }
    }

    /// Selecting one item.
    pub fn selecting(mut self, path: impl Into<String>) -> Self {
        self.select = Some(vec![path.into()]);
        self
    }
}

// ---- What the model tells the views ----------------------------------------------------------------------

/// Things a view should say to the person (the TypeScript called `toast` from inside the store). The model
/// never touches the interface: it raises one of these, and the workspace (or whichever view cares) turns it
/// into a notification with `t!`, using the same words the toasts had (the comments give them).
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// `Can't open “{name}”` with the message as description. A navigation failed.
    CannotOpen { path: String, message: String },
    /// An archive whose list of files is encrypted: ask for its password (`askArchivePassword`), then
    /// navigate to `path` again, selecting `select`, in `tab`.
    ArchivePassword {
        path: String,
        tab: TabId,
        select: Option<Vec<String>>,
        message: String,
    },
    /// `“{name}” opens with 7-Zip`: offers to install it, then to open `path` again.
    NeedsSevenZip {
        path: String,
        tab: TabId,
        select: Option<Vec<String>>,
    },
    /// `“{name}” is no longer available`: a refresh found the folder gone; the tab went to its parent.
    NoLongerAvailable { path: String },
    /// `Windows keeps “{name}” closed`, with Get access where it helps (`closedNotice`).
    Closed {
        path: String,
        tab: TabId,
        from_index: bool,
        can_get_access: bool,
    },
    /// `In the Recycle Bin`: restore an item to use it again (`recycledNotice`).
    InRecycleBin,
    /// `Remote connections are read-only here` / `Archives are read-only` (`readOnlyNotice`).
    ReadOnly { remote: bool },
    /// `Couldn't open “{name}”` for a file that wouldn't open.
    CouldNotOpenFile { path: String, message: String },
    /// `Couldn't get access to “{name}”` (not shown for "cancelled").
    CouldNotGetAccess { path: String, message: String },
    /// A path typed in the address bar didn't resolve: the message is shown as it is.
    Message(String),
}

/// Work this part hands to the parts that own it (archives, disk images, previous versions). The view that
/// owns the matching feature listens for [`ExplorerEvent::Handoff`] and does it.
#[derive(Clone, Debug, PartialEq)]
pub enum Handoff {
    /// A previous version of a file (a shadow copy): opens as a read-only copy (`openVersionFile`).
    VersionFile(String),
    /// A disc or disk image (.iso, .vhd…): mounts as a drive when that's Windows' default, else opens in its
    /// app (`openDiskImage`).
    DiskImage(String),
    /// A file inside an archive: extracted to a temporary place, then opened (`openFromArchive`).
    ArchivedFile(Box<FileEntry>),
}

/// What changed in the [`Explorer`](super::Explorer), for views that need more than "something did"
/// (`cx.observe` covers that). Subscribe with `cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| ...)`.
#[derive(Clone, Debug, PartialEq)]
pub enum ExplorerEvent {
    /// Tabs or panes were added, removed, moved or reordered, or another tab came on screen, or dual-pane view
    /// was switched.
    TabsChanged,
    /// Another pane has the keyboard.
    PaneFocused,
    /// Something about one tab changed (its path, history, load state, view mode, filter…).
    TabChanged(TabId),
    /// A tab's folder was listed (or listed again): `same` when it is the same folder re-read in place.
    ListingChanged { tab: TabId, same: bool },
    /// A tab's rows (sorted, filtered, grouped) were rebuilt: a new listing, a new sort, a setting.
    RowsChanged(TabId),
    /// A tab's selection, focus or anchor changed.
    SelectionChanged(TabId),
    /// A tab's sort, group, columns or view mode changed.
    ViewChanged(TabId),
    /// The drives were read again.
    DrivesChanged,
    /// The known folders (Home, Desktop…) were found.
    KnownFoldersChanged,
    /// The set of briefly highlighted paths changed.
    FlashChanged,
    /// A tab closed. Its running search was stopped (`find_running`); the sizes part stops what it was
    /// measuring for the tab (`forget` in explorer.ts).
    TabClosed { tab: TabId, find_running: bool },
    /// A tab's search was dropped because the tab moved to another folder (stopped too if it was running).
    FindDropped { tab: TabId, was_running: bool },
    /// The person should be told something.
    Notice(Notice),
    /// Searching has something to say that is not about one file (see [`FindNotice`](super::FindNotice)).
    Find(super::FindNotice),
    /// Work for another part (see [`Handoff`]).
    Handoff(Handoff),
    /// Something the app keeps between runs changed: the last folder, the session, the visits or a folder's
    /// own view. The persistence module saves [`Explorer::memory`](super::Explorer::memory).
    MemoryChanged,

    // ---- File operations (ops_types.rs says what each carries) ----
    /// What a file operation has to tell the person: done (with Undo offered), failed, nothing to do.
    Operation(super::OpNotice),
    /// Slow work started showing or ended: the "Copying 3 items…" toast and the taskbar button.
    Busy(super::BusyEvent),
    /// A question to ask before acting; the answer calls a method (see [`Ask`](super::Ask)).
    Ask(super::Ask),
    /// Work on files in an archive or on a remote connection, for the archive part: hand it to
    /// [`Explorer::run_archive_job`](super::Explorer::run_archive_job).
    FromArchive(super::ArchiveJob),
    /// What archive work tells the person: progress and Stop while it runs, what it made or couldn't do (see
    /// [`ArchiveNotice`](super::ArchiveNotice)).
    Archive(super::ArchiveNotice),
    /// F2 on several items: the batch rename dialog opens with these paths (batch rename part).
    BatchRename(Vec<String>),
    /// The items cut to the clipboard changed (they show dimmed until pasted): [`Explorer::cut_paths`](super::Explorer::cut_paths).
    CutChanged,
    /// What Ctrl+Z would take back changed: [`Explorer::next_undo`](super::Explorer::next_undo).
    UndoChanged,
    /// A copy or move went to the copy queue: show its panel.
    QueueOpened,
    /// The New menu's kinds were read: [`Explorer::new_kinds`](super::Explorer::new_kinds).
    NewKindsChanged,

    // ---- Menu actions (actions_types.rs says what each carries) ----
    /// What a menu action has to tell the person: done, failed, or what it needs next.
    Action(super::ActionNotice),
    /// The ncdu export's progress, Stop, and result.
    Export(super::ExportNotice),
    /// Work for the workspace: a new window, a script in the terminal, a tool to install.
    Request(super::Request),
    /// A folder's sizes were measured, started, stopped or came with an archive's listing
    /// ([`Explorer::size_run`](super::Explorer::size_run)). The list, grid, status bar and preview redraw.
    SizesChanged { path: String },
    /// The remotes shown in File Explorer on demand changed ([`Explorer::on_demand`](super::Explorer::on_demand)).
    OnDemandChanged,
}
