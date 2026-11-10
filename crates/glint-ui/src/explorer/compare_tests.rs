//! Tests of compare panes: the pure comparison, then the explorer with two panes on a fake disk: the answer for
//! each pane, "diff only", and everything that makes them go away. Arabic names where a name is shown.

use super::actions_tests::{harness, Harness};
use super::compare::{compare_listings, DeepAnswer, DeepCore, DeepRequest};
use super::fake::*;
use super::*;
use crate::testing;
use glint_model::entries::DiffStatus;
use gpui_kit::TestAppContext;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
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

/// A test that changes a setting makes the explorer hand the Gallery's folders to glint-core, which keeps the
/// app handle of the last explorer to do so and drops the one before it. Dropping that handle wakes the earlier
/// test's tasks on this test's thread, which the test scheduler refuses (and the whole run aborts). The folders
/// are the same (none) in these tests, so say they were sent: nothing is handed over, nothing is dropped.
fn quiet_gallery(h: &Harness, cx: &mut TestAppContext) {
    h.run(cx, |e, _| e.gallery_sent = Some(Vec::new()));
}

/// Two folders side by side: `left` has an Arabic-named file the right lacks, one newer, one the same, and a
/// hidden one.
fn two_folders(h: &Harness, cx: &mut TestAppContext) {
    quiet_gallery(h, cx);
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

// ---- Compared deeper -----------------------------------------------------------------------------------------------

/// Where the test answers an ask.
type Reply = async_channel::Sender<Result<DeepAnswer, String>>;

/// Core, as the deeper comparison sees it: every ask is written down and waits for the test to answer it; stops
/// are counted.
#[derive(Clone, Default)]
struct FakeDeep {
    asked: Rc<RefCell<Vec<DeepRequest>>>,
    waiting: Rc<RefCell<Vec<Option<Reply>>>>,
    cancels: Rc<Cell<usize>>,
}

impl FakeDeep {
    fn core(&self) -> DeepCore {
        let (asked, waiting, cancels) = (self.asked.clone(), self.waiting.clone(), self.cancels.clone());
        DeepCore {
            compare: Rc::new(move |request| {
                asked.borrow_mut().push(request);
                let (sender, receiver) = async_channel::bounded(1);
                waiting.borrow_mut().push(Some(sender));
                Box::pin(async move { receiver.recv().await.unwrap_or_else(|_| Err("cancelled".to_string())) })
            }),
            cancel: Rc::new(move || cancels.set(cancels.get() + 1)),
        }
    }

    /// Answers the `index`th ask (from 0); an ask nobody waits for any more is no one's to answer.
    fn answer(&self, index: usize, answer: Result<DeepAnswer, String>) {
        if let Some(sender) = self.waiting.borrow_mut().get_mut(index).and_then(Option::take) {
            sender.try_send(answer).ok();
        }
    }

    fn asks(&self) -> usize {
        self.asked.borrow().len()
    }
}

fn copy_of(path: &str, folder: &str) -> MoveItem {
    MoveItem {
        path: path.into(),
        folder: folder.into(),
        name: glint_model::paths::base_name(path).to_string(),
    }
}

fn words(list: &[(&str, DiffStatus)]) -> Arc<HashMap<String, DiffStatus>> {
    Arc::new(list.iter().map(|(name, status)| (name.to_string(), *status)).collect())
}

/// What core says of [`two_folders`] compared deeper: `same.txt` has other contents, the Arabic file is only on
/// the left, and Sync takes the Arabic file and a file inside a subfolder to the right.
fn answer_for_two_folders() -> DeepAnswer {
    DeepAnswer {
        left: words(&[("same.txt", DiffStatus::Differ), ("تقرير.txt", DiffStatus::Unique)]),
        right: words(&[("same.txt", DiffStatus::Differ), ("extra.txt", DiffStatus::Unique)]),
        to_right: Arc::from(vec![
            copy_of("C:\\left\\تقرير.txt", R),
            copy_of("C:\\left\\sub\\deep.txt", "C:\\right\\sub"),
        ]),
        to_left: Arc::from(vec![copy_of("C:\\right\\extra.txt", L)]),
    }
}

fn options(cx: &mut TestAppContext, subfolders: bool, contents: bool) {
    cx.update(|cx| {
        crate::state::update(cx, |settings| {
            settings.compare_subfolders = subfolders;
            settings.compare_contents = contents;
        })
    });
    cx.run_until_parked();
}

/// Two compared folders, with the fake core in place and no option on yet.
fn compared(h: &Harness, cx: &mut TestAppContext) -> FakeDeep {
    two_folders(h, cx);
    let fake = FakeDeep::default();
    h.run(cx, |e, _| e.set_deep_core(fake.core()));
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    options(cx, false, false);
    fake
}

fn operations(h: &Harness) -> Vec<OpNotice> {
    h.events()
        .into_iter()
        .filter_map(|e| match e {
            ExplorerEvent::Operation(n) => Some(n),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn the_deeper_comparison_is_asked_only_while_subfolders_or_contents_is_on(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let fake = compared(&h, cx);
    assert_eq!(fake.asks(), 0, "both options are off");
    assert!(!h.read(cx, |e| e.deep_running()));

    options(cx, true, false);
    assert_eq!(fake.asks(), 1);
    let exclusions = cx.update(|cx| crate::state::settings(cx).find_exclusions.clone());
    assert_eq!(
        fake.asked.borrow()[0],
        DeepRequest {
            left: L.into(),
            right: R.into(),
            subfolders: true,
            contents: false,
            hidden: false,
            exclude: exclusions,
        },
        "the left pane's folder first, with the options that change the answer"
    );
    assert!(h.read(cx, |e| e.deep_running()), "waiting for core");

    // Contents alone asks too (and is another ask: the key has changed).
    options(cx, false, true);
    assert_eq!(fake.asks(), 2);
    assert!(fake.asked.borrow()[1].contents && !fake.asked.borrow()[1].subfolders);
}

#[gpui_kit::test]
fn turning_the_options_off_or_the_comparison_off_cancels_what_core_is_doing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let fake = compared(&h, cx);
    options(cx, true, false);
    assert_eq!((fake.asks(), fake.cancels.get()), (1, 0));

    options(cx, false, false);
    assert_eq!(fake.cancels.get(), 1, "it was running: core is told to stop");
    assert!(!h.read(cx, |e| e.deep_running()));
    assert_eq!(h.read(cx, |e| e.deep_error().to_string()), "");

    // Asked again, answered, and then the panes stop being compared: nothing runs, so nothing is cancelled, and
    // the answer goes.
    options(cx, true, false);
    fake.answer(1, Ok(answer_for_two_folders()));
    cx.run_until_parked();
    assert!(h.read(cx, |e| e.deep_sync_items(0).is_some()));
    let before = fake.cancels.get();
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert_eq!(fake.cancels.get(), before);
    assert!(h.read(cx, |e| e.deep_sync_items(0).is_none() && !e.deep_running()));

    // Stopped while running: cancelled.
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert_eq!(fake.asks(), 3);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert_eq!(fake.cancels.get(), before + 1);
}

#[gpui_kit::test]
fn it_is_not_asked_for_the_same_folder_in_both_panes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let fake = compared(&h, cx);
    options(cx, true, false);
    assert_eq!(fake.asks(), 1);
    // The right pane goes to the left pane's folder: nothing to compare, so the ask is withdrawn.
    assert!(h.go(cx, L));
    assert_eq!(fake.cancels.get(), 1);
    assert!(!h.read(cx, |e| e.deep_running()));
    assert_eq!(fake.asks(), 1);
    // And back to another folder: asked again.
    assert!(h.go(cx, R));
    assert_eq!(fake.asks(), 2);
}

#[gpui_kit::test]
fn a_newer_ask_stops_the_older_one_and_its_answer_is_dropped(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let fake = compared(&h, cx);
    options(cx, true, false);
    options(cx, true, true);
    assert_eq!(
        (fake.asks(), fake.cancels.get()),
        (2, 1),
        "the first was running when the second came"
    );

    // The first one's answer comes late: it is nobody's now.
    fake.answer(0, Ok(answer_for_two_folders()));
    cx.run_until_parked();
    assert!(h.read(cx, |e| e.deep_running() && e.deep_sync_items(0).is_none()));
    assert_eq!(answer(&h, cx, 0).expect("left").diffs["same.txt"], DiffStatus::Same);

    fake.answer(1, Ok(answer_for_two_folders()));
    cx.run_until_parked();
    assert!(h.read(cx, |e| !e.deep_running() && e.deep_sync_items(0).is_some()));
}

#[gpui_kit::test]
fn the_last_answer_stays_while_it_is_worked_out_again_for_the_same_folders(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let fake = compared(&h, cx);
    options(cx, true, false);
    fake.answer(0, Ok(answer_for_two_folders()));
    cx.run_until_parked();
    assert_eq!(answer(&h, cx, 0).expect("left").diffs["same.txt"], DiffStatus::Differ);

    // The right folder changes on disk: asked again for the same folders and options.
    h.disk.folder(
        R,
        vec![
            file(R, "extra.txt", 7, 7_000),
            file(R, "newer.txt", 10, 100_000),
            file(R, "same.txt", 10, 100_000),
            file(R, "more.txt", 1, 1),
        ],
    );
    let right = h.read(cx, |e| e.pane_tab_id(1));
    h.task(cx, |e, cx| e.reload(Some(right), cx));
    assert_eq!(fake.asks(), 2);
    assert!(h.read(cx, |e| e.deep_running()));
    assert!(
        h.read(cx, |e| e.deep_sync_items(0).is_some()),
        "the answer before is still there"
    );
    assert_eq!(answer(&h, cx, 0).expect("left").diffs["same.txt"], DiffStatus::Differ);

    // The new answer replaces it.
    let mut again = answer_for_two_folders();
    again.left = words(&[]);
    fake.answer(1, Ok(again));
    cx.run_until_parked();
    assert_eq!(answer(&h, cx, 0).expect("left").diffs["same.txt"], DiffStatus::Same);

    // Other options are another question: the old answer is gone at once, before the new one is in.
    options(cx, true, true);
    assert!(h.read(cx, |e| e.deep_running() && e.deep_sync_items(0).is_none()));
}

#[gpui_kit::test]
fn the_rows_and_sync_come_from_the_deeper_answer(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let fake = compared(&h, cx);
    options(cx, true, false);
    fake.answer(0, Ok(answer_for_two_folders()));
    cx.run_until_parked();

    // What core knows is as it says; the rest is compared by names, dates and sizes.
    let left = answer(&h, cx, 0).expect("left");
    assert_eq!(left.diffs["same.txt"], DiffStatus::Differ);
    assert_eq!(left.diffs["newer.txt"], DiffStatus::Newer, "not in core's answer");
    assert_eq!(left.diffs["تقرير.txt"], DiffStatus::Unique);
    assert_eq!(
        (left.unique, left.newer, left.differ, left.older, left.same),
        (1, 1, 1, 0, 0)
    );
    assert_eq!(left.diff_paths.len(), 3);
    let right = answer(&h, cx, 1).expect("right");
    assert_eq!(right.diffs["same.txt"], DiffStatus::Differ);
    assert_eq!(right.diffs["extra.txt"], DiffStatus::Unique);
    assert_eq!(right.diffs["newer.txt"], DiffStatus::Older);

    // "Only the differences" now keeps same.txt.
    h.run(cx, |e, cx| e.toggle_diff_only(cx));
    assert_eq!(names(&h, cx, 0), ["newer.txt", "same.txt", "تقرير.txt"]);

    // Sync is what core said, each into its folder (Windows' file engine makes the folders as needed).
    let to_right = h.read(cx, |e| e.deep_sync_items(0)).expect("the left pane's sync");
    assert_eq!(
        to_right
            .iter()
            .map(|i| (i.path.as_str(), i.folder.as_str(), i.name.as_str()))
            .collect::<Vec<_>>(),
        [
            ("C:\\left\\تقرير.txt", R, "تقرير.txt"),
            ("C:\\left\\sub\\deep.txt", "C:\\right\\sub", "deep.txt")
        ]
    );
    let to_left = h.read(cx, |e| e.deep_sync_items(1)).expect("the right pane's sync");
    assert_eq!(to_left.len(), 1);
    assert_eq!(to_left[0].folder, L);
}

#[gpui_kit::test]
fn an_error_is_kept_to_be_said_but_cancelled_never_is(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let fake = compared(&h, cx);
    options(cx, true, false);
    fake.answer(0, Err("cancelled".to_string()));
    cx.run_until_parked();
    assert_eq!(
        h.read(cx, |e| (e.deep_error().to_string(), e.deep_running())),
        (String::new(), false)
    );

    options(cx, false, true);
    fake.answer(1, Err("C:\\left: Access is denied".to_string()));
    cx.run_until_parked();
    assert_eq!(h.read(cx, |e| e.deep_error().to_string()), "C:\\left: Access is denied");
    assert!(h.read(cx, |e| !e.deep_running() && e.deep_sync_items(0).is_none()));
    // The rows are compared as usual, and asking again clears the error.
    assert_eq!(answer(&h, cx, 0).expect("left").diffs["same.txt"], DiffStatus::Same);
    options(cx, true, true);
    assert_eq!(h.read(cx, |e| e.deep_error().to_string()), "");
}

#[gpui_kit::test]
fn select_differences_selects_every_item_that_is_not_the_same_and_focuses_the_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    h.run(cx, |e, cx| e.select_differences(0, cx));
    h.read(cx, |e| {
        let tab = e.pane_tab(0).expect("left");
        assert_eq!(tab.selection.to_vec(), ["C:\\left\\newer.txt", "C:\\left\\تقرير.txt"]);
        assert_eq!(tab.selection.focus(), Some("C:\\left\\newer.txt"));
        assert_eq!(tab.selection.anchor(), Some("C:\\left\\newer.txt"));
        assert!(e.pane_tab(1).expect("right").selection.is_empty(), "only that pane");
    });
}

#[gpui_kit::test]
fn sync_without_a_deeper_answer_copies_the_unique_and_newer_items_to_the_other_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    h.task(cx, |e, cx| e.sync_pane(0, cx));
    assert_eq!(
        h.disk.ops().last(),
        Some(&FileOp::Copy {
            sources: vec!["C:\\left\\newer.txt".into(), "C:\\left\\تقرير.txt".into()],
            dest: R.into()
        })
    );
    assert!(h.disk.exists("C:\\right\\تقرير.txt"));
    assert_eq!(
        h.read(cx, |e| e.active_pane()),
        0,
        "the pane that was asked has the keyboard"
    );
}

