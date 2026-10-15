//! Name matching with the meaning the app's fd search has, so either engine answers a query the same
//! way: every term must match the name, case-insensitively; a term with `*` or `?` is a glob over the
//! whole name, any other term matches anywhere in it; "double quotes" keep spaces inside one term.
//!
//! Names are matched as UTF-8 bytes. ASCII names (the vast majority) are compared without folding;
//! the index keeps a lowercase copy of every other name, so matching never allocates.

/// Splits a query into terms; "double quotes" keep spaces in one term.
pub fn terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    terms.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}

/// The lowercase form names and terms are compared in.
pub fn fold(text: &str) -> String {
    text.to_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token<T> {
    /// `*`: any run of characters, none included.
    Star,
    /// `?`: exactly one character.
    One,
    Lit(T),
}

/// A whole-name glob (`*` and `?`; every other character is literal), case-insensitive.
#[derive(Debug, Clone)]
pub struct Glob {
    tokens: Vec<Token<char>>,
}

impl Glob {
    pub fn new(pattern: &str) -> Self {
        let tokens = fold(pattern)
            .chars()
            .map(|c| match c {
                '*' => Token::Star,
                '?' => Token::One,
                c => Token::Lit(c),
            })
            .collect();
        Glob { tokens }
    }

    /// `name` is the name as stored; `folded` its lowercase form, given when the name isn't ASCII.
    pub fn matches(&self, name: &[u8], folded: Option<&[u8]>) -> bool {
        glob(&self.tokens, folded.unwrap_or(name))
    }
}

/// UTF-8 width of the character starting with `lead`.
fn char_width(lead: u8) -> usize {
    match lead {
        0xF0..=0xF7 => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

/// Classic wildcard matching over UTF-8 that backtracks to the last star only: linear for one star,
/// never worse than the product of the lengths. ASCII letters in `text` compare case-insensitively
/// (the pattern is lowercase), so ASCII names need no folded copy.
fn glob(pattern: &[Token<char>], text: &[u8]) -> bool {
    // Bytes `c` takes at the start of `rest`, if it's there.
    let literal = |c: char, rest: &[u8]| -> Option<usize> {
        if c.is_ascii() {
            (rest.first()?.to_ascii_lowercase() == c as u8).then_some(1)
        } else {
            let mut buf = [0; 4];
            let encoded = c.encode_utf8(&mut buf).as_bytes();
            rest.starts_with(encoded).then_some(encoded.len())
        }
    };
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(Token::Star) => {
                star = Some((p, t));
                p += 1;
                continue;
            }
            Some(Token::One) => {
                p += 1;
                t += char_width(text[t]);
                continue;
            }
            Some(&Token::Lit(c)) => {
                if let Some(n) = literal(c, &text[t..]) {
                    p += 1;
                    t += n;
                    continue;
                }
            }
            None => {}
        }
        match star {
            // Let the last star swallow one more character and try again from there.
            Some((sp, st)) => {
                let st = st + char_width(text[st]);
                star = Some((sp, st));
                p = sp + 1;
                t = st;
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|t| matches!(t, Token::Star))
}

/// Whether `hay` contains `needle` (lowercase ASCII), ignoring ASCII case.
fn contains_ascii(hay: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else { return true };
    if needle.len() > hay.len() {
        return false;
    }
    let upper = first.to_ascii_uppercase();
    let last = hay.len() - needle.len();
    let mut from = 0;
    while from <= last {
        let window = &hay[from..=last];
        let found = if upper == first { memchr::memchr(first, window) } else { memchr::memchr2(first, upper, window) };
        let Some(at) = found.map(|i| from + i) else { return false };
        if hay[at + 1..at + needle.len()].eq_ignore_ascii_case(rest) {
            return true;
        }
        from = at + 1;
    }
    false
}

#[derive(Debug, Clone)]
enum Term {
    /// Anywhere in the name; lowercase.
    Contains { folded: String, ascii: bool },
    Glob(Glob),
}

/// A parsed query: every term must match.
#[derive(Debug, Clone)]
pub struct NameMatcher {
    terms: Vec<Term>,
}

impl NameMatcher {
    /// None when the query has no terms.
    pub fn new(query: &str) -> Option<Self> {
        let terms: Vec<Term> = terms(query)
            .into_iter()
            .map(|t| {
                if t.contains(['*', '?']) {
                    Term::Glob(Glob::new(&t))
                } else {
                    let folded = fold(&t);
                    Term::Contains { ascii: folded.is_ascii(), folded }
                }
            })
            .collect();
        (!terms.is_empty()).then_some(NameMatcher { terms })
    }

    /// `name` is the name as stored; `folded` its lowercase form, given when the name isn't ASCII.
    pub fn matches(&self, name: &[u8], folded: Option<&[u8]>) -> bool {
        self.terms.iter().all(|term| match term {
            Term::Contains { folded: needle, ascii } => match folded {
                None => *ascii && contains_ascii(name, needle.as_bytes()),
                Some(folded) => memchr::memmem::find(folded, needle.as_bytes()).is_some(),
            },
            Term::Glob(glob) => glob.matches(name, folded),
        })
    }
}

/// What fd's `--exclude` skips: a pattern without a separator is a glob over whole names, and a
/// folder that matches is skipped with everything in it; one with a separator is a glob over the
/// path below the searched folder.
#[derive(Debug, Clone, Default)]
pub struct Excludes {
    names: Vec<Glob>,
    paths: Vec<Glob>,
}

impl Excludes {
    pub fn new(patterns: &[String]) -> Self {
        let mut excludes = Excludes::default();
        for pattern in patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
            let path = pattern.replace('/', "\\");
            let path = path.trim_matches('\\');
            if path.contains('\\') {
                excludes.paths.push(Glob::new(path));
            } else if !path.is_empty() {
                excludes.names.push(Glob::new(path));
            }
        }
        excludes
    }

    pub fn name(&self, name: &[u8], folded: Option<&[u8]>) -> bool {
        self.names.iter().any(|g| g.matches(name, folded))
    }

    pub fn has_paths(&self) -> bool {
        !self.paths.is_empty()
    }

    /// `relative` is a path below the searched folder, with `\` separators.
    pub fn path(&self, relative: &str) -> bool {
        let folded = (!relative.is_ascii()).then(|| fold(relative));
        self.paths.iter().any(|g| g.matches(relative.as_bytes(), folded.as_deref().map(str::as_bytes)))
    }
}

/// Hidden the way fd sees it: the hidden attribute, or a name starting with a dot.
pub fn is_hidden(attrs: u32, name: &[u8]) -> bool {
    attrs & 0x2 != 0 || name.first() == Some(&b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(query: &str, name: &str) -> bool {
        let folded = (!name.is_ascii()).then(|| fold(name));
        NameMatcher::new(query).unwrap().matches(name.as_bytes(), folded.as_deref().map(str::as_bytes))
    }

    #[test]
    fn splits_terms_with_quotes() {
        assert_eq!(terms(r#"  report "q3 final"  pdf "#), ["report", "q3 final", "pdf"]);
        assert!(terms("   ").is_empty());
        assert!(NameMatcher::new(" \t ").is_none());
    }

    #[test]
    fn every_term_must_match_anywhere_ignoring_case() {
        assert!(matches("budget 2026", "Budget-2026.xlsx"));
        assert!(matches("BUDGET", "q1 budget.xlsx"));
        assert!(!matches("budget 2025", "Budget-2026.xlsx"));
        assert!(matches(r#""q3 final""#, "Report Q3 Final.pdf"));
        assert!(!matches(r#""q3 final""#, "q3-final.pdf"));
        assert!(matches("a", "A"));
        assert!(!matches("abc", "ab"));
    }

    #[test]
    fn globs_match_the_whole_name() {
        assert!(matches("*.pdf", "Report.PDF"));
        assert!(!matches("*.pdf", "report.pdf.bak"));
        assert!(matches("report*", "Report final.docx"));
        assert!(!matches("report*", "old report.docx"));
        assert!(matches("img_????.jpg", "IMG_0042.JPG"));
        assert!(!matches("img_????.jpg", "IMG_42.JPG"));
        assert!(matches("*a*b*c*", "xxAyyBzzC"));
        assert!(!matches("*a*b*c*", "xxCyyBzzA"));
        assert!(matches("*", "anything"));
        // A plain term next to a glob still matches anywhere, as fd's --glob '*term*' does.
        assert!(matches("*.pdf draft", "Draft v2.pdf"));
        assert!(!matches("*.pdf draft", "Final v2.pdf"));
        // Brackets are literal, unlike fd's glob mode.
        assert!(matches("[1080p]*", "[1080p] Movie.mkv"));
    }

    #[test]
    fn non_ascii_names_and_terms() {
        assert!(matches("résumé", "Mon RÉSUMÉ.pdf"));
        assert!(matches("ÉTÉ", "photos été 2026"));
        assert!(matches("*.PDF", "Ünïcödé.pdf"));
        assert!(matches("ü?er*", "Über alles.txt"));
        assert!(matches("مرحبا", "ملف مرحبا.txt"));
        assert!(!matches("résumé", "resume.pdf"));
        // An ASCII term still finds the ASCII part of a non-ASCII name.
        assert!(matches("pdf", "Ünïcödé.PDF"));
    }

    #[test]
    fn excludes_names_and_paths() {
        let excludes = Excludes::new(&["node_modules".into(), " *.TMP ".into(), "build/cache".into(), "  ".into()]);
        assert!(excludes.name(b"node_modules", None));
        assert!(excludes.name(b"Node_Modules", None));
        assert!(!excludes.name(b"node_modules_old", None));
        assert!(excludes.name(b"x.tmp", None));
        assert!(excludes.has_paths());
        assert!(excludes.path(r"Build\Cache"));
        assert!(!excludes.path(r"src\build\cache"));
        assert!(!Excludes::new(&[]).has_paths());
    }

    #[test]
    fn hidden_by_attribute_or_dot() {
        assert!(is_hidden(0x2, b"AppData"));
        assert!(is_hidden(0, b".git"));
        assert!(!is_hidden(0x4, b"pagefile.sys"));
    }

    #[test]
    fn ascii_search_finds_every_position() {
        assert!(contains_ascii(b"aaab", b"ab"));
        assert!(contains_ascii(b"xYz", b"yz"));
        assert!(contains_ascii(b"ab", b""));
        assert!(!contains_ascii(b"", b"a"));
        assert!(!contains_ascii(b"ba", b"ab"));
    }
}
