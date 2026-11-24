//! The copy queue: copies and moves that wait their turn, one at a time, each showing how far it is, its
//! speed and the file it's on; paused and resumed (a file half-copied waits where it is), cancelled, and
//! checked after copying when asked (each copy read back from the disk and compared with its original,
//! by BLAKE3). A name that's taken asks: replace it, skip it, or keep both (for the rest too). A move
//! within a drive is a rename; across drives, a copy whose originals go only once all of an item is
//! copied (and checked). Glint's own engine (CopyFileEx), for what Windows' copy dialog doesn't do;
//! Windows' own stays for every other copy.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use crate::host::{AppHandle, Event};

/// What went wrong with one file.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub path: String,
    pub error: String,
}

/// An item made where it was copied or moved to: Undo's way back.
#[derive(Clone, Debug, PartialEq)]
pub struct Made {
    pub from: String,
    pub to: String,
}

/// A job, as the app shows it.
#[derive(Clone, Debug, Default)]
pub struct JobView {
    pub id: u64,
    /// "copy" or "move".
    pub kind: String,
    pub sources: Vec<String>,
    pub dest: String,
    pub verify: bool,
    /// "waiting", "planning", "copying", "checking", "paused", "asking", "done", "cancelled" or "failed".
    pub state: String,
    pub files: u64,
    pub files_done: u64,
    pub bytes: u64,
    pub bytes_done: u64,
    /// Files read back and found the same as their originals.
    pub verified: u64,
    pub current: Option<String>,
    /// Bytes a second, lately.
    pub speed: f64,
    /// When it started and finished, in ms since 1970.
    pub started: Option<i64>,
    pub finished: Option<i64>,
    pub errors: Vec<Failure>,
    /// The name that's taken, while it asks what to do.
    pub conflict: Option<String>,
    pub made: Vec<Made>,
}

/// What to do with a name that's taken.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Answer {
    Replace,
    Skip,
    Both,
}

struct Job {
    view: Mutex<JobView>,
    /// Its `made` has been told: the first event that says it finished carries it, and only that one.
    made_told: AtomicBool,
    paused: AtomicBool,
    cancelled: AtomicBool,
    /// The answer for every name taken from now on, once one's said "for the rest too".
    always: Mutex<Option<Answer>>,
    answer: Mutex<Option<Answer>>,
    /// Woken by Resume, Cancel and an answer.
    wake: Condvar,
    woken: Mutex<()>,
}

