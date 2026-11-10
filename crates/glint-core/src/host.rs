//! The little of Tauri that Glint's Rust used, rebuilt without Tauri, so the modules moved from
//! src-tauri keep their code: `tauri::` becomes `crate::host::` and nothing else has to change.
//!
//! The GPUI app makes one [`AppHandle`] at start with two hooks: where events go (the UI subscribes to
//! them) and how to run something on the UI thread. Commands become plain functions; their
//! `Channel`s are callbacks the caller passes in; `State` is what the app registered with `manage`.
//! Window-specific calls go through [`Window`], which the UI registers for each of its windows.

use serde::Serialize;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Tauri's bundle identifier, kept so settings, caches and indexes stay where users already have them.
pub const IDENTIFIER: &str = "com.shado.shadui-ripgrep";
/// The Microsoft Store copy's identifier (the MSIX build's own in Tauri, `tauri.msix.conf.json`), so its folders
/// stay where the Store copy already has them and are never shared with a copy the setup installed on the same PC.
pub const STORE_IDENTIFIER: &str = "com.shado.glint.store";

/// The identifier this copy's folders are named after: the Store copy's when it runs from an MSIX package.
pub fn identifier() -> &'static str {
    if crate::packaged() {
        STORE_IDENTIFIER
    } else {
        IDENTIFIER
    }
}

/// What went wrong, as text (Tauri's errors were only ever shown or logged).
#[derive(Debug, Clone)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(text: String) -> Self {
        Self(text)
    }
}

impl From<&str> for Error {
    fn from(text: &str) -> Self {
        Self(text.to_string())
    }
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.0
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// An event for the UI: its name (as the web UI listened for it), the window it's for (none: every
/// window), and its payload as JSON.
#[derive(Debug, Clone)]
pub struct Event {
    pub name: String,
    pub target: Option<String>,
    pub payload: serde_json::Value,
}

type EventSink = dyn Fn(Event) + Send + Sync;
type MainThread = dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync;

struct Inner {
    events: Box<EventSink>,
    main_thread: Box<MainThread>,
    states: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    windows: RwLock<HashMap<String, Window>>,
    exit: Box<dyn Fn(i32) + Send + Sync>,
}

/// The app, as the moved modules see it. Cheap to clone.
#[derive(Clone)]
pub struct AppHandle(Arc<Inner>);

impl AppHandle {
    /// `events` receives every `emit`; `main_thread` runs a closure on the UI thread; `exit` ends the app.
    pub fn new(
        events: impl Fn(Event) + Send + Sync + 'static,
        main_thread: impl Fn(Box<dyn FnOnce() + Send>) + Send + Sync + 'static,
        exit: impl Fn(i32) + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Inner {
            events: Box::new(events),
            main_thread: Box::new(main_thread),
            states: RwLock::default(),
            windows: RwLock::default(),
            exit: Box::new(exit),
        }))
    }

    /// An app whose events go nowhere and whose "UI thread" is the caller's, for tests and tools.
    pub fn headless() -> Self {
        Self::new(|_| {}, |run| run(), |code| std::process::exit(code))
    }

    /// Registers the window the UI opened under `label` ("main", "quick"…).
    pub fn register_window(&self, window: Window) {
        if let Ok(mut windows) = self.0.windows.write() {
            windows.insert(window.label.clone(), window);
        }
    }

    pub fn unregister_window(&self, label: &str) {
        if let Ok(mut windows) = self.0.windows.write() {
            windows.remove(label);
        }
    }
}

/// What a module registered with `manage`, borrowed for a call.
pub struct State<'a, T: Send + Sync + 'static>(Arc<T>, std::marker::PhantomData<&'a T>);

impl<T: Send + Sync + 'static> State<'_, T> {
    pub fn inner(&self) -> &T {
        &self.0
    }
}

impl<T: Send + Sync + 'static> Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Send + Sync + 'static> Clone for State<'_, T> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), std::marker::PhantomData)
    }
}

/// Where Tauri put an app's folders on Windows, for [`identifier`] (`IDENTIFIER`, or the Store copy's).
pub struct PathResolver;

