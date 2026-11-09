//! Dialogs: one file each, all built the way [`drive_health`] is. Read that file first.
//!
//! The pattern every dialog follows:
//!
//! 1. **A model entity** holds the data and its state (loading, failed, ready). It calls glint-core through
//!    `core_bridge::run`, so the UI thread never waits. Dialogs that share data with other views (Drive
//!    health also feeds a badge and a banner) keep one model in a global; a dialog of its own keeps it in
//!    its view.
//! 2. **A view entity** renders the model with kit controls, theme tokens and `t!`, mirrored for right to
//!    left, with an accessibility role and name on everything that has meaning.
//! 3. **`open(window, cx)`** shows the view in the window's dialog layer with [`show`]: Escape closes it,
//!    focus goes into it, and the window behind is covered.
//! 4. **A headless test** opens it with fake data and checks the rows and their accessibility labels.
//!    Fake data comes from a `Source` the model takes (a function returning a future), so no test touches
//!    the real core.
//! 5. **An action** (`ShowDriveHealth`) and, where it makes sense, a button, open it. Register the action's
//!    handler on the workspace.

use crate::kit::prelude::*;

pub mod drive_health;
/// The keyboard shortcuts sheet (ShortcutsDialog.tsx). Wave 5 slot.
#[cfg(feature = "dialog-shortcuts")]
pub mod shortcuts;
/// A file's history (HistoryDialog.tsx). Wave 5 slot.
#[cfg(feature = "dialog-history")]
pub mod history;
/// What holds a file open, and stopping it (LocksDialog.tsx). Wave 5 slot.
#[cfg(feature = "dialog-locks")]
pub mod locks;
/// Previous versions of a file or folder (VersionsDialog.tsx). Wave 5 slot.
#[cfg(feature = "dialog-versions")]
pub mod versions;

/// Shows `view` in the window's dialog layer, titled `title`, `width` wide.
///
/// gpui-component keeps the dialog's builder and calls it again when the dialog layer rebuilds, so the
/// view is created once, before this call, and the builder only clones its handle.
pub fn show<V: Render>(window: &mut Window, cx: &mut App, title: SharedString, width: Pixels, view: Entity<V>) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        dialog.title(title.clone()).w(width).child(view.clone())
    });
}
