//! What [`crate::run`] starts once the shared services are up, and the glue between the app's services.
//!
//! Each service is a module that knows nothing of the others: one Glint per session ([`launch`]), the saved
//! state ([`persistence`]), the explorer model ([`explorer`]), the tray and the shortcut ([`tray`]), the main
//! window's place and the taskbar's buttons and Jump List ([`window_state`]) and the preview host
//! ([`preview_host`]); drops belong to the window, so the workspace owns its drop target. This file is where
//! they meet: a service exposes calls and events, and the wiring between them lives here and nowhere else.
//!
//! **A secondary window** (a second Glint.exe, `platform::secondary`) goes through the same [`start`] and brings up
//! only what [`services`] says: no folder rules, tray, shortcut, Jump List, taskbar buttons, later launches, hidden
//! Quick Search window, self-test or service update; the window is placed where its command line says instead of
//! where the main window was, and it closes when the main process ends. What it keeps is nothing: see
//! `platform::secondary` and `persistence`.
//!
//! **Win32 calls on the main window go through [`core_bridge::post_to_ui`].** ShowWindow, SetWindowPlacement
//! and SetForegroundWindow send GPUI's window procedure messages at once, and its handlers take the app,
//! which every GPUI callback and entity update already holds: called from one of those, they panic. A posted
//! job runs from the message pump, outside them. (The tray's callbacks already run there.)

use crate::components::quick_search;
use crate::components::settings::tools::describe_progress;
use crate::components::settings::{Backend as _, CoreBackend, Fetch, Fut as SettingsFut};
use crate::kit::prelude::*;
use crate::platform::window_state::ProgressState;
use crate::platform::{launch, preview_host, secondary, tray, window_state};
use crate::{core_bridge, explorer, persistence, state, t, workspace};
use glint_model::paths::{base_name, THIS_PC};
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use windows::Win32::Foundation::HWND;

/// The main window waits this long for its first folder before it is shown without it.
const SHOWN_EMPTY_AFTER: std::time::Duration = std::time::Duration::from_millis(120);

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

/// The main window's content, for what quitting asks (a copy still queued).
struct MainWorkspace(Entity<workspace::Workspace>);

impl Global for MainWorkspace {}

impl Global for MainWindow {}

/// Start-up after `core_bridge::init`: the services that need core, then the main window, then the first
/// folders. `request` is what this copy was started with (folders, `--tray`, `--quick-search`).
pub(crate) fn start(cx: &mut App, request: launch::Request) {
    let secondary_window = secondary::current();
    let services = services(secondary_window.is_some());
    preview_host::init();
    // Instant search's state for the status bar (it polls core's index service in the background).
    crate::components::status_bar::init(cx);
    start_quick_search(cx);
    if services.rules {
        start_rules(cx);
        hear_core_notices(cx);
    }
    glint_core::startup::mark("quick search, rules");
    start_explorer(cx);
    glint_core::startup::mark("explorer");
    // The disk usage maps, reading glint-core (after the explorer, whose tabs they follow).
    crate::components::disk_usage::init(cx);
    // Early, before the saved state is first written: the updater tells a first start after an update by it. (A
    // secondary window's stays idle; see `Updater::start`.)
    start_updater(cx);
    // The system fonts Typst sets documents in, found once in the background (Typst previews, PDF export).
    crate::typst_engine::warm_fonts();
    // Git's status for the folders shown, and File Explorer's on-demand folders the menus offer to hide.
    crate::components::git::init(cx);
    explorer::explorer(cx).update(cx, |e, cx| e.refresh_on_demand(cx).detach());
    // The saved remote connections, handed to core so their folders open (`initRemotes`).
    crate::dialogs::remote::push_to_core(cx);
    if services.tray {
        start_tray(cx, request.tray);
    }
    if services.jump_list {
        keep_jump_list(cx);
    }

    glint_core::startup::mark("services, tray");
    let (window, main_workspace) = match workspace::open_main_window(cx) {
        Ok(opened) => opened,
        Err(error) => {
            eprintln!("glint: couldn't open the main window: {error:#}");
            cx.quit();
            return;
        }
    };
    glint_core::startup::mark("window opened");
    cx.set_global(MainWindow(window));
    cx.set_global(MainWorkspace(main_workspace));
    let _ = window.update(cx, |_, window, cx| {
        launch::set_main_window(window);
        if let Some(hwnd) = core_bridge::hwnd_of(window) {
            MAIN_HWND.store(hwnd.0 as isize, Ordering::Release);
        }
        close_to_tray(window, cx);
    });
    match &secondary_window {
        // Opened hidden: put where its command line says, then shown.
        Some(secondary_window) => place_secondary_window(secondary_window),
        // Opened hidden: put where it was last time. It is shown once its first folder is in (below), unless
        // it starts in the tray.
        None => place_main_window(cx, false),
    }
    glint_core::startup::mark("window placed");
    if services.thumb_buttons {
        keep_thumb_buttons(cx);
    }

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
    // for the first ones, since the restored session would replace tabs placed before it. (A secondary window has
    // no session to restore: its one folder is its one tab.)
    let targets = request
        .open
        .into_iter()
        .map(|target| explorer::LaunchTarget {
            path: target.path,
            select: target.select,
        })
        .collect();
    let first = explorer::explorer(cx).update(cx, |explorer, cx| explorer.init_explorer(targets, cx));
    glint_core::startup::mark("first folders asked for");
    // The main window comes up with its first folder in it: shown empty, it was filled a moment later, and a
    // start seen in two steps looks slower than it is. A folder that takes its time (a sleeping drive, a network
    // share) doesn't hold the window back: it is shown after a moment whatever is in it.
    let reveal: Rc<dyn Fn()> = {
        let wanted = secondary_window.is_none() && !request.tray;
        let shown = Rc::new(std::cell::Cell::new(!wanted));
        Rc::new(move || {
            if !shown.replace(true) {
                glint_core::startup::mark("window shown");
                core_bridge::post_to_ui(|| {
                    if let Some(hwnd) = main_hwnd() {
                        window_state::show_hwnd(hwnd);
                    }
                });
            }
        })
    };
    {
        let reveal = reveal.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(SHOWN_EMPTY_AFTER).await;
            reveal();
        })
        .detach();
    }
    // The main Glint ending ends this window (Quit; a main window hidden in the tray is still running).
    secondary::follow_main_process(cx, |cx| cx.quit());
    cx.spawn(async move |cx| {
        first.await;
        reveal();
        cx.update(|cx| {
            if services.later_launches {
                hear_later_launches(cx);
            }
            // The Quick Search window is made ahead of time now, so the shortcut shows it at once.
            if services.quick_search_window {
                quick_search::warm(cx);
            }
            // Debug builds: GLINT_SELFTEST drives the real app through file operations (selftest.rs).
            if services.selftest {
                crate::selftest::start(cx);
            }
            // After the app updated, instant search's service follows (once per start, quietly if it can).
            if services.service_update {
                if let Some(main) = main_window(cx) {
                    let _ = main.update(cx, |_, window, cx| crate::notices::service_update(window, cx));
                }
            }
        });
    })
    .detach();

    if request.quick_search && services.quick_search_window {
        quick_search(cx);
    }
}

