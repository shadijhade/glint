//! More windows: "Open in new window" and a tab dragged out of its strip (port of src-tauri/src/more_windows.rs
//! and the `SECONDARY_WINDOW` parts of src/lib/store.ts, src/lib/explorer.ts and src/App.tsx).
//!
//! **A second window is a second `Glint.exe` process.** The explorer is one global entity per process (the
//! views look it up everywhere), so a window that has folders and tabs of its own is a process of its own, in
//! *secondary mode*, started with
//!
//! ```text
//! Glint.exe --window <folder> [--at <x>,<y>] [--size <w>x<h>] [--main <pid>]
//! ```
//!
//! - `--window <folder>`: the folder it opens on, in its one tab. Starts secondary mode.
//! - `--at <x>,<y>`: where its top-left corner goes, in screen pixels (where a tab was dropped). Without it the
//!   window is centred. A place no monitor shows is moved onto one.
//! - `--size <w>x<h>`: its size in pixels (the opener's 85%). Without it the window keeps the size it opens with.
//! - `--main <pid>`: the main Glint process. When it exits (Quit, not a hidden window) this window closes too.
//!
//! Anything else on such a command line is refused ([`parse`]), and so is `--at`, `--size` or `--main` without
//! `--window`.
//!
//! **What a secondary window keeps to itself: nothing.** The main window keeps what is saved. A secondary:
//!
//! - doesn't take the single-instance name or hear later launches (File Explorer's opens still go to the main
//!   instance), and has no tray, no hotkey, no Start with Windows, no updater, no service-update notice and no
//!   folder rules (the main process runs them; two copies would act twice);
//! - doesn't restore or save the session, the window's place, the Jump List, the taskbar's buttons, history,
//!   visits or any other memory, and never writes `state.json`;
//! - reads the settings (and the pinned folders, saved searches… in the same file) at start and follows the
//!   main window's changes: [`watch_file`] tells when `state.json` changes and `persistence` reloads it;
//! - sends Settings to the main window: [`open_main_settings`] asks it over the single-instance listener
//!   (`launch::ask_main_for_settings`), and the main window comes forward with Settings open.
//!
//! The gates are small checks of [`is_secondary`] in the places that write or start something. The tests set
//! secondary mode for their own thread ([`as_secondary`]), so the process-wide switch is never touched.

use crate::kit::prelude::*;
use crate::t;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

// ---- The command line ---------------------------------------------------------------------------------------

/// The folder the window opens on (and starts secondary mode).
pub const WINDOW: &str = "--window";
/// Where the window's top-left corner goes: `x,y` in screen pixels.
pub const AT: &str = "--at";
/// The window's size: `WxH` in pixels.
pub const SIZE: &str = "--size";
/// The main Glint process's id.
pub const MAIN: &str = "--main";

/// The biggest size accepted: far past any screen, small enough to keep every sum in range.
const MAX_SIZE: i32 = 32_768;

/// A secondary window as the command line describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecondaryWindow {
    /// The folder it opens on.
    pub folder: String,
    /// Its top-left corner on the screen, when it was given.
    pub at: Option<(i32, i32)>,
    /// Its size in pixels, when it was given.
    pub size: Option<(i32, i32)>,
    /// The main Glint process, whose end closes this window.
    pub main: Option<u32>,
}

/// Why a command line isn't a secondary window's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgError {
    /// An option with no value after it.
    MissingValue(&'static str),
    /// An option whose value isn't one.
    BadValue { option: &'static str, value: String },
    /// An option given twice.
    Twice(&'static str),
    /// `--at`, `--size` or `--main` without `--window`.
    NeedsWindow(&'static str),
    /// Something a secondary window doesn't take.
    Unexpected(String),
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArgError::MissingValue(option) => write!(f, "{option} needs a value"),
            ArgError::BadValue { option, value } => write!(f, "{option} can't take {value:?}"),
            ArgError::Twice(option) => write!(f, "{option} was given twice"),
            ArgError::NeedsWindow(option) => write!(f, "{option} only goes with {WINDOW} <folder>"),
            ArgError::Unexpected(arg) => write!(f, "a window opened with {WINDOW} takes no {arg:?}"),
        }
    }
}

/// `--name=value` as (`--name`, Some(`value`)), `--name` as (`--name`, None).
fn split_option(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((name, value)) if name.starts_with("--") => (name, Some(value)),
        _ => (arg, None),
    }
}

