//! 120 FPS OKLab Color-Wheel Photo Surfing bar.
//!
//! Renders an interactive perceptual OKLab chroma wheel and lightness/tolerance controls above the
//! pane when toggled open, re-ranking matching photos in real time without touching disk or AI.

use crate::explorer::{explorer, Explorer, ExplorerEvent, TabId};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::oklab::{oklab_to_srgb, wheel_point_to_oklab, ColorFilter, Oklab};
use glint_model::paths::same_path;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

struct GlobalState(Entity<ColorWheelModel>);
impl Global for GlobalState {}

/// Tracks which tabs currently have the Color Wheel bar open.
pub struct ColorWheelModel {
    shown: HashSet<TabId>,
    warmed_folder: HashMap<TabId, String>,
    _sub: Subscription,
}

impl ColorWheelModel {
    fn new(cx: &mut Context<Self>) -> Self {
        let exp = explorer(cx);
        let sub = cx.subscribe(&exp, |this, exp, ev: &ExplorerEvent, cx| match ev {
            ExplorerEvent::TabClosed { tab, .. } => {
                let removed = this.shown.remove(tab);
                let dropped = this.warmed_folder.remove(tab).is_some();
                if removed || dropped {
                    cx.notify();
                }
            }
            ExplorerEvent::TabChanged(tab) if this.shown.contains(tab) => {
                let Some(path) = exp.read(cx).tab(*tab).map(|t| t.path.clone()) else {
                    return;
                };
                if this.warmed_folder.get(tab).is_none_or(|prev| !same_path(prev, &path)) {
                    let tab_id = *tab;
                    this.warmed_folder.insert(tab_id, path);
                    // Defer to avoid re-entrantly updating `Explorer` from inside its own notification.
                    cx.defer(move |cx| explorer(cx).update(cx, |e, cx| e.warm_color_palettes(tab_id, cx)));
                }
            }
            _ => {}
        });
        Self {
            shown: HashSet::new(),
            warmed_folder: HashMap::new(),
            _sub: sub,
        }
    }

    pub fn is_shown(&self, tab: TabId) -> bool {
        self.shown.contains(&tab)
    }

    pub fn set_shown(&mut self, tab: TabId, on: bool, folder: Option<String>, cx: &mut Context<Self>) {
        let changed = if on {
            if let Some(path) = folder {
                self.warmed_folder.insert(tab, path);
            }
            self.shown.insert(tab)
        } else {
            let a = self.shown.remove(&tab);
            let b = self.warmed_folder.remove(&tab).is_some();
            a || b
        };
        if changed {
            cx.notify();
        }
    }
}

/// Returns the shared [`ColorWheelModel`], initializing it on demand.
pub fn shared(cx: &mut App) -> Entity<ColorWheelModel> {
    if let Some(state) = cx.try_global::<GlobalState>() {
        return state.0.clone();
    }
    let entity = cx.new(ColorWheelModel::new);
    cx.set_global(GlobalState(entity.clone()));
    entity
}

/// Returns whether the Color Wheel bar is open for `tab`.
pub fn is_shown(cx: &App, tab: TabId) -> bool {
    cx.try_global::<GlobalState>()
        .is_some_and(|s| s.0.read(cx).is_shown(tab))
}

/// Toggles the Color Wheel bar for `tab`, warming image palettes when opened and clearing the
/// filter when closed.
pub fn toggle(cx: &mut App, tab: TabId) {
    let model = shared(cx);
    let next = !model.read(cx).is_shown(tab);
    let exp = explorer(cx);
    let folder = exp.read(cx).tab(tab).map(|t| t.path.clone());
    model.update(cx, |m, cx| m.set_shown(tab, next, folder, cx));
    if next {
        exp.update(cx, |e, cx| e.warm_color_palettes(tab, cx));
    } else {
        exp.update(cx, |e, cx| e.set_color_filter(None, Some(tab), cx));
    }
}

