mod access;
mod appearance;
mod archive;
mod batch_rename;
mod calc;
mod check;
mod cleanup;
mod codesearch;
mod compare;
mod convert;
mod data_lens;
mod dedup;
mod defender;
mod doctext;
mod filediff;
mod find;
mod fsx;
mod git;
mod index;
mod jq;
mod jumplist;
mod launch;
mod locks;
mod markdown;
mod mcp;
mod meaning;
mod media;
mod newmenu;
mod native_preview;
mod ocr;
mod openers;
mod pe;
mod placement;
mod proc;
mod projects;
mod quick;
mod rclone;
mod recycle;
mod rules;
mod remote;
mod repo;
mod scripts;
mod lan;
mod share;
mod sheet;
mod shell;
mod shellmenu;
mod sign;
mod sql;
mod snap;
mod sizes;
mod term;
mod thumbs;
mod timeline;
mod tools;
mod usage;
mod verbs;
mod watch;
mod webview;
mod xray;
mod zoxide;

use fsx::blocking;
use proc::{Lanes, Slot};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{Emitter, State, Window};

/// The work in flight: the ripgrep text search, and each tab's file search (fd and the index) and
/// pdu measuring folder sizes or a disk-usage map.
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
    blocking(move || replace_contents(std::path::Path::new(&path), content.as_bytes()).map_err(|e| e.to_string())).await
}

/// Writes a temp file next to `path` and swaps it in (ReplaceFileW keeps the original's attributes,
/// permissions and streams), so a write cut short by a full disk or a dropped share leaves the file
/// as it was. A folder that doesn't allow a new file gets a plain write.
fn replace_contents(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let name = path.file_name().ok_or_else(|| std::io::Error::other("not a file"))?.to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}.glint-tmp", std::process::id()));
    let mut file = match fs::File::create_new(&temp) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return fs::write(path, content),
        Err(e) => return Err(e),
    };
    let written = file.write_all(content).and_then(|()| file.sync_all());
    drop(file);
    #[cfg(windows)]
    let written = written.and_then(|()| unsafe {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
        ReplaceFileW(&HSTRING::from(path), &HSTRING::from(temp.as_path()), PCWSTR::null(), REPLACEFILE_IGNORE_MERGE_ERRORS, None, None).map_err(std::io::Error::from)
    });
    #[cfg(not(windows))]
    let written = written.and_then(|()| fs::rename(&temp, path));
    if written.is_err() {
        if path.exists() {
            let _ = fs::remove_file(&temp);
        } else {
            // ReplaceFileW took the original but couldn't move the copy into its place
            // (ERROR_UNABLE_TO_MOVE_REPLACEMENT): the copy is all there is, so it's kept.
            let _ = fs::rename(&temp, path);
        }
    }
    written
}

