//! "Move or copy to": the folders this kind of thing usually goes to, and choosing another
//! (docs/gpui/aware/PLAN.md, F5).
//!
//! The submenu is data like every other: a heading and a row for each place, then "Choose a folder…", for moving,
//! and the same for copying. The usual place (one that is clearly where this kind of thing goes) is also offered
//! under Suggested, by [`usual_row`]; `MenuHost` hands it to `smart::dress`.

use super::event::MenuEvent;
use super::spec::{Entry, Node};
use crate::kit::prelude::*;
use crate::t;
use glint_model::aware::{usual_place, Place};
use glint_model::paths::{base_name, is_root, parent_path, same_path};

/// The most places listed under each heading.
pub const MOST: usize = 5;

/// The name a place goes by in a row: its last name, or the whole path for a drive's root.
fn name_of(folder: &str) -> String {
    if is_root(folder) {
        folder.to_string()
    } else {
        base_name(folder).to_string()
    }
}

/// The rows for one heading: a row per place (`icon`, moving or not) and "Choose a folder…".
fn rows(paths: &[String], places: &[Place], folder: &str, moving: bool, icon: IconName) -> Vec<Option<Node>> {
    let mut rows: Vec<Option<Node>> = places
        .iter()
        .filter(|place| !same_path(&place.folder, folder))
        .take(MOST)
        .map(|place| {
            let mut entry = Entry::ask(
                name_of(&place.folder).into(),
                icon,
                MenuEvent::SendTo {
                    paths: paths.to_vec(),
                    folder: place.folder.clone(),
                    moving,
                },
            );
            // Two folders of one name are told apart by where they are.
            if !is_root(&place.folder) {
                entry.hint = Some(parent_path(&place.folder).into());
            }
            Some(Node::Item(entry))
        })
        .collect();
    rows.push(
        Entry::ask(
            t!("Choose a folder…"),
            IconName::FolderSearch,
            MenuEvent::ChooseSendTo {
                paths: paths.to_vec(),
                moving,
            },
        )
        .node(),
    );
    rows
}

/// The "Move or copy to" submenu for `paths`, which are in `folder` now.
pub fn menu(paths: &[String], places: &[Place], folder: &str) -> Node {
    let mut items: Vec<Option<Node>> = vec![Some(Node::Heading(t!("Move to")))];
    items.extend(rows(paths, places, folder, true, IconName::FolderOutput));
    items.push(Some(Node::Heading(t!("Copy to"))));
    items.extend(rows(paths, places, folder, false, IconName::CopyPlus));
    Node::menu(t!("Move or copy to"), IconName::FolderOutput, items)
}

/// The row for Suggested when one place is where this kind of thing usually goes.
pub fn usual_row(
    paths: &[String],
    places: &[Place],
    ext: Option<&str>,
    folders_only: bool,
    folder: &str,
) -> Option<Node> {
    let place = usual_place(places)?;
    if same_path(&place.folder, folder) {
        return None;
    }
    let reason = match ext {
        Some(ext) => t!("Where you usually put {ext} files", ext = ext),
        None if folders_only => t!("Where you usually put folders"),
        None => t!("Where you usually put files like these"),
    };
    let mut entry = Entry::ask(
        t!("Move to “{name}”", name = name_of(&place.folder)),
        IconName::FolderOutput,
        MenuEvent::SendTo {
            paths: paths.to_vec(),
            folder: place.folder.clone(),
            moving: true,
        },
    );
    entry.smart = true;
    entry.reason = Some(reason);
    Some(Node::Item(entry))
}
