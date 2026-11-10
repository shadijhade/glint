//! Word documents laid out without Office (`DocxView` in DocxView.tsx).
//!
//! The TypeScript gave the file to docx-preview, which made HTML. Here `word/document.xml` is read straight into
//! the [`rich`](super::rich) model with `zip` and `roxmltree` (no docx crate: docx-rs is a new dependency), and
//! the shared [`RichView`] draws it.
//!
//! | TypeScript / docx-preview                    | Here                                                          |
//! |----------------------------------------------|---------------------------------------------------------------|
//! | `DOCX_EXTS`, `DOCX_MAX` (choose.rs)          | [`DOCX_EXTS`], [`DOCX_MAX`]                                   |
//! | paragraphs, runs, `w:rPr`                    | [`Reader::paragraph`], [`Reader::run`]: bold, italic, underline, strike, color, super and subscript, monospace fonts as code |
//! | heading styles, `Title`, `outlineLvl`        | [`StyleDef`] and [`Styles::resolve`]: names (`heading 1`…), `basedOn` chains, `docDefaults` |
//! | lists (`w:numPr`, numbering.xml)             | [`Numbering`]: each level's format and text (`%1.%2`), counters per list; bullets from Symbol and Wingdings become `•`, `◦`, `▪`; Arabic and Hebrew letter formats |
//! | tables (`w:tbl`)                             | [`Reader::table`]: `gridSpan`, header rows, shading, `bidiVisual`; nested tables and any blocks in a cell |
//! | images (`w:drawing`, VML `v:imagedata`)     | [`Reader::scan_drawing`]: the picture from `word/media`, its size and alt text; text boxes' paragraphs follow the paragraph they are in |
//! | hyperlinks, bookmarks, `#anchor` links       | `w:hyperlink` (relationship or `w:anchor`), `w:bookmarkStart` as [`Builder::anchor`] |
//! | footnotes and endnotes (`renderFootnotes`)   | numbered superscript links in the text, the notes at the end after a rule |
//! | right to left: `w:bidi`, `w:rtl`, `w:jc`      | a paragraph's [`Dir`] (as Word does: no `w:bidi` is left to right), a run's own direction, `jc` as start and end |
//! | `renderChanges: false`, `renderComments: false` | inserted text is shown, deleted text and comments are not   |
//! | the failed state's `fallback`                | [`DocxEvent::Failed`] and the file's icon with a note         |
//!
//! # Where the previewer is placed (for the coordinator)
//!
//! - **Which `choose.rs` arm.** `Choice::later(Body::Shell(Fallback::Icon), "docx")` for
//!   `readable && !archived && one_of(entry, &DOCX_EXTS) && entry.size <= DOCX_MAX` becomes a `Body::Docx`.
//! - **Make it.** `cx.new(|cx| DocxView::new(path, window, cx))` in `frame.rs`'s `build`.
//! - **When it can't lay the file out.** It emits [`DocxEvent::Failed`] and shows the file's icon with a note; the
//!   TypeScript showed its `fallback` (Windows' previewer over the icon). `frame.rs` can subscribe and rebuild the
//!   body as `Body::Shell(Fallback::Icon)` on the event.
//! - **Selection moves.** A new `DocxView` per file, or [`DocxView::set_path`].
//! - **Quick Look's Page Down / Page Up.** `page(down, cx)`; add `DocxView` to `page_of!`.
//!
//! # Not ported
//!
//! - **Pages as paper.** docx-preview drew each page as a sheet, scaled to the pane, and flowed the text when the
//!   pane was narrow. This is always the flowing text, in a column (the design note's recommendation). Page size,
//!   margins, columns, headers and footers (`renderHeaders`, `renderFooters`) are not drawn.
//! - **Fonts, sizes, indents, spacing, line numbers, borders, floating positions, drop caps.** Text uses the
//!   theme's font; only the bold, italic, underline, strike, color, raised and lowered runs are drawn. Colors too
//!   close to the background are dropped.
//! - **Vertical merges** (`w:vMerge`): the merged cells below the first are empty.
//! - **Equations** (OMML) show their text; **SmartArt, charts, WordArt and embedded objects** show nothing, and
//!   a picture in a format GPUI can't draw (EMF, WMF) shows its alt text.
//! - **Embedded HTML** (`altChunk`) isn't shown, as before.
//! - **Tracked changes and comments** aren't marked, as before.
//! - **Text isn't selectable or copyable** (see `rich.rs`).

use super::book::{any_attr, child, flat_text, is_external, lname, parse_xml, resolve, Archive};
use super::rich::{
    picture_of, Align, Block, Builder, Cell, Dir, Document, Picture, RichEvent, RichView, Row, Script, Style, Table,
    Text,
};
use super::text::{big_icon, redraw_after_delay, waiting};
use crate::kit::prelude::*;
use crate::{actions, t};
use gpui_kit::{EventEmitter, KeyDownEvent, MouseButton, Task};
use roxmltree::Node;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// The Word formats this view lays out (`DOCX_EXTS`).
pub const DOCX_EXTS: [&str; 4] = ["docx", "docm", "dotx", "dotm"];
/// Documents bigger than this aren't laid out (`DOCX_MAX`).
pub const DOCX_MAX: u64 = 40 * 1024 * 1024;
/// The most of `word/document.xml` read.
const DOCUMENT_LIMIT: u64 = 128 * 1024 * 1024;
/// The most blocks one document is read into.
const MAX_BLOCKS: usize = 200_000;
/// The most picture bytes one document keeps.
const PICTURE_BUDGET: usize = 128 * 1024 * 1024;
/// How deep tables and text boxes may nest.
const MAX_DEPTH: u32 = 16;

// ---- Reading the styles ----------------------------------------------------------------------------------------

/// What a `w:rPr` says about a run; none where it says nothing.
#[derive(Clone, Debug, Default, PartialEq)]
struct RProps {
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    rtl: Option<bool>,
    code: Option<bool>,
    color: Option<u32>,
    script: Option<Script>,
}

impl RProps {
    /// Puts `other` over `self`.
    fn overlay(&mut self, other: &RProps) {
        self.bold = other.bold.or(self.bold);
        self.italic = other.italic.or(self.italic);
        self.underline = other.underline.or(self.underline);
        self.strike = other.strike.or(self.strike);
        self.rtl = other.rtl.or(self.rtl);
        self.code = other.code.or(self.code);
        self.color = other.color.or(self.color);
        self.script = other.script.or(self.script);
    }
}

/// What a `w:pPr` says about a paragraph.
#[derive(Clone, Debug, Default, PartialEq)]
struct PProps {
    heading: Option<u8>,
    bidi: Option<bool>,
    jc: Option<Align>,
    num_id: Option<u32>,
    ilvl: Option<u8>,
    quote: Option<bool>,
    page_break_before: Option<bool>,
}

impl PProps {
    fn overlay(&mut self, other: &PProps) {
        self.heading = other.heading.or(self.heading);
        self.bidi = other.bidi.or(self.bidi);
        self.jc = other.jc.or(self.jc);
        self.num_id = other.num_id.or(self.num_id);
        self.ilvl = other.ilvl.or(self.ilvl);
        self.quote = other.quote.or(self.quote);
        self.page_break_before = other.page_break_before.or(self.page_break_before);
    }
}

/// `w:val`, or none.
fn val<'a>(n: Node<'a, '_>) -> Option<&'a str> {
    any_attr(n, "val")
}

/// A toggle property (`<w:b/>`, `<w:b w:val="0"/>`): on unless it says off.
fn on(n: Node) -> bool {
    !matches!(val(n), Some("0" | "false" | "off" | "none"))
}

/// Fonts that are code (`Consolas`, `Courier New`, `Cascadia Mono`…).
fn is_mono(font: &str) -> bool {
    let f = font.to_ascii_lowercase();
    [
        "consolas",
        "courier",
        "cascadia",
        "lucida console",
        "monaco",
        "menlo",
        "source code",
        "fira code",
    ]
    .iter()
    .any(|m| f.contains(m))
        || f.ends_with(" mono")
}

