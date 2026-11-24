//! Headless tests of searching by name: a fake disk behind the same `Source` the app uses. The fake either
//! looks through its folders (fd's way, or the index's when the test says it has the drive) or lets a test say what
//! a search says, when it says it.

use super::fake::*;
use super::*;
use crate::testing;
use glint_model::store::SavedSearch;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

struct Rig {
    explorer: Entity<Explorer>,
    disk: FakeDisk,
    events: Rc<RefCell<Vec<ExplorerEvent>>>,
}

fn rig(cx: &mut TestAppContext) -> Rig {
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
    Rig { explorer, disk, events }
}

/// `C:\` with folders `a` and `b`; reports in both, an Arabic name in `b`, a hidden one in `a`.
fn reports(disk: &FakeDisk) {
    disk.folder("C:\\", vec![folder("C:\\", "a"), folder("C:\\", "b")]);
    disk.folder(
        "C:\\a",
        vec![
            file("C:\\a", "report.pdf", 10, 1),
            file("C:\\a", "notes.md", 20, 2),
            hidden("C:\\a", "report-secret.txt"),
        ],
    );
    disk.folder(
        "C:\\b",
        vec![
            file("C:\\b", "report2.pdf", 30, 3),
            file("C:\\b", "تقرير السنة.pdf", 40, 4),
            file("C:\\b", "other.txt", 50, 5),
        ],
    );
}

impl Rig {
    fn go(&self, cx: &mut TestAppContext, path: &str) {
        let task = self
            .explorer
            .update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        assert!(cx.foreground_executor().block_test(task));
    }

    /// Searches the active tab and waits for the search to end.
    fn find(&self, cx: &mut TestAppContext, query: &str) {
        let task = self
            .explorer
            .update(cx, |e, cx| e.run_find(query, Wider::default(), cx));
        cx.foreground_executor().block_test(task);
    }

    /// Starts a search and lets what is ready happen, without waiting for it to end.
    fn start(&self, cx: &mut TestAppContext, query: &str) -> gpui_kit::Task<()> {
        let task = self
            .explorer
            .update(cx, |e, cx| e.run_find(query, Wider::default(), cx));
        cx.run_until_parked();
        task
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

    /// The names the list draws.
    fn names(&self, cx: &TestAppContext) -> Vec<String> {
        self.read(cx, |e| {
            e.active_tab()
                .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    fn run_of(&self, cx: &TestAppContext) -> Option<FindRun> {
        self.read(cx, |e| e.active_tab().and_then(|t| t.find.clone()))
    }

    fn found_events(&self) -> Vec<FindNotice> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                ExplorerEvent::Find(notice) => Some(notice.clone()),
                _ => None,
            })
            .collect()
    }
}

fn found(dir: &str, names: &[&str]) -> Vec<glint_model::entries::FileEntry> {
    names.iter().map(|name| file(dir, name, 1, 1)).collect()
}

fn finished(count: usize) -> Found {
    Found::Finished {
        count,
        elapsed: 3.0,
        limited: false,
        fuzzy: false,
    }
}

// ---- A search shows results ---------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_search_looks_below_the_folder_and_shows_what_it_finds(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.find(cx, "report");
    // Below the folder, hidden ones left out, named as found; sorted as the list is (folders first, then name).
    assert_eq!(r.names(cx), ["report.pdf", "report2.pdf"]);
    let run = r.run_of(cx).expect("a search is shown");
    assert!(!run.running && run.error.is_empty() && !run.limited && !run.fuzzy);
    assert_eq!(
        (run.query.as_str(), run.scope.as_str(), run.engine.as_str()),
        ("report", "C:\\", "fd")
    );
    assert_eq!(run.entries.len(), 2);
    assert!(
        run.skipped == ["node_modules", ".git"],
        "Settings' folders to skip are what it didn't look in"
    );
    // What the engine was asked: the tab's lane, the folder, Settings' options.
    let call = r.disk.finds().pop().expect("a search was started");
    assert_eq!(
        (call.lane.as_str(), call.roots.as_slice()),
        ("tab1", ["C:\\".to_string()].as_slice())
    );
    assert_eq!(
        call.options,
        FindOptions {
            hidden: false,
            respect_ignore: false,
            exclude: vec!["node_modules".into(), ".git".into()],
            max_results: Some(50_000),
            fuzzy: true,
            rank: true,
            marks: true
        }
    );
    // The tab shows the search, not the folder: its columns are the search's, nothing is selected.
    r.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert!(tab.selection.is_empty());
        assert!(tab.view_key().starts_with("find:"));
        assert_eq!(
            tab.columns,
            [
                glint_model::columns::ColumnId::Modified,
                glint_model::columns::ColumnId::Size
            ]
        );
    });
}

#[gpui_kit::test]
fn results_come_in_as_the_search_streams_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    let live = r.disk.live_find("rep");
    let waiting = r.start(cx, "rep");
    // Running, nothing yet: the list shows its searching state.
    let run = r.run_of(cx).expect("a search is shown");
    assert!(run.running && run.entries.is_empty() && run.engine.is_empty());
    assert!(r.names(cx).is_empty());

    live.try_send(Found::Started { engine: "index".into() }).expect("open");
    live.try_send(Found::Entries(found("C:\\a", &["report.pdf", "repo.txt"])))
        .expect("open");
    cx.run_until_parked();
    let run = r.run_of(cx).expect("a search is shown");
    assert!(run.running, "still searching");
    assert_eq!(run.engine, "index");
    assert_eq!(r.names(cx), ["repo.txt", "report.pdf"]);

    // The next batch adds to them.
    live.try_send(Found::Entries(found("C:\\b", &["report2.pdf"])))
        .expect("open");
    live.try_send(Found::Finished {
        count: 3,
        elapsed: 5.0,
        limited: true,
        fuzzy: false,
    })
    .expect("open");
    cx.run_until_parked();
    let run = r.run_of(cx).expect("a search is shown");
    assert!(!run.running && run.limited && !run.fuzzy);
    assert!(run.elapsed >= 0.0);
    assert_eq!(r.names(cx), ["repo.txt", "report.pdf", "report2.pdf"]);
    // The task the caller holds resolves when the search ends.
    cx.foreground_executor().block_test(waiting);
}

