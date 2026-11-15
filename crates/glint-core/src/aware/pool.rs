//! The two lanes of threads the probes run on.
//!
//! *Quiet* is for looking ahead, which nobody waits for: half the cores, each thread in Windows' efficiency mode
//! (EcoQoS: the efficiency cores of a hybrid CPU) and at background I/O priority, so it costs no heat and never
//! slows a listing. *Now* is for what something on screen waits for: every core, at normal priority.
//!
//! Each lane is a rayon pool made the first time it is used. Where one can't be made (a machine out of threads)
//! the lane is rayon's own global pool for good, and nothing else changes.

use super::Lane;
use std::sync::OnceLock;

static QUIET: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();
static NOW: OnceLock<Option<rayon::ThreadPool>> = OnceLock::new();

/// The machine's cores, 4 when it can't say.
fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get())
}

/// How many threads each lane gets.
fn wanted(lane: Lane) -> usize {
    match lane {
        Lane::Quiet => (cores() / 2).clamp(2, 8),
        Lane::Now => cores().clamp(2, 12),
    }
}

/// What a lane's thread does first: COM (shortcuts and the PDF reader need it), and for the quiet lane the
/// efficiency mode and background priority. Older Windows may refuse either: the answers are ignored.
#[cfg(windows)]
fn begin_thread(lane: Lane) {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadInformation, SetThreadPriority, ThreadPowerThrottling, THREAD_MODE_BACKGROUND_BEGIN,
        THREAD_POWER_THROTTLING_CURRENT_VERSION, THREAD_POWER_THROTTLING_EXECUTION_SPEED, THREAD_POWER_THROTTLING_STATE,
    };

    // SAFETY: plain COM initialisation of the calling thread, which is rayon's own.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    if lane == Lane::Quiet {
        // SAFETY: the handle is this thread's own pseudo-handle, and the struct outlives the call.
        unsafe {
            let thread = GetCurrentThread();
            let _ = SetThreadPriority(thread, THREAD_MODE_BACKGROUND_BEGIN);
            let state = THREAD_POWER_THROTTLING_STATE {
                Version: THREAD_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: THREAD_POWER_THROTTLING_EXECUTION_SPEED,
            };
            let _ = SetThreadInformation(
                thread,
                ThreadPowerThrottling,
                (&raw const state).cast(),
                size_of_val(&state) as u32,
            );
        }
    }
}

#[cfg(not(windows))]
fn begin_thread(_lane: Lane) {}

fn build(lane: Lane) -> Option<rayon::ThreadPool> {
    let name = match lane {
        Lane::Quiet => "quiet",
        Lane::Now => "now",
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(wanted(lane))
        .thread_name(move |i| format!("glint-aware-{name}-{i}"))
        .start_handler(move |_| begin_thread(lane))
        .build()
        .ok()
}

/// The lane's pool; None when it couldn't be made (rayon's global pool stands in).
fn pool(lane: Lane) -> Option<&'static rayon::ThreadPool> {
    let cell = match lane {
        Lane::Quiet => &QUIET,
        Lane::Now => &NOW,
    };
    cell.get_or_init(|| build(lane)).as_ref()
}

/// Runs `job` on the lane's threads, without waiting for it.
pub fn spawn(lane: Lane, job: impl FnOnce() + Send + 'static) {
    match pool(lane) {
        Some(pool) => pool.spawn(job),
        None => rayon::spawn(job),
    }
}

/// Runs `work` inside the lane's pool and returns what it returns: rayon's `scope` and `par_iter` inside it use
/// the lane's threads.
pub fn install<R: Send>(lane: Lane, work: impl FnOnce() -> R + Send) -> R {
    match pool(lane) {
        Some(pool) => pool.install(work),
        None => work(),
    }
}

/// How many threads the lane has.
pub fn threads(lane: Lane) -> usize {
    match pool(lane) {
        Some(pool) => pool.current_num_threads(),
        None => rayon::current_num_threads(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    fn wait_for(flag: &AtomicBool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if flag.load(Ordering::SeqCst) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn the_now_lane_has_at_least_as_many_threads_as_the_quiet_one() {
        assert!(threads(Lane::Quiet) >= 2);
        assert!(threads(Lane::Now) >= threads(Lane::Quiet));
    }

    #[test]
    fn a_job_spawned_on_each_lane_runs() {
        for lane in [Lane::Quiet, Lane::Now] {
            let done = Arc::new(AtomicBool::new(false));
            let flag = done.clone();
            spawn(lane, move || flag.store(true, Ordering::SeqCst));
            assert!(wait_for(&done), "{lane:?} never ran its job");
        }
    }

    #[test]
    fn install_returns_what_its_work_returns_and_runs_on_the_lane() {
        let answer = install(Lane::Now, || 6 * 7);
        assert_eq!(answer, 42);
        let name = install(Lane::Quiet, || std::thread::current().name().map(str::to_string));
        assert!(name.is_some_and(|n| n.starts_with("glint-aware-quiet-")));
    }
}
