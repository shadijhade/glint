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
use lenses::{Lens, Looked, Places};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

pub mod lenses;
pub(crate) mod pill;
#[cfg(test)]
mod tests;

/// How many kinds get a chip: the commonest ones; the band shows them all.
const CHIPS: usize = 6;
/// A window shorter than this has the head on one line (the name, then the facts beside it), so a small window
/// keeps its room for the files.
const SHORT_WINDOW: Pixels = px(620.);
/// How strong the band's colors are: it is a hairline across the whole pane, so it is told quietly.
const BAND_STRENGTH: f32 = 0.6;

/// What the head says, worked out when the explorer changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub tab: TabId,
    /// The folder's path: a different one is a different place (two folders can share a name).
    pub path: String,
    glyph: Glyph,
    /// The place's name.
    pub title: SharedString,
    /// "6 folders · 15 files · 12.6 MB".
    pub facts: SharedString,
    /// The kinds, commonest first.
    pub kinds: Vec<Category>,
    /// The search box's text: the chip whose filter it is, is lit.
    pub filter: String,
    /// True when the tab is backed by a real folder on disk (enables Time Machine / Map tools).
    pub is_folder: bool,
    /// True when the tab is displaying the unified Photo & Video Gallery (`shell:Gallery`).
    pub is_gallery: bool,
    /// How many pictures the folder's make-up counts (the Colors lens wants a dozen).
    pub images: usize,
    /// How many pictures and videos it counts (the Map lens looks into them for places, if there are any).
    pub media: usize,
}

/// The line under the name: how many folders and files and how much they weigh; while the box narrows the list,
/// how many of them are shown; for a folder too large to count by kind, how many items.
pub fn facts(tab: &Tab, kinds: &[Category]) -> SharedString {
    // What the list shows: the items "Show hidden items" leaves out aren't counted.
    let entries = tab.entries().len().saturating_sub(tab.rows.hidden);
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
        // The folder's make-up: the breakdown the status bar's popover shows, of the items the list shows (a
        // repository's hidden `.git` is no "1 folder" while the list has none).
        let kinds = tab
            .listing
            .as_ref()
            .map(|listing| {
                let sizes = explorer.size_run(&tab.path);
                if tab.rows.hidden > 0 {
                    spectrum_of(&shown_entries(&listing.entries), sizes)
                } else {
                    spectrum_of(&listing.entries, sizes)
                }
            })
            .unwrap_or_default();
        Some(Head {
            tab: tab.id,
            path: tab.path.clone(),
            images: lenses::images_of(&kinds),
            media: lenses::media_of(&kinds),
            glyph: glyph_of(explorer, Some(tab)),
            title: name_of(explorer, Some(tab)).into(),
            facts: facts(tab, &kinds),
            kinds,
            filter: tab.filter.clone(),
            is_folder: glint_model::paths::is_folder_path(&tab.path),
            is_gallery: glint_model::paths::is_gallery(&tab.path),
        })
    }
}

/// A listing's entries, shared.
type Entries = Arc<[glint_model::entries::FileEntry]>;

