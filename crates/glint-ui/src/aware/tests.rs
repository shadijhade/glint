//! Tests of the Aware entity: what it asks, what it keeps, what it tells the views, what it counts. Nothing
//! here touches the engine, the clipboard, the disk or the index: the sources are fakes built in each test.

use super::model::{Cache, Roots, INSIDE_FRESH, MOST_KEPT};
use super::pulse::{buckets, Pulses};
use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{Drive, NavOptions};
use crate::testing;
use glint_model::aware::{Origin, Part};
use glint_model::entries::attr;
use gpui_kit::TestAppContext;
use std::cell::{Cell, RefCell};

// ---- Fakes -------------------------------------------------------------------------------------------------

/// One call of the fake probe.
struct Call {
    paths: Vec<String>,
    wants: Vec<Want>,
    lane: Lane,
    stop: Arc<AtomicBool>,
}

/// The fake sources and what they were asked, shared with the test.
#[derive(Clone)]
struct Fake {
    calls: Rc<RefCell<Vec<Call>>>,
    senders: Rc<RefCell<Vec<async_channel::Sender<Answer>>>>,
    seq: Rc<Cell<u32>>,
    clip: Rc<RefCell<Clip>>,
    clip_reads: Rc<Cell<u32>>,
}

impl Fake {
    fn new() -> Fake {
        Fake {
            calls: Rc::default(),
            senders: Rc::default(),
            seq: Rc::default(),
            clip: Rc::new(RefCell::new(Clip::Empty)),
            clip_reads: Rc::default(),
        }
    }

    fn sources(&self) -> Sources {
        let (calls, senders) = (self.calls.clone(), self.senders.clone());
        let (seq, clip, reads) = (self.seq.clone(), self.clip.clone(), self.clip_reads.clone());
        Sources {
            probe: Rc::new(move |asks, lane, stop| {
                let wants = asks.iter().map(|a| a.want).collect();
                calls.borrow_mut().push(Call { paths: asks.into_iter().map(|a| a.path).collect(), wants, lane, stop });
                let (sender, receiver) = async_channel::unbounded();
                senders.borrow_mut().push(sender);
                receiver
            }),
            clip_seq: Rc::new(move || seq.get()),
            clip: Rc::new(move |_| {
                reads.set(reads.get() + 1);
                Task::ready(clip.borrow().clone())
            }),
            today: Rc::new(|| 20_000),
        }
    }

    fn asked(&self) -> usize {
        self.calls.borrow().len()
    }

    fn stopped(&self, call: usize) -> bool {
        self.calls.borrow()[call].stop.load(Ordering::SeqCst)
    }

    fn send(&self, call: usize, path: &str, part: Part) {
        let sender = self.senders.borrow()[call].clone();
        assert!(sender.try_send(Answer { path: path.into(), part }).is_ok());
    }
}

fn drive(path: &str, kind: DriveKind) -> Drive {
    Drive { path: path.into(), label: String::new(), kind, fs: "NTFS".into(), total: 100, free: 50 }
}

fn the_drives() -> Vec<Drive> {
    vec![drive("C:\\", DriveKind::Fixed), drive("Z:\\", DriveKind::Network)]
}

struct World {
    explorer: Entity<crate::explorer::Explorer>,
    aware: Entity<Aware>,
}

/// An explorer on a fake disk holding `disk`, opened at `open`, with the drives given, and an Aware on `fake`.
fn world(cx: &mut TestAppContext, fake: &Fake, disk: &FakeDisk, open: &str) -> World {
    cx.update(testing::init);
    disk.set_drives(the_drives());
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task(open, NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let task = explorer.update(cx, |e, cx| e.refresh_drives(cx));
    cx.foreground_executor().block_test(task);
    cx.update(|cx| init_with(cx, fake.sources()));
    let aware = cx.update(|cx| shared(cx));
    cx.run_until_parked();
    World { explorer, aware }
}

fn empty_disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder("C:\\x", vec![file("C:\\x", "a.bin", 5000, 0)]);
    disk
}

fn a_file(name: &str) -> FileEntry {
    file("C:\\x", name, 5000, 0)
}

fn look_ahead(cx: &mut TestAppContext, world: &World, entries: &[FileEntry]) {
    world.aware.update(cx, |aware, cx| aware.look_ahead(entries, cx));
}

fn look_now(cx: &mut TestAppContext, world: &World, entries: &[FileEntry]) -> bool {
    world.aware.update(cx, |aware, cx| aware.look_now(entries, cx))
}

