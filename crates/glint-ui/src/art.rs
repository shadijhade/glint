//! Glint's own pictures: a folder, a file's page, a place's chip, and the light behind the window.
//!
//! The icon set is strokes in one color. These are what the eye meets first (every row, every tile, every place),
//! so they are drawn for Glint: a shape filled with a gradient of its own color, its glyph (from the icon set) in
//! white on it. Each is written as an SVG, rasterized once at the exact number of pixels it covers on this screen,
//! and kept: after that a frame costs a lookup and one sprite, less than the outlined icon it replaces.
//!
//! | What                | Function    | Where                                                        |
//! |---------------------|-------------|--------------------------------------------------------------|
//! | a folder            | [`folder`]  | rows, tiles, tabs, the tree; a known folder carries its glyph |
//! | a file              | [`page`]    | rows and tiles: a page in its kind's color, the kind's glyph  |
//! | a place             | [`chip`]    | the rail, This PC, the palette: a rounded square, its glyph   |
//! | a kind of file      | [`kind`]    | [`folder`] or [`page`], whichever the kind is                 |
//! | the window's light  | [`aurora`]  | under everything: soft light in the place's color             |
//!
//! Sizes are logical pixels; [`set_scale`] says how many device pixels one is (the window does, each frame).

use crate::kit::IconName;
use crate::theme;
use glint_model::file_types::{Icon as KindIcon, Kind};
use gpui_kit::prelude::*;
use gpui_kit::{div, img, px, size, App, DevicePixels, Div, Hsla, ObjectFit, Pixels, RenderImage, Rgba, SvgSize};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::Arc;

/// Which drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Shape {
    Folder,
    Page,
    Chip,
}

/// One picture: the drawing, how many device pixels square, its color, its glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    shape: Shape,
    device: u16,
    rgb: [u8; 3],
    mark: Option<IconName>,
}

thread_local! {
    /// Device pixels in a logical one, as the window last said.
    static SCALE: Cell<f32> = const { Cell::new(1.) };
    /// The pictures made so far (None: one that couldn't be drawn, so it isn't tried every frame).
    static MADE: RefCell<HashMap<Key, Option<Arc<RenderImage>>>> = RefCell::new(HashMap::new());
    /// The icon set's glyphs as the inside of their SVGs.
    static GLYPHS: RefCell<HashMap<IconName, Option<Arc<str>>>> = RefCell::new(HashMap::new());
    /// The window's light, by color, theme and direction.
    static LIGHTS: RefCell<HashMap<(u16, bool, bool), Arc<RenderImage>>> = RefCell::new(HashMap::new());
}

/// More pictures than any session draws; past it the store starts over rather than grow without end.
const MOST_KEPT: usize = 2000;

/// The window says how many device pixels a logical one is, so pictures are drawn at the size they are shown.
pub fn set_scale(scale: f32) {
    if scale.is_finite() && scale > 0. {
        SCALE.with(|kept| kept.set(scale));
    }
}

fn byte(v: f32) -> u8 {
    // A color channel is 0..=1, so the product is 0..=255.
    (v.clamp(0., 1.) * 255.).round() as u8
}

fn rgb_of(color: Hsla) -> [u8; 3] {
    let c = Rgba::from(color);
    [byte(c.r), byte(c.g), byte(c.b)]
}

