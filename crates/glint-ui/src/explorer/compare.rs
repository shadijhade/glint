//! Compare panes: in two panes, each pane's items say how they differ from the same name in the other pane
//! (Only here, Newer, Different, Older), and "diff only" hides the identical ones. Ported from the `comparison`
//! memo in Explorer.tsx, `toggleComparePanes` and `toggleDiffOnly` in src/lib/explorer.ts, and `compareEntries`
//! (glint-model's [`compare_entries`]).
//!
//! # How it works
//!
//! [`Comparison`] is the answer for one pane: the status of every item by lowercase name, the counts of each
//! status, and the paths that differ (`diffPaths`) and that a sync would copy (`syncPaths`: only here and newer).
//! [`compare_listings`] is the pure function that makes it from two listings. The explorer keeps one per pane
//! ([`Explorer::comparison_of`]) and refreshes them from its `changed` hook, so nothing has to ask: a folder
//! opened in either pane, a listing that changed on disk, Show hidden items, Folder sizes arriving, a pane
//! switching tabs, or the flags turned on all make the answer again. A refresh whose inputs are the same as the
//! last one's does nothing (the inputs are compared by identity: the listings' `Arc`s, the sizes' versions).
//!
//! Up to [`IN_BACKGROUND`] entries are compared on the spot (a hash lookup each: well under a millisecond a
//! thousand), so the first frame after a folder opens already has its marks; more than that are compared on a
//! background thread and the marks arrive when it is done. A newer refresh makes an older answer stale.
//!
//! **When there is no comparison** (as in the TypeScript): not in two panes; the flag off; the pane shows a
//! search, or something that is not a folder, or the other pane has no listing; both panes show the same folder.
//!
//! **Diff only** is a filter of the rows ([`RowDiffs`] in the [`RowSpec`](super::RowSpec)): the pane's rows leave
//! out the items that are the same, after the hidden items (so the hidden count is the same) and before the box's
//! text. When it leaves nothing, `Tab::rows` is empty and the views say "No differences in this pane".
//!
//! # Not ported
//!
//! The compare bar's *Subfolders* and *Contents* (`useDeepCompare`, `runDeepCompare`): core has no command for
//! them in this tree, so the folders are compared by name, kind, date and size only, as with both off.

use super::model::Explorer;
use super::types::TabId;
use glint_model::entries::{compare_entries, is_hidden, DiffStatus, FileEntry};
use glint_model::paths::{is_folder_path, same_path};
use gpui_kit::{AppContext as _, Context};
use std::collections::HashMap;
use std::sync::Arc;

/// Listings with more entries than this (both panes together) are compared on a background thread.
const IN_BACKGROUND: usize = 20_000;

/// The answer for one pane (`comparison` in Explorer.tsx).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Comparison {
    /// Every item of the pane (hidden ones only while they are shown), by lowercase name.
    pub diffs: HashMap<String, DiffStatus>,
    /// Items the other pane doesn't have (or has as another kind).
    pub unique: usize,
    pub newer: usize,
    pub differ: usize,
    pub older: usize,
    pub same: usize,
    /// The paths of every item that is not the same, in the pane's listing order.
    pub diff_paths: Vec<String>,
    /// The paths a sync into the other pane would copy: only here, and newer.
    pub sync_paths: Vec<String>,
}

impl Comparison {
    /// How `entry` differs from its namesake in the other pane; `None` for a name the comparison doesn't know
    /// (a hidden item that isn't shown).
    pub fn status(&self, entry: &FileEntry) -> Option<DiffStatus> {
        self.diffs.get(&entry.lower).copied()
    }

    /// How many items are not the same.
    pub fn differences(&self) -> usize {
        self.unique + self.newer + self.differ + self.older
    }
}