fn read_rpr(rpr: Node) -> RProps {
    let mut r = RProps::default();
    for c in rpr.children().filter(|c| c.is_element()) {
        match lname(c) {
            "b" => r.bold = Some(on(c)),
            "i" => r.italic = Some(on(c)),
            "u" => r.underline = Some(on(c)),
            "strike" | "dstrike" => r.strike = Some(on(c)),
            "rtl" => r.rtl = Some(on(c)),
            "color" => {
                r.color = val(c)
                    .filter(|v| v.len() == 6)
                    .and_then(|v| u32::from_str_radix(v, 16).ok());
            }
            "vertAlign" => {
                r.script = match val(c) {
                    Some("superscript") => Some(Script::Super),
                    Some("subscript") => Some(Script::Sub),
                    _ => Some(Script::Normal),
                }
            }
            "rFonts" => {
                let font = any_attr(c, "ascii").or_else(|| any_attr(c, "hAnsi")).unwrap_or("");
                if !font.is_empty() {
                    r.code = Some(is_mono(font));
                }
            }
            _ => {}
        }
    }
    r
}

fn read_ppr(ppr: Node) -> PProps {
    let mut p = PProps::default();
    for c in ppr.children().filter(|c| c.is_element()) {
        match lname(c) {
            "bidi" => p.bidi = Some(on(c)),
            "jc" => {
                p.jc = Some(match val(c) {
                    Some("right" | "end") => Align::End,
                    Some("center") => Align::Center,
                    _ => Align::Start,
                })
            }
            "outlineLvl" => {
                p.heading = val(c)
                    .and_then(|v| v.parse::<u8>().ok())
                    .filter(|n| *n < 9)
                    .map(|n| (n + 1).min(6));
            }
            "numPr" => {
                for k in c.children().filter(|k| k.is_element()) {
                    match lname(k) {
                        "numId" => p.num_id = val(k).and_then(|v| v.parse().ok()),
                        "ilvl" => p.ilvl = val(k).and_then(|v| v.parse::<u8>().ok()).map(|n| n.min(8)),
                        _ => {}
                    }
                }
            }
            "pageBreakBefore" => p.page_break_before = Some(on(c)),
            _ => {}
        }
    }
    p
}

/// One `w:style`.
#[derive(Clone, Debug, Default)]
struct StyleDef {
    based_on: Option<String>,
    p: PProps,
    r: RProps,
}

/// The document's styles.
#[derive(Default)]
struct Styles {
    by_id: HashMap<String, StyleDef>,
    defaults: (PProps, RProps),
}

impl Styles {
    fn read(xml: &str) -> Self {
        let mut styles = Styles::default();
        let Some(doc) = parse_xml(xml) else {
            return styles;
        };
        let root = doc.root_element();
        if let Some(defaults) = child(root, "docDefaults") {
            if let Some(ppr) = child(defaults, "pPrDefault").and_then(|d| child(d, "pPr")) {
                styles.defaults.0 = read_ppr(ppr);
            }
            if let Some(rpr) = child(defaults, "rPrDefault").and_then(|d| child(d, "rPr")) {
                styles.defaults.1 = read_rpr(rpr);
            }
        }
        for style in root.children().filter(|c| c.is_element() && lname(*c) == "style") {
            let Some(id) = any_attr(style, "styleId") else {
                continue;
            };
            let name = child(style, "name").and_then(val).unwrap_or("").to_ascii_lowercase();
            let mut def = StyleDef {
                based_on: child(style, "basedOn").and_then(val).map(str::to_string),
                ..Default::default()
            };
            if let Some(ppr) = child(style, "pPr") {
                def.p = read_ppr(ppr);
            }
            if let Some(rpr) = child(style, "rPr") {
                def.r = read_rpr(rpr);
            }
            // Heading styles are known by their names; so are quotes.
            if let Some(level) = name.strip_prefix("heading ").and_then(|n| n.trim().parse::<u8>().ok()) {
                def.p.heading = Some(level.clamp(1, 6));
            } else if name == "title" {
                def.p.heading = Some(1);
            } else if name == "subtitle" {
                def.p.heading = Some(2);
            } else if name == "quote" || name == "intense quote" {
                def.p.quote = Some(true);
            }
            styles.by_id.insert(id.to_string(), def);
        }
        styles
    }

    /// A style's paragraph and run properties, over those of the styles it is based on and the document's defaults.
    fn resolve(&self, id: &str) -> (PProps, RProps) {
        let mut chain = Vec::new();
        let mut at = Some(id);
        while let Some(current) = at {
            if chain.len() > 16 || chain.contains(&current) {
                break;
            }
            let Some(def) = self.by_id.get(current) else {
                break;
            };
            chain.push(current);
            at = def.based_on.as_deref();
        }
        let (mut p, mut r) = self.defaults.clone();
        for current in chain.iter().rev() {
            if let Some(def) = self.by_id.get(*current) {
                p.overlay(&def.p);
                r.overlay(&def.r);
            }
        }
        (p, r)
    }
}

// ---- Numbering -------------------------------------------------------------------------------------------------

/// One level of a list: where it starts, how it counts and what its marker says.
#[derive(Clone, Debug)]
struct Level {
    start: u32,
    fmt: String,
    text: String,
}

impl Default for Level {
    fn default() -> Self {
        Level {
            start: 1,
            fmt: "decimal".into(),
            text: "%1.".into(),
        }
    }
}

/// `numbering.xml`: the abstract lists, the lists that use them, and where each counts from.
#[derive(Default)]
struct Numbering {
    abstracts: HashMap<u32, Vec<Level>>,
    nums: HashMap<u32, u32>,
    starts: HashMap<(u32, usize), u32>,
    counters: HashMap<u32, [u32; 9]>,
}

const ARABIC_ABJAD: [char; 28] = [
    'أ', 'ب', 'ج', 'د', 'ه', 'و', 'ز', 'ح', 'ط', 'ي', 'ك', 'ل', 'م', 'ن', 'س', 'ع', 'ف', 'ص', 'ق', 'ر', 'ش', 'ت', 'ث',
    'خ', 'ذ', 'ض', 'ظ', 'غ',
];
const ARABIC_ALPHA: [char; 28] = [
    'أ', 'ب', 'ت', 'ث', 'ج', 'ح', 'خ', 'د', 'ذ', 'ر', 'ز', 'س', 'ش', 'ص', 'ض', 'ط', 'ظ', 'ع', 'غ', 'ف', 'ق', 'ك', 'ل',
    'م', 'ن', 'ه', 'و', 'ي',
];
const HEBREW: [char; 22] = [
    'א', 'ב', 'ג', 'ד', 'ה', 'ו', 'ז', 'ח', 'ט', 'י', 'כ', 'ל', 'מ', 'נ', 'ס', 'ע', 'פ', 'צ', 'ק', 'ר', 'ש', 'ת',
];

/// `n` counted in Word's number format `fmt`.
fn number_text(n: u32, fmt: &str) -> String {
    let letters = |alphabet: &[char]| -> String {
        // a, b … z, aa, ab … as Word repeats a letter.
        let len = alphabet.len() as u32;
        let ix = (n.max(1) - 1) % len;
        let times = (n.max(1) - 1) / len + 1;
        std::iter::repeat_n(alphabet[ix as usize], times as usize).collect()
    };
    let latin = |upper: bool| -> String {
        let base = if upper { 'A' } else { 'a' };
        let ix = (n.max(1) - 1) % 26;
        let times = (n.max(1) - 1) / 26 + 1;
        std::iter::repeat_n(char::from_u32(base as u32 + ix).unwrap_or(base), times as usize).collect()
    };
    let roman = |upper: bool| -> String {
        if n == 0 || n >= 4000 {
            return n.to_string();
        }
        let mut left = n;
        let mut out = String::new();
        for (value, glyph) in [
            (1000, "m"),
            (900, "cm"),
            (500, "d"),
            (400, "cd"),
            (100, "c"),
            (90, "xc"),
            (50, "l"),
            (40, "xl"),
            (10, "x"),
            (9, "ix"),
            (5, "v"),
            (4, "iv"),
            (1, "i"),
        ] {
            while left >= value {
                out.push_str(glyph);
                left -= value;
            }
        }
        if upper {
            out.to_uppercase()
        } else {
            out
        }
    };
    match fmt {
        "lowerLetter" => latin(false),
        "upperLetter" => latin(true),
        "lowerRoman" => roman(false),
        "upperRoman" => roman(true),
        "decimalZero" => format!("{n:02}"),
        "arabicAbjad" => letters(&ARABIC_ABJAD),
        "arabicAlpha" => letters(&ARABIC_ALPHA),
        "hebrew1" | "hebrew2" if (1..=22).contains(&n) => HEBREW[(n - 1) as usize].to_string(),
        "none" => String::new(),
        _ => n.to_string(),
    }
}

