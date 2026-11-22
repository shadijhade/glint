//! 120 FPS OKLab Color-Wheel Photo Surfing bar.
//!
//! Renders an interactive perceptual OKLab chroma wheel and lightness/tolerance controls above the
//! pane when toggled open, re-ranking matching photos in real time without touching disk or AI.

use crate::explorer::{explorer, Explorer, ExplorerEvent, TabId};
use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use glint_model::oklab::Palette3;
use glint_model::oklab::{
    oklab_to_srgb, wheel_point_to_oklab, wheel_raster_bgra, ChordMode, ColorFilter, Oklab, WHEEL_MAX_CHROMA,
};
use glint_model::paths::same_path;
use gpui_kit::{
    linear_color_stop, linear_gradient, Background, DispatchPhase, KeyDownEvent, RenderImage, ScrollDelta, ScrollEvent,
};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

struct GlobalState(Entity<ColorWheelModel>);
impl Global for GlobalState {}

/// Tracks which tabs currently have the Color Wheel bar open.
pub struct ColorWheelModel {
    shown: HashSet<TabId>,
    warmed_folder: HashMap<TabId, String>,
    /// Which color of the chord the wheel and the lightness slider are set on, per tab (0 is the first).
    selected: HashMap<TabId, usize>,
    _sub: Subscription,
}

impl ColorWheelModel {
    fn new(cx: &mut Context<Self>) -> Self {
        let exp = explorer(cx);
        let sub = cx.subscribe(&exp, |this, exp, ev: &ExplorerEvent, cx| match ev {
            ExplorerEvent::TabClosed { tab, .. } => {
                let removed = this.shown.remove(tab);
                let dropped = this.warmed_folder.remove(tab).is_some();
                this.selected.remove(tab);
                if removed || dropped {
                    cx.notify();
                }
            }
            ExplorerEvent::TabChanged(tab) if this.shown.contains(tab) => {
                let Some(path) = exp.read(cx).tab(*tab).map(|t| t.path.clone()) else {
                    return;
                };
                if this.warmed_folder.get(tab).is_none_or(|prev| !same_path(prev, &path)) {
                    let tab_id = *tab;
                    this.warmed_folder.insert(tab_id, path);
                    // Defer to avoid re-entrantly updating `Explorer` from inside its own notification.
                    cx.defer(move |cx| explorer(cx).update(cx, |e, cx| e.warm_color_palettes(tab_id, cx)));
                }
            }
            _ => {}
        });
        Self {
            shown: HashSet::new(),
            warmed_folder: HashMap::new(),
            selected: HashMap::new(),
            _sub: sub,
        }
    }

    pub fn is_shown(&self, tab: TabId) -> bool {
        self.shown.contains(&tab)
    }

    /// The color of `tab`'s chord that the wheel sets (0 is the first).
    pub fn selected(&self, tab: TabId) -> usize {
        self.selected.get(&tab).copied().unwrap_or(0)
    }

    /// Selects the `index`th color of `tab`'s chord.
    pub fn select(&mut self, tab: TabId, index: usize, cx: &mut Context<Self>) {
        if self.selected(tab) != index {
            self.selected.insert(tab, index);
            cx.notify();
        }
    }

    pub fn set_shown(&mut self, tab: TabId, on: bool, folder: Option<String>, cx: &mut Context<Self>) {
        let changed = if on {
            if let Some(path) = folder {
                self.warmed_folder.insert(tab, path);
            }
            self.shown.insert(tab)
        } else {
            let a = self.shown.remove(&tab);
            let b = self.warmed_folder.remove(&tab).is_some();
            self.selected.remove(&tab);
            a || b
        };
        if changed {
            cx.notify();
        }
    }
}

/// Returns the shared [`ColorWheelModel`], initializing it on demand.
pub fn shared(cx: &mut App) -> Entity<ColorWheelModel> {
    if let Some(state) = cx.try_global::<GlobalState>() {
        return state.0.clone();
    }
    let entity = cx.new(ColorWheelModel::new);
    cx.set_global(GlobalState(entity.clone()));
    entity
}

/// Returns whether the Color Wheel bar is open for `tab`.
pub fn is_shown(cx: &App, tab: TabId) -> bool {
    cx.try_global::<GlobalState>()
        .is_some_and(|s| s.0.read(cx).is_shown(tab))
}

/// Toggles the Color Wheel bar for `tab`, warming image palettes when opened and clearing the
/// filter when closed.
pub fn toggle(cx: &mut App, tab: TabId) {
    let model = shared(cx);
    let next = !model.read(cx).is_shown(tab);
    let exp = explorer(cx);
    let folder = exp.read(cx).tab(tab).map(|t| t.path.clone());
    model.update(cx, |m, cx| m.set_shown(tab, next, folder, cx));
    if next {
        exp.update(cx, |e, cx| e.warm_color_palettes(tab, cx));
    } else {
        exp.update(cx, |e, cx| {
            e.set_color_filter(None, Some(tab), cx);
            // The grid's tiles drop their palette dots with the wheel, whether or not a filter was set.
            cx.notify();
        });
    }
}

/// Opens the Color Wheel bar for `tab` locked onto `target` OKLab color (from "Find similar colors").
pub fn open_with(cx: &mut App, tab: TabId, target: Oklab) {
    let model = shared(cx);
    let exp = explorer(cx);
    let folder = exp.read(cx).tab(tab).map(|t| t.path.clone());
    model.update(cx, |m, cx| m.set_shown(tab, true, folder, cx));
    exp.update(cx, |e, cx| {
        e.set_color_filter(Some(ColorFilter::new(target)), Some(tab), cx);
    });
}

/// What the tiles of `tab`'s grid need to show their palette dots: set while the Color Wheel is open on the tab and
/// some photos' palettes are known. The dots only ever read palettes that are already there; nothing is decoded
/// for a tile the pointer passes over.
#[derive(Clone)]
pub struct Dots {
    pub tab: TabId,
    pub palettes: Arc<HashMap<String, Palette3>>,
}

/// The dots' context for `tab`, if its Color Wheel is open and has palettes.
pub fn dots_of(cx: &App, tab: TabId) -> Option<Dots> {
    if !is_shown(cx, tab) {
        return None;
    }
    let palettes = explorer(cx).read(cx).tab(tab)?.color_palettes.clone();
    (!palettes.is_empty()).then_some(Dots { tab, palettes })
}

/// The palette of the photo at `path`, if it is in `palettes` (keyed by lowercase path).
pub fn palette_for<'a>(palettes: &'a HashMap<String, Palette3>, path: &str) -> Option<&'a Palette3> {
    thread_local! {
        static KEY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }
    KEY.with(|key| palettes.get(crate::explorer::palette_key(path, &mut key.borrow_mut())))
}

