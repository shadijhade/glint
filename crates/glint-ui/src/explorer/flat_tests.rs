//! Headless tests of the flat view (Ctrl+B): every file below a folder as one list, on a fake disk behind the
//! same `Source` the app uses. It is a search with no words, so these check what makes it a flat view: what is
//! asked, what is listed, how it turns on and off, and that sorting, the grid and Back and Forward follow.

use super::fake::*;
use super::*;
use crate::testing;
use glint_model::entries::{Sort, SortKey};
use glint_model::paths::{RECYCLE_BIN, THIS_PC};
use glint_model::store::ViewMode;
use gpui_kit::{AppContext as _, Entity, TestAppContext};

struct Rig {
    explorer: Entity<Explorer>,
    disk: FakeDisk,
}

fn rig(cx: &mut TestAppContext) -> Rig {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
    Rig { explorer, disk }
}

/// `C:\` with `a` (two files, a hidden one and a folder `deep` below it) and `b` (two files).
fn tree(disk: &FakeDisk) {
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), file("C:\\", "top.txt", 5, 50)],
    );
    disk.folder(
        "C:\\a",
        vec![
            file("C:\\a", "small.txt", 10, 300),
            hidden("C:\\a", "secret.txt"),
            folder("C:\\a", "deep"),
        ],
    );
    disk.folder("C:\\a\\deep", vec![file("C:\\a\\deep", "huge.bin", 9_000, 100)]);
    disk.folder(
        "C:\\b",
        vec![file("C:\\b", "mid.txt", 400, 200), file("C:\\b", "تقرير.pdf", 40, 400)],
    );
}

impl Rig {
    fn go(&self, cx: &mut TestAppContext, path: &str) {
        let task = self
            .explorer
            .update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        assert!(cx.foreground_executor().block_test(task));
    }

    /// Ctrl+B, and waits for the listing to end.
    fn toggle(&self, cx: &mut TestAppContext) {
        let task = self.explorer.update(cx, |e, cx| e.toggle_flat_view(cx));
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
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

    fn run_of(&self, cx: &TestAppContext) -> Option<FindRun> {
        self.read(cx, |e| e.active_tab().and_then(|t| t.find.clone()))
    }

    fn flat(&self, cx: &TestAppContext) -> bool {
        self.read(cx, |e| e.is_flat(e.active_tab_id()))
    }

    fn sort(&self, cx: &mut TestAppContext, key: SortKey, desc: bool) {
        self.explorer.update(cx, |e, cx| e.set_sort(Sort { key, desc }, cx));
        cx.run_until_parked();
    }
}

fn indexed(drives: &[&str]) -> SearchIndex {
    SearchIndex {
        on: true,
        ready: drives.iter().map(|d| d.to_string()).collect(),
        ..Default::default()
    }
}

#[gpui_kit::test]
fn the_flat_view_lists_every_file_below_the_folder_and_no_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    assert!(!r.flat(cx));
    r.toggle(cx);
    assert!(r.flat(cx));
    // Every file of every level, none of the folders, the hidden one left out as Settings say.
    let mut names = r.names(cx);
    names.sort();
    assert_eq!(names, ["huge.bin", "mid.txt", "small.txt", "top.txt", "تقرير.pdf"]);
    let run = r.run_of(cx).expect("the flat view is a run");
    assert!(run.flat && run.query.is_empty() && !run.running && run.error.is_empty());
    assert_eq!(run.scope, "C:\\");
    // What the engine was asked: every file, from the folder, with nothing to rank or to mistype.
    let call = r.disk.finds().pop().expect("a listing was asked for");
    assert_eq!(call.query, "files:");
    assert_eq!(call.roots, ["C:\\".to_string()]);
    assert!(!call.options.rank && !call.options.fuzzy);
    // The tab is a search's: the file list gives it the Folder column, which says where each file is.
    r.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert!(tab.view_key().starts_with("find:"));
        let deep = tab
            .rows
            .iter()
            .find(|row| row.name == "huge.bin")
            .expect("a file two levels down");
        assert_eq!(glint_model::paths::parent_path(&deep.path), "C:\\a\\deep");
    });
}

