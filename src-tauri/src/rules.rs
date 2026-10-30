//! Folder rules: a folder (with its subfolders, if asked) watched for files that match (by their names,
//! their size, how long ago they changed), each acted on once it's complete: moved (into folders by its
//! date, if asked), renamed by a pattern, unblocked (Windows' downloaded-from-the-internet mark taken
//! off), extracted, or put in the Recycle Bin. A file is complete once its size has held for two seconds
//! and it opens for reading. Rules are off until they're turned on (after a preview of what they'd do),
//! run while Glint runs, and all they do is logged, each step with its Undo. A rule doesn't act on what a
//! rule just made, so a rename can't feed itself.
//!
//! A sync rule keeps the whole folder in sync with a place on one of rclone's remotes (rclone.rs): a
//! copy there, a mirror, or both ways. It runs a minute after Glint starts, half a minute after files
//! in the folder stop changing, and every so many minutes if asked; each run is logged with what it did.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// A rule, as the app's settings keep it.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub on: bool,
    pub folder: String,
    pub subfolders: bool,
    /// Names it takes: patterns with * and ?, separated by ";" or "," ("*.pdf; invoice*"); empty for any.
    pub names: String,
    /// Sizes it takes, in bytes (0: no bound).
    pub min_size: u64,
    pub max_size: u64,
    /// Files last changed at least this many days ago (0: any).
    pub older_than_days: u32,
    /// "move", "rename", "recycle", "unblock", "extract" or "sync".
    pub action: String,
    /// Move: the folder it goes into; {year}, {month} and {day} of the file's date may be in it. Sync:
    /// the place on a remote of rclone's ("gdrive:Backups").
    pub target: String,
    /// Rename: the new name, made of {name}, {ext}, {date}, {time}, {year}, {month} and {day}.
    pub pattern: String,
    /// Sync: "copy" (what's new or changed here goes there, nothing there deleted), "mirror" (there
    /// becomes what's here) or "both" (changes go both ways).
    pub sync: String,
    /// Sync: also every this many minutes (0: when files change, and at start).
    pub every: u32,
}

/// What a rule did (or would do, in a preview) to one file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub id: u64,
    /// When, in ms since 1970.
    pub at: i64,
    pub rule: String,
    pub action: String,
    pub from: String,
    /// Where it is now: moved or renamed, its Recycle Bin entry, or the folder it was extracted into.
    pub to: Option<String>,
    /// Unblock: the mark taken off, to put back.
    #[serde(default)]
    pub mark: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub undone: bool,
    /// A sync's: what it did, in a few words ("3 up, 1 down").
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Default)]
struct Engine {
    rules: Vec<Rule>,
    watchers: Vec<RecommendedWatcher>,
    /// Files seen changing, waiting until they're complete: size, and since when it's held.
    pending: HashMap<String, (u64, Instant)>,
    /// What rules made just now (lower case), and when: left alone a while.
    made: HashMap<String, Instant>,
    log: Vec<Done>,
    next: u64,
    app: Option<AppHandle>,
    /// Sync rules' runs, by rule id.
    syncs: HashMap<String, Syncing>,
}

/// A sync rule's runs: whether one is going, when the last ended, and since when its folder has been
/// changing (it runs once that stops).
#[derive(Default)]
struct Syncing {
    running: bool,
    last: Option<Instant>,
    changed: Option<Instant>,
}

static ENGINE: LazyLock<Mutex<Engine>> = LazyLock::new(Default::default);
static STARTED: Once = Once::new();

fn engine() -> MutexGuard<'static, Engine> {
    ENGINE.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long a file's size must hold before it's acted on, and how long what a rule made is left alone.
const SETTLE: Duration = Duration::from_secs(2);
const MADE: Duration = Duration::from_secs(30);
/// The log keeps the latest this many.
const KEPT: usize = 500;
/// A sync runs this long after files in its folder stop changing, and first this long after Glint starts.
const SYNC_QUIET: Duration = Duration::from_secs(30);
const SYNC_FIRST: Duration = Duration::from_secs(60);

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn log_file() -> Option<PathBuf> {
    crate::tools::data_dir().map(|d| d.join("rules-log.json"))
}

fn save_log(log: &[Done]) {
    if let (Some(path), Ok(text)) = (log_file(), serde_json::to_string(log)) {
        let _ = std::fs::write(path, text);
    }
}

