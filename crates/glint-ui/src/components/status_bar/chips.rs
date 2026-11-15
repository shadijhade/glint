//! What the dock's end notices about the folder shown: it is changing now, or its drive is nearly full
//! (docs/gpui/aware/PLAN.md, F11).
//!
//! Only what is known is said (R1): no pulse, no "Changing now"; no drive found for the folder, no word about
//! space. The same numbers as the suggestion "C: is nearly full" (`suggestions.rs`) decide what is nearly full.

use crate::explorer::{Drive, DriveKind};

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
    if folder.is_empty() {
        return None;
    }
    if changing {
        if let Some(pulse) = pulse {
            return Some(Insight::Changing(pulse));
        }
    }
    let lower = folder.to_lowercase();
    let drive = drives.iter().find(|drive| {
        let root = drive.path.to_lowercase();
        !root.is_empty() && lower.starts_with(&root)
    })?;
    (drive.kind == DriveKind::Fixed && nearly_full(drive))
        .then(|| Insight::NearlyFull(drive.path.trim_end_matches(['\\', '/']).to_string()))
}

/// A drive is nearly full: under a tenth and under 15 GB free (a big drive with 100 GB left is fine).
fn nearly_full(drive: &Drive) -> bool {
    drive.total > 0 && (drive.free as f64) < drive.total as f64 * 0.10 && drive.free < 15 << 30
}
