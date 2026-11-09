//! Ported from src-tauri/src/native_preview.rs (and phase 0's owned pop-up, docs/gpui/reference/spike-phase0.rs).
//!
//! The preview pane's view of files Glint can't show itself (Word, Excel and PowerPoint documents,
//! fonts, Visio drawings…): the preview handler Windows has for the type draws the file, as in File
//! Explorer. Handlers run out of process (prevhost.exe).
//!
//! Where the old app put the handler's window inside the web view's window, here it can't: GPUI's window
//! is a DirectComposition window (`WS_EX_NOREDIRECTIONBITMAP`), and GDI-drawn handlers (Windows' text
//! previewer, Office's) are invisible as its child windows. So every handler is hosted in an **owned,
//! never-activated pop-up** kept over the pane's rectangle in screen coordinates. The pop-up follows the
//! GPUI window (a window subclass sees it move, resize, minimize, restore, change DPI) and is hidden
//! while something covers the pane (menus, dialogs, the palette: [`hide`] and [`reveal`]).
//!
//! Like File Explorer, it keeps away from files downloaded from the internet (previewing one can leak
//! credentials), and from everything when Glint runs as administrator.
//!
//! **Threads.** Everything here is for the UI thread. The COM calls into a handler pump messages while
//! they wait, so they never run inside a GPUI callback: [`show`] and friends record what is wanted and
//! post a job (`core_bridge::post_to_ui`), which does the work from the message pump. A call into a
//! handler that hangs is cancelled by a watchdog thread after 15 seconds, and that handler isn't used
//! again this session. (The old app ran a thread of its own for this; the task for this module asks for
//! the UI thread, so the watchdog is what keeps a hung handler from freezing input for good.)
//!
//! **Wiring.** Call [`init`] once at start-up on the UI thread (it registers core's release hook, so a
//! file a handler holds is let go before core renames, moves or recycles it). A view then calls
//! [`show`] with the `Window`, the pane's bounds in logical pixels and the path, keeps the pane's bounds
//! current with [`set_rect`], covers it with [`hide`] and [`reveal`], and ends with [`unload`].

use crate::core_bridge;
use crate::t;
use gpui_kit::{Bounds, Pixels, Window};
use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Mutex, Once};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::core::{implement, w, ComObject, Interface, Result as WinResult, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, RPC_E_CALL_CANCELED, RPC_E_DISCONNECTED, S_FALSE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, CreateSolidBrush, DeleteObject, FillRect, HDC};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{
    CoCancelCall, CoCreateInstance, CoEnableCallCancellation, CoInitializeEx, IBindCtx, IStream, CLSCTX_LOCAL_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, COINIT_MULTITHREADED, STGM_READ, STGM_SHARE_DENY_NONE,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::IObjectWithSite;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    AssocQueryStringW, DefSubclassProc, IInitializeWithItem, IPreviewHandler, IPreviewHandlerFrame,
    IPreviewHandlerFrame_Impl, IPreviewHandlerVisuals, IShellItem, IsUserAnAdmin, RemoveWindowSubclass,
    SHCreateItemFromParsingName, SHCreateStreamOnFileEx, SetWindowSubclass, ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION,
    PREVIEWHANDLERFRAMEINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, IsIconic, IsWindow,
    IsWindowVisible, LoadCursorW, RegisterClassW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_USERDATA,
    IDC_ARROW, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, WM_DPICHANGED,
    WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_MOVE, WM_NCDESTROY, WM_SHOWWINDOW, WM_SIZE, WM_WINDOWPOSCHANGED, WNDCLASSW,
    WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// The shellex key a file type's preview handler is registered under (IPreviewHandler's IID).
const PREVIEW_HANDLER: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
const CLASS: PCWSTR = w!("GlintPreviewHost");
/// The subclass id on the GPUI window ("prev" in ASCII), so other subclasses of it don't clash.
const SUBCLASS_ID: usize = 0x7072_6576;
/// A handler that takes longer than this in one call is stopped, and not used again this session.
const HUNG: Duration = Duration::from_secs(15);
/// HTML and mail previewers run a browser engine on the file: Glint's own preview shows those.
const SKIPPED: [&str; 7] = ["htm", "html", "xhtml", "mht", "mhtml", "eml", "svg"];

// ---- The public face ------------------------------------------------------------------------------

/// What happened to a preview, sent on the channel [`show`] returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The preview is on screen (or will be as soon as it's placed).
    Ready,
    /// No preview, and why (empty when the type simply has no previewer).
    Unavailable { reason: String },
    /// Glint let the file go because it is about to rename, move or recycle it; the preview is gone.
    Released,
}

/// Why [`show`] couldn't even start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewError {
    /// The GPUI window has no Win32 handle (yet).
    NoWindow,
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreviewError::NoWindow => f.write_str("the window has no handle yet"),
        }
    }
}

impl std::error::Error for PreviewError {}

