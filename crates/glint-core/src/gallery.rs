//! Gallery, a place Glint opens like a folder (`shell:Gallery`): the photos and videos in the gallery's
//! folders (Pictures and Videos, and the ones added in Settings), every one below them, newest first by
//! when each was taken. Instant search's index lists them at once where it covers the folders; else
//! they're walked. When each was taken comes from what it says (a photo's EXIF, a video's recording
//! date), read once and kept by path, size and date (gallery-dates.json, with Glint's data), so the
//! gallery opens at once the next time; while new ones are read, in the background, they're dated
//! when they were modified, and the app is told once they're done ("gallery-dated").

use crate::fsx::{self, Entry};
use crate::host::{AppHandle, Event};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Instant;

/// Gallery's path in Glint.
pub const PLACE: &str = "shell:Gallery";

/// The most it lists.
const MOST: usize = 200_000;

/// What a photo or a video is, by its extension: the pictures cameras and phones make, and videos.
const KINDS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "heic", "heif", "avif", "png", "tif", "tiff", "webp", "gif", "bmp", "jxl", "dng",
    "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "iiq", "srw", "mp4", "m4v", "mov",
    "3gp", "3g2", "mkv", "webm", "avi", "wmv", "mpg", "mpeg", "m2ts", "mts",
];

pub fn is_place(path: &str) -> bool {
    path.eq_ignore_ascii_case(PLACE)
}

fn is_media(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| KINDS.iter().any(|k| k.eq_ignore_ascii_case(ext)))
}

type GeoCoords = Option<[f64; 2]>;
type GeoRecord = (u64, i64, i64, GeoCoords);
type GeoRow = (String, u64, i64, i64, GeoCoords);

#[derive(Default)]
struct State {
    folders: Vec<String>,
    app: Option<AppHandle>,
    /// When each file was taken (0: it doesn't say), by its path in lower case, with the size and date
    /// it had when it was read.
    dates: Option<HashMap<String, (u64, i64, i64)>>,
    /// Cached `(size, modified, taken, Option<[lat, lon]>)` keyed by lowercase path.
    geo: Option<HashMap<String, GeoRecord>>,
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

fn geo_file() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("gallery-geo.json"))
}

#[derive(Serialize, Deserialize)]
struct Saved(Vec<(String, u64, i64, i64)>);

#[derive(Serialize, Deserialize)]
struct SavedGeo(Vec<GeoRow>);

fn load_dates() -> HashMap<String, (u64, i64, i64)> {
    let saved: Option<Saved> = dates_file()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice(&b).ok());
    saved
        .map(|s| {
            s.0.into_iter()
                .map(|(path, size, modified, taken)| (path, (size, modified, taken)))
                .collect()
        })
        .unwrap_or_default()
}

fn save_dates(dates: &HashMap<String, (u64, i64, i64)>) {
    let saved = Saved(
        dates
            .iter()
            .map(|(path, &(size, modified, taken))| (path.clone(), size, modified, taken))
            .collect(),
    );
    if let (Some(file), Ok(text)) = (dates_file(), serde_json::to_vec(&saved)) {
        let _ = std::fs::write(file, text);
    }
}

fn load_geo() -> HashMap<String, GeoRecord> {
    let saved: Option<SavedGeo> = geo_file()
        .and_then(|f| std::fs::read(f).ok())
        .and_then(|b| serde_json::from_slice(&b).ok());
    saved
        .map(|s| {
            s.0.into_iter()
                .map(|(path, size, modified, taken, place)| (path, (size, modified, taken, place)))
                .collect()
        })
        .unwrap_or_default()
}

fn save_geo(geo: &HashMap<String, GeoRecord>) {
    let saved = SavedGeo(
        geo.iter()
            .map(|(path, &(size, modified, taken, place))| (path.clone(), size, modified, taken, place))
            .collect(),
    );
    if let (Some(file), Ok(text)) = (geo_file(), serde_json::to_vec(&saved)) {
        let _ = std::fs::write(file, text);
    }
}

