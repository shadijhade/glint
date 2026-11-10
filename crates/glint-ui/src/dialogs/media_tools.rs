//! Media tools for several files (MediaTools.tsx): the Trim bar under a player, and the copy that plays for a
//! video the player can't play as it is.
//!
//! Ported from src/components/explorer/MediaTools.tsx. These are not dialogs but two small views the preview
//! pane places under and in place of a video or recording's player (`FilePreview.tsx` did). The player itself
//! is the preview pane's (`PORT ME(video)` in `components/preview/choose.rs`); these two talk to it with events and
//! calls, so they work with whatever plays. The run uses `dialogs::media::parts` (the Undo offer, the install
//! card), so this file needs the `dialog-media` feature too.
//!
//! | TypeScript                                | Here                                                       |
//! |-------------------------------------------|------------------------------------------------------------|
//! | `TrimBar` (`entry`, `player`, `video`)    | [`TrimBar`] (`path`, `name`, `video`)                      |
//! | the player's `duration` and its events    | [`TrimBar::set_duration`] (the player calls it)            |
//! | `player.currentTime = x`, `play()`        | [`TrimEvent::Seek`], [`TrimEvent::PlayPart`]               |
//! | `stopAtEnd` (the player pauses at the end)| [`TrimBar::stop_at`] (the player pauses there)             |
//! | `CantPlay` (`entry`, `onReady`)           | [`CantPlay`], [`CantPlayEvent::Ready`]                     |
//! | `mediaRun` (trim), `mediaCancel`, `mediaPlayable` | [`Backend`] (a fake in the tests)                  |
//!
//! **How the preview pane uses them.** Under a video or a recording's player: `let bar =
//! media_tools::trim_bar(window, cx, path, name, video)`, subscribe to its [`TrimEvent`]s (`Seek(secs)`: move the
//! player there; `PlayPart { start, end }`: move there and play, and pause at `end`), and call
//! `bar.update(cx, |bar, cx| bar.set_duration(secs, cx))` when the player knows its length (and again when it
//! changes). While `bar.read(cx).stop_at()` is `Some(end)`, pause the player when it passes `end`
//! (`stopAtEnd`). For a video the player can't play: `let card = media_tools::cant_play(window, cx, path, size)`,
//! subscribe to [`CantPlayEvent::Ready`] and play the copy at that path. `media_probe`'s `playable` says which
//! videos need it (the preview pane already asks for it).

use super::media::parts::{self, ToolGate};
use crate::kit::prelude::*;
use crate::{core_bridge, i18n, t, theme};
use glint_core::actions::{Output, Progress, Report};
use glint_core::media_tools::Task;
use glint_model::paths::base_name;
use gpui_kit::{Bounds, FocusHandle, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels};
use parts::Fut;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// How finely a handle moves (`step={0.05}`), in seconds.
const STEP: f64 = 0.05;
/// How close the handles may be (`0.1`).
const GAP: f64 = 0.1;
/// A video this big waits for a click before a playable copy is made (`512 * 1024 * 1024`).
const AUTOMATIC_BELOW: u64 = 512 * 1024 * 1024;

/// `x.toFixed(1)` for a positive number: a tenth, and a tie (1.25, which a double holds exactly) goes up, where
/// Rust's own formatting would go to the even tenth.
fn to_fixed_1(x: f64) -> String {
    let exact = format!("{x:.60}");
    let tie = exact
        .split_once('.')
        .is_some_and(|(_, frac)| frac.as_bytes().get(1) == Some(&b'5') && frac.bytes().skip(2).all(|b| b == b'0'));
    if tie {
        let tenths = (x * 10.).floor() as u64 + 1;
        format!("{}.{}", tenths / 10, tenths % 10)
    } else {
        format!("{x:.1}")
    }
}

/// 12.5 as "0:12.5" and 75.25 as "1:15.3" (`clock` of MediaTools.tsx: minutes, then seconds with a tenth).
pub fn clock(s: f64) -> String {
    let total = s.max(0.);
    let minutes = (total / 60.).floor();
    let seconds = total % 60.;
    format!("{}:{:0>4}", minutes as u64, to_fixed_1(seconds))
}

// ---- What the views ask of the PC ----------------------------------------------------------------------------

/// The calls into glint-core's media studio. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Runs the trim `task` on `path` as `output` says, reporting progress on `progress` (`mediaRun`).
    fn trim(
        &self,
        path: String,
        task: Task,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report>;
    /// A copy of the video that plays, made once and kept: its path (`mediaPlayable`). `progress` gets how far
    /// along it is, 0 to 1.
    fn playable(&self, path: String, progress: glint_core::host::ipc::Channel<f32>) -> Fut<String>;
    /// Stops the run under way (`mediaCancel`).
    fn cancel(&self);
    /// What the install cards ask about tools (none: every tool counts as there).
    fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>>;
}

/// glint-core's media studio, owned by the main window as the shell dialogs it opens are.
pub struct CoreBackend {
    app: glint_core::host::AppHandle,
}

