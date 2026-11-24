//! The list's marks: a label's wash, the note mark after a name, the dots, and the Label, Tags and Comment columns
//! (docs/gpui/marks, card E1b).

use super::super::cells::{chips_fit, describe_marked, first_line, row_marks};
use super::*;
use crate::components::row_facts::{self, fake, MediaCells};
use crate::dialogs::tags::{self};
use crate::{state, theme};
use glint_model::marks::Marks;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A marks backend that answers once it is let: until then every row's marks are unread.
struct Gate {
    inner: Arc<fake::MarksBackend>,
    open: Arc<AtomicBool>,
}

impl tags::Backend for Gate {
    fn read(&self, paths: Vec<String>) -> tags::Fut<Vec<Vec<String>>> {
        self.inner.read(paths)
    }

    fn set(&self, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> tags::Fut<Vec<(String, String)>> {
        self.inner.set(paths, add, remove)
    }

    fn read_marks(&self, paths: Vec<String>) -> tags::Fut<Vec<Marks>> {
        let (inner, open) = (self.inner.clone(), self.open.clone());
        Box::pin(async move {
            std::future::poll_fn(|task| {
                if open.load(Ordering::SeqCst) {
                    std::task::Poll::Ready(())
                } else {
                    task.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            })
            .await;
            inner.read_marks(paths).await
        })
    }
}

fn marks(label: Option<&str>, tags: &[&str], comment: Option<&str>) -> Marks {
    Marks {
        label: label.map(str::to_string),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        comment: comment.map(str::to_string),
    }
}

/// [`open`] with the marks read from a map; the reads wait until the returned flag is set.
fn open_marked(cx: &mut TestAppContext, disk: &FakeDisk, path: &str, items: &[(&str, Marks)]) -> (Ui, Arc<AtomicBool>) {
    let open_gate = Arc::new(AtomicBool::new(false));
    let backend: Arc<dyn tags::Backend> = Arc::new(Gate {
        inner: fake::MarksBackend::with(items),
        open: open_gate.clone(),
    });
    let media: Arc<dyn row_facts::MediaSource> = fake::Media::with(vec![]);
    cx.update(|cx| {
        tags::init_with(cx, backend);
        row_facts::init_with(cx, media);
    });
    (open(cx, disk, path), open_gate)
}

/// Frames until `done`, giving core's threads time between them.
fn until(ui: &mut Ui, what: &str, mut done: impl FnMut(&mut Ui) -> bool) {
    for _ in 0..400 {
        ui.frame();
        if done(ui) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the list never reached: {what}");
}

fn has(ui: &mut Ui, id: (&'static str, usize)) -> bool {
    ui.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
}

fn set_columns(ui: &mut Ui, columns: Vec<ColumnId>) {
    let explorer = ui.explorer.clone();
    ui.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_columns(columns, cx)));
    ui.frame();
}

/// Row 1 of [`small_disk`] is alpha.txt, 2 is berry.png, 3 is beta.md (by name); row 0 is the folder.
const ALPHA: &str = "C:\\x\\alpha.txt";
const BETA: &str = "C:\\x\\beta.md";
const BERRY: &str = "C:\\x\\berry.png";

#[gpui_kit::test]
fn a_labeled_row_has_a_wash_and_an_unlabeled_or_unread_one_has_none(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, gate) = open_marked(
        cx,
        &small_disk(),
        "C:\\x",
        &[(ALPHA, marks(Some("Red"), &[], None)), (BETA, marks(None, &["Work"], None))],
    );
    // Nothing is read yet (M4): no row has a wash, not even a faint one.
    ui.frame();
    assert!((0..4).all(|i| !has(&mut ui, ("file-row-wash", i))));
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the wash", |ui| has(ui, ("file-row-wash", 1usize)));
    assert!(!has(&mut ui, ("file-row-wash", 2usize)), "tags alone wash nothing");
    assert!(!has(&mut ui, ("file-row-wash", 0usize)) && !has(&mut ui, ("file-row-wash", 3usize)));
}

#[gpui_kit::test]
fn a_labeled_row_is_washed_inside_its_line_and_once_selected_shows_the_selection_alone(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, gate) = open_marked(cx, &small_disk(), "C:\\x", &[(ALPHA, marks(Some("Red"), &[], None))]);
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the wash", |ui| has(ui, ("file-row-wash", 1usize)));
    ui.visual.update(|window, cx| {
        window.render_frame(cx);
        let row = window.find(("file-row", 1usize));
        let wash = window.find(("file-row-wash", 1usize));
        // Inside the row's line, with its inset and a pixel of air above and below, as the selection's fill is;
        // it is a child of the line, drawn before the fill that holds the states.
        assert!(wash.path().starts_with(row.path()));
        let (line, fill) = (row.bounds(), wash.bounds());
        assert_eq!(fill.origin.x - line.origin.x, super::super::cells::ROW_INSET);
        assert_eq!(fill.origin.y - line.origin.y, px(1.));
        assert_eq!(line.size.width - fill.size.width, super::super::cells::ROW_INSET * 2.);
        assert_eq!(line.size.height - fill.size.height, px(2.));
        // Its color is the label's own wash, never the accent.
        let looks = row_marks(&marks(Some("Red"), &[], None), &state::settings(cx).labels, cx);
        let wash = looks.label.map(|label| label.wash);
        assert_eq!(wash, Some(theme::label_wash(0xef4444, cx)));
        let t = theme::tokens(cx);
        assert_ne!(wash, Some(t.primary));
        assert_ne!(wash, Some(t.selected));
    });
    // Selected, the row has the selection's fill alone: the wash, stronger than that tint, would read as the
    // state (M11). The label is still in its column, its pill and the preview.
    ui.click(1);
    assert_eq!(ui.selected(), ["alpha.txt"]);
    ui.visual.update(|window, cx| {
        window.render_frame(cx);
        assert_eq!(window.find(("file-row", 1usize)).selected(), Some(true));
        assert!(window.try_find(("file-row-wash", 1usize)).is_none());
    });
}

