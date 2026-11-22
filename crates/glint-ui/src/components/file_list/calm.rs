//! The look every view's message shares (the list, the grid and the columns): one picture, one line, one action,
//! centred. The words differ by state; the shape does not.

use crate::explorer::{Selection, TabId};
use crate::kit::prelude::*;
use crate::theme::{self, Tokens};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A centred message: the frame the picture, the line, the note and the action go in.
pub(crate) fn centered(t: &Tokens, id: &'static str) -> Stateful<Div> {
    col()
        .id(id)
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(2.))
        .p(space(6.))
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
        .text_center()
}

/// The big faded icon over an empty place.
pub(crate) fn faded(t: &Tokens, name: IconName) -> Icon {
    icon(name).size(rems(2.)).text_color(t.muted_foreground.opacity(0.6))
}

/// The one line: what is the matter, in the strong ink.
pub(crate) fn title(t: &Tokens, text: SharedString) -> Div {
    div()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.foreground)
        .max_w(rems(28.))
        .child(isolate(text))
}

/// How long a "Loading…" waits before it shows at all, and how long it then takes to fade in: a folder that opens in
/// a blink never shows it, so there is no flash of a spinner between two folders.
const BUSY_WAIT: Duration = Duration::from_millis(220);
const BUSY_FADE: Duration = Duration::from_millis(110);

/// `element` shown after [`BUSY_WAIT`], fading in; at once under reduced motion.
pub(crate) fn after_a_beat(id: &'static str, element: impl IntoElement, reduce_motion: bool) -> AnyElement {
    let element = div().size_full().child(element);
    if reduce_motion {
        return element.into_any_element();
    }
    let total = BUSY_WAIT + BUSY_FADE;
    element
        .with_animation(id, Animation::new(total), move |element, delta| {
            let passed = total.mul_f32(delta);
            let along = passed.saturating_sub(BUSY_WAIT).as_secs_f32() / BUSY_FADE.as_secs_f32();
            element.opacity(along.clamp(0., 1.))
        })
        .into_any_element()
}

/// The most rows that ease in at once: a "select all" of a folder just lands.
const MOST_EASED: usize = 200;
/// How long the overlay of an easing row is kept: the fade, and a little more.
const EASED_FOR: Duration = Duration::from_millis(170);

/// Which rows were selected just now, so their fill eases in (`motion::FAST`) instead of switching on. A view keeps
/// one and reads it each frame: a frame whose selection is the last frame's asks one pointer comparison and
/// returns the set it has, so a row that isn't changing costs nothing.
#[derive(Default)]
pub(crate) struct Eased {
    seen: Selection,
    tab: Option<TabId>,
    fresh: Arc<HashSet<String>>,
    since: Option<Instant>,
    /// Counts the changes, so each one's animations are new ones.
    turn: usize,
    wait: Option<Task<()>>,
}

impl Eased {
    /// The rows to ease in now (empty almost always) and the turn they belong to.
    pub(crate) fn read<V: 'static>(
        &mut self,
        tab: TabId,
        selection: &Selection,
        cx: &mut Context<V>,
    ) -> (Arc<HashSet<String>>, usize) {
        let now = cx.background_executor().now();
        if self.tab == Some(tab) && self.seen.same_paths(selection) {
            // Eased in: the rows' own fill is the same colour, the overlay can go.
            if self
                .since
                .is_some_and(|since| now.saturating_duration_since(since) > EASED_FOR)
            {
                self.fresh = Arc::new(HashSet::new());
                self.since = None;
            }
            return (self.fresh.clone(), self.turn);
        }
        let same_tab = self.tab == Some(tab);
        let fresh: HashSet<String> = if same_tab && !cx.reduce_motion() && selection.len() <= MOST_EASED {
            selection
                .iter()
                .filter(|path| !self.seen.contains(path))
                .map(str::to_string)
                .collect()
        } else {
            HashSet::new()
        };
        self.tab = Some(tab);
        self.seen = selection.clone();
        if fresh.is_empty() {
            self.fresh = Arc::new(HashSet::new());
            self.since = None;
            self.wait = None;
        } else {
            self.fresh = Arc::new(fresh);
            self.since = Some(now);
            self.turn = self.turn.wrapping_add(1) & 0xFFFF;
            // A frame after it has eased in takes the overlay off.
            self.wait = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(EASED_FOR + Duration::from_millis(20))
                    .await;
                this.update(cx, |_, cx| cx.notify()).ok();
            }));
        }
        (self.fresh.clone(), self.turn)
    }
}

/// The overlay that fades a just-selected row's fill in: `fill` painted at `along` (0 to 1) of its colour, under the
/// row's text.
pub(crate) fn easing_fill(
    id: (&'static str, usize),
    fill: impl Into<gpui_kit::Background>,
    radius: Pixels,
) -> impl IntoElement {
    let fill = fill.into();
    div().absolute().inset_0().rounded(radius).bg(fill).with_animation(
        id,
        Animation::new(theme::motion::FAST),
        |overlay, delta| overlay.opacity(theme::motion::enter(delta)),
    )
}
