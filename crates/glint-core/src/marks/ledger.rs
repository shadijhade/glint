//! The last home: the ledger on this PC, `marks.jsonl` in Glint's data folder (PLAN.md 5.3), for items whose
//! streams and folder can't be written, and an import's items that weren't there. Held in memory by path in
//! lower case, written at the end of the call that changed it (a call is a moment).

use glint_model::marks::lines::LEDGER_MAX_BYTES;
use glint_model::marks::{read_ledger, write_ledger, Line, Marks, Why};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

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
        let mut ledger =
            Ledger { path, lines: HashMap::new(), changed: false, flag: None, away: 0, away_dirs: HashMap::new(), away_flag: None };
        let mut bytes = Vec::new();
        let read = ledger.path.as_ref().and_then(|p| std::fs::File::open(p).ok()).map(|f| {
            f.take(LEDGER_MAX_BYTES as u64 + 1).read_to_end(&mut bytes).is_ok()
        });
        if read == Some(true) {
            for line in read_ledger(&String::from_utf8_lossy(&bytes)) {
                ledger.lines.insert(key(&line.name), line);
            }
            let waiting: Vec<String> = ledger.lines.values().filter(|l| is_away(l)).map(|l| l.name.clone()).collect();
            for name in waiting {
                ledger.tally(&name, true);
            }
        }
        ledger
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
        if self.lines.get(&k) != Some(&line) {
            let (was, now) = (self.lines.get(&k).is_some_and(is_away), is_away(&line));
            if was != now {
                self.tally(path, now);
            }
            self.lines.insert(k, line);
            self.changed = true;
            self.sync();
        }
    }

    /// Takes `path`'s line out, and gives it back.
    pub fn take(&mut self, path: &str) -> Option<Line> {
        let line = self.lines.remove(&key(path));
        if let Some(l) = &line {
            if is_away(l) {
                self.tally(path, false);
            }
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

    /// Writes it if it changed (`write_ledger`, `crate::replace_contents`); with no lines the file is removed.
    pub fn save(&mut self) -> Result<(), String> {
        if !self.changed {
            return Ok(());
        }
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
