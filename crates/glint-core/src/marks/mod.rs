//! Marks on files and folders (docs/gpui/marks/PLAN.md): a label, tags and a comment, kept in the first of
//! three homes that works (5.4): the file's own streams ([`streams`]), `.glint-marks` beside it ([`beside`]),
//! or the ledger on this PC ([`ledger`]). What they hold and the filter are glint-model's ([`glint_model::marks`]).
//!
//! - Reading and writing: [`marks_read`], [`marks_set`], [`marks_put`], [`read`]; fd's drives: [`matches`].
//! - Following an item Glint copies or moves: [`carry`].
//! - Every label and tag in use: [`marks_all`].
//! - Out and in: [`marks_export`], [`import_plan`], [`import_apply`], [`xyplorer_found`].
//!
//! The S0 contracts: the bodies that do nothing are filled by the wave's cards (PLAN.md section 9).

pub mod beside;
pub mod caps;
pub mod carry;
pub mod export;
pub mod import;
pub mod ledger;
pub mod streams;
pub mod xy_find;

pub use carry::carry;
pub use export::{marks_export, Format};
pub use glint_model::marks::{Change, Home, Marks, Outcome, Wanted};
pub use import::{
    import_apply, import_plan, At, ImportOptions, ImportPlan, ImportReport, LabelPlan, Planned, Progress,
};
pub use xy_find::{xyplorer_found, Found};

/// Every label and tag in use with how many items carry it, most used first, and how many items have a
/// comment: the index's for the drives it covers, with the ledger's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub labels: Vec<(String, u64)>,
    pub tags: Vec<(String, u64)>,
    pub comments: u64,
}

/// Each path's marks, for the rows on screen (one per path, in order).
pub async fn marks_read(paths: Vec<String>) -> Result<Vec<Marks>, String> {
    // S0: card B2 fills this in.
    let _ = paths;
    Ok(Vec::new())
}

/// Changes each path's marks, in its home: what each had before, and after with where it is now, or why not.
pub async fn marks_set(paths: Vec<String>, change: Change) -> Result<Vec<Outcome>, String> {
    // S0: card B2 fills this in.
    let _ = (paths, change);
    Ok(Vec::new())
}

/// Gives each path exactly these marks (undo, paste, an import's batch).
pub async fn marks_put(items: Vec<(String, Marks)>) -> Result<Vec<Outcome>, String> {
    // S0: card B2 fills this in.
    let _ = items;
    Ok(Vec::new())
}

/// An item's marks: its streams', else its line beside it, else the ledger's. None known is no marks.
pub fn read(path: &str) -> Marks {
    // S0: card B2 fills this in.
    let _ = path;
    Marks::default()
}

/// Whether the item at `path` is what `wanted` asks for (fd's drives, where there is no index).
pub fn matches(path: &str, wanted: &Wanted) -> bool {
    // S0: card B2 fills this in.
    wanted.accepts(&read(path))
}

/// Every label and tag in use, and how many items have a comment.
pub async fn marks_all() -> Result<Counts, String> {
    // S0: card C2 (the index's part) and B2 (the ledger's) fill this in.
    Ok(Counts::default())
}