fn pair(text: &str, separators: &[char]) -> Option<(i32, i32)> {
    let (a, b) = text.split_once(separators)?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Reads a command line (without the program's own path). `Ok(None)` is a plain Glint start (no `--window`);
/// `Ok(Some(..))` a secondary window; an error says what is wrong, and the window isn't opened.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<SecondaryWindow>, ArgError> {
    let args: Vec<String> = args.into_iter().collect();
    let mut folder: Option<String> = None;
    let mut at = None;
    let mut size = None;
    let mut main = None;
    let mut needs_window: Option<&'static str> = None;
    let mut others: Vec<&str> = Vec::new();
    let mut iter = args.iter().map(String::as_str);
    while let Some(arg) = iter.next() {
        let (name, inline) = split_option(arg);
        let option = [WINDOW, AT, SIZE, MAIN]
            .into_iter()
            .find(|option| name.eq_ignore_ascii_case(option));
        let Some(option) = option else {
            others.push(arg);
            continue;
        };
        let value = match inline {
            Some(value) => value,
            // The next argument is the value, unless it is the next option.
            None => iter
                .next()
                .filter(|next| !next.starts_with("--"))
                .ok_or(ArgError::MissingValue(option))?,
        };
        let bad = || ArgError::BadValue {
            option,
            value: value.to_string(),
        };
        let twice = || ArgError::Twice(option);
        match option {
            WINDOW => {
                let value = value.trim();
                if value.is_empty() {
                    return Err(bad());
                }
                if folder.replace(value.to_string()).is_some() {
                    return Err(twice());
                }
            }
            AT => {
                let place = pair(value, &[',']).ok_or_else(bad)?;
                if at.replace(place).is_some() {
                    return Err(twice());
                }
                needs_window.get_or_insert(AT);
            }
            SIZE => {
                let (w, h) = pair(value, &['x', 'X']).ok_or_else(bad)?;
                if !(1..=MAX_SIZE).contains(&w) || !(1..=MAX_SIZE).contains(&h) {
                    return Err(bad());
                }
                if size.replace((w, h)).is_some() {
                    return Err(twice());
                }
                needs_window.get_or_insert(SIZE);
            }
            _ => {
                let pid = value
                    .trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|pid| *pid > 0)
                    .ok_or_else(bad)?;
                if main.replace(pid).is_some() {
                    return Err(twice());
                }
                needs_window.get_or_insert(MAIN);
            }
        }
    }
    let Some(folder) = folder else {
        return match needs_window {
            Some(option) => Err(ArgError::NeedsWindow(option)),
            None => Ok(None),
        };
    };
    if let Some(other) = others.first() {
        return Err(ArgError::Unexpected((*other).to_string()));
    }
    Ok(Some(SecondaryWindow { folder, at, size, main }))
}

/// The arguments (without the program's path) that start `window`; [`parse`] reads them back.
pub fn command_line(window: &SecondaryWindow) -> Vec<String> {
    let mut args = vec![WINDOW.to_string(), window.folder.clone()];
    if let Some((x, y)) = window.at {
        args.extend([AT.to_string(), format!("{x},{y}")]);
    }
    if let Some((w, h)) = window.size {
        args.extend([SIZE.to_string(), format!("{w}x{h}")]);
    }
    if let Some(pid) = window.main {
        args.extend([MAIN.to_string(), pid.to_string()]);
    }
    args
}

// ---- Secondary mode ------------------------------------------------------------------------------------------

/// This process's window, once [`enter`] made it secondary.
static MODE: OnceLock<SecondaryWindow> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// A test's own secondary mode, for its thread only ([`as_secondary`]).
    static TEST_MODE: std::cell::RefCell<Option<SecondaryWindow>> = const { std::cell::RefCell::new(None) };
}

/// Makes this process a secondary window (once, at start: `launch::begin` does). False if it already was one.
pub fn enter(window: SecondaryWindow) -> bool {
    MODE.set(window).is_ok()
}

/// This process's secondary window, or None in the main Glint.
pub fn current() -> Option<SecondaryWindow> {
    #[cfg(test)]
    if let Some(test) = TEST_MODE.with(|mode| mode.borrow().clone()) {
        return Some(test);
    }
    MODE.get().cloned()
}

/// Whether this process is a secondary window: the writers, the updater, the tray and the rest check it.
pub fn is_secondary() -> bool {
    #[cfg(test)]
    if TEST_MODE.with(|mode| mode.borrow().is_some()) {
        return true;
    }
    MODE.get().is_some()
}

/// The main Glint's process id: this process's own, or the one a secondary window was started with.
pub fn main_pid() -> u32 {
    current()
        .and_then(|window| window.main)
        .unwrap_or_else(std::process::id)
}

/// Secondary mode for the calling thread until the guard drops (tests; the process-wide switch is left alone).
#[cfg(test)]
pub(crate) fn as_secondary(window: SecondaryWindow) -> SecondaryGuard {
    TEST_MODE.with(|mode| *mode.borrow_mut() = Some(window));
    SecondaryGuard
}

/// A plain secondary window on `folder`, for tests.
#[cfg(test)]
pub(crate) fn window_on(folder: &str) -> SecondaryWindow {
    SecondaryWindow {
        folder: folder.to_string(),
        at: None,
        size: None,
        main: None,
    }
}

