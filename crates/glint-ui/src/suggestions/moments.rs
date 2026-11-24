//! What the person was just doing, and what it suggests (the second table of [`super`]'s notes).
//!
//! The workspace hands every explorer event to [`heard`], which reads the few that mean something here (a rename, a
//! copy that took long, a deletion undone, a file opened, a folder listed) as a [`Moment`]; the marks' one way
//! of changing (`dialogs::tags::set_marks`) and Quick search's shortcut say theirs through [`noticed`]. What a
//! moment suggests is decided by [`after`], a function of what was seen in this run ([`Seen`]), the moment and the
//! settings, so each rule is a test. A moment costs a few comparisons: nothing here reads a file, and the one
//! question that needs the PC (where photos are that the Gallery doesn't show) is asked off the UI thread, once
//! a run.

use super::{folder_name, photos_of, About, Kind, Suggestion};
use crate::explorer::{self, BusyEvent, ExplorerEvent, OpNotice, Working};
use crate::kit::prelude::*;
use crate::{persistence, state, t};
use glint_model::marks::default_labels;
use glint_model::paths::{base_name, is_gallery, parent_path, same_path};
use glint_model::store::{Settings, Theme};
use std::time::{Duration, Instant};

/// Renames this close together in one folder are one job done by hand.
const RENAMES_WITHIN: Duration = Duration::from_secs(180);
/// How many of them it takes.
const RENAMES_FROM: usize = 3;
/// Opens this close together in one folder are a look through it.
const OPENS_WITHIN: Duration = Duration::from_secs(120);
const OPENS_FROM: usize = 4;
/// Copies and moves between the same two folders within this long are going back and forth.
const TRANSFERS_WITHIN: Duration = Duration::from_secs(600);
const TRANSFERS_FROM: usize = 3;
/// One kind of file moved from one folder to another this many times is a habit a rule can have.
const MOVES_FROM: usize = 3;
/// Labels given this many times with the names they came with.
const LABELS_FROM: usize = 3;
/// A folder with this many photos and videos right in it, making up most of it, belongs in the Gallery.
pub const PHOTOS_FROM: usize = 24;
/// A copy or a move that is still running this long after Glint said it was working is a long one (the notice
/// itself comes after half a second, which a dozen photos take).
const LONG_TRANSFER: Duration = Duration::from_secs(8);
/// Quick search summoned this many times (its shortcut also puts it away, which counts).
const SHORTCUTS_FROM: usize = 3;
/// The most folders remembered as offered to the Gallery (each is offered once).
const OFFERED_KEPT: usize = 32;

/// Something that just happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Moment {
    /// An item was renamed in place; `to` is its path now.
    Renamed { to: String },
    /// Items went from `from` to `dest`: their extensions (lower case, "" for a folder or none), and whether they
    /// were moved rather than copied.
    Placed {
        from: String,
        dest: String,
        exts: Vec<String>,
        moved: bool,
    },
    /// A copy or a move that took long ([`LONG_TRANSFER`]) has ended.
    SlowTransfer,
    /// A deletion was taken back.
    UndidDeletion,
    /// A file was opened with its app.
    Opened { path: String },
    /// A label was given.
    Labeled,
    /// Tags were added; the first of them.
    Tagged { tag: String },
    /// A comment was written.
    Commented,
    /// A folder with mostly photos and videos in it, which the Gallery doesn't show, was opened.
    PhotoFolder { folder: String, files: usize },
    /// Photos the Gallery doesn't show were found below `folder`, while the Gallery was open.
    PhotosElsewhere { folder: String, files: usize },
    /// Windows went to light or dark mode while Glint was running.
    WindowsThemeChanged { dark: bool },
    /// Quick search was opened with its shortcut.
    ShortcutUsed,
}

/// What this run has seen, for the rules that count.
#[derive(Default)]
pub struct Seen {
    /// Renames: when, and the item's path now.
    renames: Vec<(Instant, String)>,
    /// Copies and moves: when, from where, to where.
    transfers: Vec<(Instant, String, String)>,
    /// Moves of one kind of file: from where, to where, the extension.
    moves: Vec<(String, String, String)>,
    /// Files opened: when, and the folder they are in.
    opens: Vec<(Instant, String)>,
    labeled: usize,
    shortcuts: usize,
    /// Copies and moves Glint said it was working on, by the operation's id, and when it said so.
    slow: Vec<(u64, Instant)>,
    /// The Gallery's question was asked in this run.
    gallery_asked: bool,
}

