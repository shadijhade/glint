//! What [`crate::run`] starts once the shared services are up, and the glue between the app's services.
//!
//! Each service is a module that knows nothing of the others: one Glint per session ([`launch`]), the saved
//! state ([`persistence`]), the explorer model ([`explorer`]), the tray and the shortcut ([`tray`]), the main
//! window's place and the taskbar's buttons and Jump List ([`window_state`]) and the preview host
//! ([`preview_host`]); drops belong to the window, so the workspace owns its drop target. This file is where
//! they meet: a service exposes calls and events, and the wiring between them lives here and nowhere else.
//!
//! **Win32 calls on the main window go through [`core_bridge::post_to_ui`].** ShowWindow, SetWindowPlacement
//! and SetForegroundWindow send GPUI's window procedure messages at once, and its handlers take the app,
//! which every GPUI callback and entity update already holds: called from one of those, they panic. A posted
//! job runs from the message pump, outside them. (The tray's callbacks already run there.)

use crate::components::quick_search;
use crate::kit::prelude::*;
use crate::platform::{launch, preview_host, tray, window_state};
use crate::{core_bridge, explorer, persistence, state, t, workspace};
use glint_model::paths::{base_name, THIS_PC};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;

/// The main window's handle, 0 until it exists: posted jobs and the tray's callbacks can't hold a `Window`.
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

fn main_hwnd() -> Option<HWND> {
    match MAIN_HWND.load(Ordering::Acquire) {
        0 => None,
        raw => Some(HWND(raw as *mut _)),
    }
}

/// The main window, for notices raised outside it (the explorer's, the tray's, start-up's).
struct MainWindow(AnyWindowHandle);

impl Global for MainWindow {}

/// Start-up after `core_bridge::init`: the services that need core, then the main window, then the first
/// folders. `request` is what this copy was started with (folders, `--tray`, `--quick-search`).
pub(crate) fn start(cx: &mut App, request: launch::Request) {
    preview_host::init();
    // Instant search's state for the status bar (it polls core's index service in the background).
    crate::components::status_bar::init(cx);
    start_quick_search(cx);
    start_explorer(cx);
    start_tray(cx, request.tray);
    keep_jump_list(cx);

    let (window, _workspace) = match workspace::open_main_window(cx) {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("glint: couldn't open the main window: {error:#}");
            cx.quit();
            return;
        }
    };
    cx.set_global(MainWindow(window));
    let _ = window.update(cx, |_, window, cx| {
        launch::set_main_window(window);
        if let Some(hwnd) = core_bridge::hwnd_of(window) {
            MAIN_HWND.store(hwnd.0 as isize, Ordering::Release);
        }
        close_to_tray(window, cx);
    });
    // Opened hidden: put where it was last time, then shown, unless it starts in the tray.
    place_main_window(cx, !request.tray);
    keep_thumb_buttons(cx);

    if let Some(aside) = persistence::corrupt_file_set_aside(cx) {
        let name = aside
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        notify(
            cx,
            Notification::warning(t!(
                "Glint started with its defaults and kept the old file as {name}.",
                name = name
            ))
            .title(t!("Your saved settings couldn't be read")),
        );
    }

    // The first folders (last time's tabs, or what Glint was started with), then later launches: those wait
    // for the first ones, since the restored session would replace tabs placed before it.
    let targets = request
        .open
        .into_iter()
        .map(|target| explorer::LaunchTarget {
            path: target.path,
            select: target.select,
        })
        .collect();
    let first = explorer::explorer(cx).update(cx, |explorer, cx| explorer.init_explorer(targets, cx));
    cx.spawn(async move |cx| {
        first.await;
        cx.update(|cx| {
            hear_later_launches(cx);
            // The Quick Search window is made ahead of time now, so the shortcut shows it at once.
            quick_search::warm(cx);
            // Debug builds: GLINT_SELFTEST drives the real app through file operations (selftest.rs).
            crate::selftest::start(cx);
        });
    })
    .detach();

    if request.quick_search {
        quick_search(cx);
    }
}

