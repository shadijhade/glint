//! Offline 120 FPS Vector Photo World Map (`Ctrl+Shift+G` or header globe toggle).
//!
//! Renders embedded continent polygons via GPUI `PathBuilder`, O(N) screen-space pin clustering,
//! smooth cursor-anchored zoom/pan, and a cluster drawer to reveal or open geotagged photos.

use crate::explorer::{explorer, Explorer, ExplorerEvent, TabId};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::paths::{is_folder_path, is_gallery, same_path};
use glint_model::world_map::{cluster_pins, project, GeoPhoto, MapCamera, PinCluster, LAND_POLYGONS};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Debug, Default)]
pub struct TabMapState {
    pub folder: String,
    pub photos: Vec<GeoPhoto>,
    pub loading: bool,
    pub serial: u64,
}

type GeoLoader = std::sync::Arc<dyn Fn(String) -> Vec<GeoPhoto> + Send + Sync>;

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
                photos: Vec::new(),
                loading: true,
                serial,
            },
        );
        cx.notify();

        let loader = self.loader.clone();
        let photos_task = cx.background_executor().spawn(async move { (loader)(folder) });
        cx.spawn(async move |this, cx| {
            let photos = photos_task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(st) = this.tabs.get_mut(&tab) {
                    if st.serial == serial {
                        st.photos = photos;
                        st.loading = false;
                        cx.notify();
                    }
                }
            });
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
    let loader: GeoLoader = std::sync::Arc::new(|folder| glint_core::gallery::geo_photos_in(&folder));
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
    fitted_for: Option<(String, usize)>,
    drag: Option<DragState>,
    selected_cluster: Option<usize>,
    map_bounds: Rc<Cell<Bounds<Pixels>>>,
    _subs: [Subscription; 2],
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
        Self {
            pane,
            explorer,
            model,
            focus: cx.focus_handle(),
            camera: MapCamera::default(),
            fitted_for: None,
            drag: None,
            selected_cluster: None,
            map_bounds: Rc::new(Cell::new(Bounds::default())),
            _subs: [sub_m, sub_e],
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn tab_id(&self, cx: &App) -> TabId {
        self.explorer.read(cx).pane_tab_id(self.pane)
    }

    fn current_photos(&self, cx: &App) -> (String, Vec<GeoPhoto>, bool) {
        let tab = self.tab_id(cx);
        let guard = self.model.read(cx);
        if let Some(st) = guard.state_of(tab) {
            (st.folder.clone(), st.photos.clone(), st.loading)
        } else {
            (String::new(), Vec::new(), false)
        }
    }

    fn fit_all(&mut self, photos: &[GeoPhoto], cx: &mut Context<Self>) {
        if photos.is_empty() {
            self.camera = MapCamera::default();
        } else {
            let bounds = self.map_bounds.get();
            let w = f32::from(bounds.size.width).max(800.0);
            let h = f32::from(bounds.size.height).max(480.0);
            let pts: Vec<(f64, f64)> = photos.iter().map(|p| (p.lat, p.lon)).collect();
            self.camera = MapCamera::fit_points(&pts, f64::from(w) / f64::from(h));
        }
        self.selected_cluster = None;
        cx.notify();
    }

    fn zoom_center(&mut self, factor: f64, cx: &mut Context<Self>) {
        let bounds = self.map_bounds.get();
        let w = f32::from(bounds.size.width).max(800.0);
        let h = f32::from(bounds.size.height).max(480.0);
        self.camera.zoom_at(factor, w * 0.5, h * 0.5, w, h);
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
        let (_, photos, _) = self.current_photos(cx);
        let clusters = cluster_pins(&photos, &self.camera, w, h, 36.0);
        let px_x = f32::from(pos.x - bounds.origin.x);
        let px_y = f32::from(pos.y - bounds.origin.y);
        let mut best: Option<(usize, f32)> = None;
        for (i, cl) in clusters.iter().enumerate() {
            let dx = cl.sx - px_x;
            let dy = cl.sy - px_y;
            let dist_sq = dx * dx + dy * dy;
            let hit_r = if cl.indices.len() > 1 { 20.0 } else { 14.0 };
            if dist_sq <= hit_r * hit_r && best.is_none_or(|(_, d)| dist_sq < d) {
                best = Some((i, dist_sq));
            }
        }
        self.selected_cluster = best.map(|(i, _)| i);
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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
                self.selected_cluster = None;
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for PhotoMapView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab_id(cx);
        let (folder, photos, loading) = self.current_photos(cx);
        if !loading && !photos.is_empty() {
            let key = (folder.clone(), photos.len());
            if self.fitted_for.as_ref() != Some(&key) {
                self.fitted_for = Some(key);
                let pts: Vec<(f64, f64)> = photos.iter().map(|p| (p.lat, p.lon)).collect();
                let bounds = self.map_bounds.get();
                let w = f32::from(bounds.size.width).max(800.0);
                let h = f32::from(bounds.size.height).max(480.0);
                self.camera = MapCamera::fit_points(&pts, f64::from(w) / f64::from(h));
            }
        }

        let bounds_now = self.map_bounds.get();
        let bw = f32::from(bounds_now.size.width).max(1.0);
        let bh = f32::from(bounds_now.size.height).max(1.0);
        let clusters: Vec<PinCluster> = if bw > 2.0 && bh > 2.0 {
            cluster_pins(&photos, &self.camera, bw, bh, 36.0)
        } else {
            Vec::new()
        };
        let active_cluster = self.selected_cluster.and_then(|idx| clusters.get(idx).cloned());

        let t = theme::tokens(cx);
        let count_text = if loading {
            t!("Scanning EXIF GPS…")
        } else {
            t!("{n} geotagged photos", n = photos.len())
        };

        let photos_for_fit = photos.clone();
        let header = row_start()
            .w_full()
            .items_center()
            .justify_between()
            .gap(space(2.))
            .px(space(3.))
            .py(space(2.))
            .bg(t.background)
            .border_b_1()
            .border_color(t.border)
            .child(
                row_start()
                    .items_center()
                    .gap(space(2.))
                    .child(icon(IconName::Globe).size_4().text_color(t.primary))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.foreground)
                            .child(t!("Photo World Map")),
                    )
                    .child(div().text_xs().text_color(t.muted_foreground).child(count_text)),
            )
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.))
                    .child(
                        Button::new(("pm-fit", self.pane))
                            .compact()
                            .ghost()
                            .label(t!("Fit"))
                            .tooltip(t!("Fit all photos"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.fit_all(&photos_for_fit, cx);
                            })),
                    )
                    .child(
                        Button::new(("pm-zoom-in", self.pane))
                            .compact()
                            .ghost()
                            .label(t!("+"))
                            .accessibility_label(t!("Zoom in"))
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_center(1.6, cx))),
                    )
                    .child(
                        Button::new(("pm-zoom-out", self.pane))
                            .compact()
                            .ghost()
                            .label(t!("-"))
                            .accessibility_label(t!("Zoom out"))
                            .on_click(cx.listener(|this, _, _, cx| this.zoom_center(1.0 / 1.6, cx))),
                    )
                    .child(
                        Button::new(("pm-reset", self.pane))
                            .compact()
                            .ghost()
                            .label(t!("Reset"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.camera = MapCamera::default();
                                this.selected_cluster = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("pm-close", self.pane))
                            .compact()
                            .ghost()
                            .icon(Icon::new(IconName::X))
                            .label(t!("Back to files"))
                            .on_click(move |_, _, cx| set(cx, tab, false)),
                    ),
            );

        let cam = self.camera;
        let selected_idx = self.selected_cluster;
        let photos_draw = photos.clone();
        let bounds_cell = self.map_bounds.clone();
        let view_id = cx.entity_id();
        let ocean = t.background;
        let land_fill = t.muted;
        let grid_line = t.border.opacity(0.35);
        let pin_fill = t.primary;
        let pin_selected = t.destructive;
        let pin_text = t.primary_foreground;
        let pin_halo = t.background;
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

                // Clustered pins
                let clusters_draw = cluster_pins(&photos_draw, &cam, w, h, 36.0);
                for (i, cl) in clusters_draw.iter().enumerate() {
                    let count = cl.indices.len();
                    let radius = if count > 1 { 12.0 } else { 6.5 };
                    let x = f32::from(bounds.origin.x) + cl.sx - radius;
                    let y = f32::from(bounds.origin.y) + cl.sy - radius;
                    let pin_bounds = Bounds::new(point(px(x), px(y)), size(px(radius * 2.0), px(radius * 2.0)));
                    let fill_col = if Some(i) == selected_idx {
                        pin_selected
                    } else {
                        pin_fill
                    };
                    window.paint_quad(quad(
                        pin_bounds,
                        radius,
                        fill_col,
                        Edges::all(px(1.5)),
                        pin_halo,
                        BorderStyle::Solid,
                    ));

                    if count > 1 {
                        let label = count.to_string();
                        let run = TextRun {
                            len: label.len(),
                            font: Font {
                                weight: FontWeight::SEMIBOLD,
                                ..base_font.clone()
                            },
                            color: pin_text,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let shaped = window
                            .text_system()
                            .shape_line(SharedString::from(label), px(10.0), &[run], None);
                        let tw = f32::from(shaped.width());
                        let lx = f32::from(bounds.origin.x) + cl.sx - tw * 0.5;
                        let ly = f32::from(bounds.origin.y) + cl.sy - 6.5;
                        let _ = shaped.paint(point(px(lx), px(ly)), px(13.0), TextAlign::Left, None, window, cx);
                    }
                }
            },
        )
        .size_full();

        let map_body = div()
            .id(("photo-map-canvas", self.pane))
            .role(Role::Image)
            .aria_label(t!("Interactive offline world map of geotagged photos"))
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
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
            .when(!loading && photos.is_empty(), |box_el| {
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
                                    .child(t!("No geotagged photos in this folder")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.muted_foreground)
                                    .child(t!("Photos with EXIF GPS location coordinates appear as pins here.")),
                            ),
                    ),
                )
            });

        let drawer = active_cluster.map(|cl| {
            let pane_idx = self.pane;
            let items: Vec<(usize, GeoPhoto)> = cl
                .indices
                .iter()
                .take(32)
                .filter_map(|&idx| photos.get(idx).cloned().map(|p| (idx, p)))
                .collect();
            col()
                .w_full()
                .max_h(px(176.))
                .bg(t.background)
                .border_t_1()
                .border_color(t.border)
                .p(space(2.))
                .gap(space(1.))
                .child(
                    row_start()
                        .items_center()
                        .justify_between()
                        .px(space(1.))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.foreground)
                                .child(t!(
                                    "{n} photos near {lat}, {lon}",
                                    n = cl.indices.len(),
                                    lat = format!("{:.3}", cl.lat),
                                    lon = format!("{:.3}", cl.lon)
                                )),
                        )
                        .child(
                            Button::new(("pm-drawer-close", pane_idx))
                                .compact()
                                .ghost()
                                .icon(Icon::new(IconName::X))
                                .accessibility_label(t!("Close selection"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.selected_cluster = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .id(("pm-drawer-list", pane_idx))
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(col().gap(space(0.5)).children(items.into_iter().map(|(row_i, photo)| {
                            let reveal_path = photo.path.clone();
                            let open_path = photo.path.clone();
                            row_start()
                                .items_center()
                                .justify_between()
                                .gap(space(2.))
                                .px(space(2.))
                                .py(space(1.))
                                .rounded(theme::size::RADIUS_SM)
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(t.foreground)
                                        .child(isolate(&photo.name)),
                                )
                                .child(
                                    row_start()
                                        .items_center()
                                        .gap(space(1.))
                                        .child(
                                            Button::new(("pm-reveal", pane_idx * 10_000 + row_i))
                                                .compact()
                                                .ghost()
                                                .label(t!("Reveal"))
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(PhotoMapEvent::Reveal {
                                                        path: reveal_path.clone(),
                                                    });
                                                })),
                                        )
                                        .child(
                                            Button::new(("pm-open", pane_idx * 10_000 + row_i))
                                                .compact()
                                                .ghost()
                                                .label(t!("Open"))
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    cx.emit(PhotoMapEvent::Open {
                                                        path: open_path.clone(),
                                                    });
                                                })),
                                        ),
                                )
                        }))),
                )
        });

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
            .child(map_body)
            .when_some(drawer, |root, d| root.child(d))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, FakeDisk};
    use crate::explorer::NavOptions;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;

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
            },
            GeoPhoto {
                path: "C:\\Trip\\eiffel.jpg".into(),
                name: "eiffel.jpg".into(),
                lat: 48.8584,
                lon: 2.2945,
                taken: 1_700_000_100,
            },
        ];
        let loader: GeoLoader = std::sync::Arc::new(move |_| sample.clone());

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
}
