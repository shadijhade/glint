//! The bars over a pane's list that say what it shows (wave 15, pane-bars): the search bar while a search's results
//! are shown, and the bar for a folder listed from instant search's index because Windows keeps it closed.
//! Ported from the two bars of `Explorer.tsx` (the `find &&` block and `ClosedBar`).
//!
//! | TypeScript                                            | Here                                                    |
//! |-------------------------------------------------------|---------------------------------------------------------|
//! | which bar a pane shows, and what is in it              | [`bar_of`] -> [`Bar`] (plain data, tested without a window) |
//! | `scopeName`, the words of the search bar               | [`SearchInfo::scope_name`], [`search_words`]            |
//! | `searchSaved`, `toggleSavedSearch`                     | [`SearchInfo::saved`], `persistence::AppState::toggle_saved_search` |
//! | `stopFind`, `clearFind`, `runFind(.., { unlimited })`   | [`Explorer::stop_find`], [`Explorer::clear_find_in`], [`Explorer::run_find_in`] |
//! | `ClosedBar`, `getAccess`                               | [`Bar::Closed`], [`Explorer::get_access`]               |
//!
//! # The two bars
//!
//! - **The search bar** (`find` is set): a spinner while the search runs, else the magnifier; "Results for “query”
//!   in <folder> and its subfolders" ("Close matches" when nothing matched as typed, with a tooltip saying so; "in
//!   all drives" for This PC); "Stopped at N" with **Show all** when the result limit was hit; then at the end
//!   **Save** (**Saved** once it is in the sidebar), **Stop** while it runs, and **Close**.
//! - **The closed bar** (`listing.fromIndex && !find`): "Windows keeps this folder closed to you: shown read-only
//!   from instant search." (“to itself” when Windows refused even administrators), with **Get access** where that
//!   can help (not for a folder on another computer, nor one Windows refused).
//!
//! # How the coordinator wires it
//!
//! One [`PaneBars`] per pane, drawn above the compare bar (the workspace already does). It draws nothing when its
//! pane has nothing to say, and emits no events: it acts on the explorer and the saved state itself.
//!
//! # Differences from the TypeScript
//!
//! - Stop's square isn't filled (`fill-current`): the embedded Lucide icon is an outline.
//! - "Stopped at" is drawn in amber 700/400 where the TypeScript used amber 800/400 (the palette has no 800).
//! - The spinner and the bars have accessible names ("Searching", "Search results", "Closed folder").

#[cfg(test)]
mod tests;

use crate::components::git::Tone;
use crate::explorer::{explorer, Explorer, Tab, TabId, Wider};
use crate::kit::prelude::*;
use crate::persistence::{self, Persistence};
use crate::state::UiState;
use crate::{i18n, t, theme};
use glint_model::format::format_count;
use glint_model::paths::{base_name, THIS_PC};
use glint_model::store::SavedSearch;
use gpui_kit::{HighlightStyle, StyledText};
use std::ops::Range;

// ---- What a bar says ------------------------------------------------------------------------------------------

/// What the search bar of a pane says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchInfo {
    /// The tab the search is in (the buttons act on it, whichever pane has the keyboard).
    pub tab: TabId,
    pub query: String,
    /// The folder searched (with its subfolders); This PC (empty) searches every drive.
    pub scope: String,
    /// The search is still running.
    pub running: bool,
    /// Nothing matched as typed: these are close matches.
    pub fuzzy: bool,
    /// The search stopped at the result limit.
    pub limited: bool,
    /// The search is in the sidebar already.
    pub saved: bool,
    /// Settings' result limit, for "Stopped at N" (0 when there is none).
    pub max_results: u64,
    /// The files' contents weren't searched (a whole drive is only when asked): the bar offers to.
    pub inside_offered: bool,
}

impl SearchInfo {
    /// What the folder is called in the bar: its name, or "all drives" for This PC (`scopeName`).
    pub fn scope_name(&self) -> String {
        if self.scope == THIS_PC {
            t!("all drives").to_string()
        } else {
            base_name(&self.scope).to_string()
        }
    }
}

/// What the closed bar says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClosedInfo {
    /// The tab Get access acts on.
    pub tab: TabId,
    /// Windows refused even administrators ("to itself").
    pub refused: bool,
    /// Get access can help: the folder isn't on another computer, and Windows didn't refuse.
    pub can_get_access: bool,
}

/// The bar over a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Bar {
    /// Nothing is drawn.
    Hidden,
    /// The search bar.
    Search(SearchInfo),
    /// The folder is listed from the index because Windows keeps it closed.
    Closed(ClosedInfo),
}

