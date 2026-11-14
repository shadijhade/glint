//! Which tabs show their folder as a map, and the maps they show. Ported from src/lib/diskUsage.ts.
//!
//! | TypeScript                        | Here                                                                     |
//! |-----------------------------------|--------------------------------------------------------------------------|
//! | `useUsage` (zustand, by tab id)   | [`Usage`], one model entity in a global ([`shared`]); a [`Run`] per tab    |
//! | `showsUsage`, `setUsage`, `toggleUsage` | [`Usage::shows`], [`Usage::set`], [`Usage::toggle`]                  |
//! | `mapUsage`                        | [`Usage::map_path`]                                                       |
//! | `refreshUsage` (F5)               | [`Usage::refresh`]                                                        |
//! | `stopUsage`                       | [`Usage::stop`]                                                           |
//! | `markUsageShown`                  | [`Usage::mark_shown`]                                                     |
//! | `measuredMaps`, `partOfKnownMap`  | [`Usage::measured`] (the last eight pdu maps), `part_of_known_map`         |
//! | `remapSoon`, `dropGone`           | [`Usage::remap_soon`], `drop_gone`                                         |
//! | `entriesOf`, `useMapRows`         | [`Shown`] holds the map with its items as file-list entries, made once    |
//! | `useExplorer.subscribe(...)`      | the subscription in [`Usage::new`] to the explorer's events               |
//!
//! **Nothing here waits on the UI thread.** The core call (the index service, pdu, an archive's list) runs on
//! core's runtime behind a [`Source`]; turning its buffer into a map and its items into entries happens on a
//! background thread; the answer comes back to the model, which tells the views.

use crate::core_bridge;
use crate::explorer::{explorer, ExplorerEvent, NavOptions, TabId};
use crate::state;
use glint_model::entries::FileEntry;
use glint_model::paths::{is_folder_path, parent_path, same_path};
use glint_model::usage::{
    prune_usage, sub_usage, usage_entries, UsageEngine, UsageGrowth, UsageItem, UsageMap, WAS_NOT_THERE, WRITTEN_SINCE,
};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How many levels down a map goes (`DEPTH`).
pub const DEPTH: u32 = 6;
/// The most items a map shows, the biggest; the rest are grouped (`ITEMS`).
pub const ITEMS: u32 = 20_000;
/// How long after a folder is read again its index map is made again: the index reads the change journal every
/// 250 ms, so a second is when it has the change.
pub const REMAP_AFTER: Duration = Duration::from_secs(1);
/// How many pdu maps are kept to show again (the web app kept 8; a 20,000-node map with its entries is about
/// 10 MB, so 4 keeps the worst case near 40 MB).
const KEEP_MEASURED: usize = 4;

/// A boxed future core's work runs as.
pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// What the model asks core for (`diskUsage(path, DEPTH, ITEMS, tab, since)`).
#[derive(Clone, Debug, PartialEq)]
pub struct Ask {
    /// The folder to map.
    pub path: String,
    pub depth: u32,
    pub items: u32,
    /// The tab's lane: each tab maps on its own, so one pane's pdu run doesn't stop the other's.
    pub lane: String,
    /// Unix ms: what each item held at the index's daily mark closest to this comes along (index maps only).
    pub since: Option<f64>,
}

/// Where maps come from: glint-core in the app, a fake in tests.
pub trait Source {
    /// Maps a folder. The answer is `Err("cancelled")` when a newer map in the lane, or [`Source::cancel`], ended it.
    fn measure(&self, ask: Ask) -> Fut<Result<UsageMap, String>>;
    /// Stops the lane's pdu run (`cancelDiskUsage`).
    fn cancel(&self, lane: &str);
    /// Which of these paths no longer exist (`resolvePath` failing), for items deleted or moved away.
    fn missing(&self, paths: Vec<String>) -> Fut<Vec<String>>;
}

