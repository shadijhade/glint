//! What Glint knows about what the pointer rests on and what is selected, kept ready before it is asked for
//! (docs/gpui/aware/PLAN.md).
//!
//! One [`Aware`] entity for the app: it keeps what the engine found out about files ([`Aware::probed`]), asks for
//! what isn't known yet on the quiet lane ([`Aware::look_ahead`]) or at once ([`Aware::look_now`]), watches the
//! clipboard ([`Aware::clip`]) and the folder's changes ([`Aware::pulse`]). The free functions are the only door
//! to what the person did before ([`note_action`], [`suggest`], [`places_for`]). Views observe the entity and
//! redraw when it changes; it draws nothing.
//!
//! Card D1 of docs/gpui/aware. Nothing here waits: every wait is a task, and what arrives from the engine is
//! applied at most once a frame (R4).

mod clip;
mod model;
mod pulse;
#[cfg(test)]
mod tests;

use crate::core_bridge::{self, CoreEvent};
use crate::explorer::{self, DriveKind, ExplorerEvent};
use crate::kit::prelude::*;
use crate::{persistence, state};
use glint_core::aware::{Answer, Ask, Lane};
use glint_model::aware::habits::day_of;
use glint_model::aware::{rank, shared_ext, Clip, Facts, Habits, Pick, Place, Probed, Shape, Want};
use glint_model::entries::{is_archived, is_closed, is_recycled, is_remote, FileEntry};
use model::{Cache, Roots};
use pulse::Pulses;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The most entries one look asks about: a bigger selection costs the dock nothing (PLAN.md section 8).
pub const MOST_TO_LOOK_AT: usize = 200;
/// How long the selection stays the same before it is looked at.
const SELECTION_SETTLES: Duration = Duration::from_millis(150);
/// How long the pointer rests on a row or a tile before what isn't known about its item is asked for: long
/// enough that a pointer crossing the list asks nothing, short enough to be done before the click.
pub const RESTS_FOR: Duration = Duration::from_millis(120);
/// Answers are applied at most once in this long: a frame.
const FRAME: Duration = Duration::from_millis(16);
/// "Changing now" goes away this long after the last change (ten seconds, and a little more).
const CHANGING_ENDS: Duration = Duration::from_millis(10_500);

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
        Sources {
            probe: Rc::new(|asks, lane, stop| {
                let (channel, receiver) = core_bridge::stream::<Answer>();
                glint_core::aware::probe(asks, lane, stop, channel);
                receiver
            }),
            clip_seq: Rc::new(|| {
                // SAFETY: a plain query of Windows' clipboard change count, valid from any thread.
                unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() }
            }),
            clip: Rc::new(clip::read),
            today: Rc::new(|| {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
                day_of(i64::try_from(now).unwrap_or(i64::MAX))
            }),
        }
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

/// A set of asks on its way: the flag that ends it, whether it has finished by itself, and what it asked.
struct Run {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    asked: Vec<(String, Want)>,
}