#[gpui_kit::test]
fn nothing_found_is_a_finished_search_with_no_entries_and_close_matches_say_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.find(cx, "zzz");
    let run = r.run_of(cx).expect("a search is shown");
    assert!(!run.running && run.entries.is_empty() && run.error.is_empty());
    assert!(r.names(cx).is_empty());

    let live = r.disk.live_find("repotr");
    let waiting = r.start(cx, "repotr");
    live.try_send(Found::Started { engine: "index".into() }).expect("open");
    live.try_send(Found::Entries(found("C:\\a", &["report.pdf"])))
        .expect("open");
    live.try_send(Found::Finished {
        count: 1,
        elapsed: 2.0,
        limited: false,
        fuzzy: true,
    })
    .expect("open");
    cx.foreground_executor().block_test(waiting);
    let run = r.run_of(cx).expect("a search is shown");
    assert!(run.fuzzy, "the index looked again with typos allowed");
    assert_eq!(r.names(cx), ["report.pdf"]);
}

#[gpui_kit::test]
fn a_search_that_fails_shows_its_error_with_nothing_found(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    // core refuses the query outright
    r.disk.refuse_next_find("Type part of a file name to search for");
    r.find(cx, "ext:");
    let run = r.run_of(cx).expect("a search is shown");
    assert!(!run.running && run.entries.is_empty());
    assert_eq!(run.error, "Type part of a file name to search for");

    // fd fails halfway: what it found is dropped, the error stays through the end.
    let live = r.disk.live_find("half");
    let waiting = r.start(cx, "half");
    live.try_send(Found::Entries(found("C:\\a", &["report.pdf"])))
        .expect("open");
    live.try_send(Found::Error("fd stopped".into())).expect("open");
    live.try_send(finished(0)).expect("open");
    cx.foreground_executor().block_test(waiting);
    let run = r.run_of(cx).expect("a search is shown");
    assert_eq!(run.error, "fd stopped");
    assert!(run.entries.is_empty() && !run.running);
}

#[gpui_kit::test]
fn arabic_names_are_found_and_listed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(true);
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.find(cx, "تقرير");
    assert_eq!(r.names(cx), ["تقرير السنة.pdf"]);
    let run = r.run_of(cx).expect("a search is shown");
    assert_eq!(run.query, "تقرير");
    // Two words of a name, in either order, with a Latin one beside.
    r.find(cx, "السنة pdf");
    assert_eq!(r.names(cx), ["تقرير السنة.pdf"]);
    testing::set_rtl(false);
}

// ---- A newer query wins -----------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_newer_query_replaces_the_older_search_and_its_late_words_are_dropped(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    let old = r.disk.live_find("old");
    let new = r.disk.live_find("new");
    let first = r.start(cx, "old");
    old.try_send(Found::Entries(found("C:\\a", &["old1.txt"])))
        .expect("open");
    cx.run_until_parked();
    assert_eq!(r.names(cx), ["old1.txt"]);
    let first_id = r.run_of(cx).expect("a search").id;

    let second = r.start(cx, "new");
    let run = r.run_of(cx).expect("a search is shown");
    assert_eq!(run.query, "new");
    assert_ne!(run.id, first_id, "each search has its own number");
    // The older one was replaced, not left to run on in the background.
    assert!(
        old.try_send(Found::Entries(found("C:\\a", &["old2.txt"]))).is_err(),
        "nobody reads the older stream"
    );
    cx.foreground_executor().block_test(first);

    new.try_send(Found::Entries(found("C:\\b", &["new1.txt"])))
        .expect("open");
    new.try_send(finished(1)).expect("open");
    cx.foreground_executor().block_test(second);
    assert_eq!(r.names(cx), ["new1.txt"]);
    assert!(!r.run_of(cx).expect("a search is shown").running);
    // Core replaces the older search in the lane itself: no cancel was needed.
    assert!(r.disk.cancelled().is_empty());
    assert_eq!(r.disk.finds().len(), 2);
}

