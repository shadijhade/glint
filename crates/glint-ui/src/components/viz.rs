//! Small things drawn on the GPU for the dock: a number that rolls to its new value, a line of the last
//! counts, and the glint's light turning once round a box (docs/gpui/aware/PLAN.md: F8, F10, F11).
//!
//! They hold no state. Each takes its colors as arguments or reads the theme's tokens, and each is right when
//! every animation lands at once (Windows' "Animation effects" off): a roll is its final number, the ring is
//! not drawn.

use crate::kit::prelude::*;
use crate::theme;
use std::time::Duration;

/// A number that rolls from `from` to `to`, drawn as `words(value)`. The animation plays once for each new `to`.
pub fn roll(
    name: &'static str,
    from: u64,
    to: u64,
    words: impl Fn(u64) -> SharedString + 'static,
    cx: &App,
) -> AnyElement {
    if from == to || cx.reduce_motion() {
        return div().child(words(to)).into_any_element();
    }
    div()
        .with_animation(
            (name, to as usize),
            Animation::new(theme::motion::BASE).with_easing(theme::motion::glide),
            move |el, delta| el.child(words(roll_value(from, to, delta))),
        )
        .into_any_element()
}

/// The value a roll shows at `delta` (0 to 1) of its way from `from` to `to`.
pub fn roll_value(from: u64, to: u64, delta: f32) -> u64 {
    // Both ends are exact; only what is between goes through f64, and is kept between them.
    if delta.is_nan() || delta <= 0. {
        return from;
    }
    if delta >= 1. {
        return to;
    }
    let (low, high) = (from.min(to), from.max(to));
    let at = from as f64 + (to as f64 - from as f64) * f64::from(delta);
    // `as` saturates for floats, so nothing here can overflow.
    (at.round() as u64).clamp(low, high)
}

