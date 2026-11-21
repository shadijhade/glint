//! Live Folder Time-Machine Scrubber: unifies Windows shadow copies (VSS), Git commits, restic backups,
//! and NTFS USN journal activity into a single chronological scrubber inside any pane.
//!
//! State is per tab in [`TimeMachineModel`] (mirroring `disk_usage`), initialized lazily and doing zero
//! background work while hidden. Closing the view clears cached moments and diff entries immediately.

use crate::core_bridge;
use crate::explorer::{explorer, ExplorerEvent, TabId};
use crate::kit::prelude::*;
use crate::{i18n, t, theme};
use glint_core::host::Manager as _;
use glint_model::format::{format_count, format_date, format_size};
use glint_model::paths::{is_folder_path, join_path, same_path};
use glint_model::time_machine::{summarize_entries, EntryDelta, Moment, MomentSource, RestoreSource, TimeEntry};
use gpui_kit::{
    uniform_list, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, KeyDownEvent,
    Render, Subscription, UniformListScrollHandle, Window,
};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;

/// Boxed future used by [`Source`].
pub type Fut<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub type MomentsFn = Arc<dyn Fn(String) -> Fut<Result<Vec<Moment>, String>> + Send + Sync>;
pub type InspectFn = Arc<dyn Fn(String, Moment) -> Fut<Result<Vec<TimeEntry>, String>> + Send + Sync>;
pub type RestoreFn = Arc<dyn Fn(String, TimeEntry, bool) -> Fut<Result<String, String>> + Send + Sync>;
pub type StageFn = Arc<dyn Fn(TimeEntry) -> Fut<Result<String, String>> + Send + Sync>;

