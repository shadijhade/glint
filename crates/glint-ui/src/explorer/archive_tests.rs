//! Headless tests of the archives part: Compress to, Extract here, copying out of archives (the other pane, the
//! clipboard, a drag), opening a file from an archive, mounting an image and opening a previous version. They run
//! on the fake disk behind the same `Source` the app uses for glint-core, with Arabic names where a name is shown.

use super::fake::*;
use super::*;
use crate::testing;
use glint_model::entries::{attr, with_details, RawEntry};
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

struct Harness {
    explorer: Entity<Explorer>,
    disk: FakeDisk,
    events: Rc<RefCell<Vec<ExplorerEvent>>>,
}

/// An Arabic archive and an Arabic file, so every name that is shown is also shown right to left.
const ARCHIVE: &str = "C:\\a\\أرشيف.zip";
const ARCHIVE_DIR: &str = "C:\\a\\أرشيف.zip";

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

/// `C:\a` holds files, folders, an Arabic archive, a single compressed file and a disc image; the archive has
/// files in it; `C:\b` has one file; `C:\locked` lists a file from a folder Windows keeps closed.
fn fixture(disk: &FakeDisk) {
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), folder("C:\\", "locked")],
    );
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "sub"),
            folder("C:\\a", "v1.2"),
            file("C:\\a", "one.txt", 100, 10),
            file("C:\\a", "two.txt", 200, 20),
            file("C:\\a", "تقرير.pdf", 300, 30),
            file("C:\\a", "أرشيف.zip", 5000, 40),
            file("C:\\a", "notes.txt.gz", 50, 50),
            file("C:\\a", "disc.iso", 9000, 60),
        ],
    );
    disk.folder("C:\\a\\sub", vec![file("C:\\a\\sub", "deep.txt", 1, 1)]);
    disk.folder("C:\\a\\v1.2", vec![]);
    disk.folder("C:\\b", vec![file("C:\\b", "b1.txt", 1, 1)]);
    disk.folder(
        "C:\\locked",
        vec![with_details(
            RawEntry {
                name: "x.txt".into(),
                attrs: attr::CLOSED,
                ..Default::default()
            },
            "C:\\locked",
        )],
    );
    disk.archive_folder(
        ARCHIVE_DIR,
        ARCHIVE,
        vec![
            folder(ARCHIVE_DIR, "docs"),
            file(ARCHIVE_DIR, "inner.txt", 100, 1),
            file(ARCHIVE_DIR, "ملف.txt", 200, 2),
        ],
    );
    disk.archive_folder(
        "C:\\a\\أرشيف.zip\\docs",
        ARCHIVE,
        vec![file("C:\\a\\أرشيف.zip\\docs", "deep.md", 7, 1)],
    );
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
                .map(|t| t.selection.iter().map(str::to_string).collect())
                .unwrap_or_default()
        })
    }

    fn events(&self) -> Vec<ExplorerEvent> {
        self.events.borrow().clone()
    }

    /// What archive work told the person, in order.
    fn said(&self) -> Vec<ArchiveNotice> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Archive(n) => Some(n),
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

    fn operations(&self) -> Vec<OpNotice> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Operation(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    /// The newest job a view asked the archives part for, as the coordinator receives it.
    fn archive_job(&self) -> Option<ArchiveJob> {
        self.events().into_iter().rev().find_map(|e| match e {
            ExplorerEvent::FromArchive(job) => Some(job),
            _ => None,
        })
    }

    fn handoffs(&self) -> Vec<Handoff> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                ExplorerEvent::Handoff(h) => Some(h),
                _ => None,
            })
            .collect()
    }

    fn next_undo(&self, cx: &TestAppContext) -> Option<(UndoId, String)> {
        self.read(cx, |e| e.next_undo().map(|(id, label)| (id, label.to_string())))
    }

    /// The id the slow-work notice of the newest archive job has.
    fn shown_id(&self) -> Option<u64> {
        self.said().into_iter().rev().find_map(|n| match n {
            ArchiveNotice::Shown { id, .. } => Some(id),
            _ => None,
        })
    }

    /// Two panes: the left on `C:\a`, the right on `C:\b`, the keyboard in the left.
    fn two_panes(&self, cx: &mut TestAppContext) {
        assert!(self.go(cx, "C:\\a"));
        self.run(cx, |e, cx| e.toggle_dual(cx));
        assert!(self.go(cx, "C:\\b"));
        self.run(cx, |e, cx| e.focus_pane(0, cx));
    }
}

fn staged_names(disk: &FakeDisk) -> Vec<String> {
    disk.names(&format!("{STAGE_ROOT}1"))
}

