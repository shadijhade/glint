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
pub mod probe;

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
        marks: false,
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
    let mut folders = state().folders.clone();
    folders.retain(|f| Path::new(f).is_dir());
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
        let mut folders = state().folders.clone();
        folders.retain(|f| Path::new(f).is_dir());
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
        path: path.into(),
        name: name.into(),
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

// ---- Photos the Gallery doesn't show ---------------------------------------------------------------------------

/// A folder outside the Gallery's that holds what a camera or a phone makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Elsewhere {
    pub folder: String,
    /// How many photos and videos are below it.
    pub files: usize,
}

/// What a camera or a phone makes. No PNG, GIF, WebP or BMP: those are mostly what programs draw with.
const CAMERA_KINDS: &[&str] = &[
    "jpg", "jpeg", "heic", "heif", "dng", "cr2", "cr3", "nef", "nrw", "arw", "raf", "orf", "rw2", "pef", "srw", "mp4",
    "mov", "m4v", "mts", "m2ts", "3gp",
];

/// A file under this isn't counted: thumbnails, icons, and the pictures programs and web pages are made of.
const CAMERA_FROM_KB: u64 = 300;

/// A folder is worth naming from this many photos and videos.
pub const ELSEWHERE_FROM: usize = 40;

/// The most files looked at: a PC with more photos than this outside its Gallery is answered from the first ones.
const ELSEWHERE_LOOKS: usize = 400_000;

/// Folders photos are never looked for under (any case): Windows', programs', caches, what a build makes.
const NOT_PHOTOS: &[&str] = &[
    "windows",
    "program files",
    "program files (x86)",
    "programdata",
    "appdata",
    "$recycle.bin",
    "system volume information",
    "node_modules",
    "site-packages",
    "steamapps",
    ".git",
    "cache",
    "caches",
    "temp",
    "tmp",
    "thumbnails",
    ".thumbnails",
];

/// The folders outside the Gallery's with the most photos and videos below them, most first, at most `most` of
/// them: for the suggestion to show one in the Gallery. Asked of instant search's index only, below `drives`
/// (their roots); None when the index doesn't answer, since nothing is walked for a suggestion. `broad` are
/// folders too wide to offer themselves (the person's profile, the folder profiles are in): what is below them is.
pub fn elsewhere(drives: &[String], broad: &[String], most: usize) -> Option<Vec<Elsewhere>> {
    let shown = state().folders.clone();
    let paths = camera_files(drives)?;
    Some(group_elsewhere(paths.iter().map(String::as_str), &shown, broad, most))
}

/// Every file a camera or a phone made below `drives`, from the index.
#[cfg(windows)]
fn camera_files(drives: &[String]) -> Option<Vec<String>> {
    use shadui_index::client::{self, Event};
    if drives.is_empty() {
        return None;
    }
    let request = shadui_index::proto::SearchRequest {
        query: format!("ext:{} size:>{CAMERA_FROM_KB}kb", CAMERA_KINDS.join(";")),
        roots: drives.to_vec(),
        hidden: false,
        exclude: Vec::new(),
        max_results: Some(ELSEWHERE_LOOKS),
        fuzzy: false,
        rank: false,
        marks: false,
    };
    let mut found = Vec::new();
    client::search(&request, |event| {
        if let Event::Hits(hits) = event {
            found.extend(hits.into_iter().filter(|h| !h.dir).map(|h| h.path));
        }
        true
    })
    .ok()?;
    Some(found)
}

#[cfg(not(windows))]
fn camera_files(_drives: &[String]) -> Option<Vec<String>> {
    None
}

/// Whether `path` is `folder` or below it (any case, either slash at the end of `folder`).
fn is_under(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches(['\\', '/']);
    path.len() >= folder.len()
        && path.is_char_boundary(folder.len())
        && path[..folder.len()].eq_ignore_ascii_case(folder)
        && matches!(path.as_bytes().get(folder.len()), None | Some(b'\\' | b'/'))
}

