//! Back and Forward through searches and folders, on a fake disk: a search's results are a place of their own,
//! brought back as they were left (the folder opened from them selected), without searching again.

use super::fake::*;
use super::*;
use crate::testing;
use gpui_kit::{AppContext as _, Entity, TestAppContext};
use std::cell::RefCell;
use std::rc::Rc;

struct Rig {
    explorer: Entity<Explorer>,
    disk: FakeDisk,
    events: Rc<RefCell<Vec<ExplorerEvent>>>,
}

/// `C:\` with `a` (holding the folder `Personal Stuff` and a few files) and `b` (holding `personal notes.txt`).
fn rig(cx: &mut TestAppContext) -> Rig {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), folder("C:\\", "c")],
    );
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "Personal Stuff"),
            file("C:\\a", "notes.md", 20, 2),
            file("C:\\a", "report.pdf", 10, 1),
        ],
    );
    disk.folder(
        "C:\\a\\Personal Stuff",
        vec![file("C:\\a\\Personal Stuff", "passport.jpg", 5, 5)],
    );
    disk.folder(
        "C:\\b",
        vec![
            file("C:\\b", "personal notes.txt", 30, 3),
            file("C:\\b", "other.txt", 40, 4),
        ],
    );
    disk.folder("C:\\c", vec![]);
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

impl Rig {
    fn go(&self, cx: &mut TestAppContext, path: &str) {
        let task = self
            .explorer
            .update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        assert!(cx.foreground_executor().block_test(task), "{path} opens");
    }

    fn find(&self, cx: &mut TestAppContext, query: &str) {
        let task = self.explorer.update(cx, |e, cx| {
            e.set_filter(query, None, cx);
            e.run_find(query, Wider::default(), cx)
        });
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
    }

    fn back(&self, cx: &mut TestAppContext) -> bool {
        let task = self.explorer.update(cx, |e, cx| e.go_back(cx));
        let went = cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        went
    }

    fn forward(&self, cx: &mut TestAppContext) -> bool {
        let task = self.explorer.update(cx, |e, cx| e.go_forward(cx));
        let went = cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        went
    }

    /// Selects the row `path` and opens it, as Enter on it does.
    fn open(&self, cx: &mut TestAppContext, path: &str) {
        let task = self.explorer.update(cx, |e, cx| {
            e.select_only(Some(path), cx);
            let entry = e
                .active_tab()
                .and_then(|t| t.rows.iter().find(|r| r.path == path).cloned())
                .expect("the row is shown");
            e.open_entry(&entry, cx)
        });
        assert!(cx.foreground_executor().block_test(task), "{path} opens");
        cx.run_until_parked();
    }

    fn read<R>(&self, cx: &TestAppContext, f: impl FnOnce(&Explorer) -> R) -> R {
        self.explorer.read_with(cx, |e, _| f(e))
    }

    fn path(&self, cx: &TestAppContext) -> String {
        self.read(cx, |e| e.current_path().to_string())
    }

    /// The words of the search shown, if one is.
    fn search(&self, cx: &TestAppContext) -> Option<String> {
        self.read(cx, |e| {
            e.active_tab().and_then(|t| t.find.as_ref().map(|f| f.query.clone()))
        })
    }

    fn filter(&self, cx: &TestAppContext) -> String {
        self.read(cx, |e| e.active_tab().map(|t| t.filter.clone()).unwrap_or_default())
    }

    fn names(&self, cx: &TestAppContext) -> Vec<String> {
        self.read(cx, |e| {
            e.active_tab()
                .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
                .unwrap_or_default()
        })
    }

    /// The selected paths, and the one the keyboard is on.
    fn selected(&self, cx: &TestAppContext) -> (Vec<String>, Option<String>) {
        self.read(cx, |e| {
            let tab = e.active_tab().expect("a tab");
            (tab.selection.to_vec(), tab.selection.focus().map(str::to_string))
        })
    }

    /// Back and Forward, each place by its folder or as "search:" and its words.
    fn history(&self, cx: &TestAppContext) -> (Vec<String>, Vec<String>) {
        self.read(cx, |e| {
            let named = |list: &[Visit]| -> Vec<String> {
                list.iter()
                    .map(|v| match v.search_query() {
                        Some(query) => format!("search:{query}"),
                        None => v.path.clone(),
                    })
                    .collect()
            };
            let tab = e.active_tab().expect("a tab");
            (named(&tab.back), named(&tab.forward))
        })
    }

