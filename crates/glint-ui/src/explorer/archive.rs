//! Archives, disk images and previous versions: Compress to, Extract here, copying out of an archive (the other
//! pane, the clipboard, a drag), opening a file from an archive, mounting a disc image and opening a previous
//! version. Ported from explorer.ts: `staging`, `extracting`, `compressEntries`, `extractHere`,
//! `copyOutOfArchive`, `dragFromArchive`, `openFromArchive`, `openArchivedWith`, `openDiskImage`, `mountImage`
//! and `openVersionFile`.
//!
//! **How the work runs.** Every method checks what the TypeScript checked first (archives and closed folders are
//! read-only for it: it raises the matching [`Notice`]), then spawns a task that asks the [`Source`]. The heavy
//! part (extracting, compressing, downloading) is glint-core's, on its own threads, one job per tab (its *lane*);
//! the UI thread only awaits. Slow work tells the person through [`ExplorerEvent::Archive`]: a notice after 400 ms
//! (500 ms for the small jobs) with the percent as it advances and a **Stop** button
//! ([`Explorer::stop_archive_work`]), the files core had to leave out, and a failure with its reason.
//!
//! **What isn't here.** The password for Compress is asked by the caller (the password dialog) and passed in.
//! Starting the drag of the extracted copies ([`platform::dragdrop::drag_out`](crate::platform::dragdrop)) and
//! the notices' wording are the coordinator's; the doc of each method says what to do with its result.

use super::archive_types::*;
use super::model::Explorer;
use super::ops_types::*;
use super::source::{CompressRequest, Fut, Source};
use super::types::*;
use glint_model::entries::{is_archived, is_closed, is_remote, FileEntry};
use glint_model::paths::{base_name, is_remote_path, parent_path, same_path};
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use std::collections::HashMap;

// ---- State -------------------------------------------------------------------------------------------------

/// One archive job in progress.
struct Run {
    /// The tab that started it: core cancels by tab.
    lane: String,
    doing: Doing,
    /// Its notice is on screen.
    shown: bool,
    /// The last percent told.
    percent: Option<u8>,
    /// Waits out the delay before the notice shows; dropped (cancelled) when the work ends.
    _timer: Task<()>,
    /// Carries core's progress to the model; dropped when the work ends.
    _progress: Task<()>,
}

/// What the archives part keeps in the [`Explorer`]: the jobs running and the drag that is ready.
#[derive(Default)]
pub(super) struct ArchiveState {
    runs: HashMap<u64, Run>,
    /// Copies extracted for a drag that didn't start (the button came up while they were being made), by the
    /// items' key: the next drag of the same items starts at once (`readyToDrag`).
    ready_drag: Option<(String, Vec<String>)>,
}

impl ArchiveState {
    /// How many archive jobs run now.
    pub(super) fn running(&self) -> usize {
        self.runs.len()
    }
}

/// The items of a drag, as one key (`paths.join("\n").toLowerCase()`).
fn drag_key(paths: &[String]) -> String {
    paths.join("\n").to_lowercase()
}

// ---- Progress and the notice -------------------------------------------------------------------------------

