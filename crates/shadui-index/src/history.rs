//! Where did my file go: what was renamed, moved, deleted, or put in and taken out of the Recycle
//! Bin, as the change journal tells it, kept 30 days.
//!
//! The journal names each file as it was then, and its folder by file reference: a path is put
//! together from the folders the journal itself names (as they stood at each record) and, for the
//! rest, the index. What programs do with their own files all day is left out: scratch files made
//! and gone within minutes, a delete that a save puts right back, Windows' folder, ProgramData,
//! AppData, caches and tool folders. Past the first 50 of a burst of deletes in one folder, the rest
//! are counted, so a build or a cleanup doesn't push a month of history out.

use crate::proto::unix_ms;
use crate::snapshot::Checksum;
use crate::tree::{make_ref, split_ref, Tree, NONE, ROOT};
use crate::usn::{UsnRecord, REASON_CLOSE, REASON_FILE_CREATE, REASON_FILE_DELETE, REASON_RENAME_NEW_NAME, REASON_RENAME_OLD_NAME};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// Kept at most: events, bytes (paths, mostly), and days.
pub const MAX_EVENTS: usize = 100_000;
pub const MAX_BYTES: usize = 16 << 20;
pub const KEEP_MS: i64 = 30 * 86_400_000;
/// Made and gone (or renamed) this soon after: a program's scratch file.
const SCRATCH_MS: i64 = 10 * 60_000;
/// A delete followed this soon by something new of the same name in the same place was a save.
const SAVE_MS: i64 = 5_000;
/// Deletes in one folder this close together are a burst; past `BURST` of them (`BIN_BURST` in the
/// Recycle Bin, where what's emptied matters most) they're counted.
const BURST_GAP_MS: i64 = 30_000;
const BURST: u32 = 50;
const BIN_BURST: u32 = 1_000;
/// How far back a folder's delete looks for what was in it.
const FOLD_MS: i64 = 10 * 60_000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x100;
const MAX_DEPTH: usize = 4096;
/// NTFS's `$Extend` folder, which the index leaves out: what's deleted while something still has it
/// open is moved into its `$Deleted` folder first, and goes when the last handle closes.
const EXTEND: u64 = 11;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Renamed,
    Moved,
    /// Deleted for good (not to the Recycle Bin).
    Deleted,
    /// Put in the Recycle Bin.
    Recycled,
    /// Taken back out of it.
    Restored,
    /// Deleted from it (emptied).
    Emptied,
}

const KINDS: [Kind; 6] = [Kind::Renamed, Kind::Moved, Kind::Deleted, Kind::Recycled, Kind::Restored, Kind::Emptied];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// When, Unix ms.
    pub time: i64,
    pub kind: Kind,
    pub dir: bool,
    /// Where it was; for something in the Recycle Bin, its place there.
    pub path: String,
    /// Where it went (renamed, moved, recycled, restored).
    pub to: Option<String>,
    /// For something in the Recycle Bin: where it was before.
    pub was: Option<String>,
    /// Its file reference, to find where it is now (0 for a burst's count).
    pub file: u64,
    /// A folder deleted with what was in it: how many items went with it. A burst: how many more
    /// were deleted in the folder `path`.
    pub items: u32,
    pub burst: bool,
}

impl Event {
    fn bytes(&self) -> usize {
        64 + self.path.len() + self.to.as_ref().map_or(0, String::len) + self.was.as_ref().map_or(0, String::len)
    }
}

/// Folders' paths as they stood when each journal record was written.
pub struct Paths<'a> {
    tree: &'a Tree,
    letter: char,
    /// The folders the records name: the folder each is in and its name, by file reference.
    dirs: HashMap<u64, (u64, String)>,
}

impl<'a> Paths<'a> {
    pub fn new(tree: &'a Tree, letter: char) -> Self {
        Paths { tree, letter, dirs: HashMap::new() }
    }

    /// How the folders `records` name stood before them: each one's first record says (a rename's
    /// first half names it as it was). The index, being newer, may not know them that way.
    pub fn learn(&mut self, records: &[UsnRecord]) {
        for r in records.iter().filter(|r| r.attrs & FILE_ATTRIBUTE_DIRECTORY != 0) {
            self.dirs.entry(r.file).or_insert_with(|| (r.parent, r.name.clone()));
        }
    }

    /// A folder's record moves it on to where it is after it; a rename's first half leaves it as
    /// it was.
    fn after(&mut self, r: &UsnRecord) {
        let first_half = r.reason & REASON_RENAME_OLD_NAME != 0 && r.reason & REASON_RENAME_NEW_NAME == 0;
        if r.attrs & FILE_ATTRIBUTE_DIRECTORY != 0 && !first_half {
            self.dirs.insert(r.file, (r.parent, r.name.clone()));
        }
    }

