//! The keys that work inside the palette: the arrows through the results, Enter to take one, Escape to give up.
//!
//! Bound here, not in `actions::BINDINGS`: they only mean something while the palette has the focus, and the
//! shortcut sheet lists commands, not what a text field does with Tab. Each names the palette's key context
//! first (`Palette > Input`), so it is deeper than the text input's own bindings (Up, Down, Tab and Escape do
//! nothing useful in a one-line field) and, added later, wins over them.

use crate::kit::prelude::*;

actions!(
    glint_palette,
    [
        /// Down: the next result.
        PaletteDown,
        /// Up: the previous result.
        PaletteUp,
        /// Enter: take the highlighted result.
        PaletteAccept,
        /// Escape, or Ctrl+K again: close.
        PaletteCancel,
        /// Tab: nothing. The box keeps the keyboard.
        PaletteKeep,
    ]
);

/// The palette's key context.
pub(super) const CONTEXT: &str = "Palette";

/// Marks that the keys are bound in this app.
struct Bound;

impl Global for Bound {}

/// Binds the keys, once per app.
pub(super) fn ensure_bound(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    let mut bindings = Vec::new();
    for context in ["Palette", "Palette > Input"] {
        bindings.extend([
            KeyBinding::new("down", PaletteDown, Some(context)),
            KeyBinding::new("up", PaletteUp, Some(context)),
            KeyBinding::new("enter", PaletteAccept, Some(context)),
            KeyBinding::new("escape", PaletteCancel, Some(context)),
            KeyBinding::new("ctrl-k", PaletteCancel, Some(context)),
            KeyBinding::new("tab", PaletteKeep, Some(context)),
        ]);
    }
    cx.bind_keys(bindings);
}
