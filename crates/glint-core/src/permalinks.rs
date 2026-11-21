//! Unbreakable `glint://` Permalinks backed by NTFS/ReFS File Reference Numbers & Object IDs.
//!
//! Creates stable URIs of the form:
//! `glint://open?vol=<hex8>&id=<hex16>&hint=<percent-encoded-path>[&oid=<hex32>]`
//!
//! Resolution uses `OpenFileById` on an unprivileged volume-root handle first, then falls back to
//! `shadui-index` (`Request::ResolveId`), then the encoded hint path. Links survive renames and
//! moves across any folder on the same NTFS/ReFS volume.
//!
//! A link is data from anywhere (any web page can open one), so only this PC's own drives are ever
//! looked at: merely checking whether a `\\server\share` path exists makes Windows sign in to that
//! server with the user's account. Links are made for files on those drives only.

use std::fmt::Write as _;

/// Parsed components of a `glint://open?...` permalink URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permalink {
    pub volume_serial: u32,
    pub frn: u64,
    pub hint: String,
    pub object_id: Option<[u8; 16]>,
}

/// Encodes a [`Permalink`] as a canonical `glint://open?...` URI.
pub fn format_uri(link: &Permalink) -> String {
    let mut uri = format!(
        "glint://open?vol={:08x}&id={:016x}&hint={}",
        link.volume_serial,
        link.frn,
        percent_encode(&link.hint)
    );
    if let Some(oid) = link.object_id {
        uri.push_str("&oid=");
        for byte in oid {
            let _ = write!(&mut uri, "{byte:02x}");
        }
    }
    uri
}

