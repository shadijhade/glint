//! Duplicate files: groups of identical files in a folder, which copy to keep, and what to do with the rest.
//! Ported from src/components/explorer/DuplicatesDialog.tsx (with glint-core's `dedup.rs`). Built as
//! [`super::drive_health`] is.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                   | Here                                                         |
//! |----------------------------------------------|--------------------------------------------------------------|
//! | `inspectDuplicates(root)`                    | [`open`]                                                     |
//! | `closeDuplicates` (stops the scan)           | the model's `Drop` stops a scan still running                |
//! | `DuplicatesModal` state, `scan`, `latest`    | [`Duplicates`], a model entity; a newer scan wins            |
//! | `keptFor`, `dupsFor`                         | [`Duplicates::kept_for`], [`Duplicates::dups_for`]           |
//! | Hard-link / Clone (one group, all groups)    | [`Duplicates::link_group`], [`Duplicates::link_all`], each asked first with [`super::confirm`] |
//! | Recycle (one group, all groups)              | [`Duplicates::recycle_group`], [`Duplicates::recycle_all`]   |
//! | the modal                                    | [`DuplicatesDialog`]                                         |
//!
//! **Where the work goes.** Scanning and linking are glint-core's (`find_duplicates`, `hardlink_duplicates`,
//! `clone_duplicates`) on core's runtime, reached through a [`Source`] so the tests inject fakes. Recycling is
//! the explorer's `delete_paths` (to the Recycle Bin, with Windows' own question where Settings say so, and an
//! Undo), reached through [`Effects`], which also reloads the folder and shows a file in its folder.
//!
//! **Virtualized.** Groups and their files are flattened into one list of equally tall rows (a group's header,
//! then a row per copy), so a drive with hundreds of groups draws only what is on screen.
//!
//! **Not the same as the TypeScript:** core reports no progress while it scans (it answers once), so the
//! scanning state is a spinner with Stop, as the TypeScript's was.

use crate::kit::prelude::*;
use crate::{core_bridge, explorer, i18n, t, theme};
use glint_model::format::{format_count, format_date, format_duration, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

pub use glint_core::dedup::{DedupReport, DuplicateFile, DuplicateGroup, HardlinkResult};

/// How a group's extra copies are replaced: by block clones on a drive that has them (ReFS, a Dev Drive), else
/// by hard links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    HardLink,
    Clone,
}

impl Method {
    /// The button's word, and the confirmation's OK.
    fn verb(self) -> SharedString {
        match self {
            Self::HardLink => t!("Hard-link"),
            Self::Clone => t!("Clone"),
        }
    }

    /// The "all" button's word.
    fn verb_all(self, files: usize) -> SharedString {
        match self {
            Self::HardLink => t!("Hard-link all ({n})", n = format_count(files)),
            Self::Clone => t!("Clone all ({n})", n = format_count(files)),
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::HardLink => IconName::Link2,
            Self::Clone => IconName::CopyPlus,
        }
    }

    /// The button's tooltip.
    fn hint(self) -> SharedString {
        match self {
            Self::HardLink => t!("Replace the other copies with hard links to the one you keep: the paths stay, the space is freed, and they become one file"),
            Self::Clone => t!("Replace the other copies with block clones of the one you keep: the paths stay and the space is freed, and each stays a file of its own"),
        }
    }

    /// The title of the failure notice.
    fn failed(self) -> SharedString {
        match self {
            Self::HardLink => t!("Couldn't hard-link the duplicates"),
            Self::Clone => t!("Couldn't clone the duplicates"),
        }
    }

    /// The failure notice when one named group stopped the run.
    fn failed_for(self, name: &str) -> SharedString {
        match self {
            Self::HardLink => t!("Couldn't hard-link “{name}”", name = name),
            Self::Clone => t!("Couldn't clone “{name}”", name = name),
        }
    }
}

// ---- Where the work comes from -------------------------------------------------------------------------------

type Work<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// What the dialog asks glint-core: glint-core's in the app, fakes in tests.
#[derive(Clone)]
pub struct Source {
    /// Finds the duplicates under a folder (`find_duplicates`, files of 1 byte and up).
    pub scan: Arc<dyn Fn(String) -> Work<DedupReport> + Send + Sync>,
    /// Stops the scan under way (`cancel_dedup`); its answer is then the error "cancelled".
    pub stop: Arc<dyn Fn() + Send + Sync>,
    /// Replaces copies with links or clones of the one kept: `(method, keep, copies)`.
    pub link: Arc<dyn Fn(Method, String, Vec<String>) -> Work<HardlinkResult> + Send + Sync>,
}

impl Source {
    /// glint-core's duplicate finder.
    pub fn core() -> Self {
        Self {
            scan: Arc::new(|root| Box::pin(glint_core::dedup::find_duplicates(root, Some(1)))),
            stop: Arc::new(glint_core::dedup::cancel_dedup),
            link: Arc::new(|method, keep, copies| match method {
                Method::HardLink => Box::pin(glint_core::dedup::hardlink_duplicates(keep, copies)),
                Method::Clone => Box::pin(glint_core::dedup::clone_duplicates(keep, copies)),
            }),
        }
    }
}

/// What happens to a path, given the app.
type OnPath = Rc<dyn Fn(&str, &mut App)>;
/// Recycling paths: the explorer's task that ends when it is done.
type Recycler = Rc<dyn Fn(Vec<String>, &mut App) -> Task<()>>;

/// What the dialog does to the rest of the app: the explorer's, in the app; recorders in tests.
#[derive(Clone)]
pub struct Effects {
    /// Moves paths to the Recycle Bin through the explorer (`delete_paths`: Undo, and Windows' question where
    /// Settings say so). The task ends when that is done or was cancelled.
    pub recycle: Recycler,
    /// Reads the folder on screen again (`reload()`).
    pub reload: Rc<dyn Fn(&mut App)>,
    /// Shows a file in its folder, selected (`revealInApp`).
    pub reveal: OnPath,
}

impl Effects {
    /// The explorer's.
    pub fn app() -> Self {
        Self {
            recycle: Rc::new(|paths, cx| match explorer::try_explorer(cx) {
                Some(explorer) => explorer.update(cx, |explorer, cx| explorer.delete_paths(paths, false, None, cx)),
                None => Task::ready(()),
            }),
            reload: Rc::new(|cx| {
                if let Some(explorer) = explorer::try_explorer(cx) {
                    explorer.update(cx, |explorer, cx| explorer.reload(None, cx).detach());
                }
            }),
            reveal: Rc::new(|path, cx| {
                if let Some(explorer) = explorer::try_explorer(cx) {
                    let options = explorer::NavOptions::default().selecting(path.to_string());
                    explorer.update(cx, |explorer, cx| explorer.navigate(parent_path(path), options, cx));
                }
            }),
        }
    }
}

