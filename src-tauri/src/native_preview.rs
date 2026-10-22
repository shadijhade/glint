//! The preview pane's view of files Glint can't show itself (Word, Excel and PowerPoint documents,
//! fonts, Visio drawings…): the preview handler Windows has for the type draws it in a window of its
//! own over the pane, as in File Explorer. Handlers run out of process; a thread of this process with
//! its own COM apartment owns that window and talks to the handler, so the UI thread never waits on
//! one. WebView2 would draw over the window, so the web view gets a window region with the preview's
//! rectangle cut out while it shows.
//!
//! Like File Explorer, it keeps away from files downloaded from the internet (previewing one can leak
//! credentials), and from everything when Glint runs as administrator.

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::AppHandle;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// The preview is on screen.
    Ready,
    /// No preview, and why (empty when the type simply has no previewer).
    Unavailable { reason: String },
}

/// Where the preview goes, in the window's client area, in physical pixels.
#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Shows `path` over `rect`, for the claimant `id` (a preview in the pane or Quick Look; the newest
/// call wins). Replies on `on_event`.
#[tauri::command]
pub fn native_preview_show(app: AppHandle, id: u32, path: String, rect: Option<Rect>, dark: bool, on_event: Channel<Event>) {
    imp::show(app, id, path, rect, dark, on_event);
}

/// Moves the preview, or hides it (None) while something covers it.
#[tauri::command]
pub fn native_preview_place(id: u32, rect: Option<Rect>) {
    imp::place(id, rect);
}

#[tauri::command]
pub fn native_preview_close(id: u32) {
    imp::close(id);
}

/// Before Glint moves, renames or deletes `paths`: a handler may hold one of them open.
pub fn release(paths: &[String]) {
    imp::release(paths);
}

#[cfg(not(windows))]
mod imp {
    use super::{Event, Rect};
    use tauri::{ipc::Channel, AppHandle};

    pub fn show(_: AppHandle, _: u32, _: String, _: Option<Rect>, _: bool, on_event: Channel<Event>) {
        let _ = on_event.send(Event::Unavailable { reason: String::new() });
    }
    pub fn place(_: u32, _: Option<Rect>) {}
    pub fn close(_: u32) {}
    pub fn release(_: &[String]) {}
}

