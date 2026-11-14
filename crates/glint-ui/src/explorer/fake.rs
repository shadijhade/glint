//! A fake disk for the model tests: folders with entries, held in memory, behind the same [`Source`] the app
//! uses for glint-core. Listings can be held back and released, to test results that arrive out of order.

use super::actions_types::*;
use super::archive_types::*;
use super::find_types::{FindOptions, Found, SearchIndex};
use super::ops_types::*;
use super::source::{Access, CompressRequest, Effect, Fut, Resolved, Source};
use super::types::*;
use glint_model::entries::{attr, sort_entries, with_details, FileEntry, InArchive, RawEntry, Sort, SortKey};
use glint_model::paths::{parent_path, same_path, RECYCLE_BIN};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// A file entry in `dir`.
pub fn file(dir: &str, name: &str, size: u64, modified: i64) -> FileEntry {
    with_details(
        RawEntry {
            name: name.into(),
            size,
            modified,
            ..Default::default()
        },
        dir,
    )
}

/// A folder entry in `dir`.
pub fn folder(dir: &str, name: &str) -> FileEntry {
    with_details(
        RawEntry {
            name: name.into(),
            dir: true,
            attrs: attr::DIRECTORY,
            ..Default::default()
        },
        dir,
    )
}

/// A hidden file entry in `dir`.
pub fn hidden(dir: &str, name: &str) -> FileEntry {
    with_details(
        RawEntry {
            name: name.into(),
            attrs: attr::HIDDEN,
            ..Default::default()
        },
        dir,
    )
}

/// What the fake knows about one folder.
struct Folder {
    entries: Vec<FileEntry>,
    stamp: f64,
    denied: bool,
    /// Listed as a folder in an archive (read-only), when set.
    archive: Option<InArchive>,
    /// Listed from instant search's index: Windows keeps the folder closed to this account.
    from_index: bool,
}

#[derive(Default)]
struct State {
    folders: HashMap<String, Folder>,
    /// Listings held back until released: path (lower case) to the gate's receiver.
    gates: HashMap<String, async_channel::Receiver<()>>,
    drives: Vec<Drive>,
    known: Vec<KnownFolder>,
    resolves: HashMap<String, Resolved>,
    opens: HashMap<String, Option<String>>,
    watched: Vec<Vec<String>>,
    effects: Vec<Effect>,
    listed: Vec<String>,
    window: bool,
    // ---- File operations ----
    bin_seq: u64,
    clipboard: Option<ClipboardFiles>,
    /// The name of the file a paste with no files on the clipboard makes (a picture or text there).
    paste_file: Option<String>,
    recycle_answer: bool,
    new_kinds: Vec<NewKind>,
    /// A New menu file whose app has not made it yet.
    new_pending: bool,
    ops: Vec<FileOp>,
    queued: Vec<(QueueKind, Vec<String>, String, bool)>,
    queue_error: Option<String>,
    ejected: Vec<String>,
    fail: Option<String>,
    cancel: bool,
    clipboard_fail: Option<String>,
    /// File operations wait here until released (slow work).
    op_gate: Option<async_channel::Receiver<()>>,
    // ---- Searching ----
    /// Every search started, in order.
    finds: Vec<FindCall>,
    /// The lanes asked to end, in order.
    cancelled: Vec<String>,
    /// Searches a test drives by hand: the stream a query gets, by the query.
    live_finds: HashMap<String, async_channel::Receiver<Found>>,
    /// A search that is refused (a bad filter), once.
    find_error: Option<String>,
    index: Option<SearchIndex>,
    /// How many times instant search was asked how it does.
    index_asked: usize,
    /// Every search inside files started, in order.
    inside: Vec<(super::inside::InsideEngine, super::inside::InsideRequest)>,
    /// How many times the search inside files was told to stop.
    inside_cancels: usize,
    /// A search inside files that can't start, once.
    inside_error: Option<String>,
    /// Whether the content index covers every folder.
    inside_covered: bool,
    /// A test drives the searches inside files by hand (`Explorer::inside_heard`); otherwise each one ends as it
    /// starts, having found nothing.
    inside_live: bool,
    // ---- Archives ----
    arch: ArchiveFake,
    // ---- Menu actions ----
    act: ActionFake,
}

/// A measuring the fake was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct SizeCall {
    pub path: String,
    pub lane: String,
    pub index_only: bool,
}

/// What the fake keeps for the menu actions: what it was asked, and what it answers.
#[derive(Default)]
struct ActionFake {
    sandboxed: Vec<String>,
    shell_opened: Vec<String>,
    revealed: Vec<Vec<String>>,
    forgotten: Vec<Vec<String>>,
    wallpapers: Vec<String>,
    rotated: Vec<(String, bool)>,
    pins: Vec<(Vec<String>, PinState)>,
    sizing: Vec<SizeCall>,
    sizes_stopped: Vec<String>,
    /// Measuring waits here until released (slow work).
    sizes_gate: Option<async_channel::Receiver<()>>,
    /// Folders the index has (lower-case keys): measured as "index", and the only ones an index-only ask answers.
    indexed: HashSet<String>,
    /// What a folder measures to when a test says so (lower-case keys).
    measured: HashMap<String, Measured>,
    exports: Vec<(String, String, String)>,
    export_progress: Vec<(u64, u64)>,
    export_gate: Option<async_channel::Receiver<()>>,
    scripts: Vec<Script>,
    scripts_folder: Option<String>,
    mounts: Vec<OnDemand>,
    /// The next call of this kind ("sandbox", "reveal"...) fails with this.
    fail: HashMap<&'static str, String>,
    /// Pictures that can't be turned (lower-case keys), with the reason.
    fail_rotate: HashMap<String, String>,
}

/// What the fake keeps for archives, disk images and previous versions.
#[derive(Default)]
struct ArchiveFake {
    stage_seq: u32,
    /// Staging, compressing and extracting wait here until released (slow work).
    gate: Option<async_channel::Receiver<()>>,
    /// Sent to the progress sink when a staging starts.
    progress: Vec<(u64, u64)>,
    stages: Vec<StageCall>,
    compressions: Vec<(CompressRequest, String)>,
    extractions: Vec<(String, String)>,
    /// The lanes asked to stop.
    stopped: Vec<String>,
    unstaged: Vec<String>,
    extracted_files: Vec<String>,
    mounted: Vec<String>,
    /// The drive the next mount gives (and adds to the drives).
    mount_drive: Option<String>,
    mounts_by_default: bool,
    version_copies: Vec<String>,
    opened: Vec<(String, OpenHow)>,
    /// The next archive call fails with this.
    fail: Option<String>,
    /// The next open fails with this.
    fail_open: Option<String>,
    /// Items that can't be extracted (lower-case path to reason).
    skip: HashMap<String, String>,
    /// What an archive file extracts to (lower-case path to names and sizes); a readme otherwise.
    contents: HashMap<String, Vec<(String, u64)>>,
    /// A single compressed file (lower-case path) extracts as this one file.
    single: HashMap<String, String>,
}

/// A staging the fake was asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct StageCall {
    pub paths: Vec<String>,
    pub purpose: StagePurpose,
    pub lane: String,
}

/// Where the fake stages what it extracts, where it keeps a file in an archive opened, and a version's copy.
pub const STAGE_ROOT: &str = "C:\\Temp\\Glint\\stage";
pub const OPEN_ROOT: &str = "C:\\Temp\\Glint\\open";
pub const VERSION_ROOT: &str = "C:\\Temp\\Glint\\Previous versions";

