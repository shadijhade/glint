//! What a menu's head says: the item's name, a line of facts, and its badges (docs/gpui/aware/PLAN.md, F1).
//!
//! Card E2 of docs/gpui/aware fills this in: the signatures are the contract; until then a head has a title only.

use super::spec::{Head, HeadPicture};
use crate::kit::prelude::*;
use glint_model::aware::Probed;
use glint_model::entries::FileEntry;

/// What the head needs that isn't in the entries or the answers.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    /// Now, in Unix milliseconds (for "Changed …").
    pub now: i64,
    /// Git's word for the one target: "Changed" or "New".
    pub git: Option<SharedString>,
}

/// The head of the menu for `targets`. `probed` is the one target's answers (used only with exactly one).
pub fn head_of(targets: &[FileEntry], probed: Option<&Probed>, extras: &Extras) -> Head {
    let _ = (probed, extras);
    Head {
        title: targets.first().map(|entry| SharedString::from(entry.name.clone())).unwrap_or_default(),
        picture: HeadPicture::Several,
        line: SharedString::default(),
        badges: Vec::new(),
        changed: None,
    }
}

/// The head of a folder's own menu: its name, how many items it shows, and its Git branch.
pub fn folder_head(name: SharedString, items: Option<usize>, branch: Option<SharedString>) -> Head {
    let _ = (items, branch);
    Head {
        title: name,
        picture: HeadPicture::Folder,
        line: SharedString::default(),
        badges: Vec::new(),
        changed: None,
    }
}
