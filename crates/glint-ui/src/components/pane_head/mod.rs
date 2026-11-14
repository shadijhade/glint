//! The head of a pane: the folder's picture and name, large; what is in it, in a line; and a chip for every kind of
//! thing in it, which shows only that kind when pressed. Under them a thin band, the folder's make-up by kind.
//!
//! The address field says where you are as a path. This says it as a place: its name in the largest type in the
//! window, with its own picture (a known folder's is in its color), and it answers "what is here?" before a row is
//! read. The chips are the breakdown the status bar keeps in its popover, brought to where it is seen: pressing
//! "Images" puts the kind's filter in the search box (pressing it again takes it out).
//!
//! # How the workspace wires it
//!
//! One [`PaneHead`] per pane, over the pane's bars, for the views that show a folder's items (the list, the grid,
//! the columns). It draws nothing for This PC (its own page has its headings) and while a search's results are shown
//! (the search bar is their head). It emits nothing: it reads the explorer and sets the tab's filter itself.

use crate::components::status_bar::summary::{chip_active, chip_title, items, spectrum_of, Category, FOLDERS, SAMPLE};
use crate::components::tab_strip::{glyph_of, name_of, Glyph};
use crate::explorer::{self, Explorer, Tab, TabId};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::format::{format_count, format_size};
use std::cell::Cell;
use std::rc::Rc;

#[cfg(test)]
mod tests;

/// How many kinds get a chip: the commonest ones; the band shows them all.
const CHIPS: usize = 6;
/// A window shorter than this has the head on one line (the name, then the facts beside it), so a small window
/// keeps its room for the files.
const SHORT_WINDOW: Pixels = px(620.);

/// What the head says, worked out when the explorer changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub tab: TabId,
    glyph: Glyph,
    /// The place's name.
    pub title: SharedString,
    /// "6 folders · 15 files · 12.6 MB".
    pub facts: SharedString,
    /// The kinds, commonest first.
    pub kinds: Vec<Category>,
    /// The search box's text: the chip whose filter it is, is lit.
    pub filter: String,
}

/// The line under the name: how many folders and files and how much they weigh; while the box narrows the list,
/// how many of them are shown; for a folder too large to count by kind, how many items.
pub fn facts(tab: &Tab, kinds: &[Category]) -> SharedString {
    let entries = tab.entries().len();
    if tab.is_closed() && tab.listing.is_none() {
        return t!("No access");
    }
    if !tab.filter.trim().is_empty() {
        return t!(
            "{shown} of {count} shown",
            shown = format_count(tab.rows.len()),
            count = format_count(entries)
        );
    }
    if entries == 0 && tab.listing.is_none() {
        return SharedString::default();
    }
    holds(entries, kinds)
}

/// What a folder of `entries` items holds, in words: "6 folders · 15 files · 12.6 MB", "Empty", or how many items
/// when it is too large to count by kind (`kinds` is the breakdown of its first [`SAMPLE`] entries).
pub fn holds(entries: usize, kinds: &[Category]) -> SharedString {
    if entries == 0 {
        return t!("Empty");
    }
    if entries > SAMPLE {
        return items(entries);
    }
    let folders = kinds
        .iter()
        .find(|kind| kind.group.name == FOLDERS.name)
        .map_or(0, |kind| kind.count);
    let files = kinds.iter().map(|kind| kind.count).sum::<usize>() - folders;
    let bytes: u64 = kinds
        .iter()
        .filter(|kind| kind.group.name != FOLDERS.name)
        .map(|kind| kind.bytes)
        .sum();
    let mut parts: Vec<String> = Vec::new();
    match folders {
        0 => {}
        1 => parts.push(t!("1 folder").to_string()),
        n => parts.push(t!("{count} folders", count = format_count(n)).to_string()),
    }
    match files {
        0 => {}
        1 => parts.push(t!("1 file").to_string()),
        n => parts.push(t!("{count} files", count = format_count(n)).to_string()),
    }
    if bytes > 0 {
        parts.push(format_size(bytes));
    }
    parts.join(" · ").into()
}

impl Head {
    /// The head of pane `pane`; None when the pane shows This PC or a search's results.
    pub fn of(explorer: &Explorer, pane: usize) -> Option<Head> {
        let tab = explorer.pane_tab(pane)?;
        if tab.find.is_some() || tab.is_this_pc() {
            return None;
        }
        // The folder's make-up: the breakdown the status bar's popover shows, of the same entries.
        let kinds = tab
            .listing
            .as_ref()
            .map(|listing| spectrum_of(&listing.entries, explorer.size_run(&tab.path)))
            .unwrap_or_default();
        Some(Head {
            tab: tab.id,
            glyph: glyph_of(explorer, Some(tab)),
            title: name_of(explorer, Some(tab)).into(),
            facts: facts(tab, &kinds),
            kinds,
            filter: tab.filter.clone(),
        })
    }
}

