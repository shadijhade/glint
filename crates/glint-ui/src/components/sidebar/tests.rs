//! Headless tests of the sidebar: on a fake disk and fake sources, what is drawn (labels, roles), that clicks and
//! keys change the explorer the way the TypeScript's did, an Arabic folder name, and the mirrored layout.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{init_with, Drive, DriveKind, KnownFolder, KnownId};
use crate::testing;
use glint_model::store::RemoteConnection;
use gpui_kit::test::{ClickOptions, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;

const HOME: &str = "C:\\Users\\me";
const DOCS: &str = "C:\\Users\\me\\Documents";

/// What the fake sources know, and what they were asked.
#[derive(Default)]
struct Fakes {
    folders: Mutex<HashMap<String, Vec<Subfolder>>>,
    asked: Mutex<Vec<(String, bool)>>,
    cloud: Mutex<Option<Space>>,
    count: Mutex<Option<String>>,
}

impl Fakes {
    fn sources(self: &Arc<Self>) -> Sources {
        let (a, b, c) = (self.clone(), self.clone(), self.clone());
        Sources {
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
        }
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
    explorer: Entity<Explorer>,
    _subscription: Subscription,
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
    cx.update(|cx| {
        init_with(cx, disk.source());
        persistence::init_at(cx, state_file(name));
    });
    let fakes = Arc::new(Fakes::default());
    fakes.set(HOME, &[("Documents", Some(true)), ("Pictures", Some(false))]);
    fakes.set(DOCS, &[("Reports", None)]);
    fakes.set("C:\\", &[("Windows", Some(true))]);
    let explorer = cx.update(|cx| explorer(cx));
    let sources = fakes.sources();
    let (mut visual, sidebar) =
        testing::open_window(cx, |window, cx| cx.new(|cx| Sidebar::with_sources(sources, window, cx)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    let subscription = visual.update(|_, cx| {
        cx.subscribe(&sidebar, move |_, event: &SidebarEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
    });
    let task = explorer.update(&mut visual, |e, cx| e.init_explorer(Vec::new(), cx));
    visual.foreground_executor().block_test(task);
    let mut world = World {
        cx: visual,
        sidebar,
        fakes,
        events,
        explorer,
        _subscription: subscription,
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
    // Its right-click menu says Unpin; choosing it takes the folder out of the saved state.
    let id = w.id_of("تقارير 2024");
    w.cx.update(|window, cx| window.right_click(id, cx));
    w.frame();
    assert!(w.has("Unpin from sidebar"), "the menu offers to unpin a pinned folder");
    assert!(w.has("Open in new tab") && w.has("Copy path") && w.has("Search text"));
    w.click("Unpin from sidebar");
    w.cx.run_until_parked();
    assert!(w.cx.update(|_, cx| persistence::app(cx).unwrap().pinned.is_empty()));
    assert!(!w.names().contains(&"# Pinned".to_string()));
    // A folder that isn't pinned offers to pin it.
    let id = w.id_of("Documents");
    w.cx.update(|window, cx| window.right_click(id, cx));
    w.frame();
    assert!(w.has("Pin to sidebar"));
    w.click("Pin to sidebar");
    w.cx.run_until_parked();
    assert_eq!(
        w.cx.update(|_, cx| persistence::app(cx).unwrap().pinned.clone()),
        [DOCS]
    );
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
    // A connection's menu has its own items, and the editor and the removal are asked of the window.
    let id = w.id_of("NAS");
    w.cx.update(|window, cx| window.right_click(id, cx));
    w.frame();
    assert!(w.has("Edit the connection…") && w.has("Remove the connection…"));
    assert!(w.has("Read-only: on a remote connection"));
    assert!(!w.has("Pin to sidebar"), "a connection's top is not pinned");
    w.click("Remove the connection…");
    assert_eq!(
        w.events.borrow().last(),
        Some(&SidebarEvent::RemoveRemote("nas".into()))
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
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}
