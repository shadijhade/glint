//! Files dragged onto Glint's windows. Glint's own drop target takes the webview's place: Tauri's sends
//! what it hears through the app's event loop, which waits while a drag Glint itself started is under
//! way (that drag runs on the main thread: `start_drag` in shell.rs), so until the drop nothing under
//! the pointer could light up and no tab could open. This one tells the page at once, from the main
//! thread, shows the dragged items' picture over the window as File Explorer does, and says on it what
//! a drop does there: a copy, or a move with Shift ("Move to Documents").

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

/// What the page says is under the pointer: a folder a drop would go into.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Under {
    /// The folder's name, for the words on the drag.
    name: String,
    /// Those words for a copy and for a move, in Glint's language, `%1` standing for the name.
    copy: String,
    #[serde(rename = "move")]
    moves: String,
    /// A drop there always moves, whatever the keys: the Recycle Bin.
    #[serde(default)]
    always_move: bool,
}

/// What the page last said: `None` before it has said anything about this drag (a drop is let through
/// then), `Some(None)` where nothing can be dropped.
static UNDER: Mutex<Option<Option<Under>>> = Mutex::new(None);

fn under() -> std::sync::MutexGuard<'static, Option<Option<Under>>> {
    UNDER.lock().unwrap_or_else(|e| e.into_inner())
}

/// The page, as the pointer comes over something else: what a drop there would go into.
#[tauri::command]
pub fn drag_under(under: Option<Under>) {
    *self::under() = Some(under);
}

/// What the page hears ("glint://drag"), the pointer in physical pixels from the webview's corner.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Heard {
    Enter {
        paths: Vec<String>,
        x: i32,
        y: i32,
    },
    Over {
        x: i32,
        y: i32,
    },
    Drop {
        paths: Vec<String>,
        x: i32,
        y: i32,
        #[serde(rename = "move")]
        moves: bool,
    },
    Leave,
}

/// Whether a drop moves: with Shift (or into the Recycle Bin), where the source lets it, or where it
/// only lets it move.
fn moves(shift: bool, always: bool, can_copy: bool, can_move: bool) -> bool {
    can_move && (shift || always || !can_copy)
}

