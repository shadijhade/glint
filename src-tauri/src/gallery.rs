//! Gallery, a place Glint opens like a folder (`shell:Gallery`): the photos and videos in the gallery's
//! folders (Pictures and Videos, and the ones added in Settings), every one below them, newest first by
//! when each was taken. Instant search's index lists them at once where it covers the folders; else
//! they're walked. When each was taken comes from what it says (a photo's EXIF, a video's recording
//! date), read once and kept by path, size and date (gallery-dates.json, with Glint's data), so the
//! gallery opens at once the next time; while new ones are read, in the background, they're dated
//! when they were modified, and the app is told once they're done ("gallery-dated").

use crate::fsx::{self, Entry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Instant;
use tauri::{AppHandle, Emitter};

/// Gallery's path in Glint.
pub const PLACE: &str = "shell:Gallery";

/// The most it lists.
const MOST: usize = 200_000;

/// What a photo or a video is, by its extension: the pictures cameras and phones make, and videos.
const KINDS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "heic", "heif", "avif", "png", "tif", "tiff", "webp", "gif", "bmp", "jxl", "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef",
    "iiq", "srw", "mp4", "m4v", "mov", "3gp", "3g2", "mkv", "webm", "avi", "wmv", "mpg", "mpeg", "m2ts", "mts",
];

pub fn is_place(path: &str) -> bool {
    path.eq_ignore_ascii_case(PLACE)
}

fn is_media(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| KINDS.iter().any(|k| k.eq_ignore_ascii_case(ext)))
}

#[derive(Default)]
struct State {
    folders: Vec<String>,
    app: Option<AppHandle>,
    /// When each file was taken (0: it doesn't say), by its path in lower case, with the size and date
    /// it had when it was read.
    dates: Option<HashMap<String, (u64, i64, i64)>>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
/// Files being read for when they were taken, in the background.
static DATING: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn dates_file() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("gallery-dates.json"))
}

#[derive(Serialize, Deserialize)]
struct Saved(Vec<(String, u64, i64, i64)>);

fn load_dates() -> HashMap<String, (u64, i64, i64)> {
    let saved: Option<Saved> = dates_file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok());
    saved.map(|s| s.0.into_iter().map(|(path, size, modified, taken)| (path, (size, modified, taken))).collect()).unwrap_or_default()
}

fn save_dates(dates: &HashMap<String, (u64, i64, i64)>) {
    let saved = Saved(dates.iter().map(|(path, &(size, modified, taken))| (path.clone(), size, modified, taken)).collect());
    if let (Some(file), Ok(text)) = (dates_file(), serde_json::to_vec(&saved)) {
        let _ = std::fs::write(file, text);
    }
}

/// The gallery's folders, as the app has them (Pictures, Videos and the ones added in Settings).
#[tauri::command]
pub fn gallery_set_folders(app: AppHandle, folders: Vec<String>) {
    let mut state = state();
    state.app = Some(app);
    state.folders = folders;
}

/// The photos and videos below the folders: from the index, where it covers them all, else walked.
fn files(folders: &[String]) -> Vec<Entry> {
    let mut found = from_index(folders).unwrap_or_else(|| walk(folders));
    // A folder inside another one is listed once.
    let mut seen = std::collections::HashSet::new();
    found.retain(|e| e.path.as_ref().is_some_and(|p| seen.insert(p.to_lowercase())));
    found.truncate(MOST);
    found
}

#[cfg(windows)]
fn from_index(folders: &[String]) -> Option<Vec<Entry>> {
    use shadui_index::client::{self, Event};
    let query = format!("ext:{}", KINDS.join(";"));
    let request = shadui_index::proto::SearchRequest { query, roots: folders.to_vec(), hidden: false, exclude: Vec::new(), max_results: Some(MOST), fuzzy: false, rank: false };
    let mut found = Vec::new();
    let mut whole = true;
    client::search(&request, |event| {
        match event {
            // Every folder the index covers, or it's walked instead.
            Event::Plan { skipped, .. } => whole = skipped.is_empty(),
            Event::Hits(hits) => found.extend(hits.into_iter().filter(|h| !h.dir).map(|h| {
                let name = h.path.rsplit('\\').next().unwrap_or(&h.path).to_string();
                Entry { name, path: Some(h.path), dir: false, size: h.size, modified: h.modified, attrs: h.attrs }
            })),
            Event::Done { .. } => {}
        }
        whole
    })
    .ok()?;
    whole.then_some(found)
}

