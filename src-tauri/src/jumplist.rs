//! The taskbar button's Jump List (`ICustomDestinationList`): tasks (This PC, Quick search, the
//! Recycle Bin) and Folders, the pinned and most-used ones; its progress bar (`ITaskbarList3`) while
//! long work runs (an archive read, files extracted or zipped); and Back, Forward and Up under its
//! preview (`ThumbBarAddButtons`), drawn from Windows' own icon font in the taskbar's colors.

use crate::fsx::blocking;
use tauri::WebviewWindow;

/// The most-used folders stay out when Windows is set not to show recently opened items (Settings →
/// Personalization → Start), as they do from File Explorer's own lists.
#[tauri::command]
pub async fn sync_jump_list(pinned: Vec<String>, frequent: Vec<String>) -> Result<(), String> {
    blocking(move || imp::sync_jump_list(pinned, frequent)).await
}

/// Best effort: the taskbar never fails the work it shows.
#[tauri::command]
pub fn taskbar_progress(window: WebviewWindow, state: String, completed: Option<u64>, total: Option<u64>) {
    imp::taskbar_progress(&window, &state, completed, total);
}

/// Back, Forward and Up under the taskbar button's preview, for the folder shown: each greyed out where
/// it can't go. A click comes to the window as a "thumb-button" event ("back", "forward" or "up").
#[tauri::command]
pub fn thumb_buttons(window: WebviewWindow, back: bool, forward: bool, up: bool) {
    imp::thumb_buttons(&window, [back, forward, up]);
}

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use tauri::WebviewWindow;
    use windows::core::{w, Interface, GUID, HSTRING, PCWSTR};
    use tauri::Emitter;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, PROPERTYKEY, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, RegisterWindowMessageW, HICON, WM_COMMAND, WM_SETTINGCHANGE};
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Registry::REG_DWORD;
    use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::Win32::UI::Shell::{
        DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ITaskbarList3, ShellLink, TaskbarList, TBPF_ERROR,
        TBPF_INDETERMINATE, TBPF_NOPROGRESS, TBPF_NORMAL, TBPF_PAUSED,
    };

    const PKEY_TITLE: PROPERTYKEY = PROPERTYKEY {
        fmtid: GUID::from_u128(0xf29f85e0_4ff9_1068_ab91_08002b27b3d9),
        pid: 2,
    };
    /// The most folders the list shows (Windows may allow fewer: its own setting).
    const FOLDERS: usize = 10;

    fn make_link(exe: &Path, args: &str, title: &str, description: &str, icon_path: &str, icon_index: i32) -> Option<IShellLinkW> {
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let exe_w = HSTRING::from(exe.as_os_str());
            let args_w = HSTRING::from(args);
            let desc_w = HSTRING::from(description);
            let icon_w = HSTRING::from(icon_path);
            link.SetPath(PCWSTR(exe_w.as_ptr())).ok()?;
            link.SetArguments(PCWSTR(args_w.as_ptr())).ok()?;
            link.SetDescription(PCWSTR(desc_w.as_ptr())).ok()?;
            link.SetIconLocation(PCWSTR(icon_w.as_ptr()), icon_index).ok()?;
            let store: IPropertyStore = link.cast().ok()?;
            store.SetValue(&PKEY_TITLE, &PROPVARIANT::from(title)).ok()?;
            store.Commit().ok()?;
            Some(link)
        }
    }

    /// A path as one command-line argument: in quotes, with a trailing backslash doubled, as `"C:\"`
    /// would read as `C:"`.
    fn quoted(path: &str) -> String {
        let backslashes = path.len() - path.trim_end_matches('\\').len();
        format!("\"{path}{}\"", "\\".repeat(backslashes))
    }

    /// A file in System32, wherever Windows is.
    fn system32(file: &str) -> String {
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{windows}\System32\{file}")
    }

    /// Windows' "Show recently opened items in Start, Jump Lists and File Explorer".
    fn shows_recent() -> bool {
        let value = crate::verbs::reg::get(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced", "Start_TrackDocs");
        !value.is_some_and(|v| v.kind == REG_DWORD && v.data.get(..4) == Some(&[0, 0, 0, 0][..]))
    }

    /// The arguments of the links the user removed from the list: one added again makes Windows
    /// refuse the whole category.
    unsafe fn removed(items: &IObjectArray) -> Vec<String> {
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

    pub fn sync_jump_list(pinned: Vec<String>, frequent: Vec<String>) -> Result<(), String> {
        // What its entries start (a Microsoft Store copy's alias), and the exe whose icon they show.
        let exe = crate::verbs::launch_exe().ok_or("Couldn't tell where Glint is")?;
        let icon = std::env::current_exe().map_err(|e| e.to_string())?;
        let frequent = if shows_recent() { frequent } else { Vec::new() };
        crate::shell::in_sta(move || unsafe {
            let exe_str = icon.to_string_lossy().into_owned();
            let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
            let mut slots = 0u32;
            let removed = removed(&list.BeginList::<IObjectArray>(&mut slots).map_err(|e| e.message())?);

            if let Ok(tasks) = CoCreateInstance::<_, IObjectCollection>(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) {
                let links = [
                    make_link(&exe, r#""this-pc:""#, "This PC", "Open This PC in Glint", &exe_str, 0),
                    make_link(&exe, "--quick-search", "Quick search", "Search every drive", &exe_str, 0),
                    make_link(&exe, r#""shell:RecycleBinFolder""#, "Recycle Bin", "Open the Recycle Bin in Glint", &system32("imageres.dll"), -55),
                ];
                for link in links.iter().flatten() {
                    let _ = tasks.AddObject(link);
                }
                if let Ok(array) = tasks.cast::<IObjectArray>() {
                    let _ = list.AddUserTasks(&array);
                }
            }

            if let Ok(places) = CoCreateInstance::<_, IObjectCollection>(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) {
                let (most, mut added) = ((slots as usize).clamp(4, FOLDERS), Vec::new());
                for folder in pinned.iter().chain(&frequent) {
                    let arg = quoted(folder);
                    if added.len() == most || added.contains(&arg) || removed.contains(&arg) || !Path::new(folder).is_dir() {
                        continue;
                    }
                    let name = Path::new(folder).file_name().map_or_else(|| folder.clone(), |n| n.to_string_lossy().into_owned());
                    if let Some(link) = make_link(&exe, &arg, &name, folder, &system32("shell32.dll"), 3) {
                        let _ = places.AddObject(&link);
                        added.push(arg);
                    }
                }
                if !added.is_empty() {
                    if let Ok(array) = places.cast::<IObjectArray>() {
                        let _ = list.AppendCategory(w!("Folders"), &array);
                    }
                }
            }
            list.CommitList().map_err(|e| e.message())
        })
    }

    pub fn taskbar_progress(window: &WebviewWindow, state: &str, completed: Option<u64>, total: Option<u64>) {
        let Ok(hwnd) = window.hwnd() else { return };
        let hwnd_val = hwnd.0 as isize;
        let state = state.to_string();
        let _ = window.run_on_main_thread(move || unsafe {
            let Ok(bar) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) else {
                return;
            };
            let _ = bar.HrInit();
            let h = HWND(hwnd_val as _);
            let flag = match state.as_str() {
                "indeterminate" => TBPF_INDETERMINATE,
                "normal" => TBPF_NORMAL,
                "paused" => TBPF_PAUSED,
                "error" => TBPF_ERROR,
                _ => TBPF_NOPROGRESS,
            };
            let _ = bar.SetProgressState(h, flag);
            if let (Some(c), Some(t)) = (completed, total) {
                if t > 0 && flag != TBPF_NOPROGRESS && flag != TBPF_INDETERMINATE {
                    let _ = bar.SetProgressValue(h, c.min(t), t);
                }
            }
        });
    }

    /// The preview's buttons: what each is called in the window's event, its glyph in Segoe Fluent
    /// Icons (and Segoe MDL2 Assets, Windows 10's), and its tooltip.
    const BUTTONS: [(&str, u16, &str); 3] = [("back", 0xE72B, "Back"), ("forward", 0xE72A, "Forward"), ("up", 0xE74A, "Up")];
    /// Their ids in the window's WM_COMMAND, and this module's subclass of the window.
    const FIRST_ID: u32 = 0x4C47;
    const SUBCLASS: usize = 0x676C_7462;
    /// The notification a thumbnail button's click comes with (THBN_CLICKED).
    const CLICKED: usize = 0x1800;

    /// The buttons' state, on the window's own thread.
    #[derive(Default)]
    struct Thumbs {
        window: Option<WebviewWindow>,
        enabled: [bool; 3],
        /// They're on the taskbar button there is now (Explorer restarting, or the window coming back
        /// from the tray, makes a new one).
        added: bool,
        /// Icons drawn for a taskbar this light or dark, at this size.
        icons: Vec<isize>,
        drawn: Option<(bool, i32)>,
    }

    thread_local! {
        static THUMBS: std::cell::RefCell<Thumbs> = std::cell::RefCell::new(Thumbs::default());
    }

    static BUTTON_CREATED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

    pub fn thumb_buttons(window: &WebviewWindow, enabled: [bool; 3]) {
        let Ok(hwnd) = window.hwnd() else { return };
        let hwnd = hwnd.0 as isize;
        let owner = window.clone();
        let _ = window.run_on_main_thread(move || unsafe {
            use windows::Win32::UI::Shell::SetWindowSubclass;
            let first = THUMBS.with_borrow_mut(|t| {
                t.enabled = enabled;
                t.window.replace(owner).is_none()
            });
            if first {
                let _ = BUTTON_CREATED.set(RegisterWindowMessageW(w!("TaskbarButtonCreated")));
                let _ = SetWindowSubclass(HWND(hwnd as _), Some(thumb_messages), SUBCLASS, 0);
            }
            show_thumbs(HWND(hwnd as _));
        });
    }

    /// Puts the buttons on the taskbar button, or brings them up to date there.
    unsafe fn show_thumbs(hwnd: HWND) {
        use windows::Win32::UI::Shell::{THBF_DISABLED, THBF_ENABLED, THB_FLAGS, THB_ICON, THB_TOOLTIP, THUMBBUTTON};
        let Ok(bar) = CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) else { return };
        if bar.HrInit().is_err() {
            return;
        }
        THUMBS.with_borrow_mut(|t| {
            let light = taskbar_is_light();
            let size = t.window.as_ref().and_then(|w| w.scale_factor().ok()).map_or(16, |scale| (16.0 * scale).round() as i32);
            if t.drawn != Some((light, size)) {
                let color = if light { 0x1F1F1F } else { 0xFFFFFF };
                let icons: Vec<isize> = BUTTONS.iter().map(|&(_, glyph, _)| glyph_icon(glyph, size, color).map_or(0, |i| i.0 as isize)).collect();
                for old in std::mem::replace(&mut t.icons, icons) {
                    if old != 0 {
                        let _ = DestroyIcon(HICON(old as _));
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
                        hIcon: HICON(t.icons[i] as _),
                        dwFlags: if t.enabled[i] { THBF_ENABLED } else { THBF_DISABLED },
                        ..Default::default()
                    };
                    for (to, from) in button.szTip.iter_mut().zip(tip.encode_utf16()) {
                        *to = from;
                    }
                    button
                })
                .collect();
            if t.added {
                let _ = bar.ThumbBarUpdateButtons(hwnd, &buttons);
            } else {
                // Before the taskbar button is made this fails: TaskbarButtonCreated brings them then.
                t.added = bar.ThumbBarAddButtons(hwnd, &buttons).is_ok();
            }
        });
    }

    /// The window's messages: a button's click, a new taskbar button, and a change of colors.
    unsafe extern "system" fn thumb_messages(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _: usize, _: usize) -> LRESULT {
        use windows::Win32::UI::Shell::DefSubclassProc;
        if msg == WM_COMMAND && (wparam.0 >> 16) & 0xFFFF == CLICKED {
            let id = (wparam.0 & 0xFFFF) as u32;
            if let Some(&(name, _, _)) = id.checked_sub(FIRST_ID).and_then(|i| BUTTONS.get(i as usize)) {
                THUMBS.with_borrow(|t| {
                    if let Some(window) = &t.window {
                        let _ = window.emit_to(window.label(), "thumb-button", name);
                    }
                });
                return LRESULT(0);
            }
        } else if BUTTON_CREATED.get() == Some(&msg) {
            THUMBS.with_borrow_mut(|t| t.added = false);
            show_thumbs(hwnd);
        } else if msg == WM_SETTINGCHANGE && lparam.0 != 0 && PCWSTR(lparam.0 as *const u16).to_string().is_ok_and(|s| s == "ImmersiveColorSet") {
            // Light or dark: drawn again in the taskbar's colors.
            if THUMBS.with_borrow(|t| t.added) {
                show_thumbs(hwnd);
            }
        }
        DefSubclassProc(hwnd, msg, wparam, lparam)
    }

    /// Windows' own parts (the taskbar, its previews) are light: Settings → Personalization → Colors.
    fn taskbar_is_light() -> bool {
        let value = crate::verbs::reg::get(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "SystemUsesLightTheme");
        value.is_some_and(|v| v.kind == REG_DWORD && v.data.get(..4).is_some_and(|d| d != [0, 0, 0, 0]))
    }

    /// A glyph of Windows' icon font, `size` pixels square in `color` (0xRRGGBB), as top-down BGRA
    /// pixels: drawn white on black without ClearType, so each pixel's brightness is how much of it the
    /// glyph covers, which becomes its alpha.
    unsafe fn glyph_pixels(glyph: u16, size: i32, color: u32) -> Option<Vec<u8>> {
        use windows::Win32::Foundation::{COLORREF, RECT};
        use windows::Win32::Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW, GdiFlush, GetTextFaceW, SelectObject, SetBkMode, SetTextColor, ANTIALIASED_QUALITY,
            CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_NORMAL, HGDIOBJ, OUT_DEFAULT_PRECIS, TRANSPARENT,
        };
        let dc = CreateCompatibleDC(None);
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(Some(dc), &top_down(size), DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old_bitmap = SelectObject(dc, HGDIOBJ(bitmap.0));
        let mut drawn = false;
        for face in [w!("Segoe Fluent Icons"), w!("Segoe MDL2 Assets")] {
            let font = CreateFontW(-(size * 4 / 5), 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, ANTIALIASED_QUALITY, DEFAULT_PITCH.0 as u32, face);
            let old_font = SelectObject(dc, HGDIOBJ(font.0));
            let mut chosen = [0u16; 64];
            let len = (GetTextFaceW(dc, Some(&mut chosen)).max(1) as usize - 1).min(chosen.len());
            // GDI picks another font for one that isn't there: only the icon font itself will do.
            if face.as_wide() == &chosen[..len] {
                SetTextColor(dc, COLORREF(0xFFFFFF));
                SetBkMode(dc, TRANSPARENT);
                let mut rect = RECT { left: 0, top: 0, right: size, bottom: size };
                DrawTextW(dc, &mut [glyph], &mut rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
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
            let drawn = std::slice::from_raw_parts(bits as *const u8, (size * size * 4) as usize);
            drawn.as_chunks::<4>().0.iter().flat_map(|p| [b, g, r, p[0].max(p[1]).max(p[2])]).collect()
        });
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(dc);
        pixels
    }

    /// A 32-bit top-down DIB's header, `size` pixels square.
    fn top_down(size: i32) -> windows::Win32::Graphics::Gdi::BITMAPINFO {
        use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, BI_RGB};
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
    unsafe fn glyph_icon(glyph: u16, size: i32, color: u32) -> Option<HICON> {
        use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, DIB_RGB_COLORS, HGDIOBJ};
        use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};
        let pixels = glyph_pixels(glyph, size, color)?;
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &top_down(size), DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
        let mask = CreateBitmap(size, size, 1, 1, None);
        let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() }).ok();
        let _ = DeleteObject(HGDIOBJ(mask.0));
        let _ = DeleteObject(HGDIOBJ(color.0));
        icon
    }

    #[cfg(test)]
    mod tests {
        /// Draws the three buttons' icons, as a dark and a light taskbar shows them, into PNGs in the temp
        /// folder (thumb-buttons-*.png), to look at.
        #[test]
        #[ignore = "draws on this PC's fonts"]
        fn draws_the_preview_buttons() {
            for (color, theme, under) in [(0xFFFFFFu32, "dark", [32u8, 32, 32]), (0x1F1F1F, "light", [243, 243, 243])] {
                let size = 40;
                let mut rgb = Vec::new();
                let all: Vec<Vec<u8>> = super::BUTTONS.iter().map(|&(_, glyph, _)| unsafe { super::glyph_pixels(glyph, size, color) }.expect("drawn")).collect();
                for y in 0..size as usize {
                    for icon in &all {
                        for x in 0..size as usize {
                            let p = &icon[(y * size as usize + x) * 4..][..4];
                            let a = p[3] as u32;
                            for (c, u) in [p[2], p[1], p[0]].into_iter().zip(under) {
                                rgb.push(((c as u32 * a + u as u32 * (255 - a)) / 255) as u8);
                            }
                        }
                    }
                }
                assert!(all.iter().all(|icon| icon.chunks(4).any(|p| p[3] > 200)), "each glyph shows");
                let path = std::env::temp_dir().join(format!("thumb-buttons-{theme}.png"));
                let mut encoder = png::Encoder::new(std::fs::File::create(&path).unwrap(), size as u32 * 3, size as u32);
                encoder.set_color(png::ColorType::Rgb);
                encoder.write_header().unwrap().write_image_data(&rgb).unwrap();
                assert!(unsafe { super::glyph_icon(super::BUTTONS[0].1, 20, color) }.is_some());
                println!("{}", path.display());
            }
        }

        #[test]
        fn quotes_paths_for_the_command_line() {
            assert_eq!(super::quoted(r"C:\"), r#""C:\\""#);
            assert_eq!(super::quoted(r"D:\Projects\Glint"), r#""D:\Projects\Glint""#);
            assert_eq!(super::quoted(r"\\server\share\"), r#""\\server\share\\""#);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::WebviewWindow;

    pub fn sync_jump_list(_pinned: Vec<String>, _frequent: Vec<String>) -> Result<(), String> {
        Ok(())
    }

    pub fn taskbar_progress(_window: &WebviewWindow, _state: &str, _completed: Option<u64>, _total: Option<u64>) {}

    pub fn thumb_buttons(_window: &WebviewWindow, _enabled: [bool; 3]) {}
}
