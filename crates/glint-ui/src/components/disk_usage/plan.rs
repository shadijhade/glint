//! The map as something to paint: the layout, the colors and the legend worked out once per map, size and
//! color mode, off the UI thread; and the painting of it onto a GPUI canvas. Ported from the `paint` and
//! `paintOverlay` functions of src/components/explorer/DiskUsage.tsx and the memos around them.
//!
//! **Nothing here runs per frame except the painting itself.** [`Plan::build`] lays the squarified treemap out
//! (`glint_model::usage::layout_usage`), gives every node its group and fill, adds up the legend and lists which
//! nodes are frames, tiles, smaller-items stripes and labels. The view builds it on a background thread when
//! the map, the size, the mode or the theme changes and keeps the old one on screen meanwhile. [`paint`] then
//! only walks those lists: quads for the frames and tiles, a cushion on each tile, and the labels, which are
//! shaped once per plan and cached ([`LabelCache`]).
//!
//! **Right to left.** The treemap mirrors with the interface: the biggest item sits at the top right in Arabic,
//! the labels start at their tile's right edge, and a point is mirrored before it is hit-tested.

use super::colors::{age_band, growth_band, ColorMode, MapColors, AGE_BANDS, GROWTH_ORDER};
use super::model::Shown;
use glint_model::file_types::{type_group, TypeGroup};
use glint_model::format::format_size;
use glint_model::usage::{hit_test, is_folder, layout_usage, Hit, UsageLayout, FRAME};
use gpui_kit::{
    fill, linear_color_stop, linear_gradient, pattern_slash, point, px, quad, size, App, BorderStyle, Bounds,
    ContentMask, Edges, FontWeight, Hsla, Pixels, Point, ShapedLine, SharedString, TextAlign, TextRun, Window,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// A frame's gap to the neighbouring tile, a tile's text padding, and the smallest tile that gets a label.
const GAP: f32 = 1.;
const PAD: f32 = 5.;
const LABEL_MIN_W: f32 = 44.;
const LABEL_MIN_H: f32 = 16.;
/// A tile this small in either direction gets no cushion.
const CUSHION_MIN: f32 = 6.;
/// The name and size in a frame's header need this much room for the size to follow the name.
const SIZE_ROOM: f32 = 30.;
/// The font sizes of the labels in pixels at the usual 16 px rem, as the TypeScript set them.
const BOLD_PX: f32 = 11.;
const SMALL_PX: f32 = 10.5;

/// What a plan is for: the same wanted plan is never built twice.
#[derive(Clone)]
pub struct Want {
    pub shown: Arc<Shown>,
    pub width: f32,
    pub height: f32,
    pub mode: ColorMode,
    pub colors: MapColors,
    /// When the map arrived (Unix ms): ages are counted from it, so they hold still while it is on screen.
    pub now_ms: f64,
    pub rtl: bool,
}

impl Want {
    /// Whether `other` asks for the same plan (the time ages count from doesn't matter: it holds for a map).
    pub fn same_as(&self, other: &Want) -> bool {
        self.shown.id == other.shown.id
            && self.width == other.width
            && self.height == other.height
            && self.mode == other.mode
            && self.colors == other.colors
            && self.rtl == other.rtl
    }

    /// Whether `plan` is what this asks for.
    pub fn is_met_by(&self, plan: &Plan) -> bool {
        plan.shown == self.shown.id
            && plan.width == self.width
            && plan.height == self.height
            && plan.mode == self.mode
            && plan.colors == self.colors
            && plan.rtl == self.rtl
    }
}

static NEXT_PLAN: AtomicU64 = AtomicU64::new(1);

/// A map laid out and colored for one size and mode.
pub struct Plan {
    /// Tells one plan from another (the labels are shaped again for a new one).
    pub seq: u64,
    /// The [`Shown::id`] it was made for.
    pub shown: u64,
    pub width: f32,
    pub height: f32,
    pub mode: ColorMode,
    pub colors: MapColors,
    pub rtl: bool,
    pub layout: UsageLayout,
    /// Each node's color group: files by type or age, folders none (unless growth colors everything).
    pub groups: Vec<Option<TypeGroup>>,
    /// Each node's fill.
    pub fills: Vec<Hsla>,
    /// Growth colors are on and the map can tell what grew (growth colors folders drawn whole too).
    pub growing: bool,
    /// The legend: bytes per kind (or age band, or growth band) among the files (tiles) shown.
    pub legend: Vec<(TypeGroup, f64)>,
    /// Some folder is drawn whole (too deep or too small to show inside), so the legend has "Folders".
    pub whole_folders: bool,
    /// Non-root frames, in drawing order.
    pub frames: Vec<u32>,
    /// Frames that have a room for the smaller items.
    pub rests: Vec<u32>,
    /// Tiles: files, and folders drawn whole.
    pub blocks: Vec<u32>,
    /// Nodes that get a name (and a size): frames with a header, tiles big enough.
    pub labels: Vec<u32>,
}

impl Plan {
    /// Lays out and colors the map (`layoutUsage`, `groups`, `fills`, `legend`, `wholeFolders`). The work is
    /// for a background thread.
    pub fn build(want: &Want) -> Plan {
        let shown = &want.shown;
        let map = &shown.map;
        let layout = layout_usage(map, f64::from(want.width), f64::from(want.height));
        let entries = &shown.entries;
        // Growth colors folders drawn whole too; without what grew (pdu, an archive, an older service), types do.
        let growing = want.mode == ColorMode::Growth && map.growth.is_some() && entries.len() == map.count;
        let groups: Vec<Option<TypeGroup>> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                if growing {
                    Some(growth_band(map, i))
                } else if e.dir {
                    None
                } else if want.mode == ColorMode::Age {
                    Some(age_band(e.modified, want.now_ms))
                } else {
                    Some(*type_group(e))
                }
            })
            .collect();
        let fills: Vec<Hsla> = entries
            .iter()
            .zip(&groups)
            .map(|(e, group)| match group {
                Some(g) if growing || want.mode == ColorMode::Age => want.colors.band(g),
                Some(g) => want.colors.tile(e, g),
                None => want.colors.folder,
            })
            .collect();

        let (mut frames, mut rests, mut blocks, mut labels) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for &node in &layout.order {
            let i = node as usize;
            if layout.kind.get(i) == Some(&FRAME) {
                if i != 0 {
                    frames.push(node);
                }
                if layout.rest_w.get(i).is_some_and(|&w| w > 0.) {
                    rests.push(node);
                }
                if layout.header.get(i).is_some_and(|&h| h > 0.) {
                    labels.push(node);
                }
            } else {
                blocks.push(node);
                let big = layout.w.get(i).is_some_and(|&w| w >= LABEL_MIN_W)
                    && layout.h.get(i).is_some_and(|&h| h >= LABEL_MIN_H);
                if big {
                    labels.push(node);
                }
            }
        }

        // Bytes per kind (or thermal age band) among the files shown, or per growth band among the tiles shown.
        let mut totals: Vec<(TypeGroup, f64)> = Vec::new();
        let mut add = |group: TypeGroup, bytes: f64| match totals.iter_mut().find(|(g, _)| *g == group) {
            Some((_, sum)) => *sum += bytes,
            None => totals.push((group, bytes)),
        };
        if growing {
            for &node in &blocks {
                let i = node as usize;
                if let Some(group) = groups.get(i).copied().flatten() {
                    add(group, map.size.get(i).copied().unwrap_or(0.));
                }
            }
        } else {
            for (i, group) in groups.iter().enumerate() {
                if let Some(group) = group {
                    add(*group, map.size.get(i).copied().unwrap_or(0.));
                }
            }
        }
        let legend = if growing {
            GROWTH_ORDER
                .iter()
                .filter_map(|band| totals.iter().find(|(g, _)| g == band).copied())
                .collect()
        } else if want.mode == ColorMode::Age {
            AGE_BANDS
                .iter()
                .filter_map(|band| totals.iter().find(|(g, _)| *g == band.group).copied())
                .collect()
        } else {
            // A stable sort, biggest first, the first six.
            totals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            totals.truncate(6);
            totals
        };
        let whole_folders = !growing && blocks.iter().any(|&node| is_folder(map, node as usize));

        Plan {
            seq: NEXT_PLAN.fetch_add(1, Ordering::Relaxed),
            shown: shown.id,
            width: want.width,
            height: want.height,
            mode: want.mode,
            colors: want.colors,
            rtl: want.rtl,
            layout,
            groups,
            fills,
            growing,
            legend,
            whole_folders,
            frames,
            rests,
            blocks,
            labels,
        }
    }

    /// What is under the point (`x`, `y`) in the map's own pixels (`hitTest`), mirrored in Arabic.
    pub fn hit(&self, shown: &Shown, x: f32, y: f32) -> Option<Hit> {
        let x = if self.rtl { self.width - x } else { x };
        hit_test(&shown.map, &self.layout, f64::from(x), f64::from(y))
    }

    /// The tiles a legend entry spotlights (`spotlight`): the tiles of that group, or for "Folders" the folders
    /// drawn whole. `name` is the group's name.
    pub fn spotlight(&self, shown: &Shown, name: &str) -> Vec<u32> {
        let mut set = Vec::new();
        for &node in &self.blocks {
            let i = node as usize;
            if i == 0 {
                continue;
            }
            let group = self.groups.get(i).copied().flatten();
            let on = if name == FOLDERS {
                group.is_none() && shown.entry(i).is_some_and(|e| e.dir)
            } else {
                group.is_some_and(|g| g.name == name)
            };
            if on {
                set.push(node);
            }
        }
        set
    }

    /// Where node `i` is, in the map's pixels: (x, y, width, height), mirrored in Arabic. Zero width for a node
    /// that isn't drawn.
    pub fn rect(&self, i: usize) -> (f32, f32, f32, f32) {
        let l = &self.layout;
        let (x, y, w, h) = (
            l.x.get(i).copied().unwrap_or(0.),
            l.y.get(i).copied().unwrap_or(0.),
            l.w.get(i).copied().unwrap_or(0.),
            l.h.get(i).copied().unwrap_or(0.),
        );
        (self.flip(x, w), y, w, h)
    }

    /// Where node `i`'s smaller items are, like [`Plan::rect`].
    pub fn rest_rect(&self, i: usize) -> (f32, f32, f32, f32) {
        let l = &self.layout;
        let (x, y, w, h) = (
            l.rest_x.get(i).copied().unwrap_or(0.),
            l.rest_y.get(i).copied().unwrap_or(0.),
            l.rest_w.get(i).copied().unwrap_or(0.),
            l.rest_h.get(i).copied().unwrap_or(0.),
        );
        (self.flip(x, w), y, w, h)
    }

    fn flip(&self, x: f32, w: f32) -> f32 {
        if self.rtl {
            self.width - x - w
        } else {
            x
        }
    }
}