// ---- The model ----------------------------------------------------------------------------------------------------

/// Where the scan stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Comparing sizes, then contents.
    Loading,
    /// Stopped by the person before it finished.
    Stopped,
    /// Core couldn't scan, with the reason.
    Failed(SharedString),
    /// `report` is the answer.
    Ready,
}

/// What the model tells the dialog to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DuplicatesEvent {
    /// Something worked: the sentence for a success notice.
    Done(SharedString),
    /// Something didn't: the notice's title and the reason.
    Failed { title: SharedString, reason: SharedString },
}

/// One line of the flattened list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A group's header: its sizes and its buttons.
    Group(usize),
    /// A copy: (group, file).
    File(usize, usize),
}

/// The duplicates found under a folder, which copy of each group is kept, and the work in progress.
pub struct Duplicates {
    root: String,
    source: Source,
    effects: Effects,
    state: State,
    report: Option<DedupReport>,
    rows: Vec<Row>,
    /// Per group (its hash), the path chosen to keep. Unset: the first (oldest) copy.
    kept: HashMap<String, String>,
    /// Linking or recycling is under way.
    busy: bool,
    /// The scan asked for last: an older one that answers late is ignored.
    run: u64,
}

impl EventEmitter<DuplicatesEvent> for Duplicates {}

