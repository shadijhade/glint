//! Importing the old app's settings and saved state from WebView2's localStorage (a LevelDB folder under
//! EBWebView), read-only, at the first start of this app. Wave 16 (settings-import).
//!
//! The web app kept its state in WebView2's `localStorage`, which Chromium keeps in a LevelDB folder:
//! `%LOCALAPPDATA%\com.shado.shadui-ripgrep\EBWebView\Default\Local Storage\leveldb`. [`leveldb`] reads that
//! folder (and [`snappy`] the compressed blocks in it) without writing a byte; this module turns what it holds
//! into [`OldData`].
//!
//! | localStorage key                  | where it comes from                          | where it goes                                    |
//! |-----------------------------------|----------------------------------------------|--------------------------------------------------|
//! | `shadui-ripgrep-storage`          | the zustand store (`src/lib/store.ts`)       | `state.json`, through `persistence::save_to`     |
//! | `glint-book-places`, `-size`      | `BookView.tsx`                               | `books.json`, through `book_places::import_old`  |
//! | `glint-recent-tags`               | `src/lib/tags.ts`                            | [`OldData::recent_tags`] (see below)             |
//! | `last-version`                    | `src/lib/updater.ts`                         | [`OldData::last_version`]                        |
//! | `offer-instant-search`, `service-update-notice`, `update-notice`, `quick-shortcut-taken` | `firstNotice` in `src/lib/utils.ts` | [`OldData::notices`] |
//!
//! **Chromium's keys** are `_` + origin + `\0` + a byte (1: the name is Latin-1, 0: UTF-16LE) + the name. The
//! origin here is `http://tauri.localhost` (Tauri 2 on Windows); any origin containing `tauri.localhost` is
//! taken, and where two have the same key the newer write wins. **Values** start with a byte too: 0 means the
//! rest is UTF-16LE, 1 means Latin-1. The `META:`, `METAACCESS:` and `VERSION` entries are not keys of the page
//! and are left out.
//!
//! **When it runs** ([`import_at_first_start`], called by `persistence::init` before it loads `state.json`):
//! only in the main window, only when there is no `state.json` yet, and only if the old folder is there. Then
//! `state.json` is written (once: from then on the file exists and nothing imports again), `books.json` too if
//! there isn't one, and what the old app knew is kept for this run in [`imported`]. Any failure is logged and
//! the app starts with its defaults; a failure writes nothing, so the next start tries again.
//!
//! **Read-only, and a busy folder.** Nothing in `EBWebView` is ever written, moved or deleted. If the old app
//! is running and a file can't be read, it is copied to a temp folder and the copy is read (the copy is removed
//! afterwards). Files that are only ever listed or read are `.log`, `.ldb` and `.sst`; `LOCK` is never touched.
//!
//! **What isn't carried over exactly:**
//! - A book's place was a CFI (an EPUB position); this app keeps a chapter and a block. The CFI's spine step
//!   gives the chapter (`/6/14` is the 7th chapter) and the block is 0, so a book reopens at the top of the
//!   chapter it was left in. See [`place_from_cfi`].
//! - Recent tags and the rest of [`OldData`] are not saved anywhere by this module: the recent tags are the
//!   starting point of `dialogs::tags::Tags` (which has nowhere of its own to keep them yet), and the first-notice
//!   flags and the last version are for whoever remembers notices. They exist for the run that imported, and
//!   read as `None` on later starts.
//! - Zstandard-compressed blocks (newer Chromium can write them) are skipped with a note in [`OldData::problems`];
//!   Edge WebView2 for this app's data has only ever used Snappy.

pub mod leveldb;
pub mod snappy;

use crate::book_places::Place;
use glint_model::store::{load_persisted, PERSIST_VERSION, STORAGE_KEY};
use serde::de::{Deserializer, MapAccess, Visitor};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The book reader's places (`PLACES` in BookView.tsx).
const PLACES_KEY: &str = "glint-book-places";
/// The book reader's text size (`SIZE` in BookView.tsx).
const SIZE_KEY: &str = "glint-book-size";
/// The recent tags (`RECENT_KEY` in tags.ts).
const RECENT_TAGS_KEY: &str = "glint-recent-tags";
/// The version that last ran (updater.ts).
const LAST_VERSION_KEY: &str = "last-version";
/// The `firstNotice` keys: a flag per notice, holding the value it was last shown for.
const NOTICE_KEYS: [&str; 4] = [
    "offer-instant-search",
    "service-update-notice",
    "update-notice",
    "quick-shortcut-taken",
];
/// The most recent tags the old app kept.
const MOST_RECENT_TAGS: usize = 20;
/// The biggest file read. LevelDB's own are 2 MiB at most, apart from a log that is still growing.
const MAX_FILE: u64 = 256 << 20;