/// The legend entry for the folders drawn whole, which has no group.
pub const FOLDERS: &str = "Folders";

/// A box in whole pixels with a one-pixel gap on the right and bottom, so neighbours stay apart (`box`). Tiles
/// under four pixels keep their size.
fn boxed(origin: Point<Pixels>, (x, y, w, h): (f32, f32, f32, f32)) -> Bounds<Pixels> {
    let (x0, y0) = (x.round(), y.round());
    let (bw, bh) = ((x + w).round() - x0, (y + h).round() - y0);
    let gap = if bw > 3. && bh > 3. { GAP } else { 0. };
    Bounds {
        origin: origin + point(px(x0), px(y0)),
        size: size(px((bw - gap).max(0.)), px((bh - gap).max(0.))),
    }
}

/// A line of the map's text, shaped and placed.
struct Placed {
    line: ShapedLine,
    /// Where the line's box starts, in the map's pixels.
    at: (f32, f32),
    /// The tile it must stay inside.
    clip: (f32, f32, f32, f32),
    line_height: Pixels,
    node: u32,
}

/// The labels of one plan, shaped for one text size and reading direction. Shaping costs, so it happens when the
/// plan or the text size changes, not per frame.
#[derive(Default)]
pub struct LabelCache {
    key: Option<(u64, u32, bool)>,
    placed: Vec<Placed>,
}

