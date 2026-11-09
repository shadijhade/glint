//! Run in Windows Sandbox: a program (or any file) opened in a fresh, throwaway Windows that's gone,
//! with all it did, the moment its window closes. Only a copy of the file goes in, in a folder of its
//! own shared read-only, so nothing else on this PC is in reach; the network is off unless asked for.
//! Windows Sandbox comes with Windows 11 Pro, Enterprise and Education (Settings turns it on once).

use crate::fsx::blocking;
use std::path::{Path, PathBuf};

/// Windows Sandbox's own program, where it's installed: the Windows feature's, or the Microsoft Store's.
fn sandbox_exe() -> Option<PathBuf> {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let feature = system.join(r"System32\WindowsSandbox.exe");
    if feature.exists() {
        return Some(feature);
    }
    // The Store's Windows Sandbox puts an app alias for wsb in the user's WindowsApps folder.
    let alias = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?.join(r"Microsoft\WindowsApps\WindowsSandbox.exe");
    alias.exists().then_some(alias)
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The configuration: the copy's folder shared read-only on the sandbox's desktop, the file opened
/// as the sandbox starts, the network on or off, and no clipboard shared with this PC.
fn config(folder: &Path, file: &str, network: bool) -> String {
    let inside = r"C:\Users\WDAGUtilityAccount\Desktop\From Glint";
    let program = Path::new(file).extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "exe" | "msi" | "bat" | "cmd" | "com"));
    let command = if program { format!(r#"cmd.exe /c start "" "{inside}\{file}""#) } else { format!(r#"explorer.exe "{inside}\{file}""#) };
    format!(
        "<Configuration>\n  <Networking>{}</Networking>\n  <ClipboardRedirection>Disable</ClipboardRedirection>\n  <PrinterRedirection>Disable</PrinterRedirection>\n  <MappedFolders>\n    <MappedFolder>\n      <HostFolder>{}</HostFolder>\n      <SandboxFolder>{inside}</SandboxFolder>\n      <ReadOnly>true</ReadOnly>\n    </MappedFolder>\n  </MappedFolders>\n  <LogonCommand>\n    <Command>{}</Command>\n  </LogonCommand>\n</Configuration>\n",
        if network { "Default" } else { "Disable" },
        xml(&folder.to_string_lossy()),
        xml(&command),
    )
}

/// Opens a copy of `path` in Windows Sandbox. Rejects with "notInstalled" where Windows has no
/// Sandbox, or it's off.
pub async fn sandbox_run(path: String, network: bool) -> Result<(), String> {
    blocking(move || {
        let exe = sandbox_exe().ok_or("notInstalled")?;
        let source = Path::new(&path);
        let name = source.file_name().ok_or("Not a file")?.to_string_lossy().into_owned();
        let folder = std::env::temp_dir().join("Glint").join(format!("sandbox-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())));
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        std::fs::copy(source, folder.join(&name)).map_err(|e| format!("Couldn't copy it for the sandbox: {e}"))?;
        let wsb = folder.with_extension("wsb");
        std::fs::write(&wsb, config(&folder, &name, network)).map_err(|e| e.to_string())?;
        let mut command = std::process::Command::new(exe);
        command.arg(&wsb);
        command.spawn().map(drop).map_err(|e| format!("Couldn't start Windows Sandbox: {e}"))
    })
    .await
}

/// Whether Windows Sandbox is there to run things in.
pub fn sandbox_available() -> bool {
    sandbox_exe().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_only_the_copy_read_only_and_cuts_the_network() {
        let wsb = config(Path::new(r"C:\Temp\Glint\sandbox-1"), "setup & go.exe", false);
        assert!(wsb.contains("<Networking>Disable</Networking>"));
        assert!(wsb.contains(r"<HostFolder>C:\Temp\Glint\sandbox-1</HostFolder>"));
        assert!(wsb.contains("<ReadOnly>true</ReadOnly>"));
        assert!(wsb.contains("setup &amp; go.exe"), "{wsb}");
        assert!(wsb.contains("cmd.exe /c start"));
        assert!(config(Path::new(r"C:\x"), "report.pdf", true).contains("explorer.exe") && config(Path::new(r"C:\x"), "report.pdf", true).contains("<Networking>Default</Networking>"));
    }
}
