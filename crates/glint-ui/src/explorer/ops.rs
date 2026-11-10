//! File operations: copy and move through Windows' file engine or the copy queue, delete, drops, copy to the
//! other pane, and the "busy" toast that slow work shows. Ported from explorer.ts: `fileOp`, `busy`,
//! `copyToOtherPane`, `otherPaneFolder`, `dropFiles`, `deletePaths`.
//!
//! **How an operation runs.** The method checks what the TypeScript checked first (archives, closed folders and
//! the Recycle Bin are read-only for it: it raises the matching notice), then spawns a task that asks the
//! [`Source`](super::Source). Windows' file engine runs on a blocking thread of core's, with its own progress
//! and conflict dialogs; the UI thread only awaits. When it is done the model offers Undo, reads the folder
//! again and selects what arrived. Every method returns a `Task` that resolves when all that is done; detach
//! it to let it run (dropping it stops waiting, not Windows).

use super::model::{Explorer, Prefs};
use super::ops_types::*;
use super::types::*;
use glint_model::format::plural;
use glint_model::paths::{
    base_name, is_folder_path, is_recycle_bin, is_remote_path, join_path, parent_path, same_path,
};
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use std::collections::HashSet;
use std::time::Duration;

/// Slow work shows its toast after this long (`busy`: "quick operations finish before it appears").
const BUSY_AFTER: Duration = Duration::from_millis(500);

// ---- Slow work -----------------------------------------------------------------------------------------------

impl Explorer {
    /// Slow work starts (`busy`): its toast shows if it is still running half a second from now. Pair with
    /// [`Explorer::end_busy`].
    pub(super) fn begin_busy(&mut self, what: Working, cx: &mut Context<Self>) -> u64 {
        self.ops.busy_seq += 1;
        let id = self.ops.busy_seq;
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(BUSY_AFTER).await;
            this.update(cx, |this, cx| this.show_busy(id, cx)).ok();
        });
        self.ops.busy.insert(
            id,
            super::ops_state::BusyWork {
                what,
                shown: false,
                _timer: Some(timer),
            },
        );
        id
    }

    fn show_busy(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(work) = self.ops.busy.get_mut(&id) {
            work.shown = true;
            cx.emit(ExplorerEvent::Busy(BusyEvent::Shown {
                id,
                what: work.what.clone(),
            }));
        }
    }

    /// The slow work ended: its toast goes, and the taskbar button once nothing else runs.
    pub(super) fn end_busy(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(work) = self.ops.busy.remove(&id) else {
            return;
        };
        cx.emit(ExplorerEvent::Busy(BusyEvent::Ended {
            id,
            was_shown: work.shown,
            idle: self.ops.busy.is_empty(),
        }));
    }

    /// How many slow operations (copy, move, delete…) are running. An update waits for 0 before the app closes
    /// (`whenIdle`).
    pub fn busy_count(&self) -> usize {
        self.ops.busy.len()
    }
}

/// Runs `op` as slow work: its toast, and a copy or move into the queue where Settings say so.
pub(super) async fn run_op_busy(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    op: FileOp,
    what: Working,
) -> Result<bool, String> {
    let id = this.update(cx, |this, cx| this.begin_busy(what, cx)).ok();
    let result = run_op(this, cx, op).await;
    if let Some(id) = id {
        this.update(cx, |this, cx| this.end_busy(id, cx)).ok();
    }
    result
}

/// `fileOp`: a copy or move goes to the copy queue where Settings say copies do (it says what it did when it is
/// done); anything else goes through Windows' file engine, then Undo is offered for what it did. Resolves
/// `false` when the person cancelled in one of Windows' dialogs (what was done by then can still be undone).
pub(super) async fn run_op(this: &WeakEntity<Explorer>, cx: &mut AsyncApp, op: FileOp) -> Result<bool, String> {
    let Ok((source, prefs)) = this.update(cx, |this, cx| (this.source.clone(), Prefs::read(cx))) else {
        return Ok(false);
    };
    if prefs.copy_queue {
        let queued = match &op {
            FileOp::Copy { sources, dest } => Some((QueueKind::Copy, sources.clone(), dest.clone())),
            FileOp::Move { sources, dest } => Some((QueueKind::Move, sources.clone(), dest.clone())),
            _ => None,
        };
        if let Some((kind, sources, dest)) = queued {
            super::queue::queue_in(this, cx, kind, sources, dest).await;
            return Ok(true);
        }
    }
    let performed = source.file_op(op.clone()).await?;
    let completed = performed.completed;
    this.update(cx, |this, cx| this.after_file_op(&op, &performed, cx)).ok();
    Ok(completed)
}

