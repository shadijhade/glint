//! A quicker start: the folders the last session showed are read in Rust the moment Glint starts, while
//! WebView2's own processes are still starting, so the first listing the page asks for is waiting for it
//! (used only if the folder hasn't changed since). The page says which folders those are as its tabs
//! change (`startup_hint`).
//!
//! `npm run bench -- --start` measures it: with GLINT_BENCH_START naming a file, the page reports its
//! first folder painted, and Glint writes the time since it started there and quits.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::Instant;

static STARTED: OnceLock<Instant> = OnceLock::new();
/// Listings read at start, by folder (lowercase): the bytes the page takes, and the folder's last-write
/// time when they were read.
static WARM: LazyLock<Mutex<HashMap<String, Warm>>> = LazyLock::new(Default::default);
/// A listing read ahead, and its folder's stamp then.
type Warm = (Vec<u8>, f64);
/// The most folders read ahead at start.
const MOST: usize = 4;

fn hint_file() -> Option<PathBuf> {
    crate::tools::local_data_dir().map(|d| d.join("startup.json"))
}

/// Notes when Glint started, and reads the last session's folders on a thread of their own.
pub fn begin() {
    let _ = STARTED.set(Instant::now());
    let Some(file) = hint_file() else { return };
    std::thread::spawn(move || {
        let Ok(bytes) = std::fs::read(file) else { return };
        let Ok(paths) = serde_json::from_slice::<Vec<String>>(&bytes) else { return };
        for path in paths.into_iter().take(MOST) {
            if let Some(warm) = read(&path) {
                WARM.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_lowercase(), warm);
            }
        }
    });
}

/// A plain folder's listing as the page takes it, and its stamp; None for anything that isn't one
/// (archives, remotes, the Recycle Bin…: those are read when asked).
fn read(path: &str) -> Option<Warm> {
    if crate::remote::is_remote(path) || crate::recycle::is_place(path) || crate::recent::is_place(path) || crate::gallery::is_place(path) {
        return None;
    }
    let stamp = crate::fsx::folder_stamp(path.to_string()).ok()?;
    let started = Instant::now();
    let mut entries = crate::fsx::read_dir(path).ok()?;
    crate::fsx::sort_entries(&mut entries);
    Some((crate::fsx::encode_listing(path, &entries, crate::fsx::ms_since(started), None, false), stamp))
}

/// The listing read at start for `path`, if it's still as the folder is (taken: it's used once).
pub fn take(path: &str) -> Option<Vec<u8>> {
    let (bytes, stamp) = WARM.lock().unwrap_or_else(|e| e.into_inner()).remove(&path.to_lowercase())?;
    let now = crate::fsx::folder_stamp(path.to_string()).ok()?;
    (now == stamp).then_some(bytes)
}

/// The folders shown now (the panes' tabs), for the next start to read ahead.
#[tauri::command]
pub fn startup_hint(paths: Vec<String>) {
    let Some(file) = hint_file() else { return };
    let paths: Vec<String> = paths.into_iter().filter(|p| !p.is_empty()).take(MOST).collect();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, serde_json::to_vec(&paths).unwrap_or_default());
}

/// The page painted its first folder: with GLINT_BENCH_START set, the time since start is written there
/// and Glint quits (the bench harness's cold start).
#[tauri::command]
pub fn bench_first_folder(app: tauri::AppHandle) {
    let Some(file) = std::env::var_os("GLINT_BENCH_START") else { return };
    let ms = STARTED.get().map_or(0.0, |s| s.elapsed().as_secs_f64() * 1000.0);
    let _ = std::fs::write(file, format!("{ms:.1}"));
    app.exit(0);
}
