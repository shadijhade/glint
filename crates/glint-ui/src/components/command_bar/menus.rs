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
use glint_model::paths::base_name;
use glint_model::store::ViewMode;

// ---- Keys and tooltips -------------------------------------------------------------------------------------

/// The first key bound to `action`, as menus word it ("Ctrl+Shift+N"; Delete is "Del"), from `actions::BINDINGS`.
pub fn keys_of(action: &dyn Action) -> Option<String> {
    BINDINGS
        .iter()
        .find(|binding| action.partial_eq(&*(binding.action)()))
        .map(|binding| key_text(binding.keys))
}

/// A tooltip: `template` translated ("Cut ({0})" is a catalog's text, so the shortcut can sit where the language
/// puts it), then `{0}`, `{1}`… replaced by the keys `BINDINGS` gives the actions.
pub fn keyed(template: &'static str, actions: &[&dyn Action]) -> SharedString {
    keyed_with(template, actions, &[])
}

/// [`keyed`] with `{name}` placeholders (a count, a file's name) filled in after the keys, the way `t!` does.
pub fn keyed_with(template: &'static str, actions: &[&dyn Action], vars: &[(&str, &str)]) -> SharedString {
    let mut text = glint_model::i18n::translate(i18n::language(), template).to_string();
    for (i, action) in actions.iter().enumerate() {
        text = text.replace(&format!("{{{i}}}"), &keys_of(*action).unwrap_or_default());
    }
    for (name, value) in vars {
        text = text.replace(&format!("{{{name}}}"), value);
    }
    text.into()
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
    /// The flat view of the folder (every file below it as one list), on or off.
    Flat,
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
            Pick::Flat => "flat".into(),
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
            "flat" => Pick::Flat,
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
            .with_picture(kind.icon.as_deref()),
        )
    }));
    items
}

// ---- The grid's tile sizes ----------------------------------------------------------------------------------

/// The grid's tile sizes, under the arrow by the view buttons: larger, smaller, then each size (`showSizes`).
pub fn sizes_menu(tile: TileSize) -> Vec<Node> {
    let bigger = zoom_tiles(tile.as_str(), 1);
    let smaller = zoom_tiles(tile.as_str(), -1);
    let mut nodes = Vec::new();
    // A step that can't be taken is not offered.
    if bigger != tile {
        nodes.push(Node::Item(picked(
            t!("Larger tiles"),
            IconName::ZoomIn,
            Pick::Larger,
            Some(&crate::actions::GridLarger),
        )));
    }
    if smaller != tile {
        nodes.push(Node::Item(picked(
            t!("Smaller tiles"),
            IconName::ZoomOut,
            Pick::Smaller,
            Some(&crate::actions::GridSmaller),
        )));
    }
    if !nodes.is_empty() {
        nodes.push(Node::Separator);
    }
    nodes.extend(tile_rows(tile));
    nodes
}

/// One row for each tile size, the one in use checked.
fn tile_rows(tile: TileSize) -> Vec<Node> {
    TILE_SIZES
        .iter()
        .map(|spec| {
            let mut entry = picked(
                SharedString::from(glint_model::i18n::translate(i18n::language(), spec.label).to_string()),
                IconName::Check,
                Pick::Tiles(spec.size),
                (spec.size == TileSize::Medium).then_some(&crate::actions::GridReset as &dyn Action),
            )
            .checked(spec.size == tile);
            entry.icon = None;
            Node::Item(entry)
        })
        .collect()
}

// ---- View -------------------------------------------------------------------------------------------------

/// When even the icons don't fit, the view toggles in a menu (and narrower still, list or grid too)
/// (`showToggles`).
pub fn view_menu(f: &Facts, fold: usize) -> Vec<Node> {
    let mut nodes = Vec::new();
    // Where views can't be chosen (This PC) they are not offered.
    if fold >= 5 && f.can_view() {
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
            let mut entry =
                picked(label, IconName::Check, Pick::View(view), maker).checked(!f.mapped && f.view == view);
            entry.icon = None;
            nodes.push(Node::Item(entry));
        }
        // The grid's sizes, which the arrow by the views holds while the bar is wider.
        if f.view == ViewMode::Grid && !f.mapped {
            nodes.push(Node::Menu {
                label: t!("Tile size"),
                icon: Some(IconName::LayoutGrid),
                disabled: false,
                items: tile_rows(f.tile_size),
            });
        }
        nodes.push(Node::Separator);
    }
    nodes.extend(toggle_rows(f));
    nodes
}

