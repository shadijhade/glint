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
//! # Compared deeper
//!
//! With **Subfolders** or **Contents** on in the compare bar's options, the left pane asks glint-core to compare the
//! two folders all the way down ([`glint_core::compare::compare_folders`], `runDeepCompare` in explorer.ts), on core's
//! threads. Its answer ([`DeepAnswer`]) says how each top-level item compares (a folder is "different" when anything
//! inside differs) and what Sync copies each way, into the folder each goes in. The rows' statuses then come from it
//! (`deepStatuses?.[e.lower] ?? compareEntries(..)`), and Sync uses its copies ([`Explorer::sync_pane`]).
//!
//! - It is asked only while the panes are compared, the left pane shows a folder on disk and the right one a folder
//!   it can write to, and one of the two options is on. Whatever the left pane's listing, the right pane's listing,
//!   the options, Show hidden items or the exclusions change, it is asked again; a newer ask stops the older one in
//!   core (`cancel_compare`), and so does turning it off or leaving compare panes.
//! - **The last answer stays** while it is worked out again for the same folders and options, so the marks don't
//!   flicker; another pair of folders or other options clears it at once.
//! - An error is kept for the bar to say ("Couldn't compare deeper: ..."), but `"cancelled"` is not one.
//! - Core is reached through [`DeepCore`], which tests replace.

use super::model::{Explorer, Prefs};
use super::ops::{notify_failed, run_op_busy};
use super::ops_types::{Failing, FileOp, MoveItem, Working};
use super::source::Fut;
use super::tab::{PathSet, Selection};
use super::types::TabId;
use crate::core_bridge;
use glint_model::entries::{compare_entries, is_hidden, DiffStatus, FileEntry};
use glint_model::paths::{base_name, is_folder_path, same_path};
use gpui_kit::{AppContext as _, Context, Task};
use std::collections::HashMap;
use std::rc::Rc;
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
    compare_listings_deep(own, other, show_hidden, own_sizes, other_sizes, None)
}

/// [`compare_listings`] with the deeper comparison's statuses for this pane (`deepStatuses`, by lowercase name):
/// an item they know is as they say (`deepStatuses?.[e.lower] ?? compareEntries(..)`); any other is compared by name,
/// kind, date and size as usual.
pub fn compare_listings_deep(
    own: &[FileEntry],
    other: &[FileEntry],
    show_hidden: bool,
    own_sizes: Option<&HashMap<String, u64>>,
    other_sizes: Option<&HashMap<String, u64>>,
    deep: Option<&HashMap<String, DiffStatus>>,
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
        let status = deep
            .and_then(|d| d.get(&entry.lower).copied())
            .unwrap_or_else(|| compare_entries(entry, namesake, own_size, other_size));
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
    /// The tab has opened its first folder.
    ready: bool,
    /// A search's results are shown (`find`).
    searching: bool,
    /// The folder is inside an archive, which can't be written to.
    archive: bool,
    /// Windows keeps the folder closed to this account.
    closed: bool,
}

/// What a refresh was made from, to tell a second refresh with the same inputs from a real change.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    show_hidden: bool,
    panes: Vec<(TabId, String, usize, usize, u64, usize, usize)>,
    /// The deeper answer's number, and whether each pane reads it.
    deep: (u64, bool, bool),
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
    /// The deeper comparison (Subfolders, Contents).
    deep: DeepState,
    /// How the deeper comparison reaches core.
    core: DeepCore,
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
            self.stop_deep();
            self.clear_comparison(cx);
            return;
        }
        let prefs = Prefs::read(cx);
        let show_hidden = prefs.show_hidden;
        let sides: Vec<Side> = self.panes.iter().take(2).map(|p| self.side_of(p.active)).collect();
        if sides.len() < 2 {
            self.stop_deep();
            self.clear_comparison(cx);
            return;
        }
        // The deeper comparison first: its answer is part of what the comparison is made from.
        self.refresh_deep(&sides, &prefs, cx);
        let deep: Vec<Option<Arc<HashMap<String, DiffStatus>>>> =
            (0..2).map(|i| self.deep_statuses(&sides, i, &prefs)).collect();
        self.compare.deep.valid = [deep[0].is_some(), deep[1].is_some()];
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
            deep: (self.compare.deep.version, deep[0].is_some(), deep[1].is_some()),
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
                    deep: deep[i].clone(),
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
            ready: t.is_some_and(|t| t.ready),
            searching,
            archive: t.is_some_and(|t| t.archive().is_some()),
            closed: t.is_some_and(|t| t.is_closed()),
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
        self.compare.deep.valid = [false, false];
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
    /// The deeper comparison's statuses for this pane.
    deep: Option<Arc<HashMap<String, DiffStatus>>>,
}

