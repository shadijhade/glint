//! The grid's keyboard: which key moves where, and type-ahead. Ported from the key handler in Explorer.tsx (the part
//! the list and the grid share) and `gridMove` (glint_model::grid, which does the arithmetic).
//!
//! Pure functions, so they are tested without a window.

use crate::components::file_list::INVALID_NAME_CHARS;
use crate::explorer::Rows;
use crate::kit::prelude::Keystroke;
use std::time::{Duration, Instant};

/// GPUI's key names for the keys that move the cursor, as the DOM named them (`gridMove` speaks the DOM's:
/// "ArrowLeft", "PageUp"…). Left and Right swap in a right-to-left interface, where the next tile is on the
/// left: the TypeScript let Right always mean the next item, which in Arabic points the wrong way.
pub fn move_key(key: &str, rtl: bool) -> Option<&'static str> {
    Some(match (key, rtl) {
        ("left", false) | ("right", true) => "ArrowLeft",
        ("right", false) | ("left", true) => "ArrowRight",
        ("up", _) => "ArrowUp",
        ("down", _) => "ArrowDown",
        ("pageup", _) => "PageUp",
        ("pagedown", _) => "PageDown",
        ("home", _) => "Home",
        ("end", _) => "End",
        _ => return None,
    })
}

/// Whether the rename box refuses this key: a character no Windows name can hold (`\ / : * ? " < > |`; with
/// Ctrl it is a shortcut, not a character).
pub fn refuses_in_name(keystroke: &Keystroke) -> bool {
    let mut chars = keystroke.key_char.as_deref().unwrap_or("").chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => INVALID_NAME_CHARS.contains(&c) && !keystroke.modifiers.control,
        _ => false,
    }
}

/// A pause longer than this starts the typed text over.
const TYPE_AHEAD_PAUSE: Duration = Duration::from_millis(900);

/// Type-ahead: the text typed so far and when (`typeahead` in Explorer.tsx).
#[derive(Default)]
pub struct TypeAhead {
    text: String,
    at: Option<Instant>,
}

impl TypeAhead {
    /// The next item (after row `at`, or -1 for none) whose lowercase name starts with what has been typed, going
    /// round the end: one letter repeated cycles through the names starting with it. `typed` is one character, as
    /// the keyboard produced it.
    pub fn find(&mut self, typed: &str, now: Instant, at: i64, rows: &Rows) -> Option<usize> {
        let key = typed.to_lowercase();
        let paused = self.at.is_none_or(|last| now.duration_since(last) > TYPE_AHEAD_PAUSE);
        if paused {
            self.text = key.clone();
        } else {
            self.text.push_str(&key);
        }
        self.at = Some(now);
        let repeated = self
            .text
            .chars()
            .all(|c| key.starts_with(c) && key.chars().count() == 1);
        let needle = if repeated { key.as_str() } else { self.text.as_str() };
        let count = rows.len();
        if count == 0 {
            return None;
        }
        let start = if needle.chars().count() == 1 { at + 1 } else { at.max(0) };
        let count_i = i64::try_from(count).ok()?;
        (0..count_i)
            .map(|k| usize::try_from((start + k).rem_euclid(count_i)).unwrap_or(0))
            .find(|&i| rows.get(i).is_some_and(|entry| entry.lower.starts_with(needle)))
    }

    /// Forgets what was typed (the folder changed).
    pub fn reset(&mut self) {
        self.text.clear();
        self.at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::file;
    use glint_model::entries::FileEntry;
    use std::sync::Arc;

    fn rows(names: &[&str]) -> Rows {
        let entries: Vec<FileEntry> = names.iter().map(|n| file("C:\\x", n, 1, 1)).collect();
        Rows::identity(Arc::from(entries))
    }

    #[test]
    fn arrows_mirror_in_a_right_to_left_interface_and_the_rest_do_not() {
        assert_eq!(move_key("right", false), Some("ArrowRight"));
        assert_eq!(move_key("left", false), Some("ArrowLeft"));
        assert_eq!(move_key("right", true), Some("ArrowLeft"));
        assert_eq!(move_key("left", true), Some("ArrowRight"));
        for rtl in [false, true] {
            assert_eq!(move_key("down", rtl), Some("ArrowDown"));
            assert_eq!(move_key("pageup", rtl), Some("PageUp"));
            assert_eq!(move_key("home", rtl), Some("Home"));
            assert_eq!(move_key("enter", rtl), None);
            assert_eq!(move_key("a", rtl), None);
        }
    }

    fn key(text: &'static str) -> Keystroke {
        Keystroke::parse(text).expect("a keystroke written in the tests")
    }

    #[test]
    fn the_rename_box_refuses_what_no_name_can_hold() {
        for refused in ["?", "*", "<", ">", "|", "\\", "/", ":", "\""] {
            let mut stroke = key(refused);
            if stroke.key_char.is_none() && refused.len() == 1 {
                stroke.key_char = Some(refused.to_string());
            }
            assert!(refuses_in_name(&stroke), "{refused}");
        }
        for allowed in ["a", "enter", "escape", "left", "ctrl-a", "shift-a"] {
            assert!(!refuses_in_name(&key(allowed)), "{allowed}");
        }
        // With Ctrl it is a shortcut (Ctrl+/ is not a typed slash).
        let mut shortcut = key("ctrl-/");
        shortcut.key_char = Some("/".to_string());
        assert!(!refuses_in_name(&shortcut));
    }

    #[test]
    fn typing_jumps_to_the_next_name_that_starts_with_it() {
        let rows = rows(&["alpha", "apple", "banana", "avocado"]);
        let mut ahead = TypeAhead::default();
        let t0 = Instant::now();
        // From nothing, "a" goes to the first.
        assert_eq!(ahead.find("a", t0, -1, &rows), Some(0));
        // "a" again within the pause cycles to the next a-name, round the end.
        assert_eq!(ahead.find("a", t0, 0, &rows), Some(1));
        assert_eq!(ahead.find("a", t0, 1, &rows), Some(3));
        assert_eq!(ahead.find("a", t0, 3, &rows), Some(0));
        // A different letter makes it a word: "av" finds avocado, starting from the current item.
        ahead.reset();
        assert_eq!(ahead.find("a", t0, -1, &rows), Some(0));
        assert_eq!(ahead.find("v", t0, 0, &rows), Some(3));
        // Nothing starts with "avx".
        assert_eq!(ahead.find("x", t0, 3, &rows), None);
    }

    #[test]
    fn a_pause_starts_over_and_case_does_not_matter() {
        let rows = rows(&["Alpha", "banana"]);
        let mut ahead = TypeAhead::default();
        let t0 = Instant::now();
        assert_eq!(ahead.find("B", t0, -1, &rows), Some(1));
        // A second later (past the pause) "a" is a new word, not "ba".
        assert_eq!(ahead.find("a", t0 + Duration::from_millis(1000), 1, &rows), Some(0));
        ahead.reset();
        assert_eq!(ahead.find("a", t0, -1, &rows), Some(0));
    }

    #[test]
    fn arabic_names_are_found_by_their_first_letter() {
        let rows = rows(&["تقرير.pdf", "صلاة.mp3", "Report.txt"]);
        let mut ahead = TypeAhead::default();
        assert_eq!(ahead.find("ص", Instant::now(), -1, &rows), Some(1));
    }

    #[test]
    fn no_rows_find_nothing() {
        let mut ahead = TypeAhead::default();
        assert_eq!(ahead.find("a", Instant::now(), -1, &Rows::empty()), None);
    }
}
