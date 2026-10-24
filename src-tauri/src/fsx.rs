//! File-system access for the explorer: one Win32 enumeration pass per folder (names, sizes,
//! dates and attributes all come with it), natural folders-first sorting, drives and previews.

use serde::Serialize;
#[cfg(test)]
use std::cmp::Ordering;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Serialize, Clone, Debug)]
pub struct Entry {
    pub name: String,
    /// Full path. Only search results carry it; a listing's entries all live in its folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub dir: bool,
    pub size: u64,
    /// Last write time in milliseconds since the Unix epoch, 0 when unknown.
    pub modified: i64,
    /// Windows FILE_ATTRIBUTE_* bits: hidden, system, read-only, online-only…
    pub attrs: u32,
}

/// Runs blocking file-system work off the async runtime's worker threads.
pub async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

pub fn ms_since(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

/// Why a folder couldn't be listed. "denied": Windows doesn't let this user open it (the UI says so
/// in the tab, with Get access unless it's on another computer); anything else is said in words.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ListError {
    pub kind: &'static str,
    pub message: String,
    /// The folder as asked for, made absolute.
    pub path: String,
    /// On another computer, which decides who may open it.
    pub remote: bool,
}

impl ListError {
    fn other(path: &str, message: String) -> ListError {
        ListError { kind: "other", message, path: path.to_string(), remote: false }
    }
}

/// Lists a folder for the UI, sorted, as one binary buffer (see `encode_listing`). A path that goes
/// into a zip or 7z archive lists the folder in the archive. A link Windows won't list, such as
/// "Documents and Settings" (kept for old apps, it lets no one look inside), lists where it points;
/// a folder Windows keeps this user out of lists what instant search's index has in it, if it can.
/// The Recycle Bin (`recycle::PLACE`) lists its items, each with a path of its own (`encode_items`),
/// and a remote connection's folder (`remote:<id>\…`) what's in it there.
#[tauri::command]
pub async fn list_dir(path: String) -> Result<tauri::ipc::Response, ListError> {
    let asked = path.clone();
    if crate::remote::is_remote(path.trim()) {
        return crate::remote::listing(path.trim(), Instant::now()).await.map(tauri::ipc::Response::new).map_err(|e| ListError::other(path.trim(), e));
    }
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        if crate::recycle::is_place(path.trim()) {
            return crate::recycle::listing(started).map(tauri::ipc::Response::new).map_err(|e| ListError::other(crate::recycle::PLACE, e));
        }
        let path = normalize(&path).map_err(|e| ListError::other(&path, e))?;
        if let Some(listing) = crate::archive::list(&path, started) {
            return listing.map(tauri::ipc::Response::new).map_err(|e| ListError::other(&path, e));
        }
        let mut from_index = false;
        let (path, mut entries) = match read_dir(&path) {
            Ok(entries) => (path, entries),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                let remote = on_other_computer(&path);
                if let Some((entries, target)) = link_target(&path).and_then(|target| Some((read_dir(&target).ok()?, target))) {
                    (target, entries)
                } else if let Some(entries) = (!remote).then(|| crate::index::listing(&path)).flatten() {
                    from_index = true;
                    (path, entries)
                } else {
                    return Err(ListError { kind: "denied", message: describe(&e), remote, path });
                }
            }
            // A path into (or to) an archive not named like one: a renamed zip, or one opened with
            // "Open as archive". Its first bytes say so, and it's remembered from then on.
            Err(e) => match crate::archive::sniff(&path).then(|| crate::archive::list(&path, started)).flatten() {
                Some(listing) => return listing.map(tauri::ipc::Response::new).map_err(|e| ListError::other(&path, e)),
                None => return Err(ListError::other(&path, describe(&e))),
            },
        };
        sort_entries(&mut entries);
        Ok(tauri::ipc::Response::new(encode_listing(&path, &entries, ms_since(started), None, from_index)))
    })
    .await
    .map_err(|e| ListError::other(&asked, e.to_string()))?
}

/// Where a junction or symbolic link to a folder points, made absolute; None for anything else.
fn link_target(path: &str) -> Option<String> {
    if stat(path)?.attrs & 0x400 == 0 {
        return None;
    }
    let target = std::fs::read_link(path).ok()?;
    // A relative link is relative to the folder it's in.
    let target = match Path::new(path).parent() {
        Some(parent) => parent.join(target),
        None => target,
    };
    normalize(&target.to_string_lossy()).ok()
}

