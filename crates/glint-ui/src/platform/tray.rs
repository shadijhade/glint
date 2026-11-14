//! The tray icon, the global hotkey and Start with Windows (port of the non-window parts of src-tauri/src/quick.rs).
//!
//! What Settings → Quick search turned on, without the Quick Search window itself (that is a view the
//! coordinator builds; this module only says when to show it):
//!
//! | quick.rs                                   | Here                                              |
//! |--------------------------------------------|---------------------------------------------------|
//! | `tray()`, the Tauri tray icon and its menu | [`set_tray`]: `Shell_NotifyIconW` + a hidden window |
//! | `parse`, `listen`, `stop` (RegisterHotKey) | [`parse`], [`set_hotkey`] (same thread, same format) |
//! | `quick_configure`                          | [`configure`]                                      |
//! | `Quick::keep_running`                      | [`keep_running`] / [`set_keep_running`]           |
//! | `set_start_with_windows`                   | [`set_start_with_windows`] (Run key or StartupTask) |
//!
//! **Threads.** [`init`] makes a hidden window on the calling (UI) thread. Its own WndProc handles the tray's
//! callback message and shows the menu with `TrackPopupMenu`, so that modal loop runs from the message pump and
//! never inside a GPUI callback. The hotkey lives on a thread of its own (Windows sends `WM_HOTKEY` to the
//! thread that registered it); that thread only posts a message to the hidden window, so every callback the
//! coordinator wires runs on the UI thread, from the pump. When Explorer restarts, the window hears
//! `TaskbarCreated` and adds the icon again.
//!
//! **A secondary window has none of this** (`platform::secondary`): [`init`] makes no window, [`set_tray`],
//! [`set_hotkey`] and [`set_start_with_windows`] do nothing, and [`keep_running`] is false, so closing the
//! window closes it. The main process owns the icon, the shortcut and what starts with Windows.
//!
//! The icon is Glint.exe's resource id 1 (another agent adds it), falling back to a stock icon.

use crate::t;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
#[cfg(test)]
use windows::Win32::System::Registry::{RegGetValueW, RRF_RT_REG_SZ};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_SETVERSION, NOTIFYICONDATAW,
    NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, GetCursorPos,
    GetMessageW, GetSystemMetrics, LoadIconW, LoadImageW, PeekMessageW, PostMessageW, PostThreadMessageW,
    RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, TrackPopupMenu, HICON, IDI_APPLICATION, IMAGE_ICON,
    LR_DEFAULTCOLOR, LR_SHARED, MF_STRING, MSG, PM_NOREMOVE, SM_CXSMICON, SM_CYSMICON, TPM_LAYOUTRTL, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WM_APP, WM_CONTEXTMENU, WM_HOTKEY, WM_LBUTTONUP, WM_NULL, WM_QUIT,
    WM_RBUTTONUP, WM_USER, WNDCLASSW, WS_POPUP,
};

// ---- What the coordinator wires -----------------------------------------------------------------------

/// What the tray and the hotkey ask the app to do. Each runs on the UI thread, from the message pump (never
/// inside a GPUI callback), so a callback should hand its work to GPUI (an `AsyncApp`, a `Window` update) and
/// return.
#[derive(Clone)]
pub struct Callbacks {
    /// Show and focus the main window (a click on the icon, the menu's Open Glint).
    pub open_main: Arc<dyn Fn() + Send + Sync>,
    /// Show (or put away) the Quick Search window (the hotkey, the menu's Quick search).
    pub quick_search: Arc<dyn Fn() + Send + Sync>,
    /// Quit Glint for real (the menu's Quit Glint).
    pub quit: Arc<dyn Fn() + Send + Sync>,
}

/// The wired callbacks, once [`init`] has run.
static CALLBACKS: Mutex<Option<Callbacks>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A callback that panicked must not turn the tray off for good.
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs one of the wired callbacks, outside the lock so it may call back into this module.
fn call(pick: impl Fn(&Callbacks) -> Arc<dyn Fn() + Send + Sync>) {
    let callback = lock(&CALLBACKS).as_ref().map(pick);
    if let Some(callback) = callback {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback())).is_err() {
            eprintln!("glint: a tray callback panicked");
        }
    }
}

// ---- The hidden window ------------------------------------------------------------------------------------

