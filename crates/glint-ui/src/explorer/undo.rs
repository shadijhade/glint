//! Undo (Ctrl+Z): the last file operations, newest last, each with what takes it back. Ported from explorer.ts:
//! `UndoStep`, `rememberUndo`, `nextUndo`, `undo`.
//!
//! An operation that Ctrl+Z can take back (a copy, move, rename, deletion, new item, paste, Restore, a finished
//! copy-queue job) calls [`Explorer::remember_undo`] with what takes it back and says so with an
//! [`OpNotice`] that carries the step's [`UndoId`]: its **Undo** button calls [`Explorer::undo`] with it.

use super::model::Explorer;
use super::ops_state::{UndoStep, UNDO_MOST};
use super::ops_types::*;
use super::types::*;
use gpui_kit::{Context, Task};

impl Explorer {
    /// Keeps how to take back what was just done, for Ctrl+Z (the newest 50 steps) (`rememberUndo`). `label`
    /// completes "Undid the {label}": "move of 3 items".
    pub fn remember_undo(&mut self, label: impl Into<String>, action: UndoAction, cx: &mut Context<Self>) -> UndoId {
        self.ops.undo_seq += 1;
        let id = UndoId(self.ops.undo_seq);
        self.ops.undo.push(UndoStep {
            id,
            label: label.into(),
            action,
        });
        if self.ops.undo.len() > UNDO_MOST {
            self.ops.undo.remove(0);
        }
        cx.emit(ExplorerEvent::UndoChanged);
        id
    }

    /// What Ctrl+Z would take back now, if anything: its id and label (`nextUndo`).
    pub fn next_undo(&self) -> Option<(UndoId, &str)> {
        self.ops.undo.last().map(|s| (s.id, s.label.as_str()))
    }

    /// Takes back the last copy, move, rename, deletion or new item (Ctrl+Z), or `step` if it is still there to
    /// undo (`undo`). Says `NothingToUndo` when there is none. The folder on screen and the Recycle Bin are read
    /// again afterwards, as most undos recycle.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn undo(&mut self, step: Option<UndoId>, cx: &mut Context<Self>) -> Task<()> {
        let at = match step {
            Some(id) => self.ops.undo.iter().position(|s| s.id == id),
            None => self.ops.undo.len().checked_sub(1),
        };
        let Some(at) = at else {
            cx.emit(ExplorerEvent::Operation(OpNotice::NothingToUndo));
            return Task::ready(());
        };
        let step = self.ops.undo.remove(at);
        cx.emit(ExplorerEvent::UndoChanged);
        // What offered it (a toast with Undo) has done its job.
        cx.emit(ExplorerEvent::Operation(OpNotice::DismissUndo));
        let label = step.label;
        let busy = self.begin_busy(Working::Undoing(label.clone()), cx);
        let running = self.source.file_op(step.action.op());
        cx.spawn(async move |this, cx| {
            let result = running.await;
            this.update(cx, |this, cx| {
                this.end_busy(busy, cx);
                cx.emit(ExplorerEvent::Operation(match result {
                    Ok(_) => OpNotice::Undid { label },
                    Err(message) => OpNotice::UndoFailed { label, message },
                }));
                this.reload(None, cx).detach();
                // Undoing a deletion or a restore changes what's in the Recycle Bin, as most undos do (they recycle).
                this.reload_recycle_bins(cx);
            })
            .ok();
        })
    }
}
