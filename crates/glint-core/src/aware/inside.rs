//! What is in a folder, all the way down.
//!
//! Card B2 of docs/gpui/aware: the number of files, the folders and the bytes below a folder, and how much of it
//! is "junk" (what a build leaves, that can be made again). The instant search index answers at once when it
//! covers the folder; otherwise a walk on every core of the lane counts it, reporting as it grows.

use super::{pool, Inside, Lane};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Folders a build leaves that can be made again (see [`is_junk`] for the ones that need a neighbour).
pub const JUNK: &[&str] = &[
    "target",
    "node_modules",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".angular",
    ".gradle",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".venv",
    "venv",
    "obj",
];

/// How often a walk reports what it has counted so far.
const REPORT_EVERY: Duration = Duration::from_millis(150);

/// A walk that has run this long gives up as if stopped: a folder this big is not "free" to count.
const LONGEST: Duration = Duration::from_secs(3);

/// A service from before counts files by a search; more than this and the count isn't said (R1: not guessed).
const MOST_FILES_FROM_INDEX: usize = 20_000;

/// What is in the folder at `path`, all the way down. From the instant search index when it covers the folder
/// (one report, `done`); else a walk on the lane's threads that reports about every 150 ms (`done: false`) and
/// once at the end. `stop` makes it give up without a last report.
pub fn inside(path: &str, lane: Lane, stop: &AtomicBool, on: &mut dyn FnMut(Inside)) {
    if stop.load(Ordering::Relaxed) {
        return;
    }
    if let Some(answer) = from_index(path) {
        if !stop.load(Ordering::Relaxed) {
            on(answer);
        }
        return;
    }
    walk(path, lane, stop, on);
}

/// Whether `name`, a folder right inside `folder`, is one a build leaves behind. Some names are only junk with
/// the project file that makes them so beside them: a `target` with no `Cargo.toml` may be anyone's.
fn is_junk(folder: &Path, name: &str) -> bool {
    if !JUNK.iter().any(|j| name.eq_ignore_ascii_case(j)) {
        return false;
    }
    if name.eq_ignore_ascii_case("target") {
        folder.join("Cargo.toml").is_file()
    } else if name.eq_ignore_ascii_case("node_modules") {
        folder.join("package.json").is_file()
    } else if name.eq_ignore_ascii_case("obj") {
        std::fs::read_dir(folder).is_ok_and(|read| {
            read.flatten().any(|entry| {
                let n = entry.file_name();
                let s = n.to_string_lossy();
                let b = s.as_bytes();
                (b.len() >= 7 && b[b.len() - 7..].eq_ignore_ascii_case(b".csproj"))
                    || (b.len() >= 8 && b[b.len() - 8..].eq_ignore_ascii_case(b".vcxproj"))
            })
        })
    } else if name.eq_ignore_ascii_case(".venv") || name.eq_ignore_ascii_case("venv") {
        folder.join(name).join("pyvenv.cfg").is_file()
    } else {
        true
    }
}

/// The index's answer for `path`, when it covers it: the sizes, with the files and folders below counted in
/// the service (one pass over its names, nothing sent but the counts).
#[cfg(windows)]
fn from_index(path: &str) -> Option<Inside> {
    let sizes = shadui_index::client::sizes_counted(path).ok()?;
    let junk = sizes.sizes.iter().filter(|(name, _)| is_junk(Path::new(path), name)).map(|(_, bytes)| *bytes).sum();
    let (files, folders) = match (sizes.files, sizes.folders) {
        (Some(files), Some(folders)) => (files, folders),
        // A service from before counts nothing: its files by a search, as before it updates itself.
        _ => (files_by_search(path)?, 0),
    };
    Some(Inside { files, folders, bytes: sizes.total, junk, done: true, from_index: true })
}

/// The files below `path` by a search of the index, for a service from before `Sizes` counted them: None when
/// the index doesn't cover the folder, 0 when there are more than `MOST_FILES_FROM_INDEX`.
#[cfg(windows)]
fn files_by_search(path: &str) -> Option<u64> {
    use shadui_index::client::{self, Event};
    use shadui_index::proto::SearchRequest;
    let request = SearchRequest {
        query: "file:".into(),
        roots: vec![path.into()],
        hidden: true,
        exclude: Vec::new(),
        max_results: Some(MOST_FILES_FROM_INDEX),
        fuzzy: false,
        rank: false,
        marks: false,
        count_only: false,
    };
    let mut covered = false;
    let mut files = None;
    let asked = client::search(&request, |event| {
        match event {
            Event::Plan { indexed, skipped } => {
                covered = !indexed.is_empty() && skipped.is_empty();
                return covered;
            }
            Event::Done { count, limited, .. } => files = Some(if limited { 0 } else { count }),
            Event::Hits(_) => {}
        }
        true
    });
    if asked.is_err() || !covered {
        return None;
    }
    files
}

