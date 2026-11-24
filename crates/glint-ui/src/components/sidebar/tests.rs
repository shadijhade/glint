//! Headless tests of the sidebar: on a fake disk and fake sources, what is drawn (labels, roles), that clicks and
//! keys change the explorer the way the TypeScript's did, an Arabic folder name, and the mirrored layout.

use super::rows::PortalKind;
use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{init_with, Drive, DriveKind, KnownFolder, KnownId};
use crate::platform::dragdrop::{Answer, DropAnswers, DropEffect, DropEvent};
use crate::testing;
use glint_model::store::RemoteConnection;
use gpui_kit::test::{ClickOptions, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;

const HOME: &str = "C:\\Users\\me";
const DOCS: &str = "C:\\Users\\me\\Documents";

/// Names with how many items have each.
type Counted = Vec<(String, u64)>;

/// A change of tags the backend was asked for: the paths, the tags added and the tags taken off.
type TagsChange = (Vec<String>, Vec<String>, Vec<String>);

/// What the fake sources know, and what they were asked.
struct Fakes {
    folders: Mutex<HashMap<String, Vec<Subfolder>>>,
    asked: Mutex<Vec<(String, bool)>>,
    cloud: Mutex<Option<Space>>,
    count: Mutex<Option<String>>,
    /// The tags instant search lists (an error where it is off), and how many times it was asked.
    tags: Mutex<Result<Vec<(String, u64)>, String>>,
    tags_asked: Mutex<usize>,
    /// The labels and tags the index and the ledger count (an error where marks can't be counted), and what has a tag.
    marks: Mutex<Result<(Counted, Counted), String>>,
    tagged: Mutex<Result<Vec<String>, String>>,
    /// The tags asked of the `tagged` source, and the tags changes the backend was asked for: (paths, add, remove).
    tagged_asked: Mutex<Vec<String>>,
    sets: Mutex<Vec<TagsChange>>,
    /// The portals mounted, what was done to them ("refresh:id", "unmount:id"), and where core's change notice goes.
    portals: Mutex<Vec<PortalItem>>,
    portal_log: Mutex<Vec<String>>,
    portal_tx: Mutex<Option<async_channel::Sender<()>>>,
}

impl Default for Fakes {
    fn default() -> Self {
        Fakes {
            folders: Mutex::default(),
            asked: Mutex::default(),
            cloud: Mutex::default(),
            count: Mutex::default(),
            tags: Mutex::new(Err("Instant search is off".to_string())),
            tags_asked: Mutex::default(),
            marks: Mutex::new(Err("Marks can't be counted".to_string())),
            tagged: Mutex::new(Err("Nothing can be searched".to_string())),
            tagged_asked: Mutex::default(),
            sets: Mutex::default(),
            portals: Mutex::default(),
            portal_log: Mutex::default(),
            portal_tx: Mutex::default(),
        }
    }
}

/// glint-core's tags, as the tests need them: nothing has any tags, and setting them works (and is written down).
struct NoTags(Arc<Fakes>);

impl tags::Backend for NoTags {
    fn read(&self, paths: Vec<String>) -> tags::Fut<Vec<Vec<String>>> {
        Box::pin(async move { Ok(paths.iter().map(|_| Vec::new()).collect()) })
    }

    fn set(&self, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> tags::Fut<Vec<(String, String)>> {
        self.0.sets.lock().unwrap().push((paths, add, remove));
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl Fakes {
    fn sources(self: &Arc<Self>) -> Sources {
        let (a, b, c, d) = (self.clone(), self.clone(), self.clone(), self.clone());
        let (e, f, g, h) = (self.clone(), self.clone(), self.clone(), self.clone());
        let (i, j) = (self.clone(), self.clone());
        Sources {
            portals: Arc::new(move || e.portals.lock().unwrap().clone()),
            portal_changes: Arc::new(move || {
                let (tx, rx) = async_channel::unbounded();
                *f.portal_tx.lock().unwrap() = Some(tx);
                Some(rx)
            }),
            portal_refresh: Arc::new(move |id| {
                let g = g.clone();
                Box::pin(async move {
                    g.portal_log.lock().unwrap().push(format!("refresh:{id}"));
                    Ok(())
                })
            }),
            portal_unmount: Arc::new(move |id| {
                let h = h.clone();
                Box::pin(async move {
                    h.portal_log.lock().unwrap().push(format!("unmount:{id}"));
                    h.portals.lock().unwrap().retain(|p| p.id != id);
                    h.tell_portals();
                    Ok(())
                })
            }),
            subfolders: Arc::new(move |path, hidden| {
                let a = a.clone();
                Box::pin(async move {
                    a.asked.lock().unwrap().push((path.clone(), hidden));
                    match a.folders.lock().unwrap().get(&key_of(&path)) {
                        Some(list) => Ok(list.clone()),
                        None => Err("The system cannot find the path specified".to_string()),
                    }
                })
            }),
            cloud_space: Arc::new(move |_, _| {
                let b = b.clone();
                Box::pin(async move { Ok(*b.cloud.lock().unwrap()) })
            }),
            saved_count: Arc::new(move |_, _| {
                let c = c.clone();
                Box::pin(async move { Ok(c.count.lock().unwrap().clone()) })
            }),
            tags: Arc::new(move || {
                let d = d.clone();
                Box::pin(async move {
                    *d.tags_asked.lock().unwrap() += 1;
                    d.tags.lock().unwrap().clone()
                })
            }),
            marks: Arc::new(move || {
                let i = i.clone();
                Box::pin(async move { i.marks.lock().unwrap().clone() })
            }),
            tagged: Arc::new(move |tag| {
                let j = j.clone();
                Box::pin(async move {
                    j.tagged_asked.lock().unwrap().push(tag);
                    j.tagged.lock().unwrap().clone()
                })
            }),
        }
    }

    /// What core's change notice does: tells the sidebar to read the portals again.
    fn tell_portals(&self) {
        if let Some(tx) = self.portal_tx.lock().unwrap().as_ref() {
            let _ = tx.try_send(());
        }
    }

    fn mount(&self, id: &str, name: &str, kind: PortalKind) {
        self.portals.lock().unwrap().push(PortalItem {
            id: id.into(),
            name: name.into(),
            path: format!("C:\\Users\\me\\Glint portals\\{name}"),
            kind,
            what: "what it shows".into(),
            active: false,
        });
        self.tell_portals();
    }

    fn set(&self, path: &str, kids: &[(&str, Option<bool>)]) {
        self.folders.lock().unwrap().insert(
            key_of(path),
            kids.iter()
                .map(|(name, children)| Subfolder {
                    name: (*name).to_string(),
                    children: *children,
                })
                .collect(),
        );
    }

    fn times_asked(&self, path: &str) -> usize {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| key_of(p) == key_of(path))
            .count()
    }
}

struct World {
    cx: VisualTestContext,
    sidebar: Entity<Sidebar>,
    fakes: Arc<Fakes>,
    events: Rc<RefCell<Vec<SidebarEvent>>>,
    requests: Rc<RefCell<Vec<SidebarRequest>>>,
    disk: FakeDisk,
    explorer: Entity<Explorer>,
    _subscriptions: Vec<Subscription>,
}

fn state_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("glint-sidebar-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp folder");
    let file = dir.join(format!("{name}.json"));
    let _ = std::fs::remove_file(&file);
    file
}

/// A window with the sidebar over an explorer on a fake disk: Home with Documents and Pictures, drives C: and a
/// USB drive E:, opened at Home.
fn world(cx: &mut TestAppContext, name: &str) -> World {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    disk.set_known(vec![
        KnownFolder {
            id: KnownId::Home,
            name: "Home",
            path: HOME.into(),
        },
        KnownFolder {
            id: KnownId::Documents,
            name: "Documents",
            path: DOCS.into(),
        },
    ]);
    disk.set_drives(vec![
        Drive {
            path: "C:\\".into(),
            label: String::new(),
            kind: DriveKind::Fixed,
            fs: "NTFS".into(),
            total: 1000,
            free: 400,
        },
        Drive {
            path: "E:\\".into(),
            label: "Photos".into(),
            kind: DriveKind::Removable,
            fs: "FAT32".into(),
            total: 0,
            free: 0,
        },
    ]);
    disk.folder(
        HOME,
        vec![
            folder(HOME, "Documents"),
            folder(HOME, "Pictures"),
            file(HOME, "readme.txt", 5, 1),
        ],
    );
    disk.folder(DOCS, vec![folder(DOCS, "Reports"), file(DOCS, "a.txt", 5, 1)]);
    disk.folder(
        &format!("{DOCS}\\Reports"),
        vec![file(&format!("{DOCS}\\Reports"), "q1.txt", 1, 1)],
    );
    disk.folder("C:\\", vec![folder("C:\\", "Windows")]);
    let fakes = Arc::new(Fakes::default());
    cx.update(|cx| {
        init_with(cx, disk.source());
        persistence::init_at(cx, state_file(name));
        tags::init_with(cx, Arc::new(NoTags(fakes.clone())));
        // Settings start with the seven labels, which would add a section to every list below: the tests of
        // labels set their own.
        state::update(cx, |s| s.labels.clear());
    });
    fakes.set(HOME, &[("Documents", Some(true)), ("Pictures", Some(false))]);
    fakes.set(DOCS, &[("Reports", None)]);
    fakes.set("C:\\", &[("Windows", Some(true))]);
    let explorer = cx.update(|cx| explorer(cx));
    let sources = fakes.sources();
    let (mut visual, sidebar) =
        testing::open_window(cx, |window, cx| cx.new(|cx| Sidebar::with_sources(sources, window, cx)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let requests = Rc::new(RefCell::new(Vec::new()));
    let asked = requests.clone();
    let subscriptions = visual.update(|_, cx| {
        vec![
            cx.subscribe(&sidebar, move |_, event: &SidebarEvent, _| {
                sink.borrow_mut().push(event.clone())
            }),
            cx.subscribe(&sidebar, move |_, event: &SidebarRequest, _| {
                asked.borrow_mut().push(event.clone())
            }),
        ]
    });
    let task = explorer.update(&mut visual, |e, cx| e.init_explorer(Vec::new(), cx));
    visual.foreground_executor().block_test(task);
    let mut world = World {
        cx: visual,
        sidebar,
        fakes,
        events,
        requests,
        disk,
        explorer,
        _subscriptions: subscriptions,
    };
    world.settle(|w| w.path() == HOME && !w.tab_loading());
    world
}

impl World {
    /// Lets the background reads come back until `done`.
    fn settle(&mut self, mut done: impl FnMut(&mut World) -> bool) {
        for _ in 0..400 {
            self.cx.run_until_parked();
            if done(self) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("what the sidebar waited for never came");
    }

    fn path(&mut self) -> String {
        self.explorer.read_with(&self.cx, |e, _| e.current_path().to_string())
    }

    fn tab_loading(&mut self) -> bool {
        self.explorer
            .read_with(&self.cx, |e, _| e.active_tab().is_some_and(|t| t.loading))
    }

    fn frame(&mut self) {
        self.cx.update(|window, cx| window.render_frame(cx));
    }

    fn rows(&mut self) -> Vec<Row> {
        self.sidebar.read_with(&self.cx, |s, _| s.rows().to_vec())
    }

    fn names(&mut self) -> Vec<String> {
        self.rows()
            .iter()
            .map(|r| match r {
                Row::Heading { title, .. } => format!("# {title}"),
                Row::Place(p) => match &p.label {
                    Label::Fixed(s) => (*s).to_string(),
                    Label::Plain(s) => s.to_string(),
                },
                Row::Folder(f) => format!("{}> {}", " ".repeat(f.depth), f.name),
                Row::Note { .. } => "!".to_string(),
                Row::Search(s) => format!("? {}", s.query),
                Row::Label(l) => format!("~ {}", l.name),
                Row::Tag(t) => format!("# {}", t.name),
                Row::Portal(p) => format!("@ {}", p.name),
                Row::AddConnection => "+ add".to_string(),
            })
            .collect()
    }

    fn role_of(&mut self, label: &str) -> Option<Role> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|e| e.role())
        })
    }

    fn has(&mut self, label: &str) -> bool {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn id_of(&mut self, label: &str) -> ElementId {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_by_label(label)
                .path()
                .last()
                .cloned()
                .expect("an element with an id")
        })
    }

    fn click(&mut self, label: &str) {
        let id = self.id_of(label);
        self.cx.update(|window, cx| window.click(id, cx));
    }

    fn middle_click(&mut self, label: &str) {
        let id = self.id_of(label);
        self.cx.update(|window, cx| {
            window.click_with_options(id, ClickOptions::new().with_button(MouseButton::Middle), cx)
        });
    }

    fn press(&mut self, key: &str) {
        self.cx.update(|window, cx| window.press(key, cx));
    }

    fn focus(&mut self) {
        let focus = self.sidebar.read_with(&self.cx, |s, _| s.focus.clone());
        self.cx.update(|window, cx| window.focus(&focus, cx));
        self.frame();
    }

    fn cursor_row(&mut self) -> Option<String> {
        let (cursor, rows) = self.sidebar.read_with(&self.cx, |s, _| (s.cursor, s.rows.clone()));
        cursor.and_then(|c| rows.get(c).map(Row::key))
    }

    fn selected(&mut self, label: &str) -> Option<bool> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|e| e.selected())
        })
    }

    fn expanded(&mut self, label: &str) -> Option<bool> {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|e| e.expanded())
        })
    }

    fn pin(&mut self, path: &str) {
        let path = path.to_string();
        self.cx
            .update(|_, cx| persistence::update_app(cx, |app| app.toggle_pin(&path)));
        self.cx.run_until_parked();
    }

    fn x_of(&mut self, id: impl Into<ElementId>) -> f32 {
        let id = id.into();
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find(id).bounds().center().x)
        })
    }
}

