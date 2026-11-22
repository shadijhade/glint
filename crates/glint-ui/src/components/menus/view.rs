//! Draws a menu and drives it from the keyboard (the popover in src/components/Menu.tsx).
//!
//! The kit's `PopupMenu` is not used for this because Glint's menus need three things it doesn't have: a strip of
//! icon buttons at the top, headings, and rows that are not mirrored by the kit for the Arabic interface. This
//! view is those, with the kit's tokens and the same keys.
//!
//! **Submenus are flyouts.** A submenu opens beside the menu (toward the end of the line; toward the start when
//! there is no room), as a column of its own drawn in a deferred layer, so a menu that scrolls doesn't clip it.
//! The pointer resting on its row for [`OPEN_AFTER`] opens it; moving toward it keeps it open (a safe triangle
//! from where the pointer was, and a [`GRACE`] period inside it); resting on another row of the parent closes it;
//! a click opens it at once. Flyouts nest. `levels[0]` is the menu, the rest are its flyouts, and `active` is
//! the column the keyboard is in.
//!
//! **Keys.** Down and Up move through everything that can be chosen in the active column, round the ends; Home
//! and End jump; Enter and Space choose; the arrow toward the end of the line (Right in English, Left in Arabic)
//! opens a submenu and moves into it or, in the strip, moves to the next button; the arrow toward the start (and
//! Backspace) goes back to the submenu's row; Escape does that too, and closes the menu from the top. The first
//! row past the strip is selected when a menu opens, as `focusFirst` did.
//!
//! **Height.** No column is taller than `kit::menu_max_height`. The head and the strip stay put and the rows
//! scroll under them, with a thin scroll bar, and the bottom edge fades while more is below.
//!
//! **The Aware wave** (docs/gpui/aware/PLAN.md) adds four things. A menu may start with a *head* (the item's
//! picture, name, facts and badges: [`Node::Head`]); a row may carry a small *reason* line, be drawn as a
//! *suggestion*, or say why it is greyed out; and typing in an open menu *filters* every row, flyouts included
//! (`filter::matches`).

use super::filter::{self, Found};
use super::spec::{Badge, Entry, Head, HeadPicture, Node, Run, Tone};
use crate::kit::prelude::*;
use crate::kit::{menu_max_height, rtl_line};
use crate::{art, i18n, t, theme};
use gpui_kit::{anchored, deferred, rems, Anchor};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// The longest the typed filter may grow: nothing is as long as this in a menu.
const MOST_TYPED: usize = 64;
/// How many badges a head draws; with more, the last place says how many are not shown.
const MOST_BADGES: usize = 3;
/// What a head (name and facts) takes from the room the rows may use, so the whole menu fits its cap.
const HEAD_ROOM: f32 = 66.;
/// What the line of badges adds to it.
const BADGE_LINE_ROOM: f32 = 24.;
/// What the filter line takes.
const FILTER_ROOM: f32 = 56.;
/// What a strip that stays at the top takes.
const STRIP_ROOM: f32 = 46.;
/// The frame's own padding and border.
const FRAME_ROOM: f32 = 10.;
/// How many ids a column keeps for itself: the rows of column `lv` are `lv * LEVEL_STEP + their place`.
pub(super) const LEVEL_STEP: usize = 1000;
/// How long the pointer rests on a submenu's row before the flyout opens.
pub(super) const OPEN_AFTER: Duration = Duration::from_millis(150);
/// How long a flyout stays open while the pointer is on its way to it across the parent's other rows.
pub(super) const GRACE: Duration = Duration::from_millis(300);
/// How long the pointer rests on a strip's button before its name shows.
const TIP_AFTER: Duration = Duration::from_millis(300);
/// How far a flyout reaches back over its parent, so the pointer never crosses a gap.
const OVERLAP: f32 = 3.;

/// What the view tells its host.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuViewEvent {
    /// An item was chosen.
    Chose(Run),
    /// Escape, or a press outside the menu.
    Dismissed,
}

impl EventEmitter<MenuViewEvent> for MenuView {}

/// One place the keyboard can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stop {
    /// A row or a submenu, by its place in the level's nodes.
    Row(usize),
    /// A button of the strip: the strip's place, then the button's.
    Button(usize, usize),
}

/// Which row a drawn row is: one of a column's nodes (the column, then the node), or one the filter found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Row(usize, usize),
    Found(usize),
}

