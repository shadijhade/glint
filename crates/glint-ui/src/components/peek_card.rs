//! The peek card: what a folder holds, shown beside a tab or a breadcrumb the pointer rests on, so another tab or
//! a parent folder can be looked into without leaving the view.
//!
//! It is a micro-card (the folder's name and path, then what is in it in the head's words, "6 folders · 15 files ·
//! 12.6 MB", over the folder's make-up band) and it is only ever said from what is already known: the listing of
//! a tab that shows the folder, or the listing the explorer read ahead (a folder under a hovering crumb is asked
//! for as soon as the card opens, and the card fills in when it arrives). Nothing waits for it, and nothing is
//! drawn for what is not known: no spinner, no dots.
//!
//! It is quiet. It opens after [`DWELL`] of the pointer resting on its element, arrives in a short fade, never
//! takes the focus and never catches the pointer, and goes with the pointer leaving, a press, Escape, or a drag
//! starting. Moving between two elements that ask for the same card (a crumb and its arrow) keeps it.
//!
//! A view that wants cards keeps one [`PeekHost`] (an entity, drawn as a child of the view) and tells it where the
//! pointer is with [`PeekHost::enter`] and [`PeekHost::leave`].

use crate::components::pane_head::{holds, shares};
use crate::components::status_bar::summary::spectrum_sized;
use crate::explorer::{Explorer, Listed};
use crate::kit::prelude::*;
use crate::{i18n, theme};
use glint_model::paths::same_path;
use std::time::Duration;

/// How long the pointer rests on a tab or a crumb before its card opens.
pub const DWELL: Duration = Duration::from_millis(400);

/// How long a card stays after the pointer left, so that moving to the next element asking for the same card
/// (a crumb to its arrow) doesn't close and reopen it.
const GRACE: Duration = Duration::from_millis(90);

/// What an element asks the card to say.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Which card this is: two elements with one key (a crumb and its arrow) share the card.
    pub key: SharedString,
    /// The element the pointer is on (told apart from the other elements of the card, for [`PeekHost::leave`]).
    pub element: SharedString,
    /// The folder.
    pub path: String,
    /// Its name as the element shows it.
    pub name: SharedString,
    /// A line under the rest (what pressing the element does), if any.
    pub hint: Option<SharedString>,
    /// Ask the explorer to read the folder ahead when nothing is known of it (a folder on disk; not a remote or
    /// an archive, which cost a connection or a copy).
    pub read_ahead: bool,
}

/// What the card says of the folder's contents.
#[derive(Clone, Debug, PartialEq)]
struct Facts {
    /// "6 folders · 15 files · 12.6 MB".
    words: SharedString,
    /// Its make-up: each kind's color and share, commonest first.
    band: Vec<(Hsla, f32)>,
    /// Where the listing's entries are, to tell a listing from another.
    at: usize,
}

impl Facts {
    /// What `listed` holds, in the head's words and colors (as a folder's row says it: `file_list::Peek`).
    fn of(listed: &Listed) -> Facts {
        let kinds = spectrum_sized(&listed.entries, None);
        let band = kinds
            .iter()
            .zip(shares(&kinds))
            .map(|(kind, share)| {
                let color = theme::hsl(kind.group.hue as f32, kind.group.sat as f32, 54.).opacity(0.85);
                (color, share)
            })
            .collect();
        Facts {
            words: holds(listed.entries.len(), &kinds),
            band,
            at: listed.entries.as_ptr() as usize,
        }
    }
}

/// What is known of the folder at `path`: a tab that shows it, else what was read ahead.
fn known(explorer: &Explorer, path: &str) -> Option<Facts> {
    let shown = explorer
        .panes()
        .iter()
        .flat_map(|pane| pane.tabs.iter())
        .filter_map(|id| explorer.tab(*id))
        .filter(|tab| tab.find.is_none())
        .filter_map(|tab| tab.listing.as_ref())
        .find(|listed| same_path(&listed.path, path));
    let listed = shown.or_else(|| {
        explorer
            .peeks()
            .into_iter()
            .find(|(peeked, _)| same_path(peeked, path))
            .map(|(_, listed)| listed)
    })?;
    Some(Facts::of(listed))
}