impl Explorer {
    /// A job starts: its notice shows if it is still running after [`Doing::shows_after`]. Pair with
    /// [`Explorer::end_archive_work`]. Returns its id (shared with the busy toasts' numbering), the tab's lane and
    /// where core reports progress to.
    fn begin_archive_work(&mut self, doing: Doing, cx: &mut Context<Self>) -> (u64, String, ProgressSink) {
        self.ops.busy_seq += 1;
        let id = self.ops.busy_seq;
        let lane = self.active_tab_id().to_string();
        let (sink, stream) = async_channel::unbounded::<StageProgress>();
        let delay = doing.shows_after();
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| this.show_archive_work(id, cx)).ok();
        });
        let progress = cx.spawn(async move |this, cx| {
            while let Ok(progress) = stream.recv().await {
                if this
                    .update(cx, |this, cx| this.archive_progress(id, progress, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.archives.runs.insert(
            id,
            Run {
                lane: lane.clone(),
                doing,
                shown: false,
                percent: None,
                _timer: timer,
                _progress: progress,
            },
        );
        (id, lane, sink)
    }

    fn show_archive_work(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(run) = self.archives.runs.get_mut(&id) {
            run.shown = true;
            cx.emit(ExplorerEvent::Archive(ArchiveNotice::Shown {
                id,
                doing: run.doing.clone(),
                percent: run.percent,
            }));
        }
    }

    /// Core reported how far a job has come. Only a changed percent is told, as the TypeScript's label only
    /// changed then.
    fn archive_progress(&mut self, id: u64, progress: StageProgress, cx: &mut Context<Self>) {
        let Some(run) = self.archives.runs.get_mut(&id) else {
            return;
        };
        let percent = progress.percent();
        if run.percent == Some(percent) {
            return;
        }
        run.percent = Some(percent);
        cx.emit(ExplorerEvent::Archive(ArchiveNotice::Progress {
            id,
            doing: run.doing.clone(),
            percent,
            done: progress.done,
            total: progress.total,
            shown: run.shown,
        }));
    }

    /// The job ended: its notice goes, and the taskbar progress once nothing else runs.
    fn end_archive_work(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(run) = self.archives.runs.remove(&id) else {
            return;
        };
        cx.emit(ExplorerEvent::Archive(ArchiveNotice::Ended {
            id,
            was_shown: run.shown,
            idle: self.is_idle(),
        }));
    }

    /// The **Stop** button of an archive notice (`cancelArchiveStage`): the job `id` (from
    /// [`ArchiveNotice::Shown`]) stops and leaves nothing behind. A job that has ended is left alone.
    pub fn stop_archive_work(&mut self, id: u64) {
        if let Some(run) = self.archives.runs.get(&id).filter(|run| run.doing.stoppable()) {
            self.source.cancel_stage(&run.lane);
        }
    }
}

/// Runs `work` as a job with a notice, progress and Stop (`staging`): afterwards says which files had to be left
/// out and, on failure, why. Resolves the result: `Err("cancelled")` when stopped (said by nothing), the reason
/// when it failed (said as `failure`).
async fn staging(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    doing: Doing,
    verb: SkippedVerb,
    failure: ArchiveFailure,
    work: impl FnOnce(&dyn Source, &str, ProgressSink) -> Fut<Result<Staged, String>>,
) -> Result<Staged, String> {
    let begun = this.update(cx, |this, cx| {
        let (id, lane, sink) = this.begin_archive_work(doing, cx);
        (this.source.clone(), id, lane, sink)
    });
    let Ok((source, id, lane, sink)) = begun else {
        return Err("cancelled".to_string());
    };
    let result = work(&*source, &lane, sink).await;
    this.update(cx, |this, cx| {
        this.end_archive_work(id, cx);
        match &result {
            Ok(staged) if !staged.skipped.is_empty() => {
                cx.emit(ExplorerEvent::Archive(skipped_notice(&staged.skipped, verb)))
            }
            Err(message) if message != "cancelled" => cx.emit(ExplorerEvent::Archive(ArchiveNotice::Failed {
                what: failure,
                message: message.clone(),
            })),
            _ => {}
        }
    })
    .ok();
    result
}

/// A small job with a notice but no progress or Stop (`busy`): opening a file from an archive, mounting, copying
/// out a version. Resolves what the work did; its failure is the caller's to say.
async fn busy_work<T>(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    doing: Doing,
    work: impl FnOnce(&dyn Source) -> Fut<Result<T, String>>,
) -> Result<T, String> {
    let begun = this.update(cx, |this, cx| {
        let (id, _, _) = this.begin_archive_work(doing, cx);
        (this.source.clone(), id)
    });
    let Ok((source, id)) = begun else {
        return Err("cancelled".to_string());
    };
    let result = work(&*source).await;
    this.update(cx, |this, cx| this.end_archive_work(id, cx)).ok();
    result
}

/// The warning for files core left out: the first three by name, how many more, and the first one's reason.
fn skipped_notice(skipped: &[Skipped], verb: SkippedVerb) -> ArchiveNotice {
    ArchiveNotice::Skipped {
        count: skipped.len(),
        names: skipped.iter().take(3).map(|s| base_name(&s.path).to_string()).collect(),
        more: skipped.len().saturating_sub(3),
        reason: skipped.first().map(|s| s.reason.clone()).unwrap_or_default(),
        verb,
    }
}

/// Extracts `paths` (in an archive) for Windows' file engine to copy, or downloads them (on a remote), with
/// progress once it takes a moment and a Stop button (`extracting`).
async fn extracting(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    paths: Vec<String>,
    purpose: StagePurpose,
) -> Result<Staged, String> {
    let subject = Subject::of(&paths);
    let (doing, verb, failure) = if paths.iter().any(|p| is_remote_path(p)) {
        (
            Doing::Downloading(subject.clone()),
            SkippedVerb::Downloaded,
            ArchiveFailure::Download(subject),
        )
    } else {
        (
            Doing::ExtractingFromArchive(subject.clone()),
            SkippedVerb::Extracted,
            ArchiveFailure::Extract(subject),
        )
    };
    staging(this, cx, doing, verb, failure, move |source, lane, sink| {
        source.stage(paths, purpose, lane, sink)
    })
    .await
}

// ---- Compress to and Extract here ----------------------------------------------------------------------------

impl Explorer {
    /// The entries of the paths in the tab, in the order of `paths` (`None` for one the tab doesn't list): its
    /// listing's and a search's results.
    fn entries_for(&self, tab: TabId, paths: &[String]) -> Vec<Option<FileEntry>> {
        let mut found: Vec<Option<FileEntry>> = vec![None; paths.len()];
        let Some(t) = self.tabs.get(&tab) else {
            return found;
        };
        let wanted: HashMap<String, Vec<usize>> = paths.iter().enumerate().fold(HashMap::new(), |mut m, (i, p)| {
            m.entry(p.to_lowercase()).or_default().push(i);
            m
        });
        let listed = t.listing.iter().flat_map(|l| l.entries.iter());
        let searched = t.find.iter().flat_map(|f| f.entries.iter());
        for entry in listed.chain(searched) {
            if let Some(at) = wanted.get(&entry.path.to_lowercase()) {
                for &i in at {
                    if let Some(slot) = found.get_mut(i) {
                        slot.get_or_insert_with(|| entry.clone());
                    }
                }
            }
        }
        found
    }

    /// Compress to: `paths` (on disk) into an archive next to them (a zip unless `format` says a 7z or a tar.zst),
    /// named after the first one as Explorer does, then selected and renamed in place like a new folder. A zip or a
    /// 7z can have a `password` (the caller asks for it: `dialogs::password::ask_compress_password`). Ctrl+Z takes
    /// it back (`compressEntries`).
    ///
    /// Items in an archive or on a remote say the archive is read-only; items in a folder Windows keeps closed say
    /// so. Where the archive is made outside the folder on screen, [`ArchiveNotice::Compressed`] says so.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn compress(
        &mut self,
        paths: Vec<String>,
        format: CompressFormat,
        password: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let tab = self.active_tab_id();
        let Some(first_path) = paths.first().cloned() else {
            return Task::ready(());
        };
        let entries = self.entries_for(tab, &paths);
        // An archive file on disk can be compressed; what is inside one can't (core says so too). A path the tab
        // doesn't list is judged by the tab.
        let unlisted = entries.iter().any(Option::is_none);
        if paths.iter().any(|p| is_remote_path(p))
            || entries.iter().flatten().any(is_archived)
            || (unlisted && (self.archive_of(Some(tab)).is_some() || self.remote_of(Some(tab))))
        {
            self.read_only_notice(Some(tab), cx);
            return Task::ready(());
        }
        if entries.iter().flatten().any(is_closed) {
            self.closed_notice(Some(tab), cx);
            return Task::ready(());
        }
        let folder = parent_path(&first_path);
        let (first_name, first_dir) = match entries.first().and_then(Option::as_ref) {
            Some(first) => (first.name.clone(), first.dir),
            None => (base_name(&first_path).to_string(), false),
        };
        // NOTE(port): a name that starts with its only dot (".gitignore") loses all of it, as `slice(0, lastIndexOf('.'))`
        // did in the TypeScript, and becomes ".zip".
        let stem = if first_dir || !first_name.contains('.') {
            first_name.clone()
        } else {
            first_name
                .rsplit_once('.')
                .map_or(first_name.clone(), |(stem, _)| stem.to_string())
        };
        let name = format!("{stem}.{}", format.ext());
        let subject = if paths.len() == 1 {
            Subject::One(first_name)
        } else {
            Subject::Many(paths.len())
        };
        let request = CompressRequest {
            paths,
            folder: folder.clone(),
            name,
            format,
            password: password.filter(|p| !p.is_empty()),
        };
        cx.spawn(async move |this, cx| {
            let staged = staging(
                &this,
                cx,
                Doing::Compressing(subject.clone()),
                SkippedVerb::Added,
                ArchiveFailure::Compress(subject.clone()),
                move |source, lane, sink| source.compress(request, lane, sink),
            )
            .await;
            let Some(zip) = staged.ok().and_then(|s| s.paths.first().cloned()) else {
                return;
            };
            let navigation = this
                .update(cx, |this, cx| {
                    this.remember_undo(
                        format!("compression into “{}”", base_name(&zip)),
                        UndoAction::Recycle(vec![zip.clone()]),
                        cx,
                    );
                    let shown = this
                        .tabs
                        .get(&tab)
                        .is_some_and(|t| t.find.is_none() && same_path(&t.path, &folder));
                    if shown {
                        Some(this.show_in(tab, &folder, Some(vec![zip.clone()]), cx))
                    } else {
                        cx.emit(ExplorerEvent::Archive(ArchiveNotice::Compressed {
                            what: subject,
                            zip: zip.clone(),
                        }));
                        None
                    }
                })
                .ok()
                .flatten();
            if let Some(navigation) = navigation {
                navigation.await;
                // Named in place, like a new folder.
                this.update(cx, |this, cx| this.rename_focused(tab, cx)).ok();
            }
        })
    }

    /// Extract here: the archive file at `path` (on disk, not in another archive) into a new folder next to it,
    /// named after it (Explorer's Extract All); a single compressed file (.gz…) comes out as the file itself.
    /// Ctrl+Z takes it back (`extractHere`). Shown selected where the archive is; elsewhere
    /// [`ArchiveNotice::Extracted`] offers to open it.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn extract_here(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        let tab = self.active_tab_id();
        let path = path.to_string();
        let name = base_name(&path).to_string();
        let subject = Subject::One(name.clone());
        cx.spawn(async move |this, cx| {
            let archive = path.clone();
            let staged = staging(
                &this,
                cx,
                Doing::Extracting(subject.clone()),
                SkippedVerb::Extracted,
                ArchiveFailure::Extract(subject),
                move |source, lane, sink| source.extract_here(&archive, lane, sink),
            )
            .await;
            let Ok(staged) = staged else {
                return;
            };
            let navigation = this
                .update(cx, |this, cx| {
                    this.remember_undo(
                        format!("extraction of “{name}”"),
                        UndoAction::Recycle(vec![staged.folder.clone()]),
                        cx,
                    );
                    // One file out of a .gz is shown where it is; a folder is opened.
                    let file =
                        staged.paths.len() == 1 && staged.paths.first().is_some_and(|p| same_path(p, &staged.folder));
                    let here = this
                        .tabs
                        .get(&tab)
                        .filter(|t| t.find.is_none() && same_path(&t.path, &parent_path(&path)))
                        .map(|t| t.path.clone());
                    match here {
                        Some(folder) => Some(this.show_in(tab, &folder, Some(vec![staged.folder.clone()]), cx)),
                        None => {
                            cx.emit(ExplorerEvent::Archive(ArchiveNotice::Extracted {
                                archive: path.clone(),
                                folder: staged.folder.clone(),
                                file,
                            }));
                            None
                        }
                    }
                })
                .ok()
                .flatten();
            if let Some(navigation) = navigation {
                navigation.await;
            }
        })
    }

    /// The **Open** / **Show** button of [`ArchiveNotice::Extracted`]: opens the extracted `folder`, or shows a single
    /// extracted file in its folder, selected. Resolves whether it opened.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn show_extracted(&mut self, folder: &str, file: bool, cx: &mut Context<Self>) -> Task<bool> {
        if file {
            self.navigate_task(parent_path(folder), NavOptions::default().selecting(folder), cx)
        } else {
            self.navigate_task(folder, NavOptions::default(), cx)
        }
    }
}

