//! Headless tests of Git: the marks and the branch following the tabs' folders, the chip and its popover (a branch,
//! a dirty tree, a repository that waits for trust or for F5), the status bar's words, the row badges, the
//! repository card, the README (in Arabic too) and the Changes view with its two sides. Git's answers come from a
//! fake [`Source`], so nothing touches the real core; the explorer runs on a fake disk.

use super::cards::{ReadmeCard, RepoCard};
use super::changes::{ChangesView, OpenSides};
use super::chip::GitChip;
use super::model::{self, Counts, Fut, Git, GitEvent, Source, View};
use super::words::{self, Tone};
use super::{mark_of, marks, row, shared, status_line};
use crate::dialogs::compare_files::{OnClose, Side};
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{self, Explorer, NavOptions, TabId};
use crate::kit::prelude::*;
use crate::testing;
use glint_core::fsx::TextPreview as TextRead;
use glint_core::git::{DiffLine, FileDiff, LineKind};
use glint_core::repo::{Author, Lines, RepoCard as Card};
use glint_model::entries::GitMark;
use glint_model::groups::GitMarks;
use glint_model::paths::THIS_PC;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{ElementId, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ---- A fake Git ---------------------------------------------------------------------------------------------

/// One status request the model made.
#[derive(Clone, Debug, PartialEq)]
struct Asked {
    folder: String,
    lane: String,
    trusted: Vec<String>,
    force: bool,
}

#[derive(Default)]
struct State {
    views: HashMap<String, View>,
    asked: Vec<Asked>,
    cancelled: Vec<String>,
    watched: Vec<Vec<String>>,
    diff: Option<Result<Option<FileDiff>, String>>,
    head: Option<Result<Option<String>, String>>,
    discarded: Vec<String>,
    card: Option<Result<Option<Card>, String>>,
    cards_asked: usize,
    gates: HashMap<String, (async_channel::Sender<()>, async_channel::Receiver<()>)>,
}

/// The fake source: answers with the views it was given, can hold an answer back, and sees Stop and watching.
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<State>>);

fn key(path: &str) -> String {
    path.trim_end_matches(['\\', '/']).to_lowercase()
}

impl Fake {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Says folder `path` is in a repository with this view.
    fn put(&self, path: &str, view: View) {
        self.state().views.insert(key(path), view);
    }

    fn hold(&self, path: &str) {
        let gate = async_channel::bounded(1);
        self.state().gates.insert(key(path), gate);
    }

    fn release(&self, path: &str) {
        // The sender goes: every waiting (and later) receive ends.
        self.state().gates.remove(&key(path));
    }

    fn asked(&self) -> Vec<Asked> {
        self.state().asked.clone()
    }
}

impl Source for Fake {
    fn view(&self, folder: String, lane: String, trusted: Vec<String>, force: bool) -> Fut<Option<View>> {
        let (view, gate) = {
            let mut state = self.state();
            state.asked.push(Asked {
                folder: folder.clone(),
                lane,
                trusted,
                force,
            });
            (
                state.views.get(&key(&folder)).cloned(),
                state.gates.get(&key(&folder)).map(|(_, receiver)| receiver.clone()),
            )
        };
        Box::pin(async move {
            if let Some(gate) = gate {
                let _ = gate.recv().await;
            }
            Ok(view)
        })
    }

    fn cancel(&self, lane: &str) {
        self.state().cancelled.push(lane.to_string());
    }

    fn watch(&self, roots: Vec<String>) {
        self.state().watched.push(roots);
    }

    fn diff(&self, _: String) -> Fut<Option<FileDiff>> {
        let answer = self.state().diff.clone().unwrap_or(Ok(None));
        Box::pin(async move { answer })
    }

    fn head_copy(&self, path: String) -> Fut<Option<String>> {
        let answer = self.state().head.clone().unwrap_or_else(|| {
            let name = glint_model::paths::base_name(&path).to_string();
            Ok(Some(format!("C:\\Temp\\Glint\\head\\1\\{name}")))
        });
        Box::pin(async move { answer })
    }

    fn head_discard(&self, path: String) {
        self.state().discarded.push(path);
    }

    fn card(&self, _: String) -> Fut<Option<Card>> {
        let answer = {
            let mut state = self.state();
            state.cards_asked += 1;
            state.card.clone().unwrap_or(Ok(None))
        };
        Box::pin(async move { answer })
    }
}

// ---- Fixtures -----------------------------------------------------------------------------------------------

/// `C:\repo`'s view: on main, two ahead and one behind, a dirty tree with a name in Arabic among its marks.
fn repo_view() -> View {
    let mut marks = GitMarks::new();
    marks.insert("a.txt".into(), GitMark::Modified);
    marks.insert("b.rs".into(), GitMark::Staged);
    marks.insert("ملف.txt".into(), GitMark::Untracked);
    marks.insert("target".into(), GitMark::Ignored);
    marks.insert("src".into(), GitMark::Conflicted);
    View {
        root: "C:\\repo".into(),
        branch: Some("main".into()),
        head: Some("abc1234".into()),
        upstream: Some("origin/main".into()),
        ahead: 2,
        behind: 1,
        marks: Arc::new(marks),
        counts: Counts {
            modified: 3,
            staged: 1,
            untracked: 4,
            conflicted: 1,
        },
        elapsed: 12.4,
        ..Default::default()
    }
}

/// `C:\repo` (with its `.git`, files, one named in Arabic, and a README), its `src`, and `C:\plain`.
fn standard_disk(disk: &FakeDisk) {
    disk.folder(
        "C:\\repo",
        vec![
            folder("C:\\repo", ".git"),
            folder("C:\\repo", "src"),
            folder("C:\\repo", "target"),
            file("C:\\repo", "a.txt", 10, 1),
            file("C:\\repo", "b.rs", 20, 2),
            file("C:\\repo", "ملف.txt", 30, 3),
            file("C:\\repo", "README.md", 40, 4),
        ],
    );
    disk.folder("C:\\repo\\src", vec![file("C:\\repo\\src", "lib.rs", 5, 1)]);
    disk.folder("C:\\plain", vec![file("C:\\plain", "notes.txt", 5, 1)]);
    disk.folder("C:\\", vec![folder("C:\\", "repo"), folder("C:\\", "plain")]);
}