/// A bullet's character: Symbol and Wingdings keep theirs in the private use area, which only those fonts draw.
fn bullet_char(text: &str) -> String {
    let c = text.chars().next();
    match c {
        None => "•".into(),
        Some('o') => "◦".into(),
        Some('\u{f0a7}' | '§' | '\u{f0fc}') => "▪".into(),
        Some(c) if (0xf000..=0xf0ff).contains(&(c as u32)) => "•".into(),
        Some(c) => c.to_string(),
    }
}

impl Numbering {
    fn read(xml: &str) -> Self {
        let mut numbering = Numbering::default();
        let Some(doc) = parse_xml(xml) else {
            return numbering;
        };
        let root = doc.root_element();
        for abs in root.children().filter(|c| c.is_element() && lname(*c) == "abstractNum") {
            let Some(id) = any_attr(abs, "abstractNumId").and_then(|v| v.parse::<u32>().ok()) else {
                continue;
            };
            let mut levels = vec![Level::default(); 9];
            for lvl in abs.children().filter(|c| c.is_element() && lname(*c) == "lvl") {
                let Some(ix) = any_attr(lvl, "ilvl")
                    .and_then(|v| v.parse::<usize>().ok())
                    .filter(|i| *i < 9)
                else {
                    continue;
                };
                let mut level = Level {
                    text: format!("%{}.", ix + 1),
                    ..Default::default()
                };
                for c in lvl.children().filter(|c| c.is_element()) {
                    match lname(c) {
                        "start" => level.start = val(c).and_then(|v| v.parse().ok()).unwrap_or(1),
                        "numFmt" => level.fmt = val(c).unwrap_or("decimal").to_string(),
                        "lvlText" => level.text = val(c).unwrap_or("").to_string(),
                        _ => {}
                    }
                }
                levels[ix] = level;
            }
            numbering.abstracts.insert(id, levels);
        }
        for num in root.children().filter(|c| c.is_element() && lname(*c) == "num") {
            let Some(id) = any_attr(num, "numId").and_then(|v| v.parse::<u32>().ok()) else {
                continue;
            };
            if let Some(abs) = child(num, "abstractNumId").and_then(val).and_then(|v| v.parse().ok()) {
                numbering.nums.insert(id, abs);
            }
            for over in num.children().filter(|c| c.is_element() && lname(*c) == "lvlOverride") {
                let ix = any_attr(over, "ilvl").and_then(|v| v.parse::<usize>().ok());
                let start = child(over, "startOverride")
                    .and_then(val)
                    .and_then(|v| v.parse::<u32>().ok());
                if let (Some(ix), Some(start)) = (ix, start) {
                    numbering.starts.insert((id, ix), start);
                }
            }
        }
        numbering
    }

    /// The marker of the next item of list `num_id` at `ilvl` ("•", "2.", "1.b)", "ب."), counting it.
    fn marker(&mut self, num_id: u32, ilvl: u8) -> String {
        let ilvl = usize::from(ilvl.min(8));
        let Some(levels) = self.nums.get(&num_id).and_then(|abs| self.abstracts.get(abs)) else {
            return "•".into();
        };
        let start_of = |k: usize| self.starts.get(&(num_id, k)).copied().unwrap_or(levels[k].start);
        let counters = self.counters.entry(num_id).or_insert([0; 9]);
        for (k, count) in counters.iter_mut().enumerate().take(ilvl) {
            if *count == 0 {
                *count = start_of(k);
            }
        }
        counters[ilvl] = if counters[ilvl] == 0 {
            start_of(ilvl)
        } else {
            counters[ilvl] + 1
        };
        for c in counters.iter_mut().skip(ilvl + 1) {
            *c = 0;
        }
        let level = &levels[ilvl];
        if level.fmt == "bullet" {
            return bullet_char(&level.text);
        }
        let mut out = String::new();
        let mut chars = level.text.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, chars.peek().and_then(|d| d.to_digit(10))) {
                ('%', Some(n)) if (1..=9).contains(&n) => {
                    chars.next();
                    let k = (n - 1) as usize;
                    let count = if counters[k] == 0 { start_of(k) } else { counters[k] };
                    out.push_str(&number_text(count, &levels[k].fmt));
                }
                _ => out.push(c),
            }
        }
        out
    }
}

// ---- Reading the document --------------------------------------------------------------------------------------

/// A relationship: where an id points.
struct Rel {
    target: String,
    external: bool,
}

/// What a paragraph is made of, in order.
enum Piece {
    Text(Text),
    Block(Block),
    Blocks(Vec<Block>),
}

/// A paragraph being read.
struct Para {
    pieces: Vec<Piece>,
    text: Text,
    dir: Dir,
}

impl Para {
    fn new(dir: Dir) -> Self {
        Para {
            pieces: Vec::new(),
            text: Text::new(dir),
            dir,
        }
    }

    /// Closes the text so far, to put a picture after it.
    fn split(&mut self) {
        let text = std::mem::replace(&mut self.text, Text::new(self.dir));
        if !text.is_blank() {
            self.pieces.push(Piece::Text(text));
        }
    }
}

/// A style for a run's properties.
fn style_of(r: &RProps, link: &Option<Arc<str>>) -> Style {
    Style {
        bold: r.bold.unwrap_or(false),
        italic: r.italic.unwrap_or(false),
        underline: r.underline.unwrap_or(false),
        strike: r.strike.unwrap_or(false),
        code: r.code.unwrap_or(false),
        script: r.script.unwrap_or_default(),
        color: r.color,
        link: link.clone(),
        dir: match r.rtl {
            Some(true) => Dir::Rtl,
            Some(false) => Dir::Ltr,
            None => Dir::Auto,
        },
    }
}

/// Which notes the text refers to, in order.
struct NoteRef {
    endnote: bool,
    id: String,
    number: u32,
}

struct Reader<'a> {
    arch: &'a Archive,
    rels: HashMap<String, Rel>,
    styles: Styles,
    numbering: Numbering,
    b: Builder,
    budget: usize,
    notes: Vec<NoteRef>,
    depth: u32,
    blocks: usize,
    /// The marker of the footnote being read, for its first paragraph.
    note_marker: Option<String>,
}

fn rels_of(arch: &Archive) -> HashMap<String, Rel> {
    let mut map = HashMap::new();
    if let Some(xml) = arch.read_text("word/_rels/document.xml.rels") {
        if let Some(doc) = parse_xml(&xml) {
            for rel in doc.root_element().children().filter(|c| c.is_element()) {
                if let (Some(id), Some(target)) = (rel.attribute("Id"), rel.attribute("Target")) {
                    map.insert(
                        id.to_string(),
                        Rel {
                            target: target.to_string(),
                            external: rel
                                .attribute("TargetMode")
                                .is_some_and(|m| m.eq_ignore_ascii_case("External")),
                        },
                    );
                }
            }
        }
    }
    map
}

/// A Word document, read into blocks: `word/document.xml`, its styles and numbering, relationships, pictures and
/// notes.
pub(super) fn read_docx_archive(arch: &Archive) -> Result<Document, String> {
    let body_xml = arch
        .read("word/document.xml", DOCUMENT_LIMIT)
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .trim_start_matches('\u{feff}')
                .to_string()
        })
        .ok_or_else(|| t!("Glint can't lay this document out.").to_string())?;
    let xml = parse_xml(&body_xml).ok_or_else(|| t!("Glint can't lay this document out.").to_string())?;
    let mut reader = Reader {
        arch,
        rels: rels_of(arch),
        styles: arch
            .read_text("word/styles.xml")
            .map(|x| Styles::read(&x))
            .unwrap_or_default(),
        numbering: arch
            .read_text("word/numbering.xml")
            .map(|x| Numbering::read(&x))
            .unwrap_or_default(),
        b: Builder::new(),
        budget: PICTURE_BUDGET,
        notes: Vec::new(),
        depth: 0,
        blocks: 0,
        note_marker: None,
    };
    let root = xml.root_element();
    let body = child(root, "body").ok_or_else(|| t!("Glint can't lay this document out.").to_string())?;
    reader.body_children(body);
    // Footnotes and endnotes, after the text, in the order they were referred to.
    if !reader.notes.is_empty() {
        let foot_text = arch.read_text("word/footnotes.xml");
        let end_text = arch.read_text("word/endnotes.xml");
        let foot = foot_text.as_deref().and_then(parse_xml);
        let end = end_text.as_deref().and_then(parse_xml);
        reader.b.push(Block::Rule);
        let notes = std::mem::take(&mut reader.notes);
        for note in notes {
            let source = if note.endnote { end.as_ref() } else { foot.as_ref() };
            let element = source.and_then(|doc| {
                doc.root_element().children().find(|c| {
                    c.is_element()
                        && matches!(lname(*c), "footnote" | "endnote")
                        && any_attr(*c, "id") == Some(note.id.as_str())
                })
            });
            if let Some(element) = element {
                reader.b.anchor(format!("fn{}", note.number));
                reader.note_marker = Some(format!("{}.", note.number));
                reader.body_children(element);
                reader.note_marker = None;
            }
        }
    }
    Ok(reader.b.finish())
}