/// On a network share or a mapped network drive: that computer decides who may open what.
#[cfg(windows)]
pub fn on_other_computer(path: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    const DRIVE_REMOTE: u32 = 4;
    if path.starts_with(r"\\") {
        return !path.starts_with(r"\\?\") || path.starts_with(r"\\?\UNC\");
    }
    path.get(..3).is_some_and(|root| unsafe { GetDriveTypeW(PCWSTR(win::wide_str(root).as_ptr())) } == DRIVE_REMOTE)
}

#[cfg(not(windows))]
pub fn on_other_computer(_: &str) -> bool {
    false
}

/// What a listing shows of each item: a folder's entries, or an archive's.
pub trait Listed {
    fn name(&self) -> &str;
    fn dir(&self) -> bool;
    fn size(&self) -> u64;
    fn modified(&self) -> i64;
    fn attrs(&self) -> u32;
}

impl Listed for Entry {
    fn name(&self) -> &str {
        &self.name
    }
    fn dir(&self) -> bool {
        self.dir
    }
    fn size(&self) -> u64 {
        self.size
    }
    fn modified(&self) -> i64 {
        self.modified
    }
    fn attrs(&self) -> u32 {
        self.attrs
    }
}

/// A listing of a folder in an archive also says where the archive is in its path, and how its
/// list of entries was read.
pub struct InArchive {
    /// UTF-16 units of the listing's path that are the archive's own path (an archive in an archive
    /// included), and those that are the archive file on disk.
    pub archive: u32,
    pub outer: u32,
    /// Files and folders in the whole archive.
    pub entries: u32,
    /// The archive's list of entries was already in memory.
    pub cached: bool,
    /// How long reading it took (0 when it was in memory).
    pub read_ms: f64,
    /// The archive's format, by its code (`archive::Format::code`).
    pub format: u8,
}

/// A listing as bytes rather than JSON: a third of the size for big folders, and nothing to parse.
/// All little-endian, laid out so each array can be viewed in place as a typed array:
///   0: u32 count · 4: u32 path length (UTF-8 bytes) · 8: f64 milliseconds spent
///   16: u32 archive (UTF-16 units of the path that are the archive the folder is in; 0 for a
///   folder on disk) · 20: u32 outer (those that are the archive file on disk, when archives nest)
///   24: u32 entries in the archive · 28: u32 flags (1: the archive's list of entries was in memory;
///   2: listed from instant search's index, as Windows keeps the folder closed to this user; 8: a
///   remote's folder; bits 8 to 15: the archive's format code plus one)
///   32: f64 milliseconds reading the archive's list of entries
///   40: f64 size[count] · then f64 modified[count] · u32 attrs[count] · u32 name length[count]
///   (UTF-16 units, how JavaScript counts) · the path (UTF-8) · every name back to back (UTF-8).
pub fn encode_listing<T: Listed>(path: &str, entries: &[T], elapsed: f64, archive: Option<&InArchive>, from_index: bool) -> Vec<u8> {
    let names: usize = entries.iter().map(|e| e.name().len()).sum();
    let mut out = Vec::with_capacity(40 + 24 * entries.len() + path.len() + names);
    out.extend((entries.len() as u32).to_le_bytes());
    out.extend((path.len() as u32).to_le_bytes());
    out.extend(elapsed.to_le_bytes());
    let (inside, outer, count, flags, read_ms) = archive.map_or((0, 0, 0, 0, 0.0), |a| (a.archive, a.outer, a.entries, a.cached as u32 | (a.format as u32 + 1) << 8, a.read_ms));
    let flags = flags | if from_index { 2 } else { 0 };
    out.extend(inside.to_le_bytes());
    out.extend(outer.to_le_bytes());
    out.extend(count.to_le_bytes());
    out.extend(flags.to_le_bytes());
    out.extend(read_ms.to_le_bytes());
    for e in entries {
        out.extend((e.size() as f64).to_le_bytes());
    }
    for e in entries {
        out.extend((e.modified() as f64).to_le_bytes());
    }
    for e in entries {
        // The directory bit is what the UI reads, so set it on every platform.
        out.extend((e.attrs() | if e.dir() { 0x10 } else { 0 }).to_le_bytes());
    }
    for e in entries {
        out.extend((e.name().encode_utf16().count() as u32).to_le_bytes());
    }
    out.extend(path.as_bytes());
    for e in entries {
        out.extend(e.name().as_bytes());
    }
    out
}

/// Flag 4 in a listing's header: its items each carry their own path (`encode_items`).
const ITEMS: u32 = 4;

/// Flag 8: a folder on a remote connection (remote.rs), read-only, its files downloaded to be read.
const REMOTE: u32 = 8;

/// Marks a listing (`encode_listing`'s) as a remote's.
pub fn mark_remote(listing: &mut [u8]) {
    let flags = u32::from_le_bytes([listing[28], listing[29], listing[30], listing[31]]) | REMOTE;
    listing[28..32].copy_from_slice(&flags.to_le_bytes());
}

/// Items that each have their own path, such as search results, as bytes: `encode_listing`'s header
/// (with flag 4, its archive fields 0, and at 24 a u32 of columns: 1 when names follow the paths, 2
/// when origins do), then
///   f64 size[count] · f64 modified[count] · u32 attrs[count] · u32 path length[count] · u32 name
///   length[count] · u32 origin length[count] when there are origins (UTF-16 units) · the location
///   (UTF-8) · every path back to back (UTF-8) · only when some name isn't its path's last part, every
///   name · and the origins.
/// Otherwise each name is the last `name length` units of its path, which search results always are.
/// `origins`, one per entry or none: where each came from (the Recycle Bin's, where it was deleted from).
pub fn encode_items(location: &str, entries: &[Entry], origins: &[String], elapsed: f64, from_index: bool) -> Vec<u8> {
    let paths: Vec<&str> = entries.iter().map(|e| e.path.as_deref().unwrap_or(&e.name)).collect();
    let tails = entries.iter().zip(&paths).all(|(e, p)| *p == e.name || p.strip_suffix(e.name.as_str()).is_some_and(|rest| rest.ends_with(['\\', '/'])));
    let with_origins = !origins.is_empty();
    let text: usize = paths.iter().map(|p| p.len()).sum::<usize>() + if tails { 0 } else { entries.iter().map(|e| e.name.len()).sum() } + origins.iter().map(|o| o.len()).sum::<usize>();
    let mut out = Vec::with_capacity(40 + 32 * entries.len() + location.len() + text);
    out.extend((entries.len() as u32).to_le_bytes());
    out.extend((location.len() as u32).to_le_bytes());
    out.extend(elapsed.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend((u32::from(!tails) | u32::from(with_origins) << 1).to_le_bytes());
    out.extend((ITEMS | if from_index { 2 } else { 0 }).to_le_bytes());
    out.extend(0f64.to_le_bytes());
    for e in entries {
        out.extend((e.size as f64).to_le_bytes());
    }
    for e in entries {
        out.extend((e.modified as f64).to_le_bytes());
    }
    for e in entries {
        out.extend((e.attrs | if e.dir { 0x10 } else { 0 }).to_le_bytes());
    }
    for path in &paths {
        out.extend((path.encode_utf16().count() as u32).to_le_bytes());
    }
    for e in entries {
        out.extend((e.name.encode_utf16().count() as u32).to_le_bytes());
    }
    for origin in origins {
        out.extend((origin.encode_utf16().count() as u32).to_le_bytes());
    }
    out.extend(location.as_bytes());
    for path in &paths {
        out.extend(path.as_bytes());
    }
    if !tails {
        for e in entries {
            out.extend(e.name.as_bytes());
        }
    }
    for origin in origins {
        out.extend(origin.as_bytes());
    }
    out
}

/// What a change inside an archive is answered with.
pub const READ_ONLY: &str = "Archives open read-only in Glint. Copy items out of the archive to change them.";

#[derive(Serialize)]
pub struct Resolved {
    pub path: String,
    pub dir: bool,
}

/// Checks a typed or pasted location; a file resolves too, so the caller can open its folder. An
/// archive is a folder, as are the folders in it.
#[tauri::command]
pub async fn resolve_path(path: String) -> Result<Resolved, String> {
    if crate::remote::is_remote(path.trim()) {
        return crate::remote::resolve(path.trim()).await;
    }
    blocking(move || {
        if crate::recycle::is_place(path.trim()) {
            return Ok(Resolved { path: crate::recycle::PLACE.into(), dir: true });
        }
        let path = normalize(&path)?;
        // A first read stopped (a big .tar.gz) is "cancelled" as it is: there's nothing to report.
        let archive_error = |e: String| if e == "cancelled" { e } else { format!("Can't open “{path}”. {e}") };
        if let Some(found) = crate::archive::resolve(&path) {
            return found.map_err(archive_error);
        }
        match std::fs::metadata(&path) {
            Ok(meta) => Ok(Resolved { dir: meta.is_dir(), path }),
            Err(_) if crate::archive::sniff(&path) => crate::archive::resolve(&path).unwrap_or_else(|| Err(format!("Can't find “{path}”."))).map_err(archive_error),
            Err(e) => match stat(&path) {
                // Seen from its folder, but its own permissions keep this user out (C:\PerfLogs): it
                // opens, and says so.
                Some(entry) => Ok(Resolved { dir: entry.dir, path }),
                // Its folder can't be looked into either: opened as a folder, which says why.
                None if e.kind() == io::ErrorKind::PermissionDenied => Ok(Resolved { dir: true, path }),
                None => Err(format!("Can't find “{path}”. Check the spelling and try again.")),
            },
        }
    })
    .await
}

/// Why a file couldn't be read, in words (the preview pane shows it).
pub(crate) fn file_error(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "This file doesn't exist (anymore).".into(),
        io::ErrorKind::PermissionDenied => "You don't have permission to read this file.".into(),
        // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        _ if matches!(e.raw_os_error(), Some(32 | 33)) => "Another app is using this file, so it can't be read right now.".into(),
        _ => e.to_string(),
    }
}

fn describe(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "This folder doesn't exist (anymore).".into(),
        io::ErrorKind::PermissionDenied => "You don't have permission to open this folder.".into(),
        _ => e.to_string(),
    }
}

