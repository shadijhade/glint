//! Drawing the disk usage map's view: the top bar (name and size, the color modes, Free up space, the legend),
//! the growth strip, the map itself on a canvas, what it says while it works or when it can't, and the hover
//! tooltip. Ported from `UsageMapView`, `GrowthStrip`, `Tooltip` and `Elapsed` in
//! src/components/explorer/DiskUsage.tsx.

use super::colors::ColorMode;
use super::model::Run;
use super::plan::{self, Overlay, FOLDERS};
use super::words::{self, Style};
use super::{DiskUsage, DiskUsageEvent, Drawn};
use crate::actions;
use crate::explorer::TabId;
use crate::kit::prelude::*;
use crate::{state, t, theme};
use glint_model::format::format_size;
use glint_model::usage::growth_leaders;
use gpui_kit::{anchored, canvas, deferred, rems, Anchor, Hsla};
use std::time::Duration;

/// The seconds in "Measuring … 12 s" tick this often.
const TICK: Duration = Duration::from_millis(500);

fn centered(t: &theme::Tokens, id: &'static str) -> Stateful<Div> {
    col()
        .id(id)
        .size_full()
        .items_center()
        .justify_center()
        .gap(space(2.))
        .p(space(6.))
        .text_size(theme::size::SM)
        .text_color(t.muted_foreground)
        .text_center()
}

