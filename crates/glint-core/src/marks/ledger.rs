//! The last home: the ledger on this PC, `marks.jsonl` in Glint's data folder (PLAN.md 5.3), for items whose
//! streams and folder can't be written, and an import's items that weren't there. Held in memory by path in
//! lower case, written at the end of the call that changed it (a call is a moment).

use glint_model::marks::lines::LEDGER_MAX_BYTES;
use glint_model::marks::{read_ledger, write_ledger, Line, Marks, Why};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

/// What the file looked like when it was read or written last: its modified time and size.
type Stamp = (SystemTime, u64);

fn stamp_of(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// The lines of the file (at most `LEDGER_MAX_BYTES`, `read_ledger`: a line cut short is left out). A file that isn't
/// there holds none; one that can't be read is None.
fn read_file(path: &Path) -> Option<Vec<Line>> {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let mut bytes = Vec::new();
    f.take(LEDGER_MAX_BYTES as u64 + 1).read_to_end(&mut bytes).ok()?;
    Some(read_ledger(&String::from_utf8_lossy(&bytes)))
}

/// True while the shared ledger holds no line: readers then don't lock it at all.
static SHARED_EMPTY: AtomicBool = AtomicBool::new(false);
/// How many lines of the shared ledger wait for their item (`Why::Away`): a listing reads it to know whether any
/// folder it lists could wake marks (F31).
static SHARED_AWAY: AtomicUsize = AtomicUsize::new(0);
static SHARED: OnceLock<Mutex<Ledger>> = OnceLock::new();

/// The marks kept on this PC, by path.
pub struct Ledger {
    /// Where it is kept; None: in memory only.
    path: Option<PathBuf>,
    /// By the path without its end separator, in lower case.
    lines: HashMap<String, Line>,
    changed: bool,
    /// Told whether the lines are none (the shared one only).
    flag: Option<&'static AtomicBool>,
    /// How many lines are `Why::Away`, kept right by `open`, `put` and `take`.
    away: usize,
    /// How many of those each folder holds, by [`parent_key`]: what a listing asks without a scan (F31).
    away_dirs: HashMap<String, usize>,
    /// Told that count (the shared one only).
    pub(super) away_flag: Option<&'static AtomicUsize>,
    /// The file as it was when read or written last; None: it wasn't there.
    stamp: Option<Stamp>,
    /// What this process put (`Some`) or took (`None`), by key, since the last read: laid over what the file holds
    /// whenever it is read again, so another process's lines are kept and ours win.
    touched: HashMap<String, Option<Line>>,
    /// How many times the file was read (the tests' seam for "didn't read it again").
    reads: usize,
}

fn is_away(line: &Line) -> bool {
    line.why == Some(Why::Away)
}

/// The folder an item's path is in, in lower case and without an end separator.
fn parent_key(path: &str) -> String {
    let t = path.trim_end_matches(['\\', '/']);
    t.rfind(['\\', '/']).map(|i| t[..i].trim_end_matches(['\\', '/']).to_lowercase()).unwrap_or_default()
}

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

impl Ledger {
    /// Reads the file (at most `LEDGER_MAX_BYTES`, `read_ledger`). No file, or one that doesn't read: empty.
    pub fn open(path: Option<PathBuf>) -> Ledger {
        let mut ledger = Ledger {
            path,
            lines: HashMap::new(),
            changed: false,
            flag: None,
            away: 0,
            away_dirs: HashMap::new(),
            away_flag: None,
            stamp: None,
            touched: HashMap::new(),
            reads: 0,
        };
        ledger.reload();
        ledger
    }

    /// Reads the file and lays what this process changed since its last read over it. A file that can't be read
    /// leaves the lines as they are.
    fn reload(&mut self) {
        let Some(path) = self.path.clone() else { return };
        // Taken before the read: a write that lands in between makes the next check read again, never miss it.
        let stamp = stamp_of(&path);
        self.reads += 1;
        let Some(read) = read_file(&path) else { return };
        self.stamp = stamp;
        self.lines = read.into_iter().map(|l| (key(&l.name), l)).collect();
        for (k, line) in &self.touched {
            match line {
                Some(l) => {
                    self.lines.insert(k.clone(), l.clone());
                }
                None => {
                    self.lines.remove(k);
                }
            }
        }
        self.away = 0;
        self.away_dirs.clear();
        let waiting: Vec<String> = self.lines.values().filter(|l| is_away(l)).map(|l| l.name.clone()).collect();
        for name in waiting {
            self.tally(&name, true);
        }
        self.sync();
    }

    /// Reads the file again if another process changed it since (a `metadata` call, nothing more when it didn't).
    /// The shared ledger's own timer calls it once a second (`shared`); `save` calls it before it writes. Never on
    /// the thread that lists a folder.
    pub fn refresh(&mut self) -> bool {
        let changed = self.path.as_deref().is_some_and(|p| stamp_of(p) != self.stamp);
        if changed {
            self.reload();
        }
        changed
    }

    /// One waiting line more (`up`) or less for `path`'s folder.
    fn tally(&mut self, path: &str, up: bool) {
        let dir = parent_key(path);
        if up {
            self.away += 1;
            *self.away_dirs.entry(dir).or_default() += 1;
        } else if let Some(n) = self.away_dirs.get_mut(&dir) {
            self.away = self.away.saturating_sub(1);
            *n -= 1;
            if *n == 0 {
                self.away_dirs.remove(&dir);
            }
        }
    }

    /// Whether a line waits for an item right inside `folder`, in any case: no scan of the lines.
    pub fn waits_in(&self, folder: &str) -> bool {
        self.away > 0 && self.away_dirs.contains_key(&folder.trim_end_matches(['\\', '/']).to_lowercase())
    }

    fn sync(&self) {
        if let Some(flag) = self.flag {
            flag.store(self.lines.is_empty(), Ordering::Relaxed);
        }
        if let Some(flag) = self.away_flag {
            flag.store(self.away, Ordering::Relaxed);
        }
    }

    /// How many lines wait for their item (`Why::Away`).
    pub fn away(&self) -> usize {
        self.away
    }

    /// The waiting lines whose item is right inside `folder`, in any case.
    pub fn away_in(&self, folder: &str) -> Vec<Line> {
        if !self.waits_in(folder) {
            return Vec::new();
        }
        let want = folder.trim_end_matches(['\\', '/']).to_lowercase();
        self.lines.values().filter(|l| is_away(l) && parent_key(&l.name) == want).cloned().collect()
    }

    /// The line for `path`, in any case.
    pub fn get(&self, path: &str) -> Option<&Line> {
        self.lines.get(&key(path))
    }

    /// Gives `path` these marks (`at` in Unix seconds); empty marks take its line away.
    pub fn put(&mut self, path: &str, marks: &Marks, why: Why, at: i64) {
        let k = key(path);
        if marks.is_empty() {
            self.take(path);
            return;
        }
        let rest = self.lines.get(&k).map(|l| l.rest.clone()).unwrap_or_default();
        let line = Line { name: path.to_string(), marks: marks.clone(), at, why: Some(why), rest };
        self.set(line);
    }

    /// Keeps `line` as it is (its `rest` too) under its own name, if that isn't what is held already.
    fn set(&mut self, line: Line) {
        let k = key(&line.name);
        if self.lines.get(&k) != Some(&line) {
            let (was, now) = (self.lines.get(&k).is_some_and(is_away), is_away(&line));
            if was != now {
                self.tally(&line.name, now);
            }
            self.touched.insert(k.clone(), Some(line.clone()));
            self.lines.insert(k, line);
            self.changed = true;
            self.sync();
        }
    }

    /// Every line under the folder `from` (any depth, by key prefix, so `C:\\a\\b` never reaches `C:\\a\\bc`) goes to the
    /// same place under `to`: moved when `moved`, copied when not. Each keeps its marks, `why`, `at` and `rest`.
    /// The caller saves once.
    pub fn carry_under(&mut self, from: &str, to: &str, moved: bool) {
        let prefix = key(from);
        let to = to.trim_end_matches(['\\', '/']);
        if prefix.is_empty() || prefix == key(to) {
            return;
        }
        let under: Vec<Line> = self
            .lines
            .iter()
            .filter(|(k, _)| k.strip_prefix(prefix.as_str()).is_some_and(|r| r.starts_with(['\\', '/'])))
            .map(|(_, l)| l.clone())
            .collect();
        for line in under {
            // The name's own case below the folder is kept: skip as many chars as the prefix has bytes in lower case.
            let mut seen = 0;
            let below = line
                .name
                .char_indices()
                .find(|&(_, c)| {
                    let done = seen >= prefix.len();
                    seen += c.to_lowercase().map(char::len_utf8).sum::<usize>();
                    done
                })
                .map_or("", |(i, _)| &line.name[i..]);
            let name = format!("{to}{below}");
            if moved {
                self.take(&line.name);
            }
            self.set(Line { name, ..line });
        }
    }

    /// Takes `path`'s line out, and gives it back.
    pub fn take(&mut self, path: &str) -> Option<Line> {
        let line = self.lines.remove(&key(path));
        if let Some(l) = &line {
            if is_away(l) {
                self.tally(path, false);
            }
            self.touched.insert(key(path), None);
            self.changed = true;
            self.sync();
        }
        line
    }

    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.lines.values()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Writes it if it changed (`write_ledger`, `crate::replace_contents`); with no lines the file is removed. If
    /// another process wrote the file since it was read, its lines are read first and this one's changes go over
    /// them (the file is written whole, so a line it holds that we never touched stays).
    pub fn save(&mut self) -> Result<(), String> {
        if !self.changed {
            return Ok(());
        }
        // ponytail: a write by another process between this read and our write is lost; a lock file if it matters.
        self.refresh();
        if let Some(path) = &self.path {
            let lines: Vec<Line> = self.lines.values().cloned().collect();
            let text = write_ledger(&lines);
            if text.is_empty() {
                match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                    _ => {}
                }
            } else {
                if let Some(folder) = path.parent() {
                    let _ = std::fs::create_dir_all(folder);
                }
                crate::replace_contents(path, text.as_bytes()).map_err(|e| e.to_string())?;
            }
        }
        self.changed = false;
        self.touched.clear();
        self.stamp = self.path.as_deref().and_then(stamp_of);
        Ok(())
    }
}