#[gpui_kit::test]
fn searching_the_same_folder_again_keeps_the_old_results_until_the_first_new_ones_come(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.find(cx, "report");
    assert_eq!(r.names(cx), ["report.pdf", "report2.pdf"]);

    // F5 with results showing: the same search again.
    let live = r.disk.live_find("report");
    let waiting = r.run(cx, |e, cx| e.rerun_find(cx));
    assert!(r.run_of(cx).expect("a search").running);
    assert_eq!(
        r.names(cx),
        ["report.pdf", "report2.pdf"],
        "still the last results while it runs"
    );
    assert_eq!(
        r.run_of(cx).expect("a search").engine,
        "fd",
        "and the engine that answered last"
    );
    live.try_send(Found::Entries(found("C:\\a", &["report-new.pdf"])))
        .expect("open");
    cx.run_until_parked();
    assert_eq!(r.names(cx), ["report-new.pdf"], "the first new ones replace them");
    live.try_send(finished(1)).expect("open");
    cx.foreground_executor().block_test(waiting);

    // A search that finds nothing new empties the list.
    let live = r.disk.live_find("report");
    let waiting = r.run(cx, |e, cx| e.rerun_find(cx));
    live.try_send(finished(0)).expect("open");
    cx.foreground_executor().block_test(waiting);
    assert!(r.names(cx).is_empty());

    // Another scope starts empty.
    r.find(cx, "report");
    let task = r.run(cx, |e, cx| e.run_find("report", Wider::in_scope("C:\\b"), cx));
    cx.foreground_executor().block_test(task);
    assert_eq!(r.names(cx), ["report2.pdf"]);
    assert_eq!(r.run_of(cx).expect("a search").scope, "C:\\b");
}

#[gpui_kit::test]
fn a_long_list_waits_for_small_batches_but_the_end_shows_everything(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    let live = r.disk.live_find("many");
    let waiting = r.start(cx, "many");
    let batch = |from: usize, n: usize| -> Vec<_> {
        (from..from + n)
            .map(|i| file("C:\\a", &format!("many{i:05}.txt"), 1, 1))
            .collect()
    };
    live.try_send(Found::Entries(batch(0, 3_000))).expect("open");
    cx.run_until_parked();
    let shown = |r: &Rig, cx: &TestAppContext| r.run_of(cx).expect("a search").entries.len();
    assert_eq!(shown(&r, cx), 3_000);
    // 100 more is under a quarter of what shows: it waits, so the list isn't copied for every batch.
    live.try_send(Found::Entries(batch(3_000, 100))).expect("open");
    cx.run_until_parked();
    assert_eq!(shown(&r, cx), 3_000);
    // A big one carries the waiting ones along.
    live.try_send(Found::Entries(batch(3_100, 1_000))).expect("open");
    cx.run_until_parked();
    assert_eq!(shown(&r, cx), 4_100);
    // Small again, then the end: everything shows.
    live.try_send(Found::Entries(batch(4_100, 10))).expect("open");
    live.try_send(finished(4_110)).expect("open");
    cx.foreground_executor().block_test(waiting);
    assert_eq!(shown(&r, cx), 4_110);
    assert!(!r.run_of(cx).expect("a search").running);
}

// ---- Cancelling -----------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn leaving_the_results_cancels_a_running_search_and_shows_the_folder_with_the_filter_kept(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\a");
    r.run(cx, |e, cx| e.set_filter("report", None, cx));
    let live = r.disk.live_find("report");
    let _waiting = r.start(cx, "report");
    assert!(r.run_of(cx).expect("a search").running);

    r.run(cx, |e, cx| e.leave_find(None, cx));
    assert_eq!(r.disk.cancelled(), ["tab1"]);
    assert!(r.run_of(cx).is_none());
    // The folder is back (read again), narrowed by the text still in the box.
    assert_eq!(r.names(cx), ["report.pdf"]);
    r.read(cx, |e| assert_eq!(e.active_tab().expect("a tab").filter, "report"));
    assert!(r.disk.listed().len() >= 2, "the folder was read again");
    assert!(
        live.try_send(Found::Entries(found("C:\\a", &["late.txt"]))).is_err(),
        "nobody reads it any more"
    );
    // Leaving when nothing is shown does nothing.
    r.run(cx, |e, cx| e.leave_find(None, cx));
    assert_eq!(r.disk.cancelled().len(), 1);
}

#[gpui_kit::test]
fn clearing_the_search_cancels_it_and_empties_the_filter(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\a");
    r.run(cx, |e, cx| e.set_filter("report", None, cx));
    let _live = r.disk.live_find("report");
    let _waiting = r.start(cx, "report");
    r.run(cx, |e, cx| e.clear_find(cx));
    assert_eq!(r.disk.cancelled(), ["tab1"]);
    assert!(r.run_of(cx).is_none());
    assert_eq!(r.names(cx), ["notes.md", "report.pdf"]);
    r.read(cx, |e| assert!(e.active_tab().expect("a tab").filter.is_empty()));

    // A finished search has nothing to cancel.
    r.find(cx, "notes");
    r.run(cx, |e, cx| e.clear_find(cx));
    assert_eq!(r.disk.cancelled().len(), 1);
    // And an empty query is clearing.
    r.find(cx, "notes");
    r.run(cx, |e, cx| e.run_find("  ", Wider::default(), cx).detach());
    assert!(r.run_of(cx).is_none());
}

#[gpui_kit::test]
fn leaving_the_folder_closing_the_tab_and_making_something_new_cancel_a_running_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");

    // Going to another folder.
    let _a = r.disk.live_find("one");
    let _waiting = r.start(cx, "one");
    r.go(cx, "C:\\b");
    assert_eq!(r.disk.cancelled(), ["tab1"]);
    assert!(r.run_of(cx).is_none(), "the search went with the folder");
    assert!(r.events.borrow().contains(&ExplorerEvent::FindDropped {
        tab: TabId(1),
        was_running: true
    }));

    // Closing the tab: its lane, not another's.
    r.run(cx, |e, cx| e.new_tab(Some("C:\\a"), cx));
    cx.run_until_parked();
    let second = r.read(cx, |e| e.active_tab_id());
    assert_ne!(second, TabId(1));
    let _b = r.disk.live_find("two");
    let _waiting2 = r.start(cx, "two");
    r.run(cx, |e, cx| e.close_tab(None, cx));
    assert_eq!(r.disk.cancelled(), ["tab1".to_string(), second.to_string()]);
    assert!(r.events.borrow().contains(&ExplorerEvent::TabClosed {
        tab: second,
        find_running: true
    }));

    // New folder puts something in the folder: the search ends first.
    r.go(cx, "C:\\a");
    let _c = r.disk.live_find("three");
    let _waiting3 = r.start(cx, "three");
    let task = r.explorer.update(cx, |e, cx| e.new_item(true, cx));
    cx.foreground_executor().block_test(task);
    assert_eq!(r.disk.cancelled().len(), 3);
    assert!(r.run_of(cx).is_none());
    r.read(cx, |e| {
        assert!(
            e.active_tab().expect("a tab").renaming.is_some(),
            "the new folder is being named"
        )
    });
}

