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

use super::data::MenuData;
use super::event::{CloudPin, CompressFormat, MediaTool, MenuEvent, PdfTool};
use super::kinds::*; // includes `on_a_drive`
use super::send_to;
use super::spec::{separator, visible_items, when, Command, Entry, Node, Task};
use crate::explorer::is_disk_image;
use crate::kit::prelude::*;
use crate::t;
use glint_model::entries::{
    attr, cloud_state, is_archive_file, is_archived, is_browsable, is_closed, is_cloud_file, is_online_only,
    is_recycled, is_remote, CloudState, FileEntry, InArchive,
};
use glint_model::paths::{is_folder_path, is_gallery, is_recent, is_recycle_bin, parent_path, same_path};

/// The pictures Windows' text recognition reads.
const TEXT_PICTURES: &[&str] = &["png", "jpg", "jpeg", "jpe", "bmp", "gif", "tif", "tiff", "webp"];

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
    /// The lists from [`MenuData`].
    pub data: MenuData,
    /// What was found out about the one target (docs/gpui/aware): decides the rows that depend on it.
    pub probed: Option<glint_model::aware::Probed>,
    /// The folders this kind of thing has gone to before, heaviest first, for "Move or copy to".
    pub places: Vec<glint_model::aware::Place>,
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
    // Why what changes things is greyed out: the first cause that holds.
    let why_fixed: Option<SharedString> = if archived.is_some() {
        Some(t!("Nothing changes inside an archive"))
    } else if remote {
        Some(t!("Read-only: on a remote connection"))
    } else if closed {
        Some(t!("Windows keeps this folder closed"))
    } else {
        None
    };
    let why_no_other = Some(t!("The other pane shows nothing that can take items"));
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
    let blocked = c
        .probed
        .as_ref()
        .and_then(|p| p.origin.as_ref())
        .is_some_and(|o| o.blocked);

    // Quick-action icon bar at the top, in Windows 11's order.
    let mut strip: Vec<Entry> = vec![
        Entry::command(t!("Cut"), IconName::Scissors, Command::Cut).disabled_because(fixed, &why_fixed),
        Entry::command(t!("Copy"), IconName::Copy, Command::Copy),
    ];
    if let Some(e) = one.filter(|e| e.dir) {
        strip.push(
            Entry::ask(
                t!("Paste into folder"),
                IconName::ClipboardPaste,
                MenuEvent::PasteInto { folder: e.path.clone() },
            )
            .disabled_because(fixed, &why_fixed),
        );
    }
    strip.push(
        if one.is_some() {
            Entry::command(t!("Rename"), IconName::PenLine, Command::Rename)
        } else {
            Entry::command(t!("Rename {n} items…", n = n), IconName::PenLine, Command::BatchRename).hinted("F2")
        }
        .disabled_because(fixed, &why_fixed),
    );
    strip.push(Entry::command(
        if n == 1 { t!("Copy path") } else { t!("Copy paths") },
        IconName::ClipboardCopy,
        Command::CopyPath,
    ));
    if !fixed {
        strip.push(Entry::ask(
            t!("Share…"),
            IconName::Share2,
            MenuEvent::Share { paths: paths.clone() },
        ));
    }
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
            .disabled_because(fixed, &why_fixed),
    );

    let archive_file = one.filter(|e| is_archive_file(e));
    let folder_one = one.filter(|e| e.dir);
    let mut m: Vec<Option<Node>> = Vec::new();
    m.push(Some(Node::Strip(strip)));

    // ---- The rows the menu leads with ----------------------------------------------------------------------
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
    // Search results, Recent and the Gallery show files from everywhere: the way to the folder is next to Open.
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
    // An archive that was extracted before: its folder is a click away, ahead of extracting it again.
    if let Some(folder) = archive_file
        .filter(|_| !fixed)
        .and(c.probed.as_ref())
        .and_then(|probed| probed.extracted.clone().flatten())
    {
        m.push(
            Entry::task(
                t!("Open extracted folder"),
                IconName::FolderOpen,
                Task::OpenExtracted(folder),
            )
            .node(),
        );
    }
    if let Some(e) = archive_file.filter(|_| !fixed) {
        m.push(
            Entry::ask(
                t!("Extract here"),
                IconName::ArchiveRestore,
                MenuEvent::ExtractHere { path: e.path.clone() },
            )
            .node(),
        );
    }
    if let Some(e) = archive_file.filter(|_| !closed) {
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
    // Several folders open in tabs; one folder in a tab, beside this pane, or in a terminal.
    if one.is_none() && !folders.is_empty() {
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
    if let Some(e) = folder_one {
        m.push(
            Entry::task(
                t!("Open in new tab"),
                IconName::SquarePlus,
                Task::OpenInNewTabs(vec![e.path.clone()]),
            )
            .hint_of(Command::OpenInNewTab)
            .node(),
        );
    }
    if let Some(e) = folder_one.filter(|e| !closed && !is_recycled(e)) {
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
    if let Some(e) = folder_one.filter(|_| !fixed) {
        m.push(
            Entry::task(
                t!("Open in Terminal"),
                IconName::SquareTerminal,
                Task::OpenTerminal(e.path.clone()),
            )
            .node(),
        );
    }
    // A file: choose the app, or the editor Settings names, then a look at it.
    if let Some(e) = one.filter(|e| !e.dir && !closed && !is_archive_file(e)) {
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
    if one.is_some_and(|e| !e.dir) {
        m.push(Entry::command(t!("Quick Look"), IconName::Eye, Command::QuickLook).node());
    }

    // ---- What kind of thing it is -----------------------------------------------------------------------------
    // A section is a heading, the kind's most used rows, and "More … tools" with the rest. A mixed selection
    // gets each kind's section; what can't apply to anything chosen isn't there.
    let readable_picture =
        |e: &&FileEntry| !e.dir && on_disk(e) && !is_online_only(e) && TEXT_PICTURES.contains(&e.ext());
    let pic_count = targets
        .iter()
        .filter(|e| can_convert(e) || can_rotate(e) || can_wallpaper(e) || readable_picture(e))
        .count();
    let any_picture = pic_count > 0;
    let doc_count = targets
        .iter()
        .filter(|e| !fixed && (can_convert_doc(e) || is_office_file(e)))
        .count();
    let any_document = doc_count > 0;
    let any_video = !videos.is_empty();
    let any_pdf = !pdfs.is_empty();
    // Make smaller works on pictures, videos, PDFs and Office files: it lives with the first of them there.
    let shrinks = !fixed && targets.iter().any(can_shrink);
    let shrink_home = if any_picture {
        Some("picture")
    } else if any_video {
        Some("video")
    } else if any_pdf {
        Some("pdf")
    } else if any_document {
        Some("document")
    } else {
        None
    };
    let smaller_for = |home: &str| -> Option<Node> {
        if shrinks && shrink_home == Some(home) {
            Entry::ask(
                t!("Make smaller…"),
                IconName::Minimize2,
                MenuEvent::MakeSmaller { paths: paths.clone() },
            )
            .node()
        } else {
            None
        }
    };
    // Personal info and the date taken belong with pictures; for anything else they are under Clean up.
    let personal_rows = || -> Vec<Option<Node>> {
        vec![
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
        ]
    };
    let mut sections: Vec<Option<Node>> = Vec::new();

    if any_picture {
        sections.push(Some(Node::Heading(if pic_count > 1 {
            t!("Pictures")
        } else {
            t!("Picture")
        })));
        let rotate_paths: Vec<String> = rotatable.iter().map(|e| e.path.clone()).collect();
        sections.push(when(!rotatable.is_empty(), || {
            entry_node(Entry::ask(
                t!("Rotate right"),
                IconName::RotateCwSquare,
                MenuEvent::Rotate {
                    paths: rotate_paths.clone(),
                    clockwise: true,
                },
            ))
        }));
        if let Some(e) = one.filter(|e| can_wallpaper(e)) {
            sections.push(
                Entry::ask(
                    t!("Set as desktop background"),
                    IconName::Wallpaper,
                    MenuEvent::SetWallpaper { path: e.path.clone() },
                )
                .node(),
            );
        }
        if !pictures.is_empty() {
            sections.push(
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
        let mut more: Vec<Option<Node>> = vec![
            when(!rotatable.is_empty(), || {
                entry_node(Entry::ask(
                    t!("Rotate left"),
                    IconName::RotateCcwSquare,
                    MenuEvent::Rotate {
                        paths: rotate_paths.clone(),
                        clockwise: false,
                    },
                ))
            }),
            one.filter(readable_picture).and_then(|e| {
                Entry::ask(
                    t!("Copy the text in it"),
                    IconName::ScanText,
                    MenuEvent::CopyTextIn { path: e.path.clone() },
                )
                .node()
            }),
            smaller_for("picture"),
            one.filter(|e| can_convert(e)).and_then(|e| {
                Entry::ask(
                    t!("Find similar colors"),
                    IconName::Pipette,
                    MenuEvent::MatchColor { path: e.path.clone() },
                )
                .node()
            }),
        ];
        more.extend(personal_rows());
        sections.push(Some(Node::menu(more_picture_label(), IconName::Image, more)));
    }

    if any_video || media.iter().any(|e| !is_video_file(e)) {
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
        let song_row = when(targets.iter().any(is_song), || {
            entry_node(Entry::ask(
                t!("Edit song details…"),
                IconName::Music,
                MenuEvent::EditSongs { paths: paths.clone() },
            ))
        });
        if any_video {
            sections.push(Some(Node::Heading(if videos.len() > 1 {
                t!("Videos")
            } else {
                t!("Video")
            })));
            sections.push(
                Entry::ask(
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
                )
                .node(),
            );
            sections.push(tool(t!("Make a GIF…"), IconName::Clapperboard, MediaTool::Gif));
            sections.push(tool(t!("Take the sound out…"), IconName::AudioLines, MediaTool::Audio));
            sections.push(song_row);
            sections.push(Some(Node::menu(
                more_video_label(),
                IconName::Film,
                vec![
                    tool(t!("Make subtitles…"), IconName::Captions, MediaTool::Subtitles),
                    tool(t!("Even out the volume…"), IconName::Volume2, MediaTool::Volume),
                    smaller_for("video"),
                ],
            )));
        } else {
            sections.push(Some(Node::Heading(t!("Audio"))));
            sections.push(song_row);
            sections.push(tool(t!("Even out the volume…"), IconName::Volume2, MediaTool::Volume));
            sections.push(Some(Node::menu(
                more_audio_label(),
                IconName::AudioLines,
                vec![tool(t!("Make subtitles…"), IconName::Captions, MediaTool::Subtitles)],
            )));
        }
    }

    if any_pdf {
        sections.push(Some(Node::Heading(t!("PDF"))));
        sections.extend(pdf_section(&pdfs, smaller_for("pdf")));
    }

    if any_document {
        sections.push(Some(Node::Heading(if doc_count > 1 {
            t!("Documents")
        } else {
            t!("Document")
        })));
        sections.push(when(!fixed && targets.iter().any(can_convert_doc), || {
            entry_node(Entry::ask(
                t!("Convert document…"),
                IconName::FileType,
                MenuEvent::ConvertDocument { paths: paths.clone() },
            ))
        }));
        sections.push(smaller_for("document"));
    }

    if !data_files.is_empty() {
        sections.push(Some(Node::Heading(t!("Data"))));
        sections.push(
            Entry::ask(
                t!("Convert data…"),
                IconName::FileCog,
                MenuEvent::ConvertData {
                    paths: data_files.iter().map(|e| e.path.clone()).collect(),
                },
            )
            .node(),
        );
        if data_files.len() > 1 {
            sections.push(
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
    }

    // Programs, scripts and installers: Windows asks for administrator approval, or a throwaway Windows tries it.
    let loose = |e: &&FileEntry| !e.dir && !closed && archived.is_none() && !remote && !is_recycle_bin(&c.folder);
    let program_rows: Vec<Option<Node>> = vec![
        one.filter(|e| loose(e) && is_program(&e.name)).and_then(|e| {
            Entry::task(
                t!("Run as administrator"),
                IconName::ShieldCheck,
                Task::RunAsAdmin(e.path.clone()),
            )
            .node()
        }),
        one.filter(|e| loose(e) && runs_in_sandbox(e, blocked)).and_then(|e| {
            Entry::ask(
                t!("Run in Windows Sandbox"),
                IconName::Box,
                MenuEvent::RunInSandbox { path: e.path.clone() },
            )
            .node()
        }),
        // Windows marked it as downloaded from the internet: one press takes the mark off.
        one.filter(|e| blocked && !e.dir && !fixed).and_then(|e| {
            Entry::ask(
                t!("Unblock"),
                IconName::LockOpen,
                MenuEvent::Unblock {
                    paths: vec![e.path.clone()],
                },
            )
            .node()
        }),
    ];
    if program_rows.iter().any(Option::is_some) {
        sections.push(Some(Node::Heading(t!("Program"))));
        sections.extend(program_rows);
    }

    if let Some(e) = archive_file.filter(|_| !fixed && !remote) {
        sections.push(Some(Node::Heading(t!("Archive"))));
        sections.push(
            Entry::ask(
                t!("Mount as live folder"),
                IconName::Pin,
                MenuEvent::PinPortal {
                    name: e.name.clone(),
                    source: glint_core::portals::PortalSource::Archive {
                        archive_path: e.path.clone(),
                    },
                },
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| is_disk_image(e) && !fixed) {
        sections.push(Some(Node::Heading(t!("Disk image"))));
        sections.push(Entry::ask(t!("Mount"), IconName::Disc3, MenuEvent::Mount { path: e.path.clone() }).node());
    }
    if cloud {
        sections.push(Some(Node::Heading(t!("Cloud file"))));
        sections.push(
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
        sections.push(
            Entry::ask(
                t!("Free up space"),
                IconName::CloudOff,
                MenuEvent::CloudPin {
                    paths: paths.clone(),
                    state: CloudPin::Free,
                },
            )
            .disabled_because(freed, &Some(t!("It is online only already")))
            .node(),
        );
    }
    m.extend(sections);
    m.push(separator());

    // ---- Organize -------------------------------------------------------------------------------------------------
    // Not in the Recycle Bin's menu (it has its own), and not for what can't be moved or copied from here.
    if !fixed {
        m.push(Some(send_to::menu(&paths, &c.places, &c.folder)));
    }
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
            .disabled_because(!other, &why_no_other)
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
            .disabled_because(!other || fixed, if !other { &why_no_other } else { &why_fixed })
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
    if one.is_none() && !fixed {
        m.push(
            Entry::command(t!("Rename {n} items…", n = n), IconName::PenLine, Command::BatchRename)
                .hinted("F2")
                .node(),
        );
    }

    // ---- More -----------------------------------------------------------------------------------------------------
    let mut more: Vec<Option<Node>> = Vec::new();
    // What isn't one of the first rows for this kind of item, but is still its own.
    if let Some(e) = archive_file.filter(|_| !closed) {
        more.push(
            Entry::task(
                t!("Open in new tab"),
                IconName::SquarePlus,
                Task::OpenInNewTabs(vec![e.path.clone()]),
            )
            .hint_of(Command::OpenInNewTab)
            .node(),
        );
        more.push(
            if archived.is_some() || remote {
                Entry::ask(
                    t!("Open with…"),
                    IconName::AppWindow,
                    MenuEvent::OpenWithPicker { path: e.path.clone() },
                )
            } else {
                Entry::task(t!("Open with…"), IconName::AppWindow, Task::OpenWith(e.path.clone()))
            }
            .node(),
        );
    }
    // A .docx or .jar is a zip too: Enter opens it in its app, this looks inside.
    if let Some(e) = one.filter(|e| is_browsable(e) && !closed) {
        more.push(
            Entry::task(
                t!("Open as archive"),
                IconName::FileArchive,
                Task::Navigate(e.path.clone()),
            )
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| is_browsable(e) && !is_archive_file(e) && !fixed) {
        more.push(
            Entry::ask(
                t!("Extract here"),
                IconName::ArchiveRestore,
                MenuEvent::ExtractHere { path: e.path.clone() },
            )
            .node(),
        );
    }
    if folder_one.is_some() {
        more.push(Entry::command(t!("Quick Look"), IconName::Eye, Command::QuickLook).node());
    }
    if let Some(e) = folder_one.filter(|e| archived.is_none() && !remote && !closed && !is_recycled(e)) {
        more.push(
            Entry::ask(
                t!("Open in new window"),
                IconName::AppWindow,
                MenuEvent::OpenInNewWindow { path: e.path.clone() },
            )
            .hinted("Ctrl+N")
            .node(),
        );
    }
    if !remote {
        more.push(
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
    // As File Explorer's Previous Versions: what Windows' restore points and backups kept of it.
    if let Some(e) = one.filter(|e| !closed && archived.is_none() && !remote && on_a_drive(&e.path)) {
        more.push(
            Entry::ask(
                t!("Previous versions…"),
                IconName::ArchiveRestore,
                MenuEvent::PreviousVersions { path: e.path.clone() },
            )
            .node(),
        );
    }
    more.push(separator());
    if let Some(e) = one.filter(|_| !fixed && !c.data.openers.is_empty()) {
        more.push(Some(open_in_menu(&e.path, &c.data)));
    }
    if !fixed {
        let cwd = if is_folder_path(&c.folder) {
            c.folder.clone()
        } else {
            paths.first().map(|p| parent_path(p)).unwrap_or_default()
        };
        more.push(Some(scripts_menu(&cwd, &paths, &c.data)));
        more.push(Some(tags_menu(&paths, &c.data)));
    }
    if one.is_some() && !c.find && !fixed {
        more.push(
            Entry::ask(
                t!("Rename items in this folder…"),
                IconName::PenLine,
                MenuEvent::BatchRenameFolder,
            )
            .hinted("Ctrl+Shift+R")
            .node(),
        );
    }
    if let Some(e) = one.filter(|e| can_back_up(e)) {
        more.push(
            Entry::ask(
                t!("Back up…"),
                IconName::DatabaseBackup,
                MenuEvent::BackUp { folder: e.path.clone() },
            )
            .node(),
        );
    }
    if let Some(e) = folder_one.filter(|_| !fixed) {
        more.push(
            Entry::ask(
                t!("Search here"),
                IconName::Search,
                MenuEvent::SearchText { folder: e.path.clone() },
            )
            .node(),
        );
    }
    more.push(separator());
    more.push(Some(Node::menu(
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
            // A checksum file (`.sha256`, `.sha512`, `.b3sum`): the files it lists are checked against it.
            one.filter(|e| !e.dir && !fixed && glint_core::checksums::is_sums_file(&e.name))
                .and_then(|e| {
                    Entry::ask(
                        t!("Check files in this folder"),
                        IconName::FileCheck,
                        MenuEvent::CheckSums { path: e.path.clone() },
                    )
                    .node()
                }),
            folder_one.filter(|_| !fixed).and_then(|e| {
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
        ],
    )));
    let folder_ok = folder_one.filter(|_| !fixed);
    let mut clean: Vec<Option<Node>> = vec![
        folder_ok.and_then(|e| {
            Entry::ask(
                t!("Find duplicate files"),
                IconName::Files,
                MenuEvent::Duplicates { folder: e.path.clone() },
            )
            .node()
        }),
        folder_ok.and_then(|e| {
            Entry::ask(
                t!("Clean up…"),
                IconName::BrushCleaning,
                MenuEvent::Cleanup { folder: e.path.clone() },
            )
            .node()
        }),
    ];
    if !any_picture {
        clean.extend(personal_rows());
    }
    if shrinks && shrink_home.is_none() {
        clean.push(
            Entry::ask(
                t!("Make smaller…"),
                IconName::Minimize2,
                MenuEvent::MakeSmaller { paths: paths.clone() },
            )
            .node(),
        );
    }
    clean.push(separator());
    clean.push(one.filter(|e| !fixed && on_a_drive(&e.path)).and_then(|e| {
        Entry::ask(
            t!("Copy permalink"),
            IconName::Link,
            MenuEvent::CopyPermalink { path: e.path.clone() },
        )
        .node()
    }));
    more.push(Some(Node::menu(t!("Clean up"), IconName::BrushCleaning, clean)));
    m.push(Some(Node::menu(t!("More"), IconName::Ellipsis, more)));
    m.push(separator());

    // ---- The end ------------------------------------------------------------------------------------------------
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

/// The kinds' "More … tools" submenus: what their sections keep a click further away.
fn more_picture_label() -> SharedString {
    t!("More picture tools")
}

fn more_video_label() -> SharedString {
    t!("More video tools")
}

fn more_audio_label() -> SharedString {
    t!("More audio tools")
}

fn more_pdf_label() -> SharedString {
    t!("More PDF tools")
}

/// The names of the "More … tools" submenus, for the Suggested section (a copy of a row in one of them doesn't
/// repeat the shelf's name).
pub(super) fn kind_shelves() -> [SharedString; 4] {
    [
        more_picture_label(),
        more_video_label(),
        more_audio_label(),
        more_pdf_label(),
    ]
}

/// What Windows' Sandbox is offered for: programs, scripts and installers, and what Windows marked as
/// downloaded from the internet (but never a picture, a recording or a plain text file).
fn runs_in_sandbox(e: &FileEntry, downloaded: bool) -> bool {
    const RUNNABLE: &[&str] = &[
        "exe", "msi", "bat", "cmd", "com", "ps1", "vbs", "js", "jar", "appx", "msix", "msc", "scr", "lnk",
    ];
    const PLAIN: &[&str] = &[
        "txt", "md", "log", "json", "xml", "yml", "yaml", "toml", "ini", "csv", "tsv", "rs", "ts", "py", "html", "css",
        "cfg", "conf", "sql", "svg",
    ];
    if e.dir {
        return false;
    }
    if RUNNABLE.contains(&e.ext()) {
        return true;
    }
    downloaded && !can_convert(e) && !can_wallpaper(e) && !is_media_file(e) && !PLAIN.contains(&e.ext())
}

/// A PDF's rows: the most used ones, then "More PDF tools" with the rest (and Make smaller, when it lives here).
fn pdf_section(pdfs: &[&FileEntry], make_smaller: Option<Node>) -> Vec<Option<Node>> {
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
        tool(t!("Make smaller"), IconName::Minimize2, PdfTool::Optimize),
        Some(Node::menu(
            more_pdf_label(),
            IconName::FileText,
            vec![
                tool(t!("Turn pages…"), IconName::RotateCw, PdfTool::Rotate),
                tool(t!("Save its pictures…"), IconName::Images, PdfTool::Images),
                make_smaller,
                separator(),
                tool(t!("Add a password…"), IconName::KeyRound, PdfTool::Encrypt),
                tool(t!("Take the password off…"), IconName::LockOpen, PdfTool::Decrypt),
                tool(t!("Stamp…"), IconName::Stamp, PdfTool::Stamp),
            ],
        )),
    ]
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

/// The Tags submenu: the seven color tags, each checked when every item has it (choosing it then takes it off
/// them all; otherwise it goes on them all, as with some of them having it already), then "Edit tags…". The
/// workspace handles both events (`dialogs::tags::set_tags`, `dialogs::tags::open`); the checks come from
/// `MenuData::tags`, the tags the file list has read (`MenuData::read_tags`).
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
