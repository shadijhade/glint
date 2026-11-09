//! Instant search inside files: an index of what the files below the folders you choose hold (text and
//! code, and documents' text: PDF, Word, Excel, PowerPoint, OpenDocument, EPUB), kept current as they
//! change, so words are found across all of them in milliseconds rather than by reading every file
//! again. tantivy (a search engine in Rust, like Lucene) keeps it in Glint's own data folder. It holds
//! your files' text, so it stays on this PC, reads only what your account can, and goes when it's
//! turned off. Documents are read by Glint's own worker processes, as Documents search reads them.
//!
//! What's typed is matched word by word (any case), and each file's lines holding every word are what
//! Text mode shows, as ripgrep's would be. Regular expressions, matching case and whole words go to
//! ripgrep, which reads the files themselves.

use crate::fsx::blocking;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, UNIX_EPOCH};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{Field, IndexRecordOption, Schema, Value, FAST, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};
use tauri::{AppHandle, Emitter, Manager, Window};

/// A line that matched: its file's label, its number, its text, and where each word is in it.
type Line = (String, usize, String, Vec<(usize, usize)>);

/// Text and code, by extension: read as text.
const TEXT_EXTS: &[&str] = &[
    "txt", "md", "markdown", "rst", "org", "adoc", "csv", "tsv", "json", "jsonl", "yaml", "yml", "toml", "xml", "html", "htm", "css", "scss", "less", "js", "jsx", "ts", "tsx", "mjs", "cjs", "rs",
    "py", "rb", "go", "java", "kt", "kts", "swift", "c", "h", "cpp", "hpp", "cc", "cs", "fs", "vb", "php", "pl", "lua", "sh", "ps1", "psm1", "bat", "cmd", "sql", "r", "scala", "dart", "zig", "nim",
    "ex", "exs", "erl", "hs", "ml", "clj", "vue", "svelte", "ini", "cfg", "conf", "log", "tex", "bib", "srt", "vtt", "gradle", "properties", "env", "proto", "graphql", "tf", "nix",
];
const MAX_TEXT: u64 = 8 << 20;
/// Folders never looked in: build output, package caches, version control.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", ".hg", ".svn", "target", ".next", "__pycache__", ".venv", "venv", ".cache", ".gradle", "$recycle.bin", "system volume information"];
/// Matches shown at most from one file.
const PER_FILE: usize = 50;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub enabled: bool,
    pub folders: Vec<String>,
    /// The text in pictures too (screenshots, scans, photos of whiteboards), read by Windows' OCR.
    #[serde(default = "on")]
    pub pictures: bool,
}

fn on() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Config { enabled: false, folders: Vec::new(), pictures: true }
    }
}

/// The biggest picture OCR reads for the index.
const MAX_PICTURE: u64 = 25 << 20;

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub folders: Vec<String>,
    pub pictures: bool,
    /// Files in the index.
    pub files: u64,
    /// Reading the folders: how many files looked at, and how many (re)indexed.
    pub indexing: bool,
    pub seen: u64,
    pub indexed: u64,
    /// The index's size on disk.
    pub bytes: u64,
    pub error: Option<String>,
}

struct Fields {
    path: Field,
    under: Field,
    name: Field,
    body: Field,
    labels: Field,
    modified: Field,
}

struct Content {
    dir: PathBuf,
    _index: Index,
    reader: IndexReader,
    writer: Mutex<IndexWriter>,
    f: Fields,
    /// Each indexed file's date (its path in lowercase): what's changed since is indexed again.
    known: Mutex<HashMap<String, u64>>,
    config: RwLock<Config>,
    status: Mutex<Status>,
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    /// Paths reported changed, indexed together a moment later.
    pending: Mutex<HashSet<PathBuf>>,
}

static CONTENT: OnceLock<Arc<Content>> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();

fn data_dir() -> Option<PathBuf> {
    match APP.get() {
        Some(app) => app.path().app_local_data_dir().ok().map(|d| d.join("content")),
        // The MCP server, a process of its own: the same folder.
        None => crate::tools::data_dir().map(|d| d.join("content")),
    }
}

/// A file and its lines that matched (number, text).
pub type FileLines = (String, Vec<(usize, String)>);

