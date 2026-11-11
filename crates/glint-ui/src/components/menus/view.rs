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

use super::spec::{Entry, Node, Run};
use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use gpui_kit::rems;

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
        }
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
    pub fn selected_label(&self) -> Option<SharedString> {
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

    // ---- Moving -----------------------------------------------------------------------------------------

    /// Where the child of the scroll list for `stop` is: the Back row first, then one child per node.
    fn child_of(level: &Level, stop: Stop) -> usize {
        let back = usize::from(level.title.is_some());
        match stop {
            Stop::Back => 0,
            Stop::Row(i) | Stop::Button(i, _) => i + back,
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
        cx.notify();
    }

    /// Back to the menu this one was entered from.
    fn leave(&mut self, cx: &mut Context<Self>) {
        if self.levels.len() > 1 {
            self.levels.pop();
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
        match event.keystroke.key.as_str() {
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            "home" => self.jump(false, cx),
            "end" => self.jump(true, cx),
            "enter" | "space" => self.activate(cx),
            "right" => self.sideways(!rtl, cx),
            "left" => self.sideways(rtl, cx),
            "backspace" => self.leave(cx),
            "escape" => cx.emit(MenuViewEvent::Dismissed),
            _ => return,
        }
        cx.stop_propagation();
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

    fn render_row(&self, ix: usize, entry: &Entry, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let stop = Stop::Row(ix);
        let selected = self.is_at(stop);
        let role = match entry.check {
            Some(_) => Role::MenuItemCheckBox,
            None => Role::MenuItem,
        };
        let tint = if entry.danger {
            t.destructive
        } else {
            t.muted_foreground
        };
        let mut row = row_start()
            .id(("menu-row", ix))
            .role(role)
            .aria_label(entry.label.clone())
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
            .when(entry.danger, |d| d.text_color(t.destructive))
            .when(entry.disabled, |d| d.opacity(0.4).cursor_default());
        if let Some(checked) = entry.check {
            row = row.aria_toggled(if checked { Toggled::True } else { Toggled::False });
        }
        if selected {
            row = row.aria_active_descendant();
        }
        if !entry.disabled {
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
        row.children(Self::icon_of(entry, Some(tint)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_start()
                    .child(isolate(entry.label.clone())),
            )
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
        self.cur = self.levels.last().and_then(Level::stop);
        let no_nodes: Vec<Node> = Vec::new();
        let (title, nodes) = match self.levels.last() {
            Some(level) => (level.title.clone(), &level.nodes),
            None => (None, &no_nodes),
        };
        let count = nodes.len();
        let rows: Vec<AnyElement> = nodes
            .iter()
            .enumerate()
            .map(|(i, node)| match node {
                Node::Separator => div()
                    .h(px(1.))
                    .my(space(1.))
                    .bg(t.border.opacity(0.7))
                    .into_any_element(),
                Node::Heading(text) => div()
                    .px(space(2.))
                    .pb(space(1.))
                    .pt(space(if i > 0 { 2. } else { 1.5 }))
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.muted_foreground)
                    .text_start()
                    .when(i > 0, |d| d.mt(space(1.)).border_t_1().border_color(t.border))
                    .child(isolate(text.clone()))
                    .into_any_element(),
                Node::Strip(entries) => self.render_strip(i, entries, i + 1 == count, &t, cx),
                Node::Item(entry) => self.render_row(i, entry, &t, cx),
                Node::Menu {
                    label, icon, disabled, ..
                } => self.render_menu_row(i, label, *icon, *disabled, &t, cx),
            })
            .collect();

        let back = title.map(|title| {
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

        // No taller than the window: what doesn't fit scrolls.
        let max_height = (window.viewport_size().height - px(8.)).max(px(120.));
        // A menu arrives: there at once, settling the last few pixels as it fades in (a level deeper or back
        // arrives the same way).
        let level = self.levels.len();
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
            .min_w(rems(14.5))
            .max_w(rems(26.))
            .max_h(max_height)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(space(1.))
            .rounded(theme::size::RADIUS_SHEET)
            .border_1()
            .border_color(t.stroke)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow(theme::elevation::floating(cx))
            .text_start()
            .children(back)
            .children(rows)
            .with_animation(
                ("menu-enter", level),
                Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
                |menu, delta| menu.opacity(0.4 + 0.6 * delta).top(px(-5. * (1. - delta))),
            )
    }
}
