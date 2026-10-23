//! Windows shell integration: copy, move, delete and rename through IFileOperation (Explorer's own
//! engine, so progress, conflicts, Recycle Bin and permission prompts behave exactly like Explorer),
//! opening files as a double-click there does, the file clipboard shared with Explorer, Properties,
//! Open with and the terminal.

use crate::archive;
use crate::fsx::blocking;
use serde::{Deserialize, Serialize};
use tauri::Window;

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileOp {
    Copy { sources: Vec<String>, dest: String },
    Move { sources: Vec<String>, dest: String },
    Delete { paths: Vec<String>, permanent: bool, confirm: bool },
    Rename { path: String, name: String },
    /// Undoing a move: each item back into `folder` as `name`.
    MoveBack { items: Vec<Back> },
    /// Undoing a delete, or Restore in the Recycle Bin: its items (as [`Done::to`] names them) back
    /// where they came from, into their folder made again if it's gone.
    Restore { items: Vec<Back> },
    /// Delete for good in the Recycle Bin: its items, by their `$R` paths, with their `$I` records
    /// (the app has asked first, naming them as they were).
    Purge { paths: Vec<String> },
    /// Sync in compare panes: each item copied into `folder` as `name`, each to a folder of its own.
    CopyInto { items: Vec<Back> },
}

#[derive(Deserialize)]
pub struct Back {
    pub path: String,
    pub folder: String,
    pub name: String,
}

/// What became of one of the items asked for: where it is now (the copy, the item moved or renamed,
/// or its Recycle Bin entry); None when it's gone for good or was skipped.
#[derive(Serialize, Debug)]
pub struct Done {
    pub from: String,
    pub to: Option<String>,
}

#[derive(Serialize, Debug)]
pub struct Performed {
    /// False when the person cancelled in one of Windows' dialogs; what was done before stays done.
    pub completed: bool,
    pub items: Vec<Done>,
}

#[derive(Serialize)]
pub struct ClipboardFiles {
    pub paths: Vec<String>,
    pub cut: bool,
}

#[cfg(windows)]
pub(crate) fn owner(window: &Window) -> isize {
    window.hwnd().map_or(0, |h| h.0 as isize)
}

#[cfg(not(windows))]
pub(crate) fn owner(_: &Window) -> isize {
    0
}

#[cfg(windows)]
pub(crate) use imp::in_sta;

/// Runs `f` on the UI thread, which owns the window and has COM set up.
async fn on_main<T, F>(window: &Window, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    window.run_on_main_thread(move || {
        let _ = tx.send(f());
    })
    .map_err(|e| e.to_string())?;
    blocking(move || rx.recv().map_err(|e| e.to_string())?).await
}

/// Says what became of each item, for undoing it. Nothing is written into a zip or 7z archive, or
/// taken out of one this way (Windows' own zip folders would): archives open read-only, and what's
/// copied out of them is extracted first (`archive::archive_stage`).
#[tauri::command]
pub async fn file_op(window: Window, op: FileOp) -> Result<Performed, String> {
    let owner = owner(&window);
    blocking(move || {
        let touches_archive = match &op {
            FileOp::Copy { sources, dest } | FileOp::Move { sources, dest } => archive::is_read_only(dest) || sources.iter().any(|s| archive::is_inside(s)),
            FileOp::Delete { paths, .. } | FileOp::Purge { paths } => paths.iter().any(|p| archive::is_inside(p)),
            FileOp::Rename { path, .. } => archive::is_inside(path),
            FileOp::MoveBack { items } | FileOp::Restore { items } | FileOp::CopyInto { items } => items.iter().any(|b| archive::is_inside(&b.path) || archive::is_read_only(&b.folder)),
        };
        if touches_archive {
            return Err(crate::fsx::READ_ONLY.into());
        }
        // A preview handler may hold the file open: what's moved, renamed or deleted goes from the preview first.
        let leaving: Vec<String> = match &op {
            FileOp::Move { sources, .. } => sources.clone(),
            FileOp::Delete { paths, .. } | FileOp::Purge { paths } => paths.clone(),
            FileOp::Rename { path, .. } => vec![path.clone()],
            FileOp::MoveBack { items } => items.iter().map(|b| b.path.clone()).collect(),
            FileOp::Copy { .. } | FileOp::Restore { .. } | FileOp::CopyInto { .. } => Vec::new(),
        };
        if !leaving.is_empty() {
            crate::native_preview::release(&leaving);
        }
        imp::file_op(owner, op)
    })
    .await
}

/// Moves `paths` to the Recycle Bin without asking first (pictures Convert replaced): where each went.
pub(crate) fn recycle(owner: isize, paths: Vec<String>) -> Result<Performed, String> {
    imp::file_op(owner, FileOp::Delete { paths, permanent: false, confirm: false })
}

/// The files on the clipboard (copied or cut), if any; what extracted copies may not be deleted for.
pub fn clipboard_paths() -> Vec<String> {
    imp::clipboard_files(0).ok().flatten().map(|files| files.paths).unwrap_or_default()
}

/// Puts files on the clipboard the way Explorer does, so they can be pasted there too.
#[tauri::command]
pub async fn clipboard_set_files(window: Window, paths: Vec<String>, cut: bool) -> Result<(), String> {
    let owner = owner(&window);
    on_main(&window, move || imp::set_clipboard_files(owner, &paths, cut)).await
}

