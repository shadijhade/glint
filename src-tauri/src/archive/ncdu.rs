//! ncdu's JSON exports (`ncdu -o export.json`, or compressed), browsed as the folders they describe:
//! names, sizes and dates, so the disk usage map, folder sizes and search work on a disk that was
//! scanned somewhere else. Nothing in one can be opened or copied out: it holds no file's contents.
//!
//! The export is `[1, minor, {metadata}, root]`, where a folder is an array of its own info and its
//! children, and a file is its info object. It's read as a stream with a stack of the folders it's
//! in, rather than recursively, so a deep one can't run the stack out.

use super::tree::{Item, IN_ARCHIVE, LIST_ONLY};
use super::Failure;
use std::collections::HashSet;
use std::io::{BufRead, ErrorKind};

/// Folders nested deeper than this are taken for damage (real disks stop long before).
const MAX_DEPTH: usize = 2048;
/// The longest name read, and all the paths together (each item keeps its full path).
const MAX_NAME: usize = 64 << 10;
const MAX_PATHS: u64 = 512 << 20;
/// Metadata and unknown values skipped over, at most.
const MAX_SKIPPED: u64 = 16 << 20;

/// Whether `head` starts like an ncdu export: `[1,` and a minor version, then its metadata naming
/// the program that wrote it.
pub fn looks_like(head: &[u8]) -> bool {
    let text: Vec<u8> = head.iter().copied().filter(|b| !b.is_ascii_whitespace()).take(64).collect();
    let text = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&text);
    let Some(rest) = text.strip_prefix(b"[1,") else { return false };
    let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
    digits > 0 && rest[digits..].starts_with(b",{\"progname\"")
}

fn damaged(what: &str) -> Failure {
    Failure::Damaged(format!("it isn't an ncdu export Glint can read ({what})"))
}

/// A one-letter escape's byte ("\n"…; "\"", "\\" and "\/" are themselves).
fn unescaped(b: u8) -> u8 {
    match b {
        b'n' => b'\n',
        b't' => b'\t',
        b'r' => b'\r',
        b'b' => 8,
        b'f' => 12,
        other => other,
    }
}

/// A character as UTF-8, or U+FFFD for none (a surrogate on its own).
fn push_char(out: &mut Vec<u8>, c: Option<char>) {
    let mut buf = [0u8; 4];
    out.extend_from_slice(c.unwrap_or('\u{FFFD}').encode_utf8(&mut buf).as_bytes());
}

/// What a file or folder's info object says. `dev` is written only where it changes from the
/// folder's.
#[derive(Default)]
struct Info {
    name: Vec<u8>,
    asize: u64,
    mtime: i64,
    dev: Option<u64>,
    ino: u64,
    hard_link: bool,
}

struct Lexer<R> {
    r: R,
    peeked: Option<u8>,
    skipped: u64,
}

impl<R: BufRead> Lexer<R> {
    fn byte(&mut self) -> Result<Option<u8>, Failure> {
        if let Some(b) = self.peeked.take() {
            return Ok(Some(b));
        }
        loop {
            let buf = match self.r.fill_buf() {
                Ok(buf) => buf,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(Failure::Damaged(format!("it can't be decompressed ({e})"))),
            };
            let Some(&b) = buf.first() else { return Ok(None) };
            self.r.consume(1);
            return Ok(Some(b));
        }
    }

    /// The next byte that isn't white space.
    fn token(&mut self) -> Result<Option<u8>, Failure> {
        loop {
            match self.byte()? {
                Some(b' ' | b'\t' | b'\n' | b'\r') => {}
                other => return Ok(other),
            }
        }
    }

    fn expect(&mut self, want: u8) -> Result<(), Failure> {
        match self.token()? {
            Some(b) if b == want => Ok(()),
            Some(b) => Err(damaged(&format!("“{}” where “{}” should be", b as char, want as char))),
            None => Err(damaged("it ends early")),
        }
    }

