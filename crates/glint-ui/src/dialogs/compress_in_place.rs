//! Compressing in place with Windows' WOF: the files stay where they are, stored compressed and opened as
//! before. Ported from src/components/explorer/CompressInPlaceDialog.tsx, with glint-core's `wof`.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                               | Here                                                         |
//! |------------------------------------------|--------------------------------------------------------------|
//! | `canCompressInPlace(entry)`              | [`can_compress_in_place`] (a path: on this PC, not a share)  |
//! | `openCompressInPlace(entries)`           | [`open`] (paths; says so when none can be compressed)        |
//! | `invoke("wof_*")` and its `Channel`      | [`Backend`] (a fake in the tests), [`CoreBackend`]           |
//! | `CompressInPlaceModal`                   | [`CompressInPlaceDialog`]                                    |
//! | `WofResult`                              | the Done view of the dialog                                  |
//! | `HINTS`, `saving`, the choices           | [`hint`], [`CompressInPlaceDialog::saving`], the four radios |
//!
//! **Closing the dialog stops the work** (the TypeScript's cleanup called `wof_cancel`): the view's `Drop` does it.
//!
//! **Undo.** A finished compression that core can take back (`Report::undo`) is kept as the explorer's step
//! "compressing 12 files in place" (`UndoAction::Wof(id)`, which runs `glint_core::wof::wof_undo`), so Ctrl+Z
//! decompresses the files as the TypeScript's `rememberUndo` did, and a notification offers Undo for 8 s
//! (`offerUndo`). The result's own Undo button runs the same take-back with this dialog's progress, so it drops
//! the explorer's step first (core undoes a run once).
//!
//! Enter does not start the compression (the TypeScript's `onSubmit` did): the dialog layer keeps Enter for the
//! dialog's own buttons, and this dialog has no box to type in. Tab to Compress and press Enter or Space.

use crate::explorer::{self, UndoAction, UndoId};
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::host::ipc::Channel;
use glint_core::wof::{Algorithm, Measure, Progress, Report};
use glint_model::format::{format_count, format_duration, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The calls into glint-core's WOF. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// What the paths hold and what each algorithm would leave of them (`wof_measure`).
    fn measure(&self, paths: Vec<String>) -> Fut<Measure>;
    /// Compresses what the paths hold (`wof_compress`), reporting progress on `progress`.
    fn compress(&self, paths: Vec<String>, algorithm: Algorithm, progress: Channel<Progress>) -> Fut<Report>;
    /// Takes WOF's compression off what the paths hold (`wof_decompress`).
    fn decompress(&self, paths: Vec<String>, progress: Channel<Progress>) -> Fut<Report>;
    /// Takes back a compression run by its `Report::undo` id (`wof_undo`).
    fn undo(&self, id: u64, progress: Channel<Progress>) -> Fut<Report>;
    /// Stops the work under way (`wof_cancel`).
    fn cancel(&self);
}

/// glint-core's WOF.
pub struct CoreBackend;

/// Waits for a core call and flattens the two layers of failure (the task panicking, the call failing).
fn flat<T: Send + 'static>(call: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(call);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

impl Backend for CoreBackend {
    fn measure(&self, paths: Vec<String>) -> Fut<Measure> {
        flat(glint_core::wof::wof_measure(paths))
    }

    fn compress(&self, paths: Vec<String>, algorithm: Algorithm, progress: Channel<Progress>) -> Fut<Report> {
        flat(glint_core::wof::wof_compress(paths, algorithm, progress))
    }

    fn decompress(&self, paths: Vec<String>, progress: Channel<Progress>) -> Fut<Report> {
        flat(glint_core::wof::wof_decompress(paths, progress))
    }

    fn undo(&self, id: u64, progress: Channel<Progress>) -> Fut<Report> {
        flat(glint_core::wof::wof_undo(id, progress))
    }

    fn cancel(&self) {
        glint_core::wof::wof_cancel();
    }
}

struct SharedBackend(Arc<dyn Backend>);

impl Global for SharedBackend {}

/// Sets the backend the dialog uses (tests pass a fake). [`backend`] gives core's if this was never called.
pub fn init_with(cx: &mut App, backend: Arc<dyn Backend>) {
    cx.set_global(SharedBackend(backend));
}

/// The backend in use.
pub fn backend(cx: &App) -> Arc<dyn Backend> {
    match cx.try_global::<SharedBackend>() {
        Some(shared) => shared.0.clone(),
        None => Arc::new(CoreBackend),
    }
}

// ---- What can be compressed -----------------------------------------------------------------------------

/// Files and folders Compress in place takes (`canCompressInPlace`): on the PC itself, not on a share. NTFS is
/// checked when measuring. The menu decides the rest (not archived, closed, recycled, online-only or remote:
/// those are entries' states, and the menu passes only paths of plain items).
pub fn can_compress_in_place(path: &str) -> bool {
    !path.trim().is_empty() && !path.starts_with("\\\\")
}

/// `paths` that Compress in place takes, in order, each once.
pub fn usable_paths(paths: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    paths
        .iter()
        .filter(|p| can_compress_in_place(p) && seen.insert(p.to_lowercase()))
        .cloned()
        .collect()
}

/// WOF's algorithms, lightest first.
pub const ALGORITHMS: [Algorithm; 4] = [
    Algorithm::Xpress4k,
    Algorithm::Xpress8k,
    Algorithm::Xpress16k,
    Algorithm::Lzx,
];

/// The name glint-core's estimates call an algorithm by.
pub fn algorithm_name(algorithm: Algorithm) -> &'static str {
    match algorithm {
        Algorithm::Xpress4k => "xpress4k",
        Algorithm::Xpress8k => "xpress8k",
        Algorithm::Xpress16k => "xpress16k",
        Algorithm::Lzx => "lzx",
    }
}

/// An algorithm's name for the person and what it is good for (`HINTS`).
pub fn hint(algorithm: Algorithm) -> (SharedString, SharedString) {
    match algorithm {
        Algorithm::Xpress4k => (t!("Light"), t!("Fastest to open")),
        Algorithm::Xpress8k => (t!("Balanced"), t!("Windows' own default")),
        Algorithm::Xpress16k => (t!("Strong"), t!("A little more saved")),
        Algorithm::Lzx => (t!("Strongest"), t!("Most saved; for things used rarely")),
    }
}

// ---- The dialog ---------------------------------------------------------------------------------------------

/// Where the work stands.
enum Phase {
    /// Choosing.
    Setup,
    /// Compressing or decompressing, with the newest progress.
    Running { progress: Progress, decompress: bool },
    /// Finished (or stopped).
    Done { report: Report, decompress: bool },
    /// Windows or the drive refused.
    Failed { reason: SharedString, decompress: bool },
}

