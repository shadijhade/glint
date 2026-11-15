//! "Move or copy to": the folders this kind of thing usually goes to, and choosing another
//! (docs/gpui/aware/PLAN.md, F5).
//!
//! Card G1 of docs/gpui/aware fills this in: the signatures are the contract; until then the submenu is empty
//! (so it isn't shown) and no place is the usual one.

use super::spec::Node;
use crate::kit::prelude::*;
use crate::t;
use glint_model::aware::Place;

/// The most places listed under each heading.
pub const MOST: usize = 5;

/// The "Move or copy to" submenu for `paths`, which are in `folder` now.
pub fn menu(paths: &[String], places: &[Place], folder: &str) -> Node {
    let _ = (paths, places, folder);
    Node::menu(t!("Move or copy to"), IconName::FolderOutput, Vec::new())
}

/// The row for Suggested when one place is where this kind of thing usually goes.
pub fn usual_row(
    paths: &[String],
    places: &[Place],
    ext: Option<&str>,
    folders_only: bool,
    folder: &str,
) -> Option<Node> {
    let _ = (paths, places, ext, folders_only, folder);
    None
}
