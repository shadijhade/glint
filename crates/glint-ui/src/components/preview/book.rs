//! Books: EPUB, FB2 and comics, read page by page (`BookView` in BookView.tsx).
//!
//! The TypeScript opened every kind of book in foliate-js. Here each format has a reader that turns a chapter
//! into the [`rich`](super::rich) model, which [`BookView`] draws. Nothing is a new crate: the container is `zip`,
//! the XML is `roxmltree`.
//!
//! | TypeScript                                  | Here                                                          |
//! |---------------------------------------------|---------------------------------------------------------------|
//! | `BOOK_EXTS`                                 | [`BOOK_EXTS`]; [`can_read`] says which of them are read here  |
//! | foliate's `epub.js` (container, OPF, nav)   | [`open_epub`]: `META-INF/container.xml`, the OPF's manifest and spine, the nav document or the NCX |
//! | foliate's `fb2.js`                          | [`open_fb2`] (and `.fbz`, a zipped FB2)                       |
//! | foliate's `comic-book.js`                   | [`open_cbz`]: the images in natural order, `ComicInfo.xml`'s manga flag |
//! | the chapter's XHTML in an iframe            | [`xhtml_document`]: the chapter mapped to blocks, with a small CSS reading ([`Css`]) for `direction`, `text-align`, bold and italic |
//! | `book.dir` and `page-progression-direction` | [`Book::rtl`]: a right-to-left book turns its pages the other way (the arrow keys and the buttons swap) |
//! | `toc`, the contents button                  | [`Book::toc`], the panel at the start edge                    |
//! | `places()`, `remember()` (last 200 books)   | [`remembered`], [`remember`] (kept between runs, see `book_places`) |
//! | `startingSize()`, `css(size)`               | [`text_size`], [`set_text_size`] (70% to 200%, steps of 10)   |
//! | the keys                                    | [`BookView::on_key_down`]                                     |
//! | `showTextStart`                             | [`Book::start`]: the "bodymatter" landmark, else the first section |
//! | the fraction                                | [`BookView::fraction`]: weighted by the chapters' sizes       |
//!
//! # Where the previewer is placed (for the coordinator)
//!
//! - **Which `choose.rs` arm.** The arm `Choice::later(Body::Shell(Fallback::Icon), "book")` for
//!   `readable && !archived && one_of(entry, &BOOKS)` becomes a `Body::Book` (new variant) when
//!   [`can_read`]`(&entry.ext)` is true; `mobi`, `azw3` and `azw` keep the Windows previewer over the icon (see
//!   Not ported).
//! - **Make it.** `cx.new(|cx| BookView::new(path, window, cx))` in `frame.rs`'s `build`.
//! - **When the selection moves.** A new `BookView` per file (its `key` changes with the path), or
//!   [`BookView::set_path`] on the same view.
//! - **Quick Look's Page Down / Page Up.** `page(down, cx)`: scrolls a page, and at the end of a chapter goes to the
//!   next one. Add `BookView` to `page_of!` in `frame.rs`. The book's own keys (arrows, Page keys, Space) work when it
//!   has focus; inside Quick Look the Left and Right bindings of context "QuickLook" (previous and next file)
//!   win over them, so there the book turns with Page Up, Page Down and Space only.
//!
//! # Not ported
//!
//! - **MOBI, AZW, AZW3.** foliate read them with its own `mobi.js` (PalmDOC/Huffman text and KF8's skeleton and
//!   fragment tables, about 1,500 lines of JavaScript). That is not done here: [`can_read`] says no and the files
//!   keep the Windows previewer. PORT ME(mobi): a reader for PalmDOC (LZ77), MOBI headers and KF8's FDST/skeleton.
//! - **Read aloud** (`speechSynthesis`): Windows' speech synthesis needs `windows` features glint-ui doesn't have
//!   (`Media_SpeechSynthesis`, `Media_Playback`). PORT ME(speech): the button, reading the chapter's text.
//! - **Places and the text size** are kept between runs by [`crate::book_places`] (`books.json` beside
//!   `state.json`), the last 200 books.
//! - **Pages.** The chapter scrolls (the design note's first milestone); foliate's page-by-page columns are not
//!   made. Page Down moves a page and runs on into the next chapter.
//! - **The book's CSS** is read only for `direction`, `text-align`, bold, italic, underline, strike and
//!   `display:none` on a tag, a class or both. Fonts, sizes, margins, drop caps and floats are not drawn.
//! - **Ruby, MathML, footnote popups, fixed-layout and media-overlay books.** Ruby text is dropped, MathML shows
//!   its text, a note link is a link to the note's place.
//! - **Comics are not zoomed**: a page is fitted to the pane.

use super::markdown::base64_decode;
use super::rich::{
    self, picture_of, rtl_language, Align, Block, Builder, Cell, Dir, Document, Picture, RichEvent, RichView, Row,
    Script, Style, Table, Text,
};
use super::text::{big_icon, redraw_after_delay, waiting};
use crate::kit::prelude::*;
use crate::{actions, t, theme};
use gpui_kit::{
    img, uniform_list, Image, ImageSource, KeyDownEvent, ObjectFit, ScrollStrategy, StyledImage as _, Task,
    UniformListScrollHandle,
};
use roxmltree::{Document as Xml, Node, ParsingOptions};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The e-books the reader opens (`BOOK_EXTS`).
pub const BOOK_EXTS: [&str; 7] = ["epub", "mobi", "azw3", "azw", "fb2", "fbz", "cbz"];

/// Whether this view reads books of this extension (`BOOK_EXTS` less the MOBI family).
pub fn can_read(ext: &str) -> bool {
    matches!(ext, "epub" | "fb2" | "fbz" | "cbz")
}

/// The most of one file read out of a book (a chapter's XHTML, a stylesheet).
const TEXT_LIMIT: u64 = 24 * 1024 * 1024;
/// The most of one picture read out of a book.
const IMAGE_LIMIT: u64 = 24 * 1024 * 1024;
/// The most picture bytes one chapter keeps; the rest show their alt text.
const CHAPTER_PICTURES: usize = 96 * 1024 * 1024;
/// The biggest FB2 read whole.
const FB2_LIMIT: u64 = 256 * 1024 * 1024;

const OPS_NS: &str = "http://www.idpf.org/2007/ops";

// ---- Why a book does not open ----------------------------------------------------------------------------------

/// Why a book could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BookError {
    /// Not a kind of book this reader knows (`/not supported|PDF/`).
    Unsupported,
    /// Encrypted by a store (`/encrypt|DRM/i`).
    Drm,
    /// Anything else, with the reason as it was.
    Broken(String),
}

impl BookError {
    /// The words the TypeScript showed.
    pub fn message(&self) -> SharedString {
        match self {
            BookError::Unsupported => t!("Glint can't read this kind of book."),
            BookError::Drm => t!("This book has DRM, which only its store's app can open."),
            BookError::Broken(reason) => reason.clone().into(),
        }
    }
}

// ---- Paths in a book -------------------------------------------------------------------------------------------

/// `%20` and its kin decoded (hrefs in a book are percent-encoded; names in its zip are not).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A file's folder inside the zip: everything before the last `/`.
fn dir_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// `href` (without its fragment) resolved against the folder `dir`, as a path inside the zip.
pub(super) fn resolve(dir: &str, href: &str) -> String {
    let href = percent_decode(&href.replace('\\', "/"));
    let mut parts: Vec<String> = if href.starts_with('/') {
        Vec::new()
    } else {
        dir.split('/').filter(|p| !p.is_empty()).map(str::to_string).collect()
    };
    for segment in href.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other.to_string()),
        }
    }
    parts.join("/")
}

/// `path#fragment` as the path and the fragment (percent-decoded).
fn split_href(href: &str) -> (&str, Option<String>) {
    match href.split_once('#') {
        Some((path, fragment)) => (path, Some(percent_decode(fragment))),
        None => (href, None),
    }
}

/// Whether `href` starts with a scheme (`https:`, `javascript:`).
fn has_scheme(href: &str) -> bool {
    let mut chars = href.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && href
            .split(':')
            .next()
            .is_some_and(|s| s.len() < href.len() && s.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
}

/// Whether a link goes out to the web or the mail program.
pub(super) fn is_external(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("mailto:")
}

/// Compares file names the way people count: `page2` before `page10`, ignoring case.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits.trim_start_matches('0').to_string()
                };
                let (x, y) = (number(&mut a), number(&mut b));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

// ---- A zip -----------------------------------------------------------------------------------------------------

/// A zip (a book, a Word document), opened once and read an entry at a time.
pub(super) struct Archive {
    zip: Mutex<zip::ZipArchive<std::fs::File>>,
    /// Lowercase normalized name to entry.
    index: HashMap<String, usize>,
    /// Normalized names, by entry.
    pub(super) names: Vec<String>,
    /// Uncompressed sizes, by entry.
    sizes: Vec<u64>,
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/").trim_start_matches('/').to_string()
}

impl Archive {
    pub(super) fn open(path: &str) -> Result<Self, BookError> {
        let file = std::fs::File::open(path).map_err(|e| BookError::Broken(e.to_string()))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|_| BookError::Unsupported)?;
        let mut index = HashMap::new();
        let mut names = Vec::with_capacity(zip.len());
        let mut sizes = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            match zip.by_index_raw(i) {
                Ok(entry) => {
                    let name = normalize(&entry.name().map(|n| n.into_owned()).unwrap_or_default());
                    if !entry.is_dir() {
                        index.entry(name.to_lowercase()).or_insert(i);
                    }
                    names.push(name);
                    sizes.push(entry.size());
                }
                Err(_) => {
                    names.push(String::new());
                    sizes.push(0);
                }
            }
        }
        Ok(Self {
            zip: Mutex::new(zip),
            index,
            names,
            sizes,
        })
    }

    pub(super) fn has(&self, name: &str) -> bool {
        self.index.contains_key(&normalize(name).to_lowercase())
    }

    pub(super) fn size(&self, name: &str) -> u64 {
        self.index
            .get(&normalize(name).to_lowercase())
            .and_then(|i| self.sizes.get(*i))
            .copied()
            .unwrap_or(0)
    }

    /// The entry's bytes, unless it is missing, can't be unpacked, or is bigger than `limit`.
    pub(super) fn read(&self, name: &str, limit: u64) -> Option<Vec<u8>> {
        let i = *self.index.get(&normalize(name).to_lowercase())?;
        if self.sizes.get(i).copied()? > limit {
            return None;
        }
        let mut zip = self.zip.lock().ok()?;
        let entry = zip.by_index(i).ok()?;
        let mut out = Vec::with_capacity(usize::try_from(self.sizes[i]).unwrap_or(0).min(1 << 24));
        // The declared size can lie; the read is bounded all the same.
        entry.take(limit + 1).read_to_end(&mut out).ok()?;
        (out.len() as u64 <= limit).then_some(out)
    }

    pub(super) fn read_text(&self, name: &str) -> Option<String> {
        self.read(name, TEXT_LIMIT).map(|bytes| xml_text(&bytes))
    }
}

/// An XML or HTML file's text: UTF-8 as is, else by its byte order mark or its declared encoding.
fn xml_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.trim_start_matches('\u{feff}').to_string();
    }
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        return glint_core::textenc::decode(bytes, false).text;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_ascii_lowercase();
    if let Some(at) = head.find("encoding=") {
        let rest = &head[at + 9..];
        if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            if let Some(end) = rest[1..].find(quote) {
                if let Some(text) = glint_core::textenc::decode_as(bytes, &rest[1..1 + end]) {
                    return text;
                }
            }
        }
    }
    glint_core::textenc::decode(bytes, false).text
}

// ---- XML -------------------------------------------------------------------------------------------------------

/// The 96 Latin-1 entities, from `&nbsp;` (160).
const LATIN1: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf", "laquo", "not", "shy",
    "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para", "middot", "cedil", "sup1", "ordm",
    "raquo", "frac14", "frac12", "frac34", "iquest", "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig",
    "Ccedil", "Egrave", "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve",
    "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute", "THORN",
    "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil", "egrave", "eacute", "ecirc",
    "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde", "ograve", "oacute", "ocirc", "otilde", "ouml",
    "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml", "yacute", "thorn", "yuml",
];

/// The other entities a book is likely to use.
const MORE_ENTITIES: [(&str, u32); 52] = [
    ("ensp", 8194),
    ("emsp", 8195),
    ("thinsp", 8201),
    ("zwnj", 8204),
    ("zwj", 8205),
    ("lrm", 8206),
    ("rlm", 8207),
    ("ndash", 8211),
    ("mdash", 8212),
    ("lsquo", 8216),
    ("rsquo", 8217),
    ("sbquo", 8218),
    ("ldquo", 8220),
    ("rdquo", 8221),
    ("bdquo", 8222),
    ("dagger", 8224),
    ("Dagger", 8225),
    ("bull", 8226),
    ("hellip", 8230),
    ("permil", 8240),
    ("prime", 8242),
    ("Prime", 8243),
    ("lsaquo", 8249),
    ("rsaquo", 8250),
    ("euro", 8364),
    ("trade", 8482),
    ("larr", 8592),
    ("uarr", 8593),
    ("rarr", 8594),
    ("darr", 8595),
    ("harr", 8596),
    ("minus", 8722),
    ("infin", 8734),
    ("ne", 8800),
    ("le", 8804),
    ("ge", 8805),
    ("OElig", 338),
    ("oelig", 339),
    ("Scaron", 352),
    ("scaron", 353),
    ("Yuml", 376),
    ("fnof", 402),
    ("circ", 710),
    ("tilde", 732),
    ("alpha", 945),
    ("beta", 946),
    ("gamma", 947),
    ("delta", 948),
    ("pi", 960),
    ("sigma", 963),
    ("omega", 969),
    ("hearts", 9829),
];

/// The character an HTML entity name stands for (`nbsp`), when the name is one the book is likely to use.
fn entity_char(name: &str) -> Option<char> {
    if let Some(at) = LATIN1.iter().position(|n| *n == name) {
        return char::from_u32(160 + at as u32);
    }
    MORE_ENTITIES
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, code)| char::from_u32(*code))
}

/// XHTML that uses HTML's named entities (`&nbsp;`) without declaring them is not XML, so they are written as
/// numbers first, and a bare `&` that starts nothing becomes `&amp;`.
fn fix_entities(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('&') {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let name_end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        let name = &rest[1..name_end];
        let terminated = rest[name_end..].starts_with(';');
        if terminated && !name.is_empty() {
            if name.starts_with('#') || matches!(name, "amp" | "lt" | "gt" | "quot" | "apos") {
                out.push_str(&rest[..name_end + 1]);
            } else if let Some(c) = entity_char(name) {
                out.push_str(&format!("&#{};", c as u32));
            } else {
                out.push_str("&amp;");
                out.push_str(&rest[1..name_end + 1]);
            }
            rest = &rest[name_end + 1..];
        } else {
            out.push_str("&amp;");
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// How deep elements may nest. roxmltree overflows a small stack on a few hundred levels, and so would the
/// readers that walk the tree; real books and documents stay far below this.
const MAX_XML_DEPTH: usize = 100;

/// Whether `text` nests its elements no deeper than [`MAX_XML_DEPTH`], found by a quick scan of its tags (comments,
/// CDATA, processing instructions and the DOCTYPE are skipped; quotes in tags are respected).
fn shallow_enough(text: &str) -> bool {
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0usize, 0usize);
    let skip_to = |from: usize, end: &str| -> usize {
        text.get(from..)
            .and_then(|rest| rest.find(end))
            .map_or(bytes.len(), |at| from + at + end.len())
    };
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &bytes[i..];
        if rest.starts_with(b"<!--") {
            i = skip_to(i + 4, "-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_to(i + 9, "]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_to(i + 2, "?>");
        } else if rest.starts_with(b"<!") {
            // The DOCTYPE, with an internal subset in brackets perhaps.
            let mut brackets = 0usize;
            i += 2;
            while i < bytes.len() {
                match bytes[i] {
                    b'[' => brackets += 1,
                    b']' => brackets = brackets.saturating_sub(1),
                    b'>' if brackets == 0 => break,
                    _ => {}
                }
                i += 1;
            }
            i += 1;
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_to(i + 2, ">");
        } else {
            // A start tag: to its closing `>`, past quoted values.
            let mut quote = 0u8;
            let mut j = i + 1;
            while j < bytes.len() {
                match (quote, bytes[j]) {
                    (0, b'"' | b'\'') => quote = bytes[j],
                    (q, c) if q != 0 && c == q => quote = 0,
                    (0, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            let self_closing = j > i + 1 && bytes.get(j - 1) == Some(&b'/');
            if !self_closing {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return false;
                }
            }
            i = j + 1;
        }
    }
    true
}

/// Parses XML or XHTML (a DOCTYPE is allowed, and its entities are not loaded). Documents nested deeper than
/// [`MAX_XML_DEPTH`] are refused.
pub(super) fn parse_xml(text: &str) -> Option<Xml<'_>> {
    if !shallow_enough(text) {
        return None;
    }
    Xml::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: true,
            nodes_limit: 4_000_000,
        },
    )
    .ok()
}

