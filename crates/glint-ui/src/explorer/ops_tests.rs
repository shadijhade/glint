//! Headless tests of the file operations: copy, move, delete, drops, clipboard, rename, new items, the
//! Recycle Bin, Undo, the copy queue and the views' small asks. They run on the fake disk behind the same
//! `Source` the app uses for glint-core, which really moves entries about, so every test checks what ended up
//! on the "disk", what the tab shows and what the person is told.

use super::fake::*;
use super::*;
use crate::testing;
use glint_model::columns::ColumnId;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

struct Harness {
    explorer: Entity<Explorer>,
    disk: FakeDisk,
    events: Rc<RefCell<Vec<ExplorerEvent>>>,
}

fn harness(cx: &mut TestAppContext) -> Harness {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|cx| {
        cx.subscribe(&explorer, move |_, event: &ExplorerEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    fixture(&disk);
    Harness { explorer, disk, events }
}

/// `C:\` with folders a, b and c. `a` has a subfolder, two files and an Arabic one.
fn fixture(disk: &FakeDisk) {
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), folder("C:\\", "c")],
    );
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "sub"),
            file("C:\\a", "one.txt", 100, 10),
            file("C:\\a", "two.txt", 200, 20),
            file("C:\\a", "تقرير.pdf", 300, 30),
        ],
    );
    disk.folder("C:\\a\\sub", vec![file("C:\\a\\sub", "deep.txt", 1, 1)]);
    disk.folder("C:\\b", vec![file("C:\\b", "b1.txt", 1, 1)]);
    disk.folder("C:\\c", vec![]);
}

impl Harness {
    fn go(&self, cx: &mut TestAppContext, path: &str) -> bool {
        let task = self
            .explorer
            .update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        let done = cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        done
    }

    /// Runs an operation and everything it sets going.
    fn task<T: 'static>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Explorer, &mut gpui_kit::Context<Explorer>) -> gpui_kit::Task<T>,
    ) -> T {
        let task = self.explorer.update(cx, f);
        let result = cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        result
    }

    fn run<R>(
        &self,
        cx: &mut TestAppContext,
        f: impl FnOnce(&mut Explorer, &mut gpui_kit::Context<Explorer>) -> R,
    ) -> R {
        let result = self.explorer.update(cx, f);
        cx.run_until_parked();
        result
    }

    fn read<R>(&self, cx: &TestAppContext, f: impl FnOnce(&Explorer) -> R) -> R {
        self.explorer.read_with(cx, |e, _| f(e))
    }

    fn names(&self, cx: &TestAppContext) -> Vec<String> {
        self.read(cx, |e| {
            e.active_tab()
                .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    fn selected(&self, cx: &TestAppContext) -> Vec<String> {
        self.read(cx, |e| {
            e.active_tab()
                .map(|t| {
                    t.selection
                        .iter()
                        .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    fn renaming(&self, cx: &TestAppContext) -> Option<String> {
        self.read(cx, |e| e.active_tab().and_then(|t| t.renaming.clone()))
    }

    fn events(&self) -> Vec<ExplorerEvent> {
        self.events.borrow().clone()
    }

    /// What the file operations told the person, in order.
    fn said(&self) -> Vec<OpNotice> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Operation(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    fn notices(&self) -> Vec<Notice> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Notice(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    /// The undo offered by the newest notice that offers one.
    fn last_undo(&self) -> Option<UndoId> {
        self.said().into_iter().rev().find_map(|n| match n {
            OpNotice::Copied { undo, .. } => undo,
            OpNotice::Moved { undo, .. }
            | OpNotice::Synced { undo, .. }
            | OpNotice::Renamed { undo, .. }
            | OpNotice::Recycled { undo, .. }
            | OpNotice::Restored { undo, .. } => Some(undo),
            _ => None,
        })
    }

    fn next_undo(&self, cx: &TestAppContext) -> Option<(UndoId, String)> {
        self.read(cx, |e| e.next_undo().map(|(id, label)| (id, label.to_string())))
    }

    /// Reads a copy-queue event in and waits for the explorer to have handled it.
    fn queue_says(&self, cx: &mut TestAppContext, jobs: serde_json::Value) {
        cx.update(|cx| crate::core_bridge::emit(cx, "copy-queue", jobs));
        for _ in 0..10 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn path_of(dir: &str, name: &str) -> String {
    format!("{dir}\\{name}")
}

// ---- Paste, copy and move ---------------------------------------------------------------------------------------

#[gpui_kit::test]
fn pasting_copies_into_the_open_folder_selects_the_copies_and_offers_undo(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    h.disk.set_clipboard(&["C:\\a\\one.txt", "C:\\a\\sub"], false);
    h.task(cx, |e, cx| e.paste(None, cx));

    assert_eq!(h.disk.names("C:\\b"), ["sub", "b1.txt", "one.txt"]);
    assert!(
        h.disk.exists("C:\\b\\sub\\deep.txt"),
        "a folder is copied with what is in it"
    );
    assert!(h.disk.exists("C:\\a\\one.txt"), "a copy leaves the original");
    // The folder was read again, and the copies are selected (and flash).
    assert_eq!(h.names(cx), ["sub", "b1.txt", "one.txt"]);
    assert_eq!(h.selected(cx), ["one.txt", "sub"]);
    h.read(cx, |e| assert!(e.flashed().contains("C:\\b\\one.txt")));
    assert!(h.said().iter().any(|n| matches!(
        n,
        OpNotice::Copied { count: 2, dest, undo: Some(_) } if dest == "C:\\b"
    )));
    assert!(matches!(
        &h.disk.ops()[..],
        [FileOp::Copy { sources, dest }] if sources.len() == 2 && dest == "C:\\b"
    ));
    // A copy is not cut: the clipboard keeps its files.
    assert!(h.disk.clipboard().is_some());

    // Undo recycles the copies instead of deleting them for good.
    let (id, label) = h.next_undo(cx).expect("undo was remembered");
    assert_eq!(label, "copy of 2 items");
    assert_eq!(Some(id), h.last_undo());
    h.task(cx, |e, cx| e.undo(Some(id), cx));
    assert_eq!(h.disk.names("C:\\b"), ["b1.txt"]);
    assert_eq!(h.disk.bin_names(), ["sub", "one.txt"]);
    assert_eq!(h.names(cx), ["b1.txt"], "the folder on screen is read again");
    assert!(h.said().contains(&OpNotice::DismissUndo));
    assert!(h.said().contains(&OpNotice::Undid {
        label: "copy of 2 items".into()
    }));
    assert_eq!(h.next_undo(cx), None);
}

#[gpui_kit::test]
fn pasting_cut_files_moves_them_clears_the_clipboard_and_undo_moves_them_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    // Cut in the app: the items show dimmed until pasted.
    h.task(cx, |e, cx| {
        e.copy_to_clipboard(vec!["C:\\a\\تقرير.pdf".into()], true, cx)
    });
    h.read(cx, |e| assert!(e.cut_paths().contains("C:\\a\\تقرير.pdf")));
    assert_eq!(
        h.disk.clipboard(),
        Some(ClipboardFiles {
            paths: vec!["C:\\a\\تقرير.pdf".into()],
            cut: true
        })
    );
    assert!(h.go(cx, "C:\\c"));
    h.task(cx, |e, cx| e.paste(None, cx));

    assert_eq!(h.disk.names("C:\\c"), ["تقرير.pdf"]);
    assert!(!h.disk.exists("C:\\a\\تقرير.pdf"), "a move takes the original away");
    assert_eq!(h.disk.clipboard(), None, "a pasted cut empties the clipboard");
    h.read(cx, |e| assert!(e.cut_paths().is_empty()));
    assert_eq!(h.selected(cx), ["تقرير.pdf"]);
    assert!(h.said().iter().any(|n| matches!(
        n,
        OpNotice::Moved { count: 1, dest, .. } if dest == "C:\\c"
    )));

    let (_, label) = h.next_undo(cx).expect("undo");
    assert_eq!(label, "move of 1 item");
    h.task(cx, |e, cx| e.undo(None, cx));
    assert!(h.disk.exists("C:\\a\\تقرير.pdf"), "Undo puts it back where it was");
    assert!(!h.disk.exists("C:\\c\\تقرير.pdf"));
}

#[gpui_kit::test]
fn copying_to_the_clipboard_marks_cut_items_and_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.copy_to_clipboard(vec!["C:\\a\\one.txt".into(), "C:\\a\\two.txt".into()], true, cx)
    });
    assert_eq!(h.read(cx, |e| e.cut_paths().len()), 2);
    assert!(h.events().contains(&ExplorerEvent::CutChanged));
    assert!(h.said().contains(&OpNotice::OnClipboard { count: 2, cut: true }));
    // Copying afterwards is not cutting: nothing shows dimmed any more.
    h.task(cx, |e, cx| {
        e.copy_to_clipboard(vec!["C:\\a\\one.txt".into()], false, cx)
    });
    h.read(cx, |e| assert!(e.cut_paths().is_empty()));
    assert!(h.said().contains(&OpNotice::OnClipboard { count: 1, cut: false }));
    // Nothing selected, nothing done.
    let before = h.said().len();
    h.task(cx, |e, cx| e.copy_to_clipboard(vec![], false, cx));
    assert_eq!(h.said().len(), before);
    // The clipboard being busy is told.
    h.disk.fail_clipboard("Another program is using the clipboard");
    h.task(cx, |e, cx| {
        e.copy_to_clipboard(vec!["C:\\a\\one.txt".into()], false, cx)
    });
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::Clipboard,
        message: "Another program is using the clipboard".into()
    }));
}

#[gpui_kit::test]
fn the_recycle_bin_and_closed_folders_cant_be_cut_or_copied_from(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    h.task(cx, |e, cx| {
        e.copy_to_clipboard(vec!["C:\\$Recycle.Bin\\$R1x".into()], false, cx)
    });
    assert_eq!(h.notices(), [Notice::InRecycleBin]);
    assert_eq!(h.disk.clipboard(), None);

    h.disk.folder("C:\\secret", vec![file("C:\\secret", "plans.txt", 5, 5)]);
    h.disk.deny("C:\\secret");
    assert!(h.go(cx, "C:\\secret"));
    h.task(cx, |e, cx| {
        e.copy_to_clipboard(vec!["C:\\secret\\plans.txt".into()], true, cx)
    });
    assert!(matches!(h.notices().last(), Some(Notice::Closed { path, .. }) if path == "C:\\secret"));
    assert_eq!(h.disk.clipboard(), None);
}

#[gpui_kit::test]
fn pasting_with_no_files_makes_a_file_of_the_picture_or_text_and_starts_renaming_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\c"));
    // Nothing on the clipboard at all.
    h.task(cx, |e, cx| e.paste(None, cx));
    assert_eq!(h.said(), [OpNotice::NothingToPaste]);

    h.disk.set_paste_file(Some("Pasted text 2026-11-10.txt"));
    h.task(cx, |e, cx| e.paste(None, cx));
    assert_eq!(h.disk.names("C:\\c"), ["Pasted text 2026-11-10.txt"]);
    assert_eq!(h.selected(cx), ["Pasted text 2026-11-10.txt"]);
    assert_eq!(h.renaming(cx).as_deref(), Some("C:\\c\\Pasted text 2026-11-10.txt"));
    assert_eq!(
        h.next_undo(cx).map(|(_, l)| l).as_deref(),
        Some("paste of “Pasted text 2026-11-10.txt”")
    );

    // Into a folder that isn't the one shown, it is said instead.
    h.disk.set_paste_file(Some("Pasted image.png"));
    h.task(cx, |e, cx| e.paste(Some("C:\\b"), cx));
    assert!(h.disk.exists("C:\\b\\Pasted image.png"));
    assert!(h.said().contains(&OpNotice::PastedInto {
        made: "C:\\b\\Pasted image.png".into(),
        dest: "C:\\b".into()
    }));

    // A failing clipboard is told as "couldn't read".
    h.disk.fail_clipboard("The clipboard is locked");
    h.task(cx, |e, cx| e.paste(None, cx));
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::ReadClipboard,
        message: "The clipboard is locked".into()
    }));
}

