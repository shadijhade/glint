//! Tests of the gallery's months: the grouping (pure), and the view: what it draws for a gallery of photos with
//! Arabic names, in both directions, what a click selects, and when it draws nothing.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{Explorer, NavOptions, Rows};
use crate::state;
use crate::testing;
use glint_model::entries::{FileEntry, Sort, SortKey};
use glint_model::store::StartFolder;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::sync::Arc;

const GALLERY: &str = "shell:Gallery";
const EMPTY: &str = "C:\\Empty";

/// Noon UTC on a day: the same calendar day in every time zone within twelve hours of UTC, so the local month
/// is the month asked for wherever the tests run (the days used are the 10th to the 20th).
fn ms(year: i64, month: i64, day: i64) -> i64 {
    // days from civil (Howard Hinnant)
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400_000 + 12 * 3_600_000
}

fn rows_of(entries: Vec<FileEntry>) -> Rows {
    Rows::identity(Arc::from(entries))
}

fn photo(name: &str, year: i64, month: i64, day: i64) -> FileEntry {
    file("C:\\Pics", name, 10, ms(year, month, day))
}

#[test]
fn months_are_runs_of_the_same_month_with_their_count_and_first_item() {
    let rows = rows_of(vec![
        photo("a.jpg", 2026, 3, 18),
        photo("b.jpg", 2026, 3, 12),
        // Folders and files with no date are not photos taken in a month.
        folder("C:\\Pics", "Album"),
        file("C:\\Pics", "undated.jpg", 1, 0),
        photo("c.jpg", 2026, 1, 15),
        photo("d.jpg", 2025, 12, 14),
        photo("e.jpg", 2025, 12, 11),
        photo("f.jpg", 2025, 12, 10),
    ]);
    let months = months_of(&rows);
    let summary: Vec<(i64, u32, usize, &str)> = months
        .iter()
        .map(|m| (m.year, m.month, m.count, m.first.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            (2026, 2, 2, "C:\\Pics\\a.jpg"),
            (2026, 0, 1, "C:\\Pics\\c.jpg"),
            (2025, 11, 3, "C:\\Pics\\d.jpg"),
        ]
    );
}

#[test]
fn a_month_that_comes_back_later_in_the_order_is_a_new_run() {
    // The TypeScript groups runs, not months: sorted by name, one month can show twice.
    let rows = rows_of(vec![
        photo("a.jpg", 2026, 3, 18),
        photo("b.jpg", 2026, 2, 12),
        photo("c.jpg", 2026, 3, 11),
    ]);
    let counts: Vec<(u32, usize)> = months_of(&rows).iter().map(|m| (m.month, m.count)).collect();
    assert_eq!(counts, [(2, 1), (1, 1), (2, 1)]);
}

#[test]
fn the_utc_fallback_reads_the_calendar() {
    assert_eq!(utc_month(0), (1970, 0));
    assert_eq!(utc_month(ms(2024, 2, 29)), (2024, 1));
    assert_eq!(utc_month(ms(2000, 12, 31)), (2000, 11));
    assert_eq!(utc_month(ms(1999, 1, 1)), (1999, 0));
    assert_eq!(utc_month(-86_400_000), (1969, 11), "before 1970");
}

#[test]
fn local_months_agree_with_the_calendar_for_midmonth_noons() {
    for (y, m) in [(2026, 3), (2025, 12), (2024, 2), (1998, 7)] {
        let (year, month) = local_month(ms(y, m, 15)).expect("a date");
        assert_eq!((year, month), (y, u32::try_from(m - 1).unwrap_or(0)));
    }
}

#[test]
fn a_year_gets_a_heading_before_its_first_month() {
    let rows = rows_of(vec![
        photo("a.jpg", 2026, 3, 18),
        photo("b.jpg", 2026, 1, 12),
        photo("c.jpg", 2025, 12, 11),
    ]);
    let lines = lines_of(&months_of(&rows));
    assert_eq!(
        lines,
        [
            Line::Year(2026),
            Line::Month(0),
            Line::Month(1),
            Line::Year(2025),
            Line::Month(2)
        ]
    );
    assert!(lines_of(&[]).is_empty());
}

