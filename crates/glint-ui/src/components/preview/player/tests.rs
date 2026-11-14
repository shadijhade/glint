//! The player, headless: a fake engine behind the view, so what is drawn, and what clicks and keys send, are seen
//! without Windows' media engine. The Arabic UI and Arabic content are used where direction or text matters.

use super::fake::{self, Answers, Opening};
use super::scrub::{parse_frames, Scrub};
use super::*;
use crate::dialogs::media_tools::Handle;
use crate::explorer::fake::file;
use crate::testing;
use glint_model::entries::attr;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{point, px, TestAppContext, VisualTestContext};

struct Env {
    view: Entity<PlayerView>,
    visual: VisualTestContext,
    opening: Rc<Opening>,
}

fn entry(name: &str, size: u64) -> FileEntry {
    file("C:\\media", name, size, 3)
}

/// A player for `entry`; `answers` is filled in by `setup` before it starts.
fn open_entry(cx: &mut TestAppContext, entry: FileEntry, kind: Kind, setup: impl FnOnce(&Answers)) -> Env {
    let opening = Rc::new(Opening::default());
    let answers = Rc::new(Answers::default());
    setup(&answers);
    cx.update(|cx| {
        testing::init(cx);
        init_with(cx, fake::sources(opening.clone(), answers));
    });
    let (visual, view) = testing::open_window(cx, move |window, cx| {
        cx.new(|cx| PlayerView::new(entry, kind, window, cx))
    });
    let mut env = Env { view, visual, opening };
    env.settle();
    env
}

fn open(cx: &mut TestAppContext, name: &str, kind: Kind) -> Env {
    open_entry(cx, entry(name, 5_000_000), kind, |_| {})
}

impl Env {
    /// A few frames, and everything that was waiting for them.
    fn settle(&mut self) {
        for _ in 0..3 {
            self.visual.update(|window, cx| window.render_frame(cx));
            self.visual.run_until_parked();
        }
    }

    /// The engine says these, in order.
    fn say(&mut self, events: impl IntoIterator<Item = Event>) {
        for event in events {
            self.opening.say(event);
        }
        self.settle();
    }

    fn has(&mut self, label: &str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn has_id(&mut self, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.settle();
    }

    /// The width of an element drawn, in pixels.
    fn width_of(&mut self, id: &'static str) -> f32 {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find(id).bounds().size.width)
        })
    }

    /// Clicks an element that far across it (0 to 1).
    fn click_across(&mut self, id: &'static str, fraction: f32) {
        let width = self.width_of(id);
        assert!(width > 20., "{id} is {width} wide");
        self.visual
            .update(|window, cx| window.click_at(id, point(px(width * fraction), px(8.)), cx));
        self.settle();
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| window.press(key, cx));
        self.settle();
    }

    /// Gives the keyboard to one of the view's handles.
    fn focus(&mut self, pick: impl FnOnce(&PlayerView) -> FocusHandle) {
        let handle = self.view.read_with(&self.visual, |view, _| pick(view));
        self.visual.update(|window, cx| handle.focus(window, cx));
        self.settle();
    }

    fn last_seek(&self) -> Option<f64> {
        self.opening.last(|c| match c {
            Command::Seek(at) => Some(*at),
            _ => None,
        })
    }

    fn last_volume(&self) -> Option<f64> {
        self.opening.last(|c| match c {
            Command::Volume(v) => Some(*v),
            _ => None,
        })
    }

    fn read<T>(&self, f: impl FnOnce(&PlayerView) -> T) -> T {
        self.view.read_with(&self.visual, |view, _| f(view))
    }
}

