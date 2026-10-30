//! The copy queue: copies and moves that wait their turn, one at a time, each showing how far it is, its
//! speed and the file it's on; paused and resumed (a file half-copied waits where it is), cancelled, and
//! checked after copying when asked (each copy read back from the disk and compared with its original,
//! by BLAKE3). A name that's taken asks: replace it, skip it, or keep both (for the rest too). A move
//! within a drive is a rename; across drives, a copy whose originals go only once all of an item is
//! copied (and checked). Glint's own engine (CopyFileEx), for what Windows' copy dialog doesn't do;
//! Windows' own stays for every other copy.

use serde::Serialize;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// What went wrong with one file.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Failure {
    pub path: String,
    pub error: String,
}

/// An item made where it was copied or moved to: Undo's way back.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Made {
    pub from: String,
    pub to: String,
}

/// A job, as the app shows it.
#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
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
        (q.app.clone(), q.jobs.iter().map(|j| j.view().clone()).collect::<Vec<_>>())
    };
    if let Some(app) = app {
        let _ = app.emit("copy-queue", views);
    }
}

fn job(id: u64) -> Result<Arc<Job>, String> {
    queue().jobs.iter().find(|j| j.view().id == id).cloned().ok_or_else(|| "That job isn't in the queue any more".to_string())
}

/// Adds a copy or a move of `sources` into `dest`: its id. It starts when the ones before it are done.
#[tauri::command]
pub fn queue_add(app: AppHandle, kind: String, sources: Vec<String>, dest: String, verify: bool) -> Result<u64, String> {
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

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
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
#[tauri::command]
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

#[tauri::command]
pub fn queue_jobs() -> Vec<JobView> {
    queue().jobs.iter().map(|j| j.view().clone()).collect()
}

/// Takes the finished jobs off the list.
#[tauri::command]
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

/// What's in the items to copy: their files (and folders to make), under `dest`.
fn plan(sources: &[String], dest: &Path, failures: &mut Vec<Failure>) -> (Vec<Step>, Vec<PathBuf>) {
    let mut steps = Vec::new();
    let mut folders = Vec::new();
    for (item, source) in sources.iter().enumerate() {
        let from = PathBuf::from(source);
        let Some(name) = from.file_name() else { continue };
        let to = dest.join(name);
        match crate::fsx::stat(source) {
            Some(e) if e.dir => {
                folders.push(to.clone());
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
                                    folders.push(b.clone());
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
                Ok(()) => job.set(|v| v.made.push(Made { from: source.clone(), to: to.to_string_lossy().into_owned() })),
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
        if free < total {
            job.set(|v| v.errors.push(Failure { path: dest.to_string_lossy().into_owned(), error: format!("Not enough space there: it needs {}, and has {}", size_text(total), size_text(free)) }));
            return finish(job, "failed");
        }
    }
    // What each item made there: a folder that wasn't there is Undo's to take back whole; a file, or
    // what went into a folder that was there already, is taken back file by file (never what was there).
    let fresh: Vec<bool> = sources.iter().map(|s| !item_root(&dest, s).exists()).collect();
    let mut created: Vec<Vec<(PathBuf, PathBuf)>> = vec![Vec::new(); sources.len()];
    // The originals copied (and checked) whole: a move's to delete.
    let mut copied_from: Vec<Vec<PathBuf>> = vec![Vec::new(); sources.len()];
    for folder in &folders {
        let _ = std::fs::create_dir_all(folder);
    }
    let done = Arc::new(AtomicU64::new(0));
    let mut window = (Instant::now(), 0u64);
    for step in &steps {
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
        job.set(|v| v.current = Some(step.from.to_string_lossy().into_owned()));
        let base = done.load(Ordering::Relaxed);
        let copied = copy_file(&step.from, &to, job, &done, base);
        match copied {
            Ok(()) => {
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
                    copied_from[step.item].push(step.from.clone());
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
            for from in &copied_from[i] {
                if let Err(e) = std::fs::remove_file(from) {
                    job.set(|v| v.errors.push(Failure { path: from.to_string_lossy().into_owned(), error: format!("Copied, but the original couldn't go: {e}") }));
                }
            }
            if Path::new(source).is_dir() {
                remove_empty(Path::new(source));
            }
        }
    }
    let state = if job.view().errors.is_empty() || job.view().files_done > 0 { "done" } else { "failed" };
    finish(job, state);
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
fn free_name(path: &Path) -> PathBuf {
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else { return path.to_path_buf() };
    let name = name.to_string_lossy();
    (2..10_000).map(|n| parent.join(crate::fsx::numbered(&name, false, n))).find(|p| !p.exists()).unwrap_or_else(|| path.to_path_buf())
}

fn clear_read_only(path: &Path) {
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
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            always: Mutex::new(None),
            answer: Mutex::new(None),
            wake: Condvar::new(),
            woken: Mutex::new(()),
        }
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
        assert!(folders.contains(&dir.join("to").join("from").join("sub")));
        let job = quiet();
        let done = AtomicU64::new(0);
        for folder in &folders {
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
}
