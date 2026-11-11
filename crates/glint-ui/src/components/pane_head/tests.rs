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