/// The tray icon's callback message (version 4 notifications).
const WM_TRAY: u32 = WM_APP + 0x20;
/// Posted by the hotkey thread: the shortcut was pressed.
const WM_SHORTCUT: u32 = WM_APP + 0x21;
/// Version 4's "selected with a click" and "selected with the keyboard" (`NIN_SELECT`, `NIN_KEYSELECT`).
const NIN_SELECT: u32 = 0x400;
const NIN_KEYSELECT: u32 = 0x401;
/// Glint.exe's icon resource id, as MAKEINTRESOURCE.
const ICON_RESOURCE: usize = 1;
/// The tray icon's id for our window.
const ICON_ID: u32 = 1;
/// The menu's commands.
const CMD_OPEN: usize = 1;
const CMD_SEARCH: usize = 2;
const CMD_QUIT: usize = 3;

/// The hidden window (0 before [`init`] and after [`shutdown`]).
static WINDOW: AtomicIsize = AtomicIsize::new(0);
/// Windows' `TaskbarCreated` message (Explorer restarted), registered by [`init`].
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
/// Whether the tray is turned on in Settings (the icon is wanted).
static TRAY_WANTED: AtomicBool = AtomicBool::new(false);
/// Whether closing the main window hides it in the tray instead of quitting.
static KEEP_RUNNING: AtomicBool = AtomicBool::new(false);
/// Whether the shell accepted version 4 callbacks (otherwise the legacy format is decoded).
static VERSION4: AtomicBool = AtomicBool::new(true);
/// How many times `TaskbarCreated` made the icon come back (for tests).
static READDED: AtomicU32 = AtomicU32::new(0);

fn window() -> Option<HWND> {
    match WINDOW.load(Ordering::Acquire) {
        0 => None,
        raw => Some(HWND(raw as *mut _)),
    }
}

/// Makes the hidden window and wires the callbacks. Call once, on the UI thread, at start (`run()`), after
/// `core_bridge::init`. A second call only replaces the callbacks. `false` if Windows refused the window
/// (the tray and the hotkey then do nothing).
pub fn init(callbacks: Callbacks) -> bool {
    if crate::platform::secondary::is_secondary() {
        return false;
    }
    *lock(&CALLBACKS) = Some(callbacks);
    if window().is_some() {
        return true;
    }
    // SAFETY: plain window-class registration and creation on this thread; the class and window names are
    // static and the procedure matches the required signature.
    unsafe {
        let Ok(module) = GetModuleHandleW(None) else {
            return false;
        };
        let class = w!("GlintTrayWindow");
        let class_def = WNDCLASSW {
            lpfnWndProc: Some(tray_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        };
        // Zero means it failed; a second call for the same class also fails, harmlessly (the class exists).
        RegisterClassW(&class_def);
        let created = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Glint tray"),
            // A hidden top-level window (never shown): message-only windows don't get the broadcast
            // `TaskbarCreated`, and Explorer restarting is when the icon must come back.
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module.into()),
            None,
        );
        let Ok(created) = created else { return false };
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Release);
        WINDOW.store(created.0 as isize, Ordering::Release);
    }
    true
}

/// Takes the icon and the hotkey away and closes the hidden window. Call when the app quits (otherwise Explorer
/// keeps a dead icon until the pointer passes over it). Safe to call twice.
pub fn shutdown() {
    remove_icon();
    if let Some(listener) = lock(&LISTENER).take() {
        stop(listener);
    }
    TRAY_WANTED.store(false, Ordering::SeqCst);
    if let Some(hwnd) = window() {
        WINDOW.store(0, Ordering::Release);
        // SAFETY: the window was made by `init`; DestroyWindow only works from its own thread (the UI thread,
        // where shutdown is called), elsewhere it fails harmlessly and Windows removes the window at exit.
        let _ = unsafe { DestroyWindow(hwnd) };
    }
}

