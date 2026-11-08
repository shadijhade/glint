//! Jumping a file dialog to a folder: any app's Open or Save As dialog, put in the folder Glint shows
//! (Ctrl+Alt+G with the dialog in front), or in a folder chosen in quick search (summoned over the
//! dialog: Enter on a folder). It's done as you'd do it by hand: the folder typed in the dialog's file
//! name box and Enter pressed, then the name that was there put back.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;

/// The folder the main window shows, as its page last said.
static FOLDER: Mutex<String> = Mutex::new(String::new());
/// The file dialog that was in front when quick search was summoned (its window handle), or 0.
static DIALOG: AtomicIsize = AtomicIsize::new(0);

/// The page tells which folder its tab shows (on every navigation).
#[tauri::command]
pub fn report_folder(path: String) {
    *FOLDER.lock().unwrap_or_else(|e| e.into_inner()) = path;
}

#[cfg(windows)]
mod win {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_RETURN;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetAncestor, GetClassNameW, GetForegroundWindow, IsWindowVisible, PostMessageW, SendMessageW, SetForegroundWindow, GA_PARENT, WM_GETTEXT, WM_GETTEXTLENGTH,
        WM_KEYDOWN, WM_KEYUP, WM_SETTEXT,
    };

    fn class(hwnd: HWND) -> String {
        let mut buf = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
        String::from_utf16_lossy(&buf[..len])
    }

    /// The dialog's file name box: an edit field inside a combo box, shown.
    fn name_box(dialog: HWND) -> Option<HWND> {
        struct Found(Option<HWND>);
        unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
            let found = unsafe { &mut *(found.0 as *mut Found) };
            if class(hwnd) == "Edit" && unsafe { IsWindowVisible(hwnd) }.as_bool() {
                let parent = unsafe { GetAncestor(hwnd, GA_PARENT) };
                if matches!(class(parent).as_str(), "ComboBox" | "ComboBoxEx32") {
                    found.0 = Some(hwnd);
                    return false.into();
                }
            }
            true.into()
        }
        let mut found = Found(None);
        unsafe {
            let _ = EnumChildWindows(Some(dialog), Some(each), LPARAM(&mut found as *mut Found as isize));
        }
        found.0
    }

    /// Whether `hwnd` is an Open or Save As dialog: a dialog box with a folder view and a file name box.
    pub fn is_file_dialog(hwnd: HWND) -> bool {
        if hwnd.0.is_null() || class(hwnd) != "#32770" {
            return false;
        }
        struct Seen(bool);
        unsafe extern "system" fn each(hwnd: HWND, seen: LPARAM) -> BOOL {
            let seen = unsafe { &mut *(seen.0 as *mut Seen) };
            if matches!(class(hwnd).as_str(), "SHELLDLL_DefView" | "DirectUIHWND") {
                seen.0 = true;
                return false.into();
            }
            true.into()
        }
        let mut seen = Seen(false);
        unsafe {
            let _ = EnumChildWindows(Some(hwnd), Some(each), LPARAM(&mut seen as *mut Seen as isize));
        }
        seen.0 && name_box(hwnd).is_some()
    }

    pub fn foreground() -> HWND {
        unsafe { GetForegroundWindow() }
    }

    /// Puts `dialog` in `folder`, then gives its file name box back what it held.
    pub fn jump(dialog: HWND, folder: &str) -> Result<(), String> {
        let edit = name_box(dialog).ok_or("That dialog has no file name box")?;
        unsafe {
            let len = SendMessageW(edit, WM_GETTEXTLENGTH, None, None).0.max(0) as usize;
            let mut kept = vec![0u16; len + 1];
            SendMessageW(edit, WM_GETTEXT, Some(WPARAM(kept.len())), Some(LPARAM(kept.as_mut_ptr() as isize)));
            let path: Vec<u16> = folder.encode_utf16().chain([0]).collect();
            let _ = SetForegroundWindow(dialog);
            SendMessageW(edit, WM_SETTEXT, None, Some(LPARAM(path.as_ptr() as isize)));
            PostMessageW(Some(edit), WM_KEYDOWN, WPARAM(VK_RETURN.0 as usize), LPARAM(0)).map_err(|e| e.to_string())?;
            PostMessageW(Some(edit), WM_KEYUP, WPARAM(VK_RETURN.0 as usize), LPARAM(0xC000_0001u32 as isize)).map_err(|e| e.to_string())?;
            // Once the dialog has moved there, the name typed before (a Save As's) is back.
            let edit_handle = edit.0 as isize;
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let edit = HWND(edit_handle as *mut _);
                SendMessageW(edit, WM_SETTEXT, None, Some(LPARAM(kept.as_ptr() as isize)));
            });
        }
        Ok(())
    }
}

/// Quick search is being summoned: the file dialog in front, if one is, is remembered for it.
pub fn note_dialog() {
    #[cfg(windows)]
    {
        let front = win::foreground();
        DIALOG.store(if win::is_file_dialog(front) { front.0 as isize } else { 0 }, Ordering::SeqCst);
    }
}

/// Whether quick search was summoned over a file dialog (which a folder chosen in it then goes to).
#[tauri::command]
pub fn quick_dialog() -> bool {
    DIALOG.load(Ordering::SeqCst) != 0
}

/// Puts the file dialog quick search was summoned over in `folder`; false when there was none.
#[tauri::command]
pub fn quick_jump(folder: String) -> Result<bool, String> {
    let dialog = DIALOG.swap(0, Ordering::SeqCst);
    if dialog == 0 {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        let hwnd = windows::Win32::Foundation::HWND(dialog as *mut _);
        if !win::is_file_dialog(hwnd) {
            return Ok(false);
        }
        win::jump(hwnd, &folder)?;
    }
    Ok(true)
}

/// Ctrl+Alt+G, on a thread of its own: the file dialog in front goes to the folder Glint shows.
pub fn listen() {
    #[cfg(windows)]
    std::thread::spawn(|| unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT};
        use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};
        // Another app may have it: then there's no shortcut, and quick search still jumps dialogs.
        if RegisterHotKey(None, 7, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, 'G' as u32).is_err() {
            return;
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            if message.message != WM_HOTKEY {
                continue;
            }
            let front = win::foreground();
            let folder = FOLDER.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if !folder.is_empty() && win::is_file_dialog(front) {
                let _ = win::jump(front, &folder);
            }
        }
    });
}
