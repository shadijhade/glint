//! Small things drawn on the GPU for the dock: a number that rolls to its new value, a line of the last
//! counts, and the glint's light turning once round a box (docs/gpui/aware/PLAN.md: F8, F10, F11).
//!
//! Card C1 of docs/gpui/aware fills this in: the signatures are the contract; until then they draw at rest.

use crate::kit::prelude::*;

/// A number that rolls from `from` to `to`, drawn as `words(value)`. The animation plays once for each new `to`.
pub fn roll(
    name: &'static str,
    from: u64,
    to: u64,
    words: impl Fn(u64) -> SharedString + 'static,
    cx: &App,
) -> AnyElement {
    let _ = (name, from, cx);
    div().child(words(to)).into_any_element()
}

/// The value a roll shows at `delta` (0 to 1) of its way from `from` to `to`.
pub fn roll_value(from: u64, to: u64, delta: f32) -> u64 {
    let _ = (from, delta);
    to
}

/// A line through the last counts, the oldest at the start edge: 44 px wide and 16 px high unless resized.
pub fn spark(values: &[u8], color: Hsla) -> Div {
    let _ = (values, color);
    div()
}

/// Where a spark line of `values` goes through, in a box `width` by `height`.
pub fn spark_points(values: &[u8], width: f32, height: f32) -> Vec<(f32, f32)> {
    let _ = (values, width, height);
    Vec::new()
}

/// The glint's light turning once round a rounded box: an absolute layer for a `relative()` parent, which plays
/// once for each new `turn` and draws nothing at rest.
pub fn glint_ring(name: &'static str, turn: u64, radius: Pixels, cx: &App) -> AnyElement {
    let _ = (name, turn, radius, cx);
    div().into_any_element()
}

/// The angle of the ring's light, in degrees, at `delta` (0 to 1) of its turn.
pub fn ring_angle(delta: f32) -> f32 {
    let _ = delta;
    0.
}

/// How strongly the ring is drawn at `delta` (0 to 1) of its turn: nothing at both ends.
pub fn ring_opacity(delta: f32) -> f32 {
    let _ = delta;
    0.
}