/// Compares the items of `own` with the namesakes in `other` (`comparison`): `own_sizes` and `other_sizes` are the
/// folders' sizes "Folder sizes" measured, by name, which decide whether two folders of the same name differ.
/// Hidden items are left out of both unless `show_hidden`.
pub fn compare_listings(
    own: &[FileEntry],
    other: &[FileEntry],
    show_hidden: bool,
    own_sizes: Option<&HashMap<String, u64>>,
    other_sizes: Option<&HashMap<String, u64>>,
) -> Comparison {
    let others: HashMap<&str, &FileEntry> = other
        .iter()
        .filter(|e| show_hidden || !is_hidden(e))
        .map(|e| (e.lower.as_str(), e))
        .collect();
    let mut out = Comparison::default();
    for entry in own {
        if !show_hidden && is_hidden(entry) {
            continue;
        }
        let namesake = others.get(entry.lower.as_str()).copied();
        let own_size = own_sizes
            .filter(|_| entry.dir)
            .and_then(|s| s.get(&entry.name))
            .copied();
        let other_size = namesake
            .filter(|o| o.dir)
            .and_then(|o| other_sizes.and_then(|s| s.get(&o.name)).copied());
        let status = compare_entries(entry, namesake, own_size, other_size);
        out.diffs.insert(entry.lower.clone(), status);
        match status {
            DiffStatus::Unique => {
                out.unique += 1;
                out.diff_paths.push(entry.path.clone());
                out.sync_paths.push(entry.path.clone());
            }
            DiffStatus::Newer => {
                out.newer += 1;
                out.diff_paths.push(entry.path.clone());
                out.sync_paths.push(entry.path.clone());
            }
            DiffStatus::Differ => {
                out.differ += 1;
                out.diff_paths.push(entry.path.clone());
            }
            DiffStatus::Older => {
                out.older += 1;
                out.diff_paths.push(entry.path.clone());
            }
            DiffStatus::Same => out.same += 1,
        }
    }
    out
}

/// The comparison a [`RowSpec`](super::RowSpec) filters by when "diff only" is on. Equal when it is the same
/// answer (`version`), so a spec compare never walks the map.
#[derive(Clone, Debug)]
pub struct RowDiffs {
    pub version: u64,
    pub comparison: Arc<Comparison>,
}

impl PartialEq for RowDiffs {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

/// One pane's answer, and the tab it is for.
#[derive(Clone, Debug)]
struct Compared {
    tab: TabId,
    version: u64,
    comparison: Arc<Comparison>,
}

/// What one pane puts in a comparison: the tab, its listing and sizes (`None` when it can't be compared).
#[derive(Clone)]
struct Side {
    tab: TabId,
    path: String,
    /// The listing's entries; `None` for a search, or no listing.
    entries: Option<Arc<[FileEntry]>>,
    sizes: Option<Arc<HashMap<String, u64>>>,
    sizes_version: u64,
    /// The listing, even under a search (the other pane's side of a comparison).
    listed: Option<Arc<[FileEntry]>>,
}

/// What a refresh was made from, to tell a second refresh with the same inputs from a real change.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    show_hidden: bool,
    panes: Vec<(TabId, String, usize, usize, u64, usize, usize)>,
}

/// The explorer's comparison state (a field of [`Explorer`]).
#[derive(Default)]
pub(super) struct CompareState {
    panes: [Option<Compared>; 2],
    key: Option<Key>,
    /// Numbers the refreshes, so a background answer to an older one is dropped.
    seq: u64,
    /// Numbers the answers ([`RowDiffs::version`]).
    version: u64,
}

impl Explorer {
    /// Ctrl+Shift+M: compare the two panes, or stop (`toggleComparePanes`). In one pane it opens the second pane
    /// (on the folder you're in) with the comparison on. Turning it on or off starts with every item shown.
    pub fn toggle_compare_panes(&mut self, cx: &mut Context<Self>) {
        if !self.dual {
            self.toggle_dual(cx);
            self.compare_panes = true;
        } else {
            self.compare_panes = !self.compare_panes;
        }
        self.diff_only = false;
        self.settle_comparison(cx);
    }

    /// "Show only the differences" in the compare bar, and "Show identical items" in the empty state
    /// (`toggleDiffOnly`).
    pub fn toggle_diff_only(&mut self, cx: &mut Context<Self>) {
        self.diff_only = !self.diff_only;
        self.settle_comparison(cx);
    }