#[gpui_kit::test]
fn draws_the_places_with_roles_and_names(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "draws");
    assert_eq!(w.role_of("Places"), Some(Role::Navigation));
    // Sections, as the TypeScript ordered them: Pinned and Searches only appear with something in them.
    assert_eq!(
        w.names(),
        [
            "# Quick access",
            "Recent",
            "Gallery",
            "Home",
            "Documents",
            "# This PC",
            "Local Disk (C:)",
            "Photos (E:)",
            "Recycle Bin",
            "# Remote",
            "+ add",
        ]
    );
    for label in [
        "Recent",
        "Gallery",
        "Home",
        "Documents",
        "Local Disk (C:)",
        "Photos (E:)",
        "Recycle Bin",
    ] {
        assert_eq!(w.role_of(label), Some(Role::Button), "{label}");
    }
    assert_eq!(w.role_of("Quick access"), Some(Role::Heading));
    assert_eq!(w.role_of("Remote"), Some(Role::Heading));
    assert_eq!(w.role_of("This PC"), Some(Role::Button), "its title opens it");
    assert_eq!(w.role_of("Add a remote connection"), Some(Role::Button));
    // The folder shown is marked; the others are not.
    assert_eq!(w.selected("Home"), Some(true));
    assert_eq!(w.selected("Documents"), Some(false));
    // Only the removable drive can be ejected.
    assert_eq!(w.role_of("Eject Photos (E:)"), Some(Role::Button));
    assert!(!w.has("Eject Local Disk (C:)"));
    // The drive's space is in its description (what the tooltip says).
    let described = w.cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .find_by_label("Local Disk (C:)")
            .description()
            .map(str::to_string)
    });
    assert_eq!(described.as_deref(), Some("400 B free of 1000 B"));
}

#[gpui_kit::test]
fn a_click_opens_the_place_and_a_middle_click_opens_it_in_a_new_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "clicks");
    w.click("Documents");
    w.settle(|w| w.path() == DOCS && !w.tab_loading());
    assert_eq!(w.selected("Documents"), Some(true));
    assert_eq!(w.selected("Home"), Some(false));
    // Middle-click: a new tab next to this one; the one shown stays.
    let tabs_before = w.explorer.read_with(&w.cx, |e, _| e.panes()[0].tabs.len());
    w.middle_click("Recycle Bin");
    let (tabs, active) = w
        .explorer
        .read_with(&w.cx, |e, _| (e.panes()[0].tabs.clone(), e.active_tab_id()));
    assert_eq!(tabs.len(), tabs_before + 1);
    assert_eq!(w.path(), DOCS, "the tab shown does not change");
    assert!(tabs.contains(&active));
    // The title of This PC opens This PC.
    w.click("This PC");
    w.settle(|w| w.path() == THIS_PC && !w.tab_loading());
    assert_eq!(w.selected("This PC"), Some(true));
}

#[gpui_kit::test]
fn pinned_folders_show_with_their_names_and_the_menu_pins_and_unpins(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "pins");
    w.pin("D:\\Work\\تقارير 2024");
    assert_eq!(w.names()[5..7], ["# Pinned", "تقارير 2024"]);
    assert_eq!(
        w.role_of("تقارير 2024"),
        Some(Role::Button),
        "the plain name, no direction marks"
    );
    // Its right-click asks the window for the shared folder menu (which says Unpin for a pinned folder).
    let id = w.id_of("تقارير 2024");
    w.cx.update(|window, cx| window.right_click(id, cx));
    assert!(
        matches!(w.requests.borrow().last(), Some(SidebarRequest::ContextMenu { path, .. }) if path == r"D:\Work\تقارير 2024"),
        "{:?}",
        w.requests.borrow()
    );
    // Unpinning (the menu's job now) takes it out of the list.
    w.pin(r"D:\Work\تقارير 2024");
    assert!(!w.names().contains(&"# Pinned".to_string()));
    // The sidebar has no menu of its own any more.
    assert!(!w.has("Pin to sidebar") && !w.has("Unpin from sidebar"));
}

#[gpui_kit::test]
fn the_tree_opens_and_closes_from_the_keyboard_as_the_typescript_did(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "keys");
    w.focus();
    // Tabbing in lands on the folder shown.
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("p:{HOME}")));
    assert_eq!(w.expanded("Home"), Some(false));
    // Right opens it and reads its folders; the arrow key doesn't navigate.
    w.press("right");
    w.settle(|w| w.fakes.times_asked(HOME) == 1 && w.names().contains(&"> Pictures".to_string()));
    assert_eq!(w.expanded("Home"), Some(true));
    assert_eq!(w.path(), HOME);
    let names = w.names();
    let home = names.iter().position(|n| n == "Home").unwrap();
    assert_eq!(names[home + 1..home + 3], ["> Documents", "> Pictures"]);
    assert_eq!(w.role_of("Pictures"), Some(Role::TreeItem));
    // Pictures is known to hold no folders: no expanded state, Right does nothing.
    assert_eq!(w.expanded("Pictures"), None);
    // Right on an open folder goes into it; Down moves; Left on a closed child goes up to its parent.
    w.press("right");
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("f:{DOCS}")));
    w.press("down");
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("f:{HOME}\\Pictures")));
    w.press("right");
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("f:{HOME}\\Pictures")));
    w.press("left");
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("p:{HOME}")));
    // Left on an open folder closes it.
    w.press("left");
    assert_eq!(w.expanded("Home"), Some(false));
    assert!(!w.names().contains(&"> Pictures".to_string()));
    // Down moves on to the next place, and Enter does what a click does.
    w.press("down");
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("p:{DOCS}")));
    w.press("enter");
    w.settle(|w| w.path() == DOCS && !w.tab_loading());
    // Up goes back, and past the first place it stays; Home and End jump.
    w.press("up");
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("p:{HOME}")));
    w.press("home");
    assert_eq!(w.cursor_row().as_deref(), Some("p:shell:Recent"));
    w.press("up");
    assert_eq!(w.cursor_row().as_deref(), Some("p:shell:Recent"));
    w.press("end");
    assert_eq!(w.cursor_row().as_deref(), Some("add"));
}

