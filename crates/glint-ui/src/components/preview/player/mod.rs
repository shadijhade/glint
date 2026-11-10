//! Video and audio played with Media Foundation, with Trim, the waveform, the transcript and scrubbing
//! (`VideoPreview` and `AudioPreview` of FilePreview.tsx, Waveform.tsx, VideoScrub.tsx; previews-native.md
//! section 10, approach A).
//!
//! | TypeScript                                  | Here                                                        |
//! |---------------------------------------------|-------------------------------------------------------------|
//! | `<video controls>` / `<audio controls>`     | [`PlayerView`]: the picture, play, seek bar, time, volume, speed |
//! | `onError` and `CantPlay`                    | the engine's [`Event::Failed`] and `media_tools::cant_play` |
//! | `TrimBar` under the player                  | `media_tools::trim_bar`, wired to the engine here           |
//! | `Waveform`                                  | [`wave`]: Media Foundation decodes, [`PlayerView`] draws    |
//! | `Transcript`                                | [`transcript::Transcript`]                                  |
//! | `VideoScrub` over a grid tile               | [`scrub::Scrub`]                                            |
//!
//! **How it plays.** The engine is Media Foundation's `IMFMediaEngine` in frame-server mode, on a thread of its
//! own ([`mf`]): it plays the sound itself and hands over frames, scaled on the GPU to the size they are drawn at,
//! which the view draws as an image. A recording uses the same engine as audio only. The engine is behind the
//! [`Engine`] trait ([`sources::PlayerSources::open`]) so the tests use a fake. Letting go of the view lets go of
//! the engine, which pauses and shuts down: a closed preview makes no sound.
//!
//! **A file Windows can't play.** A codec Windows lacks (HEVC, AV1, VP9 without their Store extension…) makes the
//! engine say [`Event::Failed`]; a local video then shows `media_tools::cant_play`'s card, which has FFmpeg make a
//! copy that plays, and the player plays the copy (the TypeScript's `failed` and `copy`). The engine's own error
//! decides, not ffprobe's guess (`media_probe`'s `playable` is about WebView2's codecs, not Windows').
//!
//! **Keys** (when the picture, the seek bar or the waveform has the focus): Space or K play and pause; the
//! Left and Right arrows seek 5 seconds (the way the bar points: they swap in Arabic), J and L 10; Up and Down the
//! volume; M mutes; Home and End go to the start and the end. On the volume slider the arrows change the volume.
//! Click the picture to play and pause. The keys are bound by [`bind_keys`] (the first view made does it) in the
//! contexts `"Player"`, `"PlayerVolume"` and `"Transcript"`, deeper than Quick Look's, so they win there. Not
//! ported: the browser's fullscreen button (Quick Look's Maximize is the way to a bigger player).
//!
//! # Wiring (for the preview pane)
//!
//! - **Which arm.** `choose.rs`'s `Some(PreviewKind::Video)` and `Some(PreviewKind::Audio)` (the `PORT ME(video)`
//!   and `PORT ME(audio)` ones) get a body such as `Body::Player(Kind)`; the frame builds it with
//!   `cx.new(|cx| PlayerView::new(entry.clone(), Kind::Video, window, cx))` (or `Kind::Audio`). It does what
//!   `PictureView` does for a file in an archive (extracts it first, with the same spinner), so archived media
//!   take this arm too (the fallback for them is `Body::Icon` today).
//! - **When the selection moves.** The frame builds a new body for a new file and drops the old one, which stops
//!   it; nothing is called. (A body kept for the same file needs nothing either.)
//! - **Quick Look's Page Up and Down.** `page_of!(…, PlayerView)` in `EntryView::page_body`: [`PlayerView::page`]
//!   scrolls the transcript when it has one.
//! - **Frames.** Call [`release_retired`] from the pane's render beside `picture::release_retired`: it lets the
//!   pictures of views that have gone go of the GPU.
//! - **The grid.** A tile that shows a video lays a [`scrub::Scrub`] over itself (see its notes).

pub mod engine;
#[cfg(test)]
mod fake;
pub mod mf;
pub mod scrub;
pub mod sources;
#[cfg(test)]
mod tests;
pub mod transcript;
pub mod wave;

pub use engine::{Command, Engine, Event, Facts, Frame, Kind, Opened};
pub use scrub::Scrub;
pub use sources::{init_with, player_sources, Heard, PlayerSources};
pub use transcript::{Transcript, TranscriptEvent};

