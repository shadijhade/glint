//! The copy queue: jobs, progress, pause, cancel, questions.
//! Ported from src/components/explorer/CopyQueue.tsx and src/lib/queue.ts (the store, `listenQueue`, the
//! taskbar half of `initQueue` in explorer.ts).
//!
//! | TypeScript                                   | Here                                                     |
//! |----------------------------------------------|----------------------------------------------------------|
//! | `useQueue` (`jobs`, `open`)                  | [`CopyQueue`]: [`CopyQueue::jobs`], [`CopyQueue::is_open`] |
//! | `listenQueue` ("copy-queue" event)           | [`core_bridge::subscribe`] in [`CopyQueue::new`]         |
//! | `whenFinished(handler)`                      | [`CopyQueueEvent::Finished`]                             |
//! | `queueFiles` (queue_add, then opens)         | [`CopyQueue::queue_files`]                               |
//! | `queuePause/Resume/Cancel/Answer/Clear`      | the panel's buttons, through [`Service`]                 |
//! | `CopyQueue` panel and its closed pill        | `impl Render for CopyQueue`                              |
//! | `taskbarProgress` in `initQueue`             | [`CopyQueue`] sets it when the running jobs change       |
//!
//! **Where it goes.** The TypeScript drew it `fixed end-4 bottom-10 z-40`: over the bottom end corner of the
//! window, above the status bar. The view is that overlay itself (an absolutely placed strip along the bottom,
//! with the panel or the pill at its end), so the workspace adds the entity as the *last child of its relative
//! root*. It draws nothing at all while there are no jobs, and only the pill while it is hidden and jobs run.
//!
//! **State.** The jobs are core's word, kept as it sent them; the person's choices that are only the view's
//! (panel open, "for the rest too" ticks, which error lists are open) live here. Core tells the jobs at most
//! every 150 ms; each event replaces the list, and GPUI draws once a frame however many events came.

mod job;
#[cfg(test)]
mod tests;

use crate::kit::prelude::*;
use crate::platform::window_state::taskbar_progress;
use crate::{core_bridge, t, theme};
use glint_core::queue::JobView;
use glint_model::format::format_count;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub use job::{finished, job_from, job_title, jobs_from_json, left, state_text, status_line, taskbar, took};

/// What the panel asks of the queue: core's `queue_*` functions, one for each button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// A copy or a move of `sources` into `dest` (`queue_add`).
    Add {
        kind: JobKind,
        sources: Vec<String>,
        dest: String,
        verify: bool,
    },
    Pause(u64),
    Resume(u64),
    Cancel(u64),
    /// "replace", "skip" or "both", for every name taken from now on too when `all`.
    Answer {
        id: u64,
        answer: &'static str,
        all: bool,
    },
    /// Takes the finished jobs off the list.
    Clear,
}

/// A copy or a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
}

impl JobKind {
    fn name(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Move => "move",
        }
    }
}

type Answer<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Where the queue is read and told what to do: glint-core in the app ([`CoreService`]), a fake in tests.
pub trait Service: Send + Sync + 'static {
    /// The jobs as they are now (`queue_jobs`).
    fn jobs(&self) -> Answer<Vec<JobView>>;
    /// Does `command`. The jobs' new state arrives as a "copy-queue" event, not as the answer.
    fn send(&self, command: Command) -> Answer<Result<(), String>>;
}

/// glint-core's queue.
pub struct CoreService {
    app: glint_core::host::AppHandle,
}

impl CoreService {
    /// The queue as the app has it (needs the app handle `queue_add` keeps to tell the jobs by).
    pub fn new(app: glint_core::host::AppHandle) -> Arc<Self> {
        Arc::new(Self { app })
    }
}

impl Service for CoreService {
    fn jobs(&self) -> Answer<Vec<JobView>> {
        Box::pin(async { glint_core::queue::queue_jobs() })
    }