impl Duplicates {
    /// A model for `root`. It scans nothing until [`Duplicates::scan`].
    pub fn new(root: String, source: Source, effects: Effects) -> Self {
        Self {
            root,
            source,
            effects,
            state: State::Loading,
            report: None,
            rows: Vec::new(),
            kept: HashMap::new(),
            busy: false,
            run: 0,
        }
    }

    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn report(&self) -> Option<&DedupReport> {
        self.report.as_ref()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn busy(&self) -> bool {
        self.busy
    }

    /// Clones where the drive has them, hard links elsewhere.
    pub fn method(&self) -> Method {
        if self.report.as_ref().is_some_and(|r| r.clones) {
            Method::Clone
        } else {
            Method::HardLink
        }
    }

    /// Scans the folder, in the background. A scan still running is left to finish and ignored.
    pub fn scan(&mut self, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        self.state = State::Loading;
        cx.notify();
        let read = (self.source.scan)(self.root.clone());
        cx.spawn(async move |this, cx| {
            // Two layers: the core task may panic (outer Err), the scan may fail or be stopped (inner Err).
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                match result {
                    Ok(report) => {
                        this.rows = flatten(&report);
                        this.report = Some(report);
                        this.state = State::Ready;
                    }
                    Err(reason) if reason == "cancelled" => this.state = State::Stopped,
                    Err(reason) => this.state = State::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Stops the scan under way (the Stop button).
    pub fn stop(&self) {
        (self.source.stop)();
    }

    /// The path kept in `group`: the one chosen if it is still in the group, else the first.
    pub fn kept_for(&self, group: &DuplicateGroup) -> String {
        match self.kept.get(&group.hash) {
            Some(chosen) if group.files.iter().any(|f| &f.path == chosen) => chosen.clone(),
            _ => group.files.first().map(|f| f.path.clone()).unwrap_or_default(),
        }
    }

    /// The copies of `group` that aren't kept.
    pub fn dups_for(&self, group: &DuplicateGroup) -> Vec<String> {
        let keep = self.kept_for(group);
        group
            .files
            .iter()
            .filter(|f| f.path != keep)
            .map(|f| f.path.clone())
            .collect()
    }

    /// Keeps `path` of group `group` instead.
    pub fn keep(&mut self, group: usize, path: String, cx: &mut Context<Self>) {
        let Some(hash) = self
            .report
            .as_ref()
            .and_then(|r| r.groups.get(group))
            .map(|g| g.hash.clone())
        else {
            return;
        };
        self.kept.insert(hash, path);
        cx.notify();
    }

    fn group(&self, ix: usize) -> Option<&DuplicateGroup> {
        self.report.as_ref().and_then(|r| r.groups.get(ix))
    }

    /// The question before replacing group `ix`'s other copies: (title, message, OK). `None` if there are none.
    pub fn link_prompt(&self, ix: usize) -> Option<(SharedString, SharedString, SharedString)> {
        let group = self.group(ix)?;
        let copies = self.dups_for(group).len();
        if copies == 0 {
            return None;
        }
        let method = self.method();
        let keep = self.kept_for(group);
        let name = base_name(&keep).to_string();
        let what = if copies == 1 {
            t!("the other copy")
        } else {
            t!("the {n} other copies", n = copies)
        };
        let freed = format_size(group.wasted_bytes);
        let message = match method {
            Method::HardLink => t!(
                "Replace {what} of “{name}” with hard links to it?\n\nEvery path stays where it is and {freed} is freed, but they all become one file: changing any of them changes them all, and they share one set of dates and attributes. Each copy is compared byte for byte first.",
                what = what,
                name = name,
                freed = freed
            ),
            Method::Clone => t!(
                "Replace {what} of “{name}” with block clones of it?\n\nEvery path stays where it is and {freed} is freed: the clones share its space on this drive until one of them changes, and each stays a file of its own, with its own dates. Each copy is compared byte for byte first, and its clone again before it takes its place.",
                what = what,
                name = name,
                freed = freed
            ),
        };
        let title = match method {
            Method::HardLink => t!("Hard-link duplicates"),
            Method::Clone => t!("Clone duplicates"),
        };
        Some((title, message, method.verb()))
    }

    /// The question before replacing every group's other copies. `None` if there are no groups.
    pub fn link_all_prompt(&self) -> Option<(SharedString, SharedString, SharedString)> {
        let report = self.report.as_ref().filter(|r| !r.groups.is_empty())?;
        let method = self.method();
        let files = format_count(report.total_duplicate_files);
        let groups = format_count(report.groups.len());
        let freed = format_size(report.total_wasted_bytes);
        let message = match method {
            Method::HardLink => t!(
                "Replace all {files} duplicate files in {groups} groups with hard links to the copy you keep?\n\nEvery path stays where it is and {freed} is freed, but each group becomes one file: changing any copy changes them all. Each copy is compared byte for byte first.",
                files = files,
                groups = groups,
                freed = freed
            ),
            Method::Clone => t!(
                "Replace all {files} duplicate files in {groups} groups with block clones of the copy you keep?\n\nEvery path stays where it is and {freed} is freed: each group's clones share its space until one changes, and each stays a file of its own. Each copy is compared byte for byte first.",
                files = files,
                groups = groups,
                freed = freed
            ),
        };
        let title = match method {
            Method::HardLink => t!("Hard-link all duplicates"),
            Method::Clone => t!("Clone all duplicates"),
        };
        let ok = match method {
            Method::HardLink => t!("Hard-link all"),
            Method::Clone => t!("Clone all"),
        };
        Some((title, message, ok))
    }

    /// Replaces group `ix`'s other copies with links or clones of the kept one, then scans again.
    pub fn link_group(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(group) = self.group(ix) else { return };
        let keep = self.kept_for(group);
        let copies = self.dups_for(group);
        if copies.is_empty() || self.busy {
            return;
        }
        let method = self.method();
        self.busy = true;
        cx.notify();
        let work = (self.source.link)(method, keep, copies);
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(done) => {
                        let text = match method {
                            Method::HardLink => t!(
                                "Linked {files} · reclaimed {size} in {time}",
                                files = plural(done.linked, "file"),
                                size = format_size(done.reclaimed_bytes),
                                time = format_duration(done.elapsed_ms)
                            ),
                            Method::Clone => t!(
                                "Cloned {files} · reclaimed {size} in {time}",
                                files = plural(done.linked, "file"),
                                size = format_size(done.reclaimed_bytes),
                                time = format_duration(done.elapsed_ms)
                            ),
                        };
                        cx.emit(DuplicatesEvent::Done(text));
                        this.changed(cx);
                    }
                    Err(reason) => cx.emit(DuplicatesEvent::Failed {
                        title: method.failed(),
                        reason: reason.into(),
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Replaces every group's other copies, one group after another; stops at the first that fails.
    pub fn link_all(&mut self, cx: &mut Context<Self>) {
        let Some(report) = self.report.as_ref().filter(|r| !r.groups.is_empty()) else {
            return;
        };
        if self.busy {
            return;
        }
        let jobs: Vec<(String, String, Vec<String>)> = report
            .groups
            .iter()
            .map(|g| {
                let keep = self.kept_for(g);
                (
                    g.files.first().map(|f| f.name.clone()).unwrap_or_default(),
                    keep,
                    self.dups_for(g),
                )
            })
            .collect();
        let method = self.method();
        let source = self.source.clone();
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut linked = 0usize;
            let mut reclaimed = 0u64;
            let mut failure: Option<(String, String)> = None;
            for (name, keep, copies) in jobs {
                let work = (source.link)(method, keep, copies);
                match core_bridge::run(work).await.and_then(|inner| inner) {
                    Ok(done) => {
                        linked += done.linked;
                        reclaimed += done.reclaimed_bytes;
                    }
                    Err(reason) => {
                        failure = Some((name, reason));
                        break;
                    }
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Some((name, reason)) = failure {
                    cx.emit(DuplicatesEvent::Failed {
                        title: method.failed_for(&name),
                        reason: reason.into(),
                    });
                }
                if linked > 0 {
                    let text = match method {
                        Method::HardLink => t!(
                            "Linked {files} · reclaimed {size}",
                            files = plural(linked, "file"),
                            size = format_size(reclaimed)
                        ),
                        Method::Clone => t!(
                            "Cloned {files} · reclaimed {size}",
                            files = plural(linked, "file"),
                            size = format_size(reclaimed)
                        ),
                    };
                    cx.emit(DuplicatesEvent::Done(text));
                    this.changed(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Moves group `ix`'s other copies to the Recycle Bin, then scans again.
    pub fn recycle_group(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(group) = self.group(ix) else { return };
        let copies = self.dups_for(group);
        self.recycle(copies, cx);
    }

    /// Moves every group's other copies to the Recycle Bin, then scans again.
    pub fn recycle_all(&mut self, cx: &mut Context<Self>) {
        let Some(report) = &self.report else { return };
        let copies: Vec<String> = report.groups.iter().flat_map(|g| self.dups_for(g)).collect();
        self.recycle(copies, cx);
    }

    fn recycle(&mut self, copies: Vec<String>, cx: &mut Context<Self>) {
        if copies.is_empty() || self.busy {
            return;
        }
        self.busy = true;
        cx.notify();
        // The explorer does the deleting (Undo, Windows' question, its own notices); we wait for it, then look again.
        let recycle = self.effects.recycle.clone();
        let task = recycle(copies, cx);
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.changed(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Files changed: the folder on screen is read again and this scan starts over.
    fn changed(&mut self, cx: &mut Context<Self>) {
        let reload = self.effects.reload.clone();
        reload(cx);
        self.scan(cx);
    }

    /// Shows `path` in its folder and leaves the dialog's work to the caller (it closes first).
    fn reveal(&self, path: &str, cx: &mut App) {
        let reveal = self.effects.reveal.clone();
        reveal(path, cx);
    }
}

impl Drop for Duplicates {
    /// Closing the dialog stops a scan still running.
    fn drop(&mut self) {
        if self.state == State::Loading {
            (self.source.stop)();
        }
    }
}

/// Groups then their files, in one list of equally tall rows.
pub fn flatten(report: &DedupReport) -> Vec<Row> {
    let mut rows = Vec::with_capacity(report.groups.len() + report.total_duplicate_files + report.groups.len());
    for (g, group) in report.groups.iter().enumerate() {
        rows.push(Row::Group(g));
        rows.extend((0..group.files.len()).map(|f| Row::File(g, f)));
    }
    rows
}

/// A path from the folder scanned on (`sub\beta.bin`), so the name isn't what gets cut off (`inRoot`).
pub fn in_root(root: &str, path: &str) -> String {
    let base = root.trim_end_matches(['\\', '/']);
    let prefix = format!("{}\\", base.to_lowercase());
    if path.to_lowercase().starts_with(&prefix) {
        // The lowercase form can differ in length for a few scripts; the cut is taken from the original only
        // when the prefix has the same number of chars.
        let cut: usize = path.chars().take(base.chars().count() + 1).map(char::len_utf8).sum();
        path.get(cut..).unwrap_or(path).to_string()
    } else {
        path.to_string()
    }
}

// ---- The dialog -----------------------------------------------------------------------------------------------------

/// Each row is this tall, so the list can be virtualized.
const ROW_HEIGHT: f32 = 8.;
/// The list is this tall (in 4 px units).
const LIST_HEIGHT: f32 = 92.;

/// Duplicate files under a folder.
pub struct DuplicatesDialog {
    model: Entity<Duplicates>,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl DuplicatesDialog {
    pub fn new(model: Entity<Duplicates>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // F5 is the Refresh button's key, as in the web app's dialogs.
        super::refresh_on_f5(cx, |this, _, cx| {
            this.model.update(cx, |model, cx| {
                if !model.busy() && *model.state() != State::Loading {
                    model.scan(cx)
                }
            });
        });
        let redraw = cx.observe(&model, |_, _, cx| cx.notify());
        let notices = cx.subscribe_in(
            &model,
            window,
            |_, _, event: &DuplicatesEvent, window, cx| match event {
                DuplicatesEvent::Done(text) => window.push_notification(Notification::success(text.clone()), cx),
                DuplicatesEvent::Failed { title, reason } => {
                    window.push_notification(Notification::error(reason.clone()).title(title.clone()), cx)
                }
            },
        );
        model.update(cx, |model, cx| model.scan(cx));
        Self {
            model,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: vec![redraw, notices],
        }
    }

    /// The scanned-and-compared line under the header.
    fn stats(report: &DedupReport, t: &theme::Tokens) -> impl IntoElement {
        let mut line = t!(
            "{files} scanned → {sizes} same-size → {edge} same start and end → {groups}",
            files = plural(report.files_scanned, "file"),
            sizes = format_count(report.size_candidates),
            edge = format_count(report.edge_candidates),
            groups = plural(report.groups.len(), "duplicate group")
        )
        .to_string();
        if report.hardlinks_skipped > 0 {
            line.push_str(&format!(
                " ({})",
                t!(
                    "{n} already hard-linked, skipped",
                    n = format_count(report.hardlinks_skipped)
                )
            ));
        }
        if report.clones {
            // On ReFS or a Dev Drive a copy can be a clone of another file: it takes no space of its own until
            // one of them changes.
            line.push_str(&format!(
                " · {}",
                t!("this drive clones files, so copies can share their space")
            ));
        }
        align_row(2.)
            .id("duplicates-stats")
            .role(Role::Group)
            .aria_label(line.clone())
            .test_support()
            .when(report.clones, |this| {
                this.tooltip(|window, cx| {
                    crate::kit::tooltip(
                        t!("On ReFS or a Dev Drive, a copy can be a clone of another file: it takes no space of its own until one of them changes"),
                        window,
                        cx,
                    )
                })
            })
            .flex_wrap()
            .px(space(3.5))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .bg(t.muted.opacity(0.4))
            .text_size(theme::size::XXS)
            .text_color(t.muted_foreground)
            .child(icon(IconName::Sparkles).size_3().text_color(t.primary).flex_none())
            .child(div().min_w_0().child(line))
    }

    /// A group's header row.
    fn group_row(&self, ix: usize, group_ix: usize, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.read(cx);
        let Some(group) = model.group(group_ix) else {
            return div().into_any_element();
        };
        let method = model.method();
        let busy = model.busy();
        let name = group.files.first().map(|f| f.name.clone()).unwrap_or_default();
        let label = t!(
            "{size} each · {wasted} duplicate",
            size = format_size(group.size),
            wasted = format_size(group.wasted_bytes)
        );
        let hash: String = group.hash.chars().take(12).collect();
        let link_label = t!("{verb} the other copies of “{name}”", verb = method.verb(), name = name);
        let recycle_label = t!("Recycle the other copies of “{name}”", name = name);
        let mono = theme::mono_font(cx);

        row_start()
            .id(("duplicates-group", ix))
            .role(Role::Group)
            .aria_label(label.clone())
            .test_support()
            .h(space(ROW_HEIGHT))
            .items_center()
            .justify_between()
            .gap(space(2.))
            .px(space(3.))
            .mt(space(0.))
            .border_1()
            .border_color(t.border)
            .rounded_t(theme::size::RADIUS_LG)
            .bg(t.muted.opacity(0.4))
            .text_size(theme::size::XS)
            .child(
                align_row(2.)
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("{size} each", size = format_size(group.size))),
                    )
                    .child(div().text_color(t.muted_foreground).child("·"))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.warning)
                            .child(t!("{size} duplicate", size = format_size(group.wasted_bytes))),
                    )
                    .child(
                        div()
                            .px(space(1.5))
                            .rounded(theme::size::RADIUS_SM)
                            .border_1()
                            .border_color(t.border)
                            .bg(t.background)
                            .font_family(mono)
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(format!("#{hash}")),
                    ),
            )
            .child(
                align_row(1.)
                    .flex_none()
                    .child(
                        Button::new(("duplicates-link", group_ix))
                            .outline()
                            .xsmall()
                            .icon(method.icon())
                            .label(method.verb())
                            .tooltip(method.hint())
                            .accessibility_label(link_label)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| this.ask_link(group_ix, window, cx))),
                    )
                    .child(
                        Button::new(("duplicates-recycle", group_ix))
                            .outline()
                            .xsmall()
                            .icon(IconName::Trash)
                            .label(t!("Recycle"))
                            .tooltip(t!("Move non-kept copies to the Recycle Bin"))
                            .accessibility_label(recycle_label)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.model.update(cx, |model, cx| model.recycle_group(group_ix, cx));
                            })),
                    ),
            )
            .into_any_element()
    }

    /// A copy's row: pick it as the one to keep, its date, and a button to show it.
    fn file_row(
        &self,
        ix: usize,
        group_ix: usize,
        file_ix: usize,
        last: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let model = self.model.read(cx);
        let Some(file) = model.group(group_ix).and_then(|g| g.files.get(file_ix)) else {
            return div().into_any_element();
        };
        let kept = model.group(group_ix).is_some_and(|g| model.kept_for(g) == file.path);
        let relative = in_root(model.root(), &file.path);
        let path = file.path.clone();
        let modified = file.modified;
        let mono = theme::mono_font(cx);
        let (badge, badge_fill, badge_text) = if kept {
            (t!("Keep"), t.success.opacity(0.15), t.success)
        } else {
            (t!("Duplicate"), t.muted, t.muted_foreground)
        };
        let hint = if kept {
            t!("This copy will be kept")
        } else {
            t!("Click to keep this copy instead")
        };
        let keep_path = path.clone();
        let reveal_path = path.clone();

        row_start()
            .id(("duplicates-copy", ix))
            // The copies of a group are the choices of one question (`role="radiogroup"` of the TypeScript).
            .role(Role::RadioGroup)
            .aria_label(t!("The copy to keep"))
            .test_support()
            .h(space(ROW_HEIGHT))
            .items_center()
            .gap(space(2.))
            .px(space(3.))
            .border_x_1()
            .border_b_1()
            .border_color(t.border)
            .when(last, |this| this.rounded_b(theme::size::RADIUS_LG))
            .when(kept, |this| this.bg(t.primary.opacity(0.04)))
            .text_size(theme::size::XS)
            .child(
                Button::new(("duplicates-keep", ix))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(t.accent.opacity(0.))
                            .foreground(t.foreground)
                            .hover(t.accent)
                            .active(t.accent),
                    )
                    .role(Role::RadioButton)
                    .accessibility_label(t!("Keep {path}", path = relative.clone()))
                    .toggled(kept)
                    .tooltip(hint)
                    .flex_1()
                    .min_w_0()
                    .justify_start()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let path = keep_path.clone();
                        this.model.update(cx, |model, cx| model.keep(group_ix, path, cx));
                    }))
                    .child(
                        align_row(2.)
                            .min_w_0()
                            .child(
                                div()
                                    .flex_none()
                                    .px(space(1.5))
                                    .py(space(0.5))
                                    .rounded(theme::size::RADIUS_SM)
                                    .bg(badge_fill)
                                    .text_color(badge_text)
                                    .text_size(theme::size::XXS)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(badge.to_uppercase()),
                            )
                            .child(div().min_w_0().truncate().font_family(mono).child(isolate(relative))),
                    ),
            )
            .when(modified > 0, |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_size(theme::size::XXS)
                        .text_color(t.muted_foreground)
                        .child(format_date(modified, None, i18n::language())),
                )
            })
            .child(
                Button::new(("duplicates-reveal", ix))
                    .ghost()
                    .xsmall()
                    .icon(IconName::FolderInput)
                    .tooltip(t!("Open file location"))
                    .accessibility_label(t!("Open file location of {path}", path = path))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // The dialog closes first, then the folder opens with the file selected.
                        window.close_dialog(cx);
                        let path = reveal_path.clone();
                        this.model.update(cx, |model, cx| model.reveal(&path, cx));
                    })),
            )
            .into_any_element()
    }

    /// Asks before replacing a group's copies with links or clones.
    fn ask_link(&mut self, group_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((title, message, ok)) = self.model.read(cx).link_prompt(group_ix) else {
            return;
        };
        let model = self.model.clone();
        super::confirm(window, cx, title, message, ok, false, move |_, cx| {
            model.update(cx, |model, cx| model.link_group(group_ix, cx));
        });
    }

    /// Asks before replacing every group's copies.
    fn ask_link_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((title, message, ok)) = self.model.read(cx).link_all_prompt() else {
            return;
        };
        let model = self.model.clone();
        super::confirm(window, cx, title, message, ok, false, move |_, cx| {
            model.update(cx, |model, cx| model.link_all(cx));
        });
    }

    /// A centred message in the list's place: an icon, a headline, and optionally a line and a button.
    fn message(id: &'static str, role: Role, headline: SharedString, tone: Hsla) -> impl ParentElement + IntoElement {
        col()
            .id(id)
            .role(role)
            .aria_label(headline)
            .test_support()
            .size_full()
            .items_center()
            .justify_center()
            .gap(space(2.))
            .text_center()
            .text_color(tone)
    }
}