/// Parses a `glint://open?...` URI cleanly (case-insensitive scheme and host).
pub fn parse_uri(uri: &str) -> Option<Permalink> {
    let trimmed = uri.trim();
    let rest = if trimmed.len() >= 8 && trimmed[..8].eq_ignore_ascii_case("glint://") {
        &trimmed[8..]
    } else {
        return None;
    };
    let (_, query) = rest.split_once('?')?;
    let mut volume_serial = None;
    let mut frn = None;
    let mut hint = String::new();
    let mut object_id = None;

    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "vol" => volume_serial = u32::from_str_radix(v, 16).ok(),
            "id" => frn = u64::from_str_radix(v, 16).ok(),
            "hint" => hint = percent_decode(v),
            "oid" if v.len() == 32 => {
                let mut bytes = [0u8; 16];
                if bytes
                    .iter_mut()
                    .enumerate()
                    .all(|(i, slot)| match u8::from_str_radix(&v[i * 2..i * 2 + 2], 16) {
                        Ok(b) => {
                            *slot = b;
                            true
                        }
                        Err(_) => false,
                    })
                {
                    object_id = Some(bytes);
                }
            }
            _ => {}
        }
    }

    Some(Permalink {
        volume_serial: volume_serial?,
        frn: frn?,
        hint,
        object_id,
    })
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    for byte in input.as_bytes() {
        let safe = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if safe {
            out.push(*byte as char);
        } else {
            let _ = write!(&mut out, "%{byte:02X}");
        }
    }
    out
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(v) = u8::from_str_radix(hex, 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(if b == b'+' { b' ' } else { b });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Creates an unbreakable `glint://` permalink URI for `path`.
pub fn create_permalink(path: &str) -> Result<String, String> {
    #[cfg(windows)]
    {
        win::create_permalink(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Permalinks require Windows NTFS/ReFS file IDs".to_string())
    }
}

/// Resolves a `glint://` permalink URI back to the live file or folder path on disk.
pub fn resolve_permalink(uri: &str) -> Result<String, String> {
    let parsed = parse_uri(uri).ok_or_else(|| "Invalid glint:// permalink".to_string())?;
    #[cfg(windows)]
    {
        win::resolve_permalink(&parsed)
    }
    #[cfg(not(windows))]
    {
        if !parsed.hint.is_empty() && std::path::Path::new(&parsed.hint).exists() {
            Ok(parsed.hint)
        } else {
            Err("Couldn't resolve permalink".to_string())
        }
    }
}

/// Makes `glint://` links open this copy for as long as it's installed, as Glint starts. The copy the
/// setup program or Scoop installed points the handler under `HKCU\Software\Classes\glint` at itself,
/// writing only when it isn't already there; their uninstallers take it out again (installer/hooks.nsh,
/// scripts/package-manifests.mjs). A portable copy or a dev build (an e2e run's too) never takes the
/// links: it only removes a handler whose program is gone. The Microsoft Store's copy has them from its
/// manifest (crates/glint/msix/AppxManifest.xml), as Windows keeps a packaged copy's registry writes to
/// itself.
pub fn claim_protocol() {
    #[cfg(windows)]
    {
        if !crate::packaged() {
            let _ = win::claim_protocol(r"Software\Classes", crate::verbs::installed_exe().as_deref());
        }
    }
}

/// `claim_protocol` under a stand-in for Software\Classes, for the uninstallers' tests (verbs.rs).
#[cfg(all(windows, test))]
pub(crate) use win::claim_protocol as claim_protocol_in;

#[cfg(windows)]
mod win {
    use super::{format_uri, Permalink};
    use std::path::Path;
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FileIdType, GetDriveTypeW, GetFileInformationByHandle, GetFinalPathNameByHandleW,
        GetLogicalDrives, GetVolumeInformationW, ObjectIdType, OpenFileById, BY_HANDLE_FILE_INFORMATION,
        FILE_FLAGS_AND_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, VOLUME_NAME_DOS,
    };
    use windows::Win32::System::Ioctl::FSCTL_GET_OBJECT_ID;
    use windows::Win32::System::IO::DeviceIoControl;

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: closes owned Win32 handle once.
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn open_metadata_handle(path: &str) -> Result<OwnedHandle, String> {
        let w = wide(path);
        // SAFETY: Opens existing file or directory with metadata-only access (`0`) and backup semantics.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(w.as_ptr()),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|e| format!("Couldn't open {path}: {}", e.message()))?;
        Ok(OwnedHandle(handle))
    }

    fn strip_win32_prefix(path: &str) -> String {
        if let Some(unc) = path.strip_prefix("\\\\?\\UNC\\") {
            format!("\\\\{unc}")
        } else if let Some(rest) = path.strip_prefix("\\\\?\\") {
            rest.to_string()
        } else {
            path.to_string()
        }
    }

    fn final_path(handle: HANDLE) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        // SAFETY: queries final DOS path name into initialized UTF-16 buffer.
        let len = unsafe { GetFinalPathNameByHandleW(handle, &mut buf, VOLUME_NAME_DOS) };
        if len == 0 {
            return None;
        }
        let len = if len as usize > buf.len() {
            buf.resize(len as usize + 1, 0);
            unsafe { GetFinalPathNameByHandleW(handle, &mut buf, VOLUME_NAME_DOS) }
        } else {
            len
        };
        if len == 0 || len as usize > buf.len() {
            return None;
        }
        Some(strip_win32_prefix(&String::from_utf16_lossy(&buf[..len as usize])))
    }

    /// Whether `path` is on one of this PC's own drives (`C:\…` on a fixed, removable or RAM drive), not
    /// on a network drive or a `\\server\share` (see the module's notes). Only the drive's type is asked.
    pub(super) fn on_local_drive(path: &str) -> bool {
        const DRIVE_REMOVABLE: u32 = 2;
        const DRIVE_FIXED: u32 = 3;
        const DRIVE_RAMDISK: u32 = 6;
        let b = path.as_bytes();
        if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || !matches!(b[2], b'\\' | b'/') {
            return false;
        }
        let root = wide(&format!("{}:\\", b[0] as char));
        // SAFETY: NUL-terminated root path.
        let kind = unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) };
        matches!(kind, DRIVE_REMOVABLE | DRIVE_FIXED | DRIVE_RAMDISK)
    }

    /// A path a link names that may be opened: an existing one on this PC's own drives.
    pub(super) fn usable(path: &str) -> bool {
        on_local_drive(path) && Path::new(path).exists()
    }

    pub(super) fn create_permalink(path: &str) -> Result<String, String> {
        if !on_local_drive(&strip_win32_prefix(path)) {
            return Err("Links are only made for files on this PC's own drives".to_string());
        }
        let handle = open_metadata_handle(path)?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `handle` is an open Win32 file/folder handle and `info` is valid out-memory.
        unsafe { GetFileInformationByHandle(handle.0, &mut info) }
            .map_err(|e| format!("Couldn't read file ID for {path}: {}", e.message()))?;
        let frn = ((info.nFileIndexHigh as u64) << 32) | (info.nFileIndexLow as u64);

        let mut obj_buf = [0u8; 64];
        let mut returned = 0u32;
        // SAFETY: reads existing 64-byte FILE_OBJECTID_BUFFER without mutating read-only files.
        let object_id = unsafe {
            DeviceIoControl(
                handle.0,
                FSCTL_GET_OBJECT_ID,
                None,
                0,
                Some(obj_buf.as_mut_ptr().cast()),
                obj_buf.len() as u32,
                Some(&mut returned),
                None,
            )
            .is_ok()
        }
        .then(|| {
            if returned >= 16 {
                let mut oid = [0u8; 16];
                oid.copy_from_slice(&obj_buf[..16]);
                Some(oid)
            } else {
                None
            }
        })
        .flatten();

        let hint = final_path(handle.0).unwrap_or_else(|| path.to_string());
        Ok(format_uri(&Permalink {
            volume_serial: info.dwVolumeSerialNumber,
            frn,
            hint,
            object_id,
        }))
    }

    fn find_volume_root_by_serial(target: u32) -> Option<String> {
        // SAFETY: reads bitmask of logical drive letters.
        let mask = unsafe { GetLogicalDrives() };
        for i in 0..26u32 {
            if (mask & (1 << i)) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = format!("{letter}:\\");
            if !on_local_drive(&root) {
                continue;
            }
            let root_w = wide(&root);
            let mut serial = 0u32;
            // SAFETY: null-terminated root path; optional out-parameter for serial number.
            let ok =
                unsafe { GetVolumeInformationW(PCWSTR(root_w.as_ptr()), None, Some(&mut serial), None, None, None) }
                    .is_ok();
            if ok && serial == target {
                return Some(root);
            }
        }
        None
    }

    fn open_by_descriptor(root_handle: HANDLE, desc: &FILE_ID_DESCRIPTOR) -> Option<OwnedHandle> {
        // SAFETY: `root_handle` is valid directory handle on volume and `desc` has accurate `dwSize`.
        let h = unsafe {
            OpenFileById(
                root_handle,
                desc,
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_BACKUP_SEMANTICS.0),
            )
        }
        .ok()?;
        Some(OwnedHandle(h))
    }

    pub(super) fn resolve_permalink(link: &Permalink) -> Result<String, String> {
        if let Some(root) = find_volume_root_by_serial(link.volume_serial) {
            if let Ok(root_handle) = open_metadata_handle(&root) {
                let desc = FILE_ID_DESCRIPTOR {
                    dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
                    Type: FileIdType,
                    Anonymous: FILE_ID_DESCRIPTOR_0 {
                        FileId: link.frn as i64,
                    },
                };
                if let Some(found) = open_by_descriptor(root_handle.0, &desc) {
                    if let Some(path) = final_path(found.0) {
                        return Ok(path);
                    }
                }
                if let Some(oid) = link.object_id {
                    let guid = GUID::from_values(
                        u32::from_le_bytes(oid[0..4].try_into().unwrap_or_default()),
                        u16::from_le_bytes(oid[4..6].try_into().unwrap_or_default()),
                        u16::from_le_bytes(oid[6..8].try_into().unwrap_or_default()),
                        oid[8..16].try_into().unwrap_or_default(),
                    );
                    let oid_desc = FILE_ID_DESCRIPTOR {
                        dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32,
                        Type: ObjectIdType,
                        Anonymous: FILE_ID_DESCRIPTOR_0 { ObjectId: guid },
                    };
                    if let Some(found) = open_by_descriptor(root_handle.0, &oid_desc) {
                        if let Some(path) = final_path(found.0) {
                            return Ok(path);
                        }
                    }
                }
            }
        }

        if let Ok(Some(indexed)) = shadui_index::client::resolve_id(link.volume_serial, link.frn) {
            if usable(&indexed) {
                return Ok(indexed);
            }
        }

        if usable(&link.hint) {
            return Ok(link.hint.clone());
        }
        Err("Target file or folder could not be located on any connected drive".to_string())
    }

    /// The command `glint://` links run: `exe` with the link, which launch.rs reads.
    fn protocol_command(exe: &Path) -> String {
        format!("\"{}\" \"%1\"", exe.display())
    }

    /// The `glint://` handler under `classes` (HKCU's Software\Classes, or a test's stand-in): pointed at
    /// `installed`, written only when it runs something else; or, for a copy that isn't installed (`None`),
    /// taken out when the program it runs is gone. See `claim_protocol`.
    pub(crate) fn claim_protocol(classes: &str, installed: Option<&Path>) -> std::io::Result<()> {
        use crate::verbs::reg;
        let key = format!(r"{classes}\glint");
        let command_key = format!(r"{key}\shell\open\command");
        let current = reg::get_string(&command_key, "");
        match installed {
            Some(exe) => {
                let command = protocol_command(exe);
                if current.as_deref() != Some(command.as_str()) {
                    reg::set_string(&key, "", "URL:Glint link")?;
                    reg::set_string(&key, "URL Protocol", "")?;
                    reg::set_string(&command_key, "", &command)?;
                }
            }
            None => {
                // The program is the command's first quoted part.
                let program = current.as_deref().and_then(|c| c.strip_prefix('"')).and_then(|c| c.split('"').next());
                if program.is_some_and(|exe| !Path::new(exe).exists()) {
                    reg::delete_tree(&key)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_round_trip_handles_unicode_spaces_and_object_ids() {
        let original = Permalink {
            volume_serial: 0xABCD_1234,
            frn: 0x0003_0000_0000_4567,
            hint: "C:\\Users\\Ada Lovelace\\Reports\\résumé final (v2).pdf".to_string(),
            object_id: Some([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
        };
        let uri = format_uri(&original);
        assert!(uri.starts_with("glint://open?vol=abcd1234&id=0003000000004567&hint="));
        assert!(uri.contains("&oid=0102030405060708090a0b0c0d0e0f10"));
        let parsed = parse_uri(&uri).unwrap();
        assert_eq!(parsed, original);
    }

    #[cfg(windows)]
    #[test]
    fn only_this_pcs_own_drives_are_looked_at() {
        let temp = std::env::temp_dir();
        assert!(win::on_local_drive(&temp.to_string_lossy()));
        assert!(win::usable(&temp.to_string_lossy()));
        for elsewhere in [
            r"\\server\share\file.txt",
            r"\\?\UNC\server\share\file.txt",
            "//server/share/file.txt",
            r"\\.\pipe\glint",
            r"relative\file.txt",
            "C:",
            "",
        ] {
            assert!(!win::on_local_drive(elsewhere), "{elsewhere}");
        }
        // Refused before anything is opened, so Windows never tries to sign in to the server.
        assert!(!win::usable(r"\\glint-test.invalid\share\file.txt"));
        assert!(create_permalink(r"\\glint-test.invalid\share\file.txt").is_err());
    }

    /// The handler, under a stand-in for Software\Classes: an installed copy points it at itself, a copy
    /// that isn't installed leaves another copy's alone and only takes out one whose program is gone.
    #[cfg(windows)]
    #[test]
    fn only_an_installed_copy_takes_glint_links() {
        use crate::verbs::reg;
        let classes = format!(r"Software\GlintTests-permalinks-{}", std::process::id());
        let command = format!(r"{classes}\glint\shell\open\command");
        let installed = std::env::current_exe().unwrap();

        win::claim_protocol(&classes, None).unwrap();
        assert!(!reg::exists(&format!(r"{classes}\glint")), "a dev build doesn't take the links");

        win::claim_protocol(&classes, Some(&installed)).unwrap();
        let want = format!("\"{}\" \"%1\"", installed.display());
        assert_eq!(reg::get_string(&command, "").as_deref(), Some(want.as_str()));
        assert_eq!(reg::get_string(&format!(r"{classes}\glint"), "URL Protocol").as_deref(), Some(""));

        // Another copy's handler stays while its program is there.
        win::claim_protocol(&classes, None).unwrap();
        assert_eq!(reg::get_string(&command, "").as_deref(), Some(want.as_str()));

        // One running a program that's gone is taken out.
        reg::set_string(&command, "", r#""C:\Gone\Glint-glint-test.exe" "%1""#).unwrap();
        win::claim_protocol(&classes, None).unwrap();
        assert!(!reg::exists(&format!(r"{classes}\glint")));

        reg::delete_tree(&classes).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn permalink_resolves_real_file_after_rename_and_move_across_directories() {
        let root = std::env::temp_dir().join(format!("glint-permalink-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir_a = root.join("before");
        let dir_b = root.join("after").join("nested");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();

        let src = dir_a.join("original name.txt");
        std::fs::write(&src, b"immutable identity test").unwrap();

        let uri = create_permalink(&src.to_string_lossy()).expect("permalink created on NTFS/ReFS");
        let moved = dir_b.join("renamed target.txt");
        std::fs::rename(&src, &moved).unwrap();

        let resolved = resolve_permalink(&uri).expect("OpenFileById resolves moved file");
        let canon_moved = std::fs::canonicalize(&moved).unwrap();
        let canon_resolved = std::fs::canonicalize(&resolved).unwrap();
        assert_eq!(canon_resolved, canon_moved, "{resolved} vs {}", moved.display());

        let _ = std::fs::remove_dir_all(&root);
    }
}
