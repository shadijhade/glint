//! Marks: a label, tags and a comment that stay with the file (docs/gpui/marks/PLAN.md). Pure: no disk, no
//! clock. glint-core reads and writes the three homes (a file's own streams, `.glint-marks` beside it, the
//! ledger on this PC); this is what they hold and what is done with it.
//!
//! - [`labels`]: the named colors a label is chosen from ([`Label`]), and what a row draws ([`shown`]).
//! - [`lines`]: the text of `.glint-marks`, the ledger and an export ([`read_beside`], [`write_beside`]…).
//! - [`wanted`]: the filter of `label:`, `tag:`, `comment:` and `has:` ([`Wanted`], [`KEYS`]).
//! - [`xyplorer`]: XYplorer's `tag.dat`, read and written ([`XyFile`]).
//!
//! The S0 contracts: the bodies that do nothing are filled by the wave's cards (PLAN.md section 9).

pub mod labels;
pub mod lines;
pub mod wanted;
pub mod xyplorer;

pub use labels::{default_labels, label_of, settled, shown, Label, Shown};
pub use lines::{
    clip_marks, clip_text, read_beside, read_export, read_ledger, write_beside, write_export, write_ledger, Beside,
    Export, Line, Why,
};
pub use wanted::{Has, Terms, Wanted, KEYS};
pub use xyplorer::{XyExtra, XyFile, XyItem, XyKind, XyLabel};

use serde::{Deserialize, Serialize};

/// The streams marks live in, after the file's name: `C:\notes.txt:glint.label`. `glint.tags` is 0.16's and keeps
/// its name and format (M8).
pub const STREAM_TAGS: &str = "glint.tags";
/// One line: the label's name.
pub const STREAM_LABEL: &str = "glint.label";
/// The comment, UTF-8, lines ended by `\n`.
pub const STREAM_COMMENT: &str = "glint.comment";
/// More is never read of a label's stream (5.1).
pub const LABEL_CAP: usize = 256;
/// Nor of the tags' (0.16's cap).
pub const TAGS_CAP: usize = 4096;
/// Nor of a comment's; the index keeps its first 4 KB.
pub const COMMENT_CAP: usize = 64 * 1024;
/// The hidden file beside the files, on a drive without streams (5.2).
pub const BESIDE_NAME: &str = ".glint-marks";
/// No more labels than this (F1); an import's label past it comes in as a tag.
pub const MAX_LABELS: usize = 32;

/// What an item is marked with. Empty everywhere is no marks.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks {
    /// The label's name, as the file says it (it may be an old name of one: `Label::was`).
    pub label: Option<String>,
    /// One of each, in the order they were given.
    pub tags: Vec<String>,
    pub comment: Option<String>,
}

impl Marks {
    /// No label, no tag, no comment.
    pub fn is_empty(&self) -> bool {
        self.label.is_none() && self.tags.is_empty() && self.comment.is_none()
    }
}

/// A change asked of some items' marks (core's `marks_set`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The label, or none.
    Label(Option<String>),
    /// Tags added and taken off (any case).
    Tags { add: Vec<String>, remove: Vec<String> },
    /// The comment, or none.
    Comment(Option<String>),
    /// All three at once: undo, a file made in another's place, import, and nothing else.
    Put(Marks),
    /// What was copied from another item (Paste marks): its tags are added, and its label and its comment take
    /// the place of the ones here, each only when it has one.
    Paste(Marks),
    /// No marks at all.
    Clear,
}

impl Change {
    /// What `marks` become with this change.
    pub fn apply(&self, marks: &Marks) -> Marks {
        let mut m = marks.clone();
        match self {
            Change::Label(l) => m.label = l.clone(),
            Change::Tags { add, remove } => {
                m.tags.retain(|t| !remove.iter().any(|r| r.to_lowercase() == t.to_lowercase()));
                for a in add {
                    if !m.tags.iter().any(|t| t.to_lowercase() == a.to_lowercase()) {
                        m.tags.push(a.clone());
                    }
                }
            }
            Change::Comment(c) => m.comment = c.clone(),
            Change::Put(p) => m = p.clone(),
            Change::Paste(p) => {
                m = Change::Tags { add: p.tags.clone(), remove: Vec::new() }.apply(&m);
                m.label = p.label.clone().or(m.label);
                m.comment = p.comment.clone().or(m.comment);
            }
            Change::Clear => m = Marks::default(),
        }
        tidy(&m)
    }
}

/// What a row or a tile holds of a comment, for its tooltip and for a screen reader: all of a short one, the
/// first kilobyte of a long one (cut on a character). The whole of it is in Edit comment… and in the preview. A
/// row is made again every frame it is on screen, and a comment can be 64 KB.
pub fn comment_brief(comment: &str) -> std::borrow::Cow<'_, str> {
    const MOST: usize = 1024;
    if comment.len() <= MOST {
        return comment.into();
    }
    let cut = (0..=MOST).rev().find(|&at| comment.is_char_boundary(at)).unwrap_or(0);
    format!("{}…", &comment[..cut]).into()
}

