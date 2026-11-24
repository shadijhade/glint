//! What a search means when it was typed on a keyboard that isn't an English one.
//!
//! A Chinese input method types its punctuation full width: `：` for the colon, `；` and `，` between values, `！`
//! for not, `“ ”` for quotes, `》` and `《` where `>` and `<` are, and the space bar can give an ideographic space.
//! Someone typing `label：重要` or `size：》10mb` would have to switch to English for every filter. [`ascii_syntax`]
//! reads those as the ASCII the syntax wants, and only where the syntax is: before a filter's key, at its colon and
//! in its value. The words to look for stay as typed, so a file named `报告：终稿` is still found by its name.
//!
//! [`label_names`] is the other half: a label is found by its color in either language. `label:红色` finds the
//! label named Red, and `label:red` finds the one an import from a Chinese XYplorer named 红色.
//!
//! Both answer the text as it was when there is nothing to change (nearly always), without copying it.

use std::borrow::Cow;

/// Every filter key of the search box, in lower case: after one of these and a colon comes syntax, not a name.
pub const KEYS: [&str; 23] = [
    "ext", "size", "dm", "file", "files", "folder", "folders", "path", "git", "width", "height", "duration", "taken",
    "camera", "artist", "album", "label", "lbl", "tag", "tags", "comment", "cmt", "has",
];

/// The keys whose values are numbers, units and signs: full-width digits and letters in them are folded too.
const COUNTED: [&str; 6] = ["size", "dm", "width", "height", "duration", "taken"];

/// A full-width form of an ASCII character (U+FF01 to U+FF5E) as that character.
fn narrow(ch: char) -> Option<char> {
    let code = u32::from(ch);
    (0xFF01..=0xFF5E)
        .contains(&code)
        .then(|| char::from_u32(code - 0xFEE0))
        .flatten()
}

/// Whether anything in `text` could need reading: a character these rules know. One pass, no allocation.
fn may_need(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, '\u{3000}' | '“' | '”' | '》' | '《') || narrow(ch).is_some())
}

/// `key` (as typed before the colon, a leading not already taken off) as a filter key, if it is one.
fn key_of(key: &str) -> Option<&'static str> {
    let folded: String = key
        .chars()
        .map(|ch| narrow(ch).unwrap_or(ch).to_ascii_lowercase())
        .collect();
    KEYS.iter().copied().find(|known| *known == folded)
}

/// A filter's value with the signs the syntax uses as ASCII; in a counted key's, digits and letters too.
fn value_of(key: &str, value: &str) -> String {
    let counted = COUNTED.contains(&key);
    value
        .chars()
        .map(|ch| match ch {
            '；' => ';',
            '，' => ',',
            '》' | '＞' => '>',
            '《' | '＜' => '<',
            '＝' => '=',
            _ if counted => narrow(ch).unwrap_or(ch),
            _ => ch,
        })
        .collect()
}

/// One term (no spaces outside quotes): a filter with its key, colon and value read as ASCII, or the term as typed.
fn term_of(term: &str) -> Cow<'_, str> {
    let (not, rest) = match term.strip_prefix(['!', '！']) {
        Some(rest) => (true, rest),
        None => (false, term),
    };
    let Some(at) = rest.find([':', '：']) else {
        return Cow::Borrowed(term);
    };
    let Some(key) = key_of(&rest[..at]) else {
        return Cow::Borrowed(term);
    };
    let colon = rest[at..].chars().next().map_or(1, char::len_utf8);
    let value = value_of(key, &rest[at + colon..]);
    Cow::Owned(format!("{}{key}:{value}", if not { "!" } else { "" }))
}

