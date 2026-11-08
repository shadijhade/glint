//! Tags kept in the file itself: an NTFS (or ReFS) stream of its own, `file:glint.tags`, one tag per line.
//! They go wherever the file goes on such a drive (a move, a rename, a copy by Windows' file engine),
//! and the instant search service reads them in the same pass as the names: a small stream sits in
//! the file's own MFT record, so `tag:work` searches every drive in milliseconds. Tagging leaves a
//! file's dates as they were. FAT and exFAT drives have no streams, so nothing there can be tagged.

use crate::fsx::blocking;
use std::fs;
use std::io::{Read, Write};

/// The stream's name, after the file's: `C:\notes.txt:glint.tags`.
const STREAM: &str = "glint.tags";
/// More is never read from a stream that says it holds tags.
const MAX_BYTES: u64 = 4096;

fn stream_of(path: &str) -> String {
    format!("{}:{STREAM}", path.trim_end_matches(['\\', '/']))
}

/// A file's tags (none when it has no tags stream, or it can't be read).
pub fn read(path: &str) -> Vec<String> {
    let Ok(file) = fs::File::open(stream_of(path)) else { return Vec::new() };
    let mut text = String::new();
    if file.take(MAX_BYTES).read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    parse(&text)
}

fn parse(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in text.lines().map(str::trim).filter(|t| !t.is_empty()) {
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// Whether `path` has, for each group of `wanted`, one of its tags (any case): `tag:` filters.
pub fn matches(path: &str, wanted: &[Vec<String>]) -> bool {
    let tags = read(path);
    wanted.iter().all(|any| tags.iter().any(|t| any.iter().any(|w| t.eq_ignore_ascii_case(w))))
}

/// Writes `tags` as `path`'s (none: the stream goes), with the file's dates kept as they were.
fn write(path: &str, tags: &[String]) -> Result<(), String> {
    #[cfg(windows)]
    let times = keep_times(path);
    let result = if tags.is_empty() {
        match fs::remove_file(stream_of(path)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    } else {
        fs::File::create(stream_of(path)).and_then(|mut f| f.write_all(format!("{}\n", tags.join("\n")).as_bytes())).map_err(|e| e.to_string())
    };
    #[cfg(windows)]
    if let Some(restore) = times {
        restore();
    }
    result
}

/// The file's dates now, and a way to put them back (writing a stream moves "Date modified").
#[cfg(windows)]
fn keep_times(path: &str) -> Option<impl FnOnce()> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::{FILETIME, HANDLE};
    use windows::Win32::Storage::FileSystem::{GetFileTime, SetFileTime, FILE_FLAG_BACKUP_SEMANTICS};
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
    let file = fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(path).ok()?;
    let (mut created, mut accessed, mut written) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetFileTime(HANDLE(file.as_raw_handle()), Some(&mut created), Some(&mut accessed), Some(&mut written)) }.ok()?;
    Some(move || unsafe {
        let _ = SetFileTime(HANDLE(file.as_raw_handle()), Some(&created), Some(&accessed), Some(&written));
    })
}

/// Each path's tags, for the rows on screen.
#[tauri::command]
pub async fn tags_read(paths: Vec<String>) -> Result<Vec<Vec<String>>, String> {
    blocking(move || Ok(paths.iter().map(|p| read(p)).collect())).await
}

/// Adds `add` to each path's tags and takes `remove` off (any case). Returns the paths that couldn't
/// be tagged, each with why.
#[tauri::command]
pub async fn tags_set(paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Result<Vec<(String, String)>, String> {
    blocking(move || {
        let add: Vec<String> = add.into_iter().map(|t| t.trim().replace(['\n', '\r'], " ")).filter(|t| !t.is_empty()).collect();
        let mut failed = Vec::new();
        for path in paths {
            let mut tags = read(&path);
            tags.retain(|t| !remove.iter().any(|r| r.eq_ignore_ascii_case(t)));
            for tag in &add {
                if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                    tags.push(tag.clone());
                }
            }
            if let Err(e) = write(&path, &tags) {
                let reason = if e.contains("123") || e.to_lowercase().contains("syntax") { "This drive keeps no tags (FAT or exFAT)".to_string() } else { e };
                failed.push((path, reason));
            }
        }
        Ok(failed)
    })
    .await
}

/// Every tag on the drives instant search covers, with how many items have it, most used first; Err
/// when instant search is off (or from before tags).
#[tauri::command]
pub async fn tags_all() -> Result<Vec<(String, u64)>, String> {
    #[cfg(windows)]
    {
        blocking(|| shadui_index::client::tags().map_err(|e| e.to_string())).await
    }
    #[cfg(not(windows))]
    {
        Err("Instant search needs Windows".into())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn tags_live_in_the_file_and_leave_its_dates() {
        let dir = std::env::temp_dir().join(format!("glint-tags-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("report.pdf");
        fs::write(&file, b"%PDF").unwrap();
        let path = file.to_string_lossy().into_owned();
        let before = fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&path, &["Work".into(), "Red".into()]).unwrap();
        assert_eq!(read(&path), ["Work", "Red"]);
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), before, "Date modified stays");
        assert!(matches(&path, &[vec!["red".into()]]));
        assert!(matches(&path, &[vec!["blue".into(), "work".into()], vec!["RED".into()]]));
        assert!(!matches(&path, &[vec!["blue".into()]]));
        // The contents are untouched, and a folder takes tags too.
        assert_eq!(fs::read(&file).unwrap(), b"%PDF");
        let folder = dir.to_string_lossy().into_owned();
        write(&folder, &["Projects".into()]).unwrap();
        assert_eq!(read(&folder), ["Projects"]);
        write(&path, &[]).unwrap();
        assert!(read(&path).is_empty());
        assert_eq!(parse("a\n\nA\n b \n"), ["a", "b"]);
        let _ = fs::remove_dir_all(&dir);
    }
}