/// The head of one pane.
pub struct PaneHead {
    explorer: Entity<Explorer>,
    pane: usize,
    head: Option<Head>,
    /// Counts the places shown, so each arrival is its own animation.
    arrivals: usize,
    /// The explorer changed since the head was worked out.
    dirty: bool,
    /// How wide the chips' place was when last drawn, in pixels: as many chips as fit it are drawn.
    room: Rc<Cell<f32>>,
    _subscriptions: Vec<Subscription>,
}

impl PaneHead {
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let subscriptions = vec![cx.observe(&explorer, |this: &mut Self, _, cx| {
            this.dirty = true;
            cx.notify();
        })];
        PaneHead {
            explorer,
            pane,
            head: None,
            arrivals: 0,
            dirty: true,
            room: Rc::default(),
            _subscriptions: subscriptions,
        }
    }

    /// What the head says now (tests).
    #[cfg(test)]
    pub fn head(&mut self, cx: &App) -> Option<&Head> {
        self.refresh(cx);
        self.head.as_ref()
    }

    fn refresh(&mut self, cx: &App) {
        let _span = crate::perf::span("head refresh");
        if self.dirty {
            let head = Head::of(self.explorer.read(cx), self.pane);
            let place = |head: &Option<Head>| head.as_ref().map(|head| (head.tab, head.title.clone()));
            if place(&head) != place(&self.head) {
                self.arrivals += 1;
            }
            self.head = head;
            self.dirty = false;
        }
    }

    /// A kind's chip: shows only that kind, or everything again when it already does.
    fn toggle(&mut self, tab: TabId, query: &str, cx: &mut Context<Self>) {
        if query.is_empty() {
            return;
        }
        let shown = self.head.as_ref().is_some_and(|head| head.filter.trim() == query);
        let next = if shown { "" } else { query };
        self.explorer
            .update(cx, |explorer, cx| explorer.set_filter(next, Some(tab), cx));
    }

    fn chip(&self, i: usize, head: &Head, kind: &Category, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let lit = chip_active(kind, &head.filter);
        let color = theme::hsl(kind.group.hue as f32, kind.group.sat as f32, 54.);
        let title = chip_title(kind);
        // Read out as what it is and how many: "Images: 2".
        let said = t!(
            "{name}: {count}",
            name = t!(kind.group.name),
            count = format_count(kind.count)
        );
        let (tab, query) = (head.tab, kind.query.clone());
        Button::new(("pane-head-chip", i))
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(if lit { color.opacity(0.2) } else { t.accent.opacity(0.6) })
                    .foreground(if lit { t.foreground } else { t.muted_foreground })
                    .hover(if lit { color.opacity(0.28) } else { t.pressed })
                    .active(if lit { color.opacity(0.28) } else { t.pressed }),
            )
            .compact()
            .rounded(px(999.))
            .border_1()
            .border_color(if lit { color.opacity(0.55) } else { t.stroke })
            .disabled(kind.query.is_empty())
            .tooltip(title)
            .accessibility_label(said)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(tab, &query, cx)))
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.5))
                    .px(space(1.))
                    .text_size(theme::size::XS)
                    .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                    .child(t!(kind.group.name))
                    .child(div().opacity(0.7).child(format_count(kind.count))),
            )
            .into_any_element()
    }
}

/// How many of the first chips fit `room` pixels. A chip's width is reckoned from its words, a little over what it
/// draws at, so one that is counted in is whole on screen (and the row hides any that still doesn't fit).
pub fn fitting(kinds: &[Category], room: f32, rem: f32) -> usize {
    let mut used = 0.;
    for (i, kind) in kinds.iter().take(CHIPS).enumerate() {
        let letters = t!(kind.group.name).chars().count() + format_count(kind.count).chars().count() + 1;
        used += (letters as f32 * 0.42 + 3.2) * rem + if i > 0 { rem * 0.375 } else { 0. };
        if used > room {
            return i;
        }
    }
    kinds.len().min(CHIPS)
}

/// The band's parts: each kind's share of the items, none so thin it can't be seen, together the whole width.
pub fn shares(kinds: &[Category]) -> Vec<f32> {
    let total = kinds.iter().map(|kind| kind.count).sum::<usize>().max(1) as f32;
    let raw: Vec<f32> = kinds
        .iter()
        .map(|kind| (kind.count as f32 / total).max(0.015))
        .collect();
    let sum: f32 = raw.iter().sum();
    raw.into_iter().map(|share| share / sum).collect()
}

