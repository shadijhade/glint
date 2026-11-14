//! Searching by name: Enter in the search box, searching as you type, Stop, leaving the results, saved searches.
//! Ported from the "Search" part of src/lib/explorer.ts (`runSavedSearch`, `findScope`, `searchedDrives`,
//! `findsAsTyped`, `runFind`, `offerIndex`, `stopFind`, `leaveFind`, `clearFind`, `indexed`) and from what they
//! called in src/lib/fs.ts (`findFiles`, `cancelFind`).
//!
//! # How a search runs
//!
//! [`Explorer::run_find`] puts a [`FindRun`] in the tab (the file list then draws its states: searching,
//! nothing found, failed, the results) and asks the [`Source`] to start the search. In the app that is glint-core's
//! `find_files`: the instant search index when its service runs and has the drives, else the bundled fd, streaming
//! results as they come. Each tab searches in its own lane (its id), so a newer search in a tab ends that tab's
//! older one and no other tab's.
//!
//! The stream is read by a GPUI task kept in the explorer, one per tab. What has come is added to the run at most
//! once per wake of the task (everything waiting is taken in one go), and, so a search of 50,000 names doesn't
//! copy the list again for every 40 ms batch, once the list is long a batch that would grow it by less than a
//! quarter waits for the next one (the end always shows everything). Each result carries a number: a stream's
//! answer for a search that was replaced or left is dropped.
//!
//! A search ends when a newer one starts in the tab, the tab goes to another folder, the tab is closed, New
//! folder or New file puts something in the folder, [`Explorer::stop_find`], [`Explorer::leave_find`] or
//! [`Explorer::clear_find`]. Each of those but the first tells the source to cancel the lane, as `cancelFind` did.
//!
//! # What isn't kept
//!
//! The TypeScript kept no history of name searches. The search history in Settings (`addToHistory`) is the
//! text search's, which is another part's.

use super::find_types::*;
use super::model::{Explorer, Prefs};
use super::tab::Selection;
use super::types::*;
use glint_model::entries::{terms, FileEntry};
use glint_model::paths::{is_recycle_bin, THIS_PC};
use glint_model::store::SavedSearch;
use gpui_kit::{App, Context, Task};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Once this many results show, a batch that would grow the list by less than a quarter waits for the next one.
const WORTH_WAITING_FROM: usize = 2_000;
/// Where instant search stands is asked again when it was last read this long ago and a search as you type asks.
const INDEX_FRESH: Duration = Duration::from_secs(30);

/// The explorer's searching state.
#[derive(Default)]
pub(super) struct Searching {
    /// The numbers searches get (`findSeq`).
    seq: u64,
    /// The task reading each tab's search stream; dropping it stops reading.
    jobs: HashMap<TabId, Task<()>>,
    /// Where instant search stands, and when that was read.
    index: Option<(SearchIndex, Instant)>,
    index_reading: bool,
    /// The "instant search finds names in milliseconds" notice was raised in this run of the app.
    offered: bool,
    /// The search inside files that runs (`inside.rs`): core has one at a time.
    pub(super) inside: Option<super::inside::InsideJob>,
    /// The numbers content searches get.
    pub(super) inside_serial: u64,
    /// Core's events to the pump that reads them, once a content search has run.
    pub(super) inside_events: Option<async_channel::Sender<super::inside::RawEvent>>,
    /// Events handed to the pump that it hasn't applied yet.
    pub(super) inside_waiting: usize,
}

/// What a stream has brought that is not yet part of the run.
struct Progress {
    /// The run still shows the older search's results (same folder searched again): the first results replace them.
    kept: bool,
    pending: Vec<FileEntry>,
}

/// Which search a tab runs in core: the tab's own lane.
fn lane(tab: TabId) -> String {
    tab.to_string()
}

fn no_entries() -> Arc<[FileEntry]> {
    Arc::from(Vec::<FileEntry>::new())
}