    fn send(&self, command: Command) -> Answer<Result<(), String>> {
        use glint_core::queue as q;
        let app = self.app.clone();
        Box::pin(async move {
            match command {
                Command::Add {
                    kind,
                    sources,
                    dest,
                    verify,
                } => q::queue_add(app, kind.name().to_string(), sources, dest, verify)
                    .await
                    .map(|_| ()),
                Command::Pause(id) => q::queue_pause(id),
                Command::Resume(id) => q::queue_resume(id),
                Command::Cancel(id) => q::queue_cancel(id),
                Command::Answer { id, answer, all } => q::queue_answer(id, answer.to_string(), all),
                Command::Clear => {
                    q::queue_clear();
                    Ok(())
                }
            }
        })
    }
}

/// What the queue tells the rest of the app.
#[derive(Clone, Debug)]
pub enum CopyQueueEvent {
    /// A job that was running has ended (done, cancelled or failed): `whenFinished`'s handler in the
    /// TypeScript. The app offers Undo for `made`, words what didn't go, and reads the folders again.
    Finished(Box<JobView>),
    /// `queue_files` could not queue it ("Couldn't queue the copy"): the app shows `reason` in a notice.
    NotQueued { kind: JobKind, reason: String },
}

impl EventEmitter<CopyQueueEvent> for CopyQueue {}

/// The copy queue: the list of jobs core runs, and the panel that shows them.
pub struct CopyQueue {
    jobs: Vec<JobView>,
    open: bool,
    service: Arc<dyn Service>,
    /// The window whose taskbar button shows the progress; none in tests.
    taskbar: Option<AnyWindowHandle>,
    /// The last taskbar bar set, so an event that changes nothing visible sets nothing.
    taskbar_shown: String,
    /// Jobs whose "For the rest too" box is ticked.
    all: HashSet<u64>,
    /// Jobs whose list of what didn't go is open.
    errors_open: HashSet<u64>,
    _events: Option<Subscription>,
}

