//! The sidebar's rows, as plain data.
//! Ported from the structure of `Sidebar` in src/components/explorer/Sidebar.tsx and `TreeKids`/`TreeRow` in
//! FolderTree.tsx.
//!
//! The TypeScript nested components; a virtualised list wants one flat list, so [`build_rows`] walks the same
//! structure (sections, their places, and below an open place its folders) and lists what is on screen top to
//! bottom. It is a pure function of [`Inputs`], so it is tested without a window, and the view rebuilds the list
//! only when an input changed and keeps the rows that did not.

use super::tree::{Kids, Tree};
use crate::explorer::{drive_name, Drive, DriveKind, KnownFolder, KnownId};
use glint_model::paths::{
    base_name, in_archive_path, is_gallery, is_recent, is_remote_path, join_path, same_path, GALLERY, RECENT,
    RECYCLE_BIN, REMOTE, THIS_PC,
};
use glint_model::store::{RemoteConnection, RemoteKind, SavedSearch};
use gpui_kit::SharedString;
use std::collections::HashMap;

/// What a place's icon is. The view maps it to an icon and a color.
#[derive(Clone, Debug, PartialEq)]
pub enum Glyph {
    Recent,
    Gallery,
    Known(KnownId),
    /// A pinned folder (and every folder in the tree): the folder icon in the folder's tone.
    Folder,
    Drive(DriveKind),
    RecycleBin,
    /// A remote connection: its kind, and rclone's service.
    Remote(RemoteKind, String),
}

/// A place's name: one of Glint's own (translated when drawn) or a folder's or connection's (as it is).
#[derive(Clone, Debug, PartialEq)]
pub enum Label {
    Fixed(&'static str),
    Plain(SharedString),
}

/// A place the folder tree can open the folders of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Opens {
    pub open: bool,
    pub loading: bool,
}

/// A place: a known folder, a pinned one, a drive, the Recycle Bin, a connection.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub glyph: Glyph,
    pub label: Label,
    pub path: String,
    /// The folder shown is this one.
    pub active: bool,
    /// Second line of its tooltip, and its accessible description (`title ?? path`).
    pub detail: SharedString,
    /// Set where the tree can open this place.
    pub tree: Option<Opens>,
    /// A drive's or cloud's total and free bytes, for the used-space bar.
    pub capacity: Option<(u64, u64)>,
    /// A drive that can be safely ejected: its name.
    pub eject: Option<String>,
    /// The connection this place is, for its menu.
    pub remote: Option<String>,
    /// The sidebar has room for the tree's arrows (the "Folder tree" setting): places keep clear of them.
    pub gutter: bool,
}

/// A folder in the tree below a place.
#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    pub path: String,
    pub name: SharedString,
    /// Whether it has folders of its own (`None`: not known yet).
    pub has: Option<bool>,
    /// 0 for the folders right below a place.
    pub depth: usize,
    /// The folder it is in (the place's path, or the folder above).
    pub parent: String,
    pub open: bool,
    pub loading: bool,
    pub active: bool,
}

/// A saved search, in the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub struct Search {
    pub query: String,
    pub folder: String,
    /// Found now, by instant search ("1,000+"); where the index can say.
    pub count: Option<String>,
}

/// One row of the sidebar.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    /// A section's title; with a path it is a button that opens it (This PC's).
    Heading {
        title: &'static str,
        link: Option<String>,
        active: bool,
        first: bool,
    },
    Place(Place),
    Folder(Folder),
    /// Why a folder's folders aren't listed ("Can't look inside"), with the reason.
    Note {
        depth: usize,
        reason: String,
    },
    Search(Search),
    /// "Add a connection".
    AddConnection,
}

impl Row {
    /// What the keyboard can land on: places, the tree's folders, searches, the add button and titles that open.
    pub fn focusable(&self) -> bool {
        match self {
            Row::Heading { link, .. } => link.is_some(),
            Row::Note { .. } => false,
            _ => true,
        }
    }

