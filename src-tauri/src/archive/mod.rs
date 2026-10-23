//! Archives opened like folders, read-only: zip, 7z, RAR, cabinets, installer packages, tar and
//! single compressed files. A path goes on into an archive the way it goes into a folder
//! (`C:\Downloads\photos.zip\2024\summer`),
//! and archives inside archives open the same way. An archive's list of entries (the zip central
//! directory, the 7z header, a tar's headers) is read once, without extracting anything, into a tree
//! of folders kept in memory while it's in use, so every folder in it lists in well under a
//! millisecond after the first.
//!
//! Entries are extracted only when something needs their bytes: one at a time for previews,
//! thumbnails and opening a file (into this run's temp folder, see `extract`), or a selection at a
//! time into a staging folder that Windows' file engine copies from (copy, copy to the other pane,
//! drag out), so progress and *Replace or Skip* work as for any other copy.
//!
//! Safety: names are made Windows-safe and every path is kept inside the folder it's extracted to
//! (no "zip slip"); each entry is cut off at its declared size, entries that expand more than 1,000
//! times (past 64 MB) and zips whose entries overlap are refused as bombs; extractions that wouldn't
//! fit on the drive don't start; encrypted entries and unknown methods get a clear message; and a
//! damaged archive is an error, never a crash (every read runs under `catch_unwind`).

mod cab;
mod codec;
mod compress;
mod extract;
mod msi;
mod ncdu;
mod rar;
mod read;
mod stream;
mod tar;
mod tree;

use crate::fsx::{self, ms_since, Entry, InArchive, Resolved};
pub use read::Format;
use read::{ItemIndex, Source};
use serde::Serialize;
use shadui_index::proto::{UsageNode, NO_PARENT};
use shadui_index::query::{fold, is_hidden, parse, Excludes, NameMatcher};
use std::collections::HashMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tree::{Tree, ROOT};

/// An entry expanding more than this many times over what's stored of it...
const MAX_RATIO: u64 = 1_000;
/// ...is taken for a bomb once it's bigger than this (smaller ones can't fill a disk).
const RATIO_FLOOR: u64 = 64 << 20;
/// How much a thumbnail may take to extract (its size, plus what comes before it in a solid 7z block).
const THUMB_BUDGET: u64 = 64 << 20;
/// What the preview pane, Quick Look and text previews may take: they extract as you select.
const PREVIEW_BUDGET: u64 = 1 << 30;
/// An archive inside an archive is extracted to open it, up to this size.
const NESTED_MAX: u64 = 4 << 30;
/// Entries kept in memory across the archives opened, most recently used kept first.
const CACHE_NODES: usize = 3_000_000;
/// Extracted copies for previews, thumbnails and opening, kept for reuse while the app runs.
const COPIES_KEPT: usize = 2_000;

/// Why something in an archive couldn't be read or extracted.
#[derive(Debug, Clone)]
pub enum Failure {
    /// The entry is password-protected.
    Encrypted,
    /// A 7z whose list of entries is itself encrypted.
    EncryptedList,
    /// Compressed with a method Glint doesn't have ("MP3", "Quantum").
    Unsupported(String),
    /// Something else Glint doesn't read ("it's a sparse file…").
    Unreadable(String),
    Damaged(String),
    Bomb(String),
    /// Bigger (or deeper in a solid block) than `what` may extract.
    TooBig { size: u64, what: &'static str },
    NoSpace { need: u64, free: u64 },
    TooManyEntries(usize),
    NotFound,
    NotAFolder,
    Cancelled,
    Io(String),
}

impl From<io::Error> for Failure {
    fn from(e: io::Error) -> Self {
        Failure::from_io(&e)
    }
}

impl Failure {
    pub fn from_io(e: &io::Error) -> Failure {
        match e.kind() {
            io::ErrorKind::NotFound => Failure::NotFound,
            io::ErrorKind::UnexpectedEof => Failure::Damaged("it ends too early (is it complete?)".into()),
            _ => Failure::Io(e.to_string()),
        }
    }

    /// What to tell the user, as a sentence about the entry or archive in question.
    pub fn message(&self) -> String {
        match self {
            Failure::Encrypted => "It's password-protected, and Glint can't open encrypted files in archives yet.".into(),
            Failure::EncryptedList => "This archive is password-protected, its list of files included, and Glint can't open encrypted archives yet.".into(),
            Failure::Unsupported(what) => format!("It's compressed with {what}, which Glint can't read."),
            Failure::Unreadable(what) => format!("Glint can't read it: {what}."),
            Failure::Damaged(what) => format!("The archive is damaged: {what}."),
            Failure::Bomb(what) => format!("Glint won't extract it, because it looks like a zip bomb: {what}."),
            Failure::TooBig { size, what } => format!("It's too big to {what} from inside the archive ({}). Copy it out first.", size_text(*size)),
            Failure::NoSpace { need, free } => format!("There isn't enough free space: it needs {}, and {} is free.", size_text(*need), size_text(*free)),
            Failure::TooManyEntries(n) => format!("This archive has {} entries, more than Glint opens ({}).", count_text(*n as u64), count_text(read::MAX_ENTRIES as u64)),
            Failure::NotFound => "It isn't there (anymore).".into(),
            Failure::NotAFolder => "It's a file, not a folder.".into(),
            Failure::Cancelled => "cancelled".into(),
            Failure::Io(text) => text.clone(),
        }
    }
}

/// "1.5 GB", like the UI's sizes.
fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// "17,543,859".
fn count_text(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Runs `f`, turning a panic in an archive reader into an error: a damaged archive mustn't take the app down.
fn guarded<T>(f: impl FnOnce() -> Result<T, Failure>) -> Result<T, Failure> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(Failure::Damaged("Glint couldn't make sense of it".into())))
}

/// Whether Enter opens files with this name like folders (zip, 7z…). A .docx or .jar is a zip too,
/// but Enter opens it in its app; "Open as archive" looks inside.
pub fn is_archive_name(name: &str) -> bool {
    Format::of(name).is_some()
}

/// Where a path goes into an archive: the archive file on disk, the path inside it ("\" between
/// folders, "" for its top), and how the archive is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub outer: String,
    pub inner: String,
    pub format: Format,
}

