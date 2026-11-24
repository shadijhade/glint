//! The text of the homes that are not a file's own streams, and of an export: JSON Lines, UTF-8, `\n`.
//!
//! - `.glint-marks` beside the files (5.2): `{"glint-marks":1}`, then one line per item by `"name"`, sorted.
//! - The ledger on this PC (5.3): the same lines by `"path"`, with `"why"`.
//! - An export (F24): the labels in the first line, then the lines by path under the folder exported.
//!
//! What comes from outside is data (M10): every reader is capped, refuses a name that leaves its folder, and
//! never panics.

use super::{Label, Marks, MAX_LABELS};
use serde_json::Value;
use std::collections::HashMap;

/// More is never read of a `.glint-marks`.
pub const BESIDE_MAX_BYTES: usize = 4 << 20;
/// Nor more lines than this.
pub const BESIDE_MAX_LINES: usize = 100_000;

/// More is never read of the ledger or of an export.
pub const LEDGER_MAX_BYTES: usize = 64 << 20;

/// One item's line: whose marks, and what they are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    /// Beside: the item's name in its folder. The ledger: its full path. An export: its path under the folder.
    pub name: String,
    pub marks: Marks,
    /// When they were set, in Unix seconds (`"at"`; 0 when the line doesn't say).
    pub at: i64,
    /// The ledger's: why the marks are kept on this PC.
    pub why: Option<Why>,
    /// Keys this version doesn't know (a later version's), written back as they were.
    pub rest: serde_json::Map<String, serde_json::Value>,
}

/// Why the ledger keeps an item's marks (5.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// An import waiting for an item that was not there (a drive not connected, a file not found).
    Away,
    /// Neither the item's streams nor its folder can be written (a read-only share, an archive, a remote).
    Shut,
}

/// A `.glint-marks` as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Beside {
    /// The format's version, from the first line (1 today).
    pub version: u32,
    /// By name, as the file has them.
    pub lines: Vec<Line>,
    /// Lines left out: not JSON, cut off, past the caps, or a name with `\`, `/` or `..`.
    pub refused: usize,
}

/// A Glint export as read or to be written (F24).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Export {
    /// The labels of the PC it came from, in their order (the first line).
    pub labels: Vec<Label>,
    /// By path under the folder exported, with `\` between folders.
    pub lines: Vec<Line>,
}

/// Which file a line is read for: it decides the key of the name and which names are refused.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Beside,
    Ledger,
    Export,
}

/// A name of an item in a folder (beside, and each part of an export's path) is refused when it could leave it.
fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.encode_utf16().count() <= 255
        && !name.chars().any(|c| c < ' ' || "\\/:*?\"<>|".contains(c))
}

/// The name as the file will hold it (an export's / reads as \), or None when it is refused.
fn checked(kind: Kind, raw: &str) -> Option<String> {
    match kind {
        Kind::Beside => name_ok(raw).then(|| raw.to_string()),
        Kind::Ledger => (!raw.is_empty() && !raw.contains('\0')).then(|| raw.to_string()),
        Kind::Export => {
            let p = raw.replace('/', "\\");
            p.split('\\').all(name_ok).then_some(p)
        }
    }
}

/// A non-empty string, else none.
fn text_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

/// One line read: Err when refused, else its name and the line (None when it has no marks and nothing else).
fn parse_line(s: &str, kind: Kind) -> Result<(String, Option<Line>), ()> {
    let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(s) else {
        return Err(());
    };
    let key = if kind == Kind::Beside { "name" } else { "path" };
    let name = match o.remove(key) {
        Some(Value::String(n)) => checked(kind, &n).ok_or(())?,
        _ => return Err(()),
    };
    let label = text_of(o.remove("label").as_ref());
    let tags = match o.remove("tags") {
        Some(Value::Array(a)) => a.iter().filter_map(|t| text_of(Some(t))).collect(),
        _ => Vec::new(),
    };
    let comment = text_of(o.remove("comment").as_ref());
    let at = o.remove("at").and_then(|v| v.as_i64()).filter(|n| *n >= 0).unwrap_or(0);
    let why = match o.remove("why").as_ref().and_then(Value::as_str) {
        Some("away") => Some(Why::Away),
        Some("shut") => Some(Why::Shut),
        _ => None,
    };
    let marks = Marks { label, tags, comment };
    let kept = !marks.is_empty() || !o.is_empty();
    Ok((name.to_lowercase(), kept.then_some(Line { name, marks, at, why, rest: o })))
}

