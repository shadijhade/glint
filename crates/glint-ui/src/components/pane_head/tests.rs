//! The head of a pane: what it says of a folder, and that a chip narrows the list to its kind and back.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::NavOptions;
use crate::testing;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\Work",
        vec![
            folder("C:\\Work", "src"),
            folder("C:\\Work", "docs"),
            file("C:\\Work", "a.png", 1000, 0),
            file("C:\\Work", "b.png", 2000, 0),
            file("C:\\Work", "notes.txt", 24, 0),
        ],
    );
    disk.folder("C:\\Work\\src", Vec::new());
    disk
}

/// A window holding the head of pane 0, with `C:\Work` open.
fn open(cx: &mut TestAppContext) -> (Entity<PaneHead>, VisualTestContext) {
    let disk = disk();
    cx.update(|cx| {
        testing::init(cx);
        explorer::init_with(cx, disk.source());
    });
    let (head, visual) = cx.add_window_view(|window, cx| PaneHead::new(0, window, cx));
    let mut visual = visual.clone();
    go(&mut visual, "C:\\Work");
    (head, visual)
}

fn go(visual: &mut VisualTestContext, path: &str) {
    let path = path.to_string();
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx)));
    visual.run_until_parked();
}

#[gpui_kit::test]
fn it_names_the_folder_and_counts_what_is_in_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual) = open(cx);
    let said = visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()));
    let said = said.expect("a folder has a head");
    assert_eq!(said.title.as_ref(), "Work");
    assert_eq!(said.facts.as_ref(), "2 folders · 3 files · 3.0 KB");
    // The kinds, commonest first; two of each of the first two, in the order they came.
    let names: Vec<&str> = said.kinds.iter().map(|kind| kind.group.name).collect();
    assert_eq!(names[..2], ["Folders", "Images"], "{names:?}");
    assert_eq!(said.kinds.iter().map(|kind| kind.count).sum::<usize>(), 5);
}

#[gpui_kit::test]
fn an_empty_folder_says_so_and_this_pc_and_a_search_have_no_head(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual) = open(cx);
    go(&mut visual, "C:\\Work\\src");
    let said = visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()));
    assert_eq!(said.expect("a head").facts.as_ref(), "Empty");
    // This PC has its own page.
    go(&mut visual, glint_model::paths::THIS_PC);
    assert!(visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).is_none())));
    // A search's results are headed by the search bar.
    go(&mut visual, "C:\\Work");
    visual
        .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.run_find("png", explorer::Wider::default(), cx)))
        .detach();
    visual.run_until_parked();
    assert!(visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).is_none())));
}

#[gpui_kit::test]
fn a_chip_shows_only_its_kind_and_pressed_again_everything(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual) = open(cx);
    let filter = |visual: &mut VisualTestContext| {
        visual.update(|_, cx| {
            let explorer = explorer::explorer(cx).read(cx);
            explorer.active_tab().map(|tab| tab.filter.clone()).unwrap_or_default()
        })
    };
    // The chips are in the kinds' order: Folders, then Images.
    let press = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("pane-head-chip", 1usize), cx);
        });
        visual.run_until_parked();
    };
    let label = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find(("pane-head-chip", 1usize)).label().map(str::to_string)
    });
    assert_eq!(label.as_deref(), Some("Images: 2"));
    press(&mut visual);
    let images = filter(&mut visual);
    assert!(!images.is_empty(), "the kind's filter is in the box");
    let said = visual
        .update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()))
        .expect("a head");
    assert_eq!(said.facts.as_ref(), "2 of 5 shown");
    assert_eq!(said.filter, images);
    // Again: everything.
    press(&mut visual);
    assert_eq!(filter(&mut visual), "");
}

#[test]
fn only_the_chips_that_fit_are_drawn() {
    let kind = |name: &'static str, count: usize| Category {
        group: glint_model::file_types::TypeGroup { name, hue: 0, sat: 0 },
        count,
        bytes: 0,
        query: String::new(),
    };
    let kinds = [kind("Folders", 6), kind("Code and text", 6), kind("Images", 2)];
    // No room, no chips; room for one; room for all.
    assert_eq!(fitting(&kinds, 0., 16.), 0);
    assert_eq!(fitting(&kinds, 120., 16.), 1);
    assert_eq!(fitting(&kinds, 300., 16.), 2);
    assert_eq!(fitting(&kinds, 2000., 16.), 3);
    // Never more than the commonest few.
    let many: Vec<Category> = (0..12).map(|_| kind("Images", 1)).collect();
    assert_eq!(fitting(&many, 10_000., 16.), CHIPS);
}

#[test]
fn the_band_is_the_whole_width_and_no_part_is_too_thin_to_see() {
    let kind = |count: usize| Category {
        group: FOLDERS,
        count,
        bytes: 0,
        query: String::new(),
    };
    let parts = shares(&[kind(990), kind(9), kind(1)]);
    assert!((parts.iter().sum::<f32>() - 1.).abs() < 0.001, "{parts:?}");
    assert!(parts.iter().all(|share| *share >= 0.014), "{parts:?}");
    assert!(parts[0] > 0.9);
    assert!(shares(&[]).is_empty());
}