impl Job {
    fn run(&self, show_hidden: bool) -> Comparison {
        compare_listings_deep(
            &self.own,
            &self.other,
            show_hidden,
            self.own_sizes.as_deref(),
            self.other_sizes.as_deref(),
            self.deep.as_deref(),
        )
    }
}

// ---- Compared deeper ------------------------------------------------------------------------------------------

/// What the deeper comparison is asked: the arguments of `compareFolders`.
#[derive(Clone, Debug, PartialEq)]
pub struct DeepRequest {
    /// The left pane's folder.
    pub left: String,
    /// The right pane's folder.
    pub right: String,
    /// Compare what is inside the subfolders too.
    pub subfolders: bool,
    /// Read the files' contents when their sizes and dates don't settle it.
    pub contents: bool,
    /// Hidden items count (Show hidden items).
    pub hidden: bool,
    /// Names the comparison skips (Settings, Find files, Exclusions).
    pub exclude: Vec<String>,
}

impl DeepRequest {
    /// The folders with the options that change the answer (`deepKey`): an answer is for that key only.
    pub fn key(&self) -> String {
        serde_json::json!([
            self.left.to_lowercase(),
            self.right.to_lowercase(),
            self.subfolders,
            self.contents,
            self.hidden,
            self.exclude
        ])
        .to_string()
    }
}

/// Core's answer to a deeper comparison (`FolderDiff`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeepAnswer {
    /// How each item at the top of the left folder compares, by lowercase name. An item not here (a folder, while
    /// Subfolders is off) is compared by name, kind, date and size as usual.
    pub left: Arc<HashMap<String, DiffStatus>>,
    /// The same for the right folder.
    pub right: Arc<HashMap<String, DiffStatus>>,
    /// What Sync copies to the right: what is missing there, and files newer here, each into the folder it goes in.
    pub to_right: Arc<[MoveItem]>,
    /// What Sync copies to the left.
    pub to_left: Arc<[MoveItem]>,
}

/// A status as core words it.
fn status_of(word: &str) -> Option<DiffStatus> {
    Some(match word {
        "unique" => DiffStatus::Unique,
        "newer" => DiffStatus::Newer,
        "older" => DiffStatus::Older,
        "differ" => DiffStatus::Differ,
        "same" => DiffStatus::Same,
        _ => return None,
    })
}

impl From<glint_core::compare::FolderDiff> for DeepAnswer {
    fn from(diff: glint_core::compare::FolderDiff) -> Self {
        let statuses = |words: HashMap<String, &'static str>| -> Arc<HashMap<String, DiffStatus>> {
            Arc::new(
                words
                    .into_iter()
                    .filter_map(|(name, word)| status_of(word).map(|status| (name, status)))
                    .collect(),
            )
        };
        // Each copy goes into its folder under the name it has (`name: baseName(i.path)` in syncInto).
        let copies = |list: Vec<glint_core::compare::Copy>| -> Arc<[MoveItem]> {
            list.into_iter()
                .map(|c| MoveItem {
                    name: base_name(&c.path).to_string(),
                    path: c.path,
                    folder: c.folder,
                })
                .collect()
        };
        DeepAnswer {
            left: statuses(diff.left),
            right: statuses(diff.right),
            to_right: copies(diff.to_right),
            to_left: copies(diff.to_left),
        }
    }
}

