//! Tests of compare panes: the pure comparison, then the explorer with two panes on a fake disk: the answer for
//! each pane, "diff only", and everything that makes them go away. Arabic names where a name is shown.

use super::actions_tests::{harness, Harness};
use super::compare::compare_listings;
use super::fake::*;
use super::*;
use crate::testing;
use glint_model::entries::DiffStatus;
use gpui_kit::TestAppContext;
use std::collections::HashMap;
use std::sync::Arc;

const L: &str = "C:\\left";
const R: &str = "C:\\right";

#[test]
fn items_are_only_here_newer_older_different_or_the_same() {
    let own = vec![
        folder(L, "docs"),
        file(L, "same.txt", 10, 100_000),
        file(L, "newer.txt", 10, 200_000),
        file(L, "older.txt", 10, 100_000),
        file(L, "bigger.txt", 99, 100_000),
        file(L, "lonely.txt", 1, 1),
        file(L, "تقرير.txt", 5, 5_000),
    ];
    let other = vec![
        folder(R, "docs"),
        file(R, "SAME.txt", 10, 100_000),
        file(R, "newer.txt", 10, 100_000),
        file(R, "older.txt", 10, 200_000),
        file(R, "bigger.txt", 10, 100_000),
        file(R, "تقرير.txt", 5, 5_000),
    ];
    let c = compare_listings(&own, &other, false, None, None);
    let want = |name: &str| c.diffs[&name.to_lowercase()];
    assert_eq!(want("docs"), DiffStatus::Same, "folders without sizes are the same");
    assert_eq!(want("same.txt"), DiffStatus::Same, "names are compared in lower case");
    assert_eq!(want("newer.txt"), DiffStatus::Newer);
    assert_eq!(want("older.txt"), DiffStatus::Older);
    assert_eq!(want("bigger.txt"), DiffStatus::Differ);
    assert_eq!(want("lonely.txt"), DiffStatus::Unique);
    assert_eq!(want("تقرير.txt"), DiffStatus::Same);
    assert_eq!((c.unique, c.newer, c.differ, c.older, c.same), (1, 1, 1, 1, 3));
    assert_eq!(c.differences(), 4);
    // The differences in listing order; only here and newer are what a sync would copy.
    assert_eq!(
        c.diff_paths,
        [
            "C:\\left\\newer.txt",
            "C:\\left\\older.txt",
            "C:\\left\\bigger.txt",
            "C:\\left\\lonely.txt"
        ]
    );
    assert_eq!(c.sync_paths, ["C:\\left\\newer.txt", "C:\\left\\lonely.txt"]);
}

#[test]
fn two_seconds_of_clock_rounding_is_the_same_and_a_file_is_not_a_folder() {
    let own = vec![file(L, "a.txt", 10, 100_000), folder(L, "x"), file(L, "y", 1, 1)];
    let other = vec![file(R, "a.txt", 10, 102_000), file(R, "x", 1, 1), folder(R, "y")];
    let c = compare_listings(&own, &other, false, None, None);
    assert_eq!(c.diffs["a.txt"], DiffStatus::Same, "2000 ms apart and the same size");
    assert_eq!(c.diffs["x"], DiffStatus::Unique, "a folder against a file");
    assert_eq!(c.diffs["y"], DiffStatus::Unique);
    let later = vec![file(R, "a.txt", 10, 102_001)];
    assert_eq!(
        compare_listings(&own, &later, false, None, None).diffs["a.txt"],
        DiffStatus::Older
    );
    // A date the folder doesn't know (0) doesn't decide; the sizes do.
    let unknown = vec![file(L, "u", 1, 0)];
    let unknown_other = vec![file(R, "u", 2, 500_000)];
    assert_eq!(
        compare_listings(&unknown, &unknown_other, false, None, None).diffs["u"],
        DiffStatus::Differ
    );
}

#[test]
fn folders_differ_only_when_both_are_measured_and_their_sizes_are_not_the_same() {
    let own = vec![folder(L, "a"), folder(L, "b"), folder(L, "c")];
    let other = vec![folder(R, "a"), folder(R, "b"), folder(R, "c")];
    let ours: HashMap<String, u64> = [("a".into(), 5), ("b".into(), 5)].into();
    let theirs: HashMap<String, u64> = [("a".into(), 6), ("b".into(), 5)].into();
    let c = compare_listings(&own, &other, false, Some(&ours), Some(&theirs));
    assert_eq!(c.diffs["a"], DiffStatus::Differ);
    assert_eq!(c.diffs["b"], DiffStatus::Same);
    assert_eq!(c.diffs["c"], DiffStatus::Same, "measured on neither side");
    let c = compare_listings(&own, &other, false, Some(&ours), None);
    assert_eq!(c.diffs["a"], DiffStatus::Same, "measured only here");
}