/// Sets the wheel's color on `tab` to exactly `color` (a palette dot on a tile was pressed): the color of the chord
/// the wheel is set on, or a new filter when nothing was picked yet.
pub fn pick(cx: &mut App, tab: TabId, color: Oklab) {
    let exp = explorer(cx);
    let selected = shared(cx).read(cx).selected(tab);
    let next = match exp.read(cx).tab(tab).and_then(|t| t.color_filter) {
        Some(filter) => filter.with_target_at(selected.min(filter.count() - 1), color),
        None => ColorFilter::new(color),
    };
    exp.update(cx, |e, cx| e.set_color_filter(Some(next), Some(tab), cx));
}

/// Closes the Color Wheel bar when closing a tab.
pub fn close_tab(cx: &mut App, tab: TabId) {
    if let Some(state) = cx.try_global::<GlobalState>() {
        let model = state.0.clone();
        model.update(cx, |m, cx| m.set_shown(tab, false, None, cx));
    }
}

#[inline]
pub(crate) fn srgb_hsla(rgb: [u8; 3]) -> Hsla {
    gpui_kit::Rgba {
        r: f32::from(rgb[0]) / 255.0,
        g: f32::from(rgb[1]) / 255.0,
        b: f32::from(rgb[2]) / 255.0,
        a: 1.0,
    }
    .into()
}

/// What the pointer is dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Wheel,
    Lightness,
    Tolerance,
}

/// The lightness slider's ends: the wheel's own limits (`wheel_point_to_oklab` clamps to them).
const L_MIN: f32 = 0.15;
const L_MAX: f32 = 0.95;
/// The tolerance slider's ends: the tightest match worth having, and one that takes in most of a folder.
const TOL_MIN: f32 = 0.05;
const TOL_MAX: f32 = 0.50;
/// What an arrow key moves a slider by, and Page Up / Page Down (a tenth of the way).
const KEY_STEP: f32 = 0.01;
/// What one line of a mouse wheel moves the lightness by (a notch is three lines) and the tolerance by.
const WHEEL_L_STEP: f32 = 0.01;
const WHEEL_TOL_STEP: f32 = 0.005;

impl Drag {
    /// The slider's range, or none for the wheel.
    fn range(self) -> Option<(f32, f32)> {
        match self {
            Drag::Wheel => None,
            Drag::Lightness => Some((L_MIN, L_MAX)),
            Drag::Tolerance => Some((TOL_MIN, TOL_MAX)),
        }
    }
}

/// How far a wheel event turned, in lines (a notch is three): whichever way it came, since Windows turns Shift
/// plus the wheel into a sideways scroll.
fn wheel_lines(delta: &ScrollDelta) -> f32 {
    let (x, y) = match delta {
        ScrollDelta::Lines(lines) => (lines.x, lines.y),
        ScrollDelta::Pixels(pixels) => (f32::from(pixels.x) / 20.0, f32::from(pixels.y) / 20.0),
    };
    if y.abs() >= x.abs() {
        y
    } else {
        x
    }
}

/// Compact per-pane OKLab Color Wheel toolbar rendered above the pane listing while active.
pub struct ColorWheelBar {
    pane: usize,
    explorer: Entity<Explorer>,
    model: Entity<ColorWheelModel>,
    lightness: f32,
    tolerance: f32,
    /// What the pointer is dragging, shared with the window-level listeners that follow it outside the controls.
    drag: Rc<Cell<Option<Drag>>>,
    wheel_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Where each slider's track was drawn, and their keyboard focus.
    l_track: Rc<Cell<Bounds<Pixels>>>,
    t_track: Rc<Cell<Bounds<Pixels>>>,
    l_focus: FocusHandle,
    t_focus: FocusHandle,
    /// The filter the controls last asked for, ranked at the next frame: a drag moves many times a frame and the
    /// photos are ranked once for all of them.
    pending: Option<ColorFilter>,
    queued: bool,
    /// How many times the controls have made the explorer rank the photos (a test reads it).
    ranked: u32,
    /// The wheel's disk as one picture, for the lightness and size it was made at.
    disk: Option<Disk>,
    /// How many times the disk has been rasterized (a test reads it: it must not go up on every frame).
    rasters: u32,
    _subs: [Subscription; 2],
}

/// The wheel's disk rasterized once: `side` pixels square at lightness `step` hundredths.
struct Disk {
    side: u32,
    step: u16,
    image: Arc<RenderImage>,
}

/// The wheel's size on screen, in logical pixels.
const WHEEL_PX: f32 = 112.0;

/// The most pixels the disk is rasterized at: the wheel is small, and this is plenty for a 2x screen.
const DISK_MAX_SIDE: u32 = 256;

/// The disk's side in device pixels for a window at `scale`.
fn disk_side(scale: f32) -> u32 {
    ((WHEEL_PX * scale).round() as u32).clamp(WHEEL_PX as u32, DISK_MAX_SIDE)
}

/// The lightness the disk is drawn at, in hundredths: the slider moves by that much, so a drag never makes more
/// pictures than it has positions.
fn disk_step(lightness: f32) -> u16 {
    (lightness.clamp(0.0, 1.0) * 100.0).round() as u16
}