#[cfg(not(windows))]
fn from_index(_path: &str) -> Option<Inside> {
    None
}

/// The counts of a walk, shared by its tasks. Each only ever grows, so a report is never lower than the one
/// before.
struct Walk<'a> {
    files: AtomicU64,
    folders: AtomicU64,
    bytes: AtomicU64,
    junk: AtomicU64,
    stop: &'a AtomicBool,
    deadline: Instant,
    timed_out: AtomicBool,
}

impl<'a> Walk<'a> {
    fn new(stop: &'a AtomicBool) -> Walk<'a> {
        Walk {
            files: AtomicU64::new(0),
            folders: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            junk: AtomicU64::new(0),
            stop,
            deadline: Instant::now() + LONGEST,
            timed_out: AtomicBool::new(false),
        }
    }

    /// Stopped by the caller, or run out of time.
    fn halted(&self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return true;
        }
        if Instant::now() >= self.deadline {
            self.timed_out.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }

    fn report(&self, done: bool) -> Inside {
        Inside {
            files: self.files.load(Ordering::Relaxed),
            folders: self.folders.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            junk: self.junk.load(Ordering::Relaxed),
            done,
            from_index: false,
        }
    }
}

/// Signals the waiter condvar when dropped, so the caller wakes immediately (even if a walk panics).
struct NotifyOnDrop<'a>(&'a (std::sync::Mutex<bool>, std::sync::Condvar));

impl Drop for NotifyOnDrop<'_> {
    fn drop(&mut self) {
        let mut done = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        *done = true;
        self.0.1.notify_one();
    }
}

/// Links, junctions and cloud placeholders are never followed: junctions loop, and a placeholder read would
/// download it.
fn is_link(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Counts what is in `dir`, one task for each folder below it. `in_junk`: `dir` is inside a junk folder;
/// `top`: `dir` is the folder asked about, whose own subfolders are the ones that can be junk.
fn visit<'s, 'a: 's>(scope: &rayon::Scope<'s>, dir: std::path::PathBuf, walk: &'s Walk<'a>, in_junk: bool, top: bool) {
    if walk.halted() {
        return;
    }
    let Ok(read) = std::fs::read_dir(&dir) else { return };
    let (mut files, mut folders, mut bytes, mut junk) = (0u64, 0u64, 0u64, 0u64);
    for entry in read.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if is_link(&meta) {
            continue;
        }
        if meta.is_dir() {
            folders += 1;
            let child_in_junk = in_junk || (top && is_junk(&dir, &entry.file_name().to_string_lossy()));
            let child = entry.path();
            scope.spawn(move |scope| visit(scope, child, walk, child_in_junk, false));
        } else {
            let size = meta.len();
            files += 1;
            bytes += size;
            if in_junk {
                junk += size;
            }
        }
    }
    if folders != 0 {
        walk.folders.fetch_add(folders, Ordering::Relaxed);
    }
    if files != 0 {
        walk.files.fetch_add(files, Ordering::Relaxed);
    }
    if bytes != 0 {
        walk.bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    if junk != 0 {
        walk.junk.fetch_add(junk, Ordering::Relaxed);
    }
}

