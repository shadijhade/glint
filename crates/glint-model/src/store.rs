//! Ported from src/lib/store.ts.
//!
//! Glint's settings and saved state: their types, defaults, JSON shape and the pure update rules.
//!
//! The old app kept all of this in one zustand store, written to WebView2's localStorage under the key
//! [`STORAGE_KEY`]. The store itself (subscriptions, the delayed write, window listeners, toasts) is not
//! ported: the UI layer owns the live state. What is kept is everything that decides what ends up on disk
//! or is read back from it, so settings can be imported from the old app:
//!
//! - the types, with the same field names (`camelCase`) and values;
//! - the defaults ([`Settings::default`] equals `DEFAULT_SETTINGS`, [`AppState::default`] the store's
//!   initial state);
//! - the persisted shape: zustand's `{"state": {...}, "version": 0}` wrapper, every data field of the state
//!   (functions are dropped by `JSON.stringify`), and the merge that gives fields missing from an older
//!   file their defaults ([`load_persisted`], [`persisted_json`]);
//! - the store's update rules as pure functions on [`AppState`].
//!
//! Loading is tolerant: a file from an older or a newer Glint always loads. A value this version does not
//! understand (an unknown enum value, a field of the wrong type) costs only that field, which gets its
//! default; every other field is kept.

use crate::aware::Habits;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::marker::PhantomData;

/// The localStorage key the old app saved its state under.
pub const STORAGE_KEY: &str = "shadui-ripgrep-storage";

/// zustand's `persist` version: the store never set one, so it is 0, and it has no `migrate`.
pub const PERSIST_VERSION: i64 = 0;

/// The most folders whose own views are kept.
pub const MAX_FOLDER_VIEWS: usize = 500;

/// The most folders whose visits are kept.
pub const MAX_VISITS: usize = 300;

use crate::columns::ColumnId;
use crate::entries::{Sort, SortKey};
use crate::grid::TileSize;
use crate::groups::GroupBy;
use crate::marks::{default_labels, Label};
use crate::rank::{frecency, Visit};

/// A map from text to values that keeps the order keys were first added in, as a JS object does (for the
/// keys this state uses). The order shows in the saved JSON and decides which entries are forgotten first
/// among equals, so a `HashMap` or a sorted map would not do.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedMap<V> {
    entries: Vec<(String, V)>,
}

impl<V> Default for OrderedMap<V> {
    fn default() -> Self {
        OrderedMap { entries: Vec::new() }
    }
}

impl<V> OrderedMap<V> {
    /// An empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many entries there are.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The value under `key`.
    pub fn get(&self, key: &str) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Whether `key` is there.
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// Sets `key`: an existing key keeps its place, a new one goes last.
    pub fn insert(&mut self, key: String, value: V) {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Takes `key` out.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(at).1)
    }

    /// The entries, in order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// The keys, in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|(k, _)| k.as_str())
    }
}

impl<V: Serialize> Serialize for OrderedMap<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (k, v) in &self.entries {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for OrderedMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MapVisitor<V>(PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for MapVisitor<V> {
            type Value = OrderedMap<V>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut map = OrderedMap::new();
                while let Some((k, v)) = access.next_entry::<String, V>()? {
                    map.insert(k, v);
                }
                Ok(map)
            }
        }
        deserializer.deserialize_map(MapVisitor(PhantomData))
    }
}

/// A search made, kept for Home.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub query: String,
    pub path: String,
    /// Unix milliseconds.
    pub timestamp: i64,
}

/// The switches of a text search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// The text inside documents (PDF, Office, OpenDocument, EPUB) rather than files' own (doctext.rs).
    pub documents: bool,
    /// With documents: pictures' text too, read by Windows' OCR (ocr.rs).
    pub ocr: bool,
}

/// One of [`SearchOptions`]' switches, for [`AppState::set_option`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOptionKey {
    CaseSensitive,
    WholeWord,
    Regex,
    Documents,
    Ocr,
}

/// Which search the main box runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Files,
    Text,
}

/// How a tab shows a folder: a list with columns, or a grid of thumbnails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    List,
    Grid,
    Columns,
}

/// One pane of the session: its tabs' folders, the active one, and each tab's view.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionPane {
    pub paths: Vec<String>,
    pub active: usize,
    /// Each tab's view (`views` came later: missing means lists).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub views: Option<Vec<ViewMode>>,
}

/// The tabs open in each pane when the app closed, to reopen them.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub dual: bool,
    pub active_pane: usize,
    pub panes: Vec<SessionPane>,
}

/// Where a tab's list was scrolled: `key` names the folder or search it was for (`Tab::view_key`), `top` is how far.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct TabScroll {
    pub key: String,
    pub top: f32,
}

/// The most workspaces that are kept.
pub const MOST_WORKSPACES: usize = 40;

/// The longest name a workspace has, in characters.
pub const LONGEST_WORKSPACE_NAME: usize = 60;

/// A layout of the window kept under a name (`Work`, `Photos`, `Release`): the session's panes with their tabs,
/// folders and views, the split between two panes, and where each tab's list was scrolled. Built from the
/// [`Session`] the window already keeps between runs, so older saved states have none and load as before.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    /// Made when it is first saved ("ws-1"), kept when it is saved again or renamed.
    pub id: String,
    pub name: String,
    pub session: Session,
    /// Dual-pane view: the first pane's share of the width (`Settings::dual_split`; none is half).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dual_split: Option<f64>,
    /// Where each tab was scrolled: `scrolls[pane][tab]`, in the order of the session's panes and paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub scrolls: Vec<Vec<Option<TabScroll>>>,
}

/// A workspace's name as it is kept: the words with single spaces between them, at most
/// [`LONGEST_WORKSPACE_NAME`] characters.
pub fn clean_workspace_name(name: &str) -> String {
    let words = name.split_whitespace().collect::<Vec<_>>().join(" ");
    words
        .chars()
        .take(LONGEST_WORKSPACE_NAME)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The interface's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    System,
}

/// Windows 11's Mica behind the title bar and the sidebar, or a solid window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backdrop {
    Mica,
    Solid,
}

/// Glint's pink, or the accent color chosen in Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Glint,
    Windows,
}

/// The text size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontSize {
    Small,
    Medium,
    Large,
}

/// Where Glint opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StartFolder {
    Home,
    ThisPc,
    Recent,
    Last,
}

/// The list's columns whose width can be dragged (the name takes what's left).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Date,
    Type,
    Size,
    Taken,
    Dimensions,
    Duration,
    Artist,
    Album,
    Label,
    Tags,
    Comment,
}

/// Widths dragged in the list's header, in pixels; unset: the default (`Partial<Record<Column, number>>`).
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct ColumnWidths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artist: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<f64>,
}