/// Reads the Word document at `path`.
pub fn read_docx_file(path: &str) -> Result<Document, String> {
    let arch = Archive::open(path).map_err(|e| match e {
        super::book::BookError::Broken(reason) => reason,
        _ => t!("Glint can't lay this document out.").to_string(),
    })?;
    read_docx_archive(&arch)
}

impl Reader<'_> {
    /// The paragraphs, tables and so on directly inside `n` (the body, a cell, a text box, a note).
    fn body_children(&mut self, n: Node) {
        if self.depth >= MAX_DEPTH {
            return;
        }
        self.depth += 1;
        for c in n.children().filter(|c| c.is_element()) {
            if self.blocks >= MAX_BLOCKS {
                break;
            }
            match lname(c) {
                "p" => self.paragraph(c),
                "tbl" => self.table(c),
                "sdt" => {
                    if let Some(content) = child(c, "sdtContent") {
                        self.body_children(content);
                    }
                }
                "sdtContent" | "ins" | "moveTo" | "customXml" => self.body_children(c),
                "bookmarkStart" => {
                    if let Some(name) = any_attr(c, "name") {
                        self.b.anchor(name);
                    }
                }
                _ => {}
            }
        }
        self.depth -= 1;
    }

    /// The blocks of `n`'s content, kept apart from the document's own.
    fn nested(&mut self, n: Node) -> Vec<Block> {
        let saved = std::mem::take(&mut self.b);
        self.body_children(n);
        std::mem::replace(&mut self.b, saved).finish().blocks
    }

    fn push(&mut self, block: Block) {
        self.blocks += 1;
        self.b.push(block);
    }

    fn paragraph(&mut self, p: Node) {
        let ppr_node = child(p, "pPr");
        let (mut pp, mut base) = self.styles.defaults.clone();
        if let Some(id) = ppr_node.and_then(|n| child(n, "pStyle")).and_then(val) {
            let (sp, sr) = self.styles.resolve(id);
            pp.overlay(&sp);
            base.overlay(&sr);
        }
        if let Some(n) = ppr_node {
            pp.overlay(&read_ppr(n));
        }
        // As Word has it: a paragraph with no `w:bidi` runs left to right, whatever its letters.
        let dir = if pp.bidi == Some(true) { Dir::Rtl } else { Dir::Ltr };
        let mut para = Para::new(dir);
        for c in p.children().filter(|c| c.is_element()) {
            self.inline(c, &base, &None, &mut para);
        }
        para.split();
        if pp.page_break_before == Some(true) {
            self.push(Block::PageBreak);
        }
        let align = pp.jc.unwrap_or_default();
        let mut first_text = true;
        for piece in para.pieces {
            match piece {
                Piece::Text(mut text) => {
                    text.align = align;
                    let item = if first_text {
                        self.note_marker
                            .take()
                            .map(|marker| {
                                // A note's text starts after the space that follows its number.
                                text.trim();
                                (0u8, marker)
                            })
                            .or_else(|| {
                                pp.num_id.filter(|id| *id != 0).map(|id| {
                                    let ilvl = pp.ilvl.unwrap_or(0);
                                    (ilvl, self.numbering.marker(id, ilvl))
                                })
                            })
                    } else {
                        None
                    };
                    first_text = false;
                    let block = if let Some(level) = pp.heading {
                        Block::Heading { level, text }
                    } else if let Some((depth, marker)) = item {
                        Block::Item { depth, marker, text }
                    } else if pp.quote == Some(true) {
                        Block::Quote { depth: 1, text }
                    } else {
                        Block::Paragraph(text)
                    };
                    self.push(block);
                }
                Piece::Block(block) => self.push(block),
                Piece::Blocks(blocks) => {
                    for block in blocks {
                        self.push(block);
                    }
                }
            }
        }
    }

    /// One child of a paragraph (a run, a hyperlink, a bookmark, …).
    fn inline(&mut self, n: Node, base: &RProps, link: &Option<Arc<str>>, out: &mut Para) {
        match lname(n) {
            "r" => self.run(n, base, link, out),
            "hyperlink" => {
                let target: Option<Arc<str>> = any_attr(n, "id")
                    .and_then(|id| self.rels.get(id))
                    .filter(|rel| rel.external)
                    .map(|rel| rel.target.as_str())
                    .filter(|t| is_external(t))
                    .map(Into::into)
                    .or_else(|| any_attr(n, "anchor").map(|a| format!("#{a}").into()))
                    .or_else(|| link.clone());
                for c in n.children().filter(|c| c.is_element()) {
                    self.inline(c, base, &target, out);
                }
            }
            "bookmarkStart" => {
                if let Some(name) = any_attr(n, "name") {
                    self.b.anchor(name);
                }
            }
            "drawing" | "pict" | "object" => self.drawing(n, out),
            "dir" | "bdo" => {
                let mut inner = base.clone();
                inner.rtl = Some(val(n) == Some("rtl"));
                for c in n.children().filter(|c| c.is_element()) {
                    self.inline(c, &inner, link, out);
                }
            }
            "ins" | "smartTag" | "sdt" | "sdtContent" | "fldSimple" | "moveTo" | "customXml" => {
                for c in n.children().filter(|c| c.is_element()) {
                    self.inline(c, base, link, out);
                }
            }
            "AlternateContent" => {
                let chosen = child(n, "Choice").or_else(|| child(n, "Fallback"));
                if let Some(chosen) = chosen {
                    for c in chosen.children().filter(|c| c.is_element()) {
                        self.inline(c, base, link, out);
                    }
                }
            }
            "oMath" | "oMathPara" => {
                let text: String = n
                    .descendants()
                    .filter(|d| d.is_element() && lname(*d) == "t")
                    .map(flat_text)
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    let mut style = style_of(base, link);
                    style.italic = true;
                    out.text.push(&text, &style);
                }
            }
            _ => {}
        }
    }

    fn run(&mut self, r: Node, base: &RProps, link: &Option<Arc<str>>, out: &mut Para) {
        let mut props = base.clone();
        if let Some(rpr) = child(r, "rPr") {
            if let Some(id) = child(rpr, "rStyle").and_then(val) {
                props.overlay(&self.styles.resolve(id).1);
            }
            props.overlay(&read_rpr(rpr));
        }
        let style = style_of(&props, link);
        for c in r.children().filter(|c| c.is_element()) {
            match lname(c) {
                "t" => {
                    let text: String = c.children().filter(|t| t.is_text()).filter_map(|t| t.text()).collect();
                    out.text.push(&text, &style);
                }
                "tab" => out.text.push("    ", &style),
                "cr" => out.text.push("\n", &style),
                "br" => match any_attr(c, "type") {
                    Some("page") => {
                        out.split();
                        out.pieces.push(Piece::Block(Block::PageBreak));
                    }
                    Some("column") => {}
                    _ => out.text.push("\n", &style),
                },
                "noBreakHyphen" => out.text.push("\u{2011}", &style),
                "drawing" | "pict" | "object" => self.drawing(c, out),
                "AlternateContent" => {
                    if let Some(chosen) = child(c, "Choice").or_else(|| child(c, "Fallback")) {
                        for inner in chosen.children().filter(|i| i.is_element()) {
                            if matches!(lname(inner), "drawing" | "pict" | "object") {
                                self.drawing(inner, out);
                            }
                        }
                    }
                }
                "footnoteReference" | "endnoteReference" => {
                    if let Some(id) = any_attr(c, "id") {
                        let number = u32::try_from(self.notes.len()).unwrap_or(u32::MAX) + 1;
                        self.notes.push(NoteRef {
                            endnote: lname(c) == "endnoteReference",
                            id: id.to_string(),
                            number,
                        });
                        let mut mark = style.clone();
                        mark.script = Script::Super;
                        mark.link = Some(format!("#fn{number}").into());
                        out.text.push(&number.to_string(), &mark);
                    }
                }
                _ => {}
            }
        }
    }

    /// A drawing: its pictures where they are in the paragraph, and the paragraphs of its text boxes after them.
    fn drawing(&mut self, n: Node, out: &mut Para) {
        self.scan_drawing(n, None, "", out);
    }

    fn scan_drawing(&mut self, n: Node, size: Option<(u32, u32)>, alt: &str, out: &mut Para) {
        for c in n.children().filter(|c| c.is_element()) {
            match lname(c) {
                "inline" | "anchor" => {
                    let size = child(c, "extent").and_then(|e| {
                        let emu = |name: &str| any_attr(e, name).and_then(|v| v.parse::<u64>().ok());
                        // 9525 EMU to a pixel.
                        Some((
                            u32::try_from(emu("cx")? / 9525).ok()?,
                            u32::try_from(emu("cy")? / 9525).ok()?,
                        ))
                    });
                    let alt = child(c, "docPr")
                        .and_then(|d| any_attr(d, "descr").or_else(|| any_attr(d, "title")))
                        .unwrap_or("")
                        .to_string();
                    self.scan_drawing(c, size, &alt, out);
                }
                "blip" => {
                    if let Some(id) = any_attr(c, "embed") {
                        self.picture(id, size, alt, out);
                    }
                }
                "imagedata" => {
                    if let Some(id) = any_attr(c, "id") {
                        let alt = any_attr(c, "title").unwrap_or(alt).to_string();
                        self.picture(id, size, &alt, out);
                    }
                }
                "txbxContent" => {
                    let blocks = self.nested(c);
                    if !blocks.is_empty() {
                        out.split();
                        out.pieces.push(Piece::Blocks(blocks));
                    }
                }
                _ => self.scan_drawing(c, size, alt, out),
            }
        }
    }

    fn picture(&mut self, rel_id: &str, size: Option<(u32, u32)>, alt: &str, out: &mut Para) {
        let none = || Picture {
            image: None,
            size,
            alt: alt.to_string(),
        };
        let picture = match self.rels.get(rel_id) {
            Some(rel) if !rel.external => {
                let path = resolve("word", &rel.target);
                if self.arch.size(&path) as usize > self.budget {
                    none()
                } else {
                    match self.arch.read(&path, 24 * 1024 * 1024) {
                        Some(bytes) => {
                            self.budget = self.budget.saturating_sub(bytes.len());
                            // Pictures Word stores in a size of its own: the layout size wins when it has one.
                            picture_of(bytes, alt, size)
                        }
                        None => none(),
                    }
                }
            }
            _ => none(),
        };
        out.split();
        out.pieces.push(Piece::Block(Block::Picture(picture)));
    }

    fn table(&mut self, tbl: Node) {
        let rtl = child(tbl, "tblPr").and_then(|p| child(p, "bidiVisual")).is_some_and(on);
        let mut rows = Vec::new();
        for tr in tbl.children().filter(|c| c.is_element() && lname(*c) == "tr") {
            let header = child(tr, "trPr").and_then(|p| child(p, "tblHeader")).is_some_and(on);
            let mut cells = Vec::new();
            for tc in tr.children().filter(|c| c.is_element() && lname(*c) == "tc") {
                let props = child(tc, "tcPr");
                let span = props
                    .and_then(|p| child(p, "gridSpan"))
                    .and_then(val)
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 63);
                let shade = props
                    .and_then(|p| child(p, "shd"))
                    .and_then(|s| any_attr(s, "fill"))
                    .filter(|f| f.len() == 6)
                    .and_then(|f| u32::from_str_radix(f, 16).ok());
                let merged_below = props
                    .and_then(|p| child(p, "vMerge"))
                    .is_some_and(|m| val(m) != Some("restart"));
                let blocks = if merged_below { Vec::new() } else { self.nested(tc) };
                cells.push(Cell { blocks, span, shade });
            }
            if !cells.is_empty() {
                rows.push(Row { cells, header });
            }
        }
        if !rows.is_empty() {
            self.push(Block::Table(Table { rows, rtl }));
        }
    }
}

