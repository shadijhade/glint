//! The search box's words and rules (`FILTERS`, `onFilterChange` and friends in AddressBar.tsx), without any
//! window: which filters it offers, what counts as a finished query, and what to do with typed text.

use glint_model::entries::terms;
use glint_model::js::is_js_space;

/// A filter the box offers when it is empty: what choosing one types, an example, and what it means.
pub(super) type Offer = (&'static str, &'static str, &'static str);

/// The search box's filters, as it offers them when empty.
pub(super) const FILTERS: [Offer; 9] = [
    ("ext:", "ext:pdf", "of a type (ext:jpg,png for more)"),
    ("tag:", "tag:work", "with that tag (tag:red;blue for either)"),
    (
        "path:",
        "path:src\\lib",
        "matching folder or file path (or type \\ or /)",
    ),
    ("size:>", "size:>10mb", "bigger than (size:<1kb smaller)"),
    ("dm:", "dm:7d", "changed in the last 7 days (dm:>1y before)"),
    ("file:", "file:", "files only"),
    ("folder:", "folder:", "folders only"),
    (
        "shows:",
        "shows:dog on a beach",
        "photos showing it (photo search, on in Settings)",
    ),
    (
        "says:",
        "says:budget meeting",
        "recordings and videos saying it (speech search, on in Settings)",
    ),
];

/// What photos, videos and songs say of themselves (media.rs reads it): as you type.
pub(super) const PROP_FILTERS: [Offer; 5] = [
    (
        "width:>",
        "width:>1920",
        "wider than (height:<1080 too), photos and videos",
    ),
    (
        "taken:",
        "taken:2024",
        "taken in 2024 (taken:2024-06, taken:>2023, taken:7d)",
    ),
    ("camera:", "camera:pixel", "taken with that camera"),
    (
        "duration:>",
        "duration:>5m",
        "longer than 5 minutes (1:30, 90s), videos and songs",
    ),
    ("artist:", "artist:beatles", "songs by (album: too)"),
];

/// Offered too in a Git repository: it filters what's on screen.
pub(super) const GIT_FILTER: Offer = ("git:", "git:changed", "changed in Git (git:new, git:staged…), here");

/// Whether a search holds a `git:` filter, which works on the folder shown only (`hasGitFilter`).
pub(super) fn has_git_filter(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.match_indices("git:").any(|(at, _)| {
        lower
            .get(..at)
            .and_then(|before| before.chars().next_back())
            .is_none_or(is_js_space)
    })
}

/// A filter prefix whose value hasn't been typed yet ("ext:", "size:>"): `INCOMPLETE_FILTER`.
pub(super) fn is_incomplete_filter(term: &str) -> bool {
    let Some((key, value)) = term.split_once(':') else {
        return false;
    };
    const KEYS: [&str; 9] = [
        "ext", "size", "dm", "width", "height", "duration", "taken", "camera", "artist",
    ];
    let known = KEYS.iter().any(|k| key.eq_ignore_ascii_case(k))
        || key.eq_ignore_ascii_case("album")
        || key.eq_ignore_ascii_case("path");
    known && value.chars().all(|c| matches!(c, '<' | '>' | '='))
}

/// Whether the text starts with a drive letter and a separator ("C:\", "d:/"): a path typed in the box.
pub(super) fn starts_with_drive(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// The query to search as you type: the finished terms, the ones with spaces quoted again. Empty when none is
/// finished yet.
pub(super) fn ready_query(trimmed: &str) -> String {
    terms(trimmed)
        .into_iter()
        .filter(|t| !is_incomplete_filter(t))
        .map(|t| {
            if t.chars().any(is_js_space) {
                format!("\"{t}\"")
            } else {
                t
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_filters_are_found_only_as_words_of_their_own() {
        assert!(has_git_filter("git:changed"));
        assert!(has_git_filter("notes GIT:new"));
        assert!(!has_git_filter("digit:3"));
        assert!(!has_git_filter("legit"));
        assert!(has_git_filter("a\u{a0}git:staged"));
    }

    #[test]
    fn a_filter_without_its_value_is_not_finished() {
        for unfinished in ["ext:", "size:>", "dm:<=", "PATH:", "taken:", "album:"] {
            assert!(is_incomplete_filter(unfinished), "{unfinished}");
        }
        for finished in ["ext:pdf", "size:>1mb", "file:", "folder:", "report", "tag:"] {
            assert!(!is_incomplete_filter(finished), "{finished}");
        }
    }

    #[test]
    fn the_query_run_as_you_type_drops_unfinished_filters_and_keeps_quotes() {
        assert_eq!(ready_query("ext: report"), "report");
        assert_eq!(ready_query("ext:pdf size:>"), "ext:pdf");
        assert_eq!(ready_query("\"two words\" ext:"), "\"two words\"");
        assert_eq!(ready_query("size:>"), "");
    }

    #[test]
    fn a_typed_drive_path_starts_with_a_letter_and_a_separator() {
        assert!(starts_with_drive("C:\\Users"));
        assert!(starts_with_drive("d:/x"));
        assert!(!starts_with_drive("C:"));
        assert!(!starts_with_drive("report"));
    }

    #[test]
    fn the_offers_are_the_typescripts() {
        assert_eq!(FILTERS.len(), 9);
        assert_eq!(PROP_FILTERS.len(), 5);
        assert!(FILTERS.iter().all(|(start, example, _)| example.starts_with(start)));
        assert!(PROP_FILTERS
            .iter()
            .all(|(start, example, _)| example.starts_with(start)));
    }
}