use super::states::{centered, Waiting};
use crate::dialogs::media_tools::{self, CantPlay, CantPlayEvent, TrimBar, TrimEvent};
use crate::explorer;
use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use engine::{across, clock, next_speed, speed_label};
use glint_model::entries::{is_archived, is_remote, FileEntry};
use glint_model::file_types::{kind_of, type_label};
use gpui_kit::{
    actions, canvas, img, ClickEvent, KeyBinding, MouseButton, MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels,
    RenderImage, Size, StyledImage as _, Task,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use transcript::says_words;

actions!(
    glint_player,
    [
        TogglePlay,
        SeekLeft,
        SeekRight,
        SeekBack10,
        SeekForward10,
        VolumeUp,
        VolumeDown,
        VolumeLeft,
        VolumeRight,
        ToggleMute,
        ToStart,
        ToEnd,
        RowUp,
        RowDown,
        RowPlay,
    ]
);

/// The seconds the arrows seek.
const SEEK_SMALL: f64 = 5.;
/// And J and L.
const SEEK_BIG: f64 = 10.;
/// How much the volume keys change the volume.
const VOLUME_STEP: f64 = 0.1;
/// The width of a recording's controls and waveform (`max-w-sm`).
const NARROW: f32 = 96.;

thread_local! {
    /// Pictures whose view has gone, until a frame can let them go of the GPU's atlas.
    static RETIRED: RefCell<Vec<Arc<RenderImage>>> = const { RefCell::new(Vec::new()) };
}

/// Hands a picture that is no longer shown over to be let go of the GPU's atlas by the next frame.
pub(crate) fn retire(image: Arc<RenderImage>) {
    RETIRED.with(|retired| retired.borrow_mut().push(image));
}

/// Lets pictures that are no longer shown go of the GPU's atlas. Call once per frame from a view that has the
/// window (the pane's render does, beside `picture::release_retired`).
pub fn release_retired(window: &mut Window) {
    for image in RETIRED.with(|retired| std::mem::take(&mut *retired.borrow_mut())) {
        window.drop_image(image).ok();
    }
}

struct Bound;

impl Global for Bound {}

/// Binds the player's keys (once; the first [`PlayerView`] does it). The contexts are deeper than Quick Look's, so
/// Space and the arrows are the player's while it has the focus.
pub fn bind_keys(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    let player = Some("Player");
    let volume = Some("PlayerVolume");
    let rows = Some("Transcript");
    cx.bind_keys([
        KeyBinding::new("space", TogglePlay, player),
        KeyBinding::new("k", TogglePlay, player),
        KeyBinding::new("left", SeekLeft, player),
        KeyBinding::new("right", SeekRight, player),
        KeyBinding::new("j", SeekBack10, player),
        KeyBinding::new("l", SeekForward10, player),
        KeyBinding::new("up", VolumeUp, player),
        KeyBinding::new("down", VolumeDown, player),
        KeyBinding::new("m", ToggleMute, player),
        KeyBinding::new("home", ToStart, player),
        KeyBinding::new("end", ToEnd, player),
        KeyBinding::new("left", VolumeLeft, volume),
        KeyBinding::new("right", VolumeRight, volume),
        KeyBinding::new("up", VolumeUp, volume),
        KeyBinding::new("down", VolumeDown, volume),
        KeyBinding::new("m", ToggleMute, volume),
        KeyBinding::new("up", RowUp, rows),
        KeyBinding::new("down", RowDown, rows),
        KeyBinding::new("enter", RowPlay, rows),
        KeyBinding::new("space", RowPlay, rows),
    ]);
}

/// What the player shows.
enum Phase {
    /// A file in an archive is being copied out to play.
    Extracting,
    /// The engine has the file (loading or playing).
    Playing,
    /// Windows can't play it: the card that makes a copy that it can.
    Cant(Entity<CantPlay>),
    /// It can't be played, and why.
    Failed(SharedString),
}

/// What the pointer is dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Seek,
    Volume,
}