#[gpui_kit::test]
fn pasting_where_nothing_can_be_pasted_does_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.set_clipboard(&["C:\\a\\one.txt"], false);
    // This PC and the Recycle Bin aren't folders to paste into.
    assert!(h.go(cx, ""));
    h.task(cx, |e, cx| e.paste(None, cx));
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    h.task(cx, |e, cx| e.paste(None, cx));
    assert!(h.disk.ops().is_empty());
    // A remote connection is read-only.
    h.task(cx, |e, cx| e.paste(Some("remote:abc\\x"), cx));
    assert!(matches!(h.notices().last(), Some(Notice::ReadOnly { .. })));
    // A folder Windows keeps closed says so.
    h.disk.folder("C:\\secret", vec![]);
    h.disk.deny("C:\\secret");
    assert!(h.go(cx, "C:\\secret"));
    h.task(cx, |e, cx| e.paste(None, cx));
    assert!(matches!(h.notices().last(), Some(Notice::Closed { .. })));
    assert!(h.disk.ops().is_empty());
}

#[gpui_kit::test]
fn a_failed_copy_is_told_and_changes_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    h.disk.set_clipboard(&["C:\\a\\one.txt", "C:\\a\\two.txt"], false);
    h.disk.fail_next("Access is denied");
    h.task(cx, |e, cx| e.paste(None, cx));
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::Paste,
        message: "Access is denied".into()
    }));
    assert_eq!(h.disk.names("C:\\b"), ["b1.txt"]);
    assert_eq!(h.next_undo(cx), None);

    // The person cancelling in Windows' dialog is not an error: nothing is said, nothing is selected.
    h.disk.cancel_next();
    h.task(cx, |e, cx| e.paste(None, cx));
    assert_eq!(h.said().len(), 1);
    assert_eq!(h.selected(cx), Vec::<String>::new());
    assert_eq!(h.next_undo(cx), None);
}