/// `w` without a leading `path:` (any case) and without a leading `\\?\`.
fn bare_term(word: &str) -> &str {
    let word = match word.get(..5) {
        Some(head) if head.eq_ignore_ascii_case("path:") => &word[5..],
        _ => word,
    };
    word.strip_prefix("\\\\?\\").unwrap_or(word)
}

/// Whether `text` starts with a drive letter, a colon and a separator ("D:\", "d:/").
fn starts_with_drive(text: &str) -> bool {
    let mut chars = text.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('\\' | '/')) if letter.is_ascii_alphabetic()
    )
}

/// Where a search from the folder `path` looks: there, unless an absolute drive path typed outside it names another
/// (`findScope`).
pub(super) fn find_scope(query: &str, path: &str) -> String {
    if path == THIS_PC {
        return path.to_string();
    }
    let all = terms(query);
    let Some(term) = all.iter().map(|w| bare_term(w)).find(|w| starts_with_drive(w)) else {
        return path.to_string();
    };
    let typed = term.replace('/', "\\").to_lowercase();
    let here = path.trim_end_matches(['\\', '/']).to_lowercase();
    if typed.starts_with(&here) {
        return path.to_string();
    }
    // The drive letter was checked above, so the first character is a letter.
    let letter = typed.chars().next().map(|c| c.to_ascii_uppercase()).unwrap_or('C');
    format!("{letter}:\\")
}

impl Explorer {
    // ---- Instant search ----------------------------------------------------------------------------------

    /// Where instant search stood when last read, if it has been ([`Explorer::refresh_search_index`]).
    pub fn search_index(&self) -> Option<&SearchIndex> {
        self.searching.index.as_ref().map(|(index, _)| index)
    }

    /// Tells the explorer where instant search stands (the status bar's poll can hand over what it learns).
    pub fn set_search_index(&mut self, index: Option<SearchIndex>, cx: &mut Context<Self>) {
        let was = self.search_index().cloned();
        // A folder on screen whose drive just became ready shows its sizes without waiting for a navigation.
        let before = self.indexed_on_screen();
        self.searching.index = index.clone().map(|index| (index, Instant::now()));
        if was != index {
            cx.notify();
            self.sizes_after_index(before, cx);
        }
    }

    /// Asks the source how instant search stands, in the background. A question under way isn't repeated; a
    /// question that gets no answer keeps what was known.
    #[must_use = "dropping the task stops waiting for the answer; `.detach()` it to let it run"]
    pub fn refresh_search_index(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if std::mem::replace(&mut self.searching.index_reading, true) {
            return Task::ready(());
        }
        let asking = self.source.search_index();
        cx.spawn(async move |this, cx| {
            let answer = asking.await;
            this.update(cx, |this, cx| {
                this.searching.index_reading = false;
                if answer.is_some() {
                    this.set_search_index(answer, cx);
                }
            })
            .ok();
        })
    }

    /// The drives a search from This PC looks through (`searchedDrives`).
    fn searched_drives(&self) -> impl Iterator<Item = &Drive> {
        self.drives
            .iter()
            .filter(|d| matches!(d.kind, DriveKind::Fixed | DriveKind::Removable))
    }

    /// Whether the index has the drive `path` is on, ready to answer (`indexed`).
    pub(super) fn indexed(&self, path: &str) -> bool {
        self.search_index().is_some_and(|index| index.indexed(path))
    }

    // ---- Searching as you type -------------------------------------------------------------------------

    /// Whether a search for `query` from `path` is the index's alone, answered in milliseconds, so it can run as it's
    /// typed (`findsAsTyped`). fd's walks, .gitignore rules and what photos, videos and songs say (read from each
    /// file) wait for Enter, while typing filters what's shown; so do filters and paths while the service is older
    /// than the app, as fd answers them until it's updated. Until where instant search stands is known (see
    /// [`Explorer::refresh_search_index`]) nothing is the index's alone.
    pub fn finds_as_typed(&self, query: &str, path: &str, cx: &App) -> bool {
        if Prefs::one(cx, |s| s.find_respect_ignore) || glint_model::entries::has_prop_filter(query) {
            return false;
        }
        let outdated = self.search_index().is_some_and(|index| index.outdated);
        if outdated && glint_model::entries::has_filter_or_path(query) {
            return false;
        }
        let scope = find_scope(query, path);
        if scope == THIS_PC {
            self.searched_drives().all(|d| self.indexed(&d.path))
        } else {
            self.indexed(&scope)
        }
    }

