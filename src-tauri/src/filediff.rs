//! Compare two files: text side by side, by Difftastic's structural diff when it's installed (what
//! changed in the code, not its layout) or a line diff otherwise; and bytes, a window of each at a
//! time with the next place they differ.

use crate::fsx::{blocking, ms_since};
use serde::Serialize;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The most of a file compared as text.
const TEXT_BYTES: u64 = 8 << 20;
/// How long Difftastic may take before the line diff answers instead.
const DIFFTASTIC_TIME: Duration = Duration::from_secs(5);
/// The most Difftastic may say.
const DIFFTASTIC_OUTPUT: usize = 8 << 20;

/// Two files side by side, as text.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct TextCompare {
    pub left: Vec<String>,
    pub right: Vec<String>,
    /// The rows side by side, by line (from 0) on each side; None where only the other side has one.
    pub rows: Vec<[Option<u32>; 2]>,
    /// The changed lines on each side, with what changed in them: [start, end) in UTF-16 units of the
    /// line; none listed means the whole line.
    pub left_changes: HashMap<u32, Vec<[u32; 2]>>,
    pub right_changes: HashMap<u32, Vec<[u32; 2]>>,
    /// Who compared them: "difftastic" or "lines".
    pub engine: String,
    /// The language Difftastic read them as ("Rust").
    pub language: Option<String>,
    /// Either has a NUL in its first 8 KB, as Git tells binary files: compare the bytes instead.
    pub binary: bool,
    /// Either is over [`TEXT_BYTES`].
    pub too_big: bool,
    pub elapsed: f64,
}

/// A file's text, lines evened out (CRLF or LF); None when it's binary.
fn text_of(bytes: &[u8]) -> Option<String> {
    (!bytes[..bytes.len().min(8192)].contains(&0)).then(|| String::from_utf8_lossy(bytes).replace("\r\n", "\n"))
}

/// The UTF-16 offset of byte `at` in `line` (Difftastic counts bytes; the page, UTF-16 units).
fn utf16_at(line: &str, at: usize) -> u32 {
    let at = (0..=at.min(line.len())).rev().find(|&i| line.is_char_boundary(i)).unwrap_or(0);
    line[..at].encode_utf16().count() as u32
}

/// The rows and changed lines from a line diff: what's the same pairs up, a change's lines pair up
/// as far as both sides have them.
fn by_lines(left: &str, right: &str, compare: &mut TextCompare) {
    use gix::diff::blob::{Algorithm, Diff, InternedInput};
    let input = InternedInput::new(left, right);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let (before, mut i, mut j) = (input.before.len() as u32, 0u32, 0u32);
    for hunk in diff.hunks() {
        while i < hunk.before.start {
            compare.rows.push([Some(i), Some(j)]);
            (i, j) = (i + 1, j + 1);
        }
        let (removed, added) = (hunk.before.len() as u32, hunk.after.len() as u32);
        for k in 0..removed.max(added) {
            let (l, r) = ((k < removed).then_some(hunk.before.start + k), (k < added).then_some(hunk.after.start + k));
            if let Some(l) = l {
                compare.left_changes.insert(l, Vec::new());
            }
            if let Some(r) = r {
                compare.right_changes.insert(r, Vec::new());
            }
            compare.rows.push([l, r]);
        }
        (i, j) = (hunk.before.end, hunk.after.end);
    }
    while i < before {
        compare.rows.push([Some(i), Some(j)]);
        (i, j) = (i + 1, j + 1);
    }
    compare.engine = "lines".into();
}