/// `name` matches a pattern of * and ? (case aside).
fn wild(pattern: &[char], name: &[char]) -> bool {
    match (pattern.first(), name.first()) {
        (None, None) => true,
        (Some('*'), _) => wild(&pattern[1..], name) || (!name.is_empty() && wild(pattern, &name[1..])),
        (Some('?'), Some(_)) => wild(&pattern[1..], &name[1..]),
        (Some(p), Some(n)) => p.to_lowercase().eq(n.to_lowercase()) && wild(&pattern[1..], &name[1..]),
        _ => false,
    }
}

/// Whether `name` is already of the shape a rename rule's pattern makes (its tokens as what they stand
/// for): such a file isn't renamed again, so a rule can't keep adding to its own names.
fn renamed_already(pattern: &str, name: &str) -> bool {
    let mut shape = regex::escape(pattern.trim());
    for (token, stands) in [("{name}", ".+"), ("{ext}", r"(\.[^.]*)?"), ("{date}", r"\d{4}-\d{2}-\d{2}"), ("{time}", r"\d{2}\.\d{2}"), ("{year}", r"\d{4}"), ("{month}", r"\d{2}"), ("{day}", r"\d{2}")] {
        shape = shape.replace(&regex::escape(token), stands);
    }
    regex::RegexBuilder::new(&format!("^{shape}$")).case_insensitive(true).build().is_ok_and(|r| r.is_match(name))
}

fn takes_name(rule: &Rule, name: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let patterns: Vec<&str> = rule.names.split([';', ',']).map(str::trim).filter(|p| !p.is_empty()).collect();
    patterns.is_empty() || patterns.iter().any(|p| wild(&p.chars().collect::<Vec<_>>(), &name))
}

/// Whether `rule` takes the file at `path` (its size and date as given): its name, size and age.
fn takes(rule: &Rule, path: &Path, size: u64, modified: i64, now: i64) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy()) else { return false };
    takes_name(rule, &name)
        && !(rule.action == "rename" && renamed_already(&rule.pattern, &name))
        && (rule.min_size == 0 || size >= rule.min_size)
        && (rule.max_size == 0 || size <= rule.max_size)
        && (rule.older_than_days == 0 || now - modified >= i64::from(rule.older_than_days) * 86_400_000)
}

/// Whether `path` is where `rule` looks: in its folder, or below it when it looks in subfolders.
fn covers(rule: &Rule, path: &Path) -> bool {
    let (Some(parent), folder) = (path.parent(), Path::new(&rule.folder)) else { return false };
    let lower = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_lowercase();
    let (parent, folder) = (lower(parent), lower(folder));
    parent == folder || (rule.subfolders && parent.starts_with(&format!("{folder}\\")))
}

/// A file's date in this PC's time: year, month, day, hour, minute.
fn local(ms: i64) -> (u16, u16, u16, u16, u16) {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = ((ms.max(0) as u64) + 11_644_473_600_000) * 10_000;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut here) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        let _ = FileTimeToSystemTime(&file, &mut utc);
        let _ = SystemTimeToTzSpecificLocalTime(None, &utc, &mut here);
    }
    (here.wYear, here.wMonth, here.wDay, here.wHour, here.wMinute)
}

/// `text` with the file's tokens filled in.
fn fill(text: &str, path: &Path, modified: i64) -> String {
    let (year, month, day, hour, minute) = local(modified);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    text.replace("{name}", &stem)
        .replace("{ext}", &ext)
        .replace("{date}", &format!("{year:04}-{month:02}-{day:02}"))
        .replace("{time}", &format!("{hour:02}.{minute:02}"))
        .replace("{year}", &format!("{year:04}"))
        .replace("{month}", &format!("{month:02}"))
        .replace("{day}", &format!("{day:02}"))
}

