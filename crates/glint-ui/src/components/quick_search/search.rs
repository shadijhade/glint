//! What Quick Search does with names, apart from drawing them: the words typed, the order results come in,
//! the part of a name that matched, the status line, and the folders shown before anything is typed.
//!
//! Ported from src/components/QuickSearch.tsx (`wordsOf`, `rank`, `Name`, the status text) and
//! src/lib/zoxide.ts (`usedScore`, `usedFolders`). All pure: the view and the tests call these directly.
//!
//! Strings are walked as `char`s, never by JavaScript-style UTF-16 indices; the one range returned for the
//! view ([`mark_range`]) is in UTF-8 bytes on character boundaries, what GPUI's text runs take.

use crate::t;
use glint_model::entries::{is_hidden, FileEntry};
use glint_model::format::{format_count, format_duration};
use glint_model::js::is_js_space;
use glint_model::paths::base_name;
use glint_model::rank::{frecency, typo_distance, Visit};
use gpui_kit::SharedString;
use std::collections::HashMap;
use std::ops::Range;

/// The search lane core knows Quick Search's search by, so a newer query ends the older one and Stop can end it.
pub const LANE: &str = "quick";
/// Shown; more are asked for, so the best can be picked from them.
pub const LIMIT: usize = 100;
/// How many names the index is asked for.
pub const ASKED: usize = 1000;
/// How many used folders are offered before anything is typed.
pub const USED_FOLDERS: usize = 8;

/// Windows' own folders and caches: what a search for your files rarely means (`SYSTEM`). The TypeScript's
/// regular expression `\\(windows\\(winsxs|servicing|…)|programdata|…|node_modules|\.git)\\`, case-insensitive,
/// as the folders it names, each between backslashes.
const SYSTEM_FOLDERS: &[&str] = &[
    "\\windows\\winsxs\\",
    "\\windows\\servicing\\",
    "\\windows\\assembly\\",
    "\\windows\\installer\\",
    "\\windows\\system32\\",
    "\\windows\\syswow64\\",
    "\\windows\\softwaredistribution\\",
    "\\windows\\systemapps\\",
    "\\windows\\immersivecontrolpanel\\",
    "\\programdata\\",
    "\\$recycle.bin\\",
    "\\appdata\\local\\temp\\",
    "\\appdata\\local\\packages\\",
    "\\appdata\\local\\microsoft\\",
    "\\node_modules\\",
    "\\.git\\",
];

/// Whether `path` is inside one of Windows' own folders or a cache.
pub fn is_system(path: &str) -> bool {
    let lower = path.to_lowercase();
    SYSTEM_FOLDERS.iter().any(|folder| lower.contains(folder))
}

