//! What the file operations keep in the [`Explorer`](super::Explorer): the items cut to the clipboard, Undo's
//! steps, the slow work that is running, the copy queue's jobs as last seen and the New menu's kinds.
//! (The TypeScript kept these in module variables and in the store's `cut`.)

use super::ops_types::*;
use gpui_kit::Task;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Undo remembers this many steps (`rememberUndo` kept 50).
pub(super) const UNDO_MOST: usize = 50;

/// One step Ctrl+Z can take back.
pub(super) struct UndoStep {
    pub id: UndoId,
    /// "move of 3 items", as in "Undid the move of 3 items".
    pub label: String,
    pub action: UndoAction,
}

/// Slow work in progress (`busy`): its toast shows once it has run for half a second.
pub(super) struct BusyWork {
    pub what: Working,
    pub shown: bool,
    /// Waits out the half second; dropped (cancelled) when the work ends.
    pub _timer: Option<Task<()>>,
}

/// The state of the file operations part.
pub(super) struct OpsState {
    /// Paths cut to the clipboard here, shown dimmed until pasted (`cut`).
    pub cut: Arc<HashSet<String>>,
    pub undo: Vec<UndoStep>,
    pub undo_seq: u64,
    pub busy: HashMap<u64, BusyWork>,
    pub busy_seq: u64,
    /// The copy queue's jobs by id as last told, and whether each had finished: a job that goes from running to
    /// finished is a copy or move that is done.
    pub queue_seen: HashMap<u64, bool>,
    /// The New menu's kinds as last read.
    pub new_kinds: Arc<[NewKind]>,
    /// When they were last asked for: they are read again after 30 s (an app installed since shows up).
    pub kinds_asked: Option<Instant>,
    pub kinds_loading: bool,
}

impl OpsState {
    pub fn new() -> Self {
        OpsState {
            cut: Arc::new(HashSet::new()),
            undo: Vec::new(),
            undo_seq: 0,
            busy: HashMap::new(),
            busy_seq: 0,
            queue_seen: HashMap::new(),
            new_kinds: Arc::from(Vec::new()),
            kinds_asked: None,
            kinds_loading: false,
        }
    }
}
