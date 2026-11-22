//! Hop mode (Alt+J): keyboard-first jumping. One chord puts a small badge of one or two letters on everything in sight
//! that can be jumped to (the rows and tiles of both panes, the places in the rail, the tabs, the breadcrumbs of the
//! address); typing a badge's letters does what a click on it does, so a person who lives on the keyboard can cross
//! from one pane to the other, or to a place or a tab, without the mouse and without arrowing through a folder.
//!
//! **How it is built.**
//! - A registry ([`mark`]) that the views fill while they are drawn: one call per hit target, which does nothing at
//!   all (an atomic read) unless hop mode is on, so the lists and grids pay nothing the rest of the time. A mark says
//!   what the target is called and what a click on it does (a closure the view builds, so the view's own click
//!   handler stays the one place that knows); the registry records where the target was drawn.
//! - Turning the mode on makes the window draw once with the registry open, and the frame after that the targets are
//!   frozen, given their hints ([`hints`]) in an order that doesn't depend on layout details, and the overlay shows
//!   them. From then on nothing under the overlay can move (it covers the window and takes the pointer), so the
//!   hints stay put until the mode ends.
//! - [`HopOverlay`] is the one layer drawn above the workspace. It holds the keyboard while the mode is on, and
//!   hands it back to where it was before the target's action runs, so an action that moves focus has the last word.
//!
//! **Keys.** Letters narrow the badges (the ones that no longer match fade, the letters typed show dimmed on those
//! that do); a complete hint acts. Backspace takes a letter back. Escape, a click, Alt+J again, or any other key
//! cancels.
//!
//! **Order of hints.** The targets that are few and structural come first and get the shortest hints (tabs, then
//! breadcrumbs, then places), then the rows and tiles pane by pane (the pane at the start edge first), each in
//! reading order, so `a` is the first tab or the first breadcrumb and a file's hint is two letters in a busy window.

mod hints;
#[cfg(test)]
mod tests;

use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use gpui_kit::{canvas, Global, SharedString, Subscription, WindowId};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use hints::{generate as generate_hints, type_into, Typed, ALPHABET};

/// Whether hop mode is collecting or showing: the one thing a view reads to decide whether to mark anything.
static ON: AtomicBool = AtomicBool::new(false);

/// How much of a target must be on screen to be given a hint: a row cut off by the edge of its list by more than half
/// is not offered.
const MOST_HIDDEN: f32 = 0.5;

/// Whether hop mode is on. Cheap enough to ask for every row of a list.
pub fn active() -> bool {
    ON.load(Ordering::Relaxed)
}

/// What kind of thing a target is. The order is the order hints are handed out in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    /// A tab in the rail.
    Tab,
    /// A breadcrumb of the address.
    Crumb,
    /// A place or a folder of the rail's tree.
    Place,
    /// A row of the list (or of the columns).
    Row,
    /// A tile of the grid.
    Tile,
}

impl Kind {
    /// What it is called to a screen reader.
    fn noun(self) -> SharedString {
        match self {
            Kind::Tab => t!("tab"),
            Kind::Crumb => t!("breadcrumb"),
            Kind::Place => t!("place"),
            Kind::Row | Kind::Tile => t!("item"),
        }
    }
}

/// What a hint does: what a click on the target does.
pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// One target a view offers, made only while hop mode is on.
#[derive(Clone)]
pub struct Mark {
    /// What the target is.
    pub kind: Kind,
    /// Targets that belong together (the rows of one pane): they are numbered together, the group nearest the start
    /// edge first. The view's entity id does for it.
    pub group: u64,
    /// What it is called, for the screen reader ("Documents", a file's name).
    pub name: SharedString,
    /// What a click on it does.
    pub act: Act,
}

impl Mark {
    /// A mark whose action is `act`.
    pub fn new(
        kind: Kind,
        group: u64,
        name: impl Into<SharedString>,
        act: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Mark {
            kind,
            group,
            name: name.into(),
            act: Rc::new(act),
        }
    }
}

/// A press of the left button at `at`, for a mark's action that has to call the view's own click handler.
pub fn click(at: Point<Pixels>) -> MouseDownEvent {
    MouseDownEvent {
        button: MouseButton::Left,
        position: at,
        modifiers: Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    }
}

/// A target and where it was drawn.
struct Spot {
    mark: Mark,
    bounds: Bounds<Pixels>,
}

