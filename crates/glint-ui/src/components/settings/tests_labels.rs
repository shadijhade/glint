//! Tests of Settings → Files → Labels and Marks (docs/gpui/marks, card G1b): the section's rows, the edits through
//! the page, the switch, taking back the last import through a fake source, search, the keyboard and Arabic.

use super::labels::{self, Source, PALETTE};
use super::tests::*;
use super::*;
use crate::dialogs::tags::{self, TagsChanged};
use crate::testing;
use glint_core::marks::LastImport;
use glint_model::marks::{default_labels, Label, MAX_LABELS};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::{Arc, Mutex};

/// A source with a canned last import, remembering what it was asked.
struct FakeSource {
    last: Mutex<Option<LastImport>>,
    asked: Mutex<u32>,
    taken: Mutex<Vec<String>>,
    paths: Vec<String>,
}

impl FakeSource {
    fn new(last: Option<LastImport>) -> Arc<Self> {
        Arc::new(Self {
            last: Mutex::new(last),
            asked: Mutex::new(0),
            taken: Mutex::new(Vec::new()),
            paths: vec!["C:\\a.txt".into(), "C:\\b.txt".into()],
        })
    }
}

impl Source for FakeSource {
    fn last_import(&self) -> backend::Fut<Option<LastImport>> {
        *lock(&self.asked) += 1;
        ready(Ok(lock(&self.last).clone()))
    }

    fn take_back(&self, journal: String) -> backend::Fut<Vec<String>> {
        lock(&self.taken).push(journal);
        *lock(&self.last) = None;
        ready(Ok(self.paths.clone()))
    }
}

fn an_import() -> LastImport {
    LastImport {
        journal: "j1".into(),
        source: "C:\\Users\\me\\tag.dat".into(),
        at: 1_700_000_000_000,
        items: 3,
        ..LastImport::default()
    }
}

/// Settings on the Files page, over a fake source.
fn open_files(
    cx: &mut TestAppContext,
    last: Option<LastImport>,
) -> (VisualTestContext, Entity<SettingsView>, Arc<FakeSource>) {
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let source = FakeSource::new(last);
    let given = source.clone();
    visual.update(|_, cx| labels::set_source(cx, given));
    // Focus events (and so blur) only reach an active window.
    visual.update(|window, _| window.activate_window());
    go_to(&mut visual, "files");
    settle(&mut visual, |_| true);
    (visual, view, source)
}

fn labels_now(cx: &mut VisualTestContext) -> Vec<Label> {
    settings(cx).labels
}

fn names_now(cx: &mut VisualTestContext) -> Vec<String> {
    labels_now(cx).into_iter().map(|l| l.name).collect()
}

fn name_box(cx: &mut VisualTestContext, i: usize) -> Entity<InputState> {
    cx.update(|_, cx| labels::open_panels(cx).and_then(|(rows, _)| rows.read(cx).input(i)).expect("a name box"))
}

fn press(cx: &mut VisualTestContext, key: &str) {
    cx.update(|window, cx| window.press(key, cx));
    cx.run_until_parked();
}

fn found(cx: &mut VisualTestContext, id: &str) -> bool {
    let id = SharedString::from(id.to_string());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
}

fn disabled(cx: &mut VisualTestContext, id: &str) -> Option<bool> {
    let id = SharedString::from(id.to_string());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).map(|e| e.disabled() == Some(true))
    })
}

/// Moves the keyboard focus to the page, which leaves whatever had it.
fn leave(cx: &mut VisualTestContext, view: &Entity<SettingsView>) {
    let focus = cx.update(|_, cx| view.read(cx).focus.clone());
    cx.update(|window, cx| {
        window.focus(&focus, cx);
        window.render_frame(cx);
    });
    cx.run_until_parked();
}

#[gpui_kit::test]
fn the_section_draws_a_row_a_label_in_order_each_with_its_name(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _) = open_files(cx, None);
    assert_eq!(visual.update(|_, cx| view.read(cx).sections().to_vec()), ["files", "labels", "marks"]);
    for (i, label) in default_labels().iter().enumerate() {
        let row = visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find(format!("settings-labelrow-{i}")).map(|e| (e.role(), e.label().map(str::to_string)))
        });
        assert_eq!(row, Some((Some(Role::Group), Some(label.name.clone()))), "row {i}");
        let input = name_box(&mut visual, i);
        assert_eq!(visual.update(|_, cx| input.read(cx).value().to_string()), label.name);
    }
    assert!(has(&mut visual, "Labels") && has(&mut visual, "Marks"));
}

