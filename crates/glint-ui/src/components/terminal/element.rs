//! The element that paints a terminal's screen.
//!
//! xterm.js drew its own canvas; this is the GPUI equivalent. It owns no state: it reads a
//! [`Snapshot`](super::emu::Snapshot) of the [`Screen`], paints it, and tells the view what it measured.
//!
//! **What it paints**, in order: the background; each row's fills (cell backgrounds); the selection; each row's
//! runs of text, shaped with the mono font and `force_width` set to the cell width so every glyph sits on the
//! grid whatever font it falls back to (a wide character gets two cells); links under the pointer; the cursor
//! (a block draws its character again in the accent color over it); an input method's composition; the
//! scrollbar. Cells are grouped in runs by the snapshot, so a screenful is a few dozen shaped lines, and GPUI
//! caches the shaping between frames.
//!
//! **What it measures.** In a live terminal ([`Fit::Fill`]) the grid is as many cells as fit the bounds: the
//! element resizes the screen right here in prepaint (the parse lock is held for the reflow only) and reports
//! the new size, so the view tells the shell. In a recording ([`Fit::Fixed`]) the grid is the recording's and
//! the font is chosen to fit the width (10 to 18 px, as CastPreview.tsx did).
//!
//! **Never notifies.** Nothing here calls `cx.notify`: GPUI drops a redraw asked for during a draw. What the
//! view needs to know (the grid size) goes through a channel its task reads.
//!
//! **Right to left.** A terminal grid is one letter a cell; the shaping still joins and reorders Arabic inside
//! a run, which may not match what the program meant. That is a limit of terminals, not of this element: the
//! pane says so when right-to-left text is on screen. The scrollbar sits at the reading direction's end edge.

use super::emu::{Cursor, Palette, Screen, Snapshot, Underline};
use super::view::TerminalView;
use crate::i18n;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui_kit::{
    fill, point, px, quad, relative, rgb, size, AnyElement, App, AvailableSpace, BorderStyle, Bounds, ContentMask,
    CursorStyle, DispatchPhase, Edges, Element, ElementId, ElementInputHandler, Entity, FocusHandle, Font, FontStyle,
    FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollEvent, SharedString, Size, StrikethroughStyle,
    Style, TextAlign, TextRun, UnderlineStyle, Window,
};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// The text's padding inside the pane (`px-2 pt-1`).
pub const PAD_X: f32 = 8.;
pub const PAD_Y: f32 = 4.;
/// The scrollbar's width and the room its track needs at the end edge.
pub const SCROLLBAR: f32 = 8.;

/// How the grid is sized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// As many cells as fill the bounds (a live terminal).
    Fill,
    /// This many columns and rows, drawn as big as fits the width (a recording).
    Fixed { cols: usize, rows: usize },
}

/// Where the grid was drawn last: what the view needs to turn the pointer into a cell.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub bounds: Bounds<Pixels>,
    /// The top-left corner of the first cell.
    pub origin: Point<Pixels>,
    pub cell: Size<Pixels>,
    pub cols: usize,
    pub rows: usize,
    /// The scrollbar's track and thumb, when there is scrollback.
    pub track: Option<Bounds<Pixels>>,
    pub thumb: Option<Bounds<Pixels>>,
}

impl Geometry {
    /// The cell under `at` (clamped to the grid) and whether `at` is in its second half.
    pub fn cell_at(&self, at: Point<Pixels>) -> (usize, usize, bool) {
        let x = f32::from(at.x - self.origin.x);
        let y = f32::from(at.y - self.origin.y);
        let (cw, ch) = (f32::from(self.cell.width).max(1.), f32::from(self.cell.height).max(1.));
        let col = (x / cw).floor().max(0.) as usize;
        let row = (y / ch).floor().max(0.) as usize;
        // Past the last column the pointer is at the end of it; before the first, at its start.
        let right = (x / cw).fract() >= 0.5 || x / cw >= self.cols as f32;
        (
            col.min(self.cols.saturating_sub(1)),
            row.min(self.rows.saturating_sub(1)),
            right,
        )
    }

