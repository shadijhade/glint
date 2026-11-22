//! Live Folder Time-Machine Scrubber: unifies Windows shadow copies (VSS), Git commits, restic backups,
//! and NTFS USN journal activity into a single chronological scrubber inside any pane.
//!
//! State is per tab in [`TimeMachineModel`] (mirroring `disk_usage`), initialized lazily and doing zero
//! background work while hidden. Closing the view clears cached moments and diff entries immediately.
//!
//! **Moments stream.** The sources are asked at the same time and each one's stops arrive as soon as it has them
//! (Git is usually first: a folder in a repository shows its commits in a few milliseconds while the shadow
//! copies and backups are still being asked). The rail grows as they come; the merged order depends only on the
//! stops, so it never reshuffles, and the stop the person is on stays selected.
//!
//! **Scrubbing is cached.** Each tab keeps the last stops it read ([`cache::ScrubCache`]) and reads the stops
//! either side of the one shown ahead of time, so stepping with the arrows shows a kept result in the same frame.
//!
//! **The rail** ([`rail`]) is the timeline: ticks colored by source at their time, a sparkline of how crowded the
//! history is above it, and a playhead that snaps to stops and ticks a haptic detent at each one it crosses.

mod cache;
mod look;
mod rail;
#[cfg(test)]
mod tests;

use crate::components::panel_header::{panel_header, tool_button, Header};
use crate::core_bridge;
use crate::explorer::{explorer, ExplorerEvent, TabId};
use crate::kit::prelude::*;
use crate::platform::haptics::{self, HapticCue};
use crate::{i18n, state, t, theme};
use cache::{Key, ScrubCache};
use glint_core::host::Manager as _;
use glint_model::format::{format_count, format_date, format_size};
use glint_model::paths::{is_folder_path, join_path, same_path};
use glint_model::time_machine::{
    merge_moments, summarize_entries, EntryDelta, Moment, MomentSource, RestoreSource, TimeEntry,
};
use gpui_kit::{
    canvas, relative, uniform_list, App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle,
    Focusable, Global, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Render, Subscription,
    UniformListScrollHandle, Window,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// Boxed future used by [`Source`].
pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;
/// Receives a batch of stops as soon as one source has them (any thread).
pub type MomentsSink = Arc<dyn Fn(Vec<Moment>) + Send + Sync>;
/// Asks every source for the stops of a folder, giving each batch to the sink; done when all have answered.
pub type MomentsFn = Arc<dyn Fn(String, MomentsSink) -> Fut<Result<(), String>> + Send + Sync>;
pub type InspectFn = Arc<dyn Fn(String, Moment) -> Fut<Result<Vec<TimeEntry>, String>> + Send + Sync>;
pub type RestoreFn = Arc<dyn Fn(String, TimeEntry, bool) -> Fut<Result<String, String>> + Send + Sync>;
pub type StageFn = Arc<dyn Fn(TimeEntry) -> Fut<Result<String, String>> + Send + Sync>;

/// How long a drag of the playhead waits, after the last stop it crossed, before reading that stop. Crossing
/// ten stops in a sweep reads the one the sweep ends on, not ten.
const DRAG_SETTLE: Duration = Duration::from_millis(90);

/// Backend data source for [`TimeMachineModel`]: `glint-core` in production, a fake in tests.
#[derive(Clone)]
pub struct Source {
    /// Streams the historical moments available for a folder, batch by batch.
    pub moments: MomentsFn,
    /// Diffs `folder` at `moment` against today's live folder.
    pub inspect: InspectFn,
    /// Restores a single entry into `folder`.
    pub restore: RestoreFn,
    /// Stages a historical file version to a temp file for diffing against today.
    pub stage: StageFn,
}

impl Source {
    /// Real implementation backed by `glint-core`.
    pub fn core(cx: &App) -> Self {
        let win = cx.try_global::<core_bridge::Core>().and_then(|core| {
            core.app
                .window(crate::workspace::MAIN)
                .or_else(|| core.app.windows().into_values().next())
        });
        Source {
            moments: Arc::new(|folder, sink| {
                Box::pin(async move {
                    core_bridge::run(glint_core::time_machine::stream_moments(folder, sink))
                        .await
                        .and_then(|r| r)
                })
            }),
            inspect: Arc::new(|folder, moment| {
                Box::pin(async move {
                    core_bridge::run(glint_core::time_machine::inspect_moment(folder, moment))
                        .await
                        .and_then(|r| r)
                })
            }),
            restore: Arc::new(move |folder, item, replace| {
                let win = win.clone();
                Box::pin(async move {
                    let window = win.ok_or_else(|| "Main window handle unavailable".to_string())?;
                    core_bridge::run(glint_core::time_machine::restore_item(window, folder, item, replace))
                        .await
                        .and_then(|r| r)
                })
            }),
            stage: Arc::new(|item| {
                Box::pin(async move {
                    core_bridge::run(glint_core::time_machine::stage_item(item))
                        .await
                        .and_then(|r| r)
                })
            }),
        }
    }
}

/// Which entries the scrubber table displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    All,
    ChangesOnly,
    Deleted,
}

#[derive(Clone, Debug)]
struct TabState {
    folder: String,
    /// Some source has not answered yet.
    moments_loading: bool,
    entries_loading: bool,
    error: Option<String>,
    /// Chronological moments ordered oldest -> newest so scrubbing right advances toward today.
    moments: Arc<Vec<Moment>>,
    /// Index into `moments` of the stop shown. Meaningful once `started`.
    selected: usize,
    /// A stop has been selected (the first one to arrive is, unless the filter hides it).
    started: bool,
    /// The stop's read is waiting out [`DRAG_SETTLE`].
    deferred: bool,
    entries: Arc<Vec<TimeEntry>>,
    /// The row the keyboard is on (index into `entries`), for Space.
    row: Option<usize>,
    filter: FilterKind,
    /// Only the stops of this source are on the rail (and stepped through), when set.
    source_filter: Option<MomentSource>,
    moments_serial: u64,
    inspect_serial: u64,
}

impl TabState {
    fn new(folder: String) -> Self {
        Self {
            folder,
            moments_loading: true,
            entries_loading: false,
            error: None,
            moments: Arc::default(),
            selected: 0,
            started: false,
            deferred: false,
            entries: Arc::default(),
            row: None,
            filter: FilterKind::All,
            source_filter: None,
            moments_serial: 0,
            inspect_serial: 0,
        }
    }

    fn selected_moment(&self) -> Option<&Moment> {
        self.started.then(|| self.moments.get(self.selected)).flatten()
    }