/// The disk at `side` pixels and lightness `step` hundredths as a picture GPUI can draw in one go.
fn rasterize_disk(side: u32, step: u16) -> Option<Arc<RenderImage>> {
    let bgra = wheel_raster_bgra(side, f32::from(step) / 100.0);
    image::RgbaImage::from_raw(side, side, bgra)
        .map(|buffer| Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// A slider's value to the hundredth, kept in its range: what its keys, wheel and track give.
fn settle(value: f32, range: (f32, f32)) -> f32 {
    ((value * 100.0).round() / 100.0).clamp(range.0, range.1)
}

impl ColorWheelBar {
    pub fn new(pane: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let exp = explorer(cx);
        let model = shared(cx);
        let sub_m = cx.observe_in(&model, window, |_, _, _, cx| cx.notify());
        let sub_e = cx.observe_in(&exp, window, |_, _, _, cx| cx.notify());
        Self {
            pane,
            explorer: exp,
            model,
            lightness: 0.62,
            tolerance: ColorFilter::DEFAULT_TOLERANCE,
            drag: Rc::new(Cell::new(None)),
            wheel_bounds: Rc::new(Cell::new(Bounds::default())),
            l_track: Rc::new(Cell::new(Bounds::default())),
            t_track: Rc::new(Cell::new(Bounds::default())),
            l_focus: cx.focus_handle(),
            t_focus: cx.focus_handle(),
            pending: None,
            queued: false,
            ranked: 0,
            disk: None,
            rasters: 0,
            _subs: [sub_m, sub_e],
        }
    }

    /// The keyboard focus of a slider (the tests focus it).
    #[cfg(test)]
    fn focus_of(&self, which: Drag) -> FocusHandle {
        match which {
            Drag::Tolerance => self.t_focus.clone(),
            _ => self.l_focus.clone(),
        }
    }

    fn tab_id(&self, cx: &App) -> TabId {
        self.explorer.read(cx).pane_tab_id(self.pane)
    }

    /// The filter as the controls have it: what they last asked for, which the explorer has one frame to catch up
    /// with, or else the tab's.
    fn filter(&self, cx: &App) -> Option<ColorFilter> {
        self.pending.or_else(|| {
            let tab = self.tab_id(cx);
            self.explorer.read(cx).tab(tab).and_then(|t| t.color_filter)
        })
    }

    /// Asks for `filter`: the controls show it at once, and the photos are ranked for it at the next frame, once,
    /// whatever number of times the pointer moved in between.
    fn ask(&mut self, filter: ColorFilter, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(color) = filter.target_at(self.selected(cx)) {
            self.lightness = color.l;
        }
        self.tolerance = filter.max_dist;
        self.pending = Some(filter);
        if !self.queued {
            self.queued = true;
            let me = cx.entity();
            window.on_next_frame(move |_, cx| me.update(cx, |this, cx| this.rank(cx)));
        }
        cx.notify();
    }

    /// Hands the filter asked for to the explorer, which ranks the photos for it.
    fn rank(&mut self, cx: &mut Context<Self>) {
        self.queued = false;
        let Some(filter) = self.pending.take() else {
            return;
        };
        self.ranked += 1;
        let tab = self.tab_id(cx);
        self.explorer
            .update(cx, |e, cx| e.set_color_filter(Some(filter), Some(tab), cx));
    }

    /// Moves a slider to `value` (kept in its range, to the hundredth). With no color picked yet the value is
    /// only remembered, for the wheel to be drawn at and the first click to pick with.
    fn set_value(&mut self, which: Drag, value: f32, window: &mut Window, cx: &mut Context<Self>) {
        let Some(range) = which.range() else {
            return;
        };
        let value = settle(value, range);
        match self.filter(cx) {
            Some(mut filter) => {
                match which {
                    Drag::Lightness => {
                        // The lightness of the color the wheel is set on.
                        let at = self.selected(cx);
                        if let Some(color) = filter.target_at(at) {
                            filter = filter.with_target_at(at, Oklab { l: value, ..color });
                        }
                    }
                    _ => filter.max_dist = value,
                }
                self.ask(filter, window, cx);
            }
            None => {
                match which {
                    Drag::Lightness => self.lightness = value,
                    _ => self.tolerance = value,
                }
                cx.notify();
            }
        }
    }

    /// The value a slider has now.
    fn value_of(&self, which: Drag, cx: &App) -> f32 {
        let filter = self.filter(cx);
        match which {
            Drag::Lightness => filter
                .and_then(|f| f.target_at(self.selected(cx)))
                .map_or(self.lightness, |color| color.l),
            _ => filter.map_or(self.tolerance, |f| f.max_dist),
        }
    }

    /// The color of the chord the wheel and the lightness slider are set on.
    fn selected(&self, cx: &App) -> usize {
        let tab = self.tab_id(cx);
        let last = self.filter(cx).map_or(0, |f| f.count() - 1);
        self.model.read(cx).selected(tab).min(last)
    }

    /// Sets the wheel on the `index`th color of the chord.
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let tab = self.tab_id(cx);
        self.model.update(cx, |m, cx| m.select(tab, index, cx));
    }

    /// Adds a color to the chord (up to three) and sets the wheel on it: at first the opposite of the one the wheel
    /// is on, which is the other end of a pair that goes well together.
    fn add_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(filter) = self.filter(cx).filter(|f| f.count() < ColorFilter::MAX_TARGETS) else {
            return;
        };
        let from = filter.target_at(self.selected(cx)).unwrap_or(filter.target);
        // A gray has no opposite: a blue-green does.
        let (a, b) = if from.a.abs() + from.b.abs() < 0.04 {
            (-0.10, -0.03)
        } else {
            (-from.a, -from.b)
        };
        let next = filter.with_added(Oklab { l: from.l, a, b });
        self.select(next.count() - 1, cx);
        self.ask(next, window, cx);
    }

    /// Takes the `index`th color out of the chord; the wheel stays on the same color, or on the next one if it was
    /// that one.
    fn remove_target(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(filter) = self.filter(cx) else {
            return;
        };
        let at = self.selected(cx);
        let Some(next) = filter.without(index) else {
            return;
        };
        let at = if index < at { at - 1 } else { at.min(next.count() - 1) };
        self.select(at, cx);
        self.ask(next, window, cx);
    }

    /// Whether a photo must match any of the colors or all of them.
    fn set_mode(&mut self, mode: ChordMode, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(filter) = self.filter(cx).filter(|f| f.mode != mode) {
            self.ask(ColorFilter { mode, ..filter }, window, cx);
        }
    }

    /// The pointer is at `pos` while dragging `which`: the wheel picks the color there, a slider the value there.
    fn pointer(&mut self, which: Drag, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        match which {
            Drag::Wheel => {
                let bounds = self.wheel_bounds.get();
                let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                if w <= 1.0 || h <= 1.0 {
                    return;
                }
                let lx = (f32::from(pos.x - bounds.origin.x) / w) * 2.0 - 1.0;
                let ly = (f32::from(pos.y - bounds.origin.y) / h) * 2.0 - 1.0;
                let lightness = self.value_of(Drag::Lightness, cx);
                let target = wheel_point_to_oklab(lx, ly, lightness);
                let filter = match self.filter(cx) {
                    Some(prev) => prev.with_target_at(self.selected(cx), target),
                    None => ColorFilter {
                        max_dist: self.tolerance,
                        ..ColorFilter::new(target)
                    },
                };
                self.ask(filter, window, cx);
            }
            slider => {
                let (Some(range), track) = (
                    slider.range(),
                    if slider == Drag::Lightness {
                        self.l_track.get()
                    } else {
                        self.t_track.get()
                    },
                ) else {
                    return;
                };
                let width = f32::from(track.size.width);
                if width <= 1.0 {
                    return;
                }
                let mut along = f32::from(pos.x - track.origin.x) / width;
                if i18n::is_rtl() {
                    along = 1.0 - along;
                }
                self.set_value(
                    slider,
                    range.0 + along.clamp(0.0, 1.0) * (range.1 - range.0),
                    window,
                    cx,
                );
            }
        }
    }

    /// A key on a slider: the arrows by a hundredth, Page Up and Page Down by a tenth of the way, Home and End to
    /// the ends. The track starts at the reading direction's start, so Left and Right swap in Arabic.
    fn slider_key(&mut self, which: Drag, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((lo, hi)) = which.range() else {
            return;
        };
        let (more, less) = if i18n::is_rtl() {
            ("left", "right")
        } else {
            ("right", "left")
        };
        let now = self.value_of(which, cx);
        let next = match event.keystroke.key.as_str() {
            k if k == more => now + KEY_STEP,
            k if k == less => now - KEY_STEP,
            "up" => now + KEY_STEP,
            "down" => now - KEY_STEP,
            "pageup" => now + (hi - lo) / 10.0,
            "pagedown" => now - (hi - lo) / 10.0,
            "home" => lo,
            "end" => hi,
            _ => return,
        };
        cx.stop_propagation();
        self.set_value(which, next, window, cx);
    }

    /// The mouse wheel over the color wheel: lightness, or with Shift the tolerance (wheel up for more).
    fn wheeled(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let lines = wheel_lines(&event.delta);
        if lines == 0.0 || !lines.is_finite() {
            return;
        }
        let (which, per_line) = if event.modifiers.shift {
            (Drag::Tolerance, WHEEL_TOL_STEP)
        } else {
            (Drag::Lightness, WHEEL_L_STEP)
        };
        let now = self.value_of(which, cx);
        self.set_value(which, now + lines * per_line, window, cx);
        cx.stop_propagation();
    }
}