impl CopyQueue {
    /// The queue of the running app, in `window`: hears core's "copy-queue" events, reads the jobs once
    /// now, and shows the progress on `window`'s taskbar button.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let service: Arc<dyn Service> = match cx.try_global::<core_bridge::Core>() {
            Some(core) => CoreService::new(core.app.clone()),
            None => Arc::new(NoService),
        };
        Self::with_service(service, Some(window.window_handle()), cx)
    }

    /// [`CopyQueue::new`] for a given [`Service`] (tests pass a fake), and `taskbar` the window to show the
    /// progress on, if any.
    pub fn with_service(service: Arc<dyn Service>, taskbar: Option<AnyWindowHandle>, cx: &mut Context<Self>) -> Self {
        let events = core_bridge::subscribe(cx, |this, event, cx| {
            // The payload is the whole list. Anything else (an empty object) carries nothing to show.
            if event.name == "copy-queue" {
                if let Some(jobs) = jobs_from_json(&event.payload) {
                    this.apply(jobs, cx);
                }
            }
        });
        let read = service.jobs();
        cx.spawn(async move |this, cx| {
            if let Ok(jobs) = core_bridge::run(read).await {
                // An event that came while this was reading is newer: only an empty list takes the answer.
                let _ = this.update(cx, |this, cx| {
                    if this.jobs.is_empty() {
                        this.apply(jobs, cx);
                    }
                });
            }
        })
        .detach();
        Self {
            jobs: Vec::new(),
            open: false,
            service,
            taskbar,
            taskbar_shown: String::new(),
            all: HashSet::new(),
            errors_open: HashSet::new(),
            _events: events,
        }
    }

    /// The jobs as core last told them.
    pub fn jobs(&self) -> &[JobView] {
        &self.jobs
    }

    /// Whether a queued copy or move hasn't finished yet (`useQueue().jobs.some((job) => !finished(job))`).
    pub fn running(&self) -> bool {
        self.jobs.iter().any(|job| !job::finished(job))
    }

    /// Whether the panel is open (the pill shows while it is not and jobs run).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens the panel (`useQueue.setState({ open: true })`).
    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.set_open(true, cx);
    }

    /// Hides the panel; the jobs carry on.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        self.set_open(false, cx);
    }

    fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open != open {
            self.open = open;
            cx.notify();
        }
    }

    /// Takes core's list of jobs: notes the ones that just ended, moves the taskbar bar, redraws. Public so
    /// whoever queued a job can show it at once; core's events call it all the time.
    pub fn apply(&mut self, jobs: Vec<JobView>, cx: &mut Context<Self>) {
        let before: HashMap<u64, bool> = self.jobs.iter().map(|j| (j.id, finished(j))).collect();
        let ended: Vec<JobView> = jobs
            .iter()
            .filter(|j| finished(j) && before.get(&j.id) == Some(&false))
            .cloned()
            .collect();
        // Choices about a job that is gone are forgotten with it.
        self.all.retain(|id| jobs.iter().any(|j| j.id == *id));
        self.errors_open.retain(|id| jobs.iter().any(|j| j.id == *id));
        self.jobs = jobs;
        self.sync_taskbar(cx);
        cx.notify();
        for job in ended {
            cx.emit(CopyQueueEvent::Finished(Box::new(job)));
        }
    }

    /// Sets the taskbar button's bar when the running jobs' progress changed enough to see.
    fn sync_taskbar(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.taskbar else { return };
        let (state, done, total) = taskbar(&self.jobs);
        let key = job::taskbar_key(state, done, total);
        if key == self.taskbar_shown {
            return;
        }
        self.taskbar_shown = key;
        // A task, so it runs between updates, with the window free (this may be inside one).
        cx.spawn(async move |_, cx| {
            let _ = cx.update_window(handle, |_, window, _| {
                taskbar_progress(window, state, Some(done), Some(total));
            });
        })
        .detach();
    }

    /// Queues a copy or a move of `sources` into `dest` and shows the panel (`queueFiles`). If core refuses
    /// ("can't go into itself", not a folder) the answer is [`CopyQueueEvent::NotQueued`].
    #[cfg(test)]
    pub fn queue_files(
        &mut self,
        kind: JobKind,
        sources: Vec<String>,
        dest: String,
        verify: bool,
        cx: &mut Context<Self>,
    ) {
        let work = self.service.send(Command::Add {
            kind,
            sources,
            dest,
            verify,
        });
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.show(cx),
                Err(reason) => cx.emit(CopyQueueEvent::NotQueued { kind, reason }),
            });
        })
        .detach();
    }

    /// Sends a button's command. Core answers with an event, so nothing is awaited; a job that has gone from
    /// the queue meanwhile is not worth a word (the TypeScript ignored it too).
    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        let work = self.service.send(command);
        cx.spawn(async move |_, _| {
            let _ = core_bridge::run(work).await;
        })
        .detach();
    }
}

/// Stands in when there is no core (nothing in the app: the bridge is always up).
struct NoService;

impl Service for NoService {
    fn jobs(&self) -> Answer<Vec<JobView>> {
        Box::pin(async { Vec::new() })
    }

    fn send(&self, _: Command) -> Answer<Result<(), String>> {
        Box::pin(async { Err("The copy queue isn't available".to_string()) })
    }
}

// ---- Drawing -------------------------------------------------------------------------------------------

/// The ids of buttons that carry a job's number.
fn job_key(id: u64) -> usize {
    usize::try_from(id).unwrap_or(usize::MAX)
}

/// The last name of a path, whichever slash it uses (`path.split(/[\\/]/).pop()`).
fn last_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

