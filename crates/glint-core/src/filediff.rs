//! Compare two files: text side by side, by Difftastic's structural diff when it's installed (what
//! changed in the code, not its layout) or a line diff otherwise; and bytes, a window of each at a
//! time with the next place they differ.

use crate::fsx::{blocking, ms_since};
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
#[derive(Debug, Default)]
pub struct TextCompare {
    pub left: Vec<String>,
    pub right: Vec<String>,
    /// Whether each file uses CRLF line endings on disk.
    pub left_crlf: bool,
    pub right_crlf: bool,
    /// Whether each file ends with a newline on disk.
    pub left_newline: bool,
    pub right_newline: bool,
    /// Whether each side is UTF-8. One that isn't is shown with `�` where it isn't, and isn't saved
    /// from here: writing it back would turn every such byte into that.
    pub left_utf8: bool,
    pub right_utf8: bool,
    /// Each file as read from disk ([`content_hash`]), so a save can tell it hasn't changed since;
    /// None for a side compared as edited, or too big to compare as text.
    pub left_hash: Option<String>,
    pub right_hash: Option<String>,
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum IgnoreWhitespace {
    #[default]
    None,
    Trailing,
    All,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CompareOptions {
    pub ignore_whitespace: IgnoreWhitespace,
    pub ignore_case: bool,
    pub line_mode: bool,
}

/// What a file holds, to tell later whether it changed: its BLAKE3, in hex.
pub fn content_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
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

fn normalize_line(line: &str, options: CompareOptions) -> String {
    let trimmed = match options.ignore_whitespace {
        IgnoreWhitespace::None => line.to_string(),
        IgnoreWhitespace::Trailing => line.trim_end().to_string(),
        IgnoreWhitespace::All => line.chars().filter(|c| !c.is_whitespace()).collect(),
    };
    if options.ignore_case {
        trimmed.to_lowercase()
    } else {
        trimmed
    }
}

fn tokenize_line(line: &str) -> Vec<(usize, usize)> {
    let mut tokens = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        let mut end = start + ch.len_utf8();
        if ch.is_alphanumeric() || ch == '_' {
            while let Some(&(_, next)) = chars.peek() {
                if next.is_alphanumeric() || next == '_' {
                    end += next.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        } else if ch.is_whitespace() {
            while let Some(&(_, next)) = chars.peek() {
                if next.is_whitespace() {
                    end += next.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
        tokens.push((start, end));
    }
    tokens
}

fn chars_eq(a: char, b: char, ignore_case: bool) -> bool {
    if a == b {
        true
    } else if ignore_case {
        a.to_lowercase().eq(b.to_lowercase())
    } else {
        false
    }
}

fn tokens_eq(a: &str, b: &str, options: CompareOptions) -> bool {
    if options.ignore_whitespace == IgnoreWhitespace::All && a.chars().all(char::is_whitespace) && b.chars().all(char::is_whitespace) {
        return true;
    }
    if !options.ignore_case {
        a == b
    } else {
        a.chars().count() == b.chars().count() && a.chars().zip(b.chars()).all(|(x, y)| chars_eq(x, y, true))
    }
}

/// A byte range `[start, end)` in a line.
type Span = (usize, usize);

/// Refines a paired unmatched byte range `(ls..le)` and `(rs..re)` by trimming shared prefix/suffix
/// characters when the two slices are closely related (e.g. a single-character edit inside an identifier).
fn refine_pair(left: &str, right: &str, (ls, le): Span, (rs, re): Span, ignore_case: bool) -> (Option<Span>, Option<Span>, bool) {
    let (lt, rt) = (&left[ls..le], &right[rs..re]);
    let mut p_chars = 0usize;
    let mut p_l = 0usize;
    let mut p_r = 0usize;
    for (lc, rc) in lt.chars().zip(rt.chars()) {
        if chars_eq(lc, rc, ignore_case) {
            p_chars += 1;
            p_l += lc.len_utf8();
            p_r += rc.len_utf8();
        } else {
            break;
        }
    }
    let (lt_rem, rt_rem) = (&lt[p_l..], &rt[p_r..]);
    let mut s_chars = 0usize;
    let mut s_l = 0usize;
    let mut s_r = 0usize;
    for (lc, rc) in lt_rem.chars().rev().zip(rt_rem.chars().rev()) {
        if chars_eq(lc, rc, ignore_case) {
            s_chars += 1;
            s_l += lc.len_utf8();
            s_r += rc.len_utf8();
        } else {
            break;
        }
    }
    let max_chars = lt.chars().count().max(rt.chars().count());
    if max_chars > 0 && (p_chars + s_chars >= 3 || (p_chars + s_chars) * 2 >= max_chars) {
        let nl = (ls + p_l, le - s_l);
        let nr = (rs + p_r, re - s_r);
        (
            (nl.0 < nl.1).then_some(nl),
            (nr.0 < nr.1).then_some(nr),
            p_chars + s_chars > 0,
        )
    } else {
        ((ls < le).then_some((ls, le)), (rs < re).then_some((rs, re)), false)
    }
}

/// Word and character diff spans `[start, end)` in UTF-16 units for two paired modified lines.
/// Returns empty vecs when the whole lines are unrelated (so the whole line is tinted without noisy sub-marks).
fn line_spans(left: &str, right: &str, options: CompareOptions) -> (Vec<[u32; 2]>, Vec<[u32; 2]>) {
    let (left_eff, right_eff) = if options.ignore_whitespace == IgnoreWhitespace::Trailing {
        (left.trim_end(), right.trim_end())
    } else {
        (left, right)
    };
    let (ltoks, rtoks) = (tokenize_line(left_eff), tokenize_line(right_eff));
    if ltoks.is_empty() || rtoks.is_empty() || ltoks.len() * rtoks.len() > 40_000 {
        return (Vec::new(), Vec::new());
    }
    let (n, m) = (ltoks.len(), rtoks.len());
    let mut dp = vec![0u16; (n + 1) * (m + 1)];
    let stride = m + 1;
    for i in (0..n).rev() {
        let lt = &left_eff[ltoks[i].0..ltoks[i].1];
        for j in (0..m).rev() {
            let rt = &right_eff[rtoks[j].0..rtoks[j].1];
            dp[i * stride + j] = if tokens_eq(lt, rt, options) {
                dp[(i + 1) * stride + (j + 1)] + 1
            } else {
                dp[(i + 1) * stride + j].max(dp[i * stride + (j + 1)])
            };
        }
    }
    let mut matches = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut meaningful_match = false;
    while i < n && j < m {
        let lt = &left_eff[ltoks[i].0..ltoks[i].1];
        let rt = &right_eff[rtoks[j].0..rtoks[j].1];
        if tokens_eq(lt, rt, options) {
            if !lt.chars().all(char::is_whitespace) {
                meaningful_match = true;
            }
            matches.push((i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * stride + j] >= dp[i * stride + (j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    matches.push((n, m));

    let mut l_spans: Vec<[u32; 2]> = Vec::new();
    let mut r_spans: Vec<[u32; 2]> = Vec::new();
    let mut prev_l = 0usize;
    let mut prev_r = 0usize;
    for (mi, mj) in matches {
        let l_range = (prev_l < mi).then(|| (ltoks[prev_l].0, ltoks[mi - 1].1));
        let r_range = (prev_r < mj).then(|| (rtoks[prev_r].0, rtoks[mj - 1].1));
        let (l_range, r_range) = match (l_range, r_range) {
            (Some(lr), Some(rr)) => {
                let (nl, nr, refined) = refine_pair(left_eff, right_eff, lr, rr, options.ignore_case);
                if refined {
                    meaningful_match = true;
                }
                (nl, nr)
            }
            other => other,
        };
        let push_span = |spans: &mut Vec<[u32; 2]>, text: &str, range: Option<(usize, usize)>| {
            let Some((s, e)) = range else { return };
            if options.ignore_whitespace == IgnoreWhitespace::All && text[s..e].chars().all(char::is_whitespace) {
                return;
            }
            let (u_start, u_end) = (utf16_at(text, s), utf16_at(text, e));
            if u_start < u_end {
                if let Some(last) = spans.last_mut().filter(|last| last[1] >= u_start) {
                    last[1] = last[1].max(u_end);
                } else {
                    spans.push([u_start, u_end]);
                }
            }
        };
        push_span(&mut l_spans, left_eff, l_range);
        push_span(&mut r_spans, right_eff, r_range);
        prev_l = mi + 1;
        prev_r = mj + 1;
    }

    if !meaningful_match {
        return (Vec::new(), Vec::new());
    }
    (l_spans, r_spans)
}

/// The rows and changed lines from a line diff: what's the same pairs up, a change's lines pair up
/// as far as both sides have them.
fn by_lines(left: &str, right: &str, options: CompareOptions, compare: &mut TextCompare) {
    use gix::diff::blob::{Algorithm, Diff, InternedInput};
    if compare.left.is_empty() && !left.is_empty() {
        compare.left = left.lines().map(str::to_string).collect();
    }
    if compare.right.is_empty() && !right.is_empty() {
        compare.right = right.lines().map(str::to_string).collect();
    }
    let norm_a: String;
    let norm_b: String;
    let (a_str, b_str) = if options.ignore_whitespace == IgnoreWhitespace::None && !options.ignore_case {
        (left, right)
    } else {
        norm_a = compare.left.iter().map(|l| format!("{}\n", normalize_line(l, options))).collect();
        norm_b = compare.right.iter().map(|r| format!("{}\n", normalize_line(r, options))).collect();
        (norm_a.as_str(), norm_b.as_str())
    };
    let input = InternedInput::new(a_str, b_str);
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
            let (l_spans, r_spans) = match (l, r) {
                (Some(li), Some(ri)) => match (compare.left.get(li as usize), compare.right.get(ri as usize)) {
                    (Some(lt), Some(rt)) => line_spans(lt, rt, options),
                    _ => (Vec::new(), Vec::new()),
                },
                _ => (Vec::new(), Vec::new()),
            };
            if let Some(l) = l {
                compare.left_changes.insert(l, l_spans);
            }
            if let Some(r) = r {
                compare.right_changes.insert(r, r_spans);
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

fn compare_text(left: &str, right: &str, options: CompareOptions, left_override: Option<String>, right_override: Option<String>) -> Result<TextCompare, String> {
    let started = Instant::now();
    let mut compare = TextCompare::default();
    let has_override = left_override.is_some() || right_override.is_some();
    let from_disk = (left_override.is_none(), right_override.is_none());
    let ((left_bytes, left_big), (right_bytes, right_big)) = (
        match left_override {
            Some(text) => (text.into_bytes(), false),
            None => read_capped(left)?,
        },
        match right_override {
            Some(text) => (text.into_bytes(), false),
            None => read_capped(right)?,
        },
    );
    compare.too_big = left_big || right_big;
    compare.left_crlf = left_bytes.windows(2).any(|w| w == b"\r\n");
    compare.right_crlf = right_bytes.windows(2).any(|w| w == b"\r\n");
    compare.left_newline = left_bytes.ends_with(b"\n");
    compare.right_newline = right_bytes.ends_with(b"\n");
    compare.left_utf8 = std::str::from_utf8(&left_bytes).is_ok();
    compare.right_utf8 = std::str::from_utf8(&right_bytes).is_ok();
    compare.left_hash = (from_disk.0 && !left_big).then(|| content_hash(&left_bytes));
    compare.right_hash = (from_disk.1 && !right_big).then(|| content_hash(&right_bytes));
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
    let use_difftastic = !has_override && !options.line_mode && options.ignore_whitespace == IgnoreWhitespace::None && !options.ignore_case;
    if !use_difftastic || !by_difftastic(left, right, &mut compare) {
        (compare.rows, compare.left_changes, compare.right_changes) = Default::default();
        by_lines(&left_text, &right_text, options, &mut compare);
    }
    compare.elapsed = ms_since(started);
    Ok(compare)
}

/// Two text files side by side; `binary` or `tooBig` when they're to be compared by their bytes.
pub async fn compare_text_files(
    left: String,
    right: String,
    options: Option<CompareOptions>,
    left_override: Option<String>,
    right_override: Option<String>,
) -> Result<TextCompare, String> {
    blocking(move || compare_text(&left, &right, options.unwrap_or_default(), left_override, right_override)).await
}

fn read_at(path: &str, offset: u64, len: usize) -> Result<(u64, Vec<u8>), String> {
    let mut file = File::open(path).map_err(|e| format!("Couldn't open {path}: {e}"))?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    file.seek(SeekFrom::Start(offset.min(size))).map_err(|e| e.to_string())?;
    let mut bytes = Vec::with_capacity(len);
    file.take(len as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok((size, bytes))
}

/// A window of both files' bytes: their sizes, and their bytes from the same offset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BytesWindow {
    pub left_size: u64,
    pub right_size: u64,
    pub left: Vec<u8>,
    pub right: Vec<u8>,
}

/// `len` bytes of each file from `offset` (at most 1 MB), for the hex view.
pub async fn compare_bytes(left: String, right: String, offset: u64, len: u32) -> Result<BytesWindow, String> {
    blocking(move || {
        let len = len.min(1 << 20) as usize;
        let ((left_size, left), (right_size, right)) = (read_at(&left, offset, len)?, read_at(&right, offset, len)?);
        Ok(BytesWindow { left_size, right_size, left, right })
    })
    .await
}

/// `len` bytes of a file from `offset` (at most 1 MB), for the preview's hex view, and its size.
pub async fn file_bytes(path: String, offset: u64, len: u32) -> Result<(u64, Vec<u8>), String> {
    blocking(move || read_at(&path, offset, len.min(1 << 20) as usize)).await
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

/// The last offset before `before` where the files differ (the first differing byte in that 16-byte
/// row); None when they're the same up to `before`.
fn prev_difference(left: &str, right: &str, before: u64) -> Result<Option<u64>, String> {
    if before == 0 {
        return Ok(None);
    }
    let mut a = File::open(left).map_err(|e| format!("Couldn't open {left}: {e}"))?;
    let mut b = File::open(right).map_err(|e| format!("Couldn't open {right}: {e}"))?;
    let (size_a, size_b) = (a.metadata().map_err(|e| e.to_string())?.len(), b.metadata().map_err(|e| e.to_string())?.len());
    let max_size = size_a.max(size_b);
    let mut end = before.min(max_size);
    if end == 0 {
        return Ok(None);
    }
    let chunk_size = 1usize << 20;
    let (mut x, mut y) = (vec![0u8; chunk_size], vec![0u8; chunk_size]);
    let fill = |file: &mut File, size: u64, start: u64, len: usize, buf: &mut [u8]| -> Result<usize, String> {
        if start >= size {
            return Ok(0);
        }
        file.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
        let want = len.min((size - start) as usize);
        let mut got = 0;
        while got < want {
            match file.read(&mut buf[got..want]).map_err(|e| e.to_string())? {
                0 => break,
                n => got += n,
            }
        }
        Ok(got)
    };
    while end > 0 {
        let start = end.saturating_sub(chunk_size as u64) & !15;
        let len = (end - start) as usize;
        let (n, m) = (fill(&mut a, size_a, start, len, &mut x)?, fill(&mut b, size_b, start, len, &mut y)?);
        let limit = len.min(n.max(m));
        if let Some(i) = (0..limit).rev().find(|&i| x.get(i).filter(|_| i < n) != y.get(i).filter(|_| i < m)) {
            let row_start = i & !15;
            let first_in_row = (row_start..=i).find(|&k| x.get(k).filter(|_| k < n) != y.get(k).filter(|_| k < m)).unwrap_or(i);
            return Ok(Some(start + first_in_row as u64));
        }
        end = start;
    }
    Ok(None)
}

pub async fn next_byte_difference(left: String, right: String, from: u64) -> Result<Option<u64>, String> {
    blocking(move || next_difference(&left, &right, from)).await
}

pub async fn prev_byte_difference(left: String, right: String, before: u64) -> Result<Option<u64>, String> {
    blocking(move || prev_difference(&left, &right, before)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lines_up_two_texts_by_their_lines() {
        let mut compare = TextCompare::default();
        by_lines("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n", CompareOptions::default(), &mut compare);
        assert_eq!(compare.rows, [[Some(0), Some(0)], [Some(1), Some(1)], [Some(2), Some(2)], [Some(3), Some(3)], [None, Some(4)]]);
        assert_eq!(compare.left_changes.keys().copied().collect::<Vec<_>>(), [1]);
        let mut right: Vec<u32> = compare.right_changes.keys().copied().collect();
        right.sort();
        assert_eq!(right, [1, 4]);
    }

    #[test]
    fn marks_changed_words_and_characters_within_paired_lines() {
        let mut compare = TextCompare::default();
        by_lines(
            "let name = \"world\";\nlet version = \"v0.8.1\";\n",
            "let name = \"Glint\";\nlet version = \"v0.8.2\";\n",
            CompareOptions::default(),
            &mut compare,
        );
        // "world" vs "Glint" at UTF-16 [12..17]
        assert_eq!(compare.left_changes.get(&0), Some(&vec![[12, 17]]));
        assert_eq!(compare.right_changes.get(&0), Some(&vec![[12, 17]]));
        // "1" vs "2" at UTF-16 [20..21]
        assert_eq!(compare.left_changes.get(&1), Some(&vec![[20, 21]]));
        assert_eq!(compare.right_changes.get(&1), Some(&vec![[20, 21]]));
    }

    #[test]
    fn ignores_whitespace_and_case_when_asked() {
        let mut compare = TextCompare::default();
        by_lines(
            "Hello World   \nfoo  bar\n",
            "hello world\nfoo bar\n",
            CompareOptions {
                ignore_whitespace: IgnoreWhitespace::All,
                ignore_case: true,
                line_mode: true,
            },
            &mut compare,
        );
        assert!(compare.left_changes.is_empty() && compare.right_changes.is_empty());
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
        assert_eq!(prev_difference(&a, &b, 3 << 20).unwrap(), Some((2 << 20) + 5));
        assert_eq!(prev_difference(&a, &b, (2 << 20) & !15).unwrap(), None);
        assert_eq!(next_difference(&a, &c, 0).unwrap(), Some(100));
        assert_eq!(prev_difference(&a, &c, 112).unwrap(), Some(100));
        assert_eq!(next_difference(&a, &a, 0).unwrap(), None);
        assert_eq!(prev_difference(&a, &a, 3 << 20).unwrap(), None);
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
        let compare = compare_text(&a.to_string_lossy(), &b.to_string_lossy(), CompareOptions::default(), None, None).unwrap();
        assert_eq!((compare.engine.as_str(), compare.language.as_deref()), ("difftastic", Some("Rust")));
        // Only the string changed; the loop on one line is the same code.
        assert_eq!(compare.left_changes.get(&1), Some(&vec![[15, 22]]));
        assert_eq!(compare.right_changes.get(&1), Some(&vec![[15, 22]]));
        assert!(!compare.left_changes.contains_key(&2) && !compare.right_changes.contains_key(&2));
        assert!(!compare.left_changes.contains_key(&3), "the loop's body, on a line of its own on the left, is the same code");
        let _ = fs::remove_dir_all(&dir);
    }
}
