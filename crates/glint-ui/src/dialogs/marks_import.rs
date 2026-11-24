//! Import marks… (docs/gpui/marks, F25, F27 to F29): the preview of what a tag.dat, an XYplorerTag.dat or a
//! Glint export would do, then how far it is, then what happened. Card F2a.
//!
//! **What this file is.** [ImportDialog] asks the [Backend] for a plan (core reads the file and finds the items,
//! never on the UI thread) and draws what the plan says: every number on screen comes from the plan core answered.
//! A choice that changes the plan (a label pointed at another, the extra columns, how Glint's own marks are met, XYplorer's
//! folder) asks again; the newest answer wins, and the Import button waits for it.
//!
//! **Applying.** Import closes the dialog; how far it is shows the way archive work does (notices.rs): a notice
//! "Importing marks… 40%" that is pushed again as the percent changes, and the taskbar button's bar. The run goes on
//! in a task of the window, not of the dialog. When it ends, in this order: the labels in the settings, the follow
//! switch, [tags::TagsChanged] for the paths, one Undo step ("import of 1,190 items"), and a notice with **Undo** and
//! **What happened**.
//!
//! **Undo takes the labels back too**, as one step with the marks ([UndoAction::Labels] in a `Sequence`), but only
//! while Settings' labels are still what the import made them: the person's own changes since stay (and Undo says
//! nothing more than its own words).

use super::tags;
use crate::explorer::{self, UndoAction};
use crate::kit::prelude::*;
use crate::kit::rtl_line;
use crate::platform::pickers::{self, Filter, Options};
use crate::platform::window_state::{self, ProgressState};
use crate::{core_bridge, i18n, state, t, theme};
use glint_core::host::ipc::Channel;
use glint_core::marks::apply::apply_labels;
use glint_core::marks::{At, ImportOptions, ImportPlan, ImportReport, LabelPlan, Progress};
use glint_model::format::{format_count, format_date, plural};
use glint_model::marks::{meet, tidy, Label, Meet, XyExtra, XyKind, MAX_LABELS};
use glint_model::paths::base_name;
use gpui_kit::UniformListScrollHandle;
use std::cell::Cell;
use std::collections::HashSet;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// The dialog's width.
const WIDTH: f32 = 640.;
/// Every row of What happened has this height, in rems, so the list can draw only what is on screen.
const ROW_HEIGHT: f32 = 1.75;
/// How long the notice that offers Undo and What happened stays (longer than a plain Undo offer: the person may
/// want to read the list).
const NOTICE_FOR: Duration = Duration::from_secs(20);

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// What the dialog asks of the PC. [CoreBackend] is the real one; tests give a fake.
pub trait Backend: Send + Sync {
    /// A file the person picks (Windows' open dialog); None when cancelled. `owner` is the window's handle.
    fn pick(&self, owner: isize) -> Fut<Option<String>>;
    /// XYplorer's folder, picked the same way.
    fn pick_folder(&self, owner: isize) -> Fut<Option<String>>;
    /// Reads `source` and plans the import (`import_plan`).
    fn plan(&self, source: String, options: ImportOptions) -> Fut<ImportPlan>;
    /// Applies the plan, saying how far it is on `progress` (`import_apply`).
    fn apply(&self, plan: ImportPlan, progress: Channel<Progress>) -> Fut<ImportReport>;
}

/// Waits for a core call and flattens the two layers of failure (the task panicking, the call failing).
fn flat<T: Send + 'static>(call: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(call);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

/// glint-core's import, and Windows' pickers.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn pick(&self, owner: isize) -> Fut<Option<String>> {
        pickers::open_file(
            Options::new()
                .title(t!("Import marks").to_string())
                .filter(Filter::new(t!("Marks files").to_string(), &["dat", "jsonl"]))
                .filter(Filter::new(t!("All files").to_string(), &["*"]))
                .owner(owner),
        )
    }

    fn pick_folder(&self, owner: isize) -> Fut<Option<String>> {
        pickers::open_folder(Options::new().title(t!("XYplorer's folder").to_string()).owner(owner))
    }

    fn plan(&self, source: String, options: ImportOptions) -> Fut<ImportPlan> {
        flat(glint_core::marks::import_plan(source, options))
    }

    fn apply(&self, plan: ImportPlan, progress: Channel<Progress>) -> Fut<ImportReport> {
        flat(glint_core::marks::import_apply(plan, progress))
    }
}

// ---- What a plan comes to ---------------------------------------------------------------------------------

/// The numbers the plan comes to, made once when it arrives (a frame only reads them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Digest {
    items: usize,
    /// Different tags (any case) over all the items.
    tags: usize,
    /// Items that get a comment.
    comments: usize,
    here: usize,
    moved: usize,
    away: usize,
    missing: usize,
    /// Items Glint has marks on already.
    marked: usize,
    /// The drives and servers the away items are on, sorted.
    drives: Vec<String>,
    /// Whether applying the plan would change anything.
    changes: bool,
}

impl Digest {
    fn of(plan: &ImportPlan, how: Meet) -> Self {
        let mut digest = Digest {
            items: plan.items.len(),
            ..Digest::default()
        };
        let mut tags = HashSet::new();
        let mut drives = HashSet::new();
        for item in &plan.items {
            tags.extend(item.gets.tags.iter().map(|tag| tag.to_lowercase()));
            digest.comments += usize::from(item.gets.comment.is_some());
            match &item.at {
                At::Here => digest.here += 1,
                At::Moved { .. } => digest.moved += 1,
                At::Away => {
                    digest.away += 1;
                    drives.insert(drive_of(&item.path));
                }
                At::Missing => digest.missing += 1,
            }
            digest.marked += usize::from(item.has.is_some());
            let empty = item.gets == glint_model::marks::Marks::default();
            digest.changes |= !empty
                && match (&item.at, &item.has) {
                    // Waiting items go into the ledger; items without marks of Glint's take what the source has.
                    (At::Away | At::Missing, _) | (_, None) => true,
                    (_, Some(has)) => meet(has, &item.gets, how).0 != tidy(has),
                };
        }
        digest.tags = tags.len();
        let mut drives: Vec<String> = drives.into_iter().collect();
        drives.sort();
        digest.drives = drives;
        digest
    }
}

/// The drive (`C:`) or server (`\\server`) a path is on.
fn drive_of(path: &str) -> String {
    match path.strip_prefix("\\\\") {
        Some(rest) => format!("\\\\{}", rest.split('\\').next().unwrap_or("")),
        None => path.chars().take(2).collect::<String>().to_uppercase(),
    }
}

/// Whether `source` is XYplorer's own `tag.dat` (the one Glint can keep following).
fn is_tag_dat(source: &str) -> bool {
    base_name(source).eq_ignore_ascii_case("tag.dat")
}

/// What a label of the source becomes, in words. `shown`: with the names isolated for drawing.
fn becomes_words(label: &LabelPlan, shown: bool) -> SharedString {
    let said = |name: &str| -> SharedString {
        if shown {
            isolate(name.to_string())
        } else {
            name.to_string().into()
        }
    };
    if label.to.is_empty() {
        t!("comes in as a tag")
    } else if label.new {
        t!("New label")
    } else if label.takes_name {
        t!(
            "is {label}, which Glint will then call {name}",
            label = said(&label.to),
            name = said(&label.name)
        )
    } else {
        t!("is {label}", label = said(&label.to))
    }
}

/// What an extra column becomes (7.4).
fn extra_words(extra: &XyExtra) -> SharedString {
    let caption = extra.caption.clone();
    match extra.kind {
        XyKind::Checkbox => t!("becomes the tag {caption}", caption = caption),
        XyKind::Rating => t!("becomes tags like {caption}: 4", caption = caption),
        XyKind::List => t!("becomes tags like {caption}: value", caption = caption),
        XyKind::Icon | XyKind::Other(_) => t!("left out"),
        XyKind::Text | XyKind::Number | XyKind::Date | XyKind::Location | XyKind::Image | XyKind::Script => {
            t!("becomes a line of the comment")
        }
    }
}

/// A count as the dialog shows it.
fn count(n: usize) -> String {
    format_count(n)
}

/// A count from core.
fn count_of(n: u64) -> String {
    format_count(usize::try_from(n).unwrap_or(usize::MAX))
}

/// What the notice says when an import ends: the parts that are not 0, or that nothing was left to bring in.
fn notice_words(report: &ImportReport) -> SharedString {
    let mut parts: Vec<SharedString> = Vec::new();
    if report.marked > 0 {
        parts.push(t!("Marked {marked}", marked = count_of(report.marked)));
    }
    if report.waiting > 0 {
        parts.push(t!("{waiting} waiting", waiting = count_of(report.waiting)));
    }
    if report.missing > 0 {
        parts.push(t!("{missing} not found", missing = count_of(report.missing)));
    }
    if parts.is_empty() {
        return t!("Nothing to bring in: everything is marked already");
    }
    parts.join(" · ").into()
}

