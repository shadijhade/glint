//! A workspace's layout, taken from the explorer and put back: its panes, their tabs and folders, each tab's view
//! and where it was scrolled. Named workspaces are kept in the saved state (`glint_model::store::Workspace`); the
//! sidebar's Workspaces control and the palette do the asking (`components::sidebar::workspaces`).
//!
//! Switching is not a restart. A tab that already shows the folder a workspace wants keeps its listing, its
//! selection and its history and just moves to its place in the new layout, so a folder open in both layouts is
//! not read again; the others open as at start-up (each folder when its tab is first on screen), and tabs the new
//! layout has no place for close, as `close_tab` closes them.

use super::model::Explorer;
use super::tab::{ScrollMemo, Tab};
use super::types::*;
use glint_model::paths::{same_path, THIS_PC};
use glint_model::store::{Session, SessionPane, TabScroll, Workspace};
use gpui_kit::Context;
use std::collections::HashSet;

impl Explorer {
    /// The panes and tabs as they are now, and where each tab's list was scrolled (`scrolls[pane][tab]`, in the
    /// order of the session's panes and paths).
    pub fn workspace_layout(&self) -> (Session, Vec<Vec<Option<TabScroll>>>) {
        let session = Session {
            dual: self.dual,
            active_pane: self.active_pane,
            panes: self
                .panes
                .iter()
                .map(|pane| SessionPane {
                    paths: pane
                        .tabs
                        .iter()
                        .map(|id| self.tabs.get(id).map_or(THIS_PC, |t| t.path.as_str()).to_string())
                        .collect(),
                    active: pane.tabs.iter().position(|&t| t == pane.active).unwrap_or(0),
                    views: Some(
                        pane.tabs
                            .iter()
                            .map(|id| self.tabs.get(id).map_or(ViewMode::List, |t| t.view))
                            .collect(),
                    ),
                })
                .collect(),
        };
        let scrolls = self
            .panes
            .iter()
            .map(|pane| {
                pane.tabs
                    .iter()
                    .map(|id| {
                        self.tabs.get(id).and_then(|t| t.scroll.as_ref()).map(|memo| TabScroll {
                            key: memo.key.clone(),
                            top: memo.top,
                        })
                    })
                    .collect()
            })
            .collect();
        (session, scrolls)
    }

    /// Puts `workspace`'s layout on screen. Returns `false`, changing nothing, when it has no tabs at all.
    pub fn apply_workspace(&mut self, workspace: &Workspace, cx: &mut Context<Self>) -> bool {
        let wanted: Vec<(usize, &SessionPane)> = workspace
            .session
            .panes
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.paths.is_empty())
            .take(2)
            .collect();
        if wanted.is_empty() {
            return false;
        }
        let was_shown: HashSet<TabId> = self.visible_tabs().into_iter().collect();
        // The tabs open now, in the order of their panes: the first with a folder wanted is the one that stays.
        let order: Vec<TabId> = self.panes.iter().flat_map(|p| p.tabs.iter().copied()).collect();
        let mut old = std::mem::take(&mut self.tabs);
        let mut tabs = std::collections::HashMap::new();
        let mut panes = Vec::new();
        let mut changed_view = Vec::new();
        for (index, pane) in wanted {
            let mut ids = Vec::new();
            for (i, path) in pane.paths.iter().enumerate() {
                let view = match pane.views.as_ref().and_then(|v| v.get(i)) {
                    Some(&ViewMode::Grid) => ViewMode::Grid,
                    Some(&ViewMode::Columns) => ViewMode::Columns,
                    _ => ViewMode::List,
                };
                let scroll = workspace
                    .scrolls
                    .get(index)
                    .and_then(|tabs| tabs.get(i))
                    .and_then(|s| s.as_ref())
                    .map(|s| ScrollMemo {
                        key: s.key.clone(),
                        top: s.top,
                    });
                let kept = order
                    .iter()
                    .copied()
                    .find(|id| old.get(id).is_some_and(|t| reusable(t, path)));
                let id = match kept.and_then(|id| old.remove(&id)) {
                    Some(mut tab) => {
                        if tab.view != view {
                            tab.view = view;
                            changed_view.push(tab.id);
                        }
                        if scroll.is_some() {
                            tab.scroll = scroll;
                        }
                        let id = tab.id;
                        tabs.insert(id, tab);
                        id
                    }
                    None => {
                        let id = self.new_tab_id();
                        let mut tab = Tab::blank(id, path, view, None);
                        tab.scroll = scroll;
                        tabs.insert(id, tab);
                        id
                    }
                };
                ids.push(id);
            }
            let at = pane.active.min(ids.len().saturating_sub(1));
            if let Some(&active) = ids.get(at) {
                panes.push(Pane { tabs: ids, active });
            }
        }
        let dual = workspace.session.dual && panes.len() == 2;
        self.tabs = tabs;
        self.panes = panes;
        self.dual = dual;
        self.active_pane = if dual { workspace.session.active_pane.min(1) } else { 0 };
        self.quick_look = false;
        self.editing_address = false;

        // What the layout has no place for is closed: searches stop and folder sizes stop being measured.
        for (id, tab) in old {
            let find_running = tab.find.as_ref().is_some_and(|f| f.running);
            super::model::let_go(tab.shared_entries(), cx);
            self.stop_find_job(id, find_running);
            self.stop_sizes_of(id);
            cx.emit(ExplorerEvent::TabClosed { tab: id, find_running });
            self.refreshed_by_hand.remove(&id);
        }
        for id in changed_view {
            self.rebuild_rows(id, cx);
            cx.emit(ExplorerEvent::ViewChanged(id));
        }
        // Tabs coming on screen open their folder (the first time) or read it again: out of sight, nobody watched.
        for id in self.visible_tabs() {
            if !was_shown.contains(&id) {
                self.show_tab(id, cx);
            }
        }
        self.sync_watches();
        let path = self.current_path().to_string();
        self.remember_last_folder(&path, cx);
        cx.emit(ExplorerEvent::TabsChanged);
        self.changed(cx);
        true
    }
}

/// Whether `tab` can stand in for a tab of the new layout on `path`: it is on that folder already, with the
/// listing read, and is not showing the results of a search.
fn reusable(tab: &Tab, path: &str) -> bool {
    tab.ready && tab.find.is_none() && tab.listing.is_some() && same_path(&tab.path, path)
}