    /// An identity for the row, to find it again after the list is rebuilt (the keyboard's place in it).
    pub fn key(&self) -> String {
        match self {
            Row::Heading { title, .. } => format!("h:{title}"),
            Row::Place(p) => format!("p:{}", p.path),
            Row::Folder(f) => format!("f:{}", f.path),
            Row::Note { depth, .. } => format!("n:{depth}"),
            Row::Search(s) => format!("s:{}\n{}", s.folder, s.query),
            Row::AddConnection => "add".to_string(),
        }
    }

    /// The folder this row opens in the tree, if it is one the tree can open (`data-tree-path`/`data-nav-tree`).
    pub fn tree_path(&self) -> Option<&str> {
        match self {
            Row::Place(p) if p.tree.is_some() => Some(&p.path),
            Row::Folder(f) => Some(&f.path),
            _ => None,
        }
    }

    /// Whether the tree has it open.
    pub fn is_open(&self) -> bool {
        match self {
            Row::Place(p) => p.tree.is_some_and(|t| t.open),
            Row::Folder(f) => f.open,
            _ => false,
        }
    }

    /// Whether it is known to hold no folders (no arrow, nothing to open).
    pub fn is_leaf(&self) -> bool {
        matches!(self, Row::Folder(f) if f.has == Some(false))
    }

    /// The path a click on the row opens.
    pub fn target(&self) -> Option<&str> {
        match self {
            Row::Place(p) => Some(&p.path),
            Row::Folder(f) => Some(&f.path),
            Row::Heading { link: Some(l), .. } => Some(l),
            _ => None,
        }
    }
}

/// How full a cloud is: its total and free bytes.
pub type Space = (u64, u64);

/// Everything the list is made from.
pub struct Inputs<'a> {
    /// The folder shown (`""` for This PC).
    pub path: &'a str,
    pub known: &'a [KnownFolder],
    pub drives: &'a [Drive],
    pub pinned: &'a [String],
    pub saved: &'a [SavedSearch],
    pub remotes: &'a [RemoteConnection],
    /// The "Folder tree" setting.
    pub folder_tree: bool,
    pub tree: &'a Tree,
    /// Cloud space by connection id.
    pub spaces: &'a HashMap<String, Space>,
    /// Saved searches' counts, by [`count_key`].
    pub counts: &'a HashMap<String, String>,
}

/// Where a saved search's count is kept.
pub fn count_key(search: &SavedSearch) -> String {
    format!("{}\n{}", search.folder, search.query)
}

/// A place the folder tree can open: a folder on a drive or a share, not in an archive or on a remote (`opens`).
pub fn opens(path: &str) -> bool {
    let b = path.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    (drive || path.starts_with("\\\\")) && !in_archive_path(path) && !is_remote_path(path)
}

/// A connection's folder at its top (`remotePath`).
pub fn remote_path(remote: &RemoteConnection) -> String {
    format!("{REMOTE}{}", remote.id)
}

/// A service's name: "Google Drive" for "drive", else the service's own word (`serviceName`, without rclone's
/// own descriptions, which need the provider list).
fn service_name(kind: &str) -> String {
    match kind {
        "drive" => "Google Drive",
        "onedrive" => "OneDrive",
        "dropbox" => "Dropbox",
        "box" => "Box",
        "pcloud" => "pCloud",
        "mega" => "MEGA",
        "protondrive" => "Proton Drive",
        "iclouddrive" => "iCloud Drive",
        "s3" => "Amazon S3 and compatible",
        "b2" => "Backblaze B2",
        "azureblob" => "Azure Blob Storage",
        "azurefiles" => "Azure Files",
        "google cloud storage" => "Google Cloud Storage",
        "google photos" => "Google Photos",
        "sftp" => "SFTP",
        "ftp" => "FTP",
        "smb" => "Windows share (SMB)",
        "webdav" => "WebDAV",
        "http" => "A website's files (HTTP)",
        "local" => "A folder on this PC",
        "yandex" => "Yandex Disk",
        "crypt" => "Encrypted folder (rclone crypt)",
        other => other,
    }
    .to_string()
}

