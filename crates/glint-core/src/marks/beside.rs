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
