//! Where the explorer gets folders from: glint-core in the app, a fake in tests.
//! Ported from what src/lib/explorer.ts called in src/lib/fs.ts (`listDir`, `folderStamp`, `listDrives`,
//! `resolvePath`, `openItem`, `grantAccess`, `watchDirs`) and from the Tauri path API (`homeDir`…).
//!
//! The model never calls glint-core directly. It holds a [`Source`] (the same pattern as Drive health's
//! `Source`), so the model tests list a fake disk and never touch the real one. Every method returns a
//! future the model awaits in a GPUI task: [`CoreSource`] runs them on core's runtime through
//! [`core_bridge::run`], so the UI thread never waits, and decodes big listings off the UI thread too.

use super::types::*;
use crate::core_bridge;
use glint_core::host::{AppHandle, Manager as _, Window};
use std::future::Future;
use std::pin::Pin;

/// A future the model awaits on the UI thread (so it needn't be `Send`).
pub type Fut<T> = Pin<Box<dyn Future<Output = T>>>;

/// A typed location, checked (`resolvePath`): a file resolves too, so its folder can be opened with it selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub path: String,
    pub dir: bool,
}

/// What Get access came to (`grantAccess`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The folder opens now.
    Granted,
    /// Even administrators may not change who opens it.
    Refused,
    /// The account may open it now, yet a rule in its permissions denies it.
    StillDenied,
}

/// Things the model does that nobody waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// The folder shown, for Ctrl+Alt+G in a file dialog (`report_folder`, dialogs.rs).
    ReportFolder(String),
    /// The folders shown now, for the next start to read ahead (`startup_hint`, startup.rs).
    StartupHint(Vec<String>),
    /// The first folder was painted: with `GLINT_BENCH_START` set, the time since start is written and Glint
    /// quits (`bench_first_folder`, startup.rs; a no-op otherwise).
    BenchFirstFolder,
    /// A folder opened goes to zoxide, when history is shared with it (`zoxide_add`).
    ZoxideAdd(String),
}

/// Everything the explorer reads from or tells to the outside world.
pub trait Source {
    /// A folder (or archive, remote, Recycle Bin, Recent, Gallery) listed and decoded. Never called for This PC.
    fn list(&self, path: &str) -> Fut<Result<Listed, ListFailure>>;
    /// The folder's own last-write time in ms (`folderStamp`): a prefetched listing is good while it's the same.
    fn stamp(&self, path: &str) -> Fut<Option<f64>>;
    /// The drives and WSL distributions (`listDrives`).
    fn drives(&self) -> Fut<Result<Vec<Drive>, String>>;
    /// Home, Desktop, Documents… with their paths (the ones this PC has).
    fn known_folders(&self) -> Fut<Vec<KnownFolder>>;
    /// Checks a typed location.
    fn resolve(&self, text: &str) -> Fut<Result<Resolved, String>>;
    /// Opens a file as a double-click in File Explorer does; a shortcut to a folder comes back as that folder.
    fn open_item(&self, path: &str) -> Fut<Result<Option<String>, String>>;
    /// Get access to a folder Windows keeps closed. `Err("cancelled")` when the approval was declined.
    fn grant_access(&self, path: &str) -> Fut<Result<Access, String>>;
    /// Watches exactly these folders (`watchDirs`); each change raises core's "dir-changed". Returns whether
    /// it took (false when there is no window yet to watch for: the model asks again at its next change).
    fn watch(&self, folders: Vec<String>) -> bool;
    /// Fire and forget.
    fn effect(&self, effect: Effect);
}

// ---- Core ------------------------------------------------------------------------------------------------

/// [`Source`] backed by glint-core.
pub struct CoreSource {
    app: AppHandle,
}

impl CoreSource {
    /// A source for the app handle `core_bridge::init` made (`cx.global::<Core>().app.clone()`).
    pub fn new(app: AppHandle) -> Self {
        CoreSource { app }
    }

    /// The window shell dialogs and watchers belong to: the main window, else any.
    fn window(&self) -> Option<Window> {
        self.app
            .get_webview_window(crate::workspace::MAIN)
            .or_else(|| self.app.webview_windows().into_values().next())
    }
}

/// core's answer for a folder, decoded. Runs on a blocking thread: 100,000 names take a while to decode.
fn decode(path: &str, bytes: &[u8]) -> Result<Listed, ListFailure> {
    glint_model::entries::decode_listing(bytes)
        .map(Listed::from)
        .map_err(|e| ListFailure::other(path, format!("Glint couldn't read the folder's listing: {e}")))
}

