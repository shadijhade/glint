//! Ported from src/lib/store.ts (persist) and src/lib/settingsFile.ts.
//!
//! Loading and saving Glint's settings and session (the state the web app kept in WebView2's localStorage
//! under `shadui-ripgrep-storage`), and the settings file of Settings → About (Export and Import).
//!
//! The state is the same JSON the web app saved (`glint_model::store::persisted_json`), kept in one file:
//! `<app data dir>/state.json`. Rules:
//!
//! - **Loading** happens once at start. A missing or unreadable file gives the defaults; a file that is not a
//!   saved state is renamed aside (`state.corrupt-<time>.json`), never deleted, and the defaults are used.
//! - **Saving** is debounced (about 500 ms after the last change), atomic (a temp file in the same folder,
//!   renamed over the old one) and done by a worker thread, never the UI thread. The app quitting flushes.
//!
//! **A secondary window** (a second Glint.exe process, `platform::secondary`) keeps nothing of its own: it
//! loads the file without moving a bad one aside, never writes it (no [`Saver`]), starts without last time's
//! session or last folder, and follows the main window's changes: [`follow_main`] watches `state.json` and
//! [`apply_reloaded`] takes a new copy of it (`useStore.persist.rehydrate()`).
//!
//! The live settings are held by [`crate::state`]; the rest of the saved state (pinned folders, history, the
//! session...) lives here, in [`Persistence`]. [`init`] loads and hands the settings to `state`, whose
//! `update` calls [`sync`] after every change, so changed settings are saved soon.
//!
//! **The first start** imports the old app's state from WebView2's `localStorage` (a LevelDB folder under
//! `EBWebView`): when there is no `state.json` yet, [`init`] first asks [`crate::old_settings`] to write one (and
//! `books.json`) from it, read-only, and then loads that file like any other. See that module.

use glint_model::rank::Visit;
use glint_model::store::{
    load_persisted, persisted_json, AppState, FolderRule, FolderView, HistoryItem, OrderedMap, RemoteConnection,
    SavedSearch, Settings,
};
use gpui_kit::{App, Global};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// The saved state's file name, in the app data folder.
pub const STATE_FILE: &str = "state.json";

/// How long after the last change the state is written.
const DEBOUNCE: Duration = Duration::from_millis(500);

/// A state that keeps changing is written at least this often.
const MAX_WAIT: Duration = Duration::from_secs(5);

/// How long quitting waits for the last write.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

/// The biggest settings file Import takes (the old one read 4 MiB).
const MAX_SETTINGS_FILE: u64 = 4 << 20;

// ---- Files ----------------------------------------------------------------------------------------

/// Where the state is saved: `%APPDATA%\com.shado.shadui-ripgrep\state.json`.
pub fn state_path() -> Option<PathBuf> {
    glint_core::host::PathResolver
        .app_data_dir()
        .ok()
        .map(|dir| dir.join(STATE_FILE))
}

/// What loading found.
#[derive(Debug, Default)]
pub struct Loaded {
    /// The saved state, or the defaults.
    pub state: AppState,
    /// Set when the file was not a saved state: where it was moved to.
    pub set_aside: Option<PathBuf>,
    /// The file is there but couldn't be read, or wasn't a saved state and couldn't be moved aside: this
    /// run must not save over it (the defaults it started with would replace what the person had).
    pub keep_file: bool,
}

/// Whether `text` is a state the old app (or this one) saved: `{"state": {...}, "version": 0}`.
fn is_saved_state(text: &str) -> bool {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => {
            map.get("state").is_some_and(Value::is_object)
                && map.get("version").and_then(Value::as_i64) == Some(glint_model::store::PERSIST_VERSION)
        }
        _ => false,
    }
}

/// Seconds since 1970, for names.
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Moves a file that isn't a saved state out of the way, keeping it. Returns where it went.
fn set_aside(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let stamp = unix_secs();
    for n in 0..100u32 {
        let name = if n == 0 {
            format!("{stem}.corrupt-{stamp}.json")
        } else {
            format!("{stem}.corrupt-{stamp}-{n}.json")
        };
        let target = dir.join(name);
        if target.exists() {
            continue;
        }
        return std::fs::rename(path, &target).ok().map(|_| target);
    }
    None
}

/// [`load_from`] for a window that mustn't write: text that isn't a saved state gives the defaults and the
/// file stays where it is (the main window moves it aside).
pub fn load_read_only(path: &Path) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Loaded {
                keep_file: error.kind() != std::io::ErrorKind::NotFound,
                ..Loaded::default()
            }
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    if is_saved_state(text) {
        Loaded {
            state: load_persisted(text),
            ..Loaded::default()
        }
    } else {
        Loaded {
            keep_file: true,
            ..Loaded::default()
        }
    }
}

/// Reads the state at `path`. Missing or unreadable (a locked file, a permission) gives the defaults and
/// leaves the file alone; text that isn't a saved state is renamed aside first.
pub fn load_from(path: &Path) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Loaded {
                keep_file: error.kind() != std::io::ErrorKind::NotFound,
                ..Loaded::default()
            }
        }
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    if is_saved_state(text) {
        return Loaded {
            state: load_persisted(text),
            set_aside: None,
            keep_file: false,
        };
    }
    let aside = set_aside(path);
    Loaded {
        state: AppState::default(),
        keep_file: aside.is_none(),
        set_aside: aside,
    }
}