impl Global for Seen {}

/// Whether two folders are the same pair, either way round.
fn same_pair(a: (&str, &str), b: (&str, &str)) -> bool {
    (same_path(a.0, b.0) && same_path(a.1, b.1)) || (same_path(a.0, b.1) && same_path(a.1, b.0))
}

/// The labels still have the names Glint came with.
fn labels_untouched(settings: &Settings) -> bool {
    let defaults = default_labels();
    settings.labels.len() == defaults.len() && settings.labels.iter().zip(&defaults).all(|(a, b)| a.name == b.name)
}

/// A file Quick Look shows well: a picture, a document, text, a video, a song.
fn lookable(path: &str) -> bool {
    const KINDS: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "webp", "heic", "avif", "bmp", "tif", "tiff", "svg", "pdf", "txt", "md", "log",
        "json", "csv", "xml", "docx", "xlsx", "pptx", "epub", "mp4", "mov", "mkv", "webm", "mp3", "flac", "wav", "m4a",
    ];
    base_name(path)
        .rsplit_once('.')
        .is_some_and(|(_, ext)| KINDS.iter().any(|k| k.eq_ignore_ascii_case(ext)))
}

/// Whether `path` is `folder` or below it (any case).
fn is_in(path: &str, folder: &str) -> bool {
    let folder = folder.trim_end_matches('\\');
    path.len() >= folder.len()
        && path.is_char_boundary(folder.len())
        && path[..folder.len()].eq_ignore_ascii_case(folder)
        && matches!(path.as_bytes().get(folder.len()), None | Some(b'\\'))
}

/// `tag:work`, or `tag:"two words"`.
fn tag_query(tag: &str) -> String {
    if tag.contains(char::is_whitespace) {
        format!("tag:\"{tag}\"")
    } else {
        format!("tag:{tag}")
    }
}

