//! What the file operations tell the person, worded as the web app's toasts were: the explorer's
//! [`OpNotice`]s as notifications (with an Undo button where one is offered), the slow-work notice and the
//! taskbar button while an operation runs ([`BusyEvent`]), the questions asked before acting ([`Ask`]), and
//! what archive work says ([`ArchiveNotice`]): its progress with a Stop button, what it skipped, what it made;
//! what the menus' other actions say ([`ActionNotice`]) and an export's progress ([`ExportNotice`]).
//!
//! The workspace calls these from its subscription to the explorer, with the main window at hand.

use crate::explorer::{
    self, ActionFailure, ActionNotice, ArchiveFailure, ArchiveNotice, Ask, BusyEvent, Doing, ExportNotice,
    ExportProgress, Failing, FindNotice, OpNotice, OtherPane, PinState, QueueKind, SkippedVerb, Subject, UndoId,
    Working,
};
use crate::kit::prelude::*;
use crate::platform::window_state::{self, ProgressState};
use crate::{dialogs, t};
use glint_model::format::{format_count, plural};
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
                    turn_on_instant_search(window, cx);
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

/// Turns instant search on (Windows asks for administrator approval) and says how it went: the instant-search
/// offer's button and the disk usage map's "Turn on instant search".
pub fn turn_on_instant_search(window: &mut Window, cx: &mut App) {
    let source = crate::components::status_bar::index::shared_source(cx);
    let turning_on = crate::core_bridge::run((source.enable)());
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let result = turning_on.await.and_then(|inner| inner.map(|_| ()));
        let _ = handle.update(cx, |_, window, cx| {
            let note = match result {
                Ok(()) => Notification::success(t!("Instant search is on")),
                Err(error) => Notification::error(error).title(t!("Couldn't turn on instant search")),
            };
            window.push_notification(note, cx);
        });
    })
    .detach();
}

/// Archive work's notice, keyed by the job (the web app's loading toast with its id).
struct ArchiveToast;
/// "Ready to drag" (id "drag-ready").
struct DragReadyToast;
/// What archive work skipped (one at a time: a newer one replaces it).
struct SkippedToast;

/// `“name”` for one item, else "3 items".
fn subject(what: &Subject) -> String {
    match what {
        Subject::One(name) => format!("“{name}”"),
        Subject::Many(count) => items(*count),
    }
}

/// What archive work is doing, as its notice says it ("Extracting “photos.zip”").
fn doing(what: &Doing) -> SharedString {
    match what {
        Doing::Compressing(what) => t!("Compressing {what}", what = subject(what)),
        Doing::Extracting(what) => t!("Extracting {what}", what = subject(what)),
        Doing::ExtractingFromArchive(what) => t!("Extracting {what} from the archive", what = subject(what)),
        Doing::Downloading(what) => t!("Downloading {what}", what = subject(what)),
        Doing::ExtractingFile(name) => t!("Extracting “{name}”", name = name.clone()),
        Doing::Mounting(name) => t!("Mounting “{name}”", name = name.clone()),
        Doing::CopyingOut(name) => t!("Copying out “{name}”", name = name.clone()),
    }
}

/// The work notice: "Extracting “a.zip”…" or "… 40%", with Stop where the work can stop.
fn working(id: u64, what: &Doing, percent: Option<u8>) -> Notification {
    let message = match percent {
        Some(percent) => t!("{doing}… {percent}%", doing = doing(what), percent = percent),
        None => t!("{doing}…", doing = doing(what)),
    };
    let note = Notification::info(message)
        .id1::<ArchiveToast>(id as usize)
        .autohide(false);
    if !what.stoppable() {
        return note;
    }
    note.action(move |_, _, _| {
        Button::new(("stop-archive-work", id as usize))
            .label(t!("Stop"))
            .on_click(move |_, _, cx| {
                explorer::explorer(cx).update(cx, |e, _| e.stop_archive_work(id));
            })
    })
}

