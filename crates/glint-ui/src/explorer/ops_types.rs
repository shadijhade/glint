//! The plain types of the file operations part: what Windows' file engine is asked and answers, Undo's steps,
//! and the things the model tells the workspace while it works (notices, questions, the "busy" toast).
//! Ported from `FileOp` and `Performed` in src/lib/fs.ts, `NewKind` in src/lib/newKinds.ts and the toasts and
//! confirmations inside the file operations of src/lib/explorer.ts.
//!
//! Nothing here touches GPUI or the disk. The wording each notice has is given in its doc, exactly as the
//! TypeScript toasts had it (English, `{x}` marking what goes in): the workspace says it with `t!`.
//! `{what}` is always `plural(count, "item")` ("1 item", "3 items"), and a name is the last part of a path
//! (`base_name`, which keeps the whole path for a drive).

use super::types::TabId;

// ---- Asking Windows' file engine -------------------------------------------------------------------------

/// One item to put somewhere under a name (`{ path, folder, name }`): Undo's way back for a move or a delete,
/// Restore's, and Sync's copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveItem {
    /// Where the item is now (a Recycle Bin entry's `$R` path, the moved item's path…).
    pub path: String,
    /// The folder it goes into (made again if it's gone).
    pub folder: String,
    /// The name it gets there.
    pub name: String,
}

/// A copy, move, rename, delete or restore through Windows' own file engine (progress, conflicts, Recycle Bin,
/// permission prompts) (`FileOp` in fs.ts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileOp {
    Copy {
        sources: Vec<String>,
        dest: String,
    },
    Move {
        sources: Vec<String>,
        dest: String,
    },
    /// To the Recycle Bin, or for good when `permanent`; `confirm` lets Windows ask first.
    Delete {
        paths: Vec<String>,
        permanent: bool,
        confirm: bool,
    },
    Rename {
        path: String,
        name: String,
    },
    /// Undoing a move: each item back into its folder as its name.
    MoveBack {
        items: Vec<MoveItem>,
    },
    /// Undoing a delete, or Restore in the Recycle Bin: its items back where they were, into their folder made
    /// again if it's gone.
    Restore {
        items: Vec<MoveItem>,
    },
    /// Delete for good in the Recycle Bin: its items by their paths there (asked first, by their names).
    Purge {
        paths: Vec<String>,
    },
    /// Sync in compare panes: each item copied into its folder as its name.
    CopyInto {
        items: Vec<MoveItem>,
    },
}

/// What became of one item asked for (`Performed.items`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemResult {
    pub from: String,
    /// Where it is now (the copy, the item moved or renamed, its Recycle Bin entry); `None` when it's gone for
    /// good or was skipped.
    pub to: Option<String>,
}

/// What a file operation came to (`Performed`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Performed {
    /// False when the person cancelled in one of Windows' dialogs; what was done before stays done.
    pub completed: bool,
    pub items: Vec<ItemResult>,
}

/// Files copied or cut, here or in File Explorer, as the clipboard holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardFiles {
    pub paths: Vec<String>,
    pub cut: bool,
}

/// A kind of file Windows' New menu makes (a Word document, a zip…) (`NewKind` in newKinds.ts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewKind {
    /// Its extension, with the dot.
    pub ext: String,
    /// What the menu says: "Microsoft Word Document".
    pub name: String,
    /// Its icon as File Explorer shows it (a PNG's data URL).
    pub icon: Option<String>,
    /// Its app makes it with a command of its own (and may open a window): not renamed right away.
    pub by_app: bool,
}

/// What the copy queue is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueKind {
    Copy,
    Move,
}

impl QueueKind {
    /// Core's word for it.
    pub fn as_str(self) -> &'static str {
        match self {
            QueueKind::Copy => "copy",
            QueueKind::Move => "move",
        }
    }
}

// ---- Undo ---------------------------------------------------------------------------------------------------

/// One step Ctrl+Z can take back (`UndoStep`); ids are never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UndoId(pub u64);

