//! Git for the tabs on screen: each tab's repository view (branch, upstream, marks, counts), kept up to date as
//! the TypeScript's `useGit` store did. Ported from src/lib/git.ts.
//!
//! | TypeScript                             | Here                                                              |
//! |----------------------------------------|-------------------------------------------------------------------|
//! | `useGit` (zustand, by tab id), `GitRun` | [`Git`], one model entity in a global ([`shared`]); a [`Run`] per tab |
//! | `refreshGit`                           | [`Git::refresh`]                                                   |
//! | `forget`                               | [`Git::forget`]                                                    |
//! | `useGitMark`                           | [`mark_of`]                                                        |
//! | `trustRepository`                      | [`trust_repository`]                                               |
//! | `watchShown`                           | [`Git::watch_shown`]                                               |
//! | `useExplorer.subscribe(...)`           | the subscription in [`Git::new`] to the explorer's events          |
//! | `listen("git-changed")`                | `core_bridge::subscribe` in [`Git::new`]                           |
//! | `gitView`, `cancelGit`, `gitWatch`, `gitDiff`, `gitHeadCopy`, `gitHeadDiscard`, `repoCard` | the [`Source`] trait; [`CoreGit`] is glint-core's |
//!
//! **Nothing here waits on the UI thread.** Status, the diff and the card are glint-core calls run on its
//! runtime behind a [`Source`] (a fake in the tests); the answer comes back to the model, which tells the views.
//!
//! **Marks are converted once.** core sends `Mark`s; the model keeps them as glint-model's [`GitMark`]s in a
//! shared map ([`GitMarks`], lowercase name to mark), which is exactly what `matcher` and `group_rows` take.

use crate::core_bridge;
use crate::explorer::{try_explorer, ExplorerEvent, TabId};
use crate::state;
use glint_core::git::{FileDiff, GitView as CoreView, Mark};
use glint_core::repo::RepoCard;
use glint_model::entries::GitMark;
use glint_model::groups::GitMarks;
use glint_model::paths::{is_folder_path, same_path};
use gpui_kit::{App, AppContext as _, Context, Entity, EventEmitter, Global, Subscription, Task};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

/// What changed in the folder and below it (`GitView.counts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub modified: u32,
    pub staged: u32,
    pub untracked: u32,
    pub conflicted: u32,
}

/// Git in a folder: its repository's branch and upstream, and what changed in it (`GitView`, git.rs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The repository's working folder.
    pub root: String,
    /// The branch checked out; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// HEAD's commit, abbreviated; `None` before the first commit.
    pub head: Option<String>,
    /// The branch it tracks ("origin/main").
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// What's right in the folder has, by lowercase name. Shared: the rows, the matcher and Group by all read it.
    pub marks: Arc<GitMarks>,
    /// Changes in the folder and below it.
    pub counts: Counts,
    /// Status ran out of time (4 s): what's shown is what it found by then.
    pub partial: bool,
    /// The repository's own settings would run programs: status waits until it's trusted.
    pub limited: bool,
    /// Too many files to look through on every visit (or on another computer): F5 shows status.
    pub on_request: bool,
    /// How long status took, ms.
    pub elapsed: f64,
}

/// core's mark as the one glint-model's matcher, grouping and the rows use.
fn mark_from_core(mark: Mark) -> GitMark {
    match mark {
        Mark::Ignored => GitMark::Ignored,
        Mark::Untracked => GitMark::Untracked,
        Mark::Staged => GitMark::Staged,
        Mark::Modified => GitMark::Modified,
        Mark::Conflicted => GitMark::Conflicted,
    }
}

impl From<CoreView> for View {
    fn from(view: CoreView) -> Self {
        View {
            root: view.root,
            branch: view.branch,
            head: view.head,
            upstream: view.upstream,
            ahead: view.ahead,
            behind: view.behind,
            marks: Arc::new(view.marks.into_iter().map(|(k, m)| (k, mark_from_core(m))).collect()),
            counts: Counts {
                modified: view.counts.modified,
                staged: view.counts.staged,
                untracked: view.counts.untracked,
                conflicted: view.counts.conflicted,
            },
            partial: view.partial,
            limited: view.limited,
            on_request: view.on_request,
            elapsed: view.elapsed,
        }
    }
}

