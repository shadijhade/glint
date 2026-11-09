//! Compress in place: Windows' transparent compression (WOF, the engine behind `compact /exe` and
//! CompactGUI). A file is stored compressed and decompressed as it's read, so it opens as before; a file
//! an app rewrites is stored whole again. NTFS only, and no admin needed for the user's own files.
//!
//! The saving is measured before anything changes: a sample of the chosen files is written to a hidden
//! file and compressed with each algorithm, so the estimate is the real algorithm's. Undo takes the
//! compression off the files that were compressed.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use tauri::ipc::Channel;

static STOP: AtomicBool = AtomicBool::new(false);
/// What each run compressed, for its Undo (by the id its report gives).
static UNDO: LazyLock<Mutex<HashMap<u64, Vec<PathBuf>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_UNDO: AtomicU64 = AtomicU64::new(1);

/// Smaller than this, a file can't take less than the cluster it's in.
const SMALLEST: u64 = 4096;
/// The sample the estimate compresses: up to this many files, a few slices of each, this much in all.
const SAMPLE_FILES: usize = 48;
const SLICE: usize = 32 * 1024;
const SAMPLE_MAX: usize = 6 << 20;

/// Kinds already compressed inside (media, archives, Office's zips, PDFs) and disk images (big, and
/// rewritten in place while mounted): WOF would gain nothing, or undo itself.
const PACKED: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "gif", "webp", "avif", "heic", "heif", "jxl", "mp4", "m4v", "mov", "mkv", "webm", "avi", "wmv", "flv", "mts", "m2ts", "3gp", "mp3", "m4a", "aac", "ogg", "opus", "flac", "wma", "zip", "7z", "rar", "gz", "tgz",
    "bz2", "xz", "zst", "lz4", "lzma", "cab", "msi", "msix", "appx", "apk", "jar", "nupkg", "whl", "docx", "docm", "xlsx", "xlsm", "pptx", "pptm", "odt", "ods", "odp", "epub", "cbz", "cbr", "pdf", "vhd", "vhdx", "vmdk", "vdi", "qcow2", "pst", "ost",
];

/// WOF's algorithms, lightest first.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Algorithm {
    Xpress4k,
    Xpress8k,
    Xpress16k,
    Lzx,
}

impl Algorithm {
    const ALL: [Algorithm; 4] = [Algorithm::Xpress4k, Algorithm::Xpress8k, Algorithm::Xpress16k, Algorithm::Lzx];

    fn name(self) -> &'static str {
        match self {
            Algorithm::Xpress4k => "xpress4k",
            Algorithm::Xpress8k => "xpress8k",
            Algorithm::Xpress16k => "xpress16k",
            Algorithm::Lzx => "lzx",
        }
    }

    /// FILE_PROVIDER_COMPRESSION_*.
    fn code(self) -> u32 {
        match self {
            Algorithm::Xpress4k => 0,
            Algorithm::Lzx => 1,
            Algorithm::Xpress8k => 2,
            Algorithm::Xpress16k => 3,
        }
    }
}

/// What the chosen files hold, and what each algorithm would leave of them.
#[derive(Serialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Measure {
    /// Every chosen place is on an NTFS drive (the only kind WOF works on).
    pub ntfs: bool,
    /// Files that would be compressed, and their size.
    pub files: usize,
    pub bytes: u64,
    /// Files compressed already (by WOF, or NTFS's own compression), their size and what they take.
    pub already: usize,
    pub already_bytes: u64,
    pub already_on_disk: u64,
    /// Of those, the ones WOF compressed (what Decompress takes back).
    pub wof: usize,
    /// Left alone: packed kinds, tiny files, links, online-only and encrypted files.
    pub skipped: usize,
    /// What the files would take after, per algorithm (empty when no sample could be made).
    pub estimates: Vec<Estimate>,
    pub stopped: bool,
}

#[derive(Serialize, Debug)]
pub struct Estimate {
    pub algorithm: &'static str,
    pub after: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub current: String,
    /// Bytes saved so far (taken back, when decompressing).
    pub saved: u64,
}

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// Files compressed (or decompressed).
    pub done: usize,
    /// Files Windows found wouldn't get smaller, left as they were.
    pub unchanged: usize,
    /// The first few files that failed, and why; and how many did in all.
    pub failed: Vec<(String, String)>,
    pub failed_count: usize,
    /// What the changed files took before, and take now.
    pub before: u64,
    pub after: u64,
    /// For `wof_undo`, when anything was compressed.
    pub undo: Option<u64>,
    pub stopped: bool,
    pub elapsed_ms: f64,
}

