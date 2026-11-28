//! The named pipe the service answers on. Only SYSTEM, administrators and the users who turned
//! instant search on may open it, and none of them may create an instance of it, so no other
//! process can pose as the service.

use crate::engine::Engine;
use crate::proto::{self, Message, Request};
use crate::update::{self, Updater};
use crate::win::{self, os_error, wide, Handle, SecurityDescriptor};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PeekNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, INFINITE};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// Read and write (FILE_GENERIC_READ | FILE_GENERIC_WRITE) without FILE_CREATE_PIPE_INSTANCE (0x4).
const USER_ACCESS: u32 = (0x0012_0089 | 0x0012_0116) & !0x4;

/// The pipe's DACL: full control for SYSTEM and administrators, read and write for each allowed user.
/// Protected (P), so nothing is inherited.
pub fn sddl(allowed: &[String]) -> String {
    let mut sddl = String::from("D:P(A;;GA;;;SY)(A;;GA;;;BA)");
    for sid in allowed.iter().filter(|s| win::is_sid(s)) {
        sddl.push_str(&format!("(A;;0x{USER_ACCESS:x};;;{sid})"));
    }
    sddl
}

/// Reads and writes a pipe handle as a byte stream.
pub struct PipeIo<'a>(pub &'a Handle);

fn is_pipe_eof(e: &windows::core::Error) -> bool {
    e.code() == ERROR_BROKEN_PIPE.to_hresult() || e.code() == ERROR_PIPE_NOT_CONNECTED.to_hresult()
}

impl Read for PipeIo<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;
        let len = buf.len().min(u32::MAX as usize);
        match unsafe { ReadFile(self.0 .0, Some(&mut buf[..len]), Some(&mut read), None) } {
            Ok(()) => Ok(read as usize),
            Err(e) if is_pipe_eof(&e) => Ok(0),
            Err(e) => Err(os_error(e)),
        }
    }
}

impl Write for PipeIo<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        let len = buf.len().min(1 << 30);
        unsafe { WriteFile(self.0 .0, Some(&buf[..len]), Some(&mut written), None) }.map_err(os_error)?;
        Ok(written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The listening pipe; stop it to close it.
pub struct Server {
    stopping: Arc<AtomicBool>,
    stop_event: Arc<Handle>,
    thread: Option<JoinHandle<()>>,
}

fn manual_event() -> io::Result<Handle> {
    let h = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(os_error)?;
    Ok(Handle(h))
}

fn instance_ex(name: &str, sd: &SecurityDescriptor, first: bool, overlapped: bool) -> io::Result<Handle> {
    let attributes = sd.attributes();
    // The first instance must be ours: if anything else already serves the name, fail.
    let mut mode = PIPE_ACCESS_DUPLEX;
    if first {
        mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    if overlapped {
        mode |= FILE_FLAG_OVERLAPPED;
    }
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide(name).as_ptr()),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            1 << 19,
            1 << 19,
            0,
            Some(&attributes),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(handle))
}

#[cfg(test)]
fn instance(name: &str, sd: &SecurityDescriptor, first: bool) -> io::Result<Handle> {
    instance_ex(name, sd, first, false)
}

/// Reads and writes an overlapped server pipe handle with bounded timeouts and instant wakeup on
/// `stop` (never spinning in `sleep(1ms)` polling loops and never leaving a pending kernel IRP
/// referencing stack memory).
struct OverlappedPipe<'a> {
    pipe: &'a Handle,
    event: &'a Handle,
    stop: &'a Handle,
    read_timeout: Duration,
    write_timeout: Duration,
}

impl Read for OverlappedPipe<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(u32::MAX as usize);
        let mut ov = OVERLAPPED {
            hEvent: self.event.0,
            ..Default::default()
        };
        let _ = unsafe { ResetEvent(self.event.0) };
        match unsafe { ReadFile(self.pipe.0, Some(&mut buf[..len]), None, Some(&mut ov)) } {
            Ok(()) => {
                let mut transferred = 0u32;
                match unsafe { GetOverlappedResult(self.pipe.0, &ov, &mut transferred, true) } {
                    Ok(()) => Ok(transferred as usize),
                    Err(e) if is_pipe_eof(&e) => Ok(0),
                    Err(e) => Err(os_error(e)),
                }
            }
            Err(e) if is_pipe_eof(&e) => Ok(0),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                let handles = [self.event.0, self.stop.0];
                let timeout_ms = self.read_timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
                let wait = unsafe { WaitForMultipleObjects(&handles, false, timeout_ms) };
                if wait == WAIT_OBJECT_0 {
                    let mut transferred = 0u32;
                    match unsafe { GetOverlappedResult(self.pipe.0, &ov, &mut transferred, true) } {
                        Ok(()) => Ok(transferred as usize),
                        Err(e) if is_pipe_eof(&e) => Ok(0),
                        Err(e) => Err(os_error(e)),
                    }
                } else {
                    // Cancel pending IRP and wait for completion before `ov` and `buf` leave scope.
                    let mut transferred = 0u32;
                    let res = unsafe {
                        let _ = CancelIoEx(self.pipe.0, Some(&ov));
                        GetOverlappedResult(self.pipe.0, &ov, &mut transferred, true)
                    };
                    if wait.0 == WAIT_OBJECT_0.0 + 1 {
                        return Ok(0);
                    }
                    if wait == WAIT_TIMEOUT && res.is_ok() && transferred > 0 {
                        return Ok(transferred as usize);
                    }
                    Err(io::Error::new(io::ErrorKind::TimedOut, "timed out waiting for request"))
                }
            }
            Err(e) => Err(os_error(e)),
        }
    }
}