/// The words typed, without filters (`ext:pdf`…) and wildcards; for paths, the last segment named
/// (`wordsOf`). Lower case.
pub fn words_of(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(is_js_space)
        .filter(|w| !w.is_empty() && (!w.contains(':') || is_drive_path(w) || w.starts_with("path:")))
        .filter_map(|w| {
            let word = w.strip_prefix("path:").unwrap_or(w);
            let clean: String = word.chars().filter(|c| !matches!(c, '*' | '?' | '"')).collect();
            clean
                .rsplit(['\\', '/'])
                .find(|part| !part.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// `/^[a-z]:[\\/]/`: starts like a path on a drive ("c:\", "d:/").
fn is_drive_path(word: &str) -> bool {
    let mut chars = word.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some('a'..='z'), Some(':'), Some('\\' | '/'))
    )
}

/// Whether `lower` has `first` at the start of a word (`(^|[\s._-])first`).
fn word_starts_with(lower: &str, first: &str) -> bool {
    lower.match_indices(first).any(|(at, _)| {
        lower
            .get(..at)
            .and_then(|before| before.chars().next_back())
            .is_none_or(|c| is_js_space(c) || matches!(c, '.' | '_' | '-'))
    })
}

/// How well each entry fits the words, lower is better: a name that is what was typed, then one that starts
/// with it, then one with a word that does, then the index's close matches (fewest typos first); then what's
/// outside Windows' own folders and not hidden.
fn score(entry: &FileEntry, words: &[String]) -> usize {
    let first = words.first().map_or("", String::as_str);
    let stem = match entry.lower.rfind('.') {
        Some(dot) => entry.lower.get(..dot).unwrap_or(""),
        None => entry.lower.as_str(),
    };
    let matched = if first.is_empty() || stem == first || entry.lower == first {
        0
    } else if entry.lower.starts_with(first) {
        1
    } else if word_starts_with(&entry.lower, first) {
        2
    } else {
        3
    };
    let typos = if !first.is_empty() && !entry.lower.contains(first) {
        1 + typo_distance(&words.join(" "), &entry.name).unwrap_or(3)
    } else {
        0
    };
    (matched + typos) * 4 + if is_system(&entry.path) { 2 } else { 0 } + usize::from(is_hidden(entry))
}

/// The entries' indices, best first (`rank`): by score, then the shorter path (as JavaScript counts it, in
/// UTF-16 units). Ties keep their order, as the TypeScript's stable sort did.
pub fn rank_order(entries: &[FileEntry], words: &[String]) -> Vec<usize> {
    let mut scored: Vec<(usize, usize, usize)> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (score(e, words), e.path.encode_utf16().count(), i))
        .collect();
    scored.sort_by_key(|&(score, length, _)| (score, length));
    scored.into_iter().map(|(_, _, i)| i).collect()
}

/// What a search has found so far, ranked as it grows: each batch from core is added and the best
/// [`LIMIT`] are handed back (the TypeScript re-ranked everything found on every batch too).
pub struct Collector {
    words: Vec<String>,
    entries: Vec<FileEntry>,
}

impl Collector {
    /// A collector for `query`'s words.
    pub fn new(query: &str) -> Self {
        Self {
            words: words_of(query),
            entries: Vec::new(),
        }
    }

    /// Adds a batch and returns the best [`LIMIT`] of everything so far, best first.
    pub fn add(&mut self, batch: impl IntoIterator<Item = FileEntry>) -> Vec<FileEntry> {
        self.entries.extend(batch);
        rank_order(&self.entries, &self.words)
            .into_iter()
            .take(LIMIT)
            .filter_map(|i| self.entries.get(i).cloned())
            .collect()
    }
}

/// Where in `name` the first typed word that it holds sits, as a byte range on character boundaries
/// (`Name`'s `<mark>`): the match is case-insensitive. None when no word is in it.
pub fn mark_range(name: &str, words: &[String]) -> Option<Range<usize>> {
    let lower = name.to_lowercase();
    // Aligned character by character; a character whose lower case is longer (some capitals) breaks the
    // alignment, and then nothing is marked rather than the wrong letters.
    if lower.chars().count() != name.chars().count() {
        return None;
    }
    let word = words.iter().find(|w| lower.contains(w.as_str()))?;
    let at = lower.find(word.as_str())?;
    let before = lower.get(..at)?.chars().count();
    let starts: Vec<usize> = name
        .char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(name.len()))
        .collect();
    let start = *starts.get(before)?;
    let end = *starts.get(before + word.chars().count())?;
    (start < end).then_some(start..end)
}

/// The line under the results after a search: who answered, how many it found and how fast.
pub fn status_line(engine: &str, fuzzy: bool, count: usize, limited: bool, elapsed: f64) -> SharedString {
    let close = match (fuzzy, count) {
        (false, _) => SharedString::default(),
        (true, 0) => t!("nothing as typed, nor close · "),
        (true, _) => t!("nothing as typed · close matches: "),
    };
    let more = if count > LIMIT {
        t!(" · the best {n} shown", n = LIMIT)
    } else {
        SharedString::default()
    };
    t!(
        "{engine} · {close}{count}{plus} found in {time}{more}",
        engine = engine,
        close = close,
        count = format_count(count),
        plus = if limited { "+" } else { "" },
        time = format_duration(elapsed),
        more = more
    )
}

// ---- Folders you use most ---------------------------------------------------------------------------------

/// How much a folder is used: its visits here (frecency) and its zoxide score, together (`usedScore`).
fn used_score(path: &str, visits: &HashMap<&str, Visit>, zoxide: &HashMap<String, f64>, now: i64) -> f64 {
    visits.get(path).map_or(0.0, |visit| frecency(visit, now))
        + zoxide.get(&path.to_lowercase()).copied().unwrap_or(0.0)
}

