//! Once after the update (5.6): the files instant search says carry a 0.16 color tag are settled, so the tag
//! becomes the label it is shown as, on the quiet lane, a batch at a time. Never twice: a marker in Glint's data
//! folder says how far each drive got, so a stop (Glint closing) is taken up where it was. Where the service is
//! not there or is older than marks, nothing happens and the next start tries again. A drive the index doesn't
//! cover is settled file by file the next time a file's marks are written (`work`).

use super::{Change, Label, Outcome};
use glint_model::marks::default_labels;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Files a batch settles: one read and one write each.
const BATCH: usize = 200;
/// What the marker's format is; another number is a marker this code doesn't know, and starts again.
const VERSION: u32 = 1;
/// How long after start-up it waits, so starting is not slowed.
const DELAY: std::time::Duration = std::time::Duration::from_secs(30);

static STOP: AtomicBool = AtomicBool::new(false);

/// Starts it on a thread of its own, after the delay, on the quiet lane. Called once at start. Never in a debug
/// build: a copy run from the repository (a look at the app, the end-to-end checks) has a profile of its own but
/// this PC's drives and index, and a version the installed service shares until the release, so it would rewrite
/// the person's own files with its scratch profile's labels.
pub fn start() {
    if cfg!(debug_assertions) {
        return;
    }
    let _ = std::thread::Builder::new().name("marks-settle".into()).spawn(|| {
        let mut waited = std::time::Duration::ZERO;
        while waited < DELAY && !STOP.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(500));
            waited += std::time::Duration::from_millis(500);
        }
        crate::aware::pool::install(crate::aware::Lane::Quiet, || run(&|| STOP.load(Ordering::Relaxed)));
    });
}

/// Glint is closing: it stops between two batches.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// What the marker remembers, drive by drive (the drive as the index names it, "C:").
#[derive(Default, Serialize, Deserialize)]
struct Marker {
    version: u32,
    drives: BTreeMap<String, Drive>,
}

#[derive(Default, Serialize, Deserialize)]
struct Drive {
    /// The last path done, in lower case: the next run goes on after it.
    last: String,
    done: bool,
}

fn load(file: &Path) -> Marker {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Marker>(&t).ok())
        .filter(|m| m.version == VERSION)
        .unwrap_or(Marker { version: VERSION, drives: BTreeMap::new() })
}

/// The marker is only a hint that saves work: a write that fails costs a second look at some files.
fn save(file: &Path, marker: &Marker) {
    if let Ok(text) = serde_json::to_string(marker) {
        let _ = std::fs::write(file, text);
    }
}

/// What the work asks of the world, so a test has no index service: the drives it covers, the files tagged with
/// one of the names on a drive, and the marks' writing.
type Listing<'a> = &'a dyn Fn(&str, &[String]) -> Option<Vec<String>>;

struct Seam<'a> {
    drives: &'a dyn Fn() -> Option<Vec<String>>,
    list: Listing<'a>,
    set: &'a dyn Fn(Vec<(String, Change)>) -> Vec<Outcome>,
}

/// Settles what the index says is tagged with a color, on every drive it covers. Synchronous: it is for a thread
/// of its own. `stop` is asked between batches.
pub fn run(stop: &dyn Fn() -> bool) {
    let Some(data) = crate::tools::data_dir() else { return };
    let seam = Seam { drives: &index_drives, list: &index_list, set: &super::set_many };
    run_with(&seam, &super::labels(), &data.join("marks-settled.json"), stop);
}

/// The names a 0.16 color tag may have: the default seven, and this PC's labels' names and old names.
fn names(labels: &[Label]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for name in default_labels().iter().chain(labels).flat_map(|l| std::iter::once(&l.name).chain(&l.was)) {
        // A name the filter couldn't carry whole would match something else.
        let plain = !name.is_empty() && !name.contains(|c: char| c.is_whitespace() || c == ';' || c == '"');
        if plain && !found.iter().any(|f| f.to_lowercase() == name.to_lowercase()) {
            found.push(name.clone());
        }
    }
    found
}

