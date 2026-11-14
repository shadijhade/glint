//! Glint's engine, moved from src-tauri (see docs/gpui/STANDARDS.md section 6): everything that isn't a
//! window. Tauri is gone; `host` stands in for the little of it the modules used.

pub mod host;
pub mod access;
pub mod actions;
pub mod agents;
// (window/app module, rewritten for GPUI) mod appearance;
pub mod archive;
pub mod attachments;
pub mod backup;
pub mod bench;
pub mod batch_rename;
pub mod calc;
pub mod check;
pub mod cleanup;
pub mod codesearch;
pub mod compare;
pub mod content;
pub mod convert;
pub mod credentials;
pub mod crash;
pub mod data_lens;
pub mod dedup;
pub mod duck;
pub mod defender;
pub mod devices;
pub mod dialogs;
pub mod docconvert;
// (window/app module, rewritten for GPUI) mod dragdrop;
pub mod drivehealth;
pub mod doctext;
pub mod filediff;
pub mod find;
pub mod fsx;
pub mod gallery;
pub mod git;
pub mod index;
pub mod ioring;
pub mod jq;
// (window/app module, rewritten for GPUI) mod jumplist;
// (window/app module, rewritten for GPUI) mod launch;
pub mod locks;
pub mod mcp;
pub mod meaning;
pub mod metadata;
pub mod multilingual;
pub mod media;
pub mod media_ai;
pub mod media_tools;
pub mod newmenu;
// (window/app module, rewritten for GPUI) mod native_preview;
pub mod ocr;
pub mod ondemand;
pub mod openers;
pub mod pdf_tools;
pub mod pe;
// (window/app module, rewritten for GPUI) mod print;
pub mod photos;
// (window/app module, rewritten for GPUI) mod placement;
pub mod proc;
pub mod projects;
pub mod queue;
// (window/app module, rewritten for GPUI) mod quick;
pub mod rclone;
pub mod recent;
pub mod recycle;
pub mod rules;
pub mod sandbox;
pub mod scrub;
pub mod remote;
pub mod repo;
pub mod scripts;
pub mod lan;
pub mod localsend;
pub mod share;
// (window/app module, rewritten for GPUI) mod shared;
pub mod tags;
pub mod sheet;
pub mod shell;
pub mod shellmenu;
pub mod sign;
pub mod sql;
// (window/app module, rewritten for GPUI) mod snap;
pub mod speech;
pub mod startup;
pub mod shrink;
pub mod sizes;
pub mod songtags;
pub mod term;
pub mod textenc;
pub mod thumbs;
pub mod timeline;
pub mod tools;
pub mod usage;
pub mod vault;
pub mod wof;
pub mod verbs;
pub mod versions;
pub mod watch;
// (window/app module, rewritten for GPUI) mod more_windows;
// (WebView2's own installer check: not needed without WebView) mod webview;
pub mod xray;
pub mod zoxide;

use fsx::blocking;
use proc::{Lanes, Slot};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use crate::host::{State, Window};

/// The work in flight: the ripgrep text search, and each tab's file search (fd and the index) and
/// pdu measuring folder sizes or a disk-usage map.
#[derive(Default)]
pub struct AppState {
    pub search: Slot,
    pub finds: find::Finds,
    pub sizes: Lanes,
    pub usage: Lanes,
}

/// Saves `content` to `path`, answering its hash ([`filediff::content_hash`]). With `expected`, only
/// while the file still holds what was read then: a change made elsewhere since isn't overwritten.
pub async fn write_file_content(path: String, content: String, expected: Option<String>) -> Result<String, String> {
    blocking(move || {
        let path = std::path::Path::new(&path);
        if expected.is_some_and(|expected| fs::read(path).map(|now| filediff::content_hash(&now)).ok() != Some(expected)) {
            return Err("it was changed or removed since it was compared. Compare again first".into());
        }
        replace_contents(path, content.as_bytes()).map_err(|e| e.to_string())?;
        Ok(filediff::content_hash(content.as_bytes()))
    })
    .await
}

