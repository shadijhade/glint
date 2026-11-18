//! What reading a Word document needs besides its own markup: a zip opened once ([`Archive`]) and XML read
//! without tripping on HTML's entities or on nesting deep enough to overflow the stack. These began in the e-book
//! reader and the mail viewer, both gone.

use roxmltree::{Document as Xml, Node, ParsingOptions};
use std::collections::HashMap;
use std::io::Read as _;
use std::sync::Mutex;

/// The most of one file read out of a zip (a document part).
const TEXT_LIMIT: u64 = 24 * 1024 * 1024;
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

/// How deep elements may nest. roxmltree overflows a small stack on a few hundred levels, and so would the
/// readers that walk the tree; real documents stay far below this.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs_resolve_against_their_folder() {
        assert_eq!(resolve("OEBPS/text", "../images/a%20b.png"), "OEBPS/images/a b.png");
        assert_eq!(resolve("OEBPS", "./c1.xhtml"), "OEBPS/c1.xhtml");
        assert_eq!(resolve("OEBPS", "/abs/x.html"), "abs/x.html");
        assert_eq!(resolve("", "a\\b.html"), "a/b.html");
        assert_eq!(resolve("a", "../../x"), "x");
        assert_eq!(percent_decode("%D8%A3.html"), "أ.html");
        assert_eq!(percent_decode("100%"), "100%");
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
}
