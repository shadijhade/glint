//! Headless tests of the grid: what it draws on a fake disk (labels and roles), that clicks, keys and the wheel
//! change the explorer as the TypeScript did, thumbnails asked for and cancelled, an Arabic name, and the Arabic
//! interface mirroring.

use super::thumbnail::tests::{answer, Fake};
use super::thumbnail::{self, ThumbSource, Thumbnails};
use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{FileOp, NavOptions, ViewMode};
use crate::platform::dragdrop::{DropAnswers, DropEvent};
use crate::testing;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{InputEvent as _, TestAppContext, VisualTestContext};
use std::cell::RefCell;

/// A folder `C:\x` with a folder, pictures, an Arabic name and `more` plain files.
fn disk(more: usize) -> FakeDisk {
    let disk = FakeDisk::new();
    let mut entries = vec![
        folder("C:\\x", "docs"),
        file("C:\\x", "a.jpg", 2_000, 1),
        file("C:\\x", "b.txt", 10, 2),
        file("C:\\x", "تقرير الربع.pdf", 5_000, 3),
    ];
    for n in 0..more {
        entries.push(file("C:\\x", &format!("f{n:03}.dat"), 1, 4));
    }
    disk.folder("C:\\x", entries);
    disk.folder("C:\\x\\docs", vec![file("C:\\x\\docs", "inner.txt", 1, 1)]);
    disk.folder("C:\\empty", vec![]);
    disk
}

struct Env {
    visual: VisualTestContext,
    grid: Entity<FileGrid>,
    explorer: Entity<Explorer>,
    service: Entity<Thumbnails>,
}

fn env(cx: &mut TestAppContext, disk: &FakeDisk, thumbs: Rc<Fake>) -> Env {
    env_with(cx, disk, thumbs, None)
}

/// [`env`] on a fake Git, if there is one.
fn env_with(
    cx: &mut TestAppContext,
    disk: &FakeDisk,
    thumbs: Rc<Fake>,
    git: Option<&crate::components::file_list::fake_git::FakeGit>,
) -> Env {
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    if let Some(git) = git {
        cx.update(|cx| git.install(cx));
    }
    let source: Rc<dyn ThumbSource> = Rc::new(thumbs);
    let service = cx.update(|cx| thumbnail::init_with(cx, source));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\x", NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    explorer.update(cx, |e, cx| e.set_view(ViewMode::Grid, None, cx));
    let (visual, grid) = testing::open_window(cx, |window, cx| cx.new(|cx| FileGrid::new(0, window, cx)));
    Env {
        visual,
        grid,
        explorer,
        service,
    }
}

impl Env {
    fn frame(&mut self) {
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    fn click(&mut self, tile: usize) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("file-grid-tile", tile), cx);
        });
    }

    fn press(&mut self, keys: &str) {
        self.visual.update(|window, cx| window.press(keys, cx));
    }

    fn go(&mut self, path: &str) {
        let path = path.to_string();
        let target = path.clone();
        self.explorer
            .update(&mut self.visual, |e, cx| e.navigate(target, NavOptions::default(), cx));
        // The listing arrives through the executors; wait for the folder to be the one shown.
        self.settle(|env| {
            env.path() == path
                && env
                    .explorer
                    .read_with(&env.visual, |e, _| !e.pane_tab(0).is_some_and(|t| t.loading))
        });
    }

    fn selected(&self) -> Vec<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.pane_tab(0)
                .map(|t| {
                    t.selection
                        .iter()
                        .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    fn focus(&self) -> Option<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.pane_tab(0).and_then(|t| {
                t.selection
                    .focus()
                    .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
            })
        })
    }

    fn path(&self) -> String {
        self.explorer
            .read_with(&self.visual, |e, _| e.current_path().to_string())
    }

    fn tile_names(&mut self) -> Vec<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(Role::ListBoxOption)
                .iter()
                .filter_map(|tile| tile.label().map(str::to_string))
                .collect()
        })
    }

    fn settle(&mut self, done: impl Fn(&mut Env) -> bool) {
        for _ in 0..400 {
            self.visual.run_until_parked();
            self.frame();
            if done(self) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("it never settled");
    }

    fn wheel(&mut self, lines: f32, modifiers: Modifiers) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            let at = window.find("file-grid-body").bounds().center();
            window.dispatch_event(
                ScrollEvent {
                    position: at,
                    delta: ScrollDelta::Lines(point(0., lines)),
                    modifiers,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        });
    }

    fn tile_size(&self) -> glint_model::grid::TileSize {
        self.visual.read(|cx| state::settings(cx).tile_size)
    }

    fn act(&mut self, action: impl Action) {
        self.visual
            .update(|window, cx| window.dispatch_action(action.boxed_clone(), cx));
    }
}

mod marks;

fn plain() -> Rc<Fake> {
    Rc::new(Fake::default())
}

#[gpui_kit::test]
fn tiles_are_options_of_a_named_list_with_the_files_names(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    // Folders first, then the names in natural order; the Arabic name exactly as it is, without the isolates.
    assert_eq!(env.tile_names(), ["docs", "a.jpg", "b.txt", "تقرير الربع.pdf"]);
    env.visual.update(|window, _| {
        assert_eq!(
            window.find_by_label("Files").role(),
            Some(Role::ListBox),
            "the list has a name"
        );
        // Each option says how big it is and where it sits, for a screen reader.
        let docs = window.find_by_label("docs");
        assert_eq!(docs.role(), Some(Role::ListBoxOption));
        assert_eq!(docs.selected(), Some(false));
        assert_eq!(window.find_by_label("a.jpg").description(), Some("2.0 KB"));
    });
}

