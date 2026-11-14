//! Where the explorer gets folders from: glint-core in the app, a fake in tests.
//! Ported from what src/lib/explorer.ts called in src/lib/fs.ts (`listDir`, `folderStamp`, `listDrives`,
//! `resolvePath`, `openItem`, `grantAccess`, `watchDirs`) and from the Tauri path API (`homeDir`…).
//!
//! The model never calls glint-core directly. It holds a [`Source`] (the same pattern as Drive health's
//! `Source`), so the model tests list a fake disk and never touch the real one. Every method returns a
//! future the model awaits in a GPUI task: [`CoreSource`] runs them on core's runtime through
//! [`core_bridge::run`], so the UI thread never waits, and decodes big listings off the UI thread too.

use super::actions_types::*;
use super::archive_types::*;
use super::find_types::{FindOptions, Found, SearchIndex};
use super::inside::{InsideEngine, InsideRequest};
use super::ops_types::*;
use super::types::*;
use crate::core_bridge;
use glint_core::host::ipc::Channel;
use glint_core::host::{AppHandle, Manager as _, Window};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

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

    // ---- File operations -----------------------------------------------------------------------------

    /// A copy, move, rename, delete or restore through Windows' own file engine (`fileOp` in fs.ts): its
    /// progress and conflict dialogs belong to the main window. Says what became of each item.
    fn file_op(&self, op: FileOp) -> Fut<Result<Performed, String>>;
    /// Adds a copy or move to Glint's copy queue (`queue_add`): the job's id. The queue tells how it goes with
    /// core's "copy-queue" event.
    fn queue_add(&self, kind: QueueKind, sources: Vec<String>, dest: String, verify: bool) -> Fut<Result<u64, String>>;
    /// Puts files on the clipboard the way File Explorer does (`clipboardSetFiles`).
    fn clipboard_set_files(&self, paths: Vec<String>, cut: bool) -> Fut<Result<(), String>>;
    /// The files copied or cut, here or in File Explorer, if the clipboard holds any (`clipboardGetFiles`).
    fn clipboard_get_files(&self) -> Fut<Result<Option<ClipboardFiles>, String>>;
    /// Empties the clipboard (`clipboardClear`).
    fn clipboard_clear(&self) -> Fut<Result<(), String>>;
    /// Paste with no files on the clipboard: its picture or text as a new file in `folder`; the file made
    /// (`clipboardPasteFile`).
    fn clipboard_paste_file(&self, folder: &str) -> Fut<Result<Option<String>, String>>;
    /// Empties the Recycle Bin, every drive's, after Windows' own question: whether it's empty now
    /// (`recycleEmpty`).
    fn recycle_empty(&self) -> Fut<Result<bool, String>>;
    /// A folder or empty file in `parent`, numbered like Explorer when the name is taken: its path
    /// (`createEntry`).
    fn create_entry(&self, parent: &str, name: &str, dir: bool) -> Fut<Result<String, String>>;
    /// The kinds of file File Explorer's New menu has on this PC (`new_kinds`).
    fn new_kinds(&self) -> Fut<Result<Vec<NewKind>, String>>;
    /// Makes a file of the kind with extension `ext` in `folder`: its path, or `None` when its app hasn't made
    /// it within a few seconds (`newFile`).
    fn new_file(&self, folder: &str, ext: &str) -> Fut<Result<Option<String>, String>>;
    /// Ejects a drive: a USB drive made safe to remove, a mounted disc image, a disc (`ejectDrive`).
    fn eject(&self, path: &str) -> Fut<Result<(), String>>;

    // ---- Archives, disk images and previous versions ---------------------------------------------------

    /// Extracts files and folders from archives (or downloads them from a remote connection) into a new staging
    /// folder, keeping their folders (`archiveStage`). Each tab works in its own `lane`; `progress` hears how far
    /// it has come, from any thread. Files that can't be had are in [`Staged::skipped`]; it is an `Err` when
    /// nothing could be, and `Err("cancelled")` when [`Source::cancel_stage`] stopped it.
    fn stage(
        &self,
        paths: Vec<String>,
        purpose: StagePurpose,
        lane: &str,
        progress: ProgressSink,
    ) -> Fut<Result<Staged, String>>;
    /// Compress to: files and folders on disk into a new archive next to them (`archiveCompress`). The archive is
    /// `paths[0]` of the answer, numbered like Explorer's if its name is taken.
    fn compress(&self, request: CompressRequest, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>>;
    /// Extract here: the archive on disk into a new folder next to it, named after it (`archiveExtractHere`); a
    /// single compressed file (.gz...) comes out as the file itself. The folder (or file) is [`Staged::folder`].
    fn extract_here(&self, path: &str, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>>;
    /// Stops the lane's running staging, compression or extraction (`cancelArchiveStage`). Not waited for.
    fn cancel_stage(&self, lane: &str);
    /// Deletes a staging folder once its contents are copied (`archiveUnstage`).
    fn unstage(&self, folder: &str) -> Fut<Result<(), String>>;
    /// One file in an archive extracted to a read-only temporary copy, made once and reused: its path
    /// (`archiveExtract(path, "open")`).
    fn extract_file(&self, path: &str) -> Fut<Result<String, String>>;
    /// Mounts a disc or disk image as a drive: the drive (`F:\`) if one showed up (`mountDiskImage`).
    fn mount_image(&self, path: &str) -> Fut<Result<Option<String>, String>>;
    /// Whether opening this image mounts it: Windows' own Mount is its default app (`mountsByDefault`).
    fn mounts_by_default(&self, path: &str) -> Fut<bool>;
    /// A file in a shadow copy copied to a temporary folder, read-only, for its app to open (`versionCopy`).
    fn version_copy(&self, path: &str) -> Fut<Result<String, String>>;
    /// Opens a file on disk the way `how` says.
    fn open_with(&self, path: &str, how: &OpenHow) -> Fut<Result<(), String>>;
    /// Whether every one of these is still on disk (a drag's extracted copies may be gone by the next drag).
    fn all_exist(&self, paths: Vec<String>) -> Fut<bool>;

    // ---- Searching -----------------------------------------------------------------------------------

    /// Starts a name search of `roots` for `query` in `lane` (`findFiles`): the stream of what it says, ending
    /// with the search. Each tab searches in its own lane (its id), so a newer search in a tab ends that tab's
    /// older one and no other. An `Err` is a query core refuses (a bad filter, nothing to look for).
    fn find(
        &self,
        lane: &str,
        roots: Vec<String>,
        query: String,
        options: FindOptions,
    ) -> Fut<Result<async_channel::Receiver<Found>, String>>;
    /// Ends the lane's search, if one runs (`cancelFind`). A search started after this call isn't touched by it.
    fn cancel_find(&self, lane: &str);
    /// How instant search stands (`IndexInfo`), or `None` when that can't be said.
    fn search_index(&self) -> Fut<Option<SearchIndex>>;
    /// Whether the content index covers `path` (and is on): plain words are then answered in milliseconds.
    fn inside_covers(&self, path: &str) -> Fut<bool>;
    /// Starts a search inside files; what it finds arrives as core's `search-event`s
    /// ([`Explorer::inside_heard`](super::Explorer::inside_heard)). A search replaced by a newer one, or stopped
    /// while it starts, resolves to `Err("cancelled")`.
    fn inside_start(&self, engine: InsideEngine, request: InsideRequest) -> Fut<Result<(), String>>;
    /// Stops the search inside files that runs. Never waits on the UI thread.
    fn inside_cancel(&self);
    /// These files as rows: each with its size and date as they are on disk now (a search inside files names
    /// only their paths). One that can't be read still gets its row.
    fn entries_of(&self, paths: Vec<String>) -> Fut<Vec<glint_model::entries::FileEntry>>;

    // ---- Menu actions ---------------------------------------------------------------------------------------

    /// Opens a copy of the file in Windows Sandbox (`sandboxRun`). `Err("notInstalled")` where Windows has none.
    fn run_in_sandbox(&self, path: &str) -> Fut<Result<(), String>>;
    /// Opens a path as the shell's `open` does (`openPath`): a folder in File Explorer, a program started.
    fn shell_open(&self, path: &str) -> Fut<Result<(), String>>;
    /// Shows items in File Explorer with them selected, one window for each folder they are in
    /// (`revealItemInDir`).
    fn reveal(&self, paths: Vec<String>) -> Fut<Result<(), String>>;
    /// Takes items out of Recent (`recentForget`); the files stay where they are.
    fn forget_recent(&self, paths: Vec<String>) -> Fut<Result<(), String>>;
    /// Sets the desktop background (`setDesktopWallpaper`).
    fn set_wallpaper(&self, path: &str) -> Fut<Result<(), String>>;
    /// Turns a picture a quarter clockwise or back, in place (`rotateImage`).
    fn rotate_image(&self, path: &str, clockwise: bool) -> Fut<Result<(), String>>;
    /// Keeps cloud files on this device, frees up their space or leaves them to Windows (`setCloudPin`).
    fn set_cloud_pin(&self, paths: Vec<String>, state: PinState) -> Fut<Result<(), String>>;
    /// The sizes of a folder's subfolders (`folderSizes`): from the index when it has the folder, else with pdu
    /// (an archive's folders are in its list). With `index_only` it is an `Err` instead of running pdu when the
    /// index can't answer. Each tab measures in its own `lane`; `Err("cancelled")` when
    /// [`Source::cancel_folder_sizes`] stopped it.
    fn folder_sizes(&self, path: &str, lane: &str, index_only: bool) -> Fut<Result<Measured, String>>;
    /// Stops the lane's measuring (`cancelFolderSizes`). Not waited for.
    fn cancel_folder_sizes(&self, lane: &str);
    /// Walks a folder on disk into an ncdu JSON export at `dest` (`exportNcdu`); the export is `paths[0]` of the
    /// answer. `progress` hears how many entries are written so far (there is no total).
    fn export_ncdu(&self, path: &str, dest: &str, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>>;
    /// The scripts in Glint's Scripts folder (`listScripts`).
    fn scripts(&self) -> Fut<Result<Vec<Script>, String>>;
    /// The Scripts folder, made with a script to start from the first time (`scriptsFolder`).
    fn scripts_folder(&self) -> Fut<Result<String, String>>;
    /// The remotes shown in File Explorer on demand (`ondemand_list`).
    fn on_demand_list(&self) -> Fut<Vec<OnDemand>>;
    /// Shows a remote in File Explorer on demand: its folder there (`ondemand_mount`).
    fn on_demand_mount(&self, id: &str, name: &str) -> Fut<Result<String, String>>;
    /// Stops showing a remote in File Explorer (`ondemand_unmount`).
    fn on_demand_unmount(&self, id: &str) -> Fut<Result<(), String>>;
}

// ---- Core ------------------------------------------------------------------------------------------------

/// [`Source`] backed by glint-core.
pub struct CoreSource {
    app: AppHandle,
    /// Searches being ended on a blocking thread: a search started after waits for them, so a late Stop can't
    /// end the new search in the same lane.
    cancels: Arc<Mutex<Vec<async_channel::Receiver<()>>>>,
}

impl CoreSource {
    /// A source for the app handle `core_bridge::init` made (`cx.global::<Core>().app.clone()`).
    pub fn new(app: AppHandle) -> Self {
        CoreSource {
            app,
            cancels: Arc::default(),
        }
    }

    /// The window shell dialogs and watchers belong to: the main window, else any.
    fn window(&self) -> Option<Window> {
        self.app
            .window(crate::workspace::MAIN)
            .or_else(|| self.app.windows().into_values().next())
    }
}

/// A folder with this many items or more has its rows made on several threads.
const ROWS_ON_THREADS: usize = 4096;

/// core's answer for a folder, as the explorer keeps it: the entries core read become rows directly (each name is
/// moved, not copied; its path, lowercase name and extension are made beside it), a big folder's on several
/// threads. Runs on the thread that read the folder.
fn shape(listing: glint_core::fsx::Listing) -> Listed {
    use glint_core::fsx::Listing;
    use rayon::prelude::*;
    match listing {
        Listing::Folder {
            path,
            entries,
            elapsed,
            from_index,
            remote,
            archive,
        } => {
            let row = |entry| core_bridge::file_entry(entry, &path, None);
            let rows: Vec<glint_model::entries::FileEntry> = if entries.len() >= ROWS_ON_THREADS {
                entries.into_par_iter().map(row).collect()
            } else {
                entries.into_iter().map(row).collect()
            };
            Listed {
                entries: Arc::from(rows),
                path,
                elapsed,
                archive: archive.map(|a| glint_model::entries::InArchive {
                    path: a.path,
                    outer: a.outer,
                    format: a.format,
                    entries: a.entries,
                    cached: a.cached,
                    read_ms: a.read_ms,
                }),
                from_index,
                remote,
            }
        }
        Listing::Items {
            location,
            entries,
            origins,
            elapsed,
        } => {
            let mut origins = origins.into_iter();
            let rows: Vec<glint_model::entries::FileEntry> = entries
                .into_iter()
                .map(|entry| core_bridge::file_entry(entry, "", origins.next()))
                .collect();
            Listed {
                entries: Arc::from(rows),
                path: location,
                elapsed,
                archive: None,
                from_index: false,
                remote: false,
            }
        }
    }
}

impl Source for CoreSource {
    fn list(&self, path: &str) -> Fut<Result<Listed, ListFailure>> {
        let path = path.to_string();
        let job = async move {
            // Read and made into rows on one blocking thread: one trip there and back.
            glint_core::fsx::list_folder(path, shape)
                .await
                .map_err(|e| ListFailure {
                    kind: if e.kind == "denied" {
                        FailureKind::Denied
                    } else {
                        FailureKind::Other
                    },
                    message: e.message,
                    path: e.path,
                    remote: e.remote,
                })
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

    fn file_op(&self, op: FileOp) -> Fut<Result<Performed, String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to ask from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::file_op(window, core_op(op)));
        Box::pin(async move {
            let done = running.await??;
            Ok(Performed {
                completed: done.completed,
                items: done
                    .items
                    .into_iter()
                    .map(|item| ItemResult {
                        from: item.from,
                        to: item.to,
                    })
                    .collect(),
            })
        })
    }

    fn queue_add(&self, kind: QueueKind, sources: Vec<String>, dest: String, verify: bool) -> Fut<Result<u64, String>> {
        let running = core_bridge::run(glint_core::queue::queue_add(
            self.app.clone(),
            kind.as_str().to_string(),
            sources,
            dest,
            verify,
        ));
        Box::pin(async move { running.await? })
    }

    fn clipboard_set_files(&self, paths: Vec<String>, cut: bool) -> Fut<Result<(), String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to reach the clipboard from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::clipboard_set_files(window, paths, cut));
        Box::pin(async move { running.await? })
    }

    fn clipboard_get_files(&self) -> Fut<Result<Option<ClipboardFiles>, String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to reach the clipboard from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::clipboard_get_files(window));
        Box::pin(async move {
            Ok(running.await??.map(|files| ClipboardFiles {
                paths: files.paths,
                cut: files.cut,
            }))
        })
    }

    fn clipboard_clear(&self) -> Fut<Result<(), String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to reach the clipboard from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::clipboard_clear(window));
        Box::pin(async move { running.await? })
    }

    fn clipboard_paste_file(&self, folder: &str) -> Fut<Result<Option<String>, String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to reach the clipboard from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::clipboard_paste_file(window, folder.to_string()));
        Box::pin(async move { running.await? })
    }

    fn recycle_empty(&self) -> Fut<Result<bool, String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to ask from".to_string()) });
        };
        let running = core_bridge::run(glint_core::recycle::recycle_empty(window));
        Box::pin(async move { running.await? })
    }

    fn create_entry(&self, parent: &str, name: &str, dir: bool) -> Fut<Result<String, String>> {
        let running = core_bridge::run(glint_core::fsx::create_entry(parent.to_string(), name.to_string(), dir));
        Box::pin(async move { running.await? })
    }

    fn new_kinds(&self) -> Fut<Result<Vec<NewKind>, String>> {
        let running = core_bridge::run(glint_core::newmenu::new_kinds());
        Box::pin(async move { running.await? })
    }

    fn new_file(&self, folder: &str, ext: &str) -> Fut<Result<Option<String>, String>> {
        let running = core_bridge::run(glint_core::newmenu::new_file(folder.to_string(), ext.to_string()));
        Box::pin(async move { running.await? })
    }

    fn eject(&self, path: &str) -> Fut<Result<(), String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to eject from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::eject_drive(window, path.to_string()));
        Box::pin(async move { running.await? })
    }

    fn stage(
        &self,
        paths: Vec<String>,
        purpose: StagePurpose,
        lane: &str,
        progress: ProgressSink,
    ) -> Fut<Result<Staged, String>> {
        let running = core_bridge::run(glint_core::archive::archive_stage(
            paths,
            purpose.as_str().to_string(),
            lane.to_string(),
            progress_channel(progress),
        ));
        Box::pin(async move { running.await? })
    }

    fn compress(&self, request: CompressRequest, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>> {
        let format = match request.format {
            CompressFormat::Zip => glint_core::archive::CompressFormat::Zip,
            CompressFormat::SevenZ => glint_core::archive::CompressFormat::SevenZ,
            CompressFormat::TarZst => glint_core::archive::CompressFormat::TarZst,
        };
        let running = core_bridge::run(glint_core::archive::archive_compress(
            request.paths,
            request.folder,
            request.name,
            lane.to_string(),
            Some(format),
            request.password,
            progress_channel(progress),
        ));
        Box::pin(async move { running.await? })
    }

    fn extract_here(&self, path: &str, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>> {
        let running = core_bridge::run(glint_core::archive::archive_extract_here(
            path.to_string(),
            lane.to_string(),
            progress_channel(progress),
        ));
        Box::pin(async move { running.await? })
    }

    fn cancel_stage(&self, lane: &str) {
        glint_core::archive::cancel_archive_stage(lane.to_string());
    }

    fn unstage(&self, folder: &str) -> Fut<Result<(), String>> {
        let running = core_bridge::run(glint_core::archive::archive_unstage(folder.to_string()));
        Box::pin(async move { running.await? })
    }

    fn extract_file(&self, path: &str) -> Fut<Result<String, String>> {
        let running = core_bridge::run(glint_core::archive::archive_extract(
            path.to_string(),
            "open".to_string(),
        ));
        Box::pin(async move { running.await? })
    }

    fn mount_image(&self, path: &str) -> Fut<Result<Option<String>, String>> {
        let Some(window) = self.window() else {
            return Box::pin(async { Err("Glint has no window to mount from".to_string()) });
        };
        let running = core_bridge::run(glint_core::shell::mount_disk_image(window, path.to_string()));
        Box::pin(async move { running.await? })
    }

    fn mounts_by_default(&self, path: &str) -> Fut<bool> {
        let running = core_bridge::run(glint_core::shell::mounts_by_default(path.to_string()));
        Box::pin(async move { running.await.ok().and_then(Result::ok).unwrap_or(false) })
    }

    fn version_copy(&self, path: &str) -> Fut<Result<String, String>> {
        let running = core_bridge::run(glint_core::versions::version_copy(path.to_string()));
        Box::pin(async move { running.await? })
    }

    fn all_exist(&self, paths: Vec<String>) -> Fut<bool> {
        let running = core_bridge::run(async move {
            glint_core::fsx::blocking(move || Ok(paths.iter().all(|p| std::path::Path::new(p).exists()))).await
        });
        Box::pin(async move { running.await.ok().and_then(Result::ok).unwrap_or(false) })
    }

    fn open_with(&self, path: &str, how: &OpenHow) -> Fut<Result<(), String>> {
        let path = path.to_string();
        match how {
            OpenHow::Default => {
                let opening = self.open_item(&path);
                Box::pin(async move { opening.await.map(drop) })
            }
            OpenHow::Picker => {
                let Some(window) = self.window() else {
                    return Box::pin(async { Err("Glint has no window to ask from".to_string()) });
                };
                let running = core_bridge::run(glint_core::shell::open_with(window, path));
                Box::pin(async move { running.await? })
            }
            // The program starts on a blocking thread; it isn't waited for.
            OpenHow::Editor(editor) => {
                let editor = editor.clone();
                let running = core_bridge::run(async move {
                    glint_core::fsx::blocking(move || {
                        std::process::Command::new(&editor)
                            .arg(&path)
                            .spawn()
                            .map(drop)
                            .map_err(|e| e.to_string())
                    })
                    .await
                });
                Box::pin(async move { running.await? })
            }
        }
    }

    fn find(
        &self,
        lane: &str,
        roots: Vec<String>,
        query: String,
        options: FindOptions,
    ) -> Fut<Result<async_channel::Receiver<Found>, String>> {
        let (sender, receiver) = async_channel::unbounded::<Found>();
        // Runs on core's search thread, so rows are made there, never on the UI thread.
        let channel = Channel::new(move |event| {
            sender
                .try_send(found_from(event))
                .map_err(|_| glint_core::host::Error::from("the search was left"))
        });
        let waits = std::mem::take(&mut *self.cancels.lock().unwrap_or_else(|e| e.into_inner()));
        let (app, lane) = (self.app.clone(), lane.to_string());
        let running = core_bridge::run(async move {
            for wait in waits {
                // Closed when that search's end is done.
                let _ = wait.recv().await;
            }
            let state = app
                .try_state::<glint_core::AppState>()
                .ok_or_else(|| "Glint's search isn't ready yet".to_string())?;
            let options = glint_core::find::FindOptions {
                hidden: options.hidden,
                respect_ignore: options.respect_ignore,
                exclude: options.exclude,
                max_results: options.max_results,
                fuzzy: options.fuzzy,
                rank: options.rank,
            };
            glint_core::find::find_files(state, roots, query, options, channel, lane).await
        });
        Box::pin(async move { running.await.and_then(|started| started).map(|()| receiver) })
    }

    fn cancel_find(&self, lane: &str) {
        let (done, wait) = async_channel::bounded::<()>(1);
        {
            let mut cancels = self.cancels.lock().unwrap_or_else(|e| e.into_inner());
            cancels.retain(|w| !w.is_closed());
            cancels.push(wait);
        }
        let (app, lane) = (self.app.clone(), lane.to_string());
        // Ending a search kills fd's process: not on the UI thread.
        glint_core::host::async_runtime::spawn_blocking(move || {
            if let Some(state) = app.try_state::<glint_core::AppState>() {
                glint_core::find::cancel_find(state, lane);
            }
            drop(done);
        });
    }

    fn inside_covers(&self, path: &str) -> Fut<bool> {
        let path = path.to_string();
        let running = core_bridge::run(async move { glint_core::content::content_covers(path) });
        Box::pin(async move { running.await.unwrap_or(false) })
    }

    fn inside_start(&self, engine: InsideEngine, request: InsideRequest) -> Fut<Result<(), String>> {
        let (app, window) = (self.app.clone(), self.window());
        let running = core_bridge::run(async move {
            let window = window.ok_or_else(|| crate::t!("Glint's window isn't open").to_string())?;
            let state = app
                .try_state::<glint_core::AppState>()
                .ok_or_else(|| "Glint's search isn't ready yet".to_string())?;
            match engine {
                InsideEngine::Ripgrep => {
                    let text = glint_core::TextSearch {
                        query: request.query,
                        path: request.path,
                        case_sensitive: request.options.case_sensitive,
                        whole_word: request.options.whole_word,
                        regex: request.options.regex,
                        exclusions: request.exclusions,
                        everything: request.everything,
                    };
                    glint_core::run_ripgrep_batched(window, text, request.id, request.max_results, state).await
                }
                InsideEngine::Documents => {
                    let docs = glint_core::doctext::DocSearch {
                        query: request.query,
                        path: request.path,
                        case_sensitive: request.options.case_sensitive,
                        whole_word: request.options.whole_word,
                        regex: request.options.regex,
                        exclusions: request.exclusions,
                        everything: request.everything,
                        ocr: request.options.ocr,
                    };
                    glint_core::run_doc_search(window, docs, request.id, request.max_results, state).await
                }
                InsideEngine::Index => {
                    let words = glint_core::content::ContentSearch {
                        query: request.query,
                        path: request.path,
                    };
                    glint_core::content::run_content_search(window, words, request.id, request.max_results).await
                }
            }
            .map(drop)
        });
        Box::pin(async move { running.await.and_then(|started| started) })
    }

    fn inside_cancel(&self) {
        let app = self.app.clone();
        // Ending a search kills its process: not on the UI thread.
        glint_core::host::async_runtime::spawn_blocking(move || {
            if let Some(state) = app.try_state::<glint_core::AppState>() {
                glint_core::cancel_search(state);
            }
        });
    }

    fn entries_of(&self, paths: Vec<String>) -> Fut<Vec<glint_model::entries::FileEntry>> {
        let running = core_bridge::run(async move {
            glint_core::fsx::blocking(move || {
                Ok(paths
                    .iter()
                    .map(|path| {
                        let entry = glint_core::fsx::entry_for_path(path, false);
                        glint_model::entries::with_details(
                            glint_model::entries::RawEntry {
                                name: entry.name,
                                path: Some(path.clone()),
                                dir: entry.dir,
                                size: entry.size,
                                modified: entry.modified,
                                attrs: entry.attrs,
                                origin: None,
                            },
                            "",
                        )
                    })
                    .collect::<Vec<_>>())
            })
            .await
        });
        Box::pin(async move { running.await.ok().and_then(Result::ok).unwrap_or_default() })
    }

    fn search_index(&self) -> Fut<Option<SearchIndex>> {
        let running = core_bridge::run(glint_core::index::index_status());
        Box::pin(async move {
            let info = running.await.ok()?.ok()?;
            let on = info.state == "on";
            Some(SearchIndex {
                on,
                can_turn_on: matches!(info.state, "off" | "stopped"),
                outdated: SearchIndex::outdated_by(on, info.version.as_deref(), info.app_version),
                ready: info
                    .volumes
                    .iter()
                    .filter(|v| v.state == "ready")
                    .map(|v| v.volume.to_uppercase())
                    .collect(),
            })
        })
    }

    fn run_in_sandbox(&self, path: &str) -> Fut<Result<(), String>> {
        let running = core_bridge::run(glint_core::sandbox::sandbox_run(path.to_string(), false));
        Box::pin(async move { running.await? })
    }

    fn shell_open(&self, path: &str) -> Fut<Result<(), String>> {
        let path = path.to_string();
        let running =
            core_bridge::run(
                async move { glint_core::fsx::blocking(move || super::shell_win::shell_open(&path)).await },
            );
        Box::pin(async move { running.await? })
    }

    fn reveal(&self, paths: Vec<String>) -> Fut<Result<(), String>> {
        let running =
            core_bridge::run(async move { glint_core::fsx::blocking(move || super::shell_win::reveal(&paths)).await });
        Box::pin(async move { running.await? })
    }

    fn forget_recent(&self, paths: Vec<String>) -> Fut<Result<(), String>> {
        let running = core_bridge::run(glint_core::recent::recent_forget(paths));
        Box::pin(async move { running.await? })
    }

    fn set_wallpaper(&self, path: &str) -> Fut<Result<(), String>> {
        let running = core_bridge::run(glint_core::shell::set_desktop_wallpaper(path.to_string()));
        Box::pin(async move { running.await? })
    }

    fn rotate_image(&self, path: &str, clockwise: bool) -> Fut<Result<(), String>> {
        let running = core_bridge::run(glint_core::shell::rotate_image(path.to_string(), clockwise));
        Box::pin(async move { running.await? })
    }

    fn set_cloud_pin(&self, paths: Vec<String>, state: PinState) -> Fut<Result<(), String>> {
        let running = core_bridge::run(glint_core::shell::set_cloud_pin(paths, state.as_str().to_string()));
        Box::pin(async move { running.await? })
    }

    fn folder_sizes(&self, path: &str, lane: &str, index_only: bool) -> Fut<Result<Measured, String>> {
        let (app, path, lane) = (self.app.clone(), path.to_string(), lane.to_string());
        let running = core_bridge::run(async move {
            let state = app
                .try_state::<glint_core::AppState>()
                .ok_or_else(|| "Glint's folder sizes aren't ready yet".to_string())?;
            glint_core::sizes::folder_sizes(state, path, Some(index_only), lane).await
        });
        Box::pin(async move {
            let found = running.await??;
            Ok(Measured {
                total: found.total,
                sizes: found.sizes,
                elapsed: found.elapsed,
                engine: found.engine.to_string(),
            })
        })
    }

    fn cancel_folder_sizes(&self, lane: &str) {
        let (app, lane) = (self.app.clone(), lane.to_string());
        // Ending pdu's process is not for the UI thread.
        glint_core::host::async_runtime::spawn_blocking(move || {
            if let Some(state) = app.try_state::<glint_core::AppState>() {
                glint_core::sizes::cancel_folder_sizes(state, lane);
            }
        });
    }

    fn export_ncdu(&self, path: &str, dest: &str, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>> {
        let running = core_bridge::run(glint_core::archive::export_ncdu(
            path.to_string(),
            dest.to_string(),
            lane.to_string(),
            counting_channel(progress),
        ));
        Box::pin(async move { running.await? })
    }

    fn scripts(&self) -> Fut<Result<Vec<Script>, String>> {
        let running = core_bridge::run(glint_core::scripts::scripts_list());
        Box::pin(async move { Ok(running.await??.scripts) })
    }

    fn scripts_folder(&self) -> Fut<Result<String, String>> {
        let running = core_bridge::run(glint_core::scripts::scripts_folder());
        Box::pin(async move { running.await? })
    }

    fn on_demand_list(&self) -> Fut<Vec<OnDemand>> {
        let running =
            core_bridge::run(async { glint_core::fsx::blocking(|| Ok(glint_core::ondemand::ondemand_list())).await });
        Box::pin(async move {
            running
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default()
                .into_iter()
                .map(|mount| OnDemand {
                    id: mount.id,
                    folder: mount.folder,
                })
                .collect()
        })
    }

    fn on_demand_mount(&self, id: &str, name: &str) -> Fut<Result<String, String>> {
        let running = core_bridge::run(glint_core::ondemand::ondemand_mount(id.to_string(), name.to_string()));
        Box::pin(async move { running.await? })
    }

    fn on_demand_unmount(&self, id: &str) -> Fut<Result<(), String>> {
        let running = core_bridge::run(glint_core::ondemand::ondemand_unmount(id.to_string()));
        Box::pin(async move { running.await? })
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

/// What Compress to is asked (`archiveCompress`'s arguments).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompressRequest {
    /// The files and folders on disk to pack.
    pub paths: Vec<String>,
    /// The folder the archive goes in.
    pub folder: String,
    /// The archive's name with its extension; numbered by core if it's taken.
    pub name: String,
    pub format: CompressFormat,
    /// A zip's or a 7z's password (AES-256; a 7z's file names too).
    pub password: Option<String>,
}

/// A channel core's staging work reports to: only a changed percent is passed on (core reports often).
fn progress_channel(sink: ProgressSink) -> Channel<glint_core::archive::StageProgress> {
    let last = std::sync::atomic::AtomicI64::new(-1);
    Channel::new(move |progress: StageProgress| {
        let percent = i64::from(progress.percent());
        if last.swap(percent, std::sync::atomic::Ordering::Relaxed) != percent {
            // A closed sink means the work was left; core keeps going and finishes on its own.
            let _ = sink.try_send(progress);
        }
        Ok(())
    })
}

/// A channel core's export reports to: how many entries are written so far, passed on at most every 100 ms (core
/// reports a count as it walks, and the notice only needs to move now and then).
fn counting_channel(sink: ProgressSink) -> Channel<glint_core::archive::StageProgress> {
    let started = std::time::Instant::now();
    let last = std::sync::atomic::AtomicU64::new(0);
    Channel::new(move |progress: StageProgress| {
        let now = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX).max(1);
        let before = last.load(std::sync::atomic::Ordering::Relaxed);
        if before == 0 || now.saturating_sub(before) >= 100 {
            last.store(now, std::sync::atomic::Ordering::Relaxed);
            let _ = sink.try_send(progress);
        }
        Ok(())
    })
}

/// What core's search said, as a [`Found`].
fn found_from(event: glint_core::find::FindEvent) -> Found {
    use glint_core::find::FindEvent;
    match event {
        FindEvent::Started { engine } => Found::Started {
            engine: engine.to_string(),
        },
        FindEvent::Found(entries) => Found::Entries(
            entries
                .into_iter()
                .map(|entry| core_bridge::file_entry(entry, "", None))
                .collect(),
        ),
        FindEvent::Error { message } => Found::Error(message),
        FindEvent::Finished {
            count,
            elapsed,
            limited,
            fuzzy,
        } => Found::Finished {
            count,
            elapsed,
            limited,
            fuzzy,
        },
    }
}

/// A file operation as glint-core takes it.
fn core_op(op: FileOp) -> glint_core::shell::FileOp {
    use glint_core::shell::{Back, FileOp as Core};
    let back = |items: Vec<MoveItem>| -> Vec<Back> {
        items
            .into_iter()
            .map(|item| Back {
                path: item.path,
                folder: item.folder,
                name: item.name,
            })
            .collect()
    };
    match op {
        FileOp::Copy { sources, dest } => Core::Copy { sources, dest },
        FileOp::Move { sources, dest } => Core::Move { sources, dest },
        FileOp::Delete {
            paths,
            permanent,
            confirm,
        } => Core::Delete {
            paths,
            permanent,
            confirm,
        },
        FileOp::Rename { path, name } => Core::Rename { path, name },
        FileOp::MoveBack { items } => Core::MoveBack { items: back(items) },
        FileOp::Restore { items } => Core::Restore { items: back(items) },
        FileOp::Purge { paths } => Core::Purge { paths },
        FileOp::CopyInto { items } => Core::CopyInto { items: back(items) },
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

#[cfg(test)]
mod tests {
    use super::*;
    use glint_core::fsx::{Entry, InArchive, Listing};

    fn entry(name: &str, dir: bool, size: u64, attrs: u32) -> Entry {
        Entry {
            name: name.to_string(),
            path: None,
            dir,
            size,
            modified: 1_700_000_000_000,
            attrs,
        }
    }

    fn folder(path: &str, entries: Vec<Entry>) -> Listing {
        in_place(path, entries, false, None)
    }

    fn in_place(path: &str, entries: Vec<Entry>, remote: bool, archive: Option<InArchive>) -> Listing {
        Listing::Folder {
            path: path.to_string(),
            entries,
            elapsed: 2.5,
            from_index: false,
            remote,
            archive,
        }
    }

    /// Core's entries become rows as they are: paths joined to the folder, lowercase names, extensions and the
    /// directory bit made beside them, whatever the names hold.
    #[test]
    fn a_folders_entries_become_rows() {
        let entries = vec![
            entry("Sub Folder", true, 0, 0),
            entry("Report.PDF", false, 5_000, 0x20),
            entry(".gitignore", false, 12, 0x2),
            entry("تقرير 😀.TXT", false, 7, 0),
            entry("no extension", false, 1, 0),
        ];
        let shaped = shape(folder("C:\\Work", entries));
        assert_eq!(shaped.path, "C:\\Work");
        assert_eq!((shaped.elapsed, shaped.from_index, shaped.remote), (2.5, false, false));
        assert!(shaped.archive.is_none());
        let names: Vec<(&str, &str, &str)> = shaped
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.lower.as_str(), e.ext.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("C:\\Work\\Sub Folder", "sub folder", ""),
                ("C:\\Work\\Report.PDF", "report.pdf", "pdf"),
                ("C:\\Work\\.gitignore", ".gitignore", ""),
                ("C:\\Work\\تقرير 😀.TXT", "تقرير 😀.txt", "txt"),
                ("C:\\Work\\no extension", "no extension", ""),
            ]
        );
        // What the interface reads of them.
        assert!(shaped.entries[0].dir && shaped.entries[0].attrs & glint_model::entries::attr::DIRECTORY != 0);
        assert_eq!(
            (
                shaped.entries[1].size,
                shaped.entries[1].attrs,
                shaped.entries[1].modified
            ),
            (5_000, 0x20, 1_700_000_000_000)
        );
        assert_eq!(shaped.entries[2].attrs, 0x2);
    }

    #[test]
    fn a_big_folders_rows_are_made_on_several_threads_and_stay_in_order() {
        let count = ROWS_ON_THREADS + 500;
        let entries: Vec<Entry> = (0..count)
            .map(|i| entry(&format!("file{i:05}.txt"), false, i as u64, 0))
            .collect();
        let shaped = shape(Listing::Folder {
            path: "C:\\Big".to_string(),
            entries,
            elapsed: 0.0,
            from_index: true,
            remote: false,
            archive: None,
        });
        assert_eq!(shaped.entries.len(), count);
        assert!(shaped.from_index);
        assert!(shaped
            .entries
            .iter()
            .enumerate()
            .all(|(i, row)| row.size == i as u64 && row.name == format!("file{i:05}.txt")));
    }

    #[test]
    fn a_folder_in_an_archive_or_on_a_remote_says_so() {
        let archive = InArchive {
            path: "C:\\a.zip".into(),
            outer: "C:\\a.zip".into(),
            format: "zip",
            entries: 7,
            cached: true,
            read_ms: 0.0,
        };
        let shaped = shape(in_place(
            "C:\\a.zip\\docs",
            vec![entry("inside.txt", false, 3, 0)],
            false,
            Some(archive),
        ));
        assert_eq!(shaped.entries[0].path, "C:\\a.zip\\docs\\inside.txt");
        let archive = shaped.archive.expect("in an archive");
        assert_eq!(
            (archive.path.as_str(), archive.format, archive.entries, archive.cached),
            ("C:\\a.zip", "zip", 7, true)
        );
        let remote = shape(in_place("remote:abc\\photos", Vec::new(), true, None));
        assert!(remote.remote && remote.archive.is_none());
    }

    /// The Recycle Bin's items keep their own paths, and each says where it was deleted from.
    #[test]
    fn items_keep_their_own_paths_and_where_they_came_from() {
        let item = |name: &str, path: &str| Entry {
            path: Some(path.to_string()),
            ..entry(name, false, 1, 0)
        };
        let shaped = shape(Listing::Items {
            location: glint_core::recycle::PLACE.to_string(),
            entries: vec![
                item("budget.xlsx", "C:\\$Recycle.Bin\\S-1\\$RQ2.xlsx"),
                item("ملاحظات.txt", "D:\\$Recycle.Bin\\S-1\\$R7A.txt"),
            ],
            origins: vec!["C:\\Users\\me\\Désktop".into(), "D:\\Notes".into()],
            elapsed: 0.5,
        });
        assert_eq!(shaped.path, glint_core::recycle::PLACE);
        let rows: Vec<(&str, &str, Option<&str>)> = shaped
            .entries
            .iter()
            .map(|e| (e.name.as_str(), e.path.as_str(), e.origin.as_deref()))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "budget.xlsx",
                    "C:\\$Recycle.Bin\\S-1\\$RQ2.xlsx",
                    Some("C:\\Users\\me\\Désktop")
                ),
                ("ملاحظات.txt", "D:\\$Recycle.Bin\\S-1\\$R7A.txt", Some("D:\\Notes")),
            ]
        );
        assert_eq!(shaped.entries[1].ext, "txt");
    }

    #[test]
    fn a_search_sends_rows_with_their_own_paths() {
        use glint_core::find::FindEvent;
        let found = found_from(FindEvent::Found(vec![Entry {
            path: Some("C:\\z\\Note.MD".into()),
            ..entry("Note.MD", false, 9, 0)
        }]));
        let Found::Entries(rows) = found else { panic!("rows") };
        assert_eq!((rows[0].path.as_str(), rows[0].ext.as_str()), ("C:\\z\\Note.MD", "md"));
        assert!(
            matches!(found_from(FindEvent::Started { engine: "index" }), Found::Started { engine } if engine == "index")
        );
    }
}