/// A search the fake was asked to start.
#[derive(Clone, Debug, PartialEq)]
pub struct FindCall {
    pub lane: String,
    pub roots: Vec<String>,
    pub query: String,
    pub options: FindOptions,
}

/// Where the fake keeps what is in the Recycle Bin (the listing of the bin itself holds only their entries).
const BIN_STORE: &str = "C:\\$Recycle.Bin";

/// The fake disk and the source reading it. Cloning shares the disk.
#[derive(Clone, Default)]
pub struct FakeDisk {
    state: Rc<RefCell<State>>,
}

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

fn empty_folder() -> Folder {
    Folder {
        entries: Vec::new(),
        stamp: 1.0,
        denied: false,
        archive: None,
        from_index: false,
    }
}

fn sorted(entries: Vec<FileEntry>) -> Vec<FileEntry> {
    sort_entries(
        entries,
        Sort {
            key: SortKey::Name,
            desc: false,
        },
        true,
        false,
        None,
        None,
        None,
    )
}

impl FakeDisk {
    pub fn new() -> Self {
        let disk = FakeDisk::default();
        {
            let mut state = disk.state.borrow_mut();
            state.window = true;
            state.recycle_answer = true;
            // The Recycle Bin and where its items really are start empty.
            state.folders.insert(key(RECYCLE_BIN), empty_folder());
            state.folders.insert(key(BIN_STORE), empty_folder());
        }
        disk
    }

    /// Makes a folder, listing `entries` as the engine would (folders first, natural order).
    pub fn folder(&self, path: &str, entries: Vec<FileEntry>) -> &Self {
        let sorted = sort_entries(
            entries,
            Sort {
                key: SortKey::Name,
                desc: false,
            },
            true,
            false,
            None,
            None,
            None,
        );
        self.state.borrow_mut().folders.insert(
            key(path),
            Folder {
                entries: sorted,
                stamp: 1.0,
                denied: false,
                archive: None,
                from_index: false,
            },
        );
        self
    }

    /// Makes a folder that is listed from instant search's index: the tab opens it as a folder Windows keeps
    /// closed (read-only, with the closed bar), as core's listing says when the disk refuses and the index has it.
    /// Making it again with [`FakeDisk::folder`] is Windows letting this account in.
    pub fn index_folder(&self, path: &str, entries: Vec<FileEntry>) -> &Self {
        self.folder(path, entries);
        if let Some(folder) = self.state.borrow_mut().folders.get_mut(&key(path)) {
            folder.from_index = true;
        }
        self
    }

    /// Makes a folder Windows keeps this account out of.
    pub fn deny(&self, path: &str) -> &Self {
        let mut state = self.state.borrow_mut();
        match state.folders.get_mut(&key(path)) {
            Some(folder) => folder.denied = true,
            None => {
                state.folders.insert(
                    key(path),
                    Folder {
                        entries: Vec::new(),
                        stamp: 1.0,
                        denied: true,
                        archive: None,
                        from_index: false,
                    },
                );
            }
        }
        drop(state);
        self
    }

    /// Changes a folder's date, as an add, delete or rename in it does.
    pub fn touch(&self, path: &str) {
        if let Some(folder) = self.state.borrow_mut().folders.get_mut(&key(path)) {
            folder.stamp += 1.0;
        }
    }

    /// Removes a folder from the disk.
    pub fn remove(&self, path: &str) {
        self.state.borrow_mut().folders.remove(&key(path));
    }

    /// Holds listings of `path` back until the returned sender sends (or is dropped).
    pub fn hold(&self, path: &str) -> async_channel::Sender<()> {
        let (gate, wait) = async_channel::bounded(1);
        self.state.borrow_mut().gates.insert(key(path), wait);
        gate
    }

    pub fn set_drives(&self, drives: Vec<Drive>) {
        self.state.borrow_mut().drives = drives;
    }

    pub fn set_known(&self, known: Vec<KnownFolder>) {
        self.state.borrow_mut().known = known;
    }

    pub fn on_resolve(&self, text: &str, path: &str, dir: bool) {
        self.state.borrow_mut().resolves.insert(
            text.to_string(),
            Resolved {
                path: path.to_string(),
                dir,
            },
        );
    }

    pub fn on_open(&self, path: &str, folder: Option<&str>) {
        self.state
            .borrow_mut()
            .opens
            .insert(path.to_string(), folder.map(str::to_string));
    }

    /// Whether `watch` has a window to watch for.
    pub fn set_window(&self, there: bool) {
        self.state.borrow_mut().window = there;
    }

    /// How many times each folder was listed, in order.
    pub fn listed(&self) -> Vec<String> {
        self.state.borrow().listed.clone()
    }

    /// The last set of watched folders.
    pub fn watched(&self) -> Vec<String> {
        self.state.borrow().watched.last().cloned().unwrap_or_default()
    }

    pub fn watch_calls(&self) -> usize {
        self.state.borrow().watched.len()
    }

    pub fn effects(&self) -> Vec<Effect> {
        self.state.borrow().effects.clone()
    }

    /// A [`Source`] reading this disk.
    pub fn source(&self) -> Rc<dyn Source> {
        Rc::new(self.clone())
    }

    // ---- File operations: what a test sets up and checks ------------------------------------------------

    /// Puts files on the clipboard, as another app would.
    pub fn set_clipboard(&self, paths: &[&str], cut: bool) {
        self.state.borrow_mut().clipboard = Some(ClipboardFiles {
            paths: paths.iter().map(|p| p.to_string()).collect(),
            cut,
        });
    }

    /// The files on the clipboard.
    pub fn clipboard(&self) -> Option<ClipboardFiles> {
        self.state.borrow().clipboard.clone()
    }

    /// A picture or text is on the clipboard: pasting with no files makes a file with this name.
    pub fn set_paste_file(&self, name: Option<&str>) {
        self.state.borrow_mut().paste_file = name.map(str::to_string);
    }

    /// What Windows' "Empty the Recycle Bin" question is answered (default yes).
    pub fn set_recycle_answer(&self, yes: bool) {
        self.state.borrow_mut().recycle_answer = yes;
    }

    /// The kinds the New menu has.
    pub fn set_new_kinds(&self, kinds: Vec<NewKind>) {
        self.state.borrow_mut().new_kinds = kinds;
    }

    /// The next New menu file isn't made within the few seconds the app is given.
    pub fn set_new_pending(&self, pending: bool) {
        self.state.borrow_mut().new_pending = pending;
    }

    /// The next use of the clipboard fails with this message (another program holds it).
    pub fn fail_clipboard(&self, message: &str) {
        self.state.borrow_mut().clipboard_fail = Some(message.to_string());
    }

    /// The next file operation fails with this message.
    pub fn fail_next(&self, message: &str) {
        self.state.borrow_mut().fail = Some(message.to_string());
    }

    /// The next file operation is cancelled by the person in one of Windows' dialogs: nothing is done and it
    /// says it did not complete.
    pub fn cancel_next(&self) {
        self.state.borrow_mut().cancel = true;
    }

    /// Holds file operations back until the returned sender sends (or is dropped): slow work.
    pub fn hold_ops(&self) -> async_channel::Sender<()> {
        let (gate, wait) = async_channel::bounded(1);
        self.state.borrow_mut().op_gate = Some(wait);
        gate
    }

    /// The copy queue refuses the next job.
    pub fn fail_queue(&self, message: &str) {
        self.state.borrow_mut().queue_error = Some(message.to_string());
    }

