//! The app's end of the pipe: whether the service is there (and really is the service), asking it
//! things, and turning it on or off, which runs the service executable elevated behind the UAC prompt.

use crate::pipe::PipeIo;
use crate::proto::{self, Hit, Message, Request, SearchRequest, Sizes, Status, UsageMap, UsageRequest, FRAME_GROWTH, FRAME_HITS, FRAME_JSON, FRAME_USAGE, MAX_FRAME};
use crate::win::{self, os_error, wide, Handle};
use std::fmt;
use std::io;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HWND};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::Win32::System::Services::{CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS};
use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";

#[derive(Debug)]
pub enum Error {
    /// The service isn't running (or isn't installed).
    NotRunning,
    /// It runs, but this user wasn't allowed to use it.
    NoAccess,
    /// Something else answers on the pipe's name: never trusted.
    Untrusted(String),
    /// The service answered with an error.
    Service(String),
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotRunning => write!(f, "The instant search service isn't running"),
            Error::NoAccess => write!(f, "Instant search was turned on for another user"),
            Error::Untrusted(owner) => write!(f, "The instant search pipe is owned by {owner}, not by the system: ignored"),
            Error::Service(message) => write!(f, "{message}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Whether a pipe's owner can be the service: SYSTEM or administrators. A process running as a
/// user could create the pipe's name while the service is stopped, to feed the app made-up results.
pub(crate) fn trusted(owner: &str) -> bool {
    owner == SYSTEM || owner == ADMINISTRATORS
}

/// Opens the service's pipe, refusing any server it can't trust.
pub fn connect() -> Result<Handle, Error> {
    match connect_to(crate::PIPE_NAME, &trusted) {
        // A service from before 0.6, which can then be offered its update.
        Err(Error::NotRunning) => connect_to(crate::LEGACY_PIPE_NAME, &trusted),
        other => other,
    }
}

pub(crate) fn connect_to(name: &str, trust: &dyn Fn(&str) -> bool) -> Result<Handle, Error> {
    let name = wide(name);
    for _ in 0..4 {
        let opened = unsafe {
            // Identification only: the service can see who's asking, never act as them.
            CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                None,
            )
        };
        match opened {
            Ok(handle) => {
                let handle = Handle(handle);
                let owner = win::owner_of(handle.0).map_err(Error::Io)?;
                if !trust(&owner) {
                    return Err(Error::Untrusted(owner));
                }
                return Ok(handle);
            }
            Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => unsafe {
                let _ = WaitNamedPipeW(PCWSTR(name.as_ptr()), 500);
            },
            Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() => return Err(Error::NotRunning),
            Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => return Err(Error::NoAccess),
            Err(e) => return Err(Error::Io(os_error(e))),
        }
    }
    Err(Error::Io(io::Error::new(io::ErrorKind::TimedOut, "the instant search service is busy")))
}

fn ask(pipe: Handle, request: &Request) -> Result<Handle, Error> {
    proto::write_request(&mut PipeIo(&pipe), request).map_err(Error::Io)?;
    Ok(pipe)
}

fn message(kind: u8, payload: &[u8]) -> Result<Message, Error> {
    if kind != FRAME_JSON {
        return Err(Error::Io(io::Error::new(io::ErrorKind::InvalidData, "unexpected frame")));
    }
    match serde_json::from_slice(payload).map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e)))? {
        Message::Error { message } => Err(Error::Service(message)),
        message => Ok(message),
    }
}

fn reply(pipe: &Handle) -> Result<Message, Error> {
    let (kind, payload) = proto::read_frame(&mut PipeIo(pipe), MAX_FRAME).map_err(Error::Io)?.ok_or(Error::NotRunning)?;
    message(kind, &payload)
}

fn unexpected() -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidData, "unexpected answer from the instant search service"))
}

pub fn status() -> Result<Status, Error> {
    status_on(connect()?)
}

pub(crate) fn status_on(pipe: Handle) -> Result<Status, Error> {
    match reply(&ask(pipe, &Request::Status)?)? {
        Message::Status(status) => Ok(status),
        _ => Err(unexpected()),
    }
}

pub fn sizes(path: &str) -> Result<Sizes, Error> {
    sizes_on(connect()?, path)
}

pub(crate) fn sizes_on(pipe: Handle, path: &str) -> Result<Sizes, Error> {
    match reply(&ask(pipe, &Request::Sizes { path: path.to_string() })?)? {
        Message::Sizes(sizes) => Ok(sizes),
        _ => Err(unexpected()),
    }
}

/// A disk-usage map of a folder (see `UsageRequest`). An older service that doesn't know the request
/// closes the pipe without an answer: an error like any other. One from before growth leaves out
/// what items held at a mark.
pub fn usage(request: &UsageRequest) -> Result<UsageMap, Error> {
    usage_on(connect()?, request)
}

pub(crate) fn usage_on(pipe: Handle, request: &UsageRequest) -> Result<UsageMap, Error> {
    let pipe = ask(pipe, &Request::Usage(request.clone()))?;
    let mut io = PipeIo(&pipe);
    let (mut nodes, mut previous) = (None, None);
    let invalid = |e: String| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e));
    while let Some((kind, payload)) = proto::read_frame(&mut io, MAX_FRAME).map_err(Error::Io)? {
        match kind {
            FRAME_USAGE => nodes = Some(proto::decode_usage(&payload).map_err(invalid)?),
            FRAME_GROWTH => previous = Some(proto::decode_growth(&payload).map_err(invalid)?),
            _ => {
                return match (message(kind, &payload)?, nodes) {
                    (Message::Usage(summary), Some(nodes)) => {
                        // Earlier sizes that don't line up with the map are no use.
                        let previous = previous.filter(|p: &Vec<u64>| p.len() == nodes.len());
                        Ok(UsageMap { nodes, previous, summary })
                    }
                    _ => Err(unexpected()),
                };
            }
        }
    }
    Err(Error::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "the instant search service closed the connection")))
}