/// Turns what someone typed or pasted into an absolute path: quotes trimmed, %VARS% and ~ expanded,
/// "c:" made "C:\", slashes flipped, and no trailing separator except on drive roots.
pub fn normalize(input: &str) -> Result<String, String> {
    // Not a path on disk, and one Windows would read as a file's stream ("remote" with ":<id>").
    if crate::remote::is_remote(input.trim()) {
        return Err("That's on a remote connection, which Glint only reads".into());
    }
    let trimmed = input.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        return Err("No folder given".into());
    }
    let mut text = expand_vars(trimmed);
    if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
        if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
            text = format!("{}{}", home.to_string_lossy(), &text[1..]);
        }
    }
    if cfg!(windows) {
        text = text.replace('/', "\\");
        // "C:" alone means "the current folder on C:", but people mean the drive.
        if text.len() == 2 && text.ends_with(':') {
            text.push('\\');
        }
    }
    let mut path = std::path::absolute(&text).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
    while path.len() > 1 && path.ends_with(['\\', '/']) && !is_drive_root(&path) {
        path.pop();
    }
    if path.as_bytes().get(1) == Some(&b':') {
        path[..1].make_ascii_uppercase();
    }
    Ok(path)
}

fn is_drive_root(path: &str) -> bool {
    path.len() == 3 && path.ends_with(":\\")
}

