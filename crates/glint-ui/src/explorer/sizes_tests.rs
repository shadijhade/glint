//! Headless tests of the folder sizes: Folder sizes and Stop, the sizes the index gives as a folder opens, an
//! archive's own sizes, sorting and grouping by them, and closing a tab. They run on the fake disk behind the same
//! `Source` the app uses for glint-core, with an Arabic folder name where a name is shown.

use super::actions_tests::{harness, Harness};
use super::fake::*;
use super::*;
use crate::testing;
use glint_model::entries::{Sort, SortKey};
use glint_model::groups::GroupBy;
use gpui_kit::TestAppContext;
use std::time::Duration;

impl Harness {
    fn sizes_events(&self) -> usize {
        self.events()
            .iter()
            .filter(|e| matches!(e, ExplorerEvent::SizesChanged { .. }))
            .count()
    }
}

#[gpui_kit::test]
fn folder_sizes_measures_each_subfolder_and_the_total(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    assert!(h.read(cx, |e| e.size_run("C:\\a").is_none() && !e.is_sizing()));
    h.run(cx, |e, cx| e.calc_sizes(cx));

    assert_eq!(
        h.disk.size_calls(),
        [SizeCall {
            path: "C:\\a".into(),
            lane: "tab1".into(),
            index_only: false
        }]
    );
    h.read(cx, |e| {
        let run = e
            .size_run("c:\\A\\")
            .expect("sized, whatever the case or the separator");
        assert!(!run.running);
        assert_eq!(run.engine, "pdu");
        assert_eq!(run.total, 150 + 5000 + 7 + 10 + 20 + 30 + 40);
        assert_eq!(run.size_of("sub"), Some(150), "a folder's folders are counted");
        assert_eq!(e.folder_size("C:\\a", "big"), Some(5000));
        assert_eq!(e.folder_size("C:\\a", "تقارير"), Some(7));
        assert_eq!(
            e.folder_size("C:\\a", "one.txt"),
            None,
            "only folders have a measured size"
        );
        assert!(!e.is_sizing());
        let line = run.sized().expect("done");
        assert_eq!((line.engine, line.total), ("pdu", 5257));
    });
    assert!(h.sizes_events() >= 2, "started and done were both told");
    assert!(h.said().is_empty());
}

#[gpui_kit::test]
fn a_slow_measuring_shows_as_running_and_stop_ends_it_leaving_nothing_said(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let gate = h.disk.hold_sizes();
    h.run(cx, |e, cx| e.calc_sizes(cx));
    h.read(cx, |e| {
        assert!(e.is_sizing(), "the command bar says Stop");
        let run = e.size_run("C:\\a").expect("a run is on file while it runs");
        assert!(run.running && run.sizes.is_empty() && run.sized().is_none());
    });

    h.run(cx, |e, _| e.stop_sizes());
    assert_eq!(h.disk.sizes_stopped(), ["tab1"]);
    gate.close();
    cx.run_until_parked();
    h.read(cx, |e| {
        assert!(e.size_run("C:\\a").is_none(), "nothing was there before, so nothing is");
        assert!(!e.is_sizing());
    });
    assert!(h.said().is_empty(), "stopping is not a failure");
}

#[gpui_kit::test]
fn a_stopped_remeasuring_keeps_the_sizes_it_had_and_a_failure_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.calc_sizes(cx));
    let version = h.read(cx, |e| e.size_run("C:\\a").map(|r| r.version));

    h.disk.fail_action("sizes", "pdu isn't there");
    h.run(cx, |e, cx| e.calc_sizes(cx));
    h.read(cx, |e| {
        let run = e.size_run("C:\\a").expect("the earlier sizes stay");
        assert!(!run.running);
        assert_eq!(run.size_of("big"), Some(5000));
        assert_eq!(Some(run.version), version, "the sizes are the same ones");
    });
    assert_eq!(
        h.said(),
        [ActionNotice::Failed {
            what: ActionFailure::FolderSizes,
            message: "pdu isn't there".into()
        }]
    );
}

#[gpui_kit::test]
fn a_folder_has_no_sizes_where_it_is_not_a_folder_on_disk(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.run(cx, |e, cx| e.calc_sizes(cx));
    assert!(h.disk.size_calls().is_empty(), "This PC has no subfolders to measure");
    assert!(h.go(cx, glint_model::paths::RECYCLE_BIN));
    h.run(cx, |e, cx| e.calc_sizes(cx));
    assert!(h.disk.size_calls().is_empty());
}

#[gpui_kit::test]
fn a_folder_the_index_has_shows_its_sizes_as_it_opens_and_after_each_refresh(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.set_indexed("C:\\a");
    // The index is not known to be ready: nothing is asked.
    assert!(h.go(cx, "C:\\a"));
    assert!(h.disk.size_calls().is_empty());

    h.run(cx, |e, cx| {
        e.set_search_index(
            Some(SearchIndex {
                on: true,
                ready: vec!["C:".into()],
                ..Default::default()
            }),
            cx,
        )
    });
    // The drive just became ready: the folder on screen shows its sizes at once, from the index alone.
    assert_eq!(
        h.disk.size_calls(),
        [SizeCall {
            path: "C:\\a".into(),
            lane: "tab1".into(),
            index_only: true
        }]
    );
    h.read(cx, |e| {
        let run = e.size_run("C:\\a").expect("sized");
        assert_eq!((run.engine.as_str(), run.running), ("index", false));
    });

    // Another folder on the drive: sized as it opens, and again after a refresh.
    h.disk.set_indexed("C:\\b");
    assert!(h.go(cx, "C:\\b"));
    h.task(cx, |e, cx| e.reload(None, cx));
    let asked: Vec<String> = h.disk.size_calls().into_iter().map(|c| c.path).collect();
    assert_eq!(asked, ["C:\\a", "C:\\b", "C:\\b"]);
    assert!(h.said().is_empty());
}

