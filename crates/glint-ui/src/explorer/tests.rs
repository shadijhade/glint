//! Headless tests of the explorer model: a fake disk behind the same `Source` the app uses, and one test that
//! lists a real temporary folder through glint-core. No window is opened.

use super::fake::*;
use super::*;
use crate::testing;
use glint_model::entries::{Sort, SortKey};
use glint_model::groups::GroupBy;
use glint_model::store::{Session, SessionPane, StartFolder};
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// An explorer on a fake disk, and every event it raised.
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
    Harness { explorer, disk, events }
}

/// `C:\` with folders `a`, `b` and `c`, each with a few files; `C:\a` also has a subfolder.
fn standard_disk(disk: &FakeDisk) {
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), folder("C:\\", "c")],
    );
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "sub"),
            file("C:\\a", "file10.txt", 100, 10),
            file("C:\\a", "file2.txt", 300, 30),
            file("C:\\a", "notes.md", 200, 20),
            hidden("C:\\a", ".secret"),
        ],
    );
    disk.folder("C:\\a\\sub", vec![file("C:\\a\\sub", "deep.txt", 1, 1)]);
    disk.folder(
        "C:\\b",
        vec![file("C:\\b", "b1.txt", 1, 1), file("C:\\b", "b2.txt", 2, 2)],
    );
    disk.folder("C:\\c", vec![]);
}

impl Harness {
    fn go(&self, cx: &mut TestAppContext, path: &str) -> bool {
        self.go_with(cx, path, NavOptions::default())
    }

    fn go_with(&self, cx: &mut TestAppContext, path: &str, options: NavOptions) -> bool {
        let task = self.explorer.update(cx, |e, cx| e.navigate_task(path, options, cx));
        cx.foreground_executor().block_test(task)
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

    fn path(&self, cx: &TestAppContext) -> String {
        self.read(cx, |e| e.current_path().to_string())
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

    fn history(&self, cx: &TestAppContext) -> (Vec<String>, Vec<String>) {
        self.read(cx, |e| {
            let tab = e.active_tab();
            (
                tab.map(|t| t.back.clone()).unwrap_or_default(),
                tab.map(|t| t.forward.clone()).unwrap_or_default(),
            )
        })
    }

    fn notices(&self) -> Vec<Notice> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                ExplorerEvent::Notice(n) => Some(n.clone()),
                _ => None,
            })
            .collect()
    }

    fn tab_paths(&self, cx: &TestAppContext, pane: usize) -> Vec<String> {
        self.read(cx, |e| {
            e.panes()
                .get(pane)
                .map(|p| {
                    p.tabs
                        .iter()
                        .map(|id| e.tab(*id).map(|t| t.path.clone()).unwrap_or_default())
                        .collect()
                })
                .unwrap_or_default()
        })
    }
}

fn sort_by(key: SortKey, desc: bool) -> Sort {
    Sort { key, desc }
}

// ---- Navigation and history ---------------------------------------------------------------------------------

#[gpui_kit::test]
fn navigating_lists_the_folder_and_the_first_folder_is_not_in_history(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    assert_eq!(h.path(cx), "C:\\a");
    // The first folder a tab opens doesn't go into Back history.
    assert_eq!(h.history(cx), (vec![], vec![]));
    // Folders first, natural order ("file2" before "file10"), hidden items left out.
    assert_eq!(h.names(cx), ["sub", "file2.txt", "file10.txt", "notes.md"]);
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert!(tab.ready && !tab.loading);
        assert_eq!(
            tab.load,
            LoadState::Ready(Loaded {
                read_ms: 1.5,
                engine: Engine::Disk
            })
        );
        assert_eq!(tab.rows.hidden, 1);
        assert_eq!(tab.entries().len(), 5, "the listing itself keeps every entry");
    });
}

#[gpui_kit::test]
fn back_forward_and_up_walk_the_history(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\"));
    assert!(h.go(cx, "C:\\a"));
    assert!(h.go(cx, "C:\\a\\sub"));
    assert_eq!(h.history(cx), (vec!["C:\\".into(), "C:\\a".into()], vec![]));

    let back = h.explorer.update(cx, |e, cx| e.go_back(cx));
    assert!(cx.foreground_executor().block_test(back));
    assert_eq!(h.path(cx), "C:\\a");
    assert_eq!(h.history(cx), (vec!["C:\\".into()], vec!["C:\\a\\sub".into()]));

    let back = h.explorer.update(cx, |e, cx| e.go_back(cx));
    assert!(cx.foreground_executor().block_test(back));
    assert_eq!(h.path(cx), "C:\\");
    assert_eq!(h.history(cx), (vec![], vec!["C:\\a".into(), "C:\\a\\sub".into()]));
    // Nothing further back.
    let back = h.explorer.update(cx, |e, cx| e.go_back(cx));
    assert!(!cx.foreground_executor().block_test(back));

    let forward = h.explorer.update(cx, |e, cx| e.go_forward(cx));
    assert!(cx.foreground_executor().block_test(forward));
    assert_eq!(h.path(cx), "C:\\a");
    assert_eq!(h.history(cx), (vec!["C:\\".into()], vec!["C:\\a\\sub".into()]));

    // A new navigation forgets the forward list.
    assert!(h.go(cx, "C:\\b"));
    assert_eq!(h.history(cx), (vec!["C:\\".into(), "C:\\a".into()], vec![]));
}

#[gpui_kit::test]
fn going_up_selects_the_folder_we_came_from_and_stops_at_this_pc(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let up = h.explorer.update(cx, |e, cx| e.go_up(cx));
    assert!(cx.foreground_executor().block_test(up));
    assert_eq!(h.path(cx), "C:\\");
    assert_eq!(h.selected(cx), ["a"]);
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert_eq!(tab.selection.focus(), Some("C:\\a"));
        assert_eq!(tab.selection.anchor(), Some("C:\\a"));
    });
    // The drive's parent is This PC; there is nothing above that.
    let up = h.explorer.update(cx, |e, cx| e.go_up(cx));
    assert!(cx.foreground_executor().block_test(up));
    assert_eq!(h.path(cx), "");
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert!(tab.listing.is_none());
        assert_eq!(
            tab.load,
            LoadState::Ready(Loaded {
                read_ms: 0.0,
                engine: Engine::ThisPc
            })
        );
    });
    let up = h.explorer.update(cx, |e, cx| e.go_up(cx));
    assert!(!cx.foreground_executor().block_test(up));
}

#[gpui_kit::test]
fn navigating_with_select_selects_and_flashes_it_then_the_flash_fades(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    // The path is matched ignoring case, and the listing's own spelling is kept.
    assert!(h.go_with(cx, "C:\\a", NavOptions::default().selecting("c:\\A\\NOTES.md")));
    assert_eq!(h.selected(cx), ["notes.md"]);
    h.read(cx, |e| assert!(e.flashed().contains("C:\\a\\notes.md")));
    cx.executor().advance_clock(Duration::from_millis(1300));
    cx.run_until_parked();
    h.read(cx, |e| assert!(e.flashed().is_empty()));
    assert!(h.events.borrow().contains(&ExplorerEvent::FlashChanged));
}

#[gpui_kit::test]
fn a_superseded_navigation_is_dropped(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\"));
    // `a` takes its time; the person clicks `b` meanwhile.
    let gate = h.disk.hold("C:\\a");
    let slow = h
        .explorer
        .update(cx, |e, cx| e.navigate_task("C:\\a", NavOptions::default(), cx));
    h.read(cx, |e| assert!(e.active_tab().expect("a tab").loading));
    assert!(h.go(cx, "C:\\b"));
    assert_eq!(h.path(cx), "C:\\b");
    // Now `a` arrives: it must change nothing, and its task says it lost.
    gate.close();
    assert!(!cx.foreground_executor().block_test(slow));
    assert_eq!(h.path(cx), "C:\\b");
    assert_eq!(h.names(cx), ["b1.txt", "b2.txt"]);
    assert_eq!(
        h.history(cx),
        (vec!["C:\\".into()], vec![]),
        "only the folder that opened is in history"
    );
    h.read(cx, |e| assert!(!e.active_tab().expect("a tab").loading));
}

#[gpui_kit::test]
fn a_failed_navigation_leaves_the_tab_alone_and_says_why_unless_silent(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    // A tab that never opened a folder shows the failure itself.
    assert!(!h.go(cx, "C:\\nowhere"));
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert!(matches!(tab.load, LoadState::Failed(_)));
        assert!(!tab.loading);
    });
    assert!(matches!(h.notices().as_slice(), [Notice::CannotOpen { path, .. }] if path == "C:\\nowhere"));
    assert!(h.go(cx, "C:\\a"));
    // One that shows a folder keeps it.
    assert!(!h.go(cx, "C:\\gone"));
    assert_eq!(h.path(cx), "C:\\a");
    h.read(cx, |e| {
        assert!(matches!(e.active_tab().expect("a tab").load, LoadState::Ready(_)))
    });
    assert_eq!(h.notices().len(), 2);
    // Silent: the caller handles it.
    assert!(!h.go_with(
        cx,
        "C:\\gone",
        NavOptions {
            silent: true,
            ..Default::default()
        }
    ));
    assert_eq!(h.notices().len(), 2);
}

