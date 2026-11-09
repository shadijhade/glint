//! Single instance and launch requests (port of src-tauri/src/launch.rs and the single-instance plugin).
//!
//! Folders handed to the app from outside: on its command line (File Explorer's "Open in Glint", a
//! double-click on a folder once folders open here by default, a terminal), and from later launches,
//! which give theirs to the copy that's already running instead of opening a second window.
//! Whatever the app can't open (This PC's shell siblings, a share that keeps the user out, a drive
//! with no disc) goes to File Explorer instead, so it opens as it always did.
//!
//! How one copy runs at a time without Tauri's plugin:
//! - A named mutex in the session's `Local\` namespace decides who is first.
//! - The first copy owns a hidden message-only window on a thread of its own. A later launch finds it by
//!   its title and sends its parsed [`Request`] as `WM_COPYDATA`, then exits. The window's thread only
//!   decodes the message and puts it in a channel, so a sender never waits for GPUI.
//! - [`init`] drains that channel into a GPUI entity ([`LaunchEvents`]), so requests that arrived while
//!   the window was still opening are delivered, in order, as soon as the workspace listens.
//!
//! The coordinator calls [`begin`] first in `main`/`run()` (before GPUI), gives
//! [`Startup::Run`]'s request to the workspace (its `open` targets are the first tabs), calls
//! [`set_main_window`] once the main window exists and [`init`] with the app.

use glint_core::{archive, fsx};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Window};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A folder to open in a new tab, and the file to select in it when a file was given.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Target {
    pub path: String,
    pub select: Option<String>,
}

/// What a command line asks for: folders to open here, what File Explorer should open instead, and the
/// flags Glint understands (`--tray`, `--quick-search`).
#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Clone)]
#[serde(default)]
pub struct Request {
    pub open: Vec<Target>,
    pub explorer: Vec<String>,
    /// `--tray`: start hidden in the tray (a first launch only; a later launch just brings the window forward).
    pub tray: bool,
    /// `--quick-search`: the taskbar's Quick search task.
    pub quick_search: bool,
}

impl Request {
    /// This process's command line.
    pub fn from_env() -> Request {
        let cwd = std::env::current_dir().unwrap_or_default();
        Request::parse(
            std::env::args_os()
                .skip(1)
                .map(|arg| arg.to_string_lossy().into_owned()),
            &cwd,
        )
    }

    /// Sorts the arguments (without the program's own path) out. Relative paths are relative to
    /// `cwd`; other options (anything starting with "-") are left out, as the app has none.
    pub fn parse(args: impl IntoIterator<Item = String>, cwd: &Path) -> Request {
        let mut request = Request::default();
        let mut iter = args.into_iter().peekable();
        while let Some(raw) = iter.next() {
            let arg = raw.trim();
            if arg.eq_ignore_ascii_case("--tray") {
                request.tray = true;
            } else if arg.eq_ignore_ascii_case("--quick-search") {
                request.quick_search = true;
            }
            if arg.is_empty() || arg.starts_with('-') {
                continue;
            }
            // Explorer-style flags: `/select,"C:\path"`, `/select, C:\path`, `/e,/select,C:\path`, `/root,C:\path`
            if let Some((path_arg, force_select)) = parse_explorer_switch(arg, &mut iter) {
                if path_arg.is_empty() {
                    continue;
                }
                match target_with_mode(&path_arg, cwd, force_select) {
                    Ok(t) => request.open.push(t),
                    Err(other) => request.explorer.push(other),
                }
                continue;
            }
            match target(arg, cwd) {
                Ok(target) => request.open.push(target),
                Err(other) => request.explorer.push(other),
            }
        }
        request
    }
}

/// Parses Explorer command-line switches (`/select`, `/root`, `/e`, `/n`).
fn parse_explorer_switch(
    arg: &str,
    iter: &mut std::iter::Peekable<impl Iterator<Item = String>>,
) -> Option<(String, bool)> {
    if !arg.starts_with('/') {
        return None;
    }
    let mut force_select = false;
    let mut rest = arg;
    loop {
        let trimmed = rest.trim_start_matches([' ', ',']);
        let l = trimmed.to_ascii_lowercase();
        // A switch ends at a comma, a space or the end: `/e` isn't `/explore`.
        let Some((switch, selects)) = [("/select", true), ("/root", false), ("/e", false), ("/n", false)]
            .into_iter()
            .find(|(switch, _)| {
                l.starts_with(switch) && matches!(l.as_bytes().get(switch.len()), None | Some(b',' | b' '))
            })
        else {
            break;
        };
        force_select |= selects;
        rest = &trimmed[switch.len()..];
    }
    if rest.len() == arg.len() {
        return None;
    }
    let inline = rest.trim_matches([' ', ',', '"']);
    if !inline.is_empty() {
        return Some((inline.to_string(), force_select));
    }
    if let Some(next) = iter.peek() {
        let next_trim = next.trim();
        if !next_trim.is_empty() && !next_trim.starts_with('-') && !next_trim.starts_with('/') {
            // `peek` just showed there is a next item.
            let next_owned = iter.next()?;
            return Some((next_owned.trim().trim_matches('"').to_string(), force_select));
        }
    }
    Some((String::new(), force_select))
}

