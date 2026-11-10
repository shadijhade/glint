//! What the file operations tell the person, worded as the web app's toasts were: the explorer's
//! [`OpNotice`]s as notifications (with an Undo button where one is offered), the slow-work notice and the
//! taskbar button while an operation runs ([`BusyEvent`]), and the questions asked before acting ([`Ask`]).
//!
//! The workspace calls these from its subscription to the explorer, with the main window at hand.

use crate::explorer::{self, Ask, BusyEvent, Failing, FindNotice, OpNotice, OtherPane, QueueKind, UndoId, Working};
use crate::kit::prelude::*;
use crate::platform::window_state::{self, ProgressState};
use crate::{dialogs, t};
use glint_model::format::plural;
use glint_model::paths::base_name;
use std::time::Duration;

/// The notification that offers Undo (the web app's toast id "undo"), keyed by its step.
struct UndoToast;
/// What undoing said (id "undone"): a newer one replaces it.
struct UndoneToast;
/// "Emptied the Recycle Bin" (id "emptied").
struct EmptiedToast;
/// The slow-work notice, keyed by the operation.
struct BusyToast;

/// How long the Undo offer stays, as the web app's 8 s toast.
const UNDO_FOR: Duration = Duration::from_secs(8);

fn name_of(path: &str) -> String {
    match base_name(path) {
        "" => path.to_string(),
        name => name.to_string(),
    }
}

/// "3 items" (`plural(count, "item")`).
fn items(count: usize) -> String {
    plural(count, "item")
}

/// Shows what an operation says. Call with the main window.
pub fn show(notice: &OpNotice, window: &mut Window, cx: &mut App) {
    let undo_offer = |message: SharedString, undo: UndoId| {
        Notification::success(message)
            .id1::<UndoToast>(undo.0 as usize)
            .action(move |_, _, _| {
                Button::new("undo").label(t!("Undo")).on_click(move |_, window, cx| {
                    explorer::explorer(cx).update(cx, |e, cx| e.undo(Some(undo), cx).detach());
                    window.remove_notification1::<UndoToast>(undo.0 as usize, cx);
                })
            })
    };
    let note = match notice {
        OpNotice::DismissUndo => {
            window.remove_notification::<UndoToast>(cx);
            return;
        }
        OpNotice::Copied { count, dest, undo } => {
            let message = t!("Copied {what} to “{dest}”", what = items(*count), dest = name_of(dest));
            match undo {
                Some(undo) => return offer_undo(undo_offer(message, *undo), *undo, window, cx),
                None => Notification::success(message),
            }
        }
        OpNotice::Synced { count, undo } => {
            return offer_undo(
                undo_offer(t!("Synced {what}", what = items(*count)), *undo),
                *undo,
                window,
                cx,
            )
        }
        OpNotice::Moved { count, dest, undo } => {
            let message = t!("Moved {what} to “{dest}”", what = items(*count), dest = name_of(dest));
            return offer_undo(undo_offer(message, *undo), *undo, window, cx);
        }
        OpNotice::Renamed { to, undo } => {
            let message = t!("Renamed to “{name}”", name = name_of(to));
            return offer_undo(undo_offer(message, *undo), *undo, window, cx);
        }
        OpNotice::Recycled { count, undo } => {
            let message = t!("Moved {what} to the Recycle Bin", what = items(*count));
            return offer_undo(undo_offer(message, *undo), *undo, window, cx);
        }
        OpNotice::Restored { count, undo } => {
            let message = t!("Restored {what}", what = items(*count));
            return offer_undo(undo_offer(message, *undo), *undo, window, cx);
        }
        OpNotice::Undid { label } => {
            Notification::success(t!("Undid the {label}", label = label.clone())).id::<UndoneToast>()
        }
        OpNotice::UndoFailed { label, message } => Notification::error(message.clone())
            .title(t!("Couldn't undo the {label}", label = label.clone()))
            .id::<UndoneToast>(),
        OpNotice::NothingToUndo => Notification::info(t!("Nothing to undo")).id::<UndoneToast>(),
        OpNotice::OnClipboard { count, cut } => {
            let message = if *cut {
                t!("Cut {what}", what = items(*count))
            } else {
                t!("Copied {what}", what = items(*count))
            };
            // The web app's 1.5 s toast: the kit's own autohide is short enough for it.
            Notification::success(message)
        }
        OpNotice::EmptiedBin => Notification::success(t!("Emptied the Recycle Bin")).id::<EmptiedToast>(),
        OpNotice::NoFilesOnClipboard => Notification::info(t!("Copy or cut some first (Ctrl+C, Ctrl+X)."))
            .title(t!("There are no files on the clipboard")),
        OpNotice::NothingToPaste => {
            Notification::info(t!("Copy files, a picture or text first, here or in another app."))
                .title(t!("Nothing to paste"))
        }
        OpNotice::PastedInto { made, dest } => Notification::success(t!(
            "Pasted “{made}” into “{dest}”",
            made = name_of(made),
            dest = name_of(dest)
        )),
        OpNotice::AppIsMakingIt { kind } => Notification::info(t!("It shows here once it's made."))
            .title(t!("Your {kind} app is making it", kind = kind.clone())),
        OpNotice::InvalidName => {
            Notification::error(t!("A name can't contain any of these characters: \\ / : * ? \" < > |"))
        }
        OpNotice::OtherPane(why) => Notification::info(match why {
            OtherPane::ReadOnly { remote: true } => {
                t!("Remote connections are read-only: open a folder in the other pane to copy into")
            }
            OtherPane::ReadOnly { remote: false } => {
                t!("Archives are read-only: open a folder in the other pane to copy into")
            }
            OtherPane::Closed => {
                t!("Windows keeps the other pane's folder closed: open another one there to copy into")
            }
            OtherPane::NoFolder => t!("Open a folder in the other pane first"),
        }),
        OpNotice::QueueErrors { kind, count, detail } => {
            let title = match kind {
                QueueKind::Move => t!("{what} didn't move", what = items(*count)),
                QueueKind::Copy => t!("{what} didn't copy", what = items(*count)),
            };
            Notification::error(detail.clone()).title(title)
        }
        OpNotice::Failed { what, message } => Notification::error(message.clone()).title(failing(what)),
    };
    window.push_notification(note, cx);
}

