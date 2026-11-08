//! Photo search and speech search (`shows:` and `says:` in the search box): what's in pictures, and
//! what's said in recordings and videos, each learned by a local model (photos.rs: CLIP; speech.rs:
//! Whisper) and never leaving the PC. Each keeps what it learned of a file under the file's ID, so a
//! move or rename keeps it, reads the folders chosen in Settings in the background, at low priority,
//! and answers searches as results in the explorer.

use crate::tools::{self, Installs, Progress as Download};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, RwLock};
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::State;

/// What's turned on, and where each looks.
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub photos: bool,
    pub photo_folders: Vec<String>,
    pub speech: bool,
    pub speech_folders: Vec<String>,
}

fn config_path() -> Option<PathBuf> {
    tools::data_dir().map(|d| d.join("media-ai.json"))
}

pub fn config() -> Config {
    config_path().and_then(|p| fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// A file's identity: its volume's serial number and its ID there (NTFS's, or ReFS's 128-bit one).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileKey {
    pub serial: u64,
    pub id: u128,
}

#[cfg(windows)]
pub fn file_key(path: &Path) -> Option<FileKey> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO};
    // Attributes alone, shared every way, with backup semantics so a folder opens too.
    let file = fs::OpenOptions::new().access_mode(0x80).share_mode(7).custom_flags(0x0200_0000).open(path).ok()?;
    let mut info = FILE_ID_INFO::default();
    unsafe { GetFileInformationByHandleEx(HANDLE(file.as_raw_handle()), FileIdInfo, &mut info as *mut _ as *mut _, std::mem::size_of::<FILE_ID_INFO>() as u32) }.ok()?;
    Some(FileKey { serial: info.VolumeSerialNumber, id: u128::from_le_bytes(info.FileId.Identifier) })
}

#[cfg(not(windows))]
pub fn file_key(path: &Path) -> Option<FileKey> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).ok()?;
    Some(FileKey { serial: meta.dev(), id: meta.ino() as u128 })
}

/// A file a pass found.
#[derive(Clone, Debug)]
pub struct Found {
    pub path: PathBuf,
    pub size: u64,
    pub modified: i64,
}

/// The files below `folders` with one of `exts` (lowercase, without the dot). Links and junctions
/// aren't followed, nor hidden folders (".git", AppData).
pub fn walk(folders: &[String], exts: &[&str], stop: &AtomicBool) -> Vec<Found> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut todo: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
    while let Some(dir) = todo.pop() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if !seen.insert(dir.to_string_lossy().to_lowercase()) {
            continue;
        }
        let Ok(items) = fs::read_dir(&dir) else { continue };
        for item in items.flatten() {
            let Ok(kind) = item.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            let name = item.file_name();
            let name = name.to_string_lossy();
            if kind.is_dir() {
                if !name.starts_with('.') && !name.eq_ignore_ascii_case("AppData") && !name.starts_with('$') {
                    todo.push(item.path());
                }
                continue;
            }
            let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
            if !exts.contains(&ext.as_str()) {
                continue;
            }
            let Ok(meta) = item.metadata() else { continue };
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64);
            out.push(Found { path: item.path(), size: meta.len(), modified });
        }
    }
    out
}

/// What one model learned of one file; `data` None when the file couldn't be read (so it isn't tried
/// again until it changes).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Item<T> {
    pub serial: u64,
    /// The file ID, in hex.
    pub id: String,
    pub path: String,
    pub size: u64,
    pub modified: i64,
    pub data: Option<T>,
}

impl<T> Item<T> {
    pub fn key(&self) -> Option<FileKey> {
        Some(FileKey { serial: self.serial, id: u128::from_str_radix(&self.id, 16).ok()? })
    }
}

/// How a model's pass is going, for Settings.
#[derive(Serialize, Clone, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Files it has learned (and could read).
    pub known: usize,
    /// Files waiting in this pass.
    pub pending: usize,
    pub working: bool,
    /// Why the last pass stopped short, if it did.
    pub error: String,
}

/// One model's side: what it learned (kept in Glint's data folder as JSON lines), its progress, and
/// its background pass.
pub struct Library<T> {
    file: &'static str,
    pub items: RwLock<HashMap<FileKey, Item<T>>>,
    loaded: AtomicBool,
    progress: Mutex<Progress>,
    /// Bumped by a change of settings (or a model installed): the pass in progress starts over.
    generation: AtomicU64,
    wake: (Mutex<bool>, Condvar),
}

