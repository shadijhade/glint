//! Headless tests of the compare bar and the bar for two files selected across the panes: which bar a pane shows,
//! what is in it (the badges, the counts, the buttons and what they do), the options menu, and the same in the
//! Arabic interface. They run on the explorer's fake disk; the deeper comparison runs on the real core against
//! folders that don't exist (to see its error) and against real ones (see `explorer/compare_tests.rs`).

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{Explorer, NavOptions};
use crate::testing;
use glint_model::store::StartFolder;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext, VisualTestContext};

const L: &str = "C:\\left";
const R: &str = "C:\\right";

/// Both bars, one above the other, as the workspace puts one over each pane.
struct Host {
    bars: Vec<Entity<CompareBar>>,
}

impl Render for Host {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        col().w_full().children(self.bars.clone())
    }
}

struct Rig {
    visual: VisualTestContext,
    explorer: Entity<Explorer>,
    disk: FakeDisk,
    host: Entity<Host>,
}

/// `left` has an Arabic file the right lacks, a newer file and a file that is the same; `right` has a file `left`
/// lacks. Two panes, compared, on those two folders (or only the left, when `compared` is off).
fn rig(cx: &mut TestAppContext, rtl: bool, compared: bool) -> Rig {
    cx.update(testing::init);
    cx.update(|cx| state::update(cx, |settings| settings.start_folder = StartFolder::ThisPc));
    testing::set_rtl(rtl);
    let disk = FakeDisk::new();
    disk.folder(
        L,
        vec![
            file(L, "newer.txt", 10, 200_000),
            file(L, "same.txt", 10, 100_000),
            file(L, "تقرير.txt", 5, 5_000),
        ],
    );
    disk.folder(
        R,
        vec![
            file(R, "extra.txt", 7, 7_000),
            file(R, "newer.txt", 10, 100_000),
            file(R, "same.txt", 10, 100_000),
        ],
    );
    disk.folder("C:\\twin", vec![file("C:\\twin", "same.txt", 10, 100_000)]);
    disk.folder("C:\\", vec![folder("C:\\", "left"), folder("C:\\", "right")]);
    cx.update(|cx| crate::explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let (mut visual, host) = testing::open_window(cx, |window, cx| {
        let bars = (0..2)
            .map(|pane| cx.new(|cx| CompareBar::new(pane, window, cx)))
            .collect();
        cx.new(|_| Host { bars })
    });
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx).detach()));
    settle(&mut visual);
    let mut rig = Rig {
        visual,
        explorer,
        disk,
        host,
    };
    rig.go(0, L);
    rig.act(|e, cx| e.toggle_dual(cx));
    rig.go(1, R);
    if compared {
        rig.act(|e, cx| e.toggle_compare_panes(cx));
    }
    rig.act(|e, cx| e.focus_pane(0, cx));
    rig
}

impl Drop for Rig {
    /// The language is process-wide: whatever a test did, the next one starts in English.
    fn drop(&mut self) {
        testing::set_rtl(false);
    }
}

fn settle(visual: &mut VisualTestContext) {
    for _ in 0..20 {
        visual.run_until_parked();
    }
}

impl Rig {
    fn act<R>(&mut self, f: impl FnOnce(&mut Explorer, &mut Context<Explorer>) -> R) -> R {
        let result = self.explorer.update(&mut self.visual, f);
        settle(&mut self.visual);
        result
    }

    fn go(&mut self, pane: usize, path: &str) {
        let path = path.to_string();
        self.act(move |e, cx| {
            e.focus_pane(pane, cx);
            let tab = e.pane_tab_id(pane);
            e.navigate(path, NavOptions::in_tab(tab), cx);
        });
    }

    /// Selects `paths` in `pane`.
    fn select(&mut self, pane: usize, paths: &[&str]) {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        self.act(move |e, cx| {
            e.focus_pane(pane, cx);
            e.set_selection(paths, None, None, cx);
        });
    }

    fn bar(&mut self, pane: usize) -> Bar {
        let bar = self.host.read_with(&self.visual, |host, _| host.bars[pane].clone());
        bar.read_with(&self.visual, |bar, cx| bar.bar(cx))
    }

    fn view(&self, pane: usize) -> Entity<CompareBar> {
        self.host.read_with(&self.visual, |host, _| host.bars[pane].clone())
    }

