use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command};
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

/// Where the copy of a program that runs came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Next to the app: the setup program puts rg, fd and dust there, tauri-build next to every dev build.
    Bundled,
    /// Glint's own tools folder, where it unpacks the optional tools it downloads (tools.rs).
    Downloaded,
    /// Installed on the PC: winget's, Scoop's, Cargo's, Chocolatey's or one on PATH.
    Installed,
}

/// Where a program runs from (`rg`, or an optional tool such as `difft`). The copy that came with the
/// app wins; only when it's missing (a security tool quarantined it, a portable copy) does one Glint
/// downloaded or one installed on the PC run.
pub fn locate(name: &str) -> Option<(PathBuf, Origin)> {
    let tool = crate::tools::by_exe(name);
    find(name, tool.map_or(name, |t| t.package), tool.and_then(|t| crate::tools::folder_of(t.id)).as_deref())
}

fn find(name: &str, package: &str, downloaded: Option<&std::path::Path>) -> Option<(PathBuf, Origin)> {
    let file = format!("{name}.exe");
    let bundled = std::env::current_exe().ok()?.with_file_name(&file);
    if bundled.is_file() {
        return Some((bundled, Origin::Bundled));
    }
    if let Some(own) = downloaded.map(|d| d.join(&file)).filter(|p| p.is_file()) {
        return Some((own, Origin::Downloaded));
    }
    let env = |var: &str| std::env::var_os(var).map(PathBuf::from);
    // Relative PATH entries would resolve against wherever the app was started from.
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).filter(|d| d.is_absolute()).collect()).unwrap_or_default();
    let winget = env("LOCALAPPDATA").map(|d| d.join(r"Microsoft\WinGet"));
    dirs.extend(winget.iter().map(|w| w.join("Links")));
    dirs.extend(env("USERPROFILE").into_iter().flat_map(|home| [home.join(r"scoop\shims"), home.join(r".cargo\bin")]));
    dirs.extend(env("ProgramData").map(|d| d.join(r"chocolatey\bin")));
    if let Some(found) = dirs.iter().map(|d| d.join(&file)).find(|p| p.is_file()) {
        return Some((found, Origin::Installed));
    }
    in_winget_packages(&winget?.join("Packages"), package, &file).map(|p| (p, Origin::Installed))
}

/// A winget install this session hasn't seen on PATH yet: Packages\<id>\[<release>\[bin\]]rg.exe
/// (FFmpeg's builds keep theirs in bin).
fn in_winget_packages(packages: &std::path::Path, package: &str, file: &str) -> Option<PathBuf> {
    std::fs::read_dir(packages)
        .ok()?
        .flatten()
        .filter(|p| p.file_name().to_string_lossy().to_ascii_lowercase().contains(package))
        .flat_map(|p| std::iter::once(p.path()).chain(std::fs::read_dir(p.path()).into_iter().flatten().flatten().flat_map(|sub| [sub.path(), sub.path().join("bin")])))
        .map(|dir| dir.join(file))
        .find(|p| p.is_file())
}

/// A Windows job holding processes Glint started: closing it (dropping this) ends them, as Glint's own
/// end does, however it ends.
pub(crate) struct Job(windows::Win32::Foundation::HANDLE);

// The handle is only closed, once, by its owner.
unsafe impl Send for Job {}

impl Job {
    /// One that also holds its process to `memory` and to no child processes of its own: jq's, and
    /// Documents search's workers (doctext.rs).
    pub(crate) fn new(memory: usize) -> Result<Self, String> {
        use windows::Win32::System::JobObjects::*;
        unsafe {
            let job = Job(CreateJobObjectW(None, windows::core::PCWSTR::null()).map_err(|e| e.to_string())?);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            info.BasicLimitInformation.ActiveProcessLimit = 1;
            info.ProcessMemoryLimit = memory;
            SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32).map_err(|e| e.to_string())?;
            Ok(job)
        }
    }

    /// One that only ends what it holds: a send's or a share's (share.rs), which mustn't outlive Glint.
    pub(crate) fn ending() -> Result<Self, String> {
        use windows::Win32::System::JobObjects::*;
        unsafe {
            let job = Job(CreateJobObjectW(None, windows::core::PCWSTR::null()).map_err(|e| e.to_string())?);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(job.0, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32).map_err(|e| e.to_string())?;
            Ok(job)
        }
    }

    pub(crate) fn assign(&self, child: &Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        unsafe { windows::Win32::System::JobObjects::AssignProcessToJobObject(self.0, windows::Win32::Foundation::HANDLE(child.as_raw_handle())).map_err(|e| e.to_string()) }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub(crate) fn no_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = command;
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
        // FFmpeg's builds: Packages\Gyan.FFmpeg…\ffmpeg-9.0.2-full_build\bin\ffmpeg.exe.
        let bin = packages.join("Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe").join("ffmpeg-9.0.2-full_build").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("ffmpeg.exe"), "").unwrap();
        assert_eq!(in_winget_packages(&packages, "gyan.ffmpeg", "ffmpeg.exe"), Some(bin.join("ffmpeg.exe")));
        std::fs::remove_dir_all(&packages).unwrap();
    }
}
