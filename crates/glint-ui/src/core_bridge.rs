//! glint-core inside the UI: one `host::AppHandle`, its events, its main-thread jobs and its windows.
//!
//! glint-core (the engine moved out of src-tauri) talks to "the app" through `glint_core::host::AppHandle`,
//! the little of Tauri it used. This module makes the one handle at start, keeps it in the [`Core`] global,
//! and connects its three hooks to GPUI:
//!
//! - **Events** (`app.emit(Event::DirChanged(path))` from any thread) go into an `async_channel`; a
//!   foreground task drains it in batches into the [`CoreEvents`] entity, which emits each as a GPUI
//!   event. A view subscribes with [`subscribe`] and matches the [`CoreEvent`] it cares about. Events arrive
//!   far faster than frames, so a handler only records what it needs and calls `cx.notify()`; GPUI coalesces
//!   the redraw to one per frame.
//! - **Main-thread jobs** (`app.run_on_main_thread(job)`) do *not* run inside GPUI's executor, because some
//!   of them open modal Win32 loops (the shell menu, drag-out) and a modal loop inside a GPUI callback
//!   re-enters GPUI while it is borrowed. The UI thread owns a hidden message-only window; the hook posts
//!   each job to it (`PostMessageW`, the boxed job in `LPARAM`) and that window's own WndProc runs it, from
//!   whichever message pump is running. Call [`post_to_ui`] to do the same from UI code.
//! - **Windows**: each GPUI window registers as a `host::Window` ([`register_window`]) so core's
//!   `get_webview_window("main")` and shell dialogs' owner HWND keep working.
//!
//! **Calling core from a view.** Core's `async fn`s are driven on core's own runtime and awaited from a GPUI
//! task, so the UI thread never waits ([`run`]):
//!
//! ```ignore
//! cx.spawn(async move |this, cx| {
//!     let result = core_bridge::run(glint_core::drivehealth::drive_health()).await;
//!     this.update(cx, |this, cx| { this.load = Load::from(result); cx.notify(); }).ok();
//! }).detach();
//! ```
//!
//! Long jobs that report progress take a `host::ipc::Channel`; make one with [`stream`] and read the other
//! end in a GPUI task, updating the view at most once per frame.

use glint_core::host::{self, AppHandle, Manager as _};
use gpui_kit::{App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use std::future::Future;
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, PostMessageW, RegisterClassW, SetForegroundWindow, ShowWindow, HWND_MESSAGE,
    SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WNDCLASSW,
};

/// A closure to run on the UI thread.
type Job = Box<dyn FnOnce() + Send>;

/// Message: run the boxed [`Job`] in `LPARAM`.
const WM_JOB: u32 = WM_APP + 1;
/// Message: quit the app with the exit code in `WPARAM`.
const WM_EXIT: u32 = WM_APP + 2;

/// The message-only window jobs are posted to (0 before [`init`]).
static MESSAGE_WINDOW: AtomicIsize = AtomicIsize::new(0);
/// The code `host::AppHandle::exit` asked for; applied by [`crate::run`] once the app has quit.
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

thread_local! {
    /// The app, for the message window's WndProc to quit it (the WndProc has no `cx`). UI thread only.
    static APP: RefCell<Option<AsyncApp>> = const { RefCell::new(None) };
}

/// The exit code core asked for (0 unless `AppHandle::exit(code)` ran).
pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::Relaxed)
}

/// Everything the UI needs of glint-core, in a GPUI `Global`: `cx.global::<Core>()`.
pub struct Core {
    /// The handle core's functions take (`State`s are registered on it; clone it to move into a job).
    pub app: AppHandle,
    /// Where core's events arrive as GPUI events: `cx.subscribe(&core.events, ...)`.
    pub events: Entity<CoreEvents>,
}

impl Global for Core {}

/// One event core emitted.
pub type CoreEvent = host::Event;