    /// The pixel box of a cell (a wide one: `cells` = 2).
    pub fn cell_bounds(&self, col: usize, row: usize, cells: usize) -> Bounds<Pixels> {
        Bounds {
            origin: point(
                self.origin.x + self.cell.width * col as f32,
                self.origin.y + self.cell.height * row as f32,
            ),
            size: size(self.cell.width * cells as f32, self.cell.height),
        }
    }
}

/// A cell's size for a font.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub cell: Size<Pixels>,
    pub font_size: Pixels,
}

/// The mono font in a weight and slant.
pub fn font_of(family: &SharedString, bold: bool, italic: bool) -> Font {
    Font {
        family: family.clone(),
        features: Default::default(),
        fallbacks: None,
        weight: if bold { FontWeight::BOLD } else { FontWeight::NORMAL },
        style: if italic { FontStyle::Italic } else { FontStyle::Normal },
    }
}

/// How big a cell of `family` at `font_size` is: the advance of "m" and the font's height, both on whole
/// device pixels so rows and columns meet without seams at any scale.
pub fn measure(window: &Window, cx: &App, family: &SharedString, font_size: Pixels) -> Metrics {
    let system = cx.text_system();
    let font = font_of(family, false, false);
    let id = system.resolve_font(&font);
    let advance = system.advance(id, font_size, 'm').map_or(font_size * 0.6, |s| s.width);
    let height = system.ascent(id, font_size) + system.descent(id, font_size);
    let scale = window.scale_factor().max(1.);
    let device = |p: Pixels, round: bool| {
        let v = f32::from(p) * scale;
        px((if round { v.round() } else { v.ceil() }).max(1.) / scale)
    };
    Metrics {
        cell: size(device(advance, true), device(height, false)),
        font_size,
    }
}

/// The font size a recording `cols` wide is drawn at in `available` pixels of width: as big as fits, from 10 to
/// 18 px (a narrower pane scrolls sideways rather than go smaller).
pub fn fitted_font_size(window: &Window, cx: &App, family: &SharedString, cols: usize, available: Pixels) -> Pixels {
    let probe = measure(window, cx, family, px(14.));
    let drawn = f32::from(probe.cell.width) * cols.max(1) as f32 + 2. * PAD_X;
    let fits = (14. * f32::from(available) / drawn.max(1.)).floor();
    px(fits.clamp(10., 18.))
}

fn color(value: u32) -> Hsla {
    rgb(value).into()
}

