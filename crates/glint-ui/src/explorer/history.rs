//! Back and Forward: a tab's history of places, each kept as it was left.
//!
//! A place is a folder (or This PC) or a search's results. Each [`Visit`] keeps what the tab showed there: the
//! search box's text, what was selected and which item the keyboard was on, how far the list was scrolled, and
//! for a search its results. Back and Forward (Alt+Left and Alt+Right, the mouse's side buttons, the toolbar's
//! arrows and their menus, Backspace) bring a place back as it was: a search's results come back at once, with
//! the folder you opened from them selected.
//!
//! # How searches fit in
//!
//! - A search started in a folder is a place of its own, after the folder: Back from the results is the folder
//!   as it was before the search; Forward from there is the results again.
//! - Typing more (or less) refines the search shown: it stays one place.
//! - Leaving the results for their folder (Escape, the clear button, emptying the box, Up) is a step back: the
//!   results are then what Forward brings back.
//! - Opening a folder from the results (or going anywhere else) leaves them behind in Back.
//!
//! # Staying true
//!
//! Only the searches nearest where the tab is keep their results ([`KEPT_RESULTS`], [`KEPT_ENTRIES`]); an older
//! one is searched for again when it comes back, as is one whose results were still coming in when it was left.
//! Results brought back that are gone from the disk meanwhile are taken out (a look in the background once they
//! show). A folder that is gone is skipped: Back says so and goes on to the place before it.

use super::model::Explorer;
use super::nav::{failure_notice, match_selection, Landed};
use super::tab::{ScrollMemo, Selection, Tab};
use super::types::*;
use glint_model::entries::FileEntry;
use glint_model::paths::{in_archive_path, is_folder_path, same_path, THIS_PC};
use gpui_kit::{Context, Task};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Back history keeps this many places.
pub(super) const HISTORY_MOST: usize = 100;
/// The searches nearest where a tab is keep their results, this many of them...
pub(super) const KEPT_RESULTS: usize = 6;
/// ...holding no more than this many results together. A search past either is searched for again.
pub(super) const KEPT_ENTRIES: usize = 200_000;
/// At most this many places that turn out to be gone are skipped by one press of Back or Forward.
const SKIPPED_MOST: usize = 20;

/// Which place in Back (or Forward) a walk goes to.
#[derive(Clone, Copy, Debug)]
enum Aim {
    /// This many places on: from where the tab is, or from the place a walk is still opening.
    Steps(usize),
    /// The place at this position of the list.
    At(usize),
}

/// Every visit's number: what a walk through the history finds its place by.
static NEXT_VISIT: AtomicU64 = AtomicU64::new(1);

/// One place in a tab's Back or Forward history, as the tab showed it when it left.
#[derive(Clone, Debug)]
pub struct Visit {
    /// The folder (or This PC): where the tab was, and the folder a search shown there was run from.
    pub path: String,
    /// The search the tab showed there, if it showed one.
    pub search: Option<SearchVisit>,
    /// The search box's text: a filter narrowing the folder, or the words of the search shown.
    pub filter: String,
    /// What was selected, the item Shift-selection grows from, and the one the keyboard was on.
    pub selection: Selection,
    /// How far the list (or the grid) was scrolled.
    pub(super) scroll: Option<ScrollMemo>,
    pub(super) id: u64,
}

/// A search's results as a place in the history.
#[derive(Clone, Debug)]
pub struct SearchVisit {
    pub query: String,
    /// The folder searched, with its subfolders (This PC searches every drive).
    pub scope: String,
    /// The results as they were left, brought back at once; `None` once the search is too far back to keep them
    /// (it is searched for again).
    pub(super) run: Option<FindRun>,
}

impl Visit {
    /// The words of the search this place is, if it is one.
    pub fn search_query(&self) -> Option<&str> {
        self.search.as_ref().map(|search| search.query.as_str())
    }

    /// Whether this place is the one `tab` shows: the same folder, and the same search in it or none.
    pub(super) fn is_place_of(&self, tab: &Tab) -> bool {
        same_place_path(&self.path, &tab.path)
            && match (&self.search, &tab.find) {
                (None, None) => true,
                (Some(search), Some(find)) => search.query == find.query && same_place_path(&search.scope, &find.scope),
                _ => false,
            }
    }

