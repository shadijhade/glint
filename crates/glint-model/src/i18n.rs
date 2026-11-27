//! The interface's language: every text is written in English, and the catalogs translate it.
//!
//! A text is looked up by its English (`t!("Delete {name}?")` in the window, [`t`] and [`translate`] here):
//! English is the key, and what a catalog lacks shows in English. The catalogs are gettext files in the
//! repository's `locales/` folder (`zh-CN.po`, `de.po`, `ar.po`). build.rs compiles each into a table sorted by
//! the English, so nothing is read or parsed at start, and a lookup is a binary search that borrows static
//! text. `node scripts/i18n.mjs` lists the texts for the translators (docs/translating.md).
//!
//! **Which language.** The first of Windows' display languages that Glint has a catalog for (English counts),
//! else English: there is no choice in Settings (0.13 took it out). `GLINT_LANGUAGE` (`en`, `zh-CN`, `de`, `ar`,
//! `qps-ploc`) overrides it, for translators and tests, and so does a catalog file
//! (`GLINT_LANGUAGE=D:\work\zh-CN.po`, [`use_catalog_file`]): a translator sees their latest work in Glint as
//! installed, without building it. The window sets the language once at start ([`set_current`]), before its
//! first frame. Arabic is written right to left ([`is_rtl`]), and the window mirrors itself for it.
//!
//! **Counts.** [`crate::format::plural`] reads a count's two texts, "{n} file" and "{n} files". A language with
//! more forms ([`count_form`]: Arabic's zero, two, few and many) gives each as an entry of the "other" text with
//! the form as its context ([`crate::po::COUNT_FORMS`]).
//!
//! **Text translated where it's shown.** A table's names (a column's label, a known folder's name) are looked
//! up when they're drawn, through a variable. [`english!`](crate::english) marks them where they're written,
//! so the extractor finds them; [`crate::format::plural`]'s nouns it finds by themselves.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};

/// The catalogs from build.rs: `ZH_CN`, `DE` and `AR` (locales/zh-CN.po, de.po, ar.po), and `REJECTED`, the
/// entries it left out.
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
    /// German, from locales/de.po.
    De = 3,
    /// Arabic, from locales/ar.po: written right to left, with six forms for a count.
    Ar = 4,
}

impl Language {
    /// The language's code, as `GLINT_LANGUAGE` and its catalog's file name have it: "en", "zh-CN", "de", "ar",
    /// "qps-ploc".
    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::ZhHans => "zh-CN",
            Language::Pseudo => "qps-ploc",
            Language::De => "de",
            Language::Ar => "ar",
        }
    }

    /// The language a code names: `GLINT_LANGUAGE`, or one of Windows' language names. English for "en" and
    /// "en-…", German for "de" and "de-…" (Germany, Austria, Switzerland…), Arabic for "ar" and "ar-…";
    /// Chinese (Simplified) for "zh", "zh-CN", "zh-SG", "zh-Hans-…" and any other Chinese but these: "zh-TW",
    /// "zh-HK", "zh-MO" and "zh-Hant-…" are Traditional, which has no catalog yet, and `None` lets them fall
    /// through to the next language (English, not Simplified). The pseudo-language for "qps-ploc".
    pub fn from_code(code: &str) -> Option<Language> {
        let code = code.trim().to_ascii_lowercase().replace('_', "-");
        let mut parts = code.split('-');
        match parts.next()? {
            "en" => Some(Language::En),
            "de" => Some(Language::De),
            "ar" => Some(Language::Ar),
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
    from_byte(CURRENT.load(Ordering::Relaxed))
}

/// The language `CURRENT` keeps as `language as u8`.
fn from_byte(byte: u8) -> Language {
    match byte {
        1 => Language::ZhHans,
        2 => Language::Pseudo,
        3 => Language::De,
        4 => Language::Ar,
        _ => Language::En,
    }
}

/// Sets the language the interface speaks: the window does, once at start, before its first frame.
pub fn set_current(language: Language) {
    CURRENT.store(language as u8, Ordering::Relaxed);
}

/// Written right to left: Arabic. The window mirrors itself for it.
pub fn is_rtl(language: Language) -> bool {
    match language {
        Language::Ar => true,
        Language::En | Language::ZhHans | Language::Pseudo | Language::De => false,
    }
}

/// The form a count of `n` takes in `language` where it has more than English's "one" and "other": Arabic's
/// "zero", "two", "few" (3 to 10, and 103 to 110…) and "many" (11 to 99, and 111 to 199…), by CLDR's rules.
/// `None` for one and other, the two texts every language has ("{n} file" and "{n} files").
pub fn count_form(language: Language, n: usize) -> Option<&'static str> {
    match language {
        Language::Ar => match (n, n % 100) {
            (0, _) => Some("zero"),
            (2, _) => Some("two"),
            (_, 3..=10) => Some("few"),
            (_, 11..=99) => Some("many"),
            _ => None,
        },
        Language::En | Language::ZhHans | Language::Pseudo | Language::De => None,
    }
}