/// Files copied or cut here or in Explorer, if the clipboard holds any.
#[tauri::command]
pub async fn clipboard_get_files(window: Window) -> Result<Option<ClipboardFiles>, String> {
    let owner = owner(&window);
    on_main(&window, move || imp::clipboard_files(owner)).await
}

/// The text on the clipboard, if it holds any (the terminal pastes it).
#[tauri::command]
pub async fn clipboard_text(window: Window) -> Result<Option<String>, String> {
    let owner = owner(&window);
    on_main(&window, move || imp::clipboard_text(owner)).await
}

/// What can be pasted as a file when the clipboard holds no files.
pub enum Clip {
    /// A PNG as browsers and the Snipping Tool put one there.
    Png(Vec<u8>),
    /// A device-independent bitmap (CF_DIBV5 or CF_DIB): a screenshot, Paint's.
    Bitmap(Vec<u8>),
    Text(String),
}

/// Paste when the clipboard holds no files: its picture or its text as a new file in `folder`,
/// "Pasted image <date and time>.png" (a bitmap Glint can't make a PNG of, .bmp) or "Pasted text
/// <date and time>.txt" (UTF-8), with " (2)" and so on when that's taken. The file made, or None when
/// there's nothing to paste.
#[tauri::command]
pub async fn clipboard_paste_file(window: Window, folder: String) -> Result<Option<String>, String> {
    let owner = owner(&window);
    let Some(clip) = on_main(&window, move || imp::clipboard_content(owner)).await? else {
        return Ok(None);
    };
    blocking(move || {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S");
        let (name, bytes) = match clip {
            Clip::Png(png) => (format!("Pasted image {stamp}.png"), png),
            Clip::Bitmap(dib) => match dib_to_png(&dib) {
                Some(png) => (format!("Pasted image {stamp}.png"), png),
                None => (format!("Pasted image {stamp}.bmp"), dib_to_bmp(&dib)),
            },
            Clip::Text(text) => (format!("Pasted text {stamp}.txt"), text.into_bytes()),
        };
        let path = crate::fsx::create_numbered(&folder, &name, false)?;
        if let Err(e) = std::fs::write(&path, &bytes) {
            let _ = std::fs::remove_file(&path);
            return Err(crate::fsx::file_error(e));
        }
        Ok(Some(path))
    })
    .await
}