/// Call once at start-up, on the UI thread: prepares COM call cancellation and the watchdog, and
/// registers core's release hook so a file a handler holds is let go before core renames, moves or
/// recycles it. Safe to call again.
pub fn init() {
    ui_ready();
    static HOOK: Once = Once::new();
    HOOK.call_once(|| glint_core::host::previews::set_release_hook(release));
}

/// Shows `path` over `bounds` (the pane's rectangle in the window's client area, in logical pixels; None
/// keeps it hidden until [`set_rect`]) for the claimant `claimant` (the preview pane or Quick Look: the
/// newest call wins). `dark` picks the handler's background and text colors. Call from the UI thread.
///
/// Returns at once: the handler is created from a posted job. The receiver gets [`Event::Ready`] or
/// [`Event::Unavailable`], and [`Event::Released`] later if core needs the file.
pub fn show(
    window: &Window,
    claimant: u32,
    bounds: Option<Bounds<Pixels>>,
    path: &str,
    dark: bool,
) -> Result<async_channel::Receiver<Event>, PreviewError> {
    let owner = core_bridge::hwnd_of(window).ok_or(PreviewError::NoWindow)?;
    init();
    install_subclass(owner);
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    HOST.with(|host| {
        host.borrow_mut().wanted = Wanted {
            claimant,
            rect: bounds.map(Logical::from_bounds),
            covered: false,
        }
    });
    let (reply, events) = async_channel::unbounded();
    let path = path.to_string();
    let owner = owner.0 as isize;
    // Known to core's release hook from now on, not only once the handler has it: a rename asked for while
    // the open job waits must wait for it too.
    lock(&PENDING_PATHS).push(path.clone());
    core_bridge::post_to_ui(move || {
        serialized(move || {
            let pending = path.clone();
            open_job(generation, claimant, owner, path, dark, reply);
            let mut paths = lock(&PENDING_PATHS);
            if let Some(i) = paths.iter().position(|p| *p == pending) {
                paths.swap_remove(i);
            }
        })
    });
    Ok(events)
}

/// Moves and resizes the preview to `bounds` (logical pixels in the window's client area) when the pane
/// moves or changes size. Cheap to call every frame: nothing happens unless the rectangle changed.
pub fn set_rect(claimant: u32, bounds: Bounds<Pixels>) {
    let rect = Logical::from_bounds(bounds);
    let changed = HOST.with(|host| {
        let mut host = host.borrow_mut();
        if host.wanted.claimant != claimant || host.wanted.rect == Some(rect) {
            return false;
        }
        host.wanted.rect = Some(rect);
        true
    });
    if changed {
        post_apply();
    }
}

/// Hides the preview while something covers the pane (a menu, a dialog, the palette). [`reveal`] undoes it.
pub fn hide(claimant: u32) {
    set_covered(claimant, true);
}

/// Shows the preview again after [`hide`].
pub fn reveal(claimant: u32) {
    set_covered(claimant, false);
}

/// Ends the preview of `claimant`'s (unless another claimant took over): the handler is unloaded and its
/// window destroyed. Nothing is waited for.
pub fn unload(claimant: u32) {
    let ours = HOST.with(|host| {
        let mut host = host.borrow_mut();
        let ours = host.wanted.claimant == claimant;
        if ours {
            host.wanted.rect = None;
            // A show still waiting to open is superseded.
            GENERATION.fetch_add(1, Ordering::SeqCst);
        }
        ours
    });
    // Out of sight now; the handler is unloaded from the posted job.
    if ours {
        follow();
    }
    core_bridge::post_to_ui(move || serialized(move || close_job(claimant)));
}

/// Whether Windows has a preview handler this module would use for a file like `path`.
pub fn has_handler(path: &str) -> bool {
    handler_for(path).is_some()
}

fn set_covered(claimant: u32, covered: bool) {
    let changed = HOST.with(|host| {
        let mut host = host.borrow_mut();
        if host.wanted.claimant != claimant || host.wanted.covered == covered {
            return false;
        }
        host.wanted.covered = covered;
        true
    });
    if changed {
        // Hiding is immediate and needs no COM; showing again goes through the normal placement.
        if covered {
            follow();
        } else {
            post_apply();
        }
    }
}

/// Before Glint moves, renames or deletes `paths`: a handler may hold one of them open. Called by core
/// from any thread; waits (briefly) for the UI thread to let go.
fn release(paths: &[String]) {
    let named = |held: &str| paths.iter().any(|p| p.eq_ignore_ascii_case(held));
    let holds = lock(&SHOWN_PATH).as_deref().is_some_and(named) || lock(&PENDING_PATHS).iter().any(|p| named(p));
    if !holds {
        return;
    }
    if UI_THREAD.load(Ordering::Relaxed) == current_thread() {
        // Already on the UI thread: nobody else can run the job, so don't wait for it.
        let paths = paths.to_vec();
        serialized(move || release_job(&paths));
        return;
    }
    let (done, wait) = channel();
    let paths = paths.to_vec();
    // `done` goes when the job has run, not when it was queued: behind an open that is inside a handler call
    // it runs later, and core must not touch the file before.
    core_bridge::post_to_ui(move || {
        serialized(move || {
            release_job(&paths);
            let _ = done.send(());
        })
    });
    // A handler that hangs on Unload mustn't hold up the file operation for long.
    let _ = wait.recv_timeout(Duration::from_secs(3));
}

