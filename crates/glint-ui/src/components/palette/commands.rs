//! What Ctrl+K lists and how it ranks it: the commands, the folders you use, the calculator's answers.
//!
//! Ported from src/components/explorer/CommandPalette.tsx (ranking, the calculator's rows, "Go to") and from
//! `paletteCommands` in Explorer.tsx (which commands and folders there are, and when).
//!
//! Everything here is plain data and plain functions, so the ranking and the lists are tested without a
//! window. A [`Command`] says what it does as a [`Run`] value (not a closure), which the view carries out.

use crate::actions::{self, Binding};
use crate::components::menus::kinds::*;
use crate::components::menus::{CompressFormat, MediaTool, MenuEvent, PdfTool};
use crate::explorer::{Drive, KnownId, TabId};
use crate::kit::prelude::*;
use crate::{i18n, t};
use glint_core::openers::Opener;
use glint_core::projects::TaskGroup;
use glint_core::scripts::Script;
use glint_model::entries::{is_archive_file, is_closed, FileEntry};
use glint_model::format::{format_count, format_size, plural};
use glint_model::grid::{zoom_tiles, TileSize, TILE_SIZES};
use glint_model::groups::{GroupBy, GROUP_BYS};
use glint_model::paths::{
    base_name, is_folder_path, is_gallery, is_recent, is_recycle_bin, is_remote_path, parent_path, GALLERY, RECENT,
    RECYCLE_BIN, THIS_PC,
};
use glint_model::rank::{frecency, fuzzy_indices, fuzzy_score, Visit};
use glint_model::store::{RemoteConnection, RemoteKind, SavedSearch, Theme};
use std::collections::HashSet;
use std::ops::Range;

/// How many folders the list shows before anything is typed.
pub const FOLDERS_WHEN_EMPTY: usize = 6;
/// How many matches a query lists at most.
pub const MOST_MATCHES: usize = 40;
/// How many of the folders used most are offered (`usedFolders(visits, 60)`).
const USED_FOLDERS: usize = 60;

/// Whether a result takes you somewhere or does something. The list groups by it when nothing is typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A place: "Go to".
    Folder,
    /// A command: "Commands".
    Action,
}

/// Makes the GPUI action a command dispatches (the same makers `actions::BINDINGS` holds).
pub type ActionMaker = fn() -> Box<dyn Action>;

/// A change that has no action of its own.
#[derive(Clone, Debug)]
pub enum Change {
    /// Close every tab of the pane but this one.
    CloseOthers(TabId),
    /// A second tab of this folder.
    Duplicate(TabId),
    /// Show the folder in the other pane (opening the second pane first).
    SideBySide(String),
    /// Pin the folder to the sidebar, or take it off.
    TogglePin(String),
    /// Group the list by this.
    Group(GroupBy),
    /// Tiles of this size, in the grid.
    Tiles(TileSize),
    /// Dark or light theme.
    Theme(Theme),
    /// Every pane the same width again (`setSettings({ dualSplit: null })`).
    EvenPanes,
    /// What isn't selected, and only that (`invertSelection`).
    InvertSelection,
    /// Get access to the tab's closed folder (`getAccess`).
    GetAccess(TabId),
    /// The tab goes to the other pane (`moveTabToOtherPane`).
    MoveTab(TabId),
}

/// Which part of the Send, receive or share dialog a row opens (`showShare`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShareView {
    /// What is being sent to other PCs: the tickets, and Stop.
    Send,
    /// What is shared on this network: the links, and Stop.
    Network,
}

/// What choosing a result does. The view carries it out after closing the palette.
#[derive(Clone, Debug)]
pub enum Run {
    /// Nothing (the calculator's hint, an answer that failed).
    Nothing,
    /// Open this folder in the active tab.
    Navigate(String),
    /// Open what was typed (a path, "~", "%APPDATA%"…) in the active tab.
    GoTo(String),
    /// Put this text on the clipboard and say so.
    Copy(String),
    /// Run an action, as its key would.
    Action(ActionMaker),
    /// Something with no action.
    Change(Change),
    /// What a menu item does: handed to the main window as if it were chosen from a menu
    /// ([`crate::workspace::run_menu_command`]).
    Menu(MenuEvent),
    /// A saved search: its folder opens in the active tab with its query searched (`runSavedSearch`).
    Saved(SavedSearch),
    /// A project's task, typed into the terminal pane in the project's folder (`runTask`, through
    /// [`crate::workspace::run_task`]).
    Task {
        /// The project's folder.
        folder: String,
        /// The command line.
        command: String,
    },
    /// A folder opened in an editor or a terminal found on this PC (`openIn`).
    OpenIn {
        /// The opener's id.
        opener: String,
        /// Its name, for the message when it fails.
        name: String,
        /// The folder.
        path: String,
    },
    /// A terminal in this folder (`terminal`).
    Terminal(String),
    /// File Explorer's own window on this folder (`openPath`).
    FileExplorer(String),
    /// The dialog that adds a remote connection (`addRemote`).
    AddRemote,
    /// Folder rules, for one folder or none (`openRules`).
    Rules(Option<String>),
    /// Windows' own Properties dialog for this path (`properties`).
    Properties(String),
    /// The Send, receive or share dialog on one of its parts (`showShare`).
    Share(ShareView),
}

/// One line of the list.
#[derive(Clone, Debug)]
pub struct Command {
    /// Keys the list's rows (`folder:<lower-case path>`, `new-tab`…).
    pub id: String,
    /// The name; the part of it that matches what was typed is highlighted.
    pub label: SharedString,
    /// Shown dimmed after the label: a folder's path.
    pub detail: Option<SharedString>,
    /// Shown at the end: the keys, or "Enter copies".
    pub shortcut: Option<SharedString>,
    /// Its icon.
    pub icon: IconName,
    /// A Tailwind color name for the icon ("amber-500"), or the muted text color.
    pub tone: Option<&'static str>,
    /// A place or a command.
    pub kind: Kind,
    /// Ranking boost, e.g. how often a folder is opened.
    pub weight: f64,
    /// Listed only once something's typed: one of a set (each tile size), which would crowd the full list.
    pub search_only: bool,
    /// What choosing it does.
    pub run: Run,
}

impl Command {
    fn new(id: impl Into<String>, label: impl Into<SharedString>, icon: IconName, kind: Kind, run: Run) -> Self {
        Command {
            id: id.into(),
            label: label.into(),
            detail: None,
            shortcut: None,
            icon,
            tone: None,
            kind,
            weight: 0.,
            search_only: false,
            run,
        }
    }

    fn action(id: &str, label: impl Into<SharedString>, icon: IconName, maker: ActionMaker) -> Self {
        let mut command = Command::new(id, label, icon, Kind::Action, Run::Action(maker));
        command.shortcut = shortcut_of(maker);
        command
    }

    fn change(id: impl Into<String>, label: impl Into<SharedString>, icon: IconName, change: Change) -> Self {
        Command::new(id, label, icon, Kind::Action, Run::Change(change))
    }

    fn go(id: impl Into<String>, label: impl Into<SharedString>, icon: IconName, path: &str) -> Self {
        Command::new(id, label, icon, Kind::Action, Run::Navigate(path.to_string()))
    }
}

