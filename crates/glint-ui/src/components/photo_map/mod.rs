//! Offline 120 FPS Vector Photo World Map (`Ctrl+Shift+G` or header globe toggle).
//!
//! Renders embedded continent polygons via GPUI `PathBuilder`, O(N) screen-space pin clustering,
//! smooth cursor-anchored zoom/pan, and a cluster drawer to reveal or open geotagged photos.
//!
//! A frame costs what the camera changed: the photos are one shared set (`GeoSet`, positions worked out once),
//! and the clusters of a camera are worked out once and shared with the paint closure ([`frame::ClusterCache`]).

mod drawer;
mod frame;
mod pictures;
mod pins;
mod scrubber;

use crate::components::file_grid::thumbnail::{thumbnails, Thumbnails};
use crate::components::panel_header::{panel_header, tool_button, Header};
use crate::explorer::{explorer, Explorer, ExplorerEvent, TabId};
use crate::kit::prelude::*;
use crate::{t, theme};
use frame::{ClusterCache, Frame};
use glint_model::paths::{is_folder_path, is_gallery, same_path};
use glint_model::places::gazetteer;
use glint_model::world_map::{
    project, travel_stops, GeoPhoto, GeoSet, MapCamera, PinCluster, Stop, Timeline, LAND_POLYGONS,
};
use pictures::Leads;
use pins::{Colors, Ink};
use scrubber::Scope;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default)]
pub struct TabMapState {
    pub folder: String,
    pub photos: GeoSet,
    pub loading: bool,
    pub serial: u64,
}

/// Finds a folder's geotagged photos: calls the sink with each batch as it finds them, and returns when done.
pub type GeoLoader = Arc<dyn Fn(String, &(dyn Fn(Vec<GeoPhoto>) + Sync)) + Send + Sync>;

/// A loader that finds all of a folder's photos at once (what tests, and anything not streaming, use).
pub fn loader_of(find: impl Fn(String) -> Vec<GeoPhoto> + Send + Sync + 'static) -> GeoLoader {
    Arc::new(move |folder, sink| {
        let found = find(folder);
        if !found.is_empty() {
            sink(found);
        }
    })
}

/// How often the map takes what a running scan has found so far.
const SHOW_EVERY: Duration = Duration::from_millis(100);

struct GlobalState(Entity<PhotoMapModel>);
impl Global for GlobalState {}

/// Shared per-tab state for the offline Photo World Map.
pub struct PhotoMapModel {
    loader: GeoLoader,
    shown: HashSet<TabId>,
    tabs: HashMap<TabId, TabMapState>,
    serial: u64,
    _sub: Subscription,
}

impl PhotoMapModel {
    fn new(loader: GeoLoader, cx: &mut Context<Self>) -> Self {
        let exp = explorer(cx);
        let sub = cx.subscribe(&exp, |this, exp, ev: &ExplorerEvent, cx| match ev {
            ExplorerEvent::TabClosed { tab, .. } => {
                let removed = this.shown.remove(tab);
                let dropped = this.tabs.remove(tab).is_some();
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
                    if can_show_path(&path) {
                        this.load_folder(*tab, path, cx);
                    } else {
                        this.shown.remove(tab);
                        this.tabs.remove(tab);
                        cx.notify();
                    }
                }
            }
            _ => {}
        });
        Self {
            loader,
            shown: HashSet::new(),
            tabs: HashMap::new(),
            serial: 0,
            _sub: sub,
        }
    }

    pub fn shows(&self, tab: TabId) -> bool {
        self.shown.contains(&tab)
    }

    pub fn state_of(&self, tab: TabId) -> Option<&TabMapState> {
        self.tabs.get(&tab)
    }

    pub fn set(&mut self, tab: TabId, on: bool, folder: Option<String>, cx: &mut Context<Self>) {
        if !on {
            let removed = self.shown.remove(&tab);
            let dropped = self.tabs.remove(&tab).is_some();
            if removed || dropped {
                cx.notify();
            }
            return;
        }
        let Some(folder) = folder.filter(|p| can_show_path(p)) else {
            return;
        };
        self.shown.insert(tab);
        self.load_folder(tab, folder, cx);
    }

    pub fn toggle(&mut self, tab: TabId, folder: Option<String>, cx: &mut Context<Self>) {
        let next = !self.shows(tab);
        self.set(tab, next, folder, cx);
    }

    fn load_folder(&mut self, tab: TabId, folder: String, cx: &mut Context<Self>) {
        self.serial += 1;
        let serial = self.serial;
        self.tabs.insert(
            tab,
            TabMapState {
                folder: folder.clone(),
                photos: GeoSet::default(),
                loading: true,
                serial,
            },
        );
        cx.notify();

        // The scan runs on a thread of the pool and hands its finds over as it makes them.
        let loader = self.loader.clone();
        let (sender, receiver) = async_channel::unbounded::<Vec<GeoPhoto>>();
        cx.background_executor()
            .spawn(async move {
                (loader)(folder, &|batch| {
                    let _ = sender.try_send(batch);
                });
            })
            .detach();
        cx.spawn(async move |this, cx| {
            let mut found: Vec<GeoPhoto> = Vec::new();
            let mut last: Option<Instant> = None;
            loop {
                // The next batch, or the end of the scan (the sender is gone, nothing more to come).
                let mut done = match receiver.recv().await {
                    Ok(batch) => {
                        found.extend(batch);
                        false
                    }
                    Err(_) => true,
                };
                // What is already waiting goes with it; and a map that was just updated waits out the rest of the
                // interval, so a fast scan doesn't redraw it a hundred times a second.
                if !done {
                    if let Some(wait) = last.map(|at| SHOW_EVERY.saturating_sub(at.elapsed())) {
                        if !wait.is_zero() {
                            cx.background_executor().timer(wait).await;
                        }
                    }
                }
                while let Ok(more) = receiver.try_recv() {
                    found.extend(more);
                }
                done |= receiver.is_closed() && receiver.is_empty();
                // The set (newest first, positions worked out) is made off the UI thread from a copy of the
                // photos so far (their texts are shared, so the copy is cheap).
                let mut snapshot = found.clone();
                let photos = cx
                    .background_spawn(async move {
                        snapshot.sort_by_key(|p| std::cmp::Reverse(p.taken));
                        GeoSet::new(snapshot)
                    })
                    .await;
                let kept = this
                    .update(cx, |this, cx| match this.tabs.get_mut(&tab) {
                        Some(st) if st.serial == serial => {
                            st.photos = photos;
                            st.loading = !done;
                            cx.notify();
                            true
                        }
                        _ => false,
                    })
                    .unwrap_or(false);
                if done || !kept {
                    break;
                }
                last = Some(Instant::now());
            }
        })
        .detach();
    }
}

