//! asciinema recordings played (CastPreview.tsx, `src/lib/cast.ts`; previews-native.md section 9).
//!
//! The TypeScript played a .cast file in the terminal pane's own renderer. So does this: the recording's output
//! is fed through the same emulator ([`emu::Screen`](crate::components::terminal::emu::Screen)) and painted by the
//! same element as a live terminal, inside a [`TerminalView::playback`]. What this file adds is the player.
//!
//! | TypeScript                                     | Here                                                        |
//! |------------------------------------------------|-------------------------------------------------------------|
//! | `CastPreview`: read 4 MB, `parseCast`          | [`CastView`] reads through [`CastReader`], `glint_model::cast::parse_cast` in the background |
//! | `Player`: play, pause, the range, the clock    | [`CastView`]: a play button, a seek bar (a slider), `m:ss / m:ss` |
//! | `SPEEDS` 1 2 4 in a `<select>`                 | one button that goes 1×, 2×, 4× and round, named "Speed"   |
//! | `outputUntil` then `\x1bc`: seek by replay     | [`Screen::replay`](crate::components::terminal::emu::Screen::replay): a fresh terminal fed up to the moment, swapped in |
//! | the open-on-the-last-frame behavior            | the same: it opens at the end, Play starts it over          |
//! | `BigIcon` with the note when it isn't a cast   | [`big_icon`](super::text::big_icon) with the same words     |
//!
//! **Playing.** Every frame while playing, the events up to the moment (`base + elapsed x speed`) are fed to the
//! screen. All feeding and replaying happens on one background task that takes its jobs in order, so a seek never
//! races a feed: a burst of seeks (dragging the bar) is cut to the last, and the events before a seek are dropped.
//! The UI thread only counts and redraws.
//!
//! **Seeking** replays from the start: a fresh terminal is fed every event up to the moment, and swapped in under the
//! lock. The recording is read up to 4 MB, and alacritty's parser handles that in a fraction of a second in a
//! release build (the test `a_four_megabyte_recording_replays_in_a_blink` measures it), so there are no snapshots:
//! `Term` can't be cloned, and a forward seek simply feeds on from where it is.
//!
//! # Where the previewer is placed (for the coordinator)
//!
//! - **Which `choose.rs` arm.** `Choice::later(Body::Text, "cast")` for `readable && entry.ext == "cast"` becomes
//!   `Choice::of(Body::Cast)` (a variant to add; the file must be readable and on disk or extracted, like Docx).
//! - **Make it.** `cx.new(|cx| CastView::new(path, window, cx))` in `frame.rs`'s `build`. It needs the terminal
//!   module's feature (`terminal`), which is the same one that builds this file.
//! - **When it can't play the file.** It shows the file's icon with the note "This isn't an asciinema recording
//!   Glint can play (version 2 or 3)." itself, as the TypeScript's `BigIcon` did; nothing for the frame to do.
//! - **Selection moves.** A new `CastView` per file, or [`CastView::set_path`]; a file that changed on disk:
//!   [`CastView::reload`] (the TypeScript's effect depended on `entry.modified`).
//! - **Quick Look's Page Down / Page Up.** `page(down, cx)` scrolls a recording taller than the pane; add `CastView`
//!   to `page_of!`.
//! - **Keys.** Nothing is bound in the app's keymap. With the seek bar focused: Left and Right seek 5 seconds (swapped
//!   in Arabic, where the bar starts at the right), Home and End go to the ends, Space plays and pauses. A recording
//!   takes no other keys, as in the TypeScript (they stay the app's).
//!
//! # Not ported
//!
//! - Snapshots for seeking (see above).
//! - The browser's own `<select>` for the speed: a button cycles it. The speeds are the same.
//! - Input events ("i") and markers are not played, as before; resize events ("r") are not applied (the recording
//!   keeps its header's size, as xterm.js did).