/// What any marks become before they are kept: one-line label (cut to `LABEL_CAP`), trimmed tags without
/// doubles (any case), a comment with `\n` line ends and no NULs. A comment is never cut here: core refuses
/// one over `COMMENT_CAP` with words.
pub fn tidy(marks: &Marks) -> Marks {
    fn line(s: &str) -> String {
        s.replace(['\r', '\n'], " ").trim().to_string()
    }
    let label = marks.label.as_deref().map(line).map(|mut l| {
        if l.len() > LABEL_CAP {
            let mut end = LABEL_CAP;
            while !l.is_char_boundary(end) {
                end -= 1;
            }
            l.truncate(end);
            l = l.trim().to_string();
        }
        l
    });
    let mut tags: Vec<String> = Vec::new();
    for t in marks.tags.iter().map(|t| line(t)).filter(|t| !t.is_empty()) {
        if !tags.iter().any(|x| x.to_lowercase() == t.to_lowercase()) {
            tags.push(t);
        }
    }
    let comment = marks
        .comment
        .as_deref()
        .map(|c| c.replace("\r\n", "\n").replace('\r', "\n").replace('\0', "").trim_end().to_string());
    Marks { label: label.filter(|l| !l.is_empty()), tags, comment: comment.filter(|c| !c.trim().is_empty()) }
}

/// Where an item's marks are kept (5.4): its own streams, the folder's `.glint-marks`, or the ledger on this PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Home {
    Stream,
    Beside,
    Ledger,
}

/// What a change did to one item: its marks before, and after with where they are now, or why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub path: String,
    pub before: Marks,
    pub after: Result<(Marks, Home), String>,
}

/// How an import meets the marks an item already has in Glint (7.3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Meet {
    /// The label if it has none, the tags added, the comment if it has none.
    #[default]
    Fill,
    /// The source's replace Glint's, all three.
    Replace,
}