// ---- The main window -------------------------------------------------------------------------------------

/// Puts the hidden main window where it was last time (or fits it to the screen), then shows it when `show`.
fn place_main_window(cx: &App, show: bool) {
    let data = window_state::data_dir(&cx.global::<core_bridge::Core>().app);
    core_bridge::post_to_ui(move || {
        let Some(hwnd) = main_hwnd() else { return };
        if let Some(data) = &data {
            window_state::restore_hwnd(hwnd, data);
        }
        if show {
            window_state::show_hwnd(hwnd);
        }
    });
}

/// Out of the tray, or from behind other windows: shown as it was kept (maximized, the first time, if it
/// was) and brought forward. Only from outside GPUI's callbacks: the tray's window procedure, a posted job.
fn reveal_main() {
    if let Some(hwnd) = main_hwnd() {
        window_state::show_hwnd(hwnd);
    }
    launch::bring_to_front();
}

/// Closing the main window keeps its place, then hides it in the tray when the tray keeps Glint running, and
/// otherwise lets it close (the last window closing quits; see `run`).
///
/// PORT ME: the old app first asked when quitting would cut a copy short or lose Compare's edits
/// (`mayQuit`); the copy queue and Compare parts add that question here.
fn close_to_tray(window: &mut Window, cx: &mut App) {
    window.on_window_should_close(cx, |window, cx| {
        let Some(hwnd) = core_bridge::hwnd_of(window) else {
            return true;
        };
        // GetWindowPlacement sends the window nothing, so this is safe inside the callback.
        window_state::save_hwnd(hwnd);
        if !tray::keep_running() {
            // Quitting: the hidden Quick Search window goes too, or it would keep the app alive.
            quick_search::close(cx);
            return true;
        }
        let raw = hwnd.0 as isize;
        core_bridge::post_to_ui(move || {
            use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
            // SAFETY: hides our own window; a stale handle makes the call fail, nothing else.
            unsafe {
                let _ = ShowWindow(HWND(raw as *mut _), SW_HIDE);
            }
        });
        false
    });
}

/// The main window, once it exists.
pub(crate) fn main_window(cx: &App) -> Option<AnyWindowHandle> {
    cx.try_global::<MainWindow>().map(|main| main.0)
}

/// A notice in the main window (nothing before it exists).
fn notify(cx: &mut App, notification: Notification) {
    if let Some(main) = cx.try_global::<MainWindow>().map(|m| m.0) {
        let _ = main.update(cx, |_, window, cx| window.push_notification(notification, cx));
    }
}

/// The marker for something whose part of the port isn't built yet, as the workspace's toolbar shows it.
fn not_yet(cx: &mut App, what: SharedString) {
    notify(
        cx,
        Notification::info(t!("{what} comes with its part of the port.", what = what)),
    );
}

// ---- Launches, the tray and the shortcut ------------------------------------------------------------------

/// Later launches (File Explorer's "Open in Glint", a double-click, the taskbar's Quick search task) reach the
/// running copy here, once the first folders are open.
fn hear_later_launches(cx: &mut App) {
    let hub = launch::init(cx);
    cx.subscribe(&hub, |_, event: &launch::LaunchEvent, cx| match event {
        launch::LaunchEvent::Open(targets) => {
            let targets = targets
                .iter()
                .map(|target| explorer::LaunchTarget {
                    path: target.path.clone(),
                    select: target.select.clone(),
                })
                .collect();
            explorer::explorer(cx).update(cx, |explorer, cx| explorer.open_requested(targets, cx));
            // Launch already brought the window forward; this maximizes it if it starts in the tray maximized.
            core_bridge::post_to_ui(reveal_main);
        }
        launch::LaunchEvent::QuickSearch => quick_search(cx),
    })
    .detach();
}

/// The shortcut, the tray menu's Quick search, a launch with `--quick-search`: the Quick Search window comes or
/// goes (`summon` toggled in the web app too).
fn quick_search(cx: &mut App) {
    quick_search::toggle(cx);
}