/// A video or a recording, playing.
pub struct PlayerView {
    entry: FileEntry,
    kind: Kind,
    /// On this PC's disk: Trim, the waveform and the transcript work only on such files.
    local: bool,
    sources: PlayerSources,
    phase: Phase,
    engine: Option<Box<dyn Engine>>,
    /// The playable copy `CantPlay` made, when it did (`copy`).
    copy: Option<String>,
    facts: Facts,
    loaded: bool,
    position: f64,
    playing: bool,
    ended: bool,
    waiting: bool,
    volume: f64,
    muted: bool,
    rate: f64,
    frame: Option<Frame>,
    cover: Option<Arc<RenderImage>>,
    peaks: Option<Arc<Vec<f32>>>,
    /// The space the picture may take, measured while drawing.
    area: Rc<Cell<Size<Pixels>>>,
    /// The box last asked of the engine, in device pixels.
    asked: (u32, u32),
    seek_track: Rc<Cell<Bounds<Pixels>>>,
    volume_track: Rc<Cell<Bounds<Pixels>>>,
    wave_track: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    /// The time the thumb shows while it is dragged.
    dragged_to: f64,
    focus: FocusHandle,
    seek_focus: FocusHandle,
    volume_focus: FocusHandle,
    wave_focus: FocusHandle,
    /// The spinner for what takes a moment (copying out of an archive, the first frame).
    waiting_card: Entity<Waiting>,
    trim: Option<Entity<TrimBar>>,
    /// Where the engine was last told to pause by itself.
    stop_sent: Option<f64>,
    transcript: Option<Entity<Transcript>>,
    tasks: Vec<Task<()>>,
    _events: Option<Task<()>>,
    _card: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl PlayerView {
    /// The player for `entry`, a video or a recording (`kind`). A file in an archive is copied out first; the
    /// engine opens at once, off the UI thread, and the file's own work (the cover, the waveform, the transcript)
    /// starts beside it.
    pub fn new(entry: FileEntry, kind: Kind, window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_keys(cx);
        let sources = player_sources(cx);
        let archived = is_archived(&entry);
        let local = !archived && !is_remote(&entry);
        let waiting_card = cx.new(|cx| {
            Waiting::new(
                archived.then(|| {
                    if is_remote(&entry) {
                        t!("Downloading…")
                    } else {
                        t!("Extracting from the archive…")
                    }
                }),
                cx,
            )
        });
        let mut subscriptions = Vec::new();
        let trim = local
            .then(|| media_tools::trim_bar(window, cx, entry.path.clone(), entry.name.clone(), kind == Kind::Video));
        if let Some(trim) = &trim {
            subscriptions.push(cx.subscribe(trim, |this, _, event: &TrimEvent, cx| this.trim_asked(event, cx)));
            subscriptions.push(cx.observe(trim, |this, _, cx| this.sync_stop(cx)));
        }
        let transcript = local.then(|| cx.new(|cx| Transcript::new(entry.path.clone(), says_of(cx), cx)));
        if let Some(transcript) = &transcript {
            subscriptions.push(cx.subscribe(transcript, |this, _, event: &TranscriptEvent, cx| {
                let TranscriptEvent::Play(seconds) = event;
                this.seek(*seconds, cx);
                this.play(cx);
            }));
            // `says:` in the search changes what is marked.
            if let Some(explorer) = explorer::try_explorer(cx) {
                subscriptions.push(cx.observe(&explorer, |this, _, cx| {
                    let words = says_of(cx);
                    if let Some(transcript) = &this.transcript {
                        transcript.update(cx, |transcript, cx| transcript.set_words(words, cx));
                    }
                }));
            }
        }
        let mut view = PlayerView {
            kind,
            local,
            sources,
            phase: if archived { Phase::Extracting } else { Phase::Playing },
            engine: None,
            copy: None,
            facts: Facts::default(),
            loaded: false,
            position: 0.,
            playing: false,
            ended: false,
            waiting: false,
            volume: 1.,
            muted: false,
            rate: 1.,
            frame: None,
            cover: None,
            peaks: None,
            area: Rc::new(Cell::new(Size::default())),
            asked: (0, 0),
            seek_track: Rc::new(Cell::new(Bounds::default())),
            volume_track: Rc::new(Cell::new(Bounds::default())),
            wave_track: Rc::new(Cell::new(Bounds::default())),
            drag: None,
            dragged_to: 0.,
            focus: cx.focus_handle(),
            seek_focus: cx.focus_handle(),
            volume_focus: cx.focus_handle(),
            wave_focus: cx.focus_handle(),
            waiting_card,
            trim,
            stop_sent: None,
            transcript,
            tasks: Vec::new(),
            _events: None,
            _card: None,
            _subscriptions: subscriptions,
            entry,
        };
        view.begin(window, cx);
        view
    }

    // ---- What the tests and the pane ask -------------------------------------------------------------

    /// The file.
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }

    /// Where it is, in seconds.
    pub fn position(&self) -> f64 {
        if self.drag == Some(Drag::Seek) {
            self.dragged_to
        } else {
            self.position
        }
    }

    /// Its length in seconds, 0 until the engine knows.
    pub fn duration(&self) -> f64 {
        self.facts.duration
    }

    /// Whether it plays.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Whether it reached the end.
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// The volume, 0 to 1.
    pub fn volume(&self) -> f64 {
        self.volume
    }

    /// Whether it is muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// The speed.
    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Whether a picture of the video has arrived.
    pub fn has_frame(&self) -> bool {
        self.frame.is_some()
    }

    /// The waveform's bars once decoded.
    pub fn waveform(&self) -> Option<&Arc<Vec<f32>>> {
        self.peaks.as_ref()
    }

    /// Why it can't be played, once the engine said so and no copy was offered.
    pub fn failure(&self) -> Option<&str> {
        match &self.phase {
            Phase::Failed(note) => Some(note),
            _ => None,
        }
    }

    /// The card that offers a copy that plays, when Windows couldn't play the video as it is.
    pub fn cant_play(&self) -> Option<&Entity<CantPlay>> {
        match &self.phase {
            Phase::Cant(card) => Some(card),
            _ => None,
        }
    }

    /// The Trim bar (a file on this PC's disk has one).
    pub fn trim(&self) -> Option<&Entity<TrimBar>> {
        self.trim.as_ref()
    }

    /// The transcript (a file on this PC's disk has one; it shows nothing until speech search has heard it).
    pub fn transcript(&self) -> Option<&Entity<Transcript>> {
        self.transcript.as_ref()
    }