    /// A string's bytes, after its opening quote: escapes decoded (\u ones to UTF-8), other bytes
    /// as they are (ncdu writes names' bytes as the disk had them, UTF-8 or not).
    fn string(&mut self, max: usize) -> Result<Vec<u8>, Failure> {
        let mut out = Vec::new();
        loop {
            let b = self.byte()?.ok_or_else(|| damaged("a name doesn't end"))?;
            match b {
                b'"' => return Ok(out),
                b'\\' => match self.byte()?.ok_or_else(|| damaged("a name doesn't end"))? {
                    b'u' => {
                        let unit = self.hex4()?;
                        if (0xD800..0xDC00).contains(&unit) {
                            // A surrogate pair, when the next escape is its low half; a lone half
                            // is lost (U+FFFD), and what comes after it is read as itself.
                            match self.byte()? {
                                Some(b'\\') => match self.byte()?.ok_or_else(|| damaged("a name doesn't end"))? {
                                    b'u' => {
                                        let low = self.hex4()?;
                                        if (0xDC00..0xE000).contains(&low) {
                                            push_char(&mut out, char::from_u32(0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)));
                                        } else {
                                            push_char(&mut out, None);
                                            push_char(&mut out, char::from_u32(low));
                                        }
                                    }
                                    other => {
                                        push_char(&mut out, None);
                                        out.push(unescaped(other));
                                    }
                                },
                                other => {
                                    self.peeked = other;
                                    push_char(&mut out, None);
                                }
                            }
                        } else {
                            push_char(&mut out, char::from_u32(unit));
                        }
                    }
                    other => out.push(unescaped(other)),
                },
                other => out.push(other),
            }
            if out.len() > max {
                return Err(Failure::Bomb(format!("a name in it is over {} KB long", max >> 10)));
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, Failure> {
        let mut value = 0u32;
        for _ in 0..4 {
            let b = self.byte()?.ok_or_else(|| damaged("an escape doesn't end"))?;
            let digit = (b as char).to_digit(16).ok_or_else(|| damaged("an escape isn't hex"))?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    /// A number (its first character already read), as f64: ncdu writes integers, but JSON allows more.
    fn number(&mut self, first: u8) -> Result<f64, Failure> {
        let mut text = vec![first];
        loop {
            match self.byte()? {
                Some(b @ (b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')) if text.len() < 64 => text.push(b),
                other => {
                    self.peeked = other;
                    break;
                }
            }
        }
        std::str::from_utf8(&text).ok().and_then(|t| t.parse().ok()).ok_or_else(|| damaged("a number isn't one"))
    }

    /// Passes over an object or array (its opening bracket already read), however nested, without
    /// recursing.
    fn skip(&mut self) -> Result<(), Failure> {
        let mut depth = 1usize;
        while depth > 0 {
            match self.byte()?.ok_or_else(|| damaged("it ends early"))? {
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                b'"' => self.skipped += self.string(MAX_NAME)?.len() as u64,
                _ => {}
            }
            self.skipped += 1;
            if self.skipped > MAX_SKIPPED {
                return Err(Failure::Bomb("it holds more than ncdu writes besides names and sizes".into()));
            }
        }
        Ok(())
    }

    /// An info object's fields, after its opening brace.
    fn info(&mut self) -> Result<Info, Failure> {
        let mut info = Info::default();
        loop {
            match self.token()? {
                Some(b'}') => return Ok(info),
                Some(b',') => continue,
                Some(b'"') => {}
                _ => return Err(damaged("an entry's fields aren't named")),
            }
            let key = self.string(64)?;
            self.expect(b':')?;
            let first = self.token()?.ok_or_else(|| damaged("it ends early"))?;
            match (key.as_slice(), first) {
                (b"name", b'"') => info.name = self.string(MAX_NAME)?,
                (b"asize" | b"mtime" | b"dev" | b"ino" | b"nlink", b'-' | b'0'..=b'9') => {
                    let n = self.number(first)?;
                    match key.as_slice() {
                        b"asize" => info.asize = n.max(0.0) as u64,
                        b"mtime" => info.mtime = (n * 1000.0) as i64,
                        b"dev" => info.dev = Some(n as u64),
                        b"ino" => info.ino = n as u64,
                        _ => info.hard_link |= n > 1.0,
                    }
                }
                (b"hlnkc", b't') => {
                    self.word(b"rue")?;
                    info.hard_link = true;
                }
                (_, b't') => self.word(b"rue")?,
                (_, b'f') => self.word(b"alse")?,
                (_, b'n') => self.word(b"ull")?,
                (_, b'-' | b'0'..=b'9') => {
                    self.number(first)?;
                }
                (_, b'"') => self.skipped += self.string(MAX_NAME)?.len() as u64,
                (_, b'{' | b'[') => self.skip()?,
                _ => return Err(damaged("a field's value isn't one")),
            }
        }
    }

    fn word(&mut self, rest: &[u8]) -> Result<(), Failure> {
        for &want in rest {
            if self.byte()? != Some(want) {
                return Err(damaged("a word isn't true, false or null"));
            }
        }
        Ok(())
    }
}

/// A folder being read: its path, its device, and the names in it so far (to tell apart ones that
/// differ only in case, as Linux allows and Windows doesn't).
struct Frame {
    path: String,
    dev: u64,
    names: HashSet<String>,
}

/// `name` as it can be in Windows' folder: "\" and "/" (a Linux name can't have "/" but may have
/// "\") as "_", and a case clash with a name already there numbered: "Notes (2).txt".
fn place(name: &[u8], names: &mut HashSet<String>) -> String {
    let mut name: String = String::from_utf8_lossy(name).chars().map(|c| if c == '/' || c == '\\' { '_' } else { c }).collect();
    if name.is_empty() {
        name.push('_');
    }
    if names.insert(name.to_lowercase()) {
        return name;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name.as_str(), ""),
    };
    for n in 2.. {
        let candidate = format!("{stem} ({n}){ext}");
        if names.insert(candidate.to_lowercase()) {
            return candidate;
        }
    }
    unreachable!()
}

/// Reads an export's folders and files. Sizes are apparent sizes (`asize`); a hard link's size
/// counts once, at its first name.
pub fn read(r: impl BufRead, max_entries: usize) -> Result<Vec<Item>, Failure> {
    let mut lx = Lexer { r, peeked: None, skipped: 0 };
    lx.expect(b'[')?;
    match lx.token()? {
        Some(b'1') if matches!(lx.token()?, Some(b',')) => {}
        _ => return Err(Failure::Unreadable("it's an ncdu export in a version Glint doesn't read".into())),
    }
    let minor = lx.token()?.ok_or_else(|| damaged("it ends early"))?;
    lx.number(minor)?;
    lx.expect(b',')?;
    lx.expect(b'{')?;
    lx.skip()?;
    lx.expect(b',')?;
    // The top: the folder that was scanned (its name is its whole path there).
    lx.expect(b'[')?;
    lx.expect(b'{')?;
    let top = lx.info()?;
    let mut stack = vec![Frame { path: String::new(), dev: top.dev.unwrap_or(0), names: HashSet::new() }];
    let mut items = Vec::new();
    let mut linked: HashSet<(u64, u64)> = HashSet::new();
    let mut paths = 0u64;
    while let Some(frame) = stack.last_mut() {
        match lx.token()? {
            Some(b']') => {
                stack.pop();
                continue;
            }
            Some(b',') => {}
            _ => return Err(damaged("a folder's entries aren't separated")),
        }
        let dir = match lx.token()? {
            Some(b'{') => false,
            Some(b'[') => {
                lx.expect(b'{')?;
                true
            }
            _ => return Err(damaged("an entry isn't a file or a folder")),
        };
        let info = lx.info()?;
        if items.len() >= max_entries {
            return Err(Failure::TooManyEntries(items.len() + 1));
        }
        let name = place(&info.name, &mut frame.names);
        let path = if frame.path.is_empty() { name } else { format!("{}\\{name}", frame.path) };
        paths += path.len() as u64;
        if paths > MAX_PATHS {
            return Err(Failure::Bomb(format!("its names add up to over {} MB", MAX_PATHS >> 20)));
        }
        // A hard link's bytes are on the disk once: only its first name counts them.
        let dev = info.dev.unwrap_or(frame.dev);
        let repeated = !dir && info.hard_link && !linked.insert((dev, info.ino));
        let size = if dir || repeated { 0 } else { info.asize };
        items.push(Item { path: path.clone(), dir, size, modified: info.mtime, attrs: IN_ARCHIVE | LIST_ONLY });
        if dir {
            if stack.len() >= MAX_DEPTH {
                return Err(damaged(&format!("its folders nest more than {MAX_DEPTH} deep")));
            }
            stack.push(Frame { path, dev, names: HashSet::new() });
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(json: &str) -> Vec<(String, bool, u64)> {
        read(json.as_bytes(), 1000).unwrap().into_iter().map(|i| (i.path, i.dir, i.size)).collect()
    }

    #[test]
    fn reads_an_export() {
        let json = r#"[1,2,{"progname":"ncdu","progver":"1.19","timestamp":1700000000},
            [{"name":"/home/me","asize":4096,"dsize":4096,"dev":2049,"ino":2},
              {"name":"notes.txt","asize":1200,"dsize":4096,"ino":10,"mtime":1700000000},
              [{"name":"src","asize":4096,"ino":11,"read_error":true},
                {"name":"main.rs","asize":300,"ino":12,"extended":{"x":[1,2,{"y":null}]}},
                {"name":"a\\b","asize":1},
                {"name":"Main.rs","asize":2}
              ],
              {"name":"link","asize":1200,"ino":10,"hlnkc":true},
              {"name":"first","asize":1200,"ino":10,"hlnkc":true},
              {"name":"emoji 😀 é","asize":5,"excluded":"pattern"}
            ]]"#;
        assert_eq!(
            names(json),
            [
                ("notes.txt".into(), false, 1200),
                ("src".into(), true, 0),
                ("src\\main.rs".into(), false, 300),
                ("src\\a_b".into(), false, 1),
                ("src\\Main (2).rs".into(), false, 2),
                ("link".into(), false, 1200),
                ("first".into(), false, 0),
                ("emoji 😀 é".into(), false, 5),
            ]
        );
        assert!(looks_like(json.as_bytes()) && !looks_like(b"[1,2,3]") && !looks_like(b"{\"progname\":\"ncdu\"}"));
    }

    #[test]
    fn copes_with_what_a_crafted_export_could_do() {
        let head = r#"[1,0,{"progname":"ncdu"},[{"name":"/"}"#;
        // Deeper than any disk: an error, not a stack overflow.
        let deep = format!("{head}{}{}]", r#",[{"name":"d"}"#.repeat(10_000), "]".repeat(10_000));
        assert!(matches!(read(deep.as_bytes(), 1_000_000), Err(Failure::Damaged(_))));
        let fine = format!("{head}{}{}]", r#",[{"name":"d"}"#.repeat(2000), "]".repeat(2000));
        assert_eq!(read(fine.as_bytes(), 1_000_000).unwrap().len(), 2000);
        // Cut anywhere: an error at worst.
        let json = r#"[1,2,{"progname":"ncdu"},[{"name":"/"},{"name":"a","asize":1},[{"name":"b"},{"name":"cé"}]]]"#;
        for cut in 0..json.len() {
            let _ = read(&json.as_bytes()[..cut], 100);
        }
        // Names that aren't UTF-8, a lone surrogate, a version 2 export.
        let odd = b"[1,2,{\"progname\":\"ncdu\"},[{\"name\":\"/\"},{\"name\":\"caf\xE9\"},{\"name\":\"x\\ud800y\"}]]";
        assert_eq!(read(&odd[..], 10).unwrap().iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["caf\u{FFFD}", "x\u{FFFD}y"]);
        assert!(matches!(read(&b"[2,0,{}]"[..], 10), Err(Failure::Unreadable(_))));
        assert!(matches!(read(&b"[1,2,{\"progname\":\"ncdu\"},[{\"name\":\"/\"},{\"name\":\"a\"},{\"name\":\"b\"}]]"[..], 1), Err(Failure::TooManyEntries(_))));
    }
}
