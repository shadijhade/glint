//! *In comments*: a plain word in a search also finds the files whose comment or tags hold it (docs/gpui/marks, F15).
//! The index says which files it found that way (`Found::Marked`, a hit's two bits) and never sends a comment's
//! text; this reads the text, with the person's own rights, and finds the line to show under the row.
//!
//! # How it runs
//!
//! `Explorer::found` hands every `Found::Marked` to [`Flagged::note`], which says which of its paths to read: at
//! most [`MOST_READ`] a search, one `read_marks` call a batch (the tags hub's backend, on core's runtime: never the
//! UI thread). What is read comes back to `Explorer::flagged_read`, which puts each path's line in the run.
//!
//! # What a run holds
//!
//! `FindRun` has no field of its own for this, and what it carries to the rows is what `Inside files` uses, so a
//! file found by a comment is kept the way a file found by its contents is, plus a mark:
//!
//! - its path is in `inside.only` (found by something other than its name: the box's words are not asked of its
//!   name), and so is [`key`] of its path, which is what tells it from a file found inside;
//! - its path has an entry in `inside.hits`: no line until its marks are read (the row is there at once; M4: no
//!   placeholder), then the one line, which is a [`Hit`] with no number and no place.
//!
//! `rows.rs` puts the files with the mark under their own header, between *Names* and *Inside files*.

use super::model::Explorer;
use super::types::*;
use crate::{core_bridge, t};
use glint_model::marks::Marks;
use glint_model::text::{self, RgMatch, Submatch};
use gpui_kit::Context;
use std::collections::HashSet;
use std::sync::Arc;

/// The most files whose marks one search reads: the rest stay under their header with no line.
pub const MOST_READ: usize = 2_000;

/// A hit found by its comment (the index's `HIT_BY_COMMENT`).
pub const BY_COMMENT: u8 = 2;
/// A hit found by a tag (the index's `HIT_BY_TAG`).
pub const BY_TAG: u8 = 4;

/// What marks a path in `inside.only` as found by a comment or a tag, apart from its own entry there.
pub fn key(path: &str) -> String {
    format!("\u{1}{path}")
}

/// The paths the index said were found by a comment or a tag, in one search, and how many were asked for.
#[derive(Default)]
pub(super) struct Flagged {
    paths: HashSet<String>,
    asked: usize,
}

impl Flagged {
    /// Takes a batch of what the index flagged; gives back what is to be read now (each once, up to [`MOST_READ`] in
    /// all a search).
    pub fn note(&mut self, batch: Vec<(String, u8)>) -> Vec<(String, u8)> {
        let mut read = Vec::new();
        for (path, by) in batch {
            if by & (BY_COMMENT | BY_TAG) == 0 || !self.paths.insert(path.clone()) {
                continue;
            }
            if self.asked < MOST_READ {
                self.asked += 1;
                read.push((path, by));
            }
        }
        read
    }

    /// Whether the index flagged `path`.
    pub fn has(&self, path: &str) -> bool {
        self.paths.contains(path)
    }
}

/// The words of a search, lower case, as they are looked for in a comment and in tags.
fn words_of(phrase: &str) -> Vec<String> {
    phrase.split_whitespace().map(str::to_lowercase).collect()
}

/// A line as a row shows it, with the words in it marked (any case). A line whose lower case has other lengths
/// (a few letters change size) is shown plain.
fn hit_of(line: &str, words: &[String]) -> Hit {
    let lower = line.to_lowercase();
    let mut spots: Vec<(usize, usize)> = Vec::new();
    if lower.len() == line.len() {
        for word in words.iter().filter(|w| !w.is_empty()) {
            spots.extend(lower.match_indices(word.as_str()).map(|(at, found)| (at, at + found.len())));
        }
    }
    spots.sort_unstable();
    let mut submatches: Vec<Submatch> = Vec::new();
    for (start, end) in spots {
        match submatches.last_mut() {
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => submatches.push(Submatch { start, end }),
        }
    }
    let found = RgMatch {
        lines_text: Some(line.to_string()),
        submatches,
    };
    Hit {
        line: None,
        label: None,
        parts: text::segments(&found),
    }
}

/// The line to show under a file found by its marks: for a comment, the first of its lines that holds one of the
/// words; for tags, `tags: a, b`. Whichever the index found it by comes first. `None` when its marks no longer
/// hold any of the words.
pub fn line_of(marks: &Marks, by: u8, phrase: &str) -> Option<Hit> {
    let words = words_of(phrase);
    let holds = |text: &str| {
        let lower = text.to_lowercase();
        words.iter().any(|word| !word.is_empty() && lower.contains(word.as_str()))
    };
    let comment = || {
        let comment = marks.comment.as_deref()?;
        // A line is laid out whole: a long one is cut as the comment's other briefs are.
        comment.lines().find(|line| holds(line)).map(|line| hit_of(&glint_model::marks::comment_brief(line.trim()), &words))
    };
    let tags = || {
        let any = marks.tags.iter().any(|tag| holds(tag));
        any.then(|| hit_of(&t!("tags: {list}", list = marks.tags.join(", ")), &words))
    };
    if by & BY_COMMENT != 0 {
        comment().or_else(tags)
    } else {
        tags().or_else(comment)
    }
}

/// `added` (files that came in this batch and that the index flagged) are put under *In comments*, with no line
/// yet.
pub(super) fn mark_rows(run: &mut FindRun, added: Vec<String>) {
    if added.is_empty() {
        return;
    }
    let mut only = (*run.inside.only).clone();
    let mut hits = (*run.inside.hits).clone();
    for path in added {
        only.insert(key(&path));
        only.insert(path.clone());
        hits.entry(path).or_insert_with(|| Arc::from(Vec::new()));
    }
    run.inside.only = Arc::new(only);
    run.inside.hits = Arc::new(hits);
}