/// What `rule` would do with the file: in words, and where it'd end up (None when it wouldn't move).
fn plan(rule: &Rule, path: &Path, modified: i64) -> Result<(String, Option<PathBuf>), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match rule.action.as_str() {
        "move" => {
            let folder = PathBuf::from(fill(rule.target.trim(), path, modified));
            if !folder.is_absolute() {
                return Err("Where to? A folder such as D:\\Documents\\{year}".into());
            }
            Ok((format!("Move into {}", folder.display()), Some(folder.join(&name))))
        }
        "rename" => {
            let new = fill(rule.pattern.trim(), path, modified);
            if new.is_empty() || new.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) || new.ends_with(['.', ' ']) {
                return Err(format!("“{new}” can't be a file's name"));
            }
            Ok((format!("Rename to {new}"), Some(path.with_file_name(new))))
        }
        "recycle" => Ok(("Move to the Recycle Bin".into(), None)),
        "unblock" => Ok(("Unblock: take off Windows' downloaded-from-the-internet mark".into(), None)),
        "extract" => Ok(("Extract into a folder next to it".into(), None)),
        "sync" => Ok((format!("Sync with {}", rule.target.trim()), None)),
        other => Err(format!("Rules don't know “{other}”")),
    }
}

/// rclone's filters for the files a sync rule takes: its names (any case), sizes and age, and its
/// folder alone unless it looks in subfolders.
fn sync_filters(rule: &Rule) -> Vec<String> {
    let mut filters = Vec::new();
    let names: Vec<&str> = rule.names.split([';', ',']).map(str::trim).filter(|p| !p.is_empty()).collect();
    if !names.is_empty() {
        filters.push("--ignore-case".to_string());
        for name in names {
            filters.extend(["--include".to_string(), name.to_string()]);
        }
    }
    if rule.min_size > 0 {
        filters.extend(["--min-size".to_string(), format!("{}B", rule.min_size)]);
    }
    if rule.max_size > 0 {
        filters.extend(["--max-size".to_string(), format!("{}B", rule.max_size)]);
    }
    if rule.older_than_days > 0 {
        filters.extend(["--min-age".to_string(), format!("{}d", rule.older_than_days)]);
    }
    if !rule.subfolders {
        filters.extend(["--max-depth".to_string(), "1".to_string()]);
    }
    filters
}

fn sync_of(rule: &Rule, dry: bool) -> crate::rclone::Synced {
    let sync = crate::rclone::Sync { folder: rule.folder.trim(), target: rule.target.trim(), mode: &rule.sync, filters: sync_filters(rule), dry };
    crate::rclone::sync(&sync).unwrap_or_else(|e| crate::rclone::Synced { errors: vec![e], ..Default::default() })
}

/// Runs a sync rule now, and logs what it did.
fn run_sync(rule: Rule) {
    let synced = sync_of(&rule, false);
    let done = Done {
        id: 0,
        at: now_ms(),
        rule: rule.name.clone(),
        action: "sync".into(),
        from: rule.folder.clone(),
        to: Some(rule.target.clone()),
        mark: None,
        error: synced.errors.first().cloned(),
        undone: false,
        summary: Some(synced.summary()),
    };
    // Only a run that did something, or failed, goes in the log: a quiet one only says when it ran.
    let worth = done.error.is_some() || synced.up + synced.down + synced.deleted_here + synced.deleted_there > 0;
    {
        let mut engine = engine();
        let state = engine.syncs.entry(rule.id.clone()).or_default();
        state.running = false;
        state.last = Some(Instant::now());
        // What it changed in the folder isn't a change to sync again.
        state.changed = None;
    }
    if worth {
        record(done);
    }
}

/// Every two seconds: the sync rules that are due run (each on a thread of its own, one run at a time).
fn sync_tick(started: Instant) {
    let now = Instant::now();
    let due: Vec<Rule> = {
        let mut engine = engine();
        let rules: Vec<Rule> = engine.rules.iter().filter(|r| r.on && r.action == "sync").cloned().collect();
        let mut due = Vec::new();
        for rule in rules {
            let state = engine.syncs.entry(rule.id.clone()).or_default();
            if state.running {
                continue;
            }
            let settled = state.changed.is_some_and(|t| now.duration_since(t) >= SYNC_QUIET);
            let timed = rule.every > 0 && state.last.is_some_and(|t| now.duration_since(t) >= Duration::from_secs(u64::from(rule.every) * 60));
            let first = state.last.is_none() && now.duration_since(started) >= SYNC_FIRST;
            if settled || timed || first {
                state.running = true;
                state.changed = None;
                due.push(rule);
            }
        }
        due
    };
    for rule in due {
        std::thread::spawn(move || run_sync(rule));
    }
}