#[cfg(test)]
impl LabelCache {
    /// The nodes whose lines are shaped.
    pub fn nodes(&self) -> Vec<u32> {
        self.placed.iter().map(|p| p.node).collect()
    }
}

/// What is drawn over the map besides the tiles.
pub struct Overlay<'a> {
    /// Selected nodes: outlined in the selection color.
    pub selected: &'a [usize],
    /// The node under the pointer, and whether it is its frame's smaller items.
    pub hover: Option<(usize, bool)>,
    /// With a legend entry spotlit: its tiles, which stay lit while everything else dims.
    pub spotlight: Option<&'a [u32]>,
}

/// Cuts `text` to `max` pixels with an ellipsis (`fit`) and shapes it. `None` if nothing of it fits.
fn shape(
    window: &mut Window,
    cx: &App,
    text: &str,
    px_size: Pixels,
    weight: FontWeight,
    color: Hsla,
    max: f32,
) -> Option<ShapedLine> {
    if max <= 0. || text.is_empty() {
        return None;
    }
    let mut font = window.text_style().font();
    font.weight = weight;
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let (text, runs) = {
        let mut wrapper = cx.text_system().line_wrapper(font, px_size);
        wrapper.truncate_line(
            SharedString::from(text.to_string()),
            px(max),
            "…",
            std::slice::from_ref(&run),
            gpui_kit::TruncateFrom::End,
        )
    };
    if text.is_empty() {
        return None;
    }
    Some(window.text_system().shape_line(text, px_size, &runs, None))
}

