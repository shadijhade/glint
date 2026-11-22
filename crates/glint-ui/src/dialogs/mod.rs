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

/// What the file-action dialogs share: where results go, runs, Undo (ActionParts.tsx).
pub mod action_run;
/// Backups with restic (BackupDialog.tsx).
pub mod backup;
/// Renaming several items at once (BatchRenameDialog.tsx).
pub mod batch_rename;
/// Scanning files with Microsoft Defender (CheckDialog.tsx).
pub mod check;
/// Checking the files a checksum file lists (a toast with what doesn't match).
pub mod check_sums;
/// Cleaning up a drive or folder (CleanupDialog.tsx).
pub mod cleanup;
/// Connecting a cloud through rclone (CloudSetup.tsx).
pub mod cloud_setup;
/// Comparing two files and merging their changes (CompareFilesDialog.tsx).
pub mod compare_files;
/// Compressing in place with Windows' WOF (CompressInPlaceDialog.tsx).
pub mod compress_in_place;
/// Converting pictures (ConvertDialog.tsx).
pub mod convert;
/// Data tools: query, convert, profile (DataTools.tsx).
pub mod data_tools;
/// Converting documents (DocConvertDialog.tsx).
pub mod doc_convert;
pub mod drive_health;
/// Duplicate files (DuplicatesDialog.tsx).
pub mod duplicates;
/// A file's history (HistoryDialog.tsx).
pub mod history;
/// What holds a file open, and stopping it (LocksDialog.tsx).
pub mod locks;
/// Media files: convert, trim, extract, subtitles (MediaDialog.tsx).
pub mod media;
/// Media tools for several files (MediaTools.tsx).
pub mod media_tools;
/// A file's metadata, read and cleaned (MetadataDialog.tsx).
pub mod metadata;
/// A password asked for or set (PasswordDialog.tsx).
pub mod password;
/// PDF tools (PdfDialog.tsx).
pub mod pdf;
/// Adding and editing a remote connection (RemoteDialog.tsx).
pub mod remote;
/// Folder rules (RulesDialog.tsx).
pub mod rules;
/// Sharing files: to phones and PCs nearby (LocalSend), and through Windows' own Share.
pub mod share;
/// The keyboard shortcuts sheet (ShortcutsDialog.tsx).
pub mod shortcuts;
/// Making files smaller (ShrinkDialog.tsx).
pub mod shrink;
/// A song's tags (SongDialog.tsx).
pub mod song;
/// Tags for files and folders (TagsDialog.tsx).
pub mod tags;
/// Recent changes on a drive (TimelineDialog.tsx).
pub mod timeline;
/// Previous versions of a file or folder (VersionsDialog.tsx).
pub mod versions;
/// A file's X-Ray (XRayDialog.tsx).
pub mod xray;

/// Makes F5 run `refresh` on `view` for as long as the view lives: in a dialog with a Refresh button the key does
/// what the button does (Modal.tsx's header took it). The dialog layer sits outside the workspace, so the window's
/// own F5 never reaches a dialog; and no focus has to be inside the view, as with the dialog's own buttons.
pub fn refresh_on_f5<V: 'static>(
    cx: &mut Context<V>,
    refresh: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) {
    let view = cx.weak_entity();
    let keys = cx.intercept_keystrokes(move |event, window, cx| {
        let modifiers = event.keystroke.modifiers;
        if event.keystroke.key != "f5" || modifiers.modified() {
            return;
        }
        cx.stop_propagation();
        let _ = view.update(cx, |this, cx| refresh(this, window, cx));
    });
    // Alive as long as the view: released with it.
    cx.on_release(move |_, _| drop(keys)).detach();
}

/// What Enter does in a dialog: its main button's action, kept per dialog view.
type MainAction = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// The main actions of the dialogs on screen, by their view's id. A dialog declares its own while it renders
/// ([`main_action`]); the dialog layer's Enter ([`show`]) runs the one of its view.
#[derive(Default)]
struct MainActions(std::collections::HashMap<EntityId, MainAction>);

impl Global for MainActions {}

