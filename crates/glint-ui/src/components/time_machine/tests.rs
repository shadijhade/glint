//! Time Machine's tests: the model (streaming, selection, the scrubbing cache, the source filter) and the view
//! (the rail, its keys and its playhead, the chips, Quick Look on a row), headless on GPUI's test platform.

use super::*;
use crate::explorer;
use crate::explorer::fake::{file, FakeDisk};
use crate::explorer::NavOptions;
use crate::testing;
use glint_model::time_machine::EntryDelta;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{point, TestAppContext, VisualTestContext};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

fn moment(id: &str, ts: i64, source: MomentSource) -> Moment {
    Moment {
        id: id.into(),
        timestamp_ms: ts,
        label: format!("label {id}"),
        detail: id.chars().take(7).collect(),
        source,
        shadow_path: matches!(source, MomentSource::VssShadow).then(|| format!("\\\\?\\shadow\\{id}")),
        commit_oid: matches!(source, MomentSource::GitCommit).then(|| id.to_string()),
        repo_root: matches!(source, MomentSource::GitCommit).then(|| "C:\\Work".to_string()),
        plan_id: None,
        snapshot_id: None,
    }
}

fn entry_of(m: &Moment, name: &str, delta: EntryDelta) -> TimeEntry {
    TimeEntry {
        name: name.into(),
        dir: false,
        size: 10,
        modified: m.timestamp_ms,
        delta,
        restore: RestoreSource::ShadowPath(format!("shadow\\{}\\{name}", m.id)),
    }
}

/// A source whose moments come in two batches (the second after `gate` fires, so a test can look at the first
/// alone), counting every read, and staging into `staged`.
struct Fake {
    first: Vec<Moment>,
    later: Vec<Moment>,
    gate: Option<async_channel::Receiver<()>>,
    reads: Arc<AtomicUsize>,
    staged: Arc<Mutex<Vec<String>>>,
    stage_to: Option<String>,
    restored: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn new(first: Vec<Moment>) -> Self {
        Fake {
            first,
            later: Vec::new(),
            gate: None,
            reads: Arc::default(),
            staged: Arc::default(),
            stage_to: None,
            restored: Arc::default(),
        }
    }

    fn source(self) -> Source {
        let Fake {
            first,
            later,
            gate,
            reads,
            staged,
            stage_to,
            restored,
        } = self;
        Source {
            moments: Arc::new(move |_folder, sink| {
                let (first, later, gate) = (first.clone(), later.clone(), gate.clone());
                Box::pin(async move {
                    sink(first);
                    if let Some(gate) = gate {
                        let _ = gate.recv().await;
                    }
                    if !later.is_empty() {
                        sink(later);
                    }
                    Ok(())
                })
            }),
            inspect: Arc::new(move |_folder, m| {
                reads.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    Ok(vec![
                        entry_of(&m, "lost.txt", EntryDelta::Deleted),
                        entry_of(&m, "notes.txt", EntryDelta::Unchanged),
                    ])
                })
            }),
            restore: Arc::new(move |folder, item, _| {
                let path = join_path(&folder, &item.name);
                restored.lock().unwrap().push(path.clone());
                Box::pin(async move { Ok(path) })
            }),
            stage: Arc::new(move |item| {
                let staged_path = stage_to
                    .clone()
                    .unwrap_or_else(|| format!("C:\\Temp\\staged-{}", item.name));
                staged.lock().unwrap().push(staged_path.clone());
                Box::pin(async move { Ok(staged_path) })
            }),
        }
    }
}

/// A window with the view in pane 0, the tab on `C:\Work`, Time Machine on, and the view focused.
fn open(
    cx: &mut TestAppContext,
    source: Source,
) -> (
    Entity<TimeMachineView>,
    VisualTestContext,
    TabId,
    Arc<Mutex<Vec<TimeMachineEvent>>>,
) {
    let disk = FakeDisk::new();
    disk.folder("C:\\Work", vec![file("C:\\Work", "notes.txt", 25, 0)]);
    cx.update(|cx| {
        testing::init(cx);
        explorer::init_with(cx, disk.source());
        init_with(cx, source);
    });
    let (mut visual, view) = testing::open_window(cx, |window, cx| cx.new(|cx| TimeMachineView::new(0, window, cx)));
    let events: Arc<Mutex<Vec<TimeMachineEvent>>> = Arc::default();
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, ev: &TimeMachineEvent, _| {
            sink.lock().unwrap().push(ev.clone());
        })
        .detach();
        explorer::explorer(cx).update(cx, |e, cx| e.navigate("C:\\Work", NavOptions::default(), cx));
    });
    visual.run_until_parked();
    let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
    visual.update(|_, cx| toggle(cx, tab));
    visual.run_until_parked();
    let focus = view.read_with(&visual, |v, _| v.focus_handle.clone());
    visual.update(|window, cx| window.focus(&focus, cx));
    visual.update(|window, cx| window.render_frame(cx));
    (view, visual, tab, events)
}

