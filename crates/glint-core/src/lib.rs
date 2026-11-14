//! Glint's engine: everything that isn't a window. Until 0.10 this was the Tauri app's src-tauri; `host` stands in
//! for the little of Tauri the modules used.

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
pub mod metadata;
pub mod media;
pub mod media_tools;
pub mod newmenu;
// (window/app module, rewritten for GPUI) mod native_preview;
pub mod ocr;
pub mod ondemand;
pub mod openers;
pub mod pdf_tools;
pub mod pe;
// (window/app module, rewritten for GPUI) mod print;
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
pub mod retired;
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
pub mod whisper;
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

/// One line a search inside files found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextMatch {
    /// The file; empty when ripgrep gave its name as bytes.
    pub path: String,
    /// The line's text with its line end, or None when ripgrep gave it as bytes.
    pub text: Option<String>,
    /// The line's number, from 1.
    pub line_number: Option<u64>,
    /// The hits in the line: UTF-8 byte ranges of `text`.
    pub hits: Vec<(usize, usize)>,
    /// A Documents search's match: where it is in the document ("p. 3", "slide 2", "Sales, row 7"). None shows
    /// the line by its number.
    pub label: Option<String>,
}

/// What a search inside files says as it goes, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum TextSaid {
    /// Lines found.
    Matches(Vec<TextMatch>),
    /// The end-of-search summary: how long it took, in milliseconds.
    Summary { elapsed_ms: Option<f64> },
    /// The search failed, with the tool's words.
    Error(String),
    /// The search is over.
    Finished,
}

/// A search inside files' news: the search it belongs to, and what it said.
#[derive(Clone, Debug)]
pub struct SearchEvent {
    pub id: String,
    pub said: TextSaid,
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

/// A text search as the window asks for it. ripgrep's arguments are built here from these, never passed
/// through (a flag like `--pre` would run a program).
pub struct TextSearch {
    pub query: String,
    pub path: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub exclusions: Vec<String>,
    /// Searching again, skipping nothing: hidden files, what .gitignore files list and the exclusions too.
    pub everything: bool,
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
        stream_search(window, search_id, child, max_results, search, ticket)?;
        Ok(folder)
    })
    .await
}

/// The longest match line sent as ripgrep wrote it. Its JSON printer ignores --max-columns, so a hit in a
/// minified file or a one-line log would carry the whole line, megabytes of it, to the window.
const LONGEST_MATCH_LINE: usize = 4096;
/// A cut line keeps this much before its first hit, and this much after where the hit starts.
const CUT_BEFORE: usize = 200;
const CUT_AFTER: usize = 1000;

/// One line of ripgrep's JSON, as far as it's read.
#[derive(serde::Deserialize)]
struct RgLine {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    data: RgData,
}

#[derive(Default, serde::Deserialize)]
struct RgData {
    path: Option<RgText>,
    lines: Option<RgText>,
    line_number: Option<u64>,
    #[serde(default)]
    submatches: Vec<RgHit>,
    elapsed_total: Option<RgElapsed>,
}

/// Text ripgrep gives as `{"text": …}`, or as `{"bytes": …}` when it isn't UTF-8 (then None).
#[derive(serde::Deserialize)]
struct RgText {
    text: Option<String>,
}

#[derive(serde::Deserialize)]
struct RgHit {
    start: u64,
    end: u64,
}

#[derive(serde::Deserialize)]
struct RgElapsed {
    #[serde(default)]
    secs: u64,
    #[serde(default)]
    nanos: u64,
}

/// What one line of ripgrep's JSON says: a match (a long one cut around its first hit) or the summary. None for
/// the begin, end and context lines, and for anything that isn't ripgrep's JSON.
fn rg_said(line: &str) -> Option<TextSaid> {
    // Most lines are told apart without parsing them: the JSON starts with the type field, except ripgrep 15's
    // summary, which ends with it.
    if !line.starts_with(r#"{"type":"match""#) && !is_rg_summary(line) {
        return None;
    }
    let parsed: RgLine = serde_json::from_str(line).ok()?;
    match parsed.kind.as_str() {
        "match" => {
            let data = parsed.data;
            let mut found = TextMatch {
                path: data.path.and_then(|p| p.text).unwrap_or_default(),
                text: data.lines.and_then(|l| l.text),
                line_number: data.line_number,
                // Offsets that don't fit are dropped, not guessed.
                hits: data.submatches.iter().filter_map(|h| Some((usize::try_from(h.start).ok()?, usize::try_from(h.end).ok()?))).collect(),
                label: None,
            };
            cut_long(&mut found);
            Some(TextSaid::Matches(vec![found]))
        }
        "summary" => Some(TextSaid::Summary { elapsed_ms: parsed.data.elapsed_total.map(|e| e.secs as f64 * 1000. + e.nanos as f64 / 1e6) }),
        _ => None,
    }
}

