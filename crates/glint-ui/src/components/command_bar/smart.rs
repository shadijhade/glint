//! The island's next step, and what Paste would paste, in words (docs/gpui/aware/PLAN.md, F8 and F9).
//!
//! Card F1 of docs/gpui/aware fills this in: the signatures are the contract; until then there is no next step
//! and Paste has no words.

use crate::components::menus::Suggested;
use crate::kit::prelude::*;
use glint_model::aware::Clip;

/// What Paste would do with what is on the clipboard: "Paste 3 files", "Move 1 file here". None when the
/// clipboard holds no files.
pub fn paste_words(clip: &Clip) -> Option<SharedString> {
    let _ = clip;
    None
}

/// The Paste button's tooltip while files are on the clipboard. `keys` is Paste's key as text ("Ctrl+V").
pub fn paste_tip(clip: &Clip, keys: &str) -> SharedString {
    let _ = (clip, keys);
    SharedString::default()
}

/// The first suggestion for what is selected in `pane`, if the menus have one.
pub fn next_step(cx: &App, pane: usize) -> Option<Suggested> {
    let _ = (cx, pane);
    None
}
