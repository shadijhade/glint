//! The named pipe the service answers on. Only SYSTEM, administrators and the users who turned
//! instant search on may open it, and none of them may create an instance of it, so no other
//! process can pose as the service.

use crate::engine::Engine;
use crate::proto::{self, Message, Request};
use crate::win::{self, os_error, wide, Handle, SecurityDescriptor};
use std::io::{self, Read, Write};
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PeekNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};

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

impl Read for PipeIo<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0;
        let len = buf.len().min(u32::MAX as usize);
        match unsafe { ReadFile(self.0 .0, Some(&mut buf[..len]), Some(&mut read), None) } {
            Ok(()) => Ok(read as usize),
            Err(e) if e.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
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
    name: String,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

fn instance(name: &str, sd: &SecurityDescriptor, first: bool) -> io::Result<Handle> {
    let attributes = sd.attributes();
    // The first instance must be ours: if anything else already serves the name, fail.
    let mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { FILE_FLAGS_AND_ATTRIBUTES(0) };
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide(name).as_ptr()),
            mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            1 << 16,
            1 << 16,
            0,
            Some(&attributes),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(Handle(handle))
}

pub fn serve(engine: Arc<Engine>, allowed: &[String]) -> io::Result<Server> {
    let sddl = sddl(allowed);
    serve_on(crate::PIPE_NAME, engine.clone(), &sddl).or_else(|e| {
        engine.log.line(&format!("can't serve under the protected prefix ({e}); serving on {}", crate::LEGACY_PIPE_NAME));
        serve_on(crate::LEGACY_PIPE_NAME, engine, &sddl)
    })
}

/// Connections answered at once. More (a stuck or hostile local program opening many) are told the
/// service is busy instead of each getting a thread.
const MAX_CLIENTS: usize = 16;

/// How long a client has to send its request once connected.
const REQUEST_WAIT: Duration = Duration::from_secs(10);

/// Serves `name` with the given DACL. Each connection gets a fresh pipe instance, which takes
/// FILE_CREATE_PIPE_INSTANCE: SYSTEM and administrators have it, users never do.
pub(crate) fn serve_on(name: &str, engine: Arc<Engine>, sddl: &str) -> io::Result<Server> {
    let sd = SecurityDescriptor::from_sddl(sddl)?;
    let mut next = instance(name, &sd, true)?;
    let pipe_name = name.to_string();
    let stopping = Arc::new(AtomicBool::new(false));
    let stop = stopping.clone();
    let active = Arc::new(AtomicUsize::new(0));
    let thread = thread::spawn(move || loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let connected = unsafe { ConnectNamedPipe(next.0, None) };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let client = connected.is_ok() || connected.as_ref().is_err_and(|e| e.code() == ERROR_PIPE_CONNECTED.to_hresult());
        let pipe = loop {
            match instance(&pipe_name, &sd, false) {
                Ok(fresh) => break std::mem::replace(&mut next, fresh),
                Err(e) => {
                    engine.log.line(&format!("can't open another pipe instance: {e}"));
                    thread::sleep(Duration::from_secs(1));
                }
            }
        };
        if !client {
            continue;
        }
        if active.load(Ordering::SeqCst) >= MAX_CLIENTS {
            // Small enough for the pipe's buffer, so this never waits on the client.
            let _ = proto::write_message(&mut PipeIo(&pipe), &Message::Error { message: "The instant search service is busy; try again".into() });
            continue;
        }
        active.fetch_add(1, Ordering::SeqCst);
        let (engine, active) = (engine.clone(), active.clone());
        thread::spawn(move || {
            answer(pipe, &engine);
            active.fetch_sub(1, Ordering::SeqCst);
        });
    });
    Ok(Server { name: name.to_string(), stopping, thread: Some(thread) })
}

