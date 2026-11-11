//! The menu of a folder: the one shown (right-click on empty space), a place in the sidebar, a tile of This PC.
//! Ported from `folderMenu` in src/components/explorer/Explorer.tsx.
//!
//! [`folder_menu`] reads and returns data, like [`item_menu`](super::item::item_menu). In an archive what would
//! change it is greyed out, as it is in a folder Windows keeps this account out of ([`FolderView::no_access`]),
//! where Get access comes first and what needs to read the folder goes, unless the index listed it
//! ([`FolderView::from_index`]).

use super::data::MenuData;
use super::event::MenuEvent;
use super::item::{closed_note, open_in_menu, read_only_note, remote_note, scripts_menu};
use super::kinds::on_a_drive;
use super::spec::{is_this_pc, separator, visible_items, when, Command, Entry, Node, Task};
use crate::explorer::{NoAccess, TabId};
use crate::kit::prelude::*;
use crate::t;
use glint_model::entries::InArchive;
use glint_model::paths::{in_archive_path, is_folder_path, is_recycle_bin, is_remote_path};

/// What is known of the folder when it is the one open in a pane (the TypeScript's `view`).
#[derive(Clone, Debug, Default)]
pub struct FolderView {
    /// A search's results are shown.
    pub find: bool,
    /// The archive the folder is in.
    pub archive: Option<InArchive>,
    /// Why Windows keeps the folder closed to this account, if it does.
    pub no_access: Option<NoAccess>,
    /// The index listed the folder though it is closed.
    pub from_index: bool,
    /// The tab, for Get access.
    pub tab: Option<TabId>,
    /// In the Recycle Bin: "Restore all items" or "Restore the N items shown", and the paths it restores.
    pub restore_shown: Option<(SharedString, Vec<String>)>,
}

/// A remote connection whose top folder the menu is for (`remoteOfPath`).
#[derive(Clone, Debug, PartialEq)]
pub struct Connection {
    /// Its id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// A paired PC: its connection is changed in Settings, not here.
    pub device: bool,
}

/// What the folder menu reads besides the folder.
#[derive(Clone, Debug, Default)]
pub struct FolderCtx {
    /// The pinned folders.
    pub pinned: Vec<String>,
    /// The folder is open in a pane (`current`), with what is known of it.
    pub view: Option<FolderView>,
    /// The connection, when the folder is a remote's top.
    pub remote: Option<Connection>,
    /// The lists from [`MenuData`].
    pub data: MenuData,
}

/// "Undo", saying what it takes back (`undoLabel`).
pub fn undo_label(data: &MenuData) -> SharedString {
    match &data.undo {
        Some(step) => t!("Undo the {step}", step = step),
        None => t!("Undo"),
    }
}