/// What is known about files, the clipboard and the folder's changes, kept for the menus and the dock.
pub struct Aware {
    sources: Sources,
    cache: Cache,
    quiet: Option<Run>,
    now: Option<Run>,
    clip: Clip,
    /// The last clipboard count read, so the same one is not read twice.
    clip_seen: Option<u32>,
    pulses: Pulses,
    /// The wait before the selection is looked at; replaced by the next change of selection.
    settle: Option<Task<()>>,
    /// The wait that tells the views "changing now" is over; replaced by the next change.
    changing_ends: Option<Task<()>>,
    _clip_watch: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

struct Shared(Entity<Aware>);

impl Global for Shared {}

/// Start-up: the entity, reading the engine and Windows. Call after the explorer and the saved state exist.
pub fn init(cx: &mut App) {
    init_with(cx, Sources::core());
}

/// [`init`] with sources of your own (tests).
pub fn init_with(cx: &mut App, sources: Sources) {
    let entity = cx.new(|cx| Aware::new(sources, cx));
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

/// Today, from the entity's sources, or from the clock when there is no entity.
fn today(cx: &App) -> u32 {
    match try_shared(cx) {
        Some(aware) => aware.read(cx).today(),
        None => {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
            day_of(i64::try_from(now).unwrap_or(i64::MAX))
        }
    }
}

impl Aware {
    /// The entity: it listens to the explorer's selection and the folder watcher, and watches the clipboard.
    /// Nothing here needs an explorer to exist.
    fn new(sources: Sources, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(explorer) = explorer::try_explorer(cx) {
            subscriptions.push(cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| {
                this.on_explorer(event, cx);
            }));
        }
        subscriptions.extend(core_bridge::subscribe(cx, |this, event, cx| this.on_core(event, cx)));
        Aware {
            sources,
            cache: Cache::default(),
            quiet: None,
            now: None,
            clip: Clip::Empty,
            clip_seen: None,
            pulses: Pulses::default(),
            settle: None,
            changing_ends: None,
            _clip_watch: Some(clip::watch(cx)),
            _subscriptions: subscriptions,
        }
    }

    /// The pointer rests on these, or they were just selected: finds out what isn't known about them, quietly.
    pub fn look_ahead(&mut self, entries: &[FileEntry], cx: &mut Context<Self>) {
        self.look(entries, Lane::Quiet, cx);
    }

    /// Something on screen shows these now: finds out what isn't known about them at once.
    pub fn look_now(&mut self, entries: &[FileEntry], cx: &mut Context<Self>) {
        self.look(entries, Lane::Now, cx);
    }

    /// What was being found out is no longer wanted (a menu closed): stops asking.
    pub fn rest(&mut self) {
        self.stop(Lane::Quiet);
        self.stop(Lane::Now);
    }

    /// What has been found out about `entry`, if anything has and the file hasn't changed since.
    pub fn probed(&self, entry: &FileEntry) -> Option<&Probed> {
        self.cache.probed(entry)
    }

    /// Forgets what is known about `path` (it was unblocked, renamed, written to).
    pub fn forget(&mut self, path: &str) {
        self.cache.forget(path);
    }

    /// What is on the clipboard.
    pub fn clip(&self) -> &Clip {
        &self.clip
    }

    /// The folder's changes over the last minute: thirty counts, two seconds each, the newest last. None when
    /// nothing changed in it.
    pub fn pulse(&self, folder: &str) -> Option<[u8; 30]> {
        self.pulses.pulse(folder, Instant::now())
    }

    /// Whether the folder is changing now: three or more changes in the last ten seconds.
    pub fn changing(&self, folder: &str) -> bool {
        self.pulses.changing(folder, Instant::now())
    }

    /// Today, as the sources have it.
    pub fn today(&self) -> u32 {
        (self.sources.today)()
    }

    /// Ends the run on `lane`. What it hadn't answered may be asked again, unless it had finished by itself
    /// (then what it didn't answer can't be known, and stays unasked-about).
    fn stop(&mut self, lane: Lane) {
        let run = match lane {
            Lane::Quiet => self.quiet.take(),
            Lane::Now => self.now.take(),
        };
        let Some(run) = run else {
            return;
        };
        run.stop.store(true, Ordering::SeqCst);
        if !run.done.load(Ordering::SeqCst) {
            for (path, want) in run.asked {
                self.cache.unasked(&path, want);
            }
        }
    }

    /// The drives a look may ask about.
    fn roots(cx: &App) -> Option<Roots> {
        let explorer = explorer::try_explorer(cx)?;
        let drives = explorer.read(cx).drives();
        Some(Roots(
            drives
                .iter()
                .map(|drive| (drive.path.to_lowercase(), drive.kind == DriveKind::Fixed))
                .collect(),
        ))
    }

    fn look(&mut self, entries: &[FileEntry], lane: Lane, cx: &mut Context<Self>) {
        let now = Instant::now();
        // What to ask is worked out before the last run of this lane is stopped, so a second look at what that
        // run is still finding out asks nothing; what it hadn't answered is let go of by the stop, and is asked
        // by the next look.
        let mut planned: Vec<(&FileEntry, Want)> = Vec::new();
        if let Some(roots) = Self::roots(cx) {
            for entry in entries.iter().take(MOST_TO_LOOK_AT) {
                if !roots.fixed(&entry.path) {
                    continue;
                }
                let want = self.cache.plan(entry, now);
                if !want.is_empty() {
                    planned.push((entry, want));
                }
            }
        }
        self.stop(lane);
        let mut asks = Vec::new();
        for (entry, want) in planned {
            self.cache.asking(entry, want, now);
            asks.push(Ask { path: entry.path.clone(), dir: entry.dir, size: entry.size, want });
        }
        if asks.is_empty() {
            return;
        }
        let run = Run {
            stop: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(false)),
            asked: asks.iter().map(|ask| (ask.path.clone(), ask.want)).collect(),
        };
        let receiver = (self.sources.probe)(asks, lane, run.stop.clone());
        let (stop, done) = (run.stop.clone(), run.done.clone());
        match lane {
            Lane::Quiet => self.quiet = Some(run),
            Lane::Now => self.now = Some(run),
        }
        // One task per run: answers that arrive together are applied together, and the views hear of them once.
        cx.spawn(async move |this, cx| {
            while let Ok(first) = receiver.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = receiver.try_recv() {
                    batch.push(more);
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let applied = this.update(cx, |this, cx| {
                    let mut any = false;
                    for answer in batch {
                        any |= this.cache.take(&answer.path, answer.part);
                    }
                    if any {
                        cx.notify();
                    }
                });
                if applied.is_err() {
                    return;
                }
                cx.background_executor().timer(FRAME).await;
            }
            done.store(true, Ordering::SeqCst);
        })
        .detach();
    }