#[gpui_kit::test]
fn stop_cancels_the_search_and_keeps_what_it_found_once_it_says_it_ended(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    let live = r.disk.live_find("rep");
    let waiting = r.start(cx, "rep");
    live.try_send(Found::Entries(found("C:\\a", &["report.pdf"])))
        .expect("open");
    cx.run_until_parked();
    r.explorer.update(cx, |e, _| e.stop_find());
    assert_eq!(r.disk.cancelled(), ["tab1"]);
    // Core ends a stopped search with its usual last word.
    live.try_send(finished(1)).expect("the stream is still read");
    cx.foreground_executor().block_test(waiting);
    let run = r.run_of(cx).expect("the results stay");
    assert!(!run.running);
    assert_eq!(r.names(cx), ["report.pdf"]);
    // Stop with nothing running does nothing.
    r.explorer.update(cx, |e, _| e.stop_find());
    assert_eq!(r.disk.cancelled().len(), 1);
}

// ---- Where a search looks, and what answers ------------------------------------------------------------------

#[gpui_kit::test]
fn a_search_from_this_pc_looks_through_the_fixed_and_removable_drives(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    let drive = |path: &str, kind| Drive {
        path: path.into(),
        label: String::new(),
        kind,
        fs: "NTFS".into(),
        total: 100,
        free: 50,
    };
    r.disk.set_drives(vec![
        drive("C:\\", DriveKind::Fixed),
        drive("D:\\", DriveKind::Removable),
        drive("E:\\", DriveKind::Cdrom),
        drive("Z:\\", DriveKind::Network),
    ]);
    r.run(cx, |e, cx| e.refresh_drives(cx).detach());
    r.go(cx, "");
    r.find(cx, "report");
    let call = r.disk.finds().pop().expect("a search was started");
    assert_eq!(call.roots, ["C:\\", "D:\\"]);
    assert_eq!(r.run_of(cx).expect("a search").scope, "");
    assert_eq!(r.names(cx), ["report.pdf", "report2.pdf"]);
}

#[gpui_kit::test]
fn the_index_answers_when_it_has_the_drive_and_fd_walks_otherwise(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.find(cx, "report");
    assert_eq!(r.run_of(cx).expect("a search").engine, "fd");

    r.disk.set_search_index(Some(SearchIndex {
        on: true,
        ready: vec!["C:".into()],
        ..Default::default()
    }));
    r.find(cx, "report");
    assert_eq!(r.run_of(cx).expect("a search").engine, "index");
    assert_eq!(r.names(cx), ["report.pdf", "report2.pdf"], "the same names either way");
}

#[gpui_kit::test]
fn settings_decide_the_options_and_a_wider_search_overrides_them_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    cx.update(|cx| {
        crate::state::update(cx, |s| {
            s.show_hidden = true;
            s.find_respect_ignore = true;
            s.find_max_results = Some(1);
            s.find_exclusions = vec!["target".into()];
        })
    });
    r.find(cx, "report");
    let call = r.disk.finds().pop().expect("a search was started");
    assert!(call.options.hidden && call.options.respect_ignore);
    assert_eq!(call.options.max_results, Some(1));
    assert_eq!(call.options.exclude, ["target"]);
    let run = r.run_of(cx).expect("a search");
    assert_eq!(run.skipped, ["target", "what .gitignore files list"]);
    assert!(run.limited, "stopped at the result limit");
    assert_eq!(run.entries.len(), 1);

    let wider = Wider::in_scope("C:\\").without_limit().skipping_nothing();
    let task = r.run(cx, |e, cx| e.run_find("report", wider, cx));
    cx.foreground_executor().block_test(task);
    let call = r.disk.finds().pop().expect("a search was started");
    assert_eq!(call.options.max_results, None);
    assert!(!call.options.respect_ignore && call.options.exclude.is_empty());
    assert!(r.run_of(cx).expect("a search").skipped.is_empty());
    assert_eq!(
        r.run_of(cx).expect("a search").entries.len(),
        3,
        "hidden ones too, now that hidden items show"
    );
}

#[gpui_kit::test]
fn nothing_is_searched_below_the_recycle_bin(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, glint_model::paths::RECYCLE_BIN);
    let task = r.run(cx, |e, cx| e.run_find("report", Wider::default(), cx));
    cx.foreground_executor().block_test(task);
    assert_eq!(r.found_events(), [FindNotice::NothingBelowRecycleBin]);
    assert!(r.disk.finds().is_empty());
    assert!(r.run_of(cx).is_none());
}