/// A target given its hint, as the overlay draws it.
#[derive(Clone)]
struct Hinted {
    mark: Mark,
    bounds: Bounds<Pixels>,
    /// The letters, lowercase (what is typed against) and as the badge shows them.
    hint: String,
    shown: SharedString,
}

/// Where hop mode is.
enum Phase {
    Off,
    /// The window is being drawn with the registry open.
    Collecting,
    /// The targets are frozen with their hints.
    Showing,
}

/// The state of the mode, app-wide (one window at a time has it).
struct Hop {
    phase: Phase,
    window: Option<WindowId>,
    spots: Vec<Spot>,
    tab_clips: HashMap<u64, Bounds<Pixels>>,
    hinted: Vec<Hinted>,
    typed: String,
}

impl Global for Hop {}

impl Hop {
    fn new() -> Self {
        Hop {
            phase: Phase::Off,
            window: None,
            spots: Vec::new(),
            tab_clips: HashMap::new(),
            hinted: Vec::new(),
            typed: String::new(),
        }
    }
}

/// Records the viewport clip of tab list `group` so scrolled-off tabs are never given hints over the sidebar.
pub fn clip_tabs(group: u64, bounds: Bounds<Pixels>, window: &Window, cx: &mut App) {
    if !cx.has_global::<Hop>() {
        return;
    }
    let id = window.window_handle().window_id();
    cx.update_global::<Hop, _>(|hop, _| {
        if matches!(hop.phase, Phase::Collecting) && hop.window == Some(id) {
            hop.tab_clips.insert(group, bounds);
        }
    });
}

/// The marks of a view, one per target, drawn only while hop mode is on: `el` gets a child that notes where it was
/// drawn and nothing else (it takes no room and no pointer). `make` is called only then, so a view that builds
/// names and closures for it pays for them only in the mode.
pub fn mark<E: ParentElement>(el: E, make: impl FnOnce() -> Mark) -> E {
    if !active() {
        return el;
    }
    let mark = make();
    el.child(
        canvas(
            move |bounds, window, cx| {
                record(mark, bounds, window, cx);
            },
            |_, _, _, _| (),
        )
        // Pinned to the target's corner: an absolute child with no insets takes its place in the flow, which in a
        // block element (a list row, a rail tab) is under the target's content, a whole row lower.
        .absolute()
        .top_0()
        .left_0()
        .size_full(),
    )
}

/// Notes where a mark was drawn, if it is on screen in the window the mode is in.
fn record(mark: Mark, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let (mask, id) = (window.content_mask().bounds, window.window_handle().window_id());
    note(mark, bounds, mask, id, cx);
}

/// [`record`], given the clip and the window.
fn note(mark: Mark, bounds: Bounds<Pixels>, mask: Bounds<Pixels>, id: WindowId, cx: &mut App) {
    // The rail's tab list scrolls by itself; clip tabs to the tablist viewport recorded during prepaint
    // (falling back to the window mask if none was recorded).
    let clip = if mark.kind == Kind::Tab {
        cx.try_global::<Hop>()
            .and_then(|hop| hop.tab_clips.get(&mark.group).copied())
            .unwrap_or(mask)
    } else {
        mask
    };
    let visible = bounds.intersect(&clip);
    let (area, seen) = (
        f32::from(bounds.size.width) * f32::from(bounds.size.height),
        f32::from(visible.size.width) * f32::from(visible.size.height),
    );
    if area <= 0. || seen < area * MOST_HIDDEN {
        return;
    }
    if !cx.has_global::<Hop>() {
        return;
    }
    cx.update_global::<Hop, _>(|hop, _| {
        if !matches!(hop.phase, Phase::Collecting) || hop.window != Some(id) {
            return;
        }
        // The same target drawn again (a second frame before the freeze) replaces itself.
        hop.spots
            .retain(|spot| !(spot.mark.kind == mark.kind && spot.bounds.origin == visible.origin));
        hop.spots.push(Spot { mark, bounds: visible });
    });
}