impl<T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static> Library<T> {
    pub fn new(file: &'static str) -> Self {
        Library { file, items: RwLock::new(HashMap::new()), loaded: AtomicBool::new(false), progress: Mutex::new(Progress { known: 0, pending: 0, working: false, error: String::new() }), generation: AtomicU64::new(0), wake: (Mutex::new(false), Condvar::new()) }
    }

    fn path(&self) -> Option<PathBuf> {
        tools::data_dir().map(|d| d.join(self.file))
    }

    /// Reads what was learned before, once.
    pub fn load(&self) {
        if self.loaded.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(file) = self.path().and_then(|p| fs::File::open(p).ok()) else { return };
        let mut items = self.items.write().unwrap_or_else(|e| e.into_inner());
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            if let Ok(item) = serde_json::from_str::<Item<T>>(&line) {
                if let Some(key) = item.key() {
                    items.insert(key, item);
                }
            }
        }
        let known = items.values().filter(|i| i.data.is_some()).count();
        self.progress.lock().unwrap_or_else(|e| e.into_inner()).known = known;
    }

    /// Writes what's learned, whole (a new file that then replaces the old one).
    pub fn save(&self) {
        let Some(path) = self.path() else { return };
        let fresh = path.with_extension("new");
        let written = (|| -> std::io::Result<()> {
            let mut out = std::io::BufWriter::new(fs::File::create(&fresh)?);
            for item in self.items.read().unwrap_or_else(|e| e.into_inner()).values() {
                serde_json::to_writer(&mut out, item)?;
                out.write_all(b"\n")?;
            }
            out.flush()
        })();
        if written.is_ok() {
            let _ = fs::rename(&fresh, &path);
        } else {
            let _ = fs::remove_file(&fresh);
        }
    }

    pub fn progress(&self) -> Progress {
        self.progress.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set(&self, change: impl FnOnce(&mut Progress)) {
        change(&mut self.progress.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// Why the pass stops (a model's `learn` then says STOP).
    pub fn fail(&self, why: &str) {
        let why = why.to_string();
        self.set(|p| p.error = why);
    }

    /// Starts the pass over (settings changed, a model was installed).
    pub fn poke(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let (lock, wake) = &self.wake;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_all();
    }

    /// Waits for a poke, or `period`.
    fn sleep(&self, period: Duration) {
        let (lock, wake) = &self.wake;
        let poked = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (mut poked, _) = wake.wait_timeout_while(poked, period, |poked| !*poked).unwrap_or_else(|e| e.into_inner());
        *poked = false;
    }

    /// One pass: the folders read for files of `exts`, what's new or changed learned with `learn`,
    /// what's gone forgotten. Stops early when the settings change.
    fn pass(&self, folders: &[String], exts: &[&str], learn: &dyn Fn(&Path) -> Result<T, String>) {
        let generation = self.generation.load(Ordering::SeqCst);
        let stale = AtomicBool::new(false);
        self.set(|p| {
            p.working = true;
            p.error.clear();
        });
        let found = walk(folders, exts, &stale);
        let mut pending = Vec::new();
        {
            let mut items = self.items.write().unwrap_or_else(|e| e.into_inner());
            let mut seen = HashSet::new();
            for file in found {
                let Some(key) = file_key(&file.path) else { continue };
                seen.insert(key);
                match items.get_mut(&key) {
                    // The same file, perhaps moved or renamed: what was learned of it stays.
                    Some(item) if item.size == file.size && item.modified == file.modified => item.path = file.path.to_string_lossy().into_owned(),
                    _ => pending.push((key, file)),
                }
            }
            items.retain(|key, _| seen.contains(key));
        }
        let total = pending.len();
        self.set(|p| p.pending = total);
        let mut unsaved = 0;
        for (done, (key, file)) in pending.into_iter().enumerate() {
            if self.generation.load(Ordering::SeqCst) != generation {
                break;
            }
            let data = match learn(&file.path) {
                Ok(data) => Some(data),
                Err(e) if e == STOP => break,
                Err(_) => None,
            };
            let item = Item { serial: key.serial, id: format!("{:x}", key.id), path: file.path.to_string_lossy().into_owned(), size: file.size, modified: file.modified, data };
            self.items.write().unwrap_or_else(|e| e.into_inner()).insert(key, item);
            unsaved += 1;
            if unsaved >= 25 {
                self.save();
                unsaved = 0;
            }
            let known = self.items.read().unwrap_or_else(|e| e.into_inner()).values().filter(|i| i.data.is_some()).count();
            self.set(|p| {
                p.pending = total - done - 1;
                p.known = known;
            });
        }
        self.save();
        let known = self.items.read().unwrap_or_else(|e| e.into_inner()).values().filter(|i| i.data.is_some()).count();
        self.set(|p| {
            p.working = false;
            p.pending = 0;
            p.known = known;
        });
    }
}

/// What a model's `learn` says to end the pass (its runtime went away, say).
pub const STOP: &str = "stop";

/// How often a pass runs again on its own, for what's new in the folders.
const PERIOD: Duration = Duration::from_secs(2 * 60 * 60);

/// Starts both background passes (each waits until it's turned on and its model is there).
pub fn start() {
    spawn_worker("photo search", || {
        crate::photos::LIBRARY.load();
        loop {
            let config = config();
            if config.photos && crate::photos::installed() && !config.photo_folders.is_empty() {
                background();
                crate::photos::LIBRARY.pass(&config.photo_folders, crate::photos::EXTS, &crate::photos::learn);
                crate::photos::rest();
            }
            crate::photos::LIBRARY.sleep(PERIOD);
        }
    });
    spawn_worker("speech search", || {
        crate::speech::LIBRARY.load();
        loop {
            let config = config();
            if config.speech && crate::speech::installed() && !config.speech_folders.is_empty() {
                background();
                crate::speech::LIBRARY.pass(&config.speech_folders, crate::speech::EXTS, &crate::speech::learn);
            }
            crate::speech::LIBRARY.sleep(PERIOD);
        }
    });
}

fn spawn_worker(name: &str, run: fn()) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
        // Let Glint start first.
        std::thread::sleep(Duration::from_secs(20));
        run();
    });
}

