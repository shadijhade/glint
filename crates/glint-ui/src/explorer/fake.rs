//! A fake disk for the model tests: folders with entries, held in memory, behind the same [`Source`] the app
//! uses for glint-core. Listings can be held back and released, to test results that arrive out of order.

use super::source::{Access, Effect, Fut, Resolved, Source};
use super::types::*;
use glint_model::entries::{attr, sort_entries, with_details, FileEntry, RawEntry, Sort, SortKey};
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
}

/// The fake disk and the source reading it. Cloning shares the disk.
#[derive(Clone, Default)]
pub struct FakeDisk {
    state: Rc<RefCell<State>>,
}

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

impl FakeDisk {
    pub fn new() -> Self {
        let disk = FakeDisk::default();
        disk.state.borrow_mut().window = true;
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
}
