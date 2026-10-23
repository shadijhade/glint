//! The running index: a worker per NTFS drive (read the MFT or load the last snapshot, catch up with
//! the journal, then follow it), and the answers to the app's requests.

use crate::growth::Growth;
use crate::history::{self, History, Paths};
use crate::proto::{
    self, unix_ms, GrowthSummary, Hit, Message, SearchRequest, Sizes, Status, UsageNode, UsageRequest, UsageSummary, VolumeStatus, FRAME_GROWTH, FRAME_HITS, FRAME_USAGE,
    NEW, NO_PARENT, WRITTEN,
};
use crate::query::{self, Excludes, NameMatcher};
use crate::snapshot::{self, Meta};
use crate::tree::{self, Search, Tree, NONE, ROOT};
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
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;

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
pub const MAX_HITS: usize = 500_000;
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

/// The day `ms` (Unix ms) falls on in this PC's time zone, counted from 1601: marks of what grew
/// are taken a day apart, as people count days.
fn local_day(ms: i64) -> i64 {
    let utc = ms * 10_000 + 116_444_736_000_000_000;
    let at = FILETIME { dwLowDateTime: utc as u32, dwHighDateTime: (utc >> 32) as u32 };
    let mut local = FILETIME::default();
    let local = match unsafe { FileTimeToLocalFileTime(&at, &mut local) } {
        Ok(()) => (local.dwHighDateTime as i64) << 32 | local.dwLowDateTime as i64,
        Err(_) => utc,
    };
    local / 864_000_000_000
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

/// A map's frame, what its items held at a mark (when asked), and its summary.
type MapAnswer = (Vec<u8>, Option<Vec<u8>>, UsageSummary);

/// One drive's index and how it's doing.
struct Slot {
    letter: char,
    tree: RwLock<Tree>,
    ready: AtomicBool,
    status: Mutex<VolumeStatus>,
    /// What was renamed, moved and deleted. Taken before the tree when both are.
    history: Mutex<History>,
    /// Whether the saved history was read (once: it outlives a new read of the disk).
    history_loaded: AtomicBool,
}

impl Slot {
    fn new(letter: char, tree: Tree, ready: bool, status: VolumeStatus) -> Slot {
        Slot { letter, tree: RwLock::new(tree), ready: AtomicBool::new(ready), status: Mutex::new(status), history: Mutex::default(), history_loaded: AtomicBool::new(false) }
    }
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
        let slot = Arc::new(Slot::new(letter, tree, true, status));
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
            let slot = Arc::new(Slot::new(letter, Tree::default(), false, status));
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
        // By serial and letter: a cloned disk next to its original has the same serial (and journal).
        let path = self.data.join(format!("{:016X}-{}.idx", volume.serial, slot.letter));
        let legacy = self.data.join(format!("{:016X}.idx", volume.serial));
        if !path.exists() && legacy.exists() {
            let _ = fs::rename(&legacy, &path);
        }
        // What was renamed, moved and deleted: read once, then kept across new reads of the disk.
        let history_path = path.with_extension("hist");
        if !slot.history_loaded.swap(true, Ordering::SeqCst) {
            match history::load(&history_path) {
                Ok(saved) => *slot.history.lock().unwrap() = saved,
                Err(e) if history_path.exists() => self.log.line(&format!("{}: history unusable ({e})", slot.letter)),
                Err(_) => {}
            }
        }
        let started = Instant::now();
        // What grew carries over a new read of the disk (as far as it can): from the index read last,
        // or the snapshot the journal no longer reaches.
        let (saved, mut growth) = if rescan {
            (None, std::mem::take(&mut slot.tree.write().unwrap().growth))
        } else {
            self.load(slot.letter, &path, &volume, &journal)
        };
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
                let mut tree = match volume.scan(&progress) {
                    Ok(tree) => tree,
                    Err(_) if self.stop.is_set() => return Ok(()),
                    Err(e) => return Err(retry("can't read the MFT", e)),
                };
                self.log.line(&format!("{}: read the MFT in {:.0} ms", slot.letter, ms(started)));
                // Records may hold other folders now than the notes say: folders compare from now on.
                if growth.tracking() {
                    growth.restart(now_ms(), tree.rec(ROOT).map_or(0, |r| r.size));
                }
                tree.growth = std::mem::take(&mut growth);
                *slot.tree.write().unwrap() = tree;
                // The whole journal is replayed, but only recent changes are reread.
                (journal.first_usn, Some(since), "scan")
            }
        };

        let mut page = vec![0u8; 1 << 20];
        // The index stands where the snapshot left off, or (read from the disk) where the journal was.
        let stands_at = if source == "snapshot" { cursor } else { journal.next_usn };
        self.catch_up_history(&volume, slot, &journal, stands_at, &mut page);
        let mut replayed = 0;
        loop {
            let read = match self.apply(&volume, slot, &mut cursor, journal.id, since, &mut page) {
                // A busy drive's journal can drop its oldest records while the drive is read. What's
                // left is enough if it still reaches back to where the journal was when the read
                // began: every change the read could have missed comes after that.
                Err(Restart::Rescan(why)) if since.is_some() => match volume.journal() {
                    Ok(Some(now)) if now.id == journal.id && now.first_usn > cursor && now.first_usn <= journal.next_usn => {
                        cursor = now.first_usn;
                        continue;
                    }
                    _ => return Err(Restart::Rescan(why)),
                },
                read => read?,
            };
            if read == 0 {
                break;
            }
            replayed += read;
            if self.stop.is_set() {
                return Ok(());
            }
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
        let marked = self.mark_day(slot);
        if source == "scan" {
            self.save(slot, &path, meta(cursor));
        }
        // A snapshot the journal had to catch up gets saved again at the next chance.
        let (mut dirty, mut saved_at, mut counted_at) = (source == "snapshot" && (replayed > 0 || marked), Instant::now(), Instant::now());
        let mut today = local_day(now_ms());
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
            if local_day(now_ms()) != today {
                dirty |= self.mark_day(slot);
                today = local_day(now_ms());
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

    /// The last snapshot, if the journal still reaches back to where it left off; otherwise what grew,
    /// as it says (a snapshot of this drive's).
    fn load(&self, letter: char, path: &Path, volume: &Volume, journal: &Journal) -> (Option<(Tree, Meta)>, Growth) {
        let started = Instant::now();
        let (tree, meta) = match snapshot::load(path) {
            Ok(loaded) => loaded,
            Err(e) => {
                if path.exists() {
                    self.log.line(&format!("{letter}: snapshot unusable ({e})"));
                }
                return (None, Growth::default());
            }
        };
        if meta.volume_serial != volume.serial {
            self.log.line(&format!("{letter}: the snapshot is of another drive"));
            return (None, Growth::default());
        }
        if meta.journal_id != journal.id || meta.next_usn < journal.first_usn || meta.next_usn > journal.next_usn {
            self.log.line(&format!("{letter}: the journal no longer reaches back to the snapshot"));
            return (None, tree.growth);
        }
        self.log.line(&format!("{letter}: loaded the snapshot in {:.0} ms", ms(started)));
        (Some((tree, meta)), Growth::default())
    }

    /// Takes the day's mark for what grew, unless one was taken today already. True if it did.
    fn mark_day(&self, slot: &Slot) -> bool {
        let now = now_ms();
        let mut tree = slot.tree.write().unwrap();
        if tree.growth.last().is_some_and(|m| local_day(m.at) == local_day(now)) {
            return false;
        }
        let total = tree.rec(ROOT).map_or(0, |r| r.size);
        tree.growth.mark(now, total);
        true
    }

    fn save(&self, slot: &Slot, path: &Path, meta: Meta) {
        let started = Instant::now();
        let saved = snapshot::save(path, &slot.tree.read().unwrap(), &meta);
        match saved {
            Ok(()) => self.log.line(&format!("{}: saved the snapshot in {:.0} ms", slot.letter, ms(started))),
            Err(e) => self.log.line(&format!("{}: couldn't save the snapshot: {e}", slot.letter)),
        }
        if let Err(e) = history::save(&path.with_extension("hist"), &slot.history.lock().unwrap()) {
            self.log.line(&format!("{}: couldn't save the history: {e}", slot.letter));
        }
    }

    /// Brings what was renamed, moved and deleted up to where the index stands (`to`), when it's
    /// behind: none kept yet (or by a service from before it), or kept less often than the index.
    /// The journal's records from where it left off (its last 256 MB at most) are read twice: first
    /// for how the folders stood before them, then for what happened.
    fn catch_up_history(&self, volume: &Volume, slot: &Slot, journal: &Journal, to: i64, page: &mut [u8]) {
        let mut history = slot.history.lock().unwrap();
        let kept = if history.journal == journal.id { history.next_usn } else { 0 };
        // Reads start at a record (where history or the journal starts); a journal made far bigger
        // than Windows' 32 MB is told for its last 256 MB.
        let from = kept.max(journal.first_usn);
        let oldest = to - (256 << 20);
        if from < to {
            let started = Instant::now();
            let tree = slot.tree.read().unwrap();
            let mut paths = Paths::new(&tree, slot.letter);
            let mut read = 0;
            for learning in [true, false] {
                let mut cursor = from;
                while cursor < to {
                    let Ok(len) = volume.read_journal(cursor, journal.id, page) else { break };
                    let Some((next, records)) = usn::parse_page(&page[..len]) else { break };
                    let end = records.partition_point(|r| r.usn < to);
                    let start = records[..end].partition_point(|r| r.usn < oldest);
                    if learning {
                        paths.learn(&records[start..end]);
                    } else {
                        history.feed(&records[start..end], &mut paths);
                        read += end - start;
                    }
                    if end < records.len() || records.is_empty() {
                        break;
                    }
                    cursor = next;
                }
            }
            self.log.line(&format!("{}: read {read} journal records for history in {:.0} ms: {} events kept", slot.letter, ms(started), history.len()));
        }
        history.journal = journal.id;
        history.next_usn = history.next_usn.max(to);
    }

    /// Reads the next page of the journal and brings every record it touched up to date. Returns
    /// how many journal records it read: 0 once caught up.
    fn apply(&self, volume: &Volume, slot: &Slot, cursor: &mut i64, journal: u64, since: Option<i64>, page: &mut [u8]) -> Result<usize, Restart> {
        let len = volume.read_journal(*cursor, journal, page).map_err(|e| match e {
            JournalError::Lost => Restart::Rescan("the change journal no longer goes back far enough".into()),
            JournalError::Io(e) => Restart::Retry(format!("can't read the change journal: {e}")),
        })?;
        let Some((next, records)) = usn::parse_page(&page[..len]) else { return Ok(0) };
        if records.is_empty() {
            *cursor = next.max(*cursor);
            return Ok(0);
        }
        // What was renamed, moved or deleted is told before the index takes the changes in, while it
        // still names folders as they were.
        {
            let mut history = slot.history.lock().unwrap();
            if history.journal == journal {
                let fresh = records.partition_point(|r| r.usn < history.next_usn);
                if fresh < records.len() {
                    let tree = slot.tree.read().unwrap();
                    let mut paths = Paths::new(&tree, slot.letter);
                    paths.learn(&records[fresh..]);
                    history.feed(&records[fresh..], &mut paths);
                }
                history.next_usn = history.next_usn.max(next);
            }
        }
        // Read what each record holds now before taking the lock, so searches don't wait on the disk.
        // A record that can't be read right now is left as it is.
        let states = tree::parents_first(usn::touched(&records, since).into_iter().filter_map(|n| volume.record_state(n).ok().map(|s| (n, s))).collect());
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
        let (names, filters) = match query::parse(&request.query) {
            Ok(parsed) => parsed,
            Err(message) => return proto::write_message(out, &Message::Error { message }),
        };
        if names.is_empty() && filters.is_empty() {
            return proto::write_message(out, &Message::Error { message: "Type part of a file name to search for".into() });
        }
        let matcher = NameMatcher::from_terms(names);
        let now = now_ms() * 10_000 + 116_444_736_000_000_000;
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
                let search = Search { matcher: &matcher, filters: &filters, now, scope, hidden: request.hidden, excludes: &excludes };
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

    /// Answers what's right inside a folder: result frames, as for a search, then a summary.
    pub fn list(&self, path: &str, out: &mut dyn Write) -> io::Result<()> {
        let started = Instant::now();
        let listed = (|| -> Result<(Vec<Vec<u8>>, usize), &str> {
            let (letter, rest) = split_drive(path).ok_or("Not a folder on a local drive")?;
            let slot = self.ready_slot(letter).ok_or("This drive isn't indexed")?;
            // Encoded under the lock: names are only valid until the next change.
            let tree = slot.tree.read().unwrap();
            let dir = tree.resolve(rest).ok_or("This folder isn't in the index")?;
            let ids = tree.list(dir);
            let drive = format!("{letter}:\\");
            Ok((ids.par_chunks(BATCH).map(|ids| encode_hits(&tree, ids, &drive)).collect(), ids.len()))
        })();
        match listed {
            Ok((frames, count)) => {
                for frame in frames {
                    proto::write_frame(out, FRAME_HITS, &frame)?;
                }
                proto::write_message(out, &Message::Done { count: count as u64, limited: false, elapsed: ms(started) })
            }
            Err(message) => proto::write_message(out, &Message::Error { message: message.into() }),
        }
    }

    /// Answers a disk-usage map: one frame with the map, one with what its items held at the mark
    /// closest to `since` when that's asked for, then a summary.
    pub fn usage(&self, request: &UsageRequest, out: &mut dyn Write) -> io::Result<()> {
        match self.map(request) {
            Ok((payload, earlier, summary)) => {
                proto::write_frame(out, FRAME_USAGE, &payload)?;
                if let Some(earlier) = earlier {
                    proto::write_frame(out, FRAME_GROWTH, &earlier)?;
                }
                proto::write_message(out, &Message::Usage(summary))
            }
            Err(message) => proto::write_message(out, &Message::Error { message }),
        }
    }

    fn map(&self, request: &UsageRequest) -> Result<MapAnswer, String> {
        let started = Instant::now();
        let (letter, rest) = split_drive(&request.path).ok_or("Not a folder on a local drive")?;
        let slot = self.ready_slot(letter).ok_or("This drive isn't indexed")?;
        // Encoded under the lock: names are only valid until the next change.
        let tree = slot.tree.read().unwrap();
        let dir = tree.resolve(rest).ok_or("This folder isn't in the index")?;
        let depth = request.depth.clamp(1, MAX_USAGE_DEPTH);
        let (items, limited) = tree.usage(dir, depth, (request.max_items as usize).min(MAX_USAGE_ITEMS));
        let mark = request.since.and_then(|since| tree.growth.closest(since));
        let mut payload = Vec::with_capacity(items.len() * 48);
        let mut previous = Vec::with_capacity(if mark.is_some() { items.len() } else { 0 });
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
            if let Some(mark) = mark {
                // Files: whether they were written since (what they held isn't kept).
                previous.push(if item.dir {
                    tree.growth.then(rec, r.seq, mark.epoch, item.size).unwrap_or(NEW)
                } else if node.modified >= mark.at {
                    WRITTEN
                } else {
                    item.size
                });
            }
        }
        let total = items.first().ok_or("Not a folder")?.size;
        let growth = mark.map(|m| GrowthSummary {
            since: m.at,
            oldest: tree.growth.oldest().map_or(m.at, |o| o.at),
            drive_then: m.total,
            drive_now: tree.rec(ROOT).map_or(0, |r| r.size),
        });
        let summary = UsageSummary { path: request.path.clone(), total, items: items.len() as u64, limited, elapsed: ms(started), growth };
        Ok((payload, mark.map(|_| proto::encode_growth(&previous)), summary))
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
    fn maps_what_grew_since_a_mark() {
        use crate::tree::{make_ref, RecordState};
        let dir = |parent: u32, name: &str| RecordState { seq: 1, dir: true, names: vec![(make_ref(parent, if parent == ROOT { 5 } else { 1 }), name.into())], ..Default::default() };
        let file = |size: u64, modified_ms: i64, parent: u32, name: &str| RecordState {
            seq: 1,
            size,
            modified: modified_ms * 10_000 + 116_444_736_000_000_000,
            names: vec![(make_ref(parent, 1), name.into())],
            ..Default::default()
        };
        let mut tree = Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
        tree.set_record(20, &dir(ROOT, "Users"));
        tree.set_record(21, &file(1000, 10, 20, "old.bin"));
        tree.set_record(22, &file(500, 10, 20, "grows.log"));
        tree.growth.mark(1_000, 1500);
        // Since the mark: grows.log is written to, a new folder gets a file, and old.bin stays.
        tree.set_record(22, &file(800, 2_000, 20, "grows.log"));
        tree.set_record(30, &dir(20, "New"));
        tree.set_record(31, &file(4000, 2_000, 30, "big.iso"));
        tree.growth.mark(5_000, 5800);
        let engine = Engine::with_tree('T', tree);

        let ask = |since| engine.map(&UsageRequest { path: r"T:\Users".into(), depth: 3, max_items: 100, since }).unwrap();
        let (payload, earlier, summary) = ask(Some(0));
        let nodes = proto::decode_usage(&payload).unwrap();
        let earlier = proto::decode_growth(&earlier.unwrap()).unwrap();
        let grew: Vec<_> = nodes.iter().zip(&earlier).map(|(n, &e)| (n.name.as_str(), n.size, e)).collect();
        assert_eq!(grew, [("Users", 5800, 1500), ("New", 4000, NEW), ("old.bin", 1000, 1000), ("grows.log", 800, WRITTEN), ("big.iso", 4000, WRITTEN)]);
        assert_eq!(summary.growth, Some(GrowthSummary { since: 1_000, oldest: 1_000, drive_then: 1500, drive_now: 5800 }));
        // The mark closest to what's asked: the second, where everything was as it is.
        let (_, earlier, summary) = ask(Some(4_000));
        assert_eq!(proto::decode_growth(&earlier.unwrap()).unwrap()[..2], [5800, 4000]);
        assert_eq!(summary.growth.map(|g| g.since), Some(5_000));
        // Not asked: nothing about growth.
        let (_, earlier, summary) = ask(None);
        assert!(earlier.is_none() && summary.growth.is_none());
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