#[gpui_kit::test]
fn a_name_typed_in_the_box_renames_on_enter_and_on_leaving_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _) = open_files(cx, None);
    reveal(&mut visual, &view, "settings-labelrow-0");
    let first = name_box(&mut visual, 0);
    type_into(&mut visual, &first, "Important");
    press(&mut visual, "enter");
    let labels = labels_now(&mut visual);
    assert_eq!((labels[0].name.as_str(), labels[0].was.clone()), ("Important", vec!["Red".to_string()]));
    reveal(&mut visual, &view, "settings-labelrow-1");
    let second = name_box(&mut visual, 1);
    type_into(&mut visual, &second, "Urgent");
    leave(&mut visual, &view);
    assert_eq!(names_now(&mut visual)[1], "Urgent", "leaving the box renames too");
    assert!(has(&mut visual, "Undo"), "a change says so, with Undo");
}

#[gpui_kit::test]
fn a_refusal_shows_its_words_under_the_box_and_changes_nothing_and_escape_puts_the_name_back(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let (mut visual, view, _) = open_files(cx, None);
    reveal(&mut visual, &view, "settings-labelrow-1");
    let second = name_box(&mut visual, 1);
    type_into(&mut visual, &second, "green");
    press(&mut visual, "enter");
    assert_eq!(
        role_of(&mut visual, "Another label is or was called that"),
        Some(Role::Alert)
    );
    assert_eq!(labels_now(&mut visual), default_labels(), "nothing changed");
    visual.update(|window, cx| {
        second.update(cx, |input, cx| input.focus(window, cx));
        window.press("ctrl-a", cx);
        window.press("backspace", cx);
    });
    press(&mut visual, "enter");
    assert!(has(&mut visual, "Give the label a name"));
    assert_eq!(labels_now(&mut visual), default_labels());
    // Escape: the box has its name back and the words go.
    type_into(&mut visual, &second, "Nope");
    press(&mut visual, "escape");
    assert_eq!(visual.update(|_, cx| second.read(cx).value().to_string()), "Orange");
    assert!(!has(&mut visual, "Give the label a name"));
    assert_eq!(labels_now(&mut visual), default_labels());
}

#[gpui_kit::test]
fn a_color_pressed_in_the_popup_recolors_that_label_alone(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _, _) = open_files(cx, None);
    assert!(!found(&mut visual, "settings-label-colors-0"));
    click(&mut visual, "settings-label-dot-0");
    for (k, color) in PALETTE.iter().enumerate() {
        let id = format!("settings-label-color-0-{k}");
        let (role, label) = visual.update(|window, cx| {
            window.render_frame(cx);
            let e = window.find(id.clone());
            (e.role(), e.label().map(str::to_string))
        });
        assert_eq!((role, label), (Some(Role::Button), Some(labels::color_name(*color).to_string())), "{id}");
    }
    click(&mut visual, "settings-label-color-0-9");
    let now = labels_now(&mut visual);
    assert_eq!(now[0].color, PALETTE[9]);
    assert_eq!(now[1..], default_labels()[1..], "no other label changed");
    assert!(!found(&mut visual, "settings-label-colors-0"), "the popup closes");
}

#[gpui_kit::test]
fn move_down_remove_and_undo_change_the_list_and_put_it_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _, _) = open_files(cx, None);
    assert_eq!(disabled(&mut visual, "settings-button-label-up-0"), Some(true));
    assert_eq!(disabled(&mut visual, "settings-button-label-down-6"), Some(true));
    assert_eq!(disabled(&mut visual, "settings-button-label-down-0"), Some(false));
    click(&mut visual, "settings-button-label-down-0");
    assert_eq!(names_now(&mut visual)[..2], ["Orange", "Red"]);
    assert!(has(&mut visual, "Undo"));
    // Pressing the toast's Undo never reaches its handler under the headless pointer (as in Clear history), so
    // what it does is driven here.
    visual.update(|_, cx| labels::restore(cx, default_labels()));
    assert_eq!(labels_now(&mut visual), default_labels());
    click(&mut visual, "settings-button-label-remove-2");
    assert_eq!(names_now(&mut visual), ["Red", "Orange", "Green", "Blue", "Purple", "Gray"]);
    assert!(has(&mut visual, "Undo"));
}