#[gpui_kit::test]
fn the_header_names_the_sorts_and_a_click_sorts_by_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.frame();
    env.visual.update(|window, _| {
        for name in ["Name", "Date modified", "Date taken", "Type", "Size"] {
            assert_eq!(window.find_by_label(name).role(), Some(Role::Button), "{name}");
        }
        assert_eq!(window.find_by_label("Sort by").role(), Some(Role::Toolbar));
    });
    // Size sorts largest first; a second click flips it.
    env.visual
        .update(|window, cx| window.click(("file-grid-sort", 4_usize), cx));
    let sort = |env: &Env| {
        env.explorer
            .read_with(&env.visual, |e, _| e.pane_tab(0).map(|t| t.sort))
    };
    assert_eq!(
        sort(&env),
        Some(Sort {
            key: SortKey::Size,
            desc: true
        })
    );
    assert_eq!(env.tile_names()[..2], ["docs", "تقرير الربع.pdf"]);
    env.visual
        .update(|window, cx| window.click(("file-grid-sort", 4_usize), cx));
    assert_eq!(sort(&env).map(|s| s.desc), Some(false));
}

#[gpui_kit::test]
fn click_ctrl_click_and_shift_click_select_as_the_list_does(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.click(1);
    assert_eq!(env.selected(), ["a.jpg"]);
    assert_eq!(env.focus().as_deref(), Some("a.jpg"));
    env.visual.update(|window, cx| {
        window.click_with_modifiers(("file-grid-tile", 3_usize), Modifiers::control(), cx);
    });
    assert_eq!(env.selected(), ["a.jpg", "تقرير الربع.pdf"]);
    // Ctrl-click on a selected one takes it out.
    env.visual.update(|window, cx| {
        window.click_with_modifiers(("file-grid-tile", 1_usize), Modifiers::control(), cx);
    });
    assert_eq!(env.selected(), ["تقرير الربع.pdf"]);
    // Shift-click selects from the anchor (the last clicked) to the one clicked.
    env.click(0);
    env.visual.update(|window, cx| {
        window.click_with_modifiers(("file-grid-tile", 2_usize), Modifiers::shift(), cx);
    });
    assert_eq!(env.selected(), ["docs", "a.jpg", "b.txt"]);
    // The selection is drawn: the option says so.
    env.frame();
    env.visual
        .update(|window, _| assert_eq!(window.find_by_label("a.jpg").selected(), Some(true)));
    // A click on the empty part clears it.
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let body = window.find("file-grid-body").bounds();
        let at = point(body.center().x, body.bottom() - px(8.));
        window.pointer_down(at, MouseButton::Left, cx);
        window.pointer_up(at, MouseButton::Left, cx);
    });
    assert!(env.selected().is_empty());
}

#[gpui_kit::test]
fn a_double_click_and_enter_open_a_folder_and_middle_click_opens_it_in_a_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.visual
        .update(|window, cx| window.double_click(("file-grid-tile", 0_usize), cx));
    env.visual.run_until_parked();
    assert_eq!(env.path(), "C:\\x\\docs");
    assert_eq!(env.tile_names(), ["inner.txt"]);

    // Back in C:\x, Enter on the selected folder opens it too (the OpenItem action of the FileList context).
    env.go("C:\\x");
    env.click(0);
    env.press("enter");
    env.visual.run_until_parked();
    assert_eq!(env.path(), "C:\\x\\docs");

    // Middle-click: a folder in a new tab.
    env.go("C:\\x");
    let tabs_before = env.explorer.read_with(&env.visual, |e, _| e.panes()[0].tabs.len());
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let tile = window.find(("file-grid-tile", 0_usize)).bounds();
        window.pointer_down(tile.center(), MouseButton::Middle, cx);
    });
    env.visual.run_until_parked();
    let tabs_after = env.explorer.read_with(&env.visual, |e, _| e.panes()[0].tabs.len());
    assert_eq!(tabs_after, tabs_before + 1);
}

#[gpui_kit::test]
fn arrows_move_by_tile_and_by_row_and_home_end_go_to_the_ends(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    // 4 + 20 = 24 items in a few rows of however many columns the 1000 px window holds.
    let mut env = env(cx, &disk(20), plain());
    let cols = env.grid.read_with(&env.visual, |grid, _| grid.layout_now().cols);
    assert!((2..=20).contains(&cols), "{cols} columns");
    let names: Vec<String> = env.tile_names();
    env.click(1);
    env.press("right");
    assert_eq!(env.focus().as_deref(), Some("b.txt"));
    env.press("left");
    assert_eq!(env.focus().as_deref(), Some("a.jpg"));
    // Down is a whole row of tiles: tile 1 to tile 1 + columns.
    env.press("down");
    assert_eq!(env.focus().as_deref(), Some(names[1 + cols].as_str()));
    env.press("up");
    assert_eq!(env.focus().as_deref(), Some("a.jpg"));
    env.press("end");
    // The Arabic name sorts after the Latin ones.
    assert_eq!(env.focus().as_deref(), Some("تقرير الربع.pdf"));
    env.press("home");
    assert_eq!(env.focus().as_deref(), Some("docs"));
    // Shift extends from the anchor.
    env.press("shift-right");
    env.press("shift-right");
    assert_eq!(env.selected(), ["docs", "a.jpg", "b.txt"]);
    // Ctrl+A selects all (the SelectAll action).
    env.press("ctrl-a");
    assert_eq!(env.selected().len(), 24);
}