    /// The selection of the active tab changed: look at it once it has stayed the same for a moment.
    fn on_explorer(&mut self, event: &ExplorerEvent, cx: &mut Context<Self>) {
        let ExplorerEvent::SelectionChanged(tab) = event else {
            return;
        };
        let Some(explorer) = explorer::try_explorer(cx) else {
            return;
        };
        if explorer.read(cx).active_tab_id() != *tab {
            return;
        }
        self.settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SELECTION_SETTLES).await;
            this.update(cx, |this, cx| this.look_at_selection(cx)).ok();
        }));
    }

    /// Looks ahead at what the active tab has selected: from one item to [`MOST_TO_LOOK_AT`] of them.
    fn look_at_selection(&mut self, cx: &mut Context<Self>) {
        let Some(explorer) = explorer::try_explorer(cx) else {
            return;
        };
        let entries: Vec<FileEntry> = {
            let explorer = explorer.read(cx);
            let Some(tab) = explorer.active_tab() else {
                return;
            };
            let chosen = tab.selection.len();
            if chosen == 0 || chosen > MOST_TO_LOOK_AT {
                return;
            }
            tab.rows.chosen(&tab.selection).into_iter().cloned().collect()
        };
        self.look_ahead(&entries, cx);
    }

    /// The folder watcher reported a change.
    fn on_core(&mut self, event: &CoreEvent, cx: &mut Context<Self>) {
        let CoreEvent::DirChanged(folder) = event else {
            return;
        };
        self.pulses.beat(folder, Instant::now());
        let shown = explorer::try_explorer(cx)
            .and_then(|explorer| explorer.read(cx).active_tab().map(|tab| pulse::key(&tab.path)));
        if shown.is_some_and(|shown| shown == pulse::key(folder)) {
            cx.notify();
            // "Changing now" has to go away by itself when the changes stop.
            self.changing_ends = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(CHANGING_ENDS).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            }));
        }
    }
}

/// What is known at once about `targets` (no I/O): what they are, and what is on the clipboard.
pub fn facts(cx: &App, targets: &[FileEntry]) -> Facts {
    let files = targets.iter().filter(|entry| !entry.dir).count();
    Facts {
        shape: Shape::of(targets),
        count: targets.len(),
        files,
        folders: targets.len() - files,
        bytes: targets
            .iter()
            .filter(|entry| !entry.dir)
            .fold(0u64, |sum, entry| sum.saturating_add(entry.size)),
        ext: shared_ext(targets),
        first_name: targets.first().map(|entry| entry.lower.clone()).unwrap_or_default(),
        fixed: targets
            .iter()
            .any(|entry| is_archived(entry) || is_remote(entry) || is_closed(entry) || is_recycled(entry)),
        clip: try_shared(cx).map(|aware| aware.read(cx).clip().clone()).unwrap_or_default(),
    }
}

/// The rows to put forward for `facts`, best first, at most `most`: `rank::suggest` with the saved habits and
/// today's date. Empty when the person turned suggestions off.
pub fn suggest(cx: &App, facts: &Facts, probed: Option<&Probed>, available: &[&str], most: usize) -> Vec<Pick> {
    let settings = state::settings(cx);
    if !settings.suggest {
        return Vec::new();
    }
    let none = Habits::default();
    let habits = if settings.learn {
        persistence::app(cx).map_or(&none, |app| &app.habits)
    } else {
        &none
    };
    rank::suggest(facts, probed, available, habits, today(cx), most)
}

/// Counts `key`, chosen for something of `shape`. Nothing when the person turned learning off.
pub fn note_action(cx: &mut App, shape: Shape, key: &str) {
    if !state::settings(cx).learn || key.is_empty() || rank::is_basic(key) {
        return;
    }
    let day = today(cx);
    persistence::update_app(cx, |app| app.habits.note_action(shape, key, day));
}

/// Counts `folder` as where files with these extensions went. Nothing when the person turned learning off.
pub fn note_place(cx: &mut App, exts: &[String], folder: &str) {
    if !state::settings(cx).learn {
        return;
    }
    let day = today(cx);
    persistence::update_app(cx, |app| {
        if exts.is_empty() {
            app.habits.note_place("", folder, day);
        }
        for ext in exts.iter().take(8) {
            app.habits.note_place(ext, folder, day);
        }
    });
}

/// The folders files with extension `ext` have gone to, heaviest first. Empty when learning or suggestions are off.
pub fn places_for(cx: &App, ext: &str) -> Vec<Place> {
    let settings = state::settings(cx);
    if !settings.learn || !settings.suggest {
        return Vec::new();
    }
    persistence::app(cx).map_or_else(Vec::new, |app| app.habits.places_for(ext, today(cx)))
}

/// Forgets what was learned.
pub fn forget_habits(cx: &mut App) {
    persistence::update_app(cx, |app| app.habits.forget());
}

/// The pointer came to rest on `entry` (a row of the list, a tile of the grid): once it has stayed
/// [`RESTS_FOR`], what isn't known about the item is found out, quietly, so its menu opens knowing it. The view
/// keeps the task while the pointer stays and drops it when it leaves, which asks nothing. None when the app has
/// no entity.
pub fn rest_on(entry: FileEntry, cx: &mut App) -> Option<Task<()>> {
    let aware = try_shared(cx)?;
    Some(cx.spawn(async move |cx| {
        cx.background_executor().timer(RESTS_FOR).await;
        aware.update(cx, |aware, cx| aware.look_ahead(std::slice::from_ref(&entry), cx));
    }))
}