/// The order targets are given hints in: kind, then the group nearest the start edge, then reading order.
fn ordered(mut spots: Vec<Spot>, rtl: bool) -> Vec<Spot> {
    let mut edge: HashMap<u64, f32> = HashMap::new();
    for spot in &spots {
        let x = f32::from(spot.bounds.origin.x);
        edge.entry(spot.mark.group)
            .and_modify(|best| *best = if rtl { best.max(x) } else { best.min(x) })
            .or_insert(x);
    }
    spots.sort_by(|a, b| {
        let across = |spot: &Spot| {
            let x = edge.get(&spot.mark.group).copied().unwrap_or(0.);
            if rtl {
                -x
            } else {
                x
            }
        };
        let (ax, bx) = (f32::from(a.bounds.origin.x), f32::from(b.bounds.origin.x));
        let (ay, by) = (f32::from(a.bounds.origin.y), f32::from(b.bounds.origin.y));
        let kind = |spot: &Spot| match spot.mark.kind {
            // A row and a tile never share a pane, and are numbered alike.
            Kind::Tile => Kind::Row,
            other => other,
        };
        kind(a)
            .cmp(&kind(b))
            .then(across(a).total_cmp(&across(b)))
            .then(a.mark.group.cmp(&b.mark.group))
            .then(ay.total_cmp(&by))
            .then(if rtl { bx.total_cmp(&ax) } else { ax.total_cmp(&bx) })
    });
    spots
}

/// The window's hop layer. Always in the workspace; it draws nothing and takes nothing while the mode is off.
pub struct HopOverlay {
    focus: FocusHandle,
    /// What had the keyboard before the mode took it.
    before: Option<FocusHandle>,
    /// The window's size when the mode began: a window that is resized under the badges ends it.
    size: Size<Pixels>,
    _subscriptions: Vec<Subscription>,
}