// ---- Keys, as the TypeScript wrote them ----------------------------------------------------------------

/// A GPUI keystroke as the palette words it: `ctrl-shift-tab` is "Ctrl+Shift+Tab", `ctrl--` is "Ctrl+-".
pub fn describe_keys(keys: &str) -> String {
    let mut rest = keys;
    let mut parts: Vec<String> = Vec::new();
    for (prefix, name) in [("ctrl-", "Ctrl"), ("alt-", "Alt"), ("shift-", "Shift"), ("cmd-", "Win")] {
        if let Some(after) = rest.strip_prefix(prefix) {
            parts.push(name.to_string());
            rest = after;
        }
    }
    let mut key = rest.chars();
    let key = match (key.next(), key.next()) {
        (Some(only), None) => only.to_uppercase().collect::<String>(),
        (Some(first), Some(_)) => first.to_uppercase().chain(rest.chars().skip(1)).collect(),
        _ => String::new(),
    };
    parts.push(key);
    parts.join("+")
}

/// The first key bound to the action `maker` makes, worded for the list (`None` for an action without a key).
pub fn shortcut_of(maker: ActionMaker) -> Option<SharedString> {
    let name = maker().name();
    actions::BINDINGS
        .iter()
        .find(|binding: &&Binding| (binding.action)().name() == name)
        .map(|binding| SharedString::from(describe_keys(binding.keys)))
}

// ---- What the palette is built from --------------------------------------------------------------------

/// The state the list is built from, read once when the palette opens.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// The active tab's folder (`""` is This PC).
    pub path: String,
    /// The tab shows a search's results.
    pub finding: bool,
    /// The tab is in an archive.
    pub in_archive: bool,
    /// The tab's folder is one Windows keeps this account out of.
    pub shut: bool,
    /// The tab shows the grid.
    pub grid: bool,
    /// The tab's id.
    pub tab: Option<TabId>,
    /// How many tabs the active pane has.
    pub tabs: usize,
    /// Two panes are showing.
    pub dual: bool,
    /// A closed tab can be brought back.
    pub can_reopen: bool,
    /// The tab can go back / forward.
    pub can_back: bool,
    /// See `can_back`.
    pub can_forward: bool,
    /// Something is selected.
    pub chosen: bool,
    /// Something has the keyboard's focus in the list.
    pub focused: bool,
    /// Pinned folders, in the sidebar's order.
    pub pinned: Vec<String>,
    /// Home, Documents… with their paths.
    pub known: Vec<(KnownId, String)>,
    /// The drives.
    pub drives: Vec<Drive>,
    /// Remote connections.
    pub remotes: Vec<RemoteConnection>,
    /// How often and how lately folders were opened.
    pub visits: Vec<(String, Visit)>,
    /// zoxide's folders and scores, when history is shared with it.
    pub zoxide: Vec<(String, f64)>,
    /// The settings that change what is offered.
    pub show_hidden: bool,
    /// See `show_hidden`.
    pub show_preview: bool,
    /// The grid's tile size now.
    pub tile_size: Option<TileSize>,
    /// The dark theme is showing.
    pub dark: bool,
    /// Now, in Unix milliseconds (for frecency).
    pub now: i64,
    /// The active tab shows its disk usage map.
    pub mapped: bool,
    /// The selected items, in the order the rows show them (`chosenEntries`).
    pub chosen_entries: Vec<FileEntry>,
    /// The selected paths, in the order they were selected (`[...t.selection]`).
    pub selected: Vec<String>,
    /// The item the keyboard is on (`t.focus`).
    pub focus_path: Option<String>,
    /// The folder open in the other pane, when things can be copied or moved into it (`otherPaneFolder`).
    pub other_pane: Option<String>,
    /// What Undo would take back, in a few words (`nextUndo()?.label`).
    pub undo: Option<String>,
    /// The two files Ctrl+M compares: path and name, left then right (`comparePair`).
    pub pair: Option<((String, String), (String, String))>,
    /// The file an archive in the tab is in, when the tab is inside one (`archive.outer`).
    pub archive_outer: Option<String>,
    /// Saved searches (`savedSearches`).
    pub saved_searches: Vec<SavedSearch>,
    /// The scripts of the Scripts folder (read in the background when the palette opens).
    pub scripts: Vec<Script>,
    /// The editors and terminals found on this PC (read in the background).
    pub openers: Vec<Opener>,
    /// What this folder's projects can run (read in the background).
    pub tasks: Vec<TaskGroup>,
    /// The folder is closed to this account and nothing of it was listed (`shut && !listing`): nothing to measure.
    pub unlisted: bool,
    /// Windows' Get access can open the closed folder (it isn't on another computer, nor refused).
    pub can_get_access: bool,
    /// The panes aren't of equal width (`dualSplit !== null`).
    pub uneven: bool,
    /// The tab is at the top of an archive file on disk (the archive's path is the tab's and its own outer), and
    /// whether that format holds one file only (`SINGLE_FILE`): what "Extract all" is offered for.
    pub archive_top: Option<ArchiveTop>,
    /// File Explorer's New menu kinds on this PC: extension with its dot, and name (`newKinds`).
    pub new_kinds: Vec<(String, String)>,
    /// Something is being sent to another PC.
    pub sending: bool,
    /// Something is shared on this network.
    pub serving: bool,
}

/// The archive file a tab is at the top of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveTop {
    /// The archive file on disk (`archive.outer`).
    pub outer: String,
    /// Its format holds a single file (`SINGLE_FILE.includes(archive.format)`).
    pub single_file: bool,
}

/// How much a folder is used: its visits here (frecency) and its zoxide score, together (`usedScore`).
fn used_score(path: &str, inputs: &Inputs) -> f64 {
    let visit = inputs
        .visits
        .iter()
        .find(|(p, _)| p == path)
        .map_or(0., |(_, v)| frecency(v, inputs.now));
    let lower = path.to_lowercase();
    let zoxide = inputs
        .zoxide
        .iter()
        .find(|(p, _)| p.to_lowercase() == lower)
        .map_or(0., |(_, score)| *score);
    visit + zoxide
}

/// The folders used most, here and (shared) in terminals, best first (`usedFolders`).
fn used_folders(inputs: &Inputs, limit: usize) -> Vec<String> {
    let mut all: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();
    for (path, _) in &inputs.visits {
        if seen.insert(path.to_lowercase()) {
            all.push((path.to_lowercase(), path.clone()));
        }
    }
    for (path, _) in &inputs.zoxide {
        if seen.insert(path.to_lowercase()) {
            all.push((path.to_lowercase(), path.clone()));
        }
    }
    let mut scored: Vec<(f64, String)> = all
        .into_iter()
        .map(|(_, path)| (used_score(&path, inputs), path))
        .collect();
    // JS sorts stably, best first.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(limit).map(|(_, path)| path).collect()
}

fn remote_icon(remote: &RemoteConnection) -> IconName {
    match remote.kind {
        RemoteKind::S3 => IconName::Cloud,
        RemoteKind::Sftp => IconName::SquareTerminal,
        _ => IconName::Server,
    }
}

