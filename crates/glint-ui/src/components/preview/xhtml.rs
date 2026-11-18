//! What reading a Word document or an HTML mail's XHTML needs: a zip opened once ([`Archive`]), XML read
//! without tripping on HTML's entities or on nesting deep enough to overflow the stack, and the XHTML-to-blocks
//! conversion ([`xhtml_document`]) with a small CSS reading. These began in the e-book reader, which is gone.

use super::markdown::base64_decode;
use super::rich::{
    self, picture_of, Align, Block, Builder, Cell, Dir, Document, Picture, Row, Script, Style, Table, Text,
};
use crate::kit::prelude::*;
use roxmltree::{Document as Xml, Node, ParsingOptions};
use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, Mutex};

/// The most of one file read out of a book (a chapter's XHTML, a stylesheet).
const TEXT_LIMIT: u64 = 24 * 1024 * 1024;
/// The most of one picture read out of a book.
const IMAGE_LIMIT: u64 = 24 * 1024 * 1024;
/// The most picture bytes one chapter keeps; the rest show their alt text.
const CHAPTER_PICTURES: usize = 96 * 1024 * 1024;

const OPS_NS: &str = "http://www.idpf.org/2007/ops";

// ---- Why a zip does not open -----------------------------------------------------------------------------------

/// Why a zip could not be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZipError {
    /// Not a zip at all.
    NotAZip,
    /// The file could not be opened, with the reason as Windows gave it.
    Broken(String),
}

// ---- Paths in a zip -------------------------------------------------------------------------------------------

/// `%20` and its kin decoded (hrefs in a document are percent-encoded; names in its zip are not).
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

// ---- A zip -----------------------------------------------------------------------------------------------------

/// A zip (a book, a Word document), opened once and read an entry at a time.
pub(super) struct Archive {
    zip: Mutex<zip::ZipArchive<std::fs::File>>,
    /// Lowercase normalized name to entry.
    index: HashMap<String, usize>,
    /// Uncompressed sizes, by entry.
    sizes: Vec<u64>,
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/").trim_start_matches('/').to_string()
}

impl Archive {
    pub(super) fn open(path: &str) -> Result<Self, ZipError> {
        let file = std::fs::File::open(path).map_err(|e| ZipError::Broken(e.to_string()))?;
        let mut zip = zip::ZipArchive::new(file).map_err(|_| ZipError::NotAZip)?;
        let mut index = HashMap::new();
        let mut sizes = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            match zip.by_index_raw(i) {
                Ok(entry) => {
                    let name = normalize(&entry.name().map(|n| n.into_owned()).unwrap_or_default());
                    if !entry.is_dir() {
                        index.entry(name.to_lowercase()).or_insert(i);
                    }
                    sizes.push(entry.size());
                }
                Err(_) => {
                    sizes.push(0);
                }
            }
        }
        Ok(Self {
            zip: Mutex::new(zip),
            index,
            sizes,
        })
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
pub(super) fn xhtml_document(text: &str, chapter: &str, arch: Option<&Archive>) -> Document {
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
        // (`get`, not a slice: the address is the book's text, and byte 5 may be inside a character.)
        if href.len() > 5 && href.get(..5).is_some_and(|head| head.eq_ignore_ascii_case("data:")) {
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

#[cfg(test)]
mod tests {
    use super::*;

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

}