/// Started with Windows (Settings → Quick search): in the tray, the window not shown.
static IN_TRAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Started from the taskbar's Quick search task: quick search comes up once the window has painted.
static QUICK_AT_START: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The page has painted: the window shows now, so it never flashes white first (but not when Glint
/// started in the tray). Setup shows it anyway if this never comes.
#[tauri::command]
fn app_ready(window: tauri::WebviewWindow) {
    if window.label() == "main" && !IN_TRAY.load(std::sync::atomic::Ordering::Relaxed) && !window.is_visible().unwrap_or(true) {
        placement::show(&window);
    }
    if window.label() == "main" {
        use tauri::Manager;
        quick::warm(window.app_handle());
        if QUICK_AT_START.swap(false, std::sync::atomic::Ordering::Relaxed) {
            quick::summon(window.app_handle());
        }
    }
}

/// What a bug report needs (Settings → About → Copy diagnostics): versions, where the tools come
/// from, and instant search's state. No file names, beyond what an error may quote.
#[tauri::command]
async fn diagnostics(app: tauri::AppHandle) -> Result<String, String> {
    use std::fmt::Write;
    let version = app.package_info().version.to_string();
    blocking(move || {
        let kind = if cfg!(debug_assertions) { "dev build" } else if installed_by_setup() { "installed by the setup" } else { "Scoop or portable" };
        let mut out = format!("Glint {version} ({kind})\n");
        #[cfg(windows)]
        let _ = writeln!(out, "{} · WebView2 {}", shadui_index::windows_version(), tauri::webview_version().unwrap_or_default());
        for tool in tools::TOOLS {
            let found = proc::locate(tool.exe);
            let version = found.as_ref().and_then(|(path, _)| tools::version_of(path)).unwrap_or_default();
            let from = match found {
                None => "missing".to_string(),
                Some((_, proc::Origin::Bundled)) => "bundled".to_string(),
                Some((path, _)) => path.display().to_string(),
            };
            let _ = writeln!(out, "{}: {version} ({from})", tool.name);
        }
        let index = index::info();
        let _ = write!(out, "Instant search: {}", index.state);
        if let Some(service) = &index.version {
            let _ = write!(out, " (service {service})");
        }
        if let Some(error) = &index.error {
            let _ = write!(out, ": {error}");
        }
        out.push('\n');
        for v in &index.volumes {
            let _ = write!(out, "  {} {}", v.volume, v.state);
            match v.state.as_str() {
                "ready" => {
                    let _ = write!(out, ", {} items from the {} in {:.0} ms, {} MB", v.files + v.folders, v.source, v.build_ms, v.memory >> 20);
                }
                "scanning" => {
                    let _ = write!(out, " {} of {} records", v.progress, v.progress_total);
                }
                _ => {}
            }
            if let Some(error) = &v.error {
                let _ = write!(out, ": {error}");
            }
            out.push('\n');
        }
        #[cfg(windows)]
        if let Some(log) = shadui_index::log_path() {
            let _ = writeln!(out, "Service log (administrators can open it): {}", log.display());
        }
        Ok(out)
    })
    .await
}

#[derive(Clone, serde::Serialize)]
struct SearchEvent {
    id: String,
    r#type: String,
    lines: Vec<String>,
}

/// Text searches in the order they were asked for: one that took longer to start (its folder on a share
/// slow to answer) mustn't replace a newer one, or start after Stop.
static TEXT_SEARCH: AtomicU64 = AtomicU64::new(0);

/// Whether text search `ticket` is still the one wanted: no newer search, and no Stop, since.
fn text_current(ticket: u64) -> bool {
    TEXT_SEARCH.load(Ordering::SeqCst) == ticket
}

/// The folder a text search runs in (absolute, with %VARS% and ~ expanded), once it's known to be
/// there. Looking can take seconds on a sleeping share, so it's never done on the UI thread.
fn text_folder(path: &str) -> Result<String, String> {
    let folder = fsx::normalize(path)?;
    if std::fs::metadata(&folder).is_err() {
        return Err(format!("Can't find “{folder}”. Check the folder path and try again."));
    }
    Ok(folder)
}

#[tauri::command]
fn cancel_search(state: State<AppState>) {
    TEXT_SEARCH.fetch_add(1, Ordering::SeqCst);
    state.search.kill();
    doctext::cancel();
}

/// Stops every sidecar that runs (the text search, and each tab's file search, folder sizes and
/// disk-usage map) and waits until they've ended, so an update's installer can replace rg, fd and pdu.
#[tauri::command]
async fn stop_tools(state: State<'_, AppState>) -> Result<(), String> {
    TEXT_SEARCH.fetch_add(1, Ordering::SeqCst);
    state.search.kill();
    doctext::cancel();
    state.finds.stop_all();
    state.sizes.close_all();
    state.usage.close_all();
    fsx::blocking(|| {
        proc::wait_stopped();
        Ok(())
    })
    .await
}

/// A text search as Text mode asks for it. ripgrep's arguments are built here from these, never
/// passed through from the UI (a flag like `--pre` would run a program).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TextSearch {
    query: String,
    path: String,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    exclusions: Vec<String>,
    /// Searching again, skipping nothing: hidden files, what .gitignore files list and the exclusions too.
    #[serde(default)]
    everything: bool,
}