// ---- State (UI thread only) -----------------------------------------------------------------------

/// A rectangle in the window's client area in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Logical {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl Logical {
    fn from_bounds(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        }
    }

    /// The rectangle in screen coordinates, at the window's current scale (client logical px times DPI / 96).
    fn on_screen(self, owner: HWND) -> RECT {
        // SAFETY: GetDpiForWindow takes any handle and returns 0 for one that isn't a window.
        let dpi = unsafe { GetDpiForWindow(owner) };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        // Both edges are rounded, not the size, so neighbouring rectangles still meet without a gap.
        let edge = |v: f32| (v * scale).round() as i32;
        let (left, top) = (edge(self.x), edge(self.y));
        let (right, bottom) = (edge(self.x + self.width), edge(self.y + self.height));
        let mut origin = POINT { x: left, y: top };
        // SAFETY: `origin` is a valid POINT; a bad window handle makes the call fail and leaves it as is.
        unsafe {
            let _ = ClientToScreen(owner, &mut origin);
        }
        RECT {
            left: origin.x,
            top: origin.y,
            right: origin.x + (right - left).max(1),
            bottom: origin.y + (bottom - top).max(1),
        }
    }
}

/// What the views asked for, kept even while the handler is still opening.
#[derive(Clone, Copy)]
struct Wanted {
    claimant: u32,
    rect: Option<Logical>,
    covered: bool,
}

/// The preview on screen.
struct Shown {
    claimant: u32,
    path: String,
    clsid: GUID,
    handler: IPreviewHandler,
    popup: HWND,
    owner: HWND,
    /// The size last given to the handler, in physical pixels.
    size: (i32, i32),
    reply: async_channel::Sender<Event>,
}

struct Host {
    wanted: Wanted,
    shown: Option<Shown>,
    /// The pop-up and the window it is owned by, while a preview has one (the placement needs no more).
    tracked: Option<(HWND, HWND)>,
    /// Handlers that hung or failed this session: not tried again.
    bad: Vec<GUID>,
}

thread_local! {
    static HOST: RefCell<Host> = const { RefCell::new(Host {
        wanted: Wanted { claimant: 0, rect: None, covered: false },
        shown: None,
        tracked: None,
        bad: Vec::new(),
    }) };
    /// Jobs that arrived while another was inside a handler call (which pumps messages).
    static QUEUE: RefCell<VecDeque<Box<dyn FnOnce()>>> = const { RefCell::new(VecDeque::new()) };
    static RUNNING: Cell<bool> = const { Cell::new(false) };
    static SUBCLASSED: RefCell<HashSet<isize>> = RefCell::new(HashSet::new());
    static APPLY_QUEUED: Cell<bool> = const { Cell::new(false) };
}

/// Bumped by every show and unload, so a show that has been overtaken is not made at all (arrowing through
/// files asks for many previews and wants the last).
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The UI thread's id, set by [`init`]; 0 before.
static UI_THREAD: AtomicU32 = AtomicU32::new(0);
/// When a call into a handler started (ms since the epoch), 0 when none is running.
static BUSY: AtomicU64 = AtomicU64::new(0);
/// The file on screen, for core's release hook, which runs on other threads.
static SHOWN_PATH: Mutex<Option<String>> = Mutex::new(None);
/// Files asked for whose open job hasn't finished yet (one entry per show in flight).
static PENDING_PATHS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn current_thread() -> u32 {
    // SAFETY: no arguments, no failure.
    unsafe { GetCurrentThreadId() }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Runs `job` now, or after the job running now if that one is inside a handler call: those pump messages,
/// and a second job starting in the middle of the first would see a half-made preview.
fn serialized(job: impl FnOnce() + 'static) {
    if RUNNING.with(Cell::get) {
        QUEUE.with(|q| q.borrow_mut().push_back(Box::new(job)));
        return;
    }
    /// Clears RUNNING however the jobs end: a job that panicked must not leave every later one queued.
    struct Running;
    impl Drop for Running {
        fn drop(&mut self) {
            RUNNING.with(|r| r.set(false));
        }
    }
    RUNNING.with(|r| r.set(true));
    let _running = Running;
    job();
    while let Some(next) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
        next();
    }
}