/// Shapes and places the plan's labels (`paint`'s label pass). `rem` is the window's rem size in pixels, so the
/// text follows the text-size setting.
fn place_labels(plan: &Plan, shown: &Shown, window: &mut Window, cx: &App) -> Vec<Placed> {
    let rem = f32::from(window.rem_size()) / 16.;
    let (bold_px, small_px) = (px(BOLD_PX * rem), px(SMALL_PX * rem));
    let (bold_h, small_h) = (px(BOLD_PX * rem * 1.3), px(SMALL_PX * rem * 1.3));
    let map = &shown.map;
    let colors = &plan.colors;
    let mut placed = Vec::new();
    for &node in &plan.labels {
        let i = node as usize;
        let rect @ (x, y, w, _h) = plan.rect(i);
        let Some(name) = map.names.get(i) else { continue };
        let size_text = format_size(map.size.get(i).copied().unwrap_or(0.) as u64);
        let (start, direction) = if plan.rtl { (x + w - PAD, -1.) } else { (x + PAD, 1.) };
        let header = plan.layout.header.get(i).copied().unwrap_or(0.);
        if plan.layout.kind.get(i) == Some(&FRAME) {
            let mid = y + (header / 2.).round();
            let size_line = shape(
                window,
                cx,
                &size_text,
                small_px,
                FontWeight::NORMAL,
                colors.muted,
                f32::MAX / 4.,
            );
            let size_w = size_line.as_ref().map_or(0., |l| f32::from(l.width()));
            // The size goes after the name when both fit; otherwise the name alone.
            let with_size = w - 3. * PAD - size_w;
            let room = if with_size >= SIZE_ROOM {
                with_size
            } else {
                w - 2. * PAD
            };
            let Some(name_line) = shape(window, cx, name, bold_px, FontWeight::SEMIBOLD, colors.text, room) else {
                continue;
            };
            let name_w = f32::from(name_line.width());
            let name_x = if plan.rtl { start - name_w } else { start };
            placed.push(Placed {
                at: (name_x, mid - f32::from(bold_h) / 2.),
                clip: rect,
                line_height: bold_h,
                line: name_line,
                node,
            });
            if with_size >= SIZE_ROOM {
                if let Some(size_line) = size_line {
                    let after = start + direction * (name_w + PAD);
                    let size_x = if plan.rtl { after - size_w } else { after };
                    placed.push(Placed {
                        at: (size_x, mid - f32::from(small_h) / 2.),
                        clip: rect,
                        line_height: small_h,
                        line: size_line,
                        node,
                    });
                }
            }
        } else {
            let top = y + 9. * rem;
            let room = w - 2. * PAD;
            if let Some(name_line) = shape(window, cx, name, bold_px, FontWeight::SEMIBOLD, colors.label, room) {
                let name_w = f32::from(name_line.width());
                let name_x = if plan.rtl { start - name_w } else { start };
                placed.push(Placed {
                    at: (name_x, top - f32::from(bold_h) / 2.),
                    clip: rect,
                    line_height: bold_h,
                    line: name_line,
                    node,
                });
            }
            if plan.layout.h.get(i).copied().unwrap_or(0.) >= 30. {
                if let Some(size_line) = shape(
                    window,
                    cx,
                    &size_text,
                    small_px,
                    FontWeight::NORMAL,
                    colors.label_muted,
                    room,
                ) {
                    let size_w = f32::from(size_line.width());
                    let size_x = if plan.rtl { start - size_w } else { start };
                    placed.push(Placed {
                        at: (size_x, top + 13. * rem - f32::from(small_h) / 2.),
                        clip: rect,
                        line_height: small_h,
                        line: size_line,
                        node,
                    });
                }
            }
        }
    }
    placed
}

