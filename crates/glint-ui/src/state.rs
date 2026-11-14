//! The live settings the interface reads: one `Settings` (from glint-model, the shape the old app saved)
//! in a GPUI `Global`.
//!
//! The TypeScript kept these in a zustand store. glint-model ports only the types, defaults and pure update
//! functions (STANDARDS.md section 4), so the UI layer owns the live copy, and it lives here.
//!
//! **Pattern for the component waves.** Read with [`settings`]. Change with [`update`], never by mutating
//! the global yourself: `update` re-applies everything that follows from a setting (the theme and its
//! accent, the language and its direction) and refreshes every window.
//!
//! [`init`] puts the defaults in place; `persistence::init`, right after it, loads the saved settings
//! through [`update`], and every later [`update`] is saved soon. Code that wants to react to a setting
//! observes this global (`cx.observe_global::<UiState>`): every [`update`] notifies its observers.
//!
//! **In a secondary window** (`platform::secondary`) the settings are the main window's: `persistence::init`
//! reads them, nothing here is ever saved (the saver isn't there), and when the main window changes them
//! `persistence::apply_reloaded` hands the new ones to [`update`], so the theme and the language follow.

use glint_model::store::Settings;
use gpui_kit::{App, Global};

/// The settings in use, shared by every window.
pub struct UiState {
    /// What the settings dialog edits and the theme and language follow.
    pub settings: Settings,
}

impl Global for UiState {}

/// Puts the default settings in place. First in the start-up order: the theme and the language read them.
///
/// Debug builds also read two environment variables, for looking at the interface in the other mode without
/// opening Settings: `GLINT_LANGUAGE=ar` (or `en`) and `GLINT_THEME=light` (or `dark`). Release builds ignore them.
pub fn init(cx: &mut App) {
    let mut settings = Settings::default();
    if cfg!(debug_assertions) {
        match std::env::var("GLINT_LANGUAGE").as_deref() {
            Ok("ar") => settings.language = glint_model::store::Language::Ar,
            Ok("en") => settings.language = glint_model::store::Language::En,
            _ => {}
        }
        match std::env::var("GLINT_THEME").as_deref() {
            Ok("light") => settings.theme = glint_model::store::Theme::Light,
            Ok("dark") => settings.theme = glint_model::store::Theme::Dark,
            _ => {}
        }
    }
    cx.set_global(UiState { settings });
}

/// The settings now.
pub fn settings(cx: &App) -> &Settings {
    &cx.global::<UiState>().settings
}

/// Changes the settings, then re-applies what depends on them and redraws every window. The language and the
/// theme are applied again only when a setting they read changed: most changes (a sort, a zoom notch, a column's
/// width) touch neither.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    let looks = |s: &Settings| (s.theme, s.accent, s.backdrop, s.font_size, s.language);
    let before = looks(settings(cx));
    change(&mut cx.global_mut::<UiState>().settings);
    if looks(settings(cx)) != before {
        crate::i18n::apply_setting(cx);
        crate::theme::apply(cx);
    }
    // Saved soon (nothing before `persistence::init`, which loads the saved settings through here).
    crate::persistence::sync(cx);
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::store::Theme;
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn update_changes_the_settings_everyone_reads(cx: &mut TestAppContext) {
        let _guard = crate::testing::lock();
        cx.update(|cx| {
            crate::testing::init(cx);
            assert_eq!(settings(cx).theme, Theme::System);
            update(cx, |s| s.theme = Theme::Dark);
            assert_eq!(settings(cx).theme, Theme::Dark);
            assert!(crate::theme::tokens(cx).dark, "the tokens follow the setting");
            update(cx, |s| s.theme = Theme::Light);
            assert!(!crate::theme::tokens(cx).dark);
            // The language follows its setting too, and the direction with it.
            update(cx, |s| s.language = glint_model::store::Language::Ar);
            assert!(crate::i18n::is_rtl() && crate::i18n::is_rtl_in(cx));
            update(cx, |s| s.language = glint_model::store::Language::En);
            assert!(!crate::i18n::is_rtl());
        });
    }

    #[gpui_kit::test]
    fn only_a_change_the_theme_reads_applies_it_again(cx: &mut TestAppContext) {
        let _guard = crate::testing::lock();
        cx.update(|cx| {
            crate::testing::init(cx);
            let start = crate::theme::times_applied(cx);
            update(cx, |s| s.show_hidden = !s.show_hidden);
            assert_eq!(
                crate::theme::times_applied(cx),
                start,
                "showing hidden items leaves the theme alone"
            );
            update(cx, |s| s.theme = Theme::Dark);
            assert_eq!(crate::theme::times_applied(cx), start + 1);
            update(cx, |s| s.theme = Theme::Dark);
            assert_eq!(
                crate::theme::times_applied(cx),
                start + 1,
                "the same theme again changes nothing"
            );
        });
    }
}
