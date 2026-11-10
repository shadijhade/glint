//! What holds a file open, and stopping it. Ported from src/components/explorer/LocksDialog.tsx, built the
//! way [`super::drive_health`] is.
//!
//! | TypeScript                                   | Here                                                     |
//! |----------------------------------------------|----------------------------------------------------------|
//! | `useLocksStore` + `inspectLocks(paths)`      | [`open`]                                                 |
//! | `whoLocks`, `stopLockingProcess`             | [`Source`] (glint-core's `who_locks`, `stop_locking_process`) |
//! | `LocksModal`'s state                         | [`Locks`], a model entity                                |
//! | `LocksModal`                                 | [`LocksDialog`]                                          |
//! | `confirm(...)` before ending                 | an inline question in the dialog ([`Ask`]) with End / Cancel |
//! | toasts                                       | notifications, raised by the view from [`LocksEvent`]    |
//! | `revealInApp(exe)` after closing             | the `on_reveal` callback [`open`] takes (the dialog closes first) |
//!
//! Ending a process asks first, in the dialog itself rather than in a second window: the question names the
//! process (or the programs, for End all) and warns that unsaved work may be lost, as the TypeScript's did.
//! File Explorer and services are never part of End all (the desktop goes with the first; services need one
//! choice each).
//!
//! The process list is short (a handful of programs), so it is a plain scrolling column rather than a
//! virtualized one.

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::locks::{LockReport, LockingProcess};
use glint_model::format::{format_count, format_duration, plural};
use glint_model::paths::base_name;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Where the answers come from: glint-core in the app, fakes in tests.
#[derive(Clone)]
pub struct Source {
    /// Who has these files open (`who_locks`).
    pub scan: Arc<dyn Fn(Vec<String>) -> Fut<LockReport> + Send + Sync>,
    /// Ends a process: pid and the two halves of its start time, which guard against a reused pid.
    pub stop: Arc<dyn Fn(u32, u32, u32) -> Fut<()> + Send + Sync>,
}

/// glint-core's lock inspector, as a [`Source`].
pub fn core_source() -> Source {
    Source {
        scan: Arc::new(|paths| Box::pin(glint_core::locks::who_locks(paths))),
        stop: Arc::new(|pid, low, high| Box::pin(glint_core::locks::stop_locking_process(pid, low, high))),
    }
}

// ---- The model ------------------------------------------------------------------------------------------

/// Where a scan stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// A scan is under way (the first one shows a spinner; later ones keep the old list).
    Loading,
    /// The last scan failed, with the reason.
    Failed(SharedString),
    /// `report` holds the last scan.
    Ready,
}

/// What is being ended right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stopping {
    Nothing,
    /// One process, by pid.
    One(u32),
    /// End all (the TypeScript's `-1`).
    All,
}

/// Said by the model; the view turns each into a notification (the TypeScript's toasts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocksEvent {
    /// `Ended {name} (PID {pid})`.
    Ended { name: String, pid: u32 },
    /// `Ended {n} processes` after End all (only when at least one stopped).
    EndedMany(usize),
    /// `Couldn't end {name}` with the reason.
    CouldNotEnd { name: String, reason: String },
}

/// What End all ends: not File Explorer (the desktop goes with it) or services, which need one choice each.
pub fn endable(p: &LockingProcess) -> bool {
    p.stoppable && p.kind != "explorer" && p.kind != "service"
}

/// The lock inspector's data: the report for some paths, and the stopping that follows.
pub struct Locks {
    paths: Vec<String>,
    report: Option<LockReport>,
    state: State,
    stopping: Stopping,
    /// Bumped by every scan; an older answer is dropped.
    generation: u64,
    source: Source,
}

impl EventEmitter<LocksEvent> for Locks {}

