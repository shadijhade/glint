//! Ported from src/lib/markdown.ts.
//!
//! Markdown helpers: a document's title (for the folded README card) and which links a preview may
//! follow. The TypeScript used regular expressions; the patterns are kept as they were, with
//! JavaScript's meaning of `\s`, `.` and `trim` written out where Rust's differ. No function here returns
//! an index, so the UTF-8 versus UTF-16 question does not arise: everything takes and returns whole strings.

use crate::js::js_trim;
use regex::Regex;
use std::sync::LazyLock;

/// JavaScript's `\s`, as a regex class body: tab, line feed, vertical tab, form feed, carriage return,
/// the space characters (no-break and the Unicode space separators), U+FEFF and the two line separators.
/// Rust's `char::is_whitespace` differs (it accepts U+0085 and not U+FEFF), so the set is listed.
const JS_SPACE: &str =
    r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

/// JavaScript's `.` (without the `s` flag): any character except the line terminators.
const JS_DOT: &str = r"[^\n\r\x{2028}\x{2029}]";

/// Compiles one of this file's patterns. The patterns are constants written in this file, so a failure
/// to compile would be a bug here, not something the input can cause. Each one is compiled once, on
/// first use, and kept in a `static`.
fn compile(source: &str) -> Regex {
    Regex::new(source).expect("markdown.rs patterns are constants and always compile")
}

/// Whether `s` starts with `prefix`, ignoring ASCII case (the TS regexes with the `i` flag are ASCII here).
fn has_prefix_ignore_case(s: &str, prefix: &str) -> bool {
    s.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

/// `/^[a-z][a-z\d+.-]*:/i`: a URL scheme, such as `javascript:` or `file:`.
fn has_scheme(s: &str) -> bool {
    let bytes = s.as_bytes();
    match bytes.split_first() {
        Some((first, rest)) if first.is_ascii_alphabetic() => {
            let end = rest
                .iter()
                .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'.' | b'-')))
                .unwrap_or(rest.len());
            rest.get(end) == Some(&b':')
        }
        _ => false,
    }
}

/// `/^[a-z]:[\\/]/i`: a Windows drive path such as `C:\notes` or `C:/notes`.
fn has_drive_letter(s: &str) -> bool {
    matches!(s.as_bytes(), [drive, b':', sep, ..] if drive.is_ascii_alphabetic() && matches!(sep, b'\\' | b'/'))
}

/// Links a Markdown preview may follow: anchors within the file, web and e-mail addresses, and paths.
/// Returns the link trimmed, or `None` when it goes nowhere (a script, data or file URL, or nothing at all).
pub fn safe_href(href: &str) -> Option<&str> {
    let value = js_trim(href);
    if value.is_empty() {
        return None;
    }
    if ["http:", "https:", "mailto:"]
        .iter()
        .any(|prefix| has_prefix_ignore_case(value, prefix))
        || value.starts_with('#')
    {
        return Some(value);
    }
    // Any other scheme (javascript:, data:, file:...) goes nowhere; C:\... is a path.
    if has_scheme(value) && !has_drive_letter(value) {
        return None;
    }
    Some(value)
}

/// A line of Markdown as plain text: links as their text, without pictures, emphasis, code marks or HTML
/// tags. Mirrors the TS `plain()` pass by pass, in the same order.
fn plain(line: &str) -> String {
    static IMAGE: LazyLock<Regex> = LazyLock::new(|| compile(r"!\[[^\]]*\]\([^)]*\)"));
    static LINK: LazyLock<Regex> = LazyLock::new(|| compile(r"\[([^\]]*)\]\([^)]*\)"));
    static TAG: LazyLock<Regex> = LazyLock::new(|| compile(r"<[^>]*>"));
    static EMPHASIS: LazyLock<Regex> = LazyLock::new(|| compile(r"[*_`~]+"));
    static CLOSING_HASHES: LazyLock<Regex> = LazyLock::new(|| compile(&format!("[{JS_SPACE}]+#+[{JS_SPACE}]*$")));

    let s = IMAGE.replace_all(line, "");
    let s = LINK.replace_all(&s, "${1}");
    let s = TAG.replace_all(&s, "");
    let s = EMPHASIS.replace_all(&s, "");
    let s = CLOSING_HASHES.replace(&s, "");
    let s: String = s.replace("&nbsp;", " ");
    js_trim(&s).to_string()
}

/// An ATX heading of level one, `# Title`, with up to three spaces before the mark (`^ {0,3}#\s+(.*)$`).
/// Returns the text after the mark, untrimmed.
fn atx_h1(line: &str) -> Option<&str> {
    static ATX_H1: LazyLock<Regex> = LazyLock::new(|| compile(&format!("^ {{0,3}}#[{JS_SPACE}]+({JS_DOT}*)$")));
    ATX_H1.captures(line)?.get(1).map(|m| m.as_str())
}

/// A level-one heading written as HTML: `<h1 ...>Title</h1>`. The close tag may be missing at the end of
/// the line, and the `\b` after `h1` keeps `<h10>` from matching. Returns the text inside the tags.
fn html_h1(line: &str) -> Option<&str> {
    static HTML_H1: LazyLock<Regex> =
        LazyLock::new(|| compile(&format!(r"(?i)<h1(?-u:\b)[^>]*>({JS_DOT}*?)(?:</h1>|$)")));
    HTML_H1.captures(line)?.get(1).map(|m| m.as_str())
}

/// A setext underline under a title: up to three spaces, then one or more `=` (`^ {0,3}=+\s*$`).
fn is_setext_underline(line: &str) -> bool {
    static SETEXT: LazyLock<Regex> = LazyLock::new(|| compile(&format!("^ {{0,3}}=+[{JS_SPACE}]*$")));
    SETEXT.is_match(line)
}

