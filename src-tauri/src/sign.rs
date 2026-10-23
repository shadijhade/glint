//! Whether a file's Authenticode signature holds, as Windows checks it (WinVerifyTrust), offline: no
//! revocation lists are fetched, so it answers in milliseconds, with or without a network. A file with
//! no signature of its own may be signed in one of Windows' catalogs (Windows' own files mostly are),
//! looked up by the file's hash.

use crate::fsx::blocking;
use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Signature {
    /// "valid", "unsigned", "invalid" (changed since it was signed, or a damaged signature),
    /// "untrusted" (by a certificate Windows doesn't trust), "expired", "revoked", or "unknown".
    pub state: &'static str,
    /// Signed in a Windows catalog rather than in the file itself.
    pub catalog: bool,
    /// Who signed it (its certificate's name), and who issued that certificate.
    pub signer: Option<String>,
    pub issuer: Option<String>,
    /// When it was signed, by its timestamp (ms), when it has one.
    pub signed_at: Option<i64>,
    /// What Windows said, when it doesn't hold.
    pub detail: Option<String>,
}

#[tauri::command]
pub async fn verify_signature(path: String) -> Result<Signature, String> {
    blocking(move || Ok(verify(&path))).await
}

const TRUST_E_PROVIDER_UNKNOWN: i32 = 0x800B_0001_u32 as i32;
const TRUST_E_SUBJECT_FORM_UNKNOWN: i32 = 0x800B_0003_u32 as i32;
const TRUST_E_SUBJECT_NOT_TRUSTED: i32 = 0x800B_0004_u32 as i32;
const TRUST_E_NOSIGNATURE: i32 = 0x800B_0100_u32 as i32;
const CERT_E_EXPIRED: i32 = 0x800B_0101_u32 as i32;
const CERT_E_UNTRUSTEDROOT: i32 = 0x800B_0109_u32 as i32;
const CERT_E_CHAINING: i32 = 0x800B_010A_u32 as i32;
const CERT_E_REVOKED: i32 = 0x800B_010C_u32 as i32;
const TRUST_E_EXPLICIT_DISTRUST: i32 = 0x800B_0111_u32 as i32;
const TRUST_E_BAD_DIGEST: i32 = 0x8009_6010_u32 as i32;
const CRYPT_E_BAD_MSG: i32 = 0x8009_200D_u32 as i32;
const CRYPT_E_SECURITY_SETTINGS: i32 = 0x8009_2026_u32 as i32;

/// What WinVerifyTrust's answer means.
fn state_of(status: i32) -> &'static str {
    match status {
        0 => "valid",
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN => "unsigned",
        TRUST_E_BAD_DIGEST | CRYPT_E_BAD_MSG => "invalid",
        CERT_E_EXPIRED => "expired",
        CERT_E_REVOKED => "revoked",
        CERT_E_UNTRUSTEDROOT | CERT_E_CHAINING | TRUST_E_EXPLICIT_DISTRUST | TRUST_E_SUBJECT_NOT_TRUSTED | CRYPT_E_SECURITY_SETTINGS => "untrusted",
        _ => "unknown",
    }
}

#[cfg(windows)]
pub fn verify(path: &str) -> Signature {
    imp::verify(path)
}

#[cfg(not(windows))]
pub fn verify(_: &str) -> Signature {
    Signature { state: "unknown", ..Default::default() }
}

