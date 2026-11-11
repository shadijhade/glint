//! Glint: a notch of a mouse wheel glides to where it scrolls instead of jumping there.
//!
//! Windows reports a wheel notch as whole lines ([`ScrollDelta::Lines`], three a notch by default). Applied at
//! once, the content jumps 60 px or more in one frame and the eye loses its place: a file list scrolled that way
//! drew one frame per notch. Here the notch moves a *target*, and the offset closes in on it frame by frame,
//! fast at first and settling within about a sixth of a second (an exponential ease: each frame covers the same
//! share of what is left, so further notches while it moves simply add to the target and the speed follows).
//!
//! - The first step is taken in the event itself, so the wheel still answers in the very next frame.
//! - A notch the other way stops the rest of the old glide: the wheel never fights the hand.
//! - Anything else that moves the offset (the element clamping it at an end, a drag of the scroll bar, a jump to
//!   an item) ends the glide where that left it: every step checks the offset is still the one it wrote.
//! - Steps land on whole device pixels, so hairlines and text stay sharp while they move.
//! - Fine-grained deltas (a precision touchpad, a free-spinning wheel) are not notches and are applied as they
//!   come, as before. So is everything when the app asks for reduced motion ([`App::reduce_motion`]).
//!
//! The state is keyed by the scroll offset's allocation, kept per thread (elements are rebuilt every frame, the
//! shared offset is what lives on) and dropped when the glide ends or the offset is gone.
//!
//! This crate's own tests don't build as published, so the steps are tested from Glint, through
//! [`wheel_glide_advance`] (glint-ui's `theme::motion` tests).

use crate::{App, EntityId, ListState, Pixels, Point, ScrollDelta, Window, px};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
    time::Instant,
};

/// The time in which the distance left shrinks to 1/e of itself. 50 ms covers 95% of a notch in 150 ms.
const TIME_CONSTANT: f32 = 0.050;
/// The time the first step stands for: the notch is answered in the frame that follows the event.
const FIRST_STEP: f32 = 0.014;
/// A step never stands for more than this, so a stalled frame doesn't end the glide in one jump.
const LONGEST_STEP: f32 = 0.050;

struct Glide {
    offset: Weak<RefCell<Point<Pixels>>>,
    /// Where the offset is heading.
    target: Point<Pixels>,
    /// The offset as the last step left it; anything else found there was written by someone else.
    written: Point<Pixels>,
    last_step: Instant,
    view: EntityId,
}

/// A glide of a [`crate::list`], which keeps its place by item rather than by an offset: what is left to
/// scroll is fed to it a step a frame.
struct ListGlide {
    state: ListState,
    /// What is left to scroll, as a wheel delta (positive shows what is above).
    left: Pixels,
    height: Pixels,
    last_step: Instant,
    view: EntityId,
}

thread_local! {
    static GLIDES: RefCell<HashMap<usize, Glide>> = RefCell::new(HashMap::new());
    static LIST_GLIDES: RefCell<HashMap<usize, ListGlide>> = RefCell::new(HashMap::new());
}

/// Scrolls a [`crate::list`] by `delta` (a wheel delta) over the next frames.
pub(crate) fn push_list(
    state: &ListState,
    height: Pixels,
    delta: Pixels,
    view: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    let key = state.glide_key();
    let delta = on_grid(delta, window.scale_factor());
    let started = LIST_GLIDES.with(|glides| {
        let mut glides = glides.borrow_mut();
        match glides.get_mut(&key) {
            Some(glide) => {
                // A notch the other way: what was left of the old glide is dropped.
                if f32::from(glide.left) * f32::from(delta) < 0. {
                    glide.left = px(0.);
                }
                glide.left += delta;
                glide.height = height;
                glide.view = view;
                false
            }
            None => {
                glides.insert(
                    key,
                    ListGlide {
                        state: state.clone(),
                        left: delta,
                        height,
                        last_step: Instant::now(),
                        view,
                    },
                );
                true
            }
        }
    });
    if started {
        step_list(key, true, window, cx);
    }
}