    /// The search box paused on `query`: searches the open folder's subfolders if [`Explorer::finds_as_typed`] says
    /// the index answers at once (the address bar's `FindAsTyped`). Where instant search stands is read first when
    /// it isn't known, or was last read long ago.
    #[must_use = "dropping the task stops waiting for the search; `.detach()` it to let it run"]
    pub fn find_as_typed(&mut self, query: &str, cx: &mut Context<Self>) -> Task<()> {
        let tab = self.active_tab_id();
        let Some(path) = self.tabs.get(&tab).map(|t| t.path.clone()) else {
            return Task::ready(());
        };
        let known = self.searching.index.as_ref().map(|(_, at)| at.elapsed() < INDEX_FRESH);
        if known == Some(false) {
            self.refresh_search_index(cx).detach();
        }
        if known.is_some() {
            return if self.finds_as_typed(query, &path, cx) {
                self.find_typed(tab, query, cx)
            } else {
                Task::ready(())
            };
        }
        let asking = self.refresh_search_index(cx);
        let query = query.to_string();
        cx.spawn(async move |this, cx| {
            asking.await;
            // Still the same box, in the same folder: the person hasn't moved on while the service answered.
            let running = this
                .update(cx, |this, cx| {
                    let same = this.tabs.get(&tab).is_some_and(|t| t.path == path && t.filter == query);
                    (same && this.finds_as_typed(&query, &path, cx)).then(|| this.find_typed(tab, &query, cx))
                })
                .ok()
                .flatten();
            if let Some(running) = running {
                running.await;
            }
        })
    }

    /// A search as it is typed: the names at once, and what is inside the files too where the content index
    /// covers the folder (it answers in milliseconds, as the name index does; ripgrep waits for Enter).
    fn find_typed(&mut self, tab: TabId, query: &str, cx: &mut Context<Self>) -> Task<()> {
        let running = self.run_find_in(tab, query, Wider::default().names_only(), cx);
        let run = self.tabs.get(&tab).and_then(|t| t.find.as_ref());
        let asked = run
            .filter(|find| !super::inside::inside_words(&find.query).is_empty())
            .filter(|find| super::inside::inside_unasked(&find.scope))
            .map(|find| (find.id, find.scope.clone()));
        if let Some((id, scope)) = asked {
            let covers = self.source.inside_covers(&scope);
            cx.spawn(async move |this, cx| {
                if !covers.await {
                    return;
                }
                this.update(cx, |this, cx| {
                    // Still the same search: the person hasn't typed on while the index answered.
                    let same = this
                        .tabs
                        .get(&tab)
                        .and_then(|t| t.find.as_ref())
                        .is_some_and(|f| f.id == id);
                    if same {
                        this.search_inside(tab, &Wider::default(), cx);
                    }
                })
                .ok();
            })
            .detach();
        }
        running
    }

    // ---- Running a search ---------------------------------------------------------------------------------

    /// Searches the open folder and its subfolders (or every drive, from This PC) by name or path: the index, else
    /// fd (`runFind`). An empty query leaves the results for the folder ([`Explorer::clear_find`]). The tab shows
    /// the results as they come; the task resolves when the search ends (or is replaced or left).
    ///
    /// `wider` searches more widely than Settings say, once: elsewhere, without the limit, or skipping nothing.
    #[must_use = "dropping the task stops waiting for the search, not the search; `.detach()` it to let it run"]
    pub fn run_find(&mut self, query: &str, wider: Wider, cx: &mut Context<Self>) -> Task<()> {
        let tab = self.active_tab_id();
        self.run_find_in(tab, query, wider, cx)
    }

