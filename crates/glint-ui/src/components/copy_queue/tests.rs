//! Tests of the copy queue: the words of a job (plain functions), and the panel in a headless window with a
//! fake queue service, so nothing touches this PC's real queue.

use super::*;
use crate::platform::window_state::ProgressState;
use crate::testing;
use glint_core::queue::{Failure, Made};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

fn job(id: u64, state: &str) -> JobView {
    JobView {
        id,
        kind: "copy".into(),
        sources: vec!["C:\\Photos\\trip.jpg".into(), "C:\\Photos\\more".into()],
        dest: "D:\\Backup".into(),
        state: state.into(),
        files: 10,
        files_done: 4,
        bytes: 1000,
        bytes_done: 400,
        speed: 100.0,
        current: Some("C:\\Photos\\more\\a.jpg".into()),
        started: Some(1_000),
        ..Default::default()
    }
}

// ---- Words -------------------------------------------------------------------------------------------

#[test]
fn time_left_and_time_taken_round_as_the_typescript_did() {
    let _guard = testing::lock();
    let mut j = job(1, "copying");
    // 600 bytes at 100 B/s: 6 s. Under a second still says 1 s.
    assert_eq!(left(&j), "6 s left");
    j.bytes_done = 999;
    assert_eq!(left(&j), "1 s left");
    // 59.5 s rounds to 60 s: the TypeScript's `< 60` test is on the unrounded number.
    j.bytes = 100_000;
    j.bytes_done = 100_000 - 5_950;
    assert_eq!(left(&j), "60 s left");
    j.bytes_done = 0;
    assert_eq!(left(&j), "17 min left");
    j.bytes = 100 * (3600 + 90 * 60);
    assert_eq!(left(&j), "2 h 30 min left");
    // No speed, or nothing left: nothing to say.
    j.speed = 0.0;
    assert_eq!(left(&j), "");
    let done = JobView {
        bytes_done: 1000,
        ..job(1, "copying")
    };
    assert_eq!(left(&done), "");

    let mut j = job(1, "done");
    j.finished = Some(1_000 + 400);
    assert_eq!(took(&j), "1 s");
    j.finished = Some(1_000 + 12_000);
    assert_eq!(took(&j), "12 s");
    j.finished = Some(1_000 + 3 * 60_000);
    assert_eq!(took(&j), "3 min");
    j.started = None;
    assert_eq!(took(&j), "");
}

#[test]
fn titles_and_status_lines_read_as_the_typescript_wrote_them() {
    let _guard = testing::lock();
    let mut j = job(1, "copying");
    assert_eq!(job_title(&j), "Copying 2 items to “Backup”");
    assert_eq!(job_from(&j), "C:\\Photos");
    j.sources = vec!["C:\\Photos\\trip.jpg".into()];
    j.kind = "move".into();
    assert_eq!(job_title(&j), "Moving “trip.jpg” to “Backup”");
    j.dest = "E:\\".into();
    assert_eq!(job_title(&j), "Moving “trip.jpg” to “E:”");

    let j = job(1, "copying");
    assert_eq!(
        status_line(&j),
        "Copying · 4 of 10 files · 400 B of 1000 B · 100 B/s · 6 s left"
    );
    // Paused or waiting: no speed part.
    assert_eq!(
        status_line(&job(1, "paused")),
        "Paused · 4 of 10 files · 400 B of 1000 B"
    );
    assert_eq!(
        status_line(&JobView {
            files: 0,
            bytes: 0,
            ..job(1, "waiting")
        }),
        "Waiting its turn"
    );

    let mut over = job(1, "done");
    over.files_done = 10;
    over.bytes_done = 1000;
    over.finished = Some(1_000 + 12_000);
    assert_eq!(status_line(&over), "Done: 10 files (1000 B) in 12 s");
    over.verify = true;
    over.verified = 10;
    assert_eq!(status_line(&over), "Done: 10 files (1000 B) in 12 s · each one checked");
    // Two failed and eight checked is also "each one checked"; anything else counts.
    over.errors = vec![
        Failure {
            path: "x".into(),
            error: "y".into()
        };
        2
    ];
    over.verified = 8;
    assert!(status_line(&over).ends_with("each one checked"));
    over.verified = 5;
    assert!(status_line(&over).ends_with("5 checked"));
    // One file done, two errors: signed arithmetic, so 0 verified isn't "all of them".
    over.files_done = 1;
    over.verified = 0;
    assert!(status_line(&over).ends_with("0 checked"));
}

