//! Phase 0's Windows integration checks for the GPUI window: child windows and a WebView2 island above
//! GPUI's drawing, Glint-style drop target, the shell context menu and drag-out. Modal Win32 loops
//! (TrackPopupMenuEx, SHDoDragDrop) run from a posted message in a window subclass, never inside a GPUI
//! callback, so GPUI isn't borrowed while they pump messages.

#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::{CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler, NavigationCompletedEventHandler};
use windows::core::{implement, w, Interface, Ref, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, POINTL, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::CreateSolidBrush;
use windows::Win32::System::Com::{IBindCtx, IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY,
    DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{MK_CONTROL, MK_SHIFT, MODIFIERKEYS_FLAGS};

/// MK_ALT, which the windows crate leaves out.
const MK_ALT_BIT: u32 = 0x20;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DefSubclassProc, DragQueryFileW, IContextMenu, IContextMenu3, IShellFolder, SHBindToParent, SHCreateShellItemArrayFromIDLists,
    SHDoDragDrop, SHParseDisplayName, SetWindowSubclass, BHID_DataObject, CMF_EXPLORE, CMF_NORMAL, CMINVOKECOMMANDINFO, HDROP,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::since_start;

/// The GPUI window, once its first frame is drawn.
pub static HWND_VALUE: AtomicIsize = AtomicIsize::new(0);
pub const WM_SHOW_MENU: u32 = WM_APP + 1;
pub const WM_DRAG_OUT: u32 = WM_APP + 2;

pub fn hwnd() -> HWND {
    HWND(HWND_VALUE.load(Ordering::Relaxed) as *mut _)
}

thread_local! {
    static KEEP: RefCell<Vec<ICoreWebView2Controller>> = const { RefCell::new(Vec::new()) };
    static MENU: RefCell<Option<IContextMenu3>> = const { RefCell::new(None) };
    static FILES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub fn set_files(files: Vec<String>) {
    FILES.with(|f| *f.borrow_mut() = files);
}

fn file(ix: usize) -> Option<String> {
    FILES.with(|f| {
        let f = f.borrow();
        (!f.is_empty()).then(|| f[ix % f.len()].clone())
    })
}

// ---- Child windows above GPUI ------------------------------------------------------------------

unsafe extern "system" fn child_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(window, msg, wparam, lparam) }
}

/// A plain child window with its own red background, the way a preview handler's window sits.
pub fn child_window(parent: HWND, rect: RECT) {
    unsafe {
        let module = GetModuleHandleW(None).unwrap();
        let class = w!("GlintSpikeChild");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(child_proc),
            hInstance: module.into(),
            lpszClassName: class,
            hbrBackground: CreateSolidBrush(COLORREF(0x002020C0)),
            ..Default::default()
        };
        RegisterClassW(&wc);
        // SPIKE_LAYERED: a layered child window gets its own surface, which GDI children of a
        // DirectComposition window otherwise lack.
        let layered = std::env::var("SPIKE_LAYERED").is_ok();
        let child = CreateWindowExW(
            if layered { WS_EX_LAYERED } else { WINDOW_EX_STYLE(0) },
            class,
            w!("child"),
            WS_CHILD | WS_VISIBLE,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            Some(parent),
            None,
            Some(module.into()),
            None,
        );
        if let (true, Ok(child)) = (layered, &child) {
            let _ = SetLayeredWindowAttributes(*child, COLORREF(0), 255, LWA_ALPHA);
        }
        println!("child window: {}{}", if child.is_ok() { "created" } else { "failed" }, if layered { " (layered)" } else { "" });
    }
}

/// A WebView2 controller as a child of the GPUI window, showing a page made from a string.
pub fn webview(parent: HWND, rect: RECT, data_dir: String) {
    let t0 = since_start();
    println!("island: creating WebView2 at {t0:.1} ms");
    let env_handler = CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
        result?;
        let env = env.expect("environment");
        println!("island: environment ready after {:.1} ms", since_start() - t0);
        let controller_handler = CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, controller| {
            result?;
            let controller = controller.expect("controller");
            println!("island: controller ready after {:.1} ms", since_start() - t0);
            unsafe {
                controller.SetBounds(rect)?;
                let webview = controller.CoreWebView2()?;
                let shown = NavigationCompletedEventHandler::create(Box::new(move |_, _| {
                    println!("island: page shown after {:.1} ms", since_start() - t0);
                    Ok(())
                }));
                let mut token = 0;
                webview.add_NavigationCompleted(&shown, &mut token)?;
                let page = "<html><body style='margin:0;background:#1f6feb;color:#fff;font:26px Segoe UI'>\
                            <div style='padding:20px'>WebView2 island</div>\
                            <div style='padding:0 20px;font-size:20px' dir='rtl'>تقرير الربع 1.pdf</div></body></html>";
                webview.NavigateToString(&HSTRING::from(page))?;
            }
            KEEP.with(|k| k.borrow_mut().push(controller));
            Ok(())
        }));
        unsafe { env.CreateCoreWebView2Controller(parent, &controller_handler)? };
        Ok(())
    }));
    unsafe {
        if let Err(error) = CreateCoreWebView2EnvironmentWithOptions(PCWSTR::null(), &HSTRING::from(data_dir), None, &env_handler) {
            println!("island: environment failed: {error}");
        }
    }
}