#[gpui_kit::test]
fn clicking_a_folder_in_the_tree_opens_it_and_marks_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "tree-click");
    w.focus();
    w.press("right");
    w.settle(|w| w.names().contains(&"> Documents".to_string()));
    // The tree's row for Documents is the folder inside Home; clicking it opens it.
    let id = w.cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .find_all_by_role(Role::TreeItem)
            .first()
            .and_then(|e| e.path().last().cloned())
            .unwrap()
    });
    w.cx.update(|window, cx| window.click(id, cx));
    w.settle(|w| w.path() == DOCS && !w.tab_loading());
    let active: Vec<String> = w
        .rows()
        .iter()
        .filter_map(|r| match r {
            Row::Folder(f) if f.active => Some(f.path.clone()),
            Row::Place(p) if p.active => Some(p.path.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        active,
        [DOCS, DOCS],
        "the Documents place and its folder in Home's tree"
    );
}

#[gpui_kit::test]
fn an_explorer_change_the_sidebar_does_not_show_rebuilds_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "unchanged");
    w.settle(|w| w.path() == HOME && !w.tab_loading());
    let built = |w: &mut World| w.sidebar.read_with(&w.cx, |s, _| s.built);
    let before = built(&mut w);
    // A selection and a sort change the explorer and the settings, not what the sidebar shows.
    w.explorer.update(&mut w.cx, |e, cx| {
        e.set_selection([format!("{HOME}\\readme.txt")], None, None, cx)
    });
    w.cx.update(|_, cx| state::update(cx, |s| s.folders_first = !s.folders_first));
    w.cx.run_until_parked();
    assert_eq!(built(&mut w), before);
    // Another folder does.
    w.explorer
        .update(&mut w.cx, |e, cx| e.navigate(DOCS, NavOptions::default(), cx));
    w.settle(|w| w.path() == DOCS && !w.tab_loading());
    assert!(built(&mut w) > before);
}

#[gpui_kit::test]
fn the_tree_follows_the_folder_shown_when_asked_and_keeps_up_with_its_listing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "follows");
    // Off by default: going into Documents opens nothing.
    w.explorer.update(&mut w.cx, |e, cx| {
        e.navigate(format!("{DOCS}\\Reports"), NavOptions::default(), cx)
    });
    w.settle(|w| w.path() == format!("{DOCS}\\Reports") && !w.tab_loading());
    assert!(w.rows().iter().all(|r| !matches!(r, Row::Folder(_))));
    // On: it opens down to the folder shown, below Home (the closest place), and the folder is marked.
    w.cx.update(|_, cx| state::update(cx, |s| s.tree_follows = true));
    w.explorer
        .update(&mut w.cx, |e, cx| e.navigate(DOCS, NavOptions::default(), cx));
    w.settle(|w| w.path() == DOCS && !w.tab_loading());
    w.explorer.update(&mut w.cx, |e, cx| {
        e.navigate(format!("{DOCS}\\Reports"), NavOptions::default(), cx)
    });
    w.settle(|w| w.path() == format!("{DOCS}\\Reports") && !w.tab_loading());
    w.settle(|w| w.names().contains(&" > Reports".to_string()));
    let names = w.names();
    let home = names.iter().position(|n| n == "Home").unwrap();
    assert_eq!(names[home + 1], "> Documents");
    assert_eq!(names[home + 2], " > Reports");
    // Reports is in the tree twice: below Home's Documents and below the Documents place. Both are marked.
    let marked = w.cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .find_all_by_label("Reports")
            .iter()
            .map(|e| e.selected())
            .collect::<Vec<_>>()
    });
    assert_eq!(marked, [Some(true), Some(true)]);
}

#[gpui_kit::test]
fn a_listing_tells_the_open_folder_which_folders_it_holds(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "saw");
    w.focus();
    w.press("right");
    w.settle(|w| w.names().contains(&"> Pictures".to_string()));
    // The source knows Documents and Pictures; the disk's listing of Home also has no more folders, so the
    // same two stay. Make the source stale (it lists one folder fewer) and list Home again.
    w.fakes.set(HOME, &[("Documents", Some(true))]);
    w.explorer.update(&mut w.cx, |e, cx| {
        e.reload(None, cx).detach();
    });
    w.settle(|w| w.names().contains(&"> Pictures".to_string()));
    // The listing keeps what the tree knew of Documents (it has folders), and still shows Pictures.
    let docs = w.rows().into_iter().find_map(|r| match r {
        Row::Folder(f) if f.name == "Documents" => Some(f),
        _ => None,
    });
    assert_eq!(docs.map(|f| f.has), Some(Some(true)));
}

#[gpui_kit::test]
fn showing_hidden_folders_reads_the_open_ones_again(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "hidden");
    w.focus();
    w.press("right");
    w.settle(|w| w.names().contains(&"> Pictures".to_string()));
    assert_eq!(w.fakes.asked.lock().unwrap().last(), Some(&(HOME.to_string(), false)));
    w.cx.update(|_, cx| state::update(cx, |s| s.show_hidden = true));
    w.settle(|w| w.fakes.times_asked(HOME) == 2);
    assert_eq!(w.fakes.asked.lock().unwrap().last(), Some(&(HOME.to_string(), true)));
}

#[gpui_kit::test]
fn a_folder_that_cannot_be_read_says_so_and_a_search_reports_its_count(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "failing");
    w.focus();
    // Windows isn't listed in the fake source for D:, so opening a drive that fails shows the note.
    w.fakes.folders.lock().unwrap().clear();
    w.press("right");
    w.settle(|w| w.names().contains(&"!".to_string()));
    assert!(w.has("Can't look inside") || w.names().contains(&"!".to_string()));
    // A saved search is drawn with its count from instant search, and clicking it asks the find part to run it.
    *w.fakes.count.lock().unwrap() = Some("1,000+".to_string());
    let search = SavedSearch {
        query: "invoice".into(),
        folder: THIS_PC.into(),
    };
    let kept = search.clone();
    w.cx.update(|_, cx| persistence::update_app(cx, |app| app.toggle_saved_search(kept)));
    w.settle(|w| matches!(w.rows().iter().find(|r| matches!(r, Row::Search(_))), Some(Row::Search(s)) if s.count.as_deref() == Some("1,000+")));
    assert_eq!(w.role_of("invoice"), Some(Role::Button));
    assert_eq!(w.role_of("Searches"), Some(Role::Heading));
    w.click("invoice");
    assert_eq!(w.events.borrow().last(), Some(&SidebarEvent::RunSavedSearch(search)));
    // The cross takes it out of the sidebar.
    w.click("Remove the saved search “invoice”");
    w.cx.run_until_parked();
    assert!(w
        .cx
        .update(|_, cx| persistence::app(cx).unwrap().saved_searches.is_empty()));
    assert!(!w.names().contains(&"# Searches".to_string()));
    // Delete does the same from the keyboard.
    w.cx.update(|_, cx| {
        persistence::update_app(cx, |app| {
            app.toggle_saved_search(SavedSearch {
                query: "report".into(),
                folder: "C:\\".into(),
            })
        })
    });
    w.cx.run_until_parked();
    w.focus();
    let at = w.rows().iter().position(|r| matches!(r, Row::Search(_))).unwrap();
    w.sidebar.update(&mut w.cx, |s, cx| s.set_cursor(Some(at), cx));
    w.press("delete");
    w.cx.run_until_parked();
    assert!(w
        .cx
        .update(|_, cx| persistence::app(cx).unwrap().saved_searches.is_empty()));
}

impl World {
    /// The tag rows, in order, with their counts.
    fn tag_rows(&mut self) -> Vec<(String, Option<u64>)> {
        self.rows()
            .iter()
            .filter_map(|r| match r {
                Row::Tag(t) => Some((t.name.clone(), t.count)),
                _ => None,
            })
            .collect()
    }
}

#[gpui_kit::test]
fn the_tags_instant_search_lists_show_with_their_counts_and_a_click_finds_what_has_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "tags-indexed");
    // 15 tags in the index, most used first: the sidebar shows twelve, with how many items have each.
    let all: Vec<(String, u64)> = (0..15).map(|n| (format!("tag{n:02}"), 100 - n)).collect();
    *w.fakes.tags.lock().unwrap() = Ok(all);
    w.sidebar.update(&mut w.cx, |s, cx| s.read_tags(cx));
    w.settle(|w| w.tag_rows().len() == 12);
    assert_eq!(w.tag_rows()[0], ("tag00".to_string(), Some(100)));
    assert_eq!(w.tag_rows()[11], ("tag11".to_string(), Some(89)));
    // They sit between Pinned and Searches' place: after Quick access's places and before This PC.
    let names = w.names();
    let tags_at = names.iter().position(|n| n == "# Tags").expect("a Tags title");
    assert_eq!(names[tags_at + 1], "# tag00");
    assert!(names.iter().position(|n| n == "# This PC").unwrap() > tags_at);
    // Named for a screen reader: the tag, and what a click does with how many have it.
    assert_eq!(w.role_of("Tags"), Some(Role::Heading));
    assert_eq!(w.role_of("tag00"), Some(Role::Button));
    // A click finds what has it, on every drive: a saved search of `tag:"name"` at This PC.
    w.click("tag03");
    assert_eq!(
        w.events.borrow().last(),
        Some(&SidebarEvent::RunSavedSearch(SavedSearch {
            query: "tag:\"tag03\"".into(),
            folder: THIS_PC.into(),
        }))
    );
    // The keyboard does the same: Enter on the row it is on.
    w.focus();
    let at = w
        .rows()
        .iter()
        .position(|r| matches!(r, Row::Tag(t) if t.name == "tag05"))
        .unwrap();
    w.sidebar.update(&mut w.cx, |s, cx| s.set_cursor(Some(at), cx));
    w.press("enter");
    assert_eq!(
        w.events.borrow().last(),
        Some(&SidebarEvent::RunSavedSearch(tag_search("tag05")))
    );
}