#[gpui_kit::test]
fn a_row_with_a_comment_has_the_note_mark_named_comment_and_the_comment_is_what_it_says(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, gate) = open_marked(
        cx,
        &small_disk(),
        "C:\\x",
        &[(ALPHA, marks(None, &[], Some("first\nsecond")))],
    );
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the note", |ui| has(ui, ("file-list-note", 1usize)));
    ui.visual.update(|window, _| {
        let note = window.find(("file-list-note", 1usize));
        assert_eq!(note.label(), Some("Comment"));
        assert_eq!(note.description(), Some("first\nsecond"), "the whole comment, lines kept");
    });
    assert!(!has(&mut ui, ("file-list-note", 2usize)));
    assert!(!has(&mut ui, ("file-row-wash", 1usize)), "a comment alone washes nothing");
}

#[gpui_kit::test]
fn a_016_file_with_the_tags_work_and_red_is_washed_as_red_and_draws_one_dot(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, gate) = open_marked(cx, &small_disk(), "C:\\x", &[(ALPHA, marks(None, &["Work", "Red"], None))]);
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the wash", |ui| has(ui, ("file-row-wash", 1usize)));
    ui.visual.update(|window, _| {
        assert_eq!(window.find(("file-list-tags", 1usize)).label(), Some("Tags: Work"));
    });
}

#[gpui_kit::test]
fn the_label_tags_and_comment_columns_say_the_label_the_chips_that_fit_and_the_first_line(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let twelve: Vec<String> = (1..=12).map(|n| format!("tag{n:02}")).collect();
    let twelve_refs: Vec<&str> = twelve.iter().map(String::as_str).collect();
    let (mut ui, gate) = open_marked(
        cx,
        &small_disk(),
        "C:\\x",
        &[
            (ALPHA, marks(Some("Red"), &[], Some("first\nsecond\nthird"))),
            (BETA, marks(None, &["Ab", "Cd", "Ef"], None)),
            (BERRY, marks(None, &twelve_refs, None)),
        ],
    );
    set_columns(&mut ui, vec![ColumnId::Label, ColumnId::Tags, ColumnId::Comment]);
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the cells", |ui| has(ui, ("file-list-label", 1usize)));
    let rem = ui.frame_of().box_px / 1.25;
    until(&mut ui, "the chips", |ui| has(ui, ("file-list-tags-cell", 2usize)));
    ui.visual.update(|window, _| {
        assert_eq!(window.find(("file-list-label", 1usize)).label(), Some("Red"));
        let comment = window.find(("file-list-comment", 1usize));
        assert_eq!(comment.label(), Some("first"));
        assert_eq!(comment.description(), Some("first\nsecond\nthird"));
        assert!(window.try_find(("file-list-label", 2usize)).is_none(), "no label, an empty cell");
        // Three short tags are three chips and nothing more.
        let chips = |row: usize| (0..16).filter(|&n| window.try_find(("file-list-tag-chip", row * 16 + n)).is_some()).count();
        assert_eq!(chips(3), 3);
        assert!(window.try_find(("file-list-tag-more", 3usize)).is_none());
        // Twelve are as many as fit, then "+N" for the rest.
        let width = window.find(("file-list-tags-cell", 2usize)).bounds().size.width;
        let fit = chips_fit(&twelve, f32::from(width), rem);
        assert!((1..12).contains(&fit), "{fit}");
        assert_eq!(chips(2), fit);
        let more = format!("+{}", 12 - fit);
        assert_eq!(window.find(("file-list-tag-more", 2usize)).label(), Some(more.as_str()));
        let tags = window.find(("file-list-tags-cell", 2usize));
        assert!(tags.label().is_some_and(|label| label.starts_with("Tags: tag01, tag02")));
    });
}

