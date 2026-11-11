//! The gallery's months, to jump through (GalleryMonths.tsx).
//!
//! | TypeScript                      | Here                                                                      |
//! |---------------------------------|---------------------------------------------------------------------------|
//! | `MONTHS`                        | [`month_name`] (translated with `t!`)                                     |
//! | `Month`, `monthsOf(rows)`       | [`Month`], [`months_of`] (pure)                                           |
//! | `GalleryMonths({rows, tab, current})` | [`GalleryMonths`], one per pane; it reads the pane's rows and focus itself |
//! | `go(entry)` (`updateTab`)       | selects the month's first item and focuses it, in the explorer            |
//!
//! # How the coordinator wires it
//!
//! - **Make it once per pane**: `cx.new(|cx| GalleryMonths::new(pane, window, cx))`.
//! - **Place it beside the list or grid**, at its end edge, as `Explorer.tsx` did (`galleryHere && !mapRows &&
//!   <GalleryMonths/>` after the view, inside a row): where the pane's tab shows the Gallery (`is_gallery(path)`)
//!   and the disk usage map is not on. It is a fixed 10 rem column (`w-40`) with a border on its start edge;
//!   give it `h_full`. It draws nothing when the rows have fewer than two months, or the window is narrower than
//!   48 rem (the TypeScript's `hidden md:block`), so it may be placed unconditionally while the tab is the Gallery.
//! - It emits no events. A click selects the month's first item in the pane's tab (`focus_pane`, then
//!   `set_selection`); the list or grid, which follows the tab's focus, scrolls it into view.
//!
//! # Differences from the TypeScript
//!
//! - The list is virtualized (years and months are fixed-height lines of one `uniform_list`): a gallery of
//!   twenty years has hundreds of lines.
//! - The months are worked out when the tab's rows change, not on every draw, and for more than 5,000 rows on a
//!   background thread.
//! - Month names go through `t!` (the TypeScript's were English only); the Arabic catalog has none yet, so
//!   they show in English (see the report).

#[cfg(test)]
mod tests;

use crate::explorer::{explorer, ExplorerEvent, Rows};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::format::format_count;
use gpui_kit::{ScrollStrategy, UniformListScrollHandle};
use std::ops::Range;

/// Rows above this many are grouped into months on a background thread.
const BACKGROUND_ABOVE: usize = 5_000;

/// The narrowest window the months show in: Tailwind's `md` (48 rem).
const MIN_WIDTH_REM: f32 = 48.;

/// A line of the list is this tall (`py-1` and `text-xs`, and a year's heading made the same height).
const LINE_HEIGHT_REM: f32 = 1.75;

/// A month photos and videos were taken in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Month {
    pub year: i64,
    /// 0 for January, as JavaScript counts.
    pub month: u32,
    pub count: usize,
    /// Its first photo or video, in the order shown (the path).
    pub first: String,
}

/// The months photos and videos were taken in, in the order shown (newest first), each with how many: runs of
/// the same month in `rows`, files only, skipping what has no date (`monthsOf`).
pub fn months_of(rows: &Rows) -> Vec<Month> {
    let mut months: Vec<Month> = Vec::new();
    for entry in rows.iter() {
        if entry.dir || entry.modified == 0 {
            continue;
        }
        let Some((year, month)) = local_month(entry.modified) else {
            continue;
        };
        match months.last_mut() {
            Some(last) if last.year == year && last.month == month => last.count += 1,
            _ => months.push(Month {
                year,
                month,
                count: 1,
                first: entry.path.clone(),
            }),
        }
    }
    months
}

/// The local year and month (0 to 11) of a time in Unix milliseconds, from Windows' own time zone rules (what JS's
/// `getFullYear()` and `getMonth()` read). A time Windows can't convert falls back to UTC.
pub fn local_month(ms: i64) -> Option<(i64, u32)> {
    // TODO(coordinator): from a shared date helper (dialogs::batch_rename keeps a private copy of this too).
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns from 1601.
    let local = u64::try_from(ms.checked_add(11_644_473_600_000)?)
        .ok()
        .and_then(|ms| ms.checked_mul(10_000))
        .and_then(|ticks| {
            let file_time = FILETIME {
                dwLowDateTime: u32::try_from(ticks & 0xFFFF_FFFF).ok()?,
                dwHighDateTime: u32::try_from(ticks >> 32).ok()?,
            };
            let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
            // SAFETY: every pointer is to a live local; the calls only read the first and write the second.
            unsafe {
                FileTimeToSystemTime(&file_time, &mut utc).ok()?;
                SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
            }
            Some((i64::from(local.wYear), u32::from(local.wMonth).saturating_sub(1)))
        });
    local.or_else(|| {
        let (year, month) = utc_month(ms);
        Some((year, month))
    })
}