/// What takes a step back: the `run` closure of the TypeScript's `UndoStep`, as data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoAction {
    /// To the Recycle Bin without asking: taking back a copy, a new item, a paste (a copy is recycled rather
    /// than deleted for good).
    Recycle(Vec<String>),
    /// A move taken back.
    MoveBack(Vec<MoveItem>),
    /// A deletion taken back (or a restore, which recycles).
    Restore(Vec<MoveItem>),
    /// A rename taken back: the item at `path` gets `name` again.
    Rename { path: String, name: String },
}

impl UndoAction {
    /// The file operation that does it.
    pub fn op(&self) -> FileOp {
        match self {
            UndoAction::Recycle(paths) => FileOp::Delete {
                paths: paths.clone(),
                permanent: false,
                confirm: false,
            },
            UndoAction::MoveBack(items) => FileOp::MoveBack { items: items.clone() },
            UndoAction::Restore(items) => FileOp::Restore { items: items.clone() },
            UndoAction::Rename { path, name } => FileOp::Rename {
                path: path.clone(),
                name: name.clone(),
            },
        }
    }
}

// ---- Telling the person ------------------------------------------------------------------------------------

/// What the slow-work toast says (`busy(label, …)`): shown only when the work takes longer than half a second.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Working {
    /// `Copying {what}…`
    Copying(usize),
    /// `Moving {what}…`
    Moving(usize),
    /// `Deleting {what}…`
    Deleting(usize),
    /// `Restoring {what}…`
    Restoring(usize),
    /// `Undoing the {label}…`
    Undoing(String),
}

/// The slow-work toast comes and goes (`busy`). The workspace shows a loading notice with the words of
/// [`Working`] on `Shown` and takes it away on `Ended`, and drives the taskbar button: indeterminate on
/// `Shown`, none once `Ended` says `idle` (nothing else is running).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BusyEvent {
    Shown {
        id: u64,
        what: Working,
    },
    /// `was_shown`: a `Shown` for this id was sent, so its toast is to go. `idle`: no operation is running now.
    Ended {
        id: u64,
        was_shown: bool,
        idle: bool,
    },
}

/// Why a copy into the other pane can't happen (`copyToOtherPane`'s toast).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OtherPane {
    /// `{Remote connections|Archives} are read-only: open a folder in the other pane to copy into`
    ReadOnly { remote: bool },
    /// `Windows keeps the other pane's folder closed: open another one there to copy into`
    Closed,
    /// `Open a folder in the other pane first`
    NoFolder,
}

/// What failed, for the error toasts (`toast.error(title, { description: String(e) })`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failing {
    /// `Couldn't {move|copy} {what}`
    CopyOrMove { moving: bool, count: usize },
    /// `Couldn't delete`
    Delete,
    /// `Couldn't delete {what}` (a drop on the Recycle Bin)
    DeleteItems { count: usize },
    /// `Couldn't restore`
    Restore,
    /// `Couldn't rename “{name}”`
    Rename { name: String },
    /// `Couldn't create a {folder|file}`
    Create { dir: bool },
    /// `Couldn't make a new {kind}` (the New menu's kind name)
    MakeNew { kind: String },
    /// `Couldn't empty the Recycle Bin`
    EmptyBin,
    /// `Couldn't queue the {move|copy}`
    Queue(QueueKind),
    /// `Couldn't use the clipboard`
    Clipboard,
    /// `Couldn't read the clipboard`
    ReadClipboard,
    /// `Couldn't paste`
    Paste,
}

