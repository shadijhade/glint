use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, State, Window};
use tauri_plugin_shell::ShellExt;

struct AppState {
    current_search: Arc<Mutex<Option<Child>>>,
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

#[tauri::command]
fn read_file_content(path: String) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_file_chunk(path: String, start_line: usize, end_line: usize) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);

    let chunk: Vec<String> = reader
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let line_num = i + 1;
            if line_num >= start_line && line_num <= end_line {
                line.ok()
            } else {
                None
            }
        })
        .take(end_line - start_line + 1) // Optimization: stop after we have enough lines?
        // Actually filter_map iterates everything unless we stop.
        // Let's rewrite to be more efficient: skip and take.
        .collect();

    // Re-implementation for efficiency
    // We can't easily "skip" without iterating because lines have variable length.
    // But we can stop early.

    Ok(chunk.join("\n"))
}

#[tauri::command]
fn write_file_content(path: String, content: String) -> Result<(), String> {
    fs::write(path, content).map_err(|e| e.to_string())
}

#[derive(Clone, serde::Serialize)]
struct SearchEvent {
    id: String,
    r#type: String,
    lines: Vec<String>,
}

#[tauri::command]
fn cancel_search(state: State<AppState>) -> Result<(), String> {
    let mut current_search = state.current_search.lock().map_err(|e| e.to_string())?;
    if let Some(mut child) = current_search.take() {
        let _ = child.kill();
    }
    Ok(())
}

#[tauri::command]
fn run_ripgrep_batched(
    window: Window,
    args: Vec<String>,
    search_id: String,
    max_results: Option<usize>,
    state: State<AppState>,
) -> Result<(), String> {
    // Kill any existing search first
    {
        let mut current_search = state.current_search.lock().map_err(|e| e.to_string())?;
        if let Some(mut child) = current_search.take() {
            let _ = child.kill();
        }
    }

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
    let pid = child.id();

    // Store the child process
    {
        let mut current_search = state.current_search.lock().map_err(|e| e.to_string())?;
        *current_search = Some(child);
    }

    // Clone the Arc to move into the thread
    let search_handle = state.current_search.clone();

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
                    let mut current_search = search_handle.lock().unwrap();
                    if current_search.as_ref().map(|c| c.id()) == Some(pid) {
                        if let Some(mut child) = current_search.take() {
                            let _ = child.kill();
                        }
                    }
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
        .manage(AppState {
            current_search: Arc::new(Mutex::new(None)),
        })
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            read_file_content,
            read_file_chunk,
            write_file_content,
            run_ripgrep_batched,
            cancel_search
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
