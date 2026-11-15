//! The Suggested section of a menu, as data: every row's key, the rows put forward, and the menu dressed with
//! its head and its suggestions (docs/gpui/aware/PLAN.md, F2 and section 5.5).
//!
//! Everything here reads what it is given and nothing else, so it needs no window. A suggested row is a copy of
//! a row the menu has further down (R3): the menu's own rows are never moved, changed or removed.

use super::event::{CloudPin, CompressFormat, MediaTool, MenuEvent, PdfTool};
use super::spec::{Command, Entry, Head, Node, Run, Task};
use crate::kit::prelude::*;
use crate::t;
use glint_model::aware::{Pick, Shape, Why};
use glint_model::format::format_size;
use glint_model::paths::base_name;

/// How many rows Suggested holds at most.
const MOST: usize = 3;

impl Run {
    /// The stable key of what choosing this does ("do:extract-here"): what habits are kept under. None for a
    /// row that does nothing. Habits are saved to disk under these, so once shipped a key is never renamed.
    pub fn key(&self) -> Option<String> {
        match self {
            Run::Nothing => None,
            Run::Command(command) => Some(command_key(*command).to_string()),
            Run::Do(task) => Some(task_key(task)),
            Run::Ask(event) => event_key(event),
        }
    }
}

fn command_key(command: Command) -> &'static str {
    match command {
        Command::Open => "cmd:open",
        Command::OpenInNewTab => "cmd:open-in-new-tab",
        Command::QuickLook => "cmd:quick-look",
        Command::Rename => "cmd:rename",
        Command::BatchRename => "cmd:batch-rename",
        Command::Cut => "cmd:cut",
        Command::Copy => "cmd:copy",
        Command::Paste => "cmd:paste",
        Command::CopyPath => "cmd:copy-path",
        Command::Delete => "cmd:delete",
        Command::DeletePermanently => "cmd:delete-permanently",
        Command::Undo => "cmd:undo",
        Command::SelectAll => "cmd:select-all",
        Command::Refresh => "cmd:refresh",
        Command::NewFolder => "cmd:new-folder",
        Command::Properties => "cmd:properties",
        Command::CompareFiles => "cmd:compare-files",
    }
}

fn task_key(task: &Task) -> String {
    let key = match task {
        Task::Navigate(_) => "task:navigate",
        Task::OpenInNewTabs(_) => "task:open-in-new-tabs",
        Task::OpenInOtherPane { .. } => "task:open-in-other-pane",
        Task::CopyText { .. } => "task:copy-text",
        Task::TogglePin(_) => "task:toggle-pin",
        Task::RefreshDrives => "task:refresh-drives",
        Task::GetAccess(_) => "task:get-access",
        Task::WindowsMenu { .. } => "task:windows-menu",
        Task::OpenTerminal(_) => "task:open-terminal",
        Task::OpenWith(_) => "task:open-with",
        Task::RunAsAdmin(_) => "task:run-as-admin",
        Task::ShowProperties(_) => "task:show-properties",
        Task::OpenIn { opener, .. } => return format!("task:open-in:{opener}"),
        Task::OpenInFileExplorer(_) => "task:open-in-file-explorer",
    };
    key.to_string()
}

