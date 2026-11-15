//! What the dock's end notices about the folder shown: it is changing now, or its drive is nearly full
//! (docs/gpui/aware/PLAN.md, F11).
//!
//! Card F2 of docs/gpui/aware fills this in: the signatures are the contract; until then nothing is noticed.

use crate::explorer::Drive;

/// Something worth saying about the folder shown.
#[derive(Clone, Debug, PartialEq)]
pub enum Insight {
    /// Files are being written or removed in the folder shown: its last minute.
    Changing([u8; 30]),
    /// The folder's drive is nearly full: its root, as "C:".
    NearlyFull(String),
}

/// What to say about `folder`, if anything (an empty `folder` is a place that has nothing to say).
pub fn insight(folder: &str, changing: bool, pulse: Option<[u8; 30]>, drives: &[Drive]) -> Option<Insight> {
    let _ = (folder, changing, pulse, drives);
    None
}