/// Everything the element needs from its view for one frame.
pub struct TerminalElement {
    pub screen: Screen,
    pub palette: Palette,
    pub palette_id: u64,
    pub family: SharedString,
    /// The font size in rems (13 px at the usual size).
    pub rems: f32,
    pub fit: Fit,
    pub geometry: Rc<Cell<Option<Geometry>>>,
    /// Told the grid's size whenever it changes (and once at the start).
    pub on_size: Option<Rc<dyn Fn(u16, u16)>>,
    pub focus: Option<FocusHandle>,
    pub focused: bool,
    /// The blink phase: whether a blinking cursor shows now.
    pub cursor_on: bool,
    /// What an input method is composing, drawn at the cursor.
    pub composition: Option<SharedString>,
    /// The link under the pointer while Ctrl is held.
    pub link: Option<super::emu::LinkSpan>,
    /// Whether the scrollbar shows even when the view is at the bottom (the pointer is over the pane).
    pub scrollbar_hot: bool,
    /// The view to give mouse events and the input method to; `None` for a picture of a screen.
    pub view: Option<Entity<TerminalView>>,
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// What prepaint worked out for paint.
pub struct Prepared {
    snapshot: Arc<Snapshot>,
    geometry: Geometry,
    metrics: Metrics,
    hitbox: Hitbox,
}

impl TerminalElement {
    fn font_size(&self, window: &Window) -> Pixels {
        window.rem_size() * self.rems
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepared;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        match self.fit {
            Fit::Fill => {
                style.size.width = relative(1.).into();
                style.size.height = relative(1.).into();
                style.min_size.width = px(0.).into();
                style.min_size.height = px(0.).into();
                (window.request_layout(style, [], cx), ())
            }
            Fit::Fixed { cols, rows } => {
                let family = self.family.clone();
                let measured = window.request_measured_layout(style, move |known, available, window, cx| {
                    let width = known.width.or(match available.width {
                        AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    });
                    let font_size = match width {
                        Some(width) => fitted_font_size(window, cx, &family, cols, width),
                        None => px(14.),
                    };
                    let m = measure(window, cx, &family, font_size);
                    size(
                        m.cell.width * cols as f32 + px(2. * PAD_X),
                        m.cell.height * rows as f32 + px(2. * PAD_Y),
                    )
                });
                (measured, ())
            }
        }
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let font_size = match self.fit {
            Fit::Fill => self.font_size(window),
            Fit::Fixed { cols, .. } => fitted_font_size(window, cx, &self.family, cols, bounds.size.width),
        };
        let metrics = measure(window, cx, &self.family, font_size);
        let cell = metrics.cell;
        let pad_x = px(PAD_X);
        let pad_y = px(PAD_Y);
        // The scrollbar's room at the end edge is taken out of the text's width.
        let (cols, rows) = match self.fit {
            Fit::Fill => {
                let width = (bounds.size.width - pad_x * 2.).max(px(0.));
                let height = (bounds.size.height - pad_y).max(px(0.));
                (
                    ((f32::from(width) / f32::from(cell.width)).floor() as usize).max(super::emu::MIN_COLS),
                    ((f32::from(height) / f32::from(cell.height)).floor() as usize).max(super::emu::MIN_ROWS),
                )
            }
            Fit::Fixed { cols, rows } => (cols, rows),
        };
        let previous = self.geometry.get().map(|g| (g.cols, g.rows));
        let (snapshot, (cols, rows)) = self.screen.with(|emu| {
            if self.fit == Fit::Fill {
                emu.resize(cols, rows);
            }
            emu.set_cell_pixels(f32::from(cell.width), f32::from(cell.height));
            emu.set_palette(&self.palette, self.palette_id);
            let size = emu.size();
            (emu.snapshot(), size)
        });
        if self.fit == Fit::Fill && previous != Some((cols, rows)) {
            if let Some(report) = &self.on_size {
                report(
                    cols.min(usize::from(u16::MAX)) as u16,
                    rows.min(usize::from(u16::MAX)) as u16,
                );
            }
        }
        let origin = bounds.origin + point(pad_x, pad_y);
        // The scrollbar: a track down the end edge, a thumb for the view.
        let (track, thumb) = scrollbar_boxes(&snapshot, bounds, rows);
        let geometry = Geometry {
            bounds,
            origin,
            cell,
            cols,
            rows,
            track,
            thumb,
        };
        self.geometry.set(Some(geometry));
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        Prepared {
            snapshot,
            geometry,
            metrics,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Prepared {
            snapshot,
            geometry,
            metrics,
            hitbox,
        } = prepaint;
        let (geometry, metrics) = (*geometry, *metrics);
        let palette = self.palette.clone();
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            window.paint_quad(fill(bounds, color(palette.background)));
            self.paint_cells(snapshot, &geometry, metrics, &palette, window, cx);
            self.paint_cursor(snapshot, &geometry, metrics, &palette, window, cx);
            self.paint_composition(snapshot, &geometry, metrics, &palette, window, cx);
            self.paint_scrollbar(snapshot, &geometry, &palette, window);
        });
        let ibeam = if self.link.is_some() {
            CursorStyle::PointingHand
        } else {
            CursorStyle::IBeam
        };
        window.set_cursor_style(ibeam, hitbox);
        if let Some(view) = self.view.clone() {
            if let Some(focus) = &self.focus {
                window.handle_input(focus, ElementInputHandler::new(bounds, view.clone()), cx);
            }
            Self::listen(view, hitbox.clone(), window);
        }
    }
}

