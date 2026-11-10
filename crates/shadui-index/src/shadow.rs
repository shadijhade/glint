//! Windows' shadow copies: System Restore's restore points and backups' snapshots, each a whole volume as
//! it was, readable at its own device path (\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN\) with every
//! file's own permissions. VSS lists them only for an administrator, so the service answers
//! (Request::Snapshots); without it, `probe` finds the copies' devices by name (which any user may see),
//! matches each to its drive by serial number and dates it by the newest entry at its top.

use serde::{Deserialize, Serialize};

/// Where shadow copies' devices are, as a path.
pub const DEVICES: &str = r"\\?\GLOBALROOT\Device\";

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Its top: \\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN\.
    pub root: String,
    /// The drive it's a copy of ("C:"), "" for a volume without a letter.
    pub drive: String,
    /// When it was taken (Unix ms, 0 when unknown).
    pub created: i64,
    /// VSS's own time; false when it's from the copy's newest entry (a little before, at most).
    pub exact: bool,
}

/// Newest first, each copy once.
pub fn sorted(mut all: Vec<Snapshot>) -> Vec<Snapshot> {
    all.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| number(&b.root).cmp(&number(&a.root))));
    let mut seen = std::collections::HashSet::new();
    all.retain(|s| seen.insert(s.root.to_ascii_lowercase()));
    all
}

/// The N of HarddiskVolumeShadowCopyN: Windows numbers them as they're made (and again from 1 at boot).
fn number(root: &str) -> u64 {
    let digits: String = root.trim_end_matches('\\').chars().rev().take_while(char::is_ascii_digit).collect();
    digits.chars().rev().collect::<String>().parse().unwrap_or(0)
}

#[cfg(windows)]
pub use imp::{list, probe};

