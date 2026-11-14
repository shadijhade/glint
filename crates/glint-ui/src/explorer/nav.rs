//! Navigation: opening folders, history, re-reading, prefetching and the drives.
//! Ported from explorer.ts: `navigate`, `getAccess` (in open.rs), `goBack`, `goForward`, `goUp`, `reload`,
//! `reloadFolder`, `reread`, `refresh`, `refreshDrives`, `prefetchFolder`, `takePrefetched`, `hoverFolder`,
//! `leaveFolder`, and `showArchiveReading` (a big archive read whole says how far along it is, with Stop).
//!
//! **Stale results.** Every tab counts its navigations. A listing that comes back for an older number than
//! the tab's newest was superseded (the person clicked elsewhere meanwhile, or the tab closed) and is
//! dropped: it changes nothing, and its `Task` resolves `false`.

use super::model::Explorer;
use super::source::Effect;
use super::types::*;
use crate::startup::ReadingNote;
use glint_model::archives::needs_password;
use glint_model::columns::FolderKind;
use glint_model::paths::{in_archive_path, is_folder_path, parent_path, same_path, THIS_PC};
use gpui_kit::{App, AppContext as _, AsyncApp, Context, Global, Task, WeakEntity};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A folder that takes this long to list may be an archive being read whole: the notice looks then.
const READING_FIRST: Duration = Duration::from_millis(400);
/// And asks again this often while the folder is still being listed.
const READING_EVERY: Duration = Duration::from_millis(250);

/// A prefetched listing is good for this long (and only while the folder's own date is unchanged).
const PREFETCH_FRESH: Duration = Duration::from_millis(2000);
/// The most folders read ahead at once: they're for the next click.
const PREFETCH_MOST: usize = 8;
/// Back history keeps this many folders.
const HISTORY_MOST: usize = 100;
/// The pointer rests on a folder this long before it's read ahead (the web app waited 80 ms): its row says what is
/// in it once it has been read, so this is most of what is waited for. Long enough that a pointer crossing the
/// list reads nothing on its way.
const HOVER_FOR: Duration = Duration::from_millis(60);
/// A folder that takes this long to open says it is loading. One read in a few milliseconds is on screen before
/// a frame could have said so, and drawing that frame would only have kept its rows waiting for the next one.
const LOADING_SAID_AFTER: Duration = Duration::from_millis(40);
/// A folder re-read in less than this, while it changes, is read again as soon as the watcher says (every
/// 200 ms at most); one that takes longer waits ten times as long as its read took, up to `REREAD_PAUSE_MAX`.
/// A folder of 100,000 files that takes 47 ms to read (2026-11-11) is read about twice a second, not five times.
pub(super) const REREAD_SLOW: Duration = Duration::from_millis(20);
const REREAD_PAUSE_MAX: Duration = Duration::from_secs(2);

// ---- A big archive read whole -----------------------------------------------------------------------------

/// How far the first read of an archive has come: the archive's path, and compressed bytes read of its size
/// (`ReadProgress`).
#[derive(Clone, Debug, PartialEq, Eq)]
struct ArchiveRead {
    archive: String,
    done: u64,
    total: u64,
}

/// What the archive-reading notice asks glint-core: how far the read has come, and to stop it. The app uses
/// [`ArchiveProbe::core`]; a test puts its own in a global.
#[derive(Clone)]
struct ArchiveProbe {
    progress: ReadProgress,
    cancel: Arc<dyn Fn(&str) + Send + Sync>,
}

/// Asks how far the read of the archive a path is in has come, if one is under way.
type ReadProgress = Arc<dyn Fn(&str) -> Option<ArchiveRead> + Send + Sync>;

impl Global for ArchiveProbe {}

impl ArchiveProbe {
    /// glint-core's own (`archive_progress`, `cancel_archive_read`): quick looks at what its reads keep.
    fn core() -> Self {
        ArchiveProbe {
            progress: Arc::new(|path| {
                let found = glint_core::archive::archive_progress(path.to_string())?;
                Some(ArchiveRead {
                    archive: found.path,
                    done: found.done,
                    total: found.total,
                })
            }),
            cancel: Arc::new(|path| glint_core::archive::cancel_archive_read(path.to_string())),
        }
    }
}

fn archive_probe(cx: &App) -> ArchiveProbe {
    cx.try_global::<ArchiveProbe>()
        .cloned()
        .unwrap_or_else(ArchiveProbe::core)
}

/// The folders whose reading Stop was pressed on, in lower case: their notice is not shown again while the read
/// winds down.
#[derive(Default)]
struct ReadingStops(HashSet<String>);

impl Global for ReadingStops {}

fn stop_key(target: &str) -> String {
    target.to_lowercase()
}

/// Watches one navigation for an archive being read whole; [`ReadingWatch::finish`] ends it.
struct ReadingWatch {
    target: String,
    /// The notice was shown.
    shown: Rc<Cell<bool>>,
    /// Looks every so often; dropped (cancelled) when the navigation ends.
    _poll: Task<()>,
}

impl ReadingWatch {
    /// The folder is listed or failed: the looking stops and the notice goes.
    fn finish(self, cx: &mut AsyncApp) {
        let ReadingWatch { target, shown, _poll } = self;
        drop(_poll);
        cx.update(|cx| {
            let stopped = cx.default_global::<ReadingStops>().0.remove(&stop_key(&target));
            // Stop already took the notice away.
            if shown.get() && !stopped {
                crate::startup::archive_reading(cx, ReadingNote::Ended { target, shown: true });
            }
        });
    }
}

/// A folder read ahead: the listing, once it's in, and the folder's own date from before it was read.
pub(super) struct PrefetchEntry {
    /// The folder, as it was asked for.
    path: String,
    at: Instant,
    stamp: Option<f64>,
    state: PrefetchState,
    /// What the folder held when it was last read ahead, kept while it is read again: what its row says of it
    /// ([`Explorer::peeks`]) stays put meanwhile.
    seen: Option<Listed>,
}