/// Where a connection goes, in a few words: the sidebar's tooltip (`remoteWhere`). English, as the TypeScript's.
pub fn remote_where(r: &RemoteConnection) -> String {
    let root = r.root.trim();
    let folder = if root.is_empty() {
        String::new()
    } else {
        format!(", in {root}")
    };
    match r.kind {
        RemoteKind::Rclone => format!(
            "{} through rclone (its remote “{}”){folder}",
            service_name(r.service.as_deref().unwrap_or("")),
            r.rclone.as_deref().unwrap_or("")
        ),
        RemoteKind::Device => format!(
            "The folders “{}” shares with this PC, over Glint's devices (read-only)",
            r.name
        ),
        RemoteKind::Sftp => {
            let user = if r.user.is_empty() {
                String::new()
            } else {
                format!("{}@", r.user)
            };
            let port = match r.port {
                Some(p) if p != 0 && p != 22 => format!(":{p}"),
                _ => String::new(),
            };
            format!("SFTP at {user}{}{port}{folder}", r.host.as_deref().unwrap_or(""))
        }
        RemoteKind::S3 => {
            let endpoint = r.endpoint.trim();
            let at = if endpoint.is_empty() {
                let region = r.region.trim();
                format!("Amazon S3 ({})", if region.is_empty() { "us-east-1" } else { region })
            } else {
                endpoint.to_string()
            };
            format!("S3 bucket “{}” at {at}{folder}", r.bucket)
        }
        RemoteKind::Webdav => format!("WebDAV at {}{folder}", r.endpoint),
    }
}

/// Whether a search's filters read the files, which the index can't count (`READS_FILES`).
pub fn reads_files(query: &str) -> bool {
    let lower = query.to_lowercase();
    [
        "width:",
        "height:",
        "taken:",
        "camera:",
        "duration:",
        "artist:",
        "album:",
        "shows:",
        "says:",
        "tag:",
    ]
    .iter()
    .any(|filter| lower.contains(filter))
}

