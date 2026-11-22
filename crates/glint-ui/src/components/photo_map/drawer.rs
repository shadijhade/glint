//! The cluster drawer: what a pin holds, as a strip of thumbnails with their names under them. Every tile is a
//! stop of the keyboard (Enter opens the photo, the arrows move along the strip) and has its name for a screen
//! reader; "Reveal" under it shows the photo in its folder.

use super::*;
use std::sync::Arc;

/// A tile's side, in CSS pixels. The thumbnail service is asked for this size too, so a pin's picture and a tile's
/// are the same decode.
pub(super) const TILE: f32 = 72.0;

/// One photo of the drawer, with its thumbnail once the service has it.
pub(super) struct Item {
    pub photo: GeoPhoto,
    pub picture: Option<Arc<RenderImage>>,
}

impl PhotoMapView {
    /// The drawer for a cluster: `title` says how many and near where, `country` is the place's country when it
    /// has one, and `items` are the first photos of the cluster.
    pub(super) fn drawer(
        &mut self,
        title: SharedString,
        country: Option<SharedString>,
        items: Vec<Item>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let pane = self.pane;
        while self.tile_focus.len() < items.len() {
            self.tile_focus.push(cx.focus_handle().tab_stop(true));
        }
        let last = items.len().saturating_sub(1);
        let tiles: Vec<AnyElement> = items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let handle = self.tile_focus[i].clone();
                let name: SharedString = item.photo.name.to_string().into();
                let reveal_path = item.photo.path.to_string();
                let (ring, muted) = (t.ring, t.muted);
                let picture = div()
                    .id(("pm-tile", pane * 10_000 + i))
                    .role(Role::Button)
                    .aria_label(name.clone())
                    .aria_description(t!("Opens the photo"))
                    .test_support()
                    .track_focus(&handle)
                    .focus(move |style| style.border_2().border_color(ring))
                    .size(px(TILE))
                    .flex_none()
                    .rounded(theme::size::RADIUS_SM)
                    .overflow_hidden()
                    .bg(muted)
                    .cursor_pointer()
                    .border_2()
                    .border_color(gpui_kit::transparent_black())
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        // Enter and Space press the tile (GPUI's click), the arrows move along the strip.
                        let go = match event.keystroke.key.as_str() {
                            "right" => (i + 1).min(last),
                            "left" => i.saturating_sub(1),
                            _ => return,
                        };
                        cx.stop_propagation();
                        if let Some(handle) = this.tile_focus.get(go) {
                            window.focus(handle, cx);
                        }
                    }))
                    .on_click({
                        let path = item.photo.path.to_string();
                        cx.listener(move |_, _: &ClickEvent, _, cx| {
                            cx.emit(PhotoMapEvent::Open { path: path.clone() });
                        })
                    })
                    .when_some(item.picture, |tile, image| {
                        tile.child(img(image).size_full().object_fit(ObjectFit::Cover))
                    });
                col()
                    .w(px(TILE))
                    .flex_none()
                    .gap(px(2.))
                    .child(picture)
                    .child(
                        div()
                            .w(px(TILE))
                            .truncate()
                            .text_xs()
                            .text_color(t.foreground)
                            .child(isolate(&name)),
                    )
                    .child(
                        Button::new(("pm-reveal", pane * 10_000 + i))
                            .compact()
                            .ghost()
                            .label(t!("Reveal"))
                            .accessibility_label(t!("Reveal {name} in its folder", name = name.clone()))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(PhotoMapEvent::Reveal {
                                    path: reveal_path.clone(),
                                });
                            })),
                    )
                    .into_any_element()
            })
            .collect();

        col()
            .id(("pm-drawer", pane))
            .role(Role::Group)
            .aria_label(title.clone())
            .test_support()
            .occlude()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .bg(t.background)
            .border_t_1()
            .border_color(t.border)
            .p(space(2.))
            .gap(space(1.))
            .child(
                row_start()
                    .items_center()
                    .justify_between()
                    .px(space(1.))
                    .child(
                        row_start()
                            .items_baseline()
                            .gap(space(2.))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.foreground)
                                    .child(title),
                            )
                            .when_some(country, |line, country| {
                                line.child(div().text_xs().text_color(t.muted_foreground).child(country))
                            }),
                    )
                    .child(
                        Button::new(("pm-drawer-close", pane))
                            .compact()
                            .ghost()
                            .icon(Icon::new(IconName::X))
                            .accessibility_label(t!("Close selection"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected_cluster = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .id(("pm-drawer-strip", pane))
                    .w_full()
                    .overflow_x_scroll()
                    .child(row_start().gap(space(2.)).children(tiles)),
            )
    }
}