fn event_key(event: &MenuEvent) -> Option<String> {
    let key = match event {
        MenuEvent::Shown | MenuEvent::Closed => return None,
        MenuEvent::Restore { .. } => "do:restore",
        MenuEvent::EmptyRecycleBin => "do:empty-recycle-bin",
        MenuEvent::PasteInto { .. } => "do:paste-into",
        MenuEvent::PasteQueued { .. } => "do:paste-queued",
        MenuEvent::ExtractHere { .. } => "do:extract-here",
        MenuEvent::Mount { .. } => "do:mount",
        MenuEvent::OpenWithItsApp { .. } => "do:open-with-its-app",
        MenuEvent::OpenWithEditor { .. } => "do:open-with-editor",
        MenuEvent::OpenWithPicker { .. } => "do:open-with-picker",
        MenuEvent::RunInSandbox { .. } => "do:run-in-sandbox",
        MenuEvent::PreviousVersions { .. } => "do:previous-versions",
        MenuEvent::OpenInNewWindow { .. } => "do:open-in-new-window",
        MenuEvent::OpenFileLocation { .. } => "do:open-file-location",
        MenuEvent::ForgetRecent { .. } => "do:forget-recent",
        MenuEvent::RevealInFileExplorer { .. } => "do:reveal-in-file-explorer",
        MenuEvent::RevealArchive { .. } => "do:reveal-archive",
        MenuEvent::SearchText { .. } => "do:search-text",
        MenuEvent::CopyToOtherPane { moving, .. } => {
            if *moving {
                "do:move-other"
            } else {
                "do:copy-other"
            }
        }
        MenuEvent::Compare { .. } => "do:compare",
        MenuEvent::SetTags { .. } => "do:tags",
        MenuEvent::EditTags { .. } => "do:edit-tags",
        MenuEvent::FolderSizes => "do:folder-sizes",
        MenuEvent::NewTextDocument => "do:new-text-document",
        MenuEvent::NewOfKind { ext } => return Some(format!("do:new:{}", ext.to_lowercase())),
        MenuEvent::BatchRenameFolder => "do:batch-rename-folder",
        MenuEvent::Vault { decrypt, .. } => {
            if *decrypt {
                "do:vault:decrypt"
            } else {
                "do:vault:encrypt"
            }
        }
        MenuEvent::Compress { format, .. } => match format {
            CompressFormat::Zip => "do:compress:zip",
            CompressFormat::SevenZ => "do:compress:7z",
            CompressFormat::TarZst => "do:compress:tar-zst",
        },
        MenuEvent::CompressInPlace { .. } => "do:compress-in-place",
        MenuEvent::CompressWithPassword { .. } => "do:compress-with-password",
        MenuEvent::MakeSmaller { .. } => "do:make-smaller",
        MenuEvent::BackUp { .. } => "do:back-up",
        MenuEvent::ConvertPictures { .. } => "do:convert-pictures",
        MenuEvent::Media { tool, .. } => match tool {
            MediaTool::Convert => "do:media:convert",
            MediaTool::Audio => "do:media:audio",
            MediaTool::Gif => "do:media:gif",
            MediaTool::Subtitles => "do:media:subtitles",
            MediaTool::Volume => "do:media:volume",
        },
        MenuEvent::EditSongs { .. } => "do:edit-songs",
        MenuEvent::ConvertDocument { .. } => "do:convert-document",
        MenuEvent::QueryTogether { .. } => "do:query-together",
        MenuEvent::ConvertData { .. } => "do:convert-data",
        MenuEvent::PdfTools { tool, .. } => match tool {
            PdfTool::Merge => "do:pdf:merge",
            PdfTool::Split => "do:pdf:split",
            PdfTool::Keep => "do:pdf:keep",
            PdfTool::Rotate => "do:pdf:rotate",
            PdfTool::Optimize => "do:pdf:optimize",
            PdfTool::Images => "do:pdf:images",
            PdfTool::Encrypt => "do:pdf:encrypt",
            PdfTool::Decrypt => "do:pdf:decrypt",
            PdfTool::Stamp => "do:pdf:stamp",
        },
        MenuEvent::SetWallpaper { .. } => "do:set-wallpaper",
        MenuEvent::Rotate { clockwise, .. } => {
            if *clockwise {
                "do:rotate:right"
            } else {
                "do:rotate:left"
            }
        }
        MenuEvent::CloudPin { state, .. } => match state {
            CloudPin::Keep => "do:cloud:keep",
            CloudPin::Free => "do:cloud:free",
            CloudPin::Auto => "do:cloud:auto",
        },
        MenuEvent::XRay { .. } => "do:x-ray",
        MenuEvent::Timeline { .. } => "do:timeline",
        MenuEvent::History { .. } => "do:history",
        MenuEvent::Locks { .. } => "do:locks",
        MenuEvent::Defender { .. } => "do:defender",
        MenuEvent::Duplicates { .. } => "do:duplicates",
        MenuEvent::Cleanup { .. } => "do:cleanup",
        MenuEvent::Rule { .. } => "do:rule",
        MenuEvent::ExportNcdu { .. } => "do:export-ncdu",
        MenuEvent::RemovePersonalInfo { .. } => "do:remove-personal-info",
        MenuEvent::ChangeDateTaken { .. } => "do:change-date-taken",
        MenuEvent::SendNearby { .. } => "do:send-nearby",
        MenuEvent::SendToPc { .. } => "do:send-to-pc",
        MenuEvent::ShareOnNetwork { .. } => "do:share-on-network",
        MenuEvent::ShareWithWindows { .. } => "do:share-with-windows",
        MenuEvent::ReceiveHere { .. } => "do:receive-here",
        MenuEvent::RunScript { script, .. } => {
            return Some(format!("do:script:{}", base_name(script).to_lowercase()));
        }
        MenuEvent::InstallTool { .. } => "do:install-tool",
        MenuEvent::OpenScriptsFolder => "do:open-scripts-folder",
        MenuEvent::EditConnection { .. } => "do:edit-connection",
        MenuEvent::RemoveConnection { .. } => "do:remove-connection",
        MenuEvent::ShowOnDemand { .. } => "do:show-on-demand",
        MenuEvent::HideOnDemand { .. } => "do:hide-on-demand",
        MenuEvent::Unblock { .. } => "do:unblock",
        MenuEvent::CopyTextIn { .. } => "do:copy-text-in",
        MenuEvent::SendTo { moving, .. } => {
            if *moving {
                "do:move-to"
            } else {
                "do:copy-to"
            }
        }
        MenuEvent::ChooseSendTo { moving, .. } => {
            if *moving {
                "do:move-to-choose"
            } else {
                "do:copy-to-choose"
            }
        }
    };
    Some(key.to_string())
}