/// What `moment` suggests after what was seen before it. `dual` is whether two panes show.
pub fn after(seen: &mut Seen, moment: &Moment, now: Instant, settings: &Settings, dual: bool) -> Option<Suggestion> {
    let within = |then: Instant, span: Duration| now.saturating_duration_since(then) <= span;
    match moment {
        Moment::Renamed { to } => {
            let folder = parent_path(to);
            seen.renames
                .retain(|(when, path)| within(*when, RENAMES_WITHIN) && same_path(&parent_path(path), &folder) && path != to);
            seen.renames.push((now, to.clone()));
            if seen.renames.len() < RENAMES_FROM {
                return None;
            }
            let files: Vec<String> = std::mem::take(&mut seen.renames).into_iter().map(|(_, path)| path).collect();
            Some(
                Suggestion::new(
                    Kind::BatchRename,
                    t!("Renaming them one by one?"),
                    t!("Select several and press F2: Batch rename numbers them, swaps words and shows every new name before anything changes."),
                    t!("Rename these {n}…", n = files.len()),
                )
                .about(About::Files(files)),
            )
        }
        Moment::Placed { from, dest, exts, moved } => {
            if same_path(from, dest) {
                return None;
            }
            // One kind of file, moved the same way again: a rule can do that.
            if let (true, [ext]) = (*moved, exts.as_slice()) {
                if !ext.is_empty() {
                    seen.moves.push((from.clone(), dest.clone(), ext.clone()));
                    let again = seen
                        .moves
                        .iter()
                        .filter(|(f, d, e)| same_path(f, from) && same_path(d, dest) && e == ext)
                        .count();
                    if again >= MOVES_FROM {
                        seen.moves.retain(|(f, d, e)| !(same_path(f, from) && same_path(d, dest) && e == ext));
                        return Some(
                            Suggestion::new(
                                Kind::FolderRule,
                                t!(
                                    "Always move {kind} files to “{to}”?",
                                    kind = ext.to_uppercase(),
                                    to = folder_name(dest)
                                ),
                                t!(
                                    "That is {n} times from “{from}”. A folder rule does it by itself, and shows what it would move before you turn it on.",
                                    n = again,
                                    from = folder_name(from)
                                ),
                                t!("Make it a rule…"),
                            )
                            .about(About::Rule {
                                from: from.clone(),
                                names: format!("*.{ext}"),
                                to: dest.clone(),
                            }),
                        );
                    }
                }
            }
            if dual {
                return None;
            }
            seen.transfers.retain(|(when, _, _)| within(*when, TRANSFERS_WITHIN));
            seen.transfers.push((now, from.clone(), dest.clone()));
            let between = seen
                .transfers
                .iter()
                .filter(|(_, f, d)| same_pair((f, d), (from, dest)))
                .count();
            if between < TRANSFERS_FROM {
                return None;
            }
            seen.transfers.clear();
            Some(
                Suggestion::new(
                    Kind::TwoPanes,
                    t!("Back and forth between two folders?"),
                    t!(
                        "Put “{a}” and “{b}” side by side: drag across, or press F6 to go from one to the other.",
                        a = folder_name(from),
                        b = folder_name(dest)
                    ),
                    t!("Open two panes"),
                )
                .about(About::Folder(dest.clone())),
            )
        }
        Moment::SlowTransfer => (!settings.copy_queue).then(|| {
            Suggestion::new(
                Kind::CopyQueue,
                t!("Long copies can wait their turn"),
                t!("Glint's own queue copies one after another, can pause and go on, and checks every copy against its original."),
                t!("Use the copy queue"),
            )
        }),
        Moment::UndidDeletion => (!settings.confirm_delete).then(|| {
            Suggestion::new(
                Kind::AskBeforeDelete,
                t!("Deleted by accident?"),
                t!("Glint can ask before it moves anything to the Recycle Bin."),
                t!("Ask first"),
            )
        }),
        Moment::Opened { path } => {
            if !lookable(path) {
                return None;
            }
            let folder = parent_path(path);
            seen.opens
                .retain(|(when, in_folder)| within(*when, OPENS_WITHIN) && same_path(in_folder, &folder));
            seen.opens.push((now, folder));
            if seen.opens.len() < OPENS_FROM {
                return None;
            }
            seen.opens.clear();
            Some(Suggestion::new(
                Kind::QuickLook,
                t!("Just having a look?"),
                t!("Space shows a file without opening its app, and the arrow keys walk through the others."),
                t!("Show me"),
            ))
        }
        Moment::Labeled => {
            seen.labeled += 1;
            (seen.labeled >= LABELS_FROM && labels_untouched(settings)).then(|| {
                Suggestion::new(
                    Kind::NameLabels,
                    t!("Make the labels yours"),
                    t!("Red can be Urgent and Green can be Done: rename them, recolor them, add your own. Alt+1 to Alt+9 follow their order."),
                    t!("Name the labels"),
                )
            })
        }
        Moment::Tagged { tag } => {
            let query = tag_query(tag);
            Some(
                Suggestion::new(
                    Kind::FindMarks,
                    t!("Find it again by its tag"),
                    t!(
                        "{query} in the search box finds everything tagged like this, on every drive with instant search. label: and comment: work the same way.",
                        query = query.clone()
                    ),
                    t!("Find them"),
                )
                .about(About::Search(query)),
            )
        }
        Moment::Commented => Some(
            Suggestion::new(
                Kind::FindMarks,
                t!("Find it again by its comment"),
                t!("A word of the comment in the search box finds the file, and has:comment lists everything you wrote a note on."),
                t!("Find them"),
            )
            .about(About::Search("has:comment".to_string())),
        ),
        Moment::PhotoFolder { folder, files } => Some(
            Suggestion::new(
                Kind::GalleryFolder,
                t!("Show “{name}” in the Gallery?", name = folder_name(folder)),
                t!(
                    "{n} here. The Gallery shows your photos and videos as one grid, newest first, and this folder isn't in it yet.",
                    n = photos_of(*files)
                ),
                t!("Add to the Gallery"),
            )
            .about(About::Folder(folder.clone())),
        ),
        Moment::PhotosElsewhere { folder, files } => Some(
            Suggestion::new(
                Kind::GalleryFolder,
                t!("“{name}” isn't in your Gallery", name = folder_name(folder)),
                t!(
                    "{n} are in {folder}. Add the folder and they show here with the rest, newest first.",
                    n = photos_of(*files),
                    folder = isolate(folder.clone())
                ),
                t!("Add to the Gallery"),
            )
            .about(About::Folder(folder.clone())),
        ),
        Moment::WindowsThemeChanged { dark } => {
            let by_hand = match settings.theme {
                Theme::Light => *dark,
                Theme::Dark => !*dark,
                _ => false,
            };
            by_hand.then(|| {
                Suggestion::new(
                    Kind::MatchWindowsTheme,
                    if *dark {
                        t!("Windows went dark")
                    } else {
                        t!("Windows went light")
                    },
                    t!("Follow Windows' theme, and Glint changes with it, at night too."),
                    t!("Follow Windows"),
                )
            })
        }
        Moment::ShortcutUsed => {
            seen.shortcuts += 1;
            (seen.shortcuts >= SHORTCUTS_FROM && !settings.keep_in_tray).then(|| {
                Suggestion::new(
                    Kind::ShortcutNeedsTray,
                    t!("Keep Quick search's shortcut working"),
                    t!("The shortcut only works while Glint runs. Kept in the tray, it's there even after you close the window."),
                    t!("Keep it in the tray"),
                )
            })
        }
    }
}