/// The text up to the byte cap, cut at a line end, and how many lines that cost (one, when something was cut).
fn capped(text: &str, max: usize) -> (&str, usize) {
    if text.len() <= max {
        return (text, 0);
    }
    let cut = text.as_bytes()[..=max].iter().rposition(|b| *b == b'\n').unwrap_or(0);
    (&text[..cut], 1)
}

/// The lines of a text that aren't empty, without their line ends.
fn nonempty(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).filter(|l| !l.trim().is_empty())
}

/// The version in a first line (1 or more) under the key, when it is that line.
fn version_in(line: &str, key: &str) -> Option<u32> {
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(line) else {
        return None;
    };
    let n = o.get(key)?.as_u64().filter(|n| *n >= 1)?;
    Some(u32::try_from(n).unwrap_or(u32::MAX))
}

/// Item lines read, the later of two for the same item standing; lines past max_lines are refused.
fn read_items<'a>(lines: impl Iterator<Item = &'a str>, kind: Kind, max_lines: usize, refused: &mut usize) -> Vec<Line> {
    let mut slots: Vec<Option<Line>> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (n, l) in lines.enumerate() {
        if n >= max_lines {
            *refused += 1;
            continue;
        }
        match parse_line(l, kind) {
            Err(()) => *refused += 1,
            Ok((k, line)) => match at.get(&k) {
                Some(&i) => slots[i] = line,
                None => {
                    at.insert(k, slots.len());
                    slots.push(line);
                }
            },
        }
    }
    slots.into_iter().flatten().collect()
}

/// A .glint-marks's text, read as far as it can be.
pub fn read_beside(text: &str) -> Beside {
    let (text, mut refused) = capped(text, BESIDE_MAX_BYTES);
    let mut it = nonempty(text).peekable();
    let version = it.peek().and_then(|l| version_in(l, "glint-marks"));
    if version.is_some() {
        it.next();
    }
    let lines = read_items(it, Kind::Beside, BESIDE_MAX_LINES, &mut refused);
    Beside { version: version.unwrap_or(0), lines, refused }
}

/// One line as text: the known keys in order, then the rest sorted by key (a Map's own order can't be relied on).
fn line_text(l: &Line, key: &str) -> String {
    let mut s = format!("{{\"{key}\":{}", Value::String(l.name.clone()));
    let mut put = |k: &str, v: Value| s.push_str(&format!(",{}:{v}", Value::from(k)));
    if let Some(x) = &l.marks.label {
        put("label", Value::String(x.clone()));
    }
    if !l.marks.tags.is_empty() {
        put("tags", Value::Array(l.marks.tags.iter().cloned().map(Value::String).collect()));
    }
    if let Some(x) = &l.marks.comment {
        put("comment", Value::String(x.clone()));
    }
    if l.at != 0 {
        put("at", Value::from(l.at));
    }
    match l.why {
        Some(Why::Away) => put("why", Value::from("away")),
        Some(Why::Shut) => put("why", Value::from("shut")),
        None => {}
    }
    let mut keys: Vec<&String> = l.rest.keys().filter(|k| k.as_str() != key).collect();
    keys.sort();
    for k in keys {
        put(k, l.rest[k].clone());
    }
    s.push('}');
    s
}

