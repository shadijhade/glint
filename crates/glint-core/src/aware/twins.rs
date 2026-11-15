//! Other files on this PC with the same bytes as one file.
//!
//! Card B2 of docs/gpui/aware fills this in: the signature is the contract; until then nothing is known.

use std::sync::atomic::AtomicBool;

/// The files on this PC whose bytes are exactly those of the file at `path` (`size` bytes long): found by size
/// in the instant search index, proved by BLAKE3. `Some(empty)` when there is none; None when it can't be said
/// (no index, a size too common or out of range, stopped).
pub fn twins(path: &str, size: u64, stop: &AtomicBool) -> Option<Vec<String>> {
    let _ = (path, size, stop);
    None
}
