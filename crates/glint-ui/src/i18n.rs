//! Translated text and reading direction: `t!("English text")`.
//!
//! The strings are glint-model's catalogs (`glint_model::i18n`, ported from src/lib/i18n.ts): English is the
//! key, so `t!("Delete {name}?", name = file)` finds the Arabic text under that exact English text and falls
//! back to the English itself when a catalog lacks it. Every user-visible string in a view goes through `t!`.
//!
//! **The language lives in two places on purpose.** A `Global` ([`Current`]) is what code with a `cx` reads
//! ([`language_in`], [`is_rtl_in`]); a static atomic mirrors it so `t!` and the layout helpers
//! ([`crate::kit::row_start`]…) work anywhere, including inside `render` closures that have no `cx`.
//! [`set_language`] is the only writer, so the two never disagree.
//!
//! **Right to left.** `is_rtl()` tells a view to mirror itself. Mirroring is done with the kit's helpers
//! (`row_start`, `align_start`, `ps`/`pe`…), never with left/right assumptions in a view.
//!
//! **Usage:**
//! ```ignore
//! use glint_ui::t;
//! let title = t!("Drive health");                       // SharedString
//! let ask = t!("Delete {name}?", name = file.name);     // placeholders are filled after the lookup
//! ```

use glint_model::i18n::{self as model, Language};
use glint_model::store::Language as LanguageChoice;
use gpui_kit::{App, Global, SharedString};
use std::sync::atomic::{AtomicU8, Ordering};

/// 0 is English, 1 is Arabic: `Language` as a byte, for the lock-free mirror `t!` reads.
static MIRROR: AtomicU8 = AtomicU8::new(0);

fn to_byte(language: Language) -> u8 {
    match language {
        Language::En => 0,
        Language::Ar => 1,
    }
}

/// The language in use, as the interface now speaks it. Works anywhere, with or without a `cx`.
pub fn language() -> Language {
    match MIRROR.load(Ordering::Relaxed) {
        1 => Language::Ar,
        _ => Language::En,
    }
}

/// Whether the interface is written right to left now (Arabic). Views mirror themselves when it is.
pub fn is_rtl() -> bool {
    model::is_rtl(language())
}

/// The `Global` behind [`language_in`]; see the module notes for why there is also an atomic.
pub struct Current(pub Language);

impl Global for Current {}

/// The language in use, read through the app (what `cx`-holding code asks).
#[cfg(test)]
pub fn language_in(cx: &App) -> Language {
    cx.try_global::<Current>().map_or(Language::En, |current| current.0)
}

/// Right to left, read through the app.
#[cfg(test)]
pub fn is_rtl_in(cx: &App) -> bool {
    model::is_rtl(language_in(cx))
}

/// Windows' UI languages in order of preference ("ar-SA", "en-US"), as `navigator.languages` gave the page.
/// Empty if Windows doesn't answer. Cheap (one Win32 call), but not for a render path: call it at start.
pub fn os_languages() -> Vec<String> {
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

/// The language the settings ask for: the one chosen, else Windows' own where Glint speaks it.
pub fn language_for(choice: LanguageChoice, os_languages: &[String]) -> Language {
    let setting = match choice {
        LanguageChoice::System => None,
        LanguageChoice::En => Some("en"),
        LanguageChoice::Ar => Some("ar"),
    };
    let os: Vec<&str> = os_languages.iter().map(String::as_str).collect();
    model::language_of(setting, &os)
}

/// Sets the language everywhere. The only writer of the language: the atomic and the `Global` change together.
pub fn set_language(cx: &mut App, language: Language) {
    set_mirror(language);
    cx.set_global(Current(language));
}

/// The atomic half of [`set_language`], for the tests that flip direction without an app.
pub(crate) fn set_mirror(language: Language) {
    MIRROR.store(to_byte(language), Ordering::Relaxed);
}

/// Start-up step 3: reads the setting (and Windows' languages) and sets the language.
pub fn init(cx: &mut App) {
    cx.set_global(OsLanguages(os_languages()));
    apply_setting(cx);
}

/// Windows' languages, read once at start, so a settings change doesn't ask Windows again.
struct OsLanguages(Vec<String>);

impl Global for OsLanguages {}

/// Sets the language the settings now ask for (called at start and by `state::update`).
pub fn apply_setting(cx: &mut App) {
    let choice = crate::state::settings(cx).language;
    let os = cx
        .try_global::<OsLanguages>()
        .map(|os| os.0.clone())
        .unwrap_or_default();
    set_language(cx, language_for(choice, &os));
}

/// `text` in the current language. English text is the key; what the catalog lacks stays English.
/// The macro [`t!`] calls this with a literal, so the result borrows from static data and costs no allocation.
pub fn translate(text: &'static str) -> SharedString {
    SharedString::new_static(model::translate(language(), text))
}

/// [`translate`] with `{name}` placeholders filled in, after the lookup, so the catalog keeps them.
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
    fn language_follows_the_choice_then_windows() {
        let arabic_pc = vec!["ar-SA".to_string(), "en-US".to_string()];
        assert_eq!(language_for(LanguageChoice::En, &arabic_pc), Language::En);
        assert_eq!(language_for(LanguageChoice::Ar, &[]), Language::Ar);
        assert_eq!(language_for(LanguageChoice::System, &arabic_pc), Language::Ar);
        assert_eq!(
            language_for(LanguageChoice::System, &["de-DE".to_string()]),
            Language::En
        );
        assert_eq!(language_for(LanguageChoice::System, &[]), Language::En);
    }

    #[test]
    fn windows_answers_with_language_names() {
        // Whatever this PC speaks, names look like "en-US" and none is empty.
        for name in os_languages() {
            assert!(
                !name.is_empty() && name.contains(|c: char| c.is_ascii_alphabetic()),
                "{name:?}"
            );
        }
    }

    #[test]
    fn t_fills_placeholders_and_keeps_unknown_text_in_english() {
        // The mirror is process-wide: tests that change it hold the test lock, as the view tests do.
        let _guard = crate::testing::lock();
        MIRROR.store(0, Ordering::Relaxed);
        assert_eq!(t!("Drive health"), "Drive health");
        assert_eq!(t!("Delete {name}?", name = "a.txt"), "Delete a.txt?");
        assert_eq!(t!("{n} of {total}", n = 3, total = 10), "3 of 10");
        MIRROR.store(1, Ordering::Relaxed);
        assert!(is_rtl());
        // Text nobody translated stays as written, placeholders still filled.
        assert_eq!(t!("zzz not in any catalog {x}", x = 1), "zzz not in any catalog 1");
        MIRROR.store(0, Ordering::Relaxed);
        assert!(!is_rtl());
    }
}