/// What archive work says: its progress (with the taskbar button), what it skipped, what it made, why it failed.
pub fn archive(notice: &ArchiveNotice, window: &mut Window, cx: &mut App) {
    match notice {
        ArchiveNotice::Shown { id, doing, percent } => {
            window.push_notification(working(*id, doing, *percent), cx);
            window_state::taskbar_progress(window, ProgressState::Indeterminate, None, None);
        }
        ArchiveNotice::Progress {
            id,
            doing,
            percent,
            done,
            total,
            shown,
        } => {
            if *total == 0 {
                window_state::taskbar_progress(window, ProgressState::Indeterminate, None, None);
            } else {
                window_state::taskbar_progress(window, ProgressState::Normal, Some(*done), Some(*total));
            }
            if *shown {
                window.push_notification(working(*id, doing, Some(*percent)), cx);
            }
        }
        ArchiveNotice::Ended { id, was_shown, idle } => {
            if *was_shown {
                window.remove_notification1::<ArchiveToast>(*id as usize, cx);
            }
            if *idle {
                window_state::taskbar_progress(window, ProgressState::None, None, None);
            }
        }
        ArchiveNotice::Skipped {
            count,
            names,
            more,
            reason,
            verb,
        } => {
            let files = plural(*count, "file");
            let title = match verb {
                SkippedVerb::Added => t!("{files} couldn't be added", files = files),
                SkippedVerb::Extracted => t!("{files} couldn't be extracted", files = files),
                SkippedVerb::Downloaded => t!("{files} couldn't be downloaded", files = files),
            };
            let mut listed = names
                .iter()
                .map(|name| format!("“{name}”"))
                .collect::<Vec<_>>()
                .join(", ");
            if *more > 0 {
                listed.push_str(&t!(" and {n} more", n = *more));
            }
            let note = Notification::warning(format!("{listed}: {reason}"))
                .title(title)
                .id::<SkippedToast>()
                .autohide(false);
            window.push_notification(note, cx);
            // The web app's 10 s warning.
            let handle = window.window_handle();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(10)).await;
                let _ = handle.update(cx, |_, window, cx| window.remove_notification::<SkippedToast>(cx));
            })
            .detach();
        }
        ArchiveNotice::Failed { what, message } => {
            let title = match what {
                ArchiveFailure::Compress(what) => t!("Couldn't compress {what}", what = subject(what)),
                ArchiveFailure::Extract(what) => t!("Couldn't extract {what}", what = subject(what)),
                ArchiveFailure::Download(what) => t!("Couldn't download {what}", what = subject(what)),
                ArchiveFailure::Drag => t!("Couldn't drag these items"),
                ArchiveFailure::Mount(name) => t!("Couldn't mount “{name}”", name = name.clone()),
            };
            window.push_notification(Notification::error(message.clone()).title(title), cx);
        }
        ArchiveNotice::Compressed { what, zip } => window.push_notification(
            Notification::success(t!(
                "Compressed {what} into “{zip}”",
                what = subject(what),
                zip = name_of(zip)
            )),
            cx,
        ),
        ArchiveNotice::Extracted { archive, folder, file } => {
            let message = if *file {
                t!(
                    "Extracted “{archive}” as “{folder}”",
                    archive = name_of(archive),
                    folder = name_of(folder)
                )
            } else {
                t!(
                    "Extracted “{archive}” into “{folder}”",
                    archive = name_of(archive),
                    folder = name_of(folder)
                )
            };
            let (folder, file) = (folder.clone(), *file);
            let note = Notification::success(message).action(move |_, _, _| {
                let folder = folder.clone();
                Button::new("show-extracted")
                    .label(if file { t!("Show") } else { t!("Open") })
                    .on_click(move |_, _, cx| {
                        let folder = folder.clone();
                        explorer::explorer(cx).update(cx, |e, cx| e.show_extracted(&folder, file, cx).detach());
                    })
            });
            window.push_notification(note, cx);
        }
        ArchiveNotice::ReadyToDrag => window.push_notification(
            Notification::info(t!("They took a moment to extract: drag them again."))
                .title(t!("Ready to drag"))
                .id::<DragReadyToast>(),
            cx,
        ),
        ArchiveNotice::Mounted { name, drive } => window.push_notification(
            Notification::success(match drive {
                Some(drive) => t!(
                    "Mounted “{name}” as {drive}",
                    name = name.clone(),
                    drive = drive.clone()
                ),
                None => t!("Mounted “{name}”", name = name.clone()),
            }),
            cx,
        ),
    }
}

/// "“name”" for one path, else "3 items".
fn paths_subject(paths: &[String]) -> String {
    match paths {
        [one] => format!("“{}”", name_of(one)),
        _ => items(paths.len()),
    }
}

