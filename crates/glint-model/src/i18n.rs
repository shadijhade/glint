//! The interface's language: every text is written in English, and the catalogs translate it.
//!
//! A text is looked up by its English (`t!("Delete {name}?")` in the window, [`t`] and [`translate`] here):
//! English is the key, and what a catalog lacks shows in English. The catalogs are gettext files in the
//! repository's `locales/` folder (`zh-CN.po`). build.rs compiles each into a table sorted by the English, so
//! nothing is read or parsed at start, and a lookup is a binary search that borrows static text.
//! `node scripts/i18n.mjs` lists the texts for the translators (docs/translating.md).
//!
//! **Which language.** Windows' display language where Glint has a catalog for it, else English: there is no
//! choice in Settings (0.13 took it out). `GLINT_LANGUAGE` (`en`, `zh-CN`, `qps-ploc`) overrides it, for
//! translators and tests, and so does a catalog file (`GLINT_LANGUAGE=D:\work\zh-CN.po`, [`use_catalog_file`]):
//! a translator sees their latest work in Glint as installed, without building it. The window sets the
//! language once at start ([`set_current`]), before its first frame.
//!
//! **Text translated where it's shown.** A table's names (a column's label, a known folder's name) are looked
//! up when they're drawn, through a variable. [`english!`](crate::english) marks them where they're written,
//! so the extractor finds them; [`crate::format::plural`]'s nouns it finds by themselves.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};

/// The catalogs from build.rs: `ZH_CN` (locales/zh-CN.po), and `REJECTED`, the entries it left out.
mod catalogs {
    include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));
}

/// The languages Glint's interface speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    /// English: every text is written in it.
    En = 0,
    /// Chinese (Simplified), from locales/zh-CN.po.
    ZhHans = 1,
    /// Every text accented, a third longer and in brackets ("[Ðéĺéţé ··]"). Never chosen from Windows, only by
    /// `GLINT_LANGUAGE=qps-ploc`: text that skips the catalog stays plain, and a layout that cuts text off loses
    /// the closing bracket.
    Pseudo = 2,
}

impl Language {
    /// The language's code, as `GLINT_LANGUAGE` and its catalog's file name have it: "en", "zh-CN", "qps-ploc".
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::ZhHans => "zh-CN",
            Language::Pseudo => "qps-ploc",
        }
    }

    /// The language a code names: `GLINT_LANGUAGE`, or one of Windows' language names. English for "en" and
    /// "en-…"; Chinese (Simplified) for "zh", "zh-CN", "zh-SG", "zh-Hans-…" and any other Chinese but these:
    /// "zh-TW", "zh-HK", "zh-MO" and "zh-Hant-…" are Traditional, which has no catalog yet, and `None` lets them
    /// fall through to the next language (English, not Simplified). The pseudo-language for "qps-ploc".
    pub fn from_code(code: &str) -> Option<Language> {
        let code = code.trim().to_ascii_lowercase().replace('_', "-");
        let mut parts = code.split('-');
        match parts.next()? {
            "en" => Some(Language::En),
            "qps" if code == "qps-ploc" => Some(Language::Pseudo),
            "zh" => {
                let rest: Vec<&str> = parts.collect();
                if rest.contains(&"hans") {
                    Some(Language::ZhHans)
                } else if rest.iter().any(|part| matches!(*part, "hant" | "tw" | "hk" | "mo")) {
                    None
                } else {
                    Some(Language::ZhHans)
                }
            }
            _ => None,
        }
    }
}

/// The language for Windows' display languages, in their order of preference: the first Glint has a catalog for
/// (English counts), else English. Never the pseudo-language.
pub fn language_of(os_languages: &[&str]) -> Language {
    os_languages
        .iter()
        .filter_map(|name| Language::from_code(name))
        .find(|language| *language != Language::Pseudo)
        .unwrap_or(Language::En)
}

/// The language in use: English until the window sets it ([`set_current`]).
static CURRENT: AtomicU8 = AtomicU8::new(Language::En as u8);

/// The language the interface speaks now. Cheap: one atomic load, so `t!` asks it every time.
pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::ZhHans,
        2 => Language::Pseudo,
        _ => Language::En,
    }
}