/// Moves `from` to `to`, or to "name (2)" and on when that's taken, across drives too: nothing's
/// replaced. Where it went.
fn move_anywhere(from: &Path, to: &Path) -> Result<PathBuf, String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_COPY_ALLOWED};
    let (Some(parent), Some(name)) = (to.parent(), to.file_name()) else { return Err("Where to?".into()) };
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let name = name.to_string_lossy();
    for n in 1..10_000 {
        let candidate = parent.join(crate::fsx::numbered(&name, false, n));
        let (a, b) = (crate::fsx::wide(from), crate::fsx::wide(&candidate));
        match unsafe { MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), MOVEFILE_COPY_ALLOWED) }.map_err(crate::fsx::io_error) {
            Ok(()) => return Ok(candidate),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(crate::fsx::file_error(e)),
        }
    }
    Err("Couldn't find a free name".into())
}

/// Does what `rule` says to the file at `path`: what became of it.
fn act(rule: &Rule, path: &Path, modified: i64) -> Done {
    let mut done = Done { id: 0, at: now_ms(), rule: rule.name.clone(), action: rule.action.clone(), from: path.to_string_lossy().into_owned(), to: None, mark: None, error: None, undone: false, summary: None };
    let result: Result<Option<String>, String> = (|| match rule.action.as_str() {
        "move" | "rename" => {
            let (_, to) = plan(rule, path, modified)?;
            let to = to.ok_or("Where to?")?;
            if to.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy()) {
                return Err("It's there already".into());
            }
            move_anywhere(path, &to).map(|p| Some(p.to_string_lossy().into_owned()))
        }
        "recycle" => {
            let performed = crate::shell::recycle(0, vec![path.to_string_lossy().into_owned()])?;
            Ok(performed.items.into_iter().next().and_then(|d| d.to))
        }
        "unblock" => {
            let stream = format!("{}:Zone.Identifier", path.display());
            let mark = std::fs::read_to_string(&stream).map_err(|_| "It isn't blocked".to_string())?;
            std::fs::remove_file(&stream).map_err(|e| e.to_string())?;
            done.mark = Some(mark);
            Ok(Some(done.from.clone()))
        }
        "extract" => {
            let quiet = tauri::ipc::Channel::new(|_| Ok(()));
            crate::archive::extract_beside(&done.from, &std::sync::atomic::AtomicBool::new(false), &quiet).map(|staged| Some(staged.folder))
        }
        other => Err(format!("Rules don't know “{other}”")),
    })();
    match result {
        Ok(to) => done.to = to,
        Err(e) => done.error = Some(e),
    }
    done
}

/// Logs what a rule did, tells the app (it says so, with Undo), and remembers what it made.
fn record(mut done: Done) -> Done {
    let mut engine = engine();
    engine.next += 1;
    done.id = engine.next;
    if let Some(to) = &done.to {
        engine.made.insert(to.to_lowercase(), Instant::now());
    }
    engine.log.push(done.clone());
    let excess = engine.log.len().saturating_sub(KEPT);
    engine.log.drain(..excess);
    save_log(&engine.log);
    if let Some(app) = &engine.app {
        let _ = app.emit("rule-did", &done);
    }
    done
}

/// The files `rule` takes in its folder now (up to `most`), walked as it looks.
fn matching(rule: &Rule, most: usize) -> Vec<(PathBuf, u64, i64)> {
    let now = now_ms();
    let mut found = Vec::new();
    let mut stack = vec![PathBuf::from(&rule.folder)];
    while let Some(dir) = stack.pop() {
        for entry in crate::fsx::read_dir(&dir.to_string_lossy()).unwrap_or_default() {
            let path = dir.join(&entry.name);
            // Not into links (a junction to elsewhere), and not into hidden or system folders.
            if entry.dir {
                if rule.subfolders && entry.attrs & 0x406 == 0 {
                    stack.push(path);
                }
            } else if takes(rule, &path, entry.size, entry.modified, now) {
                found.push((path, entry.size, entry.modified));
                if found.len() >= most {
                    return found;
                }
            }
        }
    }
    found
}