/// What core measured, as the map the view draws.
fn map_of(measured: glint_core::usage::Measured) -> Result<UsageMap, String> {
    use glint_core::usage::{NEW, WRITTEN};
    use glint_model::entries::attr;
    let items = measured
        .nodes
        .into_iter()
        .map(|node| UsageItem {
            parent: node.parent,
            name: node.name,
            size: node.size,
            modified: node.modified,
            attrs: node.attrs | if node.dir { attr::DIRECTORY } else { 0 },
        })
        .collect();
    let map = UsageMap::of_items(measured.path, items).map_err(|error| error.to_string())?;
    Ok(UsageMap {
        engine: measured.engine,
        elapsed: measured.elapsed,
        limited: measured.limited,
        service_too_old: measured.service_too_old,
        growth_too_old: measured.growth_too_old,
        growth: measured.growth.map(|(previous, mark)| UsageGrowth {
            since: mark.since as f64,
            oldest: mark.oldest as f64,
            drive_then: mark.drive_then as f64,
            drive_now: mark.drive_now as f64,
            previous: previous
                .into_iter()
                .map(|then| match then {
                    NEW => WAS_NOT_THERE,
                    WRITTEN => WRITTEN_SINCE,
                    then => then as f64,
                })
                .collect(),
        }),
        ..map
    })
}

/// glint-core's `disk_usage`, through the app's handle.
pub struct CoreUsage {
    app: glint_core::host::AppHandle,
}

impl CoreUsage {
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

impl Source for CoreUsage {
    fn measure(&self, ask: Ask) -> Fut<Result<UsageMap, String>> {
        use glint_core::host::Manager as _;
        let app = self.app.clone();
        let running = core_bridge::run(async move {
            let state = app
                .try_state::<glint_core::AppState>()
                .ok_or_else(|| "Glint's measuring isn't ready yet".to_string())?;
            let measured =
                glint_core::usage::disk_usage(state, ask.path, ask.depth, ask.items, ask.lane, ask.since).await?;
            map_of(measured)
        });
        // Two layers: the core task may panic (outer Err), the map may not come (inner Err).
        Box::pin(async move { running.await.and_then(|answer| answer) })
    }

    fn cancel(&self, lane: &str) {
        use glint_core::host::Manager as _;
        let app = self.app.clone();
        let lane = lane.to_string();
        // Ending the run kills pdu's process: not on the UI thread.
        glint_core::host::async_runtime::spawn_blocking(move || {
            if let Some(state) = app.try_state::<glint_core::AppState>() {
                glint_core::usage::cancel_disk_usage(state, lane);
            }
        });
    }

