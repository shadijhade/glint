//! Tabs and panes: adding, closing, moving, switching and the dual-pane view.
//! Ported from the "Tabs and panes" section of explorer.ts: `addTab`, `placeOutsideTabs`, `showOnStart`,
//! `showTab`, `activateTab`, `newTab`, `openInNewTab`, `duplicateTab`, `cycleTab`, `selectTab`, `forget`,
//! `remember`, `closeTab`, `closePane`, `closeOtherTabs`, `reopenClosedTab`, `placeTab`,
//! `moveTabToOtherPane`, `focusPane`, `switchPane`, `swapPanes`, `openInOtherPane`, `splitWithTab`,
//! `moveTabToPane`, `toggleDual`.
//!
//! A tab coming on screen opens its folder the first time and re-reads it after that (`show_tab`): folders
//! out of sight aren't watched, so they may have changed.

use super::model::{ClosedTab, Explorer};
use super::tab::Tab;
use super::types::*;
use glint_model::paths::{same_path, THIS_PC};
use gpui_kit::{Context, Task};
use std::time::Instant;

/// The most tabs Ctrl+Shift+T remembers.
const CLOSED_TABS: usize = 25;

/// The tab that takes the place of the one at `at` after it leaves `tabs` (the one now at `at`, or the one
/// before it at the end), like a browser.
fn neighbour(tabs: &[TabId], at: usize) -> Option<TabId> {
    tabs.get(at.min(tabs.len().saturating_sub(1))).copied()
}

impl Explorer {
    /// Adds a tab for `path` to `pane`, after `after` (default: the pane's active tab), shown as `view`
    /// (default: the active tab's), with `select` selected once it opens (`addTab`). Does not show it.
    pub(super) fn add_tab(
        &mut self,
        path: &str,
        pane: usize,
        after: Option<TabId>,
        view: Option<ViewMode>,
        select: Option<&str>,
    ) -> TabId {
        let view = view
            .or_else(|| self.active_tab().map(|t| t.view))
            .unwrap_or(ViewMode::List);
        let id = self.new_tab_id();
        let pane = if pane < self.panes.len() { pane } else { 0 };
        if let Some(p) = self.panes.get_mut(pane) {
            let reference = after.unwrap_or(p.active);
            // After the reference tab; at the end when it isn't in this pane.
            let at = p
                .tabs
                .iter()
                .position(|&t| t == reference)
                .map_or(p.tabs.len(), |i| i + 1);
            p.tabs.insert(at, id);
        }
        self.tabs.insert(id, Tab::blank(id, path, view, select));
        id
    }

    /// Adds a tab at the end of the active pane for each folder handed over from outside, and returns the
    /// last one's id. With `reuse` (at start), a restored tab on screen that already shows a folder stands in
    /// for it: an update restarts the app with the folders it was first started with (`placeOutsideTabs`).
    pub(super) fn place_outside_tabs(&mut self, targets: &[LaunchTarget], reuse: bool) -> Option<TabId> {
        let mut last = None;
        for target in targets {
            let shown = (reuse && target.select.is_none())
                .then(|| {
                    self.shown_panes()
                        .iter()
                        .flat_map(|p| p.tabs.iter())
                        .find(|&&id| {
                            let path = self.tabs.get(&id).map_or(THIS_PC, |t| t.path.as_str());
                            same_path(path, &target.path)
                        })
                        .copied()
                })
                .flatten();
            if let Some(shown) = shown {
                last = Some(shown);
                continue;
            }
            let pane = self.active_pane;
            let end = self.panes.get(pane).and_then(|p| p.tabs.last().copied());
            last = Some(self.add_tab(&target.path, pane, end, None, target.select.as_deref()));
        }
        last
    }

    /// At start, before any folder opens: makes `id` the tab on screen in its pane, and that pane the active
    /// one (`showOnStart`).
    pub(super) fn show_on_start(&mut self, id: Option<TabId>) {
        let Some(id) = id else {
            return;
        };
        let Some(pane) = self.pane_of(id) else {
            return;
        };
        if let Some(p) = self.panes.get_mut(pane) {
            p.active = id;
        }
        self.active_pane = pane;
    }

