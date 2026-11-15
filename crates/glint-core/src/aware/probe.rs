//! Asking about many paths at once.
//!
//! Card B1 of docs/gpui/aware fills this in: the signature is the contract; until then nothing is answered.

use super::{Answer, Ask, Lane};
use crate::host::ipc::Channel;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Asks about every path at once and returns at once: each answer is sent as it is known, in no order. Setting
/// `stop` ends what hasn't started and makes walks and hashes give up. A part that can't be known sends nothing.
pub fn probe(asks: Vec<Ask>, lane: Lane, stop: Arc<AtomicBool>, out: Channel<Answer>) {
    let _ = (asks, lane, stop, out);
}
