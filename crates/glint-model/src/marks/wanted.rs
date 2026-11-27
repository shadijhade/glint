//! The marks filter of the open folder and fd's drives (F12): `label:important`, `label:red;orange`,
//! `tag:netplan`, `comment:lacp` (every word; quotes keep a phrase), `has:label|tag|comment|mark`, and `!` before
//! any of them for not. XYplorer's `lbl:`, `tags:` and `cmt:` are the same keys. A label answers to `tag:` too.
//!
//! The instant search service has its own copy (`shadui_index::query::MarksFilter`, PLAN.md 6.5): both run the
//! cases in `tests/fixtures/marks-filter.txt`, so they cannot drift.

use super::Marks;

/// The filter keys marks add, for the lists of keys the interface keeps (the search box, the sidebar…).
pub const KEYS: [&str; 7] = ["label", "lbl", "tag", "tags", "comment", "cmt", "has"];

/// What `has:` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Has {
    Label,
    Tag,
    Comment,
    /// Any of the three.
    Mark,
}

/// The terms of one side of a filter, every one of which must hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Terms {
    /// `label:`: groups, each of names any one of which will do (`label:red;orange`).
    pub labels: Vec<Vec<String>>,
    /// `tag:`: groups the same way; a label answers too.
    pub tags: Vec<Vec<String>>,
    /// `comment:`: words or quoted phrases, every one in the comment (any case).
    pub words: Vec<String>,
    pub has: Vec<Has>,
}

/// A marks filter: what must hold, and what must not (`!`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wanted {
    pub yes: Terms,
    pub no: Terms,
}

impl Wanted {
    /// The marks filter in `terms` (a search's words, as `glint_model::entries` splits them), and the terms that
    /// are not about marks.
    ///
    /// A term that doesn't read right (no value left, or a `has:` that is none of the four) matches nothing here, as
    /// every filter of `entries.rs` does (the service answers an error): it becomes an empty group of `yes.labels`,
    /// which no item satisfies.
    pub fn parse(terms: &[String]) -> (Wanted, Vec<String>) {
        let mut wanted = Wanted::default();
        let mut rest = Vec::new();
        for term in terms {
            let Some((key, value)) = term.split_once(':') else {
                rest.push(term.clone());
                continue;
            };
            let (not, key) = match key.strip_prefix('!') {
                Some(k) => (true, k),
                None => (false, key),
            };
            let key = key.to_ascii_lowercase();
            if !KEYS.contains(&key.as_str()) {
                rest.push(term.clone());
                continue;
            }
            let list = |split: &[char]| -> Vec<String> {
                value
                    .split(split)
                    .map(|n| n.trim().trim_matches('"').trim().to_lowercase())
                    .filter(|n| !n.is_empty())
                    .collect()
            };
            let side = if not { &mut wanted.no } else { &mut wanted.yes };
            let bad = match key.as_str() {
                "label" | "lbl" => {
                    let any = list(&[';']);
                    let bad = any.is_empty();
                    if !bad {
                        side.labels.push(any);
                    }
                    bad
                }
                "tag" | "tags" => {
                    let any = list(&[';', ',']);
                    let bad = any.is_empty();
                    if !bad {
                        side.tags.push(any);
                    }
                    bad
                }
                "comment" | "cmt" => {
                    let word = value.trim().to_lowercase();
                    let bad = word.is_empty();
                    if !bad {
                        side.words.push(word);
                    }
                    bad
                }
                _ => match value.trim().to_lowercase().as_str() {
                    "label" => {
                        side.has.push(Has::Label);
                        false
                    }
                    "tag" | "tags" => {
                        side.has.push(Has::Tag);
                        false
                    }
                    "comment" => {
                        side.has.push(Has::Comment);
                        false
                    }
                    "mark" | "marks" => {
                        side.has.push(Has::Mark);
                        false
                    }
                    _ => true,
                },
            };
            if bad {
                wanted.yes.labels.push(Vec::new());
            }
        }
        (wanted, rest)
    }

    /// Nothing asked about marks.
    pub fn is_empty(&self) -> bool {
        *self == Wanted::default()
    }