/// Difftastic's answer, read into rows and changes: its `aligned_lines` pair lines up, its `chunks`
/// say what changed in each. False when it's not there, took too long, or said something else.
fn by_difftastic(left: &str, right: &str, compare: &mut TextCompare) -> bool {
    let Ok(mut command) = crate::tools::command("difftastic") else { return false };
    command.args(["--display", "json", "--color", "never"]).arg(left).arg(right).env("DFT_UNSTABLE", "yes").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(mut child) = command.spawn() else { return false };
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(stdout) = stdout.as_mut() {
            let _ = stdout.take(DIFFTASTIC_OUTPUT as u64 + 1).read_to_end(&mut out);
        }
        out
    });
    let deadline = Instant::now() + DIFFTASTIC_TIME;
    let finished = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success() || status.code() == Some(1),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    if !finished || out.len() > DIFFTASTIC_OUTPUT {
        return false;
    }
    let Ok(answer) = serde_json::from_slice::<serde_json::Value>(&out) else { return false };
    let Some(aligned) = answer.get("aligned_lines").and_then(|a| a.as_array()) else { return false };
    let (lefts, rights) = (compare.left.len() as u64, compare.right.len() as u64);
    let side = |v: &serde_json::Value, count: u64| v.as_u64().filter(|&n| n < count).map(|n| n as u32);
    for pair in aligned {
        let (l, r) = (side(&pair[0], lefts), side(&pair[1], rights));
        if l.is_some() || r.is_some() {
            compare.rows.push([l, r]);
        }
    }
    // Only what its chunks list changed: a line one side has alone that they don't (the same code laid
    // out on more lines) shows as it is, across from nothing.
    for line in answer.get("chunks").and_then(|c| c.as_array()).into_iter().flatten().filter_map(|c| c.as_array()).flatten() {
        for (key, lines, changes) in [("lhs", &compare.left, &mut compare.left_changes), ("rhs", &compare.right, &mut compare.right_changes)] {
            let Some(side) = line.get(key) else { continue };
            let Some(number) = side.get("line_number").and_then(|n| n.as_u64()).filter(|&n| n < lines.len() as u64) else { continue };
            let text = &lines[number as usize];
            let spans = changes.entry(number as u32).or_default();
            for change in side.get("changes").and_then(|c| c.as_array()).into_iter().flatten() {
                let at = |key: &str| change.get(key).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                spans.push([utf16_at(text, at("start")), utf16_at(text, at("end"))]);
            }
        }
    }
    compare.engine = "difftastic".into();
    compare.language = answer.get("language").and_then(|l| l.as_str()).map(str::to_string);
    true
}

fn read_capped(path: &str) -> Result<(Vec<u8>, bool), String> {
    let file = File::open(path).map_err(|e| format!("Couldn't open {path}: {e}"))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > TEXT_BYTES {
        return Ok((Vec::new(), true));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(TEXT_BYTES).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok((bytes, false))
}

fn compare_text(left: &str, right: &str) -> Result<TextCompare, String> {
    let started = Instant::now();
    let mut compare = TextCompare::default();
    let ((left_bytes, left_big), (right_bytes, right_big)) = (read_capped(left)?, read_capped(right)?);
    compare.too_big = left_big || right_big;
    let (Some(left_text), Some(right_text)) = (text_of(&left_bytes), text_of(&right_bytes)) else {
        compare.binary = !compare.too_big;
        compare.elapsed = ms_since(started);
        return Ok(compare);
    };
    if compare.too_big {
        compare.elapsed = ms_since(started);
        return Ok(compare);
    }
    compare.left = left_text.lines().map(str::to_string).collect();
    compare.right = right_text.lines().map(str::to_string).collect();
    if !by_difftastic(left, right, &mut compare) {
        (compare.rows, compare.left_changes, compare.right_changes) = Default::default();
        by_lines(&left_text, &right_text, &mut compare);
    }
    compare.elapsed = ms_since(started);
    Ok(compare)
}

/// Two text files side by side; `binary` or `tooBig` when they're to be compared by their bytes.
#[tauri::command]
pub async fn compare_text_files(left: String, right: String) -> Result<TextCompare, String> {
    blocking(move || compare_text(&left, &right)).await
}

fn read_at(path: &str, offset: u64, len: usize) -> Result<(u64, Vec<u8>), String> {
    let mut file = File::open(path).map_err(|e| format!("Couldn't open {path}: {e}"))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    file.seek(SeekFrom::Start(offset.min(size))).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(len);
    file.take(len as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok((size, bytes))
}

/// `len` bytes of each file from `offset` (at most 1 MB), for the hex view: both sizes (u64), both
/// lengths (u32), then the bytes, little-endian.
#[tauri::command]
pub async fn compare_bytes(left: String, right: String, offset: u64, len: u32) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        let len = len.min(1 << 20) as usize;
        let ((left_size, left_bytes), (right_size, right_bytes)) = (read_at(&left, offset, len)?, read_at(&right, offset, len)?);
        let mut out = Vec::with_capacity(24 + left_bytes.len() + right_bytes.len());
        out.extend(left_size.to_le_bytes());
        out.extend(right_size.to_le_bytes());
        out.extend((left_bytes.len() as u32).to_le_bytes());
        out.extend((right_bytes.len() as u32).to_le_bytes());
        out.extend(left_bytes);
        out.extend(right_bytes);
        Ok(tauri::ipc::Response::new(out))
    })
    .await
}

/// `len` bytes of a file from `offset` (at most 1 MB), for the preview's hex view: its size (u64,
/// little-endian), then the bytes.
#[tauri::command]
pub async fn file_bytes(path: String, offset: u64, len: u32) -> Result<tauri::ipc::Response, String> {
    blocking(move || {
        let (size, bytes) = read_at(&path, offset, len.min(1 << 20) as usize)?;
        let mut out = Vec::with_capacity(8 + bytes.len());
        out.extend(size.to_le_bytes());
        out.extend(bytes);
        Ok(tauri::ipc::Response::new(out))
    })
    .await
}

