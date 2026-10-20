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
fn info() -> IndexInfo {
    use shadui_index::client::{self, Error};
    let mut info = IndexInfo { app_version: shadui_index::VERSION, ..Default::default() };
    match client::status() {
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
fn info() -> IndexInfo {
    IndexInfo { state: "unsupported", app_version: shadui_index::VERSION, ..Default::default() }
}

#[tauri::command]
pub async fn index_status() -> Result<IndexInfo, String> {
    blocking(|| Ok(info())).await
}

/// The service executable that ships next to the app's own. A dev build (`tauri dev` builds only the
/// app) also takes one from the other build profile's folder. Whatever this returns is installed as a
/// LocalSystem service, so release builds never look anywhere else.
#[cfg(windows)]
fn bundled() -> Result<std::path::PathBuf, String> {
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
            shadui_index::client::enable(&bundled()?, owner)?;
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