    /// Whether `self` and `other` are the same place.
    fn same_place(&self, other: &Visit) -> bool {
        same_place_path(&self.path, &other.path)
            && match (&self.search, &other.search) {
                (None, None) => true,
                (Some(a), Some(b)) => a.query == b.query && same_place_path(&a.scope, &b.scope),
                _ => false,
            }
    }
}

/// Two folders are the same place: the same path in any case, This PC only as itself.
pub(super) fn same_place_path(a: &str, b: &str) -> bool {
    if a == THIS_PC || b == THIS_PC {
        a == b
    } else {
        same_path(a, b)
    }
}

/// A search brought back from the history: it is not running (nothing reads a stream for it any more).
fn settled(mut run: FindRun) -> FindRun {
    run.running = false;
    run.inside.running = false;
    run
}

/// What was selected in a place, among the items it shows now: the paths that are still there, with the item the
/// keyboard was on (and Shift's anchor) when it still is.
pub(super) fn restored_selection(was: &Selection, entries: &[FileEntry]) -> Selection {
    if was.is_empty() {
        return Selection::default();
    }
    let selected = match_selection(entries, &was.to_vec());
    let pick = |wanted: Option<&str>| wanted.and_then(|wanted| selected.iter().find(|p| same_path(p, wanted)).cloned());
    let focus = pick(was.focus()).or_else(|| selected.last().cloned());
    let anchor = pick(was.anchor()).or_else(|| focus.clone());
    Selection {
        paths: Arc::new(selected.iter().map(String::as_str).collect()),
        anchor,
        focus,
    }
}

/// Whether a place that failed to open can be asked about on disk: a folder of a drive or a share, not one in an
/// archive or on a remote connection, nor a place of Glint's own.
fn checkable(path: &str) -> bool {
    is_folder_path(path) && !in_archive_path(path)
}