/// Ends [`as_secondary`].
#[cfg(test)]
pub(crate) struct SecondaryGuard;

#[cfg(test)]
impl Drop for SecondaryGuard {
    fn drop(&mut self) {
        TEST_MODE.with(|mode| *mode.borrow_mut() = None);
    }
}

// ---- Starting a window ----------------------------------------------------------------------------------------

/// A process to start: Glint.exe and the arguments of a secondary window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawn {
    pub exe: PathBuf,
    pub args: Vec<String>,
}

/// How a process is started: [`spawn_process`] in the app, a recorder in tests. Called off the UI thread.
pub type Spawner = Arc<dyn Fn(&Spawn) -> Result<(), String> + Send + Sync>;

struct Spawners(Spawner);

impl Global for Spawners {}

/// Makes `spawner` start the windows (tests).
#[cfg(test)]
pub fn use_spawner(cx: &mut App, spawner: Spawner) {
    cx.set_global(Spawners(spawner));
}

/// Starts the process and lets it go: the new window outlives nothing of this one.
pub fn spawn_process(spawn: &Spawn) -> Result<(), String> {
    std::process::Command::new(&spawn.exe)
        .args(&spawn.args)
        .spawn()
        .map(|_child| ())
        .map_err(|e| e.to_string())
}

fn spawner(cx: &App) -> Spawner {
    cx.try_global::<Spawners>()
        .map(|s| s.0.clone())
        .unwrap_or_else(|| Arc::new(spawn_process))
}

/// How much of the opener's size a new window gets (`0.85`), and the least it gets (`min_inner_size`).
const SIZE_SHARE: f32 = 0.85;
const MIN_SIZE: (f32, f32) = (500., 360.);
/// Where in a dropped tab's window the pointer sits: the window's corner is this far up and left of the drop.
const GRAB: (f32, f32) = (120., 20.);

/// The new window's size in pixels: 85% of the opener's (`logical` size in logical pixels, at `scale`), but at
/// least 500 x 360 logical.
pub fn new_window_size(logical: (f32, f32), scale: f32) -> (i32, i32) {
    let scale = if scale.is_finite() && scale > 0. { scale } else { 1. };
    let one = |size: f32, least: f32| {
        let size = if size.is_finite() { size } else { least };
        let pixels = (size * scale * SIZE_SHARE).max(least * scale).round();
        // Within 1..=MAX_SIZE, so the cast cannot truncate.
        pixels.clamp(1., MAX_SIZE as f32) as i32
    };
    (one(logical.0, MIN_SIZE.0), one(logical.1, MIN_SIZE.1))
}

/// The window's corner for a tab dropped at `drop` (screen pixels): up and left of it, so the pointer is on the
/// new window's title bar (`position((x - 120).max(0), (y - 20).max(0))`; here a place left of or above the
/// primary monitor stays, since the monitors can be there).
pub fn corner_for_drop(drop: (i32, i32), scale: f32) -> (i32, i32) {
    let scale = if scale.is_finite() && scale > 0. { scale } else { 1. };
    // A few hundred pixels at most.
    let grab = |v: f32| (v * scale).round() as i32;
    (drop.0.saturating_sub(grab(GRAB.0)), drop.1.saturating_sub(grab(GRAB.1)))
}

/// A point in `window` (logical pixels) as screen pixels.
pub fn screen_point(window: &Window, point: Point<Pixels>) -> (i32, i32) {
    let scale = window.scale_factor();
    let origin = window.bounds().origin;
    let at = |v: Pixels| (f32::from(v) * scale).round();
    // Screen coordinates, far inside i32.
    ((at(origin.x) + at(point.x)) as i32, (at(origin.y) + at(point.y)) as i32)
}

/// What starts a window on `folder` from `window`: its 85% size, the corner for a drop at `drop` (centred
/// without one) and the main process to follow.
fn window_for(window: &Window, folder: &str, drop: Option<(i32, i32)>) -> SecondaryWindow {
    let scale = window.scale_factor();
    let bounds = window.bounds().size;
    SecondaryWindow {
        folder: folder.to_string(),
        at: drop.map(|drop| corner_for_drop(drop, scale)),
        size: Some(new_window_size(
            (f32::from(bounds.width), f32::from(bounds.height)),
            scale,
        )),
        main: Some(main_pid()),
    }
}

/// Starts Glint.exe in secondary mode on `folder` (`openInNewWindow`): at screen point `drop` when a tab was
/// dropped there, else centred. The process starts off the UI thread; `then` runs once it has started. A failure
/// says so in `window` (the web app's "Couldn't open a new window" toast) and `then` doesn't run.
pub fn open_window(
    window: &mut Window,
    cx: &mut App,
    folder: &str,
    drop: Option<(i32, i32)>,
    then: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let args = command_line(&window_for(window, folder, drop));
    let spawner = spawner(cx);
    let started = cx.background_spawn(async move {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        spawner(&Spawn { exe, args })
    });
    window
        .spawn(cx, async move |cx| match started.await {
            Ok(()) => {
                let _ = cx.update(then);
            }
            Err(reason) => {
                let _ = cx.update(|window, cx| {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't open a new window")), cx)
                });
            }
        })
        .detach();
}