#[tauri::command]
pub fn wof_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

/// Measures `paths` (files and folders) for Compress in place.
#[tauri::command]
pub async fn wof_measure(paths: Vec<String>) -> Result<Measure, String> {
    STOP.store(false, Ordering::SeqCst);
    crate::fsx::blocking(move || Ok(measure(&paths, &STOP))).await
}

/// Compresses what `paths` hold with `algorithm`, a few files at a time.
#[tauri::command]
pub async fn wof_compress(paths: Vec<String>, algorithm: Algorithm, on_progress: Channel<Progress>) -> Result<Report, String> {
    STOP.store(false, Ordering::SeqCst);
    crate::fsx::blocking(move || {
        let walk = walk(&paths, &STOP, false);
        if !walk.ntfs {
            return Err("Compress in place works on NTFS drives only".into());
        }
        let files: Vec<(PathBuf, u64)> = walk.candidates;
        Ok(apply(&files, Some(algorithm), &STOP, &|p| {
            let _ = on_progress.send(p);
        }))
    })
    .await
}

/// Takes WOF's compression off what `paths` hold.
#[tauri::command]
pub async fn wof_decompress(paths: Vec<String>, on_progress: Channel<Progress>) -> Result<Report, String> {
    STOP.store(false, Ordering::SeqCst);
    crate::fsx::blocking(move || {
        let files = walk(&paths, &STOP, true).compressed;
        Ok(apply(&files, None, &STOP, &|p| {
            let _ = on_progress.send(p);
        }))
    })
    .await
}

/// Undoes a run of `wof_compress`: its files decompressed (the ones still there).
#[tauri::command]
pub async fn wof_undo(id: u64, on_progress: Channel<Progress>) -> Result<Report, String> {
    STOP.store(false, Ordering::SeqCst);
    let paths = UNDO.lock().unwrap_or_else(|e| e.into_inner()).remove(&id).ok_or("That's been undone already")?;
    crate::fsx::blocking(move || {
        let files: Vec<(PathBuf, u64)> = paths.into_iter().map(|p| (p, 0)).collect();
        Ok(apply(&files, None, &STOP, &|p| {
            let _ = on_progress.send(p);
        }))
    })
    .await
}

/// What a walk over the chosen paths found.
#[derive(Default)]
struct Walk {
    ntfs: bool,
    /// Files to compress, with their sizes.
    candidates: Vec<(PathBuf, u64)>,
    /// Files WOF compressed, with their sizes.
    compressed: Vec<(PathBuf, u64)>,
    already: usize,
    already_bytes: u64,
    already_on_disk: u64,
    skipped: usize,
    stopped: bool,
}

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_SPARSE: u32 = 0x200;
const ATTR_REPARSE: u32 = 0x400;
const ATTR_COMPRESSED: u32 = 0x800;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_ENCRYPTED: u32 = 0x4000;
const ATTR_RECALL_ON_OPEN: u32 = 0x40000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x400000;

fn packed(path: &Path) -> bool {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).is_some_and(|e| PACKED.contains(&e.as_str()))
}