/// The first letter in capitals ("“a” …" keeps its quote first).
fn capitalized(text: String) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => text,
    }
}

/// Takes `T`'s notification down after `after`.
fn remove_after<T: 'static>(after: Duration, window: &mut Window, cx: &mut App) {
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(after).await;
        let _ = handle.update(cx, |_, window, cx| window.remove_notification::<T>(cx));
    })
    .detach();
}

/// "Windows Sandbox isn't on" (15 s).
struct SandboxToast;
/// "Took … out of Recent" (2.5 s).
struct RecentToast;
/// An export's progress, keyed by its job.
struct ExportToast;

/// What the menus' other actions say: opening, Windows Sandbox, the desktop background, Recent, what's kept
/// on this device, File Explorer's on-demand folders, scripts.
pub fn action(notice: &ActionNotice, window: &mut Window, cx: &mut App) {
    let note = match notice {
        ActionNotice::Failed { what, message } => {
            let title = match what {
                ActionFailure::OpenFile => t!("Couldn't open the file"),
                ActionFailure::AppPicker => t!("Couldn't open the app picker"),
                ActionFailure::Sandbox => t!("Couldn't run it in Windows Sandbox"),
                ActionFailure::FileExplorer => t!("Couldn't open File Explorer"),
                ActionFailure::OptionalFeatures => t!("Couldn't open Windows' optional features"),
                ActionFailure::Recent => t!("Couldn't take it out of Recent"),
                ActionFailure::Wallpaper => t!("Couldn't set the desktop background"),
                ActionFailure::FreeUpSpace => t!("Couldn't free up space"),
                ActionFailure::CloudKept => t!("Couldn't change what's kept on this device"),
                ActionFailure::FolderSizes => t!("Couldn't calculate folder sizes"),
                ActionFailure::ScriptsFolder => t!("Couldn't open the Scripts folder"),
                ActionFailure::ShowOnDemand => t!("Couldn't show it in File Explorer"),
                ActionFailure::HideOnDemand => t!("Couldn't take it out of File Explorer"),
            };
            Notification::error(message.clone()).title(title)
        }
        ActionNotice::SandboxOpening => Notification::success(t!(
            "A copy, in a Windows of its own without the network: closing the sandbox throws away everything it did."
        ))
        .title(t!("Opening it in Windows Sandbox")),
        ActionNotice::SandboxNotOn => {
            let note = Notification::info(t!(
                "It comes with Windows 11 Pro, Enterprise and Education: turn on “Windows Sandbox” in Windows' optional features (Windows restarts once)."
            ))
            .title(t!("Windows Sandbox isn't on"))
            .id::<SandboxToast>()
            .autohide(false)
            .action(|_, _, _| {
                Button::new("optional-features")
                    .label(t!("Optional features"))
                    .on_click(|_, window, cx| {
                        window.remove_notification::<SandboxToast>(cx);
                        explorer::explorer(cx).update(cx, |e, cx| e.open_optional_features(cx).detach());
                    })
            });
            window.push_notification(note, cx);
            remove_after::<SandboxToast>(Duration::from_secs(15), window, cx);
            return;
        }
        ActionNotice::WallpaperSet { path } => {
            Notification::success(t!("“{name}” is now your desktop background", name = name_of(path)))
        }
        ActionNotice::RotateFailed { count, detail } => Notification::error(detail.clone())
            .title(t!("Couldn't rotate {what}", what = plural(*count, "picture"))),
        ActionNotice::TookOutOfRecent { paths } => {
            let message = match paths.as_slice() {
                [one] => t!("Took “{name}” out of Recent", name = name_of(one)),
                _ => t!("Took {what} out of Recent", what = items(paths.len())),
            };
            window.push_notification(Notification::success(message).id::<RecentToast>(), cx);
            remove_after::<RecentToast>(Duration::from_millis(2500), window, cx);
            return;
        }
        ActionNotice::CloudPinned { state, paths } => {
            let what = paths_subject(paths);
            let message = match state {
                PinState::Keep => t!("Keeping {items} on this device", items = what),
                PinState::Free => t!("Freeing up space from {items}", items = what),
                PinState::Auto => t!("{items} no longer always kept on this device", items = what),
            };
            Notification::success(capitalized(message.to_string()))
        }
        ActionNotice::OnDemandShown { name, folder } => {
            let folder = folder.clone();
            Notification::success(t!(
                "Its files download when an app opens them, while Glint runs. It's read-only there."
            ))
            .title(t!("“{name}” is in File Explorer", name = name.clone()))
            .action(move |_, _, _| {
                let folder = folder.clone();
                Button::new("open-on-demand").label(t!("Open")).on_click(move |_, _, cx| {
                    let folder = folder.clone();
                    explorer::explorer(cx).update(cx, |e, cx| e.open_in_file_explorer(&folder, cx).detach());
                })
            })
        }
        ActionNotice::ScriptTooLong {
            name,
            paths,
            length,
            longest,
        } => Notification::error(t!(
            "The {paths} paths take {length} characters, and a command line holds about {longest}. Choose fewer, or run it on their folder.",
            paths = format_count(*paths),
            length = format_count(*length),
            longest = format_count(*longest)
        ))
        .title(t!("Too much for “{name}”'s command line", name = name.clone())),
        ActionNotice::ScriptGone { path } => Notification::error(t!("It may have been moved or deleted."))
            .title(t!("Couldn't find the script “{name}”", name = name_of(path))),
    };
    window.push_notification(note, cx);
}