    /// The path of folder `folder` (a file reference) as it stands, e.g. `C:\Users\me`; None when
    /// neither the journal nor the index knows it.
    pub fn folder(&self, folder: u64) -> Option<String> {
        let mut names: Vec<&str> = Vec::new();
        let mut at = folder;
        loop {
            let (number, seq) = split_ref(at);
            if number == ROOT as u64 {
                break;
            }
            if number == EXTEND {
                names.push("$Extend");
                break;
            }
            if names.len() >= MAX_DEPTH {
                return None;
            }
            if let Some((up, name)) = self.dirs.get(&at) {
                names.push(name);
                at = *up;
                continue;
            }
            // The index's, if the record still holds that folder.
            let rec = self.tree.rec(u32::try_from(number).ok()?).filter(|r| r.used() && r.dir() && (seq == 0 || r.seq() == seq))?;
            if rec.first_link == NONE {
                return None;
            }
            names.push(self.tree.name(rec.first_link));
            let up = self.tree.link(rec.first_link).parent;
            at = make_ref(up, self.tree.rec(up).map_or(0, |r| r.seq()));
        }
        let mut path = format!("{}:", self.letter);
        for name in names.iter().rev() {
            path.push('\\');
            path.push_str(name);
        }
        if names.is_empty() {
            path.push('\\');
        }
        Some(path)
    }
}

fn join(folder: &str, name: &str) -> String {
    if folder.ends_with('\\') {
        format!("{folder}{name}")
    } else {
        format!("{folder}\\{name}")
    }
}

/// The folder a path is in (`C:\` for what's right in the root).
pub fn parent_of(path: &str) -> &str {
    match path.rfind('\\') {
        Some(at) if at <= 2 => &path[..at + 1],
        Some(at) => &path[..at],
        None => path,
    }
}

fn name_of(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

fn in_bin(path: &str) -> bool {
    path.split('\\').nth(1).is_some_and(|top| top.eq_ignore_ascii_case("$Recycle.Bin"))
}

/// Where NTFS moves what's deleted while it's still open, until the last handle closes.
fn set_aside(path: &str) -> bool {
    let mut parts = path.split('\\').skip(1);
    parts.next().is_some_and(|p| p.eq_ignore_ascii_case("$Extend")) && parts.next().is_some_and(|p| p.eq_ignore_ascii_case("$Deleted"))
}

/// The Recycle Bin's own files right in a user's bin: the `$I…` record beside each item, and its
/// desktop.ini.
fn bin_record(path: &str) -> bool {
    let name = name_of(path).to_lowercase();
    path.split('\\').count() == 4 && (name.starts_with("$i") || name == "desktop.ini")
}

/// Top folders programs keep for themselves.
const ROOT_NOISE: &[&str] = &["windows", "programdata", "$extend", "system volume information", "$winreagent", "$windows.~bt", "$windows.~ws", "config.msi", "recovery"];
/// Folders anywhere that hold programs' and tools' own files.
const NOISE: &[&str] = &[
    "temp", "tmp", "cache", "caches", "cache2", "code cache", "gpucache", "shadercache", "inetcache", "crashpad", "crashdumps", ".cache", ".git", ".svn", ".hg",
    "node_modules", "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache", ".venv", ".gradle", ".next", ".nuxt", ".svelte-kit", ".turbo", ".parcel-cache",
    ".terraform", ".vs", ".idea",
];

/// Whether `path` is in a folder programs keep for themselves: Windows', ProgramData, a user's
/// AppData, NTFS's own, caches, temporary and tool folders. (Only the folders count: a folder of
/// one's own named Temp is still told.)
fn noise(path: &str) -> bool {
    let lower = path.to_lowercase();
    let parts: Vec<&str> = lower.split('\\').collect();
    let folders = &parts[..parts.len().saturating_sub(1)];
    if folders.get(1).is_some_and(|top| ROOT_NOISE.contains(top)) {
        return true;
    }
    if folders.len() > 3 && folders[1] == "users" && folders[3] == "appdata" {
        return true;
    }
    folders.iter().skip(1).any(|f| NOISE.contains(f))
}

/// Names programs give scratch files: Office's `~$…` and `~WRL….tmp`, `.tmp`, a download still
/// coming in, an editor's swap file.
fn scratch_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with('~')
        || lower.starts_with(".~")
        || lower.starts_with(".#")
        || lower.ends_with('~')
        || [".tmp", ".temp", ".crdownload", ".part", ".partial", ".download", ".swp", ".swx"].iter().any(|ext| lower.ends_with(ext))
}