fn rg_args(search: &TextSearch, folder: &str) -> Vec<String> {
    // --no-messages hides per-file errors (e.g. Access Denied); pattern errors still reach stderr.
    // The total result limit is enforced while reading (--max-count is per file).
    // --crlf: $ also ends a line before CRLF, as most files on Windows have them.
    let mut args: Vec<String> = ["--json", "--no-messages", "--stats", "--crlf"].map(String::from).into();
    args.push(if search.case_sensitive { "--case-sensitive" } else { "--smart-case" }.into());
    if search.whole_word {
        args.push("--word-regexp".into());
    }
    let mut query = search.query.clone();
    if query.contains('\n') {
        args.extend(["--multiline".into(), "--multiline-dotall".into()]);
        // Textareas normalize to \n but files may use \r\n, so literal searches match both.
        if !search.regex {
            let escaped: String = query.chars().flat_map(|c| ".*+?^${}()|[]\\".contains(c).then_some('\\').into_iter().chain([c])).collect();
            query = escaped.replace('\n', "\\r?\\n");
        }
    } else if !search.regex {
        args.push("--fixed-strings".into());
    }
    if search.everything {
        args.extend(["--hidden".into(), "--no-ignore".into()]);
    } else {
        for exclusion in &search.exclusions {
            args.extend(["--glob".into(), format!("!{exclusion}")]);
        }
    }
    // "--" so queries starting with "-" aren't parsed as flags.
    args.extend(["--".into(), query, folder.into()]);
    args
}

/// Starts a text search; its results arrive as `search-event`s. Returns the folder as searched
/// (absolute, with %VARS% and ~ expanded), which the results show their paths relative to.
#[tauri::command]
async fn run_ripgrep_batched(
    window: Window,
    request: TextSearch,
    search_id: String,
    max_results: Option<usize>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let ticket = TEXT_SEARCH.fetch_add(1, Ordering::SeqCst) + 1;
    let search = state.search.clone();
    fsx::blocking(move || {
        let folder = text_folder(&request.path)?;
        let mut command = proc::tool("rg")?;
        command
            .args(rg_args(&request, &folder))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = command
            .spawn()
            .map_err(|e| format!("Failed to start ripgrep: {e}"))?;
        stream_search(window, search_id, child, max_results, search, ticket, |line: String| (!line.is_empty()).then_some(line))?;
        Ok(folder)
    })
    .await
}

/// A Documents search (Text's documents toggle): PDFs', Office's, OpenDocument's and EPUBs' text, read
/// by worker processes (doctext.rs). Its matches arrive as `search-event`s, as ripgrep's JSON lines with
/// a label each ("p. 3"), then a summary and "finished". Returns the folder as searched.
#[tauri::command]
async fn run_doc_search(window: Window, request: doctext::DocSearch, search_id: String, max_results: Option<usize>, state: State<'_, AppState>) -> Result<String, String> {
    let ticket = TEXT_SEARCH.fetch_add(1, Ordering::SeqCst) + 1;
    let search = state.search.clone();
    fsx::blocking(move || doc_search(window, request, search_id, max_results, search, ticket)).await
}