    /// Page Down and Page Up in Quick Look: the transcript scrolls a page. Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        match &self.transcript {
            Some(transcript) => transcript.update(cx, |transcript, cx| transcript.page(down, cx)),
            None => false,
        }
    }

    // ---- Starting --------------------------------------------------------------------------------------

    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if is_archived(&self.entry) {
            let running = (self.sources.extract)(&self.entry.path);
            self.tasks.push(cx.spawn_in(window, async move |this, cx| {
                let copy = running.await;
                this.update_in(cx, |this, window, cx| match copy {
                    Ok(path) => {
                        this.phase = Phase::Playing;
                        this.start(&path, window, cx);
                    }
                    Err(reason) => this.give_up(SharedString::from(reason), cx),
                })
                .ok();
            }));
        } else {
            let path = self.entry.path.clone();
            self.start(&path, window, cx);
        }
        if self.kind == Kind::Audio && !is_archived(&self.entry) {
            let running = (self.sources.cover)(&self.entry.path);
            self.tasks.push(cx.spawn(async move |this, cx| {
                let cover = running.await;
                this.update(cx, |this, cx| {
                    this.cover = cover;
                    cx.notify();
                })
                .ok();
            }));
        }
        if self.kind == Kind::Audio && self.local && self.entry.size <= wave::MAX_BYTES {
            let running = (self.sources.peaks)(&self.entry.path);
            self.tasks.push(cx.spawn(async move |this, cx| {
                let peaks = running.await;
                this.update(cx, |this, cx| {
                    this.peaks = peaks.filter(|p| !p.is_empty()).map(Arc::new);
                    cx.notify();
                })
                .ok();
            }));
        }
    }

    /// Opens `path` in the engine and listens to it.
    fn start(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.reset();
        let Opened { engine, events } = (self.sources.open)(path, self.kind);
        self.engine = Some(engine);
        self.phase = Phase::Playing;
        self._events = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(first) = events.recv().await {
                // Everything that arrived together is applied together, and drawn once.
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                let alive = this.update_in(cx, |this, window, cx| {
                    for event in batch {
                        this.heard(event, window, cx);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        }));
    }

    /// Forgets the last file's state.
    fn reset(&mut self) {
        self.engine = None;
        self.retire_frame();
        self.facts = Facts::default();
        self.loaded = false;
        self.position = 0.;
        self.playing = false;
        self.ended = false;
        self.waiting = false;
        self.asked = (0, 0);
        self.stop_sent = None;
        self.volume = 1.;
        self.muted = false;
        self.rate = 1.;
    }

    fn retire_frame(&mut self) {
        if let Some(frame) = self.frame.take() {
            retire(frame.image);
        }
    }

    /// The engine said something.
    fn heard(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::Loaded(facts) => {
                self.facts = facts;
                self.loaded = true;
                if let Some(trim) = &self.trim {
                    trim.update(cx, |trim, cx| trim.set_duration(facts.duration, cx));
                }
            }
            Event::Time(seconds) => {
                if self.drag != Some(Drag::Seek) {
                    self.position = seconds;
                }
                // The Trim bar's part ends here (`stopAtEnd`); the engine pauses at the very moment, this keeps
                // the TypeScript's rule when the engine is a different one.
                if let Some(end) = self.stop_sent {
                    if self.playing && seconds >= end {
                        self.send(Command::Pause);
                    }
                }
            }
            Event::Playing(playing) => {
                self.playing = playing;
                if playing {
                    self.ended = false;
                }
            }
            Event::Ended => {
                self.ended = true;
                self.playing = false;
            }
            Event::Waiting(waiting) => self.waiting = waiting,
            Event::Frame => {
                if let Some(frame) = self.engine.as_ref().and_then(|engine| engine.take_frame()) {
                    if let Some(old) = self.frame.replace(frame) {
                        retire(old.image);
                    }
                }
            }
            Event::Failed(why) => self.failed(why, window, cx),
        }
    }

    /// Windows can't play it: a local video gets the card that makes a copy that it can (once); anything else
    /// says so.
    fn failed(&mut self, _why: String, window: &mut Window, cx: &mut Context<Self>) {
        self.engine = None;
        self.playing = false;
        self.retire_frame();
        if self.kind == Kind::Video && self.local && self.copy.is_none() {
            let card = media_tools::cant_play(window, cx, self.entry.path.clone(), self.entry.size);
            self._card = Some(
                cx.subscribe_in(&card, window, |this, _, event: &CantPlayEvent, window, cx| {
                    let CantPlayEvent::Ready(path) = event;
                    this.copy = Some(path.clone());
                    this._card = None;
                    this.start(path, window, cx);
                    cx.notify();
                }),
            );
            self.phase = Phase::Cant(card);
        } else {
            self.give_up(
                if self.kind == Kind::Video {
                    t!("This video can't be played here")
                } else {
                    t!("This recording can't be played here")
                },
                cx,
            );
        }
        cx.notify();
    }

    fn give_up(&mut self, note: SharedString, cx: &mut Context<Self>) {
        self.engine = None;
        self.phase = Phase::Failed(note);
        cx.notify();
    }

    // ---- Doing things -----------------------------------------------------------------------------------

    fn send(&self, command: Command) {
        if let Some(engine) = &self.engine {
            engine.send(command);
        }
    }

    /// Plays (from the start when it had ended).
    pub fn play(&mut self, cx: &mut Context<Self>) {
        if self.engine.is_none() {
            return;
        }
        if self.ended {
            self.position = 0.;
            self.send(Command::Seek(0.));
        }
        self.send(Command::Play);
        cx.notify();
    }

    /// Pauses.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        self.send(Command::Pause);
        cx.notify();
    }

    /// Space, K and the play button.
    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.pause(cx);
        } else {
            self.play(cx);
        }
    }

    /// Goes to `seconds` (kept inside the file once its length is known).
    pub fn seek(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if self.engine.is_none() || !seconds.is_finite() {
            return;
        }
        let top = if self.facts.duration > 0. {
            self.facts.duration
        } else {
            f64::MAX
        };
        let seconds = seconds.clamp(0., top);
        self.position = seconds;
        self.ended = false;
        self.send(Command::Seek(seconds));
        cx.notify();
    }

    fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        let now = self.position();
        self.seek(now + delta, cx);
    }

    /// Sets the volume, 0 to 1; raising it from nothing unmutes, as the browser's slider does.
    pub fn set_volume(&mut self, volume: f64, cx: &mut Context<Self>) {
        let volume = volume.clamp(0., 1.);
        self.volume = volume;
        self.send(Command::Volume(volume));
        if self.muted && volume > 0. {
            self.muted = false;
            self.send(Command::Muted(false));
        }
        cx.notify();
    }

    /// The mute button and M.
    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.send(Command::Muted(self.muted));
        cx.notify();
    }

    /// The speed button: the next speed.
    pub fn cycle_speed(&mut self, cx: &mut Context<Self>) {
        self.rate = next_speed(self.rate);
        self.send(Command::Rate(self.rate));
        cx.notify();
    }

    /// The Trim bar asked for something.
    fn trim_asked(&mut self, event: &TrimEvent, cx: &mut Context<Self>) {
        match event {
            TrimEvent::Seek(seconds) => self.seek(*seconds, cx),
            TrimEvent::PlayPart { start, end } => {
                self.stop_sent = Some(*end);
                self.send(Command::StopAt(Some(*end)));
                self.seek(*start, cx);
                self.play(cx);
            }
        }
    }

    /// Tells the engine where the open Trim bar's part ends (it pauses there), or that there is none.
    fn sync_stop(&mut self, cx: &mut Context<Self>) {
        let stop = self.trim.as_ref().and_then(|trim| trim.read(cx).stop_at());
        if stop != self.stop_sent {
            self.stop_sent = stop;
            self.send(Command::StopAt(stop));
        }
    }

    /// Asks the engine for pictures the size they are drawn at, when that size has changed.
    fn ask_for_frames(&mut self, scale: f32) {
        if !self.facts.has_video {
            return;
        }
        let area = self.area.get();
        let want = (
            (f32::from(area.width) * scale).floor().max(0.) as u32,
            (f32::from(area.height) * scale).floor().max(0.) as u32,
        );
        if want != self.asked && want.0 > 0 && want.1 > 0 {
            self.asked = want;
            self.send(Command::Frame {
                width: want.0,
                height: want.1,
            });
        }
    }

    // ---- Dragging the bars ------------------------------------------------------------------------------

    fn fraction_in(&self, track: &Cell<Bounds<Pixels>>, x: Pixels) -> f64 {
        let bounds = track.get();
        across(
            f32::from(x),
            f32::from(bounds.origin.x),
            f32::from(bounds.size.width),
            i18n::is_rtl(),
        )
    }

    fn grab_seek(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.seek_focus, cx);
        if self.facts.duration <= 0. {
            return;
        }
        self.drag = Some(Drag::Seek);
        self.drag_seek(x, cx);
    }

    fn drag_seek(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let to = self.fraction_in(&self.seek_track, x) * self.facts.duration;
        self.dragged_to = to;
        self.seek(to, cx);
    }

    fn grab_volume(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.volume_focus, cx);
        self.drag = Some(Drag::Volume);
        self.drag_volume(x, cx);
    }

    fn drag_volume(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let to = self.fraction_in(&self.volume_track, x);
        self.set_volume(to, cx);
    }

    fn pointer_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            return;
        }
        match drag {
            Drag::Seek => self.drag_seek(event.position.x, cx),
            Drag::Volume => self.drag_volume(event.position.x, cx),
        }
    }

    fn release(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.take() == Some(Drag::Seek) {
            self.position = self.dragged_to;
            cx.notify();
        }
    }

    /// A click on the waveform: play from there (it only moves, as the TypeScript's did).
    fn click_wave(&mut self, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.wave_focus, cx);
        if self.facts.duration > 0. {
            let to = self.fraction_in(&self.wave_track, x) * self.facts.duration;
            self.seek(to, cx);
        }
    }
}