fn tab_state(visual: &mut VisualTestContext, tab: TabId) -> TabState {
    visual.update(|_, cx| shared(cx).read(cx).tabs.get(&tab).cloned().unwrap())
}

fn model(visual: &mut VisualTestContext) -> Entity<TimeMachineModel> {
    visual.update(|_, cx| shared(cx))
}

fn selected_id(visual: &mut VisualTestContext, tab: TabId) -> String {
    tab_state(visual, tab)
        .selected_moment()
        .map(|m| m.id.clone())
        .unwrap_or_default()
}

#[gpui_kit::test]
fn scrubber_loads_moments_switches_filters_and_emits_restore_and_compare(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut fake = Fake::new(vec![
        moment("m-new", 2_000_000_000_000, MomentSource::GitCommit),
        moment("m-old", 1_000_000_000_000, MomentSource::VssShadow),
    ]);
    let restored = fake.restored.clone();
    let staged = fake.staged.clone();
    fake.stage_to = None;
    let (_view, mut visual, tab, events) = open(cx, fake.source());

    assert!(visual.update(|_, cx| visible(cx, tab)));

    // Chronological order: index 0 is m-old, index 1 is m-new (selected by default).
    let state = tab_state(&mut visual, tab);
    assert_eq!(state.moments.len(), 2);
    assert_eq!(state.moments[0].id, "m-old");
    assert_eq!(state.selected, 1);
    assert_eq!(state.entries.len(), 2);

    // Compare an entry of m-new against today.
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(("tm-compare", 1usize), cx);
    });
    visual.run_until_parked();
    assert!(staged.lock().unwrap().iter().any(|p| p.contains("notes.txt")));

    // Click the older stop and filter to Deleted.
    visual.update(|window, cx| window.click(("tm-moment", 0usize), cx));
    visual.run_until_parked();
    assert_eq!(selected_id(&mut visual, tab), "m-old");
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.click("tm-filter-deleted", cx);
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(("tm-restore", 0usize), cx);
    });
    visual.run_until_parked();

    assert_eq!(restored.lock().unwrap().as_slice(), &["C:\\Work\\lost.txt"]);
    let collected = events.lock().unwrap();
    assert!(collected.iter().any(|e| matches!(e, TimeMachineEvent::Compare { .. })));
    assert!(collected.iter().any(|e| matches!(e, TimeMachineEvent::Restored { .. })));
    drop(collected);

    // Esc / close button hides and clears cached tab state.
    visual.update(|window, cx| window.click("tm-close", cx));
    visual.run_until_parked();
    assert!(!visual.update(|_, cx| shows(cx, tab)));
    assert!(visual.update(|_, cx| !shared(cx).read(cx).tabs.contains_key(&tab)));
    assert!(visual.update(|_, cx| !shared(cx).read(cx).caches.contains_key(&tab)));
}

#[gpui_kit::test]
fn the_first_stops_show_while_slower_sources_are_still_asked_and_the_selection_stays(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (open_gate, gate) = async_channel::bounded::<()>(1);
    let mut fake = Fake::new(vec![
        moment("git-b", 2_000_000_000_000, MomentSource::GitCommit),
        moment("git-a", 1_800_000_000_000, MomentSource::GitCommit),
    ]);
    // The slower source brings a stop newer than anything Git had, and one older.
    fake.later = vec![
        moment("vss-new", 2_100_000_000_000, MomentSource::VssShadow),
        moment("vss-old", 1_000_000_000_000, MomentSource::VssShadow),
    ];
    fake.gate = Some(gate);
    let (_view, mut visual, tab, _) = open(cx, fake.source());

    // The fast source's stops are on the rail and the newest is read, with the other still pending.
    let state = tab_state(&mut visual, tab);
    assert!(state.moments_loading, "a source has not answered yet");
    assert_eq!(state.moments.len(), 2);
    assert_eq!(state.selected_moment().map(|m| m.id.as_str()), Some("git-b"));
    assert_eq!(
        state.entries.len(),
        2,
        "the first stop was read while the others are asked"
    );
    visual.update(|window, cx| window.render_frame(cx));
    assert!(visual.update(|window, _| window.try_find_by_label("Git commit git-b, just now").is_some()));

    open_gate.send_blocking(()).unwrap();
    visual.run_until_parked();
    let state = tab_state(&mut visual, tab);
    assert!(!state.moments_loading);
    let ids: Vec<_> = state.moments.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["vss-old", "git-a", "git-b", "vss-new"], "merged, oldest first");
    assert_eq!(
        selected_id(&mut visual, tab),
        "git-b",
        "the stop on screen stays selected"
    );
}