/// The element's name without its namespace, as written.
pub(super) fn lname<'a>(n: Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

/// The first child element called `name`.
pub(super) fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && lname(*c) == name)
}

/// All of an element's text, white space collapsed.
pub(super) fn flat_text(n: Node) -> String {
    let mut out = String::new();
    for d in n.descendants().filter(|d| d.is_text()) {
        out.push_str(d.text().unwrap_or(""));
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An element's `epub:type` (or plain `type`), empty when it has none.
fn ops_type<'a>(n: Node<'a, '_>) -> &'a str {
    n.attribute((OPS_NS, "type"))
        .or_else(|| n.attribute("type"))
        .unwrap_or("")
}

/// A title's text for the contents: its paragraphs joined by a space.
fn title_label(title: Node) -> String {
    let paragraphs: Vec<String> = title
        .children()
        .filter(|c| c.is_element() && lname(*c) == "p")
        .map(flat_text)
        .filter(|t| !t.is_empty())
        .collect();
    if paragraphs.is_empty() {
        flat_text(title)
    } else {
        paragraphs.join(" ")
    }
}

/// An attribute by its local name, whatever namespace it is in (`href` or `xlink:href`).
pub(super) fn any_attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

// ---- A small CSS -----------------------------------------------------------------------------------------------

/// A physical text alignment as CSS says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phys {
    Left,
    Right,
    Center,
    Start,
    End,
}

/// What the CSS read says about an element.
#[derive(Clone, Debug, Default, PartialEq)]
struct Decls {
    dir: Option<Dir>,
    align: Option<Phys>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    hidden: bool,
}

impl Decls {
    /// Puts `other` over `self` (a later or more specific rule).
    fn overlay(&mut self, other: &Decls) {
        self.dir = other.dir.or(self.dir);
        self.align = other.align.or(self.align);
        self.bold = other.bold.or(self.bold);
        self.italic = other.italic.or(self.italic);
        self.underline = other.underline.or(self.underline);
        self.strike = other.strike.or(self.strike);
        self.hidden |= other.hidden;
    }
}

/// Reads `prop: value; …`, the inside of a rule or a `style` attribute, for the properties the view draws.
fn parse_decls(text: &str) -> Decls {
    let mut d = Decls::default();
    for decl in text.split(';') {
        let Some((prop, value)) = decl.split_once(':') else {
            continue;
        };
        let prop = prop.trim().to_ascii_lowercase();
        let value = value.replace("!important", "").trim().to_ascii_lowercase();
        match prop.as_str() {
            "direction" => {
                d.dir = match value.as_str() {
                    "rtl" => Some(Dir::Rtl),
                    "ltr" => Some(Dir::Ltr),
                    _ => None,
                }
            }
            "text-align" => {
                d.align = match value.as_str() {
                    "left" => Some(Phys::Left),
                    "right" => Some(Phys::Right),
                    "center" => Some(Phys::Center),
                    "start" => Some(Phys::Start),
                    "end" => Some(Phys::End),
                    _ => None,
                }
            }
            "font-weight" => {
                d.bold = match value.as_str() {
                    "bold" | "bolder" => Some(true),
                    "normal" | "lighter" => Some(false),
                    number => number.parse::<u32>().ok().map(|n| n >= 600),
                }
            }
            "font-style" => {
                d.italic = match value.as_str() {
                    "italic" | "oblique" => Some(true),
                    "normal" => Some(false),
                    _ => None,
                }
            }
            "text-decoration" | "text-decoration-line" => {
                if value.contains("underline") {
                    d.underline = Some(true);
                }
                if value.contains("line-through") {
                    d.strike = Some(true);
                }
                if value == "none" {
                    d.underline = Some(false);
                    d.strike = Some(false);
                }
            }
            "display" => d.hidden = value == "none",
            _ => {}
        }
    }
    d
}

/// One rule: a simple selector (`tag`, `.class`, `tag.class`, `#id`) and what it says.
#[derive(Clone, Debug)]
struct Rule {
    tag: Option<String>,
    class: Option<String>,
    id: Option<String>,
    decls: Decls,
}

/// The rules of a chapter's style sheets that are of the simple kinds.
#[derive(Clone, Debug, Default)]
struct Css {
    rules: Vec<Rule>,
}

impl Css {
    /// Adds the rules of a style sheet. Selectors with combinators, pseudo classes or attributes, and at-rules,
    /// are skipped.
    fn add(&mut self, sheet: &str) {
        let mut text = String::with_capacity(sheet.len());
        let mut rest = sheet;
        while let Some(start) = rest.find("/*") {
            text.push_str(&rest[..start]);
            rest = rest[start + 2..]
                .find("*/")
                .map_or("", |end| &rest[start + 2 + end + 2..]);
        }
        text.push_str(rest);
        let mut rest = text.as_str();
        while let Some(open) = rest.find('{') {
            let selectors = rest[..open].trim();
            // The block's end, counting nested braces (an @media block holds rules).
            let mut depth = 1usize;
            let mut end = rest.len();
            for (i, c) in rest[open + 1..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + 1 + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body = &rest[open + 1..end];
            if !selectors.starts_with('@') {
                let decls = parse_decls(body);
                if decls != Decls::default() {
                    for selector in selectors.split(',') {
                        if let Some(rule) = simple_selector(selector.trim(), &decls) {
                            self.rules.push(rule);
                        }
                    }
                }
            }
            rest = rest.get(end + 1..).unwrap_or("");
        }
    }

    /// What the rules say about an element, the more specific rules over the less.
    fn decls(&self, tag: &str, class: Option<&str>, id: Option<&str>) -> Decls {
        let mut hits: Vec<(u32, &Decls)> = Vec::new();
        for rule in &self.rules {
            let tag_ok = rule.tag.as_deref().is_none_or(|t| t == tag);
            let class_ok = rule
                .class
                .as_deref()
                .is_none_or(|c| class.is_some_and(|all| all.split_whitespace().any(|one| one == c)));
            let id_ok = rule.id.as_deref().is_none_or(|i| id == Some(i));
            if tag_ok && class_ok && id_ok {
                let weight = u32::from(rule.tag.is_some())
                    + 10 * u32::from(rule.class.is_some())
                    + 100 * u32::from(rule.id.is_some());
                hits.push((weight, &rule.decls));
            }
        }
        hits.sort_by_key(|(weight, _)| *weight);
        let mut out = Decls::default();
        for (_, decls) in hits {
            out.overlay(decls);
        }
        out
    }
}

/// `tag`, `.class`, `tag.class`, `#id`, `*`: the selectors read; anything else is None.
fn simple_selector(selector: &str, decls: &Decls) -> Option<Rule> {
    if selector.is_empty() || selector.contains(|c: char| c.is_whitespace() || ">+~:[]()".contains(c)) {
        return None;
    }
    let (head, id) = match selector.split_once('#') {
        Some((head, id)) => (head, Some(id.to_string())),
        None => (selector, None),
    };
    let (tag, class) = match head.split_once('.') {
        Some((tag, class)) => (tag, Some(class.to_string())),
        None => (head, None),
    };
    if class.as_deref().is_some_and(|c| c.contains('.')) {
        return None;
    }
    Some(Rule {
        tag: (!tag.is_empty() && tag != "*").then(|| tag.to_ascii_lowercase()),
        class,
        id,
        decls: decls.clone(),
    })
}

// ---- XHTML to blocks -------------------------------------------------------------------------------------------

/// What an open piece of text becomes when it is closed.
#[derive(Clone, Debug)]
enum Kind {
    Para,
    Heading(u8),
    Quote(u8),
    Item(u8, String),
}

/// A piece of text being built.
struct Open {
    text: Text,
    kind: Kind,
    phys: Option<Phys>,
}

/// What flows down from an element to its content.
#[derive(Clone)]
struct Ctx {
    style: Style,
    dir: Dir,
    phys: Option<Phys>,
    pre: bool,
}

/// Elements whose content is not shown.
const SKIPPED: [&str; 19] = [
    "script", "style", "head", "title", "meta", "link", "noscript", "audio", "video", "canvas", "iframe", "object",
    "embed", "rt", "rp", "template", "select", "button", "textarea",
];

/// Elements that hold blocks.
const CONTAINERS: [&str; 22] = [
    "html",
    "body",
    "div",
    "section",
    "article",
    "main",
    "aside",
    "header",
    "footer",
    "nav",
    "figure",
    "figcaption",
    "form",
    "fieldset",
    "address",
    "details",
    "summary",
    "center",
    "dd",
    "dt",
    "caption",
    "hgroup",
];

struct Walker<'a> {
    b: Builder,
    cur: Option<Open>,
    css: &'a Css,
    chapter: &'a str,
    arch: Option<&'a Archive>,
    budget: usize,
    lists: Vec<(bool, i64, String)>,
    quote: u8,
    heading: Option<u8>,
    marker: Option<(u8, String)>,
}

/// A chapter's XHTML as a document. `chapter` is its path inside the zip (links and pictures are relative to it);
/// `arch` is where its pictures and style sheets are read from.
fn xhtml_document(text: &str, chapter: &str, arch: Option<&Archive>) -> Document {
    let fixed = fix_entities(text);
    let Some(xml) = parse_xml(&fixed) else {
        return fallback_document(text);
    };
    let root = xml.root_element();
    let mut css = Css::default();
    for node in root.descendants().filter(|n| n.is_element()) {
        match lname(node) {
            "link"
                if node
                    .attribute("rel")
                    .is_some_and(|r| r.to_ascii_lowercase().contains("stylesheet")) =>
            {
                if let (Some(href), Some(arch)) = (node.attribute("href"), arch) {
                    if !has_scheme(href) {
                        if let Some(sheet) = arch.read_text(&resolve(dir_of(chapter), href)) {
                            css.add(&sheet);
                        }
                    }
                }
            }
            "style" => css.add(&node.descendants().filter_map(|d| d.text()).collect::<String>()),
            _ => {}
        }
    }
    let mut walker = Walker {
        b: Builder::new(),
        cur: None,
        css: &css,
        chapter,
        arch,
        budget: CHAPTER_PICTURES,
        lists: Vec::new(),
        quote: 0,
        heading: None,
        marker: None,
    };
    let cx = Ctx {
        style: Style::default(),
        dir: Dir::Auto,
        phys: None,
        pre: false,
    };
    walker.element(root, &cx);
    walker.flush();
    walker.b.finish()
}

/// A chapter that is not well-formed even after the entities were fixed: its tags stripped, paragraphs kept.
fn fallback_document(raw: &str) -> Document {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut skipping: Option<&str> = None;
    let mut tag = String::new();
    while let Some(c) = chars.next() {
        if c != '<' {
            if skipping.is_none() {
                text.push(c);
            }
            continue;
        }
        tag.clear();
        for t in chars.by_ref() {
            if t == '>' {
                break;
            }
            tag.push(t);
        }
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        match (name.as_str(), skipping) {
            ("script" | "style", None) if !closing => {
                skipping = Some(if name == "script" { "script" } else { "style" })
            }
            (n, Some(open)) if closing && n == open => skipping = None,
            (_, Some(_)) => {}
            ("p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote", None) => {
                text.push_str("\n\n")
            }
            _ => {}
        }
    }
    let text = fix_entities(&text);
    let mut decoded = String::with_capacity(text.len());
    let mut rest: &str = &text;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        rest = &rest[at..];
        match rest.find(';').filter(|end| *end <= 10) {
            Some(end) => {
                let name = &rest[1..end];
                let c = match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    other => other
                        .strip_prefix('#')
                        .and_then(|n| {
                            n.strip_prefix(['x', 'X'])
                                .map_or_else(|| n.parse().ok(), |hex| u32::from_str_radix(hex, 16).ok())
                        })
                        .and_then(char::from_u32),
                };
                match c {
                    Some(c) => decoded.push(c),
                    None => decoded.push_str(&rest[..end + 1]),
                }
                rest = &rest[end + 1..];
            }
            None => {
                decoded.push('&');
                rest = &rest[1..];
            }
        }
    }
    decoded.push_str(rest);
    let mut b = Builder::new();
    for paragraph in decoded.split("\n\n") {
        let mut text = Text::new(Dir::Auto);
        text.push(
            &paragraph.split_whitespace().collect::<Vec<_>>().join(" "),
            &Style::default(),
        );
        if !text.is_blank() {
            b.push(Block::Paragraph(text));
        }
    }
    b.finish()
}

fn ordinal(n: i64, kind: &str) -> String {
    let alpha = |n: i64, upper: bool| -> String {
        let mut n = n.max(1);
        let mut out = Vec::new();
        while n > 0 {
            let r = u8::try_from((n - 1) % 26).unwrap_or(0);
            out.push((if upper { b'A' } else { b'a' }) + r);
            n = (n - 1) / 26;
        }
        out.reverse();
        String::from_utf8(out).unwrap_or_default()
    };
    let roman = |n: i64| -> String {
        if !(1..4000).contains(&n) {
            return n.to_string();
        }
        let mut n = n;
        let mut out = String::new();
        for (value, glyph) in [
            (1000, "M"),
            (900, "CM"),
            (500, "D"),
            (400, "CD"),
            (100, "C"),
            (90, "XC"),
            (50, "L"),
            (40, "XL"),
            (10, "X"),
            (9, "IX"),
            (5, "V"),
            (4, "IV"),
            (1, "I"),
        ] {
            while n >= value {
                out.push_str(glyph);
                n -= value;
            }
        }
        out
    };
    match kind {
        "a" => alpha(n, false),
        "A" => alpha(n, true),
        "i" => roman(n).to_lowercase(),
        "I" => roman(n),
        _ => n.to_string(),
    }
}