#[gpui_kit::test]
fn a_drive_path_typed_outside_the_folder_searches_that_drive(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.disk.folder("D:\\", vec![file("D:\\", "photos report.txt", 1, 1)]);
    r.go(cx, "C:\\a");
    r.find(cx, "report D:\\");
    let call = r.disk.finds().pop().expect("a search was started");
    assert_eq!(call.roots, ["D:\\"]);
    assert_eq!(r.run_of(cx).expect("a search").scope, "D:\\");
}

// ---- Searching as you type ------------------------------------------------------------------------------------

fn indexed(drives: &[&str]) -> SearchIndex {
    SearchIndex {
        on: true,
        ready: drives.iter().map(|d| d.to_string()).collect(),
        ..Default::default()
    }
}

#[gpui_kit::test]
fn a_search_runs_as_typed_only_where_the_index_alone_answers_in_milliseconds(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\a");
    let can = |r: &Rig, cx: &mut TestAppContext, query: &str, path: &str| {
        r.explorer.read_with(cx, |e, cx| e.finds_as_typed(query, path, cx))
    };
    // Not known yet: nothing is the index's alone.
    assert!(!can(&r, cx, "report", "C:\\a"));

    r.run(cx, |e, cx| e.set_search_index(Some(indexed(&["C:"])), cx));
    assert!(can(&r, cx, "report", "C:\\a"));
    assert!(can(&r, cx, "report", "c:\\a"));
    assert!(!can(&r, cx, "report", "D:\\x"), "a drive the index hasn't read");
    // A path typed outside the folder moves the search to its drive.
    assert!(!can(&r, cx, "report D:\\", "C:\\a"));
    assert!(can(&r, cx, "report c:\\users", "C:\\a"));
    // What photos, videos and songs say is read from each file: Enter.
    assert!(!can(&r, cx, "holiday width:>1000", "C:\\a"));
    // .gitignore rules are fd's alone.
    cx.update(|cx| crate::state::update(cx, |s| s.find_respect_ignore = true));
    assert!(!can(&r, cx, "report", "C:\\a"));
    cx.update(|cx| crate::state::update(cx, |s| s.find_respect_ignore = false));
    // A service older than the app doesn't read filters or paths yet.
    r.run(cx, |e, cx| {
        e.set_search_index(
            Some(SearchIndex {
                outdated: true,
                ..indexed(&["C:"])
            }),
            cx,
        )
    });
    assert!(can(&r, cx, "report", "C:\\a"));
    assert!(!can(&r, cx, "report ext:pdf", "C:\\a"));
    assert!(!can(&r, cx, "src\\lib", "C:\\a"));
    // A stopped service answers nothing.
    r.run(cx, |e, cx| e.set_search_index(Some(SearchIndex::default()), cx));
    assert!(!can(&r, cx, "report", "C:\\a"));
}

#[gpui_kit::test]
fn this_pc_is_searched_as_typed_when_every_searched_drive_is_indexed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    let drive = |path: &str, kind| Drive {
        path: path.into(),
        label: String::new(),
        kind,
        fs: "NTFS".into(),
        total: 100,
        free: 50,
    };
    r.disk.set_drives(vec![
        drive("C:\\", DriveKind::Fixed),
        drive("D:\\", DriveKind::Removable),
        drive("Z:\\", DriveKind::Network),
    ]);
    r.run(cx, |e, cx| e.refresh_drives(cx).detach());
    let can = |r: &Rig, cx: &mut TestAppContext| r.explorer.read_with(cx, |e, cx| e.finds_as_typed("report", "", cx));
    r.run(cx, |e, cx| e.set_search_index(Some(indexed(&["C:"])), cx));
    assert!(!can(&r, cx), "D: isn't read yet");
    r.run(cx, |e, cx| e.set_search_index(Some(indexed(&["C:", "D:"])), cx));
    assert!(
        can(&r, cx),
        "the network drive isn't searched, so it needn't be indexed"
    );
}

#[gpui_kit::test]
fn finding_as_typed_asks_the_service_first_when_it_is_not_known_and_then_searches(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.disk.set_search_index(Some(indexed(&["C:"])));
    r.run(cx, |e, cx| e.set_filter("report", None, cx));
    let task = r.explorer.update(cx, |e, cx| e.find_as_typed("report", cx));
    cx.foreground_executor().block_test(task);
    assert_eq!(r.disk.index_asked(), 1);
    assert_eq!(r.run_of(cx).expect("it searched").engine, "index");
    assert_eq!(r.names(cx), ["report.pdf", "report2.pdf"]);

    // Known now: no new question, and a drive the index doesn't have is left to Enter.
    r.run(cx, |e, cx| e.clear_find(cx));
    r.disk.folder("D:\\", vec![]);
    r.go(cx, "D:\\");
    let before = r.disk.finds().len();
    let task = r.explorer.update(cx, |e, cx| e.find_as_typed("report", cx));
    cx.foreground_executor().block_test(task);
    assert_eq!(r.disk.index_asked(), 1);
    assert_eq!(r.disk.finds().len(), before);
}

#[gpui_kit::test]
fn finding_as_typed_does_nothing_if_the_box_moved_on_while_the_service_answered(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.disk.set_search_index(Some(indexed(&["C:"])));
    r.run(cx, |e, cx| e.set_filter("rep", None, cx));
    let task = r.explorer.update(cx, |e, cx| e.find_as_typed("report", cx));
    cx.foreground_executor().block_test(task);
    assert!(r.run_of(cx).is_none(), "the box says \"rep\" now, not \"report\"");
}

// ---- Saved searches, instant search's offer -------------------------------------------------------------------

