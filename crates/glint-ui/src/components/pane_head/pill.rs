//! The pill: a small rounded button with the hairline border the kind chips have. The head's smart buttons (Map,
//! Colors, History) and the search bar's Save, Stop and Close are all drawn with it, so the two look like one family.

use crate::kit::prelude::*;
use crate::theme;

/// A pill button: `lit` is for what is on (a panel that is open, a search that is saved), in the accent's tint.
pub(crate) fn pill(id: impl Into<ElementId>, lit: bool, cx: &App) -> Button {
    let t = theme::tokens(cx);
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(if lit {
                    t.primary.opacity(0.16)
                } else {
                    t.accent.opacity(0.6)
                })
                .foreground(if lit { t.foreground } else { t.muted_foreground })
                .hover(if lit { t.primary.opacity(0.24) } else { t.pressed })
                .active(if lit { t.primary.opacity(0.24) } else { t.pressed }),
        )
        .compact()
        .rounded(px(999.))
        .border_1()
        .border_color(if lit { t.primary.opacity(0.5) } else { t.stroke })
        .flex_none()
}