/// What the old app had in its localStorage.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OldData {
    /// The zustand store: the text `{"state": {...}, "version": 0}`, if it was there and had that shape. Load
    /// it with `glint_model::store::load_persisted`.
    pub store: Option<String>,
    /// Where each book was left, as the old CFI: lower-case path and CFI, the oldest book first.
    pub book_places: Vec<(String, String)>,
    /// The book reader's text size, percent, if one was saved (not clamped).
    pub book_size: Option<u32>,
    /// The tags used lately, most recent first.
    pub recent_tags: Vec<String>,
    /// The Glint version that last ran (`last-version`).
    pub last_version: Option<String>,
    /// The first-notice flags: key (`offer-instant-search`, `service-update-notice`, `update-notice`,
    /// `quick-shortcut-taken`) and the value it was shown for.
    pub notices: BTreeMap<String, String>,
    /// Things that couldn't be read, for the log.
    pub problems: Vec<String>,
}

// ---- Finding and reading the folder ------------------------------------------------------------------------------

/// Where the old app's localStorage is on this PC: `%LOCALAPPDATA%\com.shado.shadui-ripgrep\EBWebView\Default\
/// Local Storage\leveldb`. (The folder may not exist.)
pub fn default_dir() -> Option<PathBuf> {
    glint_core::host::PathResolver.app_local_data_dir().ok().map(|dir| {
        dir.join("EBWebView")
            .join("Default")
            .join("Local Storage")
            .join("leveldb")
    })
}

/// Whether `dir` holds a LevelDB: a `CURRENT` file or any `.log`, `.ldb` or `.sst`.
fn is_leveldb(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name == "current" || data_file_kind(&name).is_some()
        })
    })
}

/// The LevelDB folder meant by `dir`: `dir` itself, or the folder inside the old app's data
/// (`EBWebView` or the app's own folder), so the coordinator may hand over whichever is at hand.
fn find_leveldb(dir: &Path) -> Option<PathBuf> {
    let inside = [
        "",
        "Default/Local Storage/leveldb",
        "EBWebView/Default/Local Storage/leveldb",
        "Local Storage/leveldb",
    ];
    inside
        .iter()
        .map(|rest| {
            if rest.is_empty() {
                dir.to_path_buf()
            } else {
                dir.join(rest)
            }
        })
        .find(|candidate| is_leveldb(candidate))
}

/// What a file of a LevelDB folder is (by its lower-case name): a log or a table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FileKind {
    Log,
    Table,
}

fn data_file_kind(lower_name: &str) -> Option<FileKind> {
    match lower_name.rsplit_once('.') {
        Some((_, "log")) => Some(FileKind::Log),
        Some((_, "ldb" | "sst")) => Some(FileKind::Table),
        _ => None,
    }
}

/// A fresh name for a temp folder.
fn temp_copy_dir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "glint-old-settings-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

/// `path`'s bytes through `first`; if that fails (the old app has the file locked), through a copy in a temp
/// folder, which is removed again. The original is only ever read.
fn read_file(path: &Path, first: &dyn Fn(&Path) -> std::io::Result<Vec<u8>>) -> Result<Vec<u8>, String> {
    let error = match first(path) {
        Ok(bytes) => return Ok(bytes),
        Err(error) => error,
    };
    let name = path.file_name().ok_or("no file name")?;
    let folder = temp_copy_dir();
    std::fs::create_dir_all(&folder).map_err(|e| format!("{error}; and no temp folder for a copy: {e}"))?;
    let copy = folder.join(name);
    let result = std::fs::copy(path, &copy)
        .and_then(|_| std::fs::read(&copy))
        .map_err(|e| format!("{error}; and a copy couldn't be read either: {e}"));
    let _ = std::fs::remove_dir_all(&folder);
    result
}

