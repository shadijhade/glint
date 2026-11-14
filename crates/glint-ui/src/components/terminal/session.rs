//! A shell running in a pseudo console, as the UI reaches it: glint-core's `term.rs` behind a seam.
//!
//! The PTY is core's (`term_shells`, `term_spawn` with its output channel, `term_write`, `term_resize`,
//! `term_close`); this module is the little Glint's UI needs around it:
//!
//! - [`Backend`] says how those five are called. [`Backend::core`] calls glint-core; tests give a fake that
//!   records writes and lets the test push output, so no shell is started.
//! - [`output_channel`] makes the `Channel` core sends a session's output on. The bytes go straight into the
//!   [`Screen`](super::emu::Screen) on core's own reader thread (the parse is never on the UI thread), the
//!   folder and exit reports become [`SessionEvent`]s.
//! - [`Outbox`] is the one way anything is written to a shell. A writer thread owns it: key presses, pastes and
//!   the terminal's own answers queue to it and are written in order, and the window never waits on a pipe.
//! - [`Waker`] turns "new output" from any thread into at most one redraw per frame.

use super::emu::Screen;
use crate::core_bridge;
use glint_core::host::ipc::Channel;
use glint_core::term::TermOutput;
use gpui_kit::{App, Global};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

/// A future the view awaits on the UI thread.
pub type Fut<T> = Pin<Box<dyn Future<Output = T>>>;

/// A shell the pane can run (`Shell` in terminal.ts): `id` is "pwsh", "powershell", "cmd", "nu", "bash" or "wsl".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    pub id: String,
    pub name: String,
    pub path: String,
}

/// What a session needs to start.
pub struct Spawn {
    /// A [`Shell::id`].
    pub shell: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    /// Where core sends the session's output and reports.
    pub output: Channel<TermOutput>,
}

/// What happens in a session besides its output (`TermEvent` in terminal.ts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEvent {
    /// Its prompt said it is in this folder.
    Cwd(String),
    /// The shell ended with this code.
    Exit(u32),
}

/// How the terminal reaches glint-core's sessions.
#[derive(Clone)]
pub struct Backend {
    /// The shells on this PC (`term_shells`).
    pub shells: Rc<dyn Fn() -> Fut<Vec<Shell>>>,
    /// Starts one; resolves to its id (`term_spawn`).
    pub spawn: Rc<dyn Fn(Spawn) -> Fut<Result<u32, String>>>,
    /// Types into a session (`term_write`). May block on a pipe: only the writer thread calls it.
    pub write: Arc<dyn Fn(u32, String) + Send + Sync>,
    /// Tells a session its size (`term_resize`).
    pub resize: Arc<dyn Fn(u32, u16, u16) + Send + Sync>,
    /// Ends a session's shell (`term_close`).
    pub close: Arc<dyn Fn(u32) + Send + Sync>,
}