#[test]
fn chips_fit_counts_what_fits_and_leaves_room_for_the_rest() {
    let tags = |n: usize| (1..=n).map(|i| format!("tag{i:02}")).collect::<Vec<_>>();
    assert_eq!(chips_fit(&[], 144., 16.), 0);
    assert_eq!(chips_fit(&tags(1), 144., 16.), 1);
    assert_eq!(chips_fit(&tags(2), 144., 16.), 2);
    let fit = chips_fit(&tags(12), 144., 16.);
    assert!((1..12).contains(&fit), "{fit}");
    assert_eq!(chips_fit(&tags(12), 1., 16.), 1, "at least one, cut by the cell");
    assert_eq!(chips_fit(&tags(12), 10_000., 16.), 12);
    assert_eq!(chips_fit(&tags(40), 10_000., 16.), 16, "never more than the cap");
    assert_eq!(first_line("\n\n  \nsecond\nthird"), "second");
    assert_eq!(first_line("\r\nonly"), "only");
}

#[gpui_kit::test]
fn the_dots_leave_the_names_cell_while_the_tags_column_shows_and_come_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, gate) = open_marked(cx, &small_disk(), "C:\\x", &[(ALPHA, marks(None, &["Work"], None))]);
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the dots", |ui| has(ui, ("file-list-tags", 1usize)));
    set_columns(&mut ui, vec![ColumnId::Modified, ColumnId::Tags]);
    assert!(!has(&mut ui, ("file-list-tags", 1usize)), "the Tags column says them");
    assert!(has(&mut ui, ("file-list-tags-cell", 1usize)));
    set_columns(&mut ui, vec![ColumnId::Modified]);
    assert!(has(&mut ui, ("file-list-tags", 1usize)));
    assert!(!has(&mut ui, ("file-list-tags-cell", 1usize)));
}

#[gpui_kit::test]
fn a_row_is_described_with_its_label_tags_and_comment_and_nothing_more_without_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut ui, gate) = open_marked(
        cx,
        &small_disk(),
        "C:\\x",
        &[(ALPHA, marks(Some("Red"), &["Work", "Home"], Some("first\nsecond")))],
    );
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the wash", |ui| has(ui, ("file-row-wash", 1usize)));
    ui.visual.update(|window, _| {
        let said = window.find(("file-row", 1usize)).description().unwrap_or_default().to_string();
        assert!(said.ends_with(", label Red, tags Work, Home, comment: first"), "{said}");
        let plain = window.find(("file-row", 2usize)).description().unwrap_or_default().to_string();
        assert!(!plain.contains("label") && !plain.contains("tags") && !plain.contains("comment"), "{plain}");
    });
    let txt = file("C:\\x", "a.txt", 1, 0);
    let words = ui.visual.update(|_, cx| {
        let marked = row_marks(&marks(None, &[], None), &[], cx);
        describe_marked(&txt, "1 B", "", &MediaCells::default(), Some(&marked))
    });
    assert_eq!(words, "TXT file, 1 B");
}

#[gpui_kit::test]
fn in_a_mirrored_layout_the_note_is_at_the_names_end_the_pill_dot_at_its_start_and_nothing_overlaps(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder("C:\\ملفات", vec![file("C:\\ملفات", "تقرير.txt", 5, 0)]);
    let (mut ui, gate) = open_marked(
        cx,
        &disk,
        "C:\\ملفات",
        &[(
            "C:\\ملفات\\تقرير.txt",
            marks(Some("Red"), &["مهم"], Some("ملاحظة مهمة\nسطر ثان")),
        )],
    );
    set_columns(&mut ui, vec![ColumnId::Label, ColumnId::Tags, ColumnId::Comment]);
    gate.store(true, Ordering::SeqCst);
    until(&mut ui, "the cells", |ui| {
        has(ui, ("file-list-note", 0usize)) && has(ui, ("file-list-comment", 0usize))
    });
    let mut places = Vec::new();
    for rtl in [false, true] {
        testing::set_rtl(rtl);
        ui.list.update(&mut ui.visual, |_, cx| cx.notify());
        ui.frame();
        places.push(ui.visual.update(|window, _| {
            let note = window.find(("file-list-note", 0usize)).bounds();
            let label = window.find(("file-list-label", 0usize)).bounds();
            let pill = window.find(("file-list-label-pill", 0usize)).bounds();
            let dot = window.find(("file-list-label-dot", 0usize)).bounds();
            let tags = window.find(("file-list-tags-cell", 0usize)).bounds();
            let comment = window.find(("file-list-comment", 0usize)).bounds();
            let apart = |a: Bounds<Pixels>, b: Bounds<Pixels>| a.right() <= b.left() || b.right() <= a.left();
            let all = [note, label, tags, comment];
            for (n, a) in all.iter().enumerate() {
                for b in &all[n + 1..] {
                    assert!(apart(*a, *b), "{a:?} overlaps {b:?}");
                }
            }
            // The name's column is first in reading order: the note is nearer to the columns than the row's
            // own start, and the dot is at the pill's start.
            (note.center().x < label.center().x, dot.center().x < pill.center().x)
        }));
    }
    testing::set_rtl(false);
    assert_eq!(places[0], (true, true), "left to right");
    assert_eq!(places[1], (false, false), "right to left, all of it mirrored");
}