/// What's wrong with a rule, if anything, before it's turned on.
fn check(rule: &Rule) -> Result<(), String> {
    let folder = Path::new(rule.folder.trim());
    if !folder.is_absolute() || !folder.is_dir() {
        return Err(format!("“{}” isn't a folder", rule.folder));
    }
    if rule.action == "sync" {
        // "gdrive:Backups", not a folder on this PC ("D:\…").
        let remote = rule.target.trim().split_once(':').map(|(name, _)| name).filter(|name| name.len() > 1 && !name.contains(['\\', '/']));
        if remote.is_none() {
            return Err("Sync with which cloud? Choose one of rclone's".into());
        }
        if !matches!(rule.sync.as_str(), "copy" | "mirror" | "both") {
            return Err("Sync how? Copy, mirror or both ways".into());
        }
        if crate::proc::locate("rclone").is_none() {
            return Err("Syncing takes rclone: Settings → Tools installs it".into());
        }
    }
    if rule.action == "move" {
        let target = rule.target.trim().to_lowercase();
        let watched = rule.folder.trim().trim_end_matches('\\').to_lowercase();
        // A file moved where the rule looks would be taken again.
        if target == watched || (rule.subfolders && target.starts_with(&format!("{watched}\\"))) {
            return Err("It would move files to where it looks for them: choose a folder outside it (or don't look in subfolders)".into());
        }
    }
    plan(rule, &folder.join("example.txt"), now_ms()).map(|_| ())
}

/// Files that changed, from the watchers: each waits until it's complete.
fn seen(paths: Vec<PathBuf>) {
    let mut engine = engine();
    // A sync rule's folder changing: it runs once that stops (not while its own run is changing it).
    let syncing: Vec<(String, String)> = engine.rules.iter().filter(|r| r.on && r.action == "sync").map(|r| (r.id.clone(), r.folder.trim().trim_end_matches('\\').to_lowercase())).collect();
    for (id, folder) in syncing {
        if paths.iter().any(|p| p.to_string_lossy().to_lowercase().starts_with(&format!("{folder}\\"))) {
            let state = engine.syncs.entry(id).or_default();
            if !state.running {
                state.changed = Some(Instant::now());
            }
        }
    }
    for path in paths {
        let key = path.to_string_lossy().into_owned();
        if engine.made.get(&key.to_lowercase()).is_some_and(|t| t.elapsed() < MADE) {
            continue;
        }
        let size = std::fs::metadata(&path).ok().filter(|m| m.is_file()).map(|m| m.len());
        match size {
            Some(size) => {
                engine.pending.insert(key, (size, Instant::now()));
            }
            None => {
                engine.pending.remove(&key);
            }
        }
    }
}

/// Opens for reading while others may only read: a file still being written doesn't.
fn complete(path: &str) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).share_mode(1).open(path).is_ok()
}

/// Each half second: files that are complete now go to the first rule that takes them.
fn tick() {
    let ready: Vec<String> = {
        let mut engine = engine();
        let mut ready = Vec::new();
        let mut moved = Vec::new();
        for (path, (size, since)) in engine.pending.iter() {
            match std::fs::metadata(path) {
                Ok(m) if m.len() != *size => moved.push((path.clone(), m.len())),
                Ok(_) if since.elapsed() >= SETTLE => ready.push(path.clone()),
                Ok(_) => {}
                Err(_) => ready.push(path.clone()),
            }
        }
        for (path, size) in moved {
            engine.pending.insert(path, (size, Instant::now()));
        }
        engine.made.retain(|_, t| t.elapsed() < MADE);
        ready
    };
    for path in ready {
        if !complete(&path) {
            // Still open for writing: it waits on.
            if let Some(waiting) = engine().pending.get_mut(&path) {
                waiting.1 = Instant::now();
            }
            continue;
        }
        engine().pending.remove(&path);
        // What a rule made just now (its event can come before the rule says so) is left alone.
        if engine().made.get(&path.to_lowercase()).is_some_and(|t| t.elapsed() < MADE) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
        let file = Path::new(&path);
        let rule = engine().rules.iter().find(|r| r.on && r.action != "sync" && covers(r, file) && takes(r, file, meta.len(), modified, now_ms())).cloned();
        if let Some(rule) = rule {
            record(act(&rule, file, modified));
        }
    }
}

/// Every hour: what's grown old enough for the rules that wait on age.
fn sweep() {
    let rules: Vec<Rule> = engine().rules.iter().filter(|r| r.on && r.older_than_days > 0 && r.action != "sync").cloned().collect();
    for rule in rules {
        for (path, _, modified) in matching(&rule, 10_000) {
            record(act(&rule, &path, modified));
        }
    }
}