/// What an item gets when `incoming` meets what it `has` (7.3), and whether a different comment of Glint's was
/// kept (the report says so).
pub fn meet(has: &Marks, incoming: &Marks, how: Meet) -> (Marks, bool) {
    let (has, incoming) = (tidy(has), tidy(incoming));
    match how {
        Meet::Replace => (incoming, false),
        Meet::Fill => {
            let mut tags = has.tags.clone();
            for t in &incoming.tags {
                if !tags.iter().any(|x| x.to_lowercase() == t.to_lowercase()) {
                    tags.push(t.clone());
                }
            }
            let differ = matches!((&has.comment, &incoming.comment), (Some(a), Some(b)) if a != b);
            let out = Marks {
                label: has.label.or(incoming.label),
                tags,
                comment: has.comment.or(incoming.comment),
            };
            (out, differ)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(label: Option<&str>, tags: &[&str], comment: Option<&str>) -> Marks {
        Marks {
            label: label.map(String::from),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment: comment.map(String::from),
        }
    }

    #[test]
    fn tidy_makes_a_label_one_trimmed_line() {
        assert_eq!(tidy(&m(Some("  In\nreview "), &[], None)).label.as_deref(), Some("In review"));
        assert_eq!(tidy(&m(Some(" \n "), &[], None)).label, None);
    }

    #[test]
    fn tidy_cuts_a_long_label_on_a_character_boundary() {
        let l = tidy(&m(Some(&"\u{e9}".repeat(300)), &[], None)).label.unwrap_or_default();
        assert!(l.len() <= LABEL_CAP && !l.is_empty());
        assert!(l.chars().all(|c| c == '\u{e9}'));
    }

    #[test]
    fn tidy_keeps_one_trimmed_tag_of_each_in_any_case() {
        let t = tidy(&m(None, &[" a ", "", "A", "b\nc"], None));
        assert_eq!(t.tags, vec!["a", "b c"]);
    }

    #[test]
    fn tidy_ends_a_comments_lines_with_newlines_and_trims_its_end() {
        assert_eq!(tidy(&m(None, &[], Some("one\r\ntwo\r  \n\n"))).comment.as_deref(), Some("one\ntwo"));
        assert_eq!(tidy(&m(None, &[], Some("a\0b"))).comment.as_deref(), Some("ab"));
    }

    #[test]
    fn tidy_turns_a_comment_of_spaces_into_none() {
        assert_eq!(tidy(&m(None, &[], Some("  \n "))).comment, None);
    }

    #[test]
    fn tidy_never_cuts_a_comment() {
        let c = "x".repeat(70_000);
        assert_eq!(tidy(&m(None, &[], Some(&c))).comment.map(|c| c.len()), Some(70_000));
    }

    #[test]
    fn tidying_twice_changes_nothing() {
        let a = tidy(&m(Some(" x\ny "), &["A", "a", " b"], Some("c\r\nd \n")));
        assert_eq!(tidy(&a), a);
    }

    fn full() -> Marks {
        m(Some("Red"), &["Work", "old"], Some("note"))
    }

    #[test]
    fn a_change_sets_or_takes_off_the_label() {
        assert_eq!(Change::Label(Some("Blue".into())).apply(&full()).label.as_deref(), Some("Blue"));
        let none = Change::Label(None).apply(&full());
        assert_eq!((none.label, none.tags, none.comment), (None, full().tags, full().comment));
    }

    #[test]
    fn a_change_adds_and_takes_off_tags_in_any_case() {
        let c = Change::Tags { add: vec!["New".into(), "work".into()], remove: vec!["OLD".into()] };
        assert_eq!(c.apply(&full()).tags, vec!["Work", "New"]);
        assert_eq!(c.apply(&full()).label.as_deref(), Some("Red"));
    }

    #[test]
    fn a_change_sets_or_takes_off_the_comment() {
        assert_eq!(Change::Comment(Some("hi".into())).apply(&full()).comment.as_deref(), Some("hi"));
        let none = Change::Comment(None).apply(&full());
        assert_eq!((none.comment, none.label), (None, full().label));
    }

    #[test]
    fn put_and_label_tidy_what_they_are_given() {
        let put = Change::Put(m(Some("a\nb"), &["x", "X"], None)).apply(&full());
        assert_eq!(put, m(Some("a b"), &["x"], None));
        assert_eq!(Change::Label(Some("a\nb".into())).apply(&full()).label.as_deref(), Some("a b"));
    }

    #[test]
    fn paste_adds_the_tags_and_replaces_a_label_and_a_comment_only_with_one() {
        let all = Change::Paste(m(Some("Blue"), &["New", "work"], Some("hi"))).apply(&full());
        assert_eq!(all, m(Some("Blue"), &["Work", "old", "New"], Some("hi")));
        let tags_only = Change::Paste(m(None, &["New"], None)).apply(&full());
        assert_eq!(tags_only, m(Some("Red"), &["Work", "old", "New"], Some("note")));
    }

    #[test]
    fn a_long_comment_is_brief_in_a_row_and_a_short_one_is_whole() {
        assert_eq!(comment_brief("hi\nthere"), "hi\nthere");
        let long = "é".repeat(5000);
        let brief = comment_brief(&long);
        assert!(brief.len() <= 1024 + '…'.len_utf8() && brief.ends_with('…'));
        assert!(brief.trim_end_matches('…').chars().all(|c| c == 'é'));
    }

    #[test]
    fn clear_leaves_no_marks() {
        assert!(Change::Clear.apply(&full()).is_empty());
    }

    #[test]
    fn fill_takes_a_label_only_when_there_is_none() {
        let inc = m(Some("Blue"), &[], None);
        assert_eq!(meet(&m(None, &[], None), &inc, Meet::Fill).0.label.as_deref(), Some("Blue"));
        assert_eq!(meet(&m(Some("Red"), &[], None), &inc, Meet::Fill).0.label.as_deref(), Some("Red"));
    }

    #[test]
    fn fill_adds_tags_without_doubles_in_any_case() {
        let (out, _) = meet(&m(None, &["Work"], None), &m(None, &["work", "new"], None), Meet::Fill);
        assert_eq!(out.tags, vec!["Work", "new"]);
    }

    #[test]
    fn fill_takes_a_comment_only_when_there_is_none() {
        let inc = m(None, &[], Some("theirs"));
        assert_eq!(meet(&Marks::default(), &inc, Meet::Fill), (inc.clone(), false));
        assert_eq!(meet(&m(None, &[], Some("mine")), &inc, Meet::Fill).0.comment.as_deref(), Some("mine"));
    }

    #[test]
    fn fill_says_so_when_two_different_comments_met() {
        let (out, differ) = meet(&m(None, &[], Some("mine")), &m(None, &[], Some("theirs")), Meet::Fill);
        assert!(differ && out.comment.as_deref() == Some("mine"));
        let (_, same) = meet(&m(None, &[], Some("a")), &m(None, &[], Some("a")), Meet::Fill);
        assert!(!same);
    }

    #[test]
    fn replace_gives_the_incoming_marks_even_when_empty() {
        assert_eq!(meet(&full(), &Marks::default(), Meet::Replace), (Marks::default(), false));
        let inc = m(Some("Blue"), &["z"], None);
        assert_eq!(meet(&full(), &inc, Meet::Replace), (inc, false));
    }
}