/// The folders on offer: pinned, known, used, drives, remotes (`paletteCommands`' first half).
pub fn folders(inputs: &Inputs) -> Vec<Command> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Command> = Vec::new();
    let mut add = |path: &str, label: SharedString, icon: IconName, weight: f64, tone: Option<&'static str>| {
        let key = path.to_lowercase();
        if !seen.insert(key.clone()) {
            return;
        }
        let mut command = Command::new(
            format!("folder:{key}"),
            label,
            icon,
            Kind::Folder,
            Run::Navigate(path.to_string()),
        );
        command.detail = Some(path.to_string().into());
        command.tone = tone;
        command.weight = weight + (1. + used_score(path, inputs)).log2();
        out.push(command);
    };
    for path in &inputs.pinned {
        add(path, base_name(path).to_string().into(), IconName::Pin, 1.5, None);
    }
    for (id, path) in &inputs.known {
        add(path, i18n::translate(id.name()), crate::kit::known_icon(*id), 0.5, None);
    }
    for path in used_folders(inputs, USED_FOLDERS) {
        add(
            &path,
            base_name(&path).to_string().into(),
            IconName::Folder,
            0.,
            Some("amber-500"),
        );
    }
    for drive in &inputs.drives {
        add(
            &drive.path,
            crate::explorer::drive_name(drive).into(),
            crate::kit::drive_icon(drive.kind),
            0.,
            None,
        );
    }
    for remote in &inputs.remotes {
        add(
            &format!("{}{}", glint_model::paths::REMOTE, remote.id),
            remote.name.clone().into(),
            remote_icon(remote),
            0.5,
            None,
        );
    }
    for saved in &inputs.saved_searches {
        let place = if saved.folder.is_empty() {
            t!("every drive").to_string()
        } else {
            saved.folder.clone()
        };
        let mut command = Command::new(
            format!(
                "search:{place}
{}",
                saved.query
            ),
            saved.query.clone(),
            IconName::Search,
            Kind::Folder,
            Run::Saved(saved.clone()),
        );
        command.detail = Some(t!("Saved search in {place}", place = place));
        command.weight = 1.5;
        out.push(command);
    }
    out
}

