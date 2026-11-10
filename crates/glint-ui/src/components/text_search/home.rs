//! The start screen and the list of recent searches (History.tsx).
//!
//! | TypeScript                            | Here                                                              |
//! |---------------------------------------|-------------------------------------------------------------------|
//! | `shortPath`                           | [`short_path`]                                                     |
//! | `HistoryList`                         | `TextSearch::history_list`: a search per row, and a button that forgets it |
//! | `HistoryPopover`                      | `TextSearch::history_popover`: a panel over the view, with a filter, at most 100 rows and Clear history |
//! | `Home`, `SHORTCUTS`                   | `TextSearch::home_view`, [`SHORTCUTS`]                              |
//! | `clearWithUndo("history")`            | `TextSearch::clear_history`: a notice with Undo                      |
//!
//! The history is `persistence::app(cx).history` (`AppState::add_to_history` keeps it, newest first, at most
//! `maxHistoryItems`); it is kept between runs, as it was.
//!
//! The popover opens from the search bar's button here (it hung under the header button in the web app); it
//! closes on Escape, on a click outside, and when a search is picked.

use super::TextSearch;
use crate::kit::prelude::*;
use crate::{i18n, persistence, t, theme};
use glint_model::format::format_count;
use glint_model::store::HistoryItem;
use gpui_kit::deferred;

/// The keys the start screen lists, with what each does (`SHORTCUTS`).
pub const SHORTCUTS: [(&str, &str); 7] = [
    ("Enter", "Search, or open the result picked"),
    ("↓ ↑", "From the search box on through the results"),
    ("Shift+Enter", "A new line: search for several lines at once"),
    ("Alt+C / W / R", "Match case, whole words, regular expression"),
    ("Alt+A / D / O / M", "Code patterns, documents, pictures too, meaning"),
    ("Ctrl+H", "Replace"),
    ("Esc", "Leave the search box; again, stop the search"),
];

/// How many searches the popover draws (the filter reaches older ones).
const POPOVER_ROWS: usize = 100;
/// How many the start screen lists.
const HOME_ROWS: usize = 8;

/// "C:\a\b\c\d" becomes "…\c\d": the end of a path is the part worth seeing (`shortPath`).
pub fn short_path(path: &str) -> String {
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = path.split(sep).filter(|p| !p.is_empty()).collect();
    if parts.len() > 2 {
        format!("…{sep}{}", parts[parts.len() - 2..].join(&sep.to_string()))
    } else {
        path.to_string()
    }
}

/// "Cleared the search history" (the web app's toast).
struct ClearedToast;

impl TextSearch {
    /// The saved history, newest first.
    fn history(cx: &App) -> Vec<HistoryItem> {
        persistence::app(cx).map(|app| app.history.clone()).unwrap_or_default()
    }