#[gpui_kit::test]
fn add_a_label_adds_one_is_off_at_32_and_back_to_the_seven_colors_resets(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _, _) = open_files(cx, None);
    click(&mut visual, "settings-button-label-add");
    let now = labels_now(&mut visual);
    assert_eq!((now.len(), now[7].name.as_str()), (8, "New label"));
    assert!(default_labels().iter().all(|l| l.color != now[7].color));
    visual.update(|_, cx| {
        state::update(cx, |s| {
            s.labels = (0..MAX_LABELS)
                .map(|i| Label { name: format!("L{i}"), color: PALETTE[i % 12], was: Vec::new() })
                .collect()
        })
    });
    frame(&mut visual);
    assert_eq!(disabled(&mut visual, "settings-button-label-add"), Some(true));
    assert!(found(&mut visual, "settings-label-full"));
    click(&mut visual, "settings-button-label-defaults");
    assert_eq!(labels_now(&mut visual), default_labels());
    assert!(!found(&mut visual, "settings-label-full"));
}

#[gpui_kit::test]
fn the_switch_turns_following_xyplorer_on_and_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _, _) = open_files(cx, None);
    assert_eq!(role_of(&mut visual, "Keep following XYplorer"), Some(Role::Switch));
    assert!(!settings(&mut visual).follow_xyplorer);
    click(&mut visual, "settings-switch-follow-xyplorer");
    assert!(settings(&mut visual).follow_xyplorer);
    click(&mut visual, "settings-switch-follow-xyplorer");
    assert!(!settings(&mut visual).follow_xyplorer);
}

#[gpui_kit::test]
fn the_last_import_row_says_its_numbers_and_take_back_asks_the_source_once_with_its_journal(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let (mut visual, _, source) = open_files(cx, Some(an_import()));
    settle(&mut visual, |cx| {
        cx.update(|_, cx| labels::open_panels(cx).is_some_and(|(_, m)| m.read(cx).said().is_some()))
    });
    let said = visual.update(|_, cx| labels::open_panels(cx).and_then(|(_, m)| m.read(cx).said())).unwrap();
    assert!(said.starts_with("3 from tag.dat, "), "{said}");
    assert_eq!(disabled(&mut visual, "settings-button-take-back-import"), Some(false));
    let told: Arc<Mutex<Vec<Vec<String>>>> = Default::default();
    let log = told.clone();
    visual.update(|_, cx| {
        let hub = tags::hub(cx);
        cx.subscribe(&hub, move |_, event: &TagsChanged, _| lock(&log).push(event.paths.clone())).detach();
    });
    click(&mut visual, "settings-button-take-back-import");
    settle(&mut visual, |_| !lock(&told).is_empty());
    assert_eq!(*lock(&source.taken), ["j1"]);
    assert_eq!(*lock(&told), std::slice::from_ref(&source.paths));
    // Asked again afterwards: nothing is left to take back.
    settle(&mut visual, |cx| disabled(cx, "settings-button-take-back-import") == Some(true));
    assert_eq!(*lock(&source.asked), 2);
}

#[gpui_kit::test]
fn with_no_import_the_row_says_so_and_the_button_is_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _, source) = open_files(cx, None);
    settle(&mut visual, |cx| {
        cx.update(|_, cx| labels::open_panels(cx).is_some_and(|(_, m)| m.read(cx).said().is_some()))
    });
    let said = visual.update(|_, cx| labels::open_panels(cx).and_then(|(_, m)| m.read(cx).said()));
    assert_eq!(said.as_deref(), Some("Nothing was imported in the last 30 days"));
    assert_eq!(disabled(&mut visual, "settings-button-take-back-import"), Some(true));
    click(&mut visual, "settings-button-take-back-import");
    assert!(lock(&source.taken).is_empty());
}

#[gpui_kit::test]
fn both_sections_are_found_by_search_and_the_keyboard_reaches_the_controls(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _) = open_files(cx, None);
    for (words, label) in [
        ("labels", "Labels"),
        ("add a", "Add a label"),
        ("xyplorer", "Keep following XYplorer"),
        ("take back", "Take back the last import"),
    ] {
        assert!(pages::find(words).iter().any(|e| e.label == label && e.page == Page::Files), "{words}");
    }
    // Tab leaves the first name box for the next control (Move up is off on the first row): Move down; Space
    // presses it (Enter does not reach a button under the headless keyboard; the dialog's own Enter is the main button).
    reveal(&mut visual, &view, "settings-labelrow-0");
    let first = name_box(&mut visual, 0);
    visual.update(|window, cx| first.update(cx, |input, cx| input.focus(window, cx)));
    press(&mut visual, "tab");
    press(&mut visual, "space");
    assert_eq!(names_now(&mut visual)[..2], ["Orange", "Red"], "Tab then Space moved the label down");
}

