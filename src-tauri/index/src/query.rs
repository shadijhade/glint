//! Name matching with the meaning the app's fd search has, so either engine answers a query the same
//! way: every term must match the name, case-insensitively; a term with `*` or `?` is a glob over the
//! whole name, any other term matches anywhere in it; "double quotes" keep spaces inside one term.
//!
//! Names are matched as UTF-8 bytes. ASCII names (the vast majority) are compared without folding;
//! the index keeps a lowercase copy of every other name, so matching never allocates.
//!
//! Filters narrow the matches Everything-style ([`parse`]): `ext:`, `size:`, `dm:`, `file:` and
//! `folder:`. The index applies them itself, and the app turns them into fd's flags.

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

/// How many edits a word of a search for close matches may be from a name's: one from 4 letters, two
/// from 8 (as search engines allow typos); a shorter word must be there as typed.
pub fn edits(letters: usize) -> u8 {
    match letters {
        0..=3 => 0,
        4..=7 => 1,
        _ => 2,
    }
}

/// Words longer than this aren't matched with typos.
const MAX_NEAR: usize = 32;
/// Names are at most 255 UTF-16 units: no more characters than this.
const MAX_NAME: usize = 256;

/// A name's lowercase characters, and where its words start: at its start, after anything but a
/// letter or digit, between letters and digits, and (ASCII names) at a capital after a small letter.
struct Words {
    chars: [char; MAX_NAME],
    starts: [bool; MAX_NAME],
    len: usize,
}

impl Words {
    /// None for a name that isn't UTF-8 (the index's names are).
    fn of(name: &[u8], folded: Option<&[u8]>) -> Option<Self> {
        let mut words = Words { chars: ['\0'; MAX_NAME], starts: [false; MAX_NAME], len: 0 };
        let mut previous: Option<char> = None;
        let mut push = |words: &mut Words, original: char, lower: char| {
            if words.len == MAX_NAME {
                return;
            }
            let start = match previous {
                None => true,
                Some(p) => !p.is_alphanumeric() || (p.is_alphabetic() && original.is_numeric()) || (p.is_numeric() && original.is_alphabetic()) || (p.is_lowercase() && original.is_uppercase()),
            };
            words.chars[words.len] = lower;
            words.starts[words.len] = start;
            words.len += 1;
            previous = Some(original);
        };
        match folded {
            None => name.iter().for_each(|&b| push(&mut words, b as char, b.to_ascii_lowercase() as char)),
            // A name with other characters: its lowercase copy, so no capitals to start words at.
            Some(folded) => std::str::from_utf8(folded).ok()?.chars().for_each(|c| push(&mut words, c, c)),
        }
        Some(words)
    }
}

/// A word of a search for close matches: found within `k` edits of the start of one of a name's
/// words (prefix, so "reprot" finds "Reports"). A typo in the first letter isn't looked for, only a
/// swap of the first two.
#[derive(Debug, Clone)]
struct Near {
    word: Vec<char>,
    k: u8,
}

impl Near {
    fn distance(&self, words: &Words) -> Option<u8> {
        let (first, second) = (self.word[0], self.word.get(1).copied());
        let mut best: Option<u8> = None;
        for start in (0..words.len).filter(|&i| words.starts[i]) {
            let c = words.chars[start];
            if c != first && Some(c) != second {
                continue;
            }
            // A prefix more than k letters longer than the word is more than k edits away.
            let end = (start + self.word.len() + self.k as usize).min(words.len);
            if let Some(d) = prefix_distance(&self.word, &words.chars[start..end], self.k) {
                best = Some(best.map_or(d, |b| b.min(d)));
            }
        }
        best
    }
}

/// The fewest edits (a letter added, dropped or changed, or two neighbours swapped) that make `word`
/// a prefix of `text`, if that's at most `k`. The edit table, a column at a time (the last two kept
/// for swaps).
fn prefix_distance(word: &[char], text: &[char], k: u8) -> Option<u8> {
    let m = word.len();
    let (mut before, mut last, mut column) = ([0u8; MAX_NEAR + 1], [0u8; MAX_NEAR + 1], [0u8; MAX_NEAR + 1]);
    for (i, cell) in last.iter_mut().enumerate().take(m + 1) {
        *cell = i as u8;
    }
    let mut best = last[m];
    for j in 1..=text.len() {
        column[0] = j as u8;
        for i in 1..=m {
            let changed = u8::from(word[i - 1] != text[j - 1]);
            let mut d = (last[i] + 1).min(column[i - 1] + 1).min(last[i - 1] + changed);
            if i > 1 && j > 1 && word[i - 1] == text[j - 2] && word[i - 2] == text[j - 1] {
                d = d.min(before[i - 2] + 1);
            }
            column[i] = d;
        }
        best = best.min(column[m]);
        (before, last) = (last, column);
    }
    (best <= k).then_some(best)
}

