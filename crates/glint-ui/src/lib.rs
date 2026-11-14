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
//! **Start-up order** (in [`run`]): one copy per session (`platform::launch::begin`), then `gpui_kit::init`,
//! `state::init`, `persistence::init`, `theme::init`, `i18n::init`, `actions::init`, `core_bridge::init`;
//! then `startup::start`: the preview host, the explorer and its saved share, the tray, the Jump List, the
//! main window (hidden, placed, shown), the first folders and later launches. startup.rs is where the
//! services meet.

pub mod actions;
/// Glint's own pictures: folders, pages, chips, and the light behind the window.
pub mod art;
pub mod book_places;
pub mod components;
pub mod core_bridge;
pub mod dialogs;
pub mod drops;
pub mod explorer;
pub mod i18n;
pub mod kit;
pub mod notices;
/// The old app's settings, read from WebView2's localStorage at the first start (wave 16).
pub mod old_settings;
/// Timings for measuring, among the frame log's lines (`GLINT_FRAME_LOG`).
mod perf;
pub mod persistence;
pub mod platform;
mod selftest;
mod startup;
pub mod state;
pub mod theme;
/// Typst in process: a World over the disk and the system fonts, compiling to pages and PDF.
/// Glint keeping itself up to date: latest.json, the signed installer, the Update button.
pub mod updater;
pub mod workspace;

#[cfg(test)]
pub(crate) mod testing;
/// Wrapped right-to-left paragraphs on the real DirectWrite (the vendored GPUI's line layout).
#[cfg(test)]
mod rtl_wrap_tests;
/// Every reachable control named, and the bars mirrored in Arabic, across the main views (wave 17).
#[cfg(test)]
mod a11y_tests;

/// Starts the app and runs until it quits. Returns the exit code core asked for (0 normally); the caller
/// (`Glint.exe`) passes it to the process.
pub fn run() -> i32 {
    // One Glint per session: a later launch hands its folders to the running copy and ends here, before any
    // window or GPUI. (Glint.exe has already answered as a worker if it was started as one.)
    let request = match platform::launch::begin() {
        platform::launch::Startup::Exit => return 0,
        platform::launch::Startup::Run(request) => request,
    };
    // The assets are the icons: without them every icon, the title bar's buttons included, is invisible.
    // `AllAssets`, not `Assets`: the default set embeds only gpui-component's own 101 icons, and any other
    // Lucide icon (HeartPulse, ShieldAlert, file types…) would silently draw nothing. All of them add about
    // 1 MiB to the binary and nothing to memory until an icon is drawn.
    glint_core::startup::mark("single instance");
    let app = gpui_kit::application().with_assets(gpui_kit::assets::AllAssets);
    glint_core::startup::mark("gpui application");
    app.run(move |cx| {
        glint_core::startup::mark("gpui running");
        gpui_kit::init(cx);
        glint_core::startup::mark("kit");
        state::init(cx);
        // The saved state, its settings first: the theme and the language read them.
        persistence::init(cx);
        glint_core::startup::mark("saved state");
        theme::init(cx);
        i18n::init(cx);
        actions::init(cx);
        glint_core::startup::mark("theme, language, actions");
        core_bridge::init(cx);
        glint_core::startup::mark("core");
        dialogs::drive_health::init(cx);

        // Closing the last window quits. With the tray keeping Glint running, closing only hides the window
        // (startup.rs), so this doesn't come.
        cx.on_window_closed(|cx, _| {
            // The Quick Search window, made ahead of time and only ever hidden, doesn't keep the app open.
            let windows = cx.windows();
            if windows
                .iter()
                .all(|window| components::quick_search::is_quick_window(cx, *window))
            {
                components::quick_search::close(cx);
                cx.quit();
            }
        })
        .detach();

        startup::start(cx, request);
        cx.activate(true);
    });
    // The tray icon and the shortcut go with the app.
    platform::tray::shutdown();
    core_bridge::exit_code()
}