/// The entity core's events are emitted from. It holds nothing: it exists to be subscribed to.
pub struct CoreEvents;

impl EventEmitter<CoreEvent> for CoreEvents {}

/// Start-up step 5: makes the app handle, registers core's states (what src-tauri's `setup` registered with
/// `manage`), starts its background housekeeping and begins draining its events.
pub fn init(cx: &mut App) {
    // A secondary window leaves the housekeeping (crash reports, sweeps of old temp folders) to the main one.
    init_with(cx, !crate::platform::secondary::is_secondary());
}

/// [`init`], with the background housekeeping (crash reports, sweeps of old temp folders…) optional, so tests
/// get the bridge without touching the PC.
pub(crate) fn init_with(cx: &mut App, housekeeping: bool) {
    let message_window = create_message_window();
    let (events_in, events_out) = async_channel::unbounded::<host::Event>();
    let app = AppHandle::new(
        move |event| {
            // Unbounded, so this never blocks the emitting thread; it only fails once the UI has gone.
            let _ = events_in.try_send(event);
        },
        post_to_ui,
        |code| {
            EXIT_CODE.store(code, Ordering::Relaxed);
            post_message(WM_EXIT, WPARAM(0), LPARAM(0));
        },
    );
    APP.with(|slot| *slot.borrow_mut() = Some(cx.to_async()));

    register_states(&app);
    if housekeeping {
        start_housekeeping(&app);
    }

    let events = cx.new(|_| CoreEvents);
    let sink = events.clone();
    cx.spawn(async move |cx| {
        while let Ok(first) = events_out.recv().await {
            // Everything that arrived while the last batch was handled goes out together.
            let mut batch = vec![first];
            while let Ok(more) = events_out.try_recv() {
                batch.push(more);
            }
            sink.update(cx, |_, cx| {
                for event in batch {
                    cx.emit(event);
                }
            });
        }
    })
    .detach();

    // What the old app did when Tauri's run loop ended.
    cx.on_app_quit(|_| async {
        glint_core::archive::finish();
        glint_core::term::close_all();
        glint_core::share::stop_all();
    })
    .detach();

    if message_window.is_none() {
        eprintln!("glint: couldn't create the message window; main-thread jobs from core won't run");
    }
    cx.set_global(Core { app, events });
}

/// What src-tauri's `run()` registered with `manage` (lib.rs): the work in flight, the watchers, the
/// thumbnail service and the tool installs. Without them core's `State<T>` lookups would panic.
fn register_states(app: &AppHandle) {
    app.manage(glint_core::AppState::default());
    app.manage(glint_core::watch::FolderWatcher::default());
    app.manage(glint_core::git::RepoWatcher::default());
    app.manage(glint_core::thumbs::Thumbs::default());
    app.manage(glint_core::tools::Installs::default());
    // NOTE(coordinator): src-tauri also managed `launch::Pending` and `quick::Quick`. Both are window/app
    // modules that stay in the UI crates, so they are registered by the single-instance and quick-search waves.
}

/// What src-tauri's `setup` did on start-up that is not a window: crash reports, the tools folder, secrets'
/// app name, the thumbnail workers and the sweeps of what earlier runs left behind.
fn start_housekeeping(app: &AppHandle) {
    glint_core::startup::begin();
    if let Ok(data) = app.path().app_local_data_dir() {
        glint_core::crash::install(&data);
        glint_core::tools::set_dir(data.join("tools"));
    }
    glint_core::remote::init("Glint");
    app.state::<glint_core::thumbs::Thumbs>().warm_up();
    // Entries that run an exe that's gone are repaired or removed (verbs.rs). Release builds only: a build
    // run from a target folder must not rewrite the registry entries of the installed copy.
    if !cfg!(debug_assertions) {
        let handle = app.clone();
        std::thread::spawn(move || glint_core::verbs::repair(&handle));
    }
    // What earlier runs extracted from archives goes, except what's still on the clipboard.
    std::thread::spawn(|| glint_core::archive::sweep(&glint_core::shell::clipboard_paths()));
    std::thread::spawn(glint_core::share::sweep);
}