/// Settings' labels become `before` when they are still `made` (what an import left them as): one the person
/// changed since is theirs, and stays. Whether they were put back. Undo of an import and Settings' take back both say
/// nothing more than their own words either way.
pub(crate) fn put_labels_back(cx: &mut App, before: &[Label], made: &[Label]) -> bool {
    let same = state::settings(cx).labels == made;
    if same {
        state::update(cx, |settings| settings.labels = before.to_vec());
    }
    same
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// Where the plan stands.
enum Phase {
    /// Asking core for the first plan.
    Reading,
    /// Core couldn't, with why.
    Failed(SharedString),
    /// The plan, as it is shown (and applied).
    Ready(Box<ImportPlan>),
}

/// The preview of an import.
pub struct ImportDialog {
    source: String,
    backend: Arc<dyn Backend>,
    /// What was chosen so far; every change asks core again.
    options: ImportOptions,
    phase: Phase,
    digest: Digest,
    /// The source's extra columns, kept from the first plan that had them: a plan made without extras may not list them,
    /// and the switch that brings them back must stay.
    extras: Vec<XyExtra>,
    /// The plan being asked for (the newest answer wins).
    seq: u64,
    /// A plan is on its way: Import waits.
    pending: bool,
    /// "Keep following this file" (only offered for a tag.dat).
    follow: bool,
    /// The label row whose choices are open.
    pointing: Option<usize>,
}

impl ImportDialog {
    /// A dialog for `source` that asks for its plan at once. `have` is Glint's labels as they are now.
    pub fn new(source: String, backend: Arc<dyn Backend>, have: Vec<Label>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            source,
            backend,
            options: ImportOptions {
                extras: true,
                have,
                ..ImportOptions::default()
            },
            phase: Phase::Reading,
            digest: Digest::default(),
            extras: Vec::new(),
            seq: 0,
            pending: false,
            follow: false,
            pointing: None,
        };
        this.plan_again(cx);
        this
    }

    /// The plan shown, once there is one.
    pub fn plan(&self) -> Option<&ImportPlan> {
        match &self.phase {
            Phase::Ready(plan) => Some(plan),
            _ => None,
        }
    }

    /// What was chosen so far.
    pub fn options(&self) -> &ImportOptions {
        &self.options
    }

    /// Whether a plan is being asked for.
    pub fn is_busy(&self) -> bool {
        self.pending || matches!(self.phase, Phase::Reading)
    }

    /// Asks core for the plan with the options as they are now, on core's runtime.
    fn plan_again(&mut self, cx: &mut Context<Self>) {
        self.seq += 1;
        self.pending = true;
        let seq = self.seq;
        let call = self.backend.plan(self.source.clone(), self.options.clone());
        cx.spawn(async move |this, cx| {
            let result = call.await;
            let _ = this.update(cx, |this, cx| {
                if this.seq != seq {
                    return;
                }
                this.pending = false;
                match result {
                    Ok(plan) => {
                        // A column with no caption is one XYplorer keeps unused (`|0|0|0|||||`): not listed.
                        let named: Vec<XyExtra> = plan.extras.iter().filter(|e| !e.caption.trim().is_empty()).cloned().collect();
                        if !named.is_empty() {
                            this.extras = named;
                        }
                        this.digest = Digest::of(&plan, this.options.meet);
                        this.phase = Phase::Ready(Box::new(plan));
                    }
                    Err(reason) => this.phase = Phase::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Points the source's label `name` at the Glint label `to` (empty: as a tag) and plans again.
    pub fn point(&mut self, name: String, to: String, cx: &mut Context<Self>) {
        self.options.labels.retain(|(from, _)| *from != name);
        self.options.labels.push((name, to));
        self.pointing = None;
        self.plan_again(cx);
    }

    /// Brings the extra columns in, or leaves them out, and plans again.
    pub fn set_extras(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.options.extras != on {
            self.options.extras = on;
            self.plan_again(cx);
        }
    }

    /// Chooses how Glint's own marks are met and plans again.
    pub fn set_meet(&mut self, how: Meet, cx: &mut Context<Self>) {
        if self.options.meet != how {
            self.options.meet = how;
            self.plan_again(cx);
        }
    }

    /// Asks for XYplorer's folder and plans again with it.
    fn choose_base(&mut self, window: &Window, cx: &mut Context<Self>) {
        let picking = self.backend.pick_folder(pickers::owner_of(window));
        cx.spawn(async move |this, cx| {
            if let Ok(Some(folder)) = picking.await {
                let _ = this.update(cx, |this, cx| {
                    this.options.base = Some(folder);
                    this.plan_again(cx);
                });
            }
        })
        .detach();
    }

    /// Whether Import can be pressed: a plan is shown, no newer one is on its way, and something would change.
    pub fn can_import(&self) -> bool {
        matches!(self.phase, Phase::Ready(_)) && !self.pending && self.digest.changes
    }

    /// Whether the follow switch is offered (an XYplorer file named tag.dat).
    pub fn offers_follow(&self) -> bool {
        is_tag_dat(&self.source)
    }

    /// Sets the follow switch (applied to the settings when the import is).
    pub fn set_follow(&mut self, on: bool, cx: &mut Context<Self>) {
        self.follow = on;
        cx.notify();
    }

    /// The source's line (F28): name, version, when it changed, and what is in it.
    pub fn source_line(&self) -> Option<SharedString> {
        let plan = self.plan()?;
        let mut head = vec![base_name(&plan.source).to_string()];
        if plan.version > 0 {
            head.push(t!("version {v}", v = plan.version).to_string());
        }
        let when = format_date(plan.changed, None, i18n::language());
        if !when.is_empty() {
            head.push(t!("changed {when}", when = when).to_string());
        }
        let mut parts = vec![
            head.join(", "),
            plural(self.digest.items, "item"),
            plural(plan.labels.len(), "label"),
            plural(self.digest.tags, "tag"),
            plural(self.digest.comments, "comment"),
        ];
        if !self.extras.is_empty() {
            parts.push(plural(self.extras.len(), "extra column"));
        }
        Some(parts.join(" · ").into())
    }

    /// Imports: closes the dialog and applies the plan shown, in the window.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Ready(plan) = &self.phase else {
            return;
        };
        if !self.can_import() {
            return;
        }
        let plan = (**plan).clone();
        let follow = self.follow && self.offers_follow();
        let backend = self.backend.clone();
        window.close_dialog(cx);
        run(window, cx, backend, plan, follow);
    }
}

// ---- Applying ---------------------------------------------------------------------------------------------

/// The notice that says how far the import is.
struct ImportToast;

/// The notice that says what the import did.
struct DoneToast;

/// The progress notice, with its percent when there is one.
fn working(percent: Option<u8>) -> Notification {
    let words = match percent {
        Some(percent) => t!("Importing marks… {percent}%", percent = percent),
        None => t!("Importing marks…"),
    };
    Notification::info(words).id1::<ImportToast>(0usize).autohide(false)
}

/// How far, in whole percent; None before the total is known.
fn percent_of(progress: Progress) -> Option<u8> {
    if progress.total == 0 {
        return None;
    }
    let percent = progress.done.saturating_mul(100) / progress.total;
    Some(u8::try_from(percent.min(100)).unwrap_or(100))
}

/// Runs the import on core's runtime and shows how far it is (progress arrives at most once a frame, and the
/// notice is pushed again only when the percent changes); `finish` says what happened.
fn run(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, plan: ImportPlan, follow: bool) {
    let (channel, receiver) = core_bridge::stream::<Progress>();
    let call = backend.apply(plan.clone(), channel);
    window.push_notification(working(None), cx);
    window_state::taskbar_progress(window, ProgressState::Indeterminate, None, None);
    let finished = Rc::new(Cell::new(false));
    let ended = finished.clone();
    window
        .spawn(cx, async move |cx| {
            let mut shown = None;
            while let Ok(mut progress) = receiver.recv().await {
                // Only the newest message each time the task wakes.
                while let Ok(newer) = receiver.try_recv() {
                    progress = newer;
                }
                if ended.get() {
                    break;
                }
                let percent = percent_of(progress);
                let push = shown != percent;
                shown = percent;
                let alive = cx.update(|window, cx| {
                    if progress.total > 0 {
                        window_state::taskbar_progress(
                            window,
                            ProgressState::Normal,
                            Some(progress.done),
                            Some(progress.total),
                        );
                    }
                    if push && !ended.get() {
                        window.push_notification(working(percent), cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    window
        .spawn(cx, async move |cx| {
            let result = call.await;
            let _ = cx.update(|window, cx| {
                finished.set(true);
                finish(window, cx, &plan, follow, result);
            });
        })
        .detach();
}

/// The import's one Undo step and its words: the marks as they were, and the labels as they were when the import
/// changed them (`Sequence([Labels, Marks])`, run marks first), else `Marks` alone; none when nothing changed.
fn undo_step(report: &ImportReport, before: Vec<Label>, made: Vec<Label>) -> Option<(String, UndoAction)> {
    let marks = (!report.befores.is_empty()).then(|| UndoAction::Marks(report.befores.clone()));
    let labels = (before != made).then_some(UndoAction::Labels { before, made });
    let action = match (labels, marks) {
        (Some(labels), Some(marks)) => UndoAction::Sequence(vec![labels, marks]),
        (Some(one), None) | (None, Some(one)) => one,
        (None, None) => return None,
    };
    let what = if report.befores.is_empty() {
        t!("import of labels")
    } else {
        t!("import of {what}", what = plural(report.befores.len(), "item"))
    };
    Some((what.to_string(), action))
}

/// What happens when the import ends (see the module's note for the order).
fn finish(window: &mut Window, cx: &mut App, plan: &ImportPlan, follow: bool, result: Result<ImportReport, String>) {
    window.remove_notification1::<ImportToast>(0usize, cx);
    window_state::taskbar_progress(window, ProgressState::None, None, None);
    let report = match result {
        Ok(report) => report,
        Err(reason) => {
            window.push_notification(Notification::error(reason).title(t!("Couldn't import the marks")), cx);
            return;
        }
    };
    let labels_before = state::settings(cx).labels.clone();
    state::update(cx, |settings| {
        apply_labels(&mut settings.labels, &plan.labels);
        if follow {
            settings.follow_xyplorer = true;
        }
    });
    let paths: Vec<String> = report.befores.iter().map(|(path, _)| path.clone()).collect();
    let hub = tags::hub(cx);
    hub.update(cx, |_, cx| cx.emit(tags::TagsChanged { paths }));
    let step = undo_step(&report, labels_before, state::settings(cx).labels.clone())
        .and_then(|(what, action)| {
            explorer::try_explorer(cx)
                .map(|explorer| explorer.update(cx, |explorer, cx| explorer.remember_undo(what, action, cx)))
        });
    let key = step.map_or(0, |step| step.0 as usize);
    let items = Arc::new(report.items.clone());
    let mut note = Notification::success(notice_words(&report))
        .id1::<DoneToast>(key)
        .autohide(false)
        .content(move |_, _, _| {
            let items = items.clone();
            div()
                .child(
                    Button::new("marks-import-what")
                        .ghost()
                        .small()
                        .label(t!("What happened"))
                        .on_click(move |_, window, cx| {
                            window.remove_notification1::<DoneToast>(key, cx);
                            open_what_happened(window, cx, items.clone());
                        }),
                )
                .into_any_element()
        });
    if let Some(step) = step {
        note = note.action(move |_, _, _| {
            Button::new("marks-import-undo")
                .label(t!("Undo"))
                .on_click(move |_, window, cx| {
                    window.remove_notification1::<DoneToast>(key, cx);
                    if let Some(explorer) = explorer::try_explorer(cx) {
                        explorer.update(cx, |explorer, cx| explorer.undo(Some(step), cx).detach());
                    }
                })
        });
    }
    window.push_notification(note, cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(NOTICE_FOR).await;
        let _ = handle.update(cx, |_, window, cx| window.remove_notification1::<DoneToast>(key, cx));
    })
    .detach();
}

// ---- Drawing ----------------------------------------------------------------------------------------------

/// A label's dot in its own color (never the accent: a label is not a state).
fn dot(color: u32) -> Div {
    div()
        .size(rems(0.625))
        .flex_none()
        .rounded_full()
        .bg(theme::label_tone(color))
}

/// The heading of a part of the preview.
fn heading(text: SharedString, t: &theme::Tokens) -> Div {
    div()
        .text_size(theme::size::XS)
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.muted_foreground)
        .child(text)
}

impl ImportDialog {
    /// The labels, each with what it becomes and a list of choices to point it elsewhere.
    fn label_rows(&self, plan: &ImportPlan, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = plan.labels.iter().enumerate().map(|(ix, label)| {
            let open = self.pointing == Some(ix);
            let words = becomes_words(label, false);
            let name = label.name.clone();
            let mut choices: Vec<Button> = Vec::new();
            if open {
                for (at, have) in self.options.have.iter().enumerate() {
                    let chosen = !label.new && label.to == have.name;
                    let (from, to) = (name.clone(), have.name.clone());
                    choices.push(
                        Button::new(("marks-import-choice", ix * (MAX_LABELS + 2) + at))
                            .ghost()
                            .small()
                            .role(Role::RadioButton)
                            .toggled(chosen)
                            .accessibility_label(have.name.clone())
                            .on_click(cx.listener(move |this, _, _, cx| this.point(from.clone(), to.clone(), cx)))
                            .child(
                                row_start()
                                    .items_center()
                                    .gap(space(1.5))
                                    .child(dot(have.color))
                                    .child(isolate(have.name.clone())),
                            ),
                    );
                }
                let (from, as_new) = (name.clone(), label.name.clone());
                choices.push(
                    Button::new(("marks-import-choice", ix * (MAX_LABELS + 2) + MAX_LABELS))
                        .ghost()
                        .small()
                        .role(Role::RadioButton)
                        .toggled(label.new && !label.to.is_empty())
                        .accessibility_label(t!("New label"))
                        .label(t!("New label"))
                        .on_click(cx.listener(move |this, _, _, cx| this.point(from.clone(), as_new.clone(), cx))),
                );
                let from = name.clone();
                choices.push(
                    Button::new(("marks-import-choice", ix * (MAX_LABELS + 2) + MAX_LABELS + 1))
                        .ghost()
                        .small()
                        .role(Role::RadioButton)
                        .toggled(label.to.is_empty())
                        .accessibility_label(t!("As a tag"))
                        .label(t!("As a tag"))
                        .on_click(cx.listener(move |this, _, _, cx| this.point(from.clone(), String::new(), cx))),
                );
            }
            col()
                .gap(space(1.))
                .child(
                    row_start()
                        .items_center()
                        .gap(space(2.))
                        .child(dot(label.color))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .child(isolate(label.name.clone())),
                        )
                        .child(
                            Button::new(("marks-import-label", ix))
                                .ghost()
                                .small()
                                .label(becomes_words(label, true))
                                .accessibility_label(format!("{}: {}", label.name, words))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pointing = if this.pointing == Some(ix) { None } else { Some(ix) };
                                    cx.notify();
                                })),
                        ),
                )
                .when(open, |this| {
                    this.child(row_start().flex_wrap().gap(space(1.)).ps(4.).children(choices))
                })
        });
        col()
            .id("marks-import-labels")
            .role(Role::List)
            .aria_label(t!("Labels"))
            .test_support()
            .gap(space(1.5))
            .child(heading(t!("Labels"), t))
            .children(rows)
    }

    /// The extra columns, each with what it becomes, and the switch that leaves them out.
    fn extras_rows(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let label = t!("Bring extra columns in");
        col()
            .id("marks-import-extras")
            .role(Role::List)
            .aria_label(t!("Extra columns"))
            .test_support()
            .gap(space(1.5))
            .child(heading(t!("Extra columns"), t))
            .children(self.extras.iter().enumerate().map(|(ix, extra)| {
                let words = extra_words(extra);
                row_start()
                    .id(("marks-import-extra", ix))
                    .role(Role::ListItem)
                    .aria_label(format!("{}: {}", extra.caption, words))
                    .test_support()
                    .gap(space(2.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .child(isolate(extra.caption.clone())),
                    )
                    .child(div().text_color(t.muted_foreground).child(words))
            }))
            .child(
                row_start()
                    .items_center()
                    .gap(space(2.))
                    .child(
                        Switch::new("marks-import-extras-on")
                            .checked(self.options.extras)
                            .accessibility_label(label.clone())
                            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_extras(*on, cx))),
                    )
                    .child(label),
            )
    }

    /// Where the items are, by outcome: only the outcomes that have items.
    fn where_rows(&self, plan: &ImportPlan, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let digest = &self.digest;
        let line = |id: &'static str, n: usize, words: SharedString, more: Option<String>| {
            row_start()
                .id(id)
                .role(Role::ListItem)
                .aria_label(format!("{} {}", count(n), words))
                .test_support()
                .gap(space(2.))
                .child(
                    div()
                        .w(rems(4.))
                        .flex_none()
                        .text_end()
                        .font_weight(FontWeight::MEDIUM)
                        .child(count(n)),
                )
                .child(div().flex_1().min_w_0().child(words))
                .when_some(more, |this, more| {
                    this.child(div().text_color(t.muted_foreground).child(isolate(more)))
                })
                .into_any_element()
        };
        let mut lines: Vec<AnyElement> = Vec::new();
        if digest.here > 0 {
            lines.push(line("marks-import-here", digest.here, t!("Here"), None));
        }
        if digest.moved > 0 {
            lines.push(line(
                "marks-import-moved",
                digest.moved,
                t!("Moved since: found where they are now"),
                None,
            ));
        }
        if digest.away > 0 {
            lines.push(line(
                "marks-import-away",
                digest.away,
                t!("On a drive that is not connected: they wait"),
                Some(digest.drives.join(", ")),
            ));
        }
        if digest.missing > 0 {
            lines.push(line(
                "marks-import-missing",
                digest.missing,
                t!("Not found: they wait"),
                None,
            ));
        }
        if digest.marked > 0 {
            lines.push(line(
                "marks-import-marked",
                digest.marked,
                t!("Already marked in Glint"),
                None,
            ));
        }
        let choices = (digest.marked > 0).then(|| {
            let radio = |id: &'static str, how: Meet, words: SharedString, cx: &mut Context<Self>| {
                Button::new(id)
                    .ghost()
                    .small()
                    .role(Role::RadioButton)
                    .toggled(self.options.meet == how)
                    .accessibility_label(words.clone())
                    .label(words)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_meet(how, cx)))
            };
            row_start()
                .id("marks-import-meet")
                .role(Role::RadioGroup)
                .aria_label(t!("Already marked in Glint"))
                .test_support()
                .flex_wrap()
                .gap(space(1.))
                .ps(4.)
                .child(radio("marks-import-fill", Meet::Fill, t!("Fill what is empty"), cx))
                .child(radio(
                    "marks-import-replace",
                    Meet::Replace,
                    t!("The file's marks replace Glint's"),
                    cx,
                ))
        });
        let needs_base = plan.needs_base.then(|| {
            row_start()
                .items_center()
                .gap(space(2.))
                .child(
                    div()
                        .id("marks-import-needs-base")
                        .role(Role::Status)
                        .aria_label(t!("Glint couldn't tell where XYplorer is"))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .child(t!("Glint couldn't tell where XYplorer is")),
                )
                .child(
                    Button::new("marks-import-base")
                        .ghost()
                        .small()
                        .label(t!("Choose its folder…"))
                        .on_click(cx.listener(|this, _, window, cx| this.choose_base(window, cx))),
                )
        });
        col()
            .id("marks-import-where")
            .role(Role::List)
            .aria_label(t!("Where the items are"))
            .test_support()
            .gap(space(1.5))
            .child(heading(t!("Where the items are"), t))
            .children(lines)
            .children(choices)
            .children(needs_base)
    }

    /// The preview: everything the plan says.
    fn preview(&self, plan: &ImportPlan, t: &theme::Tokens, cx: &mut Context<Self>) -> Div {
        let line = self.source_line().unwrap_or_default();
        let mut preview = col().gap(space(3.)).child(
            col()
                .gap(space(1.))
                .child(
                    div()
                        .id("marks-import-source")
                        .role(Role::Status)
                        .aria_label(line.clone())
                        .test_support()
                        .child(rtl_line(line)),
                )
                .when(plan.version > 5, |this| {
                    this.child(
                        div()
                            .id("marks-import-newer")
                            .role(Role::Status)
                            .aria_label(t!("A newer format than Glint knows: read by the same rules"))
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!("A newer format than Glint knows: read by the same rules")),
                    )
                }),
        );
        if !plan.labels.is_empty() {
            preview = preview.child(self.label_rows(plan, t, cx));
        }
        if !self.extras.is_empty() {
            preview = preview.child(self.extras_rows(t, cx));
        }
        preview = preview.child(self.where_rows(plan, t, cx));
        if self.offers_follow() {
            let label = t!("Keep following this file");
            preview = preview.child(
                row_start()
                    .items_center()
                    .gap(space(2.))
                    .child(
                        Switch::new("marks-import-follow")
                            .checked(self.follow)
                            .accessibility_label(label.clone())
                            .on_click(cx.listener(|this, on: &bool, _, cx| this.set_follow(*on, cx))),
                    )
                    .child(label),
            );
        }
        preview
    }
}

