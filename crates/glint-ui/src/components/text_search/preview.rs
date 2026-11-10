//! The match in its file, beside the results (Preview.tsx and its `DocumentLines`).
//!
//! | TypeScript                                | Here                                                         |
//! |-------------------------------------------|--------------------------------------------------------------|
//! | `BLOCK`, `windowStart`                    | [`BLOCK`], [`window_start`]: 300-line windows aligned to 100-line blocks, so nearby matches reuse one |
//! | `Preview` (file header, buttons, lines)   | `TextSearch::preview_pane`                                    |
//! | `invoke("read_file_chunk")`               | `Source::chunk`, in the background                            |
//! | `DocumentLines`, `docLines`               | the same pane with a document's lines (`Source::doc_lines`), each page's label once |
//! | `.hl-line` scrolled to the middle          | the match's line tinted, and scrolled to the middle           |
//!
//! Not ported: the syntax colors (Prism). The text and code previews highlight with the kit's editor, which
//! numbers lines from 1 and can't mark a line; this pane has to number from the window's first line and mark
//! the match's, so it draws plain lines. A line wider than the pane is cut, not scrolled sideways.

use super::{TextSearch, TextSearchEvent};
use crate::kit::prelude::*;
use crate::{state, t, theme};
use gpui_kit::{ScrollHandle, ScrollStrategy, Task, UniformListScrollHandle};
use std::ops::Range;

/// The preview loads 300-line windows aligned to 100-line blocks, so nearby matches reuse it.
pub(super) const BLOCK: usize = 100;

/// The first line of the window around `line` (`windowStart`).
pub(super) fn window_start(line: usize) -> usize {
    let block = line.saturating_sub(1) / BLOCK;
    (block.saturating_sub(1) * BLOCK + 1).max(1)
}

/// What is loaded, or being loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    path: String,
    start: usize,
    in_document: bool,
}

enum Loaded {
    /// A text file's lines, from `start`.
    Text { start: usize, lines: Vec<SharedString> },
    /// A document's lines, from `start`, each with where it is.
    Document {
        start: usize,
        lines: Vec<(SharedString, SharedString)>,
    },
}

/// The preview's state.
#[derive(Default)]
pub(super) struct Preview {
    key: Option<Key>,
    loaded: Option<Loaded>,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    /// The match the pane was last scrolled to.
    scrolled_to: Option<(String, usize)>,
    code: UniformListScrollHandle,
    document: ScrollHandle,
}

/// A row of a code window is `leading-5`.
fn line_height() -> Rems {
    rems(1.25)
}

impl TextSearch {
    /// Loads the window around the shown match if it isn't loaded already. Cheap to call every frame.
    pub(super) fn ensure_preview(&mut self, cx: &mut Context<Self>) {
        let shown = {
            let search = self.search.read(cx);
            search
                .current(state::settings(cx).auto_open_preview)
                .and_then(|i| search.matches().get(i))
                .filter(|m| !m.path.is_empty())
                .map(|m| (m.path.clone(), m.line_number.unwrap_or(1).max(1), m.label.is_some()))
        };
        let Some((path, line, in_document)) = shown else {
            self.preview.key = None;
            self.preview.loaded = None;
            self.preview.task = None;
            return;
        };
        let line = usize::try_from(line).unwrap_or(usize::MAX);
        let start = window_start(line);
        let key = Key {
            path: path.clone(),
            start,
            in_document,
        };
        if self.preview.key.as_ref() == Some(&key) {
            self.scroll_preview(&path, line);
            return;
        }
        self.preview.key = Some(key.clone());
        self.preview.scrolled_to = None;
        let end = start + 3 * BLOCK - 1;
        let source = self.source.clone();
        self.preview.task = Some(if in_document {
            let reading = source.doc_lines(&path, start, end);
            cx.spawn(async move |this, cx| {
                let answer = reading.await;
                this.update(cx, |this, cx| {
                    if this.preview.key.as_ref() != Some(&key) {
                        return;
                    }
                    match answer {
                        Ok(lines) => {
                            this.preview.loaded = Some(Loaded::Document {
                                start,
                                lines: lines.into_iter().map(|(l, t)| (l.into(), t.into())).collect(),
                            });
                            this.preview.error = None;
                        }
                        Err(reason) => this.preview.error = Some(reason.into()),
                    }
                    cx.notify();
                })
                .ok();
            })
        } else {
            let reading = source.chunk(&path, start, end);
            cx.spawn(async move |this, cx| {
                let answer = reading.await;
                this.update(cx, |this, cx| {
                    if this.preview.key.as_ref() != Some(&key) {
                        return;
                    }
                    match answer {
                        Ok(text) => {
                            this.preview.loaded = Some(Loaded::Text {
                                start,
                                lines: text
                                    .split('\n')
                                    .map(|l| SharedString::from(l.trim_end_matches('\r').replace('\t', "    ")))
                                    .collect(),
                            });
                            this.preview.error = None;
                        }
                        Err(reason) => this.preview.error = Some(reason.into()),
                    }
                    cx.notify();
                })
                .ok();
            })
        });
    }

