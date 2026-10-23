//! What grew: how big each folder was at daily marks, for the disk-usage map's Growth colors.
//!
//! A folder's size is noted just before its first change after a mark. A folder nothing changed in
//! since a mark is as big as it is now; one that changed was as big as the first note after the mark
//! says. That's exact, and only folders that changed take room. Files need nothing kept: their last
//! write time says whether they changed since.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// The note of a folder that wasn't there at the mark (made since, or its record then held
/// something else).
pub const ABSENT: u64 = u64::MAX;
/// How many marks back folders can be compared with, besides the one of the day in progress.
pub const FOLDER_MARKS: usize = 40;
/// Marks kept for the drive's total.
pub const MAX_MARKS: usize = 400;
/// Notes kept at most (16 bytes each). Past this, the oldest marks stop being comparable.
pub const MAX_NOTES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Counts up by one with each mark (65,536 days before it wraps).
    pub epoch: u16,
    /// When it was taken, Unix ms.
    pub at: i64,
    /// The drive's total then.
    pub total: u64,
}

/// A folder's size before its first change after the mark of `epoch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub rec: u32,
    /// The record's sequence number: a record reused for another folder is another folder.
    pub seq: u16,
    pub epoch: u16,
    pub before: u64,
}

/// Record numbers spread well enough that one multiply makes a good hash, which matters here: a
/// change to a file notes every folder above it.
#[derive(Default)]
pub struct RecordHasher(u64);

impl Hasher for RecordHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.0 = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type ByRecord<V> = HashMap<u32, V, BuildHasherDefault<RecordHasher>>;

#[derive(Default, Debug, Clone)]
pub struct Growth {
    /// Oldest first; the last one started the day in progress. Empty: nothing is noted yet.
    marks: Vec<Mark>,
    /// The oldest mark folders can be compared with: notes from before it are gone.
    horizon: u16,
    /// Notes from the days before the one in progress, by record, then mark.
    notes: Vec<Note>,
    /// The day in progress's notes: sequence number and size before, by record.
    current: ByRecord<(u16, u64)>,
}

impl Growth {
    /// Whether marks are kept (once the first is taken).
    pub fn tracking(&self) -> bool {
        !self.marks.is_empty()
    }

    /// The mark that started the day in progress.
    pub fn last(&self) -> Option<Mark> {
        self.marks.last().copied()
    }

    /// Every mark kept, oldest first, for the drive's total over time.
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    /// Notes folder `rec`'s size before a change, unless it changed already since the last mark.
    /// `before` is ABSENT for a folder that's new.
    #[inline]
    pub fn note(&mut self, rec: u32, seq: u16, before: u64) {
        if !self.marks.is_empty() {
            self.current.entry(rec).or_insert((seq, before));
        }
    }

    /// Takes a mark at `at` (Unix ms) with the drive's total then: the day in progress ends, and
    /// what's past the limits goes.
    pub fn mark(&mut self, at: i64, total: u64) {
        self.mark_within(at, total, MAX_NOTES);
    }

    fn mark_within(&mut self, at: i64, total: u64, max_notes: usize) {
        let epoch = match self.marks.last() {
            Some(last) => {
                let mut ended: Vec<Note> = self.current.drain().map(|(rec, (seq, before))| Note { rec, seq, epoch: last.epoch, before }).collect();
                ended.sort_unstable_by_key(|n| n.rec);
                self.notes = merge(std::mem::take(&mut self.notes), ended);
                last.epoch.wrapping_add(1)
            }
            None => {
                self.horizon = 0;
                0
            }
        };
        self.marks.push(Mark { epoch, at, total });
        if self.marks.len() > MAX_MARKS {
            self.marks.drain(..self.marks.len() - MAX_MARKS);
        }
        // Folders compare with the last FOLDER_MARKS marks, fewer if their notes run over.
        let mut horizon = self.horizon.max(self.marks[self.marks.len().saturating_sub(FOLDER_MARKS + 1)].epoch);
        let mut per_epoch: HashMap<u16, usize> = HashMap::new();
        for note in &self.notes {
            *per_epoch.entry(note.epoch).or_default() += 1;
        }
        let mut kept: usize = per_epoch.iter().filter(|(&e, _)| e >= horizon).map(|(_, &n)| n).sum();
        while kept > max_notes && horizon < epoch {
            kept -= per_epoch.get(&horizon).copied().unwrap_or(0);
            horizon += 1;
        }
        self.horizon = horizon;
        self.notes.retain(|n| n.epoch >= horizon);
    }

    /// After the index was read from the disk again, the notes no longer say which folder is which:
    /// folders compare from a new mark on. The drive's totals stay.
    pub fn restart(&mut self, at: i64, total: u64) {
        self.notes.clear();
        self.current.clear();
        self.mark(at, total);
        self.horizon = self.marks.last().map_or(0, |m| m.epoch);
    }