/// What [`start`] brings up. The main window has all of it; a secondary window none (the main process keeps
/// the rules, the tray and the shortcut, the Jump List, the later launches and the service; the self-test is
/// the main window's too).
#[derive(Debug, PartialEq, Eq)]
struct Services {
    /// The folder rules, handed to core to watch.
    rules: bool,
    /// The tray icon, the global shortcut and Start with Windows.
    tray: bool,
    /// The taskbar's Jump List.
    jump_list: bool,
    /// Back, Forward and Up under the taskbar button's preview.
    thumb_buttons: bool,
    /// Folders handed over by later launches, and Settings asked for by a secondary window.
    later_launches: bool,
    /// The hidden Quick Search window made ahead of time (and `--quick-search`).
    quick_search_window: bool,
    /// The debug self-test.
    selftest: bool,
    /// The instant search service following the app's update.
    service_update: bool,
}

fn services(secondary: bool) -> Services {
    let main = !secondary;
    Services {
        rules: main,
        tray: main,
        jump_list: main,
        thumb_buttons: main,
        later_launches: main,
        quick_search_window: main,
        selftest: main,
        service_update: main,
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

/// Puts the hidden secondary window where its command line says (its corner at `--at`, else centred; `--size`
/// else the size it opened with), then shows it. Nothing is kept for next time.
fn place_secondary_window(window: &secondary::SecondaryWindow) {
    let (size, at) = (window.size, window.at);
    core_bridge::post_to_ui(move || {
        let Some(hwnd) = main_hwnd() else { return };
        window_state::place_secondary(hwnd, size, at);
        window_state::show_hwnd(hwnd);
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
/// otherwise lets it close (the last window closing quits; see `run`). Quitting asks first when it would cut a
/// copy or move short, or lose Compare's edits (`mayQuit`); hidden in the tray, nothing stops.
fn close_to_tray(window: &mut Window, cx: &mut App) {
    window.on_window_should_close(cx, |window, cx| {
        let Some(hwnd) = core_bridge::hwnd_of(window) else {
            return true;
        };
        // GetWindowPlacement sends the window nothing, so this is safe inside the callback.
        window_state::save_hwnd(hwnd);
        if !tray::keep_running() {
            if copying(cx) || crate::dialogs::compare_files::unsaved(cx) {
                ask_before_quitting(window, cx, Rc::new(quit_now));
                return false;
            }
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

/// A copy, move or other file operation is running, here or in the copy queue (`operationsRunning`).
fn copying(cx: &App) -> bool {
    explorer::try_explorer(cx).is_some_and(|e| e.read(cx).busy_count() > 0)
        || cx
            .try_global::<MainWorkspace>()
            .is_some_and(|main| main.0.read(cx).copy_queue_running(cx))
}

/// `mayQuit`'s questions, one after the other: a copy still running, then Compare's edits; the window closes
/// (and Glint quits) only on yes to each.
/// What runs once every question is answered yes (the window closes, or the updater quits to install).
type AfterYes = Rc<dyn Fn(&mut Window, &mut App)>;

fn ask_before_quitting(window: &mut Window, cx: &mut App, then: AfterYes) {
    let then_edits = move |window: &mut Window, cx: &mut App| {
        if crate::dialogs::compare_files::unsaved(cx) {
            let then = then.clone();
            crate::dialogs::compare_files::ask_discard(window, cx, move |window, cx| then(window, cx));
        } else {
            then(window, cx);
        }
    };
    if copying(cx) {
        crate::dialogs::confirm(
            window,
            cx,
            t!("Close Glint?"),
            t!("A copy or move is still running, and closing Glint stops it where it is."),
            t!("Close anyway"),
            true,
            then_edits,
        );
    } else {
        then_edits(window, cx);
    }
}

/// Closes the main window after the questions: the hidden Quick Search window goes too, and the app quits.
fn quit_now(window: &mut Window, cx: &mut App) {
    if !secondary::is_secondary() {
        crate::book_places::shared().flush();
    }
    quick_search::close(cx);
    window.remove_window();
}

/// The notice that an update waits for a file operation to end.
struct UpdateWaitingToast;

/// Runs `then` once no file operation is running (`operationsDone` of the web app's updater): at once when none
/// is, else after the last one ends, with a notice saying so. Closing the app would cut a copy or move short.
fn when_operations_done(cx: &mut App, then: Box<dyn FnOnce(&mut App)>) {
    let Some(explorer) = explorer::try_explorer(cx).filter(|e| e.read(cx).busy_count() > 0) else {
        return then(cx);
    };
    notify(
        cx,
        Notification::info(t!("Installing once the file operation finishes…"))
            .id::<UpdateWaitingToast>()
            .autohide(false),
    );
    let waiting = Rc::new(std::cell::RefCell::new(Some(then)));
    cx.subscribe(&explorer, move |explorer, _: &explorer::ExplorerEvent, cx| {
        if explorer.read(cx).busy_count() != 0 {
            return;
        }
        if let Some(then) = waiting.borrow_mut().take() {
            if let Some(main) = main_window(cx) {
                let _ = main.update(cx, |_, window, cx| window.remove_notification::<UpdateWaitingToast>(cx));
            }
            then(cx);
        }
    })
    .detach();
}

/// The updater's question before it quits to install: file operations end first, then `mayQuit`'s, then `go`
/// only on yes (at once when nothing is in the way).
fn confirm_quit_for_update(cx: &mut App, go: Box<dyn FnOnce(&mut App)>) {
    when_operations_done(cx, Box::new(move |cx| ask_quit_for_update(cx, go)));
}

/// [`confirm_quit_for_update`] once no operation runs: a copy still in the queue, or Compare's edits, ask first.
fn ask_quit_for_update(cx: &mut App, go: Box<dyn FnOnce(&mut App)>) {
    let go = Rc::new(std::cell::RefCell::new(Some(go)));
    let run = {
        let go = go.clone();
        move |cx: &mut App| {
            if let Some(go) = go.borrow_mut().take() {
                go(cx)
            }
        }
    };
    match main_window(cx) {
        Some(main) if copying(cx) || crate::dialogs::compare_files::unsaved(cx) => {
            let _ = main.update(cx, move |_, window, cx| {
                ask_before_quitting(window, cx, Rc::new(move |_, cx| run(cx)))
            });
        }
        _ => run(cx),
    }
}

/// Glint keeps itself up to date (never the Store copy or a debug build): latest.json checked at start and every 12
/// hours, the setup's signature checked before it runs; what it says goes to the main window.
fn start_updater(cx: &mut App) {
    let hooks = crate::updater::Hooks {
        confirm_quit: Rc::new(confirm_quit_for_update),
        quit: Rc::new(quit),
    };
    let source = crate::updater::http_source(glint_core::tools::install_tls_provider);
    let updater = crate::updater::init(cx, source, hooks);
    cx.subscribe(&updater, |_, event, cx| {
        if let Some(main) = main_window(cx) {
            let _ = main.update(cx, |_, window, cx| crate::updater::show(event, window, cx));
        }
    })
    .detach();
}

/// The main window's content, once it exists.
pub(crate) fn main_workspace(cx: &App) -> Option<Entity<workspace::Workspace>> {
    cx.try_global::<MainWorkspace>().map(|main| main.0.clone())
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
        // A secondary window's Settings are opened here, where they are kept (`open-settings`).
        launch::LaunchEvent::Settings { section } => {
            open_settings_in_main(cx, section.as_deref());
            core_bridge::post_to_ui(reveal_main);
        }
    })
    .detach();
}

/// Settings in the main window, at `section` when one is named.
fn open_settings_in_main(cx: &mut App, section: Option<&str>) {
    let (Some(main), Some(workspace)) = (main_window(cx), main_workspace(cx)) else {
        return;
    };
    let section = section.map(str::to_string);
    let _ = main.update(cx, move |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_settings_at(section.as_deref(), window, cx)
        })
    });
}

/// The shortcut, the tray menu's Quick search, a launch with `--quick-search`: the Quick Search window comes or
/// goes (`summon` toggled in the web app too).
fn quick_search(cx: &mut App) {
    quick_search::toggle(cx);
}

/// What core tells without being asked, said in the main window: a scheduled backup that finished (App.tsx's
/// "backup-finished" listener) and files a paired PC sent here (`listenForDeviceFiles`, with Show).
fn hear_core_notices(cx: &mut App) {
    let Some(events) = cx.try_global::<core_bridge::Core>().map(|core| core.events.clone()) else {
        return;
    };
    cx.subscribe(&events, |_, event: &core_bridge::CoreEvent, cx| {
        if event.name == "backup-finished" {
            crate::dialogs::backup::tell_finished(&event.payload, |note| notify(cx, note));
        }
    })
    .detach();
    crate::dialogs::nearby::hear_device_files(cx, Rc::new(notify));
}

/// The folder rules saved last time are handed to core, which watches their folders again (`initRules`).
///
/// What rules do from now on is said in the main window as they do it, with Undo (`initRules`' "rule-did" toasts).
fn start_rules(cx: &mut App) {
    let backend: std::sync::Arc<dyn crate::dialogs::rules::Backend> = std::sync::Arc::new(
        crate::dialogs::rules::CoreBackend::new(cx.global::<core_bridge::Core>().app.clone()),
    );
    crate::dialogs::rules::hear_rules(cx, backend, Rc::new(notify));
    let rules: Vec<_> = crate::dialogs::rules::saved_rules(cx)
        .iter()
        .map(crate::dialogs::rules::to_core)
        .collect();
    if rules.is_empty() {
        return;
    }
    let app = cx.global::<core_bridge::Core>().app.clone();
    let started = core_bridge::run(glint_core::rules::rules_set(app, rules));
    cx.spawn(async move |_| {
        if let Ok(Err(error)) = started.await {
            eprintln!("glint: couldn't start the folder rules: {error}");
        }
    })
    .detach();
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
            let wanted = shortcut.clone();
            let result = cx
                .background_spawn(async move { tray::set_hotkey(shortcut.as_deref()) })
                .await;
            match result {
                Ok(_) => cx.update(|cx| crate::components::settings::tell_quick_taken(None, cx)),
                Err(error) => cx.update(|cx| {
                    crate::components::settings::tell_quick_taken(wanted, cx);
                    notify(
                        cx,
                        Notification::error(error).title(t!("Couldn't set the quick search shortcut")),
                    )
                }),
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
    if !secondary::is_secondary() {
        crate::book_places::shared().flush();
    }
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
    // Where the known folders are: asked now, so the answer is there when the first folder needs it.
    explorer.update(cx, |explorer, _| explorer.read_ahead_for_start());
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
        explorer::ExplorerEvent::Notice(notice) => show_notice(cx, notice),
        // A previous version, a disk image, a file inside an archive: copied out, mounted or extracted, then opened.
        explorer::ExplorerEvent::Handoff(handoff) => {
            let handoff = handoff.clone();
            explorer.update(cx, |e, cx| e.handoff(handoff, cx).detach());
        }
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

/// Says what the explorer has to say in the main window: a notification, or for an archive whose list of files
/// is encrypted the password question, then the folder again (`askArchivePassword`).
fn show_notice(cx: &mut App, notice: &explorer::Notice) {
    let Some(main) = main_window(cx) else {
        return;
    };
    let _ = main.update(cx, |_, window, cx| match notice {
        explorer::Notice::ArchivePassword { path, tab, select, .. } => {
            crate::dialogs::password::ask_for_notice(window, cx, path.clone(), *tab, select.clone())
        }
        other => {
            if let Some(note) = notice_for(other) {
                window.push_notification(note, cx);
            }
        }
    });
}

/// "Windows keeps it closed" (the web app's toast id "closed"): one at a time.
struct ClosedToast;
/// "It opens with 7-Zip": the offer to install it.
struct SevenZipToast;
/// 7-Zip being installed: how far it has come.
struct InstallToast;
/// An archive being read whole, keyed by the folder asked for.
struct ReadingToast;

/// The explorer's notices as the web app worded them, with their buttons (Get access, Install 7-Zip). `None`
/// for the one that is a question, not a notice: an archive's password is asked in a dialog ([`show_notice`]).
fn notice_for(notice: &explorer::Notice) -> Option<Notification> {
    use explorer::Notice;
    Some(match notice {
        Notice::CannotOpen { path, message } => {
            Notification::error(message.clone()).title(t!("Can't open “{name}”", name = name_of(path)))
        }
        Notice::ArchivePassword { .. } => return None,
        Notice::NeedsSevenZip { path, tab, select } => {
            let (path, tab, select) = (path.clone(), *tab, select.clone());
            Notification::info(t!(
                "7-Zip reads disc and disk images, installers and packages. Glint installs it in a few seconds."
            ))
            .title(t!("“{name}” opens with 7-Zip", name = name_of(&path)))
            .id::<SevenZipToast>()
            .action(move |_, _, _| {
                let (path, select) = (path.clone(), select.clone());
                Button::new("install-7zip")
                    .label(t!("Install 7-Zip"))
                    .on_click(move |_, window, cx| {
                        window.remove_notification::<SevenZipToast>(cx);
                        install_seven_zip(window, cx, path.clone(), tab, select.clone());
                    })
            })
        }
        Notice::NoLongerAvailable { path } => {
            Notification::info(t!("“{name}” is no longer available", name = name_of(path)))
        }
        Notice::Closed {
            path,
            tab,
            from_index,
            can_get_access,
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
            let note = Notification::info(said)
                .title(t!("Windows keeps “{name}” closed", name = name_of(path)))
                .id::<ClosedToast>();
            if !*can_get_access {
                return Some(note);
            }
            let tab = *tab;
            note.action(move |_, _, _| {
                Button::new("get-access")
                    .label(t!("Get access"))
                    .on_click(move |_, window, cx| {
                        window.remove_notification::<ClosedToast>(cx);
                        if let Some(explorer) = explorer::try_explorer(cx) {
                            explorer.update(cx, |e, cx| e.get_access(Some(tab), cx).detach());
                        }
                    })
            })
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
    })
}

// ---- Installing 7-Zip -----------------------------------------------------------------------------------------

/// How a tool is installed: glint-core's in the app (Settings → Tools' own call), a fake in tests. It gets the
/// tool's id and where to tell how far it has come.
type ToolInstaller = Arc<dyn Fn(String, async_channel::Sender<Fetch>) -> SettingsFut<()> + Send + Sync>;

/// A [`ToolInstaller`] of the tests' own.
#[cfg(test)]
struct ToolInstall(ToolInstaller);

#[cfg(test)]
impl Global for ToolInstall {}

fn tool_installer(cx: &App) -> Option<ToolInstaller> {
    #[cfg(test)]
    if let Some(fake) = cx.try_global::<ToolInstall>() {
        return Some(fake.0.clone());
    }
    let core = cx.try_global::<core_bridge::Core>()?;
    let backend = CoreBackend::new(core.app.clone());
    Some(Arc::new(move |id, progress| backend.tool_install(id, progress)))
}

/// The installing notice, with what the install is doing.
fn installing(fetch: &Fetch) -> Notification {
    Notification::info(describe_progress(fetch))
        .title(t!("Installing {name}", name = t!("7-Zip")))
        .id::<InstallToast>()
        .autohide(false)
}

/// The Install 7-Zip button (`installTool("7zip")`): installs it, with its progress in a notice, and opens `path`
/// again in `tab` once it is there. Failing says why (stopping it says nothing).
fn install_seven_zip(
    window: &mut Window,
    cx: &mut App,
    path: String,
    tab: explorer::TabId,
    select: Option<Vec<String>>,
) {
    let Some(installer) = tool_installer(cx) else {
        return;
    };
    let (progress, heard) = async_channel::unbounded();
    let install = installer("7zip".to_string(), progress);
    window.push_notification(
        installing(&Fetch {
            stage: t!("Starting").to_string(),
            done: 0,
            total: 0,
        }),
        cx,
    );
    // The install's own words arrive until it ends; the ending takes the notice down for good.
    let over = Rc::new(std::cell::Cell::new(false));
    let ended = over.clone();
    window
        .spawn(cx, async move |cx| {
            while let Ok(fetch) = heard.recv().await {
                if over.get() {
                    return;
                }
                if cx
                    .update(|window, cx| window.push_notification(installing(&fetch), cx))
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    window
        .spawn(cx, async move |cx| {
            let result = install.await;
            ended.set(true);
            let _ = cx.update(|window, cx| {
                window.remove_notification::<InstallToast>(cx);
                match result {
                    Ok(()) => {
                        window.push_notification(
                            Notification::success("").title(t!("{name} is installed", name = t!("7-Zip"))),
                            cx,
                        );
                        if let Some(explorer) = explorer::try_explorer(cx) {
                            let options = explorer::NavOptions {
                                select,
                                tab: Some(tab),
                                ..Default::default()
                            };
                            explorer.update(cx, |e, cx| e.navigate(path, options, cx));
                        }
                    }
                    Err(reason) if reason == "cancelled" => {}
                    Err(reason) => window.push_notification(
                        Notification::error(reason).title(t!("Couldn't install {name}", name = t!("7-Zip"))),
                        cx,
                    ),
                }
            });
        })
        .detach();
}

// ---- A big archive read whole ---------------------------------------------------------------------------------

/// What the archive-reading notice is told (the explorer's navigation tells it while a folder takes a moment).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReadingNote {
    /// The first read of `archive` (what `target` is in) has come this far: `done` of `total` compressed bytes.
    Progress {
        target: String,
        archive: String,
        done: u64,
        total: u64,
    },
    /// The folder opened, failed or was stopped: the notice goes, and the taskbar button with it if it was shown.
    Ended { target: String, shown: bool },
}

/// A test's stand-in for the main window: gets every [`ReadingNote`].
#[cfg(test)]
pub(crate) struct ReadingSink(pub(crate) Rc<dyn Fn(&ReadingNote)>);

#[cfg(test)]
impl Global for ReadingSink {}

/// Where the notice's percent stands: whole percent of the file read, at most 99 until it ends, and 0 while the
/// size isn't known (`showArchiveReading`).
pub(crate) fn reading_percent(done: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    let percent = u128::from(done) * 100 / u128::from(total);
    u8::try_from(percent.min(99)).unwrap_or(99)
}

/// The notice's identity: the folder asked for, in any case.
fn reading_key(target: &str) -> usize {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    target.to_lowercase().hash(&mut hasher);
    // Only the notice's key: the hash's low bits are as good as the rest.
    hasher.finish() as usize
}

/// "Reading name… 40%", with what it means and Stop.
fn reading_notice(target: &str, archive: &str, done: u64, total: u64) -> Notification {
    let key = reading_key(target);
    let target = target.to_string();
    Notification::info(t!(
        "It keeps no list of what's in it, so it's read from start to end once."
    ))
    .title(t!(
        "Reading {name}… {percent}%",
        name = name_of(archive),
        percent = reading_percent(done, total)
    ))
    .id1::<ReadingToast>(key)
    .autohide(false)
    .action(move |_, _, _| {
        let target = target.clone();
        Button::new(("stop-reading", key))
            .label(t!("Stop"))
            .on_click(move |_, window, cx| stop_reading(window, cx, &target))
    })
}

/// Stop on the archive-reading notice: the read is stopped (its folder then says "cancelled", which says nothing),
/// and the notice goes. Called from the button, so inside an update of the window.
fn stop_reading(window: &mut Window, cx: &mut App, target: &str) {
    if let Some(explorer) = explorer::try_explorer(cx) {
        explorer.update(cx, |e, cx| e.stop_archive_reading(target, cx));
    }
    let idle = !copying(cx);
    end_reading(window, cx, target, idle);
}

/// Takes the notice for `target` down, and the taskbar button with it when nothing else is running.
fn end_reading(window: &mut Window, cx: &mut App, target: &str, idle: bool) {
    window.remove_notification1::<ReadingToast>(reading_key(target), cx);
    if idle {
        window_state::taskbar_progress(window, ProgressState::None, None, None);
    }
}

/// Shows how far a big archive's first read has come, with Stop, and the taskbar button with it; takes them away
/// when it ends (`showArchiveReading`). Never from inside an update of the explorer or of the window.
pub(crate) fn archive_reading(cx: &mut App, note: ReadingNote) {
    #[cfg(test)]
    if let Some(sink) = cx.try_global::<ReadingSink>().map(|sink| sink.0.clone()) {
        sink(&note);
        return;
    }
    let Some(main) = main_window(cx) else {
        return;
    };
    // Nothing else running: the taskbar button goes with the notice.
    let idle = !copying(cx);
    let _ = main.update(cx, move |_, window, cx| match note {
        ReadingNote::Progress {
            target,
            archive,
            done,
            total,
        } => {
            window.push_notification(reading_notice(&target, &archive, done, total), cx);
            if total > 0 {
                window_state::taskbar_progress(window, ProgressState::Normal, Some(done), Some(total));
            } else {
                window_state::taskbar_progress(window, ProgressState::Indeterminate, None, None);
            }
        }
        ReadingNote::Ended { target, shown } => {
            if shown {
                end_reading(window, cx, &target, idle);
            }
        }
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, FakeDisk};
    use crate::explorer::{init_with, Notice};
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A window with nothing in it but the notification and dialog layers, standing in for the main window.
    struct Blank;

    impl Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// The explorer on a fake disk (a closed folder, a folder with a disc image), the stand-in main window, and
    /// what the fake tool installer was asked to install.
    struct World {
        cx: TestAppContext,
        window: VisualTestContext,
        installs: Arc<Mutex<Vec<String>>>,
    }

    fn world(cx: &mut TestAppContext) -> World {
        cx.update(testing::init);
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![]);
        disk.folder("C:\\secret", vec![file("C:\\secret", "plans.txt", 5, 5)]);
        disk.deny("C:\\secret");
        disk.folder("C:\\images", vec![file("C:\\images", "a.iso", 5, 5)]);
        cx.update(|cx| init_with(cx, disk.source()));
        let (mut window, _) = testing::open_window(cx, |_, cx| cx.new(|_| Blank));
        window.update(|window, cx| cx.set_global(MainWindow(window.window_handle())));
        let opened = cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.init_explorer(Vec::new(), cx)));
        window.foreground_executor().block_test(opened);
        World {
            cx: cx.clone(),
            window,
            installs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    impl World {
        fn path(&mut self) -> String {
            self.cx
                .update(|cx| explorer::explorer(cx).read(cx).current_path().to_string())
        }

        fn tab(&mut self) -> explorer::TabId {
            self.cx.update(|cx| explorer::explorer(cx).read(cx).active_tab_id())
        }

        fn closed_tab(&mut self) -> bool {
            self.cx.update(|cx| {
                explorer::explorer(cx)
                    .read(cx)
                    .active_tab()
                    .is_some_and(|t| t.no_access().is_some())
            })
        }

        fn go(&mut self, path: &str) {
            let task = self.cx.update(|cx| {
                explorer::explorer(cx).update(cx, |e, cx| e.navigate_task(path, explorer::NavOptions::default(), cx))
            });
            self.window.foreground_executor().block_test(task);
        }

        fn say(&mut self, notice: Notice) {
            self.cx.update(|cx| show_notice(cx, &notice));
            self.settle();
        }

        fn notifications(&mut self) -> usize {
            self.window.update(|window, cx| window.notifications(cx).len())
        }

        fn dialog(&mut self) -> bool {
            self.window.update(|window, cx| window.has_active_dialog(cx))
        }

        /// Whether a control of that name is on screen (the notifications' buttons are).
        fn has(&mut self, label: &str) -> bool {
            self.window.update(|window, cx| {
                window.render_frame(cx);
                window.try_find_by_label(label).is_some()
            })
        }

        fn press(&mut self, label: &str) {
            self.window.update(|window, cx| {
                window.render_frame(cx);
                let id = window
                    .find_by_label(label)
                    .path()
                    .last()
                    .cloned()
                    .expect("a button with an id");
                window.click(id, cx);
            });
            self.settle();
        }

        /// Lets what is under way finish, and the notifications that were taken down leave.
        fn settle(&mut self) {
            for _ in 0..40 {
                self.window.run_until_parked();
                std::thread::sleep(Duration::from_millis(2));
            }
            self.window.executor().advance_clock(Duration::from_millis(500));
            self.window.run_until_parked();
        }

        /// Installs end with `answer`, and are recorded.
        fn install_with(&mut self, answer: Result<(), String>) {
            let seen = self.installs.clone();
            let installer: ToolInstaller = Arc::new(move |id, progress| {
                if let Ok(mut seen) = seen.lock() {
                    seen.push(id);
                }
                let _ = progress.try_send(Fetch {
                    stage: "Downloading".into(),
                    done: 1,
                    total: 2,
                });
                let answer = answer.clone();
                Box::pin(async move { answer })
            });
            self.cx.update(|cx| cx.set_global(ToolInstall(installer)));
        }

        fn installed(&self) -> Vec<String> {
            self.installs.lock().map(|seen| seen.clone()).unwrap_or_default()
        }
    }

    fn closed(tab: explorer::TabId, can_get_access: bool) -> Notice {
        Notice::Closed {
            path: "C:\\secret".into(),
            tab,
            from_index: false,
            can_get_access,
        }
    }

    #[test]
    fn a_secondary_window_brings_up_none_of_the_main_windows_services() {
        let none = Services {
            rules: false,
            tray: false,
            jump_list: false,
            thumb_buttons: false,
            later_launches: false,
            quick_search_window: false,
            selftest: false,
            service_update: false,
        };
        assert_eq!(services(true), none);
        let all = services(false);
        assert!(
            all.rules
                && all.tray
                && all.jump_list
                && all.thumb_buttons
                && all.later_launches
                && all.quick_search_window
                && all.selftest
                && all.service_update,
            "the main window has every one: {all:?}"
        );
    }

    #[gpui_kit::test]
    fn a_closed_folders_notice_offers_get_access_and_the_button_opens_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut w = world(cx);
        w.go("C:\\secret");
        assert!(w.closed_tab());
        let tab = w.tab();
        // Where Get access can't help (a remote, or Windows already refused) there is no button.
        w.say(closed(tab, false));
        assert_eq!(w.notifications(), 1);
        assert!(!w.has("Get access"));
        // Where it can, the notice replaces that one (one at a time) and its button asks Windows.
        w.say(closed(tab, true));
        assert_eq!(w.notifications(), 1);
        assert!(w.has("Get access"));
        w.press("Get access");
        assert!(!w.closed_tab(), "the folder opens in place");
        assert_eq!(w.path(), "C:\\secret");
        assert_eq!(w.notifications(), 0, "the notice goes with the click");
    }

    #[gpui_kit::test]
    fn a_disc_image_before_7zip_offers_to_install_it_and_opens_again_once_it_is_there(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut w = world(cx);
        w.go("C:\\");
        let tab = w.tab();
        let offer = || Notice::NeedsSevenZip {
            path: "C:\\images".into(),
            tab,
            select: Some(vec!["C:\\images\\a.iso".into()]),
        };
        w.say(offer());
        assert_eq!(w.notifications(), 1);
        assert!(w.has("Install 7-Zip"));
        w.install_with(Ok(()));
        w.press("Install 7-Zip");
        assert_eq!(w.installed(), ["7zip"]);
        assert_eq!(w.path(), "C:\\images", "opened again once it was installed");
        assert_eq!(
            w.notifications(),
            1,
            "'7-Zip is installed', and the offer and the progress are gone"
        );

        // Failing says why and leaves the folder where it was.
        w.go("C:\\");
        w.say(offer());
        w.install_with(Err("No network".into()));
        w.press("Install 7-Zip");
        assert_eq!(w.installed(), ["7zip", "7zip"]);
        assert_eq!(w.path(), "C:\\");
        assert_eq!(w.notifications(), 2, "the earlier 'installed' and the failure");

        // Stopping it says nothing.
        let before = w.notifications();
        w.say(offer());
        w.install_with(Err("cancelled".into()));
        w.press("Install 7-Zip");
        assert_eq!(w.path(), "C:\\");
        assert_eq!(w.notifications(), before);
    }

    #[gpui_kit::test]
    fn an_archive_with_a_password_is_asked_for_in_a_dialog_not_told_in_a_notice(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut w = world(cx);
        let tab = w.tab();
        let notice = Notice::ArchivePassword {
            path: "C:\\secret.7z".into(),
            tab,
            select: None,
            message: "needs a password".into(),
        };
        assert!(notice_for(&notice).is_none());
        w.say(notice);
        assert!(w.dialog(), "the password question");
        assert_eq!(w.notifications(), 0);
    }

    #[gpui_kit::test]
    fn the_other_notices_are_plain_notifications(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut w = world(cx);
        w.say(Notice::InRecycleBin);
        w.say(Notice::ReadOnly { remote: false });
        w.say(Notice::Message("Can't find “x”.".into()));
        assert_eq!(w.notifications(), 3);
        assert!(!w.dialog());
    }

    #[gpui_kit::test]
    fn the_archive_reading_notice_comes_with_stop_follows_the_read_and_goes_when_it_ends(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut w = world(cx);
        let progress = |done| ReadingNote::Progress {
            target: "C:\\big.tar.gz".into(),
            archive: "C:\\big.tar.gz".into(),
            done,
            total: 100,
        };
        w.cx.update(|cx| archive_reading(cx, progress(10)));
        w.settle();
        assert_eq!(w.notifications(), 1);
        assert!(w.has("Stop"));
        // Asked again as it goes: the same notice, not another.
        w.cx.update(|cx| archive_reading(cx, progress(60)));
        w.settle();
        assert_eq!(w.notifications(), 1);
        // The read ends: it goes.
        w.cx.update(|cx| {
            archive_reading(
                cx,
                ReadingNote::Ended {
                    target: "C:\\BIG.tar.gz".into(),
                    shown: true,
                },
            )
        });
        w.settle();
        assert_eq!(w.notifications(), 0, "found by the folder in any case");
        // Stop takes the notice down by itself.
        w.cx.update(|cx| archive_reading(cx, progress(30)));
        w.settle();
        w.press("Stop");
        assert_eq!(w.notifications(), 0);
    }

    #[gpui_kit::test]
    fn an_update_waits_for_a_file_operation_to_end_and_says_so(cx: &mut TestAppContext) {
        use crate::explorer::fake::FakeDisk;
        let _guard = testing::lock();
        cx.update(testing::init);
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![]);
        disk.folder("C:\\a", vec![file("C:\\a", "one.txt", 1, 1)]);
        disk.folder("C:\\b", vec![]);
        cx.update(|cx| init_with(cx, disk.source()));
        let (mut window, _) = testing::open_window(cx, |_, cx| cx.new(|_| Blank));
        window.update(|window, cx| cx.set_global(MainWindow(window.window_handle())));
        let opened = cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.init_explorer(Vec::new(), cx)));
        window.foreground_executor().block_test(opened);
        let went = cx.update(|cx| {
            explorer::explorer(cx).update(cx, |e, cx| {
                e.navigate_task("C:\\b", explorer::NavOptions::default(), cx)
            })
        });
        window.foreground_executor().block_test(went);

        // Nothing running: it goes ahead at once, with no notice.
        let ran = Rc::new(std::cell::Cell::new(0));
        let again = ran.clone();
        cx.update(|cx| when_operations_done(cx, Box::new(move |_| again.set(again.get() + 1))));
        assert_eq!(ran.get(), 1);

        // A paste under way: it waits, and the notice says why.
        let gate = disk.hold_ops();
        disk.set_clipboard(&["C:\\a\\one.txt"], false);
        let pasting = cx.update(|cx| explorer::explorer(cx).update(cx, |e, cx| e.paste(None, cx)));
        window.run_until_parked();
        assert_eq!(cx.update(|cx| explorer::explorer(cx).read(cx).busy_count()), 1);
        let again = ran.clone();
        cx.update(|cx| when_operations_done(cx, Box::new(move |_| again.set(again.get() + 1))));
        window.run_until_parked();
        assert_eq!(ran.get(), 1, "waiting");
        let said = window.update(|window, cx| window.notifications(cx).len());
        assert_eq!(said, 1, "a notice says what it waits for");

        // The paste ends: the update goes ahead, once, and the notice goes.
        gate.close();
        window.foreground_executor().block_test(pasting);
        for _ in 0..40 {
            window.run_until_parked();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(ran.get(), 2, "ran once the operation ended");
        // The notice leaves once its exit has played.
        window.executor().advance_clock(Duration::from_millis(500));
        window.run_until_parked();
        let said = window.update(|window, cx| window.notifications(cx).len());
        assert_eq!(said, 0, "the notice is gone");
    }

    #[test]
    fn the_percent_is_whole_stays_below_a_hundred_until_the_end_and_is_nothing_without_a_size() {
        assert_eq!(reading_percent(0, 100), 0);
        assert_eq!(reading_percent(25, 100), 25);
        assert_eq!(reading_percent(2, 3), 66);
        assert_eq!(reading_percent(100, 100), 99);
        assert_eq!(reading_percent(200, 100), 99);
        assert_eq!(reading_percent(5, 0), 0);
        assert_eq!(reading_percent(u64::MAX, u64::MAX), 99);
    }

    #[test]
    fn a_notices_key_is_the_same_in_any_case_and_differs_between_folders() {
        assert_eq!(reading_key("C:\\Big.tar.gz"), reading_key("c:\\big.TAR.gz"));
        assert_ne!(reading_key("C:\\a.tar.gz"), reading_key("C:\\b.tar.gz"));
    }
}