#[test]
fn state_words_are_the_typescripts() {
    let _guard = testing::lock();
    for (state, text) in [
        ("waiting", "Waiting its turn"),
        ("planning", "Looking at what's to copy…"),
        ("copying", "Copying"),
        ("checking", "Checking the copy"),
        ("paused", "Paused"),
        ("asking", "Asks you"),
        ("done", "Done"),
        ("cancelled", "Cancelled"),
        ("failed", "Failed"),
    ] {
        assert_eq!(state_text(state), text);
    }
}

#[test]
fn the_taskbar_follows_the_running_jobs() {
    let paused = job(2, "paused");
    assert_eq!(taskbar(&[]).0, ProgressState::None);
    assert_eq!(taskbar(&[job(1, "done")]).0, ProgressState::None);
    assert_eq!(
        taskbar(&[job(1, "copying"), paused.clone()]),
        (ProgressState::Normal, 800, 2000)
    );
    // Finished jobs' bytes don't count; all-paused is yellow.
    assert_eq!(
        taskbar(&[job(1, "done"), paused.clone()]),
        (ProgressState::Paused, 400, 1000)
    );
    // Nothing known of the size yet: a marquee.
    let planning = JobView {
        bytes: 0,
        bytes_done: 0,
        ..job(3, "planning")
    };
    assert_eq!(taskbar(&[planning]).0, ProgressState::Indeterminate);
    // Half a percent is the smallest visible step.
    let key = |done| job::taskbar_key(ProgressState::Normal, done, 1000);
    assert_eq!(key(400), key(401));
    assert_ne!(key(400), key(403));
}

#[test]
fn core_events_become_jobs_and_other_payloads_do_not() {
    let mut made = job(7, "done");
    made.errors = vec![Failure {
        path: "C:\\a\\b.txt".into(),
        error: "Access is denied".into(),
    }];
    made.made = vec![Made {
        from: "C:\\a\\b.txt".into(),
        to: "D:\\b.txt".into(),
    }];
    made.conflict = Some("D:\\x.txt".into());
    made.finished = Some(5_000);
    let payload = serde_json::to_value(vec![job(1, "copying"), made.clone()]).expect("JobView serializes");
    let jobs = jobs_from_json(&payload).expect("an array");
    assert_eq!(jobs.len(), 2);
    assert_eq!(jobs[0].sources, job(1, "copying").sources);
    assert_eq!(jobs[0].speed, 100.0);
    let back = &jobs[1];
    assert_eq!(back.id, 7);
    assert_eq!(back.errors, made.errors);
    assert_eq!(back.made, made.made);
    assert_eq!(back.conflict.as_deref(), Some("D:\\x.txt"));
    assert_eq!((back.started, back.finished), (Some(1_000), Some(5_000)));
    // The empty object core_bridge's own test sends isn't a list; a job without an id is dropped.
    assert!(jobs_from_json(&serde_json::json!({})).is_none());
    assert_eq!(
        jobs_from_json(&serde_json::json!([{ "kind": "copy" }])).map(|j| j.len()),
        Some(0)
    );
}

// ---- The panel ---------------------------------------------------------------------------------------

/// A queue service that records what it is told.
#[derive(Default)]
struct Fake {
    sent: Arc<Mutex<Vec<Command>>>,
    refuse: Option<String>,
    jobs: Vec<JobView>,
}

impl Service for Fake {
    fn jobs(&self) -> Answer<Vec<JobView>> {
        let jobs = self.jobs.clone();
        Box::pin(async move { jobs })
    }

    fn send(&self, command: Command) -> Answer<Result<(), String>> {
        self.sent.lock().unwrap_or_else(|e| e.into_inner()).push(command);
        let answer = self.refuse.clone().map_or(Ok(()), Err);
        Box::pin(async move { answer })
    }
}