/// Glint's settings. Fields are in the order `DEFAULT_SETTINGS` lists them, which is the order they are saved.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub backdrop: Backdrop,
    pub accent: Accent,
    pub font_size: FontSize,
    pub show_line_numbers: bool,
    pub default_search_path: String,
    pub max_results: Option<u64>,
    pub auto_open_preview: bool,
    pub confirm_before_replace: bool,
    /// Program used to open files; empty = system default.
    pub editor_path: String,
    pub exclusions: Vec<String>,
    pub save_search_history: bool,
    pub max_history_items: u32,
    // Files
    pub start_folder: StartFolder,
    /// Reopen last time's tabs and panes; `start_folder` when there are none.
    pub restore_tabs: bool,
    pub show_hidden: bool,
    pub folders_first: bool,
    /// Group by in the list: headers over each kind, date, size, type or Git status.
    pub group_by: GroupBy,
    pub show_preview_pane: bool,
    /// The sidebar's folder tree: drives, Quick access and pinned folders open to show their folders.
    pub folder_tree: bool,
    /// The tree opens down to the folder shown, as File Explorer's "Expand to open folder".
    pub tree_follows: bool,
    /// Each folder keeps its own columns, sort and view; off, every folder has `list_columns` and `sort`.
    pub per_folder_views: bool,
    pub list_columns: Vec<ColumnId>,
    /// Gallery: folders whose photos and videos it shows, besides Pictures and Videos.
    pub gallery_folders: Vec<String>,
    /// Copies and moves go through Glint's copy queue rather than Windows' copy dialog.
    pub copy_queue: bool,
    /// The queue reads each copy back to check it against its original.
    pub verify_copies: bool,
    /// Small thumbnails of pictures and videos in the list's icon column.
    pub list_thumbnails: bool,
    /// How big the grid's tiles are, in every tab.
    pub tile_size: TileSize,
    /// Ask before moving to the Recycle Bin (permanent deletes always ask).
    pub confirm_delete: bool,
    pub sort: Sort,
    // Find files (fd)
    pub find_respect_ignore: bool,
    pub find_max_results: Option<u64>,
    pub find_exclusions: Vec<String>,
    // Quick search: a shortcut from anywhere (None: off), the tray, and starting with Windows
    pub quick_shortcut: Option<String>,
    pub keep_in_tray: bool,
    pub start_with_windows: bool,
    // Compare panes: what's inside subfolders too, and files' contents
    pub compare_subfolders: bool,
    pub compare_contents: bool,
    /// Widths dragged in the list's header.
    pub columns: ColumnWidths,
    /// The preview pane's width in pixels (None: the default).
    pub preview_width: Option<f64>,
    /// Dual-pane view: the first pane's share of the width (None: half).
    pub dual_split: Option<f64>,
    /// Git repositories (their roots) trusted to run the programs their own settings name.
    pub git_trusted: Vec<String>,
    /// The terminal pane's shell ("" picks the first found: PowerShell 7, Windows PowerShell, cmd).
    pub terminal_shell: String,
    /// The terminal pane's height in pixels (None: the default).
    pub terminal_height: Option<f64>,
    /// Whether the terminal and the folder shown follow each other.
    pub terminal_follow: bool,
    /// The disk-usage map's Growth colors: what grew in the last this many days (1, 7 or 30).
    pub growth_days: u32,
    /// A folder's README.md above its list, and whether it's open (or folded to its title).
    pub show_readme: bool,
    pub readme_open: bool,
    /// Folder history shared with zoxide, when it's installed.
    pub zoxide: bool,
    /// Menus and the dock put forward the step that fits what is selected (docs/gpui/aware).
    pub suggest: bool,
    /// What the person chooses is counted, on this PC only, to offer the likely step first.
    pub learn: bool,
    /// Updates download in the background and install on quitting Glint or after a start with Windows (Settings →
    /// About); off, Glint only says a new version is out.
    pub auto_update: bool,
    /// Subtle tactile cues on Precision Touchpads (zoom detents, boundary bumps, File Basket snaps, checksum match).
    pub haptics: bool,
    /// The labels a file can carry, in their order (Settings → Files → Labels; docs/gpui/marks).
    pub labels: Vec<Label>,
    /// What XYplorer's `tag.dat` gains comes into Glint when Glint starts or comes to the front (docs/gpui/marks,
    /// F30). Off until chosen.
    pub follow_xyplorer: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let strings = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<String>>();
        Settings {
            theme: Theme::System,
            backdrop: Backdrop::Mica,
            accent: Accent::Glint,
            font_size: FontSize::Medium,
            show_line_numbers: true,
            default_search_path: String::new(),
            max_results: Some(10000),
            auto_open_preview: true,
            confirm_before_replace: true,
            editor_path: String::new(),
            exclusions: strings(&[
                // Folders
                "node_modules",
                ".git",
                "dist",
                "build",
                "bin",
                "obj",
                ".vs",
                ".idea",
                "__pycache__",
                ".cache",
                "vendor",
                "packages",
                ".nuget",
                "coverage",
                // Binary/Executable files
                "*.exe",
                "*.dll",
                "*.so",
                "*.dylib",
                "*.pdb",
                "*.lib",
                "*.a",
                "*.o",
                // Images
                "*.png",
                "*.jpg",
                "*.jpeg",
                "*.gif",
                "*.ico",
                "*.svg",
                "*.webp",
                "*.bmp",
                // Archives
                "*.zip",
                "*.rar",
                "*.7z",
                "*.tar",
                "*.gz",
                // Other binary formats
                "*.pdf",
                "*.woff",
                "*.woff2",
                "*.ttf",
                "*.eot",
                // Lock files
                "package-lock.json",
                "yarn.lock",
                "pnpm-lock.yaml",
            ]),
            save_search_history: true,
            max_history_items: 1000,
            start_folder: StartFolder::Home,
            restore_tabs: true,
            show_hidden: false,
            folders_first: true,
            group_by: GroupBy::None,
            show_preview_pane: true,
            folder_tree: true,
            tree_follows: false,
            per_folder_views: true,
            list_columns: vec![ColumnId::Modified, ColumnId::Type, ColumnId::Size],
            gallery_folders: Vec::new(),
            copy_queue: false,
            verify_copies: true,
            list_thumbnails: true,
            tile_size: TileSize::Medium,
            confirm_delete: false,
            sort: Sort {
                key: SortKey::Name,
                desc: false,
            },
            find_respect_ignore: false,
            find_max_results: Some(50000),
            find_exclusions: strings(&["node_modules", ".git"]),
            quick_shortcut: Some("Ctrl+Alt+F".to_string()),
            keep_in_tray: false,
            start_with_windows: false,
            compare_subfolders: false,
            compare_contents: false,
            columns: ColumnWidths::default(),
            preview_width: None,
            dual_split: None,
            git_trusted: Vec::new(),
            terminal_shell: String::new(),
            terminal_height: None,
            terminal_follow: true,
            growth_days: 7,
            show_readme: true,
            readme_open: false,
            zoxide: false,
            suggest: true,
            learn: true,
            auto_update: true,
            haptics: true,
            labels: default_labels(),
            follow_xyplorer: false,
        }
    }
}

/// What a remote connection talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteKind {
    S3,
    Webdav,
    Sftp,
    Rclone,
}

impl RemoteKind {
    /// The word it's saved as ("s3", "webdav", "sftp", "rclone"), which glint-core's connections take.
    pub fn as_str(self) -> &'static str {
        match self {
            RemoteKind::S3 => "s3",
            RemoteKind::Webdav => "webdav",
            RemoteKind::Sftp => "sftp",
            RemoteKind::Rclone => "rclone",
        }
    }
}

/// A remote connection (remote/), as its folders are reached at `remote:<id>`: no secret, which Windows
/// Credential Manager keeps. `id` is made from the first name it's given, and kept through renames.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConnection {
    pub id: String,
    pub name: String,
    pub kind: RemoteKind,
    /// S3: the service's address (Amazon's, from the region, when empty); WebDAV: the server's URL.
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// S3's access key ID; WebDAV's and SFTP's user name.
    pub user: String,
    /// SFTP: the server (a name ~/.ssh/config knows will do).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// SFTP: its port (22 when 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u32>,
    /// SFTP: a key file ("" for ssh's own choice).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// The folder on it to start at.
    pub root: String,
    /// Pictures in the grid get thumbnails, which downloads each one shown (up to 16 MB): off unless asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnails: Option<bool>,
    /// rclone: the remote's name in rclone's configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rclone: Option<String>,
    /// rclone: the remote's type there ("drive", "onedrive"...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
}

impl Default for RemoteConnection {
    fn default() -> Self {
        RemoteConnection {
            id: String::new(),
            name: String::new(),
            kind: RemoteKind::S3,
            endpoint: String::new(),
            region: String::new(),
            bucket: String::new(),
            user: String::new(),
            host: None,
            port: None,
            key: None,
            root: String::new(),
            thumbnails: None,
            rclone: None,
            service: None,
        }
    }
}

/// What a folder rule does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Move,
    Rename,
    Recycle,
    Unblock,
    Extract,
    Sync,
    /// Gives it a label (the rule's `target` names it).
    Label,
}

impl RuleAction {
    /// The word it's saved as ("move", "rename"…), which glint-core's rules take.
    pub fn as_str(self) -> &'static str {
        match self {
            RuleAction::Move => "move",
            RuleAction::Rename => "rename",
            RuleAction::Recycle => "recycle",
            RuleAction::Unblock => "unblock",
            RuleAction::Extract => "extract",
            RuleAction::Sync => "sync",
            RuleAction::Label => "label",
        }
    }
}

/// How a sync rule keeps two places alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// What's new or changed here goes there.
    Copy,
    /// There becomes what's here.
    Mirror,
    /// Both ways.
    Both,
}

impl SyncMode {
    /// The word it's saved as ("copy", "mirror", "both"), which glint-core's rules take.
    pub fn as_str(self) -> &'static str {
        match self {
            SyncMode::Copy => "copy",
            SyncMode::Mirror => "mirror",
            SyncMode::Both => "both",
        }
    }
}

