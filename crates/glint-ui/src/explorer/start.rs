//! Starting up and keeping the outside world in step: the first folders, the saved session, the watches on
//! the folders on screen and the hints for the next start.
//! Ported from explorer.ts: `initExplorer` (the parts that set up state, not the window), `restoreSession`,
//! `openTab`, `saveSession`, `syncWatches`, the start-up hint subscription, the `report_folder`
//! subscription and `firstFolderPainted`.
//!
//! **Not here** (they belong to the window or to other parts): the remotes and folder rules start
//! (`initRemotes`, `initRules`), the instant-search index check, the jump list and taskbar thumbnail
//! buttons, the New menu's kinds, the gallery's folders and the copy queue. `init_explorer` takes the folders
//! the app was started with as an argument; the launch module reads them.

use super::model::{Explorer, Prefs};
use super::nav::{navigate_in, Walk, Walking};
use super::source::Effect;
use super::tab::Tab;
use super::types::*;
use glint_model::paths::{in_archive_path, is_folder_path, parent_path, RECENT, THIS_PC};
use glint_model::store::{Session, SessionPane, StartFolder};
use gpui_kit::{AsyncApp, Context, Task, WeakEntity};
use std::collections::HashMap;
use std::time::Duration;

/// The folders shown are told to the next start this long after they last changed.
const HINT_AFTER: Duration = Duration::from_millis(1500);

impl Explorer {
    /// Opens the first folders: last time's tabs or the start folder, and any the app was started with
    /// (`initExplorer`). The task resolves once they are all open.
    ///
    /// Call it once, after [`Explorer::set_memory`] (the saved last folder and session) and with the
    /// launch module's targets: folders from File Explorer, a double-click or the command line, each in a tab
    /// of its own. Drives and known folders are read first and don't hold up the first folder, except that
    /// the known folders (Home) are found before it opens.
    #[must_use = "dropping the task cancels start-up; `.detach()` it to let it run"]
    pub fn init_explorer(&mut self, launch: Vec<LaunchTarget>, cx: &mut Context<Self>) -> Task<()> {
        // Once: a second call has nothing to start.
        if std::mem::replace(&mut self.initialized, true) {
            return Task::ready(());
        }
        // Drives fill in when they answer; a sleeping network drive mustn't hold up the first folder.
        self.refresh_drives(cx).detach();
        let known = self.source.known_folders();
        cx.spawn(async move |this, cx| {
            let known = known.await;
            let opened = this.update(cx, |this, cx| {
                this.known = known.into();
                cx.emit(ExplorerEvent::KnownFoldersChanged);
                this.begin(launch, cx)
            });
            let Ok(opened) = opened else {
                return;
            };
            match opened {
                Start::Tabs(ids) => {
                    // All at once, none waiting on another (the TypeScript's Promise.all).
                    let mut opening = Vec::new();
                    for id in ids {
                        if let Ok(task) = this.update(cx, |this, cx| this.open_tab(id, cx)) {
                            opening.push(task);
                        }
                    }
                    for task in opening {
                        task.await;
                    }
                }
                Start::Folder { start, silent, home } => {
                    let first = this.update(cx, |this, _| this.active_tab_id()).unwrap_or(TabId(0));
                    let options = NavOptions {
                        silent,
                        ..NavOptions::in_tab(first)
                    };
                    if navigate_in(&this, cx, start, options).await == Walk::Failed
                        && navigate_in(&this, cx, home, NavOptions::in_tab(first)).await == Walk::Failed
                    {
                        navigate_in(&this, cx, THIS_PC.to_string(), NavOptions::in_tab(first)).await;
                    }
                }
            }
            this.update(cx, |this, cx| {
                this.restored = true;
                this.save_session(cx);
            })
            .ok();
        })
    }