fn sent(log: &Arc<Mutex<Vec<Command>>>) -> Vec<Command> {
    log.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn open(cx: &mut TestAppContext, fake: Fake) -> (VisualTestContext, Entity<CopyQueue>, Arc<Mutex<Vec<Command>>>) {
    cx.update(testing::init);
    let log = fake.sent.clone();
    let service: Arc<dyn Service> = Arc::new(fake);
    let (visual, queue) =
        testing::open_window(cx, move |_, cx| cx.new(|cx| CopyQueue::with_service(service, None, cx)));
    (visual, queue, log)
}

fn tell(cx: &mut VisualTestContext, jobs: &[JobView]) {
    let payload = serde_json::to_value(jobs).expect("JobView serializes");
    cx.update(|_, cx| core_bridge::emit(cx, "copy-queue", payload));
}

fn has(cx: &mut VisualTestContext, label: &str) -> bool {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).is_some()
    })
}

fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).and_then(|element| element.role())
    })
}

/// Lets core's event (and work on core's runtime) come back to the foreground until `done`.
fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
    for _ in 0..400 {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the copy queue never got there");
}

fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click(id, cx);
    });
}

const TITLE: &str = "Copying 2 items to “Backup”";

#[gpui_kit::test]
fn nothing_is_drawn_without_jobs_and_a_hidden_queue_is_a_pill(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, _) = open(cx, Fake::default());
    visual.run_until_parked();
    assert!(!has(&mut visual, "Copy queue"));

    tell(&mut visual, &[job(1, "copying")]);
    settle(&mut visual, |cx| has(cx, "Copy queue · 40%"));
    assert_eq!(role_of(&mut visual, "Copy queue · 40%"), Some(Role::Button));
    assert!(!queue.read_with(&visual, |q, _| q.is_open()));

    // A job asking changes the pill's words; when only finished jobs remain there is no pill.
    tell(
        &mut visual,
        &[JobView {
            conflict: Some("D:\\a.txt".into()),
            ..job(1, "asking")
        }],
    );
    settle(&mut visual, |cx| has(cx, "The copy queue asks you"));
    tell(&mut visual, &[job(1, "done")]);
    settle(&mut visual, |cx| !has(cx, "The copy queue asks you"));
    assert!(!has(&mut visual, "Copy queue"));
}

#[gpui_kit::test]
fn the_pill_opens_the_panel_and_hide_closes_it_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, _) = open(cx, Fake::default());
    tell(&mut visual, &[job(1, "copying")]);
    settle(&mut visual, |cx| has(cx, "Copy queue · 40%"));
    click(&mut visual, "copy-queue-pill");
    visual.run_until_parked();
    assert!(queue.read_with(&visual, |q, _| q.is_open()));
    assert!(has(&mut visual, "Copy queue"));
    assert!(has(&mut visual, "Hide the copy queue"));
    click(&mut visual, "copy-queue-hide");
    visual.run_until_parked();
    assert!(!queue.read_with(&visual, |q, _| q.is_open()));
    assert!(has(&mut visual, "Copy queue · 40%"));
}

#[gpui_kit::test]
fn a_running_job_shows_its_progress_and_pause_resume_cancel_reach_core(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, log) = open(cx, Fake::default());
    visual.update(|_, cx| queue.update(cx, |q, cx| q.show(cx)));
    tell(&mut visual, &[job(1, "copying")]);
    let status = "Copying · 4 of 10 files · 400 B of 1000 B · 100 B/s · 6 s left";
    settle(&mut visual, |cx| has(cx, &format!("{TITLE}. {status}")));
    assert_eq!(
        role_of(&mut visual, &format!("{TITLE}. {status}")),
        Some(Role::ListItem)
    );
    assert_eq!(
        role_of(&mut visual, &format!("{TITLE}: 40%")),
        Some(Role::ProgressIndicator)
    );
    assert!(has(&mut visual, "Pause") && has(&mut visual, "Cancel"));
    assert!(!has(&mut visual, "Clear finished"));

    click(&mut visual, ("copy-queue-pause", 1_usize));
    click(&mut visual, ("copy-queue-cancel", 1_usize));
    settle(&mut visual, |_| sent(&log).len() == 2);
    assert_eq!(sent(&log), vec![Command::Pause(1), Command::Cancel(1)]);

    // Paused: Resume takes Pause's place.
    tell(&mut visual, &[job(1, "paused")]);
    settle(&mut visual, |cx| has(cx, "Resume"));
    assert!(!has(&mut visual, "Pause"));
    click(&mut visual, ("copy-queue-resume", 1_usize));
    settle(&mut visual, |_| sent(&log).len() == 3);
    assert_eq!(sent(&log)[2], Command::Resume(1));
}