impl PathResolver {
    fn known(var: &str) -> Result<PathBuf> {
        std::env::var_os(var).map(PathBuf::from).ok_or_else(|| Error(format!("{var} isn't set")))
    }
    /// %APPDATA%\com.shado.shadui-ripgrep
    pub fn app_data_dir(&self) -> Result<PathBuf> {
        Ok(Self::known("APPDATA")?.join(identifier()))
    }
    /// %APPDATA%\com.shado.shadui-ripgrep
    pub fn app_config_dir(&self) -> Result<PathBuf> {
        self.app_data_dir()
    }
    /// %LOCALAPPDATA%\com.shado.shadui-ripgrep
    pub fn app_local_data_dir(&self) -> Result<PathBuf> {
        Ok(Self::known("LOCALAPPDATA")?.join(identifier()))
    }
    /// %LOCALAPPDATA%\com.shado.shadui-ripgrep
    pub fn app_cache_dir(&self) -> Result<PathBuf> {
        self.app_local_data_dir()
    }
    /// %LOCALAPPDATA%\com.shado.shadui-ripgrep\logs
    pub fn app_log_dir(&self) -> Result<PathBuf> {
        Ok(self.app_local_data_dir()?.join("logs"))
    }
    /// The folder Glint.exe is in (where Tauri looked for bundled resources).
    pub fn resource_dir(&self) -> Result<PathBuf> {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(PathBuf::from))
            .ok_or_else(|| Error("can't tell where Glint.exe is".into()))
    }
    pub fn temp_dir(&self) -> Result<PathBuf> {
        Ok(std::env::temp_dir())
    }
    pub fn home_dir(&self) -> Result<PathBuf> {
        Self::known("USERPROFILE")
    }
}

/// The app's name and version.
pub struct PackageInfo {
    pub name: String,
    pub version: String,
}

/// The calls modules made on the app handle (Tauri's `Manager`), so `use crate::host::Manager` keeps working.
pub trait Manager {
    fn app_handle(&self) -> &AppHandle;

    /// Registers `value` for `state::<T>()`; false if one was already there (as Tauri did).
    fn manage<T: Send + Sync + 'static>(&self, value: T) -> bool {
        let Ok(mut states) = self.app_handle().0.states.write() else { return false };
        if states.contains_key(&TypeId::of::<T>()) {
            return false;
        }
        states.insert(TypeId::of::<T>(), Arc::new(value));
        true
    }

    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        let states = self.app_handle().0.states.read().ok()?;
        let value = states.get(&TypeId::of::<T>())?.clone();
        value.downcast::<T>().ok().map(|value| State(value, std::marker::PhantomData))
    }

    /// The registered `T`. Like Tauri, a missing state is a programming error: register every state
    /// at start (the GPUI app does, where lib.rs's setup did).
    fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        self.try_state::<T>()
            .unwrap_or_else(|| panic!("state {} wasn't registered with manage()", std::any::type_name::<T>()))
    }

    fn get_webview_window(&self, label: &str) -> Option<Window> {
        self.app_handle().0.windows.read().ok()?.get(label).cloned()
    }

    fn webview_windows(&self) -> HashMap<String, Window> {
        self.app_handle().0.windows.read().map(|w| w.clone()).unwrap_or_default()
    }

    fn path(&self) -> PathResolver {
        PathResolver
    }

    fn package_info(&self) -> PackageInfo {
        PackageInfo { name: "Glint".into(), version: env!("CARGO_PKG_VERSION").into() }
    }

    fn run_on_main_thread(&self, run: impl FnOnce() + Send + 'static) -> Result<()> {
        (self.app_handle().0.main_thread)(Box::new(run));
        Ok(())
    }

    fn exit(&self, code: i32) {
        (self.app_handle().0.exit)(code)
    }
}

/// Sending events to the UI (Tauri's `Emitter`).
pub trait Emitter: Manager {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        let payload = serde_json::to_value(payload).map_err(|e| Error(e.to_string()))?;
        (self.app_handle().0.events)(Event { name: event.into(), target: None, payload });
        Ok(())
    }

    fn emit_to<S: Serialize + Clone>(&self, target: &str, event: &str, payload: S) -> Result<()> {
        let payload = serde_json::to_value(payload).map_err(|e| Error(e.to_string()))?;
        (self.app_handle().0.events)(Event { name: event.into(), target: Some(target.into()), payload });
        Ok(())
    }
}

impl Manager for AppHandle {
    fn app_handle(&self) -> &AppHandle {
        self
    }
}

// Tauri had these on the handle itself, so modules call them without importing a trait.
impl AppHandle {
    pub fn package_info(&self) -> PackageInfo {
        Manager::package_info(self)
    }

    pub fn exit(&self, code: i32) {
        Manager::exit(self, code)
    }

    pub fn run_on_main_thread(&self, run: impl FnOnce() + Send + 'static) -> Result<()> {
        Manager::run_on_main_thread(self, run)
    }

    pub fn path(&self) -> PathResolver {
        PathResolver
    }
}

impl Emitter for AppHandle {}

type WindowCall = dyn Fn() -> bool + Send + Sync;

/// One of the UI's windows, as modules saw a Tauri window: its label, its HWND (for shell dialogs'
/// owner) and a few things the UI does for it.
#[derive(Clone)]
pub struct Window {
    label: String,
    hwnd: isize,
    app: AppHandle,
    visible: Arc<WindowCall>,
    show: Arc<WindowCall>,
    focus: Arc<WindowCall>,
    scale: Arc<dyn Fn() -> f64 + Send + Sync>,
}

pub type WebviewWindow = Window;