    /// Every file operation asked for, in order.
    pub fn ops(&self) -> Vec<FileOp> {
        self.state.borrow().ops.clone()
    }

    /// Every job added to the copy queue: kind, sources, destination, verify.
    pub fn queued(&self) -> Vec<(QueueKind, Vec<String>, String, bool)> {
        self.state.borrow().queued.clone()
    }

    // ---- Searching: what a test sets up and checks ------------------------------------------------------

    /// Hands the next search for exactly `query` to the test: what it sends to the returned sender is what that
    /// search says, whenever the test says it. Without this a search looks through the fake folders.
    pub fn live_find(&self, query: &str) -> async_channel::Sender<Found> {
        let (sender, stream) = async_channel::unbounded();
        self.state.borrow_mut().live_finds.insert(query.to_string(), stream);
        sender
    }

    /// Every search started so far.
    pub fn finds(&self) -> Vec<FindCall> {
        self.state.borrow().finds.clone()
    }

    /// Every search inside files started so far, with the engine asked.
    pub fn inside_searches(&self) -> Vec<(super::inside::InsideEngine, super::inside::InsideRequest)> {
        self.state.borrow().inside.clone()
    }

    /// How many times the search inside files was told to stop.
    pub fn inside_cancels(&self) -> usize {
        self.state.borrow().inside_cancels
    }

    /// Makes the next search inside files fail to start.
    pub fn refuse_next_inside(&self, message: &str) {
        self.state.borrow_mut().inside_error = Some(message.to_string());
    }

    /// Searches inside files stay running until the test says what they found (`Explorer::inside_heard` with the
    /// request's id) and that they ended.
    pub fn live_inside(&self) {
        self.state.borrow_mut().inside_live = true;
    }

    /// Whether the content index covers every folder (it doesn't, unless a test says so).
    pub fn set_inside_covered(&self, covered: bool) {
        self.state.borrow_mut().inside_covered = covered;
    }

    /// The lanes whose search was asked to end.
    pub fn cancelled(&self) -> Vec<String> {
        self.state.borrow().cancelled.clone()
    }

    /// Makes the next search refuse its query.
    pub fn refuse_next_find(&self, message: &str) {
        self.state.borrow_mut().find_error = Some(message.to_string());
    }

    /// How instant search stands (`None`: it can't be said).
    pub fn set_search_index(&self, index: Option<SearchIndex>) {
        self.state.borrow_mut().index = index;
    }

    /// How many times instant search was asked how it does.
    pub fn index_asked(&self) -> usize {
        self.state.borrow().index_asked
    }

    // ---- Archives: what a test sets up and checks -------------------------------------------------------

    /// Makes a folder inside an archive: listed like any folder, but read-only, with the archive it is in. Its
    /// entries get the in-archive mark, as core's do.
    pub fn archive_folder(&self, path: &str, outer: &str, entries: Vec<FileEntry>) -> &Self {
        let entries: Vec<FileEntry> = entries
            .into_iter()
            .map(|mut e| {
                e.attrs |= attr::IN_ARCHIVE;
                e
            })
            .collect();
        self.folder(path, entries);
        if let Some(folder) = self.state.borrow_mut().folders.get_mut(&key(path)) {
            folder.archive = Some(InArchive {
                path: path.to_string(),
                outer: outer.to_string(),
                format: "zip",
                entries: 3,
                cached: true,
                read_ms: 0.0,
            });
        }
        self
    }

    /// Holds staging, compressing and extracting back until the returned sender sends (or is dropped).
    pub fn hold_stages(&self) -> async_channel::Sender<()> {
        let (gate, wait) = async_channel::bounded(1);
        self.state.borrow_mut().arch.gate = Some(wait);
        gate
    }

    /// What a staging reports as it starts: (done, total) pairs.
    pub fn script_progress(&self, steps: &[(u64, u64)]) {
        self.state.borrow_mut().arch.progress = steps.to_vec();
    }

    /// The next archive call (staging, compressing, extracting, mounting, a version's copy) fails with this.
    pub fn fail_archive_next(&self, message: &str) {
        self.state.borrow_mut().arch.fail = Some(message.to_string());
    }

    /// The next opening of a file fails with this.
    pub fn fail_open_next(&self, message: &str) {
        self.state.borrow_mut().arch.fail_open = Some(message.to_string());
    }

    /// This item of an archive can't be extracted (password-protected, damaged), for this reason.
    pub fn skip_in_archive(&self, path: &str, reason: &str) {
        self.state.borrow_mut().arch.skip.insert(key(path), reason.to_string());
    }

    /// What the archive file at `path` extracts to: names with sizes.
    pub fn archive_contents(&self, path: &str, files: &[(&str, u64)]) {
        self.state
            .borrow_mut()
            .arch
            .contents
            .insert(key(path), files.iter().map(|(n, s)| (n.to_string(), *s)).collect());
    }

    /// The compressed file at `path` (a .gz) holds one file, which comes out as itself.
    pub fn single_file_archive(&self, path: &str, inner: &str) {
        self.state.borrow_mut().arch.single.insert(key(path), inner.to_string());
    }

    /// The drive the next mount of a disk image gives; it is added to the drives, so reading them finds it.
    pub fn set_mount_drive(&self, drive: Option<&str>) {
        self.state.borrow_mut().arch.mount_drive = drive.map(str::to_string);
    }

    /// Windows' Mount is the default app of disk images (default: it isn't).
    pub fn set_mounts_by_default(&self, yes: bool) {
        self.state.borrow_mut().arch.mounts_by_default = yes;
    }

    /// Every staging asked for, in order.
    pub fn stages(&self) -> Vec<StageCall> {
        self.state.borrow().arch.stages.clone()
    }

    /// Every compression asked for, with its lane.
    pub fn compressions(&self) -> Vec<(CompressRequest, String)> {
        self.state.borrow().arch.compressions.clone()
    }

    /// Every extract here asked for: the archive and the lane.
    pub fn extractions(&self) -> Vec<(String, String)> {
        self.state.borrow().arch.extractions.clone()
    }

    /// The lanes whose staging was asked to stop and hasn't stopped yet.
    pub fn stopped(&self) -> Vec<String> {
        self.state.borrow().arch.stopped.clone()
    }

    /// The staging folders removed.
    pub fn unstaged(&self) -> Vec<String> {
        self.state.borrow().arch.unstaged.clone()
    }

    /// The files in archives extracted to open.
    pub fn extracted_files(&self) -> Vec<String> {
        self.state.borrow().arch.extracted_files.clone()
    }

    /// The images mounted.
    pub fn mounted(&self) -> Vec<String> {
        self.state.borrow().arch.mounted.clone()
    }

    /// The previous versions copied out.
    pub fn version_copies(&self) -> Vec<String> {
        self.state.borrow().arch.version_copies.clone()
    }

    /// What was opened, and how.
    pub fn opened(&self) -> Vec<(String, OpenHow)> {
        self.state.borrow().arch.opened.clone()
    }

    /// The drives asked to eject.
    pub fn ejected(&self) -> Vec<String> {
        self.state.borrow().ejected.clone()
    }

    // ---- Menu actions ------------------------------------------------------------------------------------

    /// The next call of this kind fails with `message`: "sandbox", "shell_open", "reveal", "forget_recent",
    /// "wallpaper", "pin", "sizes", "export", "scripts_folder", "mount", "unmount".
    pub fn fail_action(&self, kind: &'static str, message: &str) {
        self.state.borrow_mut().act.fail.insert(kind, message.to_string());
    }