/// Opens the Color Wheel bar for `tab` locked onto `target` OKLab color (from "Find similar colors").
pub fn open_with(cx: &mut App, tab: TabId, target: Oklab) {
    let model = shared(cx);
    let exp = explorer(cx);
    let folder = exp.read(cx).tab(tab).map(|t| t.path.clone());
    model.update(cx, |m, cx| m.set_shown(tab, true, folder, cx));
    exp.update(cx, |e, cx| {
        e.set_color_filter(Some(ColorFilter::new(target)), Some(tab), cx);
    });
}

/// Closes the Color Wheel bar when closing a tab.
pub fn close_tab(cx: &mut App, tab: TabId) {
    if let Some(state) = cx.try_global::<GlobalState>() {
        let model = state.0.clone();
        model.update(cx, |m, cx| m.set_shown(tab, false, None, cx));
    }
}

#[inline]
fn srgb_hsla(rgb: [u8; 3]) -> Hsla {
    gpui_kit::Rgba {
        r: f32::from(rgb[0]) / 255.0,
        g: f32::from(rgb[1]) / 255.0,
        b: f32::from(rgb[2]) / 255.0,
        a: 1.0,
    }
    .into()
}

/// Compact per-pane OKLab Color Wheel toolbar rendered above the pane listing while active.
pub struct ColorWheelBar {
    pane: usize,
    explorer: Entity<Explorer>,
    model: Entity<ColorWheelModel>,
    lightness: f32,
    tolerance: f32,
    dragging: bool,
    wheel_bounds: Rc<Cell<Bounds<Pixels>>>,
    _subs: [Subscription; 2],
}

impl ColorWheelBar {
    pub fn new(pane: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let exp = explorer(cx);
        let model = shared(cx);
        let sub_m = cx.observe_in(&model, window, |_, _, _, cx| cx.notify());
        let sub_e = cx.observe_in(&exp, window, |_, _, _, cx| cx.notify());
        Self {
            pane,
            explorer: exp,
            model,
            lightness: 0.62,
            tolerance: ColorFilter::DEFAULT_TOLERANCE,
            dragging: false,
            wheel_bounds: Rc::new(Cell::new(Bounds::default())),
            _subs: [sub_m, sub_e],
        }
    }

    fn tab_id(&self, cx: &App) -> TabId {
        self.explorer.read(cx).pane_tab_id(self.pane)
    }

    fn apply_point(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let bounds = self.wheel_bounds.get();
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        if w <= 1.0 || h <= 1.0 {
            return;
        }
        let lx = (f32::from(pos.x - bounds.origin.x) / w) * 2.0 - 1.0;
        let ly = (f32::from(pos.y - bounds.origin.y) / h) * 2.0 - 1.0;
        let target = wheel_point_to_oklab(lx, ly, self.lightness);
        let filter = ColorFilter {
            target,
            max_dist: self.tolerance,
        };
        let tab = self.tab_id(cx);
        self.explorer
            .update(cx, |e, cx| e.set_color_filter(Some(filter), Some(tab), cx));
    }

    fn update_params(&mut self, lightness: f32, tolerance: f32, cx: &mut Context<Self>) {
        self.lightness = lightness;
        self.tolerance = tolerance;
        let tab = self.tab_id(cx);
        let current = self.explorer.read(cx).tab(tab).and_then(|t| t.color_filter);
        if let Some(prev) = current {
            let next = ColorFilter {
                target: Oklab {
                    l: lightness,
                    a: prev.target.a,
                    b: prev.target.b,
                },
                max_dist: tolerance,
            };
            self.explorer
                .update(cx, |e, cx| e.set_color_filter(Some(next), Some(tab), cx));
        } else {
            cx.notify();
        }
    }
}

