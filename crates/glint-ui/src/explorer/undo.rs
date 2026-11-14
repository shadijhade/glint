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
use crate::{core_bridge, t};
use glint_core::batch_rename::RenamePair;
use glint_model::format::plural;
#[cfg(test)]
use gpui_kit::App;
use gpui_kit::{Context, Global, Task};
use std::sync::Arc;

/// glint-core, doing the steps that are not Windows' file engine: the batch renamer and `wof_undo`.
struct CoreRunner;

impl UndoRunner for CoreRunner {
    fn rename_batch(&self, pairs: Vec<BatchPair>) -> UndoFut<Result<(), String>> {
        let pairs = pairs
            .into_iter()
            .map(|pair| RenamePair {
                from: pair.from,
                to: pair.to,
            })
            .collect();
        let call = core_bridge::run(glint_core::batch_rename::batch_rename(pairs));
        Box::pin(async move { call.await.and_then(|inner| inner).map(|_| ()) })
    }

    fn wof_undo(&self, id: u64) -> UndoFut<Result<WofUndone, String>> {
        // Nobody watches an undo's progress here; the channel only has to outlive the call.
        let (channel, progress) = core_bridge::stream::<glint_core::wof::Progress>();
        let call = core_bridge::run(glint_core::wof::wof_undo(id, channel));
        Box::pin(async move {
            let _progress = progress;
            call.await.and_then(|inner| inner).map(|report| WofUndone {
                failed_count: report.failed_count,
                first_failure: report.failed.first().map(|(_, why)| why.clone()),
            })
        })
    }
}

/// What a taken-back compression says: files that wouldn't decompress stay compressed, so the undo didn't come
/// all the way, and the person is told how many and why (the web app's second toast, `Couldn't decompress {n}
/// files` with the first reason).
fn wof_outcome(undone: WofUndone) -> Result<(), String> {
    if undone.failed_count == 0 {
        return Ok(());
    }
    let head = t!("Couldn't decompress {what}", what = plural(undone.failed_count, "file"));
    Err(match undone.first_failure {
        Some(why) => format!("{head}: {why}"),
        None => head.to_string(),
    })
}

/// The runner a test installed (the app has none: it uses [`CoreRunner`]).
struct RunnerOverride(Arc<dyn UndoRunner>);

impl Global for RunnerOverride {}

impl Explorer {
    /// Sets where the steps that are not Windows' file engine (a batch rename, a compression in place) are
    /// taken back. Tests pass a fake; the app never calls this.
    #[cfg(test)]
    pub fn set_undo_runner(cx: &mut App, runner: Arc<dyn UndoRunner>) {
        cx.set_global(RunnerOverride(runner));
    }

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

    /// Whether `step` is still there to take back (a dialog that offers "Undo last batch" asks, as Ctrl+Z
    /// may have used the step up).
    pub fn has_undo(&self, step: UndoId) -> bool {
        self.ops.undo.iter().any(|s| s.id == step)
    }

