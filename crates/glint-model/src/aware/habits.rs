//! What the person did before: counts of the actions chosen for each kind of thing, and of the folders each kind
//! of file was moved or copied to. They fade with time, and stay small.
//!
//! Card A2 of docs/gpui/aware fills this in: the signatures and the JSON shape are the contract, the bodies below
//! do nothing yet (except `is_empty`, which the saved state already relies on).

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
        let _ = (shape, action, today);
    }

    /// Counts `folder` as where a file with extension `ext` went today.
    pub fn note_place(&mut self, ext: &str, folder: &str, today: u32) {
        let _ = (ext, folder, today);
    }

    /// How much `action` weighs today for `shape` (0 when never chosen).
    pub fn action_weight(&self, shape: Shape, action: &str, today: u32) -> f64 {
        let _ = (shape, action, today);
        0.
    }

    /// How many times `action` was chosen for `shape`.
    pub fn action_uses(&self, shape: Shape, action: &str) -> u32 {
        let _ = (shape, action);
        0
    }

    /// How much `action` weighs today over every shape (the command palette's order).
    pub fn overall_weight(&self, action: &str, today: u32) -> f64 {
        let _ = (action, today);
        0.
    }

    /// The folders files with extension `ext` have gone to, heaviest first.
    pub fn places_for(&self, ext: &str, today: u32) -> Vec<Place> {
        let _ = (ext, today);
        Vec::new()
    }

    /// Forgets everything.
    pub fn forget(&mut self) {
        self.actions.clear();
        self.places.clear();
    }
}

/// How much a use weighs today: its count, halved for every [`HALF_LIFE_DAYS`] since it was last done.
pub fn weight(used: Use, today: u32) -> f64 {
    let _ = (used, today);
    0.
}

/// The day a time falls on, in days since 1970-01-01 (the time in Unix milliseconds).
pub fn day_of(unix_ms: i64) -> u32 {
    let _ = unix_ms;
    0
}
