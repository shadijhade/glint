//! A fake disk for the model tests: folders with entries, held in memory, behind the same [`Source`] the app
//! uses for glint-core. Listings can be held back and released, to test results that arrive out of order.

use super::ops_types::*;
use super::source::{Access, Effect, Fut, Resolved, Source};
use super::types::*;
use glint_model::entries::{attr, sort_entries, with_details, FileEntry, RawEntry, Sort, SortKey};
use glint_model::paths::{parent_path, same_path, RECYCLE_BIN};
use std::cell::RefCell;
use std::collections::HashMap;
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
            },
        );
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

    /// The drives asked to eject.
    pub fn ejected(&self) -> Vec<String> {
        self.state.borrow().ejected.clone()
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
            attrs: source.attrs & !attr::RECYCLED,
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
                    archive: None,
                    from_index: false,
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
}