/// A slider as the bar draws it: a gradient track, a thumb on it, a name and a value for the accessibility tree.
struct Slider {
    id: &'static str,
    pane: usize,
    name: SharedString,
    value: SharedString,
    /// The slider's ends and where it is, for assistive technology.
    range: (f32, f32),
    current: f32,
    /// Where the thumb is along the track, 0 to 1 from the reading direction's start.
    at: f32,
    track: Rc<Cell<Bounds<Pixels>>>,
    focus: FocusHandle,
    gradient: Background,
    thumb: Hsla,
}

impl Slider {
    fn element(
        self,
        window: &Window,
        entity: EntityId,
        t: &theme::Tokens,
        on_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
        on_key: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let rtl = i18n::is_rtl();
        let focused = self.focus.is_focused(window);
        let measure = self.track.clone();
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
        let track = div()
            .absolute()
            .left_0()
            .right_0()
            .top(space(2.25))
            .h(space(1.5))
            .rounded_full()
            .border_1()
            .border_color(t.border)
            .bg(self.gradient);
        let knob = div()
            .absolute()
            .top(space(1.))
            .size(space(4.))
            .rounded_full()
            .bg(self.thumb)
            .border_2()
            .border_color(if focused { t.ring } else { t.background })
            .shadow_sm();
        let knob = if rtl {
            knob.right(gpui_kit::relative(self.at)).mr(space(-2.))
        } else {
            knob.left(gpui_kit::relative(self.at)).ml(space(-2.))
        };
        div()
            .id((self.id, self.pane))
            .role(Role::Slider)
            .aria_label(self.name)
            .aria_value(self.value)
            .aria_min_numeric_value(f64::from(self.range.0))
            .aria_max_numeric_value(f64::from(self.range.1))
            .aria_numeric_value(f64::from(self.current))
            .aria_numeric_value_step(f64::from(KEY_STEP))
            .test_support()
            .track_focus(&self.focus)
            .tab_stop(true)
            .relative()
            .h(space(6.))
            .flex_1()
            .min_w(space(24.))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, on_down)
            .on_key_down(on_key)
            .child(probe)
            .child(track)
            .child(knob)
    }
}