    /// Turning this picture fails with `message`.
    pub fn fail_rotating(&self, path: &str, message: &str) {
        self.state
            .borrow_mut()
            .act
            .fail_rotate
            .insert(key(path), message.to_string());
    }

    /// Marks a folder as one the index has: it measures as "index", instantly, and an index-only ask answers.
    pub fn set_indexed(&self, path: &str) {
        self.state.borrow_mut().act.indexed.insert(key(path));
    }

    /// What measuring this folder gives, instead of adding up the fake disk.
    pub fn set_measured(&self, path: &str, measured: Measured) {
        self.state.borrow_mut().act.measured.insert(key(path), measured);
    }

    /// Holds measuring back until the returned sender sends (or is dropped).
    pub fn hold_sizes(&self) -> async_channel::Sender<()> {
        let (gate, wait) = async_channel::bounded(1);
        self.state.borrow_mut().act.sizes_gate = Some(wait);
        gate
    }

    /// Holds the ncdu export back until the returned sender sends (or is dropped).
    pub fn hold_export(&self) -> async_channel::Sender<()> {
        let (gate, wait) = async_channel::bounded(1);
        self.state.borrow_mut().act.export_gate = Some(wait);
        gate
    }

    /// What an export reports as it starts: (done, total) pairs.
    pub fn script_export_progress(&self, steps: &[(u64, u64)]) {
        self.state.borrow_mut().act.export_progress = steps.to_vec();
    }

    /// The scripts in the Scripts folder, and where that folder is.
    pub fn set_scripts(&self, folder: &str, scripts: Vec<Script>) {
        let mut state = self.state.borrow_mut();
        state.act.scripts = scripts;
        state.act.scripts_folder = Some(folder.to_string());
    }

    /// The remotes shown in File Explorer on demand.
    pub fn set_mounts(&self, mounts: Vec<OnDemand>) {
        self.state.borrow_mut().act.mounts = mounts;
    }

    /// Files run in Windows Sandbox.
    pub fn sandboxed(&self) -> Vec<String> {
        self.state.borrow().act.sandboxed.clone()
    }

    /// Paths opened as the shell's `open` does.
    pub fn shell_opened(&self) -> Vec<String> {
        self.state.borrow().act.shell_opened.clone()
    }

    /// What was shown in File Explorer.
    pub fn revealed(&self) -> Vec<Vec<String>> {
        self.state.borrow().act.revealed.clone()
    }

    /// What was taken out of Recent.
    pub fn forgotten(&self) -> Vec<Vec<String>> {
        self.state.borrow().act.forgotten.clone()
    }

    /// The pictures set as the desktop background.
    pub fn wallpapers(&self) -> Vec<String> {
        self.state.borrow().act.wallpapers.clone()
    }

    /// The pictures turned: (path, clockwise).
    pub fn rotated(&self) -> Vec<(String, bool)> {
        self.state.borrow().act.rotated.clone()
    }

    /// The cloud pins set.
    pub fn pins(&self) -> Vec<(Vec<String>, PinState)> {
        self.state.borrow().act.pins.clone()
    }

    /// Every measuring asked for, in order.
    pub fn size_calls(&self) -> Vec<SizeCall> {
        self.state.borrow().act.sizing.clone()
    }

    /// The lanes asked to stop measuring.
    pub fn sizes_stopped(&self) -> Vec<String> {
        self.state.borrow().act.sizes_stopped.clone()
    }

    /// The exports asked for: (folder, destination, lane).
    pub fn exports(&self) -> Vec<(String, String, String)> {
        self.state.borrow().act.exports.clone()
    }