// ---- Main-thread jobs: the message-only window -------------------------------------------------

unsafe extern "system" fn bridge_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_JOB => {
            // SAFETY: `post_to_ui` leaked a `Box<Job>` and put its address in LPARAM; this message is delivered
            // once, so the box is reclaimed exactly once.
            let job = unsafe { Box::from_raw(lparam.0 as *mut Job) };
            // A panic must not unwind through this `extern "system"` frame.
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).is_err() {
                eprintln!("glint: a job core posted to the UI thread panicked");
            }
            LRESULT(0)
        }
        WM_EXIT => {
            APP.with(|app| {
                if let Some(app) = app.borrow().as_ref() {
                    app.update(|cx| cx.quit());
                }
            });
            LRESULT(0)
        }
        // SAFETY: forwarding an unhandled message to the default procedure with the arguments we were given.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// Makes the hidden message-only window on the calling (UI) thread and remembers it. `None` if Windows
/// refuses; jobs are then dropped (and said so).
pub(crate) fn create_message_window() -> Option<HWND> {
    // SAFETY: plain window-class registration and creation on this thread; the class and window names are
    // static, and the procedure matches the required signature.
    unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let class = w!("GlintCoreBridge");
        let class_def = WNDCLASSW {
            lpfnWndProc: Some(bridge_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        };
        // Zero means it failed; a second call for the same class also fails, harmlessly (the class exists).
        RegisterClassW(&class_def);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("Glint core bridge"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(module.into()),
            None,
        )
        .ok()?;
        MESSAGE_WINDOW.store(window.0 as isize, Ordering::Release);
        Some(window)
    }
}

fn post_message(message: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
    let window = MESSAGE_WINDOW.load(Ordering::Acquire);
    if window == 0 {
        return false;
    }
    // SAFETY: `window` was made by `create_message_window`; posting to a window is allowed from any thread.
    unsafe { PostMessageW(Some(HWND(window as *mut _)), message, wparam, lparam) }.is_ok()
}

/// Runs `job` on the UI thread, from its message pump and never inside a GPUI callback. Use it to start
/// anything that pumps messages itself (a shell context menu, drag-out): post a job, and let the job call
/// `TrackPopupMenuEx` or `SHDoDragDrop`. Callable from any thread.
pub fn post_to_ui(job: impl FnOnce() + Send + 'static) {
    let boxed: Box<Job> = Box::new(Box::new(job));
    let raw = Box::into_raw(boxed);
    if !post_message(WM_JOB, WPARAM(0), LPARAM(raw as isize)) {
        // SAFETY: the post failed, so nobody else holds `raw`; reclaim it so the job isn't leaked.
        drop(unsafe { Box::from_raw(raw) });
        eprintln!("glint: a job for the UI thread was dropped (no message window)");
    }
}

// ---- Windows --------------------------------------------------------------------------------------