/// This thread's CPU and disk use at background priority.
fn background() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

/// Settings → Photos and recordings: each side's switch, folders, model and progress.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub config: Config,
    pub photos_installed: bool,
    pub speech_installed: bool,
    pub photos: Progress,
    pub speech: Progress,
}

#[tauri::command]
pub fn media_ai_status() -> Status {
    Status { config: config(), photos_installed: crate::photos::installed(), speech_installed: crate::speech::installed(), photos: crate::photos::LIBRARY.progress(), speech: crate::speech::LIBRARY.progress() }
}

#[tauri::command]
pub fn media_ai_configure(config: Config) -> Result<Status, String> {
    let path = config_path().ok_or("Glint's data folder isn't available")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let before = self::config();
    fs::write(&path, serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if before.photos != config.photos || before.photo_folders != config.photo_folders {
        crate::photos::LIBRARY.poke();
    }
    if before.speech != config.speech || before.speech_folders != config.speech_folders {
        crate::speech::LIBRARY.poke();
    }
    Ok(media_ai_status())
}

/// Downloads what a side needs: "photos" (ONNX Runtime and CLIP) or "speech" (Whisper and its model).
#[tauri::command]
pub async fn media_ai_install(kind: String, on_progress: Channel<Download>, installs: State<'_, Installs>) -> Result<Status, String> {
    match kind.as_str() {
        "photos" => crate::photos::install(&on_progress, &installs).await?,
        "speech" => crate::speech::install(&on_progress, &installs).await?,
        _ => return Err(format!("Nothing is called {kind}")),
    }
    Ok(media_ai_status())
}

/// What a search asks of these: `shows:` (what's in photos) or `says:` (what's said), with the text
/// after it, to the end of the query.
#[derive(Debug, PartialEq)]
pub enum Ask {
    Shows(String),
    Says(String),
}

impl Ask {
    pub fn engine(&self) -> &'static str {
        match self {
            Ask::Shows(_) => "photos",
            Ask::Says(_) => "speech",
        }
    }
}

/// `shows:` or `says:` at the start of the query or after a space.
pub fn asked(query: &str) -> Option<Ask> {
    for (prefix, shows) in [("shows:", true), ("says:", false)] {
        let at = (0..query.len().saturating_sub(prefix.len() - 1)).find(|&i| {
            query.is_char_boundary(i) && (i == 0 || query[..i].ends_with(|c: char| c.is_whitespace() || c == '"')) && query.as_bytes()[i..].get(..prefix.len()).is_some_and(|b| b.eq_ignore_ascii_case(prefix.as_bytes()))
        });
        if let Some(at) = at {
            let text = query[at + prefix.len()..].trim().trim_matches('"').trim().to_string();
            return Some(if shows { Ask::Shows(text) } else { Ask::Says(text) });
        }
    }
    None
}

/// Whether `path` is below one of `roots` (all of them when there's none, as for This PC).
pub fn below(path: &str, roots: &[String]) -> bool {
    roots.is_empty()
        || roots.iter().any(|root| {
            let root = root.trim_end_matches(['\\', '/']);
            path.len() > root.len() && path[..root.len()].eq_ignore_ascii_case(root) && path[root.len()..].starts_with(['\\', '/']) || path.eq_ignore_ascii_case(root)
        })
}