impl Walker<'_> {
    fn children(&mut self, n: Node, cx: &Ctx) {
        for c in n.children() {
            if c.is_text() {
                if let Some(text) = c.text() {
                    self.text(text, cx);
                }
            } else if c.is_element() {
                self.element(c, cx);
            }
        }
    }

    /// The text being built, opened if need be.
    fn open(&mut self, cx: &Ctx) -> &mut Open {
        if self.cur.is_none() {
            let kind = if let Some((depth, marker)) = self.marker.take() {
                Kind::Item(depth, marker)
            } else if let Some(level) = self.heading {
                Kind::Heading(level)
            } else if self.quote > 0 {
                Kind::Quote(self.quote)
            } else {
                Kind::Para
            };
            self.cur = Some(Open {
                text: Text::new(cx.dir),
                kind,
                phys: cx.phys,
            });
        }
        self.cur.get_or_insert_with(|| Open {
            text: Text::new(cx.dir),
            kind: Kind::Para,
            phys: cx.phys,
        })
    }

    /// Adds a text node: white space collapsed to one space as HTML does, except in `pre`.
    fn text(&mut self, raw: &str, cx: &Ctx) {
        if cx.pre {
            self.open(cx).text.push(raw, &cx.style);
            return;
        }
        let after_space = self
            .cur
            .as_ref()
            .is_none_or(|open| open.text.spans.is_empty() || open.text.ends_with_space());
        let mut out = String::with_capacity(raw.len());
        let mut last_space = after_space;
        for c in raw.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                if !last_space {
                    out.push(' ');
                    last_space = true;
                }
            } else {
                out.push(c);
                last_space = false;
            }
        }
        if out.trim().is_empty() && self.cur.is_none() {
            return;
        }
        if !out.is_empty() {
            self.open(cx).text.push(&out, &cx.style);
        }
    }

    /// Closes the text being built into a block.
    fn flush(&mut self) {
        let Some(Open { mut text, kind, phys }) = self.cur.take() else {
            return;
        };
        text.trim();
        if text.is_blank() {
            return;
        }
        let rtl = text.is_rtl(false);
        text.align = match (phys, rtl) {
            (Some(Phys::Center), _) => Align::Center,
            (Some(Phys::Left), false) | (Some(Phys::Right), true) | (Some(Phys::Start), _) => Align::Start,
            (Some(Phys::Right), false) | (Some(Phys::Left), true) | (Some(Phys::End), _) => Align::End,
            (None, _) => Align::Start,
        };
        self.b.push(match kind {
            Kind::Para => Block::Paragraph(text),
            Kind::Heading(level) => Block::Heading { level, text },
            Kind::Quote(depth) => Block::Quote { depth, text },
            Kind::Item(depth, marker) => Block::Item { depth, marker, text },
        });
    }

    /// Where a link goes: the web as it is, a place in the book as a path in the zip (with `#id`).
    fn link_target(&self, href: &str) -> Option<Arc<str>> {
        let href = href.trim();
        if href.is_empty() {
            return None;
        }
        if is_external(href) {
            return Some(href.into());
        }
        if has_scheme(href) {
            return None;
        }
        let (path, fragment) = split_href(href);
        let target = if path.is_empty() {
            String::new()
        } else {
            resolve(dir_of(self.chapter), path)
        };
        Some(
            match fragment {
                Some(fragment) => format!("{target}#{fragment}"),
                None => target,
            }
            .into(),
        )
    }

    /// A picture by its address in the book.
    fn load_picture(&mut self, href: &str, alt: &str) -> Picture {
        let none = || Picture {
            image: None,
            size: None,
            alt: alt.to_string(),
        };
        if href.len() > 5 && href[..5].eq_ignore_ascii_case("data:") {
            let bytes = href.split_once(',').and_then(|(_, data)| base64_decode(data));
            return match bytes {
                Some(bytes) if bytes.len() <= self.budget => {
                    self.budget -= bytes.len();
                    picture_of(bytes, alt, None)
                }
                _ => none(),
            };
        }
        if has_scheme(href) {
            return none();
        }
        let Some(arch) = self.arch else {
            return none();
        };
        let path = resolve(dir_of(self.chapter), split_href(href).0);
        if arch.size(&path) as usize > self.budget {
            return none();
        }
        match arch.read(&path, IMAGE_LIMIT) {
            Some(bytes) => {
                self.budget = self.budget.saturating_sub(bytes.len());
                picture_of(bytes, alt, None)
            }
            None => none(),
        }
    }

    fn element(&mut self, n: Node, cx: &Ctx) {
        let tag = lname(n).to_ascii_lowercase();
        if SKIPPED.contains(&tag.as_str()) {
            return;
        }
        let mut d = self.css.decls(&tag, n.attribute("class"), n.attribute("id"));
        if let Some(style) = n.attribute("style") {
            d.overlay(&parse_decls(style));
        }
        let pagebreak = n
            .attribute((OPS_NS, "type"))
            .or_else(|| n.attribute("type"))
            .is_some_and(|t| t.contains("pagebreak"));
        if d.hidden || n.has_attribute("hidden") || pagebreak {
            return;
        }
        if let Some(id) = n
            .attribute("id")
            .or_else(|| (tag == "a").then(|| n.attribute("name")).flatten())
        {
            self.b.anchor(id);
        }
        let mut cx = cx.clone();
        match n.attribute("dir").map(str::to_ascii_lowercase).as_deref() {
            Some("rtl") => cx.dir = Dir::Rtl,
            Some("ltr") => cx.dir = Dir::Ltr,
            _ => {}
        }
        if let Some(dir) = d.dir {
            cx.dir = dir;
        }
        match n.attribute("align").map(str::to_ascii_lowercase).as_deref() {
            Some("center") => cx.phys = Some(Phys::Center),
            Some("right") => cx.phys = Some(Phys::Right),
            Some("left") => cx.phys = Some(Phys::Left),
            _ => {}
        }
        if let Some(align) = d.align {
            cx.phys = Some(align);
        }
        cx.style.bold = d.bold.unwrap_or(cx.style.bold);
        cx.style.italic = d.italic.unwrap_or(cx.style.italic);
        cx.style.underline = d.underline.unwrap_or(cx.style.underline);
        cx.style.strike = d.strike.unwrap_or(cx.style.strike);
        match tag.as_str() {
            "p" => {
                self.flush();
                self.children(n, &cx);
                self.flush();
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let level = tag.as_bytes().get(1).map_or(1, |b| b - b'0').clamp(1, 6);
                let saved = self.heading.replace(level);
                self.children(n, &cx);
                self.flush();
                self.heading = saved;
            }
            "blockquote" => {
                self.flush();
                self.quote += 1;
                self.children(n, &cx);
                self.flush();
                self.quote -= 1;
            }
            "pre" => {
                self.flush();
                let mut code = String::new();
                for d in n.descendants() {
                    if d.is_text() {
                        code.push_str(d.text().unwrap_or(""));
                    } else if d.is_element() && lname(d) == "br" {
                        code.push('\n');
                    }
                }
                let code = code.trim_matches('\n').to_string();
                if !code.trim().is_empty() {
                    self.b.push(Block::Code(code));
                }
            }
            "ul" | "ol" => {
                self.flush();
                let ordered = tag == "ol";
                let start = n
                    .attribute("start")
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .unwrap_or(1);
                let kind = n.attribute("type").unwrap_or("1").to_string();
                self.lists.push((ordered, start - 1, kind));
                self.children(n, &cx);
                self.flush();
                self.lists.pop();
            }
            "li" => {
                self.flush();
                let depth = u8::try_from(self.lists.len().saturating_sub(1)).unwrap_or(u8::MAX);
                let marker = match self.lists.last_mut() {
                    Some((true, counter, kind)) => {
                        *counter = n
                            .attribute("value")
                            .and_then(|v| v.trim().parse::<i64>().ok())
                            .unwrap_or(*counter + 1);
                        format!("{}.", ordinal(*counter, kind))
                    }
                    _ => ["•", "◦", "▪"][usize::from(depth) % 3].to_string(),
                };
                self.marker = Some((depth, marker));
                self.children(n, &cx);
                self.flush();
                self.marker = None;
            }
            "dt" => {
                self.flush();
                cx.style.bold = true;
                self.children(n, &cx);
                self.flush();
            }
            "dd" => {
                self.flush();
                self.marker = Some((1, String::new()));
                self.children(n, &cx);
                self.flush();
                self.marker = None;
            }
            "table" => self.table(n, &cx),
            "hr" => {
                self.flush();
                self.b.push(Block::Rule);
            }
            "br" => {
                if self.cur.is_some() {
                    self.open(&cx).text.push("\n", &cx.style);
                }
            }
            "img" => {
                self.flush();
                let alt = n.attribute("alt").unwrap_or("").trim().to_string();
                let picture = match n.attribute("src") {
                    Some(src) => self.load_picture(src, &alt),
                    None => Picture {
                        image: None,
                        size: None,
                        alt,
                    },
                };
                self.b.push(Block::Picture(picture));
            }
            "svg" => {
                self.flush();
                // A cover page is often an SVG holding one <image>.
                if let Some(href) = n
                    .descendants()
                    .filter(|d| d.is_element() && lname(*d) == "image")
                    .find_map(|d| any_attr(d, "href"))
                {
                    let picture = self.load_picture(href, "");
                    self.b.push(Block::Picture(picture));
                }
            }
            "math" => {
                let text = flat_text(n);
                if !text.is_empty() {
                    let mut style = cx.style.clone();
                    style.italic = true;
                    self.open(&cx).text.push(&text, &style);
                }
            }
            "a" => {
                let mut inner = cx.clone();
                if let Some(target) = n.attribute("href").and_then(|href| self.link_target(href)) {
                    inner.style.link = Some(target);
                }
                self.children(n, &inner);
            }
            "q" => {
                self.text("“", &cx);
                self.children(n, &cx);
                self.text("”", &cx);
            }
            _ if CONTAINERS.contains(&tag.as_str()) => {
                self.flush();
                if tag == "center" {
                    cx.phys = Some(Phys::Center);
                }
                self.children(n, &cx);
                self.flush();
            }
            _ => {
                match tag.as_str() {
                    "b" | "strong" => cx.style.bold = true,
                    "i" | "em" | "cite" | "dfn" | "var" => cx.style.italic = true,
                    "u" | "ins" => cx.style.underline = true,
                    "s" | "strike" | "del" => cx.style.strike = true,
                    "code" | "kbd" | "samp" | "tt" => cx.style.code = true,
                    "sup" => cx.style.script = Script::Super,
                    "sub" => cx.style.script = Script::Sub,
                    "bdo" | "bdi" | "span" => {
                        if let Some(dir) = n.attribute("dir") {
                            cx.style.dir = match dir.to_ascii_lowercase().as_str() {
                                "rtl" => Dir::Rtl,
                                "ltr" => Dir::Ltr,
                                _ => cx.style.dir,
                            };
                        }
                    }
                    _ => {}
                }
                self.children(n, &cx);
            }
        }
    }

    /// A table: its rows of cells, each cell's content read into blocks of its own.
    fn table(&mut self, n: Node, cx: &Ctx) {
        self.flush();
        if let Some(caption) = child(n, "caption") {
            let mut inner = cx.clone();
            inner.style.bold = true;
            self.children(caption, &inner);
            self.flush();
        }
        let mut trs = Vec::new();
        fn collect<'a, 'i>(n: Node<'a, 'i>, out: &mut Vec<Node<'a, 'i>>) {
            for c in n.children().filter(|c| c.is_element()) {
                match lname(c) {
                    "tr" => out.push(c),
                    "thead" | "tbody" | "tfoot" => collect(c, out),
                    _ => {}
                }
            }
        }
        collect(n, &mut trs);
        let mut rows = Vec::new();
        let mut rtl_hint = None;
        for tr in trs {
            let cells_nodes: Vec<Node> = tr
                .children()
                .filter(|c| c.is_element() && matches!(lname(*c), "td" | "th"))
                .collect();
            let header = !cells_nodes.is_empty() && cells_nodes.iter().all(|c| lname(*c) == "th");
            let mut cells = Vec::new();
            for td in cells_nodes {
                let span = td
                    .attribute("colspan")
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 50);
                let mut inner = cx.clone();
                inner.style.bold |= lname(td) == "th";
                let saved_cur = self.cur.take();
                let saved_b = std::mem::take(&mut self.b);
                self.children(td, &inner);
                self.flush();
                let blocks = std::mem::replace(&mut self.b, saved_b).finish().blocks;
                self.cur = saved_cur;
                if rtl_hint.is_none() {
                    rtl_hint = blocks.iter().find_map(|b| match b {
                        Block::Paragraph(t) | Block::Heading { text: t, .. } | Block::Item { text: t, .. } => {
                            rich::first_strong_rtl(&t.plain())
                        }
                        _ => None,
                    });
                }
                cells.push(Cell {
                    blocks,
                    span,
                    shade: None,
                });
            }
            if !cells.is_empty() {
                rows.push(Row { cells, header });
            }
        }
        if rows.is_empty() {
            return;
        }
        let rtl = match cx.dir {
            Dir::Rtl => true,
            Dir::Ltr => false,
            Dir::Auto => rtl_hint.unwrap_or(false),
        };
        self.b.push(Block::Table(Table { rows, rtl }));
    }
}

// ---- What a book is --------------------------------------------------------------------------------------------

/// One chapter, ready to show.
pub enum Chapter {
    /// Text, as blocks.
    Text(Arc<Document>),
    /// A comic's page.
    Page(Arc<Image>),
}

/// Where a book's chapters come from: read when asked for.
trait Source: Send + Sync {
    /// Chapter `ix`, read and mapped.
    fn chapter(&self, ix: usize) -> Result<Chapter, String>;
    /// The chapter a file path inside the book is.
    fn locate(&self, _path: &str) -> Option<usize> {
        None
    }
    /// The chapter an id is on, for links that name only the id.
    fn chapter_with_anchor(&self, _id: &str) -> Option<usize> {
        None
    }
}

/// An entry of the contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TocItem {
    pub label: String,
    pub chapter: usize,
    /// The id within the chapter, if the entry points past its start.
    pub anchor: Option<String>,
    pub children: Vec<TocItem>,
}

/// A contents entry in reading order, with its depth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TocRow {
    pub depth: u8,
    pub label: String,
    pub chapter: usize,
    pub anchor: Option<String>,
}

/// The contents flattened, in reading order.
pub fn flatten(items: &[TocItem]) -> Vec<TocRow> {
    fn walk(items: &[TocItem], depth: u8, out: &mut Vec<TocRow>) {
        for item in items {
            out.push(TocRow {
                depth,
                label: item.label.clone(),
                chapter: item.chapter,
                anchor: item.anchor.clone(),
            });
            walk(&item.children, depth.saturating_add(1), out);
        }
    }
    let mut out = Vec::new();
    walk(items, 0, &mut out);
    out
}

/// A book, opened: what it is called, how it turns, its contents, and where its chapters are read from.
pub struct Book {
    pub title: String,
    /// Pages turn from the right (`page-progression-direction`, or a right-to-left language, or manga).
    pub rtl: bool,
    pub toc: Vec<TocItem>,
    pub count: usize,
    /// How much each chapter weighs in the book's progress (its size).
    pub weights: Vec<u64>,
    /// The chapter to open at when nothing is remembered.
    pub start: usize,
    /// The cover, when it is small enough to show beside the contents.
    pub cover: Option<Arc<Image>>,
    /// A comic: each chapter is one picture.
    pub pages: bool,
    source: Arc<dyn Source>,
}

impl Book {
    /// Chapter `ix`, read.
    pub fn chapter(&self, ix: usize) -> Result<Chapter, String> {
        self.source.chapter(ix)
    }
}

/// Reads a book of any kind this view knows, by its extension.
pub fn open_book(path: &str) -> Result<Book, BookError> {
    let ext = super::text::ext_of(path);
    let mut book = match ext.as_str() {
        "epub" => open_epub(path),
        "fb2" => {
            let size = std::fs::metadata(path).map_or(0, |m| m.len());
            if size > FB2_LIMIT {
                return Err(BookError::Broken(t!("This book is too big to preview").to_string()));
            }
            let bytes = std::fs::read(path).map_err(|e| BookError::Broken(e.to_string()))?;
            open_fb2(&bytes)
        }
        "fbz" => {
            let arch = Archive::open(path)?;
            let name = arch
                .names
                .iter()
                .find(|n| n.to_ascii_lowercase().ends_with(".fb2"))
                .cloned()
                .ok_or(BookError::Unsupported)?;
            let bytes = arch.read(&name, FB2_LIMIT).ok_or(BookError::Unsupported)?;
            open_fb2(&bytes)
        }
        "cbz" => open_cbz(path),
        _ => Err(BookError::Unsupported),
    }?;
    if book.title.trim().is_empty() {
        book.title = super::text::name_of(path).to_string();
    }
    Ok(book)
}

// ---- EPUB ------------------------------------------------------------------------------------------------------

struct EpubSource {
    arch: Archive,
    /// Each spine item's path in the zip.
    spine: Vec<String>,
    by_path: HashMap<String, usize>,
}

impl Source for EpubSource {
    fn chapter(&self, ix: usize) -> Result<Chapter, String> {
        let path = self.spine.get(ix).ok_or_else(|| "No such chapter".to_string())?;
        let text = self
            .arch
            .read_text(path)
            .ok_or_else(|| t!("This part of the book can't be read.").to_string())?;
        Ok(Chapter::Text(Arc::new(xhtml_document(&text, path, Some(&self.arch)))))
    }

    fn locate(&self, path: &str) -> Option<usize> {
        self.by_path.get(&path.to_lowercase()).copied()
    }
}

struct Item {
    path: String,
    media: String,
    props: String,
}

/// A navigation entry before its target is a chapter.
struct RawToc {
    label: String,
    href: String,
    children: Vec<RawToc>,
}

fn nav_list(ol: Node) -> Vec<RawToc> {
    let mut out = Vec::new();
    for li in ol.children().filter(|c| c.is_element() && lname(*c) == "li") {
        let link = li
            .children()
            .find(|c| c.is_element() && matches!(lname(*c), "a" | "span"));
        let label = link.map(flat_text).unwrap_or_default();
        let href = link.and_then(|l| l.attribute("href")).unwrap_or("").to_string();
        let children = li
            .children()
            .find(|c| c.is_element() && lname(*c) == "ol")
            .map(nav_list)
            .unwrap_or_default();
        out.push(RawToc { label, href, children });
    }
    out
}

fn ncx_points(parent: Node) -> Vec<RawToc> {
    parent
        .children()
        .filter(|c| c.is_element() && lname(*c) == "navPoint")
        .map(|point| RawToc {
            label: child(point, "navLabel").map(flat_text).unwrap_or_default(),
            href: child(point, "content")
                .and_then(|c| c.attribute("src"))
                .unwrap_or("")
                .to_string(),
            children: ncx_points(point),
        })
        .collect()
}

/// Turns hrefs (relative to the navigation file's folder `dir`) into chapters.
fn resolve_toc(raw: Vec<RawToc>, dir: &str, by_path: &HashMap<String, usize>) -> Vec<TocItem> {
    let mut out = Vec::new();
    for item in raw {
        let children = resolve_toc(item.children, dir, by_path);
        let (path, fragment) = split_href(&item.href);
        let chapter = by_path.get(&resolve(dir, path).to_lowercase()).copied();
        let label = item.label.trim().to_string();
        match chapter {
            Some(chapter) if !label.is_empty() => out.push(TocItem {
                label,
                chapter,
                anchor: fragment.filter(|f| !f.is_empty()),
                children,
            }),
            // An entry that goes nowhere still has its children.
            _ => out.extend(children),
        }
    }
    out
}

