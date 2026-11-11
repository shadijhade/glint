//! Ported from src/lib/i18n.ts (the Arabic catalog is in i18n_ar.rs, ported from src/lib/i18n-ar.ts).
//!
//! The interface's languages: English as the key, the Arabic catalog, right to left.
//!
//! The TS kept the current language in a module variable and wrote `lang` and `dir` on the page. That
//! is UI state, so it isn't here: the UI layer owns the current `Language` and asks these pure functions
//! with it. `setLanguage`, `language()` and the browser-only `isRtl()` are replaced by `is_rtl(language)`.

use std::borrow::Cow;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::i18n_ar::AR;

/// The languages Glint's interface speaks. English is its own text; the others are catalogs of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English: the keys of the catalogs are English, so this one has no catalog.
    En,
    /// Arabic, written right to left.
    Ar,
}

impl Language {
    /// The code the page used for `lang` and the settings file stores: `"en"` or `"ar"`.
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Ar => "ar",
        }
    }
}

/// What the language setting can hold: Windows' own language where Glint speaks it, or one chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LanguageSetting {
    /// Follow Windows' language.
    System,
    /// English, chosen.
    En,
    /// Arabic, chosen.
    Ar,
}

/// The settings' choices with the names the dialog shows them under. The labels are each language's own
/// name, so they are not translated.
pub const LANGUAGES: &[(LanguageSetting, &str)] = &[
    (LanguageSetting::System, "Windows' language"),
    (LanguageSetting::En, "English"),
    (LanguageSetting::Ar, "العربية"),
];

/// The language for a setting: the one chosen, else Windows' own where Glint speaks it, else English.
///
/// `setting` is the stored setting as its string, or `None` when it is missing; anything other than
/// `"en"` and `"ar"` counts as "follow Windows". `os_languages` is Windows' UI languages in order of
/// preference: the browser's `navigator.languages`, or the one language when that list is empty.
pub fn language_of(setting: Option<&str>, os_languages: &[&str]) -> Language {
    match setting {
        Some("en") => return Language::En,
        Some("ar") => return Language::Ar,
        _ => {}
    }
    if os_languages.iter().any(|l| l.to_lowercase().starts_with("ar")) {
        Language::Ar
    } else {
        Language::En
    }
}

/// Written right to left. Only Arabic is.
pub fn is_rtl(language: Language) -> bool {
    matches!(language, Language::Ar)
}

/// The catalog of `language`, or `None` for English, whose text is the key itself.
fn catalog(language: Language) -> Option<&'static HashMap<&'static str, &'static str>> {
    match language {
        Language::En => None,
        Language::Ar => Some(&*AR),
    }
}

/// What `t` would give in `language` before any `{name}` parts are filled in, for checks.
///
/// A text the catalog doesn't have comes back as it was given, in English. The result borrows from
/// `text` or from the static catalog.
pub fn translate(language: Language, text: &str) -> &str {
    // NOTE(port): the TS read `CATALOGS[lang][text]` from a plain object, so keys such as "constructor"
    // or "toString" found Object.prototype's members. A map lookup has no such keys, so the text
    // comes back as it was given, which is what the TS meant.
    if let Some(value) = catalog(language).and_then(|c| c.get(text)) {
        return value;
    }
    text
}

