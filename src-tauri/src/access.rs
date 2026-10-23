//! Get access: into a folder Windows keeps this user out of (C:\PerfLogs, another account's profile),
//! as File Explorer's Continue button does it. Their account is given read access to the folder and
//! what's in it, for good (see shadui_index::access); Windows asks for administrator approval unless
//! the folder is theirs to change.

use crate::fsx::{self, blocking};
use tauri::Window;

/// "granted": the folder opens now. "refused": even administrators may not change who opens it
/// (System Volume Information, WindowsApps). "stillDenied": the account may open it now, yet a rule in
/// its permissions denies it. Rejects with "cancelled" when the approval was declined.
#[tauri::command]
pub async fn grant_access(window: Window, path: String) -> Result<&'static str, String> {
    #[cfg(windows)]
    {
        use shadui_index::access::{self, Failure};
        let owner = window.hwnd().ok().map(|h| h.0 as isize);
        blocking(move || {
            let path = fsx::normalize(&path)?;
            match access::request(crate::index::bundled, &path, owner) {
                Ok(()) => Ok(if fsx::probe(&path).is_ok() { "granted" } else { "stillDenied" }),
                Err(Failure::Refused) => Ok("refused"),
                Err(Failure::Cancelled) => Err("cancelled".into()),
                // Something inside couldn't be changed: the folder opens, and that item says so when opened.
                Err(_) if fsx::probe(&path).is_ok() => Ok("granted"),
                Err(failure) => Err(failure.to_string()),
            }
        })
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = (window, path);
        Err("Getting access to a folder needs Windows".into())
    }
}