/// For the MCP server (a process of its own, while the app keeps the index's writer): files below `folder`
/// holding every word of `query`, best first, each with its first lines holding them all (number, text).
pub fn search_read_only(query: &str, folder: &str, max: usize) -> Result<Vec<FileLines>, String> {
    if !load_config().enabled {
        return Err("Search inside files instantly is off: Settings → Search inside files instantly turns it on".into());
    }
    let dir = data_dir().ok_or("Glint's data folder isn't known")?;
    let index = Index::open_in_dir(&dir).map_err(|_| "The index of what's in your files isn't made yet".to_string())?;
    let reader: IndexReader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into().map_err(|e| e.to_string())?;
    let (_, f) = schema();
    Ok(search_with(&reader, &f, query, folder, max).into_iter().map(|(path, lines)| (path, lines.into_iter().map(|(_, n, text, _)| (n, text)).collect())).collect())
}

fn config_path() -> Option<PathBuf> {
    data_dir().map(|d| d.with_file_name("content.json"))
}

fn load_config() -> Config {
    config_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let f = Fields {
        path: b.add_text_field("path", STRING | STORED),
        under: b.add_text_field("under", STRING),
        name: b.add_text_field("name", TEXT),
        body: b.add_text_field("body", TEXT | STORED),
        labels: b.add_text_field("labels", STORED),
        modified: b.add_u64_field("modified", FAST | STORED),
    };
    (b.build(), f)
}

fn open() -> Result<Arc<Content>, String> {
    if let Some(content) = CONTENT.get() {
        return Ok(content.clone());
    }
    let dir = data_dir().ok_or("Glint's data folder isn't known yet")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (schema, f) = schema();
    let index = match Index::open_in_dir(&dir) {
        Ok(index) => index,
        Err(_) => {
            // A first index, or one from another version: made anew.
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            Index::create_in_dir(&dir, schema).map_err(|e| e.to_string())?
        }
    };
    let reader = index.reader_builder().reload_policy(ReloadPolicy::OnCommitWithDelay).try_into().map_err(|e| e.to_string())?;
    let writer = index.writer(64 << 20).map_err(|e| e.to_string())?;
    let content = Arc::new(Content {
        dir,
        _index: index,
        reader,
        writer: Mutex::new(writer),
        f,
        known: Mutex::default(),
        config: RwLock::new(load_config()),
        status: Mutex::default(),
        watcher: Mutex::default(),
        pending: Mutex::default(),
    });
    content.load_known();
    let _ = CONTENT.set(content.clone());
    Ok(CONTENT.get().cloned().unwrap_or(content))
}

fn lower_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// Every folder a file is in, lowercase, the drive first ("c:", "c:\users"…): a search below a folder
/// asks for that folder among them.
fn ancestors(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy().to_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = text[at..].find('\\') {
        let end = at + i;
        if end > 0 {
            out.push(text[..end].to_string());
        }
        at = end + 1;
    }
    out
}

/// `Content::search`, over any reader of the index (the app's own, or the MCP server's read-only one).
fn search_with(reader: &IndexReader, f: &Fields, query: &str, folder: &str, max: usize) -> Vec<(String, Vec<Line>)> {
    let words: Vec<String> = words_of(query);
    if words.is_empty() {
        return Vec::new();
    }
    let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    for word in &words {
        let either: Vec<(Occur, Box<dyn Query>)> = vec![
            (Occur::Should, Box::new(TermQuery::new(Term::from_field_text(f.body, word), IndexRecordOption::WithFreqs))),
            (Occur::Should, Box::new(TermQuery::new(Term::from_field_text(f.name, word), IndexRecordOption::Basic))),
        ];
        clauses.push((Occur::Must, Box::new(BooleanQuery::new(either))));
    }
    let folder = folder.trim_end_matches('\\').to_lowercase();
    clauses.push((Occur::Must, Box::new(TermQuery::new(Term::from_field_text(f.under, &folder), IndexRecordOption::Basic))));
    let searcher = reader.searcher();
    let Ok(top) = searcher.search(&BooleanQuery::new(clauses), &TopDocs::with_limit(2000).order_by_score()) else { return Vec::new() };
    let mut out = Vec::new();
    let mut matches = 0;
    for (_, address) in top {
        let Ok(doc) = searcher.doc::<TantivyDocument>(address) else { continue };
        let path = doc.get_first(f.path).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let body = doc.get_first(f.body).and_then(|v| v.as_str()).unwrap_or_default();
        let labels: Vec<&str> = doc.get_first(f.labels).and_then(|v| v.as_str()).map(|l| l.split('\n').collect()).unwrap_or_default();
        let mut lines = Vec::new();
        for (i, line) in body.split('\n').enumerate() {
            let lower = line.to_lowercase();
            if !words.iter().all(|w| lower.contains(w.as_str())) {
                continue;
            }
            let mut spans: Vec<(usize, usize)> = Vec::new();
            for word in &words {
                let mut at = 0;
                while let Some(found) = lower[at..].find(word.as_str()) {
                    let start = at + found;
                    // Byte offsets in the line as written (lowercasing keeps them for almost all text).
                    if line.is_char_boundary(start) && line.is_char_boundary((start + word.len()).min(line.len())) {
                        spans.push((start, (start + word.len()).min(line.len())));
                    }
                    at = start + word.len().max(1);
                }
            }
            spans.sort_unstable();
            let label = labels.get(i).map(|l| l.to_string()).unwrap_or_default();
            lines.push((label, i + 1, line.to_string(), spans));
            matches += 1;
            if lines.len() >= PER_FILE || matches >= max {
                break;
            }
        }
        // A match in the name alone: the file, with its first line.
        if lines.is_empty() {
            continue;
        }
        out.push((path, lines));
        if matches >= max {
            break;
        }
    }
    out
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64)
}

