//! The bar's menus as plain data (`showNew`, `showSizes`, `showTools`, `showToggles`, `showEdits` and `taskMenu` in
//! CommandBar.tsx, `newMenuItems` in newMenu.ts), drawn by the menus' own [`MenuView`](crate::components::menus).
//!
//! A menu is a list of [`Node`]s. What a row does is its [`Run`]:
//! - a workspace command is `Run::Command` (Cut, Copy, Paste, Rename, Delete, New folder, Compare);
//! - what a dialog or another part does is `Run::Ask(MenuEvent)`, which the bar passes on as
//!   [`CommandBarEvent::Menu`](super::CommandBarEvent), so the window's one `MenuEvent` handler answers it;
//! - what only the bar knows how to do is a [`Pick`], carried in a `Run`.
//!
//! **Why a `Pick` rides in a `Run`.** The menu view hands back the `Run` of the row chosen and nothing else, and
//! `Run` has no variant for "call the bar". A `Pick` is encoded as `Task::Navigate("command-bar:<id>")` and decoded
//! by the bar when the row comes back; nothing else ever sees one (the bar draws these menus itself, not the
//! shared `MenuHost`, which would try to open the folder).
// (A `Run::Custom(u32)` in menus/spec.rs, a variant a caller can fill, would let a row carry its own meaning and
// this encoding go.)

use super::facts::{Facts, Pair};
use crate::actions::BINDINGS;
use crate::components::menus::{key_text, Command, Entry, MenuEvent, Node, Run, Task};
use crate::explorer::NewKind;
use crate::kit::prelude::*;
use crate::{i18n, t};
use glint_core::projects::TaskGroup;
use glint_model::entries::FileEntry;
use glint_model::grid::{zoom_tiles, TileSize, TILE_SIZES};
use glint_model::paths::{base_name, is_remote_path};
use glint_model::store::ViewMode;

// ---- Keys and tooltips -------------------------------------------------------------------------------------

/// The first key bound to `action`, as menus word it ("Ctrl+Shift+N"; Delete is "Del"), from `actions::BINDINGS`.
pub fn keys_of(action: &dyn Action) -> Option<String> {
    BINDINGS
        .iter()
        .find(|binding| action.partial_eq(&*(binding.action)()))
        .map(|binding| key_text(binding.keys))
}

/// A tooltip: `template` with `{0}`, `{1}`… replaced by the keys `BINDINGS` gives the actions, translated. The
/// catalog's keys are whole sentences with the shortcut in them ("Cut (Ctrl+X)"), as the TypeScript wrote them,
/// so the finished English is looked up; a sentence nobody translated stays English.
pub fn keyed(template: &'static str, actions: &[&dyn Action]) -> SharedString {
    keyed_with(template, actions, &[])
}

/// [`keyed`] with `{name}` placeholders (a count, a file's name) filled in after the lookup, the way `t!` does.
pub fn keyed_with(template: &'static str, actions: &[&dyn Action], vars: &[(&str, &str)]) -> SharedString {
    let mut english = template.to_string();
    for (i, action) in actions.iter().enumerate() {
        english = english.replace(&format!("{{{i}}}"), &keys_of(*action).unwrap_or_default());
    }
    glint_model::i18n::t(i18n::language(), &english, vars)
        .into_owned()
        .into()
}

/// A row that runs a workspace action by `Pick`, showing the key the action has.
fn picked(label: SharedString, icon: IconName, pick: Pick, keys: Option<&dyn Action>) -> Entry {
    let mut entry = Entry::new(label, icon, pick.run());
    entry.hint = keys.and_then(keys_of).map(SharedString::from);
    entry
}

// ---- What only the bar does ----------------------------------------------------------------------------------

/// The prefix that marks a `Task::Navigate` as a [`Pick`].
const SCHEME: &str = "command-bar:";