unsafe extern "system" fn tray_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let taskbar = TASKBAR_CREATED.load(Ordering::Acquire);
    if taskbar != 0 && message == taskbar {
        // Explorer restarted and lost every icon: put ours back.
        if TRAY_WANTED.load(Ordering::SeqCst) {
            READDED.fetch_add(1, Ordering::SeqCst);
            add_icon(window);
        }
        return LRESULT(0);
    }
    match message {
        WM_TRAY => {
            // Version 4: the low word of LPARAM is the event, the high word the icon's id; WPARAM is the cursor.
            let event = (lparam.0 as u32) & 0xFFFF;
            if VERSION4.load(Ordering::SeqCst) {
                match event {
                    NIN_SELECT | NIN_KEYSELECT => call(|c| c.open_main.clone()),
                    WM_CONTEXTMENU => {
                        let (x, y) = (wparam.0 as u32 as i16 as i32, (wparam.0 as u32 >> 16) as i16 as i32);
                        show_menu(window, POINT { x, y });
                    }
                    _ => {}
                }
            } else {
                // Legacy format: LPARAM is the mouse message; the menu opens at the cursor.
                match event {
                    WM_LBUTTONUP => call(|c| c.open_main.clone()),
                    WM_RBUTTONUP => show_menu(window, POINT { x: 0, y: 0 }),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_SHORTCUT => {
            call(|c| c.quick_search.clone());
            LRESULT(0)
        }
        // SAFETY: forwarding an unhandled message to the default procedure with the arguments we were given.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

// ---- The icon and its menu ----------------------------------------------------------------------------------

/// Glint.exe's icon (resource id 1), or Windows' stock application icon when there is none (tests, a bare build).
fn load_icon() -> HICON {
    // SAFETY: loads a resource of this module (id 1 as MAKEINTRESOURCE) with LR_SHARED, so Windows caches the
    // handle and it must not be destroyed or leaked per call; the stock icon is shared too.
    unsafe {
        let module = GetModuleHandleW(None).ok();
        if let Ok(icon) = LoadImageW(
            module.map(Into::into),
            PCWSTR(ICON_RESOURCE as *const u16),
            IMAGE_ICON,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR | LR_SHARED,
        ) {
            return HICON(icon.0);
        }
        LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
    }
}

fn icon_data(window: HWND) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: ICON_ID,
        ..Default::default()
    };
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = load_icon();
    // "Glint", clipped to the buffer and ending in a zero.
    for (slot, unit) in data.szTip.iter_mut().take(127).zip(t!("Glint").encode_utf16()) {
        *slot = unit;
    }
    data
}

/// Puts the icon in the tray (and asks for version 4 notifications). Used on turning it on and on `TaskbarCreated`.
fn add_icon(window: HWND) -> bool {
    let mut data = icon_data(window);
    // SAFETY: `data` is a fully initialised NOTIFYICONDATAW with the right size and our window.
    unsafe {
        // A stale entry (Explorer remembered it) is replaced rather than refused.
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            return false;
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        // If this fails the callbacks arrive in the legacy format, which tray_proc then decodes.
        VERSION4.store(Shell_NotifyIconW(NIM_SETVERSION, &data).as_bool(), Ordering::SeqCst);
    }
    true
}

fn remove_icon() {
    let Some(window) = window() else { return };
    let data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: ICON_ID,
        ..Default::default()
    };
    // SAFETY: `data` names our own icon; deleting one that isn't there fails harmlessly.
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
}

/// The menu at `at`: Open Glint, Quick search, Quit Glint. The modal `TrackPopupMenu` runs here, in our
/// WndProc, from the message pump.
fn show_menu(window: HWND, at: POINT) {
    // SAFETY: a menu made, shown and destroyed on this thread; the strings outlive the calls that copy them.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (command, text) in [
            (CMD_OPEN, t!("Open Glint")),
            (CMD_SEARCH, t!("Quick search")),
            (CMD_QUIT, t!("Quit Glint")),
        ] {
            let text = HSTRING::from(&*text);
            let _ = AppendMenuW(menu, MF_STRING, command, &text);
        }
        let mut at = at;
        if at.x == 0 && at.y == 0 {
            let _ = GetCursorPos(&mut at);
        }
        // The menu closes on a click elsewhere only if our window is in front first.
        let _ = SetForegroundWindow(window);
        let mut flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY;
        if crate::i18n::is_rtl() {
            flags |= TPM_LAYOUTRTL;
        }
        let chosen = TrackPopupMenu(menu, flags, at.x, at.y, None, window, None);
        // Documented companion of TrackPopupMenu for tray menus: lets the menu close properly.
        let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match chosen.0 as usize {
            CMD_OPEN => call(|c| c.open_main.clone()),
            CMD_SEARCH => call(|c| c.quick_search.clone()),
            CMD_QUIT => call(|c| c.quit.clone()),
            _ => {}
        }
    }
}