/// What the app opens for one argument, or else what File Explorer should get.
fn target(arg: &str, cwd: &Path) -> Result<Target, String> {
    target_with_mode(arg, cwd, false)
}

fn target_with_mode(arg: &str, cwd: &Path, force_select: bool) -> Result<Target, String> {
    // File Explorer passes a drive as "C:\" in quotes, which the command line reads as C:" since a
    // backslash before a quote escapes it.
    let arg = arg.trim_matches('"');
    if let Some(virtual_target) = resolve_virtual_location(arg) {
        return Ok(virtual_target);
    }
    let resolved_file_url = if arg
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
    {
        file_url_to_path(arg)
    } else {
        None
    };
    let arg = resolved_file_url.as_deref().unwrap_or(arg);
    if is_shell_location(arg) {
        return Err(arg.to_string());
    }
    let text = if Path::new(arg).is_absolute() || arg.starts_with(['~', '%']) {
        arg.to_string()
    } else {
        cwd.join(arg).to_string_lossy().into_owned()
    };
    let Ok(path) = fsx::normalize(&text) else {
        return Err(arg.to_string());
    };
    let Some(info) = fsx::stat(&path) else { return Err(path) };
    if info.dir && !force_select {
        return if opens_here(&path) {
            Ok(Target { path, select: None })
        } else {
            Err(path)
        };
    }
    // A zip or 7z archive opens like a folder (when it's opened with Glint), unless `/select` asked to highlight the archive file itself.
    if !info.dir && !force_select && archive::is_archive_name(&path) {
        return Ok(Target { path, select: None });
    }
    // A file (or `/select,folder`): its parent folder, with the item selected.
    let folder = Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !folder.is_empty() && opens_here(&folder) {
        Ok(Target {
            path: fsx::normalize(&folder).unwrap_or(folder),
            select: Some(path),
        })
    } else {
        Err(path)
    }
}

/// A `file:` URL's path (`file:///C:/My%20Files` is `C:\My Files`, `file://server/share/x` is
/// `\\server\share\x`). The web app used the `url` crate for this; the UI crate has no use for the rest of it.
fn file_url_to_path(url: &str) -> Option<String> {
    let rest = url.get(7..)?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = match rest.find('/') {
        Some(slash) => (rest.get(..slash)?, rest.get(slash..)?),
        None => (rest, ""),
    };
    let decoded = percent_decode(path);
    let local = host.is_empty() || host.eq_ignore_ascii_case("localhost");
    let slashes = decoded.replace('/', "\\");
    if local {
        // "\C:\Windows" -> "C:\Windows"
        let drive = slashes.strip_prefix('\\')?;
        let mut chars = drive.chars();
        let (letter, colon) = (chars.next()?, chars.next()?);
        (letter.is_ascii_alphabetic() && colon == ':').then(|| drive.to_string())
    } else {
        Some(format!("\\\\{host}{slashes}"))
    }
}

/// `%XX` escapes decoded; a bad escape stays as it was, and bytes that aren't UTF-8 become U+FFFD.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let escaped = (byte == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(value) => {
                out.push(value);
                i += 3;
            }
            None => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolves Windows virtual shell locations (`This PC`, `Recycle Bin`, and `shell:<KnownFolder>`) directly into Glint targets.
