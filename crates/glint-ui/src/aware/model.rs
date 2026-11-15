//! The cache of what was found out, and looking ahead. Card D1 of docs/gpui/aware.
//!
//! This file is the bookkeeping only, with no GPUI in it: what is kept per path, what is still to ask, and what
//! to let go of. [`super::Aware`] drives it. Everything takes the time as an argument so a test can say what
//! "thirty seconds later" is.

use glint_model::aware::{want_for, Part, Probed, Want};
use glint_model::entries::FileEntry;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// The most paths kept; past it the 64 asked about longest ago are let go.
pub(super) const MOST_KEPT: usize = 512;
/// How many are let go at once when [`MOST_KEPT`] is passed.
const LET_GO: usize = 64;
/// A folder's inside is asked again when its answer is older than this (the old one is shown meanwhile).
pub(super) const INSIDE_FRESH: Duration = Duration::from_secs(30);

/// What is kept about one path.
struct Kept {
    /// The entry's size and modified time when it was asked about: another pair means another file.
    size: u64,
    modified: i64,
    probed: Probed,
    /// What has been asked for already.
    asked: Want,
    /// When a folder's inside was last asked for.
    inside_at: Option<Instant>,
    /// Which look asked about it last (bigger is later): the order things are let go in.
    tick: u64,
}

/// What has been found out about files, by lower-case path.
#[derive(Default)]
pub(super) struct Cache {
    kept: HashMap<String, Kept>,
    tick: u64,
}

fn key(path: &str) -> String {
    path.to_lowercase()
}

impl Cache {
    /// What is still to ask about `entry`: what is worth knowing about it, less what is known and what was asked
    /// already. A file that changed since it was last asked about is forgotten first. A folder's inside comes
    /// round again once its last ask is older than [`INSIDE_FRESH`].
    pub(super) fn plan(&mut self, entry: &FileEntry, now: Instant) -> Want {
        let wanted = want_for(entry);
        if wanted.is_empty() {
            return wanted;
        }
        let id = key(&entry.path);
        if self.kept.get(&id).is_some_and(|k| k.size != entry.size || k.modified != entry.modified) {
            self.kept.remove(&id);
        }
        let Some(kept) = self.kept.get(&id) else {
            return wanted;
        };
        let mut left = wanted.minus(&kept.probed).without(kept.asked);
        left.inside = wanted.inside
            && match kept.inside_at {
                Some(at) => now.saturating_duration_since(at) >= INSIDE_FRESH,
                None => !kept.asked.inside,
            };
        left
    }

    /// Notes that `want` is being asked about `entry` now.
    pub(super) fn asking(&mut self, entry: &FileEntry, want: Want, now: Instant) {
        self.tick += 1;
        let tick = self.tick;
        let kept = self.kept.entry(key(&entry.path)).or_insert_with(|| Kept {
            size: entry.size,
            modified: entry.modified,
            probed: Probed::default(),
            asked: Want::default(),
            inside_at: None,
            tick,
        });
        kept.asked = kept.asked.and(want);
        if want.inside {
            kept.inside_at = Some(now);
        }
        kept.tick = tick;
        if self.kept.len() > MOST_KEPT {
            self.let_go();
        }
    }

    /// A run that asked for `want` about `path` was stopped: what it didn't answer may be asked again.
    pub(super) fn unasked(&mut self, path: &str, want: Want) {
        if let Some(kept) = self.kept.get_mut(&key(path)) {
            let unanswered = want.minus(&kept.probed);
            kept.asked = kept.asked.without(unanswered);
        }
    }

    /// Takes in one answer; false when nothing is kept for the path any more (it was forgotten meanwhile).
    pub(super) fn take(&mut self, path: &str, part: Part) -> bool {
        match self.kept.get_mut(&key(path)) {
            Some(kept) => {
                kept.probed.take(part);
                true
            }
            None => false,
        }
    }

    /// What is known about `entry`, if it is the file that was asked about.
    pub(super) fn probed(&self, entry: &FileEntry) -> Option<&Probed> {
        let kept = self.kept.get(&key(&entry.path))?;
        (kept.size == entry.size && kept.modified == entry.modified).then_some(&kept.probed)
    }

    /// Drops what is kept for `path`.
    pub(super) fn forget(&mut self, path: &str) {
        self.kept.remove(&key(path));
    }

    /// How many paths are kept.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.kept.len()
    }

    /// Lets go of the [`LET_GO`] paths asked about longest ago.
    fn let_go(&mut self) {
        let mut ticks: Vec<u64> = self.kept.values().map(|k| k.tick).collect();
        ticks.sort_unstable();
        let Some(&cut) = ticks.get(LET_GO.min(ticks.len()).saturating_sub(1)) else {
            return;
        };
        self.kept.retain(|_, k| k.tick > cut);
    }
}

/// The drive roots a look may ask about: lower-case roots, each with whether the drive is a fixed disk.
pub(super) struct Roots(pub(super) Vec<(String, bool)>);

impl Roots {
    /// Whether `path` is on a fixed disk: the longest root it starts with decides, and no root means no.
    pub(super) fn fixed(&self, path: &str) -> bool {
        let lower = path.to_lowercase();
        self.0
            .iter()
            .filter(|(root, _)| lower.starts_with(root.as_str()))
            .max_by_key(|(root, _)| root.len())
            .is_some_and(|(_, fixed)| *fixed)
    }
}