/// ripgrep 15 writes the summary's fields in another order than the rest (`{"data":…,"type":"summary"}`).
fn is_rg_summary(line: &str) -> bool {
    line.starts_with(r#"{"type":"summary""#) || line.ends_with(r#""type":"summary"}"#)
}

/// A match whose line is longer than `LONGEST_MATCH_LINE`, cut to a window around its first hit ("…" marks each
/// cut end), with the hits' offsets moved to match and those outside dropped.
fn cut_long(found: &mut TextMatch) {
    let Some(text) = found.text.as_deref().filter(|text| text.len() > LONGEST_MATCH_LINE) else { return };
    let first = found.hits.first().map_or(0, |&(start, _)| start).min(text.len());
    let boundary_at_or_before = |mut at: usize| {
        while at > 0 && !text.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let from = boundary_at_or_before(first.saturating_sub(CUT_BEFORE));
    let to = boundary_at_or_before(first.saturating_add(CUT_AFTER).min(text.len()));
    let head = if from > 0 { "…" } else { "" };
    let tail = if to < text.len() { "…" } else { "" };
    let shown = format!("{head}{}{tail}", &text[from..to]);
    found.hits.retain_mut(|(start, end)| {
        if *start < from || *end > to {
            return false;
        }
        (*start, *end) = (*start - from + head.len(), *end - from + head.len());
        true
    });
    found.text = Some(shown);
}

/// A Documents search: PDFs', Office's, OpenDocument's and EPUBs' text, read by worker processes (doctext.rs).
/// Its matches arrive as `Event::Search`es, each with a label ("p. 3"), then a summary and the end. Returns the
/// folder as searched.
pub async fn run_doc_search(window: Window, request: doctext::DocSearch, search_id: String, max_results: Option<usize>, state: State<'_, AppState>) -> Result<String, String> {
    let ticket = TEXT_SEARCH.fetch_add(1, Ordering::SeqCst) + 1;
    let search = state.search.clone();
    fsx::blocking(move || doc_search(window, request, search_id, max_results, search, ticket)).await
}

pub fn doc_search(window: Window, request: doctext::DocSearch, search_id: String, max_results: Option<usize>, search: Slot, ticket: u64) -> Result<String, String> {
    let folder = text_folder(&request.path)?;
    let pattern = doctext::pattern(&request)?;
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
        let emit = |said: TextSaid| window.emit(host::Event::Search(SearchEvent { id: search_id.clone(), said }));
        doctext::search(run, &base, &request, &pattern, max, emit);
        emit(TextSaid::Finished);
    });
    Ok(folder)
}

