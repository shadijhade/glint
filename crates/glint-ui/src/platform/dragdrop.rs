//! Glint's own drop target and drag-out on GPUI windows.
//!
//! Ported from src-tauri/src/dragdrop.rs (the IDropTarget) and glint-core's shell drag-out, using what
//! docs/gpui/reference/spike-phase0.rs verified: `RevokeDragDrop` on the GPUI window, then `RegisterDragDrop`
//! with ours. GPUI's own target only hands over paths and always copies; Glint's decides the effect itself
//! (copy by default, Shift moves, Alt or Ctrl+Shift makes a shortcut, the Recycle Bin always moves), shows
//! the words on the dragged picture ("Move to Documents") as File Explorer does, and tells the UI.
//!
//! **How the pieces talk.** OLE calls the target on the UI thread and expects an answer at once, so the
//! target never waits for GPUI:
//!
//! - It sends typed [`DropEvent`]s (paths, the effect, the pointer in the window's *logical* pixels) down an
//!   unbounded channel that a GPUI task reads ([`DropTarget::events`]).
//! - The views decide what is under the pointer (a folder row, a tab, the address bar) and say so through
//!   [`DropAnswers::set`] when they handle an `Enter` or `Over`. The target keeps the last [`Answer`] and
//!   uses it for the next callback, so the effect lags the pointer by one step at most, as it did on the web.
//!
//! **Drag-out** is [`drag_out`]: it hands glint-core's `shell::start_drag` (the shell's data object, the drag
//! picture, `SHDoDragDrop`) to the UI thread through `core_bridge::post_to_ui`, so its modal loop runs from
//! the message pump and never inside a GPUI callback.

use crate::core_bridge;
use gpui_kit::App;
use std::cell::{Cell, RefCell};
use std::mem::{size_of, ManuallyDrop};
use std::sync::{Arc, Mutex};
use windows::core::{implement, Ref, PCWSTR};
use windows::Win32::Foundation::{GlobalFree, HWND, POINT, POINTL};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoCreateInstance, IDataObject, CLSCTX_INPROC_SERVER, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0,
    TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, DragQueryFileW, IDropTargetHelper, CFSTR_DROPDESCRIPTION, CFSTR_LOGICALPERFORMEDDROPEFFECT,
    CFSTR_PERFORMEDDROPEFFECT, DROPDESCRIPTION, DROPIMAGETYPE, DROPIMAGE_COPY, DROPIMAGE_INVALID, DROPIMAGE_LINK,
    DROPIMAGE_MOVE, DROPIMAGE_NONE, HDROP,
};

/// Modifier-key bits in the key state OLE passes (the windows crate leaves `MK_ALT` out).
const MK_SHIFT: u32 = 0x04;
const MK_CONTROL: u32 = 0x08;
const MK_ALT: u32 = 0x20;

/// What a drop does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DropEffect {
    /// Nothing can be dropped here.
    #[default]
    None,
    /// Copy the files in.
    Copy,
    /// Move the files in.
    Move,
    /// Make shortcuts to the files.
    Link,
}

impl DropEffect {
    fn to_ole(self) -> DROPEFFECT {
        match self {
            DropEffect::None => DROPEFFECT_NONE,
            DropEffect::Copy => DROPEFFECT_COPY,
            DropEffect::Move => DROPEFFECT_MOVE,
            DropEffect::Link => DROPEFFECT_LINK,
        }
    }
}

/// What the UI says is under the pointer: a place a drop would go into.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Under {
    /// The place's name (a folder, a drive), for the words on the dragged picture.
    pub name: String,
    /// Those words for a copy, in Glint's language, `%1` standing for the name. Empty keeps the source's own picture.
    pub copy: String,
    /// The words for a move.
    pub moves: String,
    /// A drop there always moves, whatever the keys: the Recycle Bin.
    pub always_move: bool,
}

/// What the UI last said about the region under the pointer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Answer {
    /// Nothing said yet about this drag: a drop is let through with Glint's key rules.
    #[default]
    Unknown,
    /// Nothing can be dropped here.
    Refuse,
    /// A drop here goes into this.
    Accept(Under),
}

