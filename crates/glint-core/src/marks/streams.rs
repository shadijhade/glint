//! A file's own marks: its `glint.tags`, `glint.label` and `glint.comment` streams (PLAN.md 5.1), each plain
//! UTF-8 a script can read (`Get-Content f -Stream glint.comment`). Read with one listing of the file's streams,
//! then one open for each `glint.*` it has, so a file with none costs one call; written with the file's dates
//! kept (M3, `tags::keep_times`). `glint.tags` keeps 0.16's name and format (M8).

use glint_model::marks::Marks;

/// The marks in `path`'s streams; None when it has no `glint.*` stream, or they can't be read.
pub fn read(path: &str) -> Option<Marks> {
    // S0: card B1 fills this in.
    let _ = path;
    None
}

/// Writes `marks` into `path`'s streams, each alone (an empty one's stream goes), with the file's dates kept.
pub fn write(path: &str, marks: &Marks) -> Result<(), String> {
    // S0: card B1 fills this in.
    let _ = (path, marks);
    Err("Marks are not written yet".into())
}
