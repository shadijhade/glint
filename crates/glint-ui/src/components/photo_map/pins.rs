//! Painting the map's pins, with a picture in them where the thumbnail service has one. The canvas's paint closure
//! calls these once per frame; they allocate nothing but what the text system shapes.

use crate::kit::prelude::*;
use glint_model::world_map::PinCluster;
use std::sync::Arc;

/// The colors a frame's painting uses, taken from the theme once.
#[derive(Clone, Copy)]
pub(super) struct Colors {
    pub pin: Hsla,
    pub selected: Hsla,
    pub text: Hsla,
    pub halo: Hsla,
}

/// What painting a pin or a name needs besides the pin: the map's corner on the window, the colors, the font.
#[derive(Clone, Copy)]
pub(super) struct Ink<'a> {
    pub origin: Point<Pixels>,
    pub colors: Colors,
    pub font: &'a Font,
}

/// The picture a pin shows: its lead photo's thumbnail, and its natural size in CSS pixels.
#[derive(Clone)]
pub(super) struct PinPicture {
    /// Which pin of the frame it is for.
    pub pin: u32,
    pub image: Arc<RenderImage>,
    pub width: f32,
    pub height: f32,
}

/// The radius of a pin with a picture in it, for one photo and for a cluster.
const SINGLE_R: f32 = 15.0;
const CLUSTER_R: f32 = 18.0;

/// How far from its center a click still picks a pin: a pin with a picture is a bigger target than a dot.
pub(super) fn hit_radius(count: u32, picture: bool) -> f32 {
    match (count > 1, picture) {
        (true, true) => CLUSTER_R + 3.0,
        (false, true) => SINGLE_R + 3.0,
        (true, false) => 20.0,
        (false, false) => 14.0,
    }
}

/// Paints `text` centered on `(x, y)` in the given size and weight.
fn paint_centered(window: &mut Window, cx: &mut App, text: &str, at: (f32, f32), size: f32, color: Hsla, font: &Font) {
    let run = TextRun {
        len: text.len(),
        font: Font {
            weight: FontWeight::SEMIBOLD,
            ..font.clone()
        },
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(SharedString::from(text.to_string()), px(size), &[run], None);
    let width = f32::from(shaped.width());
    let origin = point(px(at.0 - width * 0.5), px(at.1 - size * 0.65));
    let _ = shaped.paint(origin, px(size * 1.3), TextAlign::Left, None, window, cx);
}

/// A filled circle of `radius` at `(x, y)`.
fn disc(window: &mut Window, x: f32, y: f32, radius: f32, fill_color: Hsla, ring: Hsla, ring_px: f32) {
    let bounds = Bounds::new(
        point(px(x - radius), px(y - radius)),
        size(px(radius * 2.0), px(radius * 2.0)),
    );
    window.paint_quad(quad(
        bounds,
        radius,
        fill_color,
        Edges::all(px(ring_px)),
        ring,
        BorderStyle::Solid,
    ));
}

/// The count a badge shows: 1,200 photos are "1k".
fn count_text(count: u32) -> String {
    if count >= 1_000 {
        format!("{}k", count / 1_000)
    } else {
        count.to_string()
    }
}

/// One pin: its lead photo's thumbnail in a circle with a count badge when it has the picture, else a dot (a
/// bigger one with the count in it for a cluster). `origin` is the map's top-left corner on the window.
pub(super) fn paint_pin(
    window: &mut Window,
    cx: &mut App,
    ink: Ink,
    pin: &PinCluster,
    selected: bool,
    picture: Option<&PinPicture>,
) {
    let Ink { origin, colors, font } = ink;
    let x = f32::from(origin.x) + pin.sx;
    let y = f32::from(origin.y) + pin.sy;
    let ring = if selected { colors.selected } else { colors.halo };
    match picture {
        Some(pic) => {
            let r = if pin.count > 1 { CLUSTER_R } else { SINGLE_R };
            // The ring first: the photo sits on it, so it shows all round.
            disc(window, x, y, r + 2.0, ring, ring, 0.0);
            let side = r * 2.0;
            let frame = Bounds::new(point(px(x - r), px(y - r)), size(px(side), px(side)));
            // The picture covers the circle: scaled until both sides reach it, centered, cropped by the corners.
            let cover = (side / pic.width.max(1.0)).max(side / pic.height.max(1.0));
            let (w, h) = (pic.width * cover, pic.height * cover);
            let image = Bounds::new(point(px(x - w * 0.5), px(y - h * 0.5)), size(px(w), px(h)));
            let _ = window.paint_image(frame, image, Corners::all(px(r)), pic.image.clone(), 0, false);
            if pin.count > 1 {
                let (bx, by) = (x + r * 0.72, y - r * 0.72);
                disc(window, bx, by, 8.5, colors.selected_or(selected), colors.halo, 1.5);
                paint_centered(window, cx, &count_text(pin.count), (bx, by), 10.0, colors.text, font);
            }
        }
        None => {
            let radius = if pin.count > 1 { 12.0 } else { 6.5 };
            let fill_color = if selected { colors.selected } else { colors.pin };
            disc(window, x, y, radius, fill_color, colors.halo, 1.5);
            if pin.count > 1 {
                paint_centered(window, cx, &count_text(pin.count), (x, y), 10.0, colors.text, font);
            }
        }
    }
}

impl Colors {
    /// The badge's color: the pin's, or the selection's.
    fn selected_or(&self, selected: bool) -> Hsla {
        if selected {
            self.selected
        } else {
            self.pin
        }
    }
}