impl DiskUsage {
    /// Redraws every half second while pdu measures, so "Measuring … 12 s" counts (`Elapsed`).
    fn ensure_ticker(&mut self, running: bool, cx: &mut Context<Self>) {
        if !running {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }));
    }

    /// The segmented "Color map by" picker.
    fn mode_picker(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let modes = [
            (
                ColorMode::Type,
                IconName::Layers,
                t!("Type"),
                t!("Color tiles by file type (T)"),
            ),
            (
                ColorMode::Age,
                IconName::Flame,
                t!("Age"),
                t!("Color tiles by when they last changed, from today (hot) to over a year ago (frozen) (T)"),
            ),
            (
                ColorMode::Growth,
                IconName::TrendingUp,
                t!("Growth"),
                t!("Color tiles by what grew in the last day, week or month, from the instant search index's daily marks (T)"),
            ),
        ];
        row_start()
            .id("disk-usage-modes")
            .role(Role::RadioGroup)
            .aria_label(t!("Color map by"))
            .test_support()
            .flex_none()
            .p(space(0.5))
            .gap(space(0.5))
            .rounded(theme::size::RADIUS)
            .bg(t.foreground.opacity(0.05))
            .children(modes.into_iter().enumerate().map(|(i, (mode, glyph, label, tip))| {
                let chosen = self.mode == mode;
                Button::new(("disk-usage-mode", i))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if chosen { t.background } else { t.background.opacity(0.) })
                            .foreground(if chosen { t.primary } else { t.muted_foreground })
                            .hover(if chosen { t.background } else { t.accent })
                            .active(if chosen { t.background } else { t.accent }),
                    )
                    .role(Role::RadioButton)
                    .accessibility_label(label.clone())
                    .toggled(chosen)
                    .icon(glyph)
                    .label(label)
                    .tooltip(tip)
                    .xsmall()
                    .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
            }))
            .into_any_element()
    }

    /// One legend entry: a swatch and a name. Hovering lights its tiles on the map; clicking keeps them lit.
    #[allow(clippy::too_many_arguments)]
    fn legend_entry(
        &self,
        slot: usize,
        spot: &'static str,
        name: SharedString,
        swatch: Hsla,
        tip: SharedString,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let pinned = self.pinned_spot.as_deref() == Some(spot);
        let active = self.hover_spot.as_deref().or(self.pinned_spot.as_deref()) == Some(spot);
        let (color, foreground) = if pinned {
            (t.primary.opacity(0.15), t.primary)
        } else if active {
            (t.accent, t.foreground)
        } else {
            (t.accent.opacity(0.), t.muted_foreground)
        };
        // The wrapper owns the hover and the tooltip: a button with a tooltip of its own can't also listen to hover.
        div()
            .id(("disk-usage-legend-entry", slot))
            .flex_none()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hover_spot = Some(spot.into());
                } else if this.hover_spot.as_deref() == Some(spot) {
                    this.hover_spot = None;
                }
                cx.notify();
            }))
            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
            .child(
                Button::new(("disk-usage-legend", slot))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(color)
                            .foreground(foreground)
                            .hover(t.accent)
                            .active(t.accent),
                    )
                    .compact()
                    .role(Role::Button)
                    .accessibility_label(name.clone())
                    .toggled(pinned)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pinned_spot = if this.pinned_spot.as_deref() == Some(spot) {
                            None
                        } else {
                            Some(spot.into())
                        };
                        cx.notify();
                    }))
                    .child(
                        row_start()
                            .items_center()
                            .gap(space(1.5))
                            .text_size(theme::size::XXS)
                            .child(
                                div()
                                    .size(space(2.5))
                                    .flex_none()
                                    .rounded(theme::size::RADIUS_SM / 2.)
                                    .bg(swatch),
                            )
                            .child(name),
                    ),
            )
    }

    /// The legend: bytes per kind, age band or growth band, then "Folders" when some are drawn whole.
    fn legend(&self, drawn: Option<&Drawn>, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let Some(drawn) = drawn else {
            return div().into_any_element();
        };
        let plan = &drawn.plan;
        let colors = &plan.colors;
        let things = if plan.growing { t!("tiles") } else { t!("files") };
        let mut entries: Vec<Stateful<Div>> = plan
            .legend
            .iter()
            .enumerate()
            .map(|(i, (group, bytes))| {
                let name = t!(group.name);
                let tip = t!(
                    "{name}: {size} among the {things} shown — hover or click to spotlight on map",
                    name = name.clone(),
                    size = format_size(*bytes as u64),
                    things = things.clone()
                );
                self.legend_entry(i, group.name, name, colors.band(group), tip, t, cx)
            })
            .collect();
        if plan.whole_folders {
            entries.push(self.legend_entry(
                entries.len(),
                FOLDERS,
                t!("Folders"),
                colors.folder,
                t!("Folders too deep or too small to show what's in them — hover or click to spotlight on map"),
                t,
                cx,
            ));
        }
        // What doesn't fit wraps onto a second line, out of sight, rather than being cut.
        row_start()
            .id("disk-usage-legend")
            .role(Role::Group)
            .aria_label(t!("Legend"))
            .test_support()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex_wrap()
            .justify_end()
            .items_center()
            .gap_x(space(1.))
            .overflow_hidden()
            .children(entries)
            .into_any_element()
    }

    /// Above the map in Growth colors: the time span, and where the growth went (`GrowthStrip`).
    fn growth_strip(&self, run: &Run, title: &str, t: &theme::Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let shown = run.current()?;
        let map = &shown.map;
        let days = state::settings(cx).growth_days;
        let strip = || {
            row_start()
                .id("disk-usage-growth")
                .role(Role::Group)
                .aria_label(t!("What grew"))
                .test_support()
                .h(space(7.))
                .flex_none()
                .items_center()
                .gap(space(2.))
                .px(space(3.))
                .overflow_hidden()
                .border_b_1()
                .border_color(t.border)
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
        };
        let Some(growth) = map.growth.as_ref() else {
            let index_on = self.explorer.read(cx).search_index().is_some_and(|i| i.on);
            let (why, action): (SharedString, Option<SharedString>) = match map.engine {
                "index" => (
                    t!("The instant search service running is too old to keep daily marks of what grew."),
                    Some(t!("Update instant search")),
                ),
                "pdu" if index_on => (
                    t!("What grew comes from instant search, which doesn't cover this drive."),
                    None,
                ),
                "pdu" => (
                    t!("What grew comes from instant search's daily marks."),
                    Some(t!("Turn on instant search")),
                ),
                _ => (t!("Nothing grows inside an archive."), None),
            };
            return Some(
                strip()
                    .child(div().min_w_0().truncate().child(why))
                    .children(action.map(|label| {
                        Button::new("disk-usage-growth-enable")
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(t.primary.opacity(0.1))
                                    .foreground(t.primary)
                                    .hover(t.primary.opacity(0.15))
                                    .active(t.primary.opacity(0.2)),
                            )
                            .compact()
                            .icon(IconName::Sparkles)
                            .label(label)
                            .tooltip(t!("Windows asks for administrator approval"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DiskUsageEvent::EnableInstantSearch)))
                    }))
                    .into_any_element(),
            );
        };

        let usage = self.usage.clone();
        let (tab, path) = (self.tab(cx), run.path.clone());
        let day_buttons = row_start()
            .id("disk-usage-growth-days")
            .role(Role::RadioGroup)
            .aria_label(t!("What grew in the last"))
            .test_support()
            .flex_none()
            .p(space(0.5))
            .gap(space(0.5))
            .rounded(theme::size::RADIUS)
            .bg(t.foreground.opacity(0.05))
            .children([1u32, 7, 30].into_iter().map(|d| {
                let chosen = days == d;
                let label = if d == 1 { t!("1 day") } else { t!("{n} days", n = d) };
                let (usage, path) = (usage.clone(), path.clone());
                Button::new(("disk-usage-days", d as usize))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if chosen { t.background } else { t.background.opacity(0.) })
                            .foreground(if chosen { t.primary } else { t.muted_foreground })
                            .hover(if chosen { t.background } else { t.accent })
                            .active(if chosen { t.background } else { t.accent }),
                    )
                    .role(Role::RadioButton)
                    .accessibility_label(label.clone())
                    .toggled(chosen)
                    .label(label)
                    .xsmall()
                    .on_click(move |_, _, cx| {
                        state::update(cx, |settings| settings.growth_days = d);
                        usage.update(cx, |usage, cx| usage.map_path(tab, &path, false, cx));
                    })
            }));

        let grew = glint_model::usage::grew_by(map, 0);
        let headline = words::growth_headline(title, grew, growth.since);
        // The marks don't reach back as far as asked: the oldest is shown.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0., |d| d.as_secs_f64() * 1000.);
        let shorter = growth.since == growth.oldest && growth.oldest > now - (f64::from(days) - 0.5) * 86_400_000.0;
        let root = map.path.trim_end_matches('\\');
        let at_root = root.chars().count() <= 2;
        let drive: String = map.path.chars().take(2).collect();
        let pieces = growth_leaders(map, 3).into_iter().map(|i| {
            let change = words::signed(glint_model::usage::grew_by(map, i));
            let tip = format!("{}: {change}", map.paths.get(i).cloned().unwrap_or_default());
            div()
                .id(("disk-usage-leader", i))
                .flex_none()
                .tooltip(move |window, cx| crate::kit::tooltip(tip.clone().into(), window, cx))
                .child(format!(
                    "· {} {change}",
                    words::relative_to(&map.path, map.paths.get(i).map_or("", String::as_str))
                ))
        });
        let drive_piece = (!at_root).then(|| {
            let tip = t!(
                "The whole drive: {then} then, {now} now",
                then = format_size(growth.drive_then as u64),
                now = format_size(growth.drive_now as u64)
            );
            div()
                .id("disk-usage-drive")
                .flex_none()
                .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                .child(format!(
                    "· {drive} {}",
                    words::signed(growth.drive_now - growth.drive_then)
                ))
        });
        Some(
            strip()
                .child(day_buttons)
                .child(
                    row_start()
                        .min_w_0()
                        .flex_wrap()
                        .h_full()
                        .items_center()
                        .gap_x(space(2.))
                        .overflow_hidden()
                        .child(
                            div()
                                .flex_none()
                                .text_color(t.foreground)
                                .child(headline)
                                .children(shorter.then(|| {
                                    div()
                                        .ms(1.)
                                        .text_color(t.muted_foreground)
                                        .child(t!("(as far back as the marks go)"))
                                })),
                        )
                        .children(pieces)
                        .children(drive_piece),
                )
                .into_any_element(),
        )
    }

    /// What the body says instead of the map: working, failed, stopped, or nothing to show (`body`).
    fn state_body(&self, run: &Run, title: &str, t: &theme::Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let current = run.current();
        let tab = self.tab(cx);
        let usage = self.usage.clone();
        let title_text = crate::kit::isolate(title.to_string());
        if current.is_none() && run.running && (run.engine == "pdu" || run.shown.is_none()) {
            let busy = centered(t, "disk-usage-working")
                .role(Role::Status)
                .aria_label(if run.engine == "pdu" {
                    t!("Measuring “{title}” with pdu…", title = title)
                } else {
                    t!("Mapping “{title}”…", title = title)
                })
                .test_support()
                .child(Spinner::new().color(t.primary));
            let busy = if run.engine != "pdu" {
                busy.child(div().child(t!("Mapping “{title}”…", title = title_text)))
            } else {
                let seconds = run.started.elapsed().as_secs();
                busy.child(
                    row_start()
                        .gap(space(1.))
                        .text_color(t.foreground)
                        .child(t!("Measuring “{title}” with pdu…", title = title_text))
                        .child(div().child(t!("{n} s", n = seconds))),
                )
                .child(div().max_w(rems(24.)).text_size(theme::size::XS).child(t!(
                    "Big folders take a while. With instant search on (in Settings), folders on indexed drives are mapped in milliseconds."
                )))
                .child(
                    Button::new("disk-usage-stop")
                        .outline()
                        .small()
                        .icon(IconName::Square)
                        .label(t!("Stop"))
                        .on_click(move |_, _, cx| usage.update(cx, |usage, cx| usage.stop(tab, cx))),
                )
            };
            return Some(busy.into_any_element());
        }
        if current.is_none() && !run.error.is_empty() {
            let path = run.path.clone();
            return Some(
                centered(t, "disk-usage-failed")
                    .role(Role::Alert)
                    .aria_label(t!("Couldn't map “{title}”", title = title))
                    .test_support()
                    .child(icon(IconName::TriangleAlert).size(rems(1.5)).text_color(t.destructive))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .child(t!("Couldn't map “{title}”", title = title_text)),
                    )
                    .child(div().max_w(rems(28.)).child(run.error.clone()))
                    .child(
                        Button::new("disk-usage-retry")
                            .outline()
                            .small()
                            .label(t!("Try again"))
                            .on_click(move |_, _, cx| {
                                usage.update(cx, |usage, cx| usage.map_path(tab, &path, false, cx))
                            }),
                    )
                    .into_any_element(),
            );
        }
        if current.is_none() && !run.running {
            let path = run.path.clone();
            return Some(
                centered(t, "disk-usage-stopped")
                    .role(Role::Status)
                    .aria_label(t!("Measuring stopped."))
                    .test_support()
                    .child(div().child(t!("Measuring stopped.")))
                    .child(
                        Button::new("disk-usage-measure-again")
                            .outline()
                            .small()
                            .label(t!("Measure again"))
                            .on_click(move |_, _, cx| {
                                usage.update(cx, |usage, cx| usage.map_path(tab, &path, false, cx))
                            }),
                    )
                    .into_any_element(),
            );
        }
        if let Some(shown) = current.filter(|s| s.map.count <= 1) {
            let text = if shown.map.size.first().copied().unwrap_or(0.) > 0. {
                t!("Nothing in this folder is big enough to show.")
            } else {
                t!("This folder is empty.")
            };
            return Some(
                centered(t, "disk-usage-empty")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .child(div().child(text))
                    .into_any_element(),
            );
        }
        None
    }

    /// The tooltip by the pointer: what the node is, where, how big, and what a click does.
    fn tooltip(&self, lines: Vec<words::Line>, at: Point<Pixels>, t: &theme::Tokens) -> AnyElement {
        let rtl = crate::i18n::is_rtl();
        let label = lines.iter().map(|l| l.text.to_string()).collect::<Vec<_>>().join(". ");
        let (anchor, dx) = if rtl {
            (Anchor::TopRight, -16.)
        } else {
            (Anchor::TopLeft, 16.)
        };
        deferred(
            anchored()
                .position(at)
                .anchor(anchor)
                .offset(point(px(dx), px(18.)))
                .snap_to_window()
                .child(
                    col()
                        .id("disk-usage-tooltip")
                        .role(Role::Tooltip)
                        .aria_label(label)
                        .test_support()
                        .align_start()
                        .max_w(rems(28.))
                        .px(space(2.5))
                        .py(space(1.5))
                        .rounded(theme::size::RADIUS)
                        .border_1()
                        .border_color(t.border)
                        .bg(t.popover)
                        .text_color(t.popover_foreground)
                        .text_size(theme::size::XS)
                        .shadow_lg()
                        .children(lines.into_iter().map(|line| {
                            let base = div().max_w_full().truncate().text_start();
                            let text = crate::kit::isolate(line.text);
                            match line.style {
                                Style::Title => base.font_weight(FontWeight::MEDIUM).child(text),
                                Style::Muted => base.text_color(t.muted_foreground).child(text),
                                Style::Plain => base.child(text),
                            }
                        })),
                ),
        )
        .with_priority(2)
        .into_any_element()
    }
}