// ---- A real Windows preview handler (out of process, in prevhost.exe) ---------------------------

thread_local! {
    static HANDLERS: RefCell<Vec<windows::Win32::UI::Shell::IPreviewHandler>> = const { RefCell::new(Vec::new()) };
}

/// Hosts the file's preview handler (`clsid`) in a rectangle of the window, as native_preview.rs does.
pub fn preview_handler(parent: HWND, rect: RECT, path: &str, clsid: &str) {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_LOCAL_SERVER, STGM_READ};
    use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
    use windows::Win32::UI::Shell::{IPreviewHandler, SHCreateStreamOnFileEx};
    let t0 = since_start();
    let run = || -> Result<()> {
        unsafe {
            let clsid = windows::core::GUID::try_from(clsid.trim_matches(|c| c == '{' || c == '}')).map_err(|_| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
            let handler: IPreviewHandler = CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)?;
            if let Ok(init) = handler.cast::<IInitializeWithFile>() {
                init.Initialize(&HSTRING::from(path), STGM_READ.0)?;
            } else {
                let stream = SHCreateStreamOnFileEx(&HSTRING::from(path), STGM_READ.0, 0, false, None)?;
                handler.cast::<IInitializeWithStream>()?.Initialize(&stream, STGM_READ.0)?;
            }
            // SPIKE_POPUP: GDI-drawn handlers (Office's, Windows' text previewer) are invisible as children
            // of a DirectComposition window, so host them in an owned pop-up over the pane instead: a
            // top-level window has its own surface. It's kept over the pane as the window moves.
            let (host, host_rect) = if std::env::var("SPIKE_POPUP").is_ok() {
                let popup = host_popup(parent, rect);
                (popup, RECT { left: 0, top: 0, right: rect.right - rect.left, bottom: rect.bottom - rect.top })
            } else {
                (parent, rect)
            };
            handler.SetWindow(host, &host_rect)?;
            handler.DoPreview()?;
            HANDLERS.with(|h| h.borrow_mut().push(handler));
            Ok(())
        }
    };
    match run() {
        Ok(()) => println!("preview handler: showing after {:.1} ms", since_start() - t0),
        Err(error) => println!("preview handler: failed: {error}"),
    }
}

unsafe extern "system" fn popup_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        DefWindowProcW(window, msg, wparam, lparam)
    }
}

thread_local! {
    /// The pop-up and the client rectangle of the pane it covers, to follow the window.
    static POPUP: std::cell::Cell<Option<(HWND, RECT)>> = const { std::cell::Cell::new(None) };
}

/// An owned, never-activated pop-up window placed over a client rectangle of `owner`.
pub fn host_popup(owner: HWND, rect: RECT) -> HWND {
    unsafe {
        let module = GetModuleHandleW(None).unwrap();
        let class = w!("GlintSpikePreviewHost");
        let wc = WNDCLASSW { lpfnWndProc: Some(popup_proc), hInstance: module.into(), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let at = screen_point(owner, rect.left, rect.top);
        let popup = CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            class,
            w!("preview"),
            WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
            at.x,
            at.y,
            rect.right - rect.left,
            rect.bottom - rect.top,
            Some(owner),
            None,
            Some(module.into()),
            None,
        )
        .unwrap();
        POPUP.with(|p| p.set(Some((popup, rect))));
        println!("preview host: owned pop-up at screen {},{}", at.x, at.y);
        popup
    }
}

