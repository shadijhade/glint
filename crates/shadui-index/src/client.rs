//! The app's end of the pipe: whether the service is there (and really is the service), asking it
//! things, and turning it on or off, which runs the service executable elevated behind the UAC prompt.

use crate::pipe::PipeIo;
use crate::proto::{
    self, HistoryAnswer, HistoryRequest, Hit, Message, Request, SearchRequest, Sizes, Status, UsageMap, UsageRequest,
    FRAME_GROWTH, FRAME_HITS, FRAME_JSON, FRAME_USAGE, MAX_FRAME,
};
use crate::win::{self, os_error, wide, Handle};
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ,
    GENERIC_WRITE, HANDLE, HWND,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_NONE, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows::Win32::System::Pipes::WaitNamedPipeW;
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT, SERVICE_QUERY_STATUS,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, GetExitCodeProcess, OpenThread, WaitForSingleObject, INFINITE, THREAD_TERMINATE,
};
use windows::Win32::System::IO::CancelSynchronousIo;
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
            Error::Untrusted(owner) => write!(
                f,
                "The instant search pipe is owned by {owner}, not by the system: ignored"
            ),
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
    Err(Error::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        "the instant search service is busy",
    )))
}

const POOL_MAX: usize = 4;
const POOL_IDLE: Duration = Duration::from_secs(20);

struct PooledPipe {
    pipe: Handle,
    last_used: Instant,
}

static POOL: Mutex<Vec<PooledPipe>> = Mutex::new(Vec::new());

fn pool_take() -> Option<Handle> {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    while let Some(entry) = pool.pop() {
        if entry.last_used.elapsed() < POOL_IDLE {
            return Some(entry.pipe);
        }
    }
    None
}

fn pool_put(pipe: Handle) {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    pool.retain(|entry| now.saturating_duration_since(entry.last_used) < POOL_IDLE);
    if pool.len() < POOL_MAX {
        pool.push(PooledPipe { pipe, last_used: now });
    }
}

fn pool_clear() {
    POOL.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Runs a framed read-only RPC on a pooled keepalive connection, transparently retrying once on a
/// fresh connection if a reused pipe was closed (idle timeout, service restart, or an older 1-shot
/// service that disconnects after each reply).
fn call_pooled<T>(mut f: impl FnMut(&Handle) -> Result<T, Error>) -> Result<T, Error> {
    if let Some(pipe) = pool_take() {
        match f(&pipe) {
            Ok(val) => {
                pool_put(pipe);
                return Ok(val);
            }
            Err(Error::Service(msg)) => {
                pool_put(pipe);
                return Err(Error::Service(msg));
            }
            Err(Error::Io(_) | Error::NotRunning) => {}
            Err(other) => return Err(other),
        }
    }
    let pipe = connect()?;
    match f(&pipe) {
        Ok(val) => {
            pool_put(pipe);
            Ok(val)
        }
        Err(Error::Service(msg)) => {
            pool_put(pipe);
            Err(Error::Service(msg))
        }
        Err(e) => Err(e),
    }
}

fn ask_ref(pipe: &Handle, request: &Request) -> Result<(), Error> {
    proto::write_request(&mut PipeIo(pipe), request).map_err(Error::Io)
}

fn ask(pipe: Handle, request: &Request) -> Result<Handle, Error> {
    ask_ref(&pipe, request)?;
    Ok(pipe)
}

fn message(kind: u8, payload: &[u8]) -> Result<Message, Error> {
    if kind != FRAME_JSON {
        return Err(Error::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected frame",
        )));
    }
    match serde_json::from_slice(payload).map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e)))? {
        Message::Error { message } => Err(Error::Service(message)),
        message => Ok(message),
    }
}

fn reply(pipe: &Handle) -> Result<Message, Error> {
    let (kind, payload) = proto::read_frame(&mut PipeIo(pipe), MAX_FRAME)
        .map_err(Error::Io)?
        .ok_or(Error::NotRunning)?;
    message(kind, &payload)
}

fn unexpected() -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        "unexpected answer from the instant search service",
    ))
}

pub fn status() -> Result<Status, Error> {
    call_pooled(status_ref)
}

pub(crate) fn status_ref(pipe: &Handle) -> Result<Status, Error> {
    ask_ref(pipe, &Request::Status)?;
    match reply(pipe)? {
        Message::Status(status) => Ok(status),
        _ => Err(unexpected()),
    }
}

#[cfg(test)]
pub(crate) fn status_on(pipe: Handle) -> Result<Status, Error> {
    status_ref(&pipe)
}