// ---- Where the answers come from ---------------------------------------------------------------------------

/// A boxed answer from glint-core (or a fake).
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// Everything Git asks of glint-core: glint-core in the app, a fake in the tests.
pub trait Source {
    /// Git in `folder` for the tab's `lane` (a newer request there, or [`Source::cancel`], ends this one): `None`
    /// when the folder isn't in a repository. `trusted` repositories may run programs their settings name; `force`
    /// (F5) runs status even where it waits for it (`gitView`).
    fn view(&self, folder: String, lane: String, trusted: Vec<String>, force: bool) -> Fut<Option<View>>;
    /// Stops the lane's status run (`cancelGit`).
    fn cancel(&self, lane: &str);
    /// Watches exactly the repositories at `roots`; each emits "git-changed" with its root (`gitWatch`).
    fn watch(&self, roots: Vec<String>);
    /// What changed in the file since the last commit; `None` outside a repository (`gitDiff`).
    fn diff(&self, path: String) -> Fut<Option<FileDiff>>;
    /// The file as it is in its last commit, in a temp file; `None` when it isn't in it (`gitHeadCopy`).
    fn head_copy(&self, path: String) -> Fut<Option<String>>;
    /// Removes a copy [`Source::head_copy`] made (`gitHeadDiscard`).
    fn head_discard(&self, path: String);
    /// The card of the repository whose folder `path` is; `None` when it isn't one's (`repoCard`).
    fn card(&self, path: String) -> Fut<Option<RepoCard>>;
}

/// glint-core's git.rs and repo.rs, through the app's handle.
pub struct CoreGit {
    app: glint_core::host::AppHandle,
}

impl CoreGit {
    pub fn new(app: glint_core::host::AppHandle) -> Self {
        Self { app }
    }
}

/// Runs a core future, flattening the join error into the call's own (two layers: the task may panic, the call
/// may fail).
fn on_core<T: Send + 'static>(future: impl Future<Output = Result<T, String>> + Send + 'static) -> Fut<T> {
    let joined = core_bridge::run(future);
    Box::pin(async move { joined.await.and_then(|inner| inner) })
}

impl Source for CoreGit {
    fn view(&self, folder: String, lane: String, trusted: Vec<String>, force: bool) -> Fut<Option<View>> {
        on_core(async move {
            let view = glint_core::git::git_view(folder, lane, trusted, force).await?;
            // The marks are converted on core's runtime, not on the UI thread: a big folder has many.
            Ok(view.map(View::from))
        })
    }

    fn cancel(&self, lane: &str) {
        glint_core::git::cancel_git(lane.to_string());
    }

    fn watch(&self, roots: Vec<String>) {
        use glint_core::host::Manager as _;
        let app = self.app.clone();
        glint_core::host::async_runtime::spawn(async move {
            let (Some(window), Some(state)) = (
                app.get_webview_window(crate::workspace::MAIN),
                app.try_state::<glint_core::git::RepoWatcher>(),
            ) else {
                return;
            };
            let _ = glint_core::git::git_watch(window, state, roots).await;
        });
    }

    fn diff(&self, path: String) -> Fut<Option<FileDiff>> {
        on_core(glint_core::git::git_diff(path))
    }

    fn head_copy(&self, path: String) -> Fut<Option<String>> {
        on_core(glint_core::git::git_head_copy(path))
    }

    fn head_discard(&self, path: String) {
        // Removing the copy's folder is disk work: not on the UI thread.
        glint_core::host::async_runtime::spawn_blocking(move || glint_core::git::git_head_discard(path));
    }

    fn card(&self, path: String) -> Fut<Option<RepoCard>> {
        on_core(glint_core::repo::repo_card(path))
    }
}

/// For an app whose core isn't up: no repository anywhere.
pub struct NoCore;