    /// [`Explorer::run_find`] in a particular tab.
    #[must_use = "dropping the task stops waiting for the search, not the search; `.detach()` it to let it run"]
    pub fn run_find_in(&mut self, tab: TabId, query: &str, wider: Wider, cx: &mut Context<Self>) -> Task<()> {
        if query.trim().is_empty() {
            self.clear_find_in(tab, cx);
            return Task::ready(());
        }
        let Some(t) = self.tabs.get(&tab) else {
            return Task::ready(());
        };
        let prefs = Prefs::read(cx);
        let scope = wider.scope.clone().unwrap_or_else(|| find_scope(query, &t.path));
        // What's in the Recycle Bin isn't in folders to search: the box filters it as it's typed.
        if is_recycle_bin(&scope) {
            cx.emit(ExplorerEvent::Find(FindNotice::NothingBelowRecycleBin));
            return Task::ready(());
        }
        self.searching.seq += 1;
        let id = self.searching.seq;
        let roots: Vec<String> = if scope == THIS_PC {
            self.searched_drives().map(|d| d.path.clone()).collect()
        } else {
            vec![scope.clone()]
        };
        let respect_ignore = prefs.find_respect_ignore && !wider.skip_nothing;
        let exclude = if wider.skip_nothing {
            Vec::new()
        } else {
            prefs.find_exclusions.clone()
        };
        let options = FindOptions {
            hidden: prefs.show_hidden,
            respect_ignore,
            max_results: if wider.unlimited {
                None
            } else {
                prefs.find_max_results.and_then(|max| usize::try_from(max).ok())
            },
            // Typos allowed when nothing matches as typed; past the limit, the best matches are what's kept.
            fuzzy: true,
            rank: true,
            exclude: exclude.clone(),
        };
        let mut skipped = exclude;
        if respect_ignore {
            skipped.push("what .gitignore files list".to_string());
        }
        // The same folder searched again keeps showing the last results until the first new ones come.
        let before = t.find.as_ref().filter(|f| f.scope == scope);
        let entries = before.map_or_else(no_entries, |f| f.entries.clone());
        let engine = t.find.as_ref().map(|f| f.engine.clone()).unwrap_or_default();
        let kept = !entries.is_empty();
        // What is inside the files: searched too when the search has words for it, unasked below a drive's top.
        let words = super::inside::inside_words(query);
        let can_inside = !words.is_empty() && super::inside::can_search_inside(&scope);
        let inside_now = can_inside && !wider.names_only && (wider.inside || super::inside::inside_unasked(&scope));
        let inside = Inside {
            words: if can_inside { words } else { String::new() },
            offered: can_inside && !inside_now && !wider.names_only,
            ..Default::default()
        };
        // The older search's content search (this tab's) ends with it.
        self.inside_left(tab);
        self.update_tab(tab, |t| {
            t.find = Some(FindRun {
                id,
                query: query.to_string(),
                engine,
                scope,
                entries,
                running: true,
                elapsed: 0.0,
                limited: false,
                fuzzy: false,
                error: String::new(),
                skipped,
                inside,
            });
            t.selection = Selection::default();
            t.renaming = None;
        });
        if self.resolve_view(tab, cx) {
            cx.emit(ExplorerEvent::ViewChanged(tab));
        }
        self.rebuild_rows(tab, cx);
        cx.emit(ExplorerEvent::SelectionChanged(tab));
        self.tab_changed(tab, cx);

        let started = Instant::now();
        let starting = self.source.find(&lane(tab), roots, query.to_string(), options);
        let (finished, wait) = async_channel::bounded::<()>(1);
        let reading = cx.spawn(async move |this, cx| {
            // Dropped when the reading ends or is cancelled, which is what the returned task waits for.
            let _finished = finished;
            let stream = match starting.await {
                Ok(stream) => stream,
                Err(message) => {
                    this.update(cx, |this, cx| this.find_failed(tab, id, message, cx)).ok();
                    return;
                }
            };
            let mut progress = Progress {
                kept,
                pending: Vec::new(),
            };
            while let Ok(first) = stream.recv().await {
                let mut batch = vec![first];
                while let Ok(more) = stream.try_recv() {
                    batch.push(more);
                }
                let ended = this
                    .update(cx, |this, cx| this.found(tab, id, started, batch, &mut progress, cx))
                    .unwrap_or(true);
                if ended {
                    return;
                }
            }
            // The stream closed without saying it finished (core's search thread went): what came is what there is.
            let closed = vec![Found::Finished {
                count: 0,
                elapsed: 0.0,
                limited: false,
                fuzzy: false,
            }];
            this.update(cx, |this, cx| this.found(tab, id, started, closed, &mut progress, cx))
                .ok();
        });
        // A newer search in the tab replaces this job, which stops reading; core ends the older search itself.
        self.searching.jobs.insert(tab, reading);
        if inside_now {
            self.search_inside(tab, &wider, cx);
        }
        cx.spawn(async move |_, _| {
            let _ = wait.recv().await;
        })
    }

