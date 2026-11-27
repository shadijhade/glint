//! The running index: a worker per NTFS drive (read the MFT or load the last snapshot, catch up with
//! the journal, then follow it), and the answers to the app's requests.

use crate::growth::Growth;
use crate::history::{self, History, Paths};
use crate::proto::{
    self, unix_ms, GrowthSummary, HistoryAnswer, HistoryEvent, HistoryRequest, Message, SearchRequest, Sizes,
    Status, UsageNode, UsageRequest, UsageSummary, VolumeStatus, FRAME_GROWTH, FRAME_HITS, FRAME_USAGE, NEW, NO_PARENT,
    WRITTEN,
};
use crate::query::{self, Excludes, Filters, NameMatcher};
use crate::snapshot::{self, Meta};
use crate::tree::{self, Marked, Rank, Search, Tree, NONE, ROOT};
use crate::usn;
use crate::volume::{self, Journal, JournalError, Volume};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;

/// What a search for a client that has gone ends with: the error a write to its pipe gets, which the pipe's
/// server says nothing of.
const ERROR_BROKEN_PIPE: i32 = windows::Win32::Foundation::ERROR_BROKEN_PIPE.0 as i32;

/// How often a journal is checked where it can't be waited on (its watcher couldn't start, or failed).
const POLL: Duration = Duration::from_millis(250);
/// After the journal says it has something new, and between small batches of a trickle of changes, the index
/// lets them gather this long before it reads them: a burst is a few batches, not thousands.
const SETTLE: Duration = Duration::from_millis(5);
/// A batch of fewer records than this is a trickle: the next waits for `SETTLE`.
const SMALL_BATCH: usize = 64;
/// The longest an idle drive sleeps, so a day's change, a replaced journal and the like are noticed even if
/// the journal itself says nothing.
const IDLE_CHECK: Duration = Duration::from_secs(5 * 60);
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
/// Events a history request gets at most.
const MAX_HISTORY: usize = 10_000;

/// What a `Stop` holds: whether it was set, and which drives' watchers have something to say.
#[derive(Default)]
struct StopState {
    stopped: bool,
    /// One bit for each drive letter (`poke_bit`), set by that drive's watcher.
    pokes: u32,
}

/// What `Stop` shares with the threads that wake a sleeper (a drive's journal watcher).
#[derive(Default)]
pub struct StopShared {
    state: Mutex<StopState>,
    wake: Condvar,
}

/// Why `StopShared::wait_poke` returned.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Woken {
    Stopped,
    Poked,
    TimedOut,
}

/// The bit a drive's watcher pokes (0 for what isn't a letter: no watcher then).
fn poke_bit(letter: char) -> u32 {
    let letter = letter.to_ascii_uppercase();
    if letter.is_ascii_uppercase() {
        1 << (letter as u8 - b'A')
    } else {
        0
    }
}

impl StopShared {
    pub fn set(&self) {
        self.state.lock().unwrap().stopped = true;
        self.wake.notify_all();
    }

    pub fn is_set(&self) -> bool {
        self.state.lock().unwrap().stopped
    }

    /// Sleeps up to `time`; true if it was stopped meanwhile.
    pub fn sleep(&self, time: Duration) -> bool {
        let state = self.state.lock().unwrap();
        self.wake.wait_timeout_while(state, time, |s| !s.stopped).unwrap().0.stopped
    }

    pub fn wait(&self) {
        let state = self.state.lock().unwrap();
        drop(self.wake.wait_while(state, |s| !s.stopped).unwrap());
    }

    /// Sets `bit` and wakes whoever waits for it. A poke made before the wait begins is kept for it.
    fn poke(&self, bit: u32) {
        self.state.lock().unwrap().pokes |= bit;
        self.wake.notify_all();
    }

    fn clear_poke(&self, bit: u32) {
        self.state.lock().unwrap().pokes &= !bit;
    }

    /// Sleeps until stopped, until `bit` is poked (which it takes), or until `time` has passed. Another
    /// drive's poke doesn't end it: the wait is only for its own bit.
    fn wait_poke(&self, bit: u32, time: Duration) -> Woken {
        let state = self.state.lock().unwrap();
        let mut state = self.wake.wait_timeout_while(state, time, |s| !s.stopped && s.pokes & bit == 0).unwrap().0;
        if state.stopped {
            Woken::Stopped
        } else if state.pokes & bit != 0 {
            state.pokes &= !bit;
            Woken::Poked
        } else {
            Woken::TimedOut
        }
    }
}

/// A stop signal that sleeping threads wake up for.
#[derive(Default)]
pub struct Stop {
    shared: Arc<StopShared>,
}

impl Stop {
    /// The signal, for a thread of its own to poke a sleeper with.
    fn shared(&self) -> Arc<StopShared> {
        self.shared.clone()
    }
}

impl std::ops::Deref for Stop {
    type Target = StopShared;
    fn deref(&self) -> &StopShared {
        &self.shared
    }
}

/// What a drive's change journal can be waited on with.
trait JournalWait: Send + Sync + 'static {
    /// Blocks until the journal has a record at or after `start` (an error ends the wait too).
    fn wait(&self, start: i64, id: u64) -> io::Result<()>;
    /// Ends a wait in progress on another thread.
    fn cancel(&self);
}

impl JournalWait for volume::Waiter {
    fn wait(&self, start: i64, id: u64) -> io::Result<()> {
        let mut buf = [0u8; 16 << 10];
        volume::Waiter::wait(self, start, id, &mut buf).map(drop)
    }

    fn cancel(&self) {
        volume::Waiter::cancel(self);
    }
}

#[derive(Default)]
struct WatchState {
    /// Where to wait from (the journal's position and id), until the thread takes it up.
    arm: Option<(i64, u64)>,
    quit: bool,
    exited: bool,
    /// The wait ended with an error: the journal, or the drive, has a problem the index's own read will name.
    failed: bool,
}

struct WatchInner {
    source: Box<dyn JournalWait>,
    state: Mutex<WatchState>,
    cv: Condvar,
}

/// What `Watch::wait` ended with.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Idle {
    Stopped,
    /// The journal has something new (or something's wrong with it: reading it will say).
    Changed,
    TimedOut,
    /// The watcher can't go on: poll instead.
    Failed,
}

/// A thread that sits in a blocking read of the drive's change journal and pokes the index's thread when
/// the journal has something new, so a change shows at once and an idle drive costs no wakeups.
struct Watch {
    inner: Arc<WatchInner>,
    stop: Arc<StopShared>,
    bit: u32,
    /// The thread is waiting (or about to) from the position it was given.
    armed: bool,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    fn start(source: Box<dyn JournalWait>, stop: Arc<StopShared>, letter: char) -> Option<Watch> {
        let bit = poke_bit(letter);
        if bit == 0 {
            return None;
        }
        stop.clear_poke(bit);
        let inner = Arc::new(WatchInner { source, state: Mutex::default(), cv: Condvar::new() });
        let (shared, poke) = (inner.clone(), stop.clone());
        let thread = thread::Builder::new()
            .name(format!("journal-watch-{letter}"))
            .spawn(move || {
                loop {
                    let (start, id) = {
                        let mut state = shared.state.lock().unwrap();
                        while state.arm.is_none() && !state.quit {
                            state = shared.cv.wait(state).unwrap();
                        }
                        match state.arm.take() {
                            Some(arm) if !state.quit => arm,
                            _ => break,
                        }
                    };
                    let result = shared.source.wait(start, id);
                    let mut state = shared.state.lock().unwrap();
                    if state.quit {
                        break;
                    }
                    state.failed = result.is_err();
                    drop(state);
                    poke.poke(bit);
                }
                shared.state.lock().unwrap().exited = true;
                shared.cv.notify_all();
            })
            .ok()?;
        Some(Watch { inner, stop, bit, armed: false, thread: Some(thread) })
    }

    /// Waits for the journal to have something past `cursor`, for the stop, or for `time` to pass.
    fn wait(&mut self, cursor: i64, journal: u64, time: Duration) -> Idle {
        if !self.armed {
            self.inner.state.lock().unwrap().arm = Some((cursor, journal));
            self.inner.cv.notify_all();
            self.armed = true;
        }
        match self.stop.wait_poke(self.bit, time) {
            Woken::Stopped => Idle::Stopped,
            Woken::TimedOut => Idle::TimedOut,
            Woken::Poked => {
                self.armed = false;
                if self.inner.state.lock().unwrap().failed {
                    Idle::Failed
                } else {
                    Idle::Changed
                }
            }
        }
    }
}

/// How long stopping a drive's watcher keeps cancelling its read before leaving the thread to end with the
/// process: the service's stop, and so its updates, must never hang on a driver that won't let a read go.
const WATCH_STOP_PATIENCE: Duration = Duration::from_secs(2);