/// A count in one of `language`'s other forms ([`count_form`]): the catalog's entry for `other` (the English
/// "{n} files") with `form` as its context, if it has one.
pub fn translate_form(language: Language, form: &str, other: &str) -> Option<&'static str> {
    let mut sep_buf = [0u8; 4];
    let sep = crate::po::FORM_SEPARATOR.encode_utf8(&mut sep_buf);
    let table = catalog(language);
    table
        .binary_search_by(|(english, _)| {
            english
                .bytes()
                .cmp(form.bytes().chain(sep.bytes()).chain(other.bytes()))
        })
        .ok()
        .map(|at| table[at].1)
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
        Language::De => catalogs::DE,
        Language::Ar => catalogs::AR,
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
    if vars.is_empty() || !out.contains('{') {
        return Cow::Borrowed(out);
    }
    // Fast single-pass scan when no replacement value introduces `{...}` placeholders (the common case):
    // one allocation instead of `2 * vars.len() + 1`.
    if !vars.iter().any(|(_, v)| v.contains('{')) {
        let extra: usize = vars.iter().map(|(_, v)| v.len()).sum();
        let mut filled = String::with_capacity(out.len() + extra);
        let bytes = out.as_bytes();
        let mut i = 0;
        let mut last = 0;
        while i < bytes.len() {
            if bytes[i] == b'{' {
                if let Some(rel_end) = out[i + 1..].find(['}', '{']) {
                    let end = i + 1 + rel_end;
                    if bytes[end] == b'}' {
                        let key = &out[i + 1..end];
                        if let Some((_, value)) = vars.iter().find(|(name, _)| *name == key) {
                            filled.push_str(&out[last..i]);
                            filled.push_str(value);
                            i = end + 1;
                            last = i;
                            continue;
                        }
                    } else {
                        i = end;
                        continue;
                    }
                }
            }
            i += 1;
        }
        filled.push_str(&out[last..]);
        return Cow::Owned(filled);
    }
    // Rare chained case: a replacement value itself contains `{other}` to be filled by a later pair.
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
        assert_eq!(language_of(&["de-DE", "zh-Hans-CN"]), Language::De);
        assert_eq!(language_of(&["fr-FR", "de-CH", "en-US"]), Language::De);
        assert_eq!(language_of(&["ar-SA", "en-US"]), Language::Ar);
        assert_eq!(language_of(&["ar-EG"]), Language::Ar);
        assert_eq!(language_of(&["zh-SG"]), Language::ZhHans);
        // Traditional Chinese has no catalog: English, not Simplified, unless Simplified comes next.
        assert_eq!(language_of(&["zh-TW"]), Language::En);
        assert_eq!(language_of(&["zh-Hant-HK", "zh-Hans-CN"]), Language::ZhHans);
        assert_eq!(language_of(&["fr-FR", "es-ES"]), Language::En);
        assert_eq!(language_of(&[]), Language::En);
        assert_eq!(
            language_of(&["qps-ploc"]),
            Language::En,
            "Windows never picks the pseudo-language"
        );
    }

    #[test]
    fn codes_name_languages() {
        for language in [
            Language::En,
            Language::ZhHans,
            Language::Pseudo,
            Language::De,
            Language::Ar,
        ] {
            assert_eq!(Language::from_code(language.code()), Some(language));
            // The language in use is kept as one byte: each must come back as itself.
            assert_eq!(from_byte(language as u8), language);
        }
        assert_eq!(Language::from_code("ZH_cn"), Some(Language::ZhHans));
        assert_eq!(Language::from_code(" zh "), Some(Language::ZhHans));
        assert_eq!(Language::from_code("en-GB"), Some(Language::En));
        assert_eq!(Language::from_code("de-AT"), Some(Language::De));
        assert_eq!(Language::from_code("ar-MA"), Some(Language::Ar));
        assert_eq!(Language::from_code("zh-MO"), None);
        assert_eq!(Language::from_code("qps"), None);
        assert_eq!(Language::from_code(""), None);
        assert!(!is_rtl(Language::ZhHans));
        assert!(!is_rtl(Language::De));
        assert!(is_rtl(Language::Ar));
    }

    #[test]
    fn arabic_counts_take_six_forms() {
        let form = |n| count_form(Language::Ar, n);
        let forms: Vec<Option<&str>> = [0, 1, 2, 3, 10, 11, 99, 100, 101, 102, 103, 111, 200, 1000]
            .into_iter()
            .map(form)
            .collect();
        assert_eq!(
            forms,
            [
                Some("zero"),
                None, // one: "{n} file"
                Some("two"),
                Some("few"),
                Some("few"),
                Some("many"),
                Some("many"),
                None, // other: "{n} files"
                None,
                None,
                Some("few"),
                Some("many"),
                None,
                None,
            ]
        );
        assert_eq!(count_form(Language::De, 2), None, "German has only one and other");
        assert_eq!(count_form(Language::ZhHans, 0), None);
        assert_eq!(translate_form(Language::En, "two", "{n} files"), None);
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
        for catalog in [catalogs::ZH_CN, catalogs::DE, catalogs::AR] {
            for pair in catalog.windows(2) {
                assert!(pair[0].0 < pair[1].0, "{:?} comes before {:?}", pair[0].0, pair[1].0);
            }
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
