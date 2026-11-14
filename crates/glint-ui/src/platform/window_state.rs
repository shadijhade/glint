//! Window placement restore, the taskbar's Jump List and thumbnail toolbar (ports of placement.rs, jumplist.rs;
//! snap.rs needs no port, see below).
//!
//! Ported from src-tauri/src/placement.rs, src-tauri/src/jumplist.rs and src-tauri/src/snap.rs.
//!
//! - **Placement.** The main window comes back where it was: size, place and whether it was maximized are kept
//!   in `window.json` in Glint's local data folder (`%LOCALAPPDATA%\com.shado.shadui-ripgrep`, the same file the
//!   Tauri build wrote, so an installed Glint's saved place carries over) when the window closes or goes to the
//!   tray, and put back before it first shows. The first time, a window too big for its screen is made to fit.
//!   Unlike the Tauri build, which left a place on a vanished monitor to Windows, [`on_screen`] checks it against
//!   the monitors that are there and moves it onto one.
//! - **Jump List.** The taskbar button's tasks (This PC, Quick search, the Recycle Bin) and its Folders category
//!   (pinned, then most used), exactly as jumplist.rs built them; the taskbar progress bar; and Back, Forward and
//!   Up under the button's preview.
//! - **Snap layouts (snap.rs): not ported.** The Tauri build had to lay an invisible HTMAXBUTTON window over the
//!   page's own maximize button because the page covered the whole window. A GPUI window answers `WM_NCHITTEST`
//!   itself: any element marked `window_control_area(WindowControlArea::Max)` (the kit's title bar buttons are)
//!   returns HTMAXBUTTON, so Windows offers the layouts, and GPUI draws the hover and press states and maximizes.
//!
//! - **Secondary windows** (`platform::secondary`) keep none of this: [`restore_hwnd`], [`save_hwnd`], the Jump
//!   List and the thumbnail buttons do nothing in one, and [`place_secondary`] puts the window where its command
//!   line says (or centres it) instead of where the main window was.
//!
//! **What the coordinator calls** (all on the UI thread unless said):
//!
//! | When | Call |
//! |---|---|
//! | the main window opened (hidden if starting in the tray) | [`restore_window`] |
//! | first shown from the tray | [`show_window`] |
//! | the main window closes, hides to the tray, or the app quits | [`save_window`] |
//! | recent / pinned folders changed, and once at start | [`sync_jump_list_in_background`] |
//! | the folder shown or its history changed | [`set_thumb_buttons`] |
//! | long work starts, advances, ends | [`taskbar_progress`] |
//! | the window closes | [`remove_thumb_buttons`] |

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

// ---- Placement: pure parts --------------------------------------------------------------------------

/// The window's place when it isn't maximized or minimized (Windows' own "normal position", in workspace
/// coordinates, which only Get/SetWindowPlacement read), and whether it was maximized. This is the shape of
/// `window.json`; the Tauri build wrote the same fields.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge.
    pub right: i32,
    /// Bottom edge.
    pub bottom: i32,
    /// It was maximized when put away: it is again as it first shows.
    pub maximized: bool,
}

impl Placement {
    /// A kept place is used only if it has an area: a damaged or hand-edited file is passed over.
    fn is_usable(&self) -> bool {
        self.right > self.left && self.bottom > self.top
    }

    fn rect(&self) -> Rect {
        Rect {
            left: self.left,
            top: self.top,
            right: self.right,
            bottom: self.bottom,
        }
    }
}

/// A rectangle in pixels (left, top, right, bottom): a monitor's work area or a window's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub left: i32,
    /// Top edge.
    pub top: i32,
    /// Right edge.
    pub right: i32,
    /// Bottom edge.
    pub bottom: i32,
}

impl Rect {
    fn width(&self) -> i64 {
        i64::from(self.right) - i64::from(self.left)
    }

    fn height(&self) -> i64 {
        i64::from(self.bottom) - i64::from(self.top)
    }

    /// How much of this lies inside `other`, as (width, height); zero where they don't overlap.
    fn overlap(&self, other: &Rect) -> (i64, i64) {
        let w = i64::from(self.right.min(other.right)) - i64::from(self.left.max(other.left));
        let h = i64::from(self.bottom.min(other.bottom)) - i64::from(self.top.max(other.top));
        (w.max(0), h.max(0))
    }

    /// Squared distance between the centres, for picking the nearest monitor.
    fn centre_distance(&self, other: &Rect) -> i128 {
        let dx = i128::from(self.left) + i128::from(self.right) - i128::from(other.left) - i128::from(other.right);
        let dy = i128::from(self.top) + i128::from(self.bottom) - i128::from(other.top) - i128::from(other.bottom);
        dx * dx + dy * dy
    }
}

/// How much of a restored window must be on some monitor to count as reachable: enough of its title bar to
/// grab (a window parked mostly off-screen by a monitor that went away is not).
const MIN_VISIBLE: (i64, i64) = (120, 48);

/// Where a first window of `size` goes in a screen's work area (left, top, right, bottom), when it's over 90%
/// of it either way: 85% of it, centered, as (x, y, width, height). None when it fits as it is.
pub fn fitted((w, h): (i32, i32), (left, top, right, bottom): (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    // i64 so a nonsense size can't overflow.
    let (w, h) = (i64::from(w), i64::from(h));
    let (room_w, room_h) = (i64::from(right) - i64::from(left), i64::from(bottom) - i64::from(top));
    if w * 10 <= room_w * 9 && h * 10 <= room_h * 9 {
        return None;
    }
    let (w, h) = (w.min(room_w * 85 / 100), h.min(room_h * 85 / 100));
    // Every value is within the work area's i32 range, so the conversions can't fail; the fallback is the edge.
    let to = |v: i64, edge: i32| i32::try_from(v).unwrap_or(edge);
    Some((
        to(i64::from(left) + (room_w - w) / 2, left),
        to(i64::from(top) + (room_h - h) / 2, top),
        to(w, right),
        to(h, bottom),
    ))
}

/// A kept place checked against the monitors there are now (`monitors` are work areas, the primary first).
/// A place that still shows a title bar's worth of window on some monitor is kept as it is; one that doesn't
/// (its monitor was unplugged, or the resolution dropped) is moved onto the nearest monitor, made no bigger
/// than it, and centred there. With no monitors known it is returned as it was.
pub fn on_screen(saved: Rect, monitors: &[Rect]) -> Rect {
    if monitors.iter().any(|m| {
        let (w, h) = saved.overlap(m);
        w >= MIN_VISIBLE.0.min(saved.width()) && h >= MIN_VISIBLE.1.min(saved.height())
    }) {
        return saved;
    }
    let Some(target) = monitors.iter().min_by_key(|m| saved.centre_distance(m)) else {
        return saved;
    };
    let (w, h) = (saved.width().min(target.width()), saved.height().min(target.height()));
    let to = |v: i64, edge: i32| i32::try_from(v).unwrap_or(edge);
    let left = to(i64::from(target.left) + (target.width() - w) / 2, target.left);
    let top = to(i64::from(target.top) + (target.height() - h) / 2, target.top);
    Rect {
        left,
        top,
        right: to(i64::from(left) + w, target.right),
        bottom: to(i64::from(top) + h, target.bottom),
    }
}

/// Where a secondary window of `size` goes: its corner at `at` when given (moved onto a monitor if none shows
/// enough of it to grab), else centred in the first monitor's work area (the primary), made no bigger than it.
/// `monitors` are work areas, the primary first. With no monitors known, `at` as it was, or the origin.
pub fn secondary_rect(size: (i32, i32), at: Option<(i32, i32)>, monitors: &[Rect]) -> Rect {
    let (w, h) = (i64::from(size.0.max(1)), i64::from(size.1.max(1)));
    let to = |v: i64| i32::try_from(v).unwrap_or(i32::MAX);
    let placed = |left: i32, top: i32, w: i64, h: i64| Rect {
        left,
        top,
        right: to(i64::from(left) + w),
        bottom: to(i64::from(top) + h),
    };
    match (at, monitors.first()) {
        (Some((x, y)), _) => on_screen(placed(x, y, w, h), monitors),
        (None, Some(primary)) => {
            let (w, h) = (w.min(primary.width()), h.min(primary.height()));
            let left = to(i64::from(primary.left) + (primary.width() - w) / 2);
            let top = to(i64::from(primary.top) + (primary.height() - h) / 2);
            placed(left, top, w, h)
        }
        (None, None) => placed(0, 0, w, h),
    }
}

/// What to do to the hidden window as it is set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestorePlan {
    /// Put the window's normal place here; show it maximized if the flag says so.
    Place {
        /// The place to set.
        rect: Rect,
        /// Maximize when it first shows.
        maximized: bool,
    },
    /// Nothing kept: fit the window as it is to the screen it opened on, if it is too big (see [`fitted`]).
    Fit,
}