#[gpui_kit::test]
fn stepping_back_and_forth_shows_kept_stops_at_once_and_reads_each_stop_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let moments: Vec<Moment> = (0..6)
        .rev()
        .map(|i| {
            moment(
                &format!("git-{i}"),
                1_000_000_000_000 + i * 1_000_000,
                MomentSource::GitCommit,
            )
        })
        .collect();
    let fake = Fake::new(moments);
    let reads = fake.reads.clone();
    let (_view, mut visual, tab, _) = open(cx, fake.source());
    let model = model(&mut visual);

    // On the newest stop (5): it was read, and so was its neighbour (4).
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(tab_state(&mut visual, tab).selected, 5);

    // One step older: kept, on screen in the same update, no read; the next neighbour (3) is read ahead.
    let start = Instant::now();
    visual.update(|_, cx| model.update(cx, |m, cx| m.step_moment(tab, -1, None, cx)));
    let cached_step = start.elapsed();
    let state = tab_state(&mut visual, tab);
    assert_eq!(
        (state.selected, state.entries_loading, state.entries.len()),
        (4, false, 2)
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2, "a kept stop is not read again");
    visual.run_until_parked();
    assert_eq!(reads.load(Ordering::SeqCst), 3, "the stop beyond it was read ahead");

    // Back and forth over kept stops reads nothing more.
    for delta in [-1, 1, 1, -1] {
        visual.update(|_, cx| model.update(cx, |m, cx| m.step_moment(tab, delta, None, cx)));
        assert!(!tab_state(&mut visual, tab).entries_loading);
        visual.run_until_parked();
    }
    assert!(
        reads.load(Ordering::SeqCst) <= 5,
        "only the stops beside the ones visited were read"
    );

    // A jump to a stop nobody read ahead has to be read: it shows "Reading…" until the read lands.
    let start = Instant::now();
    visual.update(|_, cx| model.update(cx, |m, cx| m.jump_moment(tab, true, cx)));
    assert!(tab_state(&mut visual, tab).entries_loading);
    visual.run_until_parked();
    let uncached_step = start.elapsed();
    assert!(!tab_state(&mut visual, tab).entries_loading);
    println!("time machine step: kept stop {cached_step:?} (shown in the same update), unread stop {uncached_step:?}");
}

#[gpui_kit::test]
fn the_rail_has_a_named_tick_per_stop_and_the_keys_move_the_playhead(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let hour = 3_600_000;
    let (_view, mut visual, tab, _) = open(
        cx,
        Fake::new(vec![
            moment("3f2a1c9", now - 2 * hour, MomentSource::GitCommit),
            moment("aaaaaaa", now - 30 * hour, MomentSource::GitCommit),
            moment("vss-1", now - 100 * hour, MomentSource::VssShadow),
        ])
        .source(),
    );
    visual.update(|window, cx| window.render_frame(cx));
    let named = |visual: &mut VisualTestContext, label: &str| {
        visual.update(|window, _| window.try_find_by_label(label).is_some())
    };
    assert!(named(&mut visual, "Git commit 3f2a1c9, 2 hours ago"));
    assert!(named(&mut visual, "Git commit aaaaaaa, 1 day ago"));
    assert!(named(&mut visual, "Time Machine position"));
    assert_eq!(
        visual.update(|window, _| window
            .find_all_by_role(Role::Button)
            .iter()
            .filter(|e| e
                .label()
                .is_some_and(|l| l.starts_with("Git commit") || (l.starts_with("Windows") && l != "Windows Restore")))
            .count()),
        3
    );

    assert_eq!(tab_state(&mut visual, tab).selected, 2);
    visual.update(|window, cx| window.press("left", cx));
    visual.run_until_parked();
    assert_eq!(tab_state(&mut visual, tab).selected, 1);
    visual.update(|window, cx| window.press("home", cx));
    visual.run_until_parked();
    assert_eq!(tab_state(&mut visual, tab).selected, 0);
    visual.update(|window, cx| window.press("left", cx));
    assert_eq!(tab_state(&mut visual, tab).selected, 0, "never past the oldest");
    visual.update(|window, cx| window.press("end", cx));
    visual.run_until_parked();
    assert_eq!(tab_state(&mut visual, tab).selected, 2);
    visual.update(|window, cx| window.press("right", cx));
    assert_eq!(tab_state(&mut visual, tab).selected, 2, "never past today");
}