#[gpui_kit::test]
fn a_folder_windows_keeps_closed_opens_with_its_reason_and_get_access_opens_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    h.disk.folder("C:\\secret", vec![file("C:\\secret", "plans.txt", 5, 5)]);
    h.disk.deny("C:\\secret");
    assert!(h.go(cx, "C:\\a"));
    assert!(h.go(cx, "C:\\secret"));
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert_eq!(tab.path, "C:\\secret");
        assert!(tab.listing.is_none());
        assert_eq!(tab.no_access(), Some(&NoAccess::default()));
        assert!(e.closed_of(None));
    });
    assert_eq!(h.history(cx).0, ["C:\\a"], "a closed folder is still a place you went");
    // Get access: Windows says yes, and the folder opens in place (not into history).
    let access = h.explorer.update(cx, |e, cx| e.get_access(None, cx));
    cx.foreground_executor().block_test(access);
    cx.run_until_parked();
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert!(tab.no_access().is_none());
        assert_eq!(tab.back, ["C:\\a"]);
    });
}

#[gpui_kit::test]
fn reloading_keeps_the_selection_and_a_vanished_folder_opens_its_parent(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\notes.md"), cx));
    // A file arrives on disk; the folder is read again in place.
    h.disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "sub"),
            file("C:\\a", "file2.txt", 300, 30),
            file("C:\\a", "notes.md", 200, 20),
            file("C:\\a", "new.txt", 7, 7),
        ],
    );
    let reload = h.explorer.update(cx, |e, cx| e.reload(None, cx));
    cx.foreground_executor().block_test(reload);
    assert_eq!(h.names(cx), ["sub", "file2.txt", "new.txt", "notes.md"]);
    assert_eq!(h.selected(cx), ["notes.md"]);
    assert!(h
        .events
        .borrow()
        .iter()
        .any(|e| matches!(e, ExplorerEvent::ListingChanged { same: true, .. })));
    assert_eq!(h.history(cx).0, Vec::<String>::new(), "a re-read is not a visit");

    // The folder is deleted: its parent opens, and the person is told.
    h.disk.remove("C:\\a");
    let reload = h.explorer.update(cx, |e, cx| e.reload(None, cx));
    cx.foreground_executor().block_test(reload);
    cx.run_until_parked();
    assert_eq!(h.path(cx), "C:\\");
    assert!(h
        .notices()
        .contains(&Notice::NoLongerAvailable { path: "C:\\a".into() }));
}

#[gpui_kit::test]
fn core_dir_changed_rereads_the_tabs_showing_that_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\b"));
    assert_eq!(h.names(cx).len(), 2);
    h.disk.folder("C:\\b", vec![file("C:\\b", "b1.txt", 1, 1)]);
    // The event comes from core's watchers, with the folder's path (any case).
    cx.update(|cx| crate::core_bridge::emit(cx, "dir-changed", serde_json::json!("c:\\B")));
    for _ in 0..50 {
        cx.run_until_parked();
        if h.names(cx).len() == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.names(cx), ["b1.txt"]);
    // Another folder's change is none of this tab's business.
    let before = h.disk.listed().len();
    cx.update(|cx| crate::core_bridge::emit(cx, "dir-changed", serde_json::json!("C:\\a")));
    for _ in 0..10 {
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.disk.listed().len(), before);
}

/// Waits for a "dir-changed" to have reached a tab that is on its way somewhere: its read is waiting.
fn wait_for_a_read_to_wait(h: &Harness, cx: &mut TestAppContext) {
    for _ in 0..50 {
        cx.run_until_parked();
        if h.read(cx, |e| e.active_tab().is_some_and(|tab| tab.reload_waiting)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the change on disk never reached the tab");
}

#[gpui_kit::test]
fn a_change_on_disk_does_not_take_the_tab_back_from_a_folder_being_opened(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\"));
    // Enter on a folder: its listing is on its way.
    let gate = h.disk.hold("C:\\a");
    let opening = h
        .explorer
        .update(cx, |e, cx| e.navigate_task("C:\\a", NavOptions::default(), cx));
    cx.run_until_parked();
    // The folder being left changes meanwhile. Reading it again then would be the tab's newer navigation, and
    // the folder asked for would never open.
    cx.update(|cx| crate::core_bridge::emit(cx, "dir-changed", serde_json::json!("C:\\")));
    wait_for_a_read_to_wait(&h, cx);
    gate.close();
    assert!(
        cx.foreground_executor().block_test(opening),
        "the folder asked for opens"
    );
    cx.run_until_parked();
    assert_eq!(h.path(cx), "C:\\a");
    assert_eq!(h.history(cx).0, ["C:\\"]);
    h.read(cx, |e| {
        assert!(
            !e.active_tab().expect("a tab").reload_waiting,
            "the folder left behind is not read again"
        )
    });
    let read = h.disk.listed().len();
    cx.run_until_parked();
    assert_eq!(h.disk.listed().len(), read);

    // A folder that cannot be opened leaves the tab where it was: what changed there meanwhile is read then.
    let gate = h.disk.hold("C:\\gone");
    let opening = h
        .explorer
        .update(cx, |e, cx| e.navigate_task("C:\\gone", NavOptions::default(), cx));
    cx.run_until_parked();
    h.disk.folder("C:\\a", vec![file("C:\\a", "new.txt", 7, 7)]);
    cx.update(|cx| crate::core_bridge::emit(cx, "dir-changed", serde_json::json!("C:\\a")));
    wait_for_a_read_to_wait(&h, cx);
    gate.close();
    assert!(!cx.foreground_executor().block_test(opening), "there is no such folder");
    for _ in 0..50 {
        cx.run_until_parked();
        if h.names(cx) == ["new.txt"] {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.path(cx), "C:\\a");
    assert_eq!(h.names(cx), ["new.txt"]);
}

#[gpui_kit::test]
fn a_folder_read_ahead_opens_without_reading_again_unless_it_changed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\"));
    // Read ahead, then opened: one read in all.
    h.run(cx, |e, cx| e.prefetch_folder("C:\\a", cx));
    assert_eq!(h.disk.listed(), ["C:\\", "C:\\a"]);
    assert!(h.go(cx, "C:\\a"));
    assert_eq!(h.disk.listed(), ["C:\\", "C:\\a"], "the read-ahead listing was used");
    assert_eq!(h.names(cx).len(), 4);
    // It is used once.
    assert!(h.go(cx, "C:\\a"));
    assert_eq!(h.disk.listed().len(), 3);
    // The folder changed after it was read ahead: its date differs, so it is read again.
    h.run(cx, |e, cx| e.prefetch_folder("C:\\b", cx));
    h.disk.touch("C:\\b");
    assert!(h.go(cx, "C:\\b"));
    assert_eq!(h.disk.listed().iter().filter(|p| *p == "C:\\b").count(), 2);
    // Archives and places aren't read ahead.
    h.run(cx, |e, cx| e.prefetch_folder("C:\\x.zip\\inside", cx));
    h.run(cx, |e, cx| e.prefetch_folder("shell:Recent", cx));
    assert_eq!(
        h.disk
            .listed()
            .iter()
            .filter(|p| p.contains("zip") || p.contains("Recent"))
            .count(),
        0
    );
}

#[gpui_kit::test]
fn history_keeps_the_last_hundred_folders(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    for i in 0..105 {
        h.disk.folder(&format!("C:\\f{i}"), vec![]);
    }
    for i in 0..105 {
        assert!(h.go(cx, &format!("C:\\f{i}")));
    }
    let (back, _) = h.history(cx);
    assert_eq!(back.len(), 100);
    assert_eq!(back.first().map(String::as_str), Some("C:\\f4"));
    assert_eq!(back.last().map(String::as_str), Some("C:\\f103"));
}

// ---- Tabs and panes -------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn new_tabs_open_beside_the_current_one_and_closing_picks_a_neighbour(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let first = h.read(cx, |e| e.active_tab_id());
    h.run(cx, |e, cx| e.new_tab(None, cx));
    let second = h.read(cx, |e| e.active_tab_id());
    assert_ne!(first, second);
    assert_eq!(h.path(cx), "C:\\a", "a new tab opens on the open folder");
    assert!(h.go(cx, "C:\\b"));
    h.run(cx, |e, cx| e.new_tab(Some("C:\\c"), cx));
    let third = h.read(cx, |e| e.active_tab_id());
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a", "C:\\b", "C:\\c"]);

    // Closing the active tab shows the one that takes its place; the last of three shows the one before.
    h.run(cx, |e, cx| e.activate_tab(second, cx));
    h.run(cx, |e, cx| e.close_tab(None, cx));
    assert_eq!(h.read(cx, |e| e.active_tab_id()), third);
    h.run(cx, |e, cx| e.close_tab(None, cx));
    assert_eq!(h.read(cx, |e| e.active_tab_id()), first);
    // The last tab of all stays.
    h.run(cx, |e, cx| e.close_tab(None, cx));
    assert_eq!(h.read(cx, |e| e.panes()[0].tabs.len()), 1);
    assert!(h.read(cx, |e| e.tab(second).is_none()), "a closed tab's state is gone");
}

#[gpui_kit::test]
fn ctrl_shift_t_brings_back_closed_tabs_where_they_were_newest_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    for path in ["C:\\b", "C:\\c"] {
        h.run(cx, |e, cx| e.new_tab(Some(path), cx));
    }
    assert!(!h.read(cx, |e| e.can_reopen_tab()));
    let ids: Vec<TabId> = h.read(cx, |e| e.panes()[0].tabs.clone());
    h.run(cx, |e, cx| e.close_tab(Some(ids[1]), cx)); // b
    h.run(cx, |e, cx| e.close_tab(Some(ids[0]), cx)); // a
    assert_eq!(h.tab_paths(cx, 0), ["C:\\c"]);
    assert!(h.read(cx, |e| e.can_reopen_tab()));
    h.run(cx, |e, cx| e.reopen_closed_tab(cx));
    // `a` was first; it comes back first, and shows.
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a", "C:\\c"]);
    assert_eq!(h.path(cx), "C:\\a");
    h.run(cx, |e, cx| e.reopen_closed_tab(cx));
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a", "C:\\b", "C:\\c"]);
    assert!(!h.read(cx, |e| e.can_reopen_tab()));
}

