//! Freeze order: a folder's list stops re-sorting and reflowing while it changes (a build, a download, an unzip),
//! so files never jump out from under the pointer mid-click. Card 2.B.8 of the 0.16 brief.
//!
//! **How it works.** Freezing puts the tab's listing in the order its rows show and tells the rows not to sort
//! it again ([`Explorer::row_spec`] hands them the order the listing is in). A re-read of the folder then comes
//! through [`Freezes::hold`], which lays the fresh listing over the one on screen: a file still there keeps its
//! place and takes its new size and date, a file that went stays where it was and is drawn dimmed, and a file that
//! arrived goes at the end. Turning the freeze off lets the rows sort once and reads the folder again, which
//! drops what went. Leaving the folder, closing the tab, or sorting by a column header ends it too.
//!
//! **Cost.** Nothing while off: every hook checks that no tab is frozen before it looks at anything else.

use super::model::Explorer;
use super::types::*;
use glint_model::entries::FileEntry;
use glint_model::paths::THIS_PC;
use gpui_kit::Context;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// What a frozen tab remembers about its folder since it was frozen.
#[derive(Debug, Default)]
struct Freeze {
    /// Items that went since the freeze: their rows stay, dimmed.
    gone: HashSet<String>,
    /// Items that came since the freeze: their rows are at the end.
    arrived: HashSet<String>,
}

/// What the status bar says of a frozen tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrozenInfo {
    /// Items that came since the freeze ("3 new").
    pub arrived: usize,
    /// Items that went since the freeze, still shown dimmed.
    pub gone: usize,
}

/// The tabs whose order is frozen. Empty almost always.
#[derive(Debug, Default)]
pub(super) struct Freezes {
    tabs: HashMap<TabId, Freeze>,
    /// What the views draw dimmed while a tab is frozen: the items cut to the clipboard and the ones that went.
    dim: Option<Arc<HashSet<String>>>,
}

impl Freezes {
    /// No tab is frozen (the check every hook makes first).
    pub(super) fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// The tab's order is frozen.
    pub(super) fn holds(&self, id: TabId) -> bool {
        !self.tabs.is_empty() && self.tabs.contains_key(&id)
    }

    /// What is drawn dimmed besides the hidden items: the cut ones and the ones that went. `None` when no frozen
    /// tab has lost anything, so the clipboard's own set is the answer.
    pub(super) fn dim(&self) -> Option<&Arc<HashSet<String>>> {
        self.dim.as_ref()
    }

    /// Works out [`Freezes::dim`] again from the items cut to the clipboard and the ones that went.
    pub(super) fn refresh_dim(&mut self, cut: &Arc<HashSet<String>>) {
        let gone = self.tabs.values().map(|freeze| freeze.gone.len()).sum::<usize>();
        self.dim = (gone > 0).then(|| {
            let mut all: HashSet<String> = (**cut).clone();
            for freeze in self.tabs.values() {
                all.extend(freeze.gone.iter().cloned());
            }
            Arc::new(all)
        });
    }

    /// Lays a fresh listing of the folder over what the frozen tab shows (`held`, in the order on screen). Every
    /// item keeps its place, with the fresh size and date if it is still there; an item that went stays and is
    /// remembered as gone (it is back in its place if it comes back); new items go at the end.
    pub(super) fn hold(&mut self, id: TabId, held: &[FileEntry], mut fresh: Listed) -> Listed {
        let Some(freeze) = self.tabs.get_mut(&id) else {
            return fresh;
        };
        let latest = fresh.entries.clone();
        let place: HashMap<&str, usize> = latest
            .iter()
            .enumerate()
            .map(|(at, entry)| (entry.path.as_str(), at))
            .collect();
        let mut taken = vec![false; latest.len()];
        let mut shown: Vec<FileEntry> = Vec::with_capacity(held.len().max(latest.len()));
        for old in held {
            match place
                .get(old.path.as_str())
                .and_then(|&at| latest.get(at).map(|entry| (at, entry)))
            {
                Some((at, entry)) => {
                    shown.push(entry.clone());
                    if let Some(slot) = taken.get_mut(at) {
                        *slot = true;
                    }
                    freeze.gone.remove(&old.path);
                }
                None => {
                    shown.push(old.clone());
                    freeze.gone.insert(old.path.clone());
                    freeze.arrived.remove(&old.path);
                }
            }
        }
        for (entry, taken) in latest.iter().zip(&taken) {
            if !taken {
                shown.push(entry.clone());
                freeze.arrived.insert(entry.path.clone());
            }
        }
        fresh.entries = Arc::from(shown);
        fresh
    }