/// Opens an EPUB: its container, package, contents and cover. Chapters are read when asked for.
pub fn open_epub(path: &str) -> Result<Book, BookError> {
    let arch = Archive::open(path)?;
    if let Some(encryption) = arch.read_text("META-INF/encryption.xml") {
        if let Some(xml) = parse_xml(&encryption) {
            let drm = xml
                .descendants()
                .filter(|n| n.is_element() && lname(*n) == "EncryptionMethod")
                .filter_map(|n| n.attribute("Algorithm"))
                .any(|a| !(a.contains("idpf.org/2008/embedding") || a.contains("ns.adobe.com/pdf/enc#RC")));
            if drm {
                return Err(BookError::Drm);
            }
        }
    }
    let container = arch.read_text("META-INF/container.xml").ok_or(BookError::Unsupported)?;
    let opf_path = {
        let xml = parse_xml(&container).ok_or(BookError::Unsupported)?;
        xml.descendants()
            .find(|n| n.is_element() && lname(*n) == "rootfile")
            .and_then(|n| n.attribute("full-path"))
            .map(normalize)
            .ok_or(BookError::Unsupported)?
    };
    let opf_text = arch.read_text(&opf_path).ok_or(BookError::Unsupported)?;
    let opf = parse_xml(&opf_text).ok_or(BookError::Unsupported)?;
    let package = opf.root_element();
    let opf_dir = dir_of(&opf_path).to_string();

    let mut items: HashMap<String, Item> = HashMap::new();
    if let Some(manifest) = child(package, "manifest") {
        for item in manifest.children().filter(|c| c.is_element() && lname(*c) == "item") {
            if let (Some(id), Some(href)) = (item.attribute("id"), item.attribute("href")) {
                items.insert(
                    id.to_string(),
                    Item {
                        path: resolve(&opf_dir, href),
                        media: item.attribute("media-type").unwrap_or("").to_ascii_lowercase(),
                        props: item.attribute("properties").unwrap_or("").to_string(),
                    },
                );
            }
        }
    }
    let is_page = |item: &Item| {
        matches!(
            item.media.as_str(),
            "application/xhtml+xml" | "text/html" | "application/x-dtbook+xml" | "image/svg+xml" | ""
        ) && !item.path.is_empty()
    };
    let spine_node = child(package, "spine");
    let mut spine: Vec<String> = Vec::new();
    if let Some(node) = spine_node {
        let refs: Vec<Node> = node
            .children()
            .filter(|c| c.is_element() && lname(*c) == "itemref")
            .collect();
        let linear = |r: &Node| r.attribute("linear").is_none_or(|l| !l.eq_ignore_ascii_case("no"));
        let pick = |only_linear: bool| -> Vec<String> {
            refs.iter()
                .filter(|r| !only_linear || linear(r))
                .filter_map(|r| r.attribute("idref"))
                .filter_map(|id| items.get(id))
                .filter(|item| is_page(item))
                .map(|item| item.path.clone())
                .collect()
        };
        spine = pick(true);
        if spine.is_empty() {
            spine = pick(false);
        }
    }
    if spine.is_empty() {
        return Err(BookError::Unsupported);
    }
    let by_path: HashMap<String, usize> = spine
        .iter()
        .enumerate()
        .map(|(i, p)| (p.to_lowercase(), i))
        .rev()
        .collect();

    let metadata = child(package, "metadata");
    let title = metadata
        .and_then(|m| m.children().find(|c| c.is_element() && lname(*c) == "title"))
        .map(flat_text)
        .unwrap_or_default();
    let language = metadata
        .and_then(|m| m.children().find(|c| c.is_element() && lname(*c) == "language"))
        .map(flat_text)
        .unwrap_or_default();
    let ppd = spine_node.and_then(|s| s.attribute("page-progression-direction"));
    let rtl = match ppd {
        Some(d) if d.eq_ignore_ascii_case("rtl") => true,
        Some(d) if d.eq_ignore_ascii_case("ltr") => false,
        _ => rtl_language(&language),
    };

    // The contents: the nav document (EPUB 3), else the NCX (EPUB 2).
    let mut toc = Vec::new();
    let mut start_href: Option<(String, String)> = None; // (folder, href) of the bodymatter landmark
    if let Some(nav) = items.values().find(|i| i.props.split_whitespace().any(|p| p == "nav")) {
        if let Some(text) = arch.read_text(&nav.path) {
            if let Some(xml) = parse_xml(&text) {
                let nav_dir = dir_of(&nav.path).to_string();
                let navs: Vec<Node> = xml
                    .descendants()
                    .filter(|n| n.is_element() && lname(*n) == "nav")
                    .collect();
                let toc_nav = navs.iter().find(|n| ops_type(**n).contains("toc")).or(navs.first());
                if let Some(ol) = toc_nav.and_then(|n| child(*n, "ol")) {
                    toc = resolve_toc(nav_list(ol), &nav_dir, &by_path);
                }
                if let Some(landmarks) = navs.iter().find(|n| ops_type(**n).contains("landmarks")) {
                    start_href = landmarks
                        .descendants()
                        .filter(|d| d.is_element() && lname(*d) == "a")
                        .find(|a| {
                            let k = ops_type(*a);
                            k.contains("bodymatter") || k == "text"
                        })
                        .and_then(|a| a.attribute("href"))
                        .map(|h| (nav_dir.clone(), h.to_string()));
                }
            }
        }
    }
    if toc.is_empty() {
        let ncx = spine_node
            .and_then(|s| s.attribute("toc"))
            .and_then(|id| items.get(id))
            .or_else(|| items.values().find(|i| i.media == "application/x-dtbncx+xml"));
        if let Some(ncx) = ncx {
            if let Some(text) = arch.read_text(&ncx.path) {
                if let Some(xml) = parse_xml(&text) {
                    if let Some(map) = xml.descendants().find(|n| n.is_element() && lname(*n) == "navMap") {
                        toc = resolve_toc(ncx_points(map), dir_of(&ncx.path), &by_path);
                    }
                }
            }
        }
    }
    if start_href.is_none() {
        start_href = child(package, "guide")
            .and_then(|g| {
                g.children()
                    .filter(|c| c.is_element() && lname(*c) == "reference")
                    .find(|r| matches!(r.attribute("type"), Some("text" | "bodymatter")))
            })
            .and_then(|r| r.attribute("href"))
            .map(|h| (opf_dir.clone(), h.to_string()));
    }
    let start = start_href
        .and_then(|(dir, href)| by_path.get(&resolve(&dir, split_href(&href).0).to_lowercase()).copied())
        .unwrap_or(0);

    // The cover: the item marked cover-image, else the one the cover meta names.
    let cover_path = items
        .values()
        .find(|i| i.props.split_whitespace().any(|p| p == "cover-image"))
        .map(|i| i.path.clone())
        .or_else(|| {
            metadata
                .and_then(|m| {
                    m.children()
                        .find(|c| c.is_element() && lname(*c) == "meta" && c.attribute("name") == Some("cover"))
                })
                .and_then(|m| m.attribute("content"))
                .and_then(|id| items.get(id))
                .map(|i| i.path.clone())
        });
    let cover = cover_path
        .and_then(|p| arch.read(&p, 4 * 1024 * 1024))
        .map(|bytes| picture_of(bytes, "", None))
        .filter(|p| p.size.is_none_or(|(w, h)| w <= 3000 && h <= 3000))
        .and_then(|p| p.image);

    let weights = spine.iter().map(|p| arch.size(p).max(1)).collect();
    let count = spine.len();
    Ok(Book {
        title,
        rtl,
        toc,
        count,
        weights,
        start,
        cover,
        pages: false,
        source: Arc::new(EpubSource { arch, spine, by_path }),
    })
}

// ---- FB2 -------------------------------------------------------------------------------------------------------

/// A built chapter and the ids on it.
struct Fb2Source {
    chapters: Vec<Arc<Document>>,
}

impl Source for Fb2Source {
    fn chapter(&self, ix: usize) -> Result<Chapter, String> {
        self.chapters
            .get(ix)
            .map(|doc| Chapter::Text(doc.clone()))
            .ok_or_else(|| "No such chapter".to_string())
    }

    fn chapter_with_anchor(&self, id: &str) -> Option<usize> {
        self.chapters.iter().position(|doc| doc.anchors.contains_key(id))
    }
}

struct Fb2Walker<'a, 'i> {
    b: Builder,
    /// The file's `binary` elements by id: base64 text.
    binaries: HashMap<String, Node<'a, 'i>>,
    budget: usize,
}

impl Fb2Walker<'_, '_> {
    /// The text of an inline run: `strong`, `emphasis`, links, notes, images (collected to follow the paragraph).
    fn inline(&mut self, n: Node, style: &Style, text: &mut Text, pictures: &mut Vec<Picture>) {
        for c in n.children() {
            if c.is_text() {
                let raw = c.text().unwrap_or("");
                let mut out = String::with_capacity(raw.len());
                let mut last_space = text.spans.is_empty() || text.ends_with_space();
                for ch in raw.chars() {
                    if ch.is_whitespace() {
                        if !last_space {
                            out.push(' ');
                            last_space = true;
                        }
                    } else {
                        out.push(ch);
                        last_space = false;
                    }
                }
                text.push(&out, style);
            } else if c.is_element() {
                if let Some(id) = c.attribute("id") {
                    self.b.anchor(id);
                }
                let mut inner = style.clone();
                match lname(c) {
                    "strong" => inner.bold = true,
                    "emphasis" => inner.italic = true,
                    "strikethrough" => inner.strike = true,
                    "sub" => inner.script = Script::Sub,
                    "sup" => inner.script = Script::Super,
                    "code" => inner.code = true,
                    "a" => {
                        if let Some(href) = any_attr(c, "href") {
                            if is_external(href) || href.starts_with('#') {
                                inner.link = Some(percent_decode(href).into());
                            }
                            if any_attr(c, "type") == Some("note") {
                                inner.script = Script::Super;
                            }
                        }
                    }
                    "image" => {
                        pictures.push(self.picture(c));
                        continue;
                    }
                    _ => {}
                }
                self.inline(c, &inner, text, pictures);
            }
        }
    }

    /// A picture from an `image` element, whose bytes are a `binary` of the file.
    fn picture(&mut self, image: Node) -> Picture {
        let alt = any_attr(image, "title")
            .or_else(|| any_attr(image, "alt"))
            .unwrap_or("")
            .to_string();
        let id = any_attr(image, "href").map(|h| h.trim_start_matches('#').to_string());
        let bytes = id
            .and_then(|id| self.binaries.get(&id))
            .and_then(|node| base64_decode(node.text().unwrap_or("")))
            .filter(|b| b.len() <= self.budget);
        match bytes {
            Some(bytes) => {
                self.budget -= bytes.len();
                picture_of(bytes, &alt, None)
            }
            None => Picture {
                image: None,
                size: None,
                alt,
            },
        }
    }

    /// A block of inline content as a paragraph (and the pictures that were in it, after it).
    fn paragraph(&mut self, n: Node, style: &Style, make: impl FnOnce(Text) -> Block) {
        let mut text = Text::new(Dir::Auto);
        let mut pictures = Vec::new();
        self.inline(n, style, &mut text, &mut pictures);
        text.trim();
        if !text.is_blank() {
            self.b.push(make(text));
        }
        for picture in pictures {
            self.b.push(Block::Picture(picture));
        }
    }

    /// A title's text: its paragraphs joined by spaces.
    fn title_text(&mut self, title: Node) -> Text {
        let mut text = Text::new(Dir::Auto);
        let mut pictures = Vec::new();
        for p in title.children().filter(|c| c.is_element() && lname(*c) == "p") {
            if !text.spans.is_empty() {
                text.push(" ", &Style::default());
            }
            self.inline(p, &Style::default(), &mut text, &mut pictures);
        }
        text.trim();
        text
    }

    /// The blocks of one element's content (a section, an epigraph, a cite).
    fn blocks(&mut self, n: Node, depth: u8, quote: bool) {
        for c in n.children().filter(|c| c.is_element()) {
            if lname(c) == "section" {
                self.b.anchor(section_id(c));
            } else if let Some(id) = c.attribute("id") {
                self.b.anchor(id);
            }
            match lname(c) {
                "title" => {
                    let text = self.title_text(c);
                    if !text.is_blank() {
                        self.b.push(Block::Heading {
                            level: depth.clamp(1, 5),
                            text,
                        });
                    }
                }
                "subtitle" => self.paragraph(c, &Style::default(), |text| Block::Heading { level: 5, text }),
                "p" => {
                    if quote {
                        self.paragraph(c, &Style::default(), |text| Block::Quote { depth: 1, text });
                    } else {
                        self.paragraph(c, &Style::default(), Block::Paragraph);
                    }
                }
                "text-author" => {
                    let style = Style {
                        italic: true,
                        ..Default::default()
                    };
                    self.paragraph(c, &style, |mut text| {
                        text.align = Align::End;
                        Block::Paragraph(text)
                    });
                }
                "epigraph" | "cite" => self.blocks(c, depth, true),
                "annotation" => {
                    let style = Style {
                        italic: true,
                        ..Default::default()
                    };
                    for p in c.children().filter(|p| p.is_element() && lname(*p) == "p") {
                        self.paragraph(p, &style, Block::Paragraph);
                    }
                }
                "poem" => {
                    for stanza in c.descendants().filter(|s| s.is_element() && lname(*s) == "stanza") {
                        let mut text = Text::new(Dir::Auto);
                        let mut pictures = Vec::new();
                        for v in stanza.children().filter(|v| v.is_element() && lname(*v) == "v") {
                            if !text.spans.is_empty() {
                                text.push("\n", &Style::default());
                            }
                            self.inline(v, &Style::default(), &mut text, &mut pictures);
                        }
                        text.trim();
                        if !text.is_blank() {
                            self.b.push(Block::Quote { depth: 1, text });
                        }
                    }
                    for author in c.children().filter(|a| a.is_element() && lname(*a) == "text-author") {
                        self.paragraph(author, &Style::default(), |mut text| {
                            text.align = Align::End;
                            Block::Paragraph(text)
                        });
                    }
                }
                "image" => {
                    let picture = self.picture(c);
                    self.b.push(Block::Picture(picture));
                }
                "empty-line" => {}
                "table" => {
                    let mut rows = Vec::new();
                    for tr in c.descendants().filter(|r| r.is_element() && lname(*r) == "tr") {
                        let mut cells = Vec::new();
                        let mut header = true;
                        for td in tr
                            .children()
                            .filter(|d| d.is_element() && matches!(lname(*d), "td" | "th"))
                        {
                            header &= lname(td) == "th";
                            let mut text = Text::new(Dir::Auto);
                            let mut pictures = Vec::new();
                            self.inline(td, &Style::default(), &mut text, &mut pictures);
                            text.trim();
                            cells.push(Cell {
                                blocks: vec![Block::Paragraph(text)],
                                span: td
                                    .attribute("colspan")
                                    .and_then(|s| s.parse().ok())
                                    .unwrap_or(1u32)
                                    .clamp(1, 50),
                                shade: None,
                            });
                        }
                        if !cells.is_empty() {
                            rows.push(Row { cells, header });
                        }
                    }
                    if !rows.is_empty() {
                        self.b.push(Block::Table(Table { rows, rtl: false }));
                    }
                }
                "section" => {
                    // Reached only for sections inside sections; the caller handles each as a part of the chapter.
                    self.blocks(c, depth.saturating_add(1), quote);
                }
                _ => {}
            }
        }
    }

    /// The contents entries of the sections inside `section`. A section without an id is named by where it is in
    /// the file (see `blocks`), so the contents can scroll to it.
    fn section_entries(&mut self, section: Node, chapter: usize) -> Vec<TocItem> {
        let mut out = Vec::new();
        for c in section.children().filter(|c| c.is_element() && lname(*c) == "section") {
            let label = child(c, "title").map(title_label).unwrap_or_default();
            let children = self.section_entries(c, chapter);
            if label.is_empty() {
                out.extend(children);
                continue;
            }
            out.push(TocItem {
                label,
                chapter,
                anchor: Some(section_id(c)),
                children,
            });
        }
        out
    }
}

/// A section's id, or one made from where it is in the file.
fn section_id(section: Node) -> String {
    section
        .attribute("id")
        .map_or_else(|| format!("section-{}", section.range().start), str::to_string)
}