/// Every command that applies to what the active tab shows (`paletteCommands`' second half, the commands
/// that have an action or a plain change here; see the module notes of [`super`] for what waits for its part).
pub fn actions(inputs: &Inputs) -> Vec<Command> {
    use IconName as I;
    let path = inputs.path.as_str();
    // The Recycle Bin is a place, not a folder: what's done to folders isn't done to it.
    let bin = is_recycle_bin(path) && !inputs.finding;
    let in_folder = is_folder_path(path);
    let remote = is_remote_path(path);
    let writable = in_folder && !inputs.in_archive;
    let can_grid = in_folder || bin || remote || inputs.finding;
    let grid = can_grid && inputs.grid;
    let place = Place::of(inputs);
    let mut out: Vec<Command> = Vec::new();
    let mut push = |condition: bool, command: Command| {
        if condition {
            out.push(command);
        }
    };

    if let Some(tab) = inputs.tab {
        push(
            inputs.can_get_access,
            Command::change(
                "get-access",
                t!("Get access to this folder"),
                I::ShieldCheck,
                Change::GetAccess(tab),
            ),
        );
    }
    push(
        true,
        Command::action(
            "find",
            if bin {
                t!("Filter what's in the Recycle Bin")
            } else {
                t!("Find files in this folder")
            },
            I::Search,
            || Box::new(actions::SearchHere),
        ),
    );
    push(
        writable && !inputs.shut,
        Command::action("search-text", t!("Search text in this folder"), I::TextSearch, || {
            Box::new(actions::TextSearch)
        }),
    );
    push(
        bin && inputs.chosen,
        menu(
            "restore",
            t!("Restore the selection where it was deleted from"),
            I::RotateCcw,
            MenuEvent::Restore {
                paths: inputs.selected.clone(),
            },
        ),
    );
    push(
        bin && inputs.chosen,
        Command::action("purge", t!("Delete the selection for good"), I::Trash, || {
            Box::new(actions::DeletePermanently)
        }),
    );
    push(
        in_folder && !inputs.finding && !inputs.unlisted,
        menu(
            "sizes",
            t!("Calculate folder sizes"),
            I::HardDrive,
            MenuEvent::FolderSizes,
        ),
    );
    push(
        can_grid && grid,
        Command::action("view-list", t!("Show as a list"), I::List, || {
            Box::new(actions::ListView)
        }),
    );
    push(
        can_grid && !grid,
        Command::action(
            "view-grid",
            t!("Show as a grid, with thumbnails"),
            I::LayoutGrid,
            || Box::new(actions::GridView),
        ),
    );
    push(
        in_folder && !inputs.finding && !inputs.in_archive,
        Command::action(
            "disk-usage",
            if inputs.mapped {
                t!("Back to the file list")
            } else {
                t!("Disk usage")
            },
            I::LayoutDashboard,
            || Box::new(actions::DiskUsage),
        ),
    );
    let tile = inputs.tile_size.unwrap_or(TileSize::Medium);
    push(
        grid && zoom_tiles(tile.as_str(), 1) != tile,
        Command::action("zoom-in", t!("Zoom in: larger tiles"), I::ZoomIn, || {
            Box::new(actions::GridLarger)
        }),
    );
    push(
        grid && zoom_tiles(tile.as_str(), -1) != tile,
        Command::action("zoom-out", t!("Zoom out: smaller tiles"), I::ZoomOut, || {
            Box::new(actions::GridSmaller)
        }),
    );
    // Each size, once something's typed ("large", "tiles"): the grid at that size.
    for spec in TILE_SIZES {
        if can_grid && !(grid && spec.size == tile) {
            let mut command = Command::change(
                format!("tiles-{}", spec.size.as_str()),
                t!("{label} tiles", label = i18n::translate(spec.label)),
                I::LayoutGrid,
                Change::Tiles(spec.size),
            );
            command.search_only = true;
            if grid && spec.size == TileSize::Medium {
                command.shortcut = Some("Ctrl+0".into());
            }
            out.push(command);
        }
    }
    let mut push = |condition: bool, command: Command| {
        if condition {
            out.push(command);
        }
    };
    push(
        inputs.focused,
        Command::action("quick-look", t!("Quick Look"), I::Eye, || Box::new(actions::QuickLook)),
    );
    push(
        true,
        Command::action(
            "hidden",
            if inputs.show_hidden {
                t!("Hide hidden items")
            } else {
                t!("Show hidden items")
            },
            if inputs.show_hidden { I::EyeOff } else { I::Eye },
            || Box::new(actions::ToggleHidden),
        ),
    );
    push(
        true,
        Command::action(
            "preview",
            if inputs.show_preview {
                t!("Hide preview pane")
            } else {
                t!("Show preview pane")
            },
            I::PanelRight,
            || Box::new(actions::TogglePreview),
        ),
    );
    push(
        true,
        Command::action("refresh", t!("Refresh"), I::RotateCw, || Box::new(actions::Refresh)),
    );
    push(
        inputs.can_back,
        Command::action("back", t!("Back"), crate::kit::back_icon(), || Box::new(actions::Back)),
    );
    push(
        inputs.can_forward,
        Command::action("forward", t!("Forward"), crate::kit::forward_icon(), || {
            Box::new(actions::Forward)
        }),
    );
    push(
        in_folder,
        Command::action("up", t!("Up one folder"), I::ArrowUp, || Box::new(actions::Up)),
    );
    push(true, Command::go("this-pc", t!("This PC"), I::Monitor, THIS_PC));
    push(
        !is_recent(path),
        Command::go("recent", t!("Recent: the files opened lately"), I::Clock, RECENT),
    );
    push(
        !is_gallery(path),
        Command::go(
            "gallery",
            t!("Gallery: your photos and videos, by month"),
            I::Images,
            GALLERY,
        ),
    );
    push(
        !bin,
        Command::go("recycle-bin", t!("Recycle Bin"), I::Trash, RECYCLE_BIN),
    );
    push(
        true,
        Command::new(
            "empty-bin",
            t!("Empty the Recycle Bin"),
            I::Trash,
            Kind::Action,
            Run::Menu(MenuEvent::EmptyRecycleBin),
        ),
    );
    push(
        true,
        Command::action("new-tab", t!("New tab"), I::Plus, || Box::new(actions::NewTab)),
    );
    push(
        true,
        menu(
            "new-window",
            t!("New window"),
            I::AppWindow,
            MenuEvent::OpenInNewWindow {
                path: inputs.path.clone(),
            },
        ),
    );
    push(
        inputs.tabs > 1 || inputs.dual,
        Command::action("close-tab", t!("Close tab"), I::X, || Box::new(actions::CloseTab)),
    );
    if let Some(tab) = inputs.tab {
        push(
            inputs.tabs > 1,
            Command::change(
                "close-other-tabs",
                t!("Close the other tabs"),
                I::X,
                Change::CloseOthers(tab),
            ),
        );
        push(
            true,
            Command::change("duplicate-tab", t!("Duplicate tab"), I::Copy, Change::Duplicate(tab)),
        );
    }
    if let Some(tab) = inputs.tab {
        push(
            inputs.dual && inputs.tabs > 1,
            Command::change(
                "move-tab",
                t!("Move the tab to the other pane"),
                I::ArrowRightLeft,
                Change::MoveTab(tab),
            ),
        );
    }
    push(
        inputs.can_reopen,
        Command::action("reopen-tab", t!("Reopen closed tab"), I::RotateCcw, || {
            Box::new(actions::ReopenClosedTab)
        }),
    );
    push(
        inputs.tabs > 1,
        Command::action("next-tab", t!("Next tab"), I::ChevronRight, || {
            Box::new(actions::NextTab)
        }),
    );
    push(
        inputs.tabs > 1,
        Command::action("previous-tab", t!("Previous tab"), I::ChevronLeft, || {
            Box::new(actions::PreviousTab)
        }),
    );
    push(
        true,
        Command::action(
            "dual",
            if inputs.dual {
                t!("Close the second pane")
            } else {
                t!("Dual pane: two folders side by side")
            },
            I::Columns2,
            || Box::new(actions::DualPane),
        ),
    );
    for spec in GROUP_BYS {
        let label = if spec.by == GroupBy::None {
            t!("Group by: nothing (no headers in the list)")
        } else {
            t!(
                "Group by {label}: headers in the list",
                label = i18n::translate(spec.label).to_lowercase()
            )
        };
        push(
            true,
            Command::change(
                format!("group-{}", format!("{:?}", spec.by).to_lowercase()),
                label,
                I::ListTree,
                Change::Group(spec.by),
            ),
        );
    }
    push(
        true,
        Command::action(
            "compare-panes",
            t!("Compare panes: highlight differences between both panes"),
            I::GitCompareArrows,
            || Box::new(actions::ComparePanes),
        ),
    );
    push(
        inputs.dual,
        Command::action("switch-pane", t!("Switch to the other pane"), I::ArrowRightLeft, || {
            Box::new(actions::OtherPane)
        }),
    );
    push(
        inputs.dual,
        Command::action("swap-panes", t!("Swap panes"), I::ArrowLeftRight, || {
            Box::new(actions::SwapPanes)
        }),
    );
    push(
        (in_folder || remote) && !inputs.finding,
        Command::change(
            "open-other",
            if inputs.dual {
                t!("Show this folder in the other pane")
            } else {
                t!("Show this folder side by side")
            },
            I::Columns2,
            Change::SideBySide(inputs.path.clone()),
        ),
    );
    push(
        place.other.is_some(),
        Command::new(
            "open-here",
            t!("Open the other pane's folder here"),
            I::FolderInput,
            Kind::Action,
            Run::Navigate(place.other.clone().unwrap_or_default()),
        ),
    );
    push(
        inputs.dual && inputs.uneven,
        Command::change("even-panes", t!("Even out the panes"), I::Columns2, Change::EvenPanes),
    );
    for command in nearby_and_files(inputs, &place) {
        push(true, command);
    }
    if let Some(top) = &inputs.archive_top {
        push(
            true,
            menu(
                "extract-all",
                if top.single_file {
                    t!("Extract the file next to the archive")
                } else {
                    t!("Extract all: everything into a folder next to the archive")
                },
                I::ArchiveRestore,
                MenuEvent::ExtractHere {
                    path: top.outer.clone(),
                },
            ),
        );
    }
    for command in to_the_other_pane(inputs, &place) {
        push(true, command);
    }
    push(
        in_folder,
        Command::action("copy-path", t!("Copy this folder's path"), I::ClipboardCopy, || {
            Box::new(actions::CopyPath)
        }),
    );
    push(
        true,
        Command::action(
            "terminal-pane",
            t!("Terminal here, under the files"),
            I::SquareTerminal,
            || Box::new(actions::ToggleTerminal),
        ),
    );
    for command in tools(inputs, &place) {
        push(true, command);
    }
    let pinned = inputs.pinned.iter().any(|p| p == path);
    push(
        in_folder,
        Command::change(
            "pin",
            if pinned {
                t!("Unpin this folder")
            } else {
                t!("Pin this folder to the sidebar")
            },
            if pinned { I::PinOff } else { I::Pin },
            Change::TogglePin(inputs.path.clone()),
        ),
    );
    push(
        in_folder || inputs.finding,
        Command::change(
            "invert-selection",
            t!("Invert selection"),
            I::ListChecks,
            Change::InvertSelection,
        ),
    );
    push(
        writable && !inputs.finding && !inputs.shut,
        Command::action(
            "batch-rename",
            if inputs.chosen {
                t!("Rename the selected items")
            } else {
                t!("Rename items in this folder")
            },
            I::PenLine,
            || Box::new(actions::BatchRename),
        ),
    );
    push(
        writable && !inputs.finding && !inputs.shut,
        Command::action("new-folder", t!("New folder"), I::FolderPlus, || {
            Box::new(actions::NewFolder)
        }),
    );
    push(
        writable && !inputs.finding && !inputs.shut,
        menu(
            "new-file",
            t!("New text document"),
            I::FilePlus,
            MenuEvent::NewTextDocument,
        ),
    );
    for (ext, name) in &inputs.new_kinds {
        push(
            writable && !inputs.finding && !inputs.shut,
            menu(
                &format!("new-{ext}"),
                t!("New {kind}", kind = name),
                I::FilePlus,
                MenuEvent::NewOfKind { ext: ext.clone() },
            ),
        );
    }
    push(
        writable && !inputs.finding && !inputs.shut,
        Command::new(
            "paste-queued",
            t!("Paste with the copy queue: pause, resume, and each copy checked"),
            I::ClipboardPaste,
            Kind::Action,
            Run::Menu(MenuEvent::PasteQueued {
                folder: inputs.path.clone(),
            }),
        ),
    );
    for command in inspect_and_share(inputs, &place) {
        push(true, command);
    }
    if let Some(outer) = &inputs.archive_outer {
        push(
            true,
            Command::new(
                "archive-properties",
                t!("Archive properties"),
                I::Info,
                Kind::Action,
                Run::Properties(outer.clone()),
            ),
        );
    }
    push(
        writable,
        Command::action("properties", t!("Properties"), I::Info, || {
            Box::new(actions::Properties)
        }),
    );
    // From the theme showing, which System leaves to Windows.
    push(
        true,
        if inputs.dark {
            Command::change(
                "theme",
                t!("Switch to light theme"),
                I::Sun,
                Change::Theme(Theme::Light),
            )
        } else {
            Command::change("theme", t!("Switch to dark theme"), I::Moon, Change::Theme(Theme::Dark))
        },
    );
    push(
        inputs.undo.is_some(),
        Command::action(
            "undo",
            t!("Undo the {label}", label = inputs.undo.clone().unwrap_or_default()),
            I::Undo2,
            || Box::new(actions::Undo),
        ),
    );
    push(
        true,
        Command::action("settings", t!("Settings"), I::Settings, || {
            Box::new(actions::OpenSettings)
        }),
    );
    push(
        true,
        Command::action("shortcuts", t!("Keyboard shortcuts"), I::Keyboard, || {
            Box::new(actions::ShowShortcuts)
        }),
    );
    push(
        true,
        Command::action("drive-health", t!("Drive health"), I::HeartPulse, || {
            Box::new(actions::ShowDriveHealth)
        }),
    );
    push(
        true,
        Command::new(
            "add-remote",
            t!("Add a remote connection: a cloud (Google Drive, OneDrive, Dropbox and 60 more), an S3 bucket, a WebDAV or an SFTP server"),
            I::PlugZap,
            Kind::Action,
            Run::AddRemote,
        ),
    );
    push(
        true,
        Command::new(
            "rules",
            t!("Folder rules: sort out what comes into a folder, and what they did"),
            I::WandSparkles,
            Kind::Action,
            Run::Rules(None),
        ),
    );
    push(
        writable && !inputs.shut && !inputs.finding,
        Command::new(
            "rule-here",
            t!("A rule for this folder: move, rename or clear out what comes in"),
            I::WandSparkles,
            Kind::Action,
            Run::Menu(MenuEvent::Rule {
                folder: inputs.path.clone(),
            }),
        ),
    );
    out
}