#[gpui_kit::test]
fn typing_jumps_to_a_name_and_escape_clears_the_selection(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.click(0);
    env.press("b");
    assert_eq!(env.selected(), ["b.txt"]);
    // Within a second, more letters make a word ("bx" matches nothing: the selection stays).
    env.press("x");
    assert_eq!(env.selected(), ["b.txt"]);
    // Ctrl+Space toggles the focused one out.
    env.press("ctrl-space");
    assert!(env.selected().is_empty());
    env.click(2);
    env.press("escape");
    assert!(env.selected().is_empty(), "Escape clears the selection");
}

#[gpui_kit::test]
fn zoom_actions_and_ctrl_wheel_change_the_tile_size(cx: &mut TestAppContext) {
    use glint_model::grid::TileSize;
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    assert_eq!(env.tile_size(), TileSize::Medium);
    env.click(0);
    env.act(GridLarger);
    assert_eq!(env.tile_size(), TileSize::Large);
    env.act(GridSmaller);
    env.act(GridSmaller);
    assert_eq!(env.tile_size(), TileSize::Small);
    env.act(GridReset);
    assert_eq!(env.tile_size(), TileSize::Medium);

    // Ctrl+wheel: one notch up is a step bigger; a notch down is a step smaller; without Ctrl it scrolls instead.
    env.wheel(3., Modifiers::control());
    assert_eq!(env.tile_size(), TileSize::Large);
    env.wheel(-3., Modifiers::control());
    assert_eq!(env.tile_size(), TileSize::Medium);
    env.wheel(3., Modifiers::default());
    assert_eq!(env.tile_size(), TileSize::Medium);
    // The tiles really are bigger: a Large tile is wider than a Medium one.
    let width = |env: &mut Env| {
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find(("file-grid-tile", 0_usize)).bounds().size.width)
        })
    };
    let medium = width(&mut env);
    env.act(GridLarger);
    assert!(width(&mut env) > medium);
}

#[gpui_kit::test]
fn only_the_visible_rows_are_drawn_and_scrolling_brings_the_rest(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(600), plain());
    let shown = env.tile_names();
    assert!(!shown.is_empty() && shown.len() < 604, "{} tiles drawn", shown.len());
    assert_eq!(shown[0], "docs");
    // Keyboard focus first (the wheel doesn't move it), then scroll with the wheel.
    env.click(1);
    env.wheel(-30., Modifiers::default());
    let scrolled = env.tile_names();
    assert_ne!(scrolled[0], "docs", "the first row scrolled away");
    // Keyboard End scrolls to the focused last tile.
    env.press("end");
    let end = env.tile_names();
    assert!(end.iter().any(|name| name == "f599.dat"), "{end:?}");
}

#[gpui_kit::test]
fn thumbnails_are_asked_for_the_visible_rows_once_not_again_every_frame(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = plain();
    let mut env = env(cx, &disk(600), fake.clone());
    env.settle(|env| env.service.read_with(&env.visual, |t, _| t.pending_count() == 0));
    let at_top = fake.asked.borrow().len();
    // Visible rows and a couple more each side, not the 600 files.
    assert!(at_top > 0 && at_top < 150, "{at_top} thumbnails asked for at the top");
    for _ in 0..10 {
        env.frame();
    }
    env.visual.run_until_parked();
    assert_eq!(fake.asked.borrow().len(), at_top, "drawing again asks for nothing more");
    // Scrolled to the end, the rows there are asked for, and the ones left are cancelled.
    env.click(1);
    env.press("end");
    env.settle(|env| env.service.read_with(&env.visual, |t, _| t.pending_count() == 0));
    let at_end = fake.asked.borrow().len();
    assert!(at_end > at_top && at_end < 300, "{at_end} asked for in all");
    for _ in 0..10 {
        env.frame();
    }
    assert_eq!(fake.asked.borrow().len(), at_end);
}

#[gpui_kit::test]
fn thumbnails_are_asked_for_files_only_at_the_tile_size_and_drawn_when_they_arrive(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = plain();
    fake.answers
        .borrow_mut()
        .insert("C:\\x\\a.jpg".into(), answer(64, 48, [7, 7, 7, 255], true, 1));
    let mut env = env(cx, &disk(0), fake.clone());
    // The test window has its own scale (device pixels to one): sizes are asked for in device pixels.
    let scale = env.visual.update(|window, _| window.scale_factor());
    let step = |css: f32| glint_model::grid::thumb_step(css, scale);
    env.settle(|env| env.service.read_with(&env.visual, |t, _| t.held_count() >= 1));
    let asked: Vec<_> = fake
        .asked
        .borrow()
        .iter()
        .map(|a| (a.path.clone(), a.size, a.lane.clone()))
        .collect();
    // Medium tiles are 6 rem = 96 px; the folder isn't asked for; the tab is the lane.
    assert!(
        asked.contains(&("C:\\x\\a.jpg".to_string(), step(96.0), "tab1".to_string())),
        "{asked:?}"
    );
    assert!(asked.iter().all(|(path, _, _)| !path.ends_with("docs")), "{asked:?}");
    // The picture is the file's look now; the folder keeps its icon.
    let (a, docs) = (file("C:\\x", "a.jpg", 2_000, 1), folder("C:\\x", "docs"));
    env.service.read_with(&env.visual, |t, _| {
        assert!(matches!(t.look(&a, 96.0, scale), thumbnail::Look::Picture(_)));
        assert!(matches!(t.look(&docs, 96.0, scale), thumbnail::Look::Icon));
    });

    // Zoomed to Large (128 px), the new size is asked for, and the 96 px picture stands in meanwhile.
    env.click(0);
    env.act(GridLarger);
    env.frame();
    assert!(fake
        .asked
        .borrow()
        .iter()
        .any(|a| a.path.ends_with("a.jpg") && a.size == step(128.0)));
    env.service.read_with(&env.visual, |t, _| {
        assert!(matches!(t.look(&a, 128.0, scale), thumbnail::Look::StandIn(_)));
    });

    // Another folder: everything the tab waited for is cancelled.
    let cancels = fake.cancelled.borrow().len();
    env.go("C:\\x\\docs");
    env.frame();
    assert!(
        fake.cancelled.borrow()[cancels..]
            .iter()
            .any(|(lane, paths, _)| lane == "tab1" && paths.is_none()),
        "{:?}",
        fake.cancelled.borrow()
    );
}