#[gpui_kit::test]
fn a_label_with_an_arabic_name_mirrors_and_nothing_overlaps(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, view, _) = open_files(cx, None);
    visual.update(|_, cx| state::update(cx, |s| s.labels[0].name = "مهم جدا".into()));
    testing::set_rtl(true);
    visual.update(|_, cx| cx.refresh_windows());
    visual.run_until_parked();
    reveal(&mut visual, &view, "settings-labelrow-0");
    let (row, dot, remove, down) = visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find("settings-labelrow-0").bounds(),
            window.find("settings-label-dot-0").bounds(),
            window.find("settings-button-label-remove-0").bounds(),
            window.find("settings-button-label-down-0").bounds(),
        )
    });
    assert!(dot.left() > remove.left(), "the dot starts on the right when mirrored: {dot:?} {remove:?}");
    assert!(dot.left() >= remove.right() && down.left() >= remove.right(), "buttons don't overlap: {dot:?} {down:?} {remove:?}");
    assert!(remove.left() >= row.left() && dot.right() <= row.right(), "inside the row");
    testing::set_rtl(false);
}

/// An import that renamed the first label to `name`, as the journal would keep it.
fn an_import_that_renamed_the_first_label(name: &str) -> (LastImport, Vec<Label>, Vec<Label>) {
    let before = default_labels();
    let mut made = before.clone();
    made[0].name = name.into();
    (LastImport { labels: Some(before.clone()), made: Some(made.clone()), ..an_import() }, before, made)
}

#[gpui_kit::test]
fn take_back_puts_the_labels_back_after_the_marks_only_while_they_are_what_the_import_made(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    // The labels are still what it made (an Arabic name, mirrored by nothing here: only kept whole): back they go.
    let (last, before, made) = an_import_that_renamed_the_first_label("مهم");
    let (mut visual, _, source) = open_files(cx, Some(last.clone()));
    visual.update(|_, cx| state::update(cx, |s| s.labels = made.clone()));
    settle(&mut visual, |cx| disabled(cx, "settings-button-take-back-import") == Some(false));
    click(&mut visual, "settings-button-take-back-import");
    settle(&mut visual, |_| !lock(&source.taken).is_empty());
    settle(&mut visual, |cx| disabled(cx, "settings-button-take-back-import") == Some(true));
    assert_eq!(*lock(&source.taken), ["j1"], "the marks went back through the source");
    assert_eq!(labels_now(&mut visual), before);
}

#[gpui_kit::test]
fn take_back_leaves_labels_changed_since_and_a_journal_without_labels_puts_back_the_marks_alone(
    cx: &mut TestAppContext,
) {
    let _guard = testing::lock();
    let (last, _, made) = an_import_that_renamed_the_first_label("مهم");
    let (mut visual, _, source) = open_files(cx, Some(last));
    let mut theirs = made;
    theirs[2].name = "Mine".into();
    visual.update(|_, cx| state::update(cx, |s| s.labels = theirs.clone()));
    settle(&mut visual, |cx| disabled(cx, "settings-button-take-back-import") == Some(false));
    click(&mut visual, "settings-button-take-back-import");
    settle(&mut visual, |cx| disabled(cx, "settings-button-take-back-import") == Some(true));
    assert_eq!(lock(&source.taken).len(), 1, "the marks went back");
    assert_eq!(labels_now(&mut visual), theirs, "their labels stay");

    // A journal written before the labels were kept: the marks alone.
    let (mut visual, _, source) = open_files(cx, Some(an_import()));
    let mut now = default_labels();
    now[0].name = "Renamed".into();
    visual.update(|_, cx| state::update(cx, |s| s.labels = now.clone()));
    settle(&mut visual, |cx| disabled(cx, "settings-button-take-back-import") == Some(false));
    click(&mut visual, "settings-button-take-back-import");
    settle(&mut visual, |cx| disabled(cx, "settings-button-take-back-import") == Some(true));
    assert_eq!(lock(&source.taken).len(), 1);
    assert_eq!(labels_now(&mut visual), now);
}
