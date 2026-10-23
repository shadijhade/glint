//! Size-pre-filtered 3-stage duplicate file finder and atomic NTFS hardlink deduplicator.
//!
//! Stage 1: Enumerates files via Win32 `FindFirstFileExW` (`fsx::read_dir`), skipping online-only
//!          cloud placeholders, and groups by exact byte size (eliminating ~99% of files with zero reads).
//! Stage 2: Checks physical file identity (`GetFileInformationByHandle`) so existing hardlinks to the
//!          same MFT record are not double-counted, and hashes the first 4 KB + last 4 KB of same-size
//!          candidates.
//! Stage 3: Computes a full BLAKE3 content digest across remaining candidates in parallel.
//!
//! Hard-linking compares each pair byte for byte first, so a file that changed since the scan (or a
//! digest that somehow agreed) is never replaced.

use crate::fsx::{self, blocking};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const EDGE_BYTES: u64 = 4096;
const MAX_SCAN_FILES: usize = 150_000;
const MAX_GROUPS: usize = 500;
/// Windows attributes for cloud placeholders (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS).
const ONLINE_ONLY_ATTRS: u32 = 0x0000_1000 | 0x0040_0000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFile {
    pub path: String,
    pub name: String,
    pub modified: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    pub hash: String,
    pub size: u64,
    pub wasted_bytes: u64,
    pub files: Vec<DuplicateFile>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DedupReport {
    pub root: String,
    pub groups: Vec<DuplicateGroup>,
    pub total_wasted_bytes: u64,
    pub total_duplicate_files: usize,
    pub files_scanned: usize,
    pub size_candidates: usize,
    pub edge_candidates: usize,
    pub hardlinks_skipped: usize,
    pub elapsed_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardlinkResult {
    pub linked: usize,
    pub reclaimed_bytes: u64,
    pub elapsed_ms: f64,
}

static CANCEL_DEDUP: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn cancel_dedup() {
    CANCEL_DEDUP.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub async fn find_duplicates(root: String, min_bytes: Option<u64>) -> Result<DedupReport, String> {
    CANCEL_DEDUP.store(false, Ordering::Relaxed);
    blocking(move || scan_duplicates(&root, min_bytes.unwrap_or(1), &CANCEL_DEDUP)).await
}

#[tauri::command]
pub async fn hardlink_duplicates(keep: String, duplicates: Vec<String>) -> Result<HardlinkResult, String> {
    blocking(move || link_group(&keep, &duplicates)).await
}

#[derive(Clone, Debug)]
struct Candidate {
    path: String,
    name: String,
    size: u64,
    modified: i64,
}

type FileId = (u32, u64);
type DigestKey = (u64, [u8; 32]);
type CandidateGroups = HashMap<DigestKey, Vec<Candidate>>;

/// Returns `(volume_serial, file_index)` on Windows so hardlinks to the same MFT record are detected.
#[cfg(windows)]
fn file_identity(file: &File) -> Option<FileId> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).ok()?;
    }
    let index = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);
    if index == 0 {
        None
    } else {
        Some((info.dwVolumeSerialNumber, index))
    }
}

#[cfg(not(windows))]
fn file_identity(_file: &File) -> Option<(u32, u64)> {
    None
}

fn hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Reads first 4 KB + last 4 KB of `file` and returns `(physical_id, edge_digest)`.
fn edge_digest(path: &str, size: u64) -> Option<(Option<FileId>, [u8; 32])> {
    let mut file = File::open(path).ok()?;
    let id = file_identity(&file);
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());

    let mut buf = [0u8; EDGE_BYTES as usize];
    let n = file.read(&mut buf).ok()?;
    hasher.update(&buf[..n]);

    if size > EDGE_BYTES {
        let tail_pos = size.saturating_sub(EDGE_BYTES);
        if file.seek(SeekFrom::Start(tail_pos)).is_ok() {
            if let Ok(tn) = file.read(&mut buf) {
                hasher.update(&buf[..tn]);
            }
        }
    }
    Some((id, *hasher.finalize().as_bytes()))
}