/// Makes this thread ready to talk to handlers: COM, call cancellation, the window class, the watchdog.
fn ui_ready() {
    thread_local! {
        static READY: Cell<bool> = const { Cell::new(false) };
    }
    if READY.with(Cell::get) {
        return;
    }
    READY.with(|r| r.set(true));
    let thread = current_thread();
    UI_THREAD.store(thread, Ordering::Relaxed);
    // SAFETY: plain COM and window-class setup on this thread. GPUI has usually initialised COM already
    // (a changed-mode error then is fine: we keep whatever apartment it chose).
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        // Lets the watchdog cancel a call that hangs.
        let _ = CoEnableCallCancellation(None);
        register_class();
    }
    static WATCHDOG: Once = Once::new();
    WATCHDOG.call_once(|| {
        std::thread::spawn(move || watchdog(thread));
    });
}

/// Cancels a call into a handler that hangs, so the UI thread comes back.
fn watchdog(thread: u32) {
    // SAFETY: COM setup on the watchdog's own thread.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let since = BUSY.load(Ordering::Relaxed);
        if since != 0 && now_ms().saturating_sub(since) > u64::try_from(HUNG.as_millis()).unwrap_or(u64::MAX) {
            BUSY.store(0, Ordering::Relaxed);
            // SAFETY: cancelling is allowed from any thread; the target enabled cancellation in `ui_ready`.
            unsafe {
                let _ = CoCancelCall(thread, 0);
            }
        }
    }
}

/// A call into the handler, watched by the watchdog.
fn watched<T>(call: impl FnOnce() -> WinResult<T>) -> WinResult<T> {
    BUSY.store(now_ms(), Ordering::Relaxed);
    let result = call();
    BUSY.store(0, Ordering::Relaxed);
    result
}

/// The watchdog cancelled the call, or the handler's process went away: that handler isn't used again.
fn broken<T>(result: &WinResult<T>) -> bool {
    result
        .as_ref()
        .is_err_and(|e| e.code() == RPC_E_CALL_CANCELED || e.code() == RPC_E_DISCONNECTED)
}

// ---- Finding the handler --------------------------------------------------------------------------