/// Which bar `tab` shows, given the saved searches and Settings' result limit. A search wins: the closed bar is
/// for the folder itself (`listing?.fromIndex && !find`).
pub fn bar_of_tab(tab: &Tab, saved: &[SavedSearch], max_results: Option<u64>) -> Bar {
    if let Some(find) = &tab.find {
        let folder = find.scope.to_lowercase();
        return Bar::Search(SearchInfo {
            tab: tab.id,
            query: find.query.clone(),
            scope: find.scope.clone(),
            // By name or inside the files: Stop ends both.
            running: find.running || find.inside.running,
            // "Close matches" is about names: not said when the only results are files found by their contents.
            fuzzy: find.fuzzy && find.entries.len() > find.inside.only.len(),
            limited: find.limited,
            saved: saved
                .iter()
                .any(|s| s.query == find.query && s.folder.to_lowercase() == folder),
            max_results: max_results.unwrap_or(0),
            inside_offered: find.inside.offered && !find.inside.running,
        });
    }
    if tab.listing.as_ref().is_some_and(|l| l.from_index) {
        // `canGet = !!info && !info.remote && !info.refused`
        let info = tab.no_access();
        return Bar::Closed(ClosedInfo {
            tab: tab.id,
            refused: info.is_some_and(|n| n.refused),
            can_get_access: info.is_some_and(|n| !n.remote && !n.refused),
        });
    }
    Bar::Hidden
}

/// Which bar `pane` shows now.
pub fn bar_of(explorer: &Explorer, pane: usize, saved: &[SavedSearch], max_results: Option<u64>) -> Bar {
    if pane >= explorer.shown_panes().len() {
        return Bar::Hidden;
    }
    match explorer.pane_tab(pane) {
        Some(tab) => bar_of_tab(tab, saved, max_results),
        None => Bar::Hidden,
    }
}

/// A character no search text contains, standing where the query goes while the sentence is worded.
const MARK: char = '\u{E000}';

/// The search bar's sentence, and the byte range of the query in it (the part drawn in bold). With `display` the
/// query and the folder's name are isolated so an Arabic name keeps its own direction inside the sentence; without
/// it (the accessible name) they are plain.
pub fn search_words(info: &SearchInfo, display: bool) -> (String, Range<usize>) {
    let quoted = format!("“{}”", info.query);
    let scope = info.scope_name();
    let scope = if display { isolate(scope).to_string() } else { scope };
    let all = info.scope == THIS_PC;
    let template = match (info.fuzzy, all) {
        (false, true) => t!("Results for {query} in {scope}", query = MARK, scope = scope),
        (false, false) => t!(
            "Results for {query} in {scope} and its subfolders",
            query = MARK,
            scope = scope
        ),
        (true, true) => t!("Close matches for {query} in {scope}", query = MARK, scope = scope),
        (true, false) => t!(
            "Close matches for {query} in {scope} and its subfolders",
            query = MARK,
            scope = scope
        ),
    };
    let quoted = if display { isolate(quoted).to_string() } else { quoted };
    let at = template.find(MARK).unwrap_or(0);
    let text = template.replacen(MARK, &quoted, 1);
    let end = (at + quoted.len()).min(text.len());
    (text, at..end)
}

// ---- The view -------------------------------------------------------------------------------------------------

/// The bars over one pane. Draws nothing unless the pane needs one ([`bar_of`]).
pub struct PaneBars {
    pane: usize,
    explorer: Entity<Explorer>,
    /// Get access has been asked and Windows hasn't answered yet.
    working: bool,
    /// A bar to draw whatever the explorer says (tests: the fake disk can't list a folder from the index).
    #[cfg(test)]
    forced: Option<Bar>,
    _subscriptions: Vec<Subscription>,
}

