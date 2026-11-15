//! Tests of Settings' rows for suggestions and learning. Card G2 of docs/gpui/aware writes these.

use super::tests::*;
use super::*;
use crate::testing;
use glint_model::aware::Shape;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use std::sync::Arc;

fn forget_is_disabled(cx: &mut gpui_kit::VisualTestContext, view: &Entity<SettingsView>) -> Option<bool> {
    reveal(cx, view, "settings-button-habits-forget");
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find("settings-button-habits-forget").disabled()
    })
}

#[gpui_kit::test]
fn the_history_page_has_both_switches_on_and_each_changes_its_setting(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    go_to(&mut visual, "history");
    assert!(has(&mut visual, "Suggest the next step"));
    assert!(has(&mut visual, "Learn from what I do"));
    assert!(settings(&mut visual).suggest && settings(&mut visual).learn, "both on by default");
    click(&mut visual, "settings-switch-suggest");
    assert!(!settings(&mut visual).suggest);
    assert!(settings(&mut visual).learn);
    click(&mut visual, "settings-switch-learn");
    assert!(!settings(&mut visual).learn);
}

#[gpui_kit::test]
fn the_forget_row_counts_what_is_kept_and_its_button_empties_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "history");
    assert!(has(&mut visual, "Nothing learned yet"));
    assert_eq!(forget_is_disabled(&mut visual, &view), Some(true));
    visual.update(|_, cx| {
        crate::aware::note_action(cx, Shape::Archive, "do:extract-here");
        crate::aware::note_action(cx, Shape::Picture, "do:rotate:right");
        crate::aware::note_place(cx, &["zip".to_string()], r"D:\Archives");
    });
    settle(&mut visual, |cx| has(cx, "3 habits learned"));
    // An enabled button reports no `disabled` at all.
    assert_ne!(forget_is_disabled(&mut visual, &view), Some(true));
    click(&mut visual, "settings-button-habits-forget");
    let kept = visual.update(|_, cx| persistence::app(cx).is_some_and(|app| app.habits.is_empty()));
    assert!(kept, "the saved habits are empty");
    settle(&mut visual, |cx| has(cx, "Nothing learned yet"));
    assert_eq!(forget_is_disabled(&mut visual, &view), Some(true));
}

#[gpui_kit::test]
fn searching_settings_for_habits_finds_the_history_rows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let search = visual.update(|_, cx| view.read(cx).search.clone());
    type_into(&mut visual, &search, "habits");
    for label in ["Learn from what I do", "Forget what Glint learned"] {
        assert!(
            pages::SEARCH
                .iter()
                .any(|entry| entry.label == label && entry.page == Page::History),
            "{label}"
        );
    }
    assert!(has(&mut visual, "History › Learn from what I do"));
    assert!(has(&mut visual, "History › Forget what Glint learned"));
}