// ---- Compress to --------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn compress_makes_a_zip_next_to_the_item_named_after_it_selects_it_and_starts_renaming(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.compress(vec!["C:\\a\\one.txt".into()], CompressFormat::Zip, None, cx)
    });

    let calls = h.disk.compressions();
    assert_eq!(calls.len(), 1);
    let (request, lane) = &calls[0];
    assert_eq!(request.paths, ["C:\\a\\one.txt"]);
    assert_eq!(request.folder, "C:\\a");
    assert_eq!(request.name, "one.zip", "named after the item, without its extension");
    assert_eq!(request.format, CompressFormat::Zip);
    assert_eq!(request.password, None);
    assert_eq!(lane, "tab1", "each tab works in its own lane");

    assert!(h.disk.exists("C:\\a\\one.zip"));
    assert!(
        h.names(&*cx).contains(&"one.zip".to_string()),
        "the folder was read again"
    );
    assert_eq!(h.selected(cx), ["C:\\a\\one.zip"]);
    // Like a new folder, the archive is named in place.
    assert_eq!(
        h.read(cx, |e| e.active_tab().and_then(|t| t.renaming.clone())),
        Some("C:\\a\\one.zip".to_string())
    );
    // No notice for the quick job; it ended with nothing running.
    assert!(matches!(
        &h.said()[..],
        [ArchiveNotice::Ended {
            was_shown: false,
            idle: true,
            ..
        }]
    ));
    assert_eq!(h.read(cx, |e| e.busy_count()), 0);

    // Ctrl+Z recycles the archive.
    let (id, label) = h.next_undo(cx).expect("undo was remembered");
    assert_eq!(label, "compression into “one.zip”");
    h.task(cx, |e, cx| e.undo(Some(id), cx));
    assert!(!h.disk.exists("C:\\a\\one.zip"));
    assert_eq!(h.disk.bin_names(), ["one.zip"]);
}

#[gpui_kit::test]
fn compress_names_the_archive_after_the_first_item_in_the_format_asked_and_passes_the_password(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.compress(
            vec!["C:\\a\\تقرير.pdf".into(), "C:\\a\\two.txt".into()],
            CompressFormat::SevenZ,
            Some("سر".into()),
            cx,
        )
    });
    // A folder keeps the dots in its name; an empty password is no password; a tar takes none.
    h.task(cx, |e, cx| {
        e.compress(
            vec!["C:\\a\\v1.2".into()],
            CompressFormat::TarZst,
            Some(String::new()),
            cx,
        )
    });
    let calls = h.disk.compressions();
    assert_eq!(calls[0].0.name, "تقرير.7z");
    assert_eq!(calls[0].0.password.as_deref(), Some("سر"));
    assert_eq!(calls[0].0.paths.len(), 2);
    assert_eq!(calls[1].0.name, "v1.2.tar.zst");
    assert_eq!(calls[1].0.password, None);
    // Several items: undo says how many.
    assert!(h.disk.exists("C:\\a\\تقرير.7z") && h.disk.exists("C:\\a\\v1.2.tar.zst"));
}

#[gpui_kit::test]
fn compress_numbers_a_taken_name_and_an_archive_can_itself_be_compressed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| {
        e.compress(vec!["C:\\a\\أرشيف.zip".into()], CompressFormat::Zip, None, cx)
    });
    // The zip of أرشيف.zip is أرشيف.zip: taken, so numbered by the engine.
    assert!(h.disk.exists("C:\\a\\أرشيف (2).zip"));
    assert_eq!(h.selected(cx), ["C:\\a\\أرشيف (2).zip"]);
    assert!(h.notices().is_empty(), "an archive on disk is not read-only");
}

#[gpui_kit::test]
fn compress_from_where_the_archive_is_not_shown_says_so_instead_of_selecting_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    h.task(cx, |e, cx| {
        e.compress(vec!["C:\\a\\one.txt".into()], CompressFormat::Zip, None, cx)
    });
    assert!(h.disk.exists("C:\\a\\one.zip"));
    assert!(h.said().contains(&ArchiveNotice::Compressed {
        what: Subject::One("one.txt".into()),
        zip: "C:\\a\\one.zip".into(),
    }));
    assert_eq!(h.names(cx), ["b1.txt"], "the folder on screen is left alone");
    assert_eq!(h.read(cx, |e| e.active_tab().and_then(|t| t.renaming.clone())), None);
    assert!(h.next_undo(cx).is_some(), "it can still be undone");
}

#[gpui_kit::test]
fn compress_inside_an_archive_or_a_closed_folder_says_why_and_asks_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    h.task(cx, |e, cx| {
        e.compress(vec![format!("{ARCHIVE_DIR}\\inner.txt")], CompressFormat::Zip, None, cx)
    });
    assert_eq!(h.notices(), [Notice::ReadOnly { remote: false }]);

    assert!(h.go(cx, "C:\\locked"));
    h.task(cx, |e, cx| {
        e.compress(vec!["C:\\locked\\x.txt".into()], CompressFormat::Zip, None, cx)
    });
    assert!(matches!(&h.notices()[1], Notice::Closed { path, .. } if path == "C:\\locked"));

    // A remote connection's files are read-only too, and nothing at all does nothing.
    h.task(cx, |e, cx| {
        e.compress(vec!["remote:srv\\x.txt".into()], CompressFormat::Zip, None, cx)
    });
    assert_eq!(h.notices().len(), 3);
    h.task(cx, |e, cx| e.compress(Vec::new(), CompressFormat::Zip, None, cx));
    assert_eq!(h.notices().len(), 3);
    assert!(h.disk.compressions().is_empty());
    assert!(h.said().is_empty());
}

