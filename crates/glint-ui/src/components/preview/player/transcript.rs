//! What speech search heard in a recording, each part a jump to when it is said; the parts that say what
//! `says:` searched for are marked (`Transcript` in FilePreview.tsx).
//!
//! The words come from the active tab's search, `says:budget meeting` ([`says_words`]); the transcript is read
//! again when they change. A recording speech search hasn't heard shows nothing at all, as in the TypeScript.
//! Clicking a part (or Enter on the part the arrows are on) emits [`TranscriptEvent::Play`] with the second it
//! begins at: the player goes there and plays.
//!
//! A long transcript is a virtualized list (a two-hour talk is thousands of parts); a short one is plain rows.

use super::engine::clock_ms;
use super::sources::{player_sources, Heard, PlayerSources};
use super::{RowDown, RowPlay, RowUp};
use crate::kit::prelude::*;
use crate::{t, theme};
use gpui_kit::{list, ListAlignment, ListState, ScrollHandle, Task};

/// More parts than this are a virtualized list in a box of fixed height (`max-h-48`); fewer are plain rows.
const PLAIN_ROWS: usize = 12;

/// What the transcript asks of the player.
#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptEvent {
    /// "Play from here": go to this second and play (`currentTime = ms / 1000; play()`).
    Play(f64),
}

impl EventEmitter<TranscriptEvent> for Transcript {}

/// The words `says:` searched for in `query` (`/(?:^|[\s"])says:(.*)$/i`, quotes removed, trimmed): empty when
/// the search has none.
pub fn says_words(query: &str) -> String {
    let lower = query.to_lowercase();
    // Lowercasing can change byte lengths (rare: İ); then the offsets can't be trusted and nothing is found.
    if lower.len() != query.len() {
        return String::new();
    }
    let mut from = 0;
    while let Some(found) = lower[from..].find("says:") {
        let at = from + found;
        let before_ok = at == 0
            || lower[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || c == '"');
        if before_ok {
            // `.` stops at a line break; `$` then needs the end of the text, so a break ends the search.
            let rest = &query[at + 5..];
            if rest.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
                from = at + 5;
                continue;
            }
            return rest.replace('"', "").trim().to_string();
        }
        from = at + 5;
    }
    String::new()
}

/// The transcript under a recording's player.
pub struct Transcript {
    path: String,
    words: String,
    sources: PlayerSources,
    heard: Option<Heard>,
    /// The part the arrows are on.
    cursor: usize,
    list: ListState,
    scroll: ScrollHandle,
    pub(super) focus: FocusHandle,
    reading: Option<Task<()>>,
}

impl Transcript {
    /// The transcript of the recording at `path`, with the parts that say `words` marked; it is read at once.
    pub fn new(path: String, words: String, cx: &mut Context<Self>) -> Self {
        let mut transcript = Transcript {
            path,
            words,
            sources: player_sources(cx),
            heard: None,
            cursor: 0,
            list: ListState::new(0, ListAlignment::Top, px(200.)),
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            reading: None,
        };
        transcript.read(cx);
        transcript
    }

    /// The words marked, changed by the search: read again when they differ.
    pub fn set_words(&mut self, words: String, cx: &mut Context<Self>) {
        if words != self.words {
            self.words = words;
            self.read(cx);
        }
    }

    /// What was heard, once it has been read.
    pub fn heard(&self) -> Option<&Heard> {
        self.heard.as_ref()
    }