/// The UI's side of the conversation: cheap to clone, safe from any thread. The target reads it without
/// waiting for anything but a brief lock.
#[derive(Clone, Default)]
pub struct DropAnswers(Arc<Mutex<Answer>>);

impl DropAnswers {
    /// Says what a drop under the pointer would do. Stays in force until the next call or the end of the drag.
    pub fn set(&self, answer: Answer) {
        *self.lock() = answer;
    }

    /// The last answer.
    pub fn get(&self) -> Answer {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Answer> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// What the target tells the UI. Positions are the pointer in the window's logical pixels (physical pixels
/// divided by the window's DPI scale), from the window's top-left corner.
#[derive(Clone, Debug, PartialEq)]
pub enum DropEvent {
    /// Files came over the window. Answer with [`DropAnswers::set`] as the pointer moves.
    Enter {
        paths: Vec<String>,
        x: f32,
        y: f32,
        effect: DropEffect,
    },
    /// The pointer moved with the drag over the window.
    Over { x: f32, y: f32, effect: DropEffect },
    /// The drag left the window (or was cancelled).
    Leave,
    /// The files were dropped. `effect` is what the drop does (`Move` with Shift, into the Recycle Bin…).
    /// `effect` [`DropEffect::None`] means the place under the pointer refused it (its last answer was
    /// [`Answer::Refuse`]): do nothing with the files.
    Drop {
        paths: Vec<String>,
        x: f32,
        y: f32,
        effect: DropEffect,
    },
}

/// The effect Glint's rules give for the key state `keys` (OLE's MK_ bits), what the source `allowed`
/// (DROPEFFECT bits) and whether the place `always_move`s. Copy by default; Shift moves where the source lets
/// it; Alt or Ctrl+Shift makes a shortcut; a source that only moves, or the Recycle Bin, moves.
pub fn effect_for(keys: u32, allowed: u32, always_move: bool) -> DropEffect {
    let can = |effect: DROPEFFECT| allowed & effect.0 != 0;
    let (can_copy, can_move, can_link) = (can(DROPEFFECT_COPY), can(DROPEFFECT_MOVE), can(DROPEFFECT_LINK));
    let shift = keys & MK_SHIFT != 0;
    let link_keys = keys & MK_ALT != 0 || (keys & MK_CONTROL != 0 && shift);
    if link_keys && can_link && !always_move {
        DropEffect::Link
    } else if can_move && (shift || always_move || !can_copy) {
        DropEffect::Move
    } else if can_copy {
        DropEffect::Copy
    } else if can_link {
        DropEffect::Link
    } else {
        DropEffect::None
    }
}

/// A pixel position in logical pixels, for a window at `dpi` (96 is 100%; 0, which Windows answers for a
/// window it can't find, counts as 96).
pub fn to_logical(physical: i32, dpi: u32) -> f32 {
    let dpi = if dpi == 0 { 96 } else { dpi };
    physical as f32 * 96.0 / dpi as f32
}

/// A registered drop target on one Glint window. Keep it for as long as the window lives: dropping it
/// takes the target off the window.
pub struct DropTarget {
    /// What the target heard, in order. Read it from a GPUI task (`while let Ok(e) = events.recv().await`) and
    /// answer through [`DropTarget::answers`].
    pub events: async_channel::Receiver<DropEvent>,
    /// Where the UI says what is under the pointer.
    pub answers: DropAnswers,
    hwnd: HWND,
    // Keeps the COM object alive as long as it is registered.
    _target: IDropTarget,
}

impl Drop for DropTarget {
    fn drop(&mut self) {
        // SAFETY: revoking a registration this value made; a window that has gone makes it fail, harmlessly.
        unsafe {
            let _ = RevokeDragDrop(self.hwnd);
        }
    }
}

/// Takes over drops on the GPUI window `hwnd` (get it with `core_bridge::hwnd_of(window)`): revokes GPUI's
/// target and registers Glint's. Call on the UI thread, once per window, after its first frame. OLE must be
/// initialized there (GPUI does it).
pub fn register(hwnd: HWND) -> Result<DropTarget, String> {
    // The picture dragged over the window, with the words on it; without it the drag still works.
    // SAFETY: creating the shell's drag-drop helper object in-process.
    let helper =
        unsafe { CoCreateInstance::<_, IDropTargetHelper>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
    let (target, events, answers) = make_target(hwnd, helper);
    // SAFETY: registering an object we own on a window handle; both calls are fine on a window that has none.
    unsafe {
        let _ = RevokeDragDrop(hwnd);
        RegisterDragDrop(hwnd, &target).map_err(|e| e.message())?;
    }
    Ok(DropTarget {
        events,
        answers,
        hwnd,
        _target: target,
    })
}

fn make_target(
    hwnd: HWND,
    helper: Option<IDropTargetHelper>,
) -> (IDropTarget, async_channel::Receiver<DropEvent>, DropAnswers) {
    let (sender, events) = async_channel::unbounded();
    let answers = DropAnswers::default();
    let target: IDropTarget = Target {
        hwnd,
        helper,
        sender,
        answers: answers.clone(),
        data: RefCell::new(None),
        files: Cell::new(false),
        allowed: Cell::new(0),
        sent: Cell::new(None),
        said: RefCell::new(None),
    }
    .into();
    (target, events, answers)
}

#[implement(IDropTarget)]
struct Target {
    /// The window it is registered on, which the pointer is measured from.
    hwnd: HWND,
    helper: Option<IDropTargetHelper>,
    sender: async_channel::Sender<DropEvent>,
    answers: DropAnswers,
    /// What is being dragged, while it is over the window: the words go on it.
    data: RefCell<Option<IDataObject>>,
    /// Whether it carries files (only files are taken).
    files: Cell<bool>,
    /// What its source lets a drop do (DROPEFFECT bits).
    allowed: Cell<u32>,
    /// Where the UI was last told the pointer is.
    sent: Cell<Option<(i32, i32)>>,
    /// The words last put on the drag.
    said: RefCell<Option<(i32, String, String)>>,
}

impl Target {
    /// The pointer in logical pixels from the window's corner.
    fn point(&self, pt: &POINTL) -> (f32, f32, (i32, i32)) {
        let mut p = POINT { x: pt.x, y: pt.y };
        // SAFETY: converting a point for a window handle; a dead window leaves the point as it was.
        let dpi = unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
            GetDpiForWindow(self.hwnd)
        };
        (to_logical(p.x, dpi), to_logical(p.y, dpi), (p.x, p.y))
    }

    fn tell(&self, event: DropEvent) {
        // Unbounded: never blocks OLE's callback. It only fails once the UI has stopped listening.
        let _ = self.sender.try_send(event);
    }

    /// What a drop here does now, and into what, as far as the UI has said.
    fn effect(&self, keys: MODIFIERKEYS_FLAGS) -> (DropEffect, Option<Under>) {
        if !self.files.get() {
            return (DropEffect::None, None);
        }
        let into = match self.answers.get() {
            Answer::Refuse => return (DropEffect::None, None),
            Answer::Accept(into) => Some(into),
            Answer::Unknown => None,
        };
        let always = into.as_ref().is_some_and(|u| u.always_move);
        (effect_for(keys.0, self.allowed.get(), always), into)
    }

    /// Puts what a drop does on the dragged picture, when that changed.
    fn describe(&self, effect: DropEffect, into: Option<&Under>) {
        let data = self.data.borrow();
        let Some(data) = data.as_ref() else { return };
        let (kind, message, insert) = match (into, effect) {
            (Some(u), DropEffect::Move) if !u.moves.is_empty() => (DROPIMAGE_MOVE, u.moves.clone(), u.name.clone()),
            (Some(u), DropEffect::Copy) if !u.copy.is_empty() => (DROPIMAGE_COPY, u.copy.clone(), u.name.clone()),
            (Some(_), DropEffect::Link) => (DROPIMAGE_LINK, String::new(), String::new()),
            (_, DropEffect::None) => (DROPIMAGE_NONE, String::new(), String::new()),
            // Before the UI has said: the source's own picture.
            _ => (DROPIMAGE_INVALID, String::new(), String::new()),
        };
        let now = (kind.0, message, insert);
        if self.said.borrow().as_ref() == Some(&now) {
            return;
        }
        // SAFETY: `data` is the live data object of this drag.
        unsafe { words(data, DROPIMAGETYPE(now.0), &now.1, &now.2) };
        *self.said.borrow_mut() = Some(now);
    }

    fn done(&self) {
        self.data.replace(None);
        self.said.replace(None);
        self.sent.set(None);
        self.files.set(false);
        self.answers.set(Answer::Unknown);
    }
}

#[allow(non_snake_case)] // OLE's method names.
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // The UI says anew what is under the pointer.
        self.done();
        // SAFETY: OLE passes a valid pointer to the effects the source allows.
        self.allowed.set(unsafe { *effect }.0);
        let data = data.as_ref();
        // SAFETY: `d` is the drag's live data object.
        let paths = data.map(|d| unsafe { files(d) }).unwrap_or_default();
        self.files.set(!paths.is_empty());
        *self.data.borrow_mut() = data.cloned();
        let (chosen, into) = self.effect(keys);
        self.describe(chosen, into.as_ref());
        // SAFETY: as above, the out-pointer is valid for the call.
        unsafe { *effect = chosen.to_ole() };
        if let (Some(helper), Some(data)) = (&self.helper, data) {
            let at = POINT { x: pt.x, y: pt.y };
            // SAFETY: the helper draws the picture for this drag; failure just means no picture.
            unsafe {
                let _ = helper.DragEnter(self.hwnd, data, &at, chosen.to_ole());
            }
        }
        if !paths.is_empty() {
            let (x, y, physical) = self.point(pt);
            self.sent.set(Some(physical));
            self.tell(DropEvent::Enter {
                paths,
                x,
                y,
                effect: chosen,
            });
        }
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        // SAFETY: OLE passes a valid pointer to the effects the source allows.
        self.allowed.set(unsafe { *effect }.0);
        let (chosen, into) = self.effect(keys);
        self.describe(chosen, into.as_ref());
        // SAFETY: as above.
        unsafe { *effect = chosen.to_ole() };
        if let Some(helper) = &self.helper {
            let at = POINT { x: pt.x, y: pt.y };
            // SAFETY: see DragEnter.
            unsafe {
                let _ = helper.DragOver(&at, chosen.to_ole());
            }
        }
        if self.files.get() {
            let (x, y, physical) = self.point(pt);
            if self.sent.get() != Some(physical) {
                self.sent.set(Some(physical));
                self.tell(DropEvent::Over { x, y, effect: chosen });
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if let Some(helper) = &self.helper {
            // SAFETY: see DragEnter.
            unsafe {
                let _ = helper.DragLeave();
            }
        }
        // Past the window, the source's own words again.
        if let Some(data) = self.data.borrow().as_ref() {
            // SAFETY: the drag's live data object.
            unsafe { words(data, DROPIMAGE_INVALID, "", "") };
        }
        if self.files.get() {
            self.tell(DropEvent::Leave);
        }
        self.done();
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // SAFETY: OLE passes a valid pointer to the effects the source allows.
        self.allowed.set(unsafe { *effect }.0);
        let (chosen, _) = self.effect(keys);
        let data = data.as_ref();
        if let (Some(helper), Some(data)) = (&self.helper, data) {
            let at = POINT { x: pt.x, y: pt.y };
            // SAFETY: see DragEnter.
            unsafe {
                let _ = helper.Drop(data, &at, chosen.to_ole());
            }
        }
        // SAFETY: `d` is the drag's live data object.
        let paths = data.map(|d| unsafe { files(d) }).unwrap_or_default();
        if !paths.is_empty() {
            let (x, y, _) = self.point(pt);
            self.tell(DropEvent::Drop {
                paths,
                x,
                y,
                effect: chosen,
            });
        }
        // Glint moves the files itself (an optimized move, in Windows' words): the source is told not to
        // delete them afterwards, as File Explorer tells it.
        let result = if chosen == DropEffect::Move {
            if let Some(data) = data {
                // SAFETY: the drag's live data object.
                unsafe {
                    set_dword(data, CFSTR_PERFORMEDDROPEFFECT, DROPEFFECT_NONE.0);
                    set_dword(data, CFSTR_LOGICALPERFORMEDDROPEFFECT, DROPEFFECT_MOVE.0);
                }
            }
            DROPEFFECT_NONE
        } else {
            chosen.to_ole()
        };
        // SAFETY: as above, the out-pointer is valid for the call.
        unsafe { *effect = result };
        self.done();
        Ok(())
    }
}

/// The files a drag carries (CF_HDROP), as paths. Empty when it carries none.
///
/// # Safety
/// `data` must be a live data object.
unsafe fn files(data: &IDataObject) -> Vec<String> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let Ok(mut medium) = data.GetData(&format) else {
        return Vec::new();
    };
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

/// Hands the drag's data `bytes` in a format of its own; the data keeps the memory once it takes it.
///
/// # Safety
/// `data` must be a live data object.
unsafe fn set(data: &IDataObject, format: PCWSTR, bytes: &[u8]) {
    let cf = RegisterClipboardFormatW(format);
    if cf == 0 {
        return;
    }
    let Ok(global) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
        return;
    };
    let at = GlobalLock(global);
    if at.is_null() {
        let _ = GlobalFree(Some(global));
        return;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
    let _ = GlobalUnlock(global);
    let format = FORMATETC {
        cfFormat: cf as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        u: STGMEDIUM_0 { hGlobal: global },
        pUnkForRelease: ManuallyDrop::new(None),
    };
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
    let description = DROPDESCRIPTION {
        r#type: kind,
        szMessage: text(message),
        szInsert: text(insert),
    };
    let bytes = std::slice::from_raw_parts(
        &description as *const DROPDESCRIPTION as *const u8,
        size_of::<DROPDESCRIPTION>(),
    );
    set(data, CFSTR_DROPDESCRIPTION, bytes);
}

// ---- Drag-out ---------------------------------------------------------------------------------------

/// Starts dragging `paths` out of the window registered as `label` ("main"…) into Explorer, the desktop, a
/// mail…, with the shell's own drag data and a picture. Call it from the file list when the pointer has
/// moved past the drag threshold with the button still down. `copy_only` offers only a copy (files extracted
/// from an archive). The modal drag loop runs from the UI thread's message pump (glint-core posts it through
/// `core_bridge::post_to_ui`), never inside the GPUI callback that called this; the returned future resolves
/// once it ends with "copy", "move", "link", "none" or "released" (the button was already up).
pub fn drag_out(
    cx: &App,
    label: &str,
    paths: Vec<String>,
    copy_only: bool,
) -> impl std::future::Future<Output = Result<String, String>> + 'static {
    use glint_core::host::Manager as _;
    let window = cx
        .try_global::<core_bridge::Core>()
        .and_then(|core| core.app.get_webview_window(label));
    async move {
        let window = window.ok_or_else(|| "this window isn't registered with core".to_string())?;
        core_bridge::run(glint_core::shell::start_drag(window, paths, Some(copy_only))).await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::{w, Interface, HSTRING};
    use windows::Win32::System::Com::IBindCtx;
    use windows::Win32::System::Ole::OleInitialize;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{BHID_DataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
    };

    const ALL: u32 = DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0;

    #[test]
    fn a_plain_drop_copies_and_shift_moves() {
        assert_eq!(effect_for(0, ALL, false), DropEffect::Copy);
        assert_eq!(effect_for(MK_SHIFT, ALL, false), DropEffect::Move);
        assert_eq!(effect_for(MK_CONTROL, ALL, false), DropEffect::Copy);
    }

    #[test]
    fn alt_or_ctrl_shift_makes_a_shortcut() {
        assert_eq!(effect_for(MK_ALT, ALL, false), DropEffect::Link);
        assert_eq!(effect_for(MK_CONTROL | MK_SHIFT, ALL, false), DropEffect::Link);
        // A source that offers no shortcut falls back to the other rules.
        let no_link = DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0;
        assert_eq!(effect_for(MK_ALT, no_link, false), DropEffect::Copy);
    }

    #[test]
    fn the_source_limits_what_the_keys_can_do() {
        // Out of an archive the source only copies: Shift can't make it a move.
        assert_eq!(effect_for(MK_SHIFT, DROPEFFECT_COPY.0, false), DropEffect::Copy);
        // A source that only moves.
        assert_eq!(effect_for(0, DROPEFFECT_MOVE.0, false), DropEffect::Move);
        assert_eq!(effect_for(0, 0, false), DropEffect::None);
        assert_eq!(effect_for(0, DROPEFFECT_LINK.0, false), DropEffect::Link);
    }

    #[test]
    fn the_recycle_bin_always_moves() {
        assert_eq!(effect_for(0, ALL, true), DropEffect::Move);
        assert_eq!(effect_for(MK_ALT, ALL, true), DropEffect::Move);
        // ...unless the source can't move.
        assert_eq!(effect_for(0, DROPEFFECT_COPY.0, true), DropEffect::Copy);
    }

    #[test]
    fn positions_are_logical_pixels_at_any_dpi() {
        assert_eq!(to_logical(120, 96), 120.0);
        assert_eq!(to_logical(150, 144), 100.0);
        assert_eq!(to_logical(48, 0), 48.0);
        assert_eq!(to_logical(-30, 192), -15.0);
    }

    #[test]
    fn answers_are_kept_until_replaced() {
        let answers = DropAnswers::default();
        assert_eq!(answers.get(), Answer::Unknown);
        let docs = Under {
            name: "Docs".into(),
            copy: "Copy to %1".into(),
            moves: "Move to %1".into(),
            always_move: false,
        };
        answers.set(Answer::Accept(docs.clone()));
        assert_eq!(answers.clone().get(), Answer::Accept(docs));
        answers.set(Answer::Refuse);
        assert_eq!(answers.get(), Answer::Refuse);
    }

    /// A real shell data object for files, as File Explorer hands one over when dragging.
    fn data_object(paths: &[String]) -> Option<IDataObject> {
        let mut pidls: Vec<*const ITEMIDLIST> = Vec::new();
        for path in paths {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            // SAFETY: parsing a path into an item ID list, which the shell allocates (leaked: it's a test).
            if unsafe { SHParseDisplayName(&HSTRING::from(path.as_str()), None, &mut pidl, 0, None) }.is_ok() {
                pidls.push(pidl);
            }
        }
        // SAFETY: the list holds valid item IDs for the call.
        unsafe {
            SHCreateShellItemArrayFromIDLists(&pidls)
                .and_then(|items| items.BindToHandler::<_, IDataObject>(None::<&IBindCtx>, &BHID_DataObject))
                .ok()
        }
    }

    /// A hidden window to measure the pointer from, and OLE on this thread.
    fn test_window() -> HWND {
        // SAFETY: OLE init on this test's thread; a message-only window of the system's own STATIC class.
        unsafe {
            let _ = OleInitialize(None);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("dragdrop test"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .unwrap_or_default()
        }
    }

    #[test]
    fn a_synthetic_drop_with_a_shell_data_object_reaches_the_ui_side() {
        let window = test_window();
        let file = std::env::temp_dir().join("glint-dragdrop-test.txt");
        assert!(std::fs::write(&file, "x").is_ok());
        let path = file.to_string_lossy().to_string();
        let Some(data) = data_object(std::slice::from_ref(&path)) else {
            panic!("no shell data object")
        };

        let (target, events, answers) = make_target(window, None);
        let pt = POINTL { x: 20, y: 30 };
        let mut effect = DROPEFFECT(ALL);
        // SAFETY: calling the target the way OLE does, with a live data object and valid out-pointer.
        unsafe {
            assert!(target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect).is_ok());
            assert_eq!(effect, DROPEFFECT_COPY);
            // The UI answers that the pointer is over the Recycle Bin: a move, whatever the keys.
            answers.set(Answer::Accept(Under {
                name: "Recycle Bin".into(),
                always_move: true,
                ..Under::default()
            }));
            let moved_to = POINTL { x: 40, y: 30 };
            let farther = POINTL { x: 60, y: 30 };
            // OLE hands the source's allowed effects in afresh on every call.
            effect = DROPEFFECT(ALL);
            assert!(target.DragOver(MODIFIERKEYS_FLAGS(0), moved_to, &mut effect).is_ok());
            assert_eq!(effect, DROPEFFECT_MOVE);
            // A region that takes nothing.
            answers.set(Answer::Refuse);
            effect = DROPEFFECT(ALL);
            assert!(target.DragOver(MODIFIERKEYS_FLAGS(0), farther, &mut effect).is_ok());
            assert_eq!(effect, DROPEFFECT_NONE);
            // Back to "not said": Shift moves, and the drop tells the source not to delete.
            answers.set(Answer::Unknown);
            effect = DROPEFFECT(ALL);
            assert!(target
                .Drop(&data, MODIFIERKEYS_FLAGS(MK_SHIFT), pt, &mut effect)
                .is_ok());
            assert_eq!(effect, DROPEFFECT_NONE);
        }

        let mut heard = Vec::new();
        while let Ok(event) = events.try_recv() {
            heard.push(event);
        }
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        let [DropEvent::Enter {
            paths: entered,
            effect: first,
            ..
        }, DropEvent::Over { effect: second, .. }, DropEvent::Over { effect: third, .. }, DropEvent::Drop {
            paths: dropped,
            effect: last,
            ..
        }] = heard.as_slice()
        else {
            panic!("unexpected events: {heard:?}");
        };
        assert!(entered.len() == 1 && same(&entered[0], &path), "{entered:?} vs {path}");
        assert!(dropped.len() == 1 && same(&dropped[0], &path));
        assert_eq!(
            (*first, *second, *third, *last),
            (DropEffect::Copy, DropEffect::Move, DropEffect::None, DropEffect::Move)
        );
        // The drag ended, so the answer is forgotten.
        assert_eq!(answers.get(), Answer::Unknown);
        let _ = std::fs::remove_file(&file);
        // SAFETY: destroying the window this test made.
        unsafe {
            let _ = DestroyWindow(window);
        }
        let _ = target.cast::<IDropTarget>();
    }

    #[test]
    fn leaving_tells_the_ui_and_non_file_drags_are_refused() {
        let window = test_window();
        let (target, events, _answers) = make_target(window, None);
        let mut effect = DROPEFFECT(ALL);
        let pt = POINTL { x: 0, y: 0 };
        let file = std::env::temp_dir().join("glint-dragdrop-test-leave.txt");
        assert!(std::fs::write(&file, "x").is_ok());
        let Some(data) = data_object(&[file.to_string_lossy().to_string()]) else {
            panic!("no shell data object")
        };
        // SAFETY: calling the target the way OLE does.
        unsafe {
            assert!(target.DragEnter(&data, MODIFIERKEYS_FLAGS(0), pt, &mut effect).is_ok());
            assert!(target.DragLeave().is_ok());
        }
        assert!(matches!(events.try_recv(), Ok(DropEvent::Enter { .. })));
        assert_eq!(events.try_recv(), Ok(DropEvent::Leave));
        assert!(events.try_recv().is_err());
        // SAFETY: no data object at all, which OLE may pass: refused, no event.
        unsafe {
            effect = DROPEFFECT(ALL);
            assert!(target
                .DragEnter(None::<&IDataObject>, MODIFIERKEYS_FLAGS(0), pt, &mut effect)
                .is_ok());
        }
        assert_eq!(effect, DROPEFFECT_NONE);
        assert!(events.try_recv().is_err());
        let _ = std::fs::remove_file(&file);
        // SAFETY: destroying the window this test made.
        unsafe {
            let _ = DestroyWindow(window);
        }
    }
}