impl Write for OverlappedPipe<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let len = buf.len().min(1 << 30);
        let mut ov = OVERLAPPED {
            hEvent: self.event.0,
            ..Default::default()
        };
        let _ = unsafe { ResetEvent(self.event.0) };
        match unsafe { WriteFile(self.pipe.0, Some(&buf[..len]), None, Some(&mut ov)) } {
            Ok(()) => {
                let mut transferred = 0u32;
                unsafe { GetOverlappedResult(self.pipe.0, &ov, &mut transferred, true) }.map_err(os_error)?;
                Ok(transferred as usize)
            }
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                // The stop too: `Server::stop` waits for the workers, and a client that stopped reading
                // mustn't hold it up for the whole timeout.
                let handles = [self.event.0, self.stop.0];
                let timeout_ms = self.write_timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
                let wait = unsafe { WaitForMultipleObjects(&handles, false, timeout_ms) };
                if wait == WAIT_OBJECT_0 {
                    let mut transferred = 0u32;
                    unsafe { GetOverlappedResult(self.pipe.0, &ov, &mut transferred, true) }.map_err(os_error)?;
                    Ok(transferred as usize)
                } else {
                    let mut transferred = 0u32;
                    let res = unsafe {
                        let _ = CancelIoEx(self.pipe.0, Some(&ov));
                        GetOverlappedResult(self.pipe.0, &ov, &mut transferred, true)
                    };
                    if res.is_ok() && transferred > 0 {
                        return Ok(transferred as usize);
                    }
                    Err(io::Error::new(io::ErrorKind::TimedOut, "timed out writing response"))
                }
            }
            Err(e) => Err(os_error(e)),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serves the engine to the `allowed` users; `updater` when this is the installed service, which can
/// update itself (update.rs).
pub fn serve(engine: Arc<Engine>, allowed: &[String], updater: Option<Arc<Updater>>) -> io::Result<Server> {
    let sddl = sddl(allowed);
    serve_on(crate::PIPE_NAME, engine.clone(), &sddl, updater.clone()).or_else(|e| {
        engine.log.line(&format!(
            "can't serve under the protected prefix ({e}); serving on {}",
            crate::LEGACY_PIPE_NAME
        ));
        serve_on(crate::LEGACY_PIPE_NAME, engine, &sddl, updater)
    })
}

/// Connections answered at once. More (a stuck or hostile local program opening many) are told the
/// service is busy instead of each getting a worker.
const MAX_CLIENTS: usize = 32;
/// Above this many active connections, close after each reply instead of keeping idle connections open.
const KEEPALIVE_SOFT_LIMIT: usize = MAX_CLIENTS / 2;
/// How long a client has to send its first request (or update payload) once connected.
const REQUEST_WAIT: Duration = Duration::from_secs(10);
/// How long a pooled keepalive connection waits for a follow-up request before closing.
const IDLE_KEEPALIVE: Duration = Duration::from_secs(30);
/// Maximum time a write may block on a client that stopped reading.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Idle worker threads exit after this long without a new connection.
const WORKER_IDLE_EXIT: Duration = Duration::from_secs(60);

struct PoolState {
    queue: VecDeque<Handle>,
    idle_workers: usize,
    total_workers: usize,
    closed: bool,
}

struct WorkerPool {
    state: Mutex<PoolState>,
    cv: Condvar,
    engine: Arc<Engine>,
    updater: Option<Arc<Updater>>,
    stopping: Arc<AtomicBool>,
    stop_event: Arc<Handle>,
    active: Arc<AtomicUsize>,
}

