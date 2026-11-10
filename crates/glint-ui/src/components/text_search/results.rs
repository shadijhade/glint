//! The results (Results.tsx): a header with the counts and the filters, the matches grouped by file with
//! the hits marked, a menu on each row, and what is shown instead when there are none.
//!
//! | TypeScript                              | Here                                                            |
//! |-----------------------------------------|-----------------------------------------------------------------|
//! | `Row`, the `useMemo` that groups        | [`Row`], [`Grouping`]: built as matches arrive, again when a filter changes |
//! | `segments(match)` and `<mark>`          | `match_text`: the line with its hits in the accent color         |
//! | `renderRow` (file header, match)        | `TextSearch::result_rows`                                        |
//! | `openMenu`                              | `TextSearch::row_menu`                                           |
//! | `exportResults`                         | `TextSearch::export_results` (mod.rs)                            |
//! | the header                              | `TextSearch::results_header`                                     |
//! | `Centered` bodies                       | `TextSearch::results_body`                                       |
//! | `VirtualList`                           | `uniform_list` (only the rows in view are made)                  |
//!
//! Not ported: the error text is not selectable (the web page let it be copied); the menu key and Shift+F10 don't
//! open a row's menu (the right button does). The extension chips are shown from 1024 px of window width, as
//! `lg:` did.

use super::model::Match;
use super::TextSearch;
use crate::kit::prelude::*;
use crate::{i18n, state, t, theme};
use glint_model::format::format_count;
use gpui_kit::{HighlightStyle, StyledText};
use std::collections::HashSet;
use std::ops::Range;

/// A row of the list: a file's header, or one of its matches (`Row`).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Row {
    File {
        path: String,
        /// How many matches the filters leave in it.
        count: usize,
        /// The first match of it that the filters leave (selected by a click on the header).
        first: usize,
    },
    Match {
        index: usize,
    },
}

/// What the rows were built for.
#[derive(Clone, Debug, PartialEq)]
struct Built {
    run: Option<String>,
    filter: String,
    exts: Vec<String>,
    collapsed: u64,
}

/// The matches grouped by file under the filters (`useMemo` of Results). Matches arrive in the order ripgrep
/// found them, each file's together, so a batch only extends the rows; a change of filter builds them again.
#[derive(Default)]
pub(super) struct Grouping {
    pub rows: Vec<Row>,
    /// How many files the matches are in (before any filter).
    pub file_count: usize,
    /// The files the rows have headers for.
    pub file_paths: Vec<String>,
    /// The most matches any file has under the filters.
    pub max_count: usize,
    /// Matches by extension, before any filter, in the order the extensions first came.
    ext_counts: Vec<(String, usize)>,
    /// How many matches the rows are built from.
    upto: usize,
    built_for: Option<Built>,
    last_path: Option<String>,
    last_ext: String,
    header: Option<usize>,
}

/// A file's extension in lower case, without the dot; none for a name that starts with its only dot (`extOf`).
pub(super) fn ext_of(path: &str) -> String {
    let name = basename(path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.get(dot + 1..).unwrap_or("").to_lowercase(),
        _ => String::new(),
    }
}

/// The name after the last separator, which may be empty (`basename`).
pub(super) fn basename(path: &str) -> &str {
    path.rfind(['/', '\\'])
        .map_or(path, |cut| path.get(cut + 1..).unwrap_or(""))
}

impl Grouping {
    fn reset(&mut self) {
        *self = Grouping::default();
    }

