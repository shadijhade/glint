//! The folder's pulse: its changes over the last minute. Card D1 of docs/gpui/aware.
//!
//! Every change the folder watcher reports is a beat. Beats are kept for a minute, a few folders at a time, and
//! counted into thirty buckets of two seconds for the dock's little live line. The arithmetic is here, apart
//! from the clock, so it can be tested; [`super::Aware`] feeds it.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// How long a beat is kept.
const KEPT_FOR: Duration = Duration::from_secs(60);
/// The most beats kept for a folder.
const MOST_BEATS: usize = 256;
/// The most folders kept.
const MOST_FOLDERS: usize = 16;
/// How many seconds a bucket holds.
const BUCKET_SECONDS: u64 = 2;
/// Beats this recent count towards "changing now".
const NOW: Duration = Duration::from_secs(10);
/// How many recent beats make a folder "changing".
const BEATS_TO_CHANGE: usize = 3;

/// Folders' names as they are compared here: lower case, without a trailing separator.
pub(super) fn key(folder: &str) -> String {
    folder.trim_end_matches(['\\', '/']).to_lowercase()
}

/// Thirty counts of two seconds each, the newest last, from how long ago each beat was. A beat of a minute ago
/// or more is left out; a count stops at 255.
pub(crate) fn buckets(beats: &[Duration]) -> [u8; 30] {
    let mut counts = [0u32; 30];
    for age in beats {
        let seconds = age.as_secs();
        if seconds >= KEPT_FOR.as_secs() {
            continue;
        }
        let back = usize::try_from(seconds / BUCKET_SECONDS).unwrap_or(usize::MAX);
        if let Some(slot) = counts.len().checked_sub(1 + back).and_then(|at| counts.get_mut(at)) {
            *slot += 1;
        }
    }
    counts.map(|n| u8::try_from(n.min(255)).unwrap_or(255))
}

/// The beats of the folders recently changed.
#[derive(Default)]
pub(super) struct Pulses {
    beats: HashMap<String, VecDeque<Instant>>,
}

impl Pulses {
    /// A change in `folder` at `now`.
    pub(super) fn beat(&mut self, folder: &str, now: Instant) {
        let queue = self.beats.entry(key(folder)).or_default();
        queue.push_back(now);
        while queue.front().is_some_and(|at| now.saturating_duration_since(*at) > KEPT_FOR) {
            queue.pop_front();
        }
        while queue.len() > MOST_BEATS {
            queue.pop_front();
        }
        if self.beats.len() > MOST_FOLDERS {
            // The folder beaten longest ago goes.
            let oldest = self
                .beats
                .iter()
                .filter_map(|(name, queue)| queue.back().map(|at| (name.clone(), *at)))
                .min_by_key(|(_, at)| *at)
                .map(|(name, _)| name);
            if let Some(name) = oldest {
                self.beats.remove(&name);
            }
        }
    }

    /// How long ago each beat in the last minute was.
    fn ages(&self, folder: &str, now: Instant) -> Vec<Duration> {
        self.beats
            .get(&key(folder))
            .map(|queue| {
                queue
                    .iter()
                    .map(|at| now.saturating_duration_since(*at))
                    .filter(|age| *age < KEPT_FOR)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The folder's last minute, or None when nothing changed in it.
    pub(super) fn pulse(&self, folder: &str, now: Instant) -> Option<[u8; 30]> {
        let ages = self.ages(folder, now);
        (!ages.is_empty()).then(|| buckets(&ages))
    }

    /// Three or more changes in the last ten seconds.
    pub(super) fn changing(&self, folder: &str, now: Instant) -> bool {
        self.ages(folder, now).iter().filter(|age| **age <= NOW).count() >= BEATS_TO_CHANGE
    }
}