impl Job {
    fn view(&self) -> MutexGuard<'_, JobView> {
        self.view.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, change: impl FnOnce(&mut JobView)) {
        change(&mut self.view());
        tell(false);
    }

    /// Waits while it's paused (false once it's cancelled).
    fn hold(&self) -> bool {
        // Not paused (nearly always): no lock to take.
        if !self.paused.load(Ordering::SeqCst) {
            return !self.cancelled.load(Ordering::SeqCst);
        }
        let mut woken = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        while self.paused.load(Ordering::SeqCst) && !self.cancelled.load(Ordering::SeqCst) {
            woken = self.wake.wait_timeout(woken, Duration::from_millis(250)).unwrap_or_else(|e| e.into_inner()).0;
        }
        !self.cancelled.load(Ordering::SeqCst)
    }

    fn stop(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct Queue {
    jobs: Vec<Arc<Job>>,
    waiting: VecDeque<u64>,
    next: u64,
    app: Option<AppHandle>,
    worker: bool,
}

static QUEUE: LazyLock<Mutex<Queue>> = LazyLock::new(Default::default);
static TOLD: Mutex<Option<Instant>> = Mutex::new(None);

fn queue() -> MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Tells the app how the jobs are (every 150 ms at most while bytes flow; at once when `now`).
fn tell(now: bool) {
    {
        let mut told = TOLD.lock().unwrap_or_else(|e| e.into_inner());
        if !now && told.is_some_and(|t| t.elapsed() < Duration::from_millis(150)) {
            return;
        }
        *told = Some(Instant::now());
    }
    let (app, views) = {
        let q = queue();
        (q.app.clone(), q.jobs.iter().map(|j| told(j)).collect::<Vec<_>>())
    };
    if let Some(app) = app {
        app.emit(Event::CopyQueue(views));
    }
}

/// A job as an event tells it. What it made (an entry for every item copied or moved so far) is what the app
/// offers Undo for once the job has finished: it goes in the first event that says so and in no other, where it
/// made every event of a big copy longer than the one before.
fn told(job: &Job) -> JobView {
    let view = job.view();
    let finished = matches!(view.state.as_str(), "done" | "cancelled" | "failed");
    let made = if finished && !job.made_told.swap(true, Ordering::SeqCst) {
        view.made.clone()
    } else {
        Vec::new()
    };
    JobView {
        id: view.id,
        kind: view.kind.clone(),
        sources: view.sources.clone(),
        dest: view.dest.clone(),
        verify: view.verify,
        state: view.state.clone(),
        files: view.files,
        files_done: view.files_done,
        bytes: view.bytes,
        bytes_done: view.bytes_done,
        verified: view.verified,
        current: view.current.clone(),
        speed: view.speed,
        started: view.started,
        finished: view.finished,
        errors: view.errors.clone(),
        conflict: view.conflict.clone(),
        made,
    }
}

fn job(id: u64) -> Result<Arc<Job>, String> {
    queue().jobs.iter().find(|j| j.view().id == id).cloned().ok_or_else(|| "That job isn't in the queue any more".to_string())
}

/// Adds a copy or a move of `sources` into `dest`: its id. It starts when the ones before it are done.
/// Off the UI thread: a sleeping network share can take seconds to say it's a folder.
pub async fn queue_add(app: AppHandle, kind: String, sources: Vec<String>, dest: String, verify: bool) -> Result<u64, String> {
    crate::fsx::blocking(move || add(app, kind, sources, dest, verify)).await
}

fn add(app: AppHandle, kind: String, sources: Vec<String>, dest: String, verify: bool) -> Result<u64, String> {
    if kind != "copy" && kind != "move" {
        return Err(format!("The queue doesn't know “{kind}”"));
    }
    if !Path::new(&dest).is_dir() {
        return Err(format!("“{dest}” isn't a folder"));
    }
    for source in &sources {
        let inside = Path::new(&dest).starts_with(source);
        if inside {
            return Err(format!("“{source}” can't go into itself"));
        }
    }
    let start = {
        let mut q = queue();
        q.app = Some(app);
        q.next += 1;
        let id = q.next;
        let view = JobView { id, kind, sources, dest, verify, state: "waiting".into(), ..Default::default() };
        q.jobs.push(Arc::new(Job {
            view: Mutex::new(view),
            made_told: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            always: Mutex::new(None),
            answer: Mutex::new(None),
            wake: Condvar::new(),
            woken: Mutex::new(()),
        }));
        q.waiting.push_back(id);
        let start = !q.worker;
        q.worker = true;
        (start, id)
    };
    if start.0 {
        std::thread::spawn(work);
    }
    tell(true);
    Ok(start.1)
}

pub fn queue_pause(id: u64) -> Result<(), String> {
    let job = job(id)?;
    job.paused.store(true, Ordering::SeqCst);
    job.set(|v| {
        if v.state == "copying" || v.state == "waiting" {
            v.state = "paused".into();
        }
    });
    tell(true);
    Ok(())
}

pub fn queue_resume(id: u64) -> Result<(), String> {
    let job = job(id)?;
    job.paused.store(false, Ordering::SeqCst);
    job.set(|v| {
        if v.state == "paused" {
            v.state = if v.started.is_some() { "copying".into() } else { "waiting".into() };
        }
    });
    job.wake.notify_all();
    tell(true);
    Ok(())
}

pub fn queue_cancel(id: u64) -> Result<(), String> {
    let job = job(id)?;
    job.cancelled.store(true, Ordering::SeqCst);
    job.wake.notify_all();
    let waiting = {
        let mut q = queue();
        let before = q.waiting.len();
        q.waiting.retain(|&w| w != id);
        before != q.waiting.len()
    };
    if waiting {
        job.set(|v| {
            v.state = "cancelled".into();
            v.finished = Some(now_ms());
        });
    }
    tell(true);
    Ok(())
}

/// Answers a job asking about a name that's taken: "replace", "skip" or "both", for the rest too if `all`.
pub fn queue_answer(id: u64, answer: String, all: bool) -> Result<(), String> {
    let job = job(id)?;
    let answer = match answer.as_str() {
        "replace" => Answer::Replace,
        "skip" => Answer::Skip,
        _ => Answer::Both,
    };
    if all {
        *job.always.lock().unwrap_or_else(|e| e.into_inner()) = Some(answer);
    }
    *job.answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(answer);
    job.wake.notify_all();
    Ok(())
}

pub fn queue_jobs() -> Vec<JobView> {
    queue().jobs.iter().map(|j| j.view().clone()).collect()
}

/// Takes the finished jobs off the list.
pub fn queue_clear() {
    queue().jobs.retain(|j| !matches!(j.view().state.as_str(), "done" | "cancelled" | "failed"));
    tell(true);
}

/// The worker: one job at a time, in the order they came, while there are any.
fn work() {
    loop {
        let next = {
            let mut q = queue();
            match q.waiting.pop_front() {
                Some(id) => q.jobs.iter().find(|j| j.view().id == id).cloned(),
                None => {
                    q.worker = false;
                    return;
                }
            }
        };
        if let Some(job) = next {
            run(&job);
            tell(true);
        }
    }
}

/// A file to copy: where from, where to, its size, and the item it belongs to (by index).
struct Step {
    from: PathBuf,
    to: PathBuf,
    size: u64,
    item: usize,
}

/// What's in the items to copy: their files (and the folders to make, each with the one it is a copy of), under
/// `dest`.
fn plan(sources: &[String], dest: &Path, failures: &mut Vec<Failure>) -> (Vec<Step>, Vec<(PathBuf, PathBuf)>) {
    let mut steps = Vec::new();
    let mut folders = Vec::new();
    for (item, source) in sources.iter().enumerate() {
        let from = PathBuf::from(source);
        let Some(name) = from.file_name() else { continue };
        let to = dest.join(name);
        match crate::fsx::stat(source) {
            Some(e) if e.dir => {
                folders.push((from.clone(), to.clone()));
                let mut stack = vec![(from, to)];
                while let Some((dir, into)) = stack.pop() {
                    match crate::fsx::read_dir(&dir.to_string_lossy()) {
                        Ok(entries) => {
                            for e in entries {
                                let (a, b) = (dir.join(&e.name), into.join(&e.name));
                                if e.dir && e.attrs & 0x400 != 0 {
                                    // A link to another folder: what it points to isn't copied here.
                                    failures.push(Failure { path: a.to_string_lossy().into_owned(), error: "A link to another folder: left out".into() });
                                } else if e.dir {
                                    folders.push((a.clone(), b.clone()));
                                    stack.push((a, b));
                                } else {
                                    steps.push(Step { from: a, to: b, size: e.size, item });
                                }
                            }
                        }
                        Err(e) => failures.push(Failure { path: dir.to_string_lossy().into_owned(), error: e.to_string() }),
                    }
                }
            }
            Some(e) => steps.push(Step { from, to, size: e.size, item }),
            None => failures.push(Failure { path: source.clone(), error: "It isn't there any more".into() }),
        }
    }
    (steps, folders)
}

fn run(job: &Job) {
    let (kind, sources, dest, verify) = {
        let v = job.view();
        (v.kind.clone(), v.sources.clone(), PathBuf::from(&v.dest), v.verify)
    };
    if !job.hold() {
        return finish(job, "cancelled");
    }
    job.set(|v| {
        v.state = "planning".into();
        v.started = Some(now_ms());
    });
    // A move within a drive: each item renamed into place, at once.
    let mut sources = sources;
    if kind == "move" {
        let mut left = Vec::new();
        for source in sources {
            let Some(name) = Path::new(&source).file_name() else { continue };
            let to = dest.join(name);
            if to.exists() {
                left.push(source);
                continue;
            }
            match rename(Path::new(&source), &to) {
                Ok(()) => {
                    crate::marks::carry::carry_one(&source, &to.to_string_lossy(), true);
                    job.set(|v| v.made.push(Made { from: source.clone(), to: to.to_string_lossy().into_owned() }))
                }
                Err(_) => left.push(source),
            }
        }
        sources = left;
    }
    let mut failures = Vec::new();
    let (steps, folders) = plan(&sources, &dest, &mut failures);
    let total: u64 = steps.iter().map(|s| s.size).sum();
    job.set(|v| {
        v.files = steps.len() as u64;
        v.bytes = total;
        v.errors.append(&mut failures);
        v.state = "copying".into();
    });
    if let Some(free) = crate::archive::extract::free_space(&dest) {
        // Where Windows clones blocks (ReFS, a Dev Drive) a copy within the volume takes no room: it isn't
        // refused for the room it would take, and what truly can't be made fails file by file.
        if free < total && !clones_blocks(&sources, &dest) {
            job.set(|v| v.errors.push(Failure { path: dest.to_string_lossy().into_owned(), error: format!("Not enough space there: it needs {}, and has {}", size_text(total), size_text(free)) }));
            return finish(job, "failed");
        }
    }
    // What each item made there: a folder that wasn't there is Undo's to take back whole; a file, or
    // what went into a folder that was there already, is taken back file by file (never what was there).
    let fresh: Vec<bool> = sources.iter().map(|s| !item_root(&dest, s).exists()).collect();
    let created: Vec<Vec<(PathBuf, PathBuf)>> = vec![Vec::new(); sources.len()];
    // The originals copied (and checked) whole, with their copies: a move's to delete.
    let copied_from: Vec<Vec<(PathBuf, PathBuf)>> = vec![Vec::new(); sources.len()];
    let copies = Copies::default();
    for (_, folder) in &folders {
        let _ = std::fs::create_dir_all(folder);
    }
    let done = Arc::new(AtomicU64::new(0));
    // Names that are taken are asked about first, in order (Replace, Skip, Keep both; for the rest too),
    // so the copying after can go several files at a time.
    let mut todo: Vec<(usize, PathBuf)> = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        if !job.hold() {
            return finish(job, "cancelled");
        }
        let mut to = step.to.clone();
        if to.exists() {
            match ask(job, &to) {
                None => return finish(job, "cancelled"),
                Some(Answer::Skip) => {
                    done.fetch_add(step.size, Ordering::Relaxed);
                    job.set(|v| {
                        v.files_done += 1;
                        v.bytes_done = done.load(Ordering::Relaxed);
                    });
                    continue;
                }
                Some(Answer::Both) => to = free_name(&to),
                Some(Answer::Replace) => clear_read_only(&to),
            }
        }
        todo.push((index, to));
    }
    // Small files go several at a time (where Windows' own copy goes one by one, a folder of thousands of
    // small files is mostly waiting on each file's opening and closing); big ones one at a time after, at
    // the drive's full speed, with progress inside each.
    type Todo = Vec<(usize, PathBuf)>;
    let (small, large): (Todo, Todo) = todo.into_iter().partition(|(i, _)| steps[*i].size < SMALL_FILE);
    let stopped = AtomicBool::new(false);
    let started = Instant::now();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let workers = if small.len() >= 16 { std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, SMALL_WORKERS) } else { 1 };
    #[cfg(test)]
    let workers = match FORCE_WORKERS.load(Ordering::Relaxed) {
        0 => workers,
        n => n,
    };
    // The workers count what they do with atomics and keep what they made in a list of their own; the job's
    // view is brought up to date every `TELL_EVERY` by whichever worker comes by then, not by every file.
    let (small_done, small_checked) = (AtomicU64::new(0), AtomicU64::new(0));
    let reported = Mutex::new((0u64, 0u64));
    let last_tell = AtomicU64::new(0);
    let tell_view = |current: Option<&Path>| {
        let mut reported = reported.lock().unwrap_or_else(|e| e.into_inner());
        let (files, checked) = (small_done.load(Ordering::Relaxed), small_checked.load(Ordering::Relaxed));
        let (new_files, new_checked) = (files - reported.0, checked - reported.1);
        *reported = (files, checked);
        let bytes = done.load(Ordering::Relaxed);
        let speed = bytes as f64 / started.elapsed().as_secs_f64().max(0.001);
        job.set(|v| {
            v.files_done += new_files;
            v.verified += new_checked;
            v.bytes_done = bytes;
            v.speed = speed;
            if let Some(current) = current {
                v.current = Some(current.to_string_lossy().into_owned());
            }
        });
    };
    // Per worker: which small files it copied (their place in `small`), and whether each is good to take the
    // original of (copied, and checked when asked).
    let made: Vec<Vec<(usize, bool)>> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            handles.push(scope.spawn(|| {
                let mut mine: Vec<(usize, bool)> = Vec::new();
                loop {
                    let at = next.fetch_add(1, Ordering::Relaxed);
                    let Some((index, to)) = small.get(at) else { break };
                    if stopped.load(Ordering::SeqCst) || !job.hold() {
                        stopped.store(true, Ordering::SeqCst);
                        break;
                    }
                    let step = &steps[*index];
                    match copy_plain(&step.from, to) {
                        Ok(()) => {
                            copies.add(&step.from, to);
                            let good = !verify || match same(&step.from, to, job) {
                                Ok(true) => {
                                    small_checked.fetch_add(1, Ordering::Relaxed);
                                    true
                                }
                                Ok(false) => {
                                    job.set(|v| v.errors.push(Failure { path: to.to_string_lossy().into_owned(), error: "It differs from its original after copying: the copy is kept, to look at".into() }));
                                    false
                                }
                                Err(e) if e == "cancelled" => {
                                    stopped.store(true, Ordering::SeqCst);
                                    break;
                                }
                                Err(e) => {
                                    job.set(|v| v.errors.push(Failure { path: to.to_string_lossy().into_owned(), error: format!("Couldn't check it: {e}") }));
                                    false
                                }
                            };
                            mine.push((at, good));
                        }
                        Err(e) => job.set(|v| v.errors.push(Failure { path: step.from.to_string_lossy().into_owned(), error: e })),
                    }
                    done.fetch_add(step.size, Ordering::Relaxed);
                    small_done.fetch_add(1, Ordering::Relaxed);
                    let at_ms = started.elapsed().as_millis() as u64;
                    let last = last_tell.load(Ordering::Relaxed);
                    if at_ms >= last + TELL_EVERY_MS && last_tell.compare_exchange(last, at_ms, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                        tell_view(Some(&step.from));
                    }
                }
                mine
            }));
        }
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    tell_view(None);
    if stopped.load(Ordering::SeqCst) {
        return finish(job, "cancelled");
    }
    let (mut created, mut copied_from) = (created, copied_from);
    let mut copied: Vec<(usize, bool)> = made.into_iter().flatten().collect();
    copied.sort_unstable_by_key(|(at, _)| *at);
    for (at, good) in copied {
        let (index, to) = &small[at];
        let step = &steps[*index];
        created[step.item].push((step.from.clone(), to.clone()));
        if good {
            copied_from[step.item].push((step.from.clone(), to.clone()));
        }
    }
    let mut window = (Instant::now(), done.load(Ordering::Relaxed));
    for (index, to) in &large {
        let step = &steps[*index];
        let to = to.clone();
        if !job.hold() {
            return finish(job, "cancelled");
        }
        job.set(|v| v.current = Some(step.from.to_string_lossy().into_owned()));
        let base = done.load(Ordering::Relaxed);
        let copied = copy_file(&step.from, &to, job, &done, base);
        match copied {
            Ok(()) => {
                copies.add(&step.from, &to);
                created[step.item].push((step.from.clone(), to.clone()));
                let good = if verify {
                    job.set(|v| v.state = "checking".into());
                    let checked = same(&step.from, &to, job);
                    job.set(|v| {
                        if v.state == "checking" {
                            v.state = "copying".into();
                        }
                    });
                    match checked {
                        Ok(true) => {
                            job.set(|v| v.verified += 1);
                            true
                        }
                        Ok(false) => {
                            job.set(|v| v.errors.push(Failure { path: to.to_string_lossy().into_owned(), error: "It differs from its original after copying: the copy is kept, to look at".into() }));
                            false
                        }
                        Err(e) if e == "cancelled" => return finish(job, "cancelled"),
                        Err(e) => {
                            job.set(|v| v.errors.push(Failure { path: to.to_string_lossy().into_owned(), error: format!("Couldn't check it: {e}") }));
                            false
                        }
                    }
                } else {
                    true
                };
                if good {
                    copied_from[step.item].push((step.from.clone(), to.clone()));
                }
                done.store(base + step.size, Ordering::Relaxed);
            }
            Err(e) if e == "cancelled" => return finish(job, "cancelled"),
            Err(e) => {
                done.store(base + step.size, Ordering::Relaxed);
                job.set(|v| v.errors.push(Failure { path: step.from.to_string_lossy().into_owned(), error: e }));
            }
        }
        // Speed over the last few seconds.
        let now = done.load(Ordering::Relaxed);
        let elapsed = window.0.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            let speed = (now - window.1) as f64 / elapsed;
            window = (Instant::now(), now);
            job.set(|v| v.speed = speed);
        }
        job.set(|v| {
            v.files_done += 1;
            v.bytes_done = now;
        });
    }
    // A folder's own marks: no copy takes a folder's streams, so they are read and written (marks::carry), before
    // a move takes the originals away.
    let made_folders: Vec<(String, String)> = folders.iter().map(|(from, to)| (from.to_string_lossy().into_owned(), to.to_string_lossy().into_owned())).collect();
    crate::marks::carry::carry(&made_folders, false);
    // Undo's list: a new folder whole, else each file made (with where it came from).
    for (i, source) in sources.iter().enumerate() {
        let root = item_root(&dest, source);
        let made: Vec<Made> = if fresh[i] && Path::new(source).is_dir() && root.exists() {
            vec![Made { from: source.clone(), to: root.to_string_lossy().into_owned() }]
        } else {
            created[i].iter().map(|(from, to)| Made { from: from.to_string_lossy().into_owned(), to: to.to_string_lossy().into_owned() }).collect()
        };
        job.set(|v| v.made.extend(made));
        // A move: the originals that were copied (and checked) go, file by file, then the folders left
        // empty; what was skipped or failed stays where it was.
        if kind == "move" {
            // The copies have their marks before an original goes; then the line an original had beside it or on
            // this PC goes with it. Never sooner: a move that is stopped, or a copy that fails its check, leaves
            // the original as it was, marks and all.
            copies.carry();
            let mut gone = Vec::new();
            for (from, to) in &copied_from[i] {
                match std::fs::remove_file(from) {
                    Ok(()) => gone.push((from.to_string_lossy().into_owned(), to.to_string_lossy().into_owned())),
                    Err(e) => job.set(|v| v.errors.push(Failure { path: from.to_string_lossy().into_owned(), error: format!("Copied, but the original couldn't go: {e}") })),
                }
            }
            crate::marks::carry::carry_files(&gone, true);
            if Path::new(source).is_dir() {
                remove_empty(Path::new(source));
            }
        }
    }
    // Before the job says it is done, so what shows the copies next shows their marks.
    copies.carry();
    let state = if job.view().errors.is_empty() || job.view().files_done > 0 { "done" } else { "failed" };
    finish(job, state);
}