struct Env {
    visual: VisualTestContext,
    explorer: Entity<Explorer>,
    git: Entity<Git>,
    fake: Fake,
    tab: TabId,
}

/// A window holding whatever views the test makes.
struct Host {
    children: Vec<AnyView>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        col().size_full().children(self.children.clone())
    }
}

/// The skeleton's globals, a fake disk, a fake Git, and a window with `build`'s views.
fn setup(
    cx: &mut TestAppContext,
    rtl: bool,
    fake: Fake,
    build: impl FnOnce(TabId, &mut Window, &mut App) -> Vec<AnyView>,
) -> Env {
    cx.update(testing::init);
    testing::set_rtl(rtl);
    let disk = FakeDisk::new();
    standard_disk(&disk);
    cx.update(|cx| {
        explorer::init_with(cx, disk.source());
        model::init_with(cx, Rc::new(fake.clone()));
    });
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let git = cx.update(shared);
    let tab = explorer.read_with(cx, |e, _| e.pane_tab_id(0));
    let (visual, _) = testing::open_window(cx, move |window, cx| {
        let children = build(tab, window, cx);
        cx.new(|_| Host { children })
    });
    Env {
        visual,
        explorer,
        git,
        fake,
        tab,
    }
}

impl Env {
    /// Opens `path` in the tab, as the person would, and lets the listing arrive.
    fn go(&mut self, path: &str) {
        let (explorer, path) = (self.explorer.clone(), path.to_string());
        self.visual.update(|_, cx| {
            explorer.update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx));
        });
        self.visual.run_until_parked();
    }

    /// Runs the executors and draws frames until `done` holds.
    fn settle_until(&mut self, what: &str, mut done: impl FnMut(&mut Env) -> bool) {
        for _ in 0..400 {
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        panic!("never happened: {what}");
    }

    fn settle(&mut self) {
        for _ in 0..8 {
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn view(&mut self) -> Option<Arc<View>> {
        let tab = self.tab;
        self.visual.update(|_, cx| model::view_of(cx, tab))
    }

    fn marks(&mut self) -> Option<Arc<GitMarks>> {
        let tab = self.tab;
        self.visual.update(|_, cx| marks(cx, tab))
    }

    fn click(&mut self, id: impl Into<ElementId>) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.visual.run_until_parked();
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.visual.run_until_parked();
    }

    fn has(&mut self, label: &str) -> bool {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(&mut self, label: &str) -> Option<Role> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|e| e.role())
        })
    }

    fn filter(&mut self) -> String {
        let tab = self.tab;
        self.explorer.read_with(&self.visual, |e, _| {
            e.tab(tab).map(|t| t.filter.clone()).unwrap_or_default()
        })
    }
}

/// An env whose window has the tab's chip.
fn with_chip(cx: &mut TestAppContext, rtl: bool, fake: Fake) -> (Env, Entity<GitChip>) {
    let made: Rc<RefCell<Option<Entity<GitChip>>>> = Rc::new(RefCell::new(None));
    let slot = made.clone();
    let env = setup(cx, rtl, fake, move |tab, _, cx| {
        let chip = cx.new(|cx| GitChip::new(tab, cx));
        *slot.borrow_mut() = Some(chip.clone());
        // In a row, as the address bar holds it: the chip is as wide as its content.
        let row = cx.new(|_| ChipRow { chip });
        vec![row.into()]
    });
    let chip = made.borrow().clone().expect("the chip was made");
    (env, chip)
}

/// The address bar's row, in the reading direction.
struct ChipRow {
    chip: Entity<GitChip>,
}

impl Render for ChipRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        row_start().w_full().items_start().child(self.chip.clone())
    }
}

fn standard_fake() -> Fake {
    let fake = Fake::default();
    fake.put("C:\\repo", repo_view());
    fake.put("C:\\repo\\src", repo_view());
    fake
}

const CHIP_LABEL: &str = "Git: on main, tracking origin/main";

// ---- The model: marks follow the folder --------------------------------------------------------------------

#[gpui_kit::test]
fn the_marks_follow_the_folder_the_tab_is_in(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, standard_fake(), |_, _, _| Vec::new());
    env.go("C:\\repo");
    env.settle_until("the view arrives", |env| env.view().is_some());
    let view = env.view().expect("a view");
    assert_eq!(view.branch.as_deref(), Some("main"));
    let marks = env.marks().expect("marks");
    assert_eq!(marks.get("a.txt"), Some(&GitMark::Modified));
    assert_eq!(marks.get("ملف.txt"), Some(&GitMark::Untracked));
    // The mark of one item, by its path, in any tab's folder (`useGitMark`): case does not matter.
    env.visual.update(|_, cx| {
        assert_eq!(mark_of(cx, "C:\\repo\\A.TXT"), Some(GitMark::Modified));
        assert_eq!(mark_of(cx, "C:\\repo\\ملف.txt"), Some(GitMark::Untracked));
        assert_eq!(mark_of(cx, "C:\\repo\\target"), Some(GitMark::Ignored));
        assert_eq!(mark_of(cx, "C:\\repo\\nothing.txt"), None);
        assert_eq!(mark_of(cx, "C:\\elsewhere\\a.txt"), None);
        assert_eq!(mark_of(cx, "noseparator"), None);
    });
    // The first status was asked for the tab's folder, by the tab's lane, plainly (not forced).
    let asked = env.fake.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].folder, "C:\\repo");
    assert_eq!(asked[0].lane, env.tab.to_string());
    assert!(!asked[0].force);
    // The repositories of the tabs on screen are the ones watched.
    assert_eq!(env.fake.state().watched.last(), Some(&vec!["C:\\repo".to_string()]));

    // A folder in no repository has none, and the status that was running for the tab is stopped.
    env.go("C:\\plain");
    env.settle_until("the marks go", |env| env.marks().is_none());
    assert!(env.fake.state().cancelled.contains(&env.tab.to_string()) || env.view().is_none());
    env.visual.update(|_, cx| assert!(!model::in_repository(cx, env.tab)));
}