    /// F5 with results showing: the same search again, where it was run (`refresh`'s `runFind(find.query, { scope })`).
    /// Nothing if the active tab shows no search.
    #[must_use = "dropping the task stops waiting for the search, not the search; `.detach()` it to let it run"]
    pub fn rerun_find(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let Some(find) = self.active_tab().and_then(|t| t.find.as_ref()) else {
            return Task::ready(());
        };
        let (query, scope) = (find.query.clone(), find.scope.clone());
        self.run_find(&query, Wider::in_scope(scope), cx)
    }

    /// A saved search from the sidebar: its folder opens in this tab, with its query in the box, searched
    /// (`runSavedSearch`). Does nothing more when the folder can't be opened.
    #[must_use = "dropping the task stops waiting for the search; `.detach()` it to let it run"]
    pub fn run_saved_search(&mut self, saved: SavedSearch, cx: &mut Context<Self>) -> Task<()> {
        let tab = self.active_tab_id();
        let SavedSearch { query, folder } = saved;
        let opening = self.navigate_task(folder, NavOptions::in_tab(tab), cx);
        cx.spawn(async move |this, cx| {
            if !opening.await {
                return;
            }
            let running = this
                .update(cx, |this, cx| {
                    this.set_filter(&query, Some(tab), cx);
                    this.run_find_in(tab, &query, Wider::default(), cx)
                })
                .ok();
            if let Some(running) = running {
                running.await;
            }
        })
    }

    // ---- What the stream says ----------------------------------------------------------------------------

    /// A search could not start or failed outright (`catch` in `runFind`): its error shows, with nothing found.
    fn find_failed(&mut self, tab: TabId, id: u64, message: String, cx: &mut Context<Self>) {
        let ours = self
            .tabs
            .get(&tab)
            .and_then(|t| t.find.as_ref())
            .is_some_and(|f| f.id == id);
        if !ours {
            return;
        }
        // A search that failed shows its error alone: what was found inside files goes with it.
        self.inside_left(tab);
        self.update_tab(tab, |t| {
            if let Some(find) = &mut t.find {
                find.running = false;
                find.error = message;
                find.entries = no_entries();
                find.inside = Inside::default();
            }
        });
        self.rebuild_rows(tab, cx);
        cx.emit(ExplorerEvent::TabChanged(tab));
        cx.notify();
    }