/// The copies a job made, whose marks follow them where they are kept beside the files or on this PC and not in
/// them (`marks::carry`): some hundreds at a time, and when the job ends however it ends. One file at a time
/// would write a folder's `.glint-marks` again for every file in it.
#[derive(Default)]
struct Copies(Mutex<Vec<(String, String)>>);

impl Copies {
    const AT_ONCE: usize = 512;

    fn add(&self, from: &Path, to: &Path) {
        let full = {
            let mut pairs = self.0.lock().unwrap_or_else(|e| e.into_inner());
            pairs.push((from.to_string_lossy().into_owned(), to.to_string_lossy().into_owned()));
            (pairs.len() >= Self::AT_ONCE).then(|| std::mem::take(&mut *pairs))
        };
        if let Some(pairs) = full {
            crate::marks::carry::carry_files(&pairs, false);
        }
    }

    fn carry(&self) {
        let pairs = std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()));
        crate::marks::carry::carry_files(&pairs, false);
    }
}

impl Drop for Copies {
    fn drop(&mut self) {
        self.carry();
    }
}

/// Whether every source is on `dest`'s volume and that volume clones blocks (ReFS, a Dev Drive). CopyFileEx
/// clones inside Windows itself there on current Windows 11 (the queue passes no flag but COPY_FILE_COPY_SYMLINK,
/// so nothing turns that off), and a clone shares the original's space.
fn clones_blocks(sources: &[String], dest: &Path) -> bool {
    !sources.is_empty()
        && sources
            .iter()
            .all(|s| reflink_copy::check_reflink_support(s, dest).is_ok_and(|support| support == reflink_copy::ReflinkSupport::Supported))
}