#[gpui_kit::test]
fn files_that_could_not_be_added_are_reported_with_the_first_reason(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.skip_in_archive("C:\\a\\two.txt", "Access is denied");
    h.task(cx, |e, cx| {
        e.compress(
            vec!["C:\\a\\one.txt".into(), "C:\\a\\two.txt".into()],
            CompressFormat::Zip,
            None,
            cx,
        )
    });
    assert!(h.said().contains(&ArchiveNotice::Skipped {
        count: 1,
        names: vec!["two.txt".into()],
        more: 0,
        reason: "Access is denied".into(),
        verb: SkippedVerb::Added,
    }));
    // The archive was still made.
    assert!(h.disk.exists("C:\\a\\one.zip"));
}

#[gpui_kit::test]
fn many_skipped_files_name_three_and_count_the_rest(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let all: Vec<String> = ["inner.txt", "ملف.txt", "docs"]
        .iter()
        .map(|n| format!("{ARCHIVE_DIR}\\{n}"))
        .collect();
    // Five items asked for, two don't exist and two are damaged: the fake stages one and skips four.
    let mut paths = all.clone();
    paths.push(format!("{ARCHIVE_DIR}\\gone1.txt"));
    paths.push(format!("{ARCHIVE_DIR}\\gone2.txt"));
    h.disk.skip_in_archive(&all[0], "Password-protected");
    h.disk.skip_in_archive(&all[1], "Damaged");
    h.run(cx, |e, cx| {
        e.run_archive_job(ArchiveJob::ToClipboard { paths }, cx).detach()
    });
    let skipped = h
        .said()
        .into_iter()
        .find_map(|n| match n {
            ArchiveNotice::Skipped {
                count,
                names,
                more,
                reason,
                verb,
            } => Some((count, names, more, reason, verb)),
            _ => None,
        })
        .expect("a warning");
    assert_eq!(skipped.0, 4);
    assert_eq!(skipped.1, ["inner.txt", "ملف.txt", "gone1.txt"]);
    assert_eq!(skipped.2, 1);
    assert_eq!(skipped.3, "Password-protected");
    assert_eq!(skipped.4, SkippedVerb::Extracted);
}

#[gpui_kit::test]
fn a_failed_compression_is_told_with_its_reason_and_a_stopped_one_is_silent(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.fail_archive_next("There is not enough space on the disk");
    h.task(cx, |e, cx| {
        e.compress(vec!["C:\\a\\one.txt".into()], CompressFormat::Zip, None, cx)
    });
    assert!(h.said().contains(&ArchiveNotice::Failed {
        what: ArchiveFailure::Compress(Subject::One("one.txt".into())),
        message: "There is not enough space on the disk".into(),
    }));
    assert!(!h.disk.exists("C:\\a\\one.zip"));
    assert!(h.next_undo(cx).is_none(), "nothing was made, nothing to undo");

    let before = h.said().len();
    h.disk.fail_archive_next("cancelled");
    h.task(cx, |e, cx| {
        e.compress(vec!["C:\\a\\one.txt".into()], CompressFormat::Zip, None, cx)
    });
    let said = h.said();
    assert_eq!(said.len(), before + 1, "only its end");
    assert!(matches!(said.last(), Some(ArchiveNotice::Ended { .. })));
}

#[gpui_kit::test]
fn slow_archive_work_shows_a_notice_with_its_progress_and_stop_ends_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let gate = h.disk.hold_stages();
    h.disk.script_progress(&[(1, 4), (1, 4), (2, 4)]);
    let compressing = h.explorer.update(cx, |e, cx| {
        e.compress(vec!["C:\\a\\one.txt".into()], CompressFormat::Zip, None, cx)
    });
    cx.run_until_parked();
    assert_eq!(h.read(cx, |e| e.busy_count()), 1, "an update waits for it");
    // Progress reaches the taskbar at once, a changed percent at a time; the notice is not up yet.
    let progress: Vec<(u8, bool)> = h
        .said()
        .into_iter()
        .filter_map(|n| match n {
            ArchiveNotice::Progress { percent, shown, .. } => Some((percent, shown)),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [(25, false), (50, false)]);
    assert_eq!(h.shown_id(), None);

    cx.executor().advance_clock(Duration::from_millis(450));
    cx.run_until_parked();
    let id = h.shown_id().expect("the notice shows after 400 ms");
    assert!(h.said().iter().any(|n| matches!(
        n,
        ArchiveNotice::Shown { doing: Doing::Compressing(Subject::One(name)), percent: Some(50), .. } if name == "one.txt"
    )));

    // Stop asks core to stop this tab's lane; the job ends and says nothing of it.
    h.run(cx, |e, _| e.stop_archive_work(id));
    assert_eq!(h.disk.stopped(), ["tab1"]);
    gate.close();
    cx.foreground_executor().block_test(compressing);
    cx.run_until_parked();
    assert!(!h.disk.exists("C:\\a\\one.zip"));
    assert!(!h.said().iter().any(|n| matches!(n, ArchiveNotice::Failed { .. })));
    assert!(matches!(
        h.said().last(),
        Some(ArchiveNotice::Ended {
            was_shown: true,
            idle: true,
            ..
        })
    ));
    assert_eq!(h.read(cx, |e| e.busy_count()), 0);
    // Stopping what has ended does nothing.
    h.run(cx, |e, _| e.stop_archive_work(id));
    assert!(h.disk.stopped().is_empty());
}