impl Source for NoCore {
    fn view(&self, _: String, _: String, _: Vec<String>, _: bool) -> Fut<Option<View>> {
        Box::pin(async { Ok(None) })
    }

    fn cancel(&self, _: &str) {}

    fn watch(&self, _: Vec<String>) {}

    fn diff(&self, _: String) -> Fut<Option<FileDiff>> {
        Box::pin(async { Ok(None) })
    }

    fn head_copy(&self, _: String) -> Fut<Option<String>> {
        Box::pin(async { Ok(None) })
    }

    fn head_discard(&self, _: String) {}

    fn card(&self, _: String) -> Fut<Option<RepoCard>> {
        Box::pin(async { Ok(None) })
    }
}

// ---- The model ---------------------------------------------------------------------------------------------

/// A tab's Git: its folder's view (`None`: not in a repository), while it's being worked out too (`GitRun`).
#[derive(Clone, Debug)]
pub struct Run {
    pub folder: String,
    pub view: Option<Arc<View>>,
    pub running: bool,
}

/// What the model tells the window about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitEvent {
    /// A tab's Git changed: its view arrived, or it stopped having one. The rows' Git marks, `git:` filters and
    /// Group by Git are out of date for this tab: build them again.
    Changed(TabId),
}

/// The request in flight for a tab: its number (an answer for an older one is dropped) and the task waiting for it.
struct Ask {
    seq: u64,
    _task: Task<()>,
}

/// Each tab's Git (`useGit`). One lives in a global ([`shared`]).
pub struct Git {
    source: Rc<dyn Source>,
    runs: HashMap<TabId, Run>,
    asks: HashMap<TabId, Ask>,
    /// Tabs asked again while their folder's status was being worked out: the folder, to look at once more after.
    again: HashMap<TabId, String>,
    /// The repositories being watched, as the sorted lowercase roots joined (to notice a change).
    watched: String,
    /// Cards read this session, by lowercase folder: a folder previewed again shows its card at once.
    cards: HashMap<String, Option<Arc<RepoCard>>>,
    next_seq: u64,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<GitEvent> for Git {}

/// A path with its case folded, for keys.
fn key(path: &str) -> String {
    path.to_lowercase()
}

impl Git {
    /// A model reading through `source`. It follows the explorer's tabs and core's "git-changed" event.
    pub fn new(source: Rc<dyn Source>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        // Git follows each tab's listing: a folder opened (or read again) is looked at; F5 runs status even
        // where it waits for it. Search results, archives, This PC, the Recycle Bin and closed tabs have none.
        if let Some(explorer) = try_explorer(cx) {
            subscriptions.push(
                cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| match event {
                    ExplorerEvent::ListingChanged { tab, .. } => this.listing_changed(*tab, cx),
                    ExplorerEvent::TabChanged(tab) | ExplorerEvent::FindDropped { tab, .. } => {
                        this.tab_changed(*tab, cx)
                    }
                    ExplorerEvent::TabClosed { tab, .. } => this.forget(*tab, cx),
                    ExplorerEvent::TabsChanged | ExplorerEvent::PaneFocused => this.watch_shown(cx),
                    _ => {}
                }),
            );
        }
        // A repository on screen changed (a save, git add, a checkout): its tabs look again.
        if let Some(subscription) = core_bridge::subscribe(cx, |this, event, cx| {
            if event.name == "git-changed" {
                if let Some(root) = event.payload.as_str() {
                    this.repository_changed(root, cx);
                }
            }
        }) {
            subscriptions.push(subscription);
        }
        let mut this = Self {
            source,
            runs: HashMap::new(),
            asks: HashMap::new(),
            again: HashMap::new(),
            watched: String::new(),
            cards: HashMap::new(),
            next_seq: 1,
            _subscriptions: subscriptions,
        };
        // Tabs that were already open when this model was made.
        if let Some(explorer) = try_explorer(cx) {
            let tabs: Vec<TabId> = explorer.read(cx).visible_tabs();
            for tab in tabs {
                this.tab_changed(tab, cx);
            }
        }
        this
    }

