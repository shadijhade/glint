//! Watches the open folder so the list refreshes when files change elsewhere, like Explorer.

use crate::fsx::blocking;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Emitter, State, Window};

#[derive(Default)]
pub struct FolderWatcher(Arc<Mutex<Option<RecommendedWatcher>>>);

/// Watches `path` (nothing when None), emitting "dir-changed" with the path at most every 200 ms.
#[tauri::command]
pub async fn watch_dir(window: Window, state: State<'_, FolderWatcher>, path: Option<String>) -> Result<(), String> {
    let slot = state.0.clone();
    blocking(move || {
        let mut current = slot.lock().unwrap_or_else(|e| e.into_inner());
        *current = None;
        let Some(path) = path else { return Ok(()) };
        let pending = Arc::new(AtomicBool::new(false));
        let changed = path.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            // A build writing thousands of files becomes one refresh, not thousands.
            if event.is_ok() && !pending.swap(true, Ordering::SeqCst) {
                let (window, path, pending) = (window.clone(), changed.clone(), pending.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(200));
                    pending.store(false, Ordering::SeqCst);
                    let _ = window.emit("dir-changed", path);
                });
            }
        })
        .map_err(|e| e.to_string())?;
        watcher.watch(Path::new(&path), RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;
        *current = Some(watcher);
        Ok(())
    })
    .await
}