/// "1.5 GB", "320 KB": a size in words.
fn size_text(bytes: u64) -> String {
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

fn finish(job: &Job, state: &str) {
    job.set(|v| {
        v.state = state.into();
        v.current = None;
        v.conflict = None;
        v.finished = Some(now_ms());
    });
    tell(true);
}

/// Removes a folder's empty folders, from the bottom up, and the folder itself if it ends up empty.
fn remove_empty(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty(&entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// Where an item goes in `dest`: its own name.
fn item_root(dest: &Path, source: &str) -> PathBuf {
    dest.join(Path::new(source).file_name().unwrap_or_default())
}

/// "name (2).ext" and on: the first that isn't taken.
pub(crate) fn free_name(path: &Path) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return path.to_path_buf() };
    let name = name.to_string_lossy();
    (2..10_000).map(|n| parent.join(crate::fsx::numbered(&name, false, n))).find(|p| !p.exists()).unwrap_or_else(|| path.to_path_buf())
}

pub(crate) fn clear_read_only(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut permissions = meta.permissions();
        if permissions.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(path, permissions);
        }
    }
}

/// What to do with a name taken: the answer for the rest, or the one asked for now (None: cancelled).
fn ask(job: &Job, path: &Path) -> Option<Answer> {
    if let Some(always) = *job.always.lock().unwrap_or_else(|e| e.into_inner()) {
        return Some(always);
    }
    *job.answer.lock().unwrap_or_else(|e| e.into_inner()) = None;
    job.set(|v| {
        v.state = "asking".into();
        v.conflict = Some(path.to_string_lossy().into_owned());
    });
    tell(true);
    let mut woken = job.woken.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if job.stop() {
            return None;
        }
        if let Some(answer) = *job.answer.lock().unwrap_or_else(|e| e.into_inner()) {
            drop(woken);
            job.set(|v| {
                v.state = "copying".into();
                v.conflict = None;
            });
            return Some(answer);
        }
        woken = job.wake.wait_timeout(woken, Duration::from_millis(250)).unwrap_or_else(|e| e.into_inner()).0;
    }
}