#[gpui_kit::test]
fn cycling_and_numbered_tab_keys_stay_inside_the_active_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    h.run(cx, |e, cx| e.new_tab(Some("C:\\c"), cx));
    let ids: Vec<TabId> = h.read(cx, |e| e.panes()[0].tabs.clone());
    assert_eq!(h.read(cx, |e| e.active_tab_id()), ids[2]);
    h.run(cx, |e, cx| e.cycle_tab(1, cx));
    assert_eq!(h.read(cx, |e| e.active_tab_id()), ids[0], "wraps around");
    h.run(cx, |e, cx| e.cycle_tab(-1, cx));
    assert_eq!(h.read(cx, |e| e.active_tab_id()), ids[2]);
    h.run(cx, |e, cx| e.select_tab(2, cx));
    assert_eq!(h.read(cx, |e| e.active_tab_id()), ids[1]);
    h.run(cx, |e, cx| e.select_tab(9, cx));
    assert_eq!(h.read(cx, |e| e.active_tab_id()), ids[2], "Ctrl+9 is the last tab");
    h.run(cx, |e, cx| e.select_tab(5, cx));
    assert_eq!(
        h.read(cx, |e| e.active_tab_id()),
        ids[2],
        "no fifth tab: nothing happens"
    );
}

#[gpui_kit::test]
fn duplicating_and_opening_in_new_tabs_place_tabs_like_a_browser(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.set_view(ViewMode::Grid, None, cx));
    let first = h.read(cx, |e| e.active_tab_id());
    h.run(cx, |e, cx| e.duplicate_tab(first, cx));
    h.read(cx, |e| {
        assert_eq!(e.panes()[0].tabs.len(), 2);
        assert_eq!(
            e.active_tab().map(|t| (t.path.as_str(), t.view)),
            Some(("C:\\a", ViewMode::Grid))
        );
    });
    // Middle-clicks: background tabs go after each other, and the active tab stays.
    h.run(cx, |e, cx| e.activate_tab(first, cx));
    h.run(cx, |e, cx| e.open_in_new_tab("C:\\b", cx));
    h.run(cx, |e, cx| e.open_in_new_tab("C:\\c", cx));
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a", "C:\\b", "C:\\c", "C:\\a"]);
    assert_eq!(h.read(cx, |e| e.active_tab_id()), first);
}

#[gpui_kit::test]
fn dual_pane_view_keeps_the_right_panes_tabs_while_hidden(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.toggle_dual(cx));
    h.read(cx, |e| {
        assert!(e.is_dual());
        assert_eq!(e.active_pane(), 1);
        assert_eq!(e.visible_tabs().len(), 2);
    });
    assert_eq!(
        h.tab_paths(cx, 1),
        ["C:\\a"],
        "the new pane starts on the folder you're in"
    );
    assert!(h.go(cx, "C:\\b"));
    h.run(cx, |e, cx| e.focus_pane(0, cx));
    assert_eq!(h.path(cx), "C:\\a");
    h.run(cx, |e, cx| e.switch_pane(cx));
    assert_eq!(h.path(cx), "C:\\b");
    // Off, and on again: the right pane still has its folder.
    h.run(cx, |e, cx| e.toggle_dual(cx));
    h.read(cx, |e| {
        assert!(!e.is_dual());
        assert_eq!(e.active_pane(), 0);
        assert_eq!(e.visible_tabs().len(), 1);
        assert_eq!(e.panes().len(), 2);
    });
    h.run(cx, |e, cx| e.toggle_dual(cx));
    assert_eq!(h.path(cx), "C:\\b");
    // The panes trade places; the keyboard stays with its folder.
    h.run(cx, |e, cx| e.swap_panes(cx));
    assert_eq!(h.tab_paths(cx, 0), ["C:\\b"]);
    assert_eq!(h.read(cx, |e| e.active_pane()), 0);
    assert_eq!(h.path(cx), "C:\\b");
    // Closing a pane's only tab closes the pane.
    h.run(cx, |e, cx| e.close_tab(None, cx));
    h.read(cx, |e| {
        assert!(!e.is_dual());
        assert_eq!(e.panes().len(), 1);
    });
    assert_eq!(h.path(cx), "C:\\a");
}

#[gpui_kit::test]
fn tabs_move_between_panes_but_never_a_panes_only_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.toggle_dual(cx));
    assert!(h.go(cx, "C:\\b"));
    h.run(cx, |e, cx| e.focus_pane(0, cx));
    h.run(cx, |e, cx| e.new_tab(Some("C:\\c"), cx));
    let left: Vec<TabId> = h.read(cx, |e| e.panes()[0].tabs.clone());
    let right_only = h.read(cx, |e| e.panes()[1].tabs[0]);
    // The right pane's only tab can't leave.
    h.run(cx, |e, cx| e.move_tab_to_other_pane(right_only, cx));
    assert_eq!(h.tab_paths(cx, 1), ["C:\\b"]);
    // The left pane has two: `c` crosses over and becomes active there.
    h.run(cx, |e, cx| e.move_tab_to_other_pane(left[1], cx));
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a"]);
    assert_eq!(h.tab_paths(cx, 1), ["C:\\b", "C:\\c"]);
    assert_eq!(h.read(cx, |e| e.active_tab_id()), left[1]);
    assert_eq!(h.read(cx, |e| e.active_pane()), 1);
    // Reordering inside a pane: drag `c` before `b`.
    h.run(cx, |e, cx| e.place_tab(left[1], 1, 0, cx));
    assert_eq!(h.tab_paths(cx, 1), ["C:\\c", "C:\\b"]);
    // And drop it on the left pane's files.
    h.run(cx, |e, cx| e.move_tab_to_pane(left[1], 0, cx));
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a", "C:\\c"]);
    assert_eq!(h.tab_paths(cx, 1), ["C:\\b"]);
}

#[gpui_kit::test]
fn opening_in_the_other_pane_turns_dual_view_on_and_keeps_the_keyboard_where_it_was(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let task = h.explorer.update(cx, |e, cx| e.open_in_other_pane("C:\\b", None, cx));
    assert!(cx.foreground_executor().block_test(task));
    h.read(cx, |e| {
        assert!(e.is_dual());
        assert_eq!(e.active_pane(), 0, "the keyboard stays");
    });
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a"]);
    assert_eq!(h.tab_paths(cx, 1), ["C:\\b"]);
}

#[gpui_kit::test]
fn dragging_a_tab_onto_the_far_half_splits_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let only = h.read(cx, |e| e.active_tab_id());
    // A pane's only tab stays; the second pane opens on its folder.
    h.run(cx, |e, cx| e.split_with_tab(only, cx));
    h.read(cx, |e| {
        assert!(e.is_dual());
        assert_eq!(e.active_pane(), 1);
    });
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a"]);
    assert_eq!(h.tab_paths(cx, 1), ["C:\\a"]);
    h.run(cx, |e, cx| e.toggle_dual(cx));
    // With two tabs, the dragged one moves to the second pane.
    h.run(cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    let b = h.read(cx, |e| e.active_tab_id());
    h.run(cx, |e, cx| e.toggle_dual(cx));
    h.run(cx, |e, cx| e.close_pane(1, cx));
    h.run(cx, |e, cx| e.split_with_tab(b, cx));
    assert!(h.read(cx, |e| e.is_dual()));
    assert_eq!(h.read(cx, |e| e.pane_of(b)), Some(1));
    assert_eq!(h.tab_paths(cx, 1), ["C:\\b"]);
}