fn doc_search(window: Window, request: doctext::DocSearch, search_id: String, max_results: Option<usize>, search: Slot, ticket: u64) -> Result<String, String> {
    let folder = text_folder(&request.path)?;
    // By meaning, what's typed isn't a pattern: a regex that matches nothing stands in.
    let pattern = if request.meaning { regex::Regex::new("a^").map_err(|e| e.to_string())? } else { doctext::pattern(&request)? };
    if !text_current(ticket) {
        return Err("cancelled".into());
    }
    search.kill();
    let run = doctext::begin();
    let max = match max_results {
        Some(0) | None => usize::MAX,
        Some(n) => n,
    };
    let base = std::path::PathBuf::from(&folder);
    thread::spawn(move || {
        let emit = |r#type: &str, lines: Vec<String>| {
            let _ = window.emit("search-event", SearchEvent { id: search_id.clone(), r#type: r#type.into(), lines });
        };
        // By meaning: the model (loaded here the first time), and words in what's typed that it knows.
        let ready = if request.meaning {
            meaning::model().and_then(|model| model.embed(&request.query).map(|_| ()).ok_or_else(|| "The meaning model knows none of those words: try others".to_string()))
        } else {
            Ok(())
        };
        match ready {
            Err(message) => emit("error", vec![message]),
            Ok(()) => doctext::search(run, &base, &request, &pattern, max, |lines| emit("data", lines)),
        }
        emit("finished", Vec::new());
    });
    Ok(folder)
}

/// Lines `from` to `to` (1-based) of a document as the Documents search read it, with their labels, for
/// the preview.
#[tauri::command]
async fn doc_lines(path: String, from: usize, to: usize) -> Result<Vec<(String, String)>, String> {
    fsx::blocking(move || doctext::lines_of(&path, from, to)).await
}

/// A code search (Text's code patterns, by ast-grep): its matches arrive as `search-event`s, as
/// ripgrep's JSON lines (codesearch.rs), then a summary with how long it took. Returns the folder as
/// searched.
#[tauri::command]
async fn run_ast_grep(window: Window, request: codesearch::CodeSearch, search_id: String, max_results: Option<usize>, state: State<'_, AppState>) -> Result<String, String> {
    let ticket = TEXT_SEARCH.fetch_add(1, Ordering::SeqCst) + 1;
    let search = state.search.clone();
    fsx::blocking(move || ast_grep_search(window, request, search_id, max_results, search, ticket)).await
}

fn ast_grep_search(window: Window, request: codesearch::CodeSearch, search_id: String, max_results: Option<usize>, search: Slot, ticket: u64) -> Result<String, String> {
    let folder = text_folder(&request.path)?;
    let mut command = tools::command("ast-grep")?;
    command.args(codesearch::args(&request, &folder, false)).current_dir(&folder).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = command.spawn().map_err(|e| format!("Couldn't start ast-grep: {e}"))?;
    let started = Instant::now();
    let base = folder.clone();
    let mut summary = Some(());
    stream_search(window, search_id, child, max_results, search, ticket, move |line: String| {
        // ast-grep says nothing at the end: the summary ripgrep would send is made here.
        if line.is_empty() {
            return summary.take().map(|_| {
                let elapsed = started.elapsed();
                serde_json::json!({ "type": "summary", "data": { "elapsed_total": { "secs": elapsed.as_secs(), "nanos": elapsed.subsec_nanos(), "human": format!("{:.3}s", elapsed.as_secs_f64()) } } }).to_string()
            });
        }
        codesearch::as_ripgrep(&line, &base)
    })?;
    Ok(folder)
}

/// Rewrites every match of a code search in its files, with ast-grep (--update-all); how many it rewrote.
#[tauri::command]
async fn ast_grep_apply(request: codesearch::CodeSearch) -> Result<u64, String> {
    fsx::blocking(move || {
        let folder = fsx::normalize(&request.path)?;
        if request.rewrite.as_deref().unwrap_or("").is_empty() {
            return Err("Type what each match becomes first.".into());
        }
        let mut command = tools::command("ast-grep")?;
        let output = command.args(codesearch::args(&request, &folder, true)).current_dir(&folder).stdin(Stdio::null()).output().map_err(|e| format!("Couldn't start ast-grep: {e}"))?;
        let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        if !output.status.success() && codesearch::applied(&said) == 0 {
            return Err(said.trim().to_string());
        }
        Ok(codesearch::applied(&said))
    })
    .await
}

/// Sends a search's output to the UI as `search-event`s: its lines (each `convert`ed, None to skip
/// one) in batches, until it ends or `max_results` matches came, then an empty line through `convert`
/// (for a summary of its own), any error it gave, and "finished". The search replaces the one before,
/// and Documents' run, unless a newer one (or Stop) came while it started: then it's stopped instead.
fn stream_search(window: Window, search_id: String, mut child: std::process::Child, max_results: Option<usize>, search: proc::Slot, ticket: u64, mut convert: impl FnMut(String) -> Option<String> + Send + 'static) -> Result<(), String> {
    let stdout = child.stdout.take().ok_or("Failed to open stdout")?;
    let mut stderr = child.stderr.take().ok_or("Failed to open stderr")?;
    // Replaces (and stops) any search still running.
    let pid = search.replace_if(child, || text_current(ticket)).ok_or("cancelled")?;
    doctext::cancel();

    // Drain stderr on its own thread so a chatty search can never block stdout.
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
            let Ok(line) = line else { break };
            let Some(l) = convert(line) else { continue };
            // The JSON always starts with the type field, so no need to parse every line.
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
                // Limit reached: kill this search — but not a newer one that already replaced it.
                search.kill_if(pid);
                break;
            }
        }
        if let Some(last) = convert(String::new()) {
            batch.push(last);
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
/// still reaches the page, where Files uses F5 to refresh. Called for each window: the main one at
/// start, the quick search window when it's made.
#[cfg(all(windows, not(debug_assertions)))]
pub(crate) fn disable_browser_keys(window: &tauri::WebviewWindow) {
    use webview2_com::AcceleratorKeyPressedEventHandler;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2AcceleratorKeyPressedEventArgs2;
    use windows::core::Interface;

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

/// Whether the setup program installed this copy (it leaves `uninstall.exe` next to the app; the Scoop
/// manifest deletes the one 7-Zip extracts). Only those update themselves: a Scoop or portable copy, or
/// a dev build, is updated by whatever put it there, so the app just links to the new release.
#[cfg(desktop)]
pub(crate) fn installed_by_setup() -> bool {
    std::env::current_exe().is_ok_and(|exe| exe.with_file_name("uninstall.exe").exists())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Glint started as a worker: the one that reads data files for their tree (jq.rs), one that reads
/// documents for Documents search (doctext.rs), SFTP's password prompt (remote/), the MCP server an AI
/// agent starts (mcp.rs), or the installer's tool installs (tools.rs). Its exit code, once it has
/// answered; None for the app.
pub fn worker() -> Option<i32> {
    jq::worker().or_else(doctext::worker).or_else(remote::askpass).or_else(mcp::server).or_else(tools::cli)
}

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
    // Glint's window is WebView2: on a PC without it, Glint offers to install it, and ends if it can't.
    if !webview::ready() {
        return;
    }

    let in_tray = std::env::args().any(|arg| arg == "--tray");
    IN_TRAY.store(in_tray, std::sync::atomic::Ordering::Relaxed);
    QUICK_AT_START.store(launch::asks_for_quick_search(std::env::args().skip(1)), std::sync::atomic::Ordering::Relaxed);
    let builder = tauri::Builder::default();
    // One copy runs at a time: a later launch hands its folders to it and quits. Not in debug builds,
    // which would hand theirs to an installed Glint (they share its identifier).
    #[cfg(all(desktop, not(debug_assertions)))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(launch::second_launch));
    builder
        .manage(launch::Pending::new(request.open))
        .manage(AppState::default())
        .manage(watch::FolderWatcher::default())
        .manage(git::RepoWatcher::default())
        .manage(thumbs::Thumbs::default())
        .manage(quick::Quick::default())
        .manage(tools::Installs::default())
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    if let Ok(hwnd) = window.hwnd() {
                        placement::save(hwnd.0 as _);
                    }
                }
                quick::on_close(window, api);
            }
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            appearance::appearance,
            appearance::accent_colors,
            mcp::mcp_setup,
            shellmenu::shell_menu,
            read_file_content,
            read_file_chunk,
            write_file_content,
            run_ripgrep_batched,
            run_ast_grep,
            ast_grep_apply,
            run_doc_search,
            doc_lines,
            zoxide::zoxide_folders,
            zoxide::zoxide_add,
            calc::calculate,
            ocr::ocr_text,
            meaning::meaning_install,
            meaning::meaning_status,
            meaning::meaning_remove,
            recycle::recycle_empty,
            tools::tools_status,
            tools::tool_install,
            tools::tool_cancel_install,
            tools::tool_remove,
            quick::quick_configure,
            quick::quick_reveal,
            quick::quick_hide,
            quick::set_start_with_windows,
            diagnostics,
            app_ready,
            cancel_search,
            stop_tools,
            fsx::list_dir,
            access::grant_access,
            fsx::resolve_path,
            fsx::list_drives,
            fsx::read_text_preview,
            fsx::create_entry,
            newmenu::new_kinds,
            rclone::rclone_providers,
            rclone::rclone_config_start,
            rclone::rclone_config_answer,
            rclone::rclone_authorize,
            rclone::rclone_authorize_cancel,
            rclone::rclone_remotes,
            rclone::rclone_delete,
            rclone::rclone_about,
            rclone::rclone_status,
            rclone::rclone_unlock,
            newmenu::new_file,
            find::find_files,
            find::cancel_find,
            sizes::folder_sizes,
            sizes::cancel_folder_sizes,
            thumbs::thumbnail,
            thumbs::cancel_thumbnails,
            usage::disk_usage,
            usage::cancel_disk_usage,
            index::index_status,
            index::index_history,
            index::index_enable,
            index::index_disable,
            shell::file_op,
            shell::clipboard_set_files,
            shell::clipboard_get_files,
            shell::clipboard_text,
            shell::clipboard_paste_file,
            shell::clipboard_clear,
            shell::show_properties,
            shell::open_item,
            shell::open_with,
            shell::open_terminal,
            shell::start_drag,
            archive::archive_extract,
            archive::archive_stage,
            archive::archive_compress,
            archive::archive_extract_here,
            archive::cancel_archive_stage,
            archive::archive_unstage,
            archive::archive_progress,
            archive::cancel_archive_read,
            archive::archive_peek,
            archive::export_ncdu,
            locks::who_locks,
            locks::stop_locking_process,
            pe::inspect_pe,
            cleanup::cleanup_scan,
            cleanup::cancel_cleanup,
            git::git_view,
            git::cancel_git,
            git::git_watch,
            git::git_diff,
            git::git_head_copy,
            git::git_head_discard,
            filediff::compare_text_files,
            filediff::compare_bytes,
            filediff::file_bytes,
            filediff::next_byte_difference,
            term::term_shells,
            term::term_spawn,
            term::term_write,
            term::term_resize,
            term::term_close,
            dedup::find_duplicates,
            dedup::cancel_dedup,
            compare::compare_folders,
            compare::cancel_compare,
            dedup::hardlink_duplicates,
            dedup::clone_duplicates,
            convert::convert_images,
            convert::convert_cancel,
            openers::openers,
            openers::open_in,
            repo::repo_card,
            projects::project_tasks,
            scripts::scripts_list,
            scripts::scripts_folder,
            share::send_start,
            share::receive_start,
            share::share_stop,
            lan::lan_addresses,
            lan::lan_share_start,
            lan::lan_reach,
            lan::lan_open_settings,
            remote::remote_configure,
            remote::remote_secret_set,
            remote::remote_secret_delete,
            remote::remote_secret_saved,
            remote::remote_test,
            remote::remote_host_keys,
            remote::remote_trust_host,
            rules::rules_set,
            rules::rules_preview,
            rules::rules_apply,
            rules::rules_log,
            rules::rules_clear_log,
            rules::rules_undo,
            timeline::scan_timeline,
            timeline::cancel_timeline,
            data_lens::inspect_sqlite,
            data_lens::inspect_tabular,
            batch_rename::batch_rename,
            xray::inspect_file_xray,
            xray::file_real_type,
            markdown::markdown_html,
            jq::jq_query,
            jq::jq_stop,
            sheet::inspect_sheet,
            sql::sql_tables,
            sql::sql_run,
            media::media_details,
            media::media_details_many,
            sign::verify_signature,
            defender::defender_scan,
            defender::cancel_defender_scan,
            check::yara_scan,
            check::secrets_scan,
            check::cancel_check,
            native_preview::native_preview_show,
            native_preview::native_preview_place,
            native_preview::native_preview_close,
            snap::snap_place,
            xray::cancel_xray,
            xray::remove_ntfs_stream,
            watch::watch_dirs,
            launch::take_launch_targets,
            verbs::explorer_integration,
            verbs::set_explorer_integration,
            jumplist::sync_jump_list,
            jumplist::taskbar_progress,
            jumplist::thumb_buttons,
            shell::windows_share,
            shell::eject_drive,
            shell::mount_disk_image,
            shell::set_cloud_pin,
            shell::set_desktop_wallpaper,
            shell::rotate_image,
            shell::mounts_by_default,
        ])
        .setup(move |app| {
            #[cfg(all(windows, not(debug_assertions)))]
            if let Some(window) = tauri::Manager::get_webview_window(app, "main") {
                disable_browser_keys(&window);
            }
            // Made hidden: it shows once the page has painted (app_ready), or after a few seconds if the
            // page never says so, and not at all when Glint started in the tray.
            if !in_tray {
                use tauri::Manager;
                let handle = app.handle().clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(3));
                    if let Some(window) = handle.get_webview_window("main") {
                        if !window.is_visible().unwrap_or(true) {
                            placement::show(&window);
                        }
                    }
                    quick::warm(&handle);
                });
            }
            {
                use tauri::Manager;
                app.state::<thumbs::Thumbs>().warm_up();
                if let Ok(data) = app.path().app_local_data_dir() {
                    tools::set_dir(data.join("tools"));
                    if let Some(window) = app.get_webview_window("main") {
                        placement::restore(&window, &data);
                    }
                }
                if let Some(name) = &app.config().product_name {
                    remote::init(name);
                }
            }
            #[cfg(desktop)]
            if installed_by_setup() {
                app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || verbs::repair(&handle));
            // What earlier runs extracted from archives goes, except what's still on the clipboard.
            std::thread::spawn(|| archive::sweep(&shell::clipboard_paths()));
            std::thread::spawn(share::sweep);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| match event {
            // Quit from the tray with the window open: its place is kept as when it closes.
            tauri::RunEvent::ExitRequested { .. } => {
                use tauri::Manager;
                if let Some(Ok(hwnd)) = app.get_webview_window("main").map(|w| w.hwnd()) {
                    placement::save(hwnd.0 as _);
                }
            }
            tauri::RunEvent::Exit => {
                archive::finish();
                term::close_all();
                share::stop_all();
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(query: &str, regex: bool) -> TextSearch {
        TextSearch { query: query.into(), path: String::new(), case_sensitive: false, whole_word: true, regex, exclusions: vec!["*.min.js".into()], everything: false }
    }

    #[test]
    fn builds_ripgrep_arguments_from_typed_options() {
        let args = rg_args(&search("--pre=calc", false), r"C:\src");
        assert!(args.contains(&"--fixed-strings".into()) && args.contains(&"--smart-case".into()) && args.contains(&"--word-regexp".into()));
        assert!(args.contains(&"--crlf".into()));
        assert!(args.ends_with(&["--glob".into(), "!*.min.js".into(), "--".into(), "--pre=calc".into(), r"C:\src".into()]));
        // A literal query over lines is escaped and matches CRLF files too.
        let args = rg_args(&search("a.b\n(c)", false), r"C:\src");
        assert!(args.contains(&"--multiline".into()) && !args.contains(&"--fixed-strings".into()));
        assert!(args.contains(&r"a\.b\r?\n\(c\)".into()));
        assert!(!rg_args(&search("a.b", true), "C:\\").contains(&"--fixed-strings".into()));
        // Skipping nothing: no exclusions, and hidden and ignored files too.
        let args = rg_args(&TextSearch { everything: true, ..search("a", false) }, "C:\\");
        assert!(args.contains(&"--hidden".into()) && args.contains(&"--no-ignore".into()) && !args.contains(&"--glob".into()));
    }

    #[test]
    fn replaces_contents_without_leaving_a_temp_file() {
        let dir = std::env::temp_dir().join(format!("glint-replace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        fs::write(&file, "old text").unwrap();
        replace_contents(&file, b"new text").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "new text");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "the temp file is gone");
        fs::remove_dir_all(&dir).unwrap();
    }
}