/// Walks `paths`: the files to compress, and the ones compressed already. WOF hides its reparse point
/// from folder listings, so each file that might be compressed is asked (a few at a time); `all` asks
/// every file, packed kinds too, for Decompress.
#[cfg(windows)]
fn walk(paths: &[String], stop: &AtomicBool, all: bool) -> Walk {
    use rayon::prelude::*;
    use std::os::windows::fs::MetadataExt;
    let mut found = Walk { ntfs: !paths.is_empty(), ..Walk::default() };
    let mut volumes: HashMap<String, bool> = HashMap::new();
    let mut stack: Vec<(PathBuf, std::fs::Metadata)> = Vec::new();
    for path in paths {
        let path = PathBuf::from(path);
        let ntfs = *volumes.entry(volume_of(&path)).or_insert_with_key(|root| is_ntfs(root));
        found.ntfs &= ntfs;
        if !ntfs {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            stack.push((path, meta));
        }
    }
    // Files that may be WOF's already: (path, size, a kind worth compressing).
    let mut ask: Vec<(PathBuf, u64, bool)> = Vec::new();
    let mut seen = 0usize;
    while let Some((path, meta)) = stack.pop() {
        seen += 1;
        if seen.is_multiple_of(512) && stop.load(Ordering::SeqCst) {
            found.stopped = true;
            return found;
        }
        let attrs = meta.file_attributes();
        if attrs & ATTR_DIRECTORY != 0 {
            // Links to other folders (junctions, symbolic links) aren't followed.
            if attrs & ATTR_REPARSE != 0 {
                continue;
            }
            if let Ok(read) = std::fs::read_dir(&path) {
                for entry in read.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        stack.push((entry.path(), meta));
                    }
                }
            }
            continue;
        }
        let size = meta.len();
        if attrs & ATTR_COMPRESSED != 0 {
            // NTFS's own compression: compressed already, but not WOF's to take off.
            found.already += 1;
            found.already_bytes += size;
            found.already_on_disk += on_disk(&path).unwrap_or(size);
            continue;
        }
        // Links, cloud placeholders, sparse, encrypted and tiny files are left alone.
        if attrs & (ATTR_REPARSE | ATTR_SPARSE | ATTR_OFFLINE | ATTR_ENCRYPTED | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0 || size < SMALLEST {
            found.skipped += 1;
            continue;
        }
        let worth = !packed(&path);
        if worth || all {
            ask.push((path, size, worth));
        } else {
            found.skipped += 1;
        }
    }
    let asked: Vec<(PathBuf, u64, bool, bool)> = ask.into_par_iter().map(|(path, size, worth)| { let wof = wof_compressed(&path); (path, size, worth, wof) }).collect();
    for (path, size, worth, wof) in asked {
        if wof {
            found.already += 1;
            found.already_bytes += size;
            found.already_on_disk += on_disk(&path).unwrap_or(size);
            found.compressed.push((path, size));
        } else if worth {
            found.candidates.push((path, size));
        } else {
            found.skipped += 1;
        }
    }
    found
}

#[cfg(not(windows))]
fn walk(_: &[String], _: &AtomicBool, _: bool) -> Walk {
    Walk::default()
}

fn measure(paths: &[String], stop: &AtomicBool) -> Measure {
    let walk = walk(paths, stop, false);
    let bytes = walk.candidates.iter().map(|(_, size)| size).sum::<u64>();
    let mut measure = Measure {
        ntfs: walk.ntfs,
        files: walk.candidates.len(),
        bytes,
        already: walk.already,
        already_bytes: walk.already_bytes,
        already_on_disk: walk.already_on_disk,
        wof: walk.compressed.len(),
        skipped: walk.skipped,
        estimates: Vec::new(),
        stopped: walk.stopped,
    };
    if walk.ntfs && !walk.stopped && bytes > 0 {
        if let Some(ratios) = sample_ratios(paths, &walk.candidates) {
            measure.estimates = Algorithm::ALL.iter().zip(ratios).map(|(a, ratio)| Estimate { algorithm: a.name(), after: (bytes as f64 * ratio).round() as u64 }).collect();
        }
    }
    measure
}

/// Up to [`SAMPLE_FILES`] of `files`, spread by size (so the big ones, which hold most of the bytes,
/// weigh most), a few slices of each at 32 KB boundaries (WOF compresses in chunks of at most 32 KB, so
/// the slices compress as they would in place).
fn sample(files: &[(PathBuf, u64)]) -> Vec<u8> {
    use std::io::{Seek, SeekFrom};
    let total: u64 = files.iter().map(|(_, s)| s).sum();
    if total == 0 {
        return Vec::new();
    }
    let step = total / SAMPLE_FILES as u64 + 1;
    let (mut next, mut running) = (0u64, 0u64);
    let mut out = Vec::new();
    for (path, size) in files {
        running += size;
        if running < next {
            continue;
        }
        next = running + step;
        let Ok(mut file) = std::fs::File::open(path) else { continue };
        let last = size.saturating_sub(SLICE as u64) & !0x7FFF;
        let mut offsets = vec![0, (size / 2) & !0x7FFF, last];
        offsets.dedup();
        for offset in offsets {
            let mut slice = vec![0u8; SLICE];
            if file.seek(SeekFrom::Start(offset)).is_err() {
                break;
            }
            let Ok(n) = read_full(&mut file, &mut slice) else { break };
            // Whole 4 KB pages only, so the next slice starts on a chunk boundary of every algorithm.
            slice.truncate(n & !0xFFF);
            if slice.is_empty() {
                break;
            }
            out.extend_from_slice(&slice);
            let pad = out.len().next_multiple_of(32 * 1024) - out.len();
            out.resize(out.len() + pad, 0);
            if out.len() >= SAMPLE_MAX {
                return out;
            }
        }
    }
    out
}

