//! The list's compare marks: the pill after the name, the dimmed rows that are the same, and the empty state of
//! "diff only".

use super::*;

/// `C:\x` of [`small_disk`] next to `C:\y`: `alpha.txt` is the same, `beta.md` has another size there,
/// `berry.png` is only here and `docs` is a folder in both; `C:\twin` is the same as `C:\x`.
fn side_by_side(cx: &mut TestAppContext) -> Ui {
    let disk = small_disk();
    disk.folder(
        "C:\\y",
        vec![
            folder("C:\\y", "docs"),
            file("C:\\y", "alpha.txt", 100, 0),
            file("C:\\y", "beta.md", 3001, 0),
            file("C:\\y", "extra.txt", 1, 0),
        ],
    );
    disk.folder(
        "C:\\twin",
        vec![
            folder("C:\\twin", "docs"),
            file("C:\\twin", "alpha.txt", 100, 0),
            file("C:\\twin", "beta.md", 3000, 0),
            file("C:\\twin", "berry.png", 20_000, 0),
        ],
    );
    let mut ui = open(cx, &disk, "C:\\x");
    let explorer = ui.explorer.clone();
    ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.toggle_dual(cx);
            e.navigate("C:\\y".to_string(), NavOptions::default(), cx);
        })
    });
    ui.frame();
    ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.toggle_compare_panes(cx);
            e.focus_pane(0, cx);
        })
    });
    for _ in 0..4 {
        ui.frame();
    }
    ui
}

#[gpui_kit::test]
fn rows_say_how_they_differ_from_the_other_pane_and_the_same_ones_are_dimmed(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = side_by_side(cx);
    assert_eq!(ui.role_of("Only here: berry.png"), Some(Role::Group));
    assert_eq!(ui.role_of("Different: beta.md"), Some(Role::Group));
    assert_eq!(ui.role_of("Older: beta.md"), None);
    assert_eq!(ui.role_of("Only here: alpha.txt"), None, "the same has no pill");
    ui.visual.update(|window, _| {
        let pill = window.find_by_label("Only here: berry.png");
        assert_eq!(pill.description(), Some("Does not exist in the other pane"));
    });
    let frame = ui.frame_of();
    let same = |name: &str| {
        let entry = frame.rows.iter().find(|e| e.name == name).expect("a row");
        frame.same_as_other(entry)
    };
    assert!(same("alpha.txt") && same("docs"));
    assert!(!same("beta.md") && !same("berry.png"));
    // The rows are all still there.
    assert_eq!(ui.labels(), ["docs", "alpha.txt", "berry.png", "beta.md"]);
}

#[gpui_kit::test]
fn the_pills_are_there_in_the_arabic_interface_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = side_by_side(cx);
    testing::set_rtl(true);
    ui.frame();
    // The words are in Arabic now ("Only here" is "هنا فقط"): the pill is named after the file.
    let found = ui.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label("هنا فقط: berry.png").is_some()
    });
    testing::set_rtl(false);
    assert!(found);
}

#[gpui_kit::test]
fn diff_only_leaves_out_the_same_rows_and_says_so_when_none_are_left(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = side_by_side(cx);
    let explorer = ui.explorer.clone();
    ui.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.toggle_diff_only(cx)));
    ui.frame();
    assert_eq!(ui.labels(), ["berry.png", "beta.md"]);

    // The other pane goes to a folder that matches this one: nothing differs.
    ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.focus_pane(1, cx);
            e.navigate("C:\\twin".to_string(), NavOptions::default(), cx);
        })
    });
    ui.frame();
    ui.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.focus_pane(0, cx)));
    for _ in 0..3 {
        ui.frame();
    }
    assert!(ui.labels().is_empty());
    ui.visual.update(|window, _| {
        let note = window.find_by_label("No differences in this pane");
        assert_eq!(note.description(), Some("All 4 items match the other pane."));
    });
    ui.visual
        .update(|window, cx| window.click("file-list-show-identical", cx));
    ui.frame();
    assert_eq!(ui.labels().len(), 4);
    assert!(!explorer.read_with(&ui.visual, |e, _| e.diff_only));
}
