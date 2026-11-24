//! Marks on files and folders (docs/gpui/marks/PLAN.md): a label, tags and a comment, kept in the first of
//! three homes that works (5.4): the file's own streams ([`streams`]), `.glint-marks` beside it ([`beside`]),
//! or the ledger on this PC ([`ledger`]). What they hold and the filter are glint-model's ([`glint_model::marks`]).
//!
//! - Reading and writing: [`marks_read`], [`marks_set`], [`marks_put`], [`read`]; fd's drives: [`matches`].
//! - Following an item Glint copies or moves: [`carry`].
//! - Every label and tag in use: [`marks_all`].
//! - Out and in: [`marks_export`], [`import_plan`], [`import_apply`], [`xyplorer_found`].
//!
//! The S0 contracts: the bodies that do nothing are filled by the wave's cards (PLAN.md section 9).

pub mod beside;
pub mod caps;
pub mod carry;
pub mod export;
pub mod import;
pub mod ledger;
pub mod streams;
pub mod xy_find;

pub use carry::carry;
pub use export::{marks_export, Format};
pub use glint_model::marks::{Change, Home, Marks, Outcome, Wanted};
pub use import::{
    import_apply, import_plan, At, ImportOptions, ImportPlan, ImportReport, LabelPlan, Planned, Progress,
};
pub use xy_find::{xyplorer_found, Found};

use crate::fsx::blocking;
use glint_model::marks::{tidy, Beside, Line, Why, COMMENT_CAP, TAGS_CAP};
use ledger::Ledger;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// Every label and tag in use with how many items carry it, most used first, and how many items have a
/// comment: the index's for the drives it covers, with the ledger's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub labels: Vec<(String, u64)>,
    pub tags: Vec<(String, u64)>,
    pub comments: u64,
}

/// A batch bigger than this is read on every core.
const PARALLEL_OVER: usize = 512;

/// What the homes ask about the world, so a test can say a drive has no streams and use a ledger of its own.
struct World<'a> {
    /// Whether the volume of a path keeps named streams.
    streams: &'a (dyn Fn(&str) -> bool + Sync),
    ledger: &'a Mutex<Ledger>,
    /// For the shared ledger: true while it holds nothing, so reading never locks it then.
    empty: Option<&'static AtomicBool>,
    /// Unix seconds.
    now: i64,
}

impl World<'_> {
    fn ledger(&self) -> MutexGuard<'_, Ledger> {
        self.ledger.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn ledger_empty(&self) -> bool {
        self.empty.is_some_and(|f| f.load(Ordering::Relaxed))
    }
}

/// The world as it is: this PC's volumes, ledger and clock.
fn real<R>(f: impl FnOnce(&World) -> R) -> R {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    f(&World { streams: &caps::has_streams, ledger: ledger::shared(), empty: Some(ledger::shared_empty()), now })
}

/// The sidecars read in one batch, by folder in lower case: each is looked for once.
type Sidecars = Mutex<HashMap<String, Arc<Option<Beside>>>>;

fn sidecar(cache: &Sidecars, folder: &Path) -> Arc<Option<Beside>> {
    let key = folder.to_string_lossy().to_lowercase();
    if let Some(known) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return known.clone();
    }
    let read = Arc::new(beside::read(folder));
    cache.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_insert(read).clone()
}

/// The folder an item is in and its name; None for a drive's root, a relative name or anything with no name.
fn split(path: &str) -> Option<(PathBuf, String)> {
    let p = Path::new(path.trim_end_matches(['\\', '/']));
    let name = p.file_name()?.to_string_lossy().into_owned();
    let folder = p.parent()?;
    (!folder.as_os_str().is_empty()).then(|| (folder.to_path_buf(), name))
}

/// An item's marks and the home that gave them: the first with anything.
fn find(path: &str, w: &World, sidecars: &Sidecars) -> (Marks, Option<Home>) {
    if let Some(marks) = streams::read(path) {
        return (tidy(&marks), Some(Home::Stream));
    }
    if let Some((folder, name)) = split(path) {
        let side = sidecar(sidecars, &folder);
        if let Some(line) = (*side).as_ref().and_then(|b| beside::line(b, &name)).filter(|l| !l.marks.is_empty()) {
            return (tidy(&line.marks), Some(Home::Beside));
        }
    }
    if !w.ledger_empty() {
        if let Some(line) = w.ledger().get(path).filter(|l| !l.marks.is_empty()) {
            return (tidy(&line.marks), Some(Home::Ledger));
        }
    }
    (Marks::default(), None)
}

