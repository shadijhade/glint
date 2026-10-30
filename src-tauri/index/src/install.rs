//! Turning instant search on and off (run elevated, once): the service executable is copied to
//! Program Files, where only administrators can change it (a LocalSystem service must never run a
//! file its users could replace), registered to start with Windows, and told who may query it.

use crate::service::{allowed_users, parameters_key};
use crate::store;
use crate::win::{self, known_folder, os_error, wide, Key};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_NOT_ACTIVE};
use windows::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService, CreateServiceW, DeleteService, OpenSCManagerW,
    OpenServiceW, QueryServiceStatus, StartServiceW, SC_ACTION, SC_ACTION_NONE, SC_ACTION_RESTART, SC_HANDLE, SC_MANAGER_ALL_ACCESS,
    SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_CONFIG_FAILURE_ACTIONS, SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO, SERVICE_CONTROL_STOP, SERVICE_DELAYED_AUTO_START_INFO,
    SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL, SERVICE_FAILURE_ACTIONSW, SERVICE_REQUIRED_PRIVILEGES_INFOW, SERVICE_RUNNING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STOPPED, SERVICE_WIN32_OWN_PROCESS,
};
use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_DELAY_UNTIL_REBOOT, MOVEFILE_REPLACE_EXISTING};
use windows::Win32::UI::Shell::FOLDERID_ProgramFiles;

const DESCRIPTION: &str = "Keeps the names and sizes of the files on your NTFS drives in memory so Glint can search them \
                           and size folders instantly. Turn it off in Glint's settings.";

/// The step that failed, reported to the app as the process exit code (step << 16 | Windows error).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    NotElevated = 2,
    Copy = 3,
    Register = 4,
    Start = 5,
    Remove = 6,
}

#[derive(Debug)]
pub struct Failure(pub Step, pub io::Error);

impl Failure {
    pub fn exit_code(&self) -> i32 {
        ((self.0 as i32) << 16) | (self.1.raw_os_error().unwrap_or(0) & 0xFFFF)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", describe(self.0 as u32), self.1)
    }
}

/// What failed, from an exit code's step.
pub(crate) fn describe(step: u32) -> &'static str {
    match step {
        2 => "Administrator rights are needed",
        3 => "Couldn't copy the service to Program Files",
        4 => "Couldn't register the service",
        5 => "The service didn't start",
        6 => "Couldn't remove the service",
        _ => "Something went wrong",
    }
}

struct Sc(SC_HANDLE);

impl Drop for Sc {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

fn open_manager() -> io::Result<Sc> {
    Ok(Sc(unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ALL_ACCESS) }.map_err(os_error)?))
}

fn open_service(manager: &Sc) -> io::Result<Option<Sc>> {
    match unsafe { OpenServiceW(manager.0, PCWSTR(wide(crate::SERVICE_NAME).as_ptr()), SERVICE_ALL_ACCESS) } {
        Ok(service) => Ok(Some(Sc(service))),
        Err(e) if e.code() == ERROR_SERVICE_DOES_NOT_EXIST.to_hresult() => Ok(None),
        Err(e) => Err(os_error(e)),
    }
}

fn state(service: &Sc) -> io::Result<SERVICE_STATUS_CURRENT_STATE> {
    let mut status = SERVICE_STATUS::default();
    unsafe { QueryServiceStatus(service.0, &mut status) }.map_err(os_error)?;
    Ok(status.dwCurrentState)
}

