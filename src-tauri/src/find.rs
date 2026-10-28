//! Find files by name: from the instant search index when its service runs (whole drives in
//! milliseconds), otherwise with the bundled fd, which also covers what the index can't (drives it
//! doesn't index, .gitignore rules). Results stream in either way, each with its size, date and
//! attributes.

use crate::fsx::{encode_items, entry_for_path, ms_since, Entry};
use crate::proc::Slot;
use crate::AppState;
use serde::{Deserialize, Serialize};
use shadui_index::query::{parse, without_props, Kind};
use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FindOptions {
    /// Include hidden and system files.
    pub hidden: bool,
    /// Respect .gitignore and .ignore files.
    pub respect_ignore: bool,
    /// Names or globs to skip, e.g. node_modules.
    pub exclude: Vec<String>,
    pub max_results: Option<usize>,
    /// When nothing matches as typed, the index looks again with typos allowed (a word an edit or two
    /// from the start of a name's word); fd doesn't.
    #[serde(default)]
    pub fuzzy: bool,
}

/// What a search says besides its results, as JSON; the results themselves go as bytes (`encode_items`).
#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FindEvent {
    /// Who answers: "index", "fd", or "index + fd" when the index covers only some of the folders.
    Started { engine: &'static str },
    Error { message: String },
    /// `fuzzy`: nothing matched as typed, and the index found these close matches.
    #[serde(rename_all = "camelCase")]
    Finished { count: usize, elapsed: f64, limited: bool, fuzzy: bool },
}

fn tell(channel: &Channel, event: FindEvent) {
    if let Ok(json) = serde_json::to_string(&event) {
        let _ = channel.send(InvokeResponseBody::Json(json));
    }
}

/// The searches in flight, one per lane (each tab searches on its own), so a newer search in the
/// same tab, or Stop, can end one.
#[derive(Clone, Default)]
pub struct Finds(Arc<Mutex<HashMap<String, Run>>>);

/// One search: the flag that stops the index's answer, and fd's process.
#[derive(Clone, Default)]
struct Run {
    stop: Arc<AtomicBool>,
    fd: Slot,
}

impl Run {
    fn end(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.fd.kill();
    }
}

impl Finds {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Run>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts a search in `lane`, ending the one before it there.
    fn start(&self, lane: &str) -> Run {
        let run = Run::default();
        let previous = self.lock().insert(lane.to_string(), run.clone());
        if let Some(previous) = previous {
            previous.end();
        }
        run
    }

    /// Ends the lane's search, if one runs.
    fn stop(&self, lane: &str) {
        let run = self.lock().remove(lane);
        if let Some(run) = run {
            run.end();
        }
    }

    /// Ends every lane's search.
    pub fn stop_all(&self) {
        let runs: Vec<Run> = self.lock().drain().map(|(_, run)| run).collect();
        for run in runs {
            run.end();
        }
    }

    /// Forgets `run` once it's done, unless a newer search in the lane replaced it.
    fn finished(&self, lane: &str, run: &Run) {
        let mut runs = self.lock();
        if runs.get(lane).is_some_and(|current| Arc::ptr_eq(&current.stop, &run.stop)) {
            runs.remove(lane);
        }
    }
}

/// fd arguments for a query: every term must match the file name (like Everything), case-insensitively.
/// Terms with * or ? are globs over the whole name; the rest match anywhere in it.
fn fd_args(query: &str, roots: &[String], options: &FindOptions) -> Result<Vec<String>, String> {
    let (terms, filters) = parse(query)?;
    if terms.is_empty() && filters.is_empty() {
        return Err("Type part of a file name to search for".into());
    }
    // Filters alone search every name.
    let glob = terms.is_empty() || terms.iter().any(|t| t.contains(['*', '?']));
    let mut patterns = terms.into_iter().map(|t| if glob && !t.contains(['*', '?']) { format!("*{t}*") } else { t });

    let mut args: Vec<String> = ["--color=never", "--absolute-path", "--ignore-case"].map(String::from).into();
    args.push(if glob { "--glob" } else { "--fixed-strings" }.into());
    if options.hidden {
        args.push("--hidden".into());
    }
    if !options.respect_ignore {
        args.push("--no-ignore".into());
    }
    for exclude in options.exclude.iter().filter(|e| !e.trim().is_empty()) {
        args.push(format!("--exclude={}", exclude.trim()));
    }
    if let Some(max) = options.max_results.filter(|&n| n > 0) {
        args.push(format!("--max-results={max}"));
    }
    // The index's filters as fd's flags: its --size counts files only, as the index does.
    args.extend(filters.exts.iter().map(|ext| format!("--extension={ext}")));
    args.extend(filters.min_size.map(|min| format!("--size=+{min}b")));
    args.extend(filters.max_size.map(|max| format!("--size=-{max}b")));
    args.extend(filters.newer_than.map(|secs| format!("--changed-within={secs}s")));
    args.extend(filters.older_than.map(|secs| format!("--changed-before={secs}s")));
    if filters.files_only() {
        args.push("--type=file".into());
    } else if filters.kind == Some(Kind::Folders) {
        args.push("--type=directory".into());
    }
    let first = patterns.next().unwrap_or_else(|| "*".into());
    args.extend(patterns.map(|p| format!("--and={p}")));
    args.push("--".into());
    args.push(first);
    args.extend(roots.iter().cloned());
    Ok(args)
}