impl PaneBars {
    /// The bars over pane `pane`.
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        let subscriptions = vec![
            cx.observe(&explorer, |_, _, cx| cx.notify()),
            // "Stopped at N" follows Settings' limit.
            cx.observe_global::<UiState>(|_, cx| cx.notify()),
            // Save and Saved follow the sidebar's searches, wherever they change.
            cx.observe_global::<Persistence>(|_, cx| cx.notify()),
        ];
        PaneBars {
            pane,
            explorer,
            working: false,
            #[cfg(test)]
            forced: None,
            _subscriptions: subscriptions,
        }
    }

    /// The pane these bars are over.
    pub fn pane(&self) -> usize {
        self.pane
    }

    /// Which bar the pane shows now.
    pub fn bar(&self, cx: &App) -> Bar {
        #[cfg(test)]
        if let Some(bar) = &self.forced {
            return bar.clone();
        }
        let saved = persistence::app(cx)
            .map(|app| app.saved_searches.as_slice())
            .unwrap_or(&[]);
        let max_results = cx.try_global::<UiState>().and_then(|ui| ui.settings.find_max_results);
        bar_of(self.explorer.read(cx), self.pane, saved, max_results)
    }

    /// Get access is waiting for Windows.
    pub fn working(&self) -> bool {
        self.working
    }

    /// Stop: the search ends, keeping what it found. The pane takes the keyboard first, as a press inside a pane
    /// does, because the explorer stops the active tab's search.
    fn stop(&self, cx: &mut Context<Self>) {
        let pane = self.pane;
        self.explorer.update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            explorer.stop_find();
            let tab = explorer.active_tab_id();
            explorer.stop_inside(tab, cx);
        });
    }

    /// Inside files too: the same search again, in the same place, reading what the files hold as well (a whole
    /// drive isn't read unasked).
    fn search_inside(&self, info: &SearchInfo, cx: &mut Context<Self>) {
        let (tab, query, scope) = (info.tab, info.query.clone(), info.scope.clone());
        self.explorer.update(cx, |explorer, cx| {
            explorer
                .run_find_in(tab, &query, Wider::in_scope(scope).inside_too(), cx)
                .detach();
        });
    }

    /// Close: back to the folder.
    fn close(&self, tab: TabId, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| explorer.clear_find_in(tab, cx));
    }

    /// Show all: the same search again, in the same folder, without the result limit.
    fn show_all(&self, info: &SearchInfo, cx: &mut Context<Self>) {
        let (tab, query, scope) = (info.tab, info.query.clone(), info.scope.clone());
        self.explorer.update(cx, |explorer, cx| {
            explorer
                .run_find_in(tab, &query, Wider::in_scope(scope).without_limit(), cx)
                .detach();
        });
    }

    /// Save: keeps the search in the sidebar, or takes it out if it is there.
    fn save(&self, info: &SearchInfo, cx: &mut Context<Self>) {
        let search = SavedSearch {
            query: info.query.clone(),
            folder: info.scope.clone(),
        };
        persistence::update_app(cx, |app| app.toggle_saved_search(search));
    }

    /// Get access: Windows asks for administrator approval; the button shows a spinner meanwhile.
    fn get_access(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if self.working {
            return;
        }
        self.working = true;
        cx.notify();
        let asking = self
            .explorer
            .update(cx, |explorer, cx| explorer.get_access(Some(tab), cx));
        cx.spawn(async move |this, cx| {
            asking.await;
            this.update(cx, |this, cx| {
                this.working = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

// ---- Drawing --------------------------------------------------------------------------------------------------

/// The group at the end of the bar (`ms-auto`).
fn end_group() -> Div {
    let group = row_start().flex_none().gap(space(1.));
    if i18n::is_rtl() {
        group.mr_auto()
    } else {
        group.ml_auto()
    }
}

/// A button of the bar: `variant="ghost" size="sm"`.
fn ghost(id: &'static str, pane: usize) -> Button {
    Button::new((id, pane)).ghost().small()
}

impl PaneBars {
    /// The search bar.
    fn search_bar(&self, info: &SearchInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let pane = self.pane;
        let (plain, _) = search_words(info, false);

        let lead = if info.running {
            div()
                .id(("pane-search-spinner", pane))
                .role(Role::Group)
                .aria_label(t!("Searching"))
                .test_support()
                .flex_none()
                .child(Spinner::new().small().color(t.primary))
                .into_any_element()
        } else {
            icon(IconName::Search)
                .size_4()
                .flex_none()
                .text_color(t.primary)
                .into_any_element()
        };

        // “Results for” + the query in bold + the folder.
        let (text, bold) = search_words(info, true);
        let styled = StyledText::new(SharedString::from(text)).with_highlights(vec![(
            bold,
            HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
        )]);
        let mut words = div()
            .id(("pane-search-text", pane))
            .role(Role::Group)
            .aria_label(SharedString::from(plain.clone()))
            .test_support()
            .min_w_0()
            .truncate()
            .child(styled);
        if info.fuzzy {
            let tip = t!("Nothing is named as typed: these names are a letter or two off");
            words = words.tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx));
        }

        // “Stopped at N” with Show all, when the result limit was hit.
        let limit = info.limited.then(|| {
            let n = usize::try_from(info.max_results).unwrap_or(usize::MAX);
            let said = t!("Stopped at {n}", n = format_count(n));
            let ink = Tone::Amber.count_text(t.dark);
            let chosen = info.clone();
            row_start()
                .id(("pane-search-limit", pane))
                .role(Role::Group)
                .aria_label(said.clone())
                .test_support()
                .flex_none()
                .gap(space(1.5))
                .py(px(2.))
                .ps(2.)
                .pe(0.5)
                .rounded(theme::size::RADIUS)
                .bg(Tone::Amber.tint(0.10))
                .text_size(theme::size::XS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(ink)
                .child(said)
                .child(
                    Button::new(("pane-search-show-all", pane))
                        .custom(
                            ButtonCustomVariant::new(cx)
                                .color(hsla(0., 0., 0., 0.))
                                .foreground(ink)
                                .hover(Tone::Amber.tint(0.15))
                                .active(Tone::Amber.tint(0.2)),
                        )
                        .xsmall()
                        .label(t!("Show all"))
                        .tooltip(t!("Search again without the limit (Settings → Find files)"))
                        .on_click(cx.listener(move |this, _, _, cx| this.show_all(&chosen, cx))),
                )
        });

        // "Inside files too", where the files' contents weren't searched unasked.
        let inside = info.inside_offered.then(|| {
            let chosen = info.clone();
            ghost("pane-search-inside", pane)
                .icon(IconName::TextSearch)
                .label(t!("Inside files too"))
                .tooltip(t!(
                    "Read what the files here hold as well. A whole drive takes a while, so it waits to be asked"
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.search_inside(&chosen, cx)))
        });

        // Save (Saved once it is in the sidebar), Stop while it runs, Close.
        let chosen = info.clone();
        let save = ghost("pane-search-save", pane)
            .icon(if info.saved {
                icon(IconName::BookmarkCheck).text_color(t.primary)
            } else {
                icon(IconName::Bookmark)
            })
            .label(if info.saved { t!("Saved") } else { t!("Save") })
            .tooltip(if info.saved {
                t!("Take this search out of the sidebar")
            } else {
                t!("Keep this search in the sidebar, to run it again with one click")
            })
            .on_click(cx.listener(move |this, _, _, cx| this.save(&chosen, cx)));
        let stop = info.running.then(|| {
            ghost("pane-search-stop", pane)
                .icon(icon(IconName::Square).size_3())
                .label(t!("Stop"))
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        });
        let tab = info.tab;
        let close = ghost("pane-search-close", pane)
            .icon(IconName::X)
            .label(t!("Close"))
            .tooltip(t!("Back to the folder (Esc)"))
            .on_click(cx.listener(move |this, _, _, cx| this.close(tab, cx)));

        row_start()
            .id(("pane-search-bar", pane))
            .role(Role::Group)
            .aria_label(t!("Search results"))
            .test_support()
            .flex_none()
            .w_full()
            .h(space(9.))
            .gap(space(2.))
            .px(space(3.))
            .border_b_1()
            .border_color(t.border)
            .bg(t.primary.opacity(0.05))
            .text_size(theme::size::SM)
            .text_start()
            .child(lead)
            .child(words)
            .children(limit)
            .child(end_group().children(inside).child(save).children(stop).child(close))
            .into_any_element()
    }

    /// The bar over a folder listed from instant search's index.
    fn closed_bar(&self, info: &ClosedInfo, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let pane = self.pane;
        let ink = Tone::Amber.count_text(t.dark);
        let said = if info.refused {
            t!("Windows keeps this folder to itself: shown read-only from instant search.")
        } else {
            t!("Windows keeps this folder closed to you: shown read-only from instant search.")
        };
        let why = t!(
            "Instant search reads each drive's file table as the system, so it knows what's here; your account still may not open or change any of it."
        );
        let words = div()
            .id(("pane-closed-text", pane))
            .role(Role::Group)
            .aria_label(said.clone())
            .test_support()
            .min_w_0()
            .flex_1()
            .truncate()
            .tooltip(move |window, cx| crate::kit::tooltip(why.clone(), window, cx))
            .child(said);
        let tab = info.tab;
        let get = info.can_get_access.then(|| {
            Button::new(("pane-closed-get-access", pane))
                .ghost()
                .small()
                .icon(IconName::ShieldCheck)
                .label(t!("Get access"))
                .loading(self.working)
                .disabled(self.working)
                .tooltip(t!("Windows asks for administrator approval"))
                .on_click(cx.listener(move |this, _, _, cx| this.get_access(tab, cx)))
        });
        row_start()
            .id(("pane-closed-bar", pane))
            .role(Role::Group)
            .aria_label(t!("Closed folder"))
            .test_support()
            .flex_none()
            .w_full()
            .min_h(space(9.))
            .gap(space(2.))
            .px(space(3.))
            .py(space(1.))
            .border_b_1()
            .border_color(t.border)
            .bg(Tone::Amber.tint(0.10))
            .text_size(theme::size::SM)
            .text_start()
            .child(icon(IconName::FolderLock).size_4().flex_none().text_color(ink))
            .child(words)
            .children(get)
            .into_any_element()
    }
}

impl Render for PaneBars {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = crate::perf::span("pane bars");
        match self.bar(cx) {
            Bar::Hidden => div().into_any_element(),
            Bar::Search(info) => self.search_bar(&info, cx),
            Bar::Closed(info) => self.closed_bar(&info, cx),
        }
    }
}
