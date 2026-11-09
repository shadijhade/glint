//! Ported from src/lib/jsonTree.ts.
//!
//! JSON (and YAML and TOML, once converted to the same values) as a tree for the preview: the values,
//! their rows a page at a time, jq paths, and one-line previews.
//!
//! The TypeScript leaned on `JSON.parse` with a reviver to keep numbers JavaScript would change
//! (`123456789012345678901234567890`, `1.50`, `1e400`) as written. Here a small parser of our own does
//! that, because `serde_json` without `arbitrary_precision` would round them. It never panics and
//! refuses nesting deeper than [`MAX_DEPTH`].

use std::collections::HashMap;
use std::fmt;

/// How deep the parser follows arrays and objects before giving up with an error (not a crash).
pub const MAX_DEPTH: usize = 512;

/// A number as written, where a JavaScript number would change it: a big integer, 1.50, 1e400.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonNumber {
    /// The number's text exactly as it was in the document.
    pub text: String,
}

impl JsonNumber {
    /// Keeps `text` as the number's value.
    pub fn new(text: impl Into<String>) -> Self {
        JsonNumber { text: text.into() }
    }

    /// As written, for serializing it back (the TS `toJSON`, which wrote it out raw).
    pub fn to_json(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for JsonNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// Any JSON value. Objects keep their keys in the order JavaScript's `Object.keys` gives: whole-number
/// keys first, ascending, then the others as they were written.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    /// A number JavaScript shows unchanged.
    Number(f64),
    String(String),
    /// A number kept as written because a JavaScript number would differ.
    Big(JsonNumber),
    Array(Vec<JsonValue>),
    /// Key and value pairs, already in `Object.keys` order, each key once.
    Object(Vec<(String, JsonValue)>),
}

/// Why a document did not parse, and where (a byte offset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// What was wrong, in plain words.
    pub message: String,
    /// Byte offset into the text where it went wrong.
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

/// JavaScript's `String(number)`, so we can tell whether a JS number would show the source unchanged.
fn js_number_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x == 0.0 {
        return "0".to_string();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-Infinity" } else { "Infinity" }.to_string();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    // `{:e}` gives the shortest digits that round-trip, like JS: "1.2345e21", "5e-7".
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = match sci.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (sci.as_str(), 0),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        let (a, b) = digits.split_at(n as usize);
        format!("{a}.{b}")
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e_text = format!("{}{}", if e < 0 { '-' } else { '+' }, e.abs());
        if k == 1 {
            format!("{digits}e{e_text}")
        } else {
            let (a, b) = digits.split_at(1);
            format!("{a}.{b}e{e_text}")
        }
    };
    format!("{sign}{body}")
}

/// The array index a key stands for in JavaScript (canonical whole number below 2^32 - 1), if it does.
fn array_index(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    if n < u32::MAX as u64 {
        Some(n as u32)
    } else {
        None
    }
}