impl TerminalElement {
    /// Hands the pointer to the view: presses and wheel turns over the pane, moves and releases anywhere (a
    /// drag leaves the pane).
    fn listen(view: Entity<TerminalView>, hitbox: Hitbox, window: &mut Window) {
        let down_view = view.clone();
        let down_box = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && down_box.is_hovered(window) {
                down_view.update(cx, |this, cx| this.mouse_down(event, window, cx));
            }
        });
        let move_view = view.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                move_view.update(cx, |this, cx| this.mouse_move(event, window, cx));
            }
        });
        let up_view = view.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                up_view.update(cx, |this, cx| this.mouse_up(event, window, cx));
            }
        });
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                view.update(cx, |this, cx| this.wheel(event, window, cx));
            }
        });
    }

    fn paint_cells(
        &self,
        snapshot: &Snapshot,
        geometry: &Geometry,
        metrics: Metrics,
        palette: &Palette,
        window: &mut Window,
        cx: &mut App,
    ) {
        let cell = geometry.cell;
        // Backgrounds and the selection first, so no text is covered.
        for (r, row) in snapshot.rows.iter().enumerate().take(geometry.rows) {
            for f in &row.fills {
                window.paint_quad(fill(geometry.cell_bounds(f.col, r, f.cells), color(f.color)));
            }
            if let Some((a, b)) = row.selected {
                let span = geometry.cell_bounds(a, r, b + 1 - a);
                window.paint_quad(fill(span, color(palette.selection)));
            }
        }
        for (r, row) in snapshot.rows.iter().enumerate().take(geometry.rows) {
            for run in &row.runs {
                let origin = geometry.cell_bounds(run.col, r, run.cells).origin;
                let force = if run.wide { cell.width * 2. } else { cell.width };
                let underline = match run.underline {
                    Underline::None => None,
                    kind => Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(color(run.underline_color.unwrap_or(run.fg))),
                        // A dotted, dashed or double underline is drawn plain; a curl is wavy.
                        wavy: kind == Underline::Curly,
                    }),
                };
                let strikethrough = run.strike.then(|| StrikethroughStyle {
                    thickness: px(1.),
                    color: Some(color(run.fg)),
                });
                let text_run = TextRun {
                    len: run.text.len(),
                    font: font_of(&self.family, run.bold, run.italic),
                    color: color(run.fg),
                    background_color: None,
                    underline,
                    strikethrough,
                };
                let line = window.text_system().shape_line(
                    SharedString::from(run.text.clone()),
                    metrics.font_size,
                    &[text_run],
                    Some(force),
                );
                // A glyph the font can't draw is left out; the rest of the line is still there.
                line.paint(origin, cell.height, TextAlign::Left, None, window, cx).ok();
            }
            // A link under the pointer (Ctrl held) is underlined.
            if let Some((a, b)) = self.link.as_ref().and_then(|link| link.on_row(r, snapshot.cols)) {
                let span = geometry.cell_bounds(a, r, b + 1 - a);
                let line = Bounds {
                    origin: point(span.origin.x, span.origin.y + cell.height - px(2.)),
                    size: size(span.size.width, px(1.)),
                };
                window.paint_quad(fill(line, color(palette.cursor)));
            }
        }
    }

    fn paint_cursor(
        &self,
        snapshot: &Snapshot,
        geometry: &Geometry,
        metrics: Metrics,
        palette: &Palette,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(cursor) = &snapshot.cursor else { return };
        if snapshot.offset > 0 && cursor.row >= geometry.rows {
            return;
        }
        // A blinking cursor is off half the time; an unfocused one is always shown, hollow.
        if self.focused && cursor.blinking && !self.cursor_on {
            return;
        }
        let at = geometry.cell_bounds(cursor.col, cursor.row, if cursor.wide { 2 } else { 1 });
        let hue = color(palette.cursor);
        let shape = if self.focused || cursor.shape != CursorShape::Block {
            cursor.shape
        } else {
            CursorShape::HollowBlock
        };
        match shape {
            CursorShape::Block => {
                window.paint_quad(fill(at, hue));
                self.paint_cursor_glyph(cursor, at, metrics, palette, window, cx);
            }
            CursorShape::HollowBlock => window.paint_quad(quad(
                at,
                0.,
                gpui_kit::transparent_black(),
                Edges::all(px(1.)),
                hue,
                BorderStyle::Solid,
            )),
            CursorShape::Underline => {
                let line = Bounds {
                    origin: point(at.origin.x, at.origin.y + at.size.height - px(2.)),
                    size: size(at.size.width, px(2.)),
                };
                window.paint_quad(fill(line, hue));
            }
            CursorShape::Beam => {
                let line = Bounds {
                    origin: at.origin,
                    size: size(px(2.), at.size.height),
                };
                window.paint_quad(fill(line, hue));
            }
            CursorShape::Hidden => {}
        }
    }

    /// The character under a block cursor, in the accent color (`cursorAccent`).
    fn paint_cursor_glyph(
        &self,
        cursor: &Cursor,
        at: Bounds<Pixels>,
        metrics: Metrics,
        palette: &Palette,
        window: &mut Window,
        cx: &mut App,
    ) {
        if cursor.text.trim().is_empty() {
            return;
        }
        let text_run = TextRun {
            len: cursor.text.len(),
            font: font_of(&self.family, cursor.bold, cursor.italic),
            color: color(palette.cursor_accent),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            SharedString::from(cursor.text.clone()),
            metrics.font_size,
            &[text_run],
            Some(at.size.width / if cursor.wide { 2. } else { 1. }),
        );
        line.paint(at.origin, at.size.height, TextAlign::Left, None, window, cx)
            .ok();
    }

    /// What an input method is composing, underlined at the cursor.
    fn paint_composition(
        &self,
        snapshot: &Snapshot,
        geometry: &Geometry,
        metrics: Metrics,
        palette: &Palette,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (Some(text), Some(cursor)) = (&self.composition, &snapshot.cursor) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let at = geometry.cell_bounds(cursor.col, cursor.row, 1).origin;
        let text_run = TextRun {
            len: text.len(),
            font: font_of(&self.family, false, false),
            color: color(palette.foreground),
            background_color: None,
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(color(palette.foreground)),
                wavy: false,
            }),
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(text.clone(), metrics.font_size, &[text_run], None);
        let covered = Bounds {
            origin: at,
            size: size(line.width().max(geometry.cell.width), geometry.cell.height),
        };
        window.paint_quad(fill(covered, color(palette.background)));
        line.paint(at, geometry.cell.height, TextAlign::Left, None, window, cx)
            .ok();
    }

    fn paint_scrollbar(&self, snapshot: &Snapshot, geometry: &Geometry, palette: &Palette, window: &mut Window) {
        let (Some(thumb), true) = (geometry.thumb, snapshot.offset > 0 || self.scrollbar_hot) else {
            return;
        };
        let hue = color(palette.foreground).opacity(0.35);
        window.paint_quad(quad(
            thumb,
            px(SCROLLBAR / 2.),
            hue,
            Edges::all(px(0.)),
            gpui_kit::transparent_black(),
            BorderStyle::Solid,
        ));
    }
}