/// Counts the folder on the lane's threads. `on` is called from this thread only.
fn walk(path: &str, lane: Lane, stop: &AtomicBool, on: &mut dyn FnMut(Inside)) {
    let root = Path::new(path);
    if stop.load(Ordering::Relaxed) || !root.is_dir() {
        return;
    }
    let counts = Walk::new(stop);
    let done = (std::sync::Mutex::new(false), std::sync::Condvar::new());
    std::thread::scope(|threads| {
        threads.spawn(|| {
            let _notify = NotifyOnDrop(&done);
            pool::install(lane, || rayon::scope(|scope| visit(scope, root.to_path_buf(), &counts, false, true)));
        });
        let mut last = Instant::now();
        let mut guard = done.0.lock().unwrap_or_else(|e| e.into_inner());
        while !*guard {
            let wait = REPORT_EVERY.saturating_sub(last.elapsed()).min(Duration::from_millis(50));
            let (next, _) = done.1.wait_timeout(guard, wait).unwrap_or_else(|e| e.into_inner());
            guard = next;
            if *guard {
                break;
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if last.elapsed() >= REPORT_EVERY {
                drop(guard);
                on(counts.report(false));
                last = Instant::now();
                guard = done.0.lock().unwrap_or_else(|e| e.into_inner());
            }
        }
    });
    if stop.load(Ordering::Relaxed) {
        return;
    }
    // A walk that ran out of time has said what it counted, and says nothing more.
    on(counts.report(!counts.timed_out.load(Ordering::Relaxed)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// A folder of its own under the temp folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("glint-aware-inside-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::create_dir_all(&dir);
            Scratch(dir)
        }

        fn put(&self, relative: &str, bytes: usize) {
            let file = self.0.join(relative);
            if let Some(parent) = file.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(file, vec![b'x'; bytes]);
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 3 files in the root (one is the Cargo.toml), 2 in `sub`, 1 in `target`: 6 files, 2 folders, 1,000 bytes.
    fn small_tree(name: &str) -> Scratch {
        let tree = Scratch::new(name);
        tree.put("Cargo.toml", 100);
        tree.put("a.txt", 50);
        tree.put("b.txt", 150);
        tree.put("sub/c.txt", 200);
        tree.put("sub/d.txt", 300);
        tree.put("target/e.bin", 200);
        tree
    }

    fn walked(path: &Path, stop: &AtomicBool) -> Vec<Inside> {
        let mut reports = Vec::new();
        walk(&path.to_string_lossy(), Lane::Now, stop, &mut |inside| reports.push(inside));
        reports
    }

    #[test]
    fn junk_needs_its_project_file_where_it_has_one() {
        let tree = Scratch::new("junk");
        assert!(!is_junk(&tree.0, "target"));
        tree.put("Cargo.toml", 1);
        assert!(is_junk(&tree.0, "target"));
        assert!(is_junk(&tree.0, "Target"));
        assert!(!is_junk(&tree.0, "node_modules"));
        tree.put("package.json", 1);
        assert!(is_junk(&tree.0, "node_modules"));
        assert!(is_junk(&tree.0, "__pycache__"));
        assert!(!is_junk(&tree.0, "obj"));
        tree.put("App.csproj", 1);
        assert!(is_junk(&tree.0, "obj"));
        assert!(!is_junk(&tree.0, ".venv"));
        tree.put(".venv/pyvenv.cfg", 1);
        assert!(is_junk(&tree.0, ".venv"));
        assert!(!is_junk(&tree.0, "src"));
    }

    #[test]
    fn a_small_tree_is_counted_with_its_junk() {
        let tree = small_tree("small");
        let reports = walked(&tree.0, &AtomicBool::new(false));
        let last = reports.last().copied();
        assert_eq!(
            last,
            Some(Inside { files: 6, folders: 2, bytes: 1000, junk: 200, done: true, from_index: false })
        );
    }

    #[test]
    fn only_the_last_report_is_done_and_counts_never_go_down() {
        let tree = small_tree("order");
        let reports = walked(&tree.0, &AtomicBool::new(false));
        assert!(!reports.is_empty());
        let done = reports.iter().filter(|r| r.done).count();
        assert_eq!(done, 1);
        assert!(reports.last().is_some_and(|r| r.done));
        for pair in reports.windows(2) {
            assert!(pair[1].files >= pair[0].files && pair[1].folders >= pair[0].folders);
            assert!(pair[1].bytes >= pair[0].bytes && pair[1].junk >= pair[0].junk);
        }
    }

    #[test]
    fn a_walk_that_was_stopped_never_says_it_is_done() {
        let tree = small_tree("stopped");
        let reports = walked(&tree.0, &AtomicBool::new(true));
        assert!(reports.iter().all(|r| !r.done));
    }

    #[test]
    fn a_file_or_a_missing_path_is_never_reported() {
        let tree = small_tree("notfolder");
        let stop = AtomicBool::new(false);
        assert!(walked(&tree.0.join("a.txt"), &stop).is_empty());
        assert!(walked(&tree.0.join("nothing-here"), &stop).is_empty());
    }

    #[test]
    fn a_junction_back_to_the_root_is_not_followed() {
        let tree = small_tree("junction");
        let link = tree.0.join("loop");
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&tree.0)
            .output()
            .is_ok_and(|out| out.status.success());
        if !made {
            return;
        }
        let reports = walked(&tree.0, &AtomicBool::new(false));
        let _ = fs::remove_dir(&link);
        assert_eq!(
            reports.last().copied(),
            Some(Inside { files: 6, folders: 2, bytes: 1000, junk: 200, done: true, from_index: false })
        );
    }
}
