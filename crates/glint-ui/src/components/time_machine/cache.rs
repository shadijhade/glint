//! The scrubbing cache: what each stop of a folder's timeline showed the last time it was read.
//!
//! Reading a stop means walking a Git tree, a shadow copy's folder or a backup's listing, and diffing it against
//! the live folder. Stepping back and forth with the arrow keys would do that again at every step, so the model
//! keeps the last few results for each tab (this is one tab's) and reads the stops either side of the one shown
//! ahead of time. Bounded in stops and in rows, least recently used out first, and every result is only trusted
//! for a minute: it was diffed against the folder as it was then.

use glint_model::time_machine::{Moment, MomentSource, TimeEntry};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How many stops a tab keeps.
const MAX_STOPS: usize = 16;
/// How many rows (entries) all of a tab's kept stops may hold together.
const MAX_ROWS: usize = 60_000;
/// How long a result is trusted: it is a diff against the folder as it was when it was read.
const TRUST: Duration = Duration::from_secs(60);

/// A stop of a folder's timeline.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    folder: String,
    source: MomentSource,
    id: String,
}

impl Key {
    /// The key of `moment` in `folder` (Windows paths compare without case).
    pub fn new(folder: &str, moment: &Moment) -> Self {
        Key {
            folder: folder.to_lowercase(),
            source: moment.source,
            id: moment.id.clone(),
        }
    }
}

struct Slot {
    entries: Arc<Vec<TimeEntry>>,
    used: u64,
    read_at: Instant,
}

/// One tab's results, and the reads that are on their way.
#[derive(Default)]
pub struct ScrubCache {
    slots: HashMap<Key, Slot>,
    rows: usize,
    tick: u64,
    in_flight: HashSet<Key>,
}

impl ScrubCache {
    /// The kept result for `key`, which counts as used now. A result older than the trust time is dropped.
    pub fn get(&mut self, key: &Key) -> Option<Arc<Vec<TimeEntry>>> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.slots.get_mut(key)?;
        if slot.read_at.elapsed() > TRUST {
            self.remove(key);
            return None;
        }
        slot.used = tick;
        Some(slot.entries.clone())
    }

    /// Whether `key` is kept (without counting as a use).
    pub fn has(&self, key: &Key) -> bool {
        self.slots.get(key).is_some_and(|s| s.read_at.elapsed() <= TRUST)
    }

    /// Keeps `entries` for `key`, letting the least recently used go until the bounds hold. A result that is
    /// bigger than the whole row bound isn't kept.
    pub fn insert(&mut self, key: Key, entries: Arc<Vec<TimeEntry>>) {
        if entries.len() > MAX_ROWS {
            return;
        }
        self.remove(&key);
        self.tick += 1;
        self.rows += entries.len();
        self.slots.insert(
            key,
            Slot {
                entries,
                used: self.tick,
                read_at: Instant::now(),
            },
        );
        while self.slots.len() > MAX_STOPS || self.rows > MAX_ROWS {
            let Some(oldest) = self.slots.iter().min_by_key(|(_, s)| s.used).map(|(k, _)| k.clone()) else {
                break;
            };
            self.remove(&oldest);
        }
    }

    fn remove(&mut self, key: &Key) {
        if let Some(slot) = self.slots.remove(key) {
            self.rows -= slot.entries.len();
        }
    }

    /// Says a read of `key` is on its way. False when one already is (or the result is already kept), so each
    /// stop is read ahead once.
    pub fn begin_read(&mut self, key: &Key) -> bool {
        !self.has(key) && self.in_flight.insert(key.clone())
    }

    /// Says the read of `key` is over, kept or not.
    pub fn end_read(&mut self, key: &Key) {
        self.in_flight.remove(key);
    }

    /// How many stops are kept.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// How many rows the kept stops hold.
    #[cfg(test)]
    pub fn rows(&self) -> usize {
        self.rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::time_machine::{EntryDelta, RestoreSource};

    fn moment(id: &str) -> Moment {
        Moment {
            id: id.into(),
            timestamp_ms: 1,
            label: String::new(),
            detail: String::new(),
            source: MomentSource::GitCommit,
            shadow_path: None,
            commit_oid: None,
            repo_root: None,
            plan_id: None,
            snapshot_id: None,
        }
    }

    fn rows(n: usize) -> Arc<Vec<TimeEntry>> {
        Arc::new(
            (0..n)
                .map(|i| TimeEntry {
                    name: format!("f{i}"),
                    dir: false,
                    size: 1,
                    modified: 1,
                    delta: EntryDelta::Unchanged,
                    restore: RestoreSource::ShadowPath(String::new()),
                })
                .collect(),
        )
    }

    #[test]
    fn keeps_the_most_recently_used_stops_within_both_bounds() {
        let mut cache = ScrubCache::default();
        let key = |i: usize| Key::new("C:\\Work", &moment(&format!("m{i}")));
        for i in 0..MAX_STOPS {
            cache.insert(key(i), rows(10));
        }
        assert_eq!(cache.len(), MAX_STOPS);
        // m0 is used again, so m1 is the one that goes when another comes.
        assert!(cache.get(&key(0)).is_some());
        cache.insert(key(100), rows(10));
        assert_eq!(cache.len(), MAX_STOPS);
        assert!(cache.has(&key(0)) && !cache.has(&key(1)) && cache.has(&key(100)));

        // Rows are bounded too: one big stop pushes the oldest out, a huge one isn't kept at all.
        cache.insert(key(200), rows(MAX_ROWS - 20));
        assert!(cache.rows() <= MAX_ROWS);
        assert!(cache.has(&key(200)));
        cache.insert(key(300), rows(MAX_ROWS + 1));
        assert!(!cache.has(&key(300)));
        assert!(cache.has(&key(200)));
    }

    #[test]
    fn keys_ignore_the_case_of_the_folder_and_a_read_ahead_is_started_once() {
        let mut cache = ScrubCache::default();
        let m = moment("a");
        let a = Key::new("C:\\WORK", &m);
        let b = Key::new("c:\\work", &m);
        assert_eq!(a, b);
        assert!(cache.begin_read(&a));
        assert!(!cache.begin_read(&b), "already on its way");
        cache.insert(a.clone(), rows(3));
        cache.end_read(&a);
        assert!(!cache.begin_read(&b), "already kept");
        assert_eq!(cache.get(&b).map(|e| e.len()), Some(3));
    }
}