/// Expands Windows-style %NAME% variables; unknown names are left as typed.
fn expand_vars(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('%') else { break };
        match std::env::var(&after[..end]) {
            Ok(value) if end > 0 => {
                out.push_str(&rest[..start]);
                out.push_str(&value);
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Folders first, then names in natural order ("file2" before "file10"), ignoring case like Explorer.
pub fn sort_entries(entries: &mut Vec<Entry>) {
    let mut keyed: Vec<(Vec<u8>, Entry)> = entries.drain(..).map(|e| (natural_key(&e.name.to_lowercase()), e)).collect();
    keyed.sort_unstable_by(|(ka, a), (kb, b)| b.dir.cmp(&a.dir).then_with(|| ka.cmp(kb)).then_with(|| a.name.cmp(&b.name)));
    entries.extend(keyed.into_iter().map(|(_, e)| e));
}

/// A key whose plain byte order is natural order, so sorting compares keys with memcmp instead of
/// parsing numbers on every comparison: each digit run becomes '0', its length, then its digits
/// without leading zeros. Since digits are the only bytes from '0' to '9', a run still sorts against
/// other characters exactly where a digit would. Same order as `natural_cmp`. `lower` is lowercase;
/// ASCII letters are lowercased here too, so an ASCII name needn't be copied first.
pub(crate) fn natural_key(lower: &str) -> Vec<u8> {
    let bytes = lower.as_bytes();
    let mut key = Vec::with_capacity(bytes.len() + 4);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let digits = trim_zeros(&bytes[start..i]);
            key.push(b'0');
            key.push(digits.len().min(255) as u8);
            key.extend_from_slice(digits);
        } else {
            key.push(bytes[i].to_ascii_lowercase());
            i += 1;
        }
    }
    key
}

/// Compares strings with digit runs compared as numbers. Byte order elsewhere is code point order.
#[cfg(test)]
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (na, nb) = (trim_zeros(&a[si..i]), trim_zeros(&b[sj..j]));
            let ord = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if ord != Ordering::Equal {
                return ord;
            }
        } else {
            if a[i] != b[j] {
                return a[i].cmp(&b[j]);
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let start = digits.iter().position(|&d| d != b'0').unwrap_or(digits.len());
    &digits[start..]
}

/// Builds a search result from a path, with metadata when it can still be read.
pub fn entry_for_path(path: &str, dir_hint: bool) -> Entry {
    let name = Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned());
    let mut entry = stat(path).unwrap_or(Entry { name: String::new(), path: None, dir: dir_hint, size: 0, modified: 0, attrs: 0 });
    entry.name = name;
    entry.path = Some(path.to_string());
    entry
}

#[cfg(windows)]
pub use win::{probe, read_dir, stat, stat_sized};
#[cfg(windows)]
pub(crate) use win::{io_error, wide};

