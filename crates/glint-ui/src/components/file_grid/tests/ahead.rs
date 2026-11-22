//! Scroll-ahead: the thumbnails one screenful ahead of the scroll are asked for as prefetches, so tiles that
//! scroll in already have their pictures; they cost nothing at rest, are dropped when the direction reverses or
//! the folder changes, and never hold up what is on screen.

use super::*;
use glint_model::entries::FileEntry;

const PHOTOS: usize = 900;

fn name(n: usize) -> String {
    format!("p{n:04}.jpg")
}

fn photo(n: usize) -> FileEntry {
    file("C:\\x", &name(n), 2_000, 1)
}

/// `C:\x` with photos, and `C:\dirs` with only folders.
fn photos() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder("C:\\x", (0..PHOTOS).map(photo).collect());
    disk.folder(
        "C:\\dirs",
        (0..PHOTOS).map(|n| folder("C:\\dirs", &format!("d{n:04}"))).collect(),
    );
    disk
}

/// A source that has every photo's thumbnail (and that serves prefetches, unless `ignores_ahead`).
fn source(ignores_ahead: bool) -> Rc<Fake> {
    let fake = plain();
    fake.ignores_ahead.set(ignores_ahead);
    for n in 0..PHOTOS {
        fake.answers.borrow_mut().insert(
            format!("C:\\x\\{}", name(n)),
            answer(64, 48, [7, 7, 7, 255], true, Origin::Cache),
        );
    }
    fake
}

fn ahead_asks(fake: &Fake) -> usize {
    fake.asked.borrow().iter().filter(|ask| ask.ahead).count()
}

