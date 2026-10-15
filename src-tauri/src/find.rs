//! Find files by name: from the instant search index when its service runs (whole drives in
//! milliseconds), otherwise with the bundled fd, which also covers what the index can't (drives it
//! doesn't index, .gitignore rules). Results stream in either way, each with its size, date and
//! attributes.

use crate::fsx::{entry_for_path, ms_since, Entry};
use crate::proc::Slot;
use crate::AppState;
use serde::{Deserialize, Serialize};
use shadui_index::query::terms;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{State, Window};
use tauri_plugin_shell::ShellExt;

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
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FindEvent {
    /// Who answers: "index", "fd", or "index + fd" when the index covers only some of the folders.
    Started { engine: &'static str },
    Entries { entries: Vec<Entry> },
    Error { message: String },
    #[serde(rename_all = "camelCase")]
    Finished { count: usize, elapsed: f64, limited: bool },
}

/// The search in flight, so a newer one (or Stop) can end it.
#[derive(Clone, Default)]
pub struct Current(Arc<Mutex<Arc<AtomicBool>>>);

impl Current {
    /// Starts a search, telling the previous one to stop; returns the new one's stop flag.
    fn start(&self) -> Arc<AtomicBool> {
        let stop = Arc::new(AtomicBool::new(false));
        let previous = std::mem::replace(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()), stop.clone());
        previous.store(true, Ordering::SeqCst);
        stop
    }

    fn stop(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).store(true, Ordering::SeqCst);
    }
}

/// fd arguments for a query: every term must match the file name (like Everything), case-insensitively.
/// Terms with * or ? are globs over the whole name; the rest match anywhere in it.
fn fd_args(query: &str, roots: &[String], options: &FindOptions) -> Result<Vec<String>, String> {
    let terms = terms(query);
    if terms.is_empty() {
        return Err("Type part of a file name to search for".into());
    }
    let glob = terms.iter().any(|t| t.contains(['*', '?']));
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
    let first = patterns.next().unwrap();
    args.extend(patterns.map(|p| format!("--and={p}")));
    args.push("--".into());
    args.push(first);
    args.extend(roots.iter().cloned());
    Ok(args)
}

/// Sends results in batches: up to 10,000 at a time, at least every 40 ms.
struct Out<'a> {
    channel: &'a Channel<FindEvent>,
    batch: Vec<Entry>,
    sent: Instant,
    count: usize,
    started: bool,
}

impl Out<'_> {
    fn started(&mut self, engine: &'static str) {
        if !std::mem::replace(&mut self.started, true) {
            let _ = self.channel.send(FindEvent::Started { engine });
        }
    }

    fn push(&mut self, entry: Entry) {
        self.batch.push(entry);
        self.count += 1;
        if self.batch.len() >= 10_000 || self.sent.elapsed() >= Duration::from_millis(40) {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if !self.batch.is_empty() {
            let _ = self.channel.send(FindEvent::Entries { entries: std::mem::take(&mut self.batch) });
            self.sent = Instant::now();
        }
    }
}

/// What the index answered: the folders it couldn't (searched with fd instead), and whether it
/// stopped at the result limit.
struct Answered {
    skipped: Vec<String>,
    limited: bool,
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
                answered = Some(Answered { skipped, limited: false });
            }
            Event::Hits(hits) => {
                for hit in hits {
                    let name = hit.path.rsplit('\\').next().unwrap_or(&hit.path).to_string();
                    out.push(Entry { name, path: Some(hit.path), dir: hit.dir, size: hit.size, modified: hit.modified, attrs: hit.attrs });
                }
            }
            Event::Done { limited, .. } => {
                if let Some(answered) = answered.as_mut() {
                    answered.limited = limited;
                }
            }
        }
        true
    });
    answered
}

#[cfg(not(windows))]
fn search_index(_: &[String], _: &str, _: &FindOptions, _: &AtomicBool, _: &mut Out) -> Option<Answered> {
    None
}

/// Searches `roots` with fd, streaming what it finds. Returns whether it hit `options.max_results`.
fn search_fd(window: &Window, slot: &Slot, roots: &[String], query: &str, options: &FindOptions, out: &mut Out) -> Result<bool, String> {
    let args = fd_args(query, roots, options)?;
    let mut command: Command = window.shell().sidecar("fd").map_err(|e| e.to_string())?.into();
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
    // A stopped search ends quietly; only a run that finished by itself reports fd's complaints.
    if slot.finish(pid) && found == 0 && !errors.trim().is_empty() {
        return Err(errors.trim().to_string());
    }
    Ok(options.max_results.is_some_and(|max| max > 0 && found >= max))
}

#[tauri::command]
pub async fn find_files(
    window: Window,
    state: State<'_, AppState>,
    roots: Vec<String>,
    query: String,
    options: FindOptions,
    on_event: Channel<FindEvent>,
) -> Result<(), String> {
    if terms(&query).is_empty() {
        return Err("Type part of a file name to search for".into());
    }
    let stop = state.finding.start();
    state.find.kill();
    let slot = state.find.clone();
    thread::spawn(move || {
        let started = Instant::now();
        let mut out = Out { channel: &on_event, batch: Vec::new(), sent: Instant::now(), count: 0, started: false };
        let mut fd_roots = roots.clone();
        let mut limited = false;
        // .gitignore rules are fd's alone; anything else goes to the index first.
        if !options.respect_ignore {
            if let Some(answered) = search_index(&roots, &query, &options, &stop, &mut out) {
                (fd_roots, limited) = (answered.skipped, answered.limited);
            }
        }
        let left = options.max_results.filter(|&m| m > 0).map(|m| m.saturating_sub(out.count));
        if !fd_roots.is_empty() && !stop.load(Ordering::SeqCst) && left != Some(0) {
            out.started("fd");
            let options = FindOptions { max_results: left, ..options };
            match search_fd(&window, &slot, &fd_roots, &query, &options, &mut out) {
                Ok(hit_limit) => limited |= hit_limit,
                Err(message) => {
                    out.flush();
                    let _ = on_event.send(FindEvent::Error { message });
                }
            }
        } else if !fd_roots.is_empty() {
            limited = true;
        }
        out.flush();
        let _ = on_event.send(FindEvent::Finished { count: out.count, elapsed: ms_since(started), limited });
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_find(state: State<AppState>) {
    state.finding.stop();
    state.find.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> FindOptions {
        FindOptions { hidden: false, respect_ignore: false, exclude: vec!["node_modules".into(), " ".into()], max_results: Some(100) }
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
    }

    #[test]
    fn a_new_search_stops_the_last_one() {
        let current = Current::default();
        let first = current.start();
        let second = current.start();
        assert!(first.load(Ordering::SeqCst) && !second.load(Ordering::SeqCst));
        current.stop();
        assert!(second.load(Ordering::SeqCst));
    }
}
