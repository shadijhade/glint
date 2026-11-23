//! Other files on this PC with the same bytes as one file.
//!
//! Card B2 of docs/gpui/aware: the index finds the files of exactly the same size, BLAKE3 proves which of them
//! are the same bytes.

use rayon::prelude::*;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Below this a file says nothing (empty files and tiny config files are all alike).
const LEAST: u64 = 1024;
/// Above this the proof costs too much to be free.
const MOST: u64 = 256 * 1024 * 1024;
/// More files than this of one size: the size is too common to say anything about.
const MOST_CANDIDATES: usize = 64;

/// The files on this PC whose bytes are exactly those of the file at `path` (`size` bytes long): found by size
/// in the instant search index, proved by BLAKE3. `Some(empty)` when there is none; None when it can't be said
/// (no index, a size too common or out of range, stopped).
pub fn twins(path: &str, size: u64, stop: &AtomicBool) -> Option<Vec<String>> {
    if !(LEAST..=MOST).contains(&size) || stop.load(Ordering::Relaxed) {
        return None;
    }
    let candidates = same_size(path, size)?;
    if candidates.is_empty() {
        return Some(Vec::new());
    }
    let Hashed::Hash(mine) = hash_file(Path::new(path), size, stop) else {
        return None;
    };
    let theirs: Vec<Hashed> = candidates.par_iter().map(|c| hash_file(Path::new(c), size, stop)).collect();
    if theirs.iter().any(|h| matches!(h, Hashed::Stopped)) {
        return None;
    }
    Some(
        candidates
            .into_iter()
            .zip(theirs)
            .filter(|(_, hash)| matches!(hash, Hashed::Hash(h) if *h == mine))
            .map(|(candidate, _)| candidate)
            .collect(),
    )
}

/// How long the drives the index has ready are remembered: a look at one file asks the service once, not once
/// for every file looked at (a drive that starts scanning meanwhile is noticed within this long).
#[cfg(windows)]
const ROOTS_FRESH: std::time::Duration = std::time::Duration::from_secs(20);

/// The drive roots last read from the index service, and when.
#[cfg(windows)]
type KnownRoots = std::sync::Mutex<Option<(std::time::Instant, Vec<String>)>>;

/// The roots in `known` when they were read less than [`ROOTS_FRESH`] before `now`; otherwise what `read` finds,
/// remembered when it is something. `read` runs with nothing locked.
#[cfg(windows)]
fn roots_cached(
    known: &KnownRoots,
    now: std::time::Instant,
    read: impl FnOnce() -> Option<Vec<String>>,
) -> Option<Vec<String>> {
    {
        let held = known.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, roots)) = held.as_ref() {
            if now.saturating_duration_since(*at) < ROOTS_FRESH {
                return Some(roots.clone());
            }
        }
    }
    let roots = read()?;
    *known.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, roots.clone()));
    Some(roots)
}

/// The drives the index has finished scanning, as search roots; None when the service can't say, a drive is
/// still being scanned (a "no copy" would be a guess) or none is ready.
#[cfg(windows)]
fn ready_roots() -> Option<Vec<String>> {
    use shadui_index::client;
    let status = client::status().ok()?;
    if status.volumes.iter().any(|v| v.state == "scanning") {
        return None;
    }
    let roots: Vec<String> = status
        .volumes
        .iter()
        .filter(|v| v.state == "ready")
        .map(|v| format!("{}\\", v.volume.trim_end_matches(['\\', '/'])))
        .collect();
    (!roots.is_empty()).then_some(roots)
}

/// The index's files of exactly `size` bytes other than `path`, in its order; None when it can't say.
#[cfg(windows)]
fn same_size(path: &str, size: u64) -> Option<Vec<String>> {
    use shadui_index::client::{self, Event};
    use shadui_index::proto::SearchRequest;
    static KNOWN: KnownRoots = std::sync::Mutex::new(None);
    let roots = roots_cached(&KNOWN, std::time::Instant::now(), ready_roots)?;
    let request = SearchRequest {
        query: format!("file: size:{size}"),
        roots,
        hidden: true,
        exclude: Vec::new(),
        max_results: Some(MOST_CANDIDATES),
        fuzzy: false,
        rank: false,
        marks: false,
    };
    let mut found = Vec::new();
    let mut limited = false;
    let mut done = false;
    let asked = client::search(&request, |event| {
        match event {
            Event::Hits(hits) => {
                found.extend(hits.into_iter().filter(|hit| !hit.dir && !hit.path.eq_ignore_ascii_case(path)).map(|hit| hit.path))
            }
            Event::Done { limited: more, .. } => {
                limited = more;
                done = true;
            }
            Event::Plan { .. } => {}
        }
        true
    });
    if asked.is_err() || !done || limited {
        return None;
    }
    Some(found)
}

#[cfg(not(windows))]
fn same_size(_path: &str, _size: u64) -> Option<Vec<String>> {
    None
}

