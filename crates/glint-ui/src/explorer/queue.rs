//! The copy queue's side of the explorer. Ported from src/lib/queue.ts (`queueFiles`) and from explorer.ts
//! (`initQueue`'s `whenFinished`).
//!
//! Adding a job asks glint-core's queue (`queue_add`). Core tells how every job goes with its copy-queue event
//! (all jobs); this part watches it only for the moment a job finishes: Undo is offered for what it made, what
//! didn't go is said, and the folders it touched are read again. The queue's panel (progress,
//! Pause, Resume, Cancel, the conflict question) and the taskbar progress are the copy queue view's.

use super::model::{Explorer, Prefs};
use super::ops::notify_failed;
use super::ops_types::*;
use super::types::*;
use crate::components::copy_queue::finished;
use glint_core::queue::JobView;
use glint_model::format::plural;
use glint_model::paths::{base_name, parent_path, same_path};
#[cfg(test)]
use gpui_kit::Task;
use gpui_kit::{AsyncApp, Context, WeakEntity};

/// Adds a copy or move to the queue and says the panel should show (`queueFiles`).
pub(super) async fn queue_in(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    kind: QueueKind,
    sources: Vec<String>,
    dest: String,
) {
    let Ok((source, verify)) = this.update(cx, |this, cx| {
        (this.source.clone(), Prefs::one(cx, |s| s.verify_copies))
    }) else {
        return;
    };
    match source.queue_add(kind, sources, dest, verify).await {
        Ok(_) => {
            this.update(cx, |_, cx| cx.emit(ExplorerEvent::QueueOpened)).ok();
        }
        Err(message) => notify_failed(this, cx, Failing::Queue(kind), message),
    }
}

impl Explorer {
    /// Queues a copy or a move of `sources` into `dest`, and shows the queue (`queueFiles`). The queue runs it
    /// when the ones before it are done, with Pause and Resume, checking the copies afterwards where Settings say.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    #[cfg(test)]
    pub fn queue_files(
        &mut self,
        kind: QueueKind,
        sources: Vec<String>,
        dest: String,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| queue_in(&this, cx, kind, sources, dest).await)
    }

    /// Core's copy-queue event: every job as it is now. A job that went from running to finished is handled.
    pub(super) fn on_queue_event(&mut self, jobs: &[JobView], cx: &mut Context<Self>) {
        // The jobs as last told: only a job seen running before counts as newly finished (one first told as
        // finished, such as a list that was cleared and came back, isn't new).
        let before = std::mem::take(&mut self.ops.queue_seen);
        let mut newly_finished = Vec::new();
        for job in jobs {
            let done = finished(job);
            self.ops.queue_seen.insert(job.id, done);
            if done && before.get(&job.id) == Some(&false) {
                newly_finished.push(job.clone());
            }
        }
        for job in newly_finished {
            self.job_finished(job, cx);
        }
    }

    /// A queued copy or move finished: Undo offered as for Windows' own, what didn't go said, and the folders it
    /// touched read again.
    fn job_finished(&mut self, job: JobView, cx: &mut Context<Self>) {
        let moving = job.kind == "move";
        let count = job.made.len();
        if count > 0 {
            let what = plural(count, "item");
            if moving {
                let items = job
                    .made
                    .iter()
                    .map(|m| MoveItem {
                        path: m.to.clone(),
                        folder: parent_path(&m.from),
                        name: base_name(&m.from).to_string(),
                    })
                    .collect();
                let undo = self.remember_undo(format!("move of {what}"), UndoAction::MoveBack(items), cx);
                cx.emit(ExplorerEvent::Operation(OpNotice::Moved {
                    count,
                    dest: job.dest.clone(),
                    undo,
                }));
            } else {
                let made = job.made.iter().map(|m| m.to.clone()).collect();
                let undo = self.remember_undo(format!("copy of {what}"), UndoAction::Recycle(made), cx);
                cx.emit(ExplorerEvent::Operation(OpNotice::Copied {
                    count,
                    dest: job.dest.clone(),
                    undo: Some(undo),
                }));
            }
        }
        if !job.errors.is_empty() {
            let detail = job
                .errors
                .iter()
                .take(3)
                .map(|e| format!("{}: {}", base_name(&e.path), e.error))
                .collect::<Vec<_>>()
                .join("\n");
            cx.emit(ExplorerEvent::Operation(OpNotice::QueueErrors {
                kind: if moving { QueueKind::Move } else { QueueKind::Copy },
                count: job.errors.len(),
                detail,
            }));
        }
        let touched: Vec<TabId> = self
            .visible_tabs()
            .into_iter()
            .filter(|id| {
                self.tabs.get(id).is_some_and(|t| {
                    t.find.is_none()
                        && (same_path(&t.path, &job.dest)
                            || job.sources.iter().any(|s| same_path(&parent_path(s), &t.path)))
                })
            })
            .collect();
        for id in touched {
            self.reload(Some(id), cx).detach();
        }
    }
}
