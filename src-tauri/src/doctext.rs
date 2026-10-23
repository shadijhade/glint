//! Text inside documents, for Text mode's Documents search: a PDF's pages (pdf-extract), Word's,
//! PowerPoint's and OpenDocument's paragraphs and slides (their XML), spreadsheets' rows (calamine) and
//! an EPUB's chapters, each line with where it is in its document ("p. 3", "slide 2", "Sales, row 7").
//!
//! A document can be anything, and a PDF reader can panic, overflow its stack or loop on a bad one, so
//! they're read in workers: Glint itself, started with --doc-worker, each in a job object (1 GB, no
//! child processes), a file at a time, given up on after 30 seconds. Their text is kept in memory for
//! the session (256 MB of it), by path, size and date, so searching the same documents again reads
//! none, and nothing of it is written to disk.

use crate::jq::Job;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const WORKER_ARG: &str = "--doc-worker";
/// What the Documents search reads.
pub const DOCUMENTS: &[&str] = &["pdf", "docx", "docm", "dotx", "dotm", "pptx", "pptm", "ppsx", "potx", "xlsx", "xlsm", "xlsb", "xls", "ods", "odt", "ott", "odp", "epub"];
/// A document bigger than this isn't read.
const MAX_FILE: u64 = 256 << 20;
/// A part of a document (an XML file in its zip) bigger than this isn't read.
const MAX_PART: u64 = 64 << 20;
/// A document's text past this is left out.
const MAX_TEXT: usize = 32 << 20;
/// A sheet's rows past this are left out.
const MAX_SHEET_ROWS: usize = 100_000;
/// A line longer than this is cut into lines this long (Results show a line's first 2,000 characters).
const MAX_LINE: usize = 2000;
const FILE_TIMEOUT: Duration = Duration::from_secs(30);
const MEMORY: usize = 1 << 30;
const CACHE_BUDGET: usize = 256 << 20;
/// Cloud placeholders (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS): reading one
/// would download it, so the search leaves them.
const ONLINE_ONLY: u32 = 0x0000_1000 | 0x0040_0000;

/// A document's text: its lines, each with where it is in it.
pub type Lines = Vec<(String, String)>;

// The worker: reads a path a line on its standard input, answers a line of JSON for each.

#[derive(Serialize, Deserialize, Default)]
struct Answer {
    #[serde(default)]
    lines: Lines,
    #[serde(default)]
    error: Option<String>,
}

/// Glint started as a document reader: answers each path on its standard input with the document's
/// lines, until its input ends; before anything of the app starts (main.rs).
pub fn worker() -> Option<i32> {
    if std::env::args_os().nth(1).as_deref() != Some(OsStr::new(WORKER_ARG)) {
        return None;
    }
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for path in stdin.lock().lines().map_while(Result::ok) {
        let answer = match read(path.trim()) {
            Ok(lines) => Answer { lines, error: None },
            Err(e) => Answer { lines: Vec::new(), error: Some(e) },
        };
        let mut json = serde_json::to_vec(&answer).unwrap_or_else(|_| b"{}".to_vec());
        json.push(b'\n');
        if out.write_all(&json).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
    Some(0)
}

fn extension(path: &str) -> String {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// A document's lines, read as its kind is.
pub fn read(path: &str) -> Result<Lines, String> {
    match std::fs::metadata(path).map_err(|e| e.to_string())?.len() {
        // Office's own blank documents (its New menu's) are empty files.
        0 => return Ok(Vec::new()),
        n if n > MAX_FILE => return Err(format!("over {} MB", MAX_FILE >> 20)),
        _ => {}
    }
    let lines = match extension(path).as_str() {
        "pdf" => pdf(path)?,
        "docx" | "docm" | "dotx" | "dotm" => numbered(paragraphs(&part(path, "word/document.xml")?, &["p"], Some(&["t"]))),
        "pptx" | "pptm" | "ppsx" | "potx" => slides(path)?,
        "odt" | "ott" => numbered(paragraphs(&part(path, "content.xml")?, &["p", "h"], None)),
        "odp" => odf_slides(&part(path, "content.xml")?),
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => sheets(path)?,
        "epub" => epub(path)?,
        // Images too (OCR): the text Windows reads in a picture, a line each.
        ext if crate::ocr::PICTURES.contains(&ext) => crate::ocr::lines(path)?.into_iter().enumerate().map(|(i, line)| (format!("line {}", i + 1), line)).collect(),
        other => return Err(format!("not a document Glint reads: .{other}")),
    };
    let mut total = 0;
    Ok(lines
        .into_iter()
        .flat_map(|(label, text)| cut(label, text))
        .take_while(|(label, text)| {
            total += label.len() + text.len();
            total <= MAX_TEXT
        })
        .collect())
}

/// A long line, cut into lines Results can show.
fn cut(label: String, text: String) -> Vec<(String, String)> {
    if text.len() <= MAX_LINE {
        return vec![(label, text)];
    }
    let mut parts = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut at = rest.len().min(MAX_LINE);
        while !rest.is_char_boundary(at) {
            at -= 1;
        }
        // At a space, where there's one near the end.
        if at < rest.len() {
            if let Some(space) = rest[..at].rfind(' ').filter(|&s| s > at / 2) {
                at = space + 1;
            }
        }
        parts.push((label.clone(), rest[..at].trim().to_string()));
        rest = &rest[at..];
    }
    parts
}

fn pdf(path: &str) -> Result<Lines, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let pages = pdf_extract::extract_text_from_mem_by_pages(&bytes).map_err(|e| e.to_string())?;
    Ok(pages
        .iter()
        .enumerate()
        .flat_map(|(i, page)| page.lines().map(str::trim).filter(|l| !l.is_empty()).map(move |l| (format!("p. {}", i + 1), l.to_string())))
        .collect())
}