#[gpui_kit::test]
fn a_taken_name_asks_and_the_answer_goes_to_core_with_for_the_rest(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, log) = open(cx, Fake::default());
    visual.update(|_, cx| queue.update(cx, |q, cx| q.show(cx)));
    let asking = JobView {
        conflict: Some("D:\\Backup\\notes.txt".into()),
        ..job(4, "asking")
    };
    tell(&mut visual, &[asking]);
    settle(&mut visual, |cx| has(cx, "“notes.txt” is there already."));
    assert_eq!(role_of(&mut visual, "“notes.txt” is there already."), Some(Role::Group));
    for button in ["Replace it", "Skip it", "Keep both"] {
        assert_eq!(role_of(&mut visual, button), Some(Role::Button), "{button}");
    }

    click(&mut visual, ("copy-queue-skip", 4_usize));
    click(&mut visual, ("copy-queue-all", 4_usize));
    visual.run_until_parked();
    click(&mut visual, ("copy-queue-replace", 4_usize));
    settle(&mut visual, |_| sent(&log).len() == 2);
    assert_eq!(
        sent(&log),
        vec![
            Command::Answer {
                id: 4,
                answer: "skip",
                all: false
            },
            Command::Answer {
                id: 4,
                answer: "replace",
                all: true
            },
        ]
    );
    // Pausing a job that asks isn't offered.
    assert!(!has(&mut visual, "Resume"));
}

#[gpui_kit::test]
fn a_finished_job_shows_how_it_went_and_clear_finished_asks_core(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, log) = open(cx, Fake::default());
    visual.update(|_, cx| queue.update(cx, |q, cx| q.show(cx)));
    let mut over = job(2, "done");
    over.files_done = 10;
    over.bytes_done = 1000;
    over.finished = Some(13_000);
    over.errors = vec![
        Failure {
            path: "C:\\Photos\\a.jpg".into(),
            error: "Access is denied".into(),
        },
        Failure {
            path: "C:\\Photos\\b.jpg".into(),
            error: "In use".into(),
        },
    ];
    tell(&mut visual, &[over]);
    settle(&mut visual, |cx| has(cx, "Clear finished"));
    assert!(has(
        &mut visual,
        "Copied 2 items to “Backup”. Done: 10 files (1000 B) in 12 s"
    ));
    // No bar once it has ended; the errors are folded away until asked for.
    assert!(!has(&mut visual, "Copied 2 items to “Backup”: 100%"));
    assert!(has(&mut visual, "2 didn't go"));
    assert!(!has(&mut visual, "a.jpg: Access is denied"));
    click(&mut visual, ("copy-queue-errors", 2_usize));
    visual.run_until_parked();
    assert_eq!(role_of(&mut visual, "a.jpg: Access is denied"), Some(Role::ListItem));
    assert!(has(&mut visual, "b.jpg: In use"));
    click(&mut visual, ("copy-queue-errors", 2_usize));
    visual.run_until_parked();
    assert!(!has(&mut visual, "a.jpg: Access is denied"));

    click(&mut visual, "copy-queue-clear");
    settle(&mut visual, |_| !sent(&log).is_empty());
    assert_eq!(sent(&log), vec![Command::Clear]);
}

