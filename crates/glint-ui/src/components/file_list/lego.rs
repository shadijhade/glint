//! File Lego's drop badge, for the list and the grid: a ring that fills while the pointer rests on a file another
//! can be combined with, and the pill that says what the drop will do once it is ready.
//!
//! The ring is the dwell made visible (`incoming::LEGO_DWELL`, 250 ms): it fills at the dwell's own pace, so it is
//! full when the recipe arms, and the cue ([`snap`]) is felt at that moment. It is the same size as the badge that
//! follows it and both sit over the row's end (an overlay, never in the row's flow), so nothing is pushed aside as
//! the badge goes from ring to pill. With the system's animations off the ring isn't drawn and the pill is there
//! when the recipe arms, which is the same state the ring would have landed on.

use super::incoming::LEGO_DWELL;
use crate::kit::prelude::*;
use crate::platform::haptics::{self, HapticCue};
use crate::theme::Tokens;
use crate::{state, t, theme};
use glint_model::file_lego::LegoRecipe;
use gpui_kit::AnimationExt as _;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::sync::atomic::{AtomicU64, Ordering};

/// The ring's width: the badge's round part is this wide and high.
const SIZE: f32 = 5.;
/// How wide the ring's line is.
const STROKE: f32 = 2.;

/// How long the pointer rests on a file before the recipe arms (the ring's time to fill).
pub(crate) const DWELL: std::time::Duration = LEGO_DWELL;

static TURNS: AtomicU64 = AtomicU64::new(1);