/// Takes over drops on an Explorer window's webview: again after a reload (the same windows, a new target).
pub fn register(window: &tauri::WebviewWindow) {
    #[cfg(windows)]
    {
        let window = window.clone();
        let target = window.clone();
        let _ = window.run_on_main_thread(move || imp::register(&target));
    }
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
mod imp {
    use super::{moves, under, Heard, Under};
    use std::cell::{Cell, RefCell};
    use std::mem::{size_of, ManuallyDrop};
    use tauri::{Emitter, WebviewWindow};
    use windows::core::{implement, Ref, BOOL, PCWSTR};
    use windows::Win32::Foundation::{GlobalFree, DRAGDROP_E_INVALIDHWND, HWND, LPARAM, POINT, POINTL};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::System::Com::{CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL};
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::System::Ole::{
        IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    };
    use windows::Win32::System::SystemServices::{MK_SHIFT, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Shell::{
        DragQueryFileW, IDropTargetHelper, CFSTR_DROPDESCRIPTION, CFSTR_LOGICALPERFORMEDDROPEFFECT, CFSTR_PERFORMEDDROPEFFECT, CLSID_DragDropHelper, DROPDESCRIPTION,
        DROPIMAGETYPE, DROPIMAGE_COPY, DROPIMAGE_INVALID, DROPIMAGE_MOVE, DROPIMAGE_NONE, HDROP,
    };
    use windows::Win32::UI::WindowsAndMessaging::EnumChildWindows;

    pub fn register(window: &WebviewWindow) {
        let Ok(top) = window.hwnd() else { return };
        let top = HWND(top.0 as _);
        // Draws what's dragged over the window, with the words on it.
        let helper = unsafe { CoCreateInstance::<_, IDropTargetHelper>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        // The webview's own windows, as Tauri (wry) finds them for its drop target.
        let mut children: Vec<HWND> = Vec::new();
        unsafe extern "system" fn each(child: HWND, list: LPARAM) -> BOOL {
            (*(list.0 as *mut Vec<HWND>)).push(child);
            true.into()
        }
        unsafe {
            let _ = EnumChildWindows(Some(top), Some(each), LPARAM(&mut children as *mut Vec<HWND> as isize));
        }
        for child in children {
            let target: IDropTarget = Target {
                hwnd: child,
                window: window.clone(),
                helper: helper.clone(),
                data: RefCell::new(None),
                files: Cell::new(false),
                allowed: Cell::new(DROPEFFECT_NONE),
                sent: Cell::new(None),
                said: RefCell::new(None),
            }
            .into();
            unsafe {
                if RevokeDragDrop(child) != Err(DRAGDROP_E_INVALIDHWND.into()) {
                    let _ = RegisterDragDrop(child, &target);
                }
            }
        }
    }

    #[implement(IDropTarget)]
    struct Target {
        /// The window it's registered on, which the pointer is measured from.
        hwnd: HWND,
        window: WebviewWindow,
        helper: Option<IDropTargetHelper>,
        /// What's being dragged, while it's over the window: the words go on it.
        data: RefCell<Option<IDataObject>>,
        /// Whether it carries files (only files are taken).
        files: Cell<bool>,
        /// What its source lets a drop do.
        allowed: Cell<DROPEFFECT>,
        /// Where the page was last told the pointer is.
        sent: Cell<Option<(i32, i32)>>,
        /// The words last put on the drag.
        said: RefCell<Option<(i32, String, String)>>,
    }

    impl Target {
        fn point(&self, pt: &POINTL) -> (i32, i32) {
            let mut p = POINT { x: pt.x, y: pt.y };
            unsafe {
                let _ = ScreenToClient(self.hwnd, &mut p);
            }
            (p.x, p.y)
        }

        fn tell(&self, heard: Heard) {
            let _ = self.window.emit_to(self.window.label(), "glint://drag", heard);
        }

        /// What a drop here does now (nothing, a copy or a move) and into what, as far as the page has said.
        fn effect(&self, keys: MODIFIERKEYS_FLAGS) -> (DROPEFFECT, Option<Under>) {
            if !self.files.get() {
                return (DROPEFFECT_NONE, None);
            }
            let said = under().clone();
            let into = match said {
                Some(None) => return (DROPEFFECT_NONE, None),
                Some(Some(into)) => Some(into),
                None => None,
            };
            let allowed = self.allowed.get();
            let (can_copy, can_move) = (allowed.0 & DROPEFFECT_COPY.0 != 0, allowed.0 & DROPEFFECT_MOVE.0 != 0);
            let effect = if moves(keys.0 & MK_SHIFT.0 != 0, into.as_ref().is_some_and(|u| u.always_move), can_copy, can_move) {
                DROPEFFECT_MOVE
            } else if can_copy {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
            (effect, into)
        }

        /// Puts what a drop does on the dragged picture, when that changed.
        fn describe(&self, effect: DROPEFFECT, into: Option<&Under>) {
            let data = self.data.borrow();
            let Some(data) = data.as_ref() else { return };
            let (kind, message, insert) = match into {
                Some(u) if effect == DROPEFFECT_MOVE => (DROPIMAGE_MOVE, u.moves.clone(), u.name.clone()),
                Some(u) if effect == DROPEFFECT_COPY => (DROPIMAGE_COPY, u.copy.clone(), u.name.clone()),
                _ if effect == DROPEFFECT_NONE => (DROPIMAGE_NONE, String::new(), String::new()),
                // Before the page has said: the source's own picture.
                _ => (DROPIMAGE_INVALID, String::new(), String::new()),
            };
            let now = (kind.0, message, insert);
            if self.said.borrow().as_ref() == Some(&now) {
                return;
            }
            unsafe { words(data, DROPIMAGETYPE(now.0), &now.1, &now.2) };
            *self.said.borrow_mut() = Some(now);
        }

        fn done(&self) {
            self.data.replace(None);
            self.said.replace(None);
            self.sent.set(None);
            self.files.set(false);
            *under() = None;
        }
    }

    #[allow(non_snake_case)]
    impl IDropTarget_Impl for Target_Impl {
        fn DragEnter(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
            // The page says anew what's under the pointer.
            self.done();
            self.allowed.set(unsafe { *effect });
            let data = data.as_ref();
            let paths = data.map(|d| unsafe { files(d) }).unwrap_or_default();
            self.files.set(!paths.is_empty());
            *self.data.borrow_mut() = data.cloned();
            let (chosen, into) = self.effect(keys);
            self.describe(chosen, into.as_ref());
            unsafe { *effect = chosen };
            if let (Some(helper), Some(data)) = (&self.helper, data) {
                let at = POINT { x: pt.x, y: pt.y };
                unsafe {
                    let _ = helper.DragEnter(self.hwnd, data, &at, chosen);
                }
            }
            if !paths.is_empty() {
                let (x, y) = self.point(pt);
                self.sent.set(Some((x, y)));
                self.tell(Heard::Enter { paths, x, y });
            }
            Ok(())
        }

        fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
            self.allowed.set(unsafe { *effect });
            let (chosen, into) = self.effect(keys);
            self.describe(chosen, into.as_ref());
            unsafe { *effect = chosen };
            if let Some(helper) = &self.helper {
                let at = POINT { x: pt.x, y: pt.y };
                unsafe {
                    let _ = helper.DragOver(&at, chosen);
                }
            }
            if self.files.get() {
                let (x, y) = self.point(pt);
                if self.sent.get() != Some((x, y)) {
                    self.sent.set(Some((x, y)));
                    self.tell(Heard::Over { x, y });
                }
            }
            Ok(())
        }

        fn DragLeave(&self) -> windows::core::Result<()> {
            if let Some(helper) = &self.helper {
                unsafe {
                    let _ = helper.DragLeave();
                }
            }
            // Past the window, the source's own words again.
            if let Some(data) = self.data.borrow().as_ref() {
                unsafe { words(data, DROPIMAGE_INVALID, "", "") };
            }
            if self.files.get() {
                self.tell(Heard::Leave);
            }
            self.done();
            Ok(())
        }

        fn Drop(&self, data: Ref<IDataObject>, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
            let allowed = unsafe { *effect };
            self.allowed.set(allowed);
            let (chosen, into) = self.effect(keys);
            let data = data.as_ref();
            if let (Some(helper), Some(data)) = (&self.helper, data) {
                let at = POINT { x: pt.x, y: pt.y };
                unsafe {
                    let _ = helper.Drop(data, &at, chosen);
                }
            }
            let paths = data.map(|d| unsafe { files(d) }).unwrap_or_default();
            // The page finds what's under the pointer itself (what it last said may be a step behind).
            let moved = moves(
                keys.0 & MK_SHIFT.0 != 0,
                into.is_some_and(|u| u.always_move),
                allowed.0 & DROPEFFECT_COPY.0 != 0,
                allowed.0 & DROPEFFECT_MOVE.0 != 0,
            );
            if !paths.is_empty() {
                let (x, y) = self.point(pt);
                self.tell(Heard::Drop { paths, x, y, moves: moved });
            }
            // Glint moves the files itself (an optimized move, in Windows' words): the source is told not
            // to delete them afterwards, as File Explorer tells it.
            let result = if moved && chosen != DROPEFFECT_NONE {
                if let Some(data) = data {
                    unsafe {
                        set_dword(data, CFSTR_PERFORMEDDROPEFFECT, DROPEFFECT_NONE.0);
                        set_dword(data, CFSTR_LOGICALPERFORMEDDROPEFFECT, DROPEFFECT_MOVE.0);
                    }
                }
                DROPEFFECT_NONE
            } else {
                chosen
            };
            unsafe { *effect = result };
            self.done();
            Ok(())
        }
    }

    /// The files a drag carries.
    unsafe fn files(data: &IDataObject) -> Vec<String> {
        let format = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let Ok(mut medium) = data.GetData(&format) else { return Vec::new() };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            paths.push(String::from_utf16_lossy(&buf[..len]));
        }
        ReleaseStgMedium(&mut medium);
        paths
    }

    /// Hands the drag's data `bytes` in a format of its own; the data keeps the memory once it takes it.
    unsafe fn set(data: &IDataObject, format: PCWSTR, bytes: &[u8]) {
        let cf = RegisterClipboardFormatW(format);
        if cf == 0 {
            return;
        }
        let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { return };
        let at = GlobalLock(global);
        if at.is_null() {
            let _ = GlobalFree(Some(global));
            return;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        let _ = GlobalUnlock(global);
        let format = FORMATETC { cfFormat: cf as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let medium = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: global }, pUnkForRelease: ManuallyDrop::new(None) };
        if data.SetData(&format, &medium, true).is_err() {
            let _ = GlobalFree(Some(global));
        }
    }

    unsafe fn set_dword(data: &IDataObject, format: PCWSTR, value: u32) {
        set(data, format, &value.to_ne_bytes());
    }

    /// The words on the dragged picture: `message` with `%1` for `insert`, beside the kind's icon.
    unsafe fn words(data: &IDataObject, kind: DROPIMAGETYPE, message: &str, insert: &str) {
        // The struct is packed: its text is made apart, then put in whole.
        let text = |text: &str| {
            let mut out = [0u16; 260];
            for (slot, unit) in out.iter_mut().zip(text.encode_utf16().take(259)) {
                *slot = unit;
            }
            out
        };
        let description = DROPDESCRIPTION { r#type: kind, szMessage: text(message), szInsert: text(insert) };
        let bytes = std::slice::from_raw_parts(&description as *const DROPDESCRIPTION as *const u8, size_of::<DROPDESCRIPTION>());
        set(data, CFSTR_DROPDESCRIPTION, bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_moves_and_a_plain_drop_copies() {
        assert!(!moves(false, false, true, true));
        assert!(moves(true, false, true, true));
        // Out of an archive the source only copies: Shift can't make it a move.
        assert!(!moves(true, false, true, false));
        // A source that only moves.
        assert!(moves(false, false, false, true));
        // The Recycle Bin.
        assert!(moves(false, true, true, true));
    }

    #[test]
    fn the_page_hears_what_it_expects() {
        let drop = serde_json::to_value(Heard::Drop { paths: vec!["C:\\a".into()], x: 3, y: 4, moves: true }).unwrap();
        assert_eq!(drop, serde_json::json!({ "type": "drop", "paths": ["C:\\a"], "x": 3, "y": 4, "move": true }));
        assert_eq!(serde_json::to_value(Heard::Leave).unwrap(), serde_json::json!({ "type": "leave" }));
        let said: Under = serde_json::from_value(serde_json::json!({ "name": "Docs", "copy": "Copy to %1", "move": "Move to %1" })).unwrap();
        assert_eq!(said, Under { name: "Docs".into(), copy: "Copy to %1".into(), moves: "Move to %1".into(), always_move: false });
    }
}
