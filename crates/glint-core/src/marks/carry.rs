//! Marks follow what Glint copies and moves (PLAN.md 6.3, F21). Between drives that keep streams Windows'
//! engine and `CopyFileExW` take a **file's** along; where the homes differ (NTFS to exFAT and back) they are
//! read at the source and written at the destination, and a move takes them out of the source's sidecar or the
//! ledger. A **folder's own** marks are always read and written: no copy takes a folder's streams (Windows'
//! engine makes the folder anew, without them, and so does the queue).
//!
//! The faces: [`carry`] for a batch of files and folders, [`carry_files`] for the queue's files, [`carry_one`]
//! for one item, and [`taken`] and [`given`] for what is made in the place of another (read before the
//! original goes, written to what takes its place).

use super::{beside, caps, find, ledger, sidecar, split, streams, work, Change, Marks, Sidecars, World};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Whether the volume of `path` keeps streams, as `caps::has_streams` says, remembered by folder for a few seconds:
/// the queue asks it of two paths for every file it copies, and a folder's files are all on one volume
/// (`has_streams` alone costs a Windows call each).
fn streams_here(path: &str) -> bool {
    static KNOWN: Mutex<Option<HashMap<String, (bool, Instant)>>> = Mutex::new(None);
    let Some(folder) = Path::new(path).parent().filter(|f| !f.as_os_str().is_empty()) else {
        return caps::has_streams(path);
    };
    let key = folder.to_string_lossy().to_lowercase();
    if let Some(&(answer, at)) = KNOWN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key)) {
        if at.elapsed() < Duration::from_secs(10) {
            return answer;
        }
    }
    let answer = caps::has_streams(path);
    let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
    let known = known.get_or_insert_with(HashMap::new);
    if known.len() > 512 {
        known.clear();
    }
    known.insert(key, (answer, Instant::now()));
    answer
}

/// The world as it is, with the volumes' answers remembered by folder.
fn real<R>(f: impl FnOnce(&World) -> R) -> R {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    f(&World { streams: &streams_here, ledger: ledger::shared(), empty: Some(ledger::shared_empty()), now })
}

/// After Glint copied (`moved` false) or moved or renamed (`moved` true) each `(source, destination)` pair: a
/// file's marks where the homes differ, a folder's own always. Never fails: a pair that can't be read or
/// written is left.
pub fn carry(pairs: &[(String, String)], moved: bool) {
    real(|w| carry_in(pairs, moved, true, w));
}

/// [`carry`] for pairs known to be files (the queue's): no pair is looked at to tell whether it is a folder.
pub fn carry_files(pairs: &[(String, String)], moved: bool) {
    real(|w| carry_in(pairs, moved, false, w));
}

/// One pair (`carry` of a list of one).
pub fn carry_one(source: &str, destination: &str, moved: bool) {
    carry(&[(source.to_string(), destination.to_string())], moved);
}

/// The marks of an original about to be recycled (or otherwise replaced), to hand to `given`.
pub fn taken(original: &str) -> Marks {
    real(|w| find(original, w, &Sidecars::default()).0)
}

/// Gives the file made in an original's place the marks `taken` from it, in the first home that takes them.
pub fn given(made: &str, marks: &Marks) {
    real(|w| given_in(made, marks, w));
}

fn given_in(made: &str, marks: &Marks, w: &World) {
    if !marks.is_empty() {
        work(vec![(made.to_string(), Change::Put(marks.clone()))], w);
    }
}