/// Says this copy of Glint, not the Microsoft Store's, uses the service, so it stays when the Store's
/// copy that installed it goes; answers the status, as `status` does. None from a service from before
/// it, which closes the pipe without a word (where `reply` would say it isn't running).
pub fn claim() -> Result<Option<Status>, Error> {
    let pipe = ask(connect()?, &Request::Claim)?;
    match proto::read_frame(&mut PipeIo(&pipe), MAX_FRAME).map_err(Error::Io)? {
        None => Ok(None),
        Some((kind, payload)) => match message(kind, &payload)? {
            Message::Status(status) => Ok(Some(status)),
            _ => Err(unexpected()),
        },
    }
}

pub fn sizes(path: &str) -> Result<Sizes, Error> {
    call_pooled(|pipe| sizes_ref(pipe, path, false))
}

/// `sizes`, with how many files and folders are below the folder (`Sizes::files` and `folders`). A service
/// from before answers without them.
pub fn sizes_counted(path: &str) -> Result<Sizes, Error> {
    call_pooled(|pipe| sizes_ref(pipe, path, true))
}

pub(crate) fn sizes_ref(pipe: &Handle, path: &str, count: bool) -> Result<Sizes, Error> {
    ask_ref(
        pipe,
        &Request::Sizes {
            path: path.to_string(),
            count,
        },
    )?;
    match reply(pipe)? {
        Message::Sizes(sizes) => Ok(sizes),
        _ => Err(unexpected()),
    }
}

#[cfg(test)]
pub(crate) fn sizes_on(pipe: Handle, path: &str, count: bool) -> Result<Sizes, Error> {
    sizes_ref(&pipe, path, count)
}

/// A disk-usage map of a folder (see `UsageRequest`). An older service that doesn't know the request
/// closes the pipe without an answer: an error like any other. One from before growth leaves out
/// what items held at a mark.
pub fn usage(request: &UsageRequest) -> Result<UsageMap, Error> {
    call_pooled(|pipe| usage_ref(pipe, request))
}

pub(crate) fn usage_ref(pipe: &Handle, request: &UsageRequest) -> Result<UsageMap, Error> {
    ask_ref(pipe, &Request::Usage(request.clone()))?;
    let mut io = PipeIo(pipe);
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
                        Ok(UsageMap {
                            nodes,
                            previous,
                            summary,
                        })
                    }
                    _ => Err(unexpected()),
                };
            }
        }
    }
    Err(Error::Io(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "the instant search service closed the connection",
    )))
}

#[cfg(test)]
pub(crate) fn usage_on(pipe: Handle, request: &UsageRequest) -> Result<UsageMap, Error> {
    usage_ref(&pipe, request)
}

/// Everything right inside a folder, for one Windows keeps this user out of. An older service that
/// doesn't know the request closes the pipe without an answer: an error like any other.
pub fn list(path: &str) -> Result<Vec<Hit>, Error> {
    call_pooled(|pipe| list_ref(pipe, path))
}

pub(crate) fn list_ref(pipe: &Handle, path: &str) -> Result<Vec<Hit>, Error> {
    ask_ref(pipe, &Request::List { path: path.to_string() })?;
    let mut io = PipeIo(pipe);
    let mut hits = Vec::new();
    while let Some((kind, payload)) = proto::read_frame(&mut io, MAX_FRAME).map_err(Error::Io)? {
        if kind == FRAME_HITS {
            hits.extend(
                proto::decode_hits(&payload).map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e)))?,
            );
            continue;
        }
        return match message(kind, &payload)? {
            Message::Done { .. } => Ok(hits),
            _ => Err(unexpected()),
        };
    }
    Err(Error::Io(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "the instant search service closed the connection",
    )))
}

#[cfg(test)]
pub(crate) fn list_on(pipe: Handle, path: &str) -> Result<Vec<Hit>, Error> {
    list_ref(&pipe, path)
}

/// What was renamed, moved and deleted on the indexed drives (see `HistoryRequest`). An older service
/// that doesn't know the request closes the pipe without an answer: an Io error, UnexpectedEof.
pub fn history(request: &HistoryRequest) -> Result<HistoryAnswer, Error> {
    call_pooled(|pipe| history_ref(pipe, request))
}

pub(crate) fn history_ref(pipe: &Handle, request: &HistoryRequest) -> Result<HistoryAnswer, Error> {
    match answer_ref(pipe, &Request::History(request.clone()))? {
        Message::History(answer) => Ok(answer),
        _ => Err(unexpected()),
    }
}

