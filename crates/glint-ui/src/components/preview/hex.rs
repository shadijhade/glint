//! A file's bytes as hex and text, virtualized; and a program's header (`BinaryPreview`).
//! Ported from HexView.tsx and BinaryPreview.tsx.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `HexView`                                    | [`HexPreview`]                                                |
//! | `fileBytes(path, offset, len)`, `WINDOW`     | [`HexSource`], [`WINDOW`]: 64 KB read as the rows scroll       |
//! | `PAGE_ROWS`, the part before and after       | [`PAGE_ROWS`], the two buttons in the header                  |
//! | `byteClass` colors (`.hx-*`)                 | [`row_text`] and the theme's token colors                      |
//! | "Go to 0x1F00 or 8192"                       | the box in the header, [`HexPreview::go`]                      |
//! | click a byte, arrows, Page, Home, End        | [`HexPreview::select_at`], [`HexPreview::on_key_down`]         |
//! | the status line (`0x…`, `u16`, `u32`)        | [`HexPreview::status_line`]                                    |
//! | `BinaryPreview`                              | [`BinaryPreview`] (the second half of this file)               |
//!
//! A row is three pieces of text (offset, hex, characters) drawn in the monospace font, so the columns line up
//! by themselves; each byte's color is a text run, and a click is turned into a byte by asking the text where
//! it was hit. The rows stay left to right in the Arabic interface: bytes are not Arabic.
//!
//! A big file is read 64 KB at a time, a window as soon as a row of it is drawn, and only the 256 windows
//! nearest the rows on screen are kept (the TypeScript kept every window it ever read). A page of rows is
//! [`PAGE_ROWS`] long: GPUI places rows with 32-bit pixels, which are exact only up to 16 million of them, so a
//! page can't be the TypeScript's million rows (a browser's limit was 33 million pixels, and the page was
//! sized for that).
//!
//! Not ported: [`BinaryPreview`]'s signature chip (`SignatureChip`, a component of its own).