#[gpui_kit::test]
fn empty_folders_say_so_and_a_filter_that_matches_nothing_says_that(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.go("C:\\empty");
    env.visual.update(|window, _| {
        assert_eq!(window.find_by_label("This folder is empty").role(), Some(Role::Status));
    });
    env.go("C:\\x");
    env.explorer
        .update(&mut env.visual, |e, cx| e.set_filter("zzz", None, cx));
    env.frame();
    env.visual.update(|window, _| {
        assert!(window
            .try_find_by_label("Nothing in this folder matches “zzz”.")
            .is_some());
    });
}

#[gpui_kit::test]
fn right_click_and_the_menu_key_ask_for_the_menu_and_a_drag_starts_after_six_pixels(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    env.visual.update(|_, cx| {
        cx.subscribe(&env.grid, move |_, event: &FileGridEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    // Right-click on an unselected tile selects it, and asks for its menu.
    env.visual
        .update(|window, cx| window.right_click(("file-grid-tile", 2_usize), cx));
    assert_eq!(env.selected(), ["b.txt"]);
    assert!(matches!(
        events.borrow().first(),
        Some(FileGridEvent::ContextMenu { path, .. }) if path.ends_with("b.txt")
    ));
    // The menu key asks for the focused tile's.
    env.press("shift-f10");
    assert_eq!(events.borrow().len(), 2);
    // A press on a tile that moves 6 px with the button down is a drag of the selection.
    env.click(1);
    events.borrow_mut().clear();
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let from = window.find(("file-grid-tile", 1_usize)).bounds().center();
        window.pointer_down(from, MouseButton::Left, cx);
        window.pointer_move(from + point(px(2.), px(0.)), Some(MouseButton::Left), cx);
        assert!(events.borrow().is_empty(), "not yet");
        window.pointer_move(from + point(px(12.), px(0.)), Some(MouseButton::Left), cx);
        window.pointer_up(from + point(px(12.), px(0.)), MouseButton::Left, cx);
    });
    let dragged = events.borrow().clone();
    assert!(
        matches!(&dragged[..], [FileGridEvent::DragStart { paths, from_archive: false }] if paths.len() == 1 && paths[0].ends_with("a.jpg")),
        "{dragged:?}"
    );
}

#[gpui_kit::test]
fn pressing_an_item_of_a_multi_selection_keeps_it_until_release(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.click(1);
    env.visual.update(|window, cx| {
        window.click_with_modifiers(("file-grid-tile", 2_usize), Modifiers::control(), cx);
    });
    assert_eq!(env.selected(), ["a.jpg", "b.txt"]);
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let at = window.find(("file-grid-tile", 1_usize)).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
    });
    assert_eq!(
        env.selected(),
        ["a.jpg", "b.txt"],
        "kept while pressed, so all of it can be dragged"
    );
    env.visual.update(|window, cx| {
        let at = window.find(("file-grid-tile", 1_usize)).bounds().center();
        window.pointer_up(at, MouseButton::Left, cx);
    });
    assert_eq!(env.selected(), ["a.jpg"], "released without dragging: just that one");
}

#[gpui_kit::test]
fn the_arabic_interface_mirrors_the_tiles_and_the_arrow_keys(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.frame();
    let x = |env: &mut Env, tile: usize| {
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            f32::from(window.find(("file-grid-tile", tile)).bounds().origin.x)
        })
    };
    let (first, second) = (x(&mut env, 0), x(&mut env, 1));
    assert!(first < second, "English: tiles run left to right");
    testing::set_rtl(true);
    env.explorer.update(&mut env.visual, |_, cx| cx.notify());
    let (first_rtl, second_rtl) = (x(&mut env, 0), x(&mut env, 1));
    assert!(first_rtl > second_rtl, "Arabic: the first tile is at the right");
    // The Arabic name is there under its own name.
    env.visual.update(|window, _| {
        assert_eq!(
            window.find_by_label("تقرير الربع.pdf").role(),
            Some(Role::ListBoxOption)
        );
    });
    // Left is the next tile in Arabic.
    env.click(1);
    env.press("left");
    assert_eq!(env.focus().as_deref(), Some("b.txt"));
    env.press("right");
    assert_eq!(env.focus().as_deref(), Some("a.jpg"));
    testing::set_rtl(false);
}

// ---- Follow-ups: focus, marquee, drops, the menu key, SVG tiles --------------------------------------