/// Writes `bytes` to `path` so a reader sees the old file or the new one, never half of it: a temp file in
/// the same folder, flushed to disk, then renamed over. Tries a few times, since a virus scanner or an
/// indexer may hold the target for a moment.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let dir = path.parent().ok_or("no folder to write in")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("couldn't write {}: {e}", temp.display()));
    }
    let mut last = None;
    for attempt in 0..5u64 {
        match std::fs::rename(&temp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(20 * (attempt + 1)));
            }
        }
    }
    let _ = std::fs::remove_file(&temp);
    Err(format!(
        "couldn't replace {}: {}",
        path.display(),
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

/// Writes `state` to `path` in the saved shape.
pub fn save_to(path: &Path, state: &AppState) -> Result<(), String> {
    let json = persisted_json(state).map_err(|e| format!("couldn't encode the state: {e}"))?;
    write_atomic(path, json.as_bytes())
}

// ---- The saver ------------------------------------------------------------------------------------

enum Message {
    /// A newer state to write once things settle.
    Save(Box<AppState>),
    /// The app's state changed: once things settle the worker asks for it (`Saver::start_asking`).
    Changed,
    /// The state the worker asked for: written now.
    Write(Box<AppState>),
    /// Write now (this state), then say so.
    Flush(Box<AppState>, Sender<()>),
}

/// How long the worker waits for the state it asked for before asking again (and keeping the change in mind).
const ASK_AGAIN: Duration = Duration::from_secs(10);

/// Writes the state on a worker thread: debounced, atomic, never on the caller's thread. Cheap to clone.
#[derive(Clone)]
pub struct Saver {
    to_worker: Sender<Message>,
}

impl Saver {
    /// Starts the worker for `path`. It ends when every `Saver` is dropped.
    pub fn start(path: PathBuf) -> Saver {
        Self::start_with(path, None)
    }

    /// [`Saver::start`] for an app that says only that its state changed ([`Saver::changed`]): once things settle,
    /// the worker asks for the state on the returned channel, and the app hands it over ([`Saver::write`]). The
    /// state is copied once per save, not once per change: a folder opened changed it (and copied all of it, the
    /// visits, the folders' views, the session) every time.
    ///
    /// The worker holds only a weak end of the channel: the strong one (returned) is the app's to keep and drop
    /// on its own thread, so the channel is never closed by the worker as it ends (which would wake the task
    /// waiting on it from the wrong thread).
    pub fn start_asking(path: PathBuf) -> (Saver, async_channel::Sender<()>, async_channel::Receiver<()>) {
        let (asks, asked) = async_channel::bounded(1);
        (Self::start_with(path, Some(asks.downgrade())), asks, asked)
    }

    fn start_with(path: PathBuf, asks: Option<async_channel::WeakSender<()>>) -> Saver {
        let (to_worker, inbox) = mpsc::channel::<Message>();
        let spawned = std::thread::Builder::new().name("glint-persist".into()).spawn(move || {
            let write = |state: &AppState| {
                if let Err(e) = save_to(&path, state) {
                    eprintln!("glint: couldn't save the state: {e}");
                }
            };
            // A state to write once things settle; changes the app hasn't handed over yet; when the app was asked
            // for its state (and not answered yet); and when the wait for things to settle began.
            let mut pending: Option<Box<AppState>> = None;
            let mut changed = false;
            let mut asked: Option<Instant> = None;
            let mut first_pending_at = Instant::now();
            // Asks the app for its state: the changes are its to hand over now. Without anyone to ask, forgotten.
            let ask = |changed: &mut bool, asked: &mut Option<Instant>| {
                let sent = asks
                    .as_ref()
                    .and_then(async_channel::WeakSender::upgrade)
                    .is_some_and(|asks| {
                        matches!(asks.try_send(()), Ok(()) | Err(async_channel::TrySendError::Full(_)))
                    });
                *asked = sent.then(Instant::now);
                *changed = false;
            };
            loop {
                let settling = pending.is_some() || (changed && asked.is_none());
                let received = if settling {
                    let wait = DEBOUNCE.min(MAX_WAIT.saturating_sub(first_pending_at.elapsed()));
                    inbox.recv_timeout(wait)
                } else if let Some(at) = asked {
                    inbox.recv_timeout(ASK_AGAIN.saturating_sub(at.elapsed()))
                } else {
                    inbox.recv().map_err(|_| RecvTimeoutError::Disconnected)
                };
                match received {
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(state) = pending.take() {
                            write(&state);
                        }
                        if changed && asked.is_none() {
                            ask(&mut changed, &mut asked);
                        } else if asked.is_some_and(|at| at.elapsed() >= ASK_AGAIN) {
                            // No answer: what was asked for is still to write.
                            ask(&mut changed, &mut asked);
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        if let Some(state) = pending.take() {
                            write(&state);
                        }
                        return;
                    }
                    Ok(Message::Save(state)) => {
                        if !settling {
                            first_pending_at = Instant::now();
                        }
                        pending = Some(state);
                        // Past the longest wait: write now rather than keep waiting for quiet.
                        if first_pending_at.elapsed() >= MAX_WAIT {
                            if let Some(state) = pending.take() {
                                write(&state);
                            }
                        }
                    }
                    Ok(Message::Changed) => {
                        if !settling && asked.is_none() {
                            first_pending_at = Instant::now();
                        }
                        changed = true;
                        if asked.is_none() && first_pending_at.elapsed() >= MAX_WAIT {
                            ask(&mut changed, &mut asked);
                        }
                    }
                    Ok(Message::Write(state)) => {
                        pending = None;
                        write(&state);
                        asked = None;
                        // Changes since the app was asked: their own wait for quiet.
                        if changed {
                            first_pending_at = Instant::now();
                        }
                    }
                    Ok(Message::Flush(state, done)) => {
                        pending = None;
                        changed = false;
                        write(&state);
                        let _ = done.send(());
                    }
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("glint: couldn't start the saving thread: {e}");
        }
        Saver { to_worker }
    }

    /// Asks for `state` to be written soon (about 500 ms after the last ask).
    pub fn save_soon(&self, state: AppState) {
        let _ = self.to_worker.send(Message::Save(Box::new(state)));
    }

    /// The app's state changed: it's asked for once things settle (see [`Saver::start_asking`]).
    pub fn changed(&self) {
        let _ = self.to_worker.send(Message::Changed);
    }

    /// The state the worker asked for.
    pub fn write(&self, state: AppState) {
        let _ = self.to_worker.send(Message::Write(Box::new(state)));
    }

    /// Writes `state` now and waits (a few seconds at most) until it is on disk. For quitting, and from a
    /// background thread: it blocks.
    pub fn flush(&self, state: AppState) {
        let (done, wait) = mpsc::channel();
        if self.to_worker.send(Message::Flush(Box::new(state), done)).is_ok() {
            let _ = wait.recv_timeout(FLUSH_TIMEOUT);
        }
    }
}

// ---- In the app -----------------------------------------------------------------------------------

/// The saved state held by the running app (the settings in it are kept up to date with [`crate::state`]'s).
pub struct Persistence {
    app: AppState,
    /// None when this run mustn't write the file ([`Loaded::keep_file`]), and in a secondary window.
    saver: Option<Saver>,
    set_aside: Option<PathBuf>,
    /// A secondary window's watch on the file, for as long as the app runs.
    _follow: Option<Follow>,
    /// The strong end of the channel the saver asks for the state on (see `Saver::start_asking`).
    _asks: Option<async_channel::Sender<()>>,
}

/// A secondary window's watch on `state.json`, with the one strong end of the channel it reports on (the watching
/// thread holds a weak one, so the channel is closed by the app's thread and never by the watching thread as it
/// ends, which would wake the task waiting on it from the wrong thread).
struct Follow {
    _watch: crate::platform::secondary::FileWatch,
    _changed: std::sync::Arc<async_channel::Sender<()>>,
}

impl Global for Persistence {}

/// Start-up step after `state::init`: loads `state.json`, puts its settings in `state`, starts watching the
/// settings for changes and saves on quit. Without an app data folder nothing is loaded or saved. At the very
/// first start (no `state.json` yet) the old app's settings are imported first ([`crate::old_settings`]).
pub fn init(cx: &mut App) {
    match state_path() {
        Some(path) => {
            crate::old_settings::import_at_first_start(&path);
            init_at(cx, path)
        }
        None => eprintln!("glint: no app data folder; settings won't be saved"),
    }
}

/// [`init`] with the state file at `path`.
pub fn init_at(cx: &mut App, path: PathBuf) {
    // A secondary window reads the file and never writes it.
    let secondary = crate::platform::secondary::is_secondary();
    let loaded = if secondary {
        load_read_only(&path)
    } else {
        load_from(&path)
    };
    if let Some(aside) = &loaded.set_aside {
        eprintln!(
            "glint: {} wasn't a saved state; kept as {}",
            path.display(),
            aside.display()
        );
    }
    let mut app = loaded.state;
    if secondary {
        // The tabs and the last folder are the main window's: this one opens on its own folder.
        app.session = None;
        app.last_folder = None;
    }
    let saved_settings = app.settings.clone();
    crate::state::update(cx, |settings| *settings = saved_settings);
    // Debug builds' environment overrides (as state::init reads them) win over the saved settings, and are
    // the baseline, so looking at the other mode doesn't by itself write anything.
    if cfg!(debug_assertions) {
        apply_debug_overrides(cx);
        app.settings = crate::state::settings(cx).clone();
    }
    if loaded.keep_file && !secondary {
        eprintln!(
            "glint: {} couldn't be read or moved aside; this run won't save over it",
            path.display()
        );
    }
    // The first start after the old app: its notice flags and recent tags (kept outside its store) come along.
    let took_old = !secondary && crate::old_settings::imported().is_some_and(|old| take_old_extras(&mut app, old));
    let (saver, asks, asked) = match (!loaded.keep_file && !secondary).then(|| Saver::start_asking(path.clone())) {
        Some((saver, asks, asked)) => (Some(saver), Some(asks), Some(asked)),
        None => (None, None, None),
    };
    let follow = if secondary { follow_main(cx, &path) } else { None };
    cx.set_global(Persistence {
        app,
        saver: saver.clone(),
        set_aside: loaded.set_aside,
        _follow: follow,
        _asks: asks,
    });
    if took_old {
        save_soon(cx);
    }
    // The saver asks for the state once changes settle: handed over as it is then.
    if let Some(asked) = asked {
        cx.spawn(async move |cx| {
            while asked.recv().await.is_ok() {
                // Ends with the saver's worker, which closes the channel.
                cx.update(|cx| {
                    if let Some(p) = cx.try_global::<Persistence>() {
                        if let Some(saver) = &p.saver {
                            saver.write(p.app.clone());
                        }
                    }
                });
            }
        })
        .detach();
    }

    cx.on_app_quit(move |cx| {
        sync_quietly(cx);
        let snapshot = cx.try_global::<Persistence>().map(|p| p.app.clone());
        let executor = cx.background_executor().clone();
        let saver = saver.clone();
        async move {
            if let (Some(snapshot), Some(saver)) = (snapshot, saver) {
                executor.spawn(async move { saver.flush(snapshot) }).await;
            }
        }
    })
    .detach();
}

/// `GLINT_LANGUAGE` and `GLINT_THEME` (debug builds), applied over the settings in use.
fn apply_debug_overrides(cx: &mut App) {
    if let Ok(language) = std::env::var("GLINT_LANGUAGE") {
        crate::state::update(cx, |s| {
            s.language = match language.as_str() {
                "ar" => glint_model::store::Language::Ar,
                "en" => glint_model::store::Language::En,
                _ => s.language,
            }
        });
    }
    if let Ok(theme) = std::env::var("GLINT_THEME") {
        crate::state::update(cx, |s| {
            s.theme = match theme.as_str() {
                "light" => glint_model::store::Theme::Light,
                "dark" => glint_model::store::Theme::Dark,
                _ => s.theme,
            }
        });
    }
}

// ---- A secondary window follows the main window --------------------------------------------------------------

/// The state at `path`, if it is a saved state: what a secondary window takes when the file changes. Never moves
/// a bad file aside (a half-written or foreign file is passed over until the next change).
fn read_saved(path: &Path) -> Option<AppState> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    is_saved_state(text).then(|| load_persisted(text))
}

/// Watches `path` and takes each new copy of it ([`apply_reloaded`]), read off the UI thread. None when Windows
/// won't watch the folder (the window then keeps the settings it started with).
fn follow_main(cx: &mut App, path: &Path) -> Option<Follow> {
    let (changed_in, changed) = async_channel::unbounded::<()>();
    let changed_in = std::sync::Arc::new(changed_in);
    let weak = std::sync::Arc::downgrade(&changed_in);
    let Some(watch) = crate::platform::secondary::watch_file(path, move || {
        if let Some(changed_in) = weak.upgrade() {
            let _ = changed_in.try_send(());
        }
    }) else {
        eprintln!(
            "glint: can't watch {}; this window won't follow the main window's settings",
            path.display()
        );
        return None;
    };
    let path = path.to_path_buf();
    cx.spawn(async move |cx| {
        while changed.recv().await.is_ok() {
            // Several changes in a row are one read.
            while changed.try_recv().is_ok() {}
            let from = path.clone();
            if let Some(state) = cx.background_executor().spawn(async move { read_saved(&from) }).await {
                cx.update(|cx| apply_reloaded(cx, state));
            }
        }
    })
    .detach();
    Some(Follow {
        _watch: watch,
        _changed: changed_in,
    })
}

/// A secondary window takes the main window's new saved state: the settings (redrawn, with the theme and the
/// language following), and the pinned folders, saved searches and the rest, which observers of
/// [`Persistence`] hear. The window's own session and last folder stay as they are.
pub fn apply_reloaded(cx: &mut App, mut state: AppState) {
    if !cx.has_global::<Persistence>() {
        return;
    }
    let settings = state.settings.clone();
    let p = cx.global_mut::<Persistence>();
    state.session = p.app.session.take();
    state.last_folder = p.app.last_folder.take();
    p.app = state;
    if settings != *crate::state::settings(cx) {
        crate::state::update(cx, |live| *live = settings);
        if cfg!(debug_assertions) {
            apply_debug_overrides(cx);
        }
        sync_quietly(cx);
    }
}

/// Copies the live settings into the saved state; true if they had changed.
fn sync_quietly(cx: &mut App) -> bool {
    let live = crate::state::settings(cx).clone();
    if !cx.has_global::<Persistence>() {
        return false;
    }
    let p = cx.global_mut::<Persistence>();
    if p.app.settings == live {
        return false;
    }
    p.app.settings = live;
    true
}

/// One look at the live settings: saves soon if they changed. `state::update` calls it after every change.
pub fn sync(cx: &mut App) {
    if sync_quietly(cx) {
        save_soon(cx);
    }
}

/// The saved state now (its settings are those of the last [`sync`]).
pub fn app(cx: &App) -> Option<&AppState> {
    cx.try_global::<Persistence>().map(|p| &p.app)
}

/// Changes the saved state (pinned folders, history, the session...) and saves soon. For the settings use
/// [`crate::state::update`] instead.
pub fn update_app(cx: &mut App, change: impl FnOnce(&mut AppState)) {
    if cx.has_global::<Persistence>() {
        let p = cx.global_mut::<Persistence>();
        change(&mut p.app);
        if let Some(saver) = &p.saver {
            saver.changed();
        }
    }
}

/// Whether the notice `key` is new for `value`, remembered across runs (`firstNotice`): `"offer-instant-search"`
/// with `"1"` is shown once ever, `"service-update-notice"` with the app's version once per version. True without a
/// saved state (a notice can repeat; that beats never showing it).
pub fn first_notice(cx: &mut App, key: &str, value: &str) -> bool {
    if !cx.has_global::<Persistence>() {
        return true;
    }
    let mut first = false;
    update_app(cx, |app| first = app.first_notice(key, value));
    first
}

/// The tags used lately, most recent first, as kept between runs.
pub fn recent_tags(cx: &App) -> Vec<String> {
    app(cx).map(|app| app.recent_tags.clone()).unwrap_or_default()
}

/// Keeps the tags used lately (saved soon).
pub fn set_recent_tags(cx: &mut App, tags: Vec<String>) {
    update_app(cx, |app| {
        if app.recent_tags != tags {
            app.recent_tags = tags;
        }
    });
}

/// What the old app kept in localStorage outside its store, taken into the saved state at the first start after
/// it: the first-notice flags (so an updated user isn't offered instant search again) and the recent tags. What
/// the new app already has wins. True if anything was taken.
fn take_old_extras(app: &mut AppState, old: &crate::old_settings::OldData) -> bool {
    let mut taken = false;
    for (key, value) in &old.notices {
        if !app.notices.contains_key(key) {
            app.notices.insert(key.clone(), value.clone());
            taken = true;
        }
    }
    if app.recent_tags.is_empty() && !old.recent_tags.is_empty() {
        app.recent_tags = old.recent_tags.clone();
        taken = true;
    }
    taken
}

/// Where the state file that wasn't a saved state was moved to at start, if there was one.
pub fn corrupt_file_set_aside(cx: &App) -> Option<&Path> {
    cx.try_global::<Persistence>().and_then(|p| p.set_aside.as_deref())
}

fn save_soon(cx: &App) {
    if let Some(p) = cx.try_global::<Persistence>() {
        if let Some(saver) = &p.saver {
            saver.changed();
        }
    }
}

/// Writes the state now and waits, for places that exit without going through GPUI's quit. Blocks: call
/// it from a background thread or just before the process ends.
pub fn save_now(cx: &mut App) {
    sync_quietly(cx);
    if let Some(p) = cx.try_global::<Persistence>() {
        if let Some(saver) = &p.saver {
            saver.flush(p.app.clone());
        }
    }
}

// ---- The settings file (Settings → About: Export and Import) -------------------------------------

/// The parts of a settings file, in the order the dialog lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Settings,
    Pinned,
    SavedSearches,
    Remotes,
    Rules,
    History,
    Visits,
    FolderViews,
}

/// Every part, in order.
pub const ALL_PARTS: [Part; 8] = [
    Part::Settings,
    Part::Pinned,
    Part::SavedSearches,
    Part::Remotes,
    Part::Rules,
    Part::History,
    Part::Visits,
    Part::FolderViews,
];

/// What a part is called in the dialog and what is to know about it (`PARTS`): `(part, label, note)`.
pub const PARTS: [(Part, &str, Option<&str>); 8] = [
    (Part::Settings, "Settings", Some("every option here")),
    (Part::Pinned, "Pinned folders", None),
    (Part::SavedSearches, "Saved searches", None),
    (
        Part::Remotes,
        "Remote connections",
        Some("without their passwords and keys; a cloud only by its name in rclone"),
    ),
    (Part::Rules, "Folder rules", Some("they come turned off")),
    (Part::History, "Search history", None),
    (Part::Visits, "Folders Ctrl+K ranks", None),
    (
        Part::FolderViews,
        "Folders' own views",
        Some("their columns, sort and list or grid"),
    ),
];

/// A settings file as written: never a secret (connections' passwords and keys stay in Windows Credential
/// Manager, clouds' sign-ins in rclone's configuration). Parts not chosen are left out.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsFileOut<'a> {
    pub glint: &'static str,
    pub version: u32,
    /// When, as an ISO date.
    pub exported: String,
    /// From which version of Glint.
    pub app: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<&'a Settings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<&'a Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_searches: Option<&'a Vec<SavedSearch>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remotes: Option<&'a Vec<RemoteConnection>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<&'a Vec<FolderRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<&'a Vec<HistoryItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visits: Option<&'a OrderedMap<Visit>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_views: Option<&'a OrderedMap<FolderView>>,
}

/// Milliseconds since 1970 as `2026-11-10T12:00:00.000Z` (what `Date.toISOString` gives).
pub fn iso_date(unix_ms: i64) -> String {
    let secs = unix_ms.div_euclid(1000);
    let ms = unix_ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let in_day = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60
    )
}

/// The text of a settings file holding `parts` of `state` (`settingsFile()` + `JSON.stringify(.., null, 2)`).
pub fn export_settings(state: &AppState, parts: &[Part], now_ms: i64) -> Result<String, String> {
    let has = |p: Part| parts.contains(&p);
    let file = SettingsFileOut {
        glint: "settings",
        version: 1,
        exported: iso_date(now_ms),
        app: env!("CARGO_PKG_VERSION"),
        settings: has(Part::Settings).then_some(&state.settings),
        pinned: has(Part::Pinned).then_some(&state.pinned),
        saved_searches: has(Part::SavedSearches).then_some(&state.saved_searches),
        remotes: has(Part::Remotes).then_some(&state.remotes),
        rules: has(Part::Rules).then_some(&state.rules),
        history: has(Part::History).then_some(&state.history),
        visits: has(Part::Visits).then_some(&state.visits),
        folder_views: has(Part::FolderViews).then_some(&state.folder_views),
    };
    serde_json::to_string_pretty(&file).map_err(|e| format!("couldn't encode the settings file: {e}"))
}

/// Writes a settings file to `path` (`writeSettingsFile`). Blocks: call it from a background task.
pub fn write_settings_file(path: &Path, state: &AppState, parts: &[Part]) -> Result<(), String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(0))
        .unwrap_or(0);
    let text = export_settings(state, parts, now)?;
    write_atomic(path, text.as_bytes())
}