/// Opens an FB2 file's bytes: every top-level section is a chapter, the notes body is the last.
pub fn open_fb2(bytes: &[u8]) -> Result<Book, BookError> {
    let text = xml_text(bytes);
    let xml = parse_xml(&text).ok_or(BookError::Unsupported)?;
    let root = xml.root_element();
    if lname(root) != "FictionBook" {
        return Err(BookError::Unsupported);
    }
    let info = child(root, "description").and_then(|d| child(d, "title-info"));
    let title = info
        .and_then(|i| child(i, "book-title"))
        .map(flat_text)
        .unwrap_or_default();
    let language = info.and_then(|i| child(i, "lang")).map(flat_text).unwrap_or_default();
    let cover_id = info
        .and_then(|i| child(i, "coverpage"))
        .and_then(|c| child(c, "image"))
        .and_then(|i| any_attr(i, "href"))
        .map(|h| h.trim_start_matches('#').to_string());

    let mut binaries: HashMap<String, Node> = HashMap::new();
    for binary in root.children().filter(|c| c.is_element() && lname(*c) == "binary") {
        if let Some(id) = binary.attribute("id") {
            binaries.insert(id.to_string(), binary);
        }
    }
    let cover = cover_id
        .and_then(|id| {
            binaries
                .get(&id)
                .and_then(|node| base64_decode(node.text().unwrap_or("")))
        })
        .filter(|b| b.len() <= 4 * 1024 * 1024)
        .map(|b| picture_of(b, "", None))
        .filter(|p| p.size.is_none_or(|(w, h)| w <= 3000 && h <= 3000))
        .and_then(|p| p.image);
    let mut walker = Fb2Walker {
        b: Builder::new(),
        binaries,
        budget: 256 * 1024 * 1024,
    };

    let mut chapters: Vec<Arc<Document>> = Vec::new();
    let mut weights: Vec<u64> = Vec::new();
    let mut toc: Vec<TocItem> = Vec::new();
    /// Closes the chapter being built.
    fn finish(walker: &mut Fb2Walker, chapters: &mut Vec<Arc<Document>>, weights: &mut Vec<u64>) {
        let doc = std::mem::take(&mut walker.b).finish();
        weights.push(doc.plain_text().len().max(1) as u64);
        chapters.push(Arc::new(doc));
    }
    for body in root.children().filter(|c| c.is_element() && lname(*c) == "body") {
        let notes = body.attribute("name").is_some_and(|n| n.eq_ignore_ascii_case("notes"));
        let sections: Vec<Node> = body
            .children()
            .filter(|c| c.is_element() && lname(*c) == "section")
            .collect();
        if sections.is_empty() || notes {
            // One chapter: the body's own content (a notes body keeps all its sections in one run).
            let label = child(body, "title").map(title_label).unwrap_or_default();
            walker.blocks(body, 1, false);
            let ix = chapters.len();
            let children = walker.section_entries(body, ix);
            finish(&mut walker, &mut chapters, &mut weights);
            let fallback = if notes { t!("Notes").to_string() } else { title.clone() };
            toc.push(TocItem {
                label: if label.is_empty() { fallback } else { label },
                chapter: ix,
                anchor: None,
                children,
            });
            continue;
        }
        // The body's own title, epigraph and so on, before its first section, are a chapter of their own.
        let front: Vec<Node> = body
            .children()
            .filter(|c| c.is_element() && matches!(lname(*c), "title" | "epigraph" | "image" | "annotation"))
            .collect();
        if !front.is_empty() {
            let label = child(body, "title").map(title_label).unwrap_or_else(|| title.clone());
            for f in front {
                if let Some(id) = f.attribute("id") {
                    walker.b.anchor(id);
                }
                walker.blocks_of_one(f, 1);
            }
            let ix = chapters.len();
            finish(&mut walker, &mut chapters, &mut weights);
            if !label.is_empty() {
                toc.push(TocItem {
                    label,
                    chapter: ix,
                    anchor: None,
                    children: Vec::new(),
                });
            }
        }
        for section in sections {
            let ix = chapters.len();
            let label = child(section, "title").map(title_label).unwrap_or_default();
            if let Some(id) = section.attribute("id") {
                walker.b.anchor(id);
            }
            walker.blocks(section, 1, false);
            let children = walker.section_entries(section, ix);
            finish(&mut walker, &mut chapters, &mut weights);
            toc.push(TocItem {
                label: if label.is_empty() {
                    t!("Chapter {n}", n = ix + 1).to_string()
                } else {
                    label
                },
                chapter: ix,
                anchor: None,
                children,
            });
        }
    }
    if chapters.is_empty() {
        return Err(BookError::Unsupported);
    }
    let count = chapters.len();
    Ok(Book {
        title,
        rtl: rtl_language(&language),
        toc,
        count,
        weights,
        start: 0,
        cover,
        pages: false,
        source: Arc::new(Fb2Source { chapters }),
    })
}

impl Fb2Walker<'_, '_> {
    /// One child of a body as blocks (the body's own title, epigraph and so on).
    fn blocks_of_one(&mut self, n: Node, depth: u8) {
        match lname(n) {
            "title" => {
                let text = self.title_text(n);
                if !text.is_blank() {
                    self.b.push(Block::Heading {
                        level: depth.clamp(1, 5),
                        text,
                    });
                }
            }
            "image" => {
                let picture = self.picture(n);
                self.b.push(Block::Picture(picture));
            }
            "epigraph" => self.blocks(n, depth, true),
            "annotation" => {
                let style = Style {
                    italic: true,
                    ..Default::default()
                };
                for p in n.children().filter(|p| p.is_element() && lname(*p) == "p") {
                    self.paragraph(p, &style, Block::Paragraph);
                }
            }
            _ => {}
        }
    }
}

// ---- Comics ----------------------------------------------------------------------------------------------------

struct ComicSource {
    arch: Archive,
    pages: Vec<String>,
}

impl Source for ComicSource {
    fn chapter(&self, ix: usize) -> Result<Chapter, String> {
        let name = self.pages.get(ix).ok_or_else(|| "No such page".to_string())?;
        let bytes = self
            .arch
            .read(name, 64 * 1024 * 1024)
            .ok_or_else(|| t!("This page can't be read.").to_string())?;
        let format = rich::sniff_image(&bytes).ok_or_else(|| t!("This page can't be read.").to_string())?;
        Ok(Chapter::Page(Arc::new(Image::from_bytes(format, bytes))))
    }
}

/// Opens a comic book archive: its images in natural order.
pub fn open_cbz(path: &str) -> Result<Book, BookError> {
    let arch = Archive::open(path)?;
    let mut pages: Vec<String> = arch
        .names
        .iter()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            let file = lower.rsplit('/').next().unwrap_or("");
            !lower.starts_with("__macosx/")
                && !file.starts_with('.')
                && [".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp"]
                    .iter()
                    .any(|ext| file.ends_with(ext))
                && arch.has(name)
        })
        .cloned()
        .collect();
    pages.sort_by(|a, b| natural_cmp(a, b));
    if pages.is_empty() {
        return Err(BookError::Broken(t!("There are no pages in this comic.").to_string()));
    }
    let manga = arch
        .names
        .iter()
        .find(|n| n.eq_ignore_ascii_case("comicinfo.xml"))
        .and_then(|n| arch.read_text(n))
        .is_some_and(|text| {
            parse_xml(&text).is_some_and(|xml| {
                xml.descendants()
                    .find(|n| n.is_element() && lname(*n) == "Manga")
                    .is_some_and(|n| flat_text(n).eq_ignore_ascii_case("YesAndRightToLeft"))
            })
        });
    let count = pages.len();
    Ok(Book {
        title: String::new(),
        rtl: manga,
        toc: Vec::new(),
        count,
        weights: vec![1; count],
        start: 0,
        cover: None,
        pages: true,
        source: Arc::new(ComicSource { arch, pages }),
    })
}

// ---- Where you were, and the text size -------------------------------------------------------------------------

/// Where a book was left (`places()`): the chapter and the block at the top.
pub use crate::book_places::Place;

/// Where the book at `path` was left, in this run or an earlier one.
pub fn remembered(path: &str) -> Option<Place> {
    crate::book_places::shared().place(path)
}

/// Remembers where the book at `path` was left: the 200 most recent books are kept, and saved between runs.
pub fn remember(path: &str, place: Place) {
    crate::book_places::shared().remember(path, place);
}

/// The text size in percent (`startingSize`).
pub fn text_size() -> u32 {
    crate::book_places::shared().text_size()
}

/// Sets the text size, 70% to 200%.
pub fn set_text_size(percent: u32) {
    crate::book_places::shared().set_text_size(percent);
}

// ---- The previewer ---------------------------------------------------------------------------------------------

/// Opens a book from its path: [`open_book`] in the app, a fake in tests that don't want files.
pub type Opener = Arc<dyn Fn(&str) -> Result<Book, BookError> + Send + Sync>;

enum Load {
    Waiting,
    Failed(SharedString),
    Ready,
}

/// Where in a chapter to land.
#[derive(Clone, Debug)]
enum Goto {
    Start,
    End,
    Block(usize),
    Anchor(String),
}

/// An e-book in the preview: its pages, contents, text size and the place you stopped at.
pub struct BookView {
    path: String,
    opener: Opener,
    load: Load,
    started: Instant,
    generation: u64,
    book: Option<Arc<Book>>,
    rows: Vec<TocRow>,
    row_names: Vec<SharedString>,
    chapter: usize,
    wanted: u64,
    page: Option<Arc<Image>>,
    rich: Entity<RichView>,
    show_toc: bool,
    toc_cursor: usize,
    toc_scroll: UniformListScrollHandle,
    size: u32,
    focus: FocusHandle,
    _read: Task<()>,
    _chapter: Task<()>,
    _subscriptions: Vec<Subscription>,
    _release: Subscription,
}

/// Makes names unique (a contents with two "Notes" entries): the second is "Notes (2)".
fn unique(labels: impl Iterator<Item = String>) -> Vec<SharedString> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    labels
        .map(|label| {
            let label = label.trim().to_string();
            let count = seen.entry(label.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                format!("{label} ({count})").into()
            } else {
                label.into()
            }
        })
        .collect()
}

impl BookView {
    /// The view of the book at `path`. Opening happens in the background.
    pub fn new(path: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_opener(path, Arc::new(open_book), window, cx)
    }

    /// [`BookView::new`] opening through `opener`.
    pub fn with_opener(path: String, opener: Opener, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rich = cx.new(|cx| RichView::new(gpui_kit::rems(42.), t!("Book"), cx));
        let subscription = cx.subscribe(&rich, |this, _, event: &RichEvent, cx| match event {
            RichEvent::Link(href) => this.follow(href.as_ref(), cx),
            RichEvent::Scrolled => {
                this.remember_place(cx);
                cx.notify();
            }
        });
        // The comic page on screen is let go of when the view goes.
        let release = cx.on_release(|this, cx| {
            this.release_page(cx);
            this.release_cover(cx);
        });
        let mut this = Self {
            path,
            opener,
            load: Load::Waiting,
            started: Instant::now(),
            generation: 0,
            book: None,
            rows: Vec::new(),
            row_names: Vec::new(),
            chapter: 0,
            wanted: 0,
            page: None,
            rich,
            show_toc: false,
            toc_cursor: 0,
            toc_scroll: UniformListScrollHandle::new(),
            size: text_size(),
            focus: cx.focus_handle(),
            _read: Task::ready(()),
            _chapter: Task::ready(()),
            _subscriptions: vec![subscription],
            _release: release,
        };
        this.read(window, cx);
        this
    }

    /// The file shown.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows another book.
    pub fn set_path(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.path != path {
            self.path = path;
            self.read(window, cx);
        }
    }

    /// The book, once opened.
    pub fn book(&self) -> Option<&Book> {
        self.book.as_deref()
    }

    /// The chapter shown (or about to be).
    pub fn chapter(&self) -> usize {
        self.chapter
    }

    /// The rich view the chapter is drawn in.
    pub fn text_view(&self) -> &Entity<RichView> {
        &self.rich
    }

