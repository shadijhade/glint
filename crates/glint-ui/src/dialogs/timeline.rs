//! Recent changes: the files touched in a folder over the last quarter hour to month, the busiest subfolders,
//! and what Glint saw change live. Ported from src/components/explorer/TimelineDialog.tsx (with glint-core's
//! `timeline.rs`). Built as [`super::drive_health`] is.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                  | Here                                                        |
//! |---------------------------------------------|-------------------------------------------------------------|
//! | `inspectTimeline(root, windowMs)`           | [`open`]; [`Timeline::scan`] for another time window        |
//! | `closeTimeline` (cancels the scan)          | the model's `Drop` cancels a scan still running             |
//! | `TIME_WINDOWS`                              | [`WINDOWS`]                                                 |
//! | the store's `activeScanId`                  | `Timeline::run`: a newer scan wins                          |
//! | `filteredEntries`, `filteredLive`           | [`TimelineDialog`]'s cached filters, redone when the query, the folder or the report changes |
//! | Time-horizon scrubbers, tabs, filter box    | buttons (radio and tab roles) and a kit `Input`             |
//! | the modal                                   | [`TimelineDialog`]                                          |
//!
//! **Where the work goes.** The scan is glint-core's `scan_timeline` on core's runtime, reached through a
//! [`Source`] so the tests inject fakes. Showing a file in its folder, opening a folder and "Show in File
//! Explorer" are [`Effects`], recorded in tests.
//!
//! **Virtualized.** The changed files (the newest 400) and the live events (up to 500) are lists of equally
//! tall rows, so only what is on screen is drawn.