/// A settings file read for Import: the JSON as given, checked to be a Glint settings file. Each part is
/// looked at (and tolerated) only when taken.
#[derive(Debug, Clone)]
pub struct SettingsFile {
    root: Map<String, Value>,
}

impl SettingsFile {
    /// Reads the text of a settings file: what's in it, or why it isn't one Glint can take.
    pub fn parse(text: &str) -> Result<SettingsFile, String> {
        let not_one = || "That isn't a Glint settings file".to_string();
        let value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|_| "That isn't a Glint settings file (it isn't JSON)".to_string())?;
        let Value::Object(root) = value else {
            return Err(not_one());
        };
        if root.get("glint").and_then(Value::as_str) != Some("settings") {
            return Err(not_one());
        }
        match root.get("version").and_then(Value::as_f64) {
            Some(version) if version <= 1.0 => Ok(SettingsFile { root }),
            _ => Err("It's from a newer Glint: update this one first".to_string()),
        }
    }

    /// When it was exported, as the file says.
    pub fn exported(&self) -> Option<&str> {
        self.root.get("exported").and_then(Value::as_str)
    }

    /// The Glint version that wrote it.
    pub fn app(&self) -> Option<&str> {
        self.root.get("app").and_then(Value::as_str)
    }

    fn part_value(&self, part: Part) -> Option<&Value> {
        let key = match part {
            Part::Settings => "settings",
            Part::Pinned => "pinned",
            Part::SavedSearches => "savedSearches",
            Part::Remotes => "remotes",
            Part::Rules => "rules",
            Part::History => "history",
            Part::Visits => "visits",
            Part::FolderViews => "folderViews",
        };
        self.root.get(key)
    }

    /// How many of a part the file holds (settings: how many options Glint knows) (`countOf`).
    pub fn count_of(&self, part: Part) -> usize {
        if part == Part::Settings {
            return known_settings(self.part_value(part)).len();
        }
        match self.part_value(part) {
            Some(Value::Array(items)) => items.len(),
            Some(Value::Object(map)) => map.len(),
            _ => 0,
        }
    }
}

