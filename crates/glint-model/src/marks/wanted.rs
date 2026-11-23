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
    pub fn parse(terms: &[String]) -> (Wanted, Vec<String>) {
        // S0: card A5 fills this in.
        (Wanted::default(), terms.to_vec())
    }

    /// Nothing asked about marks.
    pub fn is_empty(&self) -> bool {
        *self == Wanted::default()
    }

    /// Whether an item with `marks` is wanted.
    pub fn accepts(&self, marks: &Marks) -> bool {
        // S0: card A5 fills this in.
        let _ = marks;
        true
    }
}
