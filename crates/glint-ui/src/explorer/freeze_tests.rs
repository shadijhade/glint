//! Headless tests of Freeze order (`freeze.rs`): a folder's rows keep their order and places while it changes,
//! and sort once when the freeze ends. A fake disk stands behind the explorer.

use super::fake::*;
use super::*;
use crate::testing;
use glint_model::entries::{Sort, SortKey};
use gpui_kit::{AppContext as _, Entity, TestAppContext};

struct Harness {
    explorer: Entity<Explorer>,
    disk: FakeDisk,
}

fn harness(cx: &mut TestAppContext) -> Harness {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
    Harness { explorer, disk }
}

impl Harness {
    fn go(&self, cx: &mut TestAppContext, path: &str) {
        let task = self
            .explorer
            .update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        assert!(cx.foreground_executor().block_test(task));
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

    /// The folder is read again, as a change on disk makes it.
    fn reread(&self, cx: &mut TestAppContext) {
        self.run(cx, |e, cx| e.reload(None, cx).detach());
    }

    fn names(&self, cx: &TestAppContext) -> Vec<String> {
        self.explorer.read_with(cx, |e, _| {
            e.active_tab()
                .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    fn size_of(&self, cx: &TestAppContext, name: &str) -> Option<u64> {
        self.explorer.read_with(cx, |e, _| {
            e.active_tab()
                .and_then(|t| t.rows.iter().find(|r| r.name == name).map(|r| r.size))
        })
    }

    fn frozen(&self, cx: &TestAppContext) -> bool {
        self.explorer.read_with(cx, |e, _| e.is_frozen(e.active_tab_id()))
    }
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| name.to_string()).collect()
}

const DIR: &str = "C:\\busy";

/// Four files, by size, largest first: d (40), c (30), b (20), a (10).
fn busy_folder(disk: &FakeDisk) {
    disk.folder(
        DIR,
        vec![
            file(DIR, "a.txt", 10, 1),
            file(DIR, "b.txt", 20, 2),
            file(DIR, "c.txt", 30, 3),
            file(DIR, "d.txt", 40, 4),
        ],
    );
}

fn sorted_by_size(h: &Harness, cx: &mut TestAppContext) {
    h.run(cx, |e, cx| {
        e.set_sort(
            Sort {
                key: SortKey::Size,
                desc: true,
            },
            cx,
        )
    });
}

#[gpui_kit::test]
fn a_frozen_list_keeps_every_row_where_it_is_through_adds_removes_renames_and_new_sizes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    busy_folder(&h.disk);
    h.go(cx, DIR);
    sorted_by_size(&h, cx);
    assert_eq!(h.names(cx), names(&["d.txt", "c.txt", "b.txt", "a.txt"]));

    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    assert!(h.frozen(cx));
    assert_eq!(
        h.names(cx),
        names(&["d.txt", "c.txt", "b.txt", "a.txt"]),
        "freezing moves nothing"
    );

    // c grows past d (it would be first), b is deleted, a is renamed, and two files arrive, one of them huge.
    h.disk.folder(
        DIR,
        vec![
            file(DIR, "aa.txt", 10, 5),
            file(DIR, "c.txt", 100, 6),
            file(DIR, "d.txt", 40, 4),
            file(DIR, "huge.bin", 9_000, 7),
            file(DIR, "new.txt", 5, 8),
        ],
    );
    h.reread(cx);
    assert_eq!(
        h.names(cx),
        names(&["d.txt", "c.txt", "b.txt", "a.txt", "aa.txt", "huge.bin", "new.txt"]),
        "the rows are where they were, what went stays, what came is at the end"
    );
    assert_eq!(h.size_of(cx, "c.txt"), Some(100), "sizes still update in place");

    // What went is drawn dimmed (the views dim the cut ones), and the bar can say what came.
    h.explorer.read_with(cx, |e, _| {
        assert!(e.cut_paths().contains("C:\\busy\\b.txt"));
        assert!(e.cut_paths().contains("C:\\busy\\a.txt"));
        assert!(!e.cut_paths().contains("C:\\busy\\c.txt"));
        assert_eq!(
            e.frozen_info(e.active_tab_id()),
            Some(FrozenInfo { arrived: 3, gone: 2 })
        );
    });

    // Another change: positions still hold.
    h.disk.folder(
        DIR,
        vec![
            file(DIR, "aa.txt", 10, 5),
            file(DIR, "c.txt", 100, 6),
            file(DIR, "d.txt", 4_000, 9),
            file(DIR, "huge.bin", 9_000, 7),
            file(DIR, "new.txt", 5, 8),
        ],
    );
    h.reread(cx);
    assert_eq!(
        h.names(cx),
        names(&["d.txt", "c.txt", "b.txt", "a.txt", "aa.txt", "huge.bin", "new.txt"])
    );
    assert_eq!(h.size_of(cx, "d.txt"), Some(4_000));
}

#[gpui_kit::test]
fn turning_the_freeze_off_sorts_once_and_drops_what_went(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    busy_folder(&h.disk);
    h.go(cx, DIR);
    sorted_by_size(&h, cx);
    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    h.disk.folder(
        DIR,
        vec![
            file(DIR, "a.txt", 10, 1),
            file(DIR, "c.txt", 100, 6),
            file(DIR, "d.txt", 40, 4),
            file(DIR, "huge.bin", 9_000, 7),
        ],
    );
    h.reread(cx);
    assert_eq!(h.names(cx), names(&["d.txt", "c.txt", "b.txt", "a.txt", "huge.bin"]));

    h.run(cx, |e, cx| e.set_frozen(None, false, cx));
    assert!(!h.frozen(cx));
    assert_eq!(
        h.names(cx),
        names(&["huge.bin", "c.txt", "d.txt", "a.txt"]),
        "sorted by size again, with b gone for good"
    );
    h.explorer.read_with(cx, |e, _| {
        assert!(!e.cut_paths().contains("C:\\busy\\b.txt"), "nothing is dimmed any more");
    });
}

#[gpui_kit::test]
fn toggling_and_leaving_the_folder_end_the_freeze(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    busy_folder(&h.disk);
    h.disk.folder("C:\\other", vec![file("C:\\other", "o.txt", 1, 1)]);
    h.go(cx, DIR);
    assert!(h.explorer.read_with(cx, |e, _| e.can_freeze(None)));
    h.run(cx, |e, cx| e.toggle_frozen(None, cx));
    assert!(h.frozen(cx));
    h.run(cx, |e, cx| e.toggle_frozen(None, cx));
    assert!(!h.frozen(cx));
    h.run(cx, |e, cx| e.toggle_frozen(None, cx));
    assert!(h.frozen(cx));
    h.go(cx, "C:\\other");
    assert!(!h.frozen(cx), "leaving the folder unfreezes");
    assert_eq!(h.names(cx), names(&["o.txt"]));
    h.explorer.read_with(cx, |e, _| assert!(e.freezes.is_empty()));
}

#[gpui_kit::test]
fn a_column_header_sort_ends_the_freeze_and_a_search_or_this_pc_cannot_be_frozen(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    busy_folder(&h.disk);
    h.go(cx, DIR);
    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    sorted_by_size(&h, cx);
    assert!(!h.frozen(cx), "asking for an order ends the freeze");
    assert_eq!(h.names(cx), names(&["d.txt", "c.txt", "b.txt", "a.txt"]));

    h.go(cx, glint_model::paths::THIS_PC);
    assert!(!h.explorer.read_with(cx, |e, _| e.can_freeze(None)));
    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    assert!(!h.frozen(cx));
}

#[gpui_kit::test]
fn freezing_a_list_that_was_filtered_or_hides_items_keeps_them_all_in_the_listing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk.folder(
        DIR,
        vec![
            file(DIR, "a.txt", 10, 1),
            file(DIR, "b.log", 20, 2),
            file(DIR, "c.txt", 30, 3),
            hidden(DIR, ".secret"),
        ],
    );
    h.go(cx, DIR);
    sorted_by_size(&h, cx);
    h.run(cx, |e, cx| e.set_filter("txt", None, cx));
    assert_eq!(h.names(cx), names(&["c.txt", "a.txt"]));
    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    assert_eq!(h.names(cx), names(&["c.txt", "a.txt"]), "the filter still narrows");
    h.run(cx, |e, cx| e.set_filter("", None, cx));
    assert_eq!(
        h.names(cx),
        names(&["c.txt", "a.txt", "b.log"]),
        "what the filter hid comes back after the rows that were shown"
    );
}

#[gpui_kit::test]
fn a_file_that_comes_back_takes_its_old_place(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    busy_folder(&h.disk);
    h.go(cx, DIR);
    sorted_by_size(&h, cx);
    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    // An editor saves by deleting and writing again: the file is gone in one read and back in the next.
    h.disk.folder(
        DIR,
        vec![
            file(DIR, "a.txt", 10, 1),
            file(DIR, "c.txt", 30, 3),
            file(DIR, "d.txt", 40, 4),
        ],
    );
    h.reread(cx);
    assert_eq!(h.names(cx), names(&["d.txt", "c.txt", "b.txt", "a.txt"]));
    busy_folder(&h.disk);
    h.reread(cx);
    assert_eq!(h.names(cx), names(&["d.txt", "c.txt", "b.txt", "a.txt"]));
    h.explorer.read_with(cx, |e, _| {
        assert_eq!(e.frozen_info(e.active_tab_id()), Some(FrozenInfo::default()));
        assert!(!e.cut_paths().contains("C:\\busy\\b.txt"));
    });
}

#[gpui_kit::test]
fn arabic_names_keep_their_places_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    h.disk
        .folder(DIR, vec![file(DIR, "تقرير.pdf", 30, 1), file(DIR, "ملاحظات.txt", 20, 2)]);
    h.go(cx, DIR);
    sorted_by_size(&h, cx);
    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    h.disk.folder(
        DIR,
        vec![
            file(DIR, "تقرير.pdf", 5, 1),
            file(DIR, "ملاحظات.txt", 20, 2),
            file(DIR, "جديد.txt", 50, 3),
        ],
    );
    h.reread(cx);
    assert_eq!(h.names(cx), names(&["تقرير.pdf", "ملاحظات.txt", "جديد.txt"]));
    assert_eq!(h.size_of(cx, "تقرير.pdf"), Some(5));
}

/// Costs nothing while off: the hooks are an emptiness check. Prints what a re-read of a big frozen folder costs,
/// for the record.
#[gpui_kit::test]
fn a_big_folder_costs_one_pass_a_re_read_while_frozen_and_nothing_while_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let big: Vec<_> = (0..100_000)
        .map(|i| file(DIR, &format!("file{i:06}.dat"), i % 977, i as i64))
        .collect();
    h.disk.folder(DIR, big.clone());
    h.go(cx, DIR);
    h.explorer
        .read_with(cx, |e, _| assert!(e.freezes.is_empty(), "nothing is held while off"));

    sorted_by_size(&h, cx);
    h.run(cx, |e, cx| e.set_frozen(None, true, cx));
    let mut changed = big;
    changed.push(file(DIR, "zzz.dat", 1, 1));
    h.disk.folder(DIR, changed);
    let started = std::time::Instant::now();
    h.reread(cx);
    println!(
        "re-read of 100,000 frozen entries, with the fake disk's sort: {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.
    );
    assert_eq!(h.names(cx).last().map(String::as_str), Some("zzz.dat"));
}