/// A number for a new dwell: keys the ring's animation, so it plays once for each.
pub(crate) fn next_turn() -> u64 {
    TURNS.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
thread_local! {
    /// Tests only: how many times the cue was asked for.
    pub(crate) static SNAPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The cue for a recipe arming (a touchpad's click, if the PC has one and the setting is on).
pub(crate) fn snap(cx: &App) {
    #[cfg(test)]
    SNAPS.with(|snaps| snaps.set(snaps.get() + 1));
    haptics::trigger(HapticCue::BasketSnap, state::try_settings(cx).is_none_or(|s| s.haptics));
}

/// What the pill says: the recipe's verb and the name of what it makes (the real one, once the disk has said it:
/// `output`), or its own summary where it makes nothing (a comparison, a check).
pub(crate) fn pill_text(recipe: &LegoRecipe, output: Option<&str>) -> SharedString {
    match recipe.makes(output) {
        Some(name) => t!("{verb} → {name}", verb = recipe.verb, name = isolate(name)),
        None => SharedString::from(recipe.summary.clone()),
    }
}

/// Asks the disk, off the UI thread, for the real name a mux of `recipe` will make: `clip_muxed (2).mp4` when
/// `clip_muxed.mp4` is taken. `done` takes the name back on the UI thread. None for recipes that make nothing.
pub(crate) fn lookup_output<T: 'static>(
    recipe: &LegoRecipe,
    cx: &mut Context<T>,
    done: impl FnOnce(&mut T, Option<String>, &mut Context<T>) + 'static,
) -> Option<gpui_kit::Task<()>> {
    let glint_model::file_lego::LegoAction::MuxMedia { video, .. } = &recipe.action else {
        return None;
    };
    let video = video.clone();
    Some(cx.spawn(async move |this, cx| {
        let name = cx
            .background_executor()
            .spawn(async move { glint_core::file_lego::mux_output_name(&video) })
            .await;
        let _ = this.update(cx, |this, cx| done(this, name, cx));
    }))
}

/// The ring on a round layer: faint all the way round, and `fraction` (0 to 1) of it lit from the top.
fn ring(fraction: f32, track: Hsla, lit: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window, _| {
            if let Some(path) = arc(bounds, 1.) {
                window.paint_path(path, track);
            }
            if let Some(path) = arc(bounds, fraction) {
                window.paint_path(path, lit);
            }
        },
    )
    .size_full()
}

/// `fraction` of a circle's line inside `bounds`, from the top, clockwise (or None when there is nothing to draw).
fn arc(bounds: Bounds<Pixels>, fraction: f32) -> Option<Path<Pixels>> {
    let fraction = if fraction.is_nan() { 0. } else { fraction.clamp(0., 1.) };
    let radius = f32::from(bounds.size.width.min(bounds.size.height)) / 2. - STROKE / 2.;
    if fraction <= 0. || radius <= 0. {
        return None;
    }
    let centre = bounds.center();
    let steps = ((fraction * 64.).ceil() as usize).max(2);
    let mut path = PathBuilder::stroke(px(STROKE));
    for step in 0..=steps {
        let angle = -FRAC_PI_2 + fraction * TAU * (step as f32 / steps as f32);
        let at = point(centre.x + px(radius * angle.cos()), centre.y + px(radius * angle.sin()));
        if step == 0 {
            path.move_to(at);
        } else {
            path.line_to(at);
        }
    }
    if fraction >= 1. {
        path.close();
    }
    path.build().ok()
}

/// The round part of the badge: the sparkle in a ring that is filling (`dwell` is its turn's number) or full.
fn round(id: (&'static str, usize), dwell: Option<u64>, t: &Tokens, cx: &App) -> impl IntoElement {
    let (track, lit) = (t.primary.opacity(0.25), t.primary);
    let layer = div().absolute().inset_0();
    let layer = match dwell {
        // Filling: at the dwell's own pace, so it lands full as the recipe arms.
        Some(turn) if !cx.reduce_motion() => layer
            .with_animation(
                (id.0, turn as usize),
                gpui_kit::Animation::new(LEGO_DWELL),
                move |layer, delta| layer.child(ring(delta, track, lit)),
            )
            .into_any_element(),
        // Animations off while arming: only the faint track, as the ring's start.
        Some(_) => layer.child(ring(0., track, lit)).into_any_element(),
        None => layer.child(ring(1., track, lit)).into_any_element(),
    };
    div()
        .id(id)
        .relative()
        .size(space(SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(t.popover.opacity(0.92))
        .child(layer)
        .child(icon(IconName::Sparkles).size_3().text_color(t.primary))
}

/// The badge: the ring filling while a recipe arms (`words` None, `dwell` the turn), then the ring full with the
/// pill's `words` beside it. `id` names the element (the list's row, the grid's tile); the name a screen reader
/// reads is the words, or that the drop is getting ready.
pub(crate) fn badge(
    id: (&'static str, usize),
    dwell: u64,
    words: Option<SharedString>,
    t: &Tokens,
    cx: &App,
) -> AnyElement {
    match words {
        None => div()
            .id(id)
            .role(Role::Status)
            .aria_label(t!("Getting ready to combine"))
            .test_support()
            .child(round(("lego-ring", id.1), Some(dwell), t, cx))
            .into_any_element(),
        Some(words) => row_start()
            .id(id)
            .role(Role::Status)
            .aria_label(words.clone())
            .test_support()
            .flex_none()
            .max_w_full()
            .gap(space(1.5))
            .items_center()
            .pe(2.)
            .rounded_full()
            .bg(t.popover.opacity(0.94))
            .border_1()
            .border_color(t.primary.opacity(0.55))
            .text_size(theme::size::XXS)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.primary)
            .child(round(("lego-ring", id.1), None, t, cx))
            .child(div().min_w_0().truncate().child(words))
            .into_any_element(),
    }
}

/// The badge over a list row's end: vertically centred, beside the end edge, over the row's own cells.
pub(crate) fn row_overlay(badge: AnyElement) -> impl IntoElement {
    let layer = div().absolute().top_0().bottom_0().flex().items_center();
    let layer = if crate::i18n::is_rtl() {
        layer.left(px(6.))
    } else {
        layer.right(px(6.))
    };
    layer.child(badge)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::file_lego::classify_drop;

    #[test]
    fn the_pill_names_what_a_mux_makes_and_says_the_summary_otherwise() {
        let mux = classify_drop(&[r"C:\v\subs.srt".into()], r"C:\v\clip.mp4", false).unwrap();
        assert_eq!(pill_text(&mux, None), "Mux subtitles → clip_muxed.mp4");
        // The name the disk gave wins over the one that would be made on an empty folder.
        assert_eq!(
            pill_text(&mux, Some("clip_muxed (2).mp4")),
            "Mux subtitles → clip_muxed (2).mp4"
        );
        let compare = classify_drop(&[r"C:\a\v2.rs".into()], r"C:\a\v1.rs", false).unwrap();
        assert_eq!(pill_text(&compare, None), "Compare with v1.rs");
    }

    #[test]
    fn the_ring_has_a_line_to_draw_only_between_nothing_and_everything() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(20.), px(20.)));
        assert!(arc(bounds, 0.).is_none());
        assert!(arc(bounds, f32::NAN).is_none());
        assert!(arc(bounds, 0.4).is_some());
        assert!(arc(bounds, 1.).is_some());
        assert!(arc(bounds, 7.).is_some(), "past the end is the whole ring");
        let tiny = Bounds::new(point(px(0.), px(0.)), size(px(1.), px(1.)));
        assert!(arc(tiny, 1.).is_none(), "nothing fits in it");
    }

    #[test]
    fn each_dwell_gets_a_turn_of_its_own() {
        let (a, b) = (next_turn(), next_turn());
        assert_ne!(a, b);
    }
}
