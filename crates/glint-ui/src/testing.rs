//! Helpers for the headless window tests (compiled for tests only).
//!
//! Language and reading direction are process-wide (see `i18n`), and the test runner runs tests on several
//! threads, so every test that renders a view or flips the direction holds [`lock`] for its whole body.

use crate::{actions, core_bridge, dialogs, i18n, state, theme};
use glint_model::i18n::Language;
use glint_model::store::{Backdrop, Language as LanguageChoice};
use gpui_kit::base::Root;
use gpui_kit::{
    px, size, App, Bounds, Entity, Render, TestAppContext, VisualTestContext, Window, WindowBounds, WindowOptions,
};
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

/// Serialises tests that read or change the language. Survives a failed test (poisoning is ignored), so one
/// failure doesn't turn every later test red.
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sets the interface direction without an app: Arabic when `rtl`, else English.
pub(crate) fn set_rtl(rtl: bool) {
    i18n::set_mirror(if rtl { Language::Ar } else { Language::En });
}

/// The start-up sequence of `run`, minus what needs the real PC: English whatever Windows speaks, no Mica,
/// no accent watcher, no housekeeping threads. After this a test can open windows.
pub(crate) fn init(cx: &mut App) {
    gpui_kit::init(cx);
    state::init(cx);
    state::update(cx, |settings| {
        settings.language = LanguageChoice::En;
        settings.backdrop = Backdrop::Solid;
    });
    i18n::set_language(cx, Language::En);
    theme::apply(cx);
    actions::init(cx);
    core_bridge::init_with(cx, false);
    dialogs::drive_health::init(cx);
}

/// Opens a window the way the app does (`gpui_kit::open_window`, so the dialog and notification layers are
/// there) at a fixed 1000 x 700, and gives a context to drive it. `build` makes the content view.
pub(crate) fn open_window<V: Render>(
    cx: &mut TestAppContext,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> (VisualTestContext, Entity<V>) {
    // Core's work runs on its own runtime's threads and wakes the foreground tasks that wait for it. The test
    // scheduler would call that nondeterministic and panic; parking is the opt-in for tests that do real work.
    cx.executor().allow_parking();
    let (handle, view) = cx.update(|cx| {
        cx.set_reduce_motion(true);
        let bounds = Bounds {
            origin: Default::default(),
            size: size(px(1000.), px(700.)),
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, build).expect("the test window opens")
    });
    debug_assert!(handle.downcast::<Root>().is_some());
    let visual = VisualTestContext::from_window(handle, cx);
    visual.run_until_parked();
    (visual, view)
}
