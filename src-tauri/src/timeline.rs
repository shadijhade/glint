//! Filesystem "Time Machine" and live Flight Recorder.
//!
//! 1. Maintains a bounded in-memory ring buffer (`MAX_LIVE_EVENTS = 500`) of real-time filesystem
//!    events (`created`, `modified`, `deleted`) captured from `ReadDirectoryChangesW` (`notify`).
//! 2. Scans any folder tree using `fsx::read_dir` (`FindFirstFileExW`) across a chosen time horizon
//!    (e.g. last 15m, 1h, 24h, 7d) in milliseconds, ranking the fastest-growing subfolders and listing
//!    every file touched in chronological order.

use crate::fsx::{self, blocking};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const MAX_LIVE_EVENTS: usize = 500;
const MAX_SCAN_FILES: usize = 150_000;
const MAX_TIMELINE_ITEMS: usize = 400;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveFsEvent {
    pub path: String,
    pub name: String,
    /** "created", "modified", or "deleted". */
    pub kind: String,
    pub timestamp_ms: i64,
    pub size: Option<u64>,
    pub dir: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineEntry {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub size: u64,
    pub modified: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotSubfolder {
    pub path: String,
    pub name: String,
    pub bytes_changed: u64,
    pub files_changed: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineReport {
    pub root: String,
    pub window_ms: i64,
    pub entries: Vec<TimelineEntry>,
    pub hot_folders: Vec<HotSubfolder>,
    pub live_events: Vec<LiveFsEvent>,
    pub total_bytes_changed: u64,
    pub total_files_changed: usize,
    pub files_scanned: usize,
    /// It stopped at MAX_SCAN_FILES: the folder has more files than it looked at.
    pub stopped_early: bool,
    pub elapsed_ms: f64,
}

static FLIGHT_RECORDER: LazyLock<Mutex<VecDeque<LiveFsEvent>>> = LazyLock::new(|| Mutex::new(VecDeque::with_capacity(MAX_LIVE_EVENTS)));

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Called by `watch.rs` when `notify` delivers a filesystem event on a watched directory.
pub fn record_notify_event(event: &notify::Event) {
    use notify::EventKind;
    let kind = match event.kind {
        EventKind::Create(_) => "created",
        EventKind::Modify(_) => "modified",
        EventKind::Remove(_) => "deleted",
        _ => return,
    };
    let ts = now_ms();
    // The disk is asked before the lock is taken, so a burst of events doesn't hold it up.
    let seen: Vec<_> = event
        .paths
        .iter()
        .map(|p| {
            let path_str = p.to_string_lossy().into_owned();
            let name = p.file_name().map_or_else(|| path_str.clone(), |n| n.to_string_lossy().into_owned());
            let (size, dir) = match std::fs::metadata(p) {
                Ok(m) => (if m.is_dir() { None } else { Some(m.len()) }, m.is_dir()),
                Err(_) => (None, false),
            };
            (path_str, name, size, dir)
        })
        .collect();
    let mut log = FLIGHT_RECORDER.lock().unwrap_or_else(|e| e.into_inner());
    for (path_str, name, size, dir) in seen {
        // Coalesce consecutive identical events on the same path within 500 ms.
        if let Some(front) = log.front_mut() {
            if front.path.eq_ignore_ascii_case(&path_str) && front.kind == kind && (ts - front.timestamp_ms).abs() < 500 {
                front.timestamp_ms = ts;
                front.size = size;
                continue;
            }
        }
        if log.len() >= MAX_LIVE_EVENTS {
            log.pop_back();
        }
        log.push_front(LiveFsEvent {
            path: path_str,
            name,
            kind: kind.to_string(),
            timestamp_ms: ts,
            size,
            dir,
        });
    }
}

static CANCEL: AtomicBool = AtomicBool::new(false);

/// Closing Recent changes stops its scan.
#[tauri::command]
pub fn cancel_timeline() {
    CANCEL.store(true, Ordering::Relaxed);
}

#[tauri::command]
pub async fn scan_timeline(root: String, window_ms: i64) -> Result<TimelineReport, String> {
    CANCEL.store(false, Ordering::Relaxed);
    blocking(move || build_timeline(&root, window_ms, &CANCEL)).await
}

pub fn build_timeline(root: &str, window_ms: i64, cancel: &AtomicBool) -> Result<TimelineReport, String> {
    let start = Instant::now();
    if !Path::new(root).exists() {
        return Err("Folder does not exist".into());
    }

    let now = now_ms();
    let cutoff = now.saturating_sub(window_ms.max(60_000));
    let clean_root = root.trim_end_matches(['\\', '/']).to_string();
    let root_lower = clean_root.to_lowercase();

    let mut queue = VecDeque::new();
    queue.push_back(clean_root.clone());

    let mut files_scanned = 0usize;
    let mut total_bytes_changed = 0u64;
    let mut total_files_changed = 0usize;
    let mut entries = Vec::new();
    let mut by_folder: HashMap<String, (String, u64, usize)> = HashMap::new();

    while let Some(dir) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if files_scanned >= MAX_SCAN_FILES {
            break;
        }
        let Ok(list) = fsx::read_dir(&dir) else {
            continue;
        };
        for item in list {
            // Skip reparse points/symlinks.
            if (item.attrs & 0x0400) != 0 {
                continue;
            }
            let full_path = if dir.ends_with('\\') || dir.ends_with('/') {
                format!("{dir}{}", item.name)
            } else {
                format!(r"{dir}\{}", item.name)
            };
            if item.dir {
                let lower = item.name.to_ascii_lowercase();
                if lower != ".git" && lower != "$recycle.bin" && lower != "system volume information" {
                    queue.push_back(full_path);
                }
            } else {
                files_scanned += 1;
                if item.modified >= cutoff {
                    total_files_changed += 1;
                    total_bytes_changed = total_bytes_changed.saturating_add(item.size);
                    let folder_key = dir.to_lowercase();
                    let entry_ref = by_folder.entry(folder_key).or_insert_with(|| (dir.clone(), 0, 0));
                    entry_ref.1 = entry_ref.1.saturating_add(item.size);
                    entry_ref.2 += 1;

                    entries.push(TimelineEntry {
                        path: full_path,
                        name: item.name,
                        folder: dir.clone(),
                        size: item.size,
                        modified: item.modified,
                    });
                }
            }
        }
    }

    entries.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| b.size.cmp(&a.size)));
    entries.truncate(MAX_TIMELINE_ITEMS);

    let mut hot_folders: Vec<HotSubfolder> = by_folder
        .into_values()
        .map(|(path, bytes_changed, files_changed)| {
            let name = Path::new(&path)
                .file_name()
                .map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
            HotSubfolder {
                path,
                name,
                bytes_changed,
                files_changed,
            }
        })
        .collect();
    hot_folders.sort_by(|a, b| b.bytes_changed.cmp(&a.bytes_changed).then_with(|| b.files_changed.cmp(&a.files_changed)));
    hot_folders.truncate(8);

    let live_events: Vec<LiveFsEvent> = FLIGHT_RECORDER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|ev| {
            // In the folder or below it: C:\foo, not C:\foobar.
            let path = ev.path.to_lowercase();
            path.strip_prefix(&root_lower).is_some_and(|rest| rest.is_empty() || rest.starts_with(['\\', '/']))
        })
        .take(100)
        .cloned()
        .collect();

    Ok(TimelineReport {
        root: root.to_string(),
        window_ms,
        entries,
        hot_folders,
        live_events,
        total_bytes_changed,
        total_files_changed,
        files_scanned,
        stopped_early: files_scanned >= MAX_SCAN_FILES,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_recent_modifications_and_hot_folders() {
        let dir = std::env::temp_dir().join(format!("glint-timeline-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("logs");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("app.log"), vec![b'x'; 4096]).unwrap();

        let report = build_timeline(&dir.to_string_lossy(), 3_600_000, &AtomicBool::new(false)).unwrap();
        assert_eq!(report.total_files_changed, 1);
        assert_eq!(report.total_bytes_changed, 4096);
        assert_eq!(report.entries[0].name, "app.log");
        assert_eq!(report.hot_folders.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