/// Declares what Enter does in the dialog `cx` renders: the same as its main button (the one at the footer's end),
/// `enabled` while that button can be pressed. Call it in `render`, next to the button, with the same closure the
/// button's click runs, so Enter and the click can't drift apart. Enter in a text box already does it when the
/// dialog listens to the box ([`enter_runs_main`]); Enter on a button presses that button.
pub fn main_action<V: 'static>(
    cx: &mut Context<V>,
    enabled: bool,
    run: impl Fn(&mut V, &mut Window, &mut Context<V>) + 'static,
) {
    let id = cx.entity_id();
    if !enabled {
        cx.default_global::<MainActions>().0.remove(&id);
        return;
    }
    let view = cx.weak_entity();
    let action: MainAction = std::rc::Rc::new(move |window, cx| {
        if let Some(view) = view.upgrade() {
            view.update(cx, |this, cx| run(this, window, cx));
        }
    });
    cx.default_global::<MainActions>().0.insert(id, action);
}

/// Says the dialog `cx` renders has no main action now (a render calls it first, then [`main_action`] for the state
/// that has one), so what an earlier state declared doesn't stay.
pub fn no_main<V: 'static>(cx: &mut Context<V>) {
    let id = cx.entity_id();
    cx.default_global::<MainActions>().0.remove(&id);
}

/// Runs the main action of the dialog `view`, if it has one that can be pressed now. Whether it did.
pub fn run_main(view: EntityId, window: &mut Window, cx: &mut App) -> bool {
    let action = cx.default_global::<MainActions>().0.get(&view).cloned();
    match action {
        Some(action) => {
            action(window, cx);
            true
        }
        None => false,
    }
}

/// Makes Enter in the single-line `input` run the dialog's main action ([`main_action`]), the way its button does.
/// For a dialog that doesn't already listen to the box's `PressEnter` itself.
pub fn enter_runs_main<V: 'static>(
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut Context<V>,
) -> Subscription {
    let id = cx.entity_id();
    cx.subscribe_in(input, window, move |_, _, event: &InputEvent, window, cx| {
        if matches!(event, InputEvent::PressEnter { .. }) {
            run_main(id, window, cx);
        }
    })
}

/// Whether the keyboard is in a text box: Enter there belongs to the box (and the dialog's own listener of it).
fn typing(window: &Window) -> bool {
    window.context_stack().iter().any(|context| context.contains("Input"))
}

/// Shows `view` in the window's dialog layer, titled `title`, `width` wide.
///
/// Every dialog shows the same way: it opens with the dialog layer's quiet pop and leaves with a fade, Escape
/// closes it, it is never taller than the window (its body scrolls), and Enter, when the keyboard isn't in a text
/// box, does what its main button does ([`main_action`]).
///
/// gpui-component keeps the dialog's builder and calls it again when the dialog layer rebuilds, so the
/// view is created once, before this call, and the builder only clones its handle.
pub fn show<V: Render>(window: &mut Window, cx: &mut App, title: SharedString, width: Pixels, view: Entity<V>) {
    show_unless(window, cx, title, width, view, |_| false);
}