/// Keeps the pop-up over its pane when the window moves (called from the subclass).
fn follow_popup(owner: HWND) {
    if let Some((popup, rect)) = POPUP.with(|p| p.get()) {
        let at = screen_point(owner, rect.left, rect.top);
        unsafe {
            let _ = SetWindowPos(popup, None, at.x, at.y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

// ---- Glint's own drop target ---------------------------------------------------------------------

#[implement(IDropTarget)]
pub struct Target;

fn effect_for(keys: MODIFIERKEYS_FLAGS) -> DROPEFFECT {
    // Glint's rules: copy by default, Shift moves, Ctrl+Shift or Alt makes a shortcut.
    if keys.0 & MK_ALT_BIT != 0 || (keys.0 & MK_CONTROL.0 != 0 && keys.0 & MK_SHIFT.0 != 0) {
        DROPEFFECT_LINK
    } else if keys.0 & MK_SHIFT.0 != 0 {
        DROPEFFECT_MOVE
    } else {
        DROPEFFECT_COPY
    }
}

fn effect_name(effect: DROPEFFECT) -> &'static str {
    match effect {
        DROPEFFECT_COPY => "copy",
        DROPEFFECT_MOVE => "move",
        DROPEFFECT_LINK => "link",
        _ => "none",
    }
}

/// The paths a data object carries as CF_HDROP.
pub fn dropped_paths(data: &IDataObject) -> Vec<String> {
    unsafe {
        let format = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let Ok(mut medium) = data.GetData(&format) else { return Vec::new() };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::new();
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            paths.push(String::from_utf16_lossy(&buf[..len]));
        }
        ReleaseStgMedium(&mut medium);
        paths
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let n = data.as_ref().map(|d| dropped_paths(d).len()).unwrap_or(0);
        unsafe { *effect = if n > 0 { effect_for(keys) } else { DROPEFFECT_NONE } };
        println!("drop target: enter with {n} file(s), effect {}", effect_name(unsafe { *effect }));
        Ok(())
    }
    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = effect_for(keys) };
        Ok(())
    }
    fn DragLeave(&self) -> Result<()> {
        Ok(())
    }
    fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let paths = data.as_ref().map(dropped_paths).unwrap_or_default();
        unsafe { *effect = effect_for(keys) };
        println!("drop target: dropped {} file(s) as {}: {:?}", paths.len(), effect_name(unsafe { *effect }), paths);
        Ok(())
    }
}

/// Replaces GPUI's drop target (paths only, always copy) with Glint's.
pub fn replace_drop_target(window: HWND) -> Option<IDropTarget> {
    unsafe {
        let revoked = RevokeDragDrop(window);
        let target: IDropTarget = Target.into();
        let registered = RegisterDragDrop(window, &target);
        println!("drop target: revoke GPUI's {:?}, register Glint's {:?}", revoked.map(|_| "ok"), registered.as_ref().map(|_| "ok"));
        registered.ok().map(|_| target)
    }
}

/// A shell data object for files, as File Explorer hands one over when dragging.
pub fn data_object(paths: &[String]) -> Option<IDataObject> {
    unsafe {
        let mut pidls: Vec<*const ITEMIDLIST> = Vec::new();
        for path in paths {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(&HSTRING::from(path.as_str()), None, &mut pidl, 0, None).is_ok() {
                pidls.push(pidl);
            }
        }
        // The shell's own data object for the items, as Glint's drag-out makes it (shell.rs).
        SHCreateShellItemArrayFromIDLists(&pidls)
            .and_then(|items| items.BindToHandler::<_, IDataObject>(None::<&IBindCtx>, &BHID_DataObject))
            .ok()
    }
}

/// Calls Glint's drop target the way OLE does, with a real shell data object: plain, then with Shift.
pub fn synthetic_drop(target: &IDropTarget, paths: &[String]) {
    let Some(data) = data_object(paths) else {
        println!("drop target: no data object");
        return;
    };
    unsafe {
        let pt = POINTL { x: 200, y: 200 };
        let mut effect = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
        let _ = target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect);
        let _ = target.Drop(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect);
        let _ = target.DragEnter(&data, MK_SHIFT, pt, &mut effect);
        let _ = target.Drop(&data, MK_SHIFT, pt, &mut effect);
    }
}

// ---- Subclass: the shell menu and drag-out, outside GPUI's callbacks -----------------------------

pub fn subclass(window: HWND) {
    unsafe {
        let ok = SetWindowSubclass(window, Some(subclass_proc), 0x6c696e74, 0);
        println!("subclass: {}", if ok.as_bool() { "installed" } else { "failed" });
    }
}