#[cfg(windows)]
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVE_FILE_FLAGS};
    let (a, b) = (crate::fsx::wide(from), crate::fsx::wide(to));
    // Not MOVEFILE_COPY_ALLOWED: across drives it fails, and the queue copies instead.
    unsafe { MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), MOVE_FILE_FLAGS(0)) }.map_err(crate::fsx::io_error)
}

#[cfg(not(windows))]
fn rename(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// Files under this size are copied several at a time.
const SMALL_FILE: u64 = 1 << 20;
/// How often (ms) the small files' workers bring the job's view and the app up to date.
const TELL_EVERY_MS: u64 = 75;
/// The most small files copied at once.
const SMALL_WORKERS: usize = 8;
/// Tests: how many workers the small files get (0: as usual).
#[cfg(test)]
static FORCE_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Copies one small file with Windows' CopyFileEx, without progress (its streams, attributes and dates
/// go too; a link is copied as the link).
#[cfg(windows)]
fn copy_plain(from: &Path, to: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{CopyFileExW, COPY_FILE_COPY_SYMLINK};
    if let Some(parent) = to.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (a, b) = (crate::fsx::wide(from), crate::fsx::wide(to));
    unsafe { CopyFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), None, None, None, COPY_FILE_COPY_SYMLINK) }.map_err(|e| crate::fsx::file_error(crate::fsx::io_error(e)))
}