fn run_with(seam: &Seam, labels: &[Label], file: &Path, stop: &dyn Fn() -> bool) {
    let Some(drives) = (seam.drives)() else { return };
    let names = names(labels);
    let mut marker = load(file);
    for drive in drives {
        if marker.drives.get(&drive).is_some_and(|d| d.done) {
            continue;
        }
        let Some(mut paths) = (seam.list)(&drive, &names) else { continue };
        paths.sort_by_cached_key(|p| p.to_lowercase());
        let last = marker.drives.get(&drive).map(|d| d.last.clone()).unwrap_or_default();
        paths.retain(|p| p.to_lowercase() > last);
        for batch in paths.chunks(BATCH) {
            if stop() {
                return;
            }
            // No change but the one every write makes: `work` settles what it writes and writes nothing already
            // settled (a file with a label keeps it, so most need nothing). Each file is read as it is written,
            // so a mark set meanwhile is never put back.
            let nothing = || Change::Tags { add: Vec::new(), remove: Vec::new() };
            (seam.set)(batch.iter().map(|p| (p.clone(), nothing())).collect());
            let done = marker.drives.entry(drive.clone()).or_default();
            done.last = batch.last().map(|p| p.to_lowercase()).unwrap_or_default();
            save(file, &marker);
        }
        marker.drives.entry(drive).or_default().done = true;
        save(file, &marker);
    }
}

/// The drives the service covers (ready), as it names them; None when it isn't there or isn't this version.
#[cfg(windows)]
fn index_drives() -> Option<Vec<String>> {
    if !crate::find::same_service() {
        return None;
    }
    let status = shadui_index::client::status().ok()?;
    Some(status.volumes.into_iter().filter(|v| v.state == "ready").map(|v| v.volume).collect())
}

/// What the index lists on `drive` tagged with one of `names`. None when it didn't answer for the drive.
#[cfg(windows)]
fn index_list(drive: &str, names: &[String]) -> Option<Vec<String>> {
    use shadui_index::client::{self, Event};
    let request = shadui_index::proto::SearchRequest {
        query: format!("tag:{}", names.join(";")),
        roots: vec![format!("{}\\", drive.trim_end_matches('\\'))],
        hidden: true,
        exclude: Vec::new(),
        max_results: None,
        fuzzy: false,
        rank: false,
        marks: false,
    };
    let (mut found, mut answered) = (Vec::new(), false);
    let ran = client::search(&request, |event| {
        match event {
            Event::Plan { indexed, .. } => {
                answered = !indexed.is_empty();
                return answered;
            }
            Event::Hits(hits) => found.extend(hits.into_iter().map(|h| h.path)),
            Event::Done { .. } => {}
        }
        true
    });
    (ran.is_ok() && answered).then_some(found)
}

#[cfg(not(windows))]
fn index_drives() -> Option<Vec<String>> {
    None
}

#[cfg(not(windows))]
fn index_list(_: &str, _: &[String]) -> Option<Vec<String>> {
    None
}

