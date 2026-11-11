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
pub fn startup_hint(paths: Vec<String>) {
    let Some(file) = hint_file() else { return };
    let paths: Vec<String> = paths.into_iter().filter(|p| !p.is_empty()).take(MOST).collect();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(file, serde_json::to_vec(&paths).unwrap_or_default());
}

/// The page painted its first folder: with GLINT_BENCH_START set, the time since Windows made the process is
/// written there, with every mark along the way on the lines after it, and Glint quits (the bench harness's
/// cold start).
pub fn bench_first_folder(app: crate::host::AppHandle) {
    mark("first folder painted");
    let Some(file) = std::env::var_os("GLINT_BENCH_START") else { return };
    let (total, marks) = marks();
    let mut text = format!("{total:.1}\n");
    for (name, at) in marks {
        text.push_str(&format!("{at:9.1}  {name}\n"));
    }
    let _ = std::fs::write(file, text);
    app.exit(0);
}

/// The steps of the start so far, each with the moment it was reached.
static MARKS: Mutex<Vec<(&'static str, Instant)>> = Mutex::new(Vec::new());

/// Notes that the start has reached `name`: where a slow start spends its time is read from these
/// (`GLINT_BENCH_START`). A handful of calls, each a clock read and a push.
pub fn mark(name: &'static str) {
    MARKS.lock().unwrap_or_else(|e| e.into_inner()).push((name, Instant::now()));
}

/// How long ago Windows made this process, in ms, and each mark's moment counted from then: the time before
/// `main` runs (the loader mapping the program and its libraries) is part of what a person waits for.
pub fn marks() -> (f64, Vec<(&'static str, f64)>) {
    let now = Instant::now();
    let age = process_age_ms().unwrap_or_else(|| STARTED.get().map_or(0.0, |s| ms_between(*s, now)));
    let marks = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    (age, marks.iter().map(|(name, at)| (*name, age - ms_between(*at, now))).collect())
}

fn ms_between(from: Instant, to: Instant) -> f64 {
    to.saturating_duration_since(from).as_secs_f64() * 1000.0
}

/// Milliseconds since Windows made this process (its creation time against the clock now).
#[cfg(windows)]
fn process_age_ms() -> Option<f64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    let (mut made, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // Safety: the out-parameters live through the call; the pseudo-handle needs no closing.
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut made, &mut exited, &mut kernel, &mut user) }.ok()?;
    let now = unsafe { GetSystemTimePreciseAsFileTime() };
    // FILETIME counts 100 ns.
    Some(ticks(now).saturating_sub(ticks(made)) as f64 / 10_000.0)
}

#[cfg(not(windows))]
fn process_age_ms() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_counted_from_when_the_process_was_made_and_keep_their_order() {
        mark("test: one");
        std::thread::sleep(std::time::Duration::from_millis(3));
        mark("test: two");
        let (age, marks) = marks();
        let at = |name: &str| marks.iter().find(|(marked, _)| *marked == name).map(|(_, at)| *at).expect("it was marked");
        // The process is older than its marks, and a later mark is later.
        assert!(age > 0.0);
        assert!(at("test: one") >= 0.0, "{}", at("test: one"));
        assert!(at("test: one") < at("test: two"));
        assert!(at("test: two") <= age);
    }
}