/// A folder rule (rules.rs): what it looks at (a folder, its subfolders if asked), what it takes (names,
/// sizes, age), and what it does. Off until it's turned on, after a look at what it would do.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderRule {
    pub id: String,
    pub name: String,
    pub on: bool,
    pub folder: String,
    pub subfolders: bool,
    /// Patterns with * and ?, separated by ";" or "," ("*.pdf; invoice*"): empty for any name.
    pub names: String,
    /// Bytes (0: no bound).
    pub min_size: u64,
    pub max_size: u64,
    /// Last changed at least this many days ago (0: any).
    pub older_than_days: u32,
    pub action: RuleAction,
    /// Move: where to ({year}, {month}, {day} of the file's date may be in it). Sync: the place on a remote
    /// of rclone's ("gdrive:Backups"). Label: the label's name (one of Settings' labels).
    pub target: String,
    /// Rename: the new name, of {name}, {ext}, {date}, {time}, {year}, {month}, {day}.
    pub pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncMode>,
    /// Sync: also every this many minutes (0: when files change, and at start).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every: Option<u32>,
    /// Label: also replaces a label a file already has (off: a file with a label keeps it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

impl Default for FolderRule {
    fn default() -> Self {
        FolderRule {
            id: String::new(),
            name: String::new(),
            on: false,
            folder: String::new(),
            subfolders: false,
            names: String::new(),
            min_size: 0,
            max_size: 0,
            older_than_days: 0,
            action: RuleAction::Move,
            target: String::new(),
            pattern: String::new(),
            sync: None,
            every: None,
            replace: false,
        }
    }
}

/// A folder's own view, as last set there: its columns, its sort, list or grid, and when (the least lately
/// set are forgotten first).
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FolderView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<ColumnId>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<Sort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewMode>,
    /// Unix milliseconds.
    pub at: i64,
}

/// A change to a folder's view: fields that are `None` are left as they are (`Omit<Partial<FolderView>, 'at'>`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FolderViewPatch {
    pub columns: Option<Vec<ColumnId>>,
    pub sort: Option<Sort>,
    pub view: Option<ViewMode>,
}

/// A name search kept in the sidebar: what was typed, and the folder it searches (THIS_PC: every drive).
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearch {
    pub query: String,
    pub folder: String,
}

/// Everything the app keeps between runs: the store's data fields, in the order the store declares them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub query: String,
    pub path: String,
    pub history: Vec<HistoryItem>,
    pub options: SearchOptions,
    pub settings: Settings,
    pub mode: Mode,
    pub pinned: Vec<String>,
    pub saved_searches: Vec<SavedSearch>,
    pub remotes: Vec<RemoteConnection>,
    pub rules: Vec<FolderRule>,
    pub last_folder: Option<String>,
    pub session: Option<Session>,
    /// Layouts kept under a name, in the order they were first saved. Not written while empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<Workspace>,
    /// Folders opened, for ranking in the command palette.
    pub visits: OrderedMap<Visit>,
    /// Each folder's own view, by its path in lower case.
    pub folder_views: OrderedMap<FolderView>,
    /// The tags used lately, most recent first (the old app's `glint-recent-tags` in localStorage, which was not
    /// in the store; kept here so they outlive the session). Not written while empty, so a state without any
    /// saves exactly as the old app's did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_tags: Vec<String>,
    /// The notices already shown, by key and the value they were shown for (`firstNotice`'s localStorage keys:
    /// `offer-instant-search`, `service-update-notice`, `update-notice`, `quick-shortcut-taken`). Not written while
    /// empty.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub notices: BTreeMap<String, String>,
    /// What the person did before: the actions chosen for each kind of thing and where each kind of file went
    /// (docs/gpui/aware). Not written while empty.
    #[serde(skip_serializing_if = "Habits::is_empty")]
    pub habits: Habits,
}

impl Default for AppState {
    fn default() -> Self {
        AppState {
            query: String::new(),
            path: String::new(),
            history: Vec::new(),
            options: SearchOptions::default(),
            settings: Settings::default(),
            mode: Mode::Files,
            pinned: Vec::new(),
            saved_searches: Vec::new(),
            remotes: Vec::new(),
            rules: Vec::new(),
            last_folder: None,
            session: None,
            workspaces: Vec::new(),
            visits: OrderedMap::new(),
            folder_views: OrderedMap::new(),
            recent_tags: Vec::new(),
            notices: BTreeMap::new(),
            habits: Habits::default(),
        }
    }
}

/// Reads a saved object field by field, so one value this version does not understand (an unknown enum
/// value from a newer Glint, a field of the wrong type) costs only that field, which keeps its default,
/// and not the whole file. Missing fields keep their defaults too, and anything that isn't an object gives
/// the type's default. Each line is the JSON name and the Rust field it is read into.
macro_rules! lenient_deserialize {
    ($ty:ty { $($json:literal => $field:ident),* $(,)? }) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = serde_json::Value::deserialize(deserializer)?;
                let mut out = <$ty>::default();
                if let serde_json::Value::Object(mut map) = value {
                    $(
                        if let Some(raw) = map.remove($json) {
                            if let Ok(parsed) = serde_json::from_value(raw) {
                                out.$field = parsed;
                            }
                        }
                    )*
                }
                Ok(out)
            }
        }
    };
}

lenient_deserialize!(HistoryItem { "query" => query, "path" => path, "timestamp" => timestamp });
lenient_deserialize!(SearchOptions {
    "caseSensitive" => case_sensitive,
    "wholeWord" => whole_word,
    "regex" => regex,
    "documents" => documents,
    "ocr" => ocr,
});
lenient_deserialize!(SessionPane { "paths" => paths, "active" => active, "views" => views });
lenient_deserialize!(Session { "dual" => dual, "activePane" => active_pane, "panes" => panes });
lenient_deserialize!(TabScroll { "key" => key, "top" => top });
lenient_deserialize!(Workspace {
    "id" => id,
    "name" => name,
    "session" => session,
    "dualSplit" => dual_split,
    "scrolls" => scrolls,
});
lenient_deserialize!(ColumnWidths {
    "date" => date,
    "type" => r#type,
    "size" => size,
    "taken" => taken,
    "dimensions" => dimensions,
    "duration" => duration,
    "artist" => artist,
    "album" => album,
    "label" => label,
    "tags" => tags,
    "comment" => comment,
});
lenient_deserialize!(Settings {
    "theme" => theme,
    "backdrop" => backdrop,
    "accent" => accent,
    "fontSize" => font_size,
    "showLineNumbers" => show_line_numbers,
    "defaultSearchPath" => default_search_path,
    "maxResults" => max_results,
    "autoOpenPreview" => auto_open_preview,
    "confirmBeforeReplace" => confirm_before_replace,
    "editorPath" => editor_path,
    "exclusions" => exclusions,
    "saveSearchHistory" => save_search_history,
    "maxHistoryItems" => max_history_items,
    "startFolder" => start_folder,
    "restoreTabs" => restore_tabs,
    "showHidden" => show_hidden,
    "foldersFirst" => folders_first,
    "groupBy" => group_by,
    "showPreviewPane" => show_preview_pane,
    "folderTree" => folder_tree,
    "treeFollows" => tree_follows,
    "perFolderViews" => per_folder_views,
    "listColumns" => list_columns,
    "galleryFolders" => gallery_folders,
    "copyQueue" => copy_queue,
    "verifyCopies" => verify_copies,
    "listThumbnails" => list_thumbnails,
    "tileSize" => tile_size,
    "confirmDelete" => confirm_delete,
    "sort" => sort,
    "findRespectIgnore" => find_respect_ignore,
    "findMaxResults" => find_max_results,
    "findExclusions" => find_exclusions,
    "quickShortcut" => quick_shortcut,
    "keepInTray" => keep_in_tray,
    "startWithWindows" => start_with_windows,
    "compareSubfolders" => compare_subfolders,
    "compareContents" => compare_contents,
    "columns" => columns,
    "previewWidth" => preview_width,
    "dualSplit" => dual_split,
    "gitTrusted" => git_trusted,
    "terminalShell" => terminal_shell,
    "terminalHeight" => terminal_height,
    "terminalFollow" => terminal_follow,
    "growthDays" => growth_days,
    "showReadme" => show_readme,
    "readmeOpen" => readme_open,
    "zoxide" => zoxide,
    "suggest" => suggest,
    "learn" => learn,
    "autoUpdate" => auto_update,
    "haptics" => haptics,
    "labels" => labels,
    "followXyplorer" => follow_xyplorer,
});
/// Kinds of remote connection Glint no longer has: a paired PC's folders ("device"), gone with paired devices in
/// 0.12. A saved one loads as nothing ([`load_persisted`] drops it); any other kind it doesn't know loads as S3.
const RETIRED_REMOTE_KINDS: &[&str] = &["device"];