fn kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
    if TEXT_EXTS.contains(&ext.as_str()) {
        Some("text")
    } else if crate::doctext::DOCUMENTS.contains(&ext.as_str()) {
        Some("document")
    } else if crate::ocr::PICTURES.contains(&ext.as_str()) {
        Some("picture")
    } else {
        None
    }
}

/// A text file's lines; None when it's binary after all.
fn text_lines(path: &Path) -> Option<Vec<String>> {
    let bytes = std::fs::read(path).ok()?;
    let text = if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&units)
    } else {
        if bytes[..bytes.len().min(8192)].contains(&0) {
            return None;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Some(text.lines().map(|l| l.chars().take(2000).collect()).collect())
}

impl Content {
    fn load_known(&self) {
        let searcher = self.reader.searcher();
        let mut known = HashMap::new();
        for segment in searcher.segment_readers() {
            let Ok(store) = segment.get_store_reader(64) else { continue };
            let alive = segment.alive_bitset();
            for id in 0..segment.max_doc() {
                if alive.is_some_and(|a| !a.is_alive(id)) {
                    continue;
                }
                let Ok(doc) = store.get::<TantivyDocument>(id) else { continue };
                let path = doc.get_first(self.f.path).and_then(|v| v.as_str()).map(str::to_lowercase);
                let modified = doc.get_first(self.f.modified).and_then(|v| v.as_u64()).unwrap_or(0);
                if let Some(path) = path {
                    known.insert(path, modified);
                }
            }
        }
        self.status.lock().unwrap().files = known.len() as u64;
        *self.known.lock().unwrap() = known;
    }

    fn covered(&self, path: &Path) -> bool {
        let key = lower_key(path);
        self.config.read().unwrap().folders.iter().any(|f| {
            let f = f.trim_end_matches('\\').to_lowercase();
            key == f || key.starts_with(&format!("{f}\\"))
        })
    }

    /// Indexes one file as it is now (or drops it, when it's gone or nothing to index).
    fn index_file(&self, writer: &IndexWriter, path: &Path) -> bool {
        let key = lower_key(path);
        let meta = std::fs::metadata(path).ok().filter(|m| m.is_file());
        let Some(meta) = meta.filter(|_| self.covered(path)) else {
            if self.known.lock().unwrap().remove(&key).is_some() {
                writer.delete_term(Term::from_field_text(self.f.path, &path.to_string_lossy()));
            }
            return false;
        };
        let modified = modified_ms(&meta);
        if self.known.lock().unwrap().get(&key) == Some(&modified) {
            return false;
        }
        // Files kept only in the cloud (OneDrive's) aren't read: reading would download them.
        #[cfg(windows)]
        if std::os::windows::fs::MetadataExt::file_attributes(&meta) & 0x0040_0000 != 0 {
            return false;
        }
        let (body, labels) = match kind(path) {
            Some("text") if meta.len() <= MAX_TEXT => match text_lines(path) {
                Some(lines) => (lines.join("\n"), String::new()),
                None => return false,
            },
            // A picture's text, read by Windows' OCR (nothing in a picture without text is kept).
            Some("picture") if meta.len() <= MAX_PICTURE && self.config.read().unwrap().pictures => match crate::ocr::lines(&path.to_string_lossy()) {
                Ok(lines) if !lines.is_empty() => (lines.join("\n"), String::new()),
                _ => {
                    self.known.lock().unwrap().insert(key, modified);
                    return false;
                }
            },
            Some("document") => match crate::doctext::read_isolated(path) {
                Ok(lines) => {
                    let labels: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
                    let texts: Vec<String> = lines.iter().map(|(_, t)| t.replace('\n', " ")).collect();
                    (texts.join("\n"), labels.join("\n"))
                }
                Err(_) => return false,
            },
            _ => return false,
        };
        writer.delete_term(Term::from_field_text(self.f.path, &path.to_string_lossy()));
        let mut document = doc!(
            self.f.path => path.to_string_lossy().into_owned(),
            self.f.name => path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            self.f.body => body,
            self.f.labels => labels,
            self.f.modified => modified,
        );
        for folder in ancestors(path) {
            document.add_text(self.f.under, folder);
        }
        let _ = writer.add_document(document);
        self.known.lock().unwrap().insert(key, modified);
        true
    }

    /// Brings the index up to the folders as they are: what's new or changed is read, what's gone goes.
    fn reconcile(self: &Arc<Self>) {
        let folders = self.config.read().unwrap().folders.clone();
        {
            let mut status = self.status.lock().unwrap();
            status.indexing = true;
            status.seen = 0;
            status.indexed = 0;
            status.error = None;
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
        let mut since_commit = Instant::now();
        while let Some(dir) = stack.pop() {
            if !self.config.read().unwrap().enabled {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                #[cfg(windows)]
                let attrs = std::os::windows::fs::MetadataExt::file_attributes(&meta);
                #[cfg(not(windows))]
                let attrs = 0u32;
                // Hidden and system items, and links to elsewhere, aren't looked in.
                if attrs & (0x2 | 0x4 | 0x400) != 0 {
                    continue;
                }
                if meta.is_dir() {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if !SKIP_DIRS.contains(&name.as_str()) {
                        stack.push(path);
                    }
                    continue;
                }
                if kind(&path).is_none() {
                    continue;
                }
                seen.insert(lower_key(&path));
                let indexed = {
                    let writer = self.writer.lock().unwrap();
                    self.index_file(&writer, &path)
                };
                let mut status = self.status.lock().unwrap();
                status.seen += 1;
                status.indexed += u64::from(indexed);
                drop(status);
                if since_commit.elapsed() > Duration::from_secs(3) {
                    self.commit();
                    since_commit = Instant::now();
                }
            }
        }
        // What's in the index but no longer in the folders.
        let gone: Vec<String> = self.known.lock().unwrap().keys().filter(|k| !seen.contains(*k)).cloned().collect();
        if self.config.read().unwrap().enabled {
            for key in gone {
                let path = PathBuf::from(&key);
                let writer = self.writer.lock().unwrap();
                self.index_file(&writer, &path);
            }
        }
        self.commit();
        self.status.lock().unwrap().indexing = false;
    }

    fn commit(&self) {
        let _ = self.writer.lock().unwrap().commit();
        let _ = self.reader.reload();
        let mut status = self.status.lock().unwrap();
        status.files = self.known.lock().unwrap().len() as u64;
        status.bytes = std::fs::read_dir(&self.dir).map(|d| d.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0);
    }

    /// Watches the folders: what changes is indexed again two seconds after it last changed.
    fn watch(self: &Arc<Self>) {
        use notify::{RecursiveMode, Watcher};
        let content = self.clone();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                content.pending.lock().unwrap().extend(event.paths);
            }
        });
        let Ok(mut watcher) = watcher else { return };
        for folder in self.config.read().unwrap().folders.iter() {
            let _ = watcher.watch(Path::new(folder), RecursiveMode::Recursive);
        }
        *self.watcher.lock().unwrap() = Some(watcher);
        let content = self.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(2));
            if !content.config.read().unwrap().enabled {
                return;
            }
            let paths: Vec<PathBuf> = content.pending.lock().unwrap().drain().collect();
            if paths.is_empty() {
                continue;
            }
            {
                let writer = content.writer.lock().unwrap();
                for path in paths.iter().filter(|p| kind(p).is_some()) {
                    content.index_file(&writer, path);
                }
            }
            content.commit();
        });
    }

    /// Files below `folder` holding every word of `query`, best first, each with its lines holding them all.
    fn search(&self, query: &str, folder: &str, max: usize) -> Vec<(String, Vec<Line>)> {
        search_with(&self.reader, &self.f, query, folder, max)
    }
}

