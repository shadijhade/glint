//! Instant search: whether its service runs, and turning it on or off. Both changes run the
//! bundled service executable elevated, so Windows asks for administrator approval each time;
//! nothing runs elevated unless the user turns it on here.

use crate::fsx::blocking;
use serde::Serialize;
use crate::host::Window;

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

pub async fn index_status() -> Result<IndexInfo, String> {
    blocking(|| Ok(info())).await
}

/// How many items a saved search finds below `roots`, from the index (the sidebar's counts): up to
/// `max`, and whether there are more. None when the index doesn't cover them or doesn't run.
pub async fn index_count(query: String, roots: Vec<String>, max: usize) -> Result<Option<(u64, bool)>, String> {
    #[cfg(windows)]
    {
        blocking(move || {
            use shadui_index::client::{self, Event};
            let request = shadui_index::proto::SearchRequest { query, roots, hidden: false, exclude: Vec::new(), max_results: Some(max.clamp(1, 100_000)), fuzzy: false, rank: false };
            let mut answer = None;
            let mut covered = false;
            let _ = client::search(&request, |event| {
                match event {
                    Event::Plan { indexed, skipped } => {
                        covered = !indexed.is_empty() && skipped.is_empty();
                        return covered;
                    }
                    Event::Done { count, limited, .. } => answer = Some((count, limited)),
                    Event::Hits(_) => {}
                }
                true
            });
            Ok(answer.filter(|_| covered))
        })
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = (query, roots, max);
        Ok(None)
    }
}

/// What was renamed, moved and deleted on the drives instant search covers, newest first: since
/// `since` (ms), with `query` in a path, at most `max`. Rejects with "tooOld" for a service from
/// before it kept history, and "off" when instant search doesn't run.
/// What instant search's service remembers of renames, moves and deletions.
pub use shadui_index::proto::{HistoryAnswer, HistoryEvent, Kind as HistoryKind};

pub async fn index_history(query: String, since: f64, max: u32) -> Result<HistoryAnswer, String> {
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

/// Updates a running service older than the app to the one that ships with it, without administrator
/// approval: the service takes it only with its signature from the release (update.rs in the index
/// crate). Err with why not when it can't that way (a service from before it could, a build without the
/// signature, or the service refused it), leaving the UAC route.
#[cfg(windows)]
fn update_quietly() -> Result<(), String> {
    use shadui_index::{client, update};
    let running = client::status().map_err(|e| e.to_string())?.version;
    if !update::newer(shadui_index::VERSION, &running) {
        return Err(format!("the service runs {running}, which this app ({}) doesn't update", shadui_index::VERSION));
    }
    let exe = bundled()?;
    let signature = std::fs::read_to_string(exe.with_file_name(update::SIGNATURE_FILE)).map_err(|_| "this build has no signature for its service".to_string())?;
    let bytes = std::fs::read(&exe).map_err(|e| e.to_string())?;
    match client::update(&bytes, &signature).map_err(|e| e.to_string())? {
        Some(version) => client::wait_for_version(&version, std::time::Duration::from_secs(120)).map(drop).map_err(|e| e.to_string()),
        None => Err(format!("the service ({running}) is from before it could update itself")),
    }
}

/// Updates an older service to this app's without administrator approval (update_quietly); rejects
/// with why not, for the UI to offer the UAC route instead.
pub async fn index_update() -> Result<IndexInfo, String> {
    #[cfg(windows)]
    {
        blocking(|| {
            update_quietly()?;
            Ok(info())
        })
        .await
    }
    #[cfg(not(windows))]
    {
        Err("Instant search needs Windows and NTFS".into())
    }
}

/// Installs (or updates) and starts the service for this user, after the UAC prompt; an update goes
/// without the prompt when the service can take it by itself. Rejects with "cancelled" when the prompt
/// is declined.
pub async fn index_enable(window: Window) -> Result<IndexInfo, String> {
    #[cfg(windows)]
    {
        let owner = window.hwnd().ok().map(|h| h.0 as isize);
        blocking(move || {
            if update_quietly().is_ok() {
                return Ok(info());
            }
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
