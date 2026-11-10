//! The menus' remaining actions: open with an app, Run in Windows Sandbox, Open in new window, Open file location,
//! Remove from Recent, Show in File Explorer, Rename items in this folder, Set as desktop background, Rotate,
//! cloud files' pins, Export as ncdu JSON, scripts, and Show in File Explorer on demand. Ported from
//! explorer.ts (`revealInApp`, `openInNewWindow`, `forgetRecent`, `exportAsNcdu`), Explorer.tsx (`runInSandbox`,
//! `setWallpaper`, `rotatePictures`, `cloudPin`, `revealArchive`, the "Open with" items), scripts.ts
//! (`runScript`, `openScriptsFolder`) and ondemand.ts (`showOnDemand`, `hideOnDemand`, `refreshOnDemand`).
//!
//! **How an action runs.** Each method checks what the TypeScript checked, then spawns a task that asks the
//! [`Source`](super::Source); the work is glint-core's (or the shell's), on its own threads, and the UI thread
//! only awaits. The person is told with [`ExplorerEvent::Action`] (toasts), [`ExplorerEvent::Export`] (the ncdu
//! export's progress), and sent work with [`ExplorerEvent::Request`] (a window, the terminal, a tool install).
//! Every method returns a `Task` that resolves when it is all done; detach it to let it run.
//! (The menu event each answers is named in the method's doc; the folder sizes are in `sizes.rs`.)

use super::actions_types::*;
use super::archive_types::*;
use super::model::Explorer;
use super::types::*;
use glint_model::paths::{base_name, is_folder_path, parent_path};
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use std::collections::HashMap;

/// About what cmd's prompt takes on one line; what's chosen goes on it (`LONGEST` in scripts.ts).
pub const LONGEST_COMMAND_LINE: usize = 8000;

/// Windows' own program that turns optional features on, which Run in Windows Sandbox points to.
const OPTIONAL_FEATURES: &str = "C:\\Windows\\System32\\OptionalFeatures.exe";

/// What the menu actions keep in the [`Explorer`](super::Explorer): the folder sizes measured (`sizes.rs`) and
/// the remotes shown in File Explorer on demand.
#[derive(Default)]
pub(super) struct ActionState {
    /// Folder sizes by the folder's path (lower case, no trailing separator).
    pub sizes: HashMap<String, SizeRun>,
    /// Counts runs and sizes' versions; never reused.
    pub run_seq: u64,
    /// The remotes shown in File Explorer now (`useOnDemand.mounts`).
    pub on_demand: Vec<OnDemand>,
}

/// A name for a notice: the last part of a path, or the whole path when it has none (a drive).
fn name_of(path: &str) -> String {
    match base_name(path) {
        "" => path.to_string(),
        name => name.to_string(),
    }
}

impl Explorer {
    fn say(&mut self, notice: ActionNotice, cx: &mut Context<Self>) {
        cx.emit(ExplorerEvent::Action(notice));
    }

    fn failed(&mut self, what: ActionFailure, message: String, cx: &mut Context<Self>) {
        self.say(ActionNotice::Failed { what, message }, cx);
    }

    // ---- Opening ------------------------------------------------------------------------------------------

    /// "Open with its app", "Open with {editor}" and "Open with…" for a file on disk (the menu's
    /// `OpenWithItsApp`, `OpenWithEditor` and `OpenWithPicker` with `extract_first` false): its app as a
    /// double-click opens it ([`Explorer::open_file`]; a shortcut to a folder opens that folder), the editor in
    /// Settings, or Windows' app picker. Failing says `Couldn't open the file` (the editor) or `Couldn't open the
    /// app picker`. (A file in an archive or on a remote goes through [`Explorer::open_archived_with`].)
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_on_disk(&mut self, path: &str, how: OpenHow, cx: &mut Context<Self>) -> Task<()> {
        let failure = match &how {
            OpenHow::Default => {
                let opening = self.open_file(path, None, cx);
                return cx.spawn(async move |_, _| {
                    opening.await;
                });
            }
            OpenHow::Editor(_) => ActionFailure::OpenFile,
            OpenHow::Picker => ActionFailure::AppPicker,
        };
        let opening = self.source.open_with(path, &how);
        cx.spawn(async move |this, cx| {
            if let Err(message) = opening.await {
                this.update(cx, |this, cx| this.failed(failure, message, cx)).ok();
            }
        })
    }