    /// Takes in the matches after the ones already seen.
    fn extend(&mut self, matches: &[Match], filter: &str, exts: &[String], collapsed: &HashSet<String>) {
        for (index, m) in matches.iter().enumerate().skip(self.upto) {
            let path = m.path.as_str();
            if self.last_path.as_deref() != Some(path) {
                self.last_path = Some(path.to_string());
                self.last_ext = ext_of(path);
                self.file_count += 1;
            }
            match self.ext_counts.iter_mut().find(|(ext, _)| *ext == self.last_ext) {
                Some((_, n)) => *n += 1,
                None => self.ext_counts.push((self.last_ext.clone(), 1)),
            }
            if !exts.is_empty() && !exts.contains(&self.last_ext) {
                continue;
            }
            if !filter.is_empty()
                && !m.text.as_deref().unwrap_or("").to_lowercase().contains(filter)
                && !path.to_lowercase().contains(filter)
            {
                continue;
            }
            let new_file = match self.header.and_then(|h| self.rows.get(h)) {
                Some(Row::File { path: shown, .. }) => shown != path,
                _ => true,
            };
            if new_file {
                self.rows.push(Row::File {
                    path: path.to_string(),
                    count: 0,
                    first: index,
                });
                self.header = Some(self.rows.len() - 1);
                self.file_paths.push(path.to_string());
            }
            if let Some(Row::File { count, .. }) = self.header.and_then(|h| self.rows.get_mut(h)) {
                *count += 1;
                self.max_count = self.max_count.max(*count);
            }
            if !collapsed.contains(path) {
                self.rows.push(Row::Match { index });
            }
        }
        self.upto = matches.len();
    }

    /// The four most common extensions, with their counts (`exts`).
    pub fn top_exts(&self) -> Vec<(String, usize)> {
        let mut all = self.ext_counts.clone();
        // A stable sort, as JavaScript's: ties keep the order the extensions came in.
        all.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        all.truncate(4);
        all
    }

    /// The row of a match, if the filters show it.
    pub fn row_of(&self, index: Option<usize>) -> Option<usize> {
        let index = index?;
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Match { index: i } if *i == index))
    }
}

/// The height of a row, 1.75 rem (`rowHeight`: 28 px at the usual size).
fn rows_height() -> Rems {
    rems(1.75)
}

/// `lg:` in Tailwind: where the extension chips appear.
const LG: f32 = 1024.;
/// `md:`: where the preview appears.
const MD: f32 = 768.;

/// "12 ms", or "1.50 s" from a second on (`Math.round`, `toFixed(2)`).
pub(super) fn duration_text(ms: f64) -> String {
    if ms < 1000. {
        format!("{} ms", (ms + 0.5).floor())
    } else {
        format!("{:.2} s", ms / 1000.)
    }
}

impl TextSearch {
    /// Builds the rows for what the search holds now and the filters.
    pub(super) fn regroup(&mut self, cx: &App) {
        let search = self.search.read(cx);
        let filter = self.filter.read(cx).value().to_lowercase();
        let key = Built {
            run: search.run().map(|r| r.id.clone()),
            filter: filter.clone(),
            exts: self.ext_filter.clone(),
            collapsed: self.collapse_version,
        };
        if self.grouping.built_for.as_ref() != Some(&key) || search.matches().len() < self.grouping.upto {
            self.grouping.reset();
            self.grouping.built_for = Some(key);
        }
        self.grouping
            .extend(search.matches(), &filter, &self.ext_filter, &self.collapsed);
    }

    /// Whether every file with rows is collapsed (`allCollapsed`).
    fn all_collapsed(&self) -> bool {
        !self.grouping.file_paths.is_empty() && self.collapsed.len() >= self.grouping.file_paths.len()
    }

    fn toggle_file(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
        self.collapse_version += 1;
        cx.notify();
    }

    fn toggle_collapse_all(&mut self, cx: &mut Context<Self>) {
        if self.all_collapsed() {
            self.collapsed.clear();
        } else {
            self.collapsed = self.grouping.file_paths.iter().cloned().collect();
        }
        self.collapse_version += 1;
        cx.notify();
    }