// ---- Copying out of an archive ------------------------------------------------------------------------------

impl Explorer {
    /// Does the work [`ExplorerEvent::FromArchive`] asks for: extracts the items (downloads them, from a remote), then
    /// finishes the job (`copyOutOfArchive`, `copyToClipboard`).
    ///
    /// - [`ArchiveJob::CopyTo`]: stages, copies the copies into the other pane's folder through Windows' file engine
    ///   (progress, *Replace or Skip*) and shows them there ([`Explorer::copy_staged`]), then removes the staged copies.
    /// - [`ArchiveJob::ToClipboard`]: stages, puts the copies on the clipboard ([`Explorer::copied_staged`]); Glint
    ///   removes them itself once they are off the clipboard.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn run_archive_job(&mut self, job: ArchiveJob, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| match job {
            ArchiveJob::CopyTo {
                paths,
                target_tab,
                target,
            } => {
                if paths.is_empty() {
                    return;
                }
                let Ok(staged) = extracting(&this, cx, paths, StagePurpose::Copy).await else {
                    return;
                };
                let Ok(source) = this.update(cx, |this, _| this.source.clone()) else {
                    return;
                };
                let copying = this.update(cx, |this, cx| this.copy_staged(staged.paths, target_tab, target, cx));
                if let Ok(copying) = copying {
                    copying.await;
                }
                // The copies are done with, whether or not they went.
                let _ = source.unstage(&staged.folder).await;
            }
            ArchiveJob::ToClipboard { paths } => {
                if paths.is_empty() {
                    return;
                }
                let Ok(staged) = extracting(&this, cx, paths, StagePurpose::Out).await else {
                    return;
                };
                if let Ok(setting) = this.update(cx, |this, cx| this.copied_staged(staged.paths, cx)) {
                    setting.await;
                }
            }
        })
    }

    /// Dragging items out of an archive (or off a remote connection): they are extracted first (Windows' drag
    /// carries files), and the copies' paths come back. The caller then drags them as copies, as
    /// `platform::dragdrop::drag_out(cx, workspace::MAIN, copies, true)`; if that answers `"released"` (the button
    /// came up while they were being made), call [`Explorer::drag_released`], and if it fails
    /// [`Explorer::drag_failed`]. A big selection can take longer to extract than the button is held: then the next
    /// drag of the same items finds them ready and resolves at once (unless they are gone meanwhile, when they are
    /// extracted again) (`dragFromArchive`). `Err` is the reason (already told, except "cancelled").
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn drag_from_archive(
        &mut self,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<String>, String>> {
        let key = drag_key(&paths);
        let ready = self
            .archives
            .ready_drag
            .take()
            .and_then(|(ready, copies)| (ready == key).then_some(copies));
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            if let Some(copies) = ready {
                if source.all_exist(copies.clone()).await {
                    return Ok(copies);
                }
            }
            extracting(&this, cx, paths, StagePurpose::Out)
                .await
                .map(|staged| staged.paths)
        })
    }

    /// The drag of the copies [`Explorer::drag_from_archive`] made did not start (the button was already up): they
    /// are kept for the next drag of the same `paths`, and the person is told ([`ArchiveNotice::ReadyToDrag`]).
    pub fn drag_released(&mut self, paths: &[String], copies: Vec<String>, cx: &mut Context<Self>) {
        self.archives.ready_drag = Some((drag_key(paths), copies));
        cx.emit(ExplorerEvent::Archive(ArchiveNotice::ReadyToDrag));
    }

    /// The drag of the extracted copies failed: says `Couldn't drag these items` with the message.
    pub fn drag_failed(&mut self, message: String, cx: &mut Context<Self>) {
        cx.emit(ExplorerEvent::Archive(ArchiveNotice::Failed {
            what: ArchiveFailure::Drag,
            message,
        }));
    }
}