impl HopOverlay {
    /// Makes the layer and puts the state the registry shares into the app.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<Hop>() {
            cx.set_global(Hop::new());
        }
        let focus = cx.focus_handle();
        // A layer is made off: whatever mode an earlier one was in is over.
        ON.store(false, Ordering::Relaxed);
        // The keyboard going anywhere else (another window in front, a dialog) ends it.
        let blur = cx.on_blur(&focus, window, |this, window, cx| this.end(false, window, cx));
        HopOverlay {
            focus,
            before: None,
            size: Size::default(),
            _subscriptions: vec![blur],
        }
    }

    /// Whether the mode is on in this window.
    pub fn is_on(&self, cx: &App) -> bool {
        active()
            && cx
                .try_global::<Hop>()
                .is_some_and(|hop| !matches!(hop.phase, Phase::Off))
    }

    /// Alt+J: on when off, off when on.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_on(cx) {
            self.end(true, window, cx);
        } else {
            self.begin(window, cx);
        }
    }

    /// Turns the mode on: the registry opens, the keyboard comes here, and the window is drawn with the marks.
    fn begin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = window.window_handle().window_id();
        cx.update_global::<Hop, _>(|hop, _| {
            *hop = Hop::new();
            hop.phase = Phase::Collecting;
            hop.window = Some(id);
        });
        ON.store(true, Ordering::Relaxed);
        window.dismiss_tooltips();
        self.before = window.focused(cx).filter(|handle| handle != &self.focus);
        self.size = window.viewport_size();
        self.focus.focus(window, cx);
        window.refresh();
        cx.notify();
    }

    /// The frame has been drawn with the registry open: the targets are frozen, given their hints, and shown.
    pub(crate) fn freeze(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rtl = i18n::is_rtl();
        cx.update_global::<Hop, _>(|hop, _| {
            if !matches!(hop.phase, Phase::Collecting) {
                return;
            }
            let spots = ordered(std::mem::take(&mut hop.spots), rtl);
            let hints = generate_hints(spots.len());
            hop.hinted = spots
                .into_iter()
                .zip(hints)
                .map(|(spot, hint)| Hinted {
                    shown: hint.to_uppercase().into(),
                    hint,
                    mark: spot.mark,
                    bounds: spot.bounds,
                })
                .collect();
            hop.phase = Phase::Showing;
        });
        let nothing = cx.global::<Hop>().hinted.is_empty();
        if nothing {
            // Nothing to jump to (an empty window): there is no mode to be in.
            self.end(true, window, cx);
        } else {
            cx.notify();
        }
    }

    /// Turns the mode off; the keyboard goes back to where it was when `restore` is set (not when something else
    /// has already taken it).
    fn end(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_on(cx) {
            return;
        }
        ON.store(false, Ordering::Relaxed);
        cx.update_global::<Hop, _>(|hop, _| *hop = Hop::new());
        let before = self.before.take();
        if restore && self.focus.is_focused(window) {
            if let Some(handle) = before {
                handle.focus(window, cx);
            }
        }
        window.refresh();
        cx.notify();
    }

    /// A key pressed while the badges are up.
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(cx.global::<Hop>().phase, Phase::Showing) {
            return;
        }
        let key = &event.keystroke;
        let mods = &key.modifiers;
        if key.key == "backspace" && !mods.control && !mods.alt && !mods.platform {
            // Backspace with nothing typed gives up, like any other key that is no letter.
            let was_empty = cx.update_global::<Hop, _>(|hop, _| {
                let was_empty = hop.typed.is_empty();
                hop.typed.pop();
                was_empty
            });
            if was_empty {
                self.end(true, window, cx);
            } else {
                cx.notify();
            }
            return;
        }
        let letter = match key.key.as_str() {
            key if key.chars().count() == 1 && !mods.control && !mods.alt && !mods.platform && !mods.function => {
                key.chars().next().filter(char::is_ascii_alphabetic)
            }
            _ => None,
        };
        let Some(letter) = letter else {
            self.end(true, window, cx);
            return;
        };
        let (typed, hints) = cx.update_global::<Hop, _>(|hop, _| {
            hop.typed.push(letter.to_ascii_lowercase());
            (
                hop.typed.clone(),
                hop.hinted.iter().map(|h| h.hint.clone()).collect::<Vec<_>>(),
            )
        });
        match type_into(&hints, &typed) {
            Typed::Nothing => self.end(true, window, cx),
            Typed::More => cx.notify(),
            Typed::Hit(index) => {
                let act = cx.global::<Hop>().hinted.get(index).map(|h| h.mark.act.clone());
                // The mode ends first and gives the keyboard back; the action then has the last word on focus.
                self.end(true, window, cx);
                if let Some(act) = act {
                    act(window, cx);
                }
            }
        }
    }

    /// The badge of one target.
    fn badge(index: usize, hinted: &Hinted, typed: &str, rtl: bool, window: &Window, cx: &App) -> AnyElement {
        let t = theme::tokens(cx);
        let matching = hints::still_matches(&hinted.hint, typed);
        let done = if matching { typed.chars().count() } else { 0 };
        let (gone, rest): (String, String) = (
            hinted.shown.chars().take(done).collect(),
            hinted.shown.chars().skip(done).collect(),
        );
        let bounds = hinted.bounds;
        let tall = matches!(hinted.mark.kind, Kind::Tile);
        // The badge sits at the target's start edge, centered on it (at the top of a tile), and takes no room:
        // the box it is in is the target's own.
        let place = div().absolute().p(px(2.)).top(bounds.origin.y).h(bounds.size.height);
        let place = if rtl {
            place.right(window.viewport_size().width - bounds.origin.x - bounds.size.width)
        } else {
            place.left(bounds.origin.x)
        };
        let badge = row_start()
            .id(("hop-badge", index))
            .test_support()
            .role(Role::Button)
            .aria_label(t!(
                "{hint}: {name}",
                hint = hinted.shown.clone(),
                name = hinted.mark.name.clone()
            ))
            .aria_description(hinted.mark.kind.noun())
            .flex_none()
            .px(px(5.))
            .h(rems(1.25))
            .rounded(theme::size::RADIUS_SM)
            .bg(t.primary)
            .border_1()
            .border_color(t.primary_foreground.opacity(0.35))
            .shadow(theme::elevation::raised(cx))
            .font_family(theme::mono_font(cx))
            .text_color(t.primary_foreground)
            .text_size(theme::size::XS)
            .font_weight(FontWeight::BOLD)
            .when(!matching, |badge| badge.opacity(0.16))
            .children((!gone.is_empty()).then(|| div().opacity(0.42).child(gone)))
            .child(div().child(rest));
        let badge = if cx.reduce_motion() {
            badge.into_any_element()
        } else {
            badge
                .with_animation(
                    ("hop-badge-in", index),
                    Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
                    |el, delta| el.paint_scale(theme::motion::POP + (1. - theme::motion::POP) * delta),
                )
                .into_any_element()
        };
        place
            .flex()
            .flex_row()
            .items_center()
            .when(tall, |place| place.items_start())
            .when(matches!(hinted.mark.kind, Kind::Crumb), |place| {
                place.items_end().mb(px(-3.))
            })
            .child(badge)
            .into_any_element()
    }
}