    /// The names in a folder as listed (sorted as the engine sorts them, hidden ones included).
    pub fn names(&self, path: &str) -> Vec<String> {
        self.state
            .borrow()
            .folders
            .get(&key(path))
            .map(|f| f.entries.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default()
    }

    /// Whether something is at `path`.
    pub fn exists(&self, path: &str) -> bool {
        entry_of(&self.state.borrow(), path).is_some()
    }

    /// The names in the Recycle Bin.
    pub fn bin_names(&self) -> Vec<String> {
        self.names(RECYCLE_BIN)
    }
}

/// A line of a file a search inside files found: `text[start..end]` is the match.
pub fn inside_match(path: &str, text: &str, line: u64, start: usize, end: usize) -> glint_core::TextMatch {
    glint_core::TextMatch {
        path: path.to_string(),
        text: Some(text.to_string()),
        line_number: Some(line),
        hits: vec![(start, end)],
        label: None,
    }
}

/// One of core's events of the search inside files `id`.
pub fn inside_event(id: &str, said: glint_core::TextSaid) -> glint_core::SearchEvent {
    glint_core::SearchEvent {
        id: id.to_string(),
        said,
    }
}

// ---- A disk that file operations change ----------------------------------------------------------------------

fn entry_of(state: &State, path: &str) -> Option<FileEntry> {
    let parent = parent_path(path);
    state
        .folders
        .get(&key(&parent))?
        .entries
        .iter()
        .find(|e| same_path(&e.path, path))
        .cloned()
}

fn taken(state: &State, folder: &str, name: &str) -> bool {
    state
        .folders
        .get(&key(folder))
        .is_some_and(|f| f.entries.iter().any(|e| e.name.eq_ignore_ascii_case(name)))
}

/// Puts a copy of `source` (and everything below it, for a folder) into `folder` under `name`.
fn copy_tree(state: &mut State, source: &FileEntry, folder: &str, name: &str) -> String {
    let made = with_details(
        RawEntry {
            name: name.to_string(),
            dir: source.dir,
            size: source.size,
            modified: source.modified,
            attrs: source.attrs & !(attr::RECYCLED | attr::IN_ARCHIVE | attr::REMOTE),
            ..Default::default()
        },
        folder,
    );
    let path = made.path.clone();
    let target = state.folders.entry(key(folder)).or_insert_with(empty_folder);
    target.entries.push(made);
    target.entries = sorted(std::mem::take(&mut target.entries));
    target.stamp += 1.0;
    if source.dir {
        let children = state
            .folders
            .get(&key(&source.path))
            .map(|f| f.entries.clone())
            .unwrap_or_default();
        state.folders.insert(key(&path), empty_folder());
        for child in children {
            copy_tree(state, &child, &path, &child.name);
        }
    }
    path
}

/// Takes the item at `path` off the disk, and everything below it.
fn remove_tree(state: &mut State, path: &str) {
    let parent = parent_path(path);
    if let Some(folder) = state.folders.get_mut(&key(&parent)) {
        folder.entries.retain(|e| !same_path(&e.path, path));
        folder.stamp += 1.0;
    }
    let below = format!("{}\\", key(path));
    let own = key(path);
    state.folders.retain(|k, _| *k != own && !k.starts_with(&below));
}

/// "name (2).ext", the way Explorer numbers a name that is taken ("New folder (2)" for a folder).
fn numbered(name: &str, dir: bool, n: u32) -> String {
    if n == 1 {
        return name.to_string();
    }
    match name.rfind('.') {
        Some(dot) if !dir && dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

fn make_numbered(state: &mut State, folder: &str, name: &str, dir: bool) -> Result<String, String> {
    if !state.folders.contains_key(&key(folder)) {
        return Err("The system cannot find the path specified".to_string());
    }
    let free = (1..10_000)
        .map(|n| numbered(name, dir, n))
        .find(|candidate| !taken(state, folder, candidate))
        .ok_or("Couldn't find a free name")?;
    let made = with_details(
        RawEntry {
            name: free,
            dir,
            attrs: if dir { attr::DIRECTORY } else { 0 },
            modified: 100,
            ..Default::default()
        },
        folder,
    );
    let path = made.path.clone();
    let target = state.folders.entry(key(folder)).or_insert_with(empty_folder);
    target.entries.push(made);
    target.entries = sorted(std::mem::take(&mut target.entries));
    target.stamp += 1.0;
    if dir {
        state.folders.insert(key(&path), empty_folder());
    }
    Ok(path)
}

/// Does what Windows' file engine would, on the fake disk. A name that's taken is skipped (Windows would ask).
fn apply(state: &mut State, op: &FileOp) -> Result<Performed, String> {
    if let Some(message) = state.fail.take() {
        return Err(message);
    }
    if std::mem::take(&mut state.cancel) {
        return Ok(Performed {
            completed: false,
            items: Vec::new(),
        });
    }
    let missing = |path: &str| format!("Can't find “{path}”");
    let mut items = Vec::new();
    match op {
        FileOp::Copy { sources, dest } | FileOp::Move { sources, dest } => {
            let moving = matches!(op, FileOp::Move { .. });
            for from in sources {
                let entry = entry_of(state, from).ok_or_else(|| missing(from))?;
                if taken(state, dest, &entry.name) {
                    items.push(ItemResult {
                        from: from.clone(),
                        to: None,
                    });
                    continue;
                }
                let to = copy_tree(state, &entry, dest, &entry.name);
                if moving {
                    remove_tree(state, from);
                }
                items.push(ItemResult {
                    from: from.clone(),
                    to: Some(to),
                });
            }
        }
        FileOp::Delete { paths, permanent, .. } => {
            for from in paths {
                let entry = entry_of(state, from).ok_or_else(|| missing(from))?;
                if *permanent {
                    remove_tree(state, from);
                    items.push(ItemResult {
                        from: from.clone(),
                        to: None,
                    });
                    continue;
                }
                state.bin_seq += 1;
                let staged = format!("$R{}{}", state.bin_seq, entry.name);
                let stored = copy_tree(state, &entry, BIN_STORE, &staged);
                remove_tree(state, from);
                let listed = with_details(
                    RawEntry {
                        name: entry.name.clone(),
                        path: Some(stored.clone()),
                        dir: entry.dir,
                        size: entry.size,
                        modified: 500,
                        attrs: attr::RECYCLED | if entry.dir { attr::DIRECTORY } else { 0 },
                        origin: Some(parent_path(from)),
                    },
                    BIN_STORE,
                );
                let bin = state.folders.entry(key(RECYCLE_BIN)).or_insert_with(empty_folder);
                bin.entries.push(listed);
                bin.entries = sorted(std::mem::take(&mut bin.entries));
                bin.stamp += 1.0;
                items.push(ItemResult {
                    from: from.clone(),
                    to: Some(stored),
                });
            }
        }
        FileOp::Rename { path, name } => {
            let entry = entry_of(state, path).ok_or_else(|| missing(path))?;
            let folder = parent_path(path);
            if taken(state, &folder, name) {
                return Err("A file with that name already exists".to_string());
            }
            let to = copy_tree(state, &entry, &folder, name);
            remove_tree(state, path);
            items.push(ItemResult {
                from: path.clone(),
                to: Some(to),
            });
        }
        FileOp::MoveBack { items: back } | FileOp::Restore { items: back } | FileOp::CopyInto { items: back } => {
            let restoring = matches!(op, FileOp::Restore { .. });
            let copying = matches!(op, FileOp::CopyInto { .. });
            for item in back {
                let entry = entry_of(state, &item.path).ok_or_else(|| missing(&item.path))?;
                if taken(state, &item.folder, &item.name) {
                    items.push(ItemResult {
                        from: item.path.clone(),
                        to: None,
                    });
                    continue;
                }
                let to = copy_tree(state, &entry, &item.folder, &item.name);
                if restoring {
                    remove_tree(state, &item.path);
                    let bin = state.folders.entry(key(RECYCLE_BIN)).or_insert_with(empty_folder);
                    bin.entries.retain(|e| !same_path(&e.path, &item.path));
                    bin.stamp += 1.0;
                } else if !copying {
                    remove_tree(state, &item.path);
                }
                items.push(ItemResult {
                    from: item.path.clone(),
                    to: Some(to),
                });
            }
        }
        FileOp::Purge { paths } => {
            for from in paths {
                remove_tree(state, from);
                let bin = state.folders.entry(key(RECYCLE_BIN)).or_insert_with(empty_folder);
                bin.entries.retain(|e| !same_path(&e.path, from));
                bin.stamp += 1.0;
                items.push(ItemResult {
                    from: from.clone(),
                    to: None,
                });
            }
        }
    }
    Ok(Performed { completed: true, items })
}

/// Waits while staging is held, then stops if the lane was asked to (the ask is used up).
async fn wait_stage(state: &Rc<RefCell<State>>, lane: &str) -> Result<(), String> {
    let gate = state.borrow().arch.gate.clone();
    if let Some(gate) = gate {
        let _ = gate.recv().await;
    }
    let mut state = state.borrow_mut();
    if let Some(at) = state.arch.stopped.iter().position(|l| l == lane) {
        state.arch.stopped.remove(at);
        return Err("cancelled".to_string());
    }
    if let Some(message) = state.arch.fail.take() {
        return Err(message);
    }
    Ok(())
}

/// What a staging sends to its progress sink when it starts.
fn send_progress(state: &Rc<RefCell<State>>, progress: &ProgressSink) {
    let steps = state.borrow().arch.progress.clone();
    for (done, total) in steps {
        let _ = progress.try_send(StageProgress { done, total });
    }
}

/// The bytes of every file under a folder of the fake disk.
fn tree_size(state: &State, folder: &str) -> u64 {
    let Some(found) = state.folders.get(&key(folder)) else {
        return 0;
    };
    found
        .entries
        .iter()
        .map(|e| if e.dir { tree_size(state, &e.path) } else { e.size })
        .sum()
}

/// Takes the failure asked for `kind`, once.
fn failure(state: &Rc<RefCell<State>>, kind: &'static str) -> Option<String> {
    state.borrow_mut().act.fail.remove(kind)
}

/// Waits while the gate is held; true when the lane was asked to stop meanwhile (the ask is used up).
async fn held_and_stopped(
    state: &Rc<RefCell<State>>,
    gate: Option<async_channel::Receiver<()>>,
    lane: &str,
    stopped: fn(&mut State) -> &mut Vec<String>,
) -> bool {
    if let Some(gate) = gate {
        let _ = gate.recv().await;
    }
    let mut state = state.borrow_mut();
    let list = stopped(&mut state);
    match list.iter().position(|l| l == lane) {
        Some(at) => {
            list.remove(at);
            true
        }
        None => false,
    }
}

impl Source for FakeDisk {
    fn list(&self, path: &str) -> Fut<Result<Listed, ListFailure>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            let gate = state.borrow().gates.get(&key(&path)).cloned();
            if let Some(gate) = gate {
                let _ = gate.recv().await;
            }
            let mut state = state.borrow_mut();
            state.listed.push(path.clone());
            match state.folders.get(&key(&path)) {
                Some(folder) if folder.denied => Err(ListFailure {
                    kind: FailureKind::Denied,
                    message: "Access is denied".into(),
                    path: path.clone(),
                    remote: false,
                }),
                Some(folder) => Ok(Listed {
                    path: path.clone(),
                    entries: Arc::from(folder.entries.clone()),
                    elapsed: 1.5,
                    archive: folder.archive.clone(),
                    from_index: folder.from_index,
                    remote: false,
                }),
                None => Err(ListFailure::other(&path, "The system cannot find the path specified")),
            }
        })
    }