    fn frame(&mut self) {
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    /// The accessible labels inside the bar over `pane`, top to bottom, start to end.
    fn labels(&mut self, pane: usize) -> Vec<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            let mut labels = Vec::new();
            for id in ["compare-bar", "compare-pair-bar"] {
                if let Some(frame) = window.try_find((id, pane)) {
                    labels.extend(frame.label().map(str::to_string));
                    labels.extend(
                        window
                            .within((id, pane))
                            .elements()
                            .into_iter()
                            .filter_map(|e| e.label().map(str::to_string)),
                    );
                }
            }
            labels
        })
    }

    fn has(&mut self, pane: usize, label: &str) -> bool {
        self.labels(pane).iter().any(|l| l == label)
    }

    /// Whether the element with this id is there, and whether it is disabled.
    fn button(&mut self, id: (&'static str, usize)) -> Option<bool> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).map(|e| e.disabled().unwrap_or(false))
        })
    }

    fn click(&mut self, id: (&'static str, usize)) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        settle(&mut self.visual);
    }

    fn left_of(&mut self, id: (&'static str, usize)) -> f32 {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find(id).bounds().origin.x)
        })
    }
}

fn names(pair: &Pair) -> (String, String) {
    (pair.left.name.clone(), pair.right.name.clone())
}

// ---- Which bar, and what is in it ---------------------------------------------------------------------------------------

#[gpui_kit::test]
fn there_is_no_bar_in_one_pane_or_while_nothing_is_selected_across_panes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, false);
    assert!(matches!(rig.bar(0), Bar::Hidden) && matches!(rig.bar(1), Bar::Hidden));
    // One pane: whatever is selected, there is nothing across.
    rig.act(|e, cx| e.toggle_dual(cx));
    assert!(matches!(rig.bar(0), Bar::Hidden) && matches!(rig.bar(1), Bar::Hidden));
    // Nothing is drawn, not even a frame.
    assert!(rig.labels(0).is_empty() && rig.labels(1).is_empty());
}

#[gpui_kit::test]
fn one_file_in_each_pane_is_a_pair_and_the_bar_over_the_active_pane_offers_to_compare_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, false);
    rig.select(1, &["C:\\right\\extra.txt"]);
    rig.select(0, &["C:\\left\\تقرير.txt"]);
    let Bar::Pair(pair) = rig.bar(0) else {
        panic!("the pair bar")
    };
    // The left pane's file first, whatever pane asks.
    assert_eq!(names(&pair), ("تقرير.txt".to_string(), "extra.txt".to_string()));
    assert!(matches!(rig.bar(1), Bar::Hidden), "only the pane that has the keyboard");
    assert!(rig.has(0, "Selected across panes: “تقرير.txt” ↔ “extra.txt”"));
    assert_eq!(rig.button(("compare-files", 0)), Some(false), "an enabled button");

    // The same pair from the other side is the same way round.
    rig.act(|e, cx| e.focus_pane(1, cx));
    let Bar::Pair(from_right) = rig.bar(1) else {
        panic!("the pair bar over the right pane")
    };
    assert_eq!(from_right, pair);
    assert!(matches!(rig.bar(0), Bar::Hidden));
}

#[gpui_kit::test]
fn a_folder_beside_the_file_does_not_matter_but_two_files_or_the_same_file_are_no_pair(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, false);
    rig.disk.folder(
        L,
        vec![
            folder(L, "docs"),
            file(L, "newer.txt", 10, 200_000),
            file(L, "same.txt", 10, 100_000),
        ],
    );
    rig.go(0, L);
    rig.select(1, &["C:\\right\\extra.txt"]);
    rig.select(0, &["C:\\left\\docs", "C:\\left\\newer.txt"]);
    let pair = rig.explorer.read_with(&rig.visual, |e, _| cross_pair(e, 0));
    assert_eq!(
        pair.map(|p| p.left.name),
        Some("newer.txt".to_string()),
        "the folder is not a file"
    );

    rig.select(0, &["C:\\left\\newer.txt", "C:\\left\\same.txt"]);
    assert!(rig.explorer.read_with(&rig.visual, |e, _| cross_pair(e, 0)).is_none());

    // Nothing in the other pane.
    rig.select(0, &["C:\\left\\newer.txt"]);
    rig.select(1, &[]);
    rig.act(|e, cx| e.focus_pane(0, cx));
    assert!(rig.explorer.read_with(&rig.visual, |e, _| cross_pair(e, 0)).is_none());

    // The same file in both panes (one folder open twice) is not two files.
    rig.go(1, L);
    rig.select(1, &["C:\\left\\newer.txt"]);
    rig.select(0, &["C:\\left\\newer.txt"]);
    assert!(rig.explorer.read_with(&rig.visual, |e, _| cross_pair(e, 0)).is_none());
}