#[cfg(windows)]
mod win {
    use super::Entry;
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, FILETIME};
    use windows::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, GetFileAttributesExW,
        GetFileExInfoStandard, FILE_ATTRIBUTE_DIRECTORY, FIND_FIRST_EX_LARGE_FETCH, WIN32_FILE_ATTRIBUTE_DATA,
        WIN32_FIND_DATAW,
    };

    pub fn wide(path: &Path) -> Vec<u16> {
        long_path(path).as_os_str().encode_wide().chain(Some(0)).collect()
    }

    pub fn wide_str(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    /// Paths past MAX_PATH only work with the \\?\ prefix (the app isn't long-path aware).
    fn long_path(path: &Path) -> PathBuf {
        let text = path.to_string_lossy();
        if text.len() < 248 || text.starts_with(r"\\?\") {
            path.to_path_buf()
        } else if let Some(unc) = text.strip_prefix(r"\\") {
            PathBuf::from(format!(r"\\?\UNC\{unc}"))
        } else {
            PathBuf::from(format!(r"\\?\{text}"))
        }
    }

    pub fn io_error(e: windows::core::Error) -> io::Error {
        let code = e.code().0 as u32;
        if code & 0xFFFF_0000 == 0x8007_0000 {
            io::Error::from_raw_os_error((code & 0xFFFF) as i32)
        } else {
            io::Error::other(e.message())
        }
    }

    fn unix_ms(time: FILETIME) -> i64 {
        let ticks = ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
        if ticks == 0 {
            0
        } else {
            (ticks / 10_000) as i64 - 11_644_473_600_000
        }
    }

    fn entry(name: String, attrs: u32, size_high: u32, size_low: u32, modified: FILETIME) -> Entry {
        let dir = attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        let size = if dir { 0 } else { ((size_high as u64) << 32) | size_low as u64 };
        Entry { name, path: None, dir, size, modified: unix_ms(modified), attrs }
    }

    /// FindFirstFileExW with FindExInfoBasic (no 8.3 names) and LARGE_FETCH: 20–35% faster than std::fs::read_dir.
    pub fn read_dir(dir: &str) -> io::Result<Vec<Entry>> {
        let pattern = wide(&Path::new(dir).join("*"));
        let mut data = WIN32_FIND_DATAW::default();
        let found = unsafe {
            FindFirstFileExW(
                PCWSTR(pattern.as_ptr()),
                FindExInfoBasic,
                &mut data as *mut _ as *mut _,
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_LARGE_FETCH,
            )
        };
        let handle = match found {
            Ok(handle) => handle,
            // Drive roots have no "." and ".." entries, so an empty one finds nothing at all.
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() && Path::new(dir).is_dir() => return Ok(Vec::new()),
            Err(e) => return Err(io_error(e)),
        };
        let mut entries = Vec::new();
        let result = loop {
            let len = data.cFileName.iter().position(|&c| c == 0).unwrap_or(data.cFileName.len());
            let name = String::from_utf16_lossy(&data.cFileName[..len]);
            if name != "." && name != ".." {
                entries.push(entry(name, data.dwFileAttributes, data.nFileSizeHigh, data.nFileSizeLow, data.ftLastWriteTime));
            }
            if let Err(e) = unsafe { FindNextFileW(handle, &mut data) } {
                break if e.code() == ERROR_NO_MORE_FILES.to_hresult() { Ok(()) } else { Err(io_error(e)) };
            }
        };
        unsafe {
            let _ = FindClose(handle);
        }
        result.map(|_| entries)
    }

    /// Whether `read_dir` can list `dir`, from its first entry only, and why not.
    pub fn probe(dir: &str) -> io::Result<()> {
        let pattern = wide(&Path::new(dir).join("*"));
        let mut data = WIN32_FIND_DATAW::default();
        let found = unsafe { FindFirstFileExW(PCWSTR(pattern.as_ptr()), FindExInfoBasic, &mut data as *mut _ as *mut _, FindExSearchNameMatch, None, Default::default()) };
        match found {
            Ok(handle) => {
                unsafe {
                    let _ = FindClose(handle);
                }
                Ok(())
            }
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() && Path::new(dir).is_dir() => Ok(()),
            Err(e) => Err(io_error(e)),
        }
    }

    /// Metadata without opening the file (GetFileAttributesExW), so it's cheap and never triggers a cloud download.
    pub fn stat(path: &str) -> Option<Entry> {
        stat_sized(path).map(|(entry, _)| entry)
    }

    /// `stat`, and the size Windows gives the item itself: for a folder, which `stat` sizes as 0, its own
    /// entry (its index of names: none for the smallest on NTFS, 4 KB and up for the rest).
    pub fn stat_sized(path: &str) -> Option<(Entry, u64)> {
        let mut data = WIN32_FILE_ATTRIBUTE_DATA::default();
        let wide = wide(Path::new(path));
        unsafe { GetFileAttributesExW(PCWSTR(wide.as_ptr()), GetFileExInfoStandard, &mut data as *mut _ as *mut _) }.ok()?;
        let size = ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
        Some((entry(String::new(), data.dwFileAttributes, data.nFileSizeHigh, data.nFileSizeLow, data.ftLastWriteTime), size))
    }
}

#[cfg(not(windows))]
pub fn read_dir(dir: &str) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(dir)?.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let meta = item.metadata().ok();
        entries.push(portable_entry(name, meta.as_ref()));
    }
    Ok(entries)
}

#[cfg(not(windows))]
pub fn probe(dir: &str) -> io::Result<()> {
    std::fs::read_dir(dir).map(|_| ())
}

#[cfg(not(windows))]
pub fn stat(path: &str) -> Option<Entry> {
    stat_sized(path).map(|(entry, _)| entry)
}

#[cfg(not(windows))]
pub fn stat_sized(path: &str) -> Option<(Entry, u64)> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    let name = Path::new(path).file_name()?.to_string_lossy().into_owned();
    Some((portable_entry(name, Some(&meta)), meta.len()))
}

#[cfg(not(windows))]
fn portable_entry(name: String, meta: Option<&std::fs::Metadata>) -> Entry {
    let modified = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64);
    // Dotfiles get Windows' hidden bit, so "show hidden items" works the same everywhere.
    let attrs = if name.starts_with('.') { 0x2 } else { 0 };
    let dir = meta.is_some_and(|m| m.is_dir());
    Entry { name, path: None, dir, size: if dir { 0 } else { meta.map_or(0, |m| m.len()) }, modified, attrs }
}