impl Explorer {
    /// Reads the marks of `asked` (the files a batch of the search was flagged for) and, when they come, puts each
    /// one's line in the tab's search `id`. Off the UI thread; one call.
    pub(super) fn read_flagged(
        &mut self,
        tab: TabId,
        id: u64,
        phrase: String,
        asked: Vec<(String, u8)>,
        cx: &mut Context<Self>,
    ) {
        if asked.is_empty() || phrase.is_empty() {
            return;
        }
        let hub = crate::dialogs::tags::hub(cx);
        let backend = hub.read(cx).backend();
        let paths: Vec<String> = asked.iter().map(|(path, _)| path.clone()).collect();
        cx.spawn(async move |this, cx| {
            let read = core_bridge::run(backend.read_marks(paths)).await.and_then(|read| read);
            this.update(cx, |this, cx| this.flagged_read(tab, id, &phrase, &asked, read, cx))
                .ok();
        })
        .detach();
    }

    /// What reading the marks of a batch said. A read that failed leaves the rows without lines; so does a search
    /// that is no longer the tab's.
    fn flagged_read(
        &mut self,
        tab: TabId,
        id: u64,
        phrase: &str,
        asked: &[(String, u8)],
        read: Result<Vec<Marks>, String>,
        cx: &mut Context<Self>,
    ) {
        let Ok(read) = read else {
            return;
        };
        let Some(mut run) = self
            .tabs
            .get(&tab)
            .and_then(|t| t.find.as_ref())
            .filter(|f| f.id == id)
            .cloned()
        else {
            return;
        };
        let mut hits = (*run.inside.hits).clone();
        let mut lines = 0;
        for ((path, by), marks) in asked.iter().zip(read.iter()) {
            let Some(hit) = line_of(marks, *by, phrase) else {
                continue;
            };
            // Before the lines a search of the contents may have put under the same file.
            let mut all = vec![hit];
            if let Some(before) = hits.get(path) {
                all.extend(before.iter().cloned());
            }
            hits.insert(path.clone(), all.into());
            lines += 1;
        }
        if lines == 0 {
            return;
        }
        run.inside.hits = Arc::new(hits);
        self.update_tab(tab, |t| t.find = Some(run));
        self.rebuild_rows(tab, cx);
        cx.emit(ExplorerEvent::TabChanged(tab));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(tags: &[&str], comment: Option<&str>) -> Marks {
        Marks {
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment: comment.map(str::to_string),
            ..Marks::default()
        }
    }

    fn text_of(hit: &Hit) -> String {
        hit.parts.iter().map(|p| p.text.as_str()).collect()
    }

    #[test]
    fn a_comments_line_is_the_first_that_holds_a_word_in_any_case_with_the_word_marked() {
        let m = marks(&[], Some("bond over two NICs\nLACP rate fast\nlacp again"));
        let hit = line_of(&m, BY_COMMENT, "lacp").expect("a line");
        assert_eq!(text_of(&hit), "LACP rate fast");
        assert!(hit.line.is_none() && hit.label.is_none());
        let marked: Vec<&str> = hit.parts.iter().filter(|p| p.hit).map(|p| p.text.as_str()).collect();
        assert_eq!(marked, ["LACP"]);
        // Any one of several words.
        let hit = line_of(&m, BY_COMMENT, "nics zzz").expect("a line");
        assert_eq!(text_of(&hit), "bond over two NICs");
    }

    #[test]
    fn a_tag_alone_says_all_the_tags() {
        let m = marks(&["bond", "lacp"], None);
        let hit = line_of(&m, BY_TAG, "lacp").expect("a line");
        assert_eq!(text_of(&hit), "tags: bond, lacp");
    }

    #[test]
    fn marks_that_no_longer_hold_the_word_have_no_line_and_an_arabic_comment_has_one() {
        assert!(line_of(&marks(&["bond"], Some("something else")), BY_COMMENT | BY_TAG, "lacp").is_none());
        assert!(line_of(&Marks::default(), BY_COMMENT, "lacp").is_none());
        let m = marks(&[], Some("إعداد الشبكة\nتجميع الروابط بالبروتوكول lacp\nنهاية"));
        let hit = line_of(&m, BY_COMMENT, "الروابط").expect("a line");
        assert_eq!(text_of(&hit), "تجميع الروابط بالبروتوكول lacp");
        let marked: Vec<&str> = hit.parts.iter().filter(|p| p.hit).map(|p| p.text.as_str()).collect();
        assert_eq!(marked, ["الروابط"]);
    }

    #[test]
    fn only_flagged_paths_are_read_each_once_and_no_more_than_the_most() {
        let mut flagged = Flagged::default();
        let first = flagged.note(vec![("a".into(), BY_COMMENT), ("b".into(), 0), ("a".into(), BY_COMMENT), ("c".into(), BY_TAG)]);
        assert_eq!(first, [("a".to_string(), BY_COMMENT), ("c".to_string(), BY_TAG)]);
        assert!(flagged.has("a") && !flagged.has("b"));
        let many: Vec<(String, u8)> = (0..MOST_READ + 10).map(|i| (format!("f{i}"), BY_COMMENT)).collect();
        assert_eq!(flagged.note(many).len(), MOST_READ - 2);
        assert!(flagged.has(&format!("f{}", MOST_READ + 5)), "flagged, though not read");
    }
}