    /// Indexes into `moments` of the stops the rail shows, oldest first.
    fn visible(&self) -> Vec<usize> {
        self.moments
            .iter()
            .enumerate()
            .filter(|(_, m)| self.source_filter.is_none_or(|s| m.source == s))
            .map(|(i, _)| i)
            .collect()
    }

    /// The sources that have stops, in the chips' order.
    fn sources_present(&self) -> Vec<MomentSource> {
        rail::SOURCES
            .into_iter()
            .filter(|s| self.moments.iter().any(|m| m.source == *s))
            .collect()
    }

    fn filtered_entries(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| match self.filter {
                FilterKind::All => Some(i),
                FilterKind::ChangesOnly => (!matches!(e.delta, EntryDelta::Unchanged)).then_some(i),
                FilterKind::Deleted => matches!(e.delta, EntryDelta::Deleted).then_some(i),
            })
            .collect()
    }
}

/// What the streaming read of a folder's moments hands the model.
enum Feed {
    Batch(Vec<Moment>),
    Done(Result<(), String>),
}

/// Shared per-tab state for the Time-Machine Scrubber.
pub struct TimeMachineModel {
    source: Source,
    tabs: HashMap<TabId, TabState>,
    /// What each tab read lately, and what is being read ahead.
    caches: HashMap<TabId, ScrubCache>,
    shown: HashSet<TabId>,
    serial: u64,
    _sub: Subscription,
}

