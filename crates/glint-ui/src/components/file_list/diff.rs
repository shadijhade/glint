//! What a row draws for compare panes: the small word after the name ("Only here", "Newer", "Different",
//! "Older") and the dimming of the items that are the same (`DIFF_BADGE` in FileList.tsx; the grid's tiles use the
//! same words and colors, in a corner of the thumbnail).

use crate::components::git::Tone;
use crate::kit::prelude::*;
use crate::t;
use crate::theme::{self, Tokens};
use glint_model::entries::DiffStatus;

/// How much an item that is the same in both panes is dimmed (`opacity-60`), unless it is selected.
pub const SAME_OPACITY: f32 = 0.6;

/// What the badge says of an item and how it looks.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffBadge {
    pub label: SharedString,
    /// The hover text.
    pub title: SharedString,
    /// `None` is the muted gray of "Older".
    pub tone: Option<Tone>,
}

/// The badge of a status; the same items have none (`DIFF_BADGE` has no entry for "same").
pub fn diff_badge(status: DiffStatus) -> Option<DiffBadge> {
    let (label, title, tone) = match status {
        DiffStatus::Unique => (t!("Only here"), t!("Does not exist in the other pane"), Some(Tone::Emerald)),
        DiffStatus::Newer => (
            t!("Newer"),
            t!("Modified more recently than in the other pane"),
            Some(Tone::Sky),
        ),
        DiffStatus::Differ => (
            t!("Different"),
            t!("Not the same as in the other pane: another size, or compared deeper, other contents or something different inside"),
            Some(Tone::Amber),
        ),
        DiffStatus::Older => (t!("Older"), t!("Modified earlier than in the other pane"), None),
        DiffStatus::Same => return None,
    };
    Some(DiffBadge { label, title, tone })
}

/// The tint behind the badge and the color of its word (`bg-emerald-500/15 text-emerald-700 dark:text-emerald-300`;
/// "Older" is `bg-muted text-muted-foreground`).
pub fn colors(badge: &DiffBadge, t: &Tokens) -> (Hsla, Hsla) {
    match badge.tone {
        Some(tone) => (tone.tint(0.15), tone.badge_text(t.dark)),
        None => (t.muted, t.muted_foreground),
    }
}

/// The pill after a row's name. `id` tells one row's from another's; `name` is the item's name, so a screen reader
/// hears "Newer: report.pdf".
pub fn badge_element(id: impl Into<ElementId>, name: &str, badge: &DiffBadge, cx: &App) -> impl IntoElement {
    let t = theme::tokens(cx);
    let (background, text) = colors(badge, t);
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
        .bg(background)
        .text_color(text)
        .text_size(gpui_kit::rems(0.625))
        .line_height(gpui_kit::relative(1.25))
        .font_weight(FontWeight::MEDIUM)
        .tooltip(move |window, cx| crate::kit::tooltip(title.clone(), window, cx))
        .child(badge.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_has_no_badge_and_the_others_are_worded_as_the_web_app_worded_them() {
        assert!(diff_badge(DiffStatus::Same).is_none());
        let words = |s| diff_badge(s).map(|b| b.label.to_string());
        assert_eq!(words(DiffStatus::Unique).as_deref(), Some("Only here"));
        assert_eq!(words(DiffStatus::Newer).as_deref(), Some("Newer"));
        assert_eq!(words(DiffStatus::Differ).as_deref(), Some("Different"));
        assert_eq!(words(DiffStatus::Older).as_deref(), Some("Older"));
        assert_eq!(
            diff_badge(DiffStatus::Older).map(|b| b.tone),
            Some(None),
            "older is gray"
        );
    }
}