impl<'de> Deserialize<'de> for RemoteConnection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        let retired = value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| RETIRED_REMOTE_KINDS.contains(&kind));
        if retired {
            return Ok(RemoteConnection::default());
        }
        let mut out = RemoteConnection::default();
        if let serde_json::Value::Object(mut map) = value {
            macro_rules! field {
                ($($json:literal => $field:ident),*) => {$(
                    if let Some(raw) = map.remove($json) {
                        if let Ok(parsed) = serde_json::from_value(raw) {
                            out.$field = parsed;
                        }
                    }
                )*};
            }
            field!(
                "id" => id,
                "name" => name,
                "kind" => kind,
                "endpoint" => endpoint,
                "region" => region,
                "bucket" => bucket,
                "user" => user,
                "host" => host,
                "port" => port,
                "key" => key,
                "root" => root,
                "thumbnails" => thumbnails,
                "rclone" => rclone,
                "service" => service
            );
        }
        Ok(out)
    }
}
lenient_deserialize!(FolderRule {
    "id" => id,
    "name" => name,
    "on" => on,
    "folder" => folder,
    "subfolders" => subfolders,
    "names" => names,
    "minSize" => min_size,
    "maxSize" => max_size,
    "olderThanDays" => older_than_days,
    "action" => action,
    "target" => target,
    "pattern" => pattern,
    "sync" => sync,
    "every" => every,
    "replace" => replace,
});
lenient_deserialize!(FolderView { "columns" => columns, "sort" => sort, "view" => view, "at" => at });
lenient_deserialize!(SavedSearch { "query" => query, "folder" => folder });
lenient_deserialize!(AppState {
    "query" => query,
    "path" => path,
    "history" => history,
    "options" => options,
    "settings" => settings,
    "mode" => mode,
    "pinned" => pinned,
    "savedSearches" => saved_searches,
    "remotes" => remotes,
    "rules" => rules,
    "lastFolder" => last_folder,
    "session" => session,
    "workspaces" => workspaces,
    "visits" => visits,
    "folderViews" => folder_views,
    "recentTags" => recent_tags,
    "notices" => notices,
    "habits" => habits,
});

/// zustand's saved wrapper around the state.
#[derive(Serialize)]
struct PersistedOut<'a> {
    state: &'a AppState,
    version: i64,
}

/// The wrapper read back. A missing `version` is not 0, so the file is not taken (as zustand does).
#[derive(Deserialize)]
struct PersistedIn {
    #[serde(default)]
    state: Option<AppState>,
    #[serde(default)]
    version: Option<i64>,
}

/// The text the old app saved for `state`: `{"state":{...},"version":0}`.
pub fn persisted_json(state: &AppState) -> Result<String, serde_json::Error> {
    serde_json::to_string(&PersistedOut {
        state,
        version: PERSIST_VERSION,
    })
}

/// The state kept in `text` (a saved file of the old app), with defaults for what the file lacks. Text
/// that isn't a saved state, or one of another version (zustand would refuse it, having no `migrate`),
/// gives the defaults.
pub fn load_persisted(text: &str) -> AppState {
    parse_persisted(text).unwrap_or_default()
}

/// [`load_persisted`] that says whether `text` was a saved state of this version with a state in it: None for
/// anything else. One parse of the text, so a caller that must know which it was doesn't read it twice.
pub fn parse_persisted(text: &str) -> Option<AppState> {
    let PersistedIn {
        state: Some(mut state),
        version: Some(PERSIST_VERSION),
    } = serde_json::from_str::<PersistedIn>(text).ok()?
    else {
        return None;
    };
    // A connection of a retired kind loads with no id: it goes (see `RETIRED_REMOTE_KINDS`).
    state.remotes.retain(|remote| !remote.id.is_empty());
    // A workspace entry that wasn't one (damaged, or from a newer Glint) loads as an empty shell: it goes too.
    state.workspaces.retain(|w| !w.id.is_empty() && !w.name.is_empty());
    Some(state)
}

impl AppState {
    /// Whether `value` (a version, say) is new for the notice `key`, and remembers it: a notice shown once per
    /// value (`firstNotice` in utils.ts). `"offer-instant-search"` with `"1"` is once ever.
    pub fn first_notice(&mut self, key: &str, value: &str) -> bool {
        if self.notices.get(key).is_some_and(|seen| seen == value) {
            return false;
        }
        self.notices.insert(key.to_string(), value.to_string());
        true
    }

    /// Remembers a search at the top, once: an older one with the same text and folder moves up. Kept
    /// only if `settings.save_search_history`; `now` is Unix milliseconds.
    pub fn add_to_history(&mut self, query: &str, path: &str, now: i64) {
        if !self.settings.save_search_history {
            return;
        }
        let mut history = vec![HistoryItem {
            query: query.to_string(),
            path: path.to_string(),
            timestamp: now,
        }];
        history.extend(
            self.history
                .iter()
                .filter(|h| h.query != query || h.path != path)
                .cloned(),
        );
        // Never let a bad value wipe history: at least 10 are kept.
        let keep = (self.settings.max_history_items as usize).max(10);
        history.truncate(keep);
        self.history = history;
    }

    /// Forgets one search (a recent search's × button, or Delete on it).
    pub fn remove_from_history(&mut self, query: &str, path: &str) {
        self.history.retain(|h| h.query != query || h.path != path);
    }

    /// Sets one of the search switches.
    pub fn set_option(&mut self, key: SearchOptionKey, value: bool) {
        let o = &mut self.options;
        match key {
            SearchOptionKey::CaseSensitive => o.case_sensitive = value,
            SearchOptionKey::WholeWord => o.whole_word = value,
            SearchOptionKey::Regex => o.regex = value,
            SearchOptionKey::Documents => o.documents = value,
            SearchOptionKey::Ocr => o.ocr = value,
        }
    }

    /// Pins a folder, or unpins it if it's pinned.
    pub fn toggle_pin(&mut self, path: &str) {
        if self.pinned.iter().any(|p| p == path) {
            self.pinned.retain(|p| p != path);
        } else {
            self.pinned.push(path.to_string());
        }
    }