impl Render for ImportDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let root = col()
            .id("marks-import")
            .role(Role::Group)
            .aria_label(t!("Import marks"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.));
        match &self.phase {
            Phase::Reading => {
                super::no_main(cx);
                root.child(
                    div()
                        .id("marks-import-reading")
                        .role(Role::Status)
                        .aria_label(t!("Reading…"))
                        .test_support()
                        .text_color(t.muted_foreground)
                        .child(t!("Reading…")),
                )
            }
            Phase::Failed(reason) => {
                super::main_action(cx, true, |_, window, cx| window.close_dialog(cx));
                root.child(
                    div()
                        .id("marks-import-error")
                        .role(Role::Alert)
                        .aria_label(reason.clone())
                        .test_support()
                        .text_color(t.destructive)
                        .child(reason.clone()),
                )
                .child(
                    row_start().justify_end().child(
                        Button::new("marks-import-close")
                            .primary()
                            .label(t!("Close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
            }
            Phase::Ready(plan) => {
                let plan = plan.clone();
                let can = self.can_import();
                super::main_action(cx, can, |this, window, cx| this.apply(window, cx));
                root.child(self.preview(&plan, &t, cx)).child(
                    row_start()
                        .justify_end()
                        .gap(space(2.))
                        .child(
                            Button::new("marks-import-cancel")
                                .ghost()
                                .label(t!("Cancel"))
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("marks-import-apply")
                                .primary()
                                .label(t!("Import"))
                                .disabled(!can)
                                .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                        ),
                )
            }
        }
    }
}

// ---- What happened ----------------------------------------------------------------------------------------

/// Every item of an import with what happened to it, in a list that draws only the rows on screen.
pub struct WhatHappened {
    items: Arc<Vec<(String, String)>>,
    scroll: UniformListScrollHandle,
}

impl WhatHappened {
    /// The list for `items` (path, outcome).
    pub fn new(items: Arc<Vec<(String, String)>>) -> Self {
        Self {
            items,
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// The lines Copy puts on the clipboard: path, a tab, the outcome.
    pub fn lines(&self) -> String {
        self.items
            .iter()
            .map(|(path, outcome)| format!("{path}\t{outcome}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Render for WhatHappened {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        super::main_action(cx, true, |_, window, cx| window.close_dialog(cx));
        let items = self.items.clone();
        let total = items.len();
        let colors = t.clone();
        let list = uniform_list(
            "marks-happened-rows",
            total,
            cx.processor(move |_, range: Range<usize>, _, _| {
                range
                    .filter_map(|ix| {
                        let (path, outcome) = items.get(ix)?;
                        Some(
                            row_start()
                                .id(("marks-happened-row", ix))
                                .role(Role::ListItem)
                                .aria_label(format!("{path}: {outcome}"))
                                .test_support()
                                .h(rems(ROW_HEIGHT))
                                .items_center()
                                .gap(space(2.))
                                .px(space(2.))
                                .text_size(theme::size::XS)
                                .child(div().flex_1().min_w_0().truncate().child(isolate(path.clone())))
                                .child(
                                    div()
                                        .flex_none()
                                        .max_w(rems(20.))
                                        .truncate()
                                        .text_color(colors.muted_foreground)
                                        .child(isolate(outcome.clone())),
                                )
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .size_full();
        col()
            .id("marks-happened")
            .role(Role::Group)
            .aria_label(t!("What happened"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(
                div().relative().h(rems(ROW_HEIGHT * total.clamp(3, 12) as f32)).child(
                    div()
                        .id("marks-happened-list")
                        .role(Role::List)
                        .aria_label(t!("Items"))
                        .test_support()
                        .size_full()
                        .child(list),
                ),
            )
            .child(
                row_start()
                    .justify_end()
                    .gap(space(2.))
                    .child(
                        Button::new("marks-happened-copy")
                            .ghost()
                            .label(t!("Copy"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(this.lines()));
                            })),
                    )
                    .child(
                        Button::new("marks-happened-close")
                            .primary()
                            .label(t!("Close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}

/// Opens What happened for `items`.
pub fn open_what_happened(window: &mut Window, cx: &mut App, items: Arc<Vec<(String, String)>>) {
    let view = cx.new(|_| WhatHappened::new(items));
    super::show(window, cx, t!("What happened"), px(WIDTH), view);
}

// ---- Opening ----------------------------------------------------------------------------------------------

/// Opens the dialog on `source` and gives its view.
fn show_dialog(window: &mut Window, cx: &mut App, source: String, backend: Arc<dyn Backend>) -> Entity<ImportDialog> {
    let have = state::try_settings(cx)
        .map(|settings| settings.labels.clone())
        .unwrap_or_default();
    let view = cx.new(|cx| ImportDialog::new(source, backend, have, cx));
    super::show(window, cx, t!("Import marks"), px(WIDTH), view.clone());
    view
}

/// Opens the import for `source`, or asks for a file first when there is none (nothing opens when none is chosen).
pub fn open(window: &mut Window, cx: &mut App, source: Option<String>) {
    open_with(window, cx, source, Arc::new(CoreBackend));
}

/// [open] on `backend`, for tests.
pub fn open_with(window: &mut Window, cx: &mut App, source: Option<String>, backend: Arc<dyn Backend>) {
    match source.filter(|source| !source.trim().is_empty()) {
        Some(source) => {
            show_dialog(window, cx, source, backend);
        }
        None => {
            let picking = backend.pick(pickers::owner_of(window));
            window
                .spawn(cx, async move |cx| {
                    let Ok(Some(source)) = picking.await else {
                        return;
                    };
                    if source.trim().is_empty() {
                        return;
                    }
                    let _ = cx.update(|window, cx| {
                        show_dialog(window, cx, source, backend);
                    });
                })
                .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::explorer::fake::{folder, FakeDisk};
    use crate::explorer::{Explorer, FakeUndoRunner, NavOptions};
    use crate::testing;
    use glint_core::marks::Planned;
    use glint_model::marks::Marks;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::Mutex;

    type Answer = Box<dyn Fn(&ImportOptions) -> Result<ImportPlan, String> + Send + Sync>;

    /// A backend that answers plans made by hand, writes down what it was asked, and can fail.
    struct Fake {
        answer: Answer,
        /// A plan that never arrives (the dialog stays on Reading).
        never: bool,
        file: Option<String>,
        folder: Option<String>,
        report: Mutex<Result<ImportReport, String>>,
        asked: Mutex<Vec<(String, ImportOptions)>>,
        applied: Mutex<Vec<ImportPlan>>,
        picked: Mutex<Vec<&'static str>>,
    }

    impl Fake {
        fn new(answer: impl Fn(&ImportOptions) -> Result<ImportPlan, String> + Send + Sync + 'static) -> Self {
            Fake {
                answer: Box::new(answer),
                never: false,
                file: None,
                folder: None,
                report: Mutex::new(Ok(ImportReport::default())),
                asked: Mutex::new(Vec::new()),
                applied: Mutex::new(Vec::new()),
                picked: Mutex::new(Vec::new()),
            }
        }

        fn of(plan: ImportPlan) -> Arc<Self> {
            Arc::new(Fake::new(move |_| Ok(plan.clone())))
        }

        fn asked(&self) -> Vec<(String, ImportOptions)> {
            self.asked.lock().map(|a| a.clone()).unwrap_or_default()
        }

        fn applied(&self) -> Vec<ImportPlan> {
            self.applied.lock().map(|a| a.clone()).unwrap_or_default()
        }

        fn picked(&self) -> Vec<&'static str> {
            self.picked.lock().map(|p| p.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn pick(&self, _: isize) -> Fut<Option<String>> {
            if let Ok(mut picked) = self.picked.lock() {
                picked.push("file");
            }
            let file = self.file.clone();
            Box::pin(async move { Ok(file) })
        }

        fn pick_folder(&self, _: isize) -> Fut<Option<String>> {
            if let Ok(mut picked) = self.picked.lock() {
                picked.push("folder");
            }
            let folder = self.folder.clone();
            Box::pin(async move { Ok(folder) })
        }

        fn plan(&self, source: String, options: ImportOptions) -> Fut<ImportPlan> {
            let answer = (self.answer)(&options);
            if let Ok(mut asked) = self.asked.lock() {
                asked.push((source, options));
            }
            if self.never {
                return Box::pin(std::future::pending());
            }
            Box::pin(async move { answer })
        }

        fn apply(&self, plan: ImportPlan, progress: Channel<Progress>) -> Fut<ImportReport> {
            if let Ok(mut applied) = self.applied.lock() {
                applied.push(plan);
            }
            let _ = progress.send(Progress { done: 1, total: 2 });
            let report = self
                .report
                .lock()
                .map(|r| r.clone())
                .unwrap_or_else(|_| Err("poisoned".into()));
            Box::pin(async move { report })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    const A: &str = "C:\\a\\a.txt";
    const B: &str = "C:\\a\\b.txt";
    const WHEN: i64 = 1_700_000_000_000;

    fn marked(label: Option<&str>, tags: &[&str], comment: Option<&str>) -> Marks {
        Marks {
            label: label.map(String::from),
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            comment: comment.map(String::from),
        }
    }

    fn item(path: &str, at: At, gets: Marks, has: Option<Marks>) -> Planned {
        Planned {
            path: path.to_string(),
            at,
            gets,
            has,
        }
    }

    fn label(name: &str, to: &str) -> LabelPlan {
        LabelPlan {
            name: name.to_string(),
            color: 0xFC7268,
            to: to.to_string(),
            new: false,
            takes_name: false,
        }
    }

    fn plan(source: &str, version: u32, items: Vec<Planned>) -> ImportPlan {
        ImportPlan {
            source: source.to_string(),
            version,
            changed: WHEN,
            items,
            ..ImportPlan::default()
        }
    }

    fn two_items() -> Vec<Planned> {
        vec![
            item(A, At::Here, marked(Some("Red"), &["x"], None), None),
            item(B, At::Here, marked(None, &["y"], Some("note")), None),
        ]
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the import dialog never reached the state the test waits for");
    }

    struct Rig {
        visual: VisualTestContext,
        view: Entity<ImportDialog>,
        explorer: Option<Entity<Explorer>>,
        runner: Option<Arc<FakeUndoRunner>>,
        changed: Rc<RefCell<Vec<tags::TagsChanged>>>,
        _heard: Subscription,
        _disk: Option<FakeDisk>,
    }

    /// A window with the dialog layer, the dialog open on `source` (and read), the hub's events heard and, when asked
    /// for, an explorer on a fake disk with a fake undo runner.
    fn rig(cx: &mut TestAppContext, fake: &Arc<Fake>, source: &str, with_explorer: bool) -> Rig {
        cx.update(testing::init);
        let disk = with_explorer.then(FakeDisk::new);
        let (explorer, runner) = match &disk {
            Some(disk) => {
                disk.folder("C:\\", vec![folder("C:\\", "a")]);
                disk.folder("C:\\a", vec![]);
                let runner = cx.update(FakeUndoRunner::install);
                cx.update(|cx| explorer::init_with(cx, disk.source()));
                let explorer = cx.update(|cx| explorer::explorer(cx));
                let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\a", NavOptions::default(), cx));
                cx.foreground_executor().block_test(task);
                cx.run_until_parked();
                (Some(explorer), Some(runner))
            }
            None => (None, None),
        };
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let backend: Arc<dyn Backend> = fake.clone();
        let source = source.to_string();
        let view = visual.update(|window, cx| show_dialog(window, cx, source, backend));
        let changed: Rc<RefCell<Vec<tags::TagsChanged>>> = Default::default();
        let heard = changed.clone();
        let subscription = visual.update(|_, cx| {
            let hub = tags::hub(cx);
            cx.subscribe(&hub, move |_, event: &tags::TagsChanged, _| {
                heard.borrow_mut().push(event.clone())
            })
        });
        if !fake.never {
            settle(&mut visual, |cx| view.read_with(cx, |view, _| !view.is_busy()));
        }
        Rig {
            visual,
            view,
            explorer,
            runner,
            changed,
            _heard: subscription,
            _disk: disk,
        }
    }

    impl Rig {
        fn has(&mut self, label: &str) -> bool {
            self.visual.update(|window, cx| {
                window.render_frame(cx);
                window.try_find_by_label(label).is_some()
            })
        }

        fn click(&mut self, id: impl Into<ElementId>) {
            let id = id.into();
            self.visual.update(|window, cx| {
                window.render_frame(cx);
                window.click(id, cx);
            });
            self.visual.run_until_parked();
        }

        fn press(&mut self, key: &str) {
            self.visual.update(|window, cx| {
                window.render_frame(cx);
                window.press(key, cx);
            });
            self.visual.run_until_parked();
        }

        /// Waits until no plan is on its way.
        fn read(&mut self) {
            let view = self.view.clone();
            settle(&mut self.visual, |cx| view.read_with(cx, |view, _| !view.is_busy()));
        }

        fn is_open(&mut self) -> bool {
            self.visual.update(|window, cx| window.has_active_dialog(cx))
        }

        fn line(&mut self) -> Option<String> {
            self.view
                .read_with(&self.visual, |view, _| view.source_line().map(|l| l.to_string()))
        }

        fn next_undo(&mut self) -> Option<String> {
            let explorer = self.explorer.clone()?;
            explorer.read_with(&self.visual, |e, _| e.next_undo().map(|(_, label)| label.to_string()))
        }
    }

    fn when() -> String {
        format_date(WHEN, None, i18n::language())
    }

    #[gpui_kit::test]
    fn while_the_plan_is_read_the_dialog_says_reading_and_nothing_else(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut fake = Fake::new(|_| Ok(ImportPlan::default()));
        fake.never = true;
        let fake = Arc::new(fake);
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", false);
        assert!(rig.has("Reading…"));
        assert!(!rig.has("Import"), "no buttons, no numbers yet");
        assert!(rig.line().is_none());
        assert_eq!(fake.asked().len(), 1);
    }

    #[gpui_kit::test]
    fn the_sources_line_has_the_numbers_of_the_plan_and_a_glint_export_has_no_version(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let items: Vec<Planned> = (0..1284)
            .map(|i| {
                let comment = (i < 412).then(|| "note".to_string());
                let gets = Marks {
                    label: None,
                    tags: vec![format!("t{}", i % 93)],
                    comment,
                };
                item(&format!("C:\\a\\{i}.txt"), At::Here, gets, None)
            })
            .collect();
        let mut xy = plan("C:\\x\\tag.dat", 5, items.clone());
        xy.labels = (0..7).map(|i| label(&format!("L{i}"), "Red")).collect();
        xy.extras = ["a", "b", "c"]
            .iter()
            .map(|c| XyExtra {
                caption: (*c).to_string(),
                kind: XyKind::Text,
            })
            .collect();
        let mut rig1 = rig(cx, &Fake::of(xy), "C:\\x\\tag.dat", false);
        let expected = format!(
            "tag.dat, version 5, changed {} · 1,284 items · 7 labels · 93 tags · 412 comments · 3 extra columns",
            when()
        );
        assert_eq!(rig1.line().as_deref(), Some(expected.as_str()));
        assert!(rig1.has(&expected));
        assert!(!rig1.has("A newer format than Glint knows: read by the same rules"));
        drop(rig1);

        let mut export = plan("C:\\x\\e.jsonl", 0, items.clone());
        export.labels = vec![label("Red", "Red")];
        let mut rig2 = rig(cx, &Fake::of(export), "C:\\x\\e.jsonl", false);
        let expected = format!(
            "e.jsonl, changed {} · 1,284 items · 1 label · 93 tags · 412 comments",
            when()
        );
        assert_eq!(rig2.line().as_deref(), Some(expected.as_str()));
        drop(rig2);

        let newer = plan("C:\\x\\tag.dat", 6, items);
        let mut rig3 = rig(cx, &Fake::of(newer), "C:\\x\\tag.dat", false);
        assert!(rig3.has("A newer format than Glint knows: read by the same rules"));
        assert!(rig3.line().is_some_and(|l| l.contains("version 6")));
    }

    #[gpui_kit::test]
    fn each_label_says_what_it_becomes_and_pointing_one_at_another_plans_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut p = plan("C:\\x\\tag.dat", 5, two_items());
        let mut chinese = label("红色", "Red");
        chinese.takes_name = true;
        let mut teal = label("Teal", "Teal");
        teal.new = true;
        p.labels = vec![label("Grey", "Gray"), chinese, teal, label("Odd", "")];
        let fake = Fake::of(p);
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", false);
        assert!(rig.has("Grey: is Gray"));
        assert!(rig.has("红色: is Red, which Glint will then call 红色"));
        assert!(rig.has("Teal: New label"));
        assert!(rig.has("Odd: comes in as a tag"));
        assert!(!rig.has("Blue"), "the choices are closed until a row is pressed");
        rig.click(("marks-import-label", 1usize));
        assert!(rig.has("Blue") && rig.has("New label") && rig.has("As a tag"));
        // Blue is the fifth of Glint's seven labels.
        rig.click(("marks-import-choice", MAX_LABELS + 2 + 4));
        rig.read();
        let asked = fake.asked();
        assert_eq!(asked.len(), 2);
        assert_eq!(asked[1].1.labels, [("红色".to_string(), "Blue".to_string())]);
        assert!(!rig.has("Blue"), "choosing closes the choices");
        // As a tag points it at none.
        rig.click(("marks-import-label", 3usize));
        rig.click(("marks-import-choice", 3 * (MAX_LABELS + 2) + MAX_LABELS + 1));
        rig.read();
        let last = fake.asked().pop().expect("asked again").1;
        assert_eq!(
            last.labels,
            [
                ("红色".to_string(), "Blue".to_string()),
                ("Odd".to_string(), String::new())
            ]
        );
    }

    #[gpui_kit::test]
    fn the_extra_columns_say_what_they_become_and_their_switch_plans_again_without_them(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let extra = |caption: &str, kind| XyExtra {
            caption: caption.to_string(),
            kind,
        };
        let extras = vec![
            extra("Reviewed", XyKind::Checkbox),
            extra("Stars", XyKind::Rating),
            extra("Notes", XyKind::Text),
            extra("Pic", XyKind::Icon),
        ];
        let fake = Arc::new(Fake::new(move |options| {
            let mut p = plan("C:\\x\\tag.dat", 5, two_items());
            // A plan made without extras may not list them.
            if options.extras {
                p.extras = extras.clone();
            }
            Ok(p)
        }));
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", false);
        assert!(rig.has("Reviewed: becomes the tag Reviewed"));
        assert!(rig.has("Stars: becomes tags like Stars: 4"));
        assert!(rig.has("Notes: becomes a line of the comment"));
        assert!(rig.has("Pic: left out"));
        assert!(rig.has("Bring extra columns in"));
        assert!(fake.asked()[0].1.extras, "on to begin with");
        rig.click("marks-import-extras-on");
        rig.read();
        assert!(!fake.asked()[1].1.extras);
        assert!(rig.has("Bring extra columns in"), "the switch stays to bring them back");

        let none = Fake::of(plan("C:\\x\\tag.dat", 5, two_items()));
        let mut bare = rig_again(cx, &none);
        assert!(!bare.has("Extra columns") && !bare.has("Bring extra columns in"));
    }

    /// Another dialog in a new window (the app's globals are set up already).
    fn rig_again(cx: &mut TestAppContext, fake: &Arc<Fake>) -> Rig {
        rig(cx, fake, "C:\\x\\tag.dat", false)
    }

    #[gpui_kit::test]
    fn only_the_outcomes_that_have_items_are_drawn_and_replace_plans_again_with_meet_replace(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let items = vec![
            item(A, At::Here, marked(Some("Red"), &[], None), None),
            item(
                B,
                At::Here,
                marked(Some("Blue"), &[], None),
                Some(marked(Some("Green"), &[], None)),
            ),
            item("D:\\x\\c.txt", At::Away, marked(Some("Red"), &[], None), None),
            item("E:\\x\\d.txt", At::Away, marked(Some("Red"), &[], None), None),
        ];
        let fake = Fake::of(plan("C:\\x\\tag.dat", 5, items));
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", false);
        assert!(rig.has("2 Here"));
        assert!(rig.has("2 On a drive that is not connected: they wait"));
        assert!(rig.has("1 Already marked in Glint"));
        assert!(!rig.has("1 Moved since: found where they are now"));
        assert!(!rig.has("1 Not found: they wait"));
        rig.visual.update(|window, cx| {
            window.render_frame(cx);
            let away = window.find("marks-import-away");
            assert!(away.label().is_some());
        });
        assert!(rig.has("Fill what is empty") && rig.has("The file's marks replace Glint's"));
        assert_eq!(fake.asked()[0].1.meet, Meet::Fill);
        rig.click("marks-import-replace");
        rig.read();
        assert_eq!(fake.asked()[1].1.meet, Meet::Replace);
    }

    #[test]
    fn import_is_off_while_nothing_would_change() {
        let has = marked(Some("Green"), &["x"], Some("mine"));
        let gets = marked(Some("Blue"), &["x"], Some("theirs"));
        let same = plan(
            "C:\\x\\tag.dat",
            5,
            vec![item(
                A,
                At::Here,
                gets.clone(),
                Some(marked(Some("Blue"), &["x"], Some("theirs"))),
            )],
        );
        assert!(!Digest::of(&same, Meet::Fill).changes);
        let differs = plan("C:\\x\\tag.dat", 5, vec![item(A, At::Here, gets, Some(has))]);
        // Fill keeps the label and the comment of Glint's and adds no new tag: nothing to do. Replace changes both.
        assert!(!Digest::of(&differs, Meet::Fill).changes);
        assert!(Digest::of(&differs, Meet::Replace).changes);
        let waiting = plan(
            "C:\\x\\tag.dat",
            5,
            vec![item(A, At::Missing, marked(Some("Red"), &[], None), None)],
        );
        assert!(
            Digest::of(&waiting, Meet::Fill).changes,
            "items that wait are written to the ledger"
        );
        let nothing = plan("C:\\x\\tag.dat", 5, vec![item(A, At::Here, Marks::default(), None)]);
        assert!(!Digest::of(&nothing, Meet::Fill).changes);
    }

    #[gpui_kit::test]
    fn when_xyplorer_cannot_be_told_the_dialog_asks_for_its_folder_and_plans_again_with_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut fake = Fake::new(|options| {
            let mut p = plan("C:\\x\\tag.dat", 5, two_items());
            p.needs_base = options.base.is_none();
            Ok(p)
        });
        fake.folder = Some("C:\\Apps\\XYplorer".to_string());
        let fake = Arc::new(fake);
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", false);
        assert!(rig.has("Glint couldn't tell where XYplorer is"));
        rig.click("marks-import-base");
        rig.read();
        assert_eq!(fake.picked(), ["folder"]);
        assert_eq!(fake.asked()[1].1.base.as_deref(), Some("C:\\Apps\\XYplorer"));
        assert!(
            !rig.has("Glint couldn't tell where XYplorer is"),
            "answered, so not asked again"
        );
    }

    #[gpui_kit::test]
    fn import_asks_apply_once_with_the_plan_shown_then_says_tags_changed_remembers_one_undo_step_and_offers_both(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let p = plan("C:\\x\\tag.dat", 5, two_items());
        let fake = Fake::of(p.clone());
        let report = ImportReport {
            marked: 2,
            waiting: 61,
            missing: 33,
            befores: vec![
                (A.to_string(), Marks::default()),
                (B.to_string(), marked(Some("Red"), &[], None)),
            ],
            items: vec![
                (A.to_string(), "Marked".to_string()),
                (B.to_string(), "Marked".to_string()),
            ],
            ..ImportReport::default()
        };
        if let Ok(mut r) = fake.report.lock() {
            *r = Ok(report.clone());
        }
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", true);
        rig.click("marks-import-apply");
        let heard = rig.changed.clone();
        let applied = fake.clone();
        settle(&mut rig.visual, |_| {
            !heard.borrow().is_empty() && !applied.applied().is_empty()
        });
        assert_eq!(fake.applied(), [p]);
        assert_eq!(
            rig.changed.borrow().as_slice(),
            [tags::TagsChanged {
                paths: vec![A.to_string(), B.to_string()]
            }]
        );
        assert!(!rig.is_open(), "Import closes the dialog");
        assert_eq!(rig.next_undo().as_deref(), Some("import of 2 items"));
        assert_eq!(notice_words(&report), "Marked 2 · 61 waiting · 33 not found");
        assert!(rig.has("Undo") && rig.has("What happened"), "the notice offers both");
        // The step, taken back, puts the befores.
        let explorer = rig.explorer.clone().expect("an explorer");
        explorer.update(&mut rig.visual, |e, cx| e.undo(None, cx).detach());
        let runner = rig.runner.clone().expect("a runner");
        settle(&mut rig.visual, |_| !runner.marks_put().is_empty());
        assert_eq!(runner.marks_put(), [report.befores]);
    }

    #[test]
    fn the_notice_leaves_out_what_is_nothing_and_says_when_all_of_it_is() {
        let only = |marked, waiting, missing| ImportReport {
            marked,
            waiting,
            missing,
            ..ImportReport::default()
        };
        assert_eq!(notice_words(&only(1190, 0, 33)), "Marked 1,190 · 33 not found");
        assert_eq!(notice_words(&only(0, 5, 0)), "5 waiting");
        assert_eq!(
            notice_words(&only(0, 0, 0)),
            "Nothing to bring in: everything is marked already"
        );
    }

    #[test]
    fn labels_come_in_new_are_renamed_by_xyplorers_name_and_never_past_thirty_two() {
        let mut new = label("天蓝色", "天蓝色");
        new.new = true;
        new.color = 0x0969DA;
        let mut takes = label("红色", "Red");
        takes.takes_name = true;
        let mut labels = glint_model::marks::default_labels();
        apply_labels(&mut labels, &[new.clone(), takes.clone(), label("Odd", "")]);
        assert_eq!(labels.len(), 8);
        assert_eq!(labels[7].name, "天蓝色");
        assert_eq!(labels[7].color, 0x0969DA);
        assert_eq!(labels[0].name, "红色");
        assert_eq!(labels[0].was, ["Red"]);
        // Again: nothing is added twice, nothing is renamed twice.
        apply_labels(&mut labels, &[new.clone(), takes]);
        assert_eq!(labels.len(), 8);
        assert_eq!(labels[0].was, ["Red"]);
        // 32 labels: none is added.
        let mut full: Vec<Label> = (0..MAX_LABELS)
            .map(|i| Label {
                name: format!("L{i}"),
                color: 0,
                was: Vec::new(),
            })
            .collect();
        apply_labels(&mut full, &[new]);
        assert_eq!(full.len(), MAX_LABELS);
    }

    #[gpui_kit::test]
    fn the_labels_of_the_plan_are_in_the_settings_when_the_import_ends(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut p = plan("C:\\x\\tag.dat", 5, two_items());
        let mut chinese = label("红色", "Red");
        chinese.takes_name = true;
        let mut teal = label("天蓝色", "天蓝色");
        teal.new = true;
        p.labels = vec![chinese, teal];
        let fake = Fake::of(p);
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", false);
        rig.click("marks-import-apply");
        let done = fake.clone();
        settle(&mut rig.visual, |cx| {
            !done.applied().is_empty() && cx.update(|_, cx| state::settings(cx).labels.len() == 8)
        });
        let labels = rig.visual.update(|_, cx| state::settings(cx).labels.clone());
        let red = labels
            .iter()
            .find(|l| l.name == "红色")
            .expect("Red is called 红色 now");
        assert_eq!(red.was, ["Red"]);
        assert!(labels.iter().any(|l| l.name == "天蓝色"));
    }

    #[gpui_kit::test]
    fn the_follow_switch_is_for_a_tag_dat_only_and_sets_the_setting_when_the_import_is_applied(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let fake = Fake::of(plan("C:\\x\\export.jsonl", 0, two_items()));
        let mut other = rig(cx, &fake, "C:\\x\\export.jsonl", false);
        assert!(!other.has("Keep following this file"));
        drop(other);

        let fake = Fake::of(plan("C:\\XY\\tag.dat", 5, two_items()));
        let mut rig = rig(cx, &fake, "C:\\XY\\tag.dat", false);
        assert!(rig.has("Keep following this file"));
        let follows = |rig: &mut Rig| rig.visual.update(|_, cx| state::settings(cx).follow_xyplorer);
        assert!(!follows(&mut rig), "off until chosen");
        rig.click("marks-import-follow");
        assert!(!follows(&mut rig), "choosing it does nothing yet");
        rig.click("marks-import-apply");
        let done = fake.clone();
        settle(&mut rig.visual, |cx| {
            !done.applied().is_empty() && cx.update(|_, cx| state::settings(cx).follow_xyplorer)
        });
    }

    #[gpui_kit::test]
    fn a_plan_that_fails_shows_its_words_and_an_apply_that_fails_says_so_and_remembers_no_step(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let failing = Arc::new(Fake::new(|_| Err("That file is not a tag database".to_string())));
        let mut rig1 = rig(cx, &failing, "C:\\x\\tag.dat", false);
        assert!(rig1.has("That file is not a tag database"));
        assert!(rig1.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find("marks-import-close").is_some()
        }));
        assert!(!rig1.has("Import"));
        rig1.click("marks-import-close");
        assert!(!rig1.is_open());
        drop(rig1);

        let fake = Fake::of(plan("C:\\x\\tag.dat", 5, two_items()));
        if let Ok(mut r) = fake.report.lock() {
            *r = Err("The disk is full".to_string());
        }
        let mut rig2 = rig(cx, &fake, "C:\\x\\tag.dat", true);
        rig2.click("marks-import-apply");
        let done = fake.clone();
        settle(&mut rig2.visual, |_| !done.applied().is_empty());
        rig2.visual.run_until_parked();
        assert_eq!(rig2.next_undo(), None);
        assert!(rig2.changed.borrow().is_empty());
        // The progress notice leaves with a short fade (the test clock has to run for it); what stays is the
        // failure, with its reason.
        cx.executor().advance_clock(Duration::from_secs(2));
        settle(&mut rig2.visual, |cx| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window.notifications(cx).len()
            }) == 1
        });
    }

    #[gpui_kit::test]
    fn what_happened_lists_every_item_draws_only_what_is_on_screen_and_copy_puts_the_lines_on_the_clipboard(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let items: Vec<(String, String)> = (0..2000)
            .map(|i| {
                (
                    format!("C:\\a\\{i}.txt"),
                    if i % 2 == 0 { "Marked" } else { "Waiting" }.to_string(),
                )
            })
            .collect();
        let shown = Arc::new(items.clone());
        visual.update(|window, cx| open_what_happened(window, cx, shown));
        let (first, last) = ("C:\\a\\0.txt: Marked", "C:\\a\\1999.txt: Waiting");
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find_by_label(first).is_some());
            assert!(
                window.try_find_by_label(last).is_none(),
                "rows out of sight are not drawn"
            );
            window.click("marks-happened-copy", cx);
        });
        visual.run_until_parked();
        let copied = visual.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
        let expected: Vec<String> = items.iter().map(|(p, o)| format!("{p}\t{o}")).collect();
        assert_eq!(copied, Some(expected.join("\n")));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("marks-happened-close", cx);
        });
        visual.run_until_parked();
        assert!(!visual.update(|window, cx| window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn everything_pressable_has_a_name_escape_closes_and_arabic_names_mirror_without_overlap(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let path = "C:\\ملفات\\تقرير.txt";
        let mut p = plan(
            "C:\\ملفات\\tag.dat",
            5,
            vec![item(
                path,
                At::Here,
                marked(Some("حرجة"), &["وسم"], Some("تعليق")),
                None,
            )],
        );
        let mut critical = label("حرجة", "");
        critical.new = false;
        p.labels = vec![critical];
        p.extras = vec![XyExtra {
            caption: "مراجعة".to_string(),
            kind: XyKind::Checkbox,
        }];
        let fake = Fake::of(p);
        testing::set_rtl(true);
        let mut rig = rig(cx, &fake, "C:\\ملفات\\tag.dat", false);
        rig.visual.update(|window, cx| {
            window.render_frame(cx);
            for role in [Role::Button, Role::Switch, Role::RadioButton] {
                for element in window.find_all_by_role(role) {
                    assert!(
                        element.label().is_some_and(|l| !l.is_empty()),
                        "{role:?} without a name"
                    );
                }
            }
            let (apply, cancel) = (
                window.find("marks-import-apply").bounds(),
                window.find("marks-import-cancel").bounds(),
            );
            assert!(!apply.intersects(&cancel), "the buttons do not overlap");
            assert!(window.try_find_by_label("حرجة: comes in as a tag").is_some());
            assert!(window.try_find_by_label("مراجعة: becomes the tag مراجعة").is_some());
        });
        rig.press("escape");
        assert!(!rig.is_open(), "Escape closes the dialog");
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn open_without_a_source_asks_for_a_file_first_and_does_nothing_when_none_is_chosen(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;

        let mut none = Fake::new(|_| Ok(plan("C:\\x\\tag.dat", 5, two_items())));
        none.file = None;
        let none = Arc::new(none);
        let backend: Arc<dyn Backend> = none.clone();
        visual.update(|window, cx| open_with(window, cx, None, backend));
        visual.run_until_parked();
        assert_eq!(none.picked(), ["file"]);
        assert!(none.asked().is_empty());
        assert!(!visual.update(|window, cx| window.has_active_dialog(cx)));

        let mut some = Fake::new(|_| Ok(plan("C:\\x\\tag.dat", 5, two_items())));
        some.file = Some("C:\\x\\tag.dat".to_string());
        let some = Arc::new(some);
        let backend: Arc<dyn Backend> = some.clone();
        visual.update(|window, cx| open_with(window, cx, None, backend));
        let asked = some.clone();
        settle(&mut visual, |_| !asked.asked().is_empty());
        assert_eq!(some.picked(), ["file"]);
        assert_eq!(some.asked()[0].0, "C:\\x\\tag.dat");
        assert!(visual.update(|window, cx| window.has_active_dialog(cx)));
    }

    /// An import that renamed Red to 红色 and added 天蓝色, ended with an explorer to hold its step: the labels
    /// as they were before, and the report's befores.
    fn labelled_import(cx: &mut TestAppContext) -> (Rig, Vec<Label>, Vec<(String, Marks)>) {
        let mut p = plan("C:\\x\\tag.dat", 5, two_items());
        let mut chinese = label("红色", "Red");
        chinese.takes_name = true;
        let mut teal = label("天蓝色", "天蓝色");
        teal.new = true;
        p.labels = vec![chinese, teal];
        let befores = vec![(A.to_string(), Marks::default()), (B.to_string(), marked(Some("Red"), &[], None))];
        let fake = Fake::of(p);
        if let Ok(mut r) = fake.report.lock() {
            *r = Ok(ImportReport { marked: 2, befores: befores.clone(), ..ImportReport::default() });
        }
        let mut rig = rig(cx, &fake, "C:\\x\\tag.dat", true);
        let before = rig.visual.update(|_, cx| state::settings(cx).labels.clone());
        rig.click("marks-import-apply");
        settle(&mut rig.visual, |cx| cx.update(|_, cx| state::settings(cx).labels.len() == before.len() + 1));
        rig.visual.run_until_parked();
        (rig, before, befores)
    }

    fn undo_it(rig: &mut Rig) {
        let explorer = rig.explorer.clone().expect("an explorer");
        explorer.update(&mut rig.visual, |e, cx| e.undo(None, cx).detach());
        let runner = rig.runner.clone().expect("a runner");
        settle(&mut rig.visual, |_| !runner.marks_put().is_empty());
    }

    #[gpui_kit::test]
    fn undo_of_an_import_that_renamed_and_added_labels_puts_marks_and_labels_back_in_one_step(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let (mut rig, before, befores) = labelled_import(cx);
        assert_eq!(rig.next_undo().as_deref(), Some("import of 2 items"), "one step, one label");
        let made = rig.visual.update(|_, cx| state::settings(cx).labels.clone());
        assert!(made.iter().any(|l| l.name == "红色" && l.was == ["Red"]) && made.iter().any(|l| l.name == "天蓝色"));
        undo_it(&mut rig);
        let runner = rig.runner.clone().expect("a runner");
        assert_eq!(runner.marks_put(), [befores]);
        assert_eq!(rig.visual.update(|_, cx| state::settings(cx).labels.clone()), before);
        assert_eq!(rig.next_undo(), None, "the one step is used up");
    }

    #[gpui_kit::test]
    fn undo_leaves_labels_the_person_changed_since_the_import_and_still_puts_the_marks_back(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let (mut rig, _, befores) = labelled_import(cx);
        rig.visual.update(|_, cx| state::update(cx, |s| s.labels[1].name = "مهم".into()));
        let theirs = rig.visual.update(|_, cx| state::settings(cx).labels.clone());
        undo_it(&mut rig);
        let runner = rig.runner.clone().expect("a runner");
        assert_eq!(runner.marks_put(), [befores], "the marks come back");
        assert_eq!(rig.visual.update(|_, cx| state::settings(cx).labels.clone()), theirs, "their labels stay");
        assert!(theirs.iter().any(|l| l.name == "مهم"), "an Arabic name goes through untouched");
    }

    #[test]
    fn an_arabic_label_the_import_made_is_put_back_by_undo_as_it_was() {
        let mut before = glint_model::marks::default_labels();
        before[0].name = "مهم".into();
        let mut made = before.clone();
        let mut p = plan("C:\\x\\tag.dat", 5, vec![]).labels;
        let mut sky = label("زرقاء", "زرقاء");
        sky.new = true;
        p.push(sky);
        apply_labels(&mut made, &p);
        assert_eq!(made.len(), before.len() + 1);
        let report = ImportReport::default();
        let (what, action) = undo_step(&report, before.clone(), made.clone()).expect("a step for labels alone");
        assert_eq!(what, "import of labels");
        assert_eq!(action, UndoAction::Labels { before, made });
    }

    #[test]
    fn an_import_that_changed_no_label_makes_a_marks_step_as_before_and_one_that_changed_nothing_makes_none() {
        let labels = glint_model::marks::default_labels();
        let report = ImportReport { befores: vec![(A.to_string(), Marks::default())], ..ImportReport::default() };
        let step = undo_step(&report, labels.clone(), labels.clone());
        assert_eq!(step, Some(("import of 1 item".to_string(), UndoAction::Marks(report.befores.clone()))));
        assert_eq!(undo_step(&ImportReport::default(), labels.clone(), labels), None);
    }
}