/// What a row of the bar's own menus asks of the bar.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// Show hidden items, on or off.
    Hidden,
    /// The preview pane, on or off.
    Preview,
    /// Two panes, or one.
    Dual,
    /// Highlight differences between the panes, on or off.
    ComparePanes,
    /// List, grid or columns for the tab (a disk usage map goes first).
    View(ViewMode),
    /// Tiles of this size, in the grid.
    Tiles(TileSize),
    /// Tiles one size up, in the grid.
    Larger,
    /// Tiles one size down, in the grid.
    Smaller,
    /// The command palette.
    Palette,
    /// The disk usage map, or back to the files.
    DiskUsage,
    /// Measure every folder's size here, or stop.
    FolderSizes,
    /// The terminal pane under the files.
    TerminalPane,
    /// Copy the selection into the other pane's folder.
    CopyToOther,
    /// Move it there.
    MoveToOther,
    /// Run a project task: its group in the bar's list, then the task in the group.
    Task { group: usize, task: usize },
}

impl Pick {
    /// The row's `Run`.
    pub fn run(&self) -> Run {
        Run::Do(Task::Navigate(format!("{SCHEME}{}", self.id())))
    }

    fn id(&self) -> String {
        match self {
            Pick::Hidden => "hidden".into(),
            Pick::Preview => "preview".into(),
            Pick::Dual => "dual".into(),
            Pick::ComparePanes => "compare-panes".into(),
            Pick::View(ViewMode::List) => "view:list".into(),
            Pick::View(ViewMode::Grid) => "view:grid".into(),
            Pick::View(ViewMode::Columns) => "view:columns".into(),
            Pick::Tiles(size) => format!("tiles:{}", size.as_str()),
            Pick::Larger => "larger".into(),
            Pick::Smaller => "smaller".into(),
            Pick::Palette => "palette".into(),
            Pick::DiskUsage => "disk-usage".into(),
            Pick::FolderSizes => "folder-sizes".into(),
            Pick::TerminalPane => "terminal-pane".into(),
            Pick::CopyToOther => "copy-other".into(),
            Pick::MoveToOther => "move-other".into(),
            Pick::Task { group, task } => format!("task:{group}:{task}"),
        }
    }

    /// The `Pick` a chosen `Run` carries, if it carries one.
    pub fn of(run: &Run) -> Option<Pick> {
        let Run::Do(Task::Navigate(text)) = run else {
            return None;
        };
        let id = text.strip_prefix(SCHEME)?;
        Some(match id {
            "hidden" => Pick::Hidden,
            "preview" => Pick::Preview,
            "dual" => Pick::Dual,
            "compare-panes" => Pick::ComparePanes,
            "view:list" => Pick::View(ViewMode::List),
            "view:grid" => Pick::View(ViewMode::Grid),
            "view:columns" => Pick::View(ViewMode::Columns),
            "larger" => Pick::Larger,
            "smaller" => Pick::Smaller,
            "palette" => Pick::Palette,
            "disk-usage" => Pick::DiskUsage,
            "folder-sizes" => Pick::FolderSizes,
            "terminal-pane" => Pick::TerminalPane,
            "copy-other" => Pick::CopyToOther,
            "move-other" => Pick::MoveToOther,
            other => {
                if let Some(size) = other.strip_prefix("tiles:") {
                    Pick::Tiles(TILE_SIZES.iter().find(|spec| spec.size.as_str() == size)?.size)
                } else {
                    let mut parts = other.strip_prefix("task:")?.split(':');
                    let group = parts.next()?.parse().ok()?;
                    let task = parts.next()?.parse().ok()?;
                    Pick::Task { group, task }
                }
            }
        })
    }
}

// ---- New --------------------------------------------------------------------------------------------------