fn can_show_path(path: &str) -> bool {
    is_folder_path(path) || is_gallery(path)
}

/// Initializes [`PhotoMapModel`] with a custom loader (used by headless tests).
pub fn init_with(cx: &mut App, loader: GeoLoader) -> Entity<PhotoMapModel> {
    let entity = cx.new(|cx| PhotoMapModel::new(loader, cx));
    cx.set_global(GlobalState(entity.clone()));
    entity
}

/// Returns the shared [`PhotoMapModel`], initializing it on demand.
pub fn shared(cx: &mut App) -> Entity<PhotoMapModel> {
    if let Some(state) = cx.try_global::<GlobalState>() {
        return state.0.clone();
    }
    let loader: GeoLoader = Arc::new(|folder, sink| glint_core::gallery::geo_photos_stream(&folder, sink));
    init_with(cx, loader)
}

/// Returns the shared [`PhotoMapModel`] if already initialized.
pub fn try_shared(cx: &App) -> Option<Entity<PhotoMapModel>> {
    cx.try_global::<GlobalState>().map(|s| s.0.clone())
}

/// Whether `tab` has Photo Map turned on.
pub fn shows(cx: &App, tab: TabId) -> bool {
    try_shared(cx).is_some_and(|m| m.read(cx).shows(tab))
}

/// Whether `pane`'s tab should draw the Photo World Map instead of files.
pub fn visible(cx: &App, tab: TabId) -> bool {
    shows(cx, tab)
        && explorer(cx)
            .read(cx)
            .tab(tab)
            .is_some_and(|t| t.find.is_none() && can_show_path(&t.path))
}

/// Shows or hides Photo World Map for `tab`.
pub fn set(cx: &mut App, tab: TabId, on: bool) {
    let folder = explorer(cx).read(cx).tab(tab).map(|t| t.path.clone());
    let model = shared(cx);
    model.update(cx, |m, cx| m.set(tab, on, folder, cx));
}

/// Toggles Photo World Map for `tab`.
pub fn toggle(cx: &mut App, tab: TabId) {
    let folder = explorer(cx).read(cx).tab(tab).map(|t| t.path.clone());
    let model = shared(cx);
    model.update(cx, |m, cx| m.toggle(tab, folder, cx));
}

/// Actions the Photo World Map asks the workspace to perform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhotoMapEvent {
    /// Reveals a geotagged photo in the regular file list (`close map + reveal`).
    Reveal { path: String },
    /// Opens a geotagged photo directly.
    Open { path: String },
}

impl EventEmitter<PhotoMapEvent> for PhotoMapView {}

/// How long the camera takes to fly from one view to another: quiet, and over before it is noticed as a wait.
const FLY: Duration = theme::motion::MORPH;

/// The camera on its way: from where it was to where it is going, since when.
#[derive(Clone, Copy)]
struct Fly {
    from: MapCamera,
    to: MapCamera,
    at: Instant,
}

#[derive(Clone, Copy)]
struct DragState {
    start: Point<Pixels>,
    last: Point<Pixels>,
    moved: bool,
}

/// Offline 120 FPS interactive vector Photo World Map view for a pane.
pub struct PhotoMapView {
    pane: usize,
    explorer: Entity<Explorer>,
    model: Entity<PhotoMapModel>,
    focus: FocusHandle,
    camera: MapCamera,
    /// The load (its serial) the camera was last fitted to, and how far its scan had got: 1 while photos were still
    /// coming, 2 when they were all in.
    fitted_for: Option<(u64, u8)>,
    /// The person moved the camera since it was fitted: a scan that finishes doesn't take it back.
    touched: bool,
    clusters: ClusterCache,
    thumbs: Entity<Thumbnails>,
    leads: Leads,
    /// The camera as the pictures were last asked for, whether it has rested since, and the timer that says so.
    seen_camera: MapCamera,
    /// How many times the camera has moved: the rest timer asks whether it has since it was set.
    moves: u64,
    settled: bool,
    settle: Option<Task<()>>,
    /// A fingerprint of what was last asked of the thumbnail service.
    asked: u64,
    /// One focus handle for each tile of the drawer.
    tile_focus: Vec<FocusHandle>,
    /// The camera flying to where it is going (a double-click on a pin, a new time chosen), if it is.
    fly: Option<Fly>,
    /// The time shown (the date scrubber), and the one the camera was last flown to.
    scope: Scope,
    fitted_scope: Scope,
    /// Dashed arcs between the places, in the order they were visited.
    arcs: bool,
    /// What each choice of the above makes of the photos, worked out once: the photos of the time shown, the
    /// years the photos span, and the stops of the trip.
    shown: Option<(usize, Scope, GeoSet)>,
    timeline: Option<(usize, Arc<Timeline>)>,
    stops: Option<(usize, Arc<[Stop]>)>,
    drag: Option<DragState>,
    selected_cluster: Option<usize>,
    map_bounds: Rc<Cell<Bounds<Pixels>>>,
    _subs: [Subscription; 3],
}

