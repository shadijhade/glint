//! Glint's interface on GPUI. The design is docs/gpui/UI-SKELETON.md; the rules are docs/gpui/STANDARDS.md.
//!
//! **Read in this order** if you're about to port a component:
//!
//! 1. [`kit`]: the controls and layout helpers every view uses (and the only place gpui-component is touched).
//! 2. [`theme`] and [`i18n`]: colors/sizes as tokens, and `t!` for every string; right to left.
//! 3. [`core_bridge`]: how a view calls glint-core without waiting, and hears its events.
//! 4. [`actions`]: commands and their keys.
//! 5. [`dialogs::drive_health`]: the exemplar, a whole dialog with model, view, states, accessibility and test.
//! 6. [`workspace`]: the main window the components plug into.
//!
//! **Start-up order** (in [`run`]): `gpui_kit::init`, `state::init`, `theme::init`, `i18n::init`,
//! `actions::init`, `core_bridge::init`, the shared models, then the main window.

pub mod actions;
pub mod core_bridge;
pub mod dialogs;
pub mod i18n;
pub mod kit;
pub mod state;
pub mod theme;
pub mod workspace;

#[cfg(test)]
pub(crate) mod testing;

/// Starts the app and runs until it quits. Returns the exit code core asked for (0 normally); the caller
/// (`Glint.exe`) passes it to the process.
pub fn run() -> i32 {
    // The assets are the icons: without them every icon, the title bar's buttons included, is invisible.
    // `AllAssets`, not `Assets`: the default set embeds only gpui-component's own 101 icons, and any other
    // Lucide icon (HeartPulse, ShieldAlert, file types…) would silently draw nothing. All of them add about
    // 1 MiB to the binary and nothing to memory until an icon is drawn.
    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    app.run(|cx| {
        gpui_kit::init(cx);
        state::init(cx);
        theme::init(cx);
        i18n::init(cx);
        actions::init(cx);
        core_bridge::init(cx);
        dialogs::drive_health::init(cx);

        // One window for now: closing it quits. The tray (Settings → Quick search) changes this later.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        if let Err(error) = workspace::open_main_window(cx) {
            eprintln!("glint: couldn't open the main window: {error:#}");
            cx.quit();
            return;
        }
        cx.activate(true);
    });
    core_bridge::exit_code()
}
