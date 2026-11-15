//! Ported from src/lib/i18n.ts.
//!
//! The interface's language. Glint speaks English, the key every text is written in; Windows' language is
//! what it would follow if it spoke more than one (0.13 took out the Arabic catalog and the choice in Settings:
//! the interface is always Windows' language where Glint has it, and English otherwise, which today means
//! English). The seam stays so the call sites keep asking for a language: [`translate`] and [`t`] take one.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// The languages Glint's interface speaks. English is its own text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English: every text is written in it.
    En,
}

impl Language {
    /// The language's code: `"en"`.
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
        }
    }
}

/// The language for Windows' UI languages (in order of preference): the first Glint speaks, else English.
pub fn language_of(_os_languages: &[&str]) -> Language {
    Language::En
}

/// Written right to left: none of the languages Glint speaks is.
pub fn is_rtl(language: Language) -> bool {
    match language {
        Language::En => false,
    }
}

/// What `t` would give in `language` before any `{name}` parts are filled in: the text itself, as English is
/// the key.
pub fn translate(language: Language, text: &str) -> &str {
    match language {
        Language::En => text,
    }
}

/// `text` in `language`, with its `{name}` parts filled from `vars`. Each `(name, value)` replaces every
/// `{name}` in order, as the TS did, so a value that itself contains `{other}` can be filled by a later pair.
/// Numbers are passed as their text (the TS used `String(value)`).
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

    #[test]
    fn t_fills_placeholders() {
        assert_eq!(t(Language::En, "Search {folder}", &[("folder", "Docs")]), "Search Docs");
        assert_eq!(t(Language::En, "Open", &[]), "Open");
        // A value that holds another placeholder is filled by a later pair, as the TS did.
        assert_eq!(t(Language::En, "{a} and {b}", &[("a", "{b}"), ("b", "x")]), "x and x");
    }

    #[test]
    fn the_interface_is_english_whatever_windows_speaks() {
        assert_eq!(language_of(&["ar-SA", "en-US"]), Language::En);
        assert_eq!(language_of(&[]), Language::En);
        assert!(!is_rtl(Language::En));
        assert_eq!(Language::En.code(), "en");
        assert_eq!(translate(Language::En, "constructor"), "constructor");
    }
}