/// `query` with its filters' syntax as ASCII (see the module's notes). The words between the filters, and the
/// spaces, are as typed, but for the quotes (`“ ”` become `"`) and the ideographic space (a space).
pub fn ascii_syntax(query: &str) -> Cow<'_, str> {
    if !may_need(query) {
        return Cow::Borrowed(query);
    }
    let mut out = String::with_capacity(query.len());
    let mut term = String::new();
    let mut quoted = false;
    let flush = |term: &mut String, out: &mut String| {
        if !term.is_empty() {
            out.push_str(&term_of(term));
            term.clear();
        }
    };
    for ch in query.chars() {
        let ch = match ch {
            '“' | '”' => '"',
            '\u{3000}' => ' ',
            other => other,
        };
        if ch == '"' {
            quoted = !quoted;
            term.push(ch);
        } else if !quoted && ch.is_whitespace() {
            flush(&mut term, &mut out);
            out.push(ch);
        } else {
            term.push(ch);
        }
    }
    flush(&mut term, &mut out);
    if out == query {
        Cow::Borrowed(query)
    } else {
        Cow::Owned(out)
    }
}

/// The colors labels come in, each as people write it: in English, then in Chinese (Simplified and Traditional,
/// with and without 色).
const COLORS: [&[&str]; 7] = [
    &["red", "红", "红色", "紅", "紅色"],
    &["orange", "橙", "橙色", "橘", "橘色"],
    &["yellow", "黄", "黄色", "黃", "黃色"],
    &["green", "绿", "绿色", "綠", "綠色"],
    &["blue", "蓝", "蓝色", "藍", "藍色"],
    &["purple", "紫", "紫色"],
    &["gray", "grey", "灰", "灰色"],
];

/// The label of `labels` that `word` means by its color, when no label is named `word` itself: the one whose
/// name is the same color in another language.
fn by_color<'a>(word: &str, labels: &[&'a str]) -> Option<&'a str> {
    let lower = word.trim().trim_matches('"').trim().to_lowercase();
    if lower.is_empty() || labels.iter().any(|name| name.trim().to_lowercase() == lower) {
        return None;
    }
    let color = COLORS.iter().find(|names| names.contains(&lower.as_str()))?;
    labels
        .iter()
        .copied()
        .find(|name| color.contains(&name.trim().to_lowercase().as_str()))
}

/// `query` with each `label:` name that is a color word of another language than the label's own replaced by the
/// label's name (`labels`: Settings' labels' names). Expects ASCII syntax ([`ascii_syntax`]).
pub fn label_names<'a>(query: &'a str, labels: &[&str]) -> Cow<'a, str> {
    let lower = query.to_lowercase();
    if !(lower.contains("label:") || lower.contains("lbl:")) || labels.is_empty() {
        return Cow::Borrowed(query);
    }
    let mut out = String::with_capacity(query.len());
    let mut changed = false;
    let mut term = String::new();
    let mut quoted = false;
    let mut flush = |term: &mut String, out: &mut String| {
        if term.is_empty() {
            return;
        }
        let (not, rest) = match term.strip_prefix('!') {
            Some(rest) => ("!", rest),
            None => ("", term.as_str()),
        };
        match rest.split_once(':') {
            Some((key, value)) if key.eq_ignore_ascii_case("label") || key.eq_ignore_ascii_case("lbl") => {
                let names: Vec<String> = value
                    .split(';')
                    .map(|name| match by_color(name, labels) {
                        // A color's word is one word: the name needs no quotes.
                        Some(label) => {
                            changed = true;
                            label.to_string()
                        }
                        None => name.to_string(),
                    })
                    .collect();
                out.push_str(not);
                out.push_str(key);
                out.push(':');
                out.push_str(&names.join(";"));
            }
            _ => out.push_str(term),
        }
        term.clear();
    };
    for ch in query.chars() {
        if ch == '"' {
            quoted = !quoted;
            term.push(ch);
        } else if !quoted && ch.is_whitespace() {
            flush(&mut term, &mut out);
            out.push(ch);
        } else {
            term.push(ch);
        }
    }
    flush(&mut term, &mut out);
    if changed {
        Cow::Owned(out)
    } else {
        Cow::Borrowed(query)
    }
}