#[gpui_kit::test]
fn the_same_key_again_ends_it_and_the_folder_comes_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.toggle(cx);
    assert!(r.flat(cx));
    r.toggle(cx);
    assert!(!r.flat(cx) && r.run_of(cx).is_none());
    assert_eq!(r.names(cx), ["a", "b", "top.txt"], "the folder itself, as it was");
}

#[gpui_kit::test]
fn opening_another_folder_ends_the_flat_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.toggle(cx);
    r.go(cx, "C:\\b");
    assert!(!r.flat(cx));
    assert_eq!(r.names(cx), ["mid.txt", "تقرير.pdf"]);
    // And the key works again where it is now.
    r.toggle(cx);
    assert_eq!(r.names(cx).len(), 2);
    assert_eq!(r.run_of(cx).map(|run| run.scope), Some("C:\\b".to_string()));
}

#[gpui_kit::test]
fn it_is_for_folders_only(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.disk.folder(RECYCLE_BIN, vec![]);
    r.go(cx, THIS_PC);
    r.toggle(cx);
    assert!(
        !r.flat(cx) && r.run_of(cx).is_none(),
        "every file of every drive is no view"
    );
    assert!(r.disk.finds().is_empty());
    r.go(cx, RECYCLE_BIN);
    r.toggle(cx);
    assert!(!r.flat(cx) && r.disk.finds().is_empty());
    r.go(cx, "C:\\");
    assert!(r.read(cx, |e| e.can_flatten(e.active_tab_id())));
}

#[gpui_kit::test]
fn the_whole_subtree_sorts_by_size_and_by_date(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.toggle(cx);
    r.sort(cx, SortKey::Size, true);
    assert_eq!(
        r.names(cx),
        ["huge.bin", "mid.txt", "تقرير.pdf", "small.txt", "top.txt"]
    );
    r.sort(cx, SortKey::Size, false);
    assert_eq!(
        r.names(cx),
        ["top.txt", "small.txt", "تقرير.pdf", "mid.txt", "huge.bin"]
    );
    r.sort(cx, SortKey::Modified, true);
    assert_eq!(
        r.names(cx),
        ["تقرير.pdf", "small.txt", "mid.txt", "huge.bin", "top.txt"]
    );
    r.sort(cx, SortKey::Modified, false);
    assert_eq!(
        r.names(cx),
        ["top.txt", "huge.bin", "mid.txt", "small.txt", "تقرير.pdf"]
    );
}

#[gpui_kit::test]
fn the_grid_shows_the_flat_list_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.toggle(cx);
    let list = r.names(cx);
    r.explorer.update(cx, |e, cx| e.set_view(ViewMode::Grid, None, cx));
    cx.run_until_parked();
    assert_eq!(r.read(cx, |e| e.active_tab().map(|t| t.view)), Some(ViewMode::Grid));
    assert!(r.flat(cx), "the view is a way to draw the list, not a way out of it");
    assert_eq!(r.names(cx), list);
}

#[gpui_kit::test]
fn files_stream_in_and_a_big_tree_stops_at_the_limit(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    let live = r.disk.live_find("files:");
    let waiting = r.explorer.update(cx, |e, cx| e.toggle_flat_view(cx));
    cx.run_until_parked();
    // Running with nothing yet: the list says so, and the key still ends it.
    let run = r.run_of(cx).expect("the flat view is shown at once");
    assert!(run.running && run.entries.is_empty());
    live.try_send(Found::Started { engine: "index".into() }).expect("open");
    live.try_send(Found::Entries(vec![
        file("C:\\a", "one.txt", 1, 1),
        file("C:\\b", "two.txt", 2, 2),
    ]))
    .expect("open");
    cx.run_until_parked();
    let run = r.run_of(cx).expect("a run");
    assert!(run.running, "progress: still listing");
    assert_eq!((run.engine.as_str(), r.names(cx).len()), ("index", 2));
    live.try_send(Found::Entries(vec![file("C:\\a\\deep", "three.txt", 3, 3)]))
        .expect("open");
    live.try_send(Found::Finished {
        count: 3,
        elapsed: 4.0,
        limited: true,
        fuzzy: false,
    })
    .expect("open");
    cx.foreground_executor().block_test(waiting);
    cx.run_until_parked();
    let run = r.run_of(cx).expect("a run");
    assert!(!run.running && run.limited, "a listing cut at Settings' limit says so");
    assert_eq!(r.names(cx).len(), 3);
}