fn resolve_virtual_location(arg: &str) -> Option<Target> {
    let lower = arg.trim().to_ascii_lowercase();
    match lower.as_str() {
        "::{20d04fe0-3aea-1069-a2d8-08002b30309d}"
        | "shell:::{20d04fe0-3aea-1069-a2d8-08002b30309d}"
        | "shell:mycomputerfolder"
        | "this-pc:" => {
            return Some(Target {
                path: String::new(),
                select: None,
            })
        }
        "::{645ff040-5081-101b-9f08-00aa002f954e}"
        | "shell:::{645ff040-5081-101b-9f08-00aa002f954e}"
        | "shell:recyclebinfolder" => {
            return Some(Target {
                path: "shell:RecycleBinFolder".into(),
                select: None,
            })
        }
        _ => {}
    }
    use windows::core::GUID;
    use windows::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData, FOLDERID_Music,
        FOLDERID_Pictures, FOLDERID_Profile, FOLDERID_ProgramFiles, FOLDERID_Recent, FOLDERID_RoamingAppData,
        FOLDERID_Startup, FOLDERID_Videos, FOLDERID_Windows, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
    };
    let shell_name = lower.strip_prefix("shell:")?;
    // Windows' own names have spaces ("My Pictures", "Local AppData").
    let id: GUID = match shell_name.replace(' ', "").as_str() {
        "downloads" => FOLDERID_Downloads,
        "desktop" => FOLDERID_Desktop,
        "personal" | "documents" => FOLDERID_Documents,
        "mypictures" | "pictures" => FOLDERID_Pictures,
        "mymusic" | "music" => FOLDERID_Music,
        "myvideo" | "videos" => FOLDERID_Videos,
        "profile" | "usersfilesfolder" => FOLDERID_Profile,
        "recent" => FOLDERID_Recent,
        "appdata" => FOLDERID_RoamingAppData,
        "localappdata" => FOLDERID_LocalAppData,
        "startup" => FOLDERID_Startup,
        "programfiles" => FOLDERID_ProgramFiles,
        "windows" => FOLDERID_Windows,
        _ => return None,
    };
    // SAFETY: `id` is a valid known-folder GUID; the returned buffer is read once and freed with CoTaskMemFree.
    let pwstr = unsafe { SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) }.ok()?;
    // SAFETY: `pwstr` is the NUL-terminated string Windows just returned.
    let text = unsafe { pwstr.to_string() }.ok();
    // SAFETY: the buffer came from SHGetKnownFolderPath, which documents CoTaskMemFree for it; not used again.
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(pwstr.0 as _)) };
    let folder = text.and_then(|t| fsx::normalize(&t).ok())?;
    Some(Target {
        path: folder,
        select: None,
    })
}

/// Whether the app opens a folder: one it can list, or one on this PC that Windows keeps this user
/// out of, which its tab says, offering to get access. A share that keeps them out is File Explorer's,
/// which can ask for another user name and password.
fn opens_here(dir: &str) -> bool {
    match fsx::probe(dir) {
        Ok(()) => true,
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied && !fsx::on_other_computer(dir),
    }
}

/// shell:Downloads, ::{CLSID} (This PC, Control Panel...), search-ms:, file: and other URLs: File Explorer's.
fn is_shell_location(arg: &str) -> bool {
    if arg.starts_with("::") {
        return true;
    }
    // A scheme is two or more letters, digits, "+", "-" or "." before a colon; "C:" is a drive.
    arg.find(':').is_some_and(|colon| {
        colon > 1
            && arg[..colon]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    })
}

/// Opens what the app can't in File Explorer, with its own "open" (which stays File Explorer's even
/// when folders open here by default, so this can't come back here).
pub fn hand_to_explorer(items: &[String]) {
    use windows::core::{w, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    // File Explorer's "open" is a COM verb.
    // SAFETY: plain COM initialisation for this thread; balanced by CoUninitialize below when it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    for item in items {
        let file = HSTRING::from(item.as_str());
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            // Finished before it returns, since this process may end right after.
            fMask: SEE_MASK_NOASYNC,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        // SAFETY: `info` is fully initialised and `file` (the string `lpFile` points to) outlives the call.
        let _ = unsafe { ShellExecuteExW(&mut info) };
    }
    if com.is_ok() {
        // SAFETY: CoInitializeEx succeeded on this thread above.
        unsafe { CoUninitialize() };
    }
}

/// The taskbar's Quick search task starts Glint with `--quick-search`.
pub fn asks_for_quick_search<S: AsRef<str>>(mut args: impl Iterator<Item = S>) -> bool {
    args.any(|arg| arg.as_ref().trim().eq_ignore_ascii_case("--quick-search"))
}

// ---- Bringing the window forward ----------------------------------------------------------------------

/// The main window's HWND (0 until [`set_main_window`]).
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Remembers the main window, so a later launch can bring it forward. Call once it exists.
pub fn set_main_window(window: &Window) {
    if let Some(hwnd) = crate::core_bridge::hwnd_of(window) {
        MAIN_WINDOW.store(hwnd.0 as isize, Ordering::Release);
    }
}

/// Shows the main window (out of the tray or minimised) and brings it forward. Does nothing before
/// [`set_main_window`].
pub fn bring_to_front() {
    let raw = MAIN_WINDOW.load(Ordering::Acquire);
    if raw != 0 {
        bring_forward(windows::Win32::Foundation::HWND(raw as *mut _));
    }
}

