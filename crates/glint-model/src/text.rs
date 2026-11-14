//! Ported from src/lib/text.ts.
//!
//! Find and replace in text: the search's regular expression (`to_reg_exp`), Replace All in a file's text
//! (`replace_text`), and the split of a match line into plain and hit parts for the results list
//! (`segments`).
//!
//! Byte offsets: rg reports where each hit sits as UTF-8 byte offsets, so `segments` works on bytes, as
//! the TypeScript did. The text it returns is split on char boundaries, so the UI can use it directly.

use regex::{NoExpand, Regex, RegexBuilder};

use crate::js::is_js_space;
use crate::store::SearchOptions;

// RgMatch has only the fields segments() reads. Offsets are usize: whoever builds it from rg's JSON must convert the
// numbers without `as` truncation, and reject values that do not fit.

/// One hit inside a match line: the byte range rg reported for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Submatch {
    pub start: usize,
    pub end: usize,
}

/// A match line from rg's JSON output, as far as [`segments`] reads it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RgMatch {
    /// The line's text (`data.lines.text`), or None when rg sent none.
    pub lines_text: Option<String>,
    /// The hits in the line (`data.submatches`).
    pub submatches: Vec<Submatch>,
}

/// A piece of a match line: plain text, or a hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub hit: bool,
}

/// Longest stretch of a line that segments() renders. JavaScript counted this in UTF-16 units.
const RENDER_LIMIT: usize = 2000;
/// A line longer than this, with a hit, is shortened to its last `TAIL_KEEP` units so the hit stays visible.
const LONG_LEAD: usize = 40;
const TAIL_KEEP: usize = 30;

/// Escapes the characters that mean something in a regular expression, as the TypeScript did (the set
/// `.*+?^${}()|[]\`).
pub fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The text JavaScript's `.` matches, without its `s` flag: every character but `\n`, `\r`, U+2028 and U+2029.
const JS_DOT: &str = "[^\\n\\r\\u{2028}\\u{2029}]";

/// Rewrites the unescaped `.` outside a character class to [`JS_DOT`], so a single-line regex search matches
/// what the TypeScript's `.` matches. An escaped `\.` and a `.` inside `[...]` are left as they are.
fn js_dot(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut chars = pattern.chars();
    // How many character classes are open: a `.` counts only outside every class.
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            // An escape is copied with the character it escapes, so `\.` and `\\` keep their meaning.
            '\\' => {
                out.push(c);
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
            '[' => {
                depth += 1;
                out.push(c);
                // A `^` negates the class, and a `]` right after the `[` or `[^` is a literal, not the end.
                let mut rest = chars.clone();
                if rest.clone().next() == Some('^') {
                    rest.next();
                    out.push('^');
                }
                if rest.clone().next() == Some(']') {
                    rest.next();
                    out.push(']');
                }
                chars = rest;
            }
            ']' if depth > 0 => {
                depth -= 1;
                out.push(c);
            }
            '.' if depth == 0 => out.push_str(JS_DOT),
            _ => out.push(c),
        }
    }
    out
}

/// JS RegExp for the search, built with the regex crate. The same semantics where that crate can express
/// them: smart case (rg's `--smart-case`), whole words, literal or pattern queries, and `^`, `$` and `.`
/// reading line by line when the query spans lines.
///
/// Errors: an invalid pattern, and JS syntax the regex crate does not support (lookaround, backreferences).
/// The caller shows the error instead of replacing anything.
///
/// Differences from JS: `\b`, `\w`, `\d` and `\s` are Unicode-aware (as rg's default is). A `.` in a
/// single-line pattern matches what JS's `.` matches: any character but `\n`, `\r`, U+2028 and U+2029. Over
/// several lines the `.` matches every character, as the TypeScript's `s` flag does.
pub fn to_reg_exp(query: &str, options: &SearchOptions) -> Result<Regex, regex::Error> {
    // Over several lines, ^ and $ still mean a line's start and end, as they do in rg.
    let multi_line = query.contains('\n');
    let mut source = if options.regex {
        // The regex crate's `.` skips only `\n`, so a single-line pattern gets JavaScript's exclusions.
        if multi_line {
            query.to_string()
        } else {
            js_dot(query)
        }
    } else {
        // A multi-line literal also matches CRLF files, as rg does.
        escape_regex(query).replace('\n', "\\r?\\n")
    };
    if options.whole_word {
        source = format!("\\b(?:{source})\\b");
    }
    // Smart case: a query with no capitals ignores case; any capital makes the match exact.
    let ignore_case = !options.case_sensitive && query == query.to_lowercase();
    RegexBuilder::new(&source)
        .case_insensitive(ignore_case)
        .multi_line(multi_line)
        .dot_matches_new_line(multi_line)
        .build()
}