/// Settings → Quick search → keep running in the tray: shows or removes the icon. UI thread. An error if the
/// window or the icon couldn't be made.
pub fn set_tray(on: bool) -> Result<(), String> {
    if crate::platform::secondary::is_secondary() {
        return Ok(());
    }
    TRAY_WANTED.store(on, Ordering::SeqCst);
    if !on {
        remove_icon();
        return Ok(());
    }
    let window = window().ok_or_else(|| t!("The tray isn't ready").to_string())?;
    if add_icon(window) {
        Ok(())
    } else {
        Err(t!("Windows wouldn't add Glint's icon to the tray").to_string())
    }
}

/// Whether closing the main window hides it in the tray (set by [`configure`]).
pub fn keep_running() -> bool {
    !crate::platform::secondary::is_secondary() && KEEP_RUNNING.load(Ordering::SeqCst)
}

/// Sets what [`keep_running`] answers, without touching the icon (a start with `--tray` before Settings loads).
pub fn set_keep_running(on: bool) {
    if crate::platform::secondary::is_secondary() {
        return;
    }
    KEEP_RUNNING.store(on, Ordering::SeqCst);
}

// ---- The global shortcut ---------------------------------------------------------------------------------------

/// The thread listening for the shortcut (its id and handle), to stop it.
type Listener = (u32, std::thread::JoinHandle<()>);
static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

/// "Ctrl+Alt+Space" as RegisterHotKey's modifiers and virtual key. A key alone is refused (it would be swallowed
/// everywhere). Letters, digits, Space and F1 to F12; modifiers Ctrl, Alt, Shift and Win.
pub fn parse(shortcut: &str) -> Option<(u32, u32)> {
    let (mut modifiers, mut key) = (0, None);
    for part in shortcut.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" => modifiers |= 0x2,
            "alt" => modifiers |= 0x1,
            "shift" => modifiers |= 0x4,
            "win" => modifiers |= 0x8,
            "space" => key = Some(0x20),
            other if other.len() == 1 && other.as_bytes()[0].is_ascii_alphanumeric() => {
                key = Some(other.as_bytes()[0].to_ascii_uppercase() as u32)
            }
            other => {
                key = Some(
                    0x6F + other
                        .strip_prefix('f')?
                        .parse::<u32>()
                        .ok()
                        .filter(|n| (1..=12).contains(n))?,
                )
            }
        }
    }
    Some((modifiers, key?)).filter(|&(modifiers, _)| modifiers != 0)
}

/// Listens for the shortcut on a thread of its own (Windows sends it to the thread that registered it). That
/// thread, or None when another app has the shortcut. A press posts [`WM_SHORTCUT`] to the hidden window, so the
/// callback runs on the UI thread.
fn listen(modifiers: u32, key: u32) -> Option<Listener> {
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        // SAFETY: RegisterHotKey/GetMessageW/UnregisterHotKey all act on this thread's own queue; the MSG is
        // a local the loop owns.
        unsafe {
            if RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT, key).is_err() {
                let _ = tx.send(None);
                return;
            }
            let mut message = MSG::default();
            // The queue exists before stop() can learn the thread's id, so its WM_QUIT can't be lost.
            let _ = PeekMessageW(&mut message, None, WM_USER, WM_USER, PM_NOREMOVE);
            let _ = tx.send(Some(GetCurrentThreadId()));
            // Until stop() posts WM_QUIT.
            while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                if message.message == WM_HOTKEY {
                    if let Some(window) = window() {
                        let _ = PostMessageW(Some(window), WM_SHORTCUT, WPARAM(0), LPARAM(0));
                    }
                }
            }
            let _ = UnregisterHotKey(None, 1);
        }
    });
    rx.recv().ok().flatten().map(|id| (id, thread))
}

/// Stops the listener and waits until it has let the shortcut go, so it can be taken again at once.
fn stop((id, thread): Listener) {
    // SAFETY: posts WM_QUIT to the listener's thread id; if that thread is gone the post fails harmlessly.
    let _ = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    let _ = thread.join();
}