/// The menu of `folder`. `current` is the folder open in a pane (right-click on empty space) rather than a
/// place or a tile.
pub fn folder_menu(folder: &str, current: bool, c: &FolderCtx) -> Vec<Node> {
    let owned = folder.to_string();
    let view = c.view.as_ref().filter(|_| current);
    if is_this_pc(folder) {
        return vec![Entry::task(t!("Refresh"), IconName::RotateCw, Task::RefreshDrives)
            .hint_of(Command::Refresh)
            .node()]
        .into_iter()
        .flatten()
        .collect();
    }
    let pinned = c.pinned.iter().any(|p| p == folder);
    let pin = || {
        Entry::task(
            if pinned {
                t!("Unpin from sidebar")
            } else {
                t!("Pin to sidebar")
            },
            if pinned { IconName::PinOff } else { IconName::Pin },
            Task::TogglePin(owned.clone()),
        )
        .active(pinned)
    };
    let copy_path = || {
        Entry::task(
            t!("Copy path"),
            IconName::ClipboardCopy,
            Task::CopyText {
                text: owned.clone(),
                paths: 1,
            },
        )
    };
    let refresh = || Entry::command(t!("Refresh"), IconName::RotateCw, Command::Refresh);
    let select_all =
        |disabled: bool| Entry::command(t!("Select all"), IconName::ListChecks, Command::SelectAll).disabled(disabled);
    let undo = || Entry::command(undo_label(&c.data), IconName::Undo2, Command::Undo).disabled(c.data.undo.is_none());

    // A remote's folder: read-only; its connection is changed from its top.
    if is_remote_path(folder) {
        let remote = c.remote.as_ref();
        let mut m: Vec<Option<Node>> = vec![
            when(!current, || Node::Item(open_entry(&owned))),
            when(!current, || Node::Item(open_in_new_tab_entry(&owned))),
            when(current && view.is_some(), || Node::Item(select_all(false))),
            when(current, || Node::Item(refresh())),
            separator(),
            copy_path().node(),
            when(remote.is_none(), || Node::Item(pin())),
        ];
        if let Some(r) = remote {
            m.push(separator());
            // As OneDrive does: in File Explorer and every app's Open dialog, each file downloaded when it's read.
            m.push(
                if c.data.on_demand.iter().any(|id| id == &r.id) {
                    Entry::ask(
                        t!("Stop showing in File Explorer"),
                        IconName::EyeOff,
                        MenuEvent::HideOnDemand { id: r.id.clone() },
                    )
                } else {
                    Entry::ask(
                        t!("Show in File Explorer, on demand"),
                        IconName::AppWindow,
                        MenuEvent::ShowOnDemand {
                            id: r.id.clone(),
                            name: r.name.clone(),
                        },
                    )
                }
                .node(),
            );
            if !r.device {
                m.push(
                    Entry::ask(
                        t!("Edit the connection…"),
                        IconName::PlugZap,
                        MenuEvent::EditConnection { id: r.id.clone() },
                    )
                    .node(),
                );
            }
            m.push(
                Entry::ask(
                    t!("Remove the connection…"),
                    IconName::Trash,
                    MenuEvent::RemoveConnection { id: r.id.clone() },
                )
                .node(),
            );
        }
        m.push(separator());
        m.push(remote_note());
        return visible_items(m.into_iter().flatten().collect());
    }

    // The Recycle Bin: its items go back, or for good.
    if is_recycle_bin(folder) {
        let m: Vec<Option<Node>> = vec![
            when(!current, || Node::Item(open_entry(&owned))),
            when(!current, || Node::Item(open_in_new_tab_entry(&owned))),
            view.and_then(|v| v.restore_shown.as_ref()).and_then(|(label, paths)| {
                Entry::ask(
                    label.clone(),
                    IconName::RotateCcw,
                    MenuEvent::Restore { paths: paths.clone() },
                )
                .node()
            }),
            when(current && view.is_some(), || Node::Item(select_all(false))),
            when(current, || Node::Item(undo())),
            when(current, || Node::Item(refresh())),
            separator(),
            Entry::ask(t!("Empty the Recycle Bin"), IconName::Trash, MenuEvent::EmptyRecycleBin)
                .danger(true)
                .node(),
        ];
        return visible_items(m.into_iter().flatten().collect());
    }

    let find = view.is_some_and(|v| v.find);
    let archive = view.and_then(|v| v.archive.clone());
    let no_access = view.and_then(|v| v.no_access);
    let shut = no_access.is_some();
    let unlisted = shut && !view.is_some_and(|v| v.from_index);
    let can_get = no_access.is_some_and(|n| !n.remote && !n.refused);
    // A pinned folder can be inside an archive too.
    let in_archive = archive.is_some() || in_archive_path(folder);
    let folder_path = is_folder_path(folder);
    let reads = !in_archive && !shut;

    let mut strip: Vec<Entry> = Vec::new();
    strip.push(
        if current {
            Entry::command(t!("Paste"), IconName::ClipboardPaste, Command::Paste)
        } else {
            Entry::ask(
                t!("Paste into folder"),
                IconName::ClipboardPaste,
                MenuEvent::PasteInto { folder: owned.clone() },
            )
        }
        .disabled((current && find) || in_archive || shut),
    );
    if current {
        strip.push(undo());
    }
    strip.push(copy_path());
    strip.push(pin());
    if current {
        strip.push(refresh());
    }

    let new_menu = when(current, || Node::Menu {
        label: t!("New"),
        icon: Some(IconName::FilePlus),
        disabled: find || archive.is_some() || shut,
        items: new_items(&c.data),
    });

    let mut m: Vec<Option<Node>> = vec![
        Some(Node::Strip(strip)),
        when(can_get, || {
            Node::Item(Entry::task(
                t!("Get access"),
                IconName::ShieldCheck,
                Task::GetAccess(view.and_then(|v| v.tab)),
            ))
        }),
        when(can_get, || Node::Separator),
        when(!current, || Node::Item(open_entry(&owned))),
        when(!current, || Node::Item(open_in_new_tab_entry(&owned))),
        when(current, || {
            Node::Item(
                Entry::command(t!("New folder"), IconName::FolderPlus, Command::NewFolder)
                    .disabled(find || archive.is_some() || shut),
            )
        }),
        new_menu,
        when(current, || {
            Node::Item(
                Entry::ask(
                    t!("Paste with the copy queue"),
                    IconName::ClipboardPaste,
                    MenuEvent::PasteQueued { folder: owned.clone() },
                )
                .disabled(find || in_archive || shut),
            )
        }),
        when(current && view.is_some(), || Node::Item(select_all(unlisted))),
        separator(),
        when(current, || {
            Node::Item(
                Entry::ask(t!("Folder sizes"), IconName::HardDrive, MenuEvent::FolderSizes).disabled(find || unlisted),
            )
        }),
        when(reads, || {
            Node::Item(Entry::ask(
                t!("Search here"),
                IconName::Search,
                MenuEvent::SearchText { folder: owned.clone() },
            ))
        }),
    ];

    m.push(Some(Node::menu(
        t!("Inspect & check"),
        IconName::ScanSearch,
        vec![
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Recent changes"),
                    IconName::FileClock,
                    MenuEvent::Timeline { folder: owned.clone() },
                ))
            }),
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Moved and deleted here"),
                    IconName::ClockArrowLeft,
                    MenuEvent::History { folder: owned.clone() },
                ))
            }),
            when(reads && on_a_drive(folder), || {
                Node::Item(Entry::ask(
                    t!("Previous versions of this folder"),
                    IconName::ArchiveRestore,
                    MenuEvent::PreviousVersions { path: owned.clone() },
                ))
            }),
            when(!in_archive, || {
                Node::Item(Entry::ask(
                    t!("What's locking this?"),
                    IconName::Lock,
                    MenuEvent::Locks {
                        paths: vec![owned.clone()],
                    },
                ))
            }),
            separator(),
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Scan with Microsoft Defender"),
                    IconName::ShieldCheck,
                    MenuEvent::Defender {
                        paths: vec![owned.clone()],
                    },
                ))
            }),
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Look for passwords and keys"),
                    IconName::KeyRound,
                    MenuEvent::Secrets { path: owned.clone() },
                ))
            }),
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Check with YARA rules"),
                    IconName::ScanSearch,
                    MenuEvent::Yara { path: owned.clone() },
                ))
            }),
        ],
    )));
    m.push(Some(Node::menu(
        t!("Clean up & share"),
        IconName::BrushCleaning,
        vec![
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Find duplicate files"),
                    IconName::Files,
                    MenuEvent::Duplicates { folder: owned.clone() },
                ))
            }),
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Clean up…"),
                    IconName::BrushCleaning,
                    MenuEvent::Cleanup { folder: owned.clone() },
                ))
            }),
            when(reads && folder_path, || {
                Node::Item(Entry::ask(
                    t!("A rule for this folder…"),
                    IconName::WandSparkles,
                    MenuEvent::Rule { folder: owned.clone() },
                ))
            }),
            when(reads, || {
                Node::Item(Entry::ask(
                    t!("Export as ncdu JSON…"),
                    IconName::FileBraces,
                    MenuEvent::ExportNcdu { folder: owned.clone() },
                ))
            }),
            separator(),
            when(reads && folder_path, || {
                Node::Item(Entry::ask(
                    if current {
                        t!("Send this folder to another PC…")
                    } else {
                        t!("Send to another PC…")
                    },
                    IconName::Send,
                    MenuEvent::SendToPc { path: owned.clone() },
                ))
            }),
            when(reads && folder_path, || {
                Node::Item(Entry::ask(
                    if current {
                        t!("Share this folder on this network…")
                    } else {
                        t!("Share on this network…")
                    },
                    IconName::Wifi,
                    MenuEvent::ShareOnNetwork { path: owned.clone() },
                ))
            }),
            when(current && reads && !find && folder_path, || {
                Node::Item(Entry::ask(
                    t!("Receive from another PC here…"),
                    IconName::Download,
                    MenuEvent::ReceiveHere { folder: owned.clone() },
                ))
            }),
        ],
    )));
    m.push(separator());
    m.push(
        match &archive {
            Some(archive) => Entry::ask(
                t!("Show the archive in File Explorer"),
                IconName::FolderSearch,
                MenuEvent::RevealArchive {
                    archive: archive.outer.clone(),
                },
            ),
            None => Entry::task(
                t!("Open in File Explorer"),
                IconName::FolderSearch,
                Task::OpenInFileExplorer(owned.clone()),
            ),
        }
        .node(),
    );
    m.push(when(reads, || {
        Node::Item(Entry::task(
            t!("Open in Terminal"),
            IconName::SquareTerminal,
            Task::OpenTerminal(owned.clone()),
        ))
    }));
    m.push(when(reads && !c.data.openers.is_empty(), || {
        open_in_menu(folder, &c.data)
    }));
    m.push(when(reads && folder_path, || {
        scripts_menu(folder, std::slice::from_ref(&owned), &c.data)
    }));
    m.push(separator());
    m.push(
        match &archive {
            Some(archive) => Entry::task(
                t!("Archive properties"),
                IconName::Info,
                Task::ShowProperties(archive.outer.clone()),
            ),
            None => {
                let entry = Entry::task(t!("Properties"), IconName::Info, Task::ShowProperties(owned.clone()));
                if current {
                    entry.hint_of(Command::Properties)
                } else {
                    entry
                }
            }
        }
        .node(),
    );
    let from_index = view.is_some_and(|v| v.from_index);
    if archive.is_some() || from_index {
        m.push(separator());
    }
    if archive.is_some() {
        m.push(read_only_note());
    }
    if from_index {
        m.push(closed_note());
    }
    visible_items(m.into_iter().flatten().collect())
}