/// What `walk` tells its caller about each row: the labels of the submenus it is in (outermost first; none for a
/// top-level row or a strip button) and the row. It answers whether to go on.
type Found<'a, 'f> = &'f mut dyn FnMut(&[&'a str], &'a Entry) -> bool;

/// Calls `found` for every row that can be chosen, in the menu's order. A disabled row, and a disabled
/// submenu with everything in it, is skipped. Returns false once `found` has said to stop.
fn walk<'a>(nodes: &'a [Node], menus: &mut Vec<&'a str>, found: Found<'a, '_>) -> bool {
    for node in nodes {
        match node {
            Node::Item(entry) => {
                if !entry.disabled && !found(menus, entry) {
                    return false;
                }
            }
            Node::Strip(entries) => {
                for entry in entries {
                    if !entry.disabled && !found(menus, entry) {
                        return false;
                    }
                }
            }
            Node::Menu {
                label,
                disabled: false,
                items,
                ..
            } => {
                menus.push(label.as_ref());
                let go_on = walk(items, menus, found);
                menus.pop();
                if !go_on {
                    return false;
                }
            }
            Node::Menu { .. } | Node::Head(_) | Node::Heading(_) | Node::Separator => {}
        }
    }
    true
}

/// The key of every row of `nodes` that can be chosen, in the menu's order, each once.
pub fn keys(nodes: &[Node]) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    walk(nodes, &mut Vec::new(), &mut |_, entry| {
        if let Some(key) = entry.run.key() {
            if !all.contains(&key) {
                all.push(key);
            }
        }
        true
    });
    all
}

/// The first row with `key`, as a copy made to stand alone (a submenu's row says which menu it is from).
pub fn row_for(nodes: &[Node], key: &str) -> Option<Entry> {
    let mut result: Option<Entry> = None;
    walk(nodes, &mut Vec::new(), &mut |menus, entry| {
        if entry.run.key().as_deref() != Some(key) {
            return true;
        }
        let mut row = entry.clone();
        // "Inspect & check" and "Clean up & share" are shelves: what is on them says what it does by itself
        // ("Scan with Microsoft Defender"). A row of any other submenu needs its menu's name ("Compress to ›
        // Zip file").
        let shelves = [t!("Inspect & check"), t!("Clean up & share")];
        let named: Vec<&str> = menus
            .iter()
            .copied()
            .filter(|menu| !shelves.iter().any(|shelf| shelf.as_ref() == *menu))
            .collect();
        if !named.is_empty() {
            row.label = format!("{} › {}", named.join(" › "), entry.label).into();
        }
        row.smart = true;
        row.check = None;
        row.reason = None;
        result = Some(row);
        false
    });
    result
}