/// Turns the global shortcut on (`Some("Ctrl+Alt+Space")`) or off (`None` or blank), letting the old one go
/// first. Errors say what to tell the person (an unknown shortcut, or one another app has). Blocks briefly
/// while the listener thread starts or stops: call it off the UI thread if it matters (`core_bridge::run`), or
/// accept a few milliseconds.
pub fn set_hotkey(shortcut: Option<&str>) -> Result<(), String> {
    if crate::platform::secondary::is_secondary() {
        return Ok(());
    }
    // One change at a time: the old shortcut is let go before it's taken again.
    let mut listener = lock(&LISTENER);
    if let Some(old) = listener.take() {
        stop(old);
    }
    let Some(shortcut) = shortcut.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let (modifiers, key) = parse(shortcut)
        .ok_or_else(|| t!("“{shortcut}” isn't a shortcut Glint knows", shortcut = shortcut).to_string())?;
    let started = listen(modifiers, key).ok_or_else(|| {
        t!(
            "{shortcut} is already used by another app: choose another one in Settings → Quick search",
            shortcut = shortcut
        )
        .to_string()
    })?;
    *listener = Some(started);
    Ok(())
}

/// What `quick_configure` did: Settings → Quick search. Keeps running in the tray as `tray_icon` says, and the
/// shortcut (`None`: off). The tray goes first, as before; an error from the shortcut still leaves the tray set.
/// UI thread.
#[cfg(test)]
pub fn configure(shortcut: Option<&str>, tray_icon: bool) -> Result<(), String> {
    if crate::platform::secondary::is_secondary() {
        return Ok(());
    }
    KEEP_RUNNING.store(tray_icon, Ordering::SeqCst);
    set_tray(tray_icon)?;
    set_hotkey(shortcut)
}

// ---- Start with Windows ---------------------------------------------------------------------------------------------

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// The Run value's name (the app's name, which the uninstallers expect to take out).
const RUN_NAME: &str = "Glint";
/// The Store package's startup task (AppxManifest), which starts Glint with --tray.
const STARTUP_TASK: &str = "GlintTray";

/// Starts Glint in the tray when this user signs in, or stops doing so. The Microsoft Store copy uses its
/// package's startup task; every other copy the Run key, under the app's name, running the exe File Explorer's
/// entries run, as the uninstallers expect. Blocking (the Store's task waits on Windows): call it off the UI
/// thread, e.g. `core_bridge::run(async { tray::set_start_with_windows(on) })`'s blocking variant.
pub fn set_start_with_windows(enabled: bool) -> Result<(), String> {
    // What starts with Windows is the main process's to decide.
    if crate::platform::secondary::is_secondary() {
        return Ok(());
    }
    // A debug build runs from a target folder: it must never change what an installed Glint starts with.
    if cfg!(debug_assertions) {
        return Ok(());
    }
    if glint_core::packaged() {
        return startup_task(enabled);
    }
    if enabled {
        let exe = installed_exe()
            .or_else(|| std::env::current_exe().ok())
            .ok_or_else(|| t!("Couldn't tell where Glint is").to_string())?;
        write_run(RUN_KEY, RUN_NAME, &run_command(&exe))
    } else {
        delete_run(RUN_KEY, RUN_NAME)
    }
}

/// The Run value: the exe, quoted, and `--tray` so it starts hidden.
fn run_command(exe: &std::path::Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

/// The exe File Explorer's entries run (verbs.rs's `installed_exe`): this one when the setup program installed
/// it, or a Scoop copy's `current` link. A portable copy or a dev build has none.
// TODO(coordinator): from glint_core::verbs (pub(crate) there); a private copy until it is exported.
fn installed_exe() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    if glint_core::installed_by_setup() {
        Some(exe)
    } else {
        scoop_current(&exe)
    }
}

/// A Scoop copy's `current` link to the exe (see verbs.rs).
fn scoop_current(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let name = exe.file_name()?;
    let version = exe.parent()?;
    let app = version.parent()?;
    let apps = app.parent()?;
    let current = app.join("current");
    let scoops = apps.file_name()?.eq_ignore_ascii_case("apps")
        && apps.join(r"scoop\current").is_dir()
        // scoop-install.json since Scoop 0.6, install.json before.
        && ["scoop-install.json", "install.json"].iter().any(|record| version.join(record).is_file())
        && std::fs::canonicalize(&current).ok()? == std::fs::canonicalize(version).ok()?;
    scoops.then(|| current.join(name))
}

fn win32(error: windows::Win32::Foundation::WIN32_ERROR) -> Result<(), String> {
    if error.0 == 0 {
        Ok(())
    } else {
        Err(std::io::Error::from_raw_os_error(error.0 as i32).to_string())
    }
}