// ---- Settings are the main window's ---------------------------------------------------------------------------

/// How the main instance is asked to open Settings (true once it took the request): the single-instance listener
/// in the app, a fake in tests. Called off the UI thread.
pub type MainAsk = Arc<dyn Fn(Option<String>) -> bool + Send + Sync>;

struct MainAsks(MainAsk);

impl Global for MainAsks {}

/// Makes `ask` carry the Settings request to the main instance (tests).
#[cfg(test)]
pub fn use_main_ask(cx: &mut App, ask: MainAsk) {
    cx.set_global(MainAsks(ask));
}

/// Settings from a secondary window (`open_main_settings`): the main window opens them, at `section` when given,
/// and comes to the front. Says so in `window` when the main instance doesn't answer.
pub fn open_main_settings(window: &mut Window, cx: &mut App, section: Option<&str>) {
    let ask: MainAsk = cx.try_global::<MainAsks>().map(|a| a.0.clone()).unwrap_or_else(|| {
        Arc::new(|section: Option<String>| crate::platform::launch::ask_main_for_settings(section.as_deref()))
    });
    let section = section.map(str::to_string);
    let asked = cx.background_spawn(async move { ask(section) });
    window
        .spawn(cx, async move |cx| {
            if !asked.await {
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        Notification::error(t!(
                            "Settings are kept by the main Glint window, and it isn't answering."
                        ))
                        .title(t!("Couldn't open Settings")),
                        cx,
                    )
                });
            }
        })
        .detach();
}

// ---- Watching a file ------------------------------------------------------------------------------------------

/// A running [`watch_file`]; dropping it ends the watching.
pub struct FileWatch {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FileWatch {
    /// Ends the watching and waits for its thread (a fraction of a second).
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for FileWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// What tells that a file changed: its size and when it was written (None: it isn't there).
fn signature(file: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    let meta = std::fs::metadata(file).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// How long the watching thread waits before looking at its stop flag again.
const WATCH_TICK_MS: u32 = 300;
/// A write is several events (create, write, rename); `changed` is called once they have settled.
const SETTLE: Duration = Duration::from_millis(80);

/// Calls `changed` (on the watching thread) whenever `file` is written, replaced or removed, with Windows' own
/// change notification on its folder (the file is replaced by renaming over it, so the folder is what is
/// watched). `None` when Windows won't watch the folder. The watching has begun when this returns.
pub fn watch_file(file: &Path, changed: impl Fn() + Send + 'static) -> Option<FileWatch> {
    let folder = file.parent()?.to_path_buf();
    let file = file.to_path_buf();
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_in, ready) = std::sync::mpsc::channel();
    let flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("glint-follow".into())
        .spawn(move || watch_folder(&folder, &file, &flag, &changed, ready_in))
        .ok()?;
    match ready.recv_timeout(Duration::from_secs(5)) {
        Ok(true) => Some(FileWatch {
            stop,
            thread: Some(thread),
        }),
        _ => {
            stop.store(true, Ordering::SeqCst);
            None
        }
    }
}

fn watch_folder(
    folder: &Path,
    file: &Path,
    stop: &AtomicBool,
    changed: &dyn Fn(),
    ready: std::sync::mpsc::Sender<bool>,
) {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::Storage::FileSystem::{
        FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    };
    use windows::Win32::System::Threading::WaitForSingleObject;

    let path = HSTRING::from(folder.to_string_lossy().as_ref());
    // SAFETY: a plain change-notification request for a folder name that outlives the call; the handle is used
    // only on this thread and closed below.
    let handle = unsafe {
        FindFirstChangeNotificationW(
            &path,
            false,
            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE,
        )
    };
    let Ok(handle) = handle else {
        let _ = ready.send(false);
        return;
    };
    let mut seen = signature(file);
    let _ = ready.send(true);
    while !stop.load(Ordering::SeqCst) {
        // SAFETY: `handle` is the change handle made above, still open.
        let woke = unsafe { WaitForSingleObject(handle, WATCH_TICK_MS) };
        if woke == WAIT_OBJECT_0 {
            // SAFETY: re-arms the same open handle.
            let _ = unsafe { FindNextChangeNotification(handle) };
            std::thread::sleep(SETTLE);
            let now = signature(file);
            if now != seen {
                seen = now;
                changed();
            }
        } else if woke != WAIT_TIMEOUT {
            break;
        }
    }
    // SAFETY: closes the handle made above, once.
    let _ = unsafe { FindCloseChangeNotification(handle) };
}

// ---- Watching the main process --------------------------------------------------------------------------------

/// Why a process can't be watched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchError {
    /// There is no such process (it has already ended).
    Gone,
    /// Windows wouldn't let it be watched.
    Other(String),
}

/// A running [`watch_process`]; dropping it ends the watching without calling anything.
pub struct ProcessWatch {
    stop: Arc<AtomicBool>,
}

impl Drop for ProcessWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Calls `gone` (on the watching thread) when process `pid` ends. Fails with [`WatchError::Gone`] when there is
/// no such process now.
pub fn watch_process(pid: u32, gone: impl FnOnce() + Send + 'static) -> Result<ProcessWatch, WatchError> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

    // SAFETY: opens a handle that can only be waited on; closed by the thread below.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.map_err(|e| {
        if e.code() == ERROR_INVALID_PARAMETER.to_hresult() {
            WatchError::Gone
        } else {
            WatchError::Other(e.message())
        }
    })?;
    let raw = handle.0 as isize;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("glint-main-watch".into())
        .spawn(move || {
            let handle = windows::Win32::Foundation::HANDLE(raw as *mut _);
            let mut ended = false;
            while !flag.load(Ordering::SeqCst) {
                // SAFETY: `handle` is the process handle opened above, still open.
                let woke = unsafe { WaitForSingleObject(handle, WATCH_TICK_MS) };
                if woke == WAIT_OBJECT_0 {
                    ended = true;
                    break;
                } else if woke != WAIT_TIMEOUT {
                    break;
                }
            }
            // SAFETY: closes the handle opened above, once.
            let _ = unsafe { CloseHandle(handle) };
            if ended {
                gone();
            }
        });
    if let Err(e) = spawned {
        // SAFETY: the thread never started, so the handle is still ours to close.
        let _ = unsafe { CloseHandle(handle) };
        return Err(WatchError::Other(e.to_string()));
    }
    Ok(ProcessWatch { stop })
}