impl CoreBackend {
    /// The backend over core's handle (from `core_bridge::Core`).
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Backend for CoreBackend {
    fn trim(
        &self,
        path: String,
        task: Task,
        output: Output,
        progress: glint_core::host::ipc::Channel<Progress>,
    ) -> Fut<Report> {
        let app = self.app.clone();
        parts::on_core(async move {
            let window = parts::main_window(&app)?;
            glint_core::media_tools::media_run(window, vec![path], task, output, progress).await
        })
    }

    fn playable(&self, path: String, progress: glint_core::host::ipc::Channel<f32>) -> Fut<String> {
        parts::on_core(glint_core::media_tools::media_playable(path, progress))
    }

    fn cancel(&self) {
        glint_core::media_tools::media_cancel();
    }

    fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>> {
        Some(Arc::new(crate::components::settings::CoreBackend::new(
            self.app.clone(),
        )))
    }
}

/// Stands in when core isn't started (never in the app).
struct Unavailable;

impl Backend for Unavailable {
    fn trim(&self, _: String, _: Task, _: Output, _: glint_core::host::ipc::Channel<Progress>) -> Fut<Report> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn playable(&self, _: String, _: glint_core::host::ipc::Channel<f32>) -> Fut<String> {
        Box::pin(async { Err("Glint's core isn't running".to_string()) })
    }
    fn cancel(&self) {}
    fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>> {
        None
    }
}

/// The backend for the app: core's, or a stand-in without core.
fn backend_of(cx: &App) -> Arc<dyn Backend> {
    match cx.try_global::<core_bridge::Core>() {
        Some(core) => Arc::new(CoreBackend::new(core.app.clone())),
        None => Arc::new(Unavailable),
    }
}

// ---- Trim ----------------------------------------------------------------------------------------------------

/// What the Trim bar asks the player to do.
#[derive(Clone, Debug, PartialEq)]
pub enum TrimEvent {
    /// Go to this time, in seconds: a handle moved (`player.currentTime = next[which]`).
    Seek(f64),
    /// Go to `start` and play, pausing at `end` ("Play the part").
    PlayPart {
        /// Where the part starts, in seconds.
        start: f64,
        /// Where it ends.
        end: f64,
    },
}

impl EventEmitter<TrimEvent> for TrimBar {}

/// Which handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Start,
    End,
}

impl Handle {
    /// 0 for the start, 1 for the end.
    pub fn index(self) -> usize {
        match self {
            Handle::Start => 0,
            Handle::End => 1,
        }
    }

    fn label(self) -> SharedString {
        match self {
            Handle::Start => t!("Start"),
            Handle::End => t!("End"),
        }
    }
}

/// Trim under a video or a recording's player: two handles over its length; **Save trim** keeps the part between
/// them in a new file beside it (copied as stored, so at once, or re-encoded to start on the very frame), and
/// Ctrl+Z takes it back.
pub struct TrimBar {
    path: String,
    name: String,
    video: bool,
    open: bool,
    /// The player's length in seconds, 0 until it is known.
    duration: f64,
    /// The part kept: start and end in seconds.
    range: (f64, f64),
    exact: bool,
    /// How far the save is, 0 to 1, while it runs.
    saving: Option<f32>,
    dragging: Option<Handle>,
    /// Where the track was drawn, measured each frame.
    track: Rc<Cell<Bounds<Pixels>>>,
    focus: [FocusHandle; 2],
    backend: Arc<dyn Backend>,
    gate: ToolGate,
    _subscriptions: Vec<Subscription>,
}

impl TrimBar {
    /// A closed bar for the file at `path` (`name` is what notifications call it; `video` is false for a
    /// recording, which has no "Exact"), calling `backend`.
    pub fn new(
        path: String,
        name: String,
        video: bool,
        backend: Arc<dyn Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let gate = ToolGate::new(backend.tools(), window, cx);
        let subscriptions = gate.observe(cx, |_, cx| cx.notify()).into_iter().collect();
        Self {
            path,
            name,
            video,
            open: false,
            duration: 0.,
            range: (0., 0.),
            exact: false,
            saving: None,
            dragging: None,
            track: Rc::new(Cell::new(Bounds::default())),
            focus: [cx.focus_handle(), cx.focus_handle()],
            backend,
            gate,
            _subscriptions: subscriptions,
        }
    }

    /// The player knows its length (`loadedmetadata`, `durationchange`): a finite length above 0 is kept, and the
    /// part is all of it until a handle has moved.
    pub fn set_duration(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if seconds.is_finite() && seconds > 0. {
            self.duration = seconds;
            if self.range.1 == 0. {
                self.range = (0., seconds);
            }
            cx.notify();
        }
    }

    /// Where the player must pause while the bar is open (`stopAtEnd`): the part's end.
    pub fn stop_at(&self) -> Option<f64> {
        self.open.then_some(self.range.1)
    }

    /// Whether the bar is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The part kept: start and end in seconds.
    pub fn range(&self) -> (f64, f64) {
        self.range
    }

    /// How far a save is, 0 to 1, while it runs.
    pub fn saving(&self) -> Option<f32> {
        self.saving
    }

    /// Whether Save trim can be pressed: FFmpeg is there, the length is known, and no save is under way.
    pub fn can_save(&self, cx: &App) -> bool {
        self.saving.is_none() && self.gate.ready("ffmpeg", cx) && self.duration > 0.
    }

