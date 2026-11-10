//! What a row draws for a Git mark: the pill after the name (`data-git` in FileList.tsx).
//!
//! The list and the grid call [`badge_element`] in their rows; [`super::words::row_git`]
//! says whether there is a pill and whether the row is faded.

use super::words::Badge;
use crate::kit::prelude::*;
use crate::{t, theme};

/// The small pill that says what Git thinks of an item ("Changed", "Staged", "New", "Conflict"), in the mark's
/// tone, with the longer sentence on hover. `id` tells the pill of one row from another's; `name` is the item's
/// name, so a screen reader hears "Changed: report.pdf" and each row's pill has a name of its own.
pub fn badge_element(id: impl Into<ElementId>, name: &str, badge: &Badge, cx: &App) -> impl IntoElement {
    let t = theme::tokens(cx);
    let tone = badge.tone;
    let title = badge.title.clone();
    div()
        .id(id)
        .role(Role::Group)
        .aria_label(t!("{what}: {name}", what = badge.label, name = name))
        .aria_description(badge.title.clone())
        .test_support()
        .flex_none()
        .px(space(1.5))
        .py(gpui_kit::px(1.))
        .rounded(theme::size::RADIUS_SM)
        .bg(tone.tint(0.15))
        .text_color(tone.badge_text(t.dark))
        .text_size(gpui_kit::rems(0.625))
        .line_height(gpui_kit::relative(1.25))
        .font_weight(FontWeight::MEDIUM)
        .tooltip(move |window, cx| crate::kit::tooltip(title.clone(), window, cx))
        .child(badge.label.clone())
}
