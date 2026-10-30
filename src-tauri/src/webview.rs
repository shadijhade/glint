//! WebView2, which draws Glint's window. Windows 11 has it and Windows 10 gets it with Windows Update, but
//! a PC can be without it: Windows 10 LTSC and Windows Server, one it was removed from, or a copy of Glint
//! no installer put there (Scoop's, a copied folder). Glint can't open its window then, so before anything
//! else it says so and offers to install it: Microsoft's own installer, downloaded and run once Microsoft's
//! signature on it holds, which fetches the rest and shows how it's going.

/// Microsoft's page for WebView2, and its installer (the Evergreen bootstrapper).
#[cfg(windows)]
const PAGE: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
#[cfg(windows)]
const INSTALLER: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// Whether Glint can open its window: WebView2 is there, or has just been installed. Otherwise the person
/// has been told how to get it, and Glint ends.
#[cfg(windows)]
pub fn ready() -> bool {
    if installed() {
        return true;
    }
    if !ask("Glint needs Microsoft Edge WebView2 to show its window, and this PC doesn't have it.\n\nInstall it now? Glint downloads Microsoft's installer, which fetches the rest and shows how it's going.") {
        open(PAGE);
        return false;
    }
    match install() {
        Ok(()) if installed() => true,
        Ok(()) => {
            tell("WebView2 is installed, but Glint can't find it yet. Start Glint again in a moment.");
            false
        }
        Err(e) => {
            tell(&format!("WebView2 couldn't be installed: {e}.\n\nGlint opens Microsoft's page for it, to install it from there."));
            open(PAGE);
            false
        }
    }
}

#[cfg(not(windows))]
pub fn ready() -> bool {
    true
}

/// Whether WebView2's runtime is there, as its loader finds it (the way it's found to open the window).
#[cfg(windows)]
fn installed() -> bool {
    use webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString;
    use windows::core::{PCWSTR, PWSTR};
    let mut version = PWSTR::null();
    let found = unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) }.is_ok() && !version.is_null();
    if !version.is_null() {
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(version.0 as *const _)) };
    }
    found
}

/// Downloads Microsoft's WebView2 installer and runs it, once Microsoft's signature on it holds, until it's
/// done.
#[cfg(windows)]
fn install() -> Result<(), String> {
    let file = std::env::temp_dir().join(format!("MicrosoftEdgeWebview2Setup-{}.exe", std::process::id()));
    let downloaded = tauri::async_runtime::block_on(async {
        // The TLS the updater uses; installing it twice is harmless.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .user_agent(concat!("Glint/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client.get(INSTALLER).send().await.and_then(|r| r.error_for_status()).map_err(|e| format!("Glint couldn't download its installer ({e})"))?;
        let bytes = response.bytes().await.map_err(|e| format!("the download of its installer broke off ({e})"))?;
        if bytes.len() > 64 << 20 {
            return Err("its installer's download is bigger than expected".to_string());
        }
        std::fs::write(&file, &bytes).map_err(|e| e.to_string())
    });
    if let Err(e) = downloaded {
        let _ = std::fs::remove_file(&file);
        return Err(e);
    }
    let signature = crate::sign::verify(&file.to_string_lossy());
    if signature.state != "valid" || !signature.signer.as_deref().is_some_and(|signer| signer.starts_with("Microsoft")) {
        let _ = std::fs::remove_file(&file);
        return Err("what was downloaded isn't signed by Microsoft, so it wasn't run".into());
    }
    let ran = std::process::Command::new(&file).arg("/install").status();
    let _ = std::fs::remove_file(&file);
    match ran.map_err(|e| e.to_string())? {
        status if status.success() => Ok(()),
        status => Err(format!("its installer stopped with {status}")),
    }
}

#[cfg(windows)]
fn ask(text: &str) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONINFORMATION, MB_SETFOREGROUND, MB_YESNO};
    message(text, MB_YESNO | MB_ICONINFORMATION | MB_SETFOREGROUND) == IDYES
}

#[cfg(windows)]
fn tell(text: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MB_SETFOREGROUND};
    message(text, MB_OK | MB_ICONWARNING | MB_SETFOREGROUND);
}

#[cfg(windows)]
fn message(text: &str, style: windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_STYLE) -> windows::Win32::UI::WindowsAndMessaging::MESSAGEBOX_RESULT {
    use windows::core::HSTRING;
    unsafe { windows::Win32::UI::WindowsAndMessaging::MessageBoxW(None, &HSTRING::from(text), &HSTRING::from("Glint"), style) }
}

#[cfg(windows)]
fn open(url: &str) {
    use windows::core::{w, HSTRING};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL) };
}