#[gpui_kit::test]
fn with_no_tags_from_instant_search_the_recent_ones_show_and_setting_a_tag_adds_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "tags-recent");
    // Instant search is off (its answer is an error) and nothing was tagged yet: no section at all.
    w.cx.run_until_parked();
    assert!(w.tag_rows().is_empty());
    assert!(!w.names().contains(&"# Tags".to_string()));
    // Tagging something makes the tag recent, and the sidebar lists it without a count.
    w.cx.update(|window, cx| {
        tags::set_tags(
            window,
            cx,
            vec![format!("{HOME}\\readme.txt")],
            vec!["Work".to_string()],
            Vec::new(),
        )
    });
    w.settle(|w| w.tag_rows() == [("Work".to_string(), None)]);
    assert!(w.has("Work"));
    // Asked again when the recent tags changed (after marks were asked and failed).
    w.settle(|w| *w.fakes.tags_asked.lock().unwrap() >= 2);
    // Instant search answers now: its list wins, with counts.
    *w.fakes.tags.lock().unwrap() = Ok(vec![("Work".to_string(), 7), ("Home".to_string(), 2)]);
    w.cx.update(|window, cx| {
        tags::set_tags(
            window,
            cx,
            vec![format!("{HOME}\\readme.txt")],
            vec!["Later".to_string()],
            Vec::new(),
        )
    });
    w.settle(|w| w.tag_rows() == [("Work".to_string(), Some(7)), ("Home".to_string(), Some(2))]);
}

#[gpui_kit::test]
fn an_arabic_tag_draws_with_its_name_and_the_mirrored_row_still_fills_the_sidebar(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "tags-arabic");
    *w.fakes.tags.lock().unwrap() = Ok(vec![("ضريبة".to_string(), 4)]);
    w.sidebar.update(&mut w.cx, |s, cx| s.read_tags(cx));
    w.settle(|w| w.tag_rows().len() == 1);
    assert_eq!(w.role_of("ضريبة"), Some(Role::Button));
    let ix = w.rows().iter().position(|r| matches!(r, Row::Tag(_))).unwrap();
    let english = w.bounds_of(("tag", ix));
    testing::set_rtl(true);
    w.frame();
    let mirrored = w.bounds_of(("tag", ix));
    // Mirroring changes what is at which end, not how wide the row is.
    assert_eq!(english.size.width, mirrored.size.width);
    assert_eq!(w.role_of("ضريبة"), Some(Role::Button));
    testing::set_rtl(false);
}

fn mark_label(name: &str, color: u32, was: &[&str]) -> glint_model::marks::Label {
    glint_model::marks::Label {
        name: name.into(),
        color,
        was: was.iter().map(|w| (*w).to_string()).collect(),
    }
}

const FILE_A: &str = "C:\\Users\\me\\a.txt";
const FILE_B: &str = "D:\\work\\b.txt";

impl World {
    fn set_labels(&mut self, labels: Vec<glint_model::marks::Label>) {
        self.cx.update(|_, cx| state::update(cx, |s| s.labels = labels));
        self.cx.run_until_parked();
    }

    /// The label rows, in order, with their counts.
    fn label_rows(&mut self) -> Vec<(String, Option<u64>)> {
        self.rows()
            .iter()
            .filter_map(|r| match r {
                Row::Label(l) => Some((l.name.clone(), l.count)),
                _ => None,
            })
            .collect()
    }

    /// Tells the sidebar what the index and the ledger count, and waits for it to be listed.
    fn count_marks(&mut self, labels: &[(&str, u64)], tags: &[(&str, u64)]) {
        let own = |list: &[(&str, u64)]| list.iter().map(|(n, c)| ((*n).to_string(), *c)).collect::<Vec<_>>();
        *self.fakes.marks.lock().unwrap() = Ok((own(labels), own(tags)));
        self.sidebar.update(&mut self.cx, |s, cx| s.read_tags(cx));
        self.settle(|w| w.fakes.marks.lock().unwrap().is_ok());
        self.cx.run_until_parked();
    }

    /// The keyboard on the tag's row, and its menu opened with the menu key.
    fn open_tag_menu_by_key(&mut self, tag: &str) {
        self.focus();
        let ix = self.row_of(format!("t:{tag}"));
        self.sidebar.update(&mut self.cx, |s, cx| s.set_cursor(Some(ix), cx));
        self.press("menu");
        self.frame();
    }

    fn dialog_open(&mut self) -> bool {
        self.cx.update(|window, cx| window.has_active_dialog(cx))
    }

    fn type_into_dialog(&mut self, text: &str) {
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.press("ctrl-a", cx);
            if text.is_empty() {
                window.press("backspace", cx);
            } else {
                window.input(text, cx);
            }
        });
        self.cx.run_until_parked();
    }

    fn sets(&mut self) -> Vec<TagsChange> {
        self.fakes.sets.lock().unwrap().clone()
    }
}

#[gpui_kit::test]
fn labels_come_above_tags_in_the_settings_order_with_their_counts_and_a_tag_named_as_one_is_counted_with_it(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let mut w = world(cx, "labels-counts");
    w.set_labels(vec![
        mark_label("Important", 0xef4444, &["Red"]),
        mark_label("Later", 0x3b82f6, &[]),
        mark_label("ضريبة", 0x22c55e, &[]),
    ]);
    // Important has 3 under its name and 5 under the one it was; Later has none but a 0.16 tag of its name has 2.
    w.count_marks(
        &[("Important", 3), ("red", 5), ("ضريبة", 4)],
        &[("Work", 9), ("Later", 2), ("Red", 1)],
    );
    w.settle(|w| w.label_rows().iter().any(|(_, count)| count.is_some()));
    assert_eq!(
        w.label_rows(),
        [
            ("Important".to_string(), Some(9)),
            ("Later".to_string(), Some(2)),
            ("ضريبة".to_string(), Some(4)),
        ]
    );
    // Above Tags, after Pinned; and a tag that is a label (or was one) is not in Tags.
    let names = w.names();
    let at = |what: &str| names.iter().position(|n| n == what).unwrap();
    assert!(at("Documents") < at("# Labels") && at("# Labels") < at("# Tags") && at("# Tags") < at("# This PC"));
    assert_eq!(&names[at("# Labels")..at("# Tags") + 2], ["# Labels", "~ Important", "~ Later", "~ ضريبة", "# Tags", "# Work"]);
    assert_eq!(w.tag_rows(), [("Work".to_string(), Some(9))]);
    // Every row has a role and a name.
    assert_eq!(w.role_of("Labels"), Some(Role::Heading));
    assert_eq!(w.role_of("Label Important, 9"), Some(Role::Button));
    assert_eq!(w.role_of("Label ضريبة, 4"), Some(Role::Button));
}

#[gpui_kit::test]
fn with_no_counts_known_the_labels_have_no_numbers_and_with_no_labels_there_is_no_section(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "labels-bare");
    // Marks can't be counted and neither can the tags: the seven of Settings are listed, each without a number.
    w.set_labels(glint_model::marks::default_labels());
    let labels = w.label_rows();
    assert_eq!(labels.len(), 7);
    assert_eq!(labels[0], ("Red".to_string(), None));
    assert!(labels.iter().all(|(_, count)| count.is_none()));
    assert_eq!(w.role_of("Label Red"), Some(Role::Button));
    // Marks fail, so the tags are asked of instant search as before, and a tag of a label's name counts for it.
    *w.fakes.tags.lock().unwrap() = Ok(vec![("Work".to_string(), 4), ("red".to_string(), 6)]);
    w.sidebar.update(&mut w.cx, |s, cx| s.read_tags(cx));
    w.settle(|w| w.tag_rows() == [("Work".to_string(), Some(4))]);
    assert_eq!(w.label_rows()[0], ("Red".to_string(), Some(6)));
    // No label in Settings: no section, no title.
    w.set_labels(Vec::new());
    assert!(w.label_rows().is_empty());
    assert!(!w.names().contains(&"# Labels".to_string()));
}

#[gpui_kit::test]
fn a_press_on_a_label_finds_what_has_it_at_this_pc_by_the_mouse_and_by_enter(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "labels-press");
    w.set_labels(vec![mark_label("Important", 0xef4444, &[]), mark_label("Later", 0x3b82f6, &[])]);
    w.count_marks(&[("Important", 3)], &[]);
    w.settle(|w| w.label_rows()[0].1 == Some(3));
    w.click("Label Important, 3");
    assert_eq!(
        w.events.borrow().last(),
        Some(&SidebarEvent::RunSavedSearch(SavedSearch {
            query: "label:\"Important\"".into(),
            folder: THIS_PC.into(),
        }))
    );
    // The keyboard walks label rows as it walks tag rows: Down from one lands on the next, Enter finds it.
    w.focus();
    let first = w.row_of("l:Important".to_string());
    w.sidebar.update(&mut w.cx, |s, cx| s.set_cursor(Some(first), cx));
    w.press("down");
    assert_eq!(w.cursor_row().as_deref(), Some("l:Later"));
    w.press("enter");
    assert_eq!(
        w.events.borrow().last(),
        Some(&SidebarEvent::RunSavedSearch(rows::label_search("Later")))
    );
    // Down again: past the last label the keyboard reaches what follows (a tag, else the next place).
    w.press("down");
    assert_ne!(w.cursor_row().as_deref(), Some("l:Later"));
}

#[gpui_kit::test]
fn the_mirrored_rows_of_a_label_and_a_tag_with_arabic_names_still_fill_the_sidebar(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "labels-arabic");
    w.set_labels(vec![mark_label("مهم", 0xef4444, &[])]);
    w.count_marks(&[("مهم", 2)], &[("ضريبة", 4)]);
    w.settle(|w| w.label_rows() == [("مهم".to_string(), Some(2))] && w.tag_rows().len() == 1);
    let label_ix = w.row_of("l:مهم".to_string());
    let tag_ix = w.row_of("t:ضريبة".to_string());
    let english = (w.bounds_of(("label", label_ix)), w.bounds_of(("tag", tag_ix)));
    testing::set_rtl(true);
    w.frame();
    let mirrored = (w.bounds_of(("label", label_ix)), w.bounds_of(("tag", tag_ix)));
    assert_eq!(english.0.size.width, mirrored.0.size.width);
    assert_eq!(english.1.size.width, mirrored.1.size.width);
    assert_eq!(w.role_of("Label مهم, 2"), Some(Role::Button));
    assert_eq!(w.role_of("ضريبة"), Some(Role::Button));
    testing::set_rtl(false);
}

