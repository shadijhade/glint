//! A gettext catalog (`.po`) read into English and its translation: what build.rs compiles into Glint from
//! `locales/`, and what `GLINT_LANGUAGE=D:\zh-CN.po` reads at start (a translator's latest work, without
//! building Glint). Std only, so build.rs can take this file as it is (`#[path = "src/po.rs"]`).
//!
//! Only what Glint's catalogs use is understood: `msgid`, `msgstr`, strings that go on over several lines, the
//! `fuzzy` flag, comments and obsolete (`#~`) entries. `msgctxt` and plural forms are read so they can be
//! reported, not used: `t!` has neither (a count's two forms are two texts, "{n} file" and "{n} files").

/// One entry: an English text (`msgid`) and what it is in the catalog's language (`msgstr`, empty while no one
/// has translated it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The English, which is the key.
    pub english: String,
    /// The translation; empty until someone translates it.
    pub translation: String,
    /// Marked `#, fuzzy`: a guess (an older English's translation) that a person hasn't checked. Not used.
    pub fuzzy: bool,
    /// The entry has a `msgctxt`.
    pub context: bool,
    /// The entry has plural forms (`msgid_plural`).
    pub plural: bool,
    /// The line its `msgid` is on, for messages.
    pub line: usize,
}

/// A whole catalog: its entries, in the file's order, without the header (the entry whose English is empty).
#[derive(Debug, Default)]
pub struct Catalog {
    /// The entries.
    pub entries: Vec<Entry>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Context,
    Id,
    Plural,
    Str,
    /// `msgstr[1]` and on: read and dropped.
    OtherForm,
}

#[derive(Default)]
struct Partial {
    english: Option<String>,
    translation: Option<String>,
    fuzzy: bool,
    context: bool,
    plural: bool,
    line: usize,
}

/// Reads a catalog. An error says where: "line 12: …".
pub fn parse(text: &str) -> Result<Catalog, String> {
    let mut catalog = Catalog::default();
    let mut entry = Partial::default();
    let mut field: Option<Field> = None;
    let mut context = String::new();
    let mut plural = String::new();
    let mut other = String::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        // Anything but a string that goes on ends an entry whose msgstr has been read.
        let starts_entry = line.is_empty()
            || line.starts_with('#')
            || line.starts_with("msgctxt")
            || (line.starts_with("msgid") && !line.starts_with("msgid_plural"));
        if starts_entry && entry.translation.is_some() {
            finish(&mut catalog, std::mem::take(&mut entry))?;
            field = None;
        }
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#,") {
            if rest.split(',').any(|flag| flag.trim() == "fuzzy") {
                entry.fuzzy = true;
            }
            continue;
        }
        if line.starts_with('#') {
            // Comments, references, earlier English (`#|`), and obsolete entries (`#~`), which are left out.
            continue;
        }
        let at = |message: &str| format!("line {number}: {message}");
        if let Some(rest) = line.strip_prefix("msgctxt") {
            entry.context = true;
            context = unquote(rest.trim()).map_err(|e| at(&e))?;
            field = Some(Field::Context);
        } else if let Some(rest) = line.strip_prefix("msgid_plural") {
            if entry.english.is_none() {
                return Err(at("msgid_plural before msgid"));
            }
            entry.plural = true;
            plural = unquote(rest.trim()).map_err(|e| at(&e))?;
            field = Some(Field::Plural);
        } else if let Some(rest) = line.strip_prefix("msgid") {
            if entry.english.is_some() {
                return Err(at("a second msgid before this entry's msgstr"));
            }
            entry.english = Some(unquote(rest.trim()).map_err(|e| at(&e))?);
            entry.line = number;
            field = Some(Field::Id);
        } else if let Some(rest) = line.strip_prefix("msgstr[") {
            let Some((form, value)) = rest.split_once(']') else {
                return Err(at("msgstr[ without its ]"));
            };
            if entry.english.is_none() {
                return Err(at("msgstr before msgid"));
            }
            let value = unquote(value.trim()).map_err(|e| at(&e))?;
            if form.trim() == "0" {
                entry.translation = Some(value);
                field = Some(Field::Str);
            } else {
                other = value;
                field = Some(Field::OtherForm);
            }
        } else if let Some(rest) = line.strip_prefix("msgstr") {
            if entry.english.is_none() {
                return Err(at("msgstr before msgid"));
            }
            entry.translation = Some(unquote(rest.trim()).map_err(|e| at(&e))?);
            field = Some(Field::Str);
        } else if line.starts_with('"') {
            let more = unquote(line).map_err(|e| at(&e))?;
            let target = match field {
                Some(Field::Context) => &mut context,
                Some(Field::Id) => entry.english.get_or_insert_with(String::new),
                Some(Field::Plural) => &mut plural,
                Some(Field::Str) => entry.translation.get_or_insert_with(String::new),
                Some(Field::OtherForm) => &mut other,
                None => return Err(at("a string that goes on from nothing")),
            };
            target.push_str(&more);
        } else {
            return Err(at(&format!("can't read {line:?}")));
        }
    }
    if entry.english.is_some() {
        if entry.translation.is_none() {
            return Err(format!(
                "line {}: msgid without a msgstr at the end of the file",
                entry.line
            ));
        }
        finish(&mut catalog, entry)?;
    }
    Ok(catalog)
}

