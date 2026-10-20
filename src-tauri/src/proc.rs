use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};

/// Holds one running sidecar (the text search, or a tab's file search or folder sizing):
/// starting a new run stops the previous one.
#[derive(Clone, Default)]
pub struct Slot(Arc<Mutex<Option<Child>>>);

impl Slot {
    fn lock(&self) -> MutexGuard<'_, Option<Child>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stops whatever is running and tracks `child` instead; returns its pid.
    pub fn replace(&self, child: Child) -> u32 {
        let pid = child.id();
        if let Some(old) = self.lock().replace(child) {
            stop(old);
        }
        pid
    }

    /// Stops the current process, if any.
    pub fn kill(&self) {
        if let Some(child) = self.lock().take() {
            stop(child);
        }
    }

    /// Stops `pid` unless a newer run already replaced it.
    pub fn kill_if(&self, pid: u32) {
        let mut current = self.lock();
        if current.as_ref().map(Child::id) == Some(pid) {
            stop(current.take().unwrap());
        }
    }

    /// Reaps `pid` after it ended on its own, with its exit status. None if it was stopped or
    /// replaced instead.
    pub fn finish(&self, pid: u32) -> Option<std::process::ExitStatus> {
        let mut current = self.lock();
        if current.as_ref().map(Child::id) != Some(pid) {
            return None;
        }
        current.take().unwrap().wait().ok()
    }
}

/// A [`Slot`] per lane, for work each tab runs on its own (folder sizes): a new run stops only the
/// previous one in the same lane.
#[derive(Clone, Default)]
pub struct Lanes(Arc<Mutex<HashMap<String, Slot>>>);

impl Lanes {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The lane's slot, made on first use.
    pub fn slot(&self, lane: &str) -> Slot {
        self.lock().entry(lane.to_string()).or_default().clone()
    }

    /// Stops what runs in the lane and forgets it (a later run makes it again).
    pub fn close(&self, lane: &str) {
        let slot = self.lock().remove(lane);
        if let Some(slot) = slot {
            slot.kill();
        }
    }

    /// Stops every lane's work and forgets the lanes.
    pub fn close_all(&self) {
        let slots: Vec<Slot> = self.lock().drain().map(|(_, slot)| slot).collect();
        for slot in slots {
            slot.kill();
        }
    }
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// A bundled CLI tool (`rg`, `fd`, `dust`), run without a console window.
pub fn tool(name: &str) -> Result<Command, String> {
    let (exe, _) = locate(name).ok_or_else(|| format!("{name}.exe is missing next to the app; reinstall the app"))?;
    let mut command = Command::new(exe);
    no_window(&mut command);
    Ok(command)
}

/// Where a tool runs from, and whether it's the copy that came with the app. That one is next to the
/// app (the setup program puts it there, tauri-build next to every dev build). Only when it's missing
/// (a security tool quarantined it, a portable copy) does one installed on the PC run: winget's,
/// Scoop's, Cargo's, Chocolatey's or one on PATH.
pub fn locate(name: &str) -> Option<(PathBuf, bool)> {
    let file = format!("{name}.exe");
    let bundled = std::env::current_exe().ok()?.with_file_name(&file);
    if bundled.is_file() {
        return Some((bundled, true));
    }
    let env = |var: &str| std::env::var_os(var).map(PathBuf::from);
    // Relative PATH entries would resolve against wherever the app was started from.
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).filter(|d| d.is_absolute()).collect()).unwrap_or_default();
    let winget = env("LOCALAPPDATA").map(|d| d.join(r"Microsoft\WinGet"));
    dirs.extend(winget.iter().map(|w| w.join("Links")));
    dirs.extend(env("USERPROFILE").into_iter().flat_map(|home| [home.join(r"scoop\shims"), home.join(r".cargo\bin")]));
    dirs.extend(env("ProgramData").map(|d| d.join(r"chocolatey\bin")));
    if let Some(found) = dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file()) {
        return Some((found, false));
    }
    in_winget_packages(&winget?.join("Packages"), if name == "rg" { "ripgrep" } else { name }, &file).map(|p| (p, false))
}

/// A winget install this session hasn't seen on PATH yet: Packages\<id>\[<release>\]rg.exe.
fn in_winget_packages(packages: &std::path::Path, package: &str, file: &str) -> Option<PathBuf> {
    std::fs::read_dir(packages)
        .ok()?
        .flatten()
        .filter(|p| p.file_name().to_string_lossy().to_ascii_lowercase().contains(package))
        .flat_map(|p| std::iter::once(p.path()).chain(std::fs::read_dir(p.path()).into_iter().flatten().flatten().map(|sub| sub.path())))
        .map(|dir| dir.join(file))
        .find(|p| p.is_file())
}

fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = command;
}

/// Where Text search's ripgrep comes from: "bundled", "installed" on the PC, or "missing".
#[tauri::command]
pub fn ripgrep_status() -> &'static str {
    match locate("rg") {
        Some((_, true)) => "bundled",
        Some(_) => "installed",
        None => "missing",
    }
}

/// Installs ripgrep with winget, when the user asks to from the card Text mode shows without it.
#[tauri::command]
pub async fn install_ripgrep() -> Result<(), String> {
    crate::fsx::blocking(|| {
        let mut winget = Command::new("winget");
        winget
            .args(["install", "--id", "BurntSushi.ripgrep.MSVC", "--exact", "--source", "winget", "--silent", "--disable-interactivity"])
            .args(["--accept-package-agreements", "--accept-source-agreements"])
            .stdin(Stdio::null());
        no_window(&mut winget);
        let out = winget.output().map_err(|e| format!("winget isn't available on this PC ({e})"))?;
        if locate("rg").is_some() {
            return Ok(());
        }
        // winget's last words, past its progress bars.
        let said = String::from_utf8_lossy(&out.stdout).lines().map(str::trim).rfind(|l| l.chars().any(char::is_alphabetic)).map(str::to_string);
        Err(said.unwrap_or_else(|| format!("winget stopped with {}", out.status)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_what_winget_installed_without_path() {
        let packages = std::env::temp_dir().join(format!("glint-winget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&packages);
        let release = packages.join("BurntSushi.ripgrep.MSVC_Microsoft.Winget.Source_8wekyb3d8bbwe").join("ripgrep-15.2.0-x86_64-pc-windows-msvc");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::create_dir_all(packages.join("sharkdp.fd_Microsoft.Winget.Source_8wekyb3d8bbwe")).unwrap();
        assert_eq!(in_winget_packages(&packages, "ripgrep", "rg.exe"), None);
        std::fs::write(release.join("rg.exe"), "").unwrap();
        assert_eq!(in_winget_packages(&packages, "ripgrep", "rg.exe"), Some(release.join("rg.exe")));
        assert_eq!(in_winget_packages(&packages, "fd", "fd.exe"), None);
        std::fs::remove_dir_all(&packages).unwrap();
    }
}