impl Locks {
    /// A model for `paths`, scanning at once.
    pub fn new(paths: Vec<String>, source: Source, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            paths,
            report: None,
            state: State::Loading,
            stopping: Stopping::Nothing,
            generation: 0,
            source,
        };
        this.scan(cx);
        this
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn report(&self) -> Option<&LockReport> {
        self.report.as_ref()
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn stopping(&self) -> Stopping {
        self.stopping
    }

    /// The programs End all would end.
    pub fn endable(&self) -> Vec<LockingProcess> {
        self.report
            .as_ref()
            .map(|r| r.processes.iter().filter(|p| endable(p)).cloned().collect())
            .unwrap_or_default()
    }

    /// Asks who has the paths open. Runs on core's runtime; the UI thread doesn't wait.
    pub fn scan(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.state = State::Loading;
        cx.notify();
        let read = (self.source.scan)(self.paths.clone());
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
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

    /// Ends one process, then scans again.
    pub fn stop_one(&mut self, proc: &LockingProcess, cx: &mut Context<Self>) {
        if self.stopping != Stopping::Nothing {
            return;
        }
        self.stopping = Stopping::One(proc.pid);
        cx.notify();
        let (pid, low, high) = (proc.pid, proc.start_time_low, proc.start_time_high);
        let name = proc.name.clone();
        let stop = (self.source.stop)(pid, low, high);
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(stop).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.stopping = Stopping::Nothing;
                match result {
                    Ok(()) => {
                        cx.emit(LocksEvent::Ended { name, pid });
                        this.scan(cx);
                    }
                    Err(reason) => {
                        cx.emit(LocksEvent::CouldNotEnd { name, reason });
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Ends every [`Locks::endable`] process one after the other, then scans again.
    pub fn stop_all(&mut self, cx: &mut Context<Self>) {
        let targets = self.endable();
        if targets.is_empty() || self.stopping != Stopping::Nothing {
            return;
        }
        self.stopping = Stopping::All;
        cx.notify();
        let source = self.source.clone();
        cx.spawn(async move |this, cx| {
            let mut stopped = 0;
            let mut failures = Vec::new();
            for proc in targets {
                let one = (source.stop)(proc.pid, proc.start_time_low, proc.start_time_high);
                match core_bridge::run(one).await.and_then(|inner| inner) {
                    Ok(()) => stopped += 1,
                    Err(reason) => failures.push((proc.name, reason)),
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.stopping = Stopping::Nothing;
                for (name, reason) in failures {
                    cx.emit(LocksEvent::CouldNotEnd { name, reason });
                }
                if stopped > 0 {
                    cx.emit(LocksEvent::EndedMany(stopped));
                }
                this.scan(cx);
            });
        })
        .detach();
    }
}

// ---- Words ------------------------------------------------------------------------------------------------

/// The badge on a process: its service, or what kind of program it is.
pub fn kind_badge(proc: &LockingProcess) -> SharedString {
    if let Some(service) = &proc.service {
        return t!("Service: {name}", name = service);
    }
    match proc.kind.as_str() {
        "console" => t!("Console"),
        "service" => t!("Service"),
        "explorer" => t!("Explorer"),
        "critical" => t!("System critical"),
        _ => t!("Application"),
    }
}

fn process_icon(kind: &str) -> IconName {
    match kind {
        "console" => IconName::SquareTerminal,
        "service" => IconName::Cog,
        "explorer" => IconName::Folder,
        "critical" => IconName::ShieldAlert,
        _ => IconName::AppWindow,
    }
}

fn process_tone(kind: &str, t: &theme::Tokens) -> Hsla {
    match kind {
        "console" | "explorer" => t.warning,
        "service" => theme::tailwind("sky-500").unwrap_or(t.primary),
        "critical" => t.destructive,
        _ => t.primary,
    }
}

/// The dialog's title: the item's name, or how many items.
pub fn title_for(paths: &[String]) -> SharedString {
    match paths {
        [one] => {
            let name = base_name(one);
            t!(
                "What's locking “{name}”",
                name = if name.is_empty() { one } else { name }
            )
        }
        _ => t!("What's locking {n} items", n = format_count(paths.len())),
    }
}

/// The line under the title: the path, or the count.
fn subtitle_for(paths: &[String]) -> SharedString {
    match paths {
        [one] => one.clone().into(),
        _ => t!("{n} items", n = paths.len()),
    }
}

/// The footer line: how many programs, how many files were looked at, and how long it took.
pub fn summary(report: &LockReport) -> SharedString {
    t!(
        "{programs} · {files} checked in {time}",
        programs = plural(report.processes.len(), "program"),
        files = plural(report.files_checked, "file"),
        time = format_duration(report.elapsed_ms)
    )
}

// ---- The view ---------------------------------------------------------------------------------------------

/// A question waiting for an answer: ending is never done on a click alone.
#[derive(Clone, Debug)]
enum Ask {
    One(LockingProcess),
    All(Vec<LockingProcess>),
}

type OnReveal = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Who has the items open, with End for each program that can be ended.
pub struct LocksDialog {
    locks: Entity<Locks>,
    on_reveal: OnReveal,
    ask: Option<Ask>,
    _subscriptions: Vec<Subscription>,
}

impl LocksDialog {
    pub fn new(locks: Entity<Locks>, on_reveal: OnReveal, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let redraw = cx.observe(&locks, |_, _, cx| cx.notify());
        let events = cx.subscribe_in(&locks, window, |_, _, event: &LocksEvent, window, cx| match event {
            LocksEvent::Ended { name, pid } => window.push_notification(
                Notification::success(t!("Ended {name} (PID {pid})", name = name, pid = pid)),
                cx,
            ),
            LocksEvent::EndedMany(n) => window.push_notification(
                Notification::success(if *n == 1 {
                    t!("Ended 1 process")
                } else {
                    t!("Ended {n} processes", n = format_count(*n))
                }),
                cx,
            ),
            LocksEvent::CouldNotEnd { name, reason } => window.push_notification(
                Notification::error(SharedString::from(reason.clone())).title(t!("Couldn't end {name}", name = name)),
                cx,
            ),
        });
        Self {
            locks,
            on_reveal,
            ask: None,
            _subscriptions: vec![redraw, events],
        }
    }

    /// The question in words: the TypeScript's `confirm` text, title first.
    fn question(ask: &Ask) -> (SharedString, SharedString, SharedString) {
        match ask {
            Ask::One(proc) => (
                t!("End locking process"),
                format!(
                    "{} {}",
                    t!(
                        "End “{name}” (PID {pid}) to release its file handles?",
                        name = proc.name,
                        pid = proc.pid
                    ),
                    t!("Unsaved work in that process may be lost.")
                )
                .into(),
                t!("End process"),
            ),
            Ask::All(procs) => {
                let names = procs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ");
                (
                    t!("End all locking processes"),
                    format!(
                        "{} {}",
                        t!(
                            "End {programs} ({names})?",
                            programs = plural(procs.len(), "program"),
                            names = names
                        ),
                        t!("Unsaved work in them may be lost. File Explorer and services are left running; end those one at a time.")
                    )
                    .into(),
                    t!("End all"),
                )
            }
        }
    }

    fn process_row(
        &self,
        proc: &LockingProcess,
        busy: bool,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tone = process_tone(&proc.kind, t);
        let badge = kind_badge(proc);
        let pid = proc.pid;
        let name = SharedString::from(proc.name.clone());
        let ending_this = self.locks.read(cx).stopping() == Stopping::One(pid);
        let mono = theme::mono_font(cx);

        let actions = row().flex_none().gap(space(1.)).children(proc.exe.clone().map(|exe| {
            let reveal_exe = exe.clone();
            let copy_exe = exe;
            let on_reveal = self.on_reveal.clone();
            row()
                .gap(space(1.))
                .child(
                    Button::new(("locks-reveal", pid as usize))
                        .ghost()
                        .xsmall()
                        .icon(IconName::FolderInput)
                        .tooltip(t!("Reveal executable in Glint"))
                        .accessibility_label(t!("Reveal executable in Glint: {name}", name = proc.name))
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            on_reveal(reveal_exe.clone(), window, cx);
                        }),
                )
                .child(
                    Button::new(("locks-copy", pid as usize))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ClipboardCopy)
                        .tooltip(t!("Copy executable path"))
                        .accessibility_label(t!("Copy executable path: {name}", name = proc.name))
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_exe.clone()));
                            window.push_notification(Notification::success(t!("Copied executable path")), cx);
                        }),
                )
        }));
        let end = proc.stoppable.then(|| {
            let proc = proc.clone();
            let title = t!("End {name} (PID {pid})", name = proc.name, pid = proc.pid);
            Button::new(("locks-end", pid as usize))
                .outline()
                .xsmall()
                .danger()
                .icon(IconName::OctagonX)
                .label(t!("End"))
                .loading(ending_this)
                .disabled(busy)
                .tooltip(title.clone())
                .accessibility_label(title)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ask = Some(Ask::One(proc.clone()));
                    cx.notify();
                }))
        });

        row_start()
            .id(("locks-process", pid as usize))
            .role(Role::Group)
            .aria_label(format!("{} (PID {}): {}", proc.name, pid, badge))
            .test_support()
            .gap(space(3.))
            .px(space(3.))
            .py(space(2.5))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.card.opacity(0.6))
            .child(icon(process_icon(&proc.kind)).size_4().text_color(tone).flex_none())
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .gap(space(0.5))
                    .child(
                        align_row(2.)
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_size(theme::size::SM)
                                    .child(isolate(name)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px(space(1.5))
                                    .py(space(0.5))
                                    .rounded(theme::size::RADIUS_SM)
                                    .border_1()
                                    .border_color(t.border)
                                    .bg(t.muted)
                                    .font_family(mono.clone())
                                    .text_size(theme::size::XXS)
                                    .text_color(t.muted_foreground)
                                    .child(t!("PID {pid}", pid = pid)),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px(space(1.5))
                                    .py(space(0.5))
                                    .rounded(theme::size::RADIUS_SM)
                                    .bg(t.primary.opacity(0.1))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_size(theme::size::XXS)
                                    .text_color(t.primary)
                                    .child(badge),
                            ),
                    )
                    .children(proc.exe.clone().map(|exe| {
                        div()
                            .max_w_full()
                            .truncate()
                            .font_family(mono)
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(SharedString::from(exe))
                    })),
            )
            .child(actions)
            .children(end)
    }

    /// The inline question, with its two answers.
    fn ask_panel(&self, ask: &Ask, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, text, yes) = Self::question(ask);
        let confirm = ask.clone();
        col()
            .id("locks-ask")
            .role(Role::Alert)
            .aria_label(format!("{title}. {text}"))
            .test_support()
            .gap(space(2.))
            .p(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.destructive.opacity(0.4))
            .bg(t.destructive.opacity(0.08))
            .child(
                align_row(2.)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.destructive)
                    .child(icon(IconName::TriangleAlert).size_4())
                    .child(title),
            )
            .child(div().text_size(theme::size::XS).child(text))
            .child(
                align_row(2.)
                    .child(
                        Button::new("locks-ask-yes")
                            .danger()
                            .small()
                            .label(yes)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask = None;
                                let confirm = confirm.clone();
                                this.locks.update(cx, |locks, cx| match &confirm {
                                    Ask::One(proc) => locks.stop_one(proc, cx),
                                    Ask::All(_) => locks.stop_all(cx),
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("locks-ask-no")
                            .ghost()
                            .small()
                            .label(t!("Cancel"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.ask = None;
                                cx.notify();
                            })),
                    ),
            )
    }
}