fn step_list(key: usize, first: bool, window: &mut Window, cx: &mut App) {
    let scale = window.scale_factor();
    let Some((state, height, by, view)) = LIST_GLIDES.with(|glides| {
        let mut glides = glides.borrow_mut();
        let glide = glides.get_mut(&key)?;
        let now = Instant::now();
        let elapsed = if first {
            FIRST_STEP
        } else {
            now.duration_since(glide.last_step)
                .as_secs_f32()
                .clamp(0.001, LONGEST_STEP)
        };
        glide.last_step = now;
        let share = 1. - (-elapsed / TIME_CONSTANT).exp();
        let by = advance(px(0.), glide.left, share, scale);
        glide.left -= by;
        Some((glide.state.clone(), glide.height, by, glide.view))
    }) else {
        return;
    };
    // The list scrolls itself (and tells whoever listens), outside the borrow above.
    let moved = by != px(0.) && state.glide_step(height, by, view, window, cx);
    let done = LIST_GLIDES.with(|glides| {
        let mut glides = glides.borrow_mut();
        let done = glides
            .get(&key)
            .is_none_or(|glide| !moved || glide.left == px(0.));
        if done {
            glides.remove(&key);
        }
        done
    });
    if !done {
        window.on_next_frame(move |window, cx| step_list(key, false, window, cx));
    }
}

/// How far a "line" of a wheel scrolls: a third of 100 px at the usual text size, so a notch of three lines (and a
/// touchpad's 120 units) moves 100 px, as it does in a browser and close to File Explorer's three rows. GPUI's own
/// measure is the window's text line (about 21 px: 62 px a notch), which made long lists slow to get through.
/// It follows the rem size, so it grows with the text-size setting as rows do.
pub(crate) fn line_height(window: &Window) -> Pixels {
    px(100. / 3.) * (f32::from(window.rem_size()) / 16.)
}

/// `value` on the device-pixel grid: a glide must end on a whole pixel, or what it scrolled stays soft.
fn on_grid(value: Pixels, scale: f32) -> Pixels {
    let scale = scale.max(0.1);
    px((f32::from(value) * scale).round() / scale)
}

/// Whether a scroll is a wheel notch that should glide: whole lines, and motion not reduced.
pub(crate) fn wants(delta: &ScrollDelta, cx: &App) -> bool {
    !cx.reduce_motion()
        && matches!(delta, ScrollDelta::Lines(lines) if lines.x.abs().max(lines.y.abs()) >= 0.9)
}

/// For an element that scrolls itself (the kit's text editor keeps its own offset and bounds): whether a scroll
/// is a wheel notch that should glide.
pub fn wheel_notch_glides(delta: &ScrollDelta, cx: &App) -> bool {
    wants(delta, cx)
}

/// For an element that scrolls itself: how much of what is `left` of a glide to scroll in this step, `elapsed`
/// after the step before it (None for the first step, taken in the event). The element subtracts it from what
/// is left and steps again next frame until nothing is, or until it stops moving.
pub fn wheel_glide_advance(left: Pixels, elapsed: Option<std::time::Duration>, scale: f32) -> Pixels {
    // Under half a device pixel left (what steps on the grid leave behind in floating point): all of it, so
    // subtracting the step leaves exactly nothing.
    if left.abs() <= px(0.5 / scale.max(0.1)) {
        return left;
    }
    let seconds = elapsed.map_or(FIRST_STEP, |elapsed| {
        elapsed.as_secs_f32().clamp(0.001, LONGEST_STEP)
    });
    advance(px(0.), on_grid(left, scale), 1. - (-seconds / TIME_CONSTANT).exp(), scale)
}

