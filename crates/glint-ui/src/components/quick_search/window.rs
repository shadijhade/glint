//! The Quick Search window itself: made hidden ahead of time, then shown and hidden (never destroyed while
//! Glint runs), so the shortcut brings it up at once.
//!
//! Ported from src-tauri/src/quick.rs (`window`, `summon`, `toggle`, `quick_hide`, `warm`, `on_close`).
//!
//! | quick.rs                                  | Here                                                         |
//! |-------------------------------------------|--------------------------------------------------------------|
//! | `window()`, made once, hidden             | [`prepare`] (a GPUI `PopUp` window, `show: false`)           |
//! | `warm()`, a second after the main window  | [`warm`]                                                     |
//! | `summon()` (shortcut, tray, `--quick-search`) | [`toggle`] (shows, or hides when it is up and in front)  |
//! | `toggle()` without hiding                 | [`summon`]                                                   |
//! | `quick_hide`                              | [`hide`]                                                     |
//! | `on_close` destroying it without a tray   | [`close`]                                                    |
//!
//! **Kind and look.** 640 × 440, no title bar, not resizable, on top while open (`WindowKind::PopUp` is a
//! tool window: it has no taskbar button), centered on its monitor each time it is shown, with the rounded
//! corners Windows 11 gives its own pop-ups. Escape, a click elsewhere (it loses the foreground) or Alt+F4
//! put it away.
//!
//! **Win32 calls never run inside a GPUI callback** (`STANDARDS.md` 5): showing, placing, hiding and bringing
//! the window forward all send it messages at once, so they are posted ([`core_bridge::post_to_ui`]) and run
//! from the message pump. The checks that only read (is it in front?) are made where they are asked.

use super::{QuickSearch, QuickSearchEvent};
use crate::kit::prelude::*;
use crate::{core_bridge, state};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_HIDE,
};

/// The label core knows this window by (`get_webview_window("quick")`), for `open_item`'s owner and drag-out.
pub const LABEL: &str = "quick";
/// The window's size in logical pixels (`inner_size(640.0, 440.0)`).
const WIDTH: f32 = 640.;
const HEIGHT: f32 = 440.;

/// Whether the window has been shown and not put away since. Set synchronously by [`summon`] and [`hide`], so a
/// second call before the posted job has run still sees what the first decided.
static SHOWN: AtomicBool = AtomicBool::new(false);

/// The window, once [`prepare`] has made it.
struct QuickWindow {
    handle: AnyWindowHandle,
    view: Entity<QuickSearch>,
    /// Its HWND, for the posted jobs (which can't hold a `Window`).
    hwnd: isize,
    _subscriptions: Vec<Subscription>,
}

impl Global for QuickWindow {}

/// What the coordinator listens to: the things Quick Search asks of the rest of Glint.
pub struct QuickSearchHub;

impl EventEmitter<QuickSearchEvent> for QuickSearchHub {}

/// The hub, in a global ([`init`]).
struct Hub(Entity<QuickSearchHub>);

impl Global for Hub {}

/// Start-up: binds Quick Search's keys and makes the hub its events come out of. Call once, after
/// `actions::init`. Subscribe to the returned hub for [`QuickSearchEvent::Reveal`].
pub fn init(cx: &mut App) -> Entity<QuickSearchHub> {
    super::bind_keys(cx);
    if let Some(hub) = cx.try_global::<Hub>() {
        return hub.0.clone();
    }
    let hub = cx.new(|_| QuickSearchHub);
    cx.set_global(Hub(hub.clone()));
    hub
}

/// Makes the window now, hidden, if it isn't there: the first press of the shortcut then shows a window that
/// already exists. Does nothing when it is there. Say why on failure; the shortcut then tries again.
pub fn prepare(cx: &mut App) {
    if cx.has_global::<QuickWindow>() {
        return;
    }
    match make(cx) {
        Ok(window) => cx.set_global(window),
        Err(error) => eprintln!("glint: couldn't make the Quick Search window: {error}"),
    }
}