// ---- The copy queue -------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn paste_queued_adds_the_clipboard_to_the_copy_queue_and_a_cut_empties_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    // Nothing on the clipboard.
    h.task(cx, |e, cx| e.paste_queued(None, cx));
    assert_eq!(h.said(), [OpNotice::NoFilesOnClipboard]);

    h.disk.set_clipboard(&["C:\\a\\one.txt"], false);
    h.task(cx, |e, cx| e.paste_queued(None, cx));
    assert_eq!(
        h.disk.queued(),
        [(
            QueueKind::Copy,
            vec!["C:\\a\\one.txt".to_string()],
            "C:\\b".to_string(),
            true
        )]
    );
    assert!(h.events().contains(&ExplorerEvent::QueueOpened));
    assert!(h.disk.ops().is_empty(), "the queue does it, not Windows' dialog");
    assert!(h.disk.clipboard().is_some(), "a copy leaves the clipboard");

    h.disk.set_clipboard(&["C:\\a\\two.txt"], true);
    h.task(cx, |e, cx| e.paste_queued(Some("C:\\c"), cx));
    assert_eq!(h.disk.queued()[1].0, QueueKind::Move);
    assert_eq!(h.disk.queued()[1].2, "C:\\c");
    assert_eq!(h.disk.clipboard(), None);

    // Settings → verify copies is passed on.
    cx.update(|cx| crate::state::update(cx, |s| s.verify_copies = false));
    h.disk.set_clipboard(&["C:\\a\\one.txt"], false);
    h.task(cx, |e, cx| e.paste_queued(None, cx));
    assert!(!h.disk.queued()[2].3);

    // A queue that refuses is told.
    h.disk.fail_queue("“C:\\a” can't go into itself");
    h.task(cx, |e, cx| e.paste_queued(None, cx));
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::Queue(QueueKind::Copy),
        message: "“C:\\a” can't go into itself".into()
    }));
    // Remote connections are read-only.
    h.task(cx, |e, cx| e.paste_queued(Some("remote:abc\\x"), cx));
    assert!(matches!(h.notices().last(), Some(Notice::ReadOnly { .. })));
}

#[gpui_kit::test]
fn with_the_copy_queue_on_settings_send_pastes_and_drops_to_the_queue(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    cx.update(|cx| crate::state::update(cx, |s| s.copy_queue = true));
    assert!(h.go(cx, "C:\\b"));
    h.disk.set_clipboard(&["C:\\a\\one.txt"], true);
    h.task(cx, |e, cx| e.paste(None, cx));
    assert_eq!(h.disk.queued().len(), 1);
    assert_eq!(h.disk.queued()[0].0, QueueKind::Move);
    assert!(h.disk.ops().is_empty());
    assert!(h.disk.exists("C:\\a\\one.txt"), "the queue hasn't run");
    assert_eq!(
        h.disk.clipboard(),
        None,
        "a cut paste forgets the clipboard once queued"
    );

    h.task(cx, |e, cx| {
        e.drop_files(vec!["C:\\a\\two.txt".into()], "C:\\b", false, cx)
    });
    assert_eq!(h.disk.queued().len(), 2);
    assert_eq!(h.disk.queued()[1].0, QueueKind::Copy);

    // Renames and deletes never go to the queue.
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\b\\b1.txt".into()], false, None, cx)
    });
    assert_eq!(h.disk.queued().len(), 2);
    assert!(matches!(&h.disk.ops()[..], [FileOp::Delete { .. }]));
}

#[gpui_kit::test]
fn a_finished_queue_job_offers_undo_reports_errors_and_reads_the_folder_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    let job = |state: &str| {
        serde_json::json!([{
            "id": 7, "kind": "copy", "sources": ["C:\\a\\one.txt", "C:\\a\\two.txt"], "dest": "C:\\b",
            "state": state, "errors": [{"path": "C:\\a\\two.txt", "error": "Access is denied"}],
            "made": [{"from": "C:\\a\\one.txt", "to": "C:\\b\\one.txt"}]
        }])
    };
    // Running: nothing yet.
    h.queue_says(cx, job("copying"));
    assert!(h.said().is_empty());
    // It makes its copy, and finishes.
    h.disk.folder(
        "C:\\b",
        vec![file("C:\\b", "b1.txt", 1, 1), file("C:\\b", "one.txt", 100, 10)],
    );
    h.queue_says(cx, job("done"));
    assert!(h.said().iter().any(|n| matches!(
        n,
        OpNotice::Copied { count: 1, dest, undo: Some(_) } if dest == "C:\\b"
    )));
    assert!(h.said().contains(&OpNotice::QueueErrors {
        kind: QueueKind::Copy,
        count: 1,
        detail: "two.txt: Access is denied".into()
    }));
    assert_eq!(
        h.names(cx),
        ["b1.txt", "one.txt"],
        "the folder it went into is read again"
    );
    assert_eq!(h.next_undo(cx).map(|(_, l)| l).as_deref(), Some("copy of 1 item"));

    // Told again (the queue repeats every job in each event): nothing new happens.
    let before = h.said().len();
    h.queue_says(cx, job("done"));
    assert_eq!(h.said().len(), before);

    // Undo recycles the copy.
    h.task(cx, |e, cx| e.undo(None, cx));
    assert_eq!(h.disk.bin_names(), ["one.txt"]);
    assert_eq!(h.names(cx), ["b1.txt"]);

    // A finished move can be taken back.
    let moved = serde_json::json!([{
        "id": 8, "kind": "move", "sources": ["C:\\a\\two.txt"], "dest": "C:\\b", "state": "copying",
        "made": []
    }]);
    h.queue_says(cx, moved);
    h.disk.folder(
        "C:\\b",
        vec![file("C:\\b", "b1.txt", 1, 1), file("C:\\b", "two.txt", 200, 20)],
    );
    h.disk
        .folder("C:\\a", vec![folder("C:\\a", "sub"), file("C:\\a", "one.txt", 100, 10)]);
    h.queue_says(
        cx,
        serde_json::json!([{
            "id": 8, "kind": "move", "sources": ["C:\\a\\two.txt"], "dest": "C:\\b", "state": "done",
            "made": [{"from": "C:\\a\\two.txt", "to": "C:\\b\\two.txt"}]
        }]),
    );
    assert!(h.said().iter().any(|n| matches!(n, OpNotice::Moved { count: 1, .. })));
    assert_eq!(h.next_undo(cx).map(|(_, l)| l).as_deref(), Some("move of 1 item"));
    h.task(cx, |e, cx| e.undo(None, cx));
    assert!(matches!(h.disk.ops().last(), Some(FileOp::MoveBack { items })
        if items.len() == 1 && items[0].folder == "C:\\a" && items[0].name == "two.txt"));
}

// ---- Delete ---------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn deleting_recycles_selects_the_next_item_and_undo_restores(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into()], false, Some("C:\\a\\two.txt".into()), cx)
    });
    assert!(!h.disk.exists("C:\\a\\one.txt"));
    assert_eq!(h.disk.bin_names(), ["one.txt"]);
    assert_eq!(h.names(cx), ["sub", "two.txt", "تقرير.pdf"]);
    assert_eq!(h.selected(cx), ["two.txt"], "the next item is selected, like Explorer");
    assert!(matches!(
        &h.disk.ops()[..],
        [FileOp::Delete {
            permanent: false,
            confirm: false,
            ..
        }]
    ));
    assert!(h
        .said()
        .iter()
        .any(|n| matches!(n, OpNotice::Recycled { count: 1, .. })));
    assert_eq!(h.next_undo(cx).map(|(_, l)| l).as_deref(), Some("deletion of 1 item"));

    h.task(cx, |e, cx| e.undo(None, cx));
    assert!(h.disk.exists("C:\\a\\one.txt"));
    assert_eq!(h.disk.bin_names(), Vec::<String>::new());
    assert_eq!(h.names(cx), ["sub", "one.txt", "two.txt", "تقرير.pdf"]);
}

