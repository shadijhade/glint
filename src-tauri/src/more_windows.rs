//! More windows: Open in new window, and a tab dragged out of its strip. Each is a window of Glint's
//! own, on the folder it was opened with. The main window keeps what's saved (settings, tabs, history),
//! the folder rules and the tray; the others read its settings as they change.

use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

static NEXT: AtomicU32 = AtomicU32::new(2);

/// Opens a window of its own on `path`, at screen point (`x`, `y`) when given (where a tab was dropped).
#[tauri::command]
pub async fn open_window(app: AppHandle, path: String, x: Option<f64>, y: Option<f64>) -> Result<(), String> {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let label = format!("glint-{n}");
    // What the page opens, before any of its own scripts run.
    let script = format!("window.__GLINT_WINDOW__ = {};", serde_json::json!({ "id": n, "path": path }));
    let (mut width, mut height) = (1100.0, 720.0);
    if let Some(main) = app.get_webview_window("main") {
        if let (Ok(size), Ok(scale)) = (main.inner_size(), main.scale_factor()) {
            width = (size.width as f64 / scale * 0.85).max(500.0);
            height = (size.height as f64 / scale * 0.85).max(360.0);
        }
    }
    let mut builder = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
        .title("Glint")
        .inner_size(width, height)
        .min_inner_size(500.0, 360.0)
        .decorations(false)
        .transparent(true)
        // Shown once the page has painted (app_ready), as the main window is.
        .visible(false)
        .initialization_script(&script);
    builder = match (x, y) {
        (Some(x), Some(y)) => builder.position((x - 120.0).max(0.0), (y - 20.0).max(0.0)),
        _ => builder.center(),
    };
    let window = builder.build().map_err(|e| e.to_string())?;
    #[cfg(all(windows, not(debug_assertions)))]
    crate::disable_browser_keys(&window);
    let _ = window;
    Ok(())
}

/// From another window: the main window comes to the front with Settings open (they're kept there).
#[tauri::command]
pub fn open_main_settings(app: AppHandle, section: Option<String>) {
    crate::launch::bring_to_front(&app);
    let _ = app.emit_to("main", "open-settings", section);
}