    fn stamp(&self, path: &str) -> Fut<Option<f64>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move { state.borrow().folders.get(&key(&path)).map(|f| f.stamp) })
    }

    fn drives(&self) -> Fut<Result<Vec<Drive>, String>> {
        let drives = self.state.borrow().drives.clone();
        Box::pin(async move { Ok(drives) })
    }

    fn known_folders(&self) -> Fut<Vec<KnownFolder>> {
        let known = self.state.borrow().known.clone();
        Box::pin(async move { known })
    }

    fn resolve(&self, text: &str) -> Fut<Result<Resolved, String>> {
        let found = self.state.borrow().resolves.get(text).cloned();
        let text = text.to_string();
        Box::pin(async move { found.ok_or_else(|| format!("Can't find “{text}”.")) })
    }

    fn open_item(&self, path: &str) -> Fut<Result<Option<String>, String>> {
        let found = self.state.borrow().opens.get(path).cloned();
        Box::pin(async move { found.ok_or_else(|| "no app opens it".to_string()) })
    }

    fn grant_access(&self, path: &str) -> Fut<Result<Access, String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            // Granting makes the folder open.
            if let Some(folder) = state.borrow_mut().folders.get_mut(&key(&path)) {
                folder.denied = false;
            }
            Ok(Access::Granted)
        })
    }

    fn watch(&self, folders: Vec<String>) -> bool {
        let mut state = self.state.borrow_mut();
        if !state.window {
            return false;
        }
        state.watched.push(folders);
        true
    }

    fn effect(&self, effect: Effect) {
        self.state.borrow_mut().effects.push(effect);
    }

    fn file_op(&self, op: FileOp) -> Fut<Result<Performed, String>> {
        let state = self.state.clone();
        Box::pin(async move {
            let gate = state.borrow().op_gate.clone();
            if let Some(gate) = gate {
                let _ = gate.recv().await;
            }
            let mut state = state.borrow_mut();
            state.ops.push(op.clone());
            apply(&mut state, &op)
        })
    }

    fn queue_add(&self, kind: QueueKind, sources: Vec<String>, dest: String, verify: bool) -> Fut<Result<u64, String>> {
        let state = self.state.clone();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.queue_error.take() {
                return Err(message);
            }
            state.queued.push((kind, sources, dest, verify));
            Ok(state.queued.len() as u64)
        })
    }

    fn clipboard_set_files(&self, paths: Vec<String>, cut: bool) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.clipboard_fail.take() {
                return Err(message);
            }
            state.clipboard = Some(ClipboardFiles { paths, cut });
            Ok(())
        })
    }

    fn clipboard_get_files(&self) -> Fut<Result<Option<ClipboardFiles>, String>> {
        let state = self.state.clone();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.clipboard_fail.take() {
                return Err(message);
            }
            Ok(state.clipboard.clone())
        })
    }

    fn clipboard_clear(&self) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        Box::pin(async move {
            state.borrow_mut().clipboard = None;
            Ok(())
        })
    }

    fn clipboard_paste_file(&self, folder: &str) -> Fut<Result<Option<String>, String>> {
        let state = self.state.clone();
        let folder = folder.to_string();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            let Some(name) = state.paste_file.take() else {
                return Ok(None);
            };
            make_numbered(&mut state, &folder, &name, false).map(Some)
        })
    }

    fn recycle_empty(&self) -> Fut<Result<bool, String>> {
        let state = self.state.clone();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.fail.take() {
                return Err(message);
            }
            if !state.recycle_answer {
                return Ok(false);
            }
            state.folders.insert(key(RECYCLE_BIN), empty_folder());
            state.folders.insert(key(BIN_STORE), empty_folder());
            Ok(true)
        })
    }

    fn create_entry(&self, parent: &str, name: &str, dir: bool) -> Fut<Result<String, String>> {
        let state = self.state.clone();
        let (parent, name) = (parent.to_string(), name.to_string());
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.fail.take() {
                return Err(message);
            }
            make_numbered(&mut state, &parent, &name, dir)
        })
    }

    fn new_kinds(&self) -> Fut<Result<Vec<NewKind>, String>> {
        let state = self.state.clone();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.fail.take() {
                return Err(message);
            }
            Ok(state.new_kinds.clone())
        })
    }

    fn new_file(&self, folder: &str, ext: &str) -> Fut<Result<Option<String>, String>> {
        let state = self.state.clone();
        let (folder, ext) = (folder.to_string(), ext.to_string());
        Box::pin(async move {
            let mut state = state.borrow_mut();
            let kind = state
                .new_kinds
                .iter()
                .find(|k| k.ext == ext)
                .map(|k| k.name.clone())
                .ok_or_else(|| format!("This PC has no {ext} in its New menu"))?;
            if state.new_pending {
                return Ok(None);
            }
            make_numbered(&mut state, &folder, &format!("New {kind}{ext}"), false).map(Some)
        })
    }

    fn eject(&self, path: &str) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            state.borrow_mut().ejected.push(path);
            Ok(())
        })
    }

    fn stage(
        &self,
        paths: Vec<String>,
        purpose: StagePurpose,
        lane: &str,
        progress: ProgressSink,
    ) -> Fut<Result<Staged, String>> {
        let state = self.state.clone();
        let lane = lane.to_string();
        state.borrow_mut().arch.stages.push(StageCall {
            paths: paths.clone(),
            purpose,
            lane: lane.clone(),
        });
        Box::pin(async move {
            send_progress(&state, &progress);
            wait_stage(&state, &lane).await?;
            let mut state = state.borrow_mut();
            state.arch.stage_seq += 1;
            let folder = format!("{STAGE_ROOT}{}", state.arch.stage_seq);
            state.folders.insert(key(&folder), empty_folder());
            let (mut made, mut skipped, mut bytes) = (Vec::new(), Vec::new(), 0);
            for path in &paths {
                if let Some(reason) = state.arch.skip.get(&key(path)) {
                    skipped.push(Skipped {
                        path: path.clone(),
                        reason: reason.clone(),
                    });
                    continue;
                }
                match entry_of(&state, path) {
                    Some(entry) => {
                        bytes += entry.size;
                        made.push(copy_tree(&mut state, &entry, &folder, &entry.name));
                    }
                    None => skipped.push(Skipped {
                        path: path.clone(),
                        reason: "It isn't in the archive".to_string(),
                    }),
                }
            }
            if made.is_empty() {
                return Err("Nothing could be extracted".to_string());
            }
            Ok(Staged {
                folder,
                paths: made,
                skipped,
                bytes,
            })
        })
    }

    fn compress(&self, request: CompressRequest, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>> {
        let state = self.state.clone();
        let lane = lane.to_string();
        Box::pin(async move {
            send_progress(&state, &progress);
            wait_stage(&state, &lane).await?;
            let mut state = state.borrow_mut();
            state.arch.compressions.push((request.clone(), lane));
            let mut skipped = Vec::new();
            for path in &request.paths {
                if let Some(reason) = state.arch.skip.get(&key(path)) {
                    skipped.push(Skipped {
                        path: path.clone(),
                        reason: reason.clone(),
                    });
                } else if entry_of(&state, path).is_none() {
                    return Err(format!("Can't find “{path}”"));
                }
            }
            let archive = make_numbered(&mut state, &request.folder, &request.name, false)?;
            Ok(Staged {
                folder: request.folder,
                paths: vec![archive],
                skipped,
                bytes: 1,
            })
        })
    }

    fn extract_here(&self, path: &str, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>> {
        let state = self.state.clone();
        let (path, lane) = (path.to_string(), lane.to_string());
        Box::pin(async move {
            send_progress(&state, &progress);
            wait_stage(&state, &lane).await?;
            let mut state = state.borrow_mut();
            state.arch.extractions.push((path.clone(), lane));
            if entry_of(&state, &path).is_none() {
                return Err(format!("Can't find “{path}”"));
            }
            let parent = parent_path(&path);
            if let Some(inner) = state.arch.single.get(&key(&path)).cloned() {
                let file = make_numbered(&mut state, &parent, &inner, false)?;
                return Ok(Staged {
                    folder: file.clone(),
                    paths: vec![file],
                    skipped: Vec::new(),
                    bytes: 1,
                });
            }
            let name = path.rsplit('\\').next().unwrap_or(&path);
            let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem).to_string();
            let folder = make_numbered(&mut state, &parent, &stem, true)?;
            let files = state
                .arch
                .contents
                .get(&key(&path))
                .cloned()
                .unwrap_or_else(|| vec![("readme.txt".to_string(), 10)]);
            let mut made = Vec::new();
            for (name, size) in files {
                let made_path = make_numbered(&mut state, &folder, &name, false)?;
                if let Some(entry) = state
                    .folders
                    .get_mut(&key(&folder))
                    .and_then(|f| f.entries.iter_mut().find(|e| same_path(&e.path, &made_path)))
                {
                    entry.size = size;
                }
                made.push(made_path);
            }
            Ok(Staged {
                folder,
                paths: made,
                skipped: Vec::new(),
                bytes: 1,
            })
        })
    }

    fn cancel_stage(&self, lane: &str) {
        self.state.borrow_mut().arch.stopped.push(lane.to_string());
    }

    fn unstage(&self, folder: &str) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let folder = folder.to_string();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            state.arch.unstaged.push(folder.clone());
            remove_tree(&mut state, &folder);
            Ok(())
        })
    }

    fn extract_file(&self, path: &str) -> Fut<Result<String, String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.arch.fail.take() {
                return Err(message);
            }
            let entry = entry_of(&state, &path).ok_or_else(|| "That isn't in an archive".to_string())?;
            state.arch.extracted_files.push(path);
            // Made once and reused.
            if taken(&state, OPEN_ROOT, &entry.name) {
                return Ok(format!("{OPEN_ROOT}\\{}", entry.name));
            }
            Ok(copy_tree(&mut state, &entry, OPEN_ROOT, &entry.name))
        })
    }

    fn mount_image(&self, path: &str) -> Fut<Result<Option<String>, String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.arch.fail.take() {
                return Err(message);
            }
            state.arch.mounted.push(path);
            let drive = state.arch.mount_drive.clone();
            if let Some(drive) = &drive {
                state.drives.push(Drive {
                    path: drive.clone(),
                    label: "CD Drive".to_string(),
                    kind: DriveKind::Cdrom,
                    fs: "UDF".to_string(),
                    total: 100,
                    free: 0,
                });
            }
            Ok(drive)
        })
    }

    fn mounts_by_default(&self, _path: &str) -> Fut<bool> {
        let yes = self.state.borrow().arch.mounts_by_default;
        Box::pin(async move { yes })
    }

    fn version_copy(&self, path: &str) -> Fut<Result<String, String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.arch.fail.take() {
                return Err(message);
            }
            let name = path.rsplit('\\').next().unwrap_or(&path).to_string();
            state.arch.version_copies.push(path);
            Ok(format!("{VERSION_ROOT}\\{name}"))
        })
    }

    fn open_with(&self, path: &str, how: &OpenHow) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let (path, how) = (path.to_string(), how.clone());
        Box::pin(async move {
            let mut state = state.borrow_mut();
            state.arch.opened.push((path, how));
            match state.arch.fail_open.take() {
                Some(message) => Err(message),
                None => Ok(()),
            }
        })
    }

    fn all_exist(&self, paths: Vec<String>) -> Fut<bool> {
        let state = self.state.clone();
        Box::pin(async move {
            let state = state.borrow();
            paths.iter().all(|p| entry_of(&state, p).is_some())
        })
    }

    fn find(
        &self,
        lane: &str,
        roots: Vec<String>,
        query: String,
        options: FindOptions,
    ) -> Fut<Result<async_channel::Receiver<Found>, String>> {
        let state = self.state.clone();
        let call = FindCall {
            lane: lane.to_string(),
            roots,
            query,
            options,
        };
        Box::pin(async move {
            let mut state = state.borrow_mut();
            state.finds.push(call.clone());
            if let Some(message) = state.find_error.take() {
                return Err(message);
            }
            if let Some(stream) = state.live_finds.remove(&call.query) {
                return Ok(stream);
            }
            let (sender, stream) = async_channel::unbounded();
            for found in walk(&state, &call) {
                let _ = sender.try_send(found);
            }
            Ok(stream)
        })
    }

    fn cancel_find(&self, lane: &str) {
        self.state.borrow_mut().cancelled.push(lane.to_string());
    }

    fn inside_covers(&self, _path: &str) -> Fut<bool> {
        let covered = self.state.borrow().inside_covered;
        Box::pin(async move { covered })
    }

    fn inside_start(
        &self,
        engine: super::inside::InsideEngine,
        request: super::inside::InsideRequest,
    ) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            state.inside.push((engine, request));
            match state.inside_error.take() {
                Some(message) => Err(message),
                None if state.inside_live => Ok(()),
                // Nobody sends this search's events: it ends at once, quietly, as a stopped one does.
                None => Err("cancelled".to_string()),
            }
        })
    }

    fn inside_cancel(&self) {
        self.state.borrow_mut().inside_cancels += 1;
    }

    fn entries_of(&self, paths: Vec<String>) -> Fut<Vec<FileEntry>> {
        let state = self.state.clone();
        Box::pin(async move {
            let state = state.borrow();
            paths
                .iter()
                .map(|path| {
                    // A file the fake disk doesn't hold still gets its row, as on disk.
                    entry_of(&state, path).unwrap_or_else(|| {
                        glint_model::entries::with_details(
                            glint_model::entries::RawEntry {
                                name: glint_model::paths::base_name(path).to_string(),
                                path: Some(path.clone()),
                                ..Default::default()
                            },
                            "",
                        )
                    })
                })
                .collect()
        })
    }

    fn run_in_sandbox(&self, path: &str) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            if let Some(message) = failure(&state, "sandbox") {
                return Err(message);
            }
            state.borrow_mut().act.sandboxed.push(path);
            Ok(())
        })
    }

    fn shell_open(&self, path: &str) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            if let Some(message) = failure(&state, "shell_open") {
                return Err(message);
            }
            state.borrow_mut().act.shell_opened.push(path);
            Ok(())
        })
    }

    fn reveal(&self, paths: Vec<String>) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        Box::pin(async move {
            if let Some(message) = failure(&state, "reveal") {
                return Err(message);
            }
            state.borrow_mut().act.revealed.push(paths);
            Ok(())
        })
    }

    fn forget_recent(&self, paths: Vec<String>) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        Box::pin(async move {
            if let Some(message) = failure(&state, "forget_recent") {
                return Err(message);
            }
            state.borrow_mut().act.forgotten.push(paths);
            Ok(())
        })
    }

    fn set_wallpaper(&self, path: &str) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            if let Some(message) = failure(&state, "wallpaper") {
                return Err(message);
            }
            state.borrow_mut().act.wallpapers.push(path);
            Ok(())
        })
    }

    fn rotate_image(&self, path: &str, clockwise: bool) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let path = path.to_string();
        Box::pin(async move {
            let mut state = state.borrow_mut();
            if let Some(message) = state.act.fail_rotate.get(&key(&path)) {
                return Err(message.clone());
            }
            state.act.rotated.push((path, clockwise));
            Ok(())
        })
    }

    fn set_cloud_pin(&self, paths: Vec<String>, pin: PinState) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        Box::pin(async move {
            if let Some(message) = failure(&state, "pin") {
                return Err(message);
            }
            state.borrow_mut().act.pins.push((paths, pin));
            Ok(())
        })
    }

    fn folder_sizes(&self, path: &str, lane: &str, index_only: bool) -> Fut<Result<Measured, String>> {
        let state = self.state.clone();
        let (path, lane) = (path.to_string(), lane.to_string());
        let gate = {
            let mut state = state.borrow_mut();
            state.act.sizing.push(SizeCall {
                path: path.clone(),
                lane: lane.clone(),
                index_only,
            });
            state.act.sizes_gate.clone()
        };
        Box::pin(async move {
            if held_and_stopped(&state, gate, &lane, |st| &mut st.act.sizes_stopped).await {
                return Err("cancelled".to_string());
            }
            if let Some(message) = failure(&state, "sizes") {
                return Err(message);
            }
            let state = state.borrow();
            let indexed = state.act.indexed.contains(&key(&path));
            if index_only && !indexed {
                return Err("not indexed".to_string());
            }
            if let Some(forced) = state.act.measured.get(&key(&path)) {
                return Ok(forced.clone());
            }
            let sizes: Vec<(String, u64)> = state
                .folders
                .get(&key(&path))
                .map(|f| {
                    f.entries
                        .iter()
                        .filter(|e| e.dir)
                        .map(|e| (e.name.clone(), tree_size(&state, &e.path)))
                        .collect()
                })
                .unwrap_or_default();
            Ok(Measured {
                total: tree_size(&state, &path),
                sizes,
                elapsed: 1.5,
                engine: if indexed { "index" } else { "pdu" }.to_string(),
            })
        })
    }

    fn cancel_folder_sizes(&self, lane: &str) {
        self.state.borrow_mut().act.sizes_stopped.push(lane.to_string());
    }

    fn export_ncdu(&self, path: &str, dest: &str, lane: &str, progress: ProgressSink) -> Fut<Result<Staged, String>> {
        let state = self.state.clone();
        let (path, dest, lane) = (path.to_string(), dest.to_string(), lane.to_string());
        let (gate, steps) = {
            let mut state = state.borrow_mut();
            state.act.exports.push((path.clone(), dest.clone(), lane.clone()));
            (state.act.export_gate.clone(), state.act.export_progress.clone())
        };
        Box::pin(async move {
            for (done, total) in steps {
                let _ = progress.try_send(StageProgress { done, total });
            }
            if held_and_stopped(&state, gate, &lane, |st| &mut st.arch.stopped).await {
                return Err("cancelled".to_string());
            }
            if let Some(message) = failure(&state, "export") {
                return Err(message);
            }
            Ok(Staged {
                folder: path,
                paths: vec![dest],
                skipped: Vec::new(),
                bytes: 0,
            })
        })
    }

    fn scripts(&self) -> Fut<Result<Vec<Script>, String>> {
        let scripts = self.state.borrow().act.scripts.clone();
        Box::pin(async move { Ok(scripts) })
    }

    fn scripts_folder(&self) -> Fut<Result<String, String>> {
        let state = self.state.clone();
        Box::pin(async move {
            if let Some(message) = failure(&state, "scripts_folder") {
                return Err(message);
            }
            let folder = state.borrow().act.scripts_folder.clone();
            Ok(folder.unwrap_or_else(|| "C:\\Glint\\scripts".to_string()))
        })
    }

    fn on_demand_list(&self) -> Fut<Vec<OnDemand>> {
        let mounts = self.state.borrow().act.mounts.clone();
        Box::pin(async move { mounts })
    }

    fn on_demand_mount(&self, id: &str, name: &str) -> Fut<Result<String, String>> {
        let state = self.state.clone();
        let (id, name) = (id.to_string(), name.to_string());
        Box::pin(async move {
            if let Some(message) = failure(&state, "mount") {
                return Err(message);
            }
            let folder = format!("C:\\Users\\me\\Glint remotes\\{name}");
            let mut state = state.borrow_mut();
            state.act.mounts.retain(|m| m.id != id);
            state.act.mounts.push(OnDemand {
                id,
                folder: folder.clone(),
            });
            Ok(folder)
        })
    }

    fn on_demand_unmount(&self, id: &str) -> Fut<Result<(), String>> {
        let state = self.state.clone();
        let id = id.to_string();
        Box::pin(async move {
            if let Some(message) = failure(&state, "unmount") {
                return Err(message);
            }
            state.borrow_mut().act.mounts.retain(|m| m.id != id);
            Ok(())
        })
    }

    fn search_index(&self) -> Fut<Option<SearchIndex>> {
        let mut state = self.state.borrow_mut();
        state.index_asked += 1;
        let index = state.index.clone();
        Box::pin(async move { index })
    }
}