fn read_all(paths: &[String], w: &World) -> Vec<Marks> {
    let sidecars = Sidecars::default();
    if paths.len() > PARALLEL_OVER {
        use rayon::prelude::*;
        paths.par_iter().map(|p| find(p, w, &sidecars).0).collect()
    } else {
        paths.iter().map(|p| find(p, w, &sidecars).0).collect()
    }
}

/// Where an item's marks would go: its streams when its volume has them, else beside it, else the ledger.
fn home_in(path: &str, streams: &dyn Fn(&str) -> bool) -> Home {
    if std::fs::symlink_metadata(path).is_ok() && streams(path) {
        Home::Stream
    } else if split(path).is_some_and(|(folder, _)| folder.is_dir()) {
        Home::Beside
    } else {
        Home::Ledger
    }
}

/// Where `path`'s marks would be kept (PLAN 5.4).
pub fn home_of(path: &str) -> Home {
    home_in(path, &caps::has_streams)
}

/// One folder's sidecar changes in a batch: (name, marks) and the outcomes waiting for it.
struct Group {
    folder: PathBuf,
    changes: Vec<(String, Marks)>,
    waiting: Vec<usize>,
}

/// The worker of `marks_set` and `marks_put`: one outcome a path, in order.
fn work(items: Vec<(String, Change)>, w: &World) -> Vec<Outcome> {
    let sidecars = Sidecars::default();
    let mut out: Vec<Outcome> = Vec::with_capacity(items.len());
    let mut groups: Vec<Group> = Vec::new();
    let mut by_folder: HashMap<String, usize> = HashMap::new();
    let mut ledger_take: Vec<String> = Vec::new();
    let mut ledger_put: Vec<usize> = Vec::new();

    for (path, change) in items {
        let (before, here) = find(&path, w, &sidecars);
        let after = change.apply(&before);
        let i = out.len();
        out.push(Outcome { path: path.clone(), before: before.clone(), after: Err(String::new()) });

        let Some((folder, name)) = split(&path) else {
            out[i].after = Err("A drive itself can't be marked".into());
            continue;
        };
        if after.tags.iter().map(|t| t.len() + 1).sum::<usize>() > TAGS_CAP {
            out[i].after = Err("These are more tags than a file can hold (4 KB)".into());
            continue;
        }
        if after.comment.as_ref().is_some_and(|c| c.len() > COMMENT_CAP) {
            out[i].after = Err("A comment can be 64 KB at most".into());
            continue;
        }
        if after == before {
            let home = here.unwrap_or_else(|| home_in(&path, w.streams));
            out[i].after = Ok((after, home));
            continue;
        }

        let mut group_of = |folder: &Path, groups: &mut Vec<Group>| {
            let key = folder.to_string_lossy().to_lowercase();
            *by_folder.entry(key).or_insert_with(|| {
                groups.push(Group { folder: folder.to_path_buf(), changes: Vec::new(), waiting: Vec::new() });
                groups.len() - 1
            })
        };
        if (w.streams)(&path) {
            match streams::write(&path, &after) {
                Ok(()) => {
                    // The stream has it now: no line beside it or on this PC may say otherwise.
                    let side = sidecar(&sidecars, &folder);
                    if (*side).as_ref().and_then(|b| beside::line(b, &name)).is_some() {
                        let g = group_of(&folder, &mut groups);
                        groups[g].changes.push((name, Marks::default()));
                    }
                    ledger_take.push(path);
                    out[i].after = Ok((after, Home::Stream));
                    continue;
                }
                // A later home would be hidden behind the marks the streams hold.
                Err(e) if streams::read(&path).is_some() => {
                    out[i].after = Err(e);
                    continue;
                }
                Err(_) => {}
            }
        }
        if std::fs::symlink_metadata(&path).is_err() {
            // Nothing to write beside: the item isn't there.
            ledger_put.push(i);
            out[i].after = Ok((after, Home::Ledger));
            continue;
        }
        let g = group_of(&folder, &mut groups);
        groups[g].changes.push((name, after.clone()));
        groups[g].waiting.push(i);
        out[i].after = Ok((after, Home::Beside));
    }

    for g in groups {
        match beside::change(&g.folder, &g.changes, w.now) {
            Ok(()) => ledger_take.extend(g.waiting.iter().map(|&i| out[i].path.clone())),
            Err(_) => {
                for i in g.waiting {
                    if let Ok((_, home)) = &mut out[i].after {
                        *home = Home::Ledger;
                    }
                    ledger_put.push(i);
                }
            }
        }
    }

    if !ledger_put.is_empty() || (!ledger_take.is_empty() && !w.ledger_empty()) {
        let mut ledger = w.ledger();
        for path in &ledger_take {
            ledger.take(path);
        }
        for &i in &ledger_put {
            if let Ok((marks, _)) = &out[i].after {
                ledger.put(&out[i].path, marks, Why::Shut, w.now);
            }
        }
        // A take that couldn't be written is no loss: the home before it wins when read.
        if let Err(e) = ledger.save() {
            for &i in &ledger_put {
                out[i].after = Err(format!("Couldn't keep the marks on this PC: {e}"));
            }
        }
    }
    out
}

