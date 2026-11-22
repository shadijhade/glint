//! Gallery, a place Glint opens like a folder (`shell:Gallery`): the photos and videos in the gallery's
//! folders (Pictures and Videos, and the ones added in Settings), every one below them, newest first by
//! when each was taken. Instant search's index lists them at once where it covers the folders; else
//! they're walked. When each was taken comes from what it says (a photo's EXIF, a video's recording
//! date), read once and kept by path, size and date (gallery-meta.bin, with Glint's data: `meta.rs`, which
//! also holds where each was taken and how big it is, for the Photo Map), so the gallery opens at once the
//! next time; while new ones are read, in the background, they're dated when they were modified, and the app
//! is told once they're done ("gallery-dated").

use crate::fsx::{self, Entry};
use crate::host::{AppHandle, Event};
use glint_model::world_map::GeoPhoto;
use meta::{Job, Meta, Records, F_DATE, F_GEO};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::Instant;

pub mod meta;

/// Gallery's path in Glint.
pub const PLACE: &str = "shell:Gallery";

/// The most it lists.
const MOST: usize = 200_000;

/// The most photos the map looks into (a folder's, or the Gallery's).
const GEO_CAP: usize = 100_000;

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

#[derive(Default)]
struct State {
    folders: Vec<String>,
    app: Option<AppHandle>,
    /// What is known of each file, by its path in lower case (None until first asked for).
    records: Option<Records>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(Default::default);
/// Files being read for when they were taken, in the background.
static DATING: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs `f` on the records, loading them (and the old JSON files', once) the first time.
fn with_records<R>(f: impl FnOnce(&mut Records) -> R) -> R {
    let mut st = state();
    if st.records.is_none() {
        let (records, imported) = match crate::tools::data_dir() {
            Some(dir) => meta::load(&dir),
            None => (Records::new(), false),
        };
        st.records = Some(records);
        if imported {
            save_soon();
        }
    }
    f(st.records.get_or_insert_with(Records::new))
}

/// The records are written soon (once for a burst of changes), and the old JSON files go after.
fn save_soon() {
    meta::schedule_save(crate::tools::data_dir(), || state().records.clone());
}

fn lower(path: &str) -> String {
    path.to_lowercase()
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
    with_records(|records| {
        for e in &mut entries {
            let path = e.path.as_deref().unwrap_or_default();
            match records.get(&lower(path)) {
                Some(m) if m.is_of(e.size, e.modified) && m.flags & F_DATE != 0 => {
                    if m.taken > 0 {
                        e.modified = m.taken;
                    }
                }
                _ => unread.push(Job {
                    path: path.to_string(),
                    size: e.size,
                    modified: e.modified,
                }),
            }
        }
    });
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

/// Keeps what a scan read of `jobs` (rows are indices into them, and their records).
fn keep(jobs: &[Job], rows: &[(usize, Meta)]) {
    with_records(|records| {
        for &(i, meta) in rows {
            if let Some(job) = jobs.get(i) {
                records.insert(lower(&job.path), meta);
            }
        }
    });
    save_soon();
}

/// Reads when each file was taken (and the rest of what its header says), keeps it, and tells the app.
fn date(unread: Vec<Job>) {
    meta::scan(&unread, &|rows| keep(&unread, &rows));
    let app = state().app.clone();
    DATING.store(false, Ordering::SeqCst);
    if let Some(app) = app {
        app.emit(Event::GalleryDated);
    }
}

/// The files the map looks into for `folder` (or all Gallery folders for `shell:Gallery`).
fn geo_entries(folder: &str) -> Vec<Entry> {
    if is_place(folder) {
        let folders: Vec<String> = state()
            .folders
            .iter()
            .filter(|f| Path::new(f).is_dir())
            .cloned()
            .collect();
        files(&folders).into_iter().take(GEO_CAP).collect()
    } else if Path::new(folder).is_dir() {
        walk_capped(&[folder.to_string()], GEO_CAP)
    } else {
        Vec::new()
    }
}

fn geo_photo(path: &str, size: u64, modified: i64, meta: &Meta) -> Option<GeoPhoto> {
    let [lat, lon] = meta.place()?;
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    Some(GeoPhoto {
        path: path.to_string(),
        name: name.to_string(),
        lat,
        lon,
        taken: if meta.taken > 0 { meta.taken } else { modified },
        size,
        modified,
    })
}

/// Finds the photos and videos with a place in `folder` (or all Gallery folders for `shell:Gallery`) and hands
/// them to `emit` as they are found: first every one the cache already knows, at once, then the ones read
/// now, a batch every ~100 ms while the scan runs. `emit` may be called from several threads.
pub fn geo_photos_stream(folder: &str, emit: &(dyn Fn(Vec<GeoPhoto>) + Sync)) {
    let entries = geo_entries(folder);
    let mut known = Vec::new();
    let mut unread = Vec::new();
    with_records(|records| {
        for e in &entries {
            let Some(path) = e.path.as_ref() else {
                continue;
            };
            match records.get(&lower(path)) {
                Some(m) if m.is_of(e.size, e.modified) && m.flags & F_GEO != 0 => {
                    known.extend(geo_photo(path, e.size, e.modified, m));
                }
                _ => unread.push(Job {
                    path: path.clone(),
                    size: e.size,
                    modified: e.modified,
                }),
            }
        }
    });
    if !known.is_empty() {
        emit(known);
    }
    if unread.is_empty() {
        return;
    }
    meta::scan(&unread, &|rows| {
        keep(&unread, &rows);
        let found: Vec<GeoPhoto> = rows
            .iter()
            .filter_map(|(i, m)| {
                let job = unread.get(*i)?;
                geo_photo(&job.path, job.size, job.modified, m)
            })
            .collect();
        if !found.is_empty() {
            emit(found);
        }
    });
}

/// [`geo_photos_stream`], waiting for all of them: newest first.
pub fn geo_photos_in(folder: &str) -> Vec<GeoPhoto> {
    let all = Mutex::new(Vec::new());
    geo_photos_stream(folder, &|mut batch| {
        all.lock().unwrap_or_else(|e| e.into_inner()).append(&mut batch)
    });
    let mut out = all.into_inner().unwrap_or_else(|e| e.into_inner());
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
    fn a_plain_folder_gives_only_its_geotagged_photos_and_streams_them() {
        let dir = std::env::temp_dir().join(format!("glint-geo-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "plain").unwrap();
        std::fs::write(dir.join("empty.jpg"), "not exif").unwrap();
        assert!(geo_photos_in(&dir.to_string_lossy()).is_empty());

        let tokyo = meta::tests::test_jpeg("2022:03:04 05:06:07", Some((35.6762, 139.6503)), (800, 600), 3_000);
        let plain = meta::tests::test_jpeg("2022:03:05 05:06:07", None, (800, 600), 3_000);
        std::fs::write(dir.join("tokyo.jpg"), tokyo).unwrap();
        std::fs::write(dir.join("nowhere.jpg"), plain).unwrap();
        let batches = Mutex::new(Vec::new());
        geo_photos_stream(&dir.to_string_lossy(), &|batch| batches.lock().unwrap().push(batch));
        let batches = batches.into_inner().unwrap();
        let all: Vec<&GeoPhoto> = batches.iter().flatten().collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "tokyo.jpg");
        assert!((all[0].lat - 35.6762).abs() < 1e-3 && (all[0].lon - 139.6503).abs() < 1e-3);
        assert!(all[0].taken > 0 && all[0].size > 0 && all[0].modified > 0);
        // The same photo as a list, newest first.
        assert_eq!(geo_photos_in(&dir.to_string_lossy()).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