// ---- Extract here --------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn extract_here_makes_a_folder_next_to_the_archive_selects_it_and_undo_recycles_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.archive_contents(ARCHIVE, &[("ملف.txt", 20), ("b.txt", 5)]);
    h.task(cx, |e, cx| e.extract_here(ARCHIVE, cx));
    assert_eq!(h.disk.extractions(), [(ARCHIVE.to_string(), "tab1".to_string())]);
    assert!(h.disk.exists("C:\\a\\أرشيف"), "named after the archive");
    assert!(h.disk.exists("C:\\a\\أرشيف\\ملف.txt"));
    assert_eq!(h.selected(cx), ["C:\\a\\أرشيف"]);
    assert!(h.said().iter().all(|n| matches!(n, ArchiveNotice::Ended { .. })));

    let (id, label) = h.next_undo(cx).expect("undo was remembered");
    assert_eq!(label, "extraction of “أرشيف.zip”");
    h.task(cx, |e, cx| e.undo(Some(id), cx));
    assert!(!h.disk.exists("C:\\a\\أرشيف"));
    assert_eq!(h.disk.bin_names(), ["أرشيف"]);
}

#[gpui_kit::test]
fn extracting_a_single_compressed_file_shows_the_file_itself(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.single_file_archive("C:\\a\\notes.txt.gz", "notes.txt");
    h.task(cx, |e, cx| e.extract_here("C:\\a\\notes.txt.gz", cx));
    assert!(h.disk.exists("C:\\a\\notes.txt"));
    assert_eq!(h.selected(cx), ["C:\\a\\notes.txt"]);
    assert_eq!(
        h.next_undo(cx).map(|(_, l)| l),
        Some("extraction of “notes.txt.gz”".to_string())
    );
}

#[gpui_kit::test]
fn extracting_where_the_archive_is_not_shown_offers_to_open_the_result(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\b"));
    h.task(cx, |e, cx| e.extract_here(ARCHIVE, cx));
    h.disk.single_file_archive("C:\\a\\notes.txt.gz", "notes.txt");
    h.task(cx, |e, cx| e.extract_here("C:\\a\\notes.txt.gz", cx));
    assert_eq!(
        h.said()
            .into_iter()
            .filter(|n| matches!(n, ArchiveNotice::Extracted { .. }))
            .collect::<Vec<_>>(),
        [
            ArchiveNotice::Extracted {
                archive: ARCHIVE.into(),
                folder: "C:\\a\\أرشيف".into(),
                file: false
            },
            ArchiveNotice::Extracted {
                archive: "C:\\a\\notes.txt.gz".into(),
                folder: "C:\\a\\notes.txt".into(),
                file: true
            },
        ]
    );
    assert_eq!(h.names(cx), ["b1.txt"], "the folder on screen is left alone");

    // Open opens the folder; Show shows the one file, selected, in its folder.
    h.task(cx, |e, cx| e.show_extracted("C:\\a\\أرشيف", false, cx));
    assert_eq!(h.read(cx, |e| e.current_path().to_string()), "C:\\a\\أرشيف");
    h.task(cx, |e, cx| e.show_extracted("C:\\a\\notes.txt", true, cx));
    assert_eq!(h.read(cx, |e| e.current_path().to_string()), "C:\\a");
    assert_eq!(h.selected(cx), ["C:\\a\\notes.txt"]);
}

#[gpui_kit::test]
fn a_failed_extraction_is_told_and_leaves_nothing_to_undo(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.fail_archive_next("The archive is damaged");
    h.task(cx, |e, cx| e.extract_here(ARCHIVE, cx));
    assert!(h.said().contains(&ArchiveNotice::Failed {
        what: ArchiveFailure::Extract(Subject::One("أرشيف.zip".into())),
        message: "The archive is damaged".into(),
    }));
    assert!(h.next_undo(cx).is_none());
}

// ---- Copying out of an archive -----------------------------------------------------------------------------------

