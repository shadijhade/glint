//! The rich text the book and Word previews share: blocks and runs, right to left (previews-native.md sections 2-3).
//!
//! An EPUB chapter, an FB2 section and a Word document are all read into the same small model ([`Document`]: a
//! list of [`Block`]s, each holding [`Text`] made of styled [`Span`]s), and one view ([`RichView`]) draws it.
//! The readers (`book.rs`, `docx.rs`) know their file formats; this file knows nothing about them.
//!
//! | What a reader makes                             | Here                                                        |
//! |-------------------------------------------------|-------------------------------------------------------------|
//! | headings, paragraphs, quotes, code, rules       | [`Block::Heading`], [`Block::Paragraph`], [`Block::Quote`], [`Block::Code`], [`Block::Rule`] |
//! | lists (numbered, bulleted, nested)              | [`Block::Item`]: the reader works out the marker            |
//! | bold, italic, underline, strike, code, links    | [`Style`] on each [`Span`]                                  |
//! | `dir="rtl"`, `xml:lang`, `w:bidi`, `w:rtl`      | [`Dir`] on a [`Text`] (the paragraph) and on a [`Style`] (a run) |
//! | tables                                          | [`Block::Table`]: rows of [`Cell`]s holding blocks          |
//! | pictures                                        | [`Block::Picture`], made by [`picture_of`]                  |
//! | page breaks                                     | [`Block::PageBreak`]                                        |
//! | ids to jump to (`#chapter-2`, bookmarks)        | [`Document::anchors`], filled by [`Builder::anchor`]        |
//!
//! # Drawing
//!
//! The view does not use the kit's `TextView` (its HTML subset has no paragraph direction). Each block is its own
//! GPUI element in a `list`, so only the blocks on screen are laid out, and a text block is a `StyledText` with one
//! highlight per span. **Right to left** is decided per paragraph: a [`Dir::Auto`] paragraph takes the direction of
//! its first strong letter, and an Arabic paragraph is wrapped in an RLI .. PDI isolate (see `kit::isolate`) so
//! DirectWrite orders it as its own right-to-left paragraph, right-aligned, with a list's marker and a quote's bar
//! on its right. A run with its own direction (Word's `w:rtl`) is isolated the same way inside the paragraph.
//!
//! # Not ported
//!
//! - Text is not selectable and cannot be copied from the view (the TypeScript's pages were `select-text`).
//!   [`Document::plain_text`] is there for the owner to offer Copy all.
//! - A link is clicked, not tabbed to: `StyledText` has no focusable pieces.
//! - Superscript and subscript are drawn with Unicode's superscript and subscript digits and signs when every
//!   character has one, else as plain text (GPUI has no baseline shift).
//! - Justified text is aligned at the start edge (GPUI has no justification).

use super::notebook::picture_size;
use crate::kit::prelude::*;
use crate::kit::ScrollableElement as _;
use crate::{t, theme};
use gpui_kit::{
    img, list, FontStyle, FontWeight, HighlightStyle, Hsla, Image, ImageFormat, ImageSource, InteractiveText,
    ListAlignment, ListOffset, ListState, ObjectFit, StrikethroughStyle, StyledImage as _, StyledText, UnderlineStyle,
    WeakEntity,
};
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// The biggest picture kept (compressed bytes); a bigger one is shown as its alt text.
pub const MAX_PICTURE_BYTES: usize = 24 * 1024 * 1024;
/// The most pixels a picture may have (the decoded copy is four bytes each); a bigger one is shown as its alt text.
pub const MAX_PICTURE_PIXELS: u64 = 48_000_000;
/// A table is cut into pieces of this many rows, so a long one is virtualized like everything else.
pub const TABLE_CHUNK: usize = 40;
/// The size of reading text, in rems (15 px at the usual size), before the reader's own scale.
const READING: f32 = 0.9375;

const RLI: char = '\u{2067}';
const LRI: char = '\u{2066}';
const PDI: char = '\u{2069}';

// ---- The model -------------------------------------------------------------------------------------------------

/// Which way text runs: set by the file (`dir`, `w:bidi`), else read from the text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dir {
    /// Not said: the first strong letter decides.
    #[default]
    Auto,
    Ltr,
    Rtl,
}

/// Where a paragraph's lines sit, in the paragraph's own direction (Word's `start` and `end`, and `left` and
/// `right` in a right-to-left paragraph, are the same thing).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    End,
    Center,
}

/// Raised or lowered text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Script {
    #[default]
    Normal,
    Super,
    Sub,
}

/// How a run looks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Monospace, on a tinted ground.
    pub code: bool,
    pub script: Script,
    /// `0xrrggbb`, when the file sets one (a color too close to the background is ignored when drawn).
    pub color: Option<u32>,
    /// Where a click goes: a web address, `mailto:`, or `#id` / `file#id` inside the book.
    pub link: Option<Arc<str>>,
    /// The run's own direction when it differs from its paragraph's (Word's `w:rtl`).
    pub dir: Dir,
}

/// A run of text in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

/// The text of one paragraph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Text {
    pub spans: Vec<Span>,
    pub dir: Dir,
    pub align: Align,
}

impl Text {
    /// An empty paragraph text with a direction.
    pub fn new(dir: Dir) -> Self {
        Self {
            spans: Vec::new(),
            dir,
            align: Align::Start,
        }
    }

    /// Adds `text` in `style`, joining the previous span when it has the same style.
    pub fn push(&mut self, text: &str, style: &Style) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if &last.style == style {
                last.text.push_str(text);
                return;
            }
        }
        self.spans.push(Span {
            text: text.to_string(),
            style: style.clone(),
        });
    }

    /// The characters, styles left out.
    pub fn plain(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// Whether there is nothing to see: no text, or only white space.
    pub fn is_blank(&self) -> bool {
        self.spans.iter().all(|span| span.text.trim().is_empty())
    }

    /// Whether the text ends in a space (an HTML run of white space collapses to one).
    pub fn ends_with_space(&self) -> bool {
        self.spans.last().is_some_and(|span| span.text.ends_with(' '))
    }

    /// Takes the white space off both ends.
    pub fn trim(&mut self) {
        while let Some(first) = self.spans.first_mut() {
            let trimmed = first.text.trim_start();
            if trimmed.is_empty() {
                self.spans.remove(0);
            } else {
                first.text = trimmed.to_string();
                break;
            }
        }
        while let Some(last) = self.spans.last_mut() {
            let trimmed = last.text.trim_end();
            if trimmed.is_empty() {
                self.spans.pop();
            } else {
                last.text = trimmed.to_string();
                break;
            }
        }
    }

    /// Whether the paragraph runs right to left: as said, else by its first strong letter, else `default`.
    pub fn is_rtl(&self, default: bool) -> bool {
        match self.dir {
            Dir::Rtl => true,
            Dir::Ltr => false,
            Dir::Auto => first_strong_rtl(&self.plain()).unwrap_or(default),
        }
    }
}