impl CopyQueue {
    /// One job: title, status, its buttons, the bar, the file it's on, the question, the errors.
    fn job_card(&self, job: &JobView, last: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let id = job.id;
        let key = job_key(id);
        let over = finished(job);
        let share = if job.bytes > 0 {
            ((job.bytes_done as f64 / job.bytes as f64) * 100.0).min(100.0)
        } else if over {
            100.0
        } else {
            0.0
        };
        let title = SharedString::from(job_title(job));
        let status = SharedString::from(status_line(job));
        let from_to = t!("From {from} to {to}", from = job_from(job), to = job.dest);

        let head_icon = icon(if job.kind == "move" {
            IconName::FolderInput
        } else {
            IconName::Copy
        })
        .size_4()
        .mt(space(0.5))
        .flex_none()
        .text_color(t.primary);

        let words = col()
            .id(("copy-queue-words", key))
            .flex_1()
            .min_w_0()
            .align_start()
            .tooltip({
                let from_to = from_to.clone();
                move |window, cx| tooltip(from_to.clone(), window, cx)
            })
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child(isolate(title.clone())),
            )
            .child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(isolate(status.clone())),
            );

        let buttons = (!over).then(|| {
            let paused = job.state == "paused";
            let asking = job.state == "asking";
            let toggle = if paused {
                Button::new(("copy-queue-resume", key))
                    .ghost()
                    .small()
                    .icon(IconName::Play)
                    .accessibility_label(t!("Resume"))
                    .tooltip(t!("Resume"))
                    .on_click(cx.listener(move |this, _, _, cx| this.send(Command::Resume(id), cx)))
            } else {
                Button::new(("copy-queue-pause", key))
                    .ghost()
                    .small()
                    .icon(IconName::Pause)
                    .accessibility_label(t!("Pause"))
                    .tooltip(t!("Pause: a file half-copied waits where it is"))
                    .disabled(asking)
                    .on_click(cx.listener(move |this, _, _, cx| this.send(Command::Pause(id), cx)))
            };
            let cancel = Button::new(("copy-queue-cancel", key))
                .ghost()
                .small()
                .icon(IconName::X)
                .accessibility_label(t!("Cancel"))
                .tooltip(t!("Cancel: what's copied stays, a file half-copied goes"))
                .on_click(cx.listener(move |this, _, _, cx| this.send(Command::Cancel(id), cx)));
            row_start().flex_none().gap(space(0.5)).child(toggle).child(cancel)
        });

        let done_mark = (over && job.state == "done" && job.errors.is_empty()).then(|| {
            icon(IconName::CircleCheck)
                .size_4()
                .mt(space(0.5))
                .flex_none()
                .text_color(t.success)
        });

        let head = align_row(2.)
            .items_start()
            .child(head_icon)
            .child(words)
            .children(buttons)
            .children(done_mark);

        let bar = (!over).then(|| {
            let fill = if job.state == "paused" { t.warning } else { t.primary };
            row_start()
                .id(("copy-queue-progress", key))
                .role(Role::ProgressIndicator)
                .aria_label(t!("{title}: {n}%", title = title.clone(), n = share.round() as u64))
                .test_support()
                .h(space(1.5))
                .w_full()
                .overflow_hidden()
                .rounded_full()
                .bg(t.muted)
                .child(
                    div()
                        .h_full()
                        .w(relative((share / 100.0) as f32))
                        .rounded_full()
                        .bg(fill),
                )
        });

        let current = job.current.clone().filter(|c| !over && !c.is_empty()).map(|current| {
            div()
                .id(("copy-queue-current", key))
                .max_w_full()
                .truncate()
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .tooltip({
                    let current = SharedString::from(current.clone());
                    move |window, cx| tooltip(current.clone(), window, cx)
                })
                .child(isolate(current))
        });

        let question = (job.state == "asking")
            .then(|| job.conflict.clone())
            .flatten()
            .map(|conflict| self.question(id, &conflict, &t, cx));

        let errors = (!job.errors.is_empty()).then(|| self.errors(job, &t, cx));

        col()
            .id(("copy-queue-job", key))
            .role(Role::ListItem)
            .aria_label(SharedString::from(format!("{title}. {status}")))
            .test_support()
            .gap(space(1.5))
            .px(space(3.))
            .py(space(2.5))
            .when(!last, |this| this.border_b_1().border_color(t.border))
            .child(head)
            .children(bar)
            .children(current)
            .children(question)
            .children(errors)
    }

    /// The question of a name that is taken: Replace it, Skip it, Keep both, and "for the rest too".
    fn question(&self, id: u64, conflict: &str, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let key = job_key(id);
        let all = self.all.contains(&id);
        let name = last_name(conflict).to_string();
        col()
            .id(("copy-queue-question", key))
            .role(Role::Group)
            .aria_label(t!("“{name}” is there already.", name = name))
            .test_support()
            .gap(space(1.5))
            .p(space(2.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.warning.opacity(0.4))
            .bg(t.warning.opacity(0.1))
            .text_size(theme::size::XS)
            .child(
                row_start()
                    .flex_wrap()
                    .gap_x(space(1.))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child(isolate(format!("“{name}”"))),
                    )
                    .child(t!("is there already.")),
            )
            .child(
                row_start()
                    .flex_wrap()
                    .gap(space(1.5))
                    .children(
                        [
                            ("copy-queue-replace", "replace", t!("Replace it")),
                            ("copy-queue-skip", "skip", t!("Skip it")),
                            ("copy-queue-both", "both", t!("Keep both")),
                        ]
                        .map(|(button, answer, label)| {
                            let button = Button::new((button, key)).small().label(label).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    let all = this.all.contains(&id);
                                    this.send(Command::Answer { id, answer, all }, cx);
                                },
                            ));
                            // Replace is the one the question leans to, as the TypeScript's filled button.
                            if answer == "replace" {
                                button.primary()
                            } else {
                                button.outline()
                            }
                        }),
                    )
                    .child(
                        Checkbox::new(("copy-queue-all", key))
                            .label(t!("For the rest too"))
                            .checked(all)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if *checked {
                                    this.all.insert(id);
                                } else {
                                    this.all.remove(&id);
                                }
                                cx.notify();
                            })),
                    ),
            )
    }

    /// "3 didn't go" and, opened, what each one's trouble was.
    fn errors(&self, job: &JobView, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let id = job.id;
        let key = job_key(id);
        let open = self.errors_open.contains(&id);
        let label = if job.errors.len() == 1 {
            t!("1 didn't go")
        } else {
            t!("{n} didn't go", n = format_count(job.errors.len()))
        };
        let toggle = align_row(1.)
            .child(
                Button::new(("copy-queue-errors", key))
                    .ghost()
                    .xsmall()
                    .icon(IconName::TriangleAlert)
                    .label(label.clone())
                    .text_color(t.destructive)
                    .selected(open)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.errors_open.remove(&id) {
                            this.errors_open.insert(id);
                        }
                        cx.notify();
                    })),
            )
            .child(
                icon(if open {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .size_3p5()
                .text_color(t.destructive),
            );
        let list = open.then(|| {
            col()
                .id(("copy-queue-error-list", key))
                .role(Role::List)
                .aria_label(label)
                .max_h(space(32.))
                .overflow_y_scroll()
                .gap(space(0.5))
                .text_size(theme::size::XS)
                .children(job.errors.iter().enumerate().map(|(i, e)| {
                    let name = last_name(&e.path).to_string();
                    let detail = SharedString::from(format!("{}\n{}", e.path, e.error));
                    row_start()
                        .id(("copy-queue-error", key * 4096 + i))
                        .role(Role::ListItem)
                        .aria_label(format!("{name}: {}", e.error))
                        .test_support()
                        .gap(space(1.))
                        .tooltip(move |window, cx| tooltip(detail.clone(), window, cx))
                        .child(div().flex_none().text_color(t.foreground).child(isolate(name)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.muted_foreground)
                                .child(SharedString::from(format!(": {}", e.error))),
                        )
                }))
        });
        col()
            .gap(space(1.))
            .text_size(theme::size::XS)
            .child(toggle)
            .children(list)
    }
}

