//! The island's next step, and what Paste would paste, in words (docs/gpui/aware/PLAN.md, F8 and F9).
//!
//! The words are plain functions of what is on the clipboard, so the bar and its tests read them the same way.
//! The next step is the first row the menus would put forward for the selection (the same ranking as the menu's
//! Suggested section), asked of the menu host, which keeps nothing of its own for it.

use crate::components::menus::{self, Suggested};
use crate::kit::prelude::*;
use crate::t;
use glint_model::aware::Clip;
use glint_model::format::format_count;
use glint_model::paths::base_name;

/// How many names Paste's tooltip lists before it says how many more there are.
const NAMES: usize = 3;

/// What Paste would do with what is on the clipboard: "Paste 3 files", "Move 1 file here", "Paste picture". None
/// when the clipboard holds nothing to say it for. The clipboard may hold folders too; "files" is said for both, as
/// Windows does. Text is explained in the tooltip only: a button that says "Paste text" would shout at every copy.
pub fn paste_words(clip: &Clip) -> Option<SharedString> {
    if matches!(clip, Clip::Picture) {
        return Some(t!("Paste picture"));
    }
    let Clip::Files { count, cut, .. } = clip else {
        return None;
    };
    let n = format_count(*count);
    Some(match (*cut, *count == 1) {
        (false, true) => t!("Paste 1 file"),
        (false, false) => t!("Paste {n} files", n = n),
        (true, true) => t!("Move 1 file here"),
        (true, false) => t!("Move {n} files here", n = n),
    })
}

/// The Paste button's tooltip while something is on the clipboard. `keys` is Paste's key as text ("Ctrl+V").
pub fn paste_tip(clip: &Clip, keys: &str) -> SharedString {
    let Clip::Files { count, cut, from, names } = clip else {
        return match clip {
            Clip::Picture => t!("Paste picture as a new PNG file ({keys})", keys = keys),
            Clip::Text => t!("Paste text as a new .txt file ({keys})", keys = keys),
            _ => t!("Paste ({keys})", keys = keys),
        };
    };
    let mut listed = names.iter().take(NAMES).cloned().collect::<Vec<_>>().join(", ");
    if *count > NAMES {
        listed = t!("{names} and {n} more", names = listed, n = format_count(count - NAMES)).to_string();
    }
    // A drive's root has no name of its own besides its letter: it is said whole ("C:\\").
    let folder = from.as_deref().map(|from| match base_name(from) {
        name if name.ends_with(':') => from,
        name => name,
    });
    match (*cut, folder) {
        (false, Some(folder)) => t!("Paste ({keys}): {names}, from {folder}", keys = keys, names = listed, folder = folder),
        (false, None) => t!("Paste ({keys}): {names}", keys = keys, names = listed),
        (true, Some(folder)) => t!(
            "Move here ({keys}): {names}, from {folder}",
            keys = keys,
            names = listed,
            folder = folder
        ),
        (true, None) => t!("Move here ({keys}): {names}", keys = keys, names = listed),
    }
}

/// The first suggestion for what is selected in `pane`, if the menus have one.
pub fn next_step(cx: &App, pane: usize) -> Option<Suggested> {
    menus::host(cx)?.read(cx).suggestions(pane, 1, cx).into_iter().next()
}