#[gpui_kit::test]
fn deleting_for_good_asks_windows_when_settings_say_and_offers_no_undo(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    cx.update(|cx| crate::state::update(cx, |s| s.confirm_delete = true));
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\two.txt"), cx));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\two.txt".into()], true, None, cx)
    });
    assert!(!h.disk.exists("C:\\a\\two.txt"));
    assert_eq!(h.disk.bin_names(), Vec::<String>::new());
    assert!(matches!(
        &h.disk.ops()[..],
        [FileOp::Delete {
            permanent: true,
            confirm: true,
            ..
        }]
    ));
    assert_eq!(
        h.selected(cx),
        Vec::<String>::new(),
        "nothing is selected when there is no next item"
    );
    assert_eq!(h.next_undo(cx), None);
    assert!(!h.said().iter().any(|n| matches!(n, OpNotice::Recycled { .. })));
}

#[gpui_kit::test]
fn a_cancelled_or_failed_delete_leaves_the_selection_and_says_what_it_must(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    // The person said no in Windows' own question.
    h.disk.cancel_next();
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into()], false, Some("C:\\a\\two.txt".into()), cx)
    });
    assert!(h.disk.exists("C:\\a\\one.txt"));
    assert_eq!(h.selected(cx), ["one.txt"]);
    assert!(h.said().is_empty());
    // A failure.
    h.disk.fail_next("The file is in use");
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into()], false, None, cx)
    });
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::Delete,
        message: "The file is in use".into()
    }));
    // Nothing to delete is nothing done.
    h.task(cx, |e, cx| e.delete_paths(vec![], false, None, cx));
    assert_eq!(h.disk.ops().len(), 2);
}

#[gpui_kit::test]
fn a_closed_folder_cant_be_changed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.folder("C:\\secret", vec![file("C:\\secret", "plans.txt", 5, 5)]);
    h.disk.deny("C:\\secret");
    assert!(h.go(cx, "C:\\secret"));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\secret\\plans.txt".into()], false, None, cx)
    });
    assert!(matches!(h.notices().last(), Some(Notice::Closed { .. })));
    h.task(cx, |e, cx| e.new_item(true, cx));
    h.run(cx, |e, cx| e.begin_rename(Some("C:\\secret\\plans.txt"), cx));
    assert_eq!(h.notices().len(), 3);
    assert!(h.disk.ops().is_empty());
    assert_eq!(h.renaming(cx), None);
}

// ---- Drops ----------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn dropping_files_on_the_open_folder_copies_or_moves_and_selects_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    h.task(cx, |e, cx| {
        e.drop_files(vec!["C:\\a\\one.txt".into()], "C:\\b", false, cx)
    });
    assert!(h.disk.exists("C:\\b\\one.txt") && h.disk.exists("C:\\a\\one.txt"));
    assert_eq!(h.selected(cx), ["one.txt"]);
    assert_eq!(h.names(cx), ["b1.txt", "one.txt"]);

    h.task(cx, |e, cx| {
        e.drop_files(vec!["C:\\a\\تقرير.pdf".into()], "C:\\b", true, cx)
    });
    assert!(h.disk.exists("C:\\b\\تقرير.pdf") && !h.disk.exists("C:\\a\\تقرير.pdf"));
    assert!(h.said().iter().any(|n| matches!(n, OpNotice::Moved { count: 1, .. })));
    assert_eq!(h.selected(cx), ["تقرير.pdf"]);
}

#[gpui_kit::test]
fn a_drop_that_would_do_nothing_does_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    // Into the folder they are already in, onto themselves, into themselves.
    h.task(cx, |e, cx| {
        e.drop_files(vec!["C:\\a\\one.txt".into()], "C:\\a", false, cx)
    });
    h.task(cx, |e, cx| e.drop_files(vec!["C:\\a".into()], "C:\\a", true, cx));
    h.task(cx, |e, cx| e.drop_files(vec!["C:\\a".into()], "C:\\a\\sub", true, cx));
    assert!(h.disk.ops().is_empty());
    assert!(h.said().is_empty());
    // A remote connection is read-only.
    h.task(cx, |e, cx| {
        e.drop_files(vec!["C:\\a\\one.txt".into()], "remote:abc\\x", false, cx)
    });
    assert!(matches!(h.notices().last(), Some(Notice::ReadOnly { .. })));
    assert!(h.disk.ops().is_empty());
}

#[gpui_kit::test]
fn a_drop_on_another_folder_says_it_copied_and_a_drop_on_the_recycle_bin_recycles(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    // The folder isn't on screen: it can only be said.
    h.task(cx, |e, cx| {
        e.drop_files(vec!["C:\\a\\one.txt".into()], "C:\\c", false, cx)
    });
    assert!(h.disk.exists("C:\\c\\one.txt"));
    let copied: Vec<_> = h
        .said()
        .into_iter()
        .filter(|n| matches!(n, OpNotice::Copied { dest, .. } if dest == "C:\\c"))
        .collect();
    assert_eq!(
        copied.len(),
        2,
        "Windows' copy offers Undo, and the drop says it too, as the web app did"
    );
    assert!(matches!(copied[1], OpNotice::Copied { undo: None, .. }));
    assert_eq!(h.names(cx), ["b1.txt"], "this tab isn't showing it");

    // On the Recycle Bin: recycled, with Windows asking where Settings say.
    h.task(cx, |e, cx| {
        e.drop_files(
            vec!["C:\\a\\two.txt".into()],
            glint_model::paths::RECYCLE_BIN,
            false,
            cx,
        )
    });
    assert!(!h.disk.exists("C:\\a\\two.txt"));
    assert_eq!(h.disk.bin_names(), ["two.txt"]);
    assert!(h
        .said()
        .iter()
        .any(|n| matches!(n, OpNotice::Recycled { count: 1, .. })));
    // A failure there is told by its count.
    h.disk.fail_next("Access is denied");
    h.task(cx, |e, cx| {
        e.drop_files(vec!["C:\\a\\sub".into()], glint_model::paths::RECYCLE_BIN, false, cx)
    });
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::DeleteItems { count: 1 },
        message: "Access is denied".into()
    }));
}

// ---- Dual panes -------------------------------------------------------------------------------------------------------

/// Two panes: the left on `C:\a`, the right on `C:\b`, the keyboard in the left.
fn two_panes(h: &Harness, cx: &mut TestAppContext) {
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.toggle_dual(cx));
    assert!(h.go(cx, "C:\\b"));
    h.run(cx, |e, cx| e.focus_pane(0, cx));
}