/// Compress in place for some files and folders: measures them, offers the four algorithms with what each
/// would save, runs one with progress and Stop, and tells what happened.
pub struct CompressInPlaceDialog {
    paths: Vec<String>,
    /// The name for the title, and the folder(s) under it.
    heading: SharedString,
    place: SharedString,
    backend: Arc<dyn Backend>,
    measure: Option<Measure>,
    measure_error: Option<SharedString>,
    algorithm: Algorithm,
    phase: Phase,
    /// The explorer's Undo step for the compression that just finished, while it is still offered.
    step: Option<UndoId>,
}

/// What a run does to the files.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Compress,
    Decompress,
    /// The result's Undo: decompresses what the compression compressed.
    TakeBack,
}

impl RunKind {
    /// Whether the run shows as a decompression (the Done view says so).
    fn decompresses(self) -> bool {
        self != RunKind::Compress
    }
}

/// The notification that offers Undo for a compression, keyed by its step.
struct CompressedToast;

/// How long the Undo offer stays, as the web app's 8 s toast.
const UNDO_FOR: std::time::Duration = std::time::Duration::from_secs(8);

/// Keeps how to take the compression back, for Ctrl+Z (`rememberUndo(what, ...)` in `offerWofUndo`): the files
/// that run compressed are decompressed. `None` when there is no explorer to keep it in.
fn remember_undo(id: u64, what: &str, cx: &mut App) -> Option<UndoId> {
    let explorer = explorer::try_explorer(cx)?;
    let what = what.to_string();
    Some(explorer.update(cx, |explorer, cx| explorer.remember_undo(what, UndoAction::Wof(id), cx)))
}

/// Says what was compressed with an Undo button (`offerUndo`): it takes the step back, as Ctrl+Z would.
fn offer_undo(said: String, step: UndoId, window: &mut Window, cx: &mut App) {
    let key = step.0 as usize;
    let note = Notification::success(SharedString::from(said))
        .id1::<CompressedToast>(key)
        .action(move |_, _, _| {
            Button::new("compress-undo-toast")
                .label(t!("Undo"))
                .on_click(move |_, window, cx| {
                    window.remove_notification1::<CompressedToast>(key, cx);
                    if let Some(explorer) = explorer::try_explorer(cx) {
                        explorer.update(cx, |explorer, cx| explorer.undo(Some(step), cx).detach());
                    }
                })
        });
    window.push_notification(note, cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(UNDO_FOR).await;
        let _ = handle.update(cx, |_, window, cx| {
            window.remove_notification1::<CompressedToast>(key, cx)
        });
    })
    .detach();
}

/// Keeps a finished compression for Ctrl+Z as "compressing 12 files in place" and offers Undo with a
/// notification (`offerWofUndo`): the explorer's step.
fn offer_run(id: u64, done: usize, window: &mut Window, cx: &mut App) -> Option<UndoId> {
    let what = t!("compressing {what} in place", what = plural(done, "file")).to_string();
    let step = remember_undo(id, &what, cx)?;
    offer_undo(capitalized(&what), step, window, cx);
    Some(step)
}

/// `text` with its first letter in capitals (`what.charAt(0).toUpperCase() + what.slice(1)`).
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

impl Drop for CompressInPlaceDialog {
    fn drop(&mut self) {
        // The TypeScript's cleanup: closing the dialog stops what it started (and a measuring under way).
        self.backend.cancel();
    }
}

impl CompressInPlaceDialog {
    /// A dialog for `paths` (already filtered), measuring them at once on core's runtime.
    pub fn new(paths: Vec<String>, backend: Arc<dyn Backend>, cx: &mut Context<Self>) -> Self {
        let heading = match paths.as_slice() {
            [one] => base_name(one).to_string(),
            _ => t!("{n} items", n = format_count(paths.len())).to_string(),
        };
        let dirs: HashSet<String> = paths.iter().map(|p| parent_path(p).to_lowercase()).collect();
        let place = match (dirs.len(), paths.first()) {
            (1, Some(first)) => parent_path(first),
            _ => t!("In {n} folders", n = format_count(dirs.len())).to_string(),
        };
        let measuring = backend.measure(paths.clone());
        cx.spawn(async move |this, cx| {
            let result = measuring.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(measure) => this.measure = Some(measure),
                    Err(reason) => this.measure_error = Some(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            paths,
            heading: heading.into(),
            place: place.into(),
            backend,
            measure: None,
            measure_error: None,
            algorithm: Algorithm::Xpress8k,
            phase: Phase::Setup,
            step: None,
        }
    }

    /// The dialog's title: `Compress in place: name`.
    pub fn title(&self) -> SharedString {
        t!("Compress in place: {name}", name = isolate(self.heading.clone()))
    }

    /// The chosen algorithm.
    pub fn algorithm(&self) -> Algorithm {
        self.algorithm
    }

    /// Whether a compression or decompression is under way.
    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    /// The finished run's report, if there is one.
    pub fn report(&self) -> Option<&Report> {
        match &self.phase {
            Phase::Done { report, .. } => Some(report),
            _ => None,
        }
    }

    /// Bytes `algorithm` would save, from the sample (`saving`); `None` before measuring.
    pub fn saving(&self, algorithm: Algorithm) -> Option<u64> {
        let measure = self.measure.as_ref()?;
        let name = algorithm_name(algorithm);
        let estimate = measure.estimates.iter().find(|e| e.algorithm == name)?;
        Some(measure.bytes.saturating_sub(estimate.after))
    }

    /// Chooses how hard to compress.
    pub fn choose(&mut self, algorithm: Algorithm, cx: &mut Context<Self>) {
        if matches!(self.phase, Phase::Setup) {
            self.algorithm = algorithm;
            cx.notify();
        }
    }

    /// Whether Compress can start: measured, NTFS, something to compress, and not already running.
    pub fn can_compress(&self) -> bool {
        matches!(self.phase, Phase::Setup) && self.measure.as_ref().is_some_and(|m| m.ntfs && m.files > 0)
    }

    /// Compresses (`run(false)`) or decompresses (`run(true)`) what was chosen.
    pub fn run(&mut self, decompress: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.phase, Phase::Setup) {
            return;
        }
        let Some(measure) = &self.measure else {
            return;
        };
        let total = if decompress { measure.wof } else { measure.files };
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = if decompress {
            self.backend.decompress(self.paths.clone(), channel)
        } else {
            self.backend.compress(self.paths.clone(), self.algorithm, channel)
        };
        let kind = if decompress {
            RunKind::Decompress
        } else {
            RunKind::Compress
        };
        self.watch(total, kind, receiver, call, window, cx);
    }

    /// Takes back the compression that just finished (the result's Undo).
    pub fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Phase::Done {
            report,
            decompress: false,
        } = &self.phase
        else {
            return;
        };
        let Some(id) = report.undo else {
            return;
        };
        let total = report.done;
        // This takes it back with progress; the explorer's step (and its notification) must not run it again.
        if let Some(step) = self.step.take() {
            if let Some(explorer) = explorer::try_explorer(cx) {
                explorer.update(cx, |explorer, cx| explorer.forget_undo(step, cx));
            }
            window.remove_notification1::<CompressedToast>(step.0 as usize, cx);
        }
        let (channel, receiver) = core_bridge::stream::<Progress>();
        let call = self.backend.undo(id, channel);
        self.watch(total, RunKind::TakeBack, receiver, call, window, cx);
    }

