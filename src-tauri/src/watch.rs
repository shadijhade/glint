//! Watches the folders on screen (one per visible pane) so their lists refresh when files change
//! elsewhere, like Explorer.

use crate::fsx::blocking;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, State, Window};

/// One watcher per folder, by its lowercase path.
#[derive(Default)]
pub struct FolderWatcher(Arc<Mutex<HashMap<String, RecommendedWatcher>>>);

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

/// Watches exactly `paths`: folders no longer listed stop being watched, the rest keep their
/// watcher. Each emits "dir-changed" with its path, at most every 200 ms.
#[tauri::command]
pub async fn watch_dirs(window: Window, state: State<'_, FolderWatcher>, paths: Vec<String>) -> Result<(), String> {
    let watchers = state.0.clone();
    blocking(move || {
        let mut watchers = watchers.lock().unwrap_or_else(|e| e.into_inner());
        let wanted: HashSet<String> = paths.iter().map(|p| key(p)).collect();
        watchers.retain(|k, _| wanted.contains(k));
        let mut failed = Ok(());
        for path in paths {
            if watchers.contains_key(&key(&path)) || removable(&path) {
                continue;
            }
            match watch(&window, &path) {
                Ok(watcher) => {
                    watchers.insert(key(&path), watcher);
                }
                Err(e) => failed = Err(e),
            }
        }
        failed
    })
    .await
}

/// A folder on a USB stick or memory card isn't watched: the open handle would keep Windows from
/// ejecting it ("This device is currently in use"). F5 refreshes it, and Glint's own changes show anyway.
/// ponytail: USB hard drives report as fixed, so they're still watched; closing handles on
/// DBT_DEVICEQUERYREMOVE would cover them too.
fn removable(path: &str) -> bool {
    #[cfg(windows)]
    if let Some(drive) = path.get(..2).filter(|d| d.ends_with(':')) {
        use windows::core::HSTRING;
        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
        const DRIVE_REMOVABLE: u32 = 2;
        return unsafe { GetDriveTypeW(&HSTRING::from(format!("{drive}\\"))) } == DRIVE_REMOVABLE;
    }
    let _ = path;
    false
}

fn watch(window: &Window, path: &str) -> Result<RecommendedWatcher, String> {
    let pending = Arc::new(AtomicBool::new(false));
    let (window, changed) = (window.clone(), path.to_string());
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // A build writing thousands of files becomes one refresh, not thousands.
        if let Ok(ev) = event {
            crate::timeline::record_notify_event(&ev);
            if !pending.swap(true, Ordering::SeqCst) {
                let (window, path, pending) = (window.clone(), changed.clone(), pending.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    pending.store(false, Ordering::SeqCst);
                    let _ = window.emit("dir-changed", path);
                });
            }
        }
    })
    .map_err(|e| e.to_string())?;
    watcher.watch(Path::new(path), RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;
    Ok(watcher)
}
