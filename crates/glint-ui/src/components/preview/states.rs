//! The preview's states, as every kind of preview shows them: what is in the middle of the pane
//! (PreviewState.tsx's `Centered`, `BigIcon` and `Waiting`).

use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::entries::FileEntry;
use glint_model::file_types::kind_of;
use std::time::Duration;

/// A spinner shows only for what takes a moment (`useLater(300)`).
const SPINNER_AFTER: Duration = Duration::from_millis(300);

/// The middle of the pane: a column, centred, muted, `text-sm` (`Centered`). The caller adds the children.
pub fn centered(cx: &App) -> Div {
    let t = theme::tokens(cx);
    col()
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(3.))
        .p(space(6.))
        .text_center()
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
}

/// "This file is empty." in the middle of the pane.
pub fn empty(cx: &App) -> impl IntoElement {
    centered(cx)
        .id("preview-empty")
        .role(Role::Status)
        .aria_label(t!("This file is empty."))
        .test_support()
        .child(t!("This file is empty."))
}

/// A line of the pane's own words, in the middle of it, wrapped (`<p className="max-w-sm">`).
pub fn sentence(text: impl Into<SharedString>) -> Div {
    div().max_w(rems(24.)).min_w_0().child(text.into())
}

/// The file's kind, large, with why it isn't shown (or nothing to show) (`BigIcon`). `extra` goes below the note
/// (the password form).
pub fn big_icon(
    entry: &FileEntry,
    note: Option<SharedString>,
    extra: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let kind = kind_of(entry);
    let tone = theme::kind_tone(kind, cx);
    let label: SharedString = glint_model::file_types::type_label(entry).into();
    let mut body = centered(cx)
        .id("preview-icon")
        .role(Role::Group)
        .aria_label(label)
        .test_support()
        .child(
            icon(kind_icon(kind.icon))
                .size(px(64.))
                .flex_none()
                .text_color(tone.color),
        );
    if let Some(note) = note {
        body = body.child(
            sentence(note.clone())
                .id("preview-note")
                .role(Role::Status)
                .aria_label(note)
                .test_support(),
        );
    }
    if let Some(extra) = extra {
        body = body.child(extra);
    }
    body
}

/// One icon and a sentence or two, in the middle of the pane: a cloud for online-only, a lock for closed.
pub fn icon_and_text(
    name: IconName,
    color: Hsla,
    id: &'static str,
    text: SharedString,
    extra: Option<AnyElement>,
    cx: &App,
) -> impl IntoElement {
    let mut body = centered(cx)
        .id(id)
        .role(Role::Group)
        .aria_label(text.clone())
        .test_support()
        .child(icon(name).size(px(56.)).flex_none().text_color(color))
        .child(sentence(text));
    if let Some(extra) = extra {
        body = body.child(extra);
    }
    body
}

/// What is still being read: nothing at first, a spinner once it takes a moment (`Waiting`).
pub struct Waiting {
    what: Option<SharedString>,
    late: bool,
    _later: gpui_kit::Task<()>,
}

impl Waiting {
    /// Starts the clock; `what` is the line under the spinner ("Extracting from the archive…").
    pub fn new(what: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        let later = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SPINNER_AFTER).await;
            this.update(cx, |this, cx| {
                this.late = true;
                cx.notify();
            })
            .ok();
        });
        Waiting {
            what,
            late: false,
            _later: later,
        }
    }

    /// Whether the spinner shows yet.
    pub fn is_late(&self) -> bool {
        self.late
    }
}

impl Render for Waiting {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.late {
            return div().size_full().into_any_element();
        }
        let t = theme::tokens(cx).clone();
        let what = self.what.clone().unwrap_or_else(|| t!("Reading…"));
        centered(cx)
            .id("preview-waiting")
            .role(Role::Status)
            .aria_label(what.clone())
            .test_support()
            .child(Spinner::new().color(t.primary))
            .children(self.what.clone().map(sentence))
            .into_any_element()
    }
}