/// The words of what's typed, lowercase, as the index keeps them.
fn words_of(query: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for word in query.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()) {
        let word = word.to_lowercase();
        if !words.contains(&word) {
            words.push(word);
        }
    }
    words
}

/// Starts content indexing at launch, when it's on.
pub fn start(app: &AppHandle) {
    let _ = APP.set(app.clone());
    if !load_config().enabled {
        return;
    }
    std::thread::spawn(|| {
        if let Ok(content) = open() {
            content.status.lock().unwrap().enabled = true;
            content.watch();
            content.reconcile();
        }
    });
}

#[tauri::command]
pub async fn content_status() -> Result<Status, String> {
    blocking(|| {
        let config = load_config();
        let mut status = CONTENT.get().map(|c| c.status.lock().unwrap().clone()).unwrap_or_default();
        status.enabled = config.enabled;
        status.folders = config.folders;
        status.pictures = config.pictures;
        Ok(status)
    })
    .await
}

/// Turns content indexing on (for `folders`) or off (the index is deleted).
#[tauri::command]
pub async fn content_configure(enabled: bool, folders: Vec<String>, pictures: Option<bool>) -> Result<Status, String> {
    blocking(move || {
        let pictures = pictures.unwrap_or_else(|| load_config().pictures);
        let config = Config { enabled, folders: folders.into_iter().filter(|f| !f.trim().is_empty()).collect(), pictures };
        let path = config_path().ok_or("Glint's data folder isn't known yet")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, serde_json::to_vec(&config).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if !enabled {
            if let Some(content) = CONTENT.get() {
                *content.config.write().unwrap() = config.clone();
                *content.watcher.lock().unwrap() = None;
                let mut writer = content.writer.lock().unwrap();
                let _ = writer.delete_all_documents();
                let _ = writer.commit();
                content.known.lock().unwrap().clear();
                let mut status = content.status.lock().unwrap();
                *status = Status::default();
            }
            return Ok(Status { enabled: false, folders: config.folders, pictures: config.pictures, ..Default::default() });
        }
        let content = open()?;
        *content.config.write().unwrap() = config.clone();
        content.status.lock().unwrap().enabled = true;
        let worker = content.clone();
        std::thread::spawn(move || {
            *worker.watcher.lock().unwrap() = None;
            worker.watch();
            worker.reconcile();
        });
        let mut status = content.status.lock().unwrap().clone();
        status.folders = config.folders;
        Ok(status)
    })
    .await
}