#[gpui_kit::test]
fn copying_out_of_an_archive_to_the_other_pane_extracts_copies_shows_them_and_removes_the_staged_copies(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.two_panes(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let paths = vec![format!("{ARCHIVE_DIR}\\inner.txt"), format!("{ARCHIVE_DIR}\\ملف.txt")];
    h.task(cx, |e, cx| e.copy_to_other_pane(false, Some(paths.clone()), cx));
    let job = h.archive_job().expect("the explorer hands the job over");
    let ArchiveJob::CopyTo { target, .. } = &job else {
        panic!("a copy to the other pane");
    };
    assert_eq!(target, "C:\\b");
    assert!(
        h.disk.stages().is_empty(),
        "nothing is extracted until the coordinator runs the job"
    );

    h.task(cx, |e, cx| e.run_archive_job(job, cx));
    let stages = h.disk.stages();
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].purpose, StagePurpose::Copy);
    assert_eq!(stages[0].paths, paths);
    assert_eq!(stages[0].lane, "tab1");

    // Copied by Windows' file engine from the staged copies, into the other pane's folder, and shown there.
    assert!(matches!(
        &h.disk.ops()[..],
        [FileOp::Copy { sources, dest }]
            if dest == "C:\\b" && sources.len() == 2 && sources[0].starts_with(STAGE_ROOT)
    ));
    assert!(h.disk.exists("C:\\b\\inner.txt") && h.disk.exists("C:\\b\\ملف.txt"));
    h.read(cx, |e| {
        let other = e.pane_tab(1).expect("right pane");
        assert!(other.rows.iter().any(|r| r.name == "ملف.txt"));
        assert!(other.selection.contains("C:\\b\\ملف.txt"));
        assert_eq!(e.active_pane(), 0, "the keyboard stays where it was");
    });
    // The staged copies are removed, and the archive is untouched.
    assert_eq!(h.disk.unstaged(), [format!("{STAGE_ROOT}1")]);
    assert!(staged_names(&h.disk).is_empty());
    assert_eq!(h.disk.names(ARCHIVE_DIR), ["docs", "inner.txt", "ملف.txt"]);
    assert_eq!(h.read(cx, |e| e.busy_count()), 0);
}

#[gpui_kit::test]
fn a_folder_in_an_archive_is_copied_out_with_what_is_in_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.two_panes(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    h.task(cx, |e, cx| {
        e.copy_to_other_pane(false, Some(vec![format!("{ARCHIVE_DIR}\\docs")]), cx)
    });
    let job = h.archive_job().expect("the job");
    h.task(cx, |e, cx| e.run_archive_job(job, cx));
    assert!(h.disk.exists("C:\\b\\docs\\deep.md"));
}

#[gpui_kit::test]
fn copying_out_of_an_archive_that_cannot_be_extracted_copies_nothing_and_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.two_panes(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let path = format!("{ARCHIVE_DIR}\\inner.txt");
    h.disk.fail_archive_next("The password doesn't open it.");
    h.task(cx, |e, cx| {
        e.run_archive_job(
            ArchiveJob::CopyTo {
                paths: vec![path],
                target_tab: TabId(2),
                target: "C:\\b".into(),
            },
            cx,
        )
    });
    assert!(h.said().contains(&ArchiveNotice::Failed {
        what: ArchiveFailure::Extract(Subject::One("inner.txt".into())),
        message: "The password doesn't open it.".into(),
    }));
    assert!(h.disk.ops().is_empty());
    assert!(h.disk.unstaged().is_empty());
}

#[gpui_kit::test]
fn a_copy_that_fails_still_removes_the_staged_copies(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.two_panes(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let path = format!("{ARCHIVE_DIR}\\inner.txt");
    h.disk.fail_next("Access is denied");
    h.task(cx, |e, cx| {
        e.run_archive_job(
            ArchiveJob::CopyTo {
                paths: vec![path],
                target_tab: TabId(2),
                target: "C:\\b".into(),
            },
            cx,
        )
    });
    assert!(h.operations().iter().any(|n| matches!(
        n,
        OpNotice::Failed { what: Failing::CopyOrMove { moving: false, count: 1 }, message } if message == "Access is denied"
    )));
    assert_eq!(h.disk.unstaged().len(), 1);
}

#[gpui_kit::test]
fn copying_from_an_archive_to_the_clipboard_extracts_first_and_puts_the_copies_on_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let paths = vec![format!("{ARCHIVE_DIR}\\ملف.txt"), format!("{ARCHIVE_DIR}\\inner.txt")];
    h.task(cx, |e, cx| e.copy_to_clipboard(paths.clone(), false, cx));
    let job = h.archive_job().expect("the job");
    assert_eq!(job, ArchiveJob::ToClipboard { paths: paths.clone() });
    assert!(h.disk.clipboard().is_none());

    h.task(cx, |e, cx| e.run_archive_job(job, cx));
    assert_eq!(h.disk.stages()[0].purpose, StagePurpose::Out);
    let clipboard = h.disk.clipboard().expect("files on the clipboard");
    assert!(!clipboard.cut);
    assert_eq!(clipboard.paths.len(), 2);
    assert!(clipboard.paths.iter().all(|p| p.starts_with(STAGE_ROOT)));
    assert!(h.operations().contains(&OpNotice::OnClipboard { count: 2, cut: false }));
    // They stay until they are off the clipboard: Glint removes them itself.
    assert!(h.disk.unstaged().is_empty());
}

#[gpui_kit::test]
fn files_from_a_remote_connection_are_downloaded_and_said_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let gate = h.disk.hold_stages();
    let job = h.explorer.update(cx, |e, cx| {
        e.run_archive_job(
            ArchiveJob::ToClipboard {
                paths: vec!["remote:srv\\a.txt".into(), "remote:srv\\b.txt".into()],
            },
            cx,
        )
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(450));
    cx.run_until_parked();
    assert!(h.said().iter().any(|n| matches!(
        n,
        ArchiveNotice::Shown {
            doing: Doing::Downloading(Subject::Many(2)),
            ..
        }
    )));
    h.disk.fail_archive_next("The connection dropped");
    gate.close();
    cx.foreground_executor().block_test(job);
    cx.run_until_parked();
    assert!(h.said().contains(&ArchiveNotice::Failed {
        what: ArchiveFailure::Download(Subject::Many(2)),
        message: "The connection dropped".into(),
    }));
}

