use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex, MutexGuard};

/// Holds one running sidecar (the text search, or a tab's file search or folder sizing):
/// starting a new run stops the previous one.
#[derive(Clone, Default)]
pub struct Slot(Arc<Mutex<Option<Child>>>);

impl Slot {
    fn lock(&self) -> MutexGuard<'_, Option<Child>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stops whatever is running and tracks `child` instead; returns its pid.
    pub fn replace(&self, child: Child) -> u32 {
        let pid = child.id();
        if let Some(old) = self.lock().replace(child) {
            stop(old);
        }
        pid
    }

    /// Stops the current process, if any.
    pub fn kill(&self) {
        if let Some(child) = self.lock().take() {
            stop(child);
        }
    }

    /// Stops `pid` unless a newer run already replaced it.
    pub fn kill_if(&self, pid: u32) {
        let mut current = self.lock();
        if current.as_ref().map(Child::id) == Some(pid) {
            stop(current.take().unwrap());
        }
    }

    /// Reaps `pid` after it ended on its own. Returns false if it was stopped or replaced instead.
    pub fn finish(&self, pid: u32) -> bool {
        let mut current = self.lock();
        if current.as_ref().map(Child::id) != Some(pid) {
            return false;
        }
        let _ = current.take().unwrap().wait();
        true
    }
}

/// A [`Slot`] per lane, for work each tab runs on its own (folder sizes): a new run stops only the
/// previous one in the same lane.
#[derive(Clone, Default)]
pub struct Lanes(Arc<Mutex<HashMap<String, Slot>>>);

impl Lanes {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The lane's slot, made on first use.
    pub fn slot(&self, lane: &str) -> Slot {
        self.lock().entry(lane.to_string()).or_default().clone()
    }

    /// Stops what runs in the lane and forgets it (a later run makes it again).
    pub fn close(&self, lane: &str) {
        let slot = self.lock().remove(lane);
        if let Some(slot) = slot {
            slot.kill();
        }
    }
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}
