//! Git in folders: the branch a folder's repository is on, how far it is from its upstream, and what
//! changed in the folder, rolled up to what's right in it (a folder shows the strongest change below
//! it). Read with gitoxide, which only reads: nothing here writes to a repository, runs git, or goes
//! on the network.
//!
//! A repository's own settings can name programs for status to run (filter drivers, such as Git LFS's,
//! and `core.fsmonitor`): one whose `.git/config` does is *limited* (no status) until it's trusted.

use crate::fsx::{blocking, ms_since};
use gix::bstr::{BStr, BString, ByteSlice};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, State, Window};

/// What a name in a folder has, strongest last: a folder shows the strongest of what's below it.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Mark {
    /// Left out by .gitignore.
    Ignored,
    /// New, not added yet.
    Untracked,
    /// Changed and added (to the next commit).
    Staged,
    /// Changed since it was added or committed (deleted included).
    Modified,
    /// A merge left it with conflicts.
    Conflicted,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub modified: u32,
    pub staged: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitView {
    /// The repository's working folder.
    pub root: String,
    /// The branch checked out ("main"); None when HEAD is detached.
    pub branch: Option<String>,
    /// HEAD's commit, abbreviated; None before the first commit.
    pub head: Option<String>,
    /// The branch it tracks ("origin/main").
    pub upstream: Option<String>,
    /// Commits only on this side, and only on the upstream's (up to [`MAX_WALK`] each).
    pub ahead: u32,
    pub behind: u32,
    /// What's right in the folder has, by lowercase name.
    pub marks: HashMap<String, Mark>,
    /// Changes in the folder and below it.
    pub counts: Counts,
    /// Status ran out of time: what's shown is what it found by then.
    pub partial: bool,
    /// The repository's own settings would run programs: status waits until it's trusted.
    pub limited: bool,
    /// Too many files to look through on every visit: status shows on F5.
    pub on_request: bool,
    pub elapsed: f64,
}

/// How long status may take before what it found so far is shown.
const DEADLINE: Duration = Duration::from_secs(4);
/// Above this many files, status runs only when asked (F5).
const BIG_INDEX: usize = 300_000;
/// Ahead and behind are counted up to this.
const MAX_WALK: usize = 10_000;

/// Status runs in flight, by tab: another for the tab, or Stop, ends one.
static RUNS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = LazyLock::new(Default::default);

/// At most two status runs at once: each already uses up to 8 threads.
static SLOTS: LazyLock<(Mutex<u32>, Condvar)> = LazyLock::new(|| (Mutex::new(2), Condvar::new()));

struct Slot;

impl Slot {
    fn take() -> Slot {
        let (free, freed) = &*SLOTS;
        let mut free = free.lock().unwrap_or_else(|e| e.into_inner());
        while *free == 0 {
            free = freed.wait(free).unwrap_or_else(|e| e.into_inner());
        }
        *free -= 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (free, freed) = &*SLOTS;
        *free.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        freed.notify_one();
    }
}

/// A view worked out, and the generation of its repository it's good for.
type Kept = (u64, GitView);

/// Views already worked out, by repository and folder, good while their repository's watcher has
/// seen nothing change (its generation).
static CACHE: LazyLock<Mutex<HashMap<(String, String), Kept>>> = LazyLock::new(Default::default);
/// How many times each watched repository changed, by its lowercase root. A repository not watched
/// has none, and its views aren't kept.
static GENERATIONS: LazyLock<Mutex<HashMap<String, u64>>> = LazyLock::new(Default::default);
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