#[gpui_kit::test]
fn dragging_the_playhead_snaps_to_stops_ticks_a_detent_for_each_and_reads_where_it_rests(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let day = 86_400_000;
    let base = 1_700_000_000_000;
    let fake = Fake::new(vec![
        moment("m4", base + 40 * day, MomentSource::GitCommit),
        moment("m3", base + 30 * day, MomentSource::GitCommit),
        moment("m2", base + 20 * day, MomentSource::GitCommit),
        moment("m1", base + 10 * day, MomentSource::GitCommit),
        moment("m0", base, MomentSource::GitCommit),
    ]);
    let reads = fake.reads.clone();
    let (view, mut visual, tab, _) = open(cx, fake.source());
    visual.update(|window, cx| window.render_frame(cx));
    assert_eq!(tab_state(&mut visual, tab).selected, 4);

    let at = |visual: &mut VisualTestContext, ix: usize| {
        let name = rail::stop_name(
            &moment(&format!("m{ix}"), base + ix as i64 * 10 * day, MomentSource::GitCommit),
            0,
        );
        let _ = name;
        visual.update(|window, _| {
            window
                .elements()
                .into_iter()
                .find(|e| e.label().is_some_and(|l| l.starts_with(&format!("Git commit m{ix}"))))
                .map(|e| e.bounds().center())
                .unwrap()
        })
    };
    let (p4, p3, p1, p0) = (
        at(&mut visual, 4),
        at(&mut visual, 3),
        at(&mut visual, 1),
        at(&mut visual, 0),
    );
    let detents =
        |view: &Entity<TimeMachineView>, visual: &mut VisualTestContext| view.read_with(visual, |v, _| v.detents);
    let before = detents(&view, &mut visual);

    // Press on the newest stop (nothing changes), drag across 3, 1 and onto the oldest.
    visual.update(|window, cx| {
        window.pointer_down(p4, MouseButton::Left, cx);
        window.pointer_move(p3, Some(MouseButton::Left), cx);
        window.pointer_move(p1, Some(MouseButton::Left), cx);
        window.pointer_move(p0, Some(MouseButton::Left), cx);
    });
    assert_eq!(
        tab_state(&mut visual, tab).selected,
        0,
        "the playhead is on the stop the pointer is nearest"
    );
    assert_eq!(
        detents(&view, &mut visual) - before,
        3,
        "one detent for each stop it landed on"
    );
    // The sweep read nothing but where it rests: the stops crossed were not read one by one.
    let reads_during = reads.load(Ordering::SeqCst);
    visual.update(|window, cx| window.pointer_up(p0, MouseButton::Left, cx));
    visual.run_until_parked();
    let state = tab_state(&mut visual, tab);
    assert!(!state.entries_loading && !state.deferred);
    assert_eq!(state.entries.len(), 2);
    assert!(
        reads.load(Ordering::SeqCst) - reads_during <= 2,
        "the stop it rests on, and its neighbour read ahead: {reads_during} then {}",
        reads.load(Ordering::SeqCst)
    );
}

#[gpui_kit::test]
fn source_chips_filter_the_rail_and_the_steps_and_only_show_for_sources_present(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let base = 1_700_000_000_000;
    let (_view, mut visual, tab, _) = open(
        cx,
        Fake::new(vec![
            moment("g2", base + 25, MomentSource::GitCommit),
            moment("v1", base + 20, MomentSource::VssShadow),
            moment("g1", base + 10, MomentSource::GitCommit),
        ])
        .source(),
    );
    visual.update(|window, cx| window.render_frame(cx));
    let state = tab_state(&mut visual, tab);
    assert_eq!(
        state.sources_present(),
        [MomentSource::GitCommit, MomentSource::VssShadow]
    );
    for label in ["All", "Git", "Windows Restore"] {
        assert!(
            visual.update(|window, _| window
                .find_all_by_label(label)
                .iter()
                .any(|e| e.role() == Some(Role::Button))),
            "chip {label}"
        );
    }
    assert!(
        visual.update(|window, _| window.try_find_by_label("Journal").is_none()),
        "no journal stops, no chip"
    );

    // Windows Restore: the rail keeps one stop, and the selection moves to it.
    visual.update(|window, cx| window.click(("tm-source", 1usize), cx));
    visual.run_until_parked();
    let state = tab_state(&mut visual, tab);
    assert_eq!(state.source_filter, Some(MomentSource::VssShadow));
    assert_eq!(state.visible().len(), 1);
    assert_eq!(selected_id(&mut visual, tab), "v1");
    visual.update(|window, cx| window.render_frame(cx));
    assert!(visual.update(|window, _| window.try_find_by_label("Git commit g2, just now").is_none()));

    // Git: stepping goes between the commits only, skipping the restore point between them.
    visual.update(|window, cx| window.click(("tm-source", 0usize), cx));
    visual.run_until_parked();
    assert_eq!(selected_id(&mut visual, tab), "g2", "the nearest Git stop in time");
    visual.update(|window, cx| window.press("left", cx));
    assert_eq!(selected_id(&mut visual, tab), "g1");

    // All again; the filter is the tab's, and survives the folder changing under it.
    visual.update(|window, cx| window.click("tm-source-all", cx));
    assert_eq!(tab_state(&mut visual, tab).source_filter, None);
    visual.update(|window, cx| window.click(("tm-source", 0usize), cx));
    let model = model(&mut visual);
    visual.update(|_, cx| model.update(cx, |m, cx| m.load_folder(tab, "C:\\Work".into(), cx)));
    visual.run_until_parked();
    assert_eq!(tab_state(&mut visual, tab).source_filter, Some(MomentSource::GitCommit));
}

