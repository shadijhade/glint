//! Instant search: whether its service runs, and turning it on or off. Both changes run the
//! bundled service executable elevated, so Windows asks for administrator approval each time;
//! nothing runs elevated unless the user turns it on here.

use crate::fsx::blocking;
use serde::Serialize;
use tauri::Window;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IndexInfo {
    /// "on", "off" (not installed), "stopped", "noAccess" (turned on for another user), "untrusted"
    /// (something else holds the service's pipe), or "unsupported" (not Windows).
    pub state: &'static str,
    /// The running service's version and this app's, to offer an update when they differ.
    pub version: Option<String>,
    pub app_version: &'static str,
    pub volumes: Vec<shadui_index::proto::VolumeStatus>,
    pub error: Option<String>,
}

#[cfg(windows)]
pub(crate) fn info() -> IndexInfo {
    use shadui_index::client::{self, Error};
    use std::sync::atomic::{AtomicBool, Ordering};
    // A copy that isn't the Microsoft Store's says once that it uses the service, so it stays when the
    // Store's copy that installed it goes; a service from before closes the pipe on that.
    static CLAIMED: AtomicBool = AtomicBool::new(false);
    let mut info = IndexInfo { app_version: shadui_index::VERSION, ..Default::default() };
    let status = if crate::store_package().is_none() && !CLAIMED.load(Ordering::SeqCst) {
        match client::claim() {
            Ok(Some(status)) => {
                CLAIMED.store(true, Ordering::SeqCst);
                Ok(status)
            }
            // A service from before has nothing to keep.
            Ok(None) | Err(Error::Io(_)) => {
                CLAIMED.store(true, Ordering::SeqCst);
                client::status()
            }
            Err(e) => Err(e),
        }
    } else {
        client::status()
    };
    match status {
        Ok(status) => {
            info.state = "on";
            info.version = Some(status.version);
            info.volumes = status.volumes;
        }
        Err(Error::NoAccess) => info.state = "noAccess",
        Err(Error::Untrusted(owner)) => {
            info.state = "untrusted";
            info.error = Some(format!("The pipe is owned by {owner}"));
        }
        Err(e) => {
            info.state = if client::installed() { "stopped" } else { "off" };
            if !matches!(e, Error::NotRunning) {
                info.error = Some(e.to_string());
            }
        }
    }
    info
}

#[cfg(not(windows))]
pub(crate) fn info() -> IndexInfo {
    IndexInfo { state: "unsupported", app_version: shadui_index::VERSION, ..Default::default() }
}

#[tauri::command]
pub async fn index_status() -> Result<IndexInfo, String> {
    blocking(|| Ok(info())).await
}

/// What was renamed, moved and deleted on the drives instant search covers, newest first: since
/// `since` (ms), with `query` in a path, at most `max`. Rejects with "tooOld" for a service from
/// before it kept history, and "off" when instant search doesn't run.
#[tauri::command]
pub async fn index_history(query: String, since: f64, max: u32) -> Result<shadui_index::proto::HistoryAnswer, String> {
    #[cfg(windows)]
    {
        use shadui_index::client::{history, Error};
        use shadui_index::proto::HistoryRequest;
        blocking(move || match history(&HistoryRequest { query, since: since as i64, max }) {
            Ok(answer) => Ok(answer),
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err("tooOld".into()),
            Err(Error::NotRunning) => Err("off".into()),
            Err(e) => Err(e.to_string()),
        })
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = (query, since, max);
        Err("off".into())
    }
}

/// Every entry listed from the index, in a folder Windows keeps closed to this user: read-only, and
/// it can't be opened (Glint's own bit, which Windows doesn't use; `ATTR.closed` in the UI).
pub const CLOSED: u32 = 0x8000_0000;

/// What's right inside a folder Windows keeps this user out of, from the index (which the service
/// reads as the system): None when instant search is off, hasn't got the drive, or is from before it
/// could list folders.
#[cfg(windows)]
pub(crate) fn listing(path: &str) -> Option<Vec<crate::fsx::Entry>> {
    let hits = shadui_index::client::list(path).ok()?;
    let entries = hits.into_iter().map(|hit| {
        let name = hit.path.rsplit('\\').next().unwrap_or_default().to_string();
        // As a folder's own listing has them: a folder's size is measured apart.
        crate::fsx::Entry { name, path: None, dir: hit.dir, size: if hit.dir { 0 } else { hit.size }, modified: hit.modified, attrs: hit.attrs | CLOSED }
    });
    Some(entries.collect())
}

#[cfg(not(windows))]
pub(crate) fn listing(_: &str) -> Option<Vec<crate::fsx::Entry>> {
    None
}

/// The service executable that ships next to the app's own. A dev build (`tauri dev` builds only the
/// app) also takes one from the other build profile's folder. Whatever this returns is installed as a
/// LocalSystem service (or run elevated to get into a folder), so release builds never look anywhere else.
#[cfg(windows)]
pub(crate) fn bundled() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let service = exe.with_file_name(shadui_index::SERVICE_EXE);
    if service.exists() {
        return Ok(service);
    }
    if cfg!(debug_assertions) {
        if let Some(other) = ["..\\release", "..\\debug"].iter().map(|rel| exe.with_file_name(rel).join(shadui_index::SERVICE_EXE)).find(|p| p.exists()) {
            return Ok(other);
        }
    }
    Err(format!("{} is missing next to the app; reinstall the app", shadui_index::SERVICE_EXE))
}

/// Installs (or updates) and starts the service for this user, after the UAC prompt.
/// Rejects with "cancelled" when the prompt is declined.
#[tauri::command]
pub async fn index_enable(window: Window) -> Result<IndexInfo, String> {
    #[cfg(windows)]
    {
        let owner = window.hwnd().ok().map(|h| h.0 as isize);
        blocking(move || {
            shadui_index::client::enable(&bundled()?, owner, crate::store_package())?;
            Ok(info())
        })
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Err("Instant search needs Windows and NTFS".into())
    }
}

/// Stops and removes the service and its index files, after the UAC prompt.
#[tauri::command]
pub async fn index_disable(window: Window) -> Result<IndexInfo, String> {
    #[cfg(windows)]
    {
        let owner = window.hwnd().ok().map(|h| h.0 as isize);
        blocking(move || {
            shadui_index::client::disable(&bundled()?, owner)?;
            Ok(info())
        })
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Err("Instant search needs Windows and NTFS".into())
    }
}