/// Replace All in one file's text, the way rg found the matches: line by line, so ^, $ and \s stay
/// within a line and line ends never change, unless the query itself spans lines.
///
/// A literal search inserts the replacement verbatim. A regex search reads the replacement the JS way
/// (`$1`, `$&`, `$$`, `` $` ``, `$'`, `$<name>`). See [`expand_js_template`].
pub fn replace_text(
    content: &str,
    query: &str,
    options: &SearchOptions,
    replacement: &str,
) -> Result<String, regex::Error> {
    let pattern = to_reg_exp(query, options)?;
    let replace = |text: &str| -> String {
        if options.regex {
            replace_all_expanded(&pattern, text, replacement)
        } else {
            pattern.replace_all(text, NoExpand(replacement)).into_owned()
        }
    };
    if query.contains('\n') {
        return Ok(replace(content));
    }
    // Same split as JS's content.split(/(\r?\n)/): every line is replaced, separators are kept as they are.
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(nl) = rest.find('\n') {
        let line_end = if nl > 0 && rest.as_bytes()[nl - 1] == b'\r' {
            nl - 1
        } else {
            nl
        };
        out.push_str(&replace(&rest[..line_end]));
        out.push_str(&rest[line_end..=nl]);
        rest = &rest[nl + 1..];
    }
    out.push_str(&replace(rest));
    Ok(out)
}

/// Replaces every match in `text`, reading each replacement the JS way.
fn replace_all_expanded(pattern: &Regex, text: &str, template: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in pattern.captures_iter(text) {
        let Some(whole) = caps.get(0) else { continue };
        out.push_str(&text[last..whole.start()]);
        out.push_str(&expand_js_template(template, text, &caps, pattern));
        last = whole.end();
    }
    out.push_str(&text[last..]);
    out
}