    /// Shows the run's progress as it comes (at most once a frame) and its result when it ends. A finished
    /// compression is kept for Ctrl+Z and offered with Undo.
    fn watch(
        &mut self,
        total: usize,
        kind: RunKind,
        receiver: async_channel::Receiver<Progress>,
        call: Fut<Report>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let decompress = kind.decompresses();
        self.phase = Phase::Running {
            progress: Progress {
                done: 0,
                total,
                ..Default::default()
            },
            decompress,
        };
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(mut progress) = receiver.recv().await {
                // Keep only the newest message each time the task wakes, so a chatty run redraws once.
                while let Ok(newer) = receiver.try_recv() {
                    progress = newer;
                }
                let alive = this.update(cx, |this, cx| {
                    if let Phase::Running { progress: shown, .. } = &mut this.phase {
                        *shown = progress;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn_in(window, async move |this, cx| {
            // `offerWofUndo`: core gives an id only when it compressed anything. A run stopped by closing the
            // dialog still has files compressed, so the Undo is kept even when the dialog is gone.
            let result = call.await;
            let undo_of = match &result {
                Ok(report) if kind == RunKind::Compress => report.undo.map(|id| (id, report.done)),
                _ => None,
            };
            let alive = this.update_in(cx, |this, window, cx| {
                this.step = undo_of.and_then(|(id, done)| offer_run(id, done, window, cx));
                this.phase = match result {
                    Ok(report) => Phase::Done { report, decompress },
                    Err(reason) => Phase::Failed {
                        reason: reason.into(),
                        decompress,
                    },
                };
                cx.notify();
            });
            if let (Err(_), Some((id, done))) = (alive, undo_of) {
                let _ = cx.update(|window, cx| offer_run(id, done, window, cx));
            }
        })
        .detach();
    }

    fn stop(&mut self) {
        self.backend.cancel();
    }

    fn close(window: &mut Window, cx: &mut App) {
        window.close_dialog(cx);
    }
}

// ---- Drawing ------------------------------------------------------------------------------------------------

/// A line of small muted text.
fn note(text: SharedString, t: &theme::Tokens) -> Div {
    div()
        .text_size(theme::size::XS)
        .text_color(t.muted_foreground)
        .child(text)
}

impl CompressInPlaceDialog {
    /// The four "How hard" radios, each with what it would save.
    fn algorithm_choices(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        row_start()
            .id("compress-algorithms")
            .role(Role::RadioGroup)
            .aria_label(t!("How hard"))
            .test_support()
            .gap(space(2.))
            .children(ALGORITHMS.iter().enumerate().map(|(ix, &algorithm)| {
                let chosen = algorithm == self.algorithm;
                let (label, plain_hint) = hint(algorithm);
                let line = match self.saving(algorithm) {
                    Some(saved) => t!("About {size} saved", size = format_size(saved)),
                    None => plain_hint,
                };
                Button::new(("compress-algorithm", ix))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(if chosen {
                                t.primary.opacity(0.1)
                            } else {
                                t.background.opacity(0.)
                            })
                            .foreground(t.foreground)
                            .hover(if chosen { t.primary.opacity(0.15) } else { t.accent })
                            .active(t.accent),
                    )
                    .flex_1()
                    .min_w_0()
                    .h_auto()
                    .border_1()
                    .border_color(if chosen { t.primary } else { t.border })
                    .px(space(2.))
                    .py(space(1.5))
                    .role(Role::RadioButton)
                    .toggled(chosen)
                    .accessibility_label(format!("{label}. {line}"))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(algorithm, cx)))
                    .child(
                        col()
                            .w_full()
                            .align_start()
                            .text_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(label))
                            .child(
                                div()
                                    .text_size(theme::size::XXS)
                                    .text_color(t.muted_foreground)
                                    .child(line),
                            ),
                    )
            }))
    }