/// `SetForegroundWindow` rather than a synthetic focus request, which presses Alt when Windows says no.
/// What asked for it (a launch handing over its folders, the shortcut, the tray) allowed this process to come forward.
pub fn bring_forward(hwnd: windows::Win32::Foundation::HWND) {
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW};
    // SAFETY: plain window calls; a stale handle makes them fail, never misbehave.
    unsafe {
        let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        let _ = SetForegroundWindow(hwnd);
    }
}

// ---- One copy at a time ---------------------------------------------------------------------------------

/// What [`begin`] decided.
#[derive(Debug, PartialEq)]
pub enum Startup {
    /// This copy runs. Open `request.open` as the first tabs; `request.tray` asks to start in the tray.
    Run(Request),
    /// Nothing more to do here: the request went to the running copy or to File Explorer. Return from `main`.
    Exit,
}

/// How long a later launch waits for the running copy's window before it gives up and runs anyway.
const FORWARD_PATIENCE: Duration = Duration::from_secs(3);

/// Call first in `main`, before GPUI. Reads the command line, gives File Explorer what the app can't open,
/// and decides who is first: the first copy returns [`Startup::Run`] (and starts listening for later
/// launches), a later one hands its folders over and returns [`Startup::Exit`].
///
/// Debug builds use a name of their own, so a build run from a target folder neither takes over from nor
/// hands its folders to an installed Glint (which the web app avoided by not enforcing it at all).
pub fn begin() -> Startup {
    let request = Request::from_env();
    if !request.explorer.is_empty() {
        hand_to_explorer(&request.explorer);
        if request.open.is_empty() && !request.tray && !request.quick_search {
            return Startup::Exit;
        }
    }
    begin_with(
        if cfg!(debug_assertions) { "debug" } else { "v1" },
        request,
        FORWARD_PATIENCE,
    )
}

/// The part of [`begin`] that is about instances, with the name and patience as parameters for tests.
fn begin_with(name: &str, request: Request, patience: Duration) -> Startup {
    match claim(name) {
        Some(Claim::First(endpoint)) => {
            *lock(&INSTANCE) = Some(endpoint);
            Startup::Run(request)
        }
        Some(Claim::Later) => {
            // File Explorer already has what the app can't open (that was this launch's first step).
            let forwarded = Request {
                explorer: Vec::new(),
                ..request.clone()
            };
            if forward(name, &forwarded, patience) {
                Startup::Exit
            } else {
                // The running copy didn't answer (hung, or on its way out): better a second window than nothing.
                Startup::Run(request)
            }
        }
        // Windows refused the mutex or the window: run without the guarantee.
        None => Startup::Run(request),
    }
}

/// This copy's endpoint, kept for the life of the process once [`begin`] made it.
static INSTANCE: Mutex<Option<Endpoint>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

enum Claim {
    First(Endpoint),
    Later,
}

/// The session-wide mutex's name (`Local\` is per logon session, as a user's single copy should be).
fn mutex_name(name: &str) -> String {
    format!("Local\\Glint.SingleInstance.{name}")
}

/// The listener window's title, which a later launch finds it by.
fn window_title(name: &str) -> String {
    format!("Glint.Launch.{name}")
}

/// The window class of the listener (the title tells the endpoints apart).
const LISTENER_CLASS: windows::core::PCWSTR = windows::core::w!("GlintLaunchListener");
/// Identifies our `WM_COPYDATA` among anything else sent to the window ("GLNT" + 1).
const COPYDATA_ID: usize = 0x474C_4E54_0001;
/// The largest request accepted: far more folders than anyone passes on a command line.
const MAX_MESSAGE: usize = 1 << 20;

/// The first copy's claim on its name: the mutex and the thread that owns the listener window.
struct Endpoint {
    mutex: isize,
    window: isize,
    thread: Option<std::thread::JoinHandle<()>>,
    incoming: async_channel::Receiver<Request>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        // SAFETY: `window` is the listener window made by `claim`; posting to a window is allowed from any thread.
        let _ = unsafe {
            PostMessageW(
                Some(HWND(self.window as *mut _)),
                WM_CLOSE,
                Default::default(),
                Default::default(),
            )
        };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // SAFETY: `mutex` is the handle CreateMutexW gave `claim`, closed exactly once, here.
        let _ = unsafe { CloseHandle(HANDLE(self.mutex as *mut _)) };
    }
}

