//! Ported from src/lib/rank.ts.
//!
//! Ranking for Ctrl+K and search: fuzzy matching of typed letters, close matches for typos, and
//! frecency (how often and how recently a folder was opened).
//!
//! Strings are walked as `char`s, never by JS-style UTF-16 indices. The highlight positions
//! `fuzzy_indices` returns are UTF-8 byte offsets into the text, ready for GPUI text runs.

use crate::js::is_js_space;
use regex::Regex;
use std::sync::LazyLock;

/// How often and how recently a folder was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Visit {
    /// How many times it was opened.
    pub count: u32,
    /// When it was last opened, in Unix milliseconds.
    pub last: i64,
}

const HOUR: i64 = 3_600_000;

/// zoxide-style rank: the number of visits, weighted by how recent the last one was.
/// `now` is the current time in Unix milliseconds.
pub fn frecency(visit: &Visit, now: i64) -> f64 {
    let age = now.saturating_sub(visit.last);
    let weight = if age < HOUR {
        4.0
    } else if age < 24 * HOUR {
        2.0
    } else if age < 7 * 24 * HOUR {
        0.5
    } else {
        0.25
    };
    f64::from(visit.count) * weight
}

/// The query as it is matched: lowercase, with every space removed.
fn clean_query(query: &str) -> Vec<char> {
    query.to_lowercase().chars().filter(|c| !is_js_space(*c)).collect()
}

/// UTF-16 length, as JS `.length` (the TypeScript's scores use it).
// TODO(coordinator): from js.rs, if it gains a UTF-16 length helper.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// A text prepared for matching: its characters, the lowercase form to search in, and where each
/// lowercase character came from in the original.
struct Folded {
    orig: Vec<char>,
    lower: Vec<char>,
    /// For each `lower` position, the index in `orig` it came from.
    from: Vec<usize>,
    /// For each `orig` index, its UTF-8 byte offset in the text.
    bytes: Vec<usize>,
}

impl Folded {
    fn new(text: &str) -> Folded {
        let orig: Vec<char> = text.chars().collect();
        let bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        // Lowercasing the whole text gets context rules right (a final sigma). When that keeps one
        // character per character, positions line up. Otherwise (a capital whose lowercase is two
        // characters, such as a dotted I) lowercase each character and remember where it came from.
        let whole: Vec<char> = text.to_lowercase().chars().collect();
        let (lower, from) = if whole.len() == orig.len() {
            (whole, (0..orig.len()).collect())
        } else {
            let mut lower = Vec::new();
            let mut from = Vec::new();
            for (i, c) in orig.iter().enumerate() {
                for l in c.to_lowercase() {
                    lower.push(l);
                    from.push(i);
                }
            }
            (lower, from)
        };
        Folded {
            orig,
            lower,
            from,
            bytes,
        }
    }

    /// Where `c` is in the lowercase text at or after `at`, as `indexOf` does.
    fn index_of(&self, c: char, at: usize) -> Option<usize> {
        self.lower.get(at..)?.iter().position(|x| *x == c).map(|p| p + at)
    }

    /// A word start: the text's start, after a separator, or a capital after a small letter.
    fn is_boundary(&self, i: usize) -> bool {
        if i == 0 {
            return true;
        }
        let Some(&o) = self.from.get(i) else { return false };
        if o == 0 {
            return false;
        }
        let (Some(&c), Some(&prev)) = (self.orig.get(o), self.orig.get(o - 1)) else {
            return false;
        };
        matches!(prev, '\\' | '/' | '.' | '_' | '-' | '(' | ')' | '[' | ']')
            || is_js_space(prev)
            || (!is_same_lower(c) && is_same_lower(prev))
    }
}

/// Whether a character is its own lowercase form.
fn is_same_lower(c: char) -> bool {
    let mut lower = c.to_lowercase();
    lower.next() == Some(c) && lower.next().is_none()
}

/// What one in-order run of the query's letters, starting at `start`, found.
struct Run {
    score: f64,
    /// One past the last matched position (in lowercase characters).
    end: usize,
    /// The matched positions (in lowercase characters).
    found: Vec<usize>,
}

/// The letters in order from `start`, each as early as it comes: the score, and where it ended.
fn run_from(folded: &Folded, q: &[char], start: usize) -> Option<Run> {
    let mut score = 0.0_f64;
    let mut at = start;
    let mut streak = 0_u32;
    let mut found_at = Vec::with_capacity(q.len());
    for &c in q {
        let found = folded.index_of(c, at)?;
        found_at.push(found);
        streak = if found == at { streak + 1 } else { 0 };
        let boundary = if folded.is_boundary(found) { 3.0 } else { 0.0 };
        score += 1.0 + boundary + 2.0 * f64::from(streak) - 0.05 * (found - at) as f64;
        at = found + 1;
    }
    Some(Run {
        score,
        end: at,
        found: found_at,
    })
}

