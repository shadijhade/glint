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

    /// Reaps `pid` after it ended on its own, with its exit status. None if it was stopped or
    /// replaced instead.
    pub fn finish(&self, pid: u32) -> Option<std::process::ExitStatus> {
        let mut current = self.lock();
        if current.as_ref().map(Child::id) != Some(pid) {
            return None;
        }
        current.take().unwrap().wait().ok()
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

    /// Stops every lane's work and forgets the lanes.
    pub fn close_all(&self) {
        let slots: Vec<Slot> = self.lock().drain().map(|(_, slot)| slot).collect();
        for slot in slots {
            slot.kill();
        }
    }
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// A bundled CLI tool (`rg`, `fd`, `dust`): the setup program puts it next to the app, and
/// tauri-build copies it next to every dev build. Only that copy runs (SECURITY.md), never one found
/// on PATH, and without a console window.
pub fn tool(name: &str) -> Result<std::process::Command, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?.with_file_name(format!("{name}.exe"));
    if !exe.is_file() {
        return Err(format!("{name}.exe is missing next to the app; reinstall the app"));
    }
    let mut command = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}