impl TimeMachineModel {
    fn new(source: Source, cx: &mut Context<Self>) -> Self {
        let exp = explorer(cx);
        let sub = cx.subscribe(&exp, |this, exp, ev: &ExplorerEvent, cx| match ev {
            ExplorerEvent::TabClosed { tab, .. } => {
                let removed = this.shown.remove(tab);
                let dropped = this.tabs.remove(tab).is_some();
                this.caches.remove(tab);
                if removed || dropped {
                    cx.notify();
                }
            }
            ExplorerEvent::TabChanged(tab) if this.shown.contains(tab) => {
                let Some(path) = exp.read(cx).tab(*tab).map(|t| t.path.clone()) else {
                    return;
                };
                let changed = this.tabs.get(tab).is_none_or(|state| !same_path(&state.folder, &path));
                if changed {
                    if is_folder_path(&path) {
                        this.load_folder(*tab, path, cx);
                    } else {
                        this.shown.remove(tab);
                        this.tabs.remove(tab);
                        this.caches.remove(tab);
                        cx.notify();
                    }
                }
            }
            _ => {}
        });
        Self {
            source,
            tabs: HashMap::new(),
            caches: HashMap::new(),
            shown: HashSet::new(),
            serial: 0,
            _sub: sub,
        }
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    /// Whether `tab` has Time Machine turned on.
    pub fn shows(&self, tab: TabId) -> bool {
        self.shown.contains(&tab)
    }

    /// Turns Time Machine on or off for `tab` in `folder`. Turning off drops cached moments and entries.
    pub fn set(&mut self, tab: TabId, on: bool, folder: Option<String>, cx: &mut Context<Self>) {
        if !on {
            let removed = self.shown.remove(&tab);
            let dropped = self.tabs.remove(&tab).is_some();
            self.caches.remove(&tab);
            if removed || dropped {
                cx.notify();
            }
            return;
        }
        let Some(folder) = folder.filter(|p| is_folder_path(p)) else {
            return;
        };
        self.shown.insert(tab);
        self.load_folder(tab, folder, cx);
    }

    /// Toggles Time Machine for `tab`.
    pub fn toggle(&mut self, tab: TabId, folder: Option<String>, cx: &mut Context<Self>) {
        let on = !self.shows(tab);
        self.set(tab, on, folder, cx);
    }

    fn load_folder(&mut self, tab: TabId, folder: String, cx: &mut Context<Self>) {
        let serial = self.next_serial();
        let mut state = TabState::new(folder.clone());
        // The filters stay with the tab while it lives, whatever folder it goes to.
        if let Some(old) = self.tabs.get(&tab) {
            state.filter = old.filter;
            state.source_filter = old.source_filter;
        }
        state.moments_serial = serial;
        self.tabs.insert(tab, state);
        self.caches.insert(tab, ScrubCache::default());
        cx.notify();

        // Every source is asked at once; what each finds comes through the channel, in whatever order.
        let (tx, rx) = async_channel::unbounded::<Feed>();
        let sink: MomentsSink = {
            let tx = tx.clone();
            Arc::new(move |batch| {
                let _ = tx.try_send(Feed::Batch(batch));
            })
        };
        let fetch = (self.source.moments)(folder, sink);
        cx.background_spawn(async move {
            let res = fetch.await;
            let _ = tx.try_send(Feed::Done(res));
        })
        .detach();
        cx.spawn(async move |this, cx| {
            while let Ok(feed) = rx.recv().await {
                let more = this
                    .update(cx, |this, cx| this.on_feed(tab, serial, feed, cx))
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
        })
        .detach();
    }

    /// One thing the sources found. Returns whether the read is still wanted.
    fn on_feed(&mut self, tab: TabId, serial: u64, feed: Feed, cx: &mut Context<Self>) -> bool {
        if !self.shown.contains(&tab) || self.tabs.get(&tab).is_none_or(|s| s.moments_serial != serial) {
            return false;
        }
        match feed {
            Feed::Batch(batch) => self.add_moments(tab, batch, cx),
            Feed::Done(res) => {
                let mut start = None;
                if let Some(state) = self.tabs.get_mut(&tab) {
                    state.moments_loading = false;
                    if state.moments.is_empty() {
                        state.entries = Arc::default();
                        if let Err(err) = res {
                            state.error = Some(err);
                        }
                    } else if !state.started {
                        // The source the filter asked for has no stops here: show them all instead.
                        state.source_filter = None;
                        start = state.moments.len().checked_sub(1);
                    }
                }
                if let Some(last) = start {
                    self.select_with(tab, last, None, cx);
                }
                cx.notify();
            }
        }
        true
    }

    /// Merges a source's stops into the tab's. The stop on screen stays selected; the first stop to arrive
    /// (the newest one the filter lets through) is selected and read.
    fn add_moments(&mut self, tab: TabId, batch: Vec<Moment>, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get_mut(&tab) else {
            return;
        };
        let kept = state.selected_moment().map(|m| (m.source, m.id.clone()));
        let mut all: Vec<Moment> = state.moments.iter().cloned().collect();
        all.extend(batch);
        let mut merged = merge_moments(all);
        // Newest-first -> oldest-first so scrubbing forward moves right toward Today.
        merged.reverse();
        state.moments = Arc::new(merged);
        if let Some((source, id)) = kept {
            if let Some(at) = state.moments.iter().position(|m| m.source == source && m.id == id) {
                state.selected = at;
            }
        }
        let first = if state.started {
            None
        } else {
            state.visible().last().copied()
        };
        state.error = None;
        cx.notify();
        if let Some(last) = first {
            self.select_with(tab, last, None, cx);
        }
    }

    fn select_moment(&mut self, tab: TabId, index: usize, cx: &mut Context<Self>) {
        self.select_with(tab, index, None, cx);
    }

    /// Selects stop `index` and reads it, after `delay` when there is one (a drag that is still going).
    fn select_with(&mut self, tab: TabId, index: usize, delay: Option<Duration>, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get_mut(&tab) else {
            return;
        };
        let Some(moment) = state.moments.get(index).cloned() else {
            return;
        };
        let reading = state.entries_loading && !(state.deferred && delay.is_none());
        if state.started && state.selected == index && (reading || !state.entries.is_empty()) {
            return;
        }
        state.started = true;
        state.selected = index;
        let folder = state.folder.clone();
        self.load_entries(tab, folder, moment, delay, cx);
    }

    /// Steps to the stop `delta` places along the rail's visible stops, never past either end.
    fn step_moment(&mut self, tab: TabId, delta: isize, delay: Option<Duration>, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.tabs.get(&tab) else {
            return false;
        };
        let visible = state.visible();
        let Some(at) = visible.iter().position(|&i| i == state.selected) else {
            return false;
        };
        let next = (at as isize + delta).clamp(0, visible.len() as isize - 1) as usize;
        if next == at {
            return false;
        }
        self.select_with(tab, visible[next], delay, cx);
        true
    }

    /// Jumps to the oldest (or newest) visible stop.
    fn jump_moment(&mut self, tab: TabId, oldest: bool, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.tabs.get(&tab) else {
            return false;
        };
        let visible = state.visible();
        let target = if oldest { visible.first() } else { visible.last() };
        match target.copied() {
            Some(index) if index != state.selected => {
                self.select_with(tab, index, None, cx);
                true
            }
            _ => false,
        }
    }

    /// A drag has come to rest: read the stop it ended on now instead of waiting out the settle time.
    fn settle(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get(&tab) else {
            return;
        };
        if !state.deferred {
            // The stop it rests on was kept: only the reading ahead was waiting.
            self.prefetch(tab, cx);
            return;
        }
        let (index, folder) = (state.selected, state.folder.clone());
        if let Some(moment) = state.moments.get(index).cloned() {
            self.load_entries(tab, folder, moment, None, cx);
        }
    }

    fn set_filter(&mut self, tab: TabId, filter: FilterKind, cx: &mut Context<Self>) {
        if let Some(state) = self.tabs.get_mut(&tab) {
            if state.filter != filter {
                state.filter = filter;
                state.row = None;
                cx.notify();
            }
        }
    }

    /// Shows only `source`'s stops (or all of them). When the stop on screen isn't one of them, the nearest
    /// one in time is selected.
    fn set_source_filter(&mut self, tab: TabId, source: Option<MomentSource>, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get_mut(&tab) else {
            return;
        };
        if state.source_filter == source {
            return;
        }
        state.source_filter = source;
        let visible = state.visible();
        let now_at = state.selected_moment().map_or(0, |m| m.timestamp_ms);
        let hidden = !visible.contains(&state.selected);
        let nearest = visible
            .iter()
            .copied()
            .min_by_key(|&i| (state.moments[i].timestamp_ms - now_at).abs());
        cx.notify();
        if let (true, Some(near)) = (hidden, nearest) {
            self.select_with(tab, near, None, cx);
        }
    }

    /// Moves the keyboard's row `delta` places among the rows the filter shows.
    fn move_row(&mut self, tab: TabId, delta: isize, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get_mut(&tab) else {
            return;
        };
        let rows = state.filtered_entries();
        if rows.is_empty() {
            return;
        }
        let now = state.row.and_then(|r| rows.iter().position(|&i| i == r));
        let next = match now {
            Some(at) => (at as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
            None if delta >= 0 => 0,
            None => rows.len() - 1,
        };
        if state.row != Some(rows[next]) {
            state.row = Some(rows[next]);
            cx.notify();
        }
    }

    fn pick_row(&mut self, tab: TabId, row: usize, cx: &mut Context<Self>) {
        if let Some(state) = self.tabs.get_mut(&tab) {
            if state.row != Some(row) {
                state.row = Some(row);
                cx.notify();
            }
        }
    }

    fn load_entries(
        &mut self,
        tab: TabId,
        folder: String,
        moment: Moment,
        delay: Option<Duration>,
        cx: &mut Context<Self>,
    ) {
        let serial = self.next_serial();
        let key = Key::new(&folder, &moment);
        let kept = self.caches.get_mut(&tab).and_then(|c| c.get(&key));
        let Some(state) = self.tabs.get_mut(&tab) else {
            return;
        };
        state.inspect_serial = serial;
        state.row = None;
        state.error = None;
        if let Some(hit) = kept {
            // Read lately: it is on screen in this frame.
            state.entries = hit;
            state.entries_loading = false;
            state.deferred = false;
            cx.notify();
            if delay.is_none() {
                self.prefetch(tab, cx);
            }
            return;
        }
        state.entries_loading = true;
        state.deferred = delay.is_some();
        cx.notify();

        let inspect = self.source.inspect.clone();
        cx.spawn(async move |this, cx| {
            if let Some(wait) = delay {
                cx.background_executor().timer(wait).await;
                let wanted = this
                    .update(cx, |this, _| {
                        this.tabs.get(&tab).is_some_and(|s| s.inspect_serial == serial)
                    })
                    .unwrap_or(false);
                if !wanted {
                    return;
                }
            }
            let res = inspect(folder, moment).await;
            this.update(cx, |this, cx| {
                if !this.shown.contains(&tab) {
                    return;
                }
                let Some(state) = this.tabs.get_mut(&tab) else {
                    return;
                };
                if state.inspect_serial != serial {
                    // Not the stop on screen any more, but worth keeping for when the person comes back to it.
                    if let (Ok(entries), Some(cache)) = (res, this.caches.get_mut(&tab)) {
                        cache.insert(key, Arc::new(entries));
                    }
                    return;
                }
                state.entries_loading = false;
                state.deferred = false;
                match res {
                    Ok(entries) => {
                        let entries = Arc::new(entries);
                        state.entries = entries.clone();
                        state.error = None;
                        if let Some(cache) = this.caches.get_mut(&tab) {
                            cache.insert(key, entries);
                        }
                    }
                    Err(err) => {
                        state.entries = Arc::default();
                        state.error = Some(err);
                    }
                }
                cx.notify();
                this.prefetch(tab, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reads the stops either side of the one on screen, so the next arrow key finds them kept. Backups are
    /// left out: reading one starts a restic process, too much to spend on a stop nobody may step to.
    fn prefetch(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get(&tab) else {
            return;
        };
        if state.deferred {
            return;
        }
        let visible = state.visible();
        let Some(at) = visible.iter().position(|&i| i == state.selected) else {
            return;
        };
        let folder = state.folder.clone();
        let next = [at.checked_sub(1), Some(at + 1)]
            .into_iter()
            .flatten()
            .filter_map(|v| visible.get(v))
            .map(|&i| state.moments[i].clone())
            .filter(|m| m.source != MomentSource::ResticBackup)
            .collect::<Vec<_>>();
        for moment in next {
            let key = Key::new(&folder, &moment);
            let Some(cache) = self.caches.get_mut(&tab) else {
                return;
            };
            if !cache.begin_read(&key) {
                continue;
            }
            let inspect = self.source.inspect.clone();
            let folder = folder.clone();
            cx.spawn(async move |this, cx| {
                let res = inspect(folder.clone(), moment).await;
                this.update(cx, |this, _| {
                    let same_folder = this.tabs.get(&tab).is_some_and(|s| same_path(&s.folder, &folder));
                    if let Some(cache) = this.caches.get_mut(&tab) {
                        cache.end_read(&key);
                        if let (true, Ok(entries)) = (same_folder, res) {
                            cache.insert(key, Arc::new(entries));
                        }
                    }
                })
                .ok();
            })
            .detach();
        }
    }

    fn source(&self) -> Source {
        self.source.clone()
    }
}

struct State(Entity<TimeMachineModel>);
impl Global for State {}

/// Initializes the shared [`TimeMachineModel`] using `glint-core`.
pub fn init(cx: &mut App) {
    let source = Source::core(cx);
    init_with(cx, source);
}

/// Initializes the shared [`TimeMachineModel`] with an explicit [`Source`] (for tests).
pub fn init_with(cx: &mut App, source: Source) {
    let model = cx.new(|cx| TimeMachineModel::new(source, cx));
    cx.set_global(State(model));
}

/// Returns the shared [`TimeMachineModel`], initializing it on demand.
pub fn shared(cx: &mut App) -> Entity<TimeMachineModel> {
    if !cx.has_global::<State>() {
        init(cx);
    }
    cx.global::<State>().0.clone()
}

/// Returns the shared [`TimeMachineModel`] if already initialized.
pub fn try_shared(cx: &App) -> Option<Entity<TimeMachineModel>> {
    cx.try_global::<State>().map(|s| s.0.clone())
}

/// Whether `tab` has Time Machine turned on.
pub fn shows(cx: &App, tab: TabId) -> bool {
    try_shared(cx).is_some_and(|m| m.read(cx).shows(tab))
}

/// Whether `pane`'s tab should draw Time Machine instead of files (`shows(tab)` and showing a real folder).
pub fn visible(cx: &App, tab: TabId) -> bool {
    shows(cx, tab)
        && explorer(cx)
            .read(cx)
            .tab(tab)
            .is_some_and(|t| t.find.is_none() && is_folder_path(&t.path))
}

/// Shows or hides Time Machine for `tab`.
pub fn set(cx: &mut App, tab: TabId, on: bool) {
    let folder = explorer(cx).read(cx).tab(tab).map(|t| t.path.clone());
    let model = shared(cx);
    model.update(cx, |m, cx| m.set(tab, on, folder, cx));
}

/// Toggles Time Machine for `tab`.
pub fn toggle(cx: &mut App, tab: TabId) {
    let folder = explorer(cx).read(cx).tab(tab).map(|t| t.path.clone());
    let model = shared(cx);
    model.update(cx, |m, cx| m.toggle(tab, folder, cx));
}

/// Actions the Time-Machine Scrubber asks the workspace to complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimeMachineEvent {
    /// An item was restored into `folder`.
    Restored { folder: String, path: String },
    /// A historical file version was staged for side-by-side comparison with `current`.
    Compare { current: String, staged: String },
    /// Mount the selected Git commit or backup snapshot as a live CfAPI portal folder.
    PinPortal {
        name: String,
        source: glint_core::portals::PortalSource,
    },
    /// An operation failed.
    Error { title: SharedString, message: String },
}

/// Where the rail was drawn and what it drew, for turning a pointer position into a stop. Written while the
/// view renders (the bounds from a canvas), read by the pointer handlers.
#[derive(Default)]
struct RailGeom {
    bounds: Bounds<Pixels>,
    /// Along the rail, 0..1 from its start edge, for each visible stop.
    positions: Vec<f32>,
    /// Index into the tab's moments of each visible stop.
    visible: Vec<usize>,
}

/// The per-pane Time-Machine Scrubber view.
pub struct TimeMachineView {
    pane: usize,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
    geom: Rc<RefCell<RailGeom>>,
    /// The playhead is being dragged.
    dragging: Rc<Cell<bool>>,
    /// How many haptic detents were asked for (tests can't feel them).
    #[cfg(test)]
    detents: usize,
    _sub: Subscription,
}

impl EventEmitter<TimeMachineEvent> for TimeMachineView {}

impl Focusable for TimeMachineView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TimeMachineView {
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = shared(cx);
        let sub = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            pane,
            focus_handle: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            geom: Rc::default(),
            dragging: Rc::default(),
            #[cfg(test)]
            detents: 0,
            _sub: sub,
        }
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    fn tab_id(&self, cx: &App) -> Option<TabId> {
        explorer(cx).read(cx).pane_tab(self.pane).map(|t| t.id)
    }

    fn close(&self, cx: &mut App) {
        if let Some(tab) = self.tab_id(cx) {
            set(cx, tab, false);
        }
    }

    /// One tick of the touchpad's haptic motor: the playhead crossed a stop.
    fn detent(&mut self, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.detents += 1;
        }
        haptics::trigger(HapticCue::ZoomDetent, state::try_settings(cx).is_none_or(|s| s.haptics));
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_id(cx) else {
            return;
        };
        if shared(cx).update(cx, |m, cx| m.step_moment(tab, delta, None, cx)) {
            self.detent(cx);
        }
    }

    fn jump(&mut self, oldest: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_id(cx) else {
            return;
        };
        if shared(cx).update(cx, |m, cx| m.jump_moment(tab, oldest, cx)) {
            self.detent(cx);
        }
    }

    /// The pointer is on the rail at `x` (a press, or a move with the button down): the playhead goes to the
    /// stop nearest to it. While dragging, the stop is read once the pointer has rested a moment.
    fn playhead_to(&mut self, x: Pixels, dragging: bool, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_id(cx) else {
            return;
        };
        let target = {
            let geom = self.geom.borrow();
            let width = f32::from(geom.bounds.size.width);
            if width <= 1.0 {
                return;
            }
            let frac = (f32::from(x - geom.bounds.origin.x) / width).clamp(0.0, 1.0);
            let frac = if i18n::is_rtl() { 1.0 - frac } else { frac };
            rail::nearest(&geom.positions, frac).and_then(|v| geom.visible.get(v).copied())
        };
        let Some(target) = target else {
            return;
        };
        let model = shared(cx);
        if model
            .read(cx)
            .tabs
            .get(&tab)
            .is_none_or(|s| s.selected == target && s.started)
        {
            return;
        }
        let delay = dragging.then_some(DRAG_SETTLE);
        model.update(cx, |m, cx| m.select_with(tab, target, delay, cx));
        self.detent(cx);
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if !self.dragging.replace(false) {
            return;
        }
        if let Some(tab) = self.tab_id(cx) {
            shared(cx).update(cx, |m, cx| m.settle(tab, cx));
        }
    }

    fn move_row(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tab_id(cx) {
            shared(cx).update(cx, |m, cx| m.move_row(tab, delta, cx));
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let rtl = i18n::is_rtl();
        match key {
            "escape" => {
                cx.stop_propagation();
                self.close(cx);
            }
            "left" => {
                cx.stop_propagation();
                self.step(if rtl { 1 } else { -1 }, cx);
            }
            "right" => {
                cx.stop_propagation();
                self.step(if rtl { -1 } else { 1 }, cx);
            }
            "home" => {
                cx.stop_propagation();
                self.jump(!rtl, cx);
            }
            "end" => {
                cx.stop_propagation();
                self.jump(rtl, cx);
            }
            "up" => {
                cx.stop_propagation();
                self.move_row(-1, cx);
            }
            "down" => {
                cx.stop_propagation();
                self.move_row(1, cx);
            }
            "space" => {
                cx.stop_propagation();
                self.quick_look_row(window, cx);
            }
            _ => {}
        }
    }

    fn restore_entry(&self, folder: String, entry: TimeEntry, cx: &mut Context<Self>) {
        let source = shared(cx).read(cx).source();
        cx.spawn(async move |this, cx| {
            let res = (source.restore)(folder.clone(), entry, false).await;
            this.update(cx, |_, cx| match res {
                Ok(path) => cx.emit(TimeMachineEvent::Restored { folder, path }),
                Err(message) => cx.emit(TimeMachineEvent::Error {
                    title: t!("Couldn't restore"),
                    message,
                }),
            })
            .ok();
        })
        .detach();
    }

    fn compare_entry(&self, folder: String, entry: TimeEntry, cx: &mut Context<Self>) {
        let current = join_path(&folder, &entry.name);
        let source = shared(cx).read(cx).source();
        cx.spawn(async move |this, cx| {
            let res = (source.stage)(entry).await;
            this.update(cx, |_, cx| match res {
                Ok(staged) => cx.emit(TimeMachineEvent::Compare { current, staged }),
                Err(message) => cx.emit(TimeMachineEvent::Error {
                    title: t!("Couldn't open historical version"),
                    message,
                }),
            })
            .ok();
        })
        .detach();
    }

    /// Space: stages the row's historical version read-only in the background and shows it in Quick Look,
    /// over Time Machine. Escape closes it and the keyboard is back on the row.
    fn quick_look_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tab_id(cx) else {
            return;
        };
        let model = shared(cx);
        let Some(entry) = model
            .read(cx)
            .tabs
            .get(&tab)
            .and_then(|s| s.row.and_then(|r| s.entries.get(r).cloned()))
        else {
            return;
        };
        if !look::can_look(&entry) {
            return;
        }
        let source = model.read(cx).source();
        cx.spawn_in(window, async move |this, cx| {
            let res = (source.stage)(entry.clone()).await;
            this.update_in(cx, |_, window, cx| match res {
                Ok(staged) => look::open(&entry, staged, window, cx),
                Err(message) => cx.emit(TimeMachineEvent::Error {
                    title: t!("Couldn't open historical version"),
                    message,
                }),
            })
            .ok();
        })
        .detach();
    }
}

fn source_label(source: MomentSource) -> SharedString {
    match source {
        MomentSource::VssShadow => t!("Windows Restore"),
        MomentSource::GitCommit => t!("Git"),
        MomentSource::ResticBackup => t!("Backup"),
        MomentSource::UsnActivity => t!("Journal"),
    }
}

/// The source's chip label (plural for backups, as the chips read "All · Git · Windows Restore · Backups · Journal").
fn chip_label(source: MomentSource) -> SharedString {
    match source {
        MomentSource::ResticBackup => t!("Backups"),
        other => source_label(other),
    }
}

fn delta_badge(delta: &EntryDelta, t: &theme::Tokens) -> Option<(SharedString, Hsla)> {
    match delta {
        EntryDelta::Deleted => Some((t!("Deleted since"), t.destructive)),
        EntryDelta::Modified { .. } => Some((t!("Modified"), t.warning)),
        EntryDelta::AddedSince => Some((t!("Newer"), t.tk_function)),
        EntryDelta::Unchanged => None,
    }
}

/// A small toggle button: the source chips and the entries filter.
fn pill(
    id: impl Into<ElementId>,
    label: SharedString,
    on: bool,
    dot: Option<Hsla>,
    t: &theme::Tokens,
    cx: &App,
) -> Button {
    let tone = dot.unwrap_or(t.primary);
    Button::new(id)
        .compact()
        .custom(
            ButtonCustomVariant::new(cx)
                .color(if on { tone.opacity(0.2) } else { t.accent.opacity(0.4) })
                .foreground(if on { t.foreground } else { t.muted_foreground })
                .hover(t.pressed)
                .active(t.pressed),
        )
        .border_1()
        .border_color(if on { tone } else { t.stroke })
        .accessibility_label(label)
}

impl Render for TimeMachineView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(tab_id) = self.tab_id(cx) else {
            return div().into_any_element();
        };
        if !visible(cx, tab_id) {
            return div().into_any_element();
        }
        let model = shared(cx);
        let state = model.read(cx).tabs.get(&tab_id).cloned();
        let Some(state) = state else {
            return div().into_any_element();
        };

        let t = theme::tokens(cx).clone();
        let lang = i18n::language();
        let rtl = i18n::is_rtl();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let selected = state.selected_moment().cloned();
        let (deleted_n, modified_n, unchanged_n) = summarize_entries(&state.entries);
        let filtered_idx = state.filtered_entries();

        let none = state.moments.is_empty();
        let header = panel_header(
            Header {
                close_id: "tm-close".into(),
                glyph: IconName::RotateCcwClock,
                title: t!("Time Machine"),
                subtitle: Some(state.folder.clone().into()),
                tools: vec![
                    tool_button(
                        "tm-prev",
                        IconName::ArrowLeft,
                        t!("Older snapshot"),
                        t!("Older snapshot (Left arrow)"),
                        cx.listener(|this, _, _, cx| this.step(-1, cx)),
                    )
                    .disabled(none)
                    .into_any_element(),
                    tool_button(
                        "tm-next",
                        IconName::ArrowRight,
                        t!("Newer snapshot"),
                        t!("Newer snapshot (Right arrow)"),
                        cx.listener(|this, _, _, cx| this.step(1, cx)),
                    )
                    .disabled(none)
                    .into_any_element(),
                ],
                close_name: None,
            },
            cx.listener(|this, _, _, cx| this.close(cx)),
            &t,
        );

        // The timeline: source chips, the sparkline, the rail with its playhead, and the stop's line.
        let scrubber = if state.moments.is_empty() && state.moments_loading {
            row_start()
                .w_full()
                .px(space(4.))
                .py(space(3.))
                .items_center()
                .gap(space(2.))
                .border_b_1()
                .border_color(t.stroke)
                .child(Spinner::new().color(t.primary))
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .child(t!("Scanning snapshots, Git commits, and backups…")),
                )
                .into_any_element()
        } else if state.moments.is_empty() {
            // Nothing found: the body says so once, in its own words.
            div().into_any_element()
        } else {
            let visible_ix = state.visible();
            let times: Vec<i64> = visible_ix.iter().map(|&i| state.moments[i].timestamp_ms).collect();
            let width = f32::from(self.geom.borrow().bounds.size.width);
            let gap = if width > 1.0 {
                (10.0 / width).max(0.002)
            } else {
                rail::MIN_GAP
            };
            let places = rail::positions(&times, gap);
            let bars = rail::density(&times);
            {
                let mut geom = self.geom.borrow_mut();
                geom.positions = places.clone();
                geom.visible = visible_ix.clone();
            }
            // The rail's start edge is the oldest stop; in a right-to-left window the oldest is at the right.
            let place = |p: f32| if rtl { 1.0 - p } else { p };

            // The chips: which sources' stops the rail shows.
            let present = state.sources_present();
            let chips = (present.len() > 1).then(|| {
                let mut row = row_start().items_center().gap(space(1.5)).flex_wrap().child(
                    pill("tm-source-all", t!("All"), state.source_filter.is_none(), None, &t, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(tab) = this.tab_id(cx) {
                                shared(cx).update(cx, |m, cx| m.set_source_filter(tab, None, cx));
                            }
                        }))
                        .child(div().px(space(1.5)).text_size(theme::size::XS).child(t!("All"))),
                );
                for (n, source) in present.iter().copied().enumerate() {
                    let color = rail::source_color(source, &t);
                    let label = chip_label(source);
                    let count = state.moments.iter().filter(|m| m.source == source).count();
                    row = row.child(
                        pill(
                            ("tm-source", n),
                            label.clone(),
                            state.source_filter == Some(source),
                            Some(color),
                            &t,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(tab) = this.tab_id(cx) {
                                shared(cx).update(cx, |m, cx| m.set_source_filter(tab, Some(source), cx));
                            }
                        }))
                        .child(
                            row_start()
                                .items_center()
                                .gap(space(1.))
                                .px(space(1.5))
                                .child(div().size(px(7.)).rounded_full().bg(color))
                                .child(div().text_size(theme::size::XS).child(label))
                                .child(
                                    div()
                                        .text_size(theme::size::XS)
                                        .text_color(t.muted_foreground)
                                        .child(format_count(count)),
                                ),
                        ),
                    );
                }
                row
            });

            // The sparkline of how crowded the history is, drawn quietly over the rail.
            let sparkline = row_start()
                .w_full()
                .h(px(16.))
                .items_end()
                .children(bars.iter().enumerate().map(|(i, v)| {
                    div()
                        .id(("tm-bar", i))
                        .flex_1()
                        .mx(px(0.5))
                        .h(px((2.0 + v * 14.0).min(16.0)))
                        .rounded_t(px(1.))
                        .bg(t.muted_foreground.opacity(0.16 + 0.22 * v))
                }));

            let geom = self.geom.clone();
            let dragging = self.dragging.clone();
            let drag_move = self.dragging.clone();
            let ticks = visible_ix.iter().enumerate().map(|(v, &mi)| {
                let m = &state.moments[mi];
                let active = mi == state.selected;
                let color = rail::source_color(m.source, &t);
                let name = rail::stop_name(m, now_ms);
                let when: SharedString = if m.timestamp_ms > 0 {
                    format_date(m.timestamp_ms, Some(now_ms), lang).into()
                } else {
                    m.detail.clone().into()
                };
                let tip = SharedString::from(format!("{} · {}", m.label, when));
                div()
                    .id(("tm-moment", mi))
                    .role(Role::Button)
                    .aria_label(name)
                    .test_support()
                    .absolute()
                    .left(relative(place(places.get(v).copied().unwrap_or(0.5))))
                    .ml(px(-6.))
                    .top_0()
                    .w(px(12.))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(move |window, cx| tooltip(tip.clone(), window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if let Some(tab) = this.tab_id(cx) {
                            shared(cx).update(cx, |model, cx| model.select_moment(tab, mi, cx));
                            window.focus(&this.focus_handle, cx);
                        }
                    }))
                    .child(
                        div()
                            .w(px(2.))
                            .h(px(if active { 24. } else { 14. }))
                            .rounded_full()
                            .bg(color.opacity(if active { 1. } else { 0.85 })),
                    )
            });

