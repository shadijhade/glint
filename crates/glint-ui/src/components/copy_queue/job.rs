//! What a copy-queue job says about itself, in words: the pure half of the panel.
//! Ported from src/lib/queue.ts (`finished`, `jobTitle`, `jobFrom`) and the helpers at the top of
//! src/components/explorer/CopyQueue.tsx (`left`, `took`, `STATES`, the status line of `JobCard`).
//!
//! Everything here is a plain function of a [`JobView`], so the wording, the rounding and the taskbar's
//! choice of state are tested without a window. Text goes through `t!` with the TypeScript's English.

use crate::platform::window_state::ProgressState;
use crate::t;
use glint_core::queue::{Failure, JobView, Made};
use glint_model::format::{format_count, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use gpui_kit::SharedString;
use serde_json::Value;

/// `finished(job)`: done, cancelled or failed.
pub fn finished(job: &JobView) -> bool {
    matches!(job.state.as_str(), "done" | "cancelled" | "failed")
}

/// A count of files as the `usize` the formatters take (a count can't really pass `usize`).
pub fn count(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// JavaScript's `Math.round` for the non-negative numbers used here (halves round up).
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// `STATES[job.state]`: the state in words.
pub fn state_text(state: &str) -> SharedString {
    match state {
        "waiting" => t!("Waiting its turn"),
        "planning" => t!("Looking at what's to copy…"),
        "copying" => t!("Copying"),
        "checking" => t!("Checking the copy"),
        "paused" => t!("Paused"),
        "asking" => t!("Asks you"),
        "done" => t!("Done"),
        "cancelled" => t!("Cancelled"),
        "failed" => t!("Failed"),
        // A state core adds later shows as it is rather than as nothing.
        other => SharedString::from(other.to_string()),
    }
}

/// "2 min left", "8 s left"; empty when there is no speed yet or nothing is left (`left`).
pub fn left(job: &JobView) -> String {
    if job.speed <= 0.0 || job.speed.is_nan() || job.bytes_done >= job.bytes {
        return String::new();
    }
    let seconds = (job.bytes - job.bytes_done) as f64 / job.speed;
    if seconds < 60.0 {
        t!("{n} s left", n = js_round(seconds).max(1.0) as u64).to_string()
    } else if seconds < 3600.0 {
        t!("{n} min left", n = js_round(seconds / 60.0) as u64).to_string()
    } else {
        t!(
            "{h} h {m} min left",
            h = (seconds / 3600.0).floor() as u64,
            m = js_round((seconds % 3600.0) / 60.0) as u64
        )
        .to_string()
    }
}

/// How long a finished job took: "12 s", "3 min" (`took`); empty before it has started or finished.
pub fn took(job: &JobView) -> String {
    let (Some(started), Some(finished)) = (job.started.filter(|&s| s != 0), job.finished.filter(|&f| f != 0)) else {
        return String::new();
    };
    let seconds = (finished - started) as f64 / 1000.0;
    if seconds < 60.0 {
        t!("{n} s", n = js_round(seconds).max(1.0) as u64).to_string()
    } else {
        t!("{n} min", n = js_round(seconds / 60.0) as u64).to_string()
    }
}

/// The status line under a job's title: how far it is, or how it ended.
pub fn status_line(job: &JobView) -> String {
    let state = state_text(&job.state);
    if finished(job) {
        let mut line = t!(
            "{state}: {files} ({size})",
            state = state,
            files = plural(count(job.files_done), "file"),
            size = format_size(job.bytes_done)
        )
        .to_string();
        let took = took(job);
        if !took.is_empty() {
            line = format!("{line} {}", t!("in {took}", took = took));
        }
        if job.verify {
            // Compared as the TypeScript did, in signed numbers: more errors than files isn't "all checked".
            let verified = i64::try_from(job.verified).unwrap_or(i64::MAX);
            let files_done = i64::try_from(job.files_done).unwrap_or(i64::MAX);
            let errors = i64::try_from(job.errors.len()).unwrap_or(i64::MAX);
            let checked = if verified == files_done - errors || verified == files_done {
                t!("each one checked")
            } else {
                t!("{n} checked", n = format_count(count(job.verified)))
            };
            line = format!("{line} · {checked}");
        }
        return line;
    }
    let mut parts: Vec<String> = vec![state.to_string()];
    if job.files > 0 {
        parts.push(
            t!(
                "{done} of {total}",
                done = format_count(count(job.files_done)),
                total = plural(count(job.files), "file")
            )
            .to_string(),
        );
    }
    if job.bytes > 0 {
        parts.push(
            t!(
                "{done} of {total}",
                done = format_size(job.bytes_done),
                total = format_size(job.bytes)
            )
            .to_string(),
        );
    }
    if job.speed > 0.0 && job.state == "copying" {
        parts.push(t!("{speed}/s", speed = format_size(job.speed as u64)).to_string());
        // NOTE(port): the TypeScript left a dangling " · " when no time was known yet; it is left out here.
        let left = left(job);
        if !left.is_empty() {
            parts.push(left);
        }
    }
    parts.join(" · ")
}

/// A job in a few words: "Copying 3 items to “Backup”" (`jobTitle`).
pub fn job_title(job: &JobView) -> String {
    let what = match job.sources.as_slice() {
        [only] => format!("“{}”", base_name(only)),
        many => plural(many.len(), "item"),
    };
    let over = finished(job);
    let verb = match (over, job.kind == "move") {
        (true, true) => t!("Moved"),
        (true, false) => t!("Copied"),
        (false, true) => t!("Moving"),
        (false, false) => t!("Copying"),
    };
    let dest = match base_name(&job.dest) {
        "" => job.dest.as_str(),
        name => name,
    };
    t!("{verb} {what} to “{dest}”", verb = verb, what = what, dest = dest).to_string()
}

/// Where a job's items came from, for its tooltip (`jobFrom`): each folder once, in order.
pub fn job_from(job: &JobView) -> String {
    let mut seen: Vec<String> = Vec::new();
    for source in &job.sources {
        let parent = parent_path(source);
        if !seen.contains(&parent) {
            seen.push(parent);
        }
    }
    seen.join(", ")
}

/// How the taskbar button's bar should look for these jobs, as `initQueue` in explorer.ts worked it out:
/// no bar with nothing running, yellow when all running jobs are paused, green with a value when bytes are
/// known, else a marquee. The numbers are the running jobs' bytes done and total.
pub fn taskbar(jobs: &[JobView]) -> (ProgressState, u64, u64) {
    let running: Vec<&JobView> = jobs.iter().filter(|j| !finished(j)).collect();
    let total: u64 = running.iter().map(|j| j.bytes).sum();
    let done: u64 = running.iter().map(|j| j.bytes_done).sum();
    let state = if running.is_empty() {
        ProgressState::None
    } else if running.iter().all(|j| j.state == "paused") {
        ProgressState::Paused
    } else if total > 0 {
        ProgressState::Normal
    } else {
        ProgressState::Indeterminate
    };
    (state, done, total)
}

/// What changed about the taskbar bar: the same string means the same bar (the TypeScript's `key`, which
/// kept it from being set again for every event: it moves in steps of half a percent).
pub fn taskbar_key(state: ProgressState, done: u64, total: u64) -> String {
    let steps = (done as f64 / total.max(1) as f64 * 200.0).round() as u64;
    format!("{state:?} {steps}")
}

// ---- core's word of the jobs ---------------------------------------------------------------------------

/// The jobs in a "copy-queue" event's payload (an array of the camelCase objects `JobView` serializes to).
/// `None` for any other payload; a job without an id is dropped, a missing field takes its default.
pub fn jobs_from_json(payload: &Value) -> Option<Vec<JobView>> {
    Some(payload.as_array()?.iter().filter_map(job_from_json).collect())
}

fn job_from_json(value: &Value) -> Option<JobView> {
    let o = value.as_object()?;
    let text = |key: &str| o.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let number = |key: &str| o.get(key).and_then(Value::as_u64).unwrap_or(0);
    let flag = |key: &str| o.get(key).and_then(Value::as_bool).unwrap_or(false);
    let optional = |key: &str| o.get(key).and_then(Value::as_str).map(str::to_string);
    let stamp = |key: &str| o.get(key).and_then(Value::as_i64);
    let list = |key: &str| o.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
    let pair = |item: &Value, a: &str, b: &str| {
        let get = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        (get(a), get(b))
    };
    Some(JobView {
        id: o.get("id")?.as_u64()?,
        kind: text("kind"),
        sources: list("sources")
            .iter()
            .filter_map(|s| s.as_str().map(str::to_string))
            .collect(),
        dest: text("dest"),
        verify: flag("verify"),
        state: text("state"),
        files: number("files"),
        files_done: number("filesDone"),
        bytes: number("bytes"),
        bytes_done: number("bytesDone"),
        verified: number("verified"),
        current: optional("current"),
        speed: o.get("speed").and_then(Value::as_f64).unwrap_or(0.0),
        started: stamp("started"),
        finished: stamp("finished"),
        errors: list("errors")
            .iter()
            .map(|e| {
                let (path, error) = pair(e, "path", "error");
                Failure { path, error }
            })
            .collect(),
        conflict: optional("conflict"),
        made: list("made")
            .iter()
            .map(|m| {
                let (from, to) = pair(m, "from", "to");
                Made { from, to }
            })
            .collect(),
    })
}
