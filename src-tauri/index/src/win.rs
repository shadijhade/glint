//! Small safe wrappers over the Win32 calls the service, the installer and the client share.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE,
    REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_MULTI_SZ, RRF_RT_REG_SZ,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};

/// A NUL-terminated UTF-16 copy of `text`.
pub fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

/// A windows-rs error as an io::Error, keeping the Win32 code when there is one.
pub fn os_error(e: windows::core::Error) -> io::Error {
    let code = e.code().0 as u32;
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        io::Error::other(e.message())
    }
}

/// An owned kernel handle, closed when dropped.
pub struct Handle(pub HANDLE);

// Kernel handles are process-wide; each one is used by one thread at a time here.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn process_token() -> io::Result<Handle> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(os_error)?;
    Ok(Handle(token))
}

/// Whether this process runs with an administrator's full token (after the UAC prompt).
pub fn is_elevated() -> bool {
    let Ok(token) = process_token() else { return false };
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0;
    let ok = unsafe {
        GetTokenInformation(token.0, TokenElevation, Some(&mut elevation as *mut _ as *mut _), size_of::<TOKEN_ELEVATION>() as u32, &mut len)
    };
    ok.is_ok() && elevation.TokenIsElevated != 0
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text = PWSTR::null();
    unsafe { ConvertSidToStringSidW(sid, &mut text) }.map_err(os_error)?;
    let result = unsafe { text.to_string() }.map_err(|_| io::Error::other("unreadable SID"));
    unsafe { LocalFree(Some(HLOCAL(text.0 as _))) };
    result
}

/// The user this process runs as, e.g. "S-1-5-21-…-1001".
pub fn current_user_sid() -> io::Result<String> {
    let token = process_token()?;
    let mut len = 0;
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) };
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    unsafe { GetTokenInformation(token.0, TokenUser, Some(buf.as_mut_ptr() as *mut _), len, &mut len) }.map_err(os_error)?;
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    sid_string(user.User.Sid)
}

/// Whether `text` is a SID Windows accepts (and nothing that could smuggle other SDDL in).
pub fn is_sid(text: &str) -> bool {
    if !text.starts_with("S-1-") || !text.bytes().skip(4).all(|b| b.is_ascii_digit() || b == b'-') {
        return false;
    }
    let mut sid = PSID::default();
    let ok = unsafe { ConvertStringSidToSidW(PCWSTR(wide(text).as_ptr()), &mut sid) }.is_ok();
    if ok {
        unsafe { LocalFree(Some(HLOCAL(sid.0))) };
    }
    ok
}

/// A security descriptor built from SDDL, to create an object with.
pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

impl SecurityDescriptor {
    pub fn from_sddl(sddl: &str) -> io::Result<Self> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(wide(sddl).as_ptr()), SDDL_REVISION_1, &mut sd, None) }
            .map_err(os_error)?;
        Ok(SecurityDescriptor(sd))
    }

    /// Valid while `self` lives.
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: self.0 .0, bInheritHandle: false.into() }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
    }
}

/// The owner of a kernel object such as a pipe, as a string SID.
pub fn owner_of(handle: HANDLE) -> io::Result<String> {
    let mut owner = PSID::default();
    let mut sd = PSECURITY_DESCRIPTOR::default();
    let err = unsafe { GetSecurityInfo(handle, SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION, Some(&mut owner), None, None, None, Some(&mut sd)) };
    if err != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(err.0 as i32));
    }
    let result = sid_string(owner);
    unsafe { LocalFree(Some(HLOCAL(sd.0))) };
    result
}

pub fn known_folder(id: &GUID) -> io::Result<PathBuf> {
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) }.map_err(os_error)?;
    let result = unsafe { path.to_string() }.map(PathBuf::from).map_err(|_| io::Error::other("unreadable folder path"));
    unsafe { CoTaskMemFree(Some(path.0 as *const _)) };
    result
}

/// An open HKLM key, closed when dropped.
pub struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn reg(result: windows::Win32::Foundation::WIN32_ERROR) -> io::Result<()> {
    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(result.0 as i32))
    }
}