impl Drop for Watch {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.quit = true;
        // A cancel that comes before the thread has begun its read is lost: repeat it until the thread is gone.
        let started = Instant::now();
        while !state.exited && started.elapsed() < WATCH_STOP_PATIENCE {
            drop(state);
            self.inner.source.cancel();
            self.inner.cv.notify_all();
            state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.exited {
                state = self
                    .inner
                    .cv
                    .wait_timeout(state, Duration::from_millis(5))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        let exited = state.exited;
        drop(state);
        if let Some(thread) = self.thread.take() {
            // A thread still in its read is left behind (it ends with the process) rather than joined forever.
            if exited {
                let _ = thread.join();
            }
        }
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The day `ms` (Unix ms) falls on in this PC's time zone, counted from 1601: marks of what grew
/// are taken a day apart, as people count days.
fn local_day(ms: i64) -> i64 {
    let utc = ms * 10_000 + 116_444_736_000_000_000;
    let at = FILETIME {
        dwLowDateTime: utc as u32,
        dwHighDateTime: (utc >> 32) as u32,
    };
    let mut local = FILETIME::default();
    let local = match unsafe { FileTimeToLocalFileTime(&at, &mut local) } {
        Ok(()) => (local.dwHighDateTime as i64) << 32 | local.dwLowDateTime as i64,
        Err(_) => utc,
    };
    local / 864_000_000_000
}

/// Whether the service reads the marks a record doesn't show (`far.rs`). On: the read opens each item by its file
/// reference on the drive (`OpenFileById`), never by a path. This service runs as LocalSystem, and an account that
/// can write a folder could swap part of a path for a link while the service looks, and so send it to open
/// somewhere else, another computer included; a reference walks no path, and a link is not followed
/// (docs/gpui/marks/PLAN.md, section 9, card C4).
const FAR_READS: bool = true;

/// Counts from several drives as one: names alike in any case are one, shown as first met; most used first, then
/// by name.
fn merged(counts: impl Iterator<Item = (String, u64)>) -> Vec<(String, u64)> {
    let mut all: HashMap<String, (String, u64)> = HashMap::new();
    for (name, count) in counts {
        all.entry(name.to_lowercase()).or_insert_with(|| (name, 0)).1 += count;
    }
    let mut out: Vec<(String, u64)> = all.into_values().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    out
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
        Log {
            file: Mutex::new(file),
            echo,
        }
    }

    pub fn line(&self, text: &str) {
        let secs = now_ms() / 1000;
        let (days, rest) = (secs / 86_400, secs % 86_400);
        let line = format!(
            "day {days} {:02}:{:02}:{:02} UTC  {text}\n",
            rest / 3600,
            rest / 60 % 60,
            rest % 60
        );
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
    serial32: AtomicU32,
    tree: RwLock<Tree>,
    ready: AtomicBool,
    status: Mutex<VolumeStatus>,
    /// What was renamed, moved and deleted. Taken before the tree when both are.
    history: Mutex<History>,
    /// Whether the saved history was read (once: it outlives a new read of the disk).
    history_loaded: AtomicBool,
    /// The tree's counts, and when they were worked out.
    counted: Mutex<Option<(Instant, crate::tree::TreeStats)>>,
    /// True while `Engine::save` holds `tree.read()` so `far_round` skips taking `tree.write()`
    /// and does not cause SRWLock writer-starvation against concurrent queries.
    saving: AtomicBool,
}

/// How long the tree's counts are given as they were: counting goes over every record under the tree's lock, and
/// the app asks for the status before searches with filters (and every second or so while a drive is read).
const COUNTED_FOR: Duration = Duration::from_secs(2);

impl Slot {
    fn new(letter: char, tree: Tree, ready: bool, status: VolumeStatus) -> Slot {
        Slot {
            letter,
            serial32: AtomicU32::new(0),
            tree: RwLock::new(tree),
            ready: AtomicBool::new(ready),
            status: Mutex::new(status),
            history: Mutex::default(),
            history_loaded: AtomicBool::new(false),
            counted: Mutex::new(None),
            saving: AtomicBool::new(false),
        }
    }

    /// The tree's counts, worked out again once they're [`COUNTED_FOR`] old.
    fn counts(&self) -> crate::tree::TreeStats {
        let mut counted = self.counted.lock().unwrap();
        if let Some((at, stats)) = *counted {
            if at.elapsed() < COUNTED_FOR {
                return stats;
            }
        }
        let stats = self.tree.read().unwrap().stats();
        *counted = Some((Instant::now(), stats));
        stats
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
        let engine = Arc::new(Engine {
            data,
            only,
            slots: RwLock::default(),
            stop: Stop::default(),
            workers: Mutex::default(),
            log,
        });
        engine
            .log
            .line(&format!("starting {} {}", crate::SERVICE_NAME, crate::VERSION));
        engine.discover();
        // Drives can appear later (an external disk, a volume unlocked by BitLocker).
        let watcher = engine.clone();
        let handle = thread::spawn(move || {
            while !watcher.stop.sleep(Duration::from_secs(60)) {
                watcher.discover();
            }
        });
        engine.workers.lock().unwrap().push(handle);
        // The marks a record doesn't show are read through the file system, on a thread no search waits for. It
        // is not one of `workers`, so `stop` doesn't wait for it either: another program can keep an open of its
        // own file waiting for as long as it likes (an oplock), and the service must still stop.
        if FAR_READS {
            let far = engine.clone();
            thread::spawn(move || {
                while !far.stop.is_set() {
                    if far.far_round() == 0 && far.stop.sleep(Duration::from_secs(1)) {
                        break;
                    }
                }
            });
        }
        engine
    }

    /// One round of reading the far records (`far::drain`) of every ready drive: how many it took. No lock of the
    /// engine's is held while a drive is drained. Run by a thread of its own: see [`FAR_READS`].
    fn far_round(&self) -> usize {
        let slots = self.slots.read().unwrap().clone();
        slots
            .iter()
            .filter(|s| s.ready.load(Ordering::SeqCst) && !s.saving.load(Ordering::Acquire))
            .map(|s| crate::far::drain(&s.tree, s.letter, &|| self.stop.is_set()))
            .sum()
    }

    /// An engine serving one ready drive from `tree`, with no workers: for tests.
    #[cfg(test)]
    pub(crate) fn with_tree(letter: char, tree: Tree) -> Arc<Engine> {
        let status = VolumeStatus {
            volume: format!("{letter}:"),
            state: "ready".into(),
            ..Default::default()
        };
        let slot = Arc::new(Slot::new(letter, tree, true, status));
        let log = Log {
            file: Mutex::new(None),
            echo: false,
        };
        Arc::new(Engine {
            data: PathBuf::new(),
            only: None,
            slots: RwLock::new(vec![slot]),
            stop: Stop::default(),
            workers: Mutex::default(),
            log,
        })
    }

    /// Feeds journal records to the history of the first drive, as following the journal does: for tests.
    #[cfg(test)]
    pub(crate) fn feed_history(&self, records: &[crate::usn::UsnRecord]) {
        let slot = self.slots.read().unwrap()[0].clone();
        let mut history = slot.history.lock().unwrap();
        let tree = slot.tree.read().unwrap();
        let mut paths = Paths::new(&tree, slot.letter);
        paths.learn(records);
        history.feed(records, &mut paths);
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
        let drives = volume::fixed_indexed_drives();
        let mut slots = self.slots.write().unwrap();
        for letter in drives {
            if self.stop.is_set()
                || self.only.as_ref().is_some_and(|only| !only.contains(&letter))
                || slots.iter().any(|s| s.letter == letter)
            {
                continue;
            }
            let status = VolumeStatus {
                volume: format!("{letter}:"),
                state: "scanning".into(),
                ..Default::default()
            };
            let slot = Arc::new(Slot::new(letter, Tree::default(), false, status));
            slots.push(slot.clone());
            let engine = self.clone();
            let handle = thread::Builder::new()
                .name(format!("index {letter}:"))
                .spawn(move || engine.worker(&slot))
                .expect("index worker");
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
                    if last_rescan.is_some_and(|at| at.elapsed() < Duration::from_secs(60))
                        && self.stop.sleep(RETRY_AFTER)
                    {
                        return;
                    }
                    (rescan, last_rescan) = (true, Some(Instant::now()));
                }
                Err(Restart::Retry(why)) => {
                    self.log.line(&format!(
                        "{}: {why}; trying again in {}s",
                        slot.letter,
                        RETRY_AFTER.as_secs()
                    ));
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
        let journal = match volume
            .journal()
            .map_err(|e| retry("can't read the change journal", e))?
        {
            Some(journal) => journal,
            None => {
                volume
                    .create_journal()
                    .map_err(|e| retry("can't turn the change journal on", e))?;
                self.log.line(&format!("{}: turned the change journal on", slot.letter));
                volume
                    .journal()
                    .map_err(|e| retry("can't read the change journal", e))?
                    .ok_or_else(|| Restart::Retry("no change journal".into()))?
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
                *slot.status.lock().unwrap() = VolumeStatus {
                    volume: format!("{}:", slot.letter),
                    state: "scanning".into(),
                    ..Default::default()
                };
                let since = (now_ms() - SCAN_REPLAY.as_millis() as i64) * 10_000 + 116_444_736_000_000_000;
                let progress = |done: u64, total: u64| {
                    let mut status = slot.status.lock().unwrap();
                    (status.progress, status.progress_total) = (done, total);
                    !self.stop.is_set()
                };
                let what = if volume.is_refs() { "the folders" } else { "the MFT" };
                let mut tree = match volume.scan(&progress) {
                    Ok(tree) => tree,
                    Err(_) if self.stop.is_set() => return Ok(()),
                    Err(e) => return Err(retry(&format!("can't read {what}"), e)),
                };
                self.log
                    .line(&format!("{}: read {what} in {:.0} ms", slot.letter, ms(started)));
                // Records may hold other folders now than the notes say: folders compare from now on.
                if growth.tracking() {
                    growth.restart(now_ms(), tree.rec(ROOT).map_or(0, |r| r.size()));
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
                    Ok(Some(now))
                        if now.id == journal.id && now.first_usn > cursor && now.first_usn <= journal.next_usn =>
                    {
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
        slot.serial32.store(volume.serial as u32, Ordering::SeqCst);
        slot.ready.store(true, Ordering::SeqCst);
        let stats = slot.tree.read().unwrap().stats();
        self.log.line(&format!(
            "{}: ready from {source} in {build_ms:.0} ms: {} files, {} folders, {} MB in memory",
            slot.letter,
            stats.files,
            stats.folders,
            stats.memory >> 20
        ));

        let meta = |cursor| Meta {
            volume_serial: volume.serial,
            journal_id: journal.id,
            next_usn: cursor,
            saved_at: now_ms(),
        };
        let marked = self.mark_day(slot);
        if source == "scan" {
            self.save(slot, &volume, &path, meta(cursor));
        }
        // A snapshot the journal had to catch up gets saved again at the next chance.
        let (mut dirty, mut saved_at, mut counted_at) = (
            source == "snapshot" && (replayed > 0 || marked),
            Instant::now(),
            Instant::now(),
        );
        let mut today = local_day(now_ms());
        // A thread that waits in the journal for what's new: none (polling instead) if it can't start.
        let mut watch = volume
            .waiter()
            .ok()
            .and_then(|w| Watch::start(Box::new(w), self.stop.shared(), slot.letter));
        loop {
            if self.stop.is_set() {
                if dirty {
                    self.save(slot, &volume, &path, meta(cursor));
                }
                return Ok(());
            }
            let read = self.apply(&volume, slot, &mut cursor, journal.id, None, &mut page)?;
            if read > 0 {
                dirty = true;
                if read < SMALL_BATCH {
                    self.stop.sleep(SETTLE);
                }
                continue;
            }
            if local_day(now_ms()) != today {
                dirty |= self.mark_day(slot);
                today = local_day(now_ms());
            }
            if dirty && saved_at.elapsed() > SAVE_EVERY {
                self.save(slot, &volume, &path, meta(cursor));
                (dirty, saved_at) = (false, Instant::now());
            }
            if counted_at.elapsed() > RECOUNT_EVERY {
                let mut tree = slot.tree.write().unwrap();
                tree.recompute_totals();
                tree.maybe_compact();
                counted_at = Instant::now();
            }
            let Some(watching) = watch.as_mut() else {
                self.stop.sleep(POLL);
                continue;
            };
            // Asleep until the journal has something, or until something else is due.
            let mut due = IDLE_CHECK.min(RECOUNT_EVERY.saturating_sub(counted_at.elapsed()));
            if dirty {
                due = due.min(SAVE_EVERY.saturating_sub(saved_at.elapsed()));
            }
            match watching.wait(cursor, journal.id, due.max(Duration::from_secs(1))) {
                Idle::Changed => {
                    self.stop.sleep(SETTLE);
                }
                Idle::Failed => watch = None,
                Idle::Stopped | Idle::TimedOut => {}
            }
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
            self.log
                .line(&format!("{letter}: the journal no longer reaches back to the snapshot"));
            return (None, tree.growth);
        }
        // A ReFS snapshot goes with the record numbers its file ids were given.
        if !volume.load_ids(&path.with_extension("ids")) {
            self.log
                .line(&format!("{letter}: the snapshot's record numbers are missing"));
            return (None, tree.growth);
        }
        let mut tree = tree;
        // Marks are kept beside the snapshot (a snapshot from before them has none: they come back
        // as their files change, or at the next read of the disk). 0.16 kept only tags, in `.tags`.
        let marks = path.with_extension("marks");
        (tree.marks, tree.far) = if marks.exists() {
            load_marks(&marks)
        } else {
            let old = load_tags(&path.with_extension("tags"));
            let marks = old.into_iter().map(|(n, tags)| (n, Marked { tags, ..Default::default() })).collect();
            (marks, HashSet::new())
        };
        self.log
            .line(&format!("{letter}: loaded the snapshot in {:.0} ms", ms(started)));
        (Some((tree, meta)), Growth::default())
    }

    /// Takes the day's mark for what grew, unless one was taken today already. True if it did.
    fn mark_day(&self, slot: &Slot) -> bool {
        let now = now_ms();
        let mut tree = slot.tree.write().unwrap();
        if tree.growth.last().is_some_and(|m| local_day(m.at) == local_day(now)) {
            return false;
        }
        let total = tree.rec(ROOT).map_or(0, |r| r.size());
        tree.growth.mark(now, total);
        true
    }

    fn save(&self, slot: &Slot, volume: &Volume, path: &Path, meta: Meta) {
        let started = Instant::now();
        if let Err(e) = volume.save_ids(&path.with_extension("ids")) {
            self.log
                .line(&format!("{}: couldn't save the record numbers: {e}", slot.letter));
        }
        struct ClearSaving<'a>(&'a AtomicBool);
        impl Drop for ClearSaving<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        slot.saving.store(true, Ordering::Release);
        let _saving = ClearSaving(&slot.saving);
        // Serialize marks and snapshot bytes under the tree's read lock, then flush/rename off-lock so journal
        // writes and searches queued behind them don't wait on disk I/O.
        let (marks_bytes, written) = {
            let tree = slot.tree.read().unwrap();
            let marks = encode_marks(&tree.marks, &tree.far);
            let written = snapshot::write(path, &tree, &meta);
            (marks, written)
        };
        drop(_saving);
        if let Err(e) = write_marks(&path.with_extension("marks"), &marks_bytes) {
            self.log.line(&format!("{}: couldn't save the marks: {e}", slot.letter));
        }
        let saved = written.and_then(snapshot::Written::finish);
        match saved {
            Ok(()) => self
                .log
                .line(&format!("{}: saved the snapshot in {:.0} ms", slot.letter, ms(started))),
            Err(e) => self
                .log
                .line(&format!("{}: couldn't save the snapshot: {e}", slot.letter)),
        }
        let hist_bytes = {
            let history = slot.history.lock().unwrap();
            history::encode(&history)
        };
        if let Err(e) = history::write_encoded(&path.with_extension("hist"), &hist_bytes) {
            self.log
                .line(&format!("{}: couldn't save the history: {e}", slot.letter));
        }
    }

    /// Brings what was renamed, moved and deleted up to where the index stands (`to`), when it's
    /// behind: none kept yet (or by a service from before it), or kept less often than the index.
    /// The journal's records from where it left off (its last 256 MB at most) are read twice: first
    /// for how the folders stood before them, then for what happened.
    fn catch_up_history(&self, volume: &Volume, slot: &Slot, journal: &Journal, to: i64, page: &mut [u8]) {
        let mut history = slot.history.lock().unwrap();
        let kept = if history.journal == journal.id {
            history.next_usn
        } else {
            0
        };
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
                    let Ok(len) = volume.read_journal(cursor, journal.id, page) else {
                        break;
                    };
                    let Some((next, records)) = usn::parse_page(&page[..len]) else {
                        break;
                    };
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
            self.log.line(&format!(
                "{}: read {read} journal records for history in {:.0} ms: {} events kept",
                slot.letter,
                ms(started),
                history.len()
            ));
        }
        history.journal = journal.id;
        history.next_usn = history.next_usn.max(to);
    }

    /// Reads the next page of the journal and brings every record it touched up to date. Returns
    /// how many journal records it read: 0 once caught up.
    fn apply(
        &self,
        volume: &Volume,
        slot: &Slot,
        cursor: &mut i64,
        journal: u64,
        since: Option<i64>,
        page: &mut [u8],
    ) -> Result<usize, Restart> {
        let len = volume.read_journal(*cursor, journal, page).map_err(|e| match e {
            JournalError::Lost => Restart::Rescan("the change journal no longer goes back far enough".into()),
            JournalError::Io(e) => Restart::Retry(format!("can't read the change journal: {e}")),
        })?;
        let Some((next, records)) = usn::parse_page(&page[..len]) else {
            return Ok(0);
        };
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
        let states = tree::parents_first(
            usn::touched(&records, since)
                .into_iter()
                .filter_map(|n| volume.record_state(n).ok().map(|s| (n, s)))
                .collect(),
        );
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
        self.slots
            .read()
            .unwrap()
            .iter()
            .find(|s| s.letter == letter && s.ready.load(Ordering::SeqCst))
            .cloned()
    }

    /// Every tag on the ready drives, with how many items have it, most used first.
    pub fn tags(&self) -> Vec<(String, u64)> {
        merged(self.ready_slots().iter().flat_map(|s| s.tree.read().unwrap().tag_counts()))
    }

    fn ready_slots(&self) -> Vec<Arc<Slot>> {
        let slots = self.slots.read().unwrap();
        slots.iter().filter(|s| s.ready.load(Ordering::SeqCst)).cloned().collect()
    }

    /// Every label and tag on the ready drives with how many items have it, most used first, and how many items
    /// have a comment (docs/gpui/marks).
    pub fn marks(&self) -> crate::proto::MarkCounts {
        let slots = self.ready_slots();
        let trees: Vec<_> = slots.iter().map(|s| s.tree.read().unwrap()).collect();
        crate::proto::MarkCounts {
            labels: merged(trees.iter().flat_map(|t| t.label_counts())),
            tags: merged(trees.iter().flat_map(|t| t.tag_counts())),
            comments: trees.iter().map(|t| t.comments()).sum(),
        }
    }

    /// The marked items right inside `path`, never with a comment's text.
    pub fn marks_in(&self, path: &str) -> Result<Vec<crate::proto::MarkedChild>, String> {
        let (letter, rest) = split_drive(path).ok_or("Not a folder on a local drive")?;
        let slot = self.ready_slot(letter).ok_or("This drive isn't indexed")?;
        let guard = slot.tree.read().unwrap();
        let tree: &Tree = &guard;
        let dir = tree.resolve(rest).ok_or("This folder isn't in the index")?;
        tree.rec(dir).filter(|r| r.dir()).ok_or("Not a folder")?;
        // Asked on every folder opened: the marked records (and those still to be read) are a few hundred, the
        // drive's names millions, so it goes through those records' names rather than listing the folder.
        let inside = |rec: u32| {
            tree.links_of(rec).filter(move |&id| {
                let link = tree.link(id);
                link.flags & tree::LINK_USED != 0 && link.parent == dir && link.rec >= tree::FIRST_USER_RECORD
            })
        };
        // An item whose marks are still to be read (far.rs) would answer as unmarked: the app reads the files.
        if tree.far.iter().any(|&rec| inside(rec).next().is_some()) {
            return Err("Some marks here are still being read".into());
        }
        let mut children: Vec<(u32, &Marked)> = tree
            .marks
            .iter()
            .flat_map(|(&rec, marked)| inside(rec).map(move |id| (id, marked)))
            .collect();
        // In the folder's own order (by name record), not the map's.
        children.sort_unstable_by_key(|&(id, _)| id);
        Ok(children
            .into_iter()
            .map(|(id, marked)| crate::proto::MarkedChild {
                name: tree.name(id).to_string(),
                label: marked.label.to_string(),
                tags: marked.tags.lines().map(str::to_string).collect(),
                comment: !marked.comment.is_empty(),
            })
            .collect())
    }

    pub fn status(&self) -> Status {
        let slots = self.slots.read().unwrap().clone();
        let volumes = slots
            .iter()
            .map(|slot| {
                let mut status = slot.status.lock().unwrap().clone();
                if slot.ready.load(Ordering::SeqCst) {
                    let stats = slot.counts();
                    (status.files, status.folders, status.memory) = (stats.files, stats.folders, stats.memory);
                }
                status
            })
            .collect();
        Status {
            version: crate::VERSION.into(),
            volumes,
        }
    }

    /// Answers a name search: a plan, result frames, then a summary. Ended as soon as `gone` says the client has
    /// gone: an app typing fast closes the connection of the search a newer key press replaced, and that scan (all
    /// of every drive's names, then the ranking, under the tree's read lock) would otherwise run to its end, and
    /// its pipe be found closed only at the first write. Ends with a broken-pipe error, as such a write would.
    pub fn search_until(
        &self,
        request: &SearchRequest,
        out: &mut dyn Write,
        gone: &(dyn Fn() -> bool + Sync),
    ) -> io::Result<()> {
        let started = Instant::now();
        let check = || {
            if gone() {
                Err(io::Error::from_raw_os_error(ERROR_BROKEN_PIPE))
            } else {
                Ok(())
            }
        };
        let (names, filters) = match query::parse(&request.query) {
            Ok(parsed) => parsed,
            Err(message) => return proto::write_message(out, &Message::Error { message }),
        };
        if names.is_empty() && filters.is_empty() {
            return proto::write_message(
                out,
                &Message::Error {
                    message: "Type part of a file name to search for".into(),
                },
            );
        }
        let matcher = NameMatcher::from_terms(names.clone());
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
        targets.retain(|(slot, _)| matcher.wants_drive(slot.letter));

        let max = request.max_results.filter(|&m| m > 0).unwrap_or(MAX_HITS).min(MAX_HITS);
        // What a ranked search compares names with: the first word typed (or the last segment of a path), folded, without wildcards.
        let word = names
            .first()
            .map(|w| {
                let seg = w.rsplit(['\\', '/', ':']).find(|s| !s.is_empty()).unwrap_or(w);
                query::fold(seg).replace(['*', '?', '"'], "")
            })
            .unwrap_or_default();
        // Plain words only: a wildcard or a path looks at names alone.
        let words: Option<Vec<String>> = (request.marks
            && !names.is_empty()
            && names.iter().all(|t| !t.contains(['*', '?', '\\', '/', ':'])))
        .then(|| names.iter().map(|t| query::fold(t)).collect());
        let mut send = |matcher: &NameMatcher, filters: &Filters, by_marks: bool| -> io::Result<(usize, bool)> {
            let (mut count, mut limited) = if request.rank {
                send_ranked(
                    &targets,
                    matcher,
                    filters,
                    now,
                    request.hidden,
                    &excludes,
                    word.as_bytes(),
                    max,
                    gone,
                    out,
                )?
            } else {
                let (mut count, mut limited) = (0, false);
                for (slot, scope) in &targets {
                    if count >= max {
                        limited = true;
                        break;
                    }
                    check()?;
                    // Results are encoded under the lock (ids are only valid until the next change), then sent.
                    let (mut frames, found) = {
                        let tree = slot.tree.read().unwrap();
                        let search = Search {
                            matcher,
                            filters,
                            now,
                            scope: *scope,
                            hidden: request.hidden,
                            excludes: &excludes,
                        };
                        collect_hits_framed(&tree, slot.letter, &search, max - count, gone)
                    };
                    check()?;
                    let taken = found.min(max - count);
                    limited |= taken < found;
                    count += taken;
                    for frame in &mut frames {
                        proto::write_frame_with_head(out, FRAME_HITS, frame)?;
                    }
                }
                (count, limited)
            };
            if let Some(words) = words.as_ref().filter(|_| by_marks) {
                for (slot, scope) in &targets {
                    check()?;
                    let (mut frames, found) = {
                        let tree = slot.tree.read().unwrap();
                        let search = Search {
                            matcher,
                            filters,
                            now,
                            scope: *scope,
                            hidden: request.hidden,
                            excludes: &excludes,
                        };
                        let found = tree.search_marks(&search, words, gone);
                        let taken = &found[..found.len().min(max - count)];
                        let drive = format!("{}:\\", slot.letter);
                        let frames: Vec<Vec<u8>> = taken.chunks(BATCH).map(|c| encode_hits_by(&tree, c, &drive, true)).collect();
                        (frames, found.len())
                    };
                    check()?;
                    let taken = found.min(max - count);
                    limited |= taken < found;
                    count += taken;
                    for frame in &mut frames {
                        proto::write_frame_with_head(out, FRAME_HITS, frame)?;
                    }
                }
            }
            Ok((count, limited))
        };
        let (mut count, mut limited) = send(&matcher, &filters, true)?;
        // Nothing as typed: a tag or label that nothing has gives way to the nearest that something has.
        let mut nearer = None;
        if count == 0 && request.fuzzy && !filters.marks.is_empty() {
            let (mut labels, mut tags) = (Vec::new(), Vec::new());
            for (slot, _) in &targets {
                let tree = slot.tree.read().unwrap();
                labels.extend(tree.label_counts().into_iter().map(|(n, _)| n));
                tags.extend(tree.tag_counts().into_iter().map(|(n, _)| n));
            }
            let (labels, tags): (Vec<&str>, Vec<&str>) = (labels.iter().map(String::as_str).collect(), tags.iter().map(String::as_str).collect());
            if let Some(marks) = filters.marks.nearest(&labels, &tags) {
                check()?;
                let swapped = Filters { marks, ..filters.clone() };
                (count, limited) = send(&matcher, &swapped, true)?;
                nearer = Some(swapped);
            }
        }
        let fuzzy_marks = count > 0 && nearer.is_some();
        // Still nothing: close matches of the names, when asked for and a word is long enough to have typos.
        let near = if count == 0 && request.fuzzy {
            NameMatcher::near(names)
        } else {
            None
        };
        if let Some(near) = &near {
            check()?;
            (count, limited) = send(near, nearer.as_ref().unwrap_or(&filters), false)?;
        }
        proto::write_message(
            out,
            &Message::Done {
                count: count as u64,
                limited,
                elapsed: ms(started),
                fuzzy: near.is_some() || fuzzy_marks,
            },
        )
    }

    /// Folder sizes: the folder's total and the total of each folder in it; with `count`, how many files
    /// and folders are below it too (one pass over the drive's names, where the sizes need none).
    pub fn sizes(&self, path: &str, count: bool) -> Result<Sizes, String> {
        let started = Instant::now();
        let (letter, rest) = split_drive(path).ok_or("Not a folder on a local drive")?;
        let slot = self.ready_slot(letter).ok_or("This drive isn't indexed")?;
        let tree = slot.tree.read().unwrap();
        let dir = tree.resolve(rest).ok_or("This folder isn't in the index")?;
        let total = tree.rec(dir).filter(|r| r.dir()).ok_or("Not a folder")?.size();
        let (files, folders) = if count { tree.count_below(dir) } else { (0, 0) };
        let counted = |n| count.then_some(n);
        Ok(Sizes {
            path: path.to_string(),
            total,
            sizes: tree.child_dirs(dir),
            elapsed: ms(started),
            files: counted(files),
            folders: counted(folders),
        })
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
            Ok((
                ids.par_chunks(BATCH)
                    .map(|ids| encode_hits(&tree, ids, &drive, true))
                    .collect(),
                ids.len(),
            ))
        })();
        match listed {
            Ok((mut frames, count)) => {
                for frame in &mut frames {
                    proto::write_frame_with_head(out, FRAME_HITS, frame)?;
                }
                proto::write_message(
                    out,
                    &Message::Done {
                        count: count as u64,
                        limited: false,
                        elapsed: ms(started),
                        fuzzy: false,
                    },
                )
            }
            Err(message) => proto::write_message(
                out,
                &Message::Error {
                    message: message.into(),
                },
            ),
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
                (
                    dir,
                    if dir == ROOT || name == NONE {
                        ""
                    } else {
                        tree.name(name)
                    },
                    NO_PARENT,
                )
            } else {
                (tree.link(item.link).rec, tree.name(item.link), item.parent)
            };
            let Some(r) = tree.rec(rec) else { continue };
            let node = UsageNode {
                parent,
                name: name.to_string(),
                dir: item.dir,
                size: item.size,
                modified: unix_ms(r.modified),
                attrs: r.attrs(),
            };
            proto::encode_usage_node(&mut payload, &node);
            if let Some(mark) = mark {
                // Files: whether they were written since (what they held isn't kept).
                previous.push(if item.dir {
                    tree.growth.then(rec, r.seq(), mark.epoch, item.size).unwrap_or(NEW)
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
            drive_now: tree.rec(ROOT).map_or(0, |r| r.size()),
        });
        let summary = UsageSummary {
            path: request.path.clone(),
            total,
            items: items.len() as u64,
            limited,
            elapsed: ms(started),
            growth,
        };
        Ok((payload, mark.map(|_| proto::encode_growth(&previous)), summary))
    }

    /// What was renamed, moved and deleted on every indexed drive, newest first, with where each
    /// thing that's still there is now.
    pub fn history(&self, request: &HistoryRequest) -> HistoryAnswer {
        let started = Instant::now();
        let max = (request.max as usize).clamp(1, MAX_HISTORY);
        let slots = self.slots.read().unwrap().clone();
        let (mut events, mut limited, mut oldest) = (Vec::new(), false, 0);
        for slot in slots {
            let history = slot.history.lock().unwrap();
            let (found, more) = history.find(&request.query, request.since, max);
            limited |= more;
            if let Some(first) = history.oldest() {
                oldest = if oldest == 0 { first } else { oldest.min(first) };
            }
            let tree = slot.tree.read().unwrap();
            let ready = slot.ready.load(Ordering::SeqCst);
            let mut dirs = HashMap::new();
            for e in found {
                let now = if ready && !e.burst && e.kind != history::Kind::Deleted && e.kind != history::Kind::Emptied {
                    now_at(&tree, e.file, slot.letter, &mut dirs)
                } else {
                    None
                };
                events.push(HistoryEvent {
                    time: e.time,
                    kind: e.kind,
                    dir: e.dir,
                    path: e.path.clone(),
                    to: e.to.clone(),
                    was: e.was.clone(),
                    now,
                    items: e.items,
                    burst: e.burst,
                });
            }
        }
        events.sort_by_key(|e| std::cmp::Reverse(e.time));
        if events.len() > max {
            events.truncate(max);
            limited = true;
        }
        HistoryAnswer {
            events,
            limited,
            oldest,
            elapsed: ms(started),
        }
    }

    /// Resolves `(volume_serial, frn)` across all ready indexed volumes.
    pub fn resolve_id(&self, volume_serial: u32, frn: u64) -> Option<String> {
        let slots = self.slots.read().unwrap().clone();
        for slot in slots {
            if !slot.ready.load(Ordering::SeqCst) {
                continue;
            }
            if slot.serial32.load(Ordering::SeqCst) != volume_serial {
                continue;
            }
            let tree = slot.tree.read().unwrap();
            if let Some(rel) = tree.path_of_frn(frn) {
                return Some(format!("{}:\\{}", slot.letter, rel));
            }
        }
        None
    }
}

/// Where the file `file` (a file reference) is now, if its record still holds it.
fn now_at(tree: &Tree, file: u64, letter: char, dirs: &mut HashMap<u32, String>) -> Option<String> {
    let (number, seq) = tree::split_ref(file);
    let rec = tree
        .rec(u32::try_from(number).ok()?)
        .filter(|r| r.used() && r.seq() == seq && r.first_link != NONE)?;
    Some(format!("{letter}:\\{}", tree.path(rec.first_link, dirs)?))
}

/// Searches one drive and encodes up to `max` results into frames; also returns how many matched.
/// A ranked search's answer: every drive's matches ranked, and the best `max` of them all sent, best first.
/// How many were sent, and whether there were more.
/// Gives up (a broken-pipe error) once `gone` says the client has gone, between drives and before ranking.
#[allow(clippy::too_many_arguments)]
fn send_ranked(
    targets: &[(Arc<Slot>, u32)],
    matcher: &NameMatcher,
    filters: &Filters,
    now: i64,
    hidden: bool,
    excludes: &Excludes,
    word: &[u8],
    max: usize,
    gone: &(dyn Fn() -> bool + Sync),
    out: &mut dyn Write,
) -> io::Result<(usize, bool)> {
    let check = || {
        if gone() {
            Err(io::Error::from_raw_os_error(ERROR_BROKEN_PIPE))
        } else {
            Ok(())
        }
    };
    let mut total = 0;
    let mut drives = Vec::with_capacity(targets.len());
    for (slot, scope) in targets {
        check()?;
        // Ranked and encoded under the lock: ids are only valid until the next change.
        let tree = slot.tree.read().unwrap();
        let search = Search {
            matcher,
            filters,
            now,
            scope: *scope,
            hidden,
            excludes,
        };
        let found = tree.search_until_edits(&search, gone);
        check()?;
        total += found.len();
        let ranked = tree.rank_edits_until(&found, word, max, gone);
        check()?;
        drives.push(encode_ranked(&tree, &ranked, &format!("{}:\\", slot.letter), gone));
    }
    let mut best: Vec<(Rank, usize, std::ops::Range<usize>)> = drives
        .iter()
        .enumerate()
        .flat_map(|(d, (_, hits))| hits.iter().map(move |(rank, at)| (*rank, d, at.clone())))
        .collect();
    best.sort_unstable_by_key(|&(rank, ..)| (rank.score, rank.len));
    best.truncate(max);
    for chunk in best.chunks(BATCH) {
        let mut frame = Vec::with_capacity(5 + chunk.len() * 96);
        frame.extend_from_slice(&[0u8; 5]);
        for (_, d, at) in chunk {
            frame.extend_from_slice(&drives[*d].0[at.clone()]);
        }
        proto::write_frame_with_head(out, FRAME_HITS, &mut frame)?;
    }
    Ok((best.len(), total > best.len()))
}

/// The hits of `ranked`, encoded one after another, with where each one is.
fn encode_ranked(
    tree: &Tree,
    ranked: &[Rank],
    drive: &str,
    gone: &(dyn Fn() -> bool + Sync),
) -> (Vec<u8>, Vec<(Rank, std::ops::Range<usize>)>) {
    let mut dirs: HashMap<u32, String, std::hash::BuildHasherDefault<crate::growth::RecordHasher>> = HashMap::default();
    let mut path_buf = String::with_capacity(256);
    let mut out = Vec::with_capacity(ranked.len() * 96);
    let mut hits = Vec::with_capacity(ranked.len());
    for (i, &rank) in ranked.iter().enumerate() {
        if i & 1023 == 0 && gone() {
            break;
        }
        if !tree.write_path_into(rank.id, drive, &mut dirs, &mut path_buf) {
            continue;
        }
        let Some(rec) = tree.rec(tree.link(rank.id).rec) else {
            continue;
        };
        let start = out.len();
        proto::encode_hit_raw(
            &mut out,
            &path_buf,
            rec.dir(),
            rec.attrs(),
            rec.size(),
            unix_ms(rec.modified),
            0,
        );
        hits.push((rank, start..out.len()));
    }
    (out, hits)
}

pub(crate) fn collect_hits(tree: &Tree, letter: char, search: &Search, max: usize) -> (Vec<Vec<u8>>, usize) {
    collect_hits_until(tree, letter, search, max, &|| false, false)
}

/// [`collect_hits`], with a scan that `gone` can end and a reserved 5-byte frame header at the start of each chunk.
fn collect_hits_framed(
    tree: &Tree,
    letter: char,
    search: &Search,
    max: usize,
    gone: &(dyn Fn() -> bool + Sync),
) -> (Vec<Vec<u8>>, usize) {
    collect_hits_until(tree, letter, search, max, gone, true)
}

/// [`collect_hits`], with a scan that `gone` can end (see `Tree::search_until`); nothing is encoded then.
fn collect_hits_until(
    tree: &Tree,
    letter: char,
    search: &Search,
    max: usize,
    gone: &(dyn Fn() -> bool + Sync),
    with_head: bool,
) -> (Vec<Vec<u8>>, usize) {
    let found = tree.search_until(search, gone);
    if gone() {
        return (Vec::new(), 0);
    }
    let drive = format!("{letter}:\\");
    let frames = found[..found.len().min(max)]
        .par_chunks(BATCH)
        .map(|ids| encode_hits(tree, ids, &drive, with_head))
        .collect();
    (frames, found.len())
}

fn encode_hits(tree: &Tree, ids: &[u32], drive: &str, with_head: bool) -> Vec<u8> {
    let mut dirs: HashMap<u32, String, std::hash::BuildHasherDefault<crate::growth::RecordHasher>> = HashMap::default();
    let mut path_buf = String::with_capacity(256);
    let mut out = Vec::with_capacity(ids.len() * 96 + if with_head { 5 } else { 0 });
    if with_head {
        out.extend_from_slice(&[0u8; 5]);
    }
    for &id in ids {
        if !tree.write_path_into(id, drive, &mut dirs, &mut path_buf) {
            continue;
        }
        let Some(rec) = tree.rec(tree.link(id).rec) else {
            continue;
        };
        proto::encode_hit_raw(
            &mut out,
            &path_buf,
            rec.dir(),
            rec.attrs(),
            rec.size(),
            unix_ms(rec.modified),
            0,
        );
    }
    out
}

/// The hits of `ids`, each with how it was found (0: by its name).
fn encode_hits_by(tree: &Tree, ids: &[(u32, u8)], drive: &str, with_head: bool) -> Vec<u8> {
    let mut dirs: HashMap<u32, String, std::hash::BuildHasherDefault<crate::growth::RecordHasher>> = HashMap::default();
    let mut path_buf = String::with_capacity(256);
    let mut out = Vec::with_capacity(ids.len() * 96 + if with_head { 5 } else { 0 });
    if with_head {
        out.extend_from_slice(&[0u8; 5]);
    }
    for &(id, by) in ids {
        if !tree.write_path_into(id, drive, &mut dirs, &mut path_buf) {
            continue;
        }
        let Some(rec) = tree.rec(tree.link(id).rec) else {
            continue;
        };
        proto::encode_hit_raw(
            &mut out,
            &path_buf,
            rec.dir(),
            rec.attrs(),
            rec.size(),
            unix_ms(rec.modified),
            by,
        );
    }
    out
}

/// The longest label, tags or comment a marks file may hold: 64 KB.
const MARKS_FIELD_MAX: usize = 64 << 10;

/// Marks beside a snapshot (little-endian): "GLINTMRK", version 1, a count, then each record's number, label,
/// tags and comment (each a length and its bytes), then the far records: a count and their numbers.
fn encode_marks(marks: &HashMap<u32, Marked>, far: &HashSet<u32>) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + marks.len() * 28 + far.len() * 4);
    out.extend_from_slice(b"GLINTMRK");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(marks.len() as u64).to_le_bytes());
    for (n, marked) in marks {
        out.extend_from_slice(&n.to_le_bytes());
        for text in [&marked.label, &marked.tags, &marked.comment] {
            // The three kinds of text are far below 64 KB (a label 256 bytes, tags 64 lines, a comment 4 KB).
            let text = &text.as_bytes()[..text.len().min(MARKS_FIELD_MAX)];
            out.extend_from_slice(&(text.len() as u32).to_le_bytes());
            out.extend_from_slice(text);
        }
    }
    out.extend_from_slice(&(far.len() as u64).to_le_bytes());
    for n in far {
        out.extend_from_slice(&n.to_le_bytes());
    }
    out
}

fn write_marks(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("marks.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)?;
    // From now on the old tags file is stale.
    let _ = fs::remove_file(path.with_extension("tags"));
    Ok(())
}

#[cfg(test)]
fn save_marks(path: &Path, marks: &HashMap<u32, Marked>, far: &HashSet<u32>) -> io::Result<()> {
    write_marks(path, &encode_marks(marks, far))
}

/// What `save_marks` wrote; no marks at all for a file that is missing or damaged in any way.
fn load_marks(path: &Path) -> (HashMap<u32, Marked>, HashSet<u32>) {
    fs::read(path).ok().and_then(|bytes| read_marks(&bytes)).unwrap_or_default()
}

fn read_marks(bytes: &[u8]) -> Option<(HashMap<u32, Marked>, HashSet<u32>)> {
    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Option<&'a [u8]> {
            let (head, rest) = (self.0.get(..n)?, self.0.get(n..)?);
            self.0 = rest;
            Some(head)
        }
        fn u32(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn u64(&mut self) -> Option<u64> {
            Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
        }
        fn text(&mut self) -> Option<Box<str>> {
            let len = self.u32()? as usize;
            if len > MARKS_FIELD_MAX {
                return None;
            }
            Some(String::from_utf8_lossy(self.take(len)?).into())
        }
    }
    let mut r = Reader(bytes);
    if r.take(8)? != b"GLINTMRK" || r.u32()? != 1 {
        return None;
    }
    // A record takes at least 16 bytes, a far number 4: a count the file can't hold is a damaged file.
    let count = usize::try_from(r.u64()?).ok().filter(|&c| c <= r.0.len() / 16)?;
    let mut marks = HashMap::with_capacity(count);
    for _ in 0..count {
        let n = r.u32()?;
        let (label, tags, comment) = (r.text()?, r.text()?, r.text()?);
        marks.insert(n, Marked { label, tags, comment });
    }
    let far_count = usize::try_from(r.u64()?).ok().filter(|&c| c <= r.0.len() / 4)?;
    let mut far = HashSet::with_capacity(far_count);
    for _ in 0..far_count {
        far.insert(r.u32()?);
    }
    Some((marks, far))
}

/// The tags 0.16 kept beside a snapshot ("GLINTTAG", a count, then each record's number, length and text):
/// read when there is no marks file yet.
fn load_tags(path: &Path) -> HashMap<u32, Box<str>> {
    let mut tags = HashMap::new();
    let Ok(bytes) = fs::read(path) else { return tags };
    if bytes.get(..8) != Some(b"GLINTTAG") {
        return tags;
    }
    let mut at = 16;
    while at + 8 <= bytes.len() {
        let n = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let Some(text) = bytes.get(at + 8..at + 8 + len) else {
            break;
        };
        tags.insert(n, String::from_utf8_lossy(text).into());
        at += 8 + len;
    }
    tags
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
        let dir = |parent: u32, name: &str| RecordState {
            seq: 1,
            dir: true,
            names: vec![(make_ref(parent, if parent == ROOT { 5 } else { 1 }), name.into())],
            ..Default::default()
        };
        let file = |size: u64, modified_ms: i64, parent: u32, name: &str| RecordState {
            seq: 1,
            size,
            modified: modified_ms * 10_000 + 116_444_736_000_000_000,
            names: vec![(make_ref(parent, 1), name.into())],
            ..Default::default()
        };
        let mut tree = Tree::default();
        tree.set_record(
            ROOT,
            &RecordState {
                seq: 5,
                dir: true,
                ..Default::default()
            },
        );
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

        let ask = |since| {
            engine
                .map(&UsageRequest {
                    path: r"T:\Users".into(),
                    depth: 3,
                    max_items: 100,
                    since,
                })
                .unwrap()
        };
        let (payload, earlier, summary) = ask(Some(0));
        let nodes = proto::decode_usage(&payload).unwrap();
        let earlier = proto::decode_growth(&earlier.unwrap()).unwrap();
        let grew: Vec<_> = nodes
            .iter()
            .zip(&earlier)
            .map(|(n, &e)| (n.name.as_str(), n.size, e))
            .collect();
        assert_eq!(
            grew,
            [
                ("Users", 5800, 1500),
                ("New", 4000, NEW),
                ("old.bin", 1000, 1000),
                ("grows.log", 800, WRITTEN),
                ("big.iso", 4000, WRITTEN)
            ]
        );
        assert_eq!(
            summary.growth,
            Some(GrowthSummary {
                since: 1_000,
                oldest: 1_000,
                drive_then: 1500,
                drive_now: 5800
            })
        );
        // The mark closest to what's asked: the second, where everything was as it is.
        let (_, earlier, summary) = ask(Some(4_000));
        assert_eq!(proto::decode_growth(&earlier.unwrap()).unwrap()[..2], [5800, 4000]);
        assert_eq!(summary.growth.map(|g| g.since), Some(5_000));
        // Not asked: nothing about growth.
        let (_, earlier, summary) = ask(None);
        assert!(earlier.is_none() && summary.growth.is_none());
    }

    fn marks_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-marks-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_marks() -> (HashMap<u32, Marked>, HashSet<u32>) {
        let mut marks = HashMap::new();
        marks.insert(
            40,
            Marked { label: "مهم".into(), tags: "a\nb".into(), comment: "سطر أول\nسطر ثان\nthird".into() },
        );
        marks.insert(41, Marked { tags: "only".into(), ..Default::default() });
        (marks, HashSet::from([41, 77]))
    }

    #[test]
    fn marks_saved_beside_a_snapshot_come_back_the_same_the_far_ones_too() {
        let dir = marks_dir("roundtrip");
        let (marks, far) = sample_marks();
        let file = dir.join("C.marks");
        save_marks(&file, &marks, &far).unwrap();
        assert_eq!(load_marks(&file), (marks, far));
        assert!(!dir.join("C.marks.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_tags_file_as_0_16_wrote_it_is_read_when_there_is_no_marks_file_and_goes_once_marks_are_saved() {
        let dir = marks_dir("old");
        let mut old = Vec::new();
        old.extend_from_slice(b"GLINTTAG");
        old.extend_from_slice(&1u64.to_le_bytes());
        old.extend_from_slice(&7u32.to_le_bytes());
        old.extend_from_slice(&3u32.to_le_bytes());
        old.extend_from_slice(b"a\nb");
        fs::write(dir.join("C.tags"), old).unwrap();
        // What a snapshot's load does: the marks if there are any, else the old tags.
        let old = load_tags(&dir.join("C.tags"));
        assert_eq!(old.get(&7).map(|t| &**t), Some("a\nb"));
        assert!(!dir.join("C.marks").exists());
        let marks: HashMap<u32, Marked> = old.into_iter().map(|(n, tags)| (n, Marked { tags, ..Default::default() })).collect();
        assert_eq!(marks[&7], Marked { tags: "a\nb".into(), ..Default::default() });
        save_marks(&dir.join("C.marks"), &marks, &HashSet::new()).unwrap();
        assert!(!dir.join("C.tags").exists());
        assert_eq!(load_marks(&dir.join("C.marks")).0, marks);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_marks_file_is_no_marks_and_no_panic() {
        let dir = marks_dir("damaged");
        let (marks, far) = sample_marks();
        let file = dir.join("C.marks");
        save_marks(&file, &marks, &far).unwrap();
        let good = fs::read(&file).unwrap();
        for cut in 0..good.len() {
            assert!(read_marks(&good[..cut]).is_none(), "cut at {cut}");
        }
        assert!(read_marks(&good).is_some());
        let mut bad = good.clone();
        bad[..8].copy_from_slice(b"GLINTXXX");
        assert!(read_marks(&bad).is_none());
        let mut bad = good.clone();
        bad[8..12].copy_from_slice(&2u32.to_le_bytes());
        assert!(read_marks(&bad).is_none());
        let mut bad = good.clone();
        bad[12..20].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(read_marks(&bad).is_none());
        // The first record's label length (after its number).
        let mut bad = good.clone();
        bad[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(read_marks(&bad).is_none());
        fs::write(&file, &bad).unwrap();
        assert_eq!(load_marks(&file), Default::default());
        let _ = fs::remove_dir_all(&dir);
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

    /// A journal that lives in memory: `append` adds a record, and a wait ends once there's one at or past
    /// where it started, as the real journal's blocking read does.
    #[derive(Default)]
    struct FakeJournal {
        state: Mutex<(i64, bool, bool)>,
        cv: Condvar,
        waits: std::sync::atomic::AtomicUsize,
    }

    impl FakeJournal {
        fn append(&self) {
            self.state.lock().unwrap().0 += 1;
            self.cv.notify_all();
        }
    }

    struct Fake(Arc<FakeJournal>);

    impl JournalWait for Fake {
        fn wait(&self, start: i64, _id: u64) -> io::Result<()> {
            self.0.waits.fetch_add(1, Ordering::SeqCst);
            let mut state = self.0.state.lock().unwrap();
            loop {
                if state.2 {
                    state.2 = false;
                    return Err(io::Error::other("cancelled"));
                }
                if state.1 {
                    return Err(io::Error::other("journal gone"));
                }
                if state.0 > start {
                    return Ok(());
                }
                state = self.0.cv.wait(state).unwrap();
            }
        }

        fn cancel(&self) {
            self.0.state.lock().unwrap().2 = true;
            self.0.cv.notify_all();
        }
    }

    fn watched(journal: &Arc<FakeJournal>, stop: &Stop) -> Watch {
        Watch::start(Box::new(Fake(journal.clone())), stop.shared(), 'C').unwrap()
    }

    #[test]
    fn an_idle_journal_wakes_nobody_and_a_change_shows_at_once() {
        let (journal, stop) = (Arc::new(FakeJournal::default()), Stop::default());
        let mut watch = watched(&journal, &stop);
        // Idle: the wait returns only when its time is up, with one blocking read behind it and no polling.
        let started = Instant::now();
        assert_eq!(watch.wait(0, 1, Duration::from_millis(1000)), Idle::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(990));
        assert_eq!(journal.waits.load(Ordering::SeqCst), 1, "one blocking read for the whole idle second (was 4)");
        // A change from another thread: the wait ends within a few milliseconds of it.
        let mut worst = Duration::ZERO;
        for cursor in 0..20 {
            let writer = journal.clone();
            let appended = Arc::new(Mutex::new(None));
            let at = appended.clone();
            let handle = thread::spawn(move || {
                thread::sleep(Duration::from_millis(30));
                *at.lock().unwrap() = Some(Instant::now());
                writer.append();
            });
            assert_eq!(watch.wait(cursor, 1, Duration::from_secs(10)), Idle::Changed);
            let seen = Instant::now();
            handle.join().unwrap();
            let latency = seen.duration_since(appended.lock().unwrap().unwrap());
            worst = worst.max(latency);
        }
        println!("journal change to the index's thread awake: worst of 20 = {worst:?} (the poll made it up to 250 ms, 125 ms on average)");
        assert!(worst < Duration::from_millis(50), "{worst:?}");
    }

    #[test]
    fn a_watch_ends_when_stopped_and_when_dropped_mid_wait() {
        let (journal, stop) = (Arc::new(FakeJournal::default()), Stop::default());
        let mut watch = watched(&journal, &stop);
        let shared = stop.shared();
        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            shared.set();
        });
        assert_eq!(watch.wait(0, 1, Duration::from_secs(30)), Idle::Stopped);
        stopper.join().unwrap();
        // Dropped while its thread sits in the blocking read: the read is cancelled and the thread gone.
        let started = Instant::now();
        drop(watch);
        assert!(started.elapsed() < Duration::from_secs(2));
        // Dropped before its thread has begun to wait, and one never armed.
        let (fresh, journal) = (Stop::default(), Arc::new(FakeJournal::default()));
        drop(watched(&journal, &fresh));
        let journal = Arc::new(FakeJournal::default());
        let mut armed = watched(&journal, &fresh);
        assert_eq!(armed.wait(0, 1, Duration::from_millis(20)), Idle::TimedOut);
        drop(armed);
    }

    #[test]
    fn a_failing_wait_is_told_so_the_index_polls_instead() {
        let (journal, stop) = (Arc::new(FakeJournal::default()), Stop::default());
        let mut watch = watched(&journal, &stop);
        journal.state.lock().unwrap().1 = true;
        journal.cv.notify_all();
        assert_eq!(watch.wait(0, 1, Duration::from_secs(10)), Idle::Failed);
    }

    #[test]
    fn another_drives_poke_does_not_end_a_wait() {
        let stop = Stop::default();
        let shared = stop.shared();
        let poker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            shared.poke(poke_bit('D'));
            thread::sleep(Duration::from_millis(20));
            shared.poke(poke_bit('c'));
        });
        let started = Instant::now();
        assert_eq!(stop.wait_poke(poke_bit('C'), Duration::from_secs(10)), Woken::Poked);
        assert!(started.elapsed() >= Duration::from_millis(35), "D's poke didn't count");
        assert_eq!(stop.wait_poke(poke_bit('C'), Duration::from_millis(10)), Woken::TimedOut, "taken once");
        poker.join().unwrap();
        assert_eq!(poke_bit('1'), 0);
    }

    use crate::proto::{Hit, MarkedChild, HIT_BY_COMMENT, HIT_BY_TAG};
    use crate::tree::{make_ref, RecordState};

    /// T:\{docs\{lacp.txt, rates.txt (comment), plan.yaml (tags, label), note.txt}, src\net.cfg (tag), loose.txt}.
    fn marked_engine() -> Arc<Engine> {
        let parent = |n: u32| make_ref(n, if n == ROOT { 5 } else { 1 });
        let entry = |dir: bool, p: u32, name: &str| RecordState {
            seq: 1,
            dir,
            attrs: if dir { 0x10 } else { 0x20 },
            modified: 2,
            names: vec![(parent(p), name.into())],
            ..Default::default()
        };
        let mut tree = Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, names: vec![(make_ref(ROOT, 5), ".".into())], ..Default::default() });
        tree.set_record(20, &entry(true, ROOT, "docs"));
        tree.set_record(21, &entry(false, 20, "lacp.txt"));
        tree.set_record(22, &RecordState { comment: Some("LACP rate fast".into()), ..entry(false, 20, "rates.txt") });
        let plan = RecordState { label: Some("Red".into()), tags: Some("netplan\nWork".into()), ..entry(false, 20, "plan.yaml") };
        tree.set_record(23, &plan);
        tree.set_record(24, &entry(false, 20, "note.txt"));
        tree.set_record(25, &entry(true, ROOT, "src"));
        tree.set_record(26, &RecordState { tags: Some("Netplan".into()), ..entry(false, 25, "net.cfg") });
        tree.set_record(27, &RecordState { tags: Some("work".into()), comment: Some("x".into()), ..entry(false, ROOT, "loose.txt") });
        Engine::with_tree('T', tree)
    }