    fn searches(&self) -> usize {
        self.disk.finds().len()
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
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

const STUFF: &str = "C:\\a\\Personal Stuff";

#[gpui_kit::test]
fn back_from_a_folder_opened_in_the_results_brings_the_results_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    r.find(cx, "personal");
    assert_eq!(r.names(cx), ["Personal Stuff", "personal notes.txt"]);
    assert_eq!(r.history(cx), (strings(&["C:\\"]), vec![]));
    // A folder opened from the results.
    r.open(cx, STUFF);
    assert_eq!(r.path(cx), STUFF);
    assert_eq!(r.search(cx), None);
    assert_eq!(r.history(cx), (strings(&["C:\\", "search:personal"]), vec![]));
    let searched = r.searches();

    // Back (Backspace, Alt+Left, the mouse's back button): the results again, at once, as they were left.
    assert!(r.back(cx));
    assert_eq!(r.path(cx), "C:\\");
    assert_eq!(r.search(cx).as_deref(), Some("personal"));
    assert_eq!(r.filter(cx), "personal", "the box says what was searched for");
    assert_eq!(r.names(cx), ["Personal Stuff", "personal notes.txt"]);
    assert_eq!(r.selected(cx), (strings(&[STUFF]), Some(STUFF.to_string())));
    assert_eq!(r.searches(), searched, "nothing was searched for again");
    assert_eq!(r.history(cx), (strings(&["C:\\"]), strings(&[STUFF])));
    r.read(cx, |e| {
        let find = e.active_tab().and_then(|t| t.find.as_ref()).expect("results");
        assert!(!find.running && !find.inside.running);
    });

    // Back again: the folder the search began in, as it was before it (the whole folder, no words in the box).
    assert!(r.back(cx));
    assert_eq!(r.path(cx), "C:\\");
    assert_eq!(r.search(cx), None);
    assert_eq!(r.filter(cx), "");
    assert_eq!(r.names(cx), ["a", "b", "c"]);
    assert_eq!(r.history(cx), (vec![], strings(&["search:personal", STUFF])));
    assert!(!r.back(cx), "nothing further back");

    // Forward walks the same way again, still without searching.
    assert!(r.forward(cx));
    assert_eq!(r.search(cx).as_deref(), Some("personal"));
    assert_eq!(r.selected(cx).1.as_deref(), Some(STUFF));
    assert!(r.forward(cx));
    assert_eq!(r.path(cx), STUFF);
    assert_eq!(r.names(cx), ["passport.jpg"]);
    assert_eq!(r.searches(), searched);
    assert_eq!(r.history(cx), (strings(&["C:\\", "search:personal"]), vec![]));
}

#[gpui_kit::test]
fn escape_leaves_the_results_for_the_folder_as_it_was_and_forward_brings_them_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    r.go(cx, "C:\\a");
    r.explorer
        .update(cx, |e, cx| e.select_only(Some("C:\\a\\notes.md"), cx));
    r.find(cx, "report");
    assert_eq!(r.history(cx), (strings(&["C:\\", "C:\\a"]), vec![]));
    // Escape (the box emptied, then the results cleared): a step back to the folder, with notes.md selected again.
    r.explorer.update(cx, |e, cx| {
        e.set_filter("", None, cx);
        e.clear_find(cx)
    });
    cx.run_until_parked();
    assert_eq!(r.search(cx), None);
    assert_eq!(r.path(cx), "C:\\a");
    assert_eq!(r.selected(cx).1.as_deref(), Some("C:\\a\\notes.md"));
    assert_eq!(r.history(cx), (strings(&["C:\\"]), strings(&["search:report"])));
    // Forward: the results, with their words back in the box.
    assert!(r.forward(cx));
    assert_eq!(r.search(cx).as_deref(), Some("report"));
    assert_eq!(r.filter(cx), "report");
    assert_eq!(r.names(cx), ["report.pdf"]);
    // Back from them: the folder again; Back again: the drive.
    assert!(r.back(cx));
    assert_eq!((r.path(cx), r.search(cx)), ("C:\\a".to_string(), None));
    assert!(r.back(cx));
    assert_eq!(r.path(cx), "C:\\");
    assert_eq!(r.selected(cx).1.as_deref(), Some("C:\\a"), "the folder we came out of");
}

#[gpui_kit::test]
fn typing_on_refines_the_search_shown_and_stays_one_place(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    r.find(cx, "pers");
    r.find(cx, "personal");
    r.find(cx, "personal no");
    assert_eq!(r.names(cx), ["personal notes.txt"]);
    assert_eq!(r.history(cx), (strings(&["C:\\"]), vec![]));
    // Emptying the box ends it: the folder, with the last search Forward.
    r.explorer.update(cx, |e, cx| {
        e.set_filter("", None, cx);
        e.leave_find(None, cx)
    });
    cx.run_until_parked();
    assert_eq!(r.history(cx), (vec![], strings(&["search:personal no"])));
    // A new search forgets Forward, as any new way does.
    r.find(cx, "other");
    assert_eq!(r.history(cx), (strings(&["C:\\"]), vec![]));
}