#[gpui_kit::test]
fn a_job_that_ends_is_announced_once_with_what_it_made(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, _) = open(cx, Fake::default());
    let heard = Rc::new(RefCell::new(Vec::<JobView>::new()));
    let log = heard.clone();
    let _sub = visual.update(|_, cx| {
        cx.subscribe(&queue, move |_, event: &CopyQueueEvent, _| {
            if let CopyQueueEvent::Finished(job) = event {
                log.borrow_mut().push((**job).clone());
            }
        })
    });
    // A job that first shows up already finished was never seen running: no word of it.
    tell(&mut visual, &[job(9, "done")]);
    settle(&mut visual, |cx| queue.read_with(cx, |q, _| q.jobs().len() == 1));
    assert!(heard.borrow().is_empty());

    tell(&mut visual, &[job(1, "copying"), job(9, "done")]);
    settle(&mut visual, |cx| queue.read_with(cx, |q, _| q.jobs().len() == 2));
    let mut over = job(1, "done");
    over.made = vec![Made {
        from: "C:\\Photos\\trip.jpg".into(),
        to: "D:\\Backup\\trip.jpg".into(),
    }];
    tell(&mut visual, &[over.clone(), job(9, "done")]);
    settle(&mut visual, |_| !heard.borrow().is_empty());
    // Telling it again changes nothing.
    tell(&mut visual, &[over, job(9, "done")]);
    visual.run_until_parked();
    std::thread::sleep(Duration::from_millis(20));
    visual.run_until_parked();
    assert_eq!(heard.borrow().len(), 1);
    assert_eq!(heard.borrow()[0].id, 1);
    assert_eq!(heard.borrow()[0].made[0].to, "D:\\Backup\\trip.jpg");
}

#[gpui_kit::test]
fn queueing_opens_the_panel_and_a_refusal_comes_back_as_an_event(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, log) = open(cx, Fake::default());
    visual.update(|_, cx| {
        queue.update(cx, |q, cx| {
            q.queue_files(JobKind::Move, vec!["C:\\a".into()], "D:\\".into(), true, cx)
        })
    });
    settle(&mut visual, |cx| queue.read_with(cx, |q, _| q.is_open()));
    assert_eq!(
        sent(&log),
        vec![Command::Add {
            kind: JobKind::Move,
            sources: vec!["C:\\a".into()],
            dest: "D:\\".into(),
            verify: true
        }]
    );

    let (mut visual, queue, _) = open(
        cx,
        Fake {
            refuse: Some("“C:\\a” can't go into itself".into()),
            ..Default::default()
        },
    );
    let refused = Rc::new(RefCell::new(Vec::<String>::new()));
    let log = refused.clone();
    let _sub = visual.update(|_, cx| {
        cx.subscribe(&queue, move |_, event: &CopyQueueEvent, _| {
            if let CopyQueueEvent::NotQueued { reason, .. } = event {
                log.borrow_mut().push(reason.clone());
            }
        })
    });
    visual.update(|_, cx| {
        queue.update(cx, |q, cx| {
            q.queue_files(JobKind::Copy, vec!["C:\\a".into()], "C:\\a\\b".into(), false, cx)
        })
    });
    settle(&mut visual, |_| !refused.borrow().is_empty());
    assert!(!queue.read_with(&visual, |q, _| q.is_open()));
    assert_eq!(refused.borrow()[0], "“C:\\a” can't go into itself");
}

#[gpui_kit::test]
fn jobs_already_running_when_the_window_opens_are_read(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, _) = open(
        cx,
        Fake {
            jobs: vec![job(3, "copying")],
            ..Default::default()
        },
    );
    settle(&mut visual, |cx| queue.read_with(cx, |q, _| q.jobs().len() == 1));
    assert!(has(&mut visual, "Copy queue · 40%"));
}

#[gpui_kit::test]
fn the_arabic_ui_mirrors_and_an_arabic_file_name_is_drawn_and_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, queue, _) = open(cx, Fake::default());
    testing::set_rtl(true);
    visual.update(|_, cx| queue.update(cx, |q, cx| q.show(cx)));
    let asking = JobView {
        sources: vec!["C:\\صور\\تقرير الربع.pdf".into()],
        dest: "D:\\نسخة احتياطية".into(),
        conflict: Some("D:\\نسخة احتياطية\\تقرير الربع.pdf".into()),
        errors: vec![Failure {
            path: "C:\\صور\\ملف.txt".into(),
            error: "مرفوض".into(),
        }],
        ..job(5, "asking")
    };
    tell(&mut visual, &[asking]);
    // Names are exposed as they are, without the layout-only isolates.
    settle(&mut visual, |cx| has(cx, "“تقرير الربع.pdf” is there already."));
    assert_eq!(
        role_of(&mut visual, "“تقرير الربع.pdf” is there already."),
        Some(Role::Group)
    );
    assert!(has(&mut visual, "Copy queue"));
    click(&mut visual, ("copy-queue-errors", 5_usize));
    visual.run_until_parked();
    assert!(has(&mut visual, "ملف.txt: مرفوض"));
    testing::set_rtl(false);
}
