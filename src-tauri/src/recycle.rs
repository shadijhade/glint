//! The Recycle Bin, a place Glint opens like a folder (`shell:RecycleBinFolder`, File Explorer's name
//! for it): what's in it, every drive's, as the shell lists it, each item with the name it had, the
//! folder it was deleted from, when, and its size (a folder's, everything in it), from the `$I` record
//! Windows keeps next to its `$R` file. Emptying it asks first, as File Explorer does. Restoring and
//! deleting for good are file operations on each item's `$R` path (shell.rs).

use crate::fsx::{self, Entry};
use std::path::Path;
use std::time::Instant;

/// The Recycle Bin's path in Glint, as File Explorer's address bar takes it.
pub const PLACE: &str = "shell:RecycleBinFolder";

/// Glint's mark on an item in the Recycle Bin (Windows uses no attribute this high): it's there to be
/// restored or deleted for good, not opened or changed where it is.
const RECYCLED: u32 = 0x0800_0000;

pub fn is_place(path: &str) -> bool {
    path.eq_ignore_ascii_case(PLACE)
}

/// What a `$I` record says of its item: its size (a folder's is everything in it), when it was
/// deleted (ms since 1970) and the path it was deleted from. Version 1 (Windows Vista to 8.1) keeps
/// the path in 260 characters; version 2 (Windows 10 on) gives its length first.
fn read_info(bytes: &[u8]) -> Option<(u64, i64, String)> {
    let number = |at: usize| Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?));
    let (version, size, deleted) = (number(0)?, number(8)?, number(16)?);
    let units = match version {
        1 => bytes.get(24..24 + 520)?,
        2 => {
            let count = u32::from_le_bytes(bytes.get(24..28)?.try_into().ok()?) as usize;
            bytes.get(28..28 + count.checked_mul(2)?)?
        }
        _ => return None,
    };
    let wide: Vec<u16> = units.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).take_while(|&u| u != 0).collect();
    let path = String::from_utf16(&wide).ok()?;
    // A FILETIME: 100 ns since 1601.
    let ms = (deleted as i64).saturating_sub(116_444_736_000_000_000) / 10_000;
    (!path.is_empty()).then_some((size, ms.max(0), path))
}

/// The item a `$R` file (or folder) stands for: its entry, named as it was, with its `$R` path, the
/// date it was deleted as its date and Glint's mark among its attributes; and the folder it was
/// deleted from. None without a readable `$I` record, which File Explorer leaves out too.
fn item(recycled: &str) -> Option<(Entry, String)> {
    let path = Path::new(recycled);
    let info = path.with_file_name(format!("$I{}", path.file_name()?.to_str()?.strip_prefix("$R")?));
    let (size, deleted, original) = read_info(&std::fs::read(info).ok()?)?;
    let found = fsx::stat(recycled)?;
    let original = Path::new(&original);
    let entry = Entry {
        name: original.file_name()?.to_string_lossy().into_owned(),
        path: Some(recycled.to_string()),
        dir: found.dir,
        size: if found.dir { size } else { found.size },
        modified: deleted,
        attrs: found.attrs | RECYCLED,
    };
    Some((entry, original.parent()?.to_string_lossy().into_owned()))
}

/// The Recycle Bin as a listing: `fsx::encode_items` with each item's name and the folder it was
/// deleted from, sorted as a folder's listing is.
pub fn listing(started: Instant) -> Result<Vec<u8>, String> {
    let mut items: Vec<(Entry, String)> = imp::recycled()?.iter().filter_map(|path| item(path)).collect();
    items.sort_by_cached_key(|(e, _)| (!e.dir, fsx::natural_key(&e.name.to_lowercase()), e.name.clone()));
    let (entries, origins): (Vec<Entry>, Vec<String>) = items.into_iter().unzip();
    Ok(fsx::encode_items(PLACE, &entries, &origins, fsx::ms_since(started), false))
}

/// Empties the Recycle Bin, every drive's, as File Explorer does: Windows asks first, then shows its
/// progress. Whether it's empty now (false when the person said no).
#[tauri::command]
pub async fn recycle_empty(window: tauri::Window) -> Result<bool, String> {
    let owner = crate::shell::owner(&window);
    fsx::blocking(move || imp::empty(owner)).await
}