// ---- Opening ---------------------------------------------------------------------------------------------------

impl Explorer {
    /// Opens a file in an archive with its app: a read-only temporary copy of it, so changes can't be lost
    /// silently (`openFromArchive`). A remote's file is downloaded first, with how far it has come and a Stop
    /// button. Failing says `Notice::CouldNotOpenFile`.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_archived(&mut self, entry: &FileEntry, cx: &mut Context<Self>) -> Task<()> {
        self.open_archived_with(entry, OpenHow::Default, cx)
    }

    /// "Open with" an app of your choice (or the editor in Settings) for a file in an archive or on a remote
    /// (`openArchivedWith`): the temporary copy is opened the way `how` says.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_archived_with(&mut self, entry: &FileEntry, how: OpenHow, cx: &mut Context<Self>) -> Task<()> {
        let path = entry.path.clone();
        let name = entry.name.clone();
        let remote = is_remote(entry);
        cx.spawn(async move |this, cx| {
            let copy = if remote {
                // Downloaded (with how far, and Stop), read-only, then opened; a failed download says so itself.
                let downloaded = extracting(&this, cx, vec![path.clone()], StagePurpose::Open).await;
                match downloaded {
                    Ok(staged) => staged
                        .paths
                        .into_iter()
                        .next()
                        .ok_or_else(|| "Nothing was downloaded".to_string()),
                    Err(_) => return,
                }
            } else {
                let from = path.clone();
                busy_work(&this, cx, Doing::ExtractingFile(name), move |source| {
                    source.extract_file(&from)
                })
                .await
            };
            let opened = match copy {
                Ok(copy) => open_copy(&this, cx, &copy, &how).await,
                Err(message) => Err(message),
            };
            if let Err(message) = opened {
                this.update(cx, |_, cx| {
                    cx.emit(ExplorerEvent::Notice(Notice::CouldNotOpenFile { path, message }));
                })
                .ok();
            }
        })
    }

    /// A file in a shadow copy (a previous version), opened as a read-only copy: apps can't open it where it is
    /// (`openVersionFile`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_version_file(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        let path = path.to_string();
        let name = base_name(&path).to_string();
        cx.spawn(async move |this, cx| {
            let from = path.clone();
            let copy = busy_work(&this, cx, Doing::CopyingOut(name), move |source| {
                source.version_copy(&from)
            })
            .await;
            let opened = match copy {
                Ok(copy) => open_copy(&this, cx, &copy, &OpenHow::Default).await,
                Err(message) => Err(message),
            };
            if let Err(message) = opened {
                this.update(cx, |_, cx| {
                    cx.emit(ExplorerEvent::Notice(Notice::CouldNotOpenFile { path, message }));
                })
                .ok();
            }
        })
    }

    /// Mounts a disc image (.iso, .img, .vhd, .vhdx) as a drive, and opens the new drive in `tab` (the active one by
    /// default) if one is found (`mountImage`). Says what it mounted as, or why it couldn't.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn mount_image(&mut self, path: &str, tab: Option<TabId>, cx: &mut Context<Self>) -> Task<()> {
        let tab = tab.unwrap_or_else(|| self.active_tab_id());
        let path = path.to_string();
        let name = base_name(&path).to_string();
        cx.spawn(async move |this, cx| {
            let from = path.clone();
            let mounted = busy_work(&this, cx, Doing::Mounting(name.clone()), move |source| {
                source.mount_image(&from)
            })
            .await;
            match mounted {
                Ok(drive) => {
                    // The new drive shows up in This PC and the sidebar.
                    if let Ok(reading) = this.update(cx, |this, cx| this.refresh_drives(cx)) {
                        reading.await;
                    }
                    let navigation = this.update(cx, |this, cx| {
                        cx.emit(ExplorerEvent::Archive(ArchiveNotice::Mounted {
                            name,
                            drive: drive.clone(),
                        }));
                        drive.map(|drive| this.navigate_task(drive, NavOptions::in_tab(tab), cx))
                    });
                    if let Ok(Some(navigation)) = navigation {
                        navigation.await;
                    }
                }
                Err(message) => {
                    this.update(cx, |_, cx| {
                        cx.emit(ExplorerEvent::Archive(ArchiveNotice::Failed {
                            what: ArchiveFailure::Mount(name),
                            message,
                        }));
                    })
                    .ok();
                }
            }
        })
    }

    /// A disc or disk image opens as it does from File Explorer: mounted (and its drive shown here, with no File
    /// Explorer window) when Windows' Mount is its default app, else in the app chosen for it (`openDiskImage`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn open_disk_image(&mut self, path: &str, cx: &mut Context<Self>) -> Task<()> {
        let path = path.to_string();
        let asking = self.source.mounts_by_default(&path);
        cx.spawn(async move |this, cx| {
            let mounting = asking.await;
            let task = this.update(cx, |this, cx| {
                if mounting {
                    this.mount_image(&path, None, cx)
                } else {
                    let opening = this.open_file(&path, None, cx);
                    cx.spawn(async move |_, _| {
                        opening.await;
                    })
                }
            });
            if let Ok(task) = task {
                task.await;
            }
        })
    }

    /// Does the work [`ExplorerEvent::Handoff`] hands over: opens a previous version, a disk image or a file in an
    /// archive.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn handoff(&mut self, handoff: Handoff, cx: &mut Context<Self>) -> Task<()> {
        match handoff {
            Handoff::VersionFile(path) => self.open_version_file(&path, cx),
            Handoff::DiskImage(path) => self.open_disk_image(&path, cx),
            Handoff::ArchivedFile(entry) => self.open_archived(&entry, cx),
        }
    }
}

/// Opens the temporary copy the way `how` says.
async fn open_copy(this: &WeakEntity<Explorer>, cx: &mut AsyncApp, copy: &str, how: &OpenHow) -> Result<(), String> {
    let Ok(source) = this.update(cx, |this, _| this.source.clone()) else {
        return Ok(());
    };
    source.open_with(copy, how).await
}