/// Whether the content index covers `path` (and is on).
#[tauri::command]
pub fn content_covers(path: String) -> bool {
    CONTENT.get().is_some_and(|c| c.config.read().unwrap().enabled && c.covered(Path::new(path.trim_end_matches('\\'))))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentSearch {
    pub query: String,
    pub path: String,
}

/// A Text search answered by the content index: its matches arrive as `search-event`s, as ripgrep's
/// JSON lines, then a summary with how long it took. Returns the folder as searched.
#[tauri::command]
pub async fn run_content_search(window: Window, request: ContentSearch, search_id: String, max_results: Option<usize>) -> Result<String, String> {
    blocking(move || {
        let content = CONTENT.get().cloned().ok_or("Content indexing isn't on")?;
        let folder = request.path.trim_end_matches('\\').to_string();
        let started = Instant::now();
        let max = match max_results {
            Some(0) | None => usize::MAX,
            Some(n) => n,
        };
        let found = content.search(&request.query, &folder, max);
        let emit = |kind: &str, lines: Vec<String>| {
            let _ = window.emit("search-event", crate::SearchEvent { id: search_id.clone(), r#type: kind.into(), lines });
        };
        let mut batch = Vec::new();
        let mut matches = 0;
        for (path, lines) in &found {
            for (label, number, text, spans) in lines {
                batch.push(crate::doctext::as_ripgrep(Path::new(path), *number..*number + 1, label, text, spans));
                matches += 1;
            }
        }
        let elapsed = started.elapsed();
        batch.push(
            serde_json::json!({
                "type": "summary",
                "data": {
                    "elapsed_total": { "secs": elapsed.as_secs(), "nanos": elapsed.subsec_nanos(), "human": format!("{:.3}s", elapsed.as_secs_f64()) },
                    "stats": { "searches": content.status.lock().unwrap().files, "searches_with_match": found.len(), "matches": matches, "matched_lines": matches, "engine": "index" },
                }
            })
            .to_string(),
        );
        emit("data", batch);
        emit("finished", Vec::new());
        Ok(folder)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_words_as_the_index_keeps_them() {
        assert_eq!(ancestors(Path::new(r"C:\Users\Me\Notes\a.txt")), ["c:", r"c:\users", r"c:\users\me", r"c:\users\me\notes"]);
        assert_eq!(words_of("Budget 2026: q3-report, budget"), ["budget", "2026", "q3", "report"]);
        assert_eq!(words_of("ميزانية الربع"), ["ميزانية", "الربع"], "any language's words");
        assert_eq!(kind(Path::new("a.RS")), Some("text"));
        assert_eq!(kind(Path::new("b.docx")), Some("document"));
        assert_eq!(kind(Path::new("c.exe")), None);
    }
}
