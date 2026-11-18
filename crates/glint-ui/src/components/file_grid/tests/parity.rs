//! The grid does what the list does: Shift and a right-click, the tags' dots, the keyboard reading a folder ahead,
//! and what can be dropped where.

use super::*;
use crate::components::row_facts::{self, fake};
use crate::dialogs::tags;
use glint_model::paths::{GALLERY, RECENT};
use gpui_kit::test::ClickOptions;
use std::sync::Arc;
use std::time::Duration;

/// The events the grid sends, collected.
fn listen(env: &mut Env) -> Rc<RefCell<Vec<FileGridEvent>>> {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    env.visual.update(|_, cx| {
        cx.subscribe(&env.grid, move |_, event: &FileGridEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    events
}

#[gpui_kit::test]
fn shift_and_a_right_click_asks_for_windows_own_menu_and_a_plain_one_does_not(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut env = env(cx, &disk(0), plain());
    let events = listen(&mut env);
    env.visual
        .update(|window, cx| window.right_click(("file-grid-tile", 2_usize), cx));
    env.visual.update(|window, cx| {
        window.click_with_options(
            ("file-grid-tile", 2_usize),
            ClickOptions::new()
                .with_button(MouseButton::Right)
                .with_modifiers(Modifiers::shift()),
            cx,
        )
    });
    let shifts: Vec<bool> = events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            FileGridEvent::ContextMenu { shift, .. } => Some(*shift),
            _ => None,
        })
        .collect();
    assert_eq!(shifts, [false, true]);
}

/// The names of the dots drawn on the tiles ("Tags: Red, Work"), sorted.
fn dots(env: &mut Env) -> Vec<String> {
    env.visual.update(|window, cx| {
        window.render_frame(cx);
        let mut found: Vec<String> = window
            .find_all_by_role(Role::Group)
            .iter()
            .filter_map(|group| group.label().map(str::to_string))
            .filter(|label| label.starts_with("Tags: "))
            .collect();
        found.sort();
        found
    })
}

#[gpui_kit::test]
fn tiles_show_their_tags_as_dots_and_ask_for_each_tile_once(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let tagged = fake::Tags::with(&[("C:\\x\\b.txt", &["Red", "Work"]), ("C:\\x\\docs", &["Blue"])]);
    let media = fake::Media::with(vec![]);
    let (backend, source): (Arc<dyn tags::Backend>, Arc<dyn row_facts::MediaSource>) = (tagged.clone(), media.clone());
    cx.update(|cx| {
        tags::init_with(cx, backend);
        row_facts::init_with(cx, source);
    });
    let mut env = env(cx, &disk(0), plain());
    for _ in 0..400 {
        if dots(&mut env).len() == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Unread tiles show nothing; the tagged ones have their dots, named for a screen reader.
    assert_eq!(dots(&mut env), ["Tags: Blue", "Tags: Red, Work"]);
    env.visual.update(|window, _| {
        assert_eq!(
            window.try_find_by_label("Tags: Red, Work").and_then(|dots| dots.role()),
            Some(Role::Group)
        );
    });
    // The four tiles were read together, and drawing again asks nothing more.
    assert_eq!(tagged.calls().len(), 1);
    assert_eq!(tagged.paths_read(), 4);
    env.frame();
    env.frame();
    assert_eq!(tagged.paths_read(), 4);
}

#[gpui_kit::test]
fn the_arrow_keys_landing_on_a_folder_read_it_ahead(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk(0);
    let mut env = env(cx, &disk, plain());
    assert!(!disk.listed().iter().any(|path| path == "C:\\x\\docs"));
    env.click(1);
    // The folder is the first tile, one before the picture: the keyboard goes back to it.
    env.press("left");
    assert_eq!(env.focus().as_deref(), Some("docs"));
    env.frame();
    env.visual.executor().advance_clock(Duration::from_millis(300));
    env.frame();
    assert!(
        disk.listed().iter().any(|path| path == "C:\\x\\docs"),
        "{:?}",
        disk.listed()
    );
}

/// What a drop of a file from another drive would go into, with the pointer on `tile` (or on nothing).
fn drop_into(env: &mut Env, tile: Option<usize>) -> Option<String> {
    let at = env.visual.update(|window, cx| {
        window.render_frame(cx);
        match tile {
            Some(tile) => window.find(("file-grid-tile", tile)).bounds().center(),
            None => point(px(2.), px(2.)),
        }
    });
    let dragged = vec!["D:\\elsewhere\\z.txt".to_string()];
    env.grid
        .read_with(&env.visual, |grid, cx| grid.drop_under(at, &dragged, cx))
        .map(|(dest, _)| dest)
}

#[gpui_kit::test]
fn nothing_is_dropped_into_the_gallery_recent_or_a_remote_place(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = disk(0);
    disk.folder(GALLERY, vec![]);
    disk.folder(RECENT, vec![]);
    disk.folder("remote:abc", vec![folder("remote:abc", "sub")]);
    let mut env = env(cx, &disk, plain());
    // A folder tile takes it, and so does the folder itself.
    assert_eq!(drop_into(&mut env, Some(0)).as_deref(), Some("C:\\x\\docs"));
    assert_eq!(drop_into(&mut env, None).as_deref(), Some("C:\\x"));

    for place in [GALLERY, RECENT] {
        env.go(place);
        assert_eq!(drop_into(&mut env, None), None, "{place}");
    }
    // A remote folder, and a folder tile in it.
    env.go("remote:abc");
    assert_eq!(drop_into(&mut env, None), None);
    assert_eq!(drop_into(&mut env, Some(0)), None);
}