impl Source for CoreSource {
    fn list(&self, path: &str) -> Fut<Result<Listed, ListFailure>> {
        let path = path.to_string();
        let job = async move {
            let asked = path.clone();
            let response = glint_core::fsx::list_dir(path).await.map_err(|e| ListFailure {
                kind: if e.kind == "denied" {
                    FailureKind::Denied
                } else {
                    FailureKind::Other
                },
                message: e.message,
                path: e.path,
                remote: e.remote,
            })?;
            let bytes = response.into_bytes();
            let decoded = glint_core::fsx::blocking(move || Ok(decode(&asked, &bytes))).await;
            decoded.map_err(|e| ListFailure::other("", e))?
        };
        let running = core_bridge::run(job);
        Box::pin(async move {
            // Two layers: the core task may panic (outer Err), the folder may not list (inner Err).
            running.await.unwrap_or_else(|e| Err(ListFailure::other("", e)))
        })
    }

    fn stamp(&self, path: &str) -> Fut<Option<f64>> {
        let path = path.to_string();
        let running = core_bridge::run(async move { glint_core::fsx::folder_stamp(path).ok() });
        Box::pin(async move { running.await.ok().flatten() })
    }

    fn drives(&self) -> Fut<Result<Vec<Drive>, String>> {
        let running = core_bridge::run(glint_core::fsx::list_drives());
        Box::pin(async move {
            let drives = running.await??;
            Ok(drives.into_iter().map(Drive::from).collect())
        })
    }

    fn known_folders(&self) -> Fut<Vec<KnownFolder>> {
        let running = core_bridge::run(async { glint_core::fsx::blocking(|| Ok(known_folders())).await });
        Box::pin(async move { running.await.ok().and_then(Result::ok).unwrap_or_default() })
    }

    fn resolve(&self, text: &str) -> Fut<Result<Resolved, String>> {
        let running = core_bridge::run(glint_core::fsx::resolve_path(text.to_string()));
        Box::pin(async move {
            let found = running.await??;
            Ok(Resolved {
                path: found.path,
                dir: found.dir,
            })
        })
    }

    fn open_item(&self, path: &str) -> Fut<Result<Option<String>, String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to open it from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::open_item(window, path.to_string()));
        Box::pin(async move { running.await? })
    }

    fn grant_access(&self, path: &str) -> Fut<Result<Access, String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to ask from".to_string()) });
        };
        let running = core_bridge::run(glint_core::access::grant_access(window, path.to_string()));
        Box::pin(async move {
            Ok(match running.await?? {
                "granted" => Access::Granted,
                "refused" => Access::Refused,
                _ => Access::StillDenied,
            })
        })
    }

    fn watch(&self, folders: Vec<String>) -> bool {
        let Some(window) = self.window() else {
            return false;
        };
        let app = self.app.clone();
        // Fire and forget: the watchers are core's; a failure to watch one folder only means no live refresh.
        glint_core::host::async_runtime::spawn(async move {
            if let Some(state) = app.try_state::<glint_core::watch::FolderWatcher>() {
                let _ = glint_core::watch::watch_dirs(window, state, folders).await;
            }
        });
        true
    }

    fn effect(&self, effect: Effect) {
        match effect {
            Effect::ReportFolder(path) => glint_core::dialogs::report_folder(path),
            // Writes a small file: not on the UI thread.
            Effect::StartupHint(paths) => {
                glint_core::host::async_runtime::spawn_blocking(move || glint_core::startup::startup_hint(paths));
            }
            Effect::BenchFirstFolder => glint_core::startup::bench_first_folder(self.app.clone()),
            Effect::ZoxideAdd(path) => glint_core::zoxide::zoxide_add(path),
        }
    }
}

/// Home, Desktop, Documents, Downloads, Pictures, Music and Videos as Windows has them for this account
/// (what Tauri's path API asked for), those that exist, without a trailing separator.
#[cfg(windows)]
fn known_folders() -> Vec<KnownFolder> {
    use windows::core::GUID;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Profile,
        FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
    };
    let ask = |id: &GUID| -> Option<String> {
        // SAFETY: `id` is a valid known-folder id; on success the shell returns a NUL-terminated string it
        // allocated, which is read once and then freed with `CoTaskMemFree` as the API requires.
        unsafe {
            let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let text = path.to_string().ok();
            CoTaskMemFree(Some(path.0 as *const _));
            text
        }
    };
    let wanted = [
        (KnownId::Home, &FOLDERID_Profile),
        (KnownId::Desktop, &FOLDERID_Desktop),
        (KnownId::Documents, &FOLDERID_Documents),
        (KnownId::Downloads, &FOLDERID_Downloads),
        (KnownId::Pictures, &FOLDERID_Pictures),
        (KnownId::Music, &FOLDERID_Music),
        (KnownId::Videos, &FOLDERID_Videos),
    ];
    wanted
        .into_iter()
        .filter_map(|(id, folder)| {
            let path = ask(folder)?;
            let path = path.trim_end_matches(['\\', '/']).to_string();
            (!path.is_empty()).then_some(KnownFolder {
                id,
                name: id.name(),
                path,
            })
        })
        .collect()
}

#[cfg(not(windows))]
fn known_folders() -> Vec<KnownFolder> {
    Vec::new()
}