fn start(app: &AppHandle) {
    engine().app = Some(app.clone());
    STARTED.call_once(|| {
        if let Some(Ok(text)) = log_file().map(std::fs::read_to_string) {
            let log: Vec<Done> = serde_json::from_str(&text).unwrap_or_default();
            let mut engine = engine();
            engine.next = log.last().map_or(0, |d| d.id);
            engine.log = log;
        }
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_millis(500));
            tick();
        });
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_secs(3600));
            sweep();
        });
        let started = Instant::now();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(2));
            sync_tick(started);
        });
    });
}

/// Which `rules_set` is the latest: watching a network folder can take seconds, and an older set that
/// took longer mustn't win over a newer one.
static SETS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The rules the app has (sent at start and whenever they change): those turned on are watched. Their
/// watchers are made off the UI thread.
#[tauri::command]
pub async fn rules_set(app: AppHandle, rules: Vec<Rule>) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    let set = SETS.fetch_add(1, Ordering::SeqCst) + 1;
    crate::fsx::blocking(move || {
        set_rules(&app, rules, || SETS.load(Ordering::SeqCst) == set);
        Ok(())
    })
    .await
}

fn set_rules(app: &AppHandle, rules: Vec<Rule>, latest: impl Fn() -> bool) {
    start(app);
    let mut watchers = Vec::new();
    let mut folders = HashSet::new();
    for rule in rules.iter().filter(|r| r.on && check(r).is_ok()) {
        if !folders.insert((rule.folder.to_lowercase(), rule.subfolders)) {
            continue;
        }
        let watcher = notify::recommended_watcher(|event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                if !matches!(event.kind, notify::EventKind::Remove(_) | notify::EventKind::Access(_)) {
                    seen(event.paths);
                }
            }
        });
        if let Ok(mut watcher) = watcher {
            let mode = if rule.subfolders { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
            if watcher.watch(Path::new(&rule.folder), mode).is_ok() {
                watchers.push(watcher);
            }
        }
    }
    let mut engine = engine();
    if latest() {
        engine.watchers = watchers;
        engine.rules = rules;
    }
}

/// What `rule` would do now to what's in its folder (up to 500 files), or what's wrong with it. A sync
/// rule's is rclone's dry run: each file it would copy or delete, and which way.
#[tauri::command]
pub async fn rules_preview(rule: Rule) -> Result<Vec<Done>, String> {
    crate::fsx::blocking(move || {
        check(&rule)?;
        if rule.action == "sync" {
            let synced = sync_of(&rule, true);
            if let Some(error) = synced.errors.first() {
                return Err(error.clone());
            }
            return Ok(synced
                .items
                .into_iter()
                .map(|(what, path)| Done { id: 0, at: 0, rule: rule.name.clone(), action: what, from: path, to: None, mark: None, error: None, undone: false, summary: None })
                .collect());
        }
        Ok(matching(&rule, 500)
            .into_iter()
            .map(|(path, _, modified)| {
                let (what, to) = plan(&rule, &path, modified).unwrap_or_else(|e| (e, None));
                Done { id: 0, at: 0, rule: rule.name.clone(), action: what, from: path.to_string_lossy().into_owned(), to: to.map(|p| p.to_string_lossy().into_owned()), mark: None, error: None, undone: false, summary: None }
            })
            .collect())
    })
    .await
}

/// Does what `rule` says to what's in its folder now (as it's turned on): how many it did. A sync rule
/// starts its first run, which logs what it does.
#[tauri::command]
pub async fn rules_apply(rule: Rule) -> Result<usize, String> {
    crate::fsx::blocking(move || {
        check(&rule)?;
        if rule.action == "sync" {
            let start = {
                let mut engine = engine();
                let state = engine.syncs.entry(rule.id.clone()).or_default();
                !std::mem::replace(&mut state.running, true)
            };
            if start {
                std::thread::spawn(move || run_sync(rule));
            }
            return Ok(0);
        }
        Ok(matching(&rule, 10_000).into_iter().map(|(path, _, modified)| record(act(&rule, &path, modified))).filter(|d| d.error.is_none()).count())
    })
    .await
}