#[gpui_kit::test]
fn ctrl_arrows_move_the_cursor_only_and_a_press_on_a_multi_selection_moves_it_at_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.click(1);
    env.press("ctrl-right");
    assert_eq!(env.selected(), ["a.jpg"], "the selection stays");
    assert_eq!(env.focus().as_deref(), Some("b.txt"), "the cursor moved");
    // Ctrl+Space then takes the item under the cursor in.
    env.press("ctrl-space");
    let mut chosen = env.selected();
    chosen.sort();
    assert_eq!(chosen, ["a.jpg", "b.txt"]);
    env.press("ctrl-left");
    env.press("ctrl-left");
    assert_eq!(env.focus().as_deref(), Some("docs"));
    assert_eq!(env.selected().len(), 2, "still the same two");

    // A press on one of two selected items keeps both but moves the focus (and the anchor) to it, before release.
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let at = window.find(("file-grid-tile", 2_usize)).bounds().center();
        window.pointer_down(at, MouseButton::Left, cx);
    });
    assert_eq!(env.selected().len(), 2);
    assert_eq!(env.focus().as_deref(), Some("b.txt"));
    let anchor = env.explorer.read_with(&env.visual, |e, _| {
        e.pane_tab(0).and_then(|t| t.selection.anchor().map(str::to_string))
    });
    assert!(anchor.is_some_and(|a| a.ends_with("b.txt")));
    env.visual.update(|window, cx| {
        let at = window.find(("file-grid-tile", 2_usize)).bounds().center();
        window.pointer_up(at, MouseButton::Left, cx);
    });
    assert_eq!(env.selected(), ["b.txt"]);
}

/// The window position of a tile's center, and of a spot in the empty part below the tiles.
fn spots(env: &mut Env, tile: usize) -> (Point<Pixels>, Point<Pixels>) {
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let body = window.find("file-grid-body").bounds();
        let center = window.find(("file-grid-tile", tile)).bounds().center();
        (center, point(center.x, body.bottom() - px(8.)))
    })
}

fn marquee_shown(env: &Env) -> bool {
    env.visual
        .read(|cx| env.grid.read(cx).marquee.as_ref().is_some_and(|marquee| marquee.shown))
}

#[gpui_kit::test]
fn a_marquee_selects_the_tiles_it_covers_and_ctrl_adds_to_what_was_selected(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    // Four tiles in a row, and the empty part below them. Dragging from below the second to the third.
    let (second, below_second) = spots(&mut env, 1);
    let (third, _) = spots(&mut env, 2);
    let (fourth, _) = spots(&mut env, 3);
    env.click(0);
    env.visual.update(|window, cx| {
        window.pointer_down(below_second, MouseButton::Left, cx);
    });
    assert!(
        env.selected().is_empty(),
        "a press on the empty part clears the selection"
    );
    // A tiny movement is no marquee.
    env.visual.update(|window, cx| {
        window.pointer_move(below_second + point(px(1.), px(-2.)), Some(MouseButton::Left), cx);
    });
    assert!(!marquee_shown(&env));
    assert!(env
        .visual
        .update(|window, _| window.try_find("file-grid-marquee").is_none()));
    env.visual.update(|window, cx| {
        window.pointer_move(third, Some(MouseButton::Left), cx);
    });
    assert!(marquee_shown(&env));
    let mut chosen = env.selected();
    chosen.sort();
    assert_eq!(chosen, ["a.jpg", "b.txt"], "the tiles under the rectangle");
    assert_eq!(
        env.focus().as_deref(),
        Some("b.txt"),
        "the last one covered has the cursor"
    );
    assert!(
        env.visual
            .update(|window, _| window.try_find("file-grid-marquee").is_some()),
        "it is drawn"
    );
    // Back to just the second tile.
    env.visual.update(|window, cx| {
        window.pointer_move(second, Some(MouseButton::Left), cx);
    });
    assert_eq!(env.selected(), ["a.jpg"]);
    env.visual.update(|window, cx| {
        window.pointer_up(second, MouseButton::Left, cx);
    });
    assert!(!marquee_shown(&env));
    assert!(
        env.visual
            .update(|window, _| window.try_find("file-grid-marquee").is_none()),
        "gone on release"
    );
    assert_eq!(env.selected(), ["a.jpg"], "the selection stays");

    // With Ctrl, what was selected stays and the marquee adds to it.
    env.click(0);
    env.visual.update(|window, cx| {
        window.change_modifiers(Modifiers::control(), cx);
        window.pointer_down(point(fourth.x, below_second.y), MouseButton::Left, cx);
        window.pointer_move(fourth + point(px(0.), px(-2.)), Some(MouseButton::Left), cx);
        window.pointer_move(fourth + point(px(3.), px(-6.)), Some(MouseButton::Left), cx);
        window.pointer_up(fourth, MouseButton::Left, cx);
        window.change_modifiers(Modifiers::default(), cx);
    });
    let mut chosen = env.selected();
    chosen.sort();
    assert_eq!(chosen, ["docs", "تقرير الربع.pdf"]);
}