/// A card on screen.
struct Shown {
    target: Target,
    /// Where the pointer was when it opened: the card hangs by it.
    at: Point<Pixels>,
    facts: Option<Facts>,
    /// How many cards have opened, so that each one's arrival plays once.
    arrival: u64,
}

/// The card, and the wait before it. One for a view; it draws nothing while no card is open.
pub struct PeekHost {
    explorer: Entity<Explorer>,
    /// The elements the pointer is on (of one card: another card starts over) ...
    hovering: Vec<SharedString>,
    /// ... and the wait before the card opens (or, for a card that is open, before it closes after the pointer left).
    wait: Option<Task<()>>,
    /// The card waiting to open.
    pending: Option<SharedString>,
    shown: Option<Shown>,
    arrivals: u64,
    _subscriptions: Vec<Subscription>,
}

impl PeekHost {
    /// A host reading `explorer`.
    pub fn new(explorer: Entity<Explorer>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // What was read ahead arrives: an open card fills in. (Nothing is worked out while no card is open.)
            cx.observe(&explorer, |this, _, cx| this.refresh(cx)),
            // Escape closes it, wherever the keyboard is.
            cx.observe_keystrokes(|this, event, _, cx| {
                if this.shown.is_some() && event.keystroke.key == "escape" {
                    this.hide(cx);
                }
            }),
        ];
        PeekHost {
            explorer,
            hovering: Vec::new(),
            wait: None,
            pending: None,
            shown: None,
            arrivals: 0,
            _subscriptions: subscriptions,
        }
    }

    /// The pointer came onto an element that has a card: it opens after [`DWELL`] (at once when the card for this
    /// key is already open).
    pub fn enter(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        // A drag in progress is not a rest.
        if cx.has_active_drag() {
            return;
        }
        let same_card = self
            .shown
            .as_ref()
            .map(|shown| &shown.target.key)
            .or(self.pending.as_ref())
            == Some(&target.key);
        if !same_card {
            self.hovering.clear();
        }
        if !self.hovering.contains(&target.element) {
            self.hovering.push(target.element.clone());
        }
        if self.shown.as_ref().is_some_and(|shown| shown.target.key == target.key) {
            // The same card, from another element of it: it stays, and the pending close is dropped.
            self.wait = None;
            return;
        }
        if self.shown.is_none() && self.pending.as_ref() == Some(&target.key) {
            // The same card, still waiting to open: its dwell goes on.
            return;
        }
        // Another card: the old one goes now, the new one comes after its dwell.
        if self.shown.take().is_some() {
            cx.notify();
        }
        self.pending = Some(target.key.clone());
        self.wait = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DWELL).await;
            this.update_in(cx, |this, window, cx| this.open(target, window.mouse_position(), cx))
                .ok();
        }));
    }

    /// The pointer left `element`. Its card goes a moment later, unless the pointer is on another element of the
    /// card by then (a crumb's arrow, say).
    pub fn leave(&mut self, element: &str, cx: &mut Context<Self>) {
        let Some(at) = self.hovering.iter().position(|e| e == element) else {
            return;
        };
        self.hovering.remove(at);
        if !self.hovering.is_empty() {
            return;
        }
        if self.shown.is_none() {
            self.pending = None;
            // Not open yet: the wait is over.
            self.wait = None;
            return;
        }
        self.wait = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GRACE).await;
            this.update(cx, |this, cx| this.hide(cx)).ok();
        }));
    }

    /// Closes the card and forgets the wait: a press, Escape, a drag, the view moving on.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.hovering.clear();
        self.pending = None;
        self.wait = None;
        if self.shown.take().is_some() {
            cx.notify();
        }
    }

    /// Whether a card is on screen.
    pub fn is_open(&self) -> bool {
        self.shown.is_some()
    }

    fn open(&mut self, target: Target, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.wait = None;
        self.pending = None;
        let facts = known(self.explorer.read(cx), &target.path);
        // Nothing known of it: have it read ahead, and the card fills in when it arrives.
        if facts.is_none() && target.read_ahead {
            let path = target.path.clone();
            self.explorer
                .update(cx, |explorer, cx| explorer.prefetch_folder(&path, cx));
        }
        self.arrivals += 1;
        self.shown = Some(Shown {
            target,
            at,
            facts,
            arrival: self.arrivals,
        });
        cx.notify();
    }

    /// What is known of the folder changed (a listing arrived): the open card says it, if it is new.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(shown) = self.shown.as_ref() else {
            return;
        };
        let facts = known(self.explorer.read(cx), &shown.target.path);
        if facts != shown.facts {
            if let Some(shown) = self.shown.as_mut() {
                shown.facts = facts;
            }
            cx.notify();
        }
    }

    /// The card's words for a screen reader: name, path, what it holds.
    fn describe(shown: &Shown) -> String {
        let mut parts = vec![shown.target.name.to_string(), shown.target.path.clone()];
        parts.extend(shown.facts.iter().map(|facts| facts.words.to_string()));
        parts.extend(shown.target.hint.iter().map(ToString::to_string));
        parts.join(", ")
    }
}