#[cfg(test)]
pub(crate) fn history_on(pipe: Handle, request: &HistoryRequest) -> Result<HistoryAnswer, Error> {
    history_ref(&pipe, request)
}

/// Every tag on the indexed drives with how many items have it, most used first. An older service closes
/// the pipe on it: an Io error.
pub fn tags() -> Result<Vec<(String, u64)>, Error> {
    call_pooled(|pipe| match answer_ref(pipe, &Request::Tags)? {
        Message::Tags { tags } => Ok(tags),
        _ => Err(unexpected()),
    })
}

/// Every label and tag on the indexed drives with how many items have it, most used first, and how many items
/// have a comment (docs/gpui/marks). An older service closes the pipe on it: an Io error.
pub fn marks() -> Result<proto::MarkCounts, Error> {
    call_pooled(marks_ref)
}

/// One request, one answer; a pipe closed without one is an UnexpectedEof, as the others give.
fn answer_ref(pipe: &Handle, request: &Request) -> Result<Message, Error> {
    ask_ref(pipe, request)?;
    match proto::read_frame(&mut PipeIo(pipe), MAX_FRAME).map_err(Error::Io)? {
        Some((kind, payload)) => message(kind, &payload),
        None => Err(Error::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the instant search service closed the connection",
        ))),
    }
}

pub(crate) fn marks_ref(pipe: &Handle) -> Result<proto::MarkCounts, Error> {
    match answer_ref(pipe, &Request::Marks)? {
        Message::Marks(counts) => Ok(counts),
        _ => Err(unexpected()),
    }
}

#[cfg(test)]
pub(crate) fn marks_on(pipe: Handle) -> Result<proto::MarkCounts, Error> {
    marks_ref(&pipe)
}

/// The marked items right inside the folder `path`: their names, labels and tags, and whether each has a
/// comment (never its text). An older service closes the pipe on it: an Io error.
pub fn marks_in(path: &str) -> Result<Vec<proto::MarkedChild>, Error> {
    call_pooled(|pipe| marks_in_ref(pipe, path))
}

pub(crate) fn marks_in_ref(pipe: &Handle, path: &str) -> Result<Vec<proto::MarkedChild>, Error> {
    match answer_ref(pipe, &Request::MarksIn { path: path.to_string() })? {
        Message::MarksIn { items } => Ok(items),
        _ => Err(unexpected()),
    }
}

#[cfg(test)]
pub(crate) fn marks_in_on(pipe: Handle, path: &str) -> Result<Vec<proto::MarkedChild>, Error> {
    marks_in_ref(&pipe, path)
}

/// Windows' shadow copies of every volume, newest first, with the times VSS took them (which only an
/// administrator, the service, may ask). An older service closes the pipe on it: an Io error.
pub fn snapshots() -> Result<Vec<crate::shadow::Snapshot>, Error> {
    call_pooled(|pipe| match answer_ref(pipe, &Request::Snapshots)? {
        Message::Snapshots { snapshots } => Ok(snapshots),
        _ => Err(unexpected()),
    })
}

/// Resolves `(volume_serial, frn)` through the running index service, returning None when not indexed.
pub fn resolve_id(volume_serial: u32, frn: u64) -> Result<Option<String>, Error> {
    call_pooled(|pipe| {
        match answer_ref(pipe, &Request::ResolveId { volume_serial, frn })? {
            Message::Resolved { path } => Ok(path),
            _ => Err(unexpected()),
        }
    })
}

/// Hands the running service a later build of itself: `exe`, the service executable that ships with the
/// app, and `signature`, its .sig file's text (update.rs). Ok(Some(version)) once the service has taken
/// it and is restarting into it; Ok(None) from a service from before it could update itself, which
/// hangs up on the request.
pub fn update(exe: &[u8], signature: &str) -> Result<Option<String>, Error> {
    pool_clear();
    update_on(connect()?, exe, signature)
}

pub(crate) fn update_on(pipe: Handle, exe: &[u8], signature: &str) -> Result<Option<String>, Error> {
    use std::io::Write;
    let mut io = PipeIo(&pipe);
    let request = Request::Update {
        signature: signature.to_string(),
        size: exe.len() as u64,
    };
    let hung_up = |e: &io::Error| matches!(e.raw_os_error(), Some(109 | 232)); // ERROR_BROKEN_PIPE, ERROR_NO_DATA
    match proto::write_request(&mut io, &request).and_then(|()| io.write_all(exe)) {
        Ok(()) => {}
        Err(e) if hung_up(&e) => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    }
    match proto::read_frame(&mut io, MAX_FRAME).map_err(Error::Io)? {
        None => Ok(None),
        Some((kind, payload)) => match message(kind, &payload)? {
            Message::Updating { version } => Ok(Some(version)),
            _ => Err(unexpected()),
        },
    }
}