#[gpui_kit::test]
fn an_index_that_cannot_answer_is_not_a_failure_and_shows_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.run(cx, |e, cx| {
        e.set_search_index(
            Some(SearchIndex {
                on: true,
                ready: vec!["C:".into()],
                ..Default::default()
            }),
            cx,
        )
    });
    // The drive is ready but the index has no such folder (not marked on the fake).
    assert!(h.go(cx, "C:\\a"));
    assert_eq!(h.disk.size_calls().len(), 1);
    assert!(h.read(cx, |e| e.size_run("C:\\a").is_none()));
    assert!(h.said().is_empty());
}

#[gpui_kit::test]
fn a_folder_in_an_archive_has_its_sizes_from_the_listing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let outer = "C:\\a\\أرشيف.zip";
    let mut docs = folder(outer, "docs");
    docs.size = 900;
    h.disk
        .archive_folder(outer, outer, vec![docs, file(outer, "inner.txt", 100, 1)]);
    assert!(h.go(cx, outer));
    h.read(cx, |e| {
        let run = e.size_run(outer).expect("made from the listing");
        assert_eq!(
            (run.engine.as_str(), run.total, run.running, run.elapsed),
            ("zip", 1000, false, 0.0)
        );
        assert_eq!(e.folder_size(outer, "docs"), Some(900));
    });
    assert!(h.disk.size_calls().is_empty(), "nothing is measured for it");
}

#[gpui_kit::test]
fn sorting_by_size_orders_folders_by_what_was_measured_and_unmeasured_ones_come_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| {
        e.set_sort(
            Sort {
                key: SortKey::Size,
                desc: false,
            },
            cx,
        )
    });
    // Nothing measured: every folder is alike and keeps the engine's name order.
    assert_eq!(h.names(cx)[..3], ["big", "sub", "تقارير"]);
    h.run(cx, |e, cx| e.calc_sizes(cx));
    assert_eq!(
        h.names(cx),
        ["تقارير", "sub", "big", "one.txt", "تقرير.pdf", "pic.jpg", "أرشيف.zip"],
        "folders by their measured size (7, 150, 5000), then files by theirs"
    );
    h.run(cx, |e, cx| {
        e.set_sort(
            Sort {
                key: SortKey::Size,
                desc: true,
            },
            cx,
        )
    });
    assert_eq!(h.names(cx)[..3], ["big", "sub", "تقارير"]);
}

#[gpui_kit::test]
fn grouping_by_size_puts_a_measured_folder_in_its_size_group(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.disk.set_measured(
        "C:\\a",
        Measured {
            total: 200 << 20,
            sizes: vec![("big".into(), 200 << 20), ("sub".into(), 10), ("تقارير".into(), 0)],
            elapsed: 3.0,
            engine: "pdu".into(),
        },
    );
    h.run(cx, |e, cx| e.set_group(GroupBy::Size, cx));
    let headers = |h: &Harness, cx: &TestAppContext| -> Vec<String> {
        h.read(cx, |e| {
            e.active_tab()
                .and_then(|t| {
                    t.rows
                        .groups()
                        .map(|g| g.headers.iter().map(|x| x.label.clone()).collect())
                })
                .unwrap_or_default()
        })
    };
    assert!(
        headers(&h, cx).contains(&"Folders".to_string()),
        "unmeasured folders are plain folders"
    );
    h.run(cx, |e, cx| e.calc_sizes(cx));
    let after = headers(&h, cx);
    assert!(after.iter().any(|l| l.starts_with("Large")), "{after:?}");
    assert!(
        after.contains(&"Empty".to_string()),
        "{after:?}: the Arabic folder measured 0 bytes"
    );
}

#[gpui_kit::test]
fn closing_a_tab_stops_what_it_was_measuring(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    let second = h.read(cx, |e| e.active_tab_id());
    assert!(h.go(cx, "C:\\a"));
    let _gate = h.disk.hold_sizes();
    h.run(cx, |e, cx| e.calc_sizes(cx));
    assert_eq!(h.disk.size_calls()[0].lane, second.to_string());
    h.run(cx, |e, cx| e.close_tab(None, cx));
    assert_eq!(h.disk.sizes_stopped(), [second.to_string()]);
}

#[gpui_kit::test]
fn a_run_that_was_replaced_by_a_newer_one_does_not_undo_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    assert!(h.go(cx, "C:\\a"));
    let gate = h.disk.hold_sizes();
    h.run(cx, |e, cx| e.calc_sizes(cx));
    // Core ends the first run when a second starts in the same tab: the first comes back "cancelled".
    h.run(cx, |e, cx| e.calc_sizes(cx));
    h.run(cx, |e, _| e.stop_sizes());
    gate.close();
    cx.executor().advance_clock(Duration::from_millis(10));
    cx.run_until_parked();
    h.read(cx, |e| assert!(!e.is_sizing()));
    assert!(h.said().is_empty());
}