/// The Win32 handle of a GPUI window, once it has one.
pub fn hwnd_of(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Registers `window` with core as `label` ("main", "quick"…), so `get_webview_window(label)` finds it and
/// shell dialogs can use its HWND as their owner. Pair with [`unregister_window`] when it closes.
pub fn register_window(cx: &App, label: &str, window: &Window) {
    let (Some(hwnd), Some(core)) = (hwnd_of(window), cx.try_global::<Core>()) else {
        return;
    };
    let raw = hwnd.0 as isize;
    // These run on whichever thread core calls from. The Win32 calls take any thread; the HWND travels as an integer.
    let handle = move || HWND(raw as *mut _);
    core.app.register_window(host::Window::new(
        core.app.clone(),
        label,
        raw,
        // SAFETY (all three): `raw` is a window handle; a stale one makes these calls fail, never misbehave.
        move || unsafe { ShowWindow(handle(), SW_SHOW) }.as_bool(),
        move || unsafe { SetForegroundWindow(handle()) }.as_bool(),
        move || f64::from(unsafe { GetDpiForWindow(handle()) }) / 96.0,
    ));
}

/// Forgets the window registered as `label`.
pub fn unregister_window(cx: &App, label: &str) {
    if let Some(core) = cx.try_global::<Core>() {
        core.app.unregister_window(label);
    }
}

// ---- Calling core -----------------------------------------------------------------------------------

/// Runs a core future on core's runtime and gives back its answer, awaitable from a GPUI task. The UI thread
/// is never held: core's runtime does the work (its blocking parts on blocking threads), and the GPUI task
/// just waits to be woken. A core task that panicked comes back as an `Err` with the reason, not a crash.
pub fn run<T: Send + 'static>(
    future: impl Future<Output = T> + Send + 'static,
) -> impl Future<Output = Result<T, String>> + 'static {
    let handle = host::async_runtime::spawn(future);
    async move { handle.await.map_err(|error| error.to_string()) }
}

/// A channel for core jobs that stream (progress, results as they come): hand the `Channel` to the core
/// function, read the receiver in a GPUI task. The channel never blocks the sending core thread.
pub fn stream<T: Send + 'static>() -> (host::ipc::Channel<T>, async_channel::Receiver<T>) {
    let (sender, receiver) = async_channel::unbounded();
    let channel = host::ipc::Channel::new(move |message| {
        sender
            .try_send(message)
            .map_err(|_| host::Error::from("the window that asked for this has gone"))
    });
    (channel, receiver)
}

/// One of core's entries as a row: its path (its own, or joined to `folder`), lowercase name and extension made
/// beside it, the directory bit set as Windows sets it on folders, and where it came from (the Recycle Bin's).
pub fn file_entry(
    entry: glint_core::fsx::Entry,
    folder: &str,
    origin: Option<String>,
) -> glint_model::entries::FileEntry {
    use glint_model::entries::{attr, with_details, RawEntry};
    let attrs = entry.attrs | if entry.dir { attr::DIRECTORY } else { 0 };
    with_details(
        RawEntry {
            name: entry.name,
            path: entry.path,
            dir: attrs & attr::DIRECTORY != 0,
            size: entry.size,
            modified: entry.modified,
            attrs,
            origin,
        },
        folder,
    )
}

