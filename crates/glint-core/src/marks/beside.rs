//! The second home: `.glint-marks`, a hidden JSON Lines file in the item's folder, for drives without streams
//! (PLAN.md 5.2, F19). Written whole through `replace_contents`; read on any drive (F23), capped (M10).

use glint_model::marks::lines::BESIDE_MAX_BYTES;
use glint_model::marks::{read_beside, write_beside, Beside, Line, Marks, BESIDE_NAME};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

/// Writes of one process don't interleave: held from the read to the write.
static WRITING: Mutex<()> = Mutex::new(());

/// The folder's `.glint-marks` as it reads; None when there is none (or it can't be opened).
pub fn read(folder: &Path) -> Option<Beside> {
    let mut bytes = Vec::new();
    let file = std::fs::File::open(folder.join(BESIDE_NAME)).ok()?;
    file.take(BESIDE_MAX_BYTES as u64 + 1).read_to_end(&mut bytes).ok()?;
    Some(read_beside(&String::from_utf8_lossy(&bytes)))
}

fn same_name(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

/// The line for the item named `name` (names alike in `to_lowercase()` are one).
// ponytail: a scan, so a batch over a folder of 100,000 lines costs a scan an item; a map when that is measured.
pub fn line<'a>(beside: &'a Beside, name: &str) -> Option<&'a Line> {
    beside.lines.iter().find(|l| same_name(&l.name, name))
}

/// Gives each named item of `folder` these marks, in one read and one write: empty marks take the line away.
pub fn change(folder: &Path, changes: &[(String, Marks)], at: i64) -> Result<(), String> {
    if !folder.is_dir() {
        return Err("This folder can't keep marks beside its items".into());
    }
    let _held = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let was = read(folder).unwrap_or_default();
    let mut now = was.clone();
    for (name, marks) in changes {
        let at_line = now.lines.iter().position(|l| same_name(&l.name, name));
        match (at_line, marks.is_empty()) {
            (Some(i), true) => {
                now.lines.remove(i);
            }
            (Some(i), false) => {
                now.lines[i].marks = marks.clone();
                now.lines[i].at = at;
            }
            (None, true) => {}
            (None, false) => now.lines.push(Line { name: name.clone(), marks: marks.clone(), at, ..Line::default() }),
        }
    }
    now.lines.retain(|l| std::fs::symlink_metadata(folder.join(&l.name)).is_ok());
    if now == was {
        return Ok(());
    }
    let path = folder.join(BESIDE_NAME);
    let text = write_beside(&now);
    if text.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        };
    }
    crate::replace_contents(&path, text.as_bytes()).map_err(|e| e.to_string())?;
    crate::actions::hide(&path);
    Ok(())
}

/// How many items of `folder` have their marks in its `.glint-marks` though the drive keeps streams (F23): what
/// the pane's head offers to move into the files. 0 where there is no such file or the drive keeps none.
pub fn waiting_in(folder: &str) -> u64 {
    waiting(Path::new(folder)).len() as u64
}

/// The lines of `folder`'s `.glint-marks` that wait to move: an item that is there, holds marks, and whose own
/// streams hold none (a stream wins over a line: that line is stale, not waiting). Empty where the drive keeps no
/// streams. M10: names were refused by `read` when they leave the folder; the path is only `folder` and a name.
fn waiting(folder: &Path) -> Vec<(String, Marks)> {
    if !super::caps::has_streams(&folder.to_string_lossy()) {
        return Vec::new();
    }
    let Some(side) = read(folder) else { return Vec::new() };
    side.lines
        .into_iter()
        .filter(|l| !l.marks.is_empty())
        .filter(|l| {
            let path = folder.join(&l.name);
            std::fs::symlink_metadata(&path).is_ok() && super::streams::read(&path.to_string_lossy()).is_none()
        })
        .map(|l| (l.name, l.marks))
        .collect()
}

