//! Selection in the active tab. Ported from explorer.ts: `selectOnly`, `toggleSelected`, `selectRange`,
//! `selectAll`, `invertSelection`.
//!
//! The `rows` arguments are the rows the list shows (`Tab::rows`, after sorting, filtering and grouping), so
//! a Shift-click selects what is between two clicks on screen. A view passes `tab.rows.clone()` (cheap).

use super::model::Explorer;
use super::rows::Rows;
use super::tab::{PathSet, Selection};
use super::types::*;
use gpui_kit::Context;
use std::sync::Arc;

impl Explorer {
    /// Replaces the active tab's selection with `selection`, if it differs, and tells everyone.
    fn set_selection(&mut self, id: TabId, selection: Selection, cx: &mut Context<Self>) {
        if !self.update_tab(id, |t| t.selection = selection) {
            return;
        }
        cx.emit(ExplorerEvent::SelectionChanged(id));
        cx.emit(ExplorerEvent::TabChanged(id));
        cx.notify();
    }

    /// Selects just this item (or nothing), and puts the focus and the anchor on it (`selectOnly`).
    pub fn select_only(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let selection = Selection {
            paths: Arc::new(path.into_iter().collect()),
            anchor: path.map(str::to_string),
            focus: path.map(str::to_string),
        };
        self.set_selection(id, selection, cx);
    }

    /// Ctrl-click: adds the item to the selection, or takes it out; the focus and the anchor go to it
    /// (`toggleSelected`).
    pub fn toggle_selected(&mut self, path: &str, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let mut paths = PathSet::clone(&tab.selection.paths);
        if !paths.remove(path) {
            paths.insert(path);
        }
        let selection = Selection {
            paths: Arc::new(paths),
            anchor: Some(path.to_string()),
            focus: Some(path.to_string()),
        };
        self.set_selection(id, selection, cx);
    }

    /// Shift-selection from the anchor to row `to`; with `add` (Ctrl+Shift) it extends the selection instead
    /// of replacing it (`selectRange`). Does nothing if there is no such row.
    pub fn select_range(&mut self, rows: &Rows, to: usize, add: bool, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let Some(target) = rows.get(to) else {
            return;
        };
        let at = tab.selection.anchor().and_then(|anchor| rows.position_of(anchor));
        let from = at.unwrap_or(to);
        let Some(origin) = rows.get(from) else {
            return;
        };
        let mut paths = if add {
            PathSet::clone(&tab.selection.paths)
        } else {
            PathSet::default()
        };
        for i in from.min(to)..=from.max(to) {
            if let Some(row) = rows.get(i) {
                paths.insert(&row.path);
            }
        }
        let selection = Selection {
            paths: Arc::new(paths),
            anchor: Some(origin.path.clone()),
            focus: Some(target.path.clone()),
        };
        self.set_selection(id, selection, cx);
    }

    /// Ctrl+A: every row (`selectAll`). The focus and anchor stay where they were.
    pub fn select_all(&mut self, rows: &Rows, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let selection = Selection {
            paths: Arc::new(rows.iter().map(|r| r.path.as_str()).collect()),
            anchor: tab.selection.anchor.clone(),
            focus: tab.selection.focus.clone(),
        };
        self.set_selection(id, selection, cx);
    }

    /// What isn't selected, and only that, of `rows` (File Explorer's Invert selection) (`invertSelection`).
    pub fn invert_selection(&mut self, rows: &Rows, cx: &mut Context<Self>) {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let selection = Selection {
            paths: Arc::new(
                rows.iter()
                    .filter(|r| !tab.selection.contains(&r.path))
                    .map(|r| r.path.as_str())
                    .collect(),
            ),
            anchor: tab.selection.anchor.clone(),
            focus: tab.selection.focus.clone(),
        };
        self.set_selection(id, selection, cx);
    }
}