/// A picture in the flow of the text.
#[derive(Clone)]
pub struct Picture {
    /// The picture's bytes for GPUI to decode when it is drawn; none when it could not be read or is too big.
    pub image: Option<Arc<Image>>,
    /// Its size in pixels when its header says (to lay it out before it is decoded), or what the file asks for.
    pub size: Option<(u32, u32)>,
    pub alt: String,
}

/// A table's cell: blocks, how many columns wide, its ground.
#[derive(Clone)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub span: u32,
    /// `0xrrggbb` fill (Word's `w:shd`).
    pub shade: Option<u32>,
}

/// A table's row.
#[derive(Clone)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// A heading row: drawn bold on a tint.
    pub header: bool,
}

/// A table.
#[derive(Clone)]
pub struct Table {
    pub rows: Vec<Row>,
    /// The first column is at the right (`w:bidiVisual`, `dir="rtl"`).
    pub rtl: bool,
}

/// One block of a document.
#[derive(Clone)]
pub enum Block {
    /// `level` 1 to 6.
    Heading {
        level: u8,
        text: Text,
    },
    Paragraph(Text),
    /// A list item at `depth` (0 is the outermost), with the marker the reader worked out ("•", "3.", "b)").
    Item {
        depth: u8,
        marker: String,
        text: Text,
    },
    /// A quotation, `depth` deep (1 is one bar).
    Quote {
        depth: u8,
        text: Text,
    },
    /// Preformatted text.
    Code(String),
    Rule,
    PageBreak,
    Picture(Picture),
    Table(Table),
}

impl Block {
    /// What a screen reader says for the block (before it is made unique): its text, or what it is.
    pub fn name(&self) -> String {
        match self {
            Block::Heading { text, .. }
            | Block::Paragraph(text)
            | Block::Item { text, .. }
            | Block::Quote { text, .. } => text.plain(),
            Block::Code(code) => code.lines().next().unwrap_or_default().to_string(),
            Block::Picture(picture) => picture.alt.clone(),
            Block::Table(_) => t!("Table").to_string(),
            Block::Rule | Block::PageBreak => String::new(),
        }
    }

    /// The block's text, one line per paragraph (tables: a cell per tab).
    fn push_plain(&self, out: &mut String) {
        match self {
            Block::Heading { text, .. } | Block::Paragraph(text) | Block::Quote { text, .. } => {
                out.push_str(&text.plain());
                out.push('\n');
            }
            Block::Item { marker, text, .. } => {
                out.push_str(marker);
                out.push(' ');
                out.push_str(&text.plain());
                out.push('\n');
            }
            Block::Code(code) => {
                out.push_str(code);
                out.push('\n');
            }
            Block::Table(table) => {
                for row in &table.rows {
                    for (i, cell) in row.cells.iter().enumerate() {
                        if i > 0 {
                            out.push('\t');
                        }
                        let mut inner = String::new();
                        for block in &cell.blocks {
                            block.push_plain(&mut inner);
                        }
                        out.push_str(inner.trim_end());
                    }
                    out.push('\n');
                }
            }
            Block::Picture(picture) if !picture.alt.is_empty() => {
                out.push_str(&picture.alt);
                out.push('\n');
            }
            Block::Picture(_) | Block::Rule | Block::PageBreak => {}
        }
    }
}

/// A chapter or a document: its blocks, and where each id is.
#[derive(Clone, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Id (`#id`, a bookmark's name) to the index of the block it is on.
    pub anchors: HashMap<String, usize>,
}