    /// One row for each search: its query, where, and a button that forgets it (`HistoryList`).
    fn history_list(
        &self,
        items: Vec<HistoryItem>,
        slot: &'static str,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mono = theme::mono_font(cx);
        col()
            .id((slot, 0usize))
            .role(Role::List)
            .aria_label(t!("Recent searches"))
            .test_support()
            .gap(px(1.))
            .children(items.into_iter().enumerate().map(|(i, item)| {
                let group: SharedString = format!("{slot}-group-{i}").into();
                let (pick, forget) = (item.clone(), item.clone());
                let short = short_path(&item.path);
                let tip: SharedString = format!("{}\n{}", item.query, item.path).into();
                row_start()
                    .id((slot, i + 1))
                    .role(Role::ListItem)
                    .aria_label(item.query.clone())
                    .aria_description(item.path.clone())
                    .test_support()
                    .group(group.clone())
                    .items_center()
                    .rounded(theme::size::RADIUS)
                    .hover(|s| s.bg(t.accent))
                    .child(
                        row_start()
                            .id((slot, 1000 + i))
                            .role(Role::Button)
                            .aria_label(t!(
                                "Search for {query} in {path}",
                                query = item.query.clone(),
                                path = item.path.clone()
                            ))
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .gap(space(3.))
                            .px(space(2.5))
                            .py(space(1.5))
                            .cursor_pointer()
                            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| this.pick_history(&pick, window, cx)))
                            .child(
                                icon(IconName::ClockArrowLeft)
                                    .size_4()
                                    .flex_none()
                                    .text_color(t.muted_foreground),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_family(mono.clone())
                                    .text_size(theme::size::SM)
                                    .child(isolate(item.query.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .max_w(relative(0.5))
                                    .truncate()
                                    .ms(1.)
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(isolate(short)),
                            ),
                    )
                    .child(
                        Button::new((slot, 2000 + i))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .tooltip(t!("Remove from history"))
                            .accessibility_label(t!("Remove from history"))
                            .opacity(0.45)
                            .group_hover(group, |s| s.opacity(1.))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let (query, path) = (forget.query.clone(), forget.path.clone());
                                persistence::update_app(cx, |app| app.remove_from_history(&query, &path));
                                cx.notify();
                            })),
                    )
            }))
            .into_any_element()
    }

    /// The start screen: recent searches and a keyboard cheat sheet (`Home`).
    pub(super) fn home_view(&mut self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let history = Self::history(cx);
        let total = history.len();
        let recent = (total > 0).then(|| {
            let list = self.history_list(
                history.into_iter().take(HOME_ROWS).collect(),
                "text-home-history",
                t,
                cx,
            );
            col()
                .id("text-home-recent")
                .role(Role::Group)
                .aria_label(t!("Recent searches"))
                .test_support()
                .child(
                    row_start()
                        .justify_between()
                        .px(space(2.5))
                        .pb(space(1.5))
                        .child(
                            div()
                                .id("text-home-recent-heading")
                                .role(Role::Heading)
                                .aria_label(t!("Recent searches"))
                                .test_support()
                                .text_size(theme::size::SM)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t!("Recent searches")),
                        )
                        .children((total > HOME_ROWS).then(|| {
                            let label = t!("View all {n}", n = format_count(total));
                            div()
                                .id("text-home-view-all")
                                .role(Role::Button)
                                .aria_label(label.clone())
                                .test_support()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .cursor_pointer()
                                .hover(|s| s.text_color(t.foreground))
                                .on_click(cx.listener(|this, _, window, cx| this.show_history(window, cx)))
                                .child(label)
                        })),
                )
                .child(list)
        });
        let keys = col()
            .id("text-home-keys")
            .role(Role::DescriptionList)
            .aria_label(t!("Keyboard shortcuts"))
            .test_support()
            .mx_auto()
            .gap(space(2.))
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .children(SHORTCUTS.iter().enumerate().map(|(i, (keys, what))| {
                let what = i18n::translate(what);
                row_start()
                    .id(("text-home-key", i))
                    .role(Role::Group)
                    .aria_label(format!("{keys}: {what}"))
                    .test_support()
                    .gap(space(3.))
                    .child(div().w(space(32.)).flex_none().text_end().child(key_cap(keys, t, cx)))
                    .child(div().child(what))
            }));
        div()
            .id("text-home")
            .flex_1()
            .min_h_0()
            .flex()
            .justify_center()
            .px(space(6.))
            .py(space(12.))
            .overflow_y_scroll()
            .child(
                col()
                    .w_full()
                    .max_w(rems(42.))
                    .gap(space(10.))
                    .child(
                        col()
                            .items_center()
                            .gap(space(1.5))
                            .text_center()
                            .child(
                                div()
                                    .id("text-home-title")
                                    .role(Role::Heading)
                                    .aria_label(t!("Search across your files"))
                                    .test_support()
                                    .text_size(rems(1.25))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(t!("Search across your files")),
                            )
                            .child(
                                div()
                                    .text_size(theme::size::SM)
                                    .text_color(t.muted_foreground)
                                    .child(t!("Choose a folder, type what you're looking for and press Enter.")),
                            ),
                    )
                    .children(recent)
                    .child(keys),
            )
    }

    /// The list of recent searches over the view, with its filter (`HistoryPopover`); none while it is shut.
    pub(super) fn history_popover(
        &mut self,
        t: &theme::Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.history_open {
            return None;
        }
        let all = Self::history(cx);
        let filter = self.history_filter.read(cx).value().to_lowercase();
        let shown: Vec<HistoryItem> = all
            .iter()
            .filter(|h| format!("{}\n{}", h.query, h.path).to_lowercase().contains(&filter))
            .take(POPOVER_ROWS)
            .cloned()
            .collect();
        let total = all.len();
        let rtl = i18n::is_rtl();
        let list = if shown.is_empty() {
            let said = if total > 0 {
                t!("No matching searches")
            } else {
                t!("No searches yet")
            };
            div()
                .id("text-history-empty")
                .role(Role::Status)
                .aria_label(said.clone())
                .test_support()
                .py(space(6.))
                .text_center()
                .text_size(theme::size::SM)
                .text_color(t.muted_foreground)
                .child(said)
                .into_any_element()
        } else {
            self.history_list(shown, "text-history", t, cx)
        };
        let footer = (total > 0).then(|| {
            row_start()
                .justify_between()
                .mt(space(2.))
                .pt(space(2.))
                .border_t_1()
                .border_color(t.border)
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(
                    div()
                        .id("text-history-count")
                        .role(Role::Status)
                        .aria_label(count_text(total))
                        .test_support()
                        .ps(1.)
                        .child(count_text(total)),
                )
                .child(
                    Button::new("text-history-clear")
                        .ghost()
                        .small()
                        .label(t!("Clear history"))
                        .on_click(cx.listener(|this, _, window, cx| this.clear_history(window, cx))),
                )
        });
        let panel = col()
            .id("text-history")
            .role(Role::Group)
            .aria_label(t!("Recent searches"))
            .test_support()
            .occlude()
            .absolute()
            .top(space(12.))
            .when(rtl, |d| d.left(space(3.)))
            .when(!rtl, |d| d.right(space(3.)))
            .w(rems(30.))
            .max_w(window.viewport_size().width - space(6.).to_pixels(theme::rem_size(cx)))
            .p(space(2.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Input::new(&self.history_filter).aria_label(t!("Filter recent searches…")))
            .child(
                div()
                    .id("text-history-scroll")
                    .mt(space(2.))
                    .max_h(window.viewport_size().height * 0.6)
                    .overflow_y_scroll()
                    .child(list),
            )
            .children(footer);
        // A click anywhere else puts it away.
        Some(
            deferred(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.history_open = false;
                            cx.notify();
                        }),
                    )
                    .child(panel),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    /// Clear history: every search is forgotten, with Undo (`clearWithUndo`).
    fn clear_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let before = Self::history(cx);
        persistence::update_app(cx, |app| app.history.clear());
        window.push_notification(
            Notification::success(t!("Cleared the search history"))
                .id::<ClearedToast>()
                .action(move |_, _, _| {
                    let before = before.clone();
                    Button::new("text-history-undo")
                        .label(t!("Undo"))
                        .on_click(move |_, window, cx| {
                            let again = before.clone();
                            persistence::update_app(cx, |app| app.history = again);
                            window.remove_notification::<ClearedToast>(cx);
                        })
                }),
            cx,
        );
        cx.notify();
    }
}