impl Render for ColorWheelBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab_id(cx);
        if !self.model.read(cx).is_shown(tab) {
            return div().into_any_element();
        }
        let (active_filter, matched_count, palette_count, total_files) = {
            let exp = self.explorer.read(cx);
            let Some(t_state) = exp.tab(tab) else {
                return div().into_any_element();
            };
            if t_state.is_this_pc() {
                return div().into_any_element();
            }
            let files = t_state.entries().iter().filter(|e| !e.dir).count();
            (
                t_state.color_filter,
                t_state.rows.len(),
                t_state.color_palettes.len(),
                files,
            )
        };

        let t = theme::tokens(cx);
        let active_lightness = active_filter.map_or(self.lightness, |f| f.target.l);
        let active_tol = active_filter.map_or(self.tolerance, |f| f.max_dist);
        let swatch = active_filter.map(|f| srgb_hsla(oklab_to_srgb(f.target)));
        let wheel_bounds = self.wheel_bounds.clone();
        let target_marker = active_filter.map(|f| (f.target.a, f.target.b));

        let wheel_canvas = canvas(
            move |bounds, _, _| {
                wheel_bounds.set(bounds);
                bounds
            },
            move |_, bounds, window, _| {
                const CELLS: usize = 24;
                let side = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
                if side <= 2.0 {
                    return;
                }
                let cell = side / (CELLS as f32);
                let step_px = cell.ceil();
                for iy in 0..CELLS {
                    for ix in 0..CELLS {
                        let nx = ((ix as f32 + 0.5) / (CELLS as f32)) * 2.0 - 1.0;
                        let ny = ((iy as f32 + 0.5) / (CELLS as f32)) * 2.0 - 1.0;
                        if nx * nx + ny * ny > 1.02 {
                            continue;
                        }
                        let color = srgb_hsla(oklab_to_srgb(wheel_point_to_oklab(nx, ny, active_lightness)));
                        let x = f32::from(bounds.origin.x) + (ix as f32) * cell;
                        let y = f32::from(bounds.origin.y) + (iy as f32) * cell;
                        window.paint_quad(fill(
                            Bounds::new(point(px(x), px(y)), size(px(step_px), px(step_px))),
                            color,
                        ));
                    }
                }
                if let Some((a, b)) = target_marker {
                    const MAX_CHROMA: f32 = 0.32;
                    let nx = (a / MAX_CHROMA).clamp(-1.0, 1.0);
                    let ny = (-b / MAX_CHROMA).clamp(-1.0, 1.0);
                    let cx_px = f32::from(bounds.origin.x) + (nx * 0.5 + 0.5) * side;
                    let cy_px = f32::from(bounds.origin.y) + (ny * 0.5 + 0.5) * side;
                    let ring = Bounds::new(point(px(cx_px - 6.0), px(cy_px - 6.0)), size(px(12.0), px(12.0)));
                    window.paint_quad(quad(
                        ring,
                        6.0,
                        gpui_kit::transparent_black(),
                        Edges::all(px(2.0)),
                        gpui_kit::white(),
                        BorderStyle::Solid,
                    ));
                }
            },
        )
        .size(px(112.));

        let wheel = div()
            .id(("cw-wheel", self.pane))
            .role(Role::Slider)
            .aria_label(t!("OKLab color wheel"))
            .size(px(112.))
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.dragging = true;
                    this.apply_point(ev.position, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.dragging {
                    this.apply_point(ev.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.dragging = false;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.dragging = false;
                }),
            )
            .child(wheel_canvas);

        let lightness_buttons = [
            ("cw-l-dark", t!("Dark"), 0.38f32),
            ("cw-l-mid", t!("Mid"), 0.62f32),
            ("cw-l-light", t!("Light"), 0.82f32),
        ];
        let tolerance_buttons = [
            ("cw-tol-tight", t!("Tight"), 0.14f32),
            ("cw-tol-balanced", t!("Balanced"), 0.22f32),
            ("cw-tol-wide", t!("Wide"), 0.34f32),
        ];

        let status = if active_filter.is_some() {
            t!("{n} matching photos", n = matched_count)
        } else {
            t!(
                "Click or drag on the wheel ({indexed}/{total} palettes ready)",
                indexed = palette_count,
                total = total_files
            )
        };

        row_start()
            .id(("color-wheel-bar", self.pane))
            .w_full()
            .items_center()
            .gap(space(4.))
            .px(space(3.))
            .py(space(2.))
            .bg(t.background)
            .border_b_1()
            .border_color(t.border)
            .child(wheel)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .gap(space(1.5))
                    .child(
                        row_start()
                            .items_center()
                            .justify_between()
                            .gap(space(2.))
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(2.))
                                    .when_some(swatch, |r, fill_color| {
                                        r.child(
                                            div()
                                                .size(px(14.))
                                                .rounded(theme::size::RADIUS_SM)
                                                .border_1()
                                                .border_color(t.border)
                                                .bg(fill_color),
                                        )
                                    })
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.foreground)
                                            .child(t!("Color Wheel")),
                                    )
                                    .child(div().text_xs().text_color(t.muted_foreground).child(status)),
                            )
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(1.))
                                    .when(active_filter.is_some(), |r| {
                                        r.child(
                                            Button::new(("cw-clear", self.pane))
                                                .compact()
                                                .ghost()
                                                .label(t!("Clear"))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.explorer
                                                        .update(cx, |e, cx| e.set_color_filter(None, Some(tab), cx));
                                                })),
                                        )
                                    })
                                    .child(
                                        Button::new(("cw-close", self.pane))
                                            .compact()
                                            .ghost()
                                            .icon(Icon::new(IconName::X))
                                            .accessibility_label(t!("Close Color Wheel"))
                                            .tooltip(t!("Close Color Wheel"))
                                            .on_click(move |_, _, cx| toggle(cx, tab)),
                                    ),
                            ),
                    )
                    .child(
                        row_start()
                            .items_center()
                            .flex_wrap()
                            .gap(space(3.))
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(1.))
                                    .child(div().text_xs().text_color(t.muted_foreground).child(t!("Lightness:")))
                                    .children(lightness_buttons.into_iter().map(|(id_s, label, l_val)| {
                                        let active = (active_lightness - l_val).abs() < 0.08;
                                        Button::new((id_s, self.pane))
                                            .compact()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(label)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.update_params(l_val, active_tol, cx);
                                            }))
                                    })),
                            )
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(1.))
                                    .child(div().text_xs().text_color(t.muted_foreground).child(t!("Match:")))
                                    .children(tolerance_buttons.into_iter().map(|(id_s, label, tol_val)| {
                                        let active = (active_tol - tol_val).abs() < 0.04;
                                        Button::new((id_s, self.pane))
                                            .compact()
                                            .when(active, |b| b.primary())
                                            .when(!active, |b| b.ghost())
                                            .label(label)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.update_params(active_lightness, tol_val, cx);
                                            }))
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{file, FakeDisk};
    use crate::explorer::NavOptions;
    use crate::testing;
    use glint_model::oklab::srgb_to_oklab;

    #[gpui_kit::test]
    fn toggle_and_open_with_track_tab_state_and_filter(cx: &mut gpui_kit::TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Photos", vec![file("C:\\Photos", "sky.jpg", 120, 0)]);

        cx.update(|cx| {
            testing::init(cx);
            crate::explorer::init_with(cx, disk.source());
        });
        let (_view, visual) = cx.add_window_view(|window, cx| ColorWheelBar::new(0, window, cx));
        visual.update(|_, cx| {
            explorer(cx).update(cx, |e, cx| e.navigate("C:\\Photos", NavOptions::default(), cx));
        });
        visual.run_until_parked();

        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        assert!(!visual.update(|_, cx| is_shown(cx, tab)));

        visual.update(|_, cx| toggle(cx, tab));
        assert!(visual.update(|_, cx| is_shown(cx, tab)));

        let cyan = srgb_to_oklab(0, 210, 230);
        visual.update(|_, cx| open_with(cx, tab, cyan));
        assert!(visual.update(|_, cx| is_shown(cx, tab)));
        let filter = visual.update(|_, cx| explorer(cx).read(cx).tab(tab).and_then(|t| t.color_filter));
        assert!(filter.is_some());

        visual.update(|_, cx| toggle(cx, tab));
        assert!(!visual.update(|_, cx| is_shown(cx, tab)));
        let cleared = visual.update(|_, cx| explorer(cx).read(cx).tab(tab).and_then(|t| t.color_filter));
        assert!(cleared.is_none());
    }
}