    /// The facts about what was measured, the choices, and Decompress for what is compressed already.
    fn setup_view(&self, measure: &Measure, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let mut facts: Vec<SharedString> = vec![t!(
            "{files} to compress, {size} in all",
            files = plural(measure.files, "file"),
            size = format_size(measure.bytes)
        )];
        if measure.already > 0 {
            facts.push(t!(
                "{files} compressed already: {size} taking {on_disk}",
                files = plural(measure.already, "file"),
                size = format_size(measure.already_bytes),
                on_disk = format_size(measure.already_on_disk)
            ));
        }
        if measure.skipped > 0 {
            facts.push(t!(
                "{files} left alone: photos, videos, archives and other kinds that are compressed inside, tiny files, links and online-only files",
                files = plural(measure.skipped, "file")
            ));
        }
        let chosen = self.saving(self.algorithm);
        let (_, chosen_hint) = hint(self.algorithm);
        let saved_line = match chosen {
            Some(saved) => t!(
                "About {size} saved ({percent}%), measured on a sample. {hint}. Ctrl+Z decompresses them.",
                size = format_size(saved),
                percent = (100.0 * saved as f64 / measure.bytes.max(1) as f64).round(),
                hint = chosen_hint
            ),
            None => t!("{hint}. Ctrl+Z decompresses them.", hint = chosen_hint),
        };
        col()
            .gap(space(4.))
            .child(
                col()
                    .id("compress-facts")
                    .role(Role::List)
                    .aria_label(t!("What is there"))
                    .test_support()
                    .gap(space(1.))
                    .children(facts.into_iter().enumerate().map(|(ix, fact)| {
                        div()
                            .id(("compress-fact", ix))
                            .role(Role::ListItem)
                            .aria_label(fact.clone())
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(fact)
                    })),
            )
            .when(measure.files > 0, |this| {
                this.child(
                    col()
                        .gap(space(1.5))
                        .child(
                            div()
                                .text_size(theme::size::XS)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.muted_foreground)
                                .child(t!("How hard")),
                        )
                        .child(self.algorithm_choices(t, cx))
                        .child(
                            note(saved_line.clone(), t)
                                .id("compress-saving")
                                .role(Role::Status)
                                .aria_label(saved_line)
                                .test_support(),
                        ),
                )
            })
            .when(measure.wof > 0, |this| {
                let line = t!(
                    "{files} here are compressed this way. Decompressing needs as much free space as they'll take.",
                    files = plural(measure.wof, "file")
                );
                this.child(
                    align_row(3.)
                        .rounded(theme::size::RADIUS)
                        .bg(t.muted.opacity(0.4))
                        .px(space(3.))
                        .py(space(2.))
                        .child(
                            note(line.clone(), t)
                                .id("compress-already")
                                .role(Role::Group)
                                .aria_label(line)
                                .test_support()
                                .flex_1()
                                .min_w_0(),
                        )
                        .child(
                            Button::new("compress-decompress")
                                .outline()
                                .small()
                                .icon(IconName::Expand)
                                .label(t!("Decompress"))
                                .on_click(cx.listener(|this, _, window, cx| this.run(true, window, cx))),
                        ),
                )
            })
    }

    /// Which of how many, the file being done, what has been saved, and a bar.
    fn progress_view(progress: &Progress, decompress: bool, t: &theme::Tokens) -> impl IntoElement {
        let mut text = if decompress {
            t!("Decompressing")
        } else {
            t!("Compressing")
        }
        .to_string();
        if progress.total > 0 {
            text.push_str(&format!(
                " {}",
                t!(
                    "{done} of {total}",
                    done = format_count(progress.done),
                    total = format_count(progress.total)
                )
            ));
        }
        if !progress.current.is_empty() {
            text.push_str(&format!(" · {}", progress.current));
        }
        if progress.saved > 0 {
            let how = if decompress { t!("back") } else { t!("saved") };
            text.push_str(&format!(" · {} {how}", format_size(progress.saved)));
        }
        let whole = if progress.total == 0 {
            0.
        } else {
            progress.done as f32 / progress.total as f32
        };
        let text: SharedString = text.into();
        col()
            .id("compress-progress")
            .role(Role::ProgressIndicator)
            .aria_label(text.clone())
            .test_support()
            .gap(space(2.))
            .py(space(6.))
            .child(
                align_row(2.)
                    .text_color(t.muted_foreground)
                    .child(Spinner::new())
                    .child(div().flex_1().min_w_0().truncate().child(isolate(text))),
            )
            .child(
                row_start()
                    .h(space(1.5))
                    .w_full()
                    .overflow_hidden()
                    .rounded_full()
                    .bg(t.muted)
                    .child(
                        div()
                            .h_full()
                            .w(gpui_kit::relative(whole.clamp(0., 1.)))
                            .rounded_full()
                            .bg(t.primary),
                    ),
            )
    }

    /// What a finished run did (`WofResult`): how many, how much changed, how long, and what failed.
    fn result_view(report: &Report, decompress: bool, t: &theme::Tokens) -> impl IntoElement {
        let mut headline = if report.done > 0 {
            let files = plural(report.done, "file");
            if decompress {
                t!("Decompressed {files}", files = files)
            } else {
                t!("Compressed {files}", files = files)
            }
        } else if decompress {
            t!("Nothing was decompressed")
        } else {
            t!("Nothing was compressed")
        }
        .to_string();
        if report.stopped {
            headline.push_str(&t!(" before it was stopped"));
        }
        let headline: SharedString = headline.into();
        let changed = report.before.abs_diff(report.after);
        let sign = if decompress { "+" } else { "−" };
        let sizes: Option<SharedString> = (report.done > 0).then(|| {
            t!(
                "{before} → {after} on the drive ({sign}{changed}) · {time}",
                before = format_size(report.before),
                after = format_size(report.after),
                sign = sign,
                changed = format_size(changed),
                time = format_duration(report.elapsed_ms)
            )
        });
        let unchanged: Option<SharedString> = (report.unchanged > 0).then(|| {
            t!(
                "{files} wouldn't get smaller, so Windows left them as they were.",
                files = plural(report.unchanged, "file")
            )
        });
        col()
            .gap(space(3.))
            .child(
                align_row(2.)
                    .id("compress-result")
                    .role(Role::Status)
                    .aria_label(headline.clone())
                    .test_support()
                    .items_start()
                    .child(
                        icon(IconName::CircleCheck)
                            .size_5()
                            .flex_none()
                            .text_color(if report.done > 0 { t.success } else { t.muted_foreground }),
                    )
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .align_start()
                            .child(div().font_weight(FontWeight::MEDIUM).child(headline))
                            .children(sizes.map(|s| {
                                note(s.clone(), t)
                                    .id("compress-sizes")
                                    .role(Role::Group)
                                    .aria_label(s)
                                    .test_support()
                            }))
                            .children(unchanged.map(|s| {
                                note(s.clone(), t)
                                    .id("compress-unchanged")
                                    .role(Role::Group)
                                    .aria_label(s)
                                    .test_support()
                            })),
                    ),
            )
            .when(report.failed_count > 0, |this| {
                let head = t!("{files} not done:", files = plural(report.failed_count, "file"));
                this.child(
                    col()
                        .id("compress-failed")
                        .role(Role::List)
                        .aria_label(head.clone())
                        .test_support()
                        .gap(space(0.5))
                        .rounded(theme::size::RADIUS)
                        .bg(t.warning.opacity(0.1))
                        .px(space(3.))
                        .py(space(2.))
                        .text_size(theme::size::XS)
                        .child(div().font_weight(FontWeight::MEDIUM).child(head))
                        .children(report.failed.iter().take(6).enumerate().map(|(ix, (path, why))| {
                            let line: SharedString = format!("{}: {why}", base_name(path)).into();
                            div()
                                .id(("compress-failure", ix))
                                .role(Role::ListItem)
                                .aria_label(line.clone())
                                .test_support()
                                .truncate()
                                .child(isolate(line))
                        }))
                        .when(report.failed_count > 6, |this| {
                            let more = t!("and {n} more", n = format_count(report.failed_count - 6));
                            this.child(
                                div()
                                    .id("compress-more")
                                    .role(Role::ListItem)
                                    .aria_label(more.clone())
                                    .test_support()
                                    .child(more),
                            )
                        }),
                )
            })
    }
}