impl Drop for PlayerView {
    fn drop(&mut self) {
        // The engine is dropped with the view and stops; the pictures go back from the GPU.
        self.retire_frame();
        if let Some(cover) = self.cover.take() {
            retire(cover);
        }
    }
}

/// The words `says:` searched for in the active tab's search (`useExplorer(...find.query)`).
fn says_of(cx: &App) -> String {
    let Some(explorer) = explorer::try_explorer(cx) else {
        return String::new();
    };
    let explorer = explorer.read(cx);
    let tab = explorer
        .panes()
        .get(explorer.active_pane())
        .and_then(|pane| explorer.tab(pane.active));
    tab.and_then(|tab| tab.find.as_ref())
        .map(|find| says_words(&find.query))
        .unwrap_or_default()
}

// ---- Drawing ---------------------------------------------------------------------------------------------

impl Render for PlayerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_retired(window);
        let t = theme::tokens(cx).clone();
        let narrow = self.kind == Kind::Audio;
        match &self.phase {
            Phase::Extracting => return self.waiting_card.clone().into_any_element(),
            Phase::Cant(card) => return centered(cx).p(space(3.)).child(card.clone()).into_any_element(),
            Phase::Failed(note) => {
                let note = note.clone();
                return crate::components::preview::states::big_icon(&self.entry, Some(note), None, cx)
                    .into_any_element();
            }
            Phase::Playing => {}
        }
        self.ask_for_frames(window.scale_factor());
        let name = self.entry.name.clone();
        let label = if narrow {
            t!("Recording: {name}", name = name)
        } else {
            t!("Video: {name}", name = name)
        };
        let top: AnyElement = if narrow {
            self.cover_view(&t, window, cx)
        } else {
            self.video_view(&t, window, cx)
        };
        let controls = self.controls(&t, narrow, window, cx);
        let wave = (narrow && self.local).then(|| self.wave_view(&t, window, cx)).flatten();
        col()
            .id("player")
            .role(Role::Group)
            .aria_label(label)
            .test_support()
            .size_full()
            .min_h_0()
            .items_center()
            .justify_center()
            .gap(space(3.))
            .p(space(if narrow { 6. } else { 3. }))
            .text_size(theme::size::SM)
            .text_color(t.muted_foreground)
            .on_action(cx.listener(|this, _: &TogglePlay, _, cx| this.toggle_play(cx)))
            .on_action(cx.listener(|this, _: &SeekLeft, _, cx| {
                this.seek_by(if i18n::is_rtl() { SEEK_SMALL } else { -SEEK_SMALL }, cx)
            }))
            .on_action(cx.listener(|this, _: &SeekRight, _, cx| {
                this.seek_by(if i18n::is_rtl() { -SEEK_SMALL } else { SEEK_SMALL }, cx)
            }))
            .on_action(cx.listener(|this, _: &SeekBack10, _, cx| this.seek_by(-SEEK_BIG, cx)))
            .on_action(cx.listener(|this, _: &SeekForward10, _, cx| this.seek_by(SEEK_BIG, cx)))
            .on_action(cx.listener(|this, _: &VolumeUp, _, cx| this.set_volume(this.volume + VOLUME_STEP, cx)))
            .on_action(cx.listener(|this, _: &VolumeDown, _, cx| this.set_volume(this.volume - VOLUME_STEP, cx)))
            .on_action(cx.listener(|this, _: &VolumeLeft, _, cx| {
                let step = if i18n::is_rtl() { VOLUME_STEP } else { -VOLUME_STEP };
                this.set_volume(this.volume + step, cx)
            }))
            .on_action(cx.listener(|this, _: &VolumeRight, _, cx| {
                let step = if i18n::is_rtl() { -VOLUME_STEP } else { VOLUME_STEP };
                this.set_volume(this.volume + step, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _, cx| this.toggle_mute(cx)))
            .on_action(cx.listener(|this, _: &ToStart, _, cx| this.seek(0., cx)))
            .on_action(cx.listener(|this, _: &ToEnd, _, cx| {
                let end = this.facts.duration;
                this.seek(end, cx)
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.pointer_moved(event, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::release))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::release))
            .child(top)
            .child(controls)
            .children(wave)
            .children(
                self.trim
                    .clone()
                    .map(|trim| div().w_full().flex().justify_center().child(trim)),
            )
            .children(
                self.transcript
                    .clone()
                    .map(|transcript| div().w_full().flex().justify_center().child(transcript)),
            )
            .into_any_element()
    }
}

