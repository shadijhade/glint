//! Windows 11's snap layouts on the maximize button, as any window has them. Glint draws its own title
//! bar and the page covers the whole window, so Windows can't tell where its maximize button is: an
//! invisible window of the app's own sits over the page's button and says that's what it is (it
//! answers hit tests with HTMAXBUTTON), which is what makes Windows offer the layouts when it's
//! hovered. The page doesn't see the pointer there any more, so this tells it what the pointer does
//! (for the button to light up), and a click maximizes or restores the window itself.

use serde::Deserialize;

/// Where the page's maximize button is in the window's client area, in physical pixels.
#[derive(Deserialize, Clone, Copy)]
pub struct Rect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

/// Puts the button over the page's maximize button (and keeps it on top), or takes it away.
#[tauri::command]
pub fn snap_place(window: tauri::WebviewWindow, rect: Option<Rect>) {
    #[cfg(windows)]
    imp::place(&window, rect);
    #[cfg(not(windows))]
    let _ = (window, rect);
}

#[cfg(windows)]
mod imp {
    use super::Rect;
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::OnceLock;
    use tauri::{AppHandle, Emitter, Manager};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const CLASS: PCWSTR = w!("GlintSnapButton");
    /// The button's window, 0 until it's made (on the main thread, which runs its window procedure).
    static BUTTON: AtomicIsize = AtomicIsize::new(0);
    static APP: OnceLock<AppHandle> = OnceLock::new();
    static OVER: AtomicBool = AtomicBool::new(false);
    static PRESSED: AtomicBool = AtomicBool::new(false);

    pub fn place(window: &tauri::WebviewWindow, rect: Option<Rect>) {
        let Ok(parent) = window.hwnd() else { return };
        unsafe {
            let mut button = HWND(BUTTON.load(Ordering::Relaxed) as _);
            if button.is_invalid() {
                let Some(r) = rect else { return };
                let _ = APP.set(window.app_handle().clone());
                let class = WNDCLASSW { lpfnWndProc: Some(button_proc), lpszClassName: CLASS, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
                RegisterClassW(&class);
                // Never painted: the page's button shows through. Not layered or transparent, which
                // would take it out of hit testing.
                let Ok(made) = CreateWindowExW(WS_EX_NOPARENTNOTIFY, CLASS, PCWSTR::null(), WS_CHILD | WS_CLIPSIBLINGS, r.x, r.y, r.width, r.height, Some(HWND(parent.0 as _)), None, None, None) else {
                    return;
                };
                BUTTON.store(made.0 as isize, Ordering::Relaxed);
                button = made;
            }
            match rect {
                // Over the webview, which is the window's other child.
                Some(r) => {
                    let _ = SetWindowPos(button, Some(HWND_TOP), r.x, r.y, r.width.max(1), r.height.max(1), SWP_NOACTIVATE | SWP_SHOWWINDOW);
                }
                None => {
                    let _ = ShowWindow(button, SW_HIDE);
                }
            }
        }
    }

    /// "hover", "press" or "" (the pointer left), for the page's button to look it.
    fn tell(state: &str) {
        if let Some(app) = APP.get() {
            let _ = app.emit_to("main", "snap-button", state);
        }
    }

    unsafe extern "system" fn button_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_NCHITTEST => LRESULT(HTMAXBUTTON as isize),
                WM_NCMOUSEMOVE => {
                    if !OVER.swap(true, Ordering::Relaxed) {
                        let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut track);
                        tell("hover");
                    }
                    LRESULT(0)
                }
                WM_NCMOUSELEAVE => {
                    OVER.store(false, Ordering::Relaxed);
                    PRESSED.store(false, Ordering::Relaxed);
                    tell("");
                    LRESULT(0)
                }
                // Not to DefWindowProc, which would maximize this window, inside Glint's.
                WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK => {
                    PRESSED.store(true, Ordering::Relaxed);
                    tell("press");
                    LRESULT(0)
                }
                WM_NCLBUTTONUP => {
                    if PRESSED.swap(false, Ordering::Relaxed) {
                        if let Ok(window) = GetParent(hwnd) {
                            let _ = ShowWindow(window, if IsZoomed(window).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                        }
                    }
                    tell(if OVER.load(Ordering::Relaxed) { "hover" } else { "" });
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }
}