// ---- The commands that do things to files, and the tools ----------------------------------------------

/// Where the tab is, as `paletteCommands` worked it out once for every condition.
struct Place {
    /// In the Recycle Bin (not a search of it).
    bin: bool,
    /// A folder on disk.
    in_folder: bool,
    /// A remote connection's folder.
    remote: bool,
    /// A folder where things can be made and changed: on disk, not inside an archive.
    writable: bool,
    /// Windows keeps this account out of the folder.
    shut: bool,
    /// Something in the selection is in a closed folder.
    closed_items: bool,
    /// The other pane's folder, when things can go into it.
    other: Option<String>,
}

impl Place {
    fn of(inputs: &Inputs) -> Place {
        let path = inputs.path.as_str();
        let in_folder = is_folder_path(path);
        Place {
            bin: is_recycle_bin(path) && !inputs.finding,
            in_folder,
            remote: is_remote_path(path),
            writable: in_folder && !inputs.in_archive,
            shut: inputs.shut,
            closed_items: inputs.chosen_entries.iter().any(is_closed),
            other: inputs.other_pane.clone(),
        }
    }
}

fn menu(id: &str, label: impl Into<SharedString>, icon: IconName, event: MenuEvent) -> Command {
    Command::new(id, label, icon, Kind::Action, Run::Menu(event))
}

fn paths_of(entries: &[FileEntry]) -> Vec<String> {
    entries.iter().map(|e| e.path.clone()).collect()
}

