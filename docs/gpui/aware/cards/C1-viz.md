# Card C1 — The rolling number, the spark line, the glint ring

Wave 1 · crate `glint-ui` · drawing only

**You own:** `crates/glint-ui/src/components/viz.rs` (its tests at the bottom of the file).
**Read first:** STANDARDS.md §5.1 ("Style", "Motion", "Measuring during a draw", "Tests"); PLAN.md §2 (R9); `crates/glint-ui/src/theme.rs` lines 82 to 150 (`motion`) and 632 to 660 (`glint_fill`); `crates/glint-ui/src/kit.rs:121` (`loading_sweep`: how an animation is skipped when motion is off); `crates/glint-ui/src/components/preview/player/wave.rs:191` (a `canvas` that paints quads); in the vendored GPUI: `vendor/gpui-pre-0.3.12/src/path_builder.rs` (`PathBuilder::stroke`, `move_to`, `line_to`, `arc_to`, `build`), `src/color.rs:865` (`linear_gradient`, `linear_color_stop`), `src/window.rs:5183` (`paint_path`), `src/elements/animation.rs` (`Animation`, `with_animation`).
**Check with:** `cargo test -p glint-ui components::viz` and `cargo clippy -p glint-ui --all-targets -- -D warnings`.

Three small things other cards put in the dock. They take theme colors as arguments or read `theme::tokens(cx)`; they hold no state.

## `roll` — a number that rolls to its new value (F10)

```rust
pub fn roll(name: &'static str, from: u64, to: u64, words: impl Fn(u64) -> SharedString + 'static, cx: &App) -> AnyElement
```

- Draws `words(value)` in a `div()`, where `value` goes from `from` to `to` over `theme::motion::BASE` on `theme::motion::glide`. The animation's id is `(name, to as usize)`, so it plays once for each new target and not again on a redraw.
- `from == to`, or motion is off (do what `kit::loading_sweep` does to know): a plain `div().child(words(to))`, no animation.
- The value at a moment is a pure function, public for the tests: `pub fn roll_value(from: u64, to: u64, delta: f32) -> u64` (`delta` 0 gives `from`, 1 gives `to`, between them linear in `f64`, rounded; works when `to < from`; never overflows).

## `spark` — a line of the last counts (F11)

```rust
pub fn spark(values: &[u8], color: Hsla) -> Div
```

- A `div()` 44 px wide and 16 px high (the caller may resize it) holding a `canvas` that strokes one line 1.5 px wide through the values, oldest at the start edge, newest at the end (mirrored when `crate::i18n::is_rtl()`).
- The points are a pure function, public for the tests: `pub fn spark_points(values: &[u8], width: f32, height: f32) -> Vec<(f32, f32)>`: x from 0 to `width` in equal steps; y is `height - 1 - (value / max) * (height - 2)`, where `max` is the largest value, at least 1 (so all zeros is a flat line one pixel above the bottom). Fewer than two values: no points, and nothing is drawn.

## `glint_ring` — the glint's light turning once round a box (F8)

```rust
pub fn glint_ring(name: &'static str, turn: u64, radius: Pixels, cx: &App) -> AnyElement
```

- An `absolute().inset_0()` layer (the caller's box is `relative()`) that takes no pointer events: it only paints.
- It paints the outline of a rounded rectangle (the layer's bounds, half a stroke in from the edge, corners of `radius`), 1.5 px wide, with `linear_gradient(angle, linear_color_stop(t.glint_from, 0.), linear_color_stop(t.glint_to.opacity(0.), 1.))`.
- The animation's id is `(name, turn as usize)`: a new `turn` plays it once, over 700 ms, linear: the gradient's `angle` goes from 0 to 360 and the layer's opacity is `sin(π × delta)` (nothing at the start, full halfway, nothing at the end). So at rest it draws nothing (R9).
- Motion off: draws nothing at all.
- Pure, public for the tests: `pub fn ring_angle(delta: f32) -> f32` and `pub fn ring_opacity(delta: f32) -> f32` (both clamp `delta` to 0..=1).

Build the outline with `PathBuilder::stroke(px(1.5))`: `move_to` the top edge's start, then for each side a `line_to` and for each corner an `arc_to` (or a `cubic_bezier_to`; a quarter circle's control points are 0.5523 of the radius from the corner). If `build()` fails, draw nothing.

## Tests

1. `roll_value`: `(0, 100, 0.0) == 0`, `(0, 100, 1.0) == 100`, `(0, 100, 0.5) == 50`, `(100, 0, 0.25) == 75`, `(u64::MAX - 1, u64::MAX, 0.5)` doesn't panic, a `delta` of 2.0 is treated as 1.0.
2. `spark_points`: no points for one value; for `[0, 2, 4]` in 44 × 16 the x's are 0, 22, 44 and the y's 15, 8, 1; all zeros is flat at y 15; 30 values give 30 points with x never decreasing.
3. `ring_angle(0.5) == 180`, `ring_opacity(0.) == 0`, `ring_opacity(0.5)` is within 0.001 of 1, `ring_opacity(1.)` is within 0.001 of 0.
4. In a headless window (`testing::init`, `testing::open_window`, the lock held), a view whose render returns a `relative()` box with `roll`, `spark` and `glint_ring` in it draws a frame without panicking (`window.render_frame(cx)` twice).

## Done when

The tests pass, clippy is clean, the file has no `gpui_component` import and no color that isn't a token or an argument.