    /// The mark folders can be compared with that's closest to `since` (Unix ms); the earlier of two
    /// as close.
    pub fn closest(&self, since: i64) -> Option<Mark> {
        self.marks.iter().filter(|m| m.epoch >= self.horizon).min_by_key(|m| m.at.abs_diff(since)).copied()
    }

    /// The oldest mark folders can be compared with.
    pub fn oldest(&self) -> Option<Mark> {
        self.marks.iter().find(|m| m.epoch >= self.horizon).copied()
    }

    /// How big folder `rec` (now `seq`, `now` bytes) was at the mark of `epoch`: None if it wasn't
    /// there.
    pub fn then(&self, rec: u32, seq: u16, epoch: u16, now: u64) -> Option<u64> {
        // The first note after that mark tells; with none, nothing changed since.
        let at = self.notes.partition_point(|n| (n.rec, n.epoch) < (rec, epoch));
        let first = match self.notes.get(at) {
            Some(n) if n.rec == rec => Some((n.seq, n.before)),
            _ => self.current.get(&rec).copied(),
        };
        match first {
            Some((s, before)) if s == seq => (before != ABSENT).then_some(before),
            // The record held another folder (or file) then.
            Some(_) => None,
            None => Some(now),
        }
    }

    /// What a snapshot keeps: the horizon, the marks, and every note (the day in progress's too).
    pub(crate) fn parts(&self) -> (u16, &[Mark], Vec<Note>) {
        let mut notes = self.notes.clone();
        if let Some(last) = self.marks.last() {
            let mut current: Vec<Note> = self.current.iter().map(|(&rec, &(seq, before))| Note { rec, seq, epoch: last.epoch, before }).collect();
            current.sort_unstable_by_key(|n| n.rec);
            notes = merge(notes, current);
        }
        (self.horizon, &self.marks, notes)
    }

    /// Back from a snapshot's parts; None when they don't hang together.
    pub(crate) fn from_parts(horizon: u16, marks: Vec<Mark>, notes: Vec<Note>) -> Option<Growth> {
        let Some(last) = marks.last().map(|m| m.epoch) else {
            return notes.is_empty().then(Growth::default);
        };
        let ordered = marks.windows(2).all(|w| w[0].epoch < w[1].epoch) && marks.len() <= MAX_MARKS && horizon <= last;
        let sorted = notes.windows(2).all(|w| (w[0].rec, w[0].epoch) < (w[1].rec, w[1].epoch));
        if !ordered || !sorted || notes.iter().any(|n| n.epoch < horizon || n.epoch > last) {
            return None;
        }
        let (current, notes): (Vec<Note>, Vec<Note>) = notes.into_iter().partition(|n| n.epoch == last);
        Some(Growth { marks, horizon, notes, current: current.into_iter().map(|n| (n.rec, (n.seq, n.before))).collect() })
    }

    pub fn memory(&self) -> u64 {
        (self.notes.capacity() * size_of::<Note>() + self.current.capacity() * 20 + self.marks.capacity() * size_of::<Mark>()) as u64
    }
}