#[gpui_kit::test]
fn this_pc_and_a_closed_repository_have_no_git(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, standard_fake(), |_, _, _| Vec::new());
    env.go("C:\\repo");
    env.settle_until("the view arrives", |env| env.view().is_some());
    env.go(THIS_PC);
    env.settle_until("This PC has none", |env| env.view().is_none());
    assert!(env.fake.state().cancelled.contains(&env.tab.to_string()));
    // The watch follows: nothing on screen to watch.
    env.settle_until("nothing is watched", |env| {
        env.fake.state().watched.last() == Some(&Vec::new())
    });
}

#[gpui_kit::test]
fn a_subfolder_shows_its_own_marks_and_the_repository_it_is_in(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = standard_fake();
    let mut sub = repo_view();
    sub.marks = Arc::new(GitMarks::from([("lib.rs".to_string(), GitMark::Modified)]));
    fake.put("C:\\repo\\src", sub);
    let mut env = setup(cx, false, fake, |_, _, _| Vec::new());
    env.go("C:\\repo\\src");
    env.settle_until("the view arrives", |env| env.view().is_some());
    let marks = env.marks().expect("marks");
    assert_eq!(marks.len(), 1);
    assert_eq!(marks.get("lib.rs"), Some(&GitMark::Modified));
    // The status bar's words are those of this folder's view.
    let tab = env.tab;
    let line = env.visual.update(|_, cx| status_line(cx, tab)).expect("a line");
    assert_eq!(line.said, "git · 12 ms");
    let stage = env.visual.update(|_, cx| super::stage(cx, tab)).expect("a stage");
    assert_eq!((stage.0.as_ref(), stage.1.as_ref()), ("Git status", "main"));
}

#[gpui_kit::test]
fn asking_again_while_status_runs_asks_once_more_after_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = standard_fake();
    fake.hold("C:\\repo");
    let mut env = setup(cx, false, fake.clone(), |_, _, _| Vec::new());
    env.go("C:\\repo");
    env.settle();
    assert_eq!(fake.asked().len(), 1, "status is running");
    // A build writes all the time: three changes while it runs.
    for _ in 0..3 {
        let tab = env.tab;
        env.visual.update(|_, cx| model::refresh(cx, tab, false));
    }
    env.settle();
    assert_eq!(fake.asked().len(), 1, "no new run starts over the one in flight");
    fake.release("C:\\repo");
    env.settle_until("the follow-up run", |_| fake.asked().len() == 2);
    env.settle();
    assert_eq!(fake.asked().len(), 2, "once more after it, not three times");
    env.settle_until("the view arrives", |env| env.view().is_some());
    // A change in the repository on screen (core's event) makes its tabs look again.
    env.visual
        .update(|_, cx| crate::core_bridge::emit(cx, crate::core_bridge::CoreEvent::GitChanged("C:\\repo".into())));
    env.settle_until("the event's run", |_| fake.asked().len() == 3);
    // Another repository's change does nothing.
    env.visual
        .update(|_, cx| crate::core_bridge::emit(cx, crate::core_bridge::CoreEvent::GitChanged("C:\\other".into())));
    env.settle();
    assert_eq!(fake.asked().len(), 3);
}

#[gpui_kit::test]
fn a_newer_request_for_another_folder_beats_the_one_in_flight(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = standard_fake();
    fake.hold("C:\\repo");
    let mut env = setup(cx, false, fake.clone(), |_, _, _| Vec::new());
    env.go("C:\\repo");
    env.settle();
    env.go("C:\\plain");
    env.settle_until("the plain folder was asked", |_| fake.asked().len() == 2);
    // The slow answer for the old folder arrives late: it must not bring the repository back.
    fake.release("C:\\repo");
    env.settle();
    assert!(env.view().is_none());
    assert!(env.marks().is_none());
}

#[gpui_kit::test]
fn the_model_tells_which_tab_changed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = setup(cx, false, standard_fake(), |_, _, _| Vec::new());
    let seen: Rc<RefCell<Vec<TabId>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = seen.clone();
    let git = env.git.clone();
    env.visual.update(|_, cx| {
        cx.subscribe(&git, move |_, event: &GitEvent, _| {
            let GitEvent::Changed(tab) = event;
            sink.borrow_mut().push(*tab);
        })
        .detach();
    });
    env.go("C:\\repo");
    env.settle_until("the view arrives", |env| env.view().is_some());
    assert!(seen.borrow().contains(&env.tab));
}

// ---- The chip -----------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_chip_names_the_branch_and_how_far_it_is_from_its_upstream(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, chip) = with_chip(cx, false, standard_fake());
    // Nothing yet: a tab in no repository draws no chip.
    assert!(!env.has(CHIP_LABEL));
    env.go("C:\\repo");
    env.settle_until("the chip", |env| env.has(CHIP_LABEL));
    assert_eq!(env.role_of(CHIP_LABEL), Some(Role::Button));
    assert!(chip.read_with(&env.visual, |chip, cx| chip.is_shown(cx)));
    // Going to a folder in no repository takes it away.
    env.go("C:\\plain");
    env.settle_until("no chip", |env| !env.has(CHIP_LABEL));
    assert!(!chip.read_with(&env.visual, |chip, cx| chip.is_shown(cx)));
}

