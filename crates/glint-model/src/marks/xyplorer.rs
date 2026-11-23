//! XYplorer's tag database, `tag.dat` (and `XYplorerTag.dat`, the same for one folder), read and written
//! (PLAN.md 7.1). UTF-16LE text in sections found by their names: `Labels:`, `Extra Tags:`, `Storage:`,
//! `Data:`. A data line is `Full filename|LabelID|Tags|Extra1…ExtraN|Comment`, the comment last and the only
//! field that may hold `|`, its line breaks written `¶`. Version 5 has sixteen extra fields, version 3 five.
//!
//! Only read from XYplorer's own files (M7); what is written is an export the person takes there.

use super::{Label, Marks};

/// More is never read of a `tag.dat`.
pub const XY_MAX_BYTES: usize = 64 << 20;

/// One of XYplorer's labels: `Red|000000|FC7268` is its name, its text's color and its color.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XyLabel {
    pub name: String,
    /// `0xRRGGBB`.
    pub text: u32,
    /// `0xRRGGBB`: the label's color.
    pub back: u32,
}

/// What an extra field holds (its TypeID).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XyKind {
    Text,
    Number,
    Date,
    /// Empty is off, anything else on.
    Checkbox,
    /// 1 to 5.
    Rating,
    List,
    Location,
    Icon,
    Image,
    Script,
    /// A type this reader doesn't know, by its number.
    Other(u32),
}

/// One extra field's definition (a line of `Extra Tags:`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XyExtra {
    pub caption: String,
    pub kind: XyKind,
}

/// One data line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XyItem {
    /// As written: what it means depends on the file's storage mode.
    pub path: String,
    /// 0 for none, else the label's place in the list, from 1.
    pub label: u32,
    pub tags: Vec<String>,
    /// The extra fields' values, one per definition.
    pub extras: Vec<String>,
    /// With `¶` as written.
    pub comment: String,
}

/// A `tag.dat` as read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct XyFile {
    /// From `XYplorer File Tags v5`.
    pub version: u32,
    pub labels: Vec<XyLabel>,
    pub extras: Vec<XyExtra>,
    /// How paths are written (`Storage: 0`): 0 absolute, 1 `?:` for XYplorer's drive, 2 relative to XYplorer's
    /// folder, 3 relative to the file's folder, 4 a volume's serial for the drive.
    pub storage: u32,
    pub items: Vec<XyItem>,
}

/// A `tag.dat` from its bytes: with or without a byte-order mark, `\r\n` or `\n`. Err when it isn't one.
pub fn read(bytes: &[u8]) -> Result<XyFile, String> {
    // S0: card A3 fills this in.
    let _ = bytes;
    Err("Not an XYplorer tag database".into())
}

/// An `XYplorerTag.dat` (version 5, storage 3: paths relative to its folder) that XYplorer's `tagload` merges.
/// `labels` are Glint's, written as XYplorer's list; an item's `label` is its place in them, from 1.
pub fn write(items: &[XyItem], labels: &[Label]) -> Vec<u8> {
    // S0: card A3 fills this in.
    let _ = (items, labels);
    Vec::new()
}

/// What one item brings (7.3, 7.4): its label by XYplorer's name, its tags, its comment with line breaks, and,
/// when `extras`, its extra fields as tags and comment lines.
pub fn marks_of(file: &XyFile, item: &XyItem, extras: bool) -> Marks {
    // S0: card A3 fills this in.
    let _ = (file, item, extras);
    Marks::default()
}