    /// The source, for the views that ask for a diff or a card themselves.
    pub fn source(&self) -> Rc<dyn Source> {
        self.source.clone()
    }

    /// A tab's Git: `None` for a tab in no repository (or showing a search, an archive, This PC).
    pub fn run(&self, tab: TabId) -> Option<&Run> {
        self.runs.get(&tab)
    }

    /// The card read for this folder earlier in the session: `Some(None)` when it had none.
    pub fn cached_card(&self, folder: &str) -> Option<Option<Arc<RepoCard>>> {
        self.cards.get(&key(folder)).cloned()
    }

    /// Remembers the card read for a folder.
    pub fn remember_card(&mut self, folder: &str, card: Option<Arc<RepoCard>>) {
        self.cards.insert(key(folder), card);
    }

    /// The folder a tab's Git is for, if the tab shows a real folder on disk: not a search, an archive, a folder
    /// read from the index, This PC, the Recycle Bin or a remote.
    fn wanted(&self, tab: TabId, cx: &App) -> Option<String> {
        let explorer = try_explorer(cx)?;
        let explorer = explorer.read(cx);
        let tab = explorer.tab(tab)?;
        let listing = tab.listing.as_ref()?;
        (tab.find.is_none() && listing.archive.is_none() && !listing.from_index && is_folder_path(&tab.path))
            .then(|| tab.path.clone())
    }

    /// A tab's folder was listed (or listed again).
    fn listing_changed(&mut self, tab: TabId, cx: &mut Context<Self>) {
        let Some(folder) = self.wanted(tab, cx) else {
            self.forget(tab, cx);
            return;
        };
        // F5 on the tab asked for status even where it waits for it.
        let force = try_explorer(cx)
            .is_some_and(|explorer| explorer.update(cx, |explorer, _| explorer.refreshed_by_hand.remove(&tab)));
        self.refresh(tab, &folder, force, cx);
    }

    /// Something about a tab changed: a search began (Git goes), or ended (Git comes back).
    fn tab_changed(&mut self, tab: TabId, cx: &mut Context<Self>) {
        match self.wanted(tab, cx) {
            None => self.forget(tab, cx),
            Some(folder) if !self.runs.contains_key(&tab) => self.refresh(tab, &folder, false, cx),
            Some(_) => {}
        }
    }