    fn missing(&self, paths: Vec<String>) -> Fut<Vec<String>> {
        let running = core_bridge::run(async move {
            glint_core::host::async_runtime::spawn_blocking(move || {
                paths
                    .into_iter()
                    .filter(|path| std::fs::symlink_metadata(path).is_err())
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        });
        Box::pin(async move { running.await.unwrap_or_default() })
    }
}

/// For an app whose core isn't up: every map fails with that reason.
pub struct NoCore;

impl Source for NoCore {
    fn measure(&self, _: Ask) -> Fut<Result<UsageMap, String>> {
        Box::pin(async { Err("Glint's measuring isn't available here".to_string()) })
    }

    fn cancel(&self, _: &str) {}

    fn missing(&self, _: Vec<String>) -> Fut<Vec<String>> {
        Box::pin(async { Vec::new() })
    }
}

static NEXT_SHOWN: AtomicU64 = AtomicU64::new(1);

/// A map with its items as file-list entries (`entriesOf`), made once per map, in node order (the folder mapped
/// first). The selection, the keys and the menus work on these.
pub struct Shown {
    /// Tells one map from another (the paint plan is made again when it changes).
    pub id: u64,
    pub map: UsageMap,
    pub entries: Vec<FileEntry>,
    by_path: HashMap<String, usize>,
}

impl Shown {
    /// Makes the entries and the path index for `map`. Meant for a background thread: a big map has twenty
    /// thousand nodes.
    pub fn new(map: UsageMap) -> Self {
        let entries = usage_entries(&map);
        let by_path = map.paths.iter().enumerate().map(|(i, p)| (p.clone(), i)).collect();
        Self {
            id: NEXT_SHOWN.fetch_add(1, Ordering::Relaxed),
            map,
            entries,
            by_path,
        }
    }

    /// The node at `path`, if the map has it.
    pub fn node_of(&self, path: &str) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    /// The node's entry (a file or folder as the list would show it).
    pub fn entry(&self, node: usize) -> Option<&FileEntry> {
        self.entries.get(node)
    }
}

/// A tab's disk-usage map, from the index or pdu (`UsageRun`).
#[derive(Clone)]
pub struct Run {
    /// Which request this is: an answer for an older one is dropped.
    pub id: u64,
    /// The folder being mapped.
    pub path: String,
    /// The latest map: this folder's once it arrives, until then the one shown before, if any.
    pub shown: Option<Arc<Shown>>,
    pub running: bool,
    /// Who answers (while running, who's expected to).
    pub engine: UsageEngine,
    /// Why the last map failed ("" for none, and for Stop).
    pub error: String,
    /// When the click or key that asked for it happened.
    pub started: Instant,
    /// From that click to the map on screen, ms; 0 until it's drawn.
    pub shown_in: f64,
    /// The map shown is this folder's part of an earlier map, while pdu measures it.
    pub preview: bool,
}

impl Run {
    /// The map when it's this run's folder's own (not an earlier one shown meanwhile).
    pub fn current(&self) -> Option<&Arc<Shown>> {
        self.shown.as_ref().filter(|s| same_path(&s.map.path, &self.path))
    }
}

/// What the model tells the window about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageEvent {
    /// A tab started or stopped showing its map.
    Toggled { tab: TabId, on: bool },
}

/// The tabs that show their folder as a map, and their maps (`useUsage`). One lives in a global ([`shared`]).
pub struct Usage {
    source: Rc<dyn Source>,
    runs: HashMap<TabId, Run>,
    /// The last pdu maps, oldest first: going back to one shows it again instead of measuring it again (F5
    /// does). Index maps are always asked for afresh; they take milliseconds.
    measured: Vec<Arc<Shown>>,
    /// The index service answered as a version from before maps: expect pdu until one comes from the index.
    service_too_old: bool,
    seq: u64,
    /// The one-second waits before an index map is made again.
    timers: HashMap<TabId, Task<()>>,
    /// What was selected last in each map (what a delete takes away needs finding, the selection is gone by then).
    last_selected: HashMap<TabId, Vec<String>>,
    /// How long after a re-read the index map is made again (a test shortens it).
    remap_after: Duration,
    _subscription: Subscription,
}

impl EventEmitter<UsageEvent> for Usage {}

impl Usage {
    /// A model that maps through `source` and follows the explorer's tabs.
    pub fn new(source: Rc<dyn Source>, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        // A map follows its tab: a folder opened is mapped, one read again is mapped again shortly, and a tab
        // closed takes its map along (`useExplorer.subscribe`).
        let subscription = cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| match event {
            ExplorerEvent::ListingChanged { tab, .. } => this.listing_changed(*tab, cx),
            ExplorerEvent::TabClosed { tab, .. } => this.forget(*tab, cx),
            ExplorerEvent::TabsChanged => this.forget_closed(cx),
            ExplorerEvent::SelectionChanged(tab) => this.note_selection(*tab, cx),
            ExplorerEvent::TabChanged(tab) => this.hand_rename_to_list(*tab, cx),
            _ => {}
        });
        Self {
            source,
            runs: HashMap::new(),
            measured: Vec::new(),
            service_too_old: false,
            seq: 0,
            timers: HashMap::new(),
            last_selected: HashMap::new(),
            remap_after: REMAP_AFTER,
            _subscription: subscription,
        }
    }