/// The Quick Search window's hub: "show it in Glint" opens the folder (or the file's folder, the file selected)
/// in a new tab and brings the main window forward.
fn start_quick_search(cx: &mut App) {
    let hub = quick_search::init(cx);
    cx.subscribe(&hub, |_, event: &quick_search::QuickSearchEvent, cx| {
        if let quick_search::QuickSearchEvent::Reveal { path, dir } = event {
            let target = if *dir {
                explorer::LaunchTarget {
                    path: path.clone(),
                    select: None,
                }
            } else {
                explorer::LaunchTarget {
                    path: glint_model::paths::parent_path(path),
                    select: Some(path.clone()),
                }
            };
            explorer::explorer(cx).update(cx, |explorer, cx| explorer.open_requested(vec![target], cx));
            core_bridge::post_to_ui(reveal_main);
        }
    })
    .detach();
}

/// What the tray's menu and the shortcut ask of GPUI (opening the window happens in the callback itself).
enum TrayAsk {
    QuickSearch,
    Quit,
}

/// The tray settings last applied: `(shortcut, keep running in the tray, start with Windows)`.
struct TrayApplied(Option<String>, bool, bool);

impl Global for TrayApplied {}

/// The tray icon and the shortcut, as Settings → Quick search says, at start and whenever they change.
/// `in_tray` (a `--tray` start) keeps the icon for the session even with the setting off: it is the only way
/// back to the window.
fn start_tray(cx: &mut App, in_tray: bool) {
    let (asks, heard) = async_channel::unbounded::<TrayAsk>();
    let quick = asks.clone();
    let ready = tray::init(tray::Callbacks {
        // From the tray's own window procedure, outside GPUI: the window comes forward right here, while the
        // click still allows this process to (a posted job could lose that).
        open_main: Arc::new(reveal_main),
        quick_search: Arc::new(move || {
            let _ = quick.try_send(TrayAsk::QuickSearch);
        }),
        quit: Arc::new(move || {
            let _ = asks.try_send(TrayAsk::Quit);
        }),
    });
    if !ready {
        eprintln!("glint: Windows refused the tray's window; the tray and the shortcut are off");
    }
    cx.spawn(async move |cx| {
        while let Ok(ask) = heard.recv().await {
            match ask {
                TrayAsk::QuickSearch => cx.update(quick_search),
                TrayAsk::Quit => cx.update(quit),
            }
        }
    })
    .detach();

    configure_tray(cx, in_tray);
    cx.observe_global::<state::UiState>(move |cx| configure_tray(cx, in_tray))
        .detach();
}

/// Applies the tray settings when they differ from what was applied last (every settings change lands here).
fn configure_tray(cx: &mut App, in_tray: bool) {
    let settings = state::settings(cx);
    let shortcut = settings.quick_shortcut.clone();
    let keep = settings.keep_in_tray || in_tray;
    // Not without the tray: Glint would start where nothing shows it.
    let with_windows = settings.start_with_windows && settings.keep_in_tray;
    if let Some(TrayApplied(s, k, w)) = cx.try_global::<TrayApplied>() {
        if *s == shortcut && *k == keep && *w == with_windows {
            return;
        }
    }
    let shortcut_changed = cx
        .try_global::<TrayApplied>()
        .is_none_or(|applied| applied.0 != shortcut);
    let with_windows_changed = cx
        .try_global::<TrayApplied>()
        .is_none_or(|applied| applied.2 != with_windows);
    cx.set_global(TrayApplied(shortcut.clone(), keep, with_windows));

    tray::set_keep_running(keep);
    if let Err(error) = tray::set_tray(keep) {
        eprintln!("glint: {error}");
    }
    if shortcut_changed {
        // The shortcut's listener thread is started and stopped off the UI thread: it waits for it.
        cx.spawn(async move |cx| {
            let result = cx
                .background_spawn(async move { tray::set_hotkey(shortcut.as_deref()) })
                .await;
            if let Err(error) = result {
                cx.update(|cx| {
                    notify(
                        cx,
                        Notification::error(error).title(t!("Couldn't set the quick search shortcut")),
                    )
                });
            }
        })
        .detach();
    }
    if with_windows_changed {
        // Blocking (the Store's startup task waits on Windows): off the UI thread, best effort, as before.
        cx.background_spawn(async move {
            if let Err(error) = tray::set_start_with_windows(with_windows) {
                eprintln!("glint: couldn't change starting with Windows: {error}");
            }
        })
        .detach();
    }
}

