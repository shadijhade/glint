//! What changed in a file since its last commit: its version in HEAD against the one on disk, line by line, with
//! the two side by side one click away. Ported from src/components/explorer/ChangesView.tsx.
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `ChangesView`                                | [`ChangesView`], the preview's "Changes" view of a file        |
//! | `gitDiff(entry.path)`                        | `Source::diff`, on core's runtime                             |
//! | `sideBySide`, `compareFiles({ … })`          | [`ChangesView::side_by_side`] and [`sides`], through `dialogs::compare_files::open_sides` |
//! | `gitHeadDiscard(copy)` on close              | the `on_close` handed to `open_sides`                         |
//! | the rows (`role="table"`)                    | a `uniform_list` of lines, gaps and the cut note              |
//! | `Centered`                                   | `components::preview::states::centered`                        |
//!
//! **Long files.** The diff is at most 5,000 lines (glint-core cuts it) and the rows are a virtualized list, so
//! only the dozen on screen are built. A line is one row of the list: one longer than [`MAX_SHOWN`] characters is
//! cut when drawn (the TypeScript wrapped it).

use super::model::{shared, Source};
use super::words::{self, Tone};
use crate::components::preview::states::centered;
use crate::dialogs::compare_files::{self, OnClose, Side};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::git::{FileDiff, LineKind};
use glint_model::entries::FileEntry;
use glint_model::format::{format_duration, format_size};
use gpui_kit::{uniform_list, Task, UniformListScrollHandle};
use std::ops::Range;
use std::rc::Rc;

/// The most characters of a line that are drawn.
pub const MAX_SHOWN: usize = 1000;

/// What opens the side-by-side comparison: `compare_files::open_sides` in the app, a recorder in tests.
pub type OpenSides = Rc<dyn Fn(&mut Window, &mut App, Side, Side, Option<OnClose>)>;

/// The comparison's two sides: the file as it was in the last commit (read-only, so it can't be changed or saved
/// over by a merge), and the one on disk as "Now".
pub fn sides(copy: &str, path: &str) -> (Side, Side) {
    (
        Side::named(copy, t!("In the last commit"), true),
        Side::named(path, t!("Now"), false),
    )
}

/// One line of the list.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    Line {
        kind: LineKind,
        old: u32,
        new: u32,
        text: SharedString,
    },
    /// Same lines left out between two changes: a gap in the numbers.
    Gap,
    /// More changed than is shown.
    Cut(usize),
}

/// The list's items for a diff: its lines, with a gap marker wherever the numbers jump.
fn items_of(diff: &FileDiff) -> Vec<Item> {
    let mut items = Vec::with_capacity(diff.lines.len() + 1);
    let (mut last_old, mut last_new) = (0u32, 0u32);
    for (i, line) in diff.lines.iter().enumerate() {
        let gap = i > 0
            && ((line.old > 0 && last_old > 0 && line.old != last_old + 1)
                || (line.new > 0 && last_new > 0 && line.new != last_new + 1));
        if line.old > 0 {
            last_old = line.old;
        }
        if line.new > 0 {
            last_new = line.new;
        }
        if gap {
            items.push(Item::Gap);
        }
        let text: String = if line.text.chars().count() > MAX_SHOWN {
            let mut cut: String = line.text.chars().take(MAX_SHOWN).collect();
            cut.push('…');
            cut
        } else {
            line.text.clone()
        };
        items.push(Item::Line {
            kind: line.kind,
            old: line.old,
            new: line.new,
            text: text.into(),
        });
    }
    if diff.cut {
        items.push(Item::Cut(diff.lines.len()));
    }
    items
}

enum Load {
    Reading,
    Failed(SharedString),
    /// Not in a Git repository.
    Outside,
    Ready {
        diff: FileDiff,
        items: Vec<Item>,
    },
}

/// A file's changes since its last commit.
pub struct ChangesView {
    entry: FileEntry,
    source: Rc<dyn Source>,
    open_sides: OpenSides,
    load: Load,
    generation: u64,
    scroll: UniformListScrollHandle,
    _read: Task<()>,
}

impl ChangesView {
    /// The changes of `entry`, read in the background through the Git model's source.
    pub fn new(entry: FileEntry, cx: &mut Context<Self>) -> Self {
        let source = shared(cx).read(cx).source();
        let open: OpenSides =
            Rc::new(|window, cx, left, right, on_close| compare_files::open_sides(window, cx, left, right, on_close));
        Self::with_parts(entry, source, open, cx)
    }