    /// Changes how long after a re-read an index map is made again.
    #[cfg(test)]
    pub fn set_remap_after(&mut self, after: Duration) {
        self.remap_after = after;
    }

    /// The tab's run, if it shows its map.
    pub fn run(&self, tab: TabId) -> Option<&Run> {
        self.runs.get(&tab)
    }

    /// Whether a tab shows its disk-usage map (`showsUsage`).
    pub fn shows(&self, tab: TabId) -> bool {
        self.runs.contains_key(&tab)
    }

    /// The pdu maps kept to show again, oldest first.
    pub fn measured(&self) -> &[Arc<Shown>] {
        &self.measured
    }

    /// Shows a tab's folder as a disk-usage map (`on`), or as a list again (`setUsage`).
    pub fn set(&mut self, tab: TabId, on: bool, cx: &mut Context<Self>) {
        if !on || self.runs.contains_key(&tab) {
            if !on && self.runs.contains_key(&tab) {
                self.stop(tab, cx);
                self.remove(tab, cx);
            }
            return;
        }
        let explorer = explorer(cx);
        let Some(path) = explorer.read(cx).tab(tab).map(|t| t.path.clone()) else {
            return;
        };
        // A name being edited is the list's.
        if explorer.read(cx).tab(tab).is_some_and(|t| t.renaming.is_some()) {
            explorer.update(cx, |e, cx| e.cancel_rename_in(tab, cx));
        }
        self.runs.insert(
            tab,
            Run {
                id: 0,
                path: path.clone(),
                shown: None,
                running: false,
                engine: "pdu",
                error: String::new(),
                started: Instant::now(),
                shown_in: 0.0,
                preview: false,
            },
        );
        cx.emit(UsageEvent::Toggled { tab, on: true });
        cx.notify();
        if is_folder_path(&path) {
            self.map_path(tab, &path, false, cx);
        }
    }

    /// The command bar's and the palette's Disk usage: map or list (`toggleUsage`).
    pub fn toggle(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let on = !self.runs.contains_key(&tab);
        self.set(tab, on, cx);
    }

    /// Takes the tab's run away and tells the window.
    fn remove(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.timers.remove(&tab);
        self.last_selected.remove(&tab);
        if self.runs.remove(&tab).is_some() {
            cx.emit(UsageEvent::Toggled { tab, on: false });
            cx.notify();
        }
    }

    /// A tab closed: its map goes, and pdu stops if it was measuring.
    fn forget(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if self.runs.contains_key(&tab) {
            self.stop(tab, cx);
            self.remove(tab, cx);
        }
    }

    /// Tabs came and went: the maps of tabs that are gone go too.
    fn forget_closed(&mut self, cx: &mut Context<Self>) {
        let explorer = explorer(cx);
        let gone: Vec<TabId> = self
            .runs
            .keys()
            .copied()
            .filter(|tab| explorer.read(cx).tab(*tab).is_none())
            .collect();
        for tab in gone {
            self.forget(tab, cx);
        }
    }