/// What the file operations tell the person. Carried by [`ExplorerEvent::Operation`](super::ExplorerEvent):
/// the workspace turns each into a notification with `t!`, the wording in the docs. (They are not variants of
/// [`Notice`](super::Notice) so that the match over it in startup.rs keeps compiling; fold them in when wiring
/// if you like.) Paths are carried whole; the notice shows their `base_name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpNotice {
    /// Success, 8 s, id "undo", with an **Undo** button that calls `Explorer::undo(Some(undo))`:
    /// `Copied {what} to “{dest name}”`. A drop that has no undo (`undo: None`) says the same without a button.
    Copied {
        count: usize,
        dest: String,
        undo: Option<UndoId>,
    },
    /// The same, `Synced {what}` (Sync's copies land in subfolders, out of sight).
    Synced { count: usize, undo: UndoId },
    /// The same, `Moved {what} to “{dest name}”`.
    Moved { count: usize, dest: String, undo: UndoId },
    /// The same, `Renamed to “{name of to}”`.
    Renamed { to: String, undo: UndoId },
    /// The same, `Moved {what} to the Recycle Bin`.
    Recycled { count: usize, undo: UndoId },
    /// The same, `Restored {what}`.
    Restored { count: usize, undo: UndoId },
    /// Takes away the toast that offered Undo (`toast.dismiss("undo")`): it has done its job.
    DismissUndo,
    /// Success, id "undone": `Undid the {label}`.
    Undid { label: String },
    /// Error, id "undone", with `message` as the description: `Couldn't undo the {label}`.
    UndoFailed { label: String, message: String },
    /// Info, id "undone": `Nothing to undo`.
    NothingToUndo,
    /// Success, 1.5 s: `{Cut|Copied} {what}` (to the clipboard).
    OnClipboard { count: usize, cut: bool },
    /// Success, id "emptied": `Emptied the Recycle Bin`.
    EmptiedBin,
    /// Info with a description: `There are no files on the clipboard` / `Copy or cut some first (Ctrl+C, Ctrl+X).`
    NoFilesOnClipboard,
    /// Info with a description: `Nothing to paste` / `Copy files, a picture or text first, here or in another app.`
    NothingToPaste,
    /// Success: `Pasted “{name of made}” into “{dest name}”` (a picture or text pasted as a file into a folder
    /// that isn't on screen).
    PastedInto { made: String, dest: String },
    /// Info with a description: `Your {kind} app is making it` / `It shows here once it's made.`
    AppIsMakingIt { kind: String },
    /// Error: `A name can't contain any of these characters: \ / : * ? " < > |`
    InvalidName,
    /// Info: why a copy into the other pane can't happen.
    OtherPane(OtherPane),
    /// Error with `detail` as the description (up to three `{name}: {error}` lines): `{count} item(s) didn't
    /// {move|copy}` (`plural(count, "item")`).
    QueueErrors {
        kind: QueueKind,
        count: usize,
        detail: String,
    },
    /// Error with `message` as the description.
    Failed { what: Failing, message: String },
}

/// A question the TypeScript asked before acting. The workspace asks it (a dialog) and, if the answer is yes,
/// calls the method named in the doc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// `Delete {what} for good? {It|They} can't be restored afterwards.` Title `Delete for good`, buttons
    /// `Delete` and `Cancel`. `what` is `“{name}”` for one item, else `plural(count, "item")`; It for one.
    /// On yes call [`Explorer::confirm_purge`](super::Explorer::confirm_purge) with it.
    Purge(PurgeAsk),
}

/// Items in the Recycle Bin to delete for good, once asked (`purgeRecycled`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurgeAsk {
    /// The tab the Recycle Bin is shown in.
    pub tab: TabId,
    /// Their paths in the bin.
    pub paths: Vec<String>,
    /// Their names as they were (the bin's paths say `$R…`).
    pub names: Vec<String>,
    /// The item to select afterwards, like Explorer does.
    pub next: Option<String>,
}

/// Work on files in an archive or on a remote connection, which the archive part does: they have to be
/// extracted (staged) first, then the step named here is done on the staged copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArchiveJob {
    /// Copy to the other pane's folder (`copyOutOfArchive`): extract `paths`, then
    /// `Explorer::copy_staged(staged, target_tab, target_path)`, then remove the staged copies.
    CopyTo {
        paths: Vec<String>,
        target_tab: TabId,
        target: String,
    },
    /// Copy to the clipboard (`copyToClipboard`): extract `paths`, then `Explorer::copied_staged(staged)`.
    ToClipboard { paths: Vec<String> },
}