#[cfg(not(windows))]
fn copy_plain(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::copy(from, to).map(drop).map_err(|e| e.to_string())
}

/// What a copy's progress routine is given: the job, and the bytes done before this file.
struct Progress<'a> {
    job: &'a Job,
    done: &'a AtomicU64,
    base: u64,
}

#[cfg(windows)]
unsafe extern "system" fn progress(
    _total: i64,
    transferred: i64,
    _stream_size: i64,
    _stream_transferred: i64,
    _stream: u32,
    _reason: windows::Win32::Storage::FileSystem::LPPROGRESS_ROUTINE_CALLBACK_REASON,
    _source: windows::Win32::Foundation::HANDLE,
    _dest: windows::Win32::Foundation::HANDLE,
    data: *const core::ffi::c_void,
) -> windows::Win32::Storage::FileSystem::COPYPROGRESSROUTINE_PROGRESS {
    use windows::Win32::Storage::FileSystem::{PROGRESS_CANCEL, PROGRESS_CONTINUE};
    let p = &*(data as *const Progress);
    p.done.store(p.base + transferred.max(0) as u64, Ordering::Relaxed);
    p.job.set(|v| v.bytes_done = p.done.load(Ordering::Relaxed));
    // Paused: the copy waits here, its files open, until it's resumed.
    if p.job.paused.load(Ordering::SeqCst) {
        p.job.set(|v| v.state = "paused".into());
        if p.job.hold() {
            p.job.set(|v| v.state = "copying".into());
        }
    }
    if p.job.stop() {
        PROGRESS_CANCEL
    } else {
        PROGRESS_CONTINUE
    }
}