            let playhead = visible_ix.iter().position(|&i| i == state.selected).map(|v| {
                let m = &state.moments[state.selected];
                let color = rail::source_color(m.source, &t);
                div()
                    .id(("tm-playhead", self.pane))
                    .role(Role::Slider)
                    .aria_label(t!("Time Machine position"))
                    .aria_description(rail::stop_name(m, now_ms))
                    .test_support()
                    .absolute()
                    .left(relative(place(places.get(v).copied().unwrap_or(0.5))))
                    .ml(px(-7.))
                    .top_0()
                    .w(px(14.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .size(px(12.))
                            .flex_none()
                            .rounded_full()
                            .bg(t.background)
                            .border_2()
                            .border_color(color),
                    )
                    .child(div().w(px(2.)).flex_1().bg(color.opacity(0.7)))
            });

            let rail_box = div()
                .id(("tm-rail", self.pane))
                .role(Role::Group)
                .aria_label(t!("Timeline"))
                .w_full()
                .h(px(40.))
                .px(px(7.))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus_handle, cx);
                        dragging.set(true);
                        this.playhead_to(ev.position.x, true, cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                    if drag_move.get() && ev.dragging() {
                        this.playhead_to(ev.position.x, true, cx);
                    }
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
                .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.end_drag(cx)))
                .child(
                    div()
                        .relative()
                        .size_full()
                        .child(
                            canvas(
                                move |bounds, _, _| {
                                    geom.borrow_mut().bounds = bounds;
                                    bounds
                                },
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .size_full(),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top(px(19.))
                                .h(px(2.))
                                .rounded_full()
                                .bg(t.stroke),
                        )
                        .children(ticks)
                        .children(playhead),
                );

            col()
                .w_full()
                .px(space(4.))
                .py(space(2.5))
                .gap(space(1.5))
                .border_b_1()
                .border_color(t.stroke)
                .child(
                    row_start()
                        .w_full()
                        .min_h(space(6.))
                        .items_center()
                        .justify_between()
                        .gap(space(2.))
                        .children(chips)
                        .when(state.moments_loading, |r| {
                            r.child(
                                row_start()
                                    .items_center()
                                    .gap(space(1.5))
                                    .child(Spinner::new().color(t.muted_foreground))
                                    .child(
                                        div()
                                            .text_size(theme::size::XS)
                                            .text_color(t.muted_foreground)
                                            .child(t!("Still looking for more…")),
                                    ),
                            )
                        }),
                )
                .child(sparkline)
                .child(rail_box)
                .when_some(selected.clone(), |bar, m| {
                    let deleted_str = format_count(deleted_n);
                    let modified_str = format_count(modified_n);
                    let unchanged_str = format_count(unchanged_n);
                    let summary = t!(
                        "{deleted} deleted · {modified} modified · {unchanged} unchanged",
                        deleted = deleted_str,
                        modified = modified_str,
                        unchanged = unchanged_str
                    );
                    let make_filter = |id: &'static str, kind: FilterKind, label: SharedString| {
                        pill(id, label.clone(), state.filter == kind, None, &t, cx)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(tab) = this.tab_id(cx) {
                                    shared(cx).update(cx, |model, cx| model.set_filter(tab, kind, cx));
                                }
                            }))
                            .child(div().px(space(1.5)).text_size(theme::size::XS).child(label))
                    };
                    bar.child(
                        row_start()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap(space(2.))
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(2.))
                                    .child(
                                        div()
                                            .text_size(theme::size::XS)
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(m.label.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(theme::size::XS)
                                            .text_color(t.muted_foreground)
                                            .child(summary),
                                    ),
                            )
                            .child({
                                let portal_source =
                                    match (&m.source, &m.repo_root, &m.commit_oid, &m.plan_id, &m.snapshot_id) {
                                        (MomentSource::GitCommit, Some(repo), Some(rev), _, _) => Some((
                                            format!("{} {}", glint_model::paths::base_name(&state.folder), m.detail),
                                            glint_core::portals::PortalSource::GitCommit {
                                                repo_root: repo.clone(),
                                                rev: rev.clone(),
                                                subfolder: state.folder.clone(),
                                            },
                                        )),
                                        (MomentSource::ResticBackup, _, _, Some(plan), Some(snap)) => Some((
                                            format!(
                                                "{} backup {}",
                                                glint_model::paths::base_name(&state.folder),
                                                m.detail
                                            ),
                                            glint_core::portals::PortalSource::ResticSnapshot {
                                                plan_id: plan.clone(),
                                                snapshot_id: snap.clone(),
                                                subfolder: state.folder.clone(),
                                            },
                                        )),
                                        _ => None,
                                    };
                                row_start()
                                    .items_center()
                                    .gap(space(1.))
                                    .when_some(portal_source, |r, (name, source)| {
                                        r.child(
                                            Button::new("tm-mount-portal")
                                                .compact()
                                                .ghost()
                                                .icon(Icon::new(IconName::Pin))
                                                .label(t!("Mount as live folder"))
                                                .accessibility_label(t!("Mount snapshot as live folder"))
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(TimeMachineEvent::PinPortal {
                                                        name: name.clone(),
                                                        source: source.clone(),
                                                    });
                                                })),
                                        )
                                    })
                                    .child(make_filter("tm-filter-all", FilterKind::All, t!("All")))
                                    .child(make_filter(
                                        "tm-filter-changes",
                                        FilterKind::ChangesOnly,
                                        t!("Changes only"),
                                    ))
                                    .child(make_filter("tm-filter-deleted", FilterKind::Deleted, t!("Deleted")))
                            }),
                    )
                })
                .into_any_element()
        };

        let body = if state.entries_loading {
            col()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap(space(2.))
                .child(Spinner::new().color(t.primary))
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(t!("Reading snapshot…")),
                )
                .into_any_element()
        } else if let Some(err) = state.error.clone() {
            col()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap(space(2.))
                .px(space(6.))
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.warning))
                .child(div().text_size(theme::size::SM).child(err))
                .into_any_element()
        } else if state.moments.is_empty() && state.moments_loading {
            // Still asking the sources: the strip above has the spinner, and "nothing" is not known yet.
            div().flex_1().into_any_element()
        } else if state.moments.is_empty() {
            col()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap(space(2.))
                .px(space(6.))
                .child(icon(IconName::RotateCcwClock).size_8().text_color(t.muted_foreground))
                .child(
                    div()
                        .text_size(theme::size::SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child(t!("No past versions yet")),
                )
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(t!(
                            "Turn on File History, use Git or set up backups to browse how this folder looked."
                        )),
                )
                .into_any_element()
        } else if filtered_idx.is_empty() {
            col()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap(space(2.))
                .px(space(6.))
                .child(icon(IconName::Clock).size_8().text_color(t.muted_foreground))
                .child(
                    div()
                        .text_size(theme::size::SM)
                        .font_weight(FontWeight::MEDIUM)
                        .child(t!("No matching items in this snapshot")),
                )
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(t!("Switch filters or pick another point on the timeline above.")),
                )
                .into_any_element()
        } else {
            let entries = state.entries.clone();
            let folder = state.folder.clone();
            let row_count = filtered_idx.len();
            let on_row = state.row;
            uniform_list(
                "tm-entries",
                row_count,
                cx.processor(move |_this, range: Range<usize>, _, cx| {
                    let t = theme::tokens(cx).clone();
                    range
                        .filter_map(|row_pos| {
                            let ix = *filtered_idx.get(row_pos)?;
                            let entry = entries.get(ix)?;
                            let row_entry = entry.clone();
                            let cmp_entry = entry.clone();
                            let look_entry = entry.clone();
                            let restore_folder = folder.clone();
                            let cmp_folder = folder.clone();
                            let can_compare = !entry.dir
                                && !matches!(entry.delta, EntryDelta::Deleted)
                                && !matches!(entry.restore, RestoreSource::UsnMoved { .. });
                            let can_look = look::can_look(entry);
                            let can_restore = match &entry.restore {
                                RestoreSource::UsnMoved { now_path, .. } => now_path.is_some(),
                                _ => true,
                            };
                            let when: SharedString = if entry.modified > 0 {
                                format_date(entry.modified, Some(now_ms), lang).into()
                            } else {
                                SharedString::default()
                            };
                            let size_label: SharedString = if entry.dir {
                                t!("Folder")
                            } else {
                                format_size(entry.size).into()
                            };
                            let badge = delta_badge(&entry.delta, &t);
                            let at_row = on_row == Some(ix);
                            let hover = t.accent;

                            Some(
                                row_start()
                                    .id(("tm-row", ix))
                                    .role(Role::ListItem)
                                    .aria_label(match &badge {
                                        Some((label, _)) => SharedString::from(format!("{}, {label}", entry.name)),
                                        None => SharedString::from(entry.name.clone()),
                                    })
                                    .w_full()
                                    .h(px(36.))
                                    .px(space(4.))
                                    .gap(space(2.))
                                    .items_center()
                                    .justify_between()
                                    .border_b_1()
                                    .border_color(t.stroke.opacity(0.4))
                                    .when(at_row, |r| r.bg(t.selected))
                                    .hover(move |s| s.bg(hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(tab) = this.tab_id(cx) {
                                            shared(cx).update(cx, |m, cx| m.pick_row(tab, ix, cx));
                                            window.focus(&this.focus_handle, cx);
                                        }
                                    }))
                                    .child(
                                        row_start()
                                            .flex_1()
                                            .min_w_0()
                                            .items_center()
                                            .gap(space(2.))
                                            .child(
                                                icon(if entry.dir { IconName::Folder } else { IconName::File })
                                                    .size_4()
                                                    .flex_none()
                                                    .text_color(t.muted_foreground),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_size(theme::size::SM)
                                                    .child(entry.name.clone()),
                                            )
                                            .when_some(badge, |row, (label, color)| {
                                                row.child(
                                                    div()
                                                        .px(space(1.5))
                                                        .py(px(1.))
                                                        .rounded_full()
                                                        .bg(color.opacity(0.16))
                                                        .text_color(color)
                                                        .text_size(theme::size::XS)
                                                        .child(label),
                                                )
                                            }),
                                    )
                                    .child(
                                        row_start()
                                            .items_center()
                                            .gap(space(3.))
                                            .child(
                                                div()
                                                    .w(px(76.))
                                                    .text_end()
                                                    .text_size(theme::size::XS)
                                                    .text_color(t.muted_foreground)
                                                    .child(size_label),
                                            )
                                            .child(
                                                div()
                                                    .w(px(120.))
                                                    .text_end()
                                                    .text_size(theme::size::XS)
                                                    .text_color(t.muted_foreground)
                                                    .child(when),
                                            )
                                            .when(can_look, |r| {
                                                r.child(
                                                    Button::new(("tm-look", ix))
                                                        .compact()
                                                        .ghost()
                                                        .icon(Icon::new(IconName::Eye))
                                                        .accessibility_label(t!(
                                                            "Quick Look {name}",
                                                            name = look_entry.name.clone()
                                                        ))
                                                        .tooltip(t!("Quick Look (Space)"))
                                                        .on_click(cx.listener(move |this, _, window, cx| {
                                                            if let Some(tab) = this.tab_id(cx) {
                                                                shared(cx).update(cx, |m, cx| m.pick_row(tab, ix, cx));
                                                            }
                                                            this.quick_look_row(window, cx)
                                                        })),
                                                )
                                            })
                                            .when(can_compare, |r| {
                                                r.child(
                                                    Button::new(("tm-compare", ix))
                                                        .compact()
                                                        .ghost()
                                                        .label(t!("Compare"))
                                                        .accessibility_label(t!(
                                                            "Compare {name} with today",
                                                            name = cmp_entry.name.clone()
                                                        ))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.compare_entry(
                                                                cmp_folder.clone(),
                                                                cmp_entry.clone(),
                                                                cx,
                                                            )
                                                        })),
                                                )
                                            })
                                            .when(can_restore, |r| {
                                                r.child(
                                                    Button::new(("tm-restore", ix))
                                                        .compact()
                                                        .ghost()
                                                        .icon(Icon::new(IconName::RotateCcw))
                                                        .label(t!("Restore"))
                                                        .accessibility_label(t!(
                                                            "Restore {name}",
                                                            name = row_entry.name.clone()
                                                        ))
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            this.restore_entry(
                                                                restore_folder.clone(),
                                                                row_entry.clone(),
                                                                cx,
                                                            )
                                                        })),
                                                )
                                            }),
                                    )
                                    .into_any_element(),
                            )
                        })
                        .collect()
                }),
            )
            .track_scroll(&self.scroll)
            .flex_1()
            .size_full()
            .into_any_element()
        };

        col()
            .id(("time-machine", self.pane))
            .role(Role::Region)
            .aria_label(t!("Time Machine"))
            .key_context("TimeMachine")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(t.background)
            .child(header)
            .child(scrubber)
            .child(body)
            .into_any_element()
    }
}
