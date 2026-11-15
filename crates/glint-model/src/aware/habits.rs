//! What the person did before: counts of the actions chosen for each kind of thing, and of the folders each kind
//! of file was moved or copied to. They fade with time, and stay small.
//!
//! Nothing here knows about menus: an action is a string and a folder is a string. Nothing reads the clock either:
//! `today` is always given (days since 1970-01-01, see [`day_of`]).

use super::shape::Shape;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How long it takes a use to count half as much, in days.
pub const HALF_LIFE_DAYS: f64 = 30.;
/// The most actions kept for one shape.
pub const MOST_ACTIONS: usize = 48;
/// The most folders kept for one extension.
pub const MOST_PLACES: usize = 16;

/// How often something was done, and when last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Use {
    /// How many times.
    pub n: u32,
    /// The day it was last done, in days since 1970-01-01.
    pub day: u32,
}

/// What the person did before. Saved in the app's state (`AppState::habits`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Habits {
    /// [`Shape::key`] → the action's key → its use.
    pub actions: BTreeMap<String, BTreeMap<String, Use>>,
    /// A file's extension, lower case ("" for folders) → the folder it went to → its use.
    pub places: BTreeMap<String, BTreeMap<String, Use>>,
}

/// A folder things of one kind have gone to.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub folder: String,
    /// How many times.
    pub uses: u32,
    /// Its weight today.
    pub weight: f64,
    /// Its part of the weight of every folder that kind has gone to (0 to 1).
    pub share: f64,
}

impl Habits {
    /// Nothing has been learned.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty() && self.places.is_empty()
    }

    /// Counts `action`, chosen today for something of `shape`.
    pub fn note_action(&mut self, shape: Shape, action: &str, today: u32) {
        if action.is_empty() {
            return;
        }
        let kept = self.actions.entry(shape.key().to_string()).or_default();
        count(kept, action, today);
        trim(kept, action, MOST_ACTIONS, today);
    }

    /// Counts `folder` as where a file with extension `ext` went today.
    pub fn note_place(&mut self, ext: &str, folder: &str, today: u32) {
        // A drive's root ("C:\") keeps its backslash: without it the path means the drive's current folder.
        let trimmed = folder.strip_suffix('\\').filter(|rest| !rest.ends_with(':')).unwrap_or(folder);
        if trimmed.is_empty() {
            return;
        }
        let kept = self.places.entry(ext.to_lowercase()).or_default();
        let lower = trimmed.to_lowercase();
        // The spelling first given stays; a folder that differs only in case is the same folder.
        let key = kept
            .keys()
            .find(|have| have.to_lowercase() == lower)
            .cloned()
            .unwrap_or_else(|| trimmed.to_string());
        count(kept, &key, today);
        trim(kept, &key, MOST_PLACES, today);
    }

    /// How much `action` weighs today for `shape` (0 when never chosen).
    pub fn action_weight(&self, shape: Shape, action: &str, today: u32) -> f64 {
        self.actions
            .get(shape.key())
            .and_then(|kept| kept.get(action))
            .map_or(0., |used| weight(*used, today))
    }

    /// How many times `action` was chosen for `shape`.
    pub fn action_uses(&self, shape: Shape, action: &str) -> u32 {
        self.actions.get(shape.key()).and_then(|kept| kept.get(action)).map_or(0, |used| used.n)
    }

    /// How much `action` weighs today over every shape (the command palette's order).
    pub fn overall_weight(&self, action: &str, today: u32) -> f64 {
        self.actions.values().filter_map(|kept| kept.get(action)).map(|used| weight(*used, today)).sum()
    }

    /// The folders files with extension `ext` have gone to, heaviest first.
    pub fn places_for(&self, ext: &str, today: u32) -> Vec<Place> {
        let Some(kept) = self.places.get(&ext.to_lowercase()) else {
            return Vec::new();
        };
        let mut rows: Vec<(Place, u32)> = kept
            .iter()
            .map(|(folder, used)| {
                let place = Place { folder: folder.clone(), uses: used.n, weight: weight(*used, today), share: 0. };
                (place, used.day)
            })
            .collect();
        let total: f64 = rows.iter().map(|(place, _)| place.weight).sum();
        for (place, _) in &mut rows {
            place.share = if total > 0. { place.weight / total } else { 0. };
        }
        rows.sort_by(|(a, a_day), (b, b_day)| {
            b.weight.total_cmp(&a.weight).then(b_day.cmp(a_day)).then_with(|| a.folder.cmp(&b.folder))
        });
        rows.into_iter().map(|(place, _)| place).collect()
    }

    /// Forgets everything.
    pub fn forget(&mut self) {
        self.actions.clear();
        self.places.clear();
    }
}