/// Shows an Undo offer and takes it away after the web app's 8 s (the kit keeps a notification with a button).
fn offer_undo(note: Notification, undo: UndoId, window: &mut Window, cx: &mut App) {
    window.push_notification(note, cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(UNDO_FOR).await;
        let _ = handle.update(cx, |_, window, cx| {
            window.remove_notification1::<UndoToast>(undo.0 as usize, cx)
        });
    })
    .detach();
}

/// The error toasts' titles.
fn failing(what: &Failing) -> SharedString {
    match what {
        Failing::CopyOrMove { moving: true, count } => t!("Couldn't move {what}", what = items(*count)),
        Failing::CopyOrMove { moving: false, count } => t!("Couldn't copy {what}", what = items(*count)),
        Failing::Delete => t!("Couldn't delete"),
        Failing::DeleteItems { count } => t!("Couldn't delete {what}", what = items(*count)),
        Failing::Restore => t!("Couldn't restore"),
        Failing::Rename { name } => t!("Couldn't rename “{name}”", name = name.clone()),
        Failing::Create { dir: true } => t!("Couldn't create a folder"),
        Failing::Create { dir: false } => t!("Couldn't create a file"),
        Failing::MakeNew { kind } => t!("Couldn't make a new {kind}", kind = kind.clone()),
        Failing::EmptyBin => t!("Couldn't empty the Recycle Bin"),
        Failing::Queue(QueueKind::Move) => t!("Couldn't queue the move"),
        Failing::Queue(QueueKind::Copy) => t!("Couldn't queue the copy"),
        Failing::Clipboard => t!("Couldn't use the clipboard"),
        Failing::ReadClipboard => t!("Couldn't read the clipboard"),
        Failing::Paste => t!("Couldn't paste"),
    }
}