/// "Send to a phone nearby", and what the selection can be turned into: compressed, extracted, converted,
/// encrypted, cleaned, merged, subtitled…
fn nearby_and_files(inputs: &Inputs, place: &Place) -> Vec<Command> {
    use IconName as I;
    let entries = inputs.chosen_entries.as_slice();
    let all = paths_of(entries);
    let any = |test: fn(&FileEntry) -> bool| entries.iter().any(test);
    let count = |test: fn(&FileEntry) -> bool| entries.iter().filter(|e| test(e)).count();
    let on_disk_only = !inputs.in_archive && !place.remote && !place.closed_items && !place.bin;
    let mut out: Vec<Command> = Vec::new();
    let mut add = |condition: bool, command: Command| {
        if condition {
            out.push(command);
        }
    };
    add(
        true,
        menu(
            "nearby",
            if entries.is_empty() {
                t!("Phones and computers nearby (LocalSend): send or receive")
            } else {
                t!("Send the selection to a phone nearby (LocalSend)")
            },
            I::Radar,
            MenuEvent::SendNearby { paths: all.clone() },
        ),
    );
    add(
        on_disk_only && !entries.is_empty(),
        menu(
            "compress",
            t!("Compress the selection to a zip file"),
            I::FileArchive,
            MenuEvent::Compress {
                paths: all.clone(),
                format: CompressFormat::Zip,
            },
        ),
    );
    if let [one] = entries {
        add(
            is_archive_file(one) && !inputs.in_archive && !place.closed_items,
            menu(
                "extract",
                t!("Extract “{name}” here", name = one.name),
                I::ArchiveRestore,
                MenuEvent::ExtractHere { path: one.path.clone() },
            ),
        );
        add(
            can_back_up(one),
            menu(
                "backup",
                t!("Back up the selected folder: to a drive or a cloud, encrypted, daily or weekly"),
                I::DatabaseBackup,
                MenuEvent::BackUp {
                    folder: one.path.clone(),
                },
            ),
        );
    }
    add(
        entries.iter().any(|e| can_shrink(e) || e.dir),
        menu(
            "shrink",
            t!("Make the selection smaller: videos that look the same, photos, PDFs, Office files"),
            I::Minimize2,
            MenuEvent::MakeSmaller { paths: all.clone() },
        ),
    );
    add(
        !entries.is_empty() && entries.iter().all(can_compress_in_place),
        menu(
            "compress-in-place",
            t!("Compress in place: the selection takes less space and still opens normally"),
            I::Shrink,
            MenuEvent::CompressInPlace { paths: all.clone() },
        ),
    );
    add(
        any(can_convert_doc),
        menu(
            "convert-doc",
            t!("Convert the selected documents: Word, PDF, Markdown, HTML, EPUB, PowerPoint…"),
            I::FileType,
            MenuEvent::ConvertDocument { paths: all.clone() },
        ),
    );
    add(
        any(is_song),
        menu(
            "song-details",
            t!("Edit the selected songs' details: title, artist, album, track, cover"),
            I::Music,
            MenuEvent::EditSongs { paths: all.clone() },
        ),
    );
    let media = |tool| MenuEvent::Media {
        paths: all.clone(),
        tool,
    };
    add(
        any(is_media_file),
        menu(
            "media-volume",
            t!("Even out the volume of the selected recordings and videos"),
            I::Volume2,
            media(MediaTool::Volume),
        ),
    );
    add(
        any(is_jxl),
        menu(
            "jxl-to-jpeg",
            t!("Turn the selected JPEG XL files back into JPEGs"),
            I::ImageDown,
            MenuEvent::BackToJpeg { paths: all.clone() },
        ),
    );
    add(
        any(can_convert),
        menu(
            "convert",
            t!("Convert the selected pictures: JPEG, PNG, WebP or AVIF, smaller"),
            I::ImageDown,
            MenuEvent::ConvertPictures { paths: all.clone() },
        ),
    );
    add(
        any(is_video_file),
        menu(
            "media-convert",
            t!("Convert or shrink the selected videos (MP4, HEVC, AV1, to fit a size)"),
            I::Film,
            media(MediaTool::Convert),
        ),
    );
    add(
        any(is_media_file),
        menu(
            "media-audio",
            t!("Take the sound out of the selected videos or recordings"),
            I::AudioLines,
            media(MediaTool::Audio),
        ),
    );
    add(
        any(is_video_file),
        menu(
            "media-gif",
            t!("Make a GIF of the selected video"),
            I::Clapperboard,
            media(MediaTool::Gif),
        ),
    );
    add(
        any(can_encrypt),
        menu(
            "encrypt",
            t!("Encrypt the selection with a passphrase (age)"),
            I::Lock,
            MenuEvent::Vault {
                paths: all.clone(),
                decrypt: false,
            },
        ),
    );
    add(
        any(is_age_file),
        menu(
            "decrypt",
            t!("Decrypt the selected .age files"),
            I::LockOpen,
            MenuEvent::Vault {
                paths: all.clone(),
                decrypt: true,
            },
        ),
    );
    add(
        any(can_clean),
        menu(
            "remove-personal-info",
            t!("Remove personal info: where, who and with what, before sharing"),
            I::EyeOff,
            MenuEvent::RemovePersonalInfo { paths: all.clone() },
        ),
    );
    add(
        any(can_redate),
        menu(
            "date-taken",
            t!("Change when the selected photos or videos were taken"),
            I::CalendarClock,
            MenuEvent::ChangeDateTaken { paths: all.clone() },
        ),
    );
    add(
        count(is_data_file) > 1,
        menu(
            "query-together",
            t!("Query the selected data files together with SQL (DuckDB)"),
            I::Sigma,
            MenuEvent::QueryTogether { paths: all.clone() },
        ),
    );
    add(
        any(is_data_file),
        menu(
            "convert-data",
            t!("Convert the selected data files: CSV, Parquet or JSON lines"),
            I::FileCog,
            MenuEvent::ConvertData { paths: all.clone() },
        ),
    );
    let pdf = |tool| MenuEvent::PdfTools {
        paths: all.clone(),
        tool,
    };
    add(
        count(is_pdf_file) > 1,
        menu(
            "pdf-merge",
            t!("Merge the selected PDFs into one"),
            I::FileStack,
            pdf(PdfTool::Merge),
        ),
    );
    add(
        any(is_pdf_file),
        menu(
            "pdf-tools",
            t!("PDF tools: split, pages, smaller, pictures, passwords, stamp"),
            I::FileText,
            pdf(PdfTool::Optimize),
        ),
    );
    add(
        any(is_media_file),
        menu(
            "media-subtitles",
            t!("Make subtitles for the selected videos (heard on this PC)"),
            I::Captions,
            media(MediaTool::Subtitles),
        ),
    );
    out
}

/// Copy or move the selection into the folder of the other pane.
fn to_the_other_pane(inputs: &Inputs, place: &Place) -> Vec<Command> {
    let mut out = Vec::new();
    if place.other.is_none() || !inputs.chosen {
        return out;
    }
    if !place.bin {
        out.push(menu(
            "copy-other",
            t!("Copy the selection to the other pane"),
            IconName::CopyPlus,
            MenuEvent::CopyToOtherPane {
                paths: inputs.selected.clone(),
                moving: false,
            },
        ));
    }
    if !inputs.in_archive && !place.remote && !place.shut && !place.bin {
        out.push(menu(
            "move-other",
            t!("Move the selection to the other pane"),
            IconName::FolderOutput,
            MenuEvent::CopyToOtherPane {
                paths: inputs.selected.clone(),
                moving: true,
            },
        ));
    }
    out
}

/// Terminal, the project's tasks, scripts, editors, File Explorer.
fn tools(inputs: &Inputs, place: &Place) -> Vec<Command> {
    use IconName as I;
    let entries = inputs.chosen_entries.as_slice();
    let path = inputs.path.as_str();
    let mut out: Vec<Command> = Vec::new();
    if place.writable && !place.shut {
        out.push(Command::new(
            "terminal",
            t!("Open in Terminal"),
            I::SquareTerminal,
            Kind::Action,
            Run::Terminal(path.to_string()),
        ));
    }
    // What this folder's projects can run, as the Run menu has them.
    if place.writable && !place.shut && !inputs.finding {
        for group in inputs.tasks.iter().filter(|g| g.missing.is_none()) {
            for task in &group.tasks {
                out.push(Command::new(
                    format!("run:{}:{}", group.folder, task.command),
                    t!("Run {command}", command = task.command),
                    I::Play,
                    Kind::Action,
                    Run::Task {
                        folder: group.folder.clone(),
                        command: task.command.clone(),
                    },
                ));
            }
        }
    }
    if !inputs.in_archive
        && !place.remote
        && !place.shut
        && !place.closed_items
        && !place.bin
        && (place.in_folder || !entries.is_empty())
    {
        let on: Vec<String> = if entries.is_empty() {
            vec![path.to_string()]
        } else {
            paths_of(entries)
        };
        let cwd = if place.in_folder {
            path.to_string()
        } else {
            on.first().map(|first| parent_path(first)).unwrap_or_default()
        };
        for script in &inputs.scripts {
            let detail = if script.missing.is_some() {
                t!("installs what it needs first")
            } else if entries.len() > 1 {
                t!("on the {items}", items = plural(entries.len(), "selected item"))
            } else {
                t!(
                    "on {name}",
                    name = entries
                        .first()
                        .map_or_else(|| t!("this folder").to_string(), |e| e.name.clone())
                )
            };
            let mut command = menu(
                &format!("script:{}", script.path),
                t!("Script: {name}", name = script.name),
                I::ScrollText,
                MenuEvent::RunScript {
                    script: script.path.clone(),
                    cwd: cwd.clone(),
                    paths: on.clone(),
                },
            );
            command.detail = Some(detail);
            out.push(command);
        }
    }
    out.push(menu(
        "scripts-folder",
        t!("Open the Scripts folder"),
        I::FolderCog,
        MenuEvent::OpenScriptsFolder,
    ));
    if place.writable && !place.shut {
        for opener in &inputs.openers {
            out.push(Command::new(
                format!("open-in-{}", opener.id),
                t!("Open this folder in {name}", name = opener.name),
                if opener.terminal { I::SquareTerminal } else { I::Code },
                Kind::Action,
                Run::OpenIn {
                    opener: opener.id.to_string(),
                    name: opener.name.to_string(),
                    path: path.to_string(),
                },
            ));
        }
    }
    if let Some(archive) = &inputs.archive_outer {
        out.push(menu(
            "explorer-archive",
            t!("Show the archive in File Explorer"),
            I::FileArchive,
            MenuEvent::RevealArchive {
                archive: archive.clone(),
            },
        ));
    }
    if place.writable {
        out.push(Command::new(
            "explorer",
            t!("Open in File Explorer"),
            I::FolderSearch,
            Kind::Action,
            Run::FileExplorer(path.to_string()),
        ));
    }
    out
}