/// Writes a string value under HKEY_CURRENT_USER\`key` (making the key if needed).
fn write_run(key: &str, name: &str, command: &str) -> Result<(), String> {
    let data: Vec<u8> = command
        .encode_utf16()
        .chain(Some(0))
        .flat_map(u16::to_le_bytes)
        .collect();
    // SAFETY: `data` outlives the call and its length is passed; the strings are NUL-terminated HSTRINGs.
    let error = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            data.len() as u32,
        )
    };
    win32(error)
}

/// Deletes a value; one that isn't there (or a missing key) counts as done.
fn delete_run(key: &str, name: &str) -> Result<(), String> {
    // SAFETY: plain registry call with NUL-terminated strings.
    let error = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name)) };
    if error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND {
        return Ok(());
    }
    win32(error)
}

/// A string value, if there is one.
#[cfg(test)]
fn read_run(key: &str, name: &str) -> Option<String> {
    let (key, name) = (HSTRING::from(key), HSTRING::from(name));
    let mut size = 0u32;
    // SAFETY: the first call only asks for the size; the second fills a buffer of exactly that many bytes.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .ok()
        .ok()?;
        let mut data = vec![0u8; size as usize];
        RegGetValueW(
            HKEY_CURRENT_USER,
            &key,
            &name,
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
        data.truncate(size as usize);
        let units: Vec<u16> = data.as_chunks::<2>().0.iter().map(|b| u16::from_le_bytes(*b)).collect();
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        Some(String::from_utf16_lossy(&units[..end]))
    }
}