/// After an update: waits for the service to answer as `version`, up to `limit`.
pub fn wait_for_version(version: &str, limit: std::time::Duration) -> Result<Status, Error> {
    let started = std::time::Instant::now();
    loop {
        match status() {
            Ok(status) if status.version == version => return Ok(status),
            // Still the old one, stopping, or not up yet.
            Ok(_) | Err(Error::NotRunning | Error::Io(_)) => {}
            Err(e) => return Err(e),
        }
        if started.elapsed() > limit {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("the instant search service didn't come back as version {version}"),
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

pub enum Event {
    /// The roots the index answers for, and those it doesn't (to search some other way).
    Plan {
        indexed: Vec<String>,
        skipped: Vec<String>,
    },
    Hits(Vec<Hit>),
    /// `elapsed` is the service's own time, in ms.
    /// `fuzzy`: nothing matched as typed, and these were close matches.
    Done {
        count: u64,
        limited: bool,
        elapsed: f64,
        fuzzy: bool,
    },
}

/// Runs a search, handing each event to `on` as it arrives; `on` returns false to stop listening
/// (closing the pipe stops the service's work too).
pub fn search(request: &SearchRequest, on: impl FnMut(Event) -> bool) -> Result<(), Error> {
    search_on(connect()?, request, on)
}

/// [`search`], which another thread can end at once with [`Cancel::cancel`] (a newer key press): the read it
/// waits in is cancelled and the pipe closed, so the service (from 0.12) stops scanning for it. An older service
/// finds the pipe closed at its next write, as before. Ending it this way is not an error.
pub fn search_until(request: &SearchRequest, cancel: &Cancel, on: impl FnMut(Event) -> bool) -> Result<(), Error> {
    if cancel.is_cancelled() {
        return Ok(());
    }
    search_on_until(connect()?, request, cancel, on)
}

/// Ends a [`search_until`] from another thread.
#[derive(Default)]
pub struct Cancel(std::sync::Mutex<CancelState>);

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    /// The thread waiting for the service's answer, while it waits: a handle of its own, closed when it's done.
    reader: Option<isize>,
}

impl Cancel {
    /// Marks the search ended, and cancels the read its thread waits in. A read about to begin isn't caught
    /// (the search then ends at the service's next answer, as `on` would end it).
    pub fn cancel(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.cancelled = true;
        if let Some(reader) = state.reader {
            unsafe {
                let _ = CancelSynchronousIo(HANDLE(reader as *mut _));
            }
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).cancelled
    }

    /// This thread waits for answers until the guard goes; None when the search was ended already.
    fn reading(&self) -> Option<Reading<'_>> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.cancelled {
            return None;
        }
        let thread = unsafe { OpenThread(THREAD_TERMINATE, false, GetCurrentThreadId()) }.ok();
        state.reader = thread.map(|t| t.0 as isize);
        Some(Reading { cancel: self, thread })
    }
}

/// See [`Cancel::reading`].
struct Reading<'a> {
    cancel: &'a Cancel,
    thread: Option<HANDLE>,
}

impl Drop for Reading<'_> {
    fn drop(&mut self) {
        // Under the lock, so `cancel` never uses the handle once it's closed.
        let mut state = self.cancel.0.lock().unwrap_or_else(|e| e.into_inner());
        state.reader = None;
        if let Some(thread) = self.thread.take() {
            unsafe {
                let _ = CloseHandle(thread);
            }
        }
    }
}

pub(crate) fn search_on(pipe: Handle, request: &SearchRequest, on: impl FnMut(Event) -> bool) -> Result<(), Error> {
    search_on_until(pipe, request, &Cancel::default(), on)
}

