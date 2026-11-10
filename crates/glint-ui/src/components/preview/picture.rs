//! A picture, drawn here: fit, zoom and pan, what shows through it, the color picker, Copy text, Rotate and
//! Wallpaper (`ImagePreview` in FilePreview.tsx).
//!
//! The file is read and decoded on a background thread (GPUI's own decoders: PNG, JPEG with its EXIF turn, GIF
//! and WebP with their frames, BMP, TIFF, ICO, Netpbm and SVG). A picture those can't read (HEIC, AVIF, RAW,
//! PSD…) is shown from Windows' own thumbnail of it, large. A picture in an archive or on a remote is copied out
//! to a temp file first (`archiveExtract`), as `ArchivedMedia` did. Only the picture on screen is held: when the
//! view goes, its image is let go of the GPU's atlas by the next frame ([`release_retired`]).
//!
//! Zoom is the wheel (up zooms in, 1× to 8×), double-click toggles 1× and 2×, and a zoomed picture is dragged to
//! pan. The frame is the size of the fitted picture and clips what is zoomed past it, as the TypeScript's did.

use super::sources::{sources, Sources};
use super::states::{big_icon, Waiting};
use crate::components::file_grid::thumbnail::parse_header;
use crate::explorer;
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::entries::{is_archived, is_closed, is_recycled, is_remote, FileEntry};
use gpui_kit::{
    anchored, canvas, deferred, fill, img, point, size, Bounds, ClipboardItem, Image, ImageFormat, MouseButton,
    ObjectFit, RenderImage, ScrollEvent, Size, StyledImage as _, Task,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// The biggest file decoded here; a bigger one is shown from Windows' thumbnail if it has one.
const MAX_BYTES: u64 = 64 << 20;
/// Pictures Windows' imaging component rotates in place (`ROTATE_EXTS`).
const ROTATE_EXTS: [&str; 7] = ["jpg", "jpeg", "png", "bmp", "tif", "tiff", "jfif"];
/// Those it can set as the desktop background (`WALLPAPER_EXTS`).
const WALLPAPER_EXTS: [&str; 10] = ["jpg", "jpeg", "png", "bmp", "gif", "tif", "tiff", "jfif", "webp", "jxr"];
/// The wheel's step, and the zoom's limits.
const ZOOM_STEP: f32 = 1.2;
const MAX_ZOOM: f32 = 8.;
/// The color picker's lens: pixels either side of the one under the pointer, and the size each is drawn at.
const RADIUS: i32 = 11;
const CELL: f32 = 4.;
/// The checks behind a transparent picture: a cell's size (the TypeScript's 16 px tile is 2 x 2 of these).
const CHECK: f32 = 8.;
/// `bg-zinc-950` and `bg-white`: what shows through a picture when the checks are off. (Not theme tokens: the
/// backdrop is a choice about the picture, not about the app.)
const DARK_BACKDROP: u32 = 0x09_09_0b;
const LIGHT_BACKDROP: u32 = 0xff_ff_ff;

thread_local! {
    /// Pictures whose view has gone, until a frame can let them go of the GPU's atlas.
    static RETIRED: RefCell<Vec<Arc<RenderImage>>> = const { RefCell::new(Vec::new()) };
}

/// Lets pictures that are no longer shown go of the GPU's atlas. Call once per frame from a view that has the
/// window (the pane's render does).
pub fn release_retired(window: &mut Window) {
    for image in RETIRED.with(|retired| std::mem::take(&mut *retired.borrow_mut())) {
        window.drop_image(image).ok();
    }
}

/// What shows through a transparent picture (`ImageBackdrop`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Backdrop {
    /// Checks.
    #[default]
    Checks,
    Dark,
    Light,
}

impl Backdrop {
    fn next(self) -> Self {
        match self {
            Backdrop::Checks => Backdrop::Dark,
            Backdrop::Dark => Backdrop::Light,
            Backdrop::Light => Backdrop::Checks,
        }
    }

    fn label(self) -> SharedString {
        match self {
            Backdrop::Checks => t!("Checks"),
            Backdrop::Dark => t!("Dark"),
            Backdrop::Light => t!("Light"),
        }
    }
}