#[gpui_kit::test]
fn a_marquee_held_at_the_bottom_scrolls_and_selects_what_comes_into_view(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(600), plain());
    env.frame();
    let (from, to) = env.visual.update(|window, cx| {
        window.render_frame(cx);
        let body = window.find("file-grid-body").bounds();
        // The space before the first tile of a row is no tile: a press there is on the empty part.
        let from = point(body.left() + px(2.), body.top() + px(20.));
        let to = point(body.center().x, body.bottom() - px(4.));
        (from, to)
    });
    let top = |env: &Env| env.grid.read_with(&env.visual, |grid, _| grid.scroll_top());
    env.visual.update(|window, cx| {
        window.pointer_down(from, MouseButton::Left, cx);
        window.pointer_move(to, Some(MouseButton::Left), cx);
    });
    let (before, picked) = (top(&env), env.selected().len());
    assert!(picked > 0 && picked < 604);
    // Held there, it scrolls every so often, and the rectangle covers more.
    for _ in 0..10 {
        env.visual.executor().advance_clock(Duration::from_millis(40));
        env.visual.run_until_parked();
    }
    env.frame();
    assert!(top(&env) > before, "scrolled from {before} to {}", top(&env));
    assert!(env.selected().len() > picked, "{} tiles", env.selected().len());
    // Released, it stops.
    env.visual
        .update(|window, cx| window.pointer_up(to, MouseButton::Left, cx));
    let stopped = top(&env);
    env.visual.executor().advance_clock(Duration::from_millis(200));
    env.visual.run_until_parked();
    assert_eq!(top(&env), stopped);
}

#[gpui_kit::test]
fn the_arabic_interface_counts_the_marquees_columns_from_the_right(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    testing::set_rtl(true);
    env.explorer.update(&mut env.visual, |_, cx| cx.notify());
    // In Arabic the first tile is at the right: from below the first (docs) to the second (a.jpg).
    let (first, below_first) = spots(&mut env, 0);
    let (second, _) = spots(&mut env, 1);
    assert!(first.x > second.x);
    env.visual.update(|window, cx| {
        window.pointer_down(below_first, MouseButton::Left, cx);
        window.pointer_move(second, Some(MouseButton::Left), cx);
        window.pointer_up(second, MouseButton::Left, cx);
    });
    let mut chosen = env.selected();
    chosen.sort();
    assert_eq!(chosen, ["a.jpg", "docs"]);
    testing::set_rtl(false);
}

/// Hands Glint's drop target's events to the window, as it would, and returns what the views answered.
struct Dragging {
    answers: DropAnswers,
    files: Vec<String>,
}

impl Dragging {
    fn new(files: &[&str]) -> Self {
        Dragging {
            answers: DropAnswers::default(),
            files: files.iter().map(|f| f.to_string()).collect(),
        }
    }

    /// The pointer arrives (the first call) or moves to `at`; returns what the views said a drop there does.
    fn over(&self, env: &mut Env, at: Point<Pixels>, first: bool) -> Answer {
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            if first {
                // The drag before this one has left the window.
                drops::dispatch(DropEvent::Leave, &self.answers, window, cx);
            }
            let event = if first {
                DropEvent::Enter {
                    paths: self.files.clone(),
                    x,
                    y,
                    effect: DropEffect::Copy,
                }
            } else {
                DropEvent::Over {
                    x,
                    y,
                    effect: DropEffect::Copy,
                }
            };
            drops::dispatch(event, &self.answers, window, cx);
            window.render_frame(cx);
        });
        self.answers.get()
    }

    fn drop_at(&self, env: &mut Env, at: Point<Pixels>, effect: DropEffect) {
        let (x, y) = (f32::from(at.x), f32::from(at.y));
        env.visual.update(|window, cx| {
            drops::dispatch(
                DropEvent::Drop {
                    paths: self.files.clone(),
                    x,
                    y,
                    effect,
                },
                &self.answers,
                window,
                cx,
            );
        });
        env.visual.run_until_parked();
    }
}

fn under(name: &str) -> Answer {
    Answer::Accept(Under {
        name: name.into(),
        copy: "Copy to %1".into(),
        moves: "Move to %1".into(),
        always_move: false,
    })
}

fn drop_over(env: &Env) -> Option<DropOver> {
    env.grid.read_with(&env.visual, |grid, _| grid.drop_over.clone())
}

fn drop_lit(env: &mut Env) -> bool {
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find("file-grid-drop").is_some()
    })
}

#[gpui_kit::test]
fn files_dragged_onto_a_folder_tile_go_into_it_and_onto_the_background_into_the_folder_shown(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk(0);
    let mut env = env(cx, &disk, plain());
    let outside = r"C:\other\ملف.txt";
    let files = Dragging::new(&[outside]);
    let (docs, below) = spots(&mut env, 0);
    let (a_jpg, _) = spots(&mut env, 1);

    // Over a folder tile: the drag says "into docs", and that tile lights (not the whole grid).
    assert_eq!(files.over(&mut env, docs, true), under("docs"));
    assert_eq!(drop_over(&env), Some(DropOver::Tile(r"C:\x\docs".into())));
    assert!(!drop_lit(&mut env));
    // The words are the ones File Explorer's picture will show.
    let words = env
        .grid
        .read_with(&env.visual, |grid, cx| grid.drop_words(r"C:\x\docs", cx));
    assert_eq!(
        (words.copy.as_str(), words.moves.as_str(), words.always_move),
        ("Copy to %1", "Move to %1", false)
    );
    files.drop_at(&mut env, docs, DropEffect::Copy);
    assert_eq!(
        disk.ops(),
        [FileOp::Copy {
            sources: vec![outside.into()],
            dest: r"C:\x\docs".into()
        }]
    );
    assert_eq!(drop_over(&env), None, "nothing lit after the drop");

    // Over a file tile or the empty part: the folder shown, lit as a whole. Shift moves.
    assert_eq!(files.over(&mut env, a_jpg, true), under("x"));
    assert_eq!(drop_over(&env), Some(DropOver::Pane));
    assert!(drop_lit(&mut env));
    assert_eq!(files.over(&mut env, below, false), under("x"));
    files.drop_at(&mut env, below, DropEffect::Move);
    assert_eq!(
        disk.ops()[1..],
        [FileOp::Move {
            sources: vec![outside.into()],
            dest: r"C:\x".into()
        }]
    );
}

