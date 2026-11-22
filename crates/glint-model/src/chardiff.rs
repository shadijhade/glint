//! A character-level difference between two file names, for the By-example preview (what a rename deletes and what it
//! inserts).
//!
//! The longest common subsequence of the two names' characters is what stays; the rest of the old name is deleted
//! and the rest of the new name is inserted. Names are short (a file name is at most 255 characters), so the plain
//! table is cheap; a pair too long for it is cut down to what lies between its common start and end. Combining marks
//! (Arabic and Hebrew points, accents) and joiners stay with the character they follow, so a letter and its mark are
//! never given different colors.
//!
//! The pieces are text, not positions: a view draws them as styled runs and lets the text system place them, which is
//! what keeps Arabic and Hebrew names right.

/// What happens to a piece of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// In both names.
    Same,
    /// Only in the old name: the rename removes it.
    Deleted,
    /// Only in the new name: the rename adds it.
    Inserted,
}

/// A run of text and what happens to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub edit: Edit,
}

/// The two names as runs: `old` holds the [`Edit::Same`] and [`Edit::Deleted`] pieces and reads as the old name,
/// `new` the [`Edit::Same`] and [`Edit::Inserted`] ones and reads as the new name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CharDiff {
    pub old: Vec<Piece>,
    pub new: Vec<Piece>,
}

impl CharDiff {
    /// The old name's text.
    pub fn old_text(&self) -> String {
        self.old.iter().map(|p| p.text.as_str()).collect()
    }

    /// The new name's text.
    pub fn new_text(&self) -> String {
        self.new.iter().map(|p| p.text.as_str()).collect()
    }

    /// Whether anything is deleted or inserted.
    pub fn changed(&self) -> bool {
        self.old.iter().any(|p| p.edit != Edit::Same) || self.new.iter().any(|p| p.edit != Edit::Same)
    }
}

/// The most table cells the common-subsequence search fills (about two 1,000-character names).
const MAX_CELLS: usize = 1 << 20;

/// Whether `c` sticks to the character before it: combining marks, joiners and variation selectors.
fn sticks(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F      // combining diacritics
        | 0x0483..=0x0489    // Cyrillic
        | 0x0591..=0x05BD    // Hebrew points and accents
        | 0x05BF | 0x05C1..=0x05C2 | 0x05C4..=0x05C5 | 0x05C7
        | 0x0610..=0x061A    // Arabic signs
        | 0x064B..=0x065F    // Arabic points (harakat)
        | 0x0670
        | 0x06D6..=0x06DC | 0x06DF..=0x06E4 | 0x06E7..=0x06E8 | 0x06EA..=0x06ED
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF
        | 0x200C | 0x200D    // joiners
        | 0xFE00..=0xFE0F    // variation selectors
        | 0xFE20..=0xFE2F)
}

