//! Searching inside files: the query, its options, the results and the history (Search.tsx, Results.tsx,
//! History.tsx, and the text mode of App.tsx).
//!
//! | TypeScript                                          | Here                                                    |
//! |-----------------------------------------------------|---------------------------------------------------------|
//! | text mode of `App` (the `mode`, `searchText`)       | [`TextSearch`], the view the window shows instead of the files; [`TextSearch::enter`] |
//! | `SearchBar`                                         | `bar.rs`: the query, the switches, Replace, the folder, Search and Stop |
//! | `RipgrepCard`, `AstGrepCard`, `MeaningCard`         | `bar.rs`: `settings::tools::needed` for the two tools; the meaning card here |
//! | `Results`                                           | `results.rs`: the header, the filters, the grouped, virtualized rows and their menu |
//! | `Preview`, `DocumentLines`                          | `preview.rs`: the match in its file, or the document's lines around it |
//! | `Home`, `HistoryList`, `HistoryPopover`             | `home.rs`: the start screen and the list of recent searches |
//! | `search`, `closeResults`, `cancelSearch`            | [`model::Search`]: streaming, parsed off the UI thread, a newer search replacing the older |
//! | `searchBatched`, `invoke("run_…")`, file reads      | [`source::Source`], [`source::CoreSource`]               |
//! | `replaceAll`, `rewriteAll`                          | `TextSearch::replace_all` and `rewrite_all`              |
//!
//! # What is kept between runs
//!
//! As in the TypeScript (zustand's `persist` had no `partialize`), the query, the folder, the switches and
//! the history are part of the saved state: `persistence::app(cx)` holds them (`query`, `path`, `options`,
//! `history`), and they are changed with `persistence::update_app`. The Replace box's text and whether it is
//! open (`useRewrite`) are not kept.
//!
//! # Wiring (the coordinator)
//!
//! 1. Make the view once per window, **the first time text search is asked for** (its tools model looks at every
//!    tool once, which a start has no use for): `cx.new(|cx| TextSearch::new(window, cx))`, or
//!    `TextSearch::with_tools(settings_tools, window, cx)` to share Settings' tools model. Keep
//!    it alive while the window lives (the results keep their place while out of sight) and draw it **instead
//!    of the files** (the body below the toolbar, with the sidebar, status bar and preview pane not shown)
//!    while text mode is on. It fills its parent (`size_full`), has the key context `"TextSearch"` and tracks
//!    its own focus.
//! 2. Subscribe to it for [`TextSearchEvent`]s: [`TextSearchEvent::Reveal`] is `revealInApp(path)`: back to the
//!    files, then `Explorer::navigate(parent_path(path), NavOptions::default().selecting(path))`.
//! 3. **The `TextSearch` action** (`Ctrl+Shift+F`, the palette's "Search text in this folder"), and
//!    `MenuEvent::SearchText { folder }` / `SidebarEvent::SearchText`: call
//!    `text.update(cx, |t, cx| t.enter(folder, window, cx))` (`None` for the action: the open folder). When it
//!    returns `true`, show the view and give it the keyboard (`enter` already focuses the query); when
//!    `false` it has told the person why (an archive, a folder Windows keeps closed) and the files stay.
//!    In text mode `TextSearch` does nothing (`if (mode === "files") searchText()`).
//! 4. **The `BackToFiles` action** (`Ctrl+Shift+E`) and the mode switch's "Files": show the files again and give
//!    them the keyboard (`focus_files`). Nothing else is needed: the view keeps its state.
//! 5. The mode switch (Files | Text) and its tooltip "Search inside files with ripgrep (Ctrl+Shift+F)" are the
//!    toolbar's; the history button is in the search bar here, so the toolbar needs none
//!    ([`TextSearch::show_history`] opens the same list for anything that wants to).
//!
//! # The keys
//!
//! The actions already bound in `"TextSearch"` (`TextMatchCase`, `TextWholeWord`, `TextRegex`,
//! `TextCodePatterns`, `TextDocuments`, `TextPictures`, `TextMeaning`, `TextReplace`, `TextLeave`) are handled
//! here, on the view's root. The keys that only mean something inside this view are bound by the view itself,
//! once per app, in contexts of its own, and are listed in [`keys`]: `Ctrl+F` / `Ctrl+K` focus the query,
//! `Down` on the query's last line goes on to the results, `Up`/`Down`/`Enter` in the results, and `Escape`
//! inside a field.

mod bar;
mod home;
mod model;
mod preview;
mod results;
mod source;
#[cfg(test)]
mod tests;

pub use model::{export_text, parse, Match, RawEvent, Replaced, Run, Said, Search, SearchEvent, Spec};
pub use source::{CoreSource, Engine, Fut, Request, Source};