/// Counts how often the entity says it changed.
fn count_notices(cx: &mut TestAppContext, aware: &Entity<Aware>) -> Rc<Cell<u32>> {
    let seen = Rc::new(Cell::new(0));
    let counter = seen.clone();
    cx.update(|cx| {
        cx.observe(aware, move |_, _| counter.set(counter.get() + 1)).detach();
    });
    seen
}

struct Blank;

impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

// ---- Looking ----------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_look_at_a_file_asks_once_on_the_quiet_lane_and_a_second_quiet_look_asks_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    assert_eq!(fake.asked(), 1);
    assert_eq!(fake.calls.borrow()[0].paths, vec!["C:\\x\\a.bin".to_string()]);
    assert_eq!(fake.calls.borrow()[0].lane, Lane::Quiet);
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    assert_eq!(fake.asked(), 1, "a second look at the same file asks nothing");
}

#[gpui_kit::test]
fn identical_copies_are_looked_for_only_when_one_item_is_looked_at(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    // Ctrl+A in a big folder: no item of a selection asks the index service for its twins.
    let many: Vec<FileEntry> = (0..200).map(|n| a_file(&format!("f{n}.bin"))).collect();
    look_ahead(cx, &world, &many);
    assert_eq!(fake.asked(), 1);
    {
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].wants.len(), 200, "the rest is still asked about");
        assert!(calls[0].wants.iter().all(|w| !w.twins && w.origin));
    }
    // Two items are a selection too.
    look_ahead(cx, &world, &[a_file("p.bin"), a_file("q.bin")]);
    assert!(fake.calls.borrow()[1].wants.iter().all(|w| !w.twins));
    // One of them, alone, is asked for its twins: nothing was held back from it for good.
    look_ahead(cx, &world, &[a_file("p.bin")]);
    let calls = fake.calls.borrow();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2].wants.len(), 1);
    assert!(calls[2].wants[0].twins, "a single file gets its twins");
}

#[gpui_kit::test]
fn a_look_for_now_takes_over_what_the_quiet_lane_has_not_answered_yet(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    let asked_quietly = fake.calls.borrow()[0].wants[0];
    assert!(asked_quietly.origin && asked_quietly.locked_by);
    // The origin comes in; the rest is still on its way when the right-click arrives.
    fake.send(0, "C:\\x\\a.bin", Part::Origin(Origin { host: "github.com".into(), blocked: true }));
    cx.run_until_parked();
    assert!(look_now(cx, &world, std::slice::from_ref(&entry)), "something is being asked for now, so the menu waits");
    assert_eq!(fake.asked(), 2);
    let calls = fake.calls.borrow();
    assert_eq!(calls[1].lane, Lane::Now);
    let promoted = calls[1].wants[0];
    assert!(!promoted.origin, "what came in is not asked again");
    assert_eq!(promoted, asked_quietly.minus(&Probed { origin: Some(Origin { host: String::new(), blocked: true }), ..Probed::default() }));
    assert!(calls[0].stop.load(Ordering::SeqCst), "the quiet run was only about this file: it ends, nothing is asked twice");
    assert!(!calls[1].stop.load(Ordering::SeqCst));
}

#[gpui_kit::test]
fn a_second_look_for_now_leaves_the_promoted_run_alone(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    assert!(look_now(cx, &world, std::slice::from_ref(&entry)));
    // The menu builds its rows with a look of its own a moment later: it asks nothing new and stops nothing.
    assert!(!look_now(cx, &world, std::slice::from_ref(&entry)));
    assert_eq!(fake.asked(), 2);
    assert!(!fake.stopped(1));
    assert!(world.aware.read_with(cx, |aware, _| aware.settling(&entry)));
}

#[gpui_kit::test]
fn a_quiet_run_that_is_about_other_files_too_goes_on_when_one_of_them_is_taken_over(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let (one, two) = (a_file("one.bin"), a_file("two.bin"));
    look_ahead(cx, &world, &[one.clone(), two.clone()]);
    assert!(look_now(cx, &world, std::slice::from_ref(&one)));
    assert_eq!(fake.calls.borrow()[1].paths, vec!["C:\\x\\one.bin".to_string()]);
    assert!(!fake.stopped(0), "the selection's other file is still being found out quietly");
}

#[gpui_kit::test]
fn a_quiet_look_that_has_finished_leaves_a_look_for_now_nothing_to_ask(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    // The run ends without answering everything: what can't be known stays unknown, as before.
    fake.senders.borrow_mut().clear();
    cx.run_until_parked();
    assert!(!world.aware.read_with(cx, |aware, _| aware.settling(&entry)));
    assert!(!look_now(cx, &world, std::slice::from_ref(&entry)));
    assert_eq!(fake.asked(), 1);
}

