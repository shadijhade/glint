//! The menu of one or more items: a right-click on a file or folder, or the menu key.
//! Ported from `itemMenu` and `recycledMenu` in src/components/explorer/Explorer.tsx.
//!
//! [`item_menu`] only reads: it is given the items and the facts about the pane they are in ([`ItemCtx`]) and
//! returns the menu as data, in the TypeScript's order and with its conditions. Choosing an item is
//! [`Run`](super::spec::Run)'s business.
//!
//! "fixed" below is the TypeScript's word: inside an archive, listed from the index in a folder Windows keeps
//! closed, or on a remote connection, nothing changes, so what would is greyed out or left off, and the menu
//! ends with a note saying why.

use super::data::{Device, MenuData};
use super::event::{CloudPin, CompressFormat, MediaTool, MenuEvent, PdfTool};
use super::kinds::*; // includes `on_a_drive`
use super::spec::{separator, visible_items, when, Command, Entry, Node, Task};
use crate::explorer::is_disk_image;
use crate::kit::prelude::*;
use crate::t;
use glint_model::entries::{
    attr, cloud_state, is_archive_file, is_archived, is_browsable, is_closed, is_cloud_file, is_recycled, is_remote,
    CloudState, FileEntry, InArchive,
};
use glint_model::file_types::is_markdown;
use glint_model::paths::{is_folder_path, is_gallery, is_recent, is_recycle_bin, parent_path, same_path};

/// The facts about the pane the items are in. Built by the host from the explorer, the saved state and
/// [`MenuData`]; tests build it by hand.
#[derive(Clone, Debug, Default)]
pub struct ItemCtx {
    /// The pane, for "Open in the other pane" and for which side Compare puts a file.
    pub pane: usize,
    /// The folder shown (`path`).
    pub folder: String,
    /// A search's results are shown (`find`).
    pub find: bool,
    /// The archive the folder is in (`archiveOf(tab)`).
    pub archive: Option<InArchive>,
    /// Two panes are shown.
    pub dual: bool,
    /// The other pane's folder, when there is one to copy or move into (`otherPaneFolder()`): its display name.
    pub other_folder: Option<String>,
    /// The files chosen in the other pane, on disk (`otherSelectedFiles`).
    pub other_selected: Vec<FileEntry>,
    /// The other pane's folder and the files in it, when it has no search or archive shown (`otherListing`).
    pub other_listing: Option<(String, std::sync::Arc<[FileEntry]>)>,
    /// The pinned folders (`pinned`).
    pub pinned: Vec<String>,
    /// The editor Settings names (`editorPath`), and its name for the menu (`editorName`).
    pub editor_path: String,
    /// What the editor is called ("Code" for `...\Code.exe`).
    pub editor_name: String,
    /// The paired PCs.
    pub devices: Vec<Device>,
    /// The lists from [`MenuData`].
    pub data: MenuData,
}