impl Document {
    /// The document's text, a line for each paragraph.
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            block.push_plain(&mut out);
        }
        out
    }

    /// Whether more paragraphs run right to left than left to right (what a blank paragraph follows).
    pub fn mostly_rtl(&self) -> bool {
        let (mut rtl, mut ltr) = (0usize, 0usize);
        for block in &self.blocks {
            if let Block::Heading { text, .. }
            | Block::Paragraph(text)
            | Block::Item { text, .. }
            | Block::Quote { text, .. } = block
            {
                match text.is_rtl(false) {
                    true => rtl += 1,
                    false => ltr += 1,
                }
            }
        }
        rtl > ltr
    }

    /// Every picture's image, tables included.
    fn images(&self) -> Vec<Arc<Image>> {
        fn walk(blocks: &[Block], out: &mut Vec<Arc<Image>>) {
            for block in blocks {
                match block {
                    Block::Picture(Picture { image: Some(image), .. }) => out.push(image.clone()),
                    Block::Table(table) => {
                        for cell in table.rows.iter().flat_map(|row| &row.cells) {
                            walk(&cell.blocks, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.blocks, &mut out);
        out
    }
}

/// Makes a [`Document`] a reader at a time: push blocks, mark ids, finish.
#[derive(Default)]
pub struct Builder {
    blocks: Vec<Block>,
    anchors: HashMap<String, usize>,
    pending: Vec<String>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a block. A table longer than [`TABLE_CHUNK`] rows goes in as several. Ids marked since the last block
    /// are on this one.
    pub fn push(&mut self, block: Block) {
        let at = self.blocks.len();
        for id in self.pending.drain(..) {
            self.anchors.entry(id).or_insert(at);
        }
        match block {
            Block::Table(table) if table.rows.len() > TABLE_CHUNK => {
                let rtl = table.rtl;
                let mut rows = table.rows.into_iter().peekable();
                while rows.peek().is_some() {
                    let chunk: Vec<Row> = rows.by_ref().take(TABLE_CHUNK).collect();
                    self.blocks.push(Block::Table(Table { rows: chunk, rtl }));
                }
            }
            other => self.blocks.push(other),
        }
    }

    /// Marks an id that links can go to: it is on the next block pushed.
    pub fn anchor(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !id.is_empty() {
            self.pending.push(id);
        }
    }

    /// How many blocks there are.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Takes the blocks made so far (a table cell's content), leaving the builder empty.
    pub fn take(&mut self) -> Vec<Block> {
        self.pending.clear();
        std::mem::take(&mut self.blocks)
    }

    /// The document. Ids with no block after them are on the last one.
    pub fn finish(mut self) -> Document {
        let last = self.blocks.len().saturating_sub(1);
        for id in self.pending.drain(..) {
            self.anchors.entry(id).or_insert(last);
        }
        Document {
            blocks: self.blocks,
            anchors: self.anchors,
        }
    }
}

// ---- Reading the text ------------------------------------------------------------------------------------------

/// Whether `ch` is a letter of a right-to-left script (Hebrew, Arabic, Syriac, Thaana, NKo and their forms).
fn is_rtl_letter(ch: char) -> bool {
    matches!(ch as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
        && ch.is_alphabetic()
}

/// The direction of the first strong letter of `text`: `Some(true)` for a right-to-left script, `None` for text
/// with no letters (numbers, symbols).
pub fn first_strong_rtl(text: &str) -> Option<bool> {
    text.chars().find(|ch| ch.is_alphabetic()).map(is_rtl_letter)
}

/// Whether a language code is written right to left (`ar`, `he`, `fa`, `ur`…): a book in one turns its pages the
/// other way when it doesn't say.
pub fn rtl_language(code: &str) -> bool {
    let primary = code.split(['-', '_']).next().unwrap_or("").to_ascii_lowercase();
    matches!(
        primary.as_str(),
        "ar" | "he" | "iw" | "fa" | "ur" | "ps" | "sd" | "ug" | "yi" | "dv" | "ku" | "ckb" | "syr" | "arc"
    )
}

/// `text` as superscript or subscript characters, when every one has one.
fn scripted(text: &str, script: Script) -> Option<String> {
    let map = |c: char| -> Option<char> {
        let (up, down) = match c {
            '0' => ('⁰', '₀'),
            '1' => ('¹', '₁'),
            '2' => ('²', '₂'),
            '3' => ('³', '₃'),
            '4' => ('⁴', '₄'),
            '5' => ('⁵', '₅'),
            '6' => ('⁶', '₆'),
            '7' => ('⁷', '₇'),
            '8' => ('⁸', '₈'),
            '9' => ('⁹', '₉'),
            '+' => ('⁺', '₊'),
            '-' => ('⁻', '₋'),
            '=' => ('⁼', '₌'),
            '(' => ('⁽', '₍'),
            ')' => ('⁾', '₎'),
            _ => return None,
        };
        Some(if script == Script::Super { up } else { down })
    };
    if script == Script::Normal {
        return None;
    }
    text.chars().map(map).collect()
}

/// What a text lays out as: the string GPUI draws, and the byte ranges in it. Ranges are UTF-8 byte offsets on
/// character boundaries in the *drawn* string (which has direction marks in it).
pub struct Laid {
    pub text: String,
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    pub monos: Vec<(Range<usize>, SharedString)>,
    pub links: Vec<(Range<usize>, Arc<str>)>,
}

/// Whether a file's color can be seen on the ground it will be drawn on.
fn visible_color(rgb: u32, dark: bool) -> bool {
    let (r, g, b) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
    let light = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.;
    if dark {
        light > 0.3
    } else {
        light < 0.8
    }
}

/// Lays `text` out for drawing as a paragraph running `rtl`.
pub fn lay_out(text: &Text, rtl: bool, bold: bool, t: &theme::Tokens, mono: &SharedString) -> Laid {
    let mut out = String::new();
    let wrapped = rtl || first_strong_rtl(&text.plain()) == Some(true);
    if wrapped {
        out.push(if rtl { RLI } else { LRI });
    }
    let mut laid = Laid {
        text: String::new(),
        highlights: Vec::new(),
        monos: Vec::new(),
        links: Vec::new(),
    };
    for span in &text.spans {
        if span.text.is_empty() {
            continue;
        }
        let shown = scripted(&span.text, span.style.script);
        let shown = shown.as_deref().unwrap_or(&span.text);
        let own = match span.style.dir {
            Dir::Auto => None,
            Dir::Rtl => Some(true),
            Dir::Ltr => Some(false),
        }
        .filter(|d| *d != rtl);
        if let Some(rtl_run) = own {
            out.push(if rtl_run { RLI } else { LRI });
        }
        let start = out.len();
        out.push_str(shown);
        let range = start..out.len();
        if own.is_some() {
            out.push(PDI);
        }
        let style = &span.style;
        let linked = style.link.is_some();
        let mut highlight = HighlightStyle::default();
        if style.bold || bold {
            highlight.font_weight = Some(FontWeight::BOLD);
        }
        if style.italic {
            highlight.font_style = Some(FontStyle::Italic);
        }
        if style.underline || linked {
            highlight.underline = Some(UnderlineStyle {
                thickness: gpui_kit::px(1.),
                color: None,
                wavy: false,
            });
        }
        if style.strike {
            highlight.strikethrough = Some(StrikethroughStyle {
                thickness: gpui_kit::px(1.),
                color: None,
            });
        }
        if style.code {
            highlight.background_color = Some(t.muted);
            laid.monos.push((range.clone(), mono.clone()));
        }
        if let Some(rgb) = style.color.filter(|rgb| visible_color(*rgb, t.dark)) {
            highlight.color = Some(gpui_kit::rgb(rgb).into());
        }
        if linked {
            highlight.color = Some(t.primary);
        }
        if let Some(href) = &style.link {
            laid.links.push((range.clone(), href.clone()));
        }
        laid.highlights.push((range, highlight));
    }
    if wrapped {
        out.push(PDI);
    }
    laid.text = out;
    laid
}

// ---- Pictures --------------------------------------------------------------------------------------------------

/// The format of a picture's bytes, from their first bytes. None for what GPUI cannot draw (Windows metafiles…).
pub fn sniff_image(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") && bytes.len() > 26 {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Some(ImageFormat::Tiff)
    } else {
        let head = std::str::from_utf8(bytes.get(..bytes.len().min(2048)).unwrap_or(bytes))
            .or_else(|e| std::str::from_utf8(&bytes[..e.valid_up_to()]))
            .unwrap_or("");
        head.contains("<svg").then_some(ImageFormat::Svg)
    }
}

/// A picture from its bytes. `alt` is what is said (and drawn) where the picture cannot be: a format GPUI doesn't
/// draw, or one past [`MAX_PICTURE_BYTES`] or [`MAX_PICTURE_PIXELS`]. `asked` is the size the file asks for.
pub fn picture_of(bytes: Vec<u8>, alt: &str, asked: Option<(u32, u32)>) -> Picture {
    let format = sniff_image(&bytes);
    let known = format.and_then(|f| picture_size(f, &bytes));
    let too_big = bytes.len() > MAX_PICTURE_BYTES
        || known.is_some_and(|(w, h)| u64::from(w) * u64::from(h) > MAX_PICTURE_PIXELS)
        || asked.is_some_and(|(w, h)| u64::from(w) * u64::from(h) > MAX_PICTURE_PIXELS * 4);
    let image = match format {
        Some(format) if !too_big => Some(Arc::new(Image::from_bytes(format, bytes))),
        _ => None,
    };
    Picture {
        image,
        size: asked.or(known),
        alt: alt.to_string(),
    }
}

// ---- The view --------------------------------------------------------------------------------------------------

/// What a click in the text asks of the owner.
pub enum RichEvent {
    /// A link was clicked: the address as the reader wrote it.
    Link(SharedString),
    /// The reader scrolled with the wheel or the scroll bar (the owner may show where they are).
    Scrolled,
}

/// Per-draw facts the block builders share.
struct Ctx<'a> {
    t: &'a theme::Tokens,
    mono: SharedString,
    scale: f32,
    default_rtl: bool,
    view: WeakEntity<RichView>,
    ids: std::cell::Cell<usize>,
}

impl Ctx<'_> {
    /// A fresh id for a text element inside the block being drawn.
    fn next(&self) -> usize {
        let n = self.ids.get();
        self.ids.set(n + 1);
        n
    }
}

/// A row whose children run in the paragraph's own direction.
fn dir_row(rtl: bool) -> Div {
    let row = row().items_start();
    if rtl {
        row.flex_row_reverse()
    } else {
        row
    }
}

/// Padding at the start edge of a paragraph running `rtl`.
fn pad_start(element: Div, rtl: bool, amount: gpui_kit::Rems) -> Div {
    if rtl {
        element.pr(amount)
    } else {
        element.pl(amount)
    }
}

/// A document's blocks, drawn, in a list that lays out only what is on screen.
pub struct RichView {
    doc: Arc<Document>,
    list: ListState,
    names: Vec<SharedString>,
    scale: f32,
    column: gpui_kit::Rems,
    label: SharedString,
    default_rtl: bool,
    _release: Subscription,
}

impl gpui_kit::EventEmitter<RichEvent> for RichView {}

/// Makes the names of a document's blocks unique: the second "Introduction" is "Introduction (2)".
fn unique_names(blocks: &[Block]) -> Vec<SharedString> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    blocks
        .iter()
        .map(|block| {
            let mut name: String = block.name().split_whitespace().collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                name = match block {
                    Block::Picture(_) => t!("Picture").to_string(),
                    Block::Code(_) => t!("Code").to_string(),
                    _ => return SharedString::default(),
                };
            }
            if name.chars().count() > 160 {
                name = name.chars().take(160).collect::<String>() + "…";
            }
            let count = seen.entry(name.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                name = format!("{name} ({count})");
            }
            SharedString::from(name)
        })
        .collect()
}