/// How the deeper comparison reaches glint-core: the real one in the app, a fake in tests.
#[derive(Clone)]
pub struct DeepCore {
    /// Compares the folders (`compareFolders`). `Err("cancelled")` when a newer ask or a stop overtook it.
    pub compare: Rc<dyn Fn(DeepRequest) -> Fut<Result<DeepAnswer, String>>>,
    /// Stops the comparison running, if any (`cancelCompare`).
    pub cancel: Rc<dyn Fn()>,
}

impl Default for DeepCore {
    fn default() -> Self {
        DeepCore {
            compare: Rc::new(core_compare),
            cancel: Rc::new(glint_core::compare::cancel_compare),
        }
    }
}

/// `compareFolders` on core's runtime, so the UI thread never waits for the disk.
fn core_compare(request: DeepRequest) -> Fut<Result<DeepAnswer, String>> {
    Box::pin(async move {
        let DeepRequest {
            left,
            right,
            subfolders,
            contents,
            hidden,
            exclude,
        } = request;
        core_bridge::run(glint_core::compare::compare_folders(
            left, right, subfolders, contents, hidden, exclude,
        ))
        .await
        .and_then(|answer| answer)
        .map(DeepAnswer::from)
    })
}

/// What an ask was made from, to tell a repeat from a change (the dependencies of the TypeScript's effect): the
/// request, and the two listings by identity.
#[derive(Clone, Debug, PartialEq)]
struct Asked {
    key: String,
    left: (usize, usize),
    right: (usize, usize),
}

/// The deeper comparison's state (`useDeepCompare`).
#[derive(Default)]
pub(super) struct DeepState {
    /// What was asked last (`DeepRequest::key`); the answer is for that and nothing else. Empty when nothing is.
    key: String,
    running: bool,
    result: Option<Arc<DeepAnswer>>,
    /// Why it failed; empty when it didn't, or was stopped.
    error: String,
    /// Numbers the asks (`deepSeq`): an answer to an older one is dropped.
    seq: u64,
    /// Numbers the changes of `result`, which the comparisons are made from.
    version: u64,
    asked: Option<Asked>,
    /// Waits for the answer; dropping it stops waiting.
    task: Option<Task<()>>,
    /// Whether each pane's comparison reads `result` now (it is for the folders and options the pane is in).
    valid: [bool; 2],
}

impl Explorer {
    /// Whether the deeper comparison is being worked out (the bar's spinner).
    pub fn deep_running(&self) -> bool {
        self.compare.deep.running
    }

    /// Why the deeper comparison failed, for the bar to say; empty if it didn't (a stop is no failure).
    pub fn deep_error(&self) -> &str {
        &self.compare.deep.error
    }

    /// What Sync in `pane` copies when the folders were compared deeper: the items, each into its folder (subfolders
    /// too). `None` when they weren't, or the answer is for other folders or options (`deepSync`).
    pub fn deep_sync_items(&self, pane: usize) -> Option<Arc<[MoveItem]>> {
        if !self.compare.deep.valid.get(pane).copied().unwrap_or(false) {
            return None;
        }
        let result = self.compare.deep.result.as_ref()?;
        Some(if pane == 0 {
            result.to_right.clone()
        } else {
            result.to_left.clone()
        })
    }

    /// Replaces how the deeper comparison reaches core (tests).
    #[cfg(test)]
    pub(super) fn set_deep_core(&mut self, core: DeepCore) {
        self.compare.core = core;
    }

