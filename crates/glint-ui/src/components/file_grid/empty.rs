//! What the grid says when a folder shows no tiles (the `empty` node Explorer.tsx builds for both views). Only the
//! states of a plain folder, the filter and the Recycle Bin are here; the search's, the Gallery's, Recent's and
//! the comparison's come with their parts.

use crate::kit::prelude::*;
use crate::{state, t, theme};

/// Why there is nothing to show.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Empty {
    /// The search box's text matches nothing here.
    NoMatch { filter: String, bin: bool },
    /// The Recycle Bin has nothing in it.
    BinHasNothing,
    /// Everything here is hidden, and hidden items aren't shown.
    OnlyHidden,
    /// Nothing in the folder (or archive).
    Folder { archive: bool },
}

impl Empty {
    /// The state for a listing that made no rows, from what the tab shows. `listed` is how many entries the
    /// listing has before hiding and filtering.
    pub fn of(filter: &str, bin: bool, listed: usize, archive: bool) -> Empty {
        if !filter.is_empty() {
            Empty::NoMatch {
                filter: filter.to_string(),
                bin,
            }
        } else if bin && listed == 0 {
            Empty::BinHasNothing
        } else if listed > 0 {
            Empty::OnlyHidden
        } else {
            Empty::Folder { archive }
        }
    }
}

/// The centred message (`Centered` in Explorer.tsx).
pub(super) fn view(empty: &Empty, cx: &App) -> impl IntoElement {
    let t = theme::tokens(cx);
    let strong = |text: SharedString| {
        div()
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.foreground)
            .child(text)
    };
    let note = |text: SharedString| div().text_size(theme::size::XS).child(text);
    let centered = col()
        .id("file-grid-empty")
        .role(Role::Status)
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(2.))
        .p(space(6.))
        .text_center()
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground);
    match empty {
        Empty::NoMatch { filter, bin } => {
            let place = if *bin { t!("the Recycle Bin") } else { t!("this folder") };
            centered
                .aria_label(t!(
                    "Nothing in {place} matches “{filter}”.",
                    place = place,
                    filter = filter
                ))
                .test_support()
                .child(icon(IconName::SearchX).size_6())
                .child(div().child(t!(
                    "Nothing in {place} matches “{filter}”.",
                    place = place,
                    filter = filter
                )))
            // PORT ME(find): the "Search subfolders too" button (`runFind(filter)`) when not in the Recycle Bin.
        }
        Empty::BinHasNothing => centered
            .aria_label(t!("The Recycle Bin is empty"))
            .test_support()
            .child(
                icon(IconName::Trash)
                    .size_8()
                    .text_color(t.muted_foreground.opacity(0.6)),
            )
            .child(strong(t!("The Recycle Bin is empty")))
            .child(note(t!(
                "What you delete waits here, to be restored or deleted for good."
            ))),
        Empty::OnlyHidden => centered
            .aria_label(t!("This folder only has hidden items."))
            .test_support()
            .child(div().child(t!("This folder only has hidden items.")))
            .child(
                Button::new("file-grid-show-hidden")
                    .outline()
                    .small()
                    .label(t!("Show hidden items"))
                    .on_click(|_, _, cx| state::update(cx, |settings| settings.show_hidden = true)),
            ),
        Empty::Folder { archive } => {
            let title = if *archive {
                t!("This archive is empty")
            } else {
                t!("This folder is empty")
            };
            centered
                .aria_label(title.clone())
                .test_support()
                .child(
                    icon(IconName::FolderOpen)
                        .size_8()
                        .text_color(t.muted_foreground.opacity(0.6)),
                )
                .child(strong(title))
                .when(!archive, |centered| {
                    centered.child(note(t!("Drop files here, or paste them with Ctrl+V.")))
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_follows_what_the_tab_shows() {
        assert_eq!(
            Empty::of("zz", false, 5, false),
            Empty::NoMatch {
                filter: "zz".into(),
                bin: false
            }
        );
        assert_eq!(Empty::of("", true, 0, false), Empty::BinHasNothing);
        assert_eq!(Empty::of("", false, 3, false), Empty::OnlyHidden);
        assert_eq!(Empty::of("", false, 0, true), Empty::Folder { archive: true });
        // A bin with things in it whose rows are all hidden or filtered isn't "empty".
        assert_eq!(Empty::of("", true, 2, false), Empty::OnlyHidden);
    }
}