/// Takes the name: the first caller gets the endpoint, later ones (in this or another process) `Later`.
/// `None` when Windows refuses.
fn claim(name: &str) -> Option<Claim> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let mutex_name = HSTRING::from(mutex_name(name));
    // SAFETY: a plain named-mutex creation; `mutex_name` outlives the call.
    let mutex = unsafe { CreateMutexW(None, false, &mutex_name) }.ok()?;
    // SAFETY: reads this thread's last error straight after the call that set it.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if existed {
        // SAFETY: the handle was just returned to us and is not used again.
        let _ = unsafe { CloseHandle(mutex) };
        return Some(Claim::Later);
    }
    let (incoming_in, incoming) = async_channel::unbounded();
    let (ready_in, ready) = std::sync::mpsc::channel();
    let title = window_title(name);
    let thread = std::thread::Builder::new()
        .name("glint-launch".into())
        .spawn(move || listen(&title, incoming_in, ready_in))
        .ok();
    let window = thread.as_ref().and_then(|_| ready.recv().ok()).flatten();
    match (thread, window) {
        (Some(thread), Some(window)) => Some(Claim::First(Endpoint {
            mutex: mutex.0 as isize,
            window,
            thread: Some(thread),
            incoming,
        })),
        (thread, _) => {
            // No listener: don't hold the name, or later launches would wait for a window that never comes.
            // SAFETY: the handle is ours, not stored anywhere, and not used again.
            let _ = unsafe { CloseHandle(mutex) };
            drop(thread);
            None
        }
    }
}

/// Where each listener window's requests go, by window handle.
static SINKS: Mutex<Vec<(isize, async_channel::Sender<Request>)>> = Mutex::new(Vec::new());

/// The listener thread: makes the window, tells `claim` its handle (or `None`), then runs its message loop
/// until the window is closed.
fn listen(title: &str, sink: async_channel::Sender<Request>, ready: std::sync::mpsc::Sender<Option<isize>>) {
    use windows::core::HSTRING;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, HWND_MESSAGE, MSG,
        WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
    };

    // SAFETY: class registration and window creation on this thread, with static class data and a
    // procedure of the required signature; `title` outlives the call.
    let window = unsafe {
        let module = GetModuleHandleW(None).ok();
        let class = WNDCLASSW {
            lpfnWndProc: Some(listener_proc),
            hInstance: module.map(Into::into).unwrap_or_default(),
            lpszClassName: LISTENER_CLASS,
            ..Default::default()
        };
        // Zero means it failed; a second endpoint in this process finds the class there already.
        RegisterClassW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            LISTENER_CLASS,
            &HSTRING::from(title),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            module.map(Into::into),
            None,
        )
    };
    let Ok(window) = window else {
        let _ = ready.send(None);
        return;
    };
    let raw = window.0 as isize;
    lock(&SINKS).push((raw, sink));
    let _ = ready.send(Some(raw));

    let mut message = MSG::default();
    // SAFETY: the standard message loop on the thread that owns the window; ends on WM_QUIT (0) or an error (-1).
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    lock(&SINKS).retain(|(handle, _)| *handle != raw);
}

unsafe extern "system" fn listener_proc(
    window: windows::Win32::Foundation::HWND,
    message: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DestroyWindow, PostQuitMessage, WM_CLOSE, WM_COPYDATA, WM_DESTROY,
    };
    match message {
        WM_COPYDATA => LRESULT(isize::from(accept(window.0 as isize, lparam.0))),
        WM_CLOSE => {
            // SAFETY: destroying our own window from its own thread.
            let _ = unsafe { DestroyWindow(window) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: forwarding an unhandled message to the default procedure with the arguments we were given.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Decodes a `WM_COPYDATA` (its `COPYDATASTRUCT` address is `lparam`) into the window's channel. False when
/// it isn't ours or doesn't parse, so the sender knows.
fn accept(window: isize, lparam: isize) -> bool {
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    // SAFETY: for WM_COPYDATA, LPARAM is the address of a COPYDATASTRUCT that stays valid while the message is handled.
    let Some(data) = (unsafe { (lparam as *const COPYDATASTRUCT).as_ref() }) else {
        return false;
    };
    let len = data.cbData as usize;
    if data.dwData != COPYDATA_ID || data.lpData.is_null() || len > MAX_MESSAGE {
        return false;
    }
    // SAFETY: the struct says `lpData` points to `cbData` readable bytes, valid for this call; copied out at once.
    let bytes = unsafe { std::slice::from_raw_parts(data.lpData as *const u8, len) };
    let Ok(request) = serde_json::from_slice::<Request>(bytes) else {
        return false;
    };
    let sink = lock(&SINKS)
        .iter()
        .find(|(handle, _)| *handle == window)
        .map(|(_, sink)| sink.clone());
    sink.is_some_and(|sink| sink.try_send(request).is_ok())
}

/// Sends `payload` to the listener window titled for `name`, if there is one: true once it took it.
fn send_copydata(window: windows::Win32::Foundation::HWND, id: usize, payload: &[u8]) -> bool {
    use windows::Win32::System::DataExchange::COPYDATASTRUCT;
    use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_COPYDATA};
    let data = COPYDATASTRUCT {
        dwData: id,
        cbData: u32::try_from(payload.len()).unwrap_or(u32::MAX),
        lpData: payload.as_ptr() as *mut _,
    };
    let mut answer = 0usize;
    // SAFETY: `data` and `payload` outlive this synchronous call, which only reads them.
    let sent = unsafe {
        SendMessageTimeoutW(
            window,
            WM_COPYDATA,
            Default::default(),
            windows::Win32::Foundation::LPARAM(&data as *const COPYDATASTRUCT as isize),
            SMTO_ABORTIFHUNG,
            3000,
            Some(&mut answer),
        )
    };
    sent.0 != 0 && answer == 1
}

/// Finds the listener window of the copy that owns `name`.
fn find_listener(name: &str) -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::HSTRING;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, HWND_MESSAGE};
    let title = HSTRING::from(window_title(name));
    // SAFETY: a plain window search among message-only windows; the strings outlive the call.
    unsafe { FindWindowExW(Some(HWND_MESSAGE), None, LISTENER_CLASS, &title) }.ok()
}