#[gpui_kit::test]
fn a_saved_search_opens_its_folder_puts_its_query_in_the_box_and_searches(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    let task = r.explorer.update(cx, |e, cx| {
        e.run_saved_search(
            SavedSearch {
                query: "report".into(),
                folder: "C:\\b".into(),
            },
            cx,
        )
    });
    cx.foreground_executor().block_test(task);
    r.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert_eq!(tab.path, "C:\\b");
        assert_eq!(tab.filter, "report");
    });
    assert_eq!(r.run_of(cx).expect("a search").scope, "C:\\b");
    assert_eq!(r.names(cx), ["report2.pdf"]);

    // A folder that can't be opened: nothing is searched.
    let before = r.disk.finds().len();
    let task = r.explorer.update(cx, |e, cx| {
        e.run_saved_search(
            SavedSearch {
                query: "x".into(),
                folder: "C:\\gone".into(),
            },
            cx,
        )
    });
    cx.foreground_executor().block_test(task);
    assert_eq!(r.disk.finds().len(), before);
}

#[gpui_kit::test]
fn a_saved_search_of_every_drive_runs_from_this_pc(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.disk.set_drives(vec![Drive {
        path: "C:\\".into(),
        label: String::new(),
        kind: DriveKind::Fixed,
        fs: "NTFS".into(),
        total: 1,
        free: 1,
    }]);
    r.run(cx, |e, cx| e.refresh_drives(cx).detach());
    r.go(cx, "C:\\a");
    let task = r.explorer.update(cx, |e, cx| {
        e.run_saved_search(
            SavedSearch {
                query: "report".into(),
                folder: glint_model::paths::THIS_PC.into(),
            },
            cx,
        )
    });
    cx.foreground_executor().block_test(task);
    assert!(r.read(cx, |e| e.active_tab().is_some_and(|t| t.is_this_pc())));
    assert_eq!(r.names(cx), ["report.pdf", "report2.pdf"]);
}

#[gpui_kit::test]
fn a_slow_search_without_the_index_offers_instant_search_once_when_it_could_be_turned_on(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.disk.set_search_index(Some(SearchIndex {
        can_turn_on: true,
        ..Default::default()
    }));
    // A quick one says nothing.
    r.find(cx, "report");
    assert!(r.found_events().is_empty());
    // One that took over a second does.
    let live = r.disk.live_find("slow");
    let waiting = r.start(cx, "slow");
    live.try_send(Found::Started { engine: "fd".into() }).expect("open");
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(1_050));
    live.try_send(finished(0)).expect("open");
    cx.foreground_executor().block_test(waiting);
    cx.run_until_parked();
    assert_eq!(r.found_events(), [FindNotice::OfferInstantSearch]);
}

#[gpui_kit::test]
fn a_slow_search_does_not_offer_instant_search_when_it_is_already_on(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    reports(&r.disk);
    r.go(cx, "C:\\");
    r.disk.set_search_index(Some(indexed(&["D:"])));
    let live = r.disk.live_find("slow");
    let waiting = r.start(cx, "slow");
    live.try_send(Found::Started { engine: "fd".into() }).expect("open");
    cx.run_until_parked();
    std::thread::sleep(Duration::from_millis(1_050));
    live.try_send(finished(0)).expect("open");
    cx.foreground_executor().block_test(waiting);
    cx.run_until_parked();
    assert!(r.found_events().is_empty());
}


// ---- In comments: a plain word also finds what has it in its comment or tags -------------------------------------

mod in_comments {
    use super::*;
    use crate::dialogs::tags::{Backend, Fut};
    use glint_model::groups::GroupBy;
    use glint_model::marks::Marks;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::ThreadId;

    const BOND: &str = "C:\\ops\\bond0.conf";
    const NOTES: &str = "C:\\ops\\lacp-notes.txt";

    /// A marks backend over a map (by path in lower case): it logs each `read_marks` call's paths and the thread
    /// that ran it, and holds its answers back while its gate is shut.
    type Reads = Vec<(Vec<String>, ThreadId)>;

    struct FakeMarks {
        marks: Arc<Mutex<HashMap<String, Marks>>>,
        reads: Arc<Mutex<Reads>>,
        open: Arc<AtomicBool>,
    }

    impl FakeMarks {
        fn new(items: &[(&str, Marks)]) -> Arc<Self> {
            let fake = FakeMarks {
                marks: Arc::default(),
                reads: Arc::default(),
                open: Arc::new(AtomicBool::new(true)),
            };
            for (path, marks) in items {
                fake.put(path, marks.clone());
            }
            Arc::new(fake)
        }

        fn put(&self, path: &str, marks: Marks) {
            self.marks.lock().expect("lock").insert(path.to_lowercase(), marks);
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.reads.lock().expect("lock").iter().map(|(paths, _)| paths.clone()).collect()
        }
    }

    impl Backend for FakeMarks {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
            Box::pin(async move { Ok(vec![Vec::new(); paths.len()]) })
        }

        fn set(&self, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Fut<Vec<(String, String)>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
            let (marks, reads, open) = (self.marks.clone(), self.reads.clone(), self.open.clone());
            Box::pin(async move {
                // Runs where core runs it: never the thread of the window.
                reads.lock().expect("lock").push((paths.clone(), std::thread::current().id()));
                for _ in 0..3000 {
                    if open.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                let known = marks.lock().expect("lock").clone();
                Ok(paths.iter().map(|p| known.get(&p.to_lowercase()).cloned().unwrap_or_default()).collect())
            })
        }
    }

    fn comment(text: &str) -> Marks {
        Marks { comment: Some(text.to_string()), ..Marks::default() }
    }