/// The slow-work notice comes and goes, and the taskbar button with it.
pub fn busy(event: &BusyEvent, window: &mut Window, cx: &mut App) {
    match event {
        BusyEvent::Shown { id, what } => {
            let message = match what {
                Working::Copying(count) => t!("Copying {what}…", what = items(*count)),
                Working::Moving(count) => t!("Moving {what}…", what = items(*count)),
                Working::Deleting(count) => t!("Deleting {what}…", what = items(*count)),
                Working::Restoring(count) => t!("Restoring {what}…", what = items(*count)),
                Working::Undoing(label) => t!("Undoing the {label}…", label = label.clone()),
            };
            window.push_notification(
                Notification::info(message)
                    .id1::<BusyToast>(*id as usize)
                    .autohide(false),
                cx,
            );
            window_state::taskbar_progress(window, ProgressState::Indeterminate, None, None);
        }
        BusyEvent::Ended { id, was_shown, idle } => {
            if *was_shown {
                window.remove_notification1::<BusyToast>(*id as usize, cx);
            }
            if *idle {
                window_state::taskbar_progress(window, ProgressState::None, None, None);
            }
        }
    }
}

/// Asks what the web app asked before acting, and acts on yes.
pub fn ask(question: &Ask, window: &mut Window, cx: &mut App) {
    match question {
        Ask::Purge(purge) => {
            let count = purge.paths.len();
            let what = match purge.names.as_slice() {
                [one] => format!("“{one}”"),
                _ => items(count),
            };
            let message = if count == 1 {
                t!("Delete {what} for good? It can't be restored afterwards.", what = what)
            } else {
                t!(
                    "Delete {what} for good? They can't be restored afterwards.",
                    what = what
                )
            };
            let purge = purge.clone();
            dialogs::confirm(
                window,
                cx,
                t!("Delete for good"),
                message,
                t!("Delete"),
                true,
                move |_, cx| {
                    let purge = purge.clone();
                    explorer::explorer(cx).update(cx, |e, cx| e.confirm_purge(purge, cx).detach());
                },
            );
        }
    }
}

/// The instant-search offer, kept up for the web app's 15 s.
struct OfferToast;

/// What searching tells the person.
///
/// PORT ME(coordinator): the web app made the instant-search offer once ever (`firstNotice`); the explorer
/// makes it at most once a run until the saved state keeps a flag for it.
pub fn find(notice: &FindNotice, window: &mut Window, cx: &mut App) {
    match notice {
        FindNotice::NothingBelowRecycleBin => window.push_notification(
            Notification::info(t!("What you type already narrows what's shown in it."))
                .title(t!("Nothing to search below the Recycle Bin")),
            cx,
        ),
        FindNotice::OfferInstantSearch => {
            let offer = Notification::info(t!(
                "It reads each drive's file table once and keeps up with every change. Windows asks for administrator approval."
            ))
            .title(t!("Instant search finds names in milliseconds"))
            .id::<OfferToast>()
            .action(|_, _, _| {
                Button::new("turn-on-instant-search").label(t!("Turn on")).on_click(|_, window, cx| {
                    window.remove_notification::<OfferToast>(cx);
                    let source = crate::components::status_bar::index::shared_source(cx);
                    let turning_on = crate::core_bridge::run((source.enable)());
                    let handle = window.window_handle();
                    cx.spawn(async move |cx| {
                        let result = turning_on.await.and_then(|inner| inner.map(|_| ()));
                        let _ = handle.update(cx, |_, window, cx| {
                            let note = match result {
                                Ok(()) => Notification::success(t!("Instant search is on")),
                                Err(error) => {
                                    Notification::error(error).title(t!("Couldn't turn on instant search"))
                                }
                            };
                            window.push_notification(note, cx);
                        });
                    })
                    .detach();
                })
            });
            window.push_notification(offer, cx);
            let handle = window.window_handle();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(15)).await;
                let _ = handle.update(cx, |_, window, cx| window.remove_notification::<OfferToast>(cx));
            })
            .detach();
        }
    }
}