use crate::kit::prelude::*;
use crate::{core_bridge, explorer, i18n, t, theme};
use glint_model::format::{format_count, format_date, format_full_date, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

pub use glint_core::timeline::{HotSubfolder, LiveFsEvent, TimelineEntry, TimelineReport};

/// One hour: where the dialog starts (`DEFAULT_WINDOW_MS`).
pub const DEFAULT_WINDOW_MS: i64 = 60 * 60 * 1000;

const MINUTE: i64 = 60 * 1000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// The time horizons offered (`TIME_WINDOWS`): short label, long name, milliseconds.
pub fn windows() -> [(&'static str, SharedString, i64); 6] {
    [
        ("15m", t!("Last 15 minutes"), 15 * MINUTE),
        ("1h", t!("Last hour"), HOUR),
        ("6h", t!("Last 6 hours"), 6 * HOUR),
        ("24h", t!("Last 24 hours"), DAY),
        ("7d", t!("Last 7 days"), 7 * DAY),
        ("30d", t!("Last 30 days"), 30 * DAY),
    ]
}

// ---- Where the work comes from ---------------------------------------------------------------------------------------

type Work<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// What the dialog asks glint-core: glint-core's in the app, a fake in tests.
#[derive(Clone)]
pub struct Source {
    /// Scans a folder over a time horizon (`scan_timeline(root, window_ms)`).
    pub scan: Arc<dyn Fn(String, i64) -> Work<TimelineReport> + Send + Sync>,
    /// Stops the scan under way (`cancel_timeline`).
    pub stop: Arc<dyn Fn() + Send + Sync>,
}

impl Source {
    /// glint-core's timeline.
    pub fn core() -> Self {
        Self {
            scan: Arc::new(|root, window_ms| Box::pin(glint_core::timeline::scan_timeline(root, window_ms))),
            stop: Arc::new(glint_core::timeline::cancel_timeline),
        }
    }
}

/// What happens to a path, given the app.
type OnPath = Rc<dyn Fn(&str, &mut App)>;

/// What the dialog does to the rest of the app: the explorer's, in the app; recorders in tests.
#[derive(Clone)]
pub struct Effects {
    /// Shows a file in its folder, selected (`revealInApp`).
    pub reveal: OnPath,
    /// Opens a folder in the active tab (`navigate`).
    pub navigate: OnPath,
    /// Shows a file in Windows' File Explorer (`revealItemInDir`).
    pub show_in_file_explorer: OnPath,
}

impl Effects {
    /// The explorer's and Windows'.
    pub fn app() -> Self {
        Self {
            reveal: Rc::new(|path, cx| {
                if let Some(explorer) = explorer::try_explorer(cx) {
                    let options = explorer::NavOptions::default().selecting(path.to_string());
                    explorer.update(cx, |explorer, cx| explorer.navigate(parent_path(path), options, cx));
                }
            }),
            navigate: Rc::new(|path, cx| {
                if let Some(explorer) = explorer::try_explorer(cx) {
                    explorer.update(cx, |explorer, cx| {
                        explorer.navigate(path.to_string(), explorer::NavOptions::default(), cx)
                    });
                }
            }),
            show_in_file_explorer: Rc::new(|path, cx| {
                // `explorer.exe /select,"path"` opens the folder with the file chosen. Starting it is quick, but
                // it still isn't done on the UI thread.
                let path = path.to_string();
                cx.background_spawn(async move {
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new("explorer.exe")
                        .raw_arg(format!("/select,\"{path}\""))
                        .spawn();
                })
                .detach();
            }),
        }
    }
}

// ---- The model ----------------------------------------------------------------------------------------------------------

/// Where the scan stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Reading modification times across the folder.
    Loading,
    /// Core couldn't scan, with the reason.
    Failed(SharedString),
    /// `report` is the answer.
    Ready,
}

/// The recent changes under a folder, for one time horizon.
pub struct Timeline {
    root: String,
    window_ms: i64,
    source: Source,
    state: State,
    report: Option<TimelineReport>,
    /// The scan asked for last: an older one that answers late is ignored.
    run: u64,
}

impl Timeline {
    /// A model for `root`. It scans nothing until [`Timeline::scan`].
    pub fn new(root: String, window_ms: i64, source: Source) -> Self {
        Self {
            root,
            window_ms,
            source,
            state: State::Loading,
            report: None,
            run: 0,
        }
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn window_ms(&self) -> i64 {
        self.window_ms
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn report(&self) -> Option<&TimelineReport> {
        self.report.as_ref()
    }

    /// Scans over `window_ms` (`None`: the horizon already chosen), in the background. A scan still running is
    /// left to finish and ignored.
    pub fn scan(&mut self, window_ms: Option<i64>, cx: &mut Context<Self>) {
        if let Some(window_ms) = window_ms {
            self.window_ms = window_ms;
        }
        self.run += 1;
        let run = self.run;
        self.state = State::Loading;
        cx.notify();
        let read = (self.source.scan)(self.root.clone(), self.window_ms);
        cx.spawn(async move |this, cx| {
            // Two layers: the core task may panic (outer Err), the scan may fail (inner Err).
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                match result {
                    Ok(report) => {
                        this.report = Some(report);
                        this.state = State::Ready;
                    }
                    Err(reason) => this.state = State::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Drop for Timeline {
    /// Closing the dialog stops a scan still running.
    fn drop(&mut self) {
        if self.state == State::Loading {
            (self.source.stop)();
        }
    }
}

/// Which list is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// The files changed over the horizon, newest first.
    History,
    /// What Glint saw change since it started.
    Live,
}

/// Indexes of `entries` whose name or path holds `query` (case-insensitive, trimmed) and, if given, that sit
/// directly in `folder` (`filteredEntries`).
pub fn filter_entries(entries: &[TimelineEntry], query: &str, folder: Option<&str>) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            if folder.is_some_and(|f| !e.folder.eq_ignore_ascii_case(f) && e.folder.to_lowercase() != f.to_lowercase())
            {
                return false;
            }
            q.is_empty() || e.name.to_lowercase().contains(&q) || e.path.to_lowercase().contains(&q)
        })
        .map(|(ix, _)| ix)
        .collect()
}

/// Indexes of the live events whose name or path holds `query` (`filteredLive`).
pub fn filter_live(events: &[LiveFsEvent], query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| q.is_empty() || e.name.to_lowercase().contains(&q) || e.path.to_lowercase().contains(&q))
        .map(|(ix, _)| ix)
        .collect()
}

/// A folder as the list shows it: from the root down (`sub\deeper`), "." for the root itself, else whole.
pub fn relative_folder(root: &str, folder: &str) -> String {
    if folder.starts_with(root) {
        let rest: String = folder.chars().skip(root.chars().count()).collect();
        let rest = rest.trim_start_matches('\\');
        if rest.is_empty() {
            ".".to_string()
        } else {
            rest.to_string()
        }
    } else {
        folder.to_string()
    }
}

/// The width of a hot folder's bar, in percent of the busiest's, at least 4 (the TypeScript's).
pub fn bar_percent(bytes: u64, busiest: u64) -> f32 {
    let busiest = busiest.max(1);
    let pct = (bytes as f64 / busiest as f64 * 100.0).round();
    pct.clamp(4.0, 100.0) as f32
}

// ---- The dialog -----------------------------------------------------------------------------------------------------------

/// Each list row is this tall (in 4 px units), so the lists can be virtualized.
const ROW_HEIGHT: f32 = 12.;
/// The lists are this tall.
const LIST_HEIGHT: f32 = 56.;

/// Recent changes under a folder.
pub struct TimelineDialog {
    model: Entity<Timeline>,
    effects: Effects,
    tab: Tab,
    query: Entity<InputState>,
    folder_filter: Option<String>,
    /// Indexes into the report's entries and live events that pass the filters.
    entries: Vec<usize>,
    live: Vec<usize>,
    entry_scroll: UniformListScrollHandle,
    live_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl TimelineDialog {
    pub fn new(model: Entity<Timeline>, effects: Effects, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // F5 is the Refresh button's key, as in the web app's dialogs.
        super::refresh_on_f5(cx, |this, _, cx| {
            this.model.update(cx, |model, cx| model.scan(None, cx));
        });
        let query = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter files by name or path…")));
        let typed = cx.subscribe_in(&query, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.refilter(cx);
                cx.notify();
            }
        });
        let redraw = cx.observe(&model, |this, _, cx| {
            this.refilter(cx);
            cx.notify();
        });
        model.update(cx, |model, cx| model.scan(None, cx));
        Self {
            model,
            effects,
            tab: Tab::History,
            query,
            folder_filter: None,
            entries: Vec::new(),
            live: Vec::new(),
            entry_scroll: UniformListScrollHandle::new(),
            live_scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![typed, redraw],
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    /// The lists, filtered by what is typed and the subfolder chosen.
    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let model = self.model.read(cx);
        match model.report() {
            Some(report) => {
                self.entries = filter_entries(&report.entries, &query, self.folder_filter.as_deref());
                self.live = filter_live(&report.live_events, &query);
            }
            None => {
                self.entries.clear();
                self.live.clear();
            }
        }
    }

    fn set_folder_filter(&mut self, folder: Option<String>, cx: &mut Context<Self>) {
        self.folder_filter = folder;
        self.refilter(cx);
        cx.notify();
    }

    fn choose_window(&mut self, ms: i64, cx: &mut Context<Self>) {
        // Another horizon starts afresh: the subfolder chosen was of the last report.
        self.folder_filter = None;
        self.model.update(cx, |model, cx| model.scan(Some(ms), cx));
    }

    /// The time horizon buttons, then refresh.
    fn horizons(&self, current: i64, loading: bool, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        align_row(2.)
            .items_center()
            .child(
                row_start()
                    .id("timeline-windows")
                    .role(Role::RadioGroup)
                    .aria_label(t!("Time window"))
                    .test_support()
                    .gap(space(0.5))
                    .p(space(0.5))
                    .rounded(theme::size::RADIUS)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.muted.opacity(0.4))
                    .children(windows().into_iter().enumerate().map(|(ix, (short, name, ms))| {
                        let chosen = current == ms;
                        Button::new(("timeline-window", ix))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(if chosen { t.background } else { t.background.opacity(0.) })
                                    .foreground(if chosen { t.foreground } else { t.muted_foreground })
                                    .hover(if chosen { t.background } else { t.accent })
                                    .active(if chosen { t.background } else { t.accent }),
                            )
                            .role(Role::RadioButton)
                            .accessibility_label(name.clone())
                            .tooltip(name)
                            .toggled(chosen)
                            .label(short)
                            .xsmall()
                            .on_click(cx.listener(move |this, _, _, cx| this.choose_window(ms, cx)))
                    })),
            )
            .child(
                Button::new("timeline-refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RotateCw)
                    .tooltip(t!("Refresh (F5)"))
                    .accessibility_label(t!("Refresh"))
                    .loading(loading)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |model, cx| model.scan(None, cx));
                    })),
            )
    }

    /// The tabs, the subfolder chip and the filter box.
    fn toolbar(&self, report: Option<&TimelineReport>, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let tab_button = |id: &'static str,
                          tab: Tab,
                          icon_name: IconName,
                          label: SharedString,
                          count: Option<usize>,
                          tone: Hsla,
                          cx: &mut Context<Self>| {
            let chosen = self.tab == tab;
            let named = match count {
                Some(n) => format!("{label} ({})", format_count(n)),
                None => label.to_string(),
            };
            Button::new(id)
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(if chosen { tone.opacity(0.15) } else { tone.opacity(0.) })
                        .foreground(if chosen { tone } else { t.muted_foreground })
                        .hover(if chosen { tone.opacity(0.2) } else { t.accent })
                        .active(if chosen { tone.opacity(0.25) } else { t.accent }),
                )
                .role(Role::Tab)
                .accessibility_label(named)
                .toggled(chosen)
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    cx.notify();
                }))
                .child(
                    align_row(1.5)
                        .child(icon(icon_name).size_3p5())
                        .child(label)
                        .children(count.map(|n| {
                            div()
                                .px(space(1.5))
                                .rounded_full()
                                .bg(tone.opacity(0.2))
                                .text_size(theme::size::XXS)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format_count(n))
                        })),
                )
        };

        let history = tab_button(
            "timeline-tab-history",
            Tab::History,
            IconName::Clock,
            t!("Changed files"),
            report.map(|r| r.total_files_changed),
            t.primary,
            cx,
        );
        let live = tab_button(
            "timeline-tab-live",
            Tab::Live,
            IconName::Activity,
            t!("Live"),
            report.map(|r| r.live_events.len()),
            t.success,
            cx,
        );
        let chip = self
            .folder_filter
            .as_ref()
            .filter(|_| self.tab == Tab::History)
            .map(|folder| {
                let name = {
                    let base = base_name(folder);
                    if base.is_empty() {
                        folder.as_str()
                    } else {
                        base
                    }
                };
                let label = t!("In: {name}", name = name);
                Button::new("timeline-clear-folder")
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(t.warning.opacity(0.15))
                            .foreground(t.warning)
                            .hover(t.warning.opacity(0.25))
                            .active(t.warning.opacity(0.3)),
                    )
                    .xsmall()
                    .tooltip(t!("Clear subfolder filter"))
                    .accessibility_label(t!("Clear subfolder filter: {label}", label = label.clone()))
                    .on_click(cx.listener(|this, _, _, cx| this.set_folder_filter(None, cx)))
                    .child(align_row(1.).child(isolate(label)).child(icon(IconName::X).size_3()))
            });

        row_start()
            .gap(space(2.))
            .flex_wrap()
            .items_center()
            .pb(space(2.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(
                row_start()
                    .id("timeline-tabs")
                    .role(Role::TabList)
                    .aria_label(t!("Recent changes"))
                    .test_support()
                    .gap(space(1.))
                    .child(history)
                    .child(live),
            )
            .children(chip)
            .child(
                div()
                    .when(i18n::is_rtl(), |this| this.mr_auto())
                    .when(!i18n::is_rtl(), |this| this.ml_auto())
                    .w(space(56.))
                    .child(Input::new(&self.query).aria_label(t!("Filter files by name or path…"))),
            )
    }

    /// The summary banner and the busiest folders.
    fn summary(&self, report: &TimelineReport, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let touched = t!(
            "{files} touched ({size})",
            files = plural(report.total_files_changed, "file"),
            size = format_size(report.total_bytes_changed)
        );
        let mut looked = t!(
            "Looked at {files} in {ms} ms",
            files = plural(report.files_scanned, "file"),
            ms = format!("{:.1}", report.elapsed_ms)
        )
        .to_string();
        if report.stopped_early {
            looked.push_str(&format!(" · {}", t!("stopped there: the folder has more")));
        }
        let banner = row_start()
            .id("timeline-summary")
            .role(Role::Group)
            .aria_label(format!("{touched}. {looked}"))
            .test_support()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(space(2.))
            .px(space(3.5))
            .py(space(2.5))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.2))
            .text_size(theme::size::XS)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(touched))
            .child(div().text_color(t.muted_foreground).child(looked));

        let busiest = report.hot_folders.first().map_or(1, |h| h.bytes_changed).max(1);
        let hot = (!report.hot_folders.is_empty()).then(|| {
            let cards: Vec<Div> = report
                .hot_folders
                .iter()
                .take(6)
                .enumerate()
                .map(|(ix, folder)| self.hot_card(ix, folder, busiest, t, cx))
                .collect();
            col()
                .gap(space(2.))
                .p(space(3.))
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .child(
                    align_row(1.5)
                        .text_size(theme::size::XS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(icon(IconName::Flame).size_3p5().text_color(t.warning))
                        .child(t!("Busiest folders (by bytes changed)")),
                )
                .child(
                    div()
                        .id("timeline-hot")
                        .role(Role::List)
                        .aria_label(t!("Busiest folders (by bytes changed)"))
                        .test_support()
                        .flex()
                        .flex_wrap()
                        .gap(space(1.5))
                        .children(cards),
                )
        });
        col().gap(space(3.)).child(banner).children(hot)
    }

    /// One busiest-folder card: a bar, its name and size, a button that filters to it and one that opens it.
    fn hot_card(
        &self,
        ix: usize,
        folder: &HotSubfolder,
        busiest: u64,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> Div {
        let active = self
            .folder_filter
            .as_ref()
            .is_some_and(|f| f.to_lowercase() == folder.path.to_lowercase());
        let pct = bar_percent(folder.bytes_changed, busiest);
        let path = folder.path.clone();
        let open_path = folder.path.clone();
        let name = folder.name.clone();
        let size = format_size(folder.bytes_changed);
        let count = format_count(folder.files_changed);
        div()
            .relative()
            .overflow_hidden()
            .w(relative(0.49))
            .min_w(space(48.))
            .flex_1()
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(if active { t.primary } else { t.border })
            .when(active, |this| this.bg(t.primary.opacity(0.05)))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .when(i18n::is_rtl(), |this| this.right_0())
                    .when(!i18n::is_rtl(), |this| this.left_0())
                    .w(relative(pct / 100.))
                    .bg(t.warning.opacity(0.12)),
            )
            .child(
                row_start()
                    .relative()
                    .items_center()
                    .gap(space(1.))
                    .child(
                        Button::new(("timeline-hot", ix))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(t.accent.opacity(0.))
                                    .foreground(t.foreground)
                                    .hover(t.accent.opacity(0.5))
                                    .active(t.accent),
                            )
                            .accessibility_label(t!("Only changes in {name}", name = name.clone()))
                            .tooltip(path.clone())
                            .toggled(active)
                            .xsmall()
                            .flex_1()
                            .min_w_0()
                            .justify_start()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let next = if active { None } else { Some(path.clone()) };
                                this.set_folder_filter(next, cx);
                            }))
                            .child(
                                align_row(2.)
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .flex_1()
                                            .truncate()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(isolate(name)),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.warning)
                                            .child(size),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(theme::size::XXS)
                                            .text_color(t.muted_foreground)
                                            .child(format!("({count})")),
                                    ),
                            ),
                    )
                    .child(
                        Button::new(("timeline-hot-open", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderOpen)
                            .tooltip(t!("Open folder in Glint"))
                            .accessibility_label(t!("Open folder in Glint: {name}", name = folder.name.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                // The dialog closes first, then the folder opens.
                                window.close_dialog(cx);
                                let navigate = this.effects.navigate.clone();
                                navigate(&open_path, cx);
                            })),
                    ),
            )
    }

    /// A changed file's row.
    fn entry_row(
        &self,
        slot: usize,
        item: &TimelineEntry,
        root: &str,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mono = theme::mono_font(cx);
        let folder = relative_folder(root, &item.folder);
        let when = format_date(item.modified, None, i18n::language());
        let full = format_full_date(item.modified);
        let size = format_size(item.size);
        let (reveal_path, show_path) = (item.path.clone(), item.path.clone());
        let tip = full.clone();
        row_start()
            .id(("timeline-entry", slot))
            .role(Role::Group)
            .aria_label(format!("{}, {size}, {folder}, {when}", item.name))
            .test_support()
            .h(space(ROW_HEIGHT))
            .items_center()
            .gap(space(3.))
            .px(space(3.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .hover(|style| style.bg(t.accent))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .child(
                        align_row(2.)
                            .max_w_full()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(isolate(item.name.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .font_family(mono.clone())
                                    .text_size(theme::size::XXS)
                                    .text_color(t.muted_foreground)
                                    .child(size),
                            ),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(isolate(folder)),
                    ),
            )
            .child(
                div()
                    .id(("timeline-entry-date", slot))
                    .flex_none()
                    .font_family(mono)
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .tooltip(move |window, cx| tooltip(tip.clone().into(), window, cx))
                    .child(when),
            )
            .child(
                align_row(0.5)
                    .flex_none()
                    .child(
                        Button::new(("timeline-entry-reveal", slot))
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderInput)
                            .tooltip(t!("Open file location"))
                            .accessibility_label(t!("Open file location of {name}", name = item.name.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                window.close_dialog(cx);
                                let reveal = this.effects.reveal.clone();
                                reveal(&reveal_path, cx);
                            })),
                    )
                    .child(
                        Button::new(("timeline-entry-explorer", slot))
                            .ghost()
                            .xsmall()
                            .icon(IconName::FolderOpen)
                            .tooltip(t!("Show in File Explorer"))
                            .accessibility_label(t!("Show in File Explorer: {name}", name = item.name.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let show = this.effects.show_in_file_explorer.clone();
                                show(&show_path, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// A live event's row.
    fn live_row(&self, slot: usize, event: &LiveFsEvent, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let mono = theme::mono_font(cx);
        let (word, icon_name, tone) = match event.kind.as_str() {
            "created" => (t!("created"), IconName::FilePlus, t.success),
            "deleted" => (t!("deleted"), IconName::FileX, t.destructive),
            _ => (t!("modified"), IconName::FilePen, t.primary),
        };
        let full = format_full_date(event.timestamp_ms);
        let reveal_path = event.path.clone();
        let deleted = event.kind == "deleted";
        row_start()
            .id(("timeline-live", slot))
            .role(Role::Group)
            .aria_label(format!("{word}: {}, {}", event.name, event.path))
            .test_support()
            .h(space(ROW_HEIGHT))
            .items_center()
            .gap(space(3.))
            .px(space(3.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .child(
                align_row(1.)
                    .flex_none()
                    .px(space(1.5))
                    .py(space(0.5))
                    .rounded(theme::size::RADIUS_SM)
                    .bg(tone.opacity(0.15))
                    .text_color(tone)
                    .font_family(mono.clone())
                    .text_size(theme::size::XXS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon(icon_name).size_3())
                    .child(word.to_uppercase()),
            )
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(isolate(event.name.clone())),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(isolate(event.path.clone())),
                    ),
            )
            .children(event.size.map(|size| {
                div()
                    .flex_none()
                    .font_family(mono.clone())
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(format_size(size))
            }))
            .child(
                div()
                    .flex_none()
                    .font_family(mono)
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(full),
            )
            .when(!deleted, |this| {
                this.child(
                    Button::new(("timeline-live-reveal", slot))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderInput)
                        .tooltip(t!("Open file location"))
                        .accessibility_label(t!("Open file location of {name}", name = event.name.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.close_dialog(cx);
                            let reveal = this.effects.reveal.clone();
                            reveal(&reveal_path, cx);
                        })),
                )
            })
            .into_any_element()
    }

    /// A centred message in a list's place.
    fn message(id: &'static str, role: Role, headline: SharedString, height: f32) -> impl ParentElement + IntoElement {
        col()
            .id(id)
            .role(role)
            .aria_label(headline)
            .test_support()
            .h(space(height))
            .items_center()
            .justify_center()
            .gap(space(2.))
            .text_center()
    }

    /// The changed files tab: summary, busiest folders, the list.
    fn history_body(&self, report: &TimelineReport, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let root = self.model.read(cx).root().to_string();
        let list: AnyElement = if self.entries.is_empty() {
            Self::message(
                "timeline-empty",
                Role::Status,
                t!("No files modified in this time window"),
                44.,
            )
            .child(icon(IconName::Clock).size_8().text_color(t.muted_foreground))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(t!("No files modified in this time window")),
            )
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("Try selecting a wider time horizon above (e.g. 24h, 7d, or 30d).")),
            )
            .into_any_element()
        } else {
            let truncated = report.entries.len() < report.total_files_changed;
            let note = truncated.then(|| {
                div()
                    .id("timeline-newest")
                    .role(Role::Status)
                    .aria_label(t!(
                        "The newest {n} of {total} files touched",
                        n = format_count(report.entries.len()),
                        total = format_count(report.total_files_changed)
                    ))
                    .test_support()
                    .px(space(3.))
                    .py(space(1.5))
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!(
                        "The newest {n} of {total} files touched",
                        n = format_count(report.entries.len()),
                        total = format_count(report.total_files_changed)
                    ))
            });
            let entries = report.entries.clone();
            let shown = self.entries.clone();
            let colors = t.clone();
            let count = shown.len();
            let uniform = uniform_list(
                "timeline-entries",
                count,
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range
                        .filter_map(|slot| {
                            let item = entries.get(*shown.get(slot)?)?;
                            Some(this.entry_row(slot, item, &root, &colors, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.entry_scroll)
            .size_full();
            col()
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .overflow_hidden()
                .children(note)
                .child(
                    div()
                        .relative()
                        .h(space(LIST_HEIGHT))
                        .child(
                            div()
                                .id("timeline-list")
                                .role(Role::List)
                                .aria_label(t!("Changed files"))
                                .test_support()
                                .size_full()
                                .child(uniform),
                        )
                        .child(Scrollbar::vertical(&self.entry_scroll).viewport_from_layout()),
                )
                .into_any_element()
        };
        col()
            .gap(space(3.))
            .child(self.summary(report, t, cx))
            .child(list)
            .into_any_element()
    }

    /// The live tab: the explanation, then the list.
    fn live_body(&self, report: &TimelineReport, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let intro = div()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(t!("Changes Glint saw in folders open in its tabs since it started."));
        let list: AnyElement = if self.live.is_empty() {
            Self::message(
                "timeline-live-empty",
                Role::Status,
                t!("No changes seen in this folder yet"),
                48.,
            )
            .child(icon(IconName::Activity).size_8().text_color(t.success))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .child(t!("No changes seen in this folder yet")),
            )
            .child(
                div()
                    .max_w(space(96.))
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!(
                        "Changes to files in folders open in Glint show here. Make some, then click Rescan."
                    )),
            )
            .into_any_element()
        } else {
            let events = report.live_events.clone();
            let shown = self.live.clone();
            let colors = t.clone();
            let count = shown.len();
            let uniform = uniform_list(
                "timeline-live-events",
                count,
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    range
                        .filter_map(|slot| {
                            let event = events.get(*shown.get(slot)?)?;
                            Some(this.live_row(slot, event, &colors, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&self.live_scroll)
            .size_full();
            div()
                .relative()
                .h(space(LIST_HEIGHT + 20.))
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .overflow_hidden()
                .child(
                    div()
                        .id("timeline-live-list")
                        .role(Role::List)
                        .aria_label(t!("Live"))
                        .test_support()
                        .size_full()
                        .child(uniform),
                )
                .child(Scrollbar::vertical(&self.live_scroll).viewport_from_layout())
                .into_any_element()
        };
        col().gap(space(3.)).child(intro).child(list).into_any_element()
    }
}

impl Render for TimelineDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let model = self.model.read(cx);
        let state = model.state().clone();
        let root = model.root().to_string();
        let current = model.window_ms();
        let report = model.report().cloned();
        let loading = state == State::Loading;
        let name = {
            let base = base_name(&root);
            if base.is_empty() {
                root.clone()
            } else {
                base.to_string()
            }
        };
        let mono = theme::mono_font(cx);

        let header = row_start()
            .gap(space(3.))
            .items_center()
            .justify_between()
            .flex_wrap()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .font_family(mono)
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(isolate(root.clone())),
            )
            .child(self.horizons(current, loading, &t, cx));

        let body: AnyElement = match (&state, &report) {
            (State::Loading, _) => Self::message(
                "timeline-loading",
                Role::Status,
                t!("Scanning modification timestamps across {name}…", name = name.clone()),
                64.,
            )
            .child(Spinner::new())
            .child(div().text_color(t.muted_foreground).child(t!(
                "Scanning modification timestamps across {name}…",
                name = isolate(name.clone())
            )))
            .into_any_element(),
            (State::Failed(reason), _) => {
                let headline = t!("Couldn't scan folder timeline");
                Self::message("timeline-error", Role::Alert, headline.clone(), 64.)
                    .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                    .child(div().font_weight(FontWeight::MEDIUM).child(headline))
                    .child(
                        div()
                            .id("timeline-error-reason")
                            .role(Role::Status)
                            .aria_label(reason.clone())
                            .test_support()
                            .max_w(space(112.))
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(reason.clone()),
                    )
                    .into_any_element()
            }
            (State::Ready, Some(report)) => match self.tab {
                Tab::History => self.history_body(report, &t, cx),
                Tab::Live => self.live_body(report, &t, cx),
            },
            (State::Ready, None) => div().into_any_element(),
        };

        let footer_text = match (&report, state == State::Ready) {
            (Some(_), true) => match self.tab {
                Tab::History => plural(self.entries.len(), "file"),
                Tab::Live => plural(self.live.len(), "change"),
            },
            _ => String::new(),
        };
        let footer = row_start()
            .items_center()
            .justify_between()
            .pt(space(2.))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(
                div()
                    .id("timeline-count")
                    .role(Role::Status)
                    .aria_label(footer_text.clone())
                    .test_support()
                    .child(footer_text),
            )
            .child(
                Button::new("timeline-close")
                    .outline()
                    .small()
                    .label(t!("Close"))
                    .on_click(|_, window, cx| window.close_dialog(cx)),
            );

        col()
            .id("timeline")
            .role(Role::Group)
            .aria_label(title_for(&root))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer sits outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(header)
            .child(self.toolbar(report.as_ref(), &t, cx))
            .child(div().min_h(space(LIST_HEIGHT)).child(body))
            .child(footer)
    }
}

/// The dialog's title: "Recent changes in “Photos”".
pub fn title_for(root: &str) -> SharedString {
    let name = base_name(root);
    t!(
        "Recent changes in “{name}”",
        name = if name.is_empty() { root } else { name }
    )
}

/// Opens Recent changes for `root` in `window` and starts scanning it, over the last hour (This PC's
/// `RecentChanges`, the menus' Timeline). Nothing opens for an empty path.
pub fn open(window: &mut Window, cx: &mut App, root: String) {
    open_with(window, cx, root, Source::core(), Effects::app());
}

/// [`open`] with the work and the effects given (tests pass fakes).
pub fn open_with(window: &mut Window, cx: &mut App, root: String, source: Source, effects: Effects) {
    if root.is_empty() {
        return;
    }
    let title = title_for(&root);
    let view = make(window, cx, root, source, effects);
    super::show(window, cx, title, px(860.), view);
}

/// The dialog's view, scanning already (made before it is shown, as its builder runs again).
fn make(window: &mut Window, cx: &mut App, root: String, source: Source, effects: Effects) -> Entity<TimelineDialog> {
    let model = cx.new(|_| Timeline::new(root, DEFAULT_WINDOW_MS, source));
    cx.new(|cx| TimelineDialog::new(model, effects, window, cx))
}

// ---- Tests --------------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    fn entry(folder: &str, name: &str, size: u64, modified: i64) -> TimelineEntry {
        TimelineEntry {
            path: format!("{folder}\\{name}"),
            name: name.into(),
            folder: folder.into(),
            size,
            modified,
        }
    }

    fn report() -> TimelineReport {
        TimelineReport {
            root: "C:\\Work".into(),
            window_ms: HOUR,
            entries: vec![
                entry("C:\\Work\\src", "main.rs", 4096, 1_700_000_300_000),
                entry("C:\\Work", "notes.txt", 100, 1_700_000_200_000),
                entry("C:\\Work\\src", "lib.rs", 2048, 1_700_000_100_000),
            ],
            hot_folders: vec![
                HotSubfolder {
                    path: "C:\\Work\\src".into(),
                    name: "src".into(),
                    bytes_changed: 6144,
                    files_changed: 2,
                },
                HotSubfolder {
                    path: "C:\\Work\\docs".into(),
                    name: "docs".into(),
                    bytes_changed: 1024,
                    files_changed: 1,
                },
            ],
            live_events: vec![
                LiveFsEvent {
                    path: "C:\\Work\\src\\main.rs".into(),
                    name: "main.rs".into(),
                    kind: "modified".into(),
                    timestamp_ms: 1_700_000_300_000,
                    size: Some(4096),
                    dir: false,
                },
                LiveFsEvent {
                    path: "C:\\Work\\old.txt".into(),
                    name: "old.txt".into(),
                    kind: "deleted".into(),
                    timestamp_ms: 1_700_000_250_000,
                    size: None,
                    dir: false,
                },
            ],
            total_bytes_changed: 6244,
            total_files_changed: 3,
            files_scanned: 250,
            stopped_early: false,
            elapsed_ms: 12.34,
        }
    }

    #[derive(Default)]
    struct Log {
        scans: AtomicUsize,
        stops: AtomicUsize,
        last_window: AtomicI64,
        revealed: Mutex<Vec<String>>,
        opened: Mutex<Vec<String>>,
        shown: Mutex<Vec<String>>,
    }

    fn fake(answer: Result<TimelineReport, String>) -> (Source, Arc<Log>) {
        let log = Arc::new(Log::default());
        let (a, b) = (log.clone(), log.clone());
        let source = Source {
            scan: Arc::new(move |_, window_ms| {
                a.scans.fetch_add(1, Ordering::SeqCst);
                a.last_window.store(window_ms, Ordering::SeqCst);
                let answer = answer.clone();
                Box::pin(async move { answer })
            }),
            stop: Arc::new(move || {
                b.stops.fetch_add(1, Ordering::SeqCst);
            }),
        };
        (source, log)
    }

    fn effects(log: &Arc<Log>) -> Effects {
        let (a, b, c) = (log.clone(), log.clone(), log.clone());
        Effects {
            reveal: Rc::new(move |path, _| a.revealed.lock().unwrap().push(path.to_string())),
            navigate: Rc::new(move |path, _| b.opened.lock().unwrap().push(path.to_string())),
            show_in_file_explorer: Rc::new(move |path, _| c.shown.lock().unwrap().push(path.to_string())),
        }
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn settle(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never happened: {what}");
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn open_dialog(visual: &mut VisualTestContext, root: &str, source: Source, effects: Effects) {
        let root = root.to_string();
        visual.update(|window, cx| open_with(window, cx, root, source, effects));
    }

    fn click(visual: &mut VisualTestContext, id: impl Into<ElementId> + Clone) {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        visual.run_until_parked();
    }

    #[test]
    fn the_time_windows_are_the_typescripts() {
        let _guard = testing::lock();
        let all = windows();
        let shorts: Vec<_> = all.iter().map(|w| w.0).collect();
        assert_eq!(shorts, ["15m", "1h", "6h", "24h", "7d", "30d"]);
        assert_eq!(all[1].2, DEFAULT_WINDOW_MS);
        assert_eq!(all[5].2, 30 * 24 * 60 * 60 * 1000);
        assert_eq!(all[2].1, "Last 6 hours");
    }

    #[test]
    fn filters_match_names_and_paths_and_the_chosen_subfolder() {
        let r = report();
        assert_eq!(filter_entries(&r.entries, "", None), [0, 1, 2]);
        assert_eq!(filter_entries(&r.entries, "  LIB ", None), [2]);
        // The path counts too: "work\src" is in two paths.
        assert_eq!(filter_entries(&r.entries, "work\\src", None), [0, 2]);
        assert_eq!(filter_entries(&r.entries, "", Some("c:\\work\\SRC")), [0, 2]);
        assert_eq!(
            filter_entries(&r.entries, "notes", Some("C:\\Work\\src")),
            Vec::<usize>::new()
        );
        assert_eq!(filter_live(&r.live_events, "OLD"), [1]);
        assert_eq!(filter_live(&r.live_events, ""), [0, 1]);
    }

    #[test]
    fn folders_are_shown_from_the_root_and_bars_have_a_minimum() {
        assert_eq!(relative_folder("C:\\Work", "C:\\Work\\src\\deep"), "src\\deep");
        assert_eq!(relative_folder("C:\\Work", "C:\\Work"), ".");
        assert_eq!(relative_folder("C:\\Work", "D:\\Elsewhere"), "D:\\Elsewhere");
        assert_eq!(bar_percent(6144, 6144), 100.0);
        assert_eq!(bar_percent(1024, 6144), 17.0);
        assert_eq!(bar_percent(1, 6144), 4.0);
        assert_eq!(bar_percent(5, 0), 100.0);
    }

    #[gpui_kit::test]
    fn scans_the_last_hour_in_the_background_then_lists_the_files(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(report()));
        open_dialog(&mut visual, "C:\\Work", source, effects(&log));
        // Straight after opening: a status line, not a blank dialog.
        assert_eq!(
            role_of(&mut visual, "Scanning modification timestamps across Work…"),
            Some(Role::Status)
        );
        settle(&mut visual, "the list", |cx| has(cx, "3 files"));
        assert_eq!(log.scans.load(Ordering::SeqCst), 1);
        assert_eq!(log.last_window.load(Ordering::SeqCst), DEFAULT_WINDOW_MS);

        assert_eq!(role_of(&mut visual, "Recent changes in “Work”"), Some(Role::Group));
        assert_eq!(
            role_of(&mut visual, "3 files touched (6.1 KB). Looked at 250 files in 12.3 ms"),
            Some(Role::Group)
        );
        assert_eq!(role_of(&mut visual, "Changed files (3)"), Some(Role::Tab));
        assert_eq!(role_of(&mut visual, "Live (2)"), Some(Role::Tab));
        assert_eq!(role_of(&mut visual, "Last hour"), Some(Role::RadioButton));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_by_label("Last hour").checked(), Some(true));
            assert_eq!(window.find_by_label("Last 15 minutes").checked(), Some(false));
        });
        assert_eq!(role_of(&mut visual, "Only changes in src"), Some(Role::Button));
        // Rows are named with their size, folder and date; the root's own files show ".".
        let main = "main.rs, 4.0 KB, src, ";
        let found = visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::Group)
                .iter()
                .any(|g| g.label().is_some_and(|l| l.starts_with(main)))
        });
        assert!(found, "the main.rs row has a name");
        assert!(visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::Group)
                .iter()
                .any(|g| g.label().is_some_and(|l| l.starts_with("notes.txt, 100 B, ., ")))
        }));
    }

    #[gpui_kit::test]
    fn another_time_window_scans_again_and_forgets_the_subfolder(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(report()));
        open_dialog(&mut visual, "C:\\Work", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "3 files"));

        // Choosing a busy folder filters to it and shows the chip that clears it.
        click(&mut visual, ("timeline-hot", 0_usize));
        assert!(has(&mut visual, "2 files"));
        assert_eq!(
            role_of(&mut visual, "Clear subfolder filter: In: src"),
            Some(Role::Button)
        );
        // A week: scan again, the filter goes.
        click(&mut visual, ("timeline-window", 4_usize));
        assert_eq!(
            role_of(&mut visual, "Scanning modification timestamps across Work…"),
            Some(Role::Status)
        );
        settle(&mut visual, "the second scan", |_| {
            log.scans.load(Ordering::SeqCst) == 2
        });
        assert_eq!(log.last_window.load(Ordering::SeqCst), 7 * 24 * 60 * 60 * 1000);
        settle(&mut visual, "all three again", |cx| has(cx, "3 files"));
        assert!(!has(&mut visual, "Clear subfolder filter: In: src"));
        // Refresh asks again over the same window.
        click(&mut visual, "timeline-refresh");
        settle(&mut visual, "a third scan", |_| log.scans.load(Ordering::SeqCst) == 3);
        assert_eq!(log.last_window.load(Ordering::SeqCst), 7 * 24 * 60 * 60 * 1000);
    }

    #[gpui_kit::test]
    fn typing_filters_the_files_and_the_footer_counts_them(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(report()));
        let view = visual.update(|window, cx| {
            let view = make(window, cx, r"C:\Work".into(), source, effects(&log));
            super::super::show(window, cx, title_for(r"C:\Work"), px(860.), view.clone());
            view
        });
        settle(&mut visual, "the list", |cx| has(cx, "3 files"));
        let input = view.read_with(&visual, |dialog, _| dialog.query.clone());
        visual.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.input("LIB", cx);
        });
        visual.run_until_parked();
        assert!(has(&mut visual, "1 file"), "only lib.rs holds LIB");
        assert_eq!(view.read_with(&visual, |dialog, _| dialog.entries.clone()), [2]);
        // The live tab filters by the same box, and the footer counts changes.
        click(&mut visual, "timeline-tab-live");
        assert!(has(&mut visual, "0 changes"));
        assert_eq!(view.read_with(&visual, |dialog, _| dialog.tab()), Tab::Live);
    }

    #[gpui_kit::test]
    fn live_tab_lists_what_was_seen_and_deleted_files_cannot_be_opened(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(report()));
        open_dialog(&mut visual, "C:\\Work", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "3 files"));
        click(&mut visual, "timeline-tab-live");
        assert!(has(&mut visual, "2 changes"));
        assert_eq!(
            role_of(&mut visual, "modified: main.rs, C:\\Work\\src\\main.rs"),
            Some(Role::Group)
        );
        assert_eq!(
            role_of(&mut visual, "deleted: old.txt, C:\\Work\\old.txt"),
            Some(Role::Group)
        );
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(("timeline-live-reveal", 0_usize)).is_some());
            assert!(
                window.try_find(("timeline-live-reveal", 1_usize)).is_none(),
                "a deleted file has nowhere to open"
            );
        });
        click(&mut visual, ("timeline-live-reveal", 0_usize));
        assert_eq!(
            *log.revealed.lock().unwrap(),
            vec!["C:\\Work\\src\\main.rs".to_string()]
        );
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "the dialog closes first"));
    }

    #[gpui_kit::test]
    fn row_buttons_reveal_in_glint_or_in_file_explorer_and_hot_folders_open(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(report()));
        open_dialog(&mut visual, "C:\\Work", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "3 files"));
        // Windows' own Explorer: the dialog stays.
        click(&mut visual, ("timeline-entry-explorer", 1_usize));
        assert_eq!(*log.shown.lock().unwrap(), vec!["C:\\Work\\notes.txt".to_string()]);
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        // A folder in Glint: the dialog closes first.
        click(&mut visual, ("timeline-hot-open", 1_usize));
        assert_eq!(*log.opened.lock().unwrap(), vec!["C:\\Work\\docs".to_string()]);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn errors_are_said_in_words(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Err("Folder does not exist".into()));
        open_dialog(&mut visual, "C:\\Gone", source, effects(&log));
        settle(&mut visual, "the error", |cx| has(cx, "Couldn't scan folder timeline"));
        assert_eq!(role_of(&mut visual, "Couldn't scan folder timeline"), Some(Role::Alert));
        assert_eq!(role_of(&mut visual, "Folder does not exist"), Some(Role::Status));
    }

    #[gpui_kit::test]
    fn an_empty_window_suggests_a_wider_one(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut empty = report();
        empty.entries.clear();
        empty.hot_folders.clear();
        empty.total_files_changed = 0;
        empty.total_bytes_changed = 0;
        let (source, log) = fake(Ok(empty));
        open_dialog(&mut visual, "C:\\Work", source, effects(&log));
        settle(&mut visual, "the empty line", |cx| {
            has(cx, "No files modified in this time window")
        });
        assert_eq!(
            role_of(&mut visual, "No files modified in this time window"),
            Some(Role::Status)
        );
        assert!(has(&mut visual, "0 files"));
    }

    #[gpui_kit::test]
    fn escape_closes_it_and_closing_stops_a_scan_still_running(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let log = Arc::new(Log::default());
        let b = log.clone();
        let source = Source {
            scan: Arc::new(|_, _| Box::pin(std::future::pending())),
            stop: Arc::new(move || {
                b.stops.fetch_add(1, Ordering::SeqCst);
            }),
        };
        open_dialog(&mut visual, "C:\\Work", source, effects(&log));
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            assert!(
                window.focused(cx).is_some(),
                "nothing has focus after the dialog opened"
            );
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Escape must close the dialog"));
        assert_eq!(
            log.stops.load(Ordering::SeqCst),
            1,
            "a scan under way is stopped when the dialog goes"
        );
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let mut arabic = report();
        arabic.entries = vec![entry("C:\\ملفات\\مشروع", "تقرير.docx", 1024, 1_700_000_000_000)];
        arabic.total_files_changed = 1;
        let (source, log) = fake(Ok(arabic));
        open_dialog(&mut visual, "C:\\ملفات", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "1 file"));
        assert_eq!(role_of(&mut visual, "Recent changes in “ملفات”"), Some(Role::Group));
        let found = visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::Group)
                .iter()
                .any(|g| g.label().is_some_and(|l| l.starts_with("تقرير.docx, 1.0 KB, مشروع, ")))
        });
        assert!(found, "the Arabic row is named as the file is, without isolates");
        testing::set_rtl(false);
    }
}