/// [`elsewhere`]'s answer for these files: each is counted for every folder above it, then, from each drive down,
/// the folder named is the deepest one that still holds nearly all of what is below it (a folder whose photos are
/// spread over its subfolders is named itself; one with a single subfolder of photos gives that subfolder).
pub fn group_elsewhere<'a>(
    paths: impl Iterator<Item = &'a str>,
    shown: &[String],
    broad: &[String],
    most: usize,
) -> Vec<Elsewhere> {
    use std::collections::HashMap;
    /// A folder: how it is written, how many files are below it, and the folders right inside it that have some.
    #[derive(Default)]
    struct Node {
        written: String,
        files: usize,
        inside: Vec<String>,
    }
    // How many files are right in each folder (by the folder in lower case), and how it is written.
    let mut direct: HashMap<String, (String, usize)> = HashMap::new();
    for path in paths {
        if shown.iter().any(|folder| is_under(path, folder)) {
            continue;
        }
        let Some((folder, _)) = path.rsplit_once('\\') else { continue };
        let lower = folder.to_lowercase();
        if lower.split('\\').any(|part| NOT_PHOTOS.contains(&part)) {
            continue;
        }
        direct.entry(lower).or_insert_with(|| (folder.to_string(), 0)).1 += 1;
    }
    // A folder's files count for every folder above it; a folder met for the first time is put inside the one
    // above it, once.
    let mut nodes: HashMap<String, Node> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();
    for (lower, (written, files)) in &direct {
        let (mut key, mut written) = (lower.as_str(), written.as_str());
        let mut below: Option<String> = None;
        loop {
            let fresh = !nodes.contains_key(key);
            let node = nodes.entry(key.to_string()).or_insert_with(|| Node {
                written: written.to_string(),
                ..Node::default()
            });
            node.files += files;
            node.inside.extend(below.take());
            below = fresh.then(|| key.to_string());
            match (key.rsplit_once('\\'), written.rsplit_once('\\')) {
                (Some((above, _)), Some((above_written, _))) => (key, written) = (above, above_written),
                _ => {
                    roots.extend(below.take());
                    break;
                }
            }
        }
    }
    let too_broad = |written: &str| {
        // A drive ("D:"), or one of the folders the caller named.
        !written.contains('\\') || broad.iter().any(|b| b.trim_end_matches('\\').eq_ignore_ascii_case(written))
    };
    let mut found: Vec<Elsewhere> = Vec::new();
    let mut stack = roots;
    while let Some(key) = stack.pop() {
        let Some(node) = nodes.get(&key) else { continue };
        if node.files < ELSEWHERE_FROM {
            continue;
        }
        if too_broad(&node.written) {
            stack.extend(node.inside.iter().cloned());
            continue;
        }
        // One folder inside holds nearly all of it: that one is where the photos are.
        let heavy = node
            .inside
            .iter()
            .find(|inner| nodes.get(*inner).is_some_and(|n| n.files * 100 >= node.files * 85));
        match heavy {
            Some(inner) => stack.push(inner.clone()),
            None => found.push(Elsewhere {
                folder: node.written.clone(),
                files: node.files,
            }),
        }
    }
    found.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.folder.cmp(&b.folder)));
    found.truncate(most);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` photos in `folder`.
    fn photos(folder: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{folder}\\IMG_{i:04}.JPG")).collect()
    }

    fn grouped(paths: &[String], shown: &[&str], broad: &[&str]) -> Vec<(String, usize)> {
        let shown: Vec<String> = shown.iter().map(|s| (*s).to_string()).collect();
        let broad: Vec<String> = broad.iter().map(|s| (*s).to_string()).collect();
        group_elsewhere(paths.iter().map(String::as_str), &shown, &broad, 5)
            .into_iter()
            .map(|e| (e.folder, e.files))
            .collect()
    }

    #[test]
    fn photos_elsewhere_name_the_folder_they_are_really_in() {
        // All in one folder, three levels down: that folder, not the ones above it.
        let deep = photos(r"D:\Backup\Phone\DCIM\Camera", 120);
        assert_eq!(grouped(&deep, &[], &[]), [(r"D:\Backup\Phone\DCIM\Camera".to_string(), 120)]);
        // Spread over years: the folder that holds the years.
        let mut years = photos(r"D:\Photos\2022", 60);
        years.extend(photos(r"D:\Photos\2023", 70));
        years.extend(photos(r"D:\Photos\2024\Trip", 50));
        assert_eq!(grouped(&years, &[], &[]), [(r"D:\Photos".to_string(), 180)]);
        // Two places: both, the bigger first.
        let mut two = deep.clone();
        two.extend(years.clone());
        assert_eq!(
            grouped(&two, &[], &[]),
            [(r"D:\Photos".to_string(), 180), (r"D:\Backup\Phone\DCIM\Camera".to_string(), 120)]
        );
    }

    #[test]
    fn photos_elsewhere_leave_out_the_gallery_few_files_and_what_programs_keep() {
        // In the Gallery already (any case), under a program's folder, in a cache, or too few to matter.
        let mut paths = photos(r"C:\Users\me\Pictures\2024", 500);
        paths.extend(photos(r"C:\Users\me\AppData\Local\Some App\media", 300));
        paths.extend(photos(r"C:\Program Files\Game\textures", 300));
        paths.extend(photos(r"D:\Site\node_modules\pkg\img", 300));
        paths.extend(photos(r"D:\Work\cache\thumbs", 300));
        paths.extend(photos(r"D:\Few", ELSEWHERE_FROM - 1));
        assert!(grouped(&paths, &[r"c:\users\me\pictures"], &[]).is_empty());
        // A folder whose name only starts like the Gallery's is not in it.
        let near = photos(r"C:\Users\me\Pictures old", 50);
        assert_eq!(grouped(&near, &[r"C:\Users\me\Pictures"], &[]).len(), 1);
    }

    #[test]
    fn a_drive_and_the_profile_are_too_wide_to_offer_themselves() {
        // Loose on the drive: the drive is never offered.
        assert!(grouped(&photos("D:", 90), &[], &[]).is_empty());
        // Under the profile: its folders are offered, never the profile.
        let mut paths = photos(r"C:\Users\me\Desktop\Wedding", 80);
        paths.extend(photos(r"C:\Users\me\Phone", 60));
        let found = grouped(&paths, &[], &[r"C:\Users\me", r"C:\Users"]);
        assert_eq!(
            found,
            [(r"C:\Users\me\Desktop\Wedding".to_string(), 80), (r"C:\Users\me\Phone".to_string(), 60)]
        );
    }

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
        assert_eq!(&*all[0].name, "tokyo.jpg");
        assert!((all[0].lat - 35.6762).abs() < 1e-3 && (all[0].lon - 139.6503).abs() < 1e-3);
        assert!(all[0].taken > 0 && all[0].size > 0 && all[0].modified > 0);
        // The same photo as a list, newest first.
        assert_eq!(geo_photos_in(&dir.to_string_lossy()).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