#[test]
fn hidden_items_are_compared_only_while_they_are_shown() {
    let own = vec![hidden(L, ".env"), file(L, "a", 1, 1)];
    let other = vec![file(R, "a", 1, 1)];
    let c = compare_listings(&own, &other, false, None, None);
    assert!(!c.diffs.contains_key(".env"), "hidden and not shown");
    assert_eq!(c.same, 1);
    let c = compare_listings(&own, &other, true, None, None);
    assert_eq!(c.diffs[".env"], DiffStatus::Unique);
    // A hidden namesake doesn't count while hidden items are hidden.
    let own = vec![file(L, ".env", 1, 1)];
    let mut shy = hidden(R, ".env");
    (shy.size, shy.modified) = (1, 1);
    let other = vec![shy];
    assert_eq!(
        compare_listings(&own, &other, false, None, None).diffs[".env"],
        DiffStatus::Unique
    );
    assert_eq!(
        compare_listings(&own, &other, true, None, None).diffs[".env"],
        DiffStatus::Same
    );
}

/// Two folders side by side: `left` has an Arabic-named file the right lacks, one newer, one the same, and a
/// hidden one.
fn two_folders(h: &Harness, cx: &mut TestAppContext) {
    h.disk.folder(
        L,
        vec![
            file(L, "newer.txt", 10, 200_000),
            file(L, "same.txt", 10, 100_000),
            file(L, "تقرير.txt", 5, 5_000),
            hidden(L, ".hidden"),
        ],
    );
    h.disk.folder(
        R,
        vec![
            file(R, "extra.txt", 7, 7_000),
            file(R, "newer.txt", 10, 100_000),
            file(R, "same.txt", 10, 100_000),
        ],
    );
    assert!(h.go(cx, L));
}

fn names(h: &Harness, cx: &TestAppContext, pane: usize) -> Vec<String> {
    h.read(cx, |e| {
        e.pane_tab(pane)
            .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
            .unwrap_or_default()
    })
}

fn answer(h: &Harness, cx: &TestAppContext, pane: usize) -> Option<Arc<Comparison>> {
    h.read(cx, |e| e.comparison_of(e.pane_tab_id(pane)))
}

#[gpui_kit::test]
fn ctrl_shift_m_opens_a_second_pane_and_marks_both_panes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    assert!(answer(&h, cx, 0).is_none(), "nothing is compared in one pane");
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    h.read(cx, |e| assert!(e.is_dual() && e.compare_panes && !e.diff_only));
    // The new pane starts on the folder you're in: nothing is compared with itself.
    assert!(answer(&h, cx, 0).is_none(), "the same folder in both panes");
    assert!(h.go(cx, R));

    let right = answer(&h, cx, 1).expect("the right pane is compared with the left");
    assert_eq!(right.diffs["extra.txt"], DiffStatus::Unique);
    assert_eq!(right.diffs["newer.txt"], DiffStatus::Older);
    assert_eq!(right.diffs["same.txt"], DiffStatus::Same);
    let left = answer(&h, cx, 0).expect("and the other way round");
    assert_eq!(left.diffs["newer.txt"], DiffStatus::Newer);
    assert_eq!(left.diffs["تقرير.txt"], DiffStatus::Unique);
    assert_eq!(left.diffs.len(), 3, "the hidden item is left out: {:?}", left.diffs);
    assert_eq!(left.sync_paths, ["C:\\left\\newer.txt", "C:\\left\\تقرير.txt"]);
    // Every item is still in the rows.
    assert_eq!(names(&h, cx, 0).len(), 3);
}

#[gpui_kit::test]
fn the_flag_toggles_in_two_panes_and_takes_the_marks_with_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_dual(cx));
    assert!(h.go(cx, R));
    assert!(answer(&h, cx, 1).is_none(), "off until asked");
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(answer(&h, cx, 1).is_some());
    h.run(cx, |e, cx| e.toggle_diff_only(cx));
    assert_eq!(names(&h, cx, 1).len(), 2);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    h.read(cx, |e| {
        assert!(!e.compare_panes && !e.diff_only, "turning it off shows everything")
    });
    assert!(answer(&h, cx, 1).is_none());
    assert_eq!(names(&h, cx, 1).len(), 3);
    // On again: starts with every item shown.
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    h.read(cx, |e| assert!(e.compare_panes && !e.diff_only));
    assert!(answer(&h, cx, 1).is_some());
}