/// X-Ray, comparing, the folder's history, duplicates, cleaning up, sending and sharing, locks, scans.
fn inspect_and_share(inputs: &Inputs, place: &Place) -> Vec<Command> {
    use IconName as I;
    let entries = inputs.chosen_entries.as_slice();
    let folder = inputs.path.clone();
    let chosen = inputs.chosen;
    let on_disk_only = !inputs.in_archive && !place.remote;
    let here = place.writable && !place.shut;
    // One item, or the folder when nothing is chosen.
    let one = inputs.selected.len() == 1;
    let sendable = on_disk_only
        && !place.closed_items
        && !place.bin
        && !place.shut
        && (entries.len() == 1 || (place.in_folder && !chosen));
    let sent = match entries {
        [only] => only.path.clone(),
        _ => folder.clone(),
    };
    let mut out: Vec<Command> = Vec::new();
    let mut add = |condition: bool, command: Command| {
        if condition {
            out.push(command);
        }
    };
    let xray = inputs.focus_path.clone().or_else(|| inputs.selected.first().cloned());
    add(
        on_disk_only && !place.closed_items && xray.is_some(),
        menu(
            "file-xray",
            t!("File X-Ray: hashes and hidden streams"),
            I::FingerprintPattern,
            MenuEvent::XRay {
                path: xray.unwrap_or_default(),
            },
        ),
    );
    if let Some(((_, left), (_, right))) = &inputs.pair {
        add(
            true,
            Command::action(
                "compare-files",
                t!("Compare “{left}” and “{right}”", left = left, right = right),
                I::GitCompareArrows,
                || Box::new(actions::CompareFiles),
            ),
        );
    }
    add(
        here,
        menu(
            "timeline",
            t!("Recent changes in this folder"),
            I::FileClock,
            MenuEvent::Timeline { folder: folder.clone() },
        ),
    );
    add(
        true,
        Command::action(
            "history",
            t!("Moved and deleted: where did a file go?"),
            I::ClockArrowLeft,
            || Box::new(actions::ShowHistory { query: String::new() }),
        ),
    );
    add(
        here,
        menu(
            "duplicates",
            t!("Find duplicate files in this folder"),
            I::Files,
            MenuEvent::Duplicates { folder: folder.clone() },
        ),
    );
    add(
        here,
        menu(
            "cleanup",
            t!("Clean up this folder: build output and package caches"),
            I::BrushCleaning,
            MenuEvent::Cleanup { folder: folder.clone() },
        ),
    );
    add(
        here,
        menu(
            "export-ncdu",
            t!("Export this folder as ncdu JSON…"),
            I::FileBraces,
            MenuEvent::ExportNcdu { folder: folder.clone() },
        ),
    );
    add(
        sendable,
        menu(
            "send-pc",
            match entries {
                [only] => t!("Send “{name}” to another PC", name = only.name),
                _ => t!("Send this folder to another PC"),
            },
            I::Send,
            MenuEvent::SendToPc { path: sent.clone() },
        ),
    );
    add(
        sendable,
        menu(
            "share-network",
            match entries {
                [only] => t!("Share “{name}” on this network: a link for a phone", name = only.name),
                _ => t!("Share this folder on this network: a link for a phone"),
            },
            I::Wifi,
            MenuEvent::ShareOnNetwork { path: sent },
        ),
    );
    add(
        here && !inputs.finding,
        menu(
            "receive-pc",
            t!("Receive from another PC here"),
            I::Download,
            MenuEvent::ReceiveHere { folder: folder.clone() },
        ),
    );
    add(
        inputs.sending,
        Command::new(
            "sending",
            t!("Sending to another PC: the tickets, and Stop"),
            I::Send,
            Kind::Action,
            Run::Share(ShareView::Send),
        ),
    );
    add(
        inputs.serving,
        Command::new(
            "serving",
            t!("Shared on this network: the links, and Stop"),
            I::Wifi,
            Kind::Action,
            Run::Share(ShareView::Network),
        ),
    );
    let targets = if chosen {
        inputs.selected.clone()
    } else {
        vec![folder.clone()]
    };
    add(
        on_disk_only && (place.in_folder || chosen),
        menu(
            "who-locks",
            if chosen {
                t!("What's locking the selection?")
            } else {
                t!("What's locking this folder?")
            },
            I::Lock,
            MenuEvent::Locks { paths: targets.clone() },
        ),
    );
    add(
        on_disk_only && !place.closed_items && (place.in_folder || chosen),
        menu(
            "defender",
            if chosen {
                t!("Scan the selection with Microsoft Defender")
            } else {
                t!("Scan this folder with Microsoft Defender")
            },
            I::ShieldCheck,
            MenuEvent::Defender { paths: targets },
        ),
    );
    let checked = match inputs.selected.as_slice() {
        [only] => only.clone(),
        _ => folder,
    };
    let can_check = on_disk_only && !place.closed_items && (place.in_folder || one);
    add(
        can_check,
        menu(
            "secrets",
            t!("Look for passwords and keys left in files"),
            I::KeyRound,
            MenuEvent::Secrets { path: checked.clone() },
        ),
    );
    add(
        can_check,
        menu(
            "yara",
            t!("Check with YARA rules"),
            I::ScanSearch,
            MenuEvent::Yara { path: checked },
        ),
    );
    out
}

/// Everything the palette offers: the folders, then the commands (`paletteCommands`).
pub fn all(inputs: &Inputs) -> Vec<Command> {
    let mut commands = folders(inputs);
    commands.extend(actions(inputs));
    commands
}

// ---- Ranking -------------------------------------------------------------------------------------------