/// Counts how often the entity says answers came in, at once.
fn count_answers(cx: &mut TestAppContext, aware: &Entity<Aware>) -> Rc<Cell<u32>> {
    let seen = Rc::new(Cell::new(0));
    let counter = seen.clone();
    cx.update(|cx| {
        cx.subscribe(aware, move |_, _: &Answered, _| counter.set(counter.get() + 1)).detach();
    });
    seen
}

#[gpui_kit::test]
fn an_answer_is_kept_and_announced_at_once_and_only_the_redraw_waits_for_the_frame(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    let (heard, answers) = (count_notices(cx, &world.aware), count_answers(cx, &world.aware));
    // A fast first answer is told to the views with no wait before it.
    fake.send(0, "C:\\x\\a.bin", Part::Origin(Origin { host: "github.com".into(), blocked: true }));
    cx.run_until_parked();
    assert_eq!((heard.get(), answers.get()), (1, 1));
    // The next, a moment later, is kept at once and announced at once; the redraw is due at the frame's end.
    fake.send(0, "C:\\x\\a.bin", Part::OpensWith("Notepad".into()));
    cx.run_until_parked();
    let known = world.aware.read_with(cx, |aware, _| aware.probed(&entry).cloned()).expect("kept");
    assert_eq!(known.opens_with.as_deref(), Some("Notepad"), "kept without waiting a frame");
    assert_eq!((heard.get(), answers.get()), (1, 2));
    // Answers within the same frame add nothing to what is owed.
    fake.send(0, "C:\\x\\a.bin", Part::Pages(2));
    cx.run_until_parked();
    assert_eq!((heard.get(), answers.get()), (1, 3));
    cx.executor().advance_clock(FRAME);
    cx.run_until_parked();
    assert_eq!(heard.get(), 2, "one notice for everything that came in during the frame");
    // After a quiet while the next answer is told at once again.
    cx.executor().advance_clock(Duration::from_millis(100));
    std::thread::sleep(FRAME);
    fake.send(0, "C:\\x\\a.bin", Part::Target("C:\\t".into()));
    cx.run_until_parked();
    assert_eq!(heard.get(), 3);
}

#[gpui_kit::test]
fn what_was_asked_is_answered_when_every_part_came_in_or_the_run_ended(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    let answered = |cx: &mut TestAppContext| world.aware.read_with(cx, |aware, _| aware.answered(&entry));
    assert!(answered(cx), "nothing was asked, so nothing is awaited");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    assert!(!answered(cx));
    let asked = fake.calls.borrow()[0].wants[0];
    let all = [
        (asked.origin, Part::Origin(Origin { host: String::new(), blocked: false })),
        (asked.locked_by, Part::LockedBy(Vec::new())),
        (asked.opens_with, Part::OpensWith("Notepad".into())),
        (asked.twins, Part::Twins(Vec::new())),
    ];
    let parts: Vec<Part> = all.into_iter().filter(|(wanted, _)| *wanted).map(|(_, part)| part).collect();
    let last = parts.len() - 1;
    for (n, part) in parts.into_iter().enumerate() {
        assert!(!answered(cx), "part {n} is still to come");
        fake.send(0, "C:\\x\\a.bin", part);
        cx.run_until_parked();
        assert_eq!(answered(cx), n == last);
    }
    // A run that ends with parts unanswered has nothing more to say: they can't be known.
    let other = a_file("b.bin");
    look_ahead(cx, &world, std::slice::from_ref(&other));
    assert!(!world.aware.read_with(cx, |aware, _| aware.answered(&other)));
    fake.senders.borrow_mut().clear();
    cx.run_until_parked();
    assert!(world.aware.read_with(cx, |aware, _| aware.answered(&other)));
}

#[gpui_kit::test]
fn a_folders_inside_is_answered_only_when_it_has_been_counted(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let disk = empty_disk();
    disk.folder("C:\\x\\d", Vec::new());
    let world = world(cx, &fake, &disk, "C:\\x");
    let dir = folder("C:\\x", "d");
    look_ahead(cx, &world, std::slice::from_ref(&dir));
    let inside = |done| Part::Inside(glint_model::aware::Inside { files: 1, folders: 0, bytes: 5, junk: 0, done, from_index: false });
    fake.send(0, "C:\\x\\d", inside(false));
    cx.run_until_parked();
    assert!(!world.aware.read_with(cx, |aware, _| aware.answered(&dir)), "still counting");
    fake.send(0, "C:\\x\\d", inside(true));
    cx.run_until_parked();
    assert!(world.aware.read_with(cx, |aware, _| aware.answered(&dir)));
}