impl Key {
    /// Opens (creating if needed) `HKLM\{path}`.
    pub fn create(path: &str, write: bool) -> io::Result<Key> {
        let mut key = HKEY::default();
        let access = if write { KEY_READ | KEY_WRITE } else { KEY_READ };
        reg(unsafe {
            RegCreateKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(wide(path).as_ptr()), None, None, REG_OPTION_NON_VOLATILE, access, None, &mut key, None)
        })?;
        Ok(Key(key))
    }

    fn get(&self, name: &str, flags: windows::Win32::System::Registry::REG_ROUTINE_FLAGS) -> io::Result<Vec<u16>> {
        let name = wide(name);
        let mut len = 0u32;
        let mut kind = REG_VALUE_TYPE::default();
        reg(unsafe { RegGetValueW(self.0, None, PCWSTR(name.as_ptr()), flags, Some(&mut kind), None, Some(&mut len)) })?;
        let mut buf = vec![0u16; (len as usize).div_ceil(2)];
        reg(unsafe { RegGetValueW(self.0, None, PCWSTR(name.as_ptr()), flags, Some(&mut kind), Some(buf.as_mut_ptr() as *mut _), Some(&mut len)) })?;
        buf.truncate(len as usize / 2);
        Ok(buf)
    }

    /// A REG_SZ or REG_EXPAND_SZ value (expanded).
    pub fn string(&self, name: &str) -> io::Result<String> {
        let buf = self.get(name, RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ)?;
        Ok(String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string())
    }

    pub fn strings(&self, name: &str) -> io::Result<Vec<String>> {
        let buf = self.get(name, RRF_RT_REG_MULTI_SZ)?;
        Ok(buf.split(|&u| u == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect())
    }

    pub fn set_string(&self, name: &str, value: &str) -> io::Result<()> {
        let bytes: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
        reg(unsafe { RegSetValueExW(self.0, PCWSTR(wide(name).as_ptr()), None, REG_SZ, Some(&bytes)) })
    }

    /// Deletes a value; one that isn't there is fine. True if there was one.
    pub fn delete(&self, name: &str) -> io::Result<bool> {
        match reg(unsafe { RegDeleteValueW(self.0, PCWSTR(wide(name).as_ptr())) }) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn set_strings(&self, name: &str, values: &[String]) -> io::Result<()> {
        let mut data: Vec<u16> = Vec::new();
        for value in values {
            data.extend(value.encode_utf16());
            data.push(0);
        }
        data.push(0);
        let bytes: Vec<u8> = data.iter().flat_map(|u| u.to_le_bytes()).collect();
        reg(unsafe { RegSetValueExW(self.0, PCWSTR(wide(name).as_ptr()), None, REG_MULTI_SZ, Some(&bytes)) })
    }
}

/// Where the service keeps its snapshots and log: LocalSystem's own local app data, which only
/// SYSTEM and administrators can open (the snapshots list every file name on the drives).
pub fn data_dir() -> io::Result<PathBuf> {
    let profiles = Key::create(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\S-1-5-18", false)?;
    let profile = profiles.string("ProfileImagePath")?;
    let profile = expand(&profile);
    Ok(PathBuf::from(profile).join("AppData").join("Local").join("ShadUI Ripgrep").join("Index"))
}

/// From the registry: GetVersionEx answers for compatibility, and ProductName still says
/// "Windows 10" on Windows 11.
pub fn windows_version() -> String {
    let Ok(key) = Key::create(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", false) else { return "Windows".into() };
    let get = |name: &str| key.string(name).unwrap_or_default();
    let build = get("CurrentBuild");
    let release = if build.parse::<u32>().is_ok_and(|b| b >= 22000) { "11" } else { "10" };
    let product = get("ProductName");
    let edition = product.trim_start_matches("Windows 10 ").trim_start_matches("Windows 11 ");
    format!("Windows {release} {edition} {} (build {build})", get("DisplayVersion"))
}

/// Expands %VARIABLES% the way Windows does.
fn expand(text: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    let source = wide(text);
    let mut buf = vec![0u16; 1024];
    let len = unsafe { ExpandEnvironmentStringsW(PCWSTR(source.as_ptr()), Some(&mut buf)) } as usize;
    if len == 0 || len > buf.len() {
        return text.to_string();
    }
    String::from_utf16_lossy(&buf[..len - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_who_it_runs_as() {
        let sid = current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert!(is_sid(&sid));
        assert!(!is_sid("S-1-5-21)(A;;GA;;;WD"));
        assert!(!is_sid("WD"));
        assert!(!is_sid("S-1-"));
    }

    #[test]
    fn builds_security_descriptors() {
        assert!(SecurityDescriptor::from_sddl("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;S-1-5-11)").is_ok());
        assert!(SecurityDescriptor::from_sddl("D:P(A;;GA;;;not-a-sid)").is_err());
    }

    #[test]
    fn finds_the_system_profile() {
        let dir = data_dir().unwrap();
        assert!(dir.to_string_lossy().to_lowercase().contains(r"system32\config\systemprofile"), "{}", dir.display());
    }
}