    /// The workspace with this id.
    pub fn workspace(&self, id: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id && !w.id.is_empty())
    }

    /// Keeps `layout` (its id and name are ignored) under `name`. A name another workspace has, in any case,
    /// replaces that workspace's layout and keeps its place, id and the name as it is now typed. Returns the id,
    /// or `None` for a name with nothing in it, a layout without a tab, or a new name when [`MOST_WORKSPACES`]
    /// are kept already.
    pub fn save_workspace(&mut self, name: &str, mut layout: Workspace) -> Option<String> {
        let name = clean_workspace_name(name);
        if name.is_empty() || layout.session.panes.iter().all(|p| p.paths.is_empty()) {
            return None;
        }
        layout.name = name.clone();
        // What an older or hand-edited file left without an id or a name is not a workspace.
        self.workspaces.retain(|w| !w.id.is_empty() && !w.name.is_empty());
        let lower = name.to_lowercase();
        if let Some(kept) = self.workspaces.iter_mut().find(|w| w.name.to_lowercase() == lower) {
            layout.id = kept.id.clone();
            *kept = layout;
            return Some(kept.id.clone());
        }
        if self.workspaces.len() >= MOST_WORKSPACES {
            return None;
        }
        let next = self
            .workspaces
            .iter()
            .filter_map(|w| w.id.strip_prefix("ws-").and_then(|n| n.parse::<u64>().ok()))
            .max()
            .map_or(1, |n| n + 1);
        layout.id = format!("ws-{next}");
        let id = layout.id.clone();
        self.workspaces.push(layout);
        Some(id)
    }

    /// Gives a workspace another name. `false` if there is no such workspace, the name has nothing in it, or
    /// another workspace has it (in any case); the workspace's own name may change case.
    pub fn rename_workspace(&mut self, id: &str, name: &str) -> bool {
        let name = clean_workspace_name(name);
        let lower = name.to_lowercase();
        if name.is_empty() || self.workspace(id).is_none() {
            return false;
        }
        if self
            .workspaces
            .iter()
            .any(|w| w.id != id && w.name.to_lowercase() == lower)
        {
            return false;
        }
        match self.workspaces.iter_mut().find(|w| w.id == id) {
            Some(w) => {
                w.name = name;
                true
            }
            None => false,
        }
    }

    /// Forgets a workspace. `false` if there was none with this id.
    pub fn delete_workspace(&mut self, id: &str) -> bool {
        let before = self.workspaces.len();
        self.workspaces.retain(|w| w.id != id);
        self.workspaces.len() != before
    }

    /// Keeps the search in the sidebar, or takes it out if it's there. The folder compares ignoring case.
    pub fn toggle_saved_search(&mut self, search: SavedSearch) {
        let folder = search.folder.to_lowercase();
        let same = |s: &SavedSearch| s.query == search.query && s.folder.to_lowercase() == folder;
        if self.saved_searches.iter().any(same) {
            self.saved_searches.retain(|s| !same(s));
        } else {
            self.saved_searches.push(search);
        }
    }

    /// Adds a connection, or changes the one with its id.
    pub fn save_remote(&mut self, remote: RemoteConnection) {
        match self.remotes.iter_mut().find(|r| r.id == remote.id) {
            Some(slot) => *slot = remote,
            None => self.remotes.push(remote),
        }
    }

    /// Removes a connection.
    pub fn remove_remote(&mut self, id: &str) {
        self.remotes.retain(|r| r.id != id);
    }

    /// Adds a rule, or changes the one with its id.
    pub fn save_rule(&mut self, rule: FolderRule) {
        match self.rules.iter_mut().find(|r| r.id == rule.id) {
            Some(slot) => *slot = rule,
            None => self.rules.push(rule),
        }
    }

    /// Removes a rule.
    pub fn remove_rule(&mut self, id: &str) {
        self.rules.retain(|r| r.id != id);
    }

    /// Changes a folder's own view (`None` for none: it's like other folders of its kind again). Past
    /// [`MAX_FOLDER_VIEWS`], the least lately set are forgotten. `now` is Unix milliseconds.
    pub fn set_folder_view(&mut self, path: &str, view: Option<FolderViewPatch>, now: i64) {
        let key = path.to_lowercase();
        match view {
            None => {
                self.folder_views.remove(&key);
            }
            Some(patch) => {
                let mut next = self.folder_views.get(&key).cloned().unwrap_or_default();
                if patch.columns.is_some() {
                    next.columns = patch.columns;
                }
                if patch.sort.is_some() {
                    next.sort = patch.sort;
                }
                if patch.view.is_some() {
                    next.view = patch.view;
                }
                next.at = now;
                self.folder_views.insert(key, next);
            }
        }
        let len = self.folder_views.len();
        if len > MAX_FOLDER_VIEWS {
            let mut keys: Vec<(String, i64)> = self.folder_views.iter().map(|(k, v)| (k.to_string(), v.at)).collect();
            keys.sort_by_key(|(_, at)| *at); // stable, like JS's sort
            for (old, _) in keys.into_iter().take(len - MAX_FOLDER_VIEWS) {
                self.folder_views.remove(&old);
            }
        }
    }

    /// Counts a visit to a folder. Past [`MAX_VISITS`], the least used are forgotten, as zoxide ages its
    /// database. `now` is Unix milliseconds.
    pub fn record_visit(&mut self, path: &str, now: i64) {
        let count = self.visits.get(path).map_or(0, |v| v.count).saturating_add(1);
        self.visits.insert(path.to_string(), Visit { count, last: now });
        let len = self.visits.len();
        if len > MAX_VISITS {
            let mut ranked: Vec<(String, f64)> = self
                .visits
                .iter()
                .map(|(k, v)| (k.to_string(), frecency(v, now)))
                .collect();
            ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            for (old, _) in ranked.into_iter().take(len - MAX_VISITS) {
                self.visits.remove(&old);
            }
        }
    }
}

// NOTE(port): not ported, on purpose: `useStore` (the live store: the UI layer owns the state), `setSettings`
// (a patch of 50 fields; the UI assigns the fields it changes), `setQuery`/`setPath`/`setMode`/
// `setLastFolder`/`setSession` (plain assignments to public fields), `SECONDARY_WINDOW`, `saveNow` and the
// delayed write (browser timers and localStorage), and `clearWithUndo` (a toast with Undo).

#[cfg(test)]
mod tests {
    use super::*;
    const HOUR: i64 = 3_600_000;

    /// glint-core takes a choice by the word it's saved as: the two must never drift apart.
    #[test]
    fn choices_are_handed_to_core_as_the_words_they_are_saved_as() {
        for kind in [RemoteKind::S3, RemoteKind::Webdav, RemoteKind::Sftp, RemoteKind::Rclone] {
            assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        }
        for action in [
            RuleAction::Move,
            RuleAction::Rename,
            RuleAction::Recycle,
            RuleAction::Unblock,
            RuleAction::Extract,
            RuleAction::Sync,
            RuleAction::Label,
        ] {
            assert_eq!(serde_json::to_value(action).unwrap(), action.as_str());
        }
        for sync in [SyncMode::Copy, SyncMode::Mirror, SyncMode::Both] {
            assert_eq!(serde_json::to_value(sync).unwrap(), sync.as_str());
        }
    }

    const DEFAULT_EXCLUSIONS: &str = concat!(
        r#"["node_modules",".git","dist","build","bin","obj",".vs",".idea","#,
        r#""__pycache__",".cache","vendor","packages",".nuget","coverage","#,
        r#""*.exe","*.dll","*.so","*.dylib","*.pdb","*.lib","*.a","*.o","#,
        r#""*.png","*.jpg","*.jpeg","*.gif","*.ico","*.svg","*.webp","*.bmp","#,
        r#""*.zip","*.rar","*.7z","*.tar","*.gz","#,
        r#""*.pdf","*.woff","*.woff2","*.ttf","*.eot","#,
        r#""package-lock.json","yarn.lock","pnpm-lock.yaml"]"#
    );

    /// What `JSON.stringify` of the old store's initial state gives, written out from store.ts.
    fn expected_default_json() -> String {
        let settings = format!(
            concat!(
                r#"{{"theme":"system","backdrop":"mica","accent":"glint","fontSize":"medium","#,
                r#""showLineNumbers":true,"defaultSearchPath":"","maxResults":10000,"autoOpenPreview":true,"#,
                r#""confirmBeforeReplace":true,"editorPath":"","exclusions":{},"saveSearchHistory":true,"#,
                r#""maxHistoryItems":1000,"startFolder":"home","restoreTabs":true,"showHidden":false,"#,
                r#""foldersFirst":true,"groupBy":"none","showPreviewPane":true,"folderTree":true,"#,
                r#""treeFollows":false,"perFolderViews":true,"listColumns":["modified","type","size"],"#,
                r#""galleryFolders":[],"copyQueue":false,"verifyCopies":true,"listThumbnails":true,"#,
                r#""tileSize":"medium","confirmDelete":false,"sort":{{"key":"name","desc":false}},"#,
                r#""findRespectIgnore":false,"findMaxResults":50000,"findExclusions":["node_modules",".git"],"#,
                r#""quickShortcut":"Ctrl+Alt+F","keepInTray":false,"startWithWindows":false,"#,
                r#""compareSubfolders":false,"compareContents":false,"columns":{{}},"previewWidth":null,"#,
                r#""dualSplit":null,"gitTrusted":[],"terminalShell":"","terminalHeight":null,"#,
                r#""terminalFollow":true,"growthDays":7,"showReadme":true,"readmeOpen":false,"#,
                r#""zoxide":false,"suggest":true,"learn":true,"autoUpdate":true,"haptics":true,"#,
                r#""labels":[{{"name":"Red","color":15680580}},{{"name":"Orange","color":16347926}},"#,
                r#"{{"name":"Yellow","color":15381256}},{{"name":"Green","color":2278750}},"#,
                r#"{{"name":"Blue","color":3900150}},{{"name":"Purple","color":11032055}},"#,
                r#"{{"name":"Gray","color":10265519}}],"followXyplorer":false}}"#
            ),
            DEFAULT_EXCLUSIONS
        );
        format!(
            concat!(
                r#"{{"state":{{"query":"","path":"","history":[],"#,
                r#""options":{{"caseSensitive":false,"wholeWord":false,"regex":false,"#,
                r#""documents":false,"ocr":false}},"settings":{},"mode":"files","pinned":[],"#,
                r#""savedSearches":[],"remotes":[],"rules":[],"lastFolder":null,"session":null,"visits":{{}},"#,
                r#""folderViews":{{}}}},"version":0}}"#
            ),
            settings
        )
    }