/// The on/off rows: hidden items, the flat view (where it can be shown), preview pane, dual pane, compare panes.
fn toggle_rows(f: &Facts) -> Vec<Node> {
    let rows = [
        (
            t!("Show hidden items"),
            Pick::Hidden,
            &crate::actions::ToggleHidden as &dyn Action,
            f.show_hidden,
        ),
        (
            t!("Flat view: every file in this folder and its subfolders"),
            Pick::Flat,
            &crate::actions::FlatView,
            f.flat,
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
        // The flat view is for a folder (and the way back out of it).
        .filter(|(_, pick, _, _)| *pick != Pick::Flat || f.flat || f.can_flat)
        .map(|(label, pick, action, on)| {
            let mut entry = picked(label, IconName::Check, pick, Some(action)).checked(on);
            entry.icon = None;
            Node::Item(entry)
        })
        .collect()
}

// ---- Edits (narrowest) ------------------------------------------------------------------------------------

/// The Compare row, for the pair that is chosen: none while there is no pair (it would only be greyed).
fn compare_row(pair: Option<&Pair>) -> Option<Node> {
    let pair = pair?;
    let mut entry = Entry::ask(
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
    );
    entry.hint = keys_of(&crate::actions::CompareFiles).map(SharedString::from);
    Some(Node::Item(entry))
}

/// Narrowest: what is done to the selection, in a menu by New folder (`showEdits`). What can't apply here is not
/// listed; a read-only place keeps its rows greyed (the note beside the island says why).
pub fn edits_menu(f: &Facts) -> Vec<Node> {
    let some = f.selected > 0;
    let mut nodes = Vec::new();
    if some {
        nodes.push(Node::Item(
            Entry::command(t!("Cut"), IconName::Scissors, Command::Cut).disabled(f.read_only),
        ));
        nodes.push(Node::Item(Entry::command(t!("Copy"), IconName::Copy, Command::Copy)));
    }
    if f.can_make() {
        nodes.push(Node::Item(
            Entry::command(t!("Paste"), IconName::ClipboardPaste, Command::Paste).disabled(f.read_only),
        ));
    }
    if f.dual && f.other && some {
        nodes.push(Node::Item(picked(
            t!("Copy to the other pane"),
            IconName::CopyPlus,
            Pick::CopyToOther,
            None,
        )));
        nodes.push(Node::Item(
            picked(
                t!("Move to the other pane"),
                IconName::FolderOutput,
                Pick::MoveToOther,
                None,
            )
            .disabled(f.read_only),
        ));
    }
    if some {
        nodes.extend(compare_row(f.pair.as_ref()));
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
            .disabled(f.read_only),
        ));
        nodes.push(Node::Item(
            Entry::command(t!("Delete"), IconName::Trash, Command::Delete)
                .danger(true)
                .disabled(f.read_only),
        ));
    }
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
    let paths_or_folder = if paths.is_empty() {
        vec![folder.clone()]
    } else {
        paths.clone()
    };
    // A row that can't apply here is left out, not greyed.
    let ask = |label: SharedString, icon: IconName, event: MenuEvent, applies: bool| {
        applies.then(|| Node::Item(Entry::ask(label, icon, event)))
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
        let before = nodes.len();
        if f.can_map() {
            nodes.push(Node::Item(picked(
                if f.mapped {
                    t!("Back to the file list")
                } else {
                    t!("Disk usage")
                },
                IconName::LayoutDashboard,
                Pick::DiskUsage,
                None,
            )));
        }
        if f.can_size(sizing) {
            nodes.push(Node::Item(picked(
                if sizing {
                    t!("Stop calculating folder sizes")
                } else {
                    t!("Folder sizes")
                },
                IconName::HardDrive,
                Pick::FolderSizes,
                None,
            )));
        }
        if nodes.len() > before {
            nodes.push(Node::Separator);
        }
    }
    nodes.push(Node::Item(picked(
        t!("Terminal here, under the files"),
        IconName::SquareTerminal,
        Pick::TerminalPane,
        Some(&crate::actions::ToggleTerminal),
    )));
    if in_folder {
        nodes.push(Node::Item(Entry::new(
            t!("Open in Terminal"),
            IconName::SquareTerminal,
            Run::Do(Task::OpenTerminal(folder.clone())),
        )));
    }
    nodes.push(Node::Heading(t!("Inspect")));
    nodes.extend(compare_row(f.pair.as_ref()));
    nodes.extend(ask(
        t!("File X-Ray: hashes and hidden streams"),
        IconName::FingerprintPattern,
        MenuEvent::XRay {
            path: file.clone().unwrap_or_default(),
        },
        file.is_some(),
    ));
    nodes.extend(ask(
        if paths.is_empty() {
            t!("What's locking this folder?")
        } else {
            t!("What's locking the selection?")
        },
        IconName::Lock,
        MenuEvent::Locks {
            paths: paths_or_folder.clone(),
        },
        in_folder || !paths.is_empty(),
    ));
    nodes.extend(ask(
        t!("Recent changes in this folder"),
        IconName::FileClock,
        MenuEvent::Timeline { folder: folder.clone() },
        in_folder,
    ));
    // Everywhere: no folder.
    nodes.extend(ask(
        t!("Moved and deleted: where did a file go?"),
        IconName::ClockArrowLeft,
        MenuEvent::History { folder: String::new() },
        true,
    ));
    nodes.push(Node::Heading(t!("Clean up")));
    nodes.extend(ask(
        t!("Find duplicate files in this folder"),
        IconName::Files,
        MenuEvent::Duplicates { folder: folder.clone() },
        in_folder,
    ));
    nodes.extend(ask(
        t!("Clean up this folder…"),
        IconName::BrushCleaning,
        MenuEvent::Cleanup { folder: folder.clone() },
        in_folder && !read_only,
    ));
    let pictures: Vec<String> = paths
        .iter()
        .filter(|path| crate::dialogs::convert::can_convert(path))
        .cloned()
        .collect();
    nodes.extend(ask(
        t!("Convert the selected pictures…"),
        IconName::ImageDown,
        MenuEvent::ConvertPictures {
            paths: pictures.clone(),
        },
        !pictures.is_empty(),
    ));
    // An empty folder asks for the rules in general, not for this folder.
    nodes.extend(ask(
        t!("Folder rules: sort out what comes in…"),
        IconName::WandSparkles,
        MenuEvent::Rule {
            folder: if in_folder && !read_only {
                folder.clone()
            } else {
                String::new()
            },
        },
        true,
    ));
    nodes.push(Node::Heading(t!("Share")));
    nodes.extend(ask(
        if paths.is_empty() {
            t!("Share this folder…")
        } else {
            t!("Share the selection…")
        },
        IconName::Share2,
        MenuEvent::Share {
            paths: paths_or_folder.clone(),
        },
        !f.bin && (!paths.is_empty() || in_folder),
    ));
    nodes.push(Node::Heading(t!("Check")));
    nodes.extend(ask(
        if paths.is_empty() {
            t!("Scan this folder with Microsoft Defender")
        } else {
            t!("Scan the selection with Microsoft Defender")
        },
        IconName::ShieldCheck,
        MenuEvent::Defender { paths: paths_or_folder },
        !read_only && (in_folder || !paths.is_empty()),
    ));
    // A heading with nothing under it goes.
    let mut kept: Vec<Node> = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        if matches!(node, Node::Heading(_)) && !matches!(nodes.get(i + 1), Some(Node::Item(_) | Node::Menu { .. })) {
            continue;
        }
        kept.push(node.clone());
    }
    kept
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