#[gpui_kit::test]
fn closing_other_tabs_shows_the_kept_one_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    h.run(cx, |e, cx| e.new_tab(Some("C:\\c"), cx));
    let keep = h.read(cx, |e| e.panes()[0].tabs[1]);
    h.run(cx, |e, cx| e.close_other_tabs(keep, cx));
    assert_eq!(h.tab_paths(cx, 0), ["C:\\b"]);
    assert_eq!(h.read(cx, |e| e.active_tab_id()), keep);
    // The other two can be reopened.
    h.run(cx, |e, cx| e.reopen_closed_tab(cx));
    assert_eq!(h.tab_paths(cx, 0).len(), 2);
}

// ---- Selection --------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn selection_select_only_toggle_range_all_and_invert(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    // rows: sub, file2.txt, file10.txt, notes.md
    let r = h.read(cx, |e| e.active_tab().map(|t| t.rows.clone()).unwrap_or_default());
    let p = |name: &str| format!("C:\\a\\{name}");

    h.run(cx, |e, cx| e.select_only(Some(&p("file2.txt")), cx));
    assert_eq!(h.selected(cx), ["file2.txt"]);
    h.read(cx, |e| {
        let s = &e.active_tab().expect("a tab").selection;
        assert_eq!(
            (s.anchor(), s.focus()),
            (Some(p("file2.txt").as_str()), Some(p("file2.txt").as_str()))
        );
    });

    // Ctrl-click adds, and again removes; the anchor moves to the clicked item.
    h.run(cx, |e, cx| e.toggle_selected(&p("notes.md"), cx));
    assert_eq!(h.selected(cx), ["file2.txt", "notes.md"]);
    h.run(cx, |e, cx| e.toggle_selected(&p("file2.txt"), cx));
    assert_eq!(h.selected(cx), ["notes.md"]);
    h.read(cx, |e| {
        assert_eq!(
            e.active_tab().expect("a tab").selection.anchor(),
            Some(p("file2.txt").as_str())
        )
    });

    // Shift-click from the anchor (file2.txt, row 1) to row 3 replaces the selection.
    h.run(cx, |e, cx| e.select_range(&r, 3, false, cx));
    assert_eq!(h.selected(cx), ["file2.txt", "file10.txt", "notes.md"]);
    h.read(cx, |e| {
        let s = &e.active_tab().expect("a tab").selection;
        assert_eq!(s.anchor(), Some(p("file2.txt").as_str()), "the anchor stays");
        assert_eq!(s.focus(), Some(p("notes.md").as_str()));
    });
    // Shift-click backwards, still from the anchor.
    h.run(cx, |e, cx| e.select_range(&r, 0, false, cx));
    assert_eq!(h.selected(cx), ["sub", "file2.txt"]);
    // Ctrl+Shift adds to what was there.
    h.run(cx, |e, cx| e.select_only(Some(&p("notes.md")), cx));
    h.run(cx, |e, cx| e.select_range(&r, 0, true, cx));
    // Anchor was notes.md (row 3): rows 0..=3, in that order of insertion after the notes.md already there.
    assert_eq!(h.selected(cx).len(), 4);
    // Without an anchor in the rows, the range is the one row.
    h.run(cx, |e, cx| e.select_only(None, cx));
    h.run(cx, |e, cx| e.select_range(&r, 2, false, cx));
    assert_eq!(h.selected(cx), ["file10.txt"]);
    // Out of range: nothing happens.
    h.run(cx, |e, cx| e.select_range(&r, 99, false, cx));
    assert_eq!(h.selected(cx), ["file10.txt"]);

    h.run(cx, |e, cx| e.select_all(&r, cx));
    assert_eq!(h.selected(cx).len(), 4, "hidden items aren't rows, so aren't selected");
    h.run(cx, |e, cx| e.select_only(Some(&p("sub")), cx));
    h.run(cx, |e, cx| e.invert_selection(&r, cx));
    assert_eq!(h.selected(cx), ["file2.txt", "file10.txt", "notes.md"]);
    h.run(cx, |e, cx| e.select_only(None, cx));
    assert!(h.selected(cx).is_empty());
}

#[gpui_kit::test]
fn selection_changes_raise_events_and_a_view_holding_the_old_one_sees_no_change(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\notes.md"), cx));
    let before = h.read(cx, |e| e.active_tab().expect("a tab").selection.clone());
    let marker = h.events.borrow().len();
    h.run(cx, |e, cx| e.toggle_selected("C:\\a\\sub", cx));
    assert_eq!(before.len(), 1, "copy on write");
    assert!(h.events.borrow()[marker..]
        .iter()
        .any(|e| matches!(e, ExplorerEvent::SelectionChanged(_))));
}

// ---- Sorting, hidden items, grouping --------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_sort_in_settings_orders_the_rows_and_follows_changes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    // Every folder sorts alike (no folders' own views).
    cx.update(|cx| crate::state::update(cx, |s| s.per_folder_views = false));
    assert!(h.go(cx, "C:\\a"));
    assert_eq!(h.names(cx), ["sub", "file2.txt", "file10.txt", "notes.md"]);
    cx.update(|cx| crate::state::update(cx, |s| s.sort = sort_by(SortKey::Size, true)));
    cx.run_until_parked();
    assert_eq!(h.names(cx), ["sub", "file2.txt", "notes.md", "file10.txt"]);
    h.read(cx, |e| {
        assert_eq!(e.active_tab().expect("a tab").sort, sort_by(SortKey::Size, true))
    });
    // Folders first off: the folder is sorted among the files (size 0, last when descending).
    cx.update(|cx| crate::state::update(cx, |s| s.folders_first = false));
    cx.run_until_parked();
    assert_eq!(h.names(cx), ["file2.txt", "notes.md", "file10.txt", "sub"]);
    // Show hidden items: the hidden file joins the rows (size 0).
    cx.update(|cx| crate::state::update(cx, |s| s.show_hidden = true));
    cx.run_until_parked();
    assert_eq!(h.names(cx).len(), 5);
    assert!(h.names(cx).contains(&".secret".to_string()));
    h.read(cx, |e| assert_eq!(e.active_tab().expect("a tab").rows.hidden, 0));
}

#[gpui_kit::test]
fn opening_a_folder_tells_of_the_memory_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.events.borrow_mut().clear();
    assert!(h.go(cx, "C:\\b"));
    let told = h
        .events
        .borrow()
        .iter()
        .filter(|e| **e == ExplorerEvent::MemoryChanged)
        .count();
    assert_eq!(told, 1, "the last folder and the visit in one event");
    h.read(cx, |e| {
        assert_eq!(e.memory().app().last_folder.as_deref(), Some("C:\\b"))
    });
}

#[gpui_kit::test]
fn a_folders_own_view_beats_its_kinds_which_beats_settings(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    // Downloads sorts newest first until it has its own view.
    h.run(cx, |e, _| {
        e.known = vec![KnownFolder {
            id: KnownId::Downloads,
            name: "Downloads",
            path: "C:\\b".into(),
        }]
        .into();
    });
    assert!(h.go(cx, "C:\\b"));
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert_eq!(tab.kind, glint_model::columns::FolderKind::Downloads);
        assert_eq!(tab.sort, sort_by(SortKey::Modified, true));
    });
    assert_eq!(h.names(cx), ["b2.txt", "b1.txt"]);
    // Clicking a header sorts by it and the folder remembers: other folders keep Settings' sort.
    h.run(cx, |e, cx| e.set_sort(sort_by(SortKey::Name, false), cx));
    assert_eq!(h.names(cx), ["b1.txt", "b2.txt"]);
    assert!(h.events.borrow().contains(&ExplorerEvent::MemoryChanged));
    h.read(cx, |e| {
        assert_eq!(
            e.memory().folder_view("C:\\B").and_then(|v| v.sort),
            Some(sort_by(SortKey::Name, false))
        );
    });
    assert!(h.go(cx, "C:\\a"));
    h.read(cx, |e| {
        assert_eq!(e.active_tab().expect("a tab").sort, sort_by(SortKey::Name, false))
    });
    // With per-folder views off, a header click changes the one sort everyone shares.
    cx.update(|cx| crate::state::update(cx, |s| s.per_folder_views = false));
    h.run(cx, |e, cx| e.set_sort(sort_by(SortKey::Size, false), cx));
    assert_eq!(h.names(cx), ["sub", "file10.txt", "notes.md", "file2.txt"]);
    h.read(cx, |e| {
        assert_eq!(e.active_tab().expect("a tab").sort, sort_by(SortKey::Size, false))
    });
}