#[derive(Serialize)]
pub struct Drive {
    /// Root path, e.g. "C:\".
    pub path: String,
    pub label: String,
    /// fixed, removable, network, cdrom, ram or unknown.
    pub kind: &'static str,
    pub fs: String,
    pub total: u64,
    pub free: u64,
}

#[tauri::command]
pub async fn list_drives() -> Result<Vec<Drive>, String> {
    blocking(|| Ok(drives())).await
}

#[cfg(windows)]
fn drives() -> Vec<Drive> {
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::Win32::Storage::FileSystem::GetLogicalDrives;

    let mask = unsafe { GetLogicalDrives() };
    let letters: Vec<char> = (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i) as char).collect();
    // Each drive is asked on its own thread: a sleeping network share or an empty card reader
    // can take seconds to answer, and shouldn't hold up the others.
    let (tx, rx) = mpsc::channel();
    for (index, &letter) in letters.iter().enumerate() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((index, drive_info(letter)));
        });
    }
    drop(tx);
    let deadline = Instant::now() + Duration::from_millis(800);
    let mut found: Vec<Option<Drive>> = letters.iter().map(|_| None).collect();
    while let Ok((index, drive)) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        found[index] = Some(drive);
    }
    letters
        .iter()
        .zip(found)
        .map(|(letter, drive)| {
            drive.unwrap_or_else(|| Drive {
                path: format!("{letter}:\\"),
                label: String::new(),
                kind: "unknown",
                fs: String::new(),
                total: 0,
                free: 0,
            })
        })
        .collect()
}

#[cfg(windows)]
fn drive_info(letter: char) -> Drive {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationW};
    use windows::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, SEM_FAILCRITICALERRORS};

    let path = format!("{letter}:\\");
    let root = win::wide_str(&path);
    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    let (mut free, mut total) = (0u64, 0u64);
    let kind = unsafe {
        // No "There is no disk in the drive" dialogs for empty card readers.
        let _ = SetThreadErrorMode(SEM_FAILCRITICALERRORS, None);
        let kind = match GetDriveTypeW(PCWSTR(root.as_ptr())) {
            2 => "removable",
            3 => "fixed",
            4 => "network",
            5 => "cdrom",
            6 => "ram",
            _ => "unknown",
        };
        let _ = GetVolumeInformationW(PCWSTR(root.as_ptr()), Some(&mut label), None, None, None, Some(&mut fs));
        let _ = GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free as *mut u64), Some(&mut total as *mut u64), None);
        kind
    };
    let text = |buf: &[u16]| String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())]);
    let mut label = text(&label);
    if kind == "network" && label.is_empty() {
        // Mapped drives are best known by their share, e.g. \\server\projects.
        let local = win::wide_str(&format!("{letter}:"));
        let mut remote = [0u16; 512];
        let mut len = remote.len() as u32;
        if unsafe { WNetGetConnectionW(PCWSTR(local.as_ptr()), Some(PWSTR(remote.as_mut_ptr())), &mut len) }.is_ok() {
            label = text(&remote);
        }
    }
    Drive { path, label, kind, fs: text(&fs), total, free }
}

#[cfg(not(windows))]
fn drives() -> Vec<Drive> {
    vec![Drive { path: "/".into(), label: "Root".into(), kind: "fixed", fs: String::new(), total: 0, free: 0 }]
}

#[derive(Serialize)]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
    pub binary: bool,
}

/// The start of a file as text for the preview pane: BOM-aware, and binary files are detected, not decoded.
#[tauri::command]
pub async fn read_text_preview(path: String, max_bytes: usize) -> Result<TextPreview, String> {
    // A remote's file: its first bytes, asked for as a range.
    if crate::remote::is_remote(&path) {
        let (bytes, truncated) = crate::remote::read_prefix(&path, max_bytes.clamp(1024, 4 << 20)).await?;
        return Ok(decode_text(&bytes, truncated));
    }
    blocking(move || {
        let limit = max_bytes.clamp(1024, 4 << 20);
        if let Some(read) = crate::archive::read_prefix(&path, limit) {
            let (bytes, truncated) = read?;
            return Ok(decode_text(&bytes, truncated));
        }
        let file = std::fs::File::open(&path).map_err(file_error)?;
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes).map_err(file_error)?;
        let truncated = bytes.len() > limit;
        bytes.truncate(limit);
        Ok(decode_text(&bytes, truncated))
    })
    .await
}