/// A world with the tag "Work" listed, and two files that have it.
fn world_with_work(cx: &mut TestAppContext, name: &str) -> World {
    let mut w = world(cx, name);
    w.count_marks(&[], &[("Work", 2)]);
    w.settle(|w| w.tag_rows() == [("Work".to_string(), Some(2))]);
    *w.fakes.tagged.lock().unwrap() = Ok(vec![FILE_A.to_string(), FILE_B.to_string()]);
    w
}

#[gpui_kit::test]
fn the_menu_key_on_a_tags_row_opens_its_two_rows_and_not_the_shared_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world_with_work(cx, "tag-menu");
    let before = w.rows();
    w.open_tag_menu_by_key("Work");
    assert!(w.has("Rename everywhere…") && w.has("Take off every file…"));
    assert!(w.requests.borrow().is_empty(), "the shared folder menu was not asked for");
    // Nothing is searched until a row is chosen; Escape closes it and changes nothing.
    assert!(w.fakes.tagged_asked.lock().unwrap().is_empty());
    w.press("escape");
    assert!(!w.has("Rename everywhere…"));
    assert_eq!(w.rows(), before);
    // Shift+F10 and the pointer open it as well.
    w.press("shift-f10");
    w.frame();
    assert!(w.has("Take off every file…"));
    w.press("escape");
    let id = w.id_of("Work");
    w.cx.update(|window, cx| window.right_click(id, cx));
    w.frame();
    assert!(w.has("Rename everywhere…"));
}

#[gpui_kit::test]
fn rename_everywhere_asks_set_tags_once_with_the_paths_found_and_an_empty_or_unchanged_name_asks_nothing(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let mut w = world_with_work(cx, "tag-rename");
    w.open_tag_menu_by_key("Work");
    w.click("Rename everywhere…");
    w.settle(|w| w.dialog_open());
    assert_eq!(*w.fakes.tagged_asked.lock().unwrap(), ["Work".to_string()]);
    assert!(w.has("New name"));
    // Empty, the same, the same in another case: refused in the dialog, which stays.
    for refused in ["", "Work", "  work "] {
        w.type_into_dialog(refused);
        w.press("enter");
        w.cx.run_until_parked();
        assert!(w.dialog_open(), "{refused:?} is refused");
        assert!(w.has("Give the tag another name"));
        assert!(w.sets().is_empty(), "{refused:?} asks nothing");
    }
    // A new name: one change over what the source gave, adding it and taking the old one off.
    w.type_into_dialog("Projects");
    w.press("enter");
    w.settle(|w| !w.sets().is_empty());
    assert_eq!(
        w.sets(),
        [(
            vec![FILE_A.to_string(), FILE_B.to_string()],
            vec!["Projects".to_string()],
            vec!["Work".to_string()]
        )]
    );
    assert!(!w.dialog_open(), "Rename closes the dialog");
}

#[gpui_kit::test]
fn take_off_every_file_asks_with_the_number_and_yes_takes_the_tag_off_those_paths_and_no_does_nothing(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let mut w = world_with_work(cx, "tag-take-off");
    // No: nothing is changed.
    w.open_tag_menu_by_key("Work");
    w.click("Take off every file…");
    w.settle(|w| w.dialog_open());
    assert!(w.has("Take off “Work”"), "the dialog is about the tag");
    let said = w.cx.update(|window, _| {
        window
            .try_find_by_label("Take off “Work”")
            .and_then(|e| e.description().map(str::to_string))
    });
    assert_eq!(said.as_deref(), Some("Take “Work” off 2 items?"));
    w.click("Cancel");
    w.cx.run_until_parked();
    assert!(!w.dialog_open());
    assert!(w.sets().is_empty());
    // Yes: the tag comes off every path the source gave, as one change.
    w.open_tag_menu_by_key("Work");
    w.click("Take off every file…");
    w.settle(|w| w.dialog_open());
    w.click("Take off");
    w.settle(|w| !w.sets().is_empty());
    assert_eq!(
        w.sets(),
        [(
            vec![FILE_A.to_string(), FILE_B.to_string()],
            Vec::new(),
            vec!["Work".to_string()]
        )]
    );
}

#[gpui_kit::test]
fn a_source_that_finds_nothing_says_so_and_opens_no_dialog(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world_with_work(cx, "tag-nothing");
    let notices = |w: &mut World| {
        w.frame();
        w.cx.update(|window, cx| window.notifications(cx).len())
    };
    for found in [Ok(Vec::new()), Err("The index is off".to_string())] {
        *w.fakes.tagged.lock().unwrap() = found.clone();
        for row in ["Rename everywhere…", "Take off every file…"] {
            let before = notices(&mut w);
            w.open_tag_menu_by_key("Work");
            w.click(row);
            // One notice more, and no dialog.
            w.settle(|w| notices(w) == before + 1);
            assert!(!w.dialog_open(), "{found:?}: {row} opened nothing");
        }
    }
    assert!(w.sets().is_empty());
}

#[gpui_kit::test]
fn setting_a_tag_reads_the_counts_again_labels_included(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "labels-changed");
    w.set_labels(vec![mark_label("Important", 0xef4444, &[])]);
    w.count_marks(&[("Important", 1)], &[]);
    w.settle(|w| w.label_rows() == [("Important".to_string(), Some(1))]);
    // Something was marked: the index now counts more, and TagsChanged makes the sidebar ask again.
    *w.fakes.marks.lock().unwrap() = Ok((vec![("Important".to_string(), 5)], vec![("Work".to_string(), 2)]));
    w.cx.update(|window, cx| {
        tags::set_tags(window, cx, vec![FILE_A.to_string()], vec!["Work".to_string()], Vec::new())
    });
    w.settle(|w| w.label_rows() == [("Important".to_string(), Some(5))] && w.tag_rows().len() == 1);
}

#[gpui_kit::test]
fn connections_show_their_cloud_space_and_the_add_button_asks_for_the_dialog(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "remotes");
    *w.fakes.cloud.lock().unwrap() = Some((2000, 500));
    let remotes = vec![
        RemoteConnection {
            id: "gd".into(),
            name: "My Drive".into(),
            kind: RemoteKind::Rclone,
            rclone: Some("gd".into()),
            service: Some("drive".into()),
            ..Default::default()
        },
        RemoteConnection {
            id: "nas".into(),
            name: "NAS".into(),
            kind: RemoteKind::Sftp,
            host: Some("nas.local".into()),
            ..Default::default()
        },
    ];
    w.cx.update(|_, cx| persistence::update_app(cx, |app| app.remotes = remotes));
    w.settle(|w| {
        w.rows()
            .iter()
            .any(|r| matches!(r, Row::Place(p) if p.remote.as_deref() == Some("gd") && p.capacity.is_some()))
    });
    let names = w.names();
    let remote = names.iter().position(|n| n == "# Remote").unwrap();
    assert_eq!(names[remote + 1..], ["My Drive", "NAS", "+ add"]);
    // Only the rclone connection has a cloud space to ask for; the SFTP one shows none.
    assert!(w
        .rows()
        .iter()
        .all(|r| !matches!(r, Row::Place(p) if p.remote.as_deref() == Some("nas") && p.capacity.is_some())));
    w.click("Add a remote connection");
    assert_eq!(w.events.borrow().last(), Some(&SidebarEvent::AddConnection));
    // A connection's menu is the shared folder menu, asked of the window for the connection's top.
    let id = w.id_of("NAS");
    w.cx.update(|window, cx| window.right_click(id, cx));
    assert!(
        matches!(w.requests.borrow().last(), Some(SidebarRequest::ContextMenu { path, .. }) if path == "remote:nas"),
        "{:?}",
        w.requests.borrow()
    );
}

#[gpui_kit::test]
fn arabic_names_draw_and_the_arabic_interface_mirrors(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "arabic");
    w.pin("D:\\تقارير");
    w.focus();
    w.press("right");
    w.settle(|w| w.names().contains(&"> Pictures".to_string()));
    // English: the open Home's arrow is by the start (left) edge of the place, the label to its right.
    let home_ix = w.rows().iter().position(|r| r.key() == format!("p:{HOME}")).unwrap();
    let arrow_en = w.x_of(("arrow", home_ix));
    let label_en = w.x_of(("place", home_ix));
    assert!(arrow_en < label_en);
    testing::set_rtl(true);
    w.frame();
    let ix = w.rows().iter().position(|r| r.key() == format!("p:{HOME}")).unwrap();
    let arrow_ar = w.x_of(("arrow", ix));
    let place_ar = w.x_of(("place", ix));
    assert!(
        arrow_ar > place_ar,
        "the arrow is at the start edge, which is the right in Arabic"
    );
    // The Arabic folder keeps its plain name for the screen reader and is drawn.
    assert_eq!(w.role_of("تقارير"), Some(Role::Button));
    // Right closes in the mirrored interface and Left opens: the keys follow the reading direction. (The places
    // are named in Arabic now, so the rows are asked rather than the accessibility tree.)
    let home_open = |w: &mut World| w.rows().iter().any(|r| r.tree_path() == Some(HOME) && r.is_open());
    assert!(home_open(&mut w));
    w.press("right");
    assert!(!home_open(&mut w));
    w.press("left");
    w.settle(|w| w.names().contains(&"> Pictures".to_string()));
    assert!(home_open(&mut w));
    testing::set_rtl(false);
}

impl World {
    fn row_of(&mut self, key: String) -> usize {
        self.rows().iter().position(|r| r.key() == key).expect("a row")
    }