    /// Opens the bar (the Trim button).
    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        cx.notify();
    }

    /// Closes the bar (the X).
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.dragging = None;
        cx.notify();
    }

    /// Moves a handle to `value` seconds, keeping them a tenth apart, and asks the player to go there (`move`).
    pub fn move_handle(&mut self, which: Handle, value: f64, cx: &mut Context<Self>) {
        let value = value.clamp(0., self.duration.max(0.));
        let (start, end) = self.range;
        self.range = match which {
            Handle::Start => (value.min(end - GAP).max(0.), end),
            Handle::End => (start, value.max(start + GAP).min(self.duration.max(start + GAP))),
        };
        let at = match which {
            Handle::Start => self.range.0,
            Handle::End => self.range.1,
        };
        cx.emit(TrimEvent::Seek(at));
        cx.notify();
    }

    /// The time under a pointer at window x, in seconds (mirrored in Arabic: the track starts at the right).
    fn time_at(&self, x: Pixels) -> f64 {
        let bounds = self.track.get();
        let width = f64::from(bounds.size.width);
        if width <= 0. {
            return 0.;
        }
        let across = (f64::from(x) - f64::from(bounds.origin.x)) / width;
        let across = across.clamp(0., 1.);
        let fraction = if i18n::is_rtl() { 1. - across } else { across };
        ((fraction * self.duration) / STEP).round() * STEP
    }

    /// The pointer went down on the track: the nearer handle takes it (`<input type=range>` moves the one it is).
    fn grab(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.duration <= 0. {
            return;
        }
        let at = self.time_at(event.position.x);
        let which = if (at - self.range.0).abs() <= (at - self.range.1).abs() {
            Handle::Start
        } else {
            Handle::End
        };
        self.dragging = Some(which);
        window.focus(&self.focus[which.index()], cx);
        self.move_handle(which, at, cx);
    }

    fn drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(which) = self.dragging else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.dragging = None;
            return;
        }
        let at = self.time_at(event.position.x);
        self.move_handle(which, at, cx);
    }

    fn release(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.dragging = None;
    }

    /// The arrows, Home, End and Page keys on a handle: a step, a second, five seconds, the ends.
    fn key(&mut self, which: Handle, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let now = match which {
            Handle::Start => self.range.0,
            Handle::End => self.range.1,
        };
        // The track runs right to left in Arabic, so the left arrow goes later there.
        let toward_start = if i18n::is_rtl() { "right" } else { "left" };
        let toward_end = if i18n::is_rtl() { "left" } else { "right" };
        let small = if event.keystroke.modifiers.shift { 1. } else { STEP };
        let next = match event.keystroke.key.as_str() {
            k if k == toward_start => now - small,
            k if k == toward_end => now + small,
            "down" => now - small,
            "up" => now + small,
            "pagedown" => now - 5.,
            "pageup" => now + 5.,
            "home" => 0.,
            "end" => self.duration,
            _ => return,
        };
        cx.stop_propagation();
        self.move_handle(which, next, cx);
    }

    /// Saves the part beside the file, as a new file (`save`).
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_save(cx) {
            return;
        }
        self.saving = Some(0.);
        cx.notify();
        let task = Task::Trim {
            start: self.range.0,
            end: self.range.1,
            exact: self.exact && self.video,
        };
        let kept = self.range.1 - self.range.0;
        let output = Output::Beside {
            suffix: " (trimmed)".to_string(),
        };
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.trim(self.path.clone(), task, output, channel);
        let name = self.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(mut latest) = receiver.recv().await {
                while let Ok(newer) = receiver.try_recv() {
                    latest = newer;
                }
                let alive = this.update(cx, |this, cx| {
                    if this.saving.is_some() {
                        this.saving = Some(latest.part);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            let result = call.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.saving = None;
                match result {
                    Ok(report) if !report.made.is_empty() => {
                        parts::offer_undo(&report, &t!("trim of “{name}”", name = name), window, cx);
                        let made = report.made[0].clone();
                        window.push_notification(
                            Notification::success(t!("Saved the part, {long} long", long = clock(kept))).action(
                                move |_, _, _| {
                                    let made = made.clone();
                                    Button::new("trim-show").label(t!("Show")).on_click(move |_, _, cx| {
                                        parts::reveal(&made, cx);
                                    })
                                },
                            ),
                            cx,
                        );
                        this.open = false;
                    }
                    Ok(report) => {
                        if let Some((_, why)) = report.failed.first() {
                            window.push_notification(
                                Notification::error(SharedString::from(why.clone())).title(t!("Couldn't trim it")),
                                cx,
                            );
                        }
                    }
                    Err(reason) if reason != "cancelled" => window.push_notification(
                        Notification::error(SharedString::from(reason)).title(t!("Couldn't trim it")),
                        cx,
                    ),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    /// The fractions of the track the part starts and ends at.
    fn fractions(&self) -> (f64, f64) {
        if self.duration > 0. {
            (
                (self.range.0 / self.duration).clamp(0., 1.),
                (self.range.1 / self.duration).clamp(0., 1.),
            )
        } else {
            (0., 1.)
        }
    }
}

impl TrimBar {
    /// The track with its two handles.
    fn track_view(&self, t: &theme::Tokens, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (left, right) = self.fractions();
        let rtl = i18n::is_rtl();
        let measure = self.track.clone();
        let id = cx.entity_id();
        let probe = canvas(
            move |bounds, window, _| {
                let before = measure.replace(bounds);
                if before.size != bounds.size || before.origin != bounds.origin {
                    // Not `cx.notify` here: GPUI drops a redraw asked for during a draw. After this frame it counts.
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
                bounds
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let handle = |which: Handle, at: f64| {
            let focused = self.focus[which.index()].is_focused(window);
            let seconds = clock(match which {
                Handle::Start => self.range.0,
                Handle::End => self.range.1,
            });
            let knob = div()
                .id(("trim-handle", which.index()))
                .role(Role::Slider)
                .aria_label(which.label())
                .aria_description(seconds)
                .test_support()
                .track_focus(&self.focus[which.index()])
                .tab_stop(true)
                .absolute()
                .top(space(1.))
                .size(space(4.))
                .rounded_full()
                .bg(t.primary)
                .border_2()
                .border_color(if focused { t.ring } else { t.background })
                .shadow_sm()
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| this.key(which, event, cx)));
            // A handle's centre is on its time, from the start edge.
            if rtl {
                knob.right(gpui_kit::relative(at as f32)).mr(space(-2.))
            } else {
                knob.left(gpui_kit::relative(at as f32)).ml(space(-2.))
            }
        };
        let bar = div().absolute().top(space(2.25)).h(space(1.5)).rounded_full();
        let selected = if rtl {
            bar.right(gpui_kit::relative(left as f32))
        } else {
            bar.left(gpui_kit::relative(left as f32))
        }
        .w(gpui_kit::relative((right - left).max(0.) as f32))
        .bg(t.primary);
        div()
            .id("trim-track")
            .test_support()
            .relative()
            .h(space(6.))
            .w_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event, window, cx| this.grab(event, window, cx)),
            )
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
            .child(selected)
            .child(handle(Handle::Start, left))
            .child(handle(Handle::End, right))
    }
}

impl Render for TrimBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        if !self.open {
            return div()
                .child(
                    Button::new("trim-open")
                        .ghost()
                        .compact()
                        .icon(IconName::Scissors)
                        .label(t!("Trim"))
                        .tooltip(t!("Cut it to a part, saved as a new file beside it"))
                        .on_click(cx.listener(|this, _, _, cx| this.open(cx))),
                )
                .into_any_element();
        }
        let card = self.gate.card(
            "ffmpeg",
            t!("FFmpeg cuts videos and recordings without re-encoding them."),
            IconName::Film,
            cx,
        );
        let (start, end) = self.range;
        let span: SharedString = t!(
            "{start} – {end} · keeps {kept}",
            start = clock(start),
            end = clock(end),
            kept = clock(end - start)
        );
        let saving = self.saving;
        let can_save = self.can_save(cx);
        col()
            .id("trim-bar")
            .role(Role::Group)
            .aria_label(t!("Trim"))
            .test_support()
            .w_full()
            .max_w(space(144.))
            .gap(space(2.))
            .p(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.background.opacity(0.8))
            .text_size(theme::size::XS)
            .text_start()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.drag(event, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, event, window, cx| this.release(event, window, cx)))
            .children(card)
            .child(self.track_view(&t, window, cx))
            .child(
                align_row(2.)
                    .flex_wrap()
                    .child(
                        div()
                            .id("trim-span")
                            .role(Role::Status)
                            .aria_label(span.clone())
                            .test_support()
                            .text_color(t.muted_foreground)
                            .child(span),
                    )
                    .child(div().flex_1())
                    .when(self.video, |this| {
                        this.child(
                            Checkbox::new("trim-exact")
                                .label(t!("Exact"))
                                .checked(self.exact)
                                .tooltip(t!("Re-encodes the part, so it starts on the very frame (slower); otherwise it's copied as stored and starts at the keyframe at or before the start"))
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.exact = *checked;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Button::new("trim-play")
                            .ghost()
                            .compact()
                            .icon(IconName::Play)
                            .label(t!("Play the part"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let (start, end) = this.range;
                                cx.emit(TrimEvent::PlayPart { start, end });
                            })),
                    )
                    .child(match saving {
                        Some(part) => align_row(1.5)
                            .child(Spinner::new())
                            .child(
                                Button::new("trim-stop")
                                    .outline()
                                    .compact()
                                    .label(t!("{n}% · Stop", n = (part * 100.).round()))
                                    .on_click(cx.listener(|this, _, _, _| this.stop())),
                            )
                            .into_any_element(),
                        None => Button::new("trim-save")
                            .primary()
                            .compact()
                            .icon(IconName::Scissors)
                            .label(t!("Save trim"))
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx)))
                            .into_any_element(),
                    })
                    .child(
                        Button::new("trim-close")
                            .ghost()
                            .compact()
                            .icon(IconName::X)
                            .accessibility_label(t!("Close trim"))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
            .into_any_element()
    }
}