impl PlayerView {
    /// The video: black behind the frame, the frame fitted in the space there is and never enlarged.
    fn video_view(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let scale = window.scale_factor();
        let area_cell = self.area.clone();
        let id = cx.entity_id();
        let measuring = canvas(
            move |bounds, window, _| {
                if area_cell.get() != bounds.size {
                    area_cell.set(bounds.size);
                    // Not `cx.notify` here: GPUI drops a redraw asked for during a draw. After this frame it counts.
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let picture = self.frame.clone().map(|frame| {
            // One image pixel to one device pixel: the engine made it the size it is drawn at.
            let (w, h) = (px(frame.width as f32 / scale), px(frame.height as f32 / scale));
            img(frame.image)
                .object_fit(ObjectFit::Fill)
                .w(w)
                .h(h)
                .into_any_element()
        });
        let focused = self.focus.is_focused(window);
        let stage = if let Some(picture) = picture {
            picture
        } else if self.loaded && !self.facts.has_video {
            // A "video" with no picture in it: its kind's icon.
            let kind = kind_of(&self.entry);
            let tone = theme::kind_tone(kind, cx);
            icon(crate::kit::kind_icon(kind.icon))
                .size(px(64.))
                .text_color(tone.color)
                .into_any_element()
        } else {
            div().size(px(96.)).child(self.waiting_card.clone()).into_any_element()
        };
        div()
            .id("player-video")
            .role(Role::Group)
            .aria_label(t!("Picture of {name}", name = self.entry.name.clone()))
            .test_support()
            .key_context("Player")
            .track_focus(&self.focus)
            .tab_stop(true)
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .cursor_pointer()
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus, cx);
                this.toggle_play(cx);
            }))
            .child(measuring)
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex_none()
                            .overflow_hidden()
                            .rounded(theme::size::RADIUS_SM)
                            .when(self.frame.is_some(), |stage| stage.bg(gpui_kit::black()))
                            .child(stage),
                    ),
            )
            // The ring is its own layer, so the picture doesn't move when the keyboard arrives.
            .when(focused, |video| {
                video.child(
                    div()
                        .absolute()
                        .size_full()
                        .rounded(theme::size::RADIUS_SM)
                        .border_2()
                        .border_color(t.ring),
                )
            })
            .into_any_element()
    }

    /// A song's cover (`size-44 rounded-md object-cover shadow-md`), or its kind's icon.
    fn cover_view(&mut self, _t: &theme::Tokens, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if let Some(cover) = self.cover.clone() {
            return div()
                .id("player-cover")
                .role(Role::Image)
                .aria_label(t!("Cover"))
                .test_support()
                .size(rems(11.))
                .flex_none()
                .overflow_hidden()
                .rounded(theme::size::RADIUS)
                .shadow_md()
                .child(img(cover).object_fit(ObjectFit::Cover).size_full())
                .into_any_element();
        }
        let kind = kind_of(&self.entry);
        let tone = theme::kind_tone(kind, cx);
        div()
            .id("player-icon")
            .role(Role::Image)
            .aria_label(SharedString::from(type_label(&self.entry)))
            .test_support()
            .flex_none()
            .child(
                icon(crate::kit::kind_icon(kind.icon))
                    .size(px(64.))
                    .text_color(tone.color),
            )
            .into_any_element()
    }

    /// The bars under a recording's player; nothing until they are decoded (or when the file is too big).
    fn wave_view(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let peaks = self.peaks.clone()?;
        let duration = self.facts.duration;
        let played = if duration > 0. {
            (self.position() / duration).clamp(0., 1.)
        } else {
            0.
        };
        let measure = self.wave_track.clone();
        let id = cx.entity_id();
        let probe = canvas(
            move |bounds, window, _| {
                if measure.get() != bounds {
                    measure.set(bounds);
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let focused = self.wave_focus.is_focused(window);
        Some(
            div()
                .id("player-waveform")
                .role(Role::Slider)
                .aria_label(t!("Waveform: click to play from there"))
                .aria_description(self.time_text())
                .test_support()
                .key_context("Player")
                .track_focus(&self.wave_focus)
                .tab_stop(true)
                .relative()
                .h(space(14.))
                .w_full()
                .max_w(space(NARROW))
                .cursor_pointer()
                .rounded(theme::size::RADIUS_SM)
                .border_1()
                .border_color(if focused { t.ring } else { t.background.opacity(0.) })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                        this.click_wave(event.position.x, window, cx)
                    }),
                )
                .child(probe)
                .child(div().absolute().size_full().child(wave::bars(
                    peaks,
                    played,
                    i18n::is_rtl(),
                    t.primary,
                    t.muted_foreground.opacity(0.35),
                )))
                .into_any_element(),
        )
    }

    /// "0:12 / 3:07", or "0:12" while the length is unknown.
    fn time_text(&self) -> SharedString {
        if self.facts.duration > 0. {
            t!(
                "{time} of {total}",
                time = clock(self.position()),
                total = clock(self.facts.duration)
            )
        } else {
            clock(self.position()).into()
        }
    }

    /// Play, the time, the seek bar, mute, volume and speed.
    fn controls(&mut self, t: &theme::Tokens, narrow: bool, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let small = |id: &'static str, glyph: IconName, label: SharedString| {
            Button::new(id)
                .ghost()
                .compact()
                .icon(glyph)
                .accessibility_label(label.clone())
                .tooltip(label)
        };
        let play = small(
            "player-play",
            if self.waiting && self.playing {
                IconName::LoaderCircle
            } else if self.playing {
                IconName::Pause
            } else {
                IconName::Play
            },
            if self.playing { t!("Pause") } else { t!("Play") },
        )
        .on_click(cx.listener(|this, _, _, cx| this.toggle_play(cx)));
        let silent = self.muted || self.volume <= 0.;
        let mute = small(
            "player-mute",
            if silent {
                IconName::VolumeX
            } else if self.volume < 0.5 {
                IconName::Volume1
            } else {
                IconName::Volume2
            },
            if silent { t!("Unmute") } else { t!("Mute") },
        )
        .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx)));
        let speed = Button::new("player-speed")
            .ghost()
            .compact()
            .label(speed_label(self.rate))
            .accessibility_label(t!("Playback speed: {speed}", speed = speed_label(self.rate)))
            .tooltip(t!("Playback speed"))
            .on_click(cx.listener(|this, _, _, cx| this.cycle_speed(cx)));
        let time = self.time_text();
        let timer = div()
            .id("player-time")
            .role(Role::Status)
            .aria_label(time.clone())
            .test_support()
            .flex_none()
            .px(space(1.))
            .text_size(theme::size::XS)
            .text_color(t.foreground)
            .child(time);
        col()
            .id("player-controls")
            .role(Role::Group)
            .aria_label(t!("Player controls"))
            .test_support()
            .flex_none()
            .w_full()
            .when(narrow, |controls| controls.max_w(space(NARROW)))
            .gap(space(0.5))
            .child(self.seek_view(t, window, cx))
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.))
                    .child(play)
                    .child(timer)
                    .child(div().flex_1())
                    .child(mute)
                    .child(self.volume_view(t, window, cx))
                    .child(speed),
            )
            .into_any_element()
    }

    /// The seek bar: a track, what has played, and a thumb.
    fn seek_view(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let duration = self.facts.duration;
        let played = if duration > 0. {
            (self.position() / duration).clamp(0., 1.)
        } else {
            0.
        };
        slider(
            "player-seek",
            self.seek_track.clone(),
            &self.seek_focus,
            "Player",
            t!("Seek"),
            self.time_text(),
            played,
            Some(t.primary),
            self.seek_focus.is_focused(window),
            None,
            cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                this.grab_seek(event.position.x, window, cx)
            }),
            t,
            cx,
        )
        .into_any_element()
    }

    /// The volume slider (`w-16`).
    fn volume_view(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let shown = if self.muted { 0. } else { self.volume };
        slider(
            "player-volume",
            self.volume_track.clone(),
            &self.volume_focus,
            "PlayerVolume",
            t!("Volume"),
            t!("{n}%", n = (shown * 100.).round()),
            shown,
            None,
            self.volume_focus.is_focused(window),
            Some(space(16.)),
            cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| {
                this.grab_volume(event.position.x, window, cx)
            }),
            t,
            cx,
        )
        .into_any_element()
    }
}