/// The UTC year and month (0 to 11) of a time (Howard Hinnant's days-to-civil).
fn utc_month(ms: i64) -> (i64, u32) {
    let days = ms.div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    // `month` is 1..=12 by construction.
    (year, u32::try_from(month - 1).unwrap_or(0))
}

/// A month's name, translated (`MONTHS[month]`); "" for a number past December.
pub fn month_name(month: u32) -> SharedString {
    match month {
        0 => t!("January"),
        1 => t!("February"),
        2 => t!("March"),
        3 => t!("April"),
        4 => t!("May"),
        5 => t!("June"),
        6 => t!("July"),
        7 => t!("August"),
        8 => t!("September"),
        9 => t!("October"),
        10 => t!("November"),
        11 => t!("December"),
        _ => SharedString::default(),
    }
}

/// One line of the list: a year's heading, or one of its months.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Year(i64),
    /// An index into the months.
    Month(usize),
}

/// The years and months, with a heading before the first month of each year (`i === 0 || previous year differs`).
fn lines_of(months: &[Month]) -> Vec<Line> {
    let mut lines = Vec::with_capacity(months.len() + months.len() / 6 + 1);
    for (i, month) in months.iter().enumerate() {
        if i == 0 || months.get(i - 1).is_none_or(|before| before.year != month.year) {
            lines.push(Line::Year(month.year));
        }
        lines.push(Line::Month(i));
    }
    lines
}

/// The gallery's years and months beside its list, newest first, each with how many photos and videos.
pub struct GalleryMonths {
    pane: usize,
    months: Vec<Month>,
    lines: Vec<Line>,
    /// Numbers the background grouping, so an older result is dropped.
    seq: u64,
    scroll: UniformListScrollHandle,
    /// One focus handle (a tab stop) per month, so the keyboard reaches and moves between them.
    focus: Vec<FocusHandle>,
    /// The month last scrolled to, so the list scrolls when the focus moves to another month and not on every draw.
    scrolled_to: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl GalleryMonths {
    /// The months of `pane`'s tab.
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        let subscriptions = vec![
            cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| match event {
                ExplorerEvent::RowsChanged(_) | ExplorerEvent::TabsChanged | ExplorerEvent::ListingChanged { .. } => {
                    this.rebuild(cx)
                }
                _ => {}
            }),
            // The focused item's month is marked: redraw when the selection moves.
            cx.observe(&explorer, |_, _, cx| cx.notify()),
        ];
        let mut this = GalleryMonths {
            pane,
            months: Vec::new(),
            lines: Vec::new(),
            seq: 0,
            scroll: UniformListScrollHandle::new(),
            focus: Vec::new(),
            scrolled_to: None,
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this
    }

    /// The months shown, newest first.
    pub fn months(&self) -> &[Month] {
        &self.months
    }

    /// Whether anything is drawn: two months or more.
    pub fn is_shown(&self) -> bool {
        self.months.len() >= 2
    }