#[gpui_kit::test]
fn the_index_answers_where_it_has_the_drive_and_fd_where_it_does_not(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.disk.set_search_index(Some(indexed(&["C:"])));
    r.toggle(cx);
    assert_eq!(r.run_of(cx).map(|run| run.engine), Some("index".to_string()));
    r.toggle(cx);
    // The index has other drives only: the same folder is listed by fd, with the same files.
    r.disk.set_search_index(Some(indexed(&["D:"])));
    r.toggle(cx);
    let run = r.run_of(cx).expect("a run");
    assert_eq!(run.engine, "fd");
    assert_eq!(r.names(cx).len(), 5);
}

#[gpui_kit::test]
fn a_listing_that_fails_says_so_and_leaving_it_goes_back_to_the_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.disk.refuse_next_find("fd is gone");
    r.toggle(cx);
    let run = r.run_of(cx).expect("a run");
    assert!(run.flat && run.error == "fd is gone" && run.entries.is_empty());
    r.toggle(cx);
    assert!(r.run_of(cx).is_none());
    assert_eq!(r.names(cx), ["a", "b", "top.txt"]);
}

#[gpui_kit::test]
fn the_flat_view_is_a_place_in_back_and_forward(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.toggle(cx);
    let back = r.explorer.update(cx, |e, cx| e.go_back(cx));
    assert!(cx.foreground_executor().block_test(back));
    cx.run_until_parked();
    assert!(!r.flat(cx), "Back from it is the folder");
    assert_eq!(r.names(cx), ["a", "b", "top.txt"]);
    let forward = r.explorer.update(cx, |e, cx| e.go_forward(cx));
    assert!(cx.foreground_executor().block_test(forward));
    cx.run_until_parked();
    assert!(r.flat(cx), "Forward brings the listing back");
    assert_eq!(r.names(cx).len(), 5);
}

#[gpui_kit::test]
fn f5_lists_the_files_again_still_flat(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.toggle(cx);
    let task = r.explorer.update(cx, |e, cx| e.rerun_find(cx));
    cx.foreground_executor().block_test(task);
    cx.run_until_parked();
    assert!(r.flat(cx));
    assert_eq!(r.disk.finds().len(), 2);
    assert_eq!(r.disk.finds()[1].query, "files:");
}

#[gpui_kit::test]
fn batch_rename_takes_files_from_every_subfolder_at_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    tree(&r.disk);
    r.go(cx, "C:\\");
    r.toggle(cx);
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::<Vec<String>>::new()));
    let sink = asked.clone();
    cx.update(|cx| {
        cx.subscribe(&r.explorer, move |_, event: &ExplorerEvent, _| {
            if let ExplorerEvent::BatchRename(paths) = event {
                sink.borrow_mut().push(paths.clone());
            }
        })
        .detach();
    });
    // Ctrl+A, then F2 (several selected: all at once).
    r.explorer.update(cx, |e, cx| {
        let rows = e.active_tab().expect("a tab").rows.clone();
        e.select_all(&rows, cx);
        e.begin_rename(None, cx);
    });
    cx.run_until_parked();
    let asked = asked.borrow();
    assert_eq!(asked.len(), 1, "the dialog is asked for once");
    let mut folders: Vec<String> = asked[0].iter().map(|p| glint_model::paths::parent_path(p)).collect();
    folders.sort();
    folders.dedup();
    assert_eq!(folders, ["C:\\", "C:\\a", "C:\\a\\deep", "C:\\b"]);
    assert_eq!(asked[0].len(), 5);
}