/// Hands `request` to the copy that owns `name`, waiting up to `patience` for its window to appear (it
/// may still be starting). True once that copy has taken it.
fn forward(name: &str, request: &Request, patience: Duration) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, GetWindowThreadProcessId};
    let Ok(payload) = serde_json::to_vec(request) else {
        return false;
    };
    if payload.len() > MAX_MESSAGE {
        return false;
    }
    let deadline = Instant::now() + patience;
    loop {
        if let Some(window) = find_listener(name) {
            let mut process = 0u32;
            // SAFETY: `process` is a valid out-pointer for the call.
            unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };
            // The running copy may bring its window forward for this launch.
            // SAFETY: plain call; failure only means no permission was granted.
            let _ = unsafe { AllowSetForegroundWindow(process) };
            if send_copydata(window, COPYDATA_ID, &payload) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

// ---- Delivery into GPUI ---------------------------------------------------------------------------------

/// What a later launch asked of the running copy.
#[derive(Clone, Debug, PartialEq)]
pub enum LaunchEvent {
    /// Open these folders, each in a new tab, selecting the file when one is given. The window has already
    /// been brought forward.
    Open(Vec<Target>),
    /// The taskbar's Quick search task: summon quick search (the window is not brought forward).
    QuickSearch,
}

/// The entity [`LaunchEvent`]s are emitted from. It holds nothing: it exists to be subscribed to.
pub struct LaunchEvents;

impl EventEmitter<LaunchEvent> for LaunchEvents {}

/// The [`LaunchEvents`] entity, in a GPUI `Global`: `cx.global::<LaunchHub>().0`.
pub struct LaunchHub(pub Entity<LaunchEvents>);

impl Global for LaunchHub {}

/// Starts delivering later launches' requests as [`LaunchEvent`]s. Requests that arrived since [`begin`]
/// are delivered first. Call after the main window exists (and [`set_main_window`]), then subscribe:
///
/// ```ignore
/// let hub = launch::init(cx);
/// cx.subscribe(&hub, |workspace, _, event: &launch::LaunchEvent, cx| match event { ... }).detach();
/// ```
///
/// Returns the entity (also kept as [`LaunchHub`]). Without a running listener (this copy didn't win the
/// name, or [`begin`] wasn't called) the entity exists but never emits.
pub fn init(cx: &mut App) -> Entity<LaunchEvents> {
    let events = cx.new(|_| LaunchEvents);
    cx.set_global(LaunchHub(events.clone()));
    let incoming = lock(&INSTANCE).as_ref().map(|endpoint| endpoint.incoming.clone());
    if let Some(incoming) = incoming {
        let sink = events.clone();
        cx.spawn(async move |cx| {
            while let Ok(request) = incoming.recv().await {
                // Outside any update: ShowWindow and SetForegroundWindow send GPUI's window procedure messages
                // at once, and its handlers need the app, which an update holds.
                if !request.quick_search {
                    bring_to_front();
                }
                sink.update(cx, |_, cx| deliver(request, cx));
            }
        })
        .detach();
    }
    events
}

/// What the web app's `second_launch` did on the UI thread: quick search, or the window forward and the folders.
fn deliver(request: Request, cx: &mut Context<LaunchEvents>) {
    if request.quick_search {
        cx.emit(LaunchEvent::QuickSearch);
        return;
    }
    // The window came forward before this update (see `init`).
    if !request.open.is_empty() {
        cx.emit(LaunchEvent::Open(request.open));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A folder of the test's own: tests run in parallel, and each deletes its folder when done.
    fn scratch(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-launch-{}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub folder")).unwrap();
        fs::write(dir.join("sub folder").join("notes.txt"), "hi").unwrap();
        dir
    }

    fn open(path: &Path) -> Target {
        Target {
            path: path.to_string_lossy().into_owned(),
            select: None,
        }
    }

    #[test]
    fn folders_files_and_drives() {
        let dir = scratch("folders");
        let sub = dir.join("sub folder");
        let file = sub.join("notes.txt");
        let parse = |args: &[&str]| Request::parse(args.iter().map(|a| a.to_string()), &dir);

        assert_eq!(parse(&[&sub.to_string_lossy()]).open, [open(&sub)]);
        // Relative to the folder the launch ran in, with any trailing separator dropped.
        assert_eq!(parse(&["sub folder\\"]).open, [open(&sub)]);
        assert_eq!(parse(&["."]).open, [open(&dir)]);
        // A file opens its folder with it selected.
        let request = parse(&[&file.to_string_lossy()]);
        assert_eq!(
            request.open,
            [Target {
                path: sub.to_string_lossy().into_owned(),
                select: Some(file.to_string_lossy().into_owned())
            }]
        );
        // "C:\" in quotes arrives as C:" (and "C:" alone means the drive).
        let drive = std::env::var("SystemDrive").unwrap();
        assert_eq!(
            parse(&[&format!("{drive}\"")]).open,
            [Target {
                path: format!("{drive}\\"),
                select: None
            }]
        );
        assert_eq!(
            parse(&[&drive]).open,
            [Target {
                path: format!("{drive}\\"),
                select: None
            }]
        );
        // Options are left out.
        assert_eq!(parse(&["--flag", "-x", ""]), Request::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_app_cannot_open_goes_to_file_explorer() {
        let dir = scratch("explorer");
        let sub = dir.join("sub folder");
        let file = sub.join("notes.txt");
        let parse = |arg: &str| Request::parse([arg.to_string()], &dir);
        for place in [
            "shell:::{26EE0668-A00A-44D7-9371-BEB064C98683}",
            "SHELL:Libraries",
            "search-ms:query=report",
        ] {
            assert_eq!(
                parse(place),
                Request {
                    explorer: vec![place.to_string()],
                    ..Request::default()
                },
                "{place}"
            );
        }
        // Virtual shell places that Glint owns directly (This PC, Recycle Bin, Known Folders, file:// URLs, /select).
        assert_eq!(
            parse("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}").open,
            [Target {
                path: String::new(),
                select: None
            }]
        );
        assert_eq!(
            parse("this-pc:").open,
            [Target {
                path: String::new(),
                select: None
            }]
        );
        assert_eq!(
            parse("::{645FF040-5081-101B-9F08-00AA002F954E}").open,
            [Target {
                path: "shell:RecycleBinFolder".into(),
                select: None
            }]
        );
        assert_eq!(
            parse("shell:RecycleBinFolder").open,
            [Target {
                path: "shell:RecycleBinFolder".into(),
                select: None
            }]
        );
        assert!(!parse("shell:Downloads").open.is_empty());
        assert_eq!(
            parse(r"file:///C:/Windows").open,
            [Target {
                path: r"C:\Windows".into(),
                select: None
            }]
        );
        assert_eq!(
            parse(&format!(r#"/select,"{}""#, file.display())).open,
            [Target {
                path: sub.to_string_lossy().into_owned(),
                select: Some(file.to_string_lossy().into_owned())
            }]
        );
        // Missing: File Explorer says so, as it would have.
        let missing = dir.join("missing");
        assert_eq!(parse("missing").explorer, [missing.to_string_lossy().into_owned()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_windows_keeps_the_user_out_of_open_here() {
        // Where Windows has one: its tab says why it can't be listed, and offers to get access.
        let denied = r"C:\System Volume Information";
        if Path::new(denied).exists() && fs::read_dir(denied).is_err() {
            assert_eq!(
                Request::parse([denied.to_string()], Path::new("C:\\")).open,
                [Target {
                    path: denied.into(),
                    select: None
                }]
            );
        }
    }

    #[test]
    fn shell_locations() {
        assert!(is_shell_location("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"));
        assert!(is_shell_location("shell:Downloads"));
        assert!(is_shell_location("ms-settings:storagesense"));
        assert!(!is_shell_location(r"C:\Windows"));
        assert!(!is_shell_location("C:"));
        assert!(!is_shell_location(r"\\server\share"));
        assert!(!is_shell_location(r"\\?\C:\Windows"));
        assert!(!is_shell_location(r"relative\path"));
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(
            file_url_to_path("file:///C:/My%20Files/a.txt").as_deref(),
            Some(r"C:\My Files\a.txt")
        );
        assert_eq!(file_url_to_path("file://localhost/D:/x").as_deref(), Some(r"D:\x"));
        assert_eq!(
            file_url_to_path("file://server/share/x%2Fy").as_deref(),
            Some(r"\\server\share\x\y")
        );
        assert_eq!(file_url_to_path("file:///C:/a?x=1#top").as_deref(), Some(r"C:\a"));
        // Not a drive path, and a bad escape stays as written.
        assert_eq!(file_url_to_path("file:///etc/passwd"), None);
        assert_eq!(percent_decode("100%zz%C3%A9"), "100%zz\u{e9}");
    }

    #[test]
    fn flags_are_read_and_never_opened() {
        let cwd = Path::new("C:\\");
        let request = Request::parse(["--tray".to_string(), "--Quick-Search".to_string()], cwd);
        assert!(request.tray && request.quick_search && request.open.is_empty() && request.explorer.is_empty());
        assert!(asks_for_quick_search(["a", " --QUICK-SEARCH "].into_iter()));
        assert!(!asks_for_quick_search(["a", "--tray"].into_iter()));
        assert!(!Request::parse(Vec::new(), cwd).tray);
    }

    /// Waits for the next request on an endpoint, for up to five seconds.
    fn next(endpoint: &Endpoint) -> Option<Request> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(request) = endpoint.incoming.try_recv() {
                return Some(request);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn unique(test: &str) -> String {
        format!("test-{}-{test}", std::process::id())
    }

    #[test]
    fn a_later_launch_reaches_the_first_one_and_the_name_frees_up() {
        let name = unique("roundtrip");
        let Some(Claim::First(first)) = claim(&name) else {
            panic!("the first claim should win")
        };
        assert!(matches!(claim(&name), Some(Claim::Later)), "a second claim must lose");

        let request = Request {
            open: vec![
                Target {
                    path: r"C:\Users\Test\Documents".into(),
                    select: None,
                },
                Target {
                    path: r"C:\Windows".into(),
                    select: Some(r"C:\Windows\notepad.exe".into()),
                },
                Target {
                    path: "C:\\Users\\\u{645}\u{631}\u{62d}\u{628}\u{627}".into(),
                    select: None,
                },
            ],
            quick_search: true,
            ..Request::default()
        };
        assert!(forward(&name, &request, Duration::from_secs(2)));
        assert_eq!(next(&first).as_ref(), Some(&request));
        // Requests keep their order.
        for n in 0..3 {
            let numbered = Request {
                open: vec![Target {
                    path: format!("C:\\n{n}"),
                    select: None,
                }],
                ..Request::default()
            };
            assert!(forward(&name, &numbered, Duration::from_secs(2)));
        }
        let got: Vec<_> = (0..3)
            .filter_map(|_| next(&first))
            .map(|r| r.open[0].path.clone())
            .collect();
        assert_eq!(got, ["C:\\n0", "C:\\n1", "C:\\n2"]);

        // Nobody listens under another name: forwarding gives up when its patience runs out.
        let started = Instant::now();
        assert!(!forward(&unique("nobody"), &request, Duration::from_millis(150)));
        assert!(started.elapsed() < Duration::from_secs(2));

        // When the first copy ends, the name is free again.
        drop(first);
        assert!(matches!(claim(&name), Some(Claim::First(_))));
    }

    #[test]
    fn begin_forwards_a_later_launch_and_exits() {
        let name = unique("begin");
        let request = Request {
            open: vec![Target {
                path: r"C:\Windows".into(),
                select: None,
            }],
            ..Request::default()
        };
        // The first copy runs and keeps the endpoint.
        assert_eq!(
            begin_with(&name, request.clone(), Duration::from_secs(1)),
            Startup::Run(request.clone())
        );
        // A later one is forwarded (without what it already gave File Explorer) and ends.
        let with_explorer = Request {
            explorer: vec!["shell:Libraries".into()],
            ..request.clone()
        };
        assert_eq!(begin_with(&name, with_explorer, Duration::from_secs(2)), Startup::Exit);
        let arrived = lock(&INSTANCE).as_ref().and_then(next);
        assert_eq!(arrived, Some(request));
        *lock(&INSTANCE) = None;
    }

    #[test]
    fn garbage_sent_to_the_listener_is_refused() {
        let name = unique("garbage");
        let Some(Claim::First(first)) = claim(&name) else {
            panic!("the first claim should win")
        };
        let window = find_listener(&name).expect("the listener window exists once claim returns");
        assert!(!send_copydata(window, COPYDATA_ID, b"not json"));
        assert!(!send_copydata(window, COPYDATA_ID + 1, b"{}"));
        assert!(
            send_copydata(window, COPYDATA_ID, b"{}"),
            "an empty request is a valid plain relaunch"
        );
        assert_eq!(next(&first), Some(Request::default()));
    }
}