use crate::components::settings::tools::Tools;
use crate::components::settings::CoreBackend as SettingsCoreBackend;
use crate::kit::prelude::*;
use crate::kit::TextareaState;
use crate::{actions, core_bridge, persistence, state, t, theme};
use glint_model::paths::{is_folder_path, THIS_PC};
use glint_model::store::{HistoryItem, SearchOptionKey, SearchOptions};
use gpui_kit::{ScrollStrategy, Task, UniformListScrollHandle};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

actions!(
    glint_text_search,
    [
        /// Ctrl+F or Ctrl+K: the query box has the keyboard, its text selected.
        FocusQuery,
        /// Down in the query: on its last line it goes on to the results; elsewhere it moves the caret.
        QueryDown,
        /// Down in the results: the next match.
        ResultsNext,
        /// Up in the results: the previous match.
        ResultsPrevious,
        /// Enter in the results: open the match's file.
        ResultsOpen,
    ]
);

/// Marks that the keys are bound in this app.
struct Bound;

impl Global for Bound {}

/// The keys this view binds itself, with the key context each is in (see the module notes).
pub fn keys() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("ctrl-f", "TextSearch", "FocusQuery"),
        ("ctrl-k", "TextSearch", "FocusQuery"),
        ("ctrl-f", "TextSearch > Input", "FocusQuery"),
        ("ctrl-k", "TextSearch > Input", "FocusQuery"),
        ("down", "TextQuery > Input", "QueryDown"),
        ("down", "TextResults", "ResultsNext"),
        ("up", "TextResults", "ResultsPrevious"),
        ("enter", "TextResults", "ResultsOpen"),
        ("escape", "TextSearch > Input", "TextLeave"),
    ]
}

/// Binds [`keys`], once per app. Escape inside a field is a binding of its own because the field's own
/// binding is deeper than the one in `"TextSearch"`.
fn bind_keys(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    cx.bind_keys([
        KeyBinding::new("ctrl-f", FocusQuery, Some("TextSearch")),
        KeyBinding::new("ctrl-k", FocusQuery, Some("TextSearch")),
        KeyBinding::new("ctrl-f", FocusQuery, Some("TextSearch > Input")),
        KeyBinding::new("ctrl-k", FocusQuery, Some("TextSearch > Input")),
        KeyBinding::new("down", QueryDown, Some("TextQuery > Input")),
        KeyBinding::new("down", ResultsNext, Some("TextResults")),
        KeyBinding::new("up", ResultsPrevious, Some("TextResults")),
        KeyBinding::new("enter", ResultsOpen, Some("TextResults")),
        KeyBinding::new("escape", actions::TextLeave, Some("TextSearch > Input")),
    ]);
}

/// What the view asks of the rest of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextSearchEvent {
    /// "Show in Files" (`revealInApp`): back to the files, showing the file's folder with the file selected.
    Reveal { path: String },
}

/// How long after the last key the query and folder are saved (the TypeScript saved on every key).
const SAVE_AFTER: Duration = Duration::from_millis(400);

/// The text search view.
pub struct TextSearch {
    search: Entity<Search>,
    source: Rc<dyn Source>,
    /// The tools the install cards read (none without core).
    tools: Option<Entity<Tools>>,
    focus: FocusHandle,
    results_focus: FocusHandle,
    query: Entity<TextareaState>,
    replacement: Entity<InputState>,
    folder: Entity<InputState>,
    filter: Entity<InputState>,
    history_filter: Entity<InputState>,
    /// The Replace box is open (`useRewrite().open`); not kept between runs.
    replace_open: bool,
    history_open: bool,
    ext_filter: Vec<String>,
    collapsed: HashSet<String>,
    /// Changes whenever `collapsed` does, so the rows know to be built again.
    collapse_version: u64,
    grouping: results::Grouping,
    list: UniformListScrollHandle,
    preview: preview::Preview,
    /// The switches the boxes' placeholders and the Replacement box's state were last set for.
    fields_for: Option<(bool, bool, bool, bool, bool)>,
    /// The meaning model is being downloaded (asked for here; Settings' model has no progress to read).
    meaning_downloading: bool,
    saving: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TextSearchEvent> for TextSearch {}

impl TextSearch {
    /// The view, asking glint-core.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = cx.global::<core_bridge::Core>().app.clone();
        let source: Rc<dyn Source> = Rc::new(CoreSource::new(app.clone()));
        let tools = {
            let backend: Arc<dyn crate::components::settings::Backend> = Arc::new(SettingsCoreBackend::new(app));
            Some(cx.new(|cx| Tools::new(backend, window, cx)))
        };
        Self::with_source(source, tools, window, cx)
    }