pub fn decode_text(bytes: &[u8], truncated: bool) -> TextPreview {
    let utf16 = |big_endian: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| if big_endian { u16::from_be_bytes(pair) } else { u16::from_le_bytes(pair) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    let text = match bytes {
        [0xFF, 0xFE, ..] => utf16(false),
        [0xFE, 0xFF, ..] => utf16(true),
        _ => {
            let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            if body.iter().take(8000).any(|&b| b == 0) {
                return TextPreview { text: String::new(), truncated, binary: true };
            }
            String::from_utf8_lossy(body).into_owned()
        }
    };
    TextPreview { text, truncated, binary: false }
}

/// Creates a folder or an empty file in `parent`, numbering the name like Explorer ("New folder (2)") if it's taken.
#[tauri::command]
pub async fn create_entry(parent: String, name: String, dir: bool) -> Result<String, String> {
    blocking(move || create_numbered(&parent, &name, dir)).await
}

/// What [`create_entry`] does, for Compress (a zip) and Extract here (a folder) too.
pub fn create_numbered(parent: &str, name: &str, dir: bool) -> Result<String, String> {
    if name.is_empty() || name.contains(['\\', '/', ':']) || name == "." || name == ".." {
        return Err("That name isn't allowed".into());
    }
    if crate::archive::is_read_only(parent) {
        return Err(READ_ONLY.into());
    }
    for n in 1..10_000 {
        let path = Path::new(parent).join(numbered(name, dir, n));
        let created = if dir {
            std::fs::create_dir(&path)
        } else {
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map(|_| ())
        };
        match created {
            Ok(()) => return Ok(path.to_string_lossy().into_owned()),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Couldn't find a free name".into())
}

/// `name`, or its `n`th form: "name (2).txt" (a folder's whole name is its stem).
pub(crate) fn numbered(name: &str, dir: bool, n: u32) -> String {
    if n == 1 {
        return name.to_string();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if !dir && dot > 0 => name.split_at(dot),
        _ => (name, ""),
    };
    format!("{stem} ({n}){ext}")
}

/// Moves `from` into `parent` as `name`, or as "name (2)" and on when that's taken: nothing's replaced.
pub fn move_numbered(from: &Path, parent: &Path, name: &str, dir: bool) -> Result<PathBuf, String> {
    for n in 1..10_000 {
        let to = parent.join(numbered(name, dir, n));
        match crate::batch_rename::move_no_replace(from, &to) {
            Ok(()) => return Ok(to),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(file_error(e)),
        }
    }
    Err("Couldn't find a free name".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        let mut entries: Vec<Entry> = list
            .iter()
            .map(|n| Entry { name: n.trim_end_matches('/').into(), path: None, dir: n.ends_with('/'), size: 0, modified: 0, attrs: 0 })
            .collect();
        sort_entries(&mut entries);
        entries.into_iter().map(|e| e.name).collect()
    }

    #[test]
    fn sorts_folders_first_in_natural_order() {
        assert_eq!(
            names(&["file10.txt", "File2.txt", "b/", "file1.txt", "A/", "file02.txt", "zeta"]),
            ["A", "b", "file1.txt", "File2.txt", "file02.txt", "file10.txt", "zeta"]
        );
    }

    #[test]
    fn compares_digit_runs_as_numbers() {
        assert_eq!(natural_cmp("img9", "img10"), Ordering::Less);
        assert_eq!(natural_cmp("v1.10", "v1.9"), Ordering::Greater);
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("99999999999999999999999", "100000000000000000000000"), Ordering::Less);
        assert_eq!(natural_cmp("é", "z"), Ordering::Greater);
    }

    #[test]
    fn sort_keys_order_like_natural_cmp() {
        let samples = [
            "", "a", "a1", "a01", "a1b", "a12", "a2", "a/", "a:", "a 1", "a-1", "a.1", "a_1", "file9", "file10", "file010",
            "v1.9", "v1.10", "0", "00", "000x", "9", "10", "1a", "x99y", "x100y", "é", "z", "~", "ab", "a1.txt", "a1-copy",
        ];
        for a in samples {
            for b in samples {
                assert_eq!(natural_key(a).cmp(&natural_key(b)), natural_cmp(a, b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn encodes_listings_for_the_ui() {
        let entries = [
            Entry { name: "sub".into(), path: None, dir: true, size: 0, modified: 5, attrs: 0 },
            Entry { name: "é😀.txt".into(), path: None, dir: false, size: 1 << 40, modified: 1_700_000_000_000, attrs: 0x2 },
        ];
        let archive = InArchive { archive: 3, outer: 2, entries: 50_000, cached: true, read_ms: 12.5, format: 1 };
        let bytes = encode_listing("C:\\x", &entries, 1.5, Some(&archive), false);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let f64_at = |at: usize| f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        assert_eq!((u32_at(0), u32_at(4), f64_at(8)), (2, 4, 1.5));
        // Flags: in memory (1), and the format's code plus one in bits 8 to 15 (a 7z: 2).
        assert_eq!((u32_at(16), u32_at(20), u32_at(24), u32_at(28), f64_at(32)), (3, 2, 50_000, 1 | 2 << 8, 12.5));
        assert_eq!((f64_at(40), f64_at(48)), (0.0, (1u64 << 40) as f64));
        assert_eq!((f64_at(56), f64_at(64)), (5.0, 1_700_000_000_000.0));
        assert_eq!((u32_at(72), u32_at(76)), (0x10, 0x2));
        // "é😀.txt" is 7 UTF-16 units: é, a surrogate pair, then ".txt".
        assert_eq!((u32_at(80), u32_at(84)), (3, 7));
        assert_eq!(std::str::from_utf8(&bytes[88..]).unwrap(), "C:\\xsubé😀.txt");
        // A folder on disk has zeros where the archive's details go.
        let plain = encode_listing("D:\\", &entries[..0], 0.5, None, false);
        assert_eq!(plain.len(), 40 + 3);
        assert!(plain[16..40].iter().all(|&b| b == 0));
        // One listed from the index says so in its flags.
        let closed = encode_listing("C:\\PerfLogs", &entries, 0.5, None, true);
        assert_eq!(u32::from_le_bytes(closed[28..32].try_into().unwrap()), 2);
    }

    #[test]
    fn encodes_items_with_their_own_paths() {
        let entries = [
            Entry { name: "é😀.txt".into(), path: Some("C:\\x\\é😀.txt".into()), dir: false, size: 7, modified: 9, attrs: 0x20 },
            Entry { name: "D:\\".into(), path: Some("D:\\".into()), dir: true, size: 0, modified: 0, attrs: 0 },
        ];
        let bytes = encode_items("", &entries, &[], 1.5, false);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let f64_at = |at: usize| f64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        assert_eq!((u32_at(0), u32_at(4), f64_at(8)), (2, 0, 1.5));
        // No names column (each is its path's end), and flag 4: items.
        assert_eq!((u32_at(16), u32_at(20), u32_at(24), u32_at(28)), (0, 0, 0, 4));
        assert_eq!((f64_at(40), f64_at(48), f64_at(56), f64_at(64)), (7.0, 0.0, 9.0, 0.0));
        assert_eq!((u32_at(72), u32_at(76)), (0x20, 0x10));
        // Paths, then names, in UTF-16 units: "C:\x\é😀.txt" is 12.
        assert_eq!((u32_at(80), u32_at(84), u32_at(88), u32_at(92)), (12, 3, 7, 3));
        assert_eq!(std::str::from_utf8(&bytes[96..]).unwrap(), "C:\\x\\é😀.txtD:\\");
        // A name that isn't its path's end goes after the paths, and the columns say so.
        let recycled = [Entry { name: "budget.xlsx".into(), path: Some("C:\\$Recycle.Bin\\$RQ2.xlsx".into()), dir: false, size: 1, modified: 1, attrs: 0 }];
        let bytes = encode_items("", &recycled, &[], 0.0, true);
        assert_eq!((u32::from_le_bytes(bytes[24..28].try_into().unwrap()), u32::from_le_bytes(bytes[28..32].try_into().unwrap())), (1, 4 | 2));
        assert!(std::str::from_utf8(&bytes[40 + 28..]).unwrap().ends_with("$RQ2.xlsxbudget.xlsx"));
        // Where each came from, after the names, its length after theirs.
        let bytes = encode_items("shell:RecycleBinFolder", &recycled, &["C:\\Users\\me\\Désktop".into()], 0.0, false);
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!((u32_at(4), u32_at(24)), (22, 1 | 2));
        assert_eq!((u32_at(40 + 20), u32_at(40 + 24), u32_at(40 + 28)), (25, 11, 19));
        assert_eq!(std::str::from_utf8(&bytes[40 + 32..]).unwrap(), "shell:RecycleBinFolderC:\\$Recycle.Bin\\$RQ2.xlsxbudget.xlsxC:\\Users\\me\\Désktop");
    }

    #[test]
    fn expands_variables() {
        std::env::set_var("SHADUI_TEST_VAR", "X");
        assert_eq!(expand_vars("%SHADUI_TEST_VAR%\\a"), "X\\a");
        assert_eq!(expand_vars("%NOPE_NOT_SET%\\a%"), "%NOPE_NOT_SET%\\a%");
        assert_eq!(expand_vars("100% done"), "100% done");
    }

    #[cfg(windows)]
    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize("c:").unwrap(), "C:\\");
        assert_eq!(normalize(" \"C:/Windows/\" ").unwrap(), "C:\\Windows");
        assert_eq!(normalize("C:\\Windows\\System32\\..").unwrap(), "C:\\Windows");
        assert_eq!(normalize("C:\\").unwrap(), "C:\\");
        assert!(normalize("  ").is_err());
    }

    #[test]
    fn decodes_text_previews() {
        assert_eq!(decode_text(b"\xEF\xBB\xBFhi", false).text, "hi");
        assert_eq!(decode_text(b"\xFF\xFEh\0i\0", false).text, "hi");
        assert!(decode_text(b"MZ\0\0\x03", false).binary);
    }

    #[test]
    fn lists_and_stats_a_folder() {
        let dir = std::env::temp_dir().join(format!("shadui-fsx-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        let mut entries = read_dir(dir.to_str().unwrap()).unwrap();
        sort_entries(&mut entries);
        assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.dir, e.size)).collect::<Vec<_>>(), [("sub", true, 0), ("a.txt", false, 5)]);
        assert!(entries[1].modified > 1_600_000_000_000);
        let file = entry_for_path(dir.join("a.txt").to_str().unwrap(), false);
        assert_eq!((file.name.as_str(), file.size, file.dir), ("a.txt", 5, false));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