/// The preview handler for files like `path`: by extension, ProgID, SystemFileAssociations and perceived
/// type, as File Explorer looks it up.
fn handler_for(path: &str) -> Option<GUID> {
    let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_lowercase();
    if SKIPPED.contains(&ext.as_str()) {
        return None;
    }
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` and `len` live through the call, and `len` is its capacity in characters.
    unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
            ASSOCSTR_SHELLEXTENSION,
            &HSTRING::from(format!(".{ext}")),
            &HSTRING::from(PREVIEW_HANDLER),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    }
    .ok()
    .ok()?;
    // `len` counts the terminating zero; it is never trusted past the buffer.
    let used = (len as usize).saturating_sub(1).min(buf.len());
    let text = String::from_utf16_lossy(buf.get(..used)?);
    GUID::try_from(text.trim_matches(['{', '}'])).ok()
}

// ---- Handler site ---------------------------------------------------------------------------------

/// The host's side of the conversation: no accelerators of its own, no keys taken back.
#[implement(IPreviewHandlerFrame)]
struct Frame;

impl IPreviewHandlerFrame_Impl for Frame_Impl {
    fn GetWindowContext(&self) -> WinResult<PREVIEWHANDLERFRAMEINFO> {
        Ok(PREVIEWHANDLERFRAMEINFO::default())
    }
    fn TranslateAccelerator(&self, _msg: *const windows::Win32::UI::WindowsAndMessaging::MSG) -> WinResult<()> {
        Err(S_FALSE.into())
    }
}

// ---- The pop-up -----------------------------------------------------------------------------------

/// Registers the pop-up's window class. Registering twice fails harmlessly.
///
/// # Safety
/// Must run on a thread that may create windows.
unsafe fn register_class() {
    // SAFETY: the class record is valid for the call; the window procedure is an `extern "system"` fn.
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSW {
            lpfnWndProc: Some(popup_proc),
            hInstance: module.into(),
            lpszClassName: CLASS,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&class);
    }
}

/// The pop-up never takes the activation from the GPUI window, and paints the pane's color until the
/// handler draws.
unsafe extern "system" fn popup_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: a window procedure called by the system with its own window; the GDI objects made here are
    // released here.
    unsafe {
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        if msg == WM_ERASEBKGND {
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let brush = CreateSolidBrush(COLORREF(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as u32));
            FillRect(HDC(wparam.0 as _), &client, brush);
            let _ = DeleteObject(brush.into());
            return LRESULT(1);
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

/// An owned, never-activated pop-up at `at` (screen coordinates), hidden until placed. `dark` is the
/// pane's background color.
fn create_popup(owner: HWND, at: RECT, dark: bool) -> Result<HWND, String> {
    // SAFETY: plain window creation on the UI thread with the class registered by `ui_ready`.
    unsafe {
        register_class();
        let module = GetModuleHandleW(None).unwrap_or_default();
        let popup = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            CLASS,
            PCWSTR::null(),
            WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            at.left,
            at.top,
            (at.right - at.left).max(1),
            (at.bottom - at.top).max(1),
            Some(owner),
            None,
            Some(module.into()),
            None,
        )
        .map_err(|e| e.message())?;
        // The pane's background (COLORREF is 0x00BBGGRR).
        SetWindowLongPtrW(popup, GWLP_USERDATA, if dark { 0x001C_1C1C } else { 0x00FF_FFFF });
        Ok(popup)
    }
}

/// Puts the pop-up at `at` (screen) and shows or hides it. `resize` false only moves it.
fn place_popup(popup: HWND, at: RECT, visible: bool, resize: bool) {
    // SAFETY: the handles are ours; a destroyed one just makes the calls fail.
    unsafe {
        if !visible {
            let _ = ShowWindow(popup, SW_HIDE);
            return;
        }
        let mut flags = SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW;
        if !resize {
            flags |= SWP_NOSIZE;
        }
        let _ = SetWindowPos(
            popup,
            None,
            at.left,
            at.top,
            at.right - at.left,
            at.bottom - at.top,
            flags,
        );
    }
}

/// Whether the pop-up may be on screen: something wanted it, nothing covers it, and the window is up.
fn may_show(wanted: &Wanted, owner: HWND) -> bool {
    // SAFETY: queries on a handle that may be gone, which they report as false.
    let up =
        unsafe { IsWindow(Some(owner)).as_bool() && IsWindowVisible(owner).as_bool() && !IsIconic(owner).as_bool() };
    up && wanted.rect.is_some() && !wanted.covered
}

/// Keeps the pop-up over its pane: called when the window moves, resizes, minimizes or restores. Only
/// moves and shows or hides; the size changes with the pane (`apply`).
fn follow() {
    let target = HOST.with(|host| {
        let host = host.borrow();
        let (popup, owner) = host.tracked?;
        Some((popup, owner, host.wanted.rect, may_show(&host.wanted, owner)))
    });
    let Some((popup, owner, rect, visible)) = target else {
        return;
    };
    let at = rect.map_or(RECT::default(), |r| r.on_screen(owner));
    place_popup(popup, at, visible, false);
}

fn post_apply() {
    if APPLY_QUEUED.with(|q| q.replace(true)) {
        return;
    }
    core_bridge::post_to_ui(|| {
        APPLY_QUEUED.with(|q| q.set(false));
        serialized(apply);
    });
}

/// Moves the pop-up to where the pane is now, and tells the handler when its size changed.
fn apply() {
    let target = HOST.with(|host| {
        let host = host.borrow();
        let (popup, owner) = host.tracked?;
        let shown = host.shown.as_ref();
        Some((
            popup,
            owner,
            host.wanted,
            shown.map(|s| s.size),
            shown.map(|s| s.handler.clone()),
        ))
    });
    let Some((popup, owner, wanted, size, handler)) = target else {
        return;
    };
    let Some(rect) = wanted.rect else {
        place_popup(popup, RECT::default(), false, false);
        return;
    };
    let at = rect.on_screen(owner);
    let now = (at.right - at.left, at.bottom - at.top);
    place_popup(popup, at, may_show(&wanted, owner), true);
    if let (Some(handler), true) = (handler, size.is_some_and(|size| size != now)) {
        let inside = RECT {
            left: 0,
            top: 0,
            right: now.0,
            bottom: now.1,
        };
        // No borrow is held: the call pumps messages, which may come back into this module.
        let _ = watched(|| {
            // SAFETY: `handler` is a live interface on this thread's apartment.
            unsafe { handler.SetRect(&inside) }
        });
        HOST.with(|host| {
            if let Some(shown) = host.borrow_mut().shown.as_mut().filter(|s| s.popup == popup) {
                shown.size = now;
            }
        });
    }
}

// ---- Following the window -------------------------------------------------------------------------

/// Subclasses the GPUI window (once) so the pop-up follows it.
fn install_subclass(owner: HWND) {
    let first = SUBCLASSED.with(|s| s.borrow_mut().insert(owner.0 as isize));
    if first {
        // SAFETY: called on the thread that owns the window; the procedure is an `extern "system"` fn.
        let ok = unsafe { SetWindowSubclass(owner, Some(subclass_proc), SUBCLASS_ID, 0) };
        if !ok.as_bool() {
            SUBCLASSED.with(|s| s.borrow_mut().remove(&(owner.0 as isize)));
        }
    }
}

unsafe extern "system" fn subclass_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: a subclass procedure called by the system for our window; it only forwards.
    unsafe {
        if msg == WM_NCDESTROY {
            // The window is going: the pop-up (owned by it) goes with it.
            serialized_close_now(window);
            SUBCLASSED.with(|s| s.borrow_mut().remove(&(window.0 as isize)));
            let _ = RemoveWindowSubclass(window, Some(subclass_proc), SUBCLASS_ID);
            return DefSubclassProc(window, msg, wparam, lparam);
        }
        let result = DefSubclassProc(window, msg, wparam, lparam);
        // GPUI has handled the message; now the pop-up follows. Minimizing hides it with its owner.
        if matches!(
            msg,
            WM_WINDOWPOSCHANGED | WM_MOVE | WM_SIZE | WM_SHOWWINDOW | WM_DPICHANGED
        ) {
            follow();
        }
        result
    }
}

/// The owner is being destroyed: drop what we hold without calling the handler (the apartment may be
/// going too), and destroy the pop-up.
fn serialized_close_now(owner: HWND) {
    let shown = HOST.with(|host| {
        let mut host = host.borrow_mut();
        if host.shown.as_ref().is_some_and(|s| s.owner == owner) {
            host.tracked = None;
            host.shown.take()
        } else {
            None
        }
    });
    if let Some(shown) = shown {
        *SHOWN_PATH.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // SAFETY: the pop-up is ours.
        unsafe {
            let _ = DestroyWindow(shown.popup);
        }
        let _ = shown.reply.try_send(Event::Released);
    }
}

// ---- The jobs -------------------------------------------------------------------------------------

fn open_job(
    generation: u64,
    claimant: u32,
    owner: isize,
    path: String,
    dark: bool,
    reply: async_channel::Sender<Event>,
) {
    let unavailable = |reason: String| {
        let _ = reply.try_send(Event::Unavailable { reason });
    };
    if GENERATION.load(Ordering::SeqCst) != generation {
        // Overtaken by a later show or an unload.
        return unavailable(String::new());
    }
    // The same file for another claimant (Quick Look closing): it moves.
    let same = HOST.with(|host| {
        let mut host = host.borrow_mut();
        match host.shown.as_mut().filter(|s| s.path.eq_ignore_ascii_case(&path)) {
            Some(shown) => {
                shown.claimant = claimant;
                shown.reply = reply.clone();
                true
            }
            None => false,
        }
    });
    if same {
        apply();
        let _ = reply.try_send(Event::Ready);
        return;
    }
    close_current(false);
    let owner = HWND(owner as *mut _);
    match open(claimant, &path, owner, dark, &reply) {
        Ok(()) => {
            // A handler can take the keyboard (Office does): the list keeps it.
            // SAFETY: focus moves to our own window on our own thread.
            unsafe {
                let _ = SetFocus(Some(owner));
            }
            apply();
            let _ = reply.try_send(Event::Ready);
        }
        Err(reason) => unavailable(reason),
    }
}

fn close_job(claimant: u32) {
    let ours = HOST.with(|host| host.borrow().shown.as_ref().is_some_and(|s| s.claimant == claimant));
    if ours {
        close_current(false);
    }
}

fn release_job(paths: &[String]) {
    let holds = HOST.with(|host| {
        host.borrow()
            .shown
            .as_ref()
            .map(|s| paths.iter().any(|p| p.eq_ignore_ascii_case(&s.path)))
    });
    if holds == Some(true) {
        close_current(true);
    }
}

/// Why `path` gets no preview (empty when the type simply has none), or the preview made, hidden until
/// `apply` places it.
fn open(
    claimant: u32,
    path: &str,
    owner: HWND,
    dark: bool,
    reply: &async_channel::Sender<Event>,
) -> Result<(), String> {
    // SAFETY: a plain query.
    if unsafe { IsUserAnAdmin().as_bool() } {
        return Err(t!("Windows' previewers don't run while Glint runs as administrator.").to_string());
    }
    if glint_core::xray::zone_of(path).is_some_and(|zone| zone >= 3) {
        return Err(t!(
            "Downloaded from the internet: like File Explorer, Glint doesn't preview these. Open it to see it."
        )
        .to_string());
    }
    let Some(clsid) = handler_for(path) else {
        return Err(String::new());
    };
    if HOST.with(|host| host.borrow().bad.contains(&clsid)) {
        return Err(t!("This kind of file's previewer stopped responding. Open it to see it.").to_string());
    }
    let wanted = HOST.with(|host| host.borrow().wanted);
    let at = wanted.rect.map_or(
        RECT {
            left: 0,
            top: 0,
            right: 1,
            bottom: 1,
        },
        |r| r.on_screen(owner),
    );
    let (width, height) = ((at.right - at.left).max(1), (at.bottom - at.top).max(1));
    let popup = create_popup(owner, at, dark)?;
    let inside = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    let handler = watched(|| {
        // SAFETY: `popup` is a live window of this thread.
        unsafe { preview(&clsid, path, popup, inside, dark) }
    });
    if broken(&handler) {
        HOST.with(|host| host.borrow_mut().bad.push(clsid));
    }
    let handler = match handler {
        Ok(handler) => handler,
        Err(e) => {
            // SAFETY: the pop-up is ours.
            unsafe {
                let _ = DestroyWindow(popup);
            }
            return Err(t!("Windows' previewer couldn't show it: {reason}", reason = e.message()).to_string());
        }
    };
    HOST.with(|host| {
        let mut host = host.borrow_mut();
        host.tracked = Some((popup, owner));
        host.shown = Some(Shown {
            claimant,
            path: path.to_string(),
            clsid,
            handler,
            popup,
            owner,
            size: (width, height),
            reply: reply.clone(),
        })
    });
    *SHOWN_PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.to_string());
    Ok(())
}

/// Unloads the handler and destroys its pop-up. `released` tells the view the file was let go for core.
fn close_current(released: bool) {
    let Some(shown) = HOST.with(|host| {
        let mut host = host.borrow_mut();
        host.tracked = None;
        host.shown.take()
    }) else {
        return;
    };
    *SHOWN_PATH.lock().unwrap_or_else(|e| e.into_inner()) = None;
    // SAFETY: the pop-up is ours.
    unsafe {
        let _ = ShowWindow(shown.popup, SW_HIDE);
    }
    // No borrow is held during the calls: they pump messages.
    let unloaded = watched(|| {
        // SAFETY: a live handler interface on this thread's apartment.
        unsafe { shown.handler.Unload() }
    });
    if broken(&unloaded) {
        HOST.with(|host| host.borrow_mut().bad.push(shown.clsid));
    }
    if let Ok(site) = shown.handler.cast::<IObjectWithSite>() {
        // SAFETY: same interface, same thread; clearing the site lets the handler drop our frame.
        unsafe {
            let _ = site.SetSite(None);
        }
    }
    drop(shown.handler);
    // SAFETY: the pop-up is ours.
    unsafe {
        let _ = DestroyWindow(shown.popup);
    }
    if released {
        let _ = shown.reply.try_send(Event::Released);
    }
}

/// Creates the handler out of process, gives it the file (as a stream, a path or a shell item, whichever
/// it takes) and has it draw in `host`.
///
/// # Safety
/// `host` must be a live window of the calling thread.
unsafe fn preview(clsid: &GUID, path: &str, host: HWND, inside: RECT, dark: bool) -> WinResult<IPreviewHandler> {
    // SAFETY: COM calls on this thread's apartment with interfaces this function made.
    unsafe {
        let handler: IPreviewHandler = CoCreateInstance(clsid, None, CLSCTX_LOCAL_SERVER)?;
        if let Ok(init) = handler.cast::<IInitializeWithStream>() {
            // Shared every way: the file can still be renamed, deleted or written while it shows.
            let stream: IStream = SHCreateStreamOnFileEx(
                &HSTRING::from(path),
                (STGM_READ | STGM_SHARE_DENY_NONE).0,
                FILE_ATTRIBUTE_NORMAL.0,
                false,
                None::<&IStream>,
            )?;
            init.Initialize(&stream, STGM_READ.0)?;
        } else if let Ok(init) = handler.cast::<IInitializeWithFile>() {
            init.Initialize(&HSTRING::from(path), STGM_READ.0)?;
        } else {
            let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>)?;
            handler.cast::<IInitializeWithItem>()?.Initialize(&item, STGM_READ.0)?;
        }
        if let Ok(site) = handler.cast::<IObjectWithSite>() {
            site.SetSite(&ComObject::new(Frame).to_interface::<IPreviewHandlerFrame>())?;
        }
        if let (true, Ok(visuals)) = (dark, handler.cast::<IPreviewHandlerVisuals>()) {
            let _ = visuals.SetBackgroundColor(COLORREF(0x001C_1C1C));
            let _ = visuals.SetTextColor(COLORREF(0x00F2_F2F2));
        }
        handler.SetWindow(host, &inside)?;
        handler.DoPreview()?;
        handler.SetRect(&inside)?;
        Ok(handler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, GetWindowRect, PeekMessageW, SetWindowPos as MoveWindow, TranslateMessage, MSG, PM_REMOVE,
        SWP_NOACTIVATE as NOACTIVATE, SWP_NOZORDER as NOZORDER, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    /// Runs the messages queued for this thread, as GPUI's loop would.
    fn pump() {
        // SAFETY: the standard message loop body, on the thread that owns the windows.
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn rect_of(window: HWND) -> RECT {
        let mut r = RECT::default();
        // SAFETY: a live window and a valid out-pointer.
        unsafe {
            let _ = GetWindowRect(window, &mut r);
        }
        r
    }

    #[test]
    fn text_files_have_a_windows_previewer() {
        assert!(has_handler("C:\\notes\\readme.txt"));
        assert!(
            handler_for("C:\\notes\\README.TXT").is_some(),
            "the lookup ignores case"
        );
    }

    #[test]
    fn files_without_a_previewer_or_with_a_browser_engine_have_none() {
        assert!(!has_handler("C:\\notes\\no extension"));
        assert!(!has_handler("C:\\notes\\page.html"));
        assert!(!has_handler("C:\\notes\\mail.eml"));
        assert!(!has_handler("C:\\notes\\unknown.zzqqxx"));
    }

    #[test]
    fn pdf_lookup_does_not_fail_or_panic() {
        // Edge's PDF previewer is registered on most PCs but not all; either answer is fine, a crash is not.
        let _ = handler_for("C:\\docs\\report.pdf");
    }

    #[test]
    fn logical_rectangles_become_physical_screen_rectangles() {
        let owner = test_window();
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: a live window and a valid out-pointer.
        unsafe {
            let _ = ClientToScreen(owner, &mut origin);
        }
        let at = Logical {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        }
        .on_screen(owner);
        // SAFETY: a plain query.
        let dpi = unsafe { GetDpiForWindow(owner) }.max(96);
        let scale = dpi as f32 / 96.0;
        assert_eq!(at.left, origin.x + (10.0 * scale).round() as i32);
        assert_eq!(at.top, origin.y + (20.0 * scale).round() as i32);
        assert_eq!(
            at.right - at.left,
            ((110.0 * scale).round() - (10.0 * scale).round()) as i32
        );
        // SAFETY: the window is ours.
        unsafe {
            let _ = DestroyWindow(owner);
        }
    }

    #[test]
    fn the_subclass_moves_a_tracked_popup_with_its_owner() {
        let owner = test_window();
        install_subclass(owner);
        let pane = Logical {
            x: 25.0,
            y: 15.0,
            width: 160.0,
            height: 90.0,
        };
        let popup = create_popup(owner, pane.on_screen(owner), true).expect("a pop-up window");
        HOST.with(|host| {
            let mut host = host.borrow_mut();
            host.wanted = Wanted {
                claimant: 1,
                rect: Some(pane),
                covered: false,
            };
            host.tracked = Some((popup, owner));
        });
        follow();
        // SAFETY: a live window of this thread.
        assert!(
            unsafe { IsWindowVisible(popup).as_bool() },
            "the pop-up shows over its pane"
        );
        let first = rect_of(popup);
        assert_eq!(first, pane.on_screen(owner), "it starts over the pane");

        // SAFETY: windows of this thread.
        unsafe {
            let _ = MoveWindow(owner, None, 420, 310, 0, 0, NOACTIVATE | NOZORDER | SWP_NOSIZE);
        }
        pump();
        let moved = rect_of(popup);
        assert_eq!(
            moved,
            pane.on_screen(owner),
            "the pop-up sits over the pane after the owner moved"
        );
        assert_ne!(first.left, moved.left, "it really moved");
        assert_eq!(
            moved.right - moved.left,
            first.right - first.left,
            "following keeps the size"
        );

        // A cover hides it, revealing shows it again.
        HOST.with(|host| host.borrow_mut().wanted.covered = true);
        follow();
        // SAFETY: a live window of this thread.
        assert!(!unsafe { IsWindowVisible(popup).as_bool() }, "a cover hides it");
        HOST.with(|host| host.borrow_mut().wanted.covered = false);
        follow();
        // SAFETY: a live window of this thread.
        assert!(unsafe { IsWindowVisible(popup).as_bool() }, "revealing shows it again");

        // A hidden owner (as when minimized) hides the pop-up.
        // SAFETY: a live window of this thread.
        unsafe {
            let _ = ShowWindow(owner, SW_HIDE);
        }
        pump();
        // SAFETY: a live window of this thread.
        assert!(
            !unsafe { IsWindowVisible(popup).as_bool() },
            "a hidden owner hides the pop-up"
        );

        HOST.with(|host| host.borrow_mut().tracked = None);
        // SAFETY: the windows are ours.
        unsafe {
            let _ = DestroyWindow(popup);
            let _ = DestroyWindow(owner);
        }
    }

    fn test_window() -> HWND {
        // SAFETY: plain window creation on this test's thread, which pumps its own messages.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let module = GetModuleHandleW(None).unwrap_or_default();
            let class = w!("GlintPreviewHostTestOwner");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(popup_proc),
                hInstance: module.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            register_class();
            CreateWindowExW(
                Default::default(),
                class,
                w!("owner"),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                100,
                100,
                800,
                600,
                None,
                None,
                Some(module.into()),
                None,
            )
            .expect("a test window")
        }
    }

    #[test]
    fn the_pending_queue_keeps_jobs_in_order() {
        let order = std::rc::Rc::new(RefCell::new(Vec::new()));
        let (a, b, c) = (order.clone(), order.clone(), order.clone());
        serialized(move || {
            a.borrow_mut().push(1);
            // Arrives while the first is running (as when a handler call pumps messages).
            serialized(move || b.borrow_mut().push(3));
            c.borrow_mut().push(2);
        });
        assert_eq!(*order.borrow(), vec![1, 2, 3]);
    }

    #[test]
    fn release_ignores_paths_when_nothing_is_shown() {
        // No UI thread, no job posted, no wait.
        *SHOWN_PATH.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let started = std::time::Instant::now();
        release(&["C:\\a.txt".to_string()]);
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