    /// The view, asking glint-core, with a tools model that exists already (Settings' own, if the window shares
    /// one) for the install cards. [`TextSearch::new`] makes its own, which looks at every tool once.
    pub fn with_tools(tools: Entity<Tools>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let app = cx.global::<core_bridge::Core>().app.clone();
        Self::with_source(Rc::new(CoreSource::new(app)), Some(tools), window, cx)
    }

    /// The view, asking `source` (tests give a fake), with the tools model the install cards read.
    pub fn with_source(
        source: Rc<dyn Source>,
        tools: Option<Entity<Tools>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        bind_keys(cx);
        let (query_text, folder_text, options) = persistence::app(cx)
            .map(|app| (app.query.clone(), app.path.clone(), app.options))
            .unwrap_or_default();
        // An empty folder is filled from where the person is (`useEffect` of SearchBar).
        let folder_text = if folder_text.trim().is_empty() {
            let fallback = default_folder(cx);
            if !fallback.is_empty() {
                let shown = fallback.clone();
                persistence::update_app(cx, |app| app.path = shown);
            }
            fallback
        } else {
            folder_text
        };

        let search = cx.new(|cx| Search::new(source.clone(), cx));
        let query = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder(bar::query_placeholder(&options))
                .default_value(query_text)
        });
        let replacement = cx.new(|cx| InputState::new(window, cx));
        let folder = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("Folder to search…"))
                .default_value(folder_text)
        });
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter results")));
        let history_filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter recent searches…")));

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.save_soon(cx),
                InputEvent::PressEnter { .. } => this.search_now(window, cx),
                _ => {}
            }),
        );
        subscriptions.push(
            cx.subscribe_in(&folder, window, |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.save_soon(cx),
                InputEvent::PressEnter { .. } => this.search_now(window, cx),
                _ => {}
            }),
        );
        subscriptions.push(cx.subscribe_in(
            &replacement,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } if this.can_replace(cx) => this.replace_all(window, cx),
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        ));
        for input in [&filter, &history_filter] {
            subscriptions.push(cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        // The model's changes redraw, and a new search starts the filters over.
        subscriptions.push(cx.observe(&search, |_, _, cx| cx.notify()));
        subscriptions.push(
            cx.subscribe_in(&search, window, |this, _, event: &SearchEvent, window, cx| {
                if matches!(event, SearchEvent::Started | SearchEvent::Closed) {
                    this.reset_results(window, cx);
                }
            }),
        );
        subscriptions.push(
            cx.observe_global_in::<persistence::Persistence>(window, |this, window, cx| {
                this.sync_fields(window, cx);
                cx.notify();
            }),
        );
        subscriptions.push(cx.observe_global::<state::UiState>(|_, cx| cx.notify()));
        if let Some(tools) = &tools {
            subscriptions.push(cx.observe(tools, |this, tools, cx| {
                // The model arrived: the card has done its job.
                if tools.read(cx).meaning() != (false, false) {
                    this.meaning_downloading = false;
                }
                cx.notify();
            }));
        }

        let mut this = Self {
            search,
            source,
            tools,
            focus: cx.focus_handle(),
            results_focus: cx.focus_handle(),
            query,
            replacement,
            folder,
            filter,
            history_filter,
            replace_open: false,
            history_open: false,
            ext_filter: Vec::new(),
            collapsed: HashSet::new(),
            collapse_version: 0,
            grouping: results::Grouping::default(),
            list: UniformListScrollHandle::new(),
            preview: preview::Preview::default(),
            fields_for: None,
            meaning_downloading: false,
            saving: None,
            _subscriptions: subscriptions,
        };
        this.sync_fields(window, cx);
        this
    }

    // ---- Coming to the screen ---------------------------------------------------------------------------

    /// Text search in `folder` (the open folder by default), before the window shows this view (`searchText`).
    /// Returns whether the view may be shown: ripgrep reads files on disk, so an archive's contents, and
    /// folders Windows keeps from this account, are said so and refused. The query gets the keyboard.
    pub fn enter(&mut self, folder: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if folder.is_none() {
            if let Some(explorer) = crate::explorer::try_explorer(cx) {
                if explorer.read(cx).archive_of(None).is_some() {
                    window.push_notification(
                        Notification::info(t!(
                            "It searches files on disk. Copy what you want to search out of the archive first."
                        ))
                        .title(t!("Text search can't look inside archives"))
                        .id::<ArchiveToast>(),
                        cx,
                    );
                    return false;
                }
                if explorer.read(cx).closed_of(None) {
                    explorer.update(cx, |explorer, cx| explorer.closed_notice(None, cx));
                    return false;
                }
            }
        }
        let target = match folder {
            Some(folder) => Some(folder.to_string()),
            None => crate::explorer::try_explorer(cx).map(|e| e.read(cx).current_path().to_string()),
        };
        let current_path = self.folder.read(cx).value().to_string();
        let chosen = match target {
            Some(target) if !target.is_empty() && is_folder_path(&target) => Some(target),
            _ if current_path.trim().is_empty() => Some(default_folder(cx)).filter(|f| !f.is_empty()),
            _ => None,
        };
        if let Some(chosen) = chosen {
            self.set_folder(&chosen, window, cx);
        }
        self.focus_query(window, cx);
        true
    }

    /// The search model: the run on screen, its matches, whether it is still searching.
    pub fn search(&self) -> &Entity<Search> {
        &self.search
    }

    /// The query box has the keyboard, its text selected (Ctrl+F, Ctrl+K).
    pub fn focus_query(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.update(cx, |query, cx| {
            query.focus(window, cx);
            query.select_all(window, cx);
        });
    }

    /// Shows the list of recent searches, with its filter focused (the header's history button).
    pub fn show_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.history_open = true;
        self.history_filter.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    // ---- The fields ---------------------------------------------------------------------------------------

    /// The folder box and the saved folder, together.
    fn set_folder(&mut self, folder: &str, window: &mut Window, cx: &mut Context<Self>) {
        let shown = folder.to_string();
        self.folder.update(cx, |input, cx| input.set_value(shown, window, cx));
        let saved = folder.to_string();
        persistence::update_app(cx, |app| app.path = saved);
    }

    /// Keeps the query and the folder in the saved state, a moment after the last key.
    fn save_soon(&mut self, cx: &mut Context<Self>) {
        self.saving = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_AFTER).await;
            this.update(cx, |this, cx| this.save_now(cx)).ok();
        }));
    }

    /// Puts the query and the folder in the saved state now.
    fn save_now(&mut self, cx: &mut Context<Self>) {
        self.saving = None;
        let (query, folder) = (
            self.query.read(cx).value().to_string(),
            self.folder.read(cx).value().to_string(),
        );
        persistence::update_app(cx, |app| {
            app.query = query;
            app.path = folder;
        });
    }

    /// The switches as saved.
    fn options(cx: &App) -> SearchOptions {
        persistence::app(cx).map(|app| app.options).unwrap_or_default()
    }

    /// Turns one switch on or off, as the buttons and Alt+letter do (`setOption`).
    fn set_option(&mut self, key: SearchOptionKey, value: bool, window: &mut Window, cx: &mut Context<Self>) {
        persistence::update_app(cx, |app| app.set_option(key, value));
        self.sync_fields(window, cx);
        cx.notify();
    }

    /// The boxes follow the switches: what the query and the Replacement box say when empty, and that
    /// documents aren't replaced in. Done when the switches (or the language) change, never while drawing.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let o = Self::options(cx);
        let key = (o.code, o.regex, o.documents, o.meaning, crate::i18n::is_rtl());
        if self.fields_for == Some(key) {
            return;
        }
        self.fields_for = Some(key);
        self.query.update(cx, |query, cx| {
            query.set_placeholder(bar::query_placeholder(&o), window, cx)
        });
        self.replacement.update(cx, |input, cx| {
            input.set_placeholder(bar::replacement_placeholder(&o), window, cx);
            input.set_disabled(o.documents, cx);
        });
    }

    /// The other state of a switch (Alt+C…).
    fn toggle_option(&mut self, key: SearchOptionKey, window: &mut Window, cx: &mut Context<Self>) {
        let on = bar::is_on(&Self::options(cx), key);
        self.set_option(key, !on, window, cx);
    }

    // ---- Searching ----------------------------------------------------------------------------------------

    /// Search (Enter, the Search button): the query in the folder in the box, or where the person is
    /// (`searchCurrent`).
    pub fn search_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }
        let mut path = self.folder.read(cx).value().trim().to_string();
        if path.is_empty() {
            path = default_folder(cx);
            if !path.is_empty() {
                self.set_folder(&path.clone(), window, cx);
            }
        }
        if path.is_empty() {
            window.push_notification(Notification::info(t!("Choose a folder to search in")), cx);
            return;
        }
        self.save_now(cx);
        self.start_search(query, path, None, false, cx);
    }

    /// A search with the current switches and settings. `limit` is the settings' unless given.
    fn start_search(
        &mut self,
        query: String,
        path: String,
        limit: Option<Option<u64>>,
        everything: bool,
        cx: &mut Context<Self>,
    ) {
        let settings = state::settings(cx);
        let options = Self::options(cx);
        let exclusions = settings.exclusions.clone();
        let limit = limit.unwrap_or(settings.max_results);
        // A code search shows what each match becomes, when the Replace box says.
        let text = self.replacement.read(cx).value().to_string();
        let rewrite = (options.code && self.replace_open && !text.is_empty()).then_some(text);
        let spec = Spec {
            query,
            path,
            options,
            rewrite,
            limit,
            everything,
            exclusions,
        };
        self.search.update(cx, |search, cx| search.search(spec, cx));
    }

    /// The same search again, or (`everything`) skipping nothing (`onSearchAgain`).
    fn search_again(&mut self, everything: bool, cx: &mut Context<Self>) {
        let Some(run) = self.search.read(cx).run().cloned() else {
            return;
        };
        self.start_search(run.query, run.path, Some(run.limit), everything, cx);
    }

    /// The same search without the result limit (`onShowAll`).
    fn show_all(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.search.read(cx).run().cloned() else {
            return;
        };
        self.start_search(run.query, run.path, Some(None), run.everything, cx);
    }

    /// Stop (the button, Escape).
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| search.stop(cx));
    }

    /// Back to the start screen (the results' Close).
    fn close_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| search.close(cx));
        self.focus_query(window, cx);
    }

    /// A recent search is picked (`pickHistory`): its query and folder go in the boxes and it runs.
    pub fn pick_history(&mut self, item: &HistoryItem, window: &mut Window, cx: &mut Context<Self>) {
        let (query, path) = (item.query.clone(), item.path.clone());
        self.history_open = false;
        self.query
            .update(cx, |input, cx| input.set_value(query.clone(), window, cx));
        self.set_folder(&path, window, cx);
        let saved = query.clone();
        persistence::update_app(cx, |app| app.query = saved);
        self.start_search(query, path, None, false, cx);
    }

    /// Escape (`TextLeave`): closes the history; in the query it leaves the box; in the filter it clears it;
    /// anywhere else it stops the search that runs.
    fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_open {
            self.history_open = false;
            self.leave_the_box(window, cx);
            cx.notify();
            return;
        }
        if self.query.read(cx).focus_handle(cx).is_focused(window) {
            self.leave_the_box(window, cx);
        } else if self.filter.read(cx).focus_handle(cx).is_focused(window) {
            if !self.filter.read(cx).value().is_empty() {
                self.filter.update(cx, |input, cx| input.set_value("", window, cx));
            }
        } else if !self.in_a_field(window, cx) && self.search.read(cx).searching() {
            // The other boxes keep Escape for themselves, as they did.
            self.stop(cx);
        }
    }

    /// The keyboard leaves a box: to the results when there are some on screen, else to the view itself.
    fn leave_the_box(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.read(cx).run().is_some() {
            self.results_focus.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// Whether one of the boxes has the keyboard.
    fn in_a_field(&self, window: &Window, cx: &App) -> bool {
        self.replacement.read(cx).focus_handle(cx).is_focused(window)
            || self.folder.read(cx).focus_handle(cx).is_focused(window)
            || self.history_filter.read(cx).focus_handle(cx).is_focused(window)
            || self.filter.read(cx).focus_handle(cx).is_focused(window)
            || self.query.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Down in the query: from its last line, on to the results (the TypeScript sent the page an ArrowDown).
    fn query_down(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx);
        let value = query.value();
        let end = query.selected_range().end;
        let on_last_line = !value.get(end..).unwrap_or("").contains('\n');
        if !on_last_line {
            // Down moves the caret, as in any box.
            cx.propagate();
            return;
        }
        self.results_focus.focus(window, cx);
        // With nothing picked yet, the first match is.
        let has_picked = self
            .search
            .read(cx)
            .current(state::settings(cx).auto_open_preview)
            .is_some();
        if !has_picked {
            self.move_selection(1, cx);
        }
    }

    /// Whether Replace all can go: a finished search with matches in text files.
    fn can_replace(&self, cx: &App) -> bool {
        let search = self.search.read(cx);
        !search.searching()
            && !search.matches().is_empty()
            && !search.run().is_some_and(|r| r.options.documents || r.options.meaning)
    }

    // ---- The results' keys -----------------------------------------------------------------------------------

    /// Up and Down walk the visible matches (not the file rows).
    fn move_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let current = self.search.read(cx).current(state::settings(cx).auto_open_preview);
        let selected_row = self.grouping.row_of(current).map_or(-1, |row| row as isize);
        let mut i = selected_row + step;
        while i >= 0 && (i as usize) < self.grouping.rows.len() {
            if let Some(results::Row::Match { index }) = self.grouping.rows.get(i as usize) {
                let index = *index;
                self.search.update(cx, |search, cx| search.select(Some(index), cx));
                self.list.scroll_to_item(i as usize, ScrollStrategy::Nearest);
                return;
            }
            i += step;
        }
    }

    /// Enter in the results: the picked match's file opens.
    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.search.read(cx).current(state::settings(cx).auto_open_preview);
        let path = current.and_then(|i| self.search.read(cx).matches().get(i).map(|m| m.path.clone()));
        if let Some(path) = path.filter(|p| !p.is_empty()) {
            self.open_file(path, window, cx);
        }
    }

    // ---- Opening ------------------------------------------------------------------------------------------------

    /// Opens a file: in the editor from Settings, else with the file's app as File Explorer opens it (a document
    /// always so) (`openFile`).
    pub fn open_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let editor = state::settings(cx).editor_path.trim().to_string();
        let use_editor = !editor.is_empty() && !is_document(&path);
        let opening = self.source.open(&path, use_editor.then_some(editor.as_str()));
        cx.spawn_in(window, async move |_, cx| {
            if let Err(reason) = opening.await {
                cx.update(|window, cx| {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't open the file")), cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// "Show in Files" (`revealInApp`).
    fn reveal(&mut self, path: String, cx: &mut Context<Self>) {
        cx.emit(TextSearchEvent::Reveal { path });
    }

    /// The matches' paths and lines are copied (the menu's Copy path and Copy line).
    fn copy(&mut self, text: String, message: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(Notification::success(message), cx);
    }

    // ---- Replace all -----------------------------------------------------------------------------------------------

    /// Replace all (the button, Enter in the Replacement box): in every file of the results, after asking
    /// (unless Settings turned the question off). A code pattern is rewritten by ast-grep instead.
    pub fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (run, files, busy) = {
            let search = self.search.read(cx);
            (
                search.run().cloned(),
                search.files(),
                search.replacing() || search.searching(),
            )
        };
        // One at a time: a second Enter mid-way would replace in what the first just wrote. Documents are zips
        // and PDFs: replacing in their bytes as text would ruin them.
        let Some(run) = run else { return };
        if busy || run.options.documents || run.options.meaning || files.is_empty() {
            return;
        }
        let replacement = self.replacement.read(cx).value().to_string();
        if run.options.code {
            self.rewrite_all(run, replacement, files.len(), window, cx);
            return;
        }
        if let Some(problem) = model::pattern_problem(&run.query, &run.options) {
            window.push_notification(
                Notification::error(problem).title(t!("This pattern can't be used for replacing")),
                cx,
            );
            return;
        }
        let count = files.len();
        let what = if count == 1 { t!("file") } else { t!("files") };
        let message = t!(
            "Replace every match of “{query}” with “{replacement}” in {count} {what}?\n\nThis can't be undone.",
            query = run.query.clone(),
            replacement = replacement.clone(),
            count = count,
            what = what
        );
        if state::settings(cx).confirm_before_replace {
            let this = cx.weak_entity();
            crate::dialogs::confirm(
                window,
                cx,
                t!("Replace all"),
                message,
                t!("Replace all"),
                true,
                move |window, cx| {
                    let (run, files, replacement) = (run.clone(), files.clone(), replacement.clone());
                    this.update(cx, |this, cx| this.replace_in(run, files, replacement, window, cx))
                        .ok();
                },
            );
        } else {
            self.replace_in(run, files, replacement, window, cx);
        }
    }

    /// The replacing itself: one file at a time on core's runtime, with a notice while it goes.
    fn replace_in(
        &mut self,
        run: Run,
        files: Vec<String>,
        replacement: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.update(cx, |search, _| search.set_replacing(true));
        let count = files.len();
        let loading = t!("Replacing in {count} files…", count = count);
        window.push_notification(Notification::info(loading).id::<ReplacingToast>().autohide(false), cx);
        let source = self.source.clone();
        let background = cx.background_executor().clone();
        let run_id = run.id.clone();
        cx.spawn_in(window, async move |this, cx| {
            let done =
                model::replace_in_files(source, files, run.query.clone(), run.options, replacement, background).await;
            this.update_in(cx, |this, window, cx| {
                this.search.update(cx, |search, _| search.set_replacing(false));
                window.remove_notification::<ReplacingToast>(cx);
                let what = if done.changed == 1 { t!("file") } else { t!("files") };
                let summary = t!("Replaced in {changed} {what}", changed = done.changed, what = what);
                if done.failed > 0 {
                    window.push_notification(
                        Notification::error(t!(
                            "{summary}, {failed} failed",
                            summary = summary,
                            failed = done.failed
                        )),
                        cx,
                    );
                } else {
                    window.push_notification(Notification::success(summary), cx);
                }
                // What's left to replace, unless a newer search started meanwhile. NOTE(port): it searches
                // again without `everything`, as the TypeScript did.
                let current = this.search.read(cx).run().is_some_and(|r| r.id == run_id);
                if current {
                    this.start_search(run.query.clone(), run.path.clone(), Some(run.limit), false, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// A code search's rewrite: first shown on each match (searched again with it), then, asked again, made in
    /// the files (`rewriteAll`).
    fn rewrite_all(
        &mut self,
        run: Run,
        replacement: String,
        files: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if replacement.is_empty() {
            window.push_notification(Notification::info(t!("Type what each match becomes first")), cx);
            return;
        }
        if run.rewrite.as_deref() != Some(replacement.as_str()) {
            window.push_notification(
                Notification::info(t!("Replace all again to rewrite them in the files."))
                    .title(t!("Each match now shows what it becomes")),
                cx,
            );
            self.start_search(run.query, run.path, Some(run.limit), run.everything, cx);
            return;
        }
        let matches = self.search.read(cx).matches().len();
        let match_word = if matches == 1 { t!("match") } else { t!("matches") };
        let file_word = if files == 1 { t!("file") } else { t!("files") };
        let message = t!(
            "Rewrite {matches} {match_word} of “{query}” as “{replacement}” in {files} {file_word}?\n\nThis can't be undone.",
            matches = matches,
            match_word = match_word,
            query = run.query.clone(),
            replacement = replacement.clone(),
            files = files,
            file_word = file_word
        );
        if state::settings(cx).confirm_before_replace {
            let this = cx.weak_entity();
            crate::dialogs::confirm(
                window,
                cx,
                t!("Replace all"),
                message,
                t!("Rewrite"),
                true,
                move |window, cx| {
                    let (run, replacement) = (run.clone(), replacement.clone());
                    this.update(cx, |this, cx| this.rewrite_in(run, replacement, window, cx))
                        .ok();
                },
            );
        } else {
            self.rewrite_in(run, replacement, window, cx);
        }
    }

    /// ast-grep makes the rewrite in the files.
    fn rewrite_in(&mut self, run: Run, replacement: String, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |search, _| search.set_replacing(true));
        let settings = state::settings(cx);
        let request = Request {
            id: String::new(),
            query: run.query.clone(),
            path: run.path.clone(),
            options: run.options,
            rewrite: Some(replacement),
            exclusions: settings.exclusions.clone(),
            everything: run.everything,
            max_results: None,
        };
        let rewriting = self.source.rewrite(request);
        cx.spawn_in(window, async move |this, cx| {
            let answer = rewriting.await;
            this.update_in(cx, |this, window, cx| {
                this.search.update(cx, |search, _| search.set_replacing(false));
                match answer {
                    Ok(n) => {
                        let what = if n == 1 { t!("match") } else { t!("matches") };
                        window
                            .push_notification(Notification::success(t!("Rewrote {n} {what}", n = n, what = what)), cx);
                    }
                    Err(reason) => {
                        window.push_notification(Notification::error(reason).title(t!("Couldn't rewrite them")), cx)
                    }
                }
                let current = this.search.read(cx).run().is_some_and(|r| r.id == run.id);
                if current {
                    this.start_search(run.query.clone(), run.path.clone(), Some(run.limit), run.everything, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- Export ---------------------------------------------------------------------------------------------------

    /// Export results (CSV or JSON): what the filters show, to a file the person picks.
    fn export_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let visible: Vec<Match> = {
            let search = self.search.read(cx);
            self.grouping
                .rows
                .iter()
                .filter_map(|row| match row {
                    results::Row::Match { index } => search.matches().get(*index).cloned(),
                    results::Row::File { .. } => None,
                })
                .collect()
        };
        if visible.is_empty() {
            return;
        }
        let documents = self.search.read(cx).run().is_some_and(|r| r.options.documents);
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picking = self.source.pick_export("search-results.csv".to_string(), owner);
        let source = self.source.clone();
        cx.spawn_in(window, async move |_, cx| {
            let file = match picking.await {
                Ok(Some(file)) => file,
                Ok(None) => return,
                Err(reason) => {
                    cx.update(|window, cx| {
                        window
                            .push_notification(Notification::error(reason).title(t!("Couldn't export the results")), cx)
                    })
                    .ok();
                    return;
                }
            };
            let refs: Vec<&Match> = visible.iter().collect();
            let content = export_text(&refs, &file, documents);
            let written = source.write(&file, content).await;
            cx.update(|window, cx| match written {
                Ok(()) => window.push_notification(
                    Notification::success(file.clone()).title(t!(
                        "Exported {n} results",
                        n = glint_model::format::format_count(visible.len())
                    )),
                    cx,
                ),
                Err(reason) => {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't export the results")), cx)
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- The meaning model ----------------------------------------------------------------------------------------

    /// Downloads the meaning model (the card's "Download it").
    fn download_meaning(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // PORT ME(coordinator): `Tools::meaning_progress()`, a getter for the download's progress, so the card can
        // show "Downloading 2.1 MB of 30 MB…" as the TypeScript's did; today it says only "Downloading…".
        let Some(tools) = self.tools.clone() else { return };
        self.meaning_downloading = true;
        tools.update(cx, |tools, cx| tools.install_meaning(false, window, cx));
        cx.notify();
    }

    /// Stops that download (the card's Stop).
    fn stop_meaning(&mut self, cx: &mut Context<Self>) {
        if let Some(tools) = &self.tools {
            tools.read(cx).cancel("meaning");
        }
        self.meaning_downloading = false;
        cx.notify();
    }

    /// A new search starts, or the results are closed: the filters, collapsed files, scroll and preview start
    /// over (Results was keyed by the run's id, so the TypeScript mounted a fresh one).
    fn reset_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ext_filter.clear();
        self.collapsed.clear();
        self.collapse_version += 1;
        self.preview = preview::Preview::default();
        self.list = UniformListScrollHandle::new();
        self.filter.update(cx, |input, cx| input.set_value("", window, cx));
    }
}

/// The notice that archives can't be searched (the web app's toast id "archive-text").
struct ArchiveToast;
/// "Replacing in N files…".
struct ReplacingToast;

/// `defaultTextSearchFolder`: the folder set in Settings, else the open one, else Home, else the first drive.
pub fn default_folder(cx: &App) -> String {
    let configured = state::settings(cx).default_search_path.trim().to_string();
    if !configured.is_empty() {
        return configured;
    }
    let Some(explorer) = crate::explorer::try_explorer(cx) else {
        return String::new();
    };
    let explorer = explorer.read(cx);
    let current = explorer.current_path();
    if !current.is_empty() && is_folder_path(current) && explorer.archive_of(None).is_none() {
        return current.to_string();
    }
    let home = explorer.home_folder();
    if !home.is_empty() && home != THIS_PC {
        return home.to_string();
    }
    explorer.drives().first().map(|d| d.path.clone()).unwrap_or_default()
}

/// Documents Search reads, and pictures whose text OCR reads: their app opens them, not the text editor
/// (`isDocument`).
pub fn is_document(path: &str) -> bool {
    const DOCUMENTS: [&str; 18] = [
        "pdf", "docx", "docm", "dotx", "dotm", "pptx", "pptm", "ppsx", "potx", "xlsx", "xlsm", "xlsb", "xls", "ods",
        "odt", "ott", "odp", "epub",
    ];
    const PICTURES: [&str; 14] = [
        "png", "jpg", "jpeg", "jpe", "jfif", "bmp", "gif", "tif", "tiff", "webp", "heic", "heif", "avif", "jxr",
    ];
    // The TypeScript took what follows the last dot, whatever it was in front of.
    let ext = path.rsplit_once('.').map_or(path, |(_, ext)| ext).to_lowercase();
    DOCUMENTS.contains(&ext.as_str()) || PICTURES.contains(&ext.as_str())
}

impl Render for TextSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        self.regroup(cx);
        self.ensure_preview(cx);

        let has_run = self.search.read(cx).run().is_some();
        let body = if has_run {
            self.results_view(&t, window, cx).into_any_element()
        } else {
            self.home_view(&t, cx).into_any_element()
        };
        let cards = self.cards(&t, cx);
        let history = self.history_popover(&t, window, cx);

        col()
            .id("text-search-view")
            .role(Role::Group)
            .aria_label(t!("Text search"))
            .test_support()
            .key_context("TextSearch")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .min_h_0()
            .bg(t.background)
            .text_color(t.foreground)
            .text_size(theme::size::SM)
            .text_start()
            .on_action(cx.listener(|this, _: &actions::TextMatchCase, window, cx| {
                this.toggle_option(SearchOptionKey::CaseSensitive, window, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::TextWholeWord, window, cx| {
                this.toggle_option(SearchOptionKey::WholeWord, window, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::TextRegex, window, cx| {
                this.toggle_option(SearchOptionKey::Regex, window, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::TextCodePatterns, window, cx| {
                this.toggle_option(SearchOptionKey::Code, window, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::TextDocuments, window, cx| {
                this.toggle_option(SearchOptionKey::Documents, window, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::TextPictures, window, cx| {
                this.toggle_option(SearchOptionKey::Ocr, window, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::TextMeaning, window, cx| {
                this.toggle_option(SearchOptionKey::Meaning, window, cx)
            }))
            .on_action(cx.listener(|this, _: &actions::TextReplace, _, cx| {
                this.replace_open = !this.replace_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &actions::TextLeave, window, cx| this.leave(window, cx)))
            .on_action(cx.listener(|this, _: &FocusQuery, window, cx| this.focus_query(window, cx)))
            .on_action(cx.listener(|this, _: &QueryDown, window, cx| this.query_down(window, cx)))
            .on_action(cx.listener(|this, _: &ResultsNext, _, cx| this.move_selection(1, cx)))
            .on_action(cx.listener(|this, _: &ResultsPrevious, _, cx| this.move_selection(-1, cx)))
            .on_action(cx.listener(|this, _: &ResultsOpen, window, cx| this.open_selected(window, cx)))
            .child(self.bar(&t, window, cx))
            .children(cards)
            .child(body)
            .children(history)
    }
}