// ---- The previewer ---------------------------------------------------------------------------------------------

/// Reads a Word document from its path: [`read_docx_file`] in the app, a fake in tests.
pub type DocxReader = Arc<dyn Fn(&str) -> Result<Document, String> + Send + Sync>;

/// What the view tells its owner.
pub enum DocxEvent {
    /// The file could not be laid out; the TypeScript fell back to Windows' previewer over the icon.
    Failed,
}

enum Load {
    Waiting,
    Failed(SharedString),
    Ready,
}

/// A Word document laid out (`DocxView`).
pub struct DocxView {
    path: String,
    reader: DocxReader,
    load: Load,
    started: Instant,
    generation: u64,
    rich: Entity<RichView>,
    focus: FocusHandle,
    _read: Task<()>,
    _subscription: Subscription,
}

impl EventEmitter<DocxEvent> for DocxView {}

impl DocxView {
    /// The view of the document at `path`. Reading and laying out happen in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_reader(path, Arc::new(read_docx_file), window, cx)
    }

    /// [`DocxView::new`] reading through `reader`.
    pub fn with_reader(path: String, reader: DocxReader, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rich = cx.new(|cx| RichView::new(gpui_kit::rems(48.), t!("Word document"), cx));
        let subscription = cx.subscribe(&rich, |this, _, event: &RichEvent, cx| {
            if let RichEvent::Link(href) = event {
                this.follow(href.as_ref(), cx);
            }
        });
        let mut this = Self {
            path,
            reader,
            load: Load::Waiting,
            started: Instant::now(),
            generation: 0,
            rich,
            focus: cx.focus_handle(),
            _read: Task::ready(()),
            _subscription: subscription,
        };
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another document.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.read(window, cx);
        }
    }

    /// Lays the document out again (it changed on disk).
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read(window, cx);
    }

    /// Whether the document could not be laid out.
    pub fn failed(&self) -> bool {
        matches!(self.load, Load::Failed(_))
    }

    /// Whether the document is laid out.
    pub fn ready(&self) -> bool {
        matches!(self.load, Load::Ready)
    }

    /// The rich view the document is drawn in.
    pub fn text_view(&self) -> &Entity<RichView> {
        &self.rich
    }

    /// Scrolls a page (Page Down and Page Up in Quick Look). Returns whether it moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        self.rich.update(cx, |rich, cx| rich.page(down, cx))
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load = Load::Waiting;
        self.started = Instant::now();
        let (path, reader) = (self.path.clone(), self.reader.clone());
        self._read = cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { reader(&path) }).await;
            this.update_in(cx, |this, _, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(doc) => {
                        let rtl = doc.mostly_rtl();
                        this.rich
                            .update(cx, |rich, cx| rich.set_document(Arc::new(doc), rtl, cx));
                        this.load = Load::Ready;
                    }
                    Err(reason) => {
                        this.load = Load::Failed(reason.into());
                        cx.emit(DocxEvent::Failed);
                    }
                }
                cx.notify();
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    /// A click on a link: the web and mail open outside, a bookmark scrolls to its place (`#id`).
    fn follow(&mut self, href: &str, cx: &mut Context<Self>) {
        if let Some(id) = href.strip_prefix('#') {
            self.rich.update(cx, |rich, cx| {
                rich.scroll_to_anchor(id, cx);
            });
        } else if is_external(href) {
            cx.open_url(href);
        }
    }

    /// The arrows, Page Up and Down, Home and End scroll; Space is Page Down.
    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        if keys.modifiers.control || keys.modifiers.alt || keys.modifiers.platform {
            return;
        }
        let key = match keys.key.as_str() {
            "space" if keys.modifiers.shift => "pageup",
            "space" => "pagedown",
            other => other,
        };
        if self.rich.update(cx, |rich, cx| rich.key(key, cx)) {
            cx.stop_propagation();
        }
    }
}

