//! Marks: a label, tags and a comment that stay with the file (docs/gpui/marks/PLAN.md). Pure: no disk, no
//! clock. glint-core reads and writes the three homes (a file's own streams, `.glint-marks` beside it, the
//! ledger on this PC); this is what they hold and what is done with it.
//!
//! - [`labels`]: the named colors a label is chosen from ([`Label`]), and what a row draws ([`shown`]).
//! - [`lines`]: the text of `.glint-marks`, the ledger and an export ([`read_beside`], [`write_beside`]…).
//! - [`wanted`]: the filter of `label:`, `tag:`, `comment:` and `has:` ([`Wanted`], [`KEYS`]).
//! - [`xyplorer`]: XYplorer's `tag.dat`, read and written ([`XyFile`]).
//!
//! The S0 contracts: the bodies that do nothing are filled by the wave's cards (PLAN.md section 9).

pub mod labels;
pub mod lines;
pub mod wanted;
pub mod xyplorer;

pub use labels::{default_labels, label_of, shown, Label, Shown};
pub use lines::{
    read_beside, read_export, read_ledger, write_beside, write_export, write_ledger, Beside, Export, Line, Why,
};
pub use wanted::{Has, Terms, Wanted, KEYS};
pub use xyplorer::{XyExtra, XyFile, XyItem, XyKind, XyLabel};

use serde::{Deserialize, Serialize};

/// The streams marks live in, after the file's name: `C:\notes.txt:glint.label`. `glint.tags` is 0.16's and keeps
/// its name and format (M8).
pub const STREAM_TAGS: &str = "glint.tags";
/// One line: the label's name.
pub const STREAM_LABEL: &str = "glint.label";
/// The comment, UTF-8, lines ended by `\n`.
pub const STREAM_COMMENT: &str = "glint.comment";
/// More is never read of a label's stream (5.1).
pub const LABEL_CAP: usize = 256;
/// Nor of the tags' (0.16's cap).
pub const TAGS_CAP: usize = 4096;
/// Nor of a comment's; the index keeps its first 4 KB.
pub const COMMENT_CAP: usize = 64 * 1024;
/// The hidden file beside the files, on a drive without streams (5.2).
pub const BESIDE_NAME: &str = ".glint-marks";
/// No more labels than this (F1); an import's label past it comes in as a tag.
pub const MAX_LABELS: usize = 32;

/// What an item is marked with. Empty everywhere is no marks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    /// The label's name, as the file says it (it may be an old name of one: `Label::was`).
    pub label: Option<String>,
    /// One of each, in the order they were given.
    pub tags: Vec<String>,
    pub comment: Option<String>,
}

impl Marks {
    /// No label, no tag, no comment.
    pub fn is_empty(&self) -> bool {
        self.label.is_none() && self.tags.is_empty() && self.comment.is_none()
    }
}

/// A change asked of some items' marks (core's `marks_set`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The label, or none.
    Label(Option<String>),
    /// Tags added and taken off (any case).
    Tags { add: Vec<String>, remove: Vec<String> },
    /// The comment, or none.
    Comment(Option<String>),
    /// All three at once: paste, undo, import, and nothing else.
    Put(Marks),
    /// No marks at all.
    Clear,
}

impl Change {
    /// What `marks` become with this change.
    pub fn apply(&self, marks: &Marks) -> Marks {
        // S0: card A1 fills this in.
        marks.clone()
    }
}

/// Where an item's marks are kept (5.4): its own streams, the folder's `.glint-marks`, or the ledger on this PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Home {
    Stream,
    Beside,
    Ledger,
}

/// What a change did to one item: its marks before, and after with where they are now, or why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub path: String,
    pub before: Marks,
    pub after: Result<(Marks, Home), String>,
}

/// How an import meets the marks an item already has in Glint (7.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Meet {
    /// The label if it has none, the tags added, the comment if it has none.
    #[default]
    Fill,
    /// The source's replace Glint's, all three.
    Replace,
}

/// What an item gets when `incoming` meets what it `has` (7.3), and whether a different comment of Glint's was
/// kept (the report says so).
pub fn meet(has: &Marks, incoming: &Marks, how: Meet) -> (Marks, bool) {
    // S0: card A1 fills this in.
    let _ = (incoming, how);
    (has.clone(), false)
}