// ---- A video ----------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_video_opens_in_the_engine_and_shows_its_length_time_and_picture(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "clip.mp4", Kind::Video);
    assert_eq!(
        *env.opening.opened.borrow(),
        [("C:\\media\\clip.mp4".to_string(), Kind::Video)]
    );
    assert!(env.has("Video: clip.mp4"));
    env.say([Event::Loaded(fake::film(65., 1920, 1080))]);
    assert_eq!(env.read(|v| v.duration()), 65.);
    assert!(env.has("0:00 of 1:05"));
    // It asks for pictures the size it draws them: the measured box, in device pixels.
    let asked = env.opening.last(|c| match c {
        Command::Frame { width, height } => Some((*width, *height)),
        _ => None,
    });
    assert!(asked.is_some_and(|(w, h)| w > 100 && h > 100), "{asked:?}");
    assert!(!env.read(|v| v.has_frame()));
    env.opening
        .show(fake::picture(480, 270, [0x99, 0x66, 0x33, 255]), 480, 270);
    env.settle();
    assert!(env.read(|v| v.has_frame()));
    env.say([Event::Time(12.)]);
    assert!(env.has("0:12 of 1:05"));
    // A second picture replaces the first.
    env.opening.show(fake::picture(480, 270, [0, 0, 0, 255]), 480, 270);
    env.settle();
    assert!(env.read(|v| v.has_frame()));
}

#[gpui_kit::test]
fn play_and_pause_follow_the_engine_and_click_on_the_picture_toggles(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "clip.mp4", Kind::Video);
    env.say([Event::Loaded(fake::film(65., 640, 360))]);
    assert!(env.has("Play") && !env.has("Pause"));
    env.click("player-play");
    assert!(env.opening.was_sent(&Command::Play));
    // The button says Pause once the engine says it plays, not before.
    assert!(env.has("Play"));
    env.say([Event::Playing(true)]);
    assert!(env.read(|v| v.is_playing()));
    assert!(env.has("Pause") && !env.has("Play"));
    env.click("player-play");
    assert!(env.opening.was_sent(&Command::Pause));
    env.say([Event::Playing(false)]);
    assert!(env.has("Play"));
    // A click on the picture plays.
    let before = env.opening.sent().iter().filter(|c| **c == Command::Play).count();
    env.click("player-video");
    let after = env.opening.sent().iter().filter(|c| **c == Command::Play).count();
    assert_eq!(after, before + 1);
}

#[gpui_kit::test]
fn the_keys_seek_change_the_volume_and_mute(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "clip.mp4", Kind::Video);
    env.say([Event::Loaded(fake::film(65., 640, 360)), Event::Time(12.)]);
    env.focus(|v| v.seek_focus.clone());
    env.press("right");
    assert_eq!(env.last_seek(), Some(17.));
    env.press("left");
    assert_eq!(
        env.last_seek(),
        Some(12.),
        "the engine's time hasn't moved; the view's has"
    );
    env.press("l");
    assert_eq!(env.last_seek(), Some(22.));
    env.press("j");
    assert_eq!(env.last_seek(), Some(12.));
    env.press("end");
    assert_eq!(env.last_seek(), Some(65.));
    env.press("home");
    assert_eq!(env.last_seek(), Some(0.));
    env.press("left");
    assert_eq!(env.last_seek(), Some(0.), "never before the start");
    env.press("space");
    assert!(env.opening.was_sent(&Command::Play));
    env.say([Event::Playing(true)]);
    env.press("k");
    assert!(env.opening.was_sent(&Command::Pause));
    env.press("down");
    assert!((env.last_volume().unwrap_or(0.) - 0.9).abs() < 1e-9);
    env.press("up");
    env.press("up");
    assert_eq!(env.last_volume(), Some(1.), "never past full");
    env.press("m");
    assert!(env.opening.was_sent(&Command::Muted(true)));
    assert!(env.read(|v| v.is_muted()) && env.has("Unmute"));
    env.press("m");
    assert!(env.opening.was_sent(&Command::Muted(false)));
}