impl Explorer {
    /// Runs a copy, move, rename, delete, restore, purge or Sync through Windows' file engine, as the TypeScript's
    /// `fileOp` does (Settings can send copies and moves through the copy queue; Undo is offered for what was
    /// done). Resolves `Ok(false)` if the person cancelled in one of Windows' dialogs. No toast of its own: the
    /// operations below wrap it in one.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn file_op(&mut self, op: FileOp, cx: &mut Context<Self>) -> Task<Result<bool, String>> {
        cx.spawn(async move |this, cx| run_op(&this, cx, op).await)
    }

    /// What was done is remembered for Undo and said with an Undo button (`fileOp`'s second half), and what is
    /// in the Recycle Bin may have changed.
    pub(super) fn after_file_op(&mut self, op: &FileOp, performed: &Performed, cx: &mut Context<Self>) {
        let done: Vec<(&str, &str)> = performed
            .items
            .iter()
            .filter_map(|item| item.to.as_deref().map(|to| (item.from.as_str(), to)))
            .collect();
        let count = done.len();
        if let Some(&(first_from, first_to)) = done.first() {
            let what = plural(count, "item");
            // Taking back a copy recycles it rather than deleting it for good.
            let recycle = || UndoAction::Recycle(done.iter().map(|(_, to)| (*to).to_string()).collect());
            // Where each item goes back to: its folder, under its name.
            let back = || -> Vec<MoveItem> {
                done.iter()
                    .map(|(from, to)| MoveItem {
                        path: (*to).to_string(),
                        folder: parent_path(from),
                        name: base_name(from).to_string(),
                    })
                    .collect()
            };
            match op {
                FileOp::Copy { dest, .. } => {
                    let undo = self.remember_undo(format!("copy of {what}"), recycle(), cx);
                    cx.emit(ExplorerEvent::Operation(OpNotice::Copied {
                        count,
                        dest: dest.clone(),
                        undo: Some(undo),
                    }));
                }
                // Sync's copies land in subfolders, out of sight: it says so.
                FileOp::CopyInto { .. } => {
                    let undo = self.remember_undo(format!("sync of {what}"), recycle(), cx);
                    cx.emit(ExplorerEvent::Operation(OpNotice::Synced { count, undo }));
                }
                FileOp::Move { dest, .. } => {
                    let undo = self.remember_undo(format!("move of {what}"), UndoAction::MoveBack(back()), cx);
                    cx.emit(ExplorerEvent::Operation(OpNotice::Moved {
                        count,
                        dest: dest.clone(),
                        undo,
                    }));
                }
                FileOp::Rename { .. } => {
                    let undo = self.remember_undo(
                        format!("rename to “{}”", base_name(first_to)),
                        UndoAction::Rename {
                            path: first_to.to_string(),
                            name: base_name(first_from).to_string(),
                        },
                        cx,
                    );
                    cx.emit(ExplorerEvent::Operation(OpNotice::Renamed {
                        to: first_to.to_string(),
                        undo,
                    }));
                }
                FileOp::Delete { .. } => {
                    let undo = self.remember_undo(format!("deletion of {what}"), UndoAction::Restore(back()), cx);
                    cx.emit(ExplorerEvent::Operation(OpNotice::Recycled { count, undo }));
                }
                FileOp::Restore { .. } => {
                    let undo = self.remember_undo(format!("restore of {what}"), recycle(), cx);
                    cx.emit(ExplorerEvent::Operation(OpNotice::Restored { count, undo }));
                }
                // Undo's own steps and delete-for-good have nothing to take back.
                FileOp::MoveBack { .. } | FileOp::Purge { .. } => {}
            }
        }
        // What's in the Recycle Bin changed.
        if matches!(
            op,
            FileOp::Delete { .. } | FileOp::Restore { .. } | FileOp::Purge { .. }
        ) {
            self.reload_recycle_bins(cx);
        }
    }

    /// Opens `folder` again in `tab` without a trip through history or a loading state, selecting `select` (the
    /// TypeScript's `navigate(path, { history: false, quiet: true, select, tab })` after an operation).
    pub(super) fn show_in(
        &mut self,
        tab: TabId,
        folder: &str,
        select: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        self.navigate_task(
            folder.to_string(),
            NavOptions {
                select,
                no_history: true,
                quiet: true,
                tab: Some(tab),
                ..Default::default()
            },
            cx,
        )
    }

    // ---- Copy to the other pane ------------------------------------------------------------------------

    /// The folder open in the other pane, when there is one to copy or move into: an archive isn't one, nor a
    /// folder Windows keeps closed, nor the Recycle Bin (`otherPaneFolder`).
    pub fn other_pane_folder(&self) -> Option<(TabId, String)> {
        if !self.dual {
            return None;
        }
        let other = self.panes.get(usize::from(self.active_pane == 0))?;
        let tab = self.tabs.get(&other.active)?;
        (tab.ready && is_folder_path(&tab.path) && tab.find.is_none() && tab.archive().is_none() && !tab.is_closed())
            .then(|| (tab.id, tab.path.clone()))
    }

    /// Dual-pane view: copies (or moves) items (the selection by default) to the folder open in the other pane,
    /// through Windows' file engine (`copyToOtherPane`). From an archive or a remote connection the archive part
    /// extracts them first ([`ArchiveJob::CopyTo`]).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn copy_to_other_pane(&mut self, move_: bool, paths: Option<Vec<String>>, cx: &mut Context<Self>) -> Task<()> {
        let tab_id = self.active_tab_id();
        let paths = paths.unwrap_or_else(|| self.tabs.get(&tab_id).map(|t| t.selection.to_vec()).unwrap_or_default());
        let Some((target_tab, target)) = self.other_pane_folder() else {
            let other = self
                .panes
                .get(usize::from(self.active_pane == 0))
                .map(|p| p.active)
                .and_then(|id| self.tabs.get(&id));
            let why = match other {
                Some(other) if other.archive().is_some() || other.is_remote() => OtherPane::ReadOnly {
                    remote: other.is_remote(),
                },
                Some(other) if other.is_closed() => OtherPane::Closed,
                _ => OtherPane::NoFolder,
            };
            cx.emit(ExplorerEvent::Operation(OpNotice::OtherPane(why)));
            return Task::ready(());
        };
        if move_ && self.closed_of(Some(tab_id)) {
            self.closed_notice(Some(tab_id), cx);
            return Task::ready(());
        }
        if self.recycle_bin_of(Some(tab_id)) {
            self.recycled_notice(cx);
            return Task::ready(());
        }
        if self.archive_of(Some(tab_id)).is_some() || self.remote_of(Some(tab_id)) {
            if move_ {
                self.read_only_notice(Some(tab_id), cx);
            } else if !paths.is_empty() {
                cx.emit(ExplorerEvent::FromArchive(ArchiveJob::CopyTo {
                    paths,
                    target_tab,
                    target,
                }));
            }
            return Task::ready(());
        }
        let sources: Vec<String> = paths
            .into_iter()
            .filter(|p| !same_path(&parent_path(p), &target) && !same_path(p, &target))
            .collect();
        if sources.is_empty() {
            return Task::ready(());
        }
        cx.spawn(async move |this, cx| {
            copy_into(&this, cx, move_, sources, target_tab, target, tab_id, true).await;
        })
    }

    /// Copies files already extracted from an archive into `target` (the other pane's folder), and shows them
    /// there: the second half of [`ArchiveJob::CopyTo`] (`copyOutOfArchive`). The archive part removes its staged
    /// copies afterwards.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn copy_staged(
        &mut self,
        staged: Vec<String>,
        target_tab: TabId,
        target: String,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if staged.is_empty() {
            return Task::ready(());
        }
        let from = self.active_tab_id();
        cx.spawn(async move |this, cx| {
            copy_into(&this, cx, false, staged, target_tab, target, from, false).await;
        })
    }

    // ---- Drops ------------------------------------------------------------------------------------------

    /// Files dropped on a folder (from File Explorer, the desktop, another pane or tab): copied, or moved when
    /// `move_` (Shift was held, as the drag's picture said); on the Recycle Bin, recycled (`dropFiles`). Which
    /// folder is under the pointer, and the drag's picture, are the drag and drop part's.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn drop_files(&mut self, paths: Vec<String>, dest: &str, move_: bool, cx: &mut Context<Self>) -> Task<()> {
        if is_remote_path(dest) {
            let tab = self.active_tab_id();
            self.read_only_notice(Some(tab), cx);
            return Task::ready(());
        }
        let dest = dest.to_string();
        if is_recycle_bin(&dest) {
            let count = paths.len();
            let confirm = Prefs::read(cx).confirm_delete;
            return cx.spawn(async move |this, cx| {
                let op = FileOp::Delete {
                    paths,
                    permanent: false,
                    confirm,
                };
                if let Err(message) = run_op_busy(&this, cx, op, Working::Deleting(count)).await {
                    notify_failed(&this, cx, Failing::DeleteItems { count }, message);
                }
            });
        }
        let sources = glint_model::paths::droppable(&paths, &dest);
        if sources.is_empty() {
            return Task::ready(());
        }
        let count = sources.len();
        cx.spawn(async move |this, cx| {
            let op = if move_ {
                FileOp::Move {
                    sources: sources.clone(),
                    dest: dest.clone(),
                }
            } else {
                FileOp::Copy {
                    sources: sources.clone(),
                    dest: dest.clone(),
                }
            };
            let working = if move_ {
                Working::Moving(count)
            } else {
                Working::Copying(count)
            };
            match run_op_busy(&this, cx, op, working).await {
                Ok(false) => {}
                Ok(true) => {
                    let shown = this
                        .update(cx, |this, cx| {
                            let showing = this.visible_tabs().into_iter().find(|id| {
                                this.tabs
                                    .get(id)
                                    .is_some_and(|t| t.find.is_none() && !t.is_this_pc() && same_path(&dest, &t.path))
                            });
                            let select = sources.iter().map(|p| join_path(&dest, base_name(p))).collect();
                            match showing {
                                Some(tab) => Some(this.show_in(tab, &dest, Some(select), cx)),
                                // A move said so already, with Undo.
                                None => {
                                    if !move_ {
                                        cx.emit(ExplorerEvent::Operation(OpNotice::Copied {
                                            count,
                                            dest: dest.clone(),
                                            undo: None,
                                        }));
                                    }
                                    None
                                }
                            }
                        })
                        .ok()
                        .flatten();
                    if let Some(navigation) = shown {
                        navigation.await;
                    }
                }
                Err(message) => notify_failed(&this, cx, Failing::CopyOrMove { moving: move_, count }, message),
            }
        })
    }

    // ---- Delete -----------------------------------------------------------------------------------------

    /// Moves to the Recycle Bin (or deletes for good), then selects `next` like Explorer does (`deletePaths`).
    /// Windows asks first where Settings say so. Read-only places say so instead.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn delete_paths(
        &mut self,
        paths: Vec<String>,
        permanent: bool,
        next: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        if paths.is_empty() {
            return Task::ready(());
        }
        let tab = self.active_tab_id();
        if self.archive_of(Some(tab)).is_some() || self.remote_of(Some(tab)) {
            self.read_only_notice(Some(tab), cx);
            return Task::ready(());
        }
        if self.closed_of(Some(tab)) {
            self.closed_notice(Some(tab), cx);
            return Task::ready(());
        }
        let confirm = Prefs::read(cx).confirm_delete;
        let count = paths.len();
        cx.spawn(async move |this, cx| {
            let op = FileOp::Delete {
                paths: paths.clone(),
                permanent,
                confirm,
            };
            match run_op_busy(&this, cx, op, Working::Deleting(count)).await {
                Err(message) => notify_failed(&this, cx, Failing::Delete, message),
                Ok(done) => {
                    let navigation = this
                        .update(cx, |this, cx| this.after_delete(tab, &paths, done, next.as_deref(), cx))
                        .ok()
                        .flatten();
                    if let Some(navigation) = navigation {
                        navigation.await;
                    }
                }
            }
        })
    }

    /// The folder is read again with the next item selected; a search's results lose what went instead.
    fn after_delete(
        &mut self,
        tab: TabId,
        paths: &[String],
        done: bool,
        next: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<Task<bool>> {
        let t = self.tabs.get(&tab)?;
        let Some(find) = &t.find else {
            // Cancelled in Windows' dialog: the selection stays; otherwise the next item (or none) is selected.
            let select = done.then(|| next.map(str::to_string).into_iter().collect());
            let path = t.path.clone();
            return Some(self.show_in(tab, &path, select, cx));
        };
        if done {
            let gone: HashSet<&str> = paths.iter().map(String::as_str).collect();
            let entries: Vec<_> = find
                .entries
                .iter()
                .filter(|e| !gone.contains(e.path.as_str()))
                .cloned()
                .collect();
            self.set_find_entries(tab, entries, next, cx);
        }
        None
    }

    /// Replaces a search's results and selects `select` (or nothing) in them.
    pub(super) fn set_find_entries(
        &mut self,
        tab: TabId,
        entries: Vec<glint_model::entries::FileEntry>,
        select: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.update_tab(tab, |t| {
            if let Some(find) = &mut t.find {
                find.entries = entries.into();
            }
            t.selection = super::tab::Selection {
                paths: std::sync::Arc::new(select.into_iter().collect()),
                anchor: select.map(str::to_string),
                focus: select.map(str::to_string),
            };
        });
        self.rebuild_rows(tab, cx);
        cx.emit(ExplorerEvent::SelectionChanged(tab));
        self.tab_changed(tab, cx);
    }
}