/// Moves those marks into the files' own streams, each line taken out as its stream is written; how many moved.
///
/// An item that can't be written (read-only, in use) keeps its line and isn't counted. A line also holds keys a
/// later version wrote (`Line::rest`): they leave with the line, since the streams have no place for them.
pub fn move_in(folder: &str) -> Result<u64, String> {
    let folder = Path::new(folder);
    let mut moved: Vec<(String, Marks)> = Vec::new();
    for (name, marks) in waiting(folder) {
        if super::streams::write(&folder.join(&name).to_string_lossy(), &marks).is_ok() {
            moved.push((name, Marks::default()));
        }
    }
    if moved.is_empty() {
        return Ok(0);
    }
    // One write of the sidecar for all of them; it goes when no line is left.
    change(folder, &moved, 0)?;
    Ok(moved.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marks::{caps, streams};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn folder() -> std::path::PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("glint-beside-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::create_dir_all(&d);
        d
    }

    fn marks(label: &str) -> Marks {
        Marks { label: Some(label.into()), tags: vec!["x".into(), "y".into()], comment: Some("note".into()) }
    }

    fn path(dir: &Path, name: &str) -> String {
        dir.join(name).to_string_lossy().into_owned()
    }

    /// `a.txt` waits, `b.txt` has marks in its streams already (its line is stale), `gone.txt` is gone.
    fn three_lines(dir: &Path) {
        for n in ["a.txt", "b.txt", "gone.txt"] {
            std::fs::write(dir.join(n), b"hello").unwrap();
        }
        let all: Vec<(String, Marks)> = ["a.txt", "b.txt", "gone.txt"].iter().map(|n| (n.to_string(), marks(n))).collect();
        change(dir, &all, 5).unwrap();
        streams::write(&path(dir, "b.txt"), &marks("Own")).unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
    }

    fn names(dir: &Path) -> Vec<String> {
        read(dir).map(|b| b.lines.into_iter().map(|l| l.name).collect()).unwrap_or_default()
    }

    #[test]
    fn a_line_waits_only_when_its_item_is_there_and_its_streams_hold_nothing() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        three_lines(&dir);
        assert_eq!(waiting_in(&dir.to_string_lossy()), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moving_in_writes_the_streams_keeps_dates_and_contents_and_takes_only_that_line_out() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        three_lines(&dir);
        let a = path(&dir, "a.txt");
        let before = std::fs::metadata(&a).unwrap().modified().unwrap();
        assert_eq!(move_in(&dir.to_string_lossy()), Ok(1));
        assert_eq!(streams::read(&a), Some(marks("a.txt")));
        assert_eq!(std::fs::metadata(&a).unwrap().modified().unwrap(), before);
        assert_eq!(std::fs::read(&a).unwrap(), b"hello");
        assert_eq!(streams::read(&path(&dir, "b.txt")), Some(marks("Own")));
        assert_eq!(names(&dir), vec!["b.txt".to_string()], "the stale line stays, the gone one goes with change");
        assert_eq!(waiting_in(&dir.to_string_lossy()), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn when_every_line_moved_the_sidecar_is_gone() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        change(&dir, &[("a.txt".into(), marks("A"))], 5).unwrap();
        assert!(dir.join(BESIDE_NAME).exists());
        assert_eq!(move_in(&dir.to_string_lossy()), Ok(1));
        assert!(!dir.join(BESIDE_NAME).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_read_only_file_keeps_its_line_and_is_not_counted_and_the_others_still_move() {
        let dir = folder();
        if !caps::has_streams(&dir.to_string_lossy()) {
            return;
        }
        for n in ["a.txt", "b.txt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        change(&dir, &[("a.txt".into(), marks("A")), ("b.txt".into(), marks("B"))], 5).unwrap();
        let b = dir.join("b.txt");
        let mut perms = std::fs::metadata(&b).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&b, perms.clone()).unwrap();
        let answer = move_in(&dir.to_string_lossy());
        #[allow(clippy::permissions_set_readonly_false)] // a test's own file, so it can be removed
        perms.set_readonly(false);
        std::fs::set_permissions(&b, perms).unwrap();
        assert_eq!(answer, Ok(1));
        assert_eq!(streams::read(&path(&dir, "a.txt")), Some(marks("A")));
        assert_eq!(streams::read(&path(&dir, "b.txt")), None);
        assert_eq!(names(&dir), vec!["b.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_with_no_sidecar_has_nothing_waiting_and_move_in_writes_nothing() {
        let dir = folder();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(waiting_in(&dir.to_string_lossy()), 0);
        assert_eq!(move_in(&dir.to_string_lossy()), Ok(0));
        assert!(!dir.join(BESIDE_NAME).exists());
        assert_eq!(streams::read(&path(&dir, "a.txt")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