    /// The compare bar's Select differences: every item of `pane` that isn't the same, selected, the first focused
    /// (`selectDiffs`).
    pub fn select_differences(&mut self, pane: usize, cx: &mut Context<Self>) {
        let tab = self.pane_tab_id(pane);
        let Some(comparison) = self.comparison_of(tab) else {
            return;
        };
        let Some(first) = comparison.diff_paths.first().cloned() else {
            return;
        };
        let paths: PathSet = comparison.diff_paths.iter().map(String::as_str).collect();
        let selection = Selection {
            paths: Arc::new(paths),
            anchor: Some(first.clone()),
            focus: Some(first),
        };
        self.apply_selection(tab, selection, cx);
    }

    /// The compare bar's Sync in `pane`: copies to the other pane what is only here and what is newer here. Deeper
    /// (Subfolders or Contents on) it is what core said, into the folders it goes in, subfolders too; otherwise the
    /// top level's unique and newer items go to the other pane's folder (`syncToOther`).
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn sync_pane(&mut self, pane: usize, cx: &mut Context<Self>) -> Task<()> {
        self.focus_pane(pane, cx);
        if let Some(items) = self.deep_sync_items(pane) {
            return self.sync_into(items.to_vec(), cx);
        }
        let paths = self
            .comparison_of(self.pane_tab_id(pane))
            .map(|c| c.sync_paths.clone())
            .unwrap_or_default();
        if paths.is_empty() {
            return Task::ready(());
        }
        self.copy_to_other_pane(false, Some(paths), cx)
    }

    /// Sync, deeper: each item copied into the folder it goes in through Windows' file engine, then both panes read
    /// again (`syncInto`). Says "Copying N items..." while it runs, "Synced N items" with Undo when it's done, and
    /// "Couldn't copy N items" if it fails.
    #[must_use = "dropping the task stops waiting for it; `.detach()` it to let it run"]
    pub fn sync_into(&mut self, items: Vec<MoveItem>, cx: &mut Context<Self>) -> Task<()> {
        if items.is_empty() {
            return Task::ready(());
        }
        let count = items.len();
        cx.spawn(async move |this, cx| {
            if let Err(message) = run_op_busy(&this, cx, FileOp::CopyInto { items }, Working::Copying(count)).await {
                notify_failed(&this, cx, Failing::CopyOrMove { moving: false, count }, message);
            }
            this.update(cx, |this, cx| {
                for id in this.visible_tabs() {
                    this.reload(Some(id), cx).detach();
                }
            })
            .ok();
        })
    }

    /// What the left pane asks of core, when it asks (`asks` in Explorer.tsx): the panes are compared (the caller
    /// knows), one of Subfolders and Contents is on, the left pane shows a folder on disk and the right one a folder
    /// it can copy into, and they aren't the same folder.
    fn deep_request(sides: &[Side], prefs: &Prefs) -> Option<DeepRequest> {
        if !(prefs.compare_subfolders || prefs.compare_contents) {
            return None;
        }
        let (own, other) = (sides.first()?, sides.get(1)?);
        let other_ready = other.ready && is_folder_path(&other.path) && !other.searching;
        let own_ok = !own.searching && is_folder_path(&own.path) && own.listed.is_some() && !own.archive;
        (other_ready && !other.archive && !other.closed && own_ok && !same_path(&own.path, &other.path)).then(|| {
            DeepRequest {
                left: own.path.clone(),
                right: other.path.clone(),
                subfolders: prefs.compare_subfolders,
                contents: prefs.compare_contents,
                hidden: prefs.show_hidden,
                exclude: prefs.find_exclusions.clone(),
            }
        })
    }

    /// The key (`deepKey`) of the folders pane `i` would read a deeper answer for: the left pane's folder and the
    /// right pane's, with today's options; none while the other pane isn't a folder it can compare with.
    fn pair_key(sides: &[Side], i: usize, prefs: &Prefs) -> Option<String> {
        let other = sides.get(1usize.saturating_sub(i))?;
        if !(other.ready && is_folder_path(&other.path) && !other.searching) {
            return None;
        }
        Some(
            DeepRequest {
                left: sides.first()?.path.clone(),
                right: sides.get(1)?.path.clone(),
                subfolders: prefs.compare_subfolders,
                contents: prefs.compare_contents,
                hidden: prefs.show_hidden,
                exclude: prefs.find_exclusions.clone(),
            }
            .key(),
        )
    }