impl Window {
    /// Made by the UI for each window it opens; `visible`, `show` and `focus` answer and act for it.
    pub fn new(
        app: AppHandle,
        label: impl Into<String>,
        hwnd: isize,
        visible: impl Fn() -> bool + Send + Sync + 'static,
        show: impl Fn() -> bool + Send + Sync + 'static,
        focus: impl Fn() -> bool + Send + Sync + 'static,
        scale: impl Fn() -> f64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            hwnd,
            app,
            visible: Arc::new(visible),
            show: Arc::new(show),
            focus: Arc::new(focus),
            scale: Arc::new(scale),
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// The window's HWND, as Tauri gave it (the windows crate's type).
    pub fn hwnd(&self) -> Result<windows::Win32::Foundation::HWND> {
        Ok(windows::Win32::Foundation::HWND(self.hwnd as *mut _))
    }

    pub fn is_visible(&self) -> Result<bool> {
        Ok((self.visible)())
    }

    pub fn show(&self) -> Result<()> {
        (self.show)();
        Ok(())
    }

    pub fn set_focus(&self) -> Result<()> {
        (self.focus)();
        Ok(())
    }

    pub fn scale_factor(&self) -> Result<f64> {
        Ok((self.scale)())
    }

    pub fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        self.app.emit_to(&self.label, event, payload)
    }

    pub fn app_handle(&self) -> &AppHandle {
        &self.app
    }

    pub fn run_on_main_thread(&self, run: impl FnOnce() + Send + 'static) -> Result<()> {
        Manager::run_on_main_thread(&self.app, run)
    }
}

impl Manager for Window {
    fn app_handle(&self) -> &AppHandle {
        &self.app
    }
}

/// Windows preview handlers hold the files they show open; before a file is renamed, moved, recycled or
/// replaced, the UI releases them (native_preview.rs did this in the web app). The UI registers how.
pub mod previews {
    use std::sync::OnceLock;

    type Release = dyn Fn(&[String]) + Send + Sync;
    static RELEASE: OnceLock<Box<Release>> = OnceLock::new();

    /// Set once by the UI at start.
    pub fn set_release_hook(release: impl Fn(&[String]) + Send + Sync + 'static) {
        let _ = RELEASE.set(Box::new(release));
    }

    /// Lets go of `paths` if a preview holds any of them; nothing without a UI.
    pub fn release(paths: &[String]) {
        if let Some(release) = RELEASE.get() {
            release(paths);
        }
    }
}

/// Commands' channels and binary answers.
pub mod ipc {
    use super::{Error, Result};
    use std::sync::Arc;

    /// A stream of `T` from a long job to whoever started it (progress, results as they come).
    /// Without a type it carries JSON or bytes, as Tauri's untyped channels did.
    pub struct Channel<T = InvokeResponseBody>(Arc<dyn Fn(T) -> Result<()> + Send + Sync>);

    impl<T> Channel<T> {
        pub fn new(on_message: impl Fn(T) -> Result<()> + Send + Sync + 'static) -> Self {
            Self(Arc::new(on_message))
        }

        /// A channel nobody listens to.
        pub fn ignored() -> Self {
            Self::new(|_| Ok(()))
        }

        pub fn send(&self, message: T) -> Result<()> {
            (self.0)(message)
        }
    }

    impl<T> Clone for Channel<T> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    /// Bytes a command answered with (listings, thumbnails): the GPUI side reads them directly.
    #[derive(Debug, Clone, Default)]
    pub struct Response(pub Vec<u8>);

    impl Response {
        pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
            Self(bytes.into())
        }

        pub fn into_bytes(self) -> Vec<u8> {
            self.0
        }
    }

    /// What a raw channel carried in Tauri: JSON text or bytes.
    #[derive(Debug, Clone)]
    pub enum InvokeResponseBody {
        Json(String),
        Raw(Vec<u8>),
    }

    impl From<Error> for InvokeResponseBody {
        fn from(error: Error) -> Self {
            Self::Json(serde_json::Value::String(error.0).to_string())
        }
    }
}

/// Tauri's async runtime: one tokio runtime for the core's async work, made on first use.
pub mod async_runtime {
    use std::future::Future;
    use std::sync::LazyLock;

    static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .thread_name("glint-core")
            .enable_all()
            .build()
            .expect("the core's async runtime can't start")
    });

    pub type JoinHandle<T> = tokio::task::JoinHandle<T>;
    /// Tauri re-exported tokio's channels here.
    pub use tokio::sync::mpsc::{channel, Receiver, Sender};

    pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        RUNTIME.spawn(future)
    }

    pub fn spawn_blocking<F, R>(run: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        RUNTIME.spawn_blocking(run)
    }

    /// Runs a future to its end on the calling thread (never call it from inside the runtime).
    pub fn block_on<F: Future>(future: F) -> F::Output {
        RUNTIME.block_on(future)
    }

    /// The runtime itself, for code that needs a handle.
    pub fn handle() -> tokio::runtime::Handle {
        RUNTIME.handle().clone()
    }
}