/// Copies or moves `sources` into `target`, shown in `target_tab`, and reads the folder the items left (`tab`)
/// again after a move. The common half of [`Explorer::copy_to_other_pane`] and [`Explorer::copy_staged`].
#[allow(clippy::too_many_arguments)] // The operation's own parts, passed through from the two callers.
async fn copy_into(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    move_: bool,
    sources: Vec<String>,
    target_tab: TabId,
    target: String,
    tab: TabId,
    reload_after_move: bool,
) {
    let count = sources.len();
    let op = if move_ {
        FileOp::Move {
            sources: sources.clone(),
            dest: target.clone(),
        }
    } else {
        FileOp::Copy {
            sources: sources.clone(),
            dest: target.clone(),
        }
    };
    let working = if move_ {
        Working::Moving(count)
    } else {
        Working::Copying(count)
    };
    match run_op_busy(this, cx, op, working).await {
        Ok(false) => {}
        Ok(true) => {
            let select = sources.iter().map(|p| join_path(&target, base_name(p))).collect();
            let navigation = this
                .update(cx, |this, cx| this.show_in(target_tab, &target, Some(select), cx))
                .ok();
            if let Some(navigation) = navigation {
                navigation.await;
            }
            if move_ && reload_after_move {
                this.update(cx, |this, cx| {
                    if this.tabs.get(&tab).is_some_and(|t| t.find.is_none()) {
                        this.reload(Some(tab), cx).detach();
                    }
                })
                .ok();
            }
        }
        Err(message) => notify_failed(this, cx, Failing::CopyOrMove { moving: move_, count }, message),
    }
}

/// Says an operation failed.
pub(super) fn notify_failed(this: &WeakEntity<Explorer>, cx: &mut AsyncApp, what: Failing, message: String) {
    this.update(cx, |_, cx| {
        cx.emit(ExplorerEvent::Operation(OpNotice::Failed { what, message }));
    })
    .ok();
}
