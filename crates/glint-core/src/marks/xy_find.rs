//! Finding XYplorer's `tag.dat` (PLAN.md 7.1, F27), reading only: `%APPDATA%\XYplorer`, a portable copy's `Data`
//! folder, or where its `Startup.ini` says (`[Appdata] Path=…`).

/// A `tag.dat` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    /// How many items it marks.
    pub items: u64,
    /// When it last changed, Unix ms.
    pub changed: i64,
}

/// Where XYplorer's `tag.dat` is, if it has one.
pub fn xyplorer_found() -> Option<Found> {
    // S0: card F1 fills this in.
    None
}