unsafe extern "system" fn subclass_proc(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        // While a shell menu is open, its owner-drawn and dynamic submenus (Send to, Open with) need
        // these messages.
        if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
            let handled = MENU.with(|m| {
                m.borrow().as_ref().map(|menu| {
                    let mut result = LRESULT(0);
                    menu.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)).is_ok().then_some(result)
                })
            });
            if let Some(Some(result)) = handled {
                return result;
            }
        }
        if msg == WM_WINDOWPOSCHANGED || msg == WM_MOVE {
            follow_popup(window);
        }
        match msg {
            WM_SHOW_MENU => {
                let (mut x, mut y) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
                if lparam.0 == -1 {
                    let mut cursor = POINT::default();
                    let _ = GetCursorPos(&mut cursor);
                    (x, y) = (cursor.x, cursor.y);
                }
                if let Some(path) = file(wparam.0) {
                    show_shell_menu(window, &path, x, y);
                }
                LRESULT(0)
            }
            WM_DRAG_OUT => {
                if let Some(path) = file(wparam.0) {
                    drag_out(window, &path);
                }
                LRESULT(0)
            }
            _ => DefSubclassProc(window, msg, wparam, lparam),
        }
    }
}

/// The file's own shell menu (Explorer's), shown at a screen point; runs the command picked.
pub fn show_shell_menu(window: HWND, path: &str, x: i32, y: i32) {
    unsafe {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(&HSTRING::from(path), None, &mut pidl, 0, None).is_err() {
            println!("shell menu: can't parse {path}");
            return;
        }
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        let Ok(folder) = SHBindToParent::<IShellFolder>(pidl, Some(&mut child)) else { return };
        let Ok(menu) = folder.GetUIObjectOf::<IContextMenu>(window, &[child as *const _], None) else { return };
        let popup = CreatePopupMenu().unwrap();
        let t0 = since_start();
        let _ = menu.QueryContextMenu(popup, 0, 1, 0x7fff, CMF_NORMAL | CMF_EXPLORE);
        let count = GetMenuItemCount(Some(popup));
        let mut names = Vec::new();
        for i in 0..count {
            let mut buf = [0u16; 128];
            let n = GetMenuStringW(popup, i as u32, Some(&mut buf), MF_BYPOSITION);
            if n > 0 {
                names.push(String::from_utf16_lossy(&buf[..n as usize]).replace('&', ""));
            }
        }
        println!("shell menu: {count} items in {:.1} ms: {}", since_start() - t0, names.join(" | "));
        MENU.with(|m| *m.borrow_mut() = menu.cast::<IContextMenu3>().ok());
        let _ = SetForegroundWindow(window);
        println!("shell menu: shown at {:.1} ms", since_start());
        let cmd = TrackPopupMenuEx(popup, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, x, y, window, None);
        println!("shell menu: closed at {:.1} ms (command {})", since_start(), cmd.0);
        if cmd.0 > 0 {
            let info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                hwnd: window,
                lpVerb: windows::core::PCSTR((cmd.0 - 1) as usize as *const u8),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            let _ = menu.InvokeCommand(&info);
        }
        MENU.with(|m| *m.borrow_mut() = None);
        let _ = DestroyMenu(popup);
    }
}

/// Drags the file out of the window (to File Explorer, the desktop, a mail…), as Glint's file list does.
pub fn drag_out(window: HWND, path: &str) {
    let Some(data) = data_object(&[path.to_string()]) else { return };
    unsafe {
        let t0 = since_start();
        let result = SHDoDragDrop(Some(window), &data, None::<&windows::Win32::System::Ole::IDropSource>, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK);
        println!("drag out: returned {:?} after {:.1} ms", result.map(|e| effect_name(e)), since_start() - t0);
    }
}

/// Screen point for a client point of the window.
pub fn screen_point(window: HWND, x: i32, y: i32) -> POINT {
    let mut point = POINT { x, y };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(window, &mut point);
    }
    point
}

pub fn post_at_cursor(msg: u32, wparam: usize) {
    unsafe {
        let _ = PostMessageW(Some(hwnd()), msg, WPARAM(wparam), LPARAM(-1));
    }
}

pub fn post(window: HWND, msg: u32, wparam: usize, x: i32, y: i32) {
    let lparam = ((y & 0xffff) << 16) as isize | (x & 0xffff) as isize;
    unsafe {
        let _ = PostMessageW(Some(window), msg, WPARAM(wparam), LPARAM(lparam));
    }
}