    /// [`ChangesView::new`] with a source and a way to open the comparison of your own (tests).
    pub fn with_parts(entry: FileEntry, source: Rc<dyn Source>, open_sides: OpenSides, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            entry,
            source,
            open_sides,
            load: Load::Reading,
            generation: 0,
            scroll: UniformListScrollHandle::new(),
            _read: Task::ready(()),
        };
        this.read(cx);
        this
    }

    /// The file shown.
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }

    /// Shows another file, or reads this one again when it changed (the effect's `[path, modified, size]`).
    pub fn set_entry(&mut self, entry: FileEntry, cx: &mut Context<Self>) {
        let changed =
            entry.path != self.entry.path || entry.modified != self.entry.modified || entry.size != self.entry.size;
        self.entry = entry;
        if changed {
            self.read(cx);
        }
    }

    /// The diff, once it has arrived.
    pub fn diff(&self) -> Option<&FileDiff> {
        match &self.load {
            Load::Ready { diff, .. } => Some(diff),
            _ => None,
        }
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = Load::Reading;
        let asked = self.source.diff(self.entry.path.clone());
        self._read = cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.load = match result {
                    Err(reason) => Load::Failed(reason.into()),
                    Ok(None) => Load::Outside,
                    Ok(Some(diff)) => {
                        let items = items_of(&diff);
                        Load::Ready { diff, items }
                    }
                };
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    /// Shows the last commit's version and this one side by side (`sideBySide`): the commit's is written to a
    /// temp copy, which is removed when the comparison closes.
    pub fn side_by_side(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.entry.path.clone();
        let asked = self.source.head_copy(path.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(asked).await.and_then(|inner| inner);
            this.update_in(cx, |this, window, cx| match result {
                Ok(Some(copy)) => {
                    let (left, right) = sides(&copy, &path);
                    let source = this.source.clone();
                    let discard = copy;
                    let on_close: OnClose = Box::new(move |_| source.head_discard(discard));
                    (this.open_sides)(window, cx, left, right, Some(on_close));
                }
                Ok(None) => {}
                Err(reason) => window.push_notification(
                    Notification::error(reason).title(t!("Couldn't read it from the last commit")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// The Side by side button.
    fn side_by_side_button(&self, cx: &mut Context<Self>) -> Button {
        let tip = t!("The last commit's version and this one side by side (by the code's structure, with Difftastic)");
        Button::new("changes-side-by-side")
            .ghost()
            .xsmall()
            .icon(IconName::Columns2)
            .label(t!("Side by side"))
            .tooltip(tip)
            .on_click(cx.listener(|this, _, window, cx| this.side_by_side(window, cx)))
    }

    /// One line of the list.
    fn row(&self, ix: usize, items: &[Item], t: &theme::Tokens, mono: &SharedString) -> AnyElement {
        let height = space(5.);
        match items.get(ix) {
            None => div().h(height).into_any_element(),
            Some(Item::Gap) => div()
                .id(("changes-gap", ix))
                .test_support()
                .h(height)
                .flex()
                .items_center()
                .justify_center()
                .border_y_1()
                .border_dashed()
                .border_color(t.border)
                .bg(t.muted.opacity(0.4))
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child("⋯")
                .into_any_element(),
            Some(Item::Cut(shown)) => div()
                .id(("changes-cut", ix))
                .role(Role::Status)
                .aria_label(t!(
                    "More changed than is shown here: the first {n} lines.",
                    n = words::count(u32::try_from(*shown).unwrap_or(u32::MAX))
                ))
                .test_support()
                .h(height)
                .flex()
                .items_center()
                .px(space(3.))
                .border_t_1()
                .border_color(t.border)
                .text_color(t.muted_foreground)
                .child(t!(
                    "More changed than is shown here: the first {n} lines.",
                    n = words::count(u32::try_from(*shown).unwrap_or(u32::MAX))
                ))
                .into_any_element(),
            Some(Item::Line { kind, old, new, text }) => {
                let (sign, tone, label) = match kind {
                    LineKind::Same => (" ", None, t!("Line {n}: {text}", n = new, text = text)),
                    LineKind::Removed => (
                        "−",
                        Some(Tone::Red),
                        t!("Removed line {n}: {text}", n = old, text = text),
                    ),
                    LineKind::Added => (
                        "+",
                        Some(Tone::Emerald),
                        t!("Added line {n}: {text}", n = new, text = text),
                    ),
                };
                let number = |n: u32| {
                    div()
                        .w(space(10.))
                        .flex_none()
                        .pe(1.5)
                        .text_end()
                        .text_color(t.muted_foreground.opacity(0.7))
                        .child(if n == 0 { String::new() } else { n.to_string() })
                };
                row_start()
                    .id(("changes-line", ix))
                    .role(Role::Row)
                    .aria_label(label)
                    .test_support()
                    .h(height)
                    .w_full()
                    .items_center()
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .when_some(tone, |this, tone| {
                        this.bg(tone.tint(0.1)).text_color(tone.line_text(t.dark))
                    })
                    .child(number(*old))
                    .child(number(*new))
                    .child(div().w(space(4.)).flex_none().text_center().child(sign))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pe(3.)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(if text.is_empty() {
                                SharedString::from(" ")
                            } else {
                                text.clone()
                            }),
                    )
                    .into_any_element()
            }
        }
    }
}

impl Render for ChangesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let name = self.entry.name.clone();
        let root = col()
            .id("changes-view")
            .size_full()
            .min_h_0()
            .text_start()
            .text_size(theme::size::SM);
        match &self.load {
            Load::Failed(reason) => root.child(
                centered(cx)
                    .id("changes-error")
                    .role(Role::Alert)
                    .aria_label(t!("Couldn't compare it with the last commit: {error}", error = reason))
                    .test_support()
                    .child(
                        icon(IconName::TriangleAlert)
                            .size(gpui_kit::px(40.))
                            .text_color(t.destructive),
                    )
                    .child(t!("Couldn't compare it with the last commit: {error}", error = reason)),
            ),
            Load::Reading => root.child(
                centered(cx)
                    .id("changes-reading")
                    .role(Role::Status)
                    .aria_label(t!("Reading…"))
                    .test_support()
                    .child(Spinner::new().color(t.primary)),
            ),
            Load::Outside => root.child(
                centered(cx)
                    .id("changes-outside")
                    .role(Role::Status)
                    .aria_label(t!("It isn't in a Git repository."))
                    .test_support()
                    .child(t!("It isn't in a Git repository.")),
            ),
            Load::Ready { diff, .. } if diff.binary || diff.too_big => {
                let why = if diff.binary {
                    t!("A binary file: what changed in it isn't shown as lines.")
                } else {
                    t!("Too big to compare here (over 1 MB).")
                };
                let sizes = if diff.new_file {
                    t!("New: {size}, not in the last commit", size = format_size(diff.new_size))
                } else {
                    t!(
                        "{old} in the last commit, {new} now",
                        old = format_size(diff.old_size),
                        new = format_size(diff.new_size)
                    )
                };
                let button = (!diff.new_file).then(|| self.side_by_side_button(cx));
                root.child(
                    centered(cx)
                        .id("changes-not-lines")
                        .role(Role::Group)
                        .aria_label(why.clone())
                        .test_support()
                        .child(
                            icon(IconName::FileDiff)
                                .size(gpui_kit::px(48.))
                                .text_color(t.muted_foreground),
                        )
                        .child(why)
                        .child(div().text_size(theme::size::XS).child(sizes))
                        .children(button),
                )
            }
            Load::Ready { diff, items } => {
                let summary = if diff.new_file {
                    row_start()
                        .items_center()
                        .gap(space(1.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(Tone::Emerald.count_text(t.dark))
                        .child(icon(IconName::FilePlus).size_3p5())
                        .child(t!("New: not in the last commit"))
                        .into_any_element()
                } else {
                    let text = t!(
                        "+{added} −{removed} since the last commit",
                        added = words::count(diff.added),
                        removed = words::count(diff.removed)
                    );
                    row_start()
                        .id("changes-summary")
                        .role(Role::Status)
                        .aria_label(text)
                        .test_support()
                        .items_center()
                        .gap(space(1.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(
                            div()
                                .text_color(Tone::Emerald.count_text(t.dark))
                                .child(format!("+{}", words::count(diff.added))),
                        )
                        .child(
                            div()
                                .text_color(Tone::Red.count_text(t.dark))
                                .child(format!("−{}", words::count(diff.removed))),
                        )
                        .child(t!("since the last commit"))
                        .into_any_element()
                };
                let path_tip = t!(
                    "{path}, compared in {took}",
                    path = diff.path.clone(),
                    took = format_duration(diff.elapsed)
                );
                let header = row_start()
                    .flex_none()
                    .items_center()
                    .gap(space(2.))
                    .px(space(3.))
                    .py(space(1.5))
                    .border_b_1()
                    .border_color(t.border)
                    .bg(t.card.opacity(0.95))
                    .text_size(theme::size::XS)
                    .child(summary)
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("changes-path")
                            .min_w_0()
                            .truncate()
                            .font_family(theme::mono_font(cx))
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .tooltip(move |window, cx| crate::kit::tooltip(path_tip.clone(), window, cx))
                            .child(diff.path.clone()),
                    )
                    .children((!diff.new_file).then(|| self.side_by_side_button(cx)));
                let body: AnyElement = if diff.lines.is_empty() {
                    let line_endings = diff.old_size != diff.new_size;
                    let text = if line_endings {
                        t!("Nothing changed in its lines since the last commit (its line endings did).")
                    } else {
                        t!("Nothing changed in its lines since the last commit.")
                    };
                    centered(cx)
                        .id("changes-nothing")
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .test_support()
                        .child(text)
                        .into_any_element()
                } else {
                    let mono = theme::mono_font(cx);
                    let total = items.len();
                    let items = items.clone();
                    let t_rows = t.clone();
                    div()
                        .id("changes-lines")
                        .role(Role::Table)
                        .aria_label(t!("Changes in {name}", name = name))
                        .test_support()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            uniform_list(
                                "changes-rows",
                                total,
                                cx.processor(move |this, range: Range<usize>, _, _| {
                                    range.map(|ix| this.row(ix, &items, &t_rows, &mono)).collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(&self.scroll)
                            .size_full(),
                        )
                        .child(Scrollbar::vertical(&self.scroll).viewport_from_layout())
                        .into_any_element()
                };
                root.child(header).child(body)
            }
        }
    }
}