#[gpui_kit::test]
fn copying_to_the_other_pane_copies_the_selection_and_shows_it_there(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_panes(&h, cx);
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    h.task(cx, |e, cx| e.copy_to_other_pane(false, None, cx));
    assert!(h.disk.exists("C:\\b\\one.txt") && h.disk.exists("C:\\a\\one.txt"));
    h.read(cx, |e| {
        let other = e.pane_tab(1).expect("right pane");
        assert!(other.rows.iter().any(|r| r.name == "one.txt"));
        assert!(other.selection.contains("C:\\b\\one.txt"));
        assert_eq!(e.active_pane(), 0, "the keyboard stays where it was");
    });

    // Moving takes the item out of the folder on the left.
    h.task(cx, |e, cx| {
        e.copy_to_other_pane(true, Some(vec!["C:\\a\\two.txt".into()]), cx)
    });
    assert!(h.disk.exists("C:\\b\\two.txt") && !h.disk.exists("C:\\a\\two.txt"));
    assert_eq!(h.names(cx), ["sub", "one.txt", "تقرير.pdf"]);
    // An item already in that folder isn't copied onto itself.
    let before = h.disk.ops().len();
    h.task(cx, |e, cx| {
        e.copy_to_other_pane(false, Some(vec!["C:\\b\\b1.txt".into()]), cx)
    });
    assert_eq!(h.disk.ops().len(), before);
}

#[gpui_kit::test]
fn copying_to_the_other_pane_says_why_when_there_is_no_folder_there(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    // One pane.
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    h.task(cx, |e, cx| e.copy_to_other_pane(false, None, cx));
    assert_eq!(h.said(), [OpNotice::OtherPane(OtherPane::NoFolder)]);

    // The other pane is on This PC, which has no folder to copy into either.
    h.run(cx, |e, cx| e.toggle_dual(cx));
    assert!(h.go(cx, ""));
    h.run(cx, |e, cx| e.focus_pane(0, cx));
    h.task(cx, |e, cx| {
        e.copy_to_other_pane(false, Some(vec!["C:\\a\\one.txt".into()]), cx)
    });
    assert_eq!(h.said().len(), 2);

    // A folder Windows keeps closed.
    h.disk.folder("C:\\secret", vec![]);
    h.disk.deny("C:\\secret");
    h.run(cx, |e, cx| e.focus_pane(1, cx));
    assert!(h.go(cx, "C:\\secret"));
    h.run(cx, |e, cx| e.focus_pane(0, cx));
    h.task(cx, |e, cx| {
        e.copy_to_other_pane(false, Some(vec!["C:\\a\\one.txt".into()]), cx)
    });
    assert_eq!(h.said().last(), Some(&OpNotice::OtherPane(OtherPane::Closed)));
    assert!(h.disk.ops().is_empty());
}

#[gpui_kit::test]
fn copying_from_the_recycle_bin_to_the_other_pane_says_to_restore_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_panes(&h, cx);
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    h.task(cx, |e, cx| {
        e.copy_to_other_pane(false, Some(vec!["C:\\$Recycle.Bin\\$R1x".into()]), cx)
    });
    assert_eq!(h.notices(), [Notice::InRecycleBin]);
    assert!(h.disk.ops().is_empty());
}

// ---- Rename -----------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn f2_renames_the_focused_item_and_enter_commits_the_new_name(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(true);
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\تقرير.pdf"), cx));
    h.run(cx, |e, cx| e.begin_rename(None, cx));
    assert_eq!(h.renaming(cx).as_deref(), Some("C:\\a\\تقرير.pdf"));

    let renamed = h.task(cx, |e, cx| e.commit_rename("تقرير نهائي.pdf. ", cx));
    assert!(renamed);
    assert_eq!(h.renaming(cx), None);
    assert!(
        h.disk.exists("C:\\a\\تقرير نهائي.pdf"),
        "trailing dots and spaces are dropped"
    );
    assert!(!h.disk.exists("C:\\a\\تقرير.pdf"));
    assert_eq!(h.selected(cx), ["تقرير نهائي.pdf"]);
    h.read(cx, |e| {
        let tab = e.active_tab().expect("tab");
        assert_eq!(tab.selection.focus(), Some("C:\\a\\تقرير نهائي.pdf"));
    });
    assert!(h.said().iter().any(|n| matches!(
        n,
        OpNotice::Renamed { to, .. } if to == "C:\\a\\تقرير نهائي.pdf"
    )));
    assert_eq!(
        h.next_undo(cx).map(|(_, l)| l).as_deref(),
        Some("rename to “تقرير نهائي.pdf”")
    );

    // Undo gives the old name back.
    h.task(cx, |e, cx| e.undo(None, cx));
    assert!(h.disk.exists("C:\\a\\تقرير.pdf"));
    assert!(matches!(h.disk.ops().last(), Some(FileOp::Rename { name, .. }) if name == "تقرير.pdf"));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn renaming_to_nothing_the_same_name_or_a_bad_one_renames_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    for typed in ["", "  ", "one.txt", "one.txt . ", "a/b", "what?"] {
        h.run(cx, |e, cx| e.begin_rename(Some("C:\\a\\one.txt"), cx));
        assert_eq!(h.renaming(cx).as_deref(), Some("C:\\a\\one.txt"));
        let renamed = h.task(cx, |e, cx| e.commit_rename(typed, cx));
        assert!(!renamed, "{typed:?}");
        assert_eq!(h.renaming(cx), None, "the box closes whatever comes of it");
    }
    assert_eq!(h.said(), [OpNotice::InvalidName, OpNotice::InvalidName]);
    assert!(h.disk.ops().is_empty());
    // A name that's taken is Windows' to refuse, and is told.
    h.run(cx, |e, cx| e.begin_rename(Some("C:\\a\\one.txt"), cx));
    assert!(!h.task(cx, |e, cx| e.commit_rename("two.txt", cx)));
    assert!(h.said().iter().any(|n| matches!(
        n,
        OpNotice::Failed { what: Failing::Rename { name }, .. } if name == "one.txt"
    )));
    assert!(h.disk.exists("C:\\a\\one.txt"));
    // With no box open there is nothing to commit.
    assert!(!h.task(cx, |e, cx| e.commit_rename("x.txt", cx)));
}

#[gpui_kit::test]
fn escape_cancels_renaming_and_f2_on_several_items_opens_the_batch_rename(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    h.run(cx, |e, cx| e.begin_rename(None, cx));
    assert!(h.renaming(cx).is_some());
    h.run(cx, |e, cx| e.cancel_rename(cx));
    assert_eq!(h.renaming(cx), None);
    assert!(h.disk.ops().is_empty());

    // Several selected: the batch rename, not a box.
    h.run(cx, |e, cx| e.toggle_selected("C:\\a\\two.txt", cx));
    h.run(cx, |e, cx| e.begin_rename(None, cx));
    assert_eq!(h.renaming(cx), None);
    assert!(h.events().contains(&ExplorerEvent::BatchRename(vec![
        "C:\\a\\one.txt".into(),
        "C:\\a\\two.txt".into()
    ])));
    // Naming the item asks for that one only, and selects only it.
    h.run(cx, |e, cx| e.begin_rename(Some("C:\\a\\two.txt"), cx));
    assert_eq!(h.renaming(cx).as_deref(), Some("C:\\a\\two.txt"));
    assert_eq!(h.selected(cx), ["two.txt"]);
}