#[cfg(windows)]
mod imp {
    use super::{Event, Rect};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use tauri::ipc::Channel;
    use tauri::{AppHandle, Manager};
    use windows::core::{implement, w, ComObject, Interface, Result as WinResult, GUID, HSTRING, PCWSTR, PWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, RPC_E_CALL_CANCELED, RPC_E_DISCONNECTED, S_FALSE, WPARAM};
    use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, CreateSolidBrush, DeleteObject, FillRect, RedrawWindow, SetWindowRgn, HDC, RDW_ALLCHILDREN, RDW_ERASE, RDW_INVALIDATE, RGN_DIFF};
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows::Win32::System::Com::{
        CoCancelCall, CoCreateInstance, CoEnableCallCancellation, CoInitializeEx, IBindCtx, IStream, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
        COINIT_MULTITHREADED, STGM_READ, STGM_SHARE_DENY_NONE,
    };
    use windows::Win32::System::Ole::IObjectWithSite;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, IInitializeWithItem, IPreviewHandler, IPreviewHandlerFrame, IPreviewHandlerFrame_Impl, IPreviewHandlerVisuals, IShellItem, IsUserAnAdmin,
        SHCreateItemFromParsingName, SHCreateStreamOnFileEx, ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, PREVIEWHANDLERFRAMEINFO,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;

    /// The shellex key a file type's preview handler is registered under (IPreviewHandler's IID).
    const PREVIEW_HANDLER: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
    const CLASS: PCWSTR = w!("GlintPreviewHost");
    /// A handler that takes longer than this in one call is stopped, and not used again this session.
    const HUNG: Duration = Duration::from_secs(15);
    /// HTML and mail previewers run a browser engine on the file: Glint's own preview shows those.
    const SKIPPED: [&str; 7] = ["htm", "html", "xhtml", "mht", "mhtml", "eml", "svg"];

    enum Command {
        /// Hidden until placed when `rect` is None.
        Show { app: AppHandle, id: u32, path: String, parent: isize, rect: Option<RECT>, dark: bool, reply: Channel<Event> },
        Place { id: u32, rect: Option<RECT> },
        Close { id: u32 },
        /// Lets go of the file shown if it's one of these, then says so.
        Release { paths: Vec<String>, done: Sender<()> },
    }

    struct Worker {
        commands: Mutex<Sender<Command>>,
        thread: u32,
    }

    /// The preview thread, started the first time it's needed.
    fn worker() -> &'static Worker {
        static WORKER: OnceLock<Worker> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (commands, rx) = channel();
            let (ready, id) = channel();
            // When a call into a handler started (ms since the epoch), 0 when none is running.
            let busy = Arc::new(AtomicU64::new(0));
            let watched = busy.clone();
            std::thread::spawn(move || unsafe { run(rx, ready, watched) });
            let thread = id.recv().unwrap_or(0);
            std::thread::spawn(move || watchdog(thread, busy));
            Worker { commands: Mutex::new(commands), thread }
        })
    }

    fn send(command: Command) {
        let worker = worker();
        let _ = worker.commands.lock().unwrap_or_else(|e| e.into_inner()).send(command);
        // Wakes its message loop.
        unsafe {
            let _ = PostThreadMessageW(worker.thread, WM_APP, WPARAM(0), LPARAM(0));
        }
    }

    fn rect(r: Rect) -> RECT {
        RECT { left: r.x, top: r.y, right: r.x + r.width.max(1), bottom: r.y + r.height.max(1) }
    }

    pub fn show(app: AppHandle, id: u32, path: String, at: Option<Rect>, dark: bool, reply: Channel<Event>) {
        let parent = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).map(|h| h.0 as isize);
        match parent {
            Some(parent) => send(Command::Show { app, id, path, parent, rect: at.map(rect), dark, reply }),
            None => {
                let _ = reply.send(Event::Unavailable { reason: String::new() });
            }
        }
    }

    pub fn place(id: u32, at: Option<Rect>) {
        send(Command::Place { id, rect: at.map(rect) });
    }

    pub fn close(id: u32) {
        send(Command::Close { id });
    }

    pub fn release(paths: &[String]) {
        if WORKER_STARTED.get().is_none() {
            return;
        }
        let (done, wait) = channel();
        send(Command::Release { paths: paths.to_vec(), done });
        // A handler that hangs on Unload mustn't hold up the file operation for long.
        let _ = wait.recv_timeout(Duration::from_secs(3));
    }

    /// Set once the preview thread runs: before that, nothing can hold a file.
    static WORKER_STARTED: OnceLock<()> = OnceLock::new();

    fn now_ms() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }

    /// Cancels a call into a handler that hangs, so the preview thread (and, through the windows
    /// joined to it, the UI's input) comes back.
    fn watchdog(thread: u32, busy: Arc<AtomicU64>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let since = busy.load(Ordering::Relaxed);
            if since != 0 && now_ms().saturating_sub(since) > HUNG.as_millis() as u64 {
                busy.store(0, Ordering::Relaxed);
                unsafe {
                    let _ = CoCancelCall(thread, 0);
                }
            }
        }
    }

    /// The preview handler for files like `path`: by extension, ProgID, SystemFileAssociations and
    /// perceived type, as File Explorer looks it up.
    fn handler_for(path: &str) -> Option<GUID> {
        let ext = std::path::Path::new(path).extension()?.to_string_lossy().to_lowercase();
        if SKIPPED.contains(&ext.as_str()) {
            return None;
        }
        let mut buf = [0u16; 64];
        let mut len = buf.len() as u32;
        unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_SHELLEXTENSION, &HSTRING::from(format!(".{ext}")), &HSTRING::from(PREVIEW_HANDLER), Some(PWSTR(buf.as_mut_ptr())), &mut len) }.ok().ok()?;
        let text = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
        GUID::try_from(text.trim_matches(['{', '}'])).ok()
    }

    /// The host's side of the conversation: no accelerators of its own, no keys taken back.
    #[implement(IPreviewHandlerFrame)]
    struct Frame;

    impl IPreviewHandlerFrame_Impl for Frame_Impl {
        fn GetWindowContext(&self) -> WinResult<PREVIEWHANDLERFRAMEINFO> {
            Ok(PREVIEWHANDLERFRAMEINFO::default())
        }
        fn TranslateAccelerator(&self, _msg: *const MSG) -> WinResult<()> {
            Err(S_FALSE.into())
        }
    }

    /// The host paints the pane's color until the handler draws.
    unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
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

    struct Shown {
        id: u32,
        path: String,
        clsid: GUID,
        handler: IPreviewHandler,
        host: HWND,
        web: HWND,
    }

    struct Thread {
        busy: Arc<AtomicU64>,
        shown: Option<Shown>,
        /// Handlers that hung or failed this session: not tried again.
        bad: HashSet<GUID>,
    }

    /// The watchdog cancelled the call, or the handler's process went away: that handler isn't used again.
    fn broken<T>(result: &WinResult<T>) -> bool {
        result.as_ref().is_err_and(|e| e.code() == RPC_E_CALL_CANCELED || e.code() == RPC_E_DISCONNECTED)
    }

    impl Thread {
        /// A call into the handler, watched by the watchdog.
        fn watched<T>(&self, call: impl FnOnce() -> WinResult<T>) -> WinResult<T> {
            self.busy.store(now_ms(), Ordering::Relaxed);
            let result = call();
            self.busy.store(0, Ordering::Relaxed);
            result
        }

        unsafe fn handle(&mut self, command: Command) {
            unsafe {
                match command {
                    Command::Show { app, id, path, parent, rect, dark, reply } => {
                        // The same file for another claimant (Quick Look closing): it moves.
                        if let Some(shown) = self.shown.as_mut().filter(|s| s.path.eq_ignore_ascii_case(&path)) {
                            shown.id = id;
                            self.handle(Command::Place { id, rect });
                            let _ = reply.send(Event::Ready);
                            return;
                        }
                        self.close();
                        let event = match self.open(id, &path, HWND(parent as _), rect, dark) {
                            Ok(()) => {
                                // A handler can take the keyboard (Office does): the list keeps it.
                                if let Some(window) = app.get_webview_window("main") {
                                    let _ = AsRef::<tauri::Webview>::as_ref(&window).set_focus();
                                }
                                Event::Ready
                            }
                            Err(reason) => Event::Unavailable { reason },
                        };
                        let _ = reply.send(event);
                    }
                    Command::Place { id, rect } => {
                        let Some(shown) = self.shown.as_ref().filter(|s| s.id == id) else { return };
                        match rect {
                            Some(r) => {
                                let (width, height) = (r.right - r.left, r.bottom - r.top);
                                let _ = SetWindowPos(shown.host, Some(HWND_TOP), r.left, r.top, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                                let inside = RECT { left: 0, top: 0, right: width, bottom: height };
                                let _ = self.watched(|| shown.handler.SetRect(&inside));
                                let _ = RedrawWindow(Some(shown.host), None, None, RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE);
                                cut_out(shown.web, Some(r));
                            }
                            None => {
                                let _ = ShowWindow(shown.host, SW_HIDE);
                                cut_out(shown.web, None);
                            }
                        }
                    }
                    Command::Close { id } => {
                        if self.shown.as_ref().is_some_and(|s| s.id == id) {
                            self.close();
                        }
                    }
                    Command::Release { paths, done } => {
                        if self.shown.as_ref().is_some_and(|s| paths.iter().any(|p| p.eq_ignore_ascii_case(&s.path))) {
                            self.close();
                        }
                        let _ = done.send(());
                    }
                }
            }
        }

        /// Why `path` gets no preview, or the preview on screen (at `at`, or hidden until placed).
        unsafe fn open(&mut self, id: u32, path: &str, parent: HWND, at: Option<RECT>, dark: bool) -> Result<(), String> {
            unsafe {
                if IsUserAnAdmin().as_bool() {
                    return Err("Windows' previewers don't run while Glint runs as administrator.".into());
                }
                if crate::xray::zone_of(path).is_some_and(|zone| zone >= 3) {
                    return Err("Downloaded from the internet: like File Explorer, Glint doesn't preview these. Open it to see it.".into());
                }
                let Some(clsid) = handler_for(path) else { return Err(String::new()) };
                if self.bad.contains(&clsid) {
                    return Err("This kind of file's previewer stopped responding. Open it to see it.".into());
                }
                let place = at.unwrap_or(RECT { left: 0, top: 0, right: 1, bottom: 1 });
                let (width, height) = (place.right - place.left, place.bottom - place.top);
                let host = CreateWindowExW(
                    WS_EX_NOPARENTNOTIFY,
                    CLASS,
                    PCWSTR::null(),
                    WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                    place.left,
                    place.top,
                    width,
                    height,
                    Some(parent),
                    None,
                    None,
                    None,
                )
                .map_err(|e| e.message())?;
                // The pane's background (COLORREF is 0x00BBGGRR).
                SetWindowLongPtrW(host, GWLP_USERDATA, if dark { 0x001C_1C1C } else { 0x00FF_FFFF });
                let inside = RECT { left: 0, top: 0, right: width, bottom: height };
                let handler = self.watched(|| preview(&clsid, path, host, inside, dark));
                if broken(&handler) {
                    self.bad.insert(clsid);
                }
                let handler = match handler {
                    Ok(handler) => handler,
                    Err(e) => {
                        let _ = DestroyWindow(host);
                        return Err(format!("Windows' previewer couldn't show it: {}", e.message()));
                    }
                };
                let web = FindWindowExW(Some(parent), None, w!("WRY_WEBVIEW"), PCWSTR::null()).unwrap_or_default();
                if let Some(at) = at {
                    let _ = SetWindowPos(host, Some(HWND_TOP), at.left, at.top, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    cut_out(web, Some(at));
                }
                self.shown = Some(Shown { id, path: path.to_string(), clsid, handler, host, web });
                Ok(())
            }
        }

        fn close(&mut self) {
            let Some(shown) = self.shown.take() else { return };
            unsafe {
                cut_out(shown.web, None);
                let _ = ShowWindow(shown.host, SW_HIDE);
                if broken(&self.watched(|| shown.handler.Unload())) {
                    self.bad.insert(shown.clsid);
                }
                if let Ok(site) = shown.handler.cast::<IObjectWithSite>() {
                    let _ = site.SetSite(None);
                }
                drop(shown.handler);
                let _ = DestroyWindow(shown.host);
            }
        }
    }

    /// Creates the handler out of process, gives it the file (as a stream, a path or a shell item,
    /// whichever it takes) and has it draw in `host`.
    unsafe fn preview(clsid: &GUID, path: &str, host: HWND, inside: RECT, dark: bool) -> WinResult<IPreviewHandler> {
        unsafe {
            let handler: IPreviewHandler = CoCreateInstance(clsid, None, CLSCTX_LOCAL_SERVER)?;
            if let Ok(init) = handler.cast::<IInitializeWithStream>() {
                // Shared every way: the file can still be renamed, deleted or written while it shows.
                let stream: IStream = SHCreateStreamOnFileEx(&HSTRING::from(path), (STGM_READ | STGM_SHARE_DENY_NONE).0, FILE_ATTRIBUTE_NORMAL.0, false, None::<&IStream>)?;
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

    /// Cuts `hole` (client coordinates: the web view's container sits at 0,0) out of where the web view
    /// may draw, or gives it all back (None). The outer rectangle is larger than any window, so resizing
    /// needs no new region.
    unsafe fn cut_out(web: HWND, hole: Option<RECT>) {
        if web.is_invalid() {
            return;
        }
        unsafe {
            match hole {
                None => {
                    let _ = SetWindowRgn(web, None, true);
                }
                Some(r) => {
                    let all = CreateRectRgn(-32000, -32000, 32000, 32000);
                    let hole = CreateRectRgn(r.left, r.top, r.right, r.bottom);
                    CombineRgn(Some(all), Some(all), Some(hole), RGN_DIFF);
                    let _ = DeleteObject(hole.into());
                    // The system owns `all` from here.
                    let _ = SetWindowRgn(web, Some(all), true);
                }
            }
        }
    }

    /// The preview thread: its own apartment, a message loop that handlers' calls come back through,
    /// and the commands, drained whenever one is posted.
    unsafe fn run(commands: Receiver<Command>, ready: Sender<u32>, busy: Arc<AtomicU64>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            // Lets the watchdog cancel a call that hangs.
            let _ = CoEnableCallCancellation(None);
            let class = WNDCLASSW { lpfnWndProc: Some(host_proc), lpszClassName: CLASS, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
            RegisterClassW(&class);
            // The message queue, before anyone posts to it.
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
            let _ = WORKER_STARTED.set(());
            let _ = ready.send(GetCurrentThreadId());
            let mut thread = Thread { busy, shown: None, bad: HashSet::new() };
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                if msg.hwnd.is_invalid() && msg.message == WM_APP {
                    // A preview asked for and then left (arrowing through files) isn't made at all.
                    let batch: Vec<Command> = commands.try_iter().collect();
                    let left = |i: usize, id: u32| batch[i + 1..].iter().any(|c| matches!(c, Command::Show { .. }) || matches!(c, Command::Close { id: later } if *later == id));
                    let skip: Vec<bool> = batch.iter().enumerate().map(|(i, c)| matches!(c, Command::Show { id, .. } if left(i, *id))).collect();
                    for (command, skip) in batch.into_iter().zip(skip) {
                        if !skip {
                            thread.handle(command);
                        }
                    }
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