/// Copies one file with Windows' CopyFileEx (its streams, attributes and dates go too; a link is copied
/// as the link). "cancelled" when the job is.
#[cfg(windows)]
fn copy_file(from: &Path, to: &Path, job: &Job, done: &AtomicU64, base: u64) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{CopyFileExW, COPY_FILE_COPY_SYMLINK};
    if let Some(parent) = to.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let (a, b) = (crate::fsx::wide(from), crate::fsx::wide(to));
    let context = Progress { job, done, base };
    let result = unsafe { CopyFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), Some(progress), Some(&context as *const _ as *const _), None, COPY_FILE_COPY_SYMLINK) };
    match result {
        Ok(()) => Ok(()),
        Err(_) if job.stop() => Err("cancelled".into()),
        Err(e) => Err(crate::fsx::file_error(crate::fsx::io_error(e))),
    }
}

#[cfg(not(windows))]
fn copy_file(from: &Path, to: &Path, job: &Job, done: &AtomicU64, base: u64) -> Result<(), String> {
    if job.stop() {
        return Err("cancelled".into());
    }
    let n = std::fs::copy(from, to).map_err(|e| e.to_string())?;
    done.store(base + n, Ordering::Relaxed);
    Ok(())
}

/// Whether a copy reads the same as its original: the copy read from the disk itself (past Windows'
/// cache, which would answer with what was written), both by BLAKE3.
fn same(original: &Path, copy: &Path, job: &Job) -> Result<bool, String> {
    let a = hash(original, false, job)?;
    let b = hash(copy, true, job)?;
    Ok(a == b)
}

