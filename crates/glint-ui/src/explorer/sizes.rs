//! Folder sizes: how big each subfolder of the open folder is. Ported from explorer.ts: `calcSizes`, `autoSizes`,
//! `sizeFolder`, `stopSizes`, `archiveSizes`, and the `sizes` store (`SizeRun`) the list, grid, status bar,
//! sorting and grouping read.
//!
//! **What happens.**
//! - "Folder sizes" (menu, command bar) measures the open folder's subfolders: from the instant search index
//!   when it has the folder (a millisecond or two), else with the bundled pdu. The run shows as running (the
//!   command bar says Stop, folders have "…") until it ends; Stop ends it with nothing said.
//! - With the index ready for a folder's drive, every folder shows its subfolders' sizes as it opens and after
//!   each refresh, quietly: from the index alone, and nothing happens when it can't answer.
//! - A folder in an archive has every size in its listing, so its run is made from the listing itself.
//! - A closed tab stops what it was measuring.
//!
//! **Where it is read.** [`Explorer::size_run`] (by the folder's path) gives the [`SizeRun`]; the file list's Size
//! column and the grid's tiles ask [`Explorer::folder_size`] for a folder's size (and show "…" while the run is
//! `running` and the folder has none), the status bar reads [`SizeRun::sized`], and the rows are sorted and
//! grouped by it when the sort or the grouping is by size (rows are rebuilt when a run ends). Anyone who shows
//! sizes redraws on [`ExplorerEvent::SizesChanged`] (or on any change of the explorer).

use super::actions_types::*;
use super::model::Explorer;
use super::rows::RowSizes;
use super::tab::Tab;
use super::types::*;
use glint_model::entries::SortKey;
use glint_model::groups::GroupBy;
use glint_model::paths::{is_folder_path, same_path};
use gpui_kit::Context;
use std::collections::HashMap;
use std::sync::Arc;

/// How many folders' sizes are kept (the TypeScript kept every one it ever measured).
const KEPT_MOST: usize = 256;

/// What a run began with, for its end.
struct Began {
    tab: TabId,
    path: String,
    /// What the folder had before a manual run replaced it with "running", to put back if the run fails.
    previous: Option<SizeRun>,
    run: u64,
    auto: bool,
}