/// A closed Trim bar for the file at `path`, calling core. `name` is what notifications call it; `video` is false
/// for a recording.
pub fn trim_bar(window: &mut Window, cx: &mut App, path: String, name: String, video: bool) -> Entity<TrimBar> {
    let backend = backend_of(cx);
    cx.new(|cx| TrimBar::new(path, name, video, backend, window, cx))
}

// ---- A copy that plays --------------------------------------------------------------------------------------

/// What the card tells the preview pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CantPlayEvent {
    /// The copy is made: play the file at this path (`onReady`, without `convertFileSrc`).
    Ready(String),
}

impl EventEmitter<CantPlayEvent> for CantPlay {}

/// For a video the player can't play (an AVI, a WMV, an MKV of codecs it lacks): FFmpeg makes a copy it can,
/// remuxed as stored when only the container is the problem (seconds), converted otherwise; the copy is kept for
/// next time. A video under 512 MB starts at once; a big one waits for a click, so selecting it doesn't set the PC
/// converting.
pub struct CantPlay {
    path: String,
    automatic: bool,
    started: bool,
    /// How far the copy is, 0 to 1, while it is made.
    part: Option<f32>,
    error: Option<SharedString>,
    backend: Arc<dyn Backend>,
    gate: ToolGate,
    _subscriptions: Vec<Subscription>,
}

impl CantPlay {
    /// The card for the video at `path`, `size` bytes, calling `backend`. It starts when FFmpeg is there.
    pub fn new(
        path: String,
        size: u64,
        backend: Arc<dyn Backend>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let gate = ToolGate::new(backend.tools(), window, cx);
        let subscriptions = gate
            .observe(cx, |this, cx| {
                this.begin(cx);
                cx.notify();
            })
            .into_iter()
            .collect();
        let mut this = Self {
            path,
            automatic: size < AUTOMATIC_BELOW,
            started: false,
            part: None,
            error: None,
            backend,
            gate,
            _subscriptions: subscriptions,
        };
        this.begin(cx);
        this
    }