/// A horizontal slider drawn by hand: the track, the part up to `at` (0 to 1) from the start edge, a thumb. It
/// records where the track is drawn in `measure`.
#[allow(clippy::too_many_arguments)] // Every one is a different thing about the one slider; a struct would only rename them.
fn slider(
    id: &'static str,
    measure: Rc<Cell<Bounds<Pixels>>>,
    focus: &FocusHandle,
    context: &'static str,
    name: SharedString,
    value: SharedString,
    at: f64,
    fill_color: Option<Hsla>,
    focused: bool,
    width: Option<Rems>,
    on_down: impl Fn(&gpui_kit::MouseDownEvent, &mut Window, &mut App) + 'static,
    t: &theme::Tokens,
    cx: &mut Context<PlayerView>,
) -> impl IntoElement {
    let rtl = i18n::is_rtl();
    let entity = cx.entity_id();
    let probe = canvas(
        move |bounds, window, _| {
            if measure.get() != bounds {
                measure.set(bounds);
                window.on_next_frame(move |_, cx| cx.notify(entity));
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full();
    let fill_color = fill_color.unwrap_or(t.foreground.opacity(0.7));
    let filled = div()
        .absolute()
        .top(space(2.25))
        .h(space(1.5))
        .rounded_full()
        .bg(fill_color);
    let knob = div()
        .absolute()
        .top(space(1.))
        .size(space(4.))
        .rounded_full()
        .bg(t.primary)
        .border_2()
        .border_color(if focused { t.ring } else { t.background })
        .shadow_sm();
    let (filled, knob) = if rtl {
        (
            filled.right_0(),
            knob.right(gpui_kit::relative(at as f32)).mr(space(-2.)),
        )
    } else {
        (filled.left_0(), knob.left(gpui_kit::relative(at as f32)).ml(space(-2.)))
    };
    div()
        .id(id)
        .role(Role::Slider)
        .aria_label(name)
        .aria_description(value)
        .test_support()
        .key_context(context)
        .track_focus(focus)
        .tab_stop(true)
        .relative()
        .h(space(6.))
        .when_some(width, |slider, width| slider.w(width).flex_none())
        .when(width.is_none(), |slider| slider.w_full())
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, on_down)
        .child(probe)
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(space(2.25))
                .h(space(1.5))
                .rounded_full()
                .bg(t.muted),
        )
        .child(filled.w(gpui_kit::relative(at as f32)))
        .child(knob)
}
