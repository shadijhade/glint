//! Following XYplorer (docs/gpui/marks, F30): when Glint starts and each time the main window comes to the front,
//! core is asked whether the `tag.dat` that was brought in changed since. What is new comes in on its own: the rows
//! that show marks read again ([`TagsChanged`]) and one quiet line says how many. Only while
//! `settings.follow_xyplorer` is on; turned on later, it starts then. Nothing here reads a file on the UI thread: the
//! source is core's `marks::follow`, run on core's threads.
//!
//! The window's coming to the front is heard without the workspace's code: [`install`] watches for the workspace
//! being made (`observe_new`) and listens to its window's activation from there.

use crate::dialogs::tags::{self, TagsChanged};
use crate::kit::prelude::*;
use crate::{core_bridge, state, t};
use glint_core::marks::apply::apply_labels;
use glint_core::marks::Followed;
use glint_model::format::plural;
use glint_model::marks::Label;
use gpui_kit::gpui::{AnyWindowHandle, App, Context, Global};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Not asked again within this long of the last answer (coming to the front twice is not two questions).
pub const QUIET: Duration = Duration::from_secs(5);

/// What answers "did the followed `tag.dat` change?", given Settings' labels as they are now: in the app core's
/// `marks::apply::follow_with`, in a test a fake.
pub type Source = Rc<dyn Fn(Vec<Label>) -> Pin<Box<dyn Future<Output = Result<Option<Followed>, String>>>>>;

struct Follow {
    source: Source,
    /// The clock behind the five seconds (a test moves it).
    clock: Rc<dyn Fn() -> Instant>,
    /// A question is out: never two at once.
    asking: bool,
    last_answer: Option<Instant>,
    /// The setting was on when last looked at, to tell "turned on now".
    was_on: bool,
    /// The window that says the line.
    window: Option<AnyWindowHandle>,
}

impl Global for Follow {}

/// Starts following, for the main window's process. Does nothing while the setting is off.
pub fn init(cx: &mut App) {
    if crate::platform::secondary::is_secondary() || !cx.has_global::<core_bridge::Core>() {
        return;
    }
    init_with(
        cx,
        Rc::new(|have| {
            Box::pin(async { core_bridge::run(glint_core::marks::apply::follow_with(have)).await.and_then(|answer| answer) })
        }),
    );
}

/// [`init`] with another source (tests).
pub fn init_with(cx: &mut App, source: Source) {
    install::<crate::workspace::Workspace>(cx, source, Rc::new(Instant::now));
}

/// Watches views of type `V` being made in a window (the workspace, in the app): that is "Glint starts", and its
/// window coming to the front is what asks again.
fn install<V: 'static>(cx: &mut App, source: Source, clock: Rc<dyn Fn() -> Instant>) {
    let was_on = state::settings(cx).follow_xyplorer;
    cx.set_global(Follow {
        source,
        clock,
        asking: false,
        last_answer: None,
        was_on,
        window: None,
    });
    cx.observe_new(|_: &mut V, window, cx: &mut Context<V>| {
        let Some(window) = window else { return };
        cx.global_mut::<Follow>().window = Some(window.window_handle());
        cx.observe_window_activation(window, |_, window, cx| {
            if window.is_window_active() {
                ask(cx);
            }
        })
        .detach();
        ask(cx);
    })
    .detach();
    // Turned on later: it starts then.
    cx.observe_global::<state::UiState>(|cx| {
        let on = state::settings(cx).follow_xyplorer;
        let turned_on = on && !cx.global::<Follow>().was_on;
        cx.global_mut::<Follow>().was_on = on;
        if turned_on {
            ask(cx);
        }
    })
    .detach();
}

/// Asks the source, unless following is off, a question is out, or the last answer is under [`QUIET`] old.
fn ask(cx: &mut App) {
    if !state::settings(cx).follow_xyplorer {
        return;
    }
    let follow = cx.global_mut::<Follow>();
    let now = (follow.clock)();
    if follow.asking || follow.last_answer.is_some_and(|last| now.saturating_duration_since(last) < QUIET) {
        return;
    }
    follow.asking = true;
    let source = follow.source.clone();
    let asked = source(state::settings(cx).labels.clone());
    cx.spawn(async move |cx| {
        let answer = asked.await;
        cx.update(|cx| answered(cx, answer));
    })
    .detach();
}