impl Render for PeekHost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shown) = self.shown.as_ref() else {
            return div().hidden().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let rtl = i18n::is_rtl();
        let (anchor, dx) = if rtl {
            (Anchor::TopRight, -16.)
        } else {
            (Anchor::TopLeft, 16.)
        };
        let label = Self::describe(shown);
        let band = shown.facts.as_ref().map(|facts| {
            row_start()
                .id("peek-card-band")
                .w_full()
                .h(px(4.))
                .gap(px(1.))
                .children(facts.band.iter().map(|(color, share)| {
                    div()
                        .h_full()
                        .flex_shrink(1.)
                        .w(relative(*share))
                        .rounded_full()
                        .bg(*color)
                }))
        });
        let words = shown.facts.as_ref().map(|facts| {
            div()
                .id("peek-card-words")
                .max_w_full()
                .truncate()
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(crate::kit::rtl_line(facts.words.clone()))
        });
        let arrival = shown.arrival;
        let card = col()
            .id("peek-card")
            .role(Role::Tooltip)
            .aria_label(label)
            .test_support()
            .align_start()
            .gap(space(1.))
            .min_w(rems(13.))
            .max_w(rems(22.))
            .px(space(3.))
            .py(space(2.5))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.stroke)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow(theme::elevation::floating(cx))
            .child(
                div()
                    .id("peek-card-name")
                    .max_w_full()
                    .truncate()
                    .text_size(theme::size::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .child(crate::kit::isolate(shown.target.name.clone())),
            )
            .child(
                div()
                    .id("peek-card-path")
                    .max_w_full()
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(crate::kit::isolate(shown.target.path.clone())),
            )
            .children(band)
            .children(words)
            .children(shown.target.hint.clone().map(|hint| {
                div()
                    .id("peek-card-hint")
                    .max_w_full()
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(hint)
            }))
            // It arrives readable from its first frame: a third of its opacity at least, a few pixels to move.
            .with_animation(
                ("peek-card-in", arrival),
                Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
                |card, delta| card.opacity(0.35 + 0.65 * delta).mt(px(4. * (1. - delta))),
            );
        deferred(
            anchored()
                .position(shown.at)
                .anchor(anchor)
                .offset(point(px(dx), px(18.)))
                .snap_to_window_with_margin(px(8.))
                .child(card),
        )
        .with_priority(30)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests;