/// Each path's marks, for the rows on screen (one per path, in order).
pub async fn marks_read(paths: Vec<String>) -> Result<Vec<Marks>, String> {
    blocking(move || Ok(real(|w| read_all(&paths, w)))).await
}

/// Changes each path's marks, in its home: what each had before, and after with where it is now, or why not.
pub async fn marks_set(paths: Vec<String>, change: Change) -> Result<Vec<Outcome>, String> {
    blocking(move || Ok(real(|w| work(paths.into_iter().map(|p| (p, change.clone())).collect(), w)))).await
}

/// Gives each path exactly these marks (undo, paste, an import's batch).
pub async fn marks_put(items: Vec<(String, Marks)>) -> Result<Vec<Outcome>, String> {
    blocking(move || Ok(real(|w| work(items.into_iter().map(|(p, m)| (p, Change::Put(m))).collect(), w)))).await
}

/// An item's marks: its streams', else its line beside it, else the ledger's. None known is no marks.
pub fn read(path: &str) -> Marks {
    real(|w| find(path, w, &Sidecars::default()).0)
}

/// Whether the item at `path` is what `wanted` asks for (fd's drives, where there is no index).
pub fn matches(path: &str, wanted: &Wanted) -> bool {
    wanted.is_empty() || wanted.accepts(&read(path))
}

/// The index's counts with the ledger's lines added: labels and tags alike in any case are one, shown as first
/// met, most used first and then by name.
fn merge(index: Counts, lines: impl Iterator<Item = Line>) -> Counts {
    fn add(into: &mut Vec<(String, u64)>, seen: &mut HashMap<String, usize>, name: &str, n: u64) {
        match seen.get(&name.to_lowercase()) {
            Some(&i) => into[i].1 += n,
            None => {
                seen.insert(name.to_lowercase(), into.len());
                into.push((name.to_string(), n));
            }
        }
    }
    let (mut labels, mut tags, mut comments) = (Vec::new(), Vec::new(), index.comments);
    let (mut seen_labels, mut seen_tags) = (HashMap::new(), HashMap::new());
    for (name, n) in &index.labels {
        add(&mut labels, &mut seen_labels, name, *n);
    }
    for (name, n) in &index.tags {
        add(&mut tags, &mut seen_tags, name, *n);
    }
    for line in lines {
        let marks = tidy(&line.marks);
        if let Some(l) = &marks.label {
            add(&mut labels, &mut seen_labels, l, 1);
        }
        for t in &marks.tags {
            add(&mut tags, &mut seen_tags, t, 1);
        }
        comments += u64::from(marks.comment.is_some());
    }
    for v in [&mut labels, &mut tags] {
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())).then_with(|| a.0.cmp(&b.0)));
    }
    Counts { labels, tags, comments }
}