/// Sends results in batches: up to 10,000 at a time, at least every 40 ms.
struct Out<'a> {
    channel: &'a Channel,
    batch: Vec<Entry>,
    sent: Instant,
    count: usize,
    started: bool,
    /// With property filters (width:, taken:…): what's found goes to be read first (`check`), not
    /// straight to the app.
    check: Option<mpsc::SyncSender<Entry>>,
}

impl<'a> Out<'a> {
    fn new(channel: &'a Channel, check: Option<mpsc::SyncSender<Entry>>) -> Self {
        Out { channel, batch: Vec::new(), sent: Instant::now(), count: 0, started: false, check }
    }

    fn started(&mut self, engine: &'static str) {
        if !std::mem::replace(&mut self.started, true) {
            tell(self.channel, FindEvent::Started { engine });
        }
    }

    fn push(&mut self, entry: Entry) {
        self.count += 1;
        if let Some(check) = &self.check {
            let _ = check.send(entry);
            return;
        }
        self.batch.push(entry);
        if self.batch.len() >= 10_000 || self.sent.elapsed() >= Duration::from_millis(40) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.batch.is_empty() {
            let _ = self.channel.send(InvokeResponseBody::Raw(encode_items("", &self.batch, &[], 0.0, false)));
            self.batch.clear();
            self.sent = Instant::now();
        }
    }
}

/// What the index answered: the folders it couldn't (searched with fd instead), whether it stopped at
/// the result limit, and whether these are close matches.
struct Answered {
    skipped: Vec<String>,
    limited: bool,
    fuzzy: bool,
}

/// Asks the index service. None when it isn't running, isn't allowed, or covers none of `roots`.
#[cfg(windows)]
fn search_index(roots: &[String], query: &str, options: &FindOptions, stop: &AtomicBool, out: &mut Out) -> Option<Answered> {
    use shadui_index::client::{self, Event};
    let request = shadui_index::proto::SearchRequest {
        query: query.to_string(),
        roots: roots.to_vec(),
        hidden: options.hidden,
        exclude: options.exclude.clone(),
        max_results: options.max_results,
        fuzzy: options.fuzzy,
    };
    let mut answered = None;
    // A failure halfway keeps what arrived; one before the plan leaves everything to fd.
    let _ = client::search(&request, |event| {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        match event {
            Event::Plan { indexed, skipped } => {
                if indexed.is_empty() {
                    return false;
                }
                out.started(if skipped.is_empty() { "index" } else { "index + fd" });
                answered = Some(Answered { skipped, limited: false, fuzzy: false });
            }
            Event::Hits(hits) => {
                for hit in hits {
                    let name = hit.path.rsplit('\\').next().unwrap_or(&hit.path).to_string();
                    out.push(Entry { name, path: Some(hit.path), dir: hit.dir, size: hit.size, modified: hit.modified, attrs: hit.attrs });
                }
            }
            Event::Done { limited, fuzzy, .. } => {
                if let Some(answered) = answered.as_mut() {
                    (answered.limited, answered.fuzzy) = (limited, fuzzy);
                }
            }
        }
        true
    });
    answered
}

/// Whether the running service is this app's version (so it reads every filter this app knows).
#[cfg(windows)]
pub(crate) fn same_service() -> bool {
    shadui_index::client::status().is_ok_and(|status| status.version == shadui_index::VERSION)
}

#[cfg(not(windows))]
pub(crate) fn same_service() -> bool {
    false
}

#[cfg(not(windows))]
fn search_index(_: &[String], _: &str, _: &FindOptions, _: &AtomicBool, _: &mut Out) -> Option<Answered> {
    None
}