impl Render for PaneHead {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = crate::perf::span("pane head");
        self.refresh(cx);
        let Some(head) = self.head.clone() else {
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let rem = theme::rem_size(cx);
        let short = window.viewport_size().height < SHORT_WINDOW;
        let title = div()
            .id("pane-head-title")
            .role(Role::Heading)
            .aria_level(1)
            .aria_label(head.title.clone())
            .test_support()
            .min_w_0()
            .truncate()
            .text_size(rems(if short { 1.0625 } else { 1.375 }))
            .line_height(rems(if short { 1.5 } else { 1.75 }))
            .font_weight(FontWeight::SEMIBOLD)
            .child(isolate(head.title.clone()));
        let facts = div()
            .id("pane-head-facts")
            .test_support()
            .truncate()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            // Counts, words and a size: in Arabic the line reads from the right, numbers and "MB" in their places.
            .child(crate::kit::rtl_line(head.facts.clone()));
        // The name over the facts; in a short window, beside them (the name whole, the facts giving way).
        let name = if short {
            row_start()
                .items_center()
                .gap(space(2.5))
                .child(title.flex_none().max_w(relative(0.7)))
                .child(facts.flex_shrink(1.).min_w_0())
        } else {
            col().child(title).child(facts)
        }
        .flex_none()
        .max_w(relative(0.62))
        .min_w_0();
        // The chips at the end edge. Those that don't fit go to a second line, which isn't shown: a pane too narrow
        // for all of them shows the commonest whole, never half of one.
        let fit = fitting(&head.kinds, self.room.get(), f32::from(rem));
        let (room, id) = (self.room.clone(), cx.entity_id());
        let chips = row_start()
            .id("pane-head-kinds")
            .relative()
            .flex_1()
            .min_w_0()
            .h(rems(2.))
            .flex_wrap()
            .justify_end()
            .gap_x(space(1.5))
            .gap_y(space(4.))
            .overflow_hidden()
            .children(
                head.kinds
                    .iter()
                    .take(fit)
                    .enumerate()
                    .map(|(i, kind)| self.chip(i, &head, kind, &t, cx)),
            )
            // The place's width, for the next frame (asked for when it changed: a redraw asked for while drawing
            // is dropped).
            .child(
                canvas(
                    move |bounds, window, _| {
                        let width = f32::from(bounds.size.width);
                        if (room.get() - width).abs() > 0.5 {
                            room.set(width);
                            window.on_next_frame(move |_, cx| cx.notify(id));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        // The band: the folder's make-up, each kind in its color.
        let band = row_start()
            .id("pane-head-band")
            .w_full()
            .h(px(3.))
            .gap(px(2.))
            .children(head.kinds.iter().zip(shares(&head.kinds)).map(|(kind, share)| {
                div()
                    .h_full()
                    .flex_shrink(1.)
                    .w(relative(share))
                    .rounded_full()
                    .bg(theme::hsl(kind.group.hue as f32, kind.group.sat as f32, 54.).opacity(0.85))
            }));
        // Arriving: the name comes in from the start edge as it clears, and the band grows to its width.
        let (name_row, band) = (
            row_start()
                .w_full()
                .items_center()
                .gap(space(3.))
                .child(head.glyph.picture((rem * if short { 1.5 } else { 2.5 }).round(), cx))
                .child(name)
                .child(chips),
            div().w_full().child(band),
        );
        let (name_row, band) = if cx.reduce_motion() {
            (name_row.into_any_element(), band.into_any_element())
        } else {
            use gpui_kit::{Animation, AnimationExt as _};
            let rtl = crate::i18n::is_rtl();
            let arriving = Animation::new(theme::motion::SLOW).with_easing(theme::motion::enter);
            (
                name_row
                    .relative()
                    .with_animation(("pane-head-in", self.arrivals), arriving.clone(), move |row, delta| {
                        let away = px(12. * (1. - delta));
                        let row = row.opacity(0.35 + 0.65 * delta);
                        if rtl {
                            row.right(away)
                        } else {
                            row.left(away)
                        }
                    })
                    .into_any_element(),
                band.with_animation(("pane-head-band", self.arrivals), arriving, |band, delta| {
                    band.w(relative(0.25 + 0.75 * delta)).opacity(delta)
                })
                .into_any_element(),
            )
        };
        col()
            .id(("pane-head", self.pane))
            .role(Role::Group)
            .aria_label(t!("About this folder"))
            .test_support()
            .w_full()
            .px(space(5.))
            .pt(space(if short { 2. } else { 3.5 }))
            .pb(space(if short { 1.5 } else { 2. }))
            .gap(space(if short { 1.5 } else { 2.5 }))
            .child(name_row)
            .child(band)
            .into_any_element()
    }
}