#[gpui_kit::test]
fn comparing_panes_replaces_the_pair_bar_with_the_compare_bar_over_both_panes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    assert!(matches!(rig.bar(0), Bar::Compare(_)) && matches!(rig.bar(1), Bar::Compare(_)));
    assert_eq!(rig.button(("compare-stop", 0)), Some(false));
    assert_eq!(rig.button(("compare-stop", 1)), Some(false));
    // A search hides it, and so does a place that is not a folder.
    rig.act(|e, cx| e.focus_pane(1, cx));
    rig.go(1, "");
    assert!(matches!(rig.bar(1), Bar::Hidden), "This PC is not a folder");
    assert!(matches!(rig.bar(0), Bar::Compare(_)));
    assert!(rig.has(0, "Compare panes") || rig.button(("compare-stop", 0)).is_some());
}

// ---- What the compare bar says ----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn each_bar_counts_what_differs_in_its_own_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    // Left: the Arabic file is only here, newer.txt is newer, same.txt is the same.
    let left = rig.labels(0);
    assert!(left.contains(&"1 only here".to_string()), "{left:?}");
    assert!(left.contains(&"1 newer".to_string()), "{left:?}");
    assert!(left.contains(&"1 same".to_string()), "{left:?}");
    assert!(!left
        .iter()
        .any(|l| l.ends_with("older") || l.ends_with("different") || l == "All match"));
    // Right: extra.txt is only here, newer.txt is older.
    let right = rig.labels(1);
    assert!(right.contains(&"1 only here".to_string()), "{right:?}");
    assert!(right.contains(&"1 older".to_string()), "{right:?}");
    assert!(!right.iter().any(|l| l.ends_with("newer")), "{right:?}");

    // Stop is the last button: at the end edge, so right of Sync in English.
    let (sync_x, stop_x) = (rig.left_of(("compare-sync", 0)), rig.left_of(("compare-stop", 0)));
    assert!(sync_x < stop_x, "{sync_x} {stop_x}");

    // Select differences (N) counts what is not the same, and Sync N what would be copied.
    assert_eq!(rig.button(("compare-select", 0)), Some(false));
    assert_eq!(rig.button(("compare-sync", 0)), Some(false));
    let Bar::Compare(info) = rig.bar(0) else {
        panic!("the compare bar")
    };
    assert_eq!((info.sync_count, info.deep_sync, info.other_writable), (2, false, true));
    assert_eq!(info.comparison.as_ref().map(|c| c.diff_paths.len()), Some(2));
    let Bar::Compare(info) = rig.bar(1) else {
        panic!("the compare bar")
    };
    assert_eq!(info.sync_count, 1, "only extra.txt goes left");
}

#[gpui_kit::test]
fn when_everything_matches_it_says_so_and_there_is_nothing_to_select_or_sync(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.disk.folder(
        R,
        vec![
            file(R, "newer.txt", 10, 200_000),
            file(R, "same.txt", 10, 100_000),
            file(R, "تقرير.txt", 5, 5_000),
        ],
    );
    let right = rig.explorer.read_with(&rig.visual, |e, _| e.pane_tab_id(1));
    rig.act(move |e, cx| e.reload(Some(right), cx).detach());
    assert!(rig.has(0, "All match"), "{:?}", rig.labels(0));
    assert!(rig.has(0, "3 same"));
    assert_eq!(rig.button(("compare-select", 0)), Some(true), "nothing to select");
    assert_eq!(rig.button(("compare-sync", 0)), Some(true), "nothing to sync");
    assert!(!rig.labels(0).iter().any(|l| l.ends_with("only here")));
}