fn full_digest(path: &str) -> Option<[u8; 32]> {
    let mut file = File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(*hasher.finalize().as_bytes())
}

pub fn scan_duplicates(root: &str, min_bytes: u64, cancel: &AtomicBool) -> Result<DedupReport, String> {
    let start = Instant::now();
    let root_path = Path::new(root);
    if !root_path.exists() {
        return Err("Folder does not exist".into());
    }

    let min_size = min_bytes.max(1);
    let mut by_size: HashMap<u64, Vec<Candidate>> = HashMap::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.trim_end_matches(['\\', '/']).to_string());
    let mut files_scanned = 0usize;

    // Stage 1: Fast Win32 directory enumeration and grouping by exact byte size.
    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if files_scanned >= MAX_SCAN_FILES {
            break;
        }
        let Ok(entries) = fsx::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            // Skip reparse points/symlinks and online-only cloud placeholders.
            if (entry.attrs & 0x0400) != 0 || (entry.attrs & ONLINE_ONLY_ATTRS) != 0 {
                continue;
            }
            let full_path = if dir.ends_with('\\') || dir.ends_with('/') {
                format!("{dir}{}", entry.name)
            } else {
                format!(r"{dir}\{}", entry.name)
            };
            if entry.dir {
                let lower = entry.name.to_ascii_lowercase();
                if lower != ".git" && lower != "node_modules" && lower != "$recycle.bin" && lower != "system volume information" {
                    queue.push_back(full_path);
                }
            } else if entry.size >= min_size {
                files_scanned += 1;
                by_size.entry(entry.size).or_default().push(Candidate {
                    path: full_path,
                    name: entry.name,
                    size: entry.size,
                    modified: entry.modified,
                });
            }
        }
    }

    let same_size: Vec<Candidate> = by_size
        .into_values()
        .filter(|group| group.len() >= 2)
        .flatten()
        .collect();
    let size_candidates = same_size.len();

    // Stage 2: Check physical MFT identity (skip existing hardlinks) and compute 8 KB edge digests.
    let mut seen_ids: HashSet<FileId> = HashSet::new();
    let mut hardlinks_skipped = 0usize;
    let mut by_edge: CandidateGroups = HashMap::new();

    for cand in same_size {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let Some((phys_id, digest)) = edge_digest(&cand.path, cand.size) else {
            continue;
        };
        if let Some(id) = phys_id {
            if !seen_ids.insert(id) {
                hardlinks_skipped += 1;
                continue;
            }
        }
        by_edge.entry((cand.size, digest)).or_default().push(cand);
    }

    let edge_matches: Vec<Candidate> = by_edge
        .into_values()
        .filter(|group| group.len() >= 2)
        .flatten()
        .collect();
    let edge_candidates = edge_matches.len();

    // Stage 3: a full BLAKE3 digest of each candidate's content, in parallel (files of 8 KB or less were
    // all hashed in stage 2 already, and hash the same again here).
    let workers = std::thread::available_parallelism().map(|n| n.get().clamp(2, 8)).unwrap_or(4);
    let work = Arc::new(Mutex::new(edge_matches.into_iter()));
    let by_full: Arc<Mutex<CandidateGroups>> = Arc::new(Mutex::new(HashMap::new()));

    std::thread::scope(|s| {
        for _ in 0..workers {
            let work = Arc::clone(&work);
            let by_full = Arc::clone(&by_full);
            s.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let next = work.lock().unwrap().next();
                let Some(cand) = next else { break };
                let Some(digest) = full_digest(&cand.path) else { continue };
                by_full.lock().unwrap().entry((cand.size, digest)).or_default().push(cand);
            });
        }
    });

    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }

    let full_map = Arc::try_unwrap(by_full).unwrap().into_inner().unwrap();
    let mut groups: Vec<DuplicateGroup> = full_map
        .into_iter()
        .filter(|(_, list)| list.len() >= 2)
        .map(|((size, digest), mut list)| {
            // Sort oldest first so the original file is first by default.
            list.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| a.path.cmp(&b.path)));
            let wasted_bytes = size.saturating_mul((list.len() - 1) as u64);
            DuplicateGroup {
                hash: hex_string(&digest[..16]),
                size,
                wasted_bytes,
                files: list
                    .into_iter()
                    .map(|c| DuplicateFile {
                        path: c.path,
                        name: c.name,
                        modified: c.modified,
                    })
                    .collect(),
            }
        })
        .collect();

    groups.sort_by(|a, b| b.wasted_bytes.cmp(&a.wasted_bytes).then_with(|| b.size.cmp(&a.size)));
    let total_wasted_bytes: u64 = groups.iter().map(|g| g.wasted_bytes).sum();
    let total_duplicate_files: usize = groups.iter().map(|g| g.files.len() - 1).sum();
    groups.truncate(MAX_GROUPS);

    Ok(DedupReport {
        root: root.to_string(),
        groups,
        total_wasted_bytes,
        total_duplicate_files,
        files_scanned,
        size_candidates,
        edge_candidates,
        hardlinks_skipped,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Hard links are for ordinary local files: not cloud placeholders (OneDrive and the like) or other
/// reparse points, whose content a link wouldn't share.
#[cfg(windows)]
fn plain_file(path: &str, meta: &fs::Metadata) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x0400;
    if meta.file_attributes() & (REPARSE_POINT | ONLINE_ONLY_ATTRS) != 0 {
        return Err(format!("“{path}” is a cloud or linked file, which can't be replaced by a hard link"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn plain_file(_path: &str, _meta: &fs::Metadata) -> Result<(), String> {
    Ok(())
}

fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// Compares two files byte for byte.
fn same_content(a: &str, b: &str) -> Result<bool, String> {
    let open = |p: &str| File::open(p).map_err(|e| format!("{p}: {e}"));
    let (mut fa, mut fb) = (open(a)?, open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 256 * 1024], vec![0u8; 256 * 1024]);
    loop {
        let na = read_full(&mut fa, &mut ba).map_err(|e| format!("{a}: {e}"))?;
        let nb = read_full(&mut fb, &mut bb).map_err(|e| format!("{b}: {e}"))?;
        if na != nb || ba[..na] != bb[..nb] {
            return Ok(false);
        }
        if na == 0 {
            return Ok(true);
        }
    }
}

/// Replaces each file in `duplicates` with an NTFS hard link to `keep`. Each one is compared with
/// `keep` byte for byte right before, since it may have changed since the scan; the first that
/// differs stops the batch with nothing of it replaced.
pub fn link_group(keep: &str, duplicates: &[String]) -> Result<HardlinkResult, String> {
    let start = Instant::now();
    let keep_meta = fs::metadata(keep).map_err(|e| format!("{keep}: {e}"))?;
    if !keep_meta.is_file() {
        return Err("Target to keep is not a regular file".into());
    }
    plain_file(keep, &keep_meta)?;
    let keep_size = keep_meta.len();
    let keep_id = File::open(keep).ok().and_then(|f| file_identity(&f));

    let mut linked = 0usize;
    let mut reclaimed_bytes = 0u64;

    for dup in duplicates {
        if dup.eq_ignore_ascii_case(keep) {
            continue;
        }
        let dup_meta = fs::metadata(dup).map_err(|e| format!("{dup}: {e}"))?;
        if !dup_meta.is_file() {
            return Err(format!("“{dup}” is not a regular file"));
        }
        plain_file(dup, &dup_meta)?;
        if dup_meta.len() != keep_size {
            return Err(format!("“{dup}” changed size; aborting hardlink"));
        }
        if let (Some(k_id), Some(d_id)) = (keep_id, File::open(dup).ok().and_then(|f| file_identity(&f))) {
            if k_id == d_id {
                continue;
            }
        }
        if !same_content(keep, dup)? {
            return Err(format!("“{dup}” no longer has the same content as “{keep}”, so it was left as it is"));
        }

        let dup_path = Path::new(dup);
        let tmp_path = dup_path.with_extension(format!("glint-hardlink-tmp-{}", std::process::id()));
        fs::rename(dup_path, &tmp_path).map_err(|e| format!("Couldn't prepare “{dup}”: {e}"))?;

        match fs::hard_link(keep, dup_path) {
            Ok(()) => {
                // A read-only copy can't be deleted until it's writable.
                if fs::remove_file(&tmp_path).is_err() {
                    if let Ok(meta) = fs::metadata(&tmp_path) {
                        let mut perms = meta.permissions();
                        #[allow(clippy::permissions_set_readonly_false)]
                        perms.set_readonly(false);
                        let _ = fs::set_permissions(&tmp_path, perms);
                    }
                    let _ = fs::remove_file(&tmp_path);
                }
                linked += 1;
                reclaimed_bytes += keep_size;
            }
            Err(e) => {
                let _ = fs::rename(&tmp_path, dup_path);
                return Err(format!("Couldn't create hardlink for “{dup}” (must be on the same NTFS volume): {e}"));
            }
        }
    }

    Ok(HardlinkResult {
        linked,
        reclaimed_bytes,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_duplicates_and_links_them_atomically() {
        let dir = std::env::temp_dir().join(format!("glint-dedup-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub")).unwrap();

        let content = vec![0x42u8; 16 * 1024];
        let a = dir.join("alpha.bin");
        let b = dir.join("sub").join("beta.bin");
        let c = dir.join("unique.bin");
        fs::write(&a, &content).unwrap();
        fs::write(&b, &content).unwrap();
        fs::write(&c, b"different size and content").unwrap();

        let cancel = AtomicBool::new(false);
        let report = scan_duplicates(&dir.to_string_lossy(), 1, &cancel).unwrap();
        assert_eq!(report.groups.len(), 1);
        assert_eq!(report.groups[0].files.len(), 2);
        assert_eq!(report.total_wasted_bytes, 16 * 1024);

        // Hardlink `b` to `a` and verify a subsequent scan skips it as an existing hardlink!
        let keep = report.groups[0].files[0].path.clone();
        let dup = report.groups[0].files[1].path.clone();
        let res = link_group(&keep, &[dup]).unwrap();
        assert_eq!(res.linked, 1);
        assert_eq!(res.reclaimed_bytes, 16 * 1024);

        let report_after = scan_duplicates(&dir.to_string_lossy(), 1, &cancel).unwrap();
        assert_eq!(report_after.groups.len(), 0);
        assert_eq!(report_after.hardlinks_skipped, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn never_links_files_that_differ() {
        let dir = std::env::temp_dir().join(format!("glint-dedup-differ-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Same size, same first and last 4 KB, a different byte in the middle: what a stale scan or
        // a digest collision would look like.
        let mut one = vec![0x11u8; 64 * 1024];
        let mut other = one.clone();
        other[32 * 1024] = 0x22;
        one[0] = 0x33;
        other[0] = 0x33;
        let (a, b) = (dir.join("a.bin"), dir.join("b.bin"));
        fs::write(&a, &one).unwrap();
        fs::write(&b, &other).unwrap();

        let err = link_group(&a.to_string_lossy(), &[b.to_string_lossy().into_owned()]).unwrap_err();
        assert!(err.contains("same content"), "{err}");
        assert_eq!(fs::read(&a).unwrap(), one);
        assert_eq!(fs::read(&b).unwrap(), other);

        let _ = fs::remove_dir_all(&dir);
    }
}