    /// Applies what a search said to its run, if the run is still the tab's. Returns whether the stream is done:
    /// the search finished, or this is no longer the tab's search.
    fn found(
        &mut self,
        tab: TabId,
        id: u64,
        started: Instant,
        batch: Vec<Found>,
        progress: &mut Progress,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(mut run) = self
            .tabs
            .get(&tab)
            .and_then(|t| t.find.as_ref())
            .filter(|f| f.id == id)
            .cloned()
        else {
            return true;
        };
        let mut ended = false;
        let mut slow_without_index = false;
        for event in batch {
            match event {
                Found::Started { engine } => run.engine = engine,
                Found::Entries(mut entries) => {
                    if std::mem::replace(&mut progress.kept, false) {
                        drop_kept(&mut run);
                    }
                    progress.pending.append(&mut entries);
                }
                Found::Error(message) => {
                    run.error = message;
                    run.entries = no_entries();
                    run.inside = Inside::default();
                    progress.pending.clear();
                }
                Found::Finished { limited, fuzzy, .. } => {
                    ended = true;
                    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
                    add_pending(&mut run, progress);
                    // Nothing came at all: the older results (if they were kept) are not this search's.
                    if std::mem::replace(&mut progress.kept, false) {
                        drop_kept(&mut run);
                    }
                    run.running = false;
                    run.elapsed = elapsed;
                    run.limited = limited;
                    run.fuzzy = fuzzy;
                    slow_without_index = elapsed > 1000.0 && run.engine == "fd";
                }
            }
        }
        if !ended && !progress.pending.is_empty() {
            let shown = run.entries.len();
            let worth_waiting = shown >= WORTH_WAITING_FROM && progress.pending.len() * 4 < shown;
            if !worth_waiting {
                add_pending(&mut run, progress);
            }
        }
        let failed = !run.error.is_empty();
        self.update_tab(tab, |t| t.find = Some(run));
        if failed {
            self.inside_left(tab);
        }
        self.rebuild_rows(tab, cx);
        // Not `tab_changed`: nothing the session keeps changed, and this runs for every batch.
        cx.emit(ExplorerEvent::TabChanged(tab));
        cx.notify();
        if slow_without_index {
            self.offer_instant_search(cx);
        }
        ended
    }

    /// After a slow search without the index: what it would have been with it, once (`offerIndex`). Raised only when
    /// instant search can be turned on (not installed, or stopped).
    fn offer_instant_search(&mut self, cx: &mut Context<Self>) {
        if self.searching.offered {
            return;
        }
        let asking = self.refresh_search_index(cx);
        cx.spawn(async move |this, cx| {
            asking.await;
            this.update(cx, |this, cx| {
                let can_turn_on = this.search_index().is_some_and(|index| index.can_turn_on);
                if can_turn_on && !std::mem::replace(&mut this.searching.offered, true) {
                    cx.emit(ExplorerEvent::Find(FindNotice::OfferInstantSearch));
                }
            })
            .ok();
        })
        .detach();
    }

    // ---- Ending a search ---------------------------------------------------------------------------------

    /// Stops reading a tab's search, and tells the source to end it if it was running. The tab's run isn't
    /// touched: the caller says what becomes of it.
    pub(super) fn stop_find_job(&mut self, tab: TabId, was_running: bool) {
        self.searching.jobs.remove(&tab);
        self.inside_left(tab);
        if was_running {
            self.source.cancel_find(&lane(tab));
        }
    }

    /// Stop (`stopFind`): the active tab's running search ends, keeping what it found. The stream still brings its
    /// last words, which mark the run as no longer running. (What runs inside the files is stopped with
    /// [`Explorer::stop_inside`].)
    pub fn stop_find(&mut self) {
        let tab = self.active_tab_id();
        if self
            .tabs
            .get(&tab)
            .and_then(|t| t.find.as_ref())
            .is_some_and(|f| f.running)
        {
            self.source.cancel_find(&lane(tab));
        }
    }

    /// Leaves the results (stopping a search that runs) while keeping whatever text is in the filter box, and
    /// shows the folder again (`leaveFind`). The tab is the active one's by default.
    pub fn leave_find(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) {
        let id = tab.unwrap_or_else(|| self.active_tab_id());
        let Some(running) = self.tabs.get(&id).and_then(|t| t.find.as_ref()).map(|f| f.running) else {
            return;
        };
        self.stop_find_job(id, running);
        self.update_tab(id, |t| {
            t.find = None;
            t.selection = Selection::default();
        });
        self.after_find_ended(id, cx);
        self.reload(Some(id), cx).detach();
    }

    /// Leaves the results (and the filter) for the folder itself (`clearFind`), in the active tab.
    pub fn clear_find(&mut self, cx: &mut Context<Self>) {
        let tab = self.active_tab_id();
        self.clear_find_in(tab, cx);
    }