/// Reads a settings file from disk (`readSettingsFile`). Blocks: call it from a background task.
pub fn read_settings_file(path: &Path) -> Result<SettingsFile, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    if meta.len() > MAX_SETTINGS_FILE {
        return Err("That isn't a Glint settings file".to_string());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("couldn't read {}: {e}", path.display()))?;
    let text = String::from_utf8(bytes).map_err(|_| "That isn't a Glint settings file".to_string())?;
    SettingsFile::parse(&text)
}

/// JS's `typeof` for a JSON value.
fn js_typeof(value: &Value) -> &'static str {
    match value {
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Null | Value::Object(_) | Value::Array(_) => "object",
    }
}

/// The options a file sets that Glint knows, each of the type its default has (`knownSettings`).
fn known_settings(given: Option<&Value>) -> Map<String, Value> {
    let mut known = Map::new();
    let Some(Value::Object(given)) = given else {
        return known;
    };
    let Ok(Value::Object(defaults)) = serde_json::to_value(Settings::default()) else {
        return known;
    };
    for (key, fallback) in &defaults {
        let Some(value) = given.get(key) else { continue };
        let fits = fallback.is_null()
            || value.is_null()
            || if fallback.is_array() {
                value.is_array()
            } else {
                js_typeof(value) == js_typeof(fallback)
            };
        if fits {
            known.insert(key.clone(), value.clone());
        }
    }
    known
}