#[gpui_kit::test]
fn without_a_comparison_the_bar_says_what_would_make_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    // The other pane has no folder.
    rig.go(1, "");
    assert!(matches!(rig.bar(0), Bar::Compare(_)));
    // The text is the bar's, and Stop is there; no counts, no Sync.
    assert_eq!(rig.button(("compare-stop", 0)), Some(false));
    assert_eq!(rig.button(("compare-sync", 0)), None);
    assert_eq!(rig.button(("compare-select", 0)), None);
    let Bar::Compare(info) = rig.bar(0) else {
        panic!("the compare bar")
    };
    assert!(info.comparison.is_none() && !info.same_folder && !info.pending);

    // The same folder in both panes.
    rig.go(1, L);
    let Bar::Compare(info) = rig.bar(0) else {
        panic!("the compare bar")
    };
    assert!(info.comparison.is_none() && info.same_folder && info.pair.is_none());
    // ... where a file selected in each pane is the pair to compare.
    rig.select(1, &["C:\\left\\newer.txt"]);
    rig.select(0, &["C:\\left\\same.txt"]);
    let Bar::Compare(info) = rig.bar(0) else {
        panic!("the compare bar")
    };
    assert_eq!(
        info.pair.as_ref().map(names),
        Some(("same.txt".to_string(), "newer.txt".to_string()))
    );
    assert_eq!(rig.button(("compare-pair", 0)), Some(false), "Compare files");
}

#[gpui_kit::test]
fn the_pair_gets_its_own_button_in_the_compare_bar_named_after_both_files(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.select(1, &["C:\\right\\extra.txt"]);
    rig.select(0, &["C:\\left\\تقرير.txt"]);
    assert_eq!(rig.button(("compare-pair", 0)), Some(false));
    assert_eq!(rig.button(("compare-pair", 1)), Some(false), "both bars offer it");
    // And Sync, Select differences and the counts are still there.
    assert!(rig.button(("compare-sync", 0)).is_some() && rig.button(("compare-options", 0)).is_some());
}

// ---- What the buttons do -----------------------------------------------------------------------------------------------

fn dialog_open(rig: &mut Rig) -> bool {
    rig.visual.update(|window, cx| window.has_active_dialog(cx))
}

#[gpui_kit::test]
fn compare_files_in_the_pair_bar_opens_the_two_files_side_by_side(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, false);
    rig.select(1, &["C:\\right\\extra.txt"]);
    rig.select(0, &["C:\\left\\تقرير.txt"]);
    assert!(!dialog_open(&mut rig));
    rig.click(("compare-files", 0));
    assert!(dialog_open(&mut rig), "the dialog is open");
}

#[gpui_kit::test]
fn the_pairs_button_in_the_compare_bar_opens_them_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.select(1, &["C:\\right\\extra.txt"]);
    rig.select(0, &["C:\\left\\newer.txt"]);
    assert!(!dialog_open(&mut rig));
    rig.click(("compare-pair", 1));
    assert!(dialog_open(&mut rig), "the dialog is open");
}

#[gpui_kit::test]
fn stop_ends_the_comparison_and_the_bars_go(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.click(("compare-stop", 1));
    rig.explorer.read_with(&rig.visual, |e, _| {
        assert!(!e.compare_panes && !e.diff_only && e.is_dual())
    });
    assert!(matches!(rig.bar(0), Bar::Hidden) && matches!(rig.bar(1), Bar::Hidden));
    assert_eq!(rig.button(("compare-stop", 0)), None);
}

#[gpui_kit::test]
fn select_differences_selects_them_in_its_own_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.click(("compare-select", 1));
    rig.explorer.read_with(&rig.visual, |e, _| {
        let right = e.pane_tab(1).expect("right");
        assert_eq!(
            right.selection.to_vec(),
            ["C:\\right\\extra.txt", "C:\\right\\newer.txt"]
        );
        assert!(e.pane_tab(0).expect("left").selection.is_empty());
    });
}