    /// "Run in Windows Sandbox" (`runInSandbox`): a copy of the file, opened in a throwaway Windows. Says
    /// [`ActionNotice::SandboxOpening`], or [`ActionNotice::SandboxNotOn`] where Windows has no Sandbox, or why
    /// it couldn't.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn run_in_sandbox(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        let running = self.source.run_in_sandbox(path);
        cx.spawn(async move |this, cx| {
            let notice = match running.await {
                Ok(()) => ActionNotice::SandboxOpening,
                Err(message) if message == "notInstalled" => ActionNotice::SandboxNotOn,
                Err(message) => ActionNotice::Failed {
                    what: ActionFailure::Sandbox,
                    message,
                },
            };
            this.update(cx, |this, cx| this.say(notice, cx)).ok();
        })
    }

    /// The "Optional features" button of [`ActionNotice::SandboxNotOn`]: Windows' own program for turning
    /// features on.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_optional_features(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.shell_open_path(OPTIONAL_FEATURES, ActionFailure::OptionalFeatures, cx)
    }

    /// The "Open" button of [`ActionNotice::OnDemandShown`], and any other that opens a folder in File Explorer
    /// (`openPath(folder)`). Failing says `Couldn't open File Explorer`.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_in_file_explorer(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        self.shell_open_path(path, ActionFailure::FileExplorer, cx)
    }

    fn shell_open_path(&mut self, path: &str, failure: ActionFailure, cx: &mut Context<Self>) -> Task<()> {
        let opening = self.source.shell_open(path);
        cx.spawn(async move |this, cx| {
            if let Err(message) = opening.await {
                this.update(cx, |this, cx| this.failed(failure, message, cx)).ok();
            }
        })
    }

    /// The folder a new window opens on for `path` (`openInNewWindow`'s `target`): `path` when it is a folder, else
    /// the folder shown when none is given, and the home folder for a place that isn't one (a file, This PC, the
    /// Recycle Bin).
    pub fn new_window_folder(&self, path: Option<&str>) -> String {
        let wanted = path.map_or_else(|| self.current_path().to_string(), str::to_string);
        if !wanted.is_empty() && is_folder_path(&wanted) {
            wanted
        } else {
            self.home_folder().to_string()
        }
    }

    /// "Open in new window" and Ctrl+N (`openInNewWindow`): a window of its own on `path`, the folder shown when
    /// none is given; a path that isn't a folder (a file, This PC, the Recycle Bin) gets the home folder
    /// ([`Explorer::new_window_folder`]). The window is another Glint.exe in secondary mode
    /// (`platform::secondary`), started by the workspace when it hears [`Request::NewWindow`]; a tab dragged out
    /// of its strip starts one at the drop point itself.
    pub fn open_in_new_window(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        cx.emit(ExplorerEvent::Request(Request::NewWindow {
            path: self.new_window_folder(path),
        }));
    }

    // ---- Showing ------------------------------------------------------------------------------------------

    /// "Open file location" (`revealInApp`): the folder the item is in opens in the active tab with it selected.
    /// (The text search mode is the workspace's: it switches back to Files first, as `setMode("files")` did.)
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn reveal_in_app(&mut self, path: &str, cx: &mut Context<Self>) -> Task<bool> {
        let options = NavOptions::default().selecting(path);
        self.navigate_task(parent_path(path), options, cx)
    }

    /// "Show in File Explorer" and "Show the archive in File Explorer" (`revealItemInDir`): File Explorer opens
    /// with the items selected, a window for each folder they are in. Pass the archive file itself for an item
    /// in an archive. Failing says `Couldn't open File Explorer`.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn reveal_in_file_explorer(&mut self, paths: Vec<String>, cx: &mut Context<Self>) -> Task<()> {
        let showing = self.source.reveal(paths);
        cx.spawn(async move |this, cx| {
            if let Err(message) = showing.await {
                this.update(cx, |this, cx| this.failed(ActionFailure::FileExplorer, message, cx))
                    .ok();
            }
        })
    }

    /// "Remove from Recent" (`forgetRecent`): their shortcuts in Windows' Recent Items go; the files stay where
    /// they are. Says [`ActionNotice::TookOutOfRecent`] or why it couldn't, and reads the active tab again either
    /// way.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn forget_recent(&mut self, paths: Vec<String>, cx: &mut Context<Self>) -> Task<()> {
        let tab = self.active_tab_id();
        let forgetting = self.source.forget_recent(paths.clone());
        cx.spawn(async move |this, cx| {
            let notice = match forgetting.await {
                Ok(()) => ActionNotice::TookOutOfRecent { paths },
                Err(message) => ActionNotice::Failed {
                    what: ActionFailure::Recent,
                    message,
                },
            };
            reload_after(&this, cx, tab, notice).await;
        })
    }

    /// "Rename items in this folder…" (`openBatchRename(rows, rows)`): the batch rename opens with every row of
    /// the folder shown ([`ExplorerEvent::BatchRename`]). Archives, remotes, closed folders and the Recycle Bin
    /// say they can't be changed.
    pub fn batch_rename_folder(&mut self, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        if self.archive_of(Some(id)).is_some() || self.remote_of(Some(id)) {
            self.read_only_notice(Some(id), cx);
            return;
        }
        if self.closed_of(Some(id)) {
            self.closed_notice(Some(id), cx);
            return;
        }
        if self.recycle_bin_of(Some(id)) {
            self.recycled_notice(cx);
            return;
        }
        let paths: Vec<String> = self
            .tab(id)
            .map(|tab| tab.rows.iter().map(|row| row.path.clone()).collect())
            .unwrap_or_default();
        if !paths.is_empty() {
            cx.emit(ExplorerEvent::BatchRename(paths));
        }
    }

    // ---- Pictures and cloud files ---------------------------------------------------------------------------

    /// "Set as desktop background" (`setWallpaper`). Says [`ActionNotice::WallpaperSet`] or why not.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn set_wallpaper(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        let setting = self.source.set_wallpaper(path);
        let path = path.to_string();
        cx.spawn(async move |this, cx| {
            let notice = match setting.await {
                Ok(()) => ActionNotice::WallpaperSet { path },
                Err(message) => ActionNotice::Failed {
                    what: ActionFailure::Wallpaper,
                    message,
                },
            };
            this.update(cx, |this, cx| this.say(notice, cx)).ok();
        })
    }

    /// "Rotate right" and "Rotate left" (`rotatePictures`): each picture turned in place, one after another
    /// (a JPEG losslessly). Pictures that couldn't be turned are told together ([`ActionNotice::RotateFailed`],
    /// the first three with their reasons), and the active tab is read again.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn rotate_pictures(&mut self, paths: Vec<String>, clockwise: bool, cx: &mut Context<Self>) -> Task<()> {
        let tab = self.active_tab_id();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let mut failed: Vec<String> = Vec::new();
            for path in &paths {
                if let Err(message) = source.rotate_image(path, clockwise).await {
                    failed.push(format!("{}: {message}", base_name(path)));
                }
            }
            let notice = (!failed.is_empty()).then(|| ActionNotice::RotateFailed {
                count: failed.len(),
                detail: failed.iter().take(3).cloned().collect::<Vec<_>>().join("\n"),
            });
            match notice {
                Some(notice) => reload_after(&this, cx, tab, notice).await,
                None => reload_tab(&this, cx, tab).await,
            }
        })
    }

    /// "Always keep on this device", "Free up space" and taking the first back (`cloudPin`): OneDrive's,
    /// iCloud's and Dropbox's files. Says [`ActionNotice::CloudPinned`] or why not, and reads the active tab
    /// again.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn cloud_pin(&mut self, paths: Vec<String>, state: PinState, cx: &mut Context<Self>) -> Task<()> {
        let tab = self.active_tab_id();
        let pinning = self.source.set_cloud_pin(paths.clone(), state);
        cx.spawn(async move |this, cx| {
            let notice = match pinning.await {
                Ok(()) => ActionNotice::CloudPinned { state, paths },
                Err(message) => ActionNotice::Failed {
                    what: if state == PinState::Free {
                        ActionFailure::FreeUpSpace
                    } else {
                        ActionFailure::CloudKept
                    },
                    message,
                },
            };
            if matches!(notice, ActionNotice::CloudPinned { .. }) {
                reload_after(&this, cx, tab, notice).await;
            } else {
                // A failure is told, and the folder is not read again (it did not change).
                this.update(cx, |this, cx| this.say(notice, cx)).ok();
            }
        })
    }

    // ---- Export as ncdu JSON ---------------------------------------------------------------------------------

    /// What the save picker suggests for the export of `folder`: its name without `:` and `\`, or "drive", with
    /// `.ncdu` (`exportAsNcdu`). The picker's filter is `ncdu export` (`ncdu`, `json`).
    pub fn ncdu_file_name(folder: &str) -> String {
        let name: String = base_name(folder).chars().filter(|c| !matches!(c, ':' | '\\')).collect();
        format!("{}.ncdu", if name.is_empty() { "drive" } else { name.as_str() })
    }

    /// "Export as ncdu JSON…", once the person chose `dest` in the save picker: `folder` (on disk) walked into the
    /// file ncdu, and Glint, open again; junctions and symlinks are listed, not followed. Progress and Stop come
    /// as [`ExplorerEvent::Export`], and the result as [`ExportNotice::Done`] or [`ExportNotice::Failed`].
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn export_ncdu(&mut self, folder: &str, dest: &str, cx: &mut Context<Self>) -> Task<()> {
        let name = name_of(folder);
        let (folder, dest) = (folder.to_string(), dest.to_string());
        cx.spawn(async move |this, cx| {
            let begun = this.update(cx, |this, cx| {
                let (id, lane, sink) = this.begin_export_work(name.clone(), cx);
                (this.source.clone(), id, lane, sink)
            });
            let Ok((source, id, lane, sink)) = begun else {
                return;
            };
            let result = source.export_ncdu(&folder, &dest, &lane, sink).await;
            this.update(cx, |this, cx| {
                this.end_archive_work(id, cx);
                match result {
                    Ok(_) => cx.emit(ExplorerEvent::Export(ExportNotice::Done { name, dest })),
                    // Stopped: said by nothing.
                    Err(message) if message == "cancelled" => {}
                    Err(message) => cx.emit(ExplorerEvent::Export(ExportNotice::Failed { name, message })),
                }
            })
            .ok();
        })
    }

    // ---- Scripts ---------------------------------------------------------------------------------------------

    /// A script of the Scripts folder, run on `paths` in `cwd` (`runScript`): `script` is the script's file. A
    /// script whose tool isn't installed asks for it ([`Request::InstallTool`]); one that would not fit on a
    /// command line says so ([`ActionNotice::ScriptTooLong`]); otherwise the terminal runs it
    /// ([`Request::Terminal`]), in a new tab titled with the script's name.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn run_script(&mut self, script: &str, cwd: &str, paths: Vec<String>, cx: &mut Context<Self>) -> Task<()> {
        let listing = self.source.scripts();
        let (script, cwd) = (script.to_string(), cwd.to_string());
        cx.spawn(async move |this, cx| {
            let found = listing.await.ok().and_then(|list| {
                list.into_iter()
                    .find(|s| glint_model::paths::same_path(&s.path, &script))
            });
            this.update(cx, |this, cx| this.script_found(found, script, cwd, paths, cx))
                .ok();
        })
    }

    fn script_found(
        &mut self,
        found: Option<Script>,
        asked: String,
        cwd: String,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(script) = found else {
            self.say(ActionNotice::ScriptGone { path: asked }, cx);
            return;
        };
        if let Some(tool) = script.missing {
            cx.emit(ExplorerEvent::Request(Request::InstallTool { tool }));
            return;
        }
        // The TypeScript counted UTF-16 units (`String.length`) and 3 for the quotes and a space each.
        let length: usize = paths.iter().map(|p| p.encode_utf16().count() + 3).sum();
        if length > LONGEST_COMMAND_LINE {
            self.say(
                ActionNotice::ScriptTooLong {
                    name: script.name,
                    paths: paths.len(),
                    length,
                    longest: LONGEST_COMMAND_LINE,
                },
                cx,
            );
            return;
        }
        let mut args = script.args;
        args.extend(paths);
        // The workspace's terminal pane opens a tab for the job and types `TerminalJob::line(shell)` once its
        // shell is up.
        cx.emit(ExplorerEvent::Request(Request::Terminal(TerminalJob {
            cwd,
            title: script.name,
            program: script.program,
            args,
        })));
    }

    /// "Open the Scripts folder" (`openScriptsFolder`): shown in the active tab, made with a script to start from
    /// the first time. Resolves whether it opened; failing says `Couldn't open the Scripts folder`.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_scripts_folder(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let making = self.source.scripts_folder();
        cx.spawn(async move |this, cx| match making.await {
            Ok(folder) => match this.update(cx, |this, cx| this.navigate_task(folder, NavOptions::default(), cx)) {
                Ok(navigation) => navigation.await,
                Err(_) => false,
            },
            Err(message) => {
                this.update(cx, |this, cx| this.failed(ActionFailure::ScriptsFolder, message, cx))
                    .ok();
                false
            }
        })
    }

    // ---- Remotes in File Explorer, on demand --------------------------------------------------------------

    /// The remotes shown in File Explorer on demand, as last read (`useOnDemand.mounts`): the folder menu offers
    /// "Stop showing in File Explorer" for these connections (fill `MenuData::on_demand` with their ids).
    pub fn on_demand(&self) -> &[OnDemand] {
        &self.actions.on_demand
    }

    /// Reads which remotes are shown in File Explorer (`refreshOnDemand`); [`ExplorerEvent::OnDemandChanged`]
    /// when that differs. Call it at start.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn refresh_on_demand(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let listing = self.source.on_demand_list();
        cx.spawn(async move |this, cx| {
            let mounts = listing.await;
            this.update(cx, |this, cx| {
                if this.actions.on_demand != mounts {
                    this.actions.on_demand = mounts;
                    cx.emit(ExplorerEvent::OnDemandChanged);
                    cx.notify();
                }
            })
            .ok();
        })
    }

    /// "Show in File Explorer, on demand" (`showOnDemand`): the remote `id`, called `name`, as a folder of
    /// placeholders in File Explorer and every app's Open dialog. Says [`ActionNotice::OnDemandShown`] or why not.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn show_on_demand(&mut self, id: &str, name: &str, cx: &mut Context<Self>) -> Task<()> {
        let mounting = self.source.on_demand_mount(id, name);
        let name = name.to_string();
        cx.spawn(async move |this, cx| match mounting.await {
            Ok(folder) => {
                if let Ok(refreshing) = this.update(cx, |this, cx| this.refresh_on_demand(cx)) {
                    refreshing.await;
                }
                this.update(cx, |this, cx| {
                    this.say(ActionNotice::OnDemandShown { name, folder }, cx)
                })
                .ok();
            }
            Err(message) => {
                this.update(cx, |this, cx| this.failed(ActionFailure::ShowOnDemand, message, cx))
                    .ok();
            }
        })
    }

    /// "Stop showing in File Explorer" (`hideOnDemand`): its placeholders go (what's on the remote stays). Says
    /// nothing unless it fails.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn hide_on_demand(&mut self, id: &str, cx: &mut Context<Self>) -> Task<()> {
        let unmounting = self.source.on_demand_unmount(id);
        cx.spawn(async move |this, cx| match unmounting.await {
            Ok(()) => {
                if let Ok(refreshing) = this.update(cx, |this, cx| this.refresh_on_demand(cx)) {
                    refreshing.await;
                }
            }
            Err(message) => {
                this.update(cx, |this, cx| this.failed(ActionFailure::HideOnDemand, message, cx))
                    .ok();
            }
        })
    }
}

/// Says `notice`, then reads the tab's folder again (`void reload(tab)` after the toast).
async fn reload_after(this: &WeakEntity<Explorer>, cx: &mut AsyncApp, tab: TabId, notice: ActionNotice) {
    this.update(cx, |this, cx| this.say(notice, cx)).ok();
    reload_tab(this, cx, tab).await;
}

async fn reload_tab(this: &WeakEntity<Explorer>, cx: &mut AsyncApp, tab: TabId) {
    if let Ok(reloading) = this.update(cx, |this, cx| this.reload(Some(tab), cx)) {
        reloading.await;
    }
}
