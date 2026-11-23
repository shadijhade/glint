//! Bringing marks in (PLAN.md 7.3, F25, F28–F31): one pipeline for XYplorer's `tag.dat` / `XYplorerTag.dat` and
//! Glint's own export. Read the file, find each item (by the storage mode, then where the index's history says
//! it went), plan what each gets, then apply in batches with the befores in a journal and one Undo step.
//! XYplorer's files are only read (M7).

use crate::host::ipc::Channel;
use glint_model::marks::{Marks, Meet, XyExtra};

/// What the person chose in the preview.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportOptions {
    /// Fill what is empty (the default), or the source's marks replace Glint's.
    pub meet: Meet,
    /// XYplorer's extra columns come in as tags and comment lines (7.4).
    pub extras: bool,
    /// The Glint label each of the source's labels becomes, by the source's name; one not here goes by name,
    /// else becomes a new label (32 at most, then a tag).
    pub labels: Vec<(String, String)>,
    /// XYplorer's own folder, for storage modes 1 and 2 when Glint couldn't tell (asked once in the preview).
    pub base: Option<String>,
}

/// Where a planned item is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum At {
    /// Where the source says.
    Here,
    /// Not there, but the index's history says where it went (`now`).
    Moved { now: String },
    /// On a drive that isn't connected: it waits in the ledger.
    Away,
    /// Not found: it waits, or is left out.
    Missing,
}

/// One item of the plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// As the source has it, made whole.
    pub path: String,
    pub at: At,
    /// What it gets from the source.
    pub gets: Marks,
    /// What it has in Glint already, when anything.
    pub has: Option<Marks>,
}

/// One of the source's labels and what it becomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelPlan {
    pub name: String,
    /// `0xRRGGBB`.
    pub color: u32,
    /// The Glint label it becomes.
    pub to: String,
    /// `to` is a label Glint doesn't have yet.
    pub new: bool,
}

/// What an import would do, for the preview (F28); applied as it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub source: String,
    /// The source's format version (0 for a Glint export).
    pub version: u32,
    /// When the source last changed, Unix ms.
    pub changed: i64,
    pub options: ImportOptions,
    pub labels: Vec<LabelPlan>,
    /// XYplorer's extra columns.
    pub extras: Vec<XyExtra>,
    pub items: Vec<Planned>,
}

/// How far an import is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// What an import did (F29).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub marked: u64,
    /// In the ledger, waiting for their drive or their file.
    pub waiting: u64,
    pub missing: u64,
    /// Items whose own, different comment was kept (fill what is empty).
    pub kept_comments: u64,
    /// Every item's marks before, for the one Undo step.
    pub befores: Vec<(String, Marks)>,
    /// Every item with what happened to it, worded, for "What happened".
    pub items: Vec<(String, String)>,
    /// The journal of befores in Glint's data folder (`imports\<time>.jsonl`), when one was written.
    pub journal: Option<String>,
}

/// Reads `source` and plans the import, finding each item (reading only).
pub async fn import_plan(source: String, options: ImportOptions) -> Result<ImportPlan, String> {
    // S0: card F1 fills this in.
    let _ = (source, options);
    Err("Importing marks is not in this build yet".into())
}

/// Applies `plan` in batches on background threads, saying how far it is.
pub async fn import_apply(plan: ImportPlan, progress: Channel<Progress>) -> Result<ImportReport, String> {
    // S0: card F1 fills this in.
    let _ = (plan, progress);
    Err("Importing marks is not in this build yet".into())
}