#[gpui_kit::test]
fn the_recycle_bin_cant_be_renamed_in(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into()], false, None, cx)
    });
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    let bin_path = h.read(cx, |e| e.active_tab().expect("tab").entries()[0].path.clone());
    h.run(cx, |e, cx| e.begin_rename(Some(&bin_path), cx));
    assert_eq!(h.renaming(cx), None);
    assert!(h.notices().contains(&Notice::InRecycleBin));
}

// ---- New items --------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_new_folder_is_made_selected_and_named_in_place(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\c"));
    h.task(cx, |e, cx| e.new_item(true, cx));
    assert_eq!(h.disk.names("C:\\c"), ["New folder"]);
    assert_eq!(h.selected(cx), ["New folder"]);
    assert_eq!(h.renaming(cx).as_deref(), Some("C:\\c\\New folder"));
    assert_eq!(
        h.next_undo(cx).map(|(_, l)| l).as_deref(),
        Some("new folder “New folder”")
    );
    // Another one is numbered like Explorer does.
    h.run(cx, |e, cx| e.cancel_rename(cx));
    h.task(cx, |e, cx| e.new_item(true, cx));
    assert_eq!(h.disk.names("C:\\c"), ["New folder", "New folder (2)"]);
    assert_eq!(h.renaming(cx).as_deref(), Some("C:\\c\\New folder (2)"));
    // And a text file.
    h.task(cx, |e, cx| e.new_item(false, cx));
    assert!(h.disk.exists("C:\\c\\New Text Document.txt"));
    assert_eq!(
        h.next_undo(cx).map(|(_, l)| l).as_deref(),
        Some("new file “New Text Document.txt”")
    );
    // Undo recycles the newest.
    h.task(cx, |e, cx| e.undo(None, cx));
    assert!(!h.disk.exists("C:\\c\\New Text Document.txt"));
    assert_eq!(h.disk.bin_names(), ["New Text Document.txt"]);
}

#[gpui_kit::test]
fn a_new_item_ends_a_search_shown_in_the_tab_and_a_failure_is_told(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\c"));
    h.run(cx, |e, cx| e.set_filter("zzz", None, cx));
    h.task(cx, |e, cx| e.new_item(true, cx));
    h.read(cx, |e| assert_eq!(e.active_tab().expect("tab").filter, ""));
    assert_eq!(h.names(cx), ["New folder"]);

    h.disk.fail_next("The disk is full");
    h.task(cx, |e, cx| e.new_item(false, cx));
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::Create { dir: false },
        message: "The disk is full".into()
    }));
    // This PC isn't a folder to make things in.
    assert!(h.go(cx, ""));
    let before = h.said().len();
    h.task(cx, |e, cx| e.new_item(true, cx));
    assert_eq!(h.said().len(), before);
}

fn word() -> NewKind {
    NewKind {
        ext: ".docx".into(),
        name: "Microsoft Word Document".into(),
        icon: None,
        by_app: false,
    }
}

#[gpui_kit::test]
fn the_new_menu_makes_files_of_its_kinds_and_an_apps_own_file_is_not_renamed_at_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.set_new_kinds(vec![
        word(),
        NewKind {
            ext: ".gdoc".into(),
            name: "Google Docs".into(),
            icon: None,
            by_app: true,
        },
    ]);
    assert!(h.go(cx, "C:\\c"));
    h.task(cx, |e, cx| e.new_of_kind(word(), cx));
    assert_eq!(h.disk.names("C:\\c"), ["New Microsoft Word Document.docx"]);
    assert_eq!(
        h.renaming(cx).as_deref(),
        Some("C:\\c\\New Microsoft Word Document.docx")
    );
    assert_eq!(
        h.next_undo(cx).map(|(_, l)| l).as_deref(),
        Some("new file “New Microsoft Word Document.docx”")
    );

    // A kind whose app makes it: selected, not renamed.
    h.run(cx, |e, cx| e.cancel_rename(cx));
    let app_kind = NewKind {
        ext: ".gdoc".into(),
        name: "Google Docs".into(),
        icon: None,
        by_app: true,
    };
    h.task(cx, |e, cx| e.new_of_kind(app_kind.clone(), cx));
    assert!(h.disk.exists("C:\\c\\New Google Docs.gdoc"));
    assert_eq!(h.renaming(cx), None);

    // Its app hasn't made it yet: told so, and nothing to undo or select.
    h.disk.set_new_pending(true);
    let undo_before = h.next_undo(cx);
    h.task(cx, |e, cx| e.new_of_kind(word(), cx));
    assert!(h.said().contains(&OpNotice::AppIsMakingIt {
        kind: "Microsoft Word Document".into()
    }));
    assert_eq!(h.next_undo(cx), undo_before);

    // A failure names the kind.
    h.disk.fail_next("nope");
    h.disk.set_new_pending(false);
    h.disk.set_new_kinds(vec![]);
    h.task(cx, |e, cx| e.new_of_kind(word(), cx));
    assert!(h.said().iter().any(|n| matches!(
        n,
        OpNotice::Failed { what: Failing::MakeNew { kind }, .. } if kind == "Microsoft Word Document"
    )));
}

#[gpui_kit::test]
fn the_new_menus_kinds_are_read_in_the_background_and_not_asked_again_for_30_seconds(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.set_new_kinds(vec![word()]);
    // The first look gets nothing yet and starts the read.
    let first = h.run(cx, |e, cx| e.new_kinds(cx));
    assert!(first.is_empty());
    let second = h.run(cx, |e, cx| e.new_kinds(cx));
    assert_eq!(&second[..], &[word()]);
    assert!(h.events().contains(&ExplorerEvent::NewKindsChanged));
    // A new app is installed; a look within 30 s doesn't see it.
    h.disk.set_new_kinds(vec![]);
    let third = h.run(cx, |e, cx| e.new_kinds(cx));
    assert_eq!(&third[..], &[word()]);
}

// ---- The Recycle Bin --------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn restore_puts_items_back_where_they_were_even_into_a_folder_made_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into(), "C:\\a\\sub".into()], false, None, cx)
    });
    assert_eq!(h.disk.bin_names(), ["sub", "one.txt"]);
    // The folder they came from is gone too.
    h.disk.remove("C:\\a");
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    let entries = h.read(cx, |e| e.active_tab().expect("tab").entries().to_vec());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].origin.as_deref(), Some("C:\\a"));
    h.task(cx, |e, cx| e.restore_recycled(&entries, cx));
    assert!(h.disk.exists("C:\\a\\one.txt"));
    assert!(h.disk.exists("C:\\a\\sub\\deep.txt"));
    assert_eq!(h.disk.bin_names(), Vec::<String>::new());
    assert_eq!(h.names(cx), Vec::<String>::new(), "the bin on screen is read again");
    assert!(h
        .said()
        .iter()
        .any(|n| matches!(n, OpNotice::Restored { count: 2, .. })));
    // Taking the restore back recycles them again.
    assert_eq!(h.next_undo(cx).map(|(_, l)| l).as_deref(), Some("restore of 2 items"));
    h.task(cx, |e, cx| e.undo(None, cx));
    assert!(!h.disk.exists("C:\\a\\one.txt"));
    assert_eq!(h.disk.bin_names(), ["sub", "one.txt"]);
    assert_eq!(h.names(cx).len(), 2);

    // Anything that isn't in the bin, or is without its origin, is left alone.
    let before = h.disk.ops().len();
    h.task(cx, |e, cx| e.restore_recycled(&[folder("C:\\", "a")], cx));
    assert_eq!(h.disk.ops().len(), before);
}

