//! Running as a Windows service: the control dispatcher, reporting state to the service manager,
//! and the run loop shared with `shadui-index run` in a console.

use crate::engine::{Engine, Stop};
use crate::pipe;
use crate::win::{self, os_error, wide};
use std::io;
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_CALL_NOT_IMPLEMENTED, NO_ERROR};
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, SERVICE_ACCEPT_SHUTDOWN, SERVICE_ACCEPT_STOP,
    SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING, SERVICE_TABLE_ENTRYW,
    SERVICE_WIN32_OWN_PROCESS,
};

/// The registry key under which the installer records the users allowed to use the index.
pub fn parameters_key() -> String {
    format!(r"SYSTEM\CurrentControlSet\Services\{}\Parameters", crate::SERVICE_NAME)
}

/// The users (SIDs) the installer allowed.
pub fn allowed_users() -> Vec<String> {
    win::Key::create(&parameters_key(), false).and_then(|key| key.strings("AllowedUsers")).unwrap_or_default()
}

/// Runs the index until `stop` is set: answers on the pipe once it's listening (the drives fill
/// in as they're read), then stops the workers, which save their snapshots.
pub fn run(stop: &Stop, allowed: &[String], only: Option<Vec<char>>, echo: bool, ready: impl FnOnce()) -> io::Result<()> {
    let engine = Engine::start(win::data_dir()?, only, echo);
    engine.log.line(&format!("allowed users: {}", if allowed.is_empty() { "administrators only".to_string() } else { allowed.join(", ") }));
    let server = match pipe::serve(engine.clone(), allowed) {
        Ok(server) => server,
        Err(e) => {
            engine.log.line(&format!("can't open the pipe: {e}"));
            engine.stop();
            return Err(e);
        }
    };
    ready();
    stop.wait();
    server.stop();
    engine.stop();
    Ok(())
}

struct Control {
    status: SERVICE_STATUS_HANDLE,
    stop: Arc<Stop>,
}

// The handle is a token the service manager hands out; it's used from any thread.
unsafe impl Send for Control {}

static CONTROL: OnceLock<Mutex<Control>> = OnceLock::new();

fn report(status: SERVICE_STATUS_HANDLE, state: SERVICE_STATUS_CURRENT_STATE, exit: u32, wait_hint: u32) {
    let accepted = if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 };
    let status_info = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: exit,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe {
        let _ = SetServiceStatus(status, &status_info);
    }
}

/// Hands the process to the service manager; returns when the service has stopped.
pub fn dispatch() -> io::Result<()> {
    let mut name = wide(crate::SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }.map_err(os_error)
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let name = wide(crate::SERVICE_NAME);
    let Ok(status) = (unsafe { RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(control), None) }) else { return };
    let stop = Arc::new(Stop::default());
    let _ = CONTROL.set(Mutex::new(Control { status, stop: stop.clone() }));
    report(status, SERVICE_START_PENDING, 0, 10_000);
    let result = run(&stop, &allowed_users(), None, false, || report(status, SERVICE_RUNNING, 0, 0));
    let exit = match result {
        Ok(()) => 0,
        Err(e) => e.raw_os_error().unwrap_or(1) as u32,
    };
    report(status, SERVICE_STOPPED, exit, 0);
}

unsafe extern "system" fn control(code: u32, _kind: u32, _data: *mut core::ffi::c_void, _context: *mut core::ffi::c_void) -> u32 {
    match code {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            if let Some(control) = CONTROL.get() {
                let control = control.lock().unwrap();
                // Saving snapshots takes a moment on big drives.
                report(control.status, SERVICE_STOP_PENDING, 0, 30_000);
                control.stop.set();
            }
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}