/// The export notice: "Exporting “Data”… 40%" or "… 1,200 items", with Stop.
fn exporting(id: u64, name: &str, done: u64, total: u64) -> Notification {
    let message = match ExportNotice::progress(done, total) {
        ExportProgress::Percent(percent) => {
            t!(
                "Exporting “{name}”… {percent}%",
                name = name.to_string(),
                percent = percent
            )
        }
        ExportProgress::Items(done) => t!(
            "Exporting “{name}”… {count} items",
            name = name.to_string(),
            count = format_count(usize::try_from(done).unwrap_or(usize::MAX))
        ),
        ExportProgress::Starting => t!("Exporting “{name}”…", name = name.to_string()),
    };
    Notification::info(message)
        .id1::<ExportToast>(id as usize)
        .autohide(false)
        .action(move |_, _, _| {
            Button::new(("stop-export", id as usize))
                .label(t!("Stop"))
                .on_click(move |_, _, cx| {
                    explorer::explorer(cx).update(cx, |e, _| e.stop_archive_work(id));
                })
        })
}

/// An export's progress (with the taskbar button), its failure, and where it went.
pub fn export(notice: &ExportNotice, window: &mut Window, cx: &mut App) {
    let taskbar = |window: &Window, done: u64, total: u64| {
        if total > 0 {
            window_state::taskbar_progress(window, ProgressState::Normal, Some(done), Some(total));
        } else {
            window_state::taskbar_progress(window, ProgressState::Indeterminate, None, None);
        }
    };
    match notice {
        ExportNotice::Shown { id, name, done, total } => {
            window.push_notification(exporting(*id, name, *done, *total), cx);
            taskbar(window, *done, *total);
        }
        ExportNotice::Progress {
            id,
            name,
            done,
            total,
            shown,
        } => {
            taskbar(window, *done, *total);
            if *shown {
                window.push_notification(exporting(*id, name, *done, *total), cx);
            }
        }
        ExportNotice::Ended { id, was_shown, idle } => {
            if *was_shown {
                window.remove_notification1::<ExportToast>(*id as usize, cx);
            }
            if *idle {
                window_state::taskbar_progress(window, ProgressState::None, None, None);
            }
        }
        ExportNotice::Failed { name, message } => window.push_notification(
            Notification::error(message.clone()).title(t!("Couldn't export “{name}”", name = name.clone())),
            cx,
        ),
        ExportNotice::Done { name, dest } => {
            let dest = dest.clone();
            let note = Notification::success(t!(
                "Exported “{name}” as “{file}”",
                name = name.clone(),
                file = name_of(&dest)
            ))
            .action(move |_, _, _| {
                let dest = dest.clone();
                Button::new("show-export").label(t!("Show")).on_click(move |_, _, cx| {
                    let dest = dest.clone();
                    explorer::explorer(cx).update(cx, |e, cx| e.reveal_in_app(&dest, cx).detach());
                })
            });
            window.push_notification(note, cx);
        }
    }
}