#[cfg(windows)]
mod imp {
    use super::{state_of, Signature};
    use std::os::windows::io::AsRawHandle;
    use windows::core::{w, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{FILETIME, HANDLE, HWND, INVALID_HANDLE_VALUE};
    use windows::Win32::Security::Cryptography::Catalog::{
        CryptCATAdminAcquireContext2, CryptCATAdminCalcHashFromFileHandle2, CryptCATAdminEnumCatalogFromHash, CryptCATAdminReleaseCatalogContext, CryptCATAdminReleaseContext,
        CryptCATCatalogInfoFromContext, CATALOG_INFO,
    };
    use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_CONTEXT, CERT_NAME_ISSUER_FLAG, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows::Win32::Security::WinTrust::{
        WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_CATALOG_INFO, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_CATALOG, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_NONE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn ms_of(time: FILETIME) -> i64 {
        shadui_index::proto::unix_ms((time.dwHighDateTime as i64) << 32 | time.dwLowDateTime as i64)
    }

    /// A certificate's name, or (with CERT_NAME_ISSUER_FLAG) its issuer's.
    unsafe fn name(cert: *const CERT_CONTEXT, flags: u32) -> Option<String> {
        if cert.is_null() {
            return None;
        }
        let len = unsafe { CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, None, None) } as usize;
        if len <= 1 {
            return None;
        }
        let mut text = vec![0u16; len];
        unsafe { CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, None, Some(&mut text)) };
        Some(String::from_utf16_lossy(&text[..len - 1]))
    }