#[gpui_kit::test]
fn diff_only_leaves_out_the_identical_items_and_says_so_when_nothing_is_left(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    assert_eq!(names(&h, cx, 1), ["extra.txt", "newer.txt", "same.txt"]);
    h.run(cx, |e, cx| e.toggle_diff_only(cx));
    assert_eq!(
        names(&h, cx, 1),
        ["extra.txt", "newer.txt"],
        "same.txt matches the left pane"
    );
    assert_eq!(names(&h, cx, 0), ["newer.txt", "تقرير.txt"]);
    // The box's text narrows what is left.
    h.run(cx, |e, cx| e.set_filter("new", Some(e.pane_tab_id(1)), cx));
    assert_eq!(names(&h, cx, 1), ["newer.txt"]);
    h.run(cx, |e, cx| e.set_filter("", Some(e.pane_tab_id(1)), cx));
    // Back to every item.
    h.run(cx, |e, cx| e.toggle_diff_only(cx));
    assert_eq!(names(&h, cx, 1).len(), 3);

    // A folder that matches the other pane entirely: no rows, and the answer says how many match.
    h.disk.folder(
        "C:\\twin",
        vec![
            file("C:\\twin", "extra.txt", 7, 7_000),
            file("C:\\twin", "newer.txt", 10, 100_000),
            file("C:\\twin", "same.txt", 10, 100_000),
        ],
    );
    h.run(cx, |e, cx| e.toggle_diff_only(cx));
    h.run(cx, |e, cx| e.focus_pane(0, cx));
    assert!(h.go(cx, "C:\\twin"));
    assert!(names(&h, cx, 0).is_empty());
    assert!(names(&h, cx, 1).is_empty());
    let c = answer(&h, cx, 0).expect("still compared");
    assert_eq!((c.same, c.differences()), (3, 0));
}

#[gpui_kit::test]
fn diff_only_follows_a_folder_that_changes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    h.run(cx, |e, cx| e.toggle_diff_only(cx));
    assert_eq!(names(&h, cx, 0), ["newer.txt", "تقرير.txt"]);
    // The other pane's folder gets what was missing: the Arabic file matches now.
    h.disk.folder(
        R,
        vec![
            file(R, "newer.txt", 10, 100_000),
            file(R, "same.txt", 10, 100_000),
            file(R, "تقرير.txt", 5, 5_000),
        ],
    );
    let right = h.read(cx, |e| e.pane_tab_id(1));
    h.task(cx, |e, cx| e.reload(Some(right), cx));
    assert_eq!(names(&h, cx, 0), ["newer.txt"]);
    assert_eq!(names(&h, cx, 1), ["newer.txt"]);
}

#[gpui_kit::test]
fn leaving_two_panes_ends_the_comparison_and_the_same_folder_twice_has_none(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    h.run(cx, |e, cx| e.toggle_diff_only(cx));
    assert_eq!(names(&h, cx, 1).len(), 2);
    h.run(cx, |e, cx| e.toggle_dual(cx));
    h.read(cx, |e| assert!(!e.compare_panes && !e.diff_only));
    assert!(answer(&h, cx, 0).is_none());
    assert_eq!(names(&h, cx, 0).len(), 3, "the left pane has all of its items again");
    // Both panes on one folder: nothing to compare.
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    h.read(cx, |e| assert!(e.is_dual() && e.compare_panes));
    assert!(answer(&h, cx, 0).is_some(), "the panes are on two folders again");
    h.run(cx, |e, cx| e.focus_pane(0, cx));
    assert!(h.go(cx, R));
    assert!(answer(&h, cx, 0).is_none(), "the same folder in both panes");
    assert!(answer(&h, cx, 1).is_none());
}

#[gpui_kit::test]
fn showing_hidden_items_changes_what_is_compared(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    assert!(!answer(&h, cx, 0).expect("left").diffs.contains_key(".hidden"));
    cx.update(|cx| crate::state::update(cx, |settings| settings.show_hidden = true));
    cx.run_until_parked();
    assert_eq!(
        answer(&h, cx, 0).expect("left").diffs.get(".hidden"),
        Some(&DiffStatus::Unique)
    );
}

#[gpui_kit::test]
fn comparing_with_a_previous_version_puts_it_on_the_right_and_compares(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.task(cx, |e, cx| e.compare_with_version(L, R, cx));
    h.read(cx, |e| {
        assert!(e.is_dual() && e.compare_panes && !e.diff_only);
        assert_eq!(e.active_pane(), 1);
    });
    assert_eq!(
        h.read(cx, |e| e.pane_tab(0).map(|t| t.path.clone())).as_deref(),
        Some(L)
    );
    assert_eq!(
        h.read(cx, |e| e.pane_tab(1).map(|t| t.path.clone())).as_deref(),
        Some(R)
    );
    assert!(answer(&h, cx, 1).is_some());
}