    /// The text size in percent.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// Whether the contents panel is open.
    pub fn contents_open(&self) -> bool {
        self.show_toc
    }

    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.release_page(cx);
        self.release_cover(cx);
        self.load = Load::Waiting;
        self.book = None;
        self.rows.clear();
        self.row_names.clear();
        self.show_toc = false;
        self.started = Instant::now();
        let (path, opener) = (self.path.clone(), self.opener.clone());
        self._read = cx.spawn_in(window, async move |this, cx| {
            let opened = cx.background_spawn(async move { opener(&path) }).await;
            this.update_in(cx, |this, _, cx| {
                if this.generation != generation {
                    return;
                }
                match opened {
                    Ok(book) => this.ready(Arc::new(book), cx),
                    Err(error) => this.load = Load::Failed(error.message()),
                }
                cx.notify();
            })
            .ok();
        });
        redraw_after_delay(cx);
        cx.notify();
    }

    fn ready(&mut self, book: Arc<Book>, cx: &mut Context<Self>) {
        self.rows = flatten(&book.toc);
        // "Go to …", so an entry is never named the same as the heading it leads to.
        self.row_names = unique(
            self.rows
                .iter()
                .map(|r| t!("Go to {name}", name = r.label.trim()).to_string()),
        );
        self.toc_cursor = 0;
        let place = remembered(&self.path).filter(|p| p.chapter < book.count);
        let (chapter, goto) = match place {
            Some(place) => (place.chapter, Goto::Block(place.block)),
            None => (book.start.min(book.count.saturating_sub(1)), Goto::Start),
        };
        self.book = Some(book);
        self.load = Load::Ready;
        let scale = self.size as f32 / 100.;
        self.rich.update(cx, |rich, cx| rich.set_scale(scale, cx));
        self.show_chapter(chapter, goto, cx);
    }

    /// Lets go of the cover beside the contents.
    fn release_cover(&self, cx: &mut App) {
        if let Some(cover) = self.book.as_ref().and_then(|book| book.cover.clone()) {
            cover.remove_asset(cx);
        }
    }

    /// Lets go of the comic page that was on screen.
    fn release_page(&mut self, cx: &mut App) {
        if let Some(page) = self.page.take() {
            page.remove_asset(cx);
        }
    }

    /// Reads chapter `ix` in the background and shows it, landing at `goto`.
    fn show_chapter(&mut self, ix: usize, goto: Goto, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            return;
        };
        if ix >= book.count {
            return;
        }
        self.chapter = ix;
        self.wanted += 1;
        let wanted = self.wanted;
        self._chapter = cx.spawn(async move |this, cx| {
            let reader = book.clone();
            let loaded = cx.background_spawn(async move { reader.chapter(ix) }).await;
            this.update(cx, |this, cx| {
                if this.wanted != wanted {
                    return;
                }
                let count = book.count;
                match loaded {
                    Ok(Chapter::Text(doc)) => {
                        this.release_page(cx);
                        let label = t!("Chapter {n} of {total}", n = ix + 1, total = count);
                        this.rich.update(cx, |rich, cx| {
                            rich.set_label(label, cx);
                            rich.set_document(doc, book.rtl, cx);
                            match &goto {
                                Goto::Start => {}
                                Goto::End => rich.scroll_to_end(cx),
                                Goto::Block(block) => rich.scroll_to_block(*block, cx),
                                Goto::Anchor(id) => {
                                    rich.scroll_to_anchor(id, cx);
                                }
                            }
                        });
                    }
                    Ok(Chapter::Page(image)) => {
                        this.release_page(cx);
                        this.page = Some(image);
                    }
                    Err(reason) => {
                        // A part that can't be read says so where it is, and the rest of the book still turns.
                        let mut text = Text::new(Dir::Auto);
                        text.push(&reason, &Style::default());
                        let mut builder = Builder::new();
                        builder.push(Block::Paragraph(text));
                        this.release_page(cx);
                        let doc = Arc::new(builder.finish());
                        this.rich.update(cx, |rich, cx| rich.set_document(doc, false, cx));
                    }
                }
                this.remember_place(cx);
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }

    fn remember_place(&self, cx: &App) {
        if self.book.is_none() {
            return;
        }
        let block = if self.is_pages() {
            0
        } else {
            self.rich.read(cx).top_block()
        };
        remember(
            &self.path,
            Place {
                chapter: self.chapter,
                block,
            },
        );
    }

    fn is_pages(&self) -> bool {
        self.book.as_ref().is_some_and(|b| b.pages)
    }

    /// The next page: a page down in the chapter, then the next chapter.
    pub fn next(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            return;
        };
        if !book.pages && self.rich.update(cx, |rich, cx| rich.page(true, cx)) {
            return;
        }
        if self.chapter + 1 < book.count {
            self.show_chapter(self.chapter + 1, Goto::Start, cx);
        }
    }

    /// The previous page: a page up in the chapter, then the end of the chapter before.
    pub fn prev(&mut self, cx: &mut Context<Self>) {
        let Some(book) = self.book.clone() else {
            return;
        };
        if !book.pages && self.rich.update(cx, |rich, cx| rich.page(false, cx)) {
            return;
        }
        if self.chapter > 0 {
            self.show_chapter(self.chapter - 1, Goto::End, cx);
        }
    }

    /// Page Down and Page Up in Quick Look. Returns whether anything moved.
    pub fn page(&mut self, down: bool, cx: &mut Context<Self>) -> bool {
        let before = (self.chapter, self.rich.read(cx).top_block());
        if down {
            self.next(cx);
        } else {
            self.prev(cx);
        }
        (self.chapter, self.rich.read(cx).top_block()) != before
    }

    /// Goes to a contents entry.
    pub fn go_to_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.rows.get(row).cloned() else {
            return;
        };
        self.toc_cursor = row;
        self.go(entry.chapter, entry.anchor, cx);
    }

    fn go(&mut self, chapter: usize, anchor: Option<String>, cx: &mut Context<Self>) {
        if chapter == self.chapter && !self.is_pages() {
            match anchor {
                Some(id) => {
                    self.rich.update(cx, |rich, cx| {
                        rich.scroll_to_anchor(&id, cx);
                    });
                }
                None => self.rich.update(cx, |rich, cx| rich.scroll_to_start(cx)),
            }
            self.remember_place(cx);
            cx.notify();
        } else {
            let goto = anchor.map_or(Goto::Start, Goto::Anchor);
            self.show_chapter(chapter, goto, cx);
        }
    }

    /// A click on a link in the text: the web opens outside, a place in the book is gone to.
    fn follow(&mut self, href: &str, cx: &mut Context<Self>) {
        if is_external(href) {
            cx.open_url(href);
            return;
        }
        let Some(book) = self.book.clone() else {
            return;
        };
        let (path, fragment) = split_href(href);
        let chapter = if path.is_empty() {
            match &fragment {
                Some(id) if !self.rich.read(cx).document().anchors.contains_key(id) => {
                    book.source.chapter_with_anchor(id)
                }
                _ => Some(self.chapter),
            }
        } else {
            book.source.locate(&path.to_lowercase())
        };
        if let Some(chapter) = chapter {
            self.go(chapter, fragment, cx);
        }
    }

    /// How far through the book you are, 0 to 1, by the chapters' sizes.
    pub fn fraction(&self, cx: &App) -> f32 {
        let Some(book) = &self.book else {
            return 0.;
        };
        if book.pages {
            return (self.chapter + 1) as f32 / book.count.max(1) as f32;
        }
        let total: u64 = book.weights.iter().sum();
        if total == 0 {
            return 0.;
        }
        let before: u64 = book.weights.iter().take(self.chapter).sum();
        let here = book.weights.get(self.chapter).copied().unwrap_or(0) as f32;
        ((before as f32 + here * self.rich.read(cx).fraction()) / total as f32).clamp(0., 1.)
    }

    /// The contents entry the reader is in (the last one at or before the top of the view).
    pub fn current_row(&self, cx: &App) -> Option<usize> {
        let top = self.rich.read(cx).top_block();
        let rich = self.rich.read(cx);
        let mut found = None;
        for (i, row) in self.rows.iter().enumerate() {
            let before = match row.chapter.cmp(&self.chapter) {
                Ordering::Less => true,
                Ordering::Greater => false,
                Ordering::Equal => match &row.anchor {
                    None => true,
                    Some(id) => rich.document().anchors.get(id).is_some_and(|block| *block <= top),
                },
            };
            if before {
                found = Some(i);
            }
        }
        found
    }

    /// The label shown in the bar: the section, else the book's title, else the file's name.
    pub fn heading(&self, cx: &App) -> String {
        if let Some(row) = self.current_row(cx).and_then(|i| self.rows.get(i)) {
            return row.label.trim().to_string();
        }
        match &self.book {
            Some(book) if !book.title.trim().is_empty() => book.title.clone(),
            _ => super::text::name_of(&self.path).to_string(),
        }
    }

    fn bump(&mut self, delta: i32, cx: &mut Context<Self>) {
        let size = (self.size as i32 + delta).clamp(70, 200) as u32;
        if size == self.size {
            return;
        }
        self.size = size;
        set_text_size(size);
        self.rich.update(cx, |rich, cx| rich.set_scale(size as f32 / 100., cx));
        cx.notify();
    }

    fn toggle_contents(&mut self, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
            return;
        }
        self.show_toc = !self.show_toc;
        if self.show_toc {
            self.toc_cursor = self.current_row(cx).unwrap_or(0);
        }
        cx.notify();
    }

    /// The keys of BookView.tsx: Left and Right turn the page toward the side the book's pages come from, Page Up
    /// and Page Down and Space turn it by the page, the rest scroll. With the contents open, Up and Down choose an
    /// entry, Enter goes to it and Escape closes the panel.
    pub fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        if keys.modifiers.control || keys.modifiers.alt || keys.modifiers.platform {
            return;
        }
        let key = keys.key.as_str();
        let Some(book) = self.book.clone() else {
            return;
        };
        if self.show_toc {
            let last = self.rows.len().saturating_sub(1);
            match key {
                "down" => self.toc_cursor = (self.toc_cursor + 1).min(last),
                "up" => self.toc_cursor = self.toc_cursor.saturating_sub(1),
                "enter" => {
                    let row = self.toc_cursor;
                    self.go_to_row(row, cx);
                }
                "escape" => self.show_toc = false,
                _ => return self.page_keys(key, keys.modifiers.shift, book.rtl, cx),
            }
            self.toc_scroll.scroll_to_item(self.toc_cursor, ScrollStrategy::Nearest);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.page_keys(key, keys.modifiers.shift, book.rtl, cx);
    }

    fn page_keys(&mut self, key: &str, shift: bool, rtl: bool, cx: &mut Context<Self>) {
        match key {
            // A right-to-left book comes from the right: its next page is the one to the left.
            "left" if rtl => self.next(cx),
            "left" => self.prev(cx),
            "right" if rtl => self.prev(cx),
            "right" => self.next(cx),
            "pageup" => self.prev(cx),
            "pagedown" => self.next(cx),
            "space" if shift => self.prev(cx),
            "space" => self.next(cx),
            "up" | "down" | "home" | "end" if !self.is_pages() => {
                self.rich.update(cx, |rich, cx| {
                    rich.key(key, cx);
                });
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    // ---- Drawing ---------------------------------------------------------------------------------------------

    fn bar(&self, book: &Book, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let heading = isolate(self.heading(cx));
        let percent = (self.fraction(cx) * 100.).round() as u32;
        let small = |id: &'static str, glyph: IconName, label: SharedString| {
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(glyph)
                .accessibility_label(label.clone())
                .tooltip(label)
        };
        row_start()
            .id("book-bar")
            .role(Role::Toolbar)
            .aria_label(t!("Reading controls"))
            .test_support()
            .flex_none()
            .gap(space(1.))
            .px(space(2.))
            .py(space(1.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(
                small("book-contents", IconName::ListTree, t!("Contents"))
                    .selected(self.show_toc)
                    .disabled(self.rows.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_contents(cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.muted_foreground)
                    .child(heading),
            )
            .child(
                div()
                    .id("book-percent")
                    .role(Role::Status)
                    .aria_label(t!("{n}% read", n = percent))
                    .test_support()
                    .flex_none()
                    .text_color(t.muted_foreground)
                    .child(format!("{percent}%")),
            )
            .children((!book.pages).then(|| {
                small("book-smaller", IconName::Minus, t!("Smaller text"))
                    .on_click(cx.listener(|this, _, _, cx| this.bump(-10, cx)))
            }))
            .children((!book.pages).then(|| {
                small("book-bigger", IconName::Plus, t!("Bigger text"))
                    .on_click(cx.listener(|this, _, _, cx| this.bump(10, cx)))
            }))
            .into_any_element()
    }

    /// The contents panel at the start edge.
    fn contents(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let current = self.current_row(cx);
        let cover = self.book.as_ref().and_then(|b| b.cover.clone()).map(|image| {
            div()
                .id("book-cover")
                .role(Role::Image)
                .aria_label(t!("Cover"))
                .test_support()
                .flex_none()
                .h(space(36.))
                .p(space(2.))
                .child(
                    img(ImageSource::Image(image))
                        .object_fit(ObjectFit::Contain)
                        .size_full(),
                )
        });
        let list = uniform_list(
            "book-toc",
            self.rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let t = theme::tokens(cx).clone();
                range
                    .filter_map(|ix| {
                        let row = this.rows.get(ix)?;
                        let name = this.row_names.get(ix).cloned().unwrap_or_default();
                        let active = current == Some(ix);
                        let cursor = this.toc_cursor == ix;
                        Some(
                            div()
                                .id(("book-toc-row", ix))
                                .role(Role::ListItem)
                                .aria_label(name)
                                .aria_selected(active)
                                .test_support()
                                .w_full()
                                .h(space(7.))
                                .flex()
                                .items_center()
                                .ps(2. + 3. * f32::from(row.depth))
                                .pe(2.)
                                .rounded(theme::size::RADIUS_SM)
                                .cursor_pointer()
                                .truncate()
                                .text_start()
                                .when(active, |d| d.bg(t.accent))
                                .when(cursor && this.show_toc, |d| d.border_1().border_color(t.ring))
                                .hover(|d| d.bg(t.accent))
                                .on_click(cx.listener(move |this, _, _, cx| this.go_to_row(ix, cx)))
                                .child(isolate(row.label.trim().to_string())),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.toc_scroll)
        .flex_1()
        .min_h_0();
        col()
            .id("book-contents-panel")
            .role(Role::Navigation)
            .aria_label(t!("Table of contents"))
            .test_support()
            .w(space(56.))
            .flex_none()
            .h_full()
            .min_h_0()
            .when(!crate::i18n::is_rtl(), |panel| panel.border_r_1())
            .when(crate::i18n::is_rtl(), |panel| panel.border_l_1())
            .border_color(t.border)
            .p(space(1.))
            .text_size(theme::size::XS)
            .children(cover)
            .child(list)
            .into_any_element()
    }
}

impl Focusable for BookView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BookView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("book-view")
            .size_full()
            .min_h_0()
            .text_start()
            .track_focus(&self.focus)
            .tab_stop(true)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus, cx)),
            )
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| {
                this.page(true, cx);
            }))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| {
                this.page(false, cx);
            }));
        let book = match (&self.load, self.book.clone()) {
            (Load::Waiting, _) => return root.child(waiting(self.started, Some(t!("Opening the book…")), cx)),
            (Load::Failed(reason), _) => return root.child(big_icon(&self.path, Some(reason.clone()), cx)),
            (Load::Ready, Some(book)) => book,
            (Load::Ready, None) => return root,
        };
        let rtl = book.rtl;
        let turn = |id: &'static str, glyph: IconName, label: SharedString| {
            Button::new(id)
                .ghost()
                .icon(glyph)
                .accessibility_label(label.clone())
                .tooltip(label)
                .flex_none()
                .w(space(7.))
                .h_full()
        };
        // The previous page is on the side the book comes from: the left, or the right for a right-to-left book.
        let (before, after) = if rtl {
            (IconName::ChevronRight, IconName::ChevronLeft)
        } else {
            (IconName::ChevronLeft, IconName::ChevronRight)
        };
        let prev = turn("book-prev", before, t!("Previous page")).on_click(cx.listener(|this, _, _, cx| this.prev(cx)));
        let next = turn("book-next", after, t!("Next page")).on_click(cx.listener(|this, _, _, cx| this.next(cx)));
        let content: AnyElement = match &self.page {
            Some(image) if book.pages => div()
                .id("book-page")
                .role(Role::Image)
                .aria_label(t!("Page {n} of {total}", n = self.chapter + 1, total = book.count))
                .test_support()
                .size_full()
                .p(space(2.))
                .child(
                    img(ImageSource::Image(image.clone()))
                        .object_fit(ObjectFit::Contain)
                        .size_full(),
                )
                .into_any_element(),
            _ if book.pages => div().size_full().into_any_element(),
            _ => self.rich.clone().into_any_element(),
        };
        let mut reader = row().flex_1().min_w_0().min_h_0().items_stretch();
        reader = if rtl { reader.flex_row_reverse() } else { reader };
        let reader = reader
            .child(prev)
            .child(div().flex_1().min_w_0().min_h_0().child(content))
            .child(next);
        let body = row_start()
            .relative()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .children(self.show_toc.then(|| self.contents(&t, cx)))
            .child(reader);
        root.child(self.bar(&book, &t, cx)).child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::io::Write as _;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering as AtomicOrdering;
    use std::time::Duration;

    /// A temp folder for the books a test writes, removed when it goes.
    struct Folder(std::path::PathBuf);

    impl Folder {
        fn new() -> Self {
            static COUNT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "glint-book-test-{}-{}",
                std::process::id(),
                COUNT.fetch_add(1, AtomicOrdering::Relaxed)
            ));
            std::fs::create_dir_all(&path).ok();
            Folder(path)
        }

        /// Writes a zip of `files` as `name` and returns its path.
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

        fn file(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).expect("a file");
            path.to_string_lossy().into_owned()
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

    /// An Arabic EPUB 3: right-to-left, a nav with a nested entry, three chapters, an image, a link between
    /// chapters, a table, a list and an undeclared entity.
    fn arabic_epub(folder: &Folder) -> String {
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>كتاب الاختبار</dc:title><dc:language>ar</dc:language>
    <meta name="cover" content="cover-img"/>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/c1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/c2.xhtml" media-type="application/xhtml+xml"/>
    <item id="c3" href="text/c3.xhtml" media-type="application/xhtml+xml"/>
    <item id="cover-img" href="images/cover.png" media-type="image/png" properties="cover-image"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine page-progression-direction="rtl"><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/></spine>
</package>"#;
        let nav = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body>
<nav epub:type="toc"><ol>
  <li><a href="text/c1.xhtml">الفصل الأول</a>
     <ol><li><a href="text/c1.xhtml#part">جزء فرعي</a></li></ol></li>
  <li><a href="text/c2.xhtml">الفصل الثاني</a></li>
  <li><a href="text/c3.xhtml">الفصل الثالث</a></li>
</ol></nav>
<nav epub:type="landmarks"><ol><li><a epub:type="bodymatter" href="text/c2.xhtml">Start</a></li></ol></nav>
</body></html>"#;
        let chapter = |title: &str, extra: &str| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title><link rel="stylesheet" href="../style.css"/></head>
<body><h1>{title}</h1>{extra}</body></html>"#
            )
        };
        // Enough text that a chapter is taller than the window and can be scrolled to a place in it.
        let filler: String = (0..40).map(|i| format!("<p>حشو {i}</p>")).collect();
        let c1 = chapter(
            "الفصل الأول",
            &format!("<p>بسم الله&nbsp;الرحمن <b>الرحيم</b></p><p id=\"part\">جزء فرعي في الفصل الأول، مع <a href=\"c2.xhtml#target\">رابط</a>.</p>{filler}"),
        );
        let c2 = chapter(
            "الفصل الثاني",
            &format!(
                "<ul><li>بند أول</li><li>بند ثان<ul><li>بند فرعي</li></ul></li></ul>\
                 <p>Hello &mdash; world</p><p id=\"target\">الهدف</p>\
                 <table><tr><th>الاسم</th><th>القيمة</th></tr><tr><td>أ</td><td>1</td></tr></table>\
                 <img src=\"../images/cover.png\" alt=\"صورة الغلاف\"/>{filler}"
            ),
        );
        let c3 = chapter(
            "الفصل الثالث",
            "<blockquote><p>اقتباس</p></blockquote><pre>a  b\n c</pre><p class=\"end\">النهاية</p>",
        );
        let css = ".end { text-align: center; font-weight: bold; }";
        folder.zip(
            "arabic.epub",
            &[
                ("mimetype", b"application/epub+zip"),
                ("META-INF/container.xml", CONTAINER.as_bytes()),
                ("OEBPS/content.opf", opf.as_bytes()),
                ("OEBPS/nav.xhtml", nav.as_bytes()),
                ("OEBPS/text/c1.xhtml", c1.as_bytes()),
                ("OEBPS/text/c2.xhtml", c2.as_bytes()),
                ("OEBPS/text/c3.xhtml", c3.as_bytes()),
                ("OEBPS/images/cover.png", rich::TEST_PNG),
                ("OEBPS/style.css", css.as_bytes()),
            ],
        )
    }

    fn paragraph_texts(doc: &Document) -> Vec<String> {
        doc.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(t) | Block::Heading { text: t, .. } | Block::Quote { text: t, .. } => Some(t.plain()),
                Block::Item { marker, text, .. } => Some(format!("{marker} {}", text.plain())),
                _ => None,
            })
            .collect()
    }

    // ---- The readers ----------------------------------------------------------------------------------------

    #[test]
    fn hrefs_resolve_against_their_folder() {
        assert_eq!(resolve("OEBPS/text", "../images/a%20b.png"), "OEBPS/images/a b.png");
        assert_eq!(resolve("OEBPS", "./c1.xhtml"), "OEBPS/c1.xhtml");
        assert_eq!(resolve("OEBPS", "/abs/x.html"), "abs/x.html");
        assert_eq!(resolve("", "a\\b.html"), "a/b.html");
        assert_eq!(resolve("a", "../../x"), "x");
        assert_eq!(dir_of("OEBPS/text/c1.xhtml"), "OEBPS/text");
        assert_eq!(dir_of("c1.xhtml"), "");
        assert_eq!(split_href("c.xhtml#a%20b"), ("c.xhtml", Some("a b".to_string())));
        assert_eq!(split_href("c.xhtml"), ("c.xhtml", None));
        assert_eq!(percent_decode("%D8%A3.html"), "أ.html");
        assert_eq!(percent_decode("100%"), "100%");
        assert!(has_scheme("https://x") && has_scheme("mailto:a@b") && has_scheme("javascript:alert(1)"));
        assert!(!has_scheme("c1.xhtml#x") && !has_scheme("a/b:c") && !has_scheme("#id"));
        assert!(is_external("HTTPS://x.org") && !is_external("javascript:x") && !is_external("c.xhtml"));
    }

    #[test]
    fn page_names_sort_the_way_people_count() {
        let mut names = vec!["p10.jpg", "p2.jpg", "P1.jpg", "p02a.jpg", "z.jpg", "p1b.jpg"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, ["P1.jpg", "p1b.jpg", "p2.jpg", "p02a.jpg", "p10.jpg", "z.jpg"]);
        assert_eq!(natural_cmp("a", "a"), Ordering::Equal);
        assert_eq!(natural_cmp("a1", "a"), Ordering::Greater);
    }

    #[test]
    fn xml_nested_too_deep_is_refused_before_it_can_overflow_the_stack() {
        let nest = |n: usize| {
            format!(
                "<?xml version='1.0'?><!-- <a><a> --><r>{}x{}</r>",
                "<a b=\">\">".repeat(n),
                "</a>".repeat(n)
            )
        };
        assert!(parse_xml(&nest(50)).is_some());
        assert!(
            parse_xml(&nest(MAX_XML_DEPTH)).is_none(),
            "one more than the limit with the root"
        );
        assert!(parse_xml(&nest(5000)).is_none());
        // Self-closing tags, comments and CDATA do not count.
        let flat = format!("<r>{}<![CDATA[<a><a><a>]]></r>", "<a/>".repeat(1000));
        assert!(parse_xml(&flat).is_some());
        assert!(shallow_enough("<!DOCTYPE html [ <!ENTITY x \"<y>\"> ]><html/>"));
    }

    #[test]
    fn html_entities_become_xml_and_stray_ampersands_are_escaped() {
        assert_eq!(fix_entities("a&nbsp;b &mdash; c"), "a&#160;b &#8212; c");
        assert_eq!(fix_entities("&amp; &lt; &#65; &#x41;"), "&amp; &lt; &#65; &#x41;");
        assert_eq!(fix_entities("AT&T and &unknown; x"), "AT&amp;T and &amp;unknown; x");
        assert_eq!(fix_entities("no entities"), "no entities");
        assert_eq!(entity_char("eacute"), Some('é'));
        assert_eq!(entity_char("yuml"), Some('ÿ'));
        assert_eq!(entity_char("hellip"), Some('…'));
        assert_eq!(entity_char("nope"), None);
    }

    #[test]
    fn a_small_css_gives_direction_alignment_and_weight_by_tag_class_and_id() {
        let mut css = Css::default();
        css.add(
            "/* c */ body { direction: rtl } p.note, .x { text-align: center; font-weight: 700 }\n\
             #id1 { font-style: italic !important } @media print { p { direction: ltr } } \
             div p { direction: ltr } a:hover { direction: ltr } .gone { display: none }",
        );
        let d = css.decls("body", None, None);
        assert_eq!(d.dir, Some(Dir::Rtl));
        let d = css.decls("p", Some("note other"), None);
        assert_eq!((d.align, d.bold, d.dir), (Some(Phys::Center), Some(true), None));
        assert_eq!(css.decls("span", Some("x"), None).align, Some(Phys::Center));
        assert_eq!(css.decls("p", Some("plain"), Some("id1")).italic, Some(true));
        assert!(css.decls("p", Some("gone"), None).hidden);
        // Combinators, pseudo classes and at-rules were not kept.
        assert_eq!(css.decls("p", None, None).dir, None);
        // Inline style reads the same way.
        let d = parse_decls("font-weight:bold; text-decoration: underline line-through; direction:RTL");
        assert_eq!(
            (d.bold, d.underline, d.strike, d.dir),
            (Some(true), Some(true), Some(true), Some(Dir::Rtl))
        );
        assert_eq!(parse_decls("font-weight: 400").bold, Some(false));
    }

    #[test]
    fn xhtml_chapters_map_to_blocks_with_direction_lists_tables_and_links() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml" dir="rtl"><body>
          <h2 id="top">عنوان</h2>
          <p>نص   عربي&nbsp;مع <em>تأكيد</em> و<a href="ch2.xhtml#x">رابط</a> وسطر<br/>جديد</p>
          <p dir="ltr">left to right</p>
          <ol start="3"><li>ثلاثة<ul><li>فرعي</li></ul></li><li>أربعة</li></ol>
          <blockquote><p>اقتباس</p><p>ثان</p></blockquote>
          <pre>x = 1
  y</pre>
          <table><caption>جدول</caption><tr><th>أ</th><th colspan="2">ب</th></tr><tr><td><p>1</p><p>2</p></td><td>3</td><td>4</td></tr></table>
          <hr/><script>alert(1)</script><style>p{}</style><p hidden="">no</p><p style="display:none">no</p>
          </body></html>"#;
        let doc = xhtml_document(page, "OEBPS/text/ch1.xhtml", None);
        let texts = paragraph_texts(&doc);
        assert_eq!(texts[0], "عنوان");
        assert_eq!(texts[1], "نص عربي\u{a0}مع تأكيد ورابط وسطر\nجديد");
        assert_eq!(texts[2], "left to right");
        assert_eq!(texts[3], "3. ثلاثة");
        assert_eq!(texts[4], "◦ فرعي");
        assert_eq!(texts[5], "4. أربعة");
        assert_eq!(texts[6], "اقتباس");
        assert_eq!(texts[7], "ثان");
        assert!(texts.iter().all(|t| t != "no"));
        assert!(matches!(&doc.blocks[0], Block::Heading { level: 2, .. }));
        assert_eq!(doc.anchors["top"], 0);
        // The paragraph is right to left by the document's dir; the one that says ltr is not.
        let Block::Paragraph(text) = &doc.blocks[1] else {
            panic!("a paragraph")
        };
        assert!(text.is_rtl(false) && text.dir == Dir::Rtl);
        let link = text.spans.iter().find_map(|s| s.style.link.clone()).expect("a link");
        assert_eq!(&*link, "OEBPS/text/ch2.xhtml#x");
        assert!(text.spans.iter().any(|s| s.style.italic && s.text == "تأكيد"));
        let Block::Paragraph(ltr) = &doc.blocks[2] else {
            panic!("a paragraph")
        };
        assert_eq!(ltr.dir, Dir::Ltr);
        assert!(matches!(&doc.blocks[4], Block::Item { depth: 1, .. }));
        assert!(matches!(&doc.blocks[6], Block::Quote { depth: 1, .. }));
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Code(c) if c == "x = 1\n  y")));
        let table = doc
            .blocks
            .iter()
            .find_map(|b| if let Block::Table(t) = b { Some(t) } else { None })
            .expect("a table");
        assert!(table.rtl && table.rows.len() == 2 && table.rows[0].header);
        assert_eq!(table.rows[0].cells[1].span, 2);
        assert_eq!(table.rows[1].cells[0].blocks.len(), 2);
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Rule)));
        assert!(doc
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Paragraph(t) if t.plain() == "جدول")));
    }

    #[test]
    fn a_chapter_that_is_not_well_formed_still_reads_as_text() {
        let doc = xhtml_document(
            "<html><body><p>one &amp; two<br><p>three &eacute; &#x41;<script>x</script></p> <div>four</div></body>",
            "c.html",
            None,
        );
        let text = paragraph_texts(&doc).join("|");
        assert_eq!(text, "one & two|three é A|four");
    }

    #[test]
    fn an_epub_opens_with_its_contents_direction_start_and_cover() {
        let folder = Folder::new();
        let path = arabic_epub(&folder);
        let book = open_epub(&path).expect("an epub");
        assert_eq!(book.title, "كتاب الاختبار");
        assert!(book.rtl, "page-progression-direction rtl");
        assert_eq!(book.count, 3);
        assert_eq!(book.start, 1, "the bodymatter landmark");
        assert!(book.cover.is_some());
        let rows = flatten(&book.toc);
        assert_eq!(
            rows.iter()
                .map(|r| (r.depth, r.label.as_str(), r.chapter, r.anchor.as_deref()))
                .collect::<Vec<_>>(),
            [
                (0, "الفصل الأول", 0, None),
                (1, "جزء فرعي", 0, Some("part")),
                (0, "الفصل الثاني", 1, None),
                (0, "الفصل الثالث", 2, None)
            ]
        );
        assert!(book.weights.iter().all(|w| *w > 0));
        let Ok(Chapter::Text(c1)) = book.chapter(0) else {
            panic!("chapter 1")
        };
        assert_eq!(paragraph_texts(&c1)[1], "بسم الله\u{a0}الرحمن الرحيم");
        assert_eq!(c1.anchors["part"], 2);
        let Ok(Chapter::Text(c2)) = book.chapter(1) else {
            panic!("chapter 2")
        };
        // The link in chapter 1 goes to chapter 2's id, as a path in the zip.
        let Block::Paragraph(linked) = &c1.blocks[2] else {
            panic!("a paragraph")
        };
        let href = linked.spans.iter().find_map(|s| s.style.link.clone()).expect("a link");
        assert_eq!(&*href, "OEBPS/text/c2.xhtml#target");
        assert!(c2.anchors.contains_key("target"));
        assert!(paragraph_texts(&c2).contains(&"Hello — world".to_string()));
        // The picture was read out of the zip, beside its alt text.
        let picture = c2
            .blocks
            .iter()
            .find_map(|b| if let Block::Picture(p) = b { Some(p) } else { None });
        let picture = picture.expect("a picture");
        assert!(picture.image.is_some() && picture.alt == "صورة الغلاف");
        // The style sheet's rule on a class reached the paragraph.
        let Ok(Chapter::Text(c3)) = book.chapter(2) else {
            panic!("chapter 3")
        };
        let end = c3.blocks.iter().find_map(|b| match b {
            Block::Paragraph(t) if t.plain() == "النهاية" => Some(t.clone()),
            _ => None,
        });
        let end = end.expect("the last paragraph");
        assert_eq!(end.align, Align::Center);
        assert!(end.spans[0].style.bold);
        assert!(book.chapter(3).is_err());
    }

    #[test]
    fn an_epub2_with_an_ncx_and_a_guide_opens_too() {
        let folder = Folder::new();
        let opf = r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="x"><dc:title>Old book</dc:title><dc:language>en</dc:language></metadata>
          <manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
          <item id="a" href="a.html" media-type="application/xhtml+xml"/><item id="b" href="b.html" media-type="application/xhtml+xml"/></manifest>
          <spine toc="ncx"><itemref idref="a"/><itemref idref="b"/></spine>
          <guide><reference type="text" title="Start" href="b.html"/></guide></package>"#;
        let ncx = r#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap>
          <navPoint id="1"><navLabel><text>First</text></navLabel><content src="a.html"/>
            <navPoint id="2"><navLabel><text>Second</text></navLabel><content src="b.html#s"/></navPoint></navPoint></navMap></ncx>"#;
        let path = folder.zip(
            "old.epub",
            &[
                (
                    "META-INF/container.xml",
                    CONTAINER.replace("OEBPS/content.opf", "content.opf").as_bytes(),
                ),
                ("content.opf", opf.as_bytes()),
                ("toc.ncx", ncx.as_bytes()),
                ("a.html", b"<html><body><p>A</p></body></html>"),
                ("b.html", b"<html><body><p id=\"s\">B</p></body></html>"),
            ],
        );
        let book = open_epub(&path).expect("an epub 2");
        assert_eq!(book.title, "Old book");
        assert!(!book.rtl);
        assert_eq!(book.start, 1, "the guide's text reference");
        let rows = flatten(&book.toc);
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[1].depth, rows[1].chapter, rows[1].anchor.as_deref()),
            (1, 1, Some("s"))
        );
        assert!(book.cover.is_none());
    }

    #[test]
    fn what_is_not_a_book_or_is_locked_says_so() {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let folder = Folder::new();
        let not_zip = folder.file("bad.epub", b"this is not a zip");
        assert_eq!(open_book(&not_zip).err(), Some(BookError::Unsupported));
        let empty = folder.zip("empty.epub", &[("a.txt", b"x")]);
        assert_eq!(open_book(&empty).err(), Some(BookError::Unsupported));
        let drm = folder.zip(
            "drm.epub",
            &[
                ("META-INF/container.xml", CONTAINER.as_bytes()),
                (
                    "META-INF/encryption.xml",
                    br#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/></enc:EncryptedData></encryption>"#,
                ),
            ],
        );
        assert_eq!(open_book(&drm).err(), Some(BookError::Drm));
        // Font obfuscation is not DRM.
        let fonts = folder.zip(
            "fonts.epub",
            &[
                ("META-INF/container.xml", CONTAINER.as_bytes()),
                (
                    "META-INF/encryption.xml",
                    br#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData><enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/></enc:EncryptedData></encryption>"#,
                ),
            ],
        );
        // It has no package, so it is unsupported, not locked.
        assert_eq!(open_book(&fonts).err(), Some(BookError::Unsupported));
        assert_eq!(open_book("C:\\no\\such\\book.mobi").err(), Some(BookError::Unsupported));
        assert!(matches!(
            open_book("C:\\no\\such\\book.epub").err(),
            Some(BookError::Broken(_))
        ));
        assert!(can_read("epub") && can_read("cbz") && !can_read("mobi") && !can_read("azw3"));
        assert_eq!(BookError::Unsupported.message(), "Glint can't read this kind of book.");
        assert_eq!(
            BookError::Drm.message(),
            "This book has DRM, which only its store's app can open."
        );
    }

    const FB2: &str = r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
 <description><title-info><book-title>رواية</book-title><lang>ar</lang><coverpage><image l:href="#cover.png"/></coverpage></title-info></description>
 <body>
  <title><p>رواية</p></title>
  <section id="s1"><title><p>الفصل</p><p>الأول</p></title>
    <p>نص <strong>غامق</strong> و<emphasis>مائل</emphasis> مع حاشية<a l:href="#n1" type="note">[1]</a></p>
    <poem><stanza><v>سطر أول</v><v>سطر ثان</v></stanza></poem>
    <section id="s1a"><title><p>قسم فرعي</p></title><p>داخل</p><image l:href="#cover.png"/></section>
  </section>
  <section><title><p>Second</p></title><p>English text</p><cite><p>quoted</p><text-author>Someone</text-author></cite></section>
 </body>
 <body name="notes"><section id="n1"><title><p>1</p></title><p>نص الحاشية</p></section></body>
 <binary id="cover.png" content-type="image/png">iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==</binary>