impl Render for LocksDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let locks = self.locks.read(cx);
        let paths = locks.paths().to_vec();
        let state = locks.state().clone();
        let report = locks.report().cloned();
        let stopping = locks.stopping();
        let busy = stopping != Stopping::Nothing;
        let endable_now = locks.endable();

        let header = align_row(3.)
            .child(
                col().flex_1().min_w_0().align_start().child(
                    div()
                        .max_w_full()
                        .truncate()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(isolate(subtitle_for(&paths))),
                ),
            )
            .child(
                Button::new("locks-refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .loading(state == State::Loading && report.is_some())
                    .tooltip(t!("Refresh"))
                    .accessibility_label(t!("Refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.locks.update(cx, |locks, cx| locks.scan(cx)))),
            );

        let body = match (&state, &report) {
            (State::Loading, None) => col()
                .id("locks-loading")
                .role(Role::Status)
                .aria_label(t!("Looking for apps that have it open…"))
                .test_support()
                .items_center()
                .gap(space(2.))
                .py(space(12.))
                .text_size(theme::size::SM)
                .text_color(t.muted_foreground)
                .child(Spinner::new().color(t.primary))
                .child(t!("Looking for apps that have it open…"))
                .into_any_element(),
            (State::Failed(reason), _) => col()
                .id("locks-error")
                .role(Role::Alert)
                .aria_label(format!("{}. {}", t!("Couldn't find out what's using it"), reason))
                .test_support()
                .items_center()
                .gap(space(2.))
                .py(space(10.))
                .text_center()
                .text_size(theme::size::SM)
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(t!("Couldn't find out what's using it")),
                )
                .child(
                    div()
                        .max_w(rems(28.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(reason.clone()),
                )
                .into_any_element(),
            (_, Some(report)) if report.processes.is_empty() => {
                let them = if report.files_checked == 1 {
                    t!("Nothing is using it")
                } else {
                    t!("Nothing is using them")
                };
                let detail = match report.files_checked {
                    0 => t!("There are no files in it to check."),
                    1 => t!("No program has the file open right now."),
                    n => t!(
                        "No program has any of the {n} files open right now.",
                        n = format_count(n)
                    ),
                };
                col()
                    .id("locks-free")
                    .role(Role::Status)
                    .aria_label(format!("{them}. {detail}"))
                    .test_support()
                    .items_center()
                    .gap(space(2.))
                    .py(space(12.))
                    .text_center()
                    .text_size(theme::size::SM)
                    .child(icon(IconName::LockOpen).size_8().text_color(t.success))
                    .child(div().font_weight(FontWeight::MEDIUM).child(them))
                    .child(
                        div()
                            .max_w(rems(24.))
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(detail),
                    )
                    .into_any_element()
            }
            (_, Some(report)) => col()
                .id("locks-list")
                .role(Role::List)
                .aria_label(t!("Programs using it"))
                .test_support()
                .gap(space(1.5))
                .children(report.processes.iter().map(|proc| self.process_row(proc, busy, &t, cx)))
                .into_any_element(),
            (State::Ready, None) => div().into_any_element(),
        };

        let footer = align_row(3.)
            .justify_between()
            .pt(space(2.))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .child(report.as_ref().map(summary).unwrap_or_default()),
            )
            .children((endable_now.len() > 1).then(|| {
                let all = endable_now.clone();
                Button::new("locks-end-all")
                    .ghost()
                    .xsmall()
                    .danger()
                    .icon(IconName::OctagonX)
                    .label(t!("End all ({n})", n = format_count(all.len())))
                    .loading(stopping == Stopping::All)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ask = Some(Ask::All(all.clone()));
                        cx.notify();
                    }))
            }));

        col()
            .id("locks")
            .role(Role::Group)
            .aria_label(title_for(&paths))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            .child(header)
            .child(
                div()
                    .id("locks-scroll")
                    .max_h(rems(22.))
                    .overflow_y_scroll()
                    .child(body),
            )
            .children(self.ask.as_ref().map(|ask| self.ask_panel(ask, &t, cx)))
            .child(footer)
    }
}