/// What New makes in the open folder: a folder, a text document, and the kinds of file Windows' New menu has
/// (Word, Excel, a zip…) (`newMenuItems(true)`). Each kind shows its own icon, as in the folder's New menu.
pub fn new_menu(kinds: &[NewKind]) -> Vec<Node> {
    let mut items = vec![
        Node::Item(Entry::command(t!("Folder"), IconName::FolderPlus, Command::NewFolder)),
        Node::Item(Entry::ask(
            t!("Text document"),
            IconName::FileText,
            MenuEvent::NewTextDocument,
        )),
    ];
    if !kinds.is_empty() {
        items.push(Node::Separator);
    }
    items.extend(kinds.iter().map(|kind| {
        Node::Item(
            Entry::ask(
                SharedString::from(kind.name.clone()),
                IconName::FilePlus,
                MenuEvent::NewOfKind { ext: kind.ext.clone() },
            )
            .with_data_url_picture(kind.icon.as_deref()),
        )
    }));
    items
}

// ---- The grid's tile sizes ----------------------------------------------------------------------------------

/// The grid's tile sizes, under the arrow by the view buttons: larger, smaller, then each size (`showSizes`).
pub fn sizes_menu(tile: TileSize) -> Vec<Node> {
    let bigger = zoom_tiles(tile.as_str(), 1);
    let smaller = zoom_tiles(tile.as_str(), -1);
    let mut nodes = vec![
        Node::Item(
            picked(
                t!("Larger tiles"),
                IconName::ZoomIn,
                Pick::Larger,
                Some(&crate::actions::GridLarger),
            )
            .disabled(bigger == tile),
        ),
        Node::Item(
            picked(
                t!("Smaller tiles"),
                IconName::ZoomOut,
                Pick::Smaller,
                Some(&crate::actions::GridSmaller),
            )
            .disabled(smaller == tile),
        ),
        Node::Separator,
    ];
    nodes.extend(TILE_SIZES.iter().map(|spec| {
        let mut entry = picked(
            SharedString::from(glint_model::i18n::translate(i18n::language(), spec.label).to_string()),
            IconName::Check,
            Pick::Tiles(spec.size),
            (spec.size == TileSize::Medium).then_some(&crate::actions::GridReset as &dyn Action),
        )
        .checked(spec.size == tile);
        entry.icon = None;
        Node::Item(entry)
    }));
    nodes
}

// ---- View -------------------------------------------------------------------------------------------------

/// When even the icons don't fit, the view toggles in a menu (and narrower still, list or grid too)
/// (`showToggles`).
pub fn view_menu(f: &Facts, fold: usize) -> Vec<Node> {
    let can_view = f.in_folder || f.find || f.bin || is_remote_path(&f.path);
    let mut nodes = Vec::new();
    if fold >= 5 {
        for (view, label, maker) in [
            (
                ViewMode::List,
                t!("List"),
                Some(&crate::actions::ListView as &dyn Action),
            ),
            (
                ViewMode::Grid,
                t!("Grid, with thumbnails"),
                Some(&crate::actions::GridView as &dyn Action),
            ),
            (
                ViewMode::Columns,
                t!("Columns"),
                Some(&crate::actions::ColumnsView as &dyn Action),
            ),
        ] {
            let mut entry = picked(label, IconName::Check, Pick::View(view), maker)
                .checked(!f.mapped && f.view == view)
                .disabled(!can_view);
            entry.icon = None;
            nodes.push(Node::Item(entry));
        }
        nodes.push(Node::Separator);
    }
    nodes.extend(toggle_rows(f));
    nodes
}

/// The four on/off rows: hidden items, preview pane, dual pane, compare panes.
fn toggle_rows(f: &Facts) -> Vec<Node> {
    let rows = [
        (
            t!("Show hidden items"),
            Pick::Hidden,
            &crate::actions::ToggleHidden as &dyn Action,
            f.show_hidden,
        ),
        (
            t!("Preview pane"),
            Pick::Preview,
            &crate::actions::TogglePreview,
            f.show_preview,
        ),
        (t!("Dual pane"), Pick::Dual, &crate::actions::DualPane, f.dual),
        (
            t!("Compare panes"),
            Pick::ComparePanes,
            &crate::actions::ComparePanes,
            f.compare_panes,
        ),
    ];
    rows.into_iter()
        .map(|(label, pick, action, on)| {
            let mut entry = picked(label, IconName::Check, pick, Some(action)).checked(on);
            entry.icon = None;
            Node::Item(entry)
        })
        .collect()
}