/// The text as clusters: each character with the marks that follow it.
fn clusters(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for (at, c) in text.char_indices() {
        if at > 0 && !sticks(c) {
            out.push(&text[start..at]);
            start = at;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// What `old` becomes as `new`, character by character.
pub fn diff(old: &str, new: &str) -> CharDiff {
    let (a, b) = (clusters(old), clusters(new));
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let end = a[start..]
        .iter()
        .rev()
        .zip(b[start..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (mid_a, mid_b) = (&a[start..a.len() - end], &b[start..b.len() - end]);

    // Per cluster: what it is in the old name and in the new one.
    let mut old_marks: Vec<(&str, Edit)> = a[..start].iter().map(|c| (*c, Edit::Same)).collect();
    let mut new_marks: Vec<(&str, Edit)> = b[..start].iter().map(|c| (*c, Edit::Same)).collect();
    if mid_a.len().saturating_mul(mid_b.len()) <= MAX_CELLS {
        common(mid_a, mid_b, &mut old_marks, &mut new_marks);
    } else {
        // Too long for the table: what lies between the common ends is replaced.
        old_marks.extend(mid_a.iter().map(|c| (*c, Edit::Deleted)));
        new_marks.extend(mid_b.iter().map(|c| (*c, Edit::Inserted)));
    }
    old_marks.extend(a[a.len() - end..].iter().map(|c| (*c, Edit::Same)));
    new_marks.extend(b[b.len() - end..].iter().map(|c| (*c, Edit::Same)));
    CharDiff {
        old: runs(&old_marks),
        new: runs(&new_marks),
    }
}

/// Marks the clusters of `a` and `b` that stay (the longest common subsequence) and the ones that don't.
fn common<'a>(a: &[&'a str], b: &[&'a str], old: &mut Vec<(&'a str, Edit)>, new: &mut Vec<(&'a str, Edit)>) {
    let (n, m) = (a.len(), b.len());
    // lcs[i][j]: the length of the common subsequence of a[i..] and b[j..].
    let width = m + 1;
    let mut lcs = vec![0u32; (n + 1) * width];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * width + j] = if a[i] == b[j] {
                lcs[(i + 1) * width + j + 1] + 1
            } else {
                lcs[(i + 1) * width + j].max(lcs[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            old.push((a[i], Edit::Same));
            new.push((b[j], Edit::Same));
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * width + j] >= lcs[i * width + j + 1] {
            old.push((a[i], Edit::Deleted));
            i += 1;
        } else {
            new.push((b[j], Edit::Inserted));
            j += 1;
        }
    }
    old.extend(a[i..].iter().map(|c| (*c, Edit::Deleted)));
    new.extend(b[j..].iter().map(|c| (*c, Edit::Inserted)));
}

/// Neighbouring clusters with the same edit joined into one piece.
fn runs(marks: &[(&str, Edit)]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for (text, edit) in marks {
        match out.last_mut() {
            Some(last) if last.edit == *edit => last.text.push_str(text),
            _ => out.push(Piece {
                text: (*text).to_string(),
                edit: *edit,
            }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(list: &[Piece]) -> Vec<(&str, Edit)> {
        list.iter().map(|p| (p.text.as_str(), p.edit)).collect()
    }

    #[test]
    fn an_unchanged_name_is_all_the_same() {
        let d = diff("holiday.jpg", "holiday.jpg");
        assert_eq!(pieces(&d.old), [("holiday.jpg", Edit::Same)]);
        assert_eq!(pieces(&d.new), [("holiday.jpg", Edit::Same)]);
        assert!(!d.changed());
        assert_eq!(diff("", ""), CharDiff::default());
    }

    #[test]
    fn what_goes_is_deleted_and_what_comes_is_inserted() {
        let d = diff("IMG_0042.jpg", "Tokyo_0042.jpg");
        assert_eq!(d.old_text(), "IMG_0042.jpg");
        assert_eq!(d.new_text(), "Tokyo_0042.jpg");
        let gone: String = d
            .old
            .iter()
            .filter(|p| p.edit == Edit::Deleted)
            .map(|p| p.text.as_str())
            .collect();
        let come: String = d
            .new
            .iter()
            .filter(|p| p.edit == Edit::Inserted)
            .map(|p| p.text.as_str())
            .collect();
        // "IMG" has nothing in common with "Tokyo"; "_0042.jpg" stays.
        assert_eq!(gone, "IMG");
        assert_eq!(come, "Tokyo");
        assert_eq!(d.old.last().map(|p| p.edit), Some(Edit::Same));
    }

    #[test]
    fn a_name_that_only_grows_or_shrinks_has_one_kind_of_edit() {
        let grown = diff("a.txt", "a_final.txt");
        assert!(grown.old.iter().all(|p| p.edit == Edit::Same));
        assert_eq!(
            pieces(&grown.new),
            [("a", Edit::Same), ("_final", Edit::Inserted), (".txt", Edit::Same)]
        );
        let shrunk = diff("a_final.txt", "a.txt");
        assert!(shrunk.new.iter().all(|p| p.edit == Edit::Same));
        assert_eq!(
            pieces(&shrunk.old),
            [("a", Edit::Same), ("_final", Edit::Deleted), (".txt", Edit::Same)]
        );
    }

    #[test]
    fn the_two_names_read_back_as_they_were_for_any_pair() {
        for (a, b) in [
            ("2026-04-12 - Tokyo.jpg", "IMG_20260412_Tokyo.jpg"),
            ("abcabc", "cabcab"),
            ("", "new.txt"),
            ("old.txt", ""),
            ("same", "SAME"),
        ] {
            let d = diff(a, b);
            assert_eq!(d.old_text(), a);
            assert_eq!(d.new_text(), b);
        }
    }

    #[test]
    fn an_arabic_mark_stays_with_its_letter() {
        // kitab with a fatha on the kaf, to kitab with a kasra: the mark changes, so the letter and its mark
        // change together and the cluster is never split between two styles.
        let old = "\u{643}\u{64E}\u{62A}\u{627}\u{628}.txt";
        let new = "\u{643}\u{650}\u{62A}\u{627}\u{628}.txt";
        let d = diff(old, new);
        assert_eq!(d.old_text(), old);
        assert_eq!(d.new_text(), new);
        for piece in d.old.iter().chain(&d.new) {
            // No piece starts with a mark: a mark is always in the piece of the letter before it.
            assert!(!piece.text.chars().next().is_some_and(sticks), "{piece:?}");
        }
        let gone = d.old.iter().find(|p| p.edit == Edit::Deleted).map(|p| p.text.as_str());
        assert_eq!(gone, Some("\u{643}\u{64E}"));
        let come = d.new.iter().find(|p| p.edit == Edit::Inserted).map(|p| p.text.as_str());
        assert_eq!(come, Some("\u{643}\u{650}"));
    }

    #[test]
    fn hebrew_and_arabic_names_diff_by_letters_in_reading_order() {
        let d = diff(
            "\u{5E9}\u{5DC}\u{5D5}\u{5DD}.txt",
            "\u{5E9}\u{5DC}\u{5D5}\u{5DD}_\u{5D7}\u{5D3}\u{5E9}.txt",
        );
        assert_eq!(
            pieces(&d.new),
            [
                ("\u{5E9}\u{5DC}\u{5D5}\u{5DD}", Edit::Same),
                ("_\u{5D7}\u{5D3}\u{5E9}", Edit::Inserted),
                (".txt", Edit::Same)
            ]
        );
        let a = diff(
            "\u{635}\u{648}\u{631}\u{629}.jpg",
            "\u{635}\u{648}\u{631}\u{629}_\u{661}.jpg",
        );
        assert_eq!(a.old.iter().filter(|p| p.edit != Edit::Same).count(), 0);
        assert_eq!(a.new.iter().filter(|p| p.edit == Edit::Inserted).count(), 1);
    }

    #[test]
    fn a_pair_too_long_for_the_table_is_cut_down_to_its_middle() {
        let long_a = format!("start_{}_end", "a".repeat(3000));
        let long_b = format!("start_{}_end", "b".repeat(3000));
        let d = diff(&long_a, &long_b);
        assert_eq!(d.old_text(), long_a);
        assert_eq!(d.new_text(), long_b);
        assert_eq!(d.old.len(), 3);
        assert_eq!(d.old[1].edit, Edit::Deleted);
        assert_eq!(d.new[1].edit, Edit::Inserted);
    }
}
