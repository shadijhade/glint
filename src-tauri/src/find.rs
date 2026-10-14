//! Find files by name with the bundled fd: streams results, each with its size, date and attributes.

use crate::fsx::{entry_for_path, ms_since, Entry};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{State, Window};
use tauri_plugin_shell::ShellExt;

#[derive(Deserialize)]
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
    Entries { entries: Vec<Entry> },
    Error { message: String },
    #[serde(rename_all = "camelCase")]
    Finished { count: usize, elapsed: f64, limited: bool },
}

/// Splits a query into terms; "double quotes" keep spaces in one term.
fn terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    terms.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
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

#[tauri::command]
pub async fn find_files(
    window: Window,
    state: State<'_, AppState>,
    roots: Vec<String>,
    query: String,
    options: FindOptions,
    on_event: Channel<FindEvent>,
) -> Result<(), String> {
    let args = fd_args(&query, &roots, &options)?;
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
    let slot = state.find.clone();
    let pid = slot.replace(child);

    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    thread::spawn(move || {
        let started = Instant::now();
        let mut batch = Vec::new();
        let mut count = 0;
        let mut last_send = Instant::now();
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            // fd marks folders with a trailing separator.
            let dir = line.ends_with(['\\', '/']);
            let path = line.trim_end_matches(['\\', '/']);
            if path.is_empty() {
                continue;
            }
            batch.push(entry_for_path(path, dir));
            count += 1;
            if batch.len() >= 500 || last_send.elapsed() >= Duration::from_millis(40) {
                let _ = on_event.send(FindEvent::Entries { entries: std::mem::take(&mut batch) });
                last_send = Instant::now();
            }
        }
        if !batch.is_empty() {
            let _ = on_event.send(FindEvent::Entries { entries: batch });
        }
        let errors = stderr_reader.join().unwrap_or_default();
        // A stopped search ends quietly; only a run that finished by itself reports fd's complaints.
        if slot.finish(pid) && count == 0 && !errors.trim().is_empty() {
            let _ = on_event.send(FindEvent::Error { message: errors.trim().to_string() });
        }
        let limited = options.max_results.is_some_and(|max| max > 0 && count >= max);
        let _ = on_event.send(FindEvent::Finished { count, elapsed: ms_since(started), limited });
    });
    Ok(())
}

#[tauri::command]
pub fn cancel_find(state: State<AppState>) {
    state.find.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> FindOptions {
        FindOptions { hidden: false, respect_ignore: false, exclude: vec!["node_modules".into(), " ".into()], max_results: Some(100) }
    }

    #[test]
    fn splits_terms_with_quotes() {
        assert_eq!(terms(r#"  report "q3 final"  pdf "#), ["report", "q3 final", "pdf"]);
        assert!(terms("   ").is_empty());
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
}
