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

/// Renaming several items at once (BatchRenameDialog.tsx).
pub mod batch_rename;
pub mod drive_health;
/// A file's history (HistoryDialog.tsx).
pub mod history;
/// What holds a file open, and stopping it (LocksDialog.tsx).
pub mod locks;
/// A password asked for or set (PasswordDialog.tsx).
pub mod password;
/// The keyboard shortcuts sheet (ShortcutsDialog.tsx).
pub mod shortcuts;
/// Tags for files and folders (TagsDialog.tsx).
pub mod tags;
/// Encrypting and decrypting with age (VaultDialog.tsx).
pub mod vault;
/// Previous versions of a file or folder (VersionsDialog.tsx).
pub mod versions;

/// Shows `view` in the window's dialog layer, titled `title`, `width` wide.
///
/// gpui-component keeps the dialog's builder and calls it again when the dialog layer rebuilds, so the
/// view is created once, before this call, and the builder only clones its handle.
pub fn show<V: Render>(window: &mut Window, cx: &mut App, title: SharedString, width: Pixels, view: Entity<V>) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        // Enter belongs to the dialog's own boxes and buttons, not to the dialog layer (which would close it).
        dialog
            .title(title.clone())
            .w(width)
            .on_ok(|_, _, _| false)
            .child(view.clone())
    });
}

/// Asks before something that can't be taken back (deleting for good, emptying the Recycle Bin): `title`,
/// `message`, Cancel and `ok` (drawn as a danger button when `danger`). `on_ok` runs after the dialog closes.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: SharedString,
    message: SharedString,
    ok: SharedString,
    danger: bool,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let on_ok = std::rc::Rc::new(on_ok);
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let on_ok = on_ok.clone();
        let ok_button = Button::new("confirm-ok").label(ok.clone());
        let ok_button = if danger {
            ok_button.danger()
        } else {
            ok_button.primary()
        };
        dialog
            .title(title.clone())
            .w(px(440.))
            .child(
                div()
                    .id("confirm")
                    .role(Role::Group)
                    .aria_label(title.clone())
                    .aria_description(message.clone())
                    .test_support()
                    .text_start()
                    .child(message.clone()),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("confirm-cancel")
                            .label(crate::t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(ok_button.on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        on_ok(window, cx);
                    })),
            )
    });
}