impl Focusable for DocxView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DocxView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let root = col()
            .id("docx-view")
            .size_full()
            .min_h_0()
            .text_start()
            .track_focus(&self.focus)
            .tab_stop(true)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        match &self.load {
            Load::Waiting => root.child(waiting(self.started, Some(t!("Laying out the document")), cx)),
            Load::Failed(reason) => root.child(big_icon(&self.path, Some(reason.clone()), cx)),
            Load::Ready => root.child(self.rich.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::preview::rich::TEST_PNG;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::io::Write as _;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new() -> Self {
            static COUNT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "glint-docx-test-{}-{}",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).ok();
            Folder(path)
        }

        fn zip(&self, name: &str, files: &[(&str, &[u8])]) -> String {
            let path = self.0.join(name);
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).expect("a file"));
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for (entry, bytes) in files {
                zip.start_file(*entry, options).expect("an entry");
                zip.write_all(bytes).expect("its bytes");
            }
            zip.finish().expect("a zip");
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math""#;

    const STYLES: &str = r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
      <w:docDefaults><w:pPrDefault><w:pPr><w:jc w:val="left"/></w:pPr></w:pPrDefault></w:docDefaults>
      <w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
      <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:rPr><w:b/></w:rPr></w:style>
      <w:style w:type="paragraph" w:styleId="Titre2"><w:name w:val="Heading 2"/><w:basedOn w:val="Normal"/></w:style>
      <w:style w:type="paragraph" w:styleId="Arabe"><w:name w:val="عربي"/><w:basedOn w:val="Normal"/><w:pPr><w:bidi/></w:pPr></w:style>
      <w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/></w:style>
      <w:style w:type="character" w:styleId="Strong"><w:name w:val="Strong"/><w:rPr><w:b/></w:rPr></w:style>
    </w:styles>"#;

    const NUMBERING: &str = r#"<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
      <w:abstractNum w:abstractNumId="0">
        <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
        <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1.%2)"/></w:lvl>
      </w:abstractNum>
      <w:abstractNum w:abstractNumId="1">
        <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="&#xF0B7;"/></w:lvl>
      </w:abstractNum>
      <w:abstractNum w:abstractNumId="2">
        <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="arabicAbjad"/><w:lvlText w:val="%1-"/></w:lvl>
      </w:abstractNum>
      <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
      <w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
      <w:num w:numId="3"><w:abstractNumId w:val="2"/></w:num>
      <w:num w:numId="4"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride></w:num>
    </w:numbering>"#;

    const RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
      <Relationship Id="rId1" Type="x/image" Target="media/image1.png"/>
      <Relationship Id="rId2" Type="x/hyperlink" Target="https://example.com/page" TargetMode="External"/>
      <Relationship Id="rId3" Type="x/hyperlink" Target="javascript:alert(1)" TargetMode="External"/>
      <Relationship Id="rId4" Type="x/image" Target="media/image2.emf"/>
    </Relationships>"#;

    fn paragraph_of(doc: &Document, plain: &str) -> Text {
        doc.blocks
            .iter()
            .find_map(|b| match b {
                Block::Paragraph(t)
                | Block::Heading { text: t, .. }
                | Block::Quote { text: t, .. }
                | Block::Item { text: t, .. }
                    if t.plain() == plain =>
                {
                    Some(t.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no paragraph {plain:?} in {:?}", doc.plain_text()))
    }

    /// An Arabic document with everything the reader maps.
    fn sample_document() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {NS}><w:body>
  <w:p><w:pPr><w:pStyle w:val="Heading1"/><w:bidi/></w:pPr><w:bookmarkStart w:id="0" w:name="intro"/><w:r><w:t>مقدمة</w:t></w:r></w:p>
  <w:p><w:pPr><w:pStyle w:val="Titre2"/></w:pPr><w:r><w:t>Second heading</w:t></w:r></w:p>
  <w:p><w:pPr><w:pStyle w:val="Arabe"/><w:jc w:val="right"/></w:pPr>
    <w:r><w:t xml:space="preserve">بسم الله </w:t></w:r>
    <w:r><w:rPr><w:b/><w:i/><w:u w:val="single"/><w:color w:val="C00000"/></w:rPr><w:t>الرحمن</w:t></w:r>
    <w:r><w:rPr><w:rStyle w:val="Strong"/></w:rPr><w:t xml:space="preserve"> الرحيم </w:t></w:r>
    <w:r><w:rPr><w:rtl/></w:rPr><w:t>مرحبا</w:t></w:r>
    <w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t>2</w:t></w:r>
    <w:r><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas"/></w:rPr><w:t>code</w:t></w:r>
  </w:p>
  <w:p><w:r><w:t>English paragraph with </w:t></w:r><w:hyperlink r:id="rId2"><w:r><w:t>a link</w:t></w:r></w:hyperlink>
     <w:r><w:t xml:space="preserve"> and </w:t></w:r><w:hyperlink w:anchor="intro"><w:r><w:t>an anchor</w:t></w:r></w:hyperlink>
     <w:r><w:t xml:space="preserve"> and </w:t></w:r><w:hyperlink r:id="rId3"><w:r><w:t>unsafe</w:t></w:r></w:hyperlink><w:r><w:tab/><w:t>after tab</w:t><w:br/><w:t>second line</w:t></w:r></w:p>
  <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>First</w:t></w:r></w:p>
  <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Nested</w:t></w:r></w:p>
  <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Second</w:t></w:r></w:p>
  <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>Bullet</w:t></w:r></w:p>
  <w:p><w:pPr><w:bidi/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="3"/></w:numPr></w:pPr><w:r><w:t>بند عربي</w:t></w:r></w:p>
  <w:p><w:pPr><w:bidi/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="3"/></w:numPr></w:pPr><w:r><w:t>بند ثان</w:t></w:r></w:p>
  <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="4"/></w:numPr></w:pPr><w:r><w:t>Starts at five</w:t></w:r></w:p>
  <w:p><w:pPr><w:pStyle w:val="Quote"/></w:pPr><w:r><w:t>A quotation</w:t></w:r></w:p>
  <w:p><w:r><w:t>Before picture </w:t></w:r><w:r><w:drawing><wp:inline><wp:extent cx="1905000" cy="952500"/><wp:docPr id="1" name="p" descr="رسم توضيحي"/><a:graphic><a:graphicData><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:blipFill><a:blip r:embed="rId1"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r><w:r><w:t xml:space="preserve"> after picture</w:t></w:r></w:p>
  <w:p><w:r><w:drawing><wp:inline><wp:extent cx="95250" cy="95250"/><wp:docPr id="2" name="e" descr="metafile"/><a:graphic><a:graphicData><a:blip r:embed="rId4"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
  <w:p><w:r><w:br w:type="page"/></w:r><w:r><w:t>On a new page</w:t></w:r></w:p>
  <w:tbl><w:tblPr><w:bidiVisual/></w:tblPr>
    <w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val="2"/><w:shd w:fill="DDDDDD"/></w:tcPr><w:p><w:r><w:t>عنوان الجدول</w:t></w:r></w:p></w:tc></w:tr>
    <w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:pPr><w:bidi/></w:pPr><w:r><w:t>أ</w:t></w:r></w:p><w:p><w:r><w:t>second in cell</w:t></w:r></w:p></w:tc><w:tc><w:tbl><w:tr><w:tc><w:p><w:r><w:t>nested</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:tc></w:tr>
    <w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p><w:r><w:t>ب</w:t></w:r></w:p></w:tc></w:tr>
  </w:tbl>
  <w:p><w:r><w:t>Box: </w:t></w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor><wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>Inside a text box</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp></wp:anchor></w:drawing></mc:Choice><mc:Fallback><w:pict><w:txbxContent><w:p><w:r><w:t>Fallback box</w:t></w:r></w:p></w:txbxContent></w:pict></mc:Fallback></mc:AlternateContent></w:p>
  <w:p><w:r><w:t>Note here</w:t></w:r><w:r><w:footnoteReference w:id="2"/></w:r><w:ins><w:r><w:t> inserted</w:t></w:r></w:ins><w:del><w:r><w:delText> deleted</w:delText></w:r></w:del></w:p>
  <w:sdt><w:sdtContent><w:p><w:r><w:t>In a content control</w:t></w:r></w:p></w:sdtContent></w:sdt>
  <w:p><m:oMath><m:r><m:t>x=1</m:t></m:r></m:oMath></w:p>
  <w:sectPr/>
</w:body></w:document>"#
        )
    }

    fn footnotes() -> String {
        format!(
            r#"<w:footnotes {NS}>
  <w:footnote w:type="separator" w:id="0"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>
  <w:footnote w:id="2"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> The note text</w:t></w:r></w:p></w:footnote>
</w:footnotes>"#
        )
    }

    fn write_sample(folder: &Folder) -> String {
        let document = sample_document();
        let notes = footnotes();
        folder.zip(
            "sample.docx",
            &[
                ("[Content_Types].xml", b"<Types/>"),
                ("word/document.xml", document.as_bytes()),
                ("word/styles.xml", STYLES.as_bytes()),
                ("word/numbering.xml", NUMBERING.as_bytes()),
                ("word/_rels/document.xml.rels", RELS.as_bytes()),
                ("word/footnotes.xml", notes.as_bytes()),
                ("word/media/image1.png", TEST_PNG),
                ("word/media/image2.emf", &[1, 0, 0, 0, 0, 0]),
            ],
        )
    }

    #[test]
    fn number_formats_count_the_way_word_does() {
        assert_eq!(number_text(3, "decimal"), "3");
        assert_eq!(number_text(3, "decimalZero"), "03");
        assert_eq!(number_text(27, "lowerLetter"), "aa");
        assert_eq!(number_text(2, "upperLetter"), "B");
        assert_eq!(number_text(14, "lowerRoman"), "xiv");
        assert_eq!(number_text(9, "upperRoman"), "IX");
        assert_eq!(number_text(2, "arabicAbjad"), "ب");
        assert_eq!(number_text(3, "arabicAlpha"), "ت");
        assert_eq!(number_text(29, "arabicAbjad"), "أأ");
        assert_eq!(number_text(1, "hebrew1"), "א");
        assert_eq!(number_text(30, "hebrew1"), "30");
        assert_eq!(number_text(4, "none"), "");
        assert_eq!(bullet_char("\u{f0b7}"), "•");
        assert_eq!(bullet_char("o"), "◦");
        assert_eq!(bullet_char("-"), "-");
        assert_eq!(bullet_char(""), "•");
        assert!(is_mono("Cascadia Mono") && is_mono("Courier New") && !is_mono("Calibri"));
    }

    #[test]
    fn styles_resolve_through_what_they_are_based_on() {
        let styles = Styles::read(STYLES);
        assert_eq!(styles.resolve("Heading1").0.heading, Some(1));
        assert_eq!(styles.resolve("Heading1").1.bold, Some(true));
        assert_eq!(
            styles.resolve("Titre2").0.heading,
            Some(2),
            "known by its name, not its id"
        );
        assert_eq!(styles.resolve("Arabe").0.bidi, Some(true));
        assert_eq!(styles.resolve("Quote").0.quote, Some(true));
        assert_eq!(
            styles.resolve("Normal").0.jc,
            Some(Align::Start),
            "from the document's defaults"
        );
        assert_eq!(styles.resolve("NoSuchStyle").0.heading, None);
        // A style based on itself does not loop.
        let looped = Styles::read(
            r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:styleId="A"><w:name w:val="A"/><w:basedOn w:val="B"/></w:style><w:style w:styleId="B"><w:name w:val="B"/><w:basedOn w:val="A"/></w:style></w:styles>"#,
        );
        assert_eq!(looped.resolve("A").0, PProps::default());
    }

    #[test]
    fn a_document_maps_to_blocks_with_direction_styles_lists_and_links() {
        let folder = Folder::new();
        let path = write_sample(&folder);
        let doc = read_docx_file(&path).expect("a document");
        // Headings, by style name and base; a bidi paragraph is right to left, one without is left to right.
        let Block::Heading { level, text } = &doc.blocks[0] else {
            panic!("a heading")
        };
        assert_eq!((*level, text.plain().as_str(), text.dir), (1, "مقدمة", Dir::Rtl));
        assert_eq!(doc.anchors["intro"], 0);
        assert!(matches!(&doc.blocks[1], Block::Heading { level: 2, .. }));
        // The Arabic paragraph: runs, in styles.
        let arabic = paragraph_of(&doc, "بسم الله الرحمن الرحيم مرحبا2code");
        assert_eq!(arabic.dir, Dir::Rtl);
        assert_eq!(arabic.align, Align::End, "jc right in a bidi paragraph is its end edge");
        let styled = |text: &str| arabic.spans.iter().find(|s| s.text == text).map(|s| s.style.clone());
        let strong = styled("الرحمن").expect("a run");
        assert!(strong.bold && strong.italic && strong.underline && strong.color == Some(0xc00000));
        assert!(styled(" الرحيم ").expect("a run").bold, "a character style");
        assert_eq!(styled("مرحبا").expect("a run").dir, Dir::Rtl, "w:rtl on a run");
        assert_eq!(styled("2").expect("a run").script, Script::Super);
        assert!(styled("code").expect("a run").code);
        // English: links by relationship and by bookmark; a script link is not a link.
        let english = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Paragraph(t) if t.plain().starts_with("English paragraph") => Some(t.clone()),
                _ => None,
            })
            .expect("the english paragraph");
        assert_eq!(english.dir, Dir::Ltr);
        let link = |text: &str| {
            english
                .spans
                .iter()
                .find(|s| s.text == text)
                .and_then(|s| s.style.link.clone())
        };
        assert_eq!(link("a link").as_deref(), Some("https://example.com/page"));
        assert_eq!(link("an anchor").as_deref(), Some("#intro"));
        assert_eq!(link("unsafe"), None);
        assert!(english.plain().ends_with("    after tab\nsecond line"));
        // Lists: counters per list and level, bullets from Symbol, Arabic letters, a start override.
        let markers: Vec<(u8, String, String)> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Item { depth, marker, text } => Some((*depth, marker.clone(), text.plain())),
                _ => None,
            })
            .collect();
        assert_eq!(
            markers,
            [
                (0, "1.".to_string(), "First".to_string()),
                (1, "1.a)".to_string(), "Nested".to_string()),
                (0, "2.".to_string(), "Second".to_string()),
                (0, "•".to_string(), "Bullet".to_string()),
                (0, "أ-".to_string(), "بند عربي".to_string()),
                (0, "ب-".to_string(), "بند ثان".to_string()),
                (0, "5.".to_string(), "Starts at five".to_string()),
                (0, "1.".to_string(), "The note text".to_string()),
            ]
        );
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Quote { .. })));
        // The picture splits its paragraph, keeps its alt text and its size; the metafile keeps only its alt text.
        let at = doc
            .blocks
            .iter()
            .position(|b| matches!(b, Block::Picture(p) if p.alt == "رسم توضيحي"))
            .expect("the picture");
        assert!(matches!(&doc.blocks[at - 1], Block::Paragraph(t) if t.plain() == "Before picture "));
        assert!(matches!(&doc.blocks[at + 1], Block::Paragraph(t) if t.plain() == " after picture"));
        let Block::Picture(picture) = &doc.blocks[at] else {
            panic!("a picture")
        };
        assert!(picture.image.is_some());
        assert_eq!(picture.size, Some((200, 100)));
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Picture(p) if p.alt == "metafile" && p.image.is_none())));
        // A page break, and the text after it.
        let page = doc
            .blocks
            .iter()
            .position(|b| matches!(b, Block::PageBreak))
            .expect("a page break");
        assert!(matches!(&doc.blocks[page + 1], Block::Paragraph(t) if t.plain() == "On a new page"));
        // The table: right to left, a header spanning two columns and shaded, a nested table, a merged cell.
        let table = doc
            .blocks
            .iter()
            .find_map(|b| if let Block::Table(t) = b { Some(t) } else { None })
            .expect("a table");
        assert!(table.rtl);
        assert_eq!(table.rows.len(), 3);
        assert!(
            table.rows[0].header && table.rows[0].cells[0].span == 2 && table.rows[0].cells[0].shade == Some(0xdddddd)
        );
        assert_eq!(table.rows[1].cells[0].blocks.len(), 2);
        assert!(table.rows[1].cells[1]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Table(_))));
        assert!(table.rows[2].cells[0].blocks.is_empty(), "the cell merged from above");
        // The text box's paragraphs follow the paragraph, once (the fallback is not read too).
        let texts = doc.plain_text();
        assert!(texts.contains("Box: \nInside a text box\n"), "{texts}");
        assert!(!texts.contains("Fallback box"));
        // Inserted text is shown, deleted text is not; content controls and equations are read.
        assert!(texts.contains("Note here1 inserted\n"), "{texts}");
        assert!(!texts.contains("deleted"));
        assert!(texts.contains("In a content control\n") && texts.contains("x=1\n"));
        // The note is at the end, after a rule, and the reference links to it.
        let reference = paragraph_of(&doc, "Note here1 inserted");
        let mark = reference
            .spans
            .iter()
            .find(|s| s.style.link.is_some())
            .expect("the reference");
        assert_eq!(mark.style.link.as_deref(), Some("#fn1"));
        assert_eq!(mark.style.script, Script::Super);
        assert!(doc.anchors["fn1"] > doc.anchors["intro"]);
        assert!(matches!(&doc.blocks[doc.anchors["fn1"] - 1], Block::Rule));
    }

    #[test]
    fn what_is_not_a_document_says_so() {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let folder = Folder::new();
        let empty = folder.zip("empty.docx", &[("other.txt", b"x")]);
        assert_eq!(
            read_docx_file(&empty).err().as_deref(),
            Some("Glint can't lay this document out.")
        );
        let broken = folder.zip("broken.docx", &[("word/document.xml", b"<w:document")]);
        assert!(read_docx_file(&broken).is_err());
        let bodyless = folder.zip("bodyless.docx", &[("word/document.xml", NS.as_bytes())]);
        assert!(read_docx_file(&bodyless).is_err());
        assert!(read_docx_file("C:\\no\\such\\file.docx").is_err());
        // Tables inside tables, far deeper than any document: refused, not a crash.
        let mut deep = format!("<w:document {NS}><w:body>");
        for _ in 0..200 {
            deep.push_str("<w:tbl><w:tr><w:tc>");
        }
        deep.push_str("<w:p><w:r><w:t>deep</w:t></w:r></w:p>");
        for _ in 0..200 {
            deep.push_str("</w:tc></w:tr></w:tbl>");
        }
        deep.push_str("</w:body></w:document>");
        let path = folder.zip("deep.docx", &[("word/document.xml", deep.as_bytes())]);
        assert!(read_docx_file(&path).is_err());
        // Deep enough to hit the reader's own limit, not the parser's: read, the deepest tables left out.
        let mut nest = format!("<w:document {NS}><w:body>");
        for _ in 0..30 {
            nest.push_str("<w:tbl><w:tr><w:tc><w:p><w:r><w:t>level</w:t></w:r></w:p>");
        }
        for _ in 0..30 {
            nest.push_str("</w:tc></w:tr></w:tbl>");
        }
        nest.push_str("</w:body></w:document>");
        let path = folder.zip("nested.docx", &[("word/document.xml", nest.as_bytes())]);
        let doc = read_docx_file(&path).expect("a nested document");
        assert!(doc.plain_text().matches("level").count() <= MAX_DEPTH as usize);
    }

    #[test]
    fn a_numbered_list_counts_each_list_apart() {
        let mut numbering = Numbering::read(NUMBERING);
        assert_eq!(numbering.marker(1, 0), "1.");
        assert_eq!(numbering.marker(1, 0), "2.");
        assert_eq!(numbering.marker(1, 1), "2.a)");
        assert_eq!(numbering.marker(1, 1), "2.b)");
        assert_eq!(numbering.marker(1, 0), "3.");
        assert_eq!(
            numbering.marker(1, 1),
            "3.a)",
            "a deeper level starts again under a new item"
        );
        assert_eq!(numbering.marker(4, 0), "5.");
        assert_eq!(numbering.marker(99, 0), "•", "a list nobody defined is bulleted");
    }

    // ---- The view -------------------------------------------------------------------------------------------

    fn open(cx: &mut TestAppContext, path: String) -> (Entity<DocxView>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |window, cx| cx.new(|cx| DocxView::new(path, window, cx)));
        (view, visual)
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..600 {
            cx.run_until_parked();
            cx.update(|window, cx| window.render_frame(cx));
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the document never arrived");
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
    fn an_arabic_document_is_drawn_with_its_headings_lists_and_tables(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let folder = Folder::new();
        let (view, mut visual) = open(cx, write_sample(&folder));
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.ready()));
        frames(&mut visual);
        for label in [
            "Word document",
            "مقدمة",
            "Second heading",
            "First",
            "Nested",
            "بند عربي",
            "A quotation",
        ] {
            assert!(has(&mut visual, label), "{label}");
        }
        visual.update(|window, _| {
            assert_eq!(window.find_by_label("مقدمة").role(), Some(Role::Heading));
            assert_eq!(window.find_by_label("بند عربي").role(), Some(Role::ListItem));
            assert_eq!(window.find_by_label("A quotation").role(), Some(Role::Blockquote));
        });
        // The picture and the table are further down: scroll to them.
        for (label, role, is_it) in [
            (
                "رسم توضيحي",
                Role::Image,
                (|b: &Block| matches!(b, Block::Picture(p) if p.alt == "رسم توضيحي")) as fn(&Block) -> bool,
            ),
            (
                "Table",
                Role::Table,
                (|b: &Block| matches!(b, Block::Table(_))) as fn(&Block) -> bool,
            ),
        ] {
            let ix = view
                .read_with(&visual, |v, cx| {
                    v.text_view().read(cx).document().blocks.iter().position(is_it)
                })
                .expect("the block");
            let rich = view.read_with(&visual, |v, _| v.text_view().clone());
            rich.update(&mut visual, |rich, cx| rich.scroll_to_block(ix, cx));
            frames(&mut visual);
            assert!(has(&mut visual, label), "{label}");
            visual.update(|window, _| assert_eq!(window.find_by_label(&said(label)).role(), Some(role)));
        }
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn the_keys_scroll_and_a_bookmark_link_goes_to_its_place(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let folder = Folder::new();
        let mut body = format!("<w:document {NS}><w:body>");
        body.push_str(r#"<w:p><w:r><w:t>Top</w:t></w:r></w:p>"#);
        for i in 0..120 {
            body.push_str(&format!("<w:p><w:r><w:t>line {i}</w:t></w:r></w:p>"));
        }
        body.push_str(r#"<w:p><w:bookmarkStart w:id="1" w:name="far"/><w:r><w:t>The far place</w:t></w:r></w:p></w:body></w:document>"#);
        let path = folder.zip("long.docx", &[("word/document.xml", body.as_bytes())]);
        let (view, mut visual) = open(cx, path);
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.ready()));
        frames(&mut visual);
        assert!(has(&mut visual, "Top"));
        let press = |visual: &mut VisualTestContext, key: &'static str| {
            let event = KeyDownEvent {
                keystroke: gpui_kit::Keystroke::parse(key).expect("a key"),
                is_held: false,
                prefer_character_input: false,
            };
            visual.update(|window, cx| view.update(cx, |v, cx| v.on_key_down(&event, window, cx)));
            frames(visual);
        };
        press(&mut visual, "pagedown");
        assert!(view.read_with(&visual, |v, cx| v.text_view().read(cx).top_block() > 5));
        press(&mut visual, "pageup");
        assert_eq!(view.read_with(&visual, |v, cx| v.text_view().read(cx).top_block()), 0);
        press(&mut visual, "end");
        assert!(has(&mut visual, "The far place"));
        press(&mut visual, "home");
        assert!(has(&mut visual, "Top"));
        press(&mut visual, "space");
        assert!(view.read_with(&visual, |v, cx| v.text_view().read(cx).top_block() > 5));
        // A link to the bookmark scrolls there; the web's links and links to nowhere leave it be.
        press(&mut visual, "home");
        view.update(&mut visual, |v, cx| v.follow("#far", cx));
        frames(&mut visual);
        assert!(has(&mut visual, "The far place"));
        view.update(&mut visual, |v, cx| v.follow("#nowhere", cx));
        frames(&mut visual);
        assert!(has(&mut visual, "The far place"));
        assert!(view.update(&mut visual, |v, cx| v.page(false, cx)));
    }

    #[gpui_kit::test]
    fn a_document_that_cannot_be_laid_out_says_so_and_tells_the_owner(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let folder = Folder::new();
        let path = folder.zip("bad.docx", &[("x.txt", b"x")]);
        cx.update(testing::init);
        let heard = std::rc::Rc::new(std::cell::Cell::new(false));
        let flag = heard.clone();
        let (mut visual, view) = testing::open_window(cx, {
            let path = path.clone();
            move |window, cx| {
                let view = cx.new(|cx| DocxView::new(path, window, cx));
                cx.subscribe(&view, move |_, _: &DocxEvent, _| flag.set(true)).detach();
                view
            }
        });
        settle(&mut visual, |cx| view.read_with(cx, |v, _| v.failed()));
        assert!(has(&mut visual, "Glint can't lay this document out."));
        assert!(heard.get(), "the owner was told");
        testing::set_rtl(false);
    }
}