    fn bounds_of(&mut self, id: impl Into<ElementId>) -> Bounds<Pixels> {
        let id = id.into();
        self.cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).bounds()
        })
    }

    /// Drags `files` over the middle of element `id` (entering, then moving), as File Explorer would.
    fn drag_over(&mut self, answers: &DropAnswers, files: &[String], id: impl Into<ElementId>, effect: DropEffect) {
        let at = self.bounds_of(id).center();
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        let paths = files.to_vec();
        self.cx.update(|window, cx| {
            crate::drops::dispatch(DropEvent::Leave, answers, window, cx);
            crate::drops::dispatch(DropEvent::Enter { paths, x, y, effect }, answers, window, cx);
            crate::drops::dispatch(DropEvent::Over { x, y, effect }, answers, window, cx);
        });
        self.frame();
    }

    fn drop_at(&mut self, answers: &DropAnswers, files: &[String], id: impl Into<ElementId>, effect: DropEffect) {
        let at = self.bounds_of(id).center();
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        let paths = files.to_vec();
        self.cx.update(|window, cx| {
            crate::drops::dispatch(DropEvent::Drop { paths, x, y, effect }, answers, window, cx);
        });
        self.cx.run_until_parked();
    }

    fn lit(&mut self) -> Option<String> {
        self.sidebar.read_with(&self.cx, |s, _| s.drop_hover.clone())
    }

    fn cursor_row_ix(&mut self) -> usize {
        self.sidebar
            .read_with(&self.cx, |s, _| s.cursor)
            .expect("the keyboard is on a row")
    }
}

#[gpui_kit::test]
fn the_menu_key_and_shift_f10_ask_for_the_menu_at_the_rows_bottom_start_corner(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "menu-key");
    w.focus();
    assert_eq!(w.cursor_row().as_deref(), Some(&*format!("p:{HOME}")));
    let ix = w.row_of(format!("p:{HOME}"));
    let row = w.bounds_of(("place", ix));
    w.press("menu");
    let asked = |w: &World| w.requests.borrow().clone();
    let first = asked(&w);
    assert_eq!(first.len(), 1, "{first:?}");
    let SidebarRequest::ContextMenu { path, position } = first[0].clone();
    assert_eq!(path, HOME);
    assert!(
        (f32::from(position.x) - f32::from(row.left())).abs() < 1.5,
        "the start edge"
    );
    let below = f32::from(position.y) - f32::from(row.bottom());
    assert!((0. ..4.).contains(&below), "just under the row: {below}");
    // Shift+F10 is the same; F10 alone is not.
    w.press("f10");
    assert_eq!(asked(&w).len(), 1);
    w.press("shift-f10");
    assert_eq!(asked(&w).len(), 2);
    assert_eq!(asked(&w)[1], first[0]);
    // On a folder in the tree it asks for that folder; in Arabic the start edge is the right one.
    w.press("right");
    w.settle(|w| w.names().contains(&"> Pictures".to_string()));
    w.press("down");
    testing::set_rtl(true);
    w.frame();
    let ix = w.cursor_row_ix();
    let row = w.bounds_of(("folder", ix));
    w.press("menu");
    let last = asked(&w).pop().expect("a request");
    let SidebarRequest::ContextMenu { path, position } = last;
    assert!(path.starts_with(HOME), "{path}");
    assert!(
        (f32::from(position.x) - f32::from(row.right())).abs() < 1.5,
        "the right edge in Arabic"
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn ejecting_goes_through_the_explorer_and_the_button_is_a_tab_stop(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "eject");
    // Tab moves from the list through the Workspaces button (at the top) on to the eject button of the removable
    // drive, which then shows.
    w.focus();
    w.cx.update(|window, cx| window.focus_next(cx));
    w.frame();
    let sidebar = w.sidebar.clone();
    let on_button =
        w.cx.update(|window, cx| sidebar.read(cx).eject_focus.values().any(|h| h.is_focused(window)));
    assert!(!on_button, "the Workspaces button comes first");
    w.cx.update(|window, cx| window.focus_next(cx));
    w.frame();
    let sidebar = w.sidebar.clone();
    let on_button =
        w.cx.update(|window, cx| sidebar.read(cx).eject_focus.values().any(|h| h.is_focused(window)));
    assert!(on_button, "Tab reaches the eject button");
    assert!(w.disk.ejected().is_empty());
    w.press("enter");
    w.cx.run_until_parked();
    assert_eq!(w.disk.ejected(), ["E:\\"], "ejected through the explorer's source");
    // The mouse does the same.
    w.click("Eject Photos (E:)");
    w.cx.run_until_parked();
    assert_eq!(w.disk.ejected().len(), 2);
}

#[gpui_kit::test]
fn files_dropped_on_a_place_or_a_tree_folder_are_copied_or_moved_into_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "drop-folder");
    w.disk.folder(
        "D:\\Inbox",
        vec![file("D:\\Inbox", "a.txt", 3, 1), file("D:\\Inbox", "ملف.txt", 3, 1)],
    );
    let files = vec!["D:\\Inbox\\a.txt".to_string(), "D:\\Inbox\\ملف.txt".to_string()];
    let answers = DropAnswers::default();
    let docs = w.row_of(format!("p:{DOCS}"));
    // Over Documents: the drag says where it goes, and the row is lit.
    w.drag_over(&answers, &files, ("place", docs), DropEffect::Copy);
    match answers.get() {
        Answer::Accept(under) => {
            assert_eq!(under.name, "Documents");
            assert_eq!(under.copy, "Copy to %1");
            assert_eq!(under.moves, "Move to %1");
            assert!(!under.always_move);
        }
        other => panic!("expected Accept, got {other:?}"),
    }
    assert_eq!(w.lit(), Some(format!("p:{DOCS}")));
    // A copy leaves the files where they were.
    w.drop_at(&answers, &files, ("place", docs), DropEffect::Copy);
    w.settle(|w| w.disk.names(DOCS).contains(&"ملف.txt".to_string()));
    assert_eq!(w.lit(), None, "the highlight goes with the drop");
    assert!(w.disk.names("D:\\Inbox").contains(&"a.txt".to_string()));
    // Shift moves them.
    w.disk.folder("D:\\Box", vec![file("D:\\Box", "b.txt", 3, 1)]);
    let moved = vec!["D:\\Box\\b.txt".to_string()];
    w.drag_over(&answers, &moved, ("place", docs), DropEffect::Move);
    w.drop_at(&answers, &moved, ("place", docs), DropEffect::Move);
    w.settle(|w| w.disk.names(DOCS).contains(&"b.txt".to_string()));
    assert!(w.disk.names("D:\\Box").is_empty());
    // A folder in the tree takes them too, and a folder that already holds them refuses.
    w.focus();
    w.press("right");
    w.settle(|w| w.names().contains(&"> Pictures".to_string()));
    let pictures = w.row_of(format!("f:{HOME}\\Pictures"));
    w.drag_over(&answers, &files, ("folder", pictures), DropEffect::Copy);
    assert!(matches!(answers.get(), Answer::Accept(u) if u.name == "Pictures"));
    let home_files = vec![format!("{HOME}\\readme.txt")];
    let home = w.row_of(format!("p:{HOME}"));
    w.drag_over(&answers, &home_files, ("place", home), DropEffect::Copy);
    assert_eq!(answers.get(), Answer::Refuse, "already in Home");
    assert_eq!(w.lit(), None);
}

#[gpui_kit::test]
fn views_remotes_and_titles_refuse_files_and_the_bin_takes_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "drop-refuse");
    let remotes = vec![RemoteConnection {
        id: "nas".into(),
        name: "NAS".into(),
        kind: RemoteKind::Sftp,
        host: Some("nas.local".into()),
        ..Default::default()
    }];
    w.cx.update(|_, cx| persistence::update_app(cx, |app| app.remotes = remotes));
    w.settle(|w| w.names().contains(&"NAS".to_string()));
    w.disk.folder("D:\\Inbox", vec![file("D:\\Inbox", "a.txt", 3, 1)]);
    let files = vec!["D:\\Inbox\\a.txt".to_string()];
    let answers = DropAnswers::default();
    // Recent, Gallery, a read-only remote and the This PC title take nothing.
    for (key, id) in [
        ("p:shell:Recent", "place"),
        ("p:shell:Gallery", "place"),
        ("p:remote:nas", "place"),
        ("h:This PC", "heading"),
    ] {
        let ix = w.row_of(key.to_string());
        w.drag_over(&answers, &files, (id, ix), DropEffect::Copy);
        assert_eq!(answers.get(), Answer::Refuse, "{key}");
        assert_eq!(w.lit(), None, "{key}");
    }
    // The Recycle Bin always moves.
    let bin = w.row_of(format!("p:{}", glint_model::paths::RECYCLE_BIN));
    w.drag_over(&answers, &files, ("place", bin), DropEffect::Copy);
    assert!(matches!(answers.get(), Answer::Accept(u) if u.always_move && u.name == "Recycle Bin"));
    // A drop on the Recycle Bin recycles them.
    w.drag_over(&answers, &files, ("place", bin), DropEffect::Move);
    w.drop_at(&answers, &files, ("place", bin), DropEffect::Move);
    w.settle(|w| !w.disk.names("D:\\Inbox").contains(&"a.txt".to_string()));
    assert_eq!(w.disk.bin_names(), ["a.txt"]);
}