    /// `C:\ops` with a note named for the word, a file found by its comment, one found inside, and one nothing finds.
    fn marked(cx: &mut TestAppContext, items: &[(&str, Marks)]) -> (Rig, Arc<FakeMarks>) {
        // Core's runtime answers the reads on its own threads.
        cx.executor().allow_parking();
        let r = rig(cx);
        let fake = FakeMarks::new(items);
        let backend: Arc<dyn Backend> = fake.clone();
        cx.update(|cx| crate::dialogs::tags::init_with(cx, backend));
        r.disk.folder("C:\\", vec![folder("C:\\", "ops")]);
        r.disk.folder(
            "C:\\ops",
            vec![
                file("C:\\ops", "bond0.conf", 2, 2),
                file("C:\\ops", "lacp-notes.txt", 1, 1),
                file("C:\\ops", "other.txt", 3, 3),
                file("C:\\ops", "run.sh", 4, 4),
            ],
        );
        r.go(cx, "C:\\ops");
        (r, fake)
    }

    /// Types `query` in the box and searches names (what is inside files is left alone), the search saying what the
    /// test sends it.
    fn search(r: &Rig, cx: &mut TestAppContext, query: &str) -> (async_channel::Sender<Found>, gpui_kit::Task<()>) {
        let live = r.disk.live_find(query);
        let task = r.explorer.update(cx, |e, cx| {
            e.set_filter(query, None, cx);
            e.run_find(query, Wider::default().names_only(), cx)
        });
        cx.run_until_parked();
        (live, task)
    }

    fn settle(cx: &mut TestAppContext, mut done: impl FnMut(&TestAppContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the marks never came");
    }

    /// Lets everything in flight land, for a test of what does not happen.
    fn idle(cx: &mut TestAppContext) {
        for _ in 0..30 {
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(3));
        }
    }

    impl Rig {
        /// The list's lines as words: a header as "# name (n)", a row by its name, a line under it as "  text".
        fn lines(&self, cx: &TestAppContext) -> Vec<String> {
            self.read(cx, |e| {
                let rows = &e.active_tab().expect("a tab").rows;
                (0..rows.line_count())
                    .map(|line| match rows.line(line) {
                        Line::Row(i) => rows.get(i).map(|entry| entry.name.clone()).unwrap_or_default(),
                        Line::Header(_, header) => format!("# {} ({})", header.label, header.count),
                        Line::Hit(at) if at.more > 0 => format!("  +{}", at.more),
                        Line::Hit(at) => match rows.hit(at) {
                            Some((_, hit)) => {
                                let text: String = hit.parts.iter().map(|p| p.text.as_str()).collect();
                                match hit.line {
                                    Some(n) => format!("  {n}: {text}"),
                                    None => format!("  {text}"),
                                }
                            }
                            None => "  ?".to_string(),
                        },
                        Line::Blank => String::new(),
                    })
                    .collect()
            })
        }
    }

    fn batch(by: &[(&str, u8)], files: &[&str]) -> [Found; 2] {
        let marked = by.iter().map(|(path, bits)| (path.to_string(), *bits)).collect();
        let entries = files
            .iter()
            .map(|path| {
                let (dir, name) = path.rsplit_once('\\').expect("a path");
                file(dir, name, 1, 1)
            })
            .collect();
        [Found::Marked(marked), Found::Entries(entries)]
    }

    #[gpui_kit::test]
    fn a_file_found_by_its_comment_is_listed_after_the_names_with_the_line_that_holds_the_word(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (r, fake) = marked(cx, &[(BOND, comment("bond over two NICs\nLACP rate fast\nend"))]);
        let (live, waiting) = search(&r, cx, "lacp");
        for event in batch(&[(BOND, 2)], &[NOTES, BOND]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(2)).expect("open");
        cx.foreground_executor().block_test(waiting);
        settle(cx, |cx| r.lines(cx).len() == 5);
        assert_eq!(
            r.lines(cx),
            ["# Names (1)", "lacp-notes.txt", "# In comments (1)", "bond0.conf", "  LACP rate fast"],
            "the second line of the comment, not the first"
        );
        // The service was asked for marks as the words were typed.
        let call = r.disk.finds().pop().expect("a search was started");
        assert!(call.options.marks);
        assert_eq!(fake.calls(), [vec![BOND.to_string()]]);
    }

    #[gpui_kit::test]
    fn a_file_found_by_a_tag_alone_says_its_tags(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let tagged = Marks { tags: vec!["bond".into(), "lacp".into()], ..Marks::default() };
        let (r, _fake) = marked(cx, &[(BOND, tagged)]);
        let (live, waiting) = search(&r, cx, "lacp");
        for event in batch(&[(BOND, 4)], &[BOND]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(1)).expect("open");
        cx.foreground_executor().block_test(waiting);
        settle(cx, |cx| r.lines(cx).len() == 3);
        assert_eq!(r.lines(cx), ["# In comments (1)", "bond0.conf", "  tags: bond, lacp"].map(String::from));
    }

