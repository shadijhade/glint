//! Getting into a folder Windows keeps from a user (C:\PerfLogs, another account's profile…) the way
//! File Explorer's Continue button does: the user's account is given read access to the folder and
//! everything in it, for good. The app tries this as the user first, who may own the folder, and else
//! runs this executable elevated, after the UAC prompt (`grant SID PATH`). Folders even administrators
//! may not change (System Volume Information, WindowsApps) are left as they are: taking one over can
//! stop Windows, or the apps it holds, from working.

use crate::win::{self, os_error, wide};
use std::fmt;
use std::io;
use std::path::PathBuf;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_ACCESS_DENIED, ERROR_SUCCESS, HLOCAL, WIN32_ERROR};
use windows::Win32::Security::Authorization::{
    ConvertStringSidToSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows::Win32::Security::{
    GetSecurityDescriptorControl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SE_DACL_PROTECTED, SUB_CONTAINERS_AND_OBJECTS_INHERIT, UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetFileAttributesW, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    INVALID_FILE_ATTRIBUTES,
};

/// "Read & execute" on a folder's Security tab: open it and its subfolders, read and run its files.
const READ_AND_EXECUTE: u32 = FILE_GENERIC_READ.0 | FILE_GENERIC_EXECUTE.0;
/// GetDriveTypeW's DRIVE_REMOTE.
const DRIVE_REMOTE: u32 = 4;

#[derive(Debug)]
pub enum Failure {
    /// Not a folder on this PC: a file, a link (its target is what opens), or a network share.
    NotAFolder,
    /// Who may open it can't be changed: by this user (an administrator can try next), or, elevated,
    /// by an administrator either.
    Refused,
    /// The administrator approval was declined.
    Cancelled,
    Other(io::Error),
}

impl Failure {
    /// The process exit code: what failed << 16, with the Windows error for anything else.
    pub fn exit_code(&self) -> i32 {
        match self {
            Failure::NotAFolder => 2 << 16,
            Failure::Refused => (3 << 16) | ERROR_ACCESS_DENIED.0 as i32,
            Failure::Cancelled => 5 << 16,
            Failure::Other(e) => (4 << 16) | (e.raw_os_error().unwrap_or(0) & 0xFFFF),
        }
    }

    /// Back from an exit code.
    pub fn from_exit_code(code: u32) -> Failure {
        match code >> 16 {
            2 => Failure::NotAFolder,
            3 => Failure::Refused,
            5 => Failure::Cancelled,
            _ => Failure::Other(io::Error::from_raw_os_error((code & 0xFFFF) as i32)),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::NotAFolder => write!(f, "Only a folder on this PC can be opened up this way"),
            Failure::Refused => write!(f, "Even administrators may not change who can open this folder"),
            Failure::Cancelled => write!(f, "cancelled"),
            Failure::Other(e) => write!(f, "{e}"),
        }
    }
}

fn win32(error: WIN32_ERROR) -> Result<(), Failure> {
    match error {
        ERROR_SUCCESS => Ok(()),
        ERROR_ACCESS_DENIED => Err(Failure::Refused),
        error => Err(Failure::Other(io::Error::from_raw_os_error(error.0 as i32))),
    }
}

/// Memory Windows handed out with LocalAlloc, freed when dropped.
struct Local(*mut core::ffi::c_void);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(Some(HLOCAL(self.0))) };
        }
    }
}

/// Whether `path` is a folder on a drive of this PC (not a share, not a link to somewhere else).
fn local_folder(path: &str) -> Result<(), Failure> {
    let bytes = path.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != b":\\" {
        return Err(Failure::NotAFolder);
    }
    if unsafe { GetDriveTypeW(PCWSTR(wide(&path[..3]).as_ptr())) } == DRIVE_REMOTE {
        return Err(Failure::NotAFolder);
    }
    let attrs = unsafe { GetFileAttributesW(PCWSTR(wide(path).as_ptr())) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        return Err(Failure::Other(io::Error::last_os_error()));
    }
    if attrs & FILE_ATTRIBUTE_DIRECTORY.0 == 0 || attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(Failure::NotAFolder);
    }
    Ok(())
}

/// Gives the account `sid` read access to the folder `path` and everything in it, keeping whatever
/// else its permissions say (whether it inherits its parent's too, and every other entry).
pub fn grant(path: &str, sid: &str) -> Result<(), Failure> {
    if !win::is_sid(sid) {
        return Err(Failure::Other(io::Error::new(io::ErrorKind::InvalidInput, "not a user's SID")));
    }
    local_folder(path)?;
    let name = wide(path);
    unsafe {
        let mut user = PSID::default();
        ConvertStringSidToSidW(PCWSTR(wide(sid).as_ptr()), &mut user).map_err(|e| Failure::Other(os_error(e)))?;
        let _user = Local(user.0);

        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        win32(GetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, Some(&mut dacl), None, &mut sd))?;
        let _sd = Local(sd.0);
        let (mut control, mut revision) = (0u16, 0u32);
        GetSecurityDescriptorControl(sd, &mut control, &mut revision).map_err(|e| Failure::Other(os_error(e)))?;
        // Set as it was: a folder that doesn't inherit its parent's permissions mustn't start to.
        let inheritance = if control & SE_DACL_PROTECTED.0 != 0 { PROTECTED_DACL_SECURITY_INFORMATION } else { UNPROTECTED_DACL_SECURITY_INFORMATION };

        let access = EXPLICIT_ACCESS_W {
            grfAccessPermissions: READ_AND_EXECUTE,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_USER,
                ptstrName: PWSTR(user.0 as *mut u16),
            },
        };
        let mut granted: *mut ACL = std::ptr::null_mut();
        win32(SetEntriesInAclW(Some(&[access]), Some(dacl), &mut granted))?;
        let _granted = Local(granted.cast());
        // Windows passes the new entry on to everything inside that inherits its folder's permissions.
        win32(SetNamedSecurityInfoW(PCWSTR(name.as_ptr()), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | inheritance, None, None, Some(granted), None))
    }
}