/// JS's replacement-string rules (GetSubstitution): `$$`, `$&`, `` $` ``, `$'`, `$n` and `$nn` for groups,
/// `$<name>` for named groups. Anything else after a `$` stays as written.
fn expand_js_template(template: &str, input: &str, caps: &regex::Captures, pattern: &Regex) -> String {
    let Some(whole) = caps.get(0) else { return String::new() };
    let groups = caps.len().saturating_sub(1);
    let has_named = pattern.capture_names().any(|n| n.is_some());
    let t: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < t.len() {
        let c = t[i];
        if c != '$' || i + 1 >= t.len() {
            out.push(c);
            i += 1;
            continue;
        }
        match t[i + 1] {
            '$' => {
                out.push('$');
                i += 2;
            }
            '&' => {
                out.push_str(whole.as_str());
                i += 2;
            }
            '`' => {
                out.push_str(&input[..whole.start()]);
                i += 2;
            }
            '\'' => {
                out.push_str(&input[whole.end()..]);
                i += 2;
            }
            d @ '0'..='9' => {
                let d1 = d.to_digit(10).unwrap_or(0) as usize;
                // Two digits win when they name a real group; otherwise one digit, if it names one.
                let two = t
                    .get(i + 2)
                    .and_then(|c| c.to_digit(10))
                    .map(|d2| d1 * 10 + d2 as usize);
                if let Some(n) = two.filter(|n| (1..=groups).contains(n)) {
                    out.push_str(caps.get(n).map_or("", |m| m.as_str()));
                    i += 3;
                } else if (1..=groups).contains(&d1) {
                    out.push_str(caps.get(d1).map_or("", |m| m.as_str()));
                    i += 2;
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            '<' => {
                // Without named groups, or without a closing '>', "$<" is literal text.
                let close = t[i + 2..].iter().position(|&c| c == '>').map(|p| p + i + 2);
                match close {
                    Some(end) if has_named => {
                        let name: String = t[i + 2..end].iter().collect();
                        out.push_str(caps.name(&name).map_or("", |m| m.as_str()));
                        i = end + 1;
                    }
                    _ => {
                        out.push('$');
                        i += 1;
                    }
                }
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

/// Length of a string in UTF-16 code units, as JavaScript's `.length` counts it.
fn js_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// `text.slice(0, units)` as JavaScript counts. A surrogate pair cut in half leaves a lone surrogate in
/// JS, which TextEncoder writes as U+FFFD; that is what is written here too.
fn js_slice_front(text: &str, units: usize) -> String {
    let mut out = String::with_capacity(text.len().min(units * 4));
    let mut used = 0;
    for c in text.chars() {
        let w = c.len_utf16();
        if used + w > units {
            if used < units {
                out.push('\u{FFFD}');
            }
            break;
        }
        used += w;
        out.push(c);
    }
    out
}

/// `text.slice(-units)` as JavaScript counts. A surrogate pair cut in half leaves a lone surrogate, shown
/// here as U+FFFD.
fn js_slice_back(text: &str, units: usize) -> String {
    let mut rev = String::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let w = c.len_utf16();
        if used + w > units {
            if used < units {
                rev.push('\u{FFFD}');
            }
            break;
        }
        used += w;
        rev.push(c);
    }
    rev.chars().rev().collect()
}

/// Decodes a piece of the line the way `TextDecoder` did: invalid bytes become U+FFFD, and a byte order
/// mark at the start of the piece is dropped.
fn decode_piece(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).into_owned();
    if text.starts_with('\u{feff}') {
        text['\u{feff}'.len_utf8()..].to_string()
    } else {
        text
    }
}

/// `bytes.subarray(from, to)`: clamped to the array, and empty when `to` is not after `from`.
fn sub(bytes: &[u8], from: usize, to: usize) -> &[u8] {
    let from = from.min(bytes.len());
    let to = to.min(bytes.len());
    if from >= to {
        &[]
    } else {
        &bytes[from..to]
    }
}

/// Splits a match line into plain and hit parts, using rg's UTF-8 byte offsets. Only the first 2000 UTF-16
/// units are rendered; hits beyond that in a minified line are not shown. The first part is trimmed for a
/// single-line row, and on a long line with a hit it is shortened to "…" plus its last 30 units, so the hit
/// stays visible.
///
/// Always returns at least one segment. The offsets are trusted as rg wrote them: out-of-order or
/// overlapping ones give the same odd text the TypeScript gave, and a hit past the end stops the split.
pub fn segments(m: &RgMatch) -> Vec<Segment> {
    let line = m.lines_text.as_deref().unwrap_or("");
    let bytes = js_slice_front(line, RENDER_LIMIT).into_bytes();
    let mut parts: Vec<Segment> = Vec::new();
    let mut pos = 0;
    for hit in &m.submatches {
        if hit.end > bytes.len() {
            break;
        }
        parts.push(Segment {
            text: decode_piece(sub(&bytes, pos, hit.start)),
            hit: false,
        });
        parts.push(Segment {
            text: decode_piece(sub(&bytes, hit.start, hit.end)),
            hit: true,
        });
        pos = hit.end;
    }
    let tail = decode_piece(sub(&bytes, pos, bytes.len()));
    parts.push(Segment {
        text: tail.trim_end_matches(is_js_space).to_string(),
        hit: false,
    });

    // Drop the indentation, and keep the first hit visible on a long line.
    let lead = parts[0].text.trim_start_matches(is_js_space).to_string();
    parts[0].text = if parts.len() > 1 && js_len(&lead) > LONG_LEAD {
        format!("…{}", js_slice_back(&lead, TAIL_KEEP))
    } else {
        lead
    };
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> SearchOptions {
        SearchOptions::default()
    }

    fn with(f: impl FnOnce(&mut SearchOptions)) -> SearchOptions {
        let mut o = SearchOptions::default();
        f(&mut o);
        o
    }

    /// `text.replace(toRegExp(query, o), "X")`, as the check file does it.
    fn replace(text: &str, query: &str, o: &SearchOptions) -> String {
        to_reg_exp(query, o)
            .map(|re| re.replace_all(text, "X").into_owned())
            .unwrap_or_default()
    }

    fn parts(m: &RgMatch) -> String {
        segments(m)
            .iter()
            .map(|p| if p.hit { format!("[{}]", p.text) } else { p.text.clone() })
            .collect()
    }

    fn match_line(text: &str, spans: &[(usize, usize)]) -> RgMatch {
        RgMatch {
            lines_text: Some(text.to_string()),
            submatches: spans.iter().map(|&(start, end)| Submatch { start, end }).collect(),
        }
    }

    #[test]
    fn escape_regex_escapes_only_pattern_characters() {
        assert_eq!(escape_regex("a.b*c"), "a\\.b\\*c");
        assert_eq!(
            escape_regex("(x)[y]{z}|^$?+\\"),
            "\\(x\\)\\[y\\]\\{z\\}\\|\\^\\$\\?\\+\\\\"
        );
        assert_eq!(escape_regex("plain/-text"), "plain/-text");
    }

    #[test]
    fn smart_case_follows_rg() {
        // Lowercase ignores case; any uppercase matches exactly.
        assert_eq!(replace("foo Foo FOO", "foo", &opts()), "X X X");
        assert_eq!(replace("foo Foo FOO", "Foo", &opts()), "foo X FOO");
        assert_eq!(replace("foo Foo", "foo", &with(|o| o.case_sensitive = true)), "X Foo");
    }

    #[test]
    fn literal_queries_are_escaped_and_whole_word_respects_boundaries() {
        assert_eq!(replace("a.b axb", "a.b", &opts()), "X axb");
        assert_eq!(replace("a.b axb", "a.b", &with(|o| o.regex = true)), "X X");
        assert_eq!(replace("cat concat", "cat", &with(|o| o.whole_word = true)), "X concat");
    }

    #[test]
    fn multi_line_literal_matches_crlf_files() {
        assert_eq!(replace("one\r\ntwo", "one\ntwo", &opts()), "X");
    }

    #[test]
    fn replace_all_works_line_by_line() {
        let re = with(|o| o.regex = true);
        // $ ends each line, and \s never eats a line end.
        assert_eq!(
            replace_text("a  \r\n\r\nb \n", "\\s+$", &re, "").as_deref(),
            Ok("a\r\n\r\nb\n")
        );
        assert_eq!(replace_text("x1\nx2", "^x", &re, "y").as_deref(), Ok("y1\ny2"));
        // A literal replacement is inserted verbatim, even with JS replacement syntax in it.
        assert_eq!(replace_text("a$1b", "$1", &opts(), "[$&]").as_deref(), Ok("a[$&]b"));
    }

    #[test]
    fn regex_replacements_read_js_substitutions() {
        let re = with(|o| o.regex = true);
        assert_eq!(replace_text("ab cd", "(\\w)(\\w)", &re, "$2$1").as_deref(), Ok("ba dc"));
        assert_eq!(replace_text("ab", "a", &re, "[$&$$]").as_deref(), Ok("[a$]b"));
        assert_eq!(replace_text("ab", "a", &re, "$3x").as_deref(), Ok("$3xb"));
        assert_eq!(replace_text("xay", "a", &re, "<$`|$'>").as_deref(), Ok("x<x|y>y"));
        assert_eq!(
            replace_text("2024-05", "(?<y>\\d+)-(?<m>\\d+)", &re, "$<m>/$<y>").as_deref(),
            Ok("05/2024")
        );
        assert_eq!(
            replace_text("2024-05", "(\\d+)-(\\d+)", &re, "$<m>").as_deref(),
            Ok("$<m>")
        );
    }

    #[test]
    fn unsupported_or_invalid_patterns_are_errors_not_panics() {
        let re = with(|o| o.regex = true);
        assert!(to_reg_exp("(", &re).is_err());
        assert!(to_reg_exp("foo(?=bar)", &re).is_err());
        assert!(to_reg_exp("(a)\\1", &re).is_err());
        assert!(replace_text("x", "(?<!a)x", &re, "y").is_err());
    }

    #[test]
    fn a_single_line_dot_does_not_cross_a_carriage_return_or_a_line_separator() {
        // JavaScript's `.` skips \r, U+2028 and U+2029 as well as \n, so the match stops at them.
        let re = with(|o| o.regex = true);
        assert_eq!(replace("a\rb axb", "a.b", &re), "a\rb X");
        assert_eq!(replace("a\u{2028}b axb", "a.b", &re), "a\u{2028}b X");
        assert_eq!(replace("a\u{2029}b axb", "a.b", &re), "a\u{2029}b X");
        // An ordinary character still matches the dot.
        assert_eq!(replace("a.b axb", "a.b", &re), "X X");
        // The escaped dot and a dot inside a class keep their literal meaning.
        assert_eq!(replace("a.b axb", "a\\.b", &re), "X axb");
        assert_eq!(replace("a.b a\rb", "a[.]b", &re), "X a\rb");
    }

    #[test]
    fn a_multi_line_dot_matches_every_character_as_the_s_flag_does() {
        // Over several lines the TypeScript sets the s flag, so the dot crosses \r, U+2028 and \n.
        let re = with(|o| o.regex = true);
        assert_eq!(replace("a\rb\nz", "a.b\n", &re), "Xz");
        assert_eq!(replace("a\u{2028}b\n", "a.b\n", &re), "X");
    }

    #[test]
    fn js_dot_rewrites_only_the_dots_outside_classes_and_escapes() {
        assert_eq!(js_dot("a.b\\.c[.]"), "a[^\\n\\r\\u{2028}\\u{2029}]b\\.c[.]");
        // A `]` right after `[` or `[^` is a literal, so the class runs on to the next `]`.
        assert_eq!(
            js_dot("[]a.]x.[^]y.]z."),
            "[]a.]x[^\\n\\r\\u{2028}\\u{2029}][^]y.]z[^\\n\\r\\u{2028}\\u{2029}]"
        );
        assert_eq!(js_dot("\\\\."), "\\\\[^\\n\\r\\u{2028}\\u{2029}]");
    }

    #[test]
    fn segments_use_utf8_byte_offsets() {
        // "é" is 2 bytes, so "foo" sits at bytes 9..12.
        assert_eq!(
            parts(&match_line("  héllo foo wörld\n", &[(9, 12)])),
            "héllo [foo] wörld"
        );
        assert_eq!(parts(&match_line("plain line\r\n", &[])), "plain line");
        let long = format!("{}foo", "x".repeat(100));
        assert_eq!(
            parts(&match_line(&long, &[(100, 103)])),
            format!("…{}[foo]", "x".repeat(30))
        );
    }

    #[test]
    fn segments_drop_offsets_past_the_end_and_keep_the_rest() {
        let m = match_line("abc", &[(0, 1), (2, 99)]);
        let got = segments(&m);
        assert_eq!(got.len(), 3);
        assert!(got[1].hit);
        assert_eq!(got[2].text, "bc");
        assert_eq!(segments(&RgMatch::default())[0].text, "");
    }
}