fn hex(color: Hsla) -> String {
    let [r, g, b] = rgb_of(color);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `color`, lighter by `lighter` (of 1) and turned `turn` degrees round the wheel.
fn shade(color: Hsla, lighter: f32, turn: f32) -> Hsla {
    Hsla {
        h: (color.h + turn / 360.).rem_euclid(1.),
        s: color.s,
        l: (color.l + lighter).clamp(0., 1.),
        a: 1.,
    }
}

/// A fill white reads on, in `color`'s hue: grays stay gray, and the light hues (amber, yellow) are taken down.
fn fill_of(color: Hsla) -> Hsla {
    if color.s < 0.18 {
        return Hsla {
            h: color.h,
            s: color.s.min(0.14),
            l: 0.5,
            a: 1.,
        };
    }
    let mut l = color.l.clamp(0.46, 0.56);
    if (0.10..0.20).contains(&color.h) {
        l = l.min(0.47);
    }
    Hsla {
        h: color.h,
        s: color.s.clamp(0.62, 0.92),
        l,
        a: 1.,
    }
}

/// The inside of an icon's SVG (its paths), for drawing it inside another picture. None if the set hasn't it.
fn glyph(mark: IconName, cx: &App) -> Option<Arc<str>> {
    if let Some(kept) = GLYPHS.with(|glyphs| glyphs.borrow().get(&mark).cloned()) {
        return kept;
    }
    let read = cx.asset_source().load(&mark.path()).ok().flatten().and_then(|bytes| {
        let svg = std::str::from_utf8(&bytes).ok()?;
        let open = svg.find("<svg")?;
        let start = open + svg[open..].find('>')? + 1;
        let end = svg.rfind("</svg>")?;
        (start <= end).then(|| Arc::<str>::from(svg[start..end].trim()))
    });
    GLYPHS.with(|glyphs| glyphs.borrow_mut().insert(mark, read.clone()));
    read
}

/// A glyph placed at `(x, y)`, `across` units square (the pictures are 48 units square), in white.
fn placed(inside: &str, x: f32, y: f32, across: f32, width: f32, opacity: f32) -> String {
    format!(
        "<g transform=\"translate({x} {y}) scale({:.4})\" fill=\"none\" stroke=\"#fff\" stroke-width=\"{width}\" \
         stroke-linecap=\"round\" stroke-linejoin=\"round\" stroke-opacity=\"{opacity}\">{inside}</g>",
        across / 24.
    )
}

/// A two-color gradient named `id`, from the top (leaning `lean` of the way across) to the bottom.
fn gradient(id: &str, lean: f32, from: Hsla, to: Hsla) -> String {
    format!(
        "<linearGradient id=\"{id}\" x1=\"0\" y1=\"0\" x2=\"{lean}\" y2=\"1\"><stop offset=\"0\" stop-color=\"{}\"/>\
         <stop offset=\"1\" stop-color=\"{}\"/></linearGradient>",
        hex(from),
        hex(to)
    )
}

fn folder_svg(px: u16, color: Hsla, mark: Option<&str>) -> String {
    let defs = gradient("b", 0., shade(color, -0.05, 4.), shade(color, -0.13, 8.))
        + &gradient("f", 0.35, shade(color, 0.17, -6.), shade(color, 0., 2.));
    // A known folder's glyph, pressed into the front.
    let mark = mark
        .map(|inside| placed(inside, 16.5, 20.5, 15., 2.3, 0.92))
        .unwrap_or_default();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{px}\" height=\"{px}\" viewBox=\"0 0 48 48\"><defs>{defs}</defs>\
         <path d=\"M4 12.5a4 4 0 0 1 4-4h9.1a4 4 0 0 1 3.1 1.5l2 2.5a2 2 0 0 0 1.6.8H40a4 4 0 0 1 4 4V36a4 4 0 0 1-4 \
         4H8a4 4 0 0 1-4-4Z\" fill=\"url(#b)\"/>\
         <rect x=\"4\" y=\"17\" width=\"40\" height=\"23.5\" rx=\"4.5\" fill=\"url(#f)\"/>\
         <path d=\"M8.5 17.7H39.5\" stroke=\"#fff\" stroke-opacity=\".55\" stroke-width=\"1.1\" stroke-linecap=\"round\"/>\
         {mark}</svg>"
    )
}

fn page_svg(px: u16, color: Hsla, mark: Option<&str>) -> String {
    let fill = fill_of(color);
    let defs = gradient("g", 0.6, shade(fill, 0.10, -8.), shade(fill, -0.05, 8.));
    let mark = mark
        .map(|inside| placed(inside, 14.5, 19.5, 19., 2.4, 1.))
        .unwrap_or_default();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{px}\" height=\"{px}\" viewBox=\"0 0 48 48\"><defs>{defs}</defs>\
         <path d=\"M13 3h15.5a3 3 0 0 1 2.1.9l8.5 8.5a3 3 0 0 1 .9 2.1V40a5 5 0 0 1-5 5H13a5 5 0 0 1-5-5V8a5 5 0 0 1 \
         5-5Z\" fill=\"url(#g)\"/>\
         <path d=\"M29.5 3.3v7.2a4 4 0 0 0 4 4h6.2a3 3 0 0 0-.7-1.1l-8.4-8.4a3 3 0 0 0-1.1-.7Z\" fill=\"#fff\" \
         fill-opacity=\".42\"/>{mark}</svg>"
    )
}