/// The gallery's folders, as the app has them (Pictures, Videos and the ones added in Settings).
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
    let request = shadui_index::proto::SearchRequest {
        query,
        roots: folders.to_vec(),
        hidden: false,
        exclude: Vec::new(),
        max_results: Some(MOST),
        fuzzy: false,
        rank: false,
    };
    let mut found = Vec::new();
    let mut whole = true;
    client::search(&request, |event| {
        match event {
            // Every folder the index covers, or it's walked instead.
            Event::Plan { skipped, .. } => whole = skipped.is_empty(),
            Event::Hits(hits) => found.extend(hits.into_iter().filter(|h| !h.dir).map(|h| {
                let name = h.path.rsplit('\\').next().unwrap_or(&h.path).to_string();
                Entry {
                    name,
                    path: Some(h.path),
                    dir: false,
                    size: h.size,
                    modified: h.modified,
                    attrs: h.attrs,
                }
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
    walk_capped(folders, MOST)
}

fn walk_capped(folders: &[String], limit: usize) -> Vec<Entry> {
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
                found.push(Entry {
                    path: Some(path.to_string_lossy().into_owned()),
                    ..entry
                });
                if found.len() >= limit {
                    return found;
                }
            }
        }
    }
    found
}

/// Gallery as a listing: each photo and video at its own path, its date when it was taken where it says
/// (else when it was modified), newest first. Files not dated yet are read in the background.
pub fn listing(started: Instant) -> Result<fsx::Listing, String> {
    let folders: Vec<String> = state()
        .folders
        .iter()
        .filter(|f| Path::new(f).is_dir())
        .cloned()
        .collect();
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
    Ok(fsx::Listing::Items {
        location: PLACE.into(),
        entries,
        origins: Vec::new(),
        elapsed: fsx::ms_since(started),
    })
}

/// Reads when each file was taken, four at a time, keeps it, and tells the app.
fn date(unread: Vec<(String, String, u64, i64)>) {
    let read = std::sync::Mutex::new(Vec::with_capacity(unread.len()));
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some((key, path, size, modified)) = unread.get(i) else {
                    break;
                };
                let taken = crate::media::taken_of(path).unwrap_or(0);
                read.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((key.clone(), (*size, *modified, taken)));
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
        app.emit(Event::GalleryDated);
    }
}

/// Scans `folder` (or all Gallery folders for `shell:Gallery`) for photos/videos with EXIF GPS coordinates,
/// backed by `gallery-geo.json` and 4 parallel EXIF readers.
pub fn geo_photos_in(folder: &str) -> Vec<glint_model::world_map::GeoPhoto> {
    const GEO_CAP: usize = 5_000;
    let entries = if is_place(folder) {
        let folders: Vec<String> = state()
            .folders
            .iter()
            .filter(|f| Path::new(f).is_dir())
            .cloned()
            .collect();
        files(&folders).into_iter().take(GEO_CAP).collect::<Vec<_>>()
    } else if Path::new(folder).is_dir() {
        walk_capped(&[folder.to_string()], GEO_CAP)
    } else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut unread = Vec::new();
    {
        let mut st = state();
        let geo = st.geo.get_or_insert_with(load_geo);
        for e in &entries {
            let Some(path) = e.path.as_ref() else {
                continue;
            };
            let key = path.to_lowercase();
            match geo.get(&key) {
                Some(&(size, modified, taken, place)) if size == e.size && modified == e.modified => {
                    if let Some([lat, lon]) = place {
                        if lat.is_finite() && lon.is_finite() {
                            out.push(glint_model::world_map::GeoPhoto {
                                path: path.clone(),
                                name: e.name.clone(),
                                lat,
                                lon,
                                taken: if taken > 0 { taken } else { e.modified },
                            });
                        }
                    }
                }
                _ => unread.push((key, path.clone(), e.name.clone(), e.size, e.modified)),
            }
        }
    }

    if !unread.is_empty() {
        let read = std::sync::Mutex::new(Vec::with_capacity(unread.len()));
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((key, path, name, size, modified)) = unread.get(i) else {
                        break;
                    };
                    let (taken_opt, place) = crate::media::taken_and_place(path);
                    let taken = taken_opt.unwrap_or(*modified);
                    read.lock().unwrap_or_else(|e| e.into_inner()).push((
                        key.clone(),
                        path.clone(),
                        name.clone(),
                        *size,
                        *modified,
                        taken,
                        place,
                    ));
                });
            }
        });
        let added = read.into_inner().unwrap_or_else(|e| e.into_inner());
        {
            let mut st = state();
            let geo = st.geo.get_or_insert_with(load_geo);
            for (key, path, name, size, modified, taken, place) in added {
                geo.insert(key, (size, modified, taken, place));
                if let Some([lat, lon]) = place {
                    if lat.is_finite() && lon.is_finite() {
                        out.push(glint_model::world_map::GeoPhoto {
                            path,
                            name,
                            lat,
                            lon,
                            taken,
                        });
                    }
                }
            }
            if !cfg!(test) {
                save_geo(geo);
            }
        }
    }

    out.sort_by_key(|p| std::cmp::Reverse(p.taken));
    out
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
        let mut names: Vec<String> = walk(&[dir.to_string_lossy().into_owned()])
            .into_iter()
            .map(|e| e.name)
            .collect();
        names.sort();
        assert_eq!(names, ["a.jpg", "b.MP4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn geo_cache_round_trips_and_handles_plain_folders() {
        let saved = SavedGeo(vec![(
            r"c:\pics\kyoto.jpg".into(),
            2048,
            1_700_000_000,
            1_700_000_000,
            Some([35.0116, 135.7681]),
        )]);
        let bytes = serde_json::to_vec(&saved).unwrap();
        let back: SavedGeo = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.0[0].4, Some([35.0116, 135.7681]));

        let dir = std::env::temp_dir().join(format!("glint-geo-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "plain").unwrap();
        std::fs::write(dir.join("empty.jpg"), "not exif").unwrap();
        assert!(geo_photos_in(&dir.to_string_lossy()).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