#[gpui_kit::test]
fn up_from_the_results_is_their_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\a");
    r.find(cx, "notes");
    let up = r.explorer.update(cx, |e, cx| e.go_up(cx));
    assert!(cx.foreground_executor().block_test(up));
    cx.run_until_parked();
    assert_eq!(r.path(cx), "C:\\a", "not the folder above it");
    assert_eq!(r.search(cx), None);
    assert_eq!(r.filter(cx), "");
    assert_eq!(r.history(cx), (vec![], strings(&["search:notes"])));
    // Up again: the folder above.
    let up = r.explorer.update(cx, |e, cx| e.go_up(cx));
    assert!(cx.foreground_executor().block_test(up));
    assert_eq!(r.path(cx), "C:\\");
}

#[gpui_kit::test]
fn backspace_goes_back_and_with_nothing_to_go_back_to_goes_up(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, STUFF);
    assert_eq!(r.history(cx), (vec![], vec![]));
    let step = |r: &Rig, cx: &mut TestAppContext| {
        let task = r.explorer.update(cx, |e, cx| e.go_back_or_up(cx));
        let went = cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        went
    };
    // Nothing to go back to: up, onto the folder we came out of.
    assert!(step(&r, cx));
    assert_eq!(r.path(cx), "C:\\a");
    assert_eq!(r.selected(cx).1.as_deref(), Some(STUFF));
    // Now there is: back.
    assert!(step(&r, cx));
    assert_eq!(r.path(cx), STUFF);
    assert_eq!(r.history(cx), (vec![], strings(&["C:\\a"])));
}

#[gpui_kit::test]
fn a_folder_gone_from_the_disk_is_skipped_saying_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\a");
    r.go(cx, "C:\\b");
    r.go(cx, "C:\\c");
    // b is deleted meanwhile.
    r.disk.remove("C:\\b");
    r.disk.folder("C:\\", vec![folder("C:\\", "a"), folder("C:\\", "c")]);
    assert!(r.back(cx));
    assert_eq!(r.path(cx), "C:\\a", "on to the place before it");
    assert!(r
        .notices()
        .contains(&Notice::NoLongerAvailable { path: "C:\\b".into() }));
    assert!(!r.notices().iter().any(|n| matches!(n, Notice::CannotOpen { .. })));
    assert_eq!(r.history(cx), (vec![], strings(&["C:\\c"])));
}

#[gpui_kit::test]
fn a_folder_that_fails_for_another_reason_stays_and_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\a");
    r.go(cx, "C:\\b");
    // a can't be listed, though the drive still says it is there (a device that isn't ready, say).
    r.disk.remove("C:\\a");
    assert!(!r.back(cx));
    assert_eq!(r.path(cx), "C:\\b");
    assert!(r.notices().iter().any(|n| matches!(n, Notice::CannotOpen { .. })));
    assert_eq!(r.history(cx), (strings(&["C:\\a"]), vec![]), "still there to try again");
}

#[gpui_kit::test]
fn results_gone_from_the_disk_are_taken_out_when_they_come_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    r.find(cx, "personal");
    r.open(cx, STUFF);
    // personal notes.txt is deleted meanwhile.
    r.disk.folder("C:\\b", vec![file("C:\\b", "other.txt", 40, 4)]);
    assert!(r.back(cx));
    assert_eq!(r.names(cx), ["Personal Stuff"]);
    assert_eq!(r.selected(cx).1.as_deref(), Some(STUFF));
}

#[gpui_kit::test]
fn the_selection_and_the_scroll_of_every_place_come_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\a");
    let tab = r.read(cx, |e| e.active_tab_id());
    r.explorer.update(cx, |e, cx| {
        e.select_only(Some("C:\\a\\report.pdf"), cx);
        e.save_scroll(tab, "dir:C:\\a", 120.0);
    });
    r.go(cx, "C:\\b");
    r.explorer.update(cx, |e, _| e.save_scroll(tab, "dir:C:\\b", 40.0));
    r.go(cx, "C:\\c");
    assert!(r.back(cx));
    assert_eq!(r.read(cx, |e| e.saved_scroll(tab, "dir:C:\\b")), 40.0);
    assert!(r.back(cx));
    // Two places back, its own scroll and selection, not the last folder's.
    assert_eq!(r.read(cx, |e| e.saved_scroll(tab, "dir:C:\\a")), 120.0);
    assert_eq!(
        r.selected(cx),
        (strings(&["C:\\a\\report.pdf"]), Some("C:\\a\\report.pdf".into()))
    );
}