/// Typed text that's a location rather than a search: `C:\…`, `\\server`, `~`, `%APPDATA%`, `.\…` (`looksLikePath`).
pub fn looks_like_path(text: &str) -> bool {
    let text = text.trim_matches(glint_model::js::is_js_space);
    let mut chars = text.chars();
    let Some(first) = chars.next() else { return false };
    let second = chars.next();
    if first.is_ascii_alphabetic() && second == Some(':') {
        // `c:` alone, `c:\`, `c:/` and `c:\anything`.
        return true;
    }
    if text.starts_with("\\\\") || first == '~' || first == '%' {
        return true;
    }
    // `.\`, `./`, `..\`, `../`.
    let after_dots = text.strip_prefix("..").or_else(|| text.strip_prefix('.'));
    after_dots.is_some_and(|rest| rest.starts_with(['\\', '/']))
}

/// The query as an expression, when it begins with `=` (`expression`): `Some("")` for a bare `=`.
pub fn expression_of(query: &str) -> Option<String> {
    let rest = query
        .trim_start_matches(glint_model::js::is_js_space)
        .strip_prefix('=')?;
    Some(rest.trim_matches(glint_model::js::is_js_space).to_string())
}

/// The commands that match `query`, best first, as the list shows them (without the calculator, and without
/// "Go to"; see [`results`]).
///
/// Nothing typed: the folders used most, then every command. Otherwise a match in the label counts more than
/// one only in a folder's path, where the letters typed must be close together (in a long path they'd turn up
/// anywhere).
pub fn rank(commands: &[Command], query: &str) -> Vec<Command> {
    let q = query.trim_matches(glint_model::js::is_js_space);
    if q.is_empty() {
        let mut folders: Vec<&Command> = commands.iter().filter(|c| c.kind == Kind::Folder).collect();
        folders.sort_by(|a, b| b.weight.total_cmp(&a.weight));
        return folders
            .into_iter()
            .take(FOLDERS_WHEN_EMPTY)
            .cloned()
            .chain(
                commands
                    .iter()
                    .filter(|c| c.kind == Kind::Action && !c.search_only)
                    .cloned(),
            )
            .collect();
    }
    let mut scored: Vec<(f64, &Command)> = commands
        .iter()
        .filter_map(|c| {
            let by_label = fuzzy_score(q, &c.label, false);
            let by_detail = c.detail.as_ref().and_then(|d| fuzzy_score(q, d, true));
            let score = match (by_label, by_detail) {
                (Some(label), _) => label + 2.,
                (None, Some(detail)) => detail - 1.,
                (None, None) => return None,
            };
            Some((score + c.weight, c))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(MOST_MATCHES).map(|(_, c)| c.clone()).collect()
}

/// An expression's answer, or why there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    /// The expression it answers (a later answer for an older expression isn't shown).
    pub expression: String,
    /// The value, or the reason.
    pub text: String,
    /// Whether it is a value.
    pub ok: bool,
}

/// The calculator's rows for `expression` (what follows the `=`): the hint for a bare `=`, a wait or the
/// reason for a failure, or the answer, with its hexadecimal, binary and byte-size forms for a whole number.
pub fn calculator_rows(expression: &str, answer: Option<&Answer>) -> Vec<Command> {
    let calc =
        |id: &str, label: SharedString| Command::new(id, label, IconName::Calculator, Kind::Action, Run::Nothing);
    if expression.is_empty() {
        return vec![calc(
            "calc",
            t!("Type a sum: 3 ft to cm, 1 GiB / (3 MB/s) to s, 0x1F + 1"),
        )];
    }
    let shown = answer.filter(|a| a.expression == expression);
    let Some(shown) = shown.filter(|a| a.ok) else {
        let mut row = calc(
            "calc",
            shown.map_or_else(|| SharedString::from("…"), |a| a.text.clone().into()),
        );
        row.detail = shown.map(|_| expression.to_string().into());
        return vec![row];
    };
    let copies = |id: &str, label: String, detail: SharedString, icon: IconName| {
        let mut row = Command::new(id, label.clone(), icon, Kind::Action, Run::Copy(label));
        row.detail = Some(detail);
        row.shortcut = Some(t!("Enter copies"));
        row
    };
    let mut rows = vec![copies(
        "calc",
        format!("= {}", shown.text),
        expression.to_string().into(),
        IconName::Calculator,
    )];
    // The first row copies the bare value, not "= value".
    rows[0].run = Run::Copy(shown.text.clone());
    let clean: String = shown
        .text
        .chars()
        .filter(|c| !matches!(c, ',' | '_') && !glint_model::js::is_js_space(*c))
        .collect();
    let digits = clean.strip_prefix(['+', '-']).unwrap_or(&clean);
    let whole = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    // Number.isSafeInteger: up to 2^53 - 1.
    let parsed = if whole { clean.parse::<i64>().ok() } else { None };
    if let Some(n) = parsed.filter(|n| (0..=9_007_199_254_740_991).contains(n)) {
        let n = n as u64; // 0 <= n by the filter above
        rows.push(copies(
            "calc-hex",
            format!("0x{n:X}"),
            t!("Hexadecimal"),
            IconName::Hash,
        ));
        if n <= 0xffff_ffff {
            rows.push(copies("calc-bin", format!("0b{n:b}"), t!("Binary"), IconName::Binary));
        }
        if n >= 1024 {
            rows.push(copies(
                "calc-bytes",
                format_size(n),
                t!(
                    "{count} bytes",
                    count = format_count(usize::try_from(n).unwrap_or(usize::MAX))
                ),
                IconName::HardDrive,
            ));
        }
    }
    rows
}

/// The "Go to <text>" row for typed text that looks like a path.
pub fn go_row(text: &str) -> Command {
    Command::new(
        "go",
        t!("Go to {path}", path = text),
        IconName::CornerDownLeft,
        Kind::Action,
        Run::GoTo(text.to_string()),
    )
}

/// What the list shows for `query`: the calculator's rows after an `=`; otherwise [`rank`]'s matches, with "Go
/// to" first when the text looks like a path.
pub fn results(commands: &[Command], query: &str, answer: Option<&Answer>) -> Vec<Command> {
    if let Some(expression) = expression_of(query) {
        return calculator_rows(&expression, answer);
    }
    let q = query.trim_matches(glint_model::js::is_js_space);
    let found = rank(commands, q);
    if !looks_like_path(q) {
        return found;
    }
    std::iter::once(go_row(q)).chain(found).collect()
}

/// The byte ranges of `text` that `query` matched, merged where letters touch: what to highlight. Empty when
/// nothing is typed or nothing matches. `tight` is for a folder's path.
pub fn highlight_ranges(text: &str, query: &str, tight: bool) -> Vec<Range<usize>> {
    let q = query.trim_matches(glint_model::js::is_js_space);
    if q.is_empty() {
        return Vec::new();
    }
    let Some(indices) = fuzzy_indices(q, text, tight) else {
        return Vec::new();
    };
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for at in indices {
        // `fuzzy_indices` gives byte offsets of characters, each on a boundary.
        let Some(ch) = text.get(at..).and_then(|rest| rest.chars().next()) else {
            continue;
        };
        let end = at + ch.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == at => last.end = end,
            _ => ranges.push(at..end),
        }
    }
    ranges
}