fn chip_svg(px: u16, color: Hsla, mark: Option<&str>) -> String {
    let fill = fill_of(color);
    let defs = gradient("g", 0.6, shade(fill, 0.10, -8.), shade(fill, -0.05, 8.));
    let mark = mark
        .map(|inside| placed(inside, 11., 11., 26., 2.2, 1.))
        .unwrap_or_default();
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{px}\" height=\"{px}\" viewBox=\"0 0 48 48\"><defs>{defs}</defs>\
         <rect x=\"3\" y=\"3\" width=\"42\" height=\"42\" rx=\"12\" fill=\"url(#g)\"/>\
         <path d=\"M14 4.1H34\" stroke=\"#fff\" stroke-opacity=\".35\" stroke-width=\"1\" stroke-linecap=\"round\"/>\
         {mark}</svg>"
    )
}

/// The picture for `key`, made on first use.
fn picture(key: Key, color: Hsla, cx: &App) -> Option<Arc<RenderImage>> {
    if let Some(kept) = MADE.with(|made| made.borrow().get(&key).cloned()) {
        return kept;
    }
    let inside = key.mark.and_then(|mark| glyph(mark, cx));
    let svg = match key.shape {
        Shape::Folder => folder_svg(key.device, color, inside.as_deref()),
        Shape::Page => page_svg(key.device, color, inside.as_deref()),
        Shape::Chip => chip_svg(key.device, color, inside.as_deref()),
    };
    let renderer = cx.svg_renderer();
    let side = DevicePixels(i32::from(key.device));
    let drawn = renderer
        .parse_svg(svg.as_bytes())
        .and_then(|parsed| renderer.render_parsed(&parsed, SvgSize::ExactSize(size(side, side))))
        .ok();
    MADE.with(|made| {
        let mut made = made.borrow_mut();
        if made.len() >= MOST_KEPT {
            made.clear();
        }
        made.insert(key, drawn.clone());
    });
    drawn
}

/// A square `across` wide holding the picture, or, if it couldn't be drawn, the icon set's `spare` in `color`.
fn shown(shape: Shape, across: Pixels, color: Hsla, mark: Option<IconName>, spare: IconName, cx: &App) -> Div {
    let scale = SCALE.with(Cell::get);
    // A picture is at least a few pixels and at most a large tile's worth: the cast is in range.
    let device = (f32::from(across) * scale).round().clamp(8., 1024.) as u16;
    let key = Key {
        shape,
        device,
        rgb: rgb_of(color),
        mark,
    };
    let square = div().flex_none().size(across);
    match picture(key, color, cx) {
        Some(image) => square.child(img(image).size(across).object_fit(ObjectFit::Fill)),
        None => square.child(crate::kit::icon(spare).size(across).text_color(color)),
    }
}

/// A folder, `across` wide, in `color` (the theme's folder color, or a known folder's own), with `mark` pressed
/// into its front when it is big enough to read.
pub fn folder(across: Pixels, color: Hsla, mark: Option<IconName>, cx: &App) -> Div {
    let mark = mark.filter(|_| across >= px(28.));
    shown(Shape::Folder, across, color, mark, IconName::Folder, cx)
}

/// A file: a page in `color` with the corner turned down and `mark` on it.
pub fn page(across: Pixels, color: Hsla, mark: Option<IconName>, cx: &App) -> Div {
    shown(Shape::Page, across, color, mark, IconName::File, cx)
}

/// A place: a rounded square in `color` with `mark` on it.
pub fn chip(across: Pixels, color: Hsla, mark: IconName, cx: &App) -> Div {
    shown(Shape::Chip, across, color, Some(mark), mark, cx)
}