/// Scores `text` for `query` typed as an in-order subsequence ("dl" finds "Downloads"), or `None` when
/// it doesn't match. Letters at word starts and runs of consecutive letters score higher, so the best
/// match comes first. Typed with a typo ("stetings"), it's a close match (`typo_distance`), below them all.
/// `tight` (for a path, where letters typed turn up anywhere): only letters close together match, as
/// zf matches a path.
// NOTE(port): the TS counted gaps in UTF-16 units and advanced one unit past an astral letter, so
// two consecutive emoji never counted as a run. Here gaps are in characters, so they do.
pub fn fuzzy_score(query: &str, text: &str, tight: bool) -> Option<f64> {
    let q = clean_query(query);
    let Some(&head) = q.first() else { return Some(0.0) };
    let folded = Folded::new(text);
    let mut best: Option<f64> = None;
    if !tight {
        best = run_from(&folded, &q, 0).map(|r| r.score);
    } else {
        // From each place the first letter is, the best match that stays close together.
        let limit = 3 * q.iter().map(|c| c.len_utf16()).sum::<usize>() + 2;
        let mut next = folded.index_of(head, 0);
        while let Some(start) = next {
            let Some(run) = run_from(&folded, &q, start) else { break };
            if run.end - start <= limit && best.is_none_or(|b| run.score > b) {
                best = Some(run.score);
            }
            next = folded.index_of(head, start + 1);
        }
    }
    let Some(best) = best else {
        let edits = typo_distance(query, text)?;
        return Some(-1.0 - edits as f64);
    };
    // Among equal matches, the shorter text wins.
    Some(best - 0.01 * js_len(text) as f64)
}

/// Character positions in `text` that match `query`'s letters in order (preferring word boundaries
/// and consecutive runs), or `None`. The positions are UTF-8 byte offsets of the matched characters
/// in `text`, each on a character boundary, ready for highlighting.
pub fn fuzzy_indices(query: &str, text: &str, tight: bool) -> Option<Vec<usize>> {
    let q = clean_query(query);
    let head = *q.first()?;
    let folded = Folded::new(text);
    let limit = 3 * q.iter().map(|c| c.len_utf16()).sum::<usize>() + 2;
    let mut best: Option<Run> = None;
    let mut next = folded.index_of(head, 0);
    while let Some(start) = next {
        let Some(run) = run_from(&folded, &q, start) else { break };
        if (!tight || run.end - start <= limit) && best.as_ref().is_none_or(|b| run.score > b.score) {
            best = Some(run);
        }
        next = folded.index_of(head, start + 1);
    }
    let best = best?;
    Some(
        best.found
            .iter()
            .filter_map(|&i| folded.from.get(i).and_then(|&o| folded.bytes.get(o)).copied())
            .collect(),
    )
}

/// How many edits a word may be off by in a close match: one from 4 letters, two from 8. A shorter
/// word, or a number, must be there as typed.
pub fn edits_allowed(word: &str) -> usize {
    let letters = word.chars().count();
    if letters < 4 || word.chars().all(char::is_numeric) {
        0
    } else if letters < 8 {
        1
    } else {
        2
    }
}

/// u: a capital, l: another letter, d: a digit, o: anything else.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Upper,
    Letter,
    Digit,
    Other,
}

// Compiled once, on the first non-ASCII character. The patterns are constants, so they always compile.
static UPPER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{Lu}$").expect("constant pattern"));
static LETTER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{L}$").expect("constant pattern"));
static DIGIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{N}$").expect("constant pattern"));

fn char_kind(c: char) -> Kind {
    if c.is_ascii() {
        return match c {
            'A'..='Z' => Kind::Upper,
            'a'..='z' => Kind::Letter,
            '0'..='9' => Kind::Digit,
            _ => Kind::Other,
        };
    }
    let mut buf = [0_u8; 4];
    let s: &str = c.encode_utf8(&mut buf);
    if UPPER_RE.is_match(s) {
        Kind::Upper
    } else if LETTER_RE.is_match(s) {
        Kind::Letter
    } else if DIGIT_RE.is_match(s) {
        Kind::Digit
    } else {
        Kind::Other
    }
}

/// Where a name's words start: its start, after anything but a letter or digit, between letters and
/// digits, and at a capital after a small letter.
fn word_starts(chars: &[char]) -> Vec<usize> {
    let letter = |k: Kind| k == Kind::Upper || k == Kind::Letter;
    let mut starts = Vec::new();
    let mut previous = Kind::Other;
    for (i, &c) in chars.iter().enumerate() {
        let kind = char_kind(c);
        if i == 0
            || previous == Kind::Other
            || (letter(previous) && kind == Kind::Digit)
            || (previous == Kind::Digit && letter(kind))
            || (previous == Kind::Letter && kind == Kind::Upper)
        {
            starts.push(i);
        }
        previous = kind;
    }
    starts
}