#[derive(Default)]
pub struct History {
    /// Oldest first.
    events: VecDeque<Event>,
    bytes: usize,
    /// Renames waiting for their second half: where the file was, and when.
    renaming: HashMap<u64, (String, i64)>,
    /// What was made lately: when, by file reference, and in order (to forget it).
    made: HashMap<u64, i64>,
    made_order: VecDeque<(i64, u64)>,
    /// Bursts of deletes going on, by folder (lowercase): when the last delete was, how many there
    /// were, and when the burst's count was put in (once it's past the first ones).
    bursts: HashMap<String, (i64, u32, Option<i64>)>,
    /// The journal it's current to: its id, and the next record's USN.
    pub journal: u64,
    pub next_usn: i64,
}

impl History {
    /// Takes in journal records, in order. `paths` knows the folders as they stood before the first.
    pub fn feed(&mut self, records: &[UsnRecord], paths: &mut Paths) {
        for r in records {
            self.take(r, paths);
            paths.after(r);
        }
    }

    fn take(&mut self, r: &UsnRecord, paths: &Paths) {
        let time = unix_ms(r.time);
        self.forget(time);
        let dir = r.attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
        let path = || paths.folder(r.parent).map(|folder| join(&folder, &r.name));
        if r.reason & REASON_FILE_CREATE != 0 && !self.made.contains_key(&r.file) {
            self.made.insert(r.file, time);
            self.made_order.push_back((time, r.file));
            if let Some(path) = path() {
                self.saved_over(&path, time);
            }
        }
        // A rename is two records: the old name first, then the new one (and the reason stays on
        // the file's next records, until it's closed).
        let (old, new) = (r.reason & REASON_RENAME_OLD_NAME != 0, r.reason & REASON_RENAME_NEW_NAME != 0);
        if old && !new {
            if let Some(path) = path() {
                self.renaming.insert(r.file, (path, time));
            }
        } else if new {
            if let Some((from, _)) = self.renaming.remove(&r.file) {
                if let Some(to) = path() {
                    if set_aside(&to) {
                        // Deleted while something had it open: NTFS moves it aside first, and its
                        // delete there later is NTFS's own.
                        let name = name_of(&from).to_string();
                        self.deleted(r.file, &name, r.attrs, from, time, dir);
                    } else {
                        self.saved_over(&to, time);
                        self.moved(r, from, to, time, dir);
                    }
                }
            }
        }
        if r.reason & REASON_FILE_DELETE != 0 && r.reason & REASON_CLOSE != 0 {
            if let Some(path) = path() {
                self.deleted(r.file, &r.name, r.attrs, path, time, dir);
            }
        }
    }

    /// Forgets what's past: what was made over SCRATCH_MS ago, renames that never got a second
    /// half, and bursts that ended.
    fn forget(&mut self, now: i64) {
        while let Some(&(at, file)) = self.made_order.front() {
            if now - at < SCRATCH_MS {
                break;
            }
            self.made_order.pop_front();
            if self.made.get(&file) == Some(&at) {
                self.made.remove(&file);
            }
        }
        if self.renaming.len() > 1024 {
            self.renaming.retain(|_, (_, at)| now - *at < 60_000);
        }
        if self.bursts.len() > 1024 {
            self.bursts.retain(|_, (at, _, _)| now - *at <= BURST_GAP_MS);
        }
    }

    fn made_lately(&self, file: u64, now: i64) -> bool {
        self.made.get(&file).is_some_and(|&at| now - at < SCRATCH_MS)
    }

    /// Something new took the place of what was deleted just before: that was a save, not a delete.
    fn saved_over(&mut self, path: &str, now: i64) {
        let lower = path.to_lowercase();
        for at in (0..self.events.len()).rev() {
            let e = &self.events[at];
            if now - e.time > SAVE_MS {
                return;
            }
            if e.kind == Kind::Deleted && !e.burst && e.path.to_lowercase() == lower {
                let gone = self.events.remove(at).unwrap();
                self.bytes -= gone.bytes();
                return;
            }
        }
    }

