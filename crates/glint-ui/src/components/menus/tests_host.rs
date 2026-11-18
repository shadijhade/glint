//! Tests of the menu host using what Glint knows. Card E4 of docs/gpui/aware writes these.
//!
//! The window tests start the app's pieces as `tests.rs`'s `open` does, plus the saved state (in a folder of its
//! own, removed at the end) and an Aware entity whose probe is a fake the test holds both ends of.

use super::tests::*;
use super::*;
use crate::components::file_list::fake_git::{repo_view, FakeGit};
use crate::explorer::fake::{file, FakeDisk};
use crate::explorer::{Drive, DriveKind, NavOptions};
use glint_core::aware::{Answer, Ask, Lane};
use glint_model::aware::{Clip, Origin, Part, Why};
use glint_model::entries::attr;
use gpui_kit::TestAppContext;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

// ---- The fakes and the rig -----------------------------------------------------------------------------------

/// One call of the fake probe.
struct Call {
    paths: Vec<String>,
    lane: Lane,
    stop: Arc<AtomicBool>,
}

/// What the fake probe was asked, and the way to answer.
#[derive(Clone, Default)]
struct Fake {
    calls: Rc<RefCell<Vec<Call>>>,
    senders: Rc<RefCell<Vec<async_channel::Sender<Answer>>>>,
}

impl Fake {
    fn sources(&self) -> aware::Sources {
        let (calls, senders) = (self.calls.clone(), self.senders.clone());
        aware::Sources {
            probe: Rc::new(move |asks: Vec<Ask>, lane, stop| {
                calls.borrow_mut().push(Call {
                    paths: asks.into_iter().map(|ask| ask.path).collect(),
                    lane,
                    stop,
                });
                let (sender, receiver) = async_channel::unbounded();
                senders.borrow_mut().push(sender);
                receiver
            }),
            clip_seq: Rc::new(|| 0),
            clip: Rc::new(|_| gpui_kit::Task::ready(Clip::Empty)),
            today: Rc::new(|| 20_000),
        }
    }

    /// The first call that asked about `path`.
    fn call_of(&self, path: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .position(|call| call.paths.iter().any(|p| p == path))
            .unwrap_or_else(|| panic!("nothing was asked about {path}"))
    }

    fn send(&self, call: usize, path: &str, part: Part) {
        let sender = self.senders.borrow()[call].clone();
        assert!(sender.try_send(Answer { path: path.into(), part }).is_ok());
    }
}

