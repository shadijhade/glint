//! The Recycle Bin: Restore, Delete for good and Empty. Ported from explorer.ts: `reloadRecycleBins`,
//! `restoreRecycled`, `purgeRecycled`, `emptyRecycleBin`.
//!
//! What's in the bin is restored or deleted for good, never opened or changed where it is (the file list says
//! so). Restore puts each item back into the folder it was deleted from, made again if it's gone, with Undo.
//! Deleting for good is asked first ([`Ask::Purge`]); emptying the bin is asked by Windows itself.

use super::model::Explorer;
use super::ops::{notify_failed, run_op_busy};
use super::ops_types::*;
use super::types::*;
use glint_model::entries::{is_recycled, FileEntry};
use glint_model::format::plural;
use gpui_kit::{Context, Task};

impl Explorer {
    /// Re-reads the Recycle Bin in the tabs on screen that show it: nothing watches it, so after what's done here
    /// and when the window is back in front (`reloadRecycleBins`).
    pub fn reload_recycle_bins(&mut self, cx: &mut Context<Self>) {
        for id in self.visible_tabs() {
            if self.recycle_bin_of(Some(id)) {
                self.reload(Some(id), cx).detach();
            }
        }
    }

    /// Restore in the Recycle Bin: each item back into the folder it was deleted from (made again if it's gone),
    /// with Undo (`restoreRecycled`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn restore_recycled(&mut self, entries: &[FileEntry], cx: &mut Context<Self>) -> Task<()> {
        let items: Vec<MoveItem> = entries
            .iter()
            .filter(|e| is_recycled(e))
            .filter_map(|e| {
                e.origin.as_ref().map(|folder| MoveItem {
                    path: e.path.clone(),
                    folder: folder.clone(),
                    name: e.name.clone(),
                })
            })
            .collect();
        if items.is_empty() {
            return Task::ready(());
        }
        let count = items.len();
        cx.spawn(async move |this, cx| {
            let op = FileOp::Restore { items };
            if let Err(message) = run_op_busy(&this, cx, op, Working::Restoring(count)).await {
                notify_failed(&this, cx, Failing::Restore, message);
            }
        })
    }

    /// Delete for good in the Recycle Bin: asks first, naming the items as they were ([`Ask::Purge`]); the answer
    /// yes calls [`Explorer::confirm_purge`]. `next` is selected afterwards (`purgeRecycled`).
    pub fn purge_recycled(&mut self, entries: &[FileEntry], next: Option<&str>, cx: &mut Context<Self>) {
        let items: Vec<&FileEntry> = entries.iter().filter(|e| is_recycled(e)).collect();
        if items.is_empty() {
            return;
        }
        cx.emit(ExplorerEvent::Ask(Ask::Purge(PurgeAsk {
            tab: self.active_tab_id(),
            paths: items.iter().map(|e| e.path.clone()).collect(),
            names: items.iter().map(|e| e.name.clone()).collect(),
            next: next.map(str::to_string),
        })));
    }

    /// The question of [`Ask::Purge`] was answered yes: deletes the items for good, then selects the next one
    /// like Explorer does.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn confirm_purge(&mut self, ask: PurgeAsk, cx: &mut Context<Self>) -> Task<()> {
        if ask.paths.is_empty() {
            return Task::ready(());
        }
        let count = ask.paths.len();
        cx.spawn(async move |this, cx| {
            let op = FileOp::Purge { paths: ask.paths };
            match run_op_busy(&this, cx, op, Working::Deleting(count)).await {
                Err(message) => notify_failed(&this, cx, Failing::Delete, message),
                Ok(_) => {
                    this.update(cx, |this, cx| {
                        let in_bin = this.recycle_bin_of(Some(ask.tab));
                        if let (Some(next), true) = (ask.next.as_deref(), in_bin) {
                            this.update_tab(ask.tab, |t| {
                                t.selection = super::tab::Selection::of([next]);
                                t.selection.anchor = Some(next.to_string());
                                t.selection.focus = Some(next.to_string());
                            });
                            cx.emit(ExplorerEvent::SelectionChanged(ask.tab));
                            this.tab_changed(ask.tab, cx);
                        }
                    })
                    .ok();
                }
            }
        })
    }

    /// Empties the Recycle Bin, every drive's, after Windows' own question (`emptyRecycleBin`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn empty_recycle_bin(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let emptying = self.source.recycle_empty();
        cx.spawn(async move |this, cx| {
            let result = emptying.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(true) => cx.emit(ExplorerEvent::Operation(OpNotice::EmptiedBin)),
                    // Windows asked and the answer was no.
                    Ok(false) => {}
                    Err(message) => cx.emit(ExplorerEvent::Operation(OpNotice::Failed {
                        what: Failing::EmptyBin,
                        message,
                    })),
                }
                this.reload_recycle_bins(cx);
            })
            .ok();
        })
    }
}

/// The words that name what `purge_recycled` asks about: `“{name}”` for one item, else `{count} items`
/// (`what` in `purgeRecycled`). English; the workspace's dialog translates the sentence around it.
pub fn purge_what(ask: &PurgeAsk) -> String {
    match ask.names.as_slice() {
        [only] => format!("“{only}”"),
        names => plural(names.len(), "item"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_purge_question_names_one_item_and_counts_several() {
        let ask = |names: &[&str]| PurgeAsk {
            tab: TabId(1),
            paths: names.iter().map(|n| format!("C:\\$Recycle.Bin\\{n}")).collect(),
            names: names.iter().map(|n| n.to_string()).collect(),
            next: None,
        };
        assert_eq!(purge_what(&ask(&["old.txt"])), "“old.txt”");
        assert_eq!(purge_what(&ask(&["a", "b", "c"])), "3 items");
    }
}
