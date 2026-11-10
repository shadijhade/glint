//! Links in terminal text: web addresses and Windows paths, found with plain character scanning.
//!
//! The terminal opens a link on Ctrl+click. xterm.js had a web-links addon for this; here the text of the
//! (possibly wrapped) line is scanned for `http://` and `https://` addresses, `mailto:` addresses and Windows
//! paths (`C:\folder\file.txt`, `\\server\share`, with a `:line:column` tail cut off). The scanning works on
//! `char`s and returns char positions, which the caller maps to cells.

use super::emu::LinkTarget;

/// One link found: char positions `start..end` in the scanned text, and what it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub target: LinkTarget,
}

/// What an OSC 8 hyperlink's address opens, if Glint opens it: web and mail addresses, and `file:` addresses
/// (as paths). Anything else (a program name, `javascript:`) is not opened from terminal text.
pub fn target_of_uri(uri: &str) -> Option<LinkTarget> {
    let lower = uri.get(..8).unwrap_or(uri).to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:") {
        return Some(LinkTarget::Url(uri.to_string()));
    }
    if lower.starts_with("file://") {
        return file_url_path(uri).map(LinkTarget::Path);
    }
    None
}

/// `file:///C:/a%20b` as `C:\a b` (and `file://host/share/x` as `\\host\share\x`).
pub fn file_url_path(uri: &str) -> Option<String> {
    let rest = uri.get(7..)?;
    let decoded = percent_decode(rest);
    // file:///C:/x leaves "/C:/x"; file://host/x leaves "host/x".
    if let Some(path) = decoded.strip_prefix('/') {
        if path.as_bytes().get(1) == Some(&b':') {
            return Some(path.replace('/', "\\"));
        }
        return None;
    }
    let (host, path) = decoded.split_once('/')?;
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    Some(format!("\\\\{host}\\{}", path.replace('/', "\\")))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3)).flatten();
        if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// A character that ends an address.
fn ends_address(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | '`' | '{' | '}' | '|' | '^' | '\\')
}

/// A character that ends a path (spaces end it too, unless it was quoted).
fn ends_path(c: char) -> bool {
    c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '*' | '?')
}

fn starts_with_at(text: &[char], at: usize, prefix: &str) -> bool {
    let mut i = at;
    for p in prefix.chars() {
        match text.get(i) {
            Some(c) if c.eq_ignore_ascii_case(&p) => i += 1,
            _ => return false,
        }
    }
    true
}

/// Cuts punctuation that belongs to the sentence, not the address: `.,;:!?'` and a closing bracket with no
/// opening one inside the address.
fn trim_address(text: &[char], start: usize, mut end: usize) -> usize {
    while end > start {
        let c = text[end - 1];
        let drop = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' => true,
            ')' | ']' => {
                let open = if c == ')' { '(' } else { '[' };
                let opens = text[start..end].iter().filter(|x| **x == open).count();
                let closes = text[start..end].iter().filter(|x| **x == c).count();
                closes > opens
            }
            _ => false,
        };
        if !drop {
            break;
        }
        end -= 1;
    }
    end
}

/// Every link in `text`, in order, never overlapping.
pub fn find_links(text: &[char]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let before_ok = i == 0 || !is_word(text[i - 1]);
        if before_ok {
            if let Some(link) = address_at(text, i).or_else(|| path_at(text, i)) {
                i = link.end.max(i + 1);
                found.push(link);
                continue;
            }
        }
        i += 1;
    }
    found
}

fn address_at(text: &[char], at: usize) -> Option<Found> {
    let scheme = ["https://", "http://", "mailto:", "file://"]
        .into_iter()
        .find(|prefix| starts_with_at(text, at, prefix))?;
    let body = at + scheme.chars().count();
    let mut end = body;
    while end < text.len() && !ends_address(text[end]) {
        end += 1;
    }
    let end = trim_address(text, at, end);
    if end <= body {
        return None;
    }
    let url: String = text[at..end].iter().collect();
    let target = if scheme == "file://" {
        LinkTarget::Path(file_url_path(&url)?)
    } else {
        LinkTarget::Url(url)
    };
    Some(Found { start: at, end, target })
}