    /// The deeper statuses pane `i` reads: the answer's, while it is for the folders and options the panes are in
    /// (`deepStatuses`).
    fn deep_statuses(&self, sides: &[Side], i: usize, prefs: &Prefs) -> Option<Arc<HashMap<String, DiffStatus>>> {
        let result = self.compare.deep.result.as_ref()?;
        let key = Self::pair_key(sides, i, prefs)?;
        (key == self.compare.deep.key).then(|| {
            if i == 0 {
                result.left.clone()
            } else {
                result.right.clone()
            }
        })
    }

    /// Asks core again if what the ask is made of changed, or stops what runs when it is no longer asked.
    fn refresh_deep(&mut self, sides: &[Side], prefs: &Prefs, cx: &mut Context<Self>) {
        match Self::deep_request(sides, prefs) {
            Some(request) => {
                let ptr = |side: &Side| {
                    side.listed
                        .as_ref()
                        .map_or((0, 0), |entries| (entries.as_ptr() as usize, entries.len()))
                };
                let asked = Asked {
                    key: request.key(),
                    left: sides.first().map_or((0, 0), ptr),
                    right: sides.get(1).map_or((0, 0), ptr),
                };
                if self.compare.deep.asked.as_ref() != Some(&asked) {
                    self.start_deep(request, asked, cx);
                }
            }
            None => {
                if !self.compare.deep.key.is_empty() || self.compare.deep.asked.is_some() {
                    self.stop_deep();
                }
            }
        }
    }

    /// Compares the folders as the bar says; a newer ask stops this one (`runDeepCompare`). The last answer stays
    /// while it is worked out again for the same folders and options.
    fn start_deep(&mut self, request: DeepRequest, asked: Asked, cx: &mut Context<Self>) {
        let key = request.key();
        if self.compare.deep.running {
            (self.compare.core.cancel)();
        }
        let deep = &mut self.compare.deep;
        deep.seq += 1;
        let id = deep.seq;
        if deep.key != key && deep.result.take().is_some() {
            deep.version += 1;
        }
        deep.key = key;
        deep.running = true;
        deep.error.clear();
        deep.asked = Some(asked);
        let running = (self.compare.core.compare)(request);
        self.compare.deep.task = Some(cx.spawn(async move |this, cx| {
            let answer = running.await;
            this.update(cx, |this, cx| this.deep_arrived(id, answer, cx)).ok();
        }));
    }

    /// The answer came. One to an older ask is dropped; an error is kept to be said, unless it is "cancelled".
    fn deep_arrived(&mut self, id: u64, answer: Result<DeepAnswer, String>, cx: &mut Context<Self>) {
        let deep = &mut self.compare.deep;
        if deep.seq != id {
            return;
        }
        deep.running = false;
        match answer {
            Ok(answer) => deep.result = Some(Arc::new(answer)),
            Err(message) => {
                deep.result = None;
                deep.error = if message == "cancelled" { String::new() } else { message };
            }
        }
        deep.version += 1;
        // The comparisons are made again from the new answer, and the bars show it.
        self.refresh_comparison(cx);
        cx.notify();
    }

    /// Stops asking, and stops what core is working on (`stopDeepCompare`): the answer, the error and the key go.
    fn stop_deep(&mut self) {
        let was_running = self.compare.deep.running;
        let deep = &mut self.compare.deep;
        deep.seq += 1;
        let had_answer = deep.result.is_some();
        deep.task = None;
        deep.asked = None;
        deep.running = false;
        deep.key.clear();
        deep.result = None;
        deep.error.clear();
        deep.valid = [false, false];
        if had_answer {
            deep.version += 1;
        }
        if was_running {
            (self.compare.core.cancel)();
        }
    }
}
