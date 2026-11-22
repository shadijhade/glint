//! The letters of hop mode's badges: which hint each target gets, and what typed letters leave of them.
//!
//! Hints are made from the home row first (`asdfghjkl`), then the rest of the alphabet. They are prefix-free (no hint
//! begins another, so a hint is acted on the moment its last letter is typed and never has to wait for a second
//! chance), as short as the number of targets allows, and decided by the order of the targets alone, so they stay put
//! while the overlay is open.

/// The letters hints are spelled with, in the order they are handed out: the home row, then the rest.
pub const ALPHABET: &str = "asdfghjklqwertyuiopzxcvbnm";

/// `count` hints, one per target in order: prefix-free, the shortest and the most reachable ones first.
///
/// While the alphabet is enough every hint is one letter. Past that, the alphabet's last letters are given up as
/// single hints and become the first letter of two-letter ones (three, and so on, only for absurd counts), so the
/// home row stays single for as long as it can.
pub fn generate(count: usize) -> Vec<String> {
    let mut hints: Vec<String> = ALPHABET.chars().map(String::from).collect();
    while hints.len() < count {
        // The last of the shortest hints grows into one hint per letter, which go to the end of the list.
        let shortest = hints.iter().map(String::len).min().unwrap_or(1);
        let Some(at) = hints.iter().rposition(|hint| hint.len() == shortest) else {
            break;
        };
        let grown = hints.remove(at);
        hints.extend(ALPHABET.chars().map(|letter| format!("{grown}{letter}")));
    }
    hints.truncate(count);
    hints
}

/// What typing `typed` does to the hints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typed {
    /// No hint is left that begins with those letters: hop mode ends.
    Nothing,
    /// Some hints still begin with them: wait for more.
    More,
    /// That is exactly one hint: the target at this index is the one.
    Hit(usize),
}

/// What `typed` makes of `hints`. Letters match whatever their case.
pub fn type_into(hints: &[String], typed: &str) -> Typed {
    if typed.is_empty() {
        return Typed::More;
    }
    let typed = typed.to_lowercase();
    let mut live = false;
    for (index, hint) in hints.iter().enumerate() {
        if *hint == typed {
            return Typed::Hit(index);
        }
        live |= hint.starts_with(&typed);
    }
    if live {
        Typed::More
    } else {
        Typed::Nothing
    }
}

/// Whether `hint` still begins with what has been typed.
pub fn still_matches(hint: &str, typed: &str) -> bool {
    hint.starts_with(&typed.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn hints_are_unique_and_none_begins_another() {
        for count in [0, 1, 9, 26, 27, 40, 100, 400, 676, 700, 1500] {
            let hints = generate(count);
            assert_eq!(hints.len(), count);
            let unique: HashSet<&String> = hints.iter().collect();
            assert_eq!(unique.len(), count, "{count} hints are all different");
            for (i, a) in hints.iter().enumerate() {
                for (j, b) in hints.iter().enumerate() {
                    assert!(i == j || !b.starts_with(a.as_str()), "{a} begins {b} among {count}");
                }
            }
        }
    }

    #[test]
    fn the_home_row_comes_first_and_stays_single_while_the_alphabet_is_enough() {
        let hints = generate(12);
        assert_eq!(hints[..9].concat(), "asdfghjkl");
        assert!(hints.iter().all(|hint| hint.len() == 1));
        assert_eq!(generate(26).iter().filter(|hint| hint.len() == 1).count(), 26);
    }

    #[test]
    fn past_the_alphabet_the_rare_letters_become_the_first_letter_of_two_letter_hints() {
        let hints = generate(40);
        // The home row survives as single hints; the last letters head the two-letter ones.
        assert_eq!(hints[..9].concat(), "asdfghjkl");
        assert!(hints[..9].iter().all(|hint| hint.len() == 1));
        let doubles: Vec<_> = hints.iter().filter(|hint| hint.len() == 2).collect();
        assert!(!doubles.is_empty());
        assert!(doubles
            .iter()
            .all(|hint| hint.starts_with('m') || hint.starts_with('n')));
        // The order is stable: asking for more only appends or regroups the tail, never moves the first letters.
        assert_eq!(generate(15)[..15], hints[..15]);
    }

    #[test]
    fn typing_walks_down_to_one_hint_or_gives_up() {
        let hints = generate(40);
        let first_double = hints.iter().position(|hint| hint.len() == 2).unwrap_or(0);
        let double = hints[first_double].clone();
        assert_eq!(type_into(&hints, "a"), Typed::Hit(0));
        assert_eq!(type_into(&hints, "A"), Typed::Hit(0), "the case doesn't matter");
        assert_eq!(type_into(&hints, &double[..1]), Typed::More);
        assert_eq!(type_into(&hints, &double), Typed::Hit(first_double));
        assert_eq!(type_into(&hints, "mmm"), Typed::Nothing);
        assert_eq!(type_into(&hints, ""), Typed::More);
        assert!(still_matches(&double, &double[..1].to_uppercase()));
        assert!(!still_matches(&double, "a"));
    }
}