#[gpui_kit::test]
fn a_detached_head_and_a_new_repository_are_named_as_the_typescript_named_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.put(
        "C:\\repo",
        View {
            branch: None,
            ..repo_view()
        },
    );
    let (mut env, _) = with_chip(cx, false, fake.clone());
    env.go("C:\\repo");
    env.settle_until("the chip", |env| {
        env.has("Git: HEAD detached at abc1234, tracking origin/main")
    });
    fake.put(
        "C:\\repo",
        View {
            branch: None,
            head: None,
            upstream: None,
            ..repo_view()
        },
    );
    let tab = env.tab;
    env.visual.update(|_, cx| model::refresh(cx, tab, true));
    env.settle_until("the new repository", |env| {
        env.has("Git: a repository with no commits yet")
    });
}

#[gpui_kit::test]
fn the_popover_shows_a_dirty_tree_and_its_buttons_filter_the_folder(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, chip) = with_chip(cx, false, standard_fake());
    env.go("C:\\repo");
    env.settle_until("the chip", |env| env.has(CHIP_LABEL));
    let tab = env.tab.0 as usize;
    assert!(!env.has("Git: repo"));
    env.click(("git-chip", tab));
    assert!(chip.read_with(&env.visual, |chip, _| chip.is_open()));
    assert_eq!(env.role_of("Git: repo"), Some(Role::Group), "the popover is named");
    // One pill a kind of change, each with its count, in the TypeScript's order.
    for label in ["1 Conflicts", "3 Modified", "1 Staged", "4 Untracked"] {
        assert!(env.has(label), "{label}");
    }
    // The copy buttons.
    assert!(env.has("Click to copy branch \"main\""));
    assert!(env.has("Click to copy commit SHA (abc1234)"));

    // A pill filters the folder by its kind and closes the popover.
    env.click(("git-pill", 1usize));
    assert_eq!(env.filter(), "git:modified");
    assert!(!chip.read_with(&env.visual, |chip, _| chip.is_open()));
    assert!(!env.has("Git: repo"));

    // Show changes: git:changed.
    env.click(("git-chip", tab));
    env.click(("git-show-changes", tab));
    assert_eq!(env.filter(), "git:changed");
    assert!(!chip.read_with(&env.visual, |chip, _| chip.is_open()));
}

#[gpui_kit::test]
fn the_popover_copies_the_branch_and_the_commit(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, _) = with_chip(cx, false, standard_fake());
    env.go("C:\\repo");
    env.settle_until("the chip", |env| env.has(CHIP_LABEL));
    let tab = env.tab.0 as usize;
    env.click(("git-chip", tab));
    env.click(("git-copy-branch", tab));
    let copied = env.visual.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("main"));
    env.click(("git-copy-sha", tab));
    let copied = env.visual.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("abc1234"));
}

#[gpui_kit::test]
fn escape_and_a_press_outside_close_the_popover(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, chip) = with_chip(cx, false, standard_fake());
    env.go("C:\\repo");
    env.settle_until("the chip", |env| env.has(CHIP_LABEL));
    let tab = env.tab.0 as usize;
    env.click(("git-chip", tab));
    assert!(chip.read_with(&env.visual, |chip, _| chip.is_open()));
    env.press("escape");
    assert!(
        !chip.read_with(&env.visual, |chip, _| chip.is_open()),
        "Escape closes it"
    );
    // The chip's own button opens and closes it.
    env.click(("git-chip", tab));
    assert!(chip.read_with(&env.visual, |chip, _| chip.is_open()));
    env.click(("git-chip", tab));
    assert!(
        !chip.read_with(&env.visual, |chip, _| chip.is_open()),
        "its second click closes it"
    );
}

#[gpui_kit::test]
fn the_repository_folder_button_shows_only_below_the_root(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, _) = with_chip(cx, false, standard_fake());
    env.go("C:\\repo");
    env.settle_until("the chip", |env| env.has(CHIP_LABEL));
    let tab = env.tab.0 as usize;
    env.click(("git-chip", tab));
    assert!(!env.has("Open the repository's folder"), "already at the root");
    env.press("escape");

    env.go("C:\\repo\\src");
    env.settle_until("the chip in src", |env| env.has(CHIP_LABEL));
    env.click(("git-chip", tab));
    assert!(env.has("Open the repository's folder"));
    env.click(("git-open-root", tab));
    env.settle_until("back at the root", |env| {
        let tab = env.tab;
        env.explorer
            .read_with(&env.visual, |e, _| e.tab(tab).is_some_and(|t| t.path == "C:\\repo"))
    });
}

#[gpui_kit::test]
fn a_repository_that_waits_for_trust_offers_to_trust_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.put(
        "C:\\repo",
        View {
            limited: true,
            marks: Arc::new(GitMarks::new()),
            counts: Counts::default(),
            ..repo_view()
        },
    );
    let (mut env, _) = with_chip(cx, false, fake.clone());
    env.go("C:\\repo");
    env.settle_until("the chip", |env| env.has(CHIP_LABEL));
    let tab = env.tab.0 as usize;
    env.click(("git-chip", tab));
    assert!(env.has("Git status waits for trust"));
    assert!(!env.has("Show changes"), "nothing is known of what changed");
    // The status bar says it waits too.
    let line = env.visual.update(|_, cx| status_line(cx, env.tab)).expect("a line");
    assert_eq!(line.said, "git · waits for trust");

    // Trusting: the repository goes in Settings, and status runs again, forced, with it trusted.
    fake.put("C:\\repo", repo_view());
    env.click(("git-trust", tab));
    let trusted = env
        .visual
        .update(|_, cx| crate::state::settings(cx).git_trusted.clone());
    assert_eq!(trusted, vec!["C:\\repo".to_string()]);
    env.settle_until("status runs trusted", |_| fake.asked().len() == 2);
    let second = fake.asked()[1].clone();
    assert!(second.force && second.trusted == vec!["C:\\repo".to_string()]);
    env.settle_until("the real view", |env| env.view().is_some_and(|v| !v.limited));
}