#[cfg(test)]
mod tests {
    use super::super::{caps, ledger::Ledger, read_all, work_with, Marks, World};
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn folder(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("glint-settle-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn marks(label: Option<&str>, tags: &[&str]) -> Marks {
        Marks { label: label.map(String::from), tags: tags.iter().map(|t| t.to_string()).collect(), comment: None }
    }

    /// Files in `dir` written as 0.16 wrote them: tagged Red (and one tagged Red with a label already, one with
    /// nothing to settle), through a world with no labels so nothing is settled while they are made.
    fn make(dir: &Path, w: &World, how: &[(&str, Marks)]) -> Vec<String> {
        let mut paths = Vec::new();
        for (name, m) in how {
            let p = dir.join(name);
            std::fs::write(&p, b"hello").unwrap();
            paths.push(p.to_string_lossy().into_owned());
            work_with(vec![(paths[paths.len() - 1].clone(), Change::Put(m.clone()))], w, &[]);
        }
        paths
    }

    #[test]
    fn the_batches_settle_each_once_write_nothing_for_a_file_that_needs_nothing_stop_when_told_and_never_run_twice() {
        let dir = folder("batches");
        let l = Mutex::new(Ledger::open(Some(dir.join("marks.jsonl"))));
        let w = World { streams: &caps::has_streams, ledger: &l, empty: None, now: 1_000 };
        let labels = default_labels();
        let mut how: Vec<(String, Marks)> = (0..450).map(|i| (format!("red-{i:03}.txt"), marks(None, &["Red", "work"]))).collect();
        how.push(("labeled.txt".into(), marks(Some("Blue"), &["Red"])));
        how.push(("plain.txt".into(), marks(None, &["work"])));
        let how: Vec<(&str, Marks)> = how.iter().map(|(n, m)| (n.as_str(), m.clone())).collect();
        let paths = make(&dir, &w, &how);
        let marker = dir.join("marks-settled.json");

        let sets: RefCell<Vec<usize>> = RefCell::default();
        let answered = Cell::new(true);
        let drives = || answered.get().then(|| vec!["C:".to_string()]);
        let list = |_: &str, names: &[String]| {
            assert!(names.iter().any(|n| n == "Red") && names.len() == 7, "{names:?}");
            Some(paths.clone())
        };
        // How many files each batch wrote.
        let set = |items: Vec<(String, Change)>| {
            let out = work_with(items, &w, &labels);
            sets.borrow_mut().push(out.iter().filter(|o| o.after.as_ref().is_ok_and(|(m, _)| *m != o.before)).count());
            out
        };
        let seam = Seam { drives: &drives, list: &list, set: &set };

        // Told to stop after the first batch: it has done 200 files and the marker says where.
        let asked = Cell::new(0);
        run_with(&seam, &labels, &marker, &|| {
            asked.set(asked.get() + 1);
            asked.get() > 1
        });
        assert_eq!(*sets.borrow(), [198]);
        assert!(!load(&marker).drives["C:"].done);

        // The next run takes it up where it was: the other red files; the two that need nothing (sorted among the first 200) are never written.
        run_with(&seam, &labels, &marker, &|| false);
        assert_eq!(*sets.borrow(), [198, 200, 52]);
        assert!(load(&marker).drives["C:"].done);
        let read = read_all(&paths, &w);
        assert!(read[..450].iter().all(|m| *m == marks(Some("Red"), &["work"])));
        assert_eq!(read[450], marks(Some("Blue"), &["Red"]));
        assert_eq!(read[451], marks(None, &["work"]));

        // The marker says it is done for every drive: it never runs again.
        run_with(&seam, &labels, &marker, &|| false);
        assert_eq!(sets.borrow().len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_service_that_is_not_there_or_a_drive_it_does_not_answer_for_changes_nothing_and_leaves_no_marker() {
        let dir = folder("absent");
        let marker = dir.join("marks-settled.json");
        let touched = Cell::new(false);
        let set = |_: Vec<(String, Change)>| {
            touched.set(true);
            Vec::new()
        };
        let none_drives = || None;
        let list = |_: &str, _: &[String]| Some(Vec::new());
        run_with(&Seam { drives: &none_drives, list: &list, set: &set }, &default_labels(), &marker, &|| false);
        let one = || Some(vec!["D:".to_string()]);
        let no_answer = |_: &str, _: &[String]| None;
        run_with(&Seam { drives: &one, list: &no_answer, set: &set }, &default_labels(), &marker, &|| false);
        assert!(!touched.get() && !marker.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_settled_file_keeps_its_dates_and_its_tags_stream_is_the_old_format_without_the_color() {
        let dir = folder("dates");
        let l = Mutex::new(Ledger::open(Some(dir.join("marks.jsonl"))));
        let w = World { streams: &caps::has_streams, ledger: &l, empty: None, now: 1_000 };
        let paths = make(&dir, &w, &[("a.txt", marks(None, &["Red", "work"]))]);
        let modified = std::fs::metadata(&paths[0]).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let (drives, list) = (|| Some(vec!["C:".to_string()]), |_: &str, _: &[String]| Some(paths.clone()));
        let set = |i: Vec<(String, Change)>| work_with(i, &w, &default_labels());
        run_with(&Seam { drives: &drives, list: &list, set: &set }, &default_labels(), &dir.join("m.json"), &|| false);
        assert_eq!(std::fs::metadata(&paths[0]).unwrap().modified().unwrap(), modified);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), b"hello");
        assert_eq!(read_all(&paths, &w), vec![marks(Some("Red"), &["work"])]);
        if caps::has_streams(&paths[0]) {
            let tags = std::fs::read_to_string(format!("{}:glint.tags", paths[0])).unwrap();
            assert_eq!(tags.lines().collect::<Vec<_>>(), ["work"]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
