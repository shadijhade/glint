//! Kernel-assisted lock inspection ("Who is locking this?") via Windows Restart Manager (`rstrtmgr.dll`)
//! and Win32 process inspection. Identifies processes holding open handles to a file or files inside a
//! folder in a few milliseconds, with zero background overhead when idle.

use crate::fsx::blocking;
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockingProcess {
    pub pid: u32,
    pub name: String,
    pub service: Option<String>,
    /** "app", "service", "explorer", "console", or "critical". */
    pub kind: String,
    pub exe: Option<String>,
    pub stoppable: bool,
    pub start_time_low: u32,
    pub start_time_high: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockReport {
    pub paths: Vec<String>,
    pub processes: Vec<LockingProcess>,
    pub files_checked: usize,
    pub elapsed_ms: f64,
}

#[tauri::command]
pub async fn who_locks(paths: Vec<String>) -> Result<LockReport, String> {
    blocking(move || imp::who_locks(paths)).await
}

#[tauri::command]
pub async fn stop_locking_process(pid: u32, start_time_low: u32, start_time_high: u32) -> Result<(), String> {
    blocking(move || imp::stop_locking_process(pid, start_time_low, start_time_high)).await
}

#[cfg(windows)]
mod imp {
    use super::{LockReport, LockingProcess};
    use std::collections::{HashMap, VecDeque};
    use std::ffi::OsStr;
    use std::fs;
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::time::Instant;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_MORE_DATA, ERROR_SUCCESS, FILETIME};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };

    const CCH_RM_SESSION_KEY: usize = 32;
    const CCH_RM_MAX_APP_NAME: usize = 255;
    const CCH_RM_MAX_SVC_NAME: usize = 63;

    const RM_MAIN_WINDOW: u32 = 1;
    const RM_OTHER_WINDOW: u32 = 2;
    const RM_SERVICE: u32 = 3;
    const RM_EXPLORER: u32 = 4;
    const RM_CONSOLE: u32 = 5;
    const RM_CRITICAL: u32 = 1000;

    /// Maximum files sampled inside folders so inspecting a large folder stays interactive (<25 ms).
    const MAX_FOLDER_FILES: usize = 512;
    /// Maximum files passed per RmRegisterResources call.
    const RM_BATCH_SIZE: usize = 128;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct RmUniqueProcess {
        dw_process_id: u32,
        process_start_time: FILETIME,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RmProcessInfo {
        process: RmUniqueProcess,
        str_app_name: [u16; CCH_RM_MAX_APP_NAME + 1],
        str_service_short_name: [u16; CCH_RM_MAX_SVC_NAME + 1],
        application_type: u32,
        app_status: u32,
        ts_session_id: u32,
        b_restartable: i32,
    }

    impl Default for RmProcessInfo {
        fn default() -> Self {
            Self {
                process: RmUniqueProcess::default(),
                str_app_name: [0; CCH_RM_MAX_APP_NAME + 1],
                str_service_short_name: [0; CCH_RM_MAX_SVC_NAME + 1],
                application_type: 0,
                app_status: 0,
                ts_session_id: 0,
                b_restartable: 0,
            }
        }
    }

    #[link(name = "rstrtmgr")]
    extern "system" {
        fn RmStartSession(p_session_handle: *mut u32, dw_session_flags: u32, str_session_key: *mut u16) -> u32;
        fn RmEndSession(dw_session_handle: u32) -> u32;
        fn RmRegisterResources(
            dw_session_handle: u32,
            n_files: u32,
            rgs_file_names: *const *const u16,
            n_applications: u32,
            rg_applications: *const RmUniqueProcess,
            n_services: u32,
            rgs_service_names: *const *const u16,
        ) -> u32;
        fn RmGetList(
            dw_session_handle: u32,
            pn_proc_info_needed: *mut u32,
            pn_proc_info: *mut u32,
            rg_affected_apps: *mut RmProcessInfo,
            lpdw_reboot_reasons: *mut u32,
        ) -> u32;
    }

    struct RmSession(u32);

    impl RmSession {
        fn start() -> Result<Self, String> {
            let mut handle = 0u32;
            let mut key = [0u16; CCH_RM_SESSION_KEY + 1];
            let rc = unsafe { RmStartSession(&mut handle, 0, key.as_mut_ptr()) };
            if rc != ERROR_SUCCESS.0 {
                return Err(format!("Couldn't start Windows Restart Manager session (error {rc})"));
            }
            Ok(Self(handle))
        }
    }

    impl Drop for RmSession {
        fn drop(&mut self) {
            unsafe {
                let _ = RmEndSession(self.0);
            }
        }
    }

    fn wide_nul(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }

    fn from_wide_nul(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end]).trim().to_string()
    }

    fn query_process_exe(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut size = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size);
            let _ = CloseHandle(handle);
            ok.ok()?;
            if size == 0 {
                None
            } else {
                Some(String::from_utf16_lossy(&buf[..size as usize]))
            }
        }
    }

    /// Priority boost for files inside a folder that are most likely to hold locks (.exe, .dll, .db, etc.).
    fn lock_priority(path: &Path) -> u8 {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("exe" | "dll" | "sys" | "ocx" | "node" | "pyd") => 0,
            Some("db" | "sqlite" | "sqlite3" | "wal" | "shm" | "lock" | "lck" | "log") => 1,
            _ => 2,
        }
    }

    /// Expands target paths into concrete file paths for `RmRegisterResources`.
    /// For directories, performs a bounded breadth-first walk so open files inside the folder are detected.
    fn collect_candidate_files(targets: &[String]) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut queue = VecDeque::new();

        for raw in targets {
            let p = PathBuf::from(raw);
            if p.is_dir() {
                queue.push_back((p, 0usize));
            } else if p.exists() {
                files.push(p);
            }
        }

        let mut discovered = Vec::new();
        while let Some((dir, depth)) = queue.pop_front() {
            if files.len() + discovered.len() >= MAX_FOLDER_FILES * 2 {
                break;
            }
            let Ok(read_dir) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let path = entry.path();
                let Ok(ft) = entry.file_type() else {
                    continue;
                };
                if ft.is_file() {
                    discovered.push(path);
                } else if ft.is_dir() && !ft.is_symlink() && depth < 4 && queue.len() < 128 {
                    queue.push_back((path, depth + 1));
                }
            }
        }

        discovered.sort_by_key(|p| lock_priority(p));
        let remaining = MAX_FOLDER_FILES.saturating_sub(files.len());
        files.extend(discovered.into_iter().take(remaining));
        files
    }

    fn kind_label(app_type: u32) -> &'static str {
        match app_type {
            RM_MAIN_WINDOW | RM_OTHER_WINDOW => "app",
            RM_SERVICE => "service",
            RM_EXPLORER => "explorer",
            RM_CONSOLE => "console",
            RM_CRITICAL => "critical",
            _ => "app",
        }
    }

    fn kind_rank(kind: &str) -> u8 {
        match kind {
            "app" => 0,
            "console" => 1,
            "explorer" => 2,
            "service" => 3,
            _ => 4,
        }
    }

    pub fn who_locks(paths: Vec<String>) -> Result<LockReport, String> {
        let start = Instant::now();
        if paths.is_empty() {
            return Ok(LockReport {
                paths,
                processes: Vec::new(),
                files_checked: 0,
                elapsed_ms: 0.0,
            });
        }

        let candidates = collect_candidate_files(&paths);
        let files_checked = candidates.len();
        if candidates.is_empty() {
            return Ok(LockReport {
                paths,
                processes: Vec::new(),
                files_checked: 0,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            });
        }

        let session = RmSession::start()?;
        let wide_files: Vec<Vec<u16>> = candidates.iter().map(|p| wide_nul(p.as_os_str())).collect();

        for chunk in wide_files.chunks(RM_BATCH_SIZE) {
            let ptrs: Vec<*const u16> = chunk.iter().map(|w| w.as_ptr()).collect();
            let rc = unsafe {
                RmRegisterResources(
                    session.0,
                    ptrs.len() as u32,
                    ptrs.as_ptr(),
                    0,
                    ptr::null(),
                    0,
                    ptr::null(),
                )
            };
            if rc != ERROR_SUCCESS.0 {
                return Err(format!("Couldn't register resources with Restart Manager (error {rc})"));
            }
        }

        let mut needed = 0u32;
        let mut count = 0u32;
        let mut reasons = 0u32;
        let mut infos: Vec<RmProcessInfo> = Vec::new();

        for _ in 0..4 {
            let rc = unsafe {
                RmGetList(
                    session.0,
                    &mut needed,
                    &mut count,
                    if infos.is_empty() { ptr::null_mut() } else { infos.as_mut_ptr() },
                    &mut reasons,
                )
            };
            if rc == ERROR_SUCCESS.0 {
                infos.truncate(count as usize);
                break;
            } else if rc == ERROR_MORE_DATA.0 {
                count = needed.max(1);
                infos.resize(count as usize, RmProcessInfo::default());
            } else {
                return Err(format!("Couldn't query locking processes (error {rc})"));
            }
        }

        let self_pid = std::process::id();
        let mut by_pid: HashMap<u32, LockingProcess> = HashMap::new();

        for info in infos {
            let pid = info.process.dw_process_id;
            if pid == 0 || pid == self_pid || by_pid.contains_key(&pid) {
                continue;
            }
            let exe = query_process_exe(pid);
            let mut name = from_wide_nul(&info.str_app_name);
            if name.is_empty() {
                name = exe
                    .as_deref()
                    .and_then(|p| Path::new(p).file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("Process")
                    .to_string();
            }
            let svc = from_wide_nul(&info.str_service_short_name);
            let service = if svc.is_empty() { None } else { Some(svc) };
            let kind = kind_label(info.application_type).to_string();
            let stoppable = info.application_type != RM_CRITICAL;

            by_pid.insert(
                pid,
                LockingProcess {
                    pid,
                    name,
                    service,
                    kind,
                    exe,
                    stoppable,
                    start_time_low: info.process.process_start_time.dwLowDateTime,
                    start_time_high: info.process.process_start_time.dwHighDateTime,
                },
            );
        }

        let mut processes: Vec<LockingProcess> = by_pid.into_values().collect();
        processes.sort_by(|a, b| {
            kind_rank(&a.kind)
                .cmp(&kind_rank(&b.kind))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.pid.cmp(&b.pid))
        });

        Ok(LockReport {
            paths,
            processes,
            files_checked,
            elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
        })
    }

    pub fn stop_locking_process(pid: u32, start_time_low: u32, start_time_high: u32) -> Result<(), String> {
        if pid == 0 || pid == std::process::id() {
            return Err("Refusing to stop this process".into());
        }
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .map_err(|e| format!("Couldn't open process {pid}: {}", e.message()))?;

            // Verify process creation time so a recycled PID is never terminated by mistake.
            if start_time_low != 0 || start_time_high != 0 {
                let mut created = FILETIME::default();
                let mut exited = FILETIME::default();
                let mut kernel = FILETIME::default();
                let mut user = FILETIME::default();
                if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user).is_ok()
                    && (created.dwLowDateTime != start_time_low || created.dwHighDateTime != start_time_high)
                {
                    let _ = CloseHandle(handle);
                    return Err("The process already exited".into());
                }
            }

            let result = TerminateProcess(handle, 1).map_err(|e| format!("Couldn't stop process {pid}: {}", e.message()));
            let _ = CloseHandle(handle);
            result
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::process::{Command, Stdio};

        #[test]
        fn detects_process_locking_file() {
            let dir = std::env::temp_dir().join(format!("glint-lock-test-{}", std::process::id()));
            let _ = fs::create_dir_all(&dir);
            let file = dir.join("locked.txt");
            fs::write(&file, b"glint lock test").unwrap();

            // Spawn a child powershell/cmd process that holds an open handle to `locked.txt`.
            let child = Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &format!(
                        "$f = [System.IO.File]::Open('{}', 'Open', 'ReadWrite', 'None'); Start-Sleep -Seconds 30",
                        file.display()
                    ),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();

            if let Ok(mut child) = child {
                let child_pid = child.id();
                // Poll briefly until the child opens the file handle.
                let mut found = false;
                for _ in 0..25 {
                    let report = who_locks(vec![file.to_string_lossy().into_owned()]).unwrap();
                    if report.processes.iter().any(|p| p.pid == child_pid) {
                        found = true;
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_dir_all(&dir);
                assert!(found, "Restart Manager should detect child PID {child_pid} holding locked.txt");
            } else {
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{LockReport, LockingProcess};

    pub fn who_locks(paths: Vec<String>) -> Result<LockReport, String> {
        Ok(LockReport {
            paths,
            processes: Vec::<LockingProcess>::new(),
            files_checked: 0,
            elapsed_ms: 0.0,
        })
    }

    pub fn stop_locking_process(_: u32, _: u32, _: u32) -> Result<(), String> {
        Err("Only available on Windows".into())
    }
}