#[gpui_kit::test]
fn a_repository_too_big_to_look_through_shows_its_status_when_asked(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.put(
        "C:\\repo",
        View {
            on_request: true,
            marks: Arc::new(GitMarks::new()),
            counts: Counts::default(),
            ..repo_view()
        },
    );
    let (mut env, _) = with_chip(cx, false, fake.clone());
    env.go("C:\\repo");
    env.settle_until("the chip", |env| env.has(CHIP_LABEL));
    let tab = env.tab.0 as usize;
    env.click(("git-chip", tab));
    assert!(env.has("Git status waits to be asked"));
    let line = env.visual.update(|_, cx| status_line(cx, env.tab)).expect("a line");
    assert_eq!(line.said, "git · F5 for status");
    assert!(env.visual.update(|_, cx| super::stage(cx, env.tab)).is_none());
    fake.put("C:\\repo", repo_view());
    env.click(("git-status", tab));
    env.settle_until("status runs forced", |_| fake.asked().len() == 2);
    assert!(fake.asked()[1].force);
}

#[gpui_kit::test]
fn the_chip_mirrors_in_arabic_and_hangs_its_popover_from_the_start_edge(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.put(
        "C:\\repo",
        View {
            branch: Some("فرع-رئيسي".into()),
            ..repo_view()
        },
    );
    let (mut env, chip) = with_chip(cx, true, fake);
    env.go("C:\\repo");
    let label = "Git: on فرع-رئيسي, tracking origin/main";
    env.settle_until("the chip", |env| env.has(label));
    let tab = env.tab.0 as usize;
    let button = env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.find(("git-chip", tab)).bounds()
    });
    // In the Arabic interface the chip sits at the right edge of its row.
    assert!(button.center().x > gpui_kit::px(500.), "{button:?}");
    env.click(("git-chip", tab));
    assert!(chip.read_with(&env.visual, |chip, _| chip.is_open()));
    let panel = env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.find(("git-popover", tab)).bounds()
    });
    // Hung from the chip's start edge, which is its right edge: the popover ends where the chip ends.
    assert!(
        (panel.right() - button.right()).abs() <= gpui_kit::px(8.),
        "{panel:?} under {button:?}"
    );
    assert!(panel.left() >= gpui_kit::px(0.), "kept on screen");
    assert!(env.has("Click to copy branch \"فرع-رئيسي\""));
}

// ---- The marks in rows --------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_row_badge_is_named_for_its_row_and_tinted_for_its_mark(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let env = setup(cx, true, Fake::default(), |_, _, cx| {
        struct Badges;
        impl Render for Badges {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let marks = repo_view().marks;
                let names = ["a.txt", "ملف.txt", "src", "b.rs"];
                col().size_full().children(names.iter().enumerate().map(|(i, name)| {
                    let entry = if *name == "src" {
                        folder("C:\\repo", name)
                    } else {
                        file("C:\\repo", name, 1, 1)
                    };
                    let look = words::row_git(&entry, Some(&marks));
                    row_start().children(
                        look.badge
                            .map(|badge| row::badge_element(("git-badge", i), &entry.name, &badge, cx)),
                    )
                }))
            }
        }
        vec![cx.new(|_| Badges).into()]
    });
    let mut env = env;
    env.settle();
    assert_eq!(env.role_of("Changed: a.txt"), Some(Role::Group));
    assert!(!env.has("Changed: ملف.txt"));
    // The Arabic name is untracked: "New" (the word is translated in the Arabic interface).
    let word = |mark, dir| words::badge(mark, dir).map(|b| b.label.to_string()).unwrap_or_default();
    assert!(env.has(&format!("{}: ملف.txt", word(GitMark::Untracked, false))));
    assert!(env.has(&format!("{}: src", word(GitMark::Conflicted, true))));
    assert!(env.has(&format!("{}: b.rs", word(GitMark::Staged, false))));
    assert_eq!(
        words::badge(GitMark::Modified, false).map(|b| b.tone),
        Some(Tone::Amber)
    );
    assert_eq!(words::badge(GitMark::Untracked, false).map(|b| b.tone), Some(Tone::Sky));
}

// ---- The repository card ------------------------------------------------------------------------------------

fn a_card() -> Card {
    Card {
        onefetch: true,
        tokei: true,
        name: Some("glint".into()),
        description: Some("A fast file explorer".into()),
        head: Some("abc1234".into()),
        branch: Some("main".into()),
        version: Some("0.9.5".into()),
        created: None,
        last_change: Some("2026-11-09T12:00:00+00:00".into()),
        languages: vec![("Rust".into(), 62.5), ("TypeScript".into(), 30.0), ("CSS".into(), 0.4)],
        authors: vec![
            Author {
                name: "Rafd".into(),
                commits: 900,
                share: 90,
            },
            Author {
                name: "رافد".into(),
                commits: 100,
                share: 10,
            },
        ],
        contributors: Some(5),
        commits: Some(1234),
        url: Some("git@github.com:rafd/glint.git".into()),
        license: Some("MIT".into()),
        lines: vec![Lines {
            language: "Rust".into(),
            code: 90_000,
            comments: 5_000,
            blanks: 1_000,
        }],
        total: Some(Lines {
            language: "Total".into(),
            code: 120_000,
            comments: 6_000,
            blanks: 2_000,
        }),
    }
}

fn with_repo_card(cx: &mut TestAppContext, rtl: bool, fake: Fake) -> (Env, Entity<RepoCard>) {
    let made: Rc<RefCell<Option<Entity<RepoCard>>>> = Rc::new(RefCell::new(None));
    let slot = made.clone();
    let env = setup(cx, rtl, fake, move |_, _, cx| {
        let card = cx.new(|cx| RepoCard::new("C:\\repo".into(), None, cx));
        *slot.borrow_mut() = Some(card.clone());
        vec![card.into()]
    });
    let card = made.borrow().clone().expect("the card was made");
    (env, card)
}