impl Render for DiskUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let tab: TabId = self.tab(cx);
        let Some(run) = self.usage.read(cx).run(tab).cloned() else {
            return div().id(("disk-usage", self.pane)).size_full().into_any_element();
        };
        let title = self.explorer.read(cx).tab_name(&run.path);

        // The spotlit legend entries belong to one mode and folder.
        let spot_for = Some((self.mode, run.path.clone()));
        if self.spot_for != spot_for {
            self.spot_for = spot_for;
            self.pinned_spot = None;
            self.hover_spot = None;
        }
        // Another tab's map is not this one's to keep drawn.
        if self
            .drawn
            .as_ref()
            .is_some_and(|d| run.shown.is_none() && d.shown.map.path != run.path)
        {
            self.drawn = None;
        }
        match run.shown.clone() {
            Some(shown) => self.ensure_plan(&shown, cx),
            None => {
                self.drawn = None;
                self.building = None;
            }
        }
        self.ensure_ticker(run.running && run.engine == "pdu", cx);

        let current = run.current().cloned();
        let state = self.state_body(&run, &title, &t, cx);
        let live = self.live(cx).is_some();
        let hover = if live && state.is_none() {
            self.pointer.and_then(|p| self.hit_at(p, cx))
        } else {
            None
        };

        // ---- The top bar -----------------------------------------------------------------------------------
        let mut bar = row_start()
            .id("disk-usage-bar")
            .role(Role::Group)
            .aria_label(t!("Disk usage"))
            .test_support()
            .h(space(8.))
            .flex_none()
            .items_center()
            .gap(space(3.))
            .px(space(3.))
            .overflow_hidden()
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(
                row_start()
                    .flex_none()
                    .items_center()
                    .gap(space(1.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.foreground)
                    .child(crate::kit::isolate(title.clone()))
                    .children(current.as_ref().map(|shown| {
                        div()
                            .font_weight(FontWeight::NORMAL)
                            .text_color(t.muted_foreground)
                            .child(format_size(shown.map.size.first().copied().unwrap_or(0.) as u64))
                    }))
                    .children((run.running && current.is_some()).then(|| Spinner::new().xsmall().color(t.primary))),
            )
            .child(self.mode_picker(&t, cx));
        if let Some(shown) = current.as_ref().filter(|s| matches!(s.map.engine, "index" | "pdu")) {
            let folder = run.path.clone();
            let _ = shown;
            bar = bar.child(
                Button::new("disk-usage-free-up")
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(t.accent.opacity(0.))
                            .foreground(t.muted_foreground)
                            .hover(t.accent)
                            .active(t.accent),
                    )
                    .compact()
                    .icon(IconName::BrushCleaning)
                    .label(t!("Free up space"))
                    .tooltip(t!("Clean up: build output and package caches below this folder that their projects' tools make again"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(DiskUsageEvent::FreeUpSpace { folder: folder.clone() })
                    })),
            );
        }
        if current.as_ref().is_some_and(|s| s.map.service_too_old) {
            bar = bar.child(
                Button::new("disk-usage-update-service")
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(t.primary.opacity(0.1))
                            .foreground(t.primary)
                            .hover(t.primary.opacity(0.15))
                            .active(t.primary.opacity(0.2)),
                    )
                    .compact()
                    .icon(IconName::Sparkles)
                    .label(t!("Update instant search"))
                    .tooltip(t!("pdu measured this map: the instant search service running is a version from before maps. Updated (Windows asks for administrator approval), it maps folders on indexed drives in milliseconds."))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DiskUsageEvent::EnableInstantSearch))),
            );
        }
        let bar = bar.child(self.legend(self.drawn.as_ref().filter(|_| live || run.shown.is_some()), &t, cx));

        let strip = (self.mode == ColorMode::Growth)
            .then(|| self.growth_strip(&run, &title, &t, cx))
            .flatten();

        // ---- The map ---------------------------------------------------------------------------------------
        let map_data = self.drawn.as_ref().filter(|_| state.is_none()).map(|drawn| {
            let tab_selection = self.explorer.read(cx).tab(tab);
            let selected: Vec<usize> = tab_selection
                .map(|tab| tab.selection.iter().filter_map(|p| drawn.shown.node_of(p)).collect())
                .unwrap_or_default();
            let spot = self.hover_spot.clone().or_else(|| self.pinned_spot.clone());
            let spotlight = spot.map(|name| drawn.plan.spotlight(&drawn.shown, &name));
            (drawn.plan.clone(), drawn.shown.clone(), selected, spotlight)
        });
        let body_bounds = self.body.clone();
        let labels = self.labels.clone();
        let hovered = hover.map(|h| (h.node, h.rest));
        let id = cx.entity_id();
        let surface = canvas(
            move |bounds, window, _| {
                let before = body_bounds.replace(bounds);
                if before.size != bounds.size {
                    // Not `cx.notify` here: GPUI drops a redraw asked for during a draw. After this frame, it counts.
                    window.on_next_frame(move |_, cx| cx.notify(id));
                }
                bounds
            },
            move |_, bounds, window, cx| {
                if let Some((plan, shown, selected, spotlight)) = map_data {
                    let overlay = Overlay {
                        selected: &selected,
                        hover: hovered,
                        spotlight: spotlight.as_deref(),
                    };
                    plan::paint(&plan, &shown, &overlay, &mut labels.borrow_mut(), bounds, window, cx);
                }
            },
        )
        .absolute()
        .size_full();

        let tip = hover.and_then(|h| {
            let drawn = self.drawn.as_ref()?;
            let lines = words::tooltip_lines(
                &drawn.shown.map,
                drawn.shown.entry(h.node),
                h.node,
                h.rest,
                drawn.plan.growing,
            );
            Some(self.tooltip(lines, self.pointer?, &t))
        });

        let dimmed = !live && state.is_none() && self.drawn.is_some();
        let map_label = words::map_label(&title, current.as_ref().map(|s| &s.map));
        let body = div()
            .id(("disk-usage-map", self.pane))
            .role(Role::Image)
            .aria_label(map_label)
            .test_support()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .bg(t.background)
            .cursor_default()
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus, cx);
                    this.take_pane(cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| this.pointer_moved(event.position, cx)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered {
                    this.pointer_left(cx);
                }
            }))
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if !event.is_keyboard() && !event.is_right_click() && !event.is_middle_click() {
                    this.clicked(event.position(), event.click_count(), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.context_menu(event.position, cx);
                }),
            )
            // Files dragged in from File Explorer or another pane drop into the folder mapped.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| this.files_over(event, cx)))
            .on_drop(cx.listener(|this, drop: &ExternalPaths, _, cx| this.files_dropped(drop, cx)))
            // While the next map comes, the last one stays, dimmed.
            .child(
                div()
                    .absolute()
                    .size_full()
                    .when(dimmed, |this| this.opacity(0.4))
                    .child(surface),
            )
            .children(state.map(|state| div().absolute().inset_0().bg(t.background).child(state)))
            .children(self.drop_over.then(|| {
                div()
                    .id("disk-usage-drop")
                    .test_support()
                    .absolute()
                    .inset_0()
                    .border_2()
                    .border_dashed()
                    .border_color(t.primary.opacity(0.6))
                    .bg(t.primary.opacity(0.05))
            }))
            .children(tip);

        col()
            .id(("disk-usage", self.pane))
            .role(Role::Group)
            .aria_label(t!("Disk usage"))
            .test_support()
            .key_context("FileList")
            .size_full()
            .min_h_0()
            .bg(t.background)
            .text_start()
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .on_action(cx.listener(|this, _: &actions::OpenItem, _, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &actions::ShowItemMenu, _, cx| this.keyboard_menu(cx)))
            // Nothing to select all of.
            .on_action(cx.listener(|_, _: &actions::SelectAll, _, _| {}))
            .child(bar)
            .children(strip)
            .child(body)
            .into_any_element()
    }
}