/// The objects of `value` (an array) that `keep` accepts, read as `T`.
fn list<T: serde::de::DeserializeOwned>(value: Option<&Value>, keep: impl Fn(&Map<String, Value>) -> bool) -> Vec<T> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::Object(map) if keep(map) => serde_json::from_value(item.clone()).ok(),
            _ => None,
        })
        .collect()
}

fn is_str(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).is_some_and(Value::is_string)
}

/// `existing` (or nothing when replacing) followed by the `incoming` items that `is_new` says aren't there.
fn merged<T: Clone>(existing: &[T], replace: bool, incoming: &[T], is_new: impl Fn(&[T], &T) -> bool) -> Vec<T> {
    let kept: Vec<T> = if replace { Vec::new() } else { existing.to_vec() };
    let fresh: Vec<T> = incoming.iter().filter(|n| is_new(&kept, n)).cloned().collect();
    let mut all = kept;
    all.extend(fresh);
    all
}

/// Takes `parts` of a settings file: each added to what's in `state` (what's there kept where both have it),
/// or replacing it. Folder rules always come turned off: their folders may not be on this PC. Returns what
/// it did, in a few words each (`importSettings`).
pub fn import_settings(state: &mut AppState, file: &SettingsFile, parts: &[Part], replace: bool) -> Vec<String> {
    let has = |p: Part| parts.contains(&p);
    let mut did = Vec::new();
    // The history's size follows the settings as they were before this import, as the old code read them.
    let max_history = state.settings.max_history_items.max(10) as usize;

    if has(Part::Settings) {
        let known = known_settings(file.part_value(Part::Settings));
        let base = if replace {
            Settings::default()
        } else {
            state.settings.clone()
        };
        if let Ok(Value::Object(mut all)) = serde_json::to_value(&base) {
            all.extend(known);
            if let Ok(settings) = serde_json::from_value::<Settings>(Value::Object(all)) {
                state.settings = settings;
            }
        }
        did.push("settings".to_string());
    }
    if has(Part::Pinned) {
        let pins: Vec<String> = match file.part_value(Part::Pinned) {
            Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        };
        let lower: std::collections::HashSet<String> = if replace {
            std::collections::HashSet::new()
        } else {
            state.pinned.iter().map(|p| p.to_lowercase()).collect()
        };
        let mut all = if replace { Vec::new() } else { state.pinned.clone() };
        all.extend(pins.iter().filter(|p| !lower.contains(&p.to_lowercase())).cloned());
        state.pinned = all;
        did.push(format!("{} pinned", pins.len()));
    }
    if has(Part::SavedSearches) {
        let searches: Vec<SavedSearch> = list(file.part_value(Part::SavedSearches), |v| {
            is_str(v, "query") && is_str(v, "folder")
        });
        state.saved_searches = merged(&state.saved_searches, replace, &searches, |kept, n| {
            !kept
                .iter()
                .any(|o| o.query == n.query && o.folder.to_lowercase() == n.folder.to_lowercase())
        });
        did.push(format!("{} saved searches", searches.len()));
    }
    if has(Part::Remotes) {
        let remotes: Vec<RemoteConnection> = list(file.part_value(Part::Remotes), |v| {
            is_str(v, "id") && is_str(v, "name") && is_str(v, "kind")
        });
        state.remotes = merged(&state.remotes, replace, &remotes, |kept, n| {
            !kept.iter().any(|k| k.id == n.id)
        });
        did.push(format!("{} connections", remotes.len()));
    }
    if has(Part::Rules) {
        let rules: Vec<FolderRule> = list::<FolderRule>(file.part_value(Part::Rules), |v| {
            is_str(v, "id") && is_str(v, "folder") && is_str(v, "action")
        })
        .into_iter()
        .map(|mut r| {
            r.on = false;
            r
        })
        .collect();
        state.rules = merged(&state.rules, replace, &rules, |kept, n| {
            !kept.iter().any(|k| k.id == n.id)
        });
        did.push(format!("{} rules, off", rules.len()));
    }
    if has(Part::History) {
        let history: Vec<HistoryItem> = list(file.part_value(Part::History), |v| {
            is_str(v, "query") && is_str(v, "path") && v.get("timestamp").is_some_and(Value::is_number)
        });
        let mut all: Vec<HistoryItem> = if replace { Vec::new() } else { state.history.clone() };
        all.extend(history.iter().cloned());
        // A stable sort, newest first, as JS's.
        all.sort_by_key(|h| std::cmp::Reverse(h.timestamp));
        let mut seen = std::collections::HashSet::new();
        all.retain(|h| seen.insert((h.query.clone(), h.path.clone())));
        all.truncate(max_history);
        state.history = all;
        did.push(format!("{} searches in the history", history.len()));
    }
    if has(Part::Visits) {
        if let Some(Value::Object(given)) = file.part_value(Part::Visits) {
            let mut visits: OrderedMap<Visit> = if replace {
                OrderedMap::new()
            } else {
                state.visits.clone()
            };
            for (path, v) in given {
                let Value::Object(v) = v else { continue };
                let (Some(count), Some(last)) = (
                    v.get("count").and_then(Value::as_f64),
                    v.get("last").and_then(Value::as_f64),
                ) else {
                    continue;
                };
                // Saturating float casts: no value in a file can panic here.
                let (count, last) = (count.max(0.0) as u64, last as i64);
                let merged = match visits.get(path) {
                    Some(here) => Visit {
                        count: here.count.max(count),
                        last: here.last.max(last),
                    },
                    None => Visit { count, last },
                };
                visits.insert(path.clone(), merged);
            }
            state.visits = visits;
            did.push(format!("{} ranked folders", given.len()));
        }
    }
    if has(Part::FolderViews) {
        if let Some(Value::Object(given)) = file.part_value(Part::FolderViews) {
            let mut views: OrderedMap<FolderView> = if replace {
                OrderedMap::new()
            } else {
                state.folder_views.clone()
            };
            let before = views.clone();
            for (path, v) in given {
                let Value::Object(map) = v else { continue };
                if !map.get("at").is_some_and(Value::is_number) {
                    continue;
                }
                let key = path.to_lowercase();
                if before.contains_key(&key) {
                    continue;
                }
                if let Ok(view) = serde_json::from_value::<FolderView>(v.clone()) {
                    views.insert(key, view);
                }
            }
            state.folder_views = views;
            did.push(format!("{} folders' views", given.len()));
        }
    }
    did
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::store::{Language, RemoteKind, RuleAction, Theme};

    /// A fresh folder per test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-persist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp folder");
        dir
    }

    fn sample() -> AppState {
        let mut s = AppState::default();
        s.settings.theme = Theme::Dark;
        s.settings.language = Language::Ar;
        s.pinned = vec!["C:\\Work".into()];
        s.saved_searches = vec![SavedSearch {
            query: "*.rs".into(),
            folder: "C:\\Code".into(),
        }];
        s.remotes = vec![RemoteConnection {
            id: "s3-a".into(),
            name: "A".into(),
            kind: RemoteKind::S3,
            ..Default::default()
        }];
        s.rules = vec![FolderRule {
            id: "r1".into(),
            name: "Old".into(),
            on: true,
            folder: "C:\\Downloads".into(),
            action: RuleAction::Recycle,
            ..Default::default()
        }];
        s.history = vec![HistoryItem {
            query: "a".into(),
            path: "C:\\".into(),
            timestamp: 100,
        }];
        s.visits.insert("C:\\Work".into(), Visit { count: 3, last: 50 });
        s
    }

    #[test]
    fn state_survives_a_save_and_a_load() {
        let dir = temp_dir("roundtrip");
        let path = dir.join(STATE_FILE);
        let state = sample();
        save_to(&path, &state).expect("saves");
        let loaded = load_from(&path);
        assert_eq!(loaded.state, state);
        assert!(loaded.set_aside.is_none());
        // No temp file is left behind.
        assert_eq!(std::fs::read_dir(&dir).expect("lists").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_gives_the_defaults_and_touches_nothing() {
        let dir = temp_dir("missing");
        let loaded = load_from(&dir.join(STATE_FILE));
        assert_eq!(loaded.state, AppState::default());
        assert!(loaded.set_aside.is_none());
        assert_eq!(std::fs::read_dir(&dir).expect("lists").count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_get_their_defaults_and_the_rest_is_kept() {
        let dir = temp_dir("fields");
        let path = dir.join(STATE_FILE);
        std::fs::write(
            &path,
            r#"{"state":{"pinned":["D:\\x"],"settings":{"theme":"dark"}},"version":0}"#,
        )
        .expect("writes");
        let loaded = load_from(&path);
        assert_eq!(loaded.state.pinned, vec!["D:\\x".to_string()]);
        assert_eq!(loaded.state.settings.theme, Theme::Dark);
        assert_eq!(loaded.state.settings.language, Settings::default().language);
        assert!(loaded.set_aside.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_set_aside_with_its_text_and_never_deleted() {
        for (i, text) in ["{ not json", "", "[1,2]", r#"{"state":{},"version":7}"#]
            .iter()
            .enumerate()
        {
            let dir = temp_dir(&format!("corrupt{i}"));
            let path = dir.join(STATE_FILE);
            std::fs::write(&path, text).expect("writes");
            let loaded = load_from(&path);
            assert_eq!(loaded.state, AppState::default());
            let aside = loaded.set_aside.expect("set aside");
            assert!(!path.exists());
            assert_eq!(std::fs::read_to_string(&aside).expect("kept"), *text);
            let name = aside
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            assert!(name.starts_with("state.corrupt-") && name.ends_with(".json"), "{name}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn the_saver_waits_then_writes_the_last_state_and_flush_is_immediate() {
        let dir = temp_dir("saver");
        let path = dir.join(STATE_FILE);
        let saver = Saver::start(path.clone());
        let mut state = sample();
        saver.save_soon(state.clone());
        assert!(!path.exists(), "debounced: nothing yet");
        state.pinned.push("E:\\".into());
        saver.save_soon(state.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(load_from(&path).state, state, "the last change is what was written");
        state.pinned.push("F:\\".into());
        saver.flush(state.clone());
        assert_eq!(load_from(&path).state, state, "flush has written when it returns");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_app_that_says_its_state_changed_is_asked_for_it_once_things_settle() {
        let dir = temp_dir("asked");
        let path = dir.join(STATE_FILE);
        let (saver, _asks, asked) = Saver::start_asking(path.clone());
        let started = Instant::now();
        // Three changes in a row: asked for the state once, after a quiet moment, and nothing is written meanwhile.
        for _ in 0..3 {
            saver.changed();
        }
        assert!(asked.recv_blocking().is_ok());
        assert!(
            started.elapsed() >= DEBOUNCE - Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
        assert!(asked.try_recv().is_err(), "once for the three");
        assert!(!path.exists(), "nothing written before the state is handed over");
        let mut state = sample();
        saver.write(state.clone());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(load_from(&path).state, state, "what was handed over is written");
        // A change after that: asked again.
        saver.changed();
        assert!(asked.recv_blocking().is_ok());
        state.pinned.push("G:\\".into());
        saver.flush(state.clone());
        assert_eq!(load_from(&path).state, state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn iso_dates_match_to_iso_string() {
        assert_eq!(iso_date(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_date(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
        assert_eq!(iso_date(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn the_export_has_settings_file_ts_shape() {
        let state = sample();
        let text = export_settings(&state, &ALL_PARTS, 0).expect("exports");
        let value: Value = serde_json::from_str(&text).expect("json");
        let mut keys: Vec<&str> = value.as_object().expect("object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut want = vec![
            "glint",
            "version",
            "exported",
            "app",
            "settings",
            "pinned",
            "savedSearches",
            "remotes",
            "rules",
            "history",
            "visits",
            "folderViews",
        ];
        want.sort_unstable();
        assert_eq!(keys, want);
        assert_eq!(value["glint"], "settings");
        assert_eq!(value["version"], 1);
        assert_eq!(value["exported"], "1970-01-01T00:00:00.000Z");
        assert_eq!(value["settings"]["theme"], "dark");
        assert_eq!(value["savedSearches"][0]["folder"], "C:\\Code");
        assert_eq!(value["visits"]["C:\\Work"]["count"], 3);
        // The header comes first, then parts in order, indented by two spaces like JSON.stringify(.., null, 2).
        assert!(text.starts_with("{\n  \"glint\": \"settings\",\n  \"version\": 1,\n  \"exported\""));
        let at = |k: &str| text.find(&format!("\n  \"{k}\"")).unwrap_or(usize::MAX);
        assert!(at("settings") < at("pinned") && at("pinned") < at("savedSearches") && at("rules") < at("history"));
        // Only the chosen parts are written.
        let some = export_settings(&state, &[Part::Pinned], 0).expect("exports");
        let some: Value = serde_json::from_str(&some).expect("json");
        assert!(some.get("pinned").is_some() && some.get("settings").is_none() && some.get("remotes").is_none());
    }

    #[test]
    fn a_settings_file_is_checked_before_it_is_taken() {
        assert_eq!(
            SettingsFile::parse("nope").unwrap_err(),
            "That isn't a Glint settings file (it isn't JSON)"
        );
        assert_eq!(
            SettingsFile::parse("[]").unwrap_err(),
            "That isn't a Glint settings file"
        );
        assert_eq!(
            SettingsFile::parse(r#"{"glint":"other","version":1}"#).unwrap_err(),
            "That isn't a Glint settings file"
        );
        assert_eq!(
            SettingsFile::parse(r#"{"glint":"settings","version":2}"#).unwrap_err(),
            "It's from a newer Glint: update this one first"
        );
        assert_eq!(
            SettingsFile::parse(r#"{"glint":"settings"}"#).unwrap_err(),
            "It's from a newer Glint: update this one first"
        );
        assert!(SettingsFile::parse(r#"{"glint":"settings","version":1}"#).is_ok());
    }

    #[test]
    fn export_then_import_gives_the_same_parts_and_counts() {
        let dir = temp_dir("export");
        let path = dir.join("glint-settings.json");
        let state = sample();
        write_settings_file(&path, &state, &ALL_PARTS).expect("writes");
        let file = read_settings_file(&path).expect("reads");
        assert_eq!(file.count_of(Part::Pinned), 1);
        assert_eq!(file.count_of(Part::Visits), 1);
        let options = serde_json::to_value(Settings::default()).map_or(0, |v| v.as_object().map_or(0, Map::len));
        assert!(options > 40);
        assert_eq!(file.count_of(Part::Settings), options);
        let mut target = AppState::default();
        let did = import_settings(&mut target, &file, &ALL_PARTS, true);
        assert_eq!(did.len(), 8);
        assert_eq!(target.settings, state.settings);
        assert_eq!(target.pinned, state.pinned);
        assert_eq!(target.saved_searches, state.saved_searches);
        assert_eq!(target.remotes, state.remotes);
        assert_eq!(target.history, state.history);
        assert_eq!(target.visits, state.visits);
        assert_eq!(target.rules.len(), 1);
        assert!(target.rules.iter().all(|r| !r.on), "rules come turned off");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_merges_chosen_parts_and_keeps_what_is_here() {
        let file = SettingsFile::parse(
            r#"{"glint":"settings","version":1,
                "settings":{"theme":"light","language":5,"nonsense":true,"showHidden":true},
                "pinned":["c:\\work","D:\\new",7],
                "savedSearches":[{"query":"*.rs","folder":"c:\\CODE"},{"query":"x","folder":"E:\\"},{"query":1}],
                "remotes":[{"id":"s3-a","name":"Other","kind":"s3"},{"id":"w","name":"W","kind":"webdav"}],
                "rules":[{"id":"r1","folder":"C:\\a","action":"move","on":true},{"id":"r2","folder":"C:\\b","action":"move","on":true}],
                "history":[{"query":"a","path":"C:\\","timestamp":900},{"query":"b","path":"C:\\","timestamp":500}],
                "visits":{"C:\\Work":{"count":9,"last":10},"C:\\New":{"count":1,"last":2},"bad":{"count":"x"}},
                "folderViews":{"C:\\Pics":{"at":5,"view":"grid"},"C:\\NoAt":{"view":"grid"}}}"#,
        )
        .expect("parses");
        let mut state = sample();
        // Only the chosen parts change.
        let did = import_settings(&mut state, &file, &[Part::Pinned], false);
        assert_eq!(did, vec!["2 pinned".to_string()]);
        assert_eq!(state.pinned, vec!["C:\\Work".to_string(), "D:\\new".to_string()]);
        assert_eq!(state.settings.theme, Theme::Dark);

        let all = [
            Part::Settings,
            Part::SavedSearches,
            Part::Remotes,
            Part::Rules,
            Part::History,
            Part::Visits,
            Part::FolderViews,
        ];
        let did = import_settings(&mut state, &file, &all, false);
        // Settings: known and fitting options are taken; the wrong type (language: 5) and unknown ones are not.
        assert_eq!(state.settings.theme, Theme::Light);
        assert!(state.settings.show_hidden);
        assert_eq!(state.settings.language, Language::Ar);
        assert_eq!(did[0], "settings");
        assert_eq!(
            state.saved_searches.len(),
            2,
            "the same search in other letters isn't added twice"
        );
        assert_eq!(did[1], "2 saved searches");
        assert_eq!(state.remotes.len(), 2);
        assert_eq!(state.remotes[0].name, "A", "what's here is kept where both have it");
        assert_eq!(state.rules.len(), 2);
        assert!(state.rules.iter().find(|r| r.id == "r2").is_some_and(|r| !r.on));
        assert!(
            state.rules.iter().find(|r| r.id == "r1").is_some_and(|r| r.on),
            "a rule already here is untouched"
        );
        let times: Vec<i64> = state.history.iter().map(|h| h.timestamp).collect();
        assert_eq!(
            times,
            vec![900, 500],
            "the same search (a in C) is kept once, at its newest"
        );
        assert_eq!(state.visits.get("C:\\Work"), Some(&Visit { count: 9, last: 50 }));
        assert_eq!(state.visits.get("C:\\New"), Some(&Visit { count: 1, last: 2 }));
        assert_eq!(state.visits.len(), 2);
        assert!(state.folder_views.contains_key("c:\\pics"));
        assert!(!state.folder_views.contains_key("c:\\noat"));
        assert_eq!(did.last().map(String::as_str), Some("2 folders' views"));

        // Replacing starts from nothing.
        let did = import_settings(&mut state, &file, &[Part::Pinned, Part::Settings], true);
        assert_eq!(state.pinned, vec!["c:\\work".to_string(), "D:\\new".to_string()]);
        assert_eq!(state.settings.language, Settings::default().language);
        assert_eq!(did, vec!["settings".to_string(), "2 pinned".to_string()]);
    }

    #[test]
    fn the_exported_file_never_holds_a_secret_field() {
        let text = export_settings(&sample(), &ALL_PARTS, 0)
            .expect("exports")
            .to_lowercase();
        for secret in ["password", "secret", "token", "passphrase"] {
            assert!(!text.contains(secret), "{secret}");
        }
    }

    #[gpui_kit::test]
    fn live_settings_changes_reach_the_file_and_a_corrupt_file_is_kept(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        let dir = temp_dir("app");
        let path = dir.join(STATE_FILE);
        std::fs::write(&path, "garbage").expect("writes");
        cx.update(|cx| {
            crate::testing::init(cx);
            init_at(cx, path.clone());
            assert!(corrupt_file_set_aside(cx).is_some());
            crate::state::update(cx, |s| s.show_hidden = true);
            update_app(cx, |a| a.pinned.push("C:\\Pinned".into()));
            sync(cx);
            save_now(cx);
        });
        let saved = load_from(&path);
        assert!(saved.state.settings.show_hidden);
        assert_eq!(saved.state.pinned, vec!["C:\\Pinned".to_string()]);
        assert!(std::fs::read_dir(&dir)
            .expect("lists")
            .any(|e| e.is_ok_and(|e| e.file_name().to_string_lossy().starts_with("state.corrupt-"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- A secondary window

    /// Every file of a folder with its bytes, for "nothing was written".
    fn contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .expect("lists")
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read(e.path()).unwrap_or_default(),
                )
            })
            .collect();
        files.sort();
        files
    }

    /// A saved state with a session and a last folder, which a secondary window must not take.
    fn with_session() -> AppState {
        let mut state = sample();
        state.settings.language = Language::En;
        state.last_folder = Some("C:\\Last".into());
        state.session = Some(glint_model::store::Session {
            dual: false,
            active_pane: 0,
            panes: vec![glint_model::store::SessionPane {
                paths: vec!["C:\\One".into(), "C:\\Two".into()],
                active: 1,
                views: None,
            }],
        });
        state
    }

    #[gpui_kit::test]
    fn a_secondary_window_reads_the_settings_and_never_writes_anything(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\Work"));
        let dir = temp_dir("secondary-no-writes");
        let path = dir.join(STATE_FILE);
        save_to(&path, &with_session()).expect("saves");
        let before = contents(&dir);
        cx.update(|cx| {
            crate::testing::init(cx);
            init_at(cx, path.clone());
            // The settings were read…
            assert_eq!(crate::state::settings(cx).theme, Theme::Dark);
            assert_eq!(app(cx).map(|a| a.pinned.clone()), Some(vec!["C:\\Work".to_string()]));
            // …but last time's tabs and folder are the main window's.
            assert!(app(cx).is_some_and(|a| a.session.is_none() && a.last_folder.is_none()));
            // Everything that would write: a setting, the saved state, a flush and the quit's save.
            crate::state::update(cx, |s| s.show_hidden = true);
            update_app(cx, |a| a.pinned.push("C:\\Pinned".into()));
            sync(cx);
            save_now(cx);
        });
        std::thread::sleep(std::time::Duration::from_millis(700));
        assert_eq!(contents(&dir), before, "not the file, and not a temp file beside it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn a_secondary_window_leaves_a_file_that_is_not_a_saved_state_where_it_is(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\Work"));
        let dir = temp_dir("secondary-garbage");
        let path = dir.join(STATE_FILE);
        std::fs::write(&path, "garbage").expect("writes");
        cx.update(|cx| {
            crate::testing::init(cx);
            init_at(cx, path.clone());
            assert!(corrupt_file_set_aside(cx).is_none());
            crate::state::update(cx, |s| s.show_hidden = true);
            save_now(cx);
        });
        assert_eq!(contents(&dir), vec![(STATE_FILE.to_string(), b"garbage".to_vec())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_without_writing_gives_the_same_state_as_loading() {
        let dir = temp_dir("read-only");
        let path = dir.join(STATE_FILE);
        save_to(&path, &sample()).expect("saves");
        assert_eq!(load_read_only(&path).state, load_from(&path).state);
        // A missing file is the defaults, and nothing is made.
        let missing = dir.join("nothing.json");
        let loaded = load_read_only(&missing);
        assert_eq!(loaded.state, AppState::default());
        assert!(!loaded.keep_file && !missing.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn a_secondary_window_takes_the_main_windows_new_state(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\Work"));
        let dir = temp_dir("secondary-reload");
        let path = dir.join(STATE_FILE);
        save_to(&path, &with_session()).expect("saves");
        // What a changed state in the main window looks like: a light theme, another pinned folder.
        let mut newer = sample();
        newer.settings.theme = Theme::Light;
        newer.settings.language = Language::En;
        newer.pinned.push("C:\\Newly".into());
        newer.session = None;
        let heard = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            crate::testing::init(cx);
            init_at(cx, path.clone());
            assert_eq!(crate::state::settings(cx).theme, Theme::Dark);
        });
        // Listening starts in an update of its own, after the start-up's pending notices.
        cx.update(|cx| {
            let seen = heard.clone();
            cx.observe_global::<Persistence>(move |_| seen.set(seen.get() + 1))
                .detach();
        });
        cx.update(|cx| {
            apply_reloaded(cx, newer.clone());
            assert_eq!(crate::state::settings(cx).theme, Theme::Light, "the theme follows");
            assert!(!crate::theme::tokens(cx).dark, "and is drawn");
            assert_eq!(
                app(cx).map(|a| a.pinned.clone()),
                Some(vec!["C:\\Work".to_string(), "C:\\Newly".to_string()])
            );
            assert!(app(cx).is_some_and(|a| a.session.is_none() && a.last_folder.is_none()));
        });
        assert!(heard.get() > 0, "observers of the saved state hear it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn settings_reload_when_the_file_changes(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        let _secondary = crate::platform::secondary::as_secondary(crate::platform::secondary::window_on("C:\\Work"));
        let dir = temp_dir("secondary-watch");
        let path = dir.join(STATE_FILE);
        let mut first = sample();
        first.settings.theme = Theme::Dark;
        first.settings.language = Language::En;
        save_to(&path, &first).expect("saves");
        cx.executor().allow_parking();
        cx.update(|cx| {
            crate::testing::init(cx);
            init_at(cx, path.clone());
            assert_eq!(crate::state::settings(cx).theme, Theme::Dark);
        });
        // The main window saves a new theme (the same atomic replace it always uses).
        let mut second = sample();
        second.settings.theme = Theme::Light;
        second.settings.language = Language::En;
        save_to(&path, &second).expect("saves");
        let mut followed = false;
        for _ in 0..600 {
            cx.run_until_parked();
            if cx.update(|cx| crate::state::settings(cx).theme) == Theme::Light {
                followed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(followed, "the window followed the file to the light theme");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- Notices and recent tags that outlive the run

    /// Starts the app on `path`, runs `f`, saves, and ends it (a run of Glint).
    fn a_run(cx: &mut gpui_kit::TestAppContext, path: &Path, f: impl FnOnce(&mut App)) {
        cx.update(|cx| {
            crate::testing::init(cx);
            init_at(cx, path.to_path_buf());
            f(cx);
            save_now(cx);
        });
    }

    #[gpui_kit::test]
    fn each_notice_shows_once_ever_or_once_per_version_across_restarts(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        let dir = temp_dir("notices");
        let path = dir.join(STATE_FILE);
        a_run(cx, &path, |cx| {
            assert!(first_notice(cx, "offer-instant-search", "1"));
            assert!(!first_notice(cx, "offer-instant-search", "1"), "not twice in a run");
            assert!(first_notice(cx, "service-update-notice", "0.9.5"));
        });
        // The next start: the offer never again; the service notice again only for a newer app.
        a_run(cx, &path, |cx| {
            assert!(!first_notice(cx, "offer-instant-search", "1"));
            assert!(!first_notice(cx, "service-update-notice", "0.9.5"));
            assert!(first_notice(cx, "service-update-notice", "0.9.6"));
        });
        a_run(cx, &path, |cx| {
            assert!(!first_notice(cx, "service-update-notice", "0.9.6"));
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui_kit::test]
    fn recent_tags_are_saved_and_loaded(cx: &mut gpui_kit::TestAppContext) {
        let _guard = crate::testing::lock();
        let dir = temp_dir("recent-tags");
        let path = dir.join(STATE_FILE);
        a_run(cx, &path, |cx| {
            assert!(recent_tags(cx).is_empty());
            set_recent_tags(cx, vec!["Work".into(), "Blue".into()]);
        });
        a_run(cx, &path, |cx| {
            assert_eq!(recent_tags(cx), ["Work", "Blue"]);
        });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_old_apps_flags_and_tags_come_along_but_never_replace_ours() {
        let mut old = crate::old_settings::OldData::default();
        old.notices.insert("offer-instant-search".into(), "1".into());
        old.notices.insert("service-update-notice".into(), "0.8.0".into());
        old.recent_tags = vec!["Old".into()];
        let mut app = AppState::default();
        assert!(take_old_extras(&mut app, &old));
        assert!(!app.first_notice("offer-instant-search", "1"));
        assert!(app.first_notice("service-update-notice", "0.9.5"));
        assert_eq!(app.recent_tags, ["Old"]);
        // Ours win, and there is nothing more to take.
        let mut ours = AppState::default();
        ours.notices.insert("service-update-notice".into(), "0.9.5".into());
        ours.recent_tags = vec!["Ours".into()];
        assert!(take_old_extras(&mut ours, &old), "the missing flag is taken");
        assert!(!ours.first_notice("service-update-notice", "0.9.5"));
        assert_eq!(ours.recent_tags, ["Ours"]);
        assert!(!take_old_extras(&mut ours, &old));
    }
}