    fn moved(&mut self, r: &UsnRecord, from: String, to: String, time: i64, dir: bool) {
        if from == to {
            return;
        }
        let same_folder = parent_of(&from).to_lowercase() == parent_of(&to).to_lowercase();
        let kind = match (in_bin(&from), in_bin(&to)) {
            (false, true) => Kind::Recycled,
            (true, false) => Kind::Restored,
            (true, true) => return,
            (false, false) if same_folder => Kind::Renamed,
            (false, false) => Kind::Moved,
        };
        let scratch = |p: &str| noise(p) || scratch_name(name_of(p));
        let skip = match kind {
            Kind::Renamed | Kind::Moved => scratch(&from) || scratch(&to) || r.attrs & FILE_ATTRIBUTE_TEMPORARY != 0,
            Kind::Recycled => noise(&from),
            _ => noise(&to),
        };
        // What was just made getting its name (a new folder named) isn't news either.
        if skip || (kind == Kind::Renamed && self.made_lately(r.file, time)) {
            return;
        }
        let was = if kind == Kind::Restored { self.recycled_from(r.file) } else { None };
        self.push(Event { time, kind, dir, path: from, to: Some(to), was, file: r.file, items: 0, burst: false });
    }

    /// Record `file`, named `name` with `attrs`, was deleted from `path`.
    fn deleted(&mut self, file: u64, name: &str, attrs: u32, path: String, time: i64, dir: bool) {
        let bin = in_bin(&path);
        if self.made_lately(file, time) || (bin && bin_record(&path)) {
            return;
        }
        if !bin && (noise(&path) || scratch_name(name) || attrs & FILE_ATTRIBUTE_TEMPORARY != 0) {
            return;
        }
        let was = if bin { self.recycled_from(file) } else { None };
        let kind = if bin { Kind::Emptied } else { Kind::Deleted };
        let items = if dir { self.fold(&path, time) } else { 0 };
        // A burst of deletes in one folder: past its first ones, they're counted.
        let folder = parent_of(&path).to_string();
        let (last, count, counted) = self.bursts.get(&folder.to_lowercase()).copied().unwrap_or((time, 0, None));
        let (count, counted) = if time - last > BURST_GAP_MS { (1, None) } else { (count + 1, counted) };
        let counted = if count > if bin { BIN_BURST } else { BURST } {
            match counted {
                Some(at) => {
                    if let Some(e) = self.events.iter_mut().rev().find(|e| e.burst && e.time == at && e.path == folder) {
                        e.items += 1 + items;
                    }
                    Some(at)
                }
                None => {
                    self.push(Event { time, kind, dir: true, path: folder.clone(), to: None, was: None, file: 0, items: 1 + items, burst: true });
                    Some(time)
                }
            }
        } else {
            self.push(Event { time, kind, dir, path, to: None, was, file, items, burst: false });
            counted
        };
        self.bursts.insert(folder.to_lowercase(), (time, count, counted));
    }

    /// Takes the deletes of what was in folder `path` (just before it went) out of the history: the
    /// folder's own event counts them.
    fn fold(&mut self, path: &str, now: i64) -> u32 {
        let inside = format!("{}\\", path.to_lowercase());
        let mut kept = Vec::new();
        let mut items = 0;
        while self.events.back().is_some_and(|e| now - e.time <= FOLD_MS) {
            let e = self.events.pop_back().unwrap();
            // A burst's count is of what was in its folder: the one deleted, or one inside it.
            let at = if e.burst { format!("{}\\", e.path.to_lowercase()) } else { e.path.to_lowercase() };
            if matches!(e.kind, Kind::Deleted | Kind::Emptied) && at.starts_with(&inside) {
                self.bytes -= e.bytes();
                items += if e.burst { e.items } else { 1 + e.items };
            } else {
                kept.push(e);
            }
        }
        self.events.extend(kept.into_iter().rev());
        items
    }

    /// Where the file put in the Recycle Bin as `file` came from.
    fn recycled_from(&self, file: u64) -> Option<String> {
        self.events.iter().rev().find(|e| e.kind == Kind::Recycled && e.file == file).map(|e| e.path.clone())
    }

    fn push(&mut self, event: Event) {
        let newest = event.time;
        self.bytes += event.bytes();
        self.events.push_back(event);
        while self.events.len() > MAX_EVENTS || self.bytes > MAX_BYTES || self.events.front().is_some_and(|e| newest - e.time > KEEP_MS) {
            let gone = self.events.pop_front().unwrap();
            self.bytes -= gone.bytes();
        }
    }