fn finish(catalog: &mut Catalog, entry: Partial) -> Result<(), String> {
    let Some(english) = entry.english else {
        return Ok(());
    };
    let Some(translation) = entry.translation else {
        return Err(format!("line {}: msgid without a msgstr", entry.line));
    };
    // The header: the catalog's language and the tools that wrote it, which Glint doesn't need.
    if english.is_empty() && !entry.context {
        return Ok(());
    }
    catalog.entries.push(Entry {
        english,
        translation,
        fuzzy: entry.fuzzy,
        context: entry.context,
        plural: entry.plural,
        line: entry.line,
    });
    Ok(())
}

/// A quoted PO string's text: `"a \"b\"\n"` is `a "b"` and a line break.
fn unquote(quoted: &str) -> Result<String, String> {
    let inner = quoted
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| format!("expected a string in quotes, found {quoted:?}"))?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => return Err(format!("\\{other} isn't an escape Glint's catalogs use")),
                None => return Err("the string ends in a lone \\".to_string()),
            },
            '"' => return Err(format!("a \" inside {quoted:?} needs a \\ before it")),
            c => out.push(c),
        }
    }
    Ok(out)
}

/// An entry a catalog can't use, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// The line its `msgid` is on.
    pub line: usize,
    /// Its English.
    pub english: String,
    /// Why it's left out.
    pub why: String,
}

