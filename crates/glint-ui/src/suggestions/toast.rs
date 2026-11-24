//! A suggestion's notification: its sign, what it says, its button (two where there is another way to say yes) and
//! a quiet "Don't suggest this again". Closing it, or letting it go by itself after [`SHOWN_FOR`], is "not now".
//!
//! The buttons are under the words, not beside them: a button at the end leaves the words a column a few words
//! wide, and a suggestion is read before it is pressed.

use super::{act, act_also, dismiss, Suggestion, SHOWN_FOR};
use crate::kit::prelude::*;
use crate::{t, theme};

/// The suggestion's notification, keyed by its kind.
struct SuggestionToast;

/// Shows `suggestion`, and takes it down again after [`SHOWN_FOR`] if nothing was done with it.
pub(super) fn push(suggestion: Suggestion, window: &mut Window, cx: &mut App) {
    let id = suggestion.kind.id();
    window.push_notification(notification(suggestion, cx), cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SHOWN_FOR).await;
        let _ = handle.update(cx, |_, window, cx| {
            window.remove_notification1::<SuggestionToast>(id, cx)
        });
    })
    .detach();
}

/// The notification for `suggestion`.
fn notification(suggestion: Suggestion, cx: &App) -> Notification {
    let kind = suggestion.kind;
    let id = kind.id();
    // Its own sign, in the text's quiet color: the main button is the one thing in the accent.
    let sign = icon(kind.icon()).text_color(theme::tokens(cx).muted_foreground);
    let shown = suggestion.clone();
    Notification::new()
        .icon(sign)
        .title(suggestion.title.clone())
        .message(suggestion.body.clone())
        .id1::<SuggestionToast>(id)
        .autohide(false)
        .content(move |_, _, cx| {
            let t = theme::tokens(cx);
            let (doing, looking) = (shown.clone(), shown.clone());
            let never = Button::new(SharedString::from(format!("suggestion-{id}-never")))
                .ghost()
                .xsmall()
                .label(t!("Don't suggest this again"))
                .text_color(t.muted_foreground)
                .on_click(move |_, window, cx| {
                    dismiss(cx, kind);
                    window.remove_notification1::<SuggestionToast>(id, cx);
                });
            let buttons = row_start()
                .items_center()
                .gap(space(2.))
                .child(
                    Button::new(SharedString::from(format!("suggestion-{id}")))
                        .primary()
                        .small()
                        .label(shown.button.clone())
                        .on_click(move |_, window, cx| {
                            window.remove_notification1::<SuggestionToast>(id, cx);
                            act(&doing, window, cx);
                        }),
                )
                .children(shown.also.clone().map(|label| {
                    Button::new(SharedString::from(format!("suggestion-{id}-also")))
                        .small()
                        .label(label)
                        .on_click(move |_, window, cx| {
                            window.remove_notification1::<SuggestionToast>(id, cx);
                            act_also(&looking, window, cx);
                        })
                }));
            // One button: the quiet no sits at the row's end. Two: it has a line of its own, so nothing is squeezed.
            match shown.also {
                None => buttons
                    .pt(space(2.5))
                    .child(div().flex_1())
                    .child(never)
                    .into_any_element(),
                Some(_) => col()
                    .pt(space(2.5))
                    .gap(space(1.))
                    .child(buttons)
                    .child(row_start().child(never))
                    .into_any_element(),
            }
        })
}

#[cfg(test)]
pub(super) fn is_shown(kind: super::Kind, window: &mut Window, cx: &mut App) -> bool {
    use gpui_kit::test::TestWindowExt as _;
    window.render_frame(cx);
    window
        .try_find(SharedString::from(format!("suggestion-{}", kind.id())))
        .is_some()
}
