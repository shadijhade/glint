//! Translated text and reading direction: `t!("English text")`.
//!
//! Glint speaks English (glint-model's i18n): `t!` gives the text with its `{name}` parts filled. Every
//! user-visible string in a view still goes through `t!`, so a language could come back without touching them;
//! 0.13 took out the Arabic catalog and the choice in Settings, and the interface follows Windows' language
//! where Glint has it, which today is English.
//!
//! **Direction.** Views lay themselves out with the kit's start/end helpers (`row_start`, `align_start`,
//! `ps`/`pe`…) and ask [`is_rtl`] where they must. No language Glint speaks is written right to left, so in
//! the app it answers false; the tests turn the mirror on ([`set_mirror`]) to keep the layouts direction-neutral.
//!
//! **Usage:**
//! ```ignore
//! use glint_ui::t;
//! let title = t!("Drive health");                       // SharedString
//! let ask = t!("Delete {name}?", name = file.name);     // placeholders are filled after the lookup
//! ```

use glint_model::i18n::{self as model, Language};
use gpui_kit::SharedString;
use std::sync::atomic::{AtomicBool, Ordering};

/// The layout mirrored, as for a right-to-left language: only ever set by tests.
static MIRROR: AtomicBool = AtomicBool::new(false);

/// The language in use: English.
pub fn language() -> Language {
    model::language_of(&[])
}

/// Whether views mirror themselves now. False in the app; tests set it ([`set_mirror`]).
pub fn is_rtl() -> bool {
    MIRROR.load(Ordering::Relaxed)
}

/// Mirrors the layout (tests that check a view is direction-neutral).
#[cfg(test)]
pub(crate) fn set_mirror(rtl: bool) {
    MIRROR.store(rtl, Ordering::Relaxed);
}

/// `text` in the current language. English is the key, so the result borrows from static data and costs no
/// allocation.
pub fn translate(text: &'static str) -> SharedString {
    SharedString::new_static(model::translate(language(), text))
}

/// [`translate`] with `{name}` placeholders filled in.
pub fn translate_with(text: &'static str, vars: &[(&str, String)]) -> SharedString {
    let vars: Vec<(&str, &str)> = vars.iter().map(|(name, value)| (*name, value.as_str())).collect();
    model::t(language(), text, &vars).into_owned().into()
}

/// Translated text: `t!("Text")`, or `t!("Delete {name}?", name = value)`. The values need only `ToString`.
/// English is the key; see the module notes.
#[macro_export]
macro_rules! t {
    ($text:expr) => {
        $crate::i18n::translate($text)
    };
    ($text:expr, $($name:ident = $value:expr),+ $(,)?) => {
        $crate::i18n::translate_with($text, &[$((stringify!($name), ($value).to_string())),+])
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_fills_placeholders_and_keeps_the_text_as_written() {
        // The mirror is process-wide: tests that change it hold the test lock, as the view tests do.
        let _guard = crate::testing::lock();
        assert_eq!(t!("Drive health"), "Drive health");
        assert_eq!(t!("Delete {name}?", name = "a.txt"), "Delete a.txt?");
        assert_eq!(t!("{n} of {total}", n = 3, total = 10), "3 of 10");
        assert!(!is_rtl(), "nothing in the app mirrors the layout");
        set_mirror(true);
        assert!(is_rtl());
        set_mirror(false);
        assert_eq!(language(), Language::En);
    }
}