    /// Sets up the tabs the app starts with, and says which folders to open (the first half of `init`).
    fn begin(&mut self, launch: Vec<LaunchTarget>, cx: &mut Context<Self>) -> Start {
        let prefs = Prefs::read(cx);
        let home = self.home_folder().to_string();
        let session = self.memory.session().cloned();
        let restored = match session {
            Some(session) if prefs.restore_tabs => self.restore_session(&session),
            _ => false,
        };
        let start = if restored {
            let last = self.place_outside_tabs(&launch, true);
            self.show_on_start(last);
            Start::Tabs(self.visible_tabs())
        } else if let Some((first, rest)) = launch.split_first() {
            // The first one instead of the start folder.
            let id = self.active_tab_id();
            let view = self.tabs.get(&id).map_or(ViewMode::List, |t| t.view);
            self.tabs
                .insert(id, Tab::blank(id, &first.path, view, first.select.as_deref()));
            let last = self.place_outside_tabs(rest, false);
            self.show_on_start(last);
            Start::Tabs(self.visible_tabs())
        } else {
            let last = self.memory.last_folder().map(str::to_string);
            let start = match (prefs.start_folder, last) {
                (StartFolder::ThisPc, _) => THIS_PC.to_string(),
                (StartFolder::Recent, _) => RECENT.to_string(),
                (StartFolder::Last, Some(last)) => last,
                _ => home.clone(),
            };
            Start::Folder {
                silent: start != home,
                start,
                home,
            }
        };
        cx.emit(ExplorerEvent::TabsChanged);
        cx.notify();
        start
    }

    /// Puts back last time's tabs and panes; each folder opens when its tab is first shown. `false` if there's
    /// nothing to restore (`restoreSession`).
    pub(super) fn restore_session(&mut self, session: &Session) -> bool {
        let mut tabs = HashMap::new();
        let mut panes = Vec::new();
        for pane in session.panes.iter().filter(|p| !p.paths.is_empty()).take(2) {
            let mut ids = Vec::new();
            for (i, path) in pane.paths.iter().enumerate() {
                let id = self.new_tab_id();
                let view = match pane.views.as_ref().and_then(|v| v.get(i)) {
                    Some(&ViewMode::Grid) => ViewMode::Grid,
                    Some(&ViewMode::Columns) => ViewMode::Columns,
                    _ => ViewMode::List,
                };
                tabs.insert(id, Tab::blank(id, path, view, None));
                ids.push(id);
            }
            let at = pane.active.min(ids.len().saturating_sub(1));
            if let Some(&active) = ids.get(at) {
                panes.push(Pane { tabs: ids, active });
            }
        }
        if panes.is_empty() {
            return false;
        }
        let dual = session.dual && panes.len() == 2;
        self.tabs = tabs;
        self.panes = panes;
        self.dual = dual;
        self.active_pane = if dual { session.active_pane.min(1) } else { 0 };
        true
    }

    /// Opens a tab's folder for the first time; if it's gone, the nearest parent that's there, else Home
    /// (`openTab`). The first attempt starts at once, so a navigation the caller starts right after wins over
    /// it, as in the TypeScript.
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub(super) fn open_tab(&mut self, id: TabId, cx: &mut Context<Self>) -> Task<()> {
        let path = self.tabs.get(&id).map_or(THIS_PC.to_string(), |t| t.path.clone());
        let home = self.home_folder().to_string();
        let first = (path != THIS_PC).then(|| self.begin_walk(id, path.clone(), silently(id), cx));
        cx.spawn(async move |this, cx| open_tab_walk(&this, cx, id, path, home, first).await)
    }

    /// Remembers each pane's tabs for the next start (`saveSession`). Only once start-up has restored them.
    pub(super) fn save_session(&mut self, cx: &mut Context<Self>) {
        if !self.restored {
            return;
        }
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
        if self.memory.set_session(session) {
            cx.emit(ExplorerEvent::MemoryChanged);
        }
    }