/// The tray menu's Quit Glint: the window's place is kept (a hidden one's was as it hid), then GPUI quits,
/// which saves the state.
fn quit(cx: &mut App) {
    if let Some(hwnd) = main_hwnd() {
        window_state::save_hwnd(hwnd);
    }
    cx.quit();
}

// ---- The explorer and the saved state ----------------------------------------------------------------------

/// The explorer model, given last time's folders and session, keeping its share of the saved state and
/// telling the person what it has to say.
fn start_explorer(cx: &mut App) {
    explorer::init(cx);
    let explorer = explorer::explorer(cx);
    if let Some(saved) = persistence::app(cx).cloned() {
        explorer.update(cx, |explorer, cx| {
            explorer.set_memory(explorer::Memory::from_app_state(saved), cx)
        });
    }
    cx.subscribe(&explorer, |explorer, event: &explorer::ExplorerEvent, cx| match event {
        explorer::ExplorerEvent::MemoryChanged => {
            // The explorer's four parts of the saved state; the rest of its copy is stale.
            let kept = explorer.read(cx).memory().app();
            let last_folder = kept.last_folder.clone();
            let session = kept.session.clone();
            let visits = kept.visits.clone();
            let folder_views = kept.folder_views.clone();
            persistence::update_app(cx, move |app| {
                app.last_folder = last_folder;
                app.session = session;
                app.visits = visits;
                app.folder_views = folder_views;
            });
        }
        explorer::ExplorerEvent::Notice(notice) => notify(cx, notice_for(notice)),
        explorer::ExplorerEvent::Handoff(handoff) => not_yet(
            cx,
            match handoff {
                explorer::Handoff::VersionFile(_) => t!("Opening previous versions"),
                explorer::Handoff::DiskImage(_) => t!("Opening disk images"),
                explorer::Handoff::ArchivedFile(_) => t!("Opening files inside archives"),
            },
        ),
        _ => {}
    })
    .detach();
}

/// A folder's or file's name for a notice: its last part, or the whole path when it has none (a drive).
fn name_of(path: &str) -> String {
    match base_name(path) {
        "" => path.to_string(),
        name => name.to_string(),
    }
}

