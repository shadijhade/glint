//! Windows' own context menu, File Explorer's "Show more options": the shell's verbs and every app's (7-Zip,
//! Git, an editor, an antivirus…) for what's chosen, drawn by Windows and run by the shell, as there.

use tauri::Window;

/// Windows' context menu for `paths` (in one folder) at a point in the window, in CSS pixels. The shell runs
/// what's chosen, except Rename and Delete, which come back ("rename", "delete") for Glint's own, with
/// Undo; "none" when nothing was chosen, otherwise the item's verb ("open", "7-zip.extract"…) or "other".
#[tauri::command]
pub async fn shell_menu(window: Window, paths: Vec<String>, x: f64, y: f64) -> Result<String, String> {
    let owner = crate::shell::owner(&window);
    let scale = window.scale_factor().unwrap_or(1.0);
    let (x, y) = ((x * scale).round() as i32, (y * scale).round() as i32);
    crate::shell::on_main(&window, move || imp::show(owner, &paths, x, y)).await
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use windows::core::{Interface, HSTRING, PCSTR, PCWSTR, PSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Com::IBindCtx;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        DefSubclassProc, IContextMenu, IContextMenu2, IContextMenu3, ILCreateFromPathW, ILFree, RemoveWindowSubclass, SHCreateShellItemArrayFromIDLists, SetWindowSubclass,
        BHID_SFUIObject, CMF_CANRENAME, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_CONTROL_DOWN, CMIC_MASK_PTINVOKE, CMIC_MASK_SHIFT_DOWN, CMINVOKECOMMANDINFO,
        CMINVOKECOMMANDINFOEX, GCS_VERBW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, TrackPopupMenuEx, SW_SHOWNORMAL, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR,
    };

    /// The command ids the shell may give its items.
    const FIRST: u32 = 1;
    const LAST: u32 = 0x7FFF;
    /// Strings in the command info are UTF-16 too (SEE_MASK_UNICODE; the crate doesn't name it for menus).
    const CMIC_MASK_UNICODE: u32 = 0x0000_4000;
    /// This module's subclass of the window, while a menu is open.
    const SUBCLASS: usize = 0x676C_6E74;

    thread_local! {
        /// The menu open on this (the UI) thread, for its submenus that fill themselves in as they open.
        static OPEN: RefCell<Option<(Option<IContextMenu3>, Option<IContextMenu2>)>> = const { RefCell::new(None) };
    }

    /// The window's messages while the menu is open: a submenu that fills itself in (Send to, Open with, an
    /// app's own, with icons) does so from these.
    unsafe extern "system" fn forward(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
            let handled = OPEN.with_borrow(|open| {
                let (third, second) = open.as_ref()?;
                if let Some(menu) = third {
                    let mut result = LRESULT(0);
                    return unsafe { menu.HandleMenuMsg2(msg, wparam, lparam, Some(&mut result)) }.ok().map(|_| result);
                }
                second.as_ref().and_then(|menu| unsafe { menu.HandleMenuMsg(msg, wparam, lparam) }.ok().map(|_| LRESULT(0)))
            });
            if let Some(result) = handled {
                return result;
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    /// Runs on the UI thread: the window that owns the menu gets its messages there, and COM is set up.
    pub fn show(owner: isize, paths: &[String], x: i32, y: i32) -> Result<String, String> {
        let hwnd = HWND(owner as *mut _);
        unsafe {
            let pidls: Vec<*mut ITEMIDLIST> = paths.iter().map(|p| ILCreateFromPathW(&HSTRING::from(p.as_str()))).collect();
            let shown = (|| -> Result<String, String> {
                if pidls.is_empty() || pidls.iter().any(|p| p.is_null()) {
                    return Err("Some of these items no longer exist".into());
                }
                let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p as *const _).collect();
                let menu: IContextMenu = SHCreateShellItemArrayFromIDLists(&list)
                    .and_then(|items| items.BindToHandler(None::<&IBindCtx>, &BHID_SFUIObject))
                    .map_err(|e| e.message())?;
                let popup = CreatePopupMenu().map_err(|e| e.message())?;
                let (shift, ctrl) = (GetKeyState(VK_SHIFT.0 as i32) < 0, GetKeyState(VK_CONTROL.0 as i32) < 0);
                // Rename as File Explorer has it (it comes back for Glint's own), and with Shift held, Windows'
                // extended verbs too (Copy as path, Open in Terminal…).
                let flags = CMF_NORMAL | CMF_CANRENAME | if shift { CMF_EXTENDEDVERBS } else { 0 };
                let chosen = menu.QueryContextMenu(popup, 0, FIRST, LAST, flags).ok().map_err(|e| e.message()).map(|_| {
                    let mut at = POINT { x, y };
                    let _ = ClientToScreen(hwnd, &mut at);
                    OPEN.set(Some((menu.cast().ok(), menu.cast().ok())));
                    let _ = SetWindowSubclass(hwnd, Some(forward), SUBCLASS, 0);
                    let chosen = TrackPopupMenuEx(popup, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, at.x, at.y, hwnd, None).0 as u32;
                    let _ = RemoveWindowSubclass(hwnd, Some(forward), SUBCLASS);
                    OPEN.set(None);
                    (chosen, at)
                });
                let ran = chosen.and_then(|(chosen, at)| {
                    if chosen < FIRST {
                        return Ok("none".to_string());
                    }
                    let offset = (chosen - FIRST) as usize;
                    let verb = verb_of(&menu, offset);
                    if let Some(own @ ("rename" | "delete")) = verb.as_deref() {
                        return Ok(own.to_string());
                    }
                    let mut mask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
                    if shift {
                        mask |= CMIC_MASK_SHIFT_DOWN;
                    }
                    if ctrl {
                        mask |= CMIC_MASK_CONTROL_DOWN;
                    }
                    // The item by its offset, which the shell reads from the verb's place.
                    let info = CMINVOKECOMMANDINFOEX {
                        cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                        fMask: mask,
                        hwnd,
                        lpVerb: PCSTR(offset as *const u8),
                        lpVerbW: PCWSTR(offset as *const u16),
                        nShow: SW_SHOWNORMAL.0,
                        ptInvoke: at,
                        ..Default::default()
                    };
                    menu.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO).map_err(|e| e.message())?;
                    Ok(verb.unwrap_or_else(|| "other".into()))
                });
                let _ = DestroyMenu(popup);
                ran
            })();
            for pidl in pidls.into_iter().filter(|p| !p.is_null()) {
                ILFree(Some(pidl as *const _));
            }
            shown
        }
    }

    /// An item's verb, the same in every language ("rename", "delete", "properties"…), when it has one.
    unsafe fn verb_of(menu: &IContextMenu, offset: usize) -> Option<String> {
        let mut name = [0u16; 128];
        unsafe { menu.GetCommandString(offset, GCS_VERBW, None, PSTR(name.as_mut_ptr() as *mut u8), name.len() as u32) }.ok()?;
        let len = name.iter().position(|&c| c == 0)?;
        (len > 0).then(|| String::from_utf16_lossy(&name[..len]).to_ascii_lowercase())
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn show(_: isize, _: &[String], _: i32, _: i32) -> Result<String, String> {
        Err("Only on Windows".into())
    }
}