thread_local! {
    /// Listings whose hidden items were left out, and what is left, newest first: the same listing gives the same
    /// list (so the breakdown's own cache, keyed by it, keeps working) and a folder's refresh costs nothing.
    static SHOWN: std::cell::RefCell<Vec<(Entries, Entries)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `entries` without the hidden ones, kept for the listing (one per pane or two).
fn shown_entries(entries: &Entries) -> Entries {
    SHOWN.with(|kept| {
        let mut kept = kept.borrow_mut();
        if let Some(at) = kept.iter().position(|(from, _)| Arc::ptr_eq(from, entries)) {
            let hit = kept.remove(at);
            let shown = hit.1.clone();
            kept.insert(0, hit);
            return shown;
        }
        let shown: Entries = entries
            .iter()
            .filter(|entry| !glint_model::entries::is_hidden(entry))
            .cloned()
            .collect();
        kept.insert(0, (entries.clone(), shown.clone()));
        kept.truncate(4);
        shown
    })
}

/// A picture to look into for a place: its path, size and last-write time.
pub type Picture = (String, u64, i64);

/// How the head finds out what its smart buttons need to know off the UI thread. A test puts its own answers here.
#[derive(Clone)]
pub struct Probes {
    /// How many of these files (path, size, modified) say where they were taken.
    pub places: Arc<dyn Fn(Vec<Picture>) -> usize + Send + Sync>,
    /// Whether Time Machine would have something to scrub in this folder.
    pub history: Arc<dyn Fn(String) -> bool + Send + Sync>,
}

impl Probes {
    /// What `glint-core` answers.
    pub fn core() -> Self {
        Probes {
            places: Arc::new(|files| glint_core::gallery::probe::geo_count(&files)),
            history: Arc::new(|folder| glint_core::lens::has_history(&folder)),
        }
    }
}

/// The head of one pane.
pub struct PaneHead {
    explorer: Entity<Explorer>,
    pane: usize,
    head: Option<Head>,
    /// Counts the places shown, so each arrival is its own animation.
    arrivals: usize,
    /// The place the head last said (it survives a search hiding the head): only a different one is an arrival.
    last_place: Option<(TabId, String)>,
    /// The explorer changed since the head was worked out.
    dirty: bool,
    /// How wide the chips' and buttons' place was when last drawn, in pixels (0 until it has been).
    room: Rc<Cell<f32>>,
    probes: Probes,
    /// What the pictures of each folder say of where they were taken, by path (the last answer, kept while the
    /// next listing is read so the button doesn't blink).
    places: HashMap<String, Places>,
    /// The listing that was last looked into.
    looked: Option<Looked>,
    /// Whether each folder has a history, and when that was learned.
    history: HashMap<String, (bool, Instant)>,
    history_asked: HashSet<String>,
    /// The buttons that came to be known after the place was drawn: they fade in (the others are just there).
    fresh: Vec<Lens>,
    /// The panels' models already followed (a bit each: map, time machine, color wheel).
    followed: u8,
    _subscriptions: Vec<Subscription>,
}

impl PaneHead {
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let subscriptions = vec![cx.observe(&explorer, |this: &mut Self, _, cx| {
            this.dirty = true;
            this.refresh(cx);
            this.ask(cx);
            cx.notify();
        })];
        PaneHead {
            explorer,
            pane,
            head: None,
            arrivals: 0,
            last_place: None,
            dirty: true,
            room: Rc::default(),
            probes: Probes::core(),
            places: HashMap::new(),
            looked: None,
            history: HashMap::new(),
            history_asked: HashSet::new(),
            fresh: Vec::new(),
            followed: 0,
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
            // An arrival is a different place, not a refresh, a sort or a selection of this one.
            if let Some(head) = &head {
                let place = (head.tab, head.path.clone());
                if self.last_place.as_ref() != Some(&place) {
                    self.arrivals += 1;
                    self.fresh.clear();
                    self.last_place = Some(place);
                }
            }
            self.head = head;
            self.dirty = false;
        }
    }

    /// Asks what the smart buttons need once the folder's listing is in: whether pictures here have places, and
    /// whether the folder has a history. Each runs on a background thread; the answer is kept and the head
    /// redrawn, with the button fading in if it is new.
    fn ask(&mut self, cx: &mut Context<Self>) {
        let Some(head) = self.head.as_ref() else {
            return;
        };
        if !(head.is_folder || head.is_gallery) {
            return;
        }
        let (tab, path, media, is_folder) = (head.tab, head.path.clone(), head.media, head.is_folder);
        let Some((entries, loading)) = self
            .explorer
            .read(cx)
            .tab(tab)
            .map(|tab| (tab.shared_entries(), tab.loading))
        else {
            return;
        };
        let Some(entries) = entries.filter(|_| !loading) else {
            return;
        };
        // Places: only where there are pictures, and once per listing.
        if media == 0 {
            self.places.remove(&path);
            self.looked = None;
        } else if !self.looked.as_ref().is_some_and(|looked| looked.is(&path, &entries)) {
            self.looked = Some(Looked {
                path: path.clone(),
                entries: entries.clone(),
            });
            let places = self.probes.places.clone();
            let key = path.clone();
            cx.spawn(async move |this, cx| {
                let found = cx
                    .background_spawn(async move {
                        let (files, more) = lenses::pictures_of(&entries);
                        if files.is_empty() {
                            return Places::default();
                        }
                        Places {
                            count: places(files),
                            more,
                        }
                    })
                    .await;
                this.update(cx, |this, cx| {
                    let before = this.places.get(&key).is_some_and(|p| p.count > 0);
                    if found.count > 0 && !before {
                        this.fresh.push(Lens::Map);
                    }
                    this.places.insert(key, found);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        // History: once per folder, kept a minute.
        let known = self
            .history
            .get(&path)
            .is_some_and(|(_, at)| at.elapsed() < lenses::HISTORY_KEPT);
        if is_folder && !known && self.history_asked.insert(path.clone()) {
            let history = self.probes.history.clone();
            let key = path.clone();
            cx.spawn(async move |this, cx| {
                let folder = key.clone();
                let has = cx.background_spawn(async move { history(folder) }).await;
                this.update(cx, |this, cx| {
                    this.history_asked.remove(&key);
                    let before = this.history.get(&key).is_some_and(|(has, _)| *has);
                    if has && !before {
                        this.fresh.push(Lens::History);
                    }
                    this.history.insert(key, (has, Instant::now()));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// The panels' models change what is lit: follow each once it exists.
    fn follow_panels(&mut self, cx: &mut Context<Self>) {
        if self.followed & 1 == 0 {
            if let Some(model) = crate::components::photo_map::try_shared(cx) {
                self._subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
                self.followed |= 1;
            }
        }
        if self.followed & 2 == 0 {
            if let Some(model) = crate::components::time_machine::try_shared(cx) {
                self._subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
                self.followed |= 2;
            }
        }
        if self.followed & 4 == 0 {
            let model = crate::components::color_wheel::shared(cx);
            self._subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
            self.followed |= 4;
        }
    }

    /// The smart buttons to draw now, each with whether its panel is open (it is then lit, and stays).
    fn lenses_now(&self, head: &Head, cx: &App) -> Vec<(Lens, bool)> {
        let on_disk = head.is_folder || head.is_gallery;
        let places = self.places.get(&head.path).copied().unwrap_or_default();
        let open = |lens: Lens| match lens {
            Lens::Map => crate::components::photo_map::shows(cx, head.tab),
            Lens::Colors => crate::components::color_wheel::is_shown(cx, head.tab),
            Lens::History => crate::components::time_machine::shows(cx, head.tab),
        };
        Lens::ALL
            .into_iter()
            .filter_map(|lens| {
                let open = open(lens);
                let offered = on_disk
                    && match lens {
                        Lens::Map => places.count > 0,
                        Lens::Colors => lenses::colors_fit(head),
                        Lens::History => head.is_folder && self.history.get(&head.path).is_some_and(|(has, _)| *has),
                    };
                (offered || open).then_some((lens, open))
            })
            .collect()
    }

    /// One smart button: a pill with the lens's icon and word, lit while its panel is open, fading in when it has
    /// just become known.
    fn lens_pill(&self, lens: Lens, open: bool, head: &Head, places: Places, cx: &mut Context<Self>) -> AnyElement {
        let tab = head.tab;
        let (icon, label, tip) = (
            match lens {
                Lens::Map => IconName::Globe,
                Lens::Colors => IconName::Pipette,
                Lens::History => IconName::RotateCcwClock,
            },
            lens.label(places.count, places.more),
            lens.tip(places.count, places.more),
        );
        let button = pill::pill((lens.id(), self.pane), open, cx)
            .icon(Icon::new(icon))
            .label(label)
            .accessibility_label(tip.clone())
            .tooltip(tip)
            .on_click(cx.listener(move |_, _, _, cx| {
                match lens {
                    Lens::Map => crate::components::photo_map::toggle(cx, tab),
                    Lens::Colors => crate::components::color_wheel::toggle(cx, tab),
                    Lens::History => crate::components::time_machine::toggle(cx, tab),
                }
                cx.notify();
            }));
        let slot = div().flex_none().child(button);
        if cx.reduce_motion() || !self.fresh.contains(&lens) {
            return slot.into_any_element();
        }
        use gpui_kit::{Animation, AnimationExt as _};
        slot.with_animation(
            (lens.id(), self.pane * 4 + lens.index()),
            Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
            |slot, delta| slot.opacity(delta),
        )
        .into_any_element()
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
        let first = self.dirty;
        self.refresh(cx);
        if first {
            self.ask(cx);
        }
        self.follow_panels(cx);
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
            // A line high even when empty (before the listing is in), so the head doesn't grow when words arrive.
            .h(rems(1.))
            .line_height(rems(1.))
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
        // The smart buttons at the very end, then the chips before them. The room for both is the width of one box
        // that doesn't depend on what is in it, so a button coming or going changes how many chips fit in the same
        // frame; the chips that don't fit go to a second line, which isn't shown: a pane too narrow for all of
        // them shows the commonest whole, never half of one.
        let lenses = self.lenses_now(&head, cx);
        let places = self.places.get(&head.path).copied().unwrap_or_default();
        let gap = f32::from(rem) * 0.375;
        let pills_room: f32 = lenses
            .iter()
            .map(|(lens, _)| (lens.width_rems(places.count, places.more) * f32::from(rem)) + gap)
            .sum();
        // Never measured: the whole window is the most there can be (what doesn't fit wraps out of sight).
        let room = match self.room.get() {
            measured if measured > 0. => measured,
            _ => f32::from(window.viewport_size().width),
        };
        let fit = fitting(&head.kinds, (room - pills_room).max(0.), f32::from(rem));
        let (measure, id) = (self.room.clone(), cx.entity_id());
        let chips = row_start()
            .id("pane-head-kinds")
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
            );
        let pills = lenses
            .iter()
            .map(|&(lens, open)| self.lens_pill(lens, open, &head, places, cx))
            .collect::<Vec<_>>();
        let end = row_start()
            .id("pane-head-end")
            .relative()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap(space(1.5))
            .child(chips)
            .children(pills)
            // The box's width, for the next frame (asked for when it changed: a redraw asked for while drawing is
            // dropped).
            .child(
                canvas(
                    move |bounds, window, _| {
                        let width = f32::from(bounds.size.width);
                        if (measure.get() - width).abs() > 0.5 {
                            measure.set(width);
                            window.on_next_frame(move |_, cx| cx.notify(id));
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
        // The band: the folder's make-up, each kind in its color. A hairline of them, quiet enough to be the
        // line under the head as well: the chips above it carry the colors and the counts.
        let band = row_start()
            .id("pane-head-band")
            .w_full()
            .h(px(2.))
            .gap(px(2.))
            .children(head.kinds.iter().zip(shares(&head.kinds)).map(|(kind, share)| {
                div()
                    .h_full()
                    .flex_shrink(1.)
                    .w(relative(share))
                    .rounded_full()
                    .bg(theme::hsl(kind.group.hue as f32, kind.group.sat as f32, 54.).opacity(BAND_STRENGTH))
            }));
        // Arriving: the name comes in from the start edge as it clears, and the band grows to its width.
        let (name_row, band) = (
            row_start()
                .w_full()
                .items_center()
                .gap(space(3.))
                .child(head.glyph.picture((rem * if short { 1.5 } else { 2.5 }).round(), cx))
                .child(name)
                .child(end),
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
