//! Translated text and reading direction: `t!("English text")`.
//!
//! The text is glint-model's catalogs (`glint_model::i18n`, built from `locales/*.po`): English is the key, so
//! `t!("Delete {name}?", name = file)` finds the translation under that exact English and shows the English
//! where the catalog lacks it. Every user-visible string in a view goes through `t!`. A table's names go
//! through [`translate`] where they're drawn, and are marked `english!("…")` where they're written, so
//! `node scripts/i18n.mjs` finds them for the translators (docs/translating.md).
//!
//! **The language** is Windows' display language where Glint has a catalog for it, else English; there is no
//! choice in Settings. `GLINT_LANGUAGE` overrides it ([`init`], at start, before the first frame): `en`,
//! `zh-CN`, `qps-ploc` (the pseudo-language: what skips the catalog stays plain, what gets cut off loses its
//! closing bracket), or a catalog file, `GLINT_LANGUAGE=D:\work\zh-CN.po`, which its language reads instead of
//! the built-in one.
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

/// Start-up, before the theme: picks the language (`GLINT_LANGUAGE`, else Windows' display languages).
pub fn init() {
    let asked = std::env::var("GLINT_LANGUAGE").ok();
    model::set_current(chosen(asked.as_deref(), os_languages));
}

/// The language `GLINT_LANGUAGE` asks for (`asked`), else the first of Windows' display languages Glint has a
/// catalog for. What it can't use it says on stderr and passes over.
fn chosen(asked: Option<&str>, os: impl FnOnce() -> Vec<String>) -> Language {
    let asked = asked.map(str::trim).filter(|value| !value.is_empty());
    if let Some(value) = asked {
        if value.to_ascii_lowercase().ends_with(".po") {
            match model::use_catalog_file(std::path::Path::new(value)) {
                Ok(read) => {
                    eprintln!("GLINT_LANGUAGE: {} translations from {value}", read.translated);
                    for line in &read.left_out {
                        eprintln!("GLINT_LANGUAGE: left out, {line}");
                    }
                    return read.language;
                }
                Err(e) => eprintln!("GLINT_LANGUAGE: {e}; Windows' language instead"),
            }
        } else if let Some(language) = Language::from_code(value) {
            return language;
        } else {
            eprintln!("GLINT_LANGUAGE={value}: Glint has no such language; Windows' language instead");
        }
    }
    let os = os();
    let names: Vec<&str> = os.iter().map(String::as_str).collect();
    model::language_of(&names)
}

/// Windows' display languages in order of preference ("zh-CN", "en-US"). Empty if Windows doesn't answer.
/// One Win32 call: start-up only.
fn os_languages() -> Vec<String> {
    use windows::core::PWSTR;
    use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
    let mut count = 0u32;
    let mut len = 0u32;
    // SAFETY: the first call only asks how many UTF-16 units the list needs (`len`); null is allowed for that.
    if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut count, None, &mut len) }.is_err() || len == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; len as usize];
    // SAFETY: `buffer` has the `len` units Windows just said it needs, and outlives the call.
    if unsafe {
        GetUserPreferredUILanguages(
            MUI_LANGUAGE_NAME,
            &mut count,
            Some(PWSTR(buffer.as_mut_ptr())),
            &mut len,
        )
    }
    .is_err()
    {
        return Vec::new();
    }
    // A double-null-terminated list of null-terminated names.
    buffer
        .split(|&unit| unit == 0)
        .take_while(|name| !name.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// The language in use.
pub fn language() -> Language {
    model::current()
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

/// `text` in the current language. The result borrows static data (the English, or the catalog's text) and
/// costs no allocation.
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

    #[test]
    fn the_language_is_glint_language_else_windows() {
        let windows = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            move || names
        };
        assert_eq!(chosen(None, windows(&["zh-CN", "en-US"])), Language::ZhHans);
        assert_eq!(chosen(None, windows(&["en-US", "zh-CN"])), Language::En);
        assert_eq!(chosen(None, windows(&[])), Language::En);
        assert_eq!(chosen(Some("en"), windows(&["zh-CN"])), Language::En);
        assert_eq!(chosen(Some(" zh-CN "), windows(&["en-US"])), Language::ZhHans);
        assert_eq!(chosen(Some("qps-ploc"), windows(&["en-US"])), Language::Pseudo);
        assert_eq!(
            chosen(Some(""), windows(&["zh-CN"])),
            Language::ZhHans,
            "empty is not set"
        );
        assert_eq!(
            chosen(Some("klingon"), windows(&["zh-CN"])),
            Language::ZhHans,
            "unknown: Windows'"
        );
        // A file that isn't there: said on stderr, and Windows' language instead.
        assert_eq!(chosen(Some(r"Z:\nowhere\zh-CN.po"), windows(&["en-US"])), Language::En);
    }
}