/// Paints one tile: its fill and the cushion on it, lit from the top start corner.
fn paint_tile(plan: &Plan, origin: Point<Pixels>, node: u32, window: &mut Window) {
    let i = node as usize;
    let Some(&fill_color) = plan.fills.get(i) else { return };
    let bounds = boxed(origin, plan.rect(i));
    window.paint_quad(fill(bounds, fill_color));
    if f32::from(bounds.size.width) >= CUSHION_MIN && f32::from(bounds.size.height) >= CUSHION_MIN {
        // Light from the top start corner to shade at the bottom end, whichever way the map reads.
        let angle = if plan.rtl { 225. } else { 135. };
        window.paint_quad(fill(
            bounds,
            linear_gradient(
                angle,
                linear_color_stop(plan.colors.shine, 0.),
                linear_color_stop(plan.colors.shade, 1.),
            ),
        ));
    }
}

/// Paints one label.
fn paint_placed(placed: &Placed, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let clip = Bounds {
        origin: origin + point(px(placed.clip.0), px(placed.clip.1)),
        size: size(px(placed.clip.2), px(placed.clip.3)),
    };
    let at = origin + point(px(placed.at.0), px(placed.at.1));
    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        // A line that can't be painted (a glyph missing) is left out; the map doesn't need it.
        placed
            .line
            .paint(at, placed.line_height, TextAlign::Left, None, window, cx)
            .ok();
    });
}

/// A rectangle's outline `width` pixels wide, drawn inside it.
fn outline(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla, width: f32) {
    window.paint_quad(quad(
        bounds,
        0.,
        gpui_kit::transparent_black(),
        Edges::all(px(width)),
        color,
        BorderStyle::Solid,
    ));
}

/// Paints the whole map into `bounds`: frames, the smaller-items stripes, tiles with their cushions, labels, then
/// the spotlight, the selection and the hover (`paint`, `paintOverlay`).
pub fn paint(
    plan: &Plan,
    shown: &Shown,
    overlay: &Overlay,
    cache: &mut LabelCache,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = bounds.origin;
    let colors = &plan.colors;
    window.paint_quad(fill(bounds, colors.background));
    for &node in &plan.frames {
        let i = node as usize;
        let depth = plan.layout.depth.get(i).copied().unwrap_or(1) as usize;
        let shade = colors.frames[depth.clamp(1, colors.frames.len()) - 1];
        window.paint_quad(fill(boxed(origin, plan.rect(i)), shade));
    }
    for &node in &plan.rests {
        let rest = boxed(origin, plan.rest_rect(node as usize));
        window.paint_quad(fill(rest, colors.rest));
        window.paint_quad(fill(rest, pattern_slash(colors.stripe, 1., 6.)));
    }
    for &node in &plan.blocks {
        paint_tile(plan, origin, node, window);
    }

    // Labels are shaped on the first paint of a plan, and again if the text size changes.
    let rem = window.rem_size();
    let key = (plan.seq, f32::from(rem).to_bits(), plan.rtl);
    if cache.key != Some(key) {
        cache.placed = place_labels(plan, shown, window, cx);
        cache.key = Some(key);
    }
    for placed in &cache.placed {
        paint_placed(placed, origin, window, cx);
    }

    // A legend entry spotlit: everything else dims, and the entry's tiles are drawn again over the dimming.
    if let Some(spotlight) = overlay.spotlight {
        window.paint_quad(fill(bounds, colors.dim));
        for &node in spotlight {
            if plan.layout.w.get(node as usize).is_some_and(|&w| w > 0.) {
                paint_tile(plan, origin, node, window);
            }
        }
        for placed in cache.placed.iter().filter(|p| spotlight.contains(&p.node)) {
            paint_placed(placed, origin, window, cx);
        }
        for &node in spotlight {
            if plan.layout.w.get(node as usize).is_some_and(|&w| w > 0.) {
                outline(window, boxed(origin, plan.rect(node as usize)), colors.hover, 1.);
            }
        }
    }
    for &i in overlay.selected {
        if plan.layout.w.get(i).is_some_and(|&w| w > 0.) {
            outline(window, boxed(origin, plan.rect(i)), colors.selected, 2.);
        }
    }
    if let Some((i, rest)) = overlay.hover.filter(|&(i, rest)| i != 0 || rest) {
        let rect = if rest { plan.rest_rect(i) } else { plan.rect(i) };
        let bounds = boxed(origin, rect);
        window.paint_quad(fill(bounds, colors.hover_fill));
        outline(window, bounds, colors.hover, 1.5);
    }
}