#[gpui_kit::test]
fn grouping_gives_the_list_headers_and_the_grid_none(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.set_group(GroupBy::Kind, cx));
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert_eq!(tab.group, GroupBy::Kind);
        let groups = tab.rows.groups().expect("a grouped list has headers");
        assert!(groups.headers.len() >= 2);
        assert_eq!(tab.rows.len(), 4);
    });
    // The grid has no headers.
    h.run(cx, |e, cx| e.set_view(ViewMode::Grid, None, cx));
    h.read(cx, |e| assert!(e.active_tab().expect("a tab").rows.groups().is_none()));
    h.run(cx, |e, cx| e.set_view(ViewMode::List, None, cx));
    h.read(cx, |e| assert!(e.active_tab().expect("a tab").rows.groups().is_some()));
}

#[gpui_kit::test]
fn the_filter_narrows_the_rows_and_a_new_folder_clears_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.set_filter("file", None, cx));
    assert_eq!(h.names(cx), ["file2.txt", "file10.txt"]);
    // A re-read keeps the filter; going elsewhere clears it.
    let reload = h.explorer.update(cx, |e, cx| e.reload(None, cx));
    cx.foreground_executor().block_test(reload);
    assert_eq!(h.names(cx), ["file2.txt", "file10.txt"]);
    assert!(h.go(cx, "C:\\b"));
    h.read(cx, |e| assert!(e.active_tab().expect("a tab").filter.is_empty()));
    assert_eq!(h.names(cx).len(), 2);
}

/// What Git says of `C:` in the standard disk: `file2.txt` changed, `notes.md` new, `sub` staged.
fn git_marks_of_a() -> Arc<glint_model::entries::GitMarks> {
    use glint_model::entries::GitMark;
    Arc::new(
        [
            ("file2.txt".to_string(), GitMark::Modified),
            ("notes.md".to_string(), GitMark::Untracked),
            ("sub".to_string(), GitMark::Staged),
        ]
        .into_iter()
        .collect(),
    )
}

fn rows_changed(h: &Harness) -> usize {
    h.events
        .borrow()
        .iter()
        .filter(|e| matches!(e, ExplorerEvent::RowsChanged(_)))
        .count()
}

#[gpui_kit::test]
fn the_git_filter_follows_the_marks_the_window_hands_over(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let tab = h.read(cx, |e| e.active_tab_id());
    // Without marks (not in a repository) a git: filter matches nothing.
    h.run(cx, |e, cx| e.set_filter("git:changed", None, cx));
    assert!(h.names(cx).is_empty());

    let marks = git_marks_of_a();
    h.run(cx, |e, cx| e.set_git_marks(tab, Some(marks.clone()), cx));
    assert_eq!(h.names(cx), ["sub", "file2.txt", "notes.md"]);
    h.run(cx, |e, cx| e.set_filter("git:modified", None, cx));
    assert_eq!(h.names(cx), ["file2.txt"]);
    h.read(cx, |e| {
        assert_eq!(e.tab(tab).and_then(|t| t.git_marks()).map(|m| m.len()), Some(3));
    });

    // Git sees one more change in the folder: the rows are built again.
    let mut more = (*marks).clone();
    more.insert("file10.txt".into(), glint_model::entries::GitMark::Modified);
    h.run(cx, |e, cx| e.set_git_marks(tab, Some(Arc::new(more)), cx));
    assert_eq!(h.names(cx), ["file2.txt", "file10.txt"]);

    // The repository is gone (the folder was moved out of it): nothing matches again.
    h.run(cx, |e, cx| e.set_git_marks(tab, None, cx));
    assert!(h.names(cx).is_empty());
}

#[gpui_kit::test]
fn marks_that_nothing_reads_rebuild_no_rows_and_the_same_marks_again_change_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let tab = h.read(cx, |e| e.active_tab_id());
    let before = rows_changed(&h);
    h.run(cx, |e, cx| e.set_git_marks(tab, Some(git_marks_of_a()), cx));
    assert_eq!(rows_changed(&h), before, "a plain list doesn't read the marks");
    assert_eq!(h.names(cx), ["sub", "file2.txt", "file10.txt", "notes.md"]);

    h.run(cx, |e, cx| e.set_filter("git:new", None, cx));
    assert_eq!(h.names(cx), ["notes.md"]);
    let before = rows_changed(&h);
    // A copy of the same marks is no change.
    h.run(cx, |e, cx| {
        e.set_git_marks(tab, Some(Arc::new((*git_marks_of_a()).clone())), cx)
    });
    assert_eq!(rows_changed(&h), before);
    // A tab that isn't there is ignored.
    h.run(cx, |e, cx| e.set_git_marks(TabId(99), Some(git_marks_of_a()), cx));
}

#[gpui_kit::test]
fn group_by_git_orders_the_list_under_conflicts_changed_staged_new_and_the_rest(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let tab = h.read(cx, |e| e.active_tab_id());
    h.run(cx, |e, cx| e.set_group(GroupBy::Git, cx));
    let labels = |h: &Harness, cx: &TestAppContext| -> Vec<String> {
        h.read(cx, |e| {
            e.tab(tab)
                .and_then(|t| t.rows.groups())
                .map(|g| g.headers.iter().map(|header| header.label.to_string()).collect())
                .unwrap_or_default()
        })
    };
    assert_eq!(labels(&h, cx), ["Unchanged"], "outside a repository nothing is marked");

    h.run(cx, |e, cx| e.set_git_marks(tab, Some(git_marks_of_a()), cx));
    assert_eq!(labels(&h, cx), ["Changed", "Staged", "New", "Unchanged"]);
    assert_eq!(h.names(cx), ["file2.txt", "sub", "notes.md", "file10.txt"]);

    // The grid draws no headers, and a search's results have no marks.
    h.run(cx, |e, cx| e.set_view(ViewMode::Grid, None, cx));
    h.read(cx, |e| assert!(e.tab(tab).is_some_and(|t| t.rows.groups().is_none())));
}

#[gpui_kit::test]
fn a_huge_folder_that_needs_sorting_is_sorted_in_the_background(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    cx.update(|cx| crate::state::update(cx, |s| s.per_folder_views = false));
    let big: Vec<_> = (0..25_000)
        .map(|i| {
            file(
                "C:\\big",
                &format!("f{i:05}.txt"),
                u64::try_from(i % 1000).unwrap_or(0),
                i64::from(i),
            )
        })
        .collect();
    h.disk.folder("C:\\big", big);
    assert!(h.go(cx, "C:\\big"));
    // Name order is the engine's: the listing is the rows, no order built.
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert!(tab.rows.is_identity());
        assert_eq!(tab.rows.len(), 25_000);
    });
    cx.update(|cx| crate::state::update(cx, |s| s.sort = sort_by(SortKey::Modified, true)));
    cx.run_until_parked();
    h.read(cx, |e| {
        let tab = e.active_tab().expect("a tab");
        assert_eq!(tab.rows.get(0).map(|r| r.name.as_str()), Some("f24999.txt"));
        assert_eq!(tab.rows.get(24_999).map(|r| r.name.as_str()), Some("f00000.txt"));
    });
}

#[gpui_kit::test]
fn zooming_the_grid_steps_the_tile_size_in_settings(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    let tile = |cx: &mut TestAppContext| cx.update(|cx| crate::state::settings(cx).tile_size);
    assert_eq!(tile(cx), glint_model::grid::TileSize::Medium);
    h.run(cx, |e, cx| e.zoom_grid(1, cx));
    assert_eq!(tile(cx), glint_model::grid::TileSize::Large);
    h.run(cx, |e, cx| e.zoom_grid(10, cx));
    assert_eq!(tile(cx), glint_model::grid::TileSize::Huge, "clamped at the biggest");
    h.run(cx, |e, cx| e.zoom_grid(0, cx));
    assert_eq!(tile(cx), glint_model::grid::TileSize::Medium, "0 is medium again");
}