    /// Drops `step` without taking it back, for work that does it itself and must not be done twice (the
    /// compress dialog's own Undo button, which shows the progress). Whether it was there.
    pub fn forget_undo(&mut self, step: UndoId, cx: &mut Context<Self>) -> bool {
        let Some(at) = self.ops.undo.iter().position(|s| s.id == step) else {
            return false;
        };
        self.ops.undo.remove(at);
        cx.emit(ExplorerEvent::UndoChanged);
        true
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
        let runs = step.action.runs();
        // Renamed items may be in folders that are not on screen as the active tab: each is read again.
        let folders = step.action.renamed_folders();
        let source = self.source.clone();
        let runner: Arc<dyn UndoRunner> = match cx.try_global::<RunnerOverride>() {
            Some(over) => over.0.clone(),
            None => Arc::new(CoreRunner),
        };
        cx.spawn(async move |this, cx| {
            // One after the other; the first to fail stops the rest.
            let mut result = Ok(());
            for run in runs {
                let done = match run {
                    UndoRun::File(op) => source.file_op(op).await.map(|_| ()),
                    UndoRun::RenameBatch(pairs) => runner.rename_batch(pairs).await,
                    // Files that wouldn't decompress stay compressed: the undo didn't come all the way, and
                    // the person is told which (the web app's second toast, "Couldn't decompress {n} files").
                    UndoRun::Wof(id) => runner.wof_undo(id).await.and_then(wof_outcome),
                };
                if let Err(message) = done {
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
                for folder in &folders {
                    this.reload_folder(folder, cx);
                }
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

    fn pair(from: &str, to: &str) -> BatchPair {
        BatchPair {
            from: from.into(),
            to: to.into(),
        }
    }

    /// A batch that gave `1.txt` and `2.txt` new names, as its undo pairs say them (new name back to old).
    fn batch() -> UndoAction {
        UndoAction::RenameBatch(vec![
            pair("C:\\a\\1.txt", "C:\\a\\one.txt"),
            pair("C:\\a\\2.txt", "C:\\a\\two.txt"),
        ])
    }

    type Rig = (FakeDisk, gpui_kit::Entity<Explorer>, std::sync::Arc<FakeUndoRunner>);

    fn explorer_in_a(cx: &mut TestAppContext) -> Rig {
        cx.update(testing::init);
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![folder("C:\\", "a")]);
        disk.folder(
            "C:\\a",
            vec![file("C:\\a", "1.txt", 1, 1), file("C:\\a", "2.txt", 1, 1)],
        );
        let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
        let runner = cx.update(FakeUndoRunner::install);
        let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\a", NavOptions::default(), cx));
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        (disk, explorer, runner)
    }

    fn told_by(
        cx: &mut TestAppContext,
        explorer: &gpui_kit::Entity<Explorer>,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<OpNotice>>> {
        let told = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = told.clone();
        cx.update(|cx| {
            cx.subscribe(explorer, move |_, event: &ExplorerEvent, _| {
                if let ExplorerEvent::Operation(n) = event {
                    sink.borrow_mut().push(n.clone())
                }
            })
            .detach();
        });
        told
    }

    fn undo_now(cx: &mut TestAppContext, explorer: &gpui_kit::Entity<Explorer>, step: Option<UndoId>) {
        let task = explorer.update(cx, |e, cx| e.undo(step, cx));
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
    }

    #[test]
    fn a_batch_and_a_compression_are_not_file_operations() {
        assert!(batch().ops().is_empty());
        assert!(UndoAction::Wof(3).ops().is_empty());
        assert_eq!(UndoAction::Wof(3).op(), None);
        assert_eq!(UndoAction::Wof(3).runs(), [UndoRun::Wof(3)]);
        match batch().runs().as_slice() {
            [UndoRun::RenameBatch(pairs)] => assert_eq!(pairs.len(), 2),
            other => panic!("{other:?}"),
        }
        // In a sequence they run in the same last-recorded-first order as the file operations.
        let mixed = UndoAction::Sequence(vec![UndoAction::Recycle(vec!["C:\\a\\x".into()]), batch()]);
        let runs = mixed.runs();
        assert!(matches!(runs[0], UndoRun::RenameBatch(_)));
        assert!(matches!(runs[1], UndoRun::File(FileOp::Delete { .. })));
        assert_eq!(mixed.ops().len(), 1);
    }

    #[test]
    fn a_batch_names_the_folders_it_touched_once_each() {
        let in_folders = UndoAction::RenameBatch(vec![
            pair("C:\\a\\1.txt", "C:\\a\\one.txt"),
            pair("c:\\A\\2.txt", "c:\\A\\two.txt"),
            pair("C:\\b\\3.txt", "C:\\b\\three.txt"),
            pair("C:\\تقارير\\٣.txt", "C:\\تقارير\\ثلاثة.txt"),
        ]);
        assert_eq!(
            in_folders.renamed_folders(),
            ["C:\\a", "C:\\b", "C:\\تقارير"].map(String::from)
        );
        assert!(UndoAction::Wof(1).renamed_folders().is_empty());
        let nested = UndoAction::Sequence(vec![batch(), UndoAction::Recycle(vec![])]);
        assert_eq!(nested.renamed_folders(), ["C:\\a".to_string()]);
    }

    #[gpui_kit::test]
    fn ctrl_z_takes_a_batch_rename_back_through_the_runner_and_reads_the_folder_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (disk, explorer, runner) = explorer_in_a(cx);
        let told = told_by(cx, &explorer);
        let id = explorer.update(cx, |e, cx| e.remember_undo("rename of 2 items", batch(), cx));
        assert_eq!(
            explorer.read_with(cx, |e, _| e.next_undo().map(|(i, l)| (i, l.to_string()))),
            Some((id, "rename of 2 items".to_string()))
        );
        assert!(explorer.read_with(cx, |e, _| e.has_undo(id)));
        let listed = |disk: &FakeDisk| disk.listed().iter().filter(|f| f.as_str() == "C:\\a").count();
        let before = listed(&disk);

        undo_now(cx, &explorer, None);

        // The pairs that put the names back went to the batch renamer in one call, and no file operation ran.
        assert_eq!(
            runner.renamed(),
            [vec![
                pair("C:\\a\\1.txt", "C:\\a\\one.txt"),
                pair("C:\\a\\2.txt", "C:\\a\\two.txt"),
            ]]
        );
        assert!(disk.ops().is_empty());
        assert!(told.borrow().contains(&OpNotice::Undid {
            label: "rename of 2 items".into()
        }));
        // The folder on screen was listed again, and the step is used up.
        assert!(listed(&disk) > before);
        assert!(explorer.read_with(cx, |e, _| e.next_undo().is_none() && !e.has_undo(id)));
    }

    #[gpui_kit::test]
    fn a_batch_sits_on_the_undo_stack_among_the_other_steps(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (disk, explorer, runner) = explorer_in_a(cx);
        explorer.update(cx, |e, cx| {
            e.remember_undo(
                "copy of 1 item",
                UndoAction::Recycle(vec!["C:\\a\\copy.txt".into()]),
                cx,
            );
            e.remember_undo("rename of 2 items", batch(), cx);
            e.remember_undo("compressing 3 files in place", UndoAction::Wof(9), cx);
        });
        let next = |cx: &TestAppContext| explorer.read_with(cx, |e, _| e.next_undo().map(|(_, l)| l.to_string()));
        // Newest first, one Ctrl+Z each.
        assert_eq!(next(cx).as_deref(), Some("compressing 3 files in place"));
        for expected in [Some("rename of 2 items"), Some("copy of 1 item"), None] {
            undo_now(cx, &explorer, None);
            assert_eq!(next(cx).as_deref(), expected);
        }
        assert_eq!(runner.wof_asked(), [9]);
        assert_eq!(runner.renamed().len(), 1);
        // The copy was recycled last, after both of the newer steps.
        assert!(matches!(disk.ops().as_slice(), [FileOp::Delete { .. }]));
    }

    #[gpui_kit::test]
    fn a_step_can_be_taken_back_out_of_order_by_its_id(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (_disk, explorer, runner) = explorer_in_a(cx);
        let (first, second) = explorer.update(cx, |e, cx| {
            (
                e.remember_undo("extensions of 2 files", batch(), cx),
                e.remember_undo("compressing 1 file in place", UndoAction::Wof(4), cx),
            )
        });
        undo_now(cx, &explorer, Some(first));
        assert_eq!(runner.renamed().len(), 1);
        assert!(runner.wof_asked().is_empty());
        assert!(explorer.read_with(cx, |e, _| e.has_undo(second) && !e.has_undo(first)));
    }

    #[gpui_kit::test]
    fn a_batch_that_cannot_be_renamed_back_is_told_and_used_up(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (_disk, explorer, runner) = explorer_in_a(cx);
        let told = told_by(cx, &explorer);
        explorer.update(cx, |e, cx| e.remember_undo("rename of 2 items", batch(), cx));
        *runner.fail.lock().unwrap() = Some("“one.txt” is taken".into());
        undo_now(cx, &explorer, None);
        assert!(told.borrow().contains(&OpNotice::UndoFailed {
            label: "rename of 2 items".into(),
            message: "“one.txt” is taken".into()
        }));
        assert!(explorer.read_with(cx, |e, _| e.next_undo().is_none()));
    }

    #[gpui_kit::test]
    fn ctrl_z_decompresses_what_a_run_compressed_and_tells_the_files_that_wouldnt(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (_disk, explorer, runner) = explorer_in_a(cx);
        let told = told_by(cx, &explorer);
        explorer.update(cx, |e, cx| {
            e.remember_undo("compressing 5 files in place", UndoAction::Wof(7), cx);
        });
        undo_now(cx, &explorer, None);
        assert_eq!(runner.wof_asked(), [7]);
        assert!(told.borrow().contains(&OpNotice::Undid {
            label: "compressing 5 files in place".into()
        }));

        // Two files stayed compressed: that is said, with the first reason.
        *runner.undone.lock().unwrap() = WofUndone {
            failed_count: 2,
            first_failure: Some("Access is denied".into()),
        };
        explorer.update(cx, |e, cx| {
            e.remember_undo("compressing 5 files in place", UndoAction::Wof(8), cx);
        });
        undo_now(cx, &explorer, None);
        assert_eq!(runner.wof_asked(), [7, 8]);
        assert!(told.borrow().contains(&OpNotice::UndoFailed {
            label: "compressing 5 files in place".into(),
            message: "Couldn't decompress 2 files: Access is denied".into()
        }));
    }

    #[gpui_kit::test]
    fn forgetting_a_step_drops_it_without_running_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (_disk, explorer, runner) = explorer_in_a(cx);
        let id = explorer.update(cx, |e, cx| {
            e.remember_undo("compressing 1 file in place", UndoAction::Wof(2), cx)
        });
        assert!(explorer.update(cx, |e, cx| e.forget_undo(id, cx)));
        assert!(!explorer.update(cx, |e, cx| e.forget_undo(id, cx)));
        assert!(explorer.read_with(cx, |e, _| e.next_undo().is_none()));
        assert!(runner.wof_asked().is_empty());
    }
}