/// A line through the last counts, the oldest at the start edge: 44 px wide and 16 px high unless resized.
pub fn spark(values: &[u8], color: Hsla) -> Div {
    let values = values.to_vec();
    let rtl = crate::i18n::is_rtl();
    div().w(px(44.)).h(px(16.)).child(
        canvas(
            |_, _, _| (),
            move |bounds: Bounds<Pixels>, _, window, _| {
                let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                let points = spark_points(&values, width, height);
                if points.len() < 2 {
                    return;
                }
                let mut path = PathBuilder::stroke(px(1.5));
                for (i, (x, y)) in points.into_iter().enumerate() {
                    let x = if rtl { width - x } else { x };
                    let at = point(bounds.origin.x + px(x), bounds.origin.y + px(y));
                    if i == 0 {
                        path.move_to(at);
                    } else {
                        path.line_to(at);
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .size_full(),
    )
}

/// Where a spark line of `values` goes through, in a box `width` by `height`.
pub fn spark_points(values: &[u8], width: f32, height: f32) -> Vec<(f32, f32)> {
    if values.len() < 2 {
        return Vec::new();
    }
    let max = f32::from(values.iter().copied().max().unwrap_or(0).max(1));
    let steps = (values.len() - 1) as f32;
    values
        .iter()
        .enumerate()
        .map(|(i, value)| {
            let x = width * i as f32 / steps;
            let y = height - 1. - (f32::from(*value) / max) * (height - 2.);
            (x, y)
        })
        .collect()
}

/// How long the ring takes to go round, once.
const RING_TURN: Duration = Duration::from_millis(700);
/// How wide the ring's line is.
const RING_STROKE: f32 = 1.5;

/// The glint's light turning once round a rounded box: an absolute layer for a `relative()` parent, which plays
/// once for each new `turn` and draws nothing at rest.
pub fn glint_ring(name: &'static str, turn: u64, radius: Pixels, cx: &App) -> AnyElement {
    if cx.reduce_motion() {
        return div().into_any_element();
    }
    let t = theme::tokens(cx);
    let (from, to) = (t.glint_from, t.glint_to);
    div()
        .absolute()
        .inset_0()
        .with_animation(
            (name, turn as usize),
            Animation::new(RING_TURN),
            move |layer, delta| {
                let angle = ring_angle(delta);
                let opacity = ring_opacity(delta);
                layer.child(
                    canvas(
                        |_, _, _| (),
                        move |bounds: Bounds<Pixels>, _, window, _| {
                            if opacity <= 0. {
                                return;
                            }
                            let Some(path) = ring_path(bounds, radius) else {
                                return;
                            };
                            // The opacity goes into the colors, so the path alone carries it.
                            let paint = linear_gradient(
                                angle,
                                linear_color_stop(from.opacity(opacity), 0.),
                                linear_color_stop(to.opacity(0.), 1.),
                            );
                            window.paint_path(path, paint);
                        },
                    )
                    .size_full(),
                )
            },
        )
        .into_any_element()
}

/// The outline of `bounds` as a rounded rectangle, half a stroke in from the edge, or None if it can't be built.
fn ring_path(bounds: Bounds<Pixels>, radius: Pixels) -> Option<Path<Pixels>> {
    let inset = RING_STROKE / 2.;
    let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    if width <= 2. * inset || height <= 2. * inset {
        return None;
    }
    let (left, top) = (f32::from(bounds.origin.x) + inset, f32::from(bounds.origin.y) + inset);
    let (right, bottom) = (left + width - 2. * inset, top + height - 2. * inset);
    let r = f32::from(radius).max(0.).min((right - left) / 2.).min((bottom - top) / 2.);
    // A quarter circle as a cubic curve: its control points are this far along each side.
    let k = r * 0.5523;
    let at = |x: f32, y: f32| point(px(x), px(y));
    let mut path = PathBuilder::stroke(px(RING_STROKE));
    path.move_to(at(left + r, top));
    path.line_to(at(right - r, top));
    path.cubic_bezier_to(at(right, top + r), at(right - r + k, top), at(right, top + r - k));
    path.line_to(at(right, bottom - r));
    path.cubic_bezier_to(at(right - r, bottom), at(right, bottom - r + k), at(right - r + k, bottom));
    path.line_to(at(left + r, bottom));
    path.cubic_bezier_to(at(left, bottom - r), at(left + r - k, bottom), at(left, bottom - r + k));
    path.line_to(at(left, top + r));
    path.cubic_bezier_to(at(left + r, top), at(left, top + r - k), at(left + r - k, top));
    path.close();
    path.build().ok()
}

/// The angle of the ring's light, in degrees, at `delta` (0 to 1) of its turn.
pub fn ring_angle(delta: f32) -> f32 {
    unit(delta) * 360.
}

/// How strongly the ring is drawn at `delta` (0 to 1) of its turn: nothing at both ends.
pub fn ring_opacity(delta: f32) -> f32 {
    (std::f32::consts::PI * unit(delta)).sin().max(0.)
}

/// `delta` kept to 0..=1 (a number that isn't one is the start).
fn unit(delta: f32) -> f32 {
    if delta.is_nan() {
        0.
    } else {
        delta.clamp(0., 1.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::TestAppContext;

    #[test]
    fn a_roll_starts_at_from_ends_at_to_and_is_linear_between() {
        assert_eq!(roll_value(0, 100, 0.0), 0);
        assert_eq!(roll_value(0, 100, 1.0), 100);
        assert_eq!(roll_value(0, 100, 0.5), 50);
        assert_eq!(roll_value(100, 0, 0.25), 75);
    }

    #[test]
    fn a_roll_never_overflows_and_treats_a_delta_past_one_as_one() {
        assert_eq!(roll_value(u64::MAX - 1, u64::MAX, 0.5), u64::MAX);
        assert!(roll_value(0, u64::MAX, 0.999) > u64::MAX / 2);
        assert_eq!(roll_value(0, 100, 2.0), 100);
        assert_eq!(roll_value(5, 100, -1.0), 5);
        assert_eq!(roll_value(5, 100, f32::NAN), 5);
    }

    #[test]
    fn one_value_has_no_spark_points() {
        assert!(spark_points(&[3], 44., 16.).is_empty());
        assert!(spark_points(&[], 44., 16.).is_empty());
    }

    #[test]
    fn spark_points_run_from_the_floor_to_the_top_in_equal_steps() {
        let points = spark_points(&[0, 2, 4], 44., 16.);
        let xs: Vec<f32> = points.iter().map(|p| p.0).collect();
        let ys: Vec<f32> = points.iter().map(|p| p.1).collect();
        assert_eq!(xs, vec![0., 22., 44.]);
        assert_eq!(ys, vec![15., 8., 1.]);
    }

    #[test]
    fn all_zeros_is_a_flat_line_a_pixel_above_the_bottom() {
        let points = spark_points(&[0; 5], 44., 16.);
        assert_eq!(points.len(), 5);
        assert!(points.iter().all(|p| p.1 == 15.));
    }

    #[test]
    fn thirty_values_give_thirty_points_that_never_go_back() {
        let values: Vec<u8> = (0..30).map(|i| (i * 7 % 11) as u8).collect();
        let points = spark_points(&values, 44., 16.);
        assert_eq!(points.len(), 30);
        assert!(points.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }

    #[test]
    fn the_ring_turns_a_full_circle_and_is_dark_at_both_ends() {
        assert_eq!(ring_angle(0.5), 180.);
        assert_eq!(ring_angle(2.), 360.);
        assert_eq!(ring_angle(-1.), 0.);
        assert_eq!(ring_opacity(0.), 0.);
        assert!((ring_opacity(0.5) - 1.).abs() < 0.001);
        assert!(ring_opacity(1.).abs() < 0.001);
        assert!(ring_opacity(7.) >= 0.);
    }

    #[test]
    fn the_ring_outline_fits_in_its_box_and_gives_up_on_a_box_with_no_room() {
        let bounds = Bounds {
            origin: point(px(2.), px(3.)),
            size: size(px(80.), px(30.)),
        };
        assert!(ring_path(bounds, px(8.)).is_some());
        // A radius larger than the box is held to what fits.
        assert!(ring_path(bounds, px(500.)).is_some());
        let none = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(1.), px(1.)),
        };
        assert!(ring_path(none, px(4.)).is_none());
    }

    struct Frame {
        from: u64,
        to: u64,
        turn: u64,
    }

    impl Render for Frame {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let t = theme::tokens(cx).clone();
            let (from, to, turn) = (self.from, self.to, self.turn);
            div()
                .relative()
                .size_full()
                .child(roll("viz-test-roll", from, to, |n| format!("{n} files").into(), cx))
                .child(spark(&[0, 3, 1, 5, 2], t.primary))
                .child(glint_ring("viz-test-ring", turn, px(8.), cx))
        }
    }

    fn draws_twice(cx: &mut TestAppContext, reduce_motion: bool) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let (mut visual, _frame) = testing::open_window(cx, |_, cx| cx.new(|_| Frame { from: 0, to: 40, turn: 1 }));
        visual.update(|_, cx| cx.set_reduce_motion(reduce_motion));
        visual.update(|window, _| window.refresh());
        visual.update(|window, cx| window.render_frame(cx));
        visual.update(|window, cx| window.render_frame(cx));
    }

    #[gpui_kit::test]
    fn a_frame_with_a_roll_a_spark_and_a_ring_draws_with_motion_off(cx: &mut TestAppContext) {
        draws_twice(cx, true);
    }

    #[gpui_kit::test]
    fn a_frame_with_a_roll_a_spark_and_a_ring_draws_while_they_play(cx: &mut TestAppContext) {
        draws_twice(cx, false);
    }
}