    /// Scrolls the loaded window to the match's line, once for each match.
    fn scroll_preview(&mut self, path: &str, line: usize) {
        let wanted = (path.to_string(), line);
        if self.preview.scrolled_to.as_ref() == Some(&wanted) {
            return;
        }
        let Some(loaded) = &self.preview.loaded else {
            return;
        };
        match loaded {
            Loaded::Text { start, lines } => {
                let at = line.saturating_sub(*start).min(lines.len().saturating_sub(1));
                self.preview.code.scroll_to_item(at, ScrollStrategy::Center);
            }
            Loaded::Document { start, lines } => {
                let at = line.saturating_sub(*start).min(lines.len().saturating_sub(1));
                self.preview.document.scroll_to_item(at);
            }
        }
        self.preview.scrolled_to = Some(wanted);
    }

    /// The pane: the file's name and where the match is, two buttons, and the lines around the match.
    pub(super) fn preview_pane(&mut self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let shown = {
            let search = self.search.read(cx);
            search
                .current(state::settings(cx).auto_open_preview)
                .and_then(|i| search.matches().get(i))
                .filter(|m| !m.path.is_empty())
                .cloned()
        };
        let Some(m) = shown else {
            return col()
                .id("text-preview-empty")
                .role(Role::Status)
                .aria_label(t!("Select a result to preview it"))
                .test_support()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(space(2.))
                .text_size(theme::size::SM)
                .text_color(t.muted_foreground)
                .child(icon(IconName::FileSearch).size_6())
                .child(t!("Select a result to preview it"))
                .into_any_element();
        };
        let name = super::results::basename(&m.path).to_string();
        let line = usize::try_from(m.line_number.unwrap_or(1)).unwrap_or(1).max(1);
        let count = usize::try_from(m.line_count.unwrap_or(1)).unwrap_or(1).max(1);
        let where_text: SharedString = match &m.label {
            Some(label) => label.clone().into(),
            None => t!("line {n}", n = line),
        };
        let (reveal, open) = (m.path.clone(), m.path.clone());
        let show_numbers = state::settings(cx).show_line_numbers;

        let header = row_start()
            .flex_none()
            .h(space(10.))
            .gap(space(2.))
            .ps(3.)
            .pe(1.5)
            .border_b_1()
            .border_color(t.border)
            .bg(theme::chrome_fill(cx))
            .child(
                div()
                    .id("text-preview-title")
                    .role(Role::Heading)
                    .aria_label(format!("{name} · {where_text}"))
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::SM)
                    .tooltip({
                        let tip: SharedString = m.path.clone().into();
                        move |window, cx| crate::kit::tooltip(tip.clone(), window, cx)
                    })
                    .child(
                        row_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(isolate(name)))
                            .child(div().text_color(t.muted_foreground).child(format!(" · {where_text}"))),
                    ),
            )
            .child(
                row_start()
                    .flex_none()
                    .child(
                        Button::new("text-preview-reveal")
                            .ghost()
                            .small()
                            .icon(IconName::FolderInput)
                            .tooltip(t!("Show in Files"))
                            .accessibility_label(t!("Show in Files"))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(TextSearchEvent::Reveal { path: reveal.clone() })
                            })),
                    )
                    .child(
                        Button::new("text-preview-open")
                            .ghost()
                            .small()
                            .icon(IconName::ExternalLink)
                            .tooltip(t!("Open file (Enter)"))
                            .accessibility_label(t!("Open file"))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_file(open.clone(), window, cx))),
                    ),
            );

        let body: AnyElement = if let Some(error) = self.preview.error.clone() {
            div()
                .id("text-preview-error")
                .role(Role::Alert)
                .aria_label(error.clone())
                .test_support()
                .p(space(4.))
                .text_size(theme::size::SM)
                .text_color(t.destructive)
                .child(error)
                .into_any_element()
        } else {
            match &self.preview.loaded {
                Some(Loaded::Text { start, lines }) => self.code_lines(*start, lines.len(), line, show_numbers, t, cx),
                Some(Loaded::Document { start, lines }) => {
                    self.document_lines(*start, lines.clone(), line..line + count, t)
                }
                None => div().flex_1().into_any_element(),
            }
        };

        col()
            .id("text-preview")
            .role(Role::Group)
            .aria_label(t!("Preview"))
            .test_support()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(div().flex_1().min_h_0().bg(t.card).child(body))
            .into_any_element()
    }

    /// A text file's window: line numbers, the lines, the match's tinted.
    fn code_lines(
        &self,
        start: usize,
        count: usize,
        hit: usize,
        numbers: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = t;
        div()
            .id("text-preview-code")
            .role(Role::Group)
            .aria_label(t!("File text"))
            .test_support()
            .relative()
            .size_full()
            .py(space(2.))
            .child(
                uniform_list(
                    "text-preview-lines",
                    count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let t = theme::tokens(cx).clone();
                        let mono = theme::mono_font(cx);
                        let Some(Loaded::Text { lines, .. }) = &this.preview.loaded else {
                            return Vec::new();
                        };
                        range
                            .filter_map(|i| Some((i, lines.get(i)?.clone())))
                            .map(|(i, text)| {
                                let number = start + i;
                                let marked = number == hit;
                                row_start()
                                    .id(("text-preview-line", i))
                                    .role(Role::ListItem)
                                    .aria_label(text.clone())
                                    .aria_description(t!("line {n}", n = number))
                                    .test_support()
                                    .relative()
                                    .h(line_height())
                                    .w_full()
                                    .gap(space(3.))
                                    .ps(3.)
                                    .pe(4.)
                                    .font_family(mono.clone())
                                    .text_size(rems(0.8125))
                                    .when(marked, |row| {
                                        row.bg(t.primary.opacity(0.14)).child(
                                            div()
                                                .absolute()
                                                .top_0()
                                                .bottom_0()
                                                .when(crate::i18n::is_rtl(), |d| d.right_0())
                                                .when(!crate::i18n::is_rtl(), |d| d.left_0())
                                                .w(px(2.))
                                                .bg(t.primary),
                                        )
                                    })
                                    .when(numbers, |row| {
                                        row.child(
                                            div()
                                                .w(space(10.))
                                                .flex_none()
                                                .text_end()
                                                .text_color(t.muted_foreground.opacity(0.6))
                                                .child(number.to_string()),
                                        )
                                    })
                                    .child(div().flex_1().min_w_0().truncate().child(isolate(text)))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.preview.code)
                .size_full(),
            )
            .vertical_scrollbar(&self.preview.code)
            .into_any_element()
    }

    /// A document's window: each page's (slide's, row's) label once, then its lines, wrapped; the match's
    /// passage (its `count` lines) tinted.
    fn document_lines(
        &self,
        start: usize,
        lines: Vec<(SharedString, SharedString)>,
        passage: Range<usize>,
        t: &theme::Tokens,
    ) -> AnyElement {
        let rows = lines.iter().enumerate().map(|(i, (label, text))| {
            let number = start + i;
            let marked = passage.contains(&number);
            let first = i == 0 || lines.get(i - 1).is_none_or(|(before, _)| before != label);
            row_start()
                .id(("text-preview-doc-line", i))
                .role(Role::ListItem)
                .aria_label(text.clone())
                .aria_description(label.clone())
                .test_support()
                .w_full()
                .gap(space(3.))
                .ps(3.)
                .pe(4.)
                .items_start()
                .when(marked, |row| row.bg(t.primary.opacity(0.14)))
                .child(
                    div()
                        .w(space(20.))
                        .flex_none()
                        .truncate()
                        .text_end()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground.opacity(0.7))
                        .child(if first { label.clone() } else { SharedString::default() }),
                )
                .child(div().flex_1().min_w_0().child(isolate(text.clone())))
        });
        div()
            .id("text-preview-document")
            .role(Role::Group)
            .aria_label(t!("Document text"))
            .test_support()
            .size_full()
            .py(space(2.))
            .text_size(rems(0.8125))
            .overflow_y_scroll()
            .track_scroll(&self.preview.document)
            .children(rows)
            .into_any_element()
    }
}