fn plain_read(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Reads the old app's localStorage from `dir`: the LevelDB folder itself, or the old app's `EBWebView` folder or
/// local data folder around it. Pure reading, no writes anywhere. An error when there is no LevelDB there.
pub fn read_old(dir: &Path) -> Result<OldData, String> {
    read_old_with(dir, &plain_read)
}

/// [`read_old`] reading each file with `reader` first (tests make it fail, as a locked file does).
fn read_old_with(dir: &Path, reader: &dyn Fn(&Path) -> std::io::Result<Vec<u8>>) -> Result<OldData, String> {
    let folder = find_leveldb(dir).ok_or_else(|| format!("no old settings found in {}", dir.display()))?;
    let mut files: Vec<(PathBuf, FileKind)> = std::fs::read_dir(&folder)
        .map_err(|e| format!("couldn't list {}: {e}", folder.display()))?
        .flatten()
        .filter_map(|e| {
            let kind = data_file_kind(&e.file_name().to_string_lossy().to_lowercase())?;
            e.file_type().ok()?.is_file().then(|| (e.path(), kind))
        })
        .collect();
    // Tables first (oldest data), the logs after; the sequence numbers decide anyway.
    files.sort_by(|a, b| (a.1 == FileKind::Log, &a.0).cmp(&(b.1 == FileKind::Log, &b.0)));

    let mut db = leveldb::Db::new(is_page_key);
    for (path, kind) in files {
        let shown = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_FILE) {
            db.problems.push(format!("{shown} is too big to read"));
            continue;
        }
        let bytes = match read_file(&path, reader) {
            Ok(bytes) => bytes,
            Err(why) => {
                db.problems.push(format!("{shown}: {why}"));
                continue;
            }
        };
        match kind {
            FileKind::Log => db.read_log(&bytes),
            FileKind::Table => {
                if let Err(why) = db.read_table(&bytes) {
                    db.problems.push(format!("{shown}: {why}"));
                }
            }
        }
    }
    Ok(old_data(db))
}

// ---- Chromium's localStorage keys and values ------------------------------------------------------------------------

/// A key of the page (not META or VERSION) of an origin that is this app's webview.
fn is_page_key(key: &[u8]) -> bool {
    split_key(key).is_some()
}

/// The name of the item in a Chromium localStorage key: `_` + origin + `\0` + encoding byte + name. None for
/// other keys and other origins.
fn split_key(key: &[u8]) -> Option<String> {
    let rest = key.strip_prefix(b"_")?;
    let nul = rest.iter().position(|b| *b == 0)?;
    let origin = String::from_utf8_lossy(&rest[..nul]);
    if !origin.contains("tauri.localhost") {
        return None;
    }
    let (encoding, name) = rest[nul + 1..].split_first()?;
    decode_text(*encoding, name)
}

/// Text as localStorage stores it: `encoding` 0 is UTF-16LE, 1 is Latin-1.
fn decode_text(encoding: u8, bytes: &[u8]) -> Option<String> {
    match encoding {
        1 => Some(bytes.iter().map(|b| char::from(*b)).collect()),
        0 => {
            // An odd trailing byte is dropped.
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u16::from_le_bytes(*p))
                .collect();
            Some(String::from_utf16_lossy(&units))
        }
        _ => None,
    }
}

/// A localStorage value: the encoding byte, then the text.
fn decode_value(value: &[u8]) -> Option<String> {
    let (encoding, text) = value.split_first()?;
    decode_text(*encoding, text)
}

/// The items of the database as [`OldData`].
fn old_data(db: leveldb::Db) -> OldData {
    // The newest write of each name over all origins.
    let mut items: BTreeMap<String, (u64, String)> = BTreeMap::new();
    for (key, seq, value) in db.live() {
        let (Some(name), Some(text)) = (split_key(key), decode_value(value)) else {
            continue;
        };
        if items.get(&name).is_none_or(|(have, _)| *have < seq) {
            items.insert(name, (seq, text));
        }
    }
    let mut data = OldData {
        problems: db.problems,
        ..OldData::default()
    };
    let mut get = |name: &str| items.remove(name).map(|(_, text)| text);

    if let Some(text) = get(STORAGE_KEY) {
        if is_saved_state(&text) {
            data.store = Some(text);
        } else {
            data.problems.push(format!(
                "{STORAGE_KEY} isn't a saved state of version {PERSIST_VERSION}"
            ));
        }
    }
    if let Some(text) = get(PLACES_KEY) {
        match serde_json::from_str::<Places>(&text) {
            Ok(Places(places)) => data.book_places = places,
            Err(e) => data.problems.push(format!("{PLACES_KEY}: {e}")),
        }
    }
    // `Number(localStorage.getItem(SIZE)) || 100`: text that isn't a number, or 0, means the default.
    data.book_size = get(SIZE_KEY)
        .and_then(|text| text.trim().parse::<f64>().ok())
        .filter(|n| n.is_finite() && *n >= 1.0)
        .map(|n| n.min(1e6).round() as u32);
    if let Some(text) = get(RECENT_TAGS_KEY) {
        match serde_json::from_str::<Vec<Value>>(&text) {
            Ok(tags) => {
                data.recent_tags = tags
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .filter(|t| !t.trim().is_empty())
                    .take(MOST_RECENT_TAGS)
                    .collect();
            }
            Err(e) => data.problems.push(format!("{RECENT_TAGS_KEY}: {e}")),
        }
    }
    data.last_version = get(LAST_VERSION_KEY).filter(|v| !v.is_empty());
    for key in NOTICE_KEYS {
        if let Some(value) = get(key) {
            data.notices.insert(key.to_string(), value);
        }
    }
    data
}