// ---- Opening ----------------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn opening_entries_follows_what_they_are(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let entries: Vec<_> = h.read(cx, |e| {
        e.active_tab()
            .map(|t| t.rows.iter().cloned().collect())
            .unwrap_or_default()
    });
    let by_name = |name: &str| entries.iter().find(|e| e.name == name).cloned().expect("an entry");

    // A folder opens as a folder.
    let sub = by_name("sub");
    let task = h.explorer.update(cx, |e, cx| e.open_entry(&sub, cx));
    assert!(cx.foreground_executor().block_test(task));
    assert_eq!(h.path(cx), "C:\\a\\sub");

    // A file goes to Windows; a shortcut to a folder comes back as that folder.
    assert!(h.go(cx, "C:\\a"));
    h.disk.on_open("C:\\a\\notes.md", None);
    let notes = by_name("notes.md");
    let task = h.explorer.update(cx, |e, cx| e.open_entry(&notes, cx));
    assert!(cx.foreground_executor().block_test(task));
    assert_eq!(h.path(cx), "C:\\a", "opening a file doesn't navigate");
    h.disk.on_open("C:\\a\\file2.txt", Some("C:\\b"));
    let link = by_name("file2.txt");
    let task = h.explorer.update(cx, |e, cx| e.open_entry(&link, cx));
    assert!(cx.foreground_executor().block_test(task));
    assert_eq!(h.path(cx), "C:\\b");
    // A file nothing opens says so.
    let missing = file("C:\\b", "mystery.bin", 1, 1);
    let task = h.explorer.update(cx, |e, cx| e.open_entry(&missing, cx));
    assert!(!cx.foreground_executor().block_test(task));
    assert!(matches!(h.notices().last(), Some(Notice::CouldNotOpenFile { path, .. }) if path == "C:\\b\\mystery.bin"));
}

#[gpui_kit::test]
fn special_entries_are_looked_at_refused_or_handed_to_their_part(cx: &mut TestAppContext) {
    use glint_model::entries::{attr, with_details, RawEntry};
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let special = |name: &str, attrs: u32| {
        with_details(
            RawEntry {
                name: name.into(),
                attrs,
                ..Default::default()
            },
            "C:\\a",
        )
    };
    // In the Recycle Bin: looked at (Quick Look), focus on it, not opened.
    let recycled = special("old.txt", attr::RECYCLED);
    let task = h.explorer.update(cx, |e, cx| e.open_entry(&recycled, cx));
    assert!(cx.foreground_executor().block_test(task));
    h.read(cx, |e| {
        assert!(e.quick_look);
        assert_eq!(e.active_tab().and_then(|t| t.selection.focus()), Some("C:\\a\\old.txt"));
    });
    // A file listed from the index in a closed folder can't be opened.
    let closed = special("shut.txt", attr::CLOSED);
    let task = h.explorer.update(cx, |e, cx| e.open_entry(&closed, cx));
    assert!(!cx.foreground_executor().block_test(task));
    assert!(matches!(
        h.notices().last(),
        Some(Notice::Closed {
            can_get_access: false,
            ..
        })
    ));
    // Disk images, previous versions and files inside archives are handed over.
    let image = special("setup.iso", 0);
    h.run(cx, |e, cx| e.open_entry(&image, cx).detach());
    let inside = special("inner.txt", attr::IN_ARCHIVE);
    h.run(cx, |e, cx| e.open_entry(&inside, cx).detach());
    let events = h.events.borrow();
    assert!(events.contains(&ExplorerEvent::Handoff(Handoff::DiskImage("C:\\a\\setup.iso".into()))));
    assert!(events
        .iter()
        .any(|e| matches!(e, ExplorerEvent::Handoff(Handoff::ArchivedFile(f)) if f.name == "inner.txt")));
}

#[gpui_kit::test]
fn typed_locations_open_places_folders_and_files(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    h.disk.folder("shell:RecycleBinFolder", vec![]);
    h.disk.on_resolve("c:\\a", "C:\\a", true);
    h.disk.on_resolve("C:\\a\\notes.md", "C:\\a\\notes.md", false);
    let go_to = |h: &Harness, cx: &mut TestAppContext, text: &str| {
        let task = h.explorer.update(cx, |e, cx| e.go_to(text, cx));
        cx.foreground_executor().block_test(task)
    };
    assert!(go_to(&h, cx, "c:\\a"));
    assert_eq!(h.path(cx), "C:\\a");
    // A file opens its folder with it selected.
    assert!(go_to(&h, cx, "  C:\\a\\notes.md "));
    assert_eq!(h.path(cx), "C:\\a");
    assert_eq!(h.selected(cx), ["notes.md"]);
    assert!(go_to(&h, cx, "This PC"));
    assert_eq!(h.path(cx), "");
    assert!(go_to(&h, cx, "recycle bin"));
    assert_eq!(h.path(cx), "shell:RecycleBinFolder");
    assert!(h.read(cx, |e| e.recycle_bin_of(None)));
    assert!(go_to(&h, cx, ""));
    assert_eq!(h.path(cx), "");
    // Something that isn't there says so.
    assert!(!go_to(&h, cx, "Z:\\nope"));
    assert!(matches!(h.notices().last(), Some(Notice::Message(m)) if m.contains("Z:\\nope")));
}

// ---- Start-up, session, watches -----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn starting_opens_the_start_folder_and_saves_the_session_only_once_restored(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    h.disk.set_known(vec![KnownFolder {
        id: KnownId::Home,
        name: "Home",
        path: "C:\\a".into(),
    }]);
    h.disk.set_drives(vec![Drive {
        path: "C:\\".into(),
        label: "".into(),
        kind: DriveKind::Fixed,
        fs: "NTFS".into(),
        total: 100,
        free: 50,
    }]);
    // Before start-up the session is not saved over.
    h.run(cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
    assert!(h.read(cx, |e| e.memory().session().is_none()));
    h.run(cx, |e, cx| e.close_tab(None, cx));

    let task = h.explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx));
    cx.foreground_executor().block_test(task);
    cx.run_until_parked();
    assert_eq!(h.path(cx), "C:\\a", "Home is the default start folder");
    h.read(cx, |e| {
        assert_eq!(e.known_path(KnownId::Home), Some("C:\\a"));
        assert_eq!(e.drives().len(), 1);
        assert_eq!(e.tab_name("C:\\"), "Local Disk (C:)");
        let session = e.memory().session().expect("saved once restored");
        assert_eq!(session.panes[0].paths, ["C:\\a"]);
        assert_eq!(e.memory().last_folder(), Some("C:\\a"));
        assert_eq!(e.memory().visits().get("C:\\a").map(|v| v.count), Some(1));
    });
    assert!(h.events.borrow().contains(&ExplorerEvent::DrivesChanged));
    assert!(h.events.borrow().contains(&ExplorerEvent::KnownFoldersChanged));
}

#[gpui_kit::test]
fn the_last_folder_start_setting_falls_back_to_home_quietly(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    h.disk.set_known(vec![KnownFolder {
        id: KnownId::Home,
        name: "Home",
        path: "C:\\a".into(),
    }]);
    h.disk.folder("shell:Recent", vec![]);
    cx.update(|cx| crate::state::update(cx, |s| s.start_folder = StartFolder::Last));
    // "Last folder" with a last folder that is gone falls back to Home, quietly.
    h.explorer.update(cx, |e, cx| {
        let app = glint_model::store::AppState {
            last_folder: Some("C:\\vanished".into()),
            ..Default::default()
        };
        e.set_memory(Memory::from_app_state(app), cx);
    });
    let task = h.explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx));
    cx.foreground_executor().block_test(task);
    assert_eq!(h.path(cx), "C:\\a");
    assert!(h.notices().is_empty(), "the start folder failing is not reported");
}

#[gpui_kit::test]
fn the_gallery_is_given_pictures_videos_and_the_added_folders(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    let known = |id, name: &'static str, path: &str| KnownFolder {
        id,
        name,
        path: path.into(),
    };
    h.disk.set_known(vec![
        known(KnownId::Home, "Home", "C:\\a"),
        known(KnownId::Pictures, "Pictures", "C:\\a\\Pictures"),
        known(KnownId::Music, "Music", "C:\\a\\Music"),
        known(KnownId::Videos, "Videos", "C:\\a\\Videos"),
    ]);
    // One added folder is Pictures again (any case): it is given once.
    cx.update(|cx| {
        crate::state::update(cx, |s| {
            s.gallery_folders = vec!["c:\\A\\pictures".into(), "D:\\Photos".into()]
        })
    });
    let task = h.explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx));
    cx.foreground_executor().block_test(task);
    cx.run_until_parked();
    let sent = |h: &Harness, cx: &mut TestAppContext| h.read(cx, |e| e.gallery_sent.clone());
    assert_eq!(
        sent(&h, cx).as_deref(),
        Some(
            &[
                "C:\\a\\Pictures".to_string(),
                "C:\\a\\Videos".into(),
                "D:\\Photos".into()
            ][..]
        )
    );
    // A folder added in Settings (or from the Gallery's own button) is given too.
    cx.update(|cx| crate::state::update(cx, |s| s.gallery_folders.push("E:\\Camera".into())));
    cx.run_until_parked();
    assert_eq!(sent(&h, cx).map(|f| f.len()), Some(4));
}

#[gpui_kit::test]
fn the_recent_start_folder_opens_recent(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    cx.update(|cx| crate::state::update(cx, |s| s.start_folder = StartFolder::Recent));
    h.disk.folder("shell:Recent", vec![]);
    let task = h.explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx));
    cx.foreground_executor().block_test(task);
    assert_eq!(h.path(cx), "shell:Recent");
}

