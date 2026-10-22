//! Compare panes, all the way down: how each item in one folder compares with its namesake in the
//! other, counting what's inside subfolders, and by contents if asked; and what Sync copies each
//! way (what's missing over there, and files newer here), into the folder each goes in.

use crate::fsx::{self, blocking};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const HIDDEN: u32 = 0x2;
const REPARSE_POINT: u32 = 0x400;
/// FAT and archives round times to 2 seconds.
const SAME_TIME_MS: i64 = 2000;
/// Past this many files it stops: the panes keep the top-level comparison.
const MAX_FILES: usize = 1_000_000;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    pub path: String,
    pub folder: String,
}

#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FolderDiff {
    /// Each item at the top of the left folder (by lowercase name): unique, newer, older, differ or same.
    pub left: HashMap<String, &'static str>,
    pub right: HashMap<String, &'static str>,
    /// What Sync copies to the right (and to the left), each into the folder it goes in.
    pub to_right: Vec<Copy>,
    pub to_left: Vec<Copy>,
    pub files: usize,
    pub elapsed_ms: f64,
}

/// Each comparison's number: a newer one, or closing compare panes, stops the one running.
static LATEST: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub fn cancel_compare() {
    LATEST.fetch_add(1, Ordering::Relaxed);
}

#[tauri::command]
pub async fn compare_folders(left: String, right: String, subfolders: bool, contents: bool, hidden: bool, exclude: Vec<String>) -> Result<FolderDiff, String> {
    let mine = LATEST.fetch_add(1, Ordering::Relaxed) + 1;
    blocking(move || {
        let stop = move || LATEST.load(Ordering::Relaxed) != mine;
        let options = Options { subfolders, contents, hidden, exclude: exclude.iter().map(|e| e.to_lowercase()).collect(), stop: &stop };
        compare(&left, &right, &options)
    })
    .await
}

pub struct Options<'a> {
    pub subfolders: bool,
    pub contents: bool,
    pub hidden: bool,
    /// Folder and file names to leave out (lowercase), as Find files skips them.
    pub exclude: Vec<String>,
    /// True once it should stop.
    pub stop: &'a (dyn Fn() -> bool + Sync),
}

pub fn compare(left: &str, right: &str, options: &Options) -> Result<FolderDiff, String> {
    let started = Instant::now();
    let mut diff = FolderDiff::default();
    let mut top = Top::default();
    compare_dirs(left, right, options, &mut diff, Some(&mut top))?;
    (diff.left, diff.right) = (top.left, top.right);
    diff.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    Ok(diff)
}

#[derive(Default)]
struct Top {
    left: HashMap<String, &'static str>,
    right: HashMap<String, &'static str>,
}

/// Compares two folders' items; true when they hold the same things. At the top (`top`), it says how
/// each item compares, for the panes' rows.
fn compare_dirs(left: &str, right: &str, options: &Options, diff: &mut FolderDiff, mut top: Option<&mut Top>) -> Result<bool, String> {
    let list = |dir: &str| -> Result<HashMap<String, fsx::Entry>, String> {
        let entries = fsx::read_dir(dir).map_err(|e| format!("{dir}: {e}"))?;
        Ok(entries
            .into_iter()
            .filter(|e| options.hidden || e.attrs & HIDDEN == 0)
            .filter(|e| !options.exclude.contains(&e.name.to_lowercase()))
            .map(|e| (e.name.to_lowercase(), e))
            .collect())
    };
    let (ours, theirs) = (list(left)?, list(right)?);
    let join = |dir: &str, name: &str| Path::new(dir).join(name).to_string_lossy().into_owned();
    let mut same = true;
    for (key, a) in &ours {
        if (options.stop)() {
            return Err("cancelled".into());
        }
        let Some(b) = theirs.get(key) else {
            same = false;
            diff.to_right.push(Copy { path: join(left, &a.name), folder: right.into() });
            if let Some(top) = top.as_deref_mut() {
                top.left.insert(key.clone(), "unique");
            }
            continue;
        };
        let (status, other) = if a.dir != b.dir {
            // A file and a folder of one name: Sync leaves them be.
            ("differ", "differ")
        } else if a.dir {
            // A link to a folder isn't followed, as it may lead anywhere (back here, say).
            let linked = (a.attrs | b.attrs) & REPARSE_POINT != 0;
            if !options.subfolders || linked {
                continue;
            }
            if compare_dirs(&join(left, &a.name), &join(right, &b.name), options, diff, None)? {
                ("same", "same")
            } else {
                ("differ", "differ")
            }
        } else {
            diff.files += 1;
            if diff.files > MAX_FILES {
                return Err(format!("stopped after {MAX_FILES} files"));
            }
            compare_files(&join(left, &a.name), a, &join(right, &b.name), b, options.contents)
        };
        if status == "newer" {
            diff.to_right.push(Copy { path: join(left, &a.name), folder: right.into() });
        } else if status == "older" {
            diff.to_left.push(Copy { path: join(right, &b.name), folder: left.into() });
        }
        same &= status == "same";
        if let Some(top) = top.as_deref_mut() {
            top.left.insert(key.clone(), status);
            top.right.insert(key.clone(), other);
        }
    }
    for (key, b) in &theirs {
        if !ours.contains_key(key) {
            same = false;
            diff.to_left.push(Copy { path: join(right, &b.name), folder: left.into() });
            if let Some(top) = top.as_deref_mut() {
                top.right.insert(key.clone(), "unique");
            }
        }
    }
    Ok(same)
}

