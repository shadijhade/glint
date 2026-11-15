//! Draws a menu and drives it from the keyboard (the popover in src/components/Menu.tsx).
//!
//! The kit's `PopupMenu` is not used for this because Glint's menus need three things it doesn't have: a strip of
//! icon buttons at the top, headings, and submenus that replace the menu in place with a Back row (as the
//! TypeScript's did, which keeps a long menu one column wide); and because its rows are not mirrored for the
//! Arabic interface. This view is those, with the kit's tokens and the same keys.
//!
//! **Keys.** Down and Up move through everything that can be chosen, round the ends; Home and End jump; Enter
//! and Space choose; the arrow toward the end of the line (Right in English, Left in Arabic) enters a submenu or,
//! in the strip, moves to the next button; the arrow toward the start (and Backspace) goes back; Escape closes.
//! The first row past the strip is selected when a menu opens, as `focusFirst` did.
//!
//! **The Aware wave** (docs/gpui/aware/PLAN.md) adds four things. A menu may start with a *head* (the item's
//! picture, name, facts and badges: [`Node::Head`]); a row may carry a small *reason* line, be drawn as a
//! *suggestion*, or say why it is greyed out; typing in an open menu *filters* every row, submenus included
//! (`filter::matches`); and going into a submenu or back *slides* the rows in from the side they came from.

use super::filter::{self, Found};
use super::spec::{Badge, Entry, Head, HeadPicture, Node, Run, Tone};
use crate::kit::prelude::*;
use crate::kit::rtl_line;
use crate::{art, i18n, t, theme};
use gpui_kit::rems;

/// The longest the typed filter may grow: nothing is as long as this in a menu.
const MOST_TYPED: usize = 64;
/// How many badges a head draws; with more, the last place says how many are not shown.
const MOST_BADGES: usize = 3;
/// What a head and the menu's frame take from the room the rows may use (so the whole menu fits the window).
const HEAD_ROOM: f32 = 96.;
/// The same for the filter line.
const FILTER_ROOM: f32 = 56.;
/// The frame's own padding and border.
const FRAME_ROOM: f32 = 10.;

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
    /// The Back row of a submenu.
    Back,
    /// A row or a submenu, by its place in the level's nodes.
    Row(usize),
    /// A button of the strip: the strip's place, then the button's.
    Button(usize, usize),
}

/// Which row a drawn row is: one of the level's nodes, or one the filter found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Spot {
    Row(usize),
    Found(usize),
}

/// Whether a key's text is something to type: there is some, and none of it is a control character.
fn visible_text(text: &str) -> bool {
    !text.is_empty() && !text.chars().any(char::is_control)
}

/// A menu being shown: the top one, or a submenu entered from it.
struct Level {
    /// The submenu's name, for its Back row; none for the top.
    title: Option<SharedString>,
    nodes: Vec<Node>,
    /// Where the keyboard is, as an index into [`Level::stops`].
    at: Option<usize>,
}

impl Level {
    fn new(title: Option<SharedString>, nodes: Vec<Node>) -> Level {
        let mut level = Level { title, nodes, at: None };
        let stops = level.stops();
        // The first row past the strip and Back; failing that the first thing there is.
        level.at = stops
            .iter()
            .position(|s| matches!(s, Stop::Row(_)))
            .or(if stops.is_empty() { None } else { Some(0) });
        level
    }

