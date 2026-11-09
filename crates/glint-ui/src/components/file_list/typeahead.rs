//! Type-ahead: letters typed in the list jump to the next name that starts with them.
//!
//! Ported from the last branch of the list's `onKey` in Explorer.tsx. What was typed is kept for 900 ms; one
//! letter pressed again and again cycles through the names that start with it; several different letters
//! search from the focused item itself. Pure, so the timing and the search are tested without a window.

use std::time::{Duration, Instant};

/// How long what was typed counts as one word (`now - t.at > 900`).
const KEEP: Duration = Duration::from_millis(900);

/// What has been typed lately.
#[derive(Debug, Default)]
pub struct TypeAhead {
    text: String,
    at: Option<Instant>,
}

impl TypeAhead {
    /// A letter typed at `now`: the index of the row to jump to, if any name starts with what's typed.
    /// `key` is the character lowercased, `focus` the focused row (None when nothing is), and `lower` the
    /// lowercased name of row `i` (None past the end), for `count` rows.
    pub fn press<'a>(
        &mut self,
        key: &str,
        now: Instant,
        focus: Option<usize>,
        count: usize,
        lower: impl Fn(usize) -> Option<&'a str>,
    ) -> Option<usize> {
        let expired = self.at.is_none_or(|at| now.saturating_duration_since(at) > KEEP);
        if expired {
            self.text = key.to_string();
        } else {
            self.text.push_str(key);
        }
        self.at = Some(now);
        // The same letter repeated is a cycle through that letter's names.
        let needle = if self.text.chars().all(|c| c.to_string() == key) {
            key
        } else {
            self.text.as_str()
        };
        if count == 0 {
            return None;
        }
        let start = if needle.chars().count() == 1 {
            focus.map_or(0, |at| at + 1)
        } else {
            focus.unwrap_or(0)
        };
        (0..count)
            .map(|k| (start + k) % count)
            .find(|&i| lower(i).is_some_and(|name| name.starts_with(needle)))
    }

    /// Forgets what was typed (another folder is shown).
    pub fn clear(&mut self) {
        self.text.clear();
        self.at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 6] = ["alpha", "apple", "banana", "berry", "cherry", "apricot"];

    fn name(i: usize) -> Option<&'static str> {
        NAMES.get(i).copied()
    }

    #[test]
    fn a_letter_jumps_to_the_next_name_that_starts_with_it() {
        let mut t = TypeAhead::default();
        let now = Instant::now();
        // From nothing: the first match.
        assert_eq!(t.press("b", now, None, NAMES.len(), name), Some(2));
        // Typed again after a pause, from "banana": the next one.
        let later = now + Duration::from_secs(2);
        assert_eq!(t.press("b", later, Some(2), NAMES.len(), name), Some(3));
    }

    #[test]
    fn the_same_letter_pressed_again_cycles_and_wraps() {
        let mut t = TypeAhead::default();
        let now = Instant::now();
        assert_eq!(t.press("a", now, Some(0), NAMES.len(), name), Some(1));
        // Quickly again: the text is "aa", all the same letter, so it still cycles from the focus.
        assert_eq!(
            t.press("a", now + Duration::from_millis(100), Some(1), NAMES.len(), name),
            Some(5)
        );
        // Wraps round to the first.
        assert_eq!(
            t.press("a", now + Duration::from_millis(200), Some(5), NAMES.len(), name),
            Some(0)
        );
    }

    #[test]
    fn several_letters_search_from_the_focused_item_itself() {
        let mut t = TypeAhead::default();
        let now = Instant::now();
        assert_eq!(t.press("a", now, None, NAMES.len(), name), Some(0));
        // "ap" from "alpha" (index 0): "apple" is next, and it is also found from itself.
        assert_eq!(
            t.press("p", now + Duration::from_millis(300), Some(0), NAMES.len(), name),
            Some(1)
        );
        assert_eq!(
            t.press("r", now + Duration::from_millis(500), Some(1), NAMES.len(), name),
            Some(5),
            "ap + r: apricot"
        );
    }

    #[test]
    fn nothing_matches_and_an_empty_list_both_give_none() {
        let mut t = TypeAhead::default();
        let now = Instant::now();
        assert_eq!(t.press("z", now, Some(1), NAMES.len(), name), None);
        assert_eq!(t.press("a", now + Duration::from_secs(5), None, 0, name), None);
    }

    #[test]
    fn what_was_typed_is_forgotten_after_900_ms() {
        let mut t = TypeAhead::default();
        let now = Instant::now();
        t.press("c", now, None, NAMES.len(), name);
        // 901 ms later "b" starts a new word rather than making "cb".
        assert_eq!(
            t.press("b", now + Duration::from_millis(901), Some(4), NAMES.len(), name),
            Some(2)
        );
    }

    #[test]
    fn arabic_names_match_by_their_first_letters() {
        let names = ["تقرير", "ثقافة", "صورة"];
        let mut t = TypeAhead::default();
        let at = |i: usize| names.get(i).copied();
        assert_eq!(t.press("ص", Instant::now(), None, names.len(), at), Some(2));
    }
}