impl Render for DuplicatesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let model = self.model.read(cx);
        let state = model.state().clone();
        let busy = model.busy();
        let loading = state == State::Loading;
        let root = model.root().to_string();
        let report = model.report().cloned();
        let rows = model.rows().to_vec();
        let method = model.method();
        let mono = theme::mono_font(cx);

        let wasted = report.as_ref().map_or(0, |r| r.total_wasted_bytes);
        let header = align_row(3.)
            .items_center()
            .justify_between()
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
            .child(
                align_row(2.)
                    .flex_none()
                    .when(wasted > 0, |this| {
                        this.child(
                            div()
                                .id("duplicates-reclaimable")
                                .role(Role::Status)
                                .aria_label(t!("{size} reclaimable", size = format_size(wasted)))
                                .test_support()
                                .px(space(2.))
                                .py(space(0.5))
                                .rounded(theme::size::RADIUS)
                                .bg(t.warning.opacity(0.15))
                                .text_color(t.warning)
                                .text_size(theme::size::XS)
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t!("{size} reclaimable", size = format_size(wasted))),
                        )
                    })
                    .child(
                        Button::new("duplicates-refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::RotateCw)
                            .tooltip(t!("Refresh (F5)"))
                            .accessibility_label(t!("Refresh"))
                            .loading(loading || busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |model, cx| model.scan(cx));
                            })),
                    ),
            );

        let body: AnyElement = match &state {
            State::Loading => Self::message(
                "duplicates-loading",
                Role::Status,
                t!("Comparing sizes, then contents…"),
                t.muted_foreground,
            )
            .child(Spinner::new())
            .child(t!("Comparing sizes, then contents…"))
            .child(
                Button::new("duplicates-stop")
                    .outline()
                    .small()
                    .icon(IconName::Square)
                    .label(t!("Stop"))
                    .on_click(cx.listener(|this, _, _, cx| this.model.read(cx).stop())),
            )
            .into_any_element(),
            State::Stopped => Self::message(
                "duplicates-stopped",
                Role::Status,
                t!("Stopped before it finished"),
                t.muted_foreground,
            )
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.foreground)
                    .child(t!("Stopped before it finished")),
            )
            .child(
                Button::new("duplicates-again")
                    .outline()
                    .small()
                    .icon(IconName::RotateCw)
                    .label(t!("Scan again"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |model, cx| model.scan(cx));
                    })),
            )
            .into_any_element(),
            State::Failed(reason) => {
                let headline = t!("Couldn't scan for duplicates");
                Self::message("duplicates-error", Role::Alert, headline.clone(), t.muted_foreground)
                    .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .child(headline),
                    )
                    .child(
                        div()
                            .id("duplicates-error-reason")
                            .role(Role::Status)
                            .aria_label(reason.clone())
                            .test_support()
                            .max_w(space(112.))
                            .text_size(theme::size::XS)
                            .child(reason.clone()),
                    )
                    .into_any_element()
            }
            State::Ready if rows.is_empty() => {
                let report = report.clone();
                let headline = t!("No duplicate files found");
                let checked = report
                    .as_ref()
                    .map(|r| {
                        let mut text = t!(
                            "Checked {files} in {time}.",
                            files = plural(r.files_scanned, "file"),
                            time = format_duration(r.elapsed_ms)
                        )
                        .to_string();
                        if r.hardlinks_skipped > 0 {
                            text.push_str(&format!(
                                " ({})",
                                t!(
                                    "{n} already-hardlinked files were skipped.",
                                    n = format_count(r.hardlinks_skipped)
                                )
                            ));
                        }
                        text
                    })
                    .unwrap_or_default();
                Self::message("duplicates-empty", Role::Status, headline.clone(), t.muted_foreground)
                    .child(icon(IconName::CircleCheck).size_8().text_color(t.success))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.foreground)
                            .child(headline),
                    )
                    .child(
                        div()
                            .id("duplicates-checked")
                            .role(Role::Status)
                            .aria_label(checked.clone())
                            .test_support()
                            .max_w(space(96.))
                            .text_size(theme::size::XS)
                            .child(checked),
                    )
                    .into_any_element()
            }
            State::Ready => {
                let colors = t.clone();
                let count = rows.len();
                let list = uniform_list(
                    "duplicates-list",
                    count,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|ix| {
                                let row = *rows.get(ix)?;
                                Some(match row {
                                    Row::Group(g) => this.group_row(ix, g, &colors, cx),
                                    Row::File(g, f) => {
                                        let last = !matches!(rows.get(ix + 1), Some(Row::File(..)));
                                        this.file_row(ix, g, f, last, &colors, cx)
                                    }
                                })
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full();
                div()
                    .size_full()
                    .child(
                        div()
                            .id("duplicates-groups")
                            .role(Role::List)
                            .aria_label(t!("Duplicate groups"))
                            .test_support()
                            .size_full()
                            .child(list),
                    )
                    .child(Scrollbar::vertical(&self.scroll).viewport_from_layout())
                    .into_any_element()
            }
        };

        let summary = report
            .as_ref()
            .map(|r| {
                t!(
                    "{groups} ({dups}) in {time}",
                    groups = plural(r.groups.len(), "group"),
                    dups = plural(r.total_duplicate_files, "duplicate"),
                    time = format_duration(r.elapsed_ms)
                )
            })
            .unwrap_or_default();
        let many = report
            .as_ref()
            .filter(|r| r.groups.len() > 1)
            .map(|r| r.total_duplicate_files);
        let footer = align_row(3.)
            .items_center()
            .justify_between()
            .pt(space(2.))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(
                align_row(1.)
                    .min_w_0()
                    .child(icon(IconName::Zap).size_3().text_color(t.primary).flex_none())
                    .child(div().min_w_0().truncate().child(summary)),
            )
            .children(many.map(|files| {
                align_row(1.5)
                    .flex_none()
                    .child(
                        Button::new("duplicates-link-all")
                            .ghost()
                            .small()
                            .icon(method.icon())
                            .label(method.verb_all(files))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| this.ask_link_all(window, cx))),
                    )
                    .child(
                        Button::new("duplicates-recycle-all")
                            .ghost()
                            .small()
                            .icon(IconName::Trash)
                            .label(t!("Recycle all"))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |model, cx| model.recycle_all(cx));
                            })),
                    )
            }));

        col()
            .id("duplicates")
            .role(Role::Group)
            .aria_label(title_for(&root))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer sits outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(header)
            .children(report.as_ref().map(|r| Self::stats(r, &t)))
            .child(div().relative().h(space(LIST_HEIGHT)).child(body))
            .child(footer)
    }
}