fn read_full(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::io::Read;
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// What each algorithm leaves of the sample (after / before), measured by compressing it for real in a
/// hidden file on the same drive. None when the sample can't be written anywhere there.
fn sample_ratios(paths: &[String], files: &[(PathBuf, u64)]) -> Option<[f64; 4]> {
    let bytes = sample(files);
    if bytes.is_empty() {
        return None;
    }
    let first = Path::new(paths.first()?);
    // Inside the chosen folder, else next to the chosen file; the temp folder when it's on the same drive.
    let mut places: Vec<PathBuf> = Vec::new();
    if first.is_dir() {
        places.push(first.to_path_buf());
    }
    if let Some(parent) = first.parent() {
        places.push(parent.to_path_buf());
    }
    let temp = std::env::temp_dir();
    if volume_of(&temp).eq_ignore_ascii_case(&volume_of(first)) {
        places.push(temp);
    }
    let name = format!(".glint-measure-{}", std::process::id());
    for place in places {
        let probe = place.join(&name);
        let mut ratios = [1.0f64; 4];
        let mut ok = true;
        for (i, algorithm) in Algorithm::ALL.iter().enumerate() {
            if std::fs::write(&probe, &bytes).is_err() {
                ok = false;
                break;
            }
            crate::actions::hide(&probe);
            ratios[i] = match compress_one(&probe, Some(*algorithm)) {
                Ok(Outcome::Changed { after, .. }) => after as f64 / bytes.len() as f64,
                Ok(Outcome::Unchanged) => 1.0,
                Err(_) => {
                    ok = false;
                    break;
                }
            };
            let _ = std::fs::remove_file(&probe);
        }
        let _ = std::fs::remove_file(&probe);
        if ok {
            return Some(ratios);
        }
    }
    None
}

enum Outcome {
    Changed { before: u64, after: u64 },
    Unchanged,
}

/// Compresses `files` with `algorithm`, or decompresses them (None), a few at a time.
fn apply(files: &[(PathBuf, u64)], algorithm: Option<Algorithm>, stop: &AtomicBool, progress: &(dyn Fn(Progress) + Sync)) -> Report {
    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let saved = AtomicU64::new(0);
    let last_sent = AtomicU64::new(0);
    let report = Mutex::new(Report::default());
    let changed: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 6).min(files.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some((path, _)) = files.get(i) else { break };
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let outcome = compress_one(path, algorithm);
                {
                    let mut r = report.lock().unwrap_or_else(|e| e.into_inner());
                    match outcome {
                        Ok(Outcome::Changed { before, after }) => {
                            r.done += 1;
                            r.before += before;
                            r.after += after;
                            saved.fetch_add(before.abs_diff(after), Ordering::Relaxed);
                            changed.lock().unwrap_or_else(|e| e.into_inner()).push(path.clone());
                        }
                        Ok(Outcome::Unchanged) => r.unchanged += 1,
                        Err(why) => {
                            r.failed_count += 1;
                            if r.failed.len() < 50 {
                                r.failed.push((path.to_string_lossy().into_owned(), why));
                            }
                        }
                    }
                }
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                // A few updates a second, not one per file.
                let now = started.elapsed().as_millis() as u64;
                let then = last_sent.load(Ordering::Relaxed);
                if (now >= then + 100 || n == files.len()) && last_sent.compare_exchange(then, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    let current = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    progress(Progress { done: n, total: files.len(), current, saved: saved.load(Ordering::Relaxed) });
                }
            });
        }
    });
    let mut report = report.into_inner().unwrap_or_else(|e| e.into_inner());
    report.stopped = stop.load(Ordering::SeqCst);
    let changed = changed.into_inner().unwrap_or_else(|e| e.into_inner());
    if algorithm.is_some() && !changed.is_empty() {
        let id = NEXT_UNDO.fetch_add(1, Ordering::SeqCst);
        UNDO.lock().unwrap_or_else(|e| e.into_inner()).insert(id, changed);
        report.undo = Some(id);
    }
    report.elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    report
}