/// Keeps the main process's watch, and the one strong end of its channel, for as long as the app runs. The watching
/// thread only holds a weak end, so the channel is closed (and the task waiting on it woken) by the thread that
/// owns the app, never by the watching thread as it ends.
struct MainWatch {
    _watch: Option<ProcessWatch>,
    _ended: Arc<async_channel::Sender<()>>,
}

impl Global for MainWatch {}

/// Closes this window (`quit`) when the main Glint process ends, and at once when it already has. Does nothing
/// without a `--main` pid. A process that can't be watched leaves the window open.
pub fn follow_main_process(cx: &mut App, quit: impl Fn(&mut App) + 'static) {
    let Some(pid) = current().and_then(|window| window.main) else {
        return;
    };
    follow_process(cx, pid, quit);
}

/// [`follow_main_process`] for process `pid`.
pub fn follow_process(cx: &mut App, pid: u32, quit: impl Fn(&mut App) + 'static) {
    let (ended_in, ended) = async_channel::bounded::<()>(1);
    let ended_in = Arc::new(ended_in);
    let weak = Arc::downgrade(&ended_in);
    let watch = match watch_process(pid, move || {
        if let Some(ended_in) = weak.upgrade() {
            let _ = ended_in.try_send(());
        }
    }) {
        Ok(watch) => Some(watch),
        Err(WatchError::Gone) => {
            let _ = ended_in.try_send(());
            None
        }
        Err(WatchError::Other(reason)) => {
            eprintln!("glint: can't tell when the main Glint ends ({reason}); this window stays open");
            return;
        }
    };
    cx.set_global(MainWatch {
        _watch: watch,
        _ended: ended_in,
    });
    cx.spawn(async move |cx| {
        if ended.recv().await.is_ok() {
            cx.update(|cx| quit(cx));
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn a_plain_start_is_not_a_window() {
        assert_eq!(parse(args(&[])), Ok(None));
        assert_eq!(parse(args(&["C:\\Work", "--tray"])), Ok(None));
    }

    #[test]
    fn the_whole_command_line_is_read_and_written_back_the_same() {
        let window = SecondaryWindow {
            folder: "C:\\Users\\Me\\My Files".into(),
            at: Some((-120, 40)),
            size: Some((1054, 663)),
            main: Some(4242),
        };
        let line = command_line(&window);
        assert_eq!(
            line,
            args(&[
                "--window",
                "C:\\Users\\Me\\My Files",
                "--at",
                "-120,40",
                "--size",
                "1054x663",
                "--main",
                "4242"
            ])
        );
        assert_eq!(parse(line), Ok(Some(window)));
    }

    #[test]
    fn only_the_folder_is_needed_and_the_equals_form_works() {
        let only = parse(args(&["--window", "D:\\"])).expect("parses").expect("a window");
        assert_eq!(only, window_on("D:\\"));
        assert_eq!(command_line(&only), args(&["--window", "D:\\"]));
        let equals = parse(args(&["--WINDOW=D:\\Data", "--at=10,20", "--main=7"]))
            .expect("parses")
            .expect("a window");
        assert_eq!(equals.folder, "D:\\Data");
        assert_eq!(equals.at, Some((10, 20)));
        assert_eq!(equals.main, Some(7));
        // An Arabic folder name survives the round trip.
        let arabic = SecondaryWindow {
            folder: "C:\\\u{645}\u{631}\u{62d}\u{628}\u{627}".into(),
            ..window_on("")
        };
        assert_eq!(parse(command_line(&arabic)), Ok(Some(arabic)));
    }

    #[test]
    fn bad_command_lines_are_refused() {
        let refused = |list: &[&str]| parse(args(list)).expect_err(&list.join(" "));
        assert_eq!(refused(&["--window"]), ArgError::MissingValue(WINDOW));
        assert_eq!(refused(&["--window", "--at", "1,2"]), ArgError::MissingValue(WINDOW));
        assert_eq!(
            refused(&["--window", " "]),
            ArgError::BadValue {
                option: WINDOW,
                value: " ".into()
            }
        );
        assert!(matches!(
            refused(&["--window", "C:\\", "--at", "12"]),
            ArgError::BadValue { option: AT, .. }
        ));
        assert!(matches!(
            refused(&["--window", "C:\\", "--at", "a,b"]),
            ArgError::BadValue { option: AT, .. }
        ));
        assert!(matches!(
            refused(&["--window", "C:\\", "--size", "0x10"]),
            ArgError::BadValue { option: SIZE, .. }
        ));
        assert!(matches!(
            refused(&["--window", "C:\\", "--size", "10x-3"]),
            ArgError::BadValue { option: SIZE, .. }
        ));
        assert!(matches!(
            refused(&["--window", "C:\\", "--size", "99999x10"]),
            ArgError::BadValue { option: SIZE, .. }
        ));
        assert!(matches!(
            refused(&["--window", "C:\\", "--main", "0"]),
            ArgError::BadValue { option: MAIN, .. }
        ));
        assert!(matches!(
            refused(&["--window", "C:\\", "--main", "x"]),
            ArgError::BadValue { option: MAIN, .. }
        ));
        assert_eq!(
            refused(&["--window", "C:\\", "--window", "D:\\"]),
            ArgError::Twice(WINDOW)
        );
        assert_eq!(
            refused(&["--window", "C:\\", "--main", "1", "--main", "2"]),
            ArgError::Twice(MAIN)
        );
        // Options that only mean something for a window.
        assert_eq!(refused(&["--at", "1,2"]), ArgError::NeedsWindow(AT));
        assert_eq!(refused(&["--main", "5"]), ArgError::NeedsWindow(MAIN));
        assert_eq!(refused(&["--size", "9x9"]), ArgError::NeedsWindow(SIZE));
        // Nothing else rides along: not a second folder, not the other launch flags.
        assert_eq!(
            refused(&["--window", "C:\\", "D:\\"]),
            ArgError::Unexpected("D:\\".into())
        );
        assert_eq!(
            refused(&["--window", "C:\\", "--tray"]),
            ArgError::Unexpected("--tray".into())
        );
        assert!(!refused(&["--window", "C:\\", "--bogus"]).to_string().is_empty());
    }

    #[test]
    fn the_new_window_is_85_percent_of_the_opener_but_never_below_the_least() {
        assert_eq!(new_window_size((1240., 780.), 1.), (1054, 663));
        // At 150%: physical pixels.
        assert_eq!(new_window_size((1240., 780.), 1.5), (1581, 995));
        // A small opener still gives a usable window: 500 x 360 logical.
        assert_eq!(new_window_size((520., 360.), 1.), (500, 360));
        assert_eq!(new_window_size((520., 360.), 2.), (1000, 720));
        // Nonsense in, something usable out.
        assert_eq!(new_window_size((f32::NAN, f32::INFINITY), 0.), (500, 360));
    }

    #[test]
    fn a_dropped_tabs_window_has_its_corner_up_and_left_of_the_drop() {
        assert_eq!(corner_for_drop((1000, 500), 1.), (880, 480));
        assert_eq!(corner_for_drop((1000, 500), 2.), (760, 460));
        // A monitor to the left of the primary one has negative places, which stay.
        assert_eq!(corner_for_drop((-1500, 10), 1.), (-1620, -10));
        assert_eq!(corner_for_drop((i32::MIN, i32::MIN), 1.), (i32::MIN, i32::MIN));
    }

    #[test]
    fn secondary_mode_follows_the_test_threads_switch() {
        assert!(!is_secondary());
        assert_eq!(current(), None);
        {
            let _guard = as_secondary(SecondaryWindow {
                main: Some(99),
                ..window_on("C:\\")
            });
            assert!(is_secondary());
            assert_eq!(main_pid(), 99);
        }
        assert!(!is_secondary());
        assert_eq!(main_pid(), std::process::id());
    }

    #[test]
    fn a_file_replaced_by_renaming_over_it_is_noticed_and_an_untouched_one_is_not() {
        let dir = std::env::temp_dir().join(format!("glint-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        let file = dir.join("state.json");
        std::fs::write(&file, "one").expect("writes");
        let (heard_in, heard) = std::sync::mpsc::channel();
        let watch = watch_file(&file, move || {
            let _ = heard_in.send(());
        })
        .expect("Windows watches the folder");
        // Another file in the folder is not this one.
        std::fs::write(dir.join("other.txt"), "x").expect("writes");
        assert!(heard.recv_timeout(Duration::from_millis(600)).is_err());
        // The way the main window saves: a temp file, renamed over.
        let temp = dir.join("state.json.1.tmp");
        std::fs::write(&temp, "two, longer").expect("writes");
        std::fs::rename(&temp, &file).expect("renames");
        assert!(
            heard.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the replacement is noticed"
        );
        watch.stop();
        std::fs::write(&file, "three").expect("writes");
        assert!(
            heard.recv_timeout(Duration::from_millis(600)).is_err(),
            "a stopped watch says nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Starts a process that ends by itself after about a second.
    fn short_lived() -> std::process::Child {
        std::process::Command::new("cmd")
            .args(["/C", "ping -n 2 127.0.0.1 >nul"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("cmd starts")
    }

    #[test]
    fn a_process_that_ends_is_noticed() {
        let mut child = short_lived();
        let (ended_in, ended) = std::sync::mpsc::channel();
        let _watch = watch_process(child.id(), move || {
            let _ = ended_in.send(());
        })
        .expect("a running process can be watched");
        assert!(ended.recv_timeout(Duration::from_secs(20)).is_ok());
        let _ = child.wait();
    }

    #[test]
    fn a_process_that_is_already_gone_is_not_watched_or_is_seen_to_end_at_once() {
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .expect("cmd starts");
        let pid = child.id();
        let _ = child.wait();
        drop(child);
        let (ended_in, ended) = std::sync::mpsc::channel();
        match watch_process(pid, move || {
            let _ = ended_in.send(());
        }) {
            // The usual answer: no such process.
            Err(WatchError::Gone) => {}
            // A process object Windows still holds for someone: it is over, and says so.
            Ok(_watch) => assert!(ended.recv_timeout(Duration::from_secs(5)).is_ok()),
            Err(other) => panic!("unexpected: {other:?}"),
        }
    }

    // ---- Opening a window, asking for Settings, following the main process (with a window)

    use crate::testing;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Mutex;

    /// A window with nothing in it but the notification layer.
    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn settle(window: &mut VisualTestContext) {
        for _ in 0..40 {
            window.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// An app whose spawner records what it was asked to start (and answers `answer`), and a window.
    fn rig(cx: &mut TestAppContext, answer: Result<(), String>) -> (VisualTestContext, Arc<Mutex<Vec<Spawn>>>) {
        cx.update(testing::init);
        let spawned = Arc::new(Mutex::new(Vec::new()));
        let seen = spawned.clone();
        cx.update(|cx| {
            use_spawner(
                cx,
                Arc::new(move |spawn: &Spawn| {
                    lock(&seen).push(spawn.clone());
                    answer.clone()
                }),
            )
        });
        let (window, _) = testing::open_window(cx, |_, cx| cx.new(|_| Blank));
        (window, spawned)
    }

    #[gpui_kit::test]
    fn a_new_window_starts_this_program_on_the_folder_at_85_percent_with_the_main_process(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut window, spawned) = rig(cx, Ok(()));
        let started = Rc::new(Cell::new(false));
        let (scale, opener) = window.update(|window, _| {
            (
                window.scale_factor(),
                (
                    f32::from(window.bounds().size.width),
                    f32::from(window.bounds().size.height),
                ),
            )
        });
        let flag = started.clone();
        window.update(|window, cx| open_window(window, cx, "C:\\Work", None, move |_, _| flag.set(true)));
        settle(&mut window);
        let all = lock(&spawned).clone();
        assert_eq!(all.len(), 1, "one process");
        assert_eq!(all[0].exe, std::env::current_exe().expect("this program"));
        assert_eq!(
            parse(all[0].args.clone()),
            Ok(Some(SecondaryWindow {
                folder: "C:\\Work".into(),
                at: None,
                size: Some(new_window_size(opener, scale)),
                main: Some(std::process::id()),
            })),
            "centred, 85% of this window, following this process: {:?}",
            all[0].args
        );
        assert!(started.get(), "what follows a started window runs");

        // A tab dropped at a screen point: the corner is up and left of it.
        window.update(|window, cx| open_window(window, cx, "C:\\Work", Some((1000, 500)), |_, _| {}));
        settle(&mut window);
        let all = lock(&spawned).clone();
        let window_args = parse(all[1].args.clone()).expect("good arguments").expect("a window");
        assert_eq!(window_args.at, Some(corner_for_drop((1000, 500), scale)));
        // An Arabic folder reaches the command line whole.
        window.update(|window, cx| open_window(window, cx, "C:\\\u{645}\u{631}\u{62d}\u{628}\u{627}", None, |_, _| {}));
        settle(&mut window);
        let all = lock(&spawned).clone();
        assert_eq!(all[2].args[1], "C:\\\u{645}\u{631}\u{62d}\u{628}\u{627}");
    }

    #[gpui_kit::test]
    fn a_secondary_window_opening_another_hands_on_the_same_main_process(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut window, spawned) = rig(cx, Ok(()));
        let _secondary = as_secondary(SecondaryWindow {
            main: Some(4321),
            ..window_on("C:\\Here")
        });
        window.update(|window, cx| open_window(window, cx, "C:\\There", None, |_, _| {}));
        settle(&mut window);
        let all = lock(&spawned).clone();
        assert_eq!(
            parse(all[0].args.clone()).ok().flatten().and_then(|w| w.main),
            Some(4321)
        );
    }

    #[gpui_kit::test]
    fn a_window_that_cannot_start_says_so_and_leaves_what_follows_undone(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut window, spawned) = rig(cx, Err("Windows said no".into()));
        let started = Rc::new(Cell::new(false));
        let flag = started.clone();
        window.update(|window, cx| open_window(window, cx, "C:\\Work", None, move |_, _| flag.set(true)));
        settle(&mut window);
        assert_eq!(lock(&spawned).len(), 1);
        assert!(!started.get());
        assert_eq!(window.update(|window, cx| window.notifications(cx).len()), 1);
    }

    #[gpui_kit::test]
    fn settings_from_a_secondary_window_are_asked_of_the_main_one_and_a_silent_main_is_told(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut window, _) = rig(cx, Ok(()));
        let asked = Arc::new(Mutex::new(Vec::new()));
        let answer = Arc::new(std::sync::atomic::AtomicBool::new(true));
        {
            let (asked, answer) = (asked.clone(), answer.clone());
            window.update(|_, cx| {
                use_main_ask(
                    cx,
                    Arc::new(move |section: Option<String>| {
                        lock(&asked).push(section);
                        answer.load(Ordering::SeqCst)
                    }),
                )
            });
        }
        // The main window opens its own Settings; only a secondary window asks.
        assert!(!window.update(|window, cx| crate::components::settings::handed_to_main(window, cx, None)));
        settle(&mut window);
        assert!(lock(&asked).is_empty());
        {
            let _secondary = as_secondary(window_on("C:\\Work"));
            assert!(window.update(|window, cx| crate::components::settings::handed_to_main(
                window,
                cx,
                Some("settings-about")
            )));
            settle(&mut window);
            assert_eq!(lock(&asked).as_slice(), [Some("settings-about".to_string())]);
            assert_eq!(window.update(|window, cx| window.notifications(cx).len()), 0);
            assert!(
                !window.update(|window, cx| window.has_active_dialog(cx)),
                "no Settings of its own"
            );
            // The main window doesn't answer: the person is told.
            answer.store(false, Ordering::SeqCst);
            assert!(window.update(|window, cx| crate::components::settings::handed_to_main(window, cx, None)));
            settle(&mut window);
            assert_eq!(window.update(|window, cx| window.notifications(cx).len()), 1);
        }
    }

    #[gpui_kit::test]
    fn the_window_closes_when_the_main_process_ends_and_at_once_if_it_already_has(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.executor().allow_parking();
        cx.update(testing::init);
        let mut child = short_lived();
        let quit = Rc::new(Cell::new(false));
        let flag = quit.clone();
        cx.update(|cx| follow_process(cx, child.id(), move |_| flag.set(true)));
        cx.run_until_parked();
        assert!(!quit.get(), "the main process is still running");
        let mut ended = false;
        for _ in 0..4000 {
            cx.run_until_parked();
            if quit.get() {
                ended = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ended, "the window quit once the process ended");
        let _ = child.wait();
        // A process that is gone already: the window goes at once.
        let again = Rc::new(Cell::new(false));
        let flag = again.clone();
        cx.update(|cx| follow_process(cx, child.id(), move |_| flag.set(true)));
        for _ in 0..1000 {
            cx.run_until_parked();
            if again.get() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(again.get());
        // Without a main pid (started by hand), the window follows nothing.
        let alone = Rc::new(Cell::new(false));
        let flag = alone.clone();
        {
            let _secondary = as_secondary(window_on("C:\\Work"));
            cx.update(|cx| follow_main_process(cx, move |_| flag.set(true)));
        }
        cx.run_until_parked();
        assert!(!alone.get());
    }
}