#[gpui_kit::test]
fn sync_copies_what_it_says_to_the_other_pane_and_gives_it_the_keyboard(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.click(("compare-sync", 1));
    // The right pane's Sync: extra.txt goes into the left folder; the pane asked has the keyboard.
    assert!(rig.disk.exists("C:\\left\\extra.txt"));
    assert!(!rig.disk.exists("C:\\right\\تقرير.txt"));
    rig.explorer
        .read_with(&rig.visual, |e, _| assert_eq!(e.active_pane(), 1));

    rig.click(("compare-sync", 0));
    assert!(rig.disk.exists("C:\\right\\تقرير.txt"), "the Arabic file went right");
}

#[gpui_kit::test]
fn the_sync_arrow_points_at_the_other_pane_in_both_directions(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    let sync = |rig: &mut Rig, pane: usize| -> String {
        rig.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find(("compare-sync", pane))
                .and_then(|e| e.label().map(str::to_string))
                .unwrap_or_default()
        })
    };
    assert_eq!(sync(&mut rig, 0), "Sync 2 →");
    assert_eq!(sync(&mut rig, 1), "← Sync 1");
    testing::set_rtl(true);
    rig.act(|_, cx| cx.notify());
    // The panes are mirrored, so the left pane's other pane is on its left.
    let (first, second) = (sync(&mut rig, 0), sync(&mut rig, 1));
    testing::set_rtl(false);
    assert!(first.contains('←') && !first.contains('→'), "{first:?}");
    assert!(second.contains('→') && !second.contains('←'), "{second:?}");
}

// ---- The options -----------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_options_menu_has_the_three_rows_with_their_check_marks(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.click(("compare-options", 0));
    let view = rig.view(0);
    assert!(view.read_with(&rig.visual, |bar, _| bar.menu_open()));
    let nodes = view.read_with(&rig.visual, |bar, cx| {
        bar.menu_view()
            .map(|menu| menu.read(cx).nodes().to_vec())
            .unwrap_or_default()
    });
    let rows: Vec<(String, Option<bool>)> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Item(entry) => Some((entry.label.to_string(), entry.check)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Subfolders too (skipping node_modules, .git)".to_string(), Some(false)),
            ("File contents, not only sizes and dates".to_string(), Some(false)),
            ("Only the differences".to_string(), Some(false)),
        ]
    );
    assert!(matches!(nodes[2], Node::Separator));
}

#[test]
fn the_menu_says_nothing_of_skipping_when_nothing_is_skipped_and_marks_what_is_on() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let nodes = options_menu(&Options {
        subfolders: true,
        contents: false,
        diff_only: true,
        exclusions: Vec::new(),
    });
    let rows: Vec<(String, Option<bool>)> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Item(entry) => Some((entry.label.to_string(), entry.check)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Subfolders too".to_string(), Some(true)),
            ("File contents, not only sizes and dates".to_string(), Some(false)),
            ("Only the differences".to_string(), Some(true)),
        ]
    );
    // Each row carries its pick back to the bar.
    let picks: Vec<Option<Pick>> = nodes
        .iter()
        .filter_map(|node| match node {
            Node::Item(entry) => Some(Pick::of(&entry.run)),
            _ => None,
        })
        .collect();
    assert_eq!(
        picks,
        [Some(Pick::Subfolders), Some(Pick::Contents), Some(Pick::DiffOnly)]
    );
    assert_eq!(Pick::of(&Run::Nothing), None);
    assert!(!Options::default().comparing_more());
    assert!(Options {
        diff_only: true,
        ..Options::default()
    }
    .comparing_more());
}

#[gpui_kit::test]
fn choosing_a_row_flips_the_setting_or_shows_only_the_differences(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    let choose = |rig: &mut Rig, pick: Pick| {
        rig.click(("compare-options", 0));
        let menu = rig
            .view(0)
            .read_with(&rig.visual, |bar, _| bar.menu_view().cloned())
            .expect("the menu is open");
        rig.visual.update(|_, cx| {
            menu.update(cx, |_, cx| cx.emit(MenuViewEvent::Chose(pick.run())));
        });
        settle(&mut rig.visual);
        assert!(
            !rig.view(0).read_with(&rig.visual, |bar, _| bar.menu_open()),
            "choosing closes it"
        );
    };
    let settings = |rig: &mut Rig| {
        rig.visual.update(|_, cx| {
            (
                state::settings(cx).compare_subfolders,
                state::settings(cx).compare_contents,
            )
        })
    };
    assert_eq!(settings(&mut rig), (false, false));
    choose(&mut rig, Pick::Subfolders);
    assert_eq!(settings(&mut rig), (true, false));
    choose(&mut rig, Pick::Contents);
    assert_eq!(settings(&mut rig), (true, true));
    choose(&mut rig, Pick::Subfolders);
    assert_eq!(settings(&mut rig), (false, true));

    choose(&mut rig, Pick::DiffOnly);
    rig.explorer.read_with(&rig.visual, |e, _| assert!(e.diff_only));
    // The identical items are left out of the rows.
    let rows = rig
        .explorer
        .read_with(&rig.visual, |e, _| e.pane_tab(0).map(|t| t.rows.len()));
    assert_eq!(rows, Some(2), "same.txt is left out");
    choose(&mut rig, Pick::DiffOnly);
    rig.explorer.read_with(&rig.visual, |e, _| assert!(!e.diff_only));
}