#[gpui_kit::test]
fn a_single_source_has_no_chips(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (_view, mut visual, _tab, _) = open(
        cx,
        Fake::new(vec![moment("g1", 1_700_000_000_000, MomentSource::GitCommit)]).source(),
    );
    visual.update(|window, cx| window.render_frame(cx));
    assert!(visual.update(|window, _| window.try_find_by_label("Windows Restore").is_none()));
    assert!(visual.update(|window, _| window.try_find_by_label("Git").is_none()));
}

#[gpui_kit::test]
fn space_stages_the_row_and_opens_quick_look_over_time_machine_and_escape_comes_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let file_path = std::env::temp_dir().join(format!("glint-tm-look-{}.txt", std::process::id()));
    std::fs::write(&file_path, "the old version\n").unwrap();
    let mut fake = Fake::new(vec![moment("g1", 1_700_000_000_000, MomentSource::GitCommit)]);
    fake.stage_to = Some(file_path.to_string_lossy().into_owned());
    let staged = fake.staged.clone();
    let (_view, mut visual, tab, events) = open(cx, fake.source());

    // No row yet: Space does nothing. Down puts the keyboard on the first row (the deleted file).
    visual.update(|window, cx| window.press("space", cx));
    visual.run_until_parked();
    assert!(staged.lock().unwrap().is_empty());
    visual.update(|window, cx| window.press("down", cx));
    assert_eq!(tab_state(&mut visual, tab).row, Some(0));
    visual.update(|window, cx| window.press("down", cx));
    assert_eq!(tab_state(&mut visual, tab).row, Some(1));
    visual.update(|window, cx| window.press("up", cx));
    assert_eq!(tab_state(&mut visual, tab).row, Some(0));

    visual.update(|window, cx| window.press("space", cx));
    visual.run_until_parked();
    assert_eq!(staged.lock().unwrap().len(), 1, "the historical version was staged");
    assert!(
        visual.update(|window, cx| window.has_active_dialog(cx)),
        "Quick Look is over Time Machine"
    );
    assert!(visual.update(|window, _| window.try_find_by_label("Quick Look: lost.txt").is_some()));
    assert!(
        visual.update(|_, cx| shows(cx, tab)),
        "Time Machine is still there under it"
    );

    visual.update(|window, cx| window.press("escape", cx));
    visual.run_until_parked();
    assert!(
        !visual.update(|window, cx| window.has_active_dialog(cx)),
        "Escape closes Quick Look only"
    );
    assert!(visual.update(|_, cx| shows(cx, tab)));
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .all(|e| !matches!(e, TimeMachineEvent::Error { .. })));
    let _ = std::fs::remove_file(&file_path);
}

#[test]
fn a_file_the_journal_only_remembers_cannot_be_looked_at() {
    let m = moment("u", 1, MomentSource::UsnActivity);
    let mut e = entry_of(&m, "gone.txt", EntryDelta::Deleted);
    e.restore = RestoreSource::UsnMoved {
        now_path: None,
        was_path: "C:\\Work\\gone.txt".into(),
    };
    assert!(!look::can_look(&e));
    e.restore = RestoreSource::UsnMoved {
        now_path: Some("C:\\Elsewhere\\gone.txt".into()),
        was_path: "C:\\Work\\gone.txt".into(),
    };
    assert!(look::can_look(&e));
    e.dir = true;
    assert!(!look::can_look(&e));
    let _ = point(px(0.), px(0.));
}