pub(crate) fn search_on_until(
    pipe: Handle,
    request: &SearchRequest,
    cancel: &Cancel,
    mut on: impl FnMut(Event) -> bool,
) -> Result<(), Error> {
    let pipe = ask(pipe, &Request::Search(request.clone()))?;
    let Some(_reading) = cancel.reading() else {
        return Ok(());
    };
    let mut io = PipeIo(&pipe);
    loop {
        let frame = match proto::read_frame(&mut io, MAX_FRAME) {
            Ok(frame) => frame,
            Err(_) if cancel.is_cancelled() => return Ok(()),
            Err(e) => return Err(Error::Io(e)),
        };
        let Some((kind, payload)) = frame else { break };
        if cancel.is_cancelled() {
            return Ok(());
        }
        let event = if kind == FRAME_HITS {
            Event::Hits(
                proto::decode_hits(&payload).map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidData, e)))?,
            )
        } else {
            match message(kind, &payload)? {
                Message::Plan { indexed, skipped } => Event::Plan { indexed, skipped },
                Message::Done {
                    count,
                    limited,
                    elapsed,
                    fuzzy,
                } => Event::Done {
                    count,
                    limited,
                    elapsed,
                    fuzzy,
                },
                _ => return Err(unexpected()),
            }
        };
        let done = matches!(event, Event::Done { .. });
        if !on(event) || done {
            return Ok(());
        }
    }
    Err(Error::Io(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "the instant search service closed the connection",
    )))
}

/// Whether the service is registered with Windows (running or not).
pub fn installed() -> bool {
    unsafe {
        let Ok(manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) else {
            return false;
        };
        let service = OpenServiceW(
            manager,
            PCWSTR(wide(crate::SERVICE_NAME).as_ptr()),
            SERVICE_QUERY_STATUS,
        );
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
        return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
            "cancelled".to_string()
        } else {
            os_error(e).to_string()
        });
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

/// Installs and starts the service for the current user, from the executable bundled with the app:
/// `package` is the Microsoft Store copy's package family, when it's that one (store.rs).
/// Err("cancelled") when the UAC prompt was declined.
pub fn enable(bundled: &Path, owner: Option<isize>, package: Option<&str>) -> Result<(), String> {
    let sid = win::current_user_sid().map_err(|e| e.to_string())?;
    match run_elevated(bundled, &install_args(&sid, package), owner)? {
        0 => Ok(()),
        code => Err(explain(code)),
    }
}

fn install_args(sid: &str, package: Option<&str>) -> String {
    let package = package.map_or(String::new(), |family| format!(" --package {family}"));
    format!("install --allow-sid {sid}{package}")
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
    fn names_the_store_package_only_for_the_stores_copy() {
        assert_eq!(
            install_args("S-1-5-21-1-2-3-1001", None),
            "install --allow-sid S-1-5-21-1-2-3-1001"
        );
        assert_eq!(
            install_args("S-1-5-21-1-2-3-1001", Some("Glint.Test_4cqh9yg7jc6m4")),
            "install --allow-sid S-1-5-21-1-2-3-1001 --package Glint.Test_4cqh9yg7jc6m4"
        );
    }

    fn served(tag: &str) -> (String, crate::pipe::Server) {
        use crate::tree::{make_ref, RecordState, ROOT};
        let mut tree = crate::tree::Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
        tree.set_record(
            20,
            &RecordState { seq: 1, dir: true, names: vec![(make_ref(ROOT, 5), "Users".into())], ..Default::default() },
        );
        let name = format!(r"\\.\pipe\shadui-index-test-client-{tag}-{}", std::process::id());
        let me = win::current_user_sid().unwrap();
        let dacl = format!("{}(A;;GA;;;{me})", crate::pipe::sddl(&[]));
        let engine = crate::engine::Engine::with_tree('T', tree);
        (name.clone(), crate::pipe::serve_on(&name, engine, &dacl, None).unwrap())
    }

    #[test]
    fn marks_over_a_real_pipe_answer_counts_that_decode() {
        let (name, server) = served("marks");
        let pipe = connect_to(&name, &|_: &str| true).unwrap();
        assert!(marks_on(pipe).is_ok());
        server.stop();
    }

    #[test]
    fn marks_in_over_a_real_pipe_answers_or_says_why_never_unexpected() {
        let (name, server) = served("marks-in");
        for path in [r"T:\Users", r"T:\Nowhere"] {
            let said = marks_in_on(connect_to(&name, &|_: &str| true).unwrap(), path);
            assert!(matches!(said, Ok(_) | Err(Error::Service(_))), "{said:?}");
        }
        server.stop();
    }

    #[test]
    fn marks_against_a_pipe_nobody_serves_is_the_error_connecting_gives() {
        let name = format!(r"\\.\pipe\shadui-index-test-client-nobody-{}", std::process::id());
        assert!(matches!(connect_to(&name, &|_: &str| true), Err(Error::NotRunning)));
    }

    #[test]
    fn explains_installer_exit_codes() {
        assert_eq!(explain(0x0004_0000), "Couldn't register the service");
        assert!(explain(0x0003_0005).starts_with("Couldn't copy the service to Program Files: "));
    }
}
