//! The running index: a worker per NTFS drive (read the MFT or load the last snapshot, catch up with
//! the journal, then follow it), and the answers to the app's requests.

use crate::proto::{self, unix_ms, Hit, Message, SearchRequest, Sizes, Status, UsageNode, UsageRequest, UsageSummary, VolumeStatus, FRAME_HITS, FRAME_USAGE, NO_PARENT};
use crate::query::{Excludes, NameMatcher};
use crate::snapshot::{self, Meta};
use crate::tree::{Search, Tree, NONE, ROOT};
use crate::usn;
use crate::volume::{self, Journal, JournalError, Volume};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often an idle journal is checked.
const POLL: Duration = Duration::from_millis(250);
/// After a scan, changes younger than this are read again: the disk's copy of the MFT can lag.
const SCAN_REPLAY: Duration = Duration::from_secs(120);
const SAVE_EVERY: Duration = Duration::from_secs(30 * 60);
const RECOUNT_EVERY: Duration = Duration::from_secs(60 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Results per frame.
const BATCH: usize = 4096;
/// Even a search with no limit stops here: more than any list can show.
pub const MAX_HITS: usize = 1_000_000;
/// Disk-usage maps go at most this deep and hold at most this many items: far more than a screen
/// can show.
const MAX_USAGE_DEPTH: u32 = 32;
const MAX_USAGE_ITEMS: usize = 500_000;

/// A stop signal that sleeping threads wake up for.
#[derive(Default)]
pub struct Stop {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Stop {
    pub fn set(&self) {
        *self.stopped.lock().unwrap() = true;
        self.wake.notify_all();
    }

    pub fn is_set(&self) -> bool {
        *self.stopped.lock().unwrap()
    }

    /// Sleeps up to `time`; true if it was stopped meanwhile.
    pub fn sleep(&self, time: Duration) -> bool {
        let stopped = self.stopped.lock().unwrap();
        *self.wake.wait_timeout_while(stopped, time, |s| !*s).unwrap().0
    }

    pub fn wait(&self) {
        let stopped = self.stopped.lock().unwrap();
        drop(self.wake.wait_while(stopped, |s| !*s).unwrap());
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// A line-per-event log in the data folder (and on the console when run there).
pub struct Log {
    file: Mutex<Option<fs::File>>,
    echo: bool,
}

impl Log {
    pub fn open(path: &Path, echo: bool) -> Log {
        // Start over past 1 MiB, so it never grows unbounded.
        if fs::metadata(path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = fs::rename(path, path.with_extension("old.log"));
        }
        let file = fs::OpenOptions::new().create(true).append(true).open(path).ok();
        Log { file: Mutex::new(file), echo }
    }

    pub fn line(&self, text: &str) {
        let secs = now_ms() / 1000;
        let (days, rest) = (secs / 86_400, secs % 86_400);
        let line = format!("day {days} {:02}:{:02}:{:02} UTC  {text}\n", rest / 3600, rest / 60 % 60, rest % 60);
        if self.echo {
            eprint!("{line}");
        }
        if let Some(file) = self.file.lock().unwrap().as_mut() {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// One drive's index and how it's doing.
struct Slot {
    letter: char,
    tree: RwLock<Tree>,
    ready: AtomicBool,
    status: Mutex<VolumeStatus>,
}

enum Restart {
    /// The journal can't bring the index up to date any more: read the MFT again now.
    Rescan(String),
    /// Something failed (the drive went away, a read error…): try again in a while.
    Retry(String),
}

pub struct Engine {
    data: PathBuf,
    /// Index only these drives (for testing from a console); all fixed NTFS drives when None.
    only: Option<Vec<char>>,
    slots: RwLock<Vec<Arc<Slot>>>,
    stop: Stop,
    workers: Mutex<Vec<JoinHandle<()>>>,
    pub log: Log,
}

impl Engine {
    pub fn start(data: PathBuf, only: Option<Vec<char>>, echo: bool) -> Arc<Engine> {
        let _ = fs::create_dir_all(&data);
        let log = Log::open(&data.join("service.log"), echo);
        let engine = Arc::new(Engine { data, only, slots: RwLock::default(), stop: Stop::default(), workers: Mutex::default(), log });
        engine.log.line(&format!("starting {} {}", crate::SERVICE_NAME, crate::VERSION));
        engine.discover();
        // Drives can appear later (an external disk, a volume unlocked by BitLocker).
        let watcher = engine.clone();
        let handle = thread::spawn(move || {
            while !watcher.stop.sleep(Duration::from_secs(60)) {
                watcher.discover();
            }
        });
        engine.workers.lock().unwrap().push(handle);
        engine
    }

    /// An engine serving one ready drive from `tree`, with no workers: for tests.
    #[cfg(test)]
    pub(crate) fn with_tree(letter: char, tree: Tree) -> Arc<Engine> {
        let status = VolumeStatus { volume: format!("{letter}:"), state: "ready".into(), ..Default::default() };
        let slot = Arc::new(Slot { letter, tree: RwLock::new(tree), ready: AtomicBool::new(true), status: Mutex::new(status) });
        let log = Log { file: Mutex::new(None), echo: false };
        Arc::new(Engine { data: PathBuf::new(), only: None, slots: RwLock::new(vec![slot]), stop: Stop::default(), workers: Mutex::default(), log })
    }

    /// Stops every worker; each saves its snapshot if anything changed since the last one.
    pub fn stop(&self) {
        self.stop.set();
        let workers = std::mem::take(&mut *self.workers.lock().unwrap());
        for worker in workers {
            let _ = worker.join();
        }
        self.log.line("stopped");
    }

    fn discover(self: &Arc<Self>) {
        let drives = volume::fixed_ntfs_drives();
        let mut slots = self.slots.write().unwrap();
        for letter in drives {
            if self.stop.is_set() || self.only.as_ref().is_some_and(|only| !only.contains(&letter)) || slots.iter().any(|s| s.letter == letter) {
                continue;
            }
            let status = VolumeStatus { volume: format!("{letter}:"), state: "scanning".into(), ..Default::default() };
            let slot = Arc::new(Slot { letter, tree: RwLock::default(), ready: AtomicBool::new(false), status: Mutex::new(status) });
            slots.push(slot.clone());
            let engine = self.clone();
            let handle = thread::Builder::new().name(format!("index {letter}:")).spawn(move || engine.worker(&slot)).expect("index worker");
            self.workers.lock().unwrap().push(handle);
        }
    }

    fn worker(&self, slot: &Slot) {
        let mut rescan = false;
        let mut last_rescan: Option<Instant> = None;
        while !self.stop.is_set() {
            match self.follow(slot, rescan) {
                Ok(()) => return,
                Err(Restart::Rescan(why)) => {
                    self.log.line(&format!("{}: {why}; reading the MFT again", slot.letter));
                    slot.ready.store(false, Ordering::SeqCst);
                    // A journal that keeps failing mustn't turn into back-to-back full reads of the disk.
                    if last_rescan.is_some_and(|at| at.elapsed() < Duration::from_secs(60)) && self.stop.sleep(RETRY_AFTER) {
                        return;
                    }
                    (rescan, last_rescan) = (true, Some(Instant::now()));
                }
                Err(Restart::Retry(why)) => {
                    self.log.line(&format!("{}: {why}; trying again in {}s", slot.letter, RETRY_AFTER.as_secs()));
                    slot.ready.store(false, Ordering::SeqCst);
                    *slot.tree.write().unwrap() = Tree::default();
                    {
                        let mut status = slot.status.lock().unwrap();
                        status.state = "error".into();
                        status.error = Some(why);
                    }
                    if self.stop.sleep(RETRY_AFTER) {
                        return;
                    }
                }
            }
        }
    }

    /// Builds the drive's index and keeps it current until stopped.
    fn follow(&self, slot: &Slot, rescan: bool) -> Result<(), Restart> {
        let retry = |what: &str, e: io::Error| Restart::Retry(format!("{what}: {e}"));
        let volume = Volume::open(slot.letter).map_err(|e| retry("can't open the drive", e))?;
        let journal = match volume.journal().map_err(|e| retry("can't read the change journal", e))? {
            Some(journal) => journal,
            None => {
                volume.create_journal().map_err(|e| retry("can't turn the change journal on", e))?;
                self.log.line(&format!("{}: turned the change journal on", slot.letter));
                volume.journal().map_err(|e| retry("can't read the change journal", e))?.ok_or_else(|| Restart::Retry("no change journal".into()))?
            }
        };
        let path = self.data.join(format!("{:016X}.idx", volume.serial));
        let started = Instant::now();
        let saved = if rescan { None } else { self.load(slot.letter, &path, &volume, &journal) };
        let (mut cursor, since, source) = match saved {
            Some((tree, meta)) => {
                *slot.tree.write().unwrap() = tree;
                (meta.next_usn, None, "snapshot")
            }
            None => {
                *slot.status.lock().unwrap() = VolumeStatus { volume: format!("{}:", slot.letter), state: "scanning".into(), ..Default::default() };
                let since = (now_ms() - SCAN_REPLAY.as_millis() as i64) * 10_000 + 116_444_736_000_000_000;
                let progress = |done: u64, total: u64| {
                    let mut status = slot.status.lock().unwrap();
                    (status.progress, status.progress_total) = (done, total);
                    !self.stop.is_set()
                };
                let tree = match volume.scan(&progress) {
                    Ok(tree) => tree,
                    Err(_) if self.stop.is_set() => return Ok(()),
                    Err(e) => return Err(retry("can't read the MFT", e)),
                };
                self.log.line(&format!("{}: read the MFT in {:.0} ms", slot.letter, ms(started)));
                *slot.tree.write().unwrap() = tree;
                // The whole journal is replayed from its current start (0), but only recent changes are reread.
                (0, Some(since), "scan")
            }
        };

        let mut page = vec![0u8; 1 << 20];
        let mut replayed = 0;
        loop {
            match self.apply(&volume, slot, &mut cursor, journal.id, since, &mut page)? {
                0 => break,
                read => replayed += read,
            }
            if self.stop.is_set() {
                return Ok(());
            }
        }
        if cursor == 0 {
            cursor = journal.next_usn;
        }
        let build_ms = ms(started);
        {
            let mut status = slot.status.lock().unwrap();
            status.state = "ready".into();
            status.source = source.into();
            status.build_ms = build_ms;
            status.error = None;
            status.progress = status.progress_total;
        }
        slot.ready.store(true, Ordering::SeqCst);
        let stats = slot.tree.read().unwrap().stats();
        self.log.line(&format!(
            "{}: ready from {source} in {build_ms:.0} ms: {} files, {} folders, {} MB in memory",
            slot.letter,
            stats.files,
            stats.folders,
            stats.memory >> 20
        ));

        let meta = |cursor| Meta { volume_serial: volume.serial, journal_id: journal.id, next_usn: cursor, saved_at: now_ms() };
        if source == "scan" {
            self.save(slot, &path, meta(cursor));
        }
        // A snapshot the journal had to catch up gets saved again at the next chance.
        let (mut dirty, mut saved_at, mut counted_at) = (source == "snapshot" && replayed > 0, Instant::now(), Instant::now());
        loop {
            if self.stop.is_set() {
                if dirty {
                    self.save(slot, &path, meta(cursor));
                }
                return Ok(());
            }
            if self.apply(&volume, slot, &mut cursor, journal.id, None, &mut page)? > 0 {
                dirty = true;
                continue;
            }
            if dirty && saved_at.elapsed() > SAVE_EVERY {
                self.save(slot, &path, meta(cursor));
                (dirty, saved_at) = (false, Instant::now());
            }
            if counted_at.elapsed() > RECOUNT_EVERY {
                let mut tree = slot.tree.write().unwrap();
                tree.recompute_totals();
                tree.maybe_compact();
                counted_at = Instant::now();
            }
            self.stop.sleep(POLL);
        }
    }

    /// The last snapshot, if the journal still reaches back to where it left off.
    fn load(&self, letter: char, path: &Path, volume: &Volume, journal: &Journal) -> Option<(Tree, Meta)> {
        let started = Instant::now();
        let (tree, meta) = match snapshot::load(path) {
            Ok(loaded) => loaded,
            Err(e) => {
                if path.exists() {
                    self.log.line(&format!("{letter}: snapshot unusable ({e})"));
                }
                return None;
            }
        };
        let usable = meta.volume_serial == volume.serial
            && meta.journal_id == journal.id
            && meta.next_usn >= journal.first_usn
            && meta.next_usn <= journal.next_usn;
        if !usable {
            self.log.line(&format!("{letter}: the journal no longer reaches back to the snapshot"));
            return None;
        }
        self.log.line(&format!("{letter}: loaded the snapshot in {:.0} ms", ms(started)));
        Some((tree, meta))
    }

    fn save(&self, slot: &Slot, path: &Path, meta: Meta) {
        let started = Instant::now();
        let tree = slot.tree.read().unwrap();
        match snapshot::save(path, &tree, &meta) {
            Ok(()) => self.log.line(&format!("{}: saved the snapshot in {:.0} ms", slot.letter, ms(started))),
            Err(e) => self.log.line(&format!("{}: couldn't save the snapshot: {e}", slot.letter)),
        }
    }

    /// Reads the next page of the journal and brings every record it touched up to date. Returns
    /// how many journal records it read: 0 once caught up.
    fn apply(&self, volume: &Volume, slot: &Slot, cursor: &mut i64, journal: u64, since: Option<i64>, page: &mut [u8]) -> Result<usize, Restart> {
        let len = match volume.read_journal(*cursor, journal, page) {
            Ok(len) => len,
            // During post-scan replay (since.is_some()), we only need recent changes near the head of
            // the journal; if the oldest tail chunk was trimmed while scanning/reading an active C:
            // drive, jump to the current FirstUsn (0) instead of throwing away the finished scan.
            Err(JournalError::Lost) if since.is_some() && *cursor != 0 => {
                *cursor = 0;
                volume.read_journal(0, journal, page).map_err(|e| match e {
                    JournalError::Lost => Restart::Rescan("the change journal no longer goes back far enough".into()),
                    JournalError::Io(e) => Restart::Retry(format!("can't read the change journal: {e}")),
                })?
            }
            Err(JournalError::Lost) => return Err(Restart::Rescan("the change journal no longer goes back far enough".into())),
            Err(JournalError::Io(e)) => return Err(Restart::Retry(format!("can't read the change journal: {e}"))),
        };
        let Some((next, records)) = usn::parse_page(&page[..len]) else { return Ok(0) };
        if records.is_empty() {
            *cursor = next.max(*cursor);
            return Ok(0);
        }
        // Read what each record holds now before taking the lock, so searches don't wait on the disk.
        // A record that can't be read right now is left as it is.
        let states: Vec<_> = usn::touched(&records, since).into_iter().filter_map(|n| volume.record_state(n).ok().map(|s| (n, s))).collect();
        if !states.is_empty() {
            let mut tree = slot.tree.write().unwrap();
            for (number, state) in &states {
                match state {
                    Some(state) => tree.set_record(*number, state),
                    None => tree.remove_record(*number),
                }
            }
            slot.status.lock().unwrap().updated = now_ms();
        }
        *cursor = next;
        Ok(records.len())
    }

    fn ready_slot(&self, letter: char) -> Option<Arc<Slot>> {
        self.slots.read().unwrap().iter().find(|s| s.letter == letter && s.ready.load(Ordering::SeqCst)).cloned()
    }

    pub fn status(&self) -> Status {
        let slots = self.slots.read().unwrap().clone();
        let volumes = slots
            .iter()
            .map(|slot| {
                let mut status = slot.status.lock().unwrap().clone();
                if slot.ready.load(Ordering::SeqCst) {
                    let stats = slot.tree.read().unwrap().stats();
                    (status.files, status.folders, status.memory) = (stats.files, stats.folders, stats.memory);
                }
                status
            })
            .collect();
        Status { version: crate::VERSION.into(), volumes }
    }

    /// Answers a name search: a plan, result frames, then a summary.
    pub fn search(&self, request: &SearchRequest, out: &mut dyn Write) -> io::Result<()> {
        let started = Instant::now();
        let Some(matcher) = NameMatcher::new(&request.query) else {
            return proto::write_message(out, &Message::Error { message: "Type part of a file name to search for".into() });
        };
        let excludes = Excludes::new(&request.exclude);
        let (mut targets, mut indexed, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
        for root in &request.roots {
            let target = split_drive(root).and_then(|(letter, rest)| {
                let slot = self.ready_slot(letter)?;
                let scope = slot.tree.read().unwrap().resolve(rest)?;
                Some((slot, scope))
            });
            match target {
                Some(target) => {
                    targets.push(target);
                    indexed.push(root.clone());
                }
                None => skipped.push(root.clone()),
            }
        }
        proto::write_message(out, &Message::Plan { indexed, skipped })?;

        let max = request.max_results.filter(|&m| m > 0).unwrap_or(MAX_HITS).min(MAX_HITS);
        let (mut count, mut limited) = (0, false);
        for (slot, scope) in targets {
            if count >= max {
                limited = true;
                break;
            }
            // Results are encoded under the lock (ids are only valid until the next change), then sent.
            let (frames, found) = {
                let tree = slot.tree.read().unwrap();
                let search = Search { matcher: &matcher, scope, hidden: request.hidden, excludes: &excludes };
                collect_hits(&tree, slot.letter, &search, max - count)
            };
            let taken = found.min(max - count);
            limited |= taken < found;
            count += taken;
            for frame in frames {
                proto::write_frame(out, FRAME_HITS, &frame)?;
            }
        }
        proto::write_message(out, &Message::Done { count: count as u64, limited, elapsed: ms(started) })
    }

    /// Folder sizes: the folder's total and the total of each folder in it.
    pub fn sizes(&self, path: &str) -> Result<Sizes, String> {
        let started = Instant::now();
        let (letter, rest) = split_drive(path).ok_or("Not a folder on a local drive")?;
        let slot = self.ready_slot(letter).ok_or("This drive isn't indexed")?;
        let tree = slot.tree.read().unwrap();
        let dir = tree.resolve(rest).ok_or("This folder isn't in the index")?;
        let total = tree.rec(dir).filter(|r| r.dir()).ok_or("Not a folder")?.size;
        Ok(Sizes { path: path.to_string(), total, sizes: tree.child_dirs(dir), elapsed: ms(started) })
    }

    /// Answers a disk-usage map: one frame with the map, then a summary.
    pub fn usage(&self, request: &UsageRequest, out: &mut dyn Write) -> io::Result<()> {
        match self.map(request) {
            Ok((payload, summary)) => {
                proto::write_frame(out, FRAME_USAGE, &payload)?;
                proto::write_message(out, &Message::Usage(summary))
            }
            Err(message) => proto::write_message(out, &Message::Error { message }),
        }
    }

    fn map(&self, request: &UsageRequest) -> Result<(Vec<u8>, UsageSummary), String> {
        let started = Instant::now();
        let (letter, rest) = split_drive(&request.path).ok_or("Not a folder on a local drive")?;
        let slot = self.ready_slot(letter).ok_or("This drive isn't indexed")?;
        // Encoded under the lock: names are only valid until the next change.
        let tree = slot.tree.read().unwrap();
        let dir = tree.resolve(rest).ok_or("This folder isn't in the index")?;
        let depth = request.depth.clamp(1, MAX_USAGE_DEPTH);
        let (items, limited) = tree.usage(dir, depth, (request.max_items as usize).min(MAX_USAGE_ITEMS));
        let mut payload = Vec::with_capacity(items.len() * 48);
        for item in &items {
            let (rec, name, parent) = if item.link == NONE {
                let name = tree.rec(dir).map_or(NONE, |r| r.first_link);
                (dir, if dir == ROOT || name == NONE { "" } else { tree.name(name) }, NO_PARENT)
            } else {
                (tree.link(item.link).rec, tree.name(item.link), item.parent)
            };
            let Some(r) = tree.rec(rec) else { continue };
            let node = UsageNode { parent, name: name.to_string(), dir: item.dir, size: item.size, modified: unix_ms(r.modified), attrs: r.attrs };
            proto::encode_usage_node(&mut payload, &node);
        }
        let total = items.first().ok_or("Not a folder")?.size;
        let summary = UsageSummary { path: request.path.clone(), total, items: items.len() as u64, limited, elapsed: ms(started) };
        Ok((payload, summary))
    }
}

/// Searches one drive and encodes up to `max` results into frames; also returns how many matched.
pub(crate) fn collect_hits(tree: &Tree, letter: char, search: &Search, max: usize) -> (Vec<Vec<u8>>, usize) {
    let found = tree.search(search);
    let drive = format!("{letter}:\\");
    let frames = found[..found.len().min(max)].par_chunks(BATCH).map(|ids| encode_hits(tree, ids, &drive)).collect();
    (frames, found.len())
}

fn encode_hits(tree: &Tree, ids: &[u32], drive: &str) -> Vec<u8> {
    let mut dirs = HashMap::new();
    let mut out = Vec::with_capacity(ids.len() * 96);
    for &id in ids {
        let Some(path) = tree.path(id, &mut dirs) else { continue };
        let Some(rec) = tree.rec(tree.link(id).rec) else { continue };
        let hit = Hit { path: format!("{drive}{path}"), dir: rec.dir(), size: rec.size, modified: unix_ms(rec.modified), attrs: rec.attrs };
        proto::encode_hit(&mut out, &hit);
    }
    out
}

/// The drive letter and the path below its root, for `C:\Users\me` or `\\?\C:\Users\me`.
pub fn split_drive(path: &str) -> Option<(char, &str)> {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let bytes = path.as_bytes();
    if bytes.len() < 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let rest = &path[2..];
    // "C:docs" means "docs in C:'s current folder", which a service can't know.
    if !(rest.is_empty() || rest.starts_with(['\\', '/'])) {
        return None;
    }
    Some((bytes[0].to_ascii_uppercase() as char, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_drive_paths() {
        assert_eq!(split_drive(r"c:\Users\me"), Some(('C', r"\Users\me")));
        assert_eq!(split_drive(r"D:\"), Some(('D', r"\")));
        assert_eq!(split_drive("E:"), Some(('E', "")));
        assert_eq!(split_drive(r"\\?\C:\Windows"), Some(('C', r"\Windows")));
        assert_eq!(split_drive("C:docs"), None);
        assert_eq!(split_drive(r"\\server\share"), None);
        assert_eq!(split_drive("1:"), None);
    }

    #[test]
    fn stop_wakes_sleepers() {
        let stop = Arc::new(Stop::default());
        let sleeper = stop.clone();
        let started = Instant::now();
        let handle = thread::spawn(move || sleeper.sleep(Duration::from_secs(30)));
        thread::sleep(Duration::from_millis(20));
        stop.set();
        assert!(handle.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(stop.sleep(Duration::from_secs(30)));
    }
}