    /// Groups the pane's rows into months again.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let rows = {
            let explorer = explorer(cx);
            explorer.read(cx).pane_tab(self.pane).map(|tab| tab.rows.clone())
        };
        let Some(rows) = rows else {
            self.set(Vec::new(), cx);
            return;
        };
        self.seq += 1;
        if rows.len() <= BACKGROUND_ABOVE {
            let months = months_of(&rows);
            self.set(months, cx);
            return;
        }
        let seq = self.seq;
        cx.spawn(async move |this, cx| {
            let months = cx.background_spawn(async move { months_of(&rows) }).await;
            this.update(cx, |this, cx| {
                if this.seq == seq {
                    this.set(months, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn set(&mut self, months: Vec<Month>, cx: &mut Context<Self>) {
        if months != self.months {
            self.lines = lines_of(&months);
            self.focus.truncate(months.len());
            while self.focus.len() < months.len() {
                self.focus.push(cx.focus_handle().tab_stop(true));
            }
            self.months = months;
            cx.notify();
        }
    }

    /// The index of the month the pane's focused item is in.
    fn current(&self, cx: &App) -> Option<usize> {
        let explorer = explorer(cx);
        let explorer = explorer.read(cx);
        let tab = explorer.pane_tab(self.pane)?;
        let entry = tab
            .selection
            .focus()
            .and_then(|focus| tab.rows.position_of(focus))
            .and_then(|at| tab.rows.get(at))?;
        if entry.modified == 0 {
            return None;
        }
        let (year, month) = local_month(entry.modified)?;
        self.months.iter().position(|m| m.year == year && m.month == month)
    }

    /// A click: the month's first item is selected and focused (`updateTab` with that selection, focus and anchor).
    fn go(&mut self, month: usize, cx: &mut Context<Self>) {
        let Some(first) = self.months.get(month).map(|m| m.first.clone()) else {
            return;
        };
        let pane = self.pane;
        explorer(cx).update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            explorer.set_selection([first.as_str()], Some(&first), Some(&first), cx);
        });
    }

    /// Keys on a month: Up and Down move to the previous and next month, Home and End to the first and last, Enter
    /// and Space select it. Keys with a modifier are the shortcuts' and pass by.
    fn on_key(&mut self, at: usize, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let last = self.months.len().saturating_sub(1);
        let target = match event.keystroke.key.as_str() {
            "down" => (at + 1).min(last),
            "up" => at.saturating_sub(1),
            "home" => 0,
            "end" => last,
            "enter" | "space" => {
                self.go(at, cx);
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if let Some(handle) = self.focus.get(target) {
            if let Some(line) = self.lines.iter().position(|l| *l == Line::Month(target)) {
                self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
            }
            handle.focus(window, cx);
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn render_lines(&mut self, range: Range<usize>, _window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme::tokens(cx).clone();
        let here = self.current(cx);
        range
            .filter_map(|ix| {
                let line = *self.lines.get(ix)?;
                Some(match line {
                    Line::Year(year) => {
                        let label: SharedString = year.to_string().into();
                        div()
                            .id(("gallery-year", ix))
                            .role(Role::Heading)
                            .aria_label(label.clone())
                            .test_support()
                            .h(rems(LINE_HEIGHT_REM))
                            .w_full()
                            .flex_none()
                            .flex()
                            .items_end()
                            .px(space(3.))
                            .pb(space(1.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.foreground)
                            .child(label)
                            .into_any_element()
                    }
                    Line::Month(at) => {
                        let month = self.months.get(at)?;
                        let name = month_name(month.month);
                        let count = format_count(month.count);
                        // "January 2026": the name alone repeats every year, and a name shown twice must be told apart.
                        let label = t!("{month} {year}", month = name.clone(), year = month.year.to_string());
                        let (accent, foreground, muted) = (t.accent, t.foreground, t.muted_foreground);
                        let marked = here == Some(at);
                        let handle = self.focus.get(at)?.clone();
                        let ring = t.ring;
                        row_start()
                            .id(("gallery-month", at))
                            .role(Role::Button)
                            .aria_label(label)
                            .aria_description(t!("{count} photos and videos", count = count.clone()))
                            .test_support()
                            .track_focus(&handle)
                            .focus(move |style| style.border_1().border_color(ring))
                            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                                this.on_key(at, event, window, cx)
                            }))
                            .h(rems(LINE_HEIGHT_REM))
                            .w_full()
                            .flex_none()
                            .justify_between()
                            .gap(space(2.))
                            .px(space(3.))
                            .text_color(muted)
                            .cursor_pointer()
                            .hover(move |style| style.bg(accent).text_color(foreground))
                            .when(marked, |button| {
                                button
                                    .bg(accent)
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(foreground)
                            })
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.go(at, cx)))
                            .child(div().min_w_0().truncate().child(name))
                            .child(div().flex_none().child(count))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }
}

impl Render for GalleryMonths {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `hidden md:block`, and `if (months.length < 2) return null`.
        let wide = window.viewport_size().width >= window.rem_size() * MIN_WIDTH_REM;
        if !self.is_shown() || !wide {
            return div().into_any_element();
        }
        let t = theme::tokens(cx).clone();
        // Keep the focused item's month in view.
        let current = self.current(cx);
        if current != self.scrolled_to {
            self.scrolled_to = current;
            if let Some(line) = current.and_then(|at| self.lines.iter().position(|l| *l == Line::Month(at))) {
                self.scroll.scroll_to_item(line, ScrollStrategy::Nearest);
            }
        }
        let list = uniform_list(
            "gallery-months-list",
            self.lines.len(),
            cx.processor(|this, range, window, cx| this.render_lines(range, window, cx)),
        )
        .track_scroll(&self.scroll)
        .size_full();
        let frame = div()
            .id("gallery-months")
            .role(Role::Navigation)
            .aria_label(t!("Months"))
            .test_support()
            .relative()
            .h_full()
            .w(rems(10.))
            .flex_none()
            .py(space(2.))
            .text_size(theme::size::XS)
            .text_start()
            .border_color(t.border)
            .child(list)
            .child(Scrollbar::vertical(&self.scroll).viewport_from_layout());
        // `border-s`: the left edge in English, the right in Arabic.
        if crate::i18n::is_rtl() {
            frame.border_r_1()
        } else {
            frame.border_l_1()
        }
        .into_any_element()
    }
}
