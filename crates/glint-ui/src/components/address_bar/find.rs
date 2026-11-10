//! The search box at the end of the bar (the `<input id="find-box">` and its hints in AddressBar.tsx).
//!
//! Typing in it filters the folder shown (`Explorer::set_filter`), as the TypeScript's `updateTab({filter})`
//! did. Searching the subfolders is the find part's: the box tells it with [`AddressBarEvent`]s.

use super::*;
use glint_model::entries::asks_models;
use glint_model::js::is_js_space;
use glint_model::paths::is_folder_path;
use glint_model::plain::plain_query;
use search::{has_git_filter, ready_query, starts_with_drive, Offer, FILTERS, GIT_FILTER, PROP_FILTERS};

/// Typing waits this long before a search as you type starts (the TypeScript's 100 ms).
const AS_TYPED_AFTER: Duration = Duration::from_millis(100);

impl AddressBar {
    /// Puts the tab's filter in the box when something else changed it (clearing a search, another tab), and
    /// says what the box searches.
    pub(super) fn sync_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let filter = self.seen.filter.clone();
        if self.search.read(cx).value().as_ref() != filter.as_str() {
            self.plain = self.read_plain(&filter);
            self.search.update(cx, |input, cx| input.set_value(filter, window, cx));
        }
        let placeholder = t!("Search {folder}", folder = self.seen.folder_name);
        if self.placeholder != placeholder {
            self.placeholder = placeholder.clone();
            self.search
                .update(cx, |input, cx| input.set_placeholder(placeholder, window, cx));
        }
    }

    /// What plain words in the box mean as filters, while the box has the keyboard.
    fn read_plain(&self, text: &str) -> Option<glint_model::plain::Plain> {
        if self.find_focused {
            plain_query(text)
        } else {
            None
        }
    }

    pub(super) fn on_search_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let value = self.search.read(cx).value().to_string();
                self.filter_changed(value, cx);
            }
            InputEvent::PressEnter { .. } => self.run_search(window, cx),
            InputEvent::Focus => {
                self.find_focused = true;
                self.plain = self.read_plain(&self.search.read(cx).value());
                cx.notify();
            }
            InputEvent::Blur => {
                self.find_focused = false;
                self.plain = None;
                cx.notify();
            }
        }
    }

    /// The text in the box changed (`onFilterChange`): it filters the folder now, and may search below it.
    pub(super) fn filter_changed(&mut self, value: String, cx: &mut Context<Self>) {
        self.find_timer = None;
        self.plain = self.read_plain(&value);
        self.explorer
            .update(cx, |explorer, cx| explorer.set_filter(&value, None, cx));
        let trimmed = value.trim_matches(is_js_space);
        if trimmed.is_empty() || has_git_filter(trimmed) {
            if self.seen.has_find {
                cx.emit(AddressBarEvent::LeaveFind);
            }
            cx.notify();
            return;
        }
        let path = &self.seen.path;
        // Where there is no folder to search below, only a drive path typed in the box does.
        if path != THIS_PC && !is_folder_path(path) && !starts_with_drive(trimmed) {
            cx.notify();
            return;
        }
        let query = ready_query(trimmed);
        // Subfolders as you type only where the index answers in milliseconds: after the pause the explorer is
        // asked, and it decides (`findsAsTyped`, which also reads where instant search stands if that is not
        // known yet). An fd walk waits for Enter.
        if !query.is_empty() && !asks_models(&query) {
            let explorer = self.explorer.clone();
            self.find_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(AS_TYPED_AFTER).await;
                let running = this
                    .update(cx, |_, cx| {
                        explorer.update(cx, |explorer, cx| explorer.find_as_typed(&query, cx))
                    })
                    .ok();
                if let Some(running) = running {
                    running.await;
                }
            }));
        }
        cx.notify();
    }

    /// Enter: searches the folder's subfolders too.
    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_timer = None;
        let filter = self.search.read(cx).value().to_string();
        // Git's marks are the folder shown's: a search of its subfolders couldn't ask them.
        if has_git_filter(&filter) {
            window.push_notification(
                Notification::info(t!("Git's marks are for the open folder only."))
                    .title(t!("git: filters this folder, as you type")),
                cx,
            );
            return;
        }
        cx.emit(AddressBarEvent::RunFind { query: filter });
    }

    /// Tab: the filters read from plain words go in the box.
    fn take_plain(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.plain.as_ref().map(|p| p.query.clone()) {
            Some(query) => self.set_search_text(query, window, cx),
            None => cx.propagate(),
        }
    }

    /// Puts text in the box, as typing it would (a hint was clicked, Tab took the plain words' filters).
    fn set_search_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| {
            input.set_value(text.clone(), window, cx);
            input.focus(window, cx);
        });
        self.filter_changed(text, cx);
    }

    /// Escape: clears the search and goes back to the list, like Explorer.
    fn leave_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_timer = None;
        if !self.search.read(cx).value().is_empty() || self.seen.has_find {
            self.clear_search(window, cx);
        }
        window.focus(&self.focus, cx);
        cx.emit(AddressBarEvent::FocusFiles { first: false });
    }

    /// The clear button: the filter empties and the folder itself comes back.
    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_timer = None;
        self.plain = None;
        self.search.update(cx, |input, cx| input.set_value("", window, cx));
        self.explorer
            .update(cx, |explorer, cx| explorer.set_filter("", None, cx));
        cx.emit(AddressBarEvent::ClearFind);
        cx.notify();
    }

    /// The search box with its hints.
    pub(super) fn search_box(
        &mut self,
        t: &theme::Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let seen = self.seen.clone();
        let rem = window.rem_size();
        let viewport = window.viewport_size();
        let has_text = !seen.filter.is_empty() || seen.has_find;
        // In a narrow window, just its magnifier until it's clicked into or holds a search.
        let width = if viewport.width >= rem * 40. {
            (viewport.width * 0.22).max(rem * 8.).min(rem * 19.)
        } else if self.find_focused || has_text {
            (rem * 16.).min(viewport.width * 0.5)
        } else {
            rem * 2. + px(2.)
        };
        let rtl = i18n::is_rtl();
        let name = SharedString::from(seen.folder_name.clone());

        let clear = has_text.then(|| {
            Button::new("address-search-clear")
                .ghost()
                .xsmall()
                .icon(IconName::X)
                .accessibility_label(t!("Clear search"))
                .tooltip(t!("Clear (Esc)"))
                .on_click(cx.listener(|this, _, window, cx| this.clear_search(window, cx)))
        });
        let input = Input::new(&self.search)
            .aria_label(t!("Search {folder} and its subfolders.", folder = name))
            .prefix(icon(IconName::Search).size_4().text_color(t.muted_foreground))
            .suffix(div().children(clear));

        let plain = self.plain.clone().map(|plain| {
            let query = plain.query.clone();
            col()
                .id("address-plain")
                .role(Role::Button)
                .aria_label(plain.query.clone())
                .test_support()
                .occlude()
                .gap(space(1.5))
                .p(space(1.5))
                .rounded(theme::size::RADIUS)
                .border_1()
                .border_color(t.border)
                .bg(t.popover)
                .text_color(t.popover_foreground)
                .text_size(theme::size::XS)
                .shadow_lg()
                .cursor_pointer()
                // Mouse down keeps the focus in the box; a click takes the filters, as Tab does.
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(cx.listener(move |this, _, window, cx| this.set_search_text(query.clone(), window, cx)))
                .child(
                    row_start()
                        .flex_wrap()
                        .gap(space(1.))
                        .children(plain.chips.iter().map(|chip| {
                            div()
                                .px(space(1.5))
                                .py(space(0.5))
                                .rounded(theme::size::RADIUS_SM)
                                .bg(t.primary.opacity(0.12))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.primary)
                                .child(chip.clone())
                        })),
                )
                .child(
                    row_start()
                        .gap(space(1.5))
                        .min_w_0()
                        .text_color(t.muted_foreground)
                        .children(Keystroke::parse("tab").ok().map(Kbd::new))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family(theme::mono_font(cx))
                                .text_size(theme::size::XXS)
                                .child(plain.query.clone()),
                        ),
                )
        });

        let hints = (self.find_focused && seen.filter.is_empty() && !seen.has_find).then(|| {
            let offers = |title: Option<SharedString>, list: Vec<Offer>, base: usize, cx: &mut Context<Self>| {
                col()
                    .mt(space(1.5))
                    .children(title.map(|title| {
                        div()
                            .px(space(1.5))
                            .pb(space(0.5))
                            .pt(space(1.))
                            .text_color(t.muted_foreground)
                            .child(title)
                    }))
                    .children(list.into_iter().enumerate().map(|(i, (start, example, meaning))| {
                        row_start()
                            .id(("address-offer", base + i))
                            .role(Role::Button)
                            .aria_label(example)
                            .test_support()
                            .items_baseline()
                            .gap(space(2.))
                            .px(space(1.5))
                            .py(space(1.))
                            .rounded(theme::size::RADIUS_SM)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.accent))
                            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.set_search_text(start.to_string(), window, cx)
                            }))
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(theme::mono_font(cx))
                                    .text_size(theme::size::XXS)
                                    .text_color(t.primary)
                                    .child(example),
                            )
                            .child(div().text_color(t.muted_foreground).child(i18n::translate(meaning)))
                    }))
            };
            let mut first: Vec<Offer> = FILTERS.to_vec();
            if self.in_repository {
                first.push(GIT_FILTER);
            }
            let base = first.len();
            let height = (viewport.height - rem * 3.5).max(px(0.));
            col()
                .id("address-hints")
                .role(Role::Group)
                .aria_label(t!("Search filters"))
                .test_support()
                .occlude()
                .max_h(height)
                .overflow_y_scroll()
                .p(space(2.5))
                .rounded(theme::size::RADIUS)
                .border_1()
                .border_color(t.border)
                .bg(t.popover)
                .text_color(t.popover_foreground)
                .text_size(theme::size::XS)
                .shadow_lg()
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .child(div().text_color(t.muted_foreground).child(t!(
                    "Typing filters this folder. Enter searches its subfolders too, by name or path: as you type, where instant search covers them."
                )))
                .child(offers(None, first, 0, cx))
                .child(offers(
                    Some(t!("What photos, videos and songs say (read from the files):")),
                    PROP_FILTERS.to_vec(),
                    base,
                    cx,
                ))
        });

        let overlay = |content: AnyElement| {
            deferred(
                div()
                    .absolute()
                    .top_full()
                    .mt(space(1.))
                    .when(rtl, |d| d.left_0())
                    .when(!rtl, |d| d.right_0())
                    .w_full()
                    .min_w(rems(19.))
                    .child(content),
            )
            .with_priority(10)
        };

        div()
            .id("address-search")
            .test_support()
            .key_context(keys::FIND)
            .relative()
            .flex_none()
            .w(width)
            .on_action(cx.listener(|this, _: &ListComplete, window, cx| this.take_plain(window, cx)))
            .on_action(cx.listener(|_, _: &ListDown, _, cx| cx.emit(AddressBarEvent::FocusFiles { first: true })))
            .on_action(cx.listener(|this, _: &ListCancel, window, cx| this.leave_search(window, cx)))
            .tooltip(tip(t!(
                "Typing filters this folder · Enter searches its subfolders too, by name or path, with the instant search index or fd (* and ? work, words all must match); where the index covers them, they're searched as you type. Filters: ext:pdf, path:src\\lib, size:>10mb, dm:7d (changed in the last 7 days), dm:>1y, file:, folder:, and for photos, videos and songs width:>1920, height:, taken:2024, camera:, duration:>5m, artist:, album:; shows: (what photos show) and says: (what recordings say), with photo and speech search on in Settings"
            )))
            .child(input)
            .children(plain.map(|p| overlay(p.into_any_element())))
            .children(hints.map(|h| overlay(h.into_any_element())))
    }
}
