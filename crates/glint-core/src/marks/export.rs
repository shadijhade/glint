//! Export marks… (PLAN.md F24, F33): everything marked below a folder into one file, Glint's JSON Lines or an
//! `XYplorerTag.dat` that XYplorer's `tagload` merges.

/// The format an export is written in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// JSON Lines: the labels first, then a line per item, by path under the folder.
    #[default]
    Glint,
    /// An `XYplorerTag.dat`: version 5, storage 3.
    Xyplorer,
}

/// Writes the marks of everything below `root` to `to`; how many items were written.
pub async fn marks_export(root: String, to: String, format: Format) -> Result<u64, String> {
    // S0: card F3 fills this in.
    let _ = (root, to, format);
    Err("Exporting marks is not in this build yet".into())
}
