//! The basket: files gathered from several folders to be moved, copied, zipped or renamed together.
//!
//! Items are added from the keyboard (Ctrl+Shift+B), the palette or by dropping them on the dock, and stay while
//! the person goes from folder to folder. The basket is a plain list of paths in the [`Explorer`]: it costs
//! nothing while it is empty (no timer, no watcher, no redraw), and it is checked against the disk only when it
//! is used, so an item moved or deleted in the meantime is dropped then ([`Explorer::prune_basket`]).

use super::model::Explorer;
use super::ops_types::*;
use super::queue::queue_in;
use super::types::*;
use glint_model::paths::{in_archive_path, is_recycle_bin, is_remote_path, parent_path, same_path};
use gpui_kit::{Context, Task};

impl Explorer {
    /// What the basket holds, in the order it was filled.
    pub fn basket(&self) -> &[String] {
        &self.ops.basket
    }

    /// These paths as rows (size, date, kind), read in the background: what the basket's drawer shows of its items.
    /// One that can't be read still gets its row.
    pub fn basket_rows(&self, paths: Vec<String>) -> super::source::Fut<Vec<glint_model::entries::FileEntry>> {
        self.source.entries_of(paths)
    }

    /// Adds `paths` to the basket (Ctrl+Shift+B). What is in it already, and what can't be moved or copied from
    /// where it is (inside an archive, on a remote connection, in the Recycle Bin), is left out. Says how many
    /// were added.
    pub fn add_to_basket(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let mut added = 0;
        for path in paths {
            if path.is_empty() || is_remote_path(&path) || in_archive_path(&path) || is_recycle_bin(&path) {
                continue;
            }
            if self.ops.basket.iter().any(|held| same_path(held, &path)) {
                continue;
            }
            self.ops.basket.push(path);
            added += 1;
        }
        cx.emit(ExplorerEvent::Operation(OpNotice::Basketed {
            added,
            total: self.ops.basket.len(),
        }));
        if added > 0 {
            cx.notify();
        }
    }

    /// Takes one item out of the basket.
    pub fn remove_from_basket(&mut self, path: &str, cx: &mut Context<Self>) {
        let before = self.ops.basket.len();
        self.ops.basket.retain(|held| !same_path(held, path));
        if self.ops.basket.len() != before {
            cx.notify();
        }
    }

    /// Empties the basket. The files stay where they are.
    pub fn empty_basket(&mut self, cx: &mut Context<Self>) {
        if !self.ops.basket.is_empty() {
            self.ops.basket.clear();
            cx.notify();
        }
    }

    /// Drops what is no longer where it was (moved, renamed or deleted since it was added), looking in the
    /// background. The task is done when the basket is as it should be.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn prune_basket(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.ops.basket.is_empty() {
            return Task::ready(());
        }
        let looking = self.source.missing(self.ops.basket.clone());
        cx.spawn(async move |this, cx| {
            let gone = looking.await;
            if gone.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                this.ops.basket.retain(|held| !gone.iter().any(|g| same_path(g, held)));
                cx.notify();
            })
            .ok();
        })
    }

    /// "Move here" and "Copy here": what the basket holds goes into the folder shown, through the copy queue. A
    /// move empties the basket (its items are not where they were); a copy keeps it for the next folder.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn basket_to_here(&mut self, kind: QueueKind, cx: &mut Context<Self>) -> Task<()> {
        let dest = self.current_path().to_string();
        if self.paste_target(&dest, cx).is_none() {
            return Task::ready(());
        }
        let pruning = self.prune_basket(cx);
        cx.spawn(async move |this, cx| {
            pruning.await;
            let Ok(items) = this.update(cx, |this, _| this.ops.basket.clone()) else {
                return;
            };
            // Moving an item into the folder it is in does nothing.
            let items: Vec<String> = items
                .into_iter()
                .filter(|item| kind == QueueKind::Copy || !same_path(&parent_path(item), &dest))
                .collect();
            if items.is_empty() {
                return;
            }
            let moved = items.clone();
            queue_in(&this, cx, kind, items, dest).await;
            if kind == QueueKind::Move {
                this.update(cx, |this, cx| {
                    this.ops.basket.retain(|held| !moved.iter().any(|m| same_path(m, held)));
                    cx.notify();
                })
                .ok();
            }
        })
    }

    /// "Batch rename": the batch rename dialog opens with what the basket holds.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn basket_batch_rename(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let pruning = self.prune_basket(cx);
        cx.spawn(async move |this, cx| {
            pruning.await;
            this.update(cx, |this, cx| {
                let items = this.ops.basket.clone();
                if !items.is_empty() {
                    cx.emit(ExplorerEvent::BatchRename(items));
                }
            })
            .ok();
        })
    }
}