#[tauri::command]
pub fn rules_log() -> Vec<Done> {
    engine().log.clone()
}

#[tauri::command]
pub fn rules_clear_log() {
    let mut engine = engine();
    engine.log.clear();
    save_log(&engine.log);
}

/// Takes back what a rule did to one file: moved back, restored from the Recycle Bin, blocked again, or
/// what was extracted put in the Recycle Bin.
#[tauri::command]
pub async fn rules_undo(id: u64) -> Result<(), String> {
    crate::fsx::blocking(move || {
        let done = engine().log.iter().find(|d| d.id == id).cloned().ok_or("That's not in the log any more")?;
        if done.undone || done.error.is_some() {
            return Err("There's nothing to take back".into());
        }
        let to = done.to.clone().ok_or("There's nothing to take back")?;
        match done.action.as_str() {
            "move" | "rename" => {
                if Path::new(&done.from).exists() {
                    return Err(format!("Something's at “{}” again", done.from));
                }
                let (a, b) = (Path::new(&to), Path::new(&done.from));
                engine().made.insert(done.from.to_lowercase(), Instant::now());
                move_anywhere(a, b).map(|_| ())?;
            }
            "recycle" => {
                let from = Path::new(&done.from);
                let (Some(folder), Some(name)) = (from.parent(), from.file_name()) else { return Err("Where it was isn't known".into()) };
                engine().made.insert(done.from.to_lowercase(), Instant::now());
                crate::shell::restore(vec![crate::shell::Back { path: to, folder: folder.to_string_lossy().into_owned(), name: name.to_string_lossy().into_owned() }])?;
            }
            "unblock" => {
                let mark = done.mark.clone().ok_or("The mark that was taken off isn't known")?;
                std::fs::write(format!("{}:Zone.Identifier", done.from), mark).map_err(|e| e.to_string())?;
            }
            "extract" => {
                crate::shell::recycle(0, vec![to])?;
            }
            _ => return Err("There's nothing to take back".into()),
        }
        let mut engine = engine();
        if let Some(entry) = engine.log.iter_mut().find(|d| d.id == id) {
            entry.undone = true;
        }
        save_log(&engine.log);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncs_what_it_takes() {
        let r = Rule { action: "sync".into(), target: "gd:Backups".into(), sync: "copy".into(), names: "*.pdf; scan*".into(), min_size: 1024, older_than_days: 2, ..rule("") };
        assert_eq!(sync_filters(&r), ["--ignore-case", "--include", "*.pdf", "--include", "scan*", "--min-size", "1024B", "--min-age", "2d", "--max-depth", "1"]);
        assert!(sync_filters(&Rule { subfolders: true, names: String::new(), min_size: 0, older_than_days: 0, ..r.clone() }).is_empty());
        let mut checked = Rule { folder: std::env::temp_dir().to_string_lossy().into(), ..r.clone() };
        checked.target = r"D:\Backups".into();
        assert!(check(&checked).unwrap_err().starts_with("Sync with which cloud"));
        checked.target = "gd:Backups".into();
        checked.sync = "sideways".into();
        assert!(check(&checked).unwrap_err().starts_with("Sync how"));
        assert_eq!(plan(&r, Path::new(r"C:\Downloads\a.pdf"), 0).unwrap().0, "Sync with gd:Backups");
    }

    fn rule(names: &str) -> Rule {
        Rule { id: "r".into(), name: "R".into(), folder: r"C:\Downloads".into(), names: names.into(), action: "move".into(), target: r"D:\Docs\{year}".into(), ..Default::default() }
    }

    #[test]
    fn takes_what_it_says() {
        let now = 1_720_953_005_000;
        let r = rule("*.pdf; invoice*");
        assert!(takes(&r, Path::new(r"C:\Downloads\Report.PDF"), 10, now, now));
        assert!(takes(&r, Path::new(r"C:\Downloads\invoice 12.txt"), 10, now, now));
        assert!(!takes(&r, Path::new(r"C:\Downloads\photo.jpg"), 10, now, now));
        assert!(takes(&rule(""), Path::new(r"C:\Downloads\anything"), 10, now, now));
        let sized = Rule { min_size: 100, max_size: 1000, ..rule("") };
        assert!(!takes(&sized, Path::new(r"C:\Downloads\a"), 99, now, now) && takes(&sized, Path::new(r"C:\Downloads\a"), 100, now, now) && !takes(&sized, Path::new(r"C:\Downloads\a"), 1001, now, now));
        let old = Rule { older_than_days: 30, ..rule("") };
        assert!(!takes(&old, Path::new(r"C:\Downloads\a"), 1, now - 29 * 86_400_000, now) && takes(&old, Path::new(r"C:\Downloads\a"), 1, now - 31 * 86_400_000, now));
        assert!(covers(&r, Path::new(r"c:\downloads\a.pdf")) && !covers(&r, Path::new(r"C:\Downloads\sub\a.pdf")));
        assert!(covers(&Rule { subfolders: true, ..r.clone() }, Path::new(r"C:\Downloads\sub\a.pdf")));
        assert!(!covers(&Rule { subfolders: true, ..r }, Path::new(r"C:\Downloads2\a.pdf")));
        assert!(wild(&"a?c*".chars().collect::<Vec<_>>(), &"abcdef".chars().collect::<Vec<_>>()));
        // A rename doesn't take its own work again.
        let dated = Rule { action: "rename".into(), names: "*.txt".into(), pattern: "{date} {name}{ext}".into(), ..rule("") };
        assert!(takes(&dated, Path::new(r"C:\Downloads\memo.txt"), 1, now, now));
        assert!(!takes(&dated, Path::new(r"C:\Downloads\2026-10-24 memo.txt"), 1, now, now));
        assert!(renamed_already("{name}-old{ext}", "x-old.txt") && !renamed_already("{name}-old{ext}", "x.txt"));
        assert!(renamed_already("archive (1).zip", "ARCHIVE (1).zip") && !renamed_already("archive (1).zip", "archive 1.zip"));
    }

    #[test]
    fn plans_with_the_files_date() {
        let modified = 1_720_953_005_000;
        let (year, month, day, ..) = local(modified);
        let r = rule("*.pdf");
        let (what, to) = plan(&r, Path::new(r"C:\Downloads\a.pdf"), modified).unwrap();
        assert_eq!(to.unwrap(), PathBuf::from(format!(r"D:\Docs\{year:04}\a.pdf")));
        assert!(what.starts_with("Move into D:\\Docs\\"));
        let renamed = Rule { action: "rename".into(), pattern: "{date} {name}{ext}".into(), ..r.clone() };
        assert_eq!(plan(&renamed, Path::new(r"C:\Downloads\scan.pdf"), modified).unwrap().1.unwrap(), PathBuf::from(format!(r"C:\Downloads\{year:04}-{month:02}-{day:02} scan.pdf")));
        assert!(plan(&Rule { pattern: "a/b".into(), ..renamed }, Path::new(r"C:\Downloads\x"), modified).is_err());
        assert!(plan(&Rule { target: "relative".into(), ..r }, Path::new(r"C:\Downloads\x"), modified).is_err());
    }

    #[test]
    fn acts_and_takes_it_back() {
        let dir = std::env::temp_dir().join(format!("glint-rules-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("in")).unwrap();
        std::fs::write(dir.join("in").join("a.pdf"), "pdf").unwrap();
        std::fs::create_dir_all(dir.join("out")).unwrap();
        std::fs::write(dir.join("out").join("a.pdf"), "already there").unwrap();
        let r = Rule { folder: dir.join("in").to_string_lossy().into(), target: dir.join("out").to_string_lossy().into(), ..rule("*.pdf") };
        assert!(check(&r).is_ok());
        assert!(check(&Rule { target: dir.join("in").to_string_lossy().into(), ..r.clone() }).is_err());
        assert_eq!(matching(&r, 10).len(), 1);
        let done = act(&r, &dir.join("in").join("a.pdf"), 0);
        // Nothing's replaced: it's "a (2).pdf" there.
        assert_eq!(done.to.as_deref(), Some(&*dir.join("out").join("a (2).pdf").to_string_lossy()));
        assert_eq!(std::fs::read_to_string(dir.join("out").join("a.pdf")).unwrap(), "already there");
        let done = record(done);
        tauri::async_runtime::block_on(rules_undo(done.id)).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("in").join("a.pdf")).unwrap(), "pdf");
        assert!(engine().log.iter().any(|d| d.id == done.id && d.undone));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