/// Every label and tag in use, and how many items have a comment.
pub async fn marks_all() -> Result<Counts, String> {
    blocking(|| {
        #[cfg(windows)]
        let index = shadui_index::client::marks()
            .map(|c| Counts { labels: c.labels, tags: c.tags, comments: c.comments })
            .unwrap_or_default();
        #[cfg(not(windows))]
        let index = Counts::default();
        let lines: Vec<Line> = if ledger::shared_empty().load(Ordering::Relaxed) {
            Vec::new()
        } else {
            ledger::shared().lock().unwrap_or_else(|e| e.into_inner()).lines().cloned().collect()
        };
        Ok(merge(index, lines.into_iter()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn none(_: &str) -> bool {
        false
    }

    /// A folder of the test's own, empty.
    fn folder() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("glint-marks-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn ledger_in(dir: &Path) -> Mutex<Ledger> {
        Mutex::new(Ledger::open(Some(dir.join("marks.jsonl"))))
    }

    fn world<'a>(ledger: &'a Mutex<Ledger>, streams: &'a (dyn Fn(&str) -> bool + Sync)) -> World<'a> {
        World { streams, ledger, empty: None, now: 1_000 }
    }

    fn file(dir: &Path, name: &str) -> String {
        let p = dir.join(name);
        std::fs::write(&p, b"hello").unwrap();
        p.to_string_lossy().into_owned()
    }

    fn label(name: &str) -> Change {
        Change::Label(Some(name.into()))
    }

    fn marked(name: &str) -> Marks {
        Marks { label: Some(name.into()), ..Marks::default() }
    }

    fn set(paths: &[&String], change: Change, w: &World) -> Vec<Outcome> {
        work(paths.iter().map(|p| ((*p).clone(), change.clone())).collect(), w)
    }

    fn sidecar_text(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(".glint-marks")).unwrap_or_default()
    }

    #[test]
    fn a_label_on_a_file_where_streams_work_goes_in_its_stream_and_leaves_its_dates() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&dir);
        let w = world(&l, &caps::has_streams);
        let path = file(&dir, "a.txt");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let out = set(&[&path], label("Red"), &w);
        assert_eq!(out[0].before, Marks::default());
        assert_eq!(out[0].after, Ok((marked("Red"), Home::Stream)));
        assert_eq!(streams::read(&path), Some(marked("Red")));
        assert_eq!(read_all(std::slice::from_ref(&path), &w), vec![marked("Red")]);
        assert!(!dir.join(".glint-marks").exists());
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_no_streams_the_label_goes_in_a_hidden_sidecar_of_one_line() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let path = file(&dir, "a.txt");
        let out = set(&[&path], label("Red"), &w);
        assert_eq!(out[0].after, Ok((marked("Red"), Home::Beside)));
        let text = sidecar_text(&dir);
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("{\"glint-marks\":1}"));
        let line: serde_json::Value = serde_json::from_str(lines.next().unwrap_or("")).unwrap();
        let mut keys: Vec<&String> = line.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(keys, ["at", "label", "name"]);
        assert_eq!((line["name"].as_str(), line["label"].as_str(), line["at"].as_i64()), (Some("a.txt"), Some("Red"), Some(1_000)));
        assert_eq!(lines.next(), None);
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            assert_ne!(std::fs::metadata(dir.join(".glint-marks")).unwrap().file_attributes() & 2, 0, "hidden");
        }
        assert_eq!(read_all(std::slice::from_ref(&path), &w), vec![marked("Red")]);
        assert!(streams::read(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn three_items_of_a_folder_come_back_in_order_and_five_paths_read_in_order() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let (c, a, b, d) = (file(&dir, "c.txt"), file(&dir, "a.txt"), file(&dir, "b.txt"), file(&dir, "d.txt"));
        let gone = dir.join("e.txt").to_string_lossy().into_owned();
        let out = set(&[&c, &a, &b], label("Red"), &w);
        assert_eq!(out.iter().map(|o| o.path.as_str()).collect::<Vec<_>>(), [&c, &a, &b]);
        assert!(out.iter().all(|o| o.after == Ok((marked("Red"), Home::Beside))));
        let names: Vec<String> = sidecar_text(&dir)
            .lines()
            .skip(1)
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["name"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(names, ["a.txt", "b.txt", "c.txt"]);
        let read = read_all(&[a, d, gone, c, b], &w);
        let red = marked("Red");
        assert_eq!(read, vec![red.clone(), Marks::default(), Marks::default(), red.clone(), red]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn taking_the_marks_off_the_last_marked_item_removes_the_sidecar() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let a = file(&dir, "a.txt");
        set(&[&a], label("Red"), &w);
        assert!(dir.join(".glint-marks").exists());
        let out = set(&[&a], Change::Clear, &w);
        assert_eq!(out[0].before, marked("Red"));
        assert!(!dir.join(".glint-marks").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_line_whose_file_was_deleted_is_gone_after_the_next_write() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let (a, b) = (file(&dir, "a.txt"), file(&dir, "b.txt"));
        set(&[&a, &b], label("Red"), &w);
        std::fs::remove_file(&a).unwrap();
        assert!(sidecar_text(&dir).contains("a.txt"));
        set(&[&b], label("Blue"), &w);
        let text = sidecar_text(&dir);
        assert!(!text.contains("a.txt") && text.contains("b.txt"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sidecar_from_a_later_version_keeps_its_version_and_unknown_keys() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let fixture = format!("{}/../../tests/fixtures/marks/later-version.glint-marks", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(fixture, dir.join(".glint-marks")).unwrap();
        let (a, b) = (file(&dir, "a.txt"), file(&dir, "b.txt"));
        let out = set(&[&b], label("Blue"), &w);
        assert_eq!(out[0].after.as_ref().map(|(_, h)| *h), Ok(Home::Beside));
        let text = sidecar_text(&dir);
        assert_eq!(text.lines().next(), Some("{\"glint-marks\":2}"));
        let a_line = text.lines().find(|l| l.contains("a.txt")).unwrap_or("");
        assert!(a_line.contains("\"rating\":4") && a_line.contains("\"color\":\"#ff8800\""), "{a_line}");
        assert_eq!(read_all(&[a, b], &w)[1].label.as_deref(), Some("Blue"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_cannot_hold_a_sidecar_sends_the_marks_to_the_ledger() {
        let dir = folder();
        std::fs::create_dir(dir.join(".glint-marks")).unwrap();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let a = file(&dir, "a.txt");
        let out = set(&[&a], label("Red"), &w);
        assert_eq!(out[0].after, Ok((marked("Red"), Home::Ledger)));
        let text = std::fs::read_to_string(dir.join("marks.jsonl")).unwrap();
        assert!(text.starts_with("{\"glint-ledger\":1}\n"), "{text}");
        let line: serde_json::Value = serde_json::from_str(text.lines().nth(1).unwrap_or("")).unwrap();
        assert_eq!((line["path"].as_str(), line["why"].as_str()), (Some(a.as_str()), Some("shut")));
        assert_eq!(read_all(&[a], &w), vec![marked("Red")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_mark_is_in_one_place_and_the_stream_wins_over_a_line_that_says_otherwise() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&dir);
        let a = file(&dir, "a.txt");
        set(&[&a], label("Beside"), &world(&l, &none));
        l.lock().unwrap().put(&a, &marked("Ledger"), Why::Shut, 1);
        assert!(sidecar_text(&dir).contains("Beside"));
        let w = world(&l, &caps::has_streams);
        let out = set(&[&a], label("Stream"), &w);
        assert_eq!(out[0].after, Ok((marked("Stream"), Home::Stream)));
        assert!(!dir.join(".glint-marks").exists());
        assert!(l.lock().unwrap().get(&a).is_none());
        // A line written by hand beside a file whose streams have marks says nothing.
        beside::change(&dir, &[("a.txt".into(), marked("Other"))], 5).unwrap();
        assert_eq!(read_all(&[a], &w), vec![marked("Stream")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marks_in_streams_that_cannot_be_written_are_an_error_and_nothing_goes_beside() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&dir);
        let w = world(&l, &caps::has_streams);
        let a = file(&dir, "a.txt");
        set(&[&a], label("Red"), &w);
        let mut perms = std::fs::metadata(&a).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&a, perms.clone()).unwrap();
        let out = set(&[&a], label("Blue"), &w);
        #[allow(clippy::permissions_set_readonly_false)] // so the folder can be removed
        perms.set_readonly(false);
        std::fs::set_permissions(&a, perms).unwrap();
        assert!(out[0].after.is_err(), "{:?}", out[0].after);
        assert!(!dir.join(".glint-marks").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tags_set_and_tags_read_work_as_they_did_through_the_homes() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        let a = file(&dir, "a.txt");
        let failed = crate::host::async_runtime::block_on(crate::tags::tags_set(vec![a.clone()], vec!["Work".into(), "Red".into()], vec![])).unwrap();
        assert!(failed.is_empty());
        assert_eq!(crate::tags::read(&a), ["Work", "Red"]);
        crate::host::async_runtime::block_on(crate::tags::tags_set(vec![a.clone()], vec![], vec!["work".into()])).unwrap();
        assert_eq!(crate::tags::read(&a), ["Red"]);
        let b = file(&dir, "b.txt");
        std::fs::write(format!("{b}:glint.tags"), "Hand\nMade\n").unwrap();
        let read = crate::host::async_runtime::block_on(crate::tags::tags_read(vec![a, b])).unwrap();
        assert_eq!(read, vec![vec!["Red".to_string()], vec!["Hand".to_string(), "Made".to_string()]]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_comment_over_the_cap_is_refused_with_words_and_kept_nowhere() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let a = file(&dir, "a.txt");
        let out = set(&[&a], Change::Comment(Some("x".repeat(COMMENT_CAP + 1))), &w);
        assert!(out[0].after.as_ref().is_err_and(|e| e.contains("64 KB")), "{:?}", out[0].after);
        assert_eq!(read_all(&[a], &w), vec![Marks::default()]);
        assert!(!dir.join(".glint-marks").exists() && l.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_itself_cannot_be_marked() {
        let dir = folder();
        let l = ledger_in(&dir);
        let out = set(&[&"C:\\".to_string()], label("Red"), &world(&l, &none));
        assert!(out[0].after.as_ref().is_err_and(|e| e.contains("A drive itself can't be marked")));
        assert!(read("C:\\").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn put_gives_exactly_the_marks_named_and_putting_the_before_back_is_undo() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let a = file(&dir, "a.txt");
        let first = Marks { label: Some("Old".into()), tags: vec!["t".into()], comment: Some("c".into()) };
        work(vec![(a.clone(), Change::Put(first.clone()))], &w);
        let new = Marks { label: Some("a\nb".into()), tags: vec![], comment: None };
        let out = work(vec![(a.clone(), Change::Put(new))], &w);
        assert_eq!(out[0].before, first);
        assert_eq!(out[0].after, Ok((marked("a b"), Home::Beside)));
        assert_eq!(read_all(std::slice::from_ref(&a), &w), vec![marked("a b")]);
        work(vec![(a.clone(), Change::Put(out[0].before.clone()))], &w);
        assert_eq!(read_all(&[a], &w), vec![first]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_that_changes_nothing_writes_nothing() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&dir);
        let w = world(&l, &caps::has_streams);
        let a = file(&dir, "a.txt");
        set(&[&a], label("Red"), &w);
        let stream = format!("{a}:glint.label");
        let bytes = std::fs::read(&stream).unwrap();
        let out = set(&[&a], label("Red"), &w);
        assert_eq!(std::fs::read(&stream).unwrap(), bytes);
        assert_eq!(out[0].after, Ok((marked("Red"), Home::Stream)));
        assert_eq!(out[0].before, marked("Red"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn six_hundred_paths_mostly_missing_answer_six_hundred_in_order() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &none);
        let a = file(&dir, "a.txt");
        set(&[&a], label("Red"), &w);
        let mut paths: Vec<String> = (0..599).map(|i| dir.join(format!("missing-{i}.txt")).to_string_lossy().into_owned()).collect();
        paths.push(a);
        let read = read_all(&paths, &w);
        assert_eq!(read.len(), 600);
        assert!(read[..599].iter().all(Marks::is_empty));
        assert_eq!(read[599], marked("Red"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_wanted_matches_anything_and_homes_are_told_by_the_volume_and_folder() {
        let dir = folder();
        assert!(matches(&dir.join("not-there.txt").to_string_lossy(), &Wanted::default()));
        let a = file(&dir, "a.txt");
        if caps::has_streams(&a) {
            assert_eq!(home_of(&a), Home::Stream);
        }
        assert_eq!(home_of(&dir.join("nope").join("x.txt").to_string_lossy()), Home::Ledger);
        assert_eq!(home_in(&a, &none), Home::Beside);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_join_the_ledger_in_any_case_most_used_first() {
        let line = |label: &str, tags: &[&str], comment: bool| Line {
            name: "x".into(),
            marks: Marks {
                label: Some(label.into()),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                comment: comment.then(|| "c".to_string()),
            },
            ..Line::default()
        };
        let lines = vec![line("Red", &["Work", "b"], true), line("red", &["work"], false), line("Blue", &["B", "a"], true)];
        let counts = merge(Counts::default(), lines.into_iter());
        assert_eq!(counts.labels, [("Red".to_string(), 2), ("Blue".to_string(), 1)]);
        assert_eq!(counts.tags, [("b".to_string(), 2), ("Work".to_string(), 2), ("a".to_string(), 1)]);
        assert_eq!(counts.comments, 2);
        let index = Counts { labels: vec![("blue".into(), 5)], tags: vec![], comments: 3 };
        let joined = merge(index, [line("Blue", &[], false)].into_iter());
        assert_eq!((joined.labels, joined.comments), (vec![("blue".to_string(), 6)], 3));
    }
}
