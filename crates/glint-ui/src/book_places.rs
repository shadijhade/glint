//! Where each book was left, and the reader's text size, kept between runs (the `localStorage` of BookView.tsx:
//! `places()` and `startingSize()`).
//!
//! They live in a small file of their own beside `state.json`: `%APPDATA%\com.shado.shadui-ripgrep\books.json`.
//! It is read once, the first time a book is opened, and written a moment after the last change (so turning pages
//! writes once, not per page); [`BookPlaces::flush`] writes at once, for quitting. The last [`KEEP`] books are
//! kept, keyed by their lower-case path. A file that can't be understood is moved aside (`books.corrupt-….json`)
//! and the reader starts with nothing; a file that can't be written is left for the next change to try again.
//!
//! **Tests** never touch the real folder: in a test build [`shared`] is in memory only, and tests that want a file
//! make a [`BookPlaces`] on a temp folder with [`BookPlaces::load`].
//!
//! **Importing the old app's places:** the first start of this app writes this file from the old reader's
//! `localStorage` ([`import_old`], called by `old_settings`).
//!
//! **Wiring (coordinator):** `pub mod book_places;` in lib.rs; `book_places::shared().flush()` where the app
//! flushes its state on quit (optional: a change is written half a second after it settles either way).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// The file's name, in the app data folder.
pub const FILE: &str = "books.json";
/// How many books' places are kept.
pub const KEEP: usize = 200;
/// The smallest text size, percent.
pub const MIN_SIZE: u32 = 70;
/// The largest text size, percent.
pub const MAX_SIZE: u32 = 200;
/// The text size to start with, percent.
pub const DEFAULT_SIZE: u32 = 100;
/// How long after the last change the file is written.
const SETTLE: Duration = Duration::from_millis(500);

/// Where a book was left: the chapter and the block at the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place {
    pub chapter: usize,
    pub block: usize,
}

#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    version: u32,
    #[serde(default = "default_size")]
    size: u32,
    /// Oldest first.
    #[serde(default)]
    places: Vec<SavedPlace>,
}

#[derive(Serialize, Deserialize)]
struct SavedPlace {
    path: String,
    chapter: usize,
    block: usize,
}

fn default_size() -> u32 {
    DEFAULT_SIZE
}

struct Inner {
    /// Where it is saved; none keeps it in memory only.
    path: Option<PathBuf>,
    /// Oldest first, keyed by the lower-case path.
    places: Vec<(String, Place)>,
    size: u32,
    /// When the last unsaved change was made.
    changed: Option<Instant>,
    /// Whether a thread is waiting to write.
    waiting: bool,
    settle: Duration,
}

/// The saved places and text size.
#[derive(Clone)]
pub struct BookPlaces {
    inner: Arc<Mutex<Inner>>,
}

/// Where the real file is.
pub fn file_path() -> Option<PathBuf> {
    glint_core::host::PathResolver
        .app_data_dir()
        .ok()
        .map(|dir| dir.join(FILE))
}

/// The app's places: read from the real file on first use (in a test build, in memory only).
pub fn shared() -> &'static BookPlaces {
    static SHARED: OnceLock<BookPlaces> = OnceLock::new();
    SHARED.get_or_init(|| {
        if cfg!(test) {
            BookPlaces::in_memory()
        } else {
            BookPlaces::load(file_path())
        }
    })
}

fn set_aside(path: &Path) {
    // Only the main window moves a damaged books.json aside.
    if crate::platform::secondary::is_secondary() {
        return;
    }
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem()) else {
        return;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let target = dir.join(format!("{}.corrupt-{stamp}.json", stem.to_string_lossy()));
    if !target.exists() {
        std::fs::rename(path, target).ok();
    }
}

impl BookPlaces {
    /// Places that are never saved.
    pub fn in_memory() -> Self {
        Self::make(None, Vec::new(), DEFAULT_SIZE, SETTLE)
    }

    /// The places saved at `path` (none given or none there: nothing yet), saved there again after changes.
    pub fn load(path: Option<PathBuf>) -> Self {
        Self::load_settling(path, SETTLE)
    }

