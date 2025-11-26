use std::fs;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, State, Window};

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
    state: State<AppState>,
) -> Result<(), String> {
    // Kill any existing search first
    {
        let mut current_search = state.current_search.lock().map_err(|e| e.to_string())?;
        if let Some(mut child) = current_search.take() {
            let _ = child.kill();
        }
    }

    let mut command = Command::new("rg");
    command.args(&args);
    command.stdout(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("Failed to open stdout")?;

    // Store the child process
    {
        let mut current_search = state.current_search.lock().map_err(|e| e.to_string())?;
        *current_search = Some(child);
    }

    // Clone the Arc to move into the thread
    let search_handle = state.current_search.clone();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        let mut batch = Vec::new();
        let mut last_emit = Instant::now();

        let mut total_matches = 0;
        const MAX_MATCHES: usize = 20000;

        for line in reader.lines() {
            if let Ok(l) = line {
                total_matches += 1;
                batch.push(l);

                if batch.len() >= 1000 || last_emit.elapsed() >= Duration::from_millis(50) {
                    let _ = window.emit(
                        "search-event",
                        SearchEvent {
                            r#type: "data".to_string(),
                            lines: batch.clone(),
                        },
                    );
                    batch.clear();
                    last_emit = Instant::now();
                }

                if total_matches >= MAX_MATCHES {
                    // Kill the process if limit reached
                    let mut current_search = search_handle.lock().unwrap();
                    if let Some(mut child) = current_search.take() {
                        let _ = child.kill();
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
                    r#type: "data".to_string(),
                    lines: batch,
                },
            );
        }

        let _ = window.emit(
            "search-event",
            SearchEvent {
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