#[gpui_kit::test]
fn deleting_for_good_in_the_recycle_bin_asks_first_then_selects_the_next_item(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into(), "C:\\a\\two.txt".into()], false, None, cx)
    });
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    let entries = h.read(cx, |e| e.active_tab().expect("tab").entries().to_vec());
    let (first, second) = (entries[0].clone(), entries[1].clone());

    // Asking changes nothing; the dialog's yes does the work.
    h.run(cx, |e, cx| {
        e.purge_recycled(std::slice::from_ref(&first), Some(&second.path), cx)
    });
    assert_eq!(h.disk.bin_names().len(), 2);
    let ask = h
        .events()
        .into_iter()
        .find_map(|e| match e {
            ExplorerEvent::Ask(Ask::Purge(ask)) => Some(ask),
            _ => None,
        })
        .expect("the question was asked");
    assert_eq!(ask.names, ["one.txt"]);
    assert_eq!(purge_what(&ask), "“one.txt”");
    assert_eq!(ask.next.as_deref(), Some(second.path.as_str()));
    h.task(cx, |e, cx| e.confirm_purge(ask, cx));
    assert_eq!(h.disk.bin_names(), ["two.txt"]);
    assert_eq!(h.selected(cx).len(), 1);
    assert_eq!(h.names(cx), ["two.txt"]);
    assert!(matches!(h.disk.ops().last(), Some(FileOp::Purge { paths }) if paths == std::slice::from_ref(&first.path)));
    // Purging offers no Undo.
    assert_eq!(h.next_undo(cx).map(|(_, l)| l).as_deref(), Some("deletion of 2 items"));

    // Things that aren't in the bin aren't asked about.
    let asks = h.events().len();
    h.run(cx, |e, cx| e.purge_recycled(&[folder("C:\\", "a")], None, cx));
    assert_eq!(h.events().len(), asks);
}

#[gpui_kit::test]
fn emptying_the_recycle_bin_says_so_only_when_windows_emptied_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into()], false, None, cx)
    });
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    assert_eq!(h.names(cx), ["one.txt"]);

    // Windows asked and the answer was no.
    h.disk.set_recycle_answer(false);
    h.task(cx, |e, cx| e.empty_recycle_bin(cx));
    assert!(!h.said().contains(&OpNotice::EmptiedBin));
    assert_eq!(h.disk.bin_names(), ["one.txt"]);

    h.disk.set_recycle_answer(true);
    h.task(cx, |e, cx| e.empty_recycle_bin(cx));
    assert!(h.said().contains(&OpNotice::EmptiedBin));
    assert_eq!(h.disk.bin_names(), Vec::<String>::new());
    assert_eq!(h.names(cx), Vec::<String>::new(), "the bin on screen is read again");

    h.disk.fail_next("The Recycle Bin is in use");
    h.task(cx, |e, cx| e.empty_recycle_bin(cx));
    assert!(h.said().contains(&OpNotice::Failed {
        what: Failing::EmptyBin,
        message: "The Recycle Bin is in use".into()
    }));
}

// ---- Undo -------------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn undo_with_nothing_to_undo_says_so_and_a_step_can_be_undone_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\c"));
    h.task(cx, |e, cx| e.undo(None, cx));
    assert_eq!(h.said(), [OpNotice::NothingToUndo]);

    h.task(cx, |e, cx| e.new_item(true, cx));
    let id = h
        .last_undo()
        .or_else(|| h.next_undo(cx).map(|(id, _)| id))
        .expect("a step");
    h.task(cx, |e, cx| e.undo(Some(id), cx));
    assert_eq!(h.disk.names("C:\\c"), Vec::<String>::new());
    // The same step again: it's gone.
    h.task(cx, |e, cx| e.undo(Some(id), cx));
    assert_eq!(h.said().last(), Some(&OpNotice::NothingToUndo));
    assert!(h.events().contains(&ExplorerEvent::UndoChanged));
}

#[gpui_kit::test]
fn undo_takes_back_the_newest_step_first_and_remembers_only_the_newest_fifty(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let ids: Vec<UndoId> = (0..53)
        .map(|n| {
            h.run(cx, |e, cx| {
                e.remember_undo(format!("step {n}"), UndoAction::Recycle(vec![]), cx)
            })
        })
        .collect();
    assert_eq!(h.next_undo(cx), Some((ids[52], "step 52".to_string())));
    // The first three fell off.
    h.task(cx, |e, cx| e.undo(Some(ids[0]), cx));
    assert_eq!(h.said().last(), Some(&OpNotice::NothingToUndo));
    h.task(cx, |e, cx| e.undo(Some(ids[3]), cx));
    assert_eq!(h.said().last(), Some(&OpNotice::Undid { label: "step 3".into() }));
    // Default: the newest.
    h.task(cx, |e, cx| e.undo(None, cx));
    assert_eq!(
        h.said().last(),
        Some(&OpNotice::Undid {
            label: "step 52".into()
        })
    );
    assert_eq!(h.next_undo(cx), Some((ids[51], "step 51".to_string())));
}

#[gpui_kit::test]
fn a_failed_undo_is_told_and_the_step_is_not_kept(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    h.disk.set_clipboard(&["C:\\a\\one.txt"], false);
    h.task(cx, |e, cx| e.paste(None, cx));
    h.disk.fail_next("The file is in use");
    h.task(cx, |e, cx| e.undo(None, cx));
    assert!(h.said().contains(&OpNotice::UndoFailed {
        label: "copy of 1 item".into(),
        message: "The file is in use".into()
    }));
    assert_eq!(h.next_undo(cx), None);
    assert!(h.disk.exists("C:\\b\\one.txt"));
}

// ---- Slow work ----------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn slow_work_shows_its_toast_after_half_a_second_and_takes_it_away(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    let gate = h.disk.hold_ops();
    h.disk.set_clipboard(&["C:\\a\\one.txt", "C:\\a\\two.txt"], false);
    let pasting = h.explorer.update(cx, |e, cx| e.paste(None, cx));
    cx.run_until_parked();
    assert_eq!(h.read(cx, |e| e.busy_count()), 1);
    // A quick one would be done by now; this one is not.
    let busy = |h: &Harness| -> Vec<BusyEvent> {
        h.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Busy(b) => Some(b),
                _ => None,
            })
            .collect()
    };
    assert!(busy(&h).is_empty());
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    assert!(matches!(
        &busy(&h)[..],
        [BusyEvent::Shown {
            what: Working::Copying(2),
            ..
        }]
    ));
    gate.close();
    cx.foreground_executor().block_test(pasting);
    cx.run_until_parked();
    assert_eq!(h.read(cx, |e| e.busy_count()), 0);
    assert!(matches!(
        &busy(&h)[..],
        [
            BusyEvent::Shown { .. },
            BusyEvent::Ended {
                was_shown: true,
                idle: true,
                ..
            }
        ]
    ));
}