/// The first offset at or after `from` where the files differ (a byte, or one ending before the
/// other); None when they're the same from there on.
fn next_difference(left: &str, right: &str, from: u64) -> Result<Option<u64>, String> {
    let open = |path: &str| -> Result<File, String> {
        let mut file = File::open(path).map_err(|e| format!("Couldn't open {path}: {e}"))?;
        file.seek(SeekFrom::Start(from)).map_err(|e| e.to_string())?;
        Ok(file)
    };
    let (mut a, mut b) = (open(left)?, open(right)?);
    let (mut x, mut y) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    let mut at = from;
    loop {
        let fill = |file: &mut File, buf: &mut [u8]| -> Result<usize, String> {
            let mut got = 0;
            while got < buf.len() {
                match file.read(&mut buf[got..]).map_err(|e| e.to_string())? {
                    0 => break,
                    n => got += n,
                }
            }
            Ok(got)
        };
        let (n, m) = (fill(&mut a, &mut x)?, fill(&mut b, &mut y)?);
        let common = n.min(m);
        if let Some(i) = x[..common].iter().zip(&y[..common]).position(|(p, q)| p != q) {
            return Ok(Some(at + i as u64));
        }
        if n != m {
            return Ok(Some(at + common as u64));
        }
        if n == 0 {
            return Ok(None);
        }
        at += n as u64;
    }
}

#[tauri::command]
pub async fn next_byte_difference(left: String, right: String, from: u64) -> Result<Option<u64>, String> {
    blocking(move || next_difference(&left, &right, from)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lines_up_two_texts_by_their_lines() {
        let mut compare = TextCompare::default();
        by_lines("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n", &mut compare);
        assert_eq!(compare.rows, [[Some(0), Some(0)], [Some(1), Some(1)], [Some(2), Some(2)], [Some(3), Some(3)], [None, Some(4)]]);
        assert_eq!(compare.left_changes.keys().copied().collect::<Vec<_>>(), [1]);
        let mut right: Vec<u32> = compare.right_changes.keys().copied().collect();
        right.sort();
        assert_eq!(right, [1, 4]);
    }

    #[test]
    fn counts_difftastics_bytes_as_the_page_does() {
        assert_eq!(utf16_at("let été = 1;", 4), 4);
        // "été" is 5 bytes and 3 UTF-16 units; "😀" 4 bytes and 2 units.
        assert_eq!(utf16_at("let été = 1;", 9), 7);
        assert_eq!(utf16_at("😀x", 4), 2);
        assert_eq!(utf16_at("ab", 99), 2);
    }

    #[test]
    fn finds_where_bytes_differ() {
        let dir = std::env::temp_dir().join(format!("glint-filediff-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a.bin"), dir.join("b.bin"), dir.join("c.bin"));
        let mut big = vec![7u8; 3 << 20];
        fs::write(&a, &big).unwrap();
        big[(2 << 20) + 5] = 8;
        fs::write(&b, &big).unwrap();
        fs::write(&c, &big[..100]).unwrap();
        let (a, b, c) = (a.to_string_lossy(), b.to_string_lossy(), c.to_string_lossy());
        assert_eq!(next_difference(&a, &b, 0).unwrap(), Some((2 << 20) + 5));
        assert_eq!(next_difference(&a, &b, (2 << 20) + 6).unwrap(), None);
        assert_eq!(next_difference(&a, &c, 0).unwrap(), Some(100));
        assert_eq!(next_difference(&a, &a, 0).unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compares_code_by_its_structure_with_difftastic() {
        if crate::proc::locate("difft").is_none() {
            eprintln!("Difftastic isn't installed: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("glint-difftastic-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.rs"), dir.join("b.rs"));
        fs::write(&a, "fn main() {\n    let name = \"world\";\n    for i in 0..3 {\n        println!(\"{i}\");\n    }\n}\n").unwrap();
        fs::write(&b, "fn main() {\n    let name = \"Glint\";\n    for i in 0..3 { println!(\"{i}\"); }\n}\n").unwrap();
        let compare = compare_text(&a.to_string_lossy(), &b.to_string_lossy()).unwrap();
        assert_eq!((compare.engine.as_str(), compare.language.as_deref()), ("difftastic", Some("Rust")));
        // Only the string changed; the loop on one line is the same code.
        assert_eq!(compare.left_changes.get(&1), Some(&vec![[15, 22]]));
        assert_eq!(compare.right_changes.get(&1), Some(&vec![[15, 22]]));
        assert!(!compare.left_changes.contains_key(&2) && !compare.right_changes.contains_key(&2));
        assert!(!compare.left_changes.contains_key(&3), "the loop's body, on a line of its own on the left, is the same code");
        let _ = fs::remove_dir_all(&dir);
    }
}