    /// Asks for Git in a tab's folder; a view of the same folder stays on screen meanwhile (`refreshGit`).
    pub fn refresh(&mut self, tab: TabId, folder: &str, force: bool, cx: &mut Context<Self>) {
        let before = self.runs.get(&tab).cloned();
        let same = before.as_ref().filter(|b| same_path(&b.folder, folder));
        // Asked again while the same folder's status runs (a build writing all the time): once more after it,
        // rather than starting over, which a big repository's status would never get through.
        if same.is_some_and(|b| b.running) && !force {
            self.again.insert(tab, folder.to_string());
            return;
        }
        self.runs.insert(
            tab,
            Run {
                folder: folder.to_string(),
                view: same.and_then(|b| b.view.clone()),
                running: true,
            },
        );
        let seq = self.next_seq;
        self.next_seq += 1;
        let trusted = state::settings(cx).git_trusted.clone();
        let asked = self.source.view(folder.to_string(), tab.to_string(), trusted, force);
        let folder = folder.to_string();
        let task = cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| this.answered(tab, seq, &folder, result, cx))
                .ok();
        });
        // Replacing the old request drops its task: its answer, if it still comes, is not waited for.
        self.asks.insert(tab, Ask { seq, _task: task });
        cx.notify();
        cx.emit(GitEvent::Changed(tab));
    }

    /// F5 in a very big repository: runs status in the tab's folder even where it waits to be asked.
    pub fn refresh_forced(&mut self, tab: TabId, cx: &mut Context<Self>) {
        if let Some(folder) = self.runs.get(&tab).map(|run| run.folder.clone()) {
            self.refresh(tab, &folder, true, cx);
        }
    }

    /// An answer for the request numbered `seq` of `tab` (`refreshGit`'s `await`).
    fn answered(
        &mut self,
        tab: TabId,
        seq: u64,
        folder: &str,
        result: Result<Option<View>, String>,
        cx: &mut Context<Self>,
    ) {
        // Stopped for a newer request, or the tab went: that one answers instead.
        if self.asks.get(&tab).map(|ask| ask.seq) != Some(seq) {
            return;
        }
        self.asks.remove(&tab);
        match result {
            Ok(Some(view)) => {
                self.runs.insert(
                    tab,
                    Run {
                        folder: folder.to_string(),
                        view: Some(Arc::new(view)),
                        running: false,
                    },
                );
            }
            Ok(None) => {
                self.runs.remove(&tab);
            }
            Err(_) => {
                // Stopped for a newer request, or the folder went: what was shown stays.
                match self.runs.get_mut(&tab) {
                    Some(run) if run.view.is_some() => run.running = false,
                    _ => {
                        self.runs.remove(&tab);
                    }
                }
            }
        }
        self.watch_shown(cx);
        cx.notify();
        cx.emit(GitEvent::Changed(tab));
        if let Some(next) = self.again.remove(&tab) {
            if self.runs.get(&tab).is_some_and(|run| same_path(&run.folder, &next)) {
                self.refresh(tab, &next, false, cx);
            }
        }
    }

    /// Takes Git off a tab (it closed, or shows something that isn't a folder on disk) (`forget`).
    pub fn forget(&mut self, tab: TabId, cx: &mut Context<Self>) {
        self.again.remove(&tab);
        self.asks.remove(&tab);
        if self.runs.remove(&tab).is_none() {
            return;
        }
        self.source.cancel(&tab.to_string());
        self.watch_shown(cx);
        cx.notify();
        cx.emit(GitEvent::Changed(tab));
    }

    /// A watched repository changed: every tab showing it looks again.
    fn repository_changed(&mut self, root: &str, cx: &mut Context<Self>) {
        let tabs: Vec<(TabId, String)> = self
            .runs
            .iter()
            .filter(|(_, run)| run.view.as_ref().is_some_and(|view| same_path(&view.root, root)))
            .map(|(tab, run)| (*tab, run.folder.clone()))
            .collect();
        for (tab, folder) in tabs {
            self.refresh(tab, &folder, false, cx);
        }
    }

    /// Trusts a repository whose own settings name programs for status to run, and shows its status
    /// (`trustRepository`).
    pub fn trust_repository(&mut self, root: &str, cx: &mut Context<Self>) {
        if !state::settings(cx).git_trusted.iter().any(|r| same_path(r, root)) {
            let root = root.to_string();
            state::update(cx, |settings| settings.git_trusted.push(root));
        }
        let tabs: Vec<(TabId, String)> = self
            .runs
            .iter()
            .filter(|(_, run)| run.view.as_ref().is_some_and(|view| same_path(&view.root, root)))
            .map(|(tab, run)| (*tab, run.folder.clone()))
            .collect();
        for (tab, folder) in tabs {
            // NOTE(port): the TypeScript asked again without `force`, and core hands back the view it worked
            // out before (still "limited") when nothing changed in the repository since. Forcing makes the
            // status run now that the repository is trusted.
            self.refresh(tab, &folder, true, cx);
        }
    }

    /// Watches the repositories of the tabs on screen, and only those (`watchShown`).
    fn watch_shown(&mut self, cx: &mut Context<Self>) {
        let Some(explorer) = try_explorer(cx) else { return };
        let mut roots: Vec<String> = explorer
            .read(cx)
            .visible_tabs()
            .into_iter()
            .filter_map(|tab| self.runs.get(&tab)?.view.as_ref().map(|view| view.root.clone()))
            .collect();
        roots.sort();
        roots.dedup();
        let wanted = key(&roots.join("\n"));
        if wanted == self.watched {
            return;
        }
        self.watched = wanted;
        self.source.watch(roots);
    }
}

// ---- The shared model --------------------------------------------------------------------------------------

/// The shared model, in a GPUI `Global`.
struct SharedGit(Entity<Git>);

impl Global for SharedGit {}