impl PhotoMapView {
    pub fn new(pane: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        let model = shared(cx);
        let sub_m = cx.observe_in(&model, window, |this, _, _, cx| {
            this.selected_cluster = None;
            cx.notify();
        });
        let sub_e = cx.observe_in(&explorer, window, |_, _, _, cx| cx.notify());
        // A picture arriving from the thumbnail service redraws the map; a closed view lets go of what it asked for.
        let thumbs = thumbnails(cx);
        let sub_t = cx.observe(&thumbs, |_, _, cx| cx.notify());
        let (released, id) = (thumbs.clone(), cx.entity_id());
        cx.on_release(move |_, cx| released.update(cx, |thumbs, cx| thumbs.release(id, cx)))
            .detach();
        Self {
            pane,
            explorer,
            model,
            focus: cx.focus_handle(),
            camera: MapCamera::default(),
            fitted_for: None,
            touched: false,
            clusters: ClusterCache::default(),
            thumbs,
            leads: Leads::default(),
            seen_camera: MapCamera::default(),
            moves: 0,
            settled: true,
            settle: None,
            asked: 0,
            tile_focus: Vec::new(),
            fly: None,
            scope: Scope::All,
            fitted_scope: Scope::All,
            arcs: false,
            shown: None,
            timeline: None,
            stops: None,
            drag: None,
            selected_cluster: None,
            map_bounds: Rc::new(Cell::new(Bounds::default())),
            _subs: [sub_m, sub_e, sub_t],
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn tab_id(&self, cx: &App) -> TabId {
        self.explorer.read(cx).pane_tab_id(self.pane)
    }

    /// The photos of this pane's tab (a cheap clone of the shared set), whether they are still coming, and which
    /// load they belong to.
    fn current(&self, cx: &App) -> (GeoSet, bool, u64) {
        let tab = self.tab_id(cx);
        match self.model.read(cx).state_of(tab) {
            Some(st) => (st.photos.clone(), st.loading, st.serial),
            None => (GeoSet::default(), false, 0),
        }
    }

    /// What the camera sees, from the cache when nothing changed since the last frame.
    fn clusters_now(&mut self, set: &GeoSet) -> Arc<Frame> {
        let bounds = self.map_bounds.get();
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        if w > 2.0 && h > 2.0 {
            self.clusters.get(set, &self.camera, w, h)
        } else {
            Arc::default()
        }
    }

    fn fit_all(&mut self, cx: &mut Context<Self>) {
        let (all, _, _) = self.current(cx);
        let shown = self.shown_of(&all);
        let to = self.fit_camera(&shown);
        self.touched = false;
        self.fly_to(to, cx);
    }

    /// The photos of the time chosen: all of them for all time, else a set of their own, kept while the photos and
    /// the time stay as they are.
    fn shown_of(&mut self, all: &GeoSet) -> GeoSet {
        let Some((from, to)) = self.scope.range() else {
            return all.clone();
        };
        match &self.shown {
            Some((id, scope, set)) if *id == all.id() && *scope == self.scope => set.clone(),
            _ => {
                let set = all.taken_between(from, to);
                self.shown = Some((all.id(), self.scope, set.clone()));
                set
            }
        }
    }

    /// The years and months the photos were taken in, counted once for each set of photos.
    fn timeline_of(&mut self, all: &GeoSet) -> Arc<Timeline> {
        match &self.timeline {
            Some((id, timeline)) if *id == all.id() => timeline.clone(),
            _ => {
                let timeline = Arc::new(Timeline::of(all));
                self.timeline = Some((all.id(), timeline.clone()));
                timeline
            }
        }
    }

    /// The stops of the trip (the places in the order they were visited), worked out once for each set.
    fn stops_of(&mut self, set: &GeoSet) -> Arc<[Stop]> {
        match &self.stops {
            Some((id, stops)) if *id == set.id() => stops.clone(),
            _ => {
                let stops: Arc<[Stop]> = travel_stops(set).into();
                self.stops = Some((set.id(), stops.clone()));
                stops
            }
        }
    }

    /// Sends the camera toward `to`: at once when Windows' animation effects are off, or when it is there already.
    fn begin_flight(&mut self, to: MapCamera, cx: &App) {
        self.selected_cluster = None;
        if cx.reduce_motion() || to == self.camera {
            self.camera = to;
            self.fly = None;
        } else {
            self.fly = Some(Fly {
                from: self.camera,
                to,
                at: Instant::now(),
            });
        }
    }

    fn fly_to(&mut self, to: MapCamera, cx: &mut Context<Self>) {
        self.begin_flight(to, cx);
        cx.notify();
    }

    /// The camera flies to the photos of the pin that is chosen, framing them; a pin whose photos are already as
    /// close as the view shows them (one photo, or a few on the same spot) is come in on instead.
    fn fly_into_selected(&mut self, cx: &mut Context<Self>) {
        let Some(chosen) = self.selected_cluster else {
            return;
        };
        let (all, _, _) = self.current(cx);
        let set = self.shown_of(&all);
        let frame = self.clusters_now(&set);
        let Some(pin) = frame.clusters.pins.get(chosen).copied() else {
            return;
        };
        let bounds = self.map_bounds.get();
        let aspect =
            f64::from(f32::from(bounds.size.width).max(800.0)) / f64::from(f32::from(bounds.size.height).max(480.0));
        let mut to = MapCamera::fit_members(&set, frame.clusters.members_of(&pin), aspect);
        if to.zoom < self.camera.zoom * 1.5 {
            to = MapCamera::centered_on(pin.lat, pin.lon, self.camera.zoom * 4.0);
        }
        self.touched = true;
        self.fly_to(to, cx);
    }

    /// The next (or previous) pin of the view is chosen, row by row, round the end.
    fn step_selection(&mut self, forward: bool, cx: &mut Context<Self>) {
        let (all, _, _) = self.current(cx);
        let set = self.shown_of(&all);
        let count = self.clusters_now(&set).clusters.pins.len();
        if count == 0 {
            return;
        }
        self.selected_cluster = Some(match (self.selected_cluster, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
        });
        cx.notify();
    }

    /// The camera that frames all of `set` in the map's box (the default one for no photos).
    fn fit_camera(&self, set: &GeoSet) -> MapCamera {
        if set.is_empty() {
            return MapCamera::default();
        }
        let bounds = self.map_bounds.get();
        let w = f32::from(bounds.size.width).max(800.0);
        let h = f32::from(bounds.size.height).max(480.0);
        MapCamera::fit_set(set, f64::from(w) / f64::from(h))
    }

    fn zoom_center(&mut self, factor: f64, cx: &mut Context<Self>) {
        let bounds = self.map_bounds.get();
        let w = f32::from(bounds.size.width).max(800.0);
        let h = f32::from(bounds.size.height).max(480.0);
        self.camera.zoom_at(factor, w * 0.5, h * 0.5, w, h);
        self.touched = true;
        self.fly = None;
        self.selected_cluster = None;
        cx.notify();
    }

    fn pick_cluster_at(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = self.map_bounds.get();
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        if w <= 1.0 || h <= 1.0 {
            return;
        }
        let (all, _, _) = self.current(cx);
        let set = self.shown_of(&all);
        let frame = self.clusters_now(&set);
        let px_x = f32::from(pos.x - bounds.origin.x);
        let px_y = f32::from(pos.y - bounds.origin.y);
        let mut best: Option<(usize, f32)> = None;
        for (i, cl) in frame.clusters.pins.iter().enumerate() {
            let dx = cl.sx - px_x;
            let dy = cl.sy - px_y;
            let dist_sq = dx * dx + dy * dy;
            let hit_r = pins::hit_radius(cl.count, frame.top.binary_search(&(i as u32)).is_ok());
            if dist_sq <= hit_r * hit_r && best.is_none_or(|(_, d)| dist_sq < d) {
                best = Some((i, dist_sq));
            }
        }
        self.selected_cluster = best.map(|(i, _)| i);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = event.keystroke.modifiers;
        if m.control || m.alt || m.platform {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                cx.stop_propagation();
                if self.selected_cluster.take().is_some() {
                    cx.notify();
                } else {
                    let tab = self.tab_id(cx);
                    set(cx, tab, false);
                }
            }
            "=" | "+" => {
                cx.stop_propagation();
                self.zoom_center(1.6, cx);
            }
            "-" => {
                cx.stop_propagation();
                self.zoom_center(1.0 / 1.6, cx);
            }
            "0" => {
                cx.stop_propagation();
                self.camera = MapCamera::default();
                self.touched = true;
                self.fly = None;
                self.selected_cluster = None;
                cx.notify();
            }
            // On the map itself (not on one of its buttons): Enter flies into the chosen pin, the arrows choose.
            "enter" if self.selected_cluster.is_some() && self.focus.is_focused(window) => {
                cx.stop_propagation();
                self.fly_into_selected(cx);
            }
            key @ ("right" | "down" | "left" | "up") if self.focus.is_focused(window) => {
                cx.stop_propagation();
                self.step_selection(matches!(key, "right" | "down"), cx);
            }
            _ => {}
        }
    }
}

/// What the drawer says it holds: how many photos, and near where: the nearest city of the offline table (and its
/// country beside it), else the coordinates.
fn drawer_title(cl: &PinCluster) -> (SharedString, Option<SharedString>) {
    if let Some((city, country)) = gazetteer().near(cl.lat, cl.lon) {
        let title = if cl.count == 1 {
            t!("1 photo near {place}", place = city)
        } else {
            t!("{n} photos near {place}", n = cl.count, place = city)
        };
        return (title, Some(country.into()));
    }
    let (lat, lon) = (format!("{:.3}", cl.lat), format!("{:.3}", cl.lon));
    let title = if cl.count == 1 {
        t!("1 photo near {lat}, {lon}", lat = lat, lon = lon)
    } else {
        t!("{n} photos near {lat}, {lon}", n = cl.count, lat = lat, lon = lon)
    };
    (title, None)
}

impl Render for PhotoMapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab_id(cx);
        let (all, loading, serial) = self.current(cx);
        // The camera frames the photos when the first come in and once more when the scan is done (so pins are
        // there to see while it runs), unless the person has moved it meanwhile.
        let stage = if loading { 1 } else { 2 };
        let new_load = self.fitted_for.is_none_or(|(was, _)| was != serial);
        if new_load {
            self.touched = false;
            self.scope = Scope::All;
            self.fitted_scope = Scope::All;
            self.fly = None;
        }
        let later = self.fitted_for.is_some_and(|(_, at)| at < stage) && !self.touched;
        if !all.is_empty() && (new_load || later) {
            self.fitted_for = Some((serial, stage));
            self.camera = self.fit_camera(&all);
        }
        // The photos of the time chosen: a new time sends the camera to what is left of them.
        let photos = self.shown_of(&all);
        if self.fitted_scope != self.scope {
            self.fitted_scope = self.scope;
            if !photos.is_empty() {
                let to = self.fit_camera(&photos);
                self.begin_flight(to, cx);
            }
        }
        let timeline = self.timeline_of(&all);
        // A flight in the air: the camera one step on (the curve the app's things move on), and a frame to come.
        if let Some(fly) = self.fly {
            let t = (fly.at.elapsed().as_secs_f32() / FLY.as_secs_f32()).min(1.0);
            if t >= 1.0 {
                self.camera = fly.to;
                self.fly = None;
            } else {
                self.camera = fly.from.blend(&fly.to, f64::from(theme::motion::glide(t)));
                window.request_animation_frame();
            }
        }