/// Backend data source for [`TimeMachineModel`]: `glint-core` in production, a fake in tests.
#[derive(Clone)]
pub struct Source {
    /// Lists all historical moments available for `folder` (newest-first from core).
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
            moments: Arc::new(|folder| {
                Box::pin(async move {
                    core_bridge::run(glint_core::time_machine::folder_moments(folder))
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
    moments_loading: bool,
    entries_loading: bool,
    error: Option<String>,
    /// Chronological moments ordered oldest -> newest so scrubbing right advances toward today.
    moments: Vec<Moment>,
    selected: usize,
    entries: Vec<TimeEntry>,
    filter: FilterKind,
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
            moments: Vec::new(),
            selected: 0,
            entries: Vec::new(),
            filter: FilterKind::All,
            moments_serial: 0,
            inspect_serial: 0,
        }
    }

    fn selected_moment(&self) -> Option<&Moment> {
        self.moments.get(self.selected)
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

/// Shared per-tab state for the Time-Machine Scrubber.
pub struct TimeMachineModel {
    source: Source,
    tabs: HashMap<TabId, TabState>,
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
                        cx.notify();
                    }
                }
            }
            _ => {}
        });
        Self {
            source,
            tabs: HashMap::new(),
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
        state.moments_serial = serial;
        self.tabs.insert(tab, state);
        cx.notify();

        let fetch = (self.source.moments)(folder.clone());
        cx.spawn(async move |this, cx| {
            let res = fetch.await;
            this.update(cx, |this, cx| {
                if !this.shown.contains(&tab) {
                    return;
                }
                let Some(state) = this.tabs.get_mut(&tab) else {
                    return;
                };
                if state.moments_serial != serial {
                    return;
                }
                state.moments_loading = false;
                match res {
                    Ok(mut moments) => {
                        // Reverse newest-first -> oldest-first so scrubbing forward moves right toward Today.
                        moments.reverse();
                        state.moments = moments;
                        state.error = None;
                        if let Some(last) = state.moments.len().checked_sub(1) {
                            state.selected = last;
                            let folder = state.folder.clone();
                            let moment = state.moments[last].clone();
                            this.load_entries(tab, folder, moment, cx);
                        } else {
                            state.entries.clear();
                            cx.notify();
                        }
                    }
                    Err(err) => {
                        state.error = Some(err);
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn select_moment(&mut self, tab: TabId, index: usize, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get_mut(&tab) else {
            return;
        };
        if index >= state.moments.len()
            || state.selected == index && !state.entries_loading && !state.entries.is_empty()
        {
            return;
        }
        state.selected = index;
        let folder = state.folder.clone();
        let moment = state.moments[index].clone();
        self.load_entries(tab, folder, moment, cx);
    }

    fn step_moment(&mut self, tab: TabId, delta: isize, cx: &mut Context<Self>) {
        let Some(state) = self.tabs.get(&tab) else {
            return;
        };
        if state.moments.is_empty() {
            return;
        }
        let next = (state.selected as isize + delta).clamp(0, state.moments.len() as isize - 1) as usize;
        self.select_moment(tab, next, cx);
    }

    fn set_filter(&mut self, tab: TabId, filter: FilterKind, cx: &mut Context<Self>) {
        if let Some(state) = self.tabs.get_mut(&tab) {
            if state.filter != filter {
                state.filter = filter;
                cx.notify();
            }
        }
    }

    fn load_entries(&mut self, tab: TabId, folder: String, moment: Moment, cx: &mut Context<Self>) {
        let serial = self.next_serial();
        let Some(state) = self.tabs.get_mut(&tab) else {
            return;
        };
        state.inspect_serial = serial;
        state.entries_loading = true;
        state.error = None;
        cx.notify();

        let inspect = (self.source.inspect)(folder, moment);
        cx.spawn(async move |this, cx| {
            let res = inspect.await;
            this.update(cx, |this, cx| {
                if !this.shown.contains(&tab) {
                    return;
                }
                let Some(state) = this.tabs.get_mut(&tab) else {
                    return;
                };
                if state.inspect_serial != serial {
                    return;
                }
                state.entries_loading = false;
                match res {
                    Ok(entries) => {
                        state.entries = entries;
                        state.error = None;
                    }
                    Err(err) => {
                        state.entries.clear();
                        state.error = Some(err);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

/// The per-pane Time-Machine Scrubber view.
pub struct TimeMachineView {
    pane: usize,
    focus_handle: FocusHandle,
    scroll: UniformListScrollHandle,
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

    fn step(&self, delta: isize, cx: &mut App) {
        let Some(tab) = self.tab_id(cx) else {
            return;
        };
        shared(cx).update(cx, |m, cx| m.step_moment(tab, delta, cx));
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
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
}

fn source_color(source: MomentSource, t: &theme::Tokens) -> Hsla {
    match source {
        MomentSource::VssShadow => t.tk_function,
        MomentSource::GitCommit => t.primary,
        MomentSource::ResticBackup => t.warning,
        MomentSource::UsnActivity => t.muted_foreground,
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

fn delta_badge(delta: &EntryDelta, t: &theme::Tokens) -> Option<(SharedString, Hsla)> {
    match delta {
        EntryDelta::Deleted => Some((t!("Deleted since"), t.destructive)),
        EntryDelta::Modified { .. } => Some((t!("Modified"), t.warning)),
        EntryDelta::AddedSince => Some((t!("Newer"), t.tk_function)),
        EntryDelta::Unchanged => None,
    }
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
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        let selected = state.selected_moment().cloned();
        let (deleted_n, modified_n, unchanged_n) = summarize_entries(&state.entries);
        let filtered_idx = state.filtered_entries();

        let header = row_start()
            .w_full()
            .px(space(4.))
            .py(space(2.5))
            .gap(space(2.5))
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(t.stroke)
            .bg(t.chrome)
            .child(
                row_start()
                    .items_center()
                    .gap(space(2.))
                    .child(icon(IconName::RotateCcwClock).size_4().text_color(t.primary))
                    .child(
                        col()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(theme::size::SM)
                                    .child(t!("Time Machine")),
                            )
                            .child(
                                div()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(state.folder.clone()),
                            ),
                    ),
            )
            .child(
                row_start()
                    .items_center()
                    .gap(space(1.5))
                    .child(
                        Button::new("tm-prev")
                            .compact()
                            .ghost()
                            .icon(Icon::new(IconName::ArrowLeft))
                            .disabled(state.selected == 0 || state.moments.is_empty())
                            .accessibility_label(t!("Older snapshot"))
                            .tooltip(t!("Older snapshot (Left arrow)"))
                            .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
                    )
                    .child(
                        Button::new("tm-next")
                            .compact()
                            .ghost()
                            .icon(Icon::new(IconName::ArrowRight))
                            .disabled(state.moments.is_empty() || state.selected + 1 >= state.moments.len())
                            .accessibility_label(t!("Newer snapshot"))
                            .tooltip(t!("Newer snapshot (Right arrow)"))
                            .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
                    )
                    .child(
                        Button::new("tm-close")
                            .compact()
                            .ghost()
                            .label(t!("Back to files"))
                            .accessibility_label(t!("Close Time Machine"))
                            .tooltip(t!("Back to files (Esc)"))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            );

        // Horizontal timeline scrubber strip.
        let scrubber = if state.moments_loading {
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
            row_start()
                .w_full()
                .px(space(4.))
                .py(space(3.))
                .items_center()
                .border_b_1()
                .border_color(t.stroke)
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(t!(
                    "No shadow copies, Git history, journal buckets, or backups were found for this folder yet."
                ))
                .into_any_element()
        } else {
            let stops = state.moments.iter().enumerate().map(|(ix, m)| {
                let active = ix == state.selected;
                let color = source_color(m.source, &t);
                let when: SharedString = if m.timestamp_ms > 0 {
                    format_date(m.timestamp_ms, Some(now_ms), lang).into()
                } else {
                    m.detail.clone().into()
                };
                let src = source_label(m.source);
                let aria = t!(
                    "{source}: {label} ({when})",
                    source = src.clone(),
                    label = m.label.clone(),
                    when = when.clone()
                );
                Button::new(("tm-moment", ix))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if active {
                                color.opacity(0.18)
                            } else {
                                t.accent.opacity(0.45)
                            })
                            .foreground(if active { t.foreground } else { t.muted_foreground })
                            .hover(if active { color.opacity(0.24) } else { t.pressed })
                            .active(color.opacity(0.28)),
                    )
                    .compact()
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(if active { color } else { t.stroke })
                    .accessibility_label(aria)
                    .tooltip(SharedString::from(format!("{} · {}", m.label, when)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(tab) = this.tab_id(cx) {
                            shared(cx).update(cx, |model, cx| model.select_moment(tab, ix, cx));
                        }
                    }))
                    .child(
                        col()
                            .items_start()
                            .gap(px(1.))
                            .px(space(1.5))
                            .py(space(0.5))
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(1.))
                                    .child(div().size(px(7.)).rounded_full().bg(color))
                                    .child(
                                        div()
                                            .text_size(theme::size::XS)
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(src),
                                    )
                                    .child(div().text_size(theme::size::XS).opacity(0.75).child(when)),
                            )
                            .child(
                                div()
                                    .max_w(px(180.))
                                    .truncate()
                                    .text_size(theme::size::XS)
                                    .child(m.label.clone()),
                            ),
                    )
                    .into_any_element()
            });

            col()
                .w_full()
                .px(space(4.))
                .py(space(2.5))
                .gap(space(2.))
                .border_b_1()
                .border_color(t.stroke)
                .child(
                    row_start()
                        .id("tm-scrubber-rail")
                        .w_full()
                        .gap(space(1.5))
                        .flex_wrap()
                        .children(stops),
                )
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
                        let on = state.filter == kind;
                        Button::new(id)
                            .compact()
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(if on {
                                        t.primary.opacity(0.2)
                                    } else {
                                        t.accent.opacity(0.4)
                                    })
                                    .foreground(if on { t.foreground } else { t.muted_foreground })
                                    .hover(t.pressed)
                                    .active(t.pressed),
                            )
                            .border_1()
                            .border_color(if on { t.primary } else { t.stroke })
                            .accessibility_label(label.clone())
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
                                            .child(m.label),
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
        } else if state.moments.is_empty() {
            col()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .gap(space(2.))
                .px(space(6.))
                .child(icon(IconName::RotateCcwClock).size_8().text_color(t.muted_foreground))
                .child(div().text_size(theme::size::SM).font_weight(FontWeight::MEDIUM).child(t!("Nothing to scrub yet")))
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(t!("Enable Windows File History / Shadow Copies, initialize Git, or configure backups to browse historical states.")),
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
            uniform_list(
                "tm-entries",
                row_count,
                cx.processor(move |_this, range: Range<usize>, _, cx| {
                    let t = theme::tokens(cx).clone();
                    range
                        .map(|row_pos| {
                            let ix = filtered_idx[row_pos];
                            let entry = &entries[ix];
                            let row_entry = entry.clone();
                            let cmp_entry = entry.clone();
                            let restore_folder = folder.clone();
                            let cmp_folder = folder.clone();
                            let can_compare = !entry.dir
                                && !matches!(entry.delta, EntryDelta::Deleted)
                                && !matches!(entry.restore, RestoreSource::UsnMoved { .. });
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

                            row_start()
                                .id(("tm-row", ix))
                                .w_full()
                                .h(px(36.))
                                .px(space(4.))
                                .gap(space(2.))
                                .items_center()
                                .justify_between()
                                .border_b_1()
                                .border_color(t.stroke.opacity(0.4))
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
                                                        this.compare_entry(cmp_folder.clone(), cmp_entry.clone(), cx)
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
                                .into_any_element()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer;
    use crate::explorer::fake::{file, FakeDisk};
    use crate::explorer::NavOptions;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::TestAppContext;
    use std::sync::Mutex;

    fn fake_source(restored: Arc<Mutex<Vec<String>>>, staged: Arc<Mutex<Vec<String>>>) -> Source {
        Source {
            moments: Arc::new(|_folder| {
                Box::pin(async {
                    Ok(vec![
                        Moment {
                            id: "m-new".into(),
                            timestamp_ms: 2_000_000_000_000,
                            label: "Latest commit".into(),
                            detail: "abc1234".into(),
                            source: MomentSource::GitCommit,
                            shadow_path: None,
                            commit_oid: Some("abc1234".into()),
                            repo_root: Some("C:\\Work".into()),
                            plan_id: None,
                            snapshot_id: None,
                        },
                        Moment {
                            id: "m-old".into(),
                            timestamp_ms: 1_000_000_000_000,
                            label: "Shadow yesterday".into(),
                            detail: "vss".into(),
                            source: MomentSource::VssShadow,
                            shadow_path: Some("\\\\?\\GLOBALROOT\\shadow".into()),
                            commit_oid: None,
                            repo_root: None,
                            plan_id: None,
                            snapshot_id: None,
                        },
                    ])
                })
            }),
            inspect: Arc::new(|_folder, moment| {
                Box::pin(async move {
                    if moment.id == "m-old" {
                        Ok(vec![
                            TimeEntry {
                                name: "lost.txt".into(),
                                dir: false,
                                size: 42,
                                modified: 1_000_000_000_000,
                                delta: EntryDelta::Deleted,
                                restore: RestoreSource::ShadowPath("shadow\\lost.txt".into()),
                            },
                            TimeEntry {
                                name: "notes.txt".into(),
                                dir: false,
                                size: 10,
                                modified: 1_000_000_000_000,
                                delta: EntryDelta::Unchanged,
                                restore: RestoreSource::ShadowPath("shadow\\notes.txt".into()),
                            },
                        ])
                    } else {
                        Ok(vec![TimeEntry {
                            name: "notes.txt".into(),
                            dir: false,
                            size: 20,
                            modified: 2_000_000_000_000,
                            delta: EntryDelta::Modified {
                                now_size: 25,
                                now_modified: 2_500_000_000_000,
                            },
                            restore: RestoreSource::GitBlob {
                                repo: "C:\\Work".into(),
                                oid: "abc1234".into(),
                                rel_path: "notes.txt".into(),
                            },
                        }])
                    }
                })
            }),
            restore: Arc::new(move |folder, item, _| {
                let path = join_path(&folder, &item.name);
                restored.lock().unwrap().push(path.clone());
                Box::pin(async move { Ok(path) })
            }),
            stage: Arc::new(move |item| {
                let staged_path = format!("C:\\Temp\\staged-{}", item.name);
                staged.lock().unwrap().push(staged_path.clone());
                Box::pin(async move { Ok(staged_path) })
            }),
        }
    }

    #[gpui_kit::test]
    fn scrubber_loads_moments_switches_filters_and_emits_restore_and_compare(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let disk = FakeDisk::new();
        disk.folder("C:\\Work", vec![file("C:\\Work", "notes.txt", 25, 0)]);

        let restored = Arc::new(Mutex::new(Vec::new()));
        let staged = Arc::new(Mutex::new(Vec::new()));
        let src = fake_source(restored.clone(), staged.clone());

        cx.update(|cx| {
            testing::init(cx);
            explorer::init_with(cx, disk.source());
            init_with(cx, src);
        });

        let (view, visual) = cx.add_window_view(|window, cx| TimeMachineView::new(0, window, cx));
        let mut visual = visual.clone();
        let events: Arc<Mutex<Vec<TimeMachineEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        visual.update(|_, cx| {
            cx.subscribe(&view, move |_, ev: &TimeMachineEvent, _| {
                sink.lock().unwrap().push(ev.clone());
            })
            .detach();
            explorer::explorer(cx).update(cx, |e, cx| e.navigate("C:\\Work", NavOptions::default(), cx));
        });
        visual.run_until_parked();

        // Off until toggled on (zero work when hidden).
        let tab = visual.update(|_, cx| explorer(cx).read(cx).active_tab_id());
        assert!(!visual.update(|_, cx| shows(cx, tab)));

        visual.update(|_, cx| toggle(cx, tab));
        visual.run_until_parked();
        assert!(visual.update(|_, cx| visible(cx, tab)));

        // Chronological order: index 0 is m-old, index 1 is m-new (selected by default).
        visual.update(|window, cx| window.render_frame(cx));
        let model_state = visual
            .update(|_, cx| shared(cx).read(cx).tabs.get(&tab).cloned())
            .unwrap();
        assert_eq!(model_state.moments.len(), 2);
        assert_eq!(model_state.selected, 1);
        assert_eq!(model_state.entries.len(), 1);

        // Compare the modified entry on m-new against today.
        visual.update(|window, cx| {
            window.click(("tm-compare", 0usize), cx);
        });
        visual.run_until_parked();
        assert!(staged.lock().unwrap().iter().any(|p| p.contains("notes.txt")));

        // Click older stop (index 0) and filter to Deleted.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("tm-moment", 0usize), cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("tm-filter-deleted", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("tm-restore", 0usize), cx);
        });
        visual.run_until_parked();

        assert_eq!(restored.lock().unwrap().as_slice(), &["C:\\Work\\lost.txt"]);
        let collected = events.lock().unwrap();
        assert!(collected.iter().any(|e| matches!(e, TimeMachineEvent::Compare { .. })));
        assert!(collected.iter().any(|e| matches!(e, TimeMachineEvent::Restored { .. })));

        // Esc / close button hides and clears cached tab state.
        visual.update(|window, cx| window.click("tm-close", cx));
        visual.run_until_parked();
        assert!(!visual.update(|_, cx| shows(cx, tab)));
        assert!(visual.update(|_, cx| !shared(cx).read(cx).tabs.contains_key(&tab)));
    }
}