enum PrefetchState {
    /// Still being read; anyone who wants it waits on a channel.
    Pending(Vec<async_channel::Sender<(Option<f64>, Option<Listed>)>>),
    Ready(Option<Listed>),
}

/// What `navigate` takes from the prefetches.
enum Claim {
    Ready(Option<f64>, Option<Listed>),
    Waiting(async_channel::Receiver<(Option<f64>, Option<Listed>)>),
}

/// What happens next to a failed navigation, decided by its message (see `navigate`).
fn failure_notice(message: &str, target: &str, tab: TabId, select: &Option<Vec<String>>) -> Option<Notice> {
    if message == "cancelled" {
        // Stopped reading an archive: nothing to report.
        return None;
    }
    // One whose list of files is encrypted asks for its password.
    if needs_password(message) {
        return Some(Notice::ArchivePassword {
            path: target.to_string(),
            tab,
            select: select.clone(),
            message: message.to_string(),
        });
    }
    // A disk image or installer before 7-Zip is installed: install it, then open it.
    if message.contains("7-Zip isn't installed") {
        return Some(Notice::NeedsSevenZip {
            path: target.to_string(),
            tab,
            select: select.clone(),
        });
    }
    Some(Notice::CannotOpen {
        path: target.to_string(),
        message: message.to_string(),
    })
}

/// The entries of `entries` whose paths match `wanted`, ignoring case and a trailing separator, with the
/// path as the listing spells it.
///
/// Only entries as long as something asked for are lowercased: a big folder read again (a change on disk)
/// with a few items selected doesn't lowercase every path in it.
pub(super) fn match_selection(entries: &[glint_model::entries::FileEntry], wanted: &[String]) -> Vec<String> {
    let asked: Vec<&str> = wanted.iter().map(|p| p.trim_end_matches(['\\', '/'])).collect();
    let lengths: HashSet<usize> = asked.iter().map(|p| utf16_len(p)).collect();
    let by_lower: HashMap<String, &str> = entries
        .iter()
        .filter(|e| lengths.contains(&utf16_len(&e.path)))
        .map(|e| (e.path.to_lowercase(), e.path.as_str()))
        .collect();
    asked
        .iter()
        .filter_map(|p| by_lower.get(&p.to_lowercase()).map(|s| (*s).to_string()))
        .collect()
}

/// A string's length in UTF-16 units, how JavaScript counts it. Most paths are ASCII, one unit a byte: told at once.
fn utf16_len(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.chars().map(char::len_utf16).sum()
    }
}

/// What a refresh (F5) does.
pub enum Refresh {
    /// The folder is being read again.
    Folder(Task<()>),
    /// The tab shows a search: the find part runs it again with this query and scope.
    Search { query: String, scope: String },
}

