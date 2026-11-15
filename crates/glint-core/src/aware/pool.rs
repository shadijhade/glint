//! The two lanes of threads the probes run on.
//!
//! Card B1 of docs/gpui/aware fills this in: the signatures are the contract; until then both lanes are rayon's
//! own pool.

use super::Lane;

/// Runs `job` on the lane's threads, without waiting for it.
pub fn spawn(lane: Lane, job: impl FnOnce() + Send + 'static) {
    let _ = lane;
    rayon::spawn(job);
}

/// Runs `work` inside the lane's pool and returns what it returns: rayon's `scope` and `par_iter` inside it use
/// the lane's threads.
pub fn install<R: Send>(lane: Lane, work: impl FnOnce() -> R + Send) -> R {
    let _ = lane;
    work()
}

/// How many threads the lane has.
pub fn threads(lane: Lane) -> usize {
    let _ = lane;
    2
}