#[gpui_kit::test]
fn the_seek_bar_follows_the_pointer_and_mirrors_in_arabic(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "clip.mp4", Kind::Video);
    // Before the length is known there is nowhere to seek.
    env.click_across("player-seek", 0.5);
    assert_eq!(env.last_seek(), None);
    env.say([Event::Loaded(fake::film(100., 640, 360))]);
    env.click_across("player-seek", 0.25);
    assert!(
        (env.last_seek().unwrap_or(-1.) - 25.).abs() < 1.,
        "{:?}",
        env.last_seek()
    );
    assert!((env.read(|v| v.position()) - 25.).abs() < 1.);
    assert!(env.has("Seek"));
    let role = env
        .visual
        .update(|window, _| window.try_find_by_label("Seek").and_then(|e| e.role()));
    assert_eq!(role, Some(Role::Slider));

    // In Arabic the bar starts at the right: the same click is near the end, and the left arrow goes later.
    testing::set_rtl(true);
    env.view.update(&mut env.visual, |_, cx| cx.notify());
    env.settle();
    env.click_across("player-seek", 0.1);
    assert!(
        (env.last_seek().unwrap_or(-1.) - 90.).abs() < 1.,
        "{:?}",
        env.last_seek()
    );
    env.say([Event::Time(10.)]);
    env.focus(|v| v.seek_focus.clone());
    env.press("left");
    assert_eq!(env.last_seek(), Some(15.));
    env.press("right");
    assert_eq!(env.last_seek(), Some(10.));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn volume_mute_and_speed_have_their_controls(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "clip.mp4", Kind::Video);
    env.say([Event::Loaded(fake::film(65., 640, 360))]);
    env.click_across("player-volume", 0.5);
    assert!(
        (env.last_volume().unwrap_or(-1.) - 0.5).abs() < 0.1,
        "{:?}",
        env.last_volume()
    );
    assert!(env.has("Volume"));
    env.click("player-mute");
    assert!(env.read(|v| v.is_muted()));
    assert!(env.has("Unmute"));
    // Raising the volume unmutes (the browser's slider did).
    env.view.update(&mut env.visual, |v, cx| v.set_volume(0.3, cx));
    assert!(!env.read(|v| v.is_muted()));
    assert!(env.opening.was_sent(&Command::Muted(false)));
    // The speed button goes through the speeds and round.
    assert!(env.has("Playback speed: 1×"));
    env.click("player-speed");
    assert_eq!(env.read(|v| v.rate()), 1.25);
    assert!(env.has("Playback speed: 1.25×"));
    assert!(env.opening.was_sent(&Command::Rate(1.25)));
    for _ in 0..5 {
        env.click("player-speed");
    }
    assert_eq!(env.read(|v| v.rate()), 1.);
    // The keys on the volume slider are its own: the arrows change the volume, not the time.
    env.view.update(&mut env.visual, |v, cx| v.set_volume(0.5, cx));
    env.focus(|v| v.volume_focus.clone());
    env.press("right");
    assert!((env.last_volume().unwrap_or(0.) - 0.6).abs() < 1e-9);
    testing::set_rtl(true);
    env.view.update(&mut env.visual, |_, cx| cx.notify());
    env.settle();
    env.press("right");
    assert!(
        (env.last_volume().unwrap_or(0.) - 0.5).abs() < 1e-9,
        "mirrored in Arabic"
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_video_that_ended_plays_again_from_the_start(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "clip.mp4", Kind::Video);
    env.say([
        Event::Loaded(fake::film(10., 640, 360)),
        Event::Playing(true),
        Event::Time(10.),
        Event::Ended,
    ]);
    assert!(env.read(|v| v.is_ended() && !v.is_playing()));
    env.click("player-play");
    let sent = env.opening.sent();
    let at = sent.iter().rposition(|c| *c == Command::Play).expect("Play was sent");
    assert_eq!(sent.get(at - 1), Some(&Command::Seek(0.)), "{sent:?}");
}

#[gpui_kit::test]
fn a_video_with_no_picture_shows_its_icon(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "talk.mp4", Kind::Video);
    env.say([Event::Loaded(fake::song(30.))]);
    // Nothing is asked for: there is no picture to size.
    assert_eq!(
        env.opening.last(|c| matches!(c, Command::Frame { .. }).then_some(())),
        None
    );
    assert!(env.has("0:00 of 0:30"));
}