    /// Previous versions' Compare for a folder: two panes, `now` on the left and `then` on the right, compared
    /// (`compareWithVersion`). The second navigation happens once the first folder is in.
    pub fn compare_with_version(&mut self, now: &str, then: &str, cx: &mut Context<Self>) -> gpui_kit::Task<()> {
        if !self.dual {
            self.toggle_dual(cx);
        }
        let panes: Vec<TabId> = self.panes.iter().map(|p| p.active).collect();
        let (now, then) = (now.to_string(), then.to_string());
        cx.spawn(async move |this, cx| {
            let Some(&left) = panes.first() else {
                return;
            };
            let first = this.update(cx, |this, cx| {
                let options = super::types::NavOptions {
                    tab: Some(left),
                    ..Default::default()
                };
                this.navigate_task(&now, options, cx)
            });
            if let Ok(first) = first {
                first.await;
            }
            if let Some(&right) = panes.get(1) {
                let second = this.update(cx, |this, cx| {
                    let options = super::types::NavOptions {
                        tab: Some(right),
                        ..Default::default()
                    };
                    this.navigate_task(&then, options, cx)
                });
                if let Ok(second) = second {
                    second.await;
                }
            }
            this.update(cx, |this, cx| {
                this.compare_panes = true;
                this.diff_only = false;
                this.active_pane = 1.min(this.panes.len().saturating_sub(1));
                this.settle_comparison(cx);
            })
            .ok();
        })
    }

    /// How the items of `tab` differ from the other pane's: the answer for the tab on screen in a pane while the
    /// panes are being compared; `None` otherwise (and when the answer isn't in yet).
    pub fn comparison_of(&self, tab: TabId) -> Option<Arc<Comparison>> {
        if !(self.dual && self.compare_panes) {
            return None;
        }
        let pane = self.pane_of(tab)?;
        let compared = self.compare.panes.get(pane)?.as_ref()?;
        (compared.tab == tab).then(|| compared.comparison.clone())
    }

    /// The comparison a tab's rows are filtered by: the answer, while "diff only" is on.
    pub(super) fn diff_filter(&self, tab: TabId) -> Option<RowDiffs> {
        if !self.diff_only {
            return None;
        }
        let pane = self.pane_of(tab)?;
        let compared = self.compare.panes.get(pane)?.as_ref().filter(|c| c.tab == tab)?;
        (self.dual && self.compare_panes).then(|| RowDiffs {
            version: compared.version,
            comparison: compared.comparison.clone(),
        })
    }

    /// After a flag changed: the comparison is made again, the rows of both panes follow, everyone is told.
    fn settle_comparison(&mut self, cx: &mut Context<Self>) {
        self.refresh_comparison(cx);
        for tab in self.visible_tabs() {
            self.rebuild_rows(tab, cx);
        }
        cx.notify();
    }

