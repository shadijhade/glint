//! Jupyter notebooks as they read (`NotebookView` in NotebookView.tsx): Markdown cells rendered, code
//! highlighted, and each cell's outputs (text, errors, pictures) under it. Nothing in a notebook runs.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `NotebookView`, `MAX_BYTES`                  | [`NotebookView`]; the file is read by [`NotebookSource`] and parsed in the background |
//! | the JSON reading in the `useEffect`          | [`parse_notebook`]: [`Notebook`], [`Cell`], [`Output`]        |
//! | `joined`                                     | [`joined`]: a string, or an array of strings joined with nothing between |
//! | `languageNamed`                              | [`language_named`]                                            |
//! | a Markdown cell's `MarkdownView`             | the kit's `TextView` over the cell's Markdown, with pictures beside the notebook or written in the cell |
//! | a code cell's `SyntaxHighlighter`            | the same `TextView` over the code as a fenced block ([`fenced`]), highlighted by the language the notebook names |
//! | `OutputView`                                 | [`Output`]: red text for an error, a picture (PNG, JPEG, GIF, SVG), else the text |
//!
//! The cells are in a `list` (their heights differ and are not known until drawn), so only the cells on screen
//! are laid out, and a cell's `TextView` is made the first time the cell is drawn. A notebook's pictures are
//! decoded as they come on screen and let go of when the view goes or shows another notebook.
//!
//! **Bounds.** NOTE(port): the TypeScript asked for 32 MB of the file, but glint-core's `read_text_preview`
//! never reads more than 4 MB, so a notebook over 4 MB was always "too big to preview". It still is. Beyond the
//! TypeScript, a cell or output past [`MAX_LINES`] lines is cut with a note saying how many lines were left out,
//! because a notebook's output can be megabytes of text and a cell is laid out whole.
//!
//! Not ported:
//! - A text output is not selectable (it is a plain block, the TypeScript's `select-text`), and it does not scroll
//!   inside the cell when it is taller than 20 rem: it is cut at [`OUTPUT_LINES`] lines instead.
//! - `attachment:` pictures in Markdown cells (the TypeScript didn't draw them either).
//! - HTML outputs and widgets: the TypeScript showed their `text/plain` too.

use super::markdown::{base64_decode, link_action, picture_source, LinkAction};
use super::text::LANGUAGE_NAMES;
use super::text::{big_icon, editor_language, language_for, name_of, note_centered, redraw_after_delay, waiting};
use crate::kit::prelude::*;
use crate::kit::{TextView, TextViewState};
use crate::{actions, core_bridge, t, theme};
use glint_core::fsx::TextPreview as TextRead;
use glint_model::paths::{parent_path, volume_of};
use gpui_kit::{
    img, list, Image, ImageFormat, ImageSource, ListAlignment, ListState, ObjectFit, StyledImage as _, Task,
};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The most of a notebook asked for (`MAX_BYTES`); glint-core gives at most 4 MB of it.
pub const MAX_BYTES: usize = 32 * 1024 * 1024;
/// The most lines of a cell drawn.
pub const MAX_LINES: usize = 2000;
/// The most lines of a text output drawn (`max-h-80` is 20 rem, about 20 lines; the TypeScript scrolled the rest).
pub const OUTPUT_LINES: usize = 400;

/// Reads a notebook's text: the path in, the file's start out.
pub type NotebookSource =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<TextRead, String>> + Send>> + Send + Sync>;

/// glint-core's `read_text_preview` for [`MAX_BYTES`].
pub fn core_notebook_source() -> NotebookSource {
    Arc::new(|path| Box::pin(glint_core::fsx::read_text_preview(path, MAX_BYTES, None)))
}

// ---- The notebook, read ----------------------------------------------------------------------------------------

/// A picture an output holds: its bytes, and its size in pixels when its header says (to lay it out before it is
/// decoded).
#[derive(Clone)]
pub struct Picture {
    pub image: Arc<Image>,
    pub size: Option<(u32, u32)>,
}

/// A cell's output (`Output`).
#[derive(Clone)]
pub enum Output {
    /// `ename: evalue`.
    Error { name: String, value: String },
    /// A picture as a picture.
    Picture(Picture),
    /// A stream's text, or a result's `text/plain`.
    Text(String),
}

/// A cell (`Cell`).
#[derive(Clone)]
pub enum Cell {
    Markdown(String),
    Code {
        source: String,
        /// `execution_count`; none when it was never run.
        count: Option<i64>,
        outputs: Vec<Output>,
    },
    /// Anything else (a raw cell).
    Raw(String),
}

