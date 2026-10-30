//! The taskbar button's Jump List (`ICustomDestinationList`): tasks (This PC, Quick search, the
//! Recycle Bin) and Folders, the pinned and most-used ones; and its progress bar (`ITaskbarList3`)
//! while long work runs (an archive read, files extracted or zipped).

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

#[cfg(windows)]
mod imp {
    use std::path::Path;
    use tauri::WebviewWindow;
    use windows::core::{w, Interface, GUID, HSTRING, PCWSTR};
    use windows::Win32::Foundation::{HWND, PROPERTYKEY};
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
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let frequent = if shows_recent() { frequent } else { Vec::new() };
        crate::shell::in_sta(move || unsafe {
            let exe_str = exe.to_string_lossy().into_owned();
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

    #[cfg(test)]
    mod tests {
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
}
