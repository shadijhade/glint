//! The keys that work inside the address bar's own fields: the arrows through the suggestions and menus, Tab
//! to complete, Escape to give up.
//!
//! They are bound here, not in `actions::BINDINGS`: they only mean something while one of these fields has
//! the focus, and the shortcut sheet lists commands, not what a text field does with Tab. Each binding
//! names the field's own key context first (`AddressEdit > Input`), so it is deeper than the text input's
//! plain `Input` bindings (Up, Down, Tab and Escape do nothing useful in a one-line field) and, added later,
//! wins over them.

use crate::kit::prelude::*;

actions!(
    glint_address,
    [
        /// Up in a list of suggestions or a menu.
        ListUp,
        /// Down in a list of suggestions or a menu.
        ListDown,
        /// Home in a menu: its first item.
        ListFirst,
        /// End in a menu: its last item.
        ListLast,
        /// Tab in a field with suggestions: take the highlighted one (or the first) into the field.
        ListComplete,
        /// Enter in a menu: open the highlighted item.
        ListAccept,
        /// Escape: give up what was typed, or close the menu.
        ListCancel,
    ]
);

/// The key context of the address field while it is being typed in.
pub(super) const EDIT: &str = "AddressEdit";
/// The key context of the menu a chevron opens.
pub(super) const FLYOUT: &str = "AddressFlyout";
/// The key context of the search box.
pub(super) const FIND: &str = "AddressFind";

/// Marks that the keys are bound in this app.
struct Bound;

impl Global for Bound {}

/// Binds the keys, once per app (a second view of the bar finds them bound).
pub(super) fn ensure_bound(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    cx.bind_keys([
        KeyBinding::new("up", ListUp, Some("AddressEdit > Input")),
        KeyBinding::new("down", ListDown, Some("AddressEdit > Input")),
        KeyBinding::new("tab", ListComplete, Some("AddressEdit > Input")),
        KeyBinding::new("escape", ListCancel, Some("AddressEdit > Input")),
        KeyBinding::new("up", ListUp, Some("AddressFlyout")),
        KeyBinding::new("down", ListDown, Some("AddressFlyout")),
        KeyBinding::new("home", ListFirst, Some("AddressFlyout")),
        KeyBinding::new("end", ListLast, Some("AddressFlyout")),
        KeyBinding::new("enter", ListAccept, Some("AddressFlyout")),
        KeyBinding::new("escape", ListCancel, Some("AddressFlyout")),
        // The menu's filter box: the arrows and Enter still drive the list under it.
        KeyBinding::new("up", ListUp, Some("AddressFlyout > Input")),
        KeyBinding::new("down", ListDown, Some("AddressFlyout > Input")),
        KeyBinding::new("enter", ListAccept, Some("AddressFlyout > Input")),
        KeyBinding::new("escape", ListCancel, Some("AddressFlyout > Input")),
        // The search box: Tab takes the filters it read from plain words, Down goes to the list, Escape
        // clears and leaves.
        KeyBinding::new("tab", ListComplete, Some("AddressFind > Input")),
        KeyBinding::new("down", ListDown, Some("AddressFind > Input")),
        KeyBinding::new("escape", ListCancel, Some("AddressFind > Input")),
    ]);
}