/// A decoded picture and its size in pixels.
#[derive(Clone)]
struct Ready {
    image: Arc<RenderImage>,
    width: u32,
    height: u32,
}

enum Load {
    Loading,
    Failed(SharedString),
    Ready(Ready),
}

/// The color picker's reading under the pointer.
#[derive(Clone)]
struct Loupe {
    /// The pointer, in the window.
    at: Point<Pixels>,
    /// `#RRGGBB`.
    hex: SharedString,
    color: u32,
    /// The pixels around it, row by row, `0xRRGGBB` (a side of `2 * RADIUS + 1`; outside the picture, 0).
    cells: Vec<u32>,
}

/// A picture on screen, with its tools.
pub struct PictureView {
    entry: FileEntry,
    sources: Sources,
    load: Load,
    waiting: Entity<Waiting>,
    zoom: f32,
    pan: Point<Pixels>,
    /// Where the pointer went down and where the pan was then.
    drag: Option<(Point<Pixels>, Point<Pixels>)>,
    backdrop: Backdrop,
    loupe_on: bool,
    loupe: Option<Loupe>,
    /// The space the picture may take, measured while drawing (a frame later it is used).
    area: Rc<Cell<Size<Pixels>>>,
    /// Where the fitted picture was drawn last.
    frame: Rc<Cell<Bounds<Pixels>>>,
    reading: bool,
    rotating: bool,
    task: Option<Task<()>>,
}

