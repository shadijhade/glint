//! What is known at once about what a menu or the dock is for: read off the entries and the app's state, with no
//! disk and no waiting. What takes finding out is in [`super::probed`].

use super::shape::Shape;

/// What is known at once about the targets of a menu, or about the selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    /// What they are, in one word.
    pub shape: Shape,
    /// How many there are.
    pub count: usize,
    /// How many of them are files.
    pub files: usize,
    /// How many of them are folders.
    pub folders: usize,
    /// The files' bytes; folders weigh nothing here (what is in a folder takes finding out: `Probed::inside`).
    pub bytes: u64,
    /// The one extension all the files share, lower case.
    pub ext: Option<String>,
    /// The first target's name, lower case ("" for none).
    pub first_name: String,
    /// Nothing can change: in an archive, on a remote, in a folder Windows keeps closed, in the Recycle Bin.
    pub fixed: bool,
    /// What is on the clipboard.
    pub clip: Clip,
}

/// What is on the clipboard, as far as a file explorer cares.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Clip {
    /// Nothing a paste could use.
    #[default]
    Empty,
    /// Files and folders, copied or cut.
    Files {
        /// How many.
        count: usize,
        /// Cut rather than copied: a paste moves them.
        cut: bool,
        /// The folder they are all in, when they share one.
        from: Option<String>,
        /// The first five names.
        names: Vec<String>,
    },
    /// A picture (a paste makes a file of it).
    Picture,
    /// Text (a paste makes a file of it).
    Text,
}