impl Render for CompressInPlaceDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let running = self.is_running();
        let finished = matches!(self.phase, Phase::Done { .. } | Phase::Failed { .. });

        let body = match &self.phase {
            Phase::Setup => {
                let intro = t!(
                    "Windows stores the files compressed and decompresses them as they're read, so they open as before, just taking less space. Best for apps, games, tools and old projects; a file an app changes is stored whole again."
                );
                col()
                    .gap(space(4.))
                    .child(div().text_color(t.muted_foreground).child(intro))
                    .children(self.measure_error.clone().map(|reason| {
                        div()
                            .id("compress-measure-error")
                            .role(Role::Alert)
                            .aria_label(reason.clone())
                            .test_support()
                            .text_color(t.destructive)
                            .child(reason)
                    }))
                    .when(self.measure.is_none() && self.measure_error.is_none(), |this| {
                        this.child(
                            align_row(2.)
                                .id("compress-measuring")
                                .role(Role::Status)
                                .aria_label(t!("Measuring what's there…"))
                                .test_support()
                                .text_color(t.muted_foreground)
                                .child(Spinner::new())
                                .child(t!("Measuring what's there…")),
                        )
                    })
                    .children(self.measure.as_ref().map(|measure| {
                        if measure.ntfs {
                            self.setup_view(measure, &t, cx).into_any_element()
                        } else {
                            let warning =
                                t!("Compress in place works on NTFS drives only: this drive is formatted another way.");
                            align_row(2.)
                                .id("compress-not-ntfs")
                                .role(Role::Alert)
                                .aria_label(warning.clone())
                                .test_support()
                                .items_start()
                                .rounded(theme::size::RADIUS)
                                .bg(t.warning.opacity(0.1))
                                .px(space(3.))
                                .py(space(2.))
                                .text_size(theme::size::XS)
                                .child(icon(IconName::TriangleAlert).size_4().flex_none().text_color(t.warning))
                                .child(div().flex_1().min_w_0().child(warning))
                                .into_any_element()
                        }
                    }))
                    .into_any_element()
            }
            Phase::Running { progress, decompress } => {
                Self::progress_view(progress, *decompress, &t).into_any_element()
            }
            Phase::Failed { reason, decompress } => {
                let head = if *decompress {
                    t!("Couldn't decompress them")
                } else {
                    t!("Couldn't compress them")
                };
                col()
                    .id("compress-error")
                    .role(Role::Alert)
                    .aria_label(format!("{head}. {reason}"))
                    .test_support()
                    .items_center()
                    .gap(space(2.))
                    .py(space(6.))
                    .text_center()
                    .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                    .child(div().font_weight(FontWeight::MEDIUM).child(head))
                    .child(note(reason.clone(), &t).max_w(space(112.)))
                    .into_any_element()
            }
            Phase::Done { report, decompress } => Self::result_view(report, *decompress, &t).into_any_element(),
        };

        let can_undo = matches!(&self.phase, Phase::Done { report, decompress: false } if report.undo.is_some() && report.done > 0);
        let footer = row_start()
            .justify_end()
            .gap(space(2.))
            .pt(space(3.))
            .border_t_1()
            .border_color(t.border)
            .when(running, |this| {
                this.child(
                    Button::new("compress-stop")
                        .outline()
                        .icon(IconName::Square)
                        .label(t!("Stop"))
                        .on_click(cx.listener(|this, _, _, _| this.stop())),
                )
            })
            .when(can_undo, |this| {
                this.child(
                    Button::new("compress-undo")
                        .outline()
                        .icon(IconName::Undo2)
                        .label(t!("Undo"))
                        .on_click(cx.listener(|this, _, window, cx| this.undo(window, cx))),
                )
            })
            .when(finished, |this| {
                this.child(
                    Button::new("compress-close")
                        .primary()
                        .label(t!("Close"))
                        .on_click(|_, window, cx| Self::close(window, cx)),
                )
            })
            .when(!finished, |this| {
                this.child(
                    Button::new("compress-cancel")
                        .outline()
                        .label(t!("Cancel"))
                        .disabled(running)
                        .on_click(|_, window, cx| Self::close(window, cx)),
                )
                .child(
                    Button::new("compress-start")
                        .primary()
                        .icon(IconName::Shrink)
                        .label(t!("Compress"))
                        .disabled(running || !self.can_compress())
                        .on_click(cx.listener(|this, _, window, cx| this.run(false, window, cx))),
                )
            });

        col()
            .id("compress-in-place")
            .role(Role::Group)
            .aria_label(self.title())
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(
                note(isolate(self.place.clone()), &t)
                    .id("compress-place")
                    .role(Role::Group)
                    .aria_label(self.place.clone())
                    .test_support(),
            )
            .child(body)
            .child(footer)
    }
}

/// Opens Compress in place on `paths` in `window` (`openCompressInPlace`). Paths on shares are left out; when
/// none is left it says so and opens nothing.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    let chosen = usable_paths(&paths);
    if chosen.is_empty() {
        window.push_notification(
            Notification::info(t!("Compress in place works on files and folders on this PC's drives")),
            cx,
        );
        return;
    }
    let backend = backend(cx);
    open_with(window, cx, chosen, backend);
}