/// The window, the fake, and the folder the saved state lives in.
struct Rig {
    ui: Ui,
    fake: Fake,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn drive(path: &str) -> Drive {
    Drive {
        path: path.into(),
        label: String::new(),
        kind: DriveKind::Fixed,
        fs: "NTFS".into(),
        total: 100,
        free: 50,
    }
}

/// `tests.rs`'s disk, and a folder of 201 files.
fn host_disk() -> FakeDisk {
    let disk = disk();
    let many = (0..201)
        .map(|i| file("C:\\many", &format!("f{i}.txt"), 1, 0))
        .collect();
    disk.folder("C:\\many", many);
    disk.folder("C:\\apps", vec![file("C:\\apps", "setup.exe", 4_000_000, 0)]);
    disk
}

static RIGS: AtomicUsize = AtomicUsize::new(0);

/// The app's pieces, opened on `path`, with saved state and an Aware entity that asks the fake.
fn rig(cx: &mut TestAppContext, path: &str) -> Rig {
    let disk = host_disk();
    disk.set_drives(vec![drive("C:\\")]);
    let ui = open(cx, &disk, path);
    let task = ui.explorer.update(cx, |e, cx| e.refresh_drives(cx));
    cx.foreground_executor().block_test(task);
    let dir = std::env::temp_dir().join(format!(
        "glint-menus-host-{}-{}",
        std::process::id(),
        RIGS.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let fake = Fake::default();
    let (state_file, sources) = (dir.join("state.json"), fake.sources());
    cx.update(|cx| {
        persistence::init_at(cx, state_file);
        aware::init_with(cx, sources);
    });
    Rig { ui, fake, dir }
}

impl Rig {
    /// Chooses the row with this label, as the view says it was chosen.
    fn choose(&mut self, label: &str) {
        let nodes = self.ui.nodes();
        let run = find(&nodes, label).unwrap_or_else(|| panic!("no {label:?}")).run.clone();
        let view = self.ui.view();
        self.ui.visual.update(|_, cx| {
            view.update(cx, |_, cx| cx.emit(MenuViewEvent::Chose(run)));
        });
        self.ui.frame();
    }

    fn habits(&mut self) -> glint_model::aware::Habits {
        self.ui
            .visual
            .update(|_, cx| persistence::app(cx).map(|app| app.habits.clone()).unwrap_or_default())
    }

    fn opened_on(&mut self, path: &str) {
        self.ui.select(&[path]);
        self.ui.open_menu(Some(path), Some(at(100., 10.)), false);
    }

    fn head(&mut self) -> Head {
        match self.ui.nodes().into_iter().next() {
            Some(Node::Head(head)) => head,
            other => panic!("the menu doesn't start with a head: {other:?}"),
        }
    }
}

const PACK: &str = "C:\\x\\pack.zip";

fn heading(nodes: &[Node]) -> Option<usize> {
    nodes.iter().position(|node| matches!(node, Node::Heading(h) if h == "Suggested"))
}

// ---- The host dresses the menus ------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_files_menu_starts_with_a_head_named_for_it_and_then_the_strip(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.opened_on(ALPHA);
    let nodes = rig.ui.nodes();
    assert!(matches!(&nodes[0], Node::Head(head) if head.title == "alpha.txt"), "{:?}", labels(&nodes));
    assert!(matches!(&nodes[1], Node::Strip(_)), "{:?}", labels(&nodes));
}

#[gpui_kit::test]
fn this_pcs_menu_has_no_head(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.ui.open_folder(glint_model::paths::THIS_PC, at(100., 10.));
    let nodes = rig.ui.nodes();
    assert_eq!(nodes.len(), 1, "{:?}", labels(&nodes));
    assert!(matches!(&nodes[0], Node::Item(e) if e.label == "Refresh"));
}

#[gpui_kit::test]
fn a_menu_is_not_dressed_while_the_app_has_no_aware_entity(cx: &mut TestAppContext) {
    let _guard = english();
    let mut ui = open(cx, &host_disk(), "C:\\x");
    ui.select(&[ALPHA]);
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    let nodes = ui.nodes();
    assert!(matches!(&nodes[0], Node::Strip(_)), "{:?}", labels(&nodes));
    assert!(heading(&nodes).is_none());
}

#[gpui_kit::test]
fn a_zips_menu_suggests_extract_here_and_leaves_its_own_rows_as_they_were(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.opened_on(PACK);
    let nodes = rig.ui.nodes();
    assert!(matches!(&nodes[1], Node::Strip(_)));
    assert!(matches!(&nodes[2], Node::Heading(h) if h == "Suggested"), "{:?}", labels(&nodes));
    assert!(
        matches!(&nodes[3], Node::Item(e) if e.label == "Extract here" && e.smart),
        "{:?}",
        labels(&nodes)
    );
    // What follows Suggested is the menu as `item_menu` makes it, from its second node on.
    let plain = {
        let menus = rig.ui.menus.clone();
        let entry = file("C:\\x", "pack.zip", 5000, 0);
        rig.ui.visual.update(|_, cx| {
            let ctx = menus.read(cx).item_ctx(0, cx);
            item_menu(&[entry], &ctx)
        })
    };
    assert_eq!(nodes[nodes.len() - (plain.len() - 1)..], plain[1..]);
    assert_eq!(nodes[1], plain[0], "the strip is the menu's own");
}

#[gpui_kit::test]
fn with_suggestions_off_there_is_no_suggested_but_the_head_stays(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.ui.visual.update(|_, cx| state::update(cx, |settings| settings.suggest = false));
    rig.opened_on(PACK);
    let nodes = rig.ui.nodes();
    assert!(heading(&nodes).is_none(), "{:?}", labels(&nodes));
    assert!(matches!(&nodes[0], Node::Head(head) if head.title == "pack.zip"));
}

#[gpui_kit::test]
fn an_answer_while_the_menu_is_open_goes_into_the_head_and_moves_no_row(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.opened_on(ALPHA);
    let before = rig.ui.nodes();
    assert!(!rig.head().badges.iter().any(|badge| badge.label == "In use by Word"));
    let call = rig.fake.call_of(ALPHA);
    rig.fake.send(call, ALPHA, Part::LockedBy(vec!["Word".into()]));
    rig.ui.frame();
    let after = rig.ui.nodes();
    assert!(
        rig.head().badges.iter().any(|badge| badge.label == "In use by Word"),
        "{:?}",
        rig.head()
    );
    assert_eq!(labels(&before)[1..], labels(&after)[1..], "no row moved");
    assert_eq!(before[1..], after[1..], "no row changed");
}

#[gpui_kit::test]
fn opening_a_menu_for_one_file_asks_at_once_and_closing_it_stops_asking(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.opened_on(ALPHA);
    let call = rig.fake.call_of(ALPHA);
    assert_eq!(rig.fake.calls.borrow()[call].lane, Lane::Now);
    assert!(!rig.fake.calls.borrow()[call].stop.load(Ordering::SeqCst));
    rig.ui.press("escape");
    assert!(!rig.ui.is_open());
    assert!(rig.fake.calls.borrow()[call].stop.load(Ordering::SeqCst), "the run was ended");
}

// ---- What choosing counts ------------------------------------------------------------------------------------

#[gpui_kit::test]
fn choosing_extract_here_is_counted_under_archives_and_copy_is_not_counted(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.opened_on(PACK);
    rig.choose("Extract here");
    assert!(!rig.ui.is_open());
    assert_eq!(rig.habits().action_uses(Shape::Archive, "do:extract-here"), 1);
    rig.opened_on(PACK);
    rig.choose("Copy");
    assert_eq!(rig.habits().action_uses(Shape::Archive, "cmd:copy"), 0);
    assert_eq!(rig.habits().actions.values().map(|actions| actions.len()).sum::<usize>(), 1);
}

#[gpui_kit::test]
fn with_learning_off_nothing_is_counted(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.ui.visual.update(|_, cx| state::update(cx, |settings| settings.learn = false));
    rig.opened_on(PACK);
    rig.choose("Extract here");
    assert!(rig.habits().is_empty());
}

#[gpui_kit::test]
fn what_the_person_often_does_with_a_kind_of_file_is_suggested_with_its_reason(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    let shape = Shape::of(&[file("C:\\x", "alpha.txt", 100, 0)]);
    rig.ui.visual.update(|_, cx| {
        for _ in 0..3 {
            aware::note_action(cx, shape, "do:compress:zip");
        }
    });
    rig.opened_on(ALPHA);
    let nodes = rig.ui.nodes();
    let at_the_top = nodes.iter().find_map(|node| match node {
        Node::Item(e) if e.smart && e.run.key().as_deref() == Some("do:compress:zip") => Some(e),
        _ => None,
    });
    let row = at_the_top.unwrap_or_else(|| panic!("zip isn't suggested: {:?}", labels(&nodes)));
    assert_eq!(row.label, "Compress to › Zip file");
    assert_eq!(row.reason, smart::reason(&Why::Usual, shape));
    assert!(row.reason.is_some());
}

// ---- The new rows --------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn a_file_the_internet_marked_has_unblock_and_one_with_no_answer_hasnt(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.opened_on(ALPHA);
    assert!(find(&rig.ui.nodes(), "Unblock").is_none(), "nothing is known yet");
    rig.ui.press("escape");
    let origin = Origin {
        host: "github.com".into(),
        blocked: true,
    };
    // Looking ahead (the pointer rests on it) asks again, since the menu's own run was ended; its answer is
    // there when the menu opens.
    let entry = file("C:\\x", "alpha.txt", 100, 0);
    rig.ui.visual.update(|_, cx| {
        aware::shared(cx).update(cx, |aware, cx| aware.look_ahead(std::slice::from_ref(&entry), cx));
    });
    let call = rig.fake.calls.borrow().len() - 1;
    rig.fake.send(call, ALPHA, Part::Origin(origin));
    rig.ui.frame();
    rig.opened_on(ALPHA);
    let nodes = rig.ui.nodes();
    let unblock = find(&nodes, "Unblock").unwrap_or_else(|| panic!("{:?}", labels(&nodes)));
    assert_eq!(unblock.run, Run::Ask(MenuEvent::Unblock { paths: vec![ALPHA.into()] }));
}

fn blocked_ctx() -> ItemCtx {
    let mut c = ctx();
    c.probed = Some(glint_model::aware::Probed {
        origin: Some(Origin {
            host: "example.org".into(),
            blocked: true,
        }),
        ..Default::default()
    });
    c
}

#[test]
fn unblock_comes_right_after_the_sandbox_and_only_for_a_blocked_file_that_can_change() {
    let _guard = english();
    let menu = item_menu(&[text("setup.exe")], &blocked_ctx());
    let all = labels(&menu);
    let sandbox = all.iter().position(|l| l == "Run in Windows Sandbox").expect("the sandbox row");
    assert_eq!(all[sandbox + 1], "Unblock");
    assert!(find(&item_menu(&[text("setup.exe")], &ctx()), "Unblock").is_none());
    let mut c = blocked_ctx();
    c.archive = Some(an_archive());
    let inside = with_attrs(text("setup.exe"), attr::IN_ARCHIVE);
    assert!(find(&item_menu(&[inside], &c), "Unblock").is_none(), "nothing changes inside an archive");
    let two = item_menu(&[text("a.exe"), text("b.exe")], &blocked_ctx());
    assert!(find(&two, "Unblock").is_none(), "one file at a time");
}

#[test]
fn a_picture_on_disk_can_have_its_text_copied_and_other_things_cannot() {
    let _guard = english();
    let picture = item_menu(&[text("shot.png")], &ctx());
    let all = labels(&picture);
    let at = all.iter().position(|l| l == "Copy the text in it").expect("the row");
    assert_eq!(all[at + 1], "Set as desktop background");
    assert_eq!(
        find(&picture, "Copy the text in it").map(|e| e.run.clone()),
        Some(Run::Ask(MenuEvent::CopyTextIn {
            path: "C:\\x\\shot.png".into()
        }))
    );
    for name in ["a.jpg", "a.JPEG", "a.webp", "a.tif", "a.bmp", "a.gif"] {
        assert!(find(&item_menu(&[text(name)], &ctx()), "Copy the text in it").is_some(), "{name}");
    }
    assert!(find(&item_menu(&[text("notes.txt")], &ctx()), "Copy the text in it").is_none());
    assert!(find(&item_menu(&[text("a.png"), text("b.png")], &ctx()), "Copy the text in it").is_none());
    let mut c = ctx();
    c.archive = Some(an_archive());
    let inside = with_attrs(text("a.png"), attr::IN_ARCHIVE);
    assert!(find(&item_menu(&[inside], &c), "Copy the text in it").is_none());
    let online = with_attrs(text("a.png"), attr::CLOUD | attr::OFFLINE);
    assert!(find(&item_menu(&[online], &ctx()), "Copy the text in it").is_none(), "it isn't on the disk");
}

// ---- Why a row is greyed out ---------------------------------------------------------------------------------

#[test]
fn a_greyed_row_says_why() {
    let _guard = english();
    let mut c = ctx();
    c.archive = Some(an_archive());
    let inside = item_menu(&[with_attrs(text("a.txt"), attr::IN_ARCHIVE)], &c);
    for label in ["Cut", "Rename", "Delete"] {
        let row = find(&inside, label).unwrap_or_else(|| panic!("{label}"));
        assert_eq!(row.why_not.as_deref(), Some("Nothing changes inside an archive"), "{label}");
    }
    assert_eq!(find(&inside, "Copy").map(|e| e.why_not.clone()), Some(None), "what works says nothing");
    let there = item_menu(&[with_attrs(text("a.txt"), attr::REMOTE)], &ctx());
    assert_eq!(
        find(&there, "Cut").and_then(|e| e.why_not.as_deref()),
        Some("Read-only: on a remote connection")
    );
    let shut = item_menu(&[with_attrs(text("a.txt"), attr::CLOSED)], &ctx());
    assert_eq!(
        find(&shut, "Cut").and_then(|e| e.why_not.as_deref()),
        Some("Windows keeps this folder closed")
    );
    // Nothing became enabled or disabled by saying why.
    assert_eq!(find(&item_menu(&[text("a.txt")], &ctx()), "Cut").map(|e| (e.disabled, e.why_not.clone())), Some((false, None)));
}

#[test]
fn a_dual_panes_rows_say_what_the_other_pane_cannot_do() {
    let _guard = english();
    let mut c = ctx();
    c.dual = true;
    let menu = item_menu(&[text("a.txt")], &c);
    let copy = find(&menu, "Copy to the other pane").expect("the row");
    assert_eq!(copy.why_not.as_deref(), Some("The other pane shows nothing that can take items"));
    let mv = find(&menu, "Move to the other pane").expect("the row");
    assert_eq!(mv.why_not.as_deref(), Some("The other pane shows nothing that can take items"));
}

#[test]
fn a_folder_that_cannot_undo_says_so() {
    let _guard = english();
    let menu = folder_menu("C:\\x", true, &FolderCtx::default());
    assert_eq!(find(&menu, "Undo").and_then(|e| e.why_not.as_deref()), Some("Nothing to undo"));
    let mut c = FolderCtx::default();
    c.data.undo = Some("copy of 2 items".into());
    let menu = folder_menu("C:\\x", true, &c);
    assert_eq!(find(&menu, "Undo the copy of 2 items").map(|e| (e.disabled, e.why_not.clone())), Some((false, None)));
}

#[test]
fn a_folder_menu_in_a_search_or_an_archive_says_why_its_changes_are_off() {
    let _guard = english();
    let mut c = FolderCtx {
        view: Some(FolderView {
            find: true,
            ..FolderView::default()
        }),
        ..FolderCtx::default()
    };
    let menu = folder_menu("C:\\x", true, &c);
    assert_eq!(
        find(&menu, "New folder").and_then(|e| e.why_not.as_deref()),
        Some("Not while a search's results are shown")
    );
    c.view = Some(FolderView {
        archive: Some(an_archive()),
        ..FolderView::default()
    });
    let menu = folder_menu("C:\\x", true, &c);
    assert_eq!(
        find(&menu, "New folder").and_then(|e| e.why_not.as_deref()),
        Some("Nothing changes inside an archive")
    );
}

// ---- For the island ------------------------------------------------------------------------------------------

#[gpui_kit::test]
fn suggestions_for_one_zip_are_extract_here_and_for_nothing_or_too_many_are_none(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    let menus = rig.ui.menus.clone();
    rig.ui.select(&[PACK]);
    let got = rig.ui.visual.update(|_, cx| menus.read(cx).suggestions(0, 1, cx));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].key, "do:extract-here");
    assert_eq!(got[0].shape, Shape::Archive);
    assert_eq!(got[0].entry.label, "Extract here");
    let explorer = rig.ui.explorer.clone();
    rig.ui.visual.update(|_, cx| explorer.update(cx, |e, cx| e.select_only(None, cx)));
    let got = rig.ui.visual.update(|_, cx| menus.read(cx).suggestions(0, 3, cx));
    assert!(got.is_empty(), "nothing is selected");
}