#[cfg(windows)]
mod imp {
    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::Foundation::{ERROR_CANCELLED, E_ABORT, HWND};
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, FOLDERID_RecycleBinFolder, IEnumShellItems, IShellItem, SHEmptyRecycleBinW, SHGetKnownFolderItem, SHQueryRecycleBinW, KF_FLAG_DEFAULT,
        SHQUERYRBINFO, SIGDN_FILESYSPATH,
    };

    /// "Cancelled" from the copy engine, which empties the bin.
    const COPYENGINE_E_USER_CANCELLED: HRESULT = HRESULT(0x8027_0000_u32 as i32);

    /// The `$R` path of each item in the Recycle Bin, as the shell lists them: on every drive, and where
    /// a folder like Documents is kept elsewhere.
    pub fn recycled() -> Result<Vec<String>, String> {
        crate::shell::in_sta(|| unsafe {
            let bin: IShellItem = SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None).map_err(|e| e.message())?;
            let items: IEnumShellItems = bin.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems).map_err(|e| e.message())?;
            let mut paths = Vec::new();
            loop {
                let mut batch: [Option<IShellItem>; 32] = Default::default();
                let mut fetched = 0u32;
                items.Next(&mut batch, Some(&mut fetched)).map_err(|e| e.message())?;
                if fetched == 0 {
                    break;
                }
                for item in batch.iter().take(fetched as usize).flatten() {
                    if let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                        paths.extend(name.to_string().ok());
                        CoTaskMemFree(Some(name.0 as *const _));
                    }
                }
            }
            Ok(paths)
        })
    }

    /// How many items the Recycle Bin holds, every drive's.
    unsafe fn count() -> i64 {
        let mut info = SHQUERYRBINFO { cbSize: size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
        if SHQueryRecycleBinW(PCWSTR::null(), &mut info).is_ok() {
            info.i64NumItems
        } else {
            -1
        }
    }

    pub fn empty(owner: isize) -> Result<bool, String> {
        crate::shell::in_sta(move || unsafe {
            let emptied = SHEmptyRecycleBinW(Some(HWND(owner as *mut _)), PCWSTR::null(), 0);
            // An empty bin answers with an error of its own: what's left says how it went.
            let left = count();
            match emptied {
                _ if left == 0 => Ok(true),
                Ok(()) => Ok(false),
                Err(e) if [ERROR_CANCELLED.to_hresult(), E_ABORT, COPYENGINE_E_USER_CANCELLED].contains(&e.code()) => Ok(false),
                Err(e) => Err(e.message()),
            }
        })
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn recycled() -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    pub fn empty(_: isize) -> Result<bool, String> {
        Err("This is only available on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `$I` record as Windows writes it: version 1 with 260 characters for the path, version 2 with
    /// its length (NUL included) first.
    fn info(version: u64, size: u64, filetime: u64, path: &str) -> Vec<u8> {
        let mut bytes = [version, size, filetime].iter().flat_map(|n| n.to_le_bytes()).collect::<Vec<u8>>();
        let mut wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        if version == 1 {
            wide.resize(260, 0);
        } else {
            bytes.extend((wide.len() as u32).to_le_bytes());
        }
        bytes.extend(wide.iter().flat_map(|u| u.to_le_bytes()));
        bytes
    }

    #[test]
    fn reads_both_kinds_of_record() {
        // 2024-06-01 12:00 UTC as a FILETIME.
        let noon = 133_617_168_000_000_000;
        let ms = 1_717_243_200_000;
        assert_eq!(read_info(&info(2, 22, noon, r"C:\Users\me\Desktop\notes 😀.txt")), Some((22, ms, r"C:\Users\me\Desktop\notes 😀.txt".into())));
        let v1 = info(1, 1 << 33, noon, r"D:\Old\folder");
        assert_eq!(v1.len(), 544);
        assert_eq!(read_info(&v1), Some((1 << 33, ms, r"D:\Old\folder".into())));
        // Damaged: cut short, an unknown version, a length past the end, no path.
        let v2 = info(2, 1, noon, r"C:\a.txt");
        assert_eq!(read_info(&v2[..v2.len() - 4]), None);
        assert_eq!(read_info(&info(3, 1, noon, r"C:\a.txt")), None);
        let mut long = v2.clone();
        long[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_info(&long), None);
        assert_eq!(read_info(&info(2, 1, noon, "")), None);
        assert_eq!(read_info(b""), None);
    }

    #[test]
    fn names_an_item_as_it_was() {
        let dir = std::env::temp_dir().join(format!("glint-recycle-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("$RFOLDER")).unwrap();
        std::fs::write(dir.join("$RAB12CD.txt"), "hello").unwrap();
        std::fs::write(dir.join("$IAB12CD.txt"), info(2, 5, 133_617_168_000_000_000, r"C:\Users\me\Desktop\notes.txt")).unwrap();
        std::fs::write(dir.join("$IFOLDER"), info(2, 4096, 133_617_168_000_000_000, r"C:\Projects\old")).unwrap();
        std::fs::write(dir.join("$RLONELY.txt"), "no record").unwrap();
        let at = |name: &str| dir.join(name).to_string_lossy().into_owned();

        let (file, from) = item(&at("$RAB12CD.txt")).unwrap();
        assert_eq!((file.name.as_str(), file.path.as_deref(), file.dir, file.size, file.modified), ("notes.txt", Some(at("$RAB12CD.txt").as_str()), false, 5, 1_717_243_200_000));
        assert!(file.attrs & RECYCLED != 0);
        assert_eq!(from, r"C:\Users\me\Desktop");
        // A folder's size is its record's: everything in it.
        let (folder, from) = item(&at("$RFOLDER")).unwrap();
        assert_eq!((folder.name.as_str(), folder.dir, folder.size, from.as_str()), ("old", true, 4096, r"C:\Projects"));
        // Without its record, or not a `$R` name: not an item.
        assert!(item(&at("$RLONELY.txt")).is_none());
        assert!(item(&at("$IAB12CD.txt")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn its_place_is_file_explorers() {
        assert!(is_place("shell:RecycleBinFolder") && is_place("SHELL:recyclebinfolder"));
        assert!(!is_place(r"C:\$Recycle.Bin") && !is_place("shell:Desktop"));
    }
}