/// [`open`] for a given backend, without filtering `paths` (tests pass a fake).
pub fn open_with(window: &mut Window, cx: &mut App, paths: Vec<String>, backend: Arc<dyn Backend>) {
    let view = cx.new(|cx| CompressInPlaceDialog::new(paths, backend, cx));
    let title = view.read(cx).title();
    super::show(window, cx, title, px(576.), view);
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use glint_core::wof::Estimate;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A backend that answers from memory, remembers what it was asked, and can hold a run until released.
    struct Fake {
        measure: Mutex<Option<Result<Measure, String>>>,
        report: Mutex<Option<Result<Report, String>>>,
        asked: Mutex<Vec<String>>,
        cancels: AtomicUsize,
        gate: Option<async_channel::Receiver<()>>,
    }

    impl Fake {
        fn new(measure: Measure, report: Result<Report, String>) -> Arc<Fake> {
            Arc::new(Fake {
                measure: Mutex::new(Some(Ok(measure))),
                report: Mutex::new(Some(report)),
                asked: Mutex::new(Vec::new()),
                cancels: AtomicUsize::new(0),
                gate: None,
            })
        }

        fn gated(measure: Measure, report: Report) -> (Arc<Fake>, async_channel::Sender<()>) {
            let (open, gate) = async_channel::bounded(1);
            let fake = Arc::new(Fake {
                measure: Mutex::new(Some(Ok(measure))),
                report: Mutex::new(Some(Ok(report))),
                asked: Mutex::new(Vec::new()),
                cancels: AtomicUsize::new(0),
                gate: Some(gate),
            });
            (fake, open)
        }

        fn log(&self, what: String) {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).push(what);
        }

        fn asked(&self) -> Vec<String> {
            self.asked.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }

        fn run(&self, what: String, progress: Channel<Progress>) -> Fut<Report> {
            self.log(what);
            let _ = progress.send(Progress {
                done: 3,
                total: 12,
                current: "big.bin".into(),
                saved: 2 << 20,
            });
            let answer = self.report.lock().unwrap_or_else(|e| e.into_inner()).take();
            let gate = self.gate.clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    let _ = gate.recv().await;
                }
                answer.unwrap_or_else(|| Err("asked twice".into()))
            })
        }
    }

    impl Backend for Fake {
        fn measure(&self, paths: Vec<String>) -> Fut<Measure> {
            self.log(format!("measure {}", paths.join("|")));
            let answer = self.measure.lock().unwrap_or_else(|e| e.into_inner()).take();
            Box::pin(async move { answer.unwrap_or_else(|| Err("asked twice".into())) })
        }

        fn compress(&self, paths: Vec<String>, algorithm: Algorithm, progress: Channel<Progress>) -> Fut<Report> {
            self.run(
                format!("compress {} {}", algorithm_name(algorithm), paths.join("|")),
                progress,
            )
        }

        fn decompress(&self, paths: Vec<String>, progress: Channel<Progress>) -> Fut<Report> {
            self.run(format!("decompress {}", paths.join("|")), progress)
        }

        fn undo(&self, id: u64, progress: Channel<Progress>) -> Fut<Report> {
            self.run(format!("undo {id}"), progress)
        }

        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
        }
    }

    const MB: u64 = 1 << 20;

    fn measure() -> Measure {
        let estimate = |algorithm, after| Estimate { algorithm, after };
        Measure {
            ntfs: true,
            files: 12,
            bytes: 100 * MB,
            already: 2,
            already_bytes: 10 * MB,
            already_on_disk: 4 * MB,
            wof: 0,
            skipped: 5,
            estimates: vec![
                estimate("xpress4k", 70 * MB),
                estimate("xpress8k", 60 * MB),
                estimate("xpress16k", 55 * MB),
                estimate("lzx", 40 * MB),
            ],
            stopped: false,
        }
    }

    fn report() -> Report {
        Report {
            done: 12,
            unchanged: 1,
            before: 100 * MB,
            after: 60 * MB,
            undo: Some(7),
            elapsed_ms: 1234.0,
            ..Default::default()
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

    fn has_id(cx: &mut VisualTestContext, id: &'static str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
    }

    fn click(cx: &mut VisualTestContext, id: &'static str) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
    }

    fn click_nth(cx: &mut VisualTestContext, id: &'static str, n: usize) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.click((id, n), cx);
        });
    }

    /// Lets core's threads and the foreground tasks trade until `done`.
    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the dialog never reached the expected state");
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

    fn open_view(cx: &mut VisualTestContext, paths: &[&str], fake: &Arc<Fake>) -> Entity<CompressInPlaceDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let view = cx.update(|window, cx| {
            let view = cx.new(|cx| CompressInPlaceDialog::new(paths, backend, cx));
            let title = view.read(cx).title();
            crate::dialogs::show(window, cx, title, px(576.), view.clone());
            view
        });
        cx.run_until_parked();
        view
    }

    #[test]
    fn the_icons_this_file_draws_are_embedded() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        for name in [
            IconName::Shrink,
            IconName::Expand,
            IconName::Square,
            IconName::Undo2,
            IconName::CircleCheck,
            IconName::TriangleAlert,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn only_paths_on_this_pc_are_taken_each_once() {
        assert!(can_compress_in_place("C:\\Games\\Big"));
        assert!(!can_compress_in_place("\\\\server\\share\\a"));
        assert!(!can_compress_in_place("  "));
        let chosen = usable_paths(&[
            "C:\\a".into(),
            "\\\\server\\share\\b".into(),
            "c:\\A".into(),
            "D:\\b".into(),
        ]);
        assert_eq!(chosen, ["C:\\a", "D:\\b"]);
        assert!(usable_paths(&["\\\\x\\y".into()]).is_empty());
    }

    #[test]
    fn algorithm_names_match_cores_estimates() {
        let names: Vec<_> = ALGORITHMS.iter().map(|a| algorithm_name(*a)).collect();
        assert_eq!(names, ["xpress4k", "xpress8k", "xpress16k", "lzx"]);
        let _guard = testing::lock();
        assert_eq!(hint(Algorithm::Xpress8k).0, "Balanced");
        assert_eq!(hint(Algorithm::Lzx).1, "Most saved; for things used rarely");
    }

    #[gpui_kit::test]
    fn it_measures_off_the_ui_thread_then_shows_what_is_there_and_what_each_would_save(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(measure(), Ok(report()));
        let view = open_view(&mut visual, &["C:\\Games\\Big"], &fake);
        assert_eq!(fake.asked(), ["measure C:\\Games\\Big"]);
        assert_eq!(role_of(&mut visual, "Compress in place: Big"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "C:\\Games"), Some(Role::Group));

        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        assert!(!has(&mut visual, "Measuring what's there…"));
        for fact in [
            "2 files compressed already: 10 MB taking 4.0 MB",
            "5 files left alone: photos, videos, archives and other kinds that are compressed inside, tiny files, links and online-only files",
        ] {
            assert_eq!(role_of(&mut visual, fact), Some(Role::ListItem), "{fact}");
        }
        // Each radio names its algorithm and what it would save, from the sample.
        assert_eq!(
            role_of(&mut visual, "Light. About 30 MB saved"),
            Some(Role::RadioButton)
        );
        assert_eq!(
            role_of(&mut visual, "Balanced. About 40 MB saved"),
            Some(Role::RadioButton)
        );
        assert_eq!(
            role_of(&mut visual, "Strongest. About 60 MB saved"),
            Some(Role::RadioButton)
        );
        assert_eq!(role_of(&mut visual, "How hard"), Some(Role::RadioGroup));
        assert!(has(
            &mut visual,
            "About 40 MB saved (40%), measured on a sample. Windows' own default. Ctrl+Z decompresses them."
        ));
        // Windows' default is chosen; picking the strongest moves the sentence.
        assert_eq!(view.read_with(&visual, |v, _| v.algorithm()), Algorithm::Xpress8k);
        click_nth(&mut visual, "compress-algorithm", 3);
        assert_eq!(view.read_with(&visual, |v, _| v.algorithm()), Algorithm::Lzx);
        assert!(has(
            &mut visual,
            "About 60 MB saved (60%), measured on a sample. Most saved; for things used rarely. Ctrl+Z decompresses them."
        ));
        // Nothing is compressed already by this dialog, so there is no Decompress.
        assert!(!has(&mut visual, "Decompress"));
    }

    #[gpui_kit::test]
    fn compress_runs_with_the_chosen_algorithm_shows_progress_then_the_result_and_undo(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (fake, release) = Fake::gated(measure(), report());
        let view = open_view(&mut visual, &["C:\\Games\\Big", "D:\\Tools"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        // Two folders: the heading counts them and the place says so.
        assert_eq!(role_of(&mut visual, "Compress in place: 2 items"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "In 2 folders"), Some(Role::Group));

        click_nth(&mut visual, "compress-algorithm", 2);
        click(&mut visual, "compress-start");
        assert!(view.read_with(&visual, |v, _| v.is_running()));
        // The progress line is a progress indicator with the numbers; Stop is there, Cancel is off.
        settle(&mut visual, |cx| {
            has(cx, "Compressing 3 of 12 · big.bin · 2.0 MB saved")
        });
        assert_eq!(
            role_of(&mut visual, "Compressing 3 of 12 · big.bin · 2.0 MB saved"),
            Some(Role::ProgressIndicator)
        );
        assert_eq!(
            fake.asked()[1],
            "compress xpress16k C:\\Games\\Big|D:\\Tools",
            "the chosen algorithm and all the paths"
        );
        click(&mut visual, "compress-stop");
        assert_eq!(fake.cancels.load(Ordering::SeqCst), 1);

        release.try_send(()).ok();
        settle(&mut visual, |cx| has(cx, "Compressed 12 files"));
        assert_eq!(role_of(&mut visual, "Compressed 12 files"), Some(Role::Status));
        assert!(has(&mut visual, "100 MB → 60 MB on the drive (−40 MB) · 1.23 s"));
        assert!(has(
            &mut visual,
            "1 file wouldn't get smaller, so Windows left them as they were."
        ));
        // Close ends it; Undo is offered because core gave an undo id.
        assert!(has_id(&mut visual, "compress-close") && has_id(&mut visual, "compress-undo"));
        assert!(!has_id(&mut visual, "compress-start"), "the start button is gone");
    }

    /// The explorer Ctrl+Z reaches, on a fake disk, with a fake for the steps core would do.
    fn with_explorer(cx: &mut VisualTestContext) -> (Entity<explorer::Explorer>, Arc<explorer::FakeUndoRunner>) {
        let disk = explorer::fake::FakeDisk::new();
        disk.folder("C:\\", Vec::new());
        let source = disk.source();
        cx.update(|_, cx| {
            explorer::init_with(cx, source);
            let runner = explorer::FakeUndoRunner::install(cx);
            (explorer::explorer(cx), runner)
        })
    }

    fn next_undo(cx: &mut VisualTestContext, explorer: &Entity<explorer::Explorer>) -> Option<String> {
        explorer.read_with(cx, |e, _| e.next_undo().map(|(_, label)| label.to_string()))
    }

    #[gpui_kit::test]
    fn a_finished_compression_is_one_ctrl_z_and_a_toast_with_undo(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (explorer, runner) = with_explorer(&mut visual);
        explorer.update(&mut visual, |e, cx| {
            e.remember_undo(
                "copy of 1 item",
                explorer::UndoAction::Recycle(vec!["C:\\x".into()]),
                cx,
            );
        });
        let fake = Fake::new(measure(), Ok(report()));
        let _view = open_view(&mut visual, &["C:\\Games\\Big"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| has(cx, "Compressed 12 files"));
        // Kept for Ctrl+Z under the page's label, above the older step.
        assert_eq!(
            next_undo(&mut visual, &explorer).as_deref(),
            Some("compressing 12 files in place")
        );
        // The notification says it with a capital and offers Undo (the dialog is modal: it is closed first).
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        settle(&mut visual, |cx| has_id(cx, "compress-undo-toast"));

        // Ctrl+Z decompresses with core's `wof_undo` (the runner), not through the dialog's backend.
        let task = explorer.update(&mut visual, |e, cx| e.undo(None, cx));
        visual.foreground_executor().block_test(task);
        visual.run_until_parked();
        assert_eq!(runner.wof_asked(), [7]);
        assert!(!fake.asked().iter().any(|a| a.starts_with("undo")));
        assert_eq!(next_undo(&mut visual, &explorer).as_deref(), Some("copy of 1 item"));
    }

    #[gpui_kit::test]
    fn the_toasts_undo_takes_the_compression_back_through_the_same_step(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (explorer, runner) = with_explorer(&mut visual);
        let fake = Fake::new(measure(), Ok(report()));
        let _view = open_view(&mut visual, &["C:\\Games\\Big"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| has(cx, "Compressed 12 files"));
        // The dialog is modal: it is closed, and the toast's Undo is under the pointer.
        visual.update(|window, cx| window.press("escape", cx));
        visual.run_until_parked();
        settle(&mut visual, |cx| has_id(cx, "compress-undo-toast"));
        click(&mut visual, "compress-undo-toast");
        settle(&mut visual, |_| runner.wof_asked() == [7]);
        assert_eq!(next_undo(&mut visual, &explorer), None);
    }

    #[gpui_kit::test]
    fn the_results_own_undo_uses_up_the_step_so_ctrl_z_does_not_undo_twice(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (explorer, runner) = with_explorer(&mut visual);
        let fake = Fake::new(measure(), Ok(report()));
        let _view = open_view(&mut visual, &["C:\\Games\\Big"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| has(cx, "Compressed 12 files"));
        assert!(next_undo(&mut visual, &explorer).is_some());
        *fake.report.lock().unwrap() = Some(Ok(Report {
            done: 12,
            before: 60 * MB,
            after: 100 * MB,
            ..Default::default()
        }));
        click(&mut visual, "compress-undo");
        settle(&mut visual, |cx| has(cx, "Decompressed 12 files"));
        // The dialog ran core's undo with its progress; the explorer's step is gone and ran nothing.
        assert!(fake.asked().contains(&"undo 7".to_string()));
        assert_eq!(next_undo(&mut visual, &explorer), None);
        assert!(runner.wof_asked().is_empty());
    }

    #[gpui_kit::test]
    fn only_a_compression_core_can_take_back_is_kept(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let (explorer, _runner) = with_explorer(&mut visual);
        // Nothing compressed, so core gave no undo id: nothing to keep or offer.
        let fake = Fake::new(
            measure(),
            Ok(Report {
                done: 0,
                undo: None,
                ..report()
            }),
        );
        let _view = open_view(&mut visual, &["C:\\Games\\Big"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| has_id(cx, "compress-close"));
        assert_eq!(next_undo(&mut visual, &explorer), None);
        assert!(!has_id(&mut visual, "compress-undo-toast"));

        // A decompression is not kept either.
        let fake = Fake::new(
            Measure {
                wof: 4,
                files: 0,
                ..measure()
            },
            Ok(Report {
                done: 4,
                undo: Some(9),
                ..Default::default()
            }),
        );
        let _view = open_view(&mut visual, &["C:\\Games\\Small"], &fake);
        settle(&mut visual, |cx| has_id(cx, "compress-decompress"));
        click(&mut visual, "compress-decompress");
        settle(&mut visual, |cx| has(cx, "Decompressed 4 files"));
        assert_eq!(next_undo(&mut visual, &explorer), None);
    }

    #[gpui_kit::test]
    fn undo_decompresses_what_was_just_compressed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(measure(), Ok(report()));
        let view = open_view(&mut visual, &["C:\\Games\\Big"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| has(cx, "Compressed 12 files"));
        // The fake answers once; give it the undo's report.
        *fake.report.lock().unwrap() = Some(Ok(Report {
            done: 12,
            before: 60 * MB,
            after: 100 * MB,
            ..Default::default()
        }));
        click(&mut visual, "compress-undo");
        settle(&mut visual, |cx| has(cx, "Decompressed 12 files"));
        assert!(fake.asked().contains(&"undo 7".to_string()));
        assert!(view.read_with(&visual, |v, _| v.report().is_some()));
        assert!(has(&mut visual, "60 MB → 100 MB on the drive (+40 MB) · 0.0 ms"));
        assert!(
            !has_id(&mut visual, "compress-undo"),
            "a decompression can't be undone again"
        );
    }

    #[gpui_kit::test]
    fn already_compressed_files_offer_decompress(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(
            Measure {
                wof: 4,
                files: 0,
                ..measure()
            },
            Ok(Report {
                done: 4,
                before: 10 * MB,
                after: 30 * MB,
                ..Default::default()
            }),
        );
        open_view(&mut visual, &["C:\\Games\\Big"], &fake);
        settle(&mut visual, |cx| {
            has(
                cx,
                "4 files here are compressed this way. Decompressing needs as much free space as they'll take.",
            )
        });
        // Nothing new to compress: the algorithms aren't offered and Compress is off.
        assert!(!has(&mut visual, "How hard"));
        click(&mut visual, "compress-start");
        assert_eq!(fake.asked().len(), 1, "a disabled Compress asks nothing");
        click(&mut visual, "compress-decompress");
        settle(&mut visual, |cx| has(cx, "Decompressed 4 files"));
        assert_eq!(fake.asked()[1], "decompress C:\\Games\\Big");
        assert!(has(&mut visual, "10 MB → 30 MB on the drive (+20 MB) · 0.0 ms"));
    }

    #[gpui_kit::test]
    fn a_drive_that_is_not_ntfs_says_so_and_cannot_compress(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(
            Measure {
                ntfs: false,
                ..measure()
            },
            Ok(report()),
        );
        let view = open_view(&mut visual, &["E:\\Usb"], &fake);
        let warning = "Compress in place works on NTFS drives only: this drive is formatted another way.";
        settle(&mut visual, |cx| has(cx, warning));
        assert_eq!(role_of(&mut visual, warning), Some(Role::Alert));
        assert!(!view.read_with(&visual, |v, _| v.can_compress()));
        assert!(!has(&mut visual, "How hard"));
    }

    #[gpui_kit::test]
    fn a_failed_measuring_and_a_failed_run_are_alerts(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(measure(), Err("The drive is read-only".into()));
        *fake.measure.lock().unwrap() = Some(Err("Access is denied".into()));
        let view = open_view(&mut visual, &["C:\\x"], &fake);
        settle(&mut visual, |cx| has(cx, "Access is denied"));
        assert_eq!(role_of(&mut visual, "Access is denied"), Some(Role::Alert));
        assert!(!has(&mut visual, "Measuring what's there…"));
        assert!(!view.read_with(&visual, |v, _| v.can_compress()));

        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        let fake = Fake::new(measure(), Err("The drive is read-only".into()));
        let _ = open_view(&mut visual, &["C:\\y"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| {
            has(cx, "Couldn't compress them. The drive is read-only")
        });
        assert_eq!(
            role_of(&mut visual, "Couldn't compress them. The drive is read-only"),
            Some(Role::Alert)
        );
    }

    #[gpui_kit::test]
    fn failures_are_listed_six_at_a_time_with_a_count_of_the_rest(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let failed: Vec<(String, String)> = (0..8)
            .map(|i| (format!("C:\\Games\\f{i}.bin"), "In use".to_string()))
            .collect();
        let fake = Fake::new(
            measure(),
            Ok(Report {
                done: 4,
                failed,
                failed_count: 8,
                ..report()
            }),
        );
        open_view(&mut visual, &["C:\\Games"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| has(cx, "8 files not done:"));
        assert!(has(&mut visual, "f0.bin: In use") && has(&mut visual, "f5.bin: In use"));
        assert!(!has(&mut visual, "f6.bin: In use"));
        assert_eq!(role_of(&mut visual, "and 2 more"), Some(Role::ListItem));
    }

    #[gpui_kit::test]
    fn escape_closes_the_dialog_and_stops_the_work(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Fake::new(measure(), Ok(report()));
        open_view(&mut visual, &["C:\\Games"], &fake);
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            assert!(window.focused(cx).is_some(), "focus starts inside the dialog");
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        assert!(
            fake.cancels.load(Ordering::SeqCst) >= 1,
            "closing stops what was started"
        );
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = Fake::new(measure(), Ok(report()));
        let view = open_view(&mut visual, &["C:\\مستندات\\تقرير الربع"], &fake);
        settle(&mut visual, |cx| has(cx, "12 files to compress, 100 MB in all"));
        // The title carries the name inside isolates (display only); the accessible group is the dialog's.
        assert!(view.read_with(&visual, |v, _| v.title().contains("تقرير الربع")));
        assert_eq!(role_of(&mut visual, "C:\\مستندات"), Some(Role::Group));
        click(&mut visual, "compress-start");
        settle(&mut visual, |cx| has(cx, "Compressed 12 files"));
        testing::set_rtl(false);
    }
}