/// The Microsoft Store copy starts at sign-in by its package's startup task (`GlintTray`, which starts it with
/// --tray), as a Run key it wrote would be its own alone. Windows' Startup apps can turn it off, and then only
/// they can turn it on again.
fn startup_task(enabled: bool) -> Result<(), String> {
    use windows::ApplicationModel::{StartupTask, StartupTaskState};
    let task = StartupTask::GetAsync(&STARTUP_TASK.into())
        .and_then(|task| task.join())
        .map_err(|e| e.message())?;
    if !enabled {
        return task.Disable().map_err(|e| e.message());
    }
    match task
        .RequestEnableAsync()
        .and_then(|state| state.join())
        .map_err(|e| e.message())?
    {
        StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy => Ok(()),
        StartupTaskState::DisabledByUser => {
            Err(t!("Glint is turned off in Windows' Startup apps (Settings → Apps → Startup)").to_string())
        }
        _ => Err(t!("Windows doesn't let Glint start when you sign in").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Registry::RegDeleteTreeW;

    #[test]
    fn reads_shortcuts() {
        assert_eq!(parse("Ctrl+Alt+Space"), Some((0x3, 0x20)));
        assert_eq!(parse("ctrl + shift + f"), Some((0x6, 0x46)));
        assert_eq!(parse("Alt+F12"), Some((0x1, 0x7B)));
        assert_eq!(parse("Win+1"), Some((0x8, 0x31)));
        assert_eq!(parse("Space"), None, "a key alone would swallow it everywhere");
        assert_eq!(parse("Ctrl+F13"), None);
        assert_eq!(parse("Ctrl+F0"), None);
        assert_eq!(parse("Ctrl+Alt"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("Ctrl+Enter"), None);
        assert_eq!(parse("Ctrl+ب"), None, "non-ASCII keys aren't shortcuts");
    }

    #[test]
    fn run_command_quotes_the_exe_and_starts_in_the_tray() {
        let exe = std::path::Path::new(r"C:\Program Files\Glint\Glint.exe");
        assert_eq!(run_command(exe), r#""C:\Program Files\Glint\Glint.exe" --tray"#);
    }

    #[test]
    fn a_blank_shortcut_turns_the_hotkey_off_without_error() {
        assert_eq!(set_hotkey(None), Ok(()));
        assert_eq!(set_hotkey(Some("   ")), Ok(()));
        assert!(set_hotkey(Some("Ctrl+Nope")).is_err());
    }

    #[test]
    fn a_secondary_window_has_no_tray_no_shortcut_and_closes_when_asked() {
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\"));
        let callbacks = Callbacks {
            open_main: Arc::new(|| {}),
            quick_search: Arc::new(|| {}),
            quit: Arc::new(|| {}),
        };
        assert!(!init(callbacks), "no hidden window, so nothing to hear the tray");
        assert!(window().is_none());
        // Turning the tray, the shortcut, Start with Windows and "keep running" on does nothing.
        assert_eq!(set_tray(true), Ok(()));
        assert!(!TRAY_WANTED.load(Ordering::SeqCst));
        assert_eq!(set_hotkey(Some("Ctrl+Alt+Space")), Ok(()));
        assert!(lock(&LISTENER).is_none(), "no listener thread for the shortcut");
        assert_eq!(configure(Some("Ctrl+Alt+Space"), true), Ok(()));
        assert_eq!(set_start_with_windows(true), Ok(()));
        set_keep_running(true);
        assert!(!keep_running(), "closing the window closes it");
    }

    #[test]
    fn keep_running_follows_the_setting() {
        set_keep_running(true);
        assert!(keep_running());
        set_keep_running(false);
        assert!(!keep_running());
    }

    /// A key of our own under HKCU, removed at the end, so the real Run key is never touched.
    struct TestKey(String);

    impl TestKey {
        fn new(tag: &str) -> Self {
            Self(format!(r"Software\GlintUiTests\tray-{tag}-{}", std::process::id()))
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            // SAFETY: deletes the test key made by this test, by name.
            let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(self.0.as_str())) };
        }
    }

    #[test]
    fn the_run_value_is_written_read_and_removed() {
        let key = TestKey::new("run");
        assert_eq!(read_run(&key.0, "Glint"), None);
        let command = r#""C:\Users\شادي\Glint\Glint.exe" --tray"#;
        assert_eq!(write_run(&key.0, "Glint", command), Ok(()));
        assert_eq!(
            read_run(&key.0, "Glint").as_deref(),
            Some(command),
            "unicode paths survive"
        );
        assert_eq!(write_run(&key.0, "Glint", "other"), Ok(()), "writing again replaces");
        assert_eq!(read_run(&key.0, "Glint").as_deref(), Some("other"));
        assert_eq!(delete_run(&key.0, "Glint"), Ok(()));
        assert_eq!(read_run(&key.0, "Glint"), None);
        assert_eq!(delete_run(&key.0, "Glint"), Ok(()), "removing what isn't there is fine");
        assert_eq!(
            delete_run(&format!("{}\\missing", key.0), "Glint"),
            Ok(()),
            "so is a missing key"
        );
    }

    #[test]
    fn scoop_links_are_only_for_scoop_layouts() {
        // Not under an `apps` folder: not a Scoop copy.
        assert_eq!(
            scoop_current(std::path::Path::new(r"C:\Tools\Glint\1.0\Glint.exe")),
            None
        );
        assert_eq!(scoop_current(std::path::Path::new("Glint.exe")), None);
    }

    #[test]
    fn the_tray_window_comes_up_and_goes_away() {
        let _guard = crate::testing::lock();
        let opened = Arc::new(AtomicBool::new(false));
        let seen = opened.clone();
        let nothing: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        assert!(super::init(Callbacks {
            open_main: Arc::new(move || seen.store(true, Ordering::SeqCst)),
            quick_search: nothing.clone(),
            quit: nothing,
        }));
        assert!(window().is_some());
        // A shortcut press arrives on the UI thread's pump as the callback; call the handler the way it would.
        call(|c| c.open_main.clone());
        assert!(opened.load(Ordering::SeqCst));
        shutdown();
        assert!(window().is_none());
        shutdown();
    }

    #[test]
    #[ignore = "adds a real icon to this PC's tray for a moment; run with --ignored"]
    fn explorer_restarting_brings_the_icon_back() {
        use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, SendMessageW, GA_PARENT, HWND_MESSAGE};
        let _guard = crate::testing::lock();
        let nothing: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        assert!(super::init(Callbacks {
            open_main: nothing.clone(),
            quick_search: nothing.clone(),
            quit: nothing,
        }));
        let hwnd = window().expect("window");
        // SAFETY: asks about our own live window.
        let parent = unsafe { GetAncestor(hwnd, GA_PARENT) };
        assert_ne!(parent, HWND_MESSAGE, "a message-only window never hears TaskbarCreated");
        let message = TASKBAR_CREATED.load(Ordering::SeqCst);
        assert_ne!(message, 0);
        TRAY_WANTED.store(true, Ordering::SeqCst);
        let before = READDED.load(Ordering::SeqCst);
        // SAFETY: sends the registered message to our own window on this thread (runs tray_proc directly).
        unsafe { SendMessageW(hwnd, message, Some(WPARAM(0)), Some(LPARAM(0))) };
        assert_eq!(READDED.load(Ordering::SeqCst), before + 1);
        shutdown();
    }
}