/// How the left file compares with the right one, and the right with the left: by date first (the
/// newer one is synced), unless the contents are the same.
fn compare_files(left: &str, a: &fsx::Entry, right: &str, b: &fsx::Entry, contents: bool) -> (&'static str, &'static str) {
    let same_bytes = || a.size == b.size && same_contents(left, right).unwrap_or(false);
    if a.modified > 0 && b.modified > 0 && (a.modified - b.modified).abs() > SAME_TIME_MS {
        if contents && same_bytes() {
            return ("same", "same");
        }
        return if a.modified > b.modified { ("newer", "older") } else { ("older", "newer") };
    }
    if a.size != b.size || (contents && !same_bytes()) {
        return ("differ", "differ");
    }
    ("same", "same")
}

/// Reads both until they differ.
fn same_contents(left: &str, right: &str) -> std::io::Result<bool> {
    let (mut a, mut b) = (File::open(left)?, File::open(right)?);
    let (mut x, mut y) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = read_full(&mut a, &mut x)?;
        let m = read_full(&mut b, &mut y)?;
        if n != m || x[..n] != y[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_down_through_subfolders_and_by_contents() {
        let root = std::env::temp_dir().join(format!("glint-compare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (l, r) = (root.join("left"), root.join("right"));
        for dir in [l.join(r"sub\deep"), r.join(r"sub\deep"), l.join("same")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(r.join("same")).unwrap();
        let write = |p: &Path, text: &str| std::fs::write(p, text).unwrap();
        write(&l.join("only-left.txt"), "a");
        write(&r.join("only-right.txt"), "b");
        write(&l.join(r"same\x.txt"), "x");
        write(&r.join(r"same\x.txt"), "x");
        // Deep inside, the left has a file the right hasn't: the folder differs, and Sync copies just that file.
        write(&l.join(r"sub\deep\new.txt"), "n");
        // Same size and time, other bytes: only a content check sees it.
        write(&l.join("twin.txt"), "aaaa");
        write(&r.join("twin.txt"), "bbbb");
        let when = std::fs::metadata(l.join("twin.txt")).unwrap().modified().unwrap();
        File::options().write(true).open(r.join("twin.txt")).unwrap().set_modified(when).unwrap();

        let (l, r) = (l.to_string_lossy().into_owned(), r.to_string_lossy().into_owned());
        let options = |contents| Options { subfolders: true, contents, hidden: true, exclude: Vec::new(), stop: &|| false };
        let diff = compare(&l, &r, &options(false)).unwrap();
        assert_eq!(diff.left["only-left.txt"], "unique");
        assert_eq!(diff.right["only-right.txt"], "unique");
        assert_eq!(diff.left["same"], "same");
        assert_eq!(diff.left["sub"], "differ");
        assert_eq!(diff.left["twin.txt"], "same");
        let copied: Vec<&str> = diff.to_right.iter().map(|c| c.path.rsplit('\\').next().unwrap()).collect();
        assert!(copied.contains(&"only-left.txt") && copied.contains(&"new.txt") && !copied.contains(&"sub"));
        let new = diff.to_right.iter().find(|c| c.path.ends_with("new.txt")).unwrap();
        assert!(new.folder.ends_with(r"right\sub\deep"));
        assert_eq!(diff.to_left.len(), 1);

        assert_eq!(compare(&l, &r, &options(true)).unwrap().left["twin.txt"], "differ");
        // Without subfolders, folders are left to the panes.
        let shallow = compare(&l, &r, &Options { subfolders: false, ..options(false) }).unwrap();
        assert!(!shallow.left.contains_key("sub") && shallow.to_right.len() == 1);
        let _ = std::fs::remove_dir_all(&root);
    }
}