impl Backend {
    /// glint-core's terminal sessions.
    pub fn core() -> Self {
        Backend {
            shells: Rc::new(|| {
                let running = core_bridge::run(glint_core::term::term_shells());
                Box::pin(async move {
                    running
                        .await
                        .and_then(|shells| shells)
                        .map(|shells| {
                            shells
                                .into_iter()
                                .map(|s| Shell {
                                    id: s.id.to_string(),
                                    name: s.name.to_string(),
                                    path: s.path,
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                })
            }),
            spawn: Rc::new(|job| {
                let running = core_bridge::run(glint_core::term::term_spawn(
                    job.shell, job.cwd, job.cols, job.rows, job.output,
                ));
                Box::pin(async move { running.await.and_then(|started| started) })
            }),
            write: Arc::new(|id, data| {
                // A session that has closed says so; there is nobody to tell.
                let _ = glint_core::term::term_write(id, data);
            }),
            resize: Arc::new(|id, cols, rows| {
                let _ = glint_core::term::term_resize(id, cols, rows);
            }),
            close: Arc::new(glint_core::term::term_close),
        }
    }
}

struct Shared(Backend);

impl Global for Shared {}

/// Sets the backend the terminals use (tests; the app uses [`Backend::core`] without this).
pub fn init_with(cx: &mut App, backend: Backend) {
    cx.set_global(Shared(backend));
}

/// The backend in use: glint-core's unless [`init_with`] set another.
pub fn backend(cx: &mut App) -> Backend {
    if let Some(shared) = cx.try_global::<Shared>() {
        return shared.0.clone();
    }
    init_with(cx, Backend::core());
    cx.global::<Shared>().0.clone()
}

// ---- Writing --------------------------------------------------------------------------------------------------

/// One thing for the writer thread to do, in order.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Write(String),
    Resize(u16, u16),
    Close,
}

/// The queue of what is to be written to a shell. Cloneable and cheap; the writer thread drains it once the
/// session has an id (what is queued before that waits, in order).
#[derive(Clone)]
pub struct Outbox {
    tx: mpsc::Sender<Command>,
}

impl Outbox {
    /// A queue and the end the writer thread reads.
    pub fn new() -> (Outbox, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel();
        (Outbox { tx }, rx)
    }

    /// Types `text` into the shell.
    pub fn write(&self, text: impl Into<String>) {
        let _ = self.tx.send(Command::Write(text.into()));
    }

    /// Tells the shell the new size.
    pub fn resize(&self, cols: u16, rows: u16) {
        let _ = self.tx.send(Command::Resize(cols, rows));
    }

    /// Ends the shell and the writer thread.
    pub fn close(&self) {
        let _ = self.tx.send(Command::Close);
    }
}

/// Starts the thread that writes `commands` to session `id`: `term_write` and `term_resize` can wait on the
/// pseudo console's pipe, which the window must never do. A size change that has newer ones queued behind it is
/// skipped (dragging the divider asks for dozens), and the thread ends on `Close`.
pub fn start_writer(id: u32, commands: mpsc::Receiver<Command>, backend: Backend) {
    let close = backend.close.clone();
    let spawned = std::thread::Builder::new()
        .name("glint-term-write".into())
        .spawn(move || {
            let mut next = commands.recv().ok();
            while let Some(command) = next.take() {
                match command {
                    Command::Write(text) => (backend.write)(id, text),
                    Command::Resize(cols, rows) => {
                        let (mut cols, mut rows) = (cols, rows);
                        // Only the last of a run of resizes is worth telling the console.
                        loop {
                            match commands.try_recv() {
                                Ok(Command::Resize(c, r)) => (cols, rows) = (c, r),
                                Ok(other) => {
                                    (backend.resize)(id, cols, rows);
                                    next = Some(other);
                                    break;
                                }
                                Err(_) => {
                                    (backend.resize)(id, cols, rows);
                                    break;
                                }
                            }
                        }
                        if next.is_some() {
                            continue;
                        }
                    }
                    Command::Close => {
                        (backend.close)(id);
                        return;
                    }
                }
                next = commands.recv().ok();
            }
            // Every sender is gone without a Close: the view is gone, and so is the shell's reason to run.
            (backend.close)(id);
        });
    if let Err(e) = spawned {
        eprintln!("glint: couldn't start the terminal's writer thread: {e}");
        close(id);
    }
}

// ---- Output ---------------------------------------------------------------------------------------------------

/// What the thread that feeds a screen tells the view's task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Msg {
    /// New output: redraw (at most once a frame).
    Wake,
    /// Something besides cells: the title, a copy request, the bell.
    Signal(super::emu::Signal),
    /// The session reported (folder, exit).
    Session(SessionEvent),
    /// The element measured the grid at this many columns and rows.
    Resized(u16, u16),
}

/// "There is new output", from any thread, as one redraw per frame: the first call after a redraw sends a
/// [`Msg::Wake`]; the ones before the next redraw are free. The view's task calls [`Waker::rearm`] as it redraws.
#[derive(Clone)]
pub struct Waker {
    pending: Arc<AtomicBool>,
    tx: async_channel::Sender<Msg>,
}

impl Waker {
    pub fn new(tx: async_channel::Sender<Msg>) -> Self {
        Waker {
            pending: Arc::new(AtomicBool::new(false)),
            tx,
        }
    }

    /// New output is there.
    pub fn wake(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            let _ = self.tx.try_send(Msg::Wake);
        }
    }

    /// The redraw is about to be done: the next output wakes again.
    pub fn rearm(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

/// The channel core sends a session's output on. Bytes are parsed into `screen` right here (on core's reader
/// thread); reports go to `msgs`.
pub fn output_channel(screen: Screen, msgs: async_channel::Sender<Msg>) -> Channel<TermOutput> {
    Channel::new(move |output| {
        match output {
            TermOutput::Data(bytes) => screen.feed(&bytes),
            TermOutput::Cwd(path) => {
                let _ = msgs.try_send(Msg::Session(SessionEvent::Cwd(path)));
            }
            TermOutput::Exit(code) => {
                let _ = msgs.try_send(Msg::Session(SessionEvent::Exit(code)));
            }
        }
        Ok(())
    })
}

#[cfg(test)]
pub(crate) mod fake {
    //! A backend with no shell: what the UI wrote, what it resized, and a handle to push output.

    use super::*;
    use std::sync::Mutex;

    /// What a fake backend saw, and the channels it was given.
    #[derive(Default)]
    pub struct Log {
        pub writes: Mutex<Vec<(u32, String)>>,
        pub resizes: Mutex<Vec<(u32, u16, u16)>>,
        pub closed: Mutex<Vec<u32>>,
        pub spawned: Mutex<Vec<(String, String, u16, u16)>>,
        pub outputs: Mutex<Vec<Channel<TermOutput>>>,
    }

    impl Log {
        pub fn written(&self) -> String {
            self.writes
                .lock()
                .map(|w| w.iter().map(|(_, s)| s.as_str()).collect())
                .unwrap_or_default()
        }

        /// Sends bytes as the shell's output on the last session started.
        pub fn print(&self, text: &str) {
            if let Some(output) = self.outputs.lock().ok().and_then(|o| o.last().cloned()) {
                let _ = output.send(TermOutput::Data(text.as_bytes().to_vec()));
            }
        }

        /// Says the shell's prompt is in `path`, as core would.
        pub fn cwd(&self, path: &str) {
            if let Some(output) = self.outputs.lock().ok().and_then(|o| o.last().cloned()) {
                let _ = output.send(TermOutput::Cwd(path.to_string()));
            }
        }

        /// Says the shell ended with `code`, as core would.
        pub fn exit(&self, code: u32) {
            if let Some(output) = self.outputs.lock().ok().and_then(|o| o.last().cloned()) {
                let _ = output.send(TermOutput::Exit(code));
            }
        }
    }

    /// A backend that starts nothing; `shells` are what `term_shells` would find.
    pub fn backend(shells: &[(&str, &str)]) -> (Backend, Arc<Log>) {
        let log = Arc::new(Log::default());
        let found: Vec<Shell> = shells
            .iter()
            .map(|(id, name)| Shell {
                id: id.to_string(),
                name: name.to_string(),
                path: format!("C:\\{id}.exe"),
            })
            .collect();
        let next = Arc::new(std::sync::atomic::AtomicU32::new(1));
        let spawn_log = log.clone();
        let (w, r, c) = (log.clone(), log.clone(), log.clone());
        let backend = Backend {
            shells: Rc::new(move || {
                let found = found.clone();
                Box::pin(async move { found })
            }),
            spawn: Rc::new(move |job| {
                if let Ok(mut spawned) = spawn_log.spawned.lock() {
                    spawned.push((job.shell.clone(), job.cwd.clone(), job.cols, job.rows));
                }
                if let Ok(mut outputs) = spawn_log.outputs.lock() {
                    outputs.push(job.output);
                }
                let id = next.fetch_add(1, Ordering::Relaxed);
                Box::pin(async move { Ok(id) })
            }),
            write: Arc::new(move |id, text| {
                if let Ok(mut writes) = w.writes.lock() {
                    writes.push((id, text));
                }
            }),
            resize: Arc::new(move |id, cols, rows| {
                if let Ok(mut resizes) = r.resizes.lock() {
                    resizes.push((id, cols, rows));
                }
            }),
            close: Arc::new(move |id| {
                if let Ok(mut closed) = c.closed.lock() {
                    closed.push(id);
                }
            }),
        };
        (backend, log)
    }
}

#[cfg(test)]
mod tests {
    use super::fake;
    use super::*;
    use crate::components::terminal::emu::Hooks;
    use std::time::{Duration, Instant};

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        done()
    }

    #[test]
    fn the_writer_writes_in_order_collapses_resizes_and_closes() {
        let (backend, log) = fake::backend(&[]);
        let (outbox, commands) = Outbox::new();
        // Queued before the session has an id.
        outbox.write("ls\r");
        outbox.resize(80, 24);
        outbox.resize(100, 30);
        outbox.resize(120, 40);
        outbox.write("pwd\r");
        start_writer(7, commands, backend);
        assert!(wait_until(|| log.written() == "ls\rpwd\r"));
        assert!(wait_until(|| !log.resizes.lock().map(|r| r.is_empty()).unwrap_or(true)));
        assert_eq!(
            *log.resizes.lock().unwrap(),
            vec![(7, 120, 40)],
            "only the last size is sent"
        );
        assert_eq!(log.writes.lock().unwrap()[0].0, 7);
        outbox.close();
        assert!(wait_until(|| *log.closed.lock().unwrap() == vec![7]));
    }

    #[test]
    fn the_writer_closes_the_shell_when_every_sender_is_gone() {
        let (backend, log) = fake::backend(&[]);
        let (outbox, commands) = Outbox::new();
        start_writer(3, commands, backend);
        drop(outbox);
        assert!(wait_until(|| *log.closed.lock().unwrap() == vec![3]));
    }

    #[test]
    fn output_is_parsed_on_the_calling_thread_and_wakes_once_per_frame() {
        let (tx, rx) = async_channel::unbounded();
        let waker = Waker::new(tx.clone());
        let wake = waker.clone();
        let hooks = Hooks {
            wake: Arc::new(move || wake.wake()),
            ..Hooks::none()
        };
        let screen = Screen::new(80, 4, hooks);
        let channel = output_channel(screen.clone(), tx);
        for _ in 0..50 {
            channel.send(TermOutput::Data(b"x".to_vec())).unwrap();
        }
        channel.send(TermOutput::Cwd("D:\\".into())).unwrap();
        screen.with(|e| assert_eq!(e.visible_text()[0], "x".repeat(50)));
        let mut wakes = 0;
        let mut reports = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Wake => wakes += 1,
                Msg::Session(event) => reports.push(event),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(wakes, 1, "50 chunks, one wake");
        assert_eq!(reports, vec![SessionEvent::Cwd("D:\\".into())]);
        waker.rearm();
        channel.send(TermOutput::Data(b"y".to_vec())).unwrap();
        assert!(
            matches!(rx.try_recv(), Ok(Msg::Wake)),
            "the next output after a redraw wakes again"
        );
    }
}