/// Compresses one file with `algorithm` (or decompresses it, None): what it took before and after.
/// Its dates stay as they were.
#[cfg(windows)]
fn compress_one(path: &Path, algorithm: Option<Algorithm>) -> Result<Outcome, String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ};
    use windows::Win32::Storage::FileSystem::{CreateFileW, WofSetFileDataLocation, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, OPEN_EXISTING, WOF_FILE_COMPRESSION_INFO_V1, WOF_PROVIDER_FILE};
    use windows::Win32::System::IO::DeviceIoControl;
    const FSCTL_DELETE_EXTERNAL_BACKING: u32 = 0x0009_0314;

    let before = on_disk(path).ok_or_else(|| "It's gone".to_string())?;
    let handle = unsafe { CreateFileW(&HSTRING::from(path.as_os_str()), GENERIC_READ.0, FILE_SHARE_READ, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.map_err(|e| why(&e))?;
    let done = match algorithm {
        Some(algorithm) => {
            let info = WOF_FILE_COMPRESSION_INFO_V1 { Algorithm: algorithm.code(), Flags: 0 };
            unsafe { WofSetFileDataLocation(handle, WOF_PROVIDER_FILE, &info as *const _ as *const core::ffi::c_void, size_of::<WOF_FILE_COMPRESSION_INFO_V1>() as u32) }
        }
        None => unsafe { DeviceIoControl(handle, FSCTL_DELETE_EXTERNAL_BACKING, None, 0, None, 0, None, None) },
    };
    let _ = unsafe { CloseHandle(handle) };
    match done {
        Ok(()) => {
            let after = on_disk(path).unwrap_or(before);
            Ok(Outcome::Changed { before, after })
        }
        // ERROR_COMPRESSION_NOT_BENEFICIAL: Windows tried, and nothing would be saved.
        Err(e) if e.code().0 as u32 == 0x8007_0158 => Ok(Outcome::Unchanged),
        // Decompressing a file that isn't compressed (ERROR_OBJECT_NOT_EXTERNALLY_BACKED).
        Err(e) if algorithm.is_none() && e.code().0 as u32 == 0x8007_0156 => Ok(Outcome::Unchanged),
        Err(e) => Err(why(&e)),
    }
}

#[cfg(not(windows))]
fn compress_one(_: &Path, _: Option<Algorithm>) -> Result<Outcome, String> {
    Err("Compress in place needs Windows".into())
}

/// A Windows error in a few plain words.
#[cfg(windows)]
fn why(e: &windows::core::Error) -> String {
    match e.code().0 as u32 {
        0x8007_0020 | 0x8007_0021 => "It's in use by another app".into(),
        0x8007_0005 => "Windows refused (it may need administrator rights)".into(),
        0x8007_0070 | 0x8007_0027 => "There isn't enough free space".into(),
        0x8007_0002 | 0x8007_0003 => "It's gone".into(),
        0x8007_04C8 => "It's a running program".into(),
        _ => e.message().trim().to_string(),
    }
}

/// What `path` takes on its drive (compressed size for compressed files).
#[cfg(windows)]
fn on_disk(path: &Path) -> Option<u64> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetCompressedFileSizeW;
    use windows::Win32::Foundation::{GetLastError, SetLastError, NO_ERROR};
    let mut high = 0u32;
    let low = unsafe {
        SetLastError(NO_ERROR);
        GetCompressedFileSizeW(&HSTRING::from(path.as_os_str()), Some(&mut high))
    };
    if low == u32::MAX && unsafe { GetLastError() } != NO_ERROR {
        return None;
    }
    Some(((high as u64) << 32) | low as u64)
}

#[cfg(not(windows))]
fn on_disk(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Whether WOF compressed `path` (rather than it being some other reparse point).
#[cfg(windows)]
fn wof_compressed(path: &Path) -> bool {
    use windows::core::{HSTRING, BOOL};
    use windows::Win32::Storage::FileSystem::{WofIsExternalFile, WOF_PROVIDER_FILE};
    let mut external = BOOL(0);
    let mut provider = 0u32;
    unsafe { WofIsExternalFile(&HSTRING::from(path.as_os_str()), Some(&mut external), Some(&mut provider), None, None) }.is_ok() && external.as_bool() && provider == WOF_PROVIDER_FILE
}

#[cfg(not(windows))]
fn wof_compressed(_: &Path) -> bool {
    false
}

/// The root of the drive or mount `path` is on, e.g. "C:\".
#[cfg(windows)]
fn volume_of(path: &Path) -> String {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetVolumePathNameW;
    let mut buf = [0u16; 512];
    if unsafe { GetVolumePathNameW(&HSTRING::from(path.as_os_str()), &mut buf) }.is_ok() {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        return String::from_utf16_lossy(&buf[..end]);
    }
    path.components().next().map(|c| format!("{}\\", c.as_os_str().to_string_lossy())).unwrap_or_default()
}

#[cfg(not(windows))]
fn volume_of(_: &Path) -> String {
    "/".into()
}

#[cfg(windows)]
fn is_ntfs(root: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    let mut fs = [0u16; 64];
    unsafe { GetVolumeInformationW(&HSTRING::from(root), None, None, None, None, Some(&mut fs)) }.is_ok() && String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(fs.len())]) == "NTFS"
}