/// Searches `roots` with fd, streaming what it finds. Returns whether it hit `options.max_results`.
fn search_fd(slot: &Slot, roots: &[String], query: &str, options: &FindOptions, out: &mut Out) -> Result<bool, String> {
    let args = fd_args(query, roots, options)?;
    let mut command = crate::proc::tool("fd")?;
    command
        .args(&args)
        // fd prints "/" separators when it thinks it runs under MSYS (e.g. a dev build started from Git Bash).
        .env_remove("MSYSTEM")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("Failed to start fd: {e}"))?;
    let stdout = child.stdout.take().ok_or("Failed to open fd's output")?;
    let mut stderr = child.stderr.take().ok_or("Failed to open fd's errors")?;
    let pid = slot.replace(child);

    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let before = out.count;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        // fd marks folders with a trailing separator.
        let dir = line.ends_with(['\\', '/']);
        let path = line.trim_end_matches(['\\', '/']);
        if !path.is_empty() {
            out.push(entry_for_path(path, dir));
        }
    }
    let found = out.count - before;
    let errors = stderr_reader.join().unwrap_or_default();
    // A stopped search ends quietly, and so do warnings about folders fd can't open (it still exits
    // 0); only a run that failed (a missing folder, a bad pattern) reports what fd said.
    if slot.finish(pid).is_some_and(|status| !status.success()) && found == 0 && !errors.trim().is_empty() {
        return Err(errors.trim().to_string());
    }
    Ok(options.max_results.is_some_and(|max| max > 0 && found >= max))
}

/// Searches `roots` by name for the tab `lane`, stopping that tab's previous search (another tab's
/// keeps going).
#[tauri::command]
pub async fn find_files(
    state: State<'_, AppState>,
    roots: Vec<String>,
    query: String,
    options: FindOptions,
    on_event: Channel,
    lane: String,
) -> Result<(), String> {
    let (terms, filters) = parse(&query)?;
    if terms.is_empty() && filters.is_empty() {
        return Err("Type part of a file name to search for".into());
    }
    // What photos, videos and songs say of themselves (width:, taken:, artist:…) is read here: the
    // index and fd get the query without it, narrowed to the files that can say it.
    let props = filters.props.clone();
    let (query, filters) = if props.is_empty() {
        (query, filters)
    } else {
        let exts = crate::media::exts_for(&props);
        if exts.is_empty() {
            return Err("No file is all of that: width:, height:, taken: and camera: go with photos and videos, artist: and album: with songs, duration: with videos and songs.".into());
        }
        let mut narrowed = without_props(&query);
        if filters.exts.is_empty() {
            narrowed = format!("{narrowed} ext:{}", exts.join(";")).trim().to_string();
        }
        let filters = parse(&narrowed)?.1;
        (narrowed, filters)
    };
    let run = state.finds.start(&lane);
    let finds = state.finds.clone();
    thread::spawn(move || {
        // An older service would read ext: and the like as part of a name, and find nothing: until it's
        // updated, fd answers queries with filters. Asking it can wait on its pipe, so it's asked here.
        let index_reads_filters = filters.is_empty() || same_service();
        let (stop, slot) = (&run.stop, &run.fd);
        let started = Instant::now();
        // With properties to check, the index and fd find every candidate, which four threads read
        // (each file once a session): the limit is on what holds them.
        let wanted = options.max_results.filter(|&m| m > 0);
        let checking = !props.is_empty();
        let options = if checking { FindOptions { max_results: None, ..options } } else { options };
        let (to_check, checks) = mpsc::sync_channel::<Entry>(1024);
        let checks = Mutex::new(checks);
        let held = AtomicUsize::new(0);
        let (count, limited, fuzzy) = thread::scope(|scope| {
            for _ in 0..if checking { 4 } else { 0 } {
                let channel = on_event.clone();
                let (checks, held, props) = (&checks, &held, &props);
                scope.spawn(move || {
                    let mut out = Out::new(&channel, None);
                    out.started = true;
                    let next = || checks.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    while let Ok(entry) = next() {
                        // Stopped: what's left is passed over, so what finds it never waits.
                        if stop.load(Ordering::SeqCst) || !entry.path.as_deref().is_some_and(|path| crate::media::matches(path, entry.size, entry.modified, props)) {
                            continue;
                        }
                        if wanted.is_some_and(|w| held.fetch_add(1, Ordering::SeqCst) >= w) {
                            // The limit: the index and fd stop too.
                            stop.store(true, Ordering::SeqCst);
                            slot.kill();
                            continue;
                        }
                        if wanted.is_none() {
                            held.fetch_add(1, Ordering::SeqCst);
                        }
                        out.push(entry);
                    }
                    out.flush();
                });
            }
            let mut out = Out::new(&on_event, checking.then_some(to_check));
            let mut fd_roots = roots.clone();
            let (mut limited, mut fuzzy) = (false, false);
            // A folder in an archive: searched in the archive's own list of entries.
            if let [root] = roots.as_slice() {
                let searched = crate::archive::search(root, &query, options.hidden, &options.exclude, options.max_results, stop, &mut |entry| {
                    if !checking {
                        out.push(entry)
                    }
                });
                if let Some(searched) = searched {
                    match searched {
                        Ok(_) if checking => tell(&on_event, FindEvent::Error { message: "Photo, video and music filters don't look inside archives: they read files on disk.".into() }),
                        Ok((engine, hit_limit)) => {
                            out.started(engine);
                            limited = hit_limit;
                        }
                        Err(message) => {
                            out.flush();
                            tell(&on_event, FindEvent::Error { message });
                        }
                    }
                    out.flush();
                    return (out.count, limited, false);
                }
            }
            // .gitignore rules are fd's alone; anything else goes to the index first.
            if !options.respect_ignore && index_reads_filters {
                if let Some(answered) = search_index(&roots, &query, &options, stop, &mut out) {
                    (fd_roots, limited, fuzzy) = (answered.skipped, answered.limited, answered.fuzzy);
                }
            }
            let left = options.max_results.filter(|&m| m > 0).map(|m| m.saturating_sub(out.count));
            if !fd_roots.is_empty() && !stop.load(Ordering::SeqCst) && left != Some(0) {
                out.started("fd");
                let options = FindOptions { max_results: left, ..options };
                match search_fd(slot, &fd_roots, &query, &options, &mut out) {
                    Ok(hit_limit) => limited |= hit_limit,
                    Err(message) => {
                        out.flush();
                        tell(&on_event, FindEvent::Error { message });
                    }
                }
            } else if !fd_roots.is_empty() {
                limited = true;
            }
            out.flush();
            (out.count, limited, fuzzy)
        });
        // Checked: as many as held, up to the limit.
        let (count, limited) = if checking {
            let held = held.into_inner();
            (wanted.map_or(held, |w| held.min(w)), wanted.is_some_and(|w| held > w))
        } else {
            (count, limited)
        };
        tell(&on_event, FindEvent::Finished { count, elapsed: ms_since(started), limited, fuzzy });
        finds.finished(&lane, &run);
    });
    Ok(())
}