/// The menu for `targets` (never empty).
pub fn item_menu(targets: &[FileEntry], c: &ItemCtx) -> Vec<Node> {
    if targets.is_empty() {
        return Vec::new();
    }
    if targets.iter().any(is_recycled) {
        return visible_items(recycled_menu(targets).into_iter().flatten().collect());
    }
    let one: Option<&FileEntry> = match targets {
        [only] => Some(only),
        _ => None,
    };
    let paths: Vec<String> = targets.iter().map(|e| e.path.clone()).collect();
    // Archives open like folders: in a new tab too.
    let folders: Vec<&FileEntry> = targets.iter().filter(|e| e.dir || is_archive_file(e)).collect();
    let other = c.other_folder.is_some();
    // Inside an archive nothing changes: what would is greyed out, and opening a file extracts it.
    let archived = targets.iter().any(is_archived).then(|| c.archive.clone()).flatten();
    // Listed from the index (Windows keeps their folder closed): nothing changes, and only folders open.
    let closed = targets.iter().any(is_closed);
    // On a remote: read-only, and downloaded to be opened or copied, as an archive's are extracted.
    let remote = targets.iter().any(is_remote);
    let fixed = archived.is_some() || closed || remote;
    let browses = one.is_some_and(|e| e.dir || is_archive_file(e));
    let pictures: Vec<&FileEntry> = targets.iter().filter(|e| can_convert(e)).collect();
    let media: Vec<&FileEntry> = if fixed {
        Vec::new()
    } else {
        targets.iter().filter(|e| is_media_file(e)).collect()
    };
    let videos: Vec<&FileEntry> = media.iter().copied().filter(|e| is_video_file(e)).collect();
    let pdfs: Vec<&FileEntry> = if fixed {
        Vec::new()
    } else {
        targets.iter().filter(|e| is_pdf_file(e)).collect()
    };
    let data_files: Vec<&FileEntry> = if fixed {
        Vec::new()
    } else {
        targets.iter().filter(|e| is_data_file(e)).collect()
    };
    let rotatable: Vec<&FileEntry> = if fixed {
        Vec::new()
    } else {
        targets.iter().filter(|e| can_rotate(e)).collect()
    };
    // OneDrive and other cloud-synced items: always kept on this device (or not), or freed up.
    let cloud = !fixed && targets.iter().all(|e| on_disk(e) && is_cloud_file(e));
    let kept = cloud && targets.iter().all(|e| e.attrs & attr::PINNED != 0);
    let freed = cloud
        && targets
            .iter()
            .all(|e| !e.dir && cloud_state(e) == Some(CloudState::Online));
    // Two files side by side: the two chosen in this pane, the one chosen here against the one chosen in the
    // other pane, or its namesake in the other pane.
    let pair = match targets {
        [a, b] if !fixed && !a.dir && !b.dir => Some((a, b)),
        _ => None,
    };
    let other_chosen = match (one, c.other_selected.as_slice()) {
        (Some(e), [chosen]) if c.dual && !e.dir && !fixed && !same_path(&e.path, &chosen.path) => Some(chosen),
        _ => None,
    };
    let namesake_entry = one.filter(|e| !e.dir && !fixed && c.dual).and_then(|e| {
        let (other_path, entries) = c.other_listing.as_ref()?;
        if same_path(&c.folder, other_path) {
            return None;
        }
        entries.iter().find(|o| !o.dir && o.lower == e.lower && on_disk(o))
    });
    let namesake = namesake_entry.filter(|n| !other_chosen.is_some_and(|chosen| same_path(&chosen.path, &n.path)));

    let pinned = one.is_some_and(|e| c.pinned.iter().any(|p| p == &e.path));
    let n = targets.len();

    // Quick-action icon bar at the top.
    let mut strip: Vec<Entry> = vec![
        Entry::command(t!("Cut"), IconName::Scissors, Command::Cut).disabled(fixed),
        Entry::command(t!("Copy"), IconName::Copy, Command::Copy),
    ];
    if let Some(e) = one.filter(|e| e.dir) {
        strip.push(
            Entry::ask(
                t!("Paste into folder"),
                IconName::ClipboardPaste,
                MenuEvent::PasteInto { folder: e.path.clone() },
            )
            .disabled(fixed),
        );
    }
    strip.push(
        if one.is_some() {
            Entry::command(t!("Rename"), IconName::PenLine, Command::Rename)
        } else {
            Entry::command(t!("Rename {n} items…", n = n), IconName::PenLine, Command::BatchRename).hinted("F2")
        }
        .disabled(fixed),
    );
    strip.push(Entry::command(
        if n == 1 { t!("Copy path") } else { t!("Copy paths") },
        IconName::ClipboardCopy,
        Command::CopyPath,
    ));
    if let Some(e) = one.filter(|e| e.dir) {
        strip.push(
            Entry::task(
                if pinned {
                    t!("Unpin from sidebar")
                } else {
                    t!("Pin to sidebar")
                },
                if pinned { IconName::PinOff } else { IconName::Pin },
                Task::TogglePin(e.path.clone()),
            )
            .active(pinned),
        );
    }
    strip.push(
        Entry::command(t!("Delete"), IconName::Trash, Command::Delete)
            .danger(!fixed)
            .disabled(fixed),
    );

    let mut m: Vec<Option<Node>> = Vec::new();
    m.push(Some(Node::Strip(strip)));
    m.push(
        Entry::command(
            if browses || n == 1 {
                t!("Open")
            } else {
                t!("Open {n} items", n = n)
            },
            if browses {
                if one.is_some_and(|e| e.dir) {
                    IconName::FolderOpen
                } else {
                    IconName::FileArchive
                }
            } else {
                IconName::ExternalLink
            },
            Command::Open,
        )
        .node(),
    );
    // A .docx or .jar is a zip too: Enter opens it in its app, this looks inside.
    if let Some(e) = one.filter(|e| is_browsable(e) && !closed) {
        m.push(
            Entry::task(
                t!("Open as archive"),
                IconName::FileArchive,
                Task::Navigate(e.path.clone()),
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| (is_archive_file(e) || is_browsable(e)) && !fixed) {
        m.push(
            Entry::ask(
                t!("Extract here"),
                IconName::ArchiveRestore,
                MenuEvent::ExtractHere { path: e.path.clone() },
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| is_disk_image(e) && !fixed) {
        m.push(Entry::ask(t!("Mount"), IconName::Disc3, MenuEvent::Mount { path: e.path.clone() }).node());
    }
    if !folders.is_empty() {
        m.push(
            Entry::task(
                if folders.len() > 1 {
                    t!("Open {n} folders in new tabs", n = folders.len())
                } else {
                    t!("Open in new tab")
                },
                IconName::SquarePlus,
                Task::OpenInNewTabs(folders.iter().map(|f| f.path.clone()).collect()),
            )
            .hint_of(Command::OpenInNewTab)
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| e.dir && !closed && !is_recycled(e)) {
        m.push(
            Entry::task(
                if c.dual {
                    t!("Open in the other pane")
                } else {
                    t!("Open side by side")
                },
                IconName::Columns2,
                Task::OpenInOtherPane {
                    path: e.path.clone(),
                    from: c.pane,
                },
            )
            .node(),
        );
    }
    if one.is_some() {
        m.push(Entry::command(t!("Quick Look"), IconName::Eye, Command::QuickLook).node());
    }
    if let Some(e) = one.filter(|e| is_archive_file(e) && !closed) {
        m.push(
            Entry::ask(
                t!("Open with its app"),
                IconName::AppWindow,
                MenuEvent::OpenWithItsApp {
                    path: e.path.clone(),
                    extract_first: archived.is_some(),
                },
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| !e.dir && !closed && !c.editor_name.is_empty()) {
        m.push(
            Entry::ask(
                t!("Open with {editor}", editor = c.editor_name),
                IconName::Code,
                MenuEvent::OpenWithEditor {
                    path: e.path.clone(),
                    editor: c.editor_path.trim().to_string(),
                    extract_first: archived.is_some() || remote,
                },
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| !e.dir && !closed) {
        let run = if archived.is_some() || remote {
            Entry::ask(
                t!("Open with…"),
                IconName::AppWindow,
                MenuEvent::OpenWithPicker { path: e.path.clone() },
            )
        } else {
            Entry::task(t!("Open with…"), IconName::AppWindow, Task::OpenWith(e.path.clone()))
        };
        m.push(run.node());
    }
    // Programs and scripts, as File Explorer offers it: Windows asks for administrator approval.
    let loose = |e: &&FileEntry| !e.dir && !closed && archived.is_none() && !remote && !is_recycle_bin(&c.folder);
    if let Some(e) = one.filter(|e| loose(e) && is_program(&e.name)) {
        m.push(
            Entry::task(
                t!("Run as administrator"),
                IconName::ShieldCheck,
                Task::RunAsAdmin(e.path.clone()),
            )
            .node(),
        );
    }
    // A program, an installer, a script or a document from somewhere it's not trusted, tried out in a throwaway Windows.
    if let Some(e) = one.filter(loose) {
        m.push(
            Entry::ask(
                t!("Run in Windows Sandbox"),
                IconName::Box,
                MenuEvent::RunInSandbox { path: e.path.clone() },
            )
            .node(),
        );
    }
    // As File Explorer's Previous Versions: what Windows' restore points and backups kept of it.
    if let Some(e) = one.filter(|e| !closed && archived.is_none() && !remote && on_a_drive(&e.path)) {
        m.push(
            Entry::ask(
                t!("Previous versions…"),
                IconName::ArchiveRestore,
                MenuEvent::PreviousVersions { path: e.path.clone() },
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| e.dir && !fixed) {
        m.push(
            Entry::task(
                t!("Open in Terminal"),
                IconName::SquareTerminal,
                Task::OpenTerminal(e.path.clone()),
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| e.dir && archived.is_none() && !remote && !closed) {
        m.push(
            Entry::ask(
                t!("Open in new window"),
                IconName::AppWindow,
                MenuEvent::OpenInNewWindow { path: e.path.clone() },
            )
            .hinted("Ctrl+N")
            .node(),
        );
    }
    if let Some(e) = one.filter(|_| !fixed && !c.data.openers.is_empty()) {
        m.push(Some(open_in_menu(&e.path, &c.data)));
    }
    if !fixed {
        let cwd = if is_folder_path(&c.folder) {
            c.folder.clone()
        } else {
            paths.first().map(|p| parent_path(p)).unwrap_or_default()
        };
        m.push(Some(scripts_menu(&cwd, &paths, &c.data)));
    }
    if let Some(e) = one.filter(|_| c.find || is_recent(&c.folder) || is_gallery(&c.folder)) {
        m.push(
            Entry::ask(
                t!("Open file location"),
                IconName::FolderInput,
                MenuEvent::OpenFileLocation { path: e.path.clone() },
            )
            .node(),
        );
    }
    if is_recent(&c.folder) && !c.find {
        m.push(
            Entry::ask(
                t!("Remove from Recent"),
                IconName::ClockArrowLeft,
                MenuEvent::ForgetRecent { paths: paths.clone() },
            )
            .node(),
        );
    }
    if !remote {
        m.push(
            match &archived {
                Some(archive) => Entry::ask(
                    t!("Show the archive in File Explorer"),
                    IconName::FolderSearch,
                    MenuEvent::RevealArchive {
                        archive: archive.outer.clone(),
                    },
                ),
                None => Entry::ask(
                    t!("Show in File Explorer"),
                    IconName::FolderSearch,
                    MenuEvent::RevealInFileExplorer { paths: paths.clone() },
                ),
            }
            .node(),
        );
    }
    m.push(separator());

    if c.dual {
        let name = c.other_folder.clone();
        m.push(
            Entry::ask(
                match &name {
                    Some(name) => t!("Copy to “{name}” in the other pane", name = name),
                    None => t!("Copy to the other pane"),
                },
                IconName::CopyPlus,
                MenuEvent::CopyToOtherPane {
                    paths: paths.clone(),
                    moving: false,
                },
            )
            .disabled(!other)
            .node(),
        );
        m.push(
            Entry::ask(
                match &name {
                    Some(name) => t!("Move to “{name}” in the other pane", name = name),
                    None => t!("Move to the other pane"),
                },
                IconName::FolderOutput,
                MenuEvent::CopyToOtherPane {
                    paths: paths.clone(),
                    moving: true,
                },
            )
            .disabled(!other || fixed)
            .node(),
        );
    }
    if let Some((a, b)) = pair {
        m.push(
            Entry::ask(
                t!("Compare “{a}” and “{b}”", a = a.name, b = b.name),
                IconName::GitCompareArrows,
                MenuEvent::Compare {
                    left: a.path.clone(),
                    right: b.path.clone(),
                },
            )
            .hint_of(Command::CompareFiles)
            .node(),
        );
    }
    if let (Some(e), Some(chosen)) = (one, other_chosen) {
        let (left, right) = if c.pane == 0 {
            (e.path.clone(), chosen.path.clone())
        } else {
            (chosen.path.clone(), e.path.clone())
        };
        m.push(
            Entry::ask(
                t!("Compare with “{name}” in the other pane", name = chosen.name),
                IconName::GitCompareArrows,
                MenuEvent::Compare { left, right },
            )
            .hint_of(Command::CompareFiles)
            .node(),
        );
    }
    if let (Some(e), Some(other)) = (one, namesake) {
        let (left, right) = if c.pane == 0 {
            (e.path.clone(), other.path.clone())
        } else {
            (other.path.clone(), e.path.clone())
        };
        let entry = Entry::ask(
            t!("Compare with “{name}” in the other pane", name = e.name),
            IconName::GitCompareArrows,
            MenuEvent::Compare { left, right },
        );
        // Ctrl+M compares the pair it can tell; with two ways to compare it is the other one's.
        m.push(
            if other_chosen.is_some() {
                entry
            } else {
                entry.hint_of(Command::CompareFiles)
            }
            .node(),
        );
    }
    if !fixed {
        m.push(Some(tags_menu(&paths, &c.data)));
    }
    if !fixed && targets.iter().all(is_age_file) {
        m.push(
            Entry::ask(
                if n > 1 {
                    t!("Decrypt {n} files…", n = n)
                } else {
                    t!("Decrypt…")
                },
                IconName::LockOpen,
                MenuEvent::Vault {
                    paths: paths.clone(),
                    decrypt: true,
                },
            )
            .node(),
        );
    }
    if !fixed && targets.iter().any(can_encrypt) {
        m.push(
            Entry::ask(
                t!("Encrypt…"),
                IconName::Lock,
                MenuEvent::Vault {
                    paths: paths.clone(),
                    decrypt: false,
                },
            )
            .node(),
        );
    }
    if !fixed {
        let compress = |label: SharedString, format| {
            Entry::ask(
                label,
                IconName::FileArchive,
                MenuEvent::Compress {
                    paths: paths.clone(),
                    format,
                },
            )
            .node()
        };
        m.push(Some(Node::menu(
            t!("Compress to"),
            IconName::FileArchive,
            vec![
                compress(t!("Zip file"), CompressFormat::Zip),
                compress(t!("7z file"), CompressFormat::SevenZ),
                compress(t!("tar.zst file"), CompressFormat::TarZst),
                when(targets.iter().all(can_compress_in_place), || {
                    entry_node(Entry::ask(
                        t!("In place (still opens normally)…"),
                        IconName::Shrink,
                        MenuEvent::CompressInPlace { paths: paths.clone() },
                    ))
                }),
                separator(),
                Entry::ask(
                    t!("With a password…"),
                    IconName::KeyRound,
                    MenuEvent::CompressWithPassword { paths: paths.clone() },
                )
                .node(),
            ],
        )));
    }
    if !fixed && targets.iter().any(can_shrink) {
        m.push(
            Entry::ask(
                t!("Make smaller…"),
                IconName::Minimize2,
                MenuEvent::MakeSmaller { paths: paths.clone() },
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| can_back_up(e)) {
        m.push(
            Entry::ask(
                t!("Back up…"),
                IconName::DatabaseBackup,
                MenuEvent::BackUp { folder: e.path.clone() },
            )
            .node(),
        );
    }
    if !fixed && targets.iter().any(is_jxl) {
        m.push(
            Entry::ask(
                t!("Back to JPEG"),
                IconName::ImageDown,
                MenuEvent::BackToJpeg { paths: paths.clone() },
            )
            .node(),
        );
    }
    if !pictures.is_empty() {
        m.push(
            Entry::ask(
                if pictures.len() > 1 {
                    t!("Convert {n} pictures…", n = pictures.len())
                } else {
                    t!("Convert picture…")
                },
                IconName::ImageDown,
                MenuEvent::ConvertPictures {
                    paths: pictures.iter().map(|e| e.path.clone()).collect(),
                },
            )
            .node(),
        );
    }
    if !media.is_empty() {
        let media_paths: Vec<String> = media.iter().map(|e| e.path.clone()).collect();
        let tool = |label: SharedString, icon, tool| {
            Entry::ask(
                label,
                icon,
                MenuEvent::Media {
                    paths: media_paths.clone(),
                    tool,
                },
            )
            .node()
        };
        m.push(Some(Node::menu(
            t!("Media"),
            IconName::Film,
            vec![
                when(!videos.is_empty(), || {
                    entry_node(Entry::ask(
                        if videos.len() > 1 {
                            t!("Convert or shrink {n} videos…", n = videos.len())
                        } else {
                            t!("Convert or shrink…")
                        },
                        IconName::Film,
                        MenuEvent::Media {
                            paths: media_paths.clone(),
                            tool: MediaTool::Convert,
                        },
                    ))
                }),
                tool(t!("Take the sound out…"), IconName::AudioLines, MediaTool::Audio),
                when(!videos.is_empty(), || {
                    entry_node(Entry::ask(
                        t!("Make a GIF…"),
                        IconName::Clapperboard,
                        MenuEvent::Media {
                            paths: media_paths.clone(),
                            tool: MediaTool::Gif,
                        },
                    ))
                }),
                tool(t!("Make subtitles…"), IconName::Captions, MediaTool::Subtitles),
                tool(t!("Even out the volume…"), IconName::Volume2, MediaTool::Volume),
                when(targets.iter().any(is_song), || {
                    entry_node(Entry::ask(
                        t!("Edit song details…"),
                        IconName::Music,
                        MenuEvent::EditSongs { paths: paths.clone() },
                    ))
                }),
            ],
        )));
    }
    if !fixed && targets.iter().any(is_markdown) {
        m.push(
            Entry::ask(
                t!("Export as PDF"),
                IconName::FileDown,
                MenuEvent::ExportPdf {
                    paths: targets
                        .iter()
                        .filter(|e| is_markdown(e))
                        .map(|e| e.path.clone())
                        .collect(),
                    typst: false,
                },
            )
            .node(),
        );
    }
    if !fixed && targets.iter().any(is_typst) {
        m.push(
            Entry::ask(
                t!("Export as PDF"),
                IconName::FileDown,
                MenuEvent::ExportPdf {
                    paths: paths.clone(),
                    typst: true,
                },
            )
            .node(),
        );
    }
    if !fixed && targets.iter().any(can_convert_doc) {
        m.push(
            Entry::ask(
                t!("Convert document…"),
                IconName::FileType,
                MenuEvent::ConvertDocument { paths: paths.clone() },
            )
            .node(),
        );
    }
    if data_files.len() > 1 {
        m.push(
            Entry::ask(
                t!("Query {n} files together…", n = data_files.len()),
                IconName::Sigma,
                MenuEvent::QueryTogether {
                    paths: data_files.iter().map(|e| e.path.clone()).collect(),
                },
            )
            .node(),
        );
    }
    if !data_files.is_empty() {
        m.push(
            Entry::ask(
                t!("Convert data…"),
                IconName::FileCog,
                MenuEvent::ConvertData {
                    paths: data_files.iter().map(|e| e.path.clone()).collect(),
                },
            )
            .node(),
        );
    }
    if !pdfs.is_empty() {
        m.push(Some(pdf_tools_menu(&pdfs)));
    }
    if let Some(e) = one.filter(|e| can_wallpaper(e)) {
        m.push(
            Entry::ask(
                t!("Set as desktop background"),
                IconName::Wallpaper,
                MenuEvent::SetWallpaper { path: e.path.clone() },
            )
            .node(),
        );
    }
    if !rotatable.is_empty() {
        let rotate_paths: Vec<String> = rotatable.iter().map(|e| e.path.clone()).collect();
        m.push(
            Entry::ask(
                t!("Rotate right"),
                IconName::RotateCwSquare,
                MenuEvent::Rotate {
                    paths: rotate_paths.clone(),
                    clockwise: true,
                },
            )
            .node(),
        );
        m.push(
            Entry::ask(
                t!("Rotate left"),
                IconName::RotateCcwSquare,
                MenuEvent::Rotate {
                    paths: rotate_paths,
                    clockwise: false,
                },
            )
            .node(),
        );
    }
    if cloud {
        m.push(
            Entry::ask(
                t!("Always keep on this device"),
                IconName::CloudDownload,
                MenuEvent::CloudPin {
                    paths: paths.clone(),
                    state: if kept { CloudPin::Auto } else { CloudPin::Keep },
                },
            )
            .checked(kept)
            .node(),
        );
        m.push(
            Entry::ask(
                t!("Free up space"),
                IconName::CloudOff,
                MenuEvent::CloudPin {
                    paths: paths.clone(),
                    state: CloudPin::Free,
                },
            )
            .disabled(freed)
            .node(),
        );
    }
    if one.is_some() && !c.find && !fixed {
        m.push(
            Entry::ask(
                t!("Rename items in this folder…"),
                IconName::PenLine,
                MenuEvent::BatchRenameFolder,
            )
            .hinted("Ctrl+Shift+R")
            .node(),
        );
    }
    m.push(separator());

    // PORT ME(find): "Search text" and "Open file location" are MenuEvents; the search part does them.
    if let Some(e) = one.filter(|e| e.dir && !fixed) {
        m.push(
            Entry::ask(
                t!("Search text"),
                IconName::TextSearch,
                MenuEvent::SearchText { folder: e.path.clone() },
            )
            .node(),
        );
    }
    m.push(Some(Node::menu(
        t!("Inspect & check"),
        IconName::ScanSearch,
        vec![
            one.filter(|e| !e.dir && !fixed).and_then(|e| {
                Entry::ask(
                    t!("File X-Ray"),
                    IconName::FingerprintPattern,
                    MenuEvent::XRay { path: e.path.clone() },
                )
                .node()
            }),
            one.filter(|e| e.dir && !fixed).and_then(|e| {
                Entry::ask(
                    t!("Recent changes"),
                    IconName::FileClock,
                    MenuEvent::Timeline { folder: e.path.clone() },
                )
                .node()
            }),
            when(archived.is_none() && !remote, || {
                entry_node(Entry::ask(
                    t!("What's locking this?"),
                    IconName::Lock,
                    MenuEvent::Locks { paths: paths.clone() },
                ))
            }),
            separator(),
            when(!fixed, || {
                entry_node(Entry::ask(
                    t!("Scan with Microsoft Defender"),
                    IconName::ShieldCheck,
                    MenuEvent::Defender { paths: paths.clone() },
                ))
            }),
            one.filter(|_| !fixed).and_then(|e| {
                Entry::ask(
                    t!("Look for passwords and keys"),
                    IconName::KeyRound,
                    MenuEvent::Secrets { path: e.path.clone() },
                )
                .node()
            }),
            one.filter(|_| !fixed).and_then(|e| {
                Entry::ask(
                    t!("Check with YARA rules"),
                    IconName::ScanSearch,
                    MenuEvent::Yara { path: e.path.clone() },
                )
                .node()
            }),
        ],
    )));
    let folder_one = one.filter(|e| e.dir && !fixed);
    // PORT ME(sharing): "Send to my PC", "Send to a phone nearby…", "Send to another PC…", "Share on this
    // network…" and "Share with Windows…" are MenuEvents; the sharing part does them.
    m.push(Some(Node::menu(
        t!("Clean up & share"),
        IconName::BrushCleaning,
        vec![
            folder_one.and_then(|e| {
                Entry::ask(
                    t!("Find duplicate files"),
                    IconName::Files,
                    MenuEvent::Duplicates { folder: e.path.clone() },
                )
                .node()
            }),
            folder_one.and_then(|e| {
                Entry::ask(
                    t!("Clean up…"),
                    IconName::BrushCleaning,
                    MenuEvent::Cleanup { folder: e.path.clone() },
                )
                .node()
            }),
            folder_one.and_then(|e| {
                Entry::ask(
                    t!("Export as ncdu JSON…"),
                    IconName::FileBraces,
                    MenuEvent::ExportNcdu { folder: e.path.clone() },
                )
                .node()
            }),
            when(!fixed && targets.iter().any(can_clean), || {
                entry_node(Entry::ask(
                    t!("Remove personal info…"),
                    IconName::EyeOff,
                    MenuEvent::RemovePersonalInfo { paths: paths.clone() },
                ))
            }),
            when(!fixed && targets.iter().any(can_redate), || {
                entry_node(Entry::ask(
                    t!("Change when it was taken…"),
                    IconName::CalendarClock,
                    MenuEvent::ChangeDateTaken { paths: paths.clone() },
                ))
            }),
            separator(),
            when(!fixed && !c.devices.is_empty(), || {
                Node::menu(
                    t!("Send to my PC"),
                    IconName::Monitor,
                    c.devices
                        .iter()
                        .map(|d| {
                            Entry::ask(
                                SharedString::from(d.name.clone()),
                                IconName::Monitor,
                                MenuEvent::SendToDevice {
                                    device: d.id.clone(),
                                    paths: paths.clone(),
                                },
                            )
                            .node()
                        })
                        .collect(),
                )
            }),
            when(!fixed, || {
                entry_node(Entry::ask(
                    t!("Send to a phone nearby…"),
                    IconName::Smartphone,
                    MenuEvent::SendNearby { paths: paths.clone() },
                ))
            }),
            one.filter(|_| !fixed).and_then(|e| {
                Entry::ask(
                    t!("Send to another PC…"),
                    IconName::Send,
                    MenuEvent::SendToPc { path: e.path.clone() },
                )
                .node()
            }),
            one.filter(|_| !fixed).and_then(|e| {
                Entry::ask(
                    t!("Share on this network…"),
                    IconName::Wifi,
                    MenuEvent::ShareOnNetwork { path: e.path.clone() },
                )
                .node()
            }),
            when(!fixed, || {
                entry_node(Entry::ask(
                    t!("Share with Windows…"),
                    IconName::Share2,
                    MenuEvent::ShareWithWindows { paths: paths.clone() },
                ))
            }),
        ],
    )));
    m.push(separator());

    if one.is_some() && archived.is_none() && !remote {
        m.push(Entry::command(t!("Properties"), IconName::Info, Command::Properties).node());
    }
    if shell_menu_fits(targets) {
        m.push(
            Entry::task(
                t!("Show more options"),
                IconName::Ellipsis,
                Task::WindowsMenu { paths: paths.clone() },
            )
            .hinted("Shift+right-click")
            .node(),
        );
    }
    if archived.is_some() {
        m.push(read_only_note());
    }
    if remote {
        m.push(remote_note());
    }
    if closed {
        m.push(closed_note());
    }
    visible_items(m.into_iter().flatten().collect())
}

/// The note that ends a menu in an archive: why what changes things is greyed out.
pub fn read_only_note() -> Option<Node> {
    Entry::note(t!("Read-only: inside an archive"), IconName::PencilOff).node()
}

/// On a remote connection: why nothing changes.
pub fn remote_note() -> Option<Node> {
    Entry::note(t!("Read-only: on a remote connection"), IconName::PencilOff).node()
}

/// In a folder listed from the index: why nothing opens or changes.
pub fn closed_note() -> Option<Node> {
    Entry::note(t!("Read-only: Windows keeps this folder closed"), IconName::FolderLock).node()
}

/// Whether Windows' own menu has these: on disk (not in an archive, a remote or the Recycle Bin), and all
/// in one folder (`shellMenuFits`).
pub fn shell_menu_fits(targets: &[FileEntry]) -> bool {
    let Some(first) = targets.first() else { return false };
    let folder = parent_path(&first.path);
    !targets
        .iter()
        .any(|t| is_archived(t) || is_closed(t) || is_remote(t) || is_recycled(t))
        && targets.iter().all(|t| same_path(&parent_path(&t.path), &folder))
}

/// A program or script Windows can run as administrator: `/\.(exe|bat|cmd|com|msc)$/i`.
fn is_program(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".exe", ".bat", ".cmd", ".com", ".msc"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

fn entry_node(entry: Entry) -> Node {
    Node::Item(entry)
}

/// In the Recycle Bin: back where it was deleted from, a look at it, or gone for good (`recycledMenu`).
fn recycled_menu(targets: &[FileEntry]) -> Vec<Option<Node>> {
    let one: Option<&FileEntry> = match targets {
        [only] => Some(only),
        _ => None,
    };
    let originals: Vec<String> = targets
        .iter()
        .map(|e| glint_model::paths::join_path(e.origin.as_deref().unwrap_or(""), &e.name))
        .collect();
    vec![
        Entry::ask(
            if one.is_some() {
                t!("Restore")
            } else {
                t!("Restore {n} items", n = targets.len())
            },
            IconName::RotateCcw,
            MenuEvent::Restore {
                paths: targets.iter().map(|e| e.path.clone()).collect(),
            },
        )
        .node(),
        one.map(|_| Entry::command(t!("Quick Look"), IconName::Eye, Command::QuickLook))
            .and_then(|e| e.node()),
        one.and_then(|e| e.origin.clone()).and_then(|origin| {
            Entry::task(
                t!("Open the folder it was deleted from"),
                IconName::FolderOpen,
                Task::Navigate(origin),
            )
            .node()
        }),
        separator(),
        Entry::task(
            if one.is_some() {
                t!("Copy original path")
            } else {
                t!("Copy original paths")
            },
            IconName::ClipboardCopy,
            Task::CopyText {
                text: originals.join("\r\n"),
                paths: originals.len(),
            },
        )
        .node(),
        separator(),
        Entry::command(t!("Delete for good"), IconName::Trash, Command::Delete)
            .danger(true)
            .node(),
    ]
}

/// "Open in": each editor, and each terminal (a file's folder in it) (`openInItems`).
pub fn open_in_menu(path: &str, data: &MenuData) -> Node {
    Node::menu(
        t!("Open in"),
        IconName::AppWindow,
        data.openers
            .iter()
            .map(|o| {
                Entry::task(
                    SharedString::from(o.name),
                    if o.terminal {
                        IconName::SquareTerminal
                    } else {
                        IconName::Code
                    },
                    Task::OpenIn {
                        opener: o.id.to_string(),
                        name: o.name.to_string(),
                        path: path.to_string(),
                    },
                )
                .node()
            })
            .collect(),
    )
}

/// The Scripts menu: each script, run on `paths` in `cwd`, then the folder they're in (`scriptItems`).
pub fn scripts_menu(cwd: &str, paths: &[String], data: &MenuData) -> Node {
    let mut items: Vec<Option<Node>> = data
        .scripts
        .iter()
        .map(|script| match &script.missing {
            Some(tool) => Entry::ask(
                t!("{name}: install {tool} to run it", name = script.name, tool = tool),
                IconName::Download,
                MenuEvent::InstallTool { tool: tool.clone() },
            )
            .node(),
            None => Entry::ask(
                SharedString::from(script.name.clone()),
                IconName::ScrollText,
                MenuEvent::RunScript {
                    script: script.path.clone(),
                    cwd: cwd.to_string(),
                    paths: paths.to_vec(),
                },
            )
            .node(),
        })
        .collect();
    if !items.is_empty() {
        items.push(separator());
    }
    items.push(
        Entry::ask(
            t!("Open the Scripts folder"),
            IconName::FolderCog,
            MenuEvent::OpenScriptsFolder,
        )
        .node(),
    );
    Node::menu(t!("Scripts"), IconName::ScrollText, items)
}

// PORT ME(tags): the colors toggle and "Edit tags…" are MenuEvents; the checks come from `MenuData::tags`, which
// the tags part keeps current.
fn tags_menu(paths: &[String], data: &MenuData) -> Node {
    let has = |path: &String, tag: &str| {
        data.tags
            .get(&path.to_lowercase())
            .is_some_and(|tags| tags.iter().any(|x| x.eq_ignore_ascii_case(tag)))
    };
    let mut items: Vec<Option<Node>> = Vec::new();
    for (label, tag) in [
        (t!("Red"), "Red"),
        (t!("Orange"), "Orange"),
        (t!("Yellow"), "Yellow"),
        (t!("Green"), "Green"),
        (t!("Blue"), "Blue"),
        (t!("Purple"), "Purple"),
        (t!("Gray"), "Gray"),
    ] {
        let all = paths.iter().all(|p| has(p, tag));
        let (add, remove) = if all {
            (Vec::new(), vec![tag.to_string()])
        } else {
            (vec![tag.to_string()], Vec::new())
        };
        items.push(
            Entry::ask(
                label,
                IconName::Circle,
                MenuEvent::SetTags {
                    paths: paths.to_vec(),
                    add,
                    remove,
                },
            )
            .checked(all)
            .node(),
        );
    }
    items.push(separator());
    items.push(
        Entry::ask(
            t!("Edit tags…"),
            IconName::Tag,
            MenuEvent::EditTags { paths: paths.to_vec() },
        )
        .node(),
    );
    Node::menu(t!("Tags"), IconName::Tag, items)
}

fn pdf_tools_menu(pdfs: &[&FileEntry]) -> Node {
    let paths: Vec<String> = pdfs.iter().map(|e| e.path.clone()).collect();
    let tool = |label: SharedString, icon, tool| {
        Entry::ask(
            label,
            icon,
            MenuEvent::PdfTools {
                paths: paths.clone(),
                tool,
            },
        )
        .node()
    };
    Node::menu(
        t!("PDF tools"),
        IconName::FileText,
        vec![
            when(pdfs.len() > 1, || {
                entry_node(Entry::ask(
                    t!("Merge {n} PDFs…", n = pdfs.len()),
                    IconName::FileStack,
                    MenuEvent::PdfTools {
                        paths: paths.clone(),
                        tool: PdfTool::Merge,
                    },
                ))
            }),
            tool(t!("Split…"), IconName::Scissors, PdfTool::Split),
            tool(t!("Keep or take out pages…"), IconName::FileText, PdfTool::Keep),
            tool(t!("Turn pages…"), IconName::RotateCw, PdfTool::Rotate),
            tool(t!("Make smaller"), IconName::Minimize2, PdfTool::Optimize),
            tool(t!("Save its pictures…"), IconName::Images, PdfTool::Images),
            separator(),
            tool(t!("Add a password…"), IconName::KeyRound, PdfTool::Encrypt),
            tool(t!("Take the password off…"), IconName::LockOpen, PdfTool::Decrypt),
            tool(t!("Stamp…"), IconName::Stamp, PdfTool::Stamp),
        ],
    )
}
