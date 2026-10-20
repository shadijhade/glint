mod archive;
mod batch_rename;
mod data_lens;
mod dedup;
mod find;
mod fsx;
mod index;
mod launch;
mod locks;
mod pe;
mod proc;
mod shell;
mod sizes;
mod thumbs;
mod timeline;
mod usage;
mod verbs;
mod watch;
mod xray;

use fsx::blocking;
use proc::{Lanes, Slot};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, State, Window};

/// The work in flight: the ripgrep text search, and each tab's file search (fd and the index) and
/// dust measuring folder sizes or a disk-usage map.
#[derive(Default)]
pub struct AppState {
    pub search: Slot,
    pub finds: find::Finds,
    pub sizes: Lanes,
    pub usage: Lanes,
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

/// Stops every sidecar that runs (the text search, and each tab's file search, folder sizes and
/// disk-usage map), so an update's installer can replace rg, fd and dust.
#[tauri::command]
fn stop_tools(state: State<AppState>) {
    state.search.kill();
    state.finds.stop_all();
    state.sizes.close_all();
    state.usage.close_all();
}

#[tauri::command]
fn run_ripgrep_batched(
    window: Window,
    mut args: Vec<String>,
    search_id: String,
    max_results: Option<usize>,
    state: State<AppState>,
) -> Result<(), String> {
    if let Some(last) = args.last_mut() {
        let normalized = fsx::normalize(last)?;
        if std::fs::metadata(&normalized).is_err() {
            return Err(format!("Can't find “{normalized}”. Check the folder path and try again."));
        }
        *last = normalized;
    }
    // Prefer the bundled sidecar, falling back to ripgrep on PATH or standard install locations.
    let mut command = proc::tool(&window, "rg")?;
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

/// In release builds, WebView2's browser shortcuts would reload the whole app (F5, Ctrl+R), print it
/// (Ctrl+P), open a find bar (Ctrl+F) or zoom it. This switches off the browser's handling of each
/// key as it's pressed (the global setting only applies from the next page load), while the key
/// still reaches the page, where Files uses F5 to refresh.
#[cfg(all(windows, not(debug_assertions)))]
fn disable_browser_keys(app: &tauri::App) {
    use tauri::Manager;
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2AcceleratorKeyPressedEventArgs2;
    use windows::core::Interface;

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.with_webview(|webview| unsafe {
            let handler = AcceleratorKeyPressedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args.and_then(|a| a.cast::<ICoreWebView2AcceleratorKeyPressedEventArgs2>().ok()) {
                    let _ = args.SetIsBrowserAcceleratorKeyEnabled(false);
                }
                Ok(())
            }));
            let mut token = 0;
            let _ = webview.controller().add_AcceleratorKeyPressed(&handler, &mut token);
        });
    }
}

/// Whether the setup program installed this copy (it leaves `uninstall.exe` next to the app; the Scoop
/// manifest deletes the one 7-Zip extracts). Only those update themselves: a Scoop or portable copy, or
/// a dev build, is updated by whatever put it there, so the app just links to the new release.
#[cfg(desktop)]
pub(crate) fn installed_by_setup() -> bool {
    std::env::current_exe().is_ok_and(|exe| exe.with_file_name("uninstall.exe").exists())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Folders from File Explorer, a double-click or a terminal. What the app can't open goes to File
    // Explorer before any window shows, and a launch with nothing left to open here ends there.
    let request = launch::Request::from_env();
    if !request.explorer.is_empty() {
        launch::hand_to_explorer(&request.explorer);
        if request.open.is_empty() {
            return;
        }
    }

    let builder = tauri::Builder::default();
    // One copy runs at a time: a later launch hands its folders to it and quits. Not in debug builds,
    // which would hand theirs to an installed Glint (they share its identifier).
    #[cfg(all(desktop, not(debug_assertions)))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(launch::second_launch));
    builder
        .manage(launch::Pending::new(request.open))
        .manage(AppState::default())
        .manage(watch::FolderWatcher::default())
        .manage(thumbs::Thumbs::default())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            read_file_content,
            read_file_chunk,
            write_file_content,
            run_ripgrep_batched,
            cancel_search,
            stop_tools,
            fsx::list_dir,
            fsx::resolve_path,
            fsx::list_drives,
            fsx::read_text_preview,
            fsx::create_entry,
            find::find_files,
            find::cancel_find,
            sizes::folder_sizes,
            sizes::cancel_folder_sizes,
            thumbs::thumbnail,
            thumbs::cancel_thumbnails,
            usage::disk_usage,
            usage::cancel_disk_usage,
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
            shell::start_drag,
            archive::archive_extract,
            archive::archive_stage,
            archive::cancel_archive_stage,
            archive::archive_unstage,
            locks::who_locks,
            locks::stop_locking_process,
            pe::inspect_pe,
            dedup::find_duplicates,
            dedup::cancel_dedup,
            dedup::hardlink_duplicates,
            timeline::scan_timeline,
            data_lens::inspect_sqlite,
            data_lens::inspect_tabular,
            batch_rename::batch_rename,
            xray::inspect_file_xray,
            xray::remove_ntfs_stream,
            watch::watch_dirs,
            launch::take_launch_targets,
            verbs::explorer_integration,
            verbs::set_explorer_integration,
        ])
        .setup(|app| {
            #[cfg(all(windows, not(debug_assertions)))]
            disable_browser_keys(app);
            {
                use tauri::Manager;
                app.state::<thumbs::Thumbs>().warm_up();
            }
            #[cfg(desktop)]
            if installed_by_setup() {
                app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || verbs::repair(&handle));
            // What earlier runs extracted from archives goes, except what's still on the clipboard.
            std::thread::spawn(|| archive::sweep(&shell::clipboard_paths()));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_, event| {
            if let tauri::RunEvent::Exit = event {
                archive::finish();
            }
        });
}