impl RichView {
    /// An empty view whose text column is at most `column` wide. `label` names it for screen readers.
    pub fn new(column: gpui_kit::Rems, label: SharedString, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, gpui_kit::px(600.));
        let this = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            this.update(cx, |_, cx| cx.emit(RichEvent::Scrolled)).ok();
        });
        // The decoded pictures are let go of when the view goes.
        let release = cx.on_release(|this, cx| this.forget_pictures(cx));
        Self {
            doc: Arc::new(Document::default()),
            list,
            names: Vec::new(),
            scale: 1.,
            column,
            label,
            default_rtl: false,
            _release: release,
        }
    }

    /// Shows `doc`, at its start. `default_rtl` is the direction of paragraphs with no letters to tell by.
    pub fn set_document(&mut self, doc: Arc<Document>, default_rtl: bool, cx: &mut Context<Self>) {
        self.forget_pictures(cx);
        self.names = unique_names(&doc.blocks);
        self.list.reset(doc.blocks.len());
        self.doc = doc;
        self.default_rtl = default_rtl;
        cx.notify();
    }

    /// The document shown.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Names the view for screen readers.
    pub fn set_label(&mut self, label: SharedString, cx: &mut Context<Self>) {
        self.label = label;
        cx.notify();
    }

    /// Sets the text size, 1.0 being the usual reading size.
    pub fn set_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        if (self.scale - scale).abs() > f32::EPSILON {
            self.scale = scale;
            self.list.remeasure();
            cx.notify();
        }
    }

    /// Lets go of the decoded pictures of the document that was on screen.
    fn forget_pictures(&mut self, cx: &mut App) {
        for image in self.doc.images() {
            image.remove_asset(cx);
        }
    }

    /// Scrolls so block `ix` is at the top.
    pub fn scroll_to_block(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ix = ix.min(self.doc.blocks.len().saturating_sub(1));
        self.list.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: gpui_kit::px(0.),
        });
        cx.notify();
    }

    /// Scrolls to where `id` is, if the document has it.
    pub fn scroll_to_anchor(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        match self.doc.anchors.get(id).copied() {
            Some(ix) => {
                self.scroll_to_block(ix, cx);
                true
            }
            None => false,
        }
    }

    /// Scrolls to the very start.
    pub fn scroll_to_start(&mut self, cx: &mut Context<Self>) {
        self.scroll_to_block(0, cx);
    }

    /// Scrolls to the very end.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list.scroll_to_end();
        cx.notify();
    }

    /// The block at the top of the view.
    pub fn top_block(&self) -> usize {
        self.list
            .logical_scroll_top()
            .item_ix
            .min(self.doc.blocks.len().saturating_sub(1))
    }

    /// How far down the document the top of the view is, 0 to 1 (by blocks, which is what is known before they are
    /// all laid out).
    pub fn fraction(&self) -> f32 {
        let count = self.doc.blocks.len();
        if count <= 1 {
            return 0.;
        }
        (self.top_block() as f32 / (count - 1) as f32).clamp(0., 1.)
    }

    /// Whether the view shows the very start of the document.
    pub fn at_start(&self) -> bool {
        let top = self.list.logical_scroll_top();
        top.item_ix == 0 && top.offset_in_item <= gpui_kit::px(0.5)
    }

    /// Whether the last block is in view (it fits, or the view is scrolled to the end). Known once the last block
    /// has been laid out, which a frame after the end was scrolled to does.
    pub fn at_end(&self) -> bool {
        let count = self.doc.blocks.len();
        if count == 0 {
            return true;
        }
        match self.list.bounds_for_item(count - 1) {
            Some(bounds) => bounds.bottom() <= self.list.viewport_bounds().bottom() + gpui_kit::px(1.),
            None => false,
        }
    }

    /// Scrolls a page (0.85 of what shows). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        if down && self.at_end() || !down && self.at_start() {
            return false;
        }
        let height = self.list.viewport_bounds().size.height;
        let step = if height > gpui_kit::px(1.) {
            height * 0.85
        } else {
            gpui_kit::px(400.)
        };
        self.list.scroll_by(if down { step } else { -step });
        cx.notify();
        true
    }

    /// A key that scrolls the document: the arrows, Page Up and Down, Home and End. Returns whether it was one.
    pub fn key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "down" => self.list.scroll_by(gpui_kit::px(48.)),
            "up" => self.list.scroll_by(gpui_kit::px(-48.)),
            "pagedown" => {
                self.page(true, cx);
            }
            "pageup" => {
                self.page(false, cx);
            }
            "home" => self.scroll_to_start(cx),
            "end" => self.scroll_to_end(cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// One block of the list.
    fn block(&mut self, ix: usize, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let doc = self.doc.clone();
        let Some(block) = doc.blocks.get(ix) else {
            return div().into_any_element();
        };
        let ctx = Ctx {
            t: &t,
            mono: theme::mono_font(cx),
            scale: self.scale,
            default_rtl: self.default_rtl,
            view: cx.entity().downgrade(),
            ids: std::cell::Cell::new(0),
        };
        let name = self.names.get(ix).cloned().unwrap_or_default();
        let body = block_element(block, Some((ix, name)), &ctx);
        div()
            .w_full()
            .flex()
            .justify_center()
            .px(space(4.))
            .child(div().w_full().max_w(self.column).min_w_0().child(body))
            .into_any_element()
    }
}