// ---- A recording ------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_recording_has_its_cover_a_waveform_and_no_pictures(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open_entry(cx, entry("أغنية.mp3", 5_000_000), Kind::Audio, |answers| {
        *answers.cover.borrow_mut() = Some(fake::picture(64, 64, [1, 2, 3, 255]));
        *answers.peaks.borrow_mut() = Some((0..48).map(|i| i as f32 / 47.).collect());
    });
    assert_eq!(env.opening.opened.borrow()[0].1, Kind::Audio);
    assert!(env.has("Recording: أغنية.mp3"));
    env.say([Event::Loaded(fake::song(200.))]);
    assert!(env.has("Cover"));
    assert!(env.has("Waveform: click to play from there"));
    assert!(env.read(|v| v.waveform().is_some()));
    assert!(!env.has_id("player-video"));
    assert_eq!(
        env.opening.last(|c| matches!(c, Command::Frame { .. }).then_some(())),
        None
    );
    // A click on the waveform moves there, and only moves.
    env.click_across("player-waveform", 0.5);
    assert!(
        (env.last_seek().unwrap_or(-1.) - 100.).abs() < 2.,
        "{:?}",
        env.last_seek()
    );
    assert!(!env.opening.was_sent(&Command::Play));
    // Mirrored in Arabic: the start of the recording is at the right.
    testing::set_rtl(true);
    env.view.update(&mut env.visual, |_, cx| cx.notify());
    env.settle();
    env.click_across("player-waveform", 0.1);
    assert!(
        (env.last_seek().unwrap_or(-1.) - 180.).abs() < 2.,
        "{:?}",
        env.last_seek()
    );
    // Arabic text in the name and in the labels.
    assert!(env.has("Recording: أغنية.mp3"));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_song_with_no_cover_shows_its_icon_and_a_huge_recording_has_no_waveform(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open_entry(cx, entry("big.wav", wave::MAX_BYTES + 1), Kind::Audio, |answers| {
        *answers.peaks.borrow_mut() = Some(vec![0.5; 10]);
    });
    env.say([Event::Loaded(fake::song(5000.))]);
    assert!(env.has_id("player-icon") && !env.has("Cover"));
    assert!(env.read(|v| v.waveform().is_none()));
    assert!(!env.has("Waveform: click to play from there"));
    let mut env = open_entry(cx, entry("small.wav", 1000), Kind::Audio, |answers| {
        *answers.peaks.borrow_mut() = Some(vec![]);
    });
    env.say([Event::Loaded(fake::song(5.))]);
    assert!(env.read(|v| v.waveform().is_none()), "no bars is no waveform");
}

// ---- Trim -------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_trim_bar_seeks_and_plays_a_part_that_stops_at_its_end(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "clip.mp4", Kind::Video);
    env.say([Event::Loaded(fake::film(60., 640, 360))]);
    let trim = env.read(|v| v.trim().cloned()).expect("a local video has a Trim bar");
    assert!(env.has_id("trim-open"));
    env.click("trim-open");
    // Open, the player must pause at the part's end.
    assert!(env.opening.was_sent(&Command::StopAt(Some(60.))));
    trim.update(&mut env.visual, |bar, cx| bar.move_handle(Handle::Start, 10., cx));
    trim.update(&mut env.visual, |bar, cx| bar.move_handle(Handle::End, 20., cx));
    env.settle();
    assert_eq!(
        env.opening.last(|c| matches!(c, Command::StopAt(_)).then(|| c.clone())),
        Some(Command::StopAt(Some(20.)))
    );
    env.click("trim-play");
    let sent = env.opening.sent();
    let play = sent.iter().rposition(|c| *c == Command::Play).expect("it plays");
    assert_eq!(sent[play - 1], Command::Seek(10.), "{sent:?}");
    env.say([Event::Playing(true), Event::Time(15.)]);
    assert!(!sent.contains(&Command::Pause));
    let pauses = env.opening.sent().iter().filter(|c| **c == Command::Pause).count();
    assert_eq!(pauses, 0, "not yet");
    // Past the end of the part, it pauses.
    env.say([Event::Time(20.2)]);
    let pauses = env.opening.sent().iter().filter(|c| **c == Command::Pause).count();
    assert_eq!(pauses, 1);
    // Closing the bar lets it play on.
    env.click("trim-close");
    assert_eq!(
        env.opening.last(|c| matches!(c, Command::StopAt(_)).then(|| c.clone())),
        Some(Command::StopAt(None))
    );
}