impl PictureView {
    /// The picture of `entry`; its decoding starts at once, off the UI thread.
    pub fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let waiting = cx.new(|cx| {
            Waiting::new(
                is_archived(&entry).then(|| {
                    if is_remote(&entry) {
                        t!("Downloading…")
                    } else {
                        t!("Extracting from the archive…")
                    }
                }),
                cx,
            )
        });
        let mut view = PictureView {
            entry,
            sources: sources(cx),
            load: Load::Loading,
            waiting,
            zoom: 1.,
            pan: Point::default(),
            drag: None,
            backdrop: Backdrop::default(),
            loupe_on: false,
            loupe: None,
            area: Rc::new(Cell::new(Size::default())),
            frame: Rc::new(Cell::new(Bounds::default())),
            reading: false,
            rotating: false,
            task: None,
        };
        view.load(window, cx);
        view
    }

    /// The picture's size in pixels once it has been decoded.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match &self.load {
            Load::Ready(ready) => Some((ready.width, ready.height)),
            _ => None,
        }
    }

    /// Whether it failed (or is too big), and the reason.
    pub fn failure(&self) -> Option<&str> {
        match &self.load {
            Load::Failed(note) => Some(note),
            _ => None,
        }
    }

    /// The zoom, 1 to 8.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    /// What shows through it.
    pub fn backdrop(&self) -> Backdrop {
        self.backdrop
    }

    fn local(&self) -> bool {
        !is_archived(&self.entry) && !is_remote(&self.entry) && !is_recycled(&self.entry) && !is_closed(&self.entry)
    }

    fn rotatable(&self) -> bool {
        self.local() && ROTATE_EXTS.contains(&self.entry.ext.as_str())
    }

    fn can_be_wallpaper(&self) -> bool {
        self.local() && WALLPAPER_EXTS.contains(&self.entry.ext.as_str())
    }

    /// Reads and decodes the file in the background.
    fn load(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.retire();
        self.load = Load::Loading;
        let sources = self.sources.clone();
        let entry = self.entry.clone();
        let renderer = cx.svg_renderer();
        self.task = Some(cx.spawn(async move |this, cx| {
            // A picture in an archive or on a remote is shown from a temp copy.
            let path = if is_archived(&entry) {
                match (sources.extract)(&entry.path).await {
                    Ok(path) => path,
                    Err(reason) => {
                        this.update(cx, |this, cx| this.failed(SharedString::from(reason), cx))
                            .ok();
                        return;
                    }
                }
            } else {
                entry.path.clone()
            };
            let too_big = entry.size > MAX_BYTES;
            let ext = entry.ext.clone();
            let (read_path, read_renderer) = (path.clone(), renderer.clone());
            let decoded = cx
                .background_executor()
                .spawn(async move {
                    if too_big {
                        Err(t!("Too big to preview here: open it to read it.").to_string())
                    } else {
                        decode_file(&read_path, &ext, read_renderer)
                    }
                })
                .await;
            let decoded = match decoded {
                Ok(image) => Ok(image),
                Err(reason) => {
                    // HEIC, AVIF, RAW, PSD…: Windows can draw them, as a thumbnail.
                    let answer = (sources.thumbnail)(&path, entry.modified, entry.size).await;
                    let from_thumbnail = cx
                        .background_executor()
                        .spawn(async move { answer.ok().and_then(|bytes| decode_thumbnail(&bytes, renderer)) })
                        .await;
                    from_thumbnail.ok_or(reason)
                }
            };
            this.update(cx, |this, cx| match decoded {
                Ok(image) => this.decoded(image, cx),
                Err(reason) => {
                    // The reason a file couldn't be read is for the log; the person is told what the TypeScript said.
                    let note = if reason.starts_with("Too big") {
                        SharedString::from(reason)
                    } else {
                        t!("This image can't be shown here")
                    };
                    this.failed(note, cx)
                }
            })
            .ok();
        }));
    }

    fn decoded(&mut self, image: Arc<RenderImage>, cx: &mut Context<Self>) {
        let size = image.size(0);
        self.load = Load::Ready(Ready {
            width: u32::try_from(size.width.0).unwrap_or(0),
            height: u32::try_from(size.height.0).unwrap_or(0),
            image,
        });
        cx.notify();
    }

    fn failed(&mut self, note: SharedString, cx: &mut Context<Self>) {
        self.load = Load::Failed(note);
        cx.notify();
    }

    /// The picture on screen goes to the retired list, to be let go of the GPU.
    fn retire(&mut self) {
        if let Load::Ready(ready) = std::mem::replace(&mut self.load, Load::Loading) {
            RETIRED.with(|retired| retired.borrow_mut().push(ready.image));
        }
    }

    // ---- Zoom and pan ----------------------------------------------------------------------------------

    fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        self.zoom = zoom.clamp(1., MAX_ZOOM);
        if self.zoom <= 1. {
            self.pan = Point::default();
        }
        self.loupe = None;
        cx.notify();
    }

    /// The wheel: up zooms in (a step of 1.2, rounded to two places, as the TypeScript did).
    fn wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let up = event.delta.pixel_delta(px(16.)).y > px(0.);
        let factor = if up { ZOOM_STEP } else { 1. / ZOOM_STEP };
        let next = ((self.zoom * factor) * 100.).round() / 100.;
        self.set_zoom(next, cx);
    }

    fn reset_zoom(&mut self, cx: &mut Context<Self>) {
        self.set_zoom(1., cx);
    }

    // ---- The color picker --------------------------------------------------------------------------------

    /// The picture's rectangle on screen now: the fitted frame, zoomed about its center and panned.
    fn picture_bounds(&self) -> Bounds<Pixels> {
        let frame = self.frame.get();
        let (w, h) = (frame.size.width * self.zoom, frame.size.height * self.zoom);
        Bounds {
            origin: point(
                frame.origin.x + (frame.size.width - w) / 2. + self.pan.x,
                frame.origin.y + (frame.size.height - h) / 2. + self.pan.y,
            ),
            size: size(w, h),
        }
    }

    /// The color under the pointer, with the pixels around it; `None` outside the picture.
    fn read_loupe(&self, at: Point<Pixels>) -> Option<Loupe> {
        let Load::Ready(ready) = &self.load else { return None };
        let frame = self.frame.get();
        let rect = self.picture_bounds();
        if !frame.contains(&at) || !rect.contains(&at) || rect.size.width <= px(0.) || rect.size.height <= px(0.) {
            return None;
        }
        let rel_x = (at.x - rect.origin.x) / rect.size.width;
        let rel_y = (at.y - rect.origin.y) / rect.size.height;
        let bytes = ready.image.as_bytes(0)?;
        let (w, h) = (i64::from(ready.width), i64::from(ready.height));
        if w == 0 || h == 0 {
            return None;
        }
        let px_x = ((rel_x * ready.width as f32).floor() as i64).clamp(0, w - 1);
        let px_y = ((rel_y * ready.height as f32).floor() as i64).clamp(0, h - 1);
        let at_pixel = |x: i64, y: i64| -> u32 {
            if x < 0 || y < 0 || x >= w || y >= h {
                return 0;
            }
            let i = ((y * w + x) * 4) as usize;
            // GPUI keeps pictures as BGRA.
            match bytes.get(i..i + 3) {
                Some([b, g, r]) => u32::from(*r) << 16 | u32::from(*g) << 8 | u32::from(*b),
                _ => 0,
            }
        };
        let color = at_pixel(px_x, px_y);
        let mut cells = Vec::with_capacity(((2 * RADIUS + 1) * (2 * RADIUS + 1)) as usize);
        for y in -RADIUS..=RADIUS {
            for x in -RADIUS..=RADIUS {
                cells.push(at_pixel(px_x + i64::from(x), px_y + i64::from(y)));
            }
        }
        Some(Loupe {
            at,
            hex: format!("#{color:06X}").into(),
            color,
            cells,
        })
    }

    fn pointer_moved(&mut self, at: Point<Pixels>, pressed: bool, cx: &mut Context<Self>) {
        if let Some((from, pan)) = self.drag {
            if pressed {
                self.pan = point(pan.x + (at.x - from.x), pan.y + (at.y - from.y));
                cx.notify();
            } else {
                self.drag = None;
            }
        } else if self.loupe_on {
            self.loupe = self.read_loupe(at);
            cx.notify();
        }
    }

    fn pointer_down(&mut self, event: &gpui_kit::MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.click_count == 2 {
            let next = if self.zoom > 1. { 1. } else { 2. };
            self.set_zoom(next, cx);
            return;
        }
        if self.loupe_on {
            if let Some(loupe) = self.loupe.clone() {
                cx.write_to_clipboard(ClipboardItem::new_string(loupe.hex.to_string()));
                window.push_notification(Notification::success(t!("Copied {hex}", hex = loupe.hex)), cx);
            }
            return;
        }
        if self.zoom > 1. {
            self.drag = Some((event.position, self.pan));
        }
    }

    fn stop_dragging(&mut self) {
        self.drag = None;
    }

    // ---- Windows' own picture tools ----------------------------------------------------------------------

    /// Copy text: the text in it, read by Windows' OCR, to the clipboard.
    fn copy_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading {
            return;
        }
        self.reading = true;
        cx.notify();
        let running = (self.sources.ocr)(&self.entry.path);
        cx.spawn_in(window, async move |this, cx| {
            let lines = running.await;
            this.update_in(cx, |this, window, cx| {
                this.reading = false;
                match lines {
                    Ok(lines) if lines.is_empty() => {
                        window.push_notification(Notification::info(t!("No text found in this picture")), cx)
                    }
                    Ok(lines) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
                        let title = if lines.len() == 1 {
                            t!("Copied a line of text")
                        } else {
                            t!("Copied {n} lines of text", n = lines.len())
                        };
                        window.push_notification(
                            Notification::success(lines.first().cloned().unwrap_or_default()).title(title),
                            cx,
                        );
                    }
                    Err(reason) => {
                        window.push_notification(Notification::error(reason).title(t!("Couldn't read the text")), cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Rotate: a quarter clockwise, in place; the folder is read again.
    fn rotate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rotating {
            return;
        }
        self.rotating = true;
        cx.notify();
        let running = (self.sources.rotate)(&self.entry.path);
        let name = self.entry.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let done = running.await;
            this.update_in(cx, |this, window, cx| {
                this.rotating = false;
                match done {
                    Ok(()) => {
                        this.load(window, cx);
                        if let Some(explorer) = explorer::try_explorer(cx) {
                            explorer.update(cx, |explorer, cx| explorer.reload(None, cx).detach());
                        }
                    }
                    Err(reason) => window.push_notification(
                        Notification::error(reason).title(t!("Couldn't rotate “{name}”", name = name)),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Wallpaper: sets it as the desktop background.
    fn set_wallpaper(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let running = (self.sources.wallpaper)(&self.entry.path);
        let name = self.entry.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let done = running.await;
            this.update_in(cx, |_, window, cx| match done {
                Ok(()) => window.push_notification(
                    Notification::success(t!("“{name}” is now your desktop background", name = name)),
                    cx,
                ),
                Err(reason) => window.push_notification(
                    Notification::error(reason).title(t!("Couldn't set the desktop background")),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }
}

impl Drop for PictureView {
    fn drop(&mut self) {
        self.retire();
    }
}

// ---- Decoding -------------------------------------------------------------------------------------------

/// The format a file's first bytes (or, for SVG, its extension and text) say it is.
pub fn sniff(bytes: &[u8], ext: &str) -> Option<ImageFormat> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(&[0x89, b'P', b'N', b'G']) {
        Some(ImageFormat::Png)
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageFormat::Jpeg)
    } else if starts(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if starts(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some(ImageFormat::Webp)
    } else if starts(b"BM") {
        Some(ImageFormat::Bmp)
    } else if starts(&[b'I', b'I', 42, 0]) || starts(&[b'M', b'M', 0, 42]) {
        Some(ImageFormat::Tiff)
    } else if starts(&[0, 0, 1, 0]) {
        Some(ImageFormat::Ico)
    } else if bytes.len() > 2 && bytes[0] == b'P' && (b'1'..=b'6').contains(&bytes[1]) && bytes[2].is_ascii_whitespace()
    {
        Some(ImageFormat::Pnm)
    } else if ext == "svg" || ext == "svgz" || looks_like_svg(bytes) {
        Some(ImageFormat::Svg)
    } else {
        None
    }
}

/// An XML or SVG document: `<svg` within its first bytes.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    head.windows(4).any(|w| w == b"<svg")
}

/// Reads and decodes a picture file (on a background thread).
fn decode_file(path: &str, ext: &str, renderer: gpui_kit::SvgRenderer) -> Result<Arc<RenderImage>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let format = sniff(&bytes, ext).ok_or_else(|| "a picture format GPUI doesn't decode".to_string())?;
    Image::from_bytes(format, bytes)
        .to_image_data(renderer)
        .map_err(|e| e.to_string())
}

/// Windows' thumbnail as core answers it (an 8-byte header, then a BMP), decoded.
fn decode_thumbnail(answer: &[u8], renderer: gpui_kit::SvgRenderer) -> Option<Arc<RenderImage>> {
    let header = parse_header(answer)?;
    if !header.found {
        return None;
    }
    let bmp = answer.get(8..)?.to_vec();
    Image::from_bytes(ImageFormat::Bmp, bmp).to_image_data(renderer).ok()
}

/// The size a picture of `natural` pixels is drawn at in `area`: its own size, or smaller to fit (`max-h-full
/// max-w-full`, never enlarged).
pub fn fit(natural: (f32, f32), area: (f32, f32)) -> (f32, f32) {
    let (w, h) = natural;
    if w <= 0. || h <= 0. || area.0 <= 0. || area.1 <= 0. {
        return (0., 0.);
    }
    let scale = (area.0 / w).min(area.1 / h).min(1.);
    (w * scale, h * scale)
}

// ---- Drawing --------------------------------------------------------------------------------------------

impl Render for PictureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        release_retired(window);
        let t = theme::tokens(cx).clone();
        let ready = match &self.load {
            Load::Loading => return self.waiting.clone().into_any_element(),
            Load::Failed(note) => return big_icon(&self.entry, Some(note.clone()), None, cx).into_any_element(),
            Load::Ready(ready) => ready.clone(),
        };
        let area = self.area.get();
        let (fw, fh) = fit(
            (ready.width as f32, ready.height as f32),
            (f32::from(area.width), f32::from(area.height)),
        );
        let (frame_w, frame_h) = (px(fw), px(fh));
        let zoom = self.zoom;
        let (img_w, img_h) = (frame_w * zoom, frame_h * zoom);
        let (left, top) = ((frame_w - img_w) / 2. + self.pan.x, (frame_h - img_h) / 2. + self.pan.y);
        let view = cx.entity();
        let area_cell = self.area.clone();
        let frame_cell = self.frame.clone();
        let id = view.entity_id();
        let backdrop = self.backdrop;
        let checks = t.muted;
        let cursor_crosshair = self.loupe_on;
        let measuring = canvas(
            move |bounds, window, _| {
                if area_cell.get() != bounds.size {
                    area_cell.set(bounds.size);
                    // Not `cx.notify` here: GPUI drops a redraw asked for during a draw. After this frame, it counts.
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();
        let backdrop_layer = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| match backdrop {
                Backdrop::Dark => window.paint_quad(fill(bounds, gpui_kit::rgb(DARK_BACKDROP))),
                Backdrop::Light => window.paint_quad(fill(bounds, gpui_kit::rgb(LIGHT_BACKDROP))),
                Backdrop::Checks => {
                    let cell = px(CHECK);
                    let cols = (f32::from(bounds.size.width) / CHECK).ceil() as i32;
                    let rows = (f32::from(bounds.size.height) / CHECK).ceil() as i32;
                    for row in 0..rows {
                        for col in 0..cols {
                            if (row + col) % 2 == 0 {
                                let origin =
                                    point(bounds.origin.x + cell * col as f32, bounds.origin.y + cell * row as f32);
                                let square = Bounds {
                                    origin,
                                    size: size(cell, cell),
                                }
                                .intersect(&bounds);
                                window.paint_quad(fill(square, checks));
                            }
                        }
                    }
                }
            },
        )
        .absolute()
        .size_full();
        let frame_measure = canvas(move |bounds, _, _| frame_cell.set(bounds), |_, _, _, _| {})
            .absolute()
            .size_full();

        let picture = img(ready.image.clone())
            .object_fit(ObjectFit::Fill)
            .absolute()
            .left(left)
            .top(top)
            .w(img_w)
            .h(img_h);
        let frame = div()
            .id("preview-picture")
            .role(Role::Image)
            .aria_label(t!("Picture: {name}", name = self.entry.name.clone()))
            .test_support()
            .relative()
            .flex_none()
            .w(frame_w)
            .h(frame_h)
            .overflow_hidden()
            .rounded(theme::size::RADIUS_SM)
            .when(cursor_crosshair, |frame| frame.cursor_crosshair())
            .when(!cursor_crosshair && zoom > 1., |frame| frame.cursor_grab())
            .when(!cursor_crosshair && zoom <= 1., |frame| frame.cursor_pointer())
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                this.wheel(event, cx);
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseDownEvent, window, cx| this.pointer_down(event, window, cx)),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui_kit::MouseMoveEvent, _, cx| {
                this.pointer_moved(event.position, event.dragging(), cx)
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.stop_dragging()))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, _| this.stop_dragging()))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.drag = None;
                    if this.loupe.take().is_some() {
                        cx.notify();
                    }
                }
            }))
            .child(backdrop_layer)
            .child(picture)
            .child(frame_measure);

        let loupe = self
            .loupe
            .clone()
            .filter(|_| self.loupe_on)
            .map(|loupe| deferred(loupe_view(&loupe, &t, window)).with_priority(1));

        let tools = self.tools(&ready, &t, cx);
        col()
            .id("preview-picture-pane")
            .size_full()
            .items_center()
            .justify_center()
            .gap(space(2.))
            .p(space(4.))
            .child(
                div().relative().flex_1().min_h_0().w_full().child(measuring).child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(frame),
                ),
            )
            .child(tools)
            .children(loupe)
            .into_any_element()
    }
}