/// A file inside a zip-based document, as text.
fn part(path: &str, member: &str) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut entry = archive.by_name(member).map_err(|e| format!("{member}: {e}"))?;
    let mut text = String::new();
    (&mut entry).take(MAX_PART).read_to_string(&mut text).map_err(|e| e.to_string())?;
    Ok(text)
}

/// The text an entity reference stands for: a character's, XML's own five, or a space for HTML's
/// no-break space (EPUBs use it).
fn entity(name: &str) -> Option<String> {
    if let Ok(Some(c)) = quick_xml::events::BytesRef::new(name).resolve_char_ref() {
        return Some(c.to_string());
    }
    let text = match name {
        "amp" => "&",
        "lt" => "<",
        "gt" => ">",
        "quot" => "\"",
        "apos" => "'",
        "nbsp" => " ",
        _ => return None,
    };
    Some(text.to_string())
}

/// The text of each paragraph (`paras`, by local name) in XML, the text taken from inside `texts`
/// elements (Word's w:t, PowerPoint's a:t) or, without them, all of it (OpenDocument's, XHTML's); tabs
/// and line breaks as spaces.
fn paragraphs(xml: &str, paras: &[&str], texts: Option<&[&str]>) -> Vec<String> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    let mut out = Vec::new();
    // The paragraph being read, and how deep in paragraphs (a table cell's inside another's).
    let mut current = String::new();
    let mut depth = 0usize;
    let mut inside_text = 0usize;
    let taken = |inside: usize| texts.is_none() || inside > 0;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = e.local_name().into_inner();
                if paras.contains(&name) {
                    depth += 1;
                } else if texts.is_some_and(|t| t.contains(&name)) {
                    inside_text += 1;
                }
            }
            Ok(Event::Empty(e)) => {
                // Word's w:tab and w:br, OpenDocument's text:s, text:tab and text:line-break, XHTML's br.
                if depth > 0 && matches!(e.local_name().into_inner(), "tab" | "br" | "s" | "line-break") {
                    current.push(' ');
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name().into_inner();
                if paras.contains(&name) && depth > 0 {
                    depth -= 1;
                    let text = current.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !text.is_empty() {
                        out.push(text);
                    }
                    current.clear();
                } else if texts.is_some_and(|t| t.contains(&name)) {
                    inside_text = inside_text.saturating_sub(1);
                }
            }
            Ok(Event::Text(t)) if depth > 0 && taken(inside_text) => current.push_str(&t.into_inner()),
            Ok(Event::CData(t)) if depth > 0 && taken(inside_text) => current.push_str(&t.into_inner()),
            Ok(Event::GeneralRef(r)) if depth > 0 && taken(inside_text) => {
                if let Some(s) = entity(&r.into_inner()) {
                    current.push_str(&s);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// Paragraphs labelled by their number ("¶ 12").
fn numbered(paragraphs: Vec<String>) -> Lines {
    paragraphs.into_iter().enumerate().map(|(i, p)| (format!("¶ {}", i + 1), p)).collect()
}

fn slides(path: &str) -> Result<Lines, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut numbers: Vec<u32> = archive
        .file_names()
        .filter_map(|name| {
            let name = name.ok()?;
            name.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse().ok()
        })
        .collect();
    numbers.sort_unstable();
    let mut lines = Vec::new();
    for n in numbers {
        let xml = part(path, &format!("ppt/slides/slide{n}.xml"))?;
        lines.extend(paragraphs(&xml, &["p"], Some(&["t"])).into_iter().map(|p| (format!("slide {n}"), p)));
    }
    Ok(lines)
}

/// An OpenDocument presentation's paragraphs, by the page (slide) they're on.
fn odf_slides(xml: &str) -> Lines {
    use quick_xml::events::Event;
    // Each page's own XML, so `paragraphs` reads one at a time.
    let mut lines = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    let (mut page, mut start) = (0, None);
    loop {
        let before = reader.buffer_position() as usize;
        match reader.read_event() {
            Ok(Event::Start(e)) if e.local_name().into_inner() == "page" => {
                page += 1;
                start = Some(before);
            }
            Ok(Event::End(e)) if e.local_name().into_inner() == "page" => {
                if let Some(from) = start.take() {
                    let slice = xml.get(from..reader.buffer_position() as usize).unwrap_or("");
                    lines.extend(paragraphs(slice, &["p", "h"], None).into_iter().map(|p| (format!("slide {page}"), p)));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    lines
}

/// A workbook's rows, a line each: its cells with two spaces between, labelled "Sheet, row 7".
fn sheets(path: &str) -> Result<Lines, String> {
    let mut lines = Vec::new();
    for table in crate::sheet::load_workbook(path, MAX_SHEET_ROWS)? {
        let label = |row: u32| format!("{}, row {row}", table.name);
        if table.header {
            lines.push((label(table.first - 1), table.columns.join("  ")));
        }
        for (i, row) in table.rows.iter().enumerate() {
            let text = row.iter().map(crate::sheet::Cell::display).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("  ");
            if !text.is_empty() {
                lines.push((label(table.first + i as u32), text));
            }
        }
    }
    Ok(lines)
}

/// An attribute's value, by its local name.
fn attribute(e: &quick_xml::events::BytesStart, name: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().into_inner() == name).and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok()).map(|v| v.into_owned())
}

/// An EPUB's chapters in reading order: the package file META-INF/container.xml names, its manifest
/// (id → file) and spine (the order).
fn epub(path: &str) -> Result<Lines, String> {
    use quick_xml::events::Event;
    let container = part(path, "META-INF/container.xml")?;
    let mut reader = quick_xml::Reader::from_str(&container);
    let mut package = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().into_inner() == "rootfile" => {
                package = attribute(&e, "full-path");
                break;
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    let package = package.ok_or("no package file named in META-INF/container.xml")?;
    let base = package.rsplit_once('/').map(|(dir, _)| format!("{dir}/")).unwrap_or_default();
    let opf = part(path, &package)?;
    let (mut manifest, mut spine) = (HashMap::new(), Vec::new());
    let mut reader = quick_xml::Reader::from_str(&opf);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => match e.local_name().into_inner() {
                "item" => {
                    if let (Some(id), Some(href)) = (attribute(&e, "id"), attribute(&e, "href")) {
                        manifest.insert(id, href);
                    }
                }
                "itemref" => spine.extend(attribute(&e, "idref")),
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    // A div's own text too (some books have no paragraphs); the paragraphs in it are their own.
    const BLOCKS: &[&str] = &["p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "dt", "dd", "pre", "td", "th", "figcaption", "caption"];
    let mut lines = Vec::new();
    for (n, id) in spine.iter().enumerate() {
        let Some(href) = manifest.get(id) else { continue };
        let href = href.split('#').next().unwrap_or(href);
        // Hrefs are URLs: "Chapter%201.xhtml" is "Chapter 1.xhtml" in the zip.
        let Ok(xhtml) = part(path, &format!("{base}{}", unescape_url(href))) else { continue };
        lines.extend(paragraphs(&xhtml, BLOCKS, None).into_iter().map(|p| (format!("chapter {}", n + 1), p)));
    }
    Ok(lines)
}

/// %XX escapes in a URL path, decoded.
fn unescape_url(href: &str) -> String {
    let bytes = href.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1).copied().and_then(hex), bytes.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// The search: documents found in a folder, read by workers (or from memory), matched here.

/// A Documents search as Text mode asks for it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocSearch {
    pub query: String,
    pub path: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub exclusions: Vec<String>,
    #[serde(default)]
    pub everything: bool,
    /// Pictures too: their text, read by Windows' OCR (ocr.rs).
    #[serde(default)]
    pub ocr: bool,
}

/// The pattern as ripgrep would take it: smart case unless case-sensitive, a literal unless a regular
/// expression, whole words when asked.
pub fn pattern(search: &DocSearch) -> Result<regex::Regex, String> {
    let body = if search.regex { search.query.clone() } else { regex::escape(&search.query) };
    let body = if search.whole_word { format!(r"\b(?:{body})\b") } else { body };
    let insensitive = !search.case_sensitive && !search.query.chars().any(char::is_uppercase);
    regex::RegexBuilder::new(&body).case_insensitive(insensitive).size_limit(16 << 20).build().map_err(|e| e.to_string())
}

/// Whether a name matches a pattern with * and ?, ignoring case, as an exclusion does.
fn glob(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().flat_map(char::to_lowercase).collect();
    let n: Vec<char> = name.chars().flat_map(char::to_lowercase).collect();
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if let Some((s, mark)) = star {
            // The star takes one more character.
            star = Some((s, mark + 1));
            pi = s + 1;
            ni = mark + 1;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

type Key = (String, u64, Option<SystemTime>);

/// Texts read this session, by path, size and date, the least recently used dropped past the budget.
struct Cache {
    texts: HashMap<Key, (Arc<Lines>, u64)>,
    bytes: usize,
    clock: u64,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn key_of(path: &Path) -> Option<Key> {
    let meta = std::fs::metadata(path).ok()?;
    Some((path.to_string_lossy().to_lowercase(), meta.len(), meta.modified().ok()))
}

fn weight(lines: &Lines) -> usize {
    lines.iter().map(|(l, t)| l.len() + t.len() + 48).sum()
}

fn cached(key: &Key) -> Option<Arc<Lines>> {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.as_mut()?;
    cache.clock += 1;
    let clock = cache.clock;
    cache.texts.get_mut(key).map(|(lines, used)| {
        *used = clock;
        lines.clone()
    })
}

fn remember(key: Key, lines: Arc<Lines>) {
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = guard.get_or_insert_with(|| Cache { texts: HashMap::new(), bytes: 0, clock: 0 });
    cache.clock += 1;
    cache.bytes += weight(&lines);
    if let Some((old, _)) = cache.texts.insert(key, (lines, cache.clock)) {
        cache.bytes = cache.bytes.saturating_sub(weight(&old));
    }
    while cache.bytes > CACHE_BUDGET && cache.texts.len() > 1 {
        let Some(oldest) = cache.texts.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone()) else { break };
        if let Some((lines, _)) = cache.texts.remove(&oldest) {
            cache.bytes = cache.bytes.saturating_sub(weight(&lines));
        }
    }
}

/// A document's lines `from` to `to` (1-based), as the search read them, for the preview: from
/// memory, or read again.
pub fn lines_of(path: &str, from: usize, to: usize) -> Result<Lines, String> {
    let key = key_of(Path::new(path)).ok_or("This document isn't there anymore.")?;
    let lines = match cached(&key) {
        Some(lines) => lines,
        None => {
            let lines = Arc::new(Reader::start()?.read(Path::new(path), None).map_err(|why| why.unwrap_or_else(|| "Reading this document failed or took too long.".into()))?);
            remember(key, lines.clone());
            lines
        }
    };
    Ok(lines.iter().skip(from.saturating_sub(1)).take((to + 1).saturating_sub(from)).cloned().collect())
}

/// Bumped by each Documents search and Stop: an older search's workers stop.
static RUN: AtomicU64 = AtomicU64::new(0);

/// Starts a Documents search: the one before stops.
pub fn begin() -> u64 {
    RUN.fetch_add(1, Ordering::SeqCst) + 1
}

/// Stops the Documents search running, if any.
pub fn cancel() {
    RUN.fetch_add(1, Ordering::SeqCst);
}

fn current(run: u64) -> bool {
    RUN.load(Ordering::SeqCst) == run
}

/// Stops `run`, unless a newer search already did.
fn stop(run: u64) {
    let _ = RUN.compare_exchange(run, run + 1, Ordering::SeqCst, Ordering::SeqCst);
}

/// Whether Documents search reads a file of this name (Office's ~$ owner files aren't documents), and
/// with `pictures`, a picture's.
fn is_document(name: &str, pictures: bool) -> bool {
    let ext = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    let of = |list: &[&str]| list.iter().any(|d| d.eq_ignore_ascii_case(ext));
    (of(DOCUMENTS) || (pictures && of(crate::ocr::PICTURES))) && !name.starts_with("~$")
}

/// The exclusions as a search applies them: exact names (lowercase), and patterns with * or ?, which
/// only folders are matched against.
struct Skips {
    names: std::collections::HashSet<String>,
    patterns: Vec<String>,
}

impl Skips {
    fn of(search: &DocSearch) -> Self {
        let mut skips = Skips { names: Default::default(), patterns: Vec::new() };
        if !search.everything {
            for x in search.exclusions.iter().map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()) {
                if x.contains(['*', '?']) {
                    skips.patterns.push(x);
                } else {
                    skips.names.insert(x);
                }
            }
        }
        skips
    }

    fn skip(&self, name: &str, is_dir: bool) -> bool {
        if self.names.is_empty() && (!is_dir || self.patterns.is_empty()) {
            return false;
        }
        let name = name.to_lowercase();
        self.names.contains(&name) || (is_dir && self.patterns.iter().any(|p| glob(p, &name)))
    }
}

/// Sends the documents under `folder` to `paths` as it finds them, not going into links, hidden
/// folders or the excluded ones, unless `everything`. File patterns among the exclusions (*.pdf, there
/// to keep binary files out of text searches) don't apply to documents; their exact names do. Cloud
/// placeholders are left, so the search downloads nothing. Other files cost a name check each. Four
/// threads read folders, as fd does (a single one takes three times as long on a big folder).
fn walk(folder: &Path, search: &DocSearch, run: u64, paths: mpsc::SyncSender<PathBuf>) {
    let skips = Skips::of(search);
    if folder.is_file() {
        if folder.file_name().is_some_and(|n| is_document(&n.to_string_lossy(), search.ocr)) {
            let _ = paths.send(folder.to_path_buf());
        }
        return;
    }
    // The folders still to read, and how many are being read: the walk is done when both are none.
    let work = Mutex::new((vec![folder.to_path_buf()], 0usize));
    let changed = std::sync::Condvar::new();
    let next = || {
        let mut work = work.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if !current(run) {
                return None;
            }
            if let Some(dir) = work.0.pop() {
                work.1 += 1;
                return Some(dir);
            }
            if work.1 == 0 {
                return None;
            }
            work = changed.wait_timeout(work, Duration::from_millis(100)).unwrap_or_else(|e| e.into_inner()).0;
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let (paths, next, skips, work, changed) = (paths.clone(), &next, &skips, &work, &changed);
            scope.spawn(move || {
                while let Some(dir) = next() {
                    let mut subfolders = Vec::new();
                    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                        let Ok(kind) = entry.file_type() else { continue };
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        let is_dir = kind.is_dir();
                        if kind.is_symlink() || (!is_dir && !is_document(&name, search.ocr)) || skips.skip(&name, is_dir) {
                            continue;
                        }
                        #[cfg(windows)]
                        let attributes = {
                            use std::os::windows::fs::MetadataExt;
                            entry.metadata().map_or(0, |m| m.file_attributes())
                        };
                        #[cfg(not(windows))]
                        let attributes = 0u32;
                        if (name.starts_with('.') || attributes & 0x2 != 0) && !search.everything {
                            continue;
                        }
                        if is_dir {
                            subfolders.push(entry.path());
                        } else if attributes & ONLINE_ONLY == 0 && paths.send(entry.path()).is_err() {
                            // Nobody reads them anymore: the search stopped.
                            break;
                        }
                    }
                    let mut work = work.lock().unwrap_or_else(|e| e.into_inner());
                    work.0.extend(subfolders);
                    work.1 -= 1;
                    changed.notify_all();
                }
            });
        }
    });
}

/// A worker process and its pipes; dropping it (its job) ends it.
struct Reader {
    _job: Job,
    child: Child,
    stdin: ChildStdin,
    answers: mpsc::Receiver<String>,
}

impl Reader {
    fn start() -> Result<Self, String> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg(WORKER_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
            .spawn()
            .map_err(|e| e.to_string())?;
        // In its job before it's given a document (it reads nothing until then).
        let job = match Job::new(MEMORY).and_then(|job| job.assign(&child).map(|_| job)) {
            Ok(job) => job,
            Err(e) => {
                let _ = child.kill();
                return Err(e);
            }
        };
        let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
            let _ = child.kill();
            return Err("Couldn't talk to the document reader.".into());
        };
        let (tx, answers) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Reader { _job: job, child, stdin, answers })
    }

    /// A document's lines, or why not; Err(None) when the worker died on it, took too long or `run`
    /// stopped (then it's ended, and a new one reads the next).
    fn read(&mut self, path: &Path, run: Option<u64>) -> Result<Lines, Option<String>> {
        let mut line = path.to_string_lossy().into_owned();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).map_err(|_| None)?;
        let started = Instant::now();
        loop {
            match self.answers.recv_timeout(Duration::from_millis(100)) {
                // Only the worker's answers are lines of JSON.
                Ok(answer) => {
                    let Ok(answer) = serde_json::from_str::<Answer>(&answer) else { continue };
                    return match answer.error {
                        Some(e) => Err(Some(e)),
                        None => Ok(answer.lines),
                    };
                }
                Err(RecvTimeoutError::Timeout) if started.elapsed() < FILE_TIMEOUT && run.is_none_or(current) => {}
                Err(_) => return Err(None),
            }
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// A document read, and its lines.
struct Document {
    path: PathBuf,
    lines: Arc<Lines>,
}

/// Reads the documents coming from `queue` (from memory, or with up to four workers), sending each to
/// `found` as it's read, until there are no more or `run` stops; how many couldn't be read.
fn read_all(queue: mpsc::Receiver<PathBuf>, run: u64, found: mpsc::Sender<Document>) -> usize {
    let queue = Mutex::new(queue);
    let failed = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 4));
    let next = || loop {
        match queue.lock().unwrap_or_else(|e| e.into_inner()).recv_timeout(Duration::from_millis(100)) {
            Ok(path) => return Some(path),
            Err(RecvTimeoutError::Timeout) if current(run) => {}
            Err(_) => return None,
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let (found, next, failed) = (found.clone(), &next, &failed);
            scope.spawn(move || {
                let mut reader: Option<Reader> = None;
                while let Some(path) = next() {
                    if !current(run) {
                        break;
                    }
                    let key = key_of(&path);
                    if let Some(lines) = key.as_ref().and_then(cached) {
                        let _ = found.send(Document { path, lines });
                        continue;
                    }
                    if reader.is_none() {
                        reader = Reader::start().ok();
                    }
                    let Some(r) = reader.as_mut() else {
                        failed.fetch_add(1, Ordering::Relaxed);
                        break;
                    };
                    match r.read(&path, Some(run)) {
                        Ok(lines) => {
                            let lines = Arc::new(lines);
                            if let Some(key) = key {
                                remember(key, lines.clone());
                            }
                            let _ = found.send(Document { path, lines });
                        }
                        Err(why) => {
                            if current(run) {
                                failed.fetch_add(1, Ordering::Relaxed);
                            }
                            // The worker is gone, or stuck on it: a new one for the next file.
                            if why.is_none() {
                                reader = None;
                            }
                        }
                    }
                }
            });
        }
    });
    failed.into_inner()
}

/// A match as ripgrep's JSON line of one: its line number the line's place in the document, its label
/// where that is ("p. 3").
fn as_ripgrep(path: &Path, number: usize, label: &str, text: &str, spans: &[(usize, usize)]) -> String {
    let submatches: Vec<_> = spans.iter().map(|&(s, e)| serde_json::json!({ "match": { "text": &text[s..e] }, "start": s, "end": e })).collect();
    serde_json::json!({
        "type": "match",
        "data": {
            "path": { "text": path.to_string_lossy() },
            "lines": { "text": format!("{text}\n") },
            "line_number": number,
            "label": label,
            "submatches": submatches,
        }
    })
    .to_string()
}

/// Runs Documents search `run` (from `begin`): finds the documents under `folder`, reads them, and
/// sends what matches as ripgrep's JSON lines (`send(lines)`, in batches), then a summary like
/// ripgrep's, with how many documents couldn't be read; until all are read, `max` matches came, or
/// another search started.
pub fn search(run: u64, folder: &Path, request: &DocSearch, pattern: &regex::Regex, max: usize, send: impl Fn(Vec<String>)) {
    let started = Instant::now();
    let (paths, queue) = mpsc::sync_channel(256);
    let (found, documents) = mpsc::channel();
    let mut batch = Vec::new();
    let (mut matches, mut searched, mut with_match) = (0usize, 0usize, 0usize);
    let unreadable = std::thread::scope(|scope| {
        scope.spawn(move || walk(folder, request, run, paths));
        let readers = scope.spawn(move || read_all(queue, run, found));
        let mut sent = Instant::now();
        for document in documents {
            if !current(run) {
                break;
            }
            searched += 1;
            let before = matches;
            for (i, (label, text)) in document.lines.iter().enumerate() {
                let spans: Vec<_> = pattern.find_iter(text).map(|m| (m.start(), m.end())).collect();
                if !spans.is_empty() {
                    batch.push(as_ripgrep(&document.path, i + 1, label, text, &spans));
                    matches += 1;
                    if matches >= max {
                        break;
                    }
                }
            }
            with_match += usize::from(matches > before);
            if !batch.is_empty() && (batch.len() >= 1000 || sent.elapsed() >= Duration::from_millis(50)) {
                send(std::mem::take(&mut batch));
                sent = Instant::now();
            }
            if matches >= max {
                stop(run);
                break;
            }
        }
        readers.join().unwrap_or(0)
    });
    let elapsed = started.elapsed();
    batch.push(
        serde_json::json!({
            "type": "summary",
            "data": {
                "elapsed_total": { "secs": elapsed.as_secs(), "nanos": elapsed.subsec_nanos(), "human": format!("{:.3}s", elapsed.as_secs_f64()) },
                "stats": { "searches": searched, "searches_with_match": with_match, "matched_lines": matches, "unreadable": unreadable },
            }
        })
        .to_string(),
    );
    send(batch);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip(path: &Path, files: &[(&str, &str)]) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    fn pairs(lines: &[(&str, &str)]) -> Lines {
        lines.iter().map(|(l, t)| (l.to_string(), t.to_string())).collect()
    }

    #[test]
    fn reads_word_powerpoint_opendocument_and_epub() {
        let dir = std::env::temp_dir().join(format!("glint-doctext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let read = |name: &str| read(dir.join(name).to_str().unwrap());

        zip(&dir.join("a.docx"), &[("word/document.xml", r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:tab/><w:t xml:space="preserve"> brave &amp; </w:t></w:r><w:r><w:t>new world</w:t></w:r></w:p><w:p/><w:tbl><w:tr><w:tc><w:p><w:r><w:t>In a table</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>"#)]);
        assert_eq!(read("a.docx").unwrap(), pairs(&[("¶ 1", "Hello brave & new world"), ("¶ 2", "In a table")]));

        zip(&dir.join("b.pptx"), &[
            ("ppt/slides/slide10.xml", r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:r><a:t>Tenth</a:t></a:r></a:p></p:sld>"#),
            ("ppt/slides/slide2.xml", r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:r><a:t>Second </a:t></a:r><a:r><a:t>slide</a:t></a:r></a:p></p:sld>"#),
        ]);
        assert_eq!(read("b.pptx").unwrap(), pairs(&[("slide 2", "Second slide"), ("slide 10", "Tenth")]), "in the slides' order, not the zip's");

        zip(&dir.join("c.odt"), &[("content.xml", r#"<office:document-content xmlns:text="t" xmlns:office="o"><office:body><text:h>Title</text:h><text:p>Some <text:span>styled</text:span><text:s/>text</text:p></office:body></office:document-content>"#)]);
        assert_eq!(read("c.odt").unwrap(), pairs(&[("¶ 1", "Title"), ("¶ 2", "Some styled text")]));

        zip(&dir.join("d.odp"), &[("content.xml", r#"<office:document-content xmlns:draw="d" xmlns:text="t"><draw:page><text:p>One</text:p></draw:page><draw:page><text:p>Two</text:p></draw:page></office:document-content>"#)]);
        assert_eq!(read("d.odp").unwrap(), pairs(&[("slide 1", "One"), ("slide 2", "Two")]));

        zip(&dir.join("e.epub"), &[
            ("META-INF/container.xml", r#"<container><rootfiles><rootfile full-path="OEBPS/book.opf"/></rootfiles></container>"#),
            ("OEBPS/book.opf", r#"<package><manifest><item id="c1" href="one.xhtml"/><item id="c2" href="Chapter%20two.xhtml#top"/></manifest><spine><itemref idref="c2"/><itemref idref="c1"/></spine></package>"#),
            ("OEBPS/one.xhtml", "<html><body><p>Chapter one&#8217;s text</p></body></html>"),
            ("OEBPS/Chapter two.xhtml", "<html><body><h1>Prologue</h1><p>It&nbsp;begins.<br/>Again.</p></body></html>"),
        ]);
        assert_eq!(read("e.epub").unwrap(), pairs(&[("chapter 1", "Prologue"), ("chapter 1", "It begins. Again."), ("chapter 2", "Chapter one’s text")]));

        assert!(read("missing.pdf").is_err());
        std::fs::write(dir.join("broken.docx"), b"not a zip").unwrap();
        assert!(read("broken.docx").is_err());
        std::fs::write(dir.join("blank.docx"), b"").unwrap();
        assert_eq!(read("blank.docx").unwrap(), pairs(&[]), "Office's New menu makes empty files: no text, not unreadable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_as_ripgrep_would() {
        let search = |query: &str, case_sensitive, whole_word, regex| DocSearch { query: query.into(), path: String::new(), case_sensitive, whole_word, regex, exclusions: vec![], everything: false, ocr: false };
        assert!(pattern(&search("glint", false, false, false)).unwrap().is_match("GLINT rocks"), "smart case: lowercase ignores case");
        assert!(!pattern(&search("Glint", false, false, false)).unwrap().is_match("glint"), "an uppercase letter matches exactly");
        assert!(!pattern(&search("glint", true, false, false)).unwrap().is_match("Glint"));
        let dot = pattern(&search("a.b", false, false, false)).unwrap();
        assert!(dot.is_match("a.b") && !dot.is_match("axb"), "a literal unless a regular expression");
        assert!(!pattern(&search("cat", false, true, false)).unwrap().is_match("concat"));
        assert!(pattern(&search("c.t|dog", false, true, true)).unwrap().is_match("the dog"));
        assert!(pattern(&search("[", false, false, true)).is_err());

        let json: serde_json::Value = serde_json::from_str(&as_ripgrep(Path::new(r"C:\d\a.pdf"), 7, "p. 2", "find é here", &[(5, 7)])).unwrap();
        assert_eq!(
            (json["data"]["line_number"].as_u64(), json["data"]["label"].as_str(), json["data"]["submatches"][0]["match"]["text"].as_str(), json["data"]["lines"]["text"].as_str()),
            (Some(7), Some("p. 2"), Some("é"), Some("find é here\n"))
        );
    }

    #[test]
    fn long_lines_are_cut_and_exclusions_match_names() {
        let parts = cut("p. 1".into(), "word ".repeat(1000).trim().to_string());
        assert!(parts.len() >= 3 && parts.iter().all(|(l, t)| l == "p. 1" && t.len() <= MAX_LINE && !t.ends_with(' ')));
        let wide = cut("¶ 1".into(), "é".repeat(1500));
        assert!(wide.len() == 2 && wide.iter().all(|(_, t)| t.len() <= MAX_LINE), "cut between characters");

        assert!(is_document("Report.PDF", false) && is_document("a.b.xlsx", false) && !is_document("~$Report.docx", false) && !is_document("pdf", false) && !is_document("notes.txt", false));
        assert!(!is_document("scan.png", false) && is_document("scan.PNG", true) && !is_document("notes.txt", true), "pictures with OCR only");
        assert!(glob("node_modules", "Node_Modules"));
        assert!(glob("*.pdf", "Report.PDF") && !glob("*.pdf", "report.pdf.bak"));
        assert!(glob("img_????.jpg", "IMG_0042.jpg") && !glob("img_????.jpg", "IMG_42.jpg"));
        assert!(glob("*a*b*c*", "xxAyyBzzC") && !glob("*a*b*c*", "xxCyyBzzA"));
        assert!(glob("*", "") && !glob("a", ""));
        assert_eq!(unescape_url("Chapter%201%2Fend.xhtml"), "Chapter 1/end.xhtml");
        assert_eq!(unescape_url("100%.xhtml"), "100%.xhtml");
    }

    #[test]
    fn walks_to_documents_only() {
        let dir = std::env::temp_dir().join(format!("glint-docwalk-{}", std::process::id()));
        for folder in ["sub", "node_modules", ".hidden"] {
            std::fs::create_dir_all(dir.join(folder)).unwrap();
        }
        for file in ["a.pdf", "sub/b.DOCX", "sub/notes.txt", "sub/~$b.docx", "node_modules/c.pdf", ".hidden/d.pdf", "skip.xlsx"] {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        let request = |everything| DocSearch { query: String::new(), path: String::new(), case_sensitive: false, whole_word: false, regex: false, exclusions: vec!["node_modules".into(), "*.pdf".into(), "skip.xlsx".into()], everything, ocr: false };
        let found = |everything| {
            let run = begin();
            let (tx, rx) = mpsc::sync_channel(64);
            walk(&dir, &request(everything), run, tx);
            let mut names: Vec<String> = rx.iter().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/")).collect();
            names.sort();
            names
        };
        assert_eq!(found(false), ["a.pdf", "sub/b.DOCX"], "*.pdf doesn't hide documents; the folder, the exact name and Office's owner file do");
        assert_eq!(found(true), [".hidden/d.pdf", "a.pdf", "node_modules/c.pdf", "skip.xlsx", "sub/b.DOCX"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