impl Explorer {
    /// Opens a folder (or This PC) in a tab; the task resolves `false` if it couldn't, or a newer navigation
    /// won (`navigate`).
    ///
    /// The listing is read on core's runtime and decoded off the UI thread; the tab shows its old folder with
    /// [`Tab::loading`](super::Tab::loading) set until the new one is in. A path that is an archive, a remote
    /// folder, the Recycle Bin, Recent or the Gallery lists like any other. A folder Windows keeps this
    /// account out of opens all the same: its tab says why ([`LoadState::NoAccess`]).
    ///
    /// Failures are not shown here: unless `silent`, a [`Notice`] event says what to tell the person.
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn navigate_task(
        &mut self,
        target: impl Into<String>,
        options: NavOptions,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let target: String = target.into();
        let id = options.tab.unwrap_or_else(|| self.active_tab_id());
        let Some(tab) = self.tabs.get_mut(&id) else {
            return Task::ready(false);
        };
        tab.nav_seq += 1;
        let seq = tab.nav_seq;
        let started = Instant::now();
        if !options.quiet {
            tab.loading = true;
            if !tab.ready {
                tab.load = LoadState::Loading;
            }
            // Said after a moment, if the folder is still being read then (see `LOADING_SAID_AFTER`).
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(LOADING_SAID_AFTER).await;
                this.update(cx, |this, cx| {
                    if this.tabs.get(&id).is_some_and(|tab| tab.nav_seq == seq && tab.loading) {
                        cx.emit(ExplorerEvent::TabChanged(id));
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        }
        let claim = (target != THIS_PC).then(|| self.claim_prefetch(&target)).flatten();
        let source = self.source.clone();
        // Listing a big .tar.gz reads it whole: while it does, a notice says how far along that is, with Stop.
        let watch = (target != THIS_PC).then(|| self.watch_archive_reading(&target, cx));
        cx.spawn(async move |this, cx| {
            let outcome = if target == THIS_PC {
                None
            } else {
                Some(read_folder(&*source, &target, claim).await)
            };
            if let Some(watch) = watch {
                watch.finish(cx);
            }
            crate::perf::say(|| {
                let (items, core) = match &outcome {
                    Some(Ok(listing)) => (listing.entries.len(), listing.elapsed),
                    _ => (0, 0.0),
                };
                format!(
                    "nav read at={:.2} core={core:.2} items={items} path={target}",
                    crate::perf::ms(started)
                )
            });
            let opened = this
                .update(cx, |this, cx| {
                    let applying = Instant::now();
                    let opened = this.finish_navigation(id, seq, started, target, options, outcome, cx);
                    crate::perf::say(|| {
                        format!(
                            "nav applied at={:.2} took={:.2}",
                            crate::perf::ms(started),
                            crate::perf::ms(applying)
                        )
                    });
                    opened
                })
                .unwrap_or(false);
            // Everyone who follows the explorer has heard by now.
            crate::perf::say(|| format!("nav told at={:.2}", crate::perf::ms(started)));
            opened
        })
    }

    /// Looks, after a moment and then every so often, whether `target` is an archive being read whole, and says
    /// how far along that is ([`ReadingNote`]) until the returned watch is finished (`showArchiveReading`).
    fn watch_archive_reading(&mut self, target: &str, cx: &mut Context<Self>) -> ReadingWatch {
        let shown = Rc::new(Cell::new(false));
        let seen = shown.clone();
        let asked = target.to_string();
        let poll = cx.spawn(async move |_, cx| {
            cx.background_executor().timer(READING_FIRST).await;
            loop {
                let probe = cx.update(|cx| archive_probe(cx));
                let looking = asked.clone();
                // Off the UI thread: core's reads keep their state behind a lock.
                let progress = cx.background_spawn(async move { (probe.progress)(&looking) }).await;
                let stopped = cx.update(|cx| {
                    cx.try_global::<ReadingStops>()
                        .is_some_and(|stops| stops.0.contains(&stop_key(&asked)))
                });
                if stopped {
                    return;
                }
                if let Some(read) = progress {
                    seen.set(true);
                    let note = ReadingNote::Progress {
                        target: asked.clone(),
                        archive: read.archive,
                        done: read.done,
                        total: read.total,
                    };
                    cx.update(|cx| crate::startup::archive_reading(cx, note));
                }
                cx.background_executor().timer(READING_EVERY).await;
            }
        });
        ReadingWatch {
            target: target.to_string(),
            shown,
            _poll: poll,
        }
    }

    /// Stop on the archive-reading notice: the first read of the archive `target` is in is cancelled (its
    /// listing then fails with "cancelled", which says nothing), and the notice isn't shown again meanwhile. The
    /// caller takes the notice down ([`crate::startup`] does) once this has returned.
    pub fn stop_archive_reading(&mut self, target: &str, cx: &mut Context<Self>) {
        cx.default_global::<ReadingStops>().0.insert(stop_key(target));
        let probe = archive_probe(cx);
        let target = target.to_string();
        cx.background_spawn(async move { (probe.cancel)(&target) }).detach();
    }

    /// [`Explorer::navigate_task`], detached: opens the folder and doesn't wait.
    pub fn navigate(&mut self, target: impl Into<String>, options: NavOptions, cx: &mut Context<Self>) {
        self.navigate_task(target, options, cx).detach();
    }

    /// The listing is in (or failed): applies it to the tab, if this is still the newest navigation.
    #[allow(clippy::too_many_arguments)] // The navigation's own parts, passed through from one function.
    fn finish_navigation(
        &mut self,
        id: TabId,
        seq: u64,
        started: Instant,
        target: String,
        options: NavOptions,
        outcome: Option<Result<Listed, ListFailure>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let (listing, denied) = match outcome {
            None => (None, None),
            Some(Ok(listing)) => (Some(listing), None),
            // A folder Windows keeps this account out of opens all the same.
            Some(Err(failure)) if failure.kind == FailureKind::Denied => (None, Some(failure)),
            Some(Err(failure)) => {
                if self.tabs.get(&id).is_some_and(|t| t.nav_seq == seq) {
                    self.update_tab(id, |t| {
                        t.loading = false;
                        if !t.ready {
                            t.load = LoadState::Failed(failure.message.clone());
                        }
                    });
                    if !options.silent {
                        if let Some(notice) = failure_notice(&failure.message, &target, id, &options.select) {
                            cx.emit(ExplorerEvent::Notice(notice));
                        }
                    }
                    self.tab_changed(id, cx);
                    // The tab stays where it was: what changed there meanwhile is read now.
                    self.reload_if_waiting(id, true, cx);
                }
                return false;
            }
        };
        // A newer navigation in this tab won, or the tab was closed meanwhile.
        let Some(state) = self.tabs.get(&id).filter(|t| t.nav_seq == seq) else {
            return false;
        };

        let path = listing
            .as_ref()
            .map(|l| l.path.clone())
            .or_else(|| denied.as_ref().map(|d| d.path.clone()))
            .unwrap_or_else(|| THIS_PC.to_string());
        let same = state.ready
            && if path == THIS_PC {
                state.path == THIS_PC
            } else {
                state.path != THIS_PC && same_path(&path, &state.path)
            };
        // Listed from the index or not at all, the folder is one Windows keeps closed; a refresh keeps what
        // Get access found out.
        let closed = match (&denied, &listing) {
            (Some(d), _) => Some(NoAccess {
                remote: d.remote,
                ..Default::default()
            }),
            (None, Some(l)) if l.from_index => Some(NoAccess::default()),
            _ => None,
        };
        let no_access = closed.map(|closed| match state.no_access() {
            Some(known) if same => *known,
            _ => closed,
        });
        // Select what was asked for; on a refresh keep the selection; a new tab selects what it was opened
        // with; going up selects where we came from.
        let wanted: Vec<String> = if let Some(select) = &options.select {
            select.clone()
        } else if same || (!state.ready && !state.selection.is_empty()) {
            state.selection.to_vec()
        } else if state.path != THIS_PC && same_path(&parent_path(&state.path), &path) {
            vec![state.path.clone()]
        } else {
            Vec::new()
        };
        let selected: Vec<String> = match &listing {
            Some(listing) if !wanted.is_empty() => match_selection(&listing.entries, &wanted),
            _ => Vec::new(),
        };
        let focus = match state.selection.focus() {
            Some(old) if same && selected.iter().any(|p| p == old) => Some(old.to_string()),
            _ => selected.last().cloned(),
        };
        let was_running = !same && state.find.as_ref().is_some_and(|f| f.running);
        let dropped_find = !same && state.find.is_some();
        let old_path = state.path.clone();
        let was_ready = state.ready;
        // Timed from the click to the list on screen; a tab out of sight is timed when it's shown.
        let visible = self.visible_tabs().contains(&id);

        let load = match (&no_access, &listing) {
            (Some(no_access), _) => LoadState::NoAccess(*no_access),
            (None, Some(listing)) => LoadState::Ready(Loaded {
                read_ms: listing.elapsed,
                engine: listing.engine(),
            }),
            (None, None) => LoadState::Ready(Loaded {
                read_ms: 0.0,
                engine: Engine::ThisPc,
            }),
        };
        let keep_history = !options.no_history && was_ready && !same;
        let has_listing = listing.is_some();
        let archived = listing.as_ref().is_some_and(|l| l.archive.is_some());
        // The folder left: a big one's entries are freed away from this thread.
        super::model::let_go(state.shared_entries(), cx);
        self.update_tab(id, |tab| {
            tab.ready = true;
            tab.path = path.clone();
            tab.listing = listing;
            tab.load = load;
            tab.loading = false;
            tab.selection = super::tab::Selection {
                paths: std::sync::Arc::new(selected.iter().map(String::as_str).collect()),
                anchor: focus.clone(),
                focus: focus.clone(),
            };
            if !same {
                tab.filter.clear();
                tab.find = None;
                tab.renaming = None;
            }
            if keep_history {
                tab.back.push(old_path);
                let extra = tab.back.len().saturating_sub(HISTORY_MOST);
                tab.back.drain(..extra);
                tab.forward.clear();
            }
            tab.nav_started = visible.then_some(started);
        });
        if dropped_find {
            // Going to another folder ends the search, as `navigate` did with `cancelFind`.
            self.stop_find_job(id, was_running);
            cx.emit(ExplorerEvent::FindDropped { tab: id, was_running });
        }
        if options.select.is_some() && !selected.is_empty() {
            self.flash_paths(selected.clone(), cx);
        }
        if !same {
            // The last folder and the visit are told together: each MemoryChanged copies the explorer's part of the
            // saved state into the app's and wakes everything that shows it.
            let mut remembered = false;
            if id == self.active_tab_id() {
                self.editing_address = false;
                remembered = self.memory.set_last_folder(&path);
            }
            self.sync_watches();
            if is_folder_path(&path) {
                self.memory.record_visit(&path, super::model::now_ms());
                remembered = true;
            }
            if remembered {
                cx.emit(ExplorerEvent::MemoryChanged);
            }
            // A folder on disk (not one in an archive) for zoxide too, when history is shared with it.
            if is_folder_path(&path) && has_listing && !archived && super::model::Prefs::one(cx, |s| s.zoxide) {
                self.source.effect(Effect::ZoxideAdd(path.clone()));
            }
            if path == THIS_PC {
                self.refresh_drives(cx).detach();
            }
        }
        // The kind of folder decides its default columns, and a folder whose own view is a grid or a list
        // opens as it (the Gallery as a grid, until it has its own): only as the folder opens, since
        // choosing the other one there changes what it keeps.
        let kind = self.kind_of(id);
        self.update_tab(id, |t| t.kind = kind);
        self.resolve_view(id, cx);
        if !same {
            self.apply_wanted_view(id, kind, cx);
        }
        self.rebuild_rows(id, cx);
        cx.emit(ExplorerEvent::ListingChanged { tab: id, same });
        cx.emit(ExplorerEvent::SelectionChanged(id));
        self.tab_changed(id, cx);
        // In an archive every folder's size comes with the listing; with the index, every folder shows its
        // subfolders' sizes as it opens (and after each refresh).
        self.sizes_for_listing(id, cx);
        // A read of this same folder that waited for the navigation is done now (the listing may be from before
        // the change); one of the folder left behind is forgotten.
        self.reload_if_waiting(id, same, cx);
        true
    }

    /// A navigation has landed: the read that waited for it ([`Explorer::reload`]) is done now if the tab still
    /// shows the folder it was for (`still`), and forgotten otherwise.
    fn reload_if_waiting(&mut self, id: TabId, still: bool, cx: &mut Context<Self>) {
        let waiting = self
            .tabs
            .get_mut(&id)
            .is_some_and(|tab| std::mem::take(&mut tab.reload_waiting));
        if waiting && still {
            self.reload(Some(id), cx).detach();
        }
    }

    /// A folder whose own view is a grid or a list opens as it (`Explorer.tsx`'s effect on `[path, tabId]`).
    fn apply_wanted_view(&mut self, id: TabId, kind: FolderKind, cx: &mut gpui_kit::Context<Self>) {
        let per_folder = super::model::Prefs::one(cx, |s| s.per_folder_views);
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let own = (per_folder && tab.find.is_none() && tab.path != THIS_PC)
            .then(|| self.memory.folder_view(&tab.path).and_then(|v| v.view))
            .flatten();
        let by_place = (per_folder && matches!(kind, FolderKind::Recent | FolderKind::Gallery | FolderKind::Downloads))
            .then(|| kind.def().view)
            .flatten();
        if let Some(wanted) = own.or(by_place) {
            if wanted != tab.view {
                self.update_tab(id, |t| t.view = wanted);
                cx.emit(ExplorerEvent::ViewChanged(id));
            }
        }
    }

    /// Records the folder in the memory as the last one the app was in.
    pub(super) fn remember_last_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.memory.set_last_folder(path) {
            cx.emit(ExplorerEvent::MemoryChanged);
        }
    }

    /// Back (Alt+Left): the previous folder in the active tab's history (`goBack`).
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        self.walk_history(true, cx)
    }

    /// Forward (Alt+Right): the folder Back came from (`goForward`).
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_forward(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        self.walk_history(false, cx)
    }

    fn walk_history(&mut self, backwards: bool, cx: &mut Context<Self>) -> Task<bool> {
        let id = self.active_tab_id();
        let Some(tab) = self.tabs.get(&id) else {
            return Task::ready(false);
        };
        let (back, forward, path) = (tab.back.clone(), tab.forward.clone(), tab.path.clone());
        let target = if backwards { back.last() } else { forward.first() };
        let Some(target) = target.cloned() else {
            return Task::ready(false);
        };
        let navigation = self.navigate_task(
            target,
            NavOptions {
                no_history: true,
                tab: Some(id),
                ..Default::default()
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            if !navigation.await {
                return false;
            }
            this.update(cx, |this, cx| {
                this.update_tab(id, |t| {
                    if backwards {
                        t.back = back.get(..back.len().saturating_sub(1)).unwrap_or(&[]).to_vec();
                        t.forward = std::iter::once(path).chain(forward).collect();
                    } else {
                        t.back = back.into_iter().chain(std::iter::once(path)).collect();
                        t.forward = forward.get(1..).unwrap_or(&[]).to_vec();
                    }
                });
                this.tab_changed(id, cx);
            })
            .is_ok()
        })
    }

    /// Up (Alt+Up, Backspace): the parent folder (`goUp`). `false` on This PC.
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_up(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        let path = self.current_path().to_string();
        if path == THIS_PC {
            return Task::ready(false);
        }
        self.navigate_task(parent_path(&path), NavOptions::default(), cx)
    }

    /// Re-reads a tab's folder in place; if it's gone, opens its parent (`reload`). The tab is the active
    /// one's by default.
    #[must_use = "dropping the task cancels the reload; `.detach()` it to let it run"]
    pub fn reload(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) -> Task<()> {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        let Some(t) = self.tabs.get_mut(&id) else {
            return Task::ready(());
        };
        if t.path == THIS_PC {
            return self.refresh_drives(cx);
        }
        // The tab is on its way to a folder. Reading the one it is leaving again would be the newer navigation
        // of the two, and the folder asked for would never open: a change on disk in the moment after Enter
        // took the tab back. The read waits for the navigation instead: `finish_navigation` does it if the tab
        // still shows this folder then.
        if t.loading {
            t.reload_waiting = true;
            return Task::ready(());
        }
        let (path, was_archive) = (t.path.clone(), t.archive().is_some());
        // A false result from a navigation that was overtaken by a newer one isn't a missing folder.
        let seq = t.nav_seq + 1;
        let navigation = self.navigate_task(
            path.clone(),
            NavOptions {
                no_history: true,
                quiet: true,
                silent: true,
                tab: Some(id),
                ..Default::default()
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            if navigation.await {
                return;
            }
            let still_there = this
                .update(cx, |this, _| this.tabs.get(&id).is_some_and(|t| t.nav_seq == seq))
                .unwrap_or(false);
            if !still_there {
                return;
            }
            this.update(cx, |_, cx| {
                cx.emit(ExplorerEvent::Notice(Notice::NoLongerAvailable { path: path.clone() }));
            })
            .ok();
            let up_options = |silent: bool| NavOptions {
                no_history: true,
                silent,
                tab: Some(id),
                ..Default::default()
            };
            if !was_archive {
                navigate_in(&this, cx, parent_path(&path), up_options(false)).await;
                return;
            }
            // A changed (or deleted) archive: the nearest folder above that's still there, in it or out of it.
            let mut at = parent_path(&path);
            while at != THIS_PC {
                if navigate_in(&this, cx, at.clone(), up_options(true)).await != Walk::Failed {
                    return;
                }
                if !this.update(cx, |this, _| this.tabs.contains_key(&id)).unwrap_or(false) {
                    return;
                }
                at = parent_path(&at);
            }
            navigate_in(&this, cx, THIS_PC.to_string(), up_options(false)).await;
        })
    }

    /// `folder` changed on disk: re-reads every tab on screen that shows it, or an archive in it
    /// (`reloadFolder`).
    pub fn reload_folder(&mut self, folder: &str, cx: &mut Context<Self>) {
        let shown: Vec<TabId> = self
            .visible_tabs()
            .into_iter()
            .filter(|id| {
                self.tabs
                    .get(id)
                    .is_some_and(|t| t.find.is_none() && t.path != THIS_PC && same_path(folder, &t.watched_folder()))
            })
            .collect();
        for id in shown {
            self.reread(id, folder, cx);
        }
    }

    /// One read at a time per tab, then one more for what changed meanwhile: a big folder that takes longer
    /// to read than changes take to come (a build, an unzip) isn't read several times over at once.
    fn reread(&mut self, id: TabId, folder: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(&id) else {
            return;
        };
        if let Some(waiting) = tab.rereading.as_mut() {
            *waiting = Some(folder.to_string());
            return;
        }
        tab.rereading = Some(None);
        let reading = self.reload(Some(id), cx);
        let started = Instant::now();
        cx.spawn(async move |this, cx| {
            reading.await;
            // Changes came while it was read (a build, a download, an unzip): a folder that took a while to read waits
            // ten times as long before it's read again (at most 2 s), so a big one isn't read back to back for as long
            // as the changes go on. Changes meanwhile join the read that follows.
            let took = started.elapsed();
            let again = this
                .read_with(cx, |this, _| {
                    this.tabs
                        .get(&id)
                        .and_then(|t| t.rereading.as_ref())
                        .is_some_and(Option::is_some)
                })
                .unwrap_or(false);
            if again && took >= REREAD_SLOW {
                cx.background_executor().timer((took * 10).min(REREAD_PAUSE_MAX)).await;
            }
            this.update(cx, |this, cx| {
                let again = this.tabs.get_mut(&id).and_then(|t| t.rereading.take()).flatten();
                let wanted = again.filter(|again| {
                    this.tabs
                        .get(&id)
                        .is_some_and(|t| t.find.is_none() && same_path(again, &t.watched_folder()))
                });
                if let Some(again) = wanted {
                    this.reread(id, &again, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// A remote connection was removed: the tabs showing its folders go to This PC (`leaveRemote`). `id` is the
    /// connection's id.
    pub fn leave_remote(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_lowercase();
        if id.is_empty() {
            return;
        }
        let leaving: Vec<TabId> = self
            .tabs
            .values()
            .filter(|t| glint_model::paths::remote_id_of(&t.path).to_lowercase() == id)
            .map(|t| t.id)
            .collect();
        for tab in leaving {
            self.navigate(THIS_PC, NavOptions::in_tab(tab), cx);
        }
    }

    /// F5: runs the search again when one is shown, otherwise re-reads the folder (`refresh`).
    pub fn refresh(&mut self, cx: &mut Context<Self>) -> Refresh {
        let id = self.active_tab_id();
        if let Some(find) = self.tabs.get(&id).and_then(|t| t.find.as_ref()) {
            return Refresh::Search {
                query: find.query.clone(),
                scope: find.scope.clone(),
            };
        }
        self.refreshed_by_hand.insert(id);
        Refresh::Folder(self.reload(Some(id), cx))
    }

    /// Reads the drives again (`refreshDrives`). A failure keeps the drives already known.
    #[must_use = "dropping the task cancels the read; `.detach()` it to let it run"]
    pub fn refresh_drives(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let reading = self.source.drives();
        cx.spawn(async move |this, cx| {
            if let Ok(drives) = reading.await {
                this.update(cx, |this, cx| {
                    this.drives = drives.into();
                    cx.emit(ExplorerEvent::DrivesChanged);
                    // The drives name their tabs ("Local Disk (C:)").
                    cx.notify();
                })
                .ok();
            }
        })
    }

    // ---- Prefetching ------------------------------------------------------------------------------------

    /// Reads a folder ahead: the one under the pointer (after a moment, or at once on the first click of a
    /// double-click) and the one the arrow keys land on. Opening it then shows that listing at once, as long
    /// as it's fresh: under two seconds old, and the folder's own date (which every add, delete and rename
    /// in it changes) as it was (`prefetchFolder`).
    pub fn prefetch_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if !is_folder_path(path) || in_archive_path(path) {
            return;
        }
        let key = path.to_lowercase();
        if self
            .prefetched
            .get(&key)
            .is_some_and(|known| known.at.elapsed() < PREFETCH_FRESH)
        {
            return;
        }
        let at = Instant::now();
        // What it held the last time it was read ahead stays to be peeked at while it is read again.
        let seen = self.prefetched.remove(&key).and_then(|known| match known.state {
            PrefetchState::Ready(listed) => listed,
            PrefetchState::Pending(_) => known.seen,
        });
        self.prefetched.insert(
            key.clone(),
            PrefetchEntry {
                path: path.to_string(),
                at,
                stamp: None,
                state: PrefetchState::Pending(Vec::new()),
                seen,
            },
        );
        // A few at most: they're for the next click.
        while self.prefetched.len() > PREFETCH_MOST {
            let oldest = self.prefetched.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone());
            let Some(gone) = oldest.and_then(|oldest| self.prefetched.remove(&oldest)) else {
                break;
            };
            for listed in [
                gone.seen,
                match gone.state {
                    PrefetchState::Ready(listed) => listed,
                    PrefetchState::Pending(_) => None,
                },
            ] {
                super::model::let_go(listed.map(|listed| listed.entries), cx);
            }
        }
        let source = self.source.clone();
        let path = path.to_string();
        cx.spawn(async move |this, cx| {
            let stamp = source.stamp(&path).await;
            this.update(cx, |this, _| {
                if let Some(entry) = this.prefetched.get_mut(&key).filter(|e| e.at == at) {
                    entry.stamp = stamp;
                }
            })
            .ok();
            let listed = source.list(&path).await.ok();
            this.update(cx, |this, cx| {
                let Some(entry) = this.prefetched.get_mut(&key).filter(|e| e.at == at) else {
                    return;
                };
                let stamp = entry.stamp;
                entry.seen = None;
                // Anyone waiting for it gets it now (and it's theirs: taken, so it's used once).
                if let PrefetchState::Pending(waiting) =
                    std::mem::replace(&mut entry.state, PrefetchState::Ready(listed.clone()))
                {
                    if !waiting.is_empty() {
                        for waiter in waiting {
                            let _ = waiter.try_send((stamp, listed.clone()));
                        }
                        this.prefetched.remove(&key);
                        return;
                    }
                }
                // Its row can say what is in it now.
                if listed.is_some() {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The folders read ahead and what each holds, newest read first: a folder's row says what is in it while the
    /// pointer or the keyboard is on it, before it is opened. A few at most ([`PREFETCH_MOST`]), each as it was
    /// when the pointer last came to it.
    pub fn peeks(&self) -> Vec<(&str, &Listed)> {
        let mut known: Vec<&PrefetchEntry> = self.prefetched.values().collect();
        known.sort_by_key(|entry| std::cmp::Reverse(entry.at));
        known
            .into_iter()
            .filter_map(|entry| {
                let listed = match &entry.state {
                    PrefetchState::Ready(listed) => listed.as_ref(),
                    PrefetchState::Pending(_) => entry.seen.as_ref(),
                };
                listed.map(|listed| (entry.path.as_str(), listed))
            })
            .collect()
    }

    /// Takes the listing read ahead for `path`, if there is a fresh one: it's used once.
    fn claim_prefetch(&mut self, path: &str) -> Option<Claim> {
        let key = path.to_lowercase();
        let entry = self.prefetched.get_mut(&key)?;
        if entry.at.elapsed() > PREFETCH_FRESH {
            self.prefetched.remove(&key);
            return None;
        }
        match &mut entry.state {
            PrefetchState::Pending(waiting) => {
                let (sender, receiver) = async_channel::bounded(1);
                waiting.push(sender);
                Some(Claim::Waiting(receiver))
            }
            PrefetchState::Ready(_) => {
                let entry = self.prefetched.remove(&key)?;
                let PrefetchState::Ready(listed) = entry.state else {
                    return None;
                };
                Some(Claim::Ready(entry.stamp, listed))
            }
        }
    }

    /// The pointer rests on a folder: read it ahead after a moment (`hoverFolder`).
    pub fn hover_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = path.to_string();
        // Replacing the task cancels the older wait, as clearTimeout did.
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_FOR).await;
            this.update(cx, |this, cx| this.prefetch_folder(&path, cx)).ok();
        }));
    }

    /// The pointer left the folder (`leaveFolder`).
    pub fn leave_folder(&mut self) {
        self.hover_task = None;
    }
}

/// Reads a folder: the listing read ahead if it's still as the folder is, else a fresh one.
async fn read_folder(
    source: &dyn super::source::Source,
    target: &str,
    claim: Option<Claim>,
) -> Result<Listed, ListFailure> {
    if let Some(claim) = claim {
        let (stamp, listed) = match claim {
            Claim::Ready(stamp, listed) => (stamp, listed),
            Claim::Waiting(receiver) => receiver.recv().await.unwrap_or((None, None)),
        };
        if let (Some(listed), Some(then)) = (listed, stamp) {
            // Good only while the folder's own date is what it was when the read began.
            if source.stamp(target).await == Some(then) {
                return Ok(listed);
            }
        }
    }
    source.list(target).await
}

/// How a navigation inside an async task ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Walk {
    /// The folder opened.
    Opened,
    /// It couldn't be listed (or the tab is gone).
    Failed,
    /// A newer navigation in the same tab took over while it was under way.
    Superseded,
}

/// A navigation started on behalf of a walk (opening a tab's folder, then its parents), to be waited for.
pub(super) struct Walking {
    task: Task<bool>,
    id: TabId,
    /// The tab's navigation number the walk's own took.
    seq: Option<u64>,
}

impl Walking {
    /// Waits for it.
    pub async fn finish(self, this: &WeakEntity<Explorer>, cx: &mut AsyncApp) -> Walk {
        if self.task.await {
            return Walk::Opened;
        }
        // NOTE(port): the TypeScript's `openTab` and `reload` walked up to the parent folder whenever
        // `navigate` said false, even when that was because the person had meanwhile opened something else in
        // the tab; the walk then took the tab away from them. Here a walk stops when the tab's newest
        // navigation isn't its own.
        match this.update(cx, |this, _| this.tabs.get(&self.id).map(|t| t.nav_seq)) {
            Ok(now) if now == self.seq => Walk::Failed,
            Ok(_) => Walk::Superseded,
            Err(_) => Walk::Failed,
        }
    }
}

impl Explorer {
    /// Starts opening `target` in a tab now (the tab's navigation number is taken at once, as an `async`
    /// function's first step runs at once in the TypeScript), to wait for with [`Walking::finish`].
    pub(super) fn begin_walk(
        &mut self,
        id: TabId,
        target: String,
        options: NavOptions,
        cx: &mut Context<Self>,
    ) -> Walking {
        let options = NavOptions {
            tab: Some(id),
            ..options
        };
        let task = self.navigate_task(target, options, cx);
        Walking {
            task,
            id,
            seq: self.tabs.get(&id).map(|t| t.nav_seq),
        }
    }
}

/// Opens a folder from inside an async task and waits for the answer.
pub(super) async fn navigate_in(
    this: &WeakEntity<Explorer>,
    cx: &mut AsyncApp,
    target: String,
    options: NavOptions,
) -> Walk {
    let started = this.update(cx, |this, cx| {
        let id = options.tab.unwrap_or_else(|| this.active_tab_id());
        this.begin_walk(id, target, options, cx)
    });
    match started {
        Ok(walking) => walking.finish(this, cx).await,
        Err(_) => Walk::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::entries::{with_details, RawEntry};

    fn entry(path: &str) -> glint_model::entries::FileEntry {
        let (dir, name) = path.rsplit_once('\\').unwrap_or(("", path));
        with_details(
            RawEntry {
                name: name.into(),
                ..Default::default()
            },
            dir,
        )
    }

    #[test]
    fn selection_finds_paths_ignoring_case_and_trailing_separators() {
        let entries = [entry("C:\\Docs\\Report.TXT"), entry("C:\\Docs\\other.txt")];
        let found = match_selection(
            &entries,
            &["c:\\docs\\report.txt".into(), "C:\\Docs\\other.txt\\".into()],
        );
        // The path as the listing spells it, in the order asked.
        assert_eq!(found, ["C:\\Docs\\Report.TXT", "C:\\Docs\\other.txt"]);
        assert!(match_selection(&entries, &["C:\\Docs\\missing.txt".into()]).is_empty());
    }

    #[test]
    fn failures_are_told_by_their_message() {
        let select = None;
        assert_eq!(failure_notice("cancelled", "C:\\a.zip", TabId(1), &select), None);
        assert!(matches!(
            failure_notice("It is password-protected", "C:\\a.zip", TabId(1), &select),
            Some(Notice::ArchivePassword { .. })
        ));
        assert!(matches!(
            failure_notice("7-Zip isn't installed", "C:\\a.iso", TabId(1), &select),
            Some(Notice::NeedsSevenZip { .. })
        ));
        assert!(matches!(
            failure_notice("Access is denied", "C:\\x", TabId(1), &select),
            Some(Notice::CannotOpen { .. })
        ));
    }
}

/// The notice for an archive read whole: shown after a moment, kept up to date, stoppable, and gone when the
/// folder is.
#[cfg(test)]
mod reading_tests {
    use super::*;
    use crate::explorer::fake::{file, FakeDisk};
    use crate::startup::ReadingSink;
    use crate::testing;
    use gpui_kit::TestAppContext;
    use std::cell::RefCell;
    use std::sync::Mutex;

    const ARCHIVE: &str = r"C:\big.tar.gz";

    /// An explorer on a fake disk whose `big.tar.gz` takes its time (`gate.close()` ends it), a probe that says
    /// what `reads` holds, and every note the notice was told.
    struct Rig {
        explorer: gpui_kit::Entity<Explorer>,
        gate: async_channel::Sender<()>,
        reads: Arc<Mutex<Option<ArchiveRead>>>,
        cancelled: Arc<Mutex<Vec<String>>>,
        notes: Rc<RefCell<Vec<ReadingNote>>>,
    }

    fn rig(cx: &mut TestAppContext) -> Rig {
        cx.update(testing::init);
        let disk = FakeDisk::new();
        disk.folder(ARCHIVE, vec![file(ARCHIVE, "inside.txt", 1, 1)]);
        let gate = disk.hold(ARCHIVE);
        let reads = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(Mutex::new(Vec::new()));
        let notes = Rc::new(RefCell::new(Vec::new()));
        let (asked, stopped, heard) = (reads.clone(), cancelled.clone(), notes.clone());
        cx.update(|cx| {
            cx.set_global(ArchiveProbe {
                progress: Arc::new(move |_| asked.lock().unwrap().clone()),
                cancel: Arc::new(move |path| stopped.lock().unwrap().push(path.to_string())),
            });
            cx.set_global(ReadingSink(Rc::new(move |note| heard.borrow_mut().push(note.clone()))));
        });
        let explorer = cx.update(|cx| cx.new(|cx| Explorer::new(disk.source(), cx)));
        Rig {
            explorer,
            gate,
            reads,
            cancelled,
            notes,
        }
    }

    impl Rig {
        fn navigate(&self, cx: &mut TestAppContext) -> Task<bool> {
            self.explorer
                .update(cx, |e, cx| e.navigate_task(ARCHIVE, NavOptions::default(), cx))
        }

        fn wait(&self, cx: &mut TestAppContext, millis: u64) {
            cx.executor().advance_clock(Duration::from_millis(millis));
            cx.run_until_parked();
        }

        fn reading(&self, done: u64, total: u64) {
            *self.reads.lock().unwrap() = Some(ArchiveRead {
                archive: ARCHIVE.to_string(),
                done,
                total,
            });
        }
    }

    fn progress(done: u64, total: u64) -> ReadingNote {
        ReadingNote::Progress {
            target: ARCHIVE.to_string(),
            archive: ARCHIVE.to_string(),
            done,
            total,
        }
    }

    #[gpui_kit::test]
    fn a_folder_that_takes_a_moment_says_how_far_the_archive_read_has_come(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx);
        let opening = rig.navigate(cx);
        // Not before 400 ms: a quick folder never shows it.
        rig.reading(10, 100);
        rig.wait(cx, 300);
        assert!(rig.notes.borrow().is_empty());
        rig.wait(cx, 150);
        assert_eq!(*rig.notes.borrow(), [progress(10, 100)]);
        // Then every 250 ms, with where it has got to.
        rig.reading(60, 100);
        rig.wait(cx, 250);
        assert_eq!(rig.notes.borrow().last(), Some(&progress(60, 100)));
        // The folder arrives: the notice goes.
        rig.gate.close();
        assert!(cx.foreground_executor().block_test(opening));
        assert_eq!(
            rig.notes.borrow().last(),
            Some(&ReadingNote::Ended {
                target: ARCHIVE.to_string(),
                shown: true
            })
        );
        // And nothing is looked at afterwards.
        let told = rig.notes.borrow().len();
        rig.wait(cx, 1000);
        assert_eq!(rig.notes.borrow().len(), told);
    }

    #[gpui_kit::test]
    fn a_slow_folder_that_is_not_an_archive_read_shows_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx);
        let opening = rig.navigate(cx);
        // Core has no read of an archive going: nothing to say, however long it takes.
        rig.wait(cx, 2000);
        rig.gate.close();
        assert!(cx.foreground_executor().block_test(opening));
        assert!(rig.notes.borrow().is_empty());
    }

    #[gpui_kit::test]
    fn stop_cancels_the_read_and_the_notice_is_not_shown_again_meanwhile(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let rig = rig(cx);
        let opening = rig.navigate(cx);
        rig.reading(10, 100);
        rig.wait(cx, 450);
        assert_eq!(rig.notes.borrow().len(), 1);
        // Stop (the notice's button): core is asked to stop reading, whatever case the path is in.
        rig.explorer
            .update(cx, |e, cx| e.stop_archive_reading(r"C:\BIG.tar.gz", cx));
        cx.run_until_parked();
        assert_eq!(*rig.cancelled.lock().unwrap(), [r"C:\BIG.tar.gz"]);
        // The read winds down slowly; the notice does not come back.
        rig.reading(20, 100);
        rig.wait(cx, 1000);
        assert_eq!(rig.notes.borrow().len(), 1);
        // Its listing then fails with "cancelled", which says nothing, and the next time starts clean.
        rig.gate.close();
        let _ = cx.foreground_executor().block_test(opening);
        assert_eq!(rig.notes.borrow().len(), 1, "the notice was taken down by Stop itself");
        cx.update(|cx| assert!(cx.global::<ReadingStops>().0.is_empty()));
    }

    #[gpui_kit::test]
    fn the_core_probe_reads_nothing_where_no_archive_is_being_read(_cx: &mut TestAppContext) {
        let probe = ArchiveProbe::core();
        assert_eq!((probe.progress)(r"C:\nothing-is-reading-this.tar.gz"), None);
        // Cancelling a read that is not there does nothing.
        (probe.cancel)(r"C:\nothing-is-reading-this.tar.gz");
    }
}