    /// How far the copy is, 0 to 1, while it is made.
    pub fn part(&self) -> Option<f32> {
        self.part
    }

    /// Why the last try failed, if it did.
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Starts at once when FFmpeg is there and the video isn't big (the TypeScript's effect on `ready`).
    fn begin(&mut self, cx: &mut Context<Self>) {
        if !self.started && self.automatic && self.gate.ready("ffmpeg", cx) {
            self.started = true;
            self.make(cx);
        }
    }

    /// Makes the copy (`make`).
    pub fn make(&mut self, cx: &mut Context<Self>) {
        if self.part.is_some() {
            return;
        }
        self.error = None;
        self.part = Some(0.);
        cx.notify();
        let (channel, receiver) = core_bridge::stream::<f32>();
        let call = self.backend.playable(self.path.clone(), channel);
        cx.spawn(async move |this, cx| {
            while let Ok(mut latest) = receiver.recv().await {
                while let Ok(newer) = receiver.try_recv() {
                    latest = newer;
                }
                let alive = this.update(cx, |this, cx| {
                    if this.part.is_some() {
                        this.part = Some(latest);
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                this.part = None;
                match result {
                    Ok(copy) => cx.emit(CantPlayEvent::Ready(copy)),
                    Err(reason) if reason != "cancelled" => this.error = Some(reason.into()),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }
}

impl Render for CantPlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let ready = self.gate.ready("ffmpeg", cx);
        let card = self.gate.card(
            "ffmpeg",
            t!("FFmpeg makes a copy that plays here, usually in seconds."),
            IconName::Film,
            cx,
        );
        let part = self.part;
        let title = t!("This video doesn't play here as it is");
        col()
            .id("cant-play")
            .role(Role::Group)
            .aria_label(title.clone())
            .test_support()
            .max_w(space(112.))
            .items_center()
            .gap(space(3.))
            .text_center()
            .text_size(theme::size::SM)
            .child(icon(IconName::Film).size_8().text_color(t.muted_foreground))
            .child(div().font_weight(FontWeight::MEDIUM).child(title))
            .children(card)
            .when_some(part, |this, part| {
                let words = t!("Making a copy that plays · {n}%", n = (part * 100.).round());
                this.child(
                    col()
                        .id("cant-play-progress")
                        .role(Role::ProgressIndicator)
                        .aria_label(words.clone())
                        .test_support()
                        .w(space(64.))
                        .gap(space(1.))
                        .child(
                            align_row(2.)
                                .justify_center()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(Spinner::new())
                                .child(words),
                        )
                        .child(
                            row_start()
                                .h(space(1.5))
                                .w_full()
                                .overflow_hidden()
                                .rounded_full()
                                .bg(t.muted)
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui_kit::relative(part.clamp(0., 1.)))
                                        .rounded_full()
                                        .bg(t.primary),
                                ),
                        )
                        .child(
                            Button::new("cant-play-stop")
                                .ghost()
                                .compact()
                                .label(t!("Stop"))
                                .on_click(cx.listener(|this, _, _, _| this.stop())),
                        ),
                )
            })
            .when(
                ready && !self.automatic && part.is_none() && self.error.is_none(),
                |this| {
                    this.child(
                        Button::new("cant-play-make")
                            .primary()
                            .icon(IconName::Film)
                            .label(t!("Make a copy that plays"))
                            .on_click(cx.listener(|this, _, _, cx| this.make(cx))),
                    )
                },
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("cant-play-error")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.destructive)
                        .child(error),
                )
                .when(ready, |this| {
                    this.child(
                        Button::new("cant-play-retry")
                            .outline()
                            .label(t!("Try again"))
                            .on_click(cx.listener(|this, _, _, cx| this.make(cx))),
                    )
                })
            })
    }
}

/// The card for a video the player can't play, calling core; `size` is the file's size in bytes.
pub fn cant_play(window: &mut Window, cx: &mut App, path: String, size: u64) -> Entity<CantPlay> {
    let backend = backend_of(cx);
    cx.new(|cx| CantPlay::new(path, size, backend, window, cx))
}

/// The file's name, for the notifications about it.
pub fn name_of(path: &str) -> String {
    base_name(path).to_string()
}