#[cfg(not(windows))]
fn from_index(_folders: &[String]) -> Option<Vec<Entry>> {
    None
}

/// Every photo and video below the folders, walked (hidden and system folders, and links, left out).
fn walk(folders: &[String]) -> Vec<Entry> {
    let mut found = Vec::new();
    let mut stack: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
    while let Some(dir) = stack.pop() {
        for entry in fsx::read_dir(&dir.to_string_lossy()).unwrap_or_default() {
            let path = dir.join(&entry.name);
            if entry.dir {
                if entry.attrs & 0x406 == 0 {
                    stack.push(path);
                }
            } else if entry.attrs & 0x6 == 0 && is_media(&entry.name) {
                found.push(Entry { path: Some(path.to_string_lossy().into_owned()), ..entry });
                if found.len() >= MOST {
                    return found;
                }
            }
        }
    }
    found
}

/// Gallery as a listing: each photo and video at its own path, its date when it was taken where it says
/// (else when it was modified), newest first. Files not dated yet are read in the background.
pub fn listing(started: Instant) -> Result<Vec<u8>, String> {
    let folders: Vec<String> = state().folders.iter().filter(|f| Path::new(f).is_dir()).cloned().collect();
    let mut entries = files(&folders);
    let mut unread = Vec::new();
    {
        let mut state = state();
        let dates = state.dates.get_or_insert_with(load_dates);
        for e in &mut entries {
            let key = e.path.as_deref().unwrap_or_default().to_lowercase();
            match dates.get(&key) {
                Some(&(size, modified, taken)) if size == e.size && modified == e.modified => {
                    if taken > 0 {
                        e.modified = taken;
                    }
                }
                _ => unread.push((key, e.path.clone().unwrap_or_default(), e.size, e.modified)),
            }
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.modified));
    if !unread.is_empty() && !DATING.swap(true, Ordering::SeqCst) {
        std::thread::spawn(move || date(unread));
    }
    Ok(fsx::encode_items(PLACE, &entries, &[], fsx::ms_since(started), false))
}

/// Reads when each file was taken, four at a time, keeps it, and tells the app.
fn date(unread: Vec<(String, String, u64, i64)>) {
    let read = std::sync::Mutex::new(Vec::with_capacity(unread.len()));
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((key, path, size, modified)) = unread.get(i) else { break };
                let taken = crate::media::taken_of(path).unwrap_or(0);
                read.lock().unwrap_or_else(|e| e.into_inner()).push((key.clone(), (*size, *modified, taken)));
            });
        }
    });
    let app = {
        let mut state = state();
        let dates = state.dates.get_or_insert_with(load_dates);
        dates.extend(read.into_inner().unwrap_or_else(|e| e.into_inner()));
        save_dates(dates);
        state.app.clone()
    };
    DATING.store(false, Ordering::SeqCst);
    if let Some(app) = app {
        let _ = app.emit("gallery-dated", unread.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_photos_and_videos() {
        assert!(is_place("shell:gallery") && !is_place(r"C:\Gallery"));
        assert!(is_media("IMG_0001.JPG") && is_media("clip.mov") && is_media("raw.cr3"));
        assert!(!is_media("notes.txt") && !is_media("icon.ico") && !is_media("noext"));
    }

    #[test]
    fn walks_the_folders() {
        let dir = std::env::temp_dir().join(format!("glint-gallery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("2024").join("trip")).unwrap();
        std::fs::write(dir.join("a.jpg"), "x").unwrap();
        std::fs::write(dir.join("2024").join("trip").join("b.MP4"), "x").unwrap();
        std::fs::write(dir.join("2024").join("notes.txt"), "x").unwrap();
        let mut names: Vec<String> = walk(&[dir.to_string_lossy().into_owned()]).into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(names, ["a.jpg", "b.MP4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