/// The one ledger of this PC: `marks.jsonl` in `crate::tools::data_dir()`, read the first time it is asked.
pub fn shared() -> &'static Mutex<Ledger> {
    SHARED.get_or_init(|| {
        let mut ledger = Ledger::open(crate::tools::data_dir().map(|d| d.join("marks.jsonl")));
        ledger.flag = Some(&SHARED_EMPTY);
        ledger.away_flag = Some(&SHARED_AWAY);
        ledger.sync();
        // Its own timer: the file is looked at once a second (a `metadata` call), off every thread that lists, and
        // never waits on the lock: a busy second is skipped.
        let _ = std::thread::Builder::new().name("marks-ledger".into()).spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if let Some(Ok(mut l)) = SHARED.get().map(|m| m.try_lock()) {
                l.refresh();
            }
        });
        Mutex::new(ledger)
    })
}

/// The shared ledger, only if something has opened it already (a listing must not read its file).
pub(super) fn shared_if_open() -> Option<&'static Mutex<Ledger>> {
    SHARED.get()
}

/// How many lines of the shared ledger wait for their item, answered without its lock.
pub(super) fn shared_away() -> &'static AtomicUsize {
    &SHARED_AWAY
}

/// Whether the shared ledger holds nothing, answered without its lock.
pub(super) fn shared_empty() -> &'static AtomicBool {
    shared();
    &SHARED_EMPTY
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("glint-ledger-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn red() -> Marks {
        Marks { label: Some("Red".into()), tags: vec!["a".into()], comment: None }
    }

    #[test]
    fn the_ledger_puts_gets_in_any_case_takes_and_reads_back_what_it_saved() {
        let d = dir("roundtrip");
        let file = d.join("marks.jsonl");
        let mut l = Ledger::open(Some(file.clone()));
        assert!(l.is_empty());
        l.put("C:\\Docs\\A.txt", &red(), Why::Shut, 7);
        l.put("C:\\Docs\\b.txt", &red(), Why::Away, 8);
        assert_eq!(l.get("c:\\docs\\a.TXT\\").map(|x| x.marks.clone()), Some(red()));
        assert_eq!(l.take("C:\\DOCS\\b.txt").map(|x| x.at), Some(8));
        assert!(l.get("C:\\docs\\b.txt").is_none());
        l.save().unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("{\"glint-ledger\":1}\n"));
        let again = Ledger::open(Some(file));
        let lines: Vec<&Line> = again.lines().collect();
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0].name.as_str(), lines[0].at, lines[0].why), ("C:\\Docs\\A.txt", 7, Some(Why::Shut)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_emptied_ledger_removes_its_file_and_empty_marks_take_a_line_away() {
        let d = dir("empty");
        let file = d.join("marks.jsonl");
        let mut l = Ledger::open(Some(file.clone()));
        l.put("C:\\a", &red(), Why::Shut, 1);
        l.save().unwrap();
        assert!(file.exists());
        l.put("C:\\a", &Marks::default(), Why::Shut, 2);
        assert!(l.is_empty());
        l.save().unwrap();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_of_rubbish_opens_empty_and_a_ledger_with_no_path_saves_without_error() {
        let d = dir("rubbish");
        let file = d.join("marks.jsonl");
        std::fs::write(&file, b"\xff\xfe not json\n{{{{\n").unwrap();
        assert!(Ledger::open(Some(file)).is_empty());
        let mut l = Ledger::open(None);
        l.put("C:\\a", &red(), Why::Shut, 1);
        assert!(l.save().is_ok());
        assert!(!l.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn put_and_save(file: &Path, who: &mut Ledger, name: &str) {
        who.put(name, &red(), Why::Shut, 1);
        who.save().unwrap();
        let _ = file;
    }

    fn names(l: &Ledger) -> Vec<String> {
        let mut v: Vec<String> = l.lines().map(|x| x.name.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn two_ledgers_on_one_file_keep_each_others_lines_and_read_them_after_a_refresh() {
        let d = dir("two");
        let file = d.join("marks.jsonl");
        let (mut a, mut b) = (Ledger::open(Some(file.clone())), Ledger::open(Some(file.clone())));
        put_and_save(&file, &mut a, "C:\\one");
        put_and_save(&file, &mut b, "C:\\two");
        assert_eq!(names(&Ledger::open(Some(file.clone()))), ["C:\\one", "C:\\two"]);
        assert_eq!(names(&a), ["C:\\one"]);
        assert!(a.refresh());
        assert_eq!(names(&a), ["C:\\one", "C:\\two"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_line_one_took_stays_taken_when_the_other_saves_and_the_last_to_save_wins_a_shared_line() {
        let d = dir("taken");
        let file = d.join("marks.jsonl");
        let mut seed = Ledger::open(Some(file.clone()));
        put_and_save(&file, &mut seed, "C:\\x");
        let (mut a, mut b) = (Ledger::open(Some(file.clone())), Ledger::open(Some(file.clone())));
        a.take("C:\\x");
        a.save().unwrap();
        b.save().unwrap(); // touched nothing: writes nothing
        put_and_save(&file, &mut b, "C:\\y");
        assert_eq!(names(&Ledger::open(Some(file.clone()))), ["C:\\y"]);
        assert_eq!(names(&b), ["C:\\y"]);
        // The same line changed by both: the last to save wins, and the other lines stay.
        let (mut c, mut e) = (Ledger::open(Some(file.clone())), Ledger::open(Some(file.clone())));
        c.put("C:\\y", &Marks { label: Some("Blue".into()), ..Marks::default() }, Why::Shut, 2);
        e.put("C:\\y", &Marks { label: Some("Green".into()), ..Marks::default() }, Why::Shut, 3);
        e.put("C:\\z", &red(), Why::Shut, 3);
        c.save().unwrap();
        e.save().unwrap();
        let end = Ledger::open(Some(file));
        assert_eq!(end.get("C:\\y").and_then(|x| x.marks.label.clone()).as_deref(), Some("Green"));
        assert_eq!(names(&end), ["C:\\y", "C:\\z"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_that_did_not_change_is_not_read_again_and_one_cut_short_keeps_its_whole_lines() {
        let d = dir("reads");
        let file = d.join("marks.jsonl");
        let mut a = Ledger::open(Some(file.clone()));
        put_and_save(&file, &mut a, "C:\\one");
        put_and_save(&file, &mut a, "C:\\two");
        let before = a.reads;
        assert!(!a.refresh() && !a.refresh());
        assert_eq!(a.reads, before, "the file did not change since it was written");
        // Another process cuts the file short in the middle of its last line.
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::write(&file, &text[..text.len() - 12]).unwrap();
        assert!(a.refresh());
        assert_eq!(a.reads, before + 1);
        assert_eq!(a.lines().count(), 1);
        let _ = Ledger::open(Some(file));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_count_of_waiting_lines_is_right_after_put_take_open_and_a_shut_line_is_not_one() {
        let d = dir("away");
        let file = d.join("marks.jsonl");
        let mut l = Ledger::open(Some(file.clone()));
        assert_eq!(l.away(), 0);
        l.put("C:\\Docs\\a.txt", &red(), Why::Away, 1);
        l.put("C:\\Docs\\b.txt", &red(), Why::Shut, 1);
        l.put("C:\\Other\\c.txt", &red(), Why::Away, 1);
        assert_eq!(l.away(), 2);
        // The same item put again as shut is no longer waiting; as away again, once.
        l.put("C:\\Docs\\a.txt", &red(), Why::Shut, 2);
        assert_eq!(l.away(), 1);
        l.put("C:\\DOCS\\A.txt", &red(), Why::Away, 3);
        l.put("c:\\docs\\a.txt", &red(), Why::Away, 3);
        assert_eq!(l.away(), 2);
        l.take("C:\\Other\\c.txt");
        l.take("C:\\Other\\c.txt");
        assert_eq!(l.away(), 1);
        let names: Vec<String> = l.away_in("c:\\docs\\").into_iter().map(|x| x.name).collect();
        assert_eq!(names, ["c:\\docs\\a.txt"]);
        assert!(l.away_in("C:\\Other").is_empty() && l.away_in("C:\\Docs\\sub").is_empty());
        assert!(l.waits_in("C:\\DOCS\\") && !l.waits_in("C:\\Other") && !l.waits_in("C:\\"));
        l.save().unwrap();
        assert_eq!(Ledger::open(Some(file)).away(), 1);
        let mut shared = Ledger::open(None);
        let count: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(9)));
        shared.away_flag = Some(count);
        shared.put("C:\\a", &red(), Why::Away, 1);
        assert_eq!(count.load(Ordering::Relaxed), 1);
        shared.take("C:\\a");
        assert_eq!(count.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(&d);
    }
}