    /// [`Explorer::clear_find`] in a particular tab.
    pub fn clear_find_in(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(t) = self.tabs.get(&tab) else {
            return;
        };
        let find = t.find.as_ref().map(|f| f.running);
        if find.is_none() && t.filter.is_empty() {
            return;
        }
        if let Some(running) = find {
            self.stop_find_job(tab, running);
        }
        self.update_tab(tab, |t| {
            t.filter.clear();
            if t.find.take().is_some() {
                t.selection = Selection::default();
            }
        });
        self.after_find_ended(tab, cx);
        if find.is_some() {
            self.reload(Some(tab), cx).detach();
        }
    }

    /// The tab shows its folder again, not a search: its sort, columns and rows follow.
    fn after_find_ended(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if self.resolve_view(tab, cx) {
            cx.emit(ExplorerEvent::ViewChanged(tab));
        }
        self.rebuild_rows(tab, cx);
        cx.emit(ExplorerEvent::SelectionChanged(tab));
        self.tab_changed(tab, cx);
    }
}

/// Adds what has come to the run's results.
fn add_pending(run: &mut FindRun, progress: &mut Progress) {
    if progress.pending.is_empty() {
        return;
    }
    // A file found by what is in it has its row already: found by its name too, it is no longer "inside only".
    if !run.inside.only.is_empty() {
        let mut only = (*run.inside.only).clone();
        progress.pending.retain(|entry| !only.remove(&entry.path));
        if only.len() != run.inside.only.len() {
            run.inside.only = Arc::new(only);
        }
    }
    let mut all = Vec::with_capacity(run.entries.len() + progress.pending.len());
    all.extend(run.entries.iter().cloned());
    all.append(&mut progress.pending);
    run.entries = all.into();
}

/// The older search's results, kept on screen until this one's first came, go: all but the files this search has
/// found by their contents meanwhile.
fn drop_kept(run: &mut FindRun) {
    if run.inside.only.is_empty() {
        run.entries = no_entries();
        return;
    }
    let only = run.inside.only.clone();
    let ours: Vec<FileEntry> = run.entries.iter().filter(|e| only.contains(&e.path)).cloned().collect();
    run.entries = ours.into();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_looks_where_the_tab_is_unless_a_drive_path_typed_names_another_place() {
        // The TypeScript's findScope.
        assert_eq!(find_scope("report", "C:\\Users"), "C:\\Users");
        assert_eq!(find_scope("report", THIS_PC), THIS_PC);
        assert_eq!(
            find_scope("D:\\photos", THIS_PC),
            THIS_PC,
            "This PC already searches every drive"
        );
        // Inside the folder: stays.
        assert_eq!(find_scope("c:\\users\\me\\docs", "C:\\Users"), "C:\\Users");
        assert_eq!(find_scope("path:C:/Users/me", "C:\\Users\\"), "C:\\Users\\");
        // Outside it: the typed path's drive.
        assert_eq!(find_scope("D:\\photos", "C:\\Users"), "D:\\");
        assert_eq!(find_scope("holiday path:d:/pics", "C:\\Users"), "D:\\");
        assert_eq!(find_scope("\\\\?\\e:\\x", "C:\\Users"), "E:\\");
        // Not a drive path: a relative one, a quoted space, a scheme-like word.
        assert_eq!(find_scope("src\\lib", "C:\\Users"), "C:\\Users");
        assert_eq!(find_scope("http://x", "C:\\Users"), "C:\\Users");
        assert_eq!(
            find_scope("ا:\\x", "C:\\Users"),
            "C:\\Users",
            "only a Latin letter is a drive"
        );
    }

    #[test]
    fn a_path_prefix_only_comes_off_when_it_is_there_in_any_case() {
        assert_eq!(bare_term("PATH:d:\\x"), "d:\\x");
        assert_eq!(bare_term("pathology"), "pathology");
        assert_eq!(bare_term("\\\\?\\c:\\x"), "c:\\x");
        assert_eq!(bare_term("ملف"), "ملف", "short or non-ASCII words don't panic");
        assert_eq!(bare_term("pat"), "pat");
    }
}