</FictionBook>"##;

    #[test]
    fn an_fb2_opens_with_chapters_notes_and_pictures() {
        let book = open_fb2(FB2.as_bytes()).expect("an fb2");
        assert_eq!(book.title, "رواية");
        assert!(book.rtl);
        assert!(book.cover.is_some());
        // The body's title, two sections, and the notes.
        assert_eq!(book.count, 4);
        let rows = flatten(&book.toc);
        let labels: Vec<_> = rows.iter().map(|r| (r.depth, r.label.as_str())).collect();
        assert_eq!(
            labels,
            [
                (0, "رواية"),
                (0, "الفصل الأول"),
                (1, "قسم فرعي"),
                (0, "Second"),
                (0, "Notes"),
                (1, "1")
            ]
        );
        let Ok(Chapter::Text(one)) = book.chapter(1) else {
            panic!("chapter")
        };
        let texts = paragraph_texts(&one);
        assert_eq!(texts[0], "الفصل الأول", "a title's paragraphs are joined");
        assert!(texts[1].starts_with("نص غامق ومائل مع حاشية"));
        assert!(texts.contains(&"سطر أول\nسطر ثان".to_string()), "{texts:?}");
        assert!(one
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Picture(p) if p.image.is_some())));
        assert!(one.anchors.contains_key("s1a"));
        // The note link names an id in the notes body, which is another chapter.
        let Block::Paragraph(note) = &one.blocks[1] else {
            panic!("a paragraph")
        };
        let link = note
            .spans
            .iter()
            .find_map(|s| s.style.link.clone())
            .expect("a note link");
        assert_eq!(&*link, "#n1");
        assert_eq!(book.source.chapter_with_anchor("n1"), Some(3));
        assert_eq!(book.source.chapter_with_anchor("nowhere"), None);
        let Ok(Chapter::Text(two)) = book.chapter(2) else {
            panic!("chapter")
        };
        assert!(two.blocks.iter().any(|b| matches!(b, Block::Quote { .. })));
        assert!(two
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Paragraph(t) if t.plain() == "Someone" && t.align == Align::End)));
        assert!(matches!(open_fb2(b"<other/>"), Err(BookError::Unsupported)));
        assert!(matches!(open_fb2(b"not xml"), Err(BookError::Unsupported)));
    }

    #[test]
    fn an_fb2_in_an_older_encoding_is_decoded_by_its_declaration() {
        // "привет" in windows-1251.
        let mut bytes =
            br#"<?xml version="1.0" encoding="windows-1251"?><FictionBook><description><title-info><book-title>"#
                .to_vec();
        bytes.extend_from_slice(&[0xef, 0xf0, 0xe8, 0xe2, 0xe5, 0xf2]);
        bytes.extend_from_slice(b"</book-title><lang>ru</lang></title-info></description><body><section><p>x</p></section></body></FictionBook>");
        let book = open_fb2(&bytes).expect("an fb2");
        assert_eq!(book.title, "привет");
        assert!(!book.rtl);
    }

    #[test]
    fn a_zipped_fb2_and_a_comic_open() {
        let folder = Folder::new();
        let fbz = folder.zip("book.fbz", &[("book.fb2", FB2.as_bytes())]);
        assert_eq!(open_book(&fbz).expect("an fbz").count, 4);
        let png = rich::TEST_PNG;
        let cbz = folder.zip(
            "comic.cbz",
            &[
                ("page10.png", png),
                ("page2.png", png),
                ("__MACOSX/page1.png", png),
                ("page1.png", png),
                ("notes.txt", b"x"),
                (".hidden.png", png),
                (
                    "ComicInfo.xml",
                    b"<ComicInfo><Manga>YesAndRightToLeft</Manga></ComicInfo>",
                ),
            ],
        );
        let book = open_book(&cbz).expect("a comic");
        assert!(book.pages && book.rtl);
        assert_eq!(book.count, 3);
        assert_eq!(book.title, "comic.cbz", "an untitled book is named by its file");
        assert!(book.toc.is_empty());
        assert!(matches!(book.chapter(0), Ok(Chapter::Page(_))));
        assert!(book.chapter(3).is_err());
        let none = folder.zip("none.cbz", &[("a.txt", b"x")]);
        assert!(matches!(open_book(&none), Err(BookError::Broken(_))));
        let broken = folder.zip("broken.cbz", &[("p1.png", b"not a picture")]);
        let book = open_book(&broken).expect("a comic with a bad page");
        assert!(book.chapter(0).is_err());
    }

    #[test]
    fn places_and_the_text_size_are_remembered() {
        let place = Place { chapter: 2, block: 7 };
        remember("C:\\Books\\A.epub", place);
        assert_eq!(remembered("c:\\books\\a.epub"), Some(place));
        assert_eq!(remembered("c:\\books\\other.epub"), None);
        remember("C:\\Books\\A.epub", Place { chapter: 3, block: 0 });
        assert_eq!(remembered("c:\\books\\a.epub").map(|p| p.chapter), Some(3));
        for i in 0..250 {
            remember(&format!("C:\\many\\{i}.epub"), place);
        }
        assert_eq!(remembered("c:\\books\\a.epub"), None, "only the last 200 are kept");
        assert_eq!(remembered("c:\\many\\249.epub"), Some(place));
        set_text_size(500);
        assert_eq!(text_size(), 200);
        set_text_size(10);
        assert_eq!(text_size(), 70);
        set_text_size(100);
    }

    #[test]
    fn the_icons_the_book_view_draws_are_in_the_embedded_assets() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::ListTree,
            IconName::Minus,
            IconName::Plus,
            IconName::ChevronLeft,
            IconName::ChevronRight,
            IconName::Image,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    // ---- The view -------------------------------------------------------------------------------------------

    fn open(cx: &mut TestAppContext, path: String) -> (Entity<BookView>, VisualTestContext) {
        cx.update(testing::init);
        let (visual, view) = testing::open_window(cx, move |window, cx| cx.new(|cx| BookView::new(path, window, cx)));
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
        panic!("the book never arrived");
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

    /// Waits until chapter `ix` is the one drawn, starting with `first_block`'s text.
    fn on_chapter(view: &Entity<BookView>, visual: &mut VisualTestContext, ix: usize, first_block: &str) {
        for _ in 0..600 {
            visual.run_until_parked();
            let done = view.read_with(visual, |v, cx| {
                v.chapter() == ix && v.text_view().read(cx).document().plain_text().starts_with(first_block)
            });
            if done {
                frames(visual);
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let seen = view.read_with(visual, |v, cx| {
            format!(
                "chapter {} book {} load {} text {:?}",
                v.chapter(),
                v.book().is_some(),
                match &v.load {
                    Load::Waiting => "waiting".to_string(),
                    Load::Failed(why) => format!("failed: {why}"),
                    Load::Ready => "ready".to_string(),
                },
                v.text_view()
                    .read(cx)
                    .document()
                    .plain_text()
                    .chars()
                    .take(40)
                    .collect::<String>()
            )
        });
        panic!("chapter {ix} starting {first_block:?} never showed: {seen}");
    }

    /// Presses a key in the book, as the window would deliver it.
    fn press(visual: &mut VisualTestContext, view: &Entity<BookView>, key: &'static str) {
        let event = KeyDownEvent {
            keystroke: gpui_kit::Keystroke::parse(key).expect("a key"),
            is_held: false,
            prefer_character_input: false,
        };
        visual.update(|window, cx| view.update(cx, |v, cx| v.on_key_down(&event, window, cx)));
        frames(visual);
    }

    /// Presses `key` until the book is on chapter `ix`: a chapter taller than the window takes a few pages to get
    /// through before the next one starts.
    fn press_until(visual: &mut VisualTestContext, view: &Entity<BookView>, key: &'static str, ix: usize) {
        for _ in 0..60 {
            if view.read_with(visual, |v, _| v.chapter()) == ix {
                return;
            }
            press(visual, view, key);
            visual.run_until_parked();
        }
        panic!("pressing {key} never reached chapter {ix}");
    }

    #[gpui_kit::test]
    fn an_arabic_book_opens_where_its_text_starts_and_turns_the_other_way(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        set_text_size(100);
        let folder = Folder::new();
        let (view, mut visual) = open(cx, arabic_epub(&folder));
        on_chapter(&view, &mut visual, 1, "الفصل الثاني");
        // The bodymatter landmark is chapter 2; its heading, list, quote and picture are drawn, and named.
        for label in ["الفصل الثاني", "بند أول", "بند فرعي", "الهدف", "صورة الغلاف", "Table"]
        {
            assert!(has(&mut visual, label), "{label}");
        }
        assert!(has(&mut visual, &t!("Chapter {n} of {total}", n = 2, total = 3)));
        for label in [
            "Reading controls",
            "Contents",
            "Smaller text",
            "Bigger text",
            "Previous page",
            "Next page",
        ] {
            assert!(has(&mut visual, label), "{label}");
        }
        assert_eq!(view.read_with(&visual, |v, cx| v.heading(cx)), "الفصل الثاني");
        visual.update(|window, _| {
            assert_eq!(window.find_by_label("بند أول").role(), Some(Role::ListItem));
            assert_eq!(window.find_by_label("الفصل الثاني").role(), Some(Role::Heading));
        });
        // The previous page is on the side the book comes from: the right.
        visual.update(|window, _| {
            let (prev, next) = (window.find("book-prev").bounds(), window.find("book-next").bounds());
            assert!(prev.left() > next.left(), "{prev:?} {next:?}");
        });
        // A right-to-left book: the key toward the left turns forward, the one toward the right turns back.
        press_until(&mut visual, &view, "left", 2);
        on_chapter(&view, &mut visual, 2, "الفصل الثالث");
        assert!(has(&mut visual, "اقتباس") && has(&mut visual, "النهاية"));
        press(&mut visual, &view, "right");
        on_chapter(&view, &mut visual, 1, "الفصل الثاني");
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn pages_turn_through_chapters_and_the_place_is_remembered(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        set_text_size(100);
        let folder = Folder::new();
        let path = arabic_epub(&folder);
        remember(&path, Place { chapter: 0, block: 0 });
        let (view, mut visual) = open(cx, path.clone());
        on_chapter(&view, &mut visual, 0, "الفصل الأول");
        // A page forward scrolls the chapter; at its end the next chapter starts.
        press(&mut visual, &view, "pagedown");
        assert_eq!(view.read_with(&visual, |v, _| v.chapter()), 0);
        assert!(view.read_with(&visual, |v, cx| v.text_view().read(cx).top_block() > 0));
        press_until(&mut visual, &view, "pagedown", 1);
        on_chapter(&view, &mut visual, 1, "الفصل الثاني");
        // Back lands at the end of the chapter before.
        press(&mut visual, &view, "pageup");
        on_chapter(&view, &mut visual, 0, "الفصل الأول");
        assert!(view.read_with(&visual, |v, cx| v.text_view().read(cx).at_end()));
        press_until(&mut visual, &view, "space", 1);
        on_chapter(&view, &mut visual, 1, "الفصل الثاني");
        // The place was written for the next time the book is opened, and the book opens there.
        assert_eq!(remembered(&path).map(|p| p.chapter), Some(1));
        let (again, mut visual2) = open(cx, path);
        on_chapter(&again, &mut visual2, 1, "الفصل الثاني");
        let fraction = again.read_with(&visual2, |v, cx| v.fraction(cx));
        assert!(
            fraction > 0. && fraction < 1.,
            "progress follows the chapters' sizes: {fraction}"
        );
        // This book turns from the right: Left is forward. The last chapter has no chapter after it.
        press_until(&mut visual2, &again, "left", 2);
        on_chapter(&again, &mut visual2, 2, "الفصل الثالث");
        press(&mut visual2, &again, "left");
        frames(&mut visual2);
        assert_eq!(again.read_with(&visual2, |v, _| v.chapter()), 2);
    }

    #[gpui_kit::test]
    fn the_contents_open_by_button_and_by_keys_and_go_to_their_entries(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        set_text_size(100);
        let folder = Folder::new();
        let path = arabic_epub(&folder);
        remember(&path, Place { chapter: 1, block: 0 });
        let (view, mut visual) = open(cx, path);
        on_chapter(&view, &mut visual, 1, "الفصل الثاني");
        assert!(!has(&mut visual, "Table of contents"));
        visual.update(|window, cx| window.click("book-contents", cx));
        frames(&mut visual);
        assert!(has(&mut visual, "Table of contents"));
        assert!(has(&mut visual, "Cover"), "the small cover is beside the contents");
        for name in ["الفصل الأول", "جزء فرعي", "الفصل الثاني", "الفصل الثالث"] {
            assert!(has(&mut visual, &t!("Go to {name}", name = name)), "{name}");
        }
        // The entry of the chapter being read is the selected one.
        let current = t!("Go to {name}", name = "الفصل الثاني").to_string();
        visual.update(|window, _| assert_eq!(window.find_by_label(&current).selected(), Some(true)));
        // A click goes to the entry; the one with an id scrolls to it inside its chapter.
        visual.update(|window, cx| window.click(("book-toc-row", 1usize), cx));
        on_chapter(&view, &mut visual, 0, "الفصل الأول");
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, cx| v.text_view().read(cx).top_block() == 2)
        });
        assert_eq!(view.read_with(&visual, |v, cx| v.heading(cx)), "جزء فرعي");
        // The keys: Down chooses the next entry, Enter goes to it.
        press(&mut visual, &view, "down");
        press(&mut visual, &view, "enter");
        on_chapter(&view, &mut visual, 1, "الفصل الثاني");
        // Escape closes the panel.
        assert!(view.read_with(&visual, |v, _| v.contents_open()));
        press(&mut visual, &view, "escape");
        assert!(!has(&mut visual, "Table of contents"));
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_link_in_the_text_goes_to_its_place_in_another_chapter(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        set_text_size(100);
        let folder = Folder::new();
        let path = arabic_epub(&folder);
        remember(&path, Place { chapter: 0, block: 0 });
        let (view, mut visual) = open(cx, path);
        on_chapter(&view, &mut visual, 0, "الفصل الأول");
        view.update(&mut visual, |v, cx| v.follow("OEBPS/text/c2.xhtml#target", cx));
        on_chapter(&view, &mut visual, 1, "الفصل الثاني");
        settle(&mut visual, |cx| {
            view.read_with(cx, |v, cx| {
                let rich = v.text_view().read(cx);
                rich.document()
                    .anchors
                    .get("target")
                    .is_some_and(|b| *b == rich.top_block())
            })
        });
        // A link to nowhere does nothing.
        view.update(&mut visual, |v, cx| v.follow("OEBPS/text/missing.xhtml", cx));
        frames(&mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.chapter()), 1);
    }

    #[gpui_kit::test]
    fn the_text_size_buttons_step_by_ten_between_70_and_200(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        set_text_size(100);
        let folder = Folder::new();
        let path = arabic_epub(&folder);
        remember(&path, Place { chapter: 0, block: 0 });
        let (view, mut visual) = open(cx, path);
        on_chapter(&view, &mut visual, 0, "الفصل الأول");
        visual.update(|window, cx| window.click("book-bigger", cx));
        assert_eq!(view.read_with(&visual, |v, _| v.size()), 110);
        assert_eq!(text_size(), 110);
        for _ in 0..12 {
            visual.update(|window, cx| window.click("book-bigger", cx));
        }
        assert_eq!(view.read_with(&visual, |v, _| v.size()), 200);
        for _ in 0..20 {
            visual.update(|window, cx| window.click("book-smaller", cx));
        }
        assert_eq!(view.read_with(&visual, |v, _| v.size()), 70);
        set_text_size(100);
    }

    #[gpui_kit::test]
    fn a_comic_shows_a_page_at_a_time(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let folder = Folder::new();
        let png = rich::TEST_PNG;
        let path = folder.zip("c.cbz", &[("1.png", png), ("2.png", png), ("3.png", png)]);
        let (view, mut visual) = open(cx, path);
        settle(&mut visual, |cx| has(cx, "Page 1 of 3"));
        assert!(!has(&mut visual, "Smaller text"), "there is no text to resize");
        visual.update(|window, _| {
            let (prev, next) = (window.find("book-prev").bounds(), window.find("book-next").bounds());
            assert!(prev.left() < next.left(), "{prev:?} {next:?}");
        });
        press(&mut visual, &view, "right");
        settle(&mut visual, |cx| has(cx, "Page 2 of 3"));
        press(&mut visual, &view, "pagedown");
        settle(&mut visual, |cx| has(cx, "Page 3 of 3"));
        // The last page stays.
        press(&mut visual, &view, "pagedown");
        frames(&mut visual);
        assert!(has(&mut visual, "Page 3 of 3"));
        assert!((view.read_with(&visual, |v, cx| v.fraction(cx)) - 1.).abs() < 1e-6);
        press(&mut visual, &view, "left");
        settle(&mut visual, |cx| has(cx, "Page 2 of 3"));
        visual.update(|window, cx| window.click("book-prev", cx));
        settle(&mut visual, |cx| has(cx, "Page 1 of 3"));
    }

    #[gpui_kit::test]
    fn a_manga_turns_from_the_right(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(false);
        let folder = Folder::new();
        let png = rich::TEST_PNG;
        let path = folder.zip(
            "m.cbz",
            &[
                ("1.png", png),
                ("2.png", png),
                (
                    "ComicInfo.xml",
                    b"<ComicInfo><Manga>YesAndRightToLeft</Manga></ComicInfo>",
                ),
            ],
        );
        let (view, mut visual) = open(cx, path);
        settle(&mut visual, |cx| has(cx, "Page 1 of 2"));
        // The next page is the one toward the left.
        press(&mut visual, &view, "right");
        frames(&mut visual);
        assert!(has(&mut visual, "Page 1 of 2"));
        press(&mut visual, &view, "left");
        settle(&mut visual, |cx| has(cx, "Page 2 of 2"));
    }

    #[gpui_kit::test]
    fn a_book_that_cannot_be_read_says_why(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let folder = Folder::new();
        let bad = folder.file("bad.epub", b"not a zip");
        let (_view, mut visual) = open(cx, bad);
        settle(&mut visual, |cx| has(cx, "Glint can't read this kind of book."));
        let (_view, mut visual) = open(cx, folder.0.join("missing.fb2").to_string_lossy().into_owned());
        settle(&mut visual, |cx| {
            cx.update(|window, _| {
                window
                    .find_all_by_role(Role::Group)
                    .iter()
                    .any(|g| g.label().is_some_and(|l| !l.is_empty()))
            })
        });
        testing::set_rtl(false);
    }
}