#[gpui_kit::test]
fn a_drop_that_would_do_nothing_or_that_nothing_can_take_is_refused(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk(0);
    let mut env = env(cx, &disk, plain());
    let (docs, below) = spots(&mut env, 0);
    let (a_jpg, _) = spots(&mut env, 1);

    // Files already in the folder, dropped on its background (or on a file in it): nothing to do there.
    let here = Dragging::new(&[r"C:\x\a.jpg"]);
    assert_eq!(here.over(&mut env, below, true), Answer::Refuse);
    assert_eq!(drop_over(&env), None);
    assert!(!drop_lit(&mut env));
    assert_eq!(here.over(&mut env, a_jpg, false), Answer::Refuse);
    // But a folder tile of it takes them in.
    assert_eq!(here.over(&mut env, docs, false), under("docs"));
    // A folder dropped into itself.
    let itself = Dragging::new(&[r"C:\x\docs"]);
    assert_eq!(itself.over(&mut env, docs, true), Answer::Refuse);
    // A refused drop reaches nobody, even if the shell says copy.
    here.over(&mut env, below, false);
    here.drop_at(&mut env, below, DropEffect::None);
    assert!(disk.ops().is_empty());

    // Outside the grid nothing is answered.
    let elsewhere = Dragging::new(&[r"C:\other\a.txt"]);
    assert_eq!(elsewhere.over(&mut env, point(px(-5.), px(-5.)), true), Answer::Refuse);
    assert_eq!(drop_over(&env), None);

    // This PC is no folder; nor are search results or archives; a folder Windows keeps closed takes nothing.
    env.go("");
    assert_eq!(elsewhere.over(&mut env, below, false), Answer::Refuse);
    assert_eq!(drop_over(&env), None);
}

#[gpui_kit::test]
fn the_recycle_bin_always_moves_and_a_folders_own_name_is_on_the_drag(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let env = env(cx, &disk(0), plain());
    env.grid.read_with(&env.visual, |grid, cx| {
        let bin = grid.drop_words(glint_model::paths::RECYCLE_BIN, cx);
        assert!(bin.always_move);
        assert_eq!(bin.name, "Recycle Bin");
        let folder = grid.drop_words(r"C:\x\docs", cx);
        assert!(!folder.always_move);
        assert_eq!(folder.name, "docs");
        let arabic = grid.drop_words(r"C:\x\تقارير", cx);
        assert_eq!(arabic.name, "تقارير");
    });
}

#[gpui_kit::test]
fn the_arabic_interface_finds_the_tile_under_a_drag_from_the_right(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    testing::set_rtl(true);
    env.explorer.update(&mut env.visual, |_, cx| cx.notify());
    let files = Dragging::new(&[r"C:\other\ملف.txt"]);
    let (docs, _) = spots(&mut env, 0);
    let (pdf, _) = spots(&mut env, 3);
    assert!(docs.x > pdf.x, "docs is at the right");
    // (The drag's words are Arabic now, too: the tile is found by its name.)
    let named = |answer: Answer| match answer {
        Answer::Accept(under) => Some(under.name),
        _ => None,
    };
    assert_eq!(named(files.over(&mut env, docs, true)).as_deref(), Some("docs"));
    // The Arabic-named file is a file: the folder shown takes it.
    assert_eq!(named(files.over(&mut env, pdf, false)).as_deref(), Some("x"));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn tiles_dragged_out_are_dim_until_the_drag_is_over(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.click(1);
    let dragging = |env: &Env| env.grid.read_with(&env.visual, |grid, _| grid.dragging.clone());
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let from = window.find(("file-grid-tile", 1_usize)).bounds().center();
        window.pointer_down(from, MouseButton::Left, cx);
        window.pointer_move(from + point(px(12.), px(0.)), Some(MouseButton::Left), cx);
        window.pointer_up(from + point(px(12.), px(0.)), MouseButton::Left, cx);
    });
    let dim = dragging(&env).expect("the dragged items are dim");
    assert!(dim.iter().all(|path| path.ends_with("a.jpg")) && dim.len() == 1);
    // Windows has the pointer while it drags; the first move with no button down is after it.
    env.visual.update(|window, cx| {
        window.pointer_move(point(px(300.), px(300.)), None, cx);
    });
    assert!(dragging(&env).is_none());
    // Or the one who ran the drag says so.
    env.grid.update(&mut env.visual, |grid, cx| {
        grid.dragging = Some(Arc::new(HashSet::from(["x".to_string()])));
        grid.end_drag(cx);
    });
    assert!(dragging(&env).is_none());
}