/// The lines to write, sorted by name without regard to case, each ended by a line end.
fn lines_text(lines: &[Line], key: &str) -> String {
    let mut v: Vec<&Line> = lines.iter().filter(|l| !l.marks.is_empty() || !l.rest.is_empty()).collect();
    v.sort_by_cached_key(|l| (l.name.to_lowercase(), l.name.clone()));
    v.iter().map(|l| line_text(l, key) + "\n").collect()
}

/// The text of a .glint-marks: the first line, then the lines sorted by name.
pub fn write_beside(b: &Beside) -> String {
    let body = lines_text(&b.lines, "name");
    if body.is_empty() {
        return body;
    }
    format!("{{\"glint-marks\":{}}}\n{body}", b.version.max(1))
}

/// The ledger's text, read as far as it can be.
pub fn read_ledger(text: &str) -> Vec<Line> {
    let (text, mut refused) = capped(text, LEDGER_MAX_BYTES);
    let mut it = nonempty(text).peekable();
    if it.peek().is_some_and(|l| version_in(l, "glint-ledger").is_some()) {
        it.next();
    }
    read_items(it, Kind::Ledger, usize::MAX, &mut refused)
}

/// The ledger's text: one line per item, by path.
pub fn write_ledger(lines: &[Line]) -> String {
    let body = lines_text(lines, "path");
    if body.is_empty() {
        return body;
    }
    format!("{{\"glint-ledger\":1}}\n{body}")
}

/// A Glint export's text; Err when it isn't one.
pub fn read_export(text: &str) -> Result<Export, String> {
    let (text, mut refused) = capped(text, LEDGER_MAX_BYTES);
    let mut it = nonempty(text);
    let not = || "Not a Glint export".to_string();
    let first = it.next().ok_or_else(not)?;
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(first) else {
        return Err(not());
    };
    o.get("glint-export").and_then(Value::as_u64).filter(|n| *n >= 1).ok_or_else(not)?;
    let labels = match o.get("labels") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| serde_json::from_value::<Label>(v.clone()).ok())
            .take(MAX_LABELS)
            .collect(),
        _ => Vec::new(),
    };
    let lines = read_items(it, Kind::Export, usize::MAX, &mut refused);
    Ok(Export { labels, lines })
}

/// The text of a Glint export.
pub fn write_export(export: &Export) -> String {
    let labels = serde_json::to_string(&export.labels).unwrap_or_else(|_| "[]".into());
    format!("{{\"glint-export\":1,\"labels\":{labels}}}\n{}", lines_text(&export.lines, "path"))
}

/// One item's marks as a line of text for the clipboard (Copy marks, F10): the line a `.glint-marks` would hold
/// for it, so that what is copied can be read, kept and pasted back. Empty when there are no marks.
pub fn clip_text(name: &str, marks: &Marks) -> String {
    let one = Beside { version: 1, lines: vec![Line { name: name.to_string(), marks: marks.clone(), ..Default::default() }], refused: 0 };
    write_beside(&one).lines().nth(1).unwrap_or_default().to_string()
}