// ---- Edits (narrowest) ------------------------------------------------------------------------------------

/// The Compare row: "Compare “a” and “b”" when there is a pair, else the words that say what to select, greyed.
fn compare_row(pair: Option<&Pair>, otherwise: SharedString) -> Node {
    let keys = keys_of(&crate::actions::CompareFiles).map(SharedString::from);
    let mut entry = match pair {
        Some(pair) => Entry::ask(
            t!(
                "Compare “{a}” and “{b}”",
                a = pair.left_name.as_str(),
                b = pair.right_name.as_str()
            ),
            IconName::GitCompareArrows,
            MenuEvent::Compare {
                left: pair.left.clone(),
                right: pair.right.clone(),
            },
        ),
        None => Entry::note(otherwise, IconName::GitCompareArrows),
    };
    entry.hint = keys;
    Node::Item(entry)
}

/// Narrowest: what is done to the selection, in a menu by New folder (`showEdits`).
pub fn edits_menu(f: &Facts) -> Vec<Node> {
    let none = f.selected == 0;
    let mut nodes = vec![
        Node::Item(Entry::command(t!("Cut"), IconName::Scissors, Command::Cut).disabled(none || f.read_only)),
        Node::Item(Entry::command(t!("Copy"), IconName::Copy, Command::Copy).disabled(none)),
        Node::Item(
            Entry::command(t!("Paste"), IconName::ClipboardPaste, Command::Paste)
                .disabled(!f.in_folder || f.find || f.read_only),
        ),
    ];
    if f.dual {
        nodes.push(Node::Item(
            picked(
                t!("Copy to the other pane"),
                IconName::CopyPlus,
                Pick::CopyToOther,
                None,
            )
            .disabled(none || !f.other),
        ));
        nodes.push(Node::Item(
            picked(
                t!("Move to the other pane"),
                IconName::FolderOutput,
                Pick::MoveToOther,
                None,
            )
            .disabled(none || !f.other || f.read_only),
        ));
    }
    if f.dual || f.pair.is_some() {
        nodes.push(compare_row(f.pair.as_ref(), t!("Compare selected files")));
    }
    nodes.push(Node::Separator);
    nodes.push(Node::Item(
        Entry::command(
            if f.selected > 1 {
                t!("Rename {n} items", n = f.selected)
            } else {
                t!("Rename")
            },
            IconName::PenLine,
            Command::Rename,
        )
        .disabled(none || f.read_only),
    ));
    nodes.push(Node::Item(
        Entry::command(t!("Delete"), IconName::Trash, Command::Delete)
            .danger(true)
            .disabled(none || f.read_only),
    ));
    nodes
}

// ---- Tools ------------------------------------------------------------------------------------------------