#[gpui_kit::test]
fn answers_sent_together_are_kept_and_the_views_hear_of_them_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    let heard = count_notices(cx, &world.aware);
    fake.send(0, "C:\\x\\a.bin", Part::LockedBy(vec!["Word".into()]));
    fake.send(0, "C:\\X\\A.BIN", Part::Origin(Origin { host: "github.com".into(), blocked: true }));
    fake.send(0, "C:\\x\\a.bin", Part::OpensWith("Notepad".into()));
    cx.run_until_parked();
    let known = world.aware.read_with(cx, |aware, _| aware.probed(&entry).cloned()).expect("kept");
    assert_eq!(known.locked_by, Some(vec!["Word".to_string()]));
    assert_eq!(known.origin.as_ref().map(|o| o.host.as_str()), Some("github.com"));
    assert_eq!(known.opens_with.as_deref(), Some("Notepad"));
    assert_eq!(heard.get(), 1, "three answers, one redraw");
}

#[gpui_kit::test]
fn a_file_that_changed_is_asked_about_again_and_the_old_answer_is_gone(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    fake.send(0, "C:\\x\\a.bin", Part::Pages(3));
    cx.run_until_parked();
    assert_eq!(world.aware.read_with(cx, |a, _| a.probed(&entry).and_then(|p| p.pages)), Some(3));
    let grown = file("C:\\x", "a.bin", 6000, 0);
    assert!(world.aware.read_with(cx, |a, _| a.probed(&grown).is_none()), "not the file that was asked about");
    look_ahead(cx, &world, std::slice::from_ref(&grown));
    assert_eq!(fake.asked(), 2);
    assert_eq!(world.aware.read_with(cx, |a, _| a.probed(&grown).and_then(|p| p.pages)), None);
    assert!(world.aware.read_with(cx, |a, _| a.probed(&entry).is_none()));
    let touched = file("C:\\x", "a.bin", 6000, 99);
    look_ahead(cx, &world, std::slice::from_ref(&touched));
    assert_eq!(fake.asked(), 3, "a new modified time is a new file too");
}

#[gpui_kit::test]
fn looking_ahead_again_stops_the_quiet_run_but_not_the_one_for_now_and_rest_stops_both(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    look_ahead(cx, &world, &[a_file("one.bin")]);
    look_now(cx, &world, &[a_file("two.bin")]);
    assert_eq!((fake.stopped(0), fake.stopped(1)), (false, false));
    assert_eq!(fake.calls.borrow()[1].lane, Lane::Now);
    look_ahead(cx, &world, &[a_file("three.bin")]);
    assert_eq!((fake.stopped(0), fake.stopped(1), fake.stopped(2)), (true, false, false));
    world.aware.update(cx, |aware, _| aware.rest());
    assert_eq!((fake.stopped(1), fake.stopped(2)), (true, true));
}

#[gpui_kit::test]
fn a_second_look_at_what_is_being_found_out_leaves_it_alone_and_a_look_elsewhere_ends_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    // The pointer comes back to the same row before anything was answered: its run goes on.
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    assert_eq!(fake.asked(), 1);
    assert!(!fake.stopped(0), "what is being found out is not thrown away");
    assert!(world.aware.read_with(cx, |aware, _| aware.settling(&entry)));
    // It moves on to another row: the first run is ended, and the new row is asked about.
    look_ahead(cx, &world, &[a_file("b.bin")]);
    assert_eq!(fake.asked(), 2);
    assert!(fake.stopped(0));
    assert!(!world.aware.read_with(cx, |aware, _| aware.settling(&entry)));
    // And back: what the first run never answered is asked for again.
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    assert_eq!(fake.asked(), 3, "nothing was answered, so it is asked for again");
    assert!(fake.stopped(1) && !fake.stopped(2));
}

#[gpui_kit::test]
fn entries_off_the_fixed_drives_are_never_asked_about_and_only_two_hundred_are(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    look_ahead(cx, &world, &[file("Z:\\n", "a.bin", 5000, 0), file("Q:\\n", "b.bin", 5000, 0)]);
    assert_eq!(fake.asked(), 0, "a network drive and a drive nobody knows");
    let many: Vec<FileEntry> = (0..201).map(|n| a_file(&format!("f{n}.bin"))).collect();
    look_now(cx, &world, &many);
    assert_eq!(fake.asked(), 1);
    assert_eq!(fake.calls.borrow()[0].paths.len(), 200);
}