#[gpui_kit::test]
fn more_than_two_hundred_selected_suggest_nothing(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\many");
    let menus = rig.ui.menus.clone();
    let paths: Vec<String> = (0..201).map(|i| format!("C:\\many\\f{i}.txt")).collect();
    let explorer = rig.ui.explorer.clone();
    rig.ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| e.set_selection(paths.iter(), None, None, cx));
    });
    let got = rig.ui.visual.update(|_, cx| menus.read(cx).suggestions(0, 3, cx));
    assert!(got.is_empty());
    // Two hundred is still looked at.
    let two_hundred = &paths[..200];
    rig.ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| e.set_selection(two_hundred.iter(), None, None, cx));
    });
    let got = rig.ui.visual.update(|_, cx| menus.read(cx).suggestions(0, 3, cx));
    assert!(got.len() <= 3);
}

// ---- The folder's own menu -----------------------------------------------------------------------------------

#[gpui_kit::test]
fn the_folders_own_menu_has_a_head_with_how_many_items_it_shows(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.ui.open_background(at(100., 10.));
    let head = rig.head();
    assert_eq!(head.line, "5 items");
    assert!(!head.title.is_empty());
    assert!(head.badges.is_empty(), "no repository, no branch");
    assert!(matches!(&rig.ui.nodes()[1], Node::Strip(_)));
}