/// What a folder's sizes are filed under: its path in lower case, without the separators at its end.
fn key_of(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

impl Explorer {
    /// The sizes measured for the folder at `path`, being measured or done (`sizes[path]`).
    pub fn size_run(&self, path: &str) -> Option<&SizeRun> {
        self.actions.sizes.get(&key_of(path))
    }

    /// One subfolder's size, if it has been measured: `folder` is the folder shown and `name` the subfolder's name
    /// in its listing (`sizes?.sizes.get(entry.name)`).
    pub fn folder_size(&self, folder: &str, name: &str) -> Option<u64> {
        self.size_run(folder)?.size_of(name)
    }

    /// The open folder is being measured now: the command bar says "Stop calculating folder sizes" and shows a
    /// spinner (`!!s.sizes[path]?.running`).
    pub fn is_sizing(&self) -> bool {
        self.size_run(self.current_path()).is_some_and(|run| run.running)
    }

    /// "Folder sizes": measures the open folder's subfolders (`calcSizes`). Nothing for This PC, the Recycle Bin,
    /// Recent, the Gallery or a remote's top. A failure says `Couldn't calculate folder sizes`.
    pub fn calc_sizes(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_tab_id();
        self.size_folder(tab, false, cx);
    }

    /// Stops measuring the open folder, whichever tab started it (`stopSizes`).
    pub fn stop_sizes(&mut self) {
        let lane = self
            .size_run(self.current_path())
            .filter(|run| run.running)
            .map(|run| run.lane);
        if let Some(lane) = lane {
            self.source.cancel_folder_sizes(&lane.to_string());
        }
    }

    /// A closed tab stops what it was measuring (`cancelFolderSizes(id)` in `forget`).
    pub(super) fn stop_sizes_of(&mut self, tab: TabId) {
        if self.actions.sizes.values().any(|run| run.running && run.lane == tab) {
            self.source.cancel_folder_sizes(&tab.to_string());
        }
    }

    /// The sizes the rows of `tab` are ordered or grouped by: only while the sort or the grouping is by size,
    /// and not for search results.
    pub(super) fn row_sizes(&self, tab: &Tab) -> Option<RowSizes> {
        let by_size = tab.sort.key == SortKey::Size || (tab.view == ViewMode::List && tab.group == GroupBy::Size);
        if tab.find.is_some() || !by_size {
            return None;
        }
        let run = self.size_run(&tab.path)?;
        Some(RowSizes {
            version: run.version,
            sizes: run.sizes.clone(),
        })
    }

    /// The listing of `tab` was just shown: in an archive its sizes come with it, and where the index has the
    /// folder's drive they are read quietly from the index (`archiveSizes`, `autoSizes`).
    pub(super) fn sizes_for_listing(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(shown) = self.tabs.get(&tab) else {
            return;
        };
        let Some(listing) = &shown.listing else {
            return;
        };
        if let Some(archive) = &listing.archive {
            let mut sizes = HashMap::new();
            let mut total = 0u64;
            for entry in listing.entries.iter() {
                if entry.dir {
                    sizes.insert(entry.name.clone(), entry.size);
                }
                total = total.saturating_add(entry.size);
            }
            let (path, format) = (shown.path.clone(), archive.format.to_string());
            self.actions.run_seq += 1;
            let run = self.actions.run_seq;
            self.save_sizes(
                &path,
                Some(SizeRun {
                    sizes: Arc::new(sizes),
                    total,
                    elapsed: 0.0,
                    running: false,
                    engine: format,
                    lane: tab,
                    version: run,
                    run,
                }),
                cx,
            );
        } else if self.indexed(&shown.path) {
            self.size_folder(tab, true, cx);
        }
    }

    /// The folders on screen and whether the index has each one's drive: taken before instant search's state
    /// changes, to tell a folder whose drive just became ready.
    pub(super) fn indexed_on_screen(&self) -> Vec<(TabId, String, bool)> {
        self.visible_tabs()
            .into_iter()
            .filter_map(|id| self.tabs.get(&id))
            .map(|tab| (tab.id, tab.path.clone(), self.indexed(&tab.path)))
            .collect()
    }

    /// Instant search's state changed: a folder on screen whose drive just became ready shows its sizes without
    /// waiting for a navigation (`refreshIndex`).
    pub(super) fn sizes_after_index(&mut self, before: Vec<(TabId, String, bool)>, cx: &mut Context<Self>) {
        for (tab, path, was) in before {
            let still_there = self.tabs.get(&tab).is_some_and(|t| same_path(&t.path, &path));
            if !was && still_there && self.indexed(&path) {
                self.size_folder(tab, true, cx);
            }
        }
    }

    /// Measures `tab`'s folder (`sizeFolder`). `auto` asks the index alone, shows nothing while it waits and says
    /// nothing if it can't answer; a manual run shows as running and says why it failed.
    fn size_folder(&mut self, tab: TabId, auto: bool, cx: &mut Context<Self>) {
        let Some(path) = self
            .tabs
            .get(&tab)
            .map(|t| t.path.clone())
            .filter(|path| is_folder_path(path))
        else {
            return;
        };
        let previous = self.size_run(&path).cloned();
        self.actions.run_seq += 1;
        let run = self.actions.run_seq;
        if !auto {
            // The index answers in a millisecond or two: no "calculating…" flash for it.
            let started = match &previous {
                Some(before) => SizeRun {
                    running: true,
                    lane: tab,
                    run,
                    ..before.clone()
                },
                None => SizeRun {
                    sizes: Arc::new(HashMap::new()),
                    total: 0,
                    elapsed: 0.0,
                    running: true,
                    engine: "pdu".to_string(),
                    lane: tab,
                    version: run,
                    run,
                },
            };
            self.save_sizes(&path, Some(started), cx);
        }
        let asking = self.source.folder_sizes(&path, &tab.to_string(), auto);
        let began = Began {
            tab,
            path,
            previous,
            run,
            auto,
        };
        cx.spawn(async move |this, cx| {
            let result = asking.await;
            this.update(cx, |this, cx| this.sizes_measured(began, result, cx)).ok();
        })
        .detach();
    }

    /// The answer came (`sizeFolder` after `folderSizes`).
    fn sizes_measured(&mut self, began: Began, result: Result<Measured, String>, cx: &mut Context<Self>) {
        let Began {
            tab,
            path,
            previous,
            run,
            auto,
        } = began;
        match result {
            Ok(measured) => {
                self.actions.run_seq += 1;
                let version = self.actions.run_seq;
                self.save_sizes(
                    &path,
                    Some(SizeRun {
                        sizes: Arc::new(measured.sizes.into_iter().collect()),
                        total: measured.total,
                        elapsed: measured.elapsed,
                        running: false,
                        engine: measured.engine,
                        lane: tab,
                        version,
                        run,
                    }),
                    cx,
                );
            }
            Err(_) if auto => {}
            Err(message) => {
                // Only the newest run puts back what was there before it (an older, stopped run must not undo a
                // newer one's "running").
                if self.size_run(&path).is_some_and(|current| current.run == run) {
                    let restored = previous.map(|before| SizeRun {
                        running: false,
                        ..before
                    });
                    self.save_sizes(&path, restored, cx);
                }
                if message != "cancelled" {
                    cx.emit(ExplorerEvent::Action(ActionNotice::Failed {
                        what: ActionFailure::FolderSizes,
                        message,
                    }));
                }
            }
        }
    }

    /// Files (or takes away) the folder's run, tells everyone, and orders the rows again where they depend on it.
    fn save_sizes(&mut self, path: &str, run: Option<SizeRun>, cx: &mut Context<Self>) {
        let key = key_of(path);
        match run {
            Some(run) => {
                self.actions.sizes.insert(key, run);
            }
            None => {
                self.actions.sizes.remove(&key);
            }
        }
        while self.actions.sizes.len() > KEPT_MOST {
            let oldest = self
                .actions
                .sizes
                .iter()
                .filter(|(_, run)| !run.running)
                .min_by_key(|(_, run)| run.run)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => self.actions.sizes.remove(&key),
                None => break,
            };
        }
        cx.emit(ExplorerEvent::SizesChanged { path: path.to_string() });
        let showing: Vec<TabId> = self
            .tabs
            .values()
            .filter(|t| t.find.is_none() && same_path(&t.path, path))
            .map(|t| t.id)
            .collect();
        for id in showing {
            self.rebuild_rows(id, cx);
        }
        // Two folders of one name differ when their measured sizes do (compare panes).
        self.refresh_comparison(cx);
        cx.notify();
    }
}
