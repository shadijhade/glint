//! The search box at the end of the bar (the `<input id="find-box">` and its hints in AddressBar.tsx).
//!
//! Typing in it filters the folder shown (`Explorer::set_filter`), as the TypeScript's `updateTab({filter})`
//! did. Searching the subfolders is the find part's: the box tells it with [`AddressBarEvent`]s.

use super::*;
use glint_model::js::is_js_space;
use glint_model::paths::is_folder_path;
use glint_model::plain::plain_query;
use glint_model::store::{SavedSearch, SearchOptionKey, SearchOptions};
use search::{has_git_filter, ready_query, starts_with_drive, Offer, FILTERS, GIT_FILTER, PROP_FILTERS};

/// How many of the last searches the empty box offers.
const RECENT_SHOWN: usize = 5;

/// How long typing rests before a search as you type starts. Only the index is asked as you type, and it
/// answers in milliseconds, so the pause is not there to spare the search: it spares the list, which a letter or
/// two would fill with half the drive only for the next letter to throw it away. From three letters on the
/// results follow the keys.
pub(super) fn as_typed_after(query: &str) -> Duration {
    Duration::from_millis(match query.chars().count() {
        0 | 1 => 100,
        2 => 60,
        _ => 20,
    })
}

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
                self.hint_at = None;
                self.filter_changed(value, cx);
            }
            // The hint the arrows are on is what Enter takes; with none, it searches.
            InputEvent::PressEnter { .. } => {
                if !self.take_hint(window, cx) {
                    self.run_search(window, cx)
                }
            }
            InputEvent::Focus => {
                self.find_focused = true;
                self.plain = self.read_plain(&self.search.read(cx).value());
                cx.notify();
            }
            InputEvent::Blur => {
                self.find_focused = false;
                self.hint_at = None;
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
        if !query.is_empty() {
            let explorer = self.explorer.clone();
            let pause = as_typed_after(&query);
            self.find_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(pause).await;
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
        // Kept among the recent searches the empty box offers (when Settings keep them).
        if !filter.trim().is_empty() {
            let (query, folder) = (filter.clone(), self.seen.path.clone());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            crate::persistence::update_app(cx, |app| app.add_to_history(&query, &folder, now));
        }
        cx.emit(AddressBarEvent::RunFind { query: filter });
    }

    /// The switches of the search inside files (match case, whole words, a regular expression; documents,
    /// pictures' text), as they are saved.
    fn options(cx: &App) -> SearchOptions {
        crate::persistence::app(cx).map(|app| app.options).unwrap_or_default()
    }

    /// Flips one of those switches; results on screen were found the other way, so they are searched for again.
    fn toggle_option(&mut self, key: SearchOptionKey, cx: &mut Context<Self>) {
        let o = Self::options(cx);
        let on = match key {
            SearchOptionKey::CaseSensitive => o.case_sensitive,
            SearchOptionKey::WholeWord => o.whole_word,
            SearchOptionKey::Regex => o.regex,
            SearchOptionKey::Documents => o.documents,
            SearchOptionKey::Ocr => o.ocr,
        };
        crate::persistence::update_app(cx, |app| app.set_option(key, !on));
        if self.seen.has_find {
            cx.emit(AddressBarEvent::RerunFind {
                query: self.seen.filter.clone(),
                scope: String::new(),
            });
        }
        cx.notify();
    }

    /// One of the recent searches was chosen: its folder opens with its words searched for.
    fn run_recent(&mut self, query: String, folder: String, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| {
            explorer.run_saved_search(SavedSearch { query, folder }, cx).detach()
        });
    }

    /// Whether the empty box's hints are showing.
    fn hints_open(&self) -> bool {
        self.find_focused && self.seen.filter.is_empty() && !self.seen.has_find
    }

    /// The last searches the empty box offers (query and folder), newest first.
    fn recent_searches(cx: &App) -> Vec<(String, String)> {
        crate::persistence::app(cx)
            .map(|app| {
                app.history
                    .iter()
                    .take(RECENT_SHOWN)
                    .map(|item| (item.query.clone(), item.path.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The filters the hints offer, in the order they are drawn: the filters of any file, then what photos,
    /// videos and songs say.
    fn filter_offers(&self) -> (Vec<Offer>, Vec<Offer>) {
        let mut first: Vec<Offer> = FILTERS.to_vec();
        if self.in_repository {
            first.push(GIT_FILTER);
        }
        (first, PROP_FILTERS.to_vec())
    }

    /// How many hints there are to step through: the recent searches, then the filters.
    fn hint_count(&self, cx: &App) -> usize {
        let (first, props) = self.filter_offers();
        Self::recent_searches(cx).len() + first.len() + props.len()
    }

    /// Down in the empty box: to the next hint (the last stays). With no hints showing it goes to the list.
    fn hint_down(&mut self, cx: &mut Context<Self>) {
        let count = self.hint_count(cx);
        if !self.hints_open() || count == 0 {
            cx.emit(AddressBarEvent::FocusFiles { first: true });
            return;
        }
        self.hint_at = Some(self.hint_at.map_or(0, |at| (at + 1).min(count - 1)));
        cx.notify();
    }

    /// Up in the empty box: to the hint before, and from the first back to the box. Elsewhere it is the box's.
    fn hint_up(&mut self, cx: &mut Context<Self>) {
        match self.hint_at.filter(|_| self.hints_open()) {
            Some(at) => {
                self.hint_at = at.checked_sub(1);
                cx.notify();
            }
            None => cx.propagate(),
        }
    }

    /// Enter on the hint the arrows are on: a recent search runs again, a filter goes into the box. False when
    /// the arrows are on none.
    fn take_hint(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(at) = self.hint_at.filter(|_| self.hints_open()) else {
            return false;
        };
        self.hint_at = None;
        let recent = Self::recent_searches(cx);
        if let Some((query, folder)) = recent.get(at) {
            self.run_recent(query.clone(), folder.clone(), cx);
            return true;
        }
        let (first, props) = self.filter_offers();
        let offers = first.into_iter().chain(props);
        if let Some((start, _, _)) = offers.into_iter().nth(at - recent.len()) {
            self.set_search_text(start.to_string(), window, cx);
        }
        true
    }

    /// Delete on a recent search the arrows are on, or its × button: forgets it (what is saved loses it too).
    fn forget_recent(&mut self, query: &str, folder: &str, cx: &mut Context<Self>) {
        crate::persistence::update_app(cx, |app| app.remove_from_history(query, folder));
        // The arrows stay near where they were.
        let count = self.hint_count(cx);
        self.hint_at = self
            .hint_at
            .and_then(|at| count.checked_sub(1).map(|last| at.min(last)));
        cx.notify();
    }

    /// Delete with the arrows on a recent search forgets it; anywhere else it edits the box.
    fn forget_current(&mut self, cx: &mut Context<Self>) {
        let recent = Self::recent_searches(cx);
        match self.hint_at.filter(|_| self.hints_open()).and_then(|at| recent.get(at)) {
            Some((query, folder)) => self.forget_recent(query, folder, cx),
            None => cx.propagate(),
        }
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
        self.hint_at = None;
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

    /// The search box with its hints: the end of the bar's field. What is typed is looked for in names and, on
    /// Enter, inside the files too; its switches say how the words are matched there.
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
        let open = self.find_focused || has_text;
        // It takes more of the field while it is used. In a narrow window, just its magnifier until then.
        let width = if viewport.width >= rem * 40. {
            if open {
                (viewport.width * 0.3).max(rem * 12.).min(rem * 26.)
            } else {
                (viewport.width * 0.2).max(rem * 9.).min(rem * 16.)
            }
        } else if open {
            (rem * 16.).min(viewport.width * 0.5)
        } else {
            rem * 2. + px(2.)
        };
        if let Some((was_open, prev_w)) = self.find_was_open {
            if was_open != open {
                self.find_prev_width = prev_w;
                self.find_width_turn += 1;
            }
        }
        self.find_was_open = Some((open, width));
        let rtl = i18n::is_rtl();
        let name = SharedString::from(seen.folder_name.clone());
        let options = Self::options(cx);

        let clear = has_text.then(|| {
            Button::new("address-search-clear")
                .ghost()
                .xsmall()
                .icon(IconName::X)
                .accessibility_label(t!("Clear search"))
                .tooltip(t!("Clear (Esc)"))
                .on_click(cx.listener(|this, _, window, cx| this.clear_search(window, cx)))
        });
        // How the words are matched inside files: shown while the box is in use, lit while on.
        let switch = |id: &'static str, glyph: IconName, key: SearchOptionKey, on: bool, tip: SharedString| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(glyph)
                .selected(on)
                .accessibility_label(tip.clone())
                .tooltip(tip)
                // The box keeps the keyboard.
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_option(key, cx)))
        };
        let switches = open.then(|| {
            row_start()
                .id("address-search-switches")
                .gap(px(1.))
                .child(switch(
                    "address-search-case",
                    IconName::CaseSensitive,
                    SearchOptionKey::CaseSensitive,
                    options.case_sensitive,
                    t!("Match case, inside files (Alt+C)"),
                ))
                .child(switch(
                    "address-search-word",
                    IconName::WholeWord,
                    SearchOptionKey::WholeWord,
                    options.whole_word,
                    t!("Whole words, inside files (Alt+W)"),
                ))
                .child(switch(
                    "address-search-regex",
                    IconName::Regex,
                    SearchOptionKey::Regex,
                    options.regex,
                    t!("Regular expression, inside files (Alt+R)"),
                ))
        });
        let input = Input::new(&self.search)
            .appearance(false)
            .h_full()
            .aria_label(t!("Search {folder} and its subfolders.", folder = name))
            .prefix(icon(IconName::Search).size_4().text_color(t.muted_foreground))
            .suffix(row_start().gap(px(1.)).children(switches).children(clear));

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
                .rounded(theme::size::RADIUS_XL)
                .border_1()
                .border_color(t.stroke)
                .bg(t.popover)
                .text_color(t.popover_foreground)
                .text_size(theme::size::XS)
                .shadow(theme::elevation::floating(cx))
                .cursor_pointer()
                .hover(|s| s.bg(t.accent))
                .active(|s| s.bg(t.pressed))
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

        let current_hint = self.hint_at;
        let hints = self.hints_open().then(|| {
            // `first_place`: where this group starts among all the hints the arrows step through.
            let offers = |title: Option<SharedString>,
                          list: Vec<Offer>,
                          base: usize,
                          first_place: usize,
                          cx: &mut Context<Self>| {
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
                        let current = current_hint == Some(first_place + i);
                        row_start()
                            .id(("address-offer", base + i))
                            .role(Role::Button)
                            .aria_label(example)
                            .aria_selected(current)
                            .test_support()
                            .items_baseline()
                            .gap(space(2.))
                            .px(space(1.5))
                            .py(space(1.))
                            .rounded(theme::size::RADIUS_SM)
                            .cursor_pointer()
                            .when(current, |s| s.bg(t.accent))
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
            let (first, props) = self.filter_offers();
            let base = first.len();
            let height = (viewport.height - rem * 3.5).max(px(0.));
            // Where else the words are looked for inside files, beyond their own text.
            let reach = |id: &'static str, glyph: IconName, key: SearchOptionKey, on: bool, label: SharedString, tip: SharedString, off: bool, cx: &mut Context<Self>| {
                Button::new(id)
                    .outline()
                    .xsmall()
                    .icon(glyph)
                    .label(label)
                    .selected(on)
                    .disabled(off)
                    .tooltip(tip)
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_option(key, cx)))
            };
            let reaches = row_start()
                .id("address-search-reach")
                .role(Role::Group)
                .aria_label(t!("Also look inside"))
                .test_support()
                .flex_wrap()
                .gap(space(1.5))
                .mt(space(2.))
                .child(div().text_color(t.muted_foreground).child(t!("Also look inside")))
                .child(reach(
                    "address-search-documents",
                    IconName::FileText,
                    SearchOptionKey::Documents,
                    options.documents,
                    t!("Documents"),
                    t!("The text in PDF, Word, Excel, PowerPoint, OpenDocument and EPUB files"),
                    false,
                    cx,
                ))
                .child(reach(
                    "address-search-pictures",
                    IconName::ScanText,
                    SearchOptionKey::Ocr,
                    options.ocr,
                    t!("Pictures"),
                    t!("With Documents: the text in pictures, read by Windows' OCR"),
                    !options.documents,
                    cx,
                ));
            // The last searches: one click opens its folder and runs it again.
            let recent = Self::recent_searches(cx);
            let recent_count = recent.len();
            let recents = (!recent.is_empty()).then(|| {
                col()
                    .id("address-search-recent")
                    .role(Role::Group)
                    .aria_label(t!("Recent searches"))
                    .test_support()
                    .mt(space(2.))
                    .child(
                        div()
                            .px(space(1.5))
                            .pb(space(0.5))
                            .text_color(t.muted_foreground)
                            .child(t!("Recent searches")),
                    )
                    .children(recent.into_iter().enumerate().map(|(i, (query, folder))| {
                        let place = glint_model::paths::base_name(&folder).to_string();
                        let (run_query, run_folder) = (query.clone(), folder.clone());
                        let (forget_query, forget_folder) = (query.clone(), folder.clone());
                        let forget_label = t!("Remove {query} from the recent searches", query = query);
                        let current = current_hint == Some(i);
                        let recent_group = SharedString::from(format!("address-recent-group-{i}"));
                        row_start()
                            .id(("address-recent", i))
                            .group(recent_group.clone())
                            .role(Role::Button)
                            .aria_label(t!("{query} in {folder}", query = query, folder = place))
                            .aria_selected(current)
                            .test_support()
                            .gap(space(2.))
                            .px(space(1.5))
                            .py(space(1.))
                            .rounded(theme::size::RADIUS_SM)
                            .cursor_pointer()
                            .when(current, |s| s.bg(t.accent))
                            .hover(|s| s.bg(t.accent))
                            .active(|s| s.bg(t.pressed))
                            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.run_recent(run_query.clone(), run_folder.clone(), cx)
                            }))
                            .child(icon(IconName::Clock).size_3p5().text_color(t.muted_foreground))
                            .child(div().min_w_0().flex_1().truncate().child(isolate(query)))
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(space(32.))
                                    .truncate()
                                    .text_color(t.muted_foreground)
                                    .child(isolate(place)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .opacity(if current { 1. } else { 0. })
                                    .group_hover(recent_group, |s| s.opacity(1.))
                                    .child(
                                        Button::new(("address-recent-forget", i))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::X)
                                            .accessibility_label(forget_label)
                                            .tooltip(t!("Remove from the recent searches (Del)"))
                                            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                // The row under the button would run the search.
                                                cx.stop_propagation();
                                                this.forget_recent(&forget_query, &forget_folder, cx)
                                            })),
                                    ),
                            )
                    }))
            });
            col()
                .id("address-hints")
                .role(Role::Group)
                .aria_label(t!("Search filters"))
                .test_support()
                .occlude()
                .max_h(height)
                .overflow_y_scroll()
                .p(space(2.5))
                .rounded(theme::size::RADIUS_SHEET)
                .border_1()
                .border_color(t.stroke)
                .bg(t.popover)
                .text_color(t.popover_foreground)
                .text_size(theme::size::XS)
                .shadow(theme::elevation::floating(cx))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .child(div().text_color(t.muted_foreground).child(t!(
                    "Typing filters this folder. Enter searches its subfolders too: names, and inside the files. Names are found as you type where instant search covers them."
                )))
                .child(reaches)
                .children(recents)
                .child(offers(None, first, 0, recent_count, cx))
                .child(offers(
                    Some(t!("What photos, videos and songs say (read from the files):")),
                    props,
                    base,
                    recent_count + base,
                    cx,
                ))
        });

        let reduce_motion = cx.reduce_motion();
        let overlay = |id: &'static str, content: AnyElement| {
            let content = if reduce_motion {
                content
            } else {
                div()
                    .with_animation(
                        id,
                        Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
                        |el, delta| {
                            el.opacity(0.35 + 0.65 * delta)
                                .paint_scale(theme::motion::POP + (1. - theme::motion::POP) * delta)
                        },
                    )
                    .child(content)
                    .into_any_element()
            };
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
            .with_priority(Z_HINTS)
        };

        let search_div =
            div()
                .id("address-search")
                .test_support()
                .key_context(keys::FIND)
                .relative()
                .flex_none()
                .h_full()
                .on_action(cx.listener(|this, _: &crate::actions::TextMatchCase, _, cx| {
                    this.toggle_option(SearchOptionKey::CaseSensitive, cx)
                }))
                .on_action(cx.listener(|this, _: &crate::actions::TextWholeWord, _, cx| {
                    this.toggle_option(SearchOptionKey::WholeWord, cx)
                }))
                .on_action(cx.listener(|this, _: &crate::actions::TextRegex, _, cx| {
                    this.toggle_option(SearchOptionKey::Regex, cx)
                }))
                .on_action(cx.listener(|this, _: &ListComplete, window, cx| this.take_plain(window, cx)))
                .on_action(cx.listener(|this, _: &ListDown, _, cx| this.hint_down(cx)))
                .on_action(cx.listener(|this, _: &ListUp, _, cx| this.hint_up(cx)))
                .on_action(cx.listener(|this, _: &ListRemove, _, cx| this.forget_current(cx)))
                .on_action(cx.listener(|this, _: &ListCancel, window, cx| this.leave_search(window, cx)))
                .when(!self.find_focused, |d| {
                    d.tooltip(tip(t!(
                        "Filter this folder, or press Enter to search subfolders (Ctrl+F)"
                    )))
                })
                .child(input)
                .children(plain.map(|p| overlay("address-plain-in", p.into_any_element())))
                .children(hints.map(|h| overlay("address-hints-in", h.into_any_element())));
        if self.find_width_turn > 0 && !reduce_motion {
            let from = self.find_prev_width;
            search_div
                .with_animation(
                    ("address-search-w", self.find_width_turn),
                    Animation::new(theme::motion::FAST).with_easing(gpui_kit::ease_in_out),
                    move |el, delta| el.w(from + (width - from) * delta),
                )
                .into_any_element()
        } else {
            search_div.w(width).into_any_element()
        }
    }
}