    /// The part the arrows are on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        let words = (!self.words.is_empty()).then(|| self.words.clone());
        let running = (self.sources.heard)(&self.path, words.as_deref());
        self.reading = Some(cx.spawn(async move |this, cx| {
            let heard = running.await;
            this.update(cx, |this, cx| {
                this.list.reset(heard.as_ref().map_or(0, |h| h.parts.len()));
                // The first part that says the words is shown (`scrollIntoView` of the first `data-hit`).
                let first = heard.as_ref().and_then(|h| h.hits.iter().copied().min());
                this.cursor = first.unwrap_or(0);
                this.heard = heard;
                if let Some(first) = first {
                    this.reveal(first);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn reveal(&self, index: usize) {
        self.list.scroll_to_reveal_item(index);
        self.scroll.scroll_to_item(index);
    }

    fn play(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some((ms, _)) = self.heard.as_ref().and_then(|h| h.parts.get(index)) {
            self.cursor = index;
            cx.emit(TranscriptEvent::Play(*ms as f64 / 1000.));
            cx.notify();
        }
    }

    /// Page Down and Page Up in Quick Look: the rows scroll 0.85 of their box. Returns whether anything moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let Some(count) = self.heard.as_ref().map(|h| h.parts.len()).filter(|n| *n > 0) else {
            return false;
        };
        if count > PLAIN_ROWS {
            let step = self.list.viewport_bounds().size.height * 0.85;
            self.list.scroll_by(if down { step } else { -step });
        } else {
            let max = self.scroll.max_offset().y;
            let offset = self.scroll.offset();
            let step = self.scroll.bounds().size.height * 0.85;
            let top = if down { offset.y - step } else { offset.y + step };
            let top = top.clamp(-max, px(0.));
            if max <= px(0.) || top == offset.y {
                return false;
            }
            self.scroll.set_offset(gpui_kit::point(offset.x, top));
        }
        cx.notify();
        true
    }

    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let count = self.heard.as_ref().map_or(0, |h| h.parts.len());
        if count == 0 {
            return;
        }
        self.cursor = self.cursor.saturating_add_signed(by).min(count - 1);
        self.reveal(self.cursor);
        cx.notify();
    }
}

impl Render for Transcript {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let Some(heard) = self.heard.clone().filter(|h| !h.parts.is_empty()) else {
            return div().into_any_element();
        };
        let title: SharedString = if heard.lang.is_empty() {
            t!("What's said")
        } else {
            t!("What's said ({lang})", lang = heard.lang.clone())
        };
        let focused = self.focus.is_focused(window);
        let cursor = self.cursor;
        let view = cx.entity();
        let row = {
            let (heard, t) = (heard.clone(), t.clone());
            move |i: usize| -> AnyElement {
                let Some((ms, text)) = heard.parts.get(i) else {
                    return div().into_any_element();
                };
                let hit = heard.hits.contains(&i);
                let time = clock_ms(*ms);
                let view = view.clone();
                row_start()
                    .id(("transcript-part", i))
                    .role(Role::Button)
                    .aria_label(format!("{time} {text}"))
                    .test_support()
                    .w_full()
                    .gap(space(2.))
                    .items_start()
                    .rounded(theme::size::RADIUS_SM)
                    .px(space(1.))
                    .py(space(0.5))
                    .text_start()
                    .cursor_pointer()
                    .when(hit, |row| row.bg(t.primary.opacity(0.15)))
                    .when(focused && i == cursor, |row| row.border_1().border_color(t.ring))
                    .hover(|row| row.bg(t.muted))
                    .tooltip(move |window, cx| crate::kit::tooltip(t!("Play from here"), window, cx))
                    .on_click(move |_, _, cx| view.update(cx, |this, cx| this.play(i, cx)))
                    .child(div().flex_none().text_color(t.muted_foreground).child(time))
                    .child(div().flex_1().min_w_0().child(isolate(text.clone())))
                    .into_any_element()
            }
        };
        let rows = if heard.parts.len() > PLAIN_ROWS {
            let row = row.clone();
            let list = list(self.list.clone(), move |i, _, _| row(i)).size_full();
            div()
                .relative()
                .h(rems(12.))
                .child(list)
                .vertical_scrollbar(&self.list)
                .into_any_element()
        } else {
            div()
                .id("transcript-rows")
                .flex()
                .flex_col()
                .max_h(rems(12.))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children((0..heard.parts.len()).map(row))
                .into_any_element()
        };
        col()
            .id("transcript")
            .role(Role::Group)
            .aria_label(title.clone())
            .test_support()
            .key_context("Transcript")
            .track_focus(&self.focus)
            .tab_stop(true)
            .on_action(cx.listener(|this, _: &RowUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &RowDown, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &RowPlay, _, cx| {
                let at = this.cursor;
                this.play(at, cx)
            }))
            .mt(space(3.))
            .w_full()
            .max_w(rems(28.))
            .gap(space(1.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(if focused { t.ring } else { t.border })
            .bg(t.card.opacity(0.4))
            .p(space(2.))
            .text_size(theme::size::XS)
            .text_start()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.muted_foreground)
                    .child(title),
            )
            .child(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_what_follows_says_in_the_search() {
        assert_eq!(says_words("says:budget meeting"), "budget meeting");
        assert_eq!(says_words("ext:mp4 says:\"budget meeting\""), "budget meeting");
        assert_eq!(says_words("  SAYS:  Hello  "), "Hello");
        assert_eq!(says_words("\"says:x"), "x");
        // Not at the start of a word: it is part of another word.
        assert_eq!(says_words("nosays:x"), "");
        assert_eq!(says_words("ext:mp4"), "");
        assert_eq!(says_words(""), "");
        // The first one counts, and everything after it is the words.
        assert_eq!(says_words("says:a says:b"), "a says:b");
        assert_eq!(says_words("says:"), "");
        assert_eq!(says_words("says:ميزانية الربع"), "ميزانية الربع");
    }
}