#[gpui_kit::test]
fn an_entry_in_an_archive_is_never_asked_about(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let mut inside = a_file("inner.txt");
    inside.attrs |= attr::IN_ARCHIVE;
    look_ahead(cx, &world, &[inside]);
    assert_eq!(fake.asked(), 0);
}

#[gpui_kit::test]
fn the_entity_works_without_an_explorer_and_is_absent_before_init(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.update(|cx| {
        assert!(try_shared(cx).is_none());
        testing::init(cx);
        let fake = Fake::new();
        init_with(cx, fake.sources());
        let aware = shared(cx);
        aware.update(cx, |aware, cx| aware.look_ahead(&[a_file("a.bin")], cx));
        assert_eq!(fake.asked(), 0, "no explorer, no drives, nothing asked");
    });
    cx.run_until_parked();
}

// ---- The selection ----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_selection_is_looked_at_after_it_settles_and_a_huge_one_is_not(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    let mut entries = vec![folder("C:\\x", "docs"), a_file("one.txt"), a_file("two.txt")];
    entries.extend((0..300).map(|n| a_file(&format!("bulk{n}.bin"))));
    let bulk: Vec<String> = (0..300).map(|n| format!("C:\\x\\bulk{n}.bin")).collect();
    disk.folder("C:\\x", entries);
    let fake = Fake::new();
    let world = world(cx, &fake, &disk, "C:\\x");
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(fake.asked(), 0, "opening a folder selects nothing");

    world.explorer.update(cx, |e, cx| {
        e.set_selection(["C:\\x\\one.txt", "C:\\x\\two.txt"], Some("C:\\x\\one.txt"), Some("C:\\x\\two.txt"), cx)
    });
    cx.run_until_parked();
    assert_eq!(fake.asked(), 0, "not before the selection has settled");
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    assert_eq!(fake.asked(), 1);
    let mut asked = fake.calls.borrow()[0].paths.clone();
    asked.sort();
    assert_eq!(asked, vec!["C:\\x\\one.txt".to_string(), "C:\\x\\two.txt".to_string()]);
    assert_eq!(fake.calls.borrow()[0].lane, Lane::Quiet);

    world
        .explorer
        .update(cx, |e, cx| e.set_selection(bulk.iter(), Some("C:\\x\\bulk0.bin"), Some("C:\\x\\bulk1.bin"), cx));
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(fake.asked(), 1, "three hundred selected: nothing is asked");
}

// ---- The cache --------------------------------------------------------------------------------------------

#[test]
fn a_folders_inside_is_asked_again_only_once_its_answer_is_old() {
    let mut cache = Cache::default();
    let docs = folder("C:\\x", "docs");
    let start = Instant::now();
    let first = cache.plan(&docs, start);
    assert!(first.inside);
    cache.asking(&docs, first, start);
    assert!(cache.take("C:\\x\\docs", Part::Pages(1)));
    assert!(!cache.plan(&docs, start + Duration::from_secs(5)).inside, "asked a moment ago");
    assert!(cache.plan(&docs, start + INSIDE_FRESH).inside, "old enough to ask again");
    assert!(cache.probed(&docs).is_some(), "the old answer stays meanwhile");
}

#[test]
fn only_the_most_recently_asked_paths_are_kept_and_forgetting_is_not_case_sensitive() {
    let mut cache = Cache::default();
    let now = Instant::now();
    for n in 0..(MOST_KEPT + 100) {
        let entry = file("C:\\x", &format!("f{n}.bin"), 5000, 0);
        let want = cache.plan(&entry, now);
        cache.asking(&entry, want, now);
    }
    assert!(cache.len() <= MOST_KEPT);
    let newest = file("C:\\x", &format!("f{}.bin", MOST_KEPT + 99), 5000, 0);
    assert!(cache.probed(&newest).is_some(), "the newest survives");
    assert!(cache.probed(&file("C:\\x", "f0.bin", 5000, 0)).is_none(), "the oldest went");
    cache.forget("C:\\X\\F500.BIN");
    assert!(cache.probed(&file("C:\\x", "f500.bin", 5000, 0)).is_none());
}