/// The tools for this folder or the selection, in one menu so the bar fits beside the preview pane
/// (`showTools`). `selected` is the selection in row order; `sizing` says folder sizes are being measured.
pub fn tools_menu(f: &Facts, selected: &[FileEntry], fold: usize, sizing: bool) -> Vec<Node> {
    let paths: Vec<String> = selected.iter().map(|e| e.path.clone()).collect();
    let file = match selected {
        [one] if !one.dir => Some(one.path.clone()),
        _ => None,
    };
    let (in_folder, read_only) = (f.in_folder, f.read_only);
    let folder = f.path.clone();
    // The one thing selected, else the folder.
    let one_or_folder = if paths.len() == 1 {
        paths[0].clone()
    } else {
        folder.clone()
    };
    let paths_or_folder = if paths.is_empty() {
        vec![folder.clone()]
    } else {
        paths.clone()
    };
    let ask = |label: SharedString, icon: IconName, event: MenuEvent, disabled: bool| {
        Node::Item(Entry::ask(label, icon, event).disabled(disabled))
    };
    let mut nodes = Vec::new();
    if fold >= 5 {
        nodes.push(Node::Item(picked(
            t!("Command palette"),
            IconName::Command,
            Pick::Palette,
            Some(&crate::actions::CommandPalette),
        )));
        nodes.push(Node::Separator);
    }
    // Folded in from the bar, when it's that narrow.
    if fold >= 4 {
        nodes.push(Node::Item(
            picked(
                if f.mapped {
                    t!("Back to the file list")
                } else {
                    t!("Disk usage")
                },
                IconName::LayoutDashboard,
                Pick::DiskUsage,
                None,
            )
            .disabled(!in_folder || f.find),
        ));
        nodes.push(Node::Item(
            picked(
                if sizing {
                    t!("Stop calculating folder sizes")
                } else {
                    t!("Folder sizes")
                },
                IconName::HardDrive,
                Pick::FolderSizes,
                None,
            )
            .disabled(!in_folder || f.find),
        ));
        nodes.push(Node::Separator);
    }
    nodes.push(Node::Item(picked(
        t!("Terminal here, under the files"),
        IconName::SquareTerminal,
        Pick::TerminalPane,
        Some(&crate::actions::ToggleTerminal),
    )));
    nodes.push(Node::Item(
        Entry::new(
            t!("Open in Terminal"),
            IconName::SquareTerminal,
            Run::Do(Task::OpenTerminal(folder.clone())),
        )
        .disabled(!in_folder),
    ));
    nodes.push(Node::Heading(t!("Inspect")));
    nodes.push(compare_row(f.pair.as_ref(), t!("Compare two files")));
    nodes.push(ask(
        t!("File X-Ray: hashes and hidden streams"),
        IconName::FingerprintPattern,
        MenuEvent::XRay {
            path: file.clone().unwrap_or_default(),
        },
        file.is_none(),
    ));
    nodes.push(ask(
        if paths.is_empty() {
            t!("What's locking this folder?")
        } else {
            t!("What's locking the selection?")
        },
        IconName::Lock,
        MenuEvent::Locks {
            paths: paths_or_folder.clone(),
        },
        !in_folder && paths.is_empty(),
    ));
    nodes.push(ask(
        t!("Recent changes in this folder"),
        IconName::FileClock,
        MenuEvent::Timeline { folder: folder.clone() },
        !in_folder,
    ));
    // Everywhere: no folder.
    nodes.push(ask(
        t!("Moved and deleted: where did a file go?"),
        IconName::ClockArrowLeft,
        MenuEvent::History { folder: String::new() },
        false,
    ));
    nodes.push(Node::Heading(t!("Clean up")));
    nodes.push(ask(
        t!("Find duplicate files in this folder"),
        IconName::Files,
        MenuEvent::Duplicates { folder: folder.clone() },
        !in_folder,
    ));
    nodes.push(ask(
        t!("Clean up this folder…"),
        IconName::BrushCleaning,
        MenuEvent::Cleanup { folder: folder.clone() },
        !in_folder || read_only,
    ));
    let pictures: Vec<String> = paths
        .iter()
        .filter(|path| crate::dialogs::convert::can_convert(path))
        .cloned()
        .collect();
    nodes.push(ask(
        t!("Convert the selected pictures…"),
        IconName::ImageDown,
        MenuEvent::ConvertPictures {
            paths: pictures.clone(),
        },
        pictures.is_empty(),
    ));
    // An empty folder asks for the rules in general, not for this folder.
    nodes.push(ask(
        t!("Folder rules: sort out what comes in…"),
        IconName::WandSparkles,
        MenuEvent::Rule {
            folder: if in_folder && !read_only {
                folder.clone()
            } else {
                String::new()
            },
        },
        false,
    ));
    nodes.push(Node::Heading(t!("Share")));
    let one_only = paths.len() > 1 || (!in_folder && paths.len() != 1);
    let sharing_off = read_only || f.bin || one_only;
    nodes.push(ask(
        if paths.len() == 1 {
            t!("Send the selection to another PC…")
        } else {
            t!("Send this folder to another PC…")
        },
        IconName::Send,
        MenuEvent::SendToPc {
            path: one_or_folder.clone(),
        },
        sharing_off,
    ));
    nodes.push(ask(
        if paths.len() == 1 {
            t!("Share the selection on this network…")
        } else {
            t!("Share this folder on this network…")
        },
        IconName::Wifi,
        MenuEvent::ShareOnNetwork {
            path: one_or_folder.clone(),
        },
        sharing_off,
    ));
    nodes.push(ask(
        t!("Receive from another PC here…"),
        IconName::Download,
        MenuEvent::ReceiveHere { folder: folder.clone() },
        !in_folder || read_only || f.find,
    ));
    nodes.push(Node::Heading(t!("Check")));
    nodes.push(ask(
        if paths.is_empty() {
            t!("Scan this folder with Microsoft Defender")
        } else {
            t!("Scan the selection with Microsoft Defender")
        },
        IconName::ShieldCheck,
        MenuEvent::Defender { paths: paths_or_folder },
        read_only || (!in_folder && paths.is_empty()),
    ));
    let one_only_check = read_only || (!in_folder && paths.len() != 1);
    nodes.push(ask(
        t!("Look for passwords and keys left in files"),
        IconName::KeyRound,
        MenuEvent::Secrets {
            path: one_or_folder.clone(),
        },
        one_only_check,
    ));
    nodes.push(ask(
        t!("Check with YARA rules"),
        IconName::ScanSearch,
        MenuEvent::Yara { path: one_or_folder },
        one_only_check,
    ));
    nodes
}