impl WorkerPool {
    fn new(
        engine: Arc<Engine>,
        updater: Option<Arc<Updater>>,
        stopping: Arc<AtomicBool>,
        stop_event: Arc<Handle>,
        active: Arc<AtomicUsize>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(PoolState {
                queue: VecDeque::new(),
                idle_workers: 0,
                total_workers: 0,
                closed: false,
            }),
            cv: Condvar::new(),
            engine,
            updater,
            stopping,
            stop_event,
            active,
        })
    }

    fn dispatch(self: &Arc<Self>, pipe: Handle) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return;
        }
        state.queue.push_back(pipe);
        // Each waiting worker takes one connection, so another is needed once more wait than workers do.
        // (`idle_workers == 0` misses a worker that was woken but hasn't run yet: a second connection
        // then queues behind the first one's keep-alive, up to IDLE_KEEPALIVE.)
        let spawn = state.queue.len() > state.idle_workers && state.total_workers < MAX_CLIENTS;
        if spawn {
            state.total_workers += 1;
        }
        self.cv.notify_one();
        drop(state);

        if spawn {
            let pool = self.clone();
            if thread::Builder::new()
                .name("pipe-worker".into())
                .spawn(move || pool.worker_loop())
                .is_err()
            {
                let mut state = self.state.lock().unwrap();
                state.total_workers = state.total_workers.saturating_sub(1);
                self.cv.notify_all();
            }
        }
    }

    fn worker_loop(self: Arc<Self>) {
        win::high_qos();
        let io_event = match manual_event() {
            Ok(ev) => ev,
            Err(_) => {
                let mut state = self.state.lock().unwrap();
                state.total_workers = state.total_workers.saturating_sub(1);
                self.cv.notify_all();
                return;
            }
        };
        loop {
            let pipe = {
                let mut state = self.state.lock().unwrap();
                let mut waited = false;
                loop {
                    if let Some(pipe) = state.queue.pop_front() {
                        break Some(pipe);
                    }
                    if state.closed || waited {
                        break None;
                    }
                    state.idle_workers += 1;
                    let (next, timeout) = self.cv.wait_timeout(state, WORKER_IDLE_EXIT).unwrap();
                    state = next;
                    state.idle_workers -= 1;
                    waited = timeout.timed_out();
                }
            };
            let Some(pipe) = pipe else {
                break;
            };
            answer_connection(
                pipe,
                &io_event,
                &self.stop_event,
                &self.stopping,
                &self.active,
                &self.engine,
                self.updater.as_deref(),
            );
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
        let mut state = self.state.lock().unwrap();
        state.total_workers = state.total_workers.saturating_sub(1);
        self.cv.notify_all();
    }

    fn shutdown(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        // Drain queued connections that have not started yet.
        let dropped = state.queue.len();
        state.queue.clear();
        if dropped > 0 {
            self.active.fetch_sub(dropped, Ordering::SeqCst);
        }
        self.cv.notify_all();
        drop(self.cv.wait_while(state, |s| s.total_workers > 0).unwrap());
    }
}

