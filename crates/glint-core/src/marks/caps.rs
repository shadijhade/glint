//! What a volume can do, asked of the volume rather than guessed from an error's text (G7): whether it keeps
//! named streams (`GetVolumeInformationByHandleW`'s `FILE_NAMED_STREAMS`).

/// Whether the volume `path` is on keeps named streams; false when it can't be asked.
#[cfg(windows)]
pub fn has_streams(path: &str) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::GetVolumeInformationByHandleW;
    const FILE_NAMED_STREAMS: u32 = 0x0004_0000;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    let path = std::path::Path::new(path);
    if !path.is_absolute() {
        return false;
    }
    // The item itself, or its folder when it isn't there (being made, or just moved away), opened for its
    // attributes alone: a handle is on the volume the path really leads to, through a mounted folder or a link
    // too, and asking it costs one open where walking the path for its volume's root costs one a folder.
    let open = |p: &std::path::Path| std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(p).ok();
    let Some(file) = path.ancestors().take(2).find_map(open) else { return false };
    let mut flags = 0u32;
    // SAFETY: the handle is open for the call and the flags are a live local.
    let asked = unsafe { GetVolumeInformationByHandleW(HANDLE(file.as_raw_handle()), None, None, None, Some(&mut flags), None) };
    asked.is_ok() && flags & FILE_NAMED_STREAMS != 0
}

#[cfg(not(windows))]
pub fn has_streams(_path: &str) -> bool {
    false
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn the_temp_folder_is_what_trying_says_and_odd_paths_are_false() {
        let dir = std::env::temp_dir().join(format!("glint-caps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let probe = dir.join("probe.txt");
        std::fs::write(&probe, b"x").unwrap();
        let tried = std::fs::write(format!("{}:glint.probe", probe.display()), b"x").is_ok();
        let path = dir.to_string_lossy().into_owned();
        assert_eq!(has_streams(&path), tried);
        assert_eq!(has_streams(&path), tried, "asked twice");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!has_streams(""));
        assert!(!has_streams("notes\\a.txt"));
        if let Some(letter) = ('D'..='Z').rev().find(|l| !std::path::Path::new(&format!("{l}:\\")).exists()) {
            assert!(!has_streams(&format!("{letter}:\\")));
        }
    }
}