#[gpui_kit::test]
fn a_file_in_an_archive_has_no_trim_or_waveform(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut archived = entry("inside.mp3", 1000);
    archived.attrs |= attr::IN_ARCHIVE;
    let mut env = open_entry(cx, archived, Kind::Audio, |answers| {
        *answers.extracted.borrow_mut() = Some(Ok("C:\\tmp\\inside.mp3".to_string()));
        *answers.peaks.borrow_mut() = Some(vec![0.5; 10]);
    });
    // It is the copy that plays.
    assert_eq!(env.opening.opened.borrow()[0].0, "C:\\tmp\\inside.mp3");
    env.say([Event::Loaded(fake::song(5.))]);
    assert!(env.read(|v| v.trim().is_none() && v.waveform().is_none()));
    assert!(!env.has_id("trim-open"));
}

#[gpui_kit::test]
fn a_file_that_cannot_be_copied_out_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut archived = entry("inside.mp4", 1000);
    archived.attrs |= attr::IN_ARCHIVE;
    let mut env = open_entry(cx, archived, Kind::Video, |answers| {
        *answers.extracted.borrow_mut() = Some(Err("The archive is damaged".to_string()));
    });
    assert!(env.opening.opened.borrow().is_empty());
    assert_eq!(
        env.read(|v| v.failure().map(str::to_string)).as_deref(),
        Some("The archive is damaged")
    );
    assert!(env.has("The archive is damaged"));
}

// ---- Files Windows can't play -----------------------------------------------------------------------------

#[gpui_kit::test]
fn a_video_windows_cannot_play_offers_a_copy_that_plays(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    // Big enough that the copy waits for a click, so nothing runs behind the test.
    let mut env = open_entry(cx, entry("old.avi", 600 << 20), Kind::Video, |_| {});
    env.say([Event::Failed("Windows has no codec for it".into())]);
    assert!(env.opening.is_dropped(), "the engine is let go of");
    let card = env.read(|v| v.cant_play().cloned()).expect("the can't-play card");
    assert!(env.has("This video doesn't play here as it is"));
    // The copy is ready: it plays.
    card.update(&mut env.visual, |_, cx| {
        cx.emit(CantPlayEvent::Ready("C:\\copies\\old.mp4".into()))
    });
    env.settle();
    let opened = env.opening.opened.borrow().clone();
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[1], ("C:\\copies\\old.mp4".to_string(), Kind::Video));
    assert!(env.read(|v| v.cant_play().is_none()));
    // The copy failing too is the end of it.
    env.say([Event::Failed("still no".into())]);
    assert_eq!(
        env.read(|v| v.failure().map(str::to_string)).as_deref(),
        Some("This video can't be played here")
    );
    assert!(env.has("This video can't be played here"));
}

#[gpui_kit::test]
fn a_recording_that_cannot_be_played_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open(cx, "odd.mp3", Kind::Audio);
    env.say([Event::Failed("Windows couldn't decode it".into())]);
    assert!(env.read(|v| v.cant_play().is_none()));
    assert_eq!(
        env.read(|v| v.failure().map(str::to_string)).as_deref(),
        Some("This recording can't be played here")
    );
    assert!(env.has("This recording can't be played here"));
}

// ---- Letting go ---------------------------------------------------------------------------------------------

struct Holder(Option<Entity<PlayerView>>);

impl Render for Holder {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match &self.0 {
            Some(view) => div().size_full().child(view.clone()).into_any_element(),
            None => div().into_any_element(),
        }
    }
}