// ---- Dragging out of an archive ------------------------------------------------------------------------------------

#[gpui_kit::test]
fn dragging_out_of_an_archive_extracts_the_items_and_hands_over_the_copies(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let paths = vec![format!("{ARCHIVE_DIR}\\ملف.txt")];
    let copies = h
        .task(cx, |e, cx| e.drag_from_archive(paths.clone(), cx))
        .expect("extracted");
    assert_eq!(h.disk.stages()[0].purpose, StagePurpose::Out);
    assert_eq!(copies.len(), 1);
    assert!(copies[0].starts_with(STAGE_ROOT) && copies[0].ends_with("ملف.txt"));
    assert!(h.disk.exists(&copies[0]));
    assert!(h.disk.unstaged().is_empty(), "the copies live until they are dropped");
}

#[gpui_kit::test]
fn a_drag_that_never_started_leaves_the_copies_ready_for_the_next_drag_of_the_same_items(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let paths = vec![format!("{ARCHIVE_DIR}\\inner.txt")];
    let copies = h
        .task(cx, |e, cx| e.drag_from_archive(paths.clone(), cx))
        .expect("extracted");
    // The button came up while they were being made.
    h.run(cx, |e, cx| e.drag_released(&paths, copies.clone(), cx));
    assert!(h.said().contains(&ArchiveNotice::ReadyToDrag));

    let again = h
        .task(cx, |e, cx| e.drag_from_archive(paths.clone(), cx))
        .expect("ready");
    assert_eq!(again, copies);
    assert_eq!(h.disk.stages().len(), 1, "nothing was extracted again");

    // The ready copies are used once: the third drag extracts again.
    let third = h
        .task(cx, |e, cx| e.drag_from_archive(paths.clone(), cx))
        .expect("extracted");
    assert_eq!(h.disk.stages().len(), 2);
    assert_ne!(third, copies);
}

#[gpui_kit::test]
fn ready_copies_are_not_used_for_other_items_or_when_they_are_gone(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    let first = vec![format!("{ARCHIVE_DIR}\\inner.txt")];
    let other = vec![format!("{ARCHIVE_DIR}\\ملف.txt")];
    let copies = h
        .task(cx, |e, cx| e.drag_from_archive(first.clone(), cx))
        .expect("extracted");
    h.run(cx, |e, cx| e.drag_released(&first, copies.clone(), cx));
    // Other items: extracted, and the ready copies are forgotten.
    h.task(cx, |e, cx| e.drag_from_archive(other, cx)).expect("extracted");
    assert_eq!(h.disk.stages().len(), 2);
    h.task(cx, |e, cx| e.drag_from_archive(first.clone(), cx))
        .expect("extracted");
    assert_eq!(h.disk.stages().len(), 3);

    // Gone meanwhile (dropped into a folder here, which moves them): extracted again.
    let copies = h
        .task(cx, |e, cx| e.drag_from_archive(first.clone(), cx))
        .expect("extracted");
    h.run(cx, |e, cx| e.drag_released(&first, copies.clone(), cx));
    h.disk.remove(&glint_model::paths::parent_path(&copies[0]));
    let before = h.disk.stages().len();
    h.task(cx, |e, cx| e.drag_from_archive(first.clone(), cx))
        .expect("extracted");
    assert_eq!(
        h.disk.stages().len(),
        before + 1,
        "the copies were gone, so they were made again"
    );
}

#[gpui_kit::test]
fn a_drag_that_cannot_extract_returns_the_reason_and_a_failed_drag_is_told(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, ARCHIVE_DIR));
    h.disk.fail_archive_next("Not enough memory");
    let result = h.task(cx, |e, cx| {
        e.drag_from_archive(vec![format!("{ARCHIVE_DIR}\\inner.txt")], cx)
    });
    assert_eq!(result, Err("Not enough memory".to_string()));
    assert!(h.said().iter().any(|n| matches!(
        n,
        ArchiveNotice::Failed {
            what: ArchiveFailure::Extract(_),
            ..
        }
    )));

    h.run(cx, |e, cx| e.drag_failed("The drag was refused".into(), cx));
    assert_eq!(
        h.said().last(),
        Some(&ArchiveNotice::Failed {
            what: ArchiveFailure::Drag,
            message: "The drag was refused".into()
        })
    );

    // Stopped: no word of it.
    let before = h.said().len();
    h.disk.fail_archive_next("cancelled");
    let result = h.task(cx, |e, cx| {
        e.drag_from_archive(vec![format!("{ARCHIVE_DIR}\\inner.txt")], cx)
    });
    assert_eq!(result, Err("cancelled".to_string()));
    assert!(!h.said()[before..]
        .iter()
        .any(|n| matches!(n, ArchiveNotice::Failed { .. })));
}