/// Quotes one command-line argument so that the C runtime's rules (which Rust's `args` follow) read
/// it back as it was: backslashes before a quote, or before the closing one, are doubled ("C:\").
pub(crate) fn quote(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
            continue;
        }
        let escaped = if c == '"' { backslashes * 2 + 1 } else { backslashes };
        out.extend(std::iter::repeat_n('\\', escaped));
        out.push(c);
        backslashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
    out
}

/// Gets the user running the app into the folder: as themselves when they may change its
/// permissions, else elevated with the `bundled` executable (Windows asks for administrator approval
/// over `owner`).
pub fn request(bundled: impl FnOnce() -> Result<PathBuf, String>, path: &str, owner: Option<isize>) -> Result<(), Failure> {
    let sid = win::current_user_sid().map_err(Failure::Other)?;
    match grant(path, &sid) {
        Err(Failure::Refused) => {}
        done => return done,
    }
    let exe = bundled().map_err(|e| Failure::Other(io::Error::other(e)))?;
    match crate::client::run_elevated(&exe, &format!("grant {sid} {}", quote(path)), owner) {
        Ok(0) => Ok(()),
        Ok(code) => Err(Failure::from_exit_code(code)),
        Err(e) if e == "cancelled" => Err(Failure::Cancelled),
        Err(e) => Err(Failure::Other(io::Error::other(e))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn exit_codes_round_trip() {
        for failure in [Failure::NotAFolder, Failure::Refused, Failure::Cancelled] {
            let back = Failure::from_exit_code(failure.exit_code() as u32);
            assert_eq!(back.to_string(), failure.to_string());
        }
        let other = Failure::from_exit_code(Failure::Other(io::Error::from_raw_os_error(32)).exit_code() as u32);
        assert!(matches!(other, Failure::Other(e) if e.raw_os_error() == Some(32)));
    }

    #[test]
    fn quotes_arguments_as_the_runtime_reads_them() {
        assert_eq!(quote(r"C:\PerfLogs"), r#""C:\PerfLogs""#);
        assert_eq!(quote(r"C:\"), r#""C:\\""#);
        assert_eq!(quote(r"C:\My Folder\sub\"), r#""C:\My Folder\sub\\""#);
        assert_eq!(quote(r#"a\"b"#), r#""a\\\"b""#);
    }

    fn icacls(path: &std::path::Path, args: &[&str]) -> String {
        let out = Command::new("icacls").arg(path).args(args).output().unwrap();
        assert!(out.status.success(), "icacls {args:?}: {}", String::from_utf8_lossy(&out.stdout));
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn opens_a_folder_up_for_its_owner_and_keeps_the_rest() {
        // A folder of the test's own that keeps its owner out, as C:\PerfLogs keeps users out, with a
        // subfolder and a file that inherit that. As the owner, the test may still change who may open it.
        let dir = std::env::temp_dir().join(format!("glint-access-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("notes.txt"), "hi").unwrap();
        icacls(&dir, &["/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"]);
        icacls(&dir, &["/inheritance:r"]);
        // Elevated, the test runs as an administrator, whom the folder lets in anyway.
        if !win::is_elevated() {
            assert!(std::fs::read_dir(&dir).is_err(), "the folder starts out locked");
        }

        let result = grant(&dir.to_string_lossy(), &win::current_user_sid().unwrap());
        let acl = icacls(&dir, &[]);
        let inside = std::fs::read_to_string(dir.join("sub").join("notes.txt"));
        // Put back as it was before anything can fail, so the folder can be deleted.
        icacls(&dir, &["/reset", "/T", "/Q"]);
        let _ = std::fs::remove_dir_all(&dir);

        result.unwrap();
        assert!(acl.contains("(OI)(CI)(RX)"), "{acl}");
        assert!(acl.contains("(OI)(CI)(F)"), "the other entries stay: {acl}");
        assert!(!acl.contains("(I)"), "it still doesn't inherit its parent's permissions: {acl}");
        assert_eq!(inside.unwrap(), "hi", "what's inside opens too");
    }

    #[test]
    fn leaves_files_links_and_shares_alone() {
        let dir = std::env::temp_dir().join(format!("glint-access-{}-file", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let me = win::current_user_sid().unwrap();
        let file_result = grant(&file.to_string_lossy(), &me);
        let bad_sid = grant(&dir.to_string_lossy(), "S-1-5-21)(A;;GA;;;WD");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(file_result, Err(Failure::NotAFolder)));
        assert!(matches!(grant(r"\\server\share\folder", &me), Err(Failure::NotAFolder)));
        assert!(matches!(bad_sid, Err(Failure::Other(_))));
    }
}