impl Render for ColorWheelBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab_id(cx);
        if !self.model.read(cx).is_shown(tab) {
            return div().into_any_element();
        }
        let (matched_count, palette_count, total_files) = {
            let exp = self.explorer.read(cx);
            let Some(t_state) = exp.tab(tab) else {
                return div().into_any_element();
            };
            if t_state.is_this_pc() {
                return div().into_any_element();
            }
            let files = t_state.entries().iter().filter(|e| !e.dir).count();
            (t_state.rows.len(), t_state.color_palettes.len(), files)
        };
        let active_filter = self.filter(cx);

        let t = theme::tokens(cx);
        let active_lightness = active_filter.map_or(self.lightness, |f| f.target.l);
        let active_tol = active_filter.map_or(self.tolerance, |f| f.max_dist);
        let selected = self.selected(cx);
        let chord: Vec<Oklab> = active_filter.map(|f| f.targets().collect()).unwrap_or_default();
        let swatch = chord.get(selected).map(|c| srgb_hsla(oklab_to_srgb(*c)));
        let wheel_bounds = self.wheel_bounds.clone();
        // Where each color of the chord is on the wheel, and its color; the one the wheel is set on is drawn last.
        let mut markers: Vec<(f32, f32, Hsla, bool)> = chord
            .iter()
            .enumerate()
            .map(|(i, c)| (c.a, c.b, srgb_hsla(oklab_to_srgb(*c)), i == selected))
            .collect();
        markers.sort_by_key(|m| m.3);

        // The disk is one picture, made again only when the lightness (to the hundredth) or the window's scale
        // changes, never per frame.
        let side = disk_side(window.scale_factor());
        let step = disk_step(active_lightness);
        if self.disk.as_ref().is_none_or(|d| d.side != side || d.step != step) {
            if let Some(image) = rasterize_disk(side, step) {
                self.rasters += 1;
                if let Some(old) = self.disk.replace(Disk { side, step, image }) {
                    let _ = window.drop_image(old.image);
                }
            }
        }
        let disk = self.disk.as_ref().map(|d| d.image.clone());

        // Window-level listeners that follow a drag outside the control it began on; they do nothing otherwise.
        let (view, drag) = (cx.entity().downgrade(), self.drag.clone());
        let follow = canvas(
            |bounds, _, _| bounds,
            move |_, _, window, _| {
                let (moved, moving) = (view.clone(), drag.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let Some(which) = moving.get() else {
                        return;
                    };
                    if event.pressed_button != Some(MouseButton::Left) {
                        moving.set(None);
                        return;
                    }
                    moved
                        .update(cx, |this, cx| this.pointer(which, event.position, window, cx))
                        .ok();
                });
                let released = drag.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, _| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        released.set(None);
                    }
                });
            },
        )
        .absolute()
        .inset_0();

        let wheel_canvas = canvas(
            move |bounds, _, _| {
                wheel_bounds.set(bounds);
                bounds
            },
            move |_, bounds, window, _| {
                let side = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
                if side <= 2.0 {
                    return;
                }
                let square = Bounds::new(bounds.origin, size(px(side), px(side)));
                if let Some(image) = &disk {
                    let _ = window.paint_image(square, square, Corners::default(), image.clone(), 0, false);
                }
                for &(a, b, color, current) in &markers {
                    let nx = (a / WHEEL_MAX_CHROMA).clamp(-1.0, 1.0);
                    let ny = (-b / WHEEL_MAX_CHROMA).clamp(-1.0, 1.0);
                    let cx_px = f32::from(bounds.origin.x) + (nx * 0.5 + 0.5) * side;
                    let cy_px = f32::from(bounds.origin.y) + (ny * 0.5 + 0.5) * side;
                    // The color the wheel is set on: a white ring in a dark one, so it reads on pale colors too.
                    // The others of the chord: smaller, filled with their own color.
                    let rings: &[(f32, f32, Hsla, Hsla)] = if current {
                        &[
                            (7.0, 1.0, gpui_kit::black().opacity(0.45), gpui_kit::transparent_black()),
                            (6.0, 2.0, gpui_kit::white(), gpui_kit::transparent_black()),
                        ]
                    } else {
                        &[(5.0, 2.0, gpui_kit::white(), color)]
                    };
                    for &(r, width, edge, fill_color) in rings {
                        let ring = Bounds::new(point(px(cx_px - r), px(cy_px - r)), size(px(r * 2.0), px(r * 2.0)));
                        window.paint_quad(quad(
                            ring,
                            r,
                            fill_color,
                            Edges::all(px(width)),
                            edge,
                            BorderStyle::Solid,
                        ));
                    }
                }
            },
        )
        .size(px(WHEEL_PX));

        let wheel = div()
            .id(("cw-wheel", self.pane))
            .role(Role::Slider)
            .aria_label(t!("OKLab color wheel"))
            .aria_description(t!(
                "Click or drag to pick a color; the mouse wheel changes lightness, Shift and the wheel the tolerance"
            ))
            .test_support()
            .size(px(WHEEL_PX))
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.drag.set(Some(Drag::Wheel));
                    this.pointer(Drag::Wheel, ev.position, window, cx);
                }),
            )
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| this.wheeled(ev, window, cx)))
            .child(wheel_canvas);

        // The lightness track runs from dark to light in the picked color's own hue; the tolerance track from the
        // color itself to a faint wash of it. Both run from the reading direction's start.
        let (hue_a, hue_b) = chord.get(selected).map_or((0.0, 0.0), |c| (c.a, c.b));
        let tone = |l: f32| srgb_hsla(oklab_to_srgb(Oklab { l, a: hue_a, b: hue_b }));
        let angle = if i18n::is_rtl() { 270. } else { 90. };
        let picked = swatch.unwrap_or(t.primary);
        let lightness_slider = Slider {
            id: "cw-lightness",
            pane: self.pane,
            name: t!("Lightness"),
            value: SharedString::from(format!("{}%", (active_lightness * 100.0).round())),
            range: (L_MIN, L_MAX),
            current: active_lightness,
            at: ((active_lightness - L_MIN) / (L_MAX - L_MIN)).clamp(0.0, 1.0),
            track: self.l_track.clone(),
            focus: self.l_focus.clone(),
            gradient: linear_gradient(
                angle,
                linear_color_stop(tone(L_MIN), 0.),
                linear_color_stop(tone(L_MAX), 1.),
            ),
            thumb: tone(active_lightness),
        };
        let tolerance_slider = Slider {
            id: "cw-tolerance",
            pane: self.pane,
            name: t!("Tolerance"),
            value: SharedString::from(format!("{}%", (active_tol * 100.0).round())),
            range: (TOL_MIN, TOL_MAX),
            current: active_tol,
            at: ((active_tol - TOL_MIN) / (TOL_MAX - TOL_MIN)).clamp(0.0, 1.0),
            track: self.t_track.clone(),
            focus: self.t_focus.clone(),
            gradient: linear_gradient(
                angle,
                linear_color_stop(picked, 0.),
                linear_color_stop(picked.opacity(0.12), 1.),
            ),
            thumb: t.primary,
        };
        let entity = cx.entity_id();
        let slider_row = |label: SharedString, value: SharedString, slider: AnyElement| {
            row_start()
                .items_center()
                .gap(space(2.))
                .child(
                    div()
                        .w(space(18.))
                        .flex_none()
                        .text_xs()
                        .text_color(t.muted_foreground)
                        .child(label),
                )
                .child(slider)
                .child(
                    div()
                        .w(space(10.))
                        .flex_none()
                        .text_xs()
                        .text_color(t.muted_foreground)
                        .child(value),
                )
        };
        let lightness_row = slider_row(
            t!("Lightness"),
            lightness_slider.value.clone(),
            lightness_slider
                .element(
                    window,
                    entity,
                    t,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        window.focus(&this.l_focus, cx);
                        this.drag.set(Some(Drag::Lightness));
                        this.pointer(Drag::Lightness, ev.position, window, cx);
                    }),
                    cx.listener(|this, ev: &KeyDownEvent, window, cx| this.slider_key(Drag::Lightness, ev, window, cx)),
                )
                .into_any_element(),
        );
        let tolerance_row = slider_row(
            t!("Tolerance"),
            tolerance_slider.value.clone(),
            tolerance_slider
                .element(
                    window,
                    entity,
                    t,
                    cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                        window.focus(&this.t_focus, cx);
                        this.drag.set(Some(Drag::Tolerance));
                        this.pointer(Drag::Tolerance, ev.position, window, cx);
                    }),
                    cx.listener(|this, ev: &KeyDownEvent, window, cx| this.slider_key(Drag::Tolerance, ev, window, cx)),
                )
                .into_any_element(),
        );

        // The chord: a swatch for each color (press one to set the wheel on it, the cross takes it out), a plus for
        // another color, and whether a photo needs any of them or all.
        let count = chord.len();
        let mode = active_filter.map_or(ChordMode::Any, |f| f.mode);
        let pane = self.pane;
        let swatches = chord.iter().enumerate().map(|(i, color)| {
            let rgb = oklab_to_srgb(*color);
            let current = i == selected;
            let name = t!(
                "Color {n}: #{hex}",
                n = i + 1,
                hex = format!("{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
            );
            let swatch = div()
                .id(("cw-swatch", pane * ColorFilter::MAX_TARGETS + i))
                .role(Role::Button)
                .aria_label(name)
                .aria_selected(current)
                .test_support()
                .size(space(5.))
                .flex_none()
                .rounded(theme::size::RADIUS_SM)
                .border_2()
                .border_color(if current { t.foreground } else { t.border })
                .bg(srgb_hsla(rgb))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.select(i, cx)));
            row_start().items_center().child(swatch).when(count > 1, |r| {
                r.child(
                    Button::new(("cw-remove", pane * ColorFilter::MAX_TARGETS + i))
                        .compact()
                        .ghost()
                        .icon(Icon::new(IconName::X))
                        .accessibility_label(t!("Remove color {n}", n = i + 1))
                        .tooltip(t!("Remove color {n}", n = i + 1))
                        .on_click(cx.listener(move |this, _, window, cx| this.remove_target(i, window, cx))),
                )
            })
        });
        let chord_row = row_start()
            .items_center()
            .gap(space(1.5))
            .children(swatches)
            .when(active_filter.is_some() && count < ColorFilter::MAX_TARGETS, |r| {
                r.child(
                    Button::new(("cw-add", pane))
                        .compact()
                        .ghost()
                        .icon(Icon::new(IconName::Plus))
                        .accessibility_label(t!("Add a color"))
                        .tooltip(t!("Add a color: photos can match a combination of colors"))
                        .on_click(cx.listener(|this, _, window, cx| this.add_target(window, cx))),
                )
            })
            .when(count > 1, |r| {
                r.child(
                    row_start()
                        .items_center()
                        .gap(space(0.5))
                        .child(
                            Button::new(("cw-any", pane))
                                .compact()
                                .when(mode == ChordMode::Any, |b| b.primary())
                                .when(mode != ChordMode::Any, |b| b.ghost())
                                .label(t!("Any"))
                                .tooltip(t!("Photos that have at least one of the colors"))
                                .on_click(cx.listener(|this, _, window, cx| this.set_mode(ChordMode::Any, window, cx))),
                        )
                        .child(
                            Button::new(("cw-all", pane))
                                .compact()
                                .when(mode == ChordMode::All, |b| b.primary())
                                .when(mode != ChordMode::All, |b| b.ghost())
                                .label(t!("All"))
                                .tooltip(t!("Photos that have every one of the colors"))
                                .on_click(cx.listener(|this, _, window, cx| this.set_mode(ChordMode::All, window, cx))),
                        ),
                )
            });

        let status = if active_filter.is_some() {
            t!("{n} matching photos", n = matched_count)
        } else {
            t!(
                "Click or drag on the wheel ({indexed}/{total} palettes ready)",
                indexed = palette_count,
                total = total_files
            )
        };

        row_start()
            .id(("color-wheel-bar", self.pane))
            .relative()
            .w_full()
            .items_center()
            .gap(space(4.))
            .px(space(3.))
            .py(space(2.))
            .bg(t.background)
            .border_b_1()
            .border_color(t.border)
            .child(follow)
            .child(wheel)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .gap(space(1.))
                    .child(
                        row_start()
                            .items_center()
                            .justify_between()
                            .gap(space(2.))
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(2.))
                                    .when_some(swatch, |r, fill_color| {
                                        r.child(
                                            div()
                                                .size(px(14.))
                                                .rounded(theme::size::RADIUS_SM)
                                                .border_1()
                                                .border_color(t.border)
                                                .bg(fill_color),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.foreground)
                                            .child(t!("Color Wheel")),
                                    )
                                    .child(div().text_xs().text_color(t.muted_foreground).child(status)),
                            )
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(1.))
                                    .when(active_filter.is_some(), |r| {
                                        r.child(
                                            Button::new(("cw-clear", self.pane))
                                                .compact()
                                                .ghost()
                                                .label(t!("Clear"))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.pending = None;
                                                    this.explorer
                                                        .update(cx, |e, cx| e.set_color_filter(None, Some(tab), cx));
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new(("cw-close", self.pane))
                                            .compact()
                                            .ghost()
                                            .icon(Icon::new(IconName::X))
                                            .accessibility_label(t!("Close Color Wheel"))
                                            .tooltip(t!("Close Color Wheel"))
                                            .on_click(move |_, _, cx| toggle(cx, tab)),
                                    ),
                            ),
                    )
                    .child(chord_row)
                    .child(lightness_row)
                    .child(tolerance_row),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, FakeDisk};
    use crate::explorer::NavOptions;
    use crate::testing;
    use glint_model::oklab::srgb_to_oklab;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};

    #[gpui_kit::test]
    fn toggle_and_open_with_track_tab_state_and_filter(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Photos", vec![file("C:\\Photos", "sky.jpg", 120, 0)]);

        cx.update(|cx| {
            testing::init(cx);
            crate::explorer::init_with(cx, disk.source());
        });
        let (_view, visual) = cx.add_window_view(|window, cx| ColorWheelBar::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Photos", NavOptions::default(), cx));
        });
        visual.run_until_parked();

        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        assert!(!visual.update(|_, cx| is_shown(cx, tab)));

        visual.update(|_, cx| toggle(cx, tab));
        assert!(visual.update(|_, cx| is_shown(cx, tab)));

        let cyan = srgb_to_oklab(0, 210, 230);
        visual.update(|_, cx| open_with(cx, tab, cyan));
        assert!(visual.update(|_, cx| is_shown(cx, tab)));
        let filter = visual.update(|_, cx| explorer(cx).read(cx).tab(tab).and_then(|t| t.color_filter));
        assert!(filter.is_some());

        visual.update(|_, cx| toggle(cx, tab));
        assert!(!visual.update(|_, cx| is_shown(cx, tab)));
        let cleared = visual.update(|_, cx| explorer(cx).read(cx).tab(tab).and_then(|t| t.color_filter));
        assert!(cleared.is_none());
    }

    /// The disk is one picture, made when the wheel first shows and again only when the lightness (to the
    /// hundredth) or the window's scale changes: frames in between draw the picture they have. Prints what a
    /// disk costs: `cargo test -p glint-ui --lib color_wheel::tests::the_disk -- --nocapture`.
    #[gpui_kit::test]
    fn the_disk_is_rasterized_once_and_again_only_when_the_lightness_changes(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Photos", vec![file("C:\\Photos", "sky.jpg", 120, 0)]);
        cx.update(|cx| {
            testing::init(cx);
            crate::explorer::init_with(cx, disk.source());
        });
        let (view, visual) = cx.add_window_view(|window, cx| ColorWheelBar::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Photos", NavOptions::default(), cx));
        });
        visual.run_until_parked();
        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        let rasters = |visual: &mut gpui_kit::VisualTestContext| visual.update(|_, cx| view.read(cx).rasters);
        let frames = |visual: &mut gpui_kit::VisualTestContext, n: usize| {
            for _ in 0..n {
                visual.update(|window, cx| {
                    window.refresh();
                    window.render_frame(cx);
                });
            }
        };

        assert_eq!(rasters(visual), 0, "a closed wheel draws nothing");
        visual.update(|_, cx| toggle(cx, tab));
        frames(visual, 30);
        assert_eq!(rasters(visual), 1, "thirty frames, one disk");

        visual.update(|window, cx| view.update(cx, |bar, cx| bar.set_value(Drag::Lightness, 0.40, window, cx)));
        frames(visual, 30);
        assert_eq!(rasters(visual), 2, "a new lightness makes a new disk, once");
        // The same hundredth again is the same disk.
        visual.update(|window, cx| view.update(cx, |bar, cx| bar.set_value(Drag::Lightness, 0.4001, window, cx)));
        frames(visual, 5);
        assert_eq!(rasters(visual), 2);

        let best = |side: u32| {
            (0..5)
                .map(|n| {
                    let started = std::time::Instant::now();
                    std::hint::black_box(rasterize_disk(side, 50 + n));
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::MAX, f64::min)
        };
        println!(
            "wheel disk: {} px {:.2} ms, {} px {:.2} ms (once per lightness step); 0.15 painted 576 quads every frame",
            WHEEL_PX as u32,
            best(WHEEL_PX as u32),
            DISK_MAX_SIDE,
            best(DISK_MAX_SIDE)
        );
    }

    /// A window with the bar over a folder with one photo, the wheel open, and a color picked.
    fn bar_with_a_color(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (Entity<ColorWheelBar>, &mut gpui_kit::VisualTestContext, TabId) {
        let disk = FakeDisk::new();
        disk.folder("C:\\Photos", vec![file("C:\\Photos", "sky.jpg", 120, 0)]);
        cx.update(|cx| {
            testing::init(cx);
            crate::explorer::init_with(cx, disk.source());
        });
        let (view, visual) = cx.add_window_view(|window, cx| ColorWheelBar::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Photos", NavOptions::default(), cx));
        });
        visual.run_until_parked();
        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        visual.update(|_, cx| open_with(cx, tab, srgb_to_oklab(220, 120, 40)));
        visual.update(|window, cx| window.render_frame(cx));
        (view, visual, tab)
    }

    fn filter_of(visual: &mut gpui_kit::VisualTestContext, tab: TabId) -> ColorFilter {
        visual.update(|_, cx| {
            explorer(cx)
                .read(cx)
                .tab(tab)
                .and_then(|t| t.color_filter)
                .expect("a filter")
        })
    }

    #[gpui_kit::test]
    fn the_sliders_are_named_and_a_drag_ranks_the_photos_once_a_frame(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let (view, visual, tab) = bar_with_a_color(cx);
        let role_of = |visual: &mut gpui_kit::VisualTestContext, label: &str| {
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.try_find_by_label(label).and_then(|e| e.role())
            })
        };
        assert_eq!(role_of(visual, "Lightness"), Some(Role::Slider));
        assert_eq!(role_of(visual, "Tolerance"), Some(Role::Slider));

        // Fifty moves inside one frame make one ranking.
        let before = visual.update(|_, cx| view.read(cx).ranked);
        visual.update(|window, cx| {
            view.update(cx, |bar, cx| {
                for n in 0..50 {
                    bar.set_value(Drag::Lightness, 0.30 + n as f32 * 0.01, window, cx);
                }
                assert_eq!(bar.ranked, before, "nothing is ranked until the frame");
                bar.rank(cx);
                bar.rank(cx);
            })
        });
        assert_eq!(visual.update(|_, cx| view.read(cx).ranked), before + 1);
        let filter = filter_of(visual, tab);
        assert!(
            (filter.target.l - 0.79).abs() < 1e-4,
            "the last move wins: {}",
            filter.target.l
        );
        // Only the lightness moved: the color's hue and the tolerance are as they were.
        let first = srgb_to_oklab(220, 120, 40);
        assert!(
            (filter.target.a - first.a).abs() < 1e-6 && (filter.max_dist - ColorFilter::DEFAULT_TOLERANCE).abs() < 1e-6
        );

        // Past the ends is the end.
        visual.update(|window, cx| {
            view.update(cx, |bar, cx| {
                bar.set_value(Drag::Lightness, 9.0, window, cx);
                bar.set_value(Drag::Tolerance, -1.0, window, cx);
                bar.rank(cx);
            })
        });
        let filter = filter_of(visual, tab);
        assert_eq!((filter.target.l, filter.max_dist), (L_MAX, TOL_MIN));
    }

    #[gpui_kit::test]
    fn a_slider_answers_the_keyboard_and_a_press_on_its_track(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        for rtl in [false, true] {
            let (view, visual, tab) = bar_with_a_color(cx);
            testing::set_rtl(rtl);
            visual.update(|window, cx| window.render_frame(cx));
            let (more, less) = if rtl { ("left", "right") } else { ("right", "left") };
            let focus = |visual: &mut gpui_kit::VisualTestContext, which: Drag| {
                visual.update(|window, cx| {
                    let handle = view.read(cx).focus_of(which);
                    window.focus(&handle, cx);
                });
            };
            let rank = |visual: &mut gpui_kit::VisualTestContext| {
                visual.update(|_, cx| view.update(cx, |bar, cx| bar.rank(cx)));
            };
            focus(visual, Drag::Lightness);
            visual.simulate_keystrokes("home");
            rank(visual);
            assert_eq!(filter_of(visual, tab).target.l, L_MIN, "Home");
            visual.simulate_keystrokes(more);
            rank(visual);
            assert!(
                (filter_of(visual, tab).target.l - 0.16).abs() < 1e-4,
                "an arrow is a hundredth"
            );
            visual.simulate_keystrokes("pageup");
            rank(visual);
            assert!(
                (filter_of(visual, tab).target.l - 0.24).abs() < 1e-4,
                "Page Up is a tenth of the way"
            );
            visual.simulate_keystrokes(less);
            visual.simulate_keystrokes("down");
            rank(visual);
            assert!((filter_of(visual, tab).target.l - 0.22).abs() < 1e-4);
            visual.simulate_keystrokes("end");
            rank(visual);
            assert_eq!(filter_of(visual, tab).target.l, L_MAX, "End");

            focus(visual, Drag::Tolerance);
            visual.simulate_keystrokes("home");
            rank(visual);
            assert_eq!(filter_of(visual, tab).max_dist, TOL_MIN);
            visual.simulate_keystrokes("end");
            rank(visual);
            assert_eq!(filter_of(visual, tab).max_dist, TOL_MAX);

            // A press a quarter of the way along the lightness track, from the reading direction's start.
            visual.update(|window, cx| window.render_frame(cx));
            let track = visual.update(|_, cx| view.read(cx).l_track.get());
            let along = if rtl { 0.75 } else { 0.25 };
            let at = point(
                track.origin.x + track.size.width * along,
                track.origin.y + track.size.height / 2.0,
            );
            visual.simulate_mouse_down(at, MouseButton::Left, gpui_kit::Modifiers::default());
            visual.simulate_mouse_up(at, MouseButton::Left, gpui_kit::Modifiers::default());
            rank(visual);
            let l = filter_of(visual, tab).target.l;
            assert!(
                (l - (L_MIN + 0.25 * (L_MAX - L_MIN))).abs() < 0.011,
                "a quarter along: {l}"
            );
        }
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn the_wheel_changes_lightness_and_with_shift_the_tolerance(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let (view, visual, tab) = bar_with_a_color(cx);
        let before = filter_of(visual, tab);
        let wheel = visual.update(|window, cx| {
            window.render_frame(cx);
            window.find(("cw-wheel", 0usize)).bounds()
        });
        let at = wheel.center();
        let scroll = |visual: &mut gpui_kit::VisualTestContext, lines: f32, shift: bool| {
            visual.simulate_event(gpui_kit::ScrollEvent {
                position: at,
                delta: gpui_kit::ScrollDelta::Lines(point(0.0, lines)),
                modifiers: gpui_kit::Modifiers {
                    shift,
                    ..Default::default()
                },
                touch_phase: gpui_kit::TouchPhase::Moved,
                source: gpui_kit::ScrollSource::Pointer,
            });
            visual.update(|_, cx| view.update(cx, |bar, cx| bar.rank(cx)));
        };
        scroll(visual, 3.0, false);
        let after = filter_of(visual, tab);
        assert!(
            (after.target.l - (before.target.l + 0.03)).abs() < 0.011,
            "a notch up is lighter"
        );
        assert_eq!(after.max_dist, before.max_dist);
        scroll(visual, -3.0, true);
        let after = filter_of(visual, tab);
        assert!(
            (after.max_dist - (before.max_dist - 0.015)).abs() < 0.011,
            "Shift: a notch down is tighter"
        );
    }

    #[gpui_kit::test]
    fn a_chord_has_swatches_you_select_and_remove_and_any_all(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let (view, visual, tab) = bar_with_a_color(cx);
        let found = |visual: &mut gpui_kit::VisualTestContext, label: &str| {
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.try_find_by_label(label).is_some()
            })
        };
        let first = filter_of(visual, tab).target;
        let rgb = oklab_to_srgb(first);
        let first_name = format!("Color 1: #{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
        assert!(found(visual, &first_name));
        assert!(found(visual, "Add a color"));
        assert!(
            !found(visual, "Remove color 1"),
            "a single color has nothing to take out"
        );
        assert!(!found(visual, "Any"), "and nothing to combine");

        // Adding a color selects it; the wheel then sets that one and leaves the first alone.
        visual.update(|window, cx| {
            view.update(cx, |bar, cx| {
                bar.add_target(window, cx);
                bar.rank(cx);
            })
        });
        let chord = filter_of(visual, tab);
        assert_eq!(chord.count(), 2);
        assert_eq!(chord.target, first);
        assert!(
            chord.extra[0].is_some_and(|c| (c.a + first.a).abs() < 1e-6),
            "the opposite color"
        );
        assert_eq!(
            visual.update(|_, cx| view.read(cx).selected(cx)),
            1,
            "the new one is selected"
        );
        let rgb = oklab_to_srgb(chord.extra[0].expect("a second color"));
        let second_name = format!("Color 2: #{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
        for label in [
            second_name.as_str(),
            "Remove color 1",
            "Remove color 2",
            "Any",
            "All",
            "Add a color",
        ] {
            assert!(found(visual, label), "{label}");
        }

        let wheel = visual.update(|window, cx| {
            window.render_frame(cx);
            window.find(("cw-wheel", 0usize)).bounds()
        });
        let at = point(
            wheel.origin.x + wheel.size.width * 0.8,
            wheel.origin.y + wheel.size.height * 0.5,
        );
        visual.simulate_mouse_down(at, MouseButton::Left, gpui_kit::Modifiers::default());
        visual.simulate_mouse_up(at, MouseButton::Left, gpui_kit::Modifiers::default());
        visual.update(|_, cx| view.update(cx, |bar, cx| bar.rank(cx)));
        let moved = filter_of(visual, tab);
        assert_eq!(moved.target, first, "the first color stays where it was");
        assert!(
            moved.extra[0].is_some_and(|c| c.a > 0.1),
            "the selected one went to the click"
        );

        // Select the first again: the wheel moves that one.
        visual.update(|_, cx| view.update(cx, |bar, cx| bar.select(0, cx)));
        visual.simulate_mouse_down(wheel.center(), MouseButton::Left, gpui_kit::Modifiers::default());
        visual.simulate_mouse_up(wheel.center(), MouseButton::Left, gpui_kit::Modifiers::default());
        visual.update(|_, cx| view.update(cx, |bar, cx| bar.rank(cx)));
        let again = filter_of(visual, tab);
        assert!(
            again.target.a.abs() < 0.03 && again.target.b.abs() < 0.03,
            "the first went to the center"
        );
        assert_eq!(again.extra[0], moved.extra[0]);

        // All and Any; a third color fills the chord and the plus goes.
        visual.update(|window, cx| {
            view.update(cx, |bar, cx| {
                bar.set_mode(ChordMode::All, window, cx);
                bar.add_target(window, cx);
                bar.add_target(window, cx);
                bar.rank(cx);
            })
        });
        let full = filter_of(visual, tab);
        assert_eq!((full.count(), full.mode), (3, ChordMode::All));
        assert!(!found(visual, "Add a color"), "three is the most");

        // Taking the first out moves the others up and keeps the wheel on the color it was on.
        let (second, third) = (full.target_at(1), full.target_at(2));
        visual.update(|window, cx| {
            view.update(cx, |bar, cx| {
                assert_eq!(bar.selected(cx), 2);
                bar.remove_target(0, window, cx);
                bar.rank(cx);
            })
        });
        let left = filter_of(visual, tab);
        assert_eq!((left.count(), left.mode), (2, ChordMode::All));
        assert_eq!((left.target_at(0), left.target_at(1)), (second, third));
        assert_eq!(visual.update(|_, cx| view.read(cx).selected(cx)), 1);
        // The last color can't be removed.
        visual.update(|window, cx| {
            view.update(cx, |bar, cx| {
                bar.remove_target(0, window, cx);
                bar.rank(cx);
                bar.remove_target(0, window, cx);
                bar.rank(cx);
            })
        });
        assert_eq!(filter_of(visual, tab).count(), 1);
    }
}
