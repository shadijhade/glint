mod find;
mod fsx;
mod index;
mod proc;
mod shell;
mod sizes;
mod watch;

use fsx::blocking;
use proc::Slot;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, State, Window};
use tauri_plugin_shell::ShellExt;

/// The work in flight: ripgrep text search, file search (fd and the index) and dust folder sizes.
#[derive(Default)]
pub struct AppState {
    pub search: Slot,
    pub find: Slot,
    pub finding: find::Current,
    pub sizes: Slot,
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

#[tauri::command]
async fn read_file_content(path: String) -> Result<String, String> {
    blocking(move || fs::read_to_string(path).map_err(|e| e.to_string())).await
}

#[tauri::command]
async fn read_file_chunk(path: String, start_line: usize, end_line: usize) -> Result<String, String> {
    blocking(move || {
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        // Stops reading once the last wanted line is in. Lines that aren't valid UTF-8 are shown
        // lossily rather than dropped, so line numbers stay right.
        let chunk: Vec<String> = BufReader::new(file)
            .split(b'\n')
            .skip(start_line.saturating_sub(1))
            .take(end_line.saturating_sub(start_line) + 1)
            .map_while(Result::ok)
            .map(|line| String::from_utf8_lossy(line.strip_suffix(b"\r").unwrap_or(&line)).into_owned())
            .collect();
        Ok(chunk.join("\n"))
    })
    .await
}

#[tauri::command]
async fn write_file_content(path: String, content: String) -> Result<(), String> {
    blocking(move || fs::write(path, content).map_err(|e| e.to_string())).await
}

#[derive(Clone, serde::Serialize)]
struct SearchEvent {
    id: String,
    r#type: String,
    lines: Vec<String>,
}

#[tauri::command]
fn cancel_search(state: State<AppState>) {
    state.search.kill();
}

#[tauri::command]
fn run_ripgrep_batched(
    window: Window,
    args: Vec<String>,
    search_id: String,
    max_results: Option<usize>,
    state: State<AppState>,
) -> Result<(), String> {
    // The bundled sidecar, so users don't need ripgrep on PATH (it also sets CREATE_NO_WINDOW).
    let mut command: Command = window.shell().sidecar("rg").map_err(|e| e.to_string())?.into();
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to start ripgrep: {e}"))?;
    let stdout = child.stdout.take().ok_or("Failed to open stdout")?;
    let mut stderr = child.stderr.take().ok_or("Failed to open stderr")?;
    // Replaces (and stops) any search still running.
    let search = state.search.clone();
    let pid = search.replace(child);

    // Drain stderr on its own thread so a chatty rg can never block stdout.
    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut batch = Vec::new();
        let mut last_emit = Instant::now();

        let mut total_matches = 0;
        let limit = match max_results {
            Some(0) | None => usize::MAX,
            Some(n) => n,
        };

        for line in reader.lines() {
            if let Ok(l) = line {
                // rg's JSON always starts with the type field, so no need to parse every line.
                if l.starts_with(r#"{"type":"match""#) {
                    total_matches += 1;
                }

                batch.push(l);

                if batch.len() >= 1000 || last_emit.elapsed() >= Duration::from_millis(50) {
                    let _ = window.emit(
                        "search-event",
                        SearchEvent {
                            id: search_id.clone(),
                            r#type: "data".to_string(),
                            lines: batch.clone(),
                        },
                    );
                    batch.clear();
                    last_emit = Instant::now();
                }

                if total_matches >= limit {
                    // Limit reached: kill our rg — but not a newer search that already replaced it.
                    search.kill_if(pid);
                    break;
                }
            } else {
                break;
            }
        }

        if !batch.is_empty() {
            let _ = window.emit(
                "search-event",
                SearchEvent {
                    id: search_id.clone(),
                    r#type: "data".to_string(),
                    lines: batch,
                },
            );
        }

        // Surface real failures (e.g. an invalid regex); per-file errors are silenced by --no-messages.
        let errors = stderr_reader.join().unwrap_or_default();
        search.finish(pid);
        if total_matches == 0 && !errors.trim().is_empty() {
            let _ = window.emit(
                "search-event",
                SearchEvent {
                    id: search_id.clone(),
                    r#type: "error".to_string(),
                    lines: vec![errors.trim().to_string()],
                },
            );
        }

        let _ = window.emit(
            "search-event",
            SearchEvent {
                id: search_id,
                r#type: "finished".to_string(),
                lines: vec![],
            },
        );
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .manage(watch::FolderWatcher::default())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            read_file_content,
            read_file_chunk,
            write_file_content,
            run_ripgrep_batched,
            cancel_search,
            fsx::list_dir,
            fsx::resolve_path,
            fsx::list_drives,
            fsx::read_text_preview,
            fsx::create_entry,
            find::find_files,
            find::cancel_find,
            sizes::folder_sizes,
            sizes::cancel_folder_sizes,
            index::index_status,
            index::index_enable,
            index::index_disable,
            shell::file_op,
            shell::clipboard_set_files,
            shell::clipboard_get_files,
            shell::clipboard_clear,
            shell::show_properties,
            shell::open_with,
            shell::open_terminal,
            watch::watch_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