/// Serves `name` with the given DACL. Each connection gets an overlapped pipe instance, which takes
/// FILE_CREATE_PIPE_INSTANCE: SYSTEM and administrators have it, users never do.
pub(crate) fn serve_on(
    name: &str,
    engine: Arc<Engine>,
    sddl: &str,
    updater: Option<Arc<Updater>>,
) -> io::Result<Server> {
    let sd = SecurityDescriptor::from_sddl(sddl)?;
    let mut next = instance_ex(name, &sd, true, true)?;
    let stop_event = Arc::new(manual_event()?);
    let connect_event = manual_event()?;
    let pipe_name = name.to_string();
    let stopping = Arc::new(AtomicBool::new(false));
    let stop = stopping.clone();
    let stop_ev = stop_event.clone();
    let active = Arc::new(AtomicUsize::new(0));
    let pool = WorkerPool::new(
        engine.clone(),
        updater,
        stopping.clone(),
        stop_event.clone(),
        active.clone(),
    );
    // The listener and each worker run at high quality of service, as the pool that searches: in a service,
    // Windows would otherwise keep them to the efficient cores, where a connection could wait its turn.
    let thread = thread::spawn(move || {
        win::high_qos();
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let mut ov = OVERLAPPED {
                hEvent: connect_event.0,
                ..Default::default()
            };
            let _ = unsafe { ResetEvent(connect_event.0) };
            let client = match unsafe { ConnectNamedPipe(next.0, Some(&mut ov)) } {
                Ok(()) => true,
                Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => true,
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                    let handles = [connect_event.0, stop_ev.0];
                    let wait = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                    if wait == WAIT_OBJECT_0 {
                        let mut transferred = 0u32;
                        unsafe { GetOverlappedResult(next.0, &ov, &mut transferred, true) }.is_ok()
                    } else {
                        unsafe {
                            let _ = CancelIoEx(next.0, Some(&ov));
                            let mut transferred = 0u32;
                            let _ = GetOverlappedResult(next.0, &ov, &mut transferred, true);
                        }
                        break;
                    }
                }
                Err(_) => false,
            };
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let pipe = loop {
                match instance_ex(&pipe_name, &sd, false, true) {
                    Ok(fresh) => break std::mem::replace(&mut next, fresh),
                    Err(e) => {
                        engine.log.line(&format!("can't open another pipe instance: {e}"));
                        if stop.load(Ordering::SeqCst) {
                            pool.shutdown();
                            return;
                        }
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            if !client {
                continue;
            }
            if active.load(Ordering::SeqCst) >= MAX_CLIENTS {
                // Small enough for the pipe's buffer, so this never waits on the client.
                let mut busy_io = OverlappedPipe {
                    pipe: &pipe,
                    event: &connect_event,
                    stop: &stop_ev,
                    read_timeout: REQUEST_WAIT,
                    write_timeout: Duration::from_secs(2),
                };
                // No FlushFileBuffers: it waits until the client reads, and a client that never does would
                // stop the listener. Closing (never DisconnectNamedPipe) leaves the message to be read.
                let _ = proto::write_message(
                    &mut busy_io,
                    &Message::Error {
                        message: "The instant search service is busy; try again".into(),
                    },
                );
                continue;
            }
            active.fetch_add(1, Ordering::SeqCst);
            pool.dispatch(pipe);
        }
        pool.shutdown();
    });
    Ok(Server {
        stopping,
        stop_event,
        thread: Some(thread),
    })
}

impl Server {
    pub fn stop(mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        unsafe {
            let _ = SetEvent(self.stop_event.0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Whether the client sends something within `limit` on a synchronous test handle.
#[cfg(test)]
fn request_arrives(pipe: &Handle, limit: Duration) -> bool {
    let started = Instant::now();
    let mut pause = Duration::from_millis(1);
    loop {
        let mut available = 0u32;
        if unsafe { PeekNamedPipe(pipe.0, None, 0, None, Some(&mut available), None) }.is_err() {
            return false;
        }
        if available > 0 {
            return true;
        }
        if started.elapsed() >= limit {
            return false;
        }
        thread::sleep(pause);
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

/// Reads the executable that follows an Update request and hands it to the updater: its version, once
/// it's in place and this service is to stop. Read in full first, so the client hears why if it isn't.
fn receive_update(io: &mut impl Read, signature: &str, size: u64, updater: Option<&Updater>) -> Result<String, String> {
    // One arriving at a time, so connections can't each hold an executable's worth of memory.
    static RECEIVING: AtomicBool = AtomicBool::new(false);
    if size > update::MAX_SIZE {
        return Err(format!("an executable of {size} bytes isn't the service"));
    }
    if RECEIVING.swap(true, Ordering::SeqCst) {
        return Err("another update is arriving".into());
    }
    let mut exe = vec![0u8; size as usize];
    let arrived = io
        .read_exact(&mut exe)
        .map_err(|e| format!("it didn't all arrive: {e}"));
    RECEIVING.store(false, Ordering::SeqCst);
    arrived?;
    updater
        .ok_or("this one doesn't update itself (it isn't running as the installed service)")?
        .take(&exe, signature)
}

/// Whether the client of a connection has gone (closed its end): an app typing fast closes the connection of
/// the search a newer key press replaced, and that search then stops (`Engine::search_until`). The pipe is asked
/// (`PeekNamedPipe`, which fails once the other end is closed) at most every 2 ms, by whichever of the search's
/// threads asks first; nothing else uses the pipe while the search runs.
struct ClientGone {
    pipe: isize,
    began: Instant,
    /// When the pipe was last asked, in µs since `began`.
    asked: AtomicU64,
    gone: AtomicBool,
}

impl ClientGone {
    const EVERY_US: u64 = 2_000;

    fn new(pipe: &Handle) -> Self {
        ClientGone {
            pipe: pipe.0 .0 as isize,
            began: Instant::now(),
            asked: AtomicU64::new(0),
            gone: AtomicBool::new(false),
        }
    }

    fn check(&self) -> bool {
        if self.gone.load(Ordering::Relaxed) {
            return true;
        }
        let now = self.began.elapsed().as_micros() as u64;
        let asked = self.asked.load(Ordering::Relaxed);
        if now.saturating_sub(asked) < Self::EVERY_US
            || self
                .asked
                .compare_exchange(asked, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return false;
        }
        let gone = unsafe { PeekNamedPipe(HANDLE(self.pipe as *mut _), None, 0, None, None, None) }.is_err();
        if gone {
            self.gone.store(true, Ordering::Relaxed);
        }
        gone
    }
}

/// Serves one persistent connection across one or more framed requests until the client closes it,
/// idles out, the server stops, or an update completes.
fn answer_connection(
    pipe: Handle,
    io_event: &Handle,
    stop_event: &Handle,
    stopping: &AtomicBool,
    active: &AtomicUsize,
    engine: &Engine,
    updater: Option<&Updater>,
) {
    let mut io = OverlappedPipe {
        pipe: &pipe,
        event: io_event,
        stop: stop_event,
        read_timeout: REQUEST_WAIT,
        write_timeout: WRITE_TIMEOUT,
    };
    let mut handled = 0usize;
    loop {
        if stopping.load(Ordering::Acquire) {
            break;
        }
        io.read_timeout = if handled == 0 { REQUEST_WAIT } else { IDLE_KEEPALIVE };
        let Ok(request) = proto::read_request(&mut io) else {
            break;
        };
        io.read_timeout = REQUEST_WAIT;
        let mut close_after = false;
        let result = match request {
            Request::Status => proto::write_message(&mut io, &Message::Status(engine.status())),
            Request::Search(request) => {
                let gone = ClientGone::new(&pipe);
                engine.search_until(&request, &mut io, &|| gone.check())
            }
            Request::Sizes { path, count } => {
                let answer = match engine.sizes(&path, count) {
                    Ok(sizes) => Message::Sizes(sizes),
                    Err(message) => Message::Error { message },
                };
                proto::write_message(&mut io, &answer)
            }
            Request::Usage(request) => engine.usage(&request, &mut io),
            Request::List { path } => engine.list(&path, &mut io),
            Request::History(request) => proto::write_message(&mut io, &Message::History(engine.history(&request))),
            Request::Claim => {
                match crate::service::claim() {
                    Ok(true) => engine.log.line(
                        "a copy of Glint that isn't the Microsoft Store's uses the service: it stays when the Store's goes",
                    ),
                    Ok(false) => {}
                    Err(e) => engine
                        .log
                        .line(&format!("couldn't keep the service for a copy that uses it: {e}")),
                }
                proto::write_message(&mut io, &Message::Status(engine.status()))
            }
            Request::Tags => proto::write_message(&mut io, &Message::Tags { tags: engine.tags() }),
            Request::Marks => proto::write_message(&mut io, &Message::Marks(engine.marks())),
            Request::MarksIn { path } => {
                let answer = match engine.marks_in(&path) {
                    Ok(items) => Message::MarksIn { items },
                    Err(message) => Message::Error { message },
                };
                proto::write_message(&mut io, &answer)
            }
            Request::Snapshots => proto::write_message(
                &mut io,
                &match crate::shadow::list() {
                    Ok(snapshots) => Message::Snapshots { snapshots },
                    Err(message) => Message::Error { message },
                },
            ),
            Request::ResolveId { volume_serial, frn } => proto::write_message(
                &mut io,
                &Message::Resolved {
                    path: engine.resolve_id(volume_serial, frn),
                },
            ),
            Request::Update { signature, size } => {
                close_after = true;
                match receive_update(&mut io, &signature, size, updater) {
                    Ok(version) => {
                        engine.log.line(&format!("updating to {version}: restarting into it"));
                        let answered = proto::write_message(&mut io, &Message::Updating { version });
                        // Only once the answer is out: finish-update waits for this process to stop, then
                        // starts the service from the new executable.
                        if let Some(u) = updater {
                            u.stop.set();
                        }
                        answered
                    }
                    Err(message) => {
                        engine.log.line(&format!("didn't take an update: {message}"));
                        proto::write_message(
                            &mut io,
                            &Message::Error {
                                message: format!("The instant search service didn't take the update: {message}"),
                            },
                        )
                    }
                }
            }
        };
        handled += 1;
        match result {
            Ok(()) => {}
            Err(e)
                if e.raw_os_error() == Some(ERROR_NO_DATA.0 as i32)
                    || e.raw_os_error() == Some(ERROR_BROKEN_PIPE.0 as i32) =>
            {
                break;
            }
            Err(e) => {
                engine.log.line(&format!("answering a request failed: {e}"));
                break;
            }
        }
        if close_after || stopping.load(Ordering::Acquire) || active.load(Ordering::Relaxed) >= KEEPALIVE_SOFT_LIMIT {
            break;
        }
    }
    // Dropping `pipe` closes this non-reused server handle, signaling clean ERROR_BROKEN_PIPE
    // EOF to any client (old or new) without discarding buffered bytes. No FlushFileBuffers first:
    // it waits until the client reads, and `Server::stop` waits for this worker.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{self, Error, Event};
    use crate::proto::{HistoryRequest, Kind, SearchRequest, UsageRequest, NO_PARENT};
    use crate::tree::{make_ref, RecordState, Tree, ROOT};
    use crate::usn::{UsnRecord, REASON_CLOSE, REASON_FILE_DELETE, REASON_RENAME_NEW_NAME, REASON_RENAME_OLD_NAME};

    /// T:\Users\me\{report.pdf 1000 B, notes.txt 24 B}
    fn sample() -> Tree {
        let mut tree = Tree::default();
        tree.set_record(
            ROOT,
            &RecordState {
                seq: 5,
                dir: true,
                ..Default::default()
            },
        );
        tree.set_record(
            20,
            &RecordState {
                seq: 1,
                dir: true,
                names: vec![(make_ref(ROOT, 5), "Users".into())],
                ..Default::default()
            },
        );
        tree.set_record(
            21,
            &RecordState {
                seq: 1,
                dir: true,
                names: vec![(make_ref(20, 1), "me".into())],
                ..Default::default()
            },
        );
        tree.set_record(
            22,
            &RecordState {
                seq: 1,
                size: 1000,
                modified: 133_000_000_000_000_000,
                attrs: 0x20,
                names: vec![(make_ref(21, 1), "report.pdf".into())],
                ..Default::default()
            },
        );
        tree.set_record(
            23,
            &RecordState {
                seq: 1,
                size: 24,
                names: vec![(make_ref(21, 1), "notes.txt".into())],
                ..Default::default()
            },
        );
        // A mark of what grew, with nothing changed since.
        tree.growth.mark(1_000, 1024);
        tree
    }

    fn test_pipe(tag: &str) -> String {
        format!(r"\\.\pipe\shadui-index-test-{tag}-{}", std::process::id())
    }

    /// The app ends a search while the service is still scanning (a newer key press): the client's wait ends at once
    /// without an error, and the service's side sees the client has gone.
    #[test]
    fn a_search_the_app_ends_closes_its_pipe_and_the_service_sees_it() {
        let name = test_pipe("cancel");
        let me = win::current_user_sid().unwrap();
        let sd = SecurityDescriptor::from_sddl(&format!("{}(A;;GA;;;{me})", sddl(&[]))).unwrap();
        let pipe = instance(&name, &sd, true).unwrap();
        let cancel = Arc::new(client::Cancel::default());
        let app = {
            let (name, cancel) = (name.clone(), cancel.clone());
            thread::spawn(move || {
                let trust = |_: &str| true;
                let request = SearchRequest {
                    query: "pdf".into(),
                    roots: vec![r"T:\".into()],
                    hidden: false,
                    exclude: vec![],
                    max_results: None,
                    fuzzy: false,
                    rank: false,
                    marks: false,
                    count_only: false,
                };
                let ended =
                    client::search_on_until(client::connect_to(&name, &trust).unwrap(), &request, &cancel, |_| true);
                (ended.is_ok(), Instant::now())
            })
        };
        // The service's side: the request arrives, and no answer comes yet (a scan of millions of names).
        let _ = unsafe { ConnectNamedPipe(pipe.0, None) };
        assert!(request_arrives(&pipe, Duration::from_secs(10)));
        assert!(matches!(
            proto::read_request(&mut PipeIo(&pipe)),
            Ok(Request::Search(_))
        ));
        let gone = ClientGone::new(&pipe);
        thread::sleep(Duration::from_millis(50));
        assert!(!gone.check(), "the app still waits");
        let cancelled = Instant::now();
        cancel.cancel();
        let (ok, ended) = app.join().unwrap();
        assert!(ok, "ending it isn't an error");
        assert!(
            ended.duration_since(cancelled) < Duration::from_secs(2),
            "the wait ended at once"
        );
        let asked = Instant::now();
        while !gone.check() && asked.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(3));
        }
        assert!(gone.check(), "the service sees the client has gone");
    }

    #[test]
    fn answers_over_a_real_pipe() {
        let name = test_pipe("answers");
        let me = win::current_user_sid().unwrap();
        // This test serves the pipe as itself, so it needs the instance-creating right the service has.
        let dacl = format!("{}(A;;GA;;;{me})", sddl(&[]));
        let engine = Engine::with_tree('T', sample());
        // From the journal: report.pdf was draft.pdf a moment ago, and old.txt was deleted.
        let record = |ms: i64, file: u32, reason: u32, attrs: u32, name: &str| UsnRecord {
            usn: ms,
            file: make_ref(file, 1),
            parent: make_ref(21, 1),
            time: ms * 10_000 + 116_444_736_000_000_000,
            reason,
            attrs,
            name: name.into(),
        };
        engine.feed_history(&[
            record(1_000, 22, REASON_RENAME_OLD_NAME, 0x20, "draft.pdf"),
            record(1_001, 22, REASON_RENAME_NEW_NAME | REASON_CLOSE, 0x20, "report.pdf"),
            record(2_000, 30, REASON_FILE_DELETE | REASON_CLOSE, 0x20, "old.txt"),
        ]);
        let server = serve_on(&name, engine, &dacl, None).unwrap();
        // The app refuses a server that isn't SYSTEM or administrators: a test run isn't.
        if !win::is_elevated() {
            assert!(matches!(
                client::connect_to(&name, &client::trusted),
                Err(Error::Untrusted(_))
            ));
        }
        let trust = |_: &str| true;

        let status = client::status_on(client::connect_to(&name, &trust).unwrap()).unwrap();
        assert_eq!(status.volumes.len(), 1);
        assert_eq!(
            (
                status.volumes[0].volume.as_str(),
                status.volumes[0].state.as_str(),
                status.volumes[0].files
            ),
            ("T:", "ready", 2)
        );

        let request = SearchRequest {
            query: "pdf".into(),
            roots: vec![r"T:\".into(), r"Z:\Elsewhere".into()],
            hidden: false,
            exclude: vec![],
            max_results: None,
            fuzzy: false,
            rank: false,
            marks: false,
            count_only: false,
        };
        let (mut plan, mut hits, mut done) = (None, Vec::new(), None);
        client::search_on(client::connect_to(&name, &trust).unwrap(), &request, |event| {
            match event {
                Event::Plan { indexed, skipped } => plan = Some((indexed, skipped)),
                Event::Hits(batch) => hits.extend(batch),
                Event::Done { count, limited, .. } => done = Some((count, limited)),
            }
            true
        })
        .unwrap();
        assert_eq!(
            plan,
            Some((vec![r"T:\".to_string()], vec![r"Z:\Elsewhere".to_string()]))
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(
            (hits[0].path.as_str(), hits[0].size, hits[0].dir, hits[0].modified),
            (r"T:\Users\me\report.pdf", 1000, false, 1_655_526_400_000)
        );
        assert_eq!(done, Some((1, false)));

        // A typo: nothing as typed, so close matches when asked for, said to be so.
        let search = |query: &str, fuzzy, rank| {
            let request = SearchRequest {
                query: query.into(),
                roots: vec![r"T:\".into()],
                hidden: false,
                exclude: vec![],
                max_results: None,
                fuzzy,
                rank,
                marks: false,
                count_only: false,
            };
            let (mut paths, mut done) = (Vec::new(), None);
            client::search_on(client::connect_to(&name, &trust).unwrap(), &request, |event| {
                match event {
                    Event::Hits(batch) => paths.extend(batch.into_iter().map(|h| h.path)),
                    Event::Done { count, fuzzy, .. } => done = Some((count, fuzzy)),
                    Event::Plan { .. } => {}
                }
                true
            })
            .unwrap();
            (paths, done)
        };
        assert_eq!(search("reprot", false, false), (vec![], Some((0, false))));
        assert_eq!(
            search("reprot", true, false),
            (vec![r"T:\Users\me\report.pdf".to_string()], Some((1, true)))
        );
        assert_eq!(
            search("report", true, false),
            (vec![r"T:\Users\me\report.pdf".to_string()], Some((1, false))),
            "as typed first"
        );
        assert_eq!(search("zzzzzz", true, false), (vec![], Some((0, true))));
        // Ranked: the best of every drive's matches, best first, close matches too.
        assert_eq!(
            search("report", false, true),
            (vec![r"T:\Users\me\report.pdf".to_string()], Some((1, false)))
        );
        assert_eq!(
            search("reprot", true, true),
            (vec![r"T:\Users\me\report.pdf".to_string()], Some((1, true)))
        );
        assert_eq!(
            search("*", false, true).0,
            [
                r"T:\Users",
                r"T:\Users\me",
                r"T:\Users\me\notes.txt",
                r"T:\Users\me\report.pdf"
            ],
            "the shorter path first, among names as good"
        );

        let sizes = client::sizes_on(client::connect_to(&name, &trust).unwrap(), r"t:\users", false).unwrap();
        assert_eq!((sizes.total, sizes.sizes), (1024, vec![("me".to_string(), 1024)]));
        assert_eq!((sizes.files, sizes.folders), (None, None), "counted only when asked");
        let counted = client::sizes_on(client::connect_to(&name, &trust).unwrap(), r"t:\users", true).unwrap();
        assert_eq!(
            (counted.total, counted.files, counted.folders),
            (1024, Some(2), Some(1))
        );
        assert!(matches!(
            client::sizes_on(client::connect_to(&name, &trust).unwrap(), r"T:\Nowhere", true),
            Err(Error::Service(_))
        ));

        let map = |path: &str, depth, since| {
            client::usage_on(
                client::connect_to(&name, &trust).unwrap(),
                &UsageRequest {
                    path: path.into(),
                    depth,
                    max_items: 100,
                    since,
                },
            )
        };
        let found = map(r"t:\users", 2, None).unwrap();
        let (nodes, summary) = (&found.nodes, &found.summary);
        let shape: Vec<_> = nodes
            .iter()
            .map(|n| (n.parent, n.name.as_str(), n.dir, n.size))
            .collect();
        assert_eq!(
            shape,
            [
                (NO_PARENT, "Users", true, 1024),
                (0, "me", true, 1024),
                (1, "report.pdf", false, 1000),
                (1, "notes.txt", false, 24)
            ]
        );
        assert_eq!((nodes[2].modified, nodes[2].attrs), (1_655_526_400_000, 0x20));
        assert_eq!(
            (summary.path.as_str(), summary.total, summary.items, summary.limited),
            (r"t:\users", 1024, 4, false)
        );
        assert_eq!((found.previous.as_ref(), summary.growth), (None, None));
        // What each item held at the mark comes along when asked for: the folders as big, report.pdf
        // written since.
        let grew = map(r"t:\users", 2, Some(0)).unwrap();
        assert_eq!(grew.previous, Some(vec![1024, 1024, proto::WRITTEN, 24]));
        assert_eq!(
            grew.summary
                .growth
                .map(|g| (g.since, g.oldest, g.drive_then, g.drive_now)),
            Some((1_000, 1_000, 1024, 1024))
        );
        assert_eq!(
            map(r"T:\", 1, None)
                .unwrap()
                .nodes
                .iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>(),
            ["", "Users"]
        );
        assert!(matches!(
            map(r"T:\Users\me\report.pdf", 2, None),
            Err(Error::Service(_))
        ));
        assert!(matches!(map(r"Z:\", 2, None), Err(Error::Service(_))));

        let list = |path: &str| {
            let hits = client::list_on(client::connect_to(&name, &trust).unwrap(), path)?;
            let mut items: Vec<_> = hits.into_iter().map(|h| (h.path, h.dir, h.size, h.modified)).collect();
            items.sort();
            Ok::<_, Error>(items)
        };
        assert_eq!(
            list(r"t:\users\me").unwrap(),
            [
                (r"T:\Users\me\notes.txt".to_string(), false, 24, 0),
                (r"T:\Users\me\report.pdf".to_string(), false, 1000, 1_655_526_400_000)
            ]
        );
        assert_eq!(list(r"T:\").unwrap(), [(r"T:\Users".to_string(), true, 1024, 0)]);
        assert!(matches!(list(r"T:\Nowhere"), Err(Error::Service(_))));

        // What was renamed, moved and deleted: newest first, with where it is now.
        let history = |query: &str, since| {
            client::history_on(
                client::connect_to(&name, &trust).unwrap(),
                &HistoryRequest {
                    query: query.into(),
                    since,
                    max: 10,
                },
            )
            .unwrap()
        };
        let found = history("", 0);
        let told: Vec<_> = found
            .events
            .iter()
            .map(|e| (e.kind, e.path.as_str(), e.to.as_deref(), e.now.as_deref()))
            .collect();
        assert_eq!(
            told,
            [
                (Kind::Deleted, r"T:\Users\me\old.txt", None, None),
                (
                    Kind::Renamed,
                    r"T:\Users\me\draft.pdf",
                    Some(r"T:\Users\me\report.pdf"),
                    Some(r"T:\Users\me\report.pdf")
                ),
            ]
        );
        assert_eq!((found.limited, found.oldest), (false, 1_001));
        assert_eq!(history("DRAFT", 0).events.len(), 1);
        assert_eq!(history("", 1_500).events.len(), 1);
        // A request the service doesn't know (as List is to one from before it) gets the pipe closed,
        // with no answer: the app takes that as an error, and does without.
        let pipe = client::connect_to(&name, &trust).unwrap();
        let mut io = PipeIo(&pipe);
        let unknown = br#"{"op":"listAll"}"#;
        io.write_all(&(unknown.len() as u32).to_le_bytes()).unwrap();
        io.write_all(unknown).unwrap();
        assert!(proto::read_frame(&mut io, proto::MAX_FRAME).unwrap().is_none());
        server.stop();
    }

    #[test]
    fn refuses_users_it_wasnt_told_about() {
        if win::is_elevated() {
            return; // administrators are always let in
        }
        let name = test_pipe("refuses");
        let server = serve_on(&name, Engine::with_tree('T', sample()), &sddl(&[]), None).unwrap();
        assert!(matches!(
            client::connect_to(&name, &|_: &str| true),
            Err(Error::NoAccess)
        ));
        server.stop();
    }

    #[test]
    fn takes_an_update_signed_with_the_key_over_a_real_pipe() {
        use crate::update::tests::{EXE, KEY, SIGNED};
        use std::path::Path;
        use std::sync::Mutex;
        let me = win::current_user_sid().unwrap();
        let dacl = format!("{}(A;;GA;;;{me})", sddl(&[]));
        let trust = |_: &str| true;
        let dir = std::env::temp_dir().join(format!("shadui-index-pipe-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let installed = dir.join(crate::SERVICE_EXE);
        std::fs::write(&installed, b"the running one").unwrap();

        // A console run (no updater) says it can't; a service from before would close the pipe instead.
        let name = test_pipe("no-update");
        let server = serve_on(&name, Engine::with_tree('T', sample()), &dacl, None).unwrap();
        let said = client::update_on(client::connect_to(&name, &trust).unwrap(), EXE, SIGNED);
        assert!(
            matches!(&said, Err(Error::Service(m)) if m.contains("doesn't update itself")),
            "{said:?}"
        );
        server.stop();

        static STARTED: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());
        fn finisher(exe: &Path) -> io::Result<()> {
            STARTED.lock().unwrap().push(exe.to_path_buf());
            Ok(())
        }
        let updater = Arc::new(Updater::for_test(KEY, dir.clone(), finisher));
        let name = test_pipe("update");
        let server = serve_on(&name, Engine::with_tree('T', sample()), &dacl, Some(updater.clone())).unwrap();
        // Bytes that aren't what was signed: refused, nothing changed, still running.
        let said = client::update_on(client::connect_to(&name, &trust).unwrap(), b"MZ something else", SIGNED);
        assert!(
            matches!(&said, Err(Error::Service(m)) if m.contains("doesn't hold")),
            "{said:?}"
        );
        assert_eq!(std::fs::read(&installed).unwrap(), b"the running one");
        assert!(!updater.stop.is_set());
        // What was signed: in place, finish-update started from it, and the service stopping.
        let said = client::update_on(client::connect_to(&name, &trust).unwrap(), EXE, SIGNED);
        assert_eq!(said.unwrap(), Some("999.0.0".to_string()));
        assert_eq!(std::fs::read(&installed).unwrap(), EXE);
        assert_eq!(*STARTED.lock().unwrap(), std::slice::from_ref(&installed));
        assert!(updater.stop.is_set());
        // One at a time.
        let said = client::update_on(client::connect_to(&name, &trust).unwrap(), EXE, SIGNED);
        assert!(
            matches!(&said, Err(Error::Service(m)) if m.contains("already under way")),
            "{said:?}"
        );
        server.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn users_can_talk_but_never_serve() {
        assert_eq!(USER_ACCESS & 0x4, 0);
        assert_eq!(USER_ACCESS, 0x0012_019B);
        let sddl = sddl(&["S-1-5-21-1-2-3-1001".into(), "S-1-5-21)(A;;GA;;;WD".into()]);
        assert_eq!(sddl, "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;S-1-5-21-1-2-3-1001)");
        assert!(SecurityDescriptor::from_sddl(&sddl).is_ok());
    }

    #[test]
    fn reuses_one_connection_across_requests_and_stops_cleanly_while_idle() {
        let name = test_pipe("persistent");
        let me = win::current_user_sid().unwrap();
        let dacl = format!("{}(A;;GA;;;{me})", sddl(&[]));
        let server = serve_on(&name, Engine::with_tree('T', sample()), &dacl, None).unwrap();
        let pipe = client::connect_to(&name, &|_: &str| true).unwrap();

        // Multiple heterogeneous requests (including a service-level error) over one handle.
        let status = client::status_ref(&pipe).unwrap();
        assert_eq!(status.volumes.len(), 1);

        let sizes = client::sizes_ref(&pipe, r"T:\Users", true).unwrap();
        assert_eq!((sizes.total, sizes.files, sizes.folders), (1024, Some(2), Some(1)));

        assert!(matches!(
            client::sizes_ref(&pipe, r"T:\Missing", false),
            Err(Error::Service(_))
        ));

        let hits = client::list_ref(&pipe, r"T:\Users\me").unwrap();
        assert_eq!(hits.len(), 2);

        let marks = client::marks_in_ref(&pipe, r"T:\Users\me").unwrap();
        assert!(marks.is_empty());

        let again = client::status_ref(&pipe).unwrap();
        assert_eq!(again.volumes[0].files, 2);

        // Server::stop wakes the idle keepalive worker immediately even while the client handle stays open.
        let started = Instant::now();
        server.stop();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "stop should not wait on keepalive timeout"
        );
        drop(pipe);
    }
}