/// Decides how to restore: the kept place when it is usable (moved onto a monitor that exists), else the
/// first-time fit.
pub fn plan_restore(saved: Option<Placement>, monitors: &[Rect]) -> RestorePlan {
    match saved.filter(Placement::is_usable) {
        Some(p) => RestorePlan::Place {
            rect: on_screen(p.rect(), monitors),
            maximized: p.maximized,
        },
        None => RestorePlan::Fit,
    }
}

/// Reads a kept place. A missing, damaged or empty file is None: the window opens as configured.
pub fn load(file: &Path) -> Option<Placement> {
    let bytes = std::fs::read(file).ok()?;
    serde_json::from_slice::<Placement>(&bytes)
        .ok()
        .filter(Placement::is_usable)
}

/// Keeps a place (best effort: a window that can't be remembered still works). Written beside the file first
/// and renamed over it, so a crash mid-write never leaves a half file.
pub fn store(file: &Path, placement: &Placement) {
    let (Some(folder), Ok(json)) = (file.parent(), serde_json::to_vec(placement)) else {
        return;
    };
    let _ = std::fs::create_dir_all(folder);
    let temp = file.with_extension("json.tmp");
    if std::fs::write(&temp, json).is_ok() && std::fs::rename(&temp, file).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

/// Where the place is kept: `window.json` in the local data folder.
pub fn placement_file(data: &Path) -> PathBuf {
    data.join("window.json")
}

/// Glint's local data folder (`%LOCALAPPDATA%\com.shado.shadui-ripgrep`), from the app handle core uses.
pub fn data_dir(app: &glint_core::host::AppHandle) -> Option<PathBuf> {
    app.path().app_local_data_dir().ok()
}

/// Where the place is kept, once [`restore_window`] has been told.
static FILE: OnceLock<PathBuf> = OnceLock::new();
/// The window was maximized when it was put away: it is again as it first shows.
static MAXIMIZE: AtomicBool = AtomicBool::new(false);

// ---- Placement: the window ---------------------------------------------------------------------------

/// [`restore_window`] for a bare handle.
pub fn restore_hwnd(hwnd: windows::Win32::Foundation::HWND, data: &Path) {
    // A secondary window isn't put where the main window was.
    if crate::platform::secondary::is_secondary() {
        return;
    }
    let file = FILE.get_or_init(|| placement_file(data));
    imp::restore(hwnd, load(file));
}

/// Puts a secondary window (hidden, as it opens) where its command line says, or centred, in the size it says
/// (the size it opened with when none) (see [`secondary_rect`]). Only the window's own place is set: nothing is
/// kept for next time.
pub fn place_secondary(hwnd: windows::Win32::Foundation::HWND, size: Option<(i32, i32)>, at: Option<(i32, i32)>) {
    imp::place_secondary(hwnd, size, at);
}

/// [`show_window`] for a bare handle.
pub fn show_hwnd(hwnd: windows::Win32::Foundation::HWND) {
    imp::show(hwnd, MAXIMIZE.swap(false, Ordering::SeqCst));
}

/// [`save_window`] for a bare handle (the quit path may only have the handle).
pub fn save_hwnd(hwnd: windows::Win32::Foundation::HWND) {
    // The place is the main window's to keep.
    if crate::platform::secondary::is_secondary() {
        return;
    }
    let Some(file) = FILE.get() else { return };
    if let Some(placement) = imp::placement(hwnd) {
        store(file, &placement);
    }
}

// ---- Jump list: pure parts ---------------------------------------------------------------------------

/// The most folders the list shows (Windows may allow fewer: its own setting).
const FOLDERS: usize = 10;

/// One link in the list: what it starts (the arguments after Glint's exe), and how it looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSpec {
    /// The command-line arguments.
    pub args: String,
    /// The title in the list.
    pub title: String,
    /// The tooltip.
    pub description: String,
    /// The file whose icon it shows.
    pub icon_path: String,
    /// The icon's index in that file.
    pub icon_index: i32,
}

/// A path as one command-line argument: in quotes, with a trailing backslash doubled, as `"C:\"` would read
/// as `C:"`.
pub fn quoted(path: &str) -> String {
    let backslashes = path.len() - path.trim_end_matches('\\').len();
    format!("\"{path}{}\"", "\\".repeat(backslashes))
}

/// A file in System32, wherever Windows is.
fn system32(file: &str) -> String {
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    format!(r"{windows}\System32\{file}")
}

/// The Jump List's Tasks: This PC, Quick search and the Recycle Bin. `own_icon` is Glint's own exe, whose icon
/// the first two show; `system32` is Windows' System32 folder (for imageres.dll's Recycle Bin icon).
pub fn task_links(own_icon: &str, system32: &str) -> Vec<LinkSpec> {
    vec![
        LinkSpec {
            args: r#""this-pc:""#.into(),
            title: "This PC".into(),
            description: "Open This PC in Glint".into(),
            icon_path: own_icon.into(),
            icon_index: 0,
        },
        LinkSpec {
            args: "--quick-search".into(),
            title: "Quick search".into(),
            description: "Search every drive".into(),
            icon_path: own_icon.into(),
            icon_index: 0,
        },
        LinkSpec {
            args: r#""shell:RecycleBinFolder""#.into(),
            title: "Recycle Bin".into(),
            description: "Open the Recycle Bin in Glint".into(),
            icon_path: format!(r"{system32}\imageres.dll"),
            icon_index: -55,
        },
    ]
}