/// The words under a suggested row, when the reason isn't obvious.
pub fn reason(why: &Why, shape: Shape) -> Option<SharedString> {
    match why {
        Why::Plain | Why::UsualPlace => None,
        Why::InUse(app) => Some(t!("In use by {app}", app = app)),
        Why::FromInternet(host) if host.is_empty() => Some(t!("It came from the internet")),
        Why::FromInternet(host) => Some(t!("It came from {host}", host = host)),
        Why::Unsigned => Some(t!("It isn't signed")),
        Why::BuildOutput(bytes) => Some(t!("{size} of build output inside", size = format_size(*bytes))),
        Why::Usual => Some(usual(shape)),
    }
}

/// "You often do this with pictures": one whole sentence per kind, so a translator has the sentence.
fn usual(shape: Shape) -> SharedString {
    match shape {
        Shape::Nothing => t!("You often do this here"),
        Shape::Folder | Shape::Folders => t!("You often do this with folders"),
        Shape::Archive | Shape::Archives => t!("You often do this with archives"),
        Shape::DiskImage => t!("You often do this with disk images"),
        Shape::Program => t!("You often do this with programs"),
        Shape::Installer => t!("You often do this with installers"),
        Shape::Script => t!("You often do this with scripts"),
        Shape::Shortcut => t!("You often do this with shortcuts"),
        Shape::Picture | Shape::Pictures => t!("You often do this with pictures"),
        Shape::Video | Shape::Videos => t!("You often do this with videos"),
        Shape::Song | Shape::Songs => t!("You often do this with songs"),
        Shape::Pdf | Shape::Pdfs => t!("You often do this with PDFs"),
        Shape::Document | Shape::Documents => t!("You often do this with documents"),
        Shape::Data => t!("You often do this with data files"),
        Shape::Code => t!("You often do this with code"),
        Shape::Font => t!("You often do this with fonts"),
        Shape::File | Shape::Files | Shape::Mixed => t!("You often do this with files like these"),
    }
}

/// The Suggested section: its heading and at most three rows (`picks`' rows, then `extra`). Empty with no row.
pub fn zone(nodes: &[Node], picks: &[Pick], shape: Shape, extra: Vec<Node>) -> Vec<Node> {
    let extra: Vec<Node> = extra.into_iter().take(MOST).collect();
    let room = MOST - extra.len();
    let mut rows: Vec<Node> = Vec::new();
    for pick in picks {
        if rows.len() >= room {
            break;
        }
        if let Some(mut row) = row_for(nodes, &pick.key) {
            row.reason = reason(&pick.why, shape);
            rows.push(Node::Item(row));
        }
    }
    rows.extend(extra);
    if rows.is_empty() {
        return Vec::new();
    }
    let mut section = Vec::with_capacity(rows.len() + 1);
    section.push(Node::Heading(t!("Suggested")));
    section.extend(rows);
    section
}

/// The menu with its head first and Suggested after its strip. The menu's own rows are never moved or changed.
pub fn dress(nodes: Vec<Node>, head: Head, picks: &[Pick], shape: Shape, extra: Vec<Node>) -> Vec<Node> {
    let section = zone(&nodes, picks, shape, extra);
    let mut dressed = Vec::with_capacity(nodes.len() + section.len() + 2);
    dressed.push(Node::Head(head));
    let mut rest = nodes.into_iter().peekable();
    if matches!(rest.peek(), Some(Node::Strip(_))) {
        dressed.extend(rest.next());
    }
    let has_section = !section.is_empty();
    dressed.extend(section);
    if has_section && !matches!(rest.peek(), None | Some(Node::Heading(_)) | Some(Node::Separator)) {
        dressed.push(Node::Separator);
    }
    dressed.extend(rest);
    dressed
}