/// The explorer's notices as the web app worded them.
///
/// PORT ME: their buttons (Get access, Install 7-Zip) and the archive password question come with the
/// parts that own them (access, tools, archives).
fn notice_for(notice: &explorer::Notice) -> Notification {
    use explorer::Notice;
    match notice {
        Notice::CannotOpen { path, message } => {
            Notification::error(message.clone()).title(t!("Can't open “{name}”", name = name_of(path)))
        }
        Notice::ArchivePassword { path, .. } => Notification::info(t!(
            "{what} comes with its part of the port.",
            what = t!("Opening protected archives")
        ))
        .title(t!("“{name}” has a password", name = name_of(path))),
        Notice::NeedsSevenZip { path, .. } => Notification::info(t!(
            "7-Zip reads disc and disk images, installers and packages. Glint installs it in a few seconds."
        ))
        .title(t!("“{name}” opens with 7-Zip", name = name_of(path))),
        Notice::NoLongerAvailable { path } => {
            Notification::info(t!("“{name}” is no longer available", name = name_of(path)))
        }
        Notice::Closed {
            path,
            from_index,
            can_get_access,
            ..
        } => {
            let mut said = if *from_index {
                t!("Glint shows what's in it from instant search, but your account may not open or change anything there.")
                    .to_string()
            } else {
                t!("Your account may not open or change anything there.").to_string()
            };
            if *can_get_access {
                said.push(' ');
                said.push_str(&t!("Get access to open it."));
            }
            Notification::info(said).title(t!("Windows keeps “{name}” closed", name = name_of(path)))
        }
        Notice::InRecycleBin => Notification::info(t!(
            "Restore an item to use it again: it goes back to the folder it was deleted from."
        ))
        .title(t!("In the Recycle Bin")),
        Notice::ReadOnly { remote: true } => Notification::info(t!(
            "Glint opens them to look and to copy out. To change what's on one, use its own app or website."
        ))
        .title(t!("Remote connections are read-only here")),
        Notice::ReadOnly { remote: false } => Notification::info(t!(
            "Glint opens archives to look inside and copy out. To change what's in one, copy it out first."
        ))
        .title(t!("Archives are read-only")),
        Notice::CouldNotOpenFile { path, message } => {
            Notification::error(message.clone()).title(t!("Couldn't open “{name}”", name = name_of(path)))
        }
        Notice::CouldNotGetAccess { path, message } => {
            Notification::error(message.clone()).title(t!("Couldn't get access to “{name}”", name = name_of(path)))
        }
        Notice::Message(text) => Notification::info(text.clone()),
    }
}

// ---- The taskbar ---------------------------------------------------------------------------------------------

/// Back, Forward and Up under the taskbar button's preview follow the folder on screen, and act on it.
fn keep_thumb_buttons(cx: &mut App) {
    let explorer = explorer::explorer(cx);
    let mut shown = None;
    cx.observe(&explorer, move |explorer, cx| {
        let Some(tab) = explorer.read(cx).active_tab() else {
            return;
        };
        let now = (!tab.back.is_empty(), !tab.forward.is_empty(), tab.path != THIS_PC);
        if shown == Some(now) {
            return;
        }
        shown = Some(now);
        core_bridge::post_to_ui(move || {
            if let Some(hwnd) = main_hwnd() {
                window_state::set_thumb_buttons_hwnd(hwnd, now.0, now.1, now.2);
            }
        });
    })
    .detach();

    let clicks = window_state::thumb_button_events();
    cx.spawn(async move |cx| {
        while let Ok(button) = clicks.recv().await {
            cx.update(|cx| {
                explorer::explorer(cx).update(cx, |explorer, cx| match button {
                    window_state::ThumbButton::Back => explorer.go_back(cx).detach(),
                    window_state::ThumbButton::Forward => explorer.go_forward(cx).detach(),
                    window_state::ThumbButton::Up => explorer.go_up(cx).detach(),
                })
            });
        }
    })
    .detach();
}

/// The Jump List shown last: pinned folders, then the most-used ones.
struct JumpListShown(String);

impl Global for JumpListShown {}

/// The taskbar button's Jump List follows the pinned folders and the most-used ones (`updateJumpList`).
fn keep_jump_list(cx: &mut App) {
    update_jump_list(cx);
    cx.observe_global::<persistence::Persistence>(update_jump_list).detach();
}

fn update_jump_list(cx: &mut App) {
    let Some(saved) = persistence::app(cx) else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    let pinned = saved.pinned.clone();
    let mut ranked: Vec<(&str, f64)> = saved
        .visits
        .iter()
        .filter(|(path, _)| !pinned.iter().any(|p| p == path))
        .map(|(path, visit)| (path, glint_model::rank::frecency(visit, now)))
        .collect();
    // Highest first; a stable sort keeps ties in the order they were first visited, as the TypeScript's did.
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let frequent: Vec<String> = ranked.into_iter().take(8).map(|(path, _)| path.to_string()).collect();
    let key = format!("{}\n{}", pinned.join("|"), frequent.join("|"));
    if cx.try_global::<JumpListShown>().is_some_and(|shown| shown.0 == key) {
        return;
    }
    cx.set_global(JumpListShown(key));
    window_state::sync_jump_list_in_background(pinned, frequent);
}