/// Writes a temp file next to `path` and swaps it in (ReplaceFileW keeps the original's attributes,
/// permissions and streams), so a write cut short by a full disk or a dropped share leaves the file
/// as it was. A folder that doesn't allow a new file gets a plain write.
pub fn replace_contents(path: &std::path::Path, content: &[u8]) -> std::io::Result<()> {
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
    let written = written.and_then(|()| {
        use windows::core::{HSTRING, PCWSTR};
        use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
        // A new file: there's nothing to replace (ReplaceFileW would fail), so the copy takes its name.
        if !path.exists() {
            return fs::rename(&temp, path);
        }
        unsafe { ReplaceFileW(&HSTRING::from(path), &HSTRING::from(temp.as_path()), PCWSTR::null(), REPLACEFILE_IGNORE_MERGE_ERRORS, None, None) }.map_err(std::io::Error::from)
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
pub static IN_TRAY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Started from the taskbar's Quick search task: quick search comes up once the window has painted.
pub static QUICK_AT_START: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What a bug report needs (Settings → About → Copy diagnostics): versions, where the tools come
/// from, and instant search's state. No file names, beyond what an error may quote.
pub async fn diagnostics(app: crate::host::AppHandle) -> Result<String, String> {
    use std::fmt::Write;
    let version = app.package_info().version.to_string();
    blocking(move || {
        let kind = if cfg!(debug_assertions) {
            "dev build"
        } else if packaged() {
            "Microsoft Store package"
        } else if installed_by_setup() {
            "installed by the setup"
        } else {
            "Scoop or portable"
        };
        let mut out = format!("Glint {version} ({kind})\n");
        #[cfg(windows)]
        let _ = writeln!(out, "{}", shadui_index::windows_version());
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
        if let Some(panic) = crash::last() {
            let _ = writeln!(out, "Last panic:\n{panic}");
        }
        Ok(out)
    })
    .await
}

#[derive(Clone, serde::Serialize)]
pub struct SearchEvent {
    pub(crate) id: String,
    pub(crate) r#type: String,
    pub(crate) lines: Vec<String>,
}

/// Text searches in the order they were asked for: one that took longer to start (its folder on a share
/// slow to answer) mustn't replace a newer one, or start after Stop.
pub static TEXT_SEARCH: AtomicU64 = AtomicU64::new(0);

/// Whether text search `ticket` is still the one wanted: no newer search, and no Stop, since.
pub fn text_current(ticket: u64) -> bool {
    TEXT_SEARCH.load(Ordering::SeqCst) == ticket
}

/// The folder a text search runs in (absolute, with %VARS% and ~ expanded), once it's known to be
/// there. Looking can take seconds on a sleeping share, so it's never done on the UI thread.
pub fn text_folder(path: &str) -> Result<String, String> {
    let folder = fsx::normalize(path)?;
    if std::fs::metadata(&folder).is_err() {
        return Err(format!("Can't find “{folder}”. Check the folder path and try again."));
    }
    Ok(folder)
}

pub fn cancel_search(state: State<AppState>) {
    TEXT_SEARCH.fetch_add(1, Ordering::SeqCst);
    state.search.kill();
    doctext::cancel();
}

/// Stops every sidecar that runs (the text search, and each tab's file search, folder sizes and
/// disk-usage map) and waits until they've ended, so an update's installer can replace rg, fd and pdu.
pub async fn stop_tools(state: State<'_, AppState>) -> Result<(), String> {
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
pub struct TextSearch {
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

pub fn rg_args(search: &TextSearch, folder: &str) -> Vec<String> {
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
pub async fn run_ripgrep_batched(
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
pub async fn run_doc_search(window: Window, request: doctext::DocSearch, search_id: String, max_results: Option<usize>, state: State<'_, AppState>) -> Result<String, String> {
    let ticket = TEXT_SEARCH.fetch_add(1, Ordering::SeqCst) + 1;
    let search = state.search.clone();
    fsx::blocking(move || doc_search(window, request, search_id, max_results, search, ticket)).await
}

pub fn doc_search(window: Window, request: doctext::DocSearch, search_id: String, max_results: Option<usize>, search: Slot, ticket: u64) -> Result<String, String> {
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

/// Sends a search's output to the UI as `search-event`s: its lines (each `convert`ed, None to skip
/// one) in batches, until it ends or `max_results` matches came, then an empty line through `convert`
/// (for a summary of its own), any error it gave, and "finished". The search replaces the one before,
/// and Documents' run, unless a newer one (or Stop) came while it started: then it's stopped instead.
pub fn stream_search(window: Window, search_id: String, mut child: std::process::Child, max_results: Option<usize>, search: proc::Slot, ticket: u64, mut convert: impl FnMut(String) -> Option<String> + Send + 'static) -> Result<(), String> {
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
                        lines: std::mem::take(&mut batch),
                    },
                );
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

/// Whether the setup program installed this copy (it leaves `uninstall.exe` next to the app; the Scoop
/// manifest deletes the one 7-Zip extracts). Only those update themselves: a Scoop or portable copy, or
/// a dev build, is updated by whatever put it there, so the app just links to the new release.
#[cfg(desktop)]
pub fn installed_by_setup() -> bool {
    std::env::current_exe().is_ok_and(|exe| exe.with_file_name("uninstall.exe").exists())
}

/// The package this copy runs from, by its family name (`Name_PublisherId`), when it's an MSIX
/// package: the Microsoft Store's, which Windows keeps up to date. Windows keeps such a copy's writes
/// to the current user's registry to itself, so File Explorer's entries and the Run key can't be set
/// from it.
#[cfg(windows)]
pub fn package_family() -> Option<&'static str> {
    use windows::Win32::{Foundation::ERROR_SUCCESS, Storage::Packaging::Appx::GetCurrentPackageFamilyName};
    static FAMILY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FAMILY
        .get_or_init(|| {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            // APPMODEL_ERROR_NO_PACKAGE when it isn't one.
            (unsafe { GetCurrentPackageFamilyName(&mut len, Some(windows::core::PWSTR(name.as_mut_ptr()))) } == ERROR_SUCCESS)
                .then(|| String::from_utf16_lossy(&name[..(len as usize).saturating_sub(1)]))
        })
        .as_deref()
}

#[cfg(not(windows))]
pub fn package_family() -> Option<&'static str> {
    None
}

pub fn packaged() -> bool {
    package_family().is_some()
}

/// The package family when this copy is the Microsoft Store's, installed from the Store (it runs from
/// Program Files\WindowsApps): the instant search service it turns on goes once that package does
/// (shadui_index::store). A test package installed from its folder (npm run msix -- --try) isn't one,
/// so taking it out leaves the service where it is.
#[cfg(windows)]
pub fn store_package() -> Option<&'static str> {
    let family = package_family()?;
    let apps = std::path::Path::new(&std::env::var_os("ProgramFiles")?).join("WindowsApps");
    let exe = std::env::current_exe().ok()?;
    exe.to_string_lossy().to_lowercase().starts_with(&apps.to_string_lossy().to_lowercase()).then_some(family)
}

/// Whether the Microsoft Store keeps this copy up to date (see `package_family`): then the app doesn't
/// look for updates itself.
pub fn store_copy() -> bool {
    packaged()
}

/// Glint started by itself as a helper: reading a JSON, YAML or TOML file for its tree (jq), the benchmark
/// CLI, the document reader for Documents search, SFTP's password prompt, the MCP server an AI agent
/// starts, or the installer's tool installs. Its exit code once it has answered; `None` for the app.
pub fn worker() -> Option<i32> {
    jq::worker().or_else(bench::cli).or_else(doctext::worker).or_else(remote::askpass).or_else(mcp::server).or_else(tools::cli)
}