#[derive(Debug, Clone)]
enum Term {
    /// Anywhere in the name; lowercase.
    Contains { folded: String, ascii: bool },
    Glob(Glob),
    /// Close to the start of a word of the name, with a typo or two.
    Near(Near),
}

/// A parsed query: every term must match.
#[derive(Debug, Clone)]
pub struct NameMatcher {
    terms: Vec<Term>,
}

impl NameMatcher {
    /// None when the query has no terms.
    pub fn new(query: &str) -> Option<Self> {
        let matcher = NameMatcher::from_terms(terms(query));
        (!matcher.terms.is_empty()).then_some(matcher)
    }

    /// Every term must match; with none, every name does (a query of filters only).
    pub fn from_terms(terms: Vec<String>) -> Self {
        let terms = terms
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
        NameMatcher { terms }
    }

    /// The same terms with typos allowed: each word of 4 letters or more may be an edit or two from
    /// the start of a word of the name (see `edits`); globs, numbers and shorter words match as typed.
    /// None when no word is long enough to have typos looked for.
    pub fn near(terms: Vec<String>) -> Option<Self> {
        let mut near = false;
        let terms = terms
            .into_iter()
            .map(|t| {
                let word: Vec<char> = fold(&t).chars().collect();
                let k = edits(word.len());
                if t.contains(['*', '?']) || k == 0 || word.len() > MAX_NEAR || word.iter().all(|c| c.is_numeric()) {
                    return NameMatcher::from_terms(vec![t]).terms.remove(0);
                }
                near = true;
                Term::Near(Near { word, k })
            })
            .collect();
        near.then_some(NameMatcher { terms })
    }

    /// Whether this looks for close matches rather than the terms as typed.
    pub fn is_near(&self) -> bool {
        self.terms.iter().any(|t| matches!(t, Term::Near(_)))
    }

    /// `name` is the name as stored; `folded` its lowercase form, given when the name isn't ASCII.
    pub fn matches(&self, name: &[u8], folded: Option<&[u8]>) -> bool {
        self.distance(name, folded).is_some()
    }

    /// How many edits away the name is, when it matches (0 for terms matched as typed).
    pub fn distance(&self, name: &[u8], folded: Option<&[u8]>) -> Option<u32> {
        let mut words: Option<Words> = None;
        let mut total = 0;
        for term in &self.terms {
            match term {
                Term::Contains { folded: needle, ascii } => {
                    let found = match folded {
                        None => *ascii && contains_ascii(name, needle.as_bytes()),
                        Some(folded) => memchr::memmem::find(folded, needle.as_bytes()).is_some(),
                    };
                    if !found {
                        return None;
                    }
                }
                Term::Glob(glob) => {
                    if !glob.matches(name, folded) {
                        return None;
                    }
                }
                Term::Near(near) => {
                    let words = match &words {
                        Some(words) => words,
                        None => words.insert(Words::of(name, folded)?),
                    };
                    total += u32::from(near.distance(words)?);
                }
            }
        }
        Some(total)
    }
}

/// What a query asks of items besides their names: `ext:pdf` (or `ext:jpg;png`), `size:>10mb`
/// (`>`, `>=`, `<`, `<=` or exactly; b, kb, mb, gb, tb in 1024s, as Explorer counts), `dm:7d` (modified
/// in the last 7 days; h, d, w, y) or `dm:>1y` (before that), and `file:` or `folder:`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filters {
    /// Lowercase extensions without the dot; a name needs one of them.
    pub exts: Vec<String>,
    /// Files only, of at least and at most this many bytes.
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    /// Modified less than, or more than, this many seconds ago.
    pub newer_than: Option<u64>,
    pub older_than: Option<u64>,
    pub kind: Option<Kind>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Files,
    Folders,
}

impl Filters {
    pub fn is_empty(&self) -> bool {
        self == &Filters::default()
    }

    /// `file:`, or a size, which only files have.
    pub fn files_only(&self) -> bool {
        self.kind == Some(Kind::Files) || self.min_size.is_some() || self.max_size.is_some()
    }