#[gpui_kit::test]
fn start_restores_last_times_tabs_and_panes_and_opens_the_shown_ones(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    let session = Session {
        dual: true,
        active_pane: 1,
        panes: vec![
            SessionPane {
                paths: vec!["C:\\a".into(), "C:\\b".into(), "C:\\gone\\deeper".into()],
                active: 1,
                views: Some(vec![ViewMode::List, ViewMode::Grid, ViewMode::Columns]),
            },
            SessionPane {
                paths: vec!["C:\\c".into()],
                active: 7,
                views: None,
            },
        ],
    };
    h.explorer.update(cx, |e, cx| {
        let app = glint_model::store::AppState {
            session: Some(session),
            ..Default::default()
        };
        e.set_memory(Memory::from_app_state(app), cx);
    });
    let task = h.explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx));
    cx.foreground_executor().block_test(task);
    cx.run_until_parked();
    h.read(cx, |e| {
        assert!(e.is_dual());
        assert_eq!(e.active_pane(), 1);
        assert_eq!(e.panes().len(), 2);
        assert_eq!(e.panes()[0].tabs.len(), 3);
        // Only the tabs on screen opened: b (active in the left pane) and c.
        let left = e.pane_tab(0).expect("left");
        assert_eq!(
            (left.path.as_str(), left.view, left.ready),
            ("C:\\b", ViewMode::Grid, true)
        );
        let right = e.pane_tab(1).expect("right");
        assert_eq!((right.path.as_str(), right.ready), ("C:\\c", true));
        let hidden = e.tab(e.panes()[0].tabs[0]).expect("tab a");
        assert!(!hidden.ready, "tabs out of sight open when first shown");
        assert_eq!(e.tab(e.panes()[0].tabs[2]).map(|t| t.view), Some(ViewMode::Columns));
    });
    assert!(!h.disk.listed().contains(&"C:\\a".to_string()));
    // Showing the hidden tab opens its folder; one whose folder is gone opens the nearest parent that's there.
    let a = h.read(cx, |e| e.panes()[0].tabs[0]);
    h.run(cx, |e, cx| e.activate_tab(a, cx));
    assert_eq!(h.path(cx), "C:\\a");
    let deep = h.read(cx, |e| e.panes()[0].tabs[2]);
    h.run(cx, |e, cx| e.activate_tab(deep, cx));
    assert_eq!(
        h.path(cx),
        "C:\\",
        "C:\\gone\\deeper and C:\\gone are gone; C:\\ is there"
    );
    // The saved session follows what's on screen.
    h.read(cx, |e| {
        let saved = e.memory().session().expect("session");
        assert_eq!(saved.panes[0].paths, ["C:\\a", "C:\\b", "C:\\"]);
        assert_eq!(saved.panes[0].active, 2);
        assert_eq!(saved.panes[0].views.as_ref().map(Vec::len), Some(3));
    });
}

#[gpui_kit::test]
fn start_with_launch_targets_opens_the_first_instead_of_the_start_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    let targets = vec![
        LaunchTarget {
            path: "C:\\a".into(),
            select: Some("C:\\a\\notes.md".into()),
        },
        LaunchTarget {
            path: "C:\\b".into(),
            select: None,
        },
        LaunchTarget {
            path: "C:\\c".into(),
            select: None,
        },
    ];
    let task = h.explorer.update(cx, |e, cx| e.init_explorer(targets, cx));
    cx.foreground_executor().block_test(task);
    cx.run_until_parked();
    assert_eq!(h.tab_paths(cx, 0), ["C:\\a", "C:\\b", "C:\\c"]);
    // The last one handed over is the one shown, and the first selects its item.
    assert_eq!(h.path(cx), "C:\\c");
    let first = h.read(cx, |e| e.panes()[0].tabs[0]);
    h.run(cx, |e, cx| e.activate_tab(first, cx));
    assert_eq!(h.selected(cx), ["notes.md"]);
}

#[gpui_kit::test]
fn only_the_folders_on_screen_are_watched_and_a_missing_window_is_retried(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    h.disk.set_window(false);
    assert!(h.go(cx, "C:\\a"));
    assert_eq!(h.disk.watch_calls(), 0, "no window to watch for yet");
    h.disk.set_window(true);
    assert!(h.go(cx, "C:\\b"));
    assert_eq!(
        h.disk.watched(),
        ["C:\\b"],
        "asked again at the next change, with what is on screen now"
    );
    // A second pane is watched too; a tab out of sight isn't.
    h.run(cx, |e, cx| e.toggle_dual(cx));
    cx.run_until_parked();
    let mut watched = h.disk.watched();
    watched.sort();
    assert_eq!(watched, ["C:\\b"], "both panes show C:\\b: one watcher");
    assert!(h.go(cx, "C:\\a"));
    let mut watched = h.disk.watched();
    watched.sort();
    assert_eq!(watched, ["C:\\a", "C:\\b"]);
    let calls = h.disk.watch_calls();
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\notes.md"), cx));
    assert_eq!(h.disk.watch_calls(), calls, "nothing changed: nothing re-asked");
}

#[gpui_kit::test]
fn the_folder_shown_is_told_to_the_outside_world(cx: &mut TestAppContext) {
    use super::source::Effect;
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    cx.update(|cx| crate::state::update(cx, |s| s.zoxide = true));
    assert!(h.go(cx, "C:\\a"));
    let effects = h.disk.effects();
    assert!(effects.contains(&Effect::ReportFolder("C:\\a".into())));
    assert!(effects.contains(&Effect::ZoxideAdd("C:\\a".into())));
    // The hint for the next start comes a moment after the folders stop changing.
    assert!(!effects.iter().any(|e| matches!(e, Effect::StartupHint(_))));
    cx.executor().advance_clock(Duration::from_millis(1600));
    cx.run_until_parked();
    assert!(h.disk.effects().contains(&Effect::StartupHint(vec!["C:\\a".into()])));
    // The bench mark is written once.
    h.run(cx, |e, _| {
        e.first_folder_painted();
        e.first_folder_painted();
    });
    assert_eq!(
        h.disk
            .effects()
            .iter()
            .filter(|e| **e == Effect::BenchFirstFolder)
            .count(),
        1
    );
    cx.update(|cx| crate::state::update(cx, |s| s.zoxide = false));
}

#[gpui_kit::test]
fn views_remember_scroll_per_folder_and_time_the_navigation(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    let tab = h.read(cx, |e| e.active_tab_id());
    let key = h.read(cx, |e| e.view_key(None));
    assert_eq!(key, "dir:C:\\a");
    h.explorer.update(cx, |e, _| e.save_scroll(tab, &key, 120.5));
    h.read(cx, |e| {
        assert_eq!(e.saved_scroll(tab, "dir:C:\\a"), 120.5);
        assert_eq!(
            e.saved_scroll(tab, "dir:C:\\b"),
            0.0,
            "another folder starts at the top"
        );
    });
    h.read(cx, |e| assert_eq!(e.active_tab().expect("a tab").shown_in, 0.0));
    h.run(cx, |e, cx| e.mark_shown(tab, cx));
    h.read(cx, |e| {
        assert!(
            e.active_tab().expect("a tab").shown_in > 0.0,
            "timed from the click to the paint"
        )
    });
}