impl Env {
    /// Lets the answers that can arrive arrive, and draws.
    fn drain(&mut self) {
        for _ in 0..30 {
            self.visual.run_until_parked();
            self.frame();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    fn top(&self) -> f32 {
        self.grid.read_with(&self.visual, |grid, _| grid.scroll_top())
    }

    fn body_height(&self) -> f32 {
        self.grid
            .read_with(&self.visual, |grid, _| f32::from(grid.body.get().size.height))
    }

    /// Scrolls with the wheel (down for `down`) until the grid has moved `pixels` more.
    fn scroll_by(&mut self, pixels: f32, down: bool) {
        let from = self.top();
        for _ in 0..400 {
            if (self.top() - from).abs() >= pixels {
                return;
            }
            self.wheel(if down { -1. } else { 1. }, Modifiers::default());
        }
    }

    /// How many tiles drawn on the next frame have no picture of their own yet (their icon, or a stand-in).
    fn tiles_without_picture(&mut self) -> usize {
        let scale = self.visual.update(|window, _| window.scale_factor());
        let names = self.tile_names();
        assert!(names.len() > 10, "{} tiles drawn", names.len());
        let looks: Vec<bool> = self.service.read_with(&self.visual, |thumbs, _| {
            names
                .iter()
                .filter_map(|name| {
                    let n: usize = name.strip_prefix('p')?.strip_suffix(".jpg")?.parse().ok()?;
                    Some(matches!(
                        thumbs.look(&photo(n), 96.0, scale),
                        thumbnail::Look::Picture(_)
                    ))
                })
                .collect()
        });
        looks.iter().filter(|has| !**has).count()
    }
}

/// Opens the photos, scrolls one step to set the direction, lets everything that can arrive arrive, scrolls a
/// second step of a bit more than half a screen, and counts the tiles drawn without a picture on the first
/// frame after it.
fn tiles_without_picture_after_the_step(cx: &mut TestAppContext, ignores_ahead: bool) -> (usize, usize) {
    let fake = source(ignores_ahead);
    let mut env = env(cx, &photos(), fake.clone());
    env.drain();
    assert_eq!(ahead_asks(&fake), 0, "nothing is prefetched before the grid has moved");
    let screen = env.body_height();
    assert!(screen > 100.0, "a body of {screen}");
    env.scroll_by(screen * 0.6, true);
    env.drain();
    let before_step = env.top();
    env.scroll_by(screen * 0.6, true);
    assert!(env.top() > before_step + screen * 0.5);
    let without = env.tiles_without_picture();
    (without, ahead_asks(&fake))
}

#[gpui_kit::test]
fn tiles_scrolled_into_view_already_have_their_pictures(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (before, asks_before) = tiles_without_picture_after_the_step(cx, true);
    let (after, asks_after) = tiles_without_picture_after_the_step(cx, false);
    println!(
        "tiles without a picture on the first frame after a scroll step: {before} without prefetch -> {after} with it \
         ({asks_after} prefetches asked)"
    );
    assert!(before >= 5, "without prefetch the step shows {before} icons");
    assert_eq!(after, 0, "with it every tile has its picture");
    assert!(
        asks_before > 0 && asks_after > 0,
        "the asks were made either way: {asks_before} {asks_after}"
    );
}

#[gpui_kit::test]
fn what_is_on_screen_is_never_held_up_by_a_prefetch(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    // A source that never serves prefetches: they stay out, and the real asks beside them are answered all the same.
    let fake = source(true);
    let mut env = env(cx, &photos(), fake.clone());
    env.drain();
    let screen = env.body_height();
    env.scroll_by(screen * 3.0, true);
    env.drain();
    assert!(ahead_asks(&fake) > 0);
    assert_eq!(env.tiles_without_picture(), 0, "the tiles on screen got their pictures");
    // A real ask is never marked as a prefetch.
    assert!(fake.asked.borrow().iter().any(|ask| !ask.ahead));
}

#[gpui_kit::test]
fn the_screenful_ahead_is_bounded_asked_once_and_lies_beyond_the_tiles_shown(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = source(false);
    let mut env = env(cx, &photos(), fake.clone());
    env.drain();
    let screen = env.body_height();
    env.scroll_by(screen * 0.5, true);
    env.drain();
    let asked = ahead_asks(&fake);
    assert!(asked > 0 && asked <= 160, "{asked} asked ahead");
    // Resting asks nothing more, however many frames are drawn.
    for _ in 0..20 {
        env.frame();
    }
    env.visual.run_until_parked();
    assert_eq!(ahead_asks(&fake), asked);
    // Their names sort after the tiles on screen (the photos are in name order).
    let shown = env.tile_names();
    let last_shown = shown.last().expect("tiles shown").clone();
    for ask in fake.asked.borrow().iter().filter(|ask| ask.ahead) {
        let file = ask.path.rsplit('\\').next().unwrap_or("");
        assert!(file > last_shown.as_str(), "{file} is not beyond {last_shown}");
    }
}

#[gpui_kit::test]
fn reversing_the_scroll_drops_what_was_asked_ahead(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    // Prefetches that are never served stay out at the source, so there is something to drop.
    let fake = source(true);
    let mut env = env(cx, &photos(), fake.clone());
    env.drain();
    let screen = env.body_height();
    env.scroll_by(screen * 3.0, true);
    env.drain();
    assert!(fake.cancelled_ahead.borrow().is_empty(), "scrolling on drops nothing");
    assert!(ahead_asks(&fake) > 0);
    // The other way: the old prefetches go (and only those: the real asks are not cancelled by it).
    let cancels = fake.cancelled.borrow().len();
    env.scroll_by(screen * 0.5, false);
    env.drain();
    assert_eq!(
        *fake.cancelled_ahead.borrow(),
        ["tab1"],
        "the reversal dropped the prefetches"
    );
    assert_eq!(fake.cancelled.borrow().len(), cancels, "no real ask was dropped by it");
}

#[gpui_kit::test]
fn scrolling_up_asks_the_screenful_above_nearest_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = source(false);
    let mut env = env(cx, &photos(), fake.clone());
    env.drain();
    // Jump to the end (nothing lies beyond it), then scroll up through photos that were never in view.
    env.click(1);
    env.press("end");
    env.drain();
    assert_eq!(ahead_asks(&fake), 0, "the end has nothing ahead");
    let screen = env.body_height();
    env.scroll_by(screen * 0.5, false);
    env.drain();
    let asks = fake.asked.borrow();
    let names: Vec<&str> = asks
        .iter()
        .filter(|ask| ask.ahead)
        .map(|ask| ask.path.rsplit('\\').next().unwrap_or(""))
        .collect();
    assert!(names.len() > 4, "asked ahead going up: {}", names.len());
    assert!(names[0] > names[1], "nearest first going up: {names:?}");
    let first_shown_tile = env.tile_names().into_iter().next().expect("tiles");
    assert!(
        names.iter().all(|name| *name < first_shown_tile.as_str()),
        "all of them above the tiles shown ({first_shown_tile}): {names:?}"
    );
}

#[gpui_kit::test]
fn another_folder_drops_the_prefetches_and_a_folder_of_folders_asks_none(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = source(true);
    let mut env = env(cx, &photos(), fake.clone());
    env.drain();
    let screen = env.body_height();
    env.scroll_by(screen * 2.0, true);
    env.drain();
    let asked = ahead_asks(&fake);
    assert!(asked > 0);
    env.go("C:\\dirs");
    env.drain();
    assert!(
        fake.cancelled.borrow().iter().any(|cancel| cancel.1.is_none()),
        "what the tab waited for was dropped with its folder"
    );
    env.scroll_by(screen * 2.0, true);
    env.drain();
    assert_eq!(
        ahead_asks(&fake),
        asked,
        "folders have no thumbnails: nothing is prefetched for them"
    );
}