// ---- Opening ---------------------------------------------------------------------------------------------------------

fn archived(h: &Harness, cx: &mut TestAppContext, name: &str) -> glint_model::entries::FileEntry {
    assert!(h.go(cx, ARCHIVE_DIR));
    h.read(cx, |e| {
        e.active_tab()
            .and_then(|t| t.rows.iter().find(|r| r.name == name).cloned())
            .expect("the file is listed")
    })
}

#[gpui_kit::test]
fn opening_a_file_in_an_archive_extracts_a_read_only_copy_and_opens_that(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let entry = archived(&h, cx, "ملف.txt");
    // Enter on it hands the work over...
    h.task(cx, |e, cx| e.open_entry(&entry, cx));
    assert_eq!(h.handoffs(), [Handoff::ArchivedFile(Box::new(entry.clone()))]);
    assert!(h.disk.opened().is_empty());
    // ...and the coordinator gives it back.
    h.task(cx, |e, cx| {
        e.handoff(Handoff::ArchivedFile(Box::new(entry.clone())), cx)
    });
    assert_eq!(h.disk.extracted_files(), std::slice::from_ref(&entry.path));
    assert_eq!(
        h.disk.opened(),
        [(format!("{OPEN_ROOT}\\ملف.txt"), OpenHow::Default)],
        "the copy is opened, not the file in the archive"
    );
    assert!(h.notices().is_empty());
    assert_eq!(h.read(cx, |e| e.busy_count()), 0);
}

#[gpui_kit::test]
fn a_file_in_an_archive_opens_with_the_editor_or_the_picker_when_asked(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let entry = archived(&h, cx, "inner.txt");
    h.task(cx, |e, cx| {
        e.open_archived_with(&entry, OpenHow::Editor("C:\\Tools\\Code.exe".into()), cx)
    });
    h.task(cx, |e, cx| e.open_archived_with(&entry, OpenHow::Picker, cx));
    let copy = format!("{OPEN_ROOT}\\inner.txt");
    assert_eq!(
        h.disk.opened(),
        [
            (copy.clone(), OpenHow::Editor("C:\\Tools\\Code.exe".into())),
            (copy, OpenHow::Picker)
        ]
    );
    // The copy is made once and reused.
    assert_eq!(h.disk.extracted_files().len(), 2);
    assert_eq!(h.disk.names(OPEN_ROOT), ["inner.txt"]);
}

#[gpui_kit::test]
fn a_file_that_will_not_extract_or_open_says_it_could_not_be_opened(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let entry = archived(&h, cx, "ملف.txt");
    h.disk.fail_archive_next("The password doesn't open it.");
    h.task(cx, |e, cx| e.open_archived(&entry, cx));
    assert_eq!(
        h.notices(),
        [Notice::CouldNotOpenFile {
            path: entry.path.clone(),
            message: "The password doesn't open it.".into()
        }]
    );
    assert!(h.disk.opened().is_empty());

    h.disk.fail_open_next("No app is associated");
    h.task(cx, |e, cx| e.open_archived(&entry, cx));
    assert_eq!(h.notices().len(), 2);
    assert!(matches!(&h.notices()[1], Notice::CouldNotOpenFile { message, .. } if message == "No app is associated"));
}

#[gpui_kit::test]
fn a_remote_download_shows_progress_and_stop_and_its_failure_is_told_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let entry = archived(&h, cx, "ملف.txt");
    let gate = h.disk.hold_stages();
    let mut remote = entry.clone();
    remote.attrs |= attr::REMOTE;
    remote.path = "remote:srv\\ملف.txt".into();
    let opening = h.explorer.update(cx, |e, cx| e.open_archived(&remote, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(450));
    cx.run_until_parked();
    // A remote file is downloaded with progress and Stop.
    assert!(h.said().iter().any(|n| matches!(
        n,
        ArchiveNotice::Shown { doing: Doing::Downloading(Subject::One(name)), .. } if name == "ملف.txt"
    )));
    assert_eq!(h.disk.stages()[0].purpose, StagePurpose::Open);
    h.disk.fail_archive_next("The connection dropped");
    gate.close();
    cx.foreground_executor().block_test(opening);
    cx.run_until_parked();
    // Its failure was told by the download itself, not as a second "couldn't open".
    assert!(h.said().iter().any(|n| matches!(
        n,
        ArchiveNotice::Failed {
            what: ArchiveFailure::Download(_),
            ..
        }
    )));
    assert!(h.notices().is_empty());
    assert!(h.disk.opened().is_empty());
}

#[gpui_kit::test]
fn a_remote_file_is_downloaded_then_opened(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    // The fake lists the remote file in a folder of its own; core downloads it as it stages an archive's.
    h.disk.folder("remote:srv", vec![file("remote:srv", "ملف.txt", 5, 5)]);
    let mut remote = file("remote:srv", "ملف.txt", 5, 5);
    remote.attrs |= attr::REMOTE | attr::IN_ARCHIVE;
    h.task(cx, |e, cx| e.open_archived(&remote, cx));
    assert_eq!(h.disk.stages()[0].purpose, StagePurpose::Open);
    let opened = h.disk.opened();
    assert_eq!(opened.len(), 1);
    assert!(opened[0].0.starts_with(STAGE_ROOT) && opened[0].0.ends_with("ملف.txt"));
}