/// Whether a suggestion a moment brought is still true of the settings (the Overview lists it while it is).
pub fn still_holds(suggestion: &Suggestion, settings: &Settings) -> bool {
    match (suggestion.kind, &suggestion.about) {
        (Kind::CopyQueue, _) => !settings.copy_queue,
        (Kind::AskBeforeDelete, _) => !settings.confirm_delete,
        (Kind::NameLabels, _) => labels_untouched(settings),
        (Kind::ShortcutNeedsTray, _) => !settings.keep_in_tray,
        (Kind::StartWithWindows, _) => !settings.start_with_windows,
        (Kind::MatchWindowsTheme, _) => settings.theme != Theme::System,
        (Kind::XyplorerFollow, _) => !settings.follow_xyplorer,
        (Kind::ShareWithZoxide, _) => !settings.zoxide,
        (Kind::GalleryFolder, About::Folder(folder)) => !settings.gallery_folders.iter().any(|f| same_path(f, folder)),
        _ => true,
    }
}

// ---- Hearing them ----------------------------------------------------------------------------------------------

/// Something just happened: what it suggests, if anything, comes up (or waits its turn: [`super::raise`]).
pub fn noticed(moment: Moment, window: &mut Window, cx: &mut App) {
    if state::try_settings(cx).is_none() {
        return;
    }
    // A folder is offered to the Gallery once: the next time it is another folder, or nothing.
    if let Moment::PhotoFolder { folder, .. } | Moment::PhotosElsewhere { folder, .. } = &moment {
        if was_offered(cx, folder) {
            return;
        }
    }
    let dual = explorer::try_explorer(cx).is_some_and(|explorer| explorer.read(cx).is_dual());
    let mut seen = if cx.has_global::<Seen>() {
        cx.remove_global::<Seen>()
    } else {
        Seen::default()
    };
    let suggestion = after(&mut seen, &moment, Instant::now(), state::settings(cx), dual);
    cx.set_global(seen);
    let Some(suggestion) = suggestion else { return };
    let offered = match &suggestion.about {
        About::Folder(folder) if suggestion.kind == Kind::GalleryFolder => Some(folder.clone()),
        _ => None,
    };
    if super::raise(suggestion, window, cx) {
        if let Some(folder) = offered {
            note_offered(cx, &folder);
        }
    }
}

/// [`noticed`], from where there is no window at hand (the marks' change, Quick search): the main window's.
pub fn noticed_in_main(moment: Moment, cx: &mut App) {
    if let Some(main) = crate::startup::main_window(cx) {
        let _ = main.update(cx, |_, window, cx| noticed(moment, window, cx));
    }
}