/// The marks in a line [`clip_text`] made (Paste marks); None when the text is not such a line or holds no marks.
pub fn clip_marks(text: &str) -> Option<Marks> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    read_beside(line).lines.into_iter().next().map(|l| l.marks).filter(|m| !m.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("{}/../../tests/fixtures/marks/{name}", env!("CARGO_MANIFEST_DIR")))
            .unwrap_or_default()
    }

    fn line(name: &str, label: &str) -> Line {
        Line {
            name: name.into(),
            marks: Marks { label: Some(label.into()), ..Marks::default() },
            ..Line::default()
        }
    }

    fn names(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn a_later_versions_keys_are_kept_and_written_back() {
        let text = fixture("later-version.glint-marks");
        let b = read_beside(&text);
        assert_eq!((b.version, b.lines.len(), b.refused), (2, 2, 0));
        let a = &b.lines[0];
        assert_eq!(a.marks.label.as_deref(), Some("Important"));
        assert_eq!(a.marks.tags, ["x"]);
        assert_eq!((a.marks.comment.as_deref(), a.at), (Some("c"), 1795436917));
        assert_eq!(a.rest.keys().cloned().collect::<std::collections::BTreeSet<_>>().len(), 2);
        assert_eq!(a.rest["rating"], 4);
        assert!(a.rest.contains_key("color"));
        assert!(b.lines[1].rest["future"]["nested"].is_array());
        let out = write_beside(&b);
        let mut ls = out.lines();
        assert_eq!(ls.next(), Some("{\"glint-marks\":2}"));
        assert_eq!(
            ls.next(),
            Some("{\"name\":\"a.txt\",\"label\":\"Important\",\"tags\":[\"x\"],\"comment\":\"c\",\"at\":1795436917,\"color\":\"#ff8800\",\"rating\":4}")
        );
    }

    #[test]
    fn names_that_leave_the_folder_are_refused() {
        let b = read_beside(&fixture("leaves-folder.glint-marks"));
        assert_eq!((names(&b.lines), b.refused), (vec!["ok.txt"], 6));
    }

    #[test]
    fn a_file_cut_off_keeps_what_was_whole() {
        let b = read_beside(&fixture("cut.glint-marks"));
        assert_eq!((names(&b.lines), b.refused), (vec!["whole.txt"], 1));
    }

    #[test]
    fn what_is_written_reads_back_the_same() {
        let mut l1 = line("تقرير.pdf", "Red");
        l1.marks.comment = Some("one\ntwo\r\nthree".into());
        l1.marks.tags = vec!["a \"q\"".into(), "back\\slash".into()];
        l1.at = 5;
        let b = Beside { version: 1, lines: vec![line("a.txt", "Blue"), l1], refused: 0 };
        assert_eq!(read_beside(&write_beside(&b)), b);
    }

    #[test]
    fn lines_are_written_sorted_without_regard_to_case() {
        let b = Beside { version: 0, lines: vec![line("b.txt", "x"), line("C.txt", "x"), line("a.txt", "x")], refused: 0 };
        assert_eq!(names(&read_beside(&write_beside(&b)).lines), ["a.txt", "b.txt", "C.txt"]);
        assert_eq!(write_beside(&Beside::default()), "");
        let empty = Beside { version: 1, lines: vec![line("e.txt", "x")], refused: 0 };
        let mut empty = empty;
        empty.lines[0].marks = Marks::default();
        assert_eq!(write_beside(&empty), "");
    }

    #[test]
    fn bad_names_are_refused_and_the_later_of_two_stands() {
        let one = |n: &str| read_beside(&format!("{{\"name\":{},\"label\":\"x\"}}", Value::from(n)));
        for n in ["a:b", "a*b", "a\tb", &"x".repeat(256)] {
            assert_eq!(one(n).refused, 1, "{n:?}");
        }
        assert_eq!(one(&"x".repeat(255)).lines.len(), 1);
        let b = read_beside("{\"name\":\"A.txt\",\"label\":\"one\"}\n{\"name\":\"a.TXT\",\"label\":\"two\"}\n");
        assert_eq!((b.lines.len(), b.lines[0].marks.label.as_deref()), (1, Some("two")));
    }

    #[test]
    fn a_file_with_no_first_line_of_its_own_is_version_zero() {
        let b = read_beside("{\"name\":\"a.txt\",\"label\":\"x\"}\r\n\r\n{\"name\":\"b.txt\",\"label\":\"y\"}");
        assert_eq!((b.version, b.lines.len(), b.refused), (0, 2, 0));
    }

    #[test]
    fn nothing_that_is_not_a_line_panics() {
        let braces = "{".repeat(1000);
        for t in ["", "null", "42", "\"x\"", "[]", "{", braces.as_str(), "{\"name\":7}"] {
            let b = read_beside(t);
            assert!(b.lines.is_empty(), "{t:?}");
            assert_eq!(b.refused, usize::from(!t.is_empty()), "{t:?}");
            assert!(read_ledger(t).is_empty());
        }
    }

    #[test]
    fn the_caps_hold() {
        let text: String = (0..100_001).map(|i| format!("{{\"name\":\"{i}\",\"label\":\"x\"}}\n")).collect();
        assert!(text.len() < BESIDE_MAX_BYTES);
        let b = read_beside(&text);
        assert_eq!((b.lines.len(), b.refused), (100_000, 1));
        let big = format!("{{\"name\":\"{}\"}}", "x".repeat(5 << 20));
        let b = read_beside(&big);
        assert_eq!((b.lines.len(), b.refused), (0, 1));
    }

    #[test]
    fn the_ledger_keeps_why_and_its_own_key() {
        let text = "{\"glint-ledger\":1}\n{\"path\":\"C:\\\\a\",\"label\":\"x\",\"why\":\"away\"}\n{\"path\":\"C:\\\\b\",\"label\":\"x\",\"why\":\"shut\"}\n{\"path\":\"C:\\\\c\",\"label\":\"x\",\"why\":\"other\"}\n{\"name\":\"d\",\"label\":\"x\"}\n{\"path\":\"C:\\\\E\",\"label\":\"old\"}\n{\"path\":\"c:\\\\e\",\"label\":\"new\"}\n";
        let ls = read_ledger(text);
        assert_eq!(names(&ls), ["C:\\a", "C:\\b", "C:\\c", "c:\\e"]);
        assert_eq!((ls[0].why, ls[1].why, ls[2].why), (Some(Why::Away), Some(Why::Shut), None));
        assert_eq!(ls[3].marks.label.as_deref(), Some("new"));
        let out = write_ledger(&ls);
        assert!(out.starts_with("{\"glint-ledger\":1}\n"));
        assert_eq!(read_ledger(&out), ls);
        assert_eq!(write_ledger(&[]), "");
    }

    #[test]
    fn an_export_reads_back_as_written_and_leaves_out_paths_that_escape() {
        let e = Export {
            labels: vec![Label { name: "Red".into(), color: 0xff0000, was: vec!["Rot".into()] }],
            lines: vec![line("a\\b.txt", "Red"), line("c.txt", "Red")],
        };
        let out = write_export(&e);
        assert_eq!(read_export(&out), Ok(e));
        assert_eq!(read_export(&fixture("later-version.glint-marks")), Err("Not a Glint export".into()));
        assert_eq!(read_export(""), Err("Not a Glint export".into()));
        let none = write_export(&Export::default());
        assert_eq!(none, "{\"glint-export\":1,\"labels\":[]}\n");
        let mut text = none;
        for p in ["C:\\\\x", "\\\\\\\\server\\\\share\\\\x", "..\\\\x", "a\\\\..\\\\b", "\\\\abs", "a:b", "sub/notes.md"] {
            text += &format!("{{\"path\":\"{p}\",\"label\":\"x\"}}\n");
        }
        let x = read_export(&text).unwrap_or_default();
        assert_eq!(names(&x.lines), ["sub\\notes.md"]);
    }

    #[test]
    fn marks_copied_as_a_line_paste_back_the_same() {
        let marks = Marks { label: Some("Important".into()), tags: vec!["bond".into(), "تقرير".into()], comment: Some("mode 4\nLACP \"fast\"".into()) };
        let text = clip_text("bond0.conf", &marks);
        assert!(text.starts_with(r#"{"name":"bond0.conf","label":"Important""#), "{text}");
        assert_eq!(clip_marks(&text), Some(marks));
        assert_eq!(clip_text("a.txt", &Marks::default()), "");
        for odd in ["", "hello", "{}", "42", r#"{"name":"a.txt"}"#, "\u{0}"] {
            assert_eq!(clip_marks(odd), None, "{odd:?}");
        }
    }
}