    /// Runs WinVerifyTrust on `data` (what it checks already chosen), offline, reads who signed it
    /// and when, and lets go of what it kept.
    unsafe fn check(mut data: WINTRUST_DATA, catalog: bool) -> Signature {
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_NONE;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        data.dwProvFlags = WTD_CACHE_ONLY_URL_RETRIEVAL | WTD_REVOCATION_CHECK_NONE;
        let no_window = HWND(INVALID_HANDLE_VALUE.0);
        let status = unsafe { WinVerifyTrust(no_window, &mut action, &mut data as *mut _ as *mut _) };
        let mut signature = Signature { state: state_of(status), catalog, detail: (status != 0).then(|| HRESULT(status).message()), ..Default::default() };
        unsafe {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            let signer = if provider.is_null() { std::ptr::null_mut() } else { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
            if let Some(signer) = signer.as_ref() {
                if signer.csCertChain > 0 && !signer.pasCertChain.is_null() {
                    let cert = (*signer.pasCertChain).pCert;
                    signature.signer = name(cert, 0);
                    signature.issuer = name(cert, CERT_NAME_ISSUER_FLAG);
                }
                // The timestamp's counter-signature says when it was signed.
                if signer.csCounterSigners > 0 && !signer.pasCounterSigners.is_null() {
                    signature.signed_at = Some(ms_of((*signer.pasCounterSigners).sftVerifyAsOf));
                }
            }
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            WinVerifyTrust(no_window, &mut action, &mut data as *mut _ as *mut _);
        }
        signature
    }

    pub fn verify(path: &str) -> Signature {
        let path_w = wide(path);
        let mut file = WINTRUST_FILE_INFO { cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: PCWSTR(path_w.as_ptr()), ..Default::default() };
        let data = WINTRUST_DATA { dwUnionChoice: WTD_CHOICE_FILE, Anonymous: WINTRUST_DATA_0 { pFile: &mut file }, ..Default::default() };
        let own = unsafe { check(data, false) };
        // A catalog is found by the whole file's hash: past 100 MB that's slow, and Windows' own
        // files that big are signed in themselves.
        if own.state != "unsigned" || std::fs::metadata(path).is_ok_and(|m| m.len() > CATALOG_LIMIT) {
            return own;
        }
        in_catalog(path, &path_w).unwrap_or(Signature { state: "unsigned", ..Default::default() })
    }

    const CATALOG_LIMIT: u64 = 100 << 20;

    /// The Windows catalog that signs the file, found by its hash (SHA-256 catalogs, then SHA-1 ones).
    fn in_catalog(path: &str, path_w: &[u16]) -> Option<Signature> {
        let file = std::fs::File::open(path).ok()?;
        let handle = HANDLE(file.as_raw_handle());
        for algorithm in [w!("SHA256"), w!("SHA1")] {
            let mut admin = 0isize;
            if unsafe { CryptCATAdminAcquireContext2(&mut admin, None, algorithm, None, None) }.is_err() {
                continue;
            }
            let mut found = None;
            let mut size = 0u32;
            let _ = unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut size, None, None) };
            let mut hash = vec![0u8; size as usize];
            if size > 0 && unsafe { CryptCATAdminCalcHashFromFileHandle2(admin, handle, &mut size, Some(hash.as_mut_ptr()), None) }.is_ok() {
                let catalog = unsafe { CryptCATAdminEnumCatalogFromHash(admin, &hash, None, None) };
                if catalog != 0 {
                    let mut info = CATALOG_INFO { cbStruct: size_of::<CATALOG_INFO>() as u32, ..Default::default() };
                    if unsafe { CryptCATCatalogInfoFromContext(catalog, &mut info, 0) }.is_ok() {
                        // The catalog lists its members by their hash, in upper-case hex.
                        let tag = wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
                        let mut member = WINTRUST_CATALOG_INFO {
                            cbStruct: size_of::<WINTRUST_CATALOG_INFO>() as u32,
                            pcwszCatalogFilePath: PCWSTR(info.wszCatalogFile.as_ptr()),
                            pcwszMemberTag: PCWSTR(tag.as_ptr()),
                            pcwszMemberFilePath: PCWSTR(path_w.as_ptr()),
                            hMemberFile: handle,
                            pbCalculatedFileHash: hash.as_mut_ptr(),
                            cbCalculatedFileHash: size,
                            hCatAdmin: admin,
                            ..Default::default()
                        };
                        let data = WINTRUST_DATA { dwUnionChoice: WTD_CHOICE_CATALOG, Anonymous: WINTRUST_DATA_0 { pCatalog: &mut member }, ..Default::default() };
                        found = Some(unsafe { check(data, true) });
                    }
                    let _ = unsafe { CryptCATAdminReleaseCatalogContext(admin, catalog, 0) };
                }
            }
            let _ = unsafe { CryptCATAdminReleaseContext(admin, 0) };
            if found.is_some() {
                return found;
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_what_windows_signs_from_what_it_doesnt() {
        // Windows' own: signed, by Microsoft (in a catalog or in the file).
        let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let kernel32 = verify(&format!(r"{windows}\System32\kernel32.dll"));
        assert_eq!(kernel32.state, "valid", "{kernel32:?}");
        assert!(kernel32.signer.as_deref().is_some_and(|s| s.contains("Microsoft")), "{kernel32:?}");
        assert!(kernel32.issuer.is_some());

        let dir = std::env::temp_dir().join(format!("glint-sign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain.exe");
        std::fs::write(&plain, b"MZ not really a program").unwrap();
        assert_eq!(verify(&plain.to_string_lossy()).state, "unsigned");

        // A program signed in itself (Edge, as Windows ships it; Windows' own are in catalogs), changed
        // after it was signed: the signature no longer holds.
        let x86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
        let signed = format!(r"{x86}\Microsoft\Edge\Application\msedge.exe");
        if std::path::Path::new(&signed).exists() {
            let original = verify(&signed);
            assert!(original.state == "valid" && !original.catalog && original.signed_at.is_some(), "{original:?}");
            let mut bytes = std::fs::read(&signed).unwrap();
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0xFF;
            let changed = dir.join("changed.exe");
            std::fs::write(&changed, bytes).unwrap();
            let after = verify(&changed.to_string_lossy());
            assert_eq!(after.state, "invalid", "{signed}: {after:?}");
            assert!(after.signer.is_some() && after.detail.is_some(), "{after:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tells_what_windows_answered() {
        assert_eq!(state_of(0), "valid");
        assert_eq!(state_of(TRUST_E_NOSIGNATURE), "unsigned");
        assert_eq!(state_of(TRUST_E_BAD_DIGEST), "invalid");
        assert_eq!(state_of(CERT_E_UNTRUSTEDROOT), "untrusted");
    }
}
