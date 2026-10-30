//! Quick search from anywhere: a shortcut opens a small search window over every drive, answered by
//! the index, and Glint can keep running in the tray (and start with Windows) so it's always there.

use crate::launch::{bring_forward, bring_to_front, Pending, Target};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const WINDOW: &str = "quick";
const TRAY: &str = "glint";

/// What Settings → Quick search turned on.
#[derive(Default)]
pub struct Quick {
    /// The thread listening for the shortcut (its id and handle), to stop it.
    listener: Mutex<Option<(u32, std::thread::JoinHandle<()>)>>,
    /// Closing the main window hides it in the tray instead of quitting.
    pub keep_running: AtomicBool,
}

/// The search window, made once (hidden) and then shown and hidden.
fn window(app: &AppHandle) -> Option<WebviewWindow> {
    // One at a time: two callers at once (Settings and the shortcut) would each make one.
    static MAKING: Mutex<()> = Mutex::new(());
    let _one = MAKING.lock().unwrap_or_else(|e| e.into_inner());
    app.get_webview_window(WINDOW).or_else(|| {
        WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("quick.html".into()))
            .title("Glint quick search")
            .inner_size(640.0, 440.0)
            .decorations(false)
            .resizable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible(false)
            .focused(false)
            .center()
            .build()
            .ok()
            .inspect(|_window| {
                #[cfg(all(windows, not(debug_assertions)))]
                crate::disable_browser_keys(_window);
            })
    })
}

/// The shortcut and the tray's Quick search: makes the window if it isn't there yet (off the UI
/// thread, where making one can deadlock), then shows or hides it on the UI thread.
pub(crate) fn summon(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        window(&app);
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || toggle(&handle));
    });
}

/// Brings the search window over whatever is in front, or puts it away when it's there.
fn toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window(WINDOW) else { return };
    if window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false) {
        let _ = window.hide();
        return;
    }
    let _ = window.center();
    let _ = window.show();
    bring_forward(&window);
    let _ = window.emit("quick-shown", ());
}

/// "Ctrl+Alt+Space" as RegisterHotKey's modifiers and virtual key.
fn parse(shortcut: &str) -> Option<(u32, u32)> {
    let (mut modifiers, mut key) = (0, None);
    for part in shortcut.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" => modifiers |= 0x2,
            "alt" => modifiers |= 0x1,
            "shift" => modifiers |= 0x4,
            "win" => modifiers |= 0x8,
            "space" => key = Some(0x20),
            other if other.len() == 1 && other.as_bytes()[0].is_ascii_alphanumeric() => key = Some(other.as_bytes()[0].to_ascii_uppercase() as u32),
            other => key = Some(0x6F + other.strip_prefix('f')?.parse::<u32>().ok().filter(|n| (1..=12).contains(n))?),
        }
    }
    Some((modifiers, key?)).filter(|&(modifiers, _)| modifiers != 0)
}

/// Listens for the shortcut on a thread of its own (Windows sends it to the thread that registered
/// it). That thread, or None when another app has the shortcut.
#[cfg(windows)]
fn listen(app: AppHandle, modifiers: u32, key: u32) -> Option<(u32, std::thread::JoinHandle<()>)> {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT};
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || unsafe {
        if RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(modifiers) | MOD_NOREPEAT, key).is_err() {
            let _ = tx.send(None);
            return;
        }
        let _ = tx.send(Some(GetCurrentThreadId()));
        let mut message = MSG::default();
        // Until stop() posts WM_QUIT.
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            if message.message == WM_HOTKEY {
                summon(&app);
            }
        }
        let _ = UnregisterHotKey(None, 1);
    });
    rx.recv().ok().flatten().map(|id| (id, thread))
}

#[cfg(not(windows))]
fn listen(_: AppHandle, _: u32, _: u32) -> Option<(u32, std::thread::JoinHandle<()>)> {
    None
}

/// Stops the listener and waits until it has let the shortcut go, so it can be taken again at once.
fn stop((id, thread): (u32, std::thread::JoinHandle<()>)) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
    }
    let _ = (id, thread.join());
}