use super::text::{big_icon, is_archived_path, note_centered, page_list, page_scroll, redraw_after_delay, waiting};
use crate::kit::prelude::*;
use crate::{actions, core_bridge, t, theme};
use glint_model::format::{format_count, format_size};
use glint_model::hex::{byte_char, byte_class, hex, parse_offset, ByteClass};
use gpui_kit::{
    ClipboardItem, HighlightStyle, ScrollHandle, ScrollStrategy, StyledText, TextLayout, UniformListScrollHandle,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// How many bytes are read at a time (`WINDOW`).
pub const WINDOW: u64 = 64 * 1024;
/// How many rows a page holds, 2^18: 5 million pixels of rows, which 32-bit pixels place exactly.
pub const PAGE_ROWS: u64 = 1 << 18;
/// How many windows are kept: 16 MB of bytes.
const KEPT_WINDOWS: usize = 256;
/// A row's height (`LINE`, 20 px at the usual size).
const LINE: Rems = rems(1.25);
/// The width from which a row shows 16 bytes; narrower shows 8 (`onWidth`).
const WIDE: f32 = 540.;

// ---- Where bytes come from ----------------------------------------------------------------------------------

/// A read of `len` bytes of a file from `offset`: the file's size, and the bytes.
pub type BytesSource =
    Arc<dyn Fn(String, u64, u32) -> Pin<Box<dyn Future<Output = Result<(u64, Vec<u8>), String>> + Send>> + Send + Sync>;

/// A copy on disk of a file in an archive or on a remote, to read the bytes of.
pub type CopySource = Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>> + Send + Sync>;

/// What the hex view reads from: glint-core in the app, a fake in tests.
#[derive(Clone)]
pub struct HexSource {
    /// `file_bytes`.
    pub bytes: BytesSource,
    /// `archive_extract(path, "preview")`.
    pub copy: CopySource,
}

/// glint-core's `file_bytes` (at most 1 MB a read) and `archive_extract`.
pub fn core_hex_source() -> HexSource {
    HexSource {
        bytes: Arc::new(|path, offset, len| {
            Box::pin(async move {
                let answer = glint_core::filediff::file_bytes(path, offset, len).await?.into_bytes();
                // Its size (u64, little-endian), then the bytes.
                let size = answer
                    .get(..8)
                    .and_then(|head| <[u8; 8]>::try_from(head).ok())
                    .map(u64::from_le_bytes)
                    .ok_or_else(|| "The file's bytes came back without a size".to_string())?;
                Ok((size, answer.get(8..).unwrap_or_default().to_vec()))
            })
        }),
        copy: Arc::new(|path| Box::pin(glint_core::archive::archive_extract(path, "preview".to_string()))),
    }
}

// ---- A row of bytes -----------------------------------------------------------------------------------------

/// A row of bytes as the text drawn, and which bytes are which class.
#[derive(Debug, PartialEq, Eq)]
pub struct RowText {
    /// The offset: eight hex digits.
    pub offset: String,
    /// The bytes in hex, a space after each, and one more after the eighth; missing bytes as blanks.
    pub hex: String,
    /// Each byte's two digits in `hex` (a UTF-8 byte range), by byte.
    pub hex_spans: Vec<(Range<usize>, ByteClass)>,
    /// The bytes as characters, one each (`byte_char`).
    pub chars: String,
    /// Each byte's character in `chars` (a UTF-8 byte range), by byte.
    pub char_spans: Vec<(Range<usize>, ByteClass)>,
}

/// The text of the row starting at `offset`: `cols` bytes wide, `row` holding the bytes there are.
pub fn row_text(offset: u64, row: &[u8], cols: usize) -> RowText {
    let mut hex_text = String::new();
    let mut hex_spans = Vec::new();
    for k in 0..cols {
        if k == 8 {
            hex_text.push(' ');
        }
        match row.get(k) {
            Some(&byte) => {
                let start = hex_text.len();
                hex_text.push_str(&hex(u64::from(byte), 2));
                hex_spans.push((start..hex_text.len(), byte_class(byte)));
            }
            None => hex_text.push_str("  "),
        }
        if k + 1 < cols {
            hex_text.push(' ');
        }
    }
    let mut chars = String::new();
    let mut char_spans = Vec::new();
    for &byte in row.iter().take(cols) {
        let start = chars.len();
        chars.push(byte_char(byte));
        char_spans.push((start..chars.len(), byte_class(byte)));
    }
    RowText {
        offset: hex(offset, 8),
        hex: hex_text,
        hex_spans,
        chars,
        char_spans,
    }
}

/// Which byte of a row a character of its hex text belongs to (the gap before the ninth goes to the eighth).
fn byte_of_hex_char(index: usize) -> usize {
    let index = if index >= 24 { index.saturating_sub(1) } else { index };
    index / 3
}

/// The color of a class of byte: hexyl's, as the stylesheet had them (`.hx-*`).
fn class_color(class: ByteClass, t: &theme::Tokens) -> Hsla {
    match class {
        ByteClass::Null => t.muted_foreground.opacity(0.55),
        ByteClass::Text => t.tk_number,
        ByteClass::Space | ByteClass::Control => t.tk_tag,
        ByteClass::High => t.tk_attr,
    }
}

/// The text runs of a row's piece: each byte in its class's color, the selected one on a tinted ground.
fn highlights(
    spans: &[(Range<usize>, ByteClass)],
    selected: Option<usize>,
    t: &theme::Tokens,
) -> Vec<(Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .enumerate()
        .map(|(k, (range, class))| {
            let mut style = HighlightStyle {
                color: Some(class_color(*class, t)),
                ..Default::default()
            };
            if selected == Some(k) {
                style.background_color = Some(t.primary.opacity(0.22));
                style.color = Some(t.foreground);
            }
            (range.clone(), style)
        })
        .collect()
}

// ---- The view -----------------------------------------------------------------------------------------------

/// A window of bytes: asked for, or read.
enum Chunk {
    Loading,
    Loaded(Arc<Vec<u8>>),
}

/// What the bytes are read from.
enum Readable {
    /// A copy is being made (a file in an archive).
    Pending,
    /// This path.
    Ready(String),
    /// It couldn't be made or read, with why.
    Failed(SharedString),
}

/// A file's bytes as hex and text, virtualized.
pub struct HexPreview {
    path: String,
    source: HexSource,
    readable: Readable,
    size: Option<u64>,
    chunks: HashMap<u64, Chunk>,
    /// The selected byte (`at`).
    at: Option<u64>,
    /// The "Go to" box.
    typed: Entity<InputState>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// The list's size, measured as it is laid out.
    measured: Rc<Cell<Size<Pixels>>>,
    /// When the read started, for the spinner's delay.
    started: Instant,
    generation: u64,
    _typed_events: Subscription,
}

impl HexPreview {
    /// The previewer for the file at `path`. Reading happens in the background; the view shows its loading state
    /// until then.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_hex_source(), window, cx)
    }

    /// [`HexPreview::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: HexSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let typed = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Go to 0x1F00 or 8192")));
        let events = cx.subscribe_in(&typed, window, |this, _, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.go(window, cx);
            }
        });
        let mut this = Self {
            path,
            source,
            readable: Readable::Pending,
            size: None,
            chunks: HashMap::new(),
            at: None,
            typed,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            measured: Rc::new(Cell::new(Size::default())),
            started: Instant::now(),
            generation: 0,
            _typed_events: events,
        };
        this.start(cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file.
    pub fn set_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.start(cx);
        }
    }

    /// Reads the file again (it changed on disk).
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.start(cx);
    }

    /// Scrolls the rows a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let moved = page_list(&self.scroll, down);
        if moved {
            cx.notify();
        }
        moved
    }

    /// The selected byte's offset.
    pub fn selected(&self) -> Option<u64> {
        self.at
    }

    /// The file's size, once the first window has been read.
    pub fn size(&self) -> Option<u64> {
        self.size
    }

    /// Starts over for the path: the archive's copy first, then the first window.
    fn start(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.chunks.clear();
        self.size = None;
        self.at = None;
        self.started = Instant::now();
        redraw_after_delay(cx);
        if is_archived_path(&self.path) {
            // In an archive, the bytes are read from a copy extracted for the preview.
            self.readable = Readable::Pending;
            let copy = (self.source.copy)(self.path.clone());
            let task = cx.spawn(async move |this, cx| {
                let result = core_bridge::run(copy).await.and_then(|inner| inner);
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    match result {
                        Ok(copy) => {
                            this.readable = Readable::Ready(copy);
                            this.load(0, cx);
                        }
                        Err(reason) => this.readable = Readable::Failed(reason.into()),
                    }
                    cx.notify();
                })
                .ok();
            });
            task.detach();
        } else {
            self.readable = Readable::Ready(self.path.clone());
            self.load(0, cx);
        }
        cx.notify();
    }

    /// Asks for window `n` unless it is asked for already (`load`).
    fn load(&mut self, n: u64, cx: &mut Context<Self>) {
        let Readable::Ready(path) = &self.readable else {
            return;
        };
        if self.chunks.contains_key(&n) {
            return;
        }
        self.chunks.insert(n, Chunk::Loading);
        let generation = self.generation;
        let read = (self.source.bytes)(path.clone(), n * WINDOW, WINDOW as u32);
        let task = cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok((size, bytes)) => {
                        this.size = Some(size);
                        this.chunks.insert(n, Chunk::Loaded(Arc::new(bytes)));
                        this.forget_far(n);
                    }
                    Err(reason) => this.readable = Readable::Failed(reason.into()),
                }
                cx.notify();
            })
            .ok();
        });
        task.detach();
    }

    /// Keeps the windows nearest `near`, so a long scroll through a big file doesn't hold all of it.
    fn forget_far(&mut self, near: u64) {
        while self.chunks.len() > KEPT_WINDOWS {
            let far = self.chunks.keys().copied().max_by_key(|n| n.abs_diff(near));
            match far {
                Some(n) => self.chunks.remove(&n),
                None => break,
            };
        }
    }

    /// The byte at `offset`, if its window has been read (`byteAt`).
    fn byte_at(&self, offset: u64) -> Option<u8> {
        match self.chunks.get(&(offset / WINDOW)) {
            Some(Chunk::Loaded(bytes)) => bytes.get(usize::try_from(offset % WINDOW).ok()?).copied(),
            _ => None,
        }
    }

    /// How many bytes a row shows: 16 when the list is wide, else 8.
    fn cols(&self) -> u64 {
        if f32::from(self.measured.get().width) >= WIDE || self.measured.get().width == px(0.) {
            16
        } else {
            8
        }
    }

    /// The first offset of the page `at` is on (`base`).
    fn base(&self) -> u64 {
        let page = PAGE_ROWS * self.cols();
        self.at.unwrap_or(0) / page * page
    }

    /// Selects `to`, kept inside the file, and scrolls to its row (`move`).
    pub fn select_at(&mut self, to: i128, cx: &mut Context<Self>) {
        let Some(size) = self.size.filter(|&size| size > 0) else {
            return;
        };
        let to = to.clamp(0, i128::from(size) - 1) as u64; // clamped into 0..size, which is a u64
        let before = self.base();
        self.at = Some(to);
        let row = ((to - self.base()) / self.cols()) as usize; // a page holds PAGE_ROWS rows, which fit
        if self.base() != before {
            self.scroll.scroll_to_item_strict(row, ScrollStrategy::Top);
        } else {
            self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    /// Enter in the box: go to the offset typed (`go`).
    pub fn go(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(size) = self.size else { return };
        let text = self.typed.read(cx).value().to_string();
        match parse_offset(&text) {
            Some(offset) if offset < size => {
                self.select_at(i128::from(offset), cx);
                // Arrows then move from there.
                window.focus(&self.focus, cx);
            }
            found => {
                let message = if found.is_none() {
                    t!("Type an offset: 0x1F00, or 8192")
                } else {
                    t!("The file ends at 0x{end}", end = hex(size.saturating_sub(1), 8))
                };
                window.push_notification(Notification::error(message), cx);
            }
        }
    }

    /// The little-endian number of the `n` bytes from the selected one (`word`), if they're all read.
    fn word(&self, at: u64, n: u64) -> Option<u64> {
        let mut total = 0u64;
        for k in (0..n).rev() {
            total = total * 256 + u64::from(self.byte_at(at.checked_add(k)?)?);
        }
        Some(total)
    }

    /// The footer: the selected byte as numbers, or what to do (`status`).
    pub fn status_line(&self) -> SharedString {
        let (Some(at), Some(size)) = (self.at, self.size) else {
            return t!("Click a byte, or go to an offset");
        };
        let _ = size;
        let Some(value) = self.byte_at(at) else {
            return t!("Click a byte, or go to an offset");
        };
        let mut parts = vec![
            format!(
                "0x{} ({})",
                hex(at, 8),
                format_count(usize::try_from(at).unwrap_or(usize::MAX))
            ),
            if value > 0x20 && value < 0x7f {
                format!("{} = {value} = '{}'", hex(u64::from(value), 2), char::from(value))
            } else {
                format!("{} = {value}", hex(u64::from(value), 2))
            },
        ];
        for (name, n) in [("u16", 2), ("u32", 4)] {
            if let Some(number) = self.word(at, n) {
                parts.push(format!(
                    "{name} {}",
                    format_count(usize::try_from(number).unwrap_or(usize::MAX))
                ));
            }
        }
        parts.join("   ").into()
    }

    /// The arrows, Page, Home and End move the selection (`onKey`).
    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(size) = self.size.filter(|&s| s > 0) else {
            return;
        };
        let cols = i128::from(self.cols());
        // NOTE(port): with nothing selected the keys move from byte 0 (`Math.max(at, 0)`), so a first Right lands on
        // byte 1. That looks unintended; kept as the TypeScript had it.
        let from = i128::from(self.at.unwrap_or(0));
        let rows = (f32::from(self.measured.get().height) / (LINE.0 * f32::from(self.rem(cx)))).floor();
        let page = if rows.is_finite() && rows >= 1. {
            rows as i128 - 1
        } else {
            15
        }
        .max(1)
            * cols;
        let key = event.keystroke.key.as_str();
        let ctrl = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        let to = match key {
            "right" => from + 1,
            "left" => from - 1,
            "down" => from + cols,
            "up" => from - cols,
            "pagedown" => from + page,
            "pageup" => from - page,
            "home" if ctrl => 0,
            "home" => from - from % cols,
            "end" if ctrl => i128::from(size) - 1,
            "end" => from - from % cols + cols - 1,
            _ => return,
        };
        self.select_at(to, cx);
        cx.stop_propagation();
    }

    /// The size of a rem in pixels, the unit rows are laid out in.
    fn rem(&self, cx: &App) -> Pixels {
        theme::rem_size(cx)
    }

    /// The header: the "Go to" box, the size, and the buttons between pages (when the file is big).
    fn header(
        &self,
        size: u64,
        base: u64,
        page_bytes: u64,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let size_text = if size >= 1024 {
            t!(
                "{size} ({count} bytes)",
                size = format_size(size),
                count = format_count(usize::try_from(size).unwrap_or(usize::MAX))
            )
        } else {
            format_size(size).into()
        };
        let pages = (size > page_bytes).then(|| {
            let before = Button::new("hex-before")
                .ghost()
                .compact()
                .icon(IconName::ChevronLeft)
                .disabled(base == 0)
                .tooltip(t!("The part before"))
                .accessibility_label(t!("The part before"))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select_at(i128::from(base) - i128::from(page_bytes), cx)),
                );
            let after = Button::new("hex-after")
                .ghost()
                .compact()
                .icon(IconName::ChevronRight)
                .disabled(base + page_bytes >= size)
                .tooltip(t!("The part after"))
                .accessibility_label(t!("The part after"))
                .on_click(
                    cx.listener(move |this, _, _, cx| this.select_at(i128::from(base) + i128::from(page_bytes), cx)),
                );
            let range = format!(
                "{}–{}",
                hex(base, 8),
                hex(size.min(base + page_bytes).saturating_sub(1), 8)
            );
            align_row(0.5)
                .id("hex-pages")
                .role(Role::Group)
                .aria_label(t!("The part shown: {range}", range = range.clone()))
                .test_support()
                .flex_none()
                .ms(2.)
                .child(before)
                .child(div().font_family(theme::mono_font(cx)).child(range))
                .child(after)
        });
        row_start()
            .h(space(9.))
            .flex_none()
            .px(space(2.))
            .gap(space(2.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(
                div().w(space(40.)).flex_none().child(
                    Input::new(&self.typed)
                        .small()
                        .aria_label(t!("Go to offset"))
                        .font_family(theme::mono_font(cx)),
                ),
            )
            .child(
                div()
                    .id("hex-size")
                    .role(Role::Status)
                    .aria_label(size_text.clone())
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(size_text),
            )
            .children(pages)
    }

    /// One row of the list: bytes read, or the offset alone while its window is on its way.
    fn data_row(
        &mut self,
        index: usize,
        base: u64,
        cols: u64,
        t: &theme::Tokens,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let offset = base + index as u64 * cols;
        let n = offset / WINDOW;
        let bytes = match self.chunks.get(&n) {
            Some(Chunk::Loaded(bytes)) => Some(bytes.clone()),
            _ => None,
        };
        let Some(bytes) = bytes else {
            self.load(n, cx);
            return div()
                .id(("hex-row", index))
                .role(Role::Row)
                .aria_label(hex(offset, 8))
                .test_support()
                .h(LINE)
                .px(space(3.))
                .text_color(t.muted_foreground.opacity(0.5))
                .child(hex(offset, 8))
                .into_any_element();
        };
        let from = usize::try_from(offset - n * WINDOW).unwrap_or(0).min(bytes.len());
        let slice = bytes
            .get(from..(from + cols as usize).min(bytes.len()))
            .unwrap_or_default();
        let text = row_text(offset, slice, cols as usize);
        let selected = self
            .at
            .filter(|&at| at >= offset && at < offset + slice.len() as u64)
            .map(|at| (at - offset) as usize);
        let label = format!(
            "{}: {}",
            text.offset,
            slice
                .iter()
                .map(|&b| hex(u64::from(b), 2))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let hex_styled = StyledText::new(text.hex.clone()).with_highlights(highlights(&text.hex_spans, selected, t));
        let chars_styled =
            StyledText::new(text.chars.clone()).with_highlights(highlights(&text.char_spans, selected, t));
        let (hex_layout, chars_layout): (TextLayout, TextLayout) =
            (hex_styled.layout().clone(), chars_styled.layout().clone());
        let chars_text = text.chars.clone();
        let count = slice.len();
        row()
            .id(("hex-row", index))
            .role(Role::Row)
            .aria_label(label)
            .test_support()
            .h(LINE)
            .px(space(3.))
            .gap(space(3.))
            .whitespace_nowrap()
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let position = event.position();
                let byte = if let Ok(i) = hex_layout.index_for_position(position) {
                    Some(byte_of_hex_char(i))
                } else if let Ok(i) = chars_layout.index_for_position(position) {
                    Some(chars_text.get(..i).map_or(0, |head| head.chars().count()))
                } else {
                    None
                };
                if let Some(byte) = byte.filter(|&b| b < count) {
                    window.focus(&this.focus, cx);
                    this.select_at(i128::from(offset) + byte as i128, cx);
                }
            }))
            .child(
                div()
                    .flex_none()
                    .text_color(t.muted_foreground.opacity(0.7))
                    .child(text.offset),
            )
            .child(div().flex_none().child(hex_styled))
            .child(div().flex_none().child(chars_styled))
            .font_family(mono.clone())
            .into_any_element()
    }
}