#[gpui_kit::test]
fn the_icons_the_sidebar_draws_are_in_the_embedded_assets(_cx: &mut TestAppContext) {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    for name in [
        IconName::Clock,
        IconName::Images,
        IconName::Folder,
        IconName::FolderOpen,
        IconName::Trash,
        IconName::Unplug,
        IconName::Cloud,
        IconName::Database,
        IconName::Server,
        IconName::SquareTerminal,
        IconName::Monitor,
        IconName::ChevronDown,
        IconName::ChevronLeft,
        IconName::ChevronRight,
        IconName::LoaderCircle,
        IconName::Plus,
        IconName::Search,
        IconName::X,
        IconName::Pin,
        IconName::PinOff,
        IconName::ClipboardCopy,
        IconName::SquarePlus,
        IconName::TextSearch,
        IconName::PlugZap,
        IconName::PencilOff,
        IconName::Archive,
        IconName::GitBranch,
        IconName::DatabaseBackup,
        IconName::RefreshCw,
        IconName::Copy,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

// ---- Portals ------------------------------------------------------------------------------------------

/// A portal's folder on the fake disk, so opening it lands there.
fn portal_folder(w: &World, name: &str) -> String {
    let path = format!("C:\\Users\\me\\Glint portals\\{name}");
    w.disk.folder(&path, vec![file(&path, "inside.txt", 5, 1)]);
    path
}

#[gpui_kit::test]
fn mounted_portals_get_a_section_that_follows_mounting_and_unmounting(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "portals-section");
    assert!(
        !w.names().contains(&"# Portals".to_string()),
        "nothing mounted, no section"
    );
    let before_rebuilds = w.sidebar.read_with(&w.cx, |s, _| s.built);

    portal_folder(&w, "pack.zip");
    portal_folder(&w, "تقارير");
    w.fakes.mount("a", "pack.zip", PortalKind::Archive);
    w.fakes.mount("b", "تقارير", PortalKind::Search);
    w.settle(|w| w.names().contains(&"@ تقارير".to_string()));
    let names = w.names();
    let at = |what: &str| names.iter().position(|n| n == what).unwrap();
    assert_eq!(
        &names[at("# Portals")..at("# Portals") + 3],
        ["# Portals", "@ pack.zip", "@ تقارير"]
    );
    assert!(at("# Portals") < at("# This PC"));
    assert!(w.sidebar.read_with(&w.cx, |s, _| s.built) > before_rebuilds);

    // Drawn as buttons with their names.
    assert_eq!(w.role_of("pack.zip"), Some(Role::Button));
    assert_eq!(w.role_of("تقارير"), Some(Role::Button));

    // Unmounted from anywhere else (the palette, a menu): the row goes with no one asking.
    w.fakes.portals.lock().unwrap().retain(|p| p.id != "a");
    w.fakes.tell_portals();
    w.settle(|w| !w.names().contains(&"@ pack.zip".to_string()));
    w.fakes.portals.lock().unwrap().clear();
    w.fakes.tell_portals();
    w.settle(|w| !w.names().contains(&"# Portals".to_string()));
}

#[gpui_kit::test]
fn a_click_opens_the_portal_and_the_row_is_current_while_it_is_shown(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "portals-open");
    let path = portal_folder(&w, "Repo");
    w.fakes.mount("g", "Repo", PortalKind::Git);
    w.settle(|w| w.names().contains(&"@ Repo".to_string()));
    w.click("Repo");
    w.settle(|w| w.path() == path && !w.tab_loading());
    assert_eq!(w.selected("Repo"), Some(true));
    w.click("Home");
    w.settle(|w| w.path() == HOME && !w.tab_loading());
    assert_eq!(w.selected("Repo"), Some(false));
}

#[gpui_kit::test]
fn the_portals_menu_opens_from_the_pointer_and_the_keyboard_and_does_its_four_things(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "portals-menu");
    let path = portal_folder(&w, "Repo");
    w.fakes.mount("g", "Repo", PortalKind::Git);
    w.settle(|w| w.names().contains(&"@ Repo".to_string()));
    let rows_before = w.rows();

    // The keyboard: the row, then the menu key.
    w.focus();
    let ix = w.row_of("o:g".to_string());
    w.sidebar.update(&mut w.cx, |s, cx| s.set_cursor(Some(ix), cx));
    w.press("menu");
    w.frame();
    for item in ["Open", "Copy path", "Refresh", "Unmount"] {
        assert!(w.has(item), "{item} is in the portal's menu");
    }
    // The menu is the portal's: the shared folder menu was not asked for.
    assert!(w.requests.borrow().is_empty());

    // Refresh reads its source again and changes no row.
    w.click("Refresh");
    w.settle(|w| w.fakes.portal_log.lock().unwrap().contains(&"refresh:g".to_string()));
    assert!(!w.has("Unmount"), "the menu closed");
    assert_eq!(w.rows(), rows_before);

    // Copy path puts the folder on the clipboard.
    w.press("shift-f10");
    w.frame();
    w.click("Copy path");
    w.cx.update(|_, cx| {
        assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(path.clone()));
    });

    // Open goes there.
    let id = w.id_of("Repo");
    w.cx.update(|window, cx| window.right_click(id, cx));
    w.frame();
    w.click("Open");
    w.settle(|w| w.path() == path && !w.tab_loading());

    // Unmount asks nothing, and the row is gone once core says so.
    let id = w.id_of("Repo");
    w.cx.update(|window, cx| window.right_click(id, cx));
    w.frame();
    w.click("Unmount");
    w.settle(|w| !w.names().contains(&"@ Repo".to_string()));
    assert_eq!(
        w.fakes.portal_log.lock().unwrap().last().map(String::as_str),
        Some("unmount:g")
    );
}

#[gpui_kit::test]
fn the_arrows_reach_portals_and_enter_opens_one_and_the_mirrored_row_fills_the_sidebar(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "portals-keys");
    let path = portal_folder(&w, "تقارير");
    w.fakes.mount("b", "تقارير", PortalKind::Backup);
    w.settle(|w| w.names().contains(&"@ تقارير".to_string()));
    w.focus();
    let ix = w.row_of("o:b".to_string());
    w.sidebar.update(&mut w.cx, |s, cx| s.set_cursor(Some(ix), cx));
    w.press("down");
    assert_eq!(w.cursor_row().as_deref(), Some("h:This PC"));
    w.press("up");
    assert_eq!(w.cursor_row().as_deref(), Some("o:b"));
    w.press("enter");
    w.settle(|w| w.path() == path && !w.tab_loading());

    testing::set_rtl(true);
    w.frame();
    let sidebar = w.bounds_of("places");
    let row = w.bounds_of(("portal", ix));
    assert!(row.size.width > sidebar.size.width * 0.8, "the row fills the sidebar");
    testing::set_rtl(false);
}

#[test]
fn a_portal_is_told_by_its_source() {
    use glint_core::portals::{Portal, PortalSource};
    let portal = |source| Portal {
        id: "p".into(),
        name: "n".into(),
        mount_path: "C:\\m".into(),
        source,
    };
    let items: Vec<_> = [
        PortalSource::Archive {
            archive_path: "C:\\a.zip".into(),
        },
        PortalSource::SavedSearch {
            query: "invoice".into(),
            root: "C:\\Docs".into(),
        },
        PortalSource::GitCommit {
            repo_root: "C:\\repo".into(),
            rev: "0123456789abcdef".into(),
            subfolder: String::new(),
        },
        PortalSource::ResticSnapshot {
            plan_id: "plan".into(),
            snapshot_id: "fedcba9876543210".into(),
            subfolder: String::new(),
        },
    ]
    .into_iter()
    .map(|source| portals::item_of(&portal(source)))
    .collect();
    assert_eq!(
        items.iter().map(|k| k.kind).collect::<Vec<_>>(),
        [
            PortalKind::Archive,
            PortalKind::Search,
            PortalKind::Git,
            PortalKind::Backup
        ]
    );
    assert_eq!(items[0].what, "C:\\a.zip");
    assert_eq!(items[1].what, "invoice · C:\\Docs");
    assert_eq!(items[2].what, "01234567 · C:\\repo");
    assert_eq!(items[3].what, "fedcba98");
    assert!(items.iter().all(|k| k.path == "C:\\m" && k.id == "p"));
}

// ---- Workspaces ---------------------------------------------------------------------------------------

mod workspace_tests {
    use super::*;
    use crate::components::sidebar::workspaces;
    use crate::explorer::TabId;
    use glint_model::store::ViewMode;

    /// Every pane's tabs, as (folder, view) pairs.
    fn layout(w: &mut World) -> Vec<Vec<(String, ViewMode)>> {
        w.explorer.read_with(&w.cx, |e, _| {
            e.panes()
                .iter()
                .map(|p| {
                    p.tabs
                        .iter()
                        .filter_map(|id| e.tab(*id).map(|t| (t.path.clone(), t.view)))
                        .collect()
                })
                .collect()
        })
    }

    fn act(w: &mut World, f: impl FnOnce(&mut Explorer, &mut Context<Explorer>)) {
        w.explorer.update(&mut w.cx, f);
        w.settle(|w| !w.tab_loading());
    }

    fn active_tab(w: &mut World) -> TabId {
        w.explorer.read_with(&w.cx, |e, _| e.active_tab_id())
    }

    #[gpui_kit::test]
    fn a_saved_workspace_puts_the_tabs_views_split_and_scroll_back_without_reading_open_folders(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let mut w = world(cx, "ws-switch");
        // "Work": Home and Documents (as tiles) on the left, This drive on the right, split at 30 %, the right
        // list scrolled a little.
        act(&mut w, |e, cx| e.new_tab(Some(DOCS), cx));
        w.settle(|w| w.path() == DOCS && !w.tab_loading());
        act(&mut w, |e, cx| e.set_view(ViewMode::Grid, None, cx));
        act(&mut w, |e, cx| e.toggle_dual(cx));
        act(&mut w, |e, cx| {
            e.navigate("C:\\".to_string(), NavOptions::default(), cx);
        });
        w.settle(|w| w.path() == "C:\\" && !w.tab_loading());
        let right = active_tab(&mut w);
        let key = w.explorer.read_with(&w.cx, |e, _| e.view_key(None));
        act(&mut w, |e, _| e.save_scroll(right, &key, 33.));
        w.cx.update(|_, cx| state::update(cx, |s| s.dual_split = Some(0.3)));
        let before = layout(&mut w);
        assert_eq!(before.len(), 2);

        let id = w.cx.update(|_, cx| workspaces::save_as("  Work ", cx)).expect("saved");
        let listed = w.cx.update(|_, cx| workspaces::listed(cx));
        assert_eq!(
            listed,
            [workspaces::Listed {
                id: id.clone(),
                name: "Work".into(),
                current: true
            }]
        );

        // Then the window goes elsewhere: one pane, the right folder shown in the Documents tab, no split.
        act(&mut w, |e, cx| e.close_pane(1, cx));
        act(&mut w, |e, cx| {
            e.navigate("C:\\".to_string(), NavOptions::default(), cx);
        });
        w.settle(|w| w.path() == "C:\\" && !w.tab_loading());
        w.cx.update(|_, cx| state::update(cx, |s| s.dual_split = None));
        assert_ne!(layout(&mut w), before);
        assert!(!w.cx.update(|_, cx| workspaces::listed(cx))[0].current);

        // Switching puts it all back, and reads only the folder no tab had open.
        let reads = w.disk.listed().len();
        assert!(w.cx.update(|_, cx| workspaces::switch_to(&id, cx)));
        w.settle(|w| !w.tab_loading());
        w.cx.run_until_parked();
        assert_eq!(layout(&mut w), before);
        let (dual, pane, split) = w.cx.update(|_, cx| {
            let e = explorer(cx).read(cx);
            (e.is_dual(), e.active_pane(), state::settings(cx).dual_split)
        });
        assert!(dual && pane == 1 && split == Some(0.3), "{dual} {pane} {split:?}");
        let again: Vec<String> = w.disk.listed()[reads..].iter().map(|p| p.to_lowercase()).collect();
        assert_eq!(again, [DOCS.to_lowercase()], "only Documents was read again");
        // The right list is where it was left, and the layout is the one saved.
        let right = active_tab(&mut w);
        assert_eq!(w.explorer.read_with(&w.cx, |e, _| e.saved_scroll(right, &key)), 33.);
        assert!(w.cx.update(|_, cx| workspaces::listed(cx))[0].current);
        // Switching to what is already there changes no listing.
        let reads = w.disk.listed().len();
        assert!(w.cx.update(|_, cx| workspaces::switch_to(&id, cx)));
        w.settle(|w| !w.tab_loading());
        assert_eq!(w.disk.listed().len(), reads, "nothing was read again");
        assert_eq!(layout(&mut w), before);
    }