// ---- The smart buttons ----------------------------------------------------------------------------------------

use super::lenses::{Lens, MAP_LOOKS};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Folders for the buttons: no pictures, three, twenty, a few with places, and a repository.
fn photo_disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder("C:\\Empty", Vec::new());
    let pictures = |dir: &str, n: usize| {
        (0..n)
            .map(|i| file(dir, &format!("p{i}.jpg"), 1000, 0))
            .collect::<Vec<_>>()
    };
    disk.folder("C:\\Three", pictures("C:\\Three", 3));
    disk.folder("C:\\Twenty", pictures("C:\\Twenty", 20));
    disk.folder("C:\\Trip", pictures("C:\\Trip", 5));
    disk.folder(
        "C:\\Repo",
        vec![file("C:\\Repo", "main.rs", 10, 0), folder("C:\\Repo", "src")],
    );
    disk.folder("C:\\Docs", vec![file("C:\\Docs", "a.txt", 10, 0)]);
    disk
}

/// How many times each probe was asked.
#[derive(Clone, Default)]
struct Asked {
    places: Arc<AtomicUsize>,
    history: Arc<AtomicUsize>,
}

/// Probes that say every picture of "C:\Trip" has a place and that "C:\Repo" has a history, and count the asks.
fn answer(asked: &Asked) -> Probes {
    let (places, history) = (asked.places.clone(), asked.history.clone());
    Probes {
        places: Arc::new(move |files| {
            places.fetch_add(1, Ordering::SeqCst);
            if files.iter().any(|(path, ..)| path.starts_with("C:\\Trip")) {
                files.len()
            } else {
                0
            }
        }),
        history: Arc::new(move |folder| {
            history.fetch_add(1, Ordering::SeqCst);
            folder == "C:\\Repo"
        }),
    }
}

fn open_photos(cx: &mut TestAppContext) -> (Entity<PaneHead>, VisualTestContext, Asked) {
    let disk = photo_disk();
    cx.update(|cx| {
        testing::init(cx);
        explorer::init_with(cx, disk.source());
    });
    let (head, visual) = cx.add_window_view(|window, cx| PaneHead::new(0, window, cx));
    let asked = Asked::default();
    let mut visual = visual.clone();
    visual.update(|_, cx| head.update(cx, |head, _| head.probes = answer(&asked)));
    (head, visual, asked)
}

/// The buttons the head draws now, in order.
fn lenses_of(head: &Entity<PaneHead>, visual: &mut VisualTestContext) -> Vec<Lens> {
    visual.update(|window, cx| {
        window.render_frame(cx);
        head.update(cx, |head, cx| {
            let said = head.head(cx).cloned().expect("a head");
            head.lenses_now(&said, cx).into_iter().map(|(lens, _)| lens).collect()
        })
    })
}

/// Whether the pill of `lens` is on screen.
fn drawn(visual: &mut VisualTestContext, lens: Lens) -> bool {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find((lens.id(), 0usize)).is_some()
    })
}

#[gpui_kit::test]
fn an_empty_folder_and_a_few_pictures_have_no_smart_buttons(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, asked) = open_photos(cx);
    go(&mut visual, "C:\\Empty");
    assert!(lenses_of(&head, &mut visual).is_empty());
    // Nothing to look into: no picture was opened to ask.
    assert_eq!(
        asked.places.load(Ordering::SeqCst),
        0,
        "a folder without pictures does no work"
    );
    go(&mut visual, "C:\\Three");
    assert!(lenses_of(&head, &mut visual).is_empty());
    assert!(!drawn(&mut visual, Lens::Map) && !drawn(&mut visual, Lens::Colors));
    // A folder of files only: no pictures to look into.
    go(&mut visual, "C:\\Docs");
    assert!(lenses_of(&head, &mut visual).is_empty());
    assert_eq!(
        asked.places.load(Ordering::SeqCst),
        1,
        "only the folder with pictures was looked into"
    );
}

#[gpui_kit::test]
fn twenty_pictures_without_places_offer_colors_and_with_places_the_map_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Twenty");
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Colors]);
    assert!(drawn(&mut visual, Lens::Colors) && !drawn(&mut visual, Lens::Map));
    // Five pictures with places: the map, but too few to surf by color.
    go(&mut visual, "C:\\Trip");
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Map]);
    // The pill says how many, and so does its tooltip.
    let label = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find((Lens::Map.id(), 0usize)).label().map(str::to_string)
    });
    assert_eq!(
        label.as_deref(),
        Some("Photo map: 5 photos with a place (Ctrl+Shift+G)")
    );
    assert_eq!(&*Lens::Map.label(5, false), "Map 5");
    assert_eq!(&*Lens::Map.label(MAP_LOOKS, true), "Map 400+");
}