#[gpui_kit::test]
fn the_menu_key_opens_the_menu_at_the_focused_tiles_bottom_start_corner(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(20), plain());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    env.visual.update(|_, cx| {
        cx.subscribe(&env.grid, move |_, event: &FileGridEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let corner = |env: &mut Env, tile: usize, rtl: bool| {
        env.visual.update(|window, cx| {
            window.render_frame(cx);
            let bounds = window.find(("file-grid-tile", tile)).bounds();
            point(if rtl { bounds.right() } else { bounds.left() }, bounds.bottom())
        })
    };
    let asked = |events: &Rc<RefCell<Vec<FileGridEvent>>>| match events.borrow().last() {
        Some(FileGridEvent::ContextMenu { position, .. }) => Some(*position),
        _ => None,
    };
    // A tile in the second row, not the first: the corner moves with the row and the column.
    env.click(1);
    env.press("down");
    env.press("shift-f10");
    let focused = env.grid.read_with(&env.visual, |grid, cx| {
        grid.snapshot(cx)
            .and_then(|(rows, selection)| usize::try_from(FileGrid::focus_index(&rows, &selection)).ok())
    });
    let tile = focused.expect("a focused tile");
    let (want, got) = (corner(&mut env, tile, false), asked(&events).expect("a menu"));
    assert!(
        (f32::from(want.x - got.x)).abs() < 1.5 && (f32::from(want.y - got.y)).abs() < 1.5,
        "{got:?} is not the corner {want:?}"
    );
    // In Arabic it is the bottom right of the tile.
    testing::set_rtl(true);
    env.explorer.update(&mut env.visual, |_, cx| cx.notify());
    env.press("shift-f10");
    let (want, got) = (corner(&mut env, tile, true), asked(&events).expect("a menu"));
    assert!(
        (f32::from(want.x - got.x)).abs() < 1.5 && (f32::from(want.y - got.y)).abs() < 1.5,
        "{got:?} is not the corner {want:?}"
    );
    testing::set_rtl(false);
}

/// A folder of real SVG files, for the grid to draw itself.
fn svg_folder(name: &str, files: &[(&str, &str)]) -> (std::path::PathBuf, Vec<glint_model::entries::FileEntry>) {
    let dir = std::env::temp_dir().join(format!("glint-grid-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp folder");
    let mut entries = Vec::new();
    for (file_name, text) in files {
        std::fs::write(dir.join(file_name), text).expect("a temp file");
        entries.push(file(&dir.to_string_lossy(), file_name, text.len() as u64, 1));
    }
    (dir, entries)
}

#[gpui_kit::test]
fn svg_files_are_drawn_by_the_grid_crisp_in_their_own_shape_and_never_asked_of_windows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let wide = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100"><rect width="200" height="100" fill="#c00"/></svg>"##;
    let tall = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 50 200"><rect width="50" height="200" fill="#06c"/></svg>"##;
    let (dir, mut entries) = svg_folder(
        "tiles",
        &[
            ("wide.svg", wide),
            ("tall.svg", tall),
            ("broken.svg", "this is not a drawing"),
        ],
    );
    let dir_text = dir.to_string_lossy().to_string();
    entries.push(file(&dir_text, "photo.jpg", 10, 1));
    let disk = disk(0);
    disk.folder(&dir_text, entries.clone());
    let fake = plain();
    let mut env = env(cx, &disk, fake.clone());
    env.go(&dir_text);
    let scale = env.visual.update(|window, _| window.scale_factor());
    env.settle(|env| {
        env.service
            .read_with(&env.visual, |t, _| t.held_count() >= 3 && t.pending_count() == 0)
    });
    // Windows was asked about the photo only.
    let asked: Vec<String> = fake
        .asked
        .borrow()
        .iter()
        .map(|a| a.path.clone())
        .filter(|path| path.starts_with(&dir_text))
        .collect();
    assert!(
        !asked.is_empty() && asked.iter().all(|path| path.ends_with("photo.jpg")),
        "{asked:?}"
    );
    let look = |name: &str| {
        let entry = entries.iter().find(|e| e.name == name).expect("an entry").clone();
        env.service.read_with(&env.visual, |t, _| t.look(&entry, 96.0, scale))
    };
    let thumbnail::Look::Picture(wide) = look("wide.svg") else {
        panic!("the wide drawing is drawn");
    };
    let thumbnail::Look::Picture(tall) = look("tall.svg") else {
        panic!("the tall drawing is drawn");
    };
    // The shape is kept, and the long side fills the square.
    assert!(
        (wide.width / wide.height - 2.0).abs() < 0.1,
        "{}x{}",
        wide.width,
        wide.height
    );
    assert!(
        (tall.width / tall.height - 0.25).abs() < 0.05,
        "{}x{}",
        tall.width,
        tall.height
    );
    assert!((wide.width - 96.0).abs() < 2.0 && (tall.height - 96.0).abs() < 2.0);
    // What is not a drawing keeps its icon.
    assert!(matches!(look("broken.svg"), thumbnail::Look::Icon));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_svg_files_the_grid_can_read_are_drawn_by_it() {
    use glint_model::entries::attr;
    let svg = |size: u64| file("C:\\x", "logo.SVG", size, 1);
    assert!(thumbnail::is_vector(&svg(1_000)));
    assert!(thumbnail::is_vector(&svg(4 << 20)), "4 MB is the most");
    assert!(!thumbnail::is_vector(&svg((4 << 20) + 1)));
    assert!(!thumbnail::is_vector(&file("C:\\x", "a.png", 10, 1)));
    assert!(!thumbnail::is_vector(&folder("C:\\x", "d.svg")));
    let mut inside = svg(10);
    inside.attrs |= attr::IN_ARCHIVE;
    assert!(!thumbnail::is_vector(&inside), "not in archives");
    let mut cloud = svg(10);
    cloud.attrs |= attr::OFFLINE;
    assert!(
        !thumbnail::is_vector(&cloud),
        "an online-only file isn't downloaded to be drawn"
    );
}

#[test]
fn every_icon_the_grid_draws_is_in_the_embedded_assets() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    for name in [
        IconName::ChevronUp,
        IconName::ChevronDown,
        IconName::SearchX,
        IconName::Trash,
        IconName::FolderOpen,
        IconName::Cloud,
        IconName::CircleCheck,
        IconName::RefreshCw,
        IconName::KeyRound,
        IconName::FileX,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}