#[test]
fn the_names_of_the_months_are_english_until_the_catalog_has_them() {
    let _guard = testing::lock();
    testing::set_rtl(false);
    assert_eq!(month_name(0), "January");
    assert_eq!(month_name(11), "December");
    assert_eq!(month_name(12), "");
}

fn settle(visual: &mut VisualTestContext) {
    for _ in 0..30 {
        visual.run_until_parked();
    }
}

fn setup(
    cx: &mut TestAppContext,
    photos: Vec<FileEntry>,
    rtl: bool,
) -> (VisualTestContext, Entity<GalleryMonths>, Entity<Explorer>) {
    cx.update(testing::init);
    cx.update(|cx| state::update(cx, |settings| settings.start_folder = StartFolder::ThisPc));
    testing::set_rtl(rtl);
    let disk = FakeDisk::new();
    disk.folder(GALLERY, photos);
    disk.folder(EMPTY, vec![]);
    cx.update(|cx| crate::explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| crate::explorer::explorer(cx));
    let (mut visual, view) = testing::open_window(cx, |window, cx| cx.new(|cx| GalleryMonths::new(0, window, cx)));
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.init_explorer(Vec::new(), cx).detach()));
    settle(&mut visual);
    visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.navigate(GALLERY, NavOptions::default(), cx);
        })
    });
    settle(&mut visual);
    // Newest first, as the Gallery shows them.
    visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.set_sort(
                Sort {
                    key: SortKey::Modified,
                    desc: true,
                },
                cx,
            )
        })
    });
    settle(&mut visual);
    (visual, view, explorer)
}

fn arabic_gallery() -> Vec<FileEntry> {
    vec![
        photo("صورة-١.jpg", 2026, 3, 18),
        photo("صورة-٢.jpg", 2026, 3, 12),
        photo("شاطئ.jpg", 2026, 1, 15),
        photo("عيد.jpg", 2025, 12, 14),
        photo("عائلة.jpg", 2025, 12, 11),
        photo("حفلة.jpg", 2025, 12, 10),
    ]
}

fn frame(visual: &mut VisualTestContext) {
    visual.update(|window, cx| window.render_frame(cx));
}

fn role_of(visual: &mut VisualTestContext, label: &str) -> Option<Role> {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).and_then(|element| element.role())
    })
}

#[gpui_kit::test]
fn the_months_are_listed_newest_first_with_their_counts(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer) = setup(cx, arabic_gallery(), false);
    let months: Vec<(i64, u32, usize)> = view.read_with(&visual, |v, _| {
        v.months().iter().map(|m| (m.year, m.month, m.count)).collect()
    });
    assert_eq!(months, [(2026, 2, 2), (2026, 0, 1), (2025, 11, 3)]);
    assert_eq!(role_of(&mut visual, "Months"), Some(Role::Navigation));
    assert_eq!(role_of(&mut visual, "2026"), Some(Role::Heading));
    assert_eq!(role_of(&mut visual, "2025"), Some(Role::Heading));
    assert_eq!(role_of(&mut visual, "March 2026"), Some(Role::Button));
    assert_eq!(role_of(&mut visual, "January 2026"), Some(Role::Button));
    assert_eq!(role_of(&mut visual, "December 2025"), Some(Role::Button));
}

#[gpui_kit::test]
fn a_click_selects_the_months_first_item_and_focuses_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer) = setup(cx, arabic_gallery(), false);
    frame(&mut visual);
    // Third month: December 2025, whose first photo (newest) is "عيد.jpg".
    visual.update(|window, cx| window.click(("gallery-month", 2_usize), cx));
    settle(&mut visual);
    explorer.read_with(&visual, |e, _| {
        let selection = &e.pane_tab(0).expect("a tab").selection;
        assert_eq!(selection.focus(), Some("C:\\Pics\\عيد.jpg"));
        assert_eq!(selection.anchor(), Some("C:\\Pics\\عيد.jpg"));
        assert_eq!(selection.len(), 1);
        assert!(selection.contains("C:\\Pics\\عيد.jpg"));
    });
    // That month is now the marked one.
    assert_eq!(visual.update(|_, cx| view.read(cx).current(cx)), Some(2));
}

