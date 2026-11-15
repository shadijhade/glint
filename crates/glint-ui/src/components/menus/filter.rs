//! Finding a menu's rows by typing (docs/gpui/aware/PLAN.md, F3).
//!
//! Card E3 of docs/gpui/aware fills this in: the signatures are the contract; until then nothing is found.

use super::spec::{Entry, Node};
use crate::kit::prelude::*;

/// A row found by typing.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The row, as it is in the menu (its label is its own: "Zip file").
    pub entry: Entry,
    /// The submenus it is under, outermost first, joined: "Compress to", or "A › B". None at the top.
    pub path: Option<SharedString>,
    pub score: f64,
}

/// The most rows a filter shows.
pub const MOST: usize = 12;

/// The rows of `nodes` that match `query`, best first, at most [`MOST`]. An empty or all-space query finds nothing.
pub fn matches(nodes: &[Node], query: &str) -> Vec<Found> {
    let _ = (nodes, query);
    Vec::new()
}
