//! What Ctrl+K lists and how it ranks it: the commands, the folders you use, the calculator's answers.
//!
//! Ported from src/components/explorer/CommandPalette.tsx (ranking, the calculator's rows, "Go to") and from
//! `paletteCommands` in Explorer.tsx (which commands and folders there are, and when).
//!
//! Everything here is plain data and plain functions, so the ranking and the lists are tested without a
//! window. A [`Command`] says what it does as a [`Run`] value (not a closure), which the view carries out.

use crate::actions::{self, Binding};
use crate::explorer::{Drive, KnownId, TabId};
use crate::kit::prelude::*;
use crate::{i18n, t};
use glint_model::format::{format_count, format_size};
use glint_model::grid::{zoom_tiles, TileSize, TILE_SIZES};
use glint_model::groups::{GroupBy, GROUP_BYS};
use glint_model::paths::{
    base_name, is_folder_path, is_gallery, is_recent, is_recycle_bin, is_remote_path, GALLERY, RECENT, RECYCLE_BIN,
    THIS_PC,
};
use glint_model::rank::{frecency, fuzzy_indices, fuzzy_score, Visit};
use glint_model::store::{RemoteConnection, RemoteKind, Theme};
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
    // PORT ME(search): the TypeScript also listed the saved searches here ("Saved search in <folder>"); running one
    // needs the find part of the explorer.
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
    let mut out: Vec<Command> = Vec::new();
    let mut push = |condition: bool, command: Command| {
        if condition {
            out.push(command);
        }
    };

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
    // PORT ME(ops): "Empty the Recycle Bin" needs the explorer's file operations.
    push(
        true,
        Command::action("new-tab", t!("New tab"), I::Plus, || Box::new(actions::NewTab)),
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
    // PORT ME(ops): the commands that do things to files (compress, extract, convert, encrypt, PDF, media, data,
    // song details, remove personal info, make smaller, copy/move to the other pane, undo, X-Ray, duplicates,
    // cleanup, timeline, locks, Defender, secrets, YARA, Send/Share, Receive, the copy queue's paste).
    // PORT ME(tools): scripts, Run <task>, "Open this folder in <editor>", "Open in Terminal", Open in File
    // Explorer, Add a remote connection, Folder rules, Moved and deleted (history), Compare two selected files.
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