/// Whether a key's text is something to type: there is some, and none of it is a control character.
fn visible_text(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

/// Whether `p` is inside the triangle `a`, `b`, `c` (the pointer on its way from where it was to a flyout's
/// near edge).
pub(super) fn inside_triangle(p: Point<Pixels>, a: Point<Pixels>, b: Point<Pixels>, c: Point<Pixels>) -> bool {
    let side = |p1: Point<Pixels>, p2: Point<Pixels>, p3: Point<Pixels>| -> f32 {
        f32::from(p1.x - p3.x) * f32::from(p2.y - p3.y) - f32::from(p2.x - p3.x) * f32::from(p1.y - p3.y)
    };
    let (d1, d2, d3) = (side(p, a, b), side(p, b, c), side(p, c, a));
    let negative = d1 < 0. || d2 < 0. || d3 < 0.;
    let positive = d1 > 0. || d2 > 0. || d3 > 0.;
    !(negative && positive)
}

/// How a column arrives: what keys its animation, how long it takes, and the corner it grows from (0 to 1 of
/// its box along each side).
struct Arrival {
    key: ElementId,
    duration: Duration,
    from: (f32, f32),
}

/// A column of the menu: the menu itself, or a flyout.
struct Level {
    /// The submenu's name; none for the top.
    title: Option<SharedString>,
    nodes: Vec<Node>,
    /// Where the keyboard is, as an index into [`Level::stops`].
    at: Option<usize>,
    /// The row of the column before this one that opened it.
    from: usize,
    /// Which flyout it is (counted over the menu's life): what keys its arrival.
    born: usize,
}

impl Level {
    fn new(title: Option<SharedString>, nodes: Vec<Node>, from: usize, born: usize) -> Level {
        let mut level = Level {
            title,
            nodes,
            at: None,
            from,
            born,
        };
        level.at = level.first_row();
        level
    }

    /// The first row past the strip, failing that the first thing there is.
    fn first_row(&self) -> Option<usize> {
        let stops = self.stops();
        stops
            .iter()
            .position(|s| matches!(s, Stop::Row(_)))
            .or(if stops.is_empty() { None } else { Some(0) })
    }

    /// Everything that can be chosen, in the order Down goes through it.
    fn stops(&self) -> Vec<Stop> {
        let mut stops = Vec::new();
        for (i, node) in self.nodes.iter().enumerate() {
            match node {
                Node::Item(entry) if !entry.disabled => stops.push(Stop::Row(i)),
                Node::Menu { disabled: false, .. } => stops.push(Stop::Row(i)),
                Node::Strip(entries) => {
                    for (j, entry) in entries.iter().enumerate() {
                        if !entry.disabled {
                            stops.push(Stop::Button(i, j));
                        }
                    }
                }
                _ => {}
            }
        }
        stops
    }

    fn stop(&self) -> Option<Stop> {
        self.at.and_then(|at| self.stops().get(at).copied())
    }

    /// How many nodes at the top stay put (the head and the strips) while the rest scrolls under them.
    fn first_scrolled(&self) -> usize {
        self.nodes
            .iter()
            .take_while(|node| matches!(node, Node::Head(_) | Node::Strip(_)))
            .count()
    }
}

/// Where things were last painted: what a flyout is placed by, and what a press is judged by.
#[derive(Default)]
struct Geometry {
    /// Each column's frame.
    frames: Vec<Bounds<Pixels>>,
    /// The submenu rows, by (column, node).
    rows: HashMap<(usize, usize), Bounds<Pixels>>,
}

type SharedGeometry = Rc<RefCell<Geometry>>;

/// A menu on screen.
pub struct MenuView {
    levels: Vec<Level>,
    /// A scroll handle for each column.
    scrolls: Vec<ScrollHandle>,
    /// The column the keyboard is in.
    active: usize,
    focus: FocusHandle,
    /// Scroll this child of this column's list into view at the next frame (the keyboard moved to it).
    scroll_to: Option<(usize, usize)>,
    /// Whether the keyboard moved last (it is drawn as a fill at once), or the pointer did (its fill fades in
    /// by itself).
    by_keys: bool,
    /// Where each column's lit row was when the last frame was drawn, so each row doesn't work it out again.
    lit: Vec<Option<Stop>>,
    /// What a screen reader calls the menu.
    label: SharedString,
    /// What has been typed: the menu shows the rows that match it, in place of itself.
    query: String,
    /// The rows that match `query`, best first.
    found: Vec<Found>,
    /// Which of `found` the keyboard is on.
    found_at: usize,
    /// How tall the menu was drawn when nothing was typed: it keeps that height while typing, so the few rows
    /// found don't make it jump to another place in the window.
    rest_height: Rc<Cell<f32>>,
    /// Where the menu was asked for (the point it grows from).
    at: Option<Point<Pixels>>,
    geometry: SharedGeometry,
    /// Whether the head has a line of badges: decided when the menu opens, so the head's height never changes.
    badge_line: bool,
    /// How many flyouts were opened (what keys each one's arrival).
    flights: usize,
    /// The flyout about to open, after the pointer rested on its row.
    opening: Option<gpui_kit::Task<()>>,
    /// The flyout about to close, after the pointer is on its way across the other rows.
    closing: Option<gpui_kit::Task<()>>,
    /// Where the pointer last was on the row that opened the flyout (the apex of the safe triangle).
    trail: Option<Point<Pixels>>,
    /// Whether the menu has been told it is open (tooltips under it are dismissed once).
    announced: bool,
    /// Whether it has said it was dismissed (a press outside is noticed by every column).
    dismissed: bool,
}

impl MenuView {
    /// A menu of `nodes` (already cleaned by [`visible_items`](super::spec::visible_items)).
    pub fn new(nodes: Vec<Node>, label: SharedString, cx: &mut Context<Self>) -> MenuView {
        let badge_line = nodes
            .iter()
            .any(|node| matches!(node, Node::Head(head) if !head.badges.is_empty()));
        MenuView {
            levels: vec![Level::new(None, nodes, 0, 0)],
            scrolls: vec![ScrollHandle::new()],
            active: 0,
            focus: cx.focus_handle(),
            scroll_to: None,
            by_keys: true,
            lit: Vec::new(),
            label,
            query: String::new(),
            found: Vec::new(),
            found_at: 0,
            rest_height: Default::default(),
            at: None,
            geometry: Default::default(),
            badge_line,
            flights: 0,
            opening: None,
            closing: None,
            trail: None,
            announced: false,
            dismissed: false,
        }
    }

    /// Says where the menu was asked for: it grows from there, and from the side of it that is nearer the
    /// window's edge when it had to open the other way.
    pub fn anchored_at(&mut self, at: Point<Pixels>) {
        self.at = Some(at);
    }

    /// Replaces the head of the top menu (what is known about the item changed while the menu is open). Nothing
    /// else moves: its height is fixed, and the keyboard stays where it was (R2 of docs/gpui/aware/PLAN.md).
    pub fn set_head(&mut self, head: Head, cx: &mut Context<Self>) {
        let Some(top) = self.levels.first_mut() else { return };
        let Some(slot) = top.nodes.iter_mut().find(|node| matches!(node, Node::Head(_))) else {
            return;
        };
        *slot = Node::Head(head);
        cx.notify();
    }

    /// Whether the top menu starts with a head.
    fn has_head(&self) -> bool {
        self.levels
            .first()
            .is_some_and(|l| l.nodes.iter().any(|n| matches!(n, Node::Head(_))))
    }

    /// Takes the keyboard.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }

    /// The nodes of the column the keyboard is in (the menu's, or a flyout's).
    pub fn nodes(&self) -> &[Node] {
        self.levels.get(self.active).map_or(&[], |l| &l.nodes)
    }

    /// How many columns are open (1 for the menu alone, 2 with a flyout).
    pub fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Which column the keyboard is in (0 is the menu).
    pub fn active_level(&self) -> usize {
        self.active
    }

    /// The label of the row the keyboard is on, for tests and screen readers.
    #[cfg(test)]
    pub fn selected_label(&self) -> Option<SharedString> {
        if !self.query.is_empty() {
            return self.found.get(self.found_at).map(|f| f.entry.label.clone());
        }
        let level = self.levels.get(self.active)?;
        match level.stop()? {
            Stop::Row(i) => match level.nodes.get(i)? {
                Node::Item(e) => Some(e.label.clone()),
                Node::Menu { label, .. } => Some(label.clone()),
                _ => None,
            },
            Stop::Button(i, j) => match level.nodes.get(i)? {
                Node::Strip(entries) => entries.get(j).map(|e| e.label.clone()),
                _ => None,
            },
        }
    }

    /// The labels of the rows the filter found, best first, for tests.
    #[cfg(test)]
    pub fn found_labels(&self) -> Vec<SharedString> {
        self.found.iter().map(|f| f.entry.label.clone()).collect()
    }

    /// Scrolls node `i` of column `lv` into view, as the keyboard does (tests click what a capped menu scrolled
    /// out of sight).
    #[cfg(test)]
    pub(super) fn reveal(&mut self, lv: usize, i: usize) {
        if let (Some(level), Some(scroll)) = (self.levels.get(lv), self.scrolls.get(lv)) {
            if let Some(child) = i.checked_sub(level.first_scrolled()) {
                scroll.scroll_to_item(child);
            }
        }
    }

    /// The name of the flyout the keyboard is in, for tests.
    #[cfg(test)]
    pub fn flyout_title(&self) -> Option<SharedString> {
        self.levels.get(self.active).and_then(|l| l.title.clone())
    }

    // ---- Moving -----------------------------------------------------------------------------------------

    /// Where the child of the column's scroll list for `stop` is; none for what stays put above it.
    fn child_of(level: &Level, stop: Stop) -> Option<usize> {
        match stop {
            Stop::Row(i) | Stop::Button(i, _) => i.checked_sub(level.first_scrolled()),
        }
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        let lv = self.active;
        let Some(level) = self.levels.get_mut(lv) else { return };
        level.at = Some(at);
        if let Some(stop) = level.stops().get(at).copied() {
            self.scroll_to = Self::child_of(level, stop).map(|child| (lv, child));
        }
        self.by_keys = true;
        cx.notify();
    }

    /// Down (`step` 1) or Up (-1), round the ends.
    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(level) = self.levels.get(self.active) else {
            return;
        };
        let count = level.stops().len();
        if count == 0 {
            return;
        }
        let at = match level.at {
            Some(at) if step > 0 => (at + 1) % count,
            Some(at) => (at + count - 1) % count,
            None if step > 0 => 0,
            None => count - 1,
        };
        self.move_to(at, cx);
    }

    /// Home and End.
    fn jump(&mut self, last: bool, cx: &mut Context<Self>) {
        let count = self.levels.get(self.active).map_or(0, |l| l.stops().len());
        if count > 0 {
            self.move_to(if last { count - 1 } else { 0 }, cx);
        }
    }

    /// The arrow toward the end of the line (`toward_end`) or the start: in the strip it moves along the buttons,
    /// otherwise toward the end opens a submenu and moves into it, and toward the start goes back.
    fn sideways(&mut self, toward_end: bool, cx: &mut Context<Self>) {
        let lv = self.active;
        let Some(level) = self.levels.get(lv) else { return };
        let stops = level.stops();
        match level.stop() {
            Some(Stop::Button(strip, button)) => {
                let here = stops.iter().position(|s| *s == Stop::Button(strip, button));
                let next = if toward_end {
                    here.map(|p| p + 1)
                } else {
                    here.and_then(|p| p.checked_sub(1))
                };
                if let Some(next) = next.filter(|n| matches!(stops.get(*n), Some(Stop::Button(s, _)) if *s == strip)) {
                    self.move_to(next, cx);
                }
            }
            Some(Stop::Row(i)) if toward_end => {
                if matches!(level.nodes.get(i), Some(Node::Menu { disabled: false, .. })) {
                    self.enter(lv, i, cx);
                }
            }
            _ if !toward_end => self.leave(cx),
            _ => {}
        }
    }

    /// Stops what the pointer started: a flyout about to open or to close.
    fn cancel_timers(&mut self) {
        self.opening = None;
        self.closing = None;
    }

    /// Runs `then` after `delay`, unless the task is dropped first.
    fn after(
        delay: Duration,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui_kit::Task<()> {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |this, cx| then(this, cx)).ok();
        })
    }

    /// Closes every flyout past the first `keep` columns.
    fn close_beyond(&mut self, keep: usize, cx: &mut Context<Self>) {
        let keep = keep.max(1);
        if self.levels.len() <= keep {
            return;
        }
        self.levels.truncate(keep);
        self.scrolls.truncate(keep);
        self.active = self.active.min(keep - 1);
        if self.scroll_to.is_some_and(|(lv, _)| lv >= keep) {
            self.scroll_to = None;
        }
        let mut geometry = self.geometry.borrow_mut();
        geometry.frames.truncate(keep);
        geometry.rows.retain(|(lv, _), _| *lv < keep);
        drop(geometry);
        self.trail = None;
        cx.notify();
    }

    /// Opens the submenu at node `i` of column `lv` as a flyout, without moving the keyboard into it.
    fn open_flyout(&mut self, lv: usize, i: usize, cx: &mut Context<Self>) {
        let Some(Node::Menu { label, items, .. }) = self.levels.get(lv).and_then(|l| l.nodes.get(i)).cloned() else {
            return;
        };
        self.close_beyond(lv + 1, cx);
        self.flights += 1;
        let mut level = Level::new(Some(label), items, i, self.flights);
        // The keyboard stays where it was until it is sent in.
        level.at = None;
        self.levels.push(level);
        self.scrolls.push(ScrollHandle::new());
        self.trail = None;
        cx.notify();
    }

    /// Opens the submenu at node `i` of column `lv` and moves the keyboard into it (Right, Enter, a click).
    fn enter(&mut self, lv: usize, i: usize, cx: &mut Context<Self>) {
        self.cancel_timers();
        if let Some(level) = self.levels.get_mut(lv) {
            if let Some(at) = level.stops().iter().position(|s| *s == Stop::Row(i)) {
                level.at = Some(at);
            }
        }
        let open = self.levels.get(lv + 1).is_some_and(|l| l.from == i);
        if !open {
            self.open_flyout(lv, i, cx);
        } else {
            self.close_beyond(lv + 2, cx);
        }
        self.active = lv + 1;
        if let Some(level) = self.levels.get_mut(lv + 1) {
            if level.at.is_none() {
                level.at = level.first_row();
            }
            let scroll = level
                .at
                .and_then(|at| level.stops().get(at).copied())
                .and_then(|stop| Self::child_of(level, stop));
            self.scroll_to = scroll.map(|child| (lv + 1, child));
        }
        self.by_keys = true;
        cx.notify();
    }

    /// Back to the row that opened the flyout the keyboard is in; from the menu, closes a flyout the pointer
    /// opened.
    fn leave(&mut self, cx: &mut Context<Self>) {
        self.cancel_timers();
        if self.active > 0 {
            let to = self.active;
            self.active = to - 1;
            self.close_beyond(to, cx);
            if let Some(level) = self.levels.get(self.active) {
                self.scroll_to = level
                    .stop()
                    .and_then(|stop| Self::child_of(level, stop))
                    .map(|child| (self.active, child));
            }
            self.by_keys = true;
            cx.notify();
        } else if self.levels.len() > 1 {
            self.close_beyond(1, cx);
        }
    }

    /// Chooses what the keyboard is on.
    fn activate(&mut self, cx: &mut Context<Self>) {
        let lv = self.active;
        let Some(level) = self.levels.get(lv) else { return };
        match level.stop() {
            Some(Stop::Row(i)) => match level.nodes.get(i) {
                Some(Node::Item(entry)) => self.choose(entry.clone(), cx),
                Some(Node::Menu { .. }) => self.enter(lv, i, cx),
                _ => {}
            },
            Some(Stop::Button(i, j)) => {
                if let Some(Node::Strip(entries)) = level.nodes.get(i) {
                    if let Some(entry) = entries.get(j) {
                        self.choose(entry.clone(), cx);
                    }
                }
            }
            None => {}
        }
    }

    fn choose(&mut self, entry: Entry, cx: &mut Context<Self>) {
        if entry.disabled || entry.run == Run::Nothing {
            return;
        }
        cx.emit(MenuViewEvent::Chose(entry.run));
    }

    /// The keyboard (`onKeyDown` in `useMenu`).
    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mods = event.keystroke.modifiers;
        if mods.control || mods.alt || mods.platform {
            return;
        }
        let rtl = i18n::is_rtl();
        let typing = !self.query.is_empty();
        match event.keystroke.key.as_str() {
            "down" if typing => self.step_found(1, cx),
            "up" if typing => self.step_found(-1, cx),
            "home" if typing => self.jump_found(false, cx),
            "end" if typing => self.jump_found(true, cx),
            "enter" if typing => self.choose_found(cx),
            "space" if typing => self.type_text(" ", cx),
            "right" | "left" if typing => {}
            "backspace" if typing => {
                self.query.pop();
                self.refilter(cx);
            }
            "escape" if typing => {
                self.query.clear();
                self.refilter(cx);
            }
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            "home" => self.jump(false, cx),
            "end" => self.jump(true, cx),
            "enter" | "space" => self.activate(cx),
            "right" => self.sideways(!rtl, cx),
            "left" => self.sideways(rtl, cx),
            "backspace" => self.leave(cx),
            "escape" => {
                if self.levels.len() > 1 {
                    // Back to the row, as the arrow toward the start does.
                    self.active = self.active.max(1);
                    self.leave(cx);
                } else {
                    self.say_dismissed(cx);
                }
            }
            _ => {
                // Anything else that is text starts or goes on a filter.
                let Some(text) = event.keystroke.key_char.as_deref().filter(|text| visible_text(text)) else {
                    return;
                };
                self.type_text(text, cx);
            }
        }
        cx.stop_propagation();
    }

    /// Tells the host the menu was dismissed (once).
    fn say_dismissed(&mut self, cx: &mut Context<Self>) {
        if !self.dismissed {
            self.dismissed = true;
            cx.emit(MenuViewEvent::Dismissed);
        }
    }

    /// A press at `position`: outside every column of the menu it dismisses it.
    fn pressed_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let inside = self
            .geometry
            .borrow()
            .frames
            .iter()
            .any(|frame| frame.dilate(px(1.)).contains(&position));
        if !inside {
            self.say_dismissed(cx);
        }
    }

    // ---- Typing -----------------------------------------------------------------------------------------

    /// Adds what was typed to the filter.
    fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.query.chars().count() + text.chars().count() <= MOST_TYPED {
            self.query.push_str(text);
        }
        self.refilter(cx);
    }

    /// Looks through the whole menu (the top one, whatever flyouts are open) for what is typed.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.cancel_timers();
        self.found = match self.levels.first() {
            Some(top) if !self.query.is_empty() => filter::matches(&top.nodes, &self.query),
            _ => Vec::new(),
        };
        self.found_at = 0;
        if let Some(scroll) = self.scrolls.first() {
            scroll.set_offset(point(px(0.), px(0.)));
        }
        cx.notify();
    }

    fn move_found(&mut self, at: usize, cx: &mut Context<Self>) {
        self.found_at = at;
        self.scroll_to = Some((0, at));
        cx.notify();
    }

    /// Down (`step` 1) or Up (-1) through the found rows, round the ends.
    fn step_found(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.found.len();
        if count == 0 {
            return;
        }
        let at = if step > 0 {
            (self.found_at + 1) % count
        } else {
            (self.found_at + count - 1) % count
        };
        self.move_found(at, cx);
    }

    /// Home and End through the found rows.
    fn jump_found(&mut self, last: bool, cx: &mut Context<Self>) {
        if !self.found.is_empty() {
            self.move_found(if last { self.found.len() - 1 } else { 0 }, cx);
        }
    }

    /// Chooses the found row the keyboard is on (nothing when nothing was found).
    fn choose_found(&mut self, cx: &mut Context<Self>) {
        if let Some(found) = self.found.get(self.found_at) {
            let entry = found.entry.clone();
            self.choose(entry, cx);
        }
    }

    // ---- The pointer ------------------------------------------------------------------------------------

    /// Whether `stop` is the lit one of column `lv` as of the frame being drawn.
    fn is_lit(&self, lv: usize, stop: Stop) -> bool {
        self.lit.get(lv).copied().flatten() == Some(stop)
    }

    /// Makes `stop` the place the keyboard is in column `lv`.
    fn set_at(&mut self, lv: usize, stop: Stop, cx: &mut Context<Self>) {
        let Some(level) = self.levels.get_mut(lv) else { return };
        if let Some(at) = level.stops().iter().position(|s| *s == stop) {
            if level.at != Some(at) || self.by_keys {
                level.at = Some(at);
                self.by_keys = false;
                cx.notify();
            }
        }
    }

    /// The pointer is over `spot`: the keyboard follows it, a submenu's row opens its flyout after a moment,
    /// and what was open from another row closes (unless the pointer is on its way to it).
    fn pointed(&mut self, spot: Spot, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (lv, ix) = match spot {
            Spot::Found(i) => {
                if self.found_at != i {
                    self.found_at = i;
                    cx.notify();
                }
                return;
            }
            Spot::Row(lv, ix) => (lv, ix),
        };
        self.cancel_timers();
        self.set_at(lv, Stop::Row(ix), cx);
        if self.active != lv {
            self.active = lv;
            cx.notify();
        }
        let opens = matches!(
            self.levels.get(lv).and_then(|l| l.nodes.get(ix)),
            Some(Node::Menu { disabled: false, .. })
        );
        let open_from = self.levels.get(lv + 1).map(|l| l.from);
        if open_from == Some(ix) {
            // The row of the flyout that is up: what is deeper than it goes.
            self.close_beyond(lv + 2, cx);
        } else if opens {
            self.opening = Some(Self::after(OPEN_AFTER, cx, move |this, cx| {
                this.opening = None;
                this.open_flyout(lv, ix, cx);
            }));
        } else if open_from.is_some() {
            if self.on_its_way(lv, position) {
                self.closing = Some(Self::after(GRACE, cx, move |this, cx| {
                    this.closing = None;
                    this.close_beyond(lv + 1, cx);
                }));
            } else {
                self.close_beyond(lv + 1, cx);
            }
        }
    }

    /// Whether the pointer at `position` is inside the triangle from where it last was on the row that opened
    /// column `lv`'s flyout to the flyout's near edge: it is going there.
    fn on_its_way(&self, lv: usize, position: Point<Pixels>) -> bool {
        let Some(apex) = self.trail else { return false };
        let geometry = self.geometry.borrow();
        let Some(frame) = geometry.frames.get(lv + 1) else {
            return false;
        };
        let edge = if i18n::is_rtl() { frame.right() } else { frame.left() };
        inside_triangle(position, apex, point(edge, frame.top()), point(edge, frame.bottom()))
    }

    /// The pointer moved over column `lv`: where it is on the row that opened the flyout is remembered.
    fn moved_over(&mut self, lv: usize, position: Point<Pixels>) {
        let Some(from) = self.levels.get(lv + 1).map(|l| l.from) else {
            return;
        };
        let on_the_row = self
            .geometry
            .borrow()
            .rows
            .get(&(lv, from))
            .is_some_and(|row| row.contains(&position));
        if on_the_row {
            self.trail = Some(position);
        }
    }

    /// A press on a row of column `lv`.
    fn pressed_row(&mut self, lv: usize, ix: usize, cx: &mut Context<Self>) {
        self.cancel_timers();
        self.set_at(lv, Stop::Row(ix), cx);
        self.active = lv;
        self.close_beyond(lv + 1, cx);
        self.activate(cx);
    }

    // ---- Drawing ----------------------------------------------------------------------------------------

    fn icon_of(entry: &Entry, tint: Option<Hsla>) -> Option<AnyElement> {
        // A row with a picture of its own (the New menu's kinds, as Windows draws them) shows it.
        if let (None, Some(picture)) = (entry.check, entry.picture.clone()) {
            return Some(img(picture).size_4().flex_none().into_any_element());
        }
        // A check mark stands in for the icon of a row that is one of a set; unchecked it keeps the room.
        match (entry.check, entry.icon) {
            (Some(checked), _) => {
                let mark = icon(IconName::Check).size_4().flex_none();
                Some(if checked {
                    mark.into_any_element()
                } else {
                    mark.invisible().into_any_element()
                })
            }
            (None, Some(name)) => {
                let mut i = icon(name).size_4().flex_none();
                if let Some(tint) = tint {
                    i = i.text_color(tint);
                }
                Some(i.into_any_element())
            }
            (None, None) => None,
        }
    }

    /// A row of the menu, or (`Spot::Found`) one the filter found: `path` is the submenus it is in.
    fn render_row(
        &self,
        spot: Spot,
        entry: &Entry,
        path: Option<&SharedString>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (id, lit) = match spot {
            Spot::Row(lv, ix) => (
                ElementId::from(("menu-row", lv * LEVEL_STEP + ix)),
                self.is_lit(lv, Stop::Row(ix)),
            ),
            Spot::Found(i) => (ElementId::from(("menu-found", i)), self.found_at == i),
        };
        // The keyboard's row is filled at once; the pointer's fades in and out by itself.
        let selected = lit && (self.by_keys || matches!(spot, Spot::Found(_)) || self.opener_lit(spot));
        let role = match entry.check {
            Some(_) => Role::MenuItemCheckBox,
            None => Role::MenuItem,
        };
        let tint = if entry.smart {
            t.primary
        } else if entry.danger {
            t.destructive
        } else {
            t.muted_foreground
        };
        // What a screen reader says: where a found row lives, then what it is.
        let name: SharedString = match path {
            Some(path) => format!("{path} › {}", entry.label).into(),
            None => entry.label.clone(),
        };
        // Why it is put forward, or why it can't be chosen.
        let note = if entry.disabled {
            entry.why_not.clone()
        } else {
            entry.reason.clone()
        };
        let two_lines = entry.reason.is_some() && !entry.disabled;
        let accent = t.accent;
        let mut row = row_start()
            .id(id)
            .role(role)
            .aria_label(name)
            .test_support()
            .w_full()
            .gap(space(2.))
            .px(space(2.))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .text_size(theme::size::SM)
            .text_start()
            .cursor_pointer()
            .when(two_lines, |d| d.items_start())
            .when(selected, |d| d.bg(accent))
            .when(!entry.disabled && !selected, |d| d.hover(move |style| style.bg(accent)))
            .when(entry.danger, |d| d.text_color(t.destructive))
            .when(entry.disabled, |d| d.opacity(0.4).cursor_default());
        if let Some(checked) = entry.check {
            row = row.aria_toggled(if checked { Toggled::True } else { Toggled::False });
        }
        if lit {
            row = row.aria_active_descendant();
        }
        if let Some(note) = note {
            row = row.aria_description(note);
        }
        if let (true, Some(why)) = (entry.disabled, entry.why_not.clone()) {
            row = row.tooltip(move |window, cx| tooltip(why.clone(), window, cx));
        }
        if !entry.disabled {
            row = row
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered {
                        this.pointed(spot, window.mouse_position(), cx);
                    }
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| match spot {
                    Spot::Row(lv, ix) => this.pressed_row(lv, ix, cx),
                    Spot::Found(i) => {
                        this.found_at = i;
                        this.choose_found(cx);
                    }
                }));
        }
        // The icon sits at the top of a two-line row, a pixel or two down so it lines up with the label.
        let mark = Self::icon_of(entry, Some(tint)).map(|mark| {
            if two_lines {
                div().flex_none().mt(px(2.)).child(mark).into_any_element()
            } else {
                mark
            }
        });
        let label = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .text_start()
            .child(isolate(entry.label.clone()));
        let body = match (path, &entry.reason) {
            (_, Some(reason)) if two_lines => col()
                .flex_1()
                .min_w_0()
                .items_start()
                .child(label.w_full())
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .text_start()
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(isolate(reason.clone())),
                )
                .into_any_element(),
            (Some(path), _) => row_start()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .max_w_1_2()
                        .truncate()
                        .text_start()
                        .text_color(t.muted_foreground)
                        .child(isolate(format!("{path} › "))),
                )
                .child(label)
                .into_any_element(),
            _ => label.into_any_element(),
        };
        row.children(mark)
            .child(body)
            .children(entry.hint.clone().map(|hint| {
                div()
                    .flex_none()
                    .ms(6.)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(hint)
            }))
            .into_any_element()
    }

    /// Whether the row is lit because it is the one a flyout is open from (filled at once, as the keyboard's).
    fn opener_lit(&self, spot: Spot) -> bool {
        match spot {
            Spot::Row(lv, ix) => self.levels.get(lv + 1).is_some_and(|l| l.from == ix),
            Spot::Found(_) => false,
        }
    }

    // ---- The head -----------------------------------------------------------------------------------------

    /// One badge of a head: a pill, pressed with the mouse when it has something behind it.
    fn render_badge(&self, ix: usize, badge: &Badge, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let (fg, bg) = match badge.tone {
            Tone::Plain => (t.foreground, t.accent),
            Tone::Warn => (t.warning, t.warning.opacity(0.13)),
            Tone::Good => (t.success, t.success.opacity(0.13)),
        };
        let pressable = badge.run != Run::Nothing;
        let mut pill = row_start()
            .id(("menu-badge", ix))
            .role(if pressable { Role::Button } else { Role::Label })
            .aria_label(badge.label.clone())
            .test_support()
            .flex_none()
            .h(px(20.))
            .px(space(2.))
            .gap(space(1.))
            .rounded_full()
            .bg(bg)
            .text_color(fg)
            .text_size(theme::size::XXS)
            .font_weight(FontWeight::MEDIUM)
            .when(pressable, |d| d.cursor_pointer())
            .child(icon(badge.icon).size(px(11.)).flex_none())
            .child(div().whitespace_nowrap().child(isolate(badge.label.clone())));
        if let Some(tip) = badge.tip.clone() {
            pill = pill.tooltip(move |window, cx| tooltip(tip.clone(), window, cx));
        }
        if pressable {
            let run = badge.run.clone();
            pill = pill.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                cx.emit(MenuViewEvent::Chose(run.clone()));
            }));
        }
        pill.into_any_element()
    }

    /// The facts line of a head: what the item is, then (when there is room) when it changed. Badges that
    /// arrived after the menu opened, when there was no line for them, are said here too, so the head never
    /// changes height.
    fn facts_of(&self, head: &Head) -> SharedString {
        let mut parts: Vec<String> = Vec::new();
        if !head.line.is_empty() {
            parts.push(head.line.to_string());
        }
        if !self.badge_line {
            parts.extend(head.badges.iter().map(|b| b.label.to_string()));
        }
        if let Some(changed) = &head.changed {
            parts.push(changed.to_string());
        }
        parts.join(" · ").into()
    }

    /// A menu's head: the picture, the name, a line of facts, and the badges when there are any.
    fn render_head(&self, head: &Head, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let picture = match &head.picture {
            HeadPicture::Kind(kind) => art::kind(kind, px(36.), cx),
            HeadPicture::Folder => art::folder(px(36.), theme::folder_tone(cx), None, cx),
            HeadPicture::Several => art::page(px(36.), t.muted_foreground, Some(IconName::Files), cx),
        };
        // At most three badges; with more, the third place says how many are not shown.
        let shown: Vec<Badge> = if head.badges.len() > MOST_BADGES {
            let hidden = &head.badges[MOST_BADGES - 1..];
            let mut shown: Vec<Badge> = head.badges[..MOST_BADGES - 1].to_vec();
            let names = hidden.iter().map(|b| b.label.to_string()).collect::<Vec<_>>();
            shown.push(Badge {
                label: format!("+{}", hidden.len()).into(),
                icon: IconName::Ellipsis,
                tone: Tone::Plain,
                tip: Some(names.join(", ").into()),
                run: Run::Nothing,
            });
            shown
        } else {
            head.badges.clone()
        };
        let mut said = vec![head.line.to_string()];
        said.extend(head.badges.iter().map(|b| b.label.to_string()));
        let description: SharedString = said.join(", ").into();
        let badges: Vec<AnyElement> = if self.badge_line {
            shown
                .iter()
                .enumerate()
                .map(|(i, badge)| self.render_badge(i, badge, t, cx))
                .collect()
        } else {
            Vec::new()
        };
        row_start()
            .id("menu-head")
            .role(Role::Group)
            .aria_label(head.title.clone())
            .aria_description(description)
            .test_support()
            .w_full()
            .flex_none()
            .gap(space(2.5))
            .px(space(2.))
            .pt(space(1.5))
            .pb(space(1.75))
            .mb(space(1.))
            .border_b_1()
            .border_color(t.border)
            .child(picture)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .items_start()
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_start()
                            .text_size(theme::size::SM)
                            .font_weight(FontWeight::MEDIUM)
                            .child(isolate(head.title.clone())),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_start()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(rtl_line(self.facts_of(head))),
                    )
                    .when(self.badge_line, |text| {
                        // Always 20 px high once there is one, so the head never changes height.
                        text.child(
                            row_start()
                                .w_full()
                                .h(px(20.))
                                .flex_none()
                                .mt(px(3.))
                                .gap(space(1.))
                                .overflow_hidden()
                                .children(badges),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The line a filter starts with: what is typed, a caret, and how to take it back.
    fn render_filter(&self, t: &theme::Tokens) -> AnyElement {
        row_start()
            .id("menu-filter")
            .role(Role::SearchInput)
            .aria_label(t!("Filter: {query}", query = self.query))
            .test_support()
            .w_full()
            .flex_none()
            .gap(space(2.))
            .px(space(2.))
            .py(space(1.75))
            .mb(space(1.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::SM)
            .child(
                icon(IconName::Search)
                    .size_4()
                    .flex_none()
                    .text_color(t.muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(isolate(self.query.clone())),
            )
            .child(div().flex_none().w(px(1.5)).h(px(15.)).bg(t.primary))
            .child(div().flex_1())
            .child(
                div()
                    .flex_none()
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(t!("Esc")),
            )
            .into_any_element()
    }

    /// A submenu's row: its name and an arrow toward where its flyout opens.
    #[allow(clippy::too_many_arguments)]
    fn render_menu_row(
        &self,
        lv: usize,
        ix: usize,
        label: &SharedString,
        glyph: Option<IconName>,
        disabled: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let stop = Stop::Row(ix);
        let lit = self.is_lit(lv, stop);
        let opened = self.opener_lit(Spot::Row(lv, ix));
        let selected = lit && (self.by_keys || opened);
        let accent = t.accent;
        let geometry = self.geometry.clone();
        let mut row = row_start()
            .id(("menu-row", lv * LEVEL_STEP + ix))
            .role(Role::MenuItem)
            .aria_label(label.clone())
            .aria_expanded(opened)
            .test_support()
            .w_full()
            .gap(space(2.))
            .px(space(2.))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .text_size(theme::size::SM)
            .text_start()
            .cursor_pointer()
            .when(selected || opened, |d| d.bg(accent))
            .when(!disabled && !selected && !opened, |d| {
                d.hover(move |style| style.bg(accent))
            })
            .when(disabled, |d| d.opacity(0.4).cursor_default());
        if lit {
            row = row.aria_active_descendant();
        }
        if !disabled {
            row = row
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered {
                        this.pointed(Spot::Row(lv, ix), window.mouse_position(), cx);
                    }
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.pressed_row(lv, ix, cx)));
        }
        row.children(glyph.map(|g| icon(g).size_4().flex_none().text_color(t.muted_foreground)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_start()
                    .child(isolate(label.clone())),
            )
            .child(
                // The arrow points toward where the submenu goes: the end of the line.
                icon(forward_chevron())
                    .size_4()
                    .flex_none()
                    .ms(6.)
                    .text_color(t.muted_foreground)
                    .opacity(0.7),
            )
            // Where the row is, for the flyout to be placed by.
            .child(
                canvas(
                    move |bounds, _, _| {
                        geometry.borrow_mut().rows.insert((lv, ix), bounds);
                    },
                    |_, _, _, _| (),
                )
                .absolute()
                .inset_0(),
            )
            .into_any_element()
    }

    fn render_strip(
        &self,
        lv: usize,
        ix: usize,
        entries: &[Entry],
        last: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let place = lv * LEVEL_STEP + ix;
        row_start()
            .id(("menu-strip", place))
            .role(Role::Group)
            .aria_label(t!("Quick actions"))
            .test_support()
            .w_full()
            .flex_none()
            .gap(space(0.5))
            .px(space(0.5))
            .py(space(0.5))
            .when(!last, |d| {
                d.mb(space(1.)).pb(space(1.5)).border_b_1().border_color(t.border)
            })
            .children(entries.iter().enumerate().map(|(j, entry)| {
                let stop = Stop::Button(ix, j);
                let lit = self.is_lit(lv, stop);
                let selected = lit && self.by_keys;
                let tip: SharedString = match &entry.hint {
                    Some(hint) => format!("{} ({hint})", entry.label).into(),
                    None => entry.label.clone(),
                };
                let tone = if entry.danger {
                    t.destructive
                } else if entry.active {
                    t.primary
                } else {
                    t.popover_foreground.opacity(0.85)
                };
                let fill = if entry.danger {
                    t.destructive.opacity(0.15)
                } else {
                    t.accent
                };
                let mut button = div()
                    .id(("menu-strip-button", place * 32 + j))
                    .role(Role::MenuItem)
                    .aria_label(entry.label.clone())
                    .test_support()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(space(1.5))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(gpui_kit::transparent_black())
                    .text_color(tone)
                    .cursor_pointer()
                    // The keyboard's button has a ring as well as the fill.
                    .when(selected, |d| d.bg(fill).border_color(t.ring))
                    .when(!selected && !entry.disabled, |d| d.hover(move |style| style.bg(fill)))
                    .when(entry.disabled, |d| d.opacity(0.35).cursor_default());
                if lit {
                    button = button.aria_active_descendant();
                }
                if !entry.disabled {
                    button = button
                        .tooltip(move |window, cx| tooltip(tip.clone(), window, cx))
                        .tooltip_show_delay(TIP_AFTER)
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.cancel_timers();
                                this.set_at(lv, Stop::Button(ix, j), cx);
                                this.active = lv;
                                this.close_beyond(lv + 1, cx);
                            }
                        }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.cancel_timers();
                            this.set_at(lv, Stop::Button(ix, j), cx);
                            this.active = lv;
                            this.activate(cx);
                        }));
                }
                button.children(entry.icon.map(|name| icon(name).size_4().flex_none()))
            }))
            .into_any_element()
    }

    /// The rows of column `lv`: every node past the head and the strips that stay put.
    fn render_rows(&self, lv: usize, t: &theme::Tokens, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(level) = self.levels.get(lv) else {
            return Vec::new();
        };
        let nodes = &level.nodes;
        let count = nodes.len();
        let first = level.first_scrolled();
        nodes
            .iter()
            .enumerate()
            .skip(first)
            .filter_map(|(i, node)| {
                Some(match node {
                    Node::Separator => div()
                        .h(px(1.))
                        .my(space(1.))
                        .bg(t.border.opacity(0.7))
                        .into_any_element(),
                    Node::Heading(text) => {
                        // The strip and the head have their own hairline: no second line right under them.
                        let first = i == 0 || matches!(nodes.get(i - 1), Some(Node::Strip(_) | Node::Head(_)));
                        div()
                            .px(space(2.))
                            .pb(space(1.))
                            .pt(space(if first { 1.5 } else { 2. }))
                            .text_size(theme::size::XS)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.muted_foreground)
                            .text_start()
                            .when(!first, |d| d.mt(space(1.)).border_t_1().border_color(t.border))
                            .child(isolate(text.clone()))
                            .into_any_element()
                    }
                    Node::Strip(entries) => self.render_strip(lv, i, entries, i + 1 == count, t, cx),
                    Node::Head(_) => return None,
                    Node::Item(entry) => self.render_row(Spot::Row(lv, i), entry, None, t, cx),
                    Node::Menu {
                        label, icon, disabled, ..
                    } => self.render_menu_row(lv, i, label, *icon, *disabled, t, cx),
                })
            })
            .collect()
    }

    /// Column `lv`'s scrolling list and the scroll bar and bottom fade over it.
    fn render_scroller(
        &self,
        lv: usize,
        height: Pixels,
        rows: Vec<AnyElement>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let handle = self.scrolls.get(lv).cloned().unwrap_or_default();
        let id: ElementId = if lv == 0 {
            "menu-rows".into()
        } else {
            ("menu-rows", lv).into()
        };
        let list = col()
            .id(id)
            .w_full()
            .min_h_0()
            .max_h(height)
            .overflow_y_scroll()
            .track_scroll(&handle)
            // What a flyout is placed by moves with the scroll: it goes with it.
            .on_scroll(cx.listener(move |this, _, _, cx| this.close_beyond(lv + 1, cx)))
            .children(rows);
        // The bottom edge fades while there is more below: painted from the scroll handle as it is at that
        // frame, so a glide or a drag of the bar is never a frame behind.
        let fade = {
            let handle = handle.clone();
            let popover = t.popover;
            canvas(
                |_, _, _| (),
                move |bounds: Bounds<Pixels>, _, window, _| {
                    let more = handle.max_offset().y + handle.offset().y > px(1.);
                    if more {
                        let strip = Bounds {
                            origin: point(bounds.left(), bounds.bottom() - px(20.)),
                            size: size(bounds.size.width, px(20.)),
                        };
                        window.paint_quad(gpui_kit::fill(
                            strip,
                            gpui_kit::linear_gradient(
                                180.,
                                gpui_kit::linear_color_stop(popover.opacity(0.), 0.),
                                gpui_kit::linear_color_stop(popover, 1.),
                            ),
                        ));
                    }
                },
            )
            .absolute()
            .inset_0()
        };
        div()
            .relative()
            .w_full()
            .min_h_0()
            .child(list)
            .child(fade)
            .vertical_scrollbar(&handle)
            .into_any_element()
    }

    /// Column `lv`'s frame: its head and strips, its rows, and the notes of where it is.
    #[allow(clippy::too_many_arguments)]
    fn render_column(
        &self,
        lv: usize,
        cap: Pixels,
        typing: bool,
        extras: Vec<AnyElement>,
        arrival: Arrival,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let level = &self.levels[lv];
        let nodes = &level.nodes;
        let first = level.first_scrolled();
        // The head is drawn above the list that scrolls, and is not one of its rows.
        let head = (lv == 0 && !typing)
            .then(|| {
                nodes.iter().find_map(|node| match node {
                    Node::Head(head) => Some(self.render_head(head, t, cx)),
                    _ => None,
                })
            })
            .flatten();
        let strips: Vec<AnyElement> = if lv == 0 && typing {
            Vec::new()
        } else {
            nodes[..first]
                .iter()
                .enumerate()
                .filter_map(|(i, node)| match node {
                    Node::Strip(entries) => Some(self.render_strip(lv, i, entries, i + 1 == nodes.len(), t, cx)),
                    _ => None,
                })
                .collect()
        };
        let pinned_room = if lv == 0 && typing {
            0.
        } else {
            STRIP_ROOM * strips.len() as f32
        };
        let reserved = FRAME_ROOM
            + pinned_room
            + if lv == 0 && typing {
                FILTER_ROOM
            } else if head.is_some() {
                HEAD_ROOM + if self.badge_line { BADGE_LINE_ROOM } else { 0. }
            } else {
                0.
            };
        let rows_height = (cap - px(reserved)).max(px(80.));
        let (filter, rows): (Option<AnyElement>, Vec<AnyElement>) = if lv == 0 && typing {
            // What typing shows in place of the rows: a line saying what is typed, and what matches it.
            let found = if self.found.is_empty() {
                let none = t!("Nothing in this menu matches");
                vec![div()
                    .id("menu-nothing")
                    .role(Role::Label)
                    .aria_label(none.clone())
                    .test_support()
                    .px(space(2.))
                    .py(space(2.))
                    .text_size(theme::size::SM)
                    .text_color(t.muted_foreground)
                    .text_start()
                    .child(none)
                    .into_any_element()]
            } else {
                self.found
                    .iter()
                    .enumerate()
                    .map(|(i, found)| self.render_row(Spot::Found(i), &found.entry, found.path.as_ref(), t, cx))
                    .collect()
            };
            (Some(self.render_filter(t)), found)
        } else {
            (None, self.render_rows(lv, t, cx))
        };
        let scroller = self.render_scroller(lv, rows_height, rows, t, cx);

        // The menu keeps the height it had at rest while typing, so the few rows found don't make it jump.
        let kept_height = (lv == 0 && typing)
            .then(|| self.rest_height.get())
            .filter(|height| *height > 0.);
        let noted = self.rest_height.clone();
        let geometry = self.geometry.clone();
        let note = canvas(
            move |bounds: Bounds<Pixels>, _, _| {
                let mut geometry = geometry.borrow_mut();
                if geometry.frames.len() <= lv {
                    geometry.frames.resize(lv + 1, Bounds::default());
                }
                geometry.frames[lv] = bounds;
                if lv == 0 && !typing {
                    // The layer is inside the menu's hairline: a pixel at the top and one at the bottom.
                    noted.set(f32::from(bounds.size.height) + 2.);
                }
            },
            |_, _, _, _| (),
        )
        .absolute()
        .inset_0();

        // A menu with a head is wider, so the head has room for its badges (and the width doesn't change as
        // a flyout opens and closes).
        let wide = if self.has_head() { 17. } else { 14.5 };
        let (id, name): (ElementId, SharedString) = match &level.title {
            None => ("menu".into(), self.label.clone()),
            Some(title) => (("menu-flyout", lv).into(), format!("{title} ›").into()),
        };
        let column = col()
            .id(id)
            .role(Role::Menu)
            .aria_label(name)
            .test_support()
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| this.pressed_at(event.position, cx)))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, _| this.moved_over(lv, event.position)))
            .occlude()
            .min_w(rems(wide))
            .max_w(rems(26.))
            .max_h(cap)
            .p(space(1.))
            .rounded(theme::size::RADIUS_SHEET)
            .border_1()
            .border_color(t.stroke)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow(theme::elevation::floating(cx))
            .text_start()
            .when_some(kept_height, |menu, height| menu.min_h(px(height)))
            // The menu itself holds the keyboard; its flyouts are drawn inside it, in layers of their own.
            .when(lv == 0, |menu| {
                menu.key_context("Menu")
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::on_key_down))
            })
            .children(head)
            .children(filter)
            .children(strips)
            .child(scroller)
            .child(note)
            .children(extras);
        // A column arrives once, when it opens: it fades in and grows the last few percent from where it hangs.
        // Reduced motion has it there at once.
        if cx.reduce_motion() {
            return column.into_any_element();
        }
        let Arrival {
            key,
            duration,
            from: (grow_x, grow_y),
        } = arrival;
        column
            .with_animation(
                key,
                Animation::new(duration).with_easing(theme::motion::enter),
                move |menu, delta| {
                    let pop = theme::motion::POP;
                    menu.opacity(delta)
                        .paint_origin(grow_x, grow_y)
                        .paint_scale(pop + (1. - pop) * delta)
                },
            )
            .into_any_element()
    }

    /// A flyout: column `lv`, placed at the end side of the column before it, level with the row that opened
    /// it, and on the start side (or higher up) when there is no room, in a layer over everything.
    fn render_flyout(&self, lv: usize, cap: Pixels, t: &theme::Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let level = self.levels.get(lv)?;
        let (frame, row) = {
            let geometry = self.geometry.borrow();
            (
                geometry.frames.get(lv - 1).copied()?,
                geometry.rows.get(&(lv - 1, level.from)).copied()?,
            )
        };
        // The target is the column's width at the row's height: the flyout hangs from its end edge.
        let target = Bounds {
            origin: point(frame.left() - px(1.), row.top()),
            size: size(frame.size.width + px(2.), row.size.height),
        };
        let (corner, anchor, overlap) = if i18n::is_rtl() {
            (Anchor::TopLeft, Anchor::TopRight, OVERLAP)
        } else {
            (Anchor::TopRight, Anchor::TopLeft, -OVERLAP)
        };
        let column = self.render_column(
            lv,
            cap,
            false,
            Vec::new(),
            Arrival {
                key: ("flyout-enter", level.born).into(),
                duration: theme::motion::FAST,
                from: (if i18n::is_rtl() { 1. } else { 0. }, 0.),
            },
            t,
            cx,
        );
        Some(
            deferred(
                anchored()
                    .attach(target, corner)
                    .anchor(anchor)
                    .offset(point(px(overlap), px(-6.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(column),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY + lv)
            .into_any_element(),
        )
    }
}

/// The submenu arrow: points right in English, left in Arabic.
fn forward_chevron() -> IconName {
    if i18n::is_rtl() {
        IconName::ChevronLeft
    } else {
        IconName::ChevronRight
    }
}

impl Focusable for MenuView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        if !self.announced {
            // The pointer is where the menu opened: the tooltip of what it opened over (up already, or about to
            // come after its delay) must not cover it.
            self.announced = true;
            window.dismiss_tooltips();
            window.defer(cx, gpui_kit::component::tooltip::dismiss_all);
        }
        if let Some((lv, child)) = self.scroll_to.take() {
            // The children aren't measured until they are drawn once; ask again next frame if need be.
            match self.scrolls.get(lv) {
                Some(scroll) if scroll.bounds_for_item(child).is_some() => scroll.scroll_to_item(child),
                Some(_) => {
                    self.scroll_to = Some((lv, child));
                    cx.notify();
                }
                None => {}
            }
        }
        let typing = !self.query.is_empty();
        // Where each column's lit row is: the keyboard's, or in a column with a flyout open, the row it is from.
        self.lit = (0..self.levels.len())
            .map(|lv| {
                if lv == self.active {
                    self.levels[lv].stop()
                } else if let Some(next) = self.levels.get(lv + 1) {
                    Some(Stop::Row(next.from))
                } else {
                    self.levels[lv].stop()
                }
            })
            .collect();
        let cap = menu_max_height(window);

        let flyouts: Vec<AnyElement> = if typing {
            Vec::new()
        } else {
            (1..self.levels.len())
                .filter_map(|lv| self.render_flyout(lv, cap, &t, cx))
                .collect()
        };
        // It grows from the corner it hangs from: the pointer's, or (for a menu that doesn't know its point, the
        // dock's) the side of the window it is nearer to.
        let upward = match (self.at, self.geometry.borrow().frames.first()) {
            (Some(at), Some(frame)) => frame.center().y < at.y,
            (None, Some(frame)) => frame.center().y > window.viewport_size().height * 0.5,
            _ => false,
        };
        let (grow_x, grow_y) = (if i18n::is_rtl() { 1. } else { 0. }, if upward { 1. } else { 0. });
        self.render_column(
            0,
            cap,
            typing,
            flyouts,
            Arrival {
                key: "menu-enter".into(),
                duration: theme::motion::BASE,
                from: (grow_x, grow_y),
            },
            &t,
            cx,
        )
    }
}