    /// Makes the comparison again if anything it is made from changed. Called by `changed`; cheap when nothing did.
    pub(super) fn refresh_comparison(&mut self, cx: &mut Context<Self>) {
        if !(self.dual && self.compare_panes) {
            self.clear_comparison(cx);
            return;
        }
        let show_hidden = super::model::Prefs::read(cx).show_hidden;
        let sides: Vec<Side> = self.panes.iter().take(2).map(|p| self.side_of(p.active)).collect();
        if sides.len() < 2 {
            self.clear_comparison(cx);
            return;
        }
        let key = Key {
            show_hidden,
            panes: sides
                .iter()
                .map(|s| {
                    let ptr = |e: &Option<Arc<[FileEntry]>>| e.as_ref().map_or(0, |e| e.as_ptr() as usize);
                    (
                        s.tab,
                        s.path.clone(),
                        ptr(&s.entries),
                        s.entries.as_ref().map_or(0, |e| e.len()),
                        s.sizes_version,
                        ptr(&s.listed),
                        s.listed.as_ref().map_or(0, |e| e.len()),
                    )
                })
                .collect(),
        };
        if self.compare.key.as_ref() == Some(&key) {
            return;
        }
        self.compare.key = Some(key);
        self.compare.seq += 1;
        let seq = self.compare.seq;

        // Each pane's job: its entries, the other pane's listing, and both folders' sizes.
        let mut jobs: Vec<Option<Job>> = Vec::new();
        for i in 0..2 {
            let (own, other) = (&sides[i], &sides[1 - i]);
            let same_folder = same_path(&own.path, &other.path);
            let job = match (&own.entries, &other.listed) {
                (Some(entries), Some(listed)) if is_folder_path(&own.path) && !same_folder => Some(Job {
                    tab: own.tab,
                    own: entries.clone(),
                    other: listed.clone(),
                    own_sizes: own.sizes.clone(),
                    other_sizes: other.sizes.clone(),
                }),
                _ => None,
            };
            jobs.push(job);
        }
        let total: usize = jobs.iter().flatten().map(|j| j.own.len() + j.other.len()).sum();
        if total <= IN_BACKGROUND {
            let answers = jobs
                .iter()
                .map(|j| j.as_ref().map(|j| (j.tab, j.run(show_hidden))))
                .collect();
            self.apply_comparison(answers, cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            let answers: Vec<Option<(TabId, Comparison)>> = cx
                .background_spawn(async move {
                    jobs.iter()
                        .map(|j| j.as_ref().map(|j| (j.tab, j.run(show_hidden))))
                        .collect()
                })
                .await;
            this.update(cx, |this, cx| {
                if this.compare.seq == seq {
                    this.apply_comparison(answers, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Everything about one pane that a comparison reads.
    fn side_of(&self, tab: TabId) -> Side {
        let t = self.tabs.get(&tab);
        let path = t.map(|t| t.path.clone()).unwrap_or_default();
        let searching = t.is_some_and(|t| t.find.is_some());
        let listed = t.and_then(|t| t.shared_entries());
        let run = self.size_run(&path).filter(|_| !searching);
        Side {
            tab,
            entries: listed.clone().filter(|_| !searching),
            sizes: run.map(|r| r.sizes.clone()),
            sizes_version: run.map_or(0, |r| r.version),
            listed,
            path,
        }
    }

    /// Puts the answers in, and rebuilds the rows that read them.
    fn apply_comparison(&mut self, answers: Vec<Option<(TabId, Comparison)>>, cx: &mut Context<Self>) {
        let mut touched: Vec<TabId> = Vec::new();
        for (i, answer) in answers.into_iter().enumerate().take(2) {
            if let Some(old) = &self.compare.panes[i] {
                touched.push(old.tab);
            }
            self.compare.panes[i] = answer.map(|(tab, comparison)| {
                self.compare.version += 1;
                touched.push(tab);
                Compared {
                    tab,
                    version: self.compare.version,
                    comparison: Arc::new(comparison),
                }
            });
        }
        if self.diff_only {
            touched.sort();
            touched.dedup();
            for tab in touched {
                self.rebuild_rows(tab, cx);
            }
        }
        cx.notify();
    }

    /// The flag went off, or there is one pane: no answers, and the rows come back whole.
    fn clear_comparison(&mut self, cx: &mut Context<Self>) {
        self.compare.key = None;
        self.compare.seq += 1;
        let had: Vec<TabId> = self
            .compare
            .panes
            .iter_mut()
            .filter_map(|p| p.take().map(|c| c.tab))
            .collect();
        if had.is_empty() {
            return;
        }
        for tab in had {
            self.rebuild_rows(tab, cx);
        }
        cx.notify();
    }
}

/// One pane's comparison, ready to run anywhere.
struct Job {
    tab: TabId,
    own: Arc<[FileEntry]>,
    other: Arc<[FileEntry]>,
    own_sizes: Option<Arc<HashMap<String, u64>>>,
    other_sizes: Option<Arc<HashMap<String, u64>>>,
}

impl Job {
    fn run(&self, show_hidden: bool) -> Comparison {
        compare_listings(
            &self.own,
            &self.other,
            show_hidden,
            self.own_sizes.as_deref(),
            self.other_sizes.as_deref(),
        )
    }
}