/// The element for a text: a `StyledText` with its runs, or an `InteractiveText` when it has links.
fn text_node(laid: Laid, ctx: &Ctx) -> AnyElement {
    let Laid {
        text,
        highlights,
        monos,
        links,
    } = laid;
    let mut styled = StyledText::new(SharedString::from(text)).with_highlights(highlights);
    if !monos.is_empty() {
        styled = styled.with_font_family_overrides(monos);
    }
    if links.is_empty() {
        return styled.into_any_element();
    }
    let hrefs: Vec<Arc<str>> = links.iter().map(|(_, href)| href.clone()).collect();
    let ranges: Vec<Range<usize>> = links.into_iter().map(|(range, _)| range).collect();
    let view = ctx.view.clone();
    InteractiveText::new(("rich-text", ctx.next()), styled)
        .on_click(ranges, move |ix, _, cx| {
            if let Some(href) = hrefs.get(ix) {
                let href = SharedString::from(href.to_string());
                view.update(cx, |_, cx| cx.emit(RichEvent::Link(href))).ok();
            }
        })
        .into_any_element()
}

/// A paragraph's text as an element: aligned in its own direction.
fn paragraph(text: &Text, rtl: bool, bold: bool, ctx: &Ctx) -> Div {
    let laid = lay_out(text, rtl, bold, ctx.t, &ctx.mono);
    let block = div().w_full().min_w_0().line_height(gpui_kit::relative(1.55));
    // `Start` and `End` are the paragraph's own edges, so a right-to-left paragraph's start is the right.
    let block = match (text.align, rtl) {
        (Align::Center, _) => block.text_center(),
        (Align::Start, false) | (Align::End, true) => block.text_left(),
        (Align::Start, true) | (Align::End, false) => block.text_right(),
    };
    block.child(text_node(laid, ctx))
}

/// The element for one block. `top` is the block's place in the list and its screen reader name, for a block of the
/// list itself; blocks inside a table's cell have none.
fn block_element(block: &Block, top: Option<(usize, SharedString)>, ctx: &Ctx) -> AnyElement {
    let t = ctx.t;
    let reading = gpui_kit::rems(READING * ctx.scale);
    let named = |element: Div, role: Role| -> AnyElement {
        match &top {
            Some((ix, name)) => {
                let element = element.id(("rich-block", *ix)).role(role);
                if name.is_empty() {
                    element.test_support().into_any_element()
                } else {
                    element.aria_label(name.clone()).test_support().into_any_element()
                }
            }
            None => element.into_any_element(),
        }
    };
    match block {
        Block::Heading { level, text } => {
            let rtl = text.is_rtl(ctx.default_rtl);
            let factor = match level {
                1 => 1.7,
                2 => 1.4,
                3 => 1.2,
                4 => 1.1,
                _ => 1.0,
            };
            named(
                div()
                    .w_full()
                    .pt(space(if *level <= 2 { 5. } else { 3. }))
                    .pb(space(2.))
                    .text_size(gpui_kit::rems(READING * factor * ctx.scale))
                    .text_color(t.foreground)
                    .child(paragraph(text, rtl, true, ctx)),
                Role::Heading,
            )
        }
        Block::Paragraph(text) => {
            let rtl = text.is_rtl(ctx.default_rtl);
            named(
                div()
                    .w_full()
                    .pb(space(3.))
                    .text_size(reading)
                    .text_color(t.foreground)
                    .child(paragraph(text, rtl, false, ctx)),
                Role::Paragraph,
            )
        }
        Block::Item { depth, marker, text } => {
            let rtl = text.is_rtl(ctx.default_rtl);
            let marker_box = div()
                .flex_none()
                .w(space(7.))
                .text_color(t.muted_foreground)
                .line_height(gpui_kit::relative(1.55))
                .child(SharedString::from(marker.clone()));
            let marker_box = if rtl {
                marker_box.text_left()
            } else {
                marker_box.text_right()
            };
            let text_box = div().flex_1().min_w_0().child(paragraph(text, rtl, false, ctx));
            // The marker and the text are findable by the tests, which check which side each is on.
            let (marker_box, text_box) = match &top {
                Some((ix, _)) => (
                    marker_box
                        .id(("rich-marker", *ix))
                        .role(Role::ListMarker)
                        .test_support()
                        .into_any_element(),
                    text_box
                        .id(("rich-item-text", *ix))
                        .role(Role::Group)
                        .test_support()
                        .into_any_element(),
                ),
                None => (marker_box.into_any_element(), text_box.into_any_element()),
            };
            let line = dir_row(rtl)
                .w_full()
                .gap(space(2.))
                .text_size(reading)
                .text_color(t.foreground)
                .child(marker_box)
                .child(text_box);
            named(
                pad_start(div().w_full().pb(space(1.5)), rtl, space(6. * f32::from(*depth))).child(line),
                Role::ListItem,
            )
        }
        Block::Quote { depth, text } => {
            let rtl = text.is_rtl(ctx.default_rtl);
            let mut line = dir_row(rtl).w_full().gap(space(3.));
            for _ in 0..(*depth).max(1) {
                line = line.child(bar_clone(t));
            }
            named(
                div()
                    .w_full()
                    .pb(space(3.))
                    .text_size(reading)
                    .text_color(t.muted_foreground)
                    .child(
                        line.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .italic()
                                .child(paragraph(text, rtl, false, ctx)),
                        ),
                    ),
                Role::Blockquote,
            )
        }
        Block::Code(code) => named(
            div()
                .w_full()
                .mb(space(3.))
                .px(space(3.))
                .py(space(2.))
                .rounded(theme::size::RADIUS)
                .bg(t.muted)
                .font_family(ctx.mono.clone())
                .text_size(gpui_kit::rems(READING * 0.9 * ctx.scale))
                .text_color(t.foreground)
                .text_left()
                .child(SharedString::from(code.clone())),
            Role::Code,
        ),
        Block::Rule => div()
            .w_full()
            .py(space(3.))
            .child(div().h(gpui_kit::px(1.)).w_full().bg(t.border))
            .into_any_element(),
        Block::PageBreak => div()
            .w_full()
            .py(space(4.))
            .child(div().h(gpui_kit::px(1.)).w_full().bg(t.border.opacity(0.5)))
            .into_any_element(),
        Block::Picture(picture) => {
            let body = match &picture.image {
                Some(image) => {
                    let image_el = img(ImageSource::Image(image.clone())).object_fit(ObjectFit::Contain);
                    match picture.size {
                        // Laid out at its own size, or smaller to fit, before it is decoded.
                        Some((w, h)) if w > 0 && h > 0 => div()
                            .max_w_full()
                            .w(gpui_kit::px(w as f32))
                            .aspect_ratio(w as f32 / h as f32)
                            .child(image_el.size_full())
                            .into_any_element(),
                        _ => div().max_w_full().child(image_el.max_w_full()).into_any_element(),
                    }
                }
                None => {
                    let alt: SharedString = if picture.alt.is_empty() {
                        t!("Picture")
                    } else {
                        picture.alt.clone().into()
                    };
                    row()
                        .gap(space(2.))
                        .px(space(3.))
                        .py(space(2.))
                        .rounded(theme::size::RADIUS)
                        .bg(t.muted)
                        .text_size(theme::size::SM)
                        .text_color(t.muted_foreground)
                        .child(icon(IconName::Image))
                        .child(alt)
                        .into_any_element()
                }
            };
            named(
                div().w_full().pb(space(3.)).flex().justify_center().child(body),
                Role::Image,
            )
        }
        Block::Table(table) => {
            let mut grid = col().w_full().mb(space(3.)).border_1().border_color(t.border);
            for row_data in &table.rows {
                let mut line = if table.rtl { row().flex_row_reverse() } else { row() }
                    .items_stretch()
                    .w_full();
                for cell in &row_data.cells {
                    let mut cell_el = col()
                        .min_w_0()
                        .flex_basis(gpui_kit::px(0.))
                        .flex_grow(cell.span.max(1) as f32)
                        .flex_shrink_0()
                        .border_1()
                        .border_color(t.border)
                        .p(space(2.))
                        .text_size(gpui_kit::rems(READING * 0.93 * ctx.scale));
                    if let Some(shade) = cell.shade.filter(|s| visible_fill(*s, t.dark)) {
                        let fill: Hsla = gpui_kit::rgb(shade).into();
                        cell_el = cell_el.bg(fill);
                    } else if row_data.header {
                        cell_el = cell_el.bg(t.muted);
                    }
                    for inner in &cell.blocks {
                        cell_el = cell_el.child(block_element(inner, None, ctx));
                    }
                    line = line.child(cell_el);
                }
                grid = grid.child(line);
            }
            named(grid, Role::Table)
        }
    }
}