// ---- Disk images ----------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn mounting_an_image_reads_the_drives_says_so_and_opens_the_new_drive(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.set_mount_drive(Some("F:\\"));
    h.disk.folder("F:\\", vec![file("F:\\", "setup.exe", 1, 1)]);
    h.task(cx, |e, cx| e.mount_image("C:\\a\\disc.iso", None, cx));
    assert_eq!(h.disk.mounted(), ["C:\\a\\disc.iso"]);
    // The drives were read again, and the new drive is open in the tab.
    h.read(cx, |e| assert!(e.drives().iter().any(|d| d.path == "F:\\")));
    assert_eq!(h.read(cx, |e| e.current_path().to_string()), "F:\\");
    assert_eq!(h.names(cx), ["setup.exe"]);
    assert!(h.said().contains(&ArchiveNotice::Mounted {
        name: "disc.iso".into(),
        drive: Some("F:\\".into())
    }));
}

#[gpui_kit::test]
fn mounting_with_no_new_drive_just_says_it_mounted_and_a_failure_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.task(cx, |e, cx| e.mount_image("C:\\a\\disc.iso", None, cx));
    assert!(h.said().contains(&ArchiveNotice::Mounted {
        name: "disc.iso".into(),
        drive: None
    }));
    assert_eq!(h.read(cx, |e| e.current_path().to_string()), "C:\\a");

    h.disk.fail_archive_next("The image is corrupt");
    h.task(cx, |e, cx| e.mount_image("C:\\a\\disc.iso", None, cx));
    assert!(h.said().contains(&ArchiveNotice::Failed {
        what: ArchiveFailure::Mount("disc.iso".into()),
        message: "The image is corrupt".into()
    }));
}

#[gpui_kit::test]
fn the_new_drive_opens_in_the_tab_asked_for(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.two_panes(cx);
    h.disk.set_mount_drive(Some("G:\\"));
    h.disk.folder("G:\\", vec![]);
    let right = h.read(cx, |e| e.pane_tab_id(1));
    h.task(cx, |e, cx| e.mount_image("C:\\a\\disc.iso", Some(right), cx));
    h.read(cx, |e| {
        assert_eq!(e.tab(right).map(|t| t.path.clone()), Some("G:\\".to_string()));
        assert_eq!(e.active_tab().map(|t| t.path.clone()), Some("C:\\a".to_string()));
    });
}

#[gpui_kit::test]
fn a_disk_image_mounts_when_windows_does_by_default_else_it_opens_in_its_app(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let image = h.read(cx, |e| {
        e.active_tab()
            .and_then(|t| t.rows.iter().find(|r| r.name == "disc.iso").cloned())
            .expect("the image")
    });
    // Enter hands it over.
    h.task(cx, |e, cx| e.open_entry(&image, cx));
    assert_eq!(h.handoffs(), [Handoff::DiskImage("C:\\a\\disc.iso".into())]);

    // Windows' Mount is not the default app: it opens in the app chosen for it.
    h.disk.on_open("C:\\a\\disc.iso", None);
    h.task(cx, |e, cx| e.handoff(Handoff::DiskImage("C:\\a\\disc.iso".into()), cx));
    assert!(h.disk.mounted().is_empty());

    h.disk.set_mounts_by_default(true);
    h.task(cx, |e, cx| e.handoff(Handoff::DiskImage("C:\\a\\disc.iso".into()), cx));
    assert_eq!(h.disk.mounted(), ["C:\\a\\disc.iso"]);
}

// ---- Previous versions ----------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_previous_version_of_a_file_is_copied_out_and_that_copy_is_opened(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let version = "C:\\a\\@GMT-2026.11.01-10.00.00\\تقرير.pdf";
    h.task(cx, |e, cx| e.handoff(Handoff::VersionFile(version.into()), cx));
    assert_eq!(h.disk.version_copies(), [version]);
    assert_eq!(
        h.disk.opened(),
        [(format!("{VERSION_ROOT}\\تقرير.pdf"), OpenHow::Default)]
    );
    assert!(h.notices().is_empty());

    h.disk.fail_archive_next("Couldn't copy it out");
    h.task(cx, |e, cx| e.open_version_file(version, cx));
    assert_eq!(
        h.notices(),
        [Notice::CouldNotOpenFile {
            path: version.into(),
            message: "Couldn't copy it out".into()
        }]
    );
    assert_eq!(h.disk.opened().len(), 1, "the failed copy was not opened");
}

#[gpui_kit::test]
fn the_busy_count_includes_archive_work_so_an_update_waits_for_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let gate = h.disk.hold_stages();
    let job = h.explorer.update(cx, |e, cx| e.extract_here(ARCHIVE, cx));
    cx.run_until_parked();
    assert_eq!(h.read(cx, |e| e.busy_count()), 1);
    gate.close();
    cx.foreground_executor().block_test(job);
    cx.run_until_parked();
    assert_eq!(h.read(cx, |e| e.busy_count()), 0);
}