    fn toggle_ext(&mut self, ext: &str, cx: &mut Context<Self>) {
        match self.ext_filter.iter().position(|e| e == ext) {
            Some(at) => {
                self.ext_filter.remove(at);
            }
            None => self.ext_filter.push(ext.to_string()),
        }
        cx.notify();
    }

    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ext_filter.clear();
        self.filter.update(cx, |input, cx| input.set_value("", window, cx));
        cx.notify();
    }

    /// A click on a match picks it and gives the results the keyboard.
    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.results_focus.focus(window, cx);
        self.search.update(cx, |search, cx| search.select(Some(index), cx));
    }

    // ---- The view ---------------------------------------------------------------------------------------

    /// The header, and under it the list and the preview.
    pub(super) fn results_view(
        &mut self,
        t: &theme::Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let wide = f32::from(window.viewport_size().width) >= MD;
        let has_matches = !self.search.read(cx).matches().is_empty();
        let header = self.results_header(t, window, cx);
        let body = self.results_body(t, cx);
        let preview = (has_matches && wide).then(|| self.preview_pane(t, cx));
        let rtl = i18n::is_rtl();
        col()
            .id("text-results")
            .role(Role::Group)
            .aria_label(t!("Results"))
            .test_support()
            .key_context("TextResults")
            .track_focus(&self.results_focus)
            .flex_1()
            .min_h_0()
            .child(header)
            .child(
                row_start()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(div().flex_1().min_w_0().min_h_0().child(body))
                    .children(preview.map(|preview| {
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .when(rtl, |d| d.border_r_1())
                            .when(!rtl, |d| d.border_l_1())
                            .border_color(t.border)
                            .child(preview)
                    })),
            )
    }

    /// The counts, what was skipped or cut short, the filters and the buttons (`h-10` bar).
    fn results_header(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (summary, searching, unreadable, limit, limited) = {
            let search = self.search.read(cx);
            let run = search.run();
            (
                summary_parts(
                    search.matches().len(),
                    self.grouping.file_count,
                    search.searching(),
                    search.duration(),
                    run.is_some_and(|r| r.options.documents),
                    search.files_searched(),
                ),
                search.searching(),
                search.unreadable(),
                run.and_then(|r| r.limit),
                search.limited(),
            )
        };
        let aria = summary.aria();
        let wide = f32::from(window.viewport_size().width) >= LG;
        let exts = self.grouping.top_exts();
        let any_rows = !self.grouping.rows.is_empty();
        let all_collapsed = self.all_collapsed();
        let files = self.grouping.file_paths.len();

        let counts = row_start()
            .id("text-results-summary")
            .role(Role::Status)
            .aria_label(aria)
            .test_support()
            .min_w_0()
            .gap(space(1.))
            .text_color(t.muted_foreground)
            .truncate()
            .children(summary.pieces.into_iter().map(|(text, strong)| {
                div()
                    .flex_none()
                    .when(strong, |d| d.font_weight(FontWeight::SEMIBOLD).text_color(t.foreground))
                    .child(text)
            }));

        let badge = |id: &'static str, text: SharedString, tip: SharedString| {
            div()
                .id(id)
                .test_support()
                .flex_none()
                .rounded(theme::size::RADIUS)
                .bg(t.warning.opacity(0.1))
                .px(space(2.))
                .py(space(0.5))
                .text_size(theme::size::XS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.warning)
                .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                .child(text)
        };
        let unreadable_badge = (unreadable > 0).then(|| {
            badge(
                "text-results-unreadable",
                t!(
                    "{n} couldn't be read",
                    n = format_count(usize::try_from(unreadable).unwrap_or(usize::MAX))
                ),
                t!("Damaged, locked with a password, over 256 MB, or still being read after 30 seconds"),
            )
        });
        let limited_button = limited.then(|| {
            let text = t!(
                "Stopped at {n} · Show all",
                n = format_count(usize::try_from(limit.unwrap_or(0)).unwrap_or(usize::MAX))
            );
            badge(
                "text-results-limited",
                text.clone(),
                t!("Run this search again without the result limit"),
            )
            .role(Role::Button)
            .aria_label(text)
            .cursor_pointer()
            .hover(|s| s.bg(t.warning.opacity(0.2)))
            .on_click(cx.listener(|this, _, _, cx| this.show_all(cx)))
        });

        let chips = (wide && exts.len() > 1).then(|| {
            row_start()
                .gap(space(1.5))
                .children(exts.into_iter().enumerate().map(|(i, (ext, count))| {
                    let active = self.ext_filter.contains(&ext);
                    let name: SharedString = if ext.is_empty() {
                        t!("no ext")
                    } else {
                        format!(".{ext}").into()
                    };
                    let toggled = ext.clone();
                    row_start()
                        .id(("text-ext-chip", i))
                        .role(Role::Button)
                        .aria_label(format!("{name} {}", format_count(count)))
                        .aria_toggled(if active { Toggled::True } else { Toggled::False })
                        .test_support()
                        .h(space(6.))
                        .gap(space(1.))
                        .px(space(2.))
                        .rounded_full()
                        .border_1()
                        .text_size(theme::size::XS)
                        .font_family(theme::mono_font(cx))
                        .cursor_pointer()
                        .when(active, |chip| {
                            chip.border_color(t.primary.opacity(0.4))
                                .bg(t.primary.opacity(0.15))
                                .text_color(t.primary)
                        })
                        .when(!active, |chip| {
                            chip.border_color(t.border)
                                .text_color(t.muted_foreground)
                                .hover(|s| s.bg(t.accent).text_color(t.foreground))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_ext(&toggled, cx)))
                        .child(name)
                        .child(div().opacity(0.6).child(format_count(count)))
                }))
        });

        let filter_box = div().relative().w(space(44.)).flex_none().child(
            Input::new(&self.filter)
                .small()
                .aria_label(t!("Filter results"))
                .cleanable(true)
                .prefix(icon(IconName::Search).size_3p5().text_color(t.muted_foreground)),
        );

        let collapse = (files > 1).then(|| {
            let label = if all_collapsed {
                t!("Expand all files")
            } else {
                t!("Collapse all files")
            };
            Button::new("text-collapse-all")
                .ghost()
                .small()
                .icon(if all_collapsed {
                    IconName::ChevronsUpDown
                } else {
                    IconName::ChevronsDownUp
                })
                .tooltip(label.clone())
                .accessibility_label(label)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_collapse_all(cx)))
        });

        row_start()
            .id("text-results-header")
            .test_support()
            .flex_none()
            .h(space(10.))
            .gap(space(3.))
            .px(space(3.))
            .border_b_1()
            .border_color(t.border)
            .bg(theme::chrome_fill(cx))
            .text_size(theme::size::SM)
            .children(searching.then(|| Spinner::new().small().color(t.primary)))
            .child(counts)
            .children(unreadable_badge)
            .children(limited_button)
            .child(
                row_start()
                    .flex_none()
                    .gap(space(1.5))
                    .ms(0.)
                    .children(chips)
                    .child(filter_box)
                    .children(collapse)
                    .child(
                        Button::new("text-export")
                            .ghost()
                            .small()
                            .icon(IconName::Download)
                            .disabled(!any_rows)
                            .tooltip(t!("Export results (CSV or JSON)"))
                            .accessibility_label(t!("Export results (CSV or JSON)"))
                            .on_click(cx.listener(|this, _, window, cx| this.export_results(window, cx))),
                    )
                    .child(
                        Button::new("text-close")
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .label(t!("Close"))
                            .tooltip(t!("Close the results (back to recent searches)"))
                            .on_click(cx.listener(|this, _, window, cx| this.close_results(window, cx))),
                    ),
            )
    }

    /// The list, or what to say instead: the search failed, is running, found nothing, or the filters hide it all.
    fn results_body(&mut self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let (error, searching, any, run) = {
            let search = self.search.read(cx);
            (
                search.error().to_string(),
                search.searching(),
                !search.matches().is_empty(),
                search.run().cloned(),
            )
        };
        let Some(run) = run else {
            return div().into_any_element();
        };
        let centered = |id: &'static str, label: SharedString| {
            col()
                .id(id)
                .role(Role::Group)
                .aria_label(label)
                .test_support()
                .size_full()
                .items_center()
                .justify_center()
                .gap(space(2.))
                .p(space(6.))
                .text_center()
                .text_size(theme::size::SM)
                .text_color(t.muted_foreground)
        };
        if !error.is_empty() {
            return centered("text-results-error", t!("The search failed"))
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.foreground)
                        .child(t!("The search failed")),
                )
                .child(
                    div()
                        .id("text-results-error-text")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .max_w_full()
                        .text_start()
                        .text_size(theme::size::XS)
                        .font_family(theme::mono_font(cx))
                        .child(error),
                )
                .child(
                    Button::new("text-try-again")
                        .outline()
                        .small()
                        .icon(IconName::RotateCw)
                        .label(t!("Try again"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let everything = this.search.read(cx).run().is_some_and(|r| r.everything);
                            this.search_again(everything, cx)
                        })),
                )
                .into_any_element();
        }
        if !any {
            if searching {
                return centered("text-results-waiting", t!("Searching…"))
                    .child(Spinner::new().color(t.primary))
                    .child(t!("Searching…"))
                    .into_any_element();
            }
            let (searched, unreadable) = {
                let search = self.search.read(cx);
                (search.files_searched(), search.unreadable())
            };
            let exclusions = state::settings(cx).exclusions.clone();
            let mut why = String::new();
            if run.options.documents {
                if searched == Some(0) {
                    why.push_str(&if run.options.meaning {
                        t!("No documents or notes in “{path}”. ", path = run.path.clone())
                    } else {
                        t!(
                            "No PDF, Word, Excel, PowerPoint, OpenDocument or EPUB files in “{path}”. ",
                            path = run.path.clone()
                        )
                    });
                } else if run.options.meaning {
                    why.push_str(&t!(
                        "Nothing there is close enough in meaning: try saying it another way. "
                    ));
                }
                if unreadable > 0 {
                    why.push_str(&t!(
                        "{n} couldn't be read. ",
                        n = format_count(usize::try_from(unreadable).unwrap_or(usize::MAX))
                    ));
                }
                why.push_str(&if run.everything {
                    t!("Only files kept in the cloud were skipped.")
                } else {
                    t!("Skipped: hidden files and folders, files kept in the cloud, and the folders excluded in Settings.")
                });
            } else {
                if searched == Some(0) {
                    why.push_str(&t!(
                        "Nothing was searched: check that “{path}” exists and isn't skipped. ",
                        path = run.path.clone()
                    ));
                }
                why.push_str(&if run.everything {
                    t!("Nothing was skipped.")
                } else {
                    let mut skipped = exclusions;
                    skipped.push(t!("hidden files").to_string());
                    skipped.push(t!("what .gitignore files list").to_string());
                    t!("Skipped: {list}.", list = skipped.join(", "))
                });
            }
            return centered("text-results-none", t!("No results"))
                .child(icon(IconName::SearchX).size_6())
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.foreground)
                        .child(t!("No results")),
                )
                .child(
                    div()
                        .id("text-results-none-why")
                        .role(Role::Status)
                        .aria_label(why.clone())
                        .test_support()
                        .max_w(space(96.))
                        .child(why),
                )
                .children((!run.everything).then(|| {
                    Button::new("text-search-everything")
                        .outline()
                        .small()
                        .icon(IconName::FolderSearch)
                        .label(t!("Search again, skipping nothing"))
                        .on_click(cx.listener(|this, _, _, cx| this.search_again(true, cx)))
                }))
                .into_any_element();
        }
        if self.grouping.rows.is_empty() {
            return centered("text-results-filtered", t!("No results match the filter."))
                .child(t!("No results match the filter."))
                .child(
                    Button::new("text-clear-filters")
                        .outline()
                        .small()
                        .label(t!("Clear filters"))
                        .on_click(cx.listener(|this, _, window, cx| this.clear_filters(window, cx))),
                )
                .into_any_element();
        }
        div()
            .id("text-results-list")
            .role(Role::ListBox)
            .aria_label(t!("Matches"))
            .test_support()
            .relative()
            .size_full()
            .child(
                uniform_list(
                    "text-results-rows",
                    self.grouping.rows.len(),
                    cx.processor(|this, range: Range<usize>, window, cx| this.result_rows(range, window, cx)),
                )
                .track_scroll(&self.list)
                .size_full(),
            )
            .vertical_scrollbar(&self.list)
            .into_any_element()
    }

    /// The rows in `range`.
    fn result_rows(&mut self, range: Range<usize>, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme::tokens(cx).clone();
        let current = self.search.read(cx).current(state::settings(cx).auto_open_preview);
        let selected_row = self.grouping.row_of(current);
        let base = self.search.read(cx).run().map(|r| r.path.clone()).unwrap_or_default();
        let rows: Vec<(usize, Row)> = range
            .filter_map(|i| Some((i, self.grouping.rows.get(i)?.clone())))
            .collect();
        rows.into_iter()
            .map(|(i, row)| match row {
                Row::File { path, count, first } => self.file_row(i, &path, count, first, &base, &t, cx),
                Row::Match { index } => self.match_row(i, index, selected_row == Some(i), &t, cx),
            })
            .collect()
    }

    /// A file's header: collapse arrow, name, the folder it is in, and how many matches it has, as a bar.
    #[allow(clippy::too_many_arguments)]
    fn file_row(
        &self,
        i: usize,
        path: &str,
        count: usize,
        first: usize,
        base: &str,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.collapsed.contains(path);
        let name = basename(path).to_string();
        let relative = relative_to(path, base);
        let folder = relative
            .get(..relative.len().saturating_sub(name.len()))
            .unwrap_or("")
            .to_string();
        let percent = if self.grouping.max_count > 0 {
            ((count as f32 / self.grouping.max_count as f32) * 100.).round().max(8.)
        } else {
            0.
        };
        let rtl = i18n::is_rtl();
        let tip: SharedString = t!(
            "{path}\nClick to {action} matches · Double-click to open",
            path = path.to_string(),
            action = if collapsed { t!("expand") } else { t!("collapse") }
        );
        let (click_path, menu_path, menu_this) = (path.to_string(), path.to_string(), cx.weak_entity());
        row_start()
            .id(("text-file", i))
            .role(Role::ListBoxOption)
            .aria_label(name.clone())
            .aria_description(t!(
                "{path}: {n} matches",
                path = path.to_string(),
                n = format_count(count)
            ))
            .aria_expanded(!collapsed)
            .test_support()
            .h(rows_height())
            .w_full()
            .gap(space(1.5))
            .ps(2.)
            .pe(3.)
            .bg(t.muted.opacity(0.5))
            .cursor_default()
            .hover(|s| s.bg(t.accent))
            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.toggle_file(&click_path, cx);
                this.pick(first, window, cx);
                if event.click_count() >= 2 {
                    this.open_file(click_path.clone(), window, cx);
                }
            }))
            .context_menu(move |menu, _, _| Self::row_menu(menu, menu_this.clone(), menu_path.clone(), None))
            .child(
                icon(if collapsed {
                    if rtl {
                        IconName::ChevronLeft
                    } else {
                        IconName::ChevronRight
                    }
                } else {
                    IconName::ChevronDown
                })
                .size_3p5()
                .flex_none()
                .text_color(t.muted_foreground),
            )
            .child(
                icon(IconName::FileText)
                    .size_4()
                    .flex_none()
                    .text_color(t.muted_foreground),
            )
            .child(
                div().flex_1().min_w_0().truncate().text_size(theme::size::SM).child(
                    row_start()
                        .gap(space(2.))
                        .child(div().font_weight(FontWeight::MEDIUM).child(isolate(name)))
                        .child(
                            div()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(isolate(folder)),
                        ),
                ),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .h(space(5.))
                    .min_w(space(9.))
                    .px(space(2.))
                    .overflow_hidden()
                    .rounded_full()
                    .bg(t.background)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .when(percent > 0., |pill| {
                        pill.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .when(rtl, |d| d.left_0())
                                .when(!rtl, |d| d.right_0())
                                .w(gpui_kit::relative(percent / 100.))
                                .bg(t.primary.opacity(if t.dark { 0.2 } else { 0.15 })),
                        )
                    })
                    .child(div().relative().child(format_count(count))),
            )
            .into_any_element()
    }

    /// A match: its line number (or where it is in a document), the line with its hits marked, and for a
    /// code rewrite what it becomes.
    fn match_row(
        &self,
        i: usize,
        index: usize,
        selected: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(m) = self.search.read(cx).matches().get(index).cloned() else {
            return div().into_any_element();
        };
        let mono = theme::mono_font(cx);
        let prose = m.label.is_some();
        let line = m.line_number.map(|n| n.to_string()).unwrap_or_default();
        let label = m.label.clone();
        let text = m.text.as_deref().unwrap_or("").trim().to_string();
        let aria_where = label.clone().unwrap_or_else(|| line.clone());
        let (path, menu_path, menu_line) = (
            m.path.clone(),
            m.path.clone(),
            m.text.as_ref().map(|l| l.trim().to_string()),
        );
        let menu_this = cx.weak_entity();
        let tip_label = label.clone();
        row_start()
            .id(("text-match", i))
            .role(Role::ListBoxOption)
            .aria_label(text.clone())
            .aria_description(format!("{}: {aria_where}", m.path))
            .aria_selected(selected)
            .test_support()
            .relative()
            .h(rows_height())
            .w_full()
            .gap(space(2.))
            .ps(4.)
            .pe(3.)
            .cursor_default()
            .when(!selected, |row| row.hover(|s| s.bg(t.accent.opacity(0.6))))
            .when(selected, |row| {
                row.bg(t.primary.opacity(0.1))
                    .hover(|s| s.bg(t.primary.opacity(0.15)))
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .when(i18n::is_rtl(), |d| d.right_0())
                            .when(!i18n::is_rtl(), |d| d.left_0())
                            .w(px(2.))
                            .bg(t.primary),
                    )
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| this.pick(index, window, cx)),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.pick(index, window, cx);
                if event.click_count() >= 2 {
                    this.open_file(path.clone(), window, cx);
                }
            }))
            .context_menu(move |menu, _, _| {
                Self::row_menu(menu, menu_this.clone(), menu_path.clone(), menu_line.clone())
            })
            .child(match label {
                Some(label) => div()
                    .id(("text-match-label", i))
                    .w(space(24.))
                    .flex_none()
                    .truncate()
                    .text_end()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground.opacity(0.7))
                    .tooltip(move |window, cx| {
                        crate::kit::tooltip(tip_label.clone().unwrap_or_default().into(), window, cx)
                    })
                    .child(label)
                    .into_any_element(),
                None => div()
                    .w(space(12.))
                    .flex_none()
                    .text_end()
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground.opacity(0.7))
                    .child(line)
                    .into_any_element(),
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(rems(0.8125))
                    // A document's text is prose: in the sans font.
                    .when(!prose, |d| d.font_family(mono.clone()))
                    .child(match_text(&m, t)),
            )
            .children(m.replacement.as_ref().map(|replacement| {
                let flat = flatten(replacement);
                div()
                    .id(("text-match-becomes", i))
                    .flex_none()
                    .max_w(relative(0.45))
                    .truncate()
                    .font_family(mono)
                    .text_size(rems(0.8125))
                    .text_color(t.success)
                    .tooltip({
                        let tip = t!("Becomes: {text}", text = replacement.clone());
                        move |window, cx| crate::kit::tooltip(tip.clone(), window, cx)
                    })
                    .child(format!("→ {flat}"))
            }))
            .into_any_element()
    }

    /// The right-click menu Files has too (`openMenu`). `line` is the match's line, when the row is a match.
    fn row_menu(menu: PopupMenu, this: gpui_kit::WeakEntity<Self>, path: String, line: Option<String>) -> PopupMenu {
        let (open, reveal, explorer, copy_path, copy_line, xray, locks) = (
            this.clone(),
            this.clone(),
            this.clone(),
            this.clone(),
            this.clone(),
            path.clone(),
            (this.clone(), path.clone()),
        );
        let (p_open, p_reveal, p_explorer, p_copy) = (path.clone(), path.clone(), path.clone(), path.clone());
        let has_line = line.is_some();
        menu.item(
            PopupMenuItem::new(t!("Open file"))
                .icon(IconName::ExternalLink)
                .on_click(move |_, window, cx| {
                    let path = p_open.clone();
                    open.update(cx, |this, cx| this.open_file(path, window, cx)).ok();
                }),
        )
        .item(
            PopupMenuItem::new(t!("Show in Files"))
                .icon(IconName::FolderInput)
                .on_click(move |_, _, cx| {
                    let path = p_reveal.clone();
                    reveal.update(cx, |this, cx| this.reveal(path, cx)).ok();
                }),
        )
        .item(
            PopupMenuItem::new(t!("Show in File Explorer"))
                .icon(IconName::FolderSearch)
                .on_click(move |_, _, cx| {
                    let path = p_explorer.clone();
                    explorer
                        .update(cx, |this, _| this.source.show_in_file_explorer(&path))
                        .ok();
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new(t!("Copy path"))
                .icon(IconName::Copy)
                .on_click(move |_, window, cx| {
                    let path = p_copy.clone();
                    copy_path
                        .update(cx, |this, cx| this.copy(path, t!("Copied path"), window, cx))
                        .ok();
                }),
        )
        .item(
            PopupMenuItem::new(t!("Copy line"))
                .icon(IconName::Copy)
                .disabled(!has_line)
                .on_click(move |_, window, cx| {
                    let text = line.clone().unwrap_or_default();
                    copy_line
                        .update(cx, |this, cx| this.copy(text, t!("Copied line"), window, cx))
                        .ok();
                }),
        )
        .separator()
        .item(
            PopupMenuItem::new(t!("File X-Ray"))
                .icon(IconName::FingerprintPattern)
                .on_click(move |_, window, cx| crate::dialogs::xray::open(window, cx, xray.clone())),
        )
        .item(
            PopupMenuItem::new(t!("What's locking this?"))
                .icon(IconName::Lock)
                .on_click(move |_, window, cx| {
                    let (this, path) = locks.clone();
                    crate::dialogs::locks::open(window, cx, vec![path], move |exe, _, cx| {
                        this.update(cx, |this, cx| this.reveal(exe, cx)).ok();
                    });
                }),
        )
    }
}

/// `path` relative to the folder searched, without a leading separator (`relative`).
pub(super) fn relative_to<'a>(path: &'a str, base: &str) -> &'a str {
    match path.strip_prefix(base) {
        Some(rest) if !base.is_empty() => rest.trim_start_matches(['/', '\\']),
        _ => path,
    }
}