    /// Watches the folders on screen, and only those: a tab out of sight is re-read when it's shown
    /// (`syncWatches`). A folder changing raises core's "dir-changed", which `reload_folder` answers.
    pub(super) fn sync_watches(&mut self) {
        let mut folders: Vec<(String, String)> = Vec::new();
        for id in self.visible_tabs() {
            let Some(tab) = self.tabs.get(&id) else {
                continue;
            };
            if tab.listing.is_some() && is_folder_path(&tab.path) {
                let folder = tab.watched_folder();
                let key = folder.to_lowercase();
                // Like a JavaScript Map: a repeated key keeps its place and takes the later value.
                match folders.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = folder,
                    None => folders.push((key, folder)),
                }
            }
        }
        let key = folders.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join("|");
        if key == self.watching {
            return;
        }
        // If there was no window to watch for yet, the key stays as it was and the next change asks again.
        if self
            .source
            .watch(folders.into_iter().map(|(_, folder)| folder).collect())
        {
            self.watching = key;
        }
    }

    /// The folders shown in the panes now go to Rust, for the next start to read ahead (`startup_hint`), a
    /// moment after they last changed.
    pub(super) fn hint_startup(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .shown_panes()
            .iter()
            .filter_map(|p| self.tabs.get(&p.active))
            .map(|t| t.path.clone())
            .filter(|p| is_folder_path(p) && !in_archive_path(p))
            .collect();
        let key = paths.join("\n");
        if key == self.hinted {
            return;
        }
        self.hinted = key;
        // Replacing the task cancels the older wait, as clearTimeout did.
        self.hint_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HINT_AFTER).await;
            this.update(cx, |this, _| this.source.effect(Effect::StartupHint(paths)))
                .ok();
        }));
    }

    /// The folder shown goes to Rust, for Ctrl+Alt+G in a file dialog (`report_folder`).
    pub(super) fn report_folder(&mut self) {
        let path = self.current_path();
        if !path.is_empty() && path != self.reported_folder && is_folder_path(path) && !in_archive_path(path) {
            let path = path.to_string();
            self.reported_folder = path.clone();
            self.source.effect(Effect::ReportFolder(path));
        }
    }

    /// The first folder painted after Glint started: the bench harness's cold start (`firstFolderPainted`).
    /// The file list calls it after it first draws a listing; only the first call counts, and it does nothing
    /// unless `GLINT_BENCH_START` is set (see `startup.rs` in core).
    pub fn first_folder_painted(&mut self) {
        if self.first_painted {
            return;
        }
        self.first_painted = true;
        self.source.effect(Effect::BenchFirstFolder);
    }
}

/// What to open at start.
enum Start {
    /// These tabs, each at its own folder.
    Tabs(Vec<TabId>),
    /// The start folder in the first tab; Home, then This PC, if it won't open.
    Folder { start: String, silent: bool, home: String },
}

/// Options for the quiet attempts of opening a tab for the first time.
fn silently(id: TabId) -> NavOptions {
    NavOptions {
        silent: true,
        ..NavOptions::in_tab(id)
    }
}

/// The rest of `open_tab`: the folder (`first`, already under way), else the nearest parent that's there,
/// else Home, else This PC.
async fn open_tab_walk(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    id: TabId,
    path: String,
    home: String,
    first: Option<Walking>,
) {
    let mut first = first;
    if path != THIS_PC {
        let mut at = path;
        while at != THIS_PC {
            let walk = match first.take() {
                Some(walking) => walking.finish(this, cx).await,
                None => navigate_in(this, cx, at.clone(), silently(id)).await,
            };
            // Opened, or the person opened something else here meanwhile.
            if walk != Walk::Failed {
                return;
            }
            if !this.update(cx, |this, _| this.tabs.contains_key(&id)).unwrap_or(false) {
                return;
            }
            at = parent_path(&at);
        }
        if navigate_in(this, cx, home, silently(id)).await != Walk::Failed {
            return;
        }
    }
    navigate_in(this, cx, THIS_PC.to_string(), NavOptions::in_tab(id)).await;
}