/// Puts an object's pairs in `Object.keys` order: a repeated key keeps its first place and its last value.
fn finish_object(pairs: Vec<(String, JsonValue)>) -> Vec<(String, JsonValue)> {
    let mut places: HashMap<String, usize> = HashMap::with_capacity(pairs.len());
    let mut unique: Vec<(String, JsonValue)> = Vec::with_capacity(pairs.len());
    for (key, value) in pairs {
        match places.get(&key) {
            Some(&at) => {
                if let Some(slot) = unique.get_mut(at) {
                    slot.1 = value;
                }
            }
            None => {
                places.insert(key.clone(), unique.len());
                unique.push((key, value));
            }
        }
    }
    if unique.iter().any(|(k, _)| array_index(k).is_some()) {
        // Stable: whole-number keys ascending first, the rest keep their written order.
        let (mut indexed, rest): (Vec<_>, Vec<_>) = unique.into_iter().partition(|(k, _)| array_index(k).is_some());
        indexed.sort_by_key(|(k, _)| array_index(k));
        indexed.extend(rest);
        return indexed;
    }
    unique
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn fail<T>(&self, message: &str) -> Result<T, ParseError> {
        Err(ParseError {
            message: message.to_string(),
            offset: self.pos,
        })
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect_word(&mut self, word: &str, value: JsonValue) -> Result<JsonValue, ParseError> {
        match self.bytes.get(self.pos..self.pos + word.len()) {
            Some(found) if found == word.as_bytes() => {
                self.pos += word.len();
                Ok(value)
            }
            _ => self.fail("unexpected token"),
        }
    }

    fn value(&mut self, depth: usize) -> Result<JsonValue, ParseError> {
        self.skip_space();
        match self.peek() {
            None => self.fail("unexpected end of data"),
            Some(b'n') => self.expect_word("null", JsonValue::Null),
            Some(b't') => self.expect_word("true", JsonValue::Bool(true)),
            Some(b'f') => self.expect_word("false", JsonValue::Bool(false)),
            Some(b'"') => Ok(JsonValue::String(self.string()?)),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => self.fail("unexpected token"),
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonValue, ParseError> {
        if depth >= MAX_DEPTH {
            return self.fail("nested too deeply");
        }
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(JsonValue::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(items));
                }
                _ => return self.fail("expected ',' or ']'"),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonValue, ParseError> {
        if depth >= MAX_DEPTH {
            return self.fail("nested too deeply");
        }
        self.pos += 1; // {
        let mut pairs = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(JsonValue::Object(pairs));
        }
        loop {
            self.skip_space();
            if self.peek() != Some(b'"') {
                return self.fail("expected a string key");
            }
            let key = self.string()?;
            self.skip_space();
            if self.peek() != Some(b':') {
                return self.fail("expected ':'");
            }
            self.pos += 1;
            let value = self.value(depth + 1)?;
            pairs.push((key, value));
            self.skip_space();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(finish_object(pairs)));
                }
                _ => return self.fail("expected ',' or '}'"),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let Some(digits) = self.bytes.get(self.pos..self.pos + 4) else {
            return self.fail("bad unicode escape");
        };
        let mut n = 0u32;
        for &b in digits {
            let Some(d) = (b as char).to_digit(16) else {
                return self.fail("bad unicode escape");
            };
            n = n * 16 + d;
        }
        self.pos += 4;
        Ok(n)
    }

    /// A string starting at its opening quote. A lone surrogate escape becomes U+FFFD, since a Rust
    /// string can't hold one (NOTE(port): JavaScript would keep it).
    fn string(&mut self) -> Result<String, ParseError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            let start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            // The run ends at an ASCII byte or the end, so it is on char boundaries.
            if let Some(run) = self.text.get(start..self.pos) {
                out.push_str(run);
            }
            match self.peek() {
                None => return self.fail("unterminated string"),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let Some(esc) = self.peek() else {
                        return self.fail("unterminated string");
                    };
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let first = self.hex4()?;
                            if (0xD800..0xDC00).contains(&first) {
                                let pair = self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u");
                                let mut low = None;
                                if pair {
                                    let save = self.pos;
                                    self.pos += 2;
                                    match self.hex4() {
                                        Ok(l) if (0xDC00..0xE000).contains(&l) => low = Some(l),
                                        _ => self.pos = save,
                                    }
                                }
                                match low {
                                    Some(l) => {
                                        let code = 0x10000 + ((first - 0xD800) << 10) + (l - 0xDC00);
                                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                                    }
                                    None => out.push('\u{FFFD}'),
                                }
                            } else {
                                out.push(char::from_u32(first).unwrap_or('\u{FFFD}'));
                            }
                        }
                        _ => {
                            self.pos -= 1;
                            return self.fail("bad escape");
                        }
                    }
                }
                Some(_) => return self.fail("control character in string"),
            }
        }
    }

    fn digits(&mut self) -> bool {
        let start = self.pos;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        self.pos > start
    }

    fn number(&mut self) -> Result<JsonValue, ParseError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return self.fail("bad number"),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !self.digits() {
                return self.fail("bad number");
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !self.digits() {
                return self.fail("bad number");
            }
        }
        let source = self.text.get(start..self.pos).unwrap_or("");
        let Ok(n) = source.parse::<f64>() else {
            return self.fail("bad number");
        };
        // A JavaScript number only stands in if printing it gives the source back.
        if js_number_to_string(n) == source {
            Ok(JsonValue::Number(n))
        } else {
            Ok(JsonValue::Big(JsonNumber::new(source)))
        }
    }
}

/// JSON text as values, its numbers kept as written where JavaScript's would differ.
/// Errors (never panics) on bad JSON, trailing text, or nesting deeper than [`MAX_DEPTH`].
pub fn parse_json(text: &str) -> Result<JsonValue, ParseError> {
    let mut parser = Parser {
        text,
        bytes: text.as_bytes(),
        pos: 0,
    };
    let value = parser.value(0)?;
    parser.skip_space();
    if parser.pos < parser.bytes.len() {
        return parser.fail("unexpected data after the value");
    }
    Ok(value)
}