/// A search's answer as the explorer lists results: the files, best first.
pub fn search(ask: &Ask, roots: &[String], max: usize, stop: &AtomicBool) -> Result<Vec<crate::fsx::Entry>, String> {
    let paths = match ask {
        Ask::Shows(text) if text.is_empty() => return Err("Say what the photos show, as in shows:dog on a beach".into()),
        Ask::Says(text) if text.is_empty() => return Err("Say the words, as in says:quarterly budget".into()),
        Ask::Shows(text) => crate::photos::search(text, roots, max)?,
        Ask::Says(text) => crate::speech::search(text, roots, max)?,
    };
    Ok(paths
        .into_iter()
        .take_while(|_| !stop.load(Ordering::Relaxed))
        .filter_map(|path| {
            let mut entry = crate::fsx::stat(&path)?;
            entry.name = Path::new(&path).file_name()?.to_string_lossy().into_owned();
            entry.path = Some(path);
            Some(entry)
        })
        .collect())
}

/// A model's files from Hugging Face, each checked against its pinned SHA-256, into a fresh folder that
/// then replaces `dir`.
pub async fn fetch_model(dir: &Path, files: &[(&str, &str, &str)], id: &str, name: &str, progress: &Channel<Download>, installs: &Installs) -> Result<(), String> {
    let fresh = dir.with_extension("new");
    let _ = fs::remove_dir_all(&fresh);
    fs::create_dir_all(&fresh).map_err(|e| e.to_string())?;
    let stop = installs.begin(id);
    let mut result = Ok(());
    for (url, sha256, save_as) in files {
        match tools::fetch(url, sha256, &format!("{id}-{save_as}"), name, &stop, progress).await {
            Ok(file) => {
                let moved = fs::rename(&file, fresh.join(save_as)).or_else(|_| fs::copy(&file, fresh.join(save_as)).map(drop));
                let _ = fs::remove_file(&file);
                if let Err(e) = moved {
                    result = Err(e.to_string());
                    break;
                }
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    installs.end(id);
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&fresh);
        return Err(e);
    }
    let _ = fs::remove_dir_all(dir);
    fs::rename(&fresh, dir).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_a_search_asks() {
        assert_eq!(asked("shows:dog on a beach"), Some(Ask::Shows("dog on a beach".into())));
        assert_eq!(asked("ext:jpg  SHOWS: \"red car\""), Some(Ask::Shows("red car".into())));
        assert_eq!(asked("says:quarterly budget"), Some(Ask::Says("quarterly budget".into())));
        assert_eq!(asked("slideshows:x"), None, "only at the start of a word");
        assert_eq!(asked("report"), None);
        assert_eq!(asked("shows:"), Some(Ask::Shows(String::new())));
        assert_eq!(asked("\"shows:red car\""), Some(Ask::Shows("red car".into())), "as the search box quotes it");
    }

    #[test]
    fn knows_what_is_below_a_folder() {
        let roots = vec![r"C:\Users\me\Pictures".to_string()];
        assert!(below(r"C:\Users\me\Pictures\a.jpg", &roots));
        assert!(below(r"c:\users\me\pictures\trip\b.jpg", &roots));
        assert!(!below(r"C:\Users\me\Pictures2\c.jpg", &roots));
        assert!(below(r"D:\x.jpg", &[]));
        assert!(below(r"C:\a.jpg", &[r"C:\".to_string()]));
    }

    #[test]
    fn keeps_what_it_learned_under_the_file_id() {
        let dir = std::env::temp_dir().join(format!("glint-media-ai-{}", std::process::id()));
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("a.jpg"), "a").unwrap();
        fs::write(dir.join("sub").join("b.png"), "b").unwrap();
        fs::write(dir.join("c.txt"), "c").unwrap();
        let stop = AtomicBool::new(false);
        let mut found: Vec<String> = walk(&[dir.to_string_lossy().into_owned()], &["jpg", "png"], &stop).into_iter().map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        found.sort();
        assert_eq!(found, ["a.jpg", "b.png"]);
        let before = file_key(&dir.join("a.jpg")).unwrap();
        fs::rename(dir.join("a.jpg"), dir.join("sub").join("moved.jpg")).unwrap();
        assert_eq!(file_key(&dir.join("sub").join("moved.jpg")), Some(before), "a move keeps the file's ID");
        let _ = fs::remove_dir_all(&dir);
    }
}