/// Counts one more use of `key` today.
fn count(kept: &mut BTreeMap<String, Use>, key: &str, today: u32) {
    let used = kept.entry(key.to_string()).or_default();
    used.n = used.n.saturating_add(1);
    used.day = today;
}

/// Keeps at most `most` entries: drops the lightest (on a tie, the key that sorts last), never `newest`.
fn trim(kept: &mut BTreeMap<String, Use>, newest: &str, most: usize, today: u32) {
    while kept.len() > most {
        let lightest = kept
            .iter()
            .filter(|(key, _)| key.as_str() != newest)
            .min_by(|(ka, a), (kb, b)| weight(**a, today).total_cmp(&weight(**b, today)).then_with(|| kb.cmp(ka)))
            .map(|(key, _)| key.clone());
        match lightest {
            Some(key) => kept.remove(&key),
            None => break,
        };
    }
}

/// How much a use weighs today: its count, halved for every [`HALF_LIFE_DAYS`] since it was last done.
pub fn weight(used: Use, today: u32) -> f64 {
    // A day in the future counts as today.
    let days = today.saturating_sub(used.day) as f64;
    used.n as f64 * 0.5_f64.powf(days / HALF_LIFE_DAYS)
}

/// The day a time falls on, in days since 1970-01-01 (the time in Unix milliseconds).
pub fn day_of(unix_ms: i64) -> u32 {
    u32::try_from(unix_ms.max(0) / 86_400_000).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: u32 = 20_407;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn a_fresh_habits_is_empty_and_the_json_round_trips() {
        let fresh = Habits::default();
        assert!(fresh.is_empty());
        assert_eq!(serde_json::to_string(&fresh).unwrap(), r#"{"actions":{},"places":{}}"#);
        let json = r#"{"actions":{"pictures":{"do:convert-pictures":{"n":3,"day":20407}}},"places":{"pdf":{"D:\\Papers\\Invoices":{"n":4,"day":20407}}}}"#;
        let read: Habits = serde_json::from_str(json).unwrap();
        assert!(!read.is_empty());
        assert_eq!(read.action_uses(Shape::Pictures, "do:convert-pictures"), 3);
        assert_eq!(serde_json::to_string(&read).unwrap(), json);
        let half: Habits = serde_json::from_str(r#"{"actions":{}}"#).unwrap();
        assert!(half.is_empty());
    }

    #[test]
    fn three_uses_on_one_day_weigh_three_and_fade_by_half_every_thirty_days() {
        let mut h = Habits::default();
        for _ in 0..3 {
            h.note_action(Shape::Archive, "do:extract-here", D);
        }
        assert_eq!(h.action_uses(Shape::Archive, "do:extract-here"), 3);
        assert!(near(h.action_weight(Shape::Archive, "do:extract-here", D), 3.0));
        assert!(near(h.action_weight(Shape::Archive, "do:extract-here", D + 30), 1.5));
        assert!(near(h.action_weight(Shape::Archive, "do:extract-here", D + 60), 0.75));
    }

    #[test]
    fn an_action_noted_again_later_carries_the_new_day() {
        let mut h = Habits::default();
        h.note_action(Shape::Archive, "a", D);
        h.note_action(Shape::Archive, "a", D + 30);
        assert_eq!(h.action_uses(Shape::Archive, "a"), 2);
        assert!(near(h.action_weight(Shape::Archive, "a", D + 30), 2.0));
    }

    #[test]
    fn the_past_never_grows_the_weight_and_the_future_weighs_as_today() {
        let used = Use { n: 2, day: D };
        assert!(weight(used, D + 10) < weight(used, D + 5));
        assert!(near(weight(used, D - 10), 2.0));
        assert!(near(weight(used, D), 2.0));
    }

    #[test]
    fn actions_are_kept_per_shape_and_overall_weight_sums_them() {
        let mut h = Habits::default();
        h.note_action(Shape::Picture, "x", D);
        assert_eq!(h.action_weight(Shape::Pictures, "x", D), 0.);
        h.note_action(Shape::Pictures, "x", D);
        h.note_action(Shape::Pictures, "x", D);
        assert!(near(h.overall_weight("x", D), 3.0));
        assert_eq!(h.overall_weight("never", D), 0.);
    }

    #[test]
    fn an_empty_action_is_ignored() {
        let mut h = Habits::default();
        h.note_action(Shape::File, "", D);
        assert!(h.is_empty());
    }

    #[test]
    fn the_forty_ninth_action_pushes_out_the_lightest_and_stays() {
        let mut h = Habits::default();
        for i in 0..MOST_ACTIONS {
            h.note_action(Shape::Code, &format!("a{i:02}"), D);
        }
        h.note_action(Shape::Code, "new", D);
        assert_eq!(h.actions["code"].len(), MOST_ACTIONS);
        assert_eq!(h.action_uses(Shape::Code, "new"), 1);
        // All were equally heavy, so the one that sorts last went.
        assert_eq!(h.action_uses(Shape::Code, "a47"), 0);
        assert_eq!(h.action_uses(Shape::Code, "a46"), 1);
    }

    #[test]
    fn an_older_action_is_the_lightest_to_go() {
        let mut h = Habits::default();
        h.note_action(Shape::Code, "old", D - 90);
        for i in 1..MOST_ACTIONS {
            h.note_action(Shape::Code, &format!("a{i:02}"), D);
        }
        h.note_action(Shape::Code, "new", D);
        assert_eq!(h.action_uses(Shape::Code, "old"), 0);
        assert_eq!(h.action_uses(Shape::Code, "new"), 1);
    }

    #[test]
    fn the_same_folder_in_another_spelling_is_one_folder_used_twice() {
        let mut h = Habits::default();
        h.note_place("PDF", r"D:\Papers\Invoices\", D);
        h.note_place("pdf", r"d:\papers\invoices", D);
        let places = h.places_for("Pdf", D);
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].folder, r"D:\Papers\Invoices");
        assert_eq!(places[0].uses, 2);
        h.note_place("zip", r"C:\", D);
        assert_eq!(h.places_for("zip", D)[0].folder, r"C:\");
        h.note_place("zip", "", D);
        assert_eq!(h.places_for("zip", D).len(), 1);
    }

    #[test]
    fn places_come_heaviest_first_and_their_shares_add_up_to_one() {
        let mut h = Habits::default();
        h.note_place("jpg", r"D:\A", D - 60);
        for _ in 0..3 {
            h.note_place("jpg", r"D:\B", D);
        }
        h.note_place("jpg", r"D:\C", D);
        let places = h.places_for("jpg", D);
        let names: Vec<&str> = places.iter().map(|p| p.folder.as_str()).collect();
        assert_eq!(names, [r"D:\B", r"D:\C", r"D:\A"]);
        assert!(near(places.iter().map(|p| p.share).sum::<f64>(), 1.0));
        assert!(h.places_for("never", D).is_empty());
    }

    #[test]
    fn places_are_trimmed_to_the_most_kept() {
        let mut h = Habits::default();
        for i in 0..=MOST_PLACES {
            h.note_place("txt", &format!(r"D:\f{i:02}"), D);
        }
        assert_eq!(h.places["txt"].len(), MOST_PLACES);
        assert!(h.places["txt"].contains_key(r"D:\f16"));
    }

    #[test]
    fn forget_empties_everything() {
        let mut h = Habits::default();
        h.note_action(Shape::Video, "a", D);
        h.note_place("mp4", r"D:\V", D);
        assert!(!h.is_empty());
        h.forget();
        assert!(h.is_empty());
    }

    #[test]
    fn days_count_from_1970_and_never_go_negative() {
        assert_eq!(day_of(0), 0);
        assert_eq!(day_of(86_400_000), 1);
        assert_eq!(day_of(86_399_999), 0);
        assert_eq!(day_of(-5_000_000_000), 0);
        assert_eq!(day_of(i64::MAX), u32::MAX);
    }
}
