use std::collections::HashMap;
use std::process::Child;
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

    /// Reaps `pid` after it ended on its own. Returns false if it was stopped or replaced instead.
    pub fn finish(&self, pid: u32) -> bool {
        let mut current = self.lock();
        if current.as_ref().map(Child::id) != Some(pid) {
            return false;
        }
        let _ = current.take().unwrap().wait();
        true
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

/// Resolves a bundled CLI tool (`rg`, `fd`, `dust`): prefers the sidecar next to the executable,
/// then `src-tauri/binaries` (for `cargo run` builds), then `PATH` and standard Windows package
/// manager locations (WinGet, Scoop, Cargo, Chocolatey) so a system-installed copy always works too.
pub fn tool(window: &tauri::Window, name: &str) -> Result<std::process::Command, String> {
    use std::path::PathBuf;
    use tauri_plugin_shell::ShellExt;

    let bin_name = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let triple_name = if cfg!(windows) {
        format!("{name}-x86_64-pc-windows-msvc.exe")
    } else {
        name.to_string()
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join(&bin_name).is_file() || dir.join(&triple_name).is_file() {
                if let Ok(sidecar) = window.shell().sidecar(name) {
                    return Ok(sidecar.into());
                }
            }
            for rel in [
                PathBuf::from(&bin_name),
                PathBuf::from(&triple_name),
                PathBuf::from("binaries").join(&triple_name),
                PathBuf::from("..").join("..").join("binaries").join(&triple_name),
            ] {
                let candidate = dir.join(rel);
                if candidate.is_file() {
                    return Ok(no_window(std::process::Command::new(candidate)));
                }
            }
        }
    }

    if let Some(found) = find_installed(&bin_name, name) {
        return Ok(no_window(std::process::Command::new(found)));
    }

    if let Ok(sidecar) = window.shell().sidecar(name) {
        return Ok(sidecar.into());
    }

    Ok(no_window(std::process::Command::new(&bin_name)))
}

fn find_installed(bin_name: &str, name: &str) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();

    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        dirs.push(local.join("Glint"));
        dirs.push(local.join("Programs").join("Glint"));
        dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        dirs.push(home.join("scoop").join("shims"));
        dirs.push(home.join("scoop").join("apps").join(name).join("current"));
        if name == "rg" {
            dirs.push(home.join("scoop").join("apps").join("ripgrep").join("current"));
        }
        dirs.push(home.join(".cargo").join("bin"));
    }
    if let Some(prog_data) = std::env::var_os("ProgramData").map(PathBuf::from) {
        dirs.push(prog_data.join("chocolatey").join("bin"));
    }

    for dir in dirs {
        let candidate = dir.join(bin_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Also scan WinGet Packages directory in case WinGet installed the package without refreshing PATH.
    if let Some(pkgs) = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|l| l.join("Microsoft").join("WinGet").join("Packages"))
    {
        let needle = if name == "rg" { "ripgrep" } else { name };
        if let Ok(entries) = std::fs::read_dir(pkgs) {
            for entry in entries.flatten() {
                let pkg_dir = entry.path();
                let folder = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if !folder.contains(needle) && !folder.contains(name) {
                    continue;
                }
                let direct = pkg_dir.join(bin_name);
                if direct.is_file() {
                    return Some(direct);
                }
                if let Ok(subs) = std::fs::read_dir(&pkg_dir) {
                    for sub in subs.flatten() {
                        let nested = sub.path().join(bin_name);
                        if nested.is_file() {
                            return Some(nested);
                        }
                    }
                }
            }
        }
    }

    None
}

fn no_window(mut cmd: std::process::Command) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