/// What a kind of file carries on its page: the kind's own sign, without the page the icon set draws round it.
pub fn kind_mark(icon: KindIcon) -> Option<IconName> {
    Some(match icon {
        KindIcon::AppWindow => IconName::AppWindow,
        KindIcon::BookOpen => IconName::BookOpen,
        KindIcon::Database => IconName::Database,
        KindIcon::Disc => IconName::Disc,
        KindIcon::File | KindIcon::Folder => return None,
        KindIcon::FileArchive => IconName::Package,
        KindIcon::FileAudio => IconName::Music,
        KindIcon::FileCode => IconName::CodeXml,
        KindIcon::FileCog => IconName::Settings,
        KindIcon::FileImage => IconName::Image,
        KindIcon::FileJson => IconName::Braces,
        KindIcon::FileLock => IconName::KeyRound,
        KindIcon::FileSpreadsheet => IconName::Sheet,
        KindIcon::FileText => IconName::TextAlignStart,
        KindIcon::FileType => IconName::Type,
        KindIcon::FileVideo => IconName::Play,
        KindIcon::Link => IconName::Link,
        KindIcon::Presentation => IconName::Presentation,
        KindIcon::SquareTerminal => IconName::Terminal,
    })
}

/// A known folder's picture (Documents, Pictures…): the folder in the place's own color, its sign on it.
pub fn known_folder(id: crate::explorer::KnownId, across: Pixels, cx: &App) -> Div {
    let tone = theme::place_tone(theme::Place::Known(id), cx);
    folder(across, tone, Some(crate::kit::known_icon(id)), cx)
}

/// A kind of file's picture: the folder, or the kind's page.
pub fn kind(kind: &Kind, across: Pixels, cx: &App) -> Div {
    let tone = theme::kind_tone(kind, cx);
    if kind.icon == KindIcon::Folder {
        folder(across, tone.color, None, cx)
    } else {
        page(across, tone.color, kind_mark(kind.icon), cx)
    }
}

// ---- The light behind the window ---------------------------------------------------------------------------

/// The light's picture is this many pixels; the window stretches it over itself (it is all soft slopes).
const LIGHT_W: usize = 144;
const LIGHT_H: usize = 90;

/// The hue the lights lean toward as they go round the window (violet), and the one for a place that is violet
/// already (pink): a place's light is its own color at the start corner, sliding toward this at the others.
const TOWARD: f32 = 265. / 360.;
const TOWARD_FOR_VIOLET: f32 = 322. / 360.;

/// `from` moved `part` of the shorter way round the wheel to `to` (hues, 0..1).
fn toward(from: f32, to: f32, part: f32) -> f32 {
    let mut gap = (to - from).rem_euclid(1.);
    if gap > 0.5 {
        gap -= 1.;
    }
    (from + gap * part).rem_euclid(1.)
}

/// The four lights for a place whose color has `hue`: where (0..1 across and down; they sit at the corners, the
/// first at the start corner over the rail), how far they reach (of the width), their hue and their strength.
fn lights(hue: f32, dark: bool) -> [(f32, f32, f32, f32, f32); 4] {
    let far = if (toward(hue, TOWARD, 1.) - hue).abs() < 0.09 {
        TOWARD_FOR_VIOLET
    } else {
        TOWARD
    };
    let strong = if dark { 0.44 } else { 0.40 };
    [
        (0.04, -0.12, 0.42, hue, strong),
        (0.98, -0.22, 0.52, toward(hue, far, 0.5), strong * 0.62),
        (1.02, 1.18, 0.50, toward(hue, far, 0.95), strong * 0.52),
        (-0.08, 1.12, 0.42, toward(hue, far, 0.22), strong * 0.50),
    ]
}