/// A rewrite on one line: its line breaks and the spaces around them become one space.
fn flatten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending = false;
    for part in text.split('\n') {
        let part = part.trim();
        if pending && !part.is_empty() {
            out.push(' ');
        }
        out.push_str(part);
        pending = true;
    }
    out
}

/// The line with its hits marked (`segments`, `<mark>`). The offsets of the hits are UTF-8 byte offsets into
/// the text that is drawn; a line that starts right to left is isolated first, which moves them past the mark.
fn match_text(m: &Match, t: &theme::Tokens) -> StyledText {
    let mut joined = String::new();
    let mut ranges: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    for segment in m.segments() {
        let start = joined.len();
        joined.push_str(&segment.text);
        if segment.hit {
            ranges.push((
                start..joined.len(),
                HighlightStyle {
                    color: Some(t.primary),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    background_color: Some(t.primary.opacity(0.2)),
                    ..Default::default()
                },
            ));
        }
    }
    let shown = isolate(joined.clone());
    let shift = if shown.len() == joined.len() {
        0
    } else {
        '\u{2067}'.len_utf8()
    };
    let ranges = ranges
        .into_iter()
        .map(|(r, style)| (r.start + shift..r.end + shift, style))
        .collect::<Vec<_>>();
    StyledText::new(shown).with_highlights(ranges)
}