/// What `query` means as typed on any keyboard: [`ascii_syntax`], then [`label_names`].
pub fn as_meant<'a>(query: &'a str, labels: &[&str]) -> Cow<'a, str> {
    match ascii_syntax(query) {
        Cow::Borrowed(same) => label_names(same, labels),
        Cow::Owned(read) => Cow::Owned(label_names(&read, labels).into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(query: &str) -> String {
        ascii_syntax(query).into_owned()
    }

    #[test]
    fn a_query_in_ascii_is_handed_back_as_it_is() {
        for query in [
            "",
            "report 2024",
            "ext:pdf size:>10mb",
            "label:red;orange !tag:old",
            "\"two words\" dm:7d",
        ] {
            assert!(matches!(ascii_syntax(query), Cow::Borrowed(_)), "{query}");
            assert!(
                matches!(as_meant(query, &["Red", "Orange"]), Cow::Borrowed(_)),
                "{query}"
            );
        }
    }

    #[test]
    fn a_chinese_keyboards_punctuation_is_read_as_the_syntax_it_stands_for() {
        // The colon after a key, the separators and the comparison signs of its value.
        assert_eq!(read("label：重要"), "label:重要");
        assert_eq!(read("ext：jpg；png"), "ext:jpg;png");
        assert_eq!(read("tag：工作，报告"), "tag:工作,报告");
        assert_eq!(read("size：》10mb"), "size:>10mb");
        assert_eq!(read("dm：《7d"), "dm:<7d");
        // Not, in front of a key.
        assert_eq!(read("！label：red"), "!label:red");
        assert_eq!(read("!has：comment"), "!has:comment");
        // Quotes keep a phrase; the ideographic space parts terms.
        assert_eq!(read("comment：“周五 之前”　ext：pdf"), "comment:\"周五 之前\" ext:pdf");
        // Full-width letters and digits: in the key, and in a value that is a number.
        assert_eq!(read("ｓｉｚｅ：＞１０ｍｂ"), "size:>10mb");
        assert_eq!(read("taken：２０２４"), "taken:2024");
    }

    #[test]
    fn words_to_look_for_stay_as_they_were_typed() {
        // No key before the colon: a name, which may hold any of these.
        assert_eq!(read("报告：终稿"), "报告：终稿");
        assert_eq!(read("会议纪要，第二版"), "会议纪要，第二版");
        assert_eq!(read("！重要"), "！重要");
        // A name's own full-width letters are not a number's.
        assert_eq!(read("tag：ＡＢＣ"), "tag:ＡＢＣ");
        assert_eq!(read("comment：１２３"), "comment:１２３");
        // Inside quotes a space doesn't end the term, so a filter's phrase stays whole.
        assert_eq!(read("label：“to read” x"), "label:\"to read\" x");
        // Mixed with ASCII filters, only what needs reading changes.
        assert_eq!(read("ext:pdf 发票 size：》1mb"), "ext:pdf 发票 size:>1mb");
    }

    #[test]
    fn a_label_is_found_by_its_color_in_either_language() {
        let english = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"];
        assert_eq!(as_meant("label：红色", &english), "label:Red");
        assert_eq!(as_meant("label:红", &english), "label:Red");
        assert_eq!(as_meant("label:紅色;蓝", &english), "label:Red;Blue");
        assert_eq!(as_meant("!lbl:灰色 ext:pdf", &english), "!lbl:Gray ext:pdf");
        // Labels named in Chinese (an import from a Chinese XYplorer): the English word finds them.
        let chinese = ["红色", "橙色", "黄色", "绿色", "蓝色", "紫色", "天蓝色"];
        assert_eq!(as_meant("label:red", &chinese), "label:红色");
        assert_eq!(
            as_meant("label:Grey;green", &chinese),
            "label:Grey;绿色",
            "no gray label there: left alone"
        );
        // A label named that way wins over the color; a word that is no color is a name.
        let own = ["红", "Urgent"];
        assert_eq!(as_meant("label:红", &own), "label:红");
        assert_eq!(as_meant("label:urgent", &own), "label:urgent");
        assert_eq!(as_meant("label:重要", &english), "label:重要");
        // Only a label named by the color's word alone is that color's.
        assert_eq!(as_meant("label:红", &["Very red", "red"]), "label:red");
        assert_eq!(as_meant("label:蓝色", &["Sky blue", "blue"]), "label:blue");
        // Only label: is read this way.
        assert_eq!(as_meant("tag:红色 红色", &english), "tag:红色 红色");
    }
}
