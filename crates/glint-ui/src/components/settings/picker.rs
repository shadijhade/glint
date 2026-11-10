//! Windows' own file and folder pickers, for Settings (the TypeScript used Tauri's dialog plugin).
//!
//! The pickers run a modal loop, so they never run inside a GPUI callback: [`pick`] posts the work to the UI
//! thread's message pump (`core_bridge::post_to_ui`) and the answer comes back through a channel the GPUI task
//! waits on. The owner window is the main window's HWND, taken while the caller still has the `Window`.
//!
//! TODO(coordinator): the other wave-5 parts (rules, remotes, backups) need the same pickers; this file is a
//! candidate to move next to `platform/` as `platform::pickers`.

use crate::core_bridge;
use windows::core::{Interface as _, HSTRING, PCWSTR};
use windows::Win32::Foundation::{ERROR_CANCELLED, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem,
    SHCreateItemFromParsingName, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FOS_PICKFOLDERS, SIGDN_FILESYSPATH,
};

/// What to ask for.
#[derive(Clone, Debug)]
pub enum Ask {
    /// A folder.
    Folder,
    /// An existing file of a kind: `("Glint settings", "json")`.
    Open(&'static str, &'static str),
    /// A file to write, of a kind, with a suggested name.
    Save(&'static str, &'static str, String),
}

/// Asks the person for a path with Windows' picker, owned by `owner` (a window handle, 0 for none). `None` when
/// they cancel. Resolves after the picker closes; the picker itself runs on the UI thread's message pump.
pub async fn pick(ask: Ask, title: String, start: Option<String>, owner: isize) -> Result<Option<String>, String> {
    let (answer, heard) = async_channel::bounded(1);
    core_bridge::post_to_ui(move || {
        let result = show(&ask, &title, start.as_deref(), owner);
        let _ = answer.try_send(result);
    });
    heard
        .recv()
        .await
        .map_err(|_| "the picker was closed without an answer".to_string())?
}

/// Shows the picker and waits. UI thread, from the message pump only.
fn show(ask: &Ask, title: &str, start: Option<&str>, owner: isize) -> Result<Option<String>, String> {
    // SAFETY: COM calls on the UI thread, which GPUI has already put in a single-threaded apartment (a second
    // CoInitializeEx with the same mode is harmless; a different mode is reported and ignored). The owner is a
    // window handle that may be stale, which the dialog tolerates by showing unowned. Every string handed to
    // COM outlives the call, and the one COM gives back is freed once.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        run(ask, title, start, HWND(owner as *mut _)).map_err(|error| error.message())
    }
}

/// # Safety
/// UI thread; see [`show`].
unsafe fn run(ask: &Ask, title: &str, start: Option<&str>, owner: HWND) -> windows::core::Result<Option<String>> {
    let dialog: IFileDialog = match ask {
        Ask::Folder | Ask::Open(..) => {
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?.cast()?
        }
        Ask::Save(..) => CoCreateInstance::<_, IFileSaveDialog>(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?.cast()?,
    };
    dialog.SetTitle(&HSTRING::from(title))?;
    let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM;
    match ask {
        Ask::Folder => options |= FOS_PICKFOLDERS,
        Ask::Save(..) => options |= FOS_OVERWRITEPROMPT,
        Ask::Open(..) => {}
    }
    dialog.SetOptions(options)?;
    if let Ask::Open(name, extension) | Ask::Save(name, extension, _) = ask {
        let name = HSTRING::from(*name);
        let spec = HSTRING::from(format!("*.{extension}"));
        let types = [COMDLG_FILTERSPEC {
            pszName: PCWSTR(name.as_ptr()),
            pszSpec: PCWSTR(spec.as_ptr()),
        }];
        dialog.SetFileTypes(&types)?;
        dialog.SetDefaultExtension(&HSTRING::from(*extension))?;
    }
    if let Ask::Save(_, _, file_name) = ask {
        dialog.SetFileName(&HSTRING::from(file_name.as_str()))?;
    }
    if let Some(folder) = start.filter(|s| !s.trim().is_empty()) {
        // A folder that's gone just leaves the dialog where Windows would put it.
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(folder), None) {
            let _ = dialog.SetFolder(&item);
        }
    }
    match dialog.Show(Some(owner)) {
        Ok(()) => {}
        Err(error) if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) => return Ok(None),
        Err(error) => return Err(error),
    }
    let item = dialog.GetResult()?;
    let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
    let path = name.to_string().ok();
    CoTaskMemFree(Some(name.0 as *const _));
    Ok(path)
}