    /// Everything that can be chosen, in the order Down goes through it.
    fn stops(&self) -> Vec<Stop> {
        let mut stops = Vec::new();
        if self.title.is_some() {
            stops.push(Stop::Back);
        }
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
}

/// A menu on screen.
pub struct MenuView {
    levels: Vec<Level>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// Scroll this child of the list into view at the next frame (the keyboard moved to it).
    scroll_to: Option<usize>,
    /// Where the keyboard was when the last frame was drawn, so each row doesn't work it out again.
    cur: Option<Stop>,
    /// What a screen reader calls the menu.
    label: SharedString,
    /// What has been typed: the menu shows the rows that match it, in place of itself.
    query: String,
    /// The rows that match `query`, best first.
    found: Vec<Found>,
    /// Which of `found` the keyboard is on.
    found_at: usize,
    /// How many times the menu went into a submenu or back: what keys the rows' slide.
    moves: usize,
    /// Which way the last move went: 1 into a submenu (the rows arrive from the end edge), -1 back, 0 none yet.
    slide: i8,
    /// How tall the menu was drawn when nothing was typed: it keeps that height while typing, so the few rows
    /// found don't make it jump to another place in the window.
    rest_height: std::rc::Rc<std::cell::Cell<f32>>,
}

impl MenuView {
    /// A menu of `nodes` (already cleaned by [`visible_items`](super::spec::visible_items)).
    pub fn new(nodes: Vec<Node>, label: SharedString, cx: &mut Context<Self>) -> MenuView {
        MenuView {
            levels: vec![Level::new(None, nodes)],
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            scroll_to: None,
            cur: None,
            label,
            query: String::new(),
            found: Vec::new(),
            found_at: 0,
            moves: 0,
            slide: 0,
            rest_height: Default::default(),
        }
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

    /// The nodes now shown (the top menu's, or the submenu's).
    pub fn nodes(&self) -> &[Node] {
        self.levels.last().map_or(&[], |l| &l.nodes)
    }

    /// How many menus deep the view is (1 at the top).
    pub fn depth(&self) -> usize {
        self.levels.len()
    }

    /// The label of the row the keyboard is on, for tests and screen readers.
    #[cfg(test)]
    pub fn selected_label(&self) -> Option<SharedString> {
        if !self.query.is_empty() {
            return self.found.get(self.found_at).map(|f| f.entry.label.clone());
        }
        let level = self.levels.last()?;
        match level.stop()? {
            Stop::Back => level.title.clone(),
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

    // ---- Moving -----------------------------------------------------------------------------------------

    /// Where the child of the scroll list for `stop` is: the Back row first, then one child per node (the head
    /// is drawn above the list, so it is not one).
    fn child_of(level: &Level, stop: Stop) -> usize {
        let back = usize::from(level.title.is_some());
        let before = |i: usize| level.nodes.iter().take(i).filter(|n| !matches!(n, Node::Head(_))).count();
        match stop {
            Stop::Back => 0,
            Stop::Row(i) | Stop::Button(i, _) => before(i) + back,
        }
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(level) = self.levels.last_mut() else { return };
        level.at = Some(at);
        if let Some(stop) = level.stops().get(at).copied() {
            self.scroll_to = Some(Self::child_of(level, stop));
        }
        cx.notify();
    }

    /// Down (`step` 1) or Up (-1), round the ends.
    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(level) = self.levels.last() else { return };
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
        let count = self.levels.last().map_or(0, |l| l.stops().len());
        if count > 0 {
            self.move_to(if last { count - 1 } else { 0 }, cx);
        }
    }

    /// The arrow toward the end of the line (`toward_end`) or the start: in the strip it moves along the buttons,
    /// otherwise toward the end enters a submenu and toward the start goes back.
    fn sideways(&mut self, toward_end: bool, cx: &mut Context<Self>) {
        let Some(level) = self.levels.last() else { return };
        let stops = level.stops();
        match level.stop() {
            Some(Stop::Button(strip, button)) => {
                let next = if toward_end {
                    stops
                        .iter()
                        .position(|s| *s == Stop::Button(strip, button))
                        .map(|p| p + 1)
                } else {
                    stops
                        .iter()
                        .position(|s| *s == Stop::Button(strip, button))
                        .and_then(|p| p.checked_sub(1))
                };
                if let Some(next) = next.filter(|n| matches!(stops.get(*n), Some(Stop::Button(s, _)) if *s == strip)) {
                    self.move_to(next, cx);
                }
            }
            Some(Stop::Row(i)) if toward_end => {
                if matches!(level.nodes.get(i), Some(Node::Menu { disabled: false, .. })) {
                    self.enter(i, cx);
                }
            }
            _ if !toward_end => self.leave(cx),
            _ => {}
        }
    }

    /// Shows the submenu at node `i` in the menu's place.
    fn enter(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(Node::Menu { label, items, .. }) = self.levels.last().and_then(|l| l.nodes.get(i)).cloned() else {
            return;
        };
        let level = Level::new(Some(label), items);
        self.scroll_to = level
            .at
            .map(|at| level.stops().get(at).map_or(0, |s| Self::child_of(&level, *s)));
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.levels.push(level);
        self.moves += 1;
        self.slide = 1;
        cx.notify();
    }

    /// Back to the menu this one was entered from.
    fn leave(&mut self, cx: &mut Context<Self>) {
        if self.levels.len() > 1 {
            self.levels.pop();
            self.moves += 1;
            self.slide = -1;
            self.scroll.set_offset(point(px(0.), px(0.)));
            if let Some(level) = self.levels.last() {
                self.scroll_to = level.stop().map(|s| Self::child_of(level, s));
            }
            cx.notify();
        }
    }

    /// Chooses what the keyboard is on.
    fn activate(&mut self, cx: &mut Context<Self>) {
        let Some(level) = self.levels.last() else { return };
        match level.stop() {
            Some(Stop::Back) => self.leave(cx),
            Some(Stop::Row(i)) => match level.nodes.get(i) {
                Some(Node::Item(entry)) => self.choose(entry.clone(), cx),
                Some(Node::Menu { .. }) => self.enter(i, cx),
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
            "escape" => cx.emit(MenuViewEvent::Dismissed),
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

    // ---- Typing -----------------------------------------------------------------------------------------

    /// Adds what was typed to the filter.
    fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.query.chars().count() + text.chars().count() <= MOST_TYPED {
            self.query.push_str(text);
        }
        self.refilter(cx);
    }

    /// Looks through the whole menu (the top one, whichever level shows) for what is typed.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        self.found = match self.levels.first() {
            Some(top) if !self.query.is_empty() => filter::matches(&top.nodes, &self.query),
            _ => Vec::new(),
        };
        self.found_at = 0;
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    fn move_found(&mut self, at: usize, cx: &mut Context<Self>) {
        self.found_at = at;
        self.scroll_to = Some(at);
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

    // ---- Drawing ----------------------------------------------------------------------------------------

    /// Whether the keyboard is on `stop` (as of the frame being drawn).
    fn is_at(&self, stop: Stop) -> bool {
        self.cur == Some(stop)
    }

    /// The pointer is over `stop`: the keyboard follows it.
    fn hover(&mut self, stop: Stop, cx: &mut Context<Self>) {
        let Some(level) = self.levels.last_mut() else { return };
        if let Some(at) = level.stops().iter().position(|s| *s == stop) {
            if level.at != Some(at) {
                level.at = Some(at);
                cx.notify();
            }
        }
    }

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
        let (id, selected) = match spot {
            Spot::Row(ix) => (ElementId::from(("menu-row", ix)), self.is_at(Stop::Row(ix))),
            Spot::Found(i) => (ElementId::from(("menu-found", i)), self.found_at == i),
        };
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
            .when(selected, |d| d.bg(t.accent))
            .when(entry.danger, |d| d.text_color(t.destructive))
            .when(entry.disabled, |d| d.opacity(0.4).cursor_default());
        if let Some(checked) = entry.check {
            row = row.aria_toggled(if checked { Toggled::True } else { Toggled::False });
        }
        if selected {
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
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.point_at(spot, cx);
                    }
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.point_at(spot, cx);
                    match spot {
                        Spot::Row(_) => this.activate(cx),
                        Spot::Found(_) => this.choose_found(cx),
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

    /// The pointer is over `spot`: the keyboard follows it.
    fn point_at(&mut self, spot: Spot, cx: &mut Context<Self>) {
        match spot {
            Spot::Row(ix) => self.hover(Stop::Row(ix), cx),
            Spot::Found(i) => {
                if self.found_at != i {
                    self.found_at = i;
                    cx.notify();
                }
            }
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

    /// A menu's head: the picture, the name, a line of facts, and the badges with when it changed.
    fn render_head(&self, head: &Head, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let picture = match &head.picture {
            HeadPicture::Kind(kind) => art::kind(kind, px(40.), cx),
            HeadPicture::Folder => art::folder(px(40.), theme::folder_tone(cx), None, cx),
            HeadPicture::Several => art::page(px(40.), t.muted_foreground, Some(IconName::Files), cx),
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
        let badges: Vec<AnyElement> = shown
            .iter()
            .enumerate()
            .map(|(i, badge)| self.render_badge(i, badge, t, cx))
            .collect();
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
            .pt(space(2.))
            .pb(space(2.25))
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
                            .child(rtl_line(head.line.clone())),
                    )
                    .child(
                        // Always 20 px high, even with nothing in it, so the head never changes height.
                        row_start()
                            .w_full()
                            .h(px(20.))
                            .flex_none()
                            .mt(px(3.))
                            .gap(space(1.))
                            .overflow_hidden()
                            .children(badges)
                            .children(head.changed.clone().map(|changed| {
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_start()
                                    .text_size(theme::size::XXS)
                                    .text_color(t.muted_foreground)
                                    .child(isolate(changed))
                            })),
                    ),
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
            .child(icon(IconName::Search).size_4().flex_none().text_color(t.muted_foreground))
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

    fn render_menu_row(
        &self,
        ix: usize,
        label: &SharedString,
        glyph: Option<IconName>,
        disabled: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let stop = Stop::Row(ix);
        let selected = self.is_at(stop);
        let mut row = row_start()
            .id(("menu-row", ix))
            .role(Role::MenuItem)
            .aria_label(label.clone())
            .aria_expanded(false)
            .test_support()
            .w_full()
            .gap(space(2.))
            .px(space(2.))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .text_size(theme::size::SM)
            .text_start()
            .cursor_pointer()
            .when(selected, |d| d.bg(t.accent))
            .when(disabled, |d| d.opacity(0.4).cursor_default());
        if selected {
            row = row.aria_active_descendant();
        }
        if !disabled {
            row = row
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.hover(stop, cx);
                    }
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.hover(stop, cx);
                    this.activate(cx);
                }));
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
            .into_any_element()
    }

    fn render_strip(
        &self,
        ix: usize,
        entries: &[Entry],
        last: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        row_start()
            .id(("menu-strip", ix))
            .role(Role::Group)
            .aria_label(t!("Quick actions"))
            .test_support()
            .w_full()
            .gap(space(0.5))
            .px(space(0.5))
            .py(space(0.5))
            .when(!last, |d| {
                d.mb(space(1.)).pb(space(1.5)).border_b_1().border_color(t.border)
            })
            .children(entries.iter().enumerate().map(|(j, entry)| {
                let stop = Stop::Button(ix, j);
                let selected = self.is_at(stop);
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
                let mut button = div()
                    .id(("menu-strip-button", ix * 32 + j))
                    .role(Role::MenuItem)
                    .aria_label(entry.label.clone())
                    .test_support()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(space(1.5))
                    .rounded(theme::size::RADIUS)
                    .text_color(tone)
                    .cursor_pointer()
                    .when(selected, |d| {
                        d.bg(if entry.danger {
                            t.destructive.opacity(0.15)
                        } else {
                            t.accent
                        })
                    })
                    .when(entry.disabled, |d| d.opacity(0.35).cursor_default());
                if selected {
                    button = button.aria_active_descendant();
                }
                if !entry.disabled {
                    button = button
                        .tooltip(move |window, cx| tooltip(tip.clone(), window, cx))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered {
                                this.hover(stop, cx);
                            }
                        }))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.hover(stop, cx);
                            this.activate(cx);
                        }));
                }
                button.children(entry.icon.map(|name| icon(name).size_4().flex_none()))
            }))
            .into_any_element()
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
        if let Some(child) = self.scroll_to.take() {
            // The children aren't measured until they are drawn once; ask again next frame if need be.
            if self.scroll.bounds_for_item(child).is_some() {
                self.scroll.scroll_to_item(child);
            } else {
                self.scroll_to = Some(child);
                cx.notify();
            }
        }
        let typing = !self.query.is_empty();
        // At rest the menu's height is noted as it is painted; while typing it is kept.
        let noted = self.rest_height.clone();
        let note_height = (!typing).then(|| {
            canvas(
                |_, _, _| (),
                // The layer is inside the menu's hairline: a pixel at the top and one at the bottom.
                move |bounds: Bounds<Pixels>, _, _, _| noted.set(f32::from(bounds.size.height) + 2.),
            )
            .absolute()
            .inset_0()
        });
        let kept_height = typing.then(|| self.rest_height.get()).filter(|height| *height > 0.);
        self.cur = self.levels.last().and_then(Level::stop);
        let no_nodes: Vec<Node> = Vec::new();
        let (title, nodes) = match self.levels.last() {
            Some(level) => (level.title.clone(), &level.nodes),
            None => (None, &no_nodes),
        };
        let count = nodes.len();
        // The head is drawn above the list that scrolls, and is not one of its rows.
        let head = if typing {
            None
        } else {
            nodes.iter().find_map(|node| match node {
                Node::Head(head) => Some(self.render_head(head, &t, cx)),
                _ => None,
            })
        };
        let rows: Vec<AnyElement> = if typing {
            Vec::new()
        } else {
            nodes
                .iter()
                .enumerate()
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
                        Node::Strip(entries) => self.render_strip(i, entries, i + 1 == count, &t, cx),
                        Node::Head(_) => return None,
                        Node::Item(entry) => self.render_row(Spot::Row(i), entry, None, &t, cx),
                        Node::Menu {
                            label, icon, disabled, ..
                        } => self.render_menu_row(i, label, *icon, *disabled, &t, cx),
                    })
                })
                .collect()
        };
        // What typing shows in their place: a line saying what is typed, and what matches it.
        let filter = typing.then(|| self.render_filter(&t));
        let found: Vec<AnyElement> = if typing {
            if self.found.is_empty() {
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
                    .map(|(i, found)| self.render_row(Spot::Found(i), &found.entry, found.path.as_ref(), &t, cx))
                    .collect()
            }
        } else {
            Vec::new()
        };

        let back = title.filter(|_| !typing).map(|title| {
            let selected = self.is_at(Stop::Back);
            row_start()
                .id("menu-back")
                .role(Role::MenuItem)
                .aria_label(title.clone())
                .aria_description(t!("Back"))
                .test_support()
                .w_full()
                .gap(space(2.))
                .px(space(2.))
                .py(space(1.5))
                .mb(space(1.))
                .rounded(theme::size::RADIUS_SM)
                .border_b_1()
                .border_color(t.border)
                .text_size(theme::size::XS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.muted_foreground)
                .text_start()
                .cursor_pointer()
                .when(selected, |d| d.bg(t.accent).text_color(t.foreground))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.hover(Stop::Back, cx);
                    }
                }))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.leave(cx)))
                .child(icon(back_icon()).size_4().flex_none())
                .child(div().flex_1().min_w_0().truncate().text_start().child(isolate(title)))
        });

        // No taller than the window: what doesn't fit scrolls (the rows; the head and the filter stay).
        let max_height = (window.viewport_size().height - px(8.)).max(px(120.));
        let reserved = FRAME_ROOM
            + if typing {
                FILTER_ROOM
            } else if head.is_some() {
                HEAD_ROOM
            } else {
                0.
            };
        let rows_height = (max_height - px(reserved)).max(px(80.));
        // A menu with a head is wider, so the head has room for its badges (and the width doesn't change as
        // the keyboard goes into a submenu and back).
        let wide = if self.has_head() { 17. } else { 14.5 };
        // The rows slide in from the side they came from when the menu goes into a submenu or back (not when
        // typing starts or ends: the key stays as it was).
        let slide = self.slide;
        let toward_end = if i18n::is_rtl() { -1. } else { 1. };
        let rows = col()
            .id("menu-rows")
            .w_full()
            .min_h_0()
            .max_h(rows_height)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(back)
            .children(rows)
            .children(found)
            .with_animation(
                ("menu-slide", self.moves),
                Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
                move |rows, delta| {
                    if slide == 0 {
                        return rows;
                    }
                    let from = f32::from(slide) * toward_end * 10. * (1. - delta);
                    rows.opacity(0.4 + 0.6 * delta).paint_offset(px(from), px(0.))
                },
            );
        // The menu arrives once, when it opens: there at once, settling the last few pixels as it fades in.
        col()
            .id("menu")
            .role(Role::Menu)
            .aria_label(self.label.clone())
            .test_support()
            .key_context("Menu")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(MenuViewEvent::Dismissed)))
            .occlude()
            .min_w(rems(wide))
            .max_w(rems(26.))
            .max_h(max_height)
            .p(space(1.))
            .rounded(theme::size::RADIUS_SHEET)
            .border_1()
            .border_color(t.stroke)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow(theme::elevation::floating(cx))
            .text_start()
            .when_some(kept_height, |menu, height| menu.min_h(px(height)))
            .children(head)
            .children(filter)
            .child(rows)
            .children(note_height)
            .with_animation(
                "menu-enter",
                Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
                // It pops: there from the first frame, growing the last few percent as it clears.
                |menu, delta| {
                    let pop = theme::motion::POP;
                    menu.opacity(0.4 + 0.6 * delta)
                        .top(px(-5. * (1. - delta)))
                        .paint_scale(pop + (1. - pop) * delta)
                },
            )
    }
}