impl Render for CopyQueue {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.jobs.is_empty() {
            return div().into_any_element();
        }
        let t = theme::tokens(cx).clone();
        let running: Vec<&JobView> = self.jobs.iter().filter(|j| !finished(j)).collect();
        let asking = running.iter().any(|j| j.state == "asking");

        // Over the bottom end corner of the window, above the status bar (`fixed end-4 bottom-10`).
        let frame = || {
            row_start()
                .absolute()
                .left_0()
                .right_0()
                .bottom(space(10.))
                .px(space(4.))
                .justify_end()
                .items_end()
        };

        if !self.open {
            if running.is_empty() {
                return div().into_any_element();
            }
            let bytes: u64 = running.iter().map(|j| j.bytes).sum();
            let done: u64 = running.iter().map(|j| j.bytes_done).sum();
            let label = if asking {
                t!("The copy queue asks you")
            } else {
                let percent = if bytes > 0 {
                    (done as f64 / bytes as f64 * 100.0).round() as u64
                } else {
                    0
                };
                t!("Copy queue · {n}%", n = percent)
            };
            return frame()
                .child(
                    Button::new("copy-queue-pill")
                        .outline()
                        .small()
                        .rounded_full()
                        .icon(IconName::Copy)
                        .label(label.clone())
                        .accessibility_label(label)
                        .when(asking, |b| b.border_color(t.warning))
                        .on_click(cx.listener(|this, _, _, cx| this.show(cx))),
                )
                .into_any_element();
        }