#[gpui_kit::test]
fn the_repository_card_says_what_the_repository_is(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().card = Some(Ok(Some(a_card())));
    let (mut env, card) = with_repo_card(cx, false, fake.clone());
    env.settle_until("the card", |env| env.has("Repository: glint"));
    assert_eq!(env.role_of("Repository: glint"), Some(Role::Group));
    // The languages, biggest first, in whole percent and "<1" for a sliver.
    for label in ["Rust 63%", "TypeScript 30%", "CSS <1%"] {
        assert!(env.has(label), "{label}");
    }
    assert_eq!(env.role_of("Languages"), Some(Role::List));
    // The remote becomes a web address; reading it again is not needed for a folder seen before.
    assert!(env.has("github.com/rafd/glint"));
    // Seen again, the card is there at once from the model's cache.
    let cached = env.git.read_with(&env.visual, |git, _| git.cached_card("c:\\REPO"));
    assert!(cached.is_some_and(|c| c.is_some_and(|c| c.name.as_deref() == Some("glint"))));
    assert_eq!(card.read_with(&env.visual, |c, _| c.path().to_string()), "C:\\repo");
}

#[gpui_kit::test]
fn a_folder_that_is_not_a_repository_has_no_card(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    let (mut env, card) = with_repo_card(cx, false, fake.clone());
    env.settle_until("the answer", |_| fake.state().cards_asked == 1);
    env.settle();
    assert!(!env.has("Repository: C:\\repo"));
    assert!(card.read_with(&env.visual, |c, _| matches!(c.card(), Some(None))));
    // A failed read shows nothing either, and is not remembered.
    let failing = Fake::default();
    failing.state().card = Some(Err("boom".into()));
    let (mut env, _) = with_repo_card(cx, false, failing);
    env.settle();
    assert!(env
        .git
        .read_with(&env.visual, |git, _| git.cached_card("C:\\repo").is_none()));
}

#[gpui_kit::test]
fn the_card_reads_in_arabic_with_an_arabic_author(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().card = Some(Ok(Some(a_card())));
    let (mut env, _) = with_repo_card(cx, true, fake);
    env.settle_until("the card", |env| env.has("Repository: glint"));
    let bounds = env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.find("repo-card").bounds()
    });
    assert!(bounds.size.width > gpui_kit::px(100.));
    // The card is a column that fills from the start edge: its legend starts at the right in Arabic.
    let first = env.visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find(("repo-language", 0usize)).bounds(),
            window.find(("repo-language", 1usize)).bounds(),
        )
    });
    assert!(
        first.0.center().x > first.1.center().x,
        "the legend runs right to left: {first:?}"
    );
}

// ---- The README ---------------------------------------------------------------------------------------------

fn read_of(text: &str) -> TextRead {
    TextRead {
        text: text.to_string(),
        truncated: false,
        binary: false,
        encoding: None,
        guessed: false,
    }
}

fn text_source(read: Result<TextRead, String>) -> crate::components::preview::text::TextSource {
    Arc::new(move |_, _| {
        let answer = match &read {
            Ok(r) => Ok(TextRead {
                text: r.text.clone(),
                truncated: r.truncated,
                binary: r.binary,
                encoding: r.encoding.clone(),
                guessed: r.guessed,
            }),
            Err(e) => Err(e.clone()),
        };
        Box::pin(async move { answer })
    })
}

fn with_readme(cx: &mut TestAppContext, rtl: bool, read: Result<TextRead, String>) -> (Env, Entity<ReadmeCard>) {
    let made: Rc<RefCell<Option<Entity<ReadmeCard>>>> = Rc::new(RefCell::new(None));
    let slot = made.clone();
    let env = setup(cx, rtl, Fake::default(), move |_, window, cx| {
        let entry = file("C:\\repo", "README.md", 40, 4);
        let card = cx.new(|cx| ReadmeCard::with_source(entry, text_source(read), window, cx));
        *slot.borrow_mut() = Some(card.clone());
        vec![card.into()]
    });
    let card = made.borrow().clone().expect("the card was made");
    (env, card)
}

#[gpui_kit::test]
fn a_readme_is_folded_to_its_title_until_it_is_opened(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, card) = with_readme(cx, false, Ok(read_of("# Glint\n\nA fast file explorer.\n")));
    env.settle_until("the title", |env| {
        card.read_with(&env.visual, |c, _| c.title() == Some("Glint"))
    });
    assert_eq!(env.role_of("README: README.md"), Some(Role::Button));
    assert!(!env.has("README.md"), "folded: no document yet");
    assert!(card.read_with(&env.visual, |c, _| c.document().is_none()));
    assert!(!env.visual.update(|_, cx| crate::state::settings(cx).readme_open));

    // Opening it is kept in Settings, from folder to folder.
    env.click("readme-toggle");
    assert!(env.visual.update(|_, cx| crate::state::settings(cx).readme_open));
    env.settle_until("the document", |env| {
        card.read_with(&env.visual, |c, _| c.document().is_some()) && env.has("README.md")
    });
    let document = card
        .read_with(&env.visual, |c, _| c.document().cloned())
        .expect("a document");
    env.settle_until("the Markdown is parsed", |env| {
        document.read_with(&env.visual, |d, cx| d.rendered_text(cx).is_some())
    });
    let text = document
        .read_with(&env.visual, |d, cx| d.rendered_text(cx))
        .unwrap_or_default();
    assert!(text.contains("A fast file explorer."), "{text}");
    // Folding it again.
    env.click("readme-toggle");
    assert!(!env.visual.update(|_, cx| crate::state::settings(cx).readme_open));
}