    /// [`Self::load`] writing `settle` after the last change.
    pub fn load_settling(path: Option<PathBuf>, settle: Duration) -> Self {
        let (mut places, mut size) = (Vec::new(), DEFAULT_SIZE);
        if let Some(file) = &path {
            if let Ok(text) = std::fs::read_to_string(file) {
                match serde_json::from_str::<Saved>(&text) {
                    Ok(saved) => {
                        size = saved.size.clamp(MIN_SIZE, MAX_SIZE);
                        places = saved
                            .places
                            .into_iter()
                            .map(|p| {
                                (
                                    p.path.to_lowercase(),
                                    Place {
                                        chapter: p.chapter,
                                        block: p.block,
                                    },
                                )
                            })
                            .collect();
                        let extra = places.len().saturating_sub(KEEP);
                        places.drain(..extra);
                    }
                    Err(_) => set_aside(file),
                }
            }
        }
        Self::make(path, places, size, settle)
    }

    fn make(path: Option<PathBuf>, places: Vec<(String, Place)>, size: u32, settle: Duration) -> Self {
        BookPlaces {
            inner: Arc::new(Mutex::new(Inner {
                path,
                places,
                size,
                changed: None,
                waiting: false,
                settle,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Where the book at `path` was left.
    pub fn place(&self, path: &str) -> Option<Place> {
        let key = path.to_lowercase();
        self.lock().places.iter().find(|(p, _)| *p == key).map(|(_, p)| *p)
    }

    /// Remembers where the book at `path` was left; the last [`KEEP`] books are kept.
    pub fn remember(&self, path: &str, place: Place) {
        let key = path.to_lowercase();
        {
            let mut inner = self.lock();
            if inner.places.last().is_some_and(|(p, q)| *p == key && *q == place) {
                return;
            }
            inner.places.retain(|(p, _)| *p != key);
            inner.places.push((key, place));
            let extra = inner.places.len().saturating_sub(KEEP);
            inner.places.drain(..extra);
        }
        self.changed();
    }

    /// The text size in percent.
    pub fn text_size(&self) -> u32 {
        self.lock().size
    }

    /// Sets the text size, clamped to 70% to 200%.
    pub fn set_text_size(&self, percent: u32) {
        {
            let mut inner = self.lock();
            let size = percent.clamp(MIN_SIZE, MAX_SIZE);
            if inner.size == size {
                return;
            }
            inner.size = size;
        }
        self.changed();
    }

    /// Notes a change and makes sure something will write it once changes stop.
    fn changed(&self) {
        let start = {
            let mut inner = self.lock();
            if inner.path.is_none() {
                return;
            }
            inner.changed = Some(Instant::now());
            !std::mem::replace(&mut inner.waiting, true)
        };
        if start {
            let this = self.clone();
            let spawned = std::thread::Builder::new()
                .name("glint-book-places".into())
                .spawn(move || this.wait_and_write());
            if spawned.is_err() {
                self.lock().waiting = false;
            }
        }
    }

    fn wait_and_write(&self) {
        loop {
            let wait = {
                let mut inner = self.lock();
                match inner.changed {
                    Some(at) => inner.settle.checked_sub(at.elapsed()),
                    None => {
                        inner.waiting = false;
                        return;
                    }
                }
            };
            match wait {
                Some(wait) if !wait.is_zero() => std::thread::sleep(wait),
                _ => {
                    if !self.flush() {
                        // Not written: the next change wakes it again.
                        self.lock().waiting = false;
                        return;
                    }
                    let mut inner = self.lock();
                    // A change made while writing is waited for again.
                    if inner.changed.is_none() {
                        inner.waiting = false;
                        return;
                    }
                }
            }
        }
    }

    /// Writes now what hasn't been written. Whether it is saved (or there is nothing to save).
    pub fn flush(&self) -> bool {
        let (path, text, stamp) = {
            let inner = self.lock();
            let (Some(path), Some(stamp)) = (inner.path.clone(), inner.changed) else {
                return true;
            };
            let saved = Saved {
                version: 1,
                size: inner.size,
                places: inner
                    .places
                    .iter()
                    .map(|(path, place)| SavedPlace {
                        path: path.clone(),
                        chapter: place.chapter,
                        block: place.block,
                    })
                    .collect(),
            };
            match serde_json::to_string(&saved) {
                Ok(text) => (path, text, stamp),
                Err(_) => return false,
            }
        };
        let written = write_atomically(&path, &text);
        let mut inner = self.lock();
        // Unchanged since: saved. A failed write stays pending.
        if written && inner.changed == Some(stamp) {
            inner.changed = None;
        }
        written
    }
}

/// Writes the places and text size the old app kept in WebView2's `localStorage` to `path`, as this file, if
/// there is no file there yet (`places` oldest first, keyed as [`BookPlaces`] keys them; `size` in percent, clamped
/// like [`BookPlaces::set_text_size`]). Whether a file was written; nothing is written when there is nothing worth
/// keeping (no places and the default size).
pub fn import_old(path: &Path, places: &[(String, Place)], size: Option<u32>) -> bool {
    if path.exists() {
        return false;
    }
    let size = size.map_or(DEFAULT_SIZE, |s| s.clamp(MIN_SIZE, MAX_SIZE));
    let mut kept: Vec<(String, Place)> = Vec::new();
    for (book, place) in places {
        let key = book.to_lowercase();
        kept.retain(|(k, _)| *k != key);
        kept.push((key, *place));
    }
    let extra = kept.len().saturating_sub(KEEP);
    kept.drain(..extra);
    if kept.is_empty() && size == DEFAULT_SIZE {
        return false;
    }
    let saved = Saved {
        version: 1,
        size,
        places: kept
            .into_iter()
            .map(|(path, place)| SavedPlace {
                path,
                chapter: place.chapter,
                block: place.block,
            })
            .collect(),
    };
    match serde_json::to_string(&saved) {
        Ok(text) => write_atomically(path, &text),
        Err(_) => false,
    }
}

fn write_atomically(path: &Path, text: &str) -> bool {
    // A secondary window keeps nothing of its own: its places count as saved, so nothing retries.
    if crate::platform::secondary::is_secondary() {
        return true;
    }
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).is_ok() && std::fs::rename(&temp, path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("glint-book-places-{name}-{}", std::process::id()));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Folder(dir)
        }
        fn file(&self) -> PathBuf {
            self.0.join(FILE)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    const QUICK: Duration = Duration::from_millis(20);

    fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
        for _ in 0..300 {
            if ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never: {what}");
    }

    #[test]
    fn places_are_keyed_by_lower_case_path_and_the_last_200_are_kept() {
        let places = BookPlaces::in_memory();
        let place = Place { chapter: 2, block: 7 };
        places.remember("C:\\Books\\A.epub", place);
        assert_eq!(places.place("c:\\books\\a.epub"), Some(place));
        assert_eq!(places.place("c:\\books\\other.epub"), None);
        places.remember("C:\\Books\\A.epub", Place { chapter: 3, block: 0 });
        assert_eq!(places.place("c:\\books\\a.epub").map(|p| p.chapter), Some(3));
        for i in 0..250 {
            places.remember(&format!("C:\\many\\{i}.epub"), place);
        }
        assert_eq!(places.place("c:\\books\\a.epub"), None);
        assert_eq!(places.place("c:\\many\\50.epub"), Some(place));
        assert_eq!(places.place("c:\\many\\49.epub"), None);
    }

    #[test]
    fn the_text_size_stays_between_70_and_200() {
        let places = BookPlaces::in_memory();
        assert_eq!(places.text_size(), 100);
        places.set_text_size(500);
        assert_eq!(places.text_size(), 200);
        places.set_text_size(10);
        assert_eq!(places.text_size(), 70);
    }

    #[test]
    fn what_was_flushed_is_there_in_the_next_run() {
        let folder = Folder::new("roundtrip");
        let first = BookPlaces::load_settling(Some(folder.file()), Duration::from_secs(3600));
        first.remember("C:\\Books\\Ä.epub", Place { chapter: 4, block: 9 });
        first.set_text_size(130);
        assert!(!folder.file().exists(), "nothing is written before changes settle");
        assert!(first.flush());
        let second = BookPlaces::load(Some(folder.file()));
        assert_eq!(second.place("c:\\books\\ä.epub"), Some(Place { chapter: 4, block: 9 }));
        assert_eq!(second.text_size(), 130);
    }

    #[test]
    fn a_change_is_written_once_changes_settle() {
        let folder = Folder::new("settle");
        let places = BookPlaces::load_settling(Some(folder.file()), QUICK);
        for chapter in 0..20 {
            places.remember("C:\\Books\\a.epub", Place { chapter, block: 0 });
        }
        eventually("the last place", || {
            BookPlaces::load(Some(folder.file())).place("c:\\books\\a.epub") == Some(Place { chapter: 19, block: 0 })
        });
        // And a later change is written too.
        places.set_text_size(150);
        eventually("the size", || BookPlaces::load(Some(folder.file())).text_size() == 150);
    }

    #[test]
    fn an_unchanged_value_writes_nothing() {
        let folder = Folder::new("quiet");
        let places = BookPlaces::load_settling(Some(folder.file()), QUICK);
        places.set_text_size(100);
        places.remember("C:\\a.epub", Place { chapter: 1, block: 1 });
        eventually("written", || folder.file().exists());
        std::thread::sleep(Duration::from_millis(100));
        std::fs::remove_file(folder.file()).unwrap();
        places.set_text_size(100);
        places.remember("C:\\A.epub", Place { chapter: 1, block: 1 });
        std::thread::sleep(Duration::from_millis(150));
        assert!(!folder.file().exists());
    }

    #[test]
    fn a_file_that_is_not_ours_is_moved_aside() {
        let folder = Folder::new("corrupt");
        std::fs::write(folder.file(), "not json {").unwrap();
        let places = BookPlaces::load(Some(folder.file()));
        assert_eq!(places.text_size(), 100);
        assert!(!folder.file().exists());
        let kept = std::fs::read_dir(&folder.0)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains("corrupt"));
        assert!(kept);
    }

    #[test]
    fn a_missing_file_or_odd_values_load_safely() {
        let folder = Folder::new("odd");
        assert_eq!(BookPlaces::load(Some(folder.file())).text_size(), 100);
        std::fs::write(
            folder.file(),
            r#"{"size": 9999, "places": [{"path": "C:\\X.epub", "chapter": 1, "block": 2}]}"#,
        )
        .unwrap();
        let places = BookPlaces::load(Some(folder.file()));
        assert_eq!(places.text_size(), 200);
        assert_eq!(places.place("c:\\x.epub"), Some(Place { chapter: 1, block: 2 }));
    }

    #[test]
    fn old_places_are_imported_once_into_a_file_that_is_not_there_yet() {
        let folder = Folder::new("import");
        let old = vec![
            (r"C:\Books\A.epub".to_string(), Place { chapter: 1, block: 0 }),
            (r"c:\books\b.epub".to_string(), Place { chapter: 5, block: 0 }),
            (r"c:\books\a.epub".to_string(), Place { chapter: 2, block: 0 }),
        ];
        assert!(import_old(&folder.file(), &old, Some(500)));
        let places = BookPlaces::load(Some(folder.file()));
        assert_eq!(places.text_size(), 200);
        assert_eq!(
            places.place(r"c:\books\a.epub"),
            Some(Place { chapter: 2, block: 0 }),
            "the later one wins"
        );
        assert_eq!(places.place(r"C:\Books\B.epub"), Some(Place { chapter: 5, block: 0 }));
        // A file that is there is left alone.
        assert!(!import_old(&folder.file(), &[], Some(150)));
        assert_eq!(BookPlaces::load(Some(folder.file())).text_size(), 200);
        // Nothing worth keeping writes nothing.
        let other = Folder::new("import-nothing");
        assert!(!import_old(&other.file(), &[], None));
        assert!(!import_old(&other.file(), &[], Some(100)));
        assert!(!other.file().exists());
        // Only the last 200 books are kept.
        let many: Vec<(String, Place)> = (0..250)
            .map(|i| (format!(r"C:\many\{i}.epub"), Place { chapter: i, block: 0 }))
            .collect();
        let third = Folder::new("import-many");
        assert!(import_old(&third.file(), &many, None));
        let places = BookPlaces::load(Some(third.file()));
        assert_eq!(places.place(r"c:\many\49.epub"), None);
        assert_eq!(places.place(r"c:\many\50.epub").map(|p| p.chapter), Some(50));
    }

    #[test]
    fn the_shared_places_in_a_test_never_use_the_real_folder() {
        shared().set_text_size(100);
        assert!(shared().lock().path.is_none());
    }
}