/// Opens the lock inspector for `paths` (files or folders) in `window`. `on_reveal(exe, …)` runs after the
/// dialog closes when the person asks to reveal a program's file in Glint (the TypeScript's `revealInApp`).
pub fn open(
    window: &mut Window,
    cx: &mut App,
    paths: Vec<String>,
    on_reveal: impl Fn(String, &mut Window, &mut App) + 'static,
) {
    open_with(window, cx, paths, core_source(), on_reveal);
}

/// [`open`] with a given [`Source`] (tests pass fakes). Nothing opens for no paths, as in the TypeScript.
pub fn open_with(
    window: &mut Window,
    cx: &mut App,
    paths: Vec<String>,
    source: Source,
    on_reveal: impl Fn(String, &mut Window, &mut App) + 'static,
) {
    if paths.is_empty() {
        return;
    }
    let title = title_for(&paths);
    let locks = cx.new(|cx| Locks::new(paths, source, cx));
    let on_reveal: OnReveal = Rc::new(on_reveal);
    let view = cx.new(|cx| LocksDialog::new(locks, on_reveal, window, cx));
    super::show(window, cx, title, px(640.), view);
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    fn proc(pid: u32, name: &str, kind: &str) -> LockingProcess {
        LockingProcess {
            pid,
            name: name.into(),
            service: None,
            kind: kind.into(),
            exe: Some(format!("C:\\Program Files\\{name}.exe")),
            stoppable: kind != "critical",
            start_time_low: pid * 10,
            start_time_high: 1,
        }
    }

    fn report(processes: Vec<LockingProcess>, files: usize) -> LockReport {
        LockReport {
            paths: vec!["C:\\Docs\\report.docx".into()],
            processes,
            files_checked: files,
            elapsed_ms: 12.4,
        }
    }

    /// Fakes: the scan answers from a queue (the last answer repeats), and the stops are recorded.
    struct Fake {
        source: Source,
        scans: Arc<AtomicUsize>,
        stopped: Arc<Mutex<Vec<(u32, u32, u32)>>>,
    }

    fn fake(answers: Vec<Result<LockReport, String>>, stop_fails: Option<u32>) -> Fake {
        let scans = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(Mutex::new(Vec::new()));
        let (counter, log) = (scans.clone(), stopped.clone());
        let source = Source {
            scan: Arc::new(move |_| {
                let n = counter.fetch_add(1, Ordering::SeqCst);
                let answer = answers[n.min(answers.len() - 1)].clone();
                Box::pin(async move { answer })
            }),
            stop: Arc::new(move |pid, low, high| {
                log.lock().unwrap().push((pid, low, high));
                let fails = stop_fails == Some(pid);
                Box::pin(async move {
                    if fails {
                        Err("Access is denied".to_string())
                    } else {
                        Ok(())
                    }
                })
            }),
        };
        Fake { source, scans, stopped }
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
        panic!("never arrived: {what}");
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

    fn open_fake(visual: &mut VisualTestContext, paths: &[&str], source: Source) -> Rc<RefCell<Vec<String>>> {
        let revealed = Rc::new(RefCell::new(Vec::new()));
        let log = revealed.clone();
        let paths = paths.iter().map(|p| p.to_string()).collect();
        visual.update(|window, cx| open_with(window, cx, paths, source, move |exe, _, _| log.borrow_mut().push(exe)));
        revealed
    }

    #[test]
    fn words_follow_the_typescript() {
        let _guard = testing::lock();
        assert_eq!(title_for(&["C:\\a\\b.txt".into()]), "What's locking “b.txt”");
        assert_eq!(title_for(&["C:\\a".into(), "C:\\b".into()]), "What's locking 2 items");
        let mut service = proc(1, "svc", "app");
        service.service = Some("Spooler".into());
        assert_eq!(kind_badge(&service), "Service: Spooler");
        assert_eq!(kind_badge(&proc(1, "x", "critical")), "System critical");
        assert_eq!(kind_badge(&proc(1, "x", "app")), "Application");
        assert_eq!(
            summary(&report(vec![proc(1, "a", "app")], 3)),
            "1 program · 3 files checked in 12 ms"
        );
        assert!(endable(&proc(1, "a", "app")));
        assert!(!endable(&proc(1, "a", "explorer")));
        assert!(!endable(&proc(1, "a", "service")));
        assert!(!endable(&proc(1, "a", "critical")));
    }

    #[gpui_kit::test]
    fn lists_the_programs_with_labelled_rows_and_a_summary(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = fake(
            vec![Ok(report(
                vec![proc(10, "WINWORD", "app"), proc(11, "explorer", "explorer")],
                1,
            ))],
            None,
        );
        open_fake(&mut visual, &["C:\\Docs\\report.docx"], fake.source);
        // While the scan runs there is a status line, and the UI thread didn't wait for it.
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            assert!(!window.find_all_by_role(Role::Dialog).is_empty());
        });
        settle(&mut visual, "the process rows", |cx| {
            has(cx, "WINWORD (PID 10): Application")
        });
        assert_eq!(role_of(&mut visual, "What's locking “report.docx”"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "explorer (PID 11): Explorer"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "End WINWORD (PID 10)"), Some(Role::Button));
        assert_eq!(
            role_of(&mut visual, "Copy executable path: WINWORD"),
            Some(Role::Button)
        );
        assert!(has(&mut visual, "Programs using it"));
        assert!(!has(&mut visual, "Looking for apps that have it open…"));
        // One program can be ended on its own that End all leaves alone: no End all for a single endable one.
        visual.update(|window, cx| {
            assert!(window.try_find_by_label("End all (1)").is_none());
            let _ = cx;
        });
    }

    #[gpui_kit::test]
    fn ending_a_process_asks_first_then_stops_it_and_scans_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let before = report(vec![proc(10, "WINWORD", "app")], 1);
        let after = report(vec![], 1);
        let fake = fake(vec![Ok(before), Ok(after)], None);
        let (stopped, scans) = (fake.stopped.clone(), fake.scans.clone());
        open_fake(&mut visual, &["C:\\Docs\\report.docx"], fake.source);
        settle(&mut visual, "rows", |cx| has(cx, "WINWORD (PID 10): Application"));

        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("locks-end", 10_usize), cx);
        });
        visual.run_until_parked();
        // Asked, not done.
        assert!(stopped.lock().unwrap().is_empty());
        assert_eq!(role_of(&mut visual, "End locking process. End “WINWORD” (PID 10) to release its file handles? Unsaved work in that process may be lost."), Some(Role::Alert));

        // Cancel leaves everything as it was.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("locks-ask-no", cx);
        });
        visual.run_until_parked();
        assert!(!has(&mut visual, "End locking process. End “WINWORD” (PID 10) to release its file handles? Unsaved work in that process may be lost."));
        assert!(stopped.lock().unwrap().is_empty());

        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("locks-end", 10_usize), cx);
            window.render_frame(cx);
            window.click("locks-ask-yes", cx);
        });
        settle(&mut visual, "the stop and the second scan", |cx| {
            has(cx, "Nothing is using it. No program has the file open right now.")
        });
        assert_eq!(*stopped.lock().unwrap(), vec![(10, 100, 1)]);
        assert_eq!(scans.load(Ordering::SeqCst), 2);
        assert_eq!(
            role_of(
                &mut visual,
                "Nothing is using it. No program has the file open right now."
            ),
            Some(Role::Status)
        );
    }

    #[gpui_kit::test]
    fn end_all_leaves_explorer_and_services_and_names_the_programs(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let listing = report(
            vec![
                proc(10, "WINWORD", "app"),
                proc(11, "explorer", "explorer"),
                proc(12, "notepad", "app"),
                proc(13, "spooler", "service"),
            ],
            4,
        );
        let fake = fake(
            vec![Ok(listing), Ok(report(vec![proc(11, "explorer", "explorer")], 4))],
            Some(12),
        );
        let stopped = fake.stopped.clone();
        open_fake(&mut visual, &["C:\\Docs", "C:\\Other"], fake.source);
        settle(&mut visual, "rows", |cx| has(cx, "End all (2)"));
        assert_eq!(role_of(&mut visual, "What's locking 2 items"), Some(Role::Group));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("locks-end-all", cx);
        });
        visual.run_until_parked();
        let question = "End all locking processes. End 2 programs (WINWORD, notepad)? Unsaved work in them may be lost. File Explorer and services are left running; end those one at a time.";
        assert_eq!(role_of(&mut visual, question), Some(Role::Alert));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("locks-ask-yes", cx);
        });
        settle(&mut visual, "both stops", |_| stopped.lock().unwrap().len() == 2);
        // Only the two ordinary programs were asked to stop; one failing doesn't stop the other.
        let pids: Vec<u32> = stopped.lock().unwrap().iter().map(|s| s.0).collect();
        assert_eq!(pids, vec![10, 12]);
        settle(&mut visual, "the rescan", |cx| !has(cx, "End all (2)"));
    }

    #[gpui_kit::test]
    fn failure_and_empty_states_are_announced(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let broken = fake(vec![Err("Restart Manager is unavailable".into())], None);
        let scans = broken.scans.clone();
        open_fake(&mut visual, &["C:\\Docs"], broken.source);
        let label = "Couldn't find out what's using it. Restart Manager is unavailable";
        settle(&mut visual, "the error", |cx| has(cx, label));
        assert_eq!(role_of(&mut visual, label), Some(Role::Alert));
        // Refresh asks again.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("locks-refresh", cx);
        });
        settle(&mut visual, "a second scan", |_| scans.load(Ordering::SeqCst) == 2);
    }

    #[gpui_kit::test]
    fn an_empty_folder_says_there_is_nothing_to_check(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = fake(vec![Ok(report(vec![], 0))], None);
        open_fake(&mut visual, &["C:\\Empty"], fake.source);
        settle(&mut visual, "the empty state", |cx| {
            has(cx, "Nothing is using them. There are no files in it to check.")
        });
    }

    #[gpui_kit::test]
    fn reveal_closes_the_dialog_first_and_escape_closes_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = fake(vec![Ok(report(vec![proc(10, "WINWORD", "app")], 1))], None);
        let revealed = open_fake(&mut visual, &["C:\\Docs\\report.docx"], fake.source);
        settle(&mut visual, "rows", |cx| has(cx, "WINWORD (PID 10): Application"));
        visual.update(|window, cx| {
            assert!(window.focused(cx).is_some(), "focus starts inside the dialog layer");
            window.render_frame(cx);
            window.click(("locks-reveal", 10_usize), cx);
        });
        visual.run_until_parked();
        assert_eq!(*revealed.borrow(), vec!["C:\\Program Files\\WINWORD.exe".to_string()]);
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));

        // And Escape, on a fresh one.
        let fake = fake_again();
        open_fake(&mut visual, &["C:\\Docs"], fake);
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    fn fake_again() -> Source {
        fake(vec![Ok(report(vec![], 1))], None).source
    }

    #[gpui_kit::test]
    fn no_paths_open_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        open_fake(&mut visual, &[], fake_again());
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_without_trouble(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let fake = fake(vec![Ok(report(vec![proc(20, "محرر النصوص", "app")], 1))], None);
        open_fake(&mut visual, &["C:\\مستندات\\تقرير.docx"], fake.source);
        settle(&mut visual, "the Arabic row", |cx| {
            has(cx, "محرر النصوص (PID 20): Application")
        });
        assert_eq!(
            role_of(&mut visual, "محرر النصوص (PID 20): Application"),
            Some(Role::Group)
        );
        assert!(has(&mut visual, "What's locking “تقرير.docx”"));
        testing::set_rtl(false);
    }
}