/// Moves `offset` by `delta` over the next frames, redrawing `view` as it goes.
pub(crate) fn push(
    offset: &Rc<RefCell<Point<Pixels>>>,
    delta: Point<Pixels>,
    view: EntityId,
    window: &mut Window,
    cx: &mut App,
) {
    let key = Rc::as_ptr(offset) as *const () as usize;
    let current = *offset.borrow();
    let scale = window.scale_factor();
    let delta = Point {
        x: on_grid(delta.x, scale),
        y: on_grid(delta.y, scale),
    };
    let started = GLIDES.with(|glides| {
        let mut glides = glides.borrow_mut();
        match glides.get_mut(&key) {
            Some(glide) if glide.offset.upgrade().is_some() => {
                if moved(glide.written.x, current.x) {
                    glide.target.x = current.x;
                }
                if moved(glide.written.y, current.y) {
                    glide.target.y = current.y;
                }
                glide.written = current;
                // A notch the other way: what was left of the old glide is dropped.
                if f32::from(glide.target.x - current.x) * f32::from(delta.x) < 0. {
                    glide.target.x = current.x;
                }
                if f32::from(glide.target.y - current.y) * f32::from(delta.y) < 0. {
                    glide.target.y = current.y;
                }
                glide.target.x += delta.x;
                glide.target.y += delta.y;
                glide.view = view;
                false
            }
            _ => {
                glides.insert(
                    key,
                    Glide {
                        offset: Rc::downgrade(offset),
                        target: Point {
                            x: current.x + delta.x,
                            y: current.y + delta.y,
                        },
                        written: current,
                        last_step: Instant::now(),
                        view,
                    },
                );
                true
            }
        }
    });
    if started {
        step(key, true, window, cx);
    }
}

/// Whether the offset on one axis is no longer what the glide wrote.
fn moved(written: Pixels, found: Pixels) -> bool {
    (found - written).abs() > px(0.75)
}

/// One axis's next position: a share of what is left, at least a device pixel, landing on the pixel grid.
fn advance(from: Pixels, to: Pixels, share: f32, scale: f32) -> Pixels {
    let device = px(1. / scale.max(0.1));
    let left = to - from;
    if left.abs() <= device {
        return to;
    }
    let mut by = left * share;
    if by.abs() < device {
        by = if left > px(0.) { device } else { -device };
    }
    let next = px((f32::from(from + by) * scale).round() / scale);
    // Rounding must never pass the target or stand still.
    if (to - next).abs() >= left.abs() {
        to
    } else {
        next
    }
}

fn step(key: usize, first: bool, window: &mut Window, cx: &mut App) {
    let scale = window.scale_factor();
    let outcome = GLIDES.with(|glides| {
        let mut glides = glides.borrow_mut();
        let glide = glides.get_mut(&key)?;
        let Some(offset) = glide.offset.upgrade() else {
            glides.remove(&key);
            return None;
        };
        let now = Instant::now();
        let elapsed = if first {
            FIRST_STEP
        } else {
            now.duration_since(glide.last_step)
                .as_secs_f32()
                .clamp(0.001, LONGEST_STEP)
        };
        glide.last_step = now;
        let mut offset = offset.borrow_mut();
        // Clamped at an end, or moved by something else: the glide ends where that left it.
        if moved(glide.written.x, offset.x) {
            glide.target.x = offset.x;
        }
        if moved(glide.written.y, offset.y) {
            glide.target.y = offset.y;
        }
        let share = 1. - (-elapsed / TIME_CONSTANT).exp();
        offset.x = advance(offset.x, glide.target.x, share, scale);
        offset.y = advance(offset.y, glide.target.y, share, scale);
        glide.written = *offset;
        let done = *offset == glide.target;
        let view = glide.view;
        if done {
            glides.remove(&key);
        }
        Some((view, done))
    });
    let Some((view, done)) = outcome else { return };
    cx.notify(view);
    if !done {
        window.on_next_frame(move |window, cx| step(key, false, window, cx));
    }
}