fn path_at(text: &[char], at: usize) -> Option<Found> {
    let first = *text.get(at)?;
    let unc = first == '\\' && text.get(at + 1) == Some(&'\\') && text.get(at + 2).is_some_and(|c| is_word(*c));
    let drive = first.is_ascii_alphabetic()
        && text.get(at + 1) == Some(&':')
        && matches!(text.get(at + 2), Some('\\') | Some('/'))
        && text.get(at + 3).is_some_and(|c| !c.is_whitespace());
    if !unc && !drive {
        return None;
    }
    // Quoted, a path may hold spaces; the quote ends it.
    let quote = at
        .checked_sub(1)
        .and_then(|p| text.get(p))
        .copied()
        .filter(|q| matches!(q, '"' | '\''));
    let mut end = at;
    while end < text.len() && !ends_path(text[end]) && Some(text[end]) != quote {
        if text[end].is_whitespace() && quote.is_none() {
            break;
        }
        end += 1;
    }
    // Sentence punctuation is not the path's.
    while end > at && matches!(text[end - 1], '.' | ',' | ';' | ':' | '!' | ')' | ']' | '\'') {
        end -= 1;
    }
    if quote.is_some() {
        while end > at && text[end - 1] == ' ' {
            end -= 1;
        }
    }
    let shown_end = end;
    // `:12` and `:12:5` after the file name are a position in it, not part of the name.
    let mut path_end = end;
    for _ in 0..2 {
        let mut digits = path_end;
        while digits > at && text[digits - 1].is_ascii_digit() {
            digits -= 1;
        }
        if digits < path_end && digits > at + 2 && text[digits - 1] == ':' {
            path_end = digits - 1;
        } else {
            break;
        }
    }
    if path_end < at + 3 {
        return None;
    }
    let path: String = text[at..path_end].iter().collect();
    Some(Found {
        start: at,
        end: shown_end,
        target: LinkTarget::Path(path.replace('/', "\\")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(text: &str) -> Vec<(String, LinkTarget)> {
        let chars: Vec<char> = text.chars().collect();
        find_links(&chars)
            .into_iter()
            .map(|f| (chars[f.start..f.end].iter().collect(), f.target))
            .collect()
    }

    fn url(text: &str) -> LinkTarget {
        LinkTarget::Url(text.into())
    }

    fn path(text: &str) -> LinkTarget {
        LinkTarget::Path(text.into())
    }

    #[test]
    fn web_addresses_lose_the_sentences_punctuation() {
        assert_eq!(
            links("see https://example.com/a?b=1, or (http://x.org/p). Done."),
            vec![
                ("https://example.com/a?b=1".into(), url("https://example.com/a?b=1")),
                ("http://x.org/p".into(), url("http://x.org/p")),
            ]
        );
        // A bracket that belongs to the address stays.
        assert_eq!(
            links("https://en.wikipedia.org/wiki/Rust_(language)"),
            vec![(
                "https://en.wikipedia.org/wiki/Rust_(language)".into(),
                url("https://en.wikipedia.org/wiki/Rust_(language)")
            )]
        );
        assert_eq!(
            links("mailto:me@example.com now"),
            vec![("mailto:me@example.com".into(), url("mailto:me@example.com"))]
        );
        // A scheme alone is nothing; so is one inside a word.
        assert!(links("https:// and xhttps://a.b").is_empty());
    }

    #[test]
    fn windows_paths_are_found_with_their_position_cut_off() {
        assert_eq!(
            links("error in C:\\Users\\me\\a.txt:12:5 here"),
            vec![("C:\\Users\\me\\a.txt:12:5".into(), path("C:\\Users\\me\\a.txt"))]
        );
        assert_eq!(
            links("\\\\server\\share\\x.log."),
            vec![("\\\\server\\share\\x.log".into(), path("\\\\server\\share\\x.log"))]
        );
        assert_eq!(links("d:/work/x"), vec![("d:/work/x".into(), path("d:\\work\\x"))]);
        // Quoted, a path may hold spaces.
        assert_eq!(
            links("open \"C:\\Program Files\\App\\a.exe\" ok"),
            vec![(
                "C:\\Program Files\\App\\a.exe".into(),
                path("C:\\Program Files\\App\\a.exe")
            )]
        );
        // A drive letter alone, or the middle of a word, is not a path.
        assert!(links("C: and abC:\\x").is_empty());
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(
            links("file:///C:/My%20Files/a.txt"),
            vec![("file:///C:/My%20Files/a.txt".into(), path("C:\\My Files\\a.txt"))]
        );
        assert_eq!(target_of_uri("file://host/share/x"), Some(path("\\\\host\\share\\x")));
        assert_eq!(target_of_uri("javascript:alert(1)"), None);
        assert_eq!(target_of_uri("HTTPS://A.B"), Some(url("HTTPS://A.B")));
        assert_eq!(target_of_uri("ssh://host"), None);
    }

    #[test]
    fn arabic_text_around_a_link_is_left_alone() {
        let found = links("الرابط https://example.com/ص انتهى");
        assert_eq!(found.len(), 1);
        assert!(found[0].0.starts_with("https://example.com/"));
    }
}