/// What an explorer event means here, if anything. Called by the workspace for every one of them, so the common
/// ones (a selection, a tab) fall through at once.
pub fn heard(event: &ExplorerEvent, window: &mut Window, cx: &mut App) {
    match event {
        ExplorerEvent::Operation(OpNotice::Renamed { to, .. }) => {
            noticed(Moment::Renamed { to: to.clone() }, window, cx)
        }
        ExplorerEvent::Operation(OpNotice::Undid { label }) if label.starts_with("deletion of") => {
            noticed(Moment::UndidDeletion, window, cx)
        }
        ExplorerEvent::Busy(BusyEvent::Shown {
            id,
            what: Working::Copying(_) | Working::Moving(_),
        }) => cx.default_global::<Seen>().slow.push((*id, Instant::now())),
        ExplorerEvent::Busy(BusyEvent::Ended { id, .. }) => {
            let seen = cx.default_global::<Seen>();
            if let Some(at) = seen.slow.iter().position(|(slow, _)| slow == id) {
                let (_, since) = seen.slow.remove(at);
                if since.elapsed() >= LONG_TRANSFER {
                    noticed(Moment::SlowTransfer, window, cx);
                }
            }
        }
        ExplorerEvent::Placed {
            from,
            dest,
            exts,
            moved,
        } => noticed(
            Moment::Placed {
                from: from.clone(),
                dest: dest.clone(),
                exts: exts.clone(),
                moved: *moved,
            },
            window,
            cx,
        ),
        ExplorerEvent::Opened(path) => noticed(Moment::Opened { path: path.clone() }, window, cx),
        ExplorerEvent::ListingChanged { tab, same: false } => listed(*tab, window, cx),
        _ => {}
    }
}

/// A folder was listed: the Gallery asks where else photos are (once a run), and a folder of photos the Gallery
/// doesn't show is one to offer it. Nothing is looked at unless a Gallery suggestion could come up at all.
fn listed(tab: explorer::TabId, window: &mut Window, cx: &mut App) {
    if !state::settings(cx).tips
        || !super::open_to(cx, Kind::GalleryFolder)
        || super::shown_lately(cx, Kind::GalleryFolder, super::today())
    {
        return;
    }
    let Some(explorer) = explorer::try_explorer(cx) else {
        return;
    };
    let (path, pane) = {
        let explorer = explorer.read(cx);
        let pane = explorer.active_pane();
        match explorer.active_tab() {
            Some(active) if active.id == tab && active.find.is_none() => (active.path.clone(), pane),
            _ => return,
        }
    };
    if is_gallery(&path) {
        let seen = cx.default_global::<Seen>();
        if !seen.gallery_asked {
            seen.gallery_asked = true;
            ask_photos_elsewhere(window, cx);
        }
        return;
    }
    // The head has counted what the folder holds (kept for the listing, so this costs nothing twice).
    let Some(head) = crate::components::pane_head::Head::of(explorer.read(cx), pane) else {
        return;
    };
    let files: usize = head
        .kinds
        .iter()
        .filter(|kind| kind.group.name != "Folders")
        .map(|kind| kind.count)
        .sum();
    if !head.real || head.media < PHOTOS_FROM || head.media * 10 < files * 6 {
        return;
    }
    if explorer
        .read(cx)
        .gallery_folders()
        .iter()
        .any(|folder| is_in(&path, folder))
    {
        return;
    }
    noticed(
        Moment::PhotoFolder {
            folder: path,
            files: head.media,
        },
        window,
        cx,
    );
}