/// The light behind the window for a place whose color has `hue` (0..1): four soft lights at the window's corners,
/// see-through between them. `rtl` mirrors it, so the place's own light stays over the rail.
pub fn aurora(hue: f32, dark: bool, rtl: bool) -> Arc<RenderImage> {
    // Hues a few degrees apart share a picture.
    let step = (hue.rem_euclid(1.) * 120.).round() as u16 % 120;
    if let Some(kept) = LIGHTS.with(|lights| lights.borrow().get(&(step, dark, rtl)).cloned()) {
        return kept;
    }
    let hue = f32::from(step) / 120.;
    let lit: Vec<(f32, f32, f32, [f32; 3], f32)> = lights(hue, dark)
        .into_iter()
        .map(|(x, y, reach, hue, strength)| {
            let c = Rgba::from(Hsla {
                h: hue,
                s: 0.95,
                l: if dark { 0.60 } else { 0.66 },
                a: 1.,
            });
            (x, y, reach, [c.r, c.g, c.b], strength)
        })
        .collect();
    // The window is wider than tall; distances are measured as if it were 16 by 10.
    let tall = 10. / 16.;
    let mut bgra = Vec::with_capacity(LIGHT_W * LIGHT_H * 4);
    for row in 0..LIGHT_H {
        let y = (row as f32 + 0.5) / LIGHT_H as f32;
        for col in 0..LIGHT_W {
            let across = (col as f32 + 0.5) / LIGHT_W as f32;
            let x = if rtl { 1. - across } else { across };
            let (mut sum, mut color, mut dim) = (0., [0.; 3], 1.);
            for (lx, ly, reach, rgb, strength) in &lit {
                let (dx, dy) = (x - lx, (y - ly) * tall);
                let far = (dx * dx + dy * dy) / (reach * reach);
                let weight = strength * (-2.6 * far).exp();
                sum += weight;
                dim *= 1. - weight;
                for (total, part) in color.iter_mut().zip(rgb) {
                    *total += part * weight;
                }
            }
            let alpha = 1. - dim;
            let channel = |i: usize| if sum > 0. { byte(color[i] / sum) } else { 0 };
            bgra.extend_from_slice(&[channel(2), channel(1), channel(0), byte(alpha)]);
        }
    }
    let image = image::RgbaImage::from_raw(LIGHT_W as u32, LIGHT_H as u32, bgra)
        .map(|buffer| Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
        .unwrap_or_else(|| {
            // The buffer is exactly width x height x 4 bytes, so this isn't reached; an empty picture if it were.
            Arc::new(RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(1, 1))]))
        });
    LIGHTS.with(|lights| lights.borrow_mut().insert((step, dark, rtl), image.clone()));
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::hsla;

    #[test]
    fn a_fill_is_one_white_reads_on() {
        // Amber-400 is too light for white; it is taken down. Slate stays gray. Blue stays blue.
        let amber = fill_of(hsla(43. / 360., 0.96, 0.56, 1.));
        assert!(amber.l <= 0.47 && amber.s > 0.6, "{amber:?}");
        let gray = fill_of(hsla(240. / 360., 0.05, 0.64, 1.));
        assert!(gray.s <= 0.14 && (gray.l - 0.5).abs() < 0.01, "{gray:?}");
        let blue = fill_of(hsla(217. / 360., 0.91, 0.60, 1.));
        assert!((blue.h - 217. / 360.).abs() < 0.001 && blue.l <= 0.56, "{blue:?}");
    }

    #[test]
    fn the_drawings_are_svg_with_their_colors_and_glyph() {
        let amber = hsla(38. / 360., 0.92, 0.5, 1.);
        let plain = folder_svg(20, amber, None);
        assert!(plain.starts_with("<svg") && plain.ends_with("</svg>") && plain.contains("width=\"20\""));
        assert!(!plain.contains("<g transform"));
        let marked = folder_svg(64, amber, Some("<path d=\"M1 1\"/>"));
        assert!(marked.contains("<g transform") && marked.contains("M1 1"));
        for svg in [
            page_svg(20, amber, Some("<circle/>")),
            chip_svg(22, amber, Some("<circle/>")),
        ] {
            assert!(svg.contains("url(#g)") && svg.contains("<circle/>"), "{svg}");
        }
    }

    #[test]
    fn hues_lean_the_shorter_way_round() {
        // Amber (38°) to violet (265°) is shorter back through red than on through green.
        let half = toward(38. / 360., TOWARD, 0.5) * 360.;
        assert!(!(60. ..300.).contains(&half), "{half}");
        // Sky (199°) goes on through blue.
        let half = toward(199. / 360., TOWARD, 0.5) * 360.;
        assert!((199. ..265.).contains(&half), "{half}");
    }

    #[test]
    fn the_light_is_strongest_at_its_corner_and_see_through_in_the_middle() {
        for (dark, rtl) in [(true, false), (false, false), (true, true)] {
            let image = aurora(0.9, dark, rtl);
            let bytes = image.as_bytes(0).expect("one frame");
            let alpha = |col: usize, row: usize| bytes[(row * LIGHT_W + col) * 4 + 3];
            let (start, end) = if rtl { (LIGHT_W - 1, 0) } else { (0, LIGHT_W - 1) };
            let middle = alpha(LIGHT_W / 2, LIGHT_H / 2);
            assert!(alpha(start, 0) > middle.saturating_add(40), "{dark} {rtl}");
            assert!(
                alpha(start, 0) > alpha(end, 0),
                "the place's own light is the strongest"
            );
            assert!(middle < 40, "{middle}");
        }
        // The same place asks for the same picture.
        assert!(Arc::ptr_eq(&aurora(0.9, true, false), &aurora(0.9, true, false)));
    }

    #[gpui_kit::test]
    fn a_picture_is_drawn_once_at_the_pixels_it_covers(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        cx.update(|cx| {
            let amber = hsla(38. / 360., 0.92, 0.5, 1.);
            for shape in [Shape::Folder, Shape::Page, Shape::Chip] {
                let key = Key {
                    shape,
                    device: 30,
                    rgb: rgb_of(amber),
                    mark: None,
                };
                let first = picture(key, amber, cx).expect("it draws");
                let size = first.size(0);
                assert_eq!((size.width.0, size.height.0), (30, 30), "{shape:?}");
                // The corners are clear and the middle is filled.
                let bytes = first.as_bytes(0).expect("one frame");
                assert_eq!(bytes[3], 0, "{shape:?}: the corner is see-through");
                assert_eq!(bytes[(15 * 30 + 15) * 4 + 3], 255, "{shape:?}: the middle is solid");
                let again = picture(key, amber, cx).expect("kept");
                assert!(Arc::ptr_eq(&first, &again), "{shape:?}: made once");
            }
        });
    }

    /// Writes the lights for a few places as pictures to look at: `GLINT_ART_DUMP=<folder> cargo test -p glint-ui
    /// --lib dump_the_lights -- --ignored`. Each is a binary PPM, the light over the chrome it is drawn on.
    #[test]
    #[ignore = "writes pictures for a person to look at"]
    fn dump_the_lights() {
        let Ok(folder) = std::env::var("GLINT_ART_DUMP") else {
            return;
        };
        for (name, hue) in [
            ("pink", 330.),
            ("blue", 217.),
            ("emerald", 160.),
            ("amber", 38.),
            ("violet", 258.),
        ] {
            for dark in [true, false] {
                let image = aurora(hue / 360., dark, false);
                let bytes = image.as_bytes(0).expect("one frame");
                let under: [f32; 3] = if dark { [18., 18., 20.] } else { [243., 243., 245.] };
                let mut out = format!("P6\n{LIGHT_W} {LIGHT_H}\n255\n").into_bytes();
                for pixel in bytes.as_chunks::<4>().0 {
                    let alpha = f32::from(pixel[3]) / 255.;
                    for channel in [2, 1, 0] {
                        let mixed = f32::from(pixel[channel]) * alpha + under[2 - channel] * (1. - alpha);
                        out.push(mixed.round().clamp(0., 255.) as u8);
                    }
                }
                let file = format!("{folder}/light-{name}-{}.ppm", if dark { "dark" } else { "light" });
                std::fs::write(file, out).expect("the folder exists");
            }
        }
    }

    #[test]
    fn every_kind_but_the_folder_and_the_plain_file_has_a_mark() {
        assert_eq!(kind_mark(KindIcon::Folder), None);
        assert_eq!(kind_mark(KindIcon::File), None);
        assert_eq!(kind_mark(KindIcon::FileImage), Some(IconName::Image));
        assert_eq!(kind_mark(KindIcon::FileVideo), Some(IconName::Play));
    }
}