/// Two lists sorted by record then mark, as one; `later`'s notes are newer than `earlier`'s.
fn merge(earlier: Vec<Note>, later: Vec<Note>) -> Vec<Note> {
    if later.is_empty() {
        return earlier;
    }
    let mut out = Vec::with_capacity(earlier.len() + later.len());
    let (mut a, mut b) = (earlier.into_iter().peekable(), later.into_iter().peekable());
    loop {
        match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if (x.rec, x.epoch) <= (y.rec, y.epoch) => out.push(a.next().unwrap()),
            (Some(_), Some(_)) | (None, Some(_)) => out.push(b.next().unwrap()),
            (Some(_), None) => out.push(a.next().unwrap()),
            (None, None) => return out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn notes_only_the_first_change_after_each_mark() {
        let mut growth = Growth::default();
        // Nothing is noted before the first mark.
        growth.note(20, 1, 100);
        assert!(!growth.tracking());
        growth.mark(0, 1000);
        growth.note(20, 1, 100);
        growth.note(20, 1, 150);
        growth.note(21, 1, ABSENT);
        growth.mark(DAY, 1100);
        growth.note(20, 1, 170);
        growth.mark(2 * DAY, 1200);
        // Folder 20: 100 at mark 0, 170 at mark 1, unchanged since mark 2 (now 180).
        assert_eq!(growth.then(20, 1, 0, 180), Some(100));
        assert_eq!(growth.then(20, 1, 1, 180), Some(170));
        assert_eq!(growth.then(20, 1, 2, 180), Some(180));
        // Folder 21 was made after mark 0.
        assert_eq!(growth.then(21, 1, 0, 5), None);
        assert_eq!(growth.then(21, 1, 1, 5), Some(5));
        // A folder nothing happened to.
        assert_eq!(growth.then(30, 1, 0, 42), Some(42));
        // The day in progress's notes count as well.
        growth.note(30, 1, 42);
        assert_eq!(growth.then(30, 1, 0, 50), Some(42));
        assert_eq!(growth.then(30, 1, 2, 50), Some(42));
    }

    #[test]
    fn a_reused_record_is_another_folder() {
        let mut growth = Growth::default();
        growth.mark(0, 0);
        // Record 20 held folder (seq 1); it was deleted, and the record made into a new folder (seq 2)
        // the same day: its first note stays the old folder's.
        growth.note(20, 1, 300);
        growth.note(20, 2, ABSENT);
        growth.mark(DAY, 0);
        assert_eq!(growth.then(20, 2, 0, 10), None);
        assert_eq!(growth.then(20, 2, 1, 10), Some(10));
        // Made over again on a later day.
        growth.note(20, 3, ABSENT);
        assert_eq!(growth.then(20, 3, 1, 10), None);
    }

    #[test]
    fn picks_the_closest_mark_folders_compare_with() {
        let mut growth = Growth::default();
        assert_eq!(growth.closest(0), None);
        for day in 0..50 {
            growth.mark(day * DAY + 3_600_000, 1000 + day as u64);
        }
        assert_eq!(growth.marks().len(), 50);
        // Folders compare with the last 41 marks; the drive's totals go back further.
        assert_eq!(growth.oldest().unwrap().epoch, 9);
        assert_eq!(growth.closest(49 * DAY).unwrap().epoch, 49);
        assert_eq!(growth.closest(48 * DAY - 5 * 3_600_000).unwrap().epoch, 48);
        assert_eq!(growth.closest(48 * DAY - 15 * 3_600_000).unwrap().epoch, 47);
        assert_eq!(growth.closest(0).unwrap().epoch, 9);
        // Halfway between two marks, the earlier one.
        assert_eq!(growth.closest(20 * DAY + 3_600_000 + DAY / 2).unwrap().epoch, 20);
    }

    #[test]
    fn too_many_notes_give_up_the_oldest_marks() {
        let mut growth = Growth::default();
        growth.mark(0, 0);
        for day in 1..=5u32 {
            for rec in 0..100 {
                growth.note(rec + day * 1000, 1, rec as u64);
            }
            growth.mark_within(day as i64 * DAY, 0, 250);
        }
        // 500 notes, 100 a day: only the last two days' (and the day in progress, empty) stay.
        assert_eq!(growth.notes.len(), 200);
        assert_eq!(growth.oldest().unwrap().epoch, 3);
        assert_eq!(growth.then(4000, 1, 3, 7), Some(0));
        assert_eq!(growth.closest(0).unwrap().epoch, 3);
    }

    #[test]
    fn restarts_after_a_new_scan() {
        let mut growth = Growth::default();
        growth.mark(0, 500);
        growth.note(20, 1, 100);
        growth.mark(DAY, 600);
        growth.restart(DAY + 10, 700);
        assert_eq!(growth.marks().iter().map(|m| m.total).collect::<Vec<_>>(), [500, 600, 700]);
        assert_eq!(growth.oldest().unwrap().epoch, 2);
        assert_eq!(growth.then(20, 1, 2, 120), Some(120));
        assert_eq!(growth.closest(0).unwrap().at, DAY + 10);
    }

    #[test]
    fn round_trips_through_its_parts() {
        let mut growth = Growth::default();
        growth.mark(0, 1);
        growth.note(7, 1, 70);
        growth.note(3, 1, 30);
        growth.mark(DAY, 2);
        growth.note(5, 2, ABSENT);
        growth.note(3, 1, 31);
        let (horizon, marks, notes) = growth.parts();
        assert_eq!(notes.iter().map(|n| (n.rec, n.epoch)).collect::<Vec<_>>(), [(3, 0), (3, 1), (5, 1), (7, 0)]);
        let back = Growth::from_parts(horizon, marks.to_vec(), notes.clone()).unwrap();
        assert_eq!(back.then(3, 1, 0, 40), Some(30));
        assert_eq!(back.then(3, 1, 1, 40), Some(31));
        assert_eq!(back.then(5, 2, 1, 9), None);
        assert_eq!(back.current.len(), 2);
        // Out of order, or past the marks: not used.
        let mut shuffled = notes.clone();
        shuffled.swap(0, 3);
        assert!(Growth::from_parts(horizon, marks.to_vec(), shuffled).is_none());
        assert!(Growth::from_parts(horizon, marks[..1].to_vec(), notes).is_none());
        assert!(Growth::from_parts(0, vec![], vec![]).is_some_and(|g| !g.tracking()));
    }
}