/// The magnifier by the pointer: the pixels around it, and the color's name.
fn loupe_view(loupe: &Loupe, t: &theme::Tokens, window: &Window) -> impl IntoElement {
    let side = (2 * RADIUS + 1) as f32 * CELL;
    let cells = loupe.cells.clone();
    let lens = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let n = 2 * RADIUS + 1;
            for (i, color) in cells.iter().enumerate() {
                let (x, y) = ((i as i32 % n) as f32, (i as i32 / n) as f32);
                let cell = Bounds {
                    origin: point(bounds.origin.x + px(x * CELL), bounds.origin.y + px(y * CELL)),
                    size: size(px(CELL), px(CELL)),
                };
                window.paint_quad(fill(cell, gpui_kit::rgb(*color)));
            }
            // The pixel under the pointer, outlined.
            let middle = Bounds {
                origin: point(
                    bounds.origin.x + px(RADIUS as f32 * CELL),
                    bounds.origin.y + px(RADIUS as f32 * CELL),
                ),
                size: size(px(CELL), px(CELL)),
            };
            window.paint_quad(gpui_kit::outline(
                middle,
                gpui_kit::white(),
                gpui_kit::BorderStyle::Solid,
            ));
        },
    )
    .size(px(side));
    // Below the pointer near the top of the window, above it elsewhere.
    let below = loupe.at.y < px(140.);
    let _ = window;
    anchored()
        .position(loupe.at)
        .anchor(if below {
            gpui_kit::Anchor::TopLeft
        } else {
            gpui_kit::Anchor::BottomLeft
        })
        .snap_to_window()
        .child(
            col()
                .id("preview-loupe")
                .role(Role::Status)
                .aria_label(loupe.hex.clone())
                .test_support()
                .items_center()
                .gap(space(1.))
                .mt(if below { px(18.) } else { px(0.) })
                .mb(if below { px(0.) } else { px(14.) })
                .child(
                    div()
                        .rounded(theme::size::RADIUS)
                        .border_2()
                        .border_color(t.background)
                        .shadow_lg()
                        .overflow_hidden()
                        .bg(gpui_kit::rgb(DARK_BACKDROP))
                        .child(lens),
                )
                .child(
                    row()
                        .gap(space(1.5))
                        .rounded_full()
                        .border_1()
                        .border_color(t.border)
                        .bg(t.popover)
                        .text_color(t.popover_foreground)
                        .px(space(2.))
                        .py(space(0.5))
                        .text_size(theme::size::XXS)
                        .font_weight(FontWeight::MEDIUM)
                        .font_family("Cascadia Mono")
                        .child(
                            div()
                                .size_2p5()
                                .rounded_full()
                                .border_1()
                                .border_color(t.border)
                                .bg(gpui_kit::rgb(loupe.color)),
                        )
                        .child(loupe.hex.clone()),
                ),
        )
}