#[test]
fn the_longest_drive_root_decides_whether_a_path_is_on_a_fixed_disk() {
    let roots = Roots(vec![("c:\\".into(), true), ("c:\\share\\".into(), false), ("z:\\".into(), false)]);
    assert!(roots.fixed("C:\\Users\\a.txt"));
    assert!(!roots.fixed("C:\\Share\\a.txt"));
    assert!(!roots.fixed("Z:\\a.txt"));
    assert!(!roots.fixed("Q:\\a.txt"));
}

#[gpui_kit::test]
fn forgetting_a_path_makes_it_askable_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let entry = a_file("a.bin");
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    world.aware.update(cx, |aware, _| aware.forget("C:\\X\\A.BIN"));
    assert!(world.aware.read_with(cx, |a, _| a.probed(&entry).is_none()));
    look_ahead(cx, &world, std::slice::from_ref(&entry));
    assert_eq!(fake.asked(), 2);
}

// ---- The pulse --------------------------------------------------------------------------------------------

fn secs(n: &[u64]) -> Vec<Duration> {
    n.iter().map(|s| Duration::from_secs(*s)).collect()
}

#[test]
fn buckets_are_two_seconds_wide_with_the_newest_last_and_stop_at_255() {
    assert_eq!(buckets(&[]), [0; 30]);
    let one = buckets(&secs(&[1]));
    assert_eq!((one[29], one.iter().map(|n| u32::from(*n)).sum::<u32>()), (1, 1));
    let old = buckets(&secs(&[59]));
    assert_eq!((old[0], old.iter().map(|n| u32::from(*n)).sum::<u32>()), (1, 1));
    assert_eq!(buckets(&secs(&[60, 400])), [0; 30], "a minute old is gone");
    let two_apart = buckets(&secs(&[1, 2, 3]));
    assert_eq!((two_apart[29], two_apart[28]), (1, 2), "0-1 s and 2-3 s are two buckets");
    let crowd = buckets(&vec![Duration::from_secs(1); 300]);
    assert_eq!(crowd[29], 255);
}

#[test]
fn a_folder_is_changing_with_three_beats_in_ten_seconds_and_not_with_fewer_or_older() {
    let start = Instant::now();
    let mut pulses = Pulses::default();
    pulses.beat("C:\\x\\", start);
    pulses.beat("c:\\X", start + Duration::from_secs(1));
    let now = start + Duration::from_secs(3);
    assert!(!pulses.changing("C:\\x", now), "two beats are not enough");
    pulses.beat("C:\\x", start + Duration::from_secs(2));
    assert!(pulses.changing("C:\\x", now));
    assert!(!pulses.changing("C:\\x", start + Duration::from_secs(20)), "the beats are old");
    assert!(pulses.pulse("C:\\x", start + Duration::from_secs(20)).is_some(), "but still in the minute");
    assert!(pulses.pulse("C:\\x", start + Duration::from_secs(90)).is_none());
    assert!(pulses.pulse("C:\\other", now).is_none());
}

#[test]
fn the_pulses_keep_at_most_sixteen_folders_and_the_one_beaten_longest_ago_goes() {
    let start = Instant::now();
    let mut pulses = Pulses::default();
    for n in 0..17u64 {
        pulses.beat(&format!("C:\\f{n}"), start + Duration::from_millis(n));
    }
    let now = start + Duration::from_secs(1);
    assert!(pulses.pulse("C:\\f0", now).is_none());
    assert!(pulses.pulse("C:\\f16", now).is_some());
    assert!(pulses.pulse("C:\\f1", now).is_some());
}

#[gpui_kit::test]
fn a_change_in_the_shown_folder_tells_the_views_and_one_elsewhere_does_not(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let heard = count_notices(cx, &world.aware);
    cx.update(|cx| core_bridge::emit(cx, CoreEvent::DirChanged("C:\\other".into())));
    cx.run_until_parked();
    assert_eq!(heard.get(), 0);
    assert!(world.aware.read_with(cx, |a, _| a.pulse("C:\\x").is_none()));
    for _ in 0..3 {
        cx.update(|cx| core_bridge::emit(cx, CoreEvent::DirChanged("c:\\X".into())));
    }
    cx.run_until_parked();
    assert!(heard.get() >= 1);
    let (changing, pulse) = world.aware.read_with(cx, |a, _| (a.changing("C:\\x"), a.pulse("C:\\x")));
    assert!(changing);
    assert_eq!(pulse.map(|p| p[29]), Some(3));
    let before = heard.get();
    cx.executor().advance_clock(Duration::from_secs(11));
    cx.run_until_parked();
    assert!(heard.get() > before, "told again when the changes should have stopped");
}