/// Sends what ripgrep finds to the window as `Event::Search`es: its matches in batches, until it ends or
/// `max_results` matches came, then its summary, any error it gave, and the end. The search replaces the one
/// before, and Documents' run, unless a newer one (or Stop) came while it started: then it's stopped instead.
pub fn stream_search(window: Window, search_id: String, mut child: std::process::Child, max_results: Option<usize>, search: proc::Slot, ticket: u64) -> Result<(), String> {
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
        let emit = |said: TextSaid| window.emit(host::Event::Search(SearchEvent { id: search_id.clone(), said }));
        let reader = BufReader::new(stdout);
        let mut batch: Vec<TextMatch> = Vec::new();
        let mut last_emit = Instant::now();
        let mut total_matches = 0;
        let limit = match max_results {
            Some(0) | None => usize::MAX,
            Some(n) => n,
        };
        for line in reader.lines() {
            let Ok(line) = line else { break };
            match rg_said(&line) {
                Some(TextSaid::Matches(mut found)) => {
                    total_matches += found.len();
                    batch.append(&mut found);
                }
                // The summary comes after every match it counts.
                Some(said) => {
                    if !batch.is_empty() {
                        emit(TextSaid::Matches(std::mem::take(&mut batch)));
                    }
                    emit(said);
                }
                None => continue,
            }
            if !batch.is_empty() && (batch.len() >= 1000 || last_emit.elapsed() >= Duration::from_millis(50)) {
                emit(TextSaid::Matches(std::mem::take(&mut batch)));
                last_emit = Instant::now();
            }
            if total_matches >= limit {
                // Limit reached: kill this search — but not a newer one that already replaced it.
                search.kill_if(pid);
                break;
            }
        }
        if !batch.is_empty() {
            emit(TextSaid::Matches(batch));
        }

        // Surface real failures (e.g. an invalid regex); per-file errors are silenced by --no-messages.
        let errors = stderr_reader.join().unwrap_or_default();
        search.finish(pid);
        if total_matches == 0 && !errors.trim().is_empty() {
            emit(TextSaid::Error(errors.trim().to_string()));
        }
        emit(TextSaid::Finished);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn match_line(text: &str, hits: &[(usize, usize)]) -> String {
        let submatches: Vec<serde_json::Value> = hits.iter().map(|(s, e)| serde_json::json!({"match": {"text": &text[*s..*e]}, "start": s, "end": e})).collect();
        format!(r#"{{"type":"match","data":{}}}"#, serde_json::json!({"path": {"text": "C:\\a.min.js"}, "lines": {"text": text}, "line_number": 1, "submatches": submatches}))
    }

    fn one_match(said: Option<TextSaid>) -> TextMatch {
        match said {
            Some(TextSaid::Matches(mut found)) if found.len() == 1 => found.remove(0),
            other => panic!("one match: {other:?}"),
        }
    }

    #[test]
    fn ripgreps_matches_and_summary_are_read_and_the_rest_is_skipped() {
        assert_eq!(rg_said(r#"{"type":"begin","data":{}}"#), None);
        assert_eq!(rg_said(r#"{"type":"end","data":{}}"#), None);
        assert_eq!(rg_said(""), None);
        assert_eq!(rg_said("not json"), None);
        let found = one_match(rg_said(&match_line("    the needle is here\n", &[(8, 14)])));
        assert_eq!(
            found,
            TextMatch { path: "C:\\a.min.js".into(), text: Some("    the needle is here\n".into()), line_number: Some(1), hits: vec![(8, 14)], label: None }
        );
        assert_eq!(rg_said(r#"{"type":"summary","data":{"elapsed_total":{"secs":1,"nanos":500000000}}}"#), Some(TextSaid::Summary { elapsed_ms: Some(1500.) }));
        // ripgrep 15 writes the summary with its type last.
        assert_eq!(rg_said(r#"{"data":{"elapsed_total":{"secs":2,"nanos":0}},"type":"summary"}"#), Some(TextSaid::Summary { elapsed_ms: Some(2000.) }));
        // A line given as bytes has no text, and still counts.
        let bytes = one_match(rg_said(r#"{"type":"match","data":{"path":{"text":"C:\\b.bin"},"lines":{"bytes":"/w=="},"line_number":3,"submatches":[]}}"#));
        assert_eq!((bytes.text, bytes.line_number), (None, Some(3)));
    }

    #[test]
    fn a_long_match_line_is_cut_around_its_first_hit() {
        let text = format!("{}needle{}é{}", "x".repeat(5000), "y".repeat(500), "z".repeat(5000));
        let at = 5000;
        let far = text.len() - 10;
        let found = one_match(rg_said(&match_line(&text, &[(at, at + 6), (far, far + 4)])));
        let shown = found.text.as_deref().unwrap();
        assert!(shown.len() < LONGEST_MATCH_LINE);
        assert!(shown.starts_with('…') && shown.ends_with('…'));
        assert_eq!(found.hits.len(), 1, "a hit outside what's kept is dropped");
        let (start, end) = found.hits[0];
        assert_eq!(&shown[start..end], "needle");
        assert_eq!(found.line_number, Some(1));
    }
}
