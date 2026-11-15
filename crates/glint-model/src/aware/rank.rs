//! What to put forward: the rows of a menu that fit what it is for, by rule and by habit.
//!
//! Card A3 of docs/gpui/aware fills this in: the signatures are the contract, the bodies below do nothing yet.

use super::facts::Facts;
use super::habits::{Habits, Place};
use super::probed::Probed;

/// Why a row is put forward.
#[derive(Clone, Debug, PartialEq)]
pub enum Why {
    /// It is the plain next step for this kind of thing: no words needed.
    Plain,
    /// An app has the file open: its name.
    InUse(String),
    /// It was downloaded: the site, "" when unknown.
    FromInternet(String),
    /// Nobody signed it.
    Unsigned,
    /// A build left this many bytes in the folder.
    BuildOutput(u64),
    /// The person often does this with this kind of thing.
    Usual,
    /// Where the person usually puts this kind of thing.
    UsualPlace,
}

/// A row put forward.
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    /// The row's key (`Run::key` in glint-ui).
    pub key: String,
    pub why: Why,
    pub score: f64,
}

/// The rows to put forward, best first, at most `most`. `available` are the keys of the menu's rows that can be
/// chosen, in the menu's order; `probed` is the one target's answers (None for several targets).
pub fn suggest(
    facts: &Facts,
    probed: Option<&Probed>,
    available: &[&str],
    habits: &Habits,
    today: u32,
    most: usize,
) -> Vec<Pick> {
    let _ = (facts, probed, available, habits, today, most);
    Vec::new()
}

/// A row everyone knows where to find (Cut, Copy, Delete, Properties…): never put forward.
pub fn is_basic(key: &str) -> bool {
    let _ = key;
    false
}

/// The folder this kind of thing usually goes to, if one clearly is: used three times or more, and with six
/// tenths or more of the weight.
pub fn usual_place(places: &[Place]) -> Option<&Place> {
    let _ = places;
    None
}
