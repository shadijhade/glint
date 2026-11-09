//! Headless tests of the grid: what it draws on a fake disk (labels and roles), that clicks, keys and the wheel
//! change the explorer as the TypeScript did, thumbnails asked for and cancelled, an Arabic name, and the Arabic
//! interface mirroring.

use super::thumbnail::tests::{answer, Fake};
use super::thumbnail::{self, ThumbSource, Thumbnails};
use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{NavOptions, ViewMode};
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
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
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