    /// Runs a search: its hits in the order sent, and what Done said.
    fn ask(engine: &Engine, request: &SearchRequest) -> (Vec<Hit>, (u64, bool, bool)) {
        let mut wire = Vec::new();
        engine.search_until(request, &mut wire, &|| false).unwrap();
        let (mut hits, mut done) = (Vec::new(), (u64::MAX, false, false));
        let mut reader = wire.as_slice();
        while let Some((kind, payload)) = proto::read_frame(&mut reader, proto::MAX_FRAME).unwrap() {
            if kind == FRAME_HITS {
                hits.extend(proto::decode_hits(&payload).unwrap());
            } else if let Ok(Message::Done { count, limited, fuzzy, .. }) = serde_json::from_slice(&payload) {
                done = (count, limited, fuzzy);
            }
        }
        (hits, done)
    }

    fn search(query: &str, marks: bool) -> SearchRequest {
        SearchRequest {
            query: query.into(),
            roots: vec![r"T:\".into()],
            hidden: true,
            exclude: vec![],
            max_results: None,
            fuzzy: false,
            rank: false,
            marks,
        }
    }

    fn by(hits: &[Hit]) -> Vec<(String, u8)> {
        hits.iter().map(|h| (h.path.rsplit('\\').next().unwrap_or("").to_string(), h.by)).collect()
    }

    #[test]
    fn marks_merge_cases_show_the_first_spelling_and_count_comments() {
        let counts = marked_engine().marks();
        let low: Vec<(String, u64)> = counts.tags.iter().map(|(t, c)| (t.to_lowercase(), *c)).collect();
        assert_eq!(low, [("netplan".to_string(), 2), ("work".to_string(), 2)]);
        assert_eq!(counts.labels, [("Red".to_string(), 1)]);
        assert_eq!(counts.comments, 2);
    }

    #[test]
    fn marks_in_lists_the_marked_children_with_no_comment_text() {
        let engine = marked_engine();
        let mut docs = engine.marks_in(r"T:\docs").unwrap();
        docs.sort_by(|a, b| a.name.cmp(&b.name));
        let child = |name: &str, label: &str, tags: &[&str], comment| MarkedChild {
            name: name.into(),
            label: label.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment,
        };
        assert_eq!(docs, [child("plan.yaml", "Red", &["netplan", "Work"], false), child("rates.txt", "", &[], true)]);
        assert_eq!(engine.marks_in(r"T:\src").unwrap(), [child("net.cfg", "", &["Netplan"], false)]);
        assert!(!format!("{docs:?}").contains("LACP"));
        assert_eq!(engine.marks_in(r"T:\").unwrap().len(), 1, "only loose.txt: folders aren't marked");
        assert_eq!(engine.marks_in(r"T:\docs\nope").unwrap_err(), engine.sizes(r"T:\docs\nope", false).unwrap_err());
        assert_eq!(engine.marks_in(r"Q:\docs").unwrap_err(), engine.sizes(r"Q:\docs", false).unwrap_err());
    }

    #[test]
    fn marks_in_a_folder_with_an_item_still_to_be_read_far_is_no_answer() {
        let engine = marked_engine();
        engine.slots.read().unwrap()[0].tree.write().unwrap().far.insert(24);
        assert!(engine.marks_in(r"T:\docs").is_err(), "note.txt's marks aren't known yet");
        assert_eq!(engine.marks_in(r"T:\src").unwrap().len(), 1, "another folder answers");
    }

    #[test]
    fn a_search_with_marks_sends_what_is_named_and_what_is_marked_each_once() {
        let engine = marked_engine();
        let (hits, done) = ask(&engine, &search("lacp", true));
        assert_eq!(by(&hits), [("lacp.txt".to_string(), 0), ("rates.txt".to_string(), HIT_BY_COMMENT)]);
        assert_eq!(done, (2, false, false));
        let (hits, done) = ask(&engine, &search("lacp", false));
        assert_eq!((by(&hits), done), (vec![("lacp.txt".to_string(), 0)], (1, false, false)));
        let mut ranked = search("lacp", true);
        ranked.rank = true;
        let (hits, done) = ask(&engine, &ranked);
        assert_eq!(by(&hits), [("lacp.txt".to_string(), 0), ("rates.txt".to_string(), HIT_BY_COMMENT)]);
        assert_eq!(done.0, 2);
        // A tag finds by tag, and a name and a mark together need every word.
        assert!(ask(&engine, &search("netplan yaml", true)).0.is_empty(), "every word must be in the marks");
        let (hits, _) = ask(&engine, &search("plan", true));
        assert_eq!(by(&hits), [("plan.yaml".to_string(), 0), ("net.cfg".to_string(), HIT_BY_TAG)]);
    }

    #[test]
    fn wildcards_and_paths_look_at_names_only_and_max_results_limits_marks_too() {
        let engine = marked_engine();
        assert_eq!(by(&ask(&engine, &search("net*", true)).0), [("net.cfg".to_string(), 0)]);
        assert_eq!(by(&ask(&engine, &search(r"src\net", true)).0), [("net.cfg".to_string(), 0)]);
        assert!(ask(&engine, &search("netpl*", true)).0.is_empty());
        let mut one = search("lacp", true);
        one.max_results = Some(1);
        let (hits, done) = ask(&engine, &one);
        assert_eq!((hits.len(), done), (1, (1, true, false)));
        one.rank = true;
        let (hits, done) = ask(&engine, &one);
        assert_eq!((hits.len(), done), (1, (1, true, false)));
    }

    #[test]
    fn a_tag_typed_wrong_finds_the_nearest_tag_only_when_fuzzy_and_says_so() {
        let engine = marked_engine();
        let mut request = search("tag:netpaln", false);
        let (hits, done) = ask(&engine, &request);
        assert_eq!((hits.len(), done), (0, (0, false, false)));
        request.fuzzy = true;
        let (hits, done) = ask(&engine, &request);
        assert_eq!((by(&hits).len(), done), (2, (2, false, true)));
        request.query = "tag:netplan".into();
        let (hits, done) = ask(&engine, &request);
        assert_eq!((hits.len(), done), (2, (2, false, false)));
    }

    #[test]
    fn a_far_round_over_a_tree_with_nothing_far_takes_nothing_and_the_thread_ends_when_stopped() {
        let engine = marked_engine();
        let before = engine.marks();
        assert_eq!(engine.far_round(), 0);
        assert_eq!(engine.marks(), before);
        let dir = marks_dir("far-thread");
        let started = Engine::start(dir.clone(), Some(vec![]), false);
        thread::sleep(Duration::from_millis(50));
        let at = Instant::now();
        started.stop();
        assert!(at.elapsed() < Duration::from_secs(5));
        let _ = fs::remove_dir_all(&dir);
    }
}