/// Asks core, off the UI thread, where photos lie that the Gallery doesn't show; the best folder not offered
/// before is a moment of its own. Instant search answers it or nothing does.
fn ask_photos_elsewhere(window: &mut Window, cx: &mut App) {
    if !cx.has_global::<crate::core_bridge::Core>() {
        return;
    }
    let Some(explorer) = explorer::try_explorer(cx) else {
        return;
    };
    let (drives, broad) = {
        let explorer = explorer.read(cx);
        let drives: Vec<String> = explorer
            .drives()
            .iter()
            .filter(|drive| drive.kind == explorer::DriveKind::Fixed)
            .map(|drive| drive.path.clone())
            .collect();
        let home = explorer.known_path(explorer::KnownId::Home).map(str::to_string);
        let mut broad: Vec<String> = home.iter().map(|home| parent_path(home)).collect();
        broad.extend(home);
        (drives, broad)
    };
    window
        .spawn(cx, async move |cx| {
            let found = crate::core_bridge::run(async move {
                glint_core::fsx::blocking(move || Ok(glint_core::gallery::elsewhere(&drives, &broad, 4))).await
            })
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .unwrap_or_default();
            let _ = cx.update(|window, cx| {
                let first = found.into_iter().find(|place| !was_offered(cx, &place.folder));
                // Still in the Gallery: said elsewhere, it would be about something the person has left.
                let there = explorer::try_explorer(cx).is_some_and(|e| is_gallery(e.read(cx).current_path()));
                if let (Some(place), true) = (first, there) {
                    noticed(
                        Moment::PhotosElsewhere {
                            folder: place.folder,
                            files: place.files,
                        },
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
}

/// The saved notice that lists the folders offered to the Gallery, in lower case, `|` between them.
const OFFERED_KEY: &str = "suggest-gallery-offered";

fn was_offered(cx: &App, folder: &str) -> bool {
    let folder = folder.to_lowercase();
    persistence::app(cx)
        .and_then(|app| app.notices.get(OFFERED_KEY))
        .is_some_and(|list| list.split('|').any(|offered| offered == folder))
}

fn note_offered(cx: &mut App, folder: &str) {
    let folder = folder.to_lowercase();
    persistence::update_app(cx, |app| {
        let list = app.notices.entry(OFFERED_KEY.to_string()).or_default();
        let mut offered: Vec<&str> = list.split('|').filter(|f| !f.is_empty() && **f != folder).collect();
        offered.push(&folder);
        let from = offered.len().saturating_sub(OFFERED_KEPT);
        *list = offered[from..].join("|");
    });
}

// ---- What their buttons do -------------------------------------------------------------------------------------

/// The folder joins the Gallery's (Settings → Files → Gallery folders), the Gallery is read again where it shows,
/// and it is said, with Undo.
pub(super) fn add_to_gallery(folder: &str, window: &mut Window, cx: &mut App) {
    let folder = folder.to_string();
    let added = folder.clone();
    state::update(cx, |settings| {
        if !settings.gallery_folders.iter().any(|f| same_path(f, &added)) {
            settings.gallery_folders.push(added);
        }
    });
    reload_gallery(cx);
    let taken = folder.clone();
    super::say_done(
        window,
        cx,
        t!(
            "“{name}” is in the Gallery. Settings → Files lists its folders.",
            name = folder_name(&folder)
        ),
        move |settings| settings.gallery_folders.retain(|f| !same_path(f, &taken)),
    );
}

/// Every tab that shows the Gallery reads it again.
fn reload_gallery(cx: &mut App) {
    let Some(explorer) = explorer::try_explorer(cx) else {
        return;
    };
    explorer.update(cx, |explorer, cx| {
        let showing: Vec<explorer::TabId> = explorer
            .tabs()
            .filter(|tab| is_gallery(&tab.path) && tab.find.is_none())
            .map(|tab| tab.id)
            .collect();
        for tab in showing {
            explorer.reload(Some(tab), cx).detach();
        }
    });
}

/// Two panes, with `folder` in the one beside the one the keyboard is in.
pub(super) fn open_beside(folder: &str, cx: &mut App) {
    if let Some(explorer) = explorer::try_explorer(cx) {
        explorer.update(cx, |explorer, cx| {
            explorer.open_in_other_pane(folder, None, cx).detach()
        });
    }
}

/// `query` in the search box of the folder shown.
pub(super) fn search_for(query: &str, cx: &mut App) {
    if let Some(explorer) = explorer::try_explorer(cx) {
        explorer.update(cx, |explorer, cx| explorer.set_filter(query, None, cx));
    }
}

/// Folder rules, on a new rule that moves files named like `names` from `from` to `to`: off until the person has
/// looked at what it would do and turned it on.
pub(super) fn make_rule(from: &str, names: &str, to: &str, window: &mut Window, cx: &mut App) {
    use crate::dialogs::rules;
    let backend = rules::backend(cx);
    let rule = glint_model::store::FolderRule {
        names: names.to_string(),
        target: to.to_string(),
        ..rules::blank(from)
    };
    let view = cx.new(|cx| {
        let mut dialog = rules::RulesDialog::new(backend, None, window, cx);
        dialog.edit(rule, window, cx);
        dialog
    });
    crate::dialogs::show(window, cx, t!("Folder rules"), px(672.), view);
}