    /// The newest first: what happened since `since` (Unix ms) with `query` in its paths (any case),
    /// at most `max` of it; and whether more matched.
    pub fn find(&self, query: &str, since: i64, max: usize) -> (Vec<&Event>, bool) {
        let query = query.trim().to_lowercase();
        let matches = |e: &&Event| {
            e.time >= since && (query.is_empty() || [Some(&e.path), e.to.as_ref(), e.was.as_ref()].into_iter().flatten().any(|p| p.to_lowercase().contains(&query)))
        };
        let mut found: Vec<&Event> = self.events.iter().rev().filter(matches).take(max + 1).collect();
        let limited = found.len() > max;
        found.truncate(max);
        (found, limited)
    }

    /// When the oldest event kept happened.
    pub fn oldest(&self) -> Option<i64> {
        self.events.front().map(|e| e.time)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn memory(&self) -> u64 {
        self.bytes as u64
    }
}

const MAGIC: &[u8; 8] = b"SHUIHIS\0";
const FORMAT: u32 = 1;

/// Writes the history to `path` atomically (a temporary file, then a rename). Layout
/// (little-endian): magic, format, the journal id and next USN, the count, then each event (time,
/// kind, folder, burst, items, file reference, then its path, where it went and where it was, each
/// a u32 length, u32::MAX for none, and UTF-8), and a checksum of everything before it.
pub fn save(path: &Path, history: &History) -> io::Result<()> {
    let mut out = Vec::with_capacity(40 + history.bytes);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&history.journal.to_le_bytes());
    out.extend_from_slice(&history.next_usn.to_le_bytes());
    out.extend_from_slice(&(history.events.len() as u64).to_le_bytes());
    for e in &history.events {
        out.extend_from_slice(&e.time.to_le_bytes());
        out.extend_from_slice(&[KINDS.iter().position(|k| *k == e.kind).unwrap_or(0) as u8, e.dir as u8, e.burst as u8, 0]);
        out.extend_from_slice(&e.items.to_le_bytes());
        out.extend_from_slice(&e.file.to_le_bytes());
        for text in [Some(&e.path), e.to.as_ref(), e.was.as_ref()] {
            match text {
                Some(text) => {
                    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
                    out.extend_from_slice(text.as_bytes());
                }
                None => out.extend_from_slice(&u32::MAX.to_le_bytes()),
            }
        }
    }
    let mut sum = Checksum::default();
    sum.update(&out);
    out.extend_from_slice(&sum.finish().to_le_bytes());
    let temp = path.with_extension("hist.tmp");
    let mut file = fs::File::create(&temp)?;
    file.write_all(&out)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)
}

pub fn load(path: &Path) -> Result<History, String> {
    decode(&fs::read(path).map_err(|e| e.to_string())?)
}