/// "1,234 searches", or "1 search".
fn count_text(n: usize) -> SharedString {
    if n == 1 {
        t!("{n} search", n = format_count(n))
    } else {
        t!("{n} searches", n = format_count(n))
    }
}

/// A key as a cap: its name in a small box (`Kbd`). The names are descriptive ("Alt+C / W / R"), so this is
/// not the kit's `Kbd`, which draws one keystroke.
fn key_cap(keys: &str, t: &theme::Tokens, cx: &App) -> impl IntoElement {
    div()
        .px(space(1.5))
        .py(space(0.5))
        .rounded(theme::size::RADIUS_SM)
        .border_1()
        .border_color(t.border)
        .bg(t.muted)
        .text_color(t.foreground)
        .font_family(theme::mono_font(cx))
        .child(keys.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_a_long_path_is_the_part_shown() {
        assert_eq!(short_path("C:\\a\\b\\c\\d"), "…\\c\\d");
        assert_eq!(short_path("C:\\a\\b"), "…\\a\\b");
        assert_eq!(short_path("C:\\a"), "C:\\a");
        assert_eq!(short_path("/home/me/code/glint"), "…/code/glint");
        assert_eq!(short_path("D:\\مستندات\\تقارير\\2024"), "…\\تقارير\\2024");
    }
}