/// The archive file on disk that `path` goes into (or is). None for anything else, including a
/// folder named like an archive. Only parts of the path named like archives (or found to be one by
/// `sniff`) are looked at on disk, so plain folders cost nothing.
pub fn locate(path: &str) -> Option<Located> {
    let any_sniffed = !state().sniffed.is_empty();
    let mut start = 0;
    loop {
        let end = path[start..].find(['\\', '/']).map_or(path.len(), |i| start + i);
        let claimed = Format::claimed(&path[start..end]).map(|(format, _)| format);
        let format = match claimed {
            Some(format) => fsx::stat(&path[..end]).is_some_and(|e| !e.dir).then_some(format),
            None if any_sniffed => sniffed(&path[..end]),
            None => None,
        };
        if let Some(format) = format {
            let inner = path.get(end + 1..).unwrap_or("").trim_matches(['\\', '/']).replace('/', "\\");
            return Some(Located { outer: path[..end].to_string(), inner, format });
        }
        if end >= path.len() {
            return None;
        }
        start = end + 1;
    }
}

/// A file `sniff` found to be an archive, if it's still the same file.
fn sniffed(path: &str) -> Option<Format> {
    let key = path.to_lowercase();
    let (stamp, format) = *state().sniffed.get(&key)?;
    if stamp_of(path).ok() == Some(stamp) {
        return Some(format);
    }
    state().sniffed.remove(&key);
    None
}