/// Whether the shortcut or the tray is on, which is when the window is wanted ahead of time.
fn wanted(cx: &App) -> bool {
    let settings = state::settings(cx);
    settings.quick_shortcut.as_deref().is_some_and(|s| !s.trim().is_empty())
        || settings.keep_in_tray
        || crate::platform::tray::keep_running()
}

/// Makes the window a second after the call (the main window has just shown its first folder, and the two
/// mustn't start together) when the shortcut or the tray is on, and whenever one is turned on later
/// (`warm`, and `quick_configure`'s making it when the shortcut was set). Call once, from start-up.
pub fn warm(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
        cx.update(|cx| {
            if wanted(cx) {
                prepare(cx);
            }
            cx.observe_global::<state::UiState>(|cx| {
                if wanted(cx) {
                    prepare(cx);
                }
            })
            .detach();
        });
    })
    .detach();
}

fn make(cx: &mut App) -> Result<QuickWindow, String> {
    let bounds = Bounds::centered(None, size(px(WIDTH), px(HEIGHT)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        // Made hidden and not taking the keyboard: it is shown when summoned.
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    let (handle, view) = gpui_kit::open_window(options, cx, |window, cx| {
        core_bridge::register_window(cx, LABEL, window);
        cx.new(|cx| QuickSearch::new(window, cx))
    })
    .map_err(|e| format!("{e:#}"))?;

    let hwnd = handle
        .update(cx, |_, window, cx| {
            // Alt+F4 and the system menu's Close put it away; the window lives as long as Glint does.
            window.on_window_should_close(cx, |_, cx| {
                hide(cx);
                false
            });
            core_bridge::hwnd_of(window)
        })
        .map_err(|e| format!("{e:#}"))?
        .ok_or("the window has no handle")?;
    let raw = hwnd.0 as isize;
    core_bridge::post_to_ui(move || round_corners(HWND(raw as *mut _)));

    let subscription = cx.subscribe(&view, |_, event: &QuickSearchEvent, cx| {
        hide(cx);
        if let (QuickSearchEvent::Reveal { .. }, Some(hub)) = (event, cx.try_global::<Hub>().map(|h| h.0.clone())) {
            hub.update(cx, |_, cx| cx.emit(event.clone()));
        }
    });
    // Leaving core's window list when this view goes away with its window.
    let released = view.update(cx, |_, cx| {
        cx.on_release(|_, cx| core_bridge::unregister_window(cx, LABEL))
    });
    Ok(QuickWindow {
        handle,
        view,
        hwnd: raw,
        _subscriptions: vec![subscription, released],
    })
}

/// Brings Quick Search over whatever is in front (making the window first if it isn't there), empty of
/// nothing: it shows what it showed last, with the query selected to type over.
///
/// Call it from the tray's and the shortcut's callbacks and a `--quick-search` launch. Not from inside a
/// callback of Quick Search's own window.
pub fn summon(cx: &mut App) {
    prepare(cx);
    let Some((handle, view, raw)) = cx
        .try_global::<QuickWindow>()
        .map(|w| (w.handle, w.view.clone(), w.hwnd))
    else {
        return;
    };
    // Summoned over an Open or Save As dialog: a folder chosen goes to that dialog. It reads the foreground
    // window, so it comes before this window takes it.
    glint_core::dialogs::note_dialog();
    SHOWN.store(true, Ordering::SeqCst);
    core_bridge::post_to_ui(move || show_now(HWND(raw as *mut _)));
    let _ = handle.update(cx, |_, window, cx| view.update(cx, |quick, cx| quick.shown(window, cx)));
}

/// The shortcut's and the tray's press: shows the window, or puts it away when it is up and in front.
pub fn toggle(cx: &mut App) {
    let up = SHOWN.load(Ordering::SeqCst)
        && cx
            .try_global::<QuickWindow>()
            .is_some_and(|w| in_front(HWND(w.hwnd as *mut _)));
    if up {
        hide(cx);
    } else {
        summon(cx);
    }
}

/// Puts the window away (it stays, hidden). Safe anywhere, including inside GPUI callbacks.
pub fn hide(cx: &mut App) {
    SHOWN.store(false, Ordering::SeqCst);
    if let Some(raw) = cx.try_global::<QuickWindow>().map(|w| w.hwnd) {
        core_bridge::post_to_ui(move || hide_now(HWND(raw as *mut _)));
    }
}

/// Whether the window is up (shown and not put away since).
pub fn is_shown() -> bool {
    SHOWN.load(Ordering::SeqCst)
}

/// Closes the window for good: when Glint closes its main window without the tray keeping it running, the
/// hidden search window mustn't keep it alive (GPUI quits when the last window closes).
pub fn close(cx: &mut App) {
    SHOWN.store(false, Ordering::SeqCst);
    if cx.has_global::<QuickWindow>() {
        let window = cx.remove_global::<QuickWindow>();
        let _ = window.handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Whether `handle` is the Quick Search window, so a "last window closed" check can leave it out.
pub fn is_quick_window(cx: &App, handle: AnyWindowHandle) -> bool {
    cx.try_global::<QuickWindow>().is_some_and(|w| w.handle == handle)
}

// ---- Win32, from the message pump or only reading ---------------------------------------------------------

/// Whether `hwnd` is the window Windows is sending the keyboard to. Only reads.
fn in_front(hwnd: HWND) -> bool {
    // SAFETY: a plain query of the foreground window.
    unsafe { GetForegroundWindow() == hwnd }
}

/// The corner of a `size` window that centers it on `monitor` (left, top, right, bottom). Saturating, so nonsense
/// rectangles can't overflow.
pub(super) fn centered_on(monitor: (i32, i32, i32, i32), width: i32, height: i32) -> (i32, i32) {
    let (left, top, right, bottom) = monitor;
    let x = left.saturating_add(right.saturating_sub(left).saturating_sub(width) / 2);
    let y = top.saturating_add(bottom.saturating_sub(top).saturating_sub(height) / 2);
    (x, y)
}

/// Centers the window on the monitor it is on (`center()`), keeping its size.
fn center(hwnd: HWND) {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid out pointer.
    if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
        return;
    }
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: `info` is a valid MONITORINFO with cbSize set.
    if !unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) }.as_bool() {
        return;
    }
    let screen = info.rcMonitor;
    let (x, y) = centered_on(
        (screen.left, screen.top, screen.right, screen.bottom),
        rect.right.saturating_sub(rect.left),
        rect.bottom.saturating_sub(rect.top),
    );
    // SAFETY: a plain move of our own window; a stale handle just fails.
    let _ = unsafe { SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
}

/// Asks Windows 11 for the rounded corners it gives its own pop-ups (an undecorated window gets none by
/// default). Older Windows ignores it.
fn round_corners(hwnd: HWND) {
    let preference = DWMWCP_ROUND;
    // SAFETY: the attribute's value is a DWM_WINDOW_CORNER_PREFERENCE, passed with its size.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const _ as *const _,
            size_of_val(&preference) as u32,
        )
    };
}

/// Centered, shown and brought forward (`center`, `show`, `bring_forward`). Runs from the message pump.
fn show_now(hwnd: HWND) {
    center(hwnd);
    crate::platform::launch::bring_forward(hwnd);
}

/// Hides the window. Runs from the message pump.
fn hide_now(hwnd: HWND) {
    // SAFETY: hides our own window; a stale handle makes the call fail, nothing else.
    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_centered_on_its_monitor() {
        // A 1920 x 1080 monitor with the window 640 x 440 on it.
        assert_eq!(centered_on((0, 0, 1920, 1080), 640, 440), (640, 320));
        // A second monitor to the left (negative coordinates) and one above.
        assert_eq!(centered_on((-1920, 0, 0, 1080), 640, 440), (-1280, 320));
        assert_eq!(centered_on((0, -1080, 1920, 0), 640, 440), (640, -760));
        // A window bigger than the monitor hangs over evenly, and nonsense can't overflow.
        assert_eq!(centered_on((0, 0, 400, 300), 640, 440), (-120, -70));
        assert_eq!(
            centered_on((i32::MIN, i32::MIN, i32::MAX, i32::MAX), i32::MAX, 0).0,
            i32::MIN
        );
    }
}