    #[gpui_kit::test]
    fn names_then_comments_then_inside_files_are_three_parts_in_that_order(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (r, _fake) = marked(cx, &[(BOND, comment("LACP rate fast"))]);
        r.disk.live_inside();
        let live = r.disk.live_find("lacp");
        let waiting = r.explorer.update(cx, |e, cx| {
            e.set_filter("lacp", None, cx);
            e.run_find("lacp", Wider::default(), cx)
        });
        cx.run_until_parked();
        for event in batch(&[(BOND, 2)], &[NOTES, BOND]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(2)).expect("open");
        cx.foreground_executor().block_test(waiting);
        let id = r.disk.inside_searches().last().map(|(_, request)| request.id.clone()).expect("a content search");
        let heard = inside_event(
            &id,
            glint_core::TextSaid::Matches(vec![inside_match("C:\\ops\\run.sh", "start lacp\n", 3, 6, 10)]),
        );
        r.explorer.update(cx, |e, cx| e.inside_heard(&heard, cx));
        settle(cx, |cx| r.lines(cx).len() == 8);
        assert_eq!(
            r.lines(cx),
            [
                "# Names (1)",
                "lacp-notes.txt",
                "# In comments (1)",
                "bond0.conf",
                "  LACP rate fast",
                "# Inside files (1)",
                "run.sh",
                "  3: start lacp"
            ]
            .map(String::from)
        );
    }

    #[gpui_kit::test]
    fn the_row_has_no_line_until_its_marks_are_read_and_none_when_they_no_longer_hold_the_word(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (r, fake) = marked(cx, &[(BOND, comment("LACP rate fast"))]);
        fake.open.store(false, Ordering::SeqCst);
        let (live, waiting) = search(&r, cx, "lacp");
        for event in batch(&[(BOND, 2)], &[BOND]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(1)).expect("open");
        cx.foreground_executor().block_test(waiting);
        settle(cx, |_| !fake.calls().is_empty());
        idle(cx);
        assert_eq!(r.lines(cx), ["# In comments (1)", "bond0.conf"].map(String::from), "no line, no placeholder");
        fake.open.store(true, Ordering::SeqCst);
        settle(cx, |cx| r.lines(cx).len() == 3);
        assert_eq!(r.lines(cx)[2], "  LACP rate fast");

        // Changed since the index saw it: the row stays, with nothing under it.
        fake.put(BOND, comment("something else"));
        let (live, waiting) = search(&r, cx, "lacp ");
        for event in batch(&[(BOND, 2)], &[BOND]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(1)).expect("open");
        cx.foreground_executor().block_test(waiting);
        settle(cx, |_| fake.calls().len() == 2);
        idle(cx);
        assert_eq!(r.lines(cx), ["# In comments (1)", "bond0.conf"].map(String::from));
    }

    #[gpui_kit::test]
    fn only_flagged_paths_are_read_in_one_call_a_batch_off_the_window_thread(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (r, fake) = marked(cx, &[(BOND, comment("LACP")), (NOTES, comment("LACP again"))]);
        // No hit flagged: nothing is read.
        let (live, waiting) = search(&r, cx, "lacp");
        live.try_send(Found::Entries(vec![file("C:\\ops", "lacp-notes.txt", 1, 1)])).expect("open");
        live.try_send(finished(1)).expect("open");
        cx.foreground_executor().block_test(waiting);
        idle(cx);
        assert!(fake.calls().is_empty());

        // Two batches, each with its flagged ones (and a hit by name between them).
        let (live, waiting) = search(&r, cx, "lacp ");
        for event in batch(&[(BOND, 2)], &[BOND, "C:\\ops\\other.txt"]) {
            live.try_send(event).expect("open");
        }
        cx.run_until_parked();
        idle(cx);
        for event in batch(&[(NOTES, 4)], &[NOTES]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(3)).expect("open");
        cx.foreground_executor().block_test(waiting);
        settle(cx, |_| fake.calls().len() == 2);
        assert_eq!(fake.calls(), [vec![BOND.to_string()], vec![NOTES.to_string()]]);
        let here = std::thread::current().id();
        assert!(fake.reads.lock().expect("lock").iter().all(|(_, thread)| *thread != here), "never the UI thread");
    }

    #[gpui_kit::test]
    fn grouped_by_something_else_there_are_no_parts_and_the_line_follows_its_row(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (r, _fake) = marked(cx, &[(BOND, comment("LACP rate fast"))]);
        let (live, waiting) = search(&r, cx, "lacp");
        for event in batch(&[(BOND, 2)], &[NOTES, BOND]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(2)).expect("open");
        cx.foreground_executor().block_test(waiting);
        settle(cx, |cx| r.lines(cx).iter().any(|line| line == "  LACP rate fast"));
        r.explorer.update(cx, |e, cx| e.set_group(GroupBy::Type, cx));
        cx.run_until_parked();
        let lines = r.lines(cx);
        assert!(lines.iter().all(|line| !line.starts_with("# Names") && !line.starts_with("# In comments")), "{lines:?}");
        let at = lines.iter().position(|line| line == "bond0.conf").expect("the file is a row");
        assert_eq!(lines[at + 1], "  LACP rate fast");
    }

    #[gpui_kit::test]
    fn a_comment_in_arabic_matches_and_its_line_is_shown(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (r, _fake) = marked(cx, &[(BOND, comment("إعداد الشبكة\nتجميع الروابط بالبروتوكول lacp\nنهاية"))]);
        let (live, waiting) = search(&r, cx, "الروابط");
        for event in batch(&[(BOND, 2)], &[BOND]) {
            live.try_send(event).expect("open");
        }
        live.try_send(finished(1)).expect("open");
        cx.foreground_executor().block_test(waiting);
        settle(cx, |cx| r.lines(cx).len() == 3);
        assert_eq!(r.lines(cx), ["# In comments (1)", "bond0.conf", "  تجميع الروابط بالبروتوكول lacp"].map(String::from));
        testing::set_rtl(false);
    }
}