// ---- Tests ----------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{point, TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend with canned answers; a run waits for `release` when it has one.
    struct Fake {
        report: Report,
        copies: Mutex<Vec<Result<String, String>>>,
        release: Option<async_channel::Receiver<()>>,
        trims: Mutex<Vec<(String, Task, Output)>>,
        made: Mutex<usize>,
        cancelled: Mutex<usize>,
    }

    impl Fake {
        fn new(made: &[&str]) -> Self {
            Self {
                report: Report {
                    made: made.iter().map(|s| (*s).to_string()).collect(),
                    elapsed_ms: 500.,
                    ..Default::default()
                },
                copies: Mutex::new(vec![Ok("C:\\tmp\\copy.mp4".to_string())]),
                release: None,
                trims: Mutex::new(Vec::new()),
                made: Mutex::new(0),
                cancelled: Mutex::new(0),
            }
        }

        fn trims(&self) -> Vec<(String, Task, Output)> {
            self.trims.lock().map(|t| t.clone()).unwrap_or_default()
        }

        fn cancelled(&self) -> usize {
            self.cancelled.lock().map(|n| *n).unwrap_or(0)
        }

        fn made(&self) -> usize {
            self.made.lock().map(|n| *n).unwrap_or(0)
        }
    }

    impl Backend for Fake {
        fn trim(
            &self,
            path: String,
            task: Task,
            output: Output,
            progress: glint_core::host::ipc::Channel<Progress>,
        ) -> Fut<Report> {
            if let Ok(mut trims) = self.trims.lock() {
                trims.push((path, task, output));
            }
            let _ = progress.send(Progress {
                done: 0,
                total: 1,
                current: "clip.mp4".into(),
                part: 0.5,
            });
            let (report, release) = (self.report.clone(), self.release.clone());
            Box::pin(async move {
                if let Some(release) = release {
                    let _ = release.recv().await;
                }
                Ok(report)
            })
        }

        fn playable(&self, _: String, progress: glint_core::host::ipc::Channel<f32>) -> Fut<String> {
            if let Ok(mut n) = self.made.lock() {
                *n += 1;
            }
            let _ = progress.send(0.4);
            let answer = {
                let mut copies = self.copies.lock().unwrap_or_else(|e| e.into_inner());
                if copies.len() > 1 {
                    copies.remove(0)
                } else {
                    copies.first().cloned().unwrap_or_else(|| Err("none".into()))
                }
            };
            let release = self.release.clone();
            Box::pin(async move {
                if let Some(release) = release {
                    let _ = release.recv().await;
                }
                answer
            })
        }

        fn cancel(&self) {
            if let Ok(mut n) = self.cancelled.lock() {
                *n += 1;
            }
        }

        fn tools(&self) -> Option<Arc<dyn crate::components::settings::Backend>> {
            None
        }
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the view never reached the state the test waits for");
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

    fn present(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn disabled(cx: &mut VisualTestContext, id: impl Into<ElementId>) -> bool {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).disabled() == Some(true)
        })
    }

    fn click(cx: &mut VisualTestContext, id: impl Into<ElementId>) {
        let id = id.into();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        cx.run_until_parked();
    }

    fn bar(cx: &mut TestAppContext, fake: &Arc<Fake>, video: bool) -> (VisualTestContext, Entity<TrimBar>) {
        cx.update(testing::init);
        let backend: Arc<dyn Backend> = fake.clone();
        testing::open_window(cx, move |window, cx| {
            cx.new(|cx| TrimBar::new("C:\\v\\clip.mp4".into(), "clip.mp4".into(), video, backend, window, cx))
        })
    }

    /// The trim events the bar sent.
    fn watch(cx: &mut VisualTestContext, bar: &Entity<TrimBar>) -> std::rc::Rc<std::cell::RefCell<Vec<TrimEvent>>> {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = seen.clone();
        cx.update(|_, cx| {
            cx.subscribe(bar, move |_, event: &TrimEvent, _| {
                sink.borrow_mut().push(event.clone())
            })
            .detach()
        });
        seen
    }

    fn description_of(cx: &mut VisualTestContext, handle: usize) -> Option<String> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find(("trim-handle", handle))
                .and_then(|h| h.description().map(str::to_string))
        })
    }

    fn open_bar(cx: &mut VisualTestContext, bar: &Entity<TrimBar>, seconds: f64) {
        bar.update(cx, |b, cx| b.set_duration(seconds, cx));
        click(cx, "trim-open");
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Scissors,
            IconName::Play,
            IconName::X,
            IconName::Film,
            IconName::CircleCheck,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn times_are_shown_with_a_tenth_as_the_typescript_did() {
        assert_eq!(clock(0.), "0:00.0");
        assert_eq!(clock(12.5), "0:12.5");
        assert_eq!(clock(75.2), "1:15.2");
        assert_eq!(clock(600.), "10:00.0");
        assert_eq!(clock(-3.), "0:00.0");
        // toFixed(1) rounds a tie up: 1.25 is held exactly, and Rust's own format would say 1.2.
        assert_eq!(clock(1.25), "0:01.3");
        assert_eq!(clock(0.25), "0:00.3");
        // 0.35 is a hair under, so both say 0.3.
        assert_eq!(clock(0.35), "0:00.3");
        assert_eq!(clock(59.96), "0:60.0", "as toFixed said it");
    }

    #[gpui_kit::test]
    fn trim_opens_to_two_handles_that_the_keyboard_moves_and_the_player_follows(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Arc::new(Fake::new(&[]));
        let (mut visual, bar) = bar(cx, &fake, true);
        let seen = watch(&mut visual, &bar);
        assert!(present(&mut visual, "trim-open") && !present(&mut visual, "trim-track"));
        assert_eq!(bar.read_with(&visual, |b, _| b.stop_at()), None);
        open_bar(&mut visual, &bar, 100.);
        assert_eq!(role_of(&mut visual, "Start"), Some(Role::Slider));
        assert_eq!(role_of(&mut visual, "End"), Some(Role::Slider));
        assert_eq!(description_of(&mut visual, 0).as_deref(), Some("0:00.0"));
        assert_eq!(description_of(&mut visual, 1).as_deref(), Some("1:40.0"));
        assert!(has(&mut visual, "0:00.0 – 1:40.0 · keeps 1:40.0"));
        // The player pauses at the part's end while the bar is open.
        assert_eq!(bar.read_with(&visual, |b, _| b.stop_at()), Some(100.));
        // A step, a second with Shift, and Home and End.
        visual.update(|window, cx| {
            let handle = bar.read(cx).focus[0].clone();
            handle.focus(window, cx);
            window.press("right", cx);
            window.press("shift-right", cx);
        });
        assert!((bar.read_with(&visual, |b, _| b.range().0) - 1.05).abs() < 1e-9);
        assert_eq!(seen.borrow().len(), 2);
        assert!(matches!(seen.borrow()[1], TrimEvent::Seek(at) if (at - 1.05).abs() < 1e-9));
        visual.update(|window, cx| {
            let handle = bar.read(cx).focus[1].clone();
            handle.focus(window, cx);
            window.press("home", cx);
        });
        // The handles stay a tenth apart.
        let (start, end) = bar.read_with(&visual, |b, _| b.range());
        assert!((end - start - GAP).abs() < 1e-9, "{start} {end}");
        visual.update(|window, cx| {
            window.press("end", cx);
            window.press("pagedown", cx);
        });
        assert!((bar.read_with(&visual, |b, _| b.range().1) - 95.).abs() < 1e-9);
        // Play the part; then close.
        click(&mut visual, "trim-play");
        assert!(matches!(seen.borrow().last(), Some(TrimEvent::PlayPart { start, end }) if *start > 1. && *end == 95.));
        click(&mut visual, "trim-close");
        assert!(!present(&mut visual, "trim-track") && present(&mut visual, "trim-open"));
        assert_eq!(bar.read_with(&visual, |b, _| b.stop_at()), None);
    }

    #[gpui_kit::test]
    fn clicking_the_track_moves_the_nearer_handle_and_arabic_mirrors_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Arc::new(Fake::new(&[]));
        let (mut visual, bar) = bar(cx, &fake, true);
        open_bar(&mut visual, &bar, 100.);
        let width = visual.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find("trim-track").bounds().size.width)
        });
        assert!(width > 100.);
        // A quarter of the way: the start is nearer than the end, and takes it.
        visual.update(|window, cx| window.click_at("trim-track", point(px(width * 0.25), px(8.)), cx));
        visual.run_until_parked();
        let (start, end) = bar.read_with(&visual, |b, _| b.range());
        assert!((start - 25.).abs() < 0.5, "{start}");
        assert_eq!(end, 100.);
        // Three quarters: the end is nearer.
        visual.update(|window, cx| window.click_at("trim-track", point(px(width * 0.75), px(8.)), cx));
        visual.run_until_parked();
        let (start2, end2) = bar.read_with(&visual, |b, _| b.range());
        assert_eq!(start2, start);
        assert!((end2 - 75.).abs() < 0.5, "{end2}");

        // In Arabic the track starts at the right: the same click is the other end of the video, and the left
        // arrow goes later.
        testing::set_rtl(true);
        visual.update(|_, cx| bar.update(cx, |_, cx| cx.notify()));
        bar.update(&mut visual, |b, cx| {
            b.range = (0., 100.);
            cx.notify();
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.click_at("trim-track", point(px(width * 0.1), px(8.)), cx));
        visual.run_until_parked();
        let (start3, end3) = bar.read_with(&visual, |b, _| b.range());
        assert_eq!(start3, 0.);
        assert!((end3 - 90.).abs() < 0.5, "{end3}");
        visual.update(|window, cx| {
            let handle = bar.read(cx).focus[0].clone();
            handle.focus(window, cx);
            window.press("left", cx);
        });
        assert!((bar.read_with(&visual, |b, _| b.range().0) - STEP).abs() < 1e-9);
        assert_eq!(role_of(&mut visual, "Start"), Some(Role::Slider));
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn save_trim_keeps_the_part_beside_the_file_and_closes_the_bar(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Arc::new(Fake::new(&["C:\\v\\clip (trimmed).mp4"]));
        let (mut visual, bar) = bar(cx, &fake, true);
        // Before the player knows its length there is nothing to save.
        click(&mut visual, "trim-open");
        assert!(disabled(&mut visual, "trim-save"));
        bar.update(&mut visual, |b, cx| b.set_duration(f64::NAN, cx));
        bar.update(&mut visual, |b, cx| b.set_duration(0., cx));
        assert!(disabled(&mut visual, "trim-save"), "a length that isn't one is ignored");
        bar.update(&mut visual, |b, cx| b.set_duration(100., cx));
        assert!(!disabled(&mut visual, "trim-save"));
        // A later length keeps the part chosen.
        visual.update(|window, cx| {
            let handle = bar.read(cx).focus[1].clone();
            handle.focus(window, cx);
            window.press("pagedown", cx);
        });
        bar.update(&mut visual, |b, cx| b.set_duration(101., cx));
        assert_eq!(bar.read_with(&visual, |b, _| b.range()), (0., 95.));
        click(&mut visual, "trim-exact");
        click(&mut visual, "trim-save");
        settle(&mut visual, |_| !fake.trims().is_empty());
        let trims = fake.trims();
        assert_eq!(trims[0].0, "C:\\v\\clip.mp4");
        assert_eq!(
            trims[0].1,
            Task::Trim {
                start: 0.,
                end: 95.,
                exact: true
            }
        );
        assert_eq!(
            trims[0].2,
            Output::Beside {
                suffix: " (trimmed)".into()
            }
        );
        settle(&mut visual, |cx| bar.read_with(cx, |b, _| !b.is_open()));
        assert_eq!(bar.read_with(&visual, |b, _| b.saving()), None);
    }

    #[gpui_kit::test]
    fn a_recording_has_no_exact_and_ffmpeg_missing_blocks_saving(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Arc::new(Fake::new(&["C:\\v\\talk (trimmed).wav"]));
        let (mut visual, bar) = bar(cx, &fake, false);
        open_bar(&mut visual, &bar, 30.);
        assert!(!present(&mut visual, "trim-exact"));
        bar.update(&mut visual, |b, _| b.exact = true);
        click(&mut visual, "trim-save");
        settle(&mut visual, |_| !fake.trims().is_empty());
        assert!(matches!(fake.trims()[0].1, Task::Trim { exact: false, .. }));

        // FFmpeg not known to be there: nothing saves.
        let fake = Arc::new(Fake::new(&[]));
        let (mut visual, bar) = self::bar(cx, &fake, true);
        open_bar(&mut visual, &bar, 30.);
        bar.update(&mut visual, |b, cx| {
            b.gate = ToolGate::never();
            cx.notify();
        });
        assert!(disabled(&mut visual, "trim-save"));
        visual.update(|window, cx| bar.update(cx, |b, cx| b.save(window, cx)));
        assert!(fake.trims().is_empty());
    }

    #[gpui_kit::test]
    fn saving_shows_how_far_it_is_and_stop_cancels(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (release, held) = async_channel::bounded(1);
        let mut fake = Fake::new(&["C:\\v\\clip (trimmed).mp4"]);
        fake.release = Some(held);
        let fake = Arc::new(fake);
        let (mut visual, bar) = bar(cx, &fake, true);
        open_bar(&mut visual, &bar, 60.);
        click(&mut visual, "trim-save");
        settle(&mut visual, |cx| present(cx, "trim-stop"));
        assert!(has(&mut visual, "50% · Stop"));
        assert!(!present(&mut visual, "trim-save"));
        assert_eq!(bar.read_with(&visual, |b, _| b.saving()), Some(0.5));
        click(&mut visual, "trim-stop");
        assert_eq!(fake.cancelled(), 1);
        release.try_send(()).ok();
        settle(&mut visual, |cx| bar.read_with(cx, |b, _| b.saving().is_none()));
    }

    // ---- A copy that plays ----

    fn card(cx: &mut TestAppContext, fake: &Arc<Fake>, size: u64) -> (VisualTestContext, Entity<CantPlay>) {
        cx.update(testing::init);
        let backend: Arc<dyn Backend> = fake.clone();
        testing::open_window(cx, move |window, cx| {
            cx.new(|cx| CantPlay::new("C:\\v\\old.avi".into(), size, backend, window, cx))
        })
    }

    fn ready_events(
        cx: &mut VisualTestContext,
        card: &Entity<CantPlay>,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<String>>> {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let sink = seen.clone();
        cx.update(|_, cx| {
            cx.subscribe(card, move |_, event: &CantPlayEvent, _| {
                let CantPlayEvent::Ready(path) = event;
                sink.borrow_mut().push(path.clone());
            })
            .detach()
        });
        seen
    }

    #[gpui_kit::test]
    fn a_small_video_gets_its_copy_at_once_and_says_how_far_it_is(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (release, held) = async_channel::bounded(1);
        let mut fake = Fake::new(&[]);
        fake.release = Some(held);
        let fake = Arc::new(fake);
        let (mut visual, card) = card(cx, &fake, 10 * 1024 * 1024);
        let ready = ready_events(&mut visual, &card);
        assert_eq!(
            role_of(&mut visual, "This video doesn't play here as it is"),
            Some(Role::Group)
        );
        settle(&mut visual, |cx| has(cx, "Making a copy that plays · 40%"));
        assert_eq!(
            role_of(&mut visual, "Making a copy that plays · 40%"),
            Some(Role::ProgressIndicator)
        );
        assert_eq!(fake.made(), 1);
        assert!(!present(&mut visual, "cant-play-make"), "it started by itself");
        click(&mut visual, "cant-play-stop");
        assert_eq!(fake.cancelled(), 1);
        release.try_send(()).ok();
        settle(&mut visual, |_| !ready.borrow().is_empty());
        assert_eq!(*ready.borrow(), ["C:\\tmp\\copy.mp4"]);
        assert_eq!(card.read_with(&visual, |c, _| c.part()), None);
    }

    #[gpui_kit::test]
    fn a_big_video_waits_for_a_click_and_a_failure_can_be_tried_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Arc::new(Fake::new(&[]));
        if let Ok(mut copies) = fake.copies.lock() {
            *copies = vec![Err("Not enough room".to_string()), Ok("C:\\tmp\\copy.mp4".to_string())];
        }
        let (mut visual, card) = card(cx, &fake, 600 * 1024 * 1024);
        let ready = ready_events(&mut visual, &card);
        visual.run_until_parked();
        assert_eq!(fake.made(), 0, "a big video doesn't set the PC converting by itself");
        click(&mut visual, "cant-play-make");
        settle(&mut visual, |cx| has(cx, "Not enough room"));
        assert_eq!(role_of(&mut visual, "Not enough room"), Some(Role::Alert));
        assert!(ready.borrow().is_empty());
        assert!(!present(&mut visual, "cant-play-make") && present(&mut visual, "cant-play-retry"));
        click(&mut visual, "cant-play-retry");
        settle(&mut visual, |_| !ready.borrow().is_empty());
        assert_eq!(fake.made(), 2);
        assert!(!has(&mut visual, "Not enough room"));
    }

    #[gpui_kit::test]
    fn a_stopped_copy_is_not_an_error_and_arabic_works(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Arc::new(Fake::new(&[]));
        if let Ok(mut copies) = fake.copies.lock() {
            *copies = vec![Err("cancelled".to_string())];
        }
        let (mut visual, card) = card(cx, &fake, 1024);
        let ready = ready_events(&mut visual, &card);
        settle(&mut visual, |_| fake.made() == 1);
        visual.run_until_parked();
        assert!(ready.borrow().is_empty());
        assert_eq!(card.read_with(&visual, |c, _| c.error().cloned()), None);
        assert!(!present(&mut visual, "cant-play-error"));

        testing::set_rtl(true);
        visual.update(|_, cx| card.update(cx, |_, cx| cx.notify()));
        assert_eq!(
            role_of(&mut visual, "This video doesn't play here as it is"),
            Some(Role::Group)
        );
        testing::set_rtl(false);
    }
}