/// The translations a catalog can use, sorted by their English with each English once, and the entries it
/// can't, with why: placeholders that differ from the English's, a context or plural forms (`t!` has neither),
/// the same English twice. An entry no one has translated, or marked `fuzzy` (a guess no one has checked), is
/// neither: its English shows, which is no mistake.
pub fn usable(catalog: Catalog) -> (Vec<(String, String)>, Vec<Rejected>) {
    let mut rejected = Vec::new();
    let mut pairs: Vec<(String, String, usize)> = Vec::new();
    for entry in catalog.entries {
        let why = if entry.context {
            Some("it has a msgctxt, and Glint's texts have no context".to_string())
        } else if entry.plural {
            Some("it has plural forms: Glint's counts are two texts, \"{n} file\" and \"{n} files\"".to_string())
        } else if entry.fuzzy || entry.translation.is_empty() {
            continue;
        } else {
            placeholders_differ(&entry.english, &entry.translation)
        };
        match why {
            Some(why) => rejected.push(Rejected {
                line: entry.line,
                english: entry.english,
                why,
            }),
            None => pairs.push((entry.english, entry.translation, entry.line)),
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    let mut kept: Vec<(String, String)> = Vec::with_capacity(pairs.len());
    for (english, translation, line) in pairs {
        if kept.last().is_some_and(|(last, _)| *last == english) {
            rejected.push(Rejected {
                line,
                english,
                why: "the same English is in the file twice".to_string(),
            });
        } else {
            kept.push((english, translation));
        }
    }
    rejected.sort_by_key(|r| r.line);
    (kept, rejected)
}

/// Why a translation can't stand in for its English, if it can't: it must keep every `{name}` and `%1` the
/// English has and add none, or the code that fills them would leave a hole or show the braces.
pub fn placeholders_differ(english: &str, translation: &str) -> Option<String> {
    let (want, have) = (placeholders(english), placeholders(translation));
    let missing: Vec<&str> = want.iter().filter(|p| !have.contains(p)).map(String::as_str).collect();
    let extra: Vec<&str> = have.iter().filter(|p| !want.contains(p)).map(String::as_str).collect();
    match (missing.is_empty(), extra.is_empty()) {
        (true, true) => None,
        (false, true) => Some(format!("the translation lacks {}", missing.join(" "))),
        (true, false) => Some(format!(
            "the translation has {}, which the English doesn't",
            extra.join(" ")
        )),
        (false, false) => Some(format!(
            "the translation lacks {} and has {}, which the English doesn't",
            missing.join(" "),
            extra.join(" ")
        )),
    }
}

/// The `{name}`, `%1` and `%` placeholders in a text, each once, in order.
pub fn placeholders(text: &str) -> Vec<String> {
    let name_char = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut found: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let end = bytes[i + 1..].iter().position(|&b| !name_char(b)).map(|n| i + 1 + n);
            if let Some(end) = end {
                if end > i + 1 && bytes[end] == b'}' {
                    let placeholder = &text[i..=end];
                    if !found.iter().any(|f| f == placeholder) {
                        found.push(placeholder.to_string());
                    }
                    i = end + 1;
                    continue;
                }
            }
        } else if bytes[i] == b'%' {
            // "%1" (filled in by Windows), or a lone "%" (a file's extension, "% image": the Type column).
            let digit = i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit();
            let placeholder = if digit { &text[i..i + 2] } else { "%" };
            if !found.iter().any(|f| f == placeholder) {
                found.push(placeholder.to_string());
            }
            i += if digit { 2 } else { 1 };
            continue;
        }
        i += 1;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# Glint's interface in Chinese (Simplified).
msgid ""
msgstr ""
"Language: zh_CN\n"
"Content-Type: text/plain; charset=UTF-8\n"

#: crates/glint-ui/src/components/sidebar/mod.rs:10
msgid "This PC"
msgstr "此电脑"

#. {name} is a file's name.
msgid "Delete {name}?"
msgstr ""
"删除 "
"{name}？"

#, fuzzy
#| msgid "Open in new tab"
msgid "Open in a new tab"
msgstr "在新标签页中打开"

msgid "Line one\nand \"two\""
msgstr "第一行\n和“二”"

msgid "Not yet"
msgstr ""

msgctxt "verb"
msgid "Open"
msgstr "打开"

msgid "{n} file"
msgid_plural "{n} files"
msgstr[0] "{n} 个文件"

#~ msgid "Gone"
#~ msgstr "没了"
"#;

    #[test]
    fn reads_entries_strings_that_go_on_flags_and_escapes() {
        let catalog = parse(SAMPLE).unwrap();
        let english: Vec<&str> = catalog.entries.iter().map(|e| e.english.as_str()).collect();
        assert_eq!(
            english,
            [
                "This PC",
                "Delete {name}?",
                "Open in a new tab",
                "Line one\nand \"two\"",
                "Not yet",
                "Open",
                "{n} file"
            ]
        );
        let entry = |text: &str| catalog.entries.iter().find(|e| e.english == text).unwrap();
        assert_eq!(entry("This PC").translation, "此电脑");
        assert_eq!(entry("This PC").line, 8);
        assert_eq!(entry("Delete {name}?").translation, "删除 {name}？");
        assert!(entry("Open in a new tab").fuzzy);
        assert!(!entry("This PC").fuzzy);
        assert_eq!(entry("Line one\nand \"two\"").translation, "第一行\n和“二”");
        assert_eq!(entry("Not yet").translation, "");
        assert!(entry("Open").context);
        assert!(entry("{n} file").plural);
        assert_eq!(entry("{n} file").translation, "{n} 个文件");
        assert!(!english.contains(&"Gone"), "obsolete entries are left out");
    }

    #[test]
    fn says_where_a_catalog_is_broken() {
        assert_eq!(
            parse("msgid \"a\"\nmsgstr \"b").unwrap_err(),
            "line 2: expected a string in quotes, found \"\\\"b\""
        );
        assert_eq!(
            parse("msgid \"a\"\n").unwrap_err(),
            "line 1: msgid without a msgstr at the end of the file"
        );
        assert_eq!(parse("msgstr \"b\"\n").unwrap_err(), "line 1: msgstr before msgid");
        assert_eq!(
            parse("msgid \"a\\q\"\nmsgstr \"\"\n").unwrap_err(),
            "line 1: \\q isn't an escape Glint's catalogs use"
        );
        assert_eq!(
            parse("msgid \"a\"\nmsgstr \"\"\nnonsense\n").unwrap_err(),
            "line 3: can't read \"nonsense\""
        );
        assert_eq!(
            parse("\"stray\"\n").unwrap_err(),
            "line 1: a string that goes on from nothing"
        );
        assert!(parse("").unwrap().entries.is_empty());
        // Windows line ends are fine.
        assert_eq!(
            parse("msgid \"a\"\r\nmsgstr \"b\"\r\n").unwrap().entries[0].translation,
            "b"
        );
    }

    #[test]
    fn usable_keeps_translations_sorted_and_says_what_it_leaves_out() {
        let text = format!(
            "{SAMPLE}\nmsgid \"Zip\"\nmsgstr \"压缩\"\n\nmsgid \"Copy to %1\"\nmsgstr \"复制到\"\n\nmsgid \"This PC\"\nmsgstr \"这台电脑\"\n"
        );
        let (kept, rejected) = usable(parse(&text).unwrap());
        assert_eq!(
            kept,
            [
                ("Delete {name}?".to_string(), "删除 {name}？".to_string()),
                ("Line one\nand \"two\"".to_string(), "第一行\n和“二”".to_string()),
                ("This PC".to_string(), "此电脑".to_string()),
                ("Zip".to_string(), "压缩".to_string()),
            ],
            "sorted, the first of a twice-given English kept, fuzzy and untranslated ones left to English"
        );
        let why: Vec<(usize, &str)> = rejected.iter().map(|r| (r.line, r.why.as_str())).collect();
        assert_eq!(
            why,
            [
                (29, "it has a msgctxt, and Glint's texts have no context"),
                (
                    32,
                    "it has plural forms: Glint's counts are two texts, \"{n} file\" and \"{n} files\""
                ),
                (42, "the translation lacks %1"),
                (45, "the same English is in the file twice"),
            ]
        );
    }

    #[test]
    fn placeholders_must_match() {
        assert_eq!(
            placeholders("{n} of {total}, {n} again, %1, {not a name}, {}"),
            ["{n}", "{total}", "%1"]
        );
        assert_eq!(placeholders("% image"), ["%"]);
        assert_eq!(
            placeholders_differ("% image", "图像").as_deref(),
            Some("the translation lacks %")
        );
        assert_eq!(placeholders_differ("% image", "% 图像"), None);
        assert_eq!(placeholders_differ("Delete {name}?", "删除 {name}？"), None);
        assert_eq!(
            placeholders_differ("{a} and {b}", "{b} 和 {a}"),
            None,
            "the order may change"
        );
        assert_eq!(
            placeholders_differ("Delete {name}?", "删除？").as_deref(),
            Some("the translation lacks {name}")
        );
        assert_eq!(
            placeholders_differ("Delete it?", "删除 {name}？").as_deref(),
            Some("the translation has {name}, which the English doesn't")
        );
        assert_eq!(
            placeholders_differ("Copy to %1", "复制到 {name}").as_deref(),
            Some("the translation lacks %1 and has {name}, which the English doesn't")
        );
    }
}