/// Whether `text` is `{"state": {...}, "version": 0}`, which is what `load_persisted` takes.
fn is_saved_state(text: &str) -> bool {
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(map)) => {
            map.get("state").is_some_and(Value::is_object)
                && map.get("version").and_then(Value::as_i64) == Some(PERSIST_VERSION)
        }
        _ => false,
    }
}

/// The book places object `{"c:\\a.epub": "epubcfi(...)", ...}` in the order it was written in (the oldest book
/// first, since the old code deleted a book before adding it again), which a plain JSON map would lose.
struct Places(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Places {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct InOrder;
        impl<'de> Visitor<'de> for InOrder {
            type Value = Places;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of book places")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Places, A::Error> {
                let mut places: Vec<(String, String)> = Vec::new();
                while let Some((path, cfi)) = map.next_entry::<String, Value>()? {
                    if let Value::String(cfi) = cfi {
                        places.retain(|(p, _)| *p != path);
                        places.push((path, cfi));
                    }
                }
                Ok(Places(places))
            }
        }
        deserializer.deserialize_map(InOrder)
    }
}

/// The place in a book the old reader's CFI stands for. An EPUB CFI starts with the spine step: `epubcfi(/6/14!...)`
/// is the 7th item of the spine (`/6` is the spine, `/14` its 7th item: steps count 2, 4, 6...). That is the
/// chapter; the position inside it (a path into the XHTML) can't be mapped onto this reader's blocks, so the
/// block is 0 and the book reopens at the top of that chapter. None for a CFI of another shape.
pub fn place_from_cfi(cfi: &str) -> Option<Place> {
    let rest = cfi.trim();
    let rest = rest.strip_prefix("epubcfi(").unwrap_or(rest);
    let rest = rest.strip_prefix("/6/")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let step: usize = digits.parse().ok()?;
    // Even steps are elements; the spine's first item is step 2.
    if step < 2 || !step.is_multiple_of(2) {
        return None;
    }
    Some(Place {
        chapter: step / 2 - 1,
        block: 0,
    })
}

// ---- The first start ---------------------------------------------------------------------------------------------

/// What the first-start import did.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    /// What was read.
    pub data: OldData,
    /// `state.json` was written.
    pub state_written: bool,
    /// `books.json` was written, with this many books' places.
    pub books_written: Option<usize>,
}

static IMPORTED: OnceLock<Imported> = OnceLock::new();

/// What the old app's storage held, if this run imported it (the first start of this app, in the main window).
/// `None` on every later start, in a secondary window, and when there was nothing to import.
///
/// For whoever remembers notices and recent tags: [`OldData::notices`] (the first-notice flags, for example
/// `offer-instant-search` = `1` once the offer was made), [`OldData::last_version`] and [`OldData::recent_tags`] are
/// not saved by the import (only `state.json` and `books.json` are); take them from here at start and keep them
/// in the new app's own saved state.
pub fn imported() -> Option<&'static OldData> {
    IMPORTED.get().map(|imported| &imported.data)
}

/// The import at the first start, for `persistence::init`: before the state file is loaded, with the real folders.
/// Logs what it did or why it didn't; never fails the start.
pub fn import_at_first_start(state_file: &Path) {
    let books_file = crate::book_places::file_path();
    match first_start_at(default_dir().as_deref(), state_file, books_file.as_deref()) {
        Ok(Some(done)) => {
            eprintln!(
                "glint: imported the old app's settings{}{}",
                if done.state_written {
                    " (state.json written)"
                } else {
                    ""
                },
                done.books_written
                    .map_or(String::new(), |n| format!(", {n} books' places")),
            );
            for problem in &done.data.problems {
                eprintln!("glint: old settings: {problem}");
            }
            let _ = IMPORTED.set(done);
        }
        Ok(None) => {}
        Err(why) => eprintln!("glint: couldn't import the old app's settings, starting with the defaults: {why}"),
    }
}

