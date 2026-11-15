//! What Glint knows about what the pointer rests on and what is selected, kept ready before it is asked for
//! (docs/gpui/aware/PLAN.md).
//!
//! One [`Aware`] entity for the app: it keeps what the engine found out about files ([`Aware::probed`]), asks for
//! what isn't known yet on the quiet lane ([`Aware::look_ahead`]) or at once ([`Aware::look_now`]), watches the
//! clipboard ([`Aware::clip`]) and the folder's changes ([`Aware::pulse`]). The free functions are the only door
//! to what the person did before ([`note_action`], [`suggest`], [`places_for`]). Views observe the entity and
//! redraw when it changes; it draws nothing.
//!
//! Card D1 of docs/gpui/aware fills this in: the signatures are the contract; until then nothing is asked, nothing
//! is known and nothing is counted.

mod clip;
mod model;
mod pulse;
#[cfg(test)]
mod tests;

use crate::kit::prelude::*;
use glint_core::aware::{Answer, Ask, Lane};
use glint_model::aware::{Clip, Facts, Pick, Place, Probed, Shape};
use glint_model::entries::FileEntry;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Starts probing these asks on a lane, to be stopped by the flag; the answers arrive on the receiver.
pub type Probe = Rc<dyn Fn(Vec<Ask>, Lane, Arc<AtomicBool>) -> async_channel::Receiver<Answer>>;
/// Reads what is on the clipboard now.
pub type ReadClip = Rc<dyn Fn(&mut App) -> Task<Clip>>;

/// What the entity reads and starts that isn't the app's own state: the engine and Windows in the app, fakes in
/// tests.
#[derive(Clone)]
pub struct Sources {
    /// Starts probing; the answers arrive on the receiver.
    pub probe: Probe,
    /// Windows' count of clipboard changes.
    pub clip_seq: Rc<dyn Fn() -> u32>,
    /// What is on the clipboard now.
    pub clip: ReadClip,
    /// Today, in days since 1970-01-01.
    pub today: Rc<dyn Fn() -> u32>,
}

impl Sources {
    /// The engine and Windows.
    pub fn core() -> Sources {
        Sources::still()
    }

    /// Nothing ever answers, the clipboard is empty and never changes, and it is always day 20000 (tests).
    pub fn still() -> Sources {
        Sources {
            probe: Rc::new(|_, _, _| async_channel::unbounded().1),
            clip_seq: Rc::new(|| 0),
            clip: Rc::new(|_| Task::ready(Clip::Empty)),
            today: Rc::new(|| 20_000),
        }
    }
}

/// What is known about files, the clipboard and the folder's changes, kept for the menus and the dock.
pub struct Aware {
    sources: Sources,
    clip: Clip,
}

struct Shared(Entity<Aware>);

impl Global for Shared {}

/// Start-up: the entity, reading the engine and Windows. Call after the explorer and the saved state exist.
pub fn init(cx: &mut App) {
    init_with(cx, Sources::core());
}

/// [`init`] with sources of your own (tests).
pub fn init_with(cx: &mut App, sources: Sources) {
    let entity = cx.new(|_| Aware { sources, clip: Clip::Empty });
    cx.set_global(Shared(entity));
}

/// The app's entity. Panics before [`init`]: views made at start-up may rely on it.
pub fn shared(cx: &App) -> Entity<Aware> {
    cx.global::<Shared>().0.clone()
}

/// The app's entity, if there is one (tests of a single view often have none).
pub fn try_shared(cx: &App) -> Option<Entity<Aware>> {
    cx.try_global::<Shared>().map(|shared| shared.0.clone())
}

impl Aware {
    /// The pointer rests on these, or they were just selected: finds out what isn't known about them, quietly.
    pub fn look_ahead(&mut self, entries: &[FileEntry], cx: &mut Context<Self>) {
        let _ = (entries, cx);
    }

    /// Something on screen shows these now: finds out what isn't known about them at once.
    pub fn look_now(&mut self, entries: &[FileEntry], cx: &mut Context<Self>) {
        let _ = (entries, cx);
    }

    /// What was being found out is no longer wanted (a menu closed): stops asking.
    pub fn rest(&mut self) {}

    /// What has been found out about `entry`, if anything has and the file hasn't changed since.
    pub fn probed(&self, entry: &FileEntry) -> Option<&Probed> {
        let _ = entry;
        None
    }

    /// Forgets what is known about `path` (it was unblocked, renamed, written to).
    pub fn forget(&mut self, path: &str) {
        let _ = path;
    }

    /// What is on the clipboard.
    pub fn clip(&self) -> &Clip {
        &self.clip
    }

    /// The folder's changes over the last minute: thirty counts, two seconds each, the newest last. None when
    /// nothing changed in it.
    pub fn pulse(&self, folder: &str) -> Option<[u8; 30]> {
        let _ = folder;
        None
    }

    /// Whether the folder is changing now: three or more changes in the last ten seconds.
    pub fn changing(&self, folder: &str) -> bool {
        let _ = folder;
        false
    }

    /// Today, as the sources have it.
    pub fn today(&self) -> u32 {
        (self.sources.today)()
    }
}

/// What is known at once about `targets` (no I/O): what they are, and what is on the clipboard.
pub fn facts(cx: &App, targets: &[FileEntry]) -> Facts {
    let _ = cx;
    Facts { shape: Shape::of(targets), count: targets.len(), ..Facts::default() }
}

/// The rows to put forward for `facts`, best first, at most `most`: `rank::suggest` with the saved habits and
/// today's date. Empty when the person turned suggestions off.
pub fn suggest(cx: &App, facts: &Facts, probed: Option<&Probed>, available: &[&str], most: usize) -> Vec<Pick> {
    let _ = (cx, facts, probed, available, most);
    Vec::new()
}

/// Counts `key`, chosen for something of `shape`. Nothing when the person turned learning off.
pub fn note_action(cx: &mut App, shape: Shape, key: &str) {
    let _ = (cx, shape, key);
}

/// Counts `folder` as where files with these extensions went. Nothing when the person turned learning off.
pub fn note_place(cx: &mut App, exts: &[String], folder: &str) {
    let _ = (cx, exts, folder);
}

/// The folders files with extension `ext` have gone to, heaviest first. Empty when learning or suggestions are off.
pub fn places_for(cx: &App, ext: &str) -> Vec<Place> {
    let _ = (cx, ext);
    Vec::new()
}

/// Forgets what was learned.
pub fn forget_habits(cx: &mut App) {
    let _ = cx;
}