// ---- The clipboard ----------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_clipboard_is_read_when_its_count_moves_and_not_otherwise(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Blank));
    visual.update(|window, _| window.activate_window());
    let heard = count_notices(cx, &world.aware);
    assert_eq!(world.aware.read_with(&visual, |a, _| a.clip().clone()), Clip::Empty);

    fake.seq.set(7);
    *fake.clip.borrow_mut() = Clip::Files { count: 3, cut: true, from: Some("C:\\x".into()), names: vec!["a".into()] };
    visual.executor().advance_clock(Duration::from_millis(800));
    visual.run_until_parked();
    let now = world.aware.read_with(&visual, |a, _| a.clip().clone());
    assert!(matches!(now, Clip::Files { count: 3, cut: true, .. }), "{now:?}");
    assert_eq!((heard.get(), fake.clip_reads.get()), (1, 1));

    visual.executor().advance_clock(Duration::from_millis(800));
    visual.run_until_parked();
    assert_eq!(fake.clip_reads.get(), 1, "the same count asks nothing");

    fake.seq.set(8);
    visual.executor().advance_clock(Duration::from_millis(800));
    visual.run_until_parked();
    assert_eq!(fake.clip_reads.get(), 2);
    assert_eq!(heard.get(), 1, "the same clipboard again tells nobody");
}

#[test]
fn files_on_the_clipboard_say_how_many_the_first_names_and_the_folder_they_share() {
    let paths: Vec<String> = (0..7).map(|n| format!("C:\\docs\\f{n}.txt")).collect();
    match super::clip::files_clip(&paths, false) {
        Some(Clip::Files { count, cut, from, names }) => {
            assert_eq!((count, cut), (7, false));
            assert_eq!(from.as_deref(), Some("C:\\docs"));
            assert_eq!(names.len(), 5);
            assert_eq!(names[0], "f0.txt");
        }
        other => panic!("{other:?}"),
    }
    let mixed = vec!["C:\\a\\x.txt".to_string(), "C:\\b\\y.txt".to_string()];
    assert!(matches!(super::clip::files_clip(&mixed, true), Some(Clip::Files { from: None, cut: true, .. })));
    assert!(super::clip::files_clip(&[], false).is_none());
    let arabic = vec!["C:\\وثائق\\تقرير الربع 1.pdf".to_string()];
    assert!(matches!(
        super::clip::files_clip(&arabic, false),
        Some(Clip::Files { names, .. }) if names == vec!["تقرير الربع 1.pdf".to_string()]
    ));
}

// ---- Habits and suggesting --------------------------------------------------------------------------------

