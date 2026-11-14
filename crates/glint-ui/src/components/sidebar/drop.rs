//! What a drop on a place or a folder in the tree does.
//! Ported from the `data-drop` rules of Sidebar.tsx and FolderTree.tsx, and `targetAt`/`tell` in Explorer.tsx.
//!
//! A pure function from the row to its [`Target`], and from the target and the files being dragged to the
//! words on the drag picture ([`Under`]), so the rules are tested without a window.

use super::rows::Row;
use crate::platform::dragdrop::Under;
use crate::t;
use glint_model::paths::{droppable, in_archive_path, is_gallery, is_recent, is_recycle_bin, is_remote_path};

/// Where a drop on a row goes.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Target {
    /// Into this folder: copied, or moved with Shift.
    Folder(String),
    /// The Recycle Bin: the files are recycled, whatever the keys.
    Bin,
}

/// What a drop on `row` would do, or `None` for a row that takes no files: This PC (a title), Recent and
/// Gallery (views, not folders), a pinned folder in an archive and a cloud, server or other remote (read-only).
pub(super) fn target_of(row: &Row) -> Option<Target> {
    let path = match row {
        Row::Place(p) => &p.path,
        Row::Folder(f) => &f.path,
        _ => return None,
    };
    if is_recycle_bin(path) {
        return Some(Target::Bin);
    }
    if is_recent(path) || is_gallery(path) || in_archive_path(path) || is_remote_path(path) {
        return None;
    }
    Some(Target::Folder(path.clone()))
}

/// The drag picture's words for dropping `dragged` on `row` named `name`, or `None` where the drop is
/// refused: the row takes nothing, or the files are already there (or are that folder or above it), so there
/// is nothing to do (`targetAt`). `name` is the row's name as the person reads it.
pub(super) fn under(row: &Row, name: &str, dragged: &[String]) -> Option<Under> {
    match target_of(row)? {
        Target::Folder(dest) => (!droppable(dragged, &dest).is_empty()).then(|| Under {
            name: name.to_string(),
            copy: t!("Copy to %1").to_string(),
            moves: t!("Move to %1").to_string(),
            always_move: false,
        }),
        Target::Bin => Some(Under {
            name: name.to_string(),
            copy: t!("Copy to %1").to_string(),
            moves: t!("Move to %1").to_string(),
            always_move: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::super::rows::{Folder, Glyph, Label, Place};
    use super::*;
    use glint_model::paths::{GALLERY, RECENT, RECYCLE_BIN, REMOTE};
    use glint_model::store::RemoteKind;

    fn place(glyph: Glyph, path: &str, remote: Option<&str>) -> Row {
        Row::Place(Place {
            glyph,
            label: Label::Plain("x".into()),
            path: path.into(),
            active: false,
            detail: Default::default(),
            tree: None,
            capacity: None,
            eject: None,
            remote: remote.map(str::to_string),
            gutter: false,
        })
    }

    fn folder(path: &str) -> Row {
        Row::Folder(Folder {
            path: path.into(),
            name: "x".into(),
            has: None,
            depth: 0,
            parent: String::new(),
            open: false,
            loading: false,
            active: false,
        })
    }

    #[test]
    fn folders_drives_and_the_bin_take_files_and_views_do_not() {
        assert_eq!(
            target_of(&place(Glyph::Folder, "D:\\Work", None)),
            Some(Target::Folder("D:\\Work".into()))
        );
        assert_eq!(
            target_of(&folder("C:\\Users\\me\\Documents")),
            Some(Target::Folder("C:\\Users\\me\\Documents".into()))
        );
        assert_eq!(
            target_of(&place(Glyph::RecycleBin, RECYCLE_BIN, None)),
            Some(Target::Bin)
        );
        assert_eq!(target_of(&place(Glyph::Recent, RECENT, None)), None);
        assert_eq!(target_of(&place(Glyph::Gallery, GALLERY, None)), None);
        assert_eq!(target_of(&Row::AddConnection), None);
    }

    #[test]
    fn remotes_are_read_only_but_a_paired_pc_is_sent_to() {
        let path = format!("{REMOTE}nas");
        assert_eq!(
            target_of(&place(
                Glyph::Remote(RemoteKind::Sftp, String::new()),
                &path,
                Some("nas")
            )),
            None
        );
        assert_eq!(target_of(&folder(&format!("{path}\\docs"))), None);
    }

    #[test]
    fn files_already_in_the_folder_are_refused_and_the_bin_always_moves() {
        let dragged = vec!["D:\\Work\\a.txt".to_string()];
        assert_eq!(under(&folder("D:\\Work"), "Work", &dragged), None, "already there");
        let into = under(&folder("D:\\Other"), "Other", &dragged);
        assert!(matches!(&into, Some(u) if u.name == "Other" && !u.always_move));
        let bin = under(&place(Glyph::RecycleBin, RECYCLE_BIN, None), "Recycle Bin", &dragged);
        assert!(matches!(&bin, Some(u) if u.always_move));
    }
}