/// The scrollbar's track and thumb for a screen with `rows` visible and `snapshot.history` kept, at the end edge
/// of `bounds` (the left edge in a right-to-left interface). `None` when there is nothing to scroll.
fn scrollbar_boxes(
    snapshot: &Snapshot,
    bounds: Bounds<Pixels>,
    rows: usize,
) -> (Option<Bounds<Pixels>>, Option<Bounds<Pixels>>) {
    if snapshot.history == 0 {
        return (None, None);
    }
    let width = px(SCROLLBAR);
    let x = if i18n::is_rtl() {
        bounds.origin.x
    } else {
        bounds.origin.x + bounds.size.width - width
    };
    let track = Bounds {
        origin: point(x, bounds.origin.y),
        size: size(width, bounds.size.height),
    };
    let total = (snapshot.history + rows) as f32;
    let height = f32::from(bounds.size.height);
    let thumb_h = (height * rows as f32 / total).max(24.).min(height);
    // The view's top is `history - offset` lines into everything.
    let top = (height - thumb_h) * (snapshot.history - snapshot.offset.min(snapshot.history)) as f32
        / snapshot.history as f32;
    let thumb = Bounds {
        origin: point(x, bounds.origin.y + px(top)),
        size: size(width, px(thumb_h)),
    };
    (Some(track), Some(thumb))
}