fn saved_state(cx: &mut TestAppContext, tag: &str) {
    let dir = std::env::temp_dir().join(format!("glint-aware-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("state.json");
    let _ = std::fs::remove_file(&file);
    cx.update(|cx| {
        testing::init(cx);
        persistence::init_at(cx, file);
    });
}

fn pictures() -> Facts {
    Facts { shape: Shape::Pictures, count: 2, ..Facts::default() }
}

#[gpui_kit::test]
fn a_habit_is_used_only_while_learning_and_suggesting_are_on(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    saved_state(cx, "suggest");
    cx.update(|cx| {
        // No entity here, so today is the clock's.
        let day = today(cx);
        persistence::update_app(cx, |app| {
            for _ in 0..10 {
                app.habits.note_action(Shape::Pictures, "do:a-row-with-no-rule", day);
            }
        });
        let keys = ["do:a-row-with-no-rule"];
        let picks = suggest(cx, &pictures(), None, &keys, 3);
        assert_eq!(picks.len(), 1, "ten uses make it worth putting forward");
        assert_eq!(picks[0].key, "do:a-row-with-no-rule");

        state::update(cx, |s| s.learn = false);
        assert!(suggest(cx, &pictures(), None, &keys, 3).is_empty(), "a habit adds nothing with learning off");

        state::update(cx, |s| {
            s.learn = true;
            s.suggest = false;
        });
        assert!(suggest(cx, &pictures(), None, &keys, 3).is_empty(), "nothing is suggested with suggesting off");
    });
}

#[gpui_kit::test]
fn counting_is_off_with_learning_off_and_never_counts_a_basic_row(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    saved_state(cx, "count");
    cx.update(|cx| {
        let uses = |cx: &App, key: &str| {
            persistence::app(cx).map_or(0, |app| app.habits.action_uses(Shape::Picture, key))
        };
        note_action(cx, Shape::Picture, "do:x-ray");
        note_action(cx, Shape::Picture, "cmd:copy");
        note_action(cx, Shape::Picture, "task:toggle-pin");
        note_action(cx, Shape::Picture, "");
        assert_eq!(uses(cx, "do:x-ray"), 1);
        assert_eq!(uses(cx, "cmd:copy") + uses(cx, "task:toggle-pin") + uses(cx, ""), 0, "basic rows are not counted");

        state::update(cx, |s| s.learn = false);
        note_action(cx, Shape::Picture, "do:x-ray");
        note_place(cx, &["png".into()], "C:\\Pictures");
        assert_eq!(uses(cx, "do:x-ray"), 1, "nothing is counted with learning off");
        assert!(persistence::app(cx).is_some_and(|app| app.habits.places.is_empty()));
        assert!(places_for(cx, "png").is_empty());

        state::update(cx, |s| s.learn = true);
        note_place(cx, &["png".into(), "jpg".into()], "C:\\Pictures");
        note_place(cx, &[], "C:\\Somewhere");
        assert_eq!(places_for(cx, "png").first().map(|p| p.folder.as_str()), Some("C:\\Pictures"));
        assert_eq!(places_for(cx, "").first().map(|p| p.folder.as_str()), Some("C:\\Somewhere"));
        state::update(cx, |s| s.suggest = false);
        assert!(places_for(cx, "png").is_empty(), "no usual place with suggesting off");

        forget_habits(cx);
        assert!(persistence::app(cx).is_some_and(|app| app.habits.is_empty()));
    });
}

// ---- Facts ------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_facts_of_two_pictures_and_of_something_in_an_archive(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.update(testing::init);
    cx.update(|cx| {
        let two = [file("C:\\p", "A.png", 100, 0), file("C:\\p", "b.PNG", 250, 0)];
        let facts = facts(cx, &two);
        assert_eq!(facts.shape, Shape::Pictures);
        assert_eq!((facts.count, facts.files, facts.folders, facts.bytes), (2, 2, 0, 350));
        assert_eq!(facts.ext.as_deref(), Some("png"));
        assert_eq!(facts.first_name, "a.png");
        assert!(!facts.fixed);
        assert_eq!(facts.clip, Clip::Empty, "no entity, nothing on the clipboard");

        let mut inside = file("C:\\p\\pack.zip", "inner.txt", 1, 0);
        inside.attrs |= attr::IN_ARCHIVE;
        let mixed = [folder("C:\\p", "docs"), inside];
        let facts = super::facts(cx, &mixed);
        assert!(facts.fixed);
        assert_eq!((facts.files, facts.folders, facts.bytes), (1, 1, 1));
        assert_eq!(facts.ext, None);

        let none = super::facts(cx, &[]);
        assert_eq!((none.shape, none.count, none.first_name.as_str()), (Shape::Nothing, 0, ""));
    });
}

#[gpui_kit::test]
fn the_facts_carry_what_is_on_the_clipboard_from_the_entity(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let world = world(cx, &fake, &empty_disk(), "C:\\x");
    world.aware.update(cx, |aware, _| aware.clip = Clip::Text);
    let facts = cx.update(|cx| facts(cx, &[a_file("a.bin")]));
    assert_eq!(facts.clip, Clip::Text);
}

// ---- The pointer resting on a row or a tile ----------------------------------------------------------------

#[gpui_kit::test]
fn resting_on_an_item_asks_about_it_after_a_moment_and_leaving_sooner_asks_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::new();
    let _world = world(cx, &fake, &empty_disk(), "C:\\x");
    // The pointer comes to a row and stays: the view keeps the task.
    let staying = cx.update(|cx| rest_on(a_file("a.bin"), cx));
    assert!(staying.is_some());
    cx.executor().advance_clock(RESTS_FOR - Duration::from_millis(20));
    cx.run_until_parked();
    assert_eq!(fake.asked(), 0, "not before it has rested");
    cx.executor().advance_clock(Duration::from_millis(40));
    cx.run_until_parked();
    assert_eq!(fake.asked(), 1);
    assert_eq!(fake.calls.borrow()[0].lane, Lane::Quiet);
    assert_eq!(fake.calls.borrow()[0].paths, vec!["C:\\x\\a.bin".to_string()]);

    // It crosses another row without staying: the view drops the task, and nothing is asked.
    let crossing = cx.update(|cx| rest_on(a_file("b.bin"), cx));
    drop(crossing);
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(fake.asked(), 1);
}

#[gpui_kit::test]
fn resting_asks_nothing_when_the_app_has_no_entity(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.update(testing::init);
    assert!(cx.update(|cx| rest_on(a_file("a.bin"), cx)).is_none());
}