/// [`show`] for a dialog that can't be left while `busy` says so (a run under way): Escape, the close button and
/// the backdrop do nothing then.
pub fn show_unless<V: Render>(
    window: &mut Window,
    cx: &mut App,
    title: SharedString,
    width: Pixels,
    view: Entity<V>,
    busy: impl Fn(&App) -> bool + 'static,
) {
    let busy = std::rc::Rc::new(busy);
    let id = view.entity_id();
    // The entry goes with the view, so a closed dialog leaves nothing behind.
    cx.observe_release(&view, move |_, cx| {
        cx.default_global::<MainActions>().0.remove(&id);
    })
    .detach();
    window.open_dialog(cx, move |dialog, _window, _cx| {
        // Enter in a text box belongs to the box (the dialog listens to its PressEnter); anywhere else in the
        // dialog it is the main button. Returning false keeps the dialog open: the main action closes it itself.
        dialog
            .title(title.clone())
            .w(width)
            .on_cancel({
                let busy = busy.clone();
                move |_, _, cx| !busy(cx)
            })
            .on_ok(move |_, window, cx| {
                if !typing(window) {
                    run_main(id, window, cx);
                }
                false
            })
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
        // Enter is the main button, as a click on it is: closes the dialog, then runs.
        let on_enter = on_ok.clone();
        let ok_button = Button::new("confirm-ok").label(ok.clone());
        let ok_button = if danger {
            ok_button.danger()
        } else {
            ok_button.primary()
        };
        dialog
            .title(title.clone())
            .w(px(440.))
            // Enter is the main button, unless that button destroys something that can't be taken back: then
            // only a deliberate click does it, and Enter does nothing (Escape still cancels).
            .on_ok(move |_, window, cx| {
                if !danger {
                    window.close_dialog(cx);
                    on_enter(window, cx);
                }
                false
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::Cell;
    use std::rc::Rc;

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// A dialog with a main button (counted), and a body as tall as `rows` lines.
    struct Probe {
        main: Rc<Cell<usize>>,
        enabled: Rc<Cell<bool>>,
        rows: usize,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let counted = self.main.clone();
            super::main_action(cx, self.enabled.get(), move |_, _, _| counted.set(counted.get() + 1));
            col()
                .id("probe")
                .role(Role::Group)
                .aria_label("Probe")
                .test_support()
                .children((0..self.rows).map(|row| div().h(px(24.)).child(format!("row {row}"))))
        }
    }

    /// The window, how often the main action ran, whether it is enabled, and the dialog's view.
    type Opened = (VisualTestContext, Rc<Cell<usize>>, Rc<Cell<bool>>, Entity<Probe>);

    fn open(
        cx: &mut TestAppContext,
        rows: usize,
        enabled: bool,
    ) -> Opened {
        cx.update(testing::init);
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let (main, flag) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(enabled)));
        let view = {
            let (main, flag) = (main.clone(), flag.clone());
            visual.update(|window, cx| {
                let view = cx.new(|_| Probe {
                    main,
                    enabled: flag,
                    rows,
                });
                show(window, cx, "Probe".into(), px(400.), view.clone());
                view
            })
        };
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
        (visual, main, flag, view)
    }

    #[gpui_kit::test]
    fn enter_runs_the_main_action_and_leaves_the_dialog_to_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, main, _, _) = open(cx, 3, true);
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert_eq!(main.get(), 1, "Enter did what the main button does");
        // The dialog stays until the action closes it.
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn enter_does_nothing_while_the_main_button_is_disabled(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, main, flag, view) = open(cx, 3, false);
        visual.update(|window, cx| window.press("enter", cx));
        assert_eq!(main.get(), 0);
        flag.set(true);
        visual.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        });
        assert_eq!(main.get(), 1, "enabled again, Enter works");
    }

    #[gpui_kit::test]
    fn escape_closes_a_dialog(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, main, _, _) = open(cx, 3, true);
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        assert_eq!(main.get(), 0, "Escape cancels: it doesn't do the main action");
    }

    #[gpui_kit::test]
    fn a_dialog_is_never_taller_than_the_window(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _, _, _) = open(cx, 200, true);
        let (surface, window_height) = visual.update(|window, _| {
            (
                window.find("surface").bounds().size.height,
                window.viewport_size().height,
            )
        });
        assert!(surface <= window_height, "{surface:?} fits in {window_height:?}");
    }

    #[gpui_kit::test]
    fn a_dialog_that_cannot_act_has_no_main_action(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, _, flag, view) = open(cx, 3, true);
        let id = view.entity_id();
        visual.update(|_, cx| assert!(cx.default_global::<MainActions>().0.contains_key(&id)));
        flag.set(false);
        visual.update(|_, cx| view.update(cx, |_, cx| cx.notify()));
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
        visual.update(|_, cx| assert!(!cx.default_global::<MainActions>().0.contains_key(&id)));
    }

    fn ask(cx: &mut TestAppContext, danger: bool) -> (VisualTestContext, Rc<Cell<usize>>) {
        cx.update(testing::init);
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let ran = Rc::new(Cell::new(0));
        let counted = ran.clone();
        visual.update(|window, cx| {
            confirm(window, cx, "Sure?".into(), "It goes.".into(), "Do it".into(), danger, move |_, _| {
                counted.set(counted.get() + 1)
            })
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
        (visual, ran)
    }

    #[gpui_kit::test]
    fn enter_confirms_a_plain_question(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, ran) = ask(cx, false);
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert_eq!(ran.get(), 1);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn enter_never_confirms_a_danger_question_but_escape_cancels(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, ran) = ask(cx, true);
        visual.update(|window, cx| window.press("enter", cx));
        visual.run_until_parked();
        assert_eq!(ran.get(), 0, "Enter ran nothing");
        visual.update(|window, cx| assert!(window.has_active_dialog(cx), "and left it open"));
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        assert_eq!(ran.get(), 0);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }
}