/// The import itself, on the given folders: `old_dir` the old LevelDB (or the folder around it), `state_file` the
/// state to make, `books_file` the book places to make. `Ok(None)` when it isn't the first start (`state_file`
/// exists, or can't be looked at), when this is a secondary window, or when there is no old storage; an error
/// when the old storage couldn't be read or the state couldn't be written (nothing is written then, or only
/// what was complete).
fn first_start_at(
    old_dir: Option<&Path>,
    state_file: &Path,
    books_file: Option<&Path>,
) -> Result<Option<Imported>, String> {
    if crate::platform::secondary::is_secondary() {
        return Ok(None);
    }
    if !matches!(state_file.try_exists(), Ok(false)) {
        return Ok(None);
    }
    let Some(old_dir) = old_dir else {
        return Ok(None);
    };
    if find_leveldb(old_dir).is_none() {
        return Ok(None);
    }
    let data = read_old(old_dir)?;
    if data.store.is_none() && !data.problems.is_empty() {
        // Something was wrong and nothing was found: leave it to the next start rather than settle on defaults.
        return Err(data.problems.join("; "));
    }
    let state = data.store.as_deref().map(load_persisted).unwrap_or_default();
    crate::persistence::save_to(state_file, &state)?;

    let places: Vec<(String, Place)> = data
        .book_places
        .iter()
        .filter_map(|(path, cfi)| place_from_cfi(cfi).map(|place| (path.clone(), place)))
        .collect();
    let books_written = match books_file {
        Some(file) if crate::book_places::import_old(file, &places, data.book_size) => Some(places.len()),
        _ => None,
    };
    Ok(Some(Imported {
        data,
        state_written: true,
        books_written,
    }))
}

#[cfg(test)]
mod tests {
    use super::leveldb::build::{batch, log, table, Op};
    use super::*;

    /// A folder per test, removed at the end.
    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("glint-old-settings-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp folder");
            Folder(dir)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A localStorage key: `_` origin `\0` Latin-1 marker name.
    fn key(origin: &str, name: &str) -> Vec<u8> {
        let mut out = vec![b'_'];
        out.extend_from_slice(origin.as_bytes());
        out.extend_from_slice(&[0, 1]);
        out.extend_from_slice(name.as_bytes());
        out
    }

    /// A Latin-1 value (text of characters up to U+00FF) as Chromium stores it.
    fn latin1(text: &str) -> Vec<u8> {
        let mut out = vec![1];
        out.extend(text.chars().map(|c| u8::try_from(u32::from(c)).expect("latin-1 text")));
        out
    }

    /// A UTF-16 value.
    fn utf16(text: &str) -> Vec<u8> {
        let mut out = vec![0];
        for unit in text.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out
    }

    const ORIGIN: &str = "http://tauri.localhost";