/// A notebook as it reads.
#[derive(Clone)]
pub struct Notebook {
    /// The kernel's language, for highlighting code.
    pub language: String,
    pub cells: Vec<Cell>,
}

/// A JSON string, or an array of strings joined with nothing between (`joined`): how nbformat keeps text.
pub fn joined(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Array(parts)) => parts.iter().filter_map(serde_json::Value::as_str).collect(),
        _ => String::new(),
    }
}

/// A code block's language as the notebook names it, by Prism's names (`languageNamed`): "python", "C++", "ts".
pub fn language_named(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if let Some(known) = LANGUAGE_NAMES.iter().find(|n| **n == lower) {
        return known;
    }
    match lower.as_str() {
        "c++" => "cpp",
        "c#" => "csharp",
        "shell" | "zsh" | "console" => "bash",
        "pwsh" | "ps" => "powershell",
        other => language_for(other),
    }
}

/// A fenced code block that holds `source` however many backticks it has itself.
pub fn fenced(source: &str, tag: &str) -> String {
    let (mut run, mut longest) = (0usize, 0usize);
    for c in source.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}{tag}\n{}\n{fence}", source.trim_end_matches('\n'))
}

/// The text cut to `max` lines, and how many lines were left out.
pub fn cut_lines(text: &str, max: usize) -> (String, usize) {
    let total = text.split('\n').count();
    if total <= max {
        return (text.to_string(), 0);
    }
    (text.split('\n').take(max).collect::<Vec<_>>().join("\n"), total - max)
}

/// A picture's size in pixels from its header: PNG, GIF, JPEG, and SVG's `viewBox` or `width` and `height`.
pub fn picture_size(format: ImageFormat, bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let le16 = |at: usize| {
        bytes
            .get(at..at + 2)
            .map(|b| u32::from(u16::from_le_bytes([b[0], b[1]])))
    };
    let (w, h) = match format {
        ImageFormat::Png => (be32(16)?, be32(20)?),
        ImageFormat::Gif => (le16(6)?, le16(8)?),
        ImageFormat::Jpeg => jpeg_size(bytes)?,
        ImageFormat::Svg => svg_size(std::str::from_utf8(bytes).ok()?)?,
        _ => return None,
    };
    (w > 0 && h > 0).then_some((w, h))
}

/// A JPEG's size from its first start-of-frame marker.
fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    while at + 3 < bytes.len() {
        if *bytes.get(at)? != 0xFF {
            at += 1;
            continue;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            0xFF => at += 1,
            // Markers without a length.
            0x01 | 0xD0..=0xD8 => at += 2,
            // Start of frame (not the tables 0xC4, 0xC8, 0xCC): height, then width.
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                let h = u16::from_be_bytes([*bytes.get(at + 5)?, *bytes.get(at + 6)?]);
                let w = u16::from_be_bytes([*bytes.get(at + 7)?, *bytes.get(at + 8)?]);
                return Some((u32::from(w), u32::from(h)));
            }
            _ => {
                let len = u16::from_be_bytes([*bytes.get(at + 2)?, *bytes.get(at + 3)?]) as usize;
                at += 2 + len;
            }
        }
    }
    None
}

/// An SVG's size: its `viewBox`, else its `width` and `height` (units ignored).
fn svg_size(text: &str) -> Option<(u32, u32)> {
    let head = text.get(..text.len().min(4096)).unwrap_or(text);
    let tag_start = head.find("<svg")?;
    let tag = &head[tag_start..head[tag_start..].find('>').map_or(head.len(), |i| tag_start + i)];
    let attribute = |name: &str| -> Option<&str> {
        let key = format!("{name}=");
        let at = tag.find(&key)? + key.len();
        let rest = tag.get(at..)?;
        let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let inner = rest.get(1..)?;
        inner.get(..inner.find(quote)?)
    };
    let number = |text: &str| -> Option<f64> {
        let end = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .unwrap_or(text.len());
        text.get(..end)?.parse().ok()
    };
    if let Some(view_box) = attribute("viewBox") {
        let parts: Vec<f64> = view_box
            .split([' ', ','])
            .filter(|p| !p.is_empty())
            .filter_map(number)
            .collect();
        if let [_, _, w, h] = parts[..] {
            return Some((w.round() as u32, h.round() as u32));
        }
    }
    let (w, h) = (number(attribute("width")?)?, number(attribute("height")?)?);
    Some((w.round() as u32, h.round() as u32))
}