#[gpui_kit::test]
fn the_focused_items_month_is_the_marked_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer) = setup(cx, arabic_gallery(), false);
    assert_eq!(
        visual.update(|_, cx| view.read(cx).current(cx)),
        None,
        "nothing is focused yet"
    );
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.select_only(Some("C:\\Pics\\شاطئ.jpg"), cx)));
    settle(&mut visual);
    assert_eq!(
        visual.update(|_, cx| view.read(cx).current(cx)),
        Some(1),
        "January 2026"
    );
}

#[gpui_kit::test]
fn one_month_or_none_draws_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer) = setup(
        cx,
        vec![photo("a.jpg", 2026, 3, 18), photo("b.jpg", 2026, 3, 12)],
        false,
    );
    assert_eq!(view.read_with(&visual, |v, _| v.months().len()), 1);
    assert!(!view.read_with(&visual, |v, _| v.is_shown()));
    assert_eq!(role_of(&mut visual, "Months"), None);
}

#[gpui_kit::test]
fn the_months_change_when_the_folder_does(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer) = setup(cx, arabic_gallery(), false);
    assert_eq!(view.read_with(&visual, |v, _| v.months().len()), 3);
    // Leaving the gallery for an empty folder takes them away.
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.navigate(EMPTY, NavOptions::default(), cx)));
    settle(&mut visual);
    assert!(view.read_with(&visual, |v, _| v.months().is_empty()));
    assert_eq!(role_of(&mut visual, "Months"), None);
}

#[gpui_kit::test]
fn in_arabic_the_list_has_its_border_at_the_right_and_mirrors_its_rows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _explorer) = setup(cx, arabic_gallery(), true);
    frame(&mut visual);
    assert_eq!(view.read_with(&visual, |v, _| v.months().len()), 3);
    assert_eq!(role_of(&mut visual, "Months"), Some(Role::Navigation));
    // The name is at the start (right) edge and the count at the end (left): the name's right edge is beyond
    // the row's middle. Compare the two parts of a row by their order on screen.
    let (row, nav) = visual.update(|window, _| {
        (
            window.try_find(("gallery-month", 0_usize)).map(|e| e.bounds()),
            window.try_find_by_label("Months").map(|e| e.bounds()),
        )
    });
    let (row, nav) = (row.expect("a month row"), nav.expect("the list"));
    assert!(
        row.right() <= nav.right() && row.left() >= nav.left(),
        "the row is inside the list"
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_big_gallery_is_grouped_in_the_background(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut photos = Vec::new();
    for i in 0..(BACKGROUND_ABOVE + 500) {
        // Two thirds in March, the rest in February, named so the newest come first by name too.
        let (month, day) = if i < 4_000 { (3, 18) } else { (2, 14) };
        photos.push(photo(&format!("صورة{i:05}.jpg"), 2026, month, day));
    }
    let (visual, view, _explorer) = setup(cx, photos, false);
    let months: Vec<(u32, usize)> =
        view.read_with(&visual, |v, _| v.months().iter().map(|m| (m.month, m.count)).collect());
    assert_eq!(months, [(2, 4_000), (1, BACKGROUND_ABOVE + 500 - 4_000)]);
}

fn press(visual: &mut VisualTestContext, key: &str) {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.press(key, cx);
    });
    settle(visual);
}

#[gpui_kit::test]
fn the_months_are_reached_with_tab_and_moved_between_and_chosen_with_keys(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, explorer) = setup(cx, arabic_gallery(), false);
    frame(&mut visual);
    // Tab reaches the first month; Down twice reaches the third (December 2025); Up goes back one.
    visual.update(|window, cx| window.focus_next(cx));
    press(&mut visual, "down");
    press(&mut visual, "down");
    press(&mut visual, "up");
    let focused = |visual: &mut VisualTestContext| {
        visual.update(|window, cx| view.read(cx).focus.iter().position(|f| f.is_focused(window)))
    };
    assert_eq!(focused(&mut visual), Some(1), "down, down, up from the first month");
    press(&mut visual, "end");
    assert_eq!(focused(&mut visual), Some(2));
    // Enter chooses it: its first photo is selected.
    press(&mut visual, "enter");
    explorer.read_with(&visual, |e, _| {
        let selection = &e.pane_tab(0).expect("a tab").selection;
        assert_eq!(selection.focus(), Some(r"C:\Pics\عيد.jpg"));
    });
}