    /// Ends a tab's freeze. Whether it had one.
    pub(super) fn release(&mut self, id: TabId, cut: &Arc<HashSet<String>>) -> bool {
        let had = self.tabs.remove(&id).is_some();
        if had {
            self.refresh_dim(cut);
        }
        had
    }

    /// What the tab's freeze has gone through, if it is frozen.
    fn info(&self, id: TabId) -> Option<FrozenInfo> {
        let freeze = self.tabs.get(&id)?;
        Some(FrozenInfo {
            arrived: freeze
                .arrived
                .iter()
                .filter(|path| !freeze.gone.contains(*path))
                .count(),
            gone: freeze.gone.len(),
        })
    }
}

impl Explorer {
    /// The tab's order is frozen ([`Explorer::set_frozen`]).
    pub fn is_frozen(&self, tab: TabId) -> bool {
        self.freezes.holds(tab)
    }

    /// How many items came and went in a frozen tab since it was frozen; `None` when it isn't frozen.
    pub fn frozen_info(&self, tab: TabId) -> Option<FrozenInfo> {
        self.freezes.info(tab)
    }

    /// A tab's order can be frozen: it shows a folder on disk (not This PC, a search's results, an archive, or a
    /// place that is a list made of other folders' files).
    pub fn can_freeze(&self, tab: Option<TabId>) -> bool {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        self.tabs.get(&id).is_some_and(|tab| {
            tab.ready
                && !tab.loading
                && tab.listing.is_some()
                && tab.find.is_none()
                && tab.path != THIS_PC
                && tab.archive().is_none()
                && !tab.is_remote()
                && !tab.is_recycle_bin()
                && !glint_model::paths::is_recent(&tab.path)
                && !glint_model::paths::is_gallery(&tab.path)
        })
    }

    /// "Freeze order": turns the freeze of a tab (the active one's by default) on or off. Frozen, the list keeps
    /// every item where it is while the folder changes; off again, it sorts once.
    pub fn set_frozen(&mut self, tab: Option<TabId>, on: bool, cx: &mut Context<Self>) {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        if on {
            if self.freezes.holds(id) || !self.can_freeze(Some(id)) {
                return;
            }
            self.freeze_now(id, cx);
        } else if self.freezes.release(id, &self.ops.cut) {
            // The rows sort once (the listing still has what went, dimmed), and the folder is read for what is
            // really there.
            self.rebuild_rows(id, cx);
            self.reload(Some(id), cx).detach();
            cx.emit(ExplorerEvent::TabChanged(id));
            cx.notify();
        }
    }

    /// Freezes if it isn't, unfreezes if it is (the palette's command, the keyboard's).
    pub fn toggle_frozen(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        let on = !self.freezes.holds(id);
        self.set_frozen(Some(id), on, cx);
    }

    /// Puts the tab's listing in the order its rows show, so freezing changes nothing on screen, and keeps it so.
    fn freeze_now(&mut self, id: TabId, cx: &mut Context<Self>) {
        let ordered = self.tabs.get(&id).and_then(|tab| {
            let entries = tab.listing.as_ref()?.entries.clone();
            // Rows that are the listing's own entries in its own order need no new listing.
            let same = tab.rows.len() == entries.len()
                && tab
                    .rows
                    .iter()
                    .zip(entries.iter())
                    .all(|(row, entry)| std::ptr::eq(row, entry));
            if same {
                return None;
            }
            // The rows in their order, then what they leave out (hidden items, what the filter narrows away).
            let shown: HashSet<&str> = tab.rows.iter().map(|row| row.path.as_str()).collect();
            let mut all: Vec<FileEntry> = tab.rows.iter().cloned().collect();
            all.extend(
                entries
                    .iter()
                    .filter(|entry| !shown.contains(entry.path.as_str()))
                    .cloned(),
            );
            Some(all)
        });
        if let Some(all) = ordered {
            self.update_tab(id, |tab| {
                if let Some(listing) = tab.listing.as_mut() {
                    listing.entries = Arc::from(all);
                }
            });
        }
        self.freezes.tabs.insert(id, Freeze::default());
        self.rebuild_rows(id, cx);
        cx.emit(ExplorerEvent::TabChanged(id));
        cx.notify();
    }
}