    /// Whether an item passes. `folded` is its name's lowercase form when the name isn't ASCII, and
    /// `age` how many seconds ago it was modified.
    pub fn accepts(&self, name: &[u8], folded: Option<&[u8]>, dir: bool, size: u64, age: i64) -> bool {
        if (dir && self.files_only()) || (!dir && self.kind == Some(Kind::Folders)) {
            return false;
        }
        let name = folded.unwrap_or(name);
        let ext = |ext: &String| name.len() > ext.len() && name[name.len() - ext.len() - 1] == b'.' && name[name.len() - ext.len()..].eq_ignore_ascii_case(ext.as_bytes());
        (self.exts.is_empty() || self.exts.iter().any(ext))
            && self.min_size.is_none_or(|min| size >= min)
            && self.max_size.is_none_or(|max| size <= max)
            && self.newer_than.is_none_or(|secs| age < secs as i64)
            && self.older_than.is_none_or(|secs| age > secs as i64)
    }
}

/// A query's name terms and its filters. A `word:` term that isn't a filter stays a name term (which
/// can't match: names can't hold a colon); a filter that doesn't read right is an error saying how
/// it goes.
pub fn parse(query: &str) -> Result<(Vec<String>, Filters), String> {
    let mut names = Vec::new();
    let mut filters = Filters::default();
    for term in terms(query) {
        let Some((key, value)) = term.split_once(':') else {
            names.push(term);
            continue;
        };
        match key.to_ascii_lowercase().as_str() {
            "ext" => filters.exts.extend(value.split([';', ',']).map(|e| e.trim().trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty())),
            "size" => {
                let (op, amount) = split_op(value);
                let bytes = size(amount).ok_or_else(|| format!("“{term}”: a size goes like size:>10mb or size:<=1gb"))?;
                match op {
                    ">" => filters.min_size = Some(bytes.saturating_add(1)),
                    ">=" => filters.min_size = Some(bytes),
                    "<" => filters.max_size = Some(bytes.saturating_sub(1)),
                    "<=" => filters.max_size = Some(bytes),
                    _ => (filters.min_size, filters.max_size) = (Some(bytes), Some(bytes)),
                }
            }
            "dm" => {
                let (op, amount) = split_op(value);
                let secs = duration(amount).ok_or_else(|| format!("“{term}”: a time goes like dm:7d (the last 7 days) or dm:>1y (before that)"))?;
                if op.starts_with('>') {
                    filters.older_than = Some(secs);
                } else {
                    filters.newer_than = Some(secs);
                }
            }
            "file" | "files" if value.is_empty() => filters.kind = Some(Kind::Files),
            "folder" | "folders" if value.is_empty() => filters.kind = Some(Kind::Folders),
            _ => names.push(term),
        }
    }
    Ok((names, filters))
}

/// `>=10mb` → (">=", "10mb").
fn split_op(value: &str) -> (&str, &str) {
    value.split_at(value.find(|c: char| !matches!(c, '<' | '>' | '=')).unwrap_or(value.len()))
}

/// "10mb", "1.5gb" or "300" (bytes), in bytes.
fn size(text: &str) -> Option<u64> {
    let at = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let number: f64 = text[..at].parse().ok()?;
    let unit: u64 = match text[at..].to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1 << 10,
        "m" | "mb" => 1 << 20,
        "g" | "gb" => 1 << 30,
        "t" | "tb" => 1 << 40,
        _ => return None,
    };
    Some((number * unit as f64) as u64)
}