/// For [`AnyElement`]-typed callers.
impl TerminalElement {
    pub fn into_any(self) -> AnyElement {
        self.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::terminal::emu::{Hooks, Screen};

    fn snapshot_with_history(lines: usize, offset: usize) -> Arc<Snapshot> {
        let screen = Screen::new(20, 4, Hooks::none());
        for n in 0..lines {
            screen.feed(format!("line {n}\r\n").as_bytes());
        }
        screen.with(|e| {
            e.scroll_by(offset as i32);
            e.snapshot()
        })
    }

    fn area() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(400.), px(200.)),
        }
    }

    #[test]
    fn no_scrollback_no_scrollbar() {
        let snap = snapshot_with_history(2, 0);
        assert_eq!(scrollbar_boxes(&snap, area(), 4), (None, None));
    }

    #[test]
    fn the_thumb_sits_at_the_bottom_when_the_view_is_and_at_the_top_when_it_is_scrolled_all_the_way() {
        let _guard = crate::testing::lock();
        crate::testing::set_rtl(false);
        let bottom = snapshot_with_history(40, 0);
        let (track, thumb) = scrollbar_boxes(&bottom, area(), 4);
        let (track, thumb) = (track.unwrap(), thumb.unwrap());
        assert_eq!(
            track.origin.x,
            px(10. + 400. - SCROLLBAR),
            "at the end edge, the right in English"
        );
        assert!((thumb.origin.y + thumb.size.height - (track.origin.y + track.size.height)).abs() < px(0.5));
        let history = bottom.history;
        let top = snapshot_with_history(40, history);
        let (_, thumb) = scrollbar_boxes(&top, area(), 4);
        assert_eq!(thumb.unwrap().origin.y, px(20.));
        // In Arabic the bar is on the left.
        crate::testing::set_rtl(true);
        let (track, _) = scrollbar_boxes(&bottom, area(), 4);
        assert_eq!(track.unwrap().origin.x, px(10.));
        crate::testing::set_rtl(false);
    }

    #[test]
    fn the_pointer_maps_to_a_cell_and_a_half() {
        let geometry = Geometry {
            bounds: area(),
            origin: point(px(18.), px(24.)),
            cell: size(px(8.), px(16.)),
            cols: 40,
            rows: 10,
            track: None,
            thumb: None,
        };
        assert_eq!(geometry.cell_at(point(px(18.), px(24.))), (0, 0, false));
        assert_eq!(
            geometry.cell_at(point(px(18. + 8. * 3. + 5.), px(24. + 16. * 2. + 1.))),
            (3, 2, true)
        );
        // Outside the grid it clamps.
        assert_eq!(geometry.cell_at(point(px(0.), px(0.))), (0, 0, false));
        assert_eq!(geometry.cell_at(point(px(900.), px(900.))), (39, 9, true));
        let at = geometry.cell_bounds(2, 1, 2);
        assert_eq!((at.origin.x, at.origin.y, at.size.width), (px(34.), px(40.), px(16.)));
    }
}
