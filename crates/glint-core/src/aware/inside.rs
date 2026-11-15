//! What is in a folder, all the way down.
//!
//! Card B2 of docs/gpui/aware fills this in: the signature is the contract; until then nothing is reported.

use super::{Inside, Lane};
use std::sync::atomic::AtomicBool;

/// What is in the folder at `path`, all the way down. From the instant search index when it covers the folder
/// (one report, `done`); else a walk on the lane's threads that reports about every 150 ms (`done: false`) and
/// once at the end. `stop` makes it give up without a last report.
pub fn inside(path: &str, lane: Lane, stop: &AtomicBool, on: &mut dyn FnMut(Inside)) {
    let _ = (path, lane, stop, on);
}
