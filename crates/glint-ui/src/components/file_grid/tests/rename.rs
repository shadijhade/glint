//! Renaming in place in the grid (F2 on a tile), and Escape ending a search as well as the filter.

use super::*;
use crate::actions::Rename;

impl Env {
    /// The window is the active one, as a person's is: focus events (the box losing the keyboard to another tile)
    /// are only sent in an active window.
    fn activate(&mut self) {
        self.visual.update(|window, _| window.activate_window());
        self.visual.run_until_parked();
        self.frame();
    }

    /// F2 as the workspace answers it: the explorer starts renaming the focused item. Two frames, because one
    /// makes the box and the next focuses it.
    fn f2(&mut self) {
        self.explorer.update(&mut self.visual, |e, cx| e.begin_rename(None, cx));
        self.frame();
        self.frame();
    }

    fn renaming(&self) -> Option<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.pane_tab(0).and_then(|t| {
                t.renaming
                    .as_deref()
                    .map(|p| p.rsplit('\\').next().unwrap_or(p).to_string())
            })
        })
    }

    /// The text selected in the rename box, and all of it.
    fn box_text(&mut self) -> Option<(String, String)> {
        let state = self
            .grid
            .read_with(&self.visual, |grid, _| grid.rename.as_ref().map(|r| r.state.clone()))?;
        Some(self.visual.update(|_, cx| {
            let input = state.read(cx);
            let text = input.value().to_string();
            let range = input.selected_range();
            (text.get(range).unwrap_or_default().to_string(), text)
        }))
    }

    fn grid_has_the_keyboard(&mut self) -> bool {
        let grid = self.grid.clone();
        self.visual.update(|window, cx| grid.read(cx).focus.is_focused(window))
    }

    fn has_box(&mut self) -> bool {
        self.visual
            .update(|window, _| window.try_find_by_label("New name").is_some())
    }

    fn names(&self) -> Vec<String> {
        self.explorer.read_with(&self.visual, |e, _| {
            e.pane_tab(0)
                .map(|t| t.rows.iter().map(|entry| entry.name.clone()).collect())
                .unwrap_or_default()
        })
    }
}

#[gpui_kit::test]
fn f2_on_a_tile_opens_a_named_box_with_the_name_selected_without_its_extension(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    // The workspace answers F2 (the grid leaves it to propagate): stand in for it.
    let explorer = env.explorer.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &Rename, cx| explorer.update(cx, |e, cx| e.begin_rename(None, cx)));
    });
    env.click(2);
    assert!(!env.has_box());
    env.press("f2");
    env.frame();
    env.frame();
    assert_eq!(env.renaming().as_deref(), Some("b.txt"));
    assert!(env.has_box());
    env.visual.update(|window, _| {
        let input = window.find_by_label("New name");
        assert_eq!(
            input.role(),
            Some(Role::MultilineTextInput),
            "the box is named and has a role"
        );
        // It is in the tile, below the thumbnail and where the name was.
        let tile = window.find(("file-grid-tile", 2usize)).bounds();
        assert!(input.bounds().is_contained_within(&tile), "{tile:?}");
    });
    assert_eq!(env.box_text(), Some(("b".to_string(), "b.txt".to_string())));
}

#[gpui_kit::test]
fn a_folder_is_selected_whole_and_an_arabic_name_without_its_extension(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.click(0);
    env.f2();
    assert_eq!(env.box_text(), Some(("docs".to_string(), "docs".to_string())));
    env.press("escape");
    env.frame();
    env.click(3);
    env.f2();
    assert_eq!(
        env.box_text(),
        Some(("تقرير الربع".to_string(), "تقرير الربع.pdf".to_string()))
    );
}

#[gpui_kit::test]
fn typing_and_enter_rename_the_item_and_do_not_open_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    // A file: the new name replaces the selected part and keeps the extension.
    env.click(2);
    env.f2();
    env.visual.update(|window, cx| window.input("gamma", cx));
    env.press("enter");
    env.settle(|env| env.names().contains(&"gamma.txt".to_string()));
    assert!(!env.names().contains(&"b.txt".to_string()));
    assert_eq!(env.renaming(), None, "the box is closed");
    assert!(!env.has_box());
    assert_eq!(env.selected(), ["gamma.txt"], "the renamed item is selected");

    // A folder: Enter in the box must not run the grid's own Enter, which would open it under its old name.
    env.click(0);
    env.f2();
    env.visual.update(|window, cx| window.input("papers", cx));
    env.press("enter");
    env.settle(|env| env.names().contains(&"papers".to_string()));
    assert_eq!(env.path(), "C:\\x", "the folder was renamed, not opened");
    // The same for Ctrl+Enter, F2 and Alt+Enter while a box is open.
    env.f2();
    env.press("ctrl-enter");
    env.press("alt-enter");
    assert_eq!(env.path(), "C:\\x");
}

#[gpui_kit::test]
fn escape_cancels_and_a_click_elsewhere_commits(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.activate();
    env.click(2);
    env.f2();
    env.visual.update(|window, cx| window.input("gamma", cx));
    env.press("escape");
    env.frame();
    assert_eq!(env.renaming(), None);
    assert!(!env.has_box());
    assert_eq!(
        env.names(),
        ["docs", "a.jpg", "b.txt", "تقرير الربع.pdf"],
        "nothing was renamed"
    );
    assert_eq!(
        env.selected(),
        ["b.txt"],
        "Escape in the box does not also clear the selection"
    );

    // Another tile pressed while the box is open: what is typed is kept, as the web app's blur did.
    env.f2();
    env.visual.update(|window, cx| window.input("delta", cx));
    env.click(1);
    env.settle(|env| env.names().contains(&"delta.txt".to_string()));
    assert_eq!(env.renaming(), None);
}