impl Render for HexPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("hex-preview")
            .size_full()
            .min_h_0()
            .text_start()
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        match &self.readable {
            Readable::Pending => {
                return root.child(waiting(self.started, Some(t!("Extracting from the archive…")), cx))
            }
            Readable::Failed(reason) => return root.child(big_icon(&self.path, Some(reason.clone()), cx)),
            Readable::Ready(_) => {}
        }
        let Some(size) = self.size else {
            return root.child(waiting(self.started, None, cx));
        };
        if size == 0 {
            return root.child(note_centered("hex-empty", t!("This file is empty."), cx));
        }
        let cols = self.cols();
        let page_bytes = PAGE_ROWS * cols;
        let base = self.base();
        let rows = (size - base).min(page_bytes).div_ceil(cols) as usize; // at most PAGE_ROWS
        let mono = theme::mono_font(cx);

        // The list's size decides how many bytes a row holds; a canvas under it measures that, and asks for
        // another frame when it changes.
        let measured = self.measured.clone();
        let this = cx.weak_entity();
        let measure = canvas(
            move |bounds, window, _| {
                let was = measured.get();
                if (f32::from(was.width) - f32::from(bounds.size.width)).abs() > 0.5
                    || (f32::from(was.height) - f32::from(bounds.size.height)).abs() > 0.5
                {
                    measured.set(bounds.size);
                    let this = this.clone();
                    window.on_next_frame(move |_, cx| {
                        this.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full();

        let list = uniform_list(
            "hex-rows",
            rows,
            cx.processor(move |this, range: Range<usize>, _, cx| {
                let t = theme::tokens(cx).clone();
                let mono = theme::mono_font(cx);
                range
                    .map(|index| this.data_row(index, base, cols, &t, &mono, cx))
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();

        let footer = div()
            .id("hex-status")
            .role(Role::Status)
            .aria_label(self.status_line())
            .test_support()
            .h(space(8.))
            .flex_none()
            .px(space(3.))
            .flex()
            .items_center()
            .truncate()
            .border_t_1()
            .border_color(t.border)
            .font_family(mono)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(self.status_line());

        root.child(self.header(size, base, page_bytes, &t, cx))
            .child(
                div()
                    .id("hex-bytes")
                    .role(Role::Table)
                    .aria_label(t!("Bytes"))
                    .test_support()
                    .track_focus(&self.focus)
                    .tab_stop(true)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .text_size(theme::size::XS)
                    .py(space(1.))
                    .child(measure)
                    .child(list)
                    .child(Scrollbar::vertical(&self.scroll).viewport_from_layout()),
            )
            .child(footer)
    }
}

impl Focusable for HexPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ---- A program's header ------------------------------------------------------------------------------------

/// Reads a program's PE header, for [`BinaryPreview`]: the path in, what glint-core finds out.
pub type PeSource =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<glint_core::pe::PeInfo, String>> + Send>> + Send + Sync>;

/// glint-core's `inspect_pe`.
pub fn core_pe_source() -> PeSource {
    Arc::new(|path| Box::pin(glint_core::pe::inspect_pe(path)))
}

/// The extensions of programs and libraries that have a PE header (`PE_EXTS`).
const PE_EXTS: [&str; 11] = [
    "exe", "dll", "sys", "ocx", "drv", "cpl", "scr", "efi", "pyd", "node", "mui",
];

/// Whether the file is one `BinaryPreview` shows (`isPeCandidate`): a program or library, by its extension.
pub fn is_pe_candidate(path: &str) -> bool {
    PE_EXTS.contains(&super::text::ext_of(path).as_str())
}

/// The tabs of the header preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeTab {
    /// The sections and the headline numbers.
    Overview,
    /// The libraries it imports from, and which functions.
    Imports,
    /// The functions it exports.
    Exports,
}

/// What the header read says.
enum PeState {
    Waiting,
    /// "No binary header preview for this file".
    Failed,
    Ready(Arc<glint_core::pe::PeInfo>),
}

/// `0x` and the number in upper-case hex, padded (`hex` in BinaryPreview.tsx).
fn hex_pad(n: u64, width: usize) -> String {
    format!("0x{}", hex(n, width))
}

/// The imported libraries that match a filter, or all of them (`filteredImports`): a library whose name has the
/// text keeps all its functions, any other only the functions that have it.
pub fn filter_imports(imports: &[glint_core::pe::PeImportModule], query: &str) -> Vec<glint_core::pe::PeImportModule> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return imports.to_vec();
    }
    imports
        .iter()
        .filter_map(|module| {
            if module.module.to_lowercase().contains(&q) {
                return Some(module.clone());
            }
            let symbols: Vec<String> = module
                .symbols
                .iter()
                .filter(|s| s.to_lowercase().contains(&q))
                .cloned()
                .collect();
            (!symbols.is_empty()).then(|| glint_core::pe::PeImportModule {
                symbols,
                ..module.clone()
            })
        })
        .collect()
}

/// The exports that match a filter, or all of them (`filteredExports`): by name, or `#ordinal`.
pub fn filter_exports(exports: &[glint_core::pe::PeExport], query: &str) -> Vec<glint_core::pe::PeExport> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return exports.to_vec();
    }
    exports
        .iter()
        .filter(|e| e.name.to_lowercase().contains(&q) || format!("#{}", e.ordinal) == q)
        .cloned()
        .collect()
}

/// A program's or library's header: its machine, format and subsystem, sections, imports and exports.
pub struct BinaryPreview {
    path: String,
    source: PeSource,
    state: PeState,
    tab: PeTab,
    started: Instant,
    generation: u64,
    query: Entity<InputState>,
    exports_scroll: UniformListScrollHandle,
    body_scroll: ScrollHandle,
    _query_events: Subscription,
}

impl BinaryPreview {
    /// The previewer for the program at `path`. Reading happens in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_pe_source(), window, cx)
    }

    /// [`BinaryPreview::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: PeSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter imported DLLs or functions…")));
        let events = cx.subscribe(&query, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut this = Self {
            path,
            source,
            state: PeState::Waiting,
            tab: PeTab::Overview,
            started: Instant::now(),
            generation: 0,
            query,
            exports_scroll: UniformListScrollHandle::new(),
            body_scroll: ScrollHandle::new(),
            _query_events: events,
        };
        this.read(cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another file.
    pub fn set_path(&mut self, path: String, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.read(cx);
        }
    }

    /// Scrolls the tab a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let moved = match self.tab {
            PeTab::Exports => page_list(&self.exports_scroll, down),
            _ => page_scroll(&self.body_scroll, down),
        };
        if moved {
            cx.notify();
        }
        moved
    }

    /// The tab showing.
    pub fn tab(&self) -> PeTab {
        self.tab
    }

    /// Switches tab; the filter box says what it filters.
    pub fn set_tab(&mut self, tab: PeTab, window: &mut Window, cx: &mut Context<Self>) {
        self.tab = tab;
        let placeholder = if tab == PeTab::Imports {
            t!("Filter imported DLLs or functions…")
        } else {
            t!("Filter exported symbols…")
        };
        self.query
            .update(cx, |query, cx| query.set_placeholder(placeholder, window, cx));
        cx.notify();
    }

    fn read(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.state = PeState::Waiting;
        self.started = Instant::now();
        let read = (self.source)(self.path.clone());
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            this.update(cx, |this, cx| {
                if this.generation == generation {
                    this.state = match result {
                        Ok(info) => PeState::Ready(Arc::new(info)),
                        Err(_) => PeState::Failed,
                    };
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        redraw_after_delay(cx);
        cx.notify();
    }

    /// A chip: a small label on a tinted or outlined ground.
    fn chip(
        id: &'static str,
        label: SharedString,
        color: Hsla,
        ground: Hsla,
        outlined: Option<Hsla>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .role(Role::Group)
            .aria_label(label.clone())
            .test_support()
            .px(space(2.))
            .py(space(0.5))
            .rounded(theme::size::RADIUS)
            .bg(ground)
            .text_color(color)
            .when_some(outlined, |this, border| this.border_1().border_color(border))
            .child(label)
    }

    /// Copies a symbol's name, with a toast.
    fn copy_symbol(text: &str, window: &mut Window, cx: &mut App) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        window.push_notification(Notification::success(t!("Copied “{text}”", text = text)), cx);
    }

    /// The Sections tab: the header's facts, the security features, and a card for each section.
    fn overview(&self, info: &glint_core::pe::PeInfo, t: &theme::Tokens, mono: &SharedString) -> AnyElement {
        let max_bytes = info
            .sections
            .iter()
            .map(|s| s.raw_size.max(s.virtual_size))
            .max()
            .unwrap_or(1)
            .max(1);
        let language = crate::i18n::language();
        let mut facts: Vec<(SharedString, SharedString, bool)> = Vec::new();
        if let Some(ms) = info.timestamp_ms.filter(|&ms| ms != 0) {
            facts.push((
                t!("Compiled"),
                glint_model::format::format_date(ms, None, language).into(),
                false,
            ));
        }
        facts.push((t!("Entry point"), hex_pad(u64::from(info.entry_point), 8).into(), true));
        facts.push((t!("Image base"), hex_pad(info.image_base, 12).into(), true));
        facts.push((
            t!("Target OS / Linker"),
            t!(
                "Windows {os} · Linker {linker}",
                os = info.os_version,
                linker = info.linker_version
            ),
            false,
        ));
        if info.checksum != 0 {
            facts.push((t!("PE checksum"), hex_pad(u64::from(info.checksum), 8).into(), true));
        }
        let features = (!info.features.is_empty()).then(|| {
            row_start()
                .id("pe-features")
                .role(Role::List)
                .aria_label(t!("Security features"))
                .test_support()
                .flex_wrap()
                .items_center()
                .gap(space(1.5))
                .child(
                    icon(IconName::ShieldCheck)
                        .size_3p5()
                        .flex_none()
                        .text_color(t.muted_foreground),
                )
                .children(info.features.iter().enumerate().map(|(i, feature)| {
                    div()
                        .id(("pe-feature", i))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(feature.clone()))
                        .test_support()
                        .px(space(1.5))
                        .py(space(0.5))
                        .rounded(theme::size::RADIUS_SM)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.muted.opacity(0.5))
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(SharedString::from(feature.clone()))
                }))
        });
        let dl = col()
            .id("pe-facts")
            .role(Role::DescriptionList)
            .aria_label(t!("Header"))
            .test_support()
            .gap(space(1.))
            .p(space(2.5))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.card.opacity(0.5))
            .children(facts.into_iter().enumerate().map(|(i, (name, value, is_mono))| {
                row_start()
                    .id(("pe-fact", i))
                    .role(Role::Group)
                    .aria_label(format!("{name}: {value}"))
                    .test_support()
                    .gap(space(3.))
                    .child(
                        div()
                            .flex_none()
                            .w(space(36.))
                            .text_color(t.muted_foreground)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .when(is_mono, |this| this.font_family(mono.clone()))
                            .child(value),
                    )
            }));
        let amber = theme::tailwind(if t.dark { "amber-400" } else { "amber-700" }).unwrap_or(t.foreground);
        let sections = col()
            .id("pe-sections")
            .role(Role::List)
            .aria_label(t!("PE Sections"))
            .test_support()
            .gap(space(1.5))
            .child(
                align_row(1.5)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.foreground)
                    .child(icon(IconName::Layers).size_3p5().text_color(t.primary))
                    .child(t!("PE Sections")),
            )
            .children(info.sections.iter().enumerate().map(|(i, s)| {
                let widest = s.raw_size.max(s.virtual_size);
                let percent = ((f64::from(widest) / f64::from(max_bytes)) * 100.).round().max(2.) as f32;
                let name: SharedString = if s.name.is_empty() {
                    t!("(unnamed)")
                } else {
                    s.name.clone().into()
                };
                let sizes = t!(
                    "Raw {raw} · Virtual {virtual_size}",
                    raw = format_size(u64::from(s.raw_size)),
                    virtual_size = format_size(u64::from(s.virtual_size))
                );
                col()
                    .id(("pe-section", i))
                    .role(Role::ListItem)
                    .aria_label(format!("{name}: {sizes}"))
                    .test_support()
                    .px(space(2.5))
                    .py(space(1.5))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card.opacity(0.4))
                    .child(
                        row_start()
                            .justify_between()
                            .gap(space(2.))
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.foreground)
                                    .child(name),
                            )
                            .child(align_row(1.).children(s.flags.iter().map(|flag| {
                                let execute = flag == "X";
                                div()
                                    .px(space(1.))
                                    .rounded(theme::size::RADIUS_SM)
                                    .font_family(mono.clone())
                                    .text_size(theme::size::XXS)
                                    .bg(if execute { amber.opacity(0.15) } else { t.muted })
                                    .text_color(if execute { amber } else { t.muted_foreground })
                                    .child(SharedString::from(flag.clone()))
                            }))),
                    )
                    .child(
                        row_start()
                            .mt(space(1.))
                            .justify_between()
                            .gap(space(2.))
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(sizes)
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .child(format!("RVA {}", hex_pad(u64::from(s.virtual_address), 6))),
                            ),
                    )
                    .child(
                        div()
                            .mt(space(1.))
                            .h(space(1.))
                            .w_full()
                            .overflow_hidden()
                            .rounded_full()
                            .bg(t.muted)
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui_kit::relative(percent / 100.))
                                    .rounded_full()
                                    .bg(t.primary.opacity(0.7)),
                            ),
                    )
            }));
        col()
            .gap(space(3.))
            .children(features)
            .child(dl)
            .child(sections)
            .into_any_element()
    }

    /// The Imports tab: a card for each library, with its functions as buttons that copy their names.
    fn imports(
        &self,
        info: &glint_core::pe::PeInfo,
        query: &str,
        t: &theme::Tokens,
        mono: &SharedString,
    ) -> AnyElement {
        let shown = filter_imports(&info.imports, query);
        if shown.is_empty() {
            return div()
                .id("pe-no-imports")
                .role(Role::Status)
                .aria_label(t!("No matching imported modules or functions."))
                .test_support()
                .py(space(8.))
                .text_center()
                .text_color(t.muted_foreground)
                .child(t!("No matching imported modules or functions."))
                .into_any_element();
        }
        col()
            .id("pe-imports")
            .role(Role::List)
            .aria_label(t!("Imports"))
            .test_support()
            .gap(space(2.))
            .children(shown.into_iter().enumerate().map(|(i, module)| {
                let count = if module.total_count == 1 {
                    t!("{n} symbol", n = format_count(module.total_count))
                } else {
                    t!("{n} symbols", n = format_count(module.total_count))
                };
                let more = module.total_count.saturating_sub(module.symbols.len());
                col()
                    .id(("pe-import", i))
                    .role(Role::ListItem)
                    .aria_label(format!("{}: {count}", module.module))
                    .test_support()
                    .p(space(2.5))
                    .rounded(theme::size::RADIUS_LG)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.card.opacity(0.4))
                    .child(
                        row_start()
                            .justify_between()
                            .gap(space(2.))
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.foreground)
                                    .child(SharedString::from(module.module.clone())),
                            )
                            .child(
                                div()
                                    .text_size(theme::size::XXS)
                                    .text_color(t.muted_foreground)
                                    .child(count),
                            ),
                    )
                    .when(!module.symbols.is_empty(), |this| {
                        this.child(
                            row_start()
                                .mt(space(1.5))
                                .flex_wrap()
                                .gap(space(1.))
                                .children(module.symbols.iter().enumerate().map(|(k, symbol)| {
                                    let name = symbol.clone();
                                    Button::new(("pe-symbol", i * 100_000 + k))
                                        .outline()
                                        .compact()
                                        .label(SharedString::from(symbol.clone()))
                                        .tooltip(t!("Click to copy symbol name"))
                                        .on_click(move |_, window, cx| Self::copy_symbol(&name, window, cx))
                                }))
                                .when(more > 0, |this| {
                                    this.child(
                                        div()
                                            .px(space(1.))
                                            .py(space(0.5))
                                            .text_size(theme::size::XXS)
                                            .text_color(t.muted_foreground)
                                            .child(t!("+{n} more", n = format_count(more))),
                                    )
                                }),
                        )
                    })
            }))
            .into_any_element()
    }

    /// The Exports tab: a virtualized list of the symbols, each with its address and a copy button.
    fn exports(
        &self,
        info: &glint_core::pe::PeInfo,
        query: &str,
        t: &theme::Tokens,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let shown = Arc::new(filter_exports(&info.exports, query));
        if shown.is_empty() {
            let text = if info.export_count == 0 {
                t!("This binary exports no symbols.")
            } else {
                t!("No matching exported symbols.")
            };
            return div()
                .id("pe-no-exports")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .py(space(8.))
                .text_center()
                .text_color(t.muted_foreground)
                .child(text)
                .into_any_element();
        }
        let rows = shown.len();
        let tokens = t.clone();
        let mono = mono.clone();
        let list = uniform_list(
            "pe-exports",
            rows,
            cx.processor(move |_, range: Range<usize>, _, _| {
                range
                    .filter_map(|i| shown.get(i).map(|export| (i, export.clone())))
                    .map(|(i, export)| {
                        let name = export.name.clone();
                        row_start()
                            .id(("pe-export", i))
                            .role(Role::ListItem)
                            .aria_label(format!("#{} {}", export.ordinal, export.name))
                            .test_support()
                            .h(space(8.))
                            .px(space(2.5))
                            .gap(space(2.))
                            .items_center()
                            .justify_between()
                            .rounded(theme::size::RADIUS)
                            .hover(|this| this.bg(tokens.accent.opacity(0.4)))
                            .child(
                                row_start()
                                    .min_w_0()
                                    .gap(space(2.))
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_family(mono.clone())
                                            .text_size(theme::size::XXS)
                                            .text_color(tokens.muted_foreground)
                                            .child(format!("#{}", export.ordinal)),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .font_family(mono.clone())
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(tokens.foreground)
                                            .child(SharedString::from(export.name.clone())),
                                    ),
                            )
                            .child(
                                row_start()
                                    .flex_none()
                                    .gap(space(1.5))
                                    .child(
                                        div()
                                            .font_family(mono.clone())
                                            .text_size(theme::size::XXS)
                                            .text_color(tokens.muted_foreground)
                                            .child(hex_pad(u64::from(export.rva), 6)),
                                    )
                                    .child(
                                        Button::new(("pe-export-copy", i))
                                            .ghost()
                                            .compact()
                                            .icon(IconName::ClipboardCopy)
                                            .tooltip(t!("Copy symbol name"))
                                            .accessibility_label(t!("Copy symbol name"))
                                            .on_click(move |_, window, cx| Self::copy_symbol(&name, window, cx)),
                                    ),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.exports_scroll)
        .size_full();
        let cut = (info.export_count > info.exports.len() && query.trim().is_empty()).then(|| {
            let text = t!(
                "Showing first {shown} of {total} exports.",
                shown = format_count(info.exports.len()),
                total = format_count(info.export_count)
            );
            div()
                .id("pe-exports-cut")
                .role(Role::Status)
                .aria_label(text.clone())
                .test_support()
                .pt(space(2.))
                .text_center()
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(text)
        });
        col()
            .size_full()
            .min_h_0()
            .child(
                div()
                    .id("pe-export-list")
                    .role(Role::List)
                    .aria_label(t!("Exports"))
                    .test_support()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(list)
                    .child(Scrollbar::vertical(&self.exports_scroll).viewport_from_layout()),
            )
            .children(cut)
            .into_any_element()
    }

    /// One tab of the strip.
    fn tab_button(&self, tab: PeTab, label: SharedString, cx: &mut Context<Self>) -> impl IntoElement {
        let id = match tab {
            PeTab::Overview => "pe-tab-sections",
            PeTab::Imports => "pe-tab-imports",
            PeTab::Exports => "pe-tab-exports",
        };
        Button::new(id)
            .ghost()
            .small()
            .label(label.clone())
            .selected(self.tab == tab)
            .accessibility_label(label)
            .on_click(cx.listener(move |this, _, window, cx| this.set_tab(tab, window, cx)))
    }
}

impl Render for BinaryPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("binary-preview")
            .size_full()
            .min_h_0()
            .text_start()
            .text_size(theme::size::XS)
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let info = match &self.state {
            PeState::Waiting => return root.child(waiting(self.started, None, cx)),
            PeState::Failed => {
                return root.child(big_icon(
                    &self.path,
                    Some(t!("No binary header preview for this file")),
                    cx,
                ));
            }
            PeState::Ready(info) => info.clone(),
        };
        let mono = theme::mono_font(cx);
        let query = self.query.read(cx).value().to_string();
        let violet = theme::tailwind("violet-500").unwrap_or(t.primary);
        let kind = if info.is_dll { t!("DLL") } else { t!("Executable") };
        let chips = row_start()
            .flex_wrap()
            .items_center()
            .gap(space(1.5))
            .px(space(3.))
            .py(space(2.))
            .border_b_1()
            .border_color(t.border)
            .bg(t.chrome.opacity(0.5))
            .child(
                row_start()
                    .id("pe-machine")
                    .role(Role::Group)
                    .aria_label(SharedString::from(info.machine.clone()))
                    .test_support()
                    .gap(space(1.))
                    .px(space(2.))
                    .py(space(0.5))
                    .rounded(theme::size::RADIUS)
                    .bg(t.primary.opacity(0.15))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.primary)
                    .child(icon(IconName::Cpu).size_3())
                    .child(SharedString::from(info.machine.clone())),
            )
            .child(Self::chip(
                "pe-format",
                format!("{} · {kind}", info.format).into(),
                t.foreground,
                t.background,
                Some(t.border),
            ))
            .child(Self::chip(
                "pe-subsystem",
                info.subsystem.clone().into(),
                t.muted_foreground,
                t.background,
                Some(t.border),
            ))
            // PORT ME(coordinator): the signature chip (`SignatureChip`), a component of its own.
            .when(info.is_dotnet, |this| {
                this.child(Self::chip(
                    "pe-dotnet",
                    t!(".NET CLR"),
                    violet,
                    violet.opacity(0.15),
                    None,
                ))
            })
            .child(
                row_start()
                    .id("pe-elapsed")
                    .role(Role::Group)
                    .aria_label(t!("Parsed directly from the PE header"))
                    .test_support()
                    .gap(space(1.))
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(icon(IconName::Zap).size_3().text_color(t.primary))
                    .child(glint_model::format::format_duration(info.elapsed_ms)),
            );
        let tabs = align_row(1.)
            .id("pe-tabs")
            .role(Role::TabList)
            .aria_label(t!("Header"))
            .test_support()
            .flex_none()
            .px(space(3.))
            .py(space(1.5))
            .border_b_1()
            .border_color(t.border)
            .child(self.tab_button(PeTab::Overview, t!("Sections ({n})", n = info.sections.len()), cx))
            .child(self.tab_button(PeTab::Imports, t!("Imports ({n})", n = info.imports.len()), cx))
            .child(self.tab_button(
                PeTab::Exports,
                t!("Exports ({n})", n = format_count(info.export_count)),
                cx,
            ));
        let filter = (self.tab != PeTab::Overview).then(|| {
            align_row(2.)
                .flex_none()
                .px(space(3.))
                .py(space(1.5))
                .border_b_1()
                .border_color(t.border)
                .child(
                    icon(IconName::Search)
                        .size_3p5()
                        .flex_none()
                        .text_color(t.muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.query).small().appearance(false).aria_label(
                            if self.tab == PeTab::Imports {
                                t!("Filter imported DLLs or functions…")
                            } else {
                                t!("Filter exported symbols…")
                            },
                        )),
                )
        });
        let body = match self.tab {
            PeTab::Overview => div()
                .id("pe-body")
                .flex_1()
                .min_h_0()
                .track_scroll(&self.body_scroll)
                .overflow_y_scroll()
                .p(space(3.))
                .child(self.overview(&info, &t, &mono))
                .into_any_element(),
            PeTab::Imports => div()
                .id("pe-body")
                .flex_1()
                .min_h_0()
                .track_scroll(&self.body_scroll)
                .overflow_y_scroll()
                .p(space(3.))
                .child(self.imports(&info, &query, &t, &mono))
                .into_any_element(),
            PeTab::Exports => div()
                .id("pe-body")
                .flex_1()
                .min_h_0()
                .p(space(3.))
                .child(self.exports(&info, &query, &t, &mono, cx))
                .into_any_element(),
        };
        root.child(chips).child(tabs).children(filter).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A file of `size` bytes whose byte at `i` is `pattern(i)`, the paths it was read from, and the paths a copy
    /// was asked for.
    #[allow(clippy::type_complexity)] // a test helper returning the fake and its two logs
    fn fake(size: u64, pattern: fn(u64) -> u8) -> (HexSource, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
        let read_paths = Arc::new(Mutex::new(Vec::new()));
        let copied = Arc::new(Mutex::new(Vec::new()));
        let (reads, copies) = (read_paths.clone(), copied.clone());
        let source = HexSource {
            bytes: Arc::new(move |path, offset, len| {
                if let Ok(mut log) = reads.lock() {
                    log.push(path);
                }
                let end = size.min(offset + u64::from(len));
                let bytes: Vec<u8> = (offset.min(size)..end).map(pattern).collect();
                Box::pin(async move { Ok((size, bytes)) })
            }),
            copy: Arc::new(move |path| {
                if let Ok(mut log) = copies.lock() {
                    log.push(path.clone());
                }
                Box::pin(async move { Ok(format!("C:\\temp\\copy-of-{}", glint_model::paths::base_name(&path))) })
            }),
        };
        (source, read_paths, copied)
    }

    fn counting(i: u64) -> u8 {
        (i % 256) as u8
    }

    fn open(cx: &mut TestAppContext, path: &str, source: HexSource) -> (Entity<HexPreview>, VisualTestContext) {
        cx.update(testing::init);
        let path = path.to_string();
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| HexPreview::with_source(path, source, window, cx))
        });
        (view, visual)
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the bytes never arrived");
    }

    /// Frames until the rows have been drawn, asked for their windows and drawn again with their bytes.
    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..4 {
            visual.update(|window, cx| window.render_frame(cx));
            settle(visual, |_| true);
            std::thread::sleep(Duration::from_millis(10));
            visual.run_until_parked();
        }
    }

    /// Waits for the first rows to be on screen.
    fn ready(view: &Entity<HexPreview>, visual: &mut VisualTestContext) {
        settle(visual, |cx| view.read_with(cx, |v, _| v.size().is_some()));
        frames(visual);
    }

    fn press(visual: &mut VisualTestContext, view: &Entity<HexPreview>, key: &str) {
        visual.update(|window, cx| {
            let focus = view.read(cx).focus.clone();
            window.focus(&focus, cx);
            window.press(key, cx);
        });
        frames(visual);
    }

    fn selected(visual: &VisualTestContext, view: &Entity<HexPreview>) -> Option<u64> {
        view.read_with(visual, |v, _| v.selected())
    }

    #[test]
    fn a_row_is_offset_hex_and_characters_in_hexyls_classes() {
        let row = row_text(0x1F00, b"Hi\0\x01 \xFF", 16);
        assert_eq!(row.offset, "00001F00");
        // Three characters a byte, one more between the eighth and ninth, none after the last.
        assert_eq!(row.hex.len(), 48);
        assert!(row.hex.starts_with("48 69 00 01 20 FF "));
        assert_eq!(row.hex_spans.len(), 6);
        assert_eq!(&row.hex[row.hex_spans[1].0.clone()], "69");
        assert_eq!(row.chars, "Hi⋄• ×");
        assert_eq!(
            row.char_spans.iter().map(|(_, class)| *class).collect::<Vec<_>>(),
            [
                ByteClass::Text,
                ByteClass::Text,
                ByteClass::Null,
                ByteClass::Control,
                ByteClass::Space,
                ByteClass::High
            ]
        );
        // The spans are byte ranges of the text: ⋄ is three bytes.
        assert_eq!(&row.chars[row.char_spans[2].0.clone()], "⋄");
        assert_eq!(&row.chars[row.char_spans[5].0.clone()], "×");
        // Eight to a row, when the pane is narrow: no gap in the middle.
        let narrow = row_text(0, &[1, 2, 3, 4, 5, 6, 7, 8], 8);
        assert_eq!(narrow.hex, "01 02 03 04 05 06 07 08");
        // A whitespace byte other than the space shows as an underscore, as hexyl draws it.
        assert_eq!(row_text(0, b"\n", 8).chars, "_");
    }

    #[test]
    fn a_click_on_the_hex_text_finds_its_byte() {
        for (character, byte) in [
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 1),
            (23, 7),
            (24, 7),
            (25, 8),
            (26, 8),
            (28, 9),
            (46, 15),
        ] {
            assert_eq!(byte_of_hex_char(character), byte, "character {character}");
        }
    }

    #[gpui_kit::test]
    fn the_first_rows_show_their_bytes_and_arrows_move_the_selection(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, reads, _) = fake(100_000, counting);
        let (view, mut visual) = open(cx, "C:\\data\\blob.bin", source);
        // Reading is under way, and nothing is selected.
        assert!(!view.read_with(&visual, |v, _| v.size().is_some()));
        ready(&view, &mut visual);
        assert_eq!(
            reads.lock().ok().and_then(|r| r.first().cloned()).as_deref(),
            Some("C:\\data\\blob.bin")
        );
        let size_text = format!("{} ({} bytes)", format_size(100_000), format_count(100_000));
        assert!(has(&mut visual, &size_text), "the size, in full");
        assert!(has(
            &mut visual,
            "00000000: 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F"
        ));
        assert!(has(
            &mut visual,
            "00000010: 10 11 12 13 14 15 16 17 18 19 1A 1B 1C 1D 1E 1F"
        ));
        assert!(has(&mut visual, "Click a byte, or go to an offset"));
        // The arrows start from the first byte and move one, a row, a page.
        press(&mut visual, &view, "right");
        // NOTE(port): the first arrow moves from byte 0, not onto it (`Math.max(at, 0)` in the TypeScript).
        assert_eq!(selected(&visual, &view), Some(1));
        press(&mut visual, &view, "right");
        assert_eq!(selected(&visual, &view), Some(2));
        let expected = format!(
            "0x00000002 (2)   02 = 2   u16 {}   u32 {}",
            format_count(0x0302),
            format_count(0x0504_0302)
        );
        assert!(has(&mut visual, &expected), "{expected}");
        press(&mut visual, &view, "down");
        assert_eq!(selected(&visual, &view), Some(18));
        press(&mut visual, &view, "left");
        assert_eq!(selected(&visual, &view), Some(17));
        press(&mut visual, &view, "home");
        assert_eq!(selected(&visual, &view), Some(16));
        press(&mut visual, &view, "end");
        assert_eq!(selected(&visual, &view), Some(31));
        press(&mut visual, &view, "up");
        press(&mut visual, &view, "up");
        assert_eq!(selected(&visual, &view), Some(0), "the first byte is the start");
        // A printable byte is shown as its character too.
        view.update_in(&mut visual, |v, _, cx| v.select_at(0x41, cx));
        frames(&mut visual);
        let printable = format!(
            "0x00000041 (65)   41 = 65 = 'A'   u16 {}   u32 {}",
            format_count(0x4241),
            format_count(0x4443_4241)
        );
        assert!(has(&mut visual, &printable), "{printable}");
        // A file longer than a window: the last byte's window is read when it's reached; past the end stays inside.
        view.update_in(&mut visual, |v, _, cx| v.select_at(i128::from(u64::MAX), cx));
        assert_eq!(selected(&visual, &view), Some(99_999), "kept inside the file");
        frames(&mut visual);
        let last = format!(
            "0x0001869F ({})   {} = {}",
            format_count(99_999),
            hex(99_999 % 256, 2),
            99_999 % 256
        );
        assert!(has(&mut visual, &last), "{last}");
    }

    #[gpui_kit::test]
    fn page_down_and_page_up_scroll_the_rows_for_quick_look(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, _, _) = fake(100_000, counting);
        let (view, mut visual) = open(cx, "C:\\data\\blob.bin", source);
        ready(&view, &mut visual);
        assert!(
            !view.update_in(&mut visual, |v, _, cx| v.page(false, cx)),
            "already at the top"
        );
        assert!(
            view.update_in(&mut visual, |v, _, cx| v.page(true, cx)),
            "a long file scrolls"
        );
        frames(&mut visual);
        // The rows after the first page are the ones on screen now.
        assert!(!has(
            &mut visual,
            "00000000: 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F"
        ));
        assert!(view.update_in(&mut visual, |v, _, cx| v.page(false, cx)));
        frames(&mut visual);
        assert!(has(
            &mut visual,
            "00000000: 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F"
        ));
    }

    #[gpui_kit::test]
    fn go_to_takes_hex_and_decimal_and_says_what_is_wrong(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, _, _) = fake(4096, counting);
        let (view, mut visual) = open(cx, "C:\\data\\small.bin", source);
        ready(&view, &mut visual);
        let go = |visual: &mut VisualTestContext, text: &str| {
            let text = text.to_string();
            visual.update(|window, cx| {
                view.update(cx, |v, cx| {
                    v.typed.update(cx, |input, cx| input.set_value(text, window, cx));
                    v.go(window, cx);
                });
            });
            visual.run_until_parked();
        };
        go(&mut visual, "0x20");
        assert_eq!(selected(&visual, &view), Some(32));
        go(&mut visual, "256");
        assert_eq!(selected(&visual, &view), Some(256));
        // Not an offset, and past the end: the selection stays, and a notification says why.
        go(&mut visual, "zzz");
        assert_eq!(selected(&visual, &view), Some(256));
        go(&mut visual, "4096");
        assert_eq!(selected(&visual, &view), Some(256));
        go(&mut visual, "FFh");
        assert_eq!(selected(&visual, &view), Some(255));
        // The box is named.
        assert!(has(&mut visual, "Go to offset"));
    }

    #[gpui_kit::test]
    fn a_huge_file_is_shown_a_page_at_a_time(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let five_gib = 5u64 << 30;
        let (source, _, _) = fake(five_gib, |i| (i.wrapping_mul(7) % 251) as u8);
        let (view, mut visual) = open(cx, "D:\\images\\disk.img", source);
        ready(&view, &mut visual);
        assert!(has(&mut visual, "The part before"));
        assert!(has(&mut visual, "The part after"));
        let page = PAGE_ROWS * 16;
        assert!(has(
            &mut visual,
            &format!("The part shown: {}–{}", hex(0, 8), hex(page - 1, 8))
        ));
        // Going into the third page moves to it: the rows are that page's, read from where they are.
        let target = 2 * page + 5;
        view.update_in(&mut visual, |v, _, cx| v.select_at(i128::from(target), cx));
        frames(&mut visual);
        assert!(has(
            &mut visual,
            &format!("The part shown: {}–{}", hex(2 * page, 8), hex(3 * page - 1, 8))
        ));
        let bytes: Vec<String> = (0..16u64)
            .map(|k| hex((2 * page + k).wrapping_mul(7) % 251, 2))
            .collect();
        assert!(
            has(&mut visual, &format!("{}: {}", hex(2 * page, 8), bytes.join(" "))),
            "the page's first row"
        );
        // The buttons step by a page, from the page's start.
        visual.update(|window, cx| window.click("hex-before", cx));
        visual.run_until_parked();
        assert_eq!(selected(&visual, &view), Some(page));
        // Only the windows near the rows on screen are kept.
        assert!(view.read_with(&visual, |v, _| v.chunks.len()) <= KEPT_WINDOWS);
    }

    #[gpui_kit::test]
    fn empty_failed_and_archived_files_each_say_what_they_are(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (source, _, _) = fake(0, counting);
        let (view, mut visual) = open(cx, "C:\\data\\empty.bin", source);
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.size() == Some(0)));
        assert!(has(&mut visual, "This file is empty."));

        let failing = HexSource {
            bytes: Arc::new(|_, _, _| Box::pin(async { Err("Access is denied. (os error 5)".to_string()) })),
            copy: Arc::new(|path| Box::pin(async move { Ok(path) })),
        };
        let (view, mut visual) = open(cx, "C:\\data\\locked.bin", failing);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.readable, Readable::Failed(_)))
        });
        assert!(has(&mut visual, "Access is denied. (os error 5)"));

        // In an archive, the bytes come from a copy made for the preview.
        let (source, reads, copies) = fake(300, counting);
        let (view, mut visual) = open(cx, "C:\\backup\\old.zip\\inside\\data.bin", source);
        ready(&view, &mut visual);
        assert_eq!(
            copies.lock().map(|c| c.clone()).unwrap_or_default(),
            ["C:\\backup\\old.zip\\inside\\data.bin"]
        );
        assert_eq!(
            reads.lock().ok().and_then(|r| r.first().cloned()).as_deref(),
            Some("C:\\temp\\copy-of-data.bin")
        );
        assert!(has(
            &mut visual,
            "00000000: 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F"
        ));
        // A zip file itself is read where it is.
        let (source, reads, copies) = fake(300, counting);
        let (view, mut visual) = open(cx, "C:\\backup\\old.zip", source);
        ready(&view, &mut visual);
        assert!(copies.lock().map(|c| c.is_empty()).unwrap_or(false));
        assert_eq!(
            reads.lock().ok().and_then(|r| r.first().cloned()).as_deref(),
            Some("C:\\backup\\old.zip")
        );
    }

    #[gpui_kit::test]
    fn the_arabic_interface_names_the_parts_and_keeps_the_bytes_left_to_right(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (source, _, _) = fake(2000, counting);
        let (view, mut visual) = open(cx, "C:\\مستندات\\ملف.bin", source);
        ready(&view, &mut visual);
        assert!(has(&mut visual, &t!("Go to offset")));
        assert!(has(&mut visual, &t!("Bytes")));
        assert!(has(
            &mut visual,
            "00000000: 00 01 02 03 04 05 06 07 08 09 0A 0B 0C 0D 0E 0F"
        ));
        assert!(has(&mut visual, &t!("Click a byte, or go to an offset")));
        testing::set_rtl(false);
    }

    // ---- The program's header

    fn pe_info() -> glint_core::pe::PeInfo {
        use glint_core::pe::{PeExport, PeImportModule, PeSection};
        glint_core::pe::PeInfo {
            format: "PE32+".into(),
            machine: "x64".into(),
            subsystem: "Windows GUI".into(),
            is_dll: false,
            is_dotnet: true,
            is_signed: false,
            timestamp_ms: Some(1_700_000_000_000),
            linker_version: "14.38".into(),
            os_version: "10.0".into(),
            entry_point: 0x1234,
            image_base: 0x1_4000_0000,
            checksum: 0xABCD,
            features: vec!["ASLR".into(), "DEP".into()],
            sections: vec![
                PeSection {
                    name: ".text".into(),
                    virtual_size: 4096,
                    virtual_address: 0x1000,
                    raw_size: 4096,
                    raw_offset: 0x400,
                    flags: vec!["R".into(), "X".into()],
                },
                PeSection {
                    name: String::new(),
                    virtual_size: 100,
                    virtual_address: 0x2000,
                    raw_size: 512,
                    raw_offset: 0x1400,
                    flags: vec!["R".into()],
                },
            ],
            imports: vec![
                PeImportModule {
                    module: "KERNEL32.dll".into(),
                    symbols: vec!["CreateFileW".into(), "ReadFile".into()],
                    total_count: 90,
                },
                PeImportModule {
                    module: "ديل.dll".into(),
                    symbols: vec!["Open".into()],
                    total_count: 1,
                },
            ],
            export_count: 1200,
            exports: (1..=3)
                .map(|n| PeExport {
                    ordinal: n,
                    name: format!("Export{n}"),
                    rva: 0x2000 + n,
                })
                .collect(),
            elapsed_ms: 0.4,
        }
    }

    #[test]
    fn the_filters_keep_whole_modules_for_a_name_and_matching_functions_for_the_rest() {
        let info = pe_info();
        assert_eq!(filter_imports(&info.imports, "  ").len(), 2);
        // A library's name keeps all its functions.
        let by_name = filter_imports(&info.imports, "kernel");
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].symbols.len(), 2);
        // A function keeps only itself, and its module's total.
        let by_function = filter_imports(&info.imports, "readfile");
        assert_eq!(by_function[0].symbols, ["ReadFile"]);
        assert_eq!(by_function[0].total_count, 90);
        assert!(filter_imports(&info.imports, "zzz").is_empty());
        // Exports: by name, or by #ordinal.
        assert_eq!(filter_exports(&info.exports, "export2").len(), 1);
        assert_eq!(filter_exports(&info.exports, "#3")[0].name, "Export3");
        assert!(filter_exports(&info.exports, "#9").is_empty());
        assert_eq!(filter_exports(&info.exports, "").len(), 3);
    }

    #[test]
    fn programs_are_told_by_their_extension() {
        for path in ["C:\\a\\x.exe", "C:\\a\\X.DLL", "C:\\a\\d.sys", "C:\\a\\m.node"] {
            assert!(is_pe_candidate(path), "{path}");
        }
        for path in ["C:\\a\\x.txt", "C:\\a\\exe", "C:\\a\\x.exe.bak"] {
            assert!(!is_pe_candidate(path), "{path}");
        }
    }

    fn open_binary(
        cx: &mut TestAppContext,
        path: &str,
        source: PeSource,
    ) -> (Entity<BinaryPreview>, VisualTestContext) {
        cx.update(testing::init);
        let path = path.to_string();
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| BinaryPreview::with_source(path, source, window, cx))
        });
        (view, visual)
    }

    #[gpui_kit::test]
    fn a_programs_header_shows_its_sections_imports_and_exports(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let source: PeSource = Arc::new(|_| Box::pin(async { Ok(pe_info()) }));
        let (view, mut visual) = open_binary(cx, "C:\\apps\\tool.exe", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, PeState::Ready(_)))
        });
        for label in [
            "x64",
            "PE32+ · Executable",
            "Windows GUI",
            ".NET CLR",
            "Sections (2)",
            "Imports (2)",
            &format!("Exports ({})", format_count(1200)),
            "Entry point: 0x00001234",
            "Image base: 0x000140000000",
            "PE checksum: 0x0000ABCD",
            "Target OS / Linker: Windows 10.0 · Linker 14.38",
        ] {
            assert!(has(&mut visual, label), "{label}");
        }
        assert!(has(&mut visual, ".text: Raw 4.0 KB · Virtual 4.0 KB"));
        assert!(has(&mut visual, "(unnamed): Raw 512 B · Virtual 100 B"));
        // The imports tab lists the libraries, Arabic names included, and the filter narrows them.
        visual.update(|window, cx| window.click("pe-tab-imports", cx));
        visual.run_until_parked();
        assert_eq!(view.read_with(&visual, |v, _| v.tab()), PeTab::Imports);
        assert!(has(&mut visual, "KERNEL32.dll: 90 symbols"));
        assert!(has(&mut visual, "ديل.dll: 1 symbol"));
        visual
            .update(|window, cx| view.update(cx, |v, cx| v.query.update(cx, |q, cx| q.set_value("openx", window, cx))));
        visual.run_until_parked();
        assert!(has(&mut visual, "No matching imported modules or functions."));
        // The exports tab lists them with how many there are in all.
        visual.update(|window, cx| window.click("pe-tab-exports", cx));
        visual.run_until_parked();
        visual.update(|window, cx| view.update(cx, |v, cx| v.query.update(cx, |q, cx| q.set_value("", window, cx))));
        visual.run_until_parked();
        assert!(has(&mut visual, "#2 Export2"));
        assert!(has(
            &mut visual,
            &format!("Showing first 3 of {} exports.", format_count(1200))
        ));
    }

    #[gpui_kit::test]
    fn a_file_without_a_header_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let source: PeSource = Arc::new(|_| Box::pin(async { Err("Not a PE file".to_string()) }));
        let (view, mut visual) = open_binary(cx, "C:\\apps\\fake.exe", source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.state, PeState::Failed))
        });
        assert!(has(&mut visual, "No binary header preview for this file"));
    }

    #[test]
    fn every_icon_the_hex_views_draw_is_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::ChevronLeft,
            IconName::ChevronRight,
            IconName::ShieldCheck,
            IconName::Cpu,
            IconName::Zap,
            IconName::Layers,
            IconName::Search,
            IconName::ClipboardCopy,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }
}