/// The dialog's title: "Duplicate files in “Photos”".
pub fn title_for(root: &str) -> SharedString {
    let name = base_name(root);
    t!(
        "Duplicate files in “{name}”",
        name = if name.is_empty() { root } else { name }
    )
}

/// Opens the finder for `root` in `window` and starts scanning it (the menus' `Duplicates`, This PC's
/// `FindDuplicates { path }`). Nothing opens for an empty path.
pub fn open(window: &mut Window, cx: &mut App, root: String) {
    open_with(window, cx, root, Source::core(), Effects::app());
}

/// [`open`] with the work and the effects given (tests pass fakes).
pub fn open_with(window: &mut Window, cx: &mut App, root: String, source: Source, effects: Effects) {
    if root.is_empty() {
        return;
    }
    let title = title_for(&root);
    let model = cx.new(|_| Duplicates::new(root, source, effects));
    let view = cx.new(|cx| DuplicatesDialog::new(model, window, cx));
    super::show(window, cx, title, px(768.), view);
}

// ---- Tests -----------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    fn file(path: &str, modified: i64) -> DuplicateFile {
        DuplicateFile {
            path: path.into(),
            name: base_name(path).into(),
            modified,
        }
    }

    fn group(hash: &str, size: u64, paths: &[&str]) -> DuplicateGroup {
        DuplicateGroup {
            hash: hash.into(),
            size,
            wasted_bytes: size * (paths.len() as u64 - 1),
            files: paths.iter().map(|p| file(p, 1_700_000_000_000)).collect(),
        }
    }

    fn report(groups: Vec<DuplicateGroup>, clones: bool) -> DedupReport {
        DedupReport {
            root: "C:\\Photos".into(),
            total_wasted_bytes: groups.iter().map(|g| g.wasted_bytes).sum(),
            total_duplicate_files: groups.iter().map(|g| g.files.len() - 1).sum(),
            groups,
            files_scanned: 120,
            size_candidates: 9,
            edge_candidates: 4,
            hardlinks_skipped: 2,
            clones,
            elapsed_ms: 42.0,
        }
    }

    fn two_groups(clones: bool) -> DedupReport {
        report(
            vec![
                group(
                    "abcdef0123456789",
                    2048,
                    &["C:\\Photos\\a.jpg", "C:\\Photos\\sub\\b.jpg", "C:\\Photos\\c.jpg"],
                ),
                group("1111222233334444", 1024, &["C:\\Photos\\x.txt", "C:\\Photos\\y.txt"]),
            ],
            clones,
        )
    }

    /// What the fakes were asked.
    #[derive(Default)]
    struct Log {
        scans: AtomicUsize,
        stops: AtomicUsize,
        links: Mutex<Vec<(Method, String, Vec<String>)>>,
        recycled: Mutex<Vec<Vec<String>>>,
        reloads: AtomicUsize,
        revealed: Mutex<Vec<String>>,
    }

    /// A source answering every scan with `answer`, linking successfully unless `link_fails`.
    fn fake(answer: Result<DedupReport, String>, link_fails: bool) -> (Source, Arc<Log>) {
        let log = Arc::new(Log::default());
        let (a, b, c) = (log.clone(), log.clone(), log.clone());
        let source = Source {
            scan: Arc::new(move |_| {
                a.scans.fetch_add(1, Ordering::SeqCst);
                let answer = answer.clone();
                Box::pin(async move { answer })
            }),
            stop: Arc::new(move || {
                b.stops.fetch_add(1, Ordering::SeqCst);
            }),
            link: Arc::new(move |method, keep, copies| {
                let n = copies.len();
                c.links.lock().unwrap().push((method, keep, copies));
                Box::pin(async move {
                    if link_fails {
                        Err("access denied".to_string())
                    } else {
                        Ok(HardlinkResult {
                            linked: n,
                            reclaimed_bytes: 4096,
                            elapsed_ms: 5.0,
                        })
                    }
                })
            }),
        };
        (source, log)
    }

    fn effects(log: &Arc<Log>) -> Effects {
        let (a, b, c) = (log.clone(), log.clone(), log.clone());
        Effects {
            recycle: Rc::new(move |paths, _| {
                a.recycled.lock().unwrap().push(paths);
                Task::ready(())
            }),
            reload: Rc::new(move |_| {
                b.reloads.fetch_add(1, Ordering::SeqCst);
            }),
            reveal: Rc::new(move |path, _| c.revealed.lock().unwrap().push(path.to_string())),
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
    fn paths_are_shown_from_the_folder_scanned_on() {
        assert_eq!(in_root("C:\\Photos", "C:\\Photos\\sub\\b.jpg"), "sub\\b.jpg");
        assert_eq!(in_root("C:\\Photos\\", "c:\\photos\\b.jpg"), "b.jpg");
        assert_eq!(in_root("C:\\Photos", "D:\\other\\b.jpg"), "D:\\other\\b.jpg");
        assert_eq!(in_root("C:\\ملفات", "C:\\ملفات\\صورة.jpg"), "صورة.jpg");
    }

    #[test]
    fn the_list_is_each_group_then_its_copies() {
        let rows = flatten(&two_groups(false));
        assert_eq!(
            rows,
            [
                Row::Group(0),
                Row::File(0, 0),
                Row::File(0, 1),
                Row::File(0, 2),
                Row::Group(1),
                Row::File(1, 0),
                Row::File(1, 1)
            ]
        );
    }

    #[gpui_kit::test]
    fn f5_scans_again_and_each_groups_copies_are_one_question(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(two_groups(true)), false);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "Keep a.jpg"));
        assert_eq!(log.scans.load(Ordering::SeqCst), 1);
        // The copies of a group are the choices of "The copy to keep".
        let role = visual.update(|window, cx| {
            window.render_frame(cx);
            window.find(("duplicates-copy", 1_usize)).role()
        });
        assert_eq!(role, Some(Role::RadioGroup));
        // F5 is the Refresh button's key, as in the web app's dialogs.
        visual.update(|window, cx| window.press("f5", cx));
        settle(&mut visual, "the second scan", |_| {
            log.scans.load(Ordering::SeqCst) == 2
        });
        settle(&mut visual, "the list again", |cx| has(cx, "Keep a.jpg"));
    }

    #[gpui_kit::test]
    fn scans_in_the_background_then_lists_groups_with_labelled_copies(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(two_groups(false)), false);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        // Straight after opening it is scanning: a status line and Stop, not a blank dialog.
        assert_eq!(
            role_of(&mut visual, "Comparing sizes, then contents…"),
            Some(Role::Status)
        );
        settle(&mut visual, "the list", |cx| has(cx, "Keep a.jpg"));
        assert_eq!(log.scans.load(Ordering::SeqCst), 1);

        assert_eq!(role_of(&mut visual, "Duplicate files in “Photos”"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "Keep a.jpg"), Some(Role::RadioButton));
        assert_eq!(role_of(&mut visual, "Keep sub\\b.jpg"), Some(Role::RadioButton));
        assert_eq!(
            role_of(&mut visual, "2.0 KB each · 4.0 KB duplicate"),
            Some(Role::Group)
        );
        assert_eq!(role_of(&mut visual, "5.0 KB reclaimable"), Some(Role::Status));
        assert_eq!(
            role_of(
                &mut visual,
                "120 files scanned → 9 same-size → 4 same start and end → 2 duplicate groups (2 already hard-linked, skipped)"
            ),
            Some(Role::Group)
        );
        // The first copy is the one kept until another is chosen.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_by_label("Keep a.jpg").checked(), Some(true));
            assert_eq!(window.find_by_label("Keep sub\\b.jpg").checked(), Some(false));
        });
        assert!(has(&mut visual, "Hard-link all (3)"));
        assert!(has(&mut visual, "Recycle all"));
        assert!(!has(&mut visual, "Comparing sizes, then contents…"));
    }

    #[gpui_kit::test]
    fn choosing_a_copy_to_keep_changes_what_recycle_removes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(two_groups(false)), false);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "Keep a.jpg"));

        // Keep the second copy of the first group (row 2 of the flat list).
        click(&mut visual, ("duplicates-keep", 2_usize));
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.find_by_label("Keep sub\\b.jpg").checked(), Some(true));
            assert_eq!(window.find_by_label("Keep a.jpg").checked(), Some(false));
        });
        click(&mut visual, ("duplicates-recycle", 0_usize));
        assert_eq!(
            *log.recycled.lock().unwrap(),
            vec![vec!["C:\\Photos\\a.jpg".to_string(), "C:\\Photos\\c.jpg".to_string()]]
        );
        // The folder is read again and the scan starts over.
        settle(&mut visual, "a second scan", |_| log.scans.load(Ordering::SeqCst) == 2);
        assert_eq!(log.reloads.load(Ordering::SeqCst), 1);

        // Recycle all takes every group's non-kept copies; the choice survives the new scan.
        settle(&mut visual, "the list again", |cx| has(cx, "Keep a.jpg"));
        click(&mut visual, "duplicates-recycle-all");
        let all = log.recycled.lock().unwrap().last().cloned().unwrap_or_default();
        assert_eq!(all, ["C:\\Photos\\a.jpg", "C:\\Photos\\c.jpg", "C:\\Photos\\y.txt"]);
    }

    #[gpui_kit::test]
    fn linking_asks_first_then_replaces_the_other_copies(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(two_groups(false)), false);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "Keep a.jpg"));

        click(&mut visual, ("duplicates-link", 1_usize));
        // Nothing happens until it is confirmed, and the question names the file and what changes.
        assert!(log.links.lock().unwrap().is_empty());
        assert!(has(&mut visual, "Hard-link duplicates"));
        click(&mut visual, "confirm-ok");
        settle(&mut visual, "the link", |_| !log.links.lock().unwrap().is_empty());
        assert_eq!(
            *log.links.lock().unwrap(),
            vec![(
                Method::HardLink,
                "C:\\Photos\\x.txt".to_string(),
                vec!["C:\\Photos\\y.txt".to_string()]
            )]
        );
        settle(&mut visual, "a second scan", |_| log.scans.load(Ordering::SeqCst) == 2);
        assert_eq!(log.reloads.load(Ordering::SeqCst), 1);
    }

    #[gpui_kit::test]
    fn a_drive_that_clones_offers_clones_and_link_all_runs_each_group(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(two_groups(true)), false);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "Keep a.jpg"));
        assert!(has(&mut visual, "Clone all (3)"));
        assert!(has(
            &mut visual,
            "120 files scanned → 9 same-size → 4 same start and end → 2 duplicate groups (2 already hard-linked, skipped) · this drive clones files, so copies can share their space"
        ));

        click(&mut visual, "duplicates-link-all");
        assert!(has(&mut visual, "Clone all duplicates"));
        click(&mut visual, "confirm-ok");
        settle(&mut visual, "both groups", |_| log.links.lock().unwrap().len() == 2);
        let links = log.links.lock().unwrap().clone();
        assert_eq!(links[0].0, Method::Clone);
        assert_eq!(links[0].2.len(), 2);
        assert_eq!(links[1].1, "C:\\Photos\\x.txt");
    }

    #[gpui_kit::test]
    fn a_failed_link_says_so_and_does_not_rescan(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(two_groups(false)), true);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "Keep a.jpg"));
        click(&mut visual, ("duplicates-link", 1_usize));
        click(&mut visual, "confirm-ok");
        settle(&mut visual, "the link", |_| !log.links.lock().unwrap().is_empty());
        visual.run_until_parked();
        assert_eq!(log.scans.load(Ordering::SeqCst), 1);
        assert_eq!(log.reloads.load(Ordering::SeqCst), 0);
        // The buttons work again.
        settle(&mut visual, "the buttons to work again", |cx| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window.find(("duplicates-link", 1_usize)).disabled() != Some(true)
            })
        });
    }

    #[gpui_kit::test]
    fn the_model_says_what_linking_did(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        for fails in [false, true] {
            let (source, log) = fake(Ok(two_groups(false)), fails);
            let model = visual.update(|_, cx| cx.new(|_| Duplicates::new(r"C:\Photos".into(), source, effects(&log))));
            let events = Rc::new(std::cell::RefCell::new(Vec::new()));
            let seen = events.clone();
            let _subscription = visual.update(|_, cx| {
                cx.subscribe(&model, move |_, event: &DuplicatesEvent, _| {
                    seen.borrow_mut().push(event.clone())
                })
            });
            visual.update(|_, cx| model.update(cx, |m, cx| m.scan(cx)));
            settle(&mut visual, "the scan", |cx| {
                model.read_with(cx, |m, _| m.state() == &State::Ready)
            });
            visual.update(|_, cx| model.update(cx, |m, cx| m.link_group(0, cx)));
            settle(&mut visual, "an event", |_| !events.borrow().is_empty());
            let first = events.borrow()[0].clone();
            if fails {
                assert_eq!(
                    first,
                    DuplicatesEvent::Failed {
                        title: "Couldn't hard-link the duplicates".into(),
                        reason: "access denied".into()
                    }
                );
                assert!(!model.read_with(&visual, |m, _| m.busy()));
            } else {
                assert_eq!(
                    first,
                    DuplicatesEvent::Done("Linked 2 files · reclaimed 4.0 KB in 5.0 ms".into())
                );
            }
        }
    }

    #[gpui_kit::test]
    fn stop_ends_the_scan_and_scan_again_starts_it_over(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let log = Arc::new(Log::default());
        // A scan that waits to be stopped.
        let (stop_tx, stop_rx) = async_channel::unbounded::<()>();
        let (a, b) = (log.clone(), log.clone());
        let source = Source {
            scan: Arc::new(move |_| {
                a.scans.fetch_add(1, Ordering::SeqCst);
                let stop_rx = stop_rx.clone();
                Box::pin(async move {
                    let _ = stop_rx.recv().await;
                    Err("cancelled".to_string())
                })
            }),
            stop: Arc::new(move || {
                b.stops.fetch_add(1, Ordering::SeqCst);
                let _ = stop_tx.try_send(());
            }),
            link: fake(Ok(two_groups(false)), false).0.link,
        };
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        visual.run_until_parked();
        assert!(has(&mut visual, "Stop"));
        click(&mut visual, "duplicates-stop");
        settle(&mut visual, "the stopped line", |cx| {
            has(cx, "Stopped before it finished")
        });
        assert_eq!(log.stops.load(Ordering::SeqCst), 1);
        assert!(has(&mut visual, "Scan again"));
        click(&mut visual, "duplicates-again");
        assert_eq!(
            role_of(&mut visual, "Comparing sizes, then contents…"),
            Some(Role::Status)
        );
        assert_eq!(log.scans.load(Ordering::SeqCst), 2);
    }

    #[gpui_kit::test]
    fn errors_and_empty_results_are_said_in_words(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Err("Folder does not exist".into()), false);
        open_dialog(&mut visual, "C:\\Gone", source, effects(&log));
        settle(&mut visual, "the error", |cx| has(cx, "Couldn't scan for duplicates"));
        assert_eq!(role_of(&mut visual, "Couldn't scan for duplicates"), Some(Role::Alert));
        assert_eq!(role_of(&mut visual, "Folder does not exist"), Some(Role::Status));

        let mut visual = window(cx);
        let none = report(Vec::new(), false);
        let (source, log) = fake(Ok(none), false);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        settle(&mut visual, "the empty line", |cx| has(cx, "No duplicate files found"));
        assert_eq!(
            role_of(
                &mut visual,
                "Checked 120 files in 42 ms. (2 already-hardlinked files were skipped.)"
            ),
            Some(Role::Status)
        );
    }

    #[gpui_kit::test]
    fn open_file_location_closes_the_dialog_then_shows_the_file(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (source, log) = fake(Ok(two_groups(false)), false);
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "Keep a.jpg"));
        click(&mut visual, ("duplicates-reveal", 2_usize));
        assert_eq!(
            *log.revealed.lock().unwrap(),
            vec!["C:\\Photos\\sub\\b.jpg".to_string()]
        );
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn escape_closes_it_and_closing_stops_a_scan_still_running(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let log = Arc::new(Log::default());
        let b = log.clone();
        let source = Source {
            scan: Arc::new(|_| Box::pin(std::future::pending())),
            stop: Arc::new(move || {
                b.stops.fetch_add(1, Ordering::SeqCst);
            }),
            link: fake(Ok(two_groups(false)), false).0.link,
        };
        open_dialog(&mut visual, "C:\\Photos", source, effects(&log));
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
        let arabic = report(
            vec![group(
                "ffeeddccbbaa9988",
                300,
                &["C:\\ملفات\\تقرير.pdf", "C:\\ملفات\\نسخة\\تقرير.pdf"],
            )],
            false,
        );
        let (source, log) = fake(Ok(arabic), false);
        open_dialog(&mut visual, "C:\\ملفات", source, effects(&log));
        settle(&mut visual, "the list", |cx| has(cx, "Keep تقرير.pdf"));
        // The name is exposed as the file has it, without the layout-only isolates.
        assert_eq!(role_of(&mut visual, "Keep نسخة\\تقرير.pdf"), Some(Role::RadioButton));
        assert_eq!(role_of(&mut visual, "Duplicate files in “ملفات”"), Some(Role::Group));
        testing::set_rtl(false);
    }
}