/// `folders`: some pairs may be folders (each destination is looked at to tell).
fn carry_in(pairs: &[(String, String)], moved: bool, folders: bool, w: &World) {
    let sidecars = Sidecars::default();
    let mut puts: Vec<(String, Change)> = Vec::new();
    // Each put's destination with the source whose line goes once it is written (a move only).
    let mut after: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    // Folder pairs: the ledger's lines under the source go under the destination, whatever the drives keep.
    let mut dirs: Vec<(&str, &str)> = Vec::new();
    for (source, dest) in pairs {
        let dest_dir = folders && Path::new(dest).is_dir();
        if dest_dir && source.to_lowercase() != dest.to_lowercase() {
            dirs.push((source, dest));
        }
        // Both ends keep streams: Windows took a file's along, and nothing is read. Never a folder's own.
        let came_along = (w.streams)(source) && (w.streams)(dest) && !dest_dir;
        if came_along || !seen.insert(dest.as_str()) || source.to_lowercase() == dest.to_lowercase() {
            continue;
        }
        if std::fs::symlink_metadata(dest).is_err() {
            continue;
        }
        let (marks, _) = find(source, w, &sidecars);
        if marks.is_empty() {
            continue;
        }
        // Marks the destination's streams already hold came with the file: they stay.
        let written = streams::read(dest).is_some();
        if !written {
            puts.push((dest.clone(), Change::Put(marks)));
        }
        if moved {
            after.push((if written { String::new() } else { dest.clone() }, source.clone()));
        }
    }
    if !dirs.is_empty() && !w.ledger_empty() {
        let mut ledger = w.ledger();
        for (source, dest) in dirs {
            ledger.carry_under(source, dest, moved);
        }
        let _ = ledger.save();
    }
    let wrote: HashSet<String> = if puts.is_empty() {
        HashSet::new()
    } else {
        work(puts, w).into_iter().filter(|o| o.after.is_ok()).map(|o| o.path).collect()
    };

    // A move: the source's line goes from its folder's sidecar (one write a folder) and from the ledger.
    let mut lines: HashMap<String, (PathBuf, Vec<(String, Marks)>)> = HashMap::new();
    let mut ledger_gone: Vec<&str> = Vec::new();
    for (dest, source) in &after {
        if !dest.is_empty() && !wrote.contains(dest) {
            continue; // not written: the source's line is all that is left of them
        }
        if let Some((folder, name)) = split(source) {
            let side = sidecar(&sidecars, &folder);
            if (*side).as_ref().and_then(|b| beside::line(b, &name)).is_some() {
                lines.entry(folder.to_string_lossy().to_lowercase()).or_insert_with(|| (folder, Vec::new())).1.push((name, Marks::default()));
            }
        }
        ledger_gone.push(source);
    }
    for (folder, changes) in lines.into_values() {
        let _ = beside::change(&folder, &changes, w.now);
    }
    if !ledger_gone.is_empty() && !w.ledger_empty() {
        let mut ledger = w.ledger();
        for source in ledger_gone {
            ledger.take(source);
        }
        let _ = ledger.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marks::ledger::Ledger;
    use crate::marks::{caps, Home};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A folder of the test's own, empty.
    fn folder() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("glint-carry-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
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

    fn marked(name: &str) -> Marks {
        Marks { label: Some(name.into()), ..Marks::default() }
    }

    fn pair(a: &str, b: &str) -> (String, String) {
        (a.to_string(), b.to_string())
    }

    fn sidecar_text(dir: &Path) -> String {
        std::fs::read_to_string(dir.join(".glint-marks")).unwrap_or_default()
    }

    fn beside_label(dir: &Path, name: &str) -> Option<String> {
        let side = beside::read(dir)?;
        beside::line(&side, name).and_then(|l| l.marks.label.clone())
    }

    fn no(_: &str) -> bool {
        false
    }

    #[test]
    fn between_two_folders_that_both_keep_streams_nothing_is_read_or_written() {
        let (src, dst) = (folder(), folder());
        let l = ledger_in(&src);
        let a = file(&src, "a.txt");
        beside::change(&src, &[("a.txt".into(), marked("Red"))], 5).unwrap();
        let planted = std::fs::read(src.join(".glint-marks")).unwrap();
        let copy = file(&dst, "a.txt");
        carry_in(&[pair(&a, &copy)], false, true, &world(&l, &|_| true));
        carry_in(&[pair(&a, &copy)], true, true, &world(&l, &|_| true));
        assert_eq!(std::fs::read(src.join(".glint-marks")).unwrap(), planted);
        assert!(!dst.join(".glint-marks").exists());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn a_folder_copied_between_two_drives_with_streams_keeps_its_own_marks_and_a_file_is_not_read() {
        let (src, dst) = (folder(), folder());
        if !caps::has_streams(&dst.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&src);
        let (from, to) = (src.join("album"), dst.join("album"));
        std::fs::create_dir(&from).unwrap();
        std::fs::create_dir(&to).unwrap();
        let (from, to) = (from.to_string_lossy().into_owned(), to.to_string_lossy().into_owned());
        streams::write(&from, &marked("Red")).unwrap();
        // A file whose copy lost its streams some other way: between two such drives it is not Glint's to mend.
        let (a, copy) = (file(&src, "a.txt"), file(&dst, "a.txt"));
        streams::write(&a, &marked("Blue")).unwrap();
        carry_in(&[pair(&from, &to), pair(&a, &copy)], false, true, &world(&l, &|_| true));
        assert_eq!(streams::read(&to), Some(marked("Red")));
        assert_eq!(streams::read(&from), Some(marked("Red")), "a copy leaves the original its marks");
        assert_eq!(streams::read(&copy), None);
        // The queue's files are not looked at.
        let to2 = dst.join("album2");
        std::fs::create_dir(&to2).unwrap();
        carry_in(&[pair(&from, &to2.to_string_lossy())], false, false, &world(&l, &|_| true));
        assert_eq!(streams::read(&to2.to_string_lossy()), None);
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn a_copy_from_marks_beside_the_files_to_a_folder_with_streams_has_them_in_its_streams() {
        let (src, dst) = (folder(), folder());
        if !caps::has_streams(&dst.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&src);
        let a = file(&src, "a.txt");
        beside::change(&src, &[("a.txt".into(), marked("Red"))], 5).unwrap();
        let copy = file(&dst, "a.txt");
        let from = format!("{}{}", src.display(), std::path::MAIN_SEPARATOR);
        carry_in(&[pair(&a, &copy)], false, true, &world(&l, &|p| !p.starts_with(&from)));
        assert_eq!(streams::read(&copy), Some(marked("Red")));
        assert_eq!(beside_label(&src, "a.txt").as_deref(), Some("Red"), "the source keeps its line");
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn a_move_from_marks_beside_the_files_to_streams_takes_the_line_and_an_empty_sidecar_goes() {
        let (src, dst) = (folder(), folder());
        if !caps::has_streams(&dst.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&src);
        let a = file(&src, "a.txt");
        beside::change(&src, &[("a.txt".into(), marked("Red"))], 5).unwrap();
        let copy = file(&dst, "a.txt");
        let from = format!("{}{}", src.display(), std::path::MAIN_SEPARATOR);
        carry_in(&[pair(&a, &copy)], true, true, &world(&l, &|p| !p.starts_with(&from)));
        assert_eq!(streams::read(&copy), Some(marked("Red")));
        assert!(!src.join(".glint-marks").exists(), "{}", sidecar_text(&src));
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn a_move_from_streams_to_a_folder_without_leaves_the_line_beside_the_destination() {
        let (src, dst) = (folder(), folder());
        if !caps::has_streams(&src.to_string_lossy()) {
            return;
        }
        let l = ledger_in(&src);
        let a = file(&src, "a.txt");
        streams::write(&a, &marked("Red")).unwrap();
        let copy = file(&dst, "a.txt");
        let to = format!("{}{}", dst.display(), std::path::MAIN_SEPARATOR);
        carry_in(&[pair(&a, &copy)], true, true, &world(&l, &|p| !p.starts_with(&to)));
        assert_eq!(beside_label(&dst, "a.txt").as_deref(), Some("Red"));
        assert!(streams::read(&copy).is_none());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn a_rename_in_a_folder_without_streams_renames_the_line_and_keeps_the_others_and_unknown_keys() {
        let dir = folder();
        let l = ledger_in(&dir);
        let fixture = format!("{}/../../tests/fixtures/marks/later-version.glint-marks", env!("CARGO_MANIFEST_DIR"));
        std::fs::copy(fixture, dir.join(".glint-marks")).unwrap();
        let (_a, b) = (file(&dir, "a.txt"), file(&dir, "b.txt"));
        let c = dir.join("c.txt").to_string_lossy().into_owned();
        std::fs::rename(&b, &c).unwrap();
        carry_in(&[pair(&b, &c)], true, true, &world(&l, &no));
        let text = sidecar_text(&dir);
        assert_eq!(text.lines().next(), Some("{\"glint-marks\":2}"), "{text}");
        let side = beside::read(&dir).unwrap();
        assert_eq!(beside::line(&side, "c.txt").map(|l| l.marks.tags.clone()), Some(vec!["y".to_string()]));
        assert!(beside::line(&side, "b.txt").is_none());
        let a_line = text.lines().find(|l| l.contains("a.txt")).unwrap_or("");
        assert!(a_line.contains("\"rating\":4") && a_line.contains("\"color\":\"#ff8800\""), "{a_line}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_item_whose_marks_are_in_the_ledger_keeps_them_there_under_its_new_path() {
        let (src, dst) = (folder(), folder());
        // A folder that can't hold a sidecar sends the marks to the ledger.
        std::fs::create_dir(dst.join(".glint-marks")).unwrap();
        let l = ledger_in(&src);
        let a = file(&src, "a.txt");
        let copy = file(&dst, "a.txt");
        l.lock().unwrap().put(&a, &marked("Red"), glint_model::marks::Why::Shut, 1);
        carry_in(&[pair(&a, &copy)], true, true, &world(&l, &no));
        let ledger = l.lock().unwrap();
        assert_eq!(ledger.get(&copy).map(|line| line.marks.clone()), Some(marked("Red")));
        assert!(ledger.get(&a).is_none());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn twenty_pairs_of_one_folder_cost_one_write_of_its_sidecar() {
        let (src, dst) = (folder(), folder());
        let l = ledger_in(&src);
        let names: Vec<String> = (0..20).map(|i| format!("f{i}.txt")).collect();
        let changes: Vec<(String, Marks)> = names.iter().map(|n| (n.clone(), marked("Red"))).collect();
        let pairs: Vec<(String, String)> = names.iter().map(|n| pair(&file(&src, n), &file(&dst, n))).collect();
        beside::change(&src, &changes, 5).unwrap();
        // Watches the destination's sidecar for as many different modified times as writes it sees.
        let stop = std::sync::atomic::AtomicBool::new(false);
        let watched = dst.join(".glint-marks");
        let seen = std::thread::scope(|scope| {
            let watcher = scope.spawn(|| {
                let mut seen = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(t) = std::fs::metadata(&watched).and_then(|m| m.modified()) {
                        if seen.last() != Some(&t) {
                            seen.push(t);
                        }
                    }
                    std::thread::yield_now();
                }
                seen
            });
            carry_in(&pairs, false, true, &world(&l, &no));
            stop.store(true, Ordering::Relaxed);
            watcher.join().unwrap_or_default()
        });
        assert!(seen.len() <= 1, "{} writes", seen.len());
        assert_eq!(beside::read(&dst).map(|b| b.lines.len()), Some(20));
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn pairs_that_cannot_be_carried_change_nothing_and_do_not_panic() {
        let (src, dst) = (folder(), folder());
        let l = ledger_in(&src);
        let a = file(&src, "a.txt");
        beside::change(&src, &[("a.txt".into(), marked("Red"))], 5).unwrap();
        let planted = std::fs::read(src.join(".glint-marks")).unwrap();
        let never = |d: &Path| d.join("never.txt").to_string_lossy().into_owned();
        let pairs = [
            pair(&never(&src), &never(&dst)),
            pair(&a, &never(&dst)),
            pair("", ""),
            pair("\u{0}\u{1}*?|", "\u{0}"),
            pair("C:\\", "C:\\"),
            pair("notes\\a.txt", "\\\\?\\nope\\x"),
            pair(&a, &a),
        ];
        for moved in [false, true] {
            carry_in(&[], moved, true, &world(&l, &no));
            carry_in(&pairs, moved, true, &world(&l, &no));
        }
        carry(&[], true);
        assert_eq!(std::fs::read(src.join(".glint-marks")).unwrap(), planted);
        assert!(!dst.join(".glint-marks").exists());
        assert!(l.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    /// A ledger file holding these lines (`rest` carries a key a later version wrote), opened.
    fn seeded(dir: &Path, names: &[&str]) -> Mutex<Ledger> {
        let lines: Vec<glint_model::marks::Line> = names
            .iter()
            .map(|n| {
                let mut rest = serde_json::Map::new();
                rest.insert("future".into(), serde_json::json!({ "n": 3 }));
                glint_model::marks::Line {
                    name: n.to_string(),
                    marks: marked("Red"),
                    at: 77,
                    why: Some(glint_model::marks::Why::Away),
                    rest,
                }
            })
            .collect();
        std::fs::write(dir.join("marks.jsonl"), glint_model::marks::write_ledger(&lines)).unwrap();
        ledger_in(dir)
    }

    #[test]
    fn a_renamed_folder_takes_the_ledgers_lines_under_it_two_levels_deep_and_a_copy_keeps_both() {
        let dir = folder();
        let (old, new) = (dir.join("old"), dir.join("new"));
        std::fs::create_dir_all(old.join("a").join("b")).unwrap();
        let deep = old.join("a").join("b").join("X.txt").to_string_lossy().into_owned();
        let top = old.join("t.txt").to_string_lossy().into_owned();
        let l = seeded(&dir, &[&deep, &top]);
        std::fs::rename(&old, &new).unwrap();
        // A copy first (the source is gone, but its lines are what is carried): both stay.
        carry_in(&[pair(&old.to_string_lossy(), &new.to_string_lossy())], false, true, &world(&l, &no));
        let moved_deep = new.join("a").join("b").join("X.txt").to_string_lossy().into_owned();
        {
            let g = l.lock().unwrap();
            assert!(g.get(&deep).is_some() && g.get(&moved_deep).is_some());
        }
        carry_in(&[pair(&old.to_string_lossy(), &new.to_string_lossy())], true, true, &world(&l, &no));
        let g = l.lock().unwrap();
        assert!(g.get(&deep).is_none() && g.get(&top).is_none());
        let line = g.get(&moved_deep).unwrap();
        assert_eq!((line.at, line.why, line.marks.clone()), (77, Some(glint_model::marks::Why::Away), marked("Red")));
        assert_eq!(line.name, moved_deep, "the name keeps its case below the folder");
        assert_eq!(line.rest["future"]["n"], 3);
        assert!(g.waits_in(&new.join("a").join("b").to_string_lossy()));
        assert!(!g.waits_in(&old.join("a").join("b").to_string_lossy()));
        drop(g);
        let again = Ledger::open(Some(dir.join("marks.jsonl")));
        assert!(again.get(&moved_deep).is_some() && again.get(&deep).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_whose_name_starts_another_s_moves_only_its_own_lines() {
        let dir = folder();
        let (b, bc, to) = (dir.join("b"), dir.join("bc"), dir.join("moved"));
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(&bc).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        let (inb, inbc) = (b.join("x.txt").to_string_lossy().into_owned(), bc.join("y.txt").to_string_lossy().into_owned());
        let l = seeded(&dir, &[&inb, &inbc]);
        carry_in(&[pair(&b.to_string_lossy(), &to.to_string_lossy())], true, true, &world(&l, &no));
        let g = l.lock().unwrap();
        assert!(g.get(&inb).is_none());
        assert!(g.get(&to.join("x.txt").to_string_lossy()).is_some());
        assert!(g.get(&inbc).is_some(), "bc is not under b");
        assert_eq!(g.lines().count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_made_in_anothers_place_gets_its_marks() {
        let dir = folder();
        let l = ledger_in(&dir);
        let w = world(&l, &no);
        let original = file(&dir, "a.png");
        let all = Marks { label: Some("Important".into()), tags: vec!["Work".into(), "Red".into()], comment: Some("mode 4".into()) };
        work(vec![(original.clone(), Change::Put(all.clone()))], &w);
        let marks = find(&original, &w, &Sidecars::default()).0;
        assert_eq!(marks, all);
        std::fs::remove_file(&original).unwrap();
        let made = file(&dir, "a.jpg");
        given_in(&made, &marks, &w);
        let (read, home) = find(&made, &w, &Sidecars::default());
        assert_eq!((read, home), (all, Some(Home::Beside)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