/// A DIB (a BITMAPINFOHEADER, or its V4 or V5 form, then the pixels) as a .bmp file: the file header
/// before it, which says where the pixels start.
fn dib_to_bmp(dib: &[u8]) -> Vec<u8> {
    let pixels = 14 + dib_pixels_at(dib).unwrap_or(dib.len().min(40));
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend(b"BM");
    bmp.extend(((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend([0; 4]);
    bmp.extend((pixels as u32).to_le_bytes());
    bmp.extend(dib);
    bmp
}

/// Where a DIB's pixels start: after its header, the three colour masks a 40-byte header with
/// BI_BITFIELDS has after it, and its palette. A V4 or V5 header has the masks in it, but the CF_DIBV5
/// Windows makes of a bitmap repeats them after it too, when there's room for them.
fn dib_pixels_at(dib: &[u8]) -> Option<usize> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(dib.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?));
    let header = u32_at(0)? as usize;
    let (width, height) = (u32_at(4)? as i32, u32_at(8)? as i32);
    let (bits, compression, used) = (u16_at(14)?, u32_at(16)?, u32_at(32)? as usize);
    let image = (width.unsigned_abs() as usize * bits as usize).div_ceil(32) * 4 * height.unsigned_abs() as usize;
    let repeated = header >= 108 && dib.get(header..header + 12) == dib.get(40..52) && dib.len() >= header + 12 + image;
    let masks = if compression == 3 && (header == 40 || repeated) { 12 } else { 0 };
    let palette = if used > 0 { used } else if bits <= 8 { 1 << bits } else { 0 };
    Some(header + masks + 4 * palette)
}

/// A 24- or 32-bit DIB as a PNG; None for any other kind (paletted, 16-bit, compressed) or one that
/// doesn't hold what its header says. 32-bit pixels keep their alpha when a V4 or V5 header says
/// they have one and some pixel isn't fully transparent (plain 32-bit DIBs leave it 0).
fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    let u16_at = |at: usize| Some(u16::from_le_bytes(dib.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(dib.get(at..at + 4)?.try_into().ok()?));
    let header = u32_at(0)? as usize;
    let (width, height) = (u32_at(4)? as i32, u32_at(8)? as i32);
    let (bits, compression) = (u16_at(14)?, u32_at(16)?);
    let bitfields = compression == 3;
    if header < 40 || !(compression == 0 || (bitfields && bits == 32)) || !(bits == 24 || bits == 32) {
        return None;
    }
    // A standard 32-bit layout only: blue, green, red (and alpha) bytes.
    if bitfields && (u32_at(40)?, u32_at(44)?, u32_at(48)?) != (0x00ff_0000, 0x0000_ff00, 0x0000_00ff) {
        return None;
    }
    let (w, h) = (width.unsigned_abs() as usize, height.unsigned_abs() as usize);
    if w == 0 || h == 0 || w > 32_768 || h > 32_768 {
        return None;
    }
    let step = bits as usize / 8;
    let stride = (w * bits as usize).div_ceil(32) * 4;
    let start = dib_pixels_at(dib)?;
    let pixels = dib.get(start..start.checked_add(stride.checked_mul(h)?)?)?;
    let alpha_mask = if header >= 108 { u32_at(52)? } else { 0 };
    let mut rgba = Vec::with_capacity(w * h * 4);
    // Bottom-up unless the height is negative.
    for row in 0..h {
        let line = if height > 0 { h - 1 - row } else { row };
        for px in pixels[line * stride..][..w * step].chunks_exact(step) {
            rgba.extend([px[2], px[1], px[0], if step == 4 { px[3] } else { 255 }]);
        }
    }
    if step == 4 && (alpha_mask != 0xff00_0000 || rgba.iter().skip(3).step_by(4).all(|&a| a == 0)) {
        rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    }
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().ok()?.write_image_data(&rgba).ok()?;
    Some(png)
}

#[tauri::command]
pub async fn clipboard_clear(window: Window) -> Result<(), String> {
    let owner = owner(&window);
    on_main(&window, move || imp::clear_clipboard(owner)).await
}

/// Explorer's Properties dialog.
#[tauri::command]
pub async fn show_properties(window: Window, path: String) -> Result<(), String> {
    let owner = owner(&window);
    on_main(&window, move || imp::show_properties(owner, &path)).await
}

/// Opens a file as a double-click in File Explorer does: the shell's own default action, from a
/// thread with COM set up and the window as owner, so app-store apps, programs that ask for
/// administrator approval and shortcuts all open as they do there, and a type no app opens yet gets
/// Windows' app picker. A shortcut to a folder comes back as that folder, for the tab to open.
#[tauri::command]
pub async fn open_item(window: Window, path: String) -> Result<Option<String>, String> {
    let owner = owner(&window);
    blocking(move || imp::open_item(owner, path)).await
}

/// Windows' "Open with" app picker.
#[tauri::command]
pub async fn open_with(window: Window, path: String) -> Result<(), String> {
    let owner = owner(&window);
    blocking(move || imp::open_with(owner, path)).await
}

/// Drags files out of the window (into Explorer, a mail, a chat…) with the shell's own drag data,
/// as Explorer does. Must start while the mouse button is down; resolves once the drop or cancel
/// happens with "copy", "move", "link" or "none", or with "released" when the button was already up.
/// `copy_only` offers only a copy (files extracted from an archive, which are copies anyway).
#[tauri::command]
pub async fn start_drag(window: Window, paths: Vec<String>, copy_only: Option<bool>) -> Result<String, String> {
    let owner = owner(&window);
    let copy_only = copy_only.unwrap_or(false);
    on_main(&window, move || imp::start_drag(owner, &paths, copy_only)).await
}

/// Opens Windows Terminal in `path`, or a Command Prompt where Terminal isn't installed.
#[tauri::command]
pub async fn open_terminal(path: String) -> Result<(), String> {
    blocking(move || imp::open_terminal(&path)).await
}

#[cfg(windows)]
mod imp {
    use super::{Clip, ClipboardFiles, Done, FileOp, Performed};
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use windows::core::{implement, w, ComObject, Interface, Ref, BOOL, HRESULT, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{
        GlobalFree, ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_NO_ASSOCIATION, ERROR_PATH_NOT_FOUND, HANDLE, HGLOBAL,
        HWND, LPARAM, POINT,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IBindCtx, IDataObject, IPersistFile, CLSCTX_ALL, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, STGM_READ,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{IDropSource, CF_HDROP, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        DragQueryFileW, FileOperation, IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl, ILCreateFromPathW, ILFree, IShellItem,
        IShellLinkW, SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH,
        SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHObjectProperties,
        SHOpenWithDialog, ShellExecuteExW, ShellLink, BHID_DataObject, DROPFILES, FILEOPERATION_FLAGS,
        FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR, FOF_RENAMEONCOLLISION,
        FOF_WANTNUKEWARNING, HDROP, OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OAIF_REGISTER_EXT, OPENASINFO, SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST,
        SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, SHOP_FILEPATH,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetSystemMetrics, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, SM_SWAPBUTTON, SW_SHOWNORMAL,
    };

    /// HRESULT for "cancelled" from the copy engine's progress dialog.
    const COPYENGINE_E_USER_CANCELLED: i32 = 0x8027_0000_u32 as i32;

    /// Visible top-level windows of this process.
    fn visible_windows() -> HashSet<isize> {
        unsafe extern "system" fn collect(window: HWND, state: LPARAM) -> BOOL {
            let (pid, found) = unsafe { &mut *(state.0 as *mut (u32, HashSet<isize>)) };
            let mut owner = 0;
            unsafe { GetWindowThreadProcessId(window, Some(&mut owner)) };
            if owner == *pid && unsafe { IsWindowVisible(window) }.as_bool() {
                found.insert(window.0 as isize);
            }
            true.into()
        }
        let mut state = (std::process::id(), HashSet::new());
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut state as *mut _ as isize));
        }
        state.1
    }

    /// The copy engine shows its progress, "Replace or Skip Files" and confirmation dialogs from a
    /// thread of its own, unowned, so they can open behind our window. While `busy`, bring each new
    /// one to the front like Explorer's (if we're not the foreground app, Windows flashes it instead).
    fn raise_new_dialogs(busy: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
        let ours = visible_windows();
        std::thread::spawn(move || {
            let mut shown = ours.clone();
            while busy.load(Ordering::Relaxed) {
                let now = visible_windows();
                for &window in now.difference(&shown) {
                    unsafe {
                        let _ = SetForegroundWindow(HWND(window as *mut _));
                    }
                }
                // A dialog that closes and opens again (the next conflict) is raised again.
                shown = now.union(&ours).copied().collect();
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    }

    fn message(e: windows::core::Error) -> String {
        e.message()
    }

    fn hwnd(owner: isize) -> HWND {
        HWND(owner as *mut _)
    }

    /// Runs `f` on a fresh single-threaded COM apartment, which the shell's dialogs need.
    pub(crate) fn in_sta<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
        std::thread::spawn(move || unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok().map_err(message)?;
            let result = f();
            CoUninitialize();
            result
        })
        .join()
        .map_err(|_| "The operation failed unexpectedly".to_string())?
    }

    unsafe fn item(path: &str) -> Result<IShellItem, String> {
        SHCreateItemFromParsingName(&HSTRING::from(path), None::<&IBindCtx>).map_err(|e| format!("{path}: {}", e.message()))
    }

    fn parent_is(path: &str, dir: &str) -> bool {
        Path::new(path)
            .parent()
            .is_some_and(|p| p.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(dir.trim_end_matches('\\')))
    }

    pub fn file_op(owner: isize, op: FileOp) -> Result<Performed, String> {
        in_sta(move || unsafe { perform(owner, op) })
    }

    /// Hears from the copy engine what became of each item: where the copy, the moved or renamed
    /// item or the Recycle Bin entry ended up (after "Keep both" or a "- Copy" name, too).
    #[implement(IFileOperationProgressSink)]
    #[derive(Default)]
    struct Track {
        done: RefCell<Vec<Done>>,
    }

    impl Track {
        fn note(&self, item: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>, name: SIGDN) {
            if hr.is_err() {
                return;
            }
            if let Some(from) = item.as_ref().and_then(|i| display(i, SIGDN_FILESYSPATH)) {
                self.done.borrow_mut().push(Done { from, to: created.as_ref().and_then(|i| display(i, name)) });
            }
        }
    }

    #[allow(non_snake_case)]
    impl IFileOperationProgressSink_Impl for Track_Impl {
        fn StartOperations(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn FinishOperations(&self, _: HRESULT) -> windows::core::Result<()> {
            Ok(())
        }
        fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostRenameItem(&self, _: u32, item: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows::core::Result<()> {
            self.note(item, hr, created, SIGDN_FILESYSPATH);
            Ok(())
        }
        fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostMoveItem(&self, _: u32, item: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows::core::Result<()> {
            self.note(item, hr, created, SIGDN_FILESYSPATH);
            Ok(())
        }
        fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostCopyItem(&self, _: u32, item: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR, hr: HRESULT, created: Ref<IShellItem>) -> windows::core::Result<()> {
            self.note(item, hr, created, SIGDN_FILESYSPATH);
            Ok(())
        }
        fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostDeleteItem(&self, _: u32, item: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>) -> windows::core::Result<()> {
            // A recycled item lives on in the Recycle Bin, which only its shell name reaches.
            self.note(item, hr, created, SIGDN_DESKTOPABSOLUTEPARSING);
            Ok(())
        }
        fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn PostNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR, _: &PCWSTR, _: u32, _: HRESULT, _: Ref<IShellItem>) -> windows::core::Result<()> {
            Ok(())
        }
        fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
            Ok(())
        }
        fn ResetTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn PauseTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
        fn ResumeTimer(&self) -> windows::core::Result<()> {
            Ok(())
        }
    }

    fn display(item: &IShellItem, name: SIGDN) -> Option<String> {
        unsafe {
            let text = item.GetDisplayName(name).ok()?;
            let owned = text.to_string().ok();
            CoTaskMemFree(Some(text.0 as *const _));
            owned
        }
    }

    /// A recycled item is `$R…` in the Recycle Bin's folder, with its original path and deletion time
    /// in `$I…` next to it: once the `$R` file is back, that record goes too, as Restore does.
    fn forget_recycled(recycled: &str) {
        let path = Path::new(recycled);
        let in_bin = recycled.to_ascii_lowercase().contains("\\$recycle.bin\\");
        if let (true, Some(name)) = (in_bin, path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("$R"))) {
            let _ = std::fs::remove_file(path.with_file_name(format!("$I{name}")));
        }
    }

    unsafe fn perform(owner: isize, op: FileOp) -> Result<Performed, String> {
        // Either way, what's left of an item in the Recycle Bin is its `$I` record, which goes too.
        let restoring = matches!(op, FileOp::Restore { .. });
        let purging = matches!(op, FileOp::Purge { .. });
        let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(message)?;
        if owner != 0 {
            fo.SetOwnerWindow(hwnd(owner)).map_err(message)?;
        }
        let track = ComObject::new(Track::default());
        let cookie = fo.Advise(&track.to_interface::<IFileOperationProgressSink>()).map_err(message)?;
        let sink = None::<&IFileOperationProgressSink>;
        let none = FILEOPERATION_FLAGS(0);
        // The items asked for: the engine reports what's inside folders too.
        let asked: Vec<String> = match &op {
            FileOp::Copy { sources, .. } | FileOp::Move { sources, .. } => sources.clone(),
            FileOp::Delete { paths, .. } | FileOp::Purge { paths } => paths.clone(),
            FileOp::Rename { path, .. } => vec![path.clone()],
            FileOp::MoveBack { items } | FileOp::Restore { items } | FileOp::CopyInto { items } => items.iter().map(|b| b.path.clone()).collect(),
        };
        match op {
            FileOp::MoveBack { items } | FileOp::Restore { items } => {
                fo.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR).map_err(message)?;
                for back in &items {
                    // As File Explorer restores: into the folder it was deleted from, made again if it's gone.
                    if restoring && !Path::new(&back.folder).exists() {
                        std::fs::create_dir_all(&back.folder).map_err(|e| format!("{}: {e}", back.folder))?;
                    }
                    fo.MoveItem(&item(&back.path)?, &item(&back.folder)?, &HSTRING::from(back.name.as_str()), sink).map_err(message)?;
                }
            }
            FileOp::Purge { paths } => {
                fo.SetOperationFlags(FOF_NOCONFIRMATION).map_err(message)?;
                for path in &paths {
                    fo.DeleteItem(&item(path)?, sink).map_err(message)?;
                }
            }
            FileOp::CopyInto { items } => {
                fo.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR).map_err(message)?;
                for each in &items {
                    fo.CopyItem(&item(&each.path)?, &item(&each.folder)?, &HSTRING::from(each.name.as_str()), sink).map_err(message)?;
                }
            }
            FileOp::Copy { sources, dest } => {
                // Pasting into the folder the files came from makes "name - Copy" duplicates, like Explorer.
                let duplicate = sources.iter().all(|s| parent_is(s, &dest));
                let rename = if duplicate { FOF_RENAMEONCOLLISION } else { none };
                fo.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR | rename).map_err(message)?;
                let folder = item(&dest)?;
                for source in &sources {
                    fo.CopyItem(&item(source)?, &folder, PCWSTR::null(), sink).map_err(message)?;
                }
            }
            FileOp::Move { sources, dest } => {
                let sources: Vec<String> = sources.into_iter().filter(|s| !parent_is(s, &dest)).collect();
                if sources.is_empty() {
                    let _ = fo.Unadvise(cookie);
                    return Ok(Performed { completed: true, items: Vec::new() });
                }
                fo.SetOperationFlags(FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR).map_err(message)?;
                let folder = item(&dest)?;
                for source in &sources {
                    fo.MoveItem(&item(source)?, &folder, PCWSTR::null(), sink).map_err(message)?;
                }
            }
            FileOp::Delete { paths, permanent, confirm } => {
                // Like Explorer: recycle without asking, but warn if something can't be recycled.
                let flags = match (permanent, confirm) {
                    (true, _) => none,
                    (false, true) => FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE,
                    (false, false) => FOF_ALLOWUNDO | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING,
                };
                fo.SetOperationFlags(flags).map_err(message)?;
                for path in &paths {
                    fo.DeleteItem(&item(path)?, sink).map_err(message)?;
                }
            }
            FileOp::Rename { path, name } => {
                fo.SetOperationFlags(FOF_ALLOWUNDO).map_err(message)?;
                fo.RenameItem(&item(&path)?, &HSTRING::from(name), sink).map_err(message)?;
            }
        }
        let busy = Arc::new(AtomicBool::new(true));
        let raiser = raise_new_dialogs(busy.clone());
        let performed = fo.PerformOperations();
        busy.store(false, Ordering::Relaxed);
        let _ = raiser.join();
        let _ = fo.Unadvise(cookie);
        let completed = match performed {
            Ok(()) => !fo.GetAnyOperationsAborted().is_ok_and(|aborted| aborted.as_bool()),
            Err(e) if e.code() == ERROR_CANCELLED.to_hresult() || e.code().0 == COPYENGINE_E_USER_CANCELLED => false,
            Err(e) => return Err(message(e)),
        };
        let same = |a: &str, b: &str| a.trim_end_matches('\\').eq_ignore_ascii_case(b.trim_end_matches('\\'));
        let items: Vec<Done> = track.done.take().into_iter().filter(|d| asked.iter().any(|a| same(a, &d.from))).collect();
        if restoring {
            items.iter().filter(|d| d.to.is_some()).for_each(|d| forget_recycled(&d.from));
        } else if purging {
            items.iter().for_each(|d| forget_recycled(&d.from));
        }
        Ok(Performed { completed, items })
    }

    unsafe fn open_clipboard(owner: isize) -> Result<(), String> {
        for _ in 0..10 {
            if OpenClipboard(Some(hwnd(owner))).is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Err("Another app is using the clipboard. Try again.".into())
    }

    /// Hands a copy of `bytes` to the clipboard, which owns the memory once SetClipboardData succeeds.
    unsafe fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(message)?;
        let ptr = GlobalLock(handle) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(handle));
            return Err("Out of memory".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(handle);
        if let Err(e) = SetClipboardData(format, Some(HANDLE(handle.0))) {
            let _ = GlobalFree(Some(handle));
            return Err(message(e));
        }
        Ok(())
    }

    pub fn set_clipboard_files(owner: isize, paths: &[String], cut: bool) -> Result<(), String> {
        // CF_HDROP is a DROPFILES header followed by double-NUL-terminated UTF-16 paths.
        let header = DROPFILES { pFiles: size_of::<DROPFILES>() as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
        let mut data = unsafe { std::slice::from_raw_parts(&header as *const DROPFILES as *const u8, size_of::<DROPFILES>()) }.to_vec();
        for path in paths {
            for unit in OsStr::new(path).encode_wide().chain(Some(0)) {
                data.extend(unit.to_le_bytes());
            }
        }
        data.extend([0, 0]);
        // Explorer reads "Preferred DropEffect" to tell cut (move) from copy.
        let effect = if cut { DROPEFFECT_MOVE.0 } else { DROPEFFECT_COPY.0 | DROPEFFECT_LINK.0 };
        unsafe {
            let drop_effect = RegisterClipboardFormatW(w!("Preferred DropEffect"));
            open_clipboard(owner)?;
            let result = EmptyClipboard()
                .map_err(message)
                .and_then(|_| put(CF_HDROP.0 as u32, &data))
                .and_then(|_| put(drop_effect, &effect.to_le_bytes()));
            let _ = CloseClipboard();
            result
        }
    }

    pub fn clipboard_files(owner: isize) -> Result<Option<ClipboardFiles>, String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_err() {
                return Ok(None);
            }
            open_clipboard(owner)?;
            let result = read_clipboard_files();
            let _ = CloseClipboard();
            result.map(Some)
        }
    }

    const CF_UNICODETEXT: u32 = 13;
    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;

    pub fn clipboard_text(owner: isize) -> Result<Option<String>, String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
                return Ok(None);
            }
            open_clipboard(owner)?;
            let text = unicode_text();
            let _ = CloseClipboard();
            Ok(text)
        }
    }

    /// The clipboard's text (it's open), up to its NUL and never past its memory block, whatever it holds.
    unsafe fn unicode_text() -> Option<String> {
        let handle = GetClipboardData(CF_UNICODETEXT).ok().filter(|h| !h.is_invalid())?;
        let global = HGLOBAL(handle.0);
        let ptr = GlobalLock(global) as *const u16;
        let units = if ptr.is_null() { &[][..] } else { std::slice::from_raw_parts(ptr, GlobalSize(global) / 2) };
        let text = String::from_utf16_lossy(&units[..units.iter().position(|&u| u == 0).unwrap_or(units.len())]);
        let _ = GlobalUnlock(global);
        Some(text)
    }

    /// All of a clipboard format's memory block (the clipboard is open).
    unsafe fn global_bytes(format: u32) -> Option<Vec<u8>> {
        let handle = GetClipboardData(format).ok().filter(|h| !h.is_invalid())?;
        let global = HGLOBAL(handle.0);
        let ptr = GlobalLock(global) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, GlobalSize(global)).to_vec();
        let _ = GlobalUnlock(global);
        Some(bytes)
    }

    /// What's on the clipboard to paste as a file when it holds no files: a "PNG" first (what browsers
    /// and the Snipping Tool add to a picture), then text (Excel's cells come with a picture of
    /// themselves), then a bitmap (a screenshot).
    pub fn clipboard_content(owner: isize) -> Result<Option<Clip>, String> {
        unsafe {
            let png = RegisterClipboardFormatW(w!("PNG"));
            let has = |format: u32| IsClipboardFormatAvailable(format).is_ok();
            if has(CF_HDROP.0 as u32) || ![png, CF_UNICODETEXT, CF_DIBV5, CF_DIB].into_iter().any(has) {
                return Ok(None);
            }
            open_clipboard(owner)?;
            let clip = read_clip(png);
            let _ = CloseClipboard();
            Ok(clip)
        }
    }

    unsafe fn read_clip(png: u32) -> Option<Clip> {
        let has = |format: u32| IsClipboardFormatAvailable(format).is_ok();
        if has(png) {
            // Its memory block can be bigger than the PNG: up to the end of its last chunk.
            if let Some(mut bytes) = global_bytes(png).filter(|b| b.starts_with(b"\x89PNG\r\n\x1a\n")) {
                if let Some(end) = bytes.windows(4).rposition(|w| w == b"IEND") {
                    bytes.truncate((end + 8).min(bytes.len()));
                }
                return Some(Clip::Png(bytes));
            }
        }
        if has(CF_UNICODETEXT) {
            if let Some(text) = unicode_text().filter(|t| !t.trim().is_empty()) {
                return Some(Clip::Text(text));
            }
        }
        [CF_DIBV5, CF_DIB].into_iter().filter(|&f| has(f)).find_map(|f| global_bytes(f)).map(Clip::Bitmap)
    }

    unsafe fn read_clipboard_files() -> Result<ClipboardFiles, String> {
        let drop = HDROP(GetClipboardData(CF_HDROP.0 as u32).map_err(message)?.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let mut buf = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
            let len = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
            paths.push(String::from_utf16_lossy(&buf[..len]));
        }
        let effect = match GetClipboardData(RegisterClipboardFormatW(w!("Preferred DropEffect"))) {
            Ok(handle) if !handle.is_invalid() => {
                let global = HGLOBAL(handle.0);
                let ptr = GlobalLock(global) as *const u32;
                let effect = if ptr.is_null() { 0 } else { ptr.read_unaligned() };
                let _ = GlobalUnlock(global);
                effect
            }
            _ => DROPEFFECT_COPY.0,
        };
        Ok(ClipboardFiles { paths, cut: effect & DROPEFFECT_MOVE.0 != 0 && effect & DROPEFFECT_COPY.0 == 0 })
    }

    pub fn clear_clipboard(owner: isize) -> Result<(), String> {
        unsafe {
            open_clipboard(owner)?;
            let result = EmptyClipboard().map_err(message);
            let _ = CloseClipboard();
            result
        }
    }

    pub fn show_properties(owner: isize, path: &str) -> Result<(), String> {
        let shown = unsafe { SHObjectProperties(Some(hwnd(owner)), SHOP_FILEPATH, &HSTRING::from(path), PCWSTR::null()) };
        if shown.as_bool() {
            Ok(())
        } else {
            Err("Windows couldn't show the properties".into())
        }
    }

    pub fn open_with(owner: isize, path: String) -> Result<(), String> {
        in_sta(move || unsafe { pick_app(owner, &path) })
    }

    /// The app picker, which opens the file with what's picked (and can make it the default).
    unsafe fn pick_app(owner: isize, path: &str) -> Result<(), String> {
        let file = HSTRING::from(path);
        let info = OPENASINFO {
            pcszFile: PCWSTR(file.as_ptr()),
            pcszClass: PCWSTR::null(),
            oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_EXEC,
        };
        match SHOpenWithDialog(Some(hwnd(owner)), &info) {
            Err(e) if e.code() != ERROR_CANCELLED.to_hresult() => Err(message(e)),
            _ => Ok(()),
        }
    }

    pub fn open_item(owner: isize, path: String) -> Result<Option<String>, String> {
        in_sta(move || unsafe {
            if path.to_ascii_lowercase().ends_with(".lnk") {
                if let Some(folder) = shortcut_folder(&path) {
                    return Ok(Some(folder));
                }
            }
            let file = HSTRING::from(path.as_str());
            let mut info = SHELLEXECUTEINFOW {
                cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
                // The item's default action, as File Explorer runs it; what goes wrong comes back here.
                fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
                hwnd: hwnd(owner),
                lpFile: PCWSTR(file.as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            match ShellExecuteExW(&mut info) {
                Ok(()) => Ok(None),
                // Declined, such as an administrator approval.
                Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(None),
                Err(e) if e.code() == ERROR_NO_ASSOCIATION.to_hresult() => pick_app(owner, &path).map(|()| None),
                Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => Err("Windows doesn't let your account open it.".into()),
                Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() || e.code() == ERROR_PATH_NOT_FOUND.to_hresult() => Err("It isn't there anymore.".into()),
                Err(e) => Err(message(e)),
            }
        })
    }

    /// Where a shortcut points, when that's a folder (not a file, nor a place such as This PC).
    unsafe fn shortcut_folder(path: &str) -> Option<String> {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>().ok()?.Load(&HSTRING::from(path), STGM_READ).ok()?;
        let mut target = vec![0u16; 32_768];
        link.GetPath(&mut target, std::ptr::null_mut(), 0).ok()?;
        let target = String::from_utf16_lossy(&target[..target.iter().position(|&c| c == 0)?]);
        (!target.is_empty() && crate::fsx::stat(&target).is_some_and(|e| e.dir)).then_some(target)
    }

    /// Runs on the UI thread (OLE is initialized there and the drag loop needs it).
    pub fn start_drag(owner: isize, paths: &[String], copy_only: bool) -> Result<String, String> {
        unsafe {
            // The drag drops wherever the pointer is once the button is up, so if it was released while
            // this request was on its way, don't start: that would drop at once, under the pointer.
            let primary = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
            if GetAsyncKeyState(primary.0 as i32) >= 0 {
                return Ok("released".into());
            }
            let pidls: Vec<*mut ITEMIDLIST> = paths.iter().map(|p| ILCreateFromPathW(&HSTRING::from(p.as_str()))).collect();
            let dragged = (|| {
                if pidls.iter().any(|p| p.is_null()) {
                    return Err("Some of these items no longer exist".to_string());
                }
                let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p as *const _).collect();
                // The shell's data object carries every format Explorer offers (file list, shell IDs…).
                let data: IDataObject = SHCreateShellItemArrayFromIDLists(&list)
                    .and_then(|items| items.BindToHandler(None::<&IBindCtx>, &BHID_DataObject))
                    .map_err(message)?;
                let allowed = if copy_only { DROPEFFECT_COPY } else { DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK };
                let effect = SHDoDragDrop(Some(hwnd(owner)), &data, None::<&IDropSource>, allowed).map_err(message)?;
                Ok(if effect.0 & DROPEFFECT_MOVE.0 != 0 {
                    "move"
                } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
                    "copy"
                } else if effect.0 & DROPEFFECT_LINK.0 != 0 {
                    "link"
                } else {
                    "none"
                }
                .to_string())
            })();
            for pidl in pidls.into_iter().filter(|p| !p.is_null()) {
                ILFree(Some(pidl as *const _));
            }
            dragged
        }
    }

    pub fn open_terminal(path: &str) -> Result<(), String> {
        const CREATE_NEW_CONSOLE: u32 = 0x10;
        if Command::new("wt.exe").arg("-d").arg(path).spawn().is_ok() {
            return Ok(());
        }
        Command::new("cmd.exe")
            .current_dir(path)
            .creation_flags(CREATE_NEW_CONSOLE)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Couldn't open a terminal: {e}"))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Clip, ClipboardFiles, FileOp, Performed};

    const WINDOWS_ONLY: &str = "This is only available on Windows";

    pub fn file_op(_: isize, op: FileOp) -> Result<Performed, String> {
        let result = match op {
            FileOp::Rename { path, name } => {
                let to = std::path::Path::new(&path).with_file_name(name);
                if to.exists() {
                    return Err("Something with that name already exists".into());
                }
                std::fs::rename(&path, to)
            }
            FileOp::Delete { paths, permanent: true, .. } => paths.iter().try_for_each(|p| {
                let path = std::path::Path::new(p);
                if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
            }),
            _ => return Err(WINDOWS_ONLY.into()),
        };
        result.map(|_| Performed { completed: true, items: Vec::new() }).map_err(|e| e.to_string())
    }

    pub fn set_clipboard_files(_: isize, _: &[String], _: bool) -> Result<(), String> {
        Err(WINDOWS_ONLY.into())
    }

    pub fn clipboard_files(_: isize) -> Result<Option<ClipboardFiles>, String> {
        Ok(None)
    }

    pub fn clipboard_text(_: isize) -> Result<Option<String>, String> {
        Ok(None)
    }

    pub fn clipboard_content(_: isize) -> Result<Option<Clip>, String> {
        Ok(None)
    }

    pub fn clear_clipboard(_: isize) -> Result<(), String> {
        Ok(())
    }

    pub fn show_properties(_: isize, _: &str) -> Result<(), String> {
        Err(WINDOWS_ONLY.into())
    }

    pub fn open_with(_: isize, _: String) -> Result<(), String> {
        Err(WINDOWS_ONLY.into())
    }

    pub fn open_item(_: isize, _: String) -> Result<Option<String>, String> {
        Err(WINDOWS_ONLY.into())
    }

    pub fn start_drag(_: isize, _: &[String], _: bool) -> Result<String, String> {
        Err(WINDOWS_ONLY.into())
    }

    pub fn open_terminal(_: &str) -> Result<(), String> {
        Err(WINDOWS_ONLY.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 by 2 DIB: red and green on top, blue and white below, as a 40-byte header (or a V5 one with
    /// colour masks, repeated after it with `repeat` as Windows does) and its rows, bottom-up unless
    /// `top_down`; 32-bit pixels carry `alpha`.
    fn dib_with(bits: u16, v5: bool, top_down: bool, alpha: u8, repeat: bool) -> Vec<u8> {
        let masks = [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff];
        let mut d = Vec::new();
        d.extend((if v5 { 124u32 } else { 40 }).to_le_bytes());
        d.extend(2i32.to_le_bytes());
        d.extend((if top_down { -2i32 } else { 2 }).to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(bits.to_le_bytes());
        d.extend((if v5 { 3u32 } else { 0 }).to_le_bytes());
        d.extend([0u8; 20]);
        if v5 {
            for mask in masks.into_iter().chain([0xff00_0000]) {
                d.extend(mask.to_le_bytes());
            }
            d.resize(124, 0);
            if repeat {
                d.extend(masks.iter().flat_map(|m| m.to_le_bytes()));
            }
        }
        // Blue, green, red bytes.
        let (top, bottom) = ([[0, 0, 255], [0, 255, 0]], [[255, 0, 0], [255, 255, 255]]);
        for row in if top_down { [top, bottom] } else { [bottom, top] } {
            let mut line: Vec<u8> = Vec::new();
            for px in row {
                line.extend(px);
                if bits == 32 {
                    line.push(alpha);
                }
            }
            line.resize(line.len().div_ceil(4) * 4, 0);
            d.extend(line);
        }
        d
    }

    fn dib(bits: u16, v5: bool, top_down: bool, alpha: u8) -> Vec<u8> {
        dib_with(bits, v5, top_down, alpha, false)
    }

    fn pixels(png: &[u8]) -> Vec<u8> {
        let mut reader = png::Decoder::new(std::io::Cursor::new(png)).read_info().unwrap();
        let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut rgba).unwrap();
        assert_eq!((info.width, info.height, info.color_type), (2, 2, png::ColorType::Rgba));
        rgba
    }

    #[test]
    fn makes_pngs_of_bitmaps() {
        let opaque = |a: u8| vec![255, 0, 0, a, 0, 255, 0, a, 0, 0, 255, a, 255, 255, 255, a];
        // 24-bit, bottom-up, as a screenshot is.
        assert_eq!(pixels(&dib_to_png(&dib(24, false, false, 0)).unwrap()), opaque(255));
        // 32-bit with an alpha a V5 header vouches for, top-down.
        assert_eq!(pixels(&dib_to_png(&dib(32, true, true, 128)).unwrap()), opaque(128));
        // A plain 32-bit DIB leaves alpha 0: it's opaque, not invisible.
        assert_eq!(pixels(&dib_to_png(&dib(32, false, false, 0)).unwrap()), opaque(255));
        // Windows' own CF_DIBV5 of a bitmap: its masks again after the header, then the pixels.
        assert_eq!(pixels(&dib_to_png(&dib_with(32, true, false, 0, true)).unwrap()), opaque(255));
        // Paletted, or cut short: no PNG, so a .bmp of it as it is.
        let paletted = dib(8, false, false, 0);
        assert!(dib_to_png(&paletted).is_none());
        let full = dib(24, false, false, 0);
        assert!(dib_to_png(&full[..full.len() - 3]).is_none());
        let bmp = dib_to_bmp(&full);
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[2..6].try_into().unwrap()) as usize, 14 + full.len());
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 14 + 40);
        assert_eq!(dib_pixels_at(&paletted), Some(40 + 4 * 256));
    }
}