#[gpui_kit::test]
fn sync_deeper_copies_what_core_said_into_the_folders_it_goes_in_and_reads_both_panes_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    // A file inside a subfolder that only the left pane has.
    h.disk.folder(
        L,
        vec![
            folder(L, "sub"),
            file(L, "newer.txt", 10, 200_000),
            file(L, "same.txt", 10, 100_000),
            file(L, "تقرير.txt", 5, 5_000),
        ],
    );
    h.disk
        .folder("C:\\left\\sub", vec![file("C:\\left\\sub", "deep.txt", 3, 3_000)]);
    h.disk.folder(
        R,
        vec![
            folder(R, "sub"),
            file(R, "newer.txt", 10, 100_000),
            file(R, "same.txt", 10, 100_000),
        ],
    );
    h.disk.folder("C:\\right\\sub", vec![]);
    assert!(h.go(cx, L));
    let fake = FakeDeep::default();
    h.run(cx, |e, _| e.set_deep_core(fake.core()));
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    options(cx, true, false);
    fake.answer(
        0,
        Ok(DeepAnswer {
            to_right: Arc::from(vec![
                copy_of("C:\\left\\sub\\deep.txt", "C:\\right\\sub"),
                copy_of("C:\\left\\تقرير.txt", R),
            ]),
            ..DeepAnswer::default()
        }),
    );
    cx.run_until_parked();

    let listed = (h.listed(L), h.listed(R));
    h.task(cx, |e, cx| e.sync_pane(0, cx));
    assert!(h.disk.exists("C:\\right\\sub\\deep.txt"), "into the subfolder");
    assert!(h.disk.exists("C:\\right\\تقرير.txt"));
    assert!(h.disk.exists("C:\\left\\sub\\deep.txt"), "a copy, not a move");
    match h.disk.ops().last() {
        Some(FileOp::CopyInto { items }) => assert_eq!(items.len(), 2),
        other => panic!("{other:?}"),
    }
    assert!(operations(&h)
        .iter()
        .any(|n| matches!(n, OpNotice::Synced { count: 2, .. })));
    assert!(
        h.listed(L) > listed.0 && h.listed(R) > listed.1,
        "both panes are read again"
    );
    assert!(names(&h, cx, 1).contains(&"تقرير.txt".to_string()));
}