/// The rows, top to bottom.
pub fn build_rows(i: &Inputs) -> Vec<Row> {
    let here = |p: &str| i.path != THIS_PC && same_path(p, i.path);
    let tree_of = |path: &str| -> Option<Opens> {
        (i.folder_tree && opens(path)).then(|| Opens {
            open: i.tree.is_open(path),
            loading: i.tree.is_loading(path),
        })
    };
    let mut rows: Vec<Row> = Vec::new();

    // Quick access: Recent, Gallery, then the known folders.
    rows.push(Row::Heading {
        title: "Quick access",
        link: None,
        active: false,
        first: true,
    });
    rows.push(Row::Place(Place {
        glyph: Glyph::Recent,
        label: Label::Fixed("Recent"),
        path: RECENT.to_string(),
        active: is_recent(i.path),
        detail: "The files you opened lately, here or in any app, newest first".into(),
        tree: None,
        capacity: None,
        eject: None,
        remote: None,
        gutter: i.folder_tree,
    }));
    rows.push(Row::Place(Place {
        glyph: Glyph::Gallery,
        label: Label::Fixed("Gallery"),
        path: GALLERY.to_string(),
        active: is_gallery(i.path),
        detail: "Your photos and videos, newest first by when each was taken".into(),
        tree: None,
        capacity: None,
        eject: None,
        remote: None,
        gutter: i.folder_tree,
    }));
    for k in i.known {
        rows.push(Row::Place(Place {
            glyph: Glyph::Known(k.id),
            label: Label::Fixed(k.name),
            path: k.path.clone(),
            active: here(&k.path),
            detail: k.path.clone().into(),
            tree: tree_of(&k.path),
            capacity: None,
            eject: None,
            remote: None,
            gutter: i.folder_tree,
        }));
        if tree_of(&k.path).is_some() {
            push_kids(&mut rows, i, &k.path, 0);
        }
    }

    if !i.pinned.is_empty() {
        rows.push(heading("Pinned"));
        for p in i.pinned {
            rows.push(Row::Place(Place {
                glyph: Glyph::Folder,
                label: Label::Plain(base_name(p).to_string().into()),
                path: p.clone(),
                active: here(p),
                detail: p.clone().into(),
                tree: tree_of(p),
                capacity: None,
                eject: None,
                remote: None,
                gutter: i.folder_tree,
            }));
            if tree_of(p).is_some() {
                push_kids(&mut rows, i, p, 0);
            }
        }
    }

    // Tags: PORT ME(tags) belongs to the tags part.

    if !i.saved.is_empty() {
        rows.push(heading("Searches"));
        for s in i.saved {
            rows.push(Row::Search(Search {
                query: s.query.clone(),
                folder: s.folder.clone(),
                count: i.counts.get(&count_key(s)).cloned(),
            }));
        }
    }

    // This PC: its title opens it; the drives; the Recycle Bin.
    rows.push(Row::Heading {
        title: "This PC",
        link: Some(THIS_PC.to_string()),
        active: i.path == THIS_PC,
        first: false,
    });
    for d in i.drives {
        let name = drive_name(d);
        let free = crate::kit::free_of(d.total, d.free);
        let ejectable = matches!(d.kind, DriveKind::Removable | DriveKind::Cdrom);
        rows.push(Row::Place(Place {
            glyph: Glyph::Drive(d.kind),
            label: Label::Plain(name.clone().into()),
            path: d.path.clone(),
            active: here(&d.path),
            detail: if free.is_empty() { d.path.clone().into() } else { free },
            tree: if d.kind == DriveKind::Wsl {
                None
            } else {
                tree_of(&d.path)
            },
            capacity: (d.total != 0).then_some((d.total, d.free)),
            eject: ejectable.then(|| name.clone()),
            remote: None,
            gutter: i.folder_tree,
        }));
        if d.kind != DriveKind::Wsl && tree_of(&d.path).is_some() {
            push_kids(&mut rows, i, &d.path, 0);
        }
    }
    rows.push(Row::Place(Place {
        glyph: Glyph::RecycleBin,
        label: Label::Fixed("Recycle Bin"),
        path: RECYCLE_BIN.to_string(),
        active: here(RECYCLE_BIN),
        detail: "What you deleted, to restore or delete for good. Files dropped here are deleted.".into(),
        tree: None,
        capacity: None,
        eject: None,
        remote: None,
        gutter: i.folder_tree,
    }));

    // Remote connections.
    rows.push(heading("Remote"));
    for r in i.remotes {
        let path = remote_path(r);
        let space = i.spaces.get(&r.id).copied();
        let where_ = remote_where(r);
        rows.push(Row::Place(Place {
            glyph: Glyph::Remote(r.kind, r.service.clone().unwrap_or_default()),
            label: Label::Plain(r.name.clone().into()),
            active: here(&path),
            detail: match space {
                Some((total, free)) => format!("{where_}\n{}", crate::kit::free_of(total, free)).into(),
                None => where_.into(),
            },
            path,
            tree: None,
            capacity: space,
            eject: None,
            remote: Some(r.id.clone()),
            gutter: i.folder_tree,
        }));
    }
    rows.push(Row::AddConnection);
    rows
}

fn heading(title: &'static str) -> Row {
    Row::Heading {
        title,
        link: None,
        active: false,
        first: false,
    }
}