        let frame = self.clusters_now(&photos);
        let active = self
            .selected_cluster
            .and_then(|idx| frame.clusters.pins.get(idx).copied());
        let scale = window.scale_factor();
        let members: &[u32] = active.as_ref().map_or(&[][..], |cl| frame.clusters.members_of(cl));
        self.ask_for_pictures(&frame, members, &photos, scale, cx);
        let pictures = self.pin_pictures(&frame, &photos, scale, cx);

        let t = theme::tokens(cx).clone();
        let count_text = if loading && all.is_empty() {
            t!("Looking for photos with a place…")
        } else if loading {
            t!("Looking… {n} photos with a place so far", n = all.len())
        } else if self.scope != Scope::All {
            t!(
                "{n} of {total} photos with a place",
                n = photos.len(),
                total = all.len()
            )
        } else {
            t!("{n} photos with a place", n = photos.len())
        };

        let pane = self.pane;
        let header = panel_header(
            Header {
                close_id: ("pm-close", pane).into(),
                glyph: IconName::Globe,
                title: t!("Photo World Map"),
                subtitle: Some(count_text),
                tools: vec![
                    tool_button(
                        ("pm-fit", pane),
                        IconName::Maximize,
                        t!("Fit all photos"),
                        t!("Fit all photos"),
                        cx.listener(|this, _, _, cx| this.fit_all(cx)),
                    )
                    .into_any_element(),
                    tool_button(
                        ("pm-zoom-in", pane),
                        IconName::ZoomIn,
                        t!("Zoom in"),
                        t!("Zoom in (+)"),
                        cx.listener(|this, _, _, cx| this.zoom_center(1.6, cx)),
                    )
                    .into_any_element(),
                    tool_button(
                        ("pm-zoom-out", pane),
                        IconName::ZoomOut,
                        t!("Zoom out"),
                        t!("Zoom out (-)"),
                        cx.listener(|this, _, _, cx| this.zoom_center(1.0 / 1.6, cx)),
                    )
                    .into_any_element(),
                    tool_button(
                        ("pm-reset", pane),
                        IconName::RotateCcw,
                        t!("Show the whole world"),
                        t!("Show the whole world (0)"),
                        cx.listener(|this, _, _, cx| {
                            this.camera = MapCamera::default();
                            this.touched = true;
                            this.fly = None;
                            this.selected_cluster = None;
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
                ],
                close_name: None,
            },
            move |_, _, cx| set(cx, tab, false),
            &t,
        );

        let cam = self.camera;
        let selected_idx = self.selected_cluster;
        let frame_draw = frame.clone();
        let bounds_cell = self.map_bounds.clone();
        let view_id = cx.entity_id();
        let ocean = t.background;
        let land_fill = t.muted;
        let grid_line = t.border.opacity(0.35);
        let colors = Colors {
            pin: t.primary,
            selected: t.destructive,
            text: t.primary_foreground,
            halo: t.background,
            name: t.muted_foreground,
            arc: t.primary.opacity(0.7),
        };
        let stops = self.arcs.then(|| self.stops_of(&photos));
        let base_font = window.text_style().font();

        let map_canvas = canvas(
            move |bounds, window, _| {
                let prev = bounds_cell.replace(bounds);
                if prev.size != bounds.size {
                    window.on_next_frame(move |_, cx| cx.notify(view_id));
                }
                bounds
            },
            move |_, bounds, window, cx| {
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w <= 2.0 || h <= 2.0 {
                    return;
                }
                window.paint_quad(fill(bounds, ocean));

                // Graticule grid (every 60 deg longitude, every 30 deg latitude)
                for lon in [-120.0, -60.0, 0.0, 60.0, 120.0] {
                    let (wx, _) = project(0.0, lon);
                    let (sx, _) = cam.to_screen(wx, 0.5, w, h);
                    if sx >= 0.0 && sx <= w {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + px(sx), bounds.origin.y),
                                size(px(1.0), bounds.size.height),
                            ),
                            grid_line,
                        ));
                    }
                }
                for lat in [-60.0, -30.0, 0.0, 30.0, 60.0] {
                    let (_, wy) = project(lat, 0.0);
                    let (_, sy) = cam.to_screen(0.5, wy, w, h);
                    if sy >= 0.0 && sy <= h {
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x, bounds.origin.y + px(sy)),
                                size(bounds.size.width, px(1.0)),
                            ),
                            grid_line,
                        ));
                    }
                }

                // Offline simplified landmass polygons
                for ring in LAND_POLYGONS {
                    if ring.len() < 3 {
                        continue;
                    }
                    let mut pb = PathBuilder::fill();
                    let mut started = false;
                    for &(lon, lat) in *ring {
                        let (wx, wy) = project(f64::from(lat), f64::from(lon));
                        let (sx, sy) = cam.to_screen(wx, wy, w, h);
                        let pt = point(bounds.origin.x + px(sx), bounds.origin.y + px(sy));
                        if !started {
                            pb.move_to(pt);
                            started = true;
                        } else {
                            pb.line_to(pt);
                        }
                    }
                    pb.close();
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, land_fill);
                    }
                }

                let ink = Ink {
                    origin: bounds.origin,
                    colors,
                    font: &base_font,
                };
                // The names of cities, once the zoom is deep enough for them.
                pins::paint_names(window, cx, ink, &frame_draw.labels);

                // The trip, as dashed arcs under the pins.
                if let Some(stops) = &stops {
                    pins::paint_arcs(window, bounds, &cam, stops, colors.arc);
                }

                // Clustered pins, with the lead photo's picture in the ones that have it.
                let mut next_picture = pictures.iter().peekable();
                for (i, pin) in frame_draw.clusters.pins.iter().enumerate() {
                    let picture = next_picture.next_if(|p| p.pin as usize == i);
                    pins::paint_pin(window, cx, ink, pin, Some(i) == selected_idx, picture);
                }
            },
        )
        .size_full();

        let map_body = div()
            .id(("photo-map-canvas", self.pane))
            .role(Role::Image)
            .aria_label(t!("Interactive offline world map of geotagged photos"))
            .test_support()
            .relative()
            .size_full()
            .overflow_hidden()
            .cursor_grab()
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                let bounds = this.map_bounds.get();
                let w = f32::from(bounds.size.width);
                let h = f32::from(bounds.size.height);
                if w <= 1.0 || h <= 1.0 {
                    return;
                }
                let dy = f32::from(ev.delta.pixel_delta(px(24.0)).y);
                if dy.abs() < 0.01 {
                    return;
                }
                let factor = if dy > 0.0 { 1.2 } else { 1.0 / 1.2 };
                let sx = f32::from(ev.position.x - bounds.origin.x);
                let sy = f32::from(ev.position.y - bounds.origin.y);
                this.camera.zoom_at(factor, sx, sy, w, h);
                this.touched = true;
                this.fly = None;
                this.selected_cluster = None;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, _| {
                    this.drag = Some(DragState {
                        start: ev.position,
                        last: ev.position,
                        moved: false,
                    });
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                let Some(mut st) = this.drag else {
                    return;
                };
                let dx = f32::from(ev.position.x - st.last.x);
                let dy = f32::from(ev.position.y - st.last.y);
                let dist = ((f32::from(ev.position.x - st.start.x)).powi(2)
                    + (f32::from(ev.position.y - st.start.y)).powi(2))
                .sqrt();
                if dist > 4.0 {
                    st.moved = true;
                }
                st.last = ev.position;
                this.drag = Some(st);
                let w = f32::from(this.map_bounds.get().size.width);
                if w > 1.0 && (dx.abs() > 0.0 || dy.abs() > 0.0) {
                    this.camera.pan_by(dx, dy, w);
                    this.touched = true;
                    this.fly = None;
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                    let was_click = this.drag.is_some_and(|d| !d.moved);
                    this.drag = None;
                    if was_click {
                        this.pick_cluster_at(ev.position, cx);
                        // Twice: the camera flies to the pin's photos.
                        if ev.click_count >= 2 {
                            this.fly_into_selected(cx);
                        }
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.drag = None;
                }),
            )
            .child(map_canvas)
            .when(!loading && all.is_empty(), |box_el| {
                box_el.child(
                    div().absolute().inset_0().flex().items_center().justify_center().child(
                        col()
                            .items_center()
                            .gap(space(1.5))
                            .px(space(4.))
                            .py(space(3.))
                            .rounded(theme::size::RADIUS_LG)
                            .bg(t.background.opacity(0.92))
                            .border_1()
                            .border_color(t.border)
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.foreground)
                                    .child(t!("No photos with a place here")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .child(t!("Pictures that know where they were taken show up as pins.")),
                            ),
                    ),
                )
            });

        let drawer = active.map(|cl| {
            let items = self.drawer_items(frame.clusters.members_of(&cl), &photos, scale, cx);
            let (title, country) = drawer_title(&cl);
            self.drawer(title, country, items, cx)
        });
        // The drawer lies over the bottom of the map, so opening it moves no pin under the pointer.
        let map_area = div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(map_body)
            .when_some(drawer, |area, drawer| area.child(drawer));
        let scrubber = self.scrubber(&timeline, cx);

        col()
            .id(("photo-map-view", self.pane))
            .role(Role::Region)
            .aria_label(t!("Photo World Map"))
            .key_context("PhotoMap")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key_down))
            .size_full()
            .bg(t.background)
            .child(header)
            .child(map_area)
            .when_some(scrubber, |root, scrubber| root.child(scrubber))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::file_grid::thumbnail::{Ask, Fut, Origin, ThumbSource, Thumbnail};
    use crate::explorer::fake::{file, FakeDisk};
    use crate::explorer::NavOptions;
    use crate::testing;
    use gpui_kit::test::{ClickOptions, TestWindowExt as _};
    use gpui_kit::VisualTestContext;
    use std::cell::RefCell;

    #[gpui_kit::test]
    fn renders_offline_world_map_pins_and_toggles_cleanly(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Trip", vec![file("C:\\Trip", "shibuya.jpg", 420, 0)]);

        let sample = vec![
            GeoPhoto {
                path: "C:\\Trip\\shibuya.jpg".into(),
                name: "shibuya.jpg".into(),
                lat: 35.6595,
                lon: 139.7004,
                taken: 1_700_000_000,
                ..Default::default()
            },
            GeoPhoto {
                path: "C:\\Trip\\eiffel.jpg".into(),
                name: "eiffel.jpg".into(),
                lat: 48.8584,
                lon: 2.2945,
                taken: 1_700_000_100,
                ..Default::default()
            },
        ];
        let loader = loader_of(move |_| sample.clone());

        cx.update(|cx| {
            testing::init(cx);
            crate::explorer::init_with(cx, disk.source());
            init_with(cx, loader);
        });

        let (_view, visual) = cx.add_window_view(|window, cx| PhotoMapView::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Trip", NavOptions::default(), cx));
        });
        visual.run_until_parked();

        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        assert!(!visual.update(|_, cx| shows(cx, tab)));

        visual.update(|_, cx| toggle(cx, tab));
        visual.run_until_parked();
        assert!(visual.update(|_, cx| visible(cx, tab)));

        visual.update(|window, cx| window.render_frame(cx));
        let loaded = visual.update(|_, cx| shared(cx).read(cx).state_of(tab).map(|s| (s.loading, s.photos.len())));
        assert_eq!(loaded, Some((false, 2)));

        visual.update(|window, cx| window.click(("pm-close", 0usize), cx));
        visual.run_until_parked();
        assert!(!visual.update(|_, cx| shows(cx, tab)));
    }
    #[gpui_kit::test]
    fn a_scan_that_finds_in_pieces_ends_as_one_set_newest_first(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Trip", vec![file("C:\\Trip", "a.jpg", 420, 0)]);
        // A scan that hands over three batches, the newest photo in the middle one. (The test executor runs the
        // scan on the test's own thread, so a scan that waits between batches can't be watched here: the pieces
        // arriving while a scan runs are covered by `gallery::meta`'s scan test.)
        let loader: GeoLoader = Arc::new(|_, sink| {
            for taken in [5i64, 9, 7] {
                sink(vec![GeoPhoto {
                    path: format!("C:\\Trip\\{taken}.jpg").into(),
                    name: format!("{taken}.jpg").into(),
                    lat: 35.0 + taken as f64,
                    lon: 139.0,
                    taken,
                    ..Default::default()
                }]);
            }
        });
        cx.update(|cx| {
            testing::init(cx);
            crate::explorer::init_with(cx, disk.source());
            init_with(cx, loader);
        });
        let (_view, visual) = cx.add_window_view(|window, cx| PhotoMapView::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Trip", NavOptions::default(), cx));
        });
        visual.run_until_parked();
        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        visual.update(|_, cx| toggle(cx, tab));
        let mut state = None;
        for _ in 0..1_000 {
            visual.run_until_parked();
            state = visual.update(|_, cx| {
                shared(cx)
                    .read(cx)
                    .state_of(tab)
                    .map(|s| (s.loading, s.photos.iter().map(|p| p.taken).collect::<Vec<_>>()))
            });
            if state.as_ref().is_some_and(|(loading, _)| !loading) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(state, Some((false, vec![9, 7, 5])));
        // The camera was fitted to them, not left on the world.
        visual.update(|window, cx| window.render_frame(cx));
    }

    /// A thumbnail source that makes every photo a small blue square, and remembers what it was asked for.
    struct Solid(Rc<RefCell<Vec<Ask>>>);

    impl ThumbSource for Solid {
        fn thumbnail(&self, ask: Ask) -> Fut<Result<Thumbnail, String>> {
            self.0.borrow_mut().push(ask);
            Box::pin(async {
                let pixels = glint_core::thumbs::Pixels {
                    width: 8,
                    height: 8,
                    bgra: [30u8, 60, 200, 255].repeat(64),
                };
                Ok(Thumbnail {
                    pixels: Some(Arc::new(pixels)),
                    lasting: true,
                    from: Origin::Made,
                    ms: 1.0,
                })
            })
        }

        fn cancel(&self, _: &str, _: Option<Vec<String>>, _: u32) {}
    }

    fn photo(name: &str, lat: f64, lon: f64, taken: i64) -> GeoPhoto {
        GeoPhoto {
            path: format!("C:\\Trip\\{name}").into(),
            name: name.into(),
            lat,
            lon,
            taken,
            size: 420,
            modified: 5,
        }
    }

    #[gpui_kit::test]
    fn pins_hold_their_lead_photos_picture_and_the_drawer_shows_thumbnails(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Trip", vec![file("C:\\Trip", "a.jpg", 420, 0)]);
        // Two photos a few metres apart in Tokyo (one pin, the newer is its lead; the other has an Arabic name) and one
        // in Paris.
        let sample = vec![
            photo("a.jpg", 35.660, 139.700, 3),
            photo("صورة.jpg", 35.661, 139.701, 2),
            photo("c.jpg", 48.858, 2.295, 1),
        ];
        let asked = Rc::new(RefCell::new(Vec::new()));
        let source: Rc<dyn ThumbSource> = Rc::new(Solid(asked.clone()));
        cx.update(|cx| {
            testing::init(cx);
            crate::explorer::init_with(cx, disk.source());
            crate::components::file_grid::thumbnail::init_with(cx, source);
            init_with(cx, loader_of(move |_| sample.clone()));
        });
        let (view, visual) = cx.add_window_view(|window, cx| PhotoMapView::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Trip", NavOptions::default(), cx));
        });
        visual.run_until_parked();
        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        visual.update(|_, cx| toggle(cx, tab));
        visual.run_until_parked();
        for _ in 0..2 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
        // The camera has just been fitted: nothing is asked while it may still be moving.
        assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());

        // Once it has rested, the lead photo of each pin is asked for, once, at a small size.
        let mut leads: Vec<String> = Vec::new();
        for _ in 0..200 {
            visual.executor().advance_clock(Duration::from_millis(200));
            visual.run_until_parked();
            visual.update(|window, cx| window.render_frame(cx));
            leads = asked.borrow().iter().map(|a| a.path.clone()).collect();
            if leads.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        leads.sort();
        assert_eq!(leads, ["C:\\Trip\\a.jpg", "C:\\Trip\\c.jpg"]);
        assert!(asked.borrow().iter().all(|a| a.size <= 200 && a.lane == "photo-map-0"));

        // Their pictures come, and the pins hold them.
        let mut pictures = 0;
        for _ in 0..200 {
            visual.run_until_parked();
            visual.update(|window, cx| window.render_frame(cx));
            pictures = visual.update(|window, cx| {
                view.update(cx, |v, cx| {
                    let (set, _, _) = v.current(cx);
                    let frame = v.clusters_now(&set);
                    v.pin_pictures(&frame, &set, window.scale_factor(), cx).len()
                })
            });
            if pictures == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(pictures, 2);

        // The Tokyo pin's drawer: a tile for each photo, named, and asked for too.
        visual.update(|window, cx| {
            view.update(cx, |v, cx| {
                let (set, _, _) = v.current(cx);
                let frame = v.clusters_now(&set);
                v.selected_cluster = frame.clusters.pins.iter().position(|p| p.count == 2);
                cx.notify();
            });
            window.render_frame(cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
        let labels: Vec<Option<String>> = visual.update(|window, _| {
            (0..3usize)
                .map(|i| {
                    window
                        .try_find(("pm-tile", i))
                        .and_then(|t| t.label().map(str::to_string))
                })
                .collect()
        });
        assert_eq!(labels, [Some("a.jpg".to_string()), Some("صورة.jpg".to_string()), None]);
        // The drawer says where, in the words of the offline table: the city, not its coordinates.
        let title = visual.update(|window, _| window.find(("pm-drawer", 0usize)).label().map(str::to_string));
        assert_eq!(title.as_deref(), Some("2 photos near Tokyo"));

        // Pressing a tile, or Enter on it, opens that photo.
        let events = Rc::new(RefCell::new(Vec::new()));
        let log = events.clone();
        let _keep = visual.update(|_, cx| {
            cx.subscribe(&view, move |_, event: &PhotoMapEvent, _| {
                log.borrow_mut().push(event.clone())
            })
        });
        visual.update(|window, cx| window.click(("pm-tile", 0usize), cx));
        visual.update(|window, cx| {
            let handle = view.read(cx).tile_focus[1].clone();
            window.focus(&handle, cx);
            window.press("enter", cx);
        });
        assert_eq!(
            *events.borrow(),
            [
                PhotoMapEvent::Open {
                    path: "C:\\Trip\\a.jpg".into()
                },
                PhotoMapEvent::Open {
                    path: "C:\\Trip\\صورة.jpg".into()
                },
            ]
        );
        // Reveal is a button of its own under each tile.
        visual.update(|window, cx| window.click(("pm-reveal", 1usize), cx));
        assert_eq!(
            events.borrow().last(),
            Some(&PhotoMapEvent::Reveal {
                path: "C:\\Trip\\صورة.jpg".into()
            })
        );
    }

    /// Two photos in Tokyo in January 2019 and one in Paris in 2021.
    fn two_years() -> Vec<GeoPhoto> {
        use glint_model::world_map::year_range;
        let day = 86_400_000;
        let (y19, y21) = (year_range(2019).0, year_range(2021).0);
        vec![
            photo("p1.jpg", 48.858, 2.295, y21 + 100 * day),
            photo("t2.jpg", 35.661, 139.701, y19 + 12 * day),
            photo("t1.jpg", 35.660, 139.700, y19 + 10 * day),
        ]
    }

    #[gpui_kit::test]
    fn the_scrubber_shows_one_year_or_month_and_the_camera_goes_to_what_is_left(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Trip", vec![file("C:\\Trip", "a.jpg", 420, 0)]);
        let sample = two_years();
        cx.update(|cx| {
            testing::init(cx);
            // Windows' animation effects off, as `testing::open_window` has them: a flight lands at once.
            cx.set_reduce_motion(true);
            crate::explorer::init_with(cx, disk.source());
            init_with(cx, loader_of(move |_| sample.clone()));
        });
        let (view, visual) = cx.add_window_view(|window, cx| PhotoMapView::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Trip", NavOptions::default(), cx));
        });
        visual.run_until_parked();
        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        visual.update(|_, cx| toggle(cx, tab));
        visual.run_until_parked();
        for _ in 0..2 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
        let state = |visual: &mut VisualTestContext| {
            visual.update(|_, cx| {
                let v = view.read(cx);
                (v.scope, v.camera.zoom, v.shown.as_ref().map(|(_, _, set)| set.len()))
            })
        };
        let (scope, world_zoom, _) = state(visual);
        assert_eq!(scope, Scope::All);

        // 2019: Tokyo's two photos are all that is left, and the camera is on them.
        visual.update(|window, cx| window.click(("pm-year", 2019usize), cx));
        visual.update(|window, cx| window.render_frame(cx));
        let (scope, zoom, shown) = state(visual);
        assert_eq!((scope, shown), (Scope::Year(2019), Some(2)));
        assert!(zoom > world_zoom * 20.0, "{zoom} against {world_zoom}");
        // Its months: January has the photos, December none (and is not pressable).
        visual.update(|window, cx| window.click(("pm-month", 1usize), cx));
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(state(visual).0, Scope::Month(2019, 1));
        assert_eq!(state(visual).2, Some(2));
        let december = visual.update(|window, _| window.find(("pm-month", 12usize)).disabled());
        assert_eq!(december, Some(true));
        // Pressing January again goes back up to the year, the year again back out, to every photo.
        visual.update(|window, cx| window.click(("pm-month", 1usize), cx));
        assert_eq!(state(visual).0, Scope::Year(2019));
        visual.update(|window, cx| window.click(("pm-year", 2019usize), cx));
        visual.update(|window, cx| window.render_frame(cx));
        let (scope, zoom, _) = state(visual);
        assert_eq!(scope, Scope::All);
        assert!(
            (zoom - world_zoom).abs() < world_zoom * 0.01,
            "{zoom} against {world_zoom}"
        );
        // 2021 alone is the one photo in Paris; All time (the button) leaves it.
        visual.update(|window, cx| window.click(("pm-year", 2021usize), cx));
        visual.update(|window, cx| window.render_frame(cx));
        assert_eq!(state(visual).2, Some(1));
        visual.update(|window, cx| window.click(("pm-all-time", 0usize), cx));
        assert_eq!(state(visual).0, Scope::All);

        // The arcs switch: on, the trip's two stops are worked out; off again.
        visual.update(|window, cx| window.click(("pm-arcs", 0usize), cx));
        visual.update(|window, cx| window.render_frame(cx));
        let stops = visual.update(|_, cx| (view.read(cx).arcs, view.read(cx).stops.as_ref().map(|(_, s)| s.len())));
        assert_eq!(stops, (true, Some(2)));
        visual.update(|window, cx| window.click(("pm-arcs", 0usize), cx));
        assert!(!visual.update(|_, cx| view.read(cx).arcs));
    }

    #[gpui_kit::test]
    fn double_clicking_a_pin_flies_the_camera_to_its_photos(cx: &mut gpui_kit::TestAppContext) {
        use glint_model::world_map::project;
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Trip", vec![file("C:\\Trip", "a.jpg", 420, 0)]);
        let sample = two_years();
        cx.update(|cx| {
            testing::init(cx);
            // Windows' animation effects off, as `testing::open_window` has them: a flight lands at once.
            cx.set_reduce_motion(true);
            crate::explorer::init_with(cx, disk.source());
            init_with(cx, loader_of(move |_| sample.clone()));
        });
        let (view, visual) = cx.add_window_view(|window, cx| PhotoMapView::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Trip", NavOptions::default(), cx));
        });
        visual.run_until_parked();
        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        visual.update(|_, cx| toggle(cx, tab));
        visual.run_until_parked();
        for _ in 0..2 {
            visual.update(|window, cx| window.render_frame(cx));
            visual.run_until_parked();
        }
        let tokyo = project(35.6605, 139.7005);
        let pin_of_tokyo = |visual: &mut VisualTestContext| {
            visual.update(|_, cx| {
                view.update(cx, |v, cx| {
                    let (all, _, _) = v.current(cx);
                    let set = v.shown_of(&all);
                    let frame = v.clusters_now(&set);
                    frame.clusters.pins.iter().find(|p| p.count == 2).map(|p| (p.sx, p.sy))
                })
            })
        };
        let double_click = |visual: &mut VisualTestContext, (sx, sy): (f32, f32)| {
            visual.update(|window, cx| {
                window.click_with_options(
                    ("photo-map-canvas", 0usize),
                    ClickOptions::new().with_offset(point(px(sx), px(sy))).with_count(2),
                    cx,
                )
            })
        };
        let camera =
            |visual: &mut VisualTestContext| visual.update(|_, cx| (view.read(cx).camera, view.read(cx).fly.is_some()));
        let (start, _) = camera(visual);

        // With Windows' animation effects off, the camera is there at once, framing both photos.
        let at = pin_of_tokyo(visual).expect("Tokyo's pin");
        double_click(visual, at);
        let (there, flying) = camera(visual);
        assert!(
            !flying && there.zoom > start.zoom * 20.0,
            "{} against {}",
            there.zoom,
            start.zoom
        );
        assert!((there.cx - tokyo.0).abs() < 1e-3 && (there.cy - tokyo.1).abs() < 1e-3);

        // With them on, it flies: on its way after the double-click, there once the flight's time is up.
        visual.update(|window, cx| {
            view.update(cx, |v, _| v.camera = start);
            cx.set_reduce_motion(false);
            window.render_frame(cx);
        });
        let at = pin_of_tokyo(visual).expect("Tokyo's pin");
        double_click(visual, at);
        let (now, flying) = camera(visual);
        assert!(flying && now.zoom < there.zoom, "{} while flying", now.zoom);
        std::thread::sleep(FLY + Duration::from_millis(60));
        visual.update(|window, cx| window.render_frame(cx));
        let (end, flying) = camera(visual);
        assert!(!flying);
        assert!(
            (end.zoom - there.zoom).abs() < there.zoom * 0.01,
            "{} against {}",
            end.zoom,
            there.zoom
        );

        // Without the pointer: the arrows choose a pin, Enter flies into it.
        visual.update(|window, cx| {
            view.update(cx, |v, _| v.camera = start);
            let focus = view.read(cx).focus_handle();
            window.focus(&focus, cx);
            window.press("right", cx);
        });
        assert!(visual.update(|_, cx| view.read(cx).selected_cluster.is_some()));
        visual.update(|window, cx| window.press("enter", cx));
        let (now, flying) = camera(visual);
        assert!(flying || now.zoom > start.zoom * 2.0);
    }
}
