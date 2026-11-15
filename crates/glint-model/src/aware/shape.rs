//! What a selection is, in one word, and what is worth finding out about one item.
//!
//! Card A1 of docs/gpui/aware fills this in: the signatures are the contract, the bodies below do nothing yet.

use super::probed::Want;
use crate::entries::FileEntry;

/// What a menu's targets (or the selection) are: what the suggestions and the habits are kept under.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shape {
    /// No targets: the folder's own menu.
    #[default]
    Nothing,
    Folder,
    Folders,
    Archive,
    Archives,
    DiskImage,
    Program,
    Installer,
    Script,
    Shortcut,
    Picture,
    Pictures,
    Video,
    Videos,
    Song,
    Songs,
    Pdf,
    Pdfs,
    Document,
    Documents,
    Data,
    Code,
    Font,
    /// One file of no kind above.
    File,
    /// Several files of different kinds.
    Files,
    /// Files and folders together.
    Mixed,
}

impl Shape {
    /// What `targets` are.
    pub fn of(targets: &[FileEntry]) -> Shape {
        let _ = targets;
        Shape::Nothing
    }

    /// A stable word for it ("picture", "pictures"): what habits are saved under. Never changes once shipped.
    pub fn key(self) -> &'static str {
        "nothing"
    }

    /// Whether it is several things.
    pub fn is_many(self) -> bool {
        false
    }
}

/// The one extension every target has, lower case; None when they differ, there is none, or a folder is among them.
pub fn shared_ext(targets: &[FileEntry]) -> Option<String> {
    let _ = targets;
    None
}

/// What is worth asking about `entry`: nothing for what isn't a plain file or folder on one of this PC's drives.
pub fn want_for(entry: &FileEntry) -> Want {
    let _ = entry;
    Want::default()
}