/// Makes the shared model, reading through glint-core. Call it after `core_bridge::init` and `explorer::init`
/// (at start-up, so Git follows the tabs from the first folder).
pub fn init(cx: &mut App) {
    let source: Rc<dyn Source> = match cx.try_global::<core_bridge::Core>() {
        Some(core) => Rc::new(CoreGit::new(core.app.clone())),
        None => Rc::new(NoCore),
    };
    init_with(cx, source);
}

/// [`init`] with a source of your own (tests, tools).
pub fn init_with(cx: &mut App, source: Rc<dyn Source>) {
    let git = cx.new(|cx| Git::new(source, cx));
    cx.set_global(SharedGit(git));
}

/// The shared model, made on first use if [`init`] has not run.
pub fn shared(cx: &mut App) -> Entity<Git> {
    if cx.try_global::<SharedGit>().is_none() {
        init(cx);
    }
    cx.global::<SharedGit>().0.clone()
}

/// The shared model, if it exists.
pub fn try_shared(cx: &App) -> Option<Entity<Git>> {
    cx.try_global::<SharedGit>().map(|shared| shared.0.clone())
}

// ---- Plain functions for the coordinator --------------------------------------------------------------------

/// A tab's view of its repository (the TypeScript's `run.view`): `None` for a tab in no repository, and while the
/// first status is being worked out.
pub fn view_of(cx: &App, tab: TabId) -> Option<Arc<View>> {
    try_shared(cx)?.read(cx).run(tab)?.view.clone()
}

/// Whether the tab's folder is in a Git repository (its status may still be being worked out): the address bar's
/// `git:` hint (`AddressBar::set_in_repository`) and the chip.
pub fn in_repository(cx: &App, tab: TabId) -> bool {
    view_of(cx, tab).is_some()
}

/// What Git says of what's right in the tab's folder, by lowercase name (`useGit(...).view.marks`, which is
/// `undefined` while a search shows: such a tab has no run here). Pass it to `glint_model::entries::matcher` for the
/// `git:` filters and to `glint_model::groups::group_rows` for Group by Git.
pub fn marks(cx: &App, tab: TabId) -> Option<Arc<GitMarks>> {
    view_of(cx, tab).map(|view| view.marks.clone())
}

/// What Git says of the item at `path`, from its folder's view in any tab (`useGitMark`): `None` for nothing, or
/// not known.
pub fn mark_of(cx: &App, path: &str) -> Option<GitMark> {
    let cut = path.rfind(['\\', '/'])?;
    // A separator is one byte, so both halves are on character boundaries.
    let (folder, name) = (path.get(..cut)?, path.get(cut + 1..)?);
    let git = try_shared(cx)?;
    let git = git.read(cx);
    git.runs
        .values()
        .find(|run| run.view.is_some() && same_path(&run.folder, folder))?
        .view
        .as_ref()?
        .marks
        .get(&name.to_lowercase())
        .copied()
}

/// Whether a file Git has a change for can show what changed since its last commit (the preview's "Changes" view,
/// `changed` in FilePreview.tsx): not a folder, not in an archive, with a mark that isn't "ignored".
pub fn has_changes_view(cx: &App, entry: &glint_model::entries::FileEntry) -> bool {
    !entry.dir
        && !glint_model::entries::is_archived(entry)
        && mark_of(cx, &entry.path).is_some_and(|mark| mark != GitMark::Ignored)
}

/// Asks for Git in a tab's folder now (`refreshGit`); `force` runs status even where it waits for it (F5).
pub fn refresh(cx: &mut App, tab: TabId, force: bool) {
    let Some(git) = try_shared(cx) else { return };
    git.update(cx, |git, cx| {
        if let Some(folder) = git.runs.get(&tab).map(|run| run.folder.clone()) {
            git.refresh(tab, &folder, force, cx);
        }
    });
}

/// Trusts the repository at `root` and shows its status (`trustRepository`).
pub fn trust_repository(cx: &mut App, root: &str) {
    let git = shared(cx);
    git.update(cx, |git, cx| git.trust_repository(root, cx));
}
