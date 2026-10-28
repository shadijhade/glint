//! The main window comes back where it was: its size and place, and whether it was maximized, kept in
//! Glint's data folder (window.json) when it closes or goes to the tray, and put back while it's still
//! hidden, before it first shows. The first time, a window too big for the screen it opens on is made
//! to fit it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Where the place is kept.
static FILE: OnceLock<PathBuf> = OnceLock::new();
/// The window was maximized when it was put away: it is again as it first shows.
static MAXIMIZE: AtomicBool = AtomicBool::new(false);

/// The window's place when it isn't maximized or minimized (Windows' own "normal position", in
/// workspace coordinates, which only Get/SetWindowPlacement read), and whether it was maximized.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq)]
struct Placement {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    maximized: bool,
}

/// Puts the hidden main window back where it was, or fits it to the screen the first time.
pub fn restore(window: &tauri::WebviewWindow, data: &Path) {
    let file = FILE.get_or_init(|| data.join("window.json"));
    let saved = std::fs::read(file).ok().and_then(|bytes| serde_json::from_slice::<Placement>(&bytes).ok());
    if let Ok(hwnd) = window.hwnd() {
        imp::restore(hwnd.0 as _, saved.filter(|p| p.right > p.left && p.bottom > p.top));
    }
}

/// Keeps the window's place (as it closes, or goes to the tray). A hidden window's was kept as it hid.
pub fn save(hwnd: *mut std::ffi::c_void) {
    let Some(file) = FILE.get() else { return };
    if let Some(placement) = imp::placement(hwnd) {
        if let (Some(folder), Ok(json)) = (file.parent(), serde_json::to_vec(&placement)) {
            let _ = std::fs::create_dir_all(folder);
            let _ = std::fs::write(file, json);
        }
    }
}

/// Shows the main window: maximized the first time, if it was when it was put away.
pub fn show(window: &tauri::WebviewWindow) {
    if MAXIMIZE.swap(false, Ordering::SeqCst) {
        let _ = window.maximize();
    }
    let _ = window.show();
}

/// Where a first window of `size` goes in a screen's work area (left, top, right, bottom), when it's
/// over 90% of it either way: 85% of it, centered. None when it fits as it is.
#[cfg_attr(not(windows), allow(dead_code))]
fn fitted((w, h): (i32, i32), (left, top, right, bottom): (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (room_w, room_h) = (right - left, bottom - top);
    if w * 10 <= room_w * 9 && h * 10 <= room_h * 9 {
        return None;
    }
    let (w, h) = (w.min(room_w * 85 / 100), h.min(room_h * 85 / 100));
    Some((left + (room_w - w) / 2, top + (room_h - h) / 2, w, h))
}

#[cfg(windows)]
mod imp {
    use super::{Placement, MAXIMIZE};
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowPlacement, GetWindowRect, IsWindowVisible, SetWindowPlacement, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED,
        WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
    };

    fn current(hwnd: HWND) -> Option<WINDOWPLACEMENT> {
        let mut placement = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        unsafe { GetWindowPlacement(hwnd, &mut placement) }.ok().map(|_| placement)
    }

    pub fn restore(hwnd: *mut std::ffi::c_void, saved: Option<Placement>) {
        let hwnd = HWND(hwnd);
        let Some(saved) = saved else { return fit(hwnd) };
        let Some(mut placement) = current(hwnd) else { return };
        // Still hidden; a place on a screen no longer there (or now smaller) Windows moves onto one that is.
        placement.showCmd = SW_HIDE.0 as u32;
        placement.rcNormalPosition = RECT { left: saved.left, top: saved.top, right: saved.right, bottom: saved.bottom };
        if unsafe { SetWindowPlacement(hwnd, &placement) }.is_ok() {
            MAXIMIZE.store(saved.maximized, Ordering::SeqCst);
        }
    }

    /// The first time: as configured (1280 × 800 at 100%), unless that's over 90% of the screen's work
    /// area either way (a 1080p laptop at 150% is 1280 × 720), then 85% of it, centered.
    fn fit(hwnd: HWND) {
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool() {
            return;
        }
        let work = info.rcWork;
        if let Some((x, y, w, h)) = super::fitted((rect.right - rect.left, rect.bottom - rect.top), (work.left, work.top, work.right, work.bottom)) {
            let _ = unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }

    pub fn placement(hwnd: *mut std::ffi::c_void) -> Option<Placement> {
        let hwnd = HWND(hwnd);
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return None;
        }
        let placement = current(hwnd)?;
        let r = placement.rcNormalPosition;
        let minimized_from_maximized = placement.showCmd == SW_SHOWMINIMIZED.0 as u32 && placement.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0;
        let maximized = placement.showCmd == SW_SHOWMAXIMIZED.0 as u32 || minimized_from_maximized;
        Some(Placement { left: r.left, top: r.top, right: r.right, bottom: r.bottom, maximized })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Placement;

    pub fn restore(_: *mut std::ffi::c_void, _: Option<Placement>) {}

    pub fn placement(_: *mut std::ffi::c_void) -> Option<Placement> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_keeps() {
        let placement = Placement { left: -1900, top: 40, right: -300, bottom: 1040, maximized: true };
        let json = serde_json::to_vec(&placement).unwrap();
        assert_eq!(serde_json::from_slice::<Placement>(&json).unwrap(), placement);
        // A damaged file is passed over: the window opens as configured.
        assert!(serde_json::from_slice::<Placement>(b"{\"left\":1}").is_err());
    }

    #[test]
    fn fits_a_first_window_to_small_screens() {
        // 1280 × 800 at 150% is 1920 × 1200 pixels; a 1080p laptop's work area is 1920 × 1032.
        assert_eq!(fitted((1920, 1200), (0, 0, 1920, 1032)), Some((144, 77, 1632, 877)));
        // At 100% on the same screen it fits as it is, and on a second screen to the left too.
        assert_eq!(fitted((1280, 800), (0, 0, 1920, 1032)), None);
        assert_eq!(fitted((1600, 1000), (-2560, 0, 0, 1400)), None);
        // Only the side that's too big shrinks.
        assert_eq!(fitted((1000, 1000), (0, 0, 1600, 900)), Some((300, 67, 1000, 765)));
    }
}
