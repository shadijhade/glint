//! The rows' marks reach the rows (docs/gpui/marks, 6.1): while a tab's filter holds a marks key, or its sort or its
//! group is by a mark, every entry's marks are read (`RowFacts::ask_all`), and once they are all in they are handed
//! to the tab's rows, which are built once. At any other time nothing is read and the rows cost what they cost.
//!
//! Only a tab a pane shows is asked about. Nothing here waits or loops: it runs when the explorer says a tab, its
//! listing or its view changed, when the rows' facts say something landed (a read, a mark changed, a folder listed
//! again) and when the settings change (the labels), and every run only reads what is in memory.

use super::rows::RowFileMarks;
use super::tab::Tab;
use super::types::{ExplorerEvent, TabId};
use super::try_explorer;
use crate::components::row_facts;
use crate::state::{self, UiState};
use glint_model::entries::FileEntry;
use glint_model::marks::Label;
use gpui_kit::{App, Global, Subscription};
use std::collections::HashMap;
use std::sync::Arc;

/// What the marks of a tab were made from, to tell whether a new map is needed.
struct Built {
    key: String,
    entries: Arc<[FileEntry]>,
    labels: Arc<[Label]>,
    /// Every row's marks were read when it was made, and nothing has been said to change them since.
    ready: bool,
}

#[derive(Default)]
struct Follow {
    built: HashMap<TabId, Built>,
    labels: Arc<[Label]>,
    _subscriptions: Vec<Subscription>,
}

impl Global for Follow {}

/// Follows the explorer and the rows' facts from now on. Called once, at start-up, after both are there.
pub fn init(cx: &mut App) {
    if cx.try_global::<Follow>().is_some() {
        return;
    }
    let Some(explorer) = try_explorer(cx) else {
        return;
    };
    let facts = row_facts::shared(cx);
    let mut subscriptions = vec![
        cx.subscribe(&explorer, |_, event: &ExplorerEvent, cx| match event {
            ExplorerEvent::TabClosed { tab, .. } => {
                cx.global_mut::<Follow>().built.remove(tab);
                sync(cx);
            }
            ExplorerEvent::TabsChanged
            | ExplorerEvent::TabChanged(_)
            | ExplorerEvent::ListingChanged { .. }
            | ExplorerEvent::ViewChanged(_) => sync(cx),
            _ => {}
        }),
        cx.observe(&facts, |_, cx| sync(cx)),
    ];
    if cx.try_global::<UiState>().is_some() {
        subscriptions.push(cx.observe_global::<UiState>(sync));
    }
    cx.set_global(Follow {
        _subscriptions: subscriptions,
        ..Follow::default()
    });
    sync(cx);
}

/// What one tab needs of this run.
struct Job {
    id: TabId,
    key: String,
    entries: Arc<[FileEntry]>,
    wants: bool,
    shown: bool,
    holds: bool,
}

fn entries_of(tab: &Tab) -> Arc<[FileEntry]> {
    match &tab.find {
        Some(find) => find.entries.clone(),
        None => tab.shared_entries().unwrap_or_else(|| Arc::from(Vec::new())),
    }
}