#[gpui_kit::test]
fn quick_work_never_shows_a_toast(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.delete_paths(vec!["C:\\a\\one.txt".into()], false, None, cx)
    });
    cx.executor().advance_clock(Duration::from_millis(900));
    cx.run_until_parked();
    let events = h.events();
    assert!(!events
        .iter()
        .any(|e| matches!(e, ExplorerEvent::Busy(BusyEvent::Shown { .. }))));
    assert!(events.iter().any(|e| matches!(
        e,
        ExplorerEvent::Busy(BusyEvent::Ended {
            was_shown: false,
            idle: true,
            ..
        })
    )));
}

// ---- What the views ask -----------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_focus_moves_without_touching_the_selection(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    h.run(cx, |e, cx| e.toggle_selected("C:\\a\\two.txt", cx));
    // Ctrl+arrow: focus only, the anchor stays.
    h.run(cx, |e, cx| e.set_focus(Some("C:\\a\\تقرير.pdf"), None, cx));
    h.read(cx, |e| {
        let sel = &e.active_tab().expect("tab").selection;
        assert_eq!(sel.len(), 2, "the selection is kept");
        assert_eq!(sel.focus(), Some("C:\\a\\تقرير.pdf"));
        assert_eq!(sel.anchor(), Some("C:\\a\\two.txt"));
    });
    // A press on one item of a multi-selection: focus and anchor there, the others stay selected.
    h.run(cx, |e, cx| {
        e.set_focus(Some("C:\\a\\one.txt"), Some("C:\\a\\one.txt"), cx)
    });
    h.read(cx, |e| {
        let sel = &e.active_tab().expect("tab").selection;
        assert_eq!(
            (sel.focus(), sel.anchor(), sel.len()),
            (Some("C:\\a\\one.txt"), Some("C:\\a\\one.txt"), 2)
        );
    });
    // Nothing changed, nothing raised.
    let events = h.events().len();
    h.run(cx, |e, cx| e.set_focus(Some("C:\\a\\one.txt"), None, cx));
    assert_eq!(h.events().len(), events);
}

#[gpui_kit::test]
fn the_marquee_selects_exactly_what_it_covers(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| {
        e.set_selection(
            ["C:\\a\\one.txt", "C:\\a\\two.txt"],
            Some("C:\\a\\one.txt"),
            Some("C:\\a\\two.txt"),
            cx,
        )
    });
    assert_eq!(h.selected(cx), ["one.txt", "two.txt"]);
    h.read(cx, |e| {
        let sel = &e.active_tab().expect("tab").selection;
        assert_eq!(
            (sel.anchor(), sel.focus()),
            (Some("C:\\a\\one.txt"), Some("C:\\a\\two.txt"))
        );
    });
    // The same rectangle on the next pointer move raises nothing.
    let events = h.events().len();
    h.run(cx, |e, cx| {
        e.set_selection(
            vec!["C:\\a\\two.txt".to_string(), "C:\\a\\one.txt".to_string()],
            Some("C:\\a\\one.txt"),
            Some("C:\\a\\two.txt"),
            cx,
        )
    });
    assert_eq!(h.events().len(), events);
    // Shrinking it, and clearing it.
    h.run(cx, |e, cx| {
        e.set_selection(["C:\\a\\one.txt"], Some("C:\\a\\one.txt"), Some("C:\\a\\one.txt"), cx)
    });
    assert_eq!(h.selected(cx), ["one.txt"]);
    h.run(cx, |e, cx| e.set_selection(Vec::<String>::new(), None, None, cx));
    assert_eq!(h.selected(cx), Vec::<String>::new());
    assert!(h
        .events()
        .iter()
        .any(|e| matches!(e, ExplorerEvent::SelectionChanged(_))));
}

#[gpui_kit::test]
fn columns_are_a_folders_own_with_per_folder_views_and_everyones_without(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let columns = vec![ColumnId::Size];
    h.run(cx, |e, cx| e.set_columns(columns.clone(), cx));
    h.read(cx, |e| {
        assert_eq!(e.active_tab().expect("tab").columns, columns);
        assert!(e.memory().folder_view("C:\\a").is_some_and(|v| v.columns.is_some()));
    });
    assert!(h.events().contains(&ExplorerEvent::MemoryChanged));
    // Another folder keeps the others' columns.
    assert!(h.go(cx, "C:\\b"));
    h.read(cx, |e| assert_ne!(e.active_tab().expect("tab").columns, columns));
    // Back to "like other folders".
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.reset_folder_view("C:\\a", cx));
    h.read(cx, |e| {
        assert!(e.memory().folder_view("C:\\a").is_none());
        assert_ne!(e.active_tab().expect("tab").columns, columns);
    });

    // Without per-folder views the choice is Settings'.
    cx.update(|cx| crate::state::update(cx, |s| s.per_folder_views = false));
    h.run(cx, |e, cx| e.set_columns(columns.clone(), cx));
    assert_eq!(cx.update(|cx| crate::state::settings(cx).list_columns.clone()), columns);
    assert!(h.go(cx, "C:\\b"));
    h.read(cx, |e| assert_eq!(e.active_tab().expect("tab").columns, columns));
    assert!(h.read(cx, |e| e.memory().folder_view("C:\\b").is_none()));
}

#[gpui_kit::test]
fn the_address_bar_tells_the_explorer_when_it_is_being_typed_in_and_a_folder_change_ends_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.set_editing_address(true, cx));
    assert!(h.read(cx, |e| e.editing_address));
    assert!(h.go(cx, "C:\\b"));
    assert!(!h.read(cx, |e| e.editing_address), "opening another folder ends it");
    h.run(cx, |e, cx| e.set_editing_address(true, cx));
    h.run(cx, |e, cx| e.set_editing_address(false, cx));
    assert!(!h.read(cx, |e| e.editing_address));
}

#[gpui_kit::test]
fn ejecting_goes_to_the_source_and_reports_how_it_went(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let result = h.task(cx, |e, cx| e.eject("E:\\", cx));
    assert_eq!(result, Ok(()));
    assert_eq!(h.disk.ejected(), ["E:\\"]);
}

#[gpui_kit::test]
fn a_file_operation_called_directly_does_what_the_operations_above_do(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    // Sync's copies, as compare panes ask for them.
    let done = h.task(cx, |e, cx| {
        e.file_op(
            FileOp::CopyInto {
                items: vec![MoveItem {
                    path: path_of("C:\\a", "one.txt"),
                    folder: "C:\\b".into(),
                    name: "synced.txt".into(),
                }],
            },
            cx,
        )
    });
    assert_eq!(done, Ok(true));
    assert!(h.disk.exists("C:\\b\\synced.txt") && h.disk.exists("C:\\a\\one.txt"));
    assert!(h.said().iter().any(|n| matches!(n, OpNotice::Synced { count: 1, .. })));
    assert_eq!(h.next_undo(cx).map(|(_, l)| l).as_deref(), Some("sync of 1 item"));
    // A failure comes back to the caller.
    h.disk.fail_next("Nope");
    let failed = h.task(cx, |e, cx| e.file_op(FileOp::Purge { paths: vec![] }, cx));
    assert_eq!(failed, Err("Nope".to_string()));
}