/// The Folders category: the pinned folders, then the most used ones (those only when Windows shows recently
/// opened items), without repeats, without the ones the user removed from the list (`removed` holds their
/// arguments: one added again makes Windows refuse the whole category) and without folders that are gone
/// (`is_dir`), up to what the list has room for (`slots`, which Windows reports; between 4 and 10 are used).
pub fn folder_links(
    pinned: &[String],
    frequent: &[String],
    shows_recent: bool,
    slots: u32,
    removed: &[String],
    is_dir: impl Fn(&str) -> bool,
    shell32: &str,
) -> Vec<LinkSpec> {
    let frequent: &[String] = if shows_recent { frequent } else { &[] };
    let most = usize::try_from(slots).unwrap_or(FOLDERS).clamp(4, FOLDERS);
    let mut added: Vec<LinkSpec> = Vec::new();
    for folder in pinned.iter().chain(frequent) {
        let args = quoted(folder);
        if added.len() == most || added.iter().any(|l| l.args == args) || removed.contains(&args) || !is_dir(folder) {
            continue;
        }
        let title = Path::new(folder)
            .file_name()
            .map_or_else(|| folder.clone(), |n| n.to_string_lossy().into_owned());
        added.push(LinkSpec {
            args,
            title,
            description: folder.clone(),
            icon_path: shell32.into(),
            icon_index: 3,
        });
    }
    added
}

/// The state the taskbar progress bar can be in, by the names the page used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressState {
    /// No bar.
    None,
    /// A marquee, no value.
    Indeterminate,
    /// Green, with a value.
    Normal,
    /// Yellow.
    Paused,
    /// Red.
    Error,
}

impl ProgressState {
    /// "indeterminate", "normal", "paused" and "error"; anything else is no bar.
    pub fn parse(name: &str) -> Self {
        match name {
            "indeterminate" => Self::Indeterminate,
            "normal" => Self::Normal,
            "paused" => Self::Paused,
            "error" => Self::Error,
            _ => Self::None,
        }
    }

    /// Whether a completed/total value is shown in this state.
    fn shows_value(self) -> bool {
        !matches!(self, Self::None | Self::Indeterminate)
    }
}

/// The progress value to set: `(completed, total)` with completed capped at total, or None when the state
/// shows no value or total is zero.
pub fn progress_value(state: ProgressState, completed: Option<u64>, total: Option<u64>) -> Option<(u64, u64)> {
    match (completed, total) {
        (Some(c), Some(t)) if t > 0 && state.shows_value() => Some((c.min(t), t)),
        _ => None,
    }
}

/// A button under the taskbar preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbButton {
    /// Back in the folder history.
    Back,
    /// Forward in the folder history.
    Forward,
    /// To the parent folder.
    Up,
}

impl ThumbButton {
    /// The name the page's "thumb-button" event carried.
    pub fn name(self) -> &'static str {
        match self {
            Self::Back => "back",
            Self::Forward => "forward",
            Self::Up => "up",
        }
    }
}

/// The preview's buttons in order: the button, its glyph in Segoe Fluent Icons (and Segoe MDL2 Assets, Windows
/// 10's), and its tooltip.
const BUTTONS: [(ThumbButton, u16, &str); 3] = [
    (ThumbButton::Back, 0xE72B, "Back"),
    (ThumbButton::Forward, 0xE72A, "Forward"),
    (ThumbButton::Up, 0xE74A, "Up"),
];
/// Their ids in the window's WM_COMMAND.
const FIRST_ID: u32 = 0x4C47;
/// The notification a thumbnail button's click comes with (THBN_CLICKED).
const CLICKED: usize = 0x1800;

/// Which button a WM_COMMAND's WPARAM is a click of, if any.
pub fn clicked_button(wparam: usize) -> Option<ThumbButton> {
    if (wparam >> 16) & 0xFFFF != CLICKED {
        return None;
    }
    let id = u32::try_from(wparam & 0xFFFF).ok()?;
    let index = usize::try_from(id.checked_sub(FIRST_ID)?).ok()?;
    BUTTONS.get(index).map(|b| b.0)
}

/// Clicks of the preview's buttons arrive here, from the window's own message handling. Await
/// `thumb_button_events().recv()` in a GPUI task and act on the workspace there (never inside the handler).
pub fn thumb_button_events() -> async_channel::Receiver<ThumbButton> {
    thumb_channel().1.clone()
}

fn thumb_channel() -> &'static (async_channel::Sender<ThumbButton>, async_channel::Receiver<ThumbButton>) {
    static CHANNEL: OnceLock<(async_channel::Sender<ThumbButton>, async_channel::Receiver<ThumbButton>)> =
        OnceLock::new();
    CHANNEL.get_or_init(async_channel::unbounded)
}

/// The most recent Jump List sync is remembered so two syncs don't run at once (the second waits).
static SYNC: Mutex<()> = Mutex::new(());

// ---- Jump list: Windows ------------------------------------------------------------------------------

/// Rebuilds the taskbar button's Jump List: Tasks, then Folders (`pinned`, then `frequent`). Blocks (it runs
/// on a fresh COM apartment), so call it from a background thread, or use [`sync_jump_list_in_background`].
/// The most-used folders stay out when Windows is set not to show recently opened items.
pub fn sync_jump_list(pinned: Vec<String>, frequent: Vec<String>) -> Result<(), String> {
    // The Jump List is the main window's.
    if crate::platform::secondary::is_secondary() {
        return Ok(());
    }
    let _turn = SYNC.lock().unwrap_or_else(|e| e.into_inner());
    imp::sync_jump_list(&pinned, &frequent)
}

/// [`sync_jump_list`] on its own thread, for the UI thread to call when recent or pinned folders change. Best
/// effort: the taskbar never fails what the user is doing, so an error is only printed.
pub fn sync_jump_list_in_background(pinned: Vec<String>, frequent: Vec<String>) {
    if crate::platform::secondary::is_secondary() {
        return;
    }
    std::thread::spawn(move || {
        if let Err(error) = sync_jump_list(pinned, frequent) {
            eprintln!("glint: couldn't update the Jump List: {error}");
        }
    });
}

/// Sets the taskbar button's progress bar for `window`. Best effort. Call on the UI thread.
pub fn taskbar_progress(window: &gpui_kit::Window, state: ProgressState, completed: Option<u64>, total: Option<u64>) {
    if let Some(hwnd) = crate::core_bridge::hwnd_of(window) {
        imp::taskbar_progress(hwnd, state, completed, total);
    }
}

/// [`set_thumb_buttons`] for a bare handle (the start-up glue posts it, outside GPUI's callbacks).
pub fn set_thumb_buttons_hwnd(hwnd: windows::Win32::Foundation::HWND, back: bool, forward: bool, up: bool) {
    if crate::platform::secondary::is_secondary() {
        return;
    }
    imp::thumb_buttons(hwnd, [back, forward, up]);
}