#[cfg(not(windows))]
fn is_ntfs(_: &str) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::fs::MetadataExt;
    const ATTR_READONLY: u32 = 0x1;

    #[test]
    fn compresses_measures_and_undoes() {
        let dir = std::env::temp_dir().join(format!("glint-wof-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        if !is_ntfs(&volume_of(&dir)) {
            eprintln!("temp isn't on NTFS: skipped");
            return;
        }
        let text = "Glint compresses this line again and again. ".repeat(40_000);
        std::fs::write(dir.join("notes.txt"), &text).unwrap();
        std::fs::write(dir.join("sub").join("more.log"), &text).unwrap();
        std::fs::write(dir.join("photo.jpg"), &text).unwrap();
        std::fs::write(dir.join("tiny.txt"), "small").unwrap();
        // A read-only file is compressed too.
        let locked = dir.join("sub").join("locked.txt");
        std::fs::write(&locked, &text).unwrap();
        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&locked, perms).unwrap();
        let modified = std::fs::metadata(dir.join("notes.txt")).unwrap().modified().unwrap();

        let paths = vec![dir.to_string_lossy().into_owned()];
        let stop = AtomicBool::new(false);
        let m = measure(&paths, &stop);
        assert!(m.ntfs);
        assert_eq!((m.files, m.skipped, m.already), (3, 2, 0), "{m:?}");
        assert_eq!(m.estimates.len(), 4, "{m:?}");
        let lzx = m.estimates.iter().find(|e| e.algorithm == "lzx").unwrap().after;
        assert!(lzx < m.bytes / 10, "{m:?}");

        let walk = walk(&paths, &stop, false);
        let report = apply(&walk.candidates, Some(Algorithm::Xpress8k), &stop, &|_| {});
        assert_eq!((report.done, report.failed_count), (3, 0), "{report:?}");
        assert!(report.after < report.before / 10, "{report:?}");
        assert!(wof_compressed(&dir.join("notes.txt")));
        assert!(std::fs::metadata(&locked).unwrap().file_attributes() & ATTR_READONLY != 0);
        // Still reads as before, and keeps its date.
        assert_eq!(std::fs::read_to_string(dir.join("notes.txt")).unwrap(), text);
        assert_eq!(std::fs::metadata(dir.join("notes.txt")).unwrap().modified().unwrap(), modified);

        let again = measure(&paths, &stop);
        assert_eq!((again.files, again.already, again.wof), (0, 3, 3), "{again:?}");

        let id = report.undo.unwrap();
        let files: Vec<(PathBuf, u64)> = UNDO.lock().unwrap().remove(&id).unwrap().into_iter().map(|p| (p, 0)).collect();
        let back = apply(&files, None, &stop, &|_| {});
        assert_eq!((back.done, back.failed_count), (3, 0), "{back:?}");
        assert!(!wof_compressed(&dir.join("notes.txt")));
        assert_eq!(std::fs::read_to_string(dir.join("sub").join("more.log")).unwrap(), text);
        assert_eq!(std::fs::metadata(dir.join("notes.txt")).unwrap().modified().unwrap(), modified);
        assert!(!std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".glint-")));

        let mut perms = std::fs::metadata(&locked).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(&locked, perms).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// How long measuring a big folder takes: `WOF_DIR=<folder> cargo test --lib wof -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_speed_on_a_real_folder() {
        let Ok(dir) = std::env::var("WOF_DIR") else { return };
        let started = Instant::now();
        let m = measure(&[dir], &AtomicBool::new(false));
        eprintln!("{:.0} ms: {m:?}", started.elapsed().as_secs_f64() * 1000.0);
    }
}