/// The folders used most, here and (shared) in terminals, best first (`usedFolders`). `visits` are Glint's, in
/// the order they were first made; `zoxide` is zoxide's list with scores. A folder both know (whatever the case)
/// is listed once, under Glint's spelling, in the place it came first; zoxide's folders only add what Glint
/// doesn't know.
///
/// TODO(coordinator): the command palette ranks the same way; one of the two copies should become the shared one.
pub fn used_folders<'a>(
    visits: impl IntoIterator<Item = (&'a str, Visit)>,
    zoxide: &[(String, f64)],
    limit: usize,
    now: i64,
) -> Vec<String> {
    let visits: Vec<(&str, Visit)> = visits.into_iter().collect();
    let by_path: HashMap<&str, Visit> = visits.iter().copied().collect();
    let scores: HashMap<String, f64> = zoxide
        .iter()
        .map(|(path, score)| (path.to_lowercase(), *score))
        .collect();
    // Lower-case spelling → place in `all`.
    let mut place: HashMap<String, usize> = HashMap::new();
    let mut all: Vec<String> = Vec::new();
    let mut put = |path: &str, replace: bool| {
        let key = path.to_lowercase();
        match place.get(&key) {
            Some(&at) => {
                if let (true, Some(slot)) = (replace, all.get_mut(at)) {
                    *slot = path.to_string();
                }
            }
            None => {
                place.insert(key, all.len());
                all.push(path.to_string());
            }
        }
    };
    for (path, _) in &visits {
        put(path, true);
    }
    // zoxide's folders only add what Glint doesn't know already.
    for (path, _) in zoxide {
        put(path, false);
    }
    let mut ranked: Vec<(String, f64)> = all
        .into_iter()
        .map(|path| {
            let score = used_score(&path, &by_path, &scores, now);
            (path, score)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.into_iter().take(limit).map(|(path, _)| path).collect()
}

/// A used folder as a result row: a folder, named by its last part (the whole path for a drive).
pub fn folder_entry(path: &str) -> FileEntry {
    let name = match base_name(path) {
        "" => path,
        name => name,
    };
    FileEntry {
        name: name.to_string(),
        path: path.to_string(),
        dir: true,
        size: 0,
        modified: 0,
        attrs: 0,
        lower: path.to_lowercase(),
        ext: String::new(),
        origin: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::entries::{attr, with_details, RawEntry};

    fn entry(path: &str) -> FileEntry {
        let name = path.rsplit('\\').next().unwrap_or(path).to_string();
        with_details(
            RawEntry {
                name,
                path: Some(path.to_string()),
                ..Default::default()
            },
            "",
        )
    }

    fn hidden(path: &str) -> FileEntry {
        FileEntry {
            attrs: attr::HIDDEN,
            ..entry(path)
        }
    }

    fn paths(entries: &[FileEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.path.as_str()).collect()
    }

    #[test]
    fn words_leave_out_filters_and_wildcards_and_name_the_last_part_of_a_path() {
        assert_eq!(words_of("Report  ext:pdf size:>10mb"), ["report"]);
        assert_eq!(words_of("  budget 2026 "), ["budget", "2026"]);
        assert_eq!(words_of("*.pdf"), [".pdf"]);
        assert_eq!(words_of("\"my file\""), ["my", "file"]);
        assert_eq!(words_of(r"C:\Users\me\Notes"), ["notes"]);
        assert_eq!(words_of("path:src/lib/util"), ["util"]);
        assert_eq!(words_of("d:/photos/"), ["photos"]);
        assert!(words_of("dm:7d ext:txt").is_empty());
        assert!(words_of("   ").is_empty());
        // A path with a colon after the drive letter only counts when it starts like a path.
        assert!(words_of("note:today").is_empty());
        assert_eq!(words_of("تقرير الربع"), ["تقرير", "الربع"]);
    }

    #[test]
    fn system_folders_are_recognised_in_any_case() {
        assert!(is_system(r"C:\Windows\WinSxS\x86\a.dll"));
        assert!(is_system(r"C:\ProgramData\Foo\a.txt"));
        assert!(is_system(r"C:\Users\me\AppData\Local\Temp\a.txt"));
        assert!(is_system(r"D:\code\app\node_modules\left-pad\a.js"));
        assert!(is_system(r"D:\code\app\.git\config"));
        assert!(!is_system(r"C:\Windows\notepad.exe"));
        assert!(!is_system(r"C:\Users\me\Documents\a.txt"));
        // The folder's own name isn't "inside" itself.
        assert!(!is_system(r"D:\code\node_modules"));
    }

    #[test]
    fn exact_names_come_first_then_starts_then_word_starts_then_the_rest() {
        let found = [
            entry(r"C:\a\my-report-2024.txt"),
            entry(r"C:\a\report.pdf"),
            entry(r"C:\a\reports archive.zip"),
            entry(r"C:\a\an_report.docx"),
            entry(r"C:\a\unreported.txt"),
        ];
        let order = rank_order(&found, &words_of("report"));
        let ranked: Vec<&str> = order.iter().map(|&i| found[i].name.as_str()).collect();
        // report.pdf is the stem; reports… starts with it; an_report… and my-report… hold it at a word start
        // (the shorter path first); unreported holds it inside a word, in none of those.
        assert_eq!(
            ranked,
            [
                "report.pdf",
                "reports archive.zip",
                "an_report.docx",
                "my-report-2024.txt",
                "unreported.txt"
            ]
        );
    }

    #[test]
    fn system_folders_and_hidden_items_come_after_the_same_kind_of_match() {
        let found = [
            entry(r"C:\Windows\System32\notes.txt"),
            hidden(r"C:\Users\me\notes.txt"),
            entry(r"C:\Users\me\Documents\notes.txt"),
        ];
        let order = rank_order(&found, &words_of("notes"));
        assert_eq!(order, [2, 1, 0]);
    }

    #[test]
    fn close_matches_rank_by_typos_and_shorter_paths_break_ties() {
        let found = [
            entry(r"C:\data\recieve.txt"),
            entry(r"C:\x\receive.txt"),
            entry(r"C:\data\receive.txt"),
        ];
        // "recieve" is in the first by exact name; the other two are one edit away, the shorter path first.
        let order = rank_order(&found, &words_of("recieve"));
        assert_eq!(order, [0, 1, 2]);
        // No words (only filters): everything ties, so the order is the shorter path first, then as found.
        let order = rank_order(&found, &[]);
        assert_eq!(order, [1, 0, 2]);
    }

    #[test]
    fn the_collector_keeps_the_best_hundred_of_all_batches() {
        let mut collector = Collector::new("note");
        let first: Vec<FileEntry> = (0..150)
            .map(|n| entry(&format!(r"C:\many\a-note-{n:03}.txt")))
            .collect();
        let best = collector.add(first);
        assert_eq!(best.len(), LIMIT);
        // A later batch with a better name displaces the rest.
        let best = collector.add([entry(r"C:\z\note.txt")]);
        assert_eq!(best.len(), LIMIT);
        assert_eq!(paths(&best)[0], r"C:\z\note.txt");
    }

    #[test]
    fn the_matched_part_of_a_name_is_marked_whatever_its_case_or_script() {
        let words = words_of("REPORT");
        assert_eq!(mark_range("Annual Report.pdf", &words), Some(7..13));
        assert_eq!(mark_range("Annual.pdf", &words), None);
        // Arabic: byte offsets on character boundaries (two bytes a letter), the typed word found inside the name.
        let name = "تقرير الربع 1.pdf";
        let range = mark_range(name, &["الربع".to_string()]).expect("marked");
        assert_eq!(&name[range], "الربع");
        // The first word the name holds is the one marked.
        assert_eq!(
            mark_range("budget 2026.xlsx", &["zzz".into(), "2026".into()]),
            Some(7..11)
        );
        // Capitals whose lower case is longer shift the letters: nothing is marked rather than the wrong ones.
        assert_eq!(mark_range("İstanbul", &["stanbul".to_string()]), None);
    }

    #[test]
    fn the_status_line_says_who_answered_how_many_and_how_fast() {
        let _guard = crate::testing::lock();
        crate::testing::set_rtl(false);
        assert_eq!(
            status_line("index", false, 12, false, 3.2),
            "index · 12 found in 3.2 ms"
        );
        assert_eq!(
            status_line("index", false, 500, true, 1234.0),
            "index · 500+ found in 1.23 s · the best 100 shown"
        );
        assert_eq!(
            status_line("index", true, 3, false, 40.0),
            "index · nothing as typed · close matches: 3 found in 40 ms"
        );
        assert_eq!(
            status_line("fd", true, 0, false, 40.0),
            "fd · nothing as typed, nor close · 0 found in 40 ms"
        );
    }

    #[test]
    fn used_folders_add_visits_and_zoxide_scores_once_per_folder() {
        let now = 10 * 24 * 3_600_000;
        let day = |n: i64| now - n * 24 * 3_600_000;
        let visits = [
            (
                r"C:\Work",
                Visit {
                    count: 10,
                    last: day(30),
                },
            ), // 2.5
            (r"C:\Photos", Visit { count: 1, last: day(0) }), // 4 (opened within the hour)
            (
                r"C:\Old",
                Visit {
                    count: 1,
                    last: day(30),
                },
            ), // 0.25
        ];
        let zoxide = vec![
            ("c:\\photos".to_string(), 3.0),    // the same folder in another case: 7 together
            (r"D:\Elsewhere".to_string(), 4.0), // zoxide's alone
            (r"E:\Cold".to_string(), 0.1),
        ];
        let used = used_folders(visits, &zoxide, 3, now);
        assert_eq!(used, [r"C:\Photos", r"D:\Elsewhere", r"C:\Work"]);
        // Ties keep the order the folders came in.
        let tie = used_folders([(r"C:\A", Visit::default()), (r"C:\B", Visit::default())], &[], 8, now);
        assert_eq!(tie, [r"C:\A", r"C:\B"]);
    }

    #[test]
    fn a_used_folder_row_is_a_folder_named_by_its_last_part() {
        let row = folder_entry(r"C:\Users\me\Projects\");
        assert!(row.dir);
        assert_eq!(row.name, "Projects");
        assert_eq!(row.path, r"C:\Users\me\Projects\");
        // A drive is named as the path helper names it.
        assert_eq!(folder_entry(r"C:\").name, "C:");
        assert_eq!(folder_entry("").name, "");
    }
}