/// What a search says when the fake walks its folders (fd's way): every entry below the roots whose name has all
/// the query's terms in it, sorted by path, cut at the result limit. The index answers instead when it has the
/// drives.
fn walk(state: &State, call: &FindCall) -> Vec<Found> {
    let terms: Vec<String> = glint_model::entries::terms(&call.query.to_lowercase());
    let covered = state
        .index
        .as_ref()
        .is_some_and(|index| !call.roots.is_empty() && call.roots.iter().all(|root| index.indexed(root)));
    let mut found: Vec<FileEntry> = Vec::new();
    for (folder, listed) in &state.folders {
        let below = |root: &str| {
            let root = key(root);
            *folder == root || folder.starts_with(&format!("{root}\\"))
        };
        if !call.roots.iter().any(|root| below(root))
            || folder.starts_with(&key(RECYCLE_BIN))
            || folder.starts_with(&key(BIN_STORE))
        {
            continue;
        }
        for entry in &listed.entries {
            let skipped = call
                .options
                .exclude
                .iter()
                .any(|name| entry.name.eq_ignore_ascii_case(name));
            let veiled = entry.attrs & attr::HIDDEN != 0 && !call.options.hidden;
            if !skipped && !veiled && terms.iter().all(|term| entry.lower.contains(term.as_str())) {
                found.push(entry.clone());
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    let limited = call.options.max_results.is_some_and(|max| found.len() > max);
    if let Some(max) = call.options.max_results {
        found.truncate(max);
    }
    let count = found.len();
    let mut said = vec![Found::Started {
        engine: if covered { "index" } else { "fd" }.to_string(),
    }];
    if !found.is_empty() {
        said.push(Found::Entries(found));
    }
    said.push(Found::Finished {
        count,
        elapsed: 1.0,
        limited,
        fuzzy: false,
    });
    said
}