impl Tab {
    /// The place the tab shows, as it is now: what Back or Forward brings back.
    pub(super) fn visit(&self) -> Visit {
        Visit {
            path: self.path.clone(),
            search: self.find.as_ref().map(|find| SearchVisit {
                query: find.query.clone(),
                scope: find.scope.clone(),
                run: Some(find.clone()),
            }),
            // Emptying the box ends a search: the results come back with their words in it.
            filter: match &self.find {
                Some(find) if self.filter.is_empty() => find.query.clone(),
                _ => self.filter.clone(),
            },
            selection: self.selection.clone(),
            scroll: self.scroll_of(&self.view_key()),
            id: NEXT_VISIT.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// The folder the tab shows, as it was before a search began in it: its whole listing (no filter), with what
    /// was selected and how far it was scrolled.
    pub(super) fn folder_visit(&self) -> Visit {
        Visit {
            path: self.path.clone(),
            search: None,
            filter: String::new(),
            selection: self.selection.clone(),
            scroll: self.scroll_of(&format!("dir:{}", self.path)),
            id: NEXT_VISIT.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Where the list (or the grid, which keeps its place under the key with ":grid" after it) was scrolled for the
    /// view `key`, if it is that view's.
    fn scroll_of(&self, key: &str) -> Option<ScrollMemo> {
        self.scroll
            .clone()
            .filter(|memo| memo.key == key || memo.key.strip_suffix(":grid") == Some(key))
    }

    /// The tab goes somewhere new from `left`, the place it showed: that goes into Back, and Forward is forgotten.
    pub(super) fn leave_for_new(&mut self, left: Visit) {
        push_back(&mut self.back, left);
        self.forward.clear();
        keep_results(&mut self.back, &mut self.forward);
    }

    /// The tab arrives at the visit `id` from its history, leaving `left`: the places between go to the other side
    /// with it, in order. A visit no longer in the history (it changed meanwhile) makes this a step somewhere new.
    pub(super) fn arrive(&mut self, left: Visit, id: u64) {
        if let Some(at) = self.back.iter().position(|v| v.id == id) {
            // Back: what was after the place arrived at, and the place left, are Forward's first.
            let mut later: Vec<Visit> = self.back.drain(at..).skip(1).collect();
            later.push(left);
            self.forward.splice(0..0, later);
        } else if let Some(at) = self.forward.iter().position(|v| v.id == id) {
            // Forward: the place left, and what was before the place arrived at, are Back's last.
            let earlier: Vec<Visit> = self.forward.drain(..=at).collect();
            push_back(&mut self.back, left);
            for visit in earlier.into_iter().take(at) {
                push_back(&mut self.back, visit);
            }
        } else {
            push_back(&mut self.back, left);
        }
        keep_results(&mut self.back, &mut self.forward);
    }

    /// The tab leaves the search it shows for its folder (`left`, the results as they were): a step back. The
    /// results are Forward's first; the folder's own place in Back, where the search began, is taken and returned,
    /// so what was selected there and how far the list was scrolled come back with it.
    pub(super) fn step_back_from_search(&mut self, left: Visit) -> Option<Visit> {
        let folder = self
            .back
            .last()
            .is_some_and(|v| v.search.is_none() && same_place_path(&v.path, &self.path))
            .then(|| self.back.pop())
            .flatten();
        self.forward.insert(0, left);
        keep_results(&mut self.back, &mut self.forward);
        folder
    }

    /// [`Tab::step_back_from_search`], with the folder shown as it was before the search: what was selected there
    /// and how far it was scrolled.
    pub(super) fn back_from_search(&mut self, left: Visit) {
        if let Some(folder) = self.step_back_from_search(left) {
            self.selection = folder.selection;
            if folder.scroll.is_some() {
                self.scroll = folder.scroll;
            }
        }
    }

    /// Takes the visit `id` out of the history (it is gone, or it is where the tab already is).
    fn forget_visit(&mut self, id: u64) {
        self.back.retain(|v| v.id != id);
        self.forward.retain(|v| v.id != id);
    }
}

/// Adds `visit` to the end of Back: the same place twice in a row is one, and the oldest go past
/// [`HISTORY_MOST`].
fn push_back(back: &mut Vec<Visit>, visit: Visit) {
    if back.last().is_some_and(|last| last.same_place(&visit)) {
        back.pop();
    }
    back.push(visit);
    let extra = back.len().saturating_sub(HISTORY_MOST);
    back.drain(..extra);
}

/// Only the searches nearest where the tab is (Back's last and Forward's first, then outward) keep their results:
/// [`KEPT_RESULTS`] of them, with no more than [`KEPT_ENTRIES`] results together. The rest are searched for again.
pub(super) fn keep_results(back: &mut [Visit], forward: &mut [Visit]) {
    let (mut kept, mut entries) = (0usize, 0usize);
    let mut before = back.iter_mut().rev();
    let mut after = forward.iter_mut();
    loop {
        let (b, a) = (before.next(), after.next());
        if b.is_none() && a.is_none() {
            return;
        }
        for visit in [b, a].into_iter().flatten() {
            let Some(search) = visit.search.as_mut() else {
                continue;
            };
            let Some(size) = search.run.as_ref().map(|run| run.entries.len()) else {
                continue;
            };
            if kept < KEPT_RESULTS && entries + size <= KEPT_ENTRIES {
                kept += 1;
                entries += size;
            } else {
                search.run = None;
            }
        }
    }
}

impl Explorer {
    /// Back (Alt+Left, the mouse's back button, the toolbar): the place before this one in the active tab's history,
    /// as it was left (`goBack`).
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        self.go_through_history(true, 1, cx)
    }

    /// Forward (Alt+Right, the mouse's forward button): the place Back came from (`goForward`).
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_forward(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        self.go_through_history(false, 1, cx)
    }

    /// Backspace: Back, or, with nothing to go back to, Up.
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_back_or_up(&mut self, cx: &mut Context<Self>) -> Task<bool> {
        if self.active_tab().is_some_and(|tab| !tab.back.is_empty()) {
            self.go_back(cx)
        } else {
            self.go_up(cx)
        }
    }

    /// Back (or Forward) `steps` places at once in the active tab: the toolbar arrows' menus. The task resolves
    /// `false` when there is no such place, it couldn't be opened, or a newer navigation took over.
    #[must_use = "dropping the task cancels the navigation; `.detach()` it to let it run"]
    pub fn go_through_history(&mut self, backwards: bool, steps: usize, cx: &mut Context<Self>) -> Task<bool> {
        let id = self.active_tab_id();
        let Some((visit, path, landing)) = self.step_through_history(id, backwards, Aim::Steps(steps.max(1)), cx)
        else {
            return Task::ready(false);
        };
        cx.spawn(async move |this, cx| {
            let (mut visit, mut path, mut landing) = (visit, path, landing);
            let mut skipped = 0;
            loop {
                let message = match landing.await {
                    Landed::Opened => return true,
                    Landed::Superseded => return false,
                    Landed::Failed(message) => message,
                };
                // A folder that is gone is skipped, saying so: on to the place before it (after it, going forward).
                let asking = this
                    .update(cx, |this, _| {
                        checkable(&path).then(|| this.source.all_exist(vec![path.clone()]))
                    })
                    .ok()
                    .flatten();
                let gone = match asking {
                    Some(asking) => !asking.await,
                    None => false,
                };
                let next = this
                    .update(cx, |this, cx| {
                        if !gone {
                            if let Some(notice) = failure_notice(&message, &path, id, &None) {
                                cx.emit(ExplorerEvent::Notice(notice));
                            }
                            return None;
                        }
                        let was_at = this.tabs.get(&id).and_then(|t| {
                            let list = if backwards { &t.back } else { &t.forward };
                            list.iter().position(|v| v.id == visit)
                        });
                        this.update_tab(id, |t| t.forget_visit(visit));
                        this.tab_changed(id, cx);
                        cx.emit(ExplorerEvent::Notice(Notice::NoLongerAvailable { path: path.clone() }));
                        skipped += 1;
                        if skipped >= SKIPPED_MOST {
                            return None;
                        }
                        // Back, the place before it is one lower; forward, the place after it has moved into its spot.
                        let aim = match was_at {
                            Some(at) if backwards => Aim::At(at.checked_sub(1)?),
                            Some(at) => Aim::At(at),
                            None => Aim::Steps(1),
                        };
                        this.step_through_history(id, backwards, aim, cx)
                    })
                    .ok()
                    .flatten();
                let Some(next) = next else {
                    return false;
                };
                (visit, path, landing) = next;
            }
        })
    }

    /// Starts going to a place in tab `id`'s Back (or Forward) history: `aim` says which. Returns its visit's number,
    /// its folder, and how the going ends; `None` when there is no such place. A place that is where the tab already
    /// is gets passed over (and forgotten).
    fn step_through_history(
        &mut self,
        id: TabId,
        backwards: bool,
        mut aim: Aim,
        cx: &mut Context<Self>,
    ) -> Option<(u64, String, Task<Landed>)> {
        let visit = loop {
            let tab = self.tabs.get(&id)?;
            let list = if backwards { &tab.back } else { &tab.forward };
            let at = match aim {
                Aim::At(at) => Some(at).filter(|at| *at < list.len())?,
                Aim::Steps(steps) => {
                    // Pressed again while the last press's place still opens (a held Backspace): on from that place.
                    let opening = tab
                        .walking
                        .filter(|(way, _)| tab.loading && *way == backwards)
                        .and_then(|(_, number)| list.iter().position(|v| v.id == number));
                    match (opening, backwards) {
                        (Some(from), true) => from.checked_sub(steps)?,
                        (Some(from), false) => Some(from + steps).filter(|at| *at < list.len())?,
                        (None, true) => list.len().checked_sub(steps)?,
                        (None, false) => steps.checked_sub(1).filter(|at| *at < list.len())?,
                    }
                }
            };
            let visit = list.get(at)?;
            if tab.ready && visit.is_place_of(tab) {
                let same = visit.id;
                self.update_tab(id, |t| t.forget_visit(same));
                if let Aim::At(at) = aim {
                    aim = Aim::At(if backwards { at.checked_sub(1)? } else { at });
                }
                continue;
            }
            break visit.clone();
        };
        let (number, path) = (visit.id, visit.path.clone());
        let here = self
            .tabs
            .get(&id)
            .is_some_and(|tab| tab.ready && same_place_path(&tab.path, &visit.path));
        let landing = if here {
            // The same folder (a search in it, or the folder a search was run from): no need to read it first.
            self.restore_here(id, visit, cx);
            Task::ready(Landed::Opened)
        } else {
            let options = NavOptions {
                no_history: true,
                silent: true,
                tab: Some(id),
                restore: Some(Box::new(visit)),
                ..Default::default()
            };
            let landing = self.navigate_landing(path.clone(), options, cx);
            self.update_tab(id, |t| t.walking = Some((backwards, number)));
            landing
        };
        Some((number, path, landing))
    }

    /// Brings back `visit`, a place in the folder tab `id` shows already: its search's results (or the folder
    /// itself, read again in the background), its box, its selection and its scroll.
    fn restore_here(&mut self, id: TabId, visit: Visit, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(&id) else {
            return;
        };
        let left = tab.visit();
        let was_running = tab.find.as_ref().map(|find| find.running);
        let (run, rerun) = Self::restored_run(&visit);
        let entries: Arc<[FileEntry]> = match &run {
            Some(run) => run.entries.clone(),
            None => tab.shared_entries().unwrap_or_else(|| Arc::from(Vec::new())),
        };
        let selection = restored_selection(&visit.selection, &entries);
        let check = run.is_some() && rerun.is_none();
        let folder = visit.search.is_none();
        if let Some(running) = was_running {
            self.stop_find_job(id, running);
        }
        self.update_tab(id, |t| {
            t.find = run;
            t.filter = visit.filter.clone();
            t.selection = selection;
            t.renaming = None;
            t.walking = None;
            if visit.scroll.is_some() {
                t.scroll = visit.scroll.clone();
            }
            t.arrive(left, visit.id);
        });
        if self.resolve_view(id, cx) {
            cx.emit(ExplorerEvent::ViewChanged(id));
        }
        self.rebuild_rows(id, cx);
        cx.emit(ExplorerEvent::SelectionChanged(id));
        self.tab_changed(id, cx);
        if folder {
            // What changed in the folder while the results showed is read now; the selection is kept.
            self.reload(Some(id), cx).detach();
        }
        self.after_restore(id, rerun, check, cx);
    }

    /// The results a visit brings back (settled: nothing streams them now), and the search to run again instead
    /// (its words and the folder searched) when they weren't kept or were still coming in when it was left.
    pub(super) fn restored_run(visit: &Visit) -> (Option<FindRun>, Option<(String, String)>) {
        let Some(search) = &visit.search else {
            return (None, None);
        };
        let incomplete = search.run.as_ref().is_none_or(|run| run.running || run.inside.running);
        let rerun = incomplete.then(|| (search.query.clone(), search.scope.clone()));
        (search.run.clone().map(settled), rerun)
    }

    /// A place is back: a search whose results weren't kept (or weren't all in) runs again where it ran; results
    /// brought back are checked for what is gone from the disk meanwhile (`check`).
    pub(super) fn after_restore(
        &mut self,
        id: TabId,
        rerun: Option<(String, String)>,
        check: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some((query, scope)) = rerun {
            self.run_find_again(id, &query, scope, cx).detach();
        } else if check {
            self.drop_vanished_results(id, cx);
        }
    }

    /// Results brought back from the history lose what is no longer on disk: deleted, moved or renamed since they
    /// were found. Looked at in the background; nothing else changes (no new results, no new order).
    fn drop_vanished_results(&mut self, id: TabId, cx: &mut Context<Self>) {
        let Some(find) = self.tabs.get(&id).and_then(|t| t.find.as_ref()) else {
            return;
        };
        let run = find.id;
        let paths: Vec<String> = find
            .entries
            .iter()
            .filter(|entry| checkable(&entry.path))
            .map(|entry| entry.path.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        let asking = self.source.missing(paths);
        cx.spawn(async move |this, cx| {
            let gone = asking.await;
            if gone.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                let gone: HashSet<&str> = gone.iter().map(String::as_str).collect();
                let still = this
                    .tabs
                    .get(&id)
                    .and_then(|t| t.find.as_ref())
                    .is_some_and(|find| find.id == run);
                if !still {
                    return;
                }
                this.update_tab(id, |t| {
                    if let Some(find) = &mut t.find {
                        let kept: Vec<FileEntry> = find
                            .entries
                            .iter()
                            .filter(|entry| !gone.contains(entry.path.as_str()))
                            .cloned()
                            .collect();
                        find.entries = kept.into();
                    }
                    if t.selection.iter().any(|path| gone.contains(path)) {
                        let was = &t.selection;
                        let left: Vec<String> = was.iter().filter(|p| !gone.contains(p)).map(str::to_string).collect();
                        let keep = |path: Option<&str>| path.filter(|p| !gone.contains(p)).map(str::to_string);
                        let focus = keep(was.focus()).or_else(|| left.last().cloned());
                        let anchor = keep(was.anchor()).or_else(|| focus.clone());
                        let now = Selection {
                            paths: Arc::new(left.iter().map(String::as_str).collect()),
                            anchor,
                            focus,
                        };
                        t.selection = now;
                    }
                });
                this.rebuild_rows(id, cx);
                cx.emit(ExplorerEvent::SelectionChanged(id));
                this.tab_changed(id, cx);
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glint_model::entries::{with_details, RawEntry};

    fn folder(path: &str) -> Visit {
        Visit {
            path: path.to_string(),
            search: None,
            filter: String::new(),
            selection: Selection::default(),
            scroll: None,
            id: NEXT_VISIT.fetch_add(1, Ordering::Relaxed),
        }
    }

    fn search(path: &str, query: &str, results: usize) -> Visit {
        let one = with_details(
            RawEntry {
                name: "found.txt".into(),
                ..Default::default()
            },
            path,
        );
        let entries: Vec<FileEntry> = vec![one; results];
        Visit {
            search: Some(SearchVisit {
                query: query.to_string(),
                scope: path.to_string(),
                run: Some(FindRun {
                    query: query.to_string(),
                    scope: path.to_string(),
                    entries: entries.into(),
                    ..Default::default()
                }),
            }),
            ..folder(path)
        }
    }

    fn kept(visits: &[Visit]) -> Vec<bool> {
        visits
            .iter()
            .map(|v| v.search.as_ref().is_some_and(|s| s.run.is_some()))
            .collect()
    }

    #[test]
    fn the_same_place_twice_in_a_row_is_one_step_back() {
        let mut back = Vec::new();
        push_back(&mut back, folder("C:\\a"));
        push_back(&mut back, folder("c:\\A\\"));
        assert_eq!(back.len(), 1);
        assert_eq!(
            back[0].path, "c:\\A\\",
            "the newer visit, as it was left, is the one kept"
        );
        // A search in the folder is another place; This PC is only itself.
        push_back(&mut back, search("C:\\a", "x", 1));
        push_back(&mut back, folder(THIS_PC));
        push_back(&mut back, folder("\\"));
        assert_eq!(back.len(), 4);
    }

    #[test]
    fn back_keeps_a_hundred_places() {
        let mut back = Vec::new();
        for i in 0..130 {
            push_back(&mut back, folder(&format!("C:\\f{i}")));
        }
        assert_eq!(back.len(), HISTORY_MOST);
        assert_eq!(back[0].path, "C:\\f30");
    }

    #[test]
    fn only_the_searches_nearest_the_tab_keep_their_results() {
        let mut back: Vec<Visit> = (0..5).map(|i| search("C:\\", &format!("b{i}"), 10)).collect();
        let mut forward: Vec<Visit> = (0..5).map(|i| search("C:\\", &format!("f{i}"), 10)).collect();
        keep_results(&mut back, &mut forward);
        // Three each side, nearest first: Back's last three and Forward's first three.
        assert_eq!(kept(&back), [false, false, true, true, true]);
        assert_eq!(kept(&forward), [true, true, true, false, false]);
    }

    #[test]
    fn searches_with_too_many_results_together_are_searched_for_again() {
        let mut back = vec![
            search("C:\\", "old", KEPT_ENTRIES / 2),
            search("C:\\", "big", KEPT_ENTRIES / 2 + 1),
            search("C:\\", "near", KEPT_ENTRIES / 2),
        ];
        keep_results(&mut back, &mut []);
        assert_eq!(
            kept(&back),
            [true, false, true],
            "the one that would pass the limit lets go of its results"
        );
    }

    #[test]
    fn arriving_back_or_forward_moves_the_places_between_to_the_other_side() {
        let mut tab = Tab::blank(TabId(1), "C:\\e", ViewMode::List, None);
        tab.back = ["C:\\a", "C:\\b", "C:\\c", "C:\\d"].map(folder).to_vec();
        let target = tab.back[1].id;
        tab.arrive(folder("C:\\e"), target);
        let paths = |list: &[Visit]| list.iter().map(|v| v.path.clone()).collect::<Vec<_>>();
        assert_eq!(paths(&tab.back), ["C:\\a"]);
        assert_eq!(paths(&tab.forward), ["C:\\c", "C:\\d", "C:\\e"]);
        // And forward again, two at once: from b to d.
        let target = tab.forward[1].id;
        tab.arrive(folder("C:\\b"), target);
        assert_eq!(paths(&tab.back), ["C:\\a", "C:\\b", "C:\\c"]);
        assert_eq!(paths(&tab.forward), ["C:\\e"]);
        // A visit the history no longer has: a step somewhere new, Forward kept.
        tab.arrive(folder("C:\\d"), u64::MAX);
        assert_eq!(paths(&tab.back), ["C:\\a", "C:\\b", "C:\\c", "C:\\d"]);
        assert_eq!(paths(&tab.forward), ["C:\\e"]);
    }

    #[test]
    fn leaving_a_search_for_its_folder_takes_the_folders_place_and_offers_the_results_forward() {
        let mut tab = Tab::blank(TabId(1), "C:\\a", ViewMode::List, None);
        let mut before = folder("C:\\a");
        before.selection = Selection::of(["C:\\a\\x.txt"]);
        tab.back = vec![folder("C:\\"), before];
        let folder_place = tab.step_back_from_search(search("C:\\a", "x", 3));
        assert_eq!(
            folder_place.map(|v| v.selection.to_vec()),
            Some(vec!["C:\\a\\x.txt".to_string()])
        );
        assert_eq!(tab.back.len(), 1);
        assert_eq!(tab.forward.first().and_then(|v| v.search_query()), Some("x"));
        // With another folder last in Back, nothing is taken from it.
        tab.path = "C:\\b".into();
        assert!(tab.step_back_from_search(search("C:\\b", "y", 1)).is_none());
        assert_eq!(tab.back.len(), 1);
        assert_eq!(tab.forward.len(), 2);
    }

    #[test]
    fn a_visit_keeps_the_scroll_of_its_own_view_only() {
        let mut tab = Tab::blank(TabId(1), "C:\\a", ViewMode::List, None);
        tab.scroll = Some(ScrollMemo {
            key: "dir:C:\\a".into(),
            top: 40.0,
        });
        assert_eq!(tab.visit().scroll.map(|s| s.top), Some(40.0));
        tab.scroll = Some(ScrollMemo {
            key: "dir:C:\\a:grid".into(),
            top: 3.0,
        });
        assert_eq!(tab.visit().scroll.map(|s| s.top), Some(3.0), "the grid's place");
        tab.scroll = Some(ScrollMemo {
            key: "dir:C:\\other".into(),
            top: 9.0,
        });
        assert!(tab.visit().scroll.is_none(), "another folder's place is not this one's");
    }

    #[test]
    fn a_search_left_with_an_empty_box_comes_back_with_its_words() {
        let mut tab = Tab::blank(TabId(1), "C:\\a", ViewMode::List, None);
        tab.find = Some(FindRun {
            query: "report".into(),
            scope: "C:\\a".into(),
            ..Default::default()
        });
        assert_eq!(tab.visit().filter, "report");
        tab.filter = "report ext:pdf".into();
        assert_eq!(tab.visit().filter, "report ext:pdf");
    }

    #[test]
    fn results_still_coming_in_when_left_are_searched_for_again() {
        let mut visit = search("C:\\a", "x", 2);
        let (run, rerun) = Explorer::restored_run(&visit);
        assert!(run.is_some() && rerun.is_none());
        if let Some(run) = visit.search.as_mut().and_then(|s| s.run.as_mut()) {
            run.running = true;
        }
        let (run, rerun) = Explorer::restored_run(&visit);
        assert!(run.is_some_and(|run| !run.running), "shown meanwhile, not running");
        assert_eq!(rerun, Some(("x".to_string(), "C:\\a".to_string())));
        if let Some(search) = visit.search.as_mut() {
            search.run = None;
        }
        assert_eq!(
            Explorer::restored_run(&visit).1,
            Some(("x".to_string(), "C:\\a".to_string()))
        );
    }
}