#[gpui_kit::test]
fn a_readme_in_arabic_shows_its_title_and_its_text(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, card) = with_readme(cx, true, Ok(read_of("# مرحبا بكم في المشروع\n\nنص عربي طويل.\n")));
    env.visual
        .update(|_, cx| crate::state::update(cx, |s| s.readme_open = true));
    env.settle_until("the title", |env| {
        card.read_with(&env.visual, |c, _| c.title() == Some("مرحبا بكم في المشروع"))
    });
    env.settle_until("the document", |env| {
        card.read_with(&env.visual, |c, _| c.document().is_some())
    });
    let document = card
        .read_with(&env.visual, |c, _| c.document().cloned())
        .expect("a document");
    env.settle_until("the Markdown is parsed", |env| {
        document.read_with(&env.visual, |d, cx| d.rendered_text(cx).is_some())
    });
    let text = document
        .read_with(&env.visual, |d, cx| d.rendered_text(cx))
        .unwrap_or_default();
    assert!(text.contains("نص عربي طويل."), "{text}");
    // The header's chevron points to the end in the Arabic interface, and the card is a group with a name.
    assert_eq!(env.role_of("README"), Some(Role::Group));
}

#[gpui_kit::test]
fn a_readme_that_cannot_be_shown_draws_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    for read in [
        Ok(read_of("")),
        Ok(TextRead {
            binary: true,
            ..read_of("\u{0}\u{1}")
        }),
        Err("gone".to_string()),
    ] {
        let (mut env, card) = with_readme(cx, false, read);
        env.settle_until("hidden", |env| card.read_with(&env.visual, |c, _| !c.is_shown()));
        assert!(!env.has("README"));
        assert!(!env.has("README: README.md"));
    }
}

#[gpui_kit::test]
fn the_cross_stops_showing_readmes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut env, card) = with_readme(cx, false, Ok(read_of("# T\n")));
    env.settle_until("shown", |env| card.read_with(&env.visual, |c, _| c.title().is_some()));
    assert!(env.visual.update(|_, cx| crate::state::settings(cx).show_readme));
    env.click("readme-hide");
    assert!(!env.visual.update(|_, cx| crate::state::settings(cx).show_readme));
}

// ---- The Changes view ---------------------------------------------------------------------------------------

fn line(kind: LineKind, old: u32, new: u32, text: &str) -> DiffLine {
    DiffLine {
        kind,
        old,
        new,
        text: text.into(),
    }
}

fn a_diff() -> FileDiff {
    FileDiff {
        path: "src/ملف.txt".into(),
        added: 2,
        removed: 1,
        old_size: 100,
        new_size: 110,
        lines: vec![
            line(LineKind::Same, 1, 1, "one"),
            line(LineKind::Removed, 2, 0, "two"),
            line(LineKind::Added, 0, 2, "اثنان"),
            line(LineKind::Added, 0, 3, "three"),
            // Lines left out between two changes: a gap in the numbers.
            line(LineKind::Same, 20, 21, "twenty"),
        ],
        elapsed: 1.5,
        ..Default::default()
    }
}

type Recorded = Rc<RefCell<Vec<(Side, Side, Option<OnClose>)>>>;

fn with_changes(cx: &mut TestAppContext, rtl: bool, fake: Fake) -> (Env, Entity<ChangesView>, Recorded) {
    let made: Rc<RefCell<Option<Entity<ChangesView>>>> = Rc::new(RefCell::new(None));
    let slot = made.clone();
    let recorded: Recorded = Rc::new(RefCell::new(Vec::new()));
    let sink = recorded.clone();
    let source = fake.clone();
    let env = setup(cx, rtl, fake, move |_, _, cx| {
        let open: OpenSides = Rc::new(move |_, _, left, right, on_close| {
            sink.borrow_mut().push((left, right, on_close));
        });
        let entry = file("C:\\repo\\src", "ملف.txt", 110, 7);
        let view = cx.new(|cx| ChangesView::with_parts(entry, Rc::new(source), open, cx));
        *slot.borrow_mut() = Some(view.clone());
        vec![view.into()]
    });
    let view = made.borrow().clone().expect("the view was made");
    (env, view, recorded)
}

#[gpui_kit::test]
fn the_changes_view_lists_the_lines_that_changed_with_a_gap_between_distant_ones(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(a_diff())));
    let (mut env, _, _) = with_changes(cx, false, fake);
    env.settle_until("the lines", |env| env.has("Added line 2: اثنان"));
    assert_eq!(env.role_of("Changes in ملف.txt"), Some(Role::Table));
    assert_eq!(env.role_of("Removed line 2: two"), Some(Role::Row));
    assert!(env.has("Line 1: one"));
    assert!(env.has("Added line 3: three"));
    assert!(env.has("Line 21: twenty"));
    // The summary counts what was added and removed.
    assert!(env.has("+2 −1 since the last commit"));
    // The 17 lines left out between the changes are one marker.
    let gaps: Vec<usize> = env.visual.update(|window, cx| {
        window.render_frame(cx);
        (0..8usize)
            .filter(|i| window.try_find(("changes-gap", *i)).is_some())
            .collect()
    });
    // After the fourth line (items 0 to 3), before the one numbered 20.
    assert_eq!(gaps, vec![4]);
}

#[gpui_kit::test]
fn the_changes_view_reads_in_arabic_mirrored(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(a_diff())));
    let (mut env, _, _) = with_changes(cx, true, fake);
    env.settle_until("the lines", |env| env.has("Added line 2: اثنان"));
    let bounds = env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.find(("changes-line", 0usize)).bounds()
    });
    assert!(bounds.size.width > gpui_kit::px(200.), "{bounds:?}");
}