/// True for objects and arrays (not for a [`JsonNumber`] or any plain value).
pub fn is_container(value: &JsonValue) -> bool {
    matches!(value, JsonValue::Array(_) | JsonValue::Object(_))
}

/// How many children an array or object has; 0 for anything else.
pub fn size_of(value: &JsonValue) -> usize {
    match value {
        JsonValue::Array(items) => items.len(),
        JsonValue::Object(pairs) => pairs.len(),
        _ => 0,
    }
}

/// A key or index: what a row is called within its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKey<'a> {
    /// An object's key.
    Name(&'a str),
    /// An array's index.
    Index(usize),
}

/// True when `key` is a plain name jq can write after a dot: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_plain_name(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// JavaScript's `JSON.stringify` of a string: quoted, with the escapes JSON requires.
fn json_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A step in a jq path: `.name`, `.["two words"]` or `[3]`.
pub fn jq_step(key: RowKey) -> String {
    match key {
        RowKey::Index(i) => format!("[{i}]"),
        RowKey::Name(name) if is_plain_name(name) => format!(".{name}"),
        RowKey::Name(name) => format!(".[{}]", json_quote(name)),
    }
}

/// The jq path of `key` inside the value at `parent` (`.` is a document itself).
pub fn jq_path(parent: &str, key: RowKey) -> String {
    let step = jq_step(key);
    if parent == "." {
        if step.starts_with('.') {
            step
        } else {
            format!(".{step}")
        }
    } else {
        format!("{parent}{step}")
    }
}

/// One line of the tree.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow<'a> {
    /// Unique among the rows: the value's number, then its jq path in it.
    pub id: String,
    /// Its jq path within its value (`.` for a value itself).
    pub path: String,
    /// Which value it's in (a filter can give several).
    pub root: usize,
    pub depth: usize,
    /// Its key or index; `None` for a value itself.
    pub key: Option<RowKey<'a>>,
    pub value: &'a JsonValue,
    pub open: bool,
    /// A row standing for the rest of a big object or array, under `id`'s parent: how many more there are.
    pub more: Option<usize>,
}

/// How many of an open object's or array's children show at first, and with each "more".
pub const PAGE: usize = 500;

fn visit<'a>(
    rows: &mut Vec<TreeRow<'a>>,
    open: &HashMap<String, usize>,
    value: &'a JsonValue,
    key: Option<RowKey<'a>>,
    path: String,
    root: usize,
    depth: usize,
) {
    let id = format!("{root}:{path}");
    let shown = if is_container(value) {
        open.get(&id).copied()
    } else {
        None
    };
    rows.push(TreeRow {
        id: id.clone(),
        path: path.clone(),
        root,
        depth,
        key,
        value,
        open: shown.is_some(),
        more: None,
    });
    let Some(shown) = shown else { return };
    let size = size_of(value);
    let count = size.min(shown);
    match value {
        JsonValue::Array(items) => {
            for (i, item) in items.iter().take(count).enumerate() {
                visit(
                    rows,
                    open,
                    item,
                    Some(RowKey::Index(i)),
                    jq_path(&path, RowKey::Index(i)),
                    root,
                    depth + 1,
                );
            }
        }
        JsonValue::Object(pairs) => {
            for (name, item) in pairs.iter().take(count) {
                visit(
                    rows,
                    open,
                    item,
                    Some(RowKey::Name(name)),
                    jq_path(&path, RowKey::Name(name)),
                    root,
                    depth + 1,
                );
            }
        }
        _ => {}
    }
    if count < size {
        rows.push(TreeRow {
            id: format!("{id}\u{0}more"),
            path,
            root,
            depth: depth + 1,
            key: None,
            value,
            open: false,
            more: Some(size - count),
        });
    }
}

/// The rows to show: each value, and in each open object or array its children, a page or more at a
/// time (`open` holds how many show, by row id).
pub fn tree_rows<'a>(values: &'a [JsonValue], open: &HashMap<String, usize>) -> Vec<TreeRow<'a>> {
    let mut rows = Vec::new();
    for (root, value) in values.iter().enumerate() {
        visit(&mut rows, open, value, None, ".".to_string(), root, 0);
    }
    rows
}