#[gpui_kit::test]
fn letting_go_of_the_view_stops_the_engine_and_a_playing_one_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let opening = Rc::new(Opening::default());
    let answers = Rc::new(Answers::default());
    cx.update(|cx| {
        testing::init(cx);
        init_with(cx, fake::sources(opening.clone(), answers));
    });
    let (mut visual, holder) = testing::open_window(cx, |window, cx| {
        let player = cx.new(|cx| PlayerView::new(entry("clip.mp4", 5_000_000), Kind::Video, window, cx));
        cx.new(|_| Holder(Some(player)))
    });
    visual.run_until_parked();
    opening.say(Event::Loaded(fake::film(60., 640, 360)));
    opening.say(Event::Playing(true));
    opening.show(fake::picture(64, 36, [9, 9, 9, 255]), 64, 36);
    visual.update(|window, cx| window.render_frame(cx));
    visual.run_until_parked();
    assert!(!opening.is_dropped());
    holder.update(&mut visual, |holder, cx| {
        holder.0 = None;
        cx.notify();
    });
    visual.update(|window, cx| window.render_frame(cx));
    visual.run_until_parked();
    assert!(opening.is_dropped(), "the sound stops with the view");
}

// ---- Scrubbing over a tile ---------------------------------------------------------------------------------

fn tile_frames() -> Arc<scrub::Frames> {
    let mut bytes = Vec::new();
    for n in [10u32, 4, 2] {
        bytes.extend_from_slice(&n.to_le_bytes());
    }
    bytes.extend_from_slice(&125f64.to_le_bytes());
    for frame in 0..10u8 {
        for _ in 0..8 {
            bytes.extend_from_slice(&[frame * 20, 10, 10, 255]);
        }
    }
    Arc::new(parse_frames(&bytes).expect("ten frames"))
}

#[gpui_kit::test]
fn the_pointer_on_a_tile_shows_the_frame_it_is_across_with_the_time(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let opening = Rc::new(Opening::default());
    let answers = Rc::new(Answers::default());
    *answers.frames.borrow_mut() = Some(tile_frames());
    cx.update(|cx| {
        testing::init(cx);
        init_with(cx, fake::sources(opening, answers));
    });
    let (mut visual, scrub) =
        testing::open_window(cx, |_, cx| cx.new(|cx| Scrub::new("C:\\v\\tile.mp4".into(), 128, cx)));
    visual.update(|window, cx| window.render_frame(cx));
    visual.update(|window, cx| window.hover("video-scrub", cx));
    for _ in 0..4 {
        visual.update(|window, cx| window.render_frame(cx));
        visual.run_until_parked();
    }
    assert!(scrub.read_with(&visual, |s, _| s.frames().is_some()));
    // The middle of the tile is the sixth tenth, 62 seconds into 125.
    let at = scrub.read_with(&visual, |s, _| s.at()).expect("the pointer is on it");
    assert!((at - 0.5).abs() < 0.05, "{at}");
    assert!(visual.update(|window, _| window.try_find_by_label("At 1:02").is_some()));
    // Off the tile it draws nothing again.
    scrub.update(&mut visual, |s, cx| s.leave(cx));
    visual.update(|window, cx| window.render_frame(cx));
    assert!(scrub.read_with(&visual, |s, _| s.at().is_none()));
    assert!(visual.update(|window, _| window.try_find_by_label("At 1:02").is_none()));
}

#[gpui_kit::test]
fn a_tile_whose_frames_cannot_be_read_stays_quiet(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    cx.update(|cx| {
        testing::init(cx);
        init_with(
            cx,
            fake::sources(Rc::new(Opening::default()), Rc::new(Answers::default())),
        );
    });
    let (mut visual, scrub) =
        testing::open_window(cx, |_, cx| cx.new(|cx| Scrub::new("C:\\v\\bad.mp4".into(), 128, cx)));
    visual.update(|window, cx| window.render_frame(cx));
    visual.update(|window, cx| window.hover("video-scrub", cx));
    for _ in 0..3 {
        visual.update(|window, cx| window.render_frame(cx));
        visual.run_until_parked();
    }
    assert!(scrub.read_with(&visual, |s, _| s.frames().is_none()));
}

// ---- Names --------------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn every_control_has_a_name_and_none_is_named_twice(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut env = open_entry(cx, entry("clip.mp4", 5_000_000), Kind::Video, |_| {});
    env.say([Event::Loaded(fake::film(65., 640, 360))]);
    for label in [
        "Play",
        "Mute",
        "Seek",
        "Volume",
        "Playback speed: 1×",
        "Player controls",
    ] {
        let count = env.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find_all_by_label(label).len()
        });
        assert_eq!(count, 1, "{label}");
    }
}