#[cfg(windows)]
mod imp {
    use super::{sorted, Snapshot, DEVICES};
    use windows::core::{GUID, PCWSTR, PWSTR};
    use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows::Wdk::Storage::FileSystem::{NtOpenDirectoryObject, NtQueryDirectoryObject};
    use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, HANDLE, NTSTATUS, OBJECT_ATTRIBUTE_FLAGS, UNICODE_STRING};
    use windows::Win32::Storage::FileSystem::{
        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindFirstVolumeW, FindNextFileW, FindNextVolumeW, FindVolumeClose, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, GetVolumePathNamesForVolumeNameW,
        FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
    };
    use windows::Win32::Storage::Vss::{IVssSnapshotMgmt, VssSnapshotMgmt, VSS_OBJECT_PROP, VSS_OBJECT_SNAPSHOT, VSS_SNAPSHOT_PROP};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};

    /// Windows' own shadow copy provider: System Restore's, File History's and most backups'.
    const SYSTEM_PROVIDER: GUID = GUID::from_u128(0xb5946137_7b9f_4925_af80_51abd60b20d5);
    /// More of a directory's entries are left (NtQueryDirectoryObject).
    const STATUS_MORE_ENTRIES: NTSTATUS = NTSTATUS(0x105);
    const DIRECTORY_QUERY: u32 = 1;
    const OBJ_CASE_INSENSITIVE: OBJECT_ATTRIBUTE_FLAGS = OBJECT_ATTRIBUTE_FLAGS(0x40);
    const DRIVE_FIXED: u32 = 3;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    fn unix_ms(filetime: i64) -> i64 {
        if filetime <= 0 {
            0
        } else {
            (filetime - 116_444_736_000_000_000) / 10_000
        }
    }

    /// Through VSS: every shadow copy of every volume, with the time VSS took it. Takes an administrator
    /// (the service): Err when Windows says no.
    pub fn list() -> Result<Vec<Snapshot>, String> {
        unsafe {
            let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let result = query();
            if com {
                CoUninitialize();
            }
            result
        }
    }

    unsafe fn query() -> Result<Vec<Snapshot>, String> {
        unsafe {
            let mgmt: IVssSnapshotMgmt = CoCreateInstance(&VssSnapshotMgmt, None, CLSCTX_ALL).map_err(|e| format!("Windows' shadow copies can't be listed: {}", e.message()))?;
            let mut out = Vec::new();
            for volume in volumes() {
                let name = wide(&volume);
                let found = match mgmt.QuerySnapshotsByVolume(name.as_ptr(), SYSTEM_PROVIDER) {
                    Ok(found) => found,
                    Err(e) if e.code() == E_ACCESSDENIED => return Err("Listing shadow copies takes an administrator".into()),
                    // None, or a volume Windows doesn't copy.
                    Err(_) => continue,
                };
                let drive = drive_of(&volume);
                loop {
                    let mut props = [VSS_OBJECT_PROP::default()];
                    let mut fetched = 0u32;
                    if found.Next(&mut props, &mut fetched).is_err() || fetched == 0 {
                        break;
                    }
                    if props[0].Type != VSS_OBJECT_SNAPSHOT {
                        continue;
                    }
                    let snap = props[0].Obj.Snap;
                    let device = if snap.m_pwszSnapshotDeviceObject.is_null() { String::new() } else { PCWSTR(snap.m_pwszSnapshotDeviceObject).to_string().unwrap_or_default() };
                    if !device.is_empty() {
                        out.push(Snapshot { root: format!("{}\\", device.trim_end_matches('\\')), drive: drive.clone(), created: unix_ms(snap.m_tsCreationTimestamp), exact: true });
                    }
                    free(&snap);
                }
            }
            Ok(sorted(out))
        }
    }

    /// What VSS allocated for a copy's properties (VssFreeSnapshotProperties does just this).
    unsafe fn free(snap: &VSS_SNAPSHOT_PROP) {
        for text in [snap.m_pwszSnapshotDeviceObject, snap.m_pwszOriginalVolumeName, snap.m_pwszOriginatingMachine, snap.m_pwszServiceMachine, snap.m_pwszExposedName, snap.m_pwszExposedPath] {
            if !text.is_null() {
                unsafe { CoTaskMemFree(Some(text as *const _)) };
            }
        }
    }

    /// Every volume's name (\\?\Volume{…}\).
    fn volumes() -> Vec<String> {
        let mut out = Vec::new();
        let mut name = [0u16; 80];
        unsafe {
            let Ok(find) = FindFirstVolumeW(&mut name) else { return out };
            loop {
                out.push(String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())]));
                if FindNextVolumeW(find, &mut name).is_err() {
                    break;
                }
            }
            let _ = FindVolumeClose(find);
        }
        out
    }

    /// The drive letter a volume has ("C:"), or "".
    fn drive_of(volume: &str) -> String {
        let mut paths = [0u16; 1024];
        let mut len = 0u32;
        let name = wide(volume);
        if unsafe { GetVolumePathNamesForVolumeNameW(PCWSTR(name.as_ptr()), Some(&mut paths), &mut len) }.is_err() {
            return String::new();
        }
        // Zero-ended paths, then an empty one.
        paths.split(|&c| c == 0).map(String::from_utf16_lossy).find(|p| p.len() == 3 && p.ends_with(":\\")).map(|p| p[..2].to_ascii_uppercase()).unwrap_or_default()
    }

    /// Without an administrator: the copies' devices by name, each matched to its drive by serial number
    /// (a copy has its volume's) and dated by the newest entry at its top.
    pub fn probe() -> Vec<Snapshot> {
        let drives: Vec<(u32, String)> = fixed_drives().into_iter().filter_map(|d| serial(&format!("{d}\\")).map(|s| (s, d))).collect();
        let mut out = Vec::new();
        for name in shadow_devices() {
            let root = format!("{DEVICES}{name}\\");
            let Some(serial) = serial(&root) else { continue };
            let drive = drives.iter().find(|(s, _)| *s == serial).map(|(_, d)| d.clone()).unwrap_or_default();
            out.push(Snapshot { created: newest_entry(&root), root, drive, exact: false });
        }
        sorted(out)
    }

    fn fixed_drives() -> Vec<String> {
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| format!("{}:", (b'A' + i) as char))
            .filter(|d| unsafe { GetDriveTypeW(PCWSTR(wide(&format!("{d}\\")).as_ptr())) } == DRIVE_FIXED)
            .collect()
    }

    fn serial(root: &str) -> Option<u32> {
        let mut serial = 0u32;
        let path = wide(root);
        unsafe { GetVolumeInformationW(PCWSTR(path.as_ptr()), None, Some(&mut serial), None, None, None) }.ok()?;
        Some(serial)
    }

    /// An entry of an object directory (OBJECT_DIRECTORY_INFORMATION).
    #[repr(C)]
    struct DirectoryEntry {
        name: UNICODE_STRING,
        kind: UNICODE_STRING,
    }

    /// The names in \Device that are shadow copies (HarddiskVolumeShadowCopyN).
    fn shadow_devices() -> Vec<String> {
        let mut out = Vec::new();
        unsafe {
            let mut path: Vec<u16> = r"\Device".encode_utf16().collect();
            let bytes = (path.len() * 2) as u16;
            let name = UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: PWSTR(path.as_mut_ptr()) };
            let attributes = OBJECT_ATTRIBUTES { Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32, ObjectName: &name, Attributes: OBJ_CASE_INSENSITIVE, ..Default::default() };
            let mut dir = HANDLE::default();
            if NtOpenDirectoryObject(&mut dir, DIRECTORY_QUERY, &attributes).is_err() {
                return out;
            }
            // 64 KB, 8-byte aligned: the entries, a zeroed one, then their names.
            let mut buffer = vec![0u64; 8192];
            let mut context = 0u32;
            let mut restart = true;
            loop {
                let status = NtQueryDirectoryObject(dir, Some(buffer.as_mut_ptr().cast()), (buffer.len() * 8) as u32, false, restart, &mut context, None);
                restart = false;
                if status.is_err() {
                    break;
                }
                let entries = buffer.as_ptr() as *const DirectoryEntry;
                for i in 0.. {
                    let entry = &*entries.add(i);
                    if entry.name.Buffer.is_null() || entry.name.Length == 0 {
                        break;
                    }
                    let name = String::from_utf16_lossy(std::slice::from_raw_parts(entry.name.Buffer.0, entry.name.Length as usize / 2));
                    if name.starts_with("HarddiskVolumeShadowCopy") {
                        out.push(name);
                    }
                }
                if status != STATUS_MORE_ENTRIES {
                    break;
                }
            }
            let _ = CloseHandle(dir);
        }
        out
    }

    /// The newest last-written time among what's at a copy's top (System Volume Information, where VSS
    /// writes as it takes one, among them): when the copy was taken, or a little before.
    fn newest_entry(root: &str) -> i64 {
        let pattern = wide(&format!("{root}*"));
        let mut data = WIN32_FIND_DATAW::default();
        let mut newest = 0i64;
        unsafe {
            let Ok(find) = FindFirstFileExW(PCWSTR(pattern.as_ptr()), FindExInfoBasic, &mut data as *mut _ as *mut _, FindExSearchNameMatch, None, FIND_FIRST_EX_LARGE_FETCH) else { return 0 };
            loop {
                newest = newest.max(((data.ftLastWriteTime.dwHighDateTime as i64) << 32) | data.ftLastWriteTime.dwLowDateTime as i64);
                if FindNextFileW(find, &mut data).is_err() {
                    break;
                }
            }
            let _ = FindClose(find);
        }
        unix_ms(newest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_each_once() {
        let at = |n: u32, created: i64| Snapshot { root: format!(r"{DEVICES}HarddiskVolumeShadowCopy{n}\"), drive: "C:".into(), created, exact: true };
        let got = sorted(vec![at(2, 10), at(11, 30), at(3, 20), at(11, 30), at(9, 0), at(10, 0)]);
        let order: Vec<u64> = got.iter().map(|s| number(&s.root)).collect();
        assert_eq!(order, [11, 3, 2, 10, 9], "by time, then the later number first when a time isn't known");
    }

    /// What this PC has (often none): each found the same way either way, its drive known.
    #[cfg(windows)]
    #[test]
    fn probes_without_an_administrator() {
        for snapshot in probe() {
            assert!(snapshot.root.starts_with(DEVICES) && snapshot.root.ends_with('\\'), "{snapshot:?}");
            assert!(snapshot.created > 0, "{snapshot:?}");
        }
        // Without an administrator VSS says no; as one, it agrees with the probe on the copies there are.
        if let Ok(listed) = list() {
            let mut a: Vec<String> = listed.iter().map(|s| s.root.to_ascii_lowercase()).collect();
            let mut b: Vec<String> = probe().iter().map(|s| s.root.to_ascii_lowercase()).collect();
            a.sort();
            b.sort();
            assert_eq!(a, b);
        }
    }
}