impl Render for HopOverlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = div()
            .id("hop-overlay")
            .test_support()
            .track_focus(&self.focus)
            .absolute()
            .top_0()
            .left_0();
        if !self.is_on(cx) {
            return base.size_0().into_any_element();
        }
        let phase_collecting = matches!(cx.global::<Hop>().phase, Phase::Collecting);
        if phase_collecting {
            // The frame this one draws has the marks; the next one starts with them frozen.
            let me = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                me.update(cx, |this, cx| this.freeze(window, cx)).ok();
            });
        } else if window.viewport_size() != self.size {
            // The window changed under the badges: they would point at nothing.
            let me = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                me.update(cx, |this, cx| this.end(true, window, cx)).ok();
            });
        }
        let t = theme::tokens(cx).clone();
        let rtl = i18n::is_rtl();
        let (typed, badges) = {
            let hop = cx.global::<Hop>();
            let typed = hop.typed.clone();
            let badges: Vec<AnyElement> = hop
                .hinted
                .iter()
                .enumerate()
                .map(|(index, hinted)| Self::badge(index, hinted, &typed, rtl, window, cx))
                .collect();
            (typed, badges)
        };
        let usage = t!("Type the letters on a badge to go there. Backspace takes one back. Escape cancels.");
        let keys_row = || {
            row_start()
                .gap(space(1.))
                .items_center()
                .children(gpui_kit::Keystroke::parse("backspace").ok().map(Kbd::new))
                .children(gpui_kit::Keystroke::parse("escape").ok().map(Kbd::new))
        };
        let banner = row_start()
            .id("hop-usage")
            .test_support()
            .role(Role::Status)
            .aria_label(usage.clone())
            .absolute()
            .bottom(crate::components::status_bar::HEIGHT + px(20.))
            .left_0()
            .right_0()
            .justify_center()
            .child(
                row_start()
                    .gap(space(2.))
                    .px(space(3.))
                    .py(space(1.5))
                    .rounded(theme::size::RADIUS_LG)
                    .bg(t.popover)
                    .border_1()
                    .border_color(t.stroke)
                    .shadow(theme::elevation::floating(cx))
                    .text_color(t.popover_foreground)
                    .text_size(theme::size::SM)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(t!("Hop")))
                    .when(typed.is_empty(), |row| {
                        row.child(div().text_color(t.muted_foreground).child(usage))
                            .child(keys_row())
                    })
                    .when(!typed.is_empty(), |row| {
                        row.child(row_start().gap(space(1.)).children(typed.chars().map(|ch| {
                            div()
                                .px(px(6.))
                                .h(rems(1.25))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(theme::size::RADIUS_SM)
                                .bg(t.primary)
                                .text_color(t.primary_foreground)
                                .font_family(theme::mono_font(cx))
                                .text_size(theme::size::XS)
                                .font_weight(FontWeight::BOLD)
                                .child(SharedString::from(ch.to_ascii_uppercase().to_string()))
                        })))
                        .child(keys_row())
                    }),
            );
        let scrim = hsla(240. / 360., 0.2, 0.02, if t.dark { 0.16 } else { 0.06 });
        deferred(
            base.size_full()
                .bg(scrim)
                .occlude()
                .role(Role::Group)
                .aria_label(t!("Hop mode"))
                .aria_description(usage_description())
                // A click (any button) or the wheel gives up; nothing under the layer is reached.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.end(true, window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, window, cx| this.end(true, window, cx)),
                )
                .on_mouse_down(
                    MouseButton::Middle,
                    cx.listener(|this, _, window, cx| this.end(true, window, cx)),
                )
                .on_scroll(cx.listener(|this, _, window, cx| this.end(true, window, cx)))
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.key(event, window, cx);
                    cx.stop_propagation();
                }))
                .children(badges)
                .child(banner),
        )
        .with_priority(crate::kit::Z_HOP)
        .into_any_element()
    }
}

/// How the layer describes itself to a screen reader.
fn usage_description() -> SharedString {
    t!("Every item you can jump to has a badge of letters. Type them to go there. Escape cancels.")
}

/// What is on offer, hint by hint, for the tests of what hands it out.
#[cfg(test)]
pub(crate) fn offered(visual: &mut gpui_kit::VisualTestContext) -> Vec<(String, Kind, String)> {
    visual.update(|_, cx| {
        cx.global::<Hop>()
            .hinted
            .iter()
            .map(|h| (h.hint.clone(), h.mark.kind, h.mark.name.to_string()))
            .collect()
    })
}

/// Alt+J in `overlay`'s window.
pub fn toggle(overlay: &Entity<HopOverlay>, window: &mut Window, cx: &mut App) {
    overlay.update(cx, |overlay, cx| overlay.toggle(window, cx));
}