#[gpui_kit::test]
fn a_new_file_a_binary_one_and_a_big_one_say_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    // New: all of it is new, no side by side.
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(FileDiff {
        new_file: true,
        added: 3,
        new_size: 2048,
        lines: vec![line(LineKind::Added, 0, 1, "x")],
        ..a_diff()
    })));
    let (mut env, _, _) = with_changes(cx, false, fake);
    env.settle_until("new", |env| env.has("Added line 1: x"));
    assert!(env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find("changes-side-by-side").is_none()
    }));

    // Binary.
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(FileDiff {
        binary: true,
        old_size: 1024,
        new_size: 2048,
        lines: vec![],
        ..a_diff()
    })));
    let (mut env, _, _) = with_changes(cx, false, fake);
    env.settle_until("binary", |env| {
        env.has("A binary file: what changed in it isn't shown as lines.")
    });
    assert!(env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find("changes-side-by-side").is_some()
    }));

    // Too big.
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(FileDiff {
        too_big: true,
        lines: vec![],
        ..a_diff()
    })));
    let (mut env, _, _) = with_changes(cx, false, fake);
    env.settle_until("too big", |env| env.has("Too big to compare here (over 1 MB)."));
}

#[gpui_kit::test]
fn the_changes_view_says_when_it_is_outside_a_repository_or_failed_or_nothing_changed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().diff = Some(Ok(None));
    let (mut env, _, _) = with_changes(cx, false, fake);
    env.settle_until("outside", |env| env.has("It isn't in a Git repository."));

    let fake = Fake::default();
    fake.state().diff = Some(Err("the index is locked".into()));
    let (mut env, _, _) = with_changes(cx, false, fake);
    env.settle_until("failed", |env| {
        env.has("Couldn't compare it with the last commit: the index is locked")
    });
    assert_eq!(
        env.role_of("Couldn't compare it with the last commit: the index is locked"),
        Some(Role::Alert)
    );

    // Same lines, other line endings: it says so.
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(FileDiff {
        lines: vec![],
        old_size: 10,
        new_size: 12,
        ..a_diff()
    })));
    let (mut env, _, _) = with_changes(cx, false, fake);
    env.settle_until("nothing", |env| {
        env.has("Nothing changed in its lines since the last commit (its line endings did).")
    });
}

#[gpui_kit::test]
fn side_by_side_opens_the_last_commit_read_only_against_now_and_cleans_up_after(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(a_diff())));
    let (mut env, _, recorded) = with_changes(cx, false, fake.clone());
    env.settle_until("the lines", |env| env.has("Added line 2: اثنان"));
    env.click("changes-side-by-side");
    env.settle_until("the comparison opens", |_| !recorded.borrow().is_empty());
    let (left, right, on_close) = recorded.borrow_mut().remove(0);
    assert_eq!(left.path, "C:\\Temp\\Glint\\head\\1\\ملف.txt");
    assert_eq!(left.label.as_deref(), Some("In the last commit"));
    assert!(left.read_only, "the commit's version is never edited or saved over");
    assert_eq!(right.path, "C:\\repo\\src\\ملف.txt");
    assert_eq!(right.label.as_deref(), Some("Now"));
    assert!(!right.read_only);
    // Closing the comparison removes the temp copy.
    assert!(fake.state().discarded.is_empty());
    let on_close = on_close.expect("something to run when it closes");
    env.visual.update(|_, cx| on_close(cx));
    assert_eq!(
        fake.state().discarded,
        vec!["C:\\Temp\\Glint\\head\\1\\ملف.txt".to_string()]
    );
}

#[gpui_kit::test]
fn side_by_side_for_a_file_not_in_the_commit_opens_nothing_and_a_failure_is_told(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(a_diff())));
    fake.state().head = Some(Ok(None));
    let (mut env, _, recorded) = with_changes(cx, false, fake.clone());
    env.settle_until("the lines", |env| env.has("Added line 2: اثنان"));
    env.click("changes-side-by-side");
    env.settle();
    assert!(recorded.borrow().is_empty());
    fake.state().head = Some(Err("no such blob".into()));
    env.click("changes-side-by-side");
    env.settle();
    // A failure opens nothing (it is told in a toast, which the headless window does not expose).
    assert!(recorded.borrow().is_empty());
}

#[test]
fn the_sides_are_the_commit_read_only_then_now() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let (left, right) = super::changes::sides("C:\\Temp\\head\\a.rs", "C:\\repo\\a.rs");
    assert_eq!(left, Side::named("C:\\Temp\\head\\a.rs", "In the last commit", true));
    assert_eq!(right, Side::named("C:\\repo\\a.rs", "Now", false));
}

#[gpui_kit::test]
fn the_changes_view_follows_the_file_it_is_given(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Fake::default();
    fake.state().diff = Some(Ok(Some(a_diff())));
    let (mut env, view, _) = with_changes(cx, false, fake.clone());
    env.settle_until("the lines", |env| env.has("Added line 2: اثنان"));
    // The file changed on disk: read again.
    fake.state().diff = Some(Ok(Some(FileDiff { added: 9, ..a_diff() })));
    let entry = file("C:\\repo\\src", "ملف.txt", 111, 8);
    env.visual
        .update(|_, cx| view.update(cx, |v, cx| v.set_entry(entry, cx)));
    env.settle_until("read again", |env| env.has("+9 −1 since the last commit"));
}

// ---- Icons --------------------------------------------------------------------------------------------------

#[test]
fn every_icon_git_draws_is_in_the_embedded_assets() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    for name in [
        IconName::GitBranch,
        IconName::ArrowUp,
        IconName::ArrowDown,
        IconName::Check,
        IconName::Copy,
        IconName::FolderGit2,
        IconName::ListFilter,
        IconName::RotateCw,
        IconName::ShieldAlert,
        IconName::ExternalLink,
        IconName::Code,
        IconName::BookOpen,
        IconName::ChevronDown,
        IconName::ChevronLeft,
        IconName::ChevronRight,
        IconName::X,
        IconName::TriangleAlert,
        IconName::FileDiff,
        IconName::FilePlus,
        IconName::Columns2,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}
