//! What the selection weighs and holds, said in the dock's words (docs/gpui/aware/PLAN.md, F10).
//!
//! Card F2 of docs/gpui/aware fills this in: the signatures are the contract; until then the words are the
//! counts the bar already says.

use super::summary::{counts, Summary};
use crate::kit::prelude::*;
use glint_model::aware::Probed;
use glint_model::entries::FileEntry;

/// What is known about the selection beyond its count and its files' bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deep {
    /// Everything selected, folders' insides included; None when no folder is selected or one isn't known yet.
    pub bytes: Option<u64>,
    /// The files inside the selected folders; None when not known.
    pub inside: Option<u64>,
    /// "1,230 lines", "12 min 31 s", "1920 × 1080", "48 pages".
    pub detail: Option<SharedString>,
    /// Every folder's inside is finished (the numbers have stopped growing).
    pub done: bool,
}

/// The most selected items looked into.
pub const MOST: usize = 64;

/// What is known about `selected`, given what was found out about each.
pub fn deep(selected: &[FileEntry], probed: &dyn Fn(&FileEntry) -> Option<Probed>) -> Deep {
    let _ = (selected, probed);
    Deep::default()
}

/// The dock's start words: [`counts`], with what `deep` knows.
pub fn counts_deep(s: &Summary, deep: &Deep) -> SharedString {
    let _ = deep;
    counts(s)
}