/// Pixels core read (straight-alpha BGRA rows, what GPUI draws) as a picture, copied once. `None` when they don't
/// fill the size they say.
pub fn picture(pixels: &glint_core::thumbs::Pixels) -> Option<std::sync::Arc<gpui_kit::RenderImage>> {
    image::RgbaImage::from_raw(pixels.width, pixels.height, pixels.bgra.clone())
        .map(|buffer| std::sync::Arc::new(gpui_kit::RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// Lets a view emit a core event to itself in tests and tools: same path as core's own `emit`.
pub fn emit(cx: &App, event: CoreEvent) {
    if let Some(core) = cx.try_global::<Core>() {
        core.app.emit(event);
    }
}

/// Subscribes `view` to core's events: `on_event` runs for each, on the UI thread, in order. Keep the
/// returned subscription in the view (dropping it unsubscribes).
pub fn subscribe<V: 'static>(
    cx: &mut Context<V>,
    mut on_event: impl FnMut(&mut V, &CoreEvent, &mut Context<V>) + 'static,
) -> Option<gpui_kit::Subscription> {
    let events = cx.try_global::<Core>()?.events.clone();
    Some(cx.subscribe(&events, move |view, _, event: &CoreEvent, cx| on_event(view, event, cx)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, MSG, PM_REMOVE};

    /// Pumps this thread's messages until `done`, like the app's message loop does (and a modal loop would).
    fn pump_until(done: impl Fn() -> bool) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !done() && std::time::Instant::now() < deadline {
            let mut message = MSG::default();
            // SAFETY: standard message pump on the thread that owns the window.
            while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                // SAFETY: dispatching a message PeekMessageW just returned.
                unsafe { DispatchMessageW(&message) };
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        done()
    }

    /// The message window is one per process: the tests that make and pump it take turns, or one's window
    /// replaces the other's mid-test and its jobs land on a thread that never pumps them.
    static MESSAGE_WINDOW_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn jobs_posted_from_any_thread_run_on_the_ui_thread_from_the_message_pump() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let _turn = MESSAGE_WINDOW_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(create_message_window().is_some());
        let ui_thread = std::thread::current().id();
        let ran = Arc::new(AtomicBool::new(false));
        let ran_in_job = ran.clone();
        // From a worker thread, as core's threads post. The job must run on *this* thread, once pumped.
        std::thread::spawn(move || {
            post_to_ui(move || {
                assert_eq!(std::thread::current().id(), ui_thread);
                ran_in_job.store(true, Ordering::SeqCst);
            });
        })
        .join()
        .ok();
        assert!(
            !ran.load(Ordering::SeqCst),
            "a job must wait for the pump, not run inside the poster"
        );
        assert!(pump_until(|| ran.load(Ordering::SeqCst)));
    }

    #[test]
    fn a_panicking_job_does_not_take_the_ui_down() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let _turn = MESSAGE_WINDOW_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(create_message_window().is_some());
        let after = Arc::new(AtomicBool::new(false));
        let after_in_job = after.clone();
        post_to_ui(|| panic!("a job that fails"));
        post_to_ui(move || after_in_job.store(true, Ordering::SeqCst));
        assert!(
            pump_until(|| after.load(Ordering::SeqCst)),
            "the job after the panicking one still runs"
        );
    }

    #[test]
    fn core_futures_run_off_the_calling_thread_and_answer() {
        let caller = std::thread::current().id();
        let answer = host::async_runtime::block_on(run(async move { std::thread::current().id() != caller }));
        assert_eq!(answer, Ok(true));
        let failed = host::async_runtime::block_on(run(async { panic!("boom") }));
        let failed: Result<(), String> = failed;
        assert!(failed.is_err());
    }

    #[test]
    fn streams_deliver_in_order_and_fail_once_the_reader_is_gone() {
        let (channel, receiver) = stream::<u32>();
        for n in 0..3 {
            assert!(channel.send(n).is_ok());
        }
        assert_eq!(receiver.try_recv().ok(), Some(0));
        assert_eq!(receiver.try_recv().ok(), Some(1));
        assert_eq!(receiver.try_recv().ok(), Some(2));
        drop(receiver);
        assert!(channel.send(9).is_err());
    }

    #[gpui_kit::test]
    fn core_events_reach_subscribers_as_gpui_events(cx: &mut gpui_kit::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;
        let _guard = crate::testing::lock();
        let seen = Rc::new(RefCell::new(Vec::<String>::new()));
        cx.update(|cx| {
            crate::testing::init(cx);
            let events = cx.global::<Core>().events.clone();
            let seen = seen.clone();
            cx.subscribe(&events, move |_, event: &CoreEvent, _| {
                seen.borrow_mut().push(match event {
                    CoreEvent::DirChanged(path) => path.clone(),
                    CoreEvent::CopyQueue(jobs) => format!("{} jobs", jobs.len()),
                    _ => "something else".into(),
                })
            })
            .detach();
            // From another thread, as core's watchers emit.
            let app = cx.global::<Core>().app.clone();
            std::thread::spawn(move || {
                app.emit(CoreEvent::DirChanged("C:\\".into()));
                app.emit(CoreEvent::CopyQueue(Vec::new()));
            })
            .join()
            .ok();
        });
        for _ in 0..50 {
            cx.run_until_parked();
            if seen.borrow().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(*seen.borrow(), vec!["C:\\".to_string(), "0 jobs".to_string()]);
    }
}