impl PictureView {
    /// The row under the picture: its size, the zoom, Colors, what shows through, and Windows' tools.
    fn tools(&self, ready: &Ready, t: &theme::Tokens, cx: &mut Context<Self>) -> Div {
        let small = |id: &'static str, glyph: IconName, label: SharedString, hint: SharedString| {
            Button::new(id)
                .ghost()
                .small()
                .icon(glyph)
                .label(label.clone())
                .accessibility_label(label)
                .tooltip(hint)
        };
        let dimensions = format!("{} × {}", ready.width, ready.height);
        let dims: SharedString = t!("{size} px", size = dimensions);
        let zoom_chip = (self.zoom > 1.).then(|| {
            Button::new("preview-zoom")
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(t.primary.opacity(0.15))
                        .foreground(t.primary)
                        .hover(t.primary.opacity(0.25))
                        .active(t.primary.opacity(0.3)),
                )
                .compact()
                .label(format!("{:.1}×", self.zoom))
                .accessibility_label(t!("Zoom: {zoom}×", zoom = format!("{:.1}", self.zoom)))
                .tooltip(t!("Click to reset zoom to 1×"))
                .on_click(cx.listener(|this, _, _, cx| this.reset_zoom(cx)))
        });
        let colors = small(
            "preview-colors",
            IconName::Pipette,
            t!("Colors"),
            t!("A magnifier that shows each pixel's color: click the picture to copy it (#RRGGBB)"),
        )
        .selected(self.loupe_on)
        .on_click(cx.listener(|this, _, _, cx| {
            this.loupe_on = !this.loupe_on;
            this.loupe = None;
            cx.notify();
        }));
        let backdrop = Button::new("preview-backdrop")
            .ghost()
            .small()
            .label(self.backdrop.label())
            .accessibility_label(t!("Backdrop: {name}", name = self.backdrop.label()))
            .tooltip(t!("What shows through a transparent picture: checks, dark or light"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.backdrop = this.backdrop.next();
                cx.notify();
            }));
        let copy = (!is_archived(&self.entry)).then(|| {
            small(
                "preview-copy-text",
                if self.reading {
                    IconName::LoaderCircle
                } else {
                    IconName::ScanText
                },
                t!("Copy text"),
                t!("Copy the text in this picture, read by Windows' OCR"),
            )
            .disabled(self.reading)
            .on_click(cx.listener(|this, _, window, cx| this.copy_text(window, cx)))
        });
        let rotate = self.rotatable().then(|| {
            small(
                "preview-rotate",
                if self.rotating {
                    IconName::LoaderCircle
                } else {
                    IconName::RotateCwSquare
                },
                t!("Rotate"),
                t!("Turn it a quarter clockwise, in place (a photo keeps its quality: only its orientation changes)"),
            )
            .disabled(self.rotating)
            .on_click(cx.listener(|this, _, window, cx| this.rotate(window, cx)))
        });
        let wallpaper = self.can_be_wallpaper().then(|| {
            small(
                "preview-wallpaper",
                IconName::Wallpaper,
                t!("Wallpaper"),
                t!("Set as desktop background"),
            )
            .on_click(cx.listener(|this, _, window, cx| this.set_wallpaper(window, cx)))
        });
        row_start()
            .flex_none()
            .flex_wrap()
            .justify_center()
            .gap(space(1.5))
            .child(
                div()
                    .id("preview-dimensions")
                    .role(Role::Status)
                    .aria_label(dims.clone())
                    .test_support()
                    .me(1.)
                    .text_size(theme::size::XS)
                    .child(dims),
            )
            .children(zoom_chip)
            .child(colors)
            .child(backdrop)
            .children(copy)
            .children(rotate)
            .children(wallpaper)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_are_told_by_their_first_bytes_and_svg_by_its_text() {
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0, 0], "png"), Some(ImageFormat::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0], "bin"), Some(ImageFormat::Jpeg));
        assert_eq!(sniff(b"GIF89a....", "gif"), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ", "webp"), Some(ImageFormat::Webp));
        assert_eq!(sniff(b"BM\0\0", "bmp"), Some(ImageFormat::Bmp));
        assert_eq!(sniff(&[b'I', b'I', 42, 0, 8], "tif"), Some(ImageFormat::Tiff));
        assert_eq!(sniff(&[0, 0, 1, 0, 1, 0], "ico"), Some(ImageFormat::Ico));
        assert_eq!(sniff(b"P6\n1 1\n255\n", "ppm"), Some(ImageFormat::Pnm));
        assert_eq!(
            sniff(b"<?xml version=\"1.0\"?><svg xmlns=\"x\"/>", "txt"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(sniff(b"anything", "svg"), Some(ImageFormat::Svg));
        // HEIC, AVIF and the rest are not GPUI's: Windows' thumbnail stands in.
        assert_eq!(sniff(b"\0\0\0\x18ftypheic", "heic"), None);
    }

    #[test]
    fn a_picture_is_fitted_into_its_space_and_never_enlarged() {
        assert_eq!(fit((800., 600.), (400., 400.)), (400., 300.));
        assert_eq!(fit((800., 600.), (1600., 1600.)), (800., 600.));
        assert_eq!(fit((100., 400.), (300., 200.)), (50., 200.));
        assert_eq!(fit((0., 0.), (300., 200.)), (0., 0.));
        assert_eq!(fit((10., 10.), (0., 0.)), (0., 0.));
    }

    #[test]
    fn the_backdrop_cycles_through_checks_dark_and_light() {
        let mut b = Backdrop::Checks;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(b);
            b = b.next();
        }
        assert_eq!(
            seen,
            [Backdrop::Checks, Backdrop::Dark, Backdrop::Light, Backdrop::Checks]
        );
    }
}