/// What a file is by its first bytes, if it's an archive Glint reads. A compound file (as MSI
/// packages are, and old Office documents) is a package only if its root says so.
fn detect_file(path: &Path) -> Option<Format> {
    let mut head = Vec::with_capacity(4096);
    std::fs::File::open(path).ok()?.take(4096).read_to_end(&mut head).ok()?;
    Format::detect(&head).or_else(|| (head.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") && msi::is_package(path)).then_some(Format::Msi))
}

/// For a path that leads into or is a file not named like an archive (a renamed zip, a .bin that's
/// a 7z, an archive opened with "Open as archive"): reads the first bytes of the deepest file on the
/// way, and remembers it when it's an archive, so `locate` finds it from then on. True when it is one.
/// Called only once a path fails as a folder, so plain folders never pay for it.
pub fn sniff(path: &str) -> bool {
    let mut end = path.trim_end_matches(['\\', '/']).len();
    loop {
        let prefix = &path[..end];
        if let Some(found) = fsx::stat(prefix) {
            if found.dir || Format::claimed(prefix).is_some() {
                return false;
            }
            let Some(format) = detect_file(Path::new(prefix)) else { return false };
            state().sniffed.insert(prefix.to_lowercase(), ((found.size, found.modified), format));
            return true;
        }
        match prefix.rfind(['\\', '/']) {
            Some(at) if at > 2 => end = at,
            _ => return false,
        }
    }
}

/// Whether `path` is something inside an archive (not the archive file itself).
pub fn is_inside(path: &str) -> bool {
    locate(path).is_some_and(|found| !found.inner.is_empty())
}

/// Whether files can't be written into `path`: an archive, or a folder in one.
pub fn is_read_only(path: &str) -> bool {
    locate(path).is_some()
}

/// An archive read into memory: its entries as a tree, and how to get at their bytes.
pub struct Opened {
    /// Its own path as the UI writes it: the archive file, or an entry of another archive.
    path: String,
    /// The archive file on disk everything comes from, and its size and date when this was read.
    outer: String,
    stamp: (u64, i64),
    /// Where its bytes are: the archive file itself, or a temp copy of an archive in another one.
    file: PathBuf,
    format: Format,
    source: Source,
    tree: Tree,
    /// How long reading its list of entries took.
    read_ms: f64,
}

/// What's read in one go: an archive (lowercase path) and a 7z block in it (true, its index) or a zip entry (false, its index).
type Unit = (String, (bool, usize));

#[derive(Default)]
struct State {
    /// Archives read, least recently used first.
    opened: Vec<Arc<Opened>>,
    /// Single entries extracted for previews, thumbnails and opening, by archive, entry and stamp.
    copies: HashMap<String, PathBuf>,
    copy_order: Vec<String>,
    /// A lock for each 7z block (or zip entry) being extracted for previews and thumbnails.
    reading: HashMap<Unit, Arc<Mutex<()>>>,
    /// Running copies out of archives, by the tab that started each, to stop them.
    stages: HashMap<String, Arc<AtomicBool>>,
    /// Files `sniff` found to be archives though their names don't say so (lowercase path), with the
    /// size and date they had then.
    sniffed: HashMap<String, ((u64, i64), Format)>,
    /// First reads in progress (a compressed tar's whole stream), by archive (lowercase path), with
    /// its path as written, for their progress and Stop.
    scans: HashMap<String, (String, Arc<stream::Scan>)>,
    /// One read of each archive at a time: the listing, the preview and folder sizes asking at once
    /// share it.
    loading: HashMap<String, Arc<Mutex<()>>>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The archive at `path` (lowercase key `key`), from memory when it was read before and its archive
/// file hasn't changed since; otherwise read from the file `file()` gives. Also says whether it was
/// in memory.
fn opened(path: &str, outer: &str, stamp: (u64, i64), format: Format, file: impl FnOnce() -> Result<PathBuf, Failure>) -> Result<(Arc<Opened>, bool), Failure> {
    let key = path.to_lowercase();
    let in_memory = || {
        let mut state = state();
        let at = state.opened.iter().position(|o| o.path.to_lowercase() == key)?;
        let found = state.opened.remove(at);
        (found.stamp == stamp).then(|| {
            state.opened.push(found.clone());
            found
        })
    };
    if let Some(found) = in_memory() {
        return Ok((found, true));
    }
    // One read at a time: whoever waited finds it in memory.
    let lock = state().loading.entry(key.clone()).or_default().clone();
    let loading = lock.lock().unwrap_or_else(|e| e.into_inner());
    let done_loading = || {
        drop(loading);
        let mut state = state();
        if Arc::strong_count(&lock) <= 2 {
            state.loading.remove(&key);
        }
    };
    if let Some(found) = in_memory() {
        done_loading();
        return Ok((found, true));
    }
    let started = Instant::now();
    let read = file().and_then(|file| {
        let size = std::fs::metadata(&file).map_or(0, |m| m.len());
        let scan = Arc::new(stream::Scan::new(size));
        state().scans.insert(key.clone(), (path.to_string(), scan.clone()));
        let read = guarded(|| Source::open(&file, format, &scan));
        state().scans.remove(&key);
        read.map(|read| (file, read))
    });
    let (file, (source, items, format)) = match read {
        Ok(read) => read,
        Err(e) => {
            done_loading();
            return Err(e);
        }
    };
    let tree = Tree::build(&items);
    let opened = Arc::new(Opened { path: path.to_string(), outer: outer.to_string(), stamp, file, format, source, tree, read_ms: ms_since(started) });
    {
        let mut state = state();
        state.opened.retain(|o| o.path.to_lowercase() != key);
        state.opened.push(opened.clone());
        // Within the budget, keeping the newest (this one) whatever its size.
        let mut kept = 0;
        let mut keep_from = state.opened.len();
        for (i, o) in state.opened.iter().enumerate().rev() {
            kept += o.tree.len();
            if kept > CACHE_NODES && i + 1 < state.opened.len() {
                break;
            }
            keep_from = i;
        }
        state.opened.drain(..keep_from);
    }
    done_loading();
    Ok((opened, false))
}

/// The size and date of the archive file on disk: a change means reading it again.
fn stamp_of(outer: &str) -> Result<(u64, i64), Failure> {
    fsx::stat(outer).filter(|e| !e.dir).map(|e| (e.size, e.modified)).ok_or(Failure::NotFound)
}

/// A place in an archive: the innermost archive a path goes into and a node of its tree, whether
/// everything on the way was in memory, and how long reading what wasn't took.
struct Spot {
    archive: Arc<Opened>,
    node: u32,
    cached: bool,
    read_ms: f64,
}

impl Spot {
    fn node(&self) -> &tree::Node {
        self.archive.tree.node(self.node)
    }
}

/// Where `path` leads in the archive it goes into; None when it doesn't go into one.
fn spot(path: &str) -> Option<Result<Spot, Failure>> {
    let found = locate(path)?;
    Some(guarded(|| spot_in(&found.outer, &found.inner, found.format)))
}

fn spot_in(outer: &str, inner: &str, format: Format) -> Result<Spot, Failure> {
    let stamp = stamp_of(outer)?;
    let (mut archive, mut cached) = opened(outer, outer, stamp, format, || Ok(PathBuf::from(outer)))?;
    let mut read_ms = if cached { 0.0 } else { archive.read_ms };
    let mut node = ROOT;
    for part in inner.split('\\').filter(|p| !p.is_empty()) {
        if !archive.tree.node(node).dir {
            let (inside, was_cached) = enter(&archive, node)?;
            if !was_cached {
                read_ms += inside.read_ms;
            }
            cached &= was_cached;
            archive = inside;
            node = ROOT;
        }
        node = archive.tree.child(node, part).ok_or(Failure::NotFound)?;
    }
    Ok(Spot { archive, node, cached, read_ms })
}

/// The archive that the file `node` of `parent` is, opened from a temp copy of it: one named like
/// an archive (a .jar too), or one whose first bytes say it's one.
fn enter(parent: &Arc<Opened>, node: u32) -> Result<(Arc<Opened>, bool), Failure> {
    let n = parent.tree.node(node);
    let format = match Format::claimed(&n.name) {
        Some((format, _)) => format,
        None => head_of(parent, node, 4096).ok().and_then(|(head, _)| Format::detect(&head)).ok_or(Failure::NotAFolder)?,
    };
    let path = format!("{}\\{}", parent.path, parent.tree.path_of(node));
    opened(&path, &parent.outer, parent.stamp, format, || copy_of(parent, node, Purpose::Nested))
}

/// Why a single entry is extracted, which sets how big it may be.
#[derive(Clone, Copy, PartialEq)]
enum Purpose {
    Thumbnail,
    Preview,
    Open,
    Nested,
}

impl Purpose {
    fn budget(self) -> (u64, &'static str) {
        match self {
            Purpose::Thumbnail => (THUMB_BUDGET, "make a thumbnail of"),
            Purpose::Preview => (PREVIEW_BUDGET, "preview"),
            Purpose::Nested => (NESTED_MAX, "open"),
            Purpose::Open => (u64::MAX, "open"),
        }
    }
}

/// Refuses an entry that looks like a bomb: one that expands more than `MAX_RATIO` times over
/// what's stored of it, once it's past `RATIO_FLOOR`.
fn check_ratio(source: &Source, item: ItemIndex, size: u64) -> Result<(), Failure> {
    let ratio = source.ratio(item, size);
    if size > RATIO_FLOOR && ratio > MAX_RATIO {
        return Err(Failure::Bomb(format!("it would expand {} times over, to {}", count_text(ratio), size_text(size))));
    }
    Ok(())
}

/// A temp copy of the file `node` of `archive`, read-only, made once and reused while the app runs.
fn copy_of(archive: &Arc<Opened>, node: u32, purpose: Purpose) -> Result<PathBuf, Failure> {
    let n = archive.tree.node(node);
    if n.dir {
        return Err(Failure::NotAFolder);
    }
    let item = n.item as ItemIndex;
    if let Some(problem) = archive.source.problem(item) {
        return Err(problem);
    }
    let (budget, what) = purpose.budget();
    let cost = archive.source.cost(item, n.size);
    if cost > budget {
        return Err(Failure::TooBig { size: cost, what });
    }
    check_ratio(&archive.source, item, n.size)?;
    let key = |node: u32| format!("{}|{}|{}|{}", archive.path.to_lowercase(), archive.tree.path_of(node).to_lowercase(), archive.stamp.0, archive.stamp.1);
    let cached = |key: &str| state().copies.get(key).filter(|p| p.exists()).cloned();
    if let Some(path) = cached(&key(node)) {
        return Ok(path);
    }
    // One read at a time of each 7z block (each entry, in a zip): two requests for one file (its
    // thumbnail and its preview) get one copy, and two thumbnails from one solid block one read.
    let block = archive.source.block_of(item);
    let unit = (archive.path.to_lowercase(), block.map_or((false, item), |b| (true, b)));
    let lock = state().reading.entry(unit.clone()).or_default().clone();
    let reading = lock.lock().unwrap_or_else(|e| e.into_inner());
    let done_reading = || {
        drop(reading);
        let mut state = state();
        // Nobody else waits for it (they'd hold it too): forget it.
        if Arc::strong_count(&lock) <= 2 {
            state.reading.remove(&unit);
        }
    };
    if let Some(path) = cached(&key(node)) {
        done_reading();
        return Ok(path);
    }
    let mut wanted = vec![node];
    if let (Some(b), Purpose::Thumbnail | Purpose::Preview) = (block, purpose) {
        let before = archive.source.cost(item, 0);
        for (other, o) in archive.tree.nodes.iter().enumerate() {
            let other = other as u32;
            // Cheapest tests first: this runs over every entry of the archive.
            if other != node && !o.dir && o.size <= THUMB_BUDGET && archive.source.block_of(o.item as ItemIndex) == Some(b) && archive.source.cost(o.item as ItemIndex, 0) < before && has_thumbnail(&o.name) && wanted.len() < 200 && cached(&key(other)).is_none() {
                wanted.push(other);
            }
        }
    }
    let total: u64 = wanted.iter().map(|&w| archive.tree.node(w).size).sum();
    let folder = extract::new_folder(if purpose == Purpose::Nested { "nested" } else { "open" })?;
    extract::check_space(&folder, total)?;
    let mut dests = HashMap::new();
    for (i, &w) in wanted.iter().enumerate() {
        // Each in a folder of its own, so it keeps its name.
        let dest = extract::inside(&folder, &[&i.to_string(), &archive.tree.node(w).name])?;
        dests.insert(archive.tree.node(w).item as ItemIndex, (w, dest));
    }
    let items: Vec<ItemIndex> = dests.keys().copied().collect();
    let mut done = Vec::new();
    let mut failure = None;
    let failed = archive.source.read_each(
        &archive.file,
        &items,
        &mut |item, reader| {
            let (w, dest) = &dests[&item];
            let n = archive.tree.node(*w);
            match extract::write_entry(reader, dest, n.size, n.modified, true, &mut |_| true) {
                Ok(()) => done.push(*w),
                Err(e) if *w == node => failure = Some(e),
                Err(_) => {}
            }
            true
        },
        &mut |_| true,
    );
    let mut state = state();
    for w in done {
        let k = key(w);
        state.copy_order.push(k.clone());
        state.copies.insert(k, dests[&(archive.tree.node(w).item as ItemIndex)].1.clone());
    }
    // Old copies are forgotten (not deleted: one may be open in another app); they go when the app closes.
    let extra = state.copy_order.len().saturating_sub(COPIES_KEPT);
    for k in state.copy_order.drain(..extra).collect::<Vec<_>>() {
        state.copies.remove(&k);
    }
    drop(state);
    done_reading();
    if let Some(e) = failure.or_else(|| failed.into_iter().find(|(i, _)| *i == item).map(|(_, e)| e)) {
        return Err(e);
    }
    cached(&key(node)).ok_or(Failure::NotFound)
}

/// Types Windows makes thumbnails of that are worth keeping while reading through a solid block.
fn has_thumbnail(name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "heic" | "heif" | "avif" | "ico" | "jfif" | "dng" | "cr2" | "nef" | "arw" | "mp4" | "mov" | "m4v" | "wmv" | "avi" | "mkv" | "pdf"
    )
}

// What other modules ask of archives.

/// The UTF-16 length of the first `parts` parts of `path` (as JavaScript counts), where the UI cuts
/// it to get an archive's own path.
fn prefix_units(path: &str, parts: usize) -> u32 {
    let mut seen = 0;
    let mut in_part = false;
    for (i, c) in path.char_indices() {
        let separator = c == '\\' || c == '/';
        if !separator && !in_part {
            in_part = true;
            seen += 1;
        } else if separator && in_part {
            in_part = false;
            if seen == parts {
                return path[..i].encode_utf16().count() as u32;
            }
        }
    }
    path.encode_utf16().count() as u32
}

fn parts(path: &str) -> usize {
    path.split(['\\', '/']).filter(|p| !p.is_empty()).count()
}

/// Lists a folder in an archive (or an archive's top) for `fsx::list_dir`, as the same binary
/// listing. None when `path` doesn't go into an archive.
pub fn list(path: &str, started: Instant) -> Option<Result<Vec<u8>, String>> {
    let found = spot(path)?;
    Some(
        found
            .and_then(|mut spot| {
                if !spot.node().dir {
                    // An archive inside an archive is a folder too.
                    let (inside, was_cached) = guarded(|| enter(&spot.archive, spot.node))?;
                    if !was_cached {
                        spot.read_ms += inside.read_ms;
                    }
                    spot = Spot { cached: spot.cached && was_cached, read_ms: spot.read_ms, archive: inside, node: ROOT };
                }
                let tree = &spot.archive.tree;
                let children: Vec<&tree::Node> = spot.node().children.iter().map(|&c| tree.node(c)).collect();
                let info = InArchive {
                    archive: prefix_units(path, parts(&spot.archive.path)),
                    outer: prefix_units(path, parts(&spot.archive.outer)),
                    entries: tree.len() as u32,
                    cached: spot.cached,
                    read_ms: spot.read_ms,
                    format: spot.archive.format.code(),
                };
                Ok(fsx::encode_listing(path, &children, ms_since(started), Some(&info), false))
            })
            .map_err(|e| e.message()),
    )
}

/// Checks a typed path that goes into an archive: the archive file itself and its folders are
/// folders. None when it doesn't go into one.
pub fn resolve(path: &str) -> Option<Result<Resolved, String>> {
    let found = spot(path)?;
    Some(found.map(|spot| Resolved { path: path.to_string(), dir: spot.node().dir || is_archive_name(&spot.node().name) }).map_err(|e| e.message()))
}

/// The first `limit` bytes of a file in an archive (and whether there's more), for text previews.
pub fn read_prefix(path: &str, limit: usize) -> Option<Result<(Vec<u8>, bool), String>> {
    if !is_inside(path) {
        return None;
    }
    let found = spot(path)?;
    let read = found.and_then(|spot| guarded(|| head_of(&spot.archive, spot.node, limit)));
    Some(read.map_err(|e| e.message()))
}

/// The first `limit` bytes of the file `node` of `archive`, and whether there's more.
fn head_of(archive: &Opened, node: u32, limit: usize) -> Result<(Vec<u8>, bool), Failure> {
    let n = archive.tree.node(node);
    if n.dir {
        return Err(Failure::NotAFolder);
    }
    let item = n.item as ItemIndex;
    if let Some(problem) = archive.source.problem(item) {
        return Err(problem);
    }
    let cost = archive.source.cost(item, 0).saturating_add((limit as u64).min(n.size));
    if cost > PREVIEW_BUDGET {
        return Err(Failure::TooBig { size: cost, what: "preview" });
    }
    let mut bytes = Vec::new();
    let mut error = None;
    let failed = archive.source.read_each(
        &archive.file,
        &[item],
        &mut |_, reader| {
            if let Err(e) = reader.take(limit as u64 + 1).read_to_end(&mut bytes) {
                error = Some(Failure::from(e));
            }
            false
        },
        &mut |_| true,
    );
    if let Some(e) = error.or_else(|| failed.into_iter().map(|(_, e)| e).find(|e| !matches!(e, Failure::Cancelled))) {
        return Err(e);
    }
    let more = bytes.len() > limit;
    bytes.truncate(limit);
    Ok((bytes, more))
}

/// A folder's total, each of its subfolders' (by name), and the engine that answered ("zip", "7z").
pub type Sizes = (u64, Vec<(String, u64)>, &'static str);

/// Every subfolder's total, from the archive's own sizes, for `sizes::folder_sizes`. None when
/// `path` doesn't go into an archive.
pub fn folder_sizes(path: &str) -> Option<Result<Sizes, String>> {
    let found = spot(path)?;
    Some(
        found
            .and_then(|spot| {
                let (archive, node) = if spot.node().dir { (spot.archive.clone(), spot.node) } else { (guarded(|| enter(&spot.archive, spot.node))?.0, ROOT) };
                let n = archive.tree.node(node);
                let sizes = n.children.iter().map(|&c| archive.tree.node(c)).filter(|c| c.dir).map(|c| (c.name.to_string(), c.size)).collect();
                Ok((n.size, sizes, archive.format.label()))
            })
            .map_err(|e| e.message()),
    )
}

/// Searches a folder in an archive and the folders in it by name, with the meaning fd's search has
/// (`shadui_index::query`), for `find::find_files`. Returns the engine's name ("zip" or "7z") and
/// whether it stopped at `max`. None when `root` doesn't go into an archive.
pub fn search(root: &str, query: &str, hidden: bool, exclude: &[String], max: Option<usize>, stop: &AtomicBool, push: &mut dyn FnMut(Entry)) -> Option<Result<(&'static str, bool), String>> {
    let found = spot(root)?;
    Some(
        found
            .and_then(|spot| {
                let (archive, start) = if spot.node().dir { (spot.archive.clone(), spot.node) } else { (guarded(|| enter(&spot.archive, spot.node))?.0, ROOT) };
                // find_files has checked the query: it reads, and asks for something.
                let (names, filters) = parse(query).map_err(Failure::Unsupported)?;
                let matcher = NameMatcher::from_terms(names);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
                let excludes = Excludes::new(exclude);
                let max = max.filter(|&m| m > 0).unwrap_or(usize::MAX);
                let base = root.trim_end_matches(['\\', '/']);
                let mut found = 0;
                let mut stack: Vec<(u32, String)> = vec![(start, base.to_string())];
                while let Some((folder, path)) = stack.pop() {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    for &child in archive.tree.node(folder).children.iter() {
                        let c = archive.tree.node(child);
                        let folded = (!c.name.is_ascii()).then(|| fold(&c.name));
                        let folded = folded.as_deref().map(str::as_bytes);
                        if (!hidden && is_hidden(c.attrs, c.name.as_bytes())) || excludes.name(c.name.as_bytes(), folded) {
                            continue;
                        }
                        let full = format!("{path}\\{}", c.name);
                        if matcher.matches(c.name.as_bytes(), folded) && filters.accepts(c.name.as_bytes(), folded, c.dir, c.size, (now - c.modified) / 1000) {
                            if found == max {
                                return Ok((archive.format.label(), true));
                            }
                            found += 1;
                            push(Entry { name: c.name.to_string(), path: Some(full.clone()), dir: c.dir, size: c.size, modified: c.modified, attrs: c.attrs });
                        }
                        if c.dir {
                            stack.push((child, full));
                        }
                    }
                }
                Ok((archive.format.label(), false))
            })
            .map_err(|e| e.message()),
    )
}

/// A map's items, whether smaller ones were left out, and the archive's format (the engine).
pub type Usage = (Vec<UsageNode>, bool, Format);

/// A disk-usage map of a folder in an archive, from the archive's own sizes, for `usage::disk_usage`,
/// in the order the index gives one: the folder first, then breadth first, each folder's items
/// together and biggest first, empty ones left out, `depth` levels down. When there are more than
/// `max_items`, the biggest are kept (with the folders they're in), and the flag says so. Also gives
/// the archive's format, the engine that answered.
pub fn usage(path: &str, depth: u32, max_items: u32) -> Option<Result<Usage, String>> {
    let found = spot(path)?;
    Some(
        found
            .and_then(|spot| {
                let (archive, top) = if spot.node().dir { (spot.archive.clone(), spot.node) } else { (guarded(|| enter(&spot.archive, spot.node))?.0, ROOT) };
                let tree = &archive.tree;
                let mut within = Vec::new();
                let mut queue = std::collections::VecDeque::from([(top, 0u32)]);
                while let Some((node, level)) = queue.pop_front() {
                    if level >= depth {
                        continue;
                    }
                    for &child in &tree.node(node).children {
                        let c = tree.node(child);
                        if c.size > 0 {
                            within.push(child);
                            if c.dir {
                                queue.push_back((child, level + 1));
                            }
                        }
                    }
                }
                let limited = within.len() > max_items as usize;
                if limited {
                    within.sort_unstable_by_key(|&n| std::cmp::Reverse(tree.node(n).size));
                    within.truncate(max_items as usize);
                }
                // What's kept, and the folders it's in (a folder is never smaller than what's in it,
                // but ties at the cut could leave one out).
                let mut keep = std::collections::HashSet::new();
                for n in within {
                    let mut at = n;
                    while at != top && keep.insert(at) {
                        at = tree.node(at).parent;
                    }
                }
                let t = tree.node(top);
                let mut nodes = vec![UsageNode { parent: NO_PARENT, name: String::new(), dir: true, size: t.size, modified: t.modified, attrs: t.attrs }];
                let mut queue = std::collections::VecDeque::from([(top, 0u32)]);
                while let Some((node, at)) = queue.pop_front() {
                    let mut children: Vec<u32> = tree.node(node).children.iter().copied().filter(|c| keep.contains(c)).collect();
                    children.sort_by(|&a, &b| tree.node(b).size.cmp(&tree.node(a).size).then_with(|| tree.node(a).name.cmp(&tree.node(b).name)));
                    for child in children {
                        let c = tree.node(child);
                        let index = nodes.len() as u32;
                        nodes.push(UsageNode { parent: at, name: c.name.to_string(), dir: c.dir, size: c.size, modified: c.modified, attrs: c.attrs });
                        if c.dir {
                            queue.push_back((child, index));
                        }
                    }
                }
                Ok((nodes, limited, archive.format))
            })
            .map_err(|e| e.message()),
    )
}

/// For `thumbs`: the temp copy of a file in an archive to make a thumbnail from. None when `path`
/// isn't in an archive; an error when there's none to make (too big, encrypted…).
pub fn thumbnail_source(path: &str) -> Option<Result<String, String>> {
    if !is_inside(path) {
        return None;
    }
    let found = spot(path)?;
    Some(found.and_then(|spot| guarded(|| copy_of(&spot.archive, spot.node, Purpose::Thumbnail))).map(|p| p.to_string_lossy().into_owned()).map_err(|e| e.message()))
}

/// At start: what earlier runs left in the temp folder goes (on a thread of its own).
pub fn sweep(on_clipboard: &[String]) {
    extract::sweep(on_clipboard);
}

/// When the app closes: this run's temp copies go.
pub fn finish() {
    extract::finish();
}

// Commands

/// A file in an archive extracted to a temp copy: to preview it (`purpose` "preview", within a
/// budget) or open it ("open"). The copy is read-only: changes to it can't go back into the archive.
#[tauri::command]
pub async fn archive_extract(path: String, purpose: String) -> Result<String, String> {
    let purpose = if purpose == "open" { Purpose::Open } else { Purpose::Preview };
    fsx::blocking(move || {
        let spot = spot(&path).ok_or("That isn't in an archive")?.map_err(|e| e.message())?;
        guarded(|| copy_of(&spot.archive, spot.node, purpose)).map(|p| p.to_string_lossy().into_owned()).map_err(|e| e.message())
    })
    .await
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StageProgress {
    done: u64,
    total: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ReadProgress {
    /// The archive being read.
    path: String,
    done: u64,
    total: u64,
}

/// The first read of the archive `path` goes into, while it runs (a compressed tar is read whole to
/// list it): compressed bytes read of the file's size. None when nothing is being read.
#[tauri::command]
pub fn archive_progress(path: String) -> Option<ReadProgress> {
    let path = path.to_lowercase();
    let state = state();
    state.scans.iter().find(|(key, _)| path.starts_with(key.as_str())).map(|(_, (archive, scan))| ReadProgress { path: archive.clone(), done: scan.done.load(Ordering::Relaxed), total: scan.total })
}

/// Stops the first read of the archive `path` goes into (its listing then says "cancelled").
#[tauri::command]
pub fn cancel_archive_read(path: String) {
    let path = path.to_lowercase();
    for (key, (_, scan)) in state().scans.iter() {
        if path.starts_with(key.as_str()) {
            scan.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// Compressed streams bigger than this aren't read just because they're selected.
const PEEK_LIMIT: u64 = 64 << 20;

/// What the preview pane says about an archive, without reading a big compressed one whole.
#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Peek {
    format: &'static str,
    packed: u64,
    /// Its files and folders, and what they unpack to, once its list of entries is read.
    entries: Option<u32>,
    unpacked: Option<u64>,
}

#[tauri::command]
pub async fn archive_peek(path: String) -> Result<Peek, String> {
    fsx::blocking(move || {
        let found = locate(&path).ok_or("It isn't an archive Glint reads")?;
        // A big compressed stream (or ncdu export) on disk that isn't in memory yet: only what its
        // name and size say.
        let stream = matches!(found.format, Format::Tar(c) | Format::Single(c) if c != codec::Codec::Plain) || matches!(found.format, Format::Ncdu(_));
        let in_memory = state().opened.iter().any(|o| o.path.eq_ignore_ascii_case(&path));
        if stream && found.inner.is_empty() && !in_memory {
            let packed = fsx::stat(&found.outer).map_or(0, |e| e.size);
            if packed > PEEK_LIMIT {
                return Ok(Peek { format: found.format.label(), packed, entries: None, unpacked: None });
            }
        }
        let spot = spot(&path).ok_or("It isn't an archive Glint reads")?.map_err(|e| e.message())?;
        let (archive, node) = if spot.node().dir { (spot.archive.clone(), spot.node) } else { (guarded(|| enter(&spot.archive, spot.node)).map_err(|e| e.message())?.0, ROOT) };
        let packed = if found.inner.is_empty() { fsx::stat(&found.outer).map_or(0, |e| e.size) } else { spot.node().size };
        Ok(Peek { format: archive.format.label(), packed, entries: Some(archive.tree.len() as u32), unpacked: Some(archive.tree.node(node).size) })
    })
    .await
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    path: String,
    reason: String,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Staged {
    /// The folder they're in, to delete once they're copied.
    folder: String,
    /// The copy of each item asked for (in that order) that could be made, folders with what they hold.
    paths: Vec<String>,
    /// Files that couldn't be extracted, and why.
    skipped: Vec<Skipped>,
    bytes: u64,
}

/// One file to extract: where it is and where it goes.
struct Job {
    archive: Arc<Opened>,
    node: u32,
    dest: PathBuf,
    /// Its path as the UI writes it, for what's reported.
    path: String,
}

/// Extracts files and folders from archives into a new staging folder, keeping their folders, for
/// Windows' file engine to copy from: for the other pane (`purpose` "copy": a folder of this run,
/// deleted after the copy) or for other apps ("out": the clipboard, a drag). Reports progress, and
/// stops when the tab `lane` cancels. Files that can't be extracted (encrypted, damaged…) are left out
/// and reported; fails when nothing could be.
#[tauri::command]
pub async fn archive_stage(paths: Vec<String>, purpose: String, lane: String, on_progress: Channel<StageProgress>) -> Result<Staged, String> {
    in_lane(lane, move |cancel| stage(&paths, &purpose, cancel, &on_progress).map_err(|e| e.message())).await
}

/// Runs `work` with a Cancel of its own for the tab `lane` (`cancel_archive_stage`); a newer run in
/// the tab cancels it.
async fn in_lane<T: Send + 'static>(lane: String, work: impl FnOnce(&AtomicBool) -> Result<T, String> + Send + 'static) -> Result<T, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(previous) = state().stages.insert(lane.clone(), cancel.clone()) {
        previous.store(true, Ordering::SeqCst);
    }
    let ours = cancel.clone();
    let result = fsx::blocking(move || work(&cancel)).await;
    // Unless a newer run in the tab took its place (its Cancel must keep working).
    let mut guard = state();
    if guard.stages.get(&lane).is_some_and(|current| Arc::ptr_eq(current, &ours)) {
        guard.stages.remove(&lane);
    }
    result
}

/// Compress to zip: `paths` (files and folders on disk) into a new zip in `folder` named `name`,
/// numbered like Explorer's if it's taken ("photos (2).zip"), with progress. Cancel (the tab `lane`)
/// or a failure leaves nothing behind; files that can't be read are left out and reported. The
/// zip is `paths[0]` of what comes back.
#[tauri::command]
pub async fn archive_compress(paths: Vec<String>, folder: String, name: String, lane: String, on_progress: Channel<StageProgress>) -> Result<Staged, String> {
    if is_read_only(&folder) || paths.iter().any(|p| is_inside(p)) {
        return Err(fsx::READ_ONLY.into());
    }
    in_lane(lane, move |cancel| {
        let zip = fsx::create_numbered(&folder, &name, false)?;
        match compress::compress(&paths, Path::new(&zip), cancel, &mut |done, total| {
            let _ = on_progress.send(StageProgress { done, total });
        }) {
            Ok((bytes, skipped)) => Ok(Staged { folder, paths: vec![zip], skipped, bytes }),
            Err(e) => {
                let _ = std::fs::remove_file(&zip);
                Err(e)
            }
        }
    })
    .await
}

/// Extract here: a whole archive on disk into a new folder next to it named after it, as Explorer's
/// Extract All does ("photos" for photos.zip or photos.tar.gz, numbered if taken), with the same
/// limits and checks as every extraction, and progress. A single compressed file is written next to
/// it instead ("notes.txt" for notes.txt.gz), as gzip -d would. Cancel (the tab `lane`) or a failure
/// leaves nothing behind; files that can't be extracted are left out and reported. The folder (or the
/// one file) is `folder` of what comes back.
#[tauri::command]
pub async fn archive_extract_here(path: String, lane: String, on_progress: Channel<StageProgress>) -> Result<Staged, String> {
    in_lane(lane, move |cancel| {
        let archive = Path::new(&path);
        let (Some(parent), Some(stem)) = (archive.parent(), archive.file_stem()) else { return Err(Failure::NotFound.message()) };
        let spot = spot(&path).ok_or_else(|| Failure::NotFound.message())?.map_err(|e| e.message())?;
        let tree = &spot.archive.tree;
        let tops: Vec<(String, &str)> = tree.node(spot.node).children.iter().map(|&c| (format!("{path}\\{}", tree.node(c).name), &*tree.node(c).name)).collect();
        let parent = parent.to_string_lossy();
        if let (Format::Single(_), [(top, name)]) = (spot.archive.format, tops.as_slice()) {
            let file = fsx::create_numbered(&parent, name, false)?;
            let named = Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            return match guarded(|| stage_into(Path::new(&*parent), std::slice::from_ref(top), Some(&named), cancel, &on_progress)) {
                Ok(staged) if !staged.paths.is_empty() && staged.skipped.is_empty() => Ok(Staged { folder: file, ..staged }),
                other => {
                    let _ = std::fs::remove_file(&file);
                    Err(match other {
                        Ok(staged) => staged.skipped.into_iter().next().map_or_else(|| Failure::NotFound.message(), |s| s.reason),
                        Err(e) => e.message(),
                    })
                }
            };
        }
        let tops: Vec<String> = tops.into_iter().map(|(top, _)| top).collect();
        let folder = fsx::create_numbered(&parent, without_tar(&stem.to_string_lossy()), true)?;
        match guarded(|| stage_into(Path::new(&folder), &tops, None, cancel, &on_progress)) {
            Ok(staged) if !staged.paths.is_empty() => Ok(Staged { folder, ..staged }),
            other => {
                let _ = std::fs::remove_dir_all(&folder);
                Err(match other {
                    Ok(staged) => staged.skipped.into_iter().next().map_or_else(|| "The archive is empty".into(), |s| s.reason),
                    Err(e) => e.message(),
                })
            }
        }
    })
    .await
}

/// Export as ncdu JSON: the folder `path` on disk walked into `dest` as ncdu's JSON export (which
/// ncdu, and Glint, open again), written to `dest.part` and renamed when done. Progress counts
/// entries (there's no total to know ahead); Stop (the tab `lane`) or a failure leaves nothing.
/// The export is `paths[0]` of what comes back.
#[tauri::command]
pub async fn export_ncdu(path: String, dest: String, lane: String, on_progress: Channel<StageProgress>) -> Result<Staged, String> {
    if is_inside(&path) || locate(&path).is_some() {
        return Err("Only folders on disk can be exported".into());
    }
    in_lane(lane, move |cancel| {
        let part = format!("{dest}.part");
        let written = std::fs::File::create(&part).map_err(Failure::from).and_then(|file| {
            ncdu::export(Path::new(&path), file, cancel, &mut |done| {
                let _ = on_progress.send(StageProgress { done, total: 0 });
            })
        });
        match written.and_then(|done| std::fs::rename(&part, &dest).map(|_| done).map_err(Failure::from)) {
            Ok(done) => Ok(Staged { folder: path, paths: vec![dest], skipped: Vec::new(), bytes: done.bytes }),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(e.message())
            }
        }
    })
    .await
}

/// A compressed tar's name without its ".tar" ("photos" for the "photos.tar" of photos.tar.gz).
fn without_tar(stem: &str) -> &str {
    match stem.len().checked_sub(4).and_then(|at| stem.get(at..).map(|end| (at, end))) {
        Some((at, end)) if at > 0 && end.eq_ignore_ascii_case(".tar") => &stem[..at],
        _ => stem,
    }
}

fn stage(paths: &[String], purpose: &str, cancel: &AtomicBool, progress: &Channel<StageProgress>) -> Result<Staged, Failure> {
    let out = purpose != "copy";
    if out {
        // Older copies handed over (an hour or more ago) that aren't on the clipboard go now.
        let on_clipboard = crate::shell::clipboard_paths();
        extract::sweep_out(&on_clipboard, Duration::from_secs(3600));
    }
    let folder = extract::new_folder(if out { "out" } else { "copy" })?;
    let result = guarded(|| stage_into(&folder, paths, None, cancel, progress));
    if !matches!(&result, Ok(staged) if !staged.paths.is_empty()) {
        extract::remove_tree(&folder);
    }
    match result {
        Ok(staged) if staged.paths.is_empty() => Err(staged.skipped.into_iter().next().map_or(Failure::NotFound, |s| Failure::Io(s.reason))),
        other => other,
    }
}

/// Extracts `paths` into `folder`, each by its own name, or by `named` (one path only: Extract here's
/// single file, under the free name it found).
fn stage_into(folder: &Path, paths: &[String], named: Option<&str>, cancel: &AtomicBool, progress: &Channel<StageProgress>) -> Result<Staged, Failure> {
    let mut jobs = Vec::new();
    let mut staged = Vec::new();
    let mut names = std::collections::HashSet::new();
    for (k, path) in paths.iter().enumerate() {
        let spot = spot(path).ok_or(Failure::NotFound)??;
        let top = named.map_or_else(|| spot.node().name.to_string(), str::to_string);
        // Two of the same name (search results from different folders) each get a folder of their own.
        let dest_top = if names.insert(top.to_lowercase()) { extract::inside(folder, &[&top])? } else { extract::inside(folder, &[&k.to_string(), &top])? };
        if spot.node().dir {
            for f in spot.archive.tree.folders_under(spot.node) {
                std::fs::create_dir_all(dest_top.join(relative(&spot.archive.tree, spot.node, f)))?;
            }
        }
        for file in spot.archive.tree.files_under(spot.node) {
            let rel = relative(&spot.archive.tree, spot.node, file);
            let parts: Vec<&str> = rel.split('\\').filter(|p| !p.is_empty()).collect();
            let dest = if parts.is_empty() { dest_top.clone() } else { extract::inside(&dest_top, &parts)? };
            let shown = if rel.is_empty() { path.clone() } else { format!("{}\\{rel}", path.trim_end_matches('\\')) };
            jobs.push(Job { archive: spot.archive.clone(), node: file, dest, path: shown });
        }
        staged.push(dest_top.to_string_lossy().into_owned());
    }
    let files = jobs.len();
    let mut skipped = Vec::new();
    // What can't be extracted at all is reported without trying.
    jobs.retain(|job| {
        let n = job.archive.tree.node(job.node);
        let item = n.item as ItemIndex;
        let problem = job.archive.source.problem(item).map(Err).unwrap_or_else(|| check_ratio(&job.archive.source, item, n.size));
        match problem {
            Err(e) => {
                skipped.push(Skipped { path: job.path.clone(), reason: e.message() });
                false
            }
            Ok(()) => true,
        }
    });
    let bytes: u64 = jobs.iter().map(|j| j.archive.tree.node(j.node).size).sum();
    extract::check_space(folder, bytes)?;
    // Work is what's decompressed: in a solid 7z block, what's read through to reach a file too.
    let mut reach: HashMap<(String, usize), u64> = HashMap::new();
    let mut total = 0u64;
    for job in &jobs {
        let n = job.archive.tree.node(job.node);
        let item = n.item as ItemIndex;
        match job.archive.source.block_of(item) {
            Some(b) => {
                let end = job.archive.source.cost(item, n.size);
                let at = reach.entry((job.archive.path.to_lowercase(), b)).or_default();
                *at = (*at).max(end);
            }
            None => total += n.size,
        }
    }
    total += reach.values().sum::<u64>();
    let done = std::cell::Cell::new(0u64);
    let last = std::cell::Cell::new(Instant::now());
    let _ = progress.send(StageProgress { done: 0, total });
    let tick = |n: u64| {
        done.set(done.get() + n);
        if last.get().elapsed() >= Duration::from_millis(100) {
            last.set(Instant::now());
            let _ = progress.send(StageProgress { done: done.get(), total });
        }
        !cancel.load(Ordering::Relaxed)
    };
    // Archive by archive (a selection can come from search results in nested archives).
    let mut by_archive: Vec<(Arc<Opened>, Vec<&Job>)> = Vec::new();
    for job in &jobs {
        match by_archive.iter_mut().find(|(a, _)| Arc::ptr_eq(a, &job.archive)) {
            Some((_, list)) => list.push(job),
            None => by_archive.push((job.archive.clone(), vec![job])),
        }
    }
    for (archive, list) in by_archive {
        let by_item: HashMap<ItemIndex, &Job> = list.iter().map(|j| (archive.tree.node(j.node).item as ItemIndex, *j)).collect();
        let items: Vec<ItemIndex> = by_item.keys().copied().collect();
        let mut failures = Vec::new();
        let failed = archive.source.read_each(
            &archive.file,
            &items,
            &mut |item, reader| {
                let job = by_item[&item];
                let n = archive.tree.node(job.node);
                match extract::write_entry(reader, &job.dest, n.size, n.modified, false, &mut |n| tick(n)) {
                    Ok(()) => true,
                    Err(Failure::Cancelled) => false,
                    Err(e) => {
                        failures.push((item, e));
                        true
                    }
                }
            },
            &mut |n| tick(n),
        );
        if cancel.load(Ordering::Relaxed) {
            return Err(Failure::Cancelled);
        }
        for (item, e) in failures.into_iter().chain(failed) {
            if let Some(job) = by_item.get(&item) {
                skipped.push(Skipped { path: job.path.clone(), reason: e.message() });
            }
        }
    }
    let _ = progress.send(StageProgress { done: total, total });
    // Nothing to copy when every file failed (empty folders alone are worth copying).
    if !skipped.is_empty() && skipped.len() >= files {
        return Err(Failure::Io(skipped.swap_remove(0).reason));
    }
    // An item asked for whose files all failed isn't there to copy.
    staged.retain(|p| Path::new(p).exists());
    Ok(Staged { folder: folder.to_string_lossy().into_owned(), paths: staged, skipped, bytes })
}

/// The path of `node` below `top`, "\" between folders ("" for `top` itself).
fn relative(tree: &Tree, top: u32, mut node: u32) -> String {
    let mut parts = Vec::new();
    while node != top && node != ROOT {
        parts.push(&*tree.node(node).name);
        node = tree.node(node).parent;
    }
    parts.reverse();
    parts.join("\\")
}

/// Stops the tab's running copy out of an archive.
#[tauri::command]
pub fn cancel_archive_stage(lane: String) {
    if let Some(cancel) = state().stages.get(&lane) {
        cancel.store(true, Ordering::SeqCst);
    }
}

/// Deletes a staging folder once Windows' file engine has copied what's in it.
#[tauri::command]
pub async fn archive_unstage(folder: String) -> Result<(), String> {
    fsx::blocking(move || {
        let path = PathBuf::from(&folder);
        if !extract::is_ours(&path) {
            return Err("That isn't a folder Glint extracted to".into());
        }
        extract::remove_tree(&path);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests;