use super::sources::Fut;
use super::text::{big_icon, redraw_after_delay, waiting};
use crate::components::terminal::{TerminalEvent, TerminalView};
use crate::kit::prelude::*;
use crate::{core_bridge, i18n, t, theme};
use glint_model::cast::{clock, parse_cast, Cast, DEFAULT_IDLE_SECONDS};
use gpui_kit::{
    canvas, Bounds, DragMoveEvent, EventEmitter, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Role, ScrollHandle,
    Task,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The most of a recording read: past it, what it printed after isn't played (`MAX_BYTES`).
pub const MAX_BYTES: usize = 4 << 20;
/// The playback speeds (`SPEEDS`).
pub const SPEEDS: [f64; 3] = [1., 2., 4.];
/// How far the arrow keys seek on the bar, seconds.
const SEEK_STEP: f64 = 5.;
/// A frame: playing advances no oftener than this.
const FRAME: Duration = Duration::from_millis(16);
/// Within this of the end, the player is "at the end" (Play starts over).
const END_SLACK: f64 = 0.01;
const NOT_A_CAST: &str = "This isn't an asciinema recording Glint can play (version 2 or 3).";

/// Reads the start of a recording: its text, and whether the file went on past [`MAX_BYTES`]. glint-core in the
/// app, a fake in tests.
pub type CastReader = Rc<dyn Fn(&str) -> Fut<Result<(String, bool), String>>>;

/// glint-core's `read_text_preview`, up to [`MAX_BYTES`]; a file that isn't text isn't a recording.
pub fn core_reader() -> CastReader {
    Rc::new(|path| {
        let running = core_bridge::run(glint_core::fsx::read_text_preview(path.to_string(), MAX_BYTES, None));
        Box::pin(async move {
            let read = running.await.and_then(|read| read)?;
            if read.binary {
                Err(NOT_A_CAST.to_string())
            } else {
                Ok((read.text, read.truncated))
            }
        })
    })
}

/// What the background task is asked, in order.
enum Op {
    /// Print events `from..to`.
    Feed(usize, usize),
    /// Show the moment after the first `count` events: a fresh terminal fed from the start.
    Replay(usize),
}

enum Load {
    Waiting,
    Failed(SharedString),
    Ready,
}

/// The payload of a drag on the seek bar.
struct ScrubDrag;

/// What follows the pointer while the bar is dragged: nothing.
struct NoChip;

impl Render for NoChip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// What the view tells its owner. Nothing yet: the failure is shown inside the view.
pub enum CastEvent {
    /// The recording could not be played (the note is shown in the view).
    Failed,
}

/// An asciinema recording, played (`CastPreview`).
pub struct CastView {
    path: String,
    reader: CastReader,
    load: Load,
    started: Instant,
    generation: u64,
    cast: Option<Arc<Cast>>,
    truncated: bool,
    terminal: Option<Entity<TerminalView>>,
    ops: Option<async_channel::Sender<Op>>,
    /// Jobs sent to the background task and not yet done.
    pending: usize,
    /// Seconds in.
    position: f64,
    /// Events already printed (or being).
    next: usize,
    playing: bool,
    speed: usize,
    /// Where playing was (seconds) when the clock was last set.
    base: f64,
    clock: Instant,
    scrub: Rc<Cell<Bounds<Pixels>>>,
    scrub_focus: FocusHandle,
    focus: FocusHandle,
    scroll: ScrollHandle,
    _read: Task<()>,
    _worker: Task<()>,
    _ticker: Task<()>,
    _events: Option<gpui_kit::Subscription>,
}

impl EventEmitter<CastEvent> for CastView {}

impl CastView {
    /// The player for the recording at `path`. Reading and parsing happen in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_reader(path, core_reader(), window, cx)
    }

    /// [`CastView::new`] reading through `reader`.
    pub fn with_reader(path: String, reader: CastReader, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = CastView {
            path,
            reader,
            load: Load::Waiting,
            started: Instant::now(),
            generation: 0,
            cast: None,
            truncated: false,
            terminal: None,
            ops: None,
            pending: 0,
            position: 0.,
            next: 0,
            playing: false,
            speed: 0,
            base: 0.,
            clock: Instant::now(),
            scrub: Rc::new(Cell::new(Bounds::default())),
            scrub_focus: cx.focus_handle(),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            _read: Task::ready(()),
            _worker: Task::ready(()),
            _ticker: Task::ready(()),
            _events: None,
        };
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another recording.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.read(window, cx);
        }
    }

    /// Reads the recording again (it changed on disk).
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read(window, cx);
    }

    /// Whether the file could not be played.
    pub fn failed(&self) -> bool {
        matches!(self.load, Load::Failed(_))
    }

    /// Whether the recording is loaded and ready to play.
    pub fn ready(&self) -> bool {
        matches!(self.load, Load::Ready)
    }

    /// Seconds in.
    pub fn position(&self) -> f64 {
        self.position
    }

    /// The recording's length in seconds, once loaded.
    pub fn duration(&self) -> f64 {
        self.cast.as_ref().map_or(0., |cast| cast.duration)
    }

    /// Whether it is playing.
    #[cfg(test)]
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The speed now (1, 2 or 4).
    pub fn speed(&self) -> f64 {
        SPEEDS.get(self.speed).copied().unwrap_or(1.)
    }

    /// Whether everything asked of the background task has been done (tests wait for this).
    #[cfg(test)]
    pub fn settled(&self) -> bool {
        self.pending == 0
    }

    /// The terminal the recording plays in.
    pub fn terminal(&self) -> Option<&Entity<TerminalView>> {
        self.terminal.as_ref()
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = Load::Waiting;
        self.started = Instant::now();
        self.playing = false;
        self._ticker = Task::ready(());
        let (path, reader) = (self.path.clone(), self.reader.clone());
        self._read = cx.spawn_in(window, async move |this, cx| {
            let read = reader(&path).await;
            let parsed = match read {
                Ok((text, truncated)) => {
                    let cast = cx
                        .background_spawn(async move { parse_cast(&text, DEFAULT_IDLE_SECONDS) })
                        .await;
                    cast.map(|cast| (cast, truncated)).ok_or_else(|| NOT_A_CAST.to_string())
                }
                Err(reason) => Err(reason),
            };
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                match parsed {
                    Ok((cast, truncated)) => this.loaded(cast, truncated, window, cx),
                    Err(reason) => {
                        this.load = Load::Failed(reason.into());
                        this.cast = None;
                        this.terminal = None;
                        cx.emit(CastEvent::Failed);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// The recording is parsed: its terminal is made and shows the last frame (it opens at the end).
    fn loaded(&mut self, cast: Cast, truncated: bool, window: &mut Window, cx: &mut Context<Self>) {
        let cast = Arc::new(cast);
        let terminal = cx.new(|cx| TerminalView::playback(cast.cols, cast.rows, t!("Recording"), window, cx));
        // A recording's terminal never reports anything the player needs.
        self._events = Some(cx.subscribe(&terminal, |_, _, _: &TerminalEvent, _| {}));
        let screen = terminal.read(cx).screen().clone();
        let (tx, rx) = async_channel::unbounded::<Op>();
        let events = cast.clone();
        self._worker = cx.spawn(async move |this, cx| {
            while let Ok(first) = rx.recv().await {
                let mut jobs = vec![first];
                while let Ok(more) = rx.try_recv() {
                    jobs.push(more);
                }
                let count = jobs.len();
                // A replay shows a moment from nothing: whatever was asked before it doesn't matter.
                if let Some(last) = jobs.iter().rposition(|job| matches!(job, Op::Replay(_))) {
                    jobs.drain(..last);
                }
                let (screen, events) = (screen.clone(), events.clone());
                cx.background_spawn(async move { run(&screen, &events, &jobs) }).await;
                let alive = this.update(cx, |this, cx| {
                    this.pending = this.pending.saturating_sub(count);
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        self.ops = Some(tx);
        self.terminal = Some(terminal);
        self.cast = Some(cast.clone());
        self.truncated = truncated;
        self.pending = 0;
        self.next = cast.events.len();
        self.position = cast.duration;
        self.base = cast.duration;
        self.load = Load::Ready;
        self.send(Op::Replay(cast.events.len()));
    }

    fn send(&mut self, op: Op) {
        if let Some(ops) = &self.ops {
            if ops.try_send(op).is_ok() {
                self.pending += 1;
            }
        }
    }

    // ---- Playing

    /// Plays or pauses (the button). At the end, it plays from the start.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.pause(cx);
        } else {
            self.play(cx);
        }
    }

    /// Plays from where it is; from the start when it is at the end.
    pub fn play(&mut self, cx: &mut Context<Self>) {
        let Some(cast) = self.cast.clone() else { return };
        if self.position >= cast.duration - END_SLACK {
            self.jump(0., &cast);
        }
        self.playing = true;
        self.base = self.position;
        self.clock = Instant::now();
        self._ticker = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(FRAME).await;
            match this.update(cx, |this, cx| this.tick(cx)) {
                Ok(true) => {}
                _ => break,
            }
        });
        cx.notify();
    }

    /// Pauses where it is.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        if self.playing {
            self.playing = false;
            self._ticker = Task::ready(());
            cx.notify();
        }
    }

    /// One frame of playing. Returns whether it goes on.
    fn tick(&mut self, cx: &mut Context<Self>) -> bool {
        let at = self.base + self.clock.elapsed().as_secs_f64() * self.speed();
        self.advance_to(at, cx);
        self.playing
    }

    /// Prints what the recording printed up to `seconds` (forward from the position) and moves the position
    /// there. Stops at the end.
    pub fn advance_to(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let Some(cast) = self.cast.clone() else { return };
        let at = seconds.clamp(0., cast.duration);
        let end = cast.events.partition_point(|event| event.t <= at);
        if end > self.next {
            self.send(Op::Feed(self.next, end));
            self.next = end;
        }
        self.position = at;
        if at >= cast.duration {
            self.playing = false;
            self.base = at;
            self._ticker = Task::ready(());
        }
        cx.notify();
    }

    /// Shows the moment `seconds` in, paused: the screen cleared and everything printed up to it.
    pub fn seek(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let Some(cast) = self.cast.clone() else { return };
        self.playing = false;
        self._ticker = Task::ready(());
        self.jump(seconds, &cast);
        cx.notify();
    }

    fn jump(&mut self, seconds: f64, cast: &Cast) {
        let at = seconds.clamp(0., cast.duration);
        let count = cast.events.partition_point(|event| event.t <= at);
        self.position = at;
        self.base = at;
        self.next = count;
        self.clock = Instant::now();
        self.send(Op::Replay(count));
    }

    /// Seeks `by` seconds on from the position.
    pub fn seek_by(&mut self, by: f64, cx: &mut Context<Self>) {
        self.seek(self.position + by, cx);
    }

    /// The next speed: 1×, 2×, 4× and round. Playing carries on from where it is.
    pub fn cycle_speed(&mut self, cx: &mut Context<Self>) {
        // The time played so far is kept, at the old speed.
        self.base = (self.base + self.clock.elapsed().as_secs_f64() * self.speed()).min(self.duration());
        if !self.playing {
            self.base = self.position;
        }
        self.clock = Instant::now();
        self.speed = (self.speed + 1) % SPEEDS.len();
        cx.notify();
    }

    /// Scrolls a recording taller than the pane a page (Page Down and Page Up in Quick Look). Returns whether it
    /// moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let max = self.scroll.max_offset().y;
        let viewport = self.scroll.bounds().size.height;
        if max <= px(0.) {
            return false;
        }
        let offset = self.scroll.offset();
        let step = viewport * 0.85;
        let top = if down { offset.y - step } else { offset.y + step };
        let top = top.clamp(-max, px(0.));
        if top == offset.y {
            return false;
        }
        self.scroll.set_offset(gpui_kit::point(offset.x, top));
        cx.notify();
        true
    }

    // ---- The seek bar

    /// The bar was pressed or dragged at window x `x`.
    fn scrub_to(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.scrub.get();
        let fraction = across(
            f32::from(x),
            f32::from(bounds.origin.x),
            f32::from(bounds.size.width),
            i18n::is_rtl(),
        );
        self.seek(fraction * self.duration(), cx);
    }

    /// The keys of the seek bar and the view.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        if keys.modifiers.control || keys.modifiers.alt || keys.modifiers.platform {
            return;
        }
        if !(self.scrub_focus.is_focused(window) || self.focus.is_focused(window)) {
            return;
        }
        let rtl = i18n::is_rtl();
        match keys.key.as_str() {
            "space" => self.toggle(cx),
            "left" => self.seek_by(if rtl { SEEK_STEP } else { -SEEK_STEP }, cx),
            "right" => self.seek_by(if rtl { -SEEK_STEP } else { SEEK_STEP }, cx),
            "home" => self.seek(0., cx),
            "end" => self.seek(self.duration(), cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

/// How far across a track of `width` the pointer at `x` is (0 to 1), from the track's start edge; the bar starts
/// at the right edge in right to left.
// TODO(coordinator): from `preview::player::engine::across`.
pub fn across(x: f32, left: f32, width: f32, rtl: bool) -> f64 {
    if width <= 0. {
        return 0.;
    }
    let fraction = f64::from(((x - left) / width).clamp(0., 1.));
    if rtl {
        1. - fraction
    } else {
        fraction
    }
}

/// Does the background task's jobs, in order.
fn run(screen: &crate::components::terminal::emu::Screen, cast: &Cast, jobs: &[Op]) {
    for job in jobs {
        match job {
            Op::Feed(from, to) => {
                let mut bytes = Vec::new();
                for event in cast.events.get(*from..*to).unwrap_or_default() {
                    bytes.extend_from_slice(event.data.as_bytes());
                }
                screen.feed(&bytes);
            }
            Op::Replay(count) => {
                let chunks: Vec<&[u8]> = cast
                    .events
                    .get(..*count)
                    .unwrap_or_default()
                    .iter()
                    .map(|event| event.data.as_bytes())
                    .collect();
                screen.replay(&chunks);
            }
        }
    }
}

impl Focusable for CastView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CastView {
    /// The seek bar: a track, the part played, a knob; a slider for the keyboard and screen readers.
    fn scrubber(&self, t: &theme::Tokens, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rtl = i18n::is_rtl();
        let duration = self.duration();
        let at = if duration > 0. {
            (self.position / duration).clamp(0., 1.)
        } else {
            0.
        };
        let focused = self.scrub_focus.is_focused(window);
        let measure = self.scrub.clone();
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
        let filled = div()
            .absolute()
            .top(space(2.25))
            .h(space(1.5))
            .rounded_full()
            .bg(t.foreground.opacity(0.7));
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
            .id("cast-position")
            .role(Role::Slider)
            .aria_label(t!("Position"))
            .aria_description(SharedString::from(format!(
                "{} / {}",
                clock(self.position),
                clock(duration)
            )))
            .test_support()
            .track_focus(&self.scrub_focus)
            .tab_stop(true)
            .relative()
            .h(space(6.))
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.scrub_focus, cx);
                    this.scrub_to(event.position.x, cx);
                }),
            )
            .on_drag(ScrubDrag, |_, _, _, cx| cx.new(|_| NoChip))
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ScrubDrag>, _, cx| {
                this.scrub_to(event.event.position.x, cx);
            }))
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
}