/// The one quiet line: "3 marks came from XYplorer".
fn line(marks: u64) -> SharedString {
    t!(
        "{n} came from XYplorer",
        n = plural(usize::try_from(marks).unwrap_or(usize::MAX), "mark")
    )
}

/// What came: the rows read again and one quiet line. An error says nothing (it is tried again next time).
fn answered(cx: &mut App, answer: Result<Option<Followed>, String>) {
    let follow = cx.global_mut::<Follow>();
    follow.asking = false;
    follow.last_answer = Some((follow.clock)());
    let window = follow.window;
    let Ok(Some(followed)) = answer else { return };
    let marks = followed.came.saturating_add(followed.gone);
    if marks == 0 {
        return;
    }
    // A label XYplorer gained since comes into Settings as an import brings it (the marks already carry its name).
    let mut labels = state::settings(cx).labels.clone();
    apply_labels(&mut labels, &followed.labels);
    if labels != state::settings(cx).labels {
        state::update(cx, |settings| settings.labels = labels);
    }
    tags::hub(cx).update(cx, |_, cx| cx.emit(TagsChanged { paths: followed.paths }));
    if let Some(window) = window {
        window
            .update(cx, |_, window, cx| window.push_notification(Notification::info(line(marks)), cx))
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::{Cell, RefCell};

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    type Answer = Result<Option<Followed>, String>;

    /// A fake source: counts its questions and answers from a list (then with nothing); `hang` never answers.
    struct Fake {
        asked: Rc<Cell<usize>>,
        answers: Rc<RefCell<Vec<Answer>>>,
        now: Rc<Cell<Instant>>,
        /// The labels each question was handed.
        handed: Rc<RefCell<Vec<Vec<Label>>>>,
    }

    fn start(cx: &mut TestAppContext, on: bool, hang: bool, answers: Vec<Answer>) -> (VisualTestContext, Fake) {
        cx.update(|cx| {
            testing::init(cx);
            state::update(cx, |s| s.follow_xyplorer = on);
        });
        let fake = Fake {
            asked: Rc::default(),
            answers: Rc::new(RefCell::new(answers)),
            now: Rc::new(Cell::new(Instant::now())),
            handed: Rc::default(),
        };
        let (asked, queue, handed) = (fake.asked.clone(), fake.answers.clone(), fake.handed.clone());
        let source: Source = Rc::new(move |have| {
            asked.set(asked.get() + 1);
            handed.borrow_mut().push(have);
            let next = if queue.borrow().is_empty() { Ok(None) } else { queue.borrow_mut().remove(0) };
            if hang {
                Box::pin(std::future::pending())
            } else {
                Box::pin(async move { next })
            }
        });
        let clock = fake.now.clone();
        cx.update(|cx| install::<Host>(cx, source, Rc::new(move || clock.get())));
        let (visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Host));
        (visual, fake)
    }

    fn to_the_front(visual: &mut VisualTestContext) {
        visual.deactivate_window();
        visual.run_until_parked();
        visual.update(|window, _| window.activate_window());
        visual.run_until_parked();
    }

    #[gpui_kit::test]
    fn with_following_off_nothing_is_asked_until_it_is_turned_on(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = start(cx, false, false, vec![]);
        to_the_front(&mut visual);
        assert_eq!(fake.asked.get(), 0, "off: never asked, at start or in front");
        visual.update(|_, cx| state::update(cx, |s| s.follow_xyplorer = true));
        visual.run_until_parked();
        assert_eq!(fake.asked.get(), 1, "turned on later, it starts then");
    }

    #[gpui_kit::test]
    fn on_it_is_asked_at_start_and_when_the_window_comes_to_the_front_but_not_twice_in_five_seconds(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let (mut visual, fake) = start(cx, true, false, vec![]);
        assert_eq!(fake.asked.get(), 1, "asked once when Glint starts");
        to_the_front(&mut visual);
        assert_eq!(fake.asked.get(), 1, "the front again within five seconds: not asked");
        fake.now.set(fake.now.get() + Duration::from_secs(6));
        to_the_front(&mut visual);
        assert_eq!(fake.asked.get(), 2, "later, asked again");
    }

    #[gpui_kit::test]
    fn never_two_questions_at_once(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, fake) = start(cx, true, true, vec![]);
        fake.now.set(fake.now.get() + Duration::from_secs(60));
        to_the_front(&mut visual);
        assert_eq!(fake.asked.get(), 1, "the first has not answered yet");
    }

    #[gpui_kit::test]
    fn what_came_is_told_to_the_rows_and_said_once_and_nothing_else_says_anything(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let followed = Followed {
            source: "tag.dat".into(),
            came: 2,
            gone: 1,
            paths: vec!["C:\\a.txt".into(), "C:\\b.txt".into()],
            labels: Vec::new(),
        };
        let answers = vec![Ok(Some(followed)), Ok(None), Err("tag.dat is locked".into()), Ok(Some(Followed::default()))];
        let (mut visual, fake) = start(cx, false, false, answers);
        let heard: Rc<RefCell<Vec<TagsChanged>>> = Rc::default();
        let sink = heard.clone();
        visual.update(|_, cx| {
            let hub = tags::hub(cx);
            cx.subscribe(&hub, move |_, event: &TagsChanged, _| sink.borrow_mut().push(event.clone())).detach();
        });
        // Turned on: asked, and the first answer is two that came and one that went.
        visual.update(|_, cx| state::update(cx, |s| s.follow_xyplorer = true));
        visual.run_until_parked();
        assert_eq!(fake.asked.get(), 1);
        assert_eq!(heard.borrow().len(), 1);
        assert_eq!(heard.borrow()[0].paths, ["C:\\a.txt", "C:\\b.txt"]);
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.notifications(cx).len(), 1, "one quiet line");
            assert_eq!(line(3), "3 marks came from XYplorer");
            assert_eq!(line(1), "1 mark came from XYplorer");
        });
        // Nothing changed, an error, and an answer of zero: no event, no line.
        for _ in 0..3 {
            fake.now.set(fake.now.get() + Duration::from_secs(6));
            to_the_front(&mut visual);
        }
        assert_eq!(fake.asked.get(), 4);
        assert_eq!(heard.borrow().len(), 1, "no more events");
        visual.update(|window, cx| assert_eq!(window.notifications(cx).len(), 1, "still the one line"));
    }

    #[gpui_kit::test]
    fn following_hands_settings_labels_as_they_are_now_and_adds_the_label_xyplorer_gained_since(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let sky = glint_core::marks::LabelPlan {
            name: "天蓝色".into(),
            color: 0x000e_a5e9,
            to: "天蓝色".into(),
            new: true,
            takes_name: false,
        };
        let followed = Followed { came: 1, paths: vec!["C:\\a.txt".into()], labels: vec![sky.clone()], ..Followed::default() };
        let (mut visual, fake) = start(cx, false, false, vec![Ok(Some(followed))]);
        // Renamed in Glint after the import: the question carries the list as it is now.
        visual.update(|_, cx| state::update(cx, |s| s.labels[0].name = "红色".into()));
        visual.update(|_, cx| state::update(cx, |s| s.follow_xyplorer = true));
        visual.run_until_parked();
        let handed = fake.handed.borrow().clone();
        assert_eq!(handed.len(), 1);
        assert_eq!(handed[0][0].name, "红色");
        let now = visual.update(|_, cx| state::settings(cx).labels.clone());
        assert_eq!(now.len(), handed[0].len() + 1, "the new label came into Settings");
        let last = now.last().expect("a label");
        assert_eq!((last.name.as_str(), last.color), ("天蓝色", sky.color));
        assert_eq!(now[0].name, "红色", "what the person renamed stays");
    }
}