fn hash(path: &Path, from_disk: bool, job: &Job) -> Result<blake3::Hash, String> {
    use std::io::Read;
    const CHUNK: usize = 1 << 20;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // FILE_FLAG_NO_BUFFERING reads whole sectors into a buffer aligned to them: a page-aligned one, in
    // a multiple of 4 KB.
    #[cfg(windows)]
    if from_disk {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x2000_0000);
    }
    #[cfg(not(windows))]
    let _ = from_disk;
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    let layout = std::alloc::Layout::from_size_align(CHUNK, 4096).map_err(|e| e.to_string())?;
    // SAFETY: a fresh allocation of `layout`, freed below, used only as a byte buffer.
    let raw = unsafe { std::alloc::alloc(layout) };
    if raw.is_null() {
        return Err("Out of memory".into());
    }
    let buffer = unsafe { std::slice::from_raw_parts_mut(raw, CHUNK) };
    let mut hasher = blake3::Hasher::new();
    let result = loop {
        if job.stop() {
            break Err("cancelled".to_string());
        }
        match file.read(buffer) {
            Ok(0) => break Ok(hasher.finalize()),
            Ok(n) => {
                hasher.update(&buffer[..n]);
            }
            Err(e) => break Err(e.to_string()),
        }
    };
    unsafe { std::alloc::dealloc(raw, layout) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Job {
        Job {
            view: Mutex::new(JobView::default()),
            made_told: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            always: Mutex::new(None),
            answer: Mutex::new(None),
            wake: Condvar::new(),
            woken: Mutex::new(()),
        }
    }

    #[test]
    fn what_a_job_made_is_told_once_when_it_has_finished() {
        let job = quiet();
        job.set(|view| {
            view.state = "copying".into();
            view.made = vec![Made { from: "C:\\a".into(), to: "D:\\a".into() }];
        });
        assert!(told(&job).made.is_empty(), "not while it copies");
        job.set(|view| view.state = "done".into());
        assert_eq!(told(&job).made.len(), 1, "the first word that it's done");
        assert!(told(&job).made.is_empty(), "and never again");
        assert_eq!(told(&job).state, "done");
    }

    #[test]
    fn plans_copies_and_checks_them() {
        let dir = std::env::temp_dir().join(format!("glint-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("from").join("sub")).unwrap();
        std::fs::create_dir_all(dir.join("to")).unwrap();
        std::fs::write(dir.join("from").join("a.txt"), "alpha").unwrap();
        std::fs::write(dir.join("from").join("sub").join("b.bin"), vec![7u8; 300_000]).unwrap();
        std::fs::write(dir.join("one.txt"), "one").unwrap();
        let mut failures = Vec::new();
        let sources = [dir.join("from").to_string_lossy().into_owned(), dir.join("one.txt").to_string_lossy().into_owned()];
        let (steps, folders) = plan(&sources, &dir.join("to"), &mut failures);
        assert!(failures.is_empty());
        assert_eq!(steps.len(), 3);
        assert_eq!(steps.iter().map(|s| s.size).sum::<u64>(), 5 + 300_000 + 3);
        assert!(folders.contains(&(dir.join("from").join("sub"), dir.join("to").join("from").join("sub"))));
        let job = quiet();
        let done = AtomicU64::new(0);
        for (_, folder) in &folders {
            std::fs::create_dir_all(folder).unwrap();
        }
        for step in &steps {
            copy_file(&step.from, &step.to, &job, &done, 0).unwrap();
            assert!(same(&step.from, &step.to, &job).unwrap(), "{} reads the same", step.to.display());
        }
        std::fs::write(dir.join("to").join("one.txt"), "changed").unwrap();
        assert!(!same(&dir.join("one.txt"), &dir.join("to").join("one.txt"), &job).unwrap());
        assert_eq!(free_name(&dir.join("to").join("one.txt")), dir.join("to").join("one (2).txt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Copies `count` small files through the queue's own `run` with `workers` workers: the seconds it took,
    /// and the job it left.
    fn timed_copy(count: usize, workers: usize, verify: bool) -> (f64, JobView) {
        let dir = std::env::temp_dir().join(format!("glint-queue-bench-{}-{count}-{workers}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (from, to) = (dir.join("from"), dir.join("to"));
        std::fs::create_dir_all(&to).unwrap();
        for d in 0..(count / 500).max(1) {
            std::fs::create_dir_all(from.join(format!("d{d}"))).unwrap();
        }
        let data = vec![5u8; 4096];
        for i in 0..count {
            std::fs::write(from.join(format!("d{}", i % (count / 500).max(1))).join(format!("f{i}.bin")), &data).unwrap();
        }
        let job = quiet();
        job.set(|v| {
            v.kind = "copy".into();
            v.sources = vec![from.to_string_lossy().into_owned()];
            v.dest = to.to_string_lossy().into_owned();
            v.verify = verify;
        });
        FORCE_WORKERS.store(workers, Ordering::Relaxed);
        let start = Instant::now();
        run(&job);
        let seconds = start.elapsed().as_secs_f64();
        FORCE_WORKERS.store(0, Ordering::Relaxed);
        let view = job.view().clone();
        let copied = walk_count(&to.join("from"));
        assert_eq!(copied, count, "every file is there");
        let _ = std::fs::remove_dir_all(&dir);
        (seconds, view)
    }

    fn walk_count(dir: &Path) -> usize {
        std::fs::read_dir(dir).map_or(0, |entries| {
            entries.flatten().map(|e| if e.file_type().is_ok_and(|t| t.is_dir()) { walk_count(&e.path()) } else { 1 }).sum()
        })
    }

    /// Prints the cost of a copy of many small files (run with `-- --nocapture`; `GLINT_BENCH_COPY=50000`
    /// for the big one) and checks what the job says at the end.
    #[test]
    fn small_files_cost_little_through_the_queue() {
        let count = std::env::var("GLINT_BENCH_COPY").ok().and_then(|n| n.parse().ok()).unwrap_or(2000);
        for workers in [1, 16] {
            let (seconds, view) = timed_copy(count, workers, false);
            println!("queue copy of {count} small files, {workers} worker(s): {:.0} ms ({:.0} files/s)", seconds * 1000.0, count as f64 / seconds);
            assert_eq!(view.state, "done");
            assert_eq!(view.files, count as u64);
            assert_eq!(view.files_done, count as u64);
            assert_eq!(view.bytes_done, count as u64 * 4096);
            assert!(view.errors.is_empty(), "{:?}", view.errors);
            assert_eq!(view.made.len(), 1, "a new folder is one entry for Undo");
        }
    }

    #[test]
    fn a_verified_queue_copy_counts_every_file_checked() {
        let (_, view) = timed_copy(300, 4, true);
        assert_eq!(view.state, "done");
        assert_eq!(view.verified, 300);
        assert_eq!(view.files_done, 300);
    }

    #[test]
    fn only_a_volume_that_clones_blocks_skips_the_room_check() {
        let dir = std::env::temp_dir().join(format!("glint-queue-clone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        let source = dir.join("a.txt").to_string_lossy().into_owned();
        assert!(!clones_blocks(&[], &dir), "nothing to copy");
        assert!(!clones_blocks(&[r"Z:\no\such\file".into()], &dir), "a source that isn't there");
        // The temp folder is NTFS on this PC: no block cloning, so a copy that doesn't fit is still refused.
        if reflink_copy::check_reflink_support(&source, &dir).is_ok_and(|s| s == reflink_copy::ReflinkSupport::Supported) {
            assert!(clones_blocks(&[source], &dir));
        } else {
            assert!(!clones_blocks(&[source], &dir));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