// ---- Run --------------------------------------------------------------------------------------------------

/// The Run menu: each project's tasks under its name, nearest first; an install for what a project needs that
/// isn't there (`taskMenu`).
pub fn tasks_menu(groups: &[TaskGroup]) -> Vec<Node> {
    let mut nodes = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        let name = match base_name(&group.folder) {
            "" => group.folder.as_str(),
            name => name,
        };
        nodes.push(Node::Heading(format!("{name} · {}", group.source).into()));
        match &group.missing {
            Some(tool) => nodes.push(Node::Item(Entry::ask(
                t!("Install {tool} to run these", tool = tool),
                IconName::Download,
                MenuEvent::InstallTool { tool: tool.clone() },
            ))),
            None => {
                for (k, task) in group.tasks.iter().enumerate() {
                    let label = if task.detail.is_empty() {
                        task.name.clone()
                    } else {
                        format!("{}: {}", task.name, task.detail)
                    };
                    nodes.push(Node::Item(Entry::new(
                        label.into(),
                        IconName::Play,
                        Pick::Task { group: g, task: k }.run(),
                    )));
                }
            }
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bars_new_menu_draws_each_kind_with_its_own_icon() {
        let kinds = [NewKind {
            ext: ".docx".into(),
            name: "Microsoft Word Document".into(),
            icon: Some("data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==".into()),
            by_app: false,
        }];
        let nodes = new_menu(&kinds);
        let word = nodes.iter().find_map(|node| match node {
            Node::Item(entry) if entry.label.as_ref() == "Microsoft Word Document" => Some(entry),
            _ => None,
        });
        assert!(word.is_some_and(|entry| entry.picture.is_some()));
        // Folder and Text document are drawn with their icons.
        assert!(nodes.iter().all(|node| match node {
            Node::Item(entry) if entry.label.as_ref() == "Folder" || entry.label.as_ref() == "Text document" => {
                entry.picture.is_none() && entry.icon.is_some()
            }
            _ => true,
        }));
    }
}