fn wait_for(service: &Sc, wanted: SERVICE_STATUS_CURRENT_STATE, timeout: Duration) -> io::Result<()> {
    let started = Instant::now();
    while state(service)? != wanted {
        if started.elapsed() > timeout {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the service didn't respond in time"));
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn stop(service: &Sc) -> io::Result<()> {
    let mut status = SERVICE_STATUS::default();
    match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_SERVICE_NOT_ACTIVE.to_hresult() => return Ok(()),
        Err(e) => return Err(os_error(e)),
    }
    wait_for(service, SERVICE_STOPPED, Duration::from_secs(60))
}

/// Where the installed service lives.
pub fn install_dir() -> io::Result<PathBuf> {
    Ok(known_folder(&FOLDERID_ProgramFiles)?.join("ShadUI Ripgrep"))
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((fs::canonicalize(a), fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
}

/// Copies, retrying while a just-stopped service's executable is still being released.
fn copy(from: &Path, to: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) if tries < 50 && e.raw_os_error() == Some(32) => {
                tries += 1;
                thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Installs (or updates) the service and starts it. `allow` users are added to those who may use it.
/// `package` is the Microsoft Store copy's package family, when that's what installs it: the service
/// then goes once that package does (store.rs). Any other install clears it, as that copy relies on
/// the service too.
pub fn install(allow: &[String], package: Option<&str>) -> Result<(), Failure> {
    let fail = |step| move |e: io::Error| Failure(step, e);
    if !win::is_elevated() {
        return Err(Failure(Step::NotElevated, io::Error::from_raw_os_error(5)));
    }
    let manager = open_manager().map_err(fail(Step::Register))?;
    let existing = open_service(&manager).map_err(fail(Step::Register))?;
    if let Some(service) = &existing {
        stop(service).map_err(fail(Step::Copy))?;
    }

    let dir = install_dir().map_err(fail(Step::Copy))?;
    let target = dir.join(crate::SERVICE_EXE);
    let exe = std::env::current_exe().map_err(fail(Step::Copy))?;
    fs::create_dir_all(&dir).map_err(fail(Step::Copy))?;
    if !same_file(&exe, &target) {
        copy(&exe, &target).map_err(fail(Step::Copy))?;
    }

    // Quoted, so a path with spaces can't be read as another program.
    let command = wide(format!("\"{}\" service", target.display()));
    let name = wide(crate::SERVICE_NAME);
    let display = wide(crate::SERVICE_DISPLAY_NAME);
    let service = match existing {
        Some(service) => {
            unsafe {
                ChangeServiceConfigW(
                    service.0,
                    SERVICE_WIN32_OWN_PROCESS,
                    SERVICE_AUTO_START,
                    SERVICE_ERROR_NORMAL,
                    PCWSTR(command.as_ptr()),
                    PCWSTR::null(),
                    None,
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR::null(),
                    PCWSTR(display.as_ptr()),
                )
            }
            .map_err(|e| Failure(Step::Register, os_error(e)))?;
            service
        }
        None => Sc(unsafe {
            CreateServiceW(
                manager.0,
                PCWSTR(name.as_ptr()),
                PCWSTR(display.as_ptr()),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                PCWSTR(command.as_ptr()),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(), // LocalSystem: reading volumes needs it
                PCWSTR::null(),
            )
        }
        .map_err(|e| Failure(Step::Register, os_error(e)))?),
    };
    configure(&service).map_err(fail(Step::Register))?;

    // Without --allow-sid (the NSIS installer's /INSTANTSEARCH), whoever approved the prompt may use it.
    let me = win::current_user_sid().ok();
    let allow = if allow.is_empty() { me.as_slice() } else { allow };
    let mut users = allowed_users();
    for sid in allow {
        if win::is_sid(sid) && !users.contains(sid) {
            users.push(sid.clone());
        }
    }
    let parameters = Key::create(&parameters_key(), true).map_err(fail(Step::Register))?;
    parameters.set_strings("AllowedUsers", &users).map_err(fail(Step::Register))?;
    match package.filter(|family| store::split(family).is_some()) {
        Some(family) => parameters.set_string(store::VALUE, family),
        None => parameters.delete(store::VALUE).map(drop),
    }
    .map_err(fail(Step::Register))?;

    match unsafe { StartServiceW(service.0, None) } {
        Ok(()) => {}
        Err(e) if e.code() == ERROR_SERVICE_ALREADY_RUNNING.to_hresult() => {}
        Err(e) => return Err(Failure(Step::Start, os_error(e))),
    }
    wait_for(&service, SERVICE_RUNNING, Duration::from_secs(30)).map_err(fail(Step::Start))
}

/// Description, a delayed start (so it never slows down booting), restarts after a crash, and only
/// the privileges it uses.
fn configure(service: &Sc) -> io::Result<()> {
    let mut description = wide(DESCRIPTION);
    let info = SERVICE_DESCRIPTIONW { lpDescription: PWSTR(description.as_mut_ptr()) };
    unsafe { ChangeServiceConfig2W(service.0, SERVICE_CONFIG_DESCRIPTION, Some(&info as *const _ as *const _)) }.map_err(os_error)?;

    let delayed = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: true.into() };
    unsafe { ChangeServiceConfig2W(service.0, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, Some(&delayed as *const _ as *const _)) }
        .map_err(os_error)?;

    let mut actions = [
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 5_000 },
        SC_ACTION { Type: SC_ACTION_RESTART, Delay: 60_000 },
        SC_ACTION { Type: SC_ACTION_NONE, Delay: 0 },
    ];
    let failure = SERVICE_FAILURE_ACTIONSW { dwResetPeriod: 86_400, cActions: actions.len() as u32, lpsaActions: actions.as_mut_ptr(), ..Default::default() };
    unsafe { ChangeServiceConfig2W(service.0, SERVICE_CONFIG_FAILURE_ACTIONS, Some(&failure as *const _ as *const _)) }.map_err(os_error)?;

    // Opening a volume takes membership in Administrators (LocalSystem has it), not a privilege.
    // Everything else LocalSystem holds (debug, impersonate, load driver, act as the OS…) is dropped.
    let mut privileges: Vec<u16> = Vec::new();
    for privilege in ["SeChangeNotifyPrivilege", "SeBackupPrivilege", "SeManageVolumePrivilege"] {
        privileges.extend(privilege.encode_utf16());
        privileges.push(0);
    }
    privileges.push(0);
    let required = SERVICE_REQUIRED_PRIVILEGES_INFOW { pmszRequiredPrivileges: PWSTR(privileges.as_mut_ptr()) };
    unsafe { ChangeServiceConfig2W(service.0, SERVICE_CONFIG_REQUIRED_PRIVILEGES_INFO, Some(&required as *const _ as *const _)) }
        .map_err(os_error)
}

/// Stops and removes the service, its snapshots and the installed executable.
pub fn uninstall() -> Result<(), Failure> {
    let fail = |e: io::Error| Failure(Step::Remove, e);
    if !win::is_elevated() {
        return Err(Failure(Step::NotElevated, io::Error::from_raw_os_error(5)));
    }
    let manager = open_manager().map_err(fail)?;
    if let Some(service) = open_service(&manager).map_err(fail)? {
        stop(&service).map_err(fail)?;
        unsafe { DeleteService(service.0) }.map_err(|e| fail(os_error(e)))?;
    }
    // The snapshots hold every file name: they go too.
    if let Ok(data) = win::data_dir() {
        let _ = fs::remove_dir_all(&data);
        if let Some(parent) = data.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
    let dir = install_dir().map_err(fail)?;
    let installed = dir.join(crate::SERVICE_EXE);
    let exe = std::env::current_exe().map_err(fail)?;
    if installed.exists() && !same_file(&exe, &installed) {
        // The process that just stopped may still hold the file for a moment.
        let mut tries = 0;
        while let Err(e) = fs::remove_file(&installed) {
            tries += 1;
            if tries > 50 {
                return Err(fail(e));
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = fs::remove_dir(&dir);
    }
    Ok(())
}

/// Removes the service from inside it, once the Microsoft Store package that installed it is gone
/// (store.rs): Windows deletes the service as it stops, its snapshots go now, and its executable, which
/// is running, is moved aside and deleted at the next restart (as is its folder, if nothing is in it
/// by then: turning instant search on again before that puts a new one there).
pub fn remove_self() -> io::Result<()> {
    let manager = open_manager()?;
    if let Some(service) = open_service(&manager)? {
        unsafe { DeleteService(service.0) }.map_err(os_error)?;
    }
    if let Ok(data) = win::data_dir() {
        let _ = fs::remove_dir_all(&data);
        if let Some(parent) = data.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
    let dir = install_dir()?;
    let removed = dir.join(format!("{}.removed", crate::SERVICE_EXE));
    let (exe, removed_w, dir_w) = (wide(dir.join(crate::SERVICE_EXE)), wide(&removed), wide(&dir));
    unsafe {
        MoveFileExW(PCWSTR(exe.as_ptr()), PCWSTR(removed_w.as_ptr()), MOVEFILE_REPLACE_EXISTING).map_err(os_error)?;
        MoveFileExW(PCWSTR(removed_w.as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT).map_err(os_error)?;
        let _ = MoveFileExW(PCWSTR(dir_w.as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_carry_the_step_and_the_error() {
        let failure = Failure(Step::Copy, io::Error::from_raw_os_error(5));
        assert_eq!(failure.exit_code(), 0x0003_0005);
        assert!(failure.to_string().starts_with("Couldn't copy the service to Program Files"));
        assert_eq!(describe(4), "Couldn't register the service");
    }
}