/// Sets the language the interface speaks: the window does, once at start, before its first frame.
pub fn set_current(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

/// Written right to left: none of the languages Glint speaks is.
pub fn is_rtl(language: Language) -> bool {
    match language {
        Language::En | Language::ZhHans | Language::Pseudo => false,
    }
}

/// `text` in `language` before any `{name}` parts are filled in: the catalog's translation, else the English.
pub fn translate(language: Language, text: &str) -> &str {
    match language {
        Language::En => text,
        Language::Pseudo => pseudo(text),
        language => lookup(catalog(language), text).unwrap_or(text),
    }
}

/// The catalog `language` reads: a file given at start, else the one built in.
fn catalog(language: Language) -> &'static [(&'static str, &'static str)] {
    if let Some((from_file, table)) = FROM_FILE.get() {
        if *from_file == language {
            return table;
        }
    }
    match language {
        Language::En | Language::Pseudo => &[],
        Language::ZhHans => catalogs::ZH_CN,
    }
}

/// A catalog read from a file at start ([`use_catalog_file`]), which its language reads instead of the built-in.
static FROM_FILE: OnceLock<(Language, &'static [(&'static str, &'static str)])> = OnceLock::new();

/// What [`use_catalog_file`] read.
#[derive(Debug)]
pub struct CatalogFile {
    /// The language, from the file's name ("zh-CN.po").
    pub language: Language,
    /// How many translations it has that Glint uses.
    pub translated: usize,
    /// The ones it left out, and why: "line 120: \"Delete {name}?\": the translation lacks {name}".
    pub left_out: Vec<String>,
}

/// Reads a catalog file (`GLINT_LANGUAGE=D:\work\zh-CN.po`) for its language, named by the file ("zh-CN.po"),
/// to read instead of the one built in: a translator's latest work in Glint as installed. Its entries are kept
/// as build.rs keeps them ([`crate::po::usable`]). Once per run, at start, before [`set_current`].
pub fn use_catalog_file(path: &Path) -> Result<CatalogFile, String> {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let language = Language::from_code(&stem)
        .filter(|language| !matches!(language, Language::En | Language::Pseudo))
        .ok_or_else(|| format!("{}: the file's name says no language Glint speaks", path.display()))?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let catalog = crate::po::parse(&text).map_err(|e| format!("{}, {e}", path.display()))?;
    let (kept, rejected) = crate::po::usable(catalog);
    let table: Vec<(&'static str, &'static str)> = kept
        .into_iter()
        .map(|(english, translation)| -> (&'static str, &'static str) {
            (
                Box::leak(english.into_boxed_str()),
                Box::leak(translation.into_boxed_str()),
            )
        })
        .collect();
    let translated = table.len();
    FROM_FILE
        .set((language, Box::leak(table.into_boxed_slice())))
        .map_err(|_| "a catalog file was already read".to_string())?;
    let left_out = rejected
        .into_iter()
        .map(|r| format!("line {}: {:?}: {}", r.line, r.english, r.why))
        .collect();
    Ok(CatalogFile {
        language,
        translated,
        left_out,
    })
}

/// The translation a catalog has for `text`, if it has one.
fn lookup(catalog: &'static [(&'static str, &'static str)], text: &str) -> Option<&'static str> {
    catalog
        .binary_search_by(|(english, _)| (*english).cmp(text))
        .ok()
        .map(|at| catalog[at].1)
}

/// How many texts `language`'s catalog translates (English: none, it is the text).
pub fn translated_count(language: Language) -> usize {
    catalog(language).len()
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

/// The pseudo-language's `text`, made once and kept: the window asks for the same texts every frame.
fn pseudo(text: &str) -> &'static str {
    static MADE: LazyLock<Mutex<HashMap<String, &'static str>>> = LazyLock::new(Default::default);
    let mut made = MADE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&done) = made.get(text) {
        return done;
    }
    // Leaked on purpose: a few thousand texts at most, in a mode only translators and tests use.
    let done: &'static str = Box::leak(stretch(text).into_boxed_str());
    made.insert(text.to_owned(), done);
    done
}

/// Letters accented, `{name}` placeholders kept as they are, a third longer, in brackets: "[Ðéĺéţé {name}? ··]".
/// Spaces around the text stay outside the brackets; an empty text stays empty.
fn stretch(text: &str) -> String {
    let core = text.trim();
    if core.is_empty() {
        return text.to_owned();
    }
    let lead = &text[..text.len() - text.trim_start().len()];
    let tail = &text[text.trim_end().len()..];
    let mut out = String::with_capacity(text.len() * 2 + 8);
    out.push_str(lead);
    out.push('[');
    let (mut letters, mut in_name) = (0usize, false);
    for c in core.chars() {
        match c {
            '{' => in_name = true,
            '}' => in_name = false,
            _ => {}
        }
        if in_name || c == '}' {
            out.push(c);
            continue;
        }
        if c.is_ascii_alphabetic() {
            letters += 1;
        }
        out.push(accented(c));
    }
    out.push(' ');
    out.extend(std::iter::repeat_n('·', letters.div_ceil(3).max(1)));
    out.push(']');
    out.push_str(tail);
    out
}

/// A letter with an accent Segoe UI draws, or the character itself.
fn accented(c: char) -> char {
    const FROM: &str = "aceinorsuyzACDEGINORSTUYZdgklt";
    const TO: &str = "áçéíñóŕšúýžÁÇĎÉĞÍÑÓŔŠŤÚÝŽďğķĺť";
    FROM.chars()
        .position(|f| f == c)
        .and_then(|at| TO.chars().nth(at))
        .unwrap_or(c)
}

/// Interface text written in a table and translated where it's shown: `label: english!("Size")` in a column's
/// definition, which the header draws with `i18n::translate(def.label)`. It is the text itself; the mark is for
/// `node scripts/i18n.mjs`, which lists it for the translators as it lists `t!("…")`.
#[macro_export]
macro_rules! english {
    ($text:literal) => {
        $text
    };
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
    fn windows_languages_pick_the_first_glint_has() {
        assert_eq!(language_of(&["zh-CN", "en-US"]), Language::ZhHans);
        assert_eq!(language_of(&["en-US", "zh-CN"]), Language::En);
        assert_eq!(language_of(&["de-DE", "zh-Hans-CN"]), Language::ZhHans);
        assert_eq!(language_of(&["zh-SG"]), Language::ZhHans);
        // Traditional Chinese has no catalog: English, not Simplified, unless Simplified comes next.
        assert_eq!(language_of(&["zh-TW"]), Language::En);
        assert_eq!(language_of(&["zh-Hant-HK", "zh-Hans-CN"]), Language::ZhHans);
        assert_eq!(language_of(&["ar-SA", "fr-FR"]), Language::En);
        assert_eq!(language_of(&[]), Language::En);
        assert_eq!(
            language_of(&["qps-ploc"]),
            Language::En,
            "Windows never picks the pseudo-language"
        );
    }

    #[test]
    fn codes_name_languages() {
        for language in [Language::En, Language::ZhHans, Language::Pseudo] {
            assert_eq!(Language::from_code(language.code()), Some(language));
        }
        assert_eq!(Language::from_code("ZH_cn"), Some(Language::ZhHans));
        assert_eq!(Language::from_code(" zh "), Some(Language::ZhHans));
        assert_eq!(Language::from_code("en-GB"), Some(Language::En));
        assert_eq!(Language::from_code("zh-MO"), None);
        assert_eq!(Language::from_code("qps"), None);
        assert_eq!(Language::from_code(""), None);
        assert!(!is_rtl(Language::ZhHans));
    }

    #[test]
    fn english_is_the_key_and_what_a_catalog_lacks_stays_english() {
        assert_eq!(translate(Language::En, "constructor"), "constructor");
        assert_eq!(
            translate(Language::ZhHans, "A text no catalog has"),
            "A text no catalog has"
        );
        if let Some((english, chinese)) = catalogs::ZH_CN.first() {
            assert_eq!(translate(Language::ZhHans, english), *chinese);
        }
        assert_eq!(translated_count(Language::ZhHans), catalogs::ZH_CN.len());
    }

    #[test]
    fn the_catalogs_are_sorted_with_each_english_once() {
        // `lookup` is a binary search: it needs this.
        for pair in catalogs::ZH_CN.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{:?} comes before {:?}", pair[0].0, pair[1].0);
        }
    }

    #[test]
    fn every_translation_in_locales_is_used() {
        let list: Vec<String> = catalogs::REJECTED
            .iter()
            .map(|(file, line, english, why)| format!("locales/{file}:{line}: {english:?}: {why}"))
            .collect();
        assert!(list.is_empty(), "these translations are left out:\n{}", list.join("\n"));
    }

    #[test]
    fn the_pseudo_language_stretches_text_and_keeps_placeholders() {
        assert_eq!(stretch("Delete {name}?"), "[Ďéĺéťé {name}? ··]");
        assert_eq!(stretch("  Open "), "  [Ópéñ ··] ");
        assert_eq!(stretch(""), "");
        assert_eq!(stretch(" "), " ");
        assert_eq!(stretch("…"), "[… ·]");
        let filled = t(Language::Pseudo, "Delete {name}?", &[("name", "a.txt")]);
        assert_eq!(filled, "[Ďéĺéťé a.txt? ··]");
        // Made once: the same text gives the same leaked string.
        assert!(std::ptr::eq(
            translate(Language::Pseudo, "Open"),
            translate(Language::Pseudo, "Open")
        ));
    }

    #[test]
    fn the_language_in_use_starts_english() {
        // Other tests in this crate read it, so this one only reads.
        assert_eq!(current(), Language::En);
    }
}