/// The folders in `path`, while it is open in the tree, each a row (and its folders, while open): `TreeKids`.
fn push_kids(rows: &mut Vec<Row>, i: &Inputs, path: &str, depth: usize) {
    if !i.tree.is_open(path) {
        return;
    }
    match i.tree.kids(path) {
        None | Some(Kids::Loading) => {}
        Some(Kids::Failed(reason)) => rows.push(Row::Note {
            depth,
            reason: reason.clone(),
        }),
        Some(Kids::Listed(list)) => {
            for kid in list {
                let child = join_path(path, &kid.name);
                let open = i.tree.is_open(&child);
                rows.push(Row::Folder(Folder {
                    path: child.clone(),
                    name: kid.name.clone().into(),
                    has: kid.children,
                    depth,
                    parent: path.to_string(),
                    open,
                    loading: i.tree.is_loading(&child),
                    active: i.path != THIS_PC && same_path(&child, i.path),
                }));
                push_kids(rows, i, &child, depth + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::sidebar::tree::Subfolder;

    fn known(id: KnownId, path: &str) -> KnownFolder {
        KnownFolder {
            id,
            name: id.name(),
            path: path.to_string(),
        }
    }

    fn drive(path: &str, label: &str, kind: DriveKind, total: u64, free: u64) -> Drive {
        Drive {
            path: path.into(),
            label: label.into(),
            kind,
            fs: "NTFS".into(),
            total,
            free,
        }
    }

    fn remote(id: &str, name: &str, kind: RemoteKind) -> RemoteConnection {
        RemoteConnection {
            id: id.into(),
            name: name.into(),
            kind,
            ..Default::default()
        }
    }

    struct World {
        known: Vec<KnownFolder>,
        drives: Vec<Drive>,
        pinned: Vec<String>,
        saved: Vec<SavedSearch>,
        remotes: Vec<RemoteConnection>,
        tree: Tree,
        spaces: HashMap<String, Space>,
        counts: HashMap<String, String>,
    }

    impl World {
        fn new() -> World {
            World {
                known: vec![
                    known(KnownId::Home, "C:\\Users\\me"),
                    known(KnownId::Documents, "C:\\Users\\me\\Documents"),
                ],
                drives: vec![
                    drive("C:\\", "", DriveKind::Fixed, 1000, 100),
                    drive("E:\\", "Photos", DriveKind::Removable, 0, 0),
                    drive("\\\\wsl.localhost\\Ubuntu", "Ubuntu", DriveKind::Wsl, 0, 0),
                ],
                pinned: vec!["D:\\Work\\تقارير".to_string()],
                saved: vec![SavedSearch {
                    query: "invoice".into(),
                    folder: THIS_PC.into(),
                }],
                remotes: vec![remote("nas", "My NAS", RemoteKind::Sftp)],
                tree: Tree::default(),
                spaces: HashMap::new(),
                counts: HashMap::new(),
            }
        }

        fn rows(&self, path: &str, folder_tree: bool) -> Vec<Row> {
            build_rows(&Inputs {
                path,
                known: &self.known,
                drives: &self.drives,
                pinned: &self.pinned,
                saved: &self.saved,
                remotes: &self.remotes,
                folder_tree,
                tree: &self.tree,
                spaces: &self.spaces,
                counts: &self.counts,
            })
        }
    }

    fn describe(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Heading { title, .. } => format!("# {title}"),
                Row::Place(p) => match &p.label {
                    Label::Fixed(s) => (*s).to_string(),
                    Label::Plain(s) => s.to_string(),
                },
                Row::Folder(f) => format!("{}> {}", " ".repeat(f.depth), f.name),
                Row::Note { reason, .. } => format!("! {reason}"),
                Row::Search(s) => format!("? {}", s.query),
                Row::AddConnection => "+ add".to_string(),
            })
            .collect()
    }

    #[test]
    fn the_sections_come_in_the_typescripts_order() {
        let world = World::new();
        assert_eq!(
            describe(&world.rows("C:\\Users\\me", true)),
            [
                "# Quick access",
                "Recent",
                "Gallery",
                "Home",
                "Documents",
                "# Pinned",
                "تقارير",
                "# Searches",
                "? invoice",
                "# This PC",
                "Local Disk (C:)",
                "Photos (E:)",
                "Ubuntu (WSL)",
                "Recycle Bin",
                "# Remote",
                "My NAS",
                "+ add",
            ]
        );
        // Nothing pinned or saved: those sections are not there.
        let mut bare = World::new();
        bare.pinned.clear();
        bare.saved.clear();
        let text = describe(&bare.rows("", true));
        assert!(!text.contains(&"# Pinned".to_string()) && !text.contains(&"# Searches".to_string()));
    }

    #[test]
    fn the_folder_shown_is_marked_in_every_place_that_is_it_and_this_pc_only_by_its_title() {
        let mut world = World::new();
        world.pinned.push("C:\\Users\\me".to_string());
        let rows = world.rows("c:\\users\\ME\\", true);
        let active: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Place(p) if p.active => Some(p.path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            active,
            ["C:\\Users\\me", "C:\\Users\\me"],
            "Home and the pinned copy of it"
        );
        // On This PC nothing is a folder shown; the title is marked.
        let rows = world.rows(THIS_PC, true);
        assert!(rows.iter().all(|r| !matches!(r, Row::Place(p) if p.active)));
        assert!(rows.iter().any(|r| matches!(
            r,
            Row::Heading {
                title: "This PC",
                active: true,
                ..
            }
        )));
        // The Recycle Bin is its own place.
        let rows = world.rows(RECYCLE_BIN, true);
        assert!(rows
            .iter()
            .any(|r| matches!(r, Row::Place(p) if p.glyph == Glyph::RecycleBin && p.active)));
    }

    #[test]
    fn drives_carry_their_space_and_only_removable_ones_eject_and_wsl_has_no_tree() {
        let world = World::new();
        let rows = world.rows("", true);
        let place = |path: &str| {
            rows.iter()
                .find_map(|r| match r {
                    Row::Place(p) if p.path == path => Some(p.clone()),
                    _ => None,
                })
                .unwrap()
        };
        let c = place("C:\\");
        assert_eq!(c.capacity, Some((1000, 100)));
        assert_eq!(c.detail, "100 B free of 1000 B");
        assert!(c.eject.is_none() && c.tree.is_some());
        let e = place("E:\\");
        assert_eq!(e.eject.as_deref(), Some("Photos (E:)"));
        assert_eq!(e.capacity, None);
        assert_eq!(e.detail, "E:\\", "no total: the tooltip says the path");
        let wsl = place("\\\\wsl.localhost\\Ubuntu");
        assert!(wsl.tree.is_none());
    }

    #[test]
    fn the_tree_hangs_below_its_place_and_nests_while_open() {
        let mut world = World::new();
        // Home is open with two folders; Documents inside Home is open too, and Home's own place is the parent.
        world.tree.set_open("C:\\Users\\me", true);
        let read = world.tree.begin_read("C:\\Users\\me");
        world.tree.finish_read(
            "C:\\Users\\me",
            read,
            Ok(vec![
                Subfolder {
                    name: "Documents".into(),
                    children: Some(true),
                },
                Subfolder {
                    name: "Music".into(),
                    children: Some(false),
                },
            ]),
        );
        world.tree.set_open("C:\\Users\\me\\Documents", true);
        let read = world.tree.begin_read("C:\\Users\\me\\Documents");
        world.tree.finish_read(
            "C:\\Users\\me\\Documents",
            read,
            Ok(vec![Subfolder {
                name: "Reports".into(),
                children: None,
            }]),
        );
        let rows = world.rows("C:\\Users\\me\\Documents\\Reports", true);
        let text = describe(&rows);
        let home = text.iter().position(|t| t == "Home").unwrap();
        assert_eq!(text[home + 1], "> Documents");
        assert_eq!(text[home + 2], " > Reports");
        assert_eq!(text[home + 3], "> Music");
        let reports = rows
            .iter()
            .find_map(|r| match r {
                Row::Folder(f) if f.name == "Reports" => Some(f.clone()),
                _ => None,
            })
            .unwrap();
        assert!(reports.active && reports.depth == 1);
        assert_eq!(reports.parent, "C:\\Users\\me\\Documents");
        assert_eq!(reports.path, "C:\\Users\\me\\Documents\\Reports");
        // The Documents place (a known folder) is the same folder, open in the tree, so it lists Reports too.
        assert_eq!(text.iter().filter(|t| t.trim() == "> Reports").count(), 2);
        // Without the setting there is no tree at all.
        let flat = world.rows("C:\\Users\\me", false);
        assert!(flat.iter().all(|r| !matches!(r, Row::Folder(_))));
        assert!(flat
            .iter()
            .all(|r| !matches!(r, Row::Place(p) if p.tree.is_some() || p.gutter)));
    }

    #[test]
    fn a_folder_that_cannot_be_read_says_so() {
        let mut world = World::new();
        world.tree.set_open("C:\\", true);
        let read = world.tree.begin_read("C:\\");
        world.tree.finish_read("C:\\", read, Err("Access is denied".into()));
        let text = describe(&world.rows("", true));
        let c = text.iter().position(|t| t == "Local Disk (C:)").unwrap();
        assert_eq!(text[c + 1], "! Access is denied");
    }

    #[test]
    fn connections_say_where_they_go_and_show_their_space() {
        let mut world = World::new();
        world.remotes = vec![
            RemoteConnection {
                host: Some("nas.local".into()),
                user: "me".into(),
                port: Some(2222),
                root: "/srv".into(),
                ..remote("nas", "My NAS", RemoteKind::Sftp)
            },
            RemoteConnection {
                rclone: Some("gd".into()),
                service: Some("drive".into()),
                ..remote("gd", "Drive", RemoteKind::Rclone)
            },
        ];
        world.spaces.insert("gd".into(), (2000, 500));
        let rows = world.rows("remote:gd", true);
        let places: Vec<_> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Place(p) if p.remote.is_some() => Some(p.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(places[0].path, "remote:nas");
        assert_eq!(places[0].detail, "SFTP at me@nas.local:2222, in /srv");
        assert!(
            places[1].active && !places[0].active,
            "the connection's top is the folder shown"
        );
        assert_eq!(
            places[1].detail,
            "Google Drive through rclone (its remote “gd”)\n500 B free of 2.0 KB"
        );
        assert_eq!(places[1].capacity, Some((2000, 500)));
        assert!(places[1].tree.is_none(), "a connection's folders are not in the tree");
        assert_eq!(
            remote_where(&RemoteConnection {
                bucket: "b".into(),
                region: " ".into(),
                ..remote("s", "S", RemoteKind::S3)
            }),
            "S3 bucket “b” at Amazon S3 (us-east-1)"
        );
        assert_eq!(
            remote_where(&RemoteConnection {
                endpoint: "https://dav.example/x".into(),
                ..remote("w", "W", RemoteKind::Webdav)
            }),
            "WebDAV at https://dav.example/x"
        );
        assert_eq!(
            remote_where(&remote("d", "Laptop", RemoteKind::Device)),
            "The folders “Laptop” shares with this PC, over Glint's devices (read-only)"
        );
    }

    #[test]
    fn only_folders_on_drives_and_shares_open_in_the_tree() {
        assert!(opens("C:\\Users"));
        assert!(opens("c:\\"));
        assert!(opens("\\\\server\\share\\x"));
        assert!(!opens("C:"));
        assert!(!opens("remote:nas"));
        assert!(!opens("shell:Recent"));
        assert!(!opens(""));
    }

    #[test]
    fn searches_that_read_the_files_cannot_be_counted_by_the_index() {
        assert!(reads_files("Width:>1000 cat"));
        assert!(reads_files("tag:\"Work\""));
        assert!(reads_files("says:hello"));
        assert!(!reads_files("invoice kind:pdf"));
        assert!(!reads_files("shows up"), "needs the colon");
    }

    #[test]
    fn rows_know_what_the_keyboard_can_do_with_them() {
        let mut world = World::new();
        world.tree.set_open("C:\\Users\\me", true);
        let read = world.tree.begin_read("C:\\Users\\me");
        world.tree.finish_read(
            "C:\\Users\\me",
            read,
            Ok(vec![Subfolder {
                name: "Empty".into(),
                children: Some(false),
            }]),
        );
        let rows = world.rows("", true);
        assert!(!rows[0].focusable(), "a plain title");
        assert!(rows
            .iter()
            .any(|r| matches!(r, Row::Heading { link: Some(_), .. } if r.focusable())));
        let home = rows.iter().find(|r| r.tree_path() == Some("C:\\Users\\me")).unwrap();
        assert!(home.is_open() && !home.is_leaf());
        let empty = rows
            .iter()
            .find(|r| matches!(r, Row::Folder(f) if f.name == "Empty"))
            .unwrap();
        assert!(empty.is_leaf() && !empty.is_open());
        assert!(rows.iter().any(|r| r.target() == Some(RECENT)));
        // Keys tell the same path in two places apart from nothing, and are stable between rebuilds.
        assert_eq!(home.key(), "p:C:\\Users\\me");
        assert_eq!(
            rows.iter().map(Row::key).collect::<Vec<_>>(),
            world.rows("", true).iter().map(Row::key).collect::<Vec<_>>()
        );
    }
}
