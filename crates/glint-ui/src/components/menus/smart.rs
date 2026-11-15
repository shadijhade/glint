//! The Suggested section of a menu, as data: every row's key, the rows put forward, and the menu dressed with
//! its head and its suggestions (docs/gpui/aware/PLAN.md, F2 and section 5.5).
//!
//! Card E2 of docs/gpui/aware fills this in: the signatures are the contract; until then no row has a key, and
//! a dressed menu is the menu as it was.

use super::spec::{Entry, Head, Node, Run};
use crate::kit::prelude::*;
use glint_model::aware::{Pick, Shape, Why};

impl Run {
    /// The stable key of what choosing this does ("do:extract-here"): what habits are kept under. None for a
    /// row that does nothing.
    pub fn key(&self) -> Option<String> {
        None
    }
}

/// The key of every row of `nodes` that can be chosen, in the menu's order, each once.
pub fn keys(nodes: &[Node]) -> Vec<String> {
    let _ = nodes;
    Vec::new()
}

/// The first row with `key`, as a copy made to stand alone (a submenu's row says which menu it is from).
pub fn row_for(nodes: &[Node], key: &str) -> Option<Entry> {
    let _ = (nodes, key);
    None
}

/// The words under a suggested row, when the reason isn't obvious.
pub fn reason(why: &Why, shape: Shape) -> Option<SharedString> {
    let _ = (why, shape);
    None
}

/// The Suggested section: its heading and at most three rows (`picks`' rows, then `extra`). Empty with no row.
pub fn zone(nodes: &[Node], picks: &[Pick], shape: Shape, extra: Vec<Node>) -> Vec<Node> {
    let _ = (nodes, picks, shape, extra);
    Vec::new()
}

/// The menu with its head first and Suggested after its strip. The menu's own rows are never moved or changed.
pub fn dress(nodes: Vec<Node>, head: Head, picks: &[Pick], shape: Shape, extra: Vec<Node>) -> Vec<Node> {
    let _ = (head, picks, shape, extra);
    nodes
}