impl Render for CastView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = col()
            .id("cast-view")
            .size_full()
            .min_h_0()
            .text_start()
            .track_focus(&self.focus);
        match &self.load {
            Load::Waiting => {
                return root
                    .child(waiting(self.started, Some(t!("Reading…")), cx))
                    .into_any_element()
            }
            Load::Failed(note) => {
                return root
                    .child(big_icon(&self.path, Some(note.clone()), cx))
                    .into_any_element()
            }
            Load::Ready => {}
        }
        let t = theme::tokens(cx).clone();
        let duration = self.duration();
        let at_end = self.position >= duration - END_SLACK;
        let play_name = if self.playing { t!("Pause") } else { t!("Play") };
        let play_icon = if self.playing {
            IconName::Pause
        } else if at_end {
            IconName::RotateCcw
        } else {
            IconName::Play
        };
        let title = self.cast.as_ref().map(|cast| cast.title.clone()).unwrap_or_default();
        let note = match (title.is_empty(), self.truncated) {
            (false, true) => Some(format!("{title} · {}", t!("Plays its first 4 MB."))),
            (false, false) => Some(title),
            (true, true) => Some(t!("Plays its first 4 MB.").to_string()),
            (true, false) => None,
        };
        let controls = row_start()
            .id("cast-controls")
            .h(space(10.))
            .flex_none()
            .w_full()
            .gap(space(2.))
            .px(space(2.))
            .items_center()
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(
                Button::new("cast-play")
                    .ghost()
                    .small()
                    .icon(play_icon)
                    .accessibility_label(play_name.clone())
                    .tooltip(play_name)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
            )
            .child(self.scrubber(&t, window, cx))
            .child(
                div()
                    .id("cast-clock")
                    .flex_none()
                    .font_family(theme::mono_font(cx))
                    .child(format!("{} / {}", clock(self.position), clock(duration))),
            )
            .child(
                Button::new("cast-speed")
                    .ghost()
                    .small()
                    .label(format!("{}×", self.speed()))
                    .accessibility_label(t!("Speed"))
                    .tooltip(t!("Speed"))
                    .on_click(cx.listener(|this, _, _, cx| this.cycle_speed(cx))),
            );
        root.on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(|this, _: &crate::actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }))
            .child(
                div()
                    .id("cast-screen")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_scroll()
                    .track_scroll(&self.scroll)
                    .children(self.terminal.clone()),
            )
            .child(controls)
            .children(note.map(|note| {
                div()
                    .id("cast-note")
                    .test_support()
                    .flex_none()
                    .w_full()
                    .truncate()
                    .border_t_1()
                    .border_color(t.border)
                    .px(space(3.))
                    .py(space(1.5))
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(isolate(note))
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};

    /// A recording with colors, Arabic and a pause: events at 0.1, 0.5, 1.0 and 2.0 seconds.
    const CAST: &str = concat!(
        "{\"version\": 2, \"width\": 40, \"height\": 6, \"title\": \"Demo run\"}\n",
        "[0.1, \"o\", \"one\\r\\n\"]\n",
        "[0.5, \"o\", \"\\u001b[31mtwo\\u001b[0m\\r\\n\"]\n",
        "[1.0, \"o\", \"مرحبا\\r\\n\"]\n",
        "[2.0, \"o\", \"four\"]\n",
    );

    fn reader(text: &'static str, truncated: bool) -> CastReader {
        Rc::new(move |_| Box::pin(async move { Ok((text.to_string(), truncated)) }))
    }

    fn open_with(cx: &mut TestAppContext, reader: CastReader) -> (VisualTestContext, Entity<CastView>) {
        cx.update(testing::init);
        testing::set_rtl(false);
        testing::open_window(cx, |window, cx| {
            cx.new(|cx| CastView::with_reader("C:\\demo.cast".into(), reader, window, cx))
        })
    }

    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<CastView>) {
        open_with(cx, reader(CAST, false))
    }

    /// Draws frames and lets the background task come back until `done`.
    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..600 {
            cx.update(|window, cx| window.render_frame(cx));
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_millis(40));
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        panic!("the recording never got there");
    }

    /// Waits until the view has loaded and everything it asked of the screen is done.
    fn ready(cx: &mut VisualTestContext, view: &Entity<CastView>) {
        settle(cx, |cx| {
            cx.update(|_, cx| view.read(cx).ready() && view.read(cx).settled())
        });
    }

    fn rows(cx: &mut VisualTestContext, view: &Entity<CastView>) -> Vec<String> {
        let terminal = cx
            .update(|_, cx| view.read(cx).terminal().cloned())
            .expect("a terminal");
        cx.update(|_, cx| terminal.read(cx).visible_text())
    }

    fn shown(cx: &mut VisualTestContext, view: &Entity<CastView>) -> Vec<String> {
        rows(cx, view).into_iter().filter(|row| !row.is_empty()).collect()
    }

    fn act(cx: &mut VisualTestContext, view: &Entity<CastView>, f: impl FnOnce(&mut CastView, &mut Context<CastView>)) {
        view.update(cx, f);
        settle(cx, |cx| cx.update(|_, cx| view.read(cx).settled()));
    }

    fn label_role(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    #[gpui_kit::test]
    fn it_opens_on_the_last_frame(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        assert_eq!(shown(&mut cx, &view), ["one", "two", "مرحبا", "four"]);
        assert_eq!(
            cx.update(|_, cx| (view.read(cx).position(), view.read(cx).duration())),
            (2., 2.)
        );
        assert!(!cx.update(|_, cx| view.read(cx).is_playing()));
    }

    #[gpui_kit::test]
    fn seeking_replays_from_the_start_forwards_and_backwards(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        act(&mut cx, &view, |view, cx| view.seek(0.6, cx));
        assert_eq!(shown(&mut cx, &view), ["one", "two"]);
        act(&mut cx, &view, |view, cx| view.seek(0., cx));
        assert!(
            shown(&mut cx, &view).is_empty(),
            "before the first output the screen is clear"
        );
        act(&mut cx, &view, |view, cx| view.seek(1.0, cx));
        assert_eq!(
            shown(&mut cx, &view),
            ["one", "two", "مرحبا"],
            "an event at exactly the moment is shown"
        );
        act(&mut cx, &view, |view, cx| view.seek(0.2, cx));
        assert_eq!(shown(&mut cx, &view), ["one"]);
        // Past the ends it clamps.
        act(&mut cx, &view, |view, cx| view.seek(99., cx));
        assert_eq!(cx.update(|_, cx| view.read(cx).position()), 2.);
        assert_eq!(shown(&mut cx, &view).len(), 4);
        act(&mut cx, &view, |view, cx| view.seek(-5., cx));
        assert_eq!(cx.update(|_, cx| view.read(cx).position()), 0.);
    }

    #[gpui_kit::test]
    fn a_burst_of_seeks_ends_on_the_last_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        // Dragging the bar: many seeks before the background task has run any.
        view.update(&mut cx, |view, cx| {
            for step in 0..40 {
                view.seek(f64::from(step) * 0.05, cx);
            }
            view.seek(1.2, cx);
        });
        settle(&mut cx, |cx| cx.update(|_, cx| view.read(cx).settled()));
        assert_eq!(shown(&mut cx, &view), ["one", "two", "مرحبا"]);
    }

    #[gpui_kit::test]
    fn playing_prints_what_was_printed_by_each_moment_and_stops_at_the_end(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        act(&mut cx, &view, |view, cx| view.seek(0., cx));
        act(&mut cx, &view, |view, cx| view.advance_to(0.3, cx));
        assert_eq!(shown(&mut cx, &view), ["one"]);
        act(&mut cx, &view, |view, cx| view.advance_to(1.5, cx));
        assert_eq!(shown(&mut cx, &view), ["one", "two", "مرحبا"]);
        assert_eq!(cx.update(|_, cx| view.read(cx).position()), 1.5);
        view.update(&mut cx, |view, cx| {
            view.play(cx);
            assert!(view.is_playing());
            view.advance_to(2.0, cx);
            assert!(!view.is_playing(), "it stops at the end");
        });
        settle(&mut cx, |cx| cx.update(|_, cx| view.read(cx).settled()));
        assert_eq!(shown(&mut cx, &view).len(), 4);
    }

    #[gpui_kit::test]
    fn play_at_the_end_starts_over_and_the_button_says_what_it_does(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        assert_eq!(label_role(&mut cx, "Play"), Some(Role::Button));
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("cast-play", cx);
        });
        cx.run_until_parked();
        assert!(cx.update(|_, cx| view.read(cx).is_playing()));
        assert!(
            cx.update(|_, cx| view.read(cx).position()) < 0.5,
            "it began again from the start"
        );
        assert_eq!(label_role(&mut cx, "Pause"), Some(Role::Button));
        assert_eq!(label_role(&mut cx, "Play"), None);
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("cast-play", cx);
        });
        assert!(!cx.update(|_, cx| view.read(cx).is_playing()));
        // Paused partway, Play goes on from there.
        act(&mut cx, &view, |view, cx| view.seek(1.0, cx));
        view.update(&mut cx, |view, cx| view.play(cx));
        assert!(cx.update(|_, cx| view.read(cx).position()) >= 1.0);
    }

    #[gpui_kit::test]
    fn the_speed_goes_round_one_two_four(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        let speed = |cx: &mut VisualTestContext| cx.update(|_, cx| view.read(cx).speed());
        assert_eq!(speed(&mut cx), 1.);
        for expected in [2., 4., 1.] {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window.click("cast-speed", cx);
            });
            assert_eq!(speed(&mut cx), expected);
        }
        assert_eq!(label_role(&mut cx, "Speed"), Some(Role::Button));
    }

    #[gpui_kit::test]
    fn the_bar_is_a_named_slider_and_the_arrows_seek(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        assert_eq!(label_role(&mut cx, "Position"), Some(Role::Slider));
        let focus = cx.update(|_, cx| view.read(cx).scrub_focus.clone());
        cx.update(|window, cx| window.focus(&focus, cx));
        act(&mut cx, &view, |view, cx| view.seek(0., cx));
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.press("right", cx);
        });
        assert_eq!(
            cx.update(|_, cx| view.read(cx).position()),
            2.,
            "five seconds on, clamped to the end"
        );
        cx.update(|window, cx| window.press("home", cx));
        assert_eq!(cx.update(|_, cx| view.read(cx).position()), 0.);
        cx.update(|window, cx| window.press("end", cx));
        assert_eq!(cx.update(|_, cx| view.read(cx).position()), 2.);
        cx.update(|window, cx| window.press("left", cx));
        assert_eq!(
            cx.update(|_, cx| view.read(cx).position()),
            0.,
            "five seconds back, clamped to the start"
        );
        cx.update(|window, cx| window.press("space", cx));
        assert!(cx.update(|_, cx| view.read(cx).is_playing()));
        // In Arabic the bar starts at the right, and the arrows swap.
        testing::set_rtl(true);
        act(&mut cx, &view, |view, cx| view.seek(1., cx));
        cx.update(|window, cx| window.press("left", cx));
        assert_eq!(cx.update(|_, cx| view.read(cx).position()), 2.);
        testing::set_rtl(false);
    }

    #[test]
    fn the_bar_counts_from_the_start_edge() {
        assert_eq!(across(110., 10., 200., false), 0.5);
        assert_eq!(across(110., 10., 200., true), 0.5);
        assert!((across(30., 10., 200., false) - 0.1).abs() < 1e-6);
        assert!(
            (across(30., 10., 200., true) - 0.9).abs() < 1e-6,
            "in Arabic the left end is the end"
        );
        assert_eq!(across(-50., 10., 200., false), 0.);
        assert_eq!(across(999., 10., 200., false), 1.);
        assert_eq!(across(5., 10., 0., false), 0.);
    }

    #[gpui_kit::test]
    fn the_title_and_the_four_megabyte_note_are_shown(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open_with(cx, reader(CAST, true));
        ready(&mut cx, &view);
        let note = cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("cast-note").is_some()
        });
        assert!(note, "a title and a truncated file make a note under the bar");
    }

    #[gpui_kit::test]
    fn no_title_and_a_whole_file_make_no_note(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open_with(
            cx,
            reader(
                "{\"version\": 2, \"width\": 20, \"height\": 4}
[0.1, \"o\", \"x\"]
",
                false,
            ),
        );
        ready(&mut cx, &view);
        let none = cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("cast-note").is_none()
        });
        assert!(none);
    }

    #[gpui_kit::test]
    fn a_file_that_is_not_a_recording_says_so_with_its_icon(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open_with(cx, reader("just some text\n", false));
        settle(&mut cx, |cx| cx.update(|_, cx| view.read(cx).failed()));
        let note = "This isn't an asciinema recording Glint can play (version 2 or 3).";
        assert_eq!(label_role(&mut cx, note), Some(Role::Group));
        assert!(cx.update(|_, cx| view.read(cx).terminal().is_none()));
    }

    #[gpui_kit::test]
    fn a_version_glint_cannot_play_is_the_same(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open_with(cx, reader("{\"version\": 1}\n", false));
        settle(&mut cx, |cx| cx.update(|_, cx| view.read(cx).failed()));
    }

    #[gpui_kit::test]
    fn a_file_that_cannot_be_read_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let failing: CastReader = Rc::new(|_| Box::pin(async { Err("Access is denied.".to_string()) }));
        let (mut cx, view) = open_with(cx, failing);
        settle(&mut cx, |cx| cx.update(|_, cx| view.read(cx).failed()));
        assert_eq!(label_role(&mut cx, "Access is denied."), Some(Role::Group));
    }

    #[gpui_kit::test]
    fn a_recording_in_version_three_plays_with_its_relative_times_and_cut_pauses(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        const V3: &str = concat!(
            "{\"version\": 3, \"term\": {\"cols\": 30, \"rows\": 4}, \"idle_time_limit\": 1.0}\n",
            "[0.2, \"o\", \"a\"]\n",
            "[100.0, \"o\", \"b\"]\n",
            "[0.1, \"x\", \"0\"]\n",
            "[0.3, \"o\", \"c\"]\n",
        );
        let (mut cx, view) = open_with(cx, reader(V3, false));
        ready(&mut cx, &view);
        // The long pause is cut to the file's limit: 0.2 + 1.0 + 0.1 + 0.3.
        let duration = cx.update(|_, cx| view.read(cx).duration());
        assert!((duration - 1.6).abs() < 1e-9, "{duration}");
        assert_eq!(rows(&mut cx, &view)[0], "abc");
        act(&mut cx, &view, |view, cx| view.seek(0.5, cx));
        assert_eq!(rows(&mut cx, &view)[0], "a");
        // The terminal is the recording's size: 30 columns, 4 rows.
        let terminal = cx
            .update(|_, cx| view.read(cx).terminal().cloned())
            .expect("a terminal");
        let size = cx.update(|_, cx| terminal.read(cx).screen().with(|emu| emu.size()));
        assert_eq!(size, (30, 4));
    }

    #[gpui_kit::test]
    fn the_recording_is_drawn_in_the_terminal_element_in_the_apps_colors(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        let terminal = cx
            .update(|_, cx| view.read(cx).terminal().cloned())
            .expect("a terminal");
        cx.update(|window, cx| window.render_frame(cx));
        let geometry = cx.update(|_, cx| terminal.read(cx).geometry()).expect("drawn");
        assert_eq!(
            (geometry.cols, geometry.rows),
            (40, 6),
            "the recording's grid, not the pane's"
        );
        let snapshot = cx.update(|_, cx| terminal.read(cx).screen().with(|emu| emu.snapshot()));
        assert!(snapshot.rtl, "the Arabic line is noticed");
        let red = snapshot.rows[1]
            .runs
            .iter()
            .find(|run| run.text == "two")
            .map(|run| run.fg);
        let tokens = cx.update(|_, cx| crate::theme::tokens(cx).clone());
        let palette = crate::components::terminal::palette_of(&tokens);
        assert_eq!(red, Some(palette.ansi[1]), "red of the theme's ANSI set");
        assert!(snapshot.history == 0, "a recording keeps no scrollback");
        // The recording takes no keys: the terminal is named, and not a shell.
        assert_eq!(label_role(&mut cx, "Recording"), Some(Role::Terminal));
    }

    #[gpui_kit::test]
    fn a_new_file_replaces_the_recording(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut cx, view) = open(cx);
        ready(&mut cx, &view);
        cx.update(|window, cx| view.update(cx, |view, cx| view.set_path("C:\\other.cast".into(), window, cx)));
        ready(&mut cx, &view);
        assert_eq!(cx.update(|_, cx| view.read(cx).path().to_string()), "C:\\other.cast");
        assert_eq!(shown(&mut cx, &view).len(), 4, "read again");
    }

    #[test]
    fn a_four_megabyte_recording_replays_in_a_blink() {
        use crate::components::terminal::emu::{Hooks, Screen};
        // 4 MB of colored output in about 30,000 events.
        let mut text = String::from("{\"version\": 2, \"width\": 120, \"height\": 30}\n");
        let line = format!(
            "\\u001b[32mline of output \\u001b[1mbold\\u001b[0m {}\\r\\n",
            "x".repeat(100)
        );
        let mut time = 0.;
        while text.len() < MAX_BYTES {
            time += 0.01;
            text.push_str(&format!("[{time:.3}, \"o\", \"{line}\"]\n"));
        }
        let parsed = Instant::now();
        let cast = parse_cast(&text, DEFAULT_IDLE_SECONDS).expect("a recording");
        let parse = parsed.elapsed();
        let screen = Screen::fixed(cast.cols, cast.rows, Hooks::none());
        let chunks: Vec<&[u8]> = cast.events.iter().map(|event| event.data.as_bytes()).collect();
        let replayed = Instant::now();
        screen.replay(&chunks);
        let replay = replayed.elapsed();
        eprintln!(
            "{} events: parse {parse:?}, replay from the start {replay:?}",
            cast.events.len()
        );
        screen.with(|emu| assert!(emu.visible_text().iter().any(|row| row.starts_with("line of output"))));
        // A debug build parses a few times slower than the release the app ships; either is well inside this.
        assert!(replay < Duration::from_secs(10), "{replay:?}");
    }

    #[test]
    fn the_core_reader_reads_a_file_and_refuses_binary_ones() {
        let dir = std::env::temp_dir().join(format!("glint-cast-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let good = dir.join("a.cast");
        std::fs::write(&good, CAST).expect("a file");
        let bad = dir.join("b.cast");
        std::fs::write(&bad, [0u8, 159, 146, 150, 0, 0, 1, 2, 3, 0, 0, 0, 0]).expect("a file");
        let read = core_reader();
        let (text, truncated) =
            glint_core::host::async_runtime::block_on(read(&good.to_string_lossy())).expect("it reads");
        assert_eq!(text, CAST);
        assert!(!truncated);
        let refused = glint_core::host::async_runtime::block_on(read(&bad.to_string_lossy()));
        assert_eq!(refused, Err(NOT_A_CAST.to_string()));
        let missing = glint_core::host::async_runtime::block_on(read(&dir.join("none.cast").to_string_lossy()));
        assert!(missing.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
