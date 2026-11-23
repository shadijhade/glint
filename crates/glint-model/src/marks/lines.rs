//! The text of the homes that are not a file's own streams, and of an export: JSON Lines, UTF-8, `\n`.
//!
//! - `.glint-marks` beside the files (5.2): `{"glint-marks":1}`, then one line per item by `"name"`, sorted.
//! - The ledger on this PC (5.3): the same lines by `"path"`, with `"why"`.
//! - An export (F24): the labels in the first line, then the lines by path under the folder exported.
//!
//! What comes from outside is data (M10): every reader is capped, refuses a name that leaves its folder, and
//! never panics.

use super::{Label, Marks};

/// More is never read of a `.glint-marks`.
pub const BESIDE_MAX_BYTES: usize = 4 << 20;
/// Nor more lines than this.
pub const BESIDE_MAX_LINES: usize = 100_000;

/// One item's line: whose marks, and what they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    /// Beside: the item's name in its folder. The ledger: its full path. An export: its path under the folder.
    pub name: String,
    pub marks: Marks,
    /// When they were set, in Unix seconds (`"at"`; 0 when the line doesn't say).
    pub at: i64,
    /// The ledger's: why the marks are kept on this PC.
    pub why: Option<Why>,
    /// Keys this version doesn't know (a later version's), written back as they were.
    pub rest: serde_json::Map<String, serde_json::Value>,
}

/// Why the ledger keeps an item's marks (5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// An import waiting for an item that was not there (a drive not connected, a file not found).
    Away,
    /// Neither the item's streams nor its folder can be written (a read-only share, an archive, a remote).
    Shut,
}

/// A `.glint-marks` as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Beside {
    /// The format's version, from the first line (1 today).
    pub version: u32,
    /// By name, as the file has them.
    pub lines: Vec<Line>,
    /// Lines left out: not JSON, cut off, past the caps, or a name with `\`, `/` or `..`.
    pub refused: usize,
}

/// A Glint export as read or to be written (F24).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Export {
    /// The labels of the PC it came from, in their order (the first line).
    pub labels: Vec<Label>,
    /// By path under the folder exported, with `\` between folders.
    pub lines: Vec<Line>,
}

/// A `.glint-marks`'s text, read as far as it can be.
pub fn read_beside(text: &str) -> Beside {
    // S0: card A2 fills this in.
    let _ = text;
    Beside::default()
}

/// The text of a `.glint-marks`: the first line, then the lines sorted by name.
pub fn write_beside(b: &Beside) -> String {
    // S0: card A2 fills this in.
    let _ = b;
    String::new()
}

/// The ledger's text, read as far as it can be.
pub fn read_ledger(text: &str) -> Vec<Line> {
    // S0: card A2 fills this in.
    let _ = text;
    Vec::new()
}

/// The ledger's text: one line per item, by path.
pub fn write_ledger(lines: &[Line]) -> String {
    // S0: card A2 fills this in.
    let _ = lines;
    String::new()
}

/// A Glint export's text; Err when it isn't one.
pub fn read_export(text: &str) -> Result<Export, String> {
    // S0: card A2 fills this in.
    let _ = text;
    Err("Not a Glint export".into())
}

/// The text of a Glint export.
pub fn write_export(export: &Export) -> String {
    // S0: card A2 fills this in.
    let _ = export;
    String::new()
}