        let viewport = window.viewport_size();
        let margin = window.rem_size() * 2.;
        let any_finished = self.jobs.iter().any(finished);
        let last = self.jobs.len().saturating_sub(1);

        let header = align_row(2.)
            .flex_none()
            .px(space(3.))
            .py(space(2.))
            .border_b_1()
            .border_color(t.border)
            .child(icon(IconName::ShieldCheck).size_4().text_color(t.primary))
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(theme::size::SM)
                    .child(t!("Copy queue")),
            )
            .when(any_finished, |this| {
                this.child(
                    Button::new("copy-queue-clear")
                        .ghost()
                        .small()
                        .label(t!("Clear finished"))
                        .on_click(cx.listener(|this, _, _, cx| this.send(Command::Clear, cx))),
                )
            })
            .child(
                Button::new("copy-queue-hide")
                    .ghost()
                    .small()
                    .icon(IconName::X)
                    .accessibility_label(t!("Hide the copy queue"))
                    .tooltip(t!("Hide (it carries on)"))
                    .on_click(cx.listener(|this, _, _, cx| this.hide(cx))),
            );

        let cards: Vec<AnyElement> = self
            .jobs
            .iter()
            .enumerate()
            .map(|(i, job)| self.job_card(job, i == last, cx).into_any_element())
            .collect();

        let list = col()
            .id("copy-queue-jobs")
            .role(Role::List)
            .aria_label(t!("Jobs"))
            .test_support()
            .flex_1()
            .min_h_0()
            .children(cards)
            .overflow_y_scrollbar();

        frame()
            .child(
                col()
                    .id("copy-queue")
                    .role(Role::Group)
                    .aria_label(t!("Copy queue"))
                    .test_support()
                    // The panel is over the window's content: clicks on it go no further.
                    .occlude()
                    .w(rems(27.))
                    .max_w(viewport.width - margin)
                    .max_h(viewport.height * 0.6)
                    .overflow_hidden()
                    .rounded(theme::size::RADIUS_XL)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.popover)
                    .text_color(t.popover_foreground)
                    .text_size(theme::size::SM)
                    .text_start()
                    .shadow_lg()
                    .child(header)
                    .child(list),
            )
            .into_any_element()
    }
}