/// `text` in `language`: English is the key, and what a catalog doesn't have stays English. `{name}`
/// parts are filled from `vars` after the lookup, so a catalog keeps them. Each `(name, value)` replaces
/// every `{name}` in order, as the TS did, so a value that itself contains `{other}` can be filled by a
/// later pair. Numbers are passed as their text (the TS used `String(value)`).
pub fn t<'a>(language: Language, text: &'a str, vars: &[(&str, &str)]) -> Cow<'a, str> {
    let out = translate(language, text);
    if vars.is_empty() {
        return Cow::Borrowed(out);
    }
    let mut filled = out.to_string();
    for (name, value) in vars {
        filled = filled.replace(&format!("{{{name}}}"), value);
    }
    Cow::Owned(filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n_ar::AR_ENTRIES;

    /// The `{name}` parts of `text`, in order, as the TS regex `\{\w+\}` found them.
    fn placeholders(text: &str) -> Vec<&str> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let name_len = after
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if name_len > 0 && after.as_bytes().get(name_len) == Some(&b'}') {
                found.push(&rest[open..open + name_len + 2]);
                rest = &rest[open + name_len + 2..];
            } else {
                rest = &rest[open + 1..];
            }
        }
        found
    }

    // Arabic: what's translated keeps the parts filled in ({folder}...), and what isn't stays English.
    #[test]
    fn arabic_catalog_keeps_every_placeholder_of_its_english_text() {
        for (english, arabic) in AR_ENTRIES {
            for hole in placeholders(english) {
                assert!(arabic.contains(hole), "{english} -> {arabic}");
            }
        }
    }

    #[test]
    fn arabic_translates_known_text_and_keeps_unknown_text_in_english() {
        assert_eq!(translate(Language::Ar, "Open"), "فتح");
        assert_eq!(
            translate(Language::Ar, "Search {folder}").replace("{folder}", "Docs"),
            "البحث في Docs"
        );
        assert_eq!(translate(Language::Ar, "Not in the catalog"), "Not in the catalog");
        assert_eq!(translate(Language::En, "Open"), "Open");
    }

    #[test]
    fn the_arabic_catalog_has_all_its_entries_in_order() {
        // The 290 that came with the port, then what the native app added (the head of a pane and its kinds).
        assert_eq!(AR_ENTRIES.len(), 309);
        assert_eq!(AR.len(), 309, "no English text twice");
        assert_eq!(AR_ENTRIES[0], ("This PC", "هذا الكمبيوتر"));
        assert_eq!(AR_ENTRIES[289], ("Read as", "قراءة بترميز"));
        assert_eq!(AR_ENTRIES[308], ("Other files", "ملفات أخرى"));
    }

    #[test]
    fn t_fills_placeholders_after_the_lookup() {
        assert_eq!(
            t(Language::Ar, "Search {folder}", &[("folder", "Docs")]),
            "البحث في Docs"
        );
        assert_eq!(t(Language::En, "Search {folder}", &[("folder", "Docs")]), "Search Docs");
        assert_eq!(t(Language::Ar, "Open", &[]), "فتح");
        assert_eq!(t(Language::En, "Not in the catalog", &[]), "Not in the catalog");
        // A value that holds another placeholder is filled by a later pair, as the TS did.
        assert_eq!(t(Language::En, "{a} and {b}", &[("a", "{b}"), ("b", "x")]), "x and x");
    }

    #[test]
    fn language_of_takes_the_setting_first_then_windows() {
        assert_eq!(language_of(Some("en"), &["ar-EG"]), Language::En);
        assert_eq!(language_of(Some("ar"), &["en-US"]), Language::Ar);
        assert_eq!(language_of(Some("system"), &["AR-sa", "en-US"]), Language::Ar);
        assert_eq!(language_of(None, &["fr-FR", "en-US"]), Language::En);
        assert_eq!(language_of(Some("system"), &[]), Language::En);
        assert_eq!(language_of(Some("bogus"), &["ar"]), Language::Ar);
    }

    #[test]
    fn only_arabic_is_right_to_left() {
        assert!(is_rtl(Language::Ar));
        assert!(!is_rtl(Language::En));
        assert_eq!(Language::Ar.code(), "ar");
        assert_eq!(Language::En.code(), "en");
    }

    #[test]
    fn object_like_english_keys_come_back_as_given() {
        assert_eq!(translate(Language::Ar, "constructor"), "constructor");
        assert_eq!(translate(Language::Ar, "__proto__"), "__proto__");
    }
}