/// Which objects and arrays start open: the values themselves, and what's in a small one.
/// Returned in the order the TypeScript's `Map` held them; each is `(row id, how many children show)`.
pub fn initially_open(values: &[JsonValue]) -> Vec<(String, usize)> {
    let mut open = Vec::new();
    for (root, value) in values.iter().enumerate() {
        if !is_container(value) {
            continue;
        }
        open.push((format!("{root}:."), PAGE));
        if values.len() > 1 || size_of(value) > 20 {
            continue;
        }
        let children: Vec<(RowKey, &JsonValue)> = match value {
            JsonValue::Array(items) => items.iter().enumerate().map(|(i, v)| (RowKey::Index(i), v)).collect(),
            JsonValue::Object(pairs) => pairs.iter().map(|(k, v)| (RowKey::Name(k.as_str()), v)).collect(),
            _ => Vec::new(),
        };
        for (key, child) in children {
            if is_container(child) && size_of(child) <= 50 {
                open.push((format!("{root}:{}", jq_path(".", key)), PAGE));
            }
        }
    }
    open
}

/// Length in UTF-16 code units, which is what JavaScript's `.length` counted.
fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// A value in one line, for a row: strings quoted, objects and arrays as their first few items.
/// `room` counts UTF-16 units like the TypeScript; pass 80 for the usual width.
pub fn preview(value: &JsonValue, room: usize) -> String {
    match value {
        JsonValue::Null => "null".to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Number(n) => js_number_to_string(*n),
        JsonValue::Big(n) => n.text.clone(),
        JsonValue::String(s) => {
            if utf16_len(s) > room {
                // NOTE(port): JS slices by UTF-16 unit and could split a surrogate pair; we drop the half pair.
                let mut used = 0;
                let mut cut = String::new();
                for c in s.chars() {
                    used += c.len_utf16();
                    if used > room {
                        break;
                    }
                    cut.push(c);
                }
                cut.push('…');
                json_quote(&cut)
            } else {
                json_quote(s)
            }
        }
        JsonValue::Array(_) | JsonValue::Object(_) => {
            let array = matches!(value, JsonValue::Array(_));
            let items: Vec<(Option<&str>, &JsonValue)> = match value {
                JsonValue::Array(items) => items.iter().take(20).map(|v| (None, v)).collect(),
                JsonValue::Object(pairs) => pairs.iter().take(20).map(|(k, v)| (Some(k.as_str()), v)).collect(),
                _ => Vec::new(),
            };
            let mut parts: Vec<String> = Vec::new();
            let mut used = 2;
            for (key, item) in items {
                let label = match key {
                    None => String::new(),
                    Some(k) if is_plain_name(k) => format!("{k}: "),
                    Some(k) => format!("{}: ", json_quote(k)),
                };
                let body = match item {
                    JsonValue::Array(_) => "[…]".to_string(),
                    JsonValue::Object(_) => "{…}".to_string(),
                    other => preview(other, 24),
                };
                let text = format!("{label}{body}");
                let len = utf16_len(&text);
                if used + len > room {
                    break;
                }
                used += len + 2;
                parts.push(text);
            }
            let rest = if size_of(value) > parts.len() {
                if parts.is_empty() {
                    "…"
                } else {
                    ", …"
                }
            } else {
                ""
            };
            let joined = parts.join(", ");
            if array {
                format!("[{joined}{rest}]")
            } else {
                format!("{{{joined}{rest}}}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{"name":"glint","big":123456789012345678901234567890,"price":1.50,"n":3,"tools":["fd","rg"],"odd key":{"x":[]}}"#;

    fn field<'a>(value: &'a JsonValue, name: &str) -> Option<&'a JsonValue> {
        match value {
            JsonValue::Object(pairs) => pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    fn doc() -> JsonValue {
        parse_json(DOC).expect("the sample document is valid JSON")
    }

    fn big_text(value: Option<&JsonValue>) -> Option<&str> {
        match value {
            Some(JsonValue::Big(n)) => Some(n.text.as_str()),
            _ => None,
        }
    }

    #[test]
    fn parses_numbers_as_written() {
        let doc = doc();
        assert!(is_container(&doc) && !matches!(doc, JsonValue::Array(_)));
        assert_eq!(big_text(field(&doc, "big")), Some("123456789012345678901234567890"));
        assert_eq!(big_text(field(&doc, "price")), Some("1.50"));
        assert_eq!(field(&doc, "n"), Some(&JsonValue::Number(3.0)));
    }

    #[test]
    fn jq_paths() {
        assert_eq!(jq_path(".", RowKey::Name("name")), ".name");
        assert_eq!(jq_path(".", RowKey::Index(2)), ".[2]");
        assert_eq!(jq_path(".tools", RowKey::Index(1)), ".tools[1]");
        assert_eq!(jq_path(".", RowKey::Name("odd key")), ".[\"odd key\"]");
        assert_eq!(jq_path(".[\"odd key\"]", RowKey::Name("x")), ".[\"odd key\"].x");
    }

    #[test]
    fn initially_open_and_rows() {
        let doc = doc();
        let opened = initially_open(std::slice::from_ref(&doc));
        let keys: Vec<&str> = opened.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["0:.", "0:.tools", "0:.[\"odd key\"]"]);
        let map: HashMap<String, usize> = opened.into_iter().collect();
        let values = [doc];
        let labels: Vec<String> = tree_rows(&values, &map)
            .iter()
            .map(|r| {
                let key = match r.key {
                    None => ".".to_string(),
                    Some(RowKey::Name(n)) => n.to_string(),
                    Some(RowKey::Index(i)) => i.to_string(),
                };
                format!("{}{}", "  ".repeat(r.depth), key)
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                ".",
                "  name",
                "  big",
                "  price",
                "  n",
                "  tools",
                "    0",
                "    1",
                "  odd key",
                "    x"
            ]
        );
    }

    #[test]
    fn previews() {
        let doc = doc();
        assert_eq!(preview(&doc, 40), "{name: \"glint\", …}");
        let arr = JsonValue::Array(vec![
            JsonValue::String("a".into()),
            JsonValue::Number(1.0),
            JsonValue::Null,
            JsonValue::Object(vec![("b".into(), JsonValue::Number(2.0))]),
        ]);
        assert_eq!(preview(&arr, 80), "[\"a\", 1, null, {…}]");
        let long = JsonValue::Object(vec![("a b".into(), JsonValue::String("x".repeat(100)))]);
        assert_eq!(preview(&long, 30), "{…}");
    }

    #[test]
    fn pages_big_arrays() {
        let long = JsonValue::Array((0..PAGE + 5).map(|i| JsonValue::Number(i as f64)).collect());
        let values = [long];
        let open: HashMap<String, usize> = HashMap::from([("0:.".to_string(), PAGE)]);
        let paged = tree_rows(&values, &open);
        assert_eq!(paged.len(), PAGE + 2);
        assert_eq!(paged.last().and_then(|r| r.more), Some(5));
        let open2: HashMap<String, usize> = HashMap::from([("0:.".to_string(), PAGE * 2)]);
        assert_eq!(tree_rows(&values, &open2).len(), PAGE + 6);
    }

    #[test]
    fn javascript_number_text() {
        assert_eq!(js_number_to_string(3.0), "3");
        assert_eq!(js_number_to_string(1.5), "1.5");
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1e-7), "1e-7");
        assert_eq!(js_number_to_string(0.000001), "0.000001");
        assert_eq!(js_number_to_string(123456789012345680000.0), "123456789012345680000");
        assert_eq!(big_text(parse_json("1e400").ok().as_ref()), Some("1e400"));
        assert_eq!(big_text(parse_json("-0").ok().as_ref()), Some("-0"));
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\":}",
            "01",
            "1.",
            "\"abc",
            "nul",
            "[1] x",
            "\"\\q\"",
            "\"\\u12\"",
        ] {
            assert!(parse_json(bad).is_err(), "{bad:?} should not parse");
        }
        let deep = "[".repeat(MAX_DEPTH + 10);
        assert!(parse_json(&deep).is_err());
    }

    #[test]
    fn object_keys_follow_javascript_order() {
        let v = parse_json(r#"{"b":1,"2":2,"a":3,"1":4,"b":5}"#).expect("valid");
        match v {
            JsonValue::Object(pairs) => {
                let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
                assert_eq!(keys, vec!["1", "2", "b", "a"]);
                assert_eq!(pairs[2].1, JsonValue::Number(5.0));
            }
            other => panic!("not an object: {other:?}"),
        }
    }

    #[test]
    fn string_escapes() {
        let v = parse_json(r#""a\n\u00e9\ud83d\ude00""#).expect("valid");
        assert_eq!(v, JsonValue::String("a\né😀".into()));
        assert_eq!(preview(&v, 80), "\"a\\né😀\"");
    }
}