#[gpui_kit::test]
fn results_still_coming_in_when_left_are_searched_for_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    let live = r.disk.live_find("personal");
    let running = r.explorer.update(cx, |e, cx| {
        e.set_filter("personal", None, cx);
        e.run_find("personal", Wider::default(), cx)
    });
    cx.run_until_parked();
    let half = vec![folder("C:\\a", "Personal Stuff")];
    live.try_send(Found::Entries(half)).unwrap();
    cx.run_until_parked();
    assert_eq!(r.names(cx), ["Personal Stuff"]);
    r.open(cx, STUFF);
    drop(running);
    let searched = r.searches();
    // Back: what had come shows at once, and the search runs again for the rest.
    assert!(r.back(cx));
    assert_eq!(r.search(cx).as_deref(), Some("personal"));
    assert_eq!(r.searches(), searched + 1);
    assert_eq!(r.names(cx), ["Personal Stuff", "personal notes.txt"]);
    assert_eq!(
        r.history(cx),
        (strings(&["C:\\"]), strings(&[STUFF])),
        "searching again changed nothing here"
    );
}

#[gpui_kit::test]
fn a_search_too_far_back_to_keep_its_results_is_searched_for_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    r.find(cx, "personal");
    r.open(cx, STUFF);
    // Its results let go, as an older search's are.
    let tab = r.read(cx, |e| e.active_tab_id());
    r.explorer.update(cx, |e, _| {
        e.update_tab(tab, |t| {
            if let Some(search) = t.back.last_mut().and_then(|v| v.search.as_mut()) {
                search.run = None;
            }
        });
    });
    let searched = r.searches();
    assert!(r.back(cx));
    assert_eq!(r.searches(), searched + 1);
    assert_eq!(r.names(cx), ["Personal Stuff", "personal notes.txt"]);
    assert_eq!(r.history(cx), (strings(&["C:\\"]), strings(&[STUFF])));
}

#[gpui_kit::test]
fn going_straight_to_a_place_further_back_moves_the_ones_between_forward(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\");
    r.find(cx, "personal");
    r.open(cx, STUFF);
    r.go(cx, "C:\\c");
    assert_eq!(r.history(cx).0, strings(&["C:\\", "search:personal", STUFF]));
    // The Back menu's second place: the results.
    let jump = r.explorer.update(cx, |e, cx| e.go_through_history(true, 2, cx));
    assert!(cx.foreground_executor().block_test(jump));
    cx.run_until_parked();
    assert_eq!(r.search(cx).as_deref(), Some("personal"));
    assert_eq!(r.history(cx), (strings(&["C:\\"]), strings(&[STUFF, "C:\\c"])));
    // And Forward's last, two at once.
    let jump = r.explorer.update(cx, |e, cx| e.go_through_history(false, 2, cx));
    assert!(cx.foreground_executor().block_test(jump));
    cx.run_until_parked();
    assert_eq!(r.path(cx), "C:\\c");
    assert_eq!(r.history(cx), (strings(&["C:\\", "search:personal", STUFF]), vec![]));
    // There is no fourth place back.
    let jump = r.explorer.update(cx, |e, cx| e.go_through_history(true, 4, cx));
    assert!(!cx.foreground_executor().block_test(jump));
}

#[gpui_kit::test]
fn a_saved_search_is_a_place_after_its_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\c");
    let task = r.explorer.update(cx, |e, cx| {
        e.run_saved_search(
            glint_model::store::SavedSearch {
                query: "notes".into(),
                folder: "C:\\a".into(),
            },
            cx,
        )
    });
    cx.foreground_executor().block_test(task);
    cx.run_until_parked();
    assert_eq!(r.search(cx).as_deref(), Some("notes"));
    assert_eq!(r.history(cx), (strings(&["C:\\c", "C:\\a"]), vec![]));
    assert!(r.back(cx));
    assert_eq!((r.path(cx), r.search(cx)), ("C:\\a".to_string(), None));
    assert!(r.back(cx));
    assert_eq!(r.path(cx), "C:\\c");
}

#[gpui_kit::test]
fn back_pressed_again_while_the_last_place_still_opens_goes_on_from_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let r = rig(cx);
    r.go(cx, "C:\\a");
    r.go(cx, "C:\\b");
    r.go(cx, "C:\\c");
    // b takes its time to list; Backspace is held meanwhile.
    let gate = r.disk.hold("C:\\b");
    let first = r.explorer.update(cx, |e, cx| e.go_back(cx));
    cx.run_until_parked();
    let second = r.explorer.update(cx, |e, cx| e.go_back(cx));
    drop(gate);
    assert!(cx.foreground_executor().block_test(second));
    assert!(
        !cx.foreground_executor().block_test(first),
        "overtaken by the second press"
    );
    cx.run_until_parked();
    assert_eq!(r.path(cx), "C:\\a", "two presses, two places back");
    assert_eq!(r.history(cx), (vec![], strings(&["C:\\b", "C:\\c"])));
}