/// How reading a file for its hash ended.
enum Hashed {
    Hash(blake3::Hash),
    /// It can't be read, or is not `size` bytes long any more: not a twin.
    Unreadable,
    /// `stop` was set.
    Stopped,
}

/// BLAKE3 of the file, read in 1 MB pieces with `stop` looked at between them.
fn hash_file(path: &Path, size: u64, stop: &AtomicBool) -> Hashed {
    let Ok(mut file) = std::fs::File::open(path) else { return Hashed::Unreadable };
    if file.metadata().map_or(true, |meta| meta.len() != size) {
        return Hashed::Unreadable;
    }
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut total: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Hashed::Stopped;
        }
        let Ok(read) = file.read(&mut buffer) else { return Hashed::Unreadable };
        if read == 0 {
            break;
        }
        let Some(piece) = buffer.get(..read) else { return Hashed::Unreadable };
        hasher.update(piece);
        total = total.saturating_add(read as u64);
    }
    if total != size {
        return Hashed::Unreadable;
    }
    Hashed::Hash(hasher.finalize())
}

/// Whether two files have the same `size` bytes: None when stopped, `Some(false)` when either can't be read.
#[cfg(test)]
fn same_bytes(a: &Path, b: &Path, size: u64, stop: &AtomicBool) -> Option<bool> {
    let first = hash_file(a, size, stop);
    let second = hash_file(b, size, stop);
    match (first, second) {
        (Hashed::Stopped, _) | (_, Hashed::Stopped) => None,
        (Hashed::Hash(x), Hashed::Hash(y)) => Some(x == y),
        _ => Some(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-aware-twins-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn sizes_out_of_range_say_nothing_without_touching_the_disk() {
        let stop = AtomicBool::new(false);
        let nowhere = "Z:\\there-is-no-such-folder\\file.bin";
        assert_eq!(twins(nowhere, 0, &stop), None);
        assert_eq!(twins(nowhere, 1023, &stop), None);
        assert_eq!(twins(nowhere, 256 * 1024 * 1024 + 1, &stop), None);
    }

    #[test]
    fn two_files_of_the_same_bytes_are_the_same_and_one_changed_byte_is_not() {
        let dir = scratch("pairs");
        let bytes: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let mut changed = bytes.clone();
        if let Some(b) = changed.get_mut(4000) {
            *b ^= 1;
        }
        let (a, b, c) = (dir.join("a.bin"), dir.join("b.bin"), dir.join("c.bin"));
        let _ = fs::write(&a, &bytes);
        let _ = fs::write(&b, &bytes);
        let _ = fs::write(&c, &changed);
        let stop = AtomicBool::new(false);
        assert_eq!(same_bytes(&a, &b, 5000, &stop), Some(true));
        assert_eq!(same_bytes(&a, &c, 5000, &stop), Some(false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_or_a_changed_length_is_not_a_twin() {
        let dir = scratch("missing");
        let a = dir.join("a.bin");
        let _ = fs::write(&a, vec![7u8; 5000]);
        let stop = AtomicBool::new(false);
        assert_eq!(same_bytes(&a, &dir.join("gone.bin"), 5000, &stop), Some(false));
        assert_eq!(same_bytes(&a, &a, 4999, &stop), Some(false));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stopped_comparison_says_nothing() {
        let dir = scratch("stopped");
        let a = dir.join("a.bin");
        let _ = fs::write(&a, vec![7u8; 5000]);
        let stop = AtomicBool::new(true);
        assert_eq!(same_bytes(&a, &a, 5000, &stop), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn the_drives_are_asked_of_the_service_once_in_a_while_not_for_every_file() {
        use std::cell::Cell;
        use std::time::{Duration, Instant};
        let known: KnownRoots = std::sync::Mutex::new(None);
        let asked = Cell::new(0u32);
        let read = || {
            asked.set(asked.get() + 1);
            Some(vec!["C:\\".to_string()])
        };
        let start = Instant::now();
        assert_eq!(roots_cached(&known, start, read), Some(vec!["C:\\".to_string()]));
        for n in 1..50u64 {
            assert!(roots_cached(&known, start + Duration::from_millis(n * 10), read).is_some());
        }
        assert_eq!(asked.get(), 1, "fifty files, one question");
        assert!(roots_cached(&known, start + ROOTS_FRESH + Duration::from_secs(1), read).is_some());
        assert_eq!(asked.get(), 2, "asked again once the answer is old");
    }

    #[cfg(windows)]
    #[test]
    fn a_service_that_cannot_say_is_asked_again_next_time() {
        use std::cell::Cell;
        let known: KnownRoots = std::sync::Mutex::new(None);
        let asked = Cell::new(0u32);
        let read = || {
            asked.set(asked.get() + 1);
            None
        };
        let now = std::time::Instant::now();
        assert_eq!(roots_cached(&known, now, read), None);
        assert_eq!(roots_cached(&known, now, read), None);
        assert_eq!(asked.get(), 2, "a failure is not remembered");
    }
}