/// The fewest edits (a letter added, dropped or changed, or two neighbours swapped) that make `word`
/// a prefix of `text`.
fn prefix_distance(word: &[char], text: &[char]) -> usize {
    let m = word.len();
    let mut before: Vec<usize> = vec![0; m + 1];
    let mut last: Vec<usize> = (0..=m).collect();
    let mut best = last[m];
    for j in 1..=text.len() {
        let mut column: Vec<usize> = Vec::with_capacity(m + 1);
        column.push(j);
        for i in 1..=m {
            let same = word[i - 1] == text[j - 1];
            let mut d = (last[i] + 1)
                .min(column[i - 1] + 1)
                .min(last[i - 1] + usize::from(!same));
            if i > 1 && j > 1 && word[i - 1] == text[j - 2] && word[i - 2] == text[j - 1] {
                d = d.min(before[i - 2] + 1);
            }
            column.push(d);
        }
        best = best.min(column[m]);
        before = std::mem::replace(&mut last, column);
    }
    best
}

/// How many typos `query` is from `name`: each word of it within an edit of the start of a word of the
/// name (two from 8 letters; two letters swapped are one edit, and the first letter must be right or
/// swapped with the second), and shorter words and numbers in it as typed. The edits in all, or `None`
/// when it isn't close. As the instant search service finds close matches (query.rs).
pub fn typo_distance(query: &str, name: &str) -> Option<usize> {
    let chars: Vec<char> = name.chars().collect();
    // One character each (a lowercase form can be longer), so positions stay the name's.
    let lower: Vec<char> = chars.iter().map(|&c| c.to_lowercase().next().unwrap_or(c)).collect();
    let flat = name.to_lowercase();
    let starts = word_starts(&chars);
    let mut total = 0;
    for term in query.to_lowercase().split(is_js_space).filter(|t| !t.is_empty()) {
        let k = edits_allowed(term);
        if k == 0 {
            if !flat.contains(term) {
                return None;
            }
            continue;
        }
        let word: Vec<char> = term.chars().collect();
        let mut best: Option<usize> = None;
        for &s in &starts {
            let Some(&first) = lower.get(s) else { continue };
            if Some(&first) != word.first() && Some(&first) != word.get(1) {
                continue;
            }
            let end = (s + word.len() + k).min(lower.len());
            let d = prefix_distance(&word, &lower[s..end]);
            if d <= k && best.is_none_or(|b| d < b) {
                best = Some(d);
            }
        }
        total += best?;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The options that match `query`, best first (as the Ctrl+K palette orders them).
    fn best(query: &str, options: &[&str]) -> Vec<String> {
        let mut scored: Vec<(&str, f64)> = options
            .iter()
            .filter_map(|o| fuzzy_score(query, o, false).map(|s| (*o, s)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(o, _)| o.to_string()).collect()
    }

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fuzzy_score_basics() {
        assert_eq!(fuzzy_score("xyz", "Downloads", false), None);
        assert_eq!(fuzzy_score("", "anything", false), Some(0.0));
    }

    #[test]
    fn palette_ranking_puts_word_starts_and_runs_first() {
        assert_eq!(best("dl", &["Desktop", "Downloads", "Videos"]), strs(&["Downloads"]));
        assert_eq!(
            best("doc", &["Dropbox config", "Documents", "src\\docs-old"]),
            strs(&["Documents", "src\\docs-old", "Dropbox config"])
        );
        assert_eq!(
            best("nf", &["New folder", "Info notes"]),
            strs(&["New folder", "Info notes"])
        );
        assert_eq!(
            best("sh", &["Refresh", "Search here", "Show hidden items"]),
            strs(&["Show hidden items", "Search here", "Refresh"])
        );
    }

    #[test]
    fn a_typo_is_a_close_match_below_every_in_order_one() {
        assert_eq!(
            best("stetings", &["Settings", "Set as wallpaper", "Stop"]),
            strs(&["Settings"])
        );
        // A letter left out is still in order: no typo needed.
        assert_eq!(best("setings", &["Reset tiles", "Settings"]), strs(&["Settings"]));
        assert!(fuzzy_score("setings", "Settings", false).is_some_and(|s| s > 0.0));
        let typo = fuzzy_score("stetings", "Settings", false);
        let in_order = fuzzy_score("sett", "Reset tiles", false);
        assert!(typo.is_some_and(|t| in_order.is_some_and(|o| t < o)));
    }

    #[test]
    fn in_a_path_letters_far_apart_are_not_a_match() {
        let long_path = "C:\\Users\\me\\AppData\\Local\\Temp\\claude\\visualstudio\\scratchpad\\kingfisher-test";
        assert!(
            fuzzy_score("stetings", long_path, false).is_some() && fuzzy_score("stetings", long_path, true).is_none()
        );
        assert!(fuzzy_score("scratch", long_path, true).is_some_and(|s| s > 0.0));
        assert!(fuzzy_score("scrpad", long_path, true).is_some_and(|s| s > 0.0));
    }

    #[test]
    fn typo_distance_counts_edits_at_word_starts() {
        let typo = typo_distance;
        // two letters swapped are one edit
        assert_eq!(typo("reprot", "Q3 Report.pdf"), Some(1));
        // a letter missing
        assert_eq!(typo("dowloads", "Downloads"), Some(1));
        assert_eq!(typo("donwloads", "Downloads"), Some(1));
        assert_eq!(typo("downlaods", "my-downloads-old"), Some(1));
        assert_eq!(typo("invoise", "Invoices 2026"), Some(1));
        assert_eq!(typo("buget", "budget.xlsx"), Some(1));
        // a word starting at a capital
        assert_eq!(typo("reprot", "MyReportFinal.docx"), Some(1));
        // letters then digits
        assert_eq!(typo("reprot", "report2026.docx"), Some(1));
        assert_eq!(typo("photgraphs", "Photographs"), Some(1));
        // two edits from 8 letters
        assert_eq!(typo("phtograhps", "Photographs"), Some(2));
        // the first two letters swapped
        assert_eq!(typo("erport", "report"), Some(1));
        // not a typo in the first letter
        assert_eq!(typo("eport", "report"), None);
        assert_eq!(typo("report", "the report"), Some(0));
        // only at a word's start
        assert_eq!(typo("reprot", "freport.txt"), None);
        assert_eq!(typo("reprot", "Replay.mp4"), None);
        // a number as typed
        assert_eq!(typo("bdget 2026", "budget-2025.xlsx"), None);
        assert_eq!(typo("bdget 2026", "budget-2026.xlsx"), Some(1));
        assert_eq!(typo("résumé", "Mon résume.pdf"), Some(1));
        assert_eq!(typo("café", "CAFÉ menu"), Some(0));
    }

    #[test]
    fn edits_allowed_by_word_length() {
        assert_eq!(
            [
                edits_allowed("cat"),
                edits_allowed("2026"),
                edits_allowed("report"),
                edits_allowed("download")
            ],
            [0, 0, 1, 2]
        );
    }

    #[test]
    fn frecency_weighs_visits_by_recency() {
        let now = 1_000_000_000_000_i64;
        let recent = frecency(
            &Visit {
                count: 3,
                last: now - 60_000,
            },
            now,
        );
        let days_ago = frecency(
            &Visit {
                count: 3,
                last: now - 3 * 86_400_000,
            },
            now,
        );
        assert!(recent > days_ago);
        let often_old = frecency(
            &Visit {
                count: 10,
                last: now - 30 * 86_400_000,
            },
            now,
        );
        let once_recent = frecency(
            &Visit {
                count: 1,
                last: now - 2 * 3_600_000,
            },
            now,
        );
        assert!(often_old > once_recent);
        // The weights themselves.
        assert_eq!(recent, 12.0);
        assert_eq!(days_ago, 1.5);
    }

    #[test]
    fn fuzzy_indices_mark_the_matched_letters() {
        assert_eq!(fuzzy_indices("dk", "Disk usage", false), Some(vec![0, 3]));
        assert_eq!(fuzzy_indices("fch", "Find duplicate files", false), None);
        assert_eq!(
            fuzzy_indices("abc", "a____________________b____________________c", true),
            None
        );
        assert_eq!(fuzzy_indices("", "anything", false), None);
    }

    #[test]
    fn fuzzy_indices_are_utf8_byte_offsets() {
        // "é" is two bytes, so "d" sits at byte 3 although it is the third character.
        let text = "Éxd";
        let indices = fuzzy_indices("xd", text, false);
        assert_eq!(indices, Some(vec![2, 3]));
        let picked: Vec<char> = indices
            .unwrap_or_default()
            .iter()
            .filter_map(|&b| text[b..].chars().next())
            .collect();
        assert_eq!(picked, vec!['x', 'd']);
        // Arabic letters are two bytes each.
        assert_eq!(fuzzy_indices("من", "ملف من", false).map(|v| v.len()), Some(2));
    }

    #[test]
    fn unusual_input_does_not_panic() {
        assert_eq!(fuzzy_score("  ", "text", false), Some(0.0));
        assert_eq!(fuzzy_score("a", "", false), None);
        assert_eq!(typo_distance("", "name"), Some(0));
        assert!(fuzzy_score("i̇", "İstanbul", true).is_some());
        assert!(fuzzy_indices("🙂🙂", "a🙂🙂", true).is_some());
    }
}