/// The tray icon: a click shows Glint; its menu also opens the search window and quits.
fn tray(app: &AppHandle, on: bool) -> tauri::Result<()> {
    if !on {
        app.remove_tray_by_id(TRAY);
        return Ok(());
    }
    if app.tray_by_id(TRAY).is_some() {
        return Ok(());
    }
    let open = MenuItem::with_id(app, "open", "Open Glint", true, None::<&str>)?;
    let search = MenuItem::with_id(app, "search", "Quick search", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Glint", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &search, &quit])?;
    let mut builder = TrayIconBuilder::with_id(TRAY).tooltip("Glint").menu(&menu).show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => bring_to_front(app),
            "search" => summon(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                bring_to_front(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Settings → Quick search: the shortcut (None: off) and whether Glint keeps running in the tray.
/// Async, since making the search window from a command that blocks the UI thread would deadlock.
#[tauri::command]
pub async fn quick_configure(app: AppHandle, quick: State<'_, Quick>, shortcut: Option<String>, tray_icon: bool) -> Result<(), String> {
    // One change at a time: the old shortcut is let go before it's taken again.
    let mut listener = quick.listener.lock().unwrap_or_else(|e| e.into_inner());
    quick.keep_running.store(tray_icon, Ordering::SeqCst);
    tray(&app, tray_icon).map_err(|e| e.to_string())?;
    if let Some(old) = listener.take() {
        stop(old);
    }
    let Some(shortcut) = shortcut.filter(|s| !s.trim().is_empty()) else { return Ok(()) };
    let (modifiers, key) = parse(&shortcut).ok_or_else(|| format!("“{shortcut}” isn't a shortcut Glint knows"))?;
    *listener = Some(listen(app.clone(), modifiers, key).ok_or_else(|| format!("{shortcut} is already used by another app: choose another one in Settings → Quick search"))?);
    drop(listener);
    // Ready before the first press, as a new window takes a moment to start; but not while the main
    // window is starting (that waits for warm), unless Glint starts in the tray, where none shows.
    if READY.load(Ordering::SeqCst) || crate::IN_TRAY.load(Ordering::Relaxed) {
        window(&app);
    }
    Ok(())
}

/// The main window has shown its first folder.
static READY: AtomicBool = AtomicBool::new(false);

/// Makes the search window a second after the main window has shown (app_ready), so the two never start
/// at once, when the shortcut or the tray can call it: then it's ready for the first press.
pub fn warm(app: &AppHandle) {
    if READY.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        let quick = app.state::<Quick>();
        let wanted = quick.keep_running.load(Ordering::SeqCst) || quick.listener.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if wanted {
            window(&app);
        }
    });
}

/// Shows `path` in Glint in a new tab (a folder opened, a file selected in its folder) and puts the
/// search window away.
#[tauri::command]
pub fn quick_reveal(app: AppHandle, path: String, dir: bool) {
    quick_hide(app.clone());
    let target = match std::path::Path::new(&path).parent() {
        Some(folder) if !dir => Target { path: folder.to_string_lossy().into_owned(), select: Some(path) },
        _ => Target { path, select: None },
    };
    app.state::<Pending>().push(target);
    let _ = app.emit("open-requested", ());
    bring_to_front(&app);
}

#[tauri::command]
pub fn quick_hide(app: AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.hide();
    }
}

/// Starts Glint in the tray when this user signs in (the Run key), or stops doing so. Under the
/// app's name and running the exe File Explorer's entries run (src/verbs.rs), as the uninstallers
/// expect, which take it out.
#[tauri::command]
pub async fn set_start_with_windows(app: AppHandle, enabled: bool) -> Result<(), String> {
    crate::fsx::blocking(move || start_with_windows(&app, enabled)).await
}

fn start_with_windows(app: &AppHandle, enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        if crate::packaged() {
            return startup_task(enabled);
        }
        const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
        let name = &app.package_info().name;
        let result = if enabled {
            let exe = crate::verbs::installed_exe().or_else(|| std::env::current_exe().ok()).ok_or("Couldn't tell where Glint is")?;
            crate::verbs::reg::set_string(RUN, name, &format!("\"{}\" --tray", exe.display()))
        } else {
            crate::verbs::reg::delete_value(RUN, name)
        };
        result.map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let _ = (app, enabled);
        Ok(())
    }
}

/// The Microsoft Store copy starts at sign-in by its package's startup task (`GlintTray`, which starts
/// it with --tray), as a Run key it wrote would be its own alone. Windows' Startup apps can turn it
/// off, and then only they can turn it on again.
#[cfg(windows)]
fn startup_task(enabled: bool) -> Result<(), String> {
    use windows::ApplicationModel::{StartupTask, StartupTaskState};
    let task = StartupTask::GetAsync(&"GlintTray".into()).and_then(|task| task.join()).map_err(|e| e.message())?;
    if !enabled {
        return task.Disable().map_err(|e| e.message());
    }
    match task.RequestEnableAsync().and_then(|state| state.join()).map_err(|e| e.message())? {
        StartupTaskState::Enabled | StartupTaskState::EnabledByPolicy => Ok(()),
        StartupTaskState::DisabledByUser => Err("Glint is turned off in Windows' Startup apps (Settings → Apps → Startup)".into()),
        _ => Err("Windows doesn't let Glint start when you sign in".into()),
    }
}

/// Closing the main window: with the tray on it only hides; without, the hidden search window goes
/// too, so Glint really quits.
pub fn on_close(window: &tauri::Window, api: &tauri::CloseRequestApi) {
    if window.label() != "main" {
        return;
    }
    let app = window.app_handle();
    if app.state::<Quick>().keep_running.load(Ordering::SeqCst) {
        api.prevent_close();
        let _ = window.hide();
    } else if let Some(search) = app.get_webview_window(WINDOW) {
        let _ = search.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn reads_shortcuts() {
        assert_eq!(parse("Ctrl+Alt+Space"), Some((0x3, 0x20)));
        assert_eq!(parse("ctrl + shift + f"), Some((0x6, 0x46)));
        assert_eq!(parse("Alt+F12"), Some((0x1, 0x7B)));
        assert_eq!(parse("Space"), None, "a key alone would swallow it everywhere");
        assert_eq!(parse("Ctrl+F13"), None);
        assert_eq!(parse("Ctrl+Alt"), None);
    }
}