// ---- A real folder ---------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_real_temporary_folder_lists_through_glint_core(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let root = std::env::temp_dir().join(format!("glint-explorer-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("zeta")).expect("a folder");
    std::fs::write(root.join("a10.txt"), b"ten").expect("a file");
    std::fs::write(root.join("a2.txt"), b"two!").expect("a file");
    std::fs::write(root.join("Читать.md"), b"x").expect("a file");

    cx.update(testing::init);
    cx.executor().allow_parking();
    let app = cx.update(|cx| cx.global::<crate::core_bridge::Core>().app.clone());
    let source: Rc<dyn Source> = Rc::new(CoreSource::new(app));
    let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(source, cx)));
    let path = root.to_string_lossy().into_owned();
    let task = explorer.update(cx, |e, cx| e.navigate_task(path.clone(), NavOptions::default(), cx));
    let mut listed = false;
    for _ in 0..400 {
        cx.run_until_parked();
        listed = explorer.read_with(cx, |e, _| e.active_tab().is_some_and(|t| !t.loading));
        if listed {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(listed, "the folder never listed");
    drop(task);
    explorer.read_with(cx, |e, _| {
        let tab = e.active_tab().expect("a tab");
        assert_eq!(
            tab.load,
            LoadState::Ready(Loaded {
                read_ms: tab.listing.as_ref().map_or(0.0, |l| l.elapsed),
                engine: Engine::Disk
            })
        );
        let names: Vec<&str> = tab.rows.iter().map(|r| r.name.as_str()).collect();
        // Folders first, then natural order: a2 before a10; the Cyrillic name after the Latin ones.
        assert_eq!(names, ["zeta", "a2.txt", "a10.txt", "Читать.md"]);
        let sizes: Vec<u64> = tab.rows.iter().map(|r| r.size).collect();
        assert_eq!(sizes, [0, 4, 3, 1]);
        assert!(tab.rows.iter().all(|r| r.path.ends_with(&r.name)));
    });
    let _ = std::fs::remove_dir_all(&root);
}

// ---- Pure helpers ------------------------------------------------------------------------------------------------------------

#[test]
fn tab_names_follow_the_typescript() {
    let drives = vec![
        Drive {
            path: "C:\\".into(),
            label: "".into(),
            kind: DriveKind::Fixed,
            fs: "NTFS".into(),
            total: 0,
            free: 0,
        },
        Drive {
            path: "D:\\".into(),
            label: "Data".into(),
            kind: DriveKind::Removable,
            fs: "exFAT".into(),
            total: 0,
            free: 0,
        },
        Drive {
            path: "\\\\wsl.localhost\\Ubuntu".into(),
            label: "Ubuntu".into(),
            kind: DriveKind::Wsl,
            fs: "WSL".into(),
            total: 0,
            free: 0,
        },
    ];
    let remote = |path: &str| format!("remote:{}", path.len());
    let name = |path: &str| tab_name(path, &drives, &remote);
    assert_eq!(name(""), "This PC");
    assert_eq!(name("shell:RecycleBinFolder"), "Recycle Bin");
    assert_eq!(name("shell:recent"), "Recent");
    assert_eq!(name("shell:Gallery"), "Gallery");
    assert_eq!(name("C:\\Users\\me\\Documents"), "Documents");
    assert_eq!(name("C:\\"), "Local Disk (C:)");
    assert_eq!(name("D:\\"), "Data (D:)");
    assert_eq!(name("E:\\"), "E:\\", "a drive that isn't known is its path");
    assert_eq!(name("\\\\wsl.localhost\\Ubuntu"), "Ubuntu (WSL)");
    assert_eq!(
        drive_name(&Drive {
            path: "F:\\".into(),
            label: "".into(),
            kind: DriveKind::Cdrom,
            fs: "".into(),
            total: 0,
            free: 0
        }),
        "DVD Drive (F:)"
    );
    assert_eq!(
        drive_name(&Drive {
            path: "\\\\wsl.localhost\\X".into(),
            label: "".into(),
            kind: DriveKind::Wsl,
            fs: "".into(),
            total: 0,
            free: 0
        }),
        "Linux (WSL)"
    );
    assert_eq!(TabId(7).to_string(), "tab7");
}

#[gpui_kit::test]
fn start_runs_once_and_removing_a_remote_sends_its_tabs_to_this_pc(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    h.disk.folder("remote:nas", vec![file("remote:nas", "movie.mkv", 5, 5)]);
    h.disk.set_known(vec![KnownFolder {
        id: KnownId::Home,
        name: "Home",
        path: "C:\\a".into(),
    }]);
    let task = h.explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx));
    cx.foreground_executor().block_test(task);
    // A second start has nothing to do.
    let again = h.explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx));
    cx.foreground_executor().block_test(again);
    let reads = h.disk.listed().len();
    assert!(h.go(cx, "remote:nas"));
    assert!(h.read(cx, |e| e.remote_of(None)));
    h.run(cx, |e, cx| e.leave_remote("NAS", cx));
    assert_eq!(h.path(cx), "");
    assert!(h.disk.listed().len() > reads);
}

#[gpui_kit::test]
fn a_folder_read_ahead_can_be_peeked_at_and_stays_put_while_it_is_read_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\"));
    h.read(cx, |e| assert!(e.peeks().is_empty()));
    h.run(cx, |e, cx| e.prefetch_folder("C:\\a", cx));
    // What it holds, under the path its row has.
    let seen = |h: &Harness, cx: &TestAppContext| -> Vec<(String, usize)> {
        h.read(cx, |e| {
            e.peeks()
                .iter()
                .map(|(path, listed)| (path.to_string(), listed.entries.len()))
                .collect()
        })
    };
    assert_eq!(seen(&h, cx), [("C:\\a".to_string(), 5)]);
    // The pointer comes back a while later and the folder is read again: what it held is said until the new
    // listing is in, so the row's words don't blink.
    cx.executor().advance_clock(Duration::from_millis(2500));
    let gate = h.disk.hold("C:\\a");
    h.run(cx, |e, cx| e.prefetch_folder("C:\\a", cx));
    assert_eq!(seen(&h, cx), [("C:\\a".to_string(), 5)], "still said while it is read");
    gate.close();
    cx.run_until_parked();
    assert_eq!(seen(&h, cx).len(), 1);
    // Opening it takes the listing: there is nothing left to peek at.
    assert!(h.go(cx, "C:\\a"));
    assert!(seen(&h, cx).is_empty());
}

#[gpui_kit::test]
fn selecting_what_is_already_selected_tells_nobody(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\a"));
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\notes.md"), cx));
    let told = h.events.borrow().len();
    // The second press of a double-click: the same item, the same focus and anchor.
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\notes.md"), cx));
    assert_eq!(h.events.borrow().len(), told, "nothing changed, so nothing was told");
    // Another item is a change; so is nothing selected.
    h.run(cx, |e, cx| e.select_only(Some("C:\\a\\file2.txt"), cx));
    let after = h.events.borrow().len();
    assert!(after > told);
    h.run(cx, |e, cx| e.select_only(None, cx));
    assert!(h.events.borrow().len() > after);
}

#[gpui_kit::test]
fn a_folder_says_it_is_loading_only_when_it_takes_a_moment(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let h = harness(cx);
    standard_disk(&h.disk);
    assert!(h.go(cx, "C:\\"));
    h.events.borrow_mut().clear();
    let gate = h.disk.hold("C:\\a");
    let opening = h
        .explorer
        .update(cx, |e, cx| e.navigate_task("C:\\a", NavOptions::default(), cx));
    cx.run_until_parked();
    // Nothing is said at once: a folder read in a few milliseconds is drawn without a frame that says "loading".
    assert!(h.events.borrow().is_empty(), "{:?}", h.events.borrow());
    h.read(cx, |e| {
        assert!(e.active_tab().expect("a tab").loading, "the tab itself knows")
    });
    // This one takes its time: the views are told, once.
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();
    let tab = h.read(cx, |e| e.active_tab_id());
    assert_eq!(*h.events.borrow(), [ExplorerEvent::TabChanged(tab)]);
    gate.close();
    assert!(cx.foreground_executor().block_test(opening));
    // One that arrives before the moment is up is never said to be loading.
    h.events.borrow_mut().clear();
    assert!(h.go(cx, "C:\\b"));
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();
    let loading_said = h
        .events
        .borrow()
        .iter()
        .filter(|event| matches!(event, ExplorerEvent::TabChanged(_)))
        .count();
    assert_eq!(loading_said, 1, "only the one that says the folder is open");
}

#[gpui_kit::test]
fn a_big_folders_kind_is_read_from_a_sample_of_it(cx: &mut TestAppContext) {
    use glint_model::columns::FolderKind;
    let _guard = testing::lock();
    let h = harness(cx);
    // Thirty thousand photos with a text file every tenth item: mostly pictures, whichever of them are looked at.
    let entries: Vec<_> = (0..30_000)
        .map(|i| {
            if i % 10 == 0 {
                file("C:\\shots", &format!("note{i}.txt"), 1, 1)
            } else {
                file("C:\\shots", &format!("img{i}.jpg"), 1, 1)
            }
        })
        .collect();
    h.disk.folder("C:\\shots", entries);
    assert!(h.go(cx, "C:\\shots"));
    h.read(cx, |e| {
        assert_eq!(e.active_tab().expect("a tab").kind, FolderKind::Pictures)
    });
}

#[gpui_kit::test]
fn a_big_listing_is_let_go_of_a_moment_later_and_a_small_one_at_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let small: Arc<[glint_model::entries::FileEntry]> = Arc::from(vec![file("C:\\x", "a.txt", 1, 1)]);
    let big: Arc<[glint_model::entries::FileEntry]> = Arc::from(
        (0..5000)
            .map(|i| file("C:\\x", &format!("f{i}.txt"), 1, 1))
            .collect::<Vec<_>>(),
    );
    cx.update(|cx| {
        let_go(Some(small.clone()), cx);
        let_go(Some(big.clone()), cx);
        let_go(None, cx);
    });
    assert_eq!(Arc::strong_count(&small), 1, "a small one is simply dropped");
    assert_eq!(
        Arc::strong_count(&big),
        2,
        "a big one is held for a background thread to free"
    );
    cx.executor().advance_clock(Duration::from_millis(600));
    cx.run_until_parked();
    assert_eq!(Arc::strong_count(&big), 1);
}
