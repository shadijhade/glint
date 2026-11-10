//! What the menu actions ask Windows' shell for directly: open a path as `open` does, and show items in File
//! Explorer with them selected. (The TypeScript used Tauri's opener plugin: `openPath` and `revealItemInDir`.)
//!
//! Both run on a blocking thread (core's `blocking`), never on the UI thread: the shell may take a moment to
//! answer, and they set COM up for themselves.

use glint_model::paths::{parent_path, same_path};

/// The items grouped by the folder they are in, in the order the folders first appear: one File Explorer window
/// can select items of one folder only (`revealItemInDir` opened one for each).
pub(super) fn by_folder(paths: &[String]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for path in paths {
        let folder = parent_path(path);
        match groups.iter_mut().find(|(known, _)| same_path(known, &folder)) {
            Some((_, items)) => {
                if !items.iter().any(|item| same_path(item, path)) {
                    items.push(path.clone());
                }
            }
            None => groups.push((folder, vec![path.clone()])),
        }
    }
    groups
}

/// Opens `path` as the shell's `open` does: a folder in File Explorer, a program started, a file in its app.
#[cfg(windows)]
pub(super) fn shell_open(path: &str) -> Result<(), String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let file = HSTRING::from(path);
    let verb = HSTRING::from("open");
    // SAFETY: COM is set up for this blocking thread and torn down again if it was set up here; every string
    // handed to the shell outlives the call. ShellExecuteW's result is an HINSTANCE that is above 32 on success.
    let opened = unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).is_ok();
        let result = ShellExecuteW(None, &verb, &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        if initialized {
            CoUninitialize();
        }
        result.0 as isize
    };
    if opened > 32 {
        Ok(())
    } else {
        Err(format!("Windows couldn't open it (error {opened})"))
    }
}

#[cfg(not(windows))]
pub(super) fn shell_open(_: &str) -> Result<(), String> {
    Err("Opening is only available on Windows".to_string())
}

/// Shows the items in File Explorer, selected: one window for each folder they are in.
#[cfg(windows)]
pub(super) fn reveal(paths: &[String]) -> Result<(), String> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, IBindCtx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{ILFindLastID, ILFree, SHOpenFolderAndSelectItems, SHParseDisplayName};

    /// A path as an item ID list the shell allocated; freed with `ILFree`.
    struct Pidl(*mut ITEMIDLIST);

    impl Pidl {
        fn of(path: &str) -> Option<Pidl> {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            // SAFETY: parsing a path into an item ID list, which the shell allocates; it is freed on drop.
            unsafe { SHParseDisplayName(&HSTRING::from(path), None::<&IBindCtx>, &mut pidl, 0, None) }
                .ok()
                .filter(|()| !pidl.is_null())
                .map(|()| Pidl(pidl))
        }
    }

    impl Drop for Pidl {
        fn drop(&mut self) {
            // SAFETY: the list was allocated by the shell in `of` and is freed only here.
            unsafe { ILFree(Some(self.0.cast_const())) };
        }
    }

    if paths.is_empty() {
        return Ok(());
    }
    // SAFETY: COM is set up for this blocking thread and torn down again if it was set up here.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    let mut failure = None;
    for (folder, items) in by_folder(paths) {
        let shown = (|| -> Result<(), String> {
            let folder = Pidl::of(&folder).ok_or_else(|| format!("Windows doesn't know the folder “{folder}”"))?;
            let items: Vec<Pidl> = items.iter().filter_map(|item| Pidl::of(item)).collect();
            if items.is_empty() {
                // Nothing in it by those names any more: the folder alone.
                // SAFETY: the folder's list is valid for the call.
                return unsafe { SHOpenFolderAndSelectItems(folder.0, None, 0) }.map_err(|e| e.message());
            }
            // SAFETY: each item's last ID is a child of the folder, valid while its owner (`items`) lives.
            let children: Vec<*const ITEMIDLIST> =
                items.iter().map(|item| unsafe { ILFindLastID(item.0) as _ }).collect();
            // SAFETY: the folder's list and the children are valid for the call.
            unsafe { SHOpenFolderAndSelectItems(folder.0, Some(&children), 0) }.map_err(|e| e.message())
        })();
        if let Err(message) = shown {
            failure.get_or_insert(message);
        }
    }
    if initialized {
        // SAFETY: pairs the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    failure.map_or(Ok(()), Err)
}

#[cfg(not(windows))]
pub(super) fn reveal(_: &[String]) -> Result<(), String> {
    Err("Showing in File Explorer is only available on Windows".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_are_grouped_by_their_folder_in_the_order_the_folders_appear() {
        let groups = by_folder(&[
            "C:\\a\\one.txt".to_string(),
            "C:\\b\\تقرير.pdf".to_string(),
            "c:\\A\\two.txt".to_string(),
            "C:\\a\\one.txt".to_string(),
        ]);
        assert_eq!(
            groups,
            vec![
                (
                    "C:\\a".to_string(),
                    vec!["C:\\a\\one.txt".to_string(), "c:\\A\\two.txt".to_string()]
                ),
                ("C:\\b".to_string(), vec!["C:\\b\\تقرير.pdf".to_string()]),
            ]
        );
        assert!(by_folder(&[]).is_empty());
    }
}