/// "12h", "7d", "2w" or "1y", in seconds.
fn duration(text: &str) -> Option<u64> {
    let at = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
    let number: u64 = text[..at].parse().ok()?;
    let unit = match text[at..].to_ascii_lowercase().as_str() {
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        "y" => 365 * 86_400,
        _ => return None,
    };
    number.checked_mul(unit)
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
    fn reads_filters_and_applies_them() {
        let (names, f) = parse("report ext:PDF;.docx size:>10mb dm:7d").unwrap();
        assert_eq!(names, ["report"]);
        assert_eq!(f.exts, ["pdf", "docx"]);
        assert_eq!((f.min_size, f.max_size, f.newer_than), (Some((10 << 20) + 1), None, Some(7 * 86_400)));
        assert!(f.accepts(b"Q3.PDF", None, false, 11 << 20, 3600));
        assert!(!f.accepts(b"Q3.pdf", None, false, 10 << 20, 3600), "not bigger than 10 MB");
        assert!(!f.accepts(b"Q3.pdf", None, false, 11 << 20, 8 * 86_400), "too old");
        assert!(!f.accepts(b"Q3.pdf", None, true, 11 << 20, 3600), "a size means files");
        assert!(!f.accepts(b"Q3.pdfx", None, false, 11 << 20, 3600));
        let (names, f) = parse("folder: dm:>1y size:<=1.5gb").unwrap();
        assert!(names.is_empty());
        assert_eq!((f.kind, f.older_than, f.max_size), (Some(Kind::Folders), Some(365 * 86_400), Some(3 << 29)));
        // A size makes it files only, so folder: and size: together match nothing.
        assert!(!f.accepts(b"old", None, true, 0, 400 * 86_400));
        assert_eq!(parse("C:x \"a:b\"").unwrap().0, ["C:x", "a:b"], "not filters: names");
        assert!(parse("size:big").is_err() && parse("dm:7").is_err() && parse("size:").is_err());
        assert!(parse("ext: file:").unwrap().1 == Filters { kind: Some(Kind::Files), ..Default::default() });
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

    fn near(query: &str, name: &str) -> Option<u32> {
        let folded = (!name.is_ascii()).then(|| fold(name));
        NameMatcher::near(terms(query))?.distance(name.as_bytes(), folded.as_deref().map(str::as_bytes))
    }

    #[test]
    fn close_matches_allow_a_typo_or_two_at_a_word_start() {
        assert_eq!(near("reprot", "Q3 Report.pdf"), Some(1), "two letters swapped are one edit");
        assert_eq!(near("dowloads", "Downloads"), Some(1), "a letter missing");
        assert_eq!(near("donwloads", "Downloads"), Some(1));
        assert_eq!(near("downlaods", "my-downloads-old"), Some(1));
        assert_eq!(near("invoise", "Invoices 2026"), Some(1), "a letter changed, and the name's word goes on");
        assert_eq!(near("buget", "budget.xlsx"), Some(1));
        assert_eq!(near("reprot", "MyReportFinal.docx"), Some(1), "a word starting at a capital");
        assert_eq!(near("reprot", "report2026.docx"), Some(1), "letters then digits");
        assert_eq!(near("photgraphs", "Photographs"), Some(1));
        assert_eq!(near("phtograhps", "Photographs"), Some(2), "two edits from 8 letters");
        assert_eq!(near("reprot", "Reports"), Some(1));
        assert_eq!(near("erport", "report"), Some(1), "the first two letters swapped");
        assert_eq!(near("rpeort", "report"), Some(1));
        assert_eq!(near("eport", "report"), None, "nor a typo in the first letter");
        assert_eq!(near("report", "the report"), Some(0), "a word as typed is no edit");
        assert_eq!(near("reprot", "freport.txt"), None, "only at a word's start");
        assert_eq!(near("reprot", "Replay.mp4"), None, "too far");
        assert_eq!(near("bdget 2026", "budget-2025.xlsx"), None, "a number must be there as typed");
        assert_eq!(near("bdget cat", "budget dog.xlsx"), None, "and a short word");
        assert_eq!(near("bdget 2026", "budget-2026.xlsx"), Some(1));
        assert_eq!(near("résumé", "Mon résume.pdf"), Some(1), "non-ASCII names");
        assert_eq!(near("café", "CAFÉ menu"), Some(0));
        assert!(NameMatcher::near(terms("cat *.pdf")).is_none(), "no word long enough to look for typos in");
        assert!(NameMatcher::near(terms("reprot")).unwrap().is_near() && !NameMatcher::new("reprot").unwrap().is_near());
    }

    /// The edit table, filled whole: the reference `prefix_distance` is checked against.
    fn table_distance(word: &[char], text: &[char]) -> u8 {
        let (m, n) = (word.len(), text.len());
        let mut d = vec![vec![0u8; n + 1]; m + 1];
        for (i, row) in d.iter_mut().enumerate() {
            row[0] = i as u8;
        }
        for (j, cell) in d[0].iter_mut().enumerate() {
            *cell = j as u8;
        }
        for i in 1..=m {
            for j in 1..=n {
                let mut v = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + u8::from(word[i - 1] != text[j - 1]));
                if i > 1 && j > 1 && word[i - 1] == text[j - 2] && word[i - 2] == text[j - 1] {
                    v = v.min(d[i - 2][j - 2] + 1);
                }
                d[i][j] = v;
            }
        }
        (0..=n).map(|j| d[m][j]).min().unwrap()
    }

    #[test]
    fn prefix_distance_is_the_edit_tables() {
        // Every pair of short words over a small alphabet, where edits collide most.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let word: Vec<char> = (0..1 + next() % 8).map(|_| (b'a' + (next() % 3) as u8) as char).collect();
            let text: Vec<char> = (0..next() % 12).map(|_| (b'a' + (next() % 3) as u8) as char).collect();
            let expected = table_distance(&word, &text);
            for k in 0..3 {
                assert_eq!(prefix_distance(&word, &text, k), (expected <= k).then_some(expected), "{word:?} in {text:?}");
            }
        }
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