/// Stops the tab's search (Stop, a new folder, or the tab closed).
#[tauri::command]
pub fn cancel_find(state: State<AppState>, lane: String) {
    state.finds.stop(&lane);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> FindOptions {
        FindOptions { hidden: false, respect_ignore: false, exclude: vec!["node_modules".into(), " ".into()], max_results: Some(100), fuzzy: false }
    }

    #[test]
    fn builds_fixed_string_and_glob_queries() {
        let roots = vec![r"C:\Users".to_string()];
        let args = fd_args("budget 2026", &roots, &options()).unwrap();
        assert!(args.contains(&"--fixed-strings".into()));
        assert!(args.contains(&"--no-ignore".into()));
        assert!(args.contains(&"--exclude=node_modules".into()));
        assert!(args.contains(&"--max-results=100".into()));
        assert!(args.ends_with(&["--and=2026".into(), "--".into(), "budget".into(), r"C:\Users".into()]));

        let args = fd_args("*.pdf -draft", &roots, &options()).unwrap();
        assert!(args.contains(&"--glob".into()));
        assert!(args.ends_with(&["--and=*-draft*".into(), "--".into(), "*.pdf".into(), r"C:\Users".into()]));
        assert!(fd_args("  ", &roots, &options()).is_err());

        let args = fd_args("ext:pdf size:>1kb dm:7d", &roots, &options()).unwrap();
        for flag in ["--glob", "--extension=pdf", "--size=+1025b", "--changed-within=604800s", "--type=file"] {
            assert!(args.contains(&flag.into()), "{flag}");
        }
        assert!(args.ends_with(&["--".into(), "*".into(), r"C:\Users".into()]));
        assert!(fd_args("report folder: dm:>1y", &roots, &options()).unwrap().contains(&"--type=directory".into()));
        assert!(fd_args("size:huge", &roots, &options()).is_err());
    }

    #[test]
    fn a_new_search_stops_only_the_last_one_in_its_tab() {
        let finds = Finds::default();
        let stopped = |run: &Run| run.stop.load(Ordering::SeqCst);
        let first = finds.start("left");
        let other = finds.start("right");
        let second = finds.start("left");
        assert!(stopped(&first) && !stopped(&second) && !stopped(&other));

        // A search that ended by itself is forgotten, but never one that replaced it.
        finds.finished("left", &first);
        finds.stop("left");
        assert!(stopped(&second) && !stopped(&other));
        finds.finished("right", &other);
        assert!(finds.lock().is_empty());
        finds.stop("right");
        assert!(!stopped(&other));
    }
}