#[gpui_kit::test]
fn a_folder_with_a_history_offers_it_once_known_and_not_before(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, asked) = open_photos(cx);
    go(&mut visual, "C:\\Repo");
    assert_eq!(lenses_of(&head, &mut visual), [Lens::History]);
    // Not known (as in the frames before the answer comes from the background): not shown, and not drawn.
    visual.update(|_, cx| head.update(cx, |head, _| head.history.clear()));
    assert!(lenses_of(&head, &mut visual).is_empty(), "not known yet: not shown");
    assert!(!drawn(&mut visual, Lens::History));
    visual.update(|_, cx| {
        head.update(cx, |head, cx| {
            head.dirty = true;
            head.refresh(cx);
            head.ask(cx);
        })
    });
    visual.run_until_parked();
    assert_eq!(lenses_of(&head, &mut visual), [Lens::History]);
    assert!(drawn(&mut visual, Lens::History));
    // Coming back soon doesn't ask again.
    go(&mut visual, "C:\\Docs");
    go(&mut visual, "C:\\Repo");
    assert_eq!(
        asked.history.load(Ordering::SeqCst),
        3,
        "the repository (twice: its answer was dropped by the test), the other folder once; coming back asks nothing"
    );
    assert_eq!(lenses_of(&head, &mut visual), [Lens::History]);
}

#[gpui_kit::test]
fn the_gallery_always_offers_colors(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Three");
    let mut said = visual
        .update(|_, cx| head.update(cx, |head, cx| head.head(cx).cloned()))
        .expect("a head");
    assert!(!lenses::colors_fit(&said));
    said.is_gallery = true;
    said.is_folder = false;
    assert!(lenses::colors_fit(&said));
}

#[gpui_kit::test]
fn a_panel_that_is_open_keeps_its_pill_lit(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Docs");
    assert!(lenses_of(&head, &mut visual).is_empty());
    let tab = visual.update(|_, cx| explorer::explorer(cx).read(cx).active_tab_id());
    visual.update(|_, cx| {
        crate::components::photo_map::init_with(cx, Arc::new(|_, _| {}));
        crate::components::photo_map::set(cx, tab, true);
        let wheel = crate::components::color_wheel::shared(cx);
        wheel.update(cx, |wheel, cx| wheel.set_shown(tab, true, None, cx));
    });
    visual.run_until_parked();
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Map, Lens::Colors]);
    assert!(drawn(&mut visual, Lens::Map) && drawn(&mut visual, Lens::Colors));
    // Pressing the lit pill closes the panel, and with nothing to show the pill goes.
    visual.update(|window, cx| window.click((Lens::Map.id(), 0usize), cx));
    visual.run_until_parked();
    assert!(!visual.update(|_, cx| crate::components::photo_map::shows(cx, tab)));
    assert_eq!(lenses_of(&head, &mut visual), [Lens::Colors]);
}

#[gpui_kit::test]
fn an_arrival_is_a_different_place_not_a_refresh_or_a_search(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    let arrivals = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| {
            window.render_frame(cx);
            head.update(cx, |head, _| head.arrivals)
        })
    };
    go(&mut visual, "C:\\Twenty");
    let first = arrivals(&mut visual);
    // A filter changes the explorer but not the place.
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.set_filter("p1", None, cx)));
    visual.run_until_parked();
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.set_filter("", None, cx)));
    visual.run_until_parked();
    assert_eq!(arrivals(&mut visual), first, "a filter is not an arrival");
    // A search hides the head; closing it comes back to the same place without arriving again.
    visual
        .update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.run_find("p1", explorer::Wider::default(), cx)))
        .detach();
    visual.run_until_parked();
    assert!(visual.update(|_, cx| head.update(cx, |head, cx| head.head(cx).is_none())));
    let tab = visual.update(|_, cx| explorer::explorer(cx).read(cx).active_tab_id());
    visual.update(|_, cx| explorer::explorer(cx).update(cx, |e, cx| e.clear_find_in(tab, cx)));
    visual.run_until_parked();
    assert_eq!(arrivals(&mut visual), first, "back from a search is the same place");
    go(&mut visual, "C:\\Three");
    assert_eq!(arrivals(&mut visual), first + 1);
}

#[gpui_kit::test]
fn the_chips_are_there_on_the_first_frame_and_the_buttons_are_mirrored(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (head, mut visual, _) = open_photos(cx);
    go(&mut visual, "C:\\Trip");
    // Before anything has been measured the chips already show (the room is the window's, and what doesn't fit
    // wraps out of sight): one frame, and the Images chip is there.
    let chip = visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(("pane-head-chip", 0usize)).is_some()
    });
    assert!(chip, "no pop-in of the chips on the first frame");
    // Mirrored: the buttons come first (at the start of the row, which is the left), and the name after the picture.
    testing::set_rtl(true);
    visual.update(|_, cx| head.update(cx, |_, cx| cx.notify()));
    let (pill, title) = visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find((Lens::Map.id(), 0usize)).bounds(),
            window.find("pane-head-title").bounds(),
        )
    });
    testing::set_rtl(false);
    assert!(
        pill.center().x < title.center().x,
        "in a mirrored head the buttons are on the left: {pill:?} vs {title:?}"
    );
}