impl Server {
    pub fn stop(mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        let Some(thread) = self.thread.take() else { return };
        // Wake the listener from ConnectNamedPipe so it sees it should stop: cancel the wait, or
        // connect once (the canceling can miss a listener just about to wait again).
        while !thread.is_finished() {
            unsafe {
                let _ = CancelSynchronousIo(HANDLE(thread.as_raw_handle()));
            }
            let _wake = unsafe {
                CreateFileW(PCWSTR(wide(&self.name).as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, Default::default(), None)
            }
            .map(Handle);
            thread::sleep(Duration::from_millis(5));
        }
        let _ = thread.join();
    }
}

/// Whether the client sends something within `limit`: one that connects and stays silent mustn't
/// hold a connection for good. Checks at once, then ever less often (requests come right away).
/// ponytail: a client that asks and then never reads its (large) answer still holds one until it
/// goes; MAX_CLIENTS bounds them, overlapped writes with a timeout would free them too.
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

/// Serves one connection: one request, its answer, then the pipe is closed.
fn answer(pipe: Handle, engine: &Engine) {
    if !request_arrives(&pipe, REQUEST_WAIT) {
        return;
    }
    let mut io = PipeIo(&pipe);
    let Ok(request) = proto::read_request(&mut io) else { return };
    let result = match request {
        Request::Status => proto::write_message(&mut io, &Message::Status(engine.status())),
        Request::Search(request) => engine.search(&request, &mut io),
        Request::Sizes { path } => {
            let answer = match engine.sizes(&path) {
                Ok(sizes) => Message::Sizes(sizes),
                Err(message) => Message::Error { message },
            };
            proto::write_message(&mut io, &answer)
        }
        Request::Usage(request) => engine.usage(&request, &mut io),
    };
    match result {
        // Let the client read everything before the pipe goes away.
        Ok(()) => unsafe {
            let _ = FlushFileBuffers(pipe.0);
        },
        Err(e) if e.raw_os_error() == Some(ERROR_NO_DATA.0 as i32) || e.raw_os_error() == Some(ERROR_BROKEN_PIPE.0 as i32) => {}
        Err(e) => engine.log.line(&format!("answering a request failed: {e}")),
    }
    unsafe {
        let _ = DisconnectNamedPipe(pipe.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{self, Error, Event};
    use crate::proto::{SearchRequest, UsageRequest, NO_PARENT};
    use crate::tree::{make_ref, RecordState, Tree, ROOT};

    /// T:\Users\me\{report.pdf 1000 B, notes.txt 24 B}
    fn sample() -> Tree {
        let mut tree = Tree::default();
        tree.set_record(ROOT, &RecordState { seq: 5, dir: true, ..Default::default() });
        tree.set_record(20, &RecordState { seq: 1, dir: true, names: vec![(make_ref(ROOT, 5), "Users".into())], ..Default::default() });
        tree.set_record(21, &RecordState { seq: 1, dir: true, names: vec![(make_ref(20, 1), "me".into())], ..Default::default() });
        tree.set_record(22, &RecordState { seq: 1, size: 1000, modified: 133_000_000_000_000_000, attrs: 0x20, names: vec![(make_ref(21, 1), "report.pdf".into())], ..Default::default() });
        tree.set_record(23, &RecordState { seq: 1, size: 24, names: vec![(make_ref(21, 1), "notes.txt".into())], ..Default::default() });
        tree
    }

    fn test_pipe(tag: &str) -> String {
        format!(r"\\.\pipe\shadui-index-test-{tag}-{}", std::process::id())
    }

    #[test]
    fn answers_over_a_real_pipe() {
        let name = test_pipe("answers");
        let me = win::current_user_sid().unwrap();
        // This test serves the pipe as itself, so it needs the instance-creating right the service has.
        let dacl = format!("{}(A;;GA;;;{me})", sddl(&[]));
        let server = serve_on(&name, Engine::with_tree('T', sample()), &dacl).unwrap();
        // The app refuses a server that isn't SYSTEM or administrators: a test run isn't.
        if !win::is_elevated() {
            assert!(matches!(client::connect_to(&name, &client::trusted), Err(Error::Untrusted(_))));
        }
        let trust = |_: &str| true;

        let status = client::status_on(client::connect_to(&name, &trust).unwrap()).unwrap();
        assert_eq!(status.volumes.len(), 1);
        assert_eq!((status.volumes[0].volume.as_str(), status.volumes[0].state.as_str(), status.volumes[0].files), ("T:", "ready", 2));

        let request = SearchRequest { query: "pdf".into(), roots: vec![r"T:\".into(), r"Z:\Elsewhere".into()], hidden: false, exclude: vec![], max_results: None };
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
        assert_eq!(plan, Some((vec![r"T:\".to_string()], vec![r"Z:\Elsewhere".to_string()])));
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].path.as_str(), hits[0].size, hits[0].dir, hits[0].modified), (r"T:\Users\me\report.pdf", 1000, false, 1_655_526_400_000));
        assert_eq!(done, Some((1, false)));

        let sizes = client::sizes_on(client::connect_to(&name, &trust).unwrap(), r"t:\users").unwrap();
        assert_eq!((sizes.total, sizes.sizes), (1024, vec![("me".to_string(), 1024)]));
        assert!(matches!(client::sizes_on(client::connect_to(&name, &trust).unwrap(), r"T:\Nowhere"), Err(Error::Service(_))));

        let map = |path: &str, depth| client::usage_on(client::connect_to(&name, &trust).unwrap(), &UsageRequest { path: path.into(), depth, max_items: 100 });
        let (nodes, summary) = map(r"t:\users", 2).unwrap();
        let shape: Vec<_> = nodes.iter().map(|n| (n.parent, n.name.as_str(), n.dir, n.size)).collect();
        assert_eq!(shape, [(NO_PARENT, "Users", true, 1024), (0, "me", true, 1024), (1, "report.pdf", false, 1000), (1, "notes.txt", false, 24)]);
        assert_eq!((nodes[2].modified, nodes[2].attrs), (1_655_526_400_000, 0x20));
        assert_eq!((summary.path.as_str(), summary.total, summary.items, summary.limited), (r"t:\users", 1024, 4, false));
        assert_eq!(map(r"T:\", 1).unwrap().0.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["", "Users"]);
        assert!(matches!(map(r"T:\Users\me\report.pdf", 2), Err(Error::Service(_))));
        assert!(matches!(map(r"Z:\", 2), Err(Error::Service(_))));
        server.stop();
    }

    #[test]
    fn refuses_users_it_wasnt_told_about() {
        if win::is_elevated() {
            return; // administrators are always let in
        }
        let name = test_pipe("refuses");
        let server = serve_on(&name, Engine::with_tree('T', sample()), &sddl(&[])).unwrap();
        assert!(matches!(client::connect_to(&name, &|_: &str| true), Err(Error::NoAccess)));
        server.stop();
    }

    #[test]
    fn users_can_talk_but_never_serve() {
        assert_eq!(USER_ACCESS & 0x4, 0);
        assert_eq!(USER_ACCESS, 0x0012_019B);
        let sddl = sddl(&["S-1-5-21-1-2-3-1001".into(), "S-1-5-21)(A;;GA;;;WD".into()]);
        assert_eq!(sddl, "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x12019b;;;S-1-5-21-1-2-3-1001)");
        assert!(SecurityDescriptor::from_sddl(&sddl).is_ok());
    }
}