/// A path on another computer (`\\server\share`): status there runs on F5 only.
fn on_network(path: &str) -> bool {
    path.starts_with(r"\\") && !path.starts_with(r"\\?\")
}

/// Whether the repository's own config (not the user's or the system's) names programs status would
/// run: filter drivers (`filter.<name>.clean` or `.process`) or `core.fsmonitor`.
fn runs_programs(repo: &gix::Repository) -> bool {
    let config = repo.config_snapshot();
    let runs = config.plumbing().sections().any(|section| {
        let own = matches!(section.meta().source, gix::config::Source::Local | gix::config::Source::Worktree);
        let name = section.header().name();
        own && ((name.eq_ignore_ascii_case(b"filter") && (section.contains_value_name("clean") || section.contains_value_name("process")))
            || (name.eq_ignore_ascii_case(b"core") && section.contains_value_name("fsmonitor")))
    });
    runs
}

/// `folder` as a path from `root` with forward slashes ("src/lib"), "" for the root itself; None when
/// it isn't in it.
fn inside(root: &Path, folder: &Path) -> Option<String> {
    let (root, folder) = (root.to_string_lossy().replace('/', "\\"), folder.to_string_lossy().replace('/', "\\"));
    let (root, folder) = (root.trim_end_matches('\\'), folder.trim_end_matches('\\'));
    if folder.eq_ignore_ascii_case(root) {
        return Some(String::new());
    }
    let rest = folder.get(root.len()..).filter(|rest| rest.starts_with('\\') && folder[..root.len()].eq_ignore_ascii_case(root))?;
    Some(rest.trim_start_matches('\\').replace('\\', "/"))
}

/// What a status item says, and where.
fn mark_of(item: &gix::status::Item) -> Option<(Mark, &BStr)> {
    use gix::status::index_worktree::iter::Summary;
    match item {
        gix::status::Item::TreeIndex(change) => Some((Mark::Staged, change.location())),
        gix::status::Item::IndexWorktree(item) => {
            let mark = match item.summary()? {
                Summary::Conflict => Mark::Conflicted,
                Summary::Added | Summary::IntentToAdd => Mark::Untracked,
                Summary::Removed | Summary::Modified | Summary::TypeChange | Summary::Renamed | Summary::Copied => Mark::Modified,
            };
            Some((mark, item.rela_path()))
        }
    }
}

/// Counts `mark` at `path` (from the repository's root), and rolls it up to the name right in the
/// folder at `prefix` ("src/lib/", or "" at the root) that it's in.
fn record(view: &mut GitView, prefix: &str, path: &BStr, mark: Mark) {
    let counts = &mut view.counts;
    match mark {
        Mark::Modified => counts.modified += 1,
        Mark::Staged => counts.staged += 1,
        Mark::Untracked => counts.untracked += 1,
        Mark::Conflicted => counts.conflicted += 1,
        Mark::Ignored => {}
    }
    let Some(rest) = path.to_str_lossy().strip_prefix(prefix).map(str::to_string) else { return };
    let Some(name) = rest.split('/').find(|p| !p.is_empty()) else { return };
    let name = name.to_lowercase();
    let strongest = view.marks.get(&name).map_or(mark, |&had| had.max(mark));
    view.marks.insert(name, strongest);
}

/// The view of `folder` in its repository, or None when it isn't in one with a working folder (a bare
/// repository, the `.git` folder itself). `force` (F5) runs status on a network path and in a very
/// big repository too; `trusted` lists repositories (by root) whose own settings may run programs.
fn view(folder: &str, trusted: &[String], force: bool, stop: Arc<AtomicBool>) -> Result<Option<GitView>, String> {
    let started = Instant::now();
    if folder.to_lowercase().split(['\\', '/']).any(|part| part == ".git") {
        return Ok(None);
    }
    let repo = match gix::discover(folder) {
        Ok(repo) => repo,
        Err(_) => return Ok(None),
    };
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else { return Ok(None) };
    let Some(relative) = inside(&workdir, Path::new(folder)) else { return Ok(None) };
    let root = workdir.to_string_lossy().trim_end_matches(['\\', '/']).replace('/', "\\");

    let mut view = GitView { root: root.clone(), ..Default::default() };
    let head_name = repo.head_name().ok().flatten();
    view.branch = head_name.as_ref().map(|name| name.shorten().to_string());
    let head = repo.head_id().ok().map(|id| id.detach());
    view.head = head.map(|id| id.to_hex_with_len(7).to_string());
    let upstream = head_name.as_ref().and_then(|name| repo.branch_remote_tracking_ref_name(name.as_ref(), gix::remote::Direction::Fetch)).and_then(Result::ok);
    if let Some(upstream) = upstream {
        view.upstream = Some(upstream.shorten().to_string().trim_start_matches("remotes/").to_string());
        let theirs = repo.find_reference(upstream.as_ref()).ok().and_then(|mut r| r.peel_to_id().ok()).map(|id| id.detach());
        if let (Some(ours), Some(theirs)) = (head, theirs) {
            let count = |tip: gix::ObjectId, hidden: gix::ObjectId| repo.rev_walk([tip]).with_hidden([hidden]).all().map_or(0, |walk| walk.take(MAX_WALK).filter(Result::is_ok).count() as u32);
            view.ahead = count(ours, theirs);
            view.behind = count(theirs, ours);
        }
    }

    let trusted = trusted.iter().any(|t| key(t) == key(&root));
    if runs_programs(&repo) && !trusted {
        view.limited = true;
        view.elapsed = ms_since(started);
        return Ok(Some(view));
    }
    let index = repo.index_or_empty().map_err(|e| e.to_string())?;
    if !force && (index.entries().len() > BIG_INDEX || on_network(&root)) {
        view.on_request = true;
        view.elapsed = ms_since(started);
        return Ok(Some(view));
    }

    let _slot = Slot::take();
    if stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    // Out of time (or stopped), status ends with what it found: the watchdog says so.
    let interrupt = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let (interrupt, finished, stop) = (interrupt.clone(), finished.clone(), stop.clone());
        std::thread::spawn(move || {
            let begun = Instant::now();
            while !finished.load(Ordering::Relaxed) {
                if begun.elapsed() > DEADLINE || stop.load(Ordering::Relaxed) {
                    interrupt.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let prefix = if relative.is_empty() { String::new() } else { format!("{relative}/") };
    let patterns: Vec<BString> = if relative.is_empty() { Vec::new() } else { vec![BString::from(prefix.clone())] };
    let items = repo
        .status(gix::progress::Discard)
        .map_err(|e| e.to_string())?
        .untracked_files(gix::status::UntrackedFiles::Collapsed)
        .index_worktree_submodules(gix::status::Submodule::Given { ignore: gix::submodule::config::Ignore::All, check_dirty: false })
        .index_worktree_rewrites(None)
        .index_worktree_options_mut(|options| options.thread_limit = Some(8))
        .should_interrupt_owned(interrupt.clone())
        .into_iter(patterns)
        .map_err(|e| e.to_string());
    let outcome = items.map(|items| {
        for item in items.flatten() {
            if let Some((mark, path)) = mark_of(&item) {
                record(&mut view, &prefix, path, mark);
            }
        }
    });
    finished.store(true, Ordering::Relaxed);
    let _ = watchdog.join();
    if stop.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    outcome?;
    view.partial = interrupt.load(Ordering::Relaxed);

    // What's left out by .gitignore, of what's right in the folder and isn't tracked.
    if let Ok(mut excludes) = repo.excludes(&index, None, gix::worktree::stack::state::ignore::Source::WorktreeThenIdMappingIfNotSkipped) {
        let tracked: HashSet<String> = index
            .entries()
            .iter()
            .filter_map(|e| e.path(&index).to_str_lossy().strip_prefix(prefix.as_str()).and_then(|rest| rest.split('/').next()).map(str::to_lowercase))
            .collect();
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if view.marks.contains_key(&lower) || tracked.contains(&lower) || lower == ".git" {
                continue;
            }
            let dir = entry.file_type().is_ok_and(|t| t.is_dir());
            let mode = if dir { gix::index::entry::Mode::DIR } else { gix::index::entry::Mode::FILE };
            if excludes.at_entry(format!("{prefix}{name}").as_str(), Some(mode)).is_ok_and(|p| p.is_excluded()) {
                view.marks.insert(lower, Mark::Ignored);
            }
        }
    }
    view.elapsed = ms_since(started);
    Ok(Some(view))
}

/// Git in `folder` for the tab `lane` (a newer request there, or Stop, ends this one): None when it
/// isn't in a repository. A view worked out since its repository last changed comes back at once.
#[tauri::command]
pub async fn git_view(folder: String, lane: String, trusted: Vec<String>, force: bool) -> Result<Option<GitView>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(old) = RUNS.lock().unwrap_or_else(|e| e.into_inner()).insert(lane.clone(), stop.clone()) {
        old.store(true, Ordering::SeqCst);
    }
    blocking(move || {
        let cached = |root: &str| {
            let generation = GENERATIONS.lock().unwrap_or_else(|e| e.into_inner()).get(&key(root)).copied()?;
            let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            cache.get(&(key(root), key(&folder))).filter(|(g, _)| *g == generation).map(|(_, v)| v.clone())
        };
        // The repository is found again each time (cheap), and its view reused if nothing changed.
        let root = gix::discover(&folder).ok().and_then(|r| r.workdir().map(|w| w.to_string_lossy().replace('/', "\\")));
        if let Some(view) = root.as_deref().filter(|_| !force).and_then(cached) {
            return Ok(Some(view));
        }
        let generation = root.as_deref().and_then(|r| GENERATIONS.lock().unwrap_or_else(|e| e.into_inner()).get(&key(r)).copied());
        let found = view(&folder, &trusted, force, stop.clone());
        let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
        if runs.get(&lane).is_some_and(|s| Arc::ptr_eq(s, &stop)) {
            runs.remove(&lane);
        }
        drop(runs);
        if let (Ok(Some(view)), Some(generation)) = (&found, generation) {
            if !view.partial {
                let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
                if cache.len() > 256 {
                    cache.clear();
                }
                cache.insert((key(&view.root), key(&folder)), (generation, view.clone()));
            }
        }
        found
    })
    .await
}

#[tauri::command]
pub fn cancel_git(lane: String) {
    if let Some(stop) = RUNS.lock().unwrap_or_else(|e| e.into_inner()).remove(&lane) {
        stop.store(true, Ordering::SeqCst);
    }
}

/// The most of a file (either side) a diff reads.
const DIFF_BYTES: usize = 1 << 20;
/// The most lines a diff sends: a whole file rewritten is shown up to here.
const DIFF_LINES: usize = 5_000;
/// Lines kept around each change.
const CONTEXT: u32 = 3;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LineKind {
    Same,
    Removed,
    Added,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    /// Its line number before and after, from 1; 0 on the side it isn't on.
    pub old: u32,
    pub new: u32,
    pub text: String,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    /// Where it is in its repository ("src/lib/fs.ts").
    pub path: String,
    /// Not in the last commit: all of it is new.
    pub new_file: bool,
    /// Either side has a NUL byte in its first 8 KB, as Git tells binary files.
    pub binary: bool,
    /// Either side is over [`DIFF_BYTES`]: sizes only.
    pub too_big: bool,
    pub old_size: u64,
    pub new_size: u64,
    pub added: u32,
    pub removed: u32,
    /// Changes with up to [`CONTEXT`] lines around them; a gap between two is a hunk's end.
    pub lines: Vec<DiffLine>,
    /// More lines changed than [`DIFF_LINES`]: the first are shown.
    pub cut: bool,
    pub elapsed: f64,
}

/// What `old` and `new` differ in, line by line, with Windows line endings evened out (so a file
/// Git stores with LF and checks out with CRLF doesn't differ on every line).
fn line_diff(old: &str, new: &str, diff: &mut FileDiff) {
    use gix::diff::blob::{Algorithm, Diff, InternedInput};
    let (old, new) = (old.replace("\r\n", "\n"), new.replace("\r\n", "\n"));
    let input = InternedInput::new(old.as_str(), new.as_str());
    let mut found = Diff::compute(Algorithm::Histogram, &input);
    found.postprocess_lines(&input);
    diff.added = found.count_additions();
    diff.removed = found.count_removals();
    let text = |token: gix::diff::blob::Token| input.interner[token].trim_end_matches('\n').to_string();
    let (before, after) = (input.before.len() as u32, input.after.len() as u32);
    let mut shown_to = 0u32; // lines of `old` already sent
    let push = |diff: &mut FileDiff, line: DiffLine| {
        if diff.lines.len() >= DIFF_LINES {
            diff.cut = true;
        } else {
            diff.lines.push(line);
        }
    };
    let hunks: Vec<_> = found.hunks().collect();
    for (n, hunk) in hunks.iter().enumerate() {
        // Context before: back from the change, not before what's shown already.
        let from = hunk.before.start.saturating_sub(CONTEXT).max(shown_to);
        let offset = hunk.after.start as i64 - hunk.before.start as i64;
        for at in from..hunk.before.start {
            push(diff, DiffLine { kind: LineKind::Same, old: at + 1, new: (at as i64 + offset) as u32 + 1, text: text(input.before[at as usize]) });
        }
        for at in hunk.before.clone() {
            push(diff, DiffLine { kind: LineKind::Removed, old: at + 1, new: 0, text: text(input.before[at as usize]) });
        }
        for at in hunk.after.clone() {
            push(diff, DiffLine { kind: LineKind::Added, old: 0, new: at + 1, text: text(input.after[at as usize]) });
        }
        // Context after: up to the next change, which brings its own.
        let next = hunks.get(n + 1).map_or(before, |h| h.before.start);
        let to = (hunk.before.end + CONTEXT).min(next).min(before);
        let offset = hunk.after.end as i64 - hunk.before.end as i64;
        for at in hunk.before.end..to {
            let new = (at as i64 + offset) as u32;
            if new < after {
                push(diff, DiffLine { kind: LineKind::Same, old: at + 1, new: new + 1, text: text(input.before[at as usize]) });
            }
        }
        shown_to = to;
    }
}

/// Whether bytes look binary, as Git tells: a NUL in the first 8 KB.
fn binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(8192)].contains(&0)
}

/// What changed in the file at `path` since the last commit: its version in HEAD against the one on
/// disk (staged changes and the rest). None when it isn't in a repository.
fn file_diff(path: &str) -> Result<Option<FileDiff>, String> {
    let started = Instant::now();
    let file = Path::new(path);
    let Some(folder) = file.parent() else { return Ok(None) };
    let Ok(repo) = gix::discover(folder) else { return Ok(None) };
    let Some(workdir) = repo.workdir().map(Path::to_path_buf) else { return Ok(None) };
    let Some(relative) = inside(&workdir, file).filter(|r| !r.is_empty()) else { return Ok(None) };
    let mut diff = FileDiff { path: relative.clone(), ..Default::default() };

    let now = std::fs::read(file).map_err(|e| e.to_string())?;
    let then: Option<Vec<u8>> = repo
        .head_tree_id()
        .ok()
        .and_then(|tree| repo.find_tree(tree.detach()).ok())
        .and_then(|tree| tree.lookup_entry_by_path(&relative).ok().flatten())
        .and_then(|entry| repo.find_object(entry.object_id()).ok())
        .map(|object| object.detach().data);
    diff.new_file = then.is_none();
    let then = then.unwrap_or_default();
    (diff.old_size, diff.new_size) = (then.len() as u64, now.len() as u64);
    diff.binary = binary(&then) || binary(&now);
    diff.too_big = then.len() > DIFF_BYTES || now.len() > DIFF_BYTES;
    if !diff.binary && !diff.too_big {
        line_diff(&String::from_utf8_lossy(&then), &String::from_utf8_lossy(&now), &mut diff);
    }
    diff.elapsed = ms_since(started);
    Ok(Some(diff))
}

/// What changed in the file at `path` since the last commit (the Changes view); None outside a repository.
#[tauri::command]
pub async fn git_diff(path: String) -> Result<Option<FileDiff>, String> {
    blocking(move || file_diff(&path)).await
}

/// One recursive watcher per repository on screen, by its lowercase root.
#[derive(Default)]
pub struct RepoWatcher(Arc<Mutex<HashMap<String, RecommendedWatcher>>>);

/// Changes that don't change status: Git's own object store and logs, and lock files mid-write.
fn noise(path: &Path) -> bool {
    let text = path.to_string_lossy().to_lowercase().replace('/', "\\");
    text.contains("\\.git\\objects\\") || text.contains("\\.git\\logs\\") || text.ends_with(".lock")
}

/// Watches exactly the repositories at `roots`, each emitting "git-changed" with its root at most
/// every 500 ms. A repository no longer watched forgets its views.
#[tauri::command]
pub async fn git_watch(window: Window, state: State<'_, RepoWatcher>, roots: Vec<String>) -> Result<(), String> {
    let watchers = state.0.clone();
    blocking(move || {
        let mut watchers = watchers.lock().unwrap_or_else(|e| e.into_inner());
        let wanted: HashSet<String> = roots.iter().map(|r| key(r)).collect();
        let mut generations = GENERATIONS.lock().unwrap_or_else(|e| e.into_inner());
        watchers.retain(|k, _| wanted.contains(k));
        generations.retain(|k, _| wanted.contains(k));
        for root in roots {
            if watchers.contains_key(&key(&root)) || on_network(&root) {
                continue;
            }
            let pending = Arc::new(AtomicBool::new(false));
            let (window, changed, root_key) = (window.clone(), root.clone(), key(&root));
            let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                if event.paths.iter().all(|p| noise(p)) {
                    return;
                }
                // What was worked out before this change is out of date at once; the UI asks again
                // once things settle.
                GENERATIONS.lock().unwrap_or_else(|e| e.into_inner()).insert(root_key.clone(), NEXT_GENERATION.fetch_add(1, Ordering::Relaxed));
                if !pending.swap(true, Ordering::SeqCst) {
                    let (window, root, pending) = (window.clone(), changed.clone(), pending.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(500));
                        pending.store(false, Ordering::SeqCst);
                        let _ = window.emit("git-changed", root);
                    });
                }
            });
            let Ok(mut watcher) = watcher else { continue };
            if watcher.watch(&PathBuf::from(&root), RecursiveMode::Recursive).is_ok() {
                generations.insert(key(&root), NEXT_GENERATION.fetch_add(1, Ordering::Relaxed));
                watchers.insert(key(&root), watcher);
            }
        }
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    /// A test repository made with git itself (on PATH, as on every CI runner); a test skips without it.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Option<Repo> {
            let dir = std::env::temp_dir().join(format!("glint-git-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            let repo = Repo(dir);
            if repo.git(&["init", "-q", "-b", "main"]).is_none() {
                eprintln!("git isn't on PATH: skipped");
                return None;
            }
            repo.git(&["config", "user.email", "test@example.com"])?;
            repo.git(&["config", "user.name", "Test"])?;
            repo.git(&["config", "core.autocrlf", "false"])?;
            Some(repo)
        }

        fn git(&self, args: &[&str]) -> Option<()> {
            let out = Command::new("git").args(args).current_dir(&self.0).output().ok()?;
            out.status.success().then_some(())
        }

        fn put(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn at(&self, folder: &str) -> String {
            self.0.join(folder).to_string_lossy().trim_end_matches('\\').to_string()
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn marks(view: &GitView) -> Vec<(String, Mark)> {
        let mut marks: Vec<_> = view.marks.iter().map(|(k, v)| (k.clone(), *v)).collect();
        marks.sort();
        marks
    }

    #[test]
    fn rolls_changes_up_to_what_is_in_the_folder() {
        let Some(repo) = Repo::new("status") else { return };
        repo.put("README.md", "hello\n");
        repo.put("src/lib/a.rs", "a\n");
        repo.put("src/lib/b.rs", "b\n");
        repo.put("src/main.rs", "main\n");
        repo.put(".gitignore", "target/\n*.log\n");
        repo.git(&["add", "."]).unwrap();
        repo.git(&["commit", "-q", "-m", "first"]).unwrap();
        // A change in a subfolder, one staged, one new, and what .gitignore leaves out.
        repo.put("src/lib/a.rs", "a changed\n");
        repo.put("src/main.rs", "main changed\n");
        repo.git(&["add", "src/main.rs"]).unwrap();
        repo.put("src/new.rs", "new\n");
        repo.put("target/debug/app.exe", "x");
        repo.put("build.log", "x");
        let stop = Arc::new(AtomicBool::new(false));

        let top = view(&repo.at(""), &[], false, stop.clone()).unwrap().unwrap();
        assert_eq!(top.branch.as_deref(), Some("main"));
        assert_eq!(top.head.as_ref().map(String::len), Some(7));
        assert_eq!(marks(&top), [("build.log".into(), Mark::Ignored), ("src".into(), Mark::Modified), ("target".into(), Mark::Ignored)]);
        assert_eq!(top.counts, Counts { modified: 1, staged: 1, untracked: 1, conflicted: 0 });
        assert!(!top.partial && !top.limited);

        let src = view(&repo.at("src"), &[], false, stop.clone()).unwrap().unwrap();
        assert_eq!(marks(&src), [("lib".into(), Mark::Modified), ("main.rs".into(), Mark::Staged), ("new.rs".into(), Mark::Untracked)]);
        let lib = view(&repo.at("src\\lib"), &[], false, stop.clone()).unwrap().unwrap();
        assert_eq!(marks(&lib), [("a.rs".into(), Mark::Modified)]);
        assert_eq!(lib.counts.modified, 1);

        // Not a repository; the .git folder itself.
        let outside = std::env::temp_dir().join(format!("glint-git-none-{}", std::process::id()));
        fs::create_dir_all(&outside).unwrap();
        assert!(view(&outside.to_string_lossy(), &[], false, stop.clone()).unwrap().is_none() || std::env::temp_dir().join(".git").exists());
        let _ = fs::remove_dir_all(&outside);
        assert!(view(&repo.at(".git"), &[], false, stop).unwrap().is_none());
    }

    #[test]
    fn counts_ahead_and_behind_its_upstream() {
        let Some(upstream) = Repo::new("upstream") else { return };
        upstream.put("a.txt", "1\n");
        upstream.git(&["add", "."]).unwrap();
        upstream.git(&["commit", "-q", "-m", "1"]).unwrap();
        let Some(clone) = Repo::new("clone") else { return };
        fs::remove_dir_all(&clone.0).unwrap();
        Command::new("git").args(["clone", "-q"]).arg(&upstream.0).arg(&clone.0).output().unwrap();
        clone.git(&["config", "user.email", "test@example.com"]).unwrap();
        clone.git(&["config", "user.name", "Test"]).unwrap();
        // Two commits here, one there (fetched).
        for n in ["2", "3"] {
            clone.put("b.txt", n);
            clone.git(&["commit", "-q", "-am", n]).or_else(|| clone.git(&["add", "."]).and_then(|_| clone.git(&["commit", "-q", "-m", n]))).unwrap();
        }
        upstream.put("a.txt", "4\n");
        upstream.git(&["commit", "-q", "-am", "4"]).unwrap();
        clone.git(&["fetch", "-q"]).unwrap();
        let view = view(&clone.at(""), &[], false, Arc::new(AtomicBool::new(false))).unwrap().unwrap();
        assert_eq!((view.upstream.as_deref(), view.ahead, view.behind), (Some("origin/main"), 2, 1));
    }

    #[test]
    fn waits_for_trust_where_settings_would_run_programs() {
        let Some(repo) = Repo::new("filters") else { return };
        repo.put("a.txt", "a\n");
        repo.git(&["add", "."]).unwrap();
        repo.git(&["commit", "-q", "-m", "1"]).unwrap();
        repo.put("a.txt", "changed\n");
        // As Git LFS sets it up. (No .gitattributes names it, so nothing runs it here either way.)
        repo.git(&["config", "filter.lfs.clean", "git-lfs clean -- %f"]).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let limited = view(&repo.at(""), &[], false, stop.clone()).unwrap().unwrap();
        assert!(limited.limited && limited.marks.is_empty());
        let trusted = view(&repo.at(""), &[repo.at("")], false, stop).unwrap().unwrap();
        assert!(!trusted.limited);
        assert_eq!(marks(&trusted), [("a.txt".into(), Mark::Modified)]);
    }

    #[test]
    fn diffs_lines_with_some_around_each_change() {
        // Committed with LF, on disk with CRLF: only what changed differs.
        let old = (1..=20).map(|n| format!("line {n}\n")).collect::<String>();
        let new = old.replace("line 5\n", "line five\n").replace("line 18\n", "").replace('\n', "\r\n");
        let mut diff = FileDiff::default();
        line_diff(&old, &new, &mut diff);
        assert_eq!((diff.added, diff.removed), (1, 2));
        let shown: Vec<(LineKind, u32, u32, &str)> = diff.lines.iter().map(|l| (l.kind, l.old, l.new, l.text.as_str())).collect();
        use LineKind::*;
        assert_eq!(
            shown,
            [
                (Same, 2, 2, "line 2"),
                (Same, 3, 3, "line 3"),
                (Same, 4, 4, "line 4"),
                (Removed, 5, 0, "line 5"),
                (Added, 0, 5, "line five"),
                (Same, 6, 6, "line 6"),
                (Same, 7, 7, "line 7"),
                (Same, 8, 8, "line 8"),
                // A gap: lines 9–14 are the same.
                (Same, 15, 15, "line 15"),
                (Same, 16, 16, "line 16"),
                (Same, 17, 17, "line 17"),
                (Removed, 18, 0, "line 18"),
                (Same, 19, 18, "line 19"),
                (Same, 20, 19, "line 20"),
            ]
        );
    }

    #[test]
    fn diffs_a_file_against_its_last_commit() {
        let Some(repo) = Repo::new("diff") else { return };
        repo.put("a.txt", "one\ntwo\nthree\n");
        repo.put("pic.bin", "\0\x01\x02");
        repo.git(&["add", "."]).unwrap();
        repo.git(&["commit", "-q", "-m", "1"]).unwrap();
        repo.put("a.txt", "one\n2\nthree\nfour\n");
        repo.put("new.txt", "fresh\n");
        let diff = file_diff(&repo.at("a.txt")).unwrap().unwrap();
        assert_eq!((diff.path.as_str(), diff.new_file, diff.added, diff.removed), ("a.txt", false, 2, 1));
        let fresh = file_diff(&repo.at("new.txt")).unwrap().unwrap();
        assert!(fresh.new_file && fresh.added == 1 && fresh.lines[0].kind == LineKind::Added);
        assert!(file_diff(&repo.at("pic.bin")).unwrap().unwrap().binary);
    }

    /// `GLINT_GIT_FOLDER=C:\Repos\app cargo test --release -- --ignored --nocapture views_a_real_folder`:
    /// a real folder's view, and how long it takes (nothing is written).
    #[test]
    #[ignore]
    fn views_a_real_folder() {
        let Ok(folder) = std::env::var("GLINT_GIT_FOLDER") else { return };
        for run in 0..3 {
            let view = view(&folder, &[], true, Arc::new(AtomicBool::new(false))).unwrap();
            let Some(view) = view else { return println!("not in a repository") };
            let mut marks: Vec<_> = view.marks.iter().collect();
            marks.sort();
            println!("run {run}: {:.0} ms · {} {:?} ↑{} ↓{} · {:?} · partial {} · {} marks: {:?}", view.elapsed, view.branch.as_deref().unwrap_or("(detached)"), view.upstream, view.ahead, view.behind, view.counts, view.partial, marks.len(), marks.iter().take(8).collect::<Vec<_>>());
        }
    }

    #[test]
    fn finds_a_folder_in_its_repository() {
        assert_eq!(inside(Path::new(r"C:\Repos\App"), Path::new(r"c:\repos\app\src\lib")).as_deref(), Some("src/lib"));
        assert_eq!(inside(Path::new(r"C:\Repos\App\"), Path::new(r"C:\Repos\App")).as_deref(), Some(""));
        assert_eq!(inside(Path::new(r"C:\Repos\App"), Path::new(r"C:\Repos\Apple")), None);
        assert!(noise(Path::new(r"C:\r\.git\objects\ab\cd")) && noise(Path::new(r"C:\r\.git\index.lock")) && !noise(Path::new(r"C:\r\.git\index")));
    }
}
