//! The last home: the ledger on this PC, `marks.jsonl` in Glint's data folder (PLAN.md 5.3), for items whose
//! streams and folder can't be written, and an import's items that weren't there. Held in memory by path in
//! lower case, written at the end of the call that changed it (a call is a moment).

use glint_model::marks::lines::LEDGER_MAX_BYTES;
use glint_model::marks::{read_ledger, write_ledger, Line, Marks, Why};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// True while the shared ledger holds no line: readers then don't lock it at all.
static SHARED_EMPTY: AtomicBool = AtomicBool::new(false);

/// The marks kept on this PC, by path.
pub struct Ledger {
    /// Where it is kept; None: in memory only.
    path: Option<PathBuf>,
    /// By the path without its end separator, in lower case.
    lines: HashMap<String, Line>,
    changed: bool,
    /// Told whether the lines are none (the shared one only).
    flag: Option<&'static AtomicBool>,
}

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

impl Ledger {
    /// Reads the file (at most `LEDGER_MAX_BYTES`, `read_ledger`). No file, or one that doesn't read: empty.
    pub fn open(path: Option<PathBuf>) -> Ledger {
        let mut ledger = Ledger { path, lines: HashMap::new(), changed: false, flag: None };
        let mut bytes = Vec::new();
        let read = ledger.path.as_ref().and_then(|p| std::fs::File::open(p).ok()).map(|f| {
            f.take(LEDGER_MAX_BYTES as u64 + 1).read_to_end(&mut bytes).is_ok()
        });
        if read == Some(true) {
            for line in read_ledger(&String::from_utf8_lossy(&bytes)) {
                ledger.lines.insert(key(&line.name), line);
            }
        }
        ledger
    }

    fn sync(&self) {
        if let Some(flag) = self.flag {
            flag.store(self.lines.is_empty(), Ordering::Relaxed);
        }
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
            self.lines.insert(k, line);
            self.changed = true;
            self.sync();
        }
    }

    /// Takes `path`'s line out, and gives it back.
    pub fn take(&mut self, path: &str) -> Option<Line> {
        let line = self.lines.remove(&key(path));
        if line.is_some() {
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
    static SHARED: OnceLock<Mutex<Ledger>> = OnceLock::new();
    SHARED.get_or_init(|| {
        let mut ledger = Ledger::open(crate::tools::data_dir().map(|d| d.join("marks.jsonl")));
        ledger.flag = Some(&SHARED_EMPTY);
        ledger.sync();
        Mutex::new(ledger)
    })
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
}