/// Removes a leading block marker (a heading mark, `>`, a bullet or a number) and the space after it,
/// so the text of a list item or quote can be a title too.
fn strip_block_marker(line: &str) -> String {
    static BLOCK_MARKER: LazyLock<Regex> =
        LazyLock::new(|| compile(&format!("^ {{0,3}}(?:#{{1,6}}|>|[-*+]|[0-9]+[.)])[{JS_SPACE}]+")));
    BLOCK_MARKER.replace(line, "").into_owned()
}

/// A Markdown document's title: its first top heading (`# Title`, `Title` over `===`, or `<h1>`), else the
/// first line of text that has a letter in it. Only the first 300 lines are read, as the TS did.
pub fn markdown_title(text: &str) -> String {
    static LETTER: LazyLock<Regex> = LazyLock::new(|| compile(r"\p{L}"));
    static ONLY_SEPARATORS: LazyLock<Regex> = LazyLock::new(|| compile(&format!("^[-=|:{JS_SPACE}]+$")));

    // The TS split on /\r?\n/ with a limit of 300 pieces. A carriage return is only dropped when a line
    // feed follows it, so a lone trailing "\r" stays, as it did in the TS.
    let mut lines: Vec<&str> = Vec::new();
    let mut rest = text;
    while lines.len() < 300 {
        match rest.find('\n') {
            Some(at) => {
                let line = &rest[..at];
                lines.push(line.strip_suffix('\r').unwrap_or(line));
                rest = &rest[at + 1..];
            }
            None => {
                lines.push(rest);
                break;
            }
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let heading = atx_h1(line).or_else(|| html_h1(line));
        let title = match heading {
            Some(text) => plain(text),
            None if lines.get(i + 1).is_some_and(|next| is_setext_underline(next)) => plain(line),
            None => String::new(),
        };
        if !title.is_empty() {
            return title;
        }
    }

    for line in &lines {
        let title = plain(&strip_block_marker(line));
        let has_letter = LETTER.is_match(&title);
        let only_separators = ONLY_SEPARATORS.is_match(&title);
        if has_letter && !only_separators {
            return title;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_href_refuses_script_data_and_file_urls() {
        assert_eq!(safe_href("javascript:alert(1)"), None);
        assert_eq!(safe_href(" JavaScript:alert(1)"), None);
        assert_eq!(safe_href("data:text/html,<b>"), None);
        assert_eq!(safe_href("file:///C:/x"), None);
    }

    #[test]
    fn safe_href_keeps_web_mail_anchor_and_path_links() {
        assert_eq!(safe_href("https://example.com"), Some("https://example.com"));
        assert_eq!(safe_href("#install"), Some("#install"));
        assert_eq!(safe_href("docs/a.md"), Some("docs/a.md"));
        assert_eq!(safe_href("C:\\notes\\a.md"), Some("C:\\notes\\a.md"));
    }

    #[test]
    fn safe_href_refuses_an_empty_link() {
        assert_eq!(safe_href(""), None);
    }

    // Not in the check file: the mail scheme, a no-break space around a link, and a bare drive letter.
    #[test]
    fn safe_href_allows_mail_and_trims_js_whitespace() {
        assert_eq!(safe_href("MAILTO:a@example.com"), Some("MAILTO:a@example.com"));
        assert_eq!(
            safe_href("\u{A0}https://example.com\u{FEFF}"),
            Some("https://example.com")
        );
        assert_eq!(safe_href("C:"), None);
    }

    #[test]
    fn markdown_title_reads_the_first_heading_after_html() {
        assert_eq!(
            markdown_title(
                "<div align=\"center\">\n\n<img src=\"logo.svg\" width=\"96\" alt=\"Glint logo\">\n\n# Glint\n\n**A file explorer**"
            ),
            "Glint"
        );
    }

    #[test]
    fn markdown_title_reads_a_setext_heading_with_emphasis_removed() {
        assert_eq!(markdown_title("Project *Name*\n=====\n\nText"), "Project Name");
    }

    #[test]
    fn markdown_title_reads_an_html_h1_and_drops_its_tags() {
        assert_eq!(markdown_title("<h1 align=\"center\">My <b>App</b></h1>"), "My App");
    }

    #[test]
    fn markdown_title_skips_badges_and_takes_the_first_text_line() {
        assert_eq!(
            markdown_title("[![build](https://x/badge.svg)](https://x)\n\nA tool that does `things`.\n\n## Install"),
            "A tool that does things."
        );
    }

    #[test]
    fn markdown_title_drops_closing_hashes_and_link_targets() {
        assert_eq!(markdown_title("# [Docs](https://example.com) #\n"), "Docs");
    }

    #[test]
    fn markdown_title_is_empty_without_any_text() {
        assert_eq!(markdown_title("\n\n"), "");
    }

    #[test]
    fn markdown_title_reads_only_the_first_300_lines() {
        // The TS split the text into at most 300 pieces, so a heading after line 300 is never seen.
        let late = format!("{}# Late", "\n".repeat(300));
        assert_eq!(markdown_title(&late), "");
        let just_in = format!("{}# Late", "\n".repeat(299));
        assert_eq!(markdown_title(&just_in), "Late");
    }

    #[test]
    fn repeated_calls_give_the_same_answers() {
        // The patterns are compiled on first use and reused; a second call must not see different text.
        let text = "# Glint\n\nText";
        assert_eq!(markdown_title(text), "Glint");
        assert_eq!(markdown_title(text), "Glint");
        assert_eq!(markdown_title("<h1>Glint</h1>"), "Glint");
        assert_eq!(safe_href(" https://a.example "), Some("https://a.example"));
    }
}