#[gpui_kit::test]
fn a_sync_that_fails_says_so_and_still_reads_the_panes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    two_folders(&h, cx);
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, R));
    h.disk.fail_next("Access is denied");
    let before = h.listed(R);
    h.task(cx, |e, cx| e.sync_into(vec![copy_of("C:\\left\\newer.txt", R)], cx));
    assert!(operations(&h).contains(&OpNotice::Failed {
        what: Failing::CopyOrMove {
            moving: false,
            count: 1
        },
        message: "Access is denied".into()
    }));
    assert!(h.listed(R) > before);
    // Nothing to copy is nothing done.
    let ops = h.disk.ops().len();
    h.task(cx, |e, cx| e.sync_into(Vec::new(), cx));
    assert_eq!(h.disk.ops().len(), ops);
}

/// A folder tree on the real disk, for the real core to compare: `left` and `right` hold the same file, a file with
/// the same size and date but other contents, and a subfolder where `right` lacks a file `left` has.
fn real_folders(name: &str) -> (std::path::PathBuf, String, String) {
    let root = std::env::temp_dir().join(format!("glint-compare-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (left, right) = (root.join("left"), root.join("right"));
    for side in [&left, &right] {
        std::fs::create_dir_all(side.join("sub").join("inner")).expect("a temp folder");
        std::fs::write(side.join("same.txt"), "alike").expect("a temp file");
    }
    std::fs::write(left.join("changed.txt"), "one").expect("a temp file");
    std::fs::write(right.join("changed.txt"), "two").expect("a temp file");
    // Both files are as old as each other, as far as the comparison can tell.
    let time = std::fs::metadata(left.join("changed.txt"))
        .and_then(|m| m.modified())
        .expect("a date");
    let other = std::fs::OpenOptions::new()
        .write(true)
        .open(right.join("changed.txt"))
        .expect("the file");
    other.set_modified(time).expect("a date set");
    std::fs::write(left.join("sub").join("inner").join("new.txt"), "new").expect("a temp file");
    let text = |p: &std::path::Path| p.to_string_lossy().to_string();
    (root.clone(), text(&left), text(&right))
}

/// Milliseconds since 1970 of a file on the real disk.
fn modified_ms(path: &str) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

#[gpui_kit::test]
fn the_real_core_compares_inside_subfolders_and_by_contents(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.executor().allow_parking();
    let h = harness(cx);
    quiet_gallery(&h, cx);
    let (root, left, right) = real_folders("real");
    let in_left = |name: &str| format!("{left}\\{name}");
    let in_right = |name: &str| format!("{right}\\{name}");
    // What the fake disk lists is what the real disk holds, as far as the panes' listings go.
    for dir in [&left, &right] {
        let on_disk = |name: &str| {
            let path = format!("{dir}\\{name}");
            file(
                dir,
                name,
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                modified_ms(&path),
            )
        };
        h.disk.folder(
            dir,
            vec![on_disk("changed.txt"), on_disk("same.txt"), folder(dir, "sub")],
        );
        h.disk
            .folder(&format!("{dir}\\sub"), vec![folder(&format!("{dir}\\sub"), "inner")]);
    }
    let inner = in_left("sub\\inner");
    h.disk.folder(
        &inner,
        vec![file(&inner, "new.txt", 3, modified_ms(&format!("{inner}\\new.txt")))],
    );
    h.disk.folder(&in_right("sub\\inner"), vec![]);
    assert!(h.go(cx, &left));
    h.run(cx, |e, cx| e.toggle_compare_panes(cx));
    assert!(h.go(cx, &right));
    let before = answer(&h, cx, 0).expect("compared by name, date and size");
    assert_eq!(
        before.differences(),
        0,
        "the same as far as the listings can tell: {:?}",
        before.diffs
    );

    options(cx, true, true);
    for _ in 0..1000 {
        cx.run_until_parked();
        if !h.read(cx, |e| e.deep_running()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.read(cx, |e| e.deep_error().to_string()), "");
    let deeper = answer(&h, cx, 0).expect("left");
    assert_eq!(deeper.diffs["changed.txt"], DiffStatus::Differ, "other contents");
    assert_eq!(deeper.diffs["same.txt"], DiffStatus::Same);
    assert_eq!(deeper.diffs["sub"], DiffStatus::Differ, "something differs inside");
    assert_eq!(answer(&h, cx, 1).expect("right").diffs["sub"], DiffStatus::Differ);
    let items = h.read(cx, |e| e.deep_sync_items(0)).expect("what Sync copies");
    assert_eq!(
        items
            .iter()
            .map(|i| (i.path.clone(), i.folder.clone()))
            .collect::<Vec<_>>(),
        [(in_left("sub\\inner\\new.txt"), in_right("sub\\inner"))]
    );

    // Sync copies it into the subfolder it goes in.
    h.task(cx, |e, cx| e.sync_pane(0, cx));
    assert!(h.disk.exists(&in_right("sub\\inner\\new.txt")));
    let _ = std::fs::remove_dir_all(root);
}