    /// A tab coming on screen: opens its folder the first time, else re-reads it (folders out of sight aren't
    /// watched) (`showTab`).
    pub(super) fn show_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(&id) else {
            return;
        };
        if !tab.ready {
            self.open_tab(id, cx).detach();
            return;
        }
        tab.nav_started = Some(Instant::now());
        if tab.find.is_none() {
            self.reload(Some(id), cx).detach();
        }
    }

    /// Shows tab `id` in its pane and makes that pane the active one (`activateTab`).
    pub fn activate_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane_of(id) else {
            return;
        };
        let was_shown = self.visible_tabs().contains(&id);
        let changed = self.active_pane != pane || self.panes.get(pane).is_some_and(|p| p.active != id);
        if let Some(p) = self.panes.get_mut(pane) {
            p.active = id;
        }
        self.active_pane = pane;
        self.dual = self.dual || pane > 0;
        if changed {
            self.quick_look = false;
            self.editing_address = false;
        }
        if !was_shown {
            self.show_tab(id, cx);
        }
        self.sync_watches();
        if changed {
            let path = self.current_path().to_string();
            self.remember_last_folder(&path, cx);
        }
        cx.emit(ExplorerEvent::TabsChanged);
        self.changed(cx);
    }

    /// Ctrl+T: a new tab on the open folder (or `path`), shown at once (`newTab`).
    pub fn new_tab(&mut self, path: Option<&str>, cx: &mut Context<Self>) {
        let path = path.map_or_else(|| self.current_path().to_string(), str::to_string);
        let id = self.add_tab(&path, self.active_pane, None, None, None);
        self.activate_tab(id, cx);
    }

    /// Middle-click on a folder: a new tab for it next to the current one, staying where you are
    /// (`openInNewTab`). Several in a row go after each other, as in a browser.
    pub fn open_in_new_tab(&mut self, path: &str, cx: &mut Context<Self>) {
        let pane = self.active_pane;
        let Some(from) = self.panes.get(pane).map(|p| p.active) else {
            return;
        };
        let after = match self.opened_from {
            Some((origin, last)) if origin == from && self.panes.get(pane).is_some_and(|p| p.tabs.contains(&last)) => {
                last
            }
            _ => from,
        };
        let added = self.add_tab(path, pane, Some(after), None, None);
        self.opened_from = Some((from, added));
        cx.emit(ExplorerEvent::TabsChanged);
        self.changed(cx);
    }

    /// A copy of tab `id` beside it, shown (`duplicateTab`).
    pub fn duplicate_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        let (Some(tab), Some(pane)) = (self.tabs.get(&id), self.pane_of(id)) else {
            return;
        };
        let (path, view) = (tab.path.clone(), tab.view);
        let added = self.add_tab(&path, pane, Some(id), Some(view), None);
        self.activate_tab(added, cx);
    }

    /// Ctrl+Tab and Ctrl+Shift+Tab: the next or previous tab in the active pane (`cycleTab`).
    pub fn cycle_tab(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(pane) = self.panes.get(self.active_pane) else {
            return;
        };
        let count = isize::try_from(pane.tabs.len()).unwrap_or(isize::MAX);
        let here = pane
            .tabs
            .iter()
            .position(|&t| t == pane.active)
            .and_then(|i| isize::try_from(i).ok())
            .unwrap_or(0);
        if count == 0 {
            return;
        }
        let to = (here + step).rem_euclid(count);
        if let Some(id) = usize::try_from(to).ok().and_then(|i| pane.tabs.get(i)).copied() {
            self.activate_tab(id, cx);
        }
    }

    /// Ctrl+1…8: that tab of the active pane; Ctrl+9: its last one (`selectTab`). `n` counts from 1.
    pub fn select_tab(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(pane) = self.panes.get(self.active_pane) else {
            return;
        };
        let id = if n >= 9 {
            pane.tabs.last()
        } else {
            n.checked_sub(1).and_then(|i| pane.tabs.get(i))
        };
        if let Some(id) = id.copied() {
            self.activate_tab(id, cx);
        }
    }

    /// Stops what a closed tab had running and drops its bookkeeping (`forget`): a running search ends here, and the
    /// folder sizes it was measuring stop too.
    fn forget(&mut self, id: TabId, tab: Option<&Tab>, cx: &mut Context<Self>) {
        let find_running = tab.is_some_and(|t| t.find.as_ref().is_some_and(|f| f.running));
        self.stop_find_job(id, find_running);
        self.stop_sizes_of(id);
        cx.emit(ExplorerEvent::TabClosed { tab: id, find_running });
        self.refreshed_by_hand.remove(&id);
    }

    /// Remembers a closed tab for Ctrl+Shift+T (`remember`).
    fn remember(&mut self, tab: Option<&Tab>, pane: usize, index: usize) {
        let Some(tab) = tab else {
            return;
        };
        self.closed_tabs.push(ClosedTab {
            path: tab.path.clone(),
            view: tab.view,
            pane,
            index,
        });
        if self.closed_tabs.len() > CLOSED_TABS {
            self.closed_tabs.remove(0);
        }
    }

    /// Closes a tab (the active one by default). A pane's only tab closes that pane in dual-pane view; the
    /// last tab of all stays (`closeTab`).
    pub fn close_tab(&mut self, id: Option<TabId>, cx: &mut Context<Self>) {
        let id = id.unwrap_or_else(|| self.active_tab_id());
        let Some(pane) = self.pane_of(id) else {
            return;
        };
        let Some(p) = self.panes.get(pane).cloned() else {
            return;
        };
        if p.tabs.len() == 1 {
            if self.dual {
                self.close_pane(pane, cx);
            }
            return;
        }
        let at = p.tabs.iter().position(|&t| t == id).unwrap_or(0);
        let tabs: Vec<TabId> = p.tabs.iter().copied().filter(|&t| t != id).collect();
        // Like a browser: the tab that takes its place, or the one before it at the end.
        let active = if p.active == id {
            neighbour(&tabs, at).unwrap_or(p.active)
        } else {
            p.active
        };
        let shown = p.active == id && pane < self.shown_panes().len();
        if let Some(slot) = self.panes.get_mut(pane) {
            *slot = Pane { tabs, active };
        }
        let closed = self.tabs.remove(&id);
        super::model::let_go(closed.as_ref().and_then(|tab| tab.shared_entries()), cx);
        self.remember(closed.as_ref(), pane, at);
        if shown {
            self.quick_look = false;
        }
        self.forget(id, closed.as_ref(), cx);
        if shown {
            self.show_tab(active, cx);
        }
        self.sync_watches();
        cx.emit(ExplorerEvent::TabsChanged);
        self.changed(cx);
    }

    /// Closes a pane in dual-pane view, with its tabs; the other one is left on its own (`closePane`).
    pub fn close_pane(&mut self, pane: usize, cx: &mut Context<Self>) {
        if !self.dual || self.panes.len() < 2 {
            return;
        }
        let (Some(gone), Some(kept)) = (self.panes.get(pane).cloned(), self.panes.get(1 - pane.min(1)).cloned()) else {
            return;
        };
        let mut closed = Vec::new();
        for (i, id) in gone.tabs.iter().enumerate() {
            let tab = self.tabs.remove(id);
            self.remember(tab.as_ref(), pane, i);
            closed.push((*id, tab));
        }
        self.panes = vec![kept];
        self.dual = false;
        self.active_pane = 0;
        self.quick_look = false;
        self.editing_address = false;
        for (id, tab) in closed {
            self.forget(id, tab.as_ref(), cx);
        }
        self.sync_watches();
        let path = self.current_path().to_string();
        self.remember_last_folder(&path, cx);
        cx.emit(ExplorerEvent::TabsChanged);
        self.changed(cx);
    }

    /// Closes every tab of `id`'s pane but `id` (`closeOtherTabs`).
    pub fn close_other_tabs(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(pane) = self.pane_of(id) else {
            return;
        };
        // Shown first, so closing the others never brings up one that's about to go.
        self.activate_tab(id, cx);
        let others: Vec<TabId> = self
            .panes
            .get(pane)
            .map(|p| p.tabs.iter().copied().filter(|&t| t != id).collect())
            .unwrap_or_default();
        for other in others {
            self.close_tab(Some(other), cx);
        }
    }

    /// Ctrl+Shift+T: brings back the last closed tab where it was (after the current tab if its pane is gone)
    /// (`reopenClosedTab`).
    pub fn reopen_closed_tab(&mut self, cx: &mut Context<Self>) {
        let Some(last) = self.closed_tabs.pop() else {
            return;
        };
        let there = last.pane < self.shown_panes().len();
        let pane = if there { last.pane } else { self.active_pane };
        let id = self.new_tab_id();
        if let Some(p) = self.panes.get_mut(pane) {
            let at = if there {
                last.index.min(p.tabs.len())
            } else {
                p.tabs.iter().position(|&t| t == p.active).map_or(0, |i| i + 1)
            };
            p.tabs.insert(at, id);
        } else {
            return;
        }
        self.tabs.insert(id, Tab::blank(id, &last.path, last.view, None));
        self.activate_tab(id, cx);
    }

    /// A tab dragged to `index` among pane `to`'s tabs (counted with it still where it was): along its own
    /// pane, or into the other one (not a pane's only tab) (`placeTab`).
    pub fn place_tab(&mut self, id: TabId, to: usize, index: usize, cx: &mut Context<Self>) {
        let Some(from) = self.pane_of(id) else {
            return;
        };
        let from_len = self.panes.get(from).map_or(0, |p| p.tabs.len());
        if to >= self.shown_panes().len() || (from != to && from_len < 2) {
            return;
        }
        let at = self
            .panes
            .get(from)
            .and_then(|p| p.tabs.iter().position(|&t| t == id))
            .unwrap_or(0);
        if let Some(source) = self.panes.get_mut(from) {
            source.tabs.remove(at);
        }
        let place = if from == to && at < index { index - 1 } else { index };
        if let Some(target) = self.panes.get_mut(to) {
            let place = place.min(target.tabs.len());
            target.tabs.insert(place, id);
        }
        let leaving = from != to && self.panes.get(from).is_some_and(|p| p.active == id);
        let mut next = None;
        if leaving {
            if let Some(source) = self.panes.get_mut(from) {
                if let Some(active) = neighbour(&source.tabs, at) {
                    source.active = active;
                    next = Some(active);
                }
            }
        }
        if let Some(next) = next {
            self.show_tab(next, cx);
        }
        if from != to {
            self.activate_tab(id, cx);
        } else {
            cx.emit(ExplorerEvent::TabsChanged);
            self.changed(cx);
        }
    }

    /// Dual-pane view: moves a tab to the other pane (not a pane's only tab) (`moveTabToOtherPane`).
    pub fn move_tab_to_other_pane(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(from) = self.pane_of(id) else {
            return;
        };
        let Some(source) = self.panes.get(from).cloned() else {
            return;
        };
        if !self.dual || source.tabs.len() < 2 {
            return;
        }
        let to = 1 - from.min(1);
        let Some(target) = self.panes.get(to).cloned() else {
            return;
        };
        let at = source.tabs.iter().position(|&t| t == id).unwrap_or(0);
        let left: Vec<TabId> = source.tabs.iter().copied().filter(|&t| t != id).collect();
        let source_active = if source.active == id {
            neighbour(&left, at).unwrap_or(source.active)
        } else {
            source.active
        };
        let mut tabs = target.tabs.clone();
        let after = tabs.iter().position(|&t| t == target.active).map_or(0, |i| i + 1);
        tabs.insert(after, id);
        if let Some(slot) = self.panes.get_mut(from) {
            *slot = Pane {
                tabs: left,
                active: source_active,
            };
        }
        if let Some(slot) = self.panes.get_mut(to) {
            *slot = Pane {
                tabs,
                active: target.active,
            };
        }
        if source.active == id {
            self.show_tab(source_active, cx);
        }
        self.activate_tab(id, cx);
    }

    /// Makes `pane` the one the keyboard and toolbar act on (a click in it does this) (`focusPane`).
    pub fn focus_pane(&mut self, pane: usize, cx: &mut Context<Self>) {
        if pane == self.active_pane || pane >= self.shown_panes().len() {
            return;
        }
        self.active_pane = pane;
        self.quick_look = false;
        self.editing_address = false;
        let path = self.current_path().to_string();
        self.remember_last_folder(&path, cx);
        cx.emit(ExplorerEvent::PaneFocused);
        self.changed(cx);
    }

    /// F6 in dual-pane view: the other pane (`switchPane`).
    pub fn switch_pane(&mut self, cx: &mut Context<Self>) {
        let other = if self.active_pane == 0 { 1 } else { 0 };
        self.focus_pane(other, cx);
    }

    /// Ctrl+Shift+U in dual-pane view: the panes trade places, tabs and all; the keyboard stays with its
    /// folder (`swapPanes`).
    pub fn swap_panes(&mut self, cx: &mut Context<Self>) {
        if !self.dual || self.panes.len() < 2 {
            return;
        }
        self.panes.swap(0, 1);
        self.active_pane = if self.active_pane == 0 { 1 } else { 0 };
        self.editing_address = false;
        cx.emit(ExplorerEvent::TabsChanged);
        cx.emit(ExplorerEvent::PaneFocused);
        self.changed(cx);
    }

    /// Folder `path` in the pane beside pane `from` (the active one), in the tab showing there (dual-pane view
    /// is turned on for it first); the keyboard stays where it was (`openInOtherPane`).
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn open_in_other_pane(&mut self, path: &str, from: Option<usize>, cx: &mut Context<Self>) -> Task<bool> {
        let was = if self.dual { self.active_pane } else { 0 };
        let here = if self.dual { from.unwrap_or(self.active_pane) } else { 0 };
        if !self.dual {
            self.toggle_dual(cx);
        }
        let tab = if self.dual {
            self.panes.get(1 - here.min(1)).map(|p| p.active)
        } else {
            None
        };
        let Some(tab) = tab else {
            return Task::ready(false);
        };
        self.focus_pane(was, cx);
        self.navigate_task(path, NavOptions::in_tab(tab), cx)
    }

    /// A tab dragged onto the far half of the only pane: two panes, the tab in the second. A pane's only tab
    /// stays where it is and the second pane opens on its folder (`splitWithTab`).
    pub fn split_with_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        if self.dual || self.pane_of(id) != Some(0) {
            return;
        }
        let (path, view) = (tab.path.clone(), tab.view);
        let Some(left) = self.panes.first().cloned() else {
            return;
        };
        let right = self.panes.get(1).cloned();
        if left.tabs.len() < 2 {
            let copy = self.new_tab_id();
            self.tabs.insert(copy, Tab::blank(copy, &path, view, None));
            let second = match right {
                Some(right) => Pane {
                    tabs: right.tabs.iter().copied().chain([copy]).collect(),
                    active: copy,
                },
                None => Pane {
                    tabs: vec![copy],
                    active: copy,
                },
            };
            self.panes = vec![left, second];
            self.dual = true;
            self.active_pane = 1;
            self.quick_look = false;
            self.editing_address = false;
            self.open_tab(copy, cx).detach();
        } else {
            let at = left.tabs.iter().position(|&t| t == id).unwrap_or(0);
            let rest: Vec<TabId> = left.tabs.iter().copied().filter(|&t| t != id).collect();
            let left_active = if left.active == id {
                neighbour(&rest, at).unwrap_or(left.active)
            } else {
                left.active
            };
            let second = match right {
                Some(right) => Pane {
                    tabs: right.tabs.iter().copied().chain([id]).collect(),
                    active: id,
                },
                None => Pane {
                    tabs: vec![id],
                    active: id,
                },
            };
            self.panes = vec![
                Pane {
                    tabs: rest,
                    active: left_active,
                },
                second,
            ];
            self.dual = true;
            if left.active == id {
                self.show_tab(left_active, cx);
            }
            self.activate_tab(id, cx);
        }
        self.sync_watches();
        let path = self.current_path().to_string();
        self.remember_last_folder(&path, cx);
        cx.emit(ExplorerEvent::TabsChanged);
        self.changed(cx);
    }

    /// A tab dragged onto the other pane's files: it moves there (a pane's only tab stays) (`moveTabToPane`).
    pub fn move_tab_to_pane(&mut self, id: TabId, to: usize, cx: &mut Context<Self>) {
        let Some(from) = self.pane_of(id) else {
            return;
        };
        if !self.dual || from == to {
            return;
        }
        let Some(target) = self.panes.get(to) else {
            return;
        };
        let index = target
            .tabs
            .iter()
            .position(|&t| t == target.active)
            .map_or(0, |i| i + 1);
        self.place_tab(id, to, index, cx);
    }

    /// Ctrl+Shift+D: two panes side by side, or back to one. The right pane keeps its tabs while hidden
    /// (`toggleDual`).
    pub fn toggle_dual(&mut self, cx: &mut Context<Self>) {
        if self.dual {
            self.dual = false;
            self.compare_panes = false;
            self.diff_only = false;
            self.active_pane = 0;
            self.quick_look = false;
            self.editing_address = false;
        } else if self.panes.len() < 2 {
            // A new right pane starts on the folder you're in.
            let id = self.new_tab_id();
            let path = self.current_path().to_string();
            let view = self.active_tab().map_or(ViewMode::List, |t| t.view);
            self.tabs.insert(id, Tab::blank(id, &path, view, None));
            self.panes.push(Pane {
                tabs: vec![id],
                active: id,
            });
            self.dual = true;
            self.active_pane = 1;
            self.quick_look = false;
            self.editing_address = false;
            self.open_tab(id, cx).detach();
        } else {
            self.dual = true;
            self.active_pane = 1;
            self.quick_look = false;
            self.editing_address = false;
            if let Some(active) = self.panes.get(1).map(|p| p.active) {
                self.show_tab(active, cx);
            }
        }
        self.sync_watches();
        let path = self.current_path().to_string();
        self.remember_last_folder(&path, cx);
        cx.emit(ExplorerEvent::TabsChanged);
        cx.emit(ExplorerEvent::PaneFocused);
        self.changed(cx);
    }
}