// ---- The basket ------------------------------------------------------------------------------------------------

/// How many of the basket's items its menu lists before it says how many more there are.
const BASKET_LISTED: usize = 12;

/// The basket's menu: Move here, Copy here, Zip and Batch rename for what it holds, the list of what it holds (each
/// row takes its item out), and Empty. Move and Copy need a folder that can take them.
pub fn basket_menu(items: &[String], f: &Facts) -> Vec<Node> {
    if items.is_empty() {
        return Vec::new();
    }
    // Move and Copy need a folder that can take them.
    let can_drop_here = f.in_folder && !f.find && !f.read_only;
    let mut nodes = Vec::new();
    if can_drop_here {
        nodes.push(Node::Item(Entry::ask(
            t!("Move here"),
            IconName::FolderInput,
            MenuEvent::BasketMove,
        )));
        nodes.push(Node::Item(Entry::ask(
            t!("Copy here"),
            IconName::Copy,
            MenuEvent::BasketCopy,
        )));
    }
    nodes.push(Node::Item(Entry::ask(
        t!("Zip"),
        IconName::Package,
        MenuEvent::BasketZip,
    )));
    nodes.push(Node::Item(Entry::ask(
        t!("Batch rename"),
        IconName::PenLine,
        MenuEvent::BasketRename,
    )));
    nodes.push(Node::Separator);
    let mut listed: Vec<Node> = items
        .iter()
        .take(BASKET_LISTED)
        .map(|path| {
            Node::Item(Entry::ask(
                t!("Take out “{name}”", name = base_name(path)),
                IconName::X,
                MenuEvent::BasketRemove { path: path.clone() },
            ))
        })
        .collect();
    if items.len() > BASKET_LISTED {
        listed.push(Node::Heading(t!("and {n} more", n = items.len() - BASKET_LISTED)));
    }
    nodes.push(Node::Menu {
        label: t!("What is in it ({n})", n = items.len()),
        icon: Some(IconName::List),
        disabled: false,
        items: listed,
    });
    nodes.push(Node::Item(
        Entry::ask(t!("Empty the basket"), IconName::Trash, MenuEvent::BasketEmpty).danger(true),
    ));
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
            icon: Some(std::sync::Arc::new(glint_core::thumbs::Pixels {
                width: 1,
                height: 1,
                bgra: vec![0, 0, 0, 255],
            })),
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