/// A picture from an output's data: the first of PNG, JPEG, GIF, then SVG that is there and reads as base64 (or,
/// for SVG, as text). Whitespace in the base64 is skipped.
fn picture_of(data: &serde_json::Map<String, serde_json::Value>) -> Option<Picture> {
    let present = |mime: &str| {
        let text = joined(data.get(mime));
        (!text.is_empty()).then_some(text)
    };
    for (mime, format) in [
        ("image/png", ImageFormat::Png),
        ("image/jpeg", ImageFormat::Jpeg),
        ("image/gif", ImageFormat::Gif),
    ] {
        if let Some(text) = present(mime) {
            if let Some(bytes) = base64_decode(&text).filter(|b| !b.is_empty()) {
                let size = picture_size(format, &bytes);
                return Some(Picture {
                    image: Arc::new(Image::from_bytes(format, bytes)),
                    size,
                });
            }
        }
    }
    let svg = present("image/svg+xml")?;
    let bytes = svg.into_bytes();
    let size = picture_size(ImageFormat::Svg, &bytes);
    Some(Picture {
        image: Arc::new(Image::from_bytes(ImageFormat::Svg, bytes)),
        size,
    })
}

/// One output (`OutputView`): an error, a picture, or text; none when there is nothing to show.
fn output_of(value: &serde_json::Value) -> Option<Output> {
    let kind = value
        .get("output_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if kind == "error" {
        let text = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        return Some(Output::Error {
            name: text("ename"),
            value: text("evalue"),
        });
    }
    let empty = serde_json::Map::new();
    let data = value
        .get("data")
        .and_then(serde_json::Value::as_object)
        .unwrap_or(&empty);
    if let Some(picture) = picture_of(data) {
        return Some(Output::Picture(picture));
    }
    let text = if kind == "stream" {
        joined(value.get("text"))
    } else {
        joined(data.get("text/plain"))
    };
    (!text.is_empty()).then_some(Output::Text(text))
}