fn decode(bytes: &[u8]) -> Result<History, String> {
    let bad = || "history damaged".to_string();
    if bytes.len() < 40 || &bytes[..8] != MAGIC || bytes[8..12] != FORMAT.to_le_bytes() {
        return Err("not a history of this format".into());
    }
    let end = bytes.len() - 8;
    let mut sum = Checksum::default();
    sum.update(&bytes[..end]);
    if sum.finish().to_le_bytes() != bytes[end..] {
        return Err("history checksum mismatch".into());
    }
    let mut at = 16;
    let mut take = |len: usize| -> Result<&[u8], String> {
        let part = bytes[..end].get(at..at + len).ok_or_else(bad)?;
        at += len;
        Ok(part)
    };
    let u32_of = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let u64_of = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
    let mut history = History { journal: u64_of(take(8)?), next_usn: u64_of(take(8)?) as i64, ..Default::default() };
    let count = u64_of(take(8)?);
    if count > MAX_EVENTS as u64 {
        return Err(bad());
    }
    for _ in 0..count {
        let time = u64_of(take(8)?) as i64;
        let flags = take(4)?.to_vec();
        let items = u32_of(take(4)?);
        let file = u64_of(take(8)?);
        let mut texts = Vec::with_capacity(3);
        for _ in 0..3 {
            let len = u32_of(take(4)?);
            texts.push(if len == u32::MAX { None } else { Some(String::from_utf8(take(len as usize)?.to_vec()).map_err(|_| bad())?) });
        }
        let kind = *KINDS.get(flags[0] as usize).ok_or_else(bad)?;
        let path = texts[0].take().ok_or_else(bad)?;
        let event = Event { time, kind, dir: flags[1] != 0, path, to: texts[1].take(), was: texts[2].take(), file, items, burst: flags[2] != 0 };
        history.bytes += event.bytes();
        history.events.push_back(event);
    }
    if at != end {
        return Err(bad());
    }
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::RecordState;
    use crate::usn::REASON_CLOSE as CLOSE;

    const OLD: u32 = REASON_RENAME_OLD_NAME;
    const NEW: u32 = REASON_RENAME_NEW_NAME;
    const CREATE: u32 = REASON_FILE_CREATE;
    const DELETE: u32 = REASON_FILE_DELETE | CLOSE;
    const DIR: u32 = FILE_ATTRIBUTE_DIRECTORY;
    const FILE: u32 = 0x20;

    /// A journal record of record `file` (sequence number 1) in folder `parent`, at `ms`.
    fn r(ms: i64, file: u32, parent: u32, reason: u32, attrs: u32, name: &str) -> UsnRecord {
        let reference = |n: u32| make_ref(n, if n == ROOT { 5 } else { 1 });
        UsnRecord { usn: ms, file: reference(file), parent: reference(parent), time: ms * 10_000 + 116_444_736_000_000_000, reason, attrs, name: name.into() }
    }

    /// C:\Users\me\{Documents, Downloads}, C:\$Recycle.Bin\S-1-5-21-7
    fn tree() -> Tree {
        let mut tree = Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
        let dir = |parent: u32, name: &str| RecordState { seq: 1, dir: true, names: vec![(make_ref(parent, if parent == ROOT { 5 } else { 1 }), name.into())], ..Default::default() };
        tree.set_record(20, &dir(ROOT, "Users"));
        tree.set_record(21, &dir(20, "me"));
        tree.set_record(22, &dir(21, "Documents"));
        tree.set_record(23, &dir(21, "Downloads"));
        tree.set_record(30, &dir(ROOT, "$Recycle.Bin"));
        tree.set_record(31, &dir(30, "S-1-5-21-7"));
        tree
    }

    fn feed(history: &mut History, tree: &Tree, records: &[UsnRecord]) {
        let mut paths = Paths::new(tree, 'C');
        paths.learn(records);
        history.feed(records, &mut paths);
    }

    fn told(history: &History) -> Vec<String> {
        history
            .events
            .iter()
            .map(|e| {
                let mut line = format!("{:?} {}", e.kind, e.path);
                if let Some(to) = &e.to {
                    line += &format!(" → {to}");
                }
                if let Some(was) = &e.was {
                    line += &format!(" (was {was})");
                }
                if e.items > 0 {
                    line += &format!(" +{}", e.items);
                }
                line
            })
            .collect()
    }

    #[test]
    fn tells_renames_moves_and_the_recycle_bin() {
        let tree = tree();
        let mut history = History::default();
        feed(
            &mut history,
            &tree,
            &[
                // Renamed in place, and the rename's reason on the file's next records.
                r(1_000, 100, 22, OLD, FILE, "a.txt"),
                r(1_001, 100, 22, NEW, FILE, "b.txt"),
                r(1_002, 100, 22, NEW | CLOSE, FILE, "b.txt"),
                // Moved to another folder.
                r(2_000, 101, 23, OLD, FILE, "c.pdf"),
                r(2_001, 101, 22, NEW | CLOSE, FILE, "c.pdf"),
                // Put in the Recycle Bin (with its $I record beside it), then restored.
                r(3_000, 102, 22, OLD, FILE, "d.jpg"),
                r(3_001, 102, 31, NEW | CLOSE, FILE, "$RAB12.jpg"),
                r(3_002, 103, 31, CREATE | CLOSE, FILE, "$IAB12.jpg"),
                r(4_000, 102, 31, OLD, FILE, "$RAB12.jpg"),
                r(4_001, 102, 22, NEW | CLOSE, FILE, "d.jpg"),
                // Recycled, then the bin emptied.
                r(5_000, 104, 23, OLD, FILE, "e.txt"),
                r(5_001, 104, 31, NEW | CLOSE, FILE, "$RCD34.txt"),
                r(6_000, 104, 31, DELETE, FILE, "$RCD34.txt"),
                r(6_001, 105, 31, DELETE, FILE, "$ICD34.txt"),
            ],
        );
        let bin = r"C:\$Recycle.Bin\S-1-5-21-7";
        assert_eq!(
            told(&history),
            [
                r"Renamed C:\Users\me\Documents\a.txt → C:\Users\me\Documents\b.txt".to_string(),
                r"Moved C:\Users\me\Downloads\c.pdf → C:\Users\me\Documents\c.pdf".to_string(),
                format!(r"Recycled C:\Users\me\Documents\d.jpg → {bin}\$RAB12.jpg"),
                format!(r"Restored {bin}\$RAB12.jpg → C:\Users\me\Documents\d.jpg (was C:\Users\me\Documents\d.jpg)"),
                format!(r"Recycled C:\Users\me\Downloads\e.txt → {bin}\$RCD34.txt"),
                format!(r"Emptied {bin}\$RCD34.txt (was C:\Users\me\Downloads\e.txt)"),
            ]
        );
        assert_eq!((history.events[0].file, history.events[0].time), (make_ref(100, 1), 1_001));
    }

    #[test]
    fn leaves_out_what_programs_do_with_their_own_files() {
        let tree = tree();
        let mut history = History::default();
        feed(
            &mut history,
            &tree,
            &[
                // Deleted for good: told.
                r(1_000, 110, 22, DELETE, FILE, "old.log"),
                // A folder deleted with what was in it: one event, counting them.
                r(2_000, 121, 120, DELETE, FILE, "x.rs"),
                r(2_001, 122, 120, DELETE, FILE, "y.rs"),
                r(2_002, 120, 22, DELETE, DIR, "Project"),
                // Made and gone again: a scratch file.
                r(3_000, 130, 22, CREATE, FILE, "draft.txt"),
                r(4_000, 130, 22, DELETE | CREATE, FILE, "draft.txt"),
                // Office's owner file, and a temporary one.
                r(5_000, 131, 22, DELETE, FILE, "~$report.docx"),
                r(5_001, 132, 22, DELETE, FILE | FILE_ATTRIBUTE_TEMPORARY, "data.bin"),
                // A save: the old file deleted, and a scratch file renamed to its name at once.
                r(6_000, 140, 22, DELETE, FILE, "report.docx"),
                r(6_001, 141, 22, OLD, FILE, "~WRL0001.tmp"),
                r(6_002, 141, 22, NEW | CLOSE, FILE, "report.docx"),
                // A new folder getting its name.
                r(8_000, 160, 22, CREATE | CLOSE, DIR, "New folder"),
                r(8_001, 160, 22, OLD, DIR, "New folder"),
                r(8_002, 160, 22, NEW | CLOSE, DIR, "Photos"),
            ],
        );
        assert_eq!(told(&history), [r"Deleted C:\Users\me\Documents\old.log", r"Deleted C:\Users\me\Documents\Project +2"]);
        assert!(history.events[1].dir);
        // AppData, Windows', caches, packages and Git's: left out; a folder of one's own named Temp isn't.
        for path in [r"C:\Users\me\AppData\Local\x.dat", r"C:\Windows\Temp\a.etl", r"C:\Users\me\Documents\node_modules\a.js", r"C:\src\.git\objects\ab\cd", r"C:\x\Cache\f_0001"] {
            assert!(noise(path), "{path}");
        }
        for path in [r"C:\Users\me\Documents\Temp", r"C:\Users\me\Desktop\notes.txt", r"D:\Photos\cache.jpg"] {
            assert!(!noise(path), "{path}");
        }
    }

    #[test]
    fn a_folder_deleted_while_open_is_moved_aside_first() {
        // As a real journal tells PowerShell's Remove-Item -Recurse: what's in a folder is deleted,
        // then the folder (still open) is moved into $Extend\$Deleted, and deleted there once closed.
        let mut tree = tree();
        let dir = |parent: u32, name: &str| RecordState { seq: 1, dir: true, names: vec![(make_ref(parent, 1), name.into())], ..Default::default() };
        tree.set_record(29, &dir(EXTEND as u32, "$Deleted"));
        tree.set_record(40, &dir(22, "Proj"));
        tree.set_record(41, &dir(40, "src"));
        let mut history = History::default();
        feed(
            &mut history,
            &tree,
            &[
                r(1_000, 50, 41, DELETE, FILE, "f1.rs"),
                r(1_001, 51, 41, DELETE, FILE, "f2.rs"),
                r(1_002, 41, 40, OLD, DIR, "src"),
                r(1_003, 41, 29, NEW, DIR, "$DeleteA1B2"),
                r(1_004, 52, 40, DELETE, FILE, "README.md"),
                r(1_005, 40, 22, OLD, DIR, "Proj"),
                r(1_006, 40, 29, NEW, DIR, "$DeleteC3D4"),
                r(1_007, 40, 29, DELETE | NEW, DIR, "$DeleteC3D4"),
                r(1_008, 41, 29, DELETE | NEW, DIR, "$DeleteA1B2"),
            ],
        );
        assert_eq!(told(&history), [r"Deleted C:\Users\me\Documents\Proj +4"]);
        assert!(set_aside(r"C:\$Extend\$Deleted\x") && !set_aside(r"C:\$Extend\x") && !set_aside(r"C:\Users\$Deleted"));
    }

    #[test]
    fn names_folders_as_they_were_at_each_change() {
        let mut tree = tree();
        let mut history = History::default();
        // Documents is renamed Docs between two deletes in it.
        let records = [
            r(1_000, 170, 22, DELETE, FILE, "a.txt"),
            r(1_001, 22, 21, OLD, DIR, "Documents"),
            r(1_002, 22, 21, NEW | CLOSE, DIR, "Docs"),
            r(1_003, 171, 22, DELETE, FILE, "b.txt"),
        ];
        feed(&mut history, &tree, &records);
        let expected = [r"Deleted C:\Users\me\Documents\a.txt", r"Renamed C:\Users\me\Documents → C:\Users\me\Docs", r"Deleted C:\Users\me\Docs\b.txt"];
        assert_eq!(told(&history), expected);
        // Read later, when the index already has the new name: the records still tell the old one.
        tree.set_record(22, &RecordState { seq: 1, dir: true, names: vec![(make_ref(21, 1), "Docs".into())], ..Default::default() });
        let mut later = History::default();
        feed(&mut later, &tree, &records);
        assert_eq!(told(&later), expected);
        // A folder neither knows: nothing to say where it was.
        let mut unknown = History::default();
        feed(&mut unknown, &tree, &[r(1_000, 180, 999, DELETE, FILE, "lost.txt")]);
        assert!(unknown.is_empty());
    }

    #[test]
    fn counts_a_burst_of_deletes_past_its_first() {
        let tree = tree();
        let mut history = History::default();
        let records: Vec<UsnRecord> = (0..60).map(|i| r(1_000 + i * 100, 200 + i as u32, 23, DELETE, FILE, &format!("f{i}.bin"))).collect();
        feed(&mut history, &tree, &records);
        assert_eq!(history.len(), 51);
        assert_eq!(told(&history).last().unwrap(), r"Deleted C:\Users\me\Downloads +10");
        assert!(history.events[50].burst);
        // Half a minute later, a new burst: listed again.
        feed(&mut history, &tree, &[r(100_000, 300, 23, DELETE, FILE, "later.bin")]);
        assert_eq!(told(&history).last().unwrap(), r"Deleted C:\Users\me\Downloads\later.bin");
        // The folder deleted after all: everything in it folds into it.
        feed(&mut history, &tree, &[r(101_000, 23, 21, DELETE, DIR, "Downloads")]);
        assert_eq!(told(&history), [r"Deleted C:\Users\me\Downloads +61"]);
    }

    #[test]
    fn finds_keeps_and_saves() {
        let tree = tree();
        let mut history = History { journal: 7, next_usn: 99, ..Default::default() };
        feed(
            &mut history,
            &tree,
            &[
                r(1_000, 100, 22, OLD, FILE, "Été.txt"),
                r(1_001, 100, 22, NEW | CLOSE, FILE, "summer.txt"),
                r(2_000, 101, 22, DELETE, FILE, "report.pdf"),
                r(3_000, 102, 23, DELETE, FILE, "setup.exe"),
            ],
        );
        let paths = |(found, limited): (Vec<&Event>, bool)| (found.iter().map(|e| name_of(&e.path).to_string()).collect::<Vec<_>>(), limited);
        assert_eq!(paths(history.find("", 0, 10)), (vec!["setup.exe".to_string(), "report.pdf".into(), "Été.txt".into()], false));
        assert_eq!(paths(history.find("été", 0, 10)), (vec!["Été.txt".to_string()], false));
        assert_eq!(paths(history.find("SUMMER", 0, 10)), (vec!["Été.txt".to_string()], false));
        assert_eq!(paths(history.find("documents", 0, 1)), (vec!["report.pdf".to_string()], true));
        assert_eq!(paths(history.find("", 2_500, 10)), (vec!["setup.exe".to_string()], false));

        let dir = std::env::temp_dir().join(format!("shadui-index-history-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("C.hist");
        save(&file, &history).unwrap();
        let back = load(&file).unwrap();
        assert_eq!((back.journal, back.next_usn, back.events.clone(), back.bytes), (7, 99, history.events.clone(), history.bytes));
        let mut bytes = fs::read(&file).unwrap();
        bytes[60] ^= 1;
        assert!(decode(&bytes).err().unwrap().contains("checksum"));
        assert!(decode(&bytes[..30]).is_err());
        fs::remove_dir_all(&dir).unwrap();

        // Kept 30 days.
        feed(&mut history, &tree, &[r(KEEP_MS + 2_500, 103, 22, DELETE, FILE, "new.txt")]);
        assert_eq!(paths(history.find("", 0, 10)).0, ["new.txt", "setup.exe"]);
    }
}