/// What the header says, in pieces (`true` for the numbers that are bold) and as one sentence.
pub(super) struct Summary {
    pub pieces: Vec<(SharedString, bool)>,
}

impl Summary {
    /// The pieces as one sentence, for a screen reader.
    pub fn aria(&self) -> String {
        self.pieces.iter().map(|(text, _)| text.as_ref()).collect()
    }
}

/// "**12** results in **3** files · 4 ms · 40 documents read" (the header's first span).
pub(super) fn summary_parts(
    matches: usize,
    files: usize,
    searching: bool,
    duration: f64,
    documents: bool,
    searched: Option<u64>,
) -> Summary {
    let mut pieces: Vec<(SharedString, bool)> = vec![
        (format_count(matches).into(), true),
        (
            if matches == 1 {
                t!(" result in ")
            } else {
                t!(" results in ")
            },
            false,
        ),
        (format_count(files).into(), true),
        (if files == 1 { t!(" file") } else { t!(" files") }, false),
    ];
    let mut tail = String::new();
    if !searching && duration > 0. {
        tail.push_str(&format!(" · {}", duration_text(duration)));
    }
    if documents {
        if let Some(read) = searched {
            let n = usize::try_from(read).unwrap_or(usize::MAX);
            tail.push_str(&format!(
                " · {}",
                if n == 1 {
                    t!("{n} document read", n = format_count(n))
                } else {
                    t!("{n} documents read", n = format_count(n))
                }
            ));
        }
    }
    if !tail.is_empty() {
        pieces.push((tail.into(), false));
    }
    Summary { pieces }
}
