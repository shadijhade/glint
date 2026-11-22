//! The one header the 0.16 panels share (Time Machine, Photo map, Color wheel, the basket's drawer, the Copy queue):
//! a picture, the title, one short line under it that says what the panel holds now, the panel's own tools as icon
//! buttons, and a close button last.
//!
//! **Why one.** The panels were built apart and each drew its own: a text "Back to files" here, an icon there, a
//! subtitle that was a path in one and a count in another. Now closing always looks and reads the same, says its
//! key ("Close Time Machine (Esc)") and has the accessible name of what it closes, and the header's height does not
//! change when the subtitle comes or goes while data streams in.
//!
//! **Esc** is each panel's own key handler (a view with a focus handle and an `on_key_down`); this module only draws
//! what says so.

use crate::kit::prelude::*;
use crate::{t, theme};
use gpui_kit::{AnyElement, ClickEvent, ElementId};

/// How tall a header bar is at least, in spacing units: a title and a subtitle line fit, and a header with only a
/// title takes the same room, so nothing under it moves when the subtitle arrives (the subtitle line is always there).
const HEIGHT: f32 = 11.;

/// What a panel's header says. `tools` are the panel's own icon buttons (see [`tool_button`]); the close button is
/// added last, at the end edge.
pub struct Header {
    pub close_id: ElementId,
    pub glyph: IconName,
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    pub tools: Vec<AnyElement>,
    /// The close button's accessible name and tooltip when "Close <title>" isn't what the panel has always said.
    pub close_name: Option<SharedString>,
}

/// The tooltip of a panel's close button: what it closes and its key.
pub fn close_tip(title: &SharedString) -> SharedString {
    t!("Close {title} (Esc)", title = title.clone())
}

/// An icon-only button of a panel: ghost, compact, with its accessible name and a tooltip that says what it does
/// and its key ("Zoom in (+)").
pub fn tool_button(
    id: impl Into<ElementId>,
    glyph: IconName,
    name: SharedString,
    tip: SharedString,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(id)
        .compact()
        .ghost()
        .icon(Icon::new(glyph))
        .accessibility_label(name)
        .tooltip(tip)
        .on_click(on_click)
}

/// The header, drawn: a full-width bar with a bottom border in the panel's colors, for a panel that is a view of its
/// own (Time Machine, the map).
pub fn panel_header(
    header: Header,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    t: &theme::Tokens,
) -> gpui_kit::Div {
    header_row(header, on_close, t)
        .min_h(space(HEIGHT))
        .px(space(4.))
        .py(space(2.))
        .border_b_1()
        .border_color(t.stroke)
}

/// The header's row alone, with no bar around it: for a panel that is a card or a strip with its own padding
/// (the Color wheel, the basket's drawer, the Copy queue).
pub fn header_row(
    header: Header,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    t: &theme::Tokens,
) -> gpui_kit::Div {
    let Header {
        close_id,
        glyph,
        title,
        subtitle,
        tools,
        close_name,
    } = header;
    let tip = close_name.clone().unwrap_or_else(|| close_tip(&title));
    row_start()
        .w_full()
        .flex_none()
        .items_center()
        .gap(space(2.5))
        .text_start()
        .child(icon(glyph).size_4().flex_none().text_color(t.primary))
        .child(
            col()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(theme::size::SM)
                        .child(title.clone()),
                )
                .child(
                    div()
                        .h(space(4.))
                        .truncate()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .when_some(subtitle, |d, line| d.child(isolate(line))),
                ),
        )
        .child(row_start().flex_none().items_center().gap(space(1.)).children(tools))
        .child(tool_button(
            close_id,
            IconName::X,
            close_name.unwrap_or_else(|| t!("Close {title}", title = title)),
            tip,
            on_close,
        ))
}