mod imp {
    use super::{
        clicked_button, folder_links, progress_value, system32, task_links, thumb_channel, LinkSpec, Placement,
        ProgressState, Rect, RestorePlan, BUTTONS, FIRST_ID,
    };
    use std::path::{Path, PathBuf};
    use windows::core::{w, Interface, BOOL, GUID, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, PROPERTYKEY, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateBitmap, CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW,
        EnumDisplayMonitors, GdiFlush, GetMonitorInfoW, GetTextFaceW, MonitorFromWindow, SelectObject, SetBkMode,
        SetTextColor, ANTIALIASED_QUALITY, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET,
        DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, HDC, HGDIOBJ,
        HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, TRANSPARENT,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
        COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{
        DefSubclassProc, DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW,
        ITaskbarList3, RemoveWindowSubclass, SetWindowSubclass, ShellLink, TaskbarList, TBPF_ERROR, TBPF_INDETERMINATE,
        TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED, THBF_DISABLED, THBF_ENABLED, THB_FLAGS, THB_ICON, THB_TOOLTIP,
        THUMBBUTTON,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, GetWindowPlacement, GetWindowRect, IsWindowVisible, RegisterWindowMessageW,
        SetWindowPlacement, SetWindowPos, ShowWindow, HICON, ICONINFO, SWP_NOACTIVATE, SWP_NOZORDER, SW_HIDE, SW_SHOW,
        SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SW_SHOWNORMAL, WINDOWPLACEMENT, WM_COMMAND, WM_NCDESTROY, WM_SETTINGCHANGE,
        WPF_RESTORETOMAXIMIZED,
    };

    /// MONITORINFO.dwFlags bit for the primary monitor (MONITORINFOF_PRIMARY, which the crate does not export).
    const MONITOR_PRIMARY: u32 = 1;

    // ---- Placement ----

    fn current(hwnd: HWND) -> Option<WINDOWPLACEMENT> {
        let mut placement = WINDOWPLACEMENT {
            length: size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: `placement` is a valid WINDOWPLACEMENT with its length set; a stale handle just fails.
        unsafe { GetWindowPlacement(hwnd, &mut placement) }
            .ok()
            .map(|_| placement)
    }

    /// The monitors' work areas, the primary first.
    fn monitors() -> Vec<Rect> {
        unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the address of the `Vec` below, alive for the whole enumeration, used on this thread.
            let list = unsafe { &mut *(data.0 as *mut Vec<(bool, Rect)>) };
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            // SAFETY: `info` is a valid MONITORINFO with cbSize set.
            if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
                let w = info.rcWork;
                let primary = info.dwFlags & MONITOR_PRIMARY != 0;
                list.push((
                    primary,
                    Rect {
                        left: w.left,
                        top: w.top,
                        right: w.right,
                        bottom: w.bottom,
                    },
                ));
            }
            true.into()
        }
        let mut list: Vec<(bool, Rect)> = Vec::new();
        // SAFETY: the callback only touches `list`, which outlives the call.
        let _ = unsafe { EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut list as *mut _ as isize)) };
        list.sort_by_key(|(primary, _)| !*primary);
        list.into_iter().map(|(_, rect)| rect).collect()
    }

    pub fn restore(hwnd: HWND, saved: Option<Placement>) {
        match super::plan_restore(saved, &monitors()) {
            RestorePlan::Fit => fit(hwnd),
            RestorePlan::Place { rect, maximized } => {
                let Some(mut placement) = current(hwnd) else { return };
                // SAFETY: a plain visibility query on a window handle.
                let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();
                // Hidden stays hidden until `show`; a visible window takes the place at once.
                placement.showCmd = match (visible, maximized) {
                    (false, _) => SW_HIDE.0 as u32,
                    (true, true) => SW_SHOWMAXIMIZED.0 as u32,
                    (true, false) => SW_SHOWNORMAL.0 as u32,
                };
                placement.rcNormalPosition = RECT {
                    left: rect.left,
                    top: rect.top,
                    right: rect.right,
                    bottom: rect.bottom,
                };
                // SAFETY: `placement` came from GetWindowPlacement with its fields updated.
                if unsafe { SetWindowPlacement(hwnd, &placement) }.is_ok() && !visible {
                    super::MAXIMIZE.store(maximized, std::sync::atomic::Ordering::SeqCst);
                }
            }
        }
    }

    /// A secondary window's place: [`super::secondary_rect`] over the monitors there are, set as a hidden window's
    /// normal place (so it shows there), not maximized.
    pub fn place_secondary(hwnd: HWND, size: Option<(i32, i32)>, at: Option<(i32, i32)>) {
        let size = size.or_else(|| {
            let mut rect = RECT::default();
            // SAFETY: `rect` is a valid out pointer.
            unsafe { GetWindowRect(hwnd, &mut rect) }
                .ok()
                .map(|_| (rect.right - rect.left, rect.bottom - rect.top))
        });
        let Some(size) = size else { return };
        let rect = super::secondary_rect(size, at, &monitors());
        restore(
            hwnd,
            Some(Placement {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
                maximized: false,
            }),
        );
    }

    /// The first time: as configured (1280 x 800 at 100%), unless that's over 90% of the screen's work area
    /// either way (a 1080p laptop at 150% is 1280 x 720), then 85% of it, centered.
    fn fit(hwnd: HWND) {
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
        let work = info.rcWork;
        if let Some((x, y, w, h)) = super::fitted(
            (rect.right - rect.left, rect.bottom - rect.top),
            (work.left, work.top, work.right, work.bottom),
        ) {
            // SAFETY: plain move and size of a window handle; failure is ignored.
            let _ = unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }

    pub fn show(hwnd: HWND, maximize: bool) {
        // SAFETY: plain show of a window handle; a stale handle just fails.
        unsafe {
            let _ = ShowWindow(hwnd, if maximize { SW_SHOWMAXIMIZED } else { SW_SHOW });
        }
    }

    pub fn placement(hwnd: HWND) -> Option<Placement> {
        // SAFETY: a plain visibility query on a window handle.
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return None;
        }
        let placement = current(hwnd)?;
        let r = placement.rcNormalPosition;
        let minimized_from_maximized =
            placement.showCmd == SW_SHOWMINIMIZED.0 as u32 && placement.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0;
        let maximized = placement.showCmd == SW_SHOWMAXIMIZED.0 as u32 || minimized_from_maximized;
        Some(Placement {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
            maximized,
        })
    }

    // ---- Registry (HKCU DWORDs) ----

    fn reg_dword(path: &str, name: &str) -> Option<u32> {
        let (path, name) = (HSTRING::from(path), HSTRING::from(name));
        let mut data = 0u32;
        let mut size = size_of::<u32>() as u32;
        // SAFETY: `data` and `size` are valid out pointers for a REG_DWORD (4 bytes).
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &path,
                &name,
                RRF_RT_REG_DWORD,
                None,
                Some((&mut data as *mut u32).cast()),
                Some(&mut size),
            )
        };
        status.is_ok().then_some(data)
    }

    /// Windows' "Show recently opened items in Start, Jump Lists and File Explorer": on unless the DWORD is 0.
    fn shows_recent() -> bool {
        reg_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
            "Start_TrackDocs",
        ) != Some(0)
    }

    /// Windows' own parts (the taskbar, its previews) are light: Settings, Personalization, Colors.
    fn taskbar_is_light() -> bool {
        reg_dword(
            r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
            "SystemUsesLightTheme",
        )
        .is_some_and(|v| v != 0)
    }

    // ---- The exe the list's entries start ----

    /// The exe other programs (the Jump List) start Glint with: the installed one, or the Microsoft Store copy's
    /// alias, which stays put as updates come, or this one. Mirrors glint-core's `verbs::launch_exe`.
    // TODO(coordinator): from glint-core (`verbs::launch_exe` is pub(crate)).
    fn launch_exe() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let installed = if glint_core::installed_by_setup() {
            Some(exe.clone())
        } else {
            scoop_current(&exe)
        };
        installed.or_else(store_alias).or(Some(exe))
    }

    /// The Microsoft Store copy's `glint.exe` alias, in the package's own folder of aliases.
    fn store_alias() -> Option<PathBuf> {
        let family = glint_core::package_family()?;
        let alias = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join(r"Microsoft\WindowsApps")
            .join(family)
            .join("glint.exe");
        // An alias is a link Windows resolves itself, so it's looked at, not through.
        alias.symlink_metadata().is_ok().then_some(alias)
    }

    /// For an exe Scoop installed (`<root>\apps\<app>\<version>\Glint.exe`), the same exe through the app's
    /// `current` link, which `scoop update` points at each new version.
    fn scoop_current(exe: &Path) -> Option<PathBuf> {
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

    // ---- Jump list ----

    const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
        pid: 2,
    };

    fn make_link(exe: &Path, spec: &LinkSpec) -> Option<IShellLinkW> {
        // SAFETY: COM calls on this thread's apartment with strings that outlive each call.
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let exe_w = HSTRING::from(exe.as_os_str());
            let args_w = HSTRING::from(spec.args.as_str());
            let desc_w = HSTRING::from(spec.description.as_str());
            let icon_w = HSTRING::from(spec.icon_path.as_str());
            link.SetPath(PCWSTR(exe_w.as_ptr())).ok()?;
            link.SetArguments(PCWSTR(args_w.as_ptr())).ok()?;
            link.SetDescription(PCWSTR(desc_w.as_ptr())).ok()?;
            link.SetIconLocation(PCWSTR(icon_w.as_ptr()), spec.icon_index).ok()?;
            let store: IPropertyStore = link.cast().ok()?;
            store
                .SetValue(&PKEY_TITLE, &PROPVARIANT::from(spec.title.as_str()))
                .ok()?;
            store.Commit().ok()?;
            Some(link)
        }
    }

    /// The arguments of the links the user removed from the list.
    fn removed(items: &IObjectArray) -> Vec<String> {
        // SAFETY: COM calls on valid interfaces; `args` is a buffer of the length passed.
        unsafe {
            (0..items.GetCount().unwrap_or(0))
                .filter_map(|i| items.GetAt::<IShellLinkW>(i).ok())
                .filter_map(|link| {
                    let mut args = [0u16; 1024];
                    link.GetArguments(&mut args).ok()?;
                    let len = args.iter().position(|&c| c == 0).unwrap_or(args.len());
                    Some(String::from_utf16_lossy(&args[..len]))
                })
                .collect()
        }
    }

    /// Runs `f` on a fresh single-threaded COM apartment (the Jump List's COM objects want one).
    // TODO(coordinator): from glint-core (`shell::in_sta` is pub(crate)).
    fn in_sta<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
        std::thread::spawn(move || {
            // SAFETY: balanced with CoUninitialize below, on the same thread.
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }
                .ok()
                .map_err(|e| e.message())?;
            let result = f();
            // SAFETY: matches the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
            result
        })
        .join()
        .map_err(|_| "The Jump List update failed unexpectedly".to_string())?
    }

    pub fn sync_jump_list(pinned: &[String], frequent: &[String]) -> Result<(), String> {
        // What its entries start (a Microsoft Store copy's alias), and the exe whose icon they show.
        let exe = launch_exe().ok_or("Couldn't tell where Glint is")?;
        let icon = std::env::current_exe().map_err(|e| e.to_string())?;
        let (pinned, frequent, recent) = (pinned.to_vec(), frequent.to_vec(), shows_recent());
        in_sta(move || {
            let icon_str = icon.to_string_lossy().into_owned();
            let sys = system32("");
            let sys = sys
                .trim_end_matches('\\')
                .trim_end_matches("System32")
                .trim_end_matches('\\')
                .to_string();
            let system_dir = format!(r"{sys}\System32");
            // SAFETY: COM calls on this thread's apartment, on interfaces made here.
            unsafe {
                let list: ICustomDestinationList =
                    CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
                let mut slots = 0u32;
                let removed = removed(&list.BeginList::<IObjectArray>(&mut slots).map_err(|e| e.message())?);

                if let Ok(tasks) =
                    CoCreateInstance::<_, IObjectCollection>(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)
                {
                    for spec in task_links(&icon_str, &system_dir) {
                        if let Some(link) = make_link(&exe, &spec) {
                            let _ = tasks.AddObject(&link);
                        }
                    }
                    if let Ok(array) = tasks.cast::<IObjectArray>() {
                        let _ = list.AddUserTasks(&array);
                    }
                }

                if let Ok(places) =
                    CoCreateInstance::<_, IObjectCollection>(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)
                {
                    let shell32 = format!(r"{system_dir}\shell32.dll");
                    let folders = folder_links(
                        &pinned,
                        &frequent,
                        recent,
                        slots,
                        &removed,
                        |f| Path::new(f).is_dir(),
                        &shell32,
                    );
                    let mut any = false;
                    for spec in &folders {
                        if let Some(link) = make_link(&exe, spec) {
                            let _ = places.AddObject(&link);
                            any = true;
                        }
                    }
                    if any {
                        if let Ok(array) = places.cast::<IObjectArray>() {
                            let _ = list.AppendCategory(w!("Folders"), &array);
                        }
                    }
                }
                list.CommitList().map_err(|e| e.message())
            }
        })
    }

    // ---- Taskbar progress ----

    pub fn taskbar_progress(hwnd: HWND, state: ProgressState, completed: Option<u64>, total: Option<u64>) {
        // SAFETY: COM calls on the UI thread (OLE is initialized by GPUI) with a window handle.
        unsafe {
            let Ok(bar) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) else {
                return;
            };
            let _ = bar.HrInit();
            let flag = match state {
                ProgressState::Indeterminate => TBPF_INDETERMINATE,
                ProgressState::Normal => TBPF_NORMAL,
                ProgressState::Paused => TBPF_PAUSED,
                ProgressState::Error => TBPF_ERROR,
                ProgressState::None => TBPF_NOPROGRESS,
            };
            let _ = bar.SetProgressState(hwnd, flag);
            if let Some((c, t)) = progress_value(state, completed, total) {
                let _ = bar.SetProgressValue(hwnd, c, t);
            }
        }
    }

    // ---- Thumbnail toolbar ----

    /// The buttons' state, on the window's own (UI) thread.
    #[derive(Default)]
    struct Thumbs {
        enabled: [bool; 3],
        /// They're on the taskbar button there is now (Explorer restarting, or the window coming back from the
        /// tray, makes a new one).
        added: bool,
        /// Icons drawn for a taskbar this light or dark, at this size.
        icons: Vec<isize>,
        drawn: Option<(bool, i32)>,
        /// The window the subclass is on.
        subclassed: bool,
    }

    thread_local! {
        static THUMBS: std::cell::RefCell<Thumbs> = std::cell::RefCell::new(Thumbs::default());
    }

    /// This module's subclass of the window.
    const SUBCLASS: usize = 0x676C_7462;

    static BUTTON_CREATED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

    pub fn thumb_buttons(hwnd: HWND, enabled: [bool; 3]) {
        let first = THUMBS.with_borrow_mut(|t| {
            t.enabled = enabled;
            !std::mem::replace(&mut t.subclassed, true)
        });
        if first {
            // SAFETY: registering a message name and subclassing a window of this thread; `thumb_messages` has
            // the required signature and stays valid for the process.
            unsafe {
                let _ = BUTTON_CREATED.set(RegisterWindowMessageW(w!("TaskbarButtonCreated")));
                let _ = SetWindowSubclass(hwnd, Some(thumb_messages), SUBCLASS, 0);
            }
        }
        show_thumbs(hwnd);
    }

    fn forget_thumbs() {
        THUMBS.with_borrow_mut(|t| {
            for old in std::mem::take(&mut t.icons) {
                if old != 0 {
                    // SAFETY: an icon this module made and still owns.
                    let _ = unsafe { DestroyIcon(HICON(old as *mut _)) };
                }
            }
            *t = Thumbs::default();
        });
    }

    /// Puts the buttons on the taskbar button, or brings them up to date there.
    fn show_thumbs(hwnd: HWND) {
        // SAFETY: COM calls on the UI thread with a window handle; icons are owned by `THUMBS` until replaced.
        unsafe {
            let Ok(bar) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) else {
                return;
            };
            if bar.HrInit().is_err() {
                return;
            }
            // Built under the borrow, handed to the taskbar after it ends: Windows can send this window messages
            // during the calls, and `thumb_messages` borrows THUMBS again for them.
            let (buttons, added) = THUMBS.with_borrow_mut(|t| {
                let light = taskbar_is_light();
                let dpi = GetDpiForWindow(hwnd);
                let size = if dpi == 0 {
                    16
                } else {
                    i32::try_from((16 * u64::from(dpi) + 48) / 96).unwrap_or(16)
                };
                if t.drawn != Some((light, size)) {
                    let color = if light { 0x1F1F1F } else { 0xFFFFFF };
                    let icons: Vec<isize> = BUTTONS
                        .iter()
                        .map(|&(_, glyph, _)| glyph_icon(glyph, size, color).map_or(0, |i| i.0 as isize))
                        .collect();
                    for old in std::mem::replace(&mut t.icons, icons) {
                        if old != 0 {
                            let _ = DestroyIcon(HICON(old as *mut _));
                        }
                    }
                    t.drawn = Some((light, size));
                }
                let buttons: Vec<THUMBBUTTON> = BUTTONS
                    .iter()
                    .enumerate()
                    .map(|(i, &(_, _, tip))| {
                        let mut button = THUMBBUTTON {
                            dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
                            iId: FIRST_ID + i as u32,
                            hIcon: HICON(t.icons.get(i).copied().unwrap_or(0) as *mut _),
                            dwFlags: if t.enabled[i] { THBF_ENABLED } else { THBF_DISABLED },
                            ..Default::default()
                        };
                        for (to, from) in button.szTip.iter_mut().zip(tip.encode_utf16()) {
                            *to = from;
                        }
                        button
                    })
                    .collect();
                (buttons, t.added)
            });
            if added {
                let _ = bar.ThumbBarUpdateButtons(hwnd, &buttons);
            } else {
                // Before the taskbar button is made this fails: TaskbarButtonCreated brings them then.
                let ok = bar.ThumbBarAddButtons(hwnd, &buttons).is_ok();
                THUMBS.with_borrow_mut(|t| t.added = ok);
            }
        }
    }

    /// The window's messages: a button's click, a new taskbar button, and a change of colors. It runs inside the
    /// window procedure, so a click is only queued ([`super::thumb_button_events`]), never acted on here.
    unsafe extern "system" fn thumb_messages(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _: usize,
        _: usize,
    ) -> LRESULT {
        if msg == WM_COMMAND {
            if let Some(button) = clicked_button(wparam.0) {
                let _ = thumb_channel().0.try_send(button);
                return LRESULT(0);
            }
        } else if BUTTON_CREATED.get() == Some(&msg) {
            THUMBS.with_borrow_mut(|t| t.added = false);
            show_thumbs(hwnd);
        } else if msg == WM_SETTINGCHANGE && lparam.0 != 0 {
            // SAFETY: for WM_SETTINGCHANGE, a non-zero LPARAM is a NUL-terminated string from the sender.
            let area = unsafe { PCWSTR(lparam.0 as *const u16).to_string() };
            if area.is_ok_and(|s| s == "ImmersiveColorSet") && THUMBS.with_borrow(|t| t.added) {
                // Light or dark: drawn again in the taskbar's colors.
                show_thumbs(hwnd);
            }
        } else if msg == WM_NCDESTROY {
            // SAFETY: removing this module's own subclass while it runs, as the documentation advises.
            unsafe {
                let _ = RemoveWindowSubclass(hwnd, Some(thumb_messages), SUBCLASS);
            }
            forget_thumbs();
        }
        // SAFETY: forwarding to the next procedure in the chain with the arguments we were given.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    /// A glyph of Windows' icon font, `size` pixels square in `color` (0xRRGGBB), as top-down BGRA pixels:
    /// drawn white on black without ClearType, so each pixel's brightness is how much of it the glyph covers,
    /// which becomes its alpha.
    pub(super) fn glyph_pixels(glyph: u16, size: i32, color: u32) -> Option<Vec<u8>> {
        if size <= 0 {
            return None;
        }
        // SAFETY: GDI calls on a memory DC made here; every object selected into it is deselected and deleted
        // before returning, and `bits` (the DIB's pixels) is read only while the bitmap is alive.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &top_down(size), DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old_bitmap = SelectObject(dc, HGDIOBJ(bitmap.0));
            let mut drawn = false;
            for face in [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")] {
                let font = CreateFontW(
                    -(size * 4 / 5),
                    0,
                    0,
                    0,
                    FW_NORMAL.0 as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET,
                    OUT_DEFAULT_PRECIS,
                    CLIP_DEFAULT_PRECIS,
                    ANTIALIASED_QUALITY,
                    DEFAULT_PITCH.0 as u32,
                    face,
                );
                let old_font = SelectObject(dc, HGDIOBJ(font.0));
                let mut chosen = [0u16; 64];
                let len = (GetTextFaceW(dc, Some(&mut chosen)).max(1) as usize - 1).min(chosen.len());
                // GDI picks another font for one that isn't there: only the icon font itself will do.
                if face.as_wide() == &chosen[..len] {
                    SetTextColor(dc, COLORREF(0xFFFFFF));
                    SetBkMode(dc, TRANSPARENT);
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: size,
                        bottom: size,
                    };
                    DrawTextW(
                        dc,
                        &mut [glyph],
                        &mut rect,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                    );
                    drawn = true;
                }
                SelectObject(dc, old_font);
                let _ = DeleteObject(HGDIOBJ(font.0));
                if drawn {
                    break;
                }
            }
            let _ = GdiFlush();
            let pixels = drawn.then(|| {
                let (r, g, b) = ((color >> 16) as u8, (color >> 8) as u8, color as u8);
                let count = (size as usize) * (size as usize) * 4;
                let drawn = std::slice::from_raw_parts(bits as *const u8, count);
                drawn
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|p| [b, g, r, p[0].max(p[1]).max(p[2])])
                    .collect()
            });
            SelectObject(dc, old_bitmap);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(dc);
            pixels
        }
    }

    /// A 32-bit top-down DIB's header, `size` pixels square.
    fn top_down(size: i32) -> BITMAPINFO {
        BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// `glyph_pixels` as an icon.
    pub(super) fn glyph_icon(glyph: u16, size: i32, color: u32) -> Option<HICON> {
        let pixels = glyph_pixels(glyph, size, color)?;
        // SAFETY: the DIB is `size` x `size` x 4 bytes and `pixels` is exactly that long; both bitmaps are
        // deleted after the icon has copied them.
        unsafe {
            let mut bits = std::ptr::null_mut();
            let color = CreateDIBSection(None, &top_down(size), DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
            let mask = CreateBitmap(size, size, 1, 1, None);
            let icon = CreateIconIndirect(&ICONINFO {
                fIcon: true.into(),
                hbmMask: mask,
                hbmColor: color,
                ..Default::default()
            })
            .ok();
            let _ = DeleteObject(HGDIOBJ(mask.0));
            let _ = DeleteObject(HGDIOBJ(color.0));
            icon
        }
    }

    #[cfg(test)]
    pub(super) fn first_glyph() -> u16 {
        BUTTONS[0].1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn a_secondary_window_is_centred_or_put_at_its_corner_and_kept_on_a_monitor() {
        let primary = rect(0, 0, 1920, 1080);
        let second = rect(1920, 0, 3840, 1080);
        // Centred in the primary monitor's work area.
        assert_eq!(
            secondary_rect((1000, 600), None, &[primary, second]),
            rect(460, 240, 1460, 840)
        );
        // At the corner it was dropped at, wherever a monitor shows it.
        assert_eq!(
            secondary_rect((1000, 600), Some((2000, 100)), &[primary, second]),
            rect(2000, 100, 3000, 700)
        );
        // A corner no monitor shows (it was unplugged since) goes onto one, whole.
        let moved = secondary_rect((1000, 600), Some((9000, 9000)), &[primary]);
        assert_eq!((moved.right - moved.left, moved.bottom - moved.top), (1000, 600));
        assert!(moved.left >= 0 && moved.top >= 0 && moved.right <= 1920 && moved.bottom <= 1080);
        // Bigger than the screen: no bigger than it.
        assert_eq!(secondary_rect((5000, 3000), None, &[primary]), primary);
        // With no monitor known, as asked.
        assert_eq!(secondary_rect((800, 600), Some((5, 6)), &[]), rect(5, 6, 805, 606));
        assert_eq!(secondary_rect((800, 600), None, &[]), rect(0, 0, 800, 600));
        // A nonsense size is still a window.
        let tiny = secondary_rect((0, -4), None, &[primary]);
        assert_eq!((tiny.right - tiny.left, tiny.bottom - tiny.top), (1, 1));
    }

    #[test]
    fn a_secondary_window_keeps_no_place_and_builds_no_jump_list() {
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\"));
        let nowhere = windows::Win32::Foundation::HWND(std::ptr::null_mut());
        restore_hwnd(nowhere, Path::new(r"C:\glint-no-such-data-folder"));
        assert!(FILE.get().is_none(), "the place file was never even named");
        save_hwnd(nowhere);
        // The real Jump List would be rebuilt here; a secondary window leaves it to the main one.
        assert_eq!(sync_jump_list(vec![r"C:\Pinned".into()], vec![]), Ok(()));
        sync_jump_list_in_background(vec![r"C:\Pinned".into()], vec![]);
        set_thumb_buttons_hwnd(nowhere, true, true, true);
    }

    #[test]
    fn reads_what_it_keeps() {
        let placement = Placement {
            left: -1900,
            top: 40,
            right: -300,
            bottom: 1040,
            maximized: true,
        };
        let json = serde_json::to_vec(&placement).unwrap_or_default();
        assert_eq!(serde_json::from_slice::<Placement>(&json).ok(), Some(placement));
        // The Tauri build's file has exactly these fields.
        assert_eq!(
            serde_json::from_str::<Placement>(r#"{"left":1,"top":2,"right":3,"bottom":4,"maximized":false}"#).ok(),
            Some(Placement {
                left: 1,
                top: 2,
                right: 3,
                bottom: 4,
                maximized: false
            })
        );
        // A damaged file is passed over: the window opens as configured.
        assert!(serde_json::from_slice::<Placement>(b"{\"left\":1}").is_err());
    }

    #[test]
    fn keeps_and_loads_a_place_through_a_file() {
        let folder = std::env::temp_dir().join(format!("glint-window-state-{}", std::process::id()));
        let file = placement_file(&folder);
        assert_eq!(load(&file), None);
        let placement = Placement {
            left: 10,
            top: 20,
            right: 1290,
            bottom: 820,
            maximized: false,
        };
        store(&file, &placement);
        assert_eq!(load(&file), Some(placement));
        // An empty area is passed over, as is garbage.
        store(
            &file,
            &Placement {
                left: 5,
                top: 5,
                right: 5,
                bottom: 9,
                maximized: true,
            },
        );
        assert_eq!(load(&file), None);
        let _ = std::fs::write(&file, b"not json");
        assert_eq!(load(&file), None);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn fits_a_first_window_to_small_screens() {
        // 1280 x 800 at 150% is 1920 x 1200 pixels; a 1080p laptop's work area is 1920 x 1032.
        assert_eq!(fitted((1920, 1200), (0, 0, 1920, 1032)), Some((144, 77, 1632, 877)));
        // At 100% on the same screen it fits as it is, and on a second screen to the left too.
        assert_eq!(fitted((1280, 800), (0, 0, 1920, 1032)), None);
        assert_eq!(fitted((1600, 1000), (-2560, 0, 0, 1400)), None);
        // Only the side that's too big shrinks.
        assert_eq!(fitted((1000, 1000), (0, 0, 1600, 900)), Some((300, 67, 1000, 765)));
    }

    #[test]
    fn keeps_a_place_that_is_still_on_a_monitor() {
        let monitors = [rect(0, 0, 1920, 1032), rect(-2560, 0, 0, 1400)];
        let on_second = rect(-1900, 40, -300, 1040);
        assert_eq!(on_screen(on_second, &monitors), on_second);
        // Partly off the edge but with its title bar reachable.
        let hanging = rect(1700, 100, 2900, 900);
        assert_eq!(on_screen(hanging, &monitors), hanging);
    }

    #[test]
    fn moves_a_place_whose_monitor_is_gone_onto_the_nearest_one() {
        // The second monitor (to the left) was unplugged.
        let monitors = [rect(0, 0, 1920, 1032)];
        let moved = on_screen(rect(-1900, 40, -300, 1040), &monitors);
        assert_eq!(moved, rect(160, 16, 1760, 1016));
        // Too big for what's left: no bigger than the monitor.
        let big = on_screen(rect(-4000, -100, -400, 1900), &monitors);
        assert_eq!(big, rect(0, 0, 1920, 1032));
        // With no monitors known it is returned as it was.
        assert_eq!(on_screen(rect(5, 5, 100, 100), &[]), rect(5, 5, 100, 100));
        // Nonsense numbers don't overflow.
        let wild = rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        assert_eq!(on_screen(wild, &monitors), wild);
        let far = on_screen(rect(i32::MAX - 600, i32::MAX - 500, i32::MAX, i32::MAX), &monitors);
        assert_eq!(far, rect(660, 266, 1260, 766));
    }

    #[test]
    fn nearest_monitor_wins_when_there_are_several() {
        let monitors = [
            rect(0, 0, 1920, 1032),
            rect(1920, 0, 3840, 1032),
            rect(-1920, 0, 0, 1032),
        ];
        // Far to the right and below: nearest is the one at 1920..3840.
        let moved = on_screen(rect(6000, 3000, 6800, 3600), &monitors);
        assert_eq!(moved, rect(2480, 216, 3280, 816));
    }

    #[test]
    fn plans_the_restore() {
        let monitors = [rect(0, 0, 1920, 1032)];
        assert_eq!(plan_restore(None, &monitors), RestorePlan::Fit);
        let empty = Placement {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
            maximized: true,
        };
        assert_eq!(plan_restore(Some(empty), &monitors), RestorePlan::Fit);
        let kept = Placement {
            left: 100,
            top: 100,
            right: 1380,
            bottom: 900,
            maximized: true,
        };
        assert_eq!(
            plan_restore(Some(kept), &monitors),
            RestorePlan::Place {
                rect: rect(100, 100, 1380, 900),
                maximized: true
            }
        );
    }

    #[test]
    fn quotes_paths_for_the_command_line() {
        assert_eq!(quoted(r"C:\"), r#""C:\\""#);
        assert_eq!(quoted(r"D:\Projects\Glint"), r#""D:\Projects\Glint""#);
        assert_eq!(quoted(r"\\server\share\"), r#""\\server\share\\""#);
    }

    #[test]
    fn builds_the_three_tasks() {
        let tasks = task_links(r"C:\Glint\Glint.exe", r"C:\Windows\System32");
        let titles: Vec<&str> = tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["This PC", "Quick search", "Recycle Bin"]);
        assert_eq!(tasks[0].args, r#""this-pc:""#);
        assert_eq!(tasks[1].args, "--quick-search");
        assert_eq!(tasks[0].icon_path, r"C:\Glint\Glint.exe");
        assert_eq!(tasks[2].args, r#""shell:RecycleBinFolder""#);
        assert_eq!(tasks[2].icon_path, r"C:\Windows\System32\imageres.dll");
        assert_eq!(tasks[2].icon_index, -55);
    }

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn folders_are_pinned_first_without_repeats() {
        let pinned = list(&[r"C:\Work", r"D:\"]);
        let frequent = list(&[r"C:\Work", r"C:\Users\me\Downloads"]);
        let links = folder_links(
            &pinned,
            &frequent,
            true,
            10,
            &[],
            |_| true,
            r"C:\Windows\System32\shell32.dll",
        );
        let titles: Vec<&str> = links.iter().map(|l| l.title.as_str()).collect();
        // A drive's root has no file name: the path itself is the title.
        assert_eq!(titles, ["Work", r"D:\", "Downloads"]);
        assert_eq!(links[1].args, r#""D:\\""#);
        assert_eq!(links[0].description, r"C:\Work");
        assert_eq!(links[0].icon_index, 3);
    }

    #[test]
    fn most_used_folders_stay_out_when_windows_does_not_track_recent_items() {
        let pinned = list(&[r"C:\Work"]);
        let frequent = list(&[r"C:\Other"]);
        let links = folder_links(&pinned, &frequent, false, 10, &[], |_| true, "s.dll");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title, "Work");
    }

    #[test]
    fn folders_skip_removed_and_missing_ones_and_stop_at_the_room_there_is() {
        let pinned: Vec<String> = (0..20).map(|i| format!(r"C:\F{i}")).collect();
        // Room for 2 is raised to the least used, 4; room for 50 is cut to 10.
        assert_eq!(folder_links(&pinned, &[], true, 2, &[], |_| true, "s").len(), 4);
        assert_eq!(folder_links(&pinned, &[], true, 50, &[], |_| true, "s").len(), 10);
        let removed = vec![quoted(r"C:\F0")];
        let links = folder_links(&pinned, &[], true, 10, &removed, |f| f != r"C:\F1", "s");
        assert_eq!(links[0].title, "F2");
        assert!(links.iter().all(|l| l.title != "F0" && l.title != "F1"));
        assert_eq!(links.len(), 10);
    }

    #[test]
    fn progress_states_and_values() {
        assert_eq!(ProgressState::parse("normal"), ProgressState::Normal);
        assert_eq!(ProgressState::parse("indeterminate"), ProgressState::Indeterminate);
        assert_eq!(ProgressState::parse("paused"), ProgressState::Paused);
        assert_eq!(ProgressState::parse("error"), ProgressState::Error);
        assert_eq!(ProgressState::parse(""), ProgressState::None);
        assert_eq!(ProgressState::parse("surprise"), ProgressState::None);
        assert_eq!(progress_value(ProgressState::Normal, Some(5), Some(10)), Some((5, 10)));
        // More done than total is capped; a zero total or a missing number shows nothing.
        assert_eq!(progress_value(ProgressState::Error, Some(15), Some(10)), Some((10, 10)));
        assert_eq!(progress_value(ProgressState::Normal, Some(1), Some(0)), None);
        assert_eq!(progress_value(ProgressState::Normal, None, Some(10)), None);
        assert_eq!(progress_value(ProgressState::Indeterminate, Some(1), Some(10)), None);
        assert_eq!(progress_value(ProgressState::None, Some(1), Some(10)), None);
    }

    #[test]
    fn reads_thumbnail_button_clicks() {
        let click = |id: u32| (CLICKED << 16) | id as usize;
        assert_eq!(clicked_button(click(FIRST_ID)), Some(ThumbButton::Back));
        assert_eq!(clicked_button(click(FIRST_ID + 1)), Some(ThumbButton::Forward));
        assert_eq!(clicked_button(click(FIRST_ID + 2)), Some(ThumbButton::Up));
        assert_eq!(clicked_button(click(FIRST_ID + 3)), None);
        assert_eq!(clicked_button(click(FIRST_ID - 1)), None);
        // Another notification code, or a menu command, is not a click.
        assert_eq!(clicked_button(FIRST_ID as usize), None);
        assert_eq!(clicked_button((0x1801 << 16) | FIRST_ID as usize), None);
        assert_eq!(ThumbButton::Up.name(), "up");
        assert_eq!(ThumbButton::Back.name(), "back");
        assert_eq!(ThumbButton::Forward.name(), "forward");
    }

    #[test]
    fn button_clicks_reach_the_channel() {
        let events = thumb_button_events();
        assert!(thumb_channel().0.try_send(ThumbButton::Forward).is_ok());
        assert_eq!(events.try_recv().ok(), Some(ThumbButton::Forward));
    }

    #[test]
    #[ignore = "draws on this PC's fonts"]
    fn draws_the_preview_buttons() {
        for color in [0xFFFFFFu32, 0x1F1F1F] {
            for &(_, glyph, _) in &BUTTONS {
                let pixels = imp::glyph_pixels(glyph, 40, color);
                assert!(
                    pixels.is_some_and(|p| p.chunks(4).any(|px| px[3] > 200)),
                    "each glyph shows"
                );
            }
            assert!(imp::glyph_icon(imp::first_glyph(), 20, color).is_some());
        }
    }
}
