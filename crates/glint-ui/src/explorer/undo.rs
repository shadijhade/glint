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
        let ops = step.action.ops();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            // One after the other; the first to fail stops the rest.
            let mut result = Ok(());
            for op in ops {
                if let Err(message) = source.file_op(op).await {
                    result = Err(message);
                    break;
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::*;
    use crate::testing;
    use gpui_kit::{AppContext as _, TestAppContext};

    fn item(name: &str) -> MoveItem {
        MoveItem {
            path: format!("C:\\bin\\{name}"),
            folder: "C:\\a".into(),
            name: name.into(),
        }
    }

    fn sequence() -> UndoAction {
        UndoAction::Sequence(vec![
            UndoAction::Restore(vec![item("old.txt")]),
            UndoAction::Recycle(vec!["C:\\a\\new.txt".into()]),
        ])
    }

    #[test]
    fn a_sequence_runs_its_last_recorded_action_first() {
        let ops = sequence().ops();
        assert_eq!(
            ops,
            [
                FileOp::Delete {
                    paths: vec!["C:\\a\\new.txt".into()],
                    permanent: false,
                    confirm: false
                },
                FileOp::Restore {
                    items: vec![item("old.txt")]
                },
            ]
        );
        // Nested sequences flatten in the same order; a single action is one op.
        let nested = UndoAction::Sequence(vec![UndoAction::Recycle(vec!["x".into()]), sequence()]);
        assert_eq!(nested.ops().len(), 3);
        assert_eq!(
            nested.ops()[2],
            FileOp::Delete {
                paths: vec!["x".into()],
                permanent: false,
                confirm: false
            }
        );
        assert_eq!(UndoAction::Recycle(vec![]).ops().len(), 1);
    }

    #[gpui_kit::test]
    fn one_undo_takes_back_a_whole_sequence_with_one_label(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![folder("C:\\", "a"), folder("C:\\", "bin")]);
        disk.folder("C:\\a", vec![file("C:\\a", "new.txt", 1, 1)]);
        disk.folder("C:\\bin", vec![file("C:\\bin", "old.txt", 1, 1)]);
        let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
        let id = explorer.update(cx, |e, cx| e.remember_undo("run of 1 file", sequence(), cx));
        cx.run_until_parked();
        let ((got, label), steps) = explorer.read_with(cx, |e, _| {
            (
                e.next_undo().map(|(i, l)| (i, l.to_string())).unwrap(),
                e.ops.undo.len(),
            )
        });
        assert_eq!((got, label.as_str(), steps), (id, "run of 1 file", 1));

        let task = explorer.update(cx, |e, cx| e.undo(None, cx));
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        let ran = disk.ops();
        assert_eq!(ran.len(), 2);
        assert!(matches!(ran[0], FileOp::Delete { .. }));
        assert!(matches!(ran[1], FileOp::Restore { .. }));
        // The made file is gone and the replaced one is back.
        assert!(!disk.exists("C:\\a\\new.txt"));
        assert!(disk.exists("C:\\a\\old.txt"));
        assert!(explorer.read_with(cx, |e, _| e.next_undo().is_none()));
    }

    #[gpui_kit::test]
    fn a_failing_step_of_a_sequence_stops_the_rest_and_is_told(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![]);
        let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
        let told = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = told.clone();
        cx.update(|cx| {
            cx.subscribe(&explorer, move |_, event: &ExplorerEvent, _| {
                if let ExplorerEvent::Operation(n) = event {
                    sink.borrow_mut().push(n.clone())
                }
            })
            .detach();
        });
        explorer.update(cx, |e, cx| e.remember_undo("run", sequence(), cx));
        disk.fail_next("In use");
        let task = explorer.update(cx, |e, cx| e.undo(None, cx));
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        // Only the first step was tried; the restore after it never ran.
        let ran = disk.ops();
        assert_eq!(ran.len(), 1);
        assert!(matches!(ran[0], FileOp::Delete { .. }));
        assert!(told.borrow().contains(&OpNotice::UndoFailed {
            label: "run".into(),
            message: "In use".into()
        }));
    }
}