    /// Maps `path` in a tab that shows its map: from the index when it has the folder, else with pdu
    /// (`mapUsage`). `again` measures a folder pdu measured before instead of showing that map.
    pub fn map_path(&mut self, tab: TabId, path: &str, again: bool, cx: &mut Context<Self>) {
        self.timers.remove(&tab);
        let Some(before) = self.runs.get(&tab).cloned() else {
            return;
        };
        if !is_folder_path(path) {
            return;
        }
        self.seq += 1;
        let id = self.seq;
        // A folder in an archive is mapped from the archive's own list of entries, at once.
        let engine = self.expected_engine(tab, path, cx);
        let mut run = Run {
            id,
            path: path.to_string(),
            shown: before.shown.clone(),
            running: true,
            engine,
            error: String::new(),
            started: Instant::now(),
            shown_in: 0.0,
            preview: false,
        };
        if engine == "pdu" {
            let known = (!again).then(|| self.known_map(path)).flatten();
            if let Some(known) = known {
                run.shown = Some(known);
                run.running = false;
                self.runs.insert(tab, run);
                cx.notify();
                return;
            }
            self.show_part_of_known_map(tab, id, path, before.shown.clone(), cx);
        }
        self.runs.insert(tab, run);
        cx.notify();

        // The index tells what grew too, for the Growth colors (it costs it a millisecond or two).
        let growth_days = f64::from(state::settings(cx).growth_days);
        let ask = Ask {
            path: path.to_string(),
            depth: DEPTH,
            items: ITEMS,
            lane: tab.to_string(),
            since: Some(now_ms() - growth_days * 86_400_000.0),
        };
        let measuring = self.source.measure(ask);
        cx.spawn(async move |this, cx| {
            let answer = measuring.await;
            let built = match answer {
                Ok(map) => Ok(cx.background_executor().spawn(async move { Shown::new(map) }).await),
                Err(reason) => Err(reason),
            };
            this.update(cx, |this, cx| this.arrived(tab, id, built, cx)).ok();
        })
        .detach();
    }

    /// Who is expected to answer for a map of `path` in `tab`.
    fn expected_engine(&self, tab: TabId, path: &str, cx: &App) -> UsageEngine {
        let explorer = explorer(cx);
        let explorer = explorer.read(cx);
        if let Some(archive) = explorer.tab(tab).and_then(|t| t.archive()) {
            return archive.format;
        }
        let indexed = explorer.search_index().is_some_and(|index| index.indexed(path));
        if indexed && !self.service_too_old {
            "index"
        } else {
            "pdu"
        }
    }

    /// The pdu map kept for `path`, if there is one.
    fn known_map(&self, path: &str) -> Option<Arc<Shown>> {
        self.measured
            .iter()
            .rev()
            .find(|s| same_path(&s.map.path, path))
            .cloned()
    }