    #[test]
    fn default_state_serializes_to_what_the_old_app_saved() {
        let json = persisted_json(&AppState::default()).unwrap();
        assert_eq!(json, expected_default_json());
    }

    #[test]
    fn default_settings_values() {
        let s = Settings::default();
        assert_eq!(s.theme, Theme::System);
        assert_eq!(s.max_results, Some(10000));
        assert_eq!(s.max_history_items, 1000);
        assert_eq!(s.exclusions.len(), 14 + 8 + 8 + 5 + 5 + 3);
        assert_eq!(s.quick_shortcut.as_deref(), Some("Ctrl+Alt+F"));
        assert_eq!(
            s.sort,
            Sort {
                key: SortKey::Name,
                desc: false
            }
        );
        assert_eq!(s.tile_size, TileSize::Medium);
        assert_eq!(s.find_max_results, Some(50000));
        assert_eq!(s.growth_days, 7);
        assert_eq!(STORAGE_KEY, "shadui-ripgrep-storage");
    }

    #[test]
    fn notices_and_recent_tags_are_saved_and_loaded() {
        let mut state = AppState::default();
        assert!(state.first_notice("offer-instant-search", "1"));
        assert!(state.first_notice("service-update-notice", "0.9.5"));
        state.recent_tags = vec!["Work".into(), "Blue".into()];
        let json = persisted_json(&state).unwrap();
        assert!(json.contains(r#""recentTags":["Work","Blue"]"#), "{json}");
        assert!(
            json.contains(r#""notices":{"offer-instant-search":"1","service-update-notice":"0.9.5"}"#),
            "{json}"
        );
        let mut back = load_persisted(&json);
        assert_eq!(back, state);
        // Once ever, and once per value.
        assert!(!back.first_notice("offer-instant-search", "1"));
        assert!(!back.first_notice("service-update-notice", "0.9.5"));
        assert!(back.first_notice("service-update-notice", "0.9.6"));
        assert!(!back.first_notice("service-update-notice", "0.9.6"));
        // A wrong-typed value costs only that field.
        let odd = load_persisted(r#"{"state":{"recentTags":5,"notices":{"a":"b"}},"version":0}"#);
        assert!(odd.recent_tags.is_empty());
        assert_eq!(odd.notices.get("a").map(String::as_str), Some("b"));
    }

    #[test]
    fn saved_default_state_loads_back_equal() {
        let json = persisted_json(&AppState::default()).unwrap();
        assert_eq!(load_persisted(&json), AppState::default());
    }

    #[test]
    fn persisted_json_missing_fields_gets_defaults() {
        // An older file: few settings, no savedSearches, rules, remotes, folderViews or newer options.
        let text = r#"{"state":{"query":"abc","history":[{"query":"q","path":"C:\\","timestamp":5}],
            "options":{"regex":true},"settings":{"theme":"dark","showHidden":true,"maxResults":null},
            "pinned":["C:\\a"]},"version":0}"#;
        let s = load_persisted(text);
        assert_eq!(s.query, "abc");
        assert_eq!(
            s.history,
            vec![HistoryItem {
                query: "q".into(),
                path: "C:\\".into(),
                timestamp: 5
            }]
        );
        assert!(s.options.regex && !s.options.documents);
        assert_eq!(s.settings.theme, Theme::Dark);
        assert!(s.settings.show_hidden);
        assert_eq!(s.settings.max_results, None);
        assert_eq!(s.settings.max_history_items, 1000);
        assert_eq!(s.settings.exclusions, Settings::default().exclusions);
        assert_eq!(s.settings.quick_shortcut.as_deref(), Some("Ctrl+Alt+F"));
        assert_eq!(s.pinned, vec!["C:\\a".to_string()]);
        assert!(s.saved_searches.is_empty() && s.rules.is_empty() && s.visits.is_empty());
        assert_eq!(s.mode, Mode::Files);
    }

    #[test]
    fn persisted_json_with_a_state_of_nothing_or_another_version_is_the_defaults() {
        assert_eq!(load_persisted(r#"{"version":0}"#), AppState::default());
        assert_eq!(
            load_persisted(r#"{"state":{"query":"x"},"version":1}"#),
            AppState::default()
        );
        assert_eq!(load_persisted(r#"{"state":{"query":"x"}}"#), AppState::default());
        assert_eq!(load_persisted("not json"), AppState::default());
        assert_eq!(load_persisted(""), AppState::default());
        assert!(parse_persisted(r#"{"state":{"query":"x"},"version":0}"#).is_some_and(|s| s.query == "x"));
        for text in [
            r#"{"version":0}"#,
            r#"{"state":{"query":"x"},"version":1}"#,
            r#"{"state":{"query":"x"}}"#,
            r#"{"state":null,"version":0}"#,
            "not json",
            "",
        ] {
            assert!(parse_persisted(text).is_none(), "{text}");
        }
    }

    #[test]
    fn optional_fields_round_trip_and_stay_out_when_unset() {
        let remote = RemoteConnection {
            id: "r1".into(),
            kind: RemoteKind::Sftp,
            host: Some("box".into()),
            port: Some(0),
            ..Default::default()
        };
        let json = serde_json::to_string(&remote).unwrap();
        assert!(json.contains(r#""kind":"sftp""#) && json.contains(r#""host":"box""#) && !json.contains("thumbnails"));
        assert_eq!(serde_json::from_str::<RemoteConnection>(&json).unwrap(), remote);
        let rule = FolderRule {
            id: "x".into(),
            action: RuleAction::Sync,
            sync: Some(SyncMode::Both),
            ..Default::default()
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains(r#""olderThanDays":0"#) && json.contains(r#""sync":"both""#) && !json.contains("every"));
        assert_eq!(serde_json::from_str::<FolderRule>(&json).unwrap(), rule);
        let mut state = AppState::default();
        state.settings.columns.size = Some(120.5);
        state.settings.start_folder = StartFolder::ThisPc;
        state.session = Some(Session {
            dual: true,
            active_pane: 1,
            panes: vec![SessionPane {
                paths: vec!["C:\\".into()],
                active: 0,
                views: None,
            }],
        });
        let json = persisted_json(&state).unwrap();
        assert!(json.contains(r#""columns":{"size":120.5}"#) && json.contains(r#""startFolder":"thispc""#));
        assert!(json.contains(r#""session":{"dual":true,"activePane":1,"panes":[{"paths":["C:\\"],"active":0}]}"#));
        assert_eq!(load_persisted(&json), state);
    }

    #[test]
    fn a_label_rule_is_saved_as_label_and_replace_is_left_out_while_false() {
        let rule = FolderRule { id: "x".into(), action: RuleAction::Label, target: "Red".into(), ..Default::default() };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains(r#""action":"label""#) && !json.contains("replace"));
        assert_eq!(serde_json::from_str::<FolderRule>(&json).unwrap(), rule);
        let replacing = FolderRule { replace: true, ..rule };
        let json = serde_json::to_string(&replacing).unwrap();
        assert!(json.contains(r#""replace":true"#));
        assert_eq!(serde_json::from_str::<FolderRule>(&json).unwrap(), replacing);
    }

    #[test]
    fn a_rule_saved_by_0_16_1_reads_unchanged() {
        let old = r#"{"id":"r","name":"N","on":true,"folder":"C:\\D","subfolders":false,"names":"*.pdf","minSize":0,"maxSize":0,"olderThanDays":0,"action":"move","target":"D:\\X","pattern":""}"#;
        let rule: FolderRule = serde_json::from_str(old).unwrap();
        assert_eq!((rule.action, rule.replace, rule.target.as_str(), rule.on), (RuleAction::Move, false, "D:\\X", true));
        assert!(!serde_json::to_string(&rule).unwrap().contains("replace"));
    }

    #[test]
    fn history_moves_repeats_up_and_keeps_at_least_ten() {
        let mut s = AppState::default();
        s.add_to_history("a", "C:\\", 1);
        s.add_to_history("b", "C:\\", 2);
        s.add_to_history("a", "C:\\", 3);
        let order: Vec<_> = s.history.iter().map(|h| (h.query.as_str(), h.timestamp)).collect();
        assert_eq!(order, vec![("a", 3), ("b", 2)]);
        s.remove_from_history("b", "C:\\");
        assert_eq!(s.history.len(), 1);
        s.settings.max_history_items = 0;
        for i in 0..30 {
            s.add_to_history(&format!("q{i}"), "", i);
        }
        assert_eq!(s.history.len(), 10);
        assert_eq!(s.history[0].query, "q29");
        s.settings.save_search_history = false;
        s.add_to_history("never", "", 99);
        assert_eq!(s.history.len(), 10);
        assert_eq!(s.history[0].query, "q29");
    }

    #[test]
    fn search_options_switch_on_and_off() {
        let mut s = AppState::default();
        s.set_option(SearchOptionKey::Documents, true);
        assert!(s.options.documents);
        s.set_option(SearchOptionKey::Documents, false);
        assert!(!s.options.documents);
        s.set_option(SearchOptionKey::Regex, true);
        assert!(s.options.regex);
    }

    #[test]
    fn pins_saved_searches_remotes_and_rules_toggle_and_replace() {
        let mut s = AppState::default();
        s.toggle_pin("C:\\a");
        s.toggle_pin("C:\\b");
        s.toggle_pin("C:\\a");
        assert_eq!(s.pinned, vec!["C:\\b".to_string()]);
        let search = SavedSearch {
            query: "x".into(),
            folder: "C:\\Docs".into(),
        };
        s.toggle_saved_search(search);
        s.toggle_saved_search(SavedSearch {
            query: "x".into(),
            folder: "c:\\docs".into(),
        });
        assert!(s.saved_searches.is_empty());
        s.toggle_saved_search(SavedSearch {
            query: "x".into(),
            folder: "C:\\Docs".into(),
        });
        s.toggle_saved_search(SavedSearch {
            query: "y".into(),
            folder: "C:\\Docs".into(),
        });
        assert_eq!(s.saved_searches.len(), 2);
        s.save_remote(RemoteConnection {
            id: "a".into(),
            name: "one".into(),
            ..Default::default()
        });
        s.save_remote(RemoteConnection {
            id: "b".into(),
            ..Default::default()
        });
        s.save_remote(RemoteConnection {
            id: "a".into(),
            name: "renamed".into(),
            ..Default::default()
        });
        assert_eq!(s.remotes.len(), 2);
        assert_eq!(s.remotes[0].name, "renamed");
        s.remove_remote("a");
        assert_eq!(s.remotes.len(), 1);
        s.save_rule(FolderRule {
            id: "1".into(),
            ..Default::default()
        });
        s.save_rule(FolderRule {
            id: "1".into(),
            on: true,
            ..Default::default()
        });
        assert_eq!(s.rules.len(), 1);
        assert!(s.rules[0].on);
        s.remove_rule("1");
        assert!(s.rules.is_empty());
    }

    #[test]
    fn folder_views_merge_by_lower_case_path_and_forget_the_oldest() {
        let mut s = AppState::default();
        let grid = FolderViewPatch {
            view: Some(ViewMode::Grid),
            ..Default::default()
        };
        s.set_folder_view("C:\\Photos", Some(grid), 10);
        let sort = Sort {
            key: SortKey::Size,
            desc: true,
        };
        s.set_folder_view(
            "c:\\PHOTOS",
            Some(FolderViewPatch {
                sort: Some(sort),
                ..Default::default()
            }),
            20,
        );
        assert_eq!(s.folder_views.len(), 1);
        let v = s.folder_views.get("c:\\photos").unwrap();
        assert_eq!((v.view, v.sort, v.at), (Some(ViewMode::Grid), Some(sort), 20));
        s.set_folder_view("C:\\photos", None, 30);
        assert!(s.folder_views.is_empty());
        for i in 0..(MAX_FOLDER_VIEWS + 3) {
            s.set_folder_view(&format!("C:\\f{i}"), Some(FolderViewPatch::default()), i as i64);
        }
        assert_eq!(s.folder_views.len(), MAX_FOLDER_VIEWS);
        assert!(!s.folder_views.contains_key("c:\\f0") && !s.folder_views.contains_key("c:\\f2"));
        assert!(s.folder_views.contains_key("c:\\f3"));
    }

    #[test]
    fn visits_count_and_forget_the_least_used() {
        let mut s = AppState::default();
        s.record_visit("C:\\a", 1000);
        s.record_visit("C:\\a", 2000);
        assert_eq!(s.visits.get("C:\\a"), Some(&Visit { count: 2, last: 2000 }));
        for i in 0..(MAX_VISITS + 2) {
            s.record_visit(&format!("C:\\d{i}"), 3000);
        }
        assert_eq!(s.visits.len(), MAX_VISITS);
        // The twice-visited folder outranks the once-visited ones; ties go to the earliest added.
        assert!(s.visits.contains_key("C:\\a"));
        assert!(!s.visits.contains_key("C:\\d0") && !s.visits.contains_key("C:\\d2"));
        assert!(s.visits.contains_key("C:\\d3"));
    }

    #[test]
    fn frecency_weights_by_age() {
        let v = Visit { count: 2, last: 0 };
        assert_eq!(frecency(&v, 0), 8.0);
        assert_eq!(frecency(&v, HOUR), 4.0);
        assert_eq!(frecency(&v, 24 * HOUR), 1.0);
        assert_eq!(frecency(&v, 7 * 24 * HOUR), 0.5);
    }

    #[test]
    fn unknown_enum_values_and_wrong_types_cost_only_their_field() {
        let text = r#"{"state":{"query":"keep","pinned":["C:/a"],
            "settings":{"tileSize":"gigantic","theme":"dark","language":"fr","maxHistoryItems":"many",
                "listColumns":["modified","bogus"],"showHidden":true,"sort":{"key":"name","desc":true}},
            "mode":"video",
            "visits":{"C:/a":{"count":3},"C:/b":{"last":9},"C:/c":{"count":2,"last":7}}},"version":0}"#;
        let s = load_persisted(text);
        let d = Settings::default();
        assert_eq!(s.query, "keep");
        assert_eq!(s.pinned, vec!["C:/a".to_string()]);
        assert_eq!(s.settings.tile_size, TileSize::Medium);
        assert_eq!(s.settings.max_history_items, d.max_history_items);
        assert_eq!(s.settings.list_columns, d.list_columns);
        assert_eq!(s.settings.theme, Theme::Dark);
        assert!(s.settings.show_hidden);
        assert!(s.settings.sort.desc);
        assert_eq!(s.mode, Mode::Files);
        // A visit missing 'last' (or 'count') loads, and the entries around it are untouched.
        assert_eq!(s.visits.get("C:/a"), Some(&Visit { count: 3, last: 0 }));
        assert_eq!(s.visits.get("C:/b"), Some(&Visit { count: 0, last: 9 }));
        assert_eq!(s.visits.get("C:/c"), Some(&Visit { count: 2, last: 7 }));
        assert_eq!(s.visits.keys().collect::<Vec<_>>(), vec!["C:/a", "C:/b", "C:/c"]);
    }

    #[test]
    fn wrong_shaped_parts_load_as_defaults() {
        let text = r#"{"state":{"settings":"oops","options":[1],"history":[{"query":"q","timestamp":"x"}],
            "session":{"dual":true,"panes":[{"paths":["C:\\"],"views":["list","tiles"]}]},
            "remotes":[{"id":"r","kind":"ftp","port":"22"}]},"version":0}"#;
        let s = load_persisted(text);
        assert_eq!(s.settings, Settings::default());
        assert_eq!(s.options, SearchOptions::default());
        assert_eq!(s.history.len(), 1);
        assert_eq!((s.history[0].query.as_str(), s.history[0].timestamp), ("q", 0));
        let session = s.session.unwrap();
        assert!(session.dual);
        assert_eq!(session.panes[0].paths, vec!["C:\\".to_string()]);
        assert_eq!(session.panes[0].views, None);
        assert_eq!(s.remotes[0].id, "r");
        assert_eq!(s.remotes[0].kind, RemoteKind::S3);
        assert_eq!(s.remotes[0].port, None);
    }

    #[test]
    fn a_state_saved_with_what_0_12_removed_loads() {
        // 0.11 saved the documents search's By meaning, and remotes of paired PCs.
        let text = r#"{"state":{"options":{"documents":true,"ocr":true,"meaning":true,"regex":true},
            "settings":{"theme":"dark"},"pinned":["C:\\Work"],
            "remotes":[{"id":"laptop","name":"Laptop","kind":"device","host":"abc123"}]},"version":0}"#;
        let s = load_persisted(text);
        assert!(s.options.documents && s.options.ocr && s.options.regex);
        assert_eq!(s.settings.theme, Theme::Dark);
        assert_eq!(s.pinned, vec!["C:\\Work".to_string()]);
        assert!(s.remotes.is_empty());
    }

    #[test]
    fn a_paired_pcs_connection_saved_by_an_older_glint_goes() {
        let text = r#"{"state":{"remotes":[
            {"id":"laptop","name":"Laptop","kind":"device","host":"abc123"},
            {"id":"nas","name":"NAS","kind":"sftp","host":"nas.local"}]},"version":0}"#;
        let s = load_persisted(text);
        assert_eq!(s.remotes.len(), 1);
        assert_eq!((s.remotes[0].id.as_str(), s.remotes[0].kind), ("nas", RemoteKind::Sftp));
        assert_eq!(s.remotes[0].host.as_deref(), Some("nas.local"));
    }

    #[test]
    fn ordered_map_keeps_first_added_order_through_json() {
        let text = r#"{"z":{"count":1,"last":1},"a":{"count":2,"last":2}}"#;
        let m: OrderedMap<Visit> = serde_json::from_str(text).unwrap();
        assert_eq!(m.keys().collect::<Vec<_>>(), vec!["z", "a"]);
        assert_eq!(serde_json::to_string(&m).unwrap(), text);
    }

    // ---- Named workspaces --------------------------------------------------------------------------------

    fn layout(paths: &[&str], dual_split: Option<f64>) -> Workspace {
        Workspace {
            session: Session {
                dual: paths.len() > 1,
                active_pane: 0,
                panes: paths
                    .iter()
                    .map(|p| SessionPane {
                        paths: vec![(*p).to_string()],
                        active: 0,
                        views: Some(vec![ViewMode::Grid]),
                    })
                    .collect(),
            },
            dual_split,
            scrolls: vec![vec![Some(TabScroll {
                key: "dir:C:\\a".into(),
                top: 120.5,
            })]],
            ..Default::default()
        }
    }

    #[test]
    fn a_workspace_is_saved_under_a_name_and_the_same_name_replaces_it_in_place() {
        let mut s = AppState::default();
        let work = s.save_workspace("  Work  ", layout(&["C:\\a"], None)).unwrap();
        let photos = s
            .save_workspace("تقارير الصور", layout(&["C:\\p", "C:\\q"], Some(0.3)))
            .unwrap();
        assert_ne!(work, photos);
        assert_eq!(
            s.workspaces.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            ["Work", "تقارير الصور"]
        );
        // Saved again, in any case: the layout is replaced where it was, with the id it had.
        let again = s.save_workspace("WORK", layout(&["C:\\b"], Some(0.6))).unwrap();
        assert_eq!(again, work);
        assert_eq!(s.workspaces.len(), 2);
        let w = s.workspace(&work).unwrap();
        assert_eq!((w.name.as_str(), w.dual_split), ("WORK", Some(0.6)));
        assert_eq!(w.session.panes[0].paths, ["C:\\b"]);
        // Nothing to keep: no name, or no tab.
        assert!(s.save_workspace("   ", layout(&["C:\\a"], None)).is_none());
        assert!(s.save_workspace("Empty", Workspace::default()).is_none());
        assert_eq!(s.workspaces.len(), 2);
    }

    #[test]
    fn ids_are_not_reused_after_a_delete_and_there_is_a_most() {
        let mut s = AppState::default();
        let a = s.save_workspace("a", layout(&["C:\\a"], None)).unwrap();
        let b = s.save_workspace("b", layout(&["C:\\a"], None)).unwrap();
        assert!(s.delete_workspace(&a));
        assert!(!s.delete_workspace(&a));
        let c = s.save_workspace("c", layout(&["C:\\a"], None)).unwrap();
        assert!(c != a && c != b, "{a} {b} {c}");
        for n in s.workspaces.len()..MOST_WORKSPACES {
            assert!(s.save_workspace(&format!("w{n}"), layout(&["C:\\a"], None)).is_some());
        }
        assert!(s.save_workspace("one too many", layout(&["C:\\a"], None)).is_none());
        // An existing one can still be saved over when the list is full.
        assert!(s.save_workspace("b", layout(&["C:\\z"], None)).is_some());
    }

    #[test]
    fn a_workspace_is_renamed_unless_the_name_is_empty_or_taken() {
        let mut s = AppState::default();
        let work = s.save_workspace("Work", layout(&["C:\\a"], None)).unwrap();
        let home = s.save_workspace("Home", layout(&["C:\\h"], None)).unwrap();
        assert!(s.rename_workspace(&work, "  Office   work "));
        assert_eq!(s.workspace(&work).unwrap().name, "Office work");
        assert!(
            !s.rename_workspace(&work, "home"),
            "another workspace has it, in any case"
        );
        assert!(!s.rename_workspace(&work, "  "));
        assert!(!s.rename_workspace("ws-99", "Anything"));
        // Its own name in another case is a rename, not a clash.
        assert!(s.rename_workspace(&home, "HOME"));
        assert_eq!(s.workspace(&home).unwrap().name, "HOME");
        let long = "x".repeat(200);
        assert!(s.rename_workspace(&home, &long));
        assert_eq!(s.workspace(&home).unwrap().name.chars().count(), LONGEST_WORKSPACE_NAME);
    }

    #[test]
    fn workspaces_survive_a_save_and_a_load_with_their_split_views_and_scrolls() {
        let mut s = AppState::default();
        let id = s
            .save_workspace("Photos", layout(&["C:\\p", "C:\\q"], Some(0.35)))
            .unwrap();
        let loaded = load_persisted(&persisted_json(&s).unwrap());
        assert_eq!(loaded.workspaces, s.workspaces);
        let w = loaded.workspace(&id).unwrap();
        assert!(w.session.dual);
        assert_eq!(w.session.panes[1].views, Some(vec![ViewMode::Grid]));
        assert_eq!(w.dual_split, Some(0.35));
        assert_eq!(
            w.scrolls[0][0],
            Some(TabScroll {
                key: "dir:C:\\a".into(),
                top: 120.5
            })
        );
        // The JSON names are camelCase, like the rest of the file.
        assert!(persisted_json(&s).unwrap().contains(r#""dualSplit":0.35"#));
    }

    #[test]
    fn a_state_saved_before_workspaces_loads_and_saves_without_them() {
        let text = r#"{"state":{"pinned":["C:\\Work"],"session":{"dual":false,"panes":[{"paths":["C:\\"],"active":0}]}},"version":0}"#;
        let s = load_persisted(text);
        assert!(s.workspaces.is_empty());
        assert_eq!(s.pinned, ["C:\\Work"]);
        assert!(s.session.is_some());
        // Nothing is written for none, so an older Glint reading the file sees what it always saw.
        assert!(!persisted_json(&s).unwrap().contains("workspaces"));
    }

    #[test]
    fn a_damaged_workspace_costs_only_itself() {
        let text = r#"{"state":{"pinned":["C:\\Work"],"workspaces":[
            {"id":"ws-1","name":"Good","session":{"dual":false,"activePane":0,"panes":[{"paths":["C:\\a"],"active":0,"views":["grid"]}]}},
            {"id":"ws-2","name":"Odd","session":{"dual":"yes","panes":7},"dualSplit":"wide","scrolls":"none"},
            "not an object",
            {"name":"No id"}]},"version":0}"#;
        let s = load_persisted(text);
        assert_eq!(s.pinned, ["C:\\Work"]);
        let kept: Vec<_> = s.workspaces.iter().map(|w| w.name.as_str()).collect();
        assert!(kept.contains(&"Good") && kept.contains(&"Odd"), "{kept:?}");
        assert!(s.workspaces.iter().all(|w| !w.id.is_empty() && !w.name.is_empty()));
        assert_eq!(s.workspace("ws-1").unwrap().session.panes[0].paths, ["C:\\a"]);
        // A field it can't read keeps its default.
        let odd = s.workspace("ws-2").unwrap();
        assert_eq!(
            (odd.dual_split, odd.scrolls.len(), odd.session.panes.len()),
            (None, 0, 0)
        );
        // And a workspace in the list that isn't a real one is not offered a second time as a new id.
        let mut s = s;
        let next = s.save_workspace("New", layout(&["C:\\n"], None)).unwrap();
        assert_eq!(next, "ws-3");
    }

    #[test]
    fn workspace_names_are_single_spaced_and_not_too_long() {
        assert_eq!(clean_workspace_name("  a \t b\n c  "), "a b c");
        assert_eq!(clean_workspace_name(""), "");
        assert_eq!(
            clean_workspace_name(&"é".repeat(100)).chars().count(),
            LONGEST_WORKSPACE_NAME
        );
    }
}