#[gpui_kit::test]
fn the_folders_own_menu_names_its_git_branch(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    let git = FakeGit::default();
    git.put("C:\\x", repo_view(&[]));
    cx.update(|cx| git.install(cx));
    // The folder is opened again so Git reads it.
    for path in ["C:\\x\\docs", "C:\\x"] {
        let task = rig.ui.explorer.update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
        cx.foreground_executor().block_test(task);
        for _ in 0..8 {
            rig.ui.frame();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
    rig.ui.open_background(at(100., 10.));
    let head = rig.head();
    assert!(head.badges.iter().any(|badge| badge.label == "main"), "{head:?}");
}

#[gpui_kit::test]
fn a_tiles_menu_has_a_head_with_no_count(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    rig.ui.open_folder(DOCS, at(100., 10.));
    let head = rig.head();
    assert_eq!(head.title, "docs");
    assert!(head.line.is_empty(), "{head:?}");
}

// ---- The breath a menu waits for what is being found out ---------------------------------------------------

const SETUP: &str = "C:\\apps\\setup.exe";

#[gpui_kit::test]
fn a_menu_waits_a_breath_for_what_is_being_found_out_and_suggests_from_it(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\apps");
    rig.ui.select(&[SETUP]);
    // Nothing is known about the program yet: the menu asks at once, and isn't there in the same instant.
    let menus = rig.ui.menus.clone();
    rig.ui.visual.update(|window, cx| {
        menus.update(cx, |m, cx| m.open_list_menu(0, Some(SETUP), Some(at(100., 10.)), false, window, cx));
    });
    rig.ui.frame();
    assert!(!rig.ui.is_open(), "it waits a breath for what is being found out");
    // What is under it opens no tooltip from now until the menu closes: the pointer rests where the press was.
    let plain = |window: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
        let under = crate::kit::tooltip_under("setup.exe".into(), window, cx).entity_type();
        under == crate::kit::tooltip("setup.exe".into(), window, cx).entity_type()
    };
    assert!(rig.ui.visual.update(|_, cx| super::showing(cx)), "as good as open");
    assert!(!rig.ui.visual.update(plain), "no tooltip while a menu waits");
    let call = rig.fake.call_of(SETUP);
    assert_eq!(rig.fake.calls.borrow()[call].lane, Lane::Now);

    // The answers arrive within the breath, and what the menu suggests is said from them.
    rig.fake.send(
        call,
        SETUP,
        Part::Origin(glint_model::aware::Origin {
            host: "github.com".into(),
            blocked: true,
        }),
    );
    rig.fake.send(call, SETUP, Part::Signer(glint_model::aware::Signer::Unsigned));
    rig.ui.frame();
    rig.ui.visual.executor().advance_clock(std::time::Duration::from_millis(60));
    rig.ui.frame();
    assert!(rig.ui.is_open());
    let nodes = rig.ui.nodes();
    in_order(
        &labels(&nodes),
        &["# Suggested", "Scan with Microsoft Defender", "Run in Windows Sandbox", "Open"],
    );
    let reason = |label: &str| find(&nodes, label).and_then(|entry| entry.reason.clone());
    assert_eq!(reason("Scan with Microsoft Defender").as_deref(), Some("It came from github.com"));
    assert_eq!(reason("Run in Windows Sandbox").as_deref(), Some("It isn't signed"));
    assert!(find(&nodes, "Unblock").is_some(), "a file marked as downloaded can be unblocked");
    assert!(!rig.fake.calls.borrow()[call].stop.load(Ordering::SeqCst), "and its run goes on while the menu is open");
    assert!(!rig.ui.visual.update(plain), "nor while it is open");
    rig.ui.visual.update(|_, cx| menus.update(cx, |m, cx| m.close(cx)));
    rig.ui.frame();
    assert!(!rig.ui.visual.update(|_, cx| super::showing(cx)));
    assert!(rig.ui.visual.update(plain), "closed, a row says its name again");
}

#[gpui_kit::test]
fn a_right_click_on_what_the_pointer_was_resting_on_asks_what_is_left_for_now_and_waits(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\apps");
    rig.ui.select(&[SETUP]);
    // The pointer rested on the program: its questions went out on the quiet lane and aren't answered yet.
    let entry = file("C:\\apps", "setup.exe", 4_000_000, 0);
    rig.ui.visual.update(|_, cx| {
        aware::shared(cx).update(cx, |aware, cx| aware.look_ahead(std::slice::from_ref(&entry), cx));
    });
    assert_eq!(rig.fake.calls.borrow().len(), 1);
    assert_eq!(rig.fake.calls.borrow()[0].lane, Lane::Quiet);
    let menus = rig.ui.menus.clone();
    rig.ui.visual.update(|window, cx| {
        menus.update(cx, |m, cx| m.open_list_menu(0, Some(SETUP), Some(at(100., 10.)), false, window, cx));
    });
    rig.ui.frame();
    // The questions are asked again where they go first, and the menu waits for them.
    assert_eq!(rig.fake.calls.borrow().len(), 2);
    assert_eq!(rig.fake.calls.borrow()[1].lane, Lane::Now);
    assert!(!rig.ui.is_open());
    assert!(rig.ui.visual.update(|_, cx| super::showing(cx)));
}

/// Asks for the menu of the program, which waits its breath for what is being found out about it.
fn right_click_setup(rig: &mut Rig) {
    rig.ui.select(&[SETUP]);
    let menus = rig.ui.menus.clone();
    rig.ui.visual.update(|window, cx| {
        menus.update(cx, |m, cx| m.open_list_menu(0, Some(SETUP), Some(at(100., 10.)), false, window, cx));
    });
    rig.ui.frame();
    assert!(!rig.ui.is_open(), "it waits for what is being found out");
}

#[gpui_kit::test]
fn a_menu_opens_the_moment_everything_it_waits_for_is_known_not_when_the_breath_is_over(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\apps");
    right_click_setup(&mut rig);
    let call = rig.fake.call_of(SETUP);
    let origin = Part::Origin(glint_model::aware::Origin { host: "github.com".into(), blocked: true });
    let known = [
        origin,
        Part::LockedBy(Vec::new()),
        Part::OpensWith("Notepad".into()),
        Part::Signer(glint_model::aware::Signer::Unsigned),
    ];
    for part in known {
        rig.fake.send(call, SETUP, part);
        rig.ui.frame();
        assert!(!rig.ui.is_open(), "one part is still to come");
    }
    // The last one, and not a millisecond of the breath has passed.
    rig.fake.send(call, SETUP, Part::Twins(Vec::new()));
    rig.ui.frame();
    assert!(rig.ui.is_open(), "all five were answered");
    let nodes = rig.ui.nodes();
    assert!(find(&nodes, "Run in Windows Sandbox").is_some(), "{:?}", labels(&nodes));
}

#[gpui_kit::test]
fn a_menu_opens_when_the_run_ends_with_parts_that_cannot_be_known(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\apps");
    right_click_setup(&mut rig);
    let call = rig.fake.call_of(SETUP);
    // Part of it comes in, and the engine is done: what didn't come can't be known.
    rig.fake.send(call, SETUP, Part::OpensWith("Notepad".into()));
    rig.ui.frame();
    assert!(!rig.ui.is_open());
    rig.fake.senders.borrow_mut().clear();
    rig.ui.frame();
    assert!(rig.ui.is_open(), "nothing more will come, so the menu doesn't wait out the breath");
}

#[gpui_kit::test]
fn a_menu_whose_answers_never_come_opens_when_the_breath_is_over_and_not_before(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\apps");
    right_click_setup(&mut rig);
    rig.ui.visual.executor().advance_clock(LOOK_FIRST - std::time::Duration::from_millis(1));
    rig.ui.frame();
    assert!(!rig.ui.is_open());
    rig.ui.visual.executor().advance_clock(std::time::Duration::from_millis(2));
    rig.ui.frame();
    assert!(rig.ui.is_open());
}

#[gpui_kit::test]
fn the_open_project_folders_menu_waits_for_its_size_and_suggests_cleaning_up_its_build_output(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    let menus = rig.ui.menus.clone();
    rig.ui.visual.update(|window, cx| {
        menus.update(cx, |m, cx| m.open_background_menu(0, Some(at(100., 10.)), window, cx));
    });
    rig.ui.frame();
    assert!(!rig.ui.is_open(), "the folder is being asked about, and the menu waits for it");
    let call = rig.fake.call_of("C:\\x");
    assert_eq!(rig.fake.calls.borrow()[call].lane, Lane::Now);
    // The index answers at once: 3 GB, 2 GB of it what a build left.
    let gb = 1024 * 1024 * 1024;
    let inside = glint_model::aware::Inside { files: 900, folders: 40, bytes: 3 * gb, junk: 2 * gb, done: true, from_index: true };
    rig.fake.send(call, "C:\\x", Part::Inside(inside));
    rig.ui.frame();
    assert!(rig.ui.is_open(), "everything it waited for is known, with the breath not over");
    let head = rig.head();
    assert!(head.line.contains("3.0 GB"), "the folder's size is in its head: {:?}", head.line);
    let nodes = rig.ui.nodes();
    let at_top = heading(&nodes).expect("a Suggested section");
    let row = match &nodes[at_top + 1] {
        Node::Item(entry) => entry.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(row.label.as_ref(), "Clean up…");
    assert_eq!(row.reason.as_deref(), Some("2.0 GB of build output inside"));
}

#[gpui_kit::test]
fn what_comes_in_after_the_folders_menu_is_open_goes_into_its_head(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    let menus = rig.ui.menus.clone();
    rig.ui.visual.update(|window, cx| {
        menus.update(cx, |m, cx| m.open_background_menu(0, Some(at(100., 10.)), window, cx));
    });
    rig.ui.visual.executor().advance_clock(LOOK_FIRST + std::time::Duration::from_millis(5));
    rig.ui.frame();
    assert!(rig.ui.is_open(), "the breath is over");
    assert!(!rig.head().line.contains("MB"));
    let call = rig.fake.call_of("C:\\x");
    let inside = glint_model::aware::Inside { files: 4, folders: 1, bytes: 5 * 1024 * 1024, junk: 0, done: true, from_index: false };
    rig.fake.send(call, "C:\\x", Part::Inside(inside));
    rig.ui.frame();
    assert!(rig.head().line.contains("5.0 MB"), "{:?}", rig.head().line);
    assert!(heading(&rig.ui.nodes()).is_none(), "the rows don't move once the menu is open");
}

#[gpui_kit::test]
fn a_menu_asked_for_while_another_waits_is_the_one_that_opens(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx, "C:\\x");
    let menus = rig.ui.menus.clone();
    // A right-click on a file (which waits a breath), then at once one on the empty part of the list.
    rig.ui.select(&[ALPHA]);
    rig.ui.visual.update(|window, cx| {
        menus.update(cx, |m, cx| m.open_list_menu(0, Some(ALPHA), Some(at(100., 10.)), false, window, cx));
    });
    rig.ui.select(&[]);
    rig.ui.open_background(at(300., 10.));
    rig.ui.visual.executor().advance_clock(std::time::Duration::from_millis(60));
    rig.ui.frame();
    assert!(rig.ui.is_open());
    assert!(find(&rig.ui.nodes(), "New folder").is_some(), "the folder's menu, not the file's");
}