/// A quote's bar (a fresh one each, since an element is used once).
fn bar_clone(t: &theme::Tokens) -> Div {
    div().flex_none().w(space(1.)).rounded_full().bg(t.border)
}

/// Whether a cell's fill is one to draw: Word's shading is made for white paper, so on a dark ground only the
/// dark fills are kept (text color is the theme's).
fn visible_fill(rgb: u32, dark: bool) -> bool {
    let (r, g, b) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
    let light = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.;
    if dark {
        light < 0.45
    } else {
        light > 0.35
    }
}

impl Render for RichView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let blocks = list(self.list.clone(), cx.processor(Self::block)).size_full();
        div()
            .id("rich-view")
            .role(Role::Document)
            .aria_label(self.label.clone())
            .test_support()
            .relative()
            .size_full()
            .min_h_0()
            .pt(space(3.))
            .child(blocks)
            .vertical_scrollbar(&self.list)
    }
}

/// A 1 x 1 PNG, for the tests of this module and of the readers that feed it.
#[cfg(test)]
pub(crate) const TEST_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49,
    0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xFF, 0xFF, 0x3F, 0x00, 0x05, 0xFE, 0x02, 0xFE, 0xA7, 0x35, 0x81, 0x84,
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::TEST_PNG as PNG;

    fn plain(text: &str) -> Text {
        let mut out = Text::new(Dir::Auto);
        out.push(text, &Style::default());
        out
    }

    #[test]
    fn the_first_strong_letter_decides_the_direction() {
        assert_eq!(first_strong_rtl("مرحبا بالعالم"), Some(true));
        assert_eq!(first_strong_rtl("123 - مرحبا"), Some(true));
        assert_eq!(first_strong_rtl("Hello مرحبا"), Some(false));
        assert_eq!(first_strong_rtl("שלום"), Some(true));
        assert_eq!(first_strong_rtl("1234 ..."), None);
        assert!(plain("مرحبا").is_rtl(false));
        assert!(!plain("hello").is_rtl(true));
        // No letters: the document's direction.
        assert!(plain("***").is_rtl(true) && !plain("***").is_rtl(false));
        // Said, whatever the letters.
        let mut forced = plain("hello");
        forced.dir = Dir::Rtl;
        assert!(forced.is_rtl(false));
        let mut forced = plain("مرحبا");
        forced.dir = Dir::Ltr;
        assert!(!forced.is_rtl(true));
    }

    #[test]
    fn languages_that_read_right_to_left() {
        for code in ["ar", "ar-EG", "he", "fa_IR", "UR", "ckb"] {
            assert!(rtl_language(code), "{code}");
        }
        for code in ["en", "en-US", "de", "ja", ""] {
            assert!(!rtl_language(code), "{code}");
        }
    }

    #[test]
    fn spans_with_the_same_style_join_and_blank_ends_are_trimmed() {
        let mut text = Text::new(Dir::Auto);
        let bold = Style {
            bold: true,
            ..Default::default()
        };
        text.push("  a", &Style::default());
        text.push("b", &Style::default());
        text.push(" c ", &bold);
        text.push("  ", &Style::default());
        assert_eq!(text.spans.len(), 3);
        text.trim();
        assert_eq!(text.plain(), "ab c");
        assert_eq!(text.spans.len(), 2);
        assert!(!text.is_blank());
        assert!(plain("  \n ").is_blank());
    }

    #[test]
    fn scripts_use_unicodes_raised_and_lowered_forms_when_all_have_one() {
        assert_eq!(scripted("12", Script::Super).as_deref(), Some("¹²"));
        assert_eq!(scripted("2+1", Script::Sub).as_deref(), Some("₂₊₁"));
        assert_eq!(scripted("a1", Script::Super), None);
        assert_eq!(scripted("1", Script::Normal), None);
    }

    #[test]
    fn a_paragraph_lays_out_with_its_runs_in_the_drawn_strings_bytes() {
        let _guard = testing::lock();
        let t = theme::Tokens::light();
        let mono: SharedString = "Cascadia Mono".into();
        let mut text = Text::new(Dir::Auto);
        text.push("مرحبا ", &Style::default());
        text.push(
            "link",
            &Style {
                link: Some("https://example.com".into()),
                ..Default::default()
            },
        );
        text.push(
            " code",
            &Style {
                code: true,
                bold: true,
                ..Default::default()
            },
        );
        let laid = lay_out(&text, true, false, &t, &mono);
        // Wrapped as a right-to-left isolate; every range is on boundaries and inside the drawn text.
        assert!(laid.text.starts_with(RLI) && laid.text.ends_with(PDI));
        for (range, _) in &laid.highlights {
            assert!(laid.text.is_char_boundary(range.start) && laid.text.is_char_boundary(range.end));
        }
        let (range, href) = &laid.links[0];
        assert_eq!(&laid.text[range.clone()], "link");
        assert_eq!(&**href, "https://example.com");
        assert_eq!(&laid.text[laid.monos[0].0.clone()], " code");
        // Highlights are in order and do not overlap (GPUI requires it).
        for pair in laid.highlights.windows(2) {
            assert!(pair[0].0.end <= pair[1].0.start);
        }
        // A left-to-right paragraph of English has no marks.
        let laid = lay_out(&plain("hello"), false, false, &t, &mono);
        assert_eq!(laid.text, "hello");
        // A run of Arabic inside an English paragraph, said to be right to left, is isolated.
        let mut mixed = Text::new(Dir::Ltr);
        mixed.push("a ", &Style::default());
        mixed.push(
            "ب",
            &Style {
                dir: Dir::Rtl,
                ..Default::default()
            },
        );
        let laid = lay_out(&mixed, false, false, &t, &mono);
        assert_eq!(laid.text, format!("a {RLI}ب{PDI}"));
        assert_eq!(&laid.text[laid.highlights[1].0.clone()], "ب");
    }

    #[test]
    fn colors_that_vanish_on_the_ground_are_ignored() {
        assert!(visible_color(0x000000, false) && !visible_color(0x000000, true));
        assert!(!visible_color(0xffffff, false) && visible_color(0xffffff, true));
        assert!(visible_color(0xc00000, false));
    }

    #[test]
    fn the_builder_puts_ids_on_the_next_block_and_cuts_long_tables() {
        let mut builder = Builder::new();
        builder.anchor("start");
        builder.push(Block::Paragraph(plain("one")));
        builder.anchor("two");
        builder.anchor("");
        builder.push(Block::Rule);
        let rows: Vec<Row> = (0..(TABLE_CHUNK * 2 + 5))
            .map(|_| Row {
                cells: vec![Cell {
                    blocks: vec![Block::Paragraph(plain("x"))],
                    span: 1,
                    shade: None,
                }],
                header: false,
            })
            .collect();
        builder.anchor("table");
        builder.push(Block::Table(Table { rows, rtl: false }));
        builder.anchor("end");
        let doc = builder.finish();
        assert_eq!(doc.anchors["start"], 0);
        assert_eq!(doc.anchors["two"], 1);
        assert_eq!(doc.anchors["table"], 2);
        // 85 rows are three tables; an id with nothing after it is on the last block.
        assert_eq!(doc.blocks.len(), 5);
        assert_eq!(doc.anchors["end"], 4);
        assert!(doc.plain_text().starts_with("one\n"));
    }

    #[test]
    fn the_plain_text_has_a_line_for_each_paragraph() {
        let mut builder = Builder::new();
        builder.push(Block::Heading {
            level: 1,
            text: plain("عنوان"),
        });
        builder.push(Block::Item {
            depth: 0,
            marker: "1.".into(),
            text: plain("بند"),
        });
        builder.push(Block::Code("a\nb".into()));
        let doc = builder.finish();
        assert_eq!(doc.plain_text(), "عنوان\n1. بند\na\nb\n");
        assert!(doc.mostly_rtl());
    }

    #[test]
    fn pictures_are_sniffed_sized_and_capped() {
        assert_eq!(sniff_image(PNG), Some(ImageFormat::Png));
        assert_eq!(sniff_image(&[0xff, 0xd8, 0xff, 0xe0]), Some(ImageFormat::Jpeg));
        assert_eq!(sniff_image(b"GIF89a"), Some(ImageFormat::Gif));
        assert_eq!(
            sniff_image(b"<?xml version='1.0'?><svg width='3' height='4'/>"),
            Some(ImageFormat::Svg)
        );
        assert_eq!(sniff_image(b"\x01\0\0\0 EMF"), None);
        let picture = picture_of(PNG.to_vec(), "alt", None);
        assert!(picture.image.is_some());
        assert_eq!(picture.size, Some((1, 1)));
        // What GPUI cannot draw keeps its alt text; so does what is far too big.
        let emf = picture_of(vec![1, 0, 0, 0, 0], "metafile", Some((40, 20)));
        assert!(emf.image.is_none() && emf.alt == "metafile" && emf.size == Some((40, 20)));
        let huge = picture_of(PNG.to_vec(), "huge", Some((100_000, 100_000)));
        assert!(huge.image.is_none());
    }

    #[test]
    fn names_are_unique_within_a_document() {
        let blocks = vec![
            Block::Paragraph(plain("same")),
            Block::Paragraph(plain("same")),
            Block::Paragraph(plain("other  \n text")),
            Block::Rule,
            Block::Picture(picture_of(PNG.to_vec(), "", None)),
        ];
        let names = unique_names(&blocks);
        assert_eq!(names[0], "same");
        assert_eq!(names[1], "same (2)");
        assert_eq!(names[2], "other text");
        assert_eq!(names[3], "");
        assert_eq!(names[4], "Picture");
    }

    /// A window with a rich view over `doc`.
    fn open(cx: &mut TestAppContext, doc: Document) -> (Entity<RichView>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |_, cx| {
            cx.new(|cx| {
                let mut view = RichView::new(gpui_kit::rems(40.), "Test document".into(), cx);
                let rtl = doc.mostly_rtl();
                view.set_document(Arc::new(doc), rtl, cx);
                view
            })
        });
        (view, visual)
    }

    fn frames(visual: &mut VisualTestContext) {
        for _ in 0..4 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
    }

    /// A label as the interface says it now: English UI text goes through the catalog (the Arabic tests read the
    /// Arabic), text the catalog doesn't know (the documents' own Arabic) is left alone.
    fn said(label: &str) -> String {
        glint_model::i18n::translate(crate::i18n::language(), label).to_string()
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        let label = said(label);
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(&label).is_some()
        })
    }

    #[gpui_kit::test]
    fn arabic_blocks_are_drawn_with_their_roles_and_names(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let mut builder = Builder::new();
        builder.push(Block::Heading {
            level: 1,
            text: plain("الفصل الأول"),
        });
        builder.push(Block::Paragraph(plain("بسم الله الرحمن الرحيم")));
        builder.push(Block::Item {
            depth: 0,
            marker: "1.".into(),
            text: plain("البند الأول"),
        });
        builder.push(Block::Quote {
            depth: 1,
            text: plain("اقتباس"),
        });
        builder.push(Block::Code("let x = 1;".into()));
        builder.push(Block::Picture(picture_of(PNG.to_vec(), "صورة", None)));
        builder.push(Block::Table(Table {
            rows: vec![Row {
                cells: vec![
                    Cell {
                        blocks: vec![Block::Paragraph(plain("أ"))],
                        span: 1,
                        shade: None,
                    },
                    Cell {
                        blocks: vec![Block::Paragraph(plain("ب"))],
                        span: 1,
                        shade: Some(0xeeeeee),
                    },
                ],
                header: true,
            }],
            rtl: true,
        }));
        let (_view, mut visual) = open(cx, builder.finish());
        frames(&mut visual);
        assert!(has(&mut visual, "Test document"));
        for label in [
            "الفصل الأول",
            "بسم الله الرحمن الرحيم",
            "البند الأول",
            "اقتباس",
            "let x = 1;",
            "صورة",
            "Table",
        ] {
            assert!(has(&mut visual, label), "{label}");
        }
        visual.update(|window, _| {
            assert_eq!(window.find_by_label("الفصل الأول").role(), Some(Role::Heading));
            assert_eq!(
                window.find_by_label("بسم الله الرحمن الرحيم").role(),
                Some(Role::Paragraph)
            );
            assert_eq!(window.find_by_label("البند الأول").role(), Some(Role::ListItem));
            assert_eq!(window.find_by_label("اقتباس").role(), Some(Role::Blockquote));
            assert_eq!(window.find_by_label("صورة").role(), Some(Role::Image));
            assert_eq!(window.find_by_label(&said("Table")).role(), Some(Role::Table));
        });
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_right_to_left_item_has_its_marker_on_the_right_and_a_left_to_right_one_on_the_left(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        // The interface is left to right: the paragraphs' own direction is what puts the marker where it is.
        testing::set_rtl(false);
        let mut builder = Builder::new();
        builder.push(Block::Item {
            depth: 0,
            marker: "1.".into(),
            text: plain("بند عربي طويل"),
        });
        builder.push(Block::Item {
            depth: 0,
            marker: "2.".into(),
            text: plain("an english item"),
        });
        // A paragraph that says it runs right to left, whatever its letters.
        let mut forced = plain("ordered right to left");
        forced.dir = Dir::Rtl;
        builder.push(Block::Item {
            depth: 1,
            marker: "a.".into(),
            text: forced,
        });
        let (_view, mut visual) = open(cx, builder.finish());
        frames(&mut visual);
        visual.update(|window, _| {
            for (ix, marker_is_after_text) in [(0usize, true), (1, false), (2, true)] {
                let marker = window.find(("rich-marker", ix)).bounds();
                let text = window.find(("rich-item-text", ix)).bounds();
                if marker_is_after_text {
                    assert!(
                        marker.left() >= text.right() - gpui_kit::px(1.),
                        "item {ix}: {marker:?} {text:?}"
                    );
                } else {
                    assert!(
                        marker.right() <= text.left() + gpui_kit::px(1.),
                        "item {ix}: {marker:?} {text:?}"
                    );
                }
            }
            // A nested right-to-left item is indented from the right, and its text does not move on the left.
            let outer = window.find(("rich-marker", 0usize)).bounds();
            let inner = window.find(("rich-marker", 2usize)).bounds();
            assert!(
                inner.right() <= outer.right() - gpui_kit::px(20.),
                "{inner:?} {outer:?}"
            );
            let (outer, inner) = (
                window.find(("rich-item-text", 0usize)).bounds(),
                window.find(("rich-item-text", 2usize)).bounds(),
            );
            assert!(
                (inner.left() - outer.left()).abs() < gpui_kit::px(1.),
                "{inner:?} {outer:?}"
            );
        });
    }

    #[gpui_kit::test]
    fn a_long_document_only_draws_the_blocks_on_screen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut builder = Builder::new();
        for i in 0..600 {
            builder.push(Block::Paragraph(plain(&format!("paragraph number {i}"))));
        }
        let (view, mut visual) = open(cx, builder.finish());
        frames(&mut visual);
        assert!(has(&mut visual, "paragraph number 0"));
        let drawn = visual.update(|window, _| window.find_all_by_role(Role::Paragraph).len());
        assert!(drawn > 3 && drawn < 80, "{drawn} paragraphs drawn");
        // Scrolling to a block brings it on screen and takes the first away.
        view.update(&mut visual, |view, cx| view.scroll_to_block(400, cx));
        frames(&mut visual);
        assert!(has(&mut visual, "paragraph number 400"));
        assert!(!has(&mut visual, "paragraph number 0"));
        assert_eq!(view.read_with(&visual, |v, _| v.top_block()), 400);
        assert!(!view.read_with(&visual, |v, _| v.at_start()));
        view.update(&mut visual, |view, cx| view.scroll_to_end(cx));
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.at_end()));
        assert!(has(&mut visual, "paragraph number 599"));
        view.update(&mut visual, |view, cx| view.scroll_to_start(cx));
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.at_start() && !v.at_end()));
    }

    #[gpui_kit::test]
    fn paging_moves_by_the_view_and_stops_at_the_ends(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut builder = Builder::new();
        for i in 0..200 {
            builder.push(Block::Paragraph(plain(&format!("line {i}"))));
        }
        let (view, mut visual) = open(cx, builder.finish());
        frames(&mut visual);
        assert!(
            !view.update(&mut visual, |v, cx| v.page(false, cx)),
            "already at the start"
        );
        assert!(view.update(&mut visual, |v, cx| v.page(true, cx)));
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.top_block() > 0));
        let before = view.read_with(&visual, |v, _| v.top_block());
        assert!(view.update(&mut visual, |v, cx| v.page(false, cx)));
        frames(&mut visual);
        assert!(view.read_with(&visual, |v, _| v.top_block() < before));
        assert!(view.update(&mut visual, |v, cx| v.key("end", cx)));
        frames(&mut visual);
        assert!(
            !view.update(&mut visual, |v, cx| v.page(true, cx)),
            "already at the end"
        );
        assert!(!view.update(&mut visual, |v, cx| v.key("x", cx)));
    }

    #[gpui_kit::test]
    fn ids_scroll_to_their_block(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut builder = Builder::new();
        for i in 0..100 {
            if i == 70 {
                builder.anchor("seventy");
            }
            builder.push(Block::Paragraph(plain(&format!("entry {i}"))));
        }
        let (view, mut visual) = open(cx, builder.finish());
        frames(&mut visual);
        assert!(view.update(&mut visual, |v, cx| v.scroll_to_anchor("seventy", cx)));
        assert!(!view.update(&mut visual, |v, cx| v.scroll_to_anchor("nowhere", cx)));
        frames(&mut visual);
        assert!(has(&mut visual, "entry 70"));
        assert!(view.read_with(&visual, |v, _| v.fraction() > 0.6));
    }

    #[gpui_kit::test]
    fn a_click_on_a_link_asks_the_owner(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let mut text = Text::new(Dir::Auto);
        text.push(
            "visit",
            &Style {
                link: Some("https://example.com".into()),
                ..Default::default()
            },
        );
        let mut builder = Builder::new();
        builder.push(Block::Paragraph(text));
        let (view, mut visual) = open(cx, builder.finish());
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let sink = heard.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &RichEvent, _| {
                if let RichEvent::Link(href) = event {
                    sink.borrow_mut().push(href.to_string());
                }
            })
        });
        frames(&mut visual);
        // The block is clicked at its first character, which is the link.
        visual.update(|window, cx| {
            window.click_at(
                ("rich-block", 0usize),
                gpui_kit::point(gpui_kit::px(8.), gpui_kit::px(8.)),
                cx,
            )
        });
        frames(&mut visual);
        assert_eq!(*heard.borrow(), vec!["https://example.com".to_string()]);
    }
}