    /// Shows `path`'s part of a map on hand while pdu measures it (`partOfKnownMap`): the map shown, or one of
    /// the last measured. The cutting happens on a background thread.
    fn show_part_of_known_map(
        &self,
        tab: TabId,
        id: u64,
        path: &str,
        current: Option<Arc<Shown>>,
        cx: &mut Context<Self>,
    ) {
        let candidates: Vec<Arc<Shown>> = current.into_iter().chain(self.measured.iter().rev().cloned()).collect();
        if candidates.is_empty() {
            return;
        }
        let wanted = path.to_lowercase();
        cx.spawn(async move |this, cx| {
            let part = cx
                .background_executor()
                .spawn(async move {
                    candidates.iter().find_map(|shown| {
                        let map = &shown.map;
                        let node = map.paths.iter().position(|p| p.to_lowercase() == wanted)?;
                        if node == 0 || map.end.get(node) <= map.first.get(node) {
                            return None;
                        }
                        sub_usage(map, node).map(Shown::new)
                    })
                })
                .await;
            let Some(part) = part else { return };
            this.update(cx, |this, cx| {
                // Only while the same request is still waiting for its own map.
                if let Some(run) = this.runs.get_mut(&tab).filter(|r| r.id == id && r.running) {
                    run.shown = Some(Arc::new(part));
                    run.preview = true;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The map (or why not) for request `id` came back.
    fn arrived(&mut self, tab: TabId, id: u64, built: Result<Shown, String>, cx: &mut Context<Self>) {
        if !self.runs.get(&tab).is_some_and(|run| run.id == id) {
            return;
        }
        match built {
            Ok(shown) => {
                let shown = Arc::new(shown);
                self.service_too_old = shown.map.service_too_old;
                if shown.map.engine == "pdu" {
                    self.remember(shown.clone());
                }
                if let Some(run) = self.runs.get_mut(&tab) {
                    run.engine = shown.map.engine;
                    run.shown = Some(shown);
                    run.running = false;
                    run.preview = false;
                    run.error.clear();
                }
            }
            Err(reason) => {
                if let Some(run) = self.runs.get_mut(&tab) {
                    run.running = false;
                    // Stop is not a failure.
                    run.error = if reason == "cancelled" { String::new() } else { reason };
                }
            }
        }
        cx.notify();
    }

    /// Keeps a pdu map to show again (`remember`): the eight most recent, one per folder.
    pub(super) fn remember(&mut self, shown: Arc<Shown>) {
        self.measured.retain(|s| !same_path(&s.map.path, &shown.map.path));
        self.measured.push(shown);
        if self.measured.len() > KEEP_MEASURED {
            self.measured.remove(0);
        }
    }

    /// F5 on a map: reads the folder again and maps it again (measures it again when pdu drew it)
    /// (`refreshUsage`).
    pub fn refresh(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let explorer = explorer(cx);
        let reload = explorer.update(cx, |e, cx| e.reload(Some(tab), cx));
        cx.spawn(async move |this, cx| {
            reload.await;
            this.update(cx, |this, cx| {
                let path = explorer.read(cx).tab(tab).map(|t| t.path.clone());
                if let Some(path) = path.filter(|p| is_folder_path(p)) {
                    this.map_path(tab, &path, true, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Stops a tab's map that pdu is still measuring (`stopUsage`).
    pub fn stop(&mut self, tab: TabId, _cx: &mut Context<Self>) {
        self.timers.remove(&tab);
        if self.runs.get(&tab).is_some_and(|run| run.running) {
            self.source.cancel(&tab.to_string());
        }
    }

    /// Called once a map is drawn, to time it from the click; not for a part of an earlier map shown meanwhile
    /// (`markUsageShown`).
    pub fn mark_shown(&mut self, tab: TabId, id: u64, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get_mut(&tab) else { return };
        if run.id == id && !run.running && run.shown_in == 0.0 {
            // Never exactly 0, which means "not drawn yet".
            run.shown_in = (run.started.elapsed().as_secs_f64() * 1000.0).max(0.001);
            cx.notify();
        }
    }

    /// The tab's folder was listed (or listed again): a folder opened is mapped, one read again is mapped
    /// again shortly.
    fn listing_changed(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let _span = crate::perf::span("usage listing");
        let Some(run) = self.runs.get(&tab) else { return };
        let explorer = explorer(cx);
        let Some((path, has_listing)) = explorer
            .read(cx)
            .tab(tab)
            .map(|t| (t.path.clone(), t.listing.is_some()))
        else {
            return;
        };
        if !has_listing {
            self.stop(tab, cx);
            return;
        }
        if !same_path(&path, &run.path) || (run.shown.is_none() && !run.running) {
            self.map_path(tab, &path, false, cx);
        } else {
            self.drop_gone(tab, cx);
            self.remap_soon(tab, cx);
        }
    }

    /// After the tab's folder was read again (it changed, or files were pasted or deleted), maps it again from
    /// the index once the index has the change. A pdu map stays as measured until F5 (`remapSoon`).
    fn remap_soon(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let after = self.remap_after;
        let timer = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after).await;
            this.update(cx, |this, cx| {
                this.timers.remove(&tab);
                let Some(run) = this.runs.get(&tab) else { return };
                let path = explorer(cx).read(cx).tab(tab).map(|t| t.path.clone());
                let from_index = run.shown.as_ref().is_some_and(|s| s.map.engine != "pdu");
                if let Some(path) = path.filter(|p| !run.running && from_index && same_path(&run.path, p)) {
                    this.map_path(tab, &path, false, cx);
                }
            })
            .ok();
        });
        self.timers.insert(tab, timer);
    }

    /// Remembers what is selected in the map: after a delete the selection is gone with the items.
    fn note_selection(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get(&tab) else { return };
        let Some(shown) = run.shown.clone() else { return };
        let selected: Vec<String> = explorer(cx)
            .read(cx)
            .tab(tab)
            .map(|t| {
                t.selection
                    .iter()
                    .filter(|p| shown.node_of(p).is_some())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if !selected.is_empty() {
            self.last_selected.insert(tab, selected);
        }
    }

    /// Takes the items that were selected and are gone now (deleted, moved away) off the tab's maps at once
    /// (`dropGone`).
    fn drop_gone(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get(&tab) else { return };
        let Some(shown) = run.shown.clone() else { return };
        let in_map: Vec<String> = self
            .last_selected
            .remove(&tab)
            .unwrap_or_default()
            .into_iter()
            .filter(|p| shown.node_of(p).is_some())
            .collect();
        if in_map.is_empty() {
            return;
        }
        let id = run.id;
        let asking = self.source.missing(in_map);
        let measured = self.measured.clone();
        cx.spawn(async move |this, cx| {
            let gone: HashSet<String> = asking.await.into_iter().collect();
            if gone.is_empty() {
                return;
            }
            let (now, kept) = cx
                .background_executor()
                .spawn(async move {
                    let cut = |known: &Arc<Shown>| match prune_usage(&known.map, &gone) {
                        std::borrow::Cow::Borrowed(_) => known.clone(),
                        std::borrow::Cow::Owned(map) => Arc::new(Shown::new(map)),
                    };
                    (cut(&shown), measured.iter().map(cut).collect::<Vec<_>>())
                })
                .await;
            this.update(cx, |this, cx| {
                this.measured = kept;
                if let Some(run) = this.runs.get_mut(&tab).filter(|r| r.id == id) {
                    run.shown = Some(now);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Names are edited in the list: Rename (F2 or the menu) hands over to it, in the item's own folder.
    fn hand_rename_to_list(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(run) = self.runs.get(&tab) else { return };
        let explorer = explorer(cx);
        let Some(renaming) = explorer.read(cx).tab(tab).and_then(|t| t.renaming.clone()) else {
            return;
        };
        let folder = parent_path(&renaming);
        let same_folder = same_path(&folder, &run.path);
        self.set(tab, false, cx);
        if same_folder {
            return;
        }
        let navigation = explorer.update(cx, |e, cx| {
            e.navigate_task(
                folder,
                NavOptions {
                    select: Some(vec![renaming.clone()]),
                    tab: Some(tab),
                    ..Default::default()
                },
                cx,
            )
        });
        cx.spawn(async move |_, cx| {
            if navigation.await {
                explorer.update(cx, |e, cx| e.begin_rename(Some(&renaming), cx));
            }
        })
        .detach();
    }
}

/// Now, in Unix milliseconds.
fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

// ---- The shared model -----------------------------------------------------------------------------------

/// The shared model, in a GPUI `Global`.
struct SharedUsage(Entity<Usage>);

impl Global for SharedUsage {}

/// Makes the shared model, mapping through glint-core. Call after `core_bridge::init` and `explorer::init`.
pub fn init(cx: &mut App) {
    let source: Rc<dyn Source> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Rc::new(CoreUsage::new(core.app.clone())),
        None => Rc::new(NoCore),
    };
    init_with(cx, source);
}

/// [`init`] with a source of your own (tests, tools).
pub fn init_with(cx: &mut App, source: Rc<dyn Source>) {
    let usage = cx.new(|cx| Usage::new(source, cx));
    cx.set_global(SharedUsage(usage));
}

/// The shared model, made on first use if [`init`] has not run.
pub fn shared(cx: &mut App) -> Entity<Usage> {
    if cx.try_global::<SharedUsage>().is_none() {
        init(cx);
    }
    cx.global::<SharedUsage>().0.clone()
}

/// The shared model, if it exists.
pub fn try_shared(cx: &App) -> Option<Entity<Usage>> {
    cx.try_global::<SharedUsage>().map(|shared| shared.0.clone())
}