    /// Whether an item with `marks` is wanted.
    pub fn accepts(&self, marks: &Marks) -> bool {
        if self.is_empty() {
            return true;
        }
        let (yes, no) = (&self.yes, &self.no);
        let raw_label = marks.label.as_deref().unwrap_or("").trim();
        let raw_comment = marks.comment.as_deref().unwrap_or("");
        let has_label = !raw_label.is_empty();
        let has_tag = marks.tags.iter().any(|t| !t.trim().is_empty());
        let has_comment = !raw_comment.is_empty();
        let has = |h: &Has| match h {
            Has::Label => has_label,
            Has::Tag => has_tag,
            Has::Comment => has_comment,
            Has::Mark => has_label || has_tag || has_comment,
        };
        if !yes.has.iter().all(has) || no.has.iter().any(has) {
            return false;
        }
        let need_label = !yes.labels.is_empty() || !no.labels.is_empty() || !yes.tags.is_empty() || !no.tags.is_empty();
        let label = if need_label && has_label {
            raw_label.to_lowercase()
        } else {
            String::new()
        };
        let label_in = |group: &Vec<String>| !label.is_empty() && group.contains(&label);
        if !yes.labels.iter().all(label_in) || no.labels.iter().any(label_in) {
            return false;
        }
        if !yes.tags.is_empty() || !no.tags.is_empty() {
            let tags: Vec<String> = marks
                .tags
                .iter()
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .map(str::to_lowercase)
                .collect();
            let tag_in = |group: &Vec<String>| label_in(group) || tags.iter().any(|t| group.contains(t));
            if !yes.tags.iter().all(tag_in) || no.tags.iter().any(tag_in) {
                return false;
            }
        }
        if !yes.words.is_empty() || !no.words.is_empty() {
            let comment = if has_comment {
                raw_comment.to_lowercase()
            } else {
                String::new()
            };
            let word = |w: &String| comment.contains(w.as_str());
            if !yes.words.iter().all(word) || no.words.iter().any(word) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::terms;

    fn marks(label: &str, tags: &str, comment: &str) -> Marks {
        Marks {
            label: (!label.is_empty()).then(|| label.to_string()),
            tags: tags.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from).collect(),
            comment: (!comment.is_empty()).then(|| comment.replace("\\n", "\n")),
        }
    }

    fn parse(q: &str) -> (Wanted, Vec<String>) {
        Wanted::parse(&terms(q))
    }

    #[test]
    fn the_fixture_is_answered_as_the_service_answers_it() {
        let text = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks-filter.txt"));
        let mut ran = 0;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('|').map(str::trim).collect();
            assert_eq!(f.len(), 5, "line {}: five fields", n + 1);
            let (wanted, rest) = parse(f[0]);
            assert!(rest.is_empty(), "line {}: left {rest:?}", n + 1);
            assert_eq!(wanted.accepts(&marks(f[1], f[2], f[3])), f[4] == "yes", "line {}: {line}", n + 1);
            ran += 1;
        }
        assert!(ran >= 60, "only {ran} cases ran");
    }

    #[test]
    fn parse_leaves_the_terms_that_are_not_about_marks_untouched() {
        let (w, rest) = parse("!ext:pdf report !label:red");
        assert_eq!(rest, ["!ext:pdf", "report"]);
        assert_eq!(w.no.labels, [["red"]]);
        let (w, _) = parse("LABEL:Red");
        assert_eq!(w.yes.labels, [["red"]]);
        let (a, _) = parse("lbl:x tags:y cmt:z");
        let (b, _) = parse("label:x tag:y comment:z");
        assert_eq!(a, b);
    }

    #[test]
    fn a_term_that_does_not_read_right_matches_nothing() {
        for q in ["label:", "!comment:", "has:size", "tag:;"] {
            let (w, rest) = parse(q);
            assert!(rest.is_empty() && !w.is_empty(), "{q}");
            assert!(!w.accepts(&marks("Red", "a", "b")) && !w.accepts(&Marks::default()), "{q}");
        }
    }

    #[test]
    fn a_quoted_label_and_another_are_one_group_of_two() {
        let (w, _) = parse("label:\"in review\";red");
        assert_eq!(w.yes.labels, [["in review", "red"]]);
    }

    #[test]
    fn an_empty_filter_accepts_everything_and_odd_text_does_not_panic() {
        let big = "x".repeat(70_000);
        for m in [Marks::default(), marks("", "", &big), marks("مهم", "وسم", "تعليق"), marks(" ", " , ", " ")] {
            assert!(Wanted::default().accepts(&m));
            let (w, _) = parse("label:مهم tag:x comment:y has:mark !has:tag");
            let _ = w.accepts(&m);
        }
    }
}