#[gpui_kit::test]
fn escape_closes_the_menu_and_changes_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.click(("compare-options", 0));
    assert!(rig.view(0).read_with(&rig.visual, |bar, _| bar.menu_open()));
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.press("escape", cx);
    });
    settle(&mut rig.visual);
    assert!(!rig.view(0).read_with(&rig.visual, |bar, _| bar.menu_open()));
    assert!(!rig.visual.update(|_, cx| state::settings(cx).compare_subfolders));
}

// ---- Deeper ------------------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_bar_shows_a_spinner_while_the_deeper_comparison_runs_and_says_why_when_it_fails(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, true);
    rig.visual.executor().allow_parking();
    assert!(!rig.has(0, "Comparing deeper"));
    // The real core asked to read folders that are not on this disk: it fails, and the bar says so.
    rig.visual
        .update(|_, cx| state::update(cx, |settings| settings.compare_subfolders = true));
    rig.frame();
    assert!(rig.has(0, "Comparing deeper"), "{:?}", rig.labels(0));
    assert!(rig.has(1, "Comparing deeper"), "both bars show it");
    for _ in 0..1000 {
        rig.visual.run_until_parked();
        if !rig.explorer.read_with(&rig.visual, |e, _| e.deep_running()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!rig.has(0, "Comparing deeper"));
    let said = rig
        .labels(0)
        .into_iter()
        .find(|l| l.starts_with("Couldn't compare deeper: "))
        .expect("the error is in the bar");
    assert!(said.contains(L), "{said}");
    // The counts are still there, compared by names, dates and sizes.
    assert!(rig.has(0, "1 only here"));
    // The options button has its dot on: the person asked for more than the plain comparison.
    assert!(rig.button(("compare-options", 0)).is_some());
}

// ---- Arabic ----------------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_bars_mirror_in_arabic_and_their_words_are_translated_where_the_catalog_has_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, true, true);
    // The bar's end group (Stop) is at the start of the line in Arabic, so left of the first icon; in English it is
    // the other way.
    let (stop_x, bar_x) = (rig.left_of(("compare-stop", 0)), rig.left_of(("compare-sync", 0)));
    assert!(
        stop_x < bar_x,
        "Stop is the last button, so the left-most one in Arabic: {stop_x} {bar_x}"
    );
    rig.disk.folder("C:\\تقارير", vec![file("C:\\تقارير", "ملف.txt", 1, 1)]);
    rig.go(1, "C:\\تقارير");
    assert!(matches!(rig.bar(0), Bar::Compare(_)));
    let arabic = rig.labels(0);
    assert!(!arabic.is_empty());
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_pair_bar_names_arabic_files_and_mirrors(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, true, false);
    rig.select(1, &["C:\\right\\extra.txt"]);
    rig.select(0, &["C:\\left\\تقرير.txt"]);
    let pair_bar = rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(("compare-pair-bar", 0usize)).map(|e| e.bounds())
    });
    let button = rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(("compare-files", 0usize)).map(|e| e.bounds())
    });
    let (bar, button) = (pair_bar.expect("the bar"), button.expect("the button"));
    assert!(
        button.right() <= bar.left() + bar.size.width && button.origin.x < bar.origin.x + bar.size.width / 2.,
        "Compare files is at the end edge, which is the left in Arabic: {button:?} in {bar:?}"
    );
    testing::set_rtl(false);
}