fn sync(cx: &mut App) {
    let (Some(explorer), Some(facts)) = (try_explorer(cx), row_facts::try_shared(cx)) else {
        return;
    };
    let from_settings = state::try_settings(cx).map(|s| s.labels.clone()).unwrap_or_default();
    let labels = {
        let follow = cx.global_mut::<Follow>();
        if follow.labels[..] != from_settings[..] {
            follow.labels = from_settings.into();
        }
        follow.labels.clone()
    };
    let jobs: Vec<Job> = {
        let explorer = explorer.read(cx);
        let shown = explorer.visible_tabs();
        explorer
            .tabs
            .values()
            .filter(|tab| tab.marked.is_some() || shown.contains(&tab.id))
            .map(|tab| Job {
                id: tab.id,
                key: tab.view_key(),
                entries: entries_of(tab),
                wants: explorer.wants_row_marks(tab.id),
                shown: shown.contains(&tab.id),
                holds: tab.marked.is_some(),
            })
            .collect()
    };
    for job in jobs {
        if !job.wants {
            cx.global_mut::<Follow>().built.remove(&job.id);
            if job.holds {
                explorer.update(cx, |e, cx| e.set_row_marks(job.id, None, cx));
            }
            continue;
        }
        if !job.shown {
            continue;
        }
        facts.update(cx, |facts, cx| facts.ask_all(&job.key, &job.entries, cx));
        let all = facts.read(cx).has_all(&job.key);
        if !all {
            // The tab keeps what it had, unless that is another folder's.
            let follow = cx.global_mut::<Follow>();
            let other = follow.built.get(&job.id).is_some_and(|built| built.key != job.key);
            match follow.built.get_mut(&job.id) {
                Some(built) if !other => built.ready = false,
                _ => {
                    follow.built.remove(&job.id);
                    if job.holds {
                        explorer.update(cx, |e, cx| e.set_row_marks(job.id, None, cx));
                    }
                }
            }
            continue;
        }
        let current = cx.global::<Follow>().built.get(&job.id).is_some_and(|built| {
            built.ready
                && built.key == job.key
                && Arc::ptr_eq(&built.entries, &job.entries)
                && Arc::ptr_eq(&built.labels, &labels)
        });
        if current {
            continue;
        }
        let map: HashMap<_, _> = {
            let facts = facts.read(cx);
            job.entries
                .iter()
                .filter_map(|e| Some((e.path.clone(), facts.marks_of(&job.key, &e.path)?.clone())))
                .collect()
        };
        let marks = RowFileMarks {
            marks: Arc::new(map),
            labels: labels.clone(),
        };
        cx.global_mut::<Follow>().built.insert(
            job.id,
            Built {
                key: job.key.clone(),
                entries: job.entries.clone(),
                labels: labels.clone(),
                ready: true,
            },
        );
        explorer.update(cx, |e, cx| e.set_row_marks(job.id, Some(marks), cx));
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{file, folder, FakeDisk};
    use super::super::{Explorer, ExplorerHandle};
    use super::*;
    use crate::components::row_facts::{fake, init_with};
    use crate::dialogs::tags::{self, TagsChanged};
    use crate::testing;
    use glint_model::entries::{Sort, SortKey};
    use glint_model::marks::Marks;
    use gpui_kit::{AppContext as _, Entity, TestAppContext};

    struct World {
        explorer: Entity<super::super::Explorer>,
        backend: Arc<fake::MarksBackend>,
        rows_changed: std::rc::Rc<std::cell::Cell<usize>>,
    }

    fn marks(label: Option<&str>, tags: &[&str]) -> Marks {
        Marks {
            label: label.map(String::from),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment: None,
        }
    }

    /// An explorer on a fake disk with `C:\a` (four files) and `C:\b`, the shared facts on a marks backend, and
    /// this file's follower.
    fn world(cx: &mut TestAppContext, items: &[(&str, Marks)]) -> World {
        cx.executor().allow_parking();
        cx.update(testing::init);
        let backend = fake::MarksBackend::with(items);
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![folder("C:\\", "a"), folder("C:\\", "b")]);
        disk.folder(
            "C:\\a",
            vec![
                file("C:\\a", "a1.txt", 1, 1),
                file("C:\\a", "a2.txt", 1, 1),
                file("C:\\a", "a3.txt", 1, 1),
                file("C:\\a", "a4.txt", 1, 1),
            ],
        );
        disk.folder("C:\\b", vec![file("C:\\b", "b1.txt", 1, 1)]);
        let rows_changed = std::rc::Rc::new(std::cell::Cell::new(0));
        let explorer = cx.update(|cx| {
            let dyn_backend: Arc<dyn tags::Backend> = backend.clone();
            tags::init_with(cx, dyn_backend);
            init_with(cx, fake::Media::with(vec![]));
            let explorer = cx.new(|cx| Explorer::new(disk.source(), cx));
            cx.set_global(ExplorerHandle(explorer.clone()));
            let count = rows_changed.clone();
            cx.subscribe(&explorer, move |_, event: &ExplorerEvent, _| {
                if matches!(event, ExplorerEvent::RowsChanged(_)) {
                    count.set(count.get() + 1);
                }
            })
            .detach();
            row_facts::shared(cx);
            init(cx);
            explorer
        });
        let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\a", Default::default(), cx));
        cx.foreground_executor().block_test(task);
        cx.run_until_parked();
        World {
            explorer,
            backend,
            rows_changed,
        }
    }

    impl World {
        fn names(&self, cx: &TestAppContext) -> Vec<String> {
            self.explorer.read_with(cx, |e, _| {
                e.active_tab()
                    .map(|t| t.rows.iter().map(|r| r.name.clone()).collect())
                    .unwrap_or_default()
            })
        }

        fn held(&self, cx: &TestAppContext) -> bool {
            self.explorer
                .read_with(cx, |e, _| e.active_tab().is_some_and(|t| t.marked.is_some()))
        }

        fn spec_has_marks(&self, cx: &TestAppContext) -> bool {
            self.explorer.read_with(cx, |e, _| {
                e.active_tab()
                    .and_then(|t| t.rows_spec.as_ref())
                    .is_some_and(|s| s.marked.is_some())
            })
        }

        fn sort_by(&self, cx: &mut TestAppContext, key: SortKey) {
            self.explorer
                .update(cx, |e, cx| e.set_sort(Sort { key, desc: false }, cx));
            cx.run_until_parked();
        }

        fn wait_for(&self, cx: &mut TestAppContext, mut done: impl FnMut(&World, &mut TestAppContext) -> bool) {
            fake::settle(cx, |cx| done(self, cx));
        }
    }

    fn some_marks() -> Vec<(&'static str, Marks)> {
        vec![
            ("C:\\a\\a1.txt", marks(Some("Blue"), &[])),
            ("C:\\a\\a2.txt", marks(Some("Red"), &["work"])),
            ("C:\\a\\a3.txt", marks(None, &["home"])),
        ]
    }

    #[gpui_kit::test]
    fn a_tab_sorted_by_label_asks_for_every_rows_marks_once_and_a_tab_sorted_by_name_asks_for_nothing(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let w = world(cx, &some_marks());
        assert_eq!(w.backend.paths_read(), 0, "a name sort reads no marks");
        assert!(!w.held(cx) && !w.spec_has_marks(cx));
        w.sort_by(cx, SortKey::Label);
        w.wait_for(cx, |w, cx| w.held(cx));
        assert_eq!(w.backend.calls().len(), 1, "one call");
        assert_eq!(w.backend.paths_read(), 4);
        // The default labels' order: Red before Blue; the rest by name, then none.
        w.wait_for(cx, |w, cx| w.names(cx).first().map(String::as_str) == Some("a2.txt"));
        assert_eq!(w.names(cx), ["a2.txt", "a1.txt", "a3.txt", "a4.txt"]);
        // Back to a name sort: the map is let go and nothing more is read.
        w.sort_by(cx, SortKey::Name);
        assert!(!w.held(cx) && !w.spec_has_marks(cx));
        assert_eq!(w.names(cx), ["a1.txt", "a2.txt", "a3.txt", "a4.txt"]);
        assert_eq!(w.backend.calls().len(), 1);
    }

    #[gpui_kit::test]
    fn a_marks_filter_shows_every_row_until_the_marks_are_in_then_only_the_tagged_and_clearing_it_lets_the_map_go(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let w = world(cx, &some_marks());
        w.explorer.update(cx, |e, cx| e.set_filter("tag:work", None, cx));
        // Not read yet (the read is on its way): every row shows.
        assert_eq!(w.names(cx).len(), 4);
        w.wait_for(cx, |w, cx| w.names(cx).len() == 1);
        assert_eq!(w.names(cx), ["a2.txt"]);
        w.explorer.update(cx, |e, cx| e.set_filter("", None, cx));
        cx.run_until_parked();
        assert_eq!(w.names(cx).len(), 4);
        assert!(!w.held(cx), "the map is let go");
    }

    #[gpui_kit::test]
    fn a_mark_changed_and_said_brings_the_rows_up_to_date_once_read_again_and_a_folder_listed_again_too(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let w = world(cx, &some_marks());
        w.explorer.update(cx, |e, cx| e.set_filter("tag:work", None, cx));
        w.wait_for(cx, |w, cx| w.names(cx).len() == 1);
        // a4 is tagged now, and the hub says so.
        w.backend.put("C:\\a\\a4.txt", marks(None, &["work"]));
        let hub = cx.update(tags::hub);
        hub.update(cx, |_, cx| {
            cx.emit(TagsChanged {
                paths: vec!["C:\\a\\a4.txt".into()],
            })
        });
        w.wait_for(cx, |w, cx| w.names(cx) == ["a2.txt", "a4.txt"]);
        // The folder is listed again: read again, and what is new in it shows.
        w.backend.put("C:\\a\\a1.txt", marks(None, &["work"]));
        let before = w.backend.calls().len();
        w.explorer.update(cx, |e, cx| e.reload(None, cx).detach());
        w.wait_for(cx, |w, cx| w.names(cx) == ["a1.txt", "a2.txt", "a4.txt"]);
        assert!(w.backend.calls().len() > before);
    }

    #[gpui_kit::test]
    fn the_same_marks_handed_over_twice_rebuild_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let w = world(cx, &some_marks());
        w.sort_by(cx, SortKey::Label);
        w.wait_for(cx, |w, cx| w.held(cx));
        w.wait_for(cx, |w, cx| w.names(cx).first().map(String::as_str) == Some("a2.txt"));
        let (tab, same) = w.explorer.read_with(cx, |e, _| {
            let tab = e.active_tab().expect("a tab");
            (tab.id, tab.marked.clone())
        });
        let before = w.rows_changed.get();
        for _ in 0..2 {
            w.explorer
                .update(cx, |e, cx| e.set_row_marks(tab, same.clone(), cx));
        }
        cx.run_until_parked();
        assert_eq!(w.rows_changed.get(), before);
        // Nor does the follower, asked again by an unrelated change of the view, make another map.
        w.explorer.update(cx, |_, cx| cx.emit(ExplorerEvent::ViewChanged(tab)));
        cx.run_until_parked();
        assert_eq!(w.rows_changed.get(), before);
    }

    #[gpui_kit::test]
    fn a_tab_no_pane_shows_is_never_asked_about(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let w = world(cx, &some_marks());
        let first = w.explorer.read_with(cx, |e, _| e.active_tab_id());
        w.explorer.update(cx, |e, cx| e.new_tab(Some("C:\\b"), cx));
        cx.run_until_parked();
        let shown = w.explorer.read_with(cx, |e, _| e.active_tab_id());
        assert_ne!(first, shown);
        w.explorer
            .update(cx, |e, cx| e.set_filter("tag:work", Some(first), cx));
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(30));
        cx.run_until_parked();
        assert_eq!(w.backend.paths_read(), 0, "nothing asked for the tab out of sight");
        // Brought on screen, it is.
        w.explorer.update(cx, |e, cx| e.activate_tab(first, cx));
        w.wait_for(cx, |w, cx| w.names(cx) == ["a2.txt"]);
        // (Coming on screen lists the folder again, which reads again: the first call is what counts here.)
        assert_eq!(w.backend.calls().first().map(Vec::len), Some(4));
    }
}