    #[gpui_kit::test]
    fn workspaces_are_saved_again_under_a_name_renamed_and_deleted(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut w = world(cx, "ws-rename");
        let work = w.cx.update(|_, cx| workspaces::save_as("Work", cx)).unwrap();
        act(&mut w, |e, cx| e.new_tab(Some(DOCS), cx));
        let photos = w.cx.update(|_, cx| workspaces::save_as("تقارير", cx)).unwrap();
        assert_ne!(work, photos);
        // The same name (in any case) keeps the workspace and takes the new layout.
        let again = w.cx.update(|_, cx| workspaces::save_as("WORK", cx)).unwrap();
        assert_eq!(again, work);
        let names = |w: &mut World| {
            w.cx.update(|_, cx| workspaces::listed(cx))
                .into_iter()
                .map(|l| l.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&mut w), ["WORK", "تقارير"]);

        assert!(w.cx.update(|_, cx| workspaces::rename(&photos, "Pictures", cx)));
        assert!(!w.cx.update(|_, cx| workspaces::rename(&photos, "work", cx)), "taken");
        assert!(!w.cx.update(|_, cx| workspaces::rename(&photos, "   ", cx)), "empty");
        assert_eq!(names(&mut w), ["WORK", "Pictures"]);
        assert!(w.cx.update(|_, cx| workspaces::delete(&work, cx)));
        assert!(!w.cx.update(|_, cx| workspaces::delete(&work, cx)));
        assert_eq!(names(&mut w), ["Pictures"]);
        // A workspace that is gone can't be switched to, and the window stays as it is.
        let shown = layout(&mut w);
        assert!(!w.cx.update(|_, cx| workspaces::switch_to(&work, cx)));
        assert_eq!(layout(&mut w), shown);
        // They are in the saved state, so they come back with the next start.
        let saved = w.cx.update(|_, cx| persistence::app(cx).map(|a| a.workspaces.len()));
        assert_eq!(saved, Some(1));
    }

    #[gpui_kit::test]
    fn the_workspaces_button_lists_them_and_the_name_box_saves_renames_and_refuses_a_name_in_use(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let mut w = world(cx, "ws-menu");
        assert_eq!(w.role_of("Workspaces"), Some(Role::Button));

        // Nothing saved: the menu says so and offers to save.
        w.click("Workspaces");
        w.frame();
        assert!(w.has("Save current layout as…"));
        assert!(!w.has("Rename workspace"), "nothing to rename yet");
        w.press("escape");

        // Save as…: the box takes a name; Enter saves.
        w.cx.update(workspaces::ask_save_as);
        w.frame();
        assert!(w.has("Workspace name"));
        w.cx.update(|window, cx| window.input("  Work  day ", cx));
        w.cx.run_until_parked();
        w.press("enter");
        w.settle(|w| w.cx.update(|_, cx| workspaces::listed(cx)).len() == 1);
        assert_eq!(w.cx.update(|_, cx| workspaces::listed(cx))[0].name, "Work day");
        assert!(!w.has("Workspace name"), "the box closed");

        // The menu has it now, checked, with Rename and Delete.
        w.click("Workspaces");
        w.frame();
        assert!(w.has("Work day") && w.has("Rename workspace") && w.has("Delete workspace"));
        w.press("escape");

        // Rename: a name another workspace has is refused (Enter does nothing), another name works.
        let other = w.cx.update(|_, cx| workspaces::save_as("Other", cx)).unwrap();
        w.cx.update(|window, cx| workspaces::ask_rename(window, cx, other.clone()));
        w.frame();
        w.press("ctrl-a");
        w.cx.update(|window, cx| window.input("work DAY", cx));
        w.cx.run_until_parked();
        w.frame();
        assert!(w.has("Another workspace has this name"));
        w.press("enter");
        assert!(w.has("Workspace name"), "still asking");
        w.press("ctrl-a");
        w.cx.update(|window, cx| window.input("Later", cx));
        w.cx.run_until_parked();
        w.press("enter");
        w.settle(|w| {
            w.cx.update(|_, cx| workspaces::listed(cx))
                .iter()
                .any(|l| l.name == "Later")
        });
    }

    #[test]
    fn the_palette_offers_save_and_one_entry_for_each_workspace_with_rename_and_delete_when_typing() {
        use crate::components::palette::commands::{self, Inputs, Run, WorkspaceDo};
        let none = commands::actions(&Inputs::default());
        let save = none
            .iter()
            .find(|c| c.id == "workspace-save")
            .expect("Save workspace as…");
        assert_eq!(save.label.as_ref(), "Save workspace as…");
        assert!(matches!(&save.run, Run::Workspace(WorkspaceDo::SaveAs)));
        assert!(none.iter().all(|c| !c.id.starts_with("workspace-ws")));

        let inputs = Inputs {
            workspaces: vec![
                ("ws-1".into(), "Work".into(), true),
                ("ws-2".into(), "تقارير".into(), false),
            ],
            ..Default::default()
        };
        let all = commands::actions(&inputs);
        let find = |id: &str| all.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id}"));
        let work = find("workspace-ws-1");
        assert_eq!(work.label.as_ref(), "Workspace: Work");
        assert!(work.detail.is_some() && !work.search_only, "the one on screen says so");
        assert!(matches!(&work.run, Run::Workspace(WorkspaceDo::Switch(id)) if id == "ws-1"));
        assert!(find("workspace-ws-2").detail.is_none());
        assert_eq!(find("workspace-ws-2").label.as_ref(), "Workspace: تقارير");
        for (id, what) in [("workspace-rename-ws-2", "Rename"), ("workspace-delete-ws-2", "Delete")] {
            let command = find(id);
            assert!(command.search_only, "{id} is listed once something is typed");
            assert_eq!(command.label.as_ref(), format!("{what} workspace: تقارير"));
        }
        assert!(matches!(&find("workspace-delete-ws-1").run, Run::Workspace(WorkspaceDo::Delete(id)) if id == "ws-1"));
    }
}

#[gpui_kit::test]
fn the_rail_fades_at_its_bottom_edge_only_while_more_is_below(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "edge-fade");
    // The test window is tall: everything fits, nothing fades.
    w.frame();
    w.frame();
    assert!(!w.sidebar.read_with(&w.cx, |s, _| s.fade_below), "nothing is below");
    w.cx.simulate_resize(size(px(300.), px(180.)));
    w.frame();
    w.frame();
    assert!(
        w.sidebar.read_with(&w.cx, |s, _| s.fade_below),
        "rows run on below the window"
    );
    w.cx.simulate_resize(size(px(300.), px(900.)));
    w.frame();
    w.frame();
    assert!(
        !w.sidebar.read_with(&w.cx, |s, _| s.fade_below),
        "and it goes when they fit"
    );
}

#[gpui_kit::test]
fn scrolling_the_rail_moves_its_fade_and_never_panics(cx: &mut TestAppContext) {
    use gpui_kit::InputEvent as _;
    let _guard = testing::lock();
    let mut w = world(cx, "edge-fade-scroll");
    w.cx.simulate_resize(size(px(300.), px(180.)));
    w.frame();
    w.frame();
    assert!(
        w.sidebar.read_with(&w.cx, |s, _| s.fade_below),
        "rows run on below the window"
    );
    // 0.16.0 read the list in its scroll handler while the list still held itself: the first scroll over the
    // rail panicked inside the window procedure and Windows closed Glint. A touchpad's pixels to the bottom,
    // then a wheel's notch back up (it glides).
    let steps = [
        (gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-2000.))), false),
        (gpui_kit::ScrollDelta::Lines(point(0., 3.)), true),
    ];
    for (delta, below) in steps {
        let at = w.bounds_of("places").center();
        w.cx.update(|window, cx| {
            let event = gpui_kit::ScrollEvent {
                position: at,
                delta,
                ..Default::default()
            };
            window.dispatch_event(event.to_platform_input(), cx);
        });
        w.cx.run_until_parked();
        w.frame();
        assert_eq!(
            w.sidebar.read_with(&w.cx, |s, _| s.fade_below),
            below,
            "after {delta:?}"
        );
    }
}

#[gpui_kit::test]
fn a_drive_without_a_known_size_keeps_the_bars_place_so_rows_do_not_jump(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "drive-bar-place");
    let names = w.names();
    // C: has a size, E: (a USB drive) doesn't know its own this moment.
    let (c, e) = (
        names.iter().find(|n| n.contains("C:")).expect("C:").clone(),
        names.iter().find(|n| n.contains("E:")).expect("E:").clone(),
    );
    let (c_id, e_id) = (w.id_of(&c), w.id_of(&e));
    let (c_box, e_box) = (w.bounds_of(c_id), w.bounds_of(e_id));
    assert_eq!(
        c_box.size.height, e_box.size.height,
        "the same row height with and without a size"
    );
}