#[gpui_kit::test]
fn the_names_keys_stay_in_the_box_and_a_character_no_name_can_hold_is_refused(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.click(2);
    env.f2();
    // Arrows and letters edit the box; they move no tile and do no type-ahead.
    env.press("right");
    env.press("down");
    env.visual.update(|window, cx| window.input("a", cx));
    assert_eq!(env.focus().as_deref(), Some("b.txt"));
    // Characters no name can hold, and Shift+Enter, add nothing.
    env.press("?");
    env.press("shift-enter");
    let (_, text) = env.box_text().expect("the box is open");
    assert!(!text.contains('?') && !text.contains('\n'), "{text:?}");
    assert!(env.renaming().is_some(), "refusing a character does not close the box");
}

#[gpui_kit::test]
fn the_grid_has_the_keyboard_again_when_the_box_closes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    for key in ["escape", "enter"] {
        let mut env = env(cx, &disk(0), plain());
        env.click(2);
        assert!(env.grid_has_the_keyboard());
        env.f2();
        assert!(
            !env.grid_has_the_keyboard(),
            "the box holds the keyboard while it is open"
        );
        env.press(key);
        env.settle(|env| env.renaming().is_none() && !env.has_box());
        env.frame();
        assert!(env.grid_has_the_keyboard(), "after {key} nothing else holds it");
        // A key that is an action of the grid reaches it again.
        env.press("ctrl-a");
        assert_eq!(env.selected().len(), 4, "Ctrl+A after {key}");
    }
}

#[gpui_kit::test]
fn the_box_keeps_its_text_and_the_keyboard_when_a_resize_reflows_the_tiles(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    env.visual.simulate_resize(size(px(1000.), px(700.)));
    env.click(3);
    env.f2();
    env.visual.update(|window, cx| window.input("x", cx));
    let before = env.visual.update(|window, _| window.find_by_label("New name").bounds());
    let entity = env
        .grid
        .read_with(&env.visual, |grid, _| grid.rename.as_ref().map(|r| r.state.entity_id()));

    // Narrower: fewer tiles to a row, so the fourth tile moves to another row.
    env.visual.simulate_resize(size(px(330.), px(700.)));
    env.frame();
    env.frame();
    env.frame();
    let after = env.visual.update(|window, _| window.find_by_label("New name").bounds());
    assert_ne!(before.origin, after.origin, "the tile (and its box) moved");
    assert_eq!(
        env.grid
            .read_with(&env.visual, |grid, _| grid.rename.as_ref().map(|r| r.state.entity_id())),
        entity,
        "the same box, not a new one"
    );
    let (_, text) = env.box_text().expect("the box is still open");
    assert!(text.starts_with('x'), "what was typed stays: {text:?}");
    assert!(!env.grid_has_the_keyboard(), "and so does the focus in the box");
    env.visual.update(|window, cx| window.input("y", cx));
    assert!(
        env.box_text().is_some_and(|(_, text)| text.contains('y')),
        "typing still reaches it"
    );
}

#[gpui_kit::test]
fn a_long_name_wraps_in_the_box_over_the_tiles_two_lines(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let long = "a very long name that does not fit on one line of a tile at all, however wide it is.txt";
    let disk = FakeDisk::new();
    disk.folder("C:\\x", vec![file("C:\\x", long, 1, 1), file("C:\\x", "a.txt", 1, 1)]);
    let mut env = env(cx, &disk, plain());
    let boxed = |env: &mut Env| {
        for _ in 0..6 {
            env.frame();
        }
        env.visual.update(|window, _| window.find("file-grid-rename").bounds())
    };
    env.click(1);
    env.f2();
    let one_line = boxed(&mut env);
    env.press("escape");
    env.frame();
    env.click(0);
    env.f2();
    let wrapped = boxed(&mut env);
    assert!(
        wrapped.size.height > one_line.size.height,
        "{wrapped:?} against {one_line:?}: the long name takes more lines"
    );
    // Never more than the label's two lines (about one more row than one line): the name is cut off in the box,
    // not the tile's size line pushed out of the tile.
    assert!(
        f32::from(wrapped.size.height) < 2.5 * f32::from(one_line.size.height),
        "{wrapped:?} against {one_line:?}"
    );
    let tile = env
        .visual
        .update(|window, _| window.find(("file-grid-tile", 0usize)).bounds());
    assert!(wrapped.is_contained_within(&tile), "{tile:?} {wrapped:?}");
}

#[gpui_kit::test]
fn escape_ends_a_search_and_clears_a_filter(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    let searching = |env: &Env| {
        env.explorer
            .read_with(&env.visual, |e, _| e.pane_tab(0).is_some_and(|t| t.find.is_some()))
    };
    // A search's results: Escape on a tile ends the search (`clearFind`), and the folder's own tiles are back.
    let task = env.explorer.update(&mut env.visual, |e, cx| {
        e.run_find("b", crate::explorer::Wider::default(), cx)
    });
    cx.foreground_executor().block_test(task);
    env.settle(|env| env.names() == ["b.txt"]);
    assert!(searching(&env), "the search ran");
    env.click(0);
    env.press("escape");
    env.settle(|env| env.names().len() == 4);
    assert!(!searching(&env), "Escape ended the search");
    // A filter alone is cleared as before.
    env.explorer
        .update(&mut env.visual, |e, cx| e.set_filter("b.", None, cx));
    env.settle(|env| env.names() == ["b.txt"]);
    env.click(0);
    env.press("escape");
    env.settle(|env| env.names().len() == 4);
}