    fn saved_state(pinned: &str) -> String {
        format!(r#"{{"state":{{"pinned":["{pinned}"],"settings":{{"theme":"dark"}}}},"version":0}}"#)
    }

    /// An old LevelDB: the settings in a Snappy table (as compaction leaves them), the newer writes in the log.
    fn old_folder(name: &str) -> Folder {
        let folder = Folder::new(name);
        let k_state = key(ORIGIN, STORAGE_KEY);
        let k_places = key(ORIGIN, PLACES_KEY);
        let k_size = key(ORIGIN, SIZE_KEY);
        let k_tags = key(ORIGIN, RECENT_TAGS_KEY);
        let k_version = key(ORIGIN, LAST_VERSION_KEY);
        let k_notice = key(ORIGIN, "offer-instant-search");
        let k_gone = key(ORIGIN, "service-update-notice");
        let v_old = latin1(&saved_state("C:\\\\Old"));
        let v_places =
            latin1(r#"{"c:\\books\\a.epub":"epubcfi(/6/14!/4/2/1:0)","c:\\books\\b.epub":"epubcfi(/6/4!/4)"}"#);
        let v_gone = latin1("0.9.5");
        let k_meta = b"META:http://tauri.localhost".to_vec();
        let v_meta = [8, 1];
        let k_other = key("https://example.com", STORAGE_KEY);
        let v_other = latin1(&saved_state("D:\\\\Other"));
        let ldb = table(
            &[
                (&k_state, 1, Some(&v_old[..])),
                (&k_places, 2, Some(&v_places[..])),
                (&k_gone, 3, Some(&v_gone[..])),
                (&k_meta, 4, Some(&v_meta[..])),
                (&k_other, 5, Some(&v_other[..])),
                (b"VERSION", 6, Some(&b"1"[..])),
            ],
            3,
            true,
        );
        std::fs::write(folder.path("000005.ldb"), ldb).expect("writes");
        // The log: a newer state (with an Arabic folder, so UTF-16), a size, tags, a version and a notice; the
        // notice the table had is deleted.
        let v_new = utf16(&saved_state("C:\\\\مجلد"));
        let v_tags = latin1(r#"["Work","Café",7,""]"#);
        std::fs::write(
            folder.path("000004.log"),
            log(&[
                batch(
                    10,
                    &[
                        Op::Put(&k_state, &v_new),
                        Op::Put(&k_size, &latin1("130")),
                        Op::Put(&k_tags, &v_tags),
                    ],
                ),
                batch(
                    20,
                    &[
                        Op::Put(&k_version, &latin1("0.9.5")),
                        Op::Put(&k_notice, &latin1("1")),
                        Op::Delete(&k_gone),
                    ],
                ),
            ]),
        )
        .expect("writes");
        std::fs::write(folder.path("CURRENT"), "MANIFEST-000001\n").expect("writes");
        std::fs::write(folder.path("LOCK"), "").expect("writes");
        folder
    }

    /// Every file of a folder with its bytes.
    fn contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .expect("lists")
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read(e.path()).unwrap_or_default(),
                )
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn utf16_and_latin1_values_and_names_are_decoded() {
        assert_eq!(decode_value(&latin1("caf\u{e9}")).as_deref(), Some("café"));
        assert_eq!(
            decode_value(&utf16("caf\u{e9} \u{645}\u{631}\u{62d}\u{628}\u{627} \u{1f600}")).as_deref(),
            Some("café مرحبا 😀")
        );
        assert_eq!(decode_value(&[]), None);
        assert_eq!(decode_value(&[7, b'x']), None);
        // An odd trailing byte of UTF-16 is dropped, a lone surrogate becomes U+FFFD.
        assert_eq!(decode_value(&[0, b'a', 0, b'b']).as_deref(), Some("a"));
        assert_eq!(decode_value(&[0, 0x00, 0xd8]).as_deref(), Some("\u{fffd}"));
        // A name stored as UTF-16 works too.
        let mut name = vec![b'_'];
        name.extend_from_slice(b"https://tauri.localhost");
        name.extend_from_slice(&[0, 0]);
        for unit in "last-version".encode_utf16() {
            name.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(split_key(&name).as_deref(), Some("last-version"));
    }

    #[test]
    fn only_the_pages_own_keys_are_kept() {
        assert_eq!(split_key(&key("http://tauri.localhost", "a")).as_deref(), Some("a"));
        assert_eq!(split_key(&key("https://tauri.localhost", "a")).as_deref(), Some("a"));
        assert_eq!(split_key(&key("https://example.com", "a")), None);
        assert_eq!(split_key(b"META:http://tauri.localhost"), None);
        assert_eq!(split_key(b"METAACCESS:http://tauri.localhost"), None);
        assert_eq!(split_key(b"VERSION"), None);
        assert_eq!(split_key(b"_http://tauri.localhost"), None);
    }

    #[test]
    fn the_old_folder_reads_with_the_newest_writes_and_the_deletes() {
        let folder = old_folder("reads");
        let before = contents(&folder.0);
        let data = read_old(&folder.0).expect("reads");
        assert!(data.problems.is_empty(), "{:?}", data.problems);
        // The newer state (UTF-16, Arabic) won over the table's.
        let store = data.store.clone().expect("store");
        assert!(store.contains("مجلد") && !store.contains("Old"), "{store}");
        assert_eq!(load_persisted(&store).pinned, vec!["C:\\مجلد".to_string()]);
        assert_eq!(data.book_size, Some(130));
        assert_eq!(data.recent_tags, vec!["Work".to_string(), "Café".to_string()]);
        assert_eq!(data.last_version.as_deref(), Some("0.9.5"));
        assert_eq!(data.notices.get("offer-instant-search").map(String::as_str), Some("1"));
        assert!(
            !data.notices.contains_key("service-update-notice"),
            "deleted in the log"
        );
        // The places keep the order they were written in.
        let paths: Vec<&str> = data.book_places.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["c:\\books\\a.epub", "c:\\books\\b.epub"]);
        // Reading changed nothing.
        assert_eq!(contents(&folder.0), before);
        // The folder around it works as well, and a missing one is an error.
        let around = Folder::new("around");
        let inner = around.path("EBWebView/Default/Local Storage/leveldb");
        std::fs::create_dir_all(&inner).expect("folders");
        for (name, bytes) in &before {
            std::fs::write(inner.join(name), bytes).expect("copies");
        }
        assert_eq!(read_old(&around.0).expect("reads"), data);
        assert!(read_old(&folder.path("nothing")).is_err());
    }

    #[test]
    fn a_book_place_is_the_chapter_the_cfi_names() {
        let place = |cfi: &str| place_from_cfi(cfi).map(|p| (p.chapter, p.block));
        assert_eq!(place("epubcfi(/6/14!/4/2/1:0)"), Some((6, 0)));
        assert_eq!(place("epubcfi(/6/2[cover]!/4)"), Some((0, 0)));
        assert_eq!(place("epubcfi(/6/4!/4/2,/1:0,/1:5)"), Some((1, 0)));
        assert_eq!(place("/6/8"), Some((3, 0)));
        assert_eq!(place("epubcfi(/6/0!/4)"), None);
        assert_eq!(place("epubcfi(/6/7!/4)"), None);
        assert_eq!(place("epubcfi(/4/2)"), None);
        assert_eq!(place(""), None);
        assert_eq!(place("epubcfi(/6/99999999999999999999999999)"), None);
    }

    #[test]
    fn the_size_is_read_as_the_old_code_did() {
        let one = |text: &str| {
            let mut db = leveldb::Db::new(is_page_key);
            db.read_log(&log(&[batch(1, &[Op::Put(&key(ORIGIN, SIZE_KEY), &latin1(text))])]));
            old_data(db).book_size
        };
        assert_eq!(one("130"), Some(130));
        assert_eq!(one("85.5"), Some(86));
        assert_eq!(one("0"), None, "Number('0') || 100 is 100");
        assert_eq!(one("big"), None);
        assert_eq!(one(""), None);
    }

    #[test]
    fn the_import_writes_state_and_books_once_and_never_touches_the_old_folder() {
        let old = old_folder("import");
        let new = Folder::new("import-new");
        let (state_file, books_file) = (new.path("state.json"), new.path("books.json"));
        let before = contents(&old.0);

        let done = first_start_at(Some(&old.0), &state_file, Some(&books_file))
            .expect("imports")
            .expect("it was the first start");
        assert!(done.state_written);
        assert_eq!(done.books_written, Some(2));
        let loaded = crate::persistence::load_from(&state_file);
        assert_eq!(loaded.state.pinned, vec!["C:\\مجلد".to_string()]);
        assert_eq!(loaded.state.settings.theme, glint_model::store::Theme::Dark);
        let books = crate::book_places::BookPlaces::load(Some(books_file.clone()));
        assert_eq!(books.text_size(), 130);
        assert_eq!(books.place("C:\\Books\\A.epub"), Some(Place { chapter: 6, block: 0 }));
        assert_eq!(books.place("c:\\books\\b.epub"), Some(Place { chapter: 1, block: 0 }));
        assert_eq!(done.data.recent_tags, vec!["Work".to_string(), "Café".to_string()]);

        // A later start never imports again, whatever the files hold now.
        std::fs::write(&state_file, "changed since").expect("writes");
        std::fs::write(&books_file, "changed too").expect("writes");
        assert_eq!(first_start_at(Some(&old.0), &state_file, Some(&books_file)), Ok(None));
        assert_eq!(std::fs::read_to_string(&state_file).expect("reads"), "changed since");
        assert_eq!(std::fs::read_to_string(&books_file).expect("reads"), "changed too");
        // And the old folder is as it was.
        assert_eq!(contents(&old.0), before);
    }

    #[test]
    fn books_that_are_already_there_are_not_replaced() {
        let old = old_folder("books-there");
        let new = Folder::new("books-there-new");
        let books_file = new.path("books.json");
        std::fs::write(&books_file, r#"{"size": 90, "places": []}"#).expect("writes");
        let done = first_start_at(Some(&old.0), &new.path("state.json"), Some(&books_file))
            .expect("imports")
            .expect("first start");
        assert_eq!(done.books_written, None);
        assert_eq!(crate::book_places::BookPlaces::load(Some(books_file)).text_size(), 90);
    }

    #[test]
    fn a_locked_file_is_read_through_a_copy() {
        let old = old_folder("locked");
        let before = contents(&old.0);
        // Every read of a file in the old folder fails, as it does while the old app has it locked; a copy
        // made in the temp folder reads fine.
        let folder = old.0.clone();
        let locked = move |path: &Path| -> std::io::Result<Vec<u8>> {
            if path.starts_with(&folder) {
                Err(std::io::Error::from_raw_os_error(32))
            } else {
                std::fs::read(path)
            }
        };
        let data = read_old_with(&old.0, &locked).expect("reads");
        assert!(data.problems.is_empty(), "{:?}", data.problems);
        assert_eq!(data, read_old(&old.0).expect("reads"));
        assert_eq!(data.book_size, Some(130));
        assert_eq!(contents(&old.0), before, "the original is untouched");
    }

    #[test]
    fn a_file_that_cannot_be_read_even_as_a_copy_is_a_note() {
        let old = old_folder("unreadable");
        let nothing = |_: &Path| -> std::io::Result<Vec<u8>> { Err(std::io::Error::from_raw_os_error(32)) };
        // The copy is read with `std::fs::read`, so the copy works; make the file itself unreadable by
        // removing it between listing and reading instead.
        let folder = old.0.clone();
        let vanishing = move |path: &Path| -> std::io::Result<Vec<u8>> {
            if path.starts_with(&folder) {
                let _ = std::fs::remove_file(path);
            }
            nothing(path)
        };
        let data = read_old_with(&old.0, &vanishing).expect("reads");
        assert!(data.store.is_none());
        assert!(!data.problems.is_empty());
    }

    #[test]
    fn a_missing_or_damaged_folder_starts_clean() {
        let new = Folder::new("clean-new");
        let (state_file, books_file) = (new.path("state.json"), new.path("books.json"));
        // No old folder at all, and one that is not a LevelDB: nothing happens, nothing is written.
        assert_eq!(first_start_at(None, &state_file, Some(&books_file)), Ok(None));
        let missing = Folder::new("clean-missing");
        assert_eq!(
            first_start_at(Some(&missing.path("nope")), &state_file, Some(&books_file)),
            Ok(None)
        );
        assert_eq!(
            first_start_at(Some(&missing.0), &state_file, Some(&books_file)),
            Ok(None)
        );
        // A LevelDB made of noise: an error saying why, no state.json (the next start may try again).
        let damaged = Folder::new("clean-damaged");
        let noise: Vec<u8> = (0..3000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
            .collect();
        std::fs::write(damaged.path("000003.log"), &noise).expect("writes");
        std::fs::write(damaged.path("000005.ldb"), &noise).expect("writes");
        let error = first_start_at(Some(&damaged.0), &state_file, Some(&books_file)).expect_err("damaged");
        assert!(error.contains("000005.ldb"), "{error}");
        assert!(!state_file.exists() && !books_file.exists());
        // The app then loads the defaults.
        assert_eq!(
            crate::persistence::load_from(&state_file).state,
            glint_model::store::AppState::default()
        );
    }

    #[test]
    fn an_old_folder_without_our_keys_settles_on_the_defaults_once() {
        let old = Folder::new("empty-old");
        std::fs::write(
            old.path("000004.log"),
            log(&[batch(1, &[Op::Put(&key("https://example.com", "x"), &latin1("1"))])]),
        )
        .expect("writes");
        let new = Folder::new("empty-new");
        let done = first_start_at(Some(&old.0), &new.path("state.json"), None)
            .expect("imports")
            .expect("first start");
        assert!(done.state_written && done.data.store.is_none());
        assert_eq!(
            crate::persistence::load_from(&new.path("state.json")).state,
            glint_model::store::AppState::default()
        );
        assert_eq!(first_start_at(Some(&old.0), &new.path("state.json"), None), Ok(None));
    }

    #[test]
    fn a_secondary_window_never_imports() {
        let _guard = crate::testing::lock();
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\Work"));
        let old = old_folder("secondary");
        let new = Folder::new("secondary-new");
        assert_eq!(
            first_start_at(Some(&old.0), &new.path("state.json"), Some(&new.path("books.json"))),
            Ok(None)
        );
        assert!(contents(&new.0).is_empty());
    }

    #[test]
    fn a_stored_state_of_another_version_is_not_taken() {
        let mut db = leveldb::Db::new(is_page_key);
        db.read_log(&log(&[batch(
            1,
            &[Op::Put(
                &key(ORIGIN, STORAGE_KEY),
                &latin1(r#"{"state":{},"version":3}"#),
            )],
        )]));
        let data = old_data(db);
        assert!(data.store.is_none());
        assert_eq!(data.problems.len(), 1);
    }

    /// Reads this PC's real folder (read-only) and prints what it holds: `cargo test -p glint-ui
    /// real_folder -- --ignored --nocapture`.
    #[test]
    #[ignore = "reads the real EBWebView folder of this PC"]
    fn real_folder_dry_run() {
        let Some(dir) = default_dir() else { return };
        match read_old(&dir) {
            Ok(data) => {
                let state = data.store.as_deref().map(load_persisted);
                println!("store: {} bytes", data.store.as_deref().map_or(0, str::len));
                if let Some(state) = state {
                    println!(
                        "pinned {}, history {}, saved searches {}, remotes {}, rules {}, visits {}, folder views {}",
                        state.pinned.len(),
                        state.history.len(),
                        state.saved_searches.len(),
                        state.remotes.len(),
                        state.rules.len(),
                        state.visits.len(),
                        state.folder_views.len()
                    );
                    println!(
                        "theme {:?}, language {:?}",
                        state.settings.theme, state.settings.language
                    );
                }
                println!("places {:?}", data.book_places);
                println!(
                    "size {:?}, tags {:?}, last-version {:?}",
                    data.book_size, data.recent_tags, data.last_version
                );
                println!("notices {:?}", data.notices);
                println!("problems {:?}", data.problems);
            }
            Err(why) => println!("no old settings: {why}"),
        }
    }
}