/// Everything right inside a folder, for one Windows keeps this user out of. An older service that
/// doesn't know the request closes the pipe without an answer: an error like any other.
pub fn list(path: &str) -> Result<Vec<Hit>, Error> {
    list_on(connect()?, path)
}

pub(crate) fn list_on(pipe: Handle, path: &str) -> Result<Vec<Hit>, Error> {
    let pipe = ask(pipe, &Request::List { path: path.to_string() })?;
    let mut io = PipeIo(&pipe);
    let mut hits = Vec::new();
    while let Some((kind, payload)) = proto::read_frame(&mut io, MAX_FRAME).map_err(Error::Io)? {
        if kind == FRAME_HITS {
            hits.extend(proto::decode_hits(&payload).map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e)))?);
            continue;
        }
        return match message(kind, &payload)? {
            Message::Done { .. } => Ok(hits),
            _ => Err(unexpected()),
        };
    }
    Err(Error::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "the instant search service closed the connection")))
}

pub enum Event {
    /// The roots the index answers for, and those it doesn't (to search some other way).
    Plan { indexed: Vec<String>, skipped: Vec<String> },
    Hits(Vec<Hit>),
    /// `elapsed` is the service's own time, in ms.
    Done { count: u64, limited: bool, elapsed: f64 },
}

/// Runs a search, handing each event to `on` as it arrives; `on` returns false to stop listening
/// (closing the pipe stops the service's work too).
pub fn search(request: &SearchRequest, on: impl FnMut(Event) -> bool) -> Result<(), Error> {
    search_on(connect()?, request, on)
}

pub(crate) fn search_on(pipe: Handle, request: &SearchRequest, mut on: impl FnMut(Event) -> bool) -> Result<(), Error> {
    let pipe = ask(pipe, &Request::Search(request.clone()))?;
    let mut io = PipeIo(&pipe);
    while let Some((kind, payload)) = proto::read_frame(&mut io, MAX_FRAME).map_err(Error::Io)? {
        let event = if kind == FRAME_HITS {
            Event::Hits(proto::decode_hits(&payload).map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e)))?)
        } else {
            match message(kind, &payload)? {
                Message::Plan { indexed, skipped } => Event::Plan { indexed, skipped },
                Message::Done { count, limited, elapsed } => Event::Done { count, limited, elapsed },
                _ => return Err(unexpected()),
            }
        };
        let done = matches!(event, Event::Done { .. });
        if !on(event) || done {
            return Ok(());
        }
    }
    Err(Error::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "the instant search service closed the connection")))
}

/// Whether the service is registered with Windows (running or not).
pub fn installed() -> bool {
    unsafe {
        let Ok(manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else { return false };
        let service = OpenServiceW(manager, PCWSTR(wide(crate::SERVICE_NAME).as_ptr()), SERVICE_QUERY_STATUS);
        if let Ok(service) = service {
            let _ = CloseServiceHandle(service);
        }
        let _ = CloseServiceHandle(manager);
        service.is_ok()
    }
}

/// Runs `exe args` elevated (Windows shows the UAC prompt over `owner`) and waits for it to finish.
/// Err("cancelled") when the prompt was declined.
pub(crate) fn run_elevated(exe: &Path, args: &str, owner: Option<isize>) -> Result<u32, String> {
    let verb = wide("runas");
    let file = wide(exe);
    let params = wide(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        hwnd: HWND(owner.unwrap_or(0) as *mut _),
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() { "cancelled".to_string() } else { os_error(e).to_string() });
    }
    let process = Handle(info.hProcess);
    let mut code = 0;
    unsafe {
        WaitForSingleObject(process.0, INFINITE);
        GetExitCodeProcess(process.0, &mut code).map_err(|e| os_error(e).to_string())?;
    }
    Ok(code)
}

fn explain(code: u32) -> String {
    let error = code & 0xFFFF;
    let what = crate::install::describe(code >> 16);
    if error == 0 {
        what.to_string()
    } else {
        format!("{what}: {}", io::Error::from_raw_os_error(error as i32))
    }
}

/// Installs and starts the service for the current user, from the executable bundled with the app.
/// Err("cancelled") when the UAC prompt was declined.
pub fn enable(bundled: &Path, owner: Option<isize>) -> Result<(), String> {
    let sid = win::current_user_sid().map_err(|e| e.to_string())?;
    match run_elevated(bundled, &format!("install --allow-sid {sid}"), owner)? {
        0 => Ok(()),
        code => Err(explain(code)),
    }
}

/// Stops and removes the service and its snapshots.
pub fn disable(bundled: &Path, owner: Option<isize>) -> Result<(), String> {
    match run_elevated(bundled, "uninstall", owner)? {
        0 => Ok(()),
        code => Err(explain(code)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_installer_exit_codes() {
        assert_eq!(explain(0x0004_0000), "Couldn't register the service");
        assert!(explain(0x0003_0005).starts_with("Couldn't copy the service to Program Files: "));
    }
}