/// Reads a notebook's JSON. The same leniency as the TypeScript: any cell type that isn't Markdown or code is
/// raw, a missing `source` is empty, an output of a kind it doesn't know shows its text if it has any.
pub fn parse_notebook(text: &str) -> Result<Notebook, String> {
    let doc: serde_json::Value =
        serde_json::from_str(text).map_err(|_| "It isn't a notebook Glint can read".to_string())?;
    let metadata = doc.get("metadata");
    let language = metadata
        .and_then(|m| m.get("kernelspec"))
        .and_then(|k| k.get("language"))
        .or_else(|| {
            metadata
                .and_then(|m| m.get("language_info"))
                .and_then(|l| l.get("name"))
        })
        .and_then(serde_json::Value::as_str)
        .unwrap_or("python")
        .to_string();
    let cells = doc
        .get("cells")
        .and_then(serde_json::Value::as_array)
        .map(|cells| {
            cells
                .iter()
                .map(|cell| {
                    let source = joined(cell.get("source"));
                    match cell.get("cell_type").and_then(serde_json::Value::as_str) {
                        Some("markdown") => Cell::Markdown(source),
                        Some("code") => Cell::Code {
                            source,
                            count: cell.get("execution_count").and_then(serde_json::Value::as_i64),
                            outputs: cell
                                .get("outputs")
                                .and_then(serde_json::Value::as_array)
                                .map(|outs| outs.iter().filter_map(output_of).collect())
                                .unwrap_or_default(),
                        },
                        _ => Cell::Raw(source),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Notebook { language, cells })
}

/// What the file read gives: the notebook, or why not.
pub fn read_notebook(read: TextRead) -> Result<Notebook, String> {
    if read.truncated {
        return Err("This notebook is too big to preview".to_string());
    }
    parse_notebook(&read.text)
}

// ---- The previewer ---------------------------------------------------------------------------------------------

enum Load {
    Waiting,
    Failed(SharedString),
    Ready(Arc<Notebook>),
}

/// A Jupyter notebook as it reads.
pub struct NotebookView {
    path: String,
    source: NotebookSource,
    load: Load,
    started: Instant,
    generation: u64,
    cards: ListState,
    /// Each cell's rendered text, made the first time the cell is drawn.
    texts: Vec<Option<Entity<TextViewState>>>,
    /// Pictures written in Markdown cells (`data:` addresses), decoded once.
    written: Arc<Mutex<HashMap<String, Arc<Image>>>>,
    _read: Task<()>,
    _release: Subscription,
}

impl NotebookView {
    /// The view of the notebook at `path`. Reading and parsing happen in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_source(path, core_notebook_source(), window, cx)
    }

    /// [`NotebookView::new`] reading through `source` (tests pass fakes).
    pub fn with_source(path: String, source: NotebookSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let release = cx.on_release(|this, cx| this.forget_pictures(cx));
        let mut this = Self {
            path,
            source,
            load: Load::Waiting,
            started: Instant::now(),
            generation: 0,
            cards: ListState::new(0, ListAlignment::Top, px(600.)),
            texts: Vec::new(),
            written: Arc::new(Mutex::new(HashMap::new())),
            _read: Task::ready(()),
            _release: release,
        };
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another notebook.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.read(window, cx);
        }
    }

    /// Reads the notebook again (it changed on disk).
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read(window, cx);
    }

    /// The notebook, once read.
    pub fn notebook(&self) -> Option<&Notebook> {
        match &self.load {
            Load::Ready(notebook) => Some(notebook),
            _ => None,
        }
    }

    /// The Markdown a cell was drawn as (the cell's text, or the fenced code), once it has been drawn.
    #[cfg(test)]
    pub fn cell_text(&self, ix: usize, cx: &App) -> Option<String> {
        let state = self.texts.get(ix)?.as_ref()?;
        Some(state.read(cx).rendered_text().source().to_string())
    }

    /// Scrolls the cells a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let step = self.cards.viewport_bounds().size.height * 0.85;
        self.cards.scroll_by(if down { step } else { -step });
        cx.notify();
        true
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.forget_pictures(cx);
        self.load = Load::Waiting;
        self.texts.clear();
        self.cards.reset(0);
        self.started = Instant::now();
        let read = (self.source)(self.path.clone());
        self._read = cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            // The JSON is parsed, and the pictures decoded from base64, off the UI thread.
            let parsed = match result {
                Ok(read) => cx.background_spawn(async move { read_notebook(read) }).await,
                Err(reason) => Err(reason),
            };
            this.update_in(cx, |this, _, cx| {
                if this.generation != generation {
                    return;
                }
                this.load = match parsed {
                    Ok(notebook) => {
                        this.texts = vec![None; notebook.cells.len()];
                        this.cards.reset(notebook.cells.len());
                        Load::Ready(Arc::new(notebook))
                    }
                    Err(reason) => Load::Failed(reason.into()),
                };
                cx.notify();
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// Lets go of the decoded pictures of the notebook that was on screen.
    fn forget_pictures(&mut self, cx: &mut App) {
        if let Load::Ready(notebook) = &self.load {
            for cell in &notebook.cells {
                if let Cell::Code { outputs, .. } = cell {
                    for output in outputs {
                        if let Output::Picture(picture) = output {
                            picture.image.clone().remove_asset(cx);
                        }
                    }
                }
            }
        }
    }

    /// The text view of cell `ix`, made when it is first drawn: its Markdown, or its code as a fenced block.
    fn text_of(&mut self, ix: usize, notebook: &Notebook, cx: &mut Context<Self>) -> Option<Entity<TextViewState>> {
        if let Some(Some(state)) = self.texts.get(ix) {
            return Some(state.clone());
        }
        let markdown = match notebook.cells.get(ix)? {
            Cell::Markdown(source) => cut_lines(source, MAX_LINES).0,
            Cell::Code { source, .. } => {
                let tag = editor_language(language_named(&notebook.language)).unwrap_or("");
                fenced(&cut_lines(source, MAX_LINES).0, tag)
            }
            Cell::Raw(_) => return None,
        };
        let state = cx.new(|cx| TextViewState::markdown(&markdown, cx).selectable(true));
        *self.texts.get_mut(ix)? = Some(state.clone());
        Some(state)
    }

    /// The text view over a cell's state: pictures beside the notebook or written in the cell, links that open the
    /// web outside and files here.
    fn document(&self, state: &Entity<TextViewState>) -> TextView {
        let folder = parent_path(&self.path);
        let root = format!("{}\\", volume_of(&self.path));
        let written = self.written.clone();
        let (pic_folder, pic_root) = (folder.clone(), root.clone());
        TextView::new(state)
            .selectable(true)
            .scrollable(false)
            .image_source(move |uri| picture_source(uri.as_ref(), &pic_folder, &pic_root, &written))
            .on_link_click(
                move |url, _, _, cx| match link_action(url.as_ref(), &folder, &root, &[]) {
                    LinkAction::Web(address) => cx.open_url(&address),
                    LinkAction::Path(path) => {
                        if let Some(handle) = cx.try_global::<crate::explorer::ExplorerHandle>() {
                            handle
                                .0
                                .clone()
                                .update(cx, |explorer, cx| explorer.go_to(&path, cx).detach());
                        }
                    }
                    LinkAction::Heading(_) | LinkAction::None => {}
                },
            )
    }

    /// One cell and its outputs.
    fn cell(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let mono = theme::mono_font(cx);
        let Load::Ready(notebook) = &self.load else {
            return div().into_any_element();
        };
        let notebook = notebook.clone();
        let Some(cell) = notebook.cells.get(ix) else {
            return div().into_any_element();
        };
        let n = ix + 1;
        let body = match cell {
            Cell::Markdown(_) => {
                let view = self
                    .text_of(ix, &notebook, cx)
                    .map(|state| self.document(&state).w_full());
                div()
                    .id(("notebook-markdown", ix))
                    .role(Role::Group)
                    .aria_label(t!("Markdown cell {n}", n = n))
                    .test_support()
                    .px(space(1.))
                    .children(view)
                    .into_any_element()
            }
            Cell::Code { count, outputs, .. } => {
                let gutter = match count {
                    Some(count) => format!("[{count}]"),
                    None => "[ ]".to_string(),
                };
                let view = self
                    .text_of(ix, &notebook, cx)
                    .map(|state| self.document(&state).w_full());
                col()
                    .id(("notebook-code", ix))
                    .role(Role::Group)
                    .aria_label(t!("Code cell {n}", n = n))
                    .test_support()
                    .gap(space(1.))
                    .child(
                        row_start()
                            .items_start()
                            .gap(space(2.))
                            .child(
                                div()
                                    .flex_none()
                                    .w(space(10.))
                                    .pt(space(1.5))
                                    .text_end()
                                    .font_family(mono.clone())
                                    .text_size(theme::size::XXS)
                                    .text_color(t.muted_foreground)
                                    .child(gutter),
                            )
                            .child(div().flex_1().min_w_0().children(view)),
                    )
                    .children(
                        outputs
                            .iter()
                            .enumerate()
                            .map(|(k, output)| div().ms(12.).min_w_0().child(output_view(ix, k, output, &t, &mono))),
                    )
                    .into_any_element()
            }
            Cell::Raw(source) => {
                let (shown, hidden) = cut_lines(source, MAX_LINES);
                div()
                    .id(("notebook-raw", ix))
                    .role(Role::Group)
                    .aria_label(t!("Raw cell {n}", n = n))
                    .test_support()
                    .px(space(3.))
                    .py(space(2.))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.border)
                    .font_family(mono.clone())
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(shown)
                    .children((hidden > 0).then(|| more_lines(hidden)))
                    .into_any_element()
            }
        };
        div().w_full().pb(space(3.)).child(body).into_any_element()
    }
}

/// "… 12 more lines": what was left out of a cut text.
fn more_lines(hidden: usize) -> impl IntoElement {
    div()
        .pt(space(1.))
        .italic()
        .child(t!("… {n} more lines", n = glint_model::format::format_count(hidden)))
}

/// A cell's output (`OutputView`): red text for an error, a picture as a picture, else its text.
fn output_view(cell: usize, k: usize, output: &Output, t: &theme::Tokens, mono: &SharedString) -> AnyElement {
    let label = t!("Output {k} of cell {n}", k = k + 1, n = cell + 1);
    match output {
        Output::Error { name, value } => div()
            .id(("notebook-error", cell * 10_000 + k))
            .role(Role::Alert)
            .aria_label(format!("{name}: {value}"))
            .test_support()
            .px(space(3.))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .bg(t.destructive.opacity(0.1))
            .font_family(mono.clone())
            .text_size(theme::size::XS)
            .text_color(t.destructive)
            .child(format!("{name}: {value}"))
            .into_any_element(),
        Output::Picture(picture) => {
            let frame = div()
                .id(("notebook-picture", cell * 10_000 + k))
                .role(Role::Image)
                .aria_label(label)
                .test_support()
                .rounded(theme::size::RADIUS_SM)
                .bg(gpui_kit::rgb(0xffffff))
                .overflow_hidden()
                .max_w_full();
            let image = img(ImageSource::Image(picture.image.clone())).object_fit(ObjectFit::Contain);
            match picture.size {
                // Laid out at its own size, or smaller to fit, before it is decoded.
                Some((w, h)) => frame
                    .w(px(w as f32))
                    .aspect_ratio(w as f32 / h as f32)
                    .child(image.size_full())
                    .into_any_element(),
                None => frame.child(image.max_w_full()).into_any_element(),
            }
        }
        Output::Text(text) => {
            let (shown, hidden) = cut_lines(text, OUTPUT_LINES);
            div()
                .id(("notebook-output", cell * 10_000 + k))
                .role(Role::Group)
                .aria_label(label)
                .test_support()
                .px(space(1.))
                .font_family(mono.clone())
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(shown)
                .children((hidden > 0).then(|| more_lines(hidden)))
                .into_any_element()
        }
    }
}

impl Render for NotebookView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = col()
            .id("notebook-view")
            .size_full()
            .min_h_0()
            .text_start()
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let notebook = match &self.load {
            Load::Waiting => return root.child(waiting(self.started, None, cx)),
            Load::Failed(reason) => return root.child(big_icon(&self.path, Some(reason.clone()), cx)),
            Load::Ready(notebook) => notebook.clone(),
        };
        if notebook.cells.is_empty() {
            return root.child(note_centered("notebook-empty", t!("This file is empty."), cx));
        }
        let cells = list(self.cards.clone(), cx.processor(Self::cell)).size_full();
        root.child(
            div()
                .id("notebook-cells")
                .role(Role::List)
                .aria_label(t!("Cells of {name}", name = name_of(&self.path)))
                .test_support()
                .relative()
                .flex_1()
                .min_h_0()
                .px(space(3.))
                .pt(space(3.))
                .text_size(theme::size::SM)
                .child(cells)
                .vertical_scrollbar(&self.cards),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::time::Duration;

    /// A 1 x 1 PNG.
    const PNG: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

    fn read_of(text: &str) -> TextRead {
        TextRead {
            text: text.to_string(),
            truncated: false,
            binary: false,
            encoding: None,
            guessed: false,
        }
    }

    /// A notebook with an Arabic Markdown cell, a code cell with an Arabic string and every kind of output, and a
    /// raw cell.
    fn sample() -> String {
        serde_json::json!({
            "metadata": { "kernelspec": { "language": "Python", "name": "python3" } },
            "cells": [
                { "cell_type": "markdown", "source": ["# عنوان\n", "نص **عريض**"] },
                { "cell_type": "code", "execution_count": 3, "source": ["print('مرحبا')\n", "x = 1"],
                  "outputs": [
                    { "output_type": "stream", "name": "stdout", "text": ["مرحبا\n", "done\n"] },
                    { "output_type": "execute_result", "data": { "text/plain": "42" } },
                    { "output_type": "display_data", "data": { "image/png": PNG, "text/plain": "<Figure>" } },
                    { "output_type": "error", "ename": "ValueError", "evalue": "bad value", "traceback": [] },
                    { "output_type": "stream", "text": "" }
                  ] },
                { "cell_type": "code", "execution_count": null, "source": "pass", "outputs": [] },
                { "cell_type": "raw", "source": "raw text" }
            ]
        })
        .to_string()
    }

    fn source_of(text: String) -> NotebookSource {
        Arc::new(move |_| {
            let read = read_of(&text);
            Box::pin(async move { Ok(read) })
        })
    }

    fn open(cx: &mut TestAppContext, source: NotebookSource) -> (Entity<NotebookView>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| NotebookView::with_source("C:\\work\\lab.ipynb".into(), source, window, cx))
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
        panic!("the notebook never arrived");
    }

    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..4 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    fn ready(view: &Entity<NotebookView>, visual: &mut VisualTestContext) {
        settle(visual, |cx| view.read_with(cx, |v, _| v.notebook().is_some()));
        frames(visual);
    }

    #[test]
    fn text_is_a_string_or_an_array_of_strings_joined_with_nothing_between() {
        use serde_json::json;
        assert_eq!(joined(Some(&json!("a\nb"))), "a\nb");
        assert_eq!(joined(Some(&json!(["a\n", "b", "ج"]))), "a\nbج");
        assert_eq!(joined(Some(&json!(null))), "");
        assert_eq!(joined(Some(&json!(5))), "");
        assert_eq!(joined(None), "");
    }

    #[test]
    fn the_notebooks_language_is_read_the_way_the_typescript_read_it() {
        let language = |json: &str| parse_notebook(json).map(|n| n.language).unwrap_or_default();
        assert_eq!(language(r#"{"cells":[]}"#), "python");
        assert_eq!(language(r#"{"metadata":{"kernelspec":{"language":"julia"}}}"#), "julia");
        assert_eq!(language(r#"{"metadata":{"language_info":{"name":"R"}}}"#), "R");
        // The kernelspec wins over language_info.
        assert_eq!(
            language(r#"{"metadata":{"kernelspec":{"language":"python"},"language_info":{"name":"r"}}}"#),
            "python"
        );
        // Names are Prism's, through the aliases and the extensions.
        assert_eq!(language_named("Python"), "python");
        assert_eq!(language_named("C++"), "cpp");
        assert_eq!(language_named("c#"), "csharp");
        assert_eq!(language_named("zsh"), "bash");
        assert_eq!(language_named("pwsh"), "powershell");
        assert_eq!(language_named("ts"), "typescript");
        assert_eq!(language_named("julia"), "text");
    }

    #[test]
    fn cells_and_outputs_are_read_leniently() {
        let notebook = parse_notebook(&sample()).expect("a notebook");
        assert_eq!(notebook.cells.len(), 4);
        assert!(matches!(&notebook.cells[0], Cell::Markdown(s) if s == "# عنوان\nنص **عريض**"));
        let Cell::Code { source, count, outputs } = &notebook.cells[1] else {
            panic!("a code cell")
        };
        assert_eq!(source, "print('مرحبا')\nx = 1");
        assert_eq!(*count, Some(3));
        // The empty stream is dropped; the others are in order.
        assert_eq!(outputs.len(), 4);
        assert!(matches!(&outputs[0], Output::Text(t) if t == "مرحبا\ndone\n"));
        assert!(matches!(&outputs[1], Output::Text(t) if t == "42"));
        // A picture wins over its text/plain.
        assert!(matches!(&outputs[2], Output::Picture(p) if p.size == Some((1, 1))));
        assert!(matches!(&outputs[3], Output::Error { name, value } if name == "ValueError" && value == "bad value"));
        assert!(matches!(&notebook.cells[2], Cell::Code { count: None, .. }));
        assert!(matches!(&notebook.cells[3], Cell::Raw(s) if s == "raw text"));
        // Anything else is raw, and a missing source is empty.
        let odd = parse_notebook(r#"{"cells":[{"cell_type":"heading"},{"cell_type":"code"}]}"#).expect("a notebook");
        assert!(matches!(&odd.cells[0], Cell::Raw(s) if s.is_empty()));
        assert!(matches!(&odd.cells[1], Cell::Code { outputs, .. } if outputs.is_empty()));
        // No cells is no cells.
        assert!(parse_notebook(r#"{"cells":"x"}"#).expect("a notebook").cells.is_empty());
    }

    #[test]
    fn what_is_not_a_notebook_or_is_too_big_says_so() {
        assert_eq!(
            parse_notebook("not json").err().as_deref(),
            Some("It isn't a notebook Glint can read")
        );
        let mut cut = read_of("{}");
        cut.truncated = true;
        assert_eq!(
            read_notebook(cut).err().as_deref(),
            Some("This notebook is too big to preview")
        );
        assert!(read_notebook(read_of("{}")).is_ok());
    }

    #[test]
    fn pictures_are_the_first_of_png_jpeg_gif_then_svg_and_have_their_size_from_the_header() {
        use serde_json::json;
        let data = |value: serde_json::Value| value.as_object().cloned().unwrap_or_default();
        // Whitespace in the base64 is skipped, as the TypeScript's replace did.
        let spaced = format!("{}\n{}", &PNG[..20], &PNG[20..]);
        let p = picture_of(&data(json!({ "image/png": spaced }))).expect("a picture");
        assert_eq!(p.size, Some((1, 1)));
        let svg = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 200 100'></svg>";
        let p = picture_of(&data(json!({ "image/svg+xml": [svg] }))).expect("an svg");
        assert_eq!(p.size, Some((200, 100)));
        assert!(picture_of(&data(json!({ "image/png": "", "text/plain": "x" }))).is_none());
        assert!(picture_of(&data(json!({ "image/png": "%%%" }))).is_none());
        assert_eq!(svg_size("<svg width=\"640px\" height=\"480\"></svg>"), Some((640, 480)));
        assert_eq!(svg_size("<svg></svg>"), None);
        // A JPEG's size is in its start-of-frame, past the other segments.
        let jpeg = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x20, 0x00, 0x40, 0x01,
            0x01, 0x11, 0x00,
        ];
        assert_eq!(picture_size(ImageFormat::Jpeg, &jpeg), Some((64, 32)));
        assert_eq!(picture_size(ImageFormat::Jpeg, &jpeg[..6]), None);
        assert_eq!(picture_size(ImageFormat::Png, b"short"), None);
        assert_eq!(picture_size(ImageFormat::Gif, b"GIF89a\x10\x00\x08\x00"), Some((16, 8)));
    }

    #[test]
    fn code_is_fenced_with_more_backticks_than_it_has() {
        assert_eq!(fenced("x = 1\n", "py"), "```py\nx = 1\n```");
        assert_eq!(fenced("a ``` b", ""), "````\na ``` b\n````");
        assert_eq!(fenced("``````", "").lines().next(), Some("```````"));
        assert_eq!(cut_lines("a\nb\nc", 5), ("a\nb\nc".to_string(), 0));
        assert_eq!(cut_lines("a\nb\nc", 2), ("a\nb".to_string(), 1));
    }

    #[gpui_kit::test]
    fn the_cells_show_in_order_with_their_outputs_and_arabic_text(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (view, mut visual) = open(cx, source_of(sample()));
        // The read is under way: the UI thread didn't wait for it.
        assert!(view.read_with(&visual, |v, _| v.notebook().is_none()));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "Markdown cell 1"));
        assert!(has(&mut visual, "Code cell 2"));
        assert!(has(&mut visual, "Code cell 3"));
        assert!(has(&mut visual, "Raw cell 4"));
        assert!(has(&mut visual, "ValueError: bad value"));
        assert!(has(&mut visual, "Output 1 of cell 2"));
        assert!(has(&mut visual, "Output 3 of cell 2"), "the picture is named too");
        assert_eq!(
            view.read_with(&visual, |v, cx| v.cell_text(0, cx)).as_deref(),
            Some("# عنوان\nنص **عريض**")
        );
        // The code is highlighted as Python, through a fenced block.
        assert_eq!(
            view.read_with(&visual, |v, cx| v.cell_text(1, cx)).as_deref(),
            Some("```py\nprint('مرحبا')\nx = 1\n```")
        );
        // A cell that was never run has an empty gutter.
        assert!(view.read_with(&visual, |v, cx| v.cell_text(2, cx)).is_some());
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_long_notebook_only_draws_the_cells_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let cells: Vec<_> = (0..300)
            .map(|i| serde_json::json!({ "cell_type": "markdown", "source": format!("cell number {i}") }))
            .collect();
        let text = serde_json::json!({ "cells": cells }).to_string();
        let (view, mut visual) = open(cx, source_of(text));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "Markdown cell 1"));
        let drawn = visual.update(|window, _| window.find_all_by_role(Role::Group).len());
        assert!(drawn < 60, "{drawn} cells drawn");
        // Only the cells that were drawn made a text view.
        let made = (0..300)
            .filter(|&i| view.read_with(&visual, |v, cx| v.cell_text(i, cx)).is_some())
            .count();
        assert!(made > 0 && made < 100, "{made} text views made");
    }

    #[gpui_kit::test]
    fn what_cannot_be_read_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let (view, mut visual) = open(cx, source_of("[1, 2".into()));
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Failed(_)))
        });
        assert!(has(&mut visual, "It isn't a notebook Glint can read"));
        let mut big = read_of("{}");
        big.truncated = true;
        let source: NotebookSource = Arc::new(move |_| {
            let read = TextRead {
                text: big.text.clone(),
                truncated: true,
                binary: false,
                encoding: None,
                guessed: false,
            };
            Box::pin(async move { Ok(read) })
        });
        let (view, mut visual) = open(cx, source);
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, _| matches!(v.load, Load::Failed(_)))
        });
        assert!(has(&mut visual, "This notebook is too big to preview"));
        let (view, mut visual) = open(cx, source_of(r#"{"cells":[]}"#.into()));
        ready(&view, &mut visual);
        assert!(has(&mut visual, "This file is empty."));
    }
}