fn open_entry(folder: &str) -> Entry {
    Entry::task(t!("Open"), IconName::FolderOpen, Task::Navigate(folder.to_string()))
}

fn open_in_new_tab_entry(folder: &str) -> Entry {
    Entry::task(
        t!("Open in new tab"),
        IconName::SquarePlus,
        Task::OpenInNewTabs(vec![folder.to_string()]),
    )
}

/// What New makes in the open folder: a text document, and the kinds of file Windows' New menu has (Word,
/// Excel, a zip…) (`newMenuItems(false)`). Each kind shows its own icon (File Explorer's picture of it, from
/// the kind's PNG), or a plain new-file icon when it has none.
fn new_items(data: &MenuData) -> Vec<Node> {
    let mut items = vec![Node::Item(Entry::ask(
        t!("Text document"),
        IconName::FileText,
        MenuEvent::NewTextDocument,
    ))];
    if !data.new_kinds.is_empty() {
        items.push(Node::Separator);
    }
    items.extend(data.new_kinds.iter().map(|kind| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use glint_core::newmenu::NewKind;

    fn kind(ext: &str, name: &str, icon: Option<&str>) -> NewKind {
        NewKind {
            ext: ext.into(),
            name: name.into(),
            icon: icon.map(str::to_string),
            by_app: false,
        }
    }

    #[test]
    fn each_kind_in_the_new_menu_carries_its_own_icon() {
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
        let data = MenuData {
            new_kinds: vec![
                kind(".docx", "Microsoft Word Document", Some(png)),
                kind(".zip", "Zip", None),
            ],
            ..MenuData::default()
        };
        let nodes = new_items(&data);
        let entries: Vec<&Entry> = nodes
            .iter()
            .filter_map(|node| match node {
                Node::Item(entry) => Some(entry),
                _ => None,
            })
            .collect();
        assert_eq!(entries.len(), 3, "text document, then the two kinds");
        // The text document keeps its icon.
        assert!(entries[0].picture.is_none());
        assert!(entries[1].picture.is_some(), "Word has the picture File Explorer shows");
        assert_eq!(entries[1].icon, Some(IconName::FilePlus));
        // A kind with no icon falls back to the new-file icon.
        assert!(entries[2].picture.is_none());
    }
}
