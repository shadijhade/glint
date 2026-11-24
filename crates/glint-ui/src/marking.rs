//! Marking what is selected, from the keys, the menus and the palette (docs/gpui/marks, F6, F7, F9, F10): each of
//! these works out which items and which change, and hands it to `dialogs::tags::set_marks`, which remembers Undo
//! and says `TagsChanged`. Nothing here reads the disk: marks come from what `RowFacts` holds.
//!
//! The keys act on the active tab's selection, read as the workspace's other handlers read it: the rows that are
//! selected, in the order the rows show them. With no selection (or no explorer) they do nothing, quietly.

use crate::components::row_facts;
use crate::dialogs::{comment, tags};
use crate::explorer;
use crate::kit::prelude::*;
use crate::{state, t};
use glint_model::marks::{clip_marks, clip_text, label_of, shown, Change, Label, Marks};
use glint_model::paths::base_name;

/// The active tab's selection, in the order the rows show it (as `Workspace::selection`).
fn selection(cx: &App) -> Vec<String> {
    let Some(explorer) = explorer::try_explorer(cx) else {
        return Vec::new();
    };
    let explorer = explorer.read(cx);
    let Some(tab) = explorer.active_tab() else {
        return Vec::new();
    };
    tab.rows
        .iter()
        .filter(|entry| tab.selection.contains(&entry.path))
        .map(|entry| entry.path.clone())
        .collect()
}

/// The labels of this PC.
fn labels(cx: &App) -> Vec<Label> {
    state::settings(cx).labels.clone()
}

/// What each of `paths` has now, as read for the rows; an item not read counts as having no marks.
fn marks_of(paths: &[String], cx: &App) -> Vec<Marks> {
    let known = row_facts::try_shared(cx)
        .map(|facts| facts.read(cx).marks_among(paths.iter().map(String::as_str)))
        .unwrap_or_default();
    paths
        .iter()
        .map(|path| known.get(&path.to_lowercase()).cloned().unwrap_or_default())
        .collect()
}

/// `Alt+1`…`Alt+9`: gives the selection of the pane the keyboard is in the label at `index` of the labels; when
/// every selected item has it already, takes it off.
pub fn label_selection(index: usize, window: &mut Window, cx: &mut App) {
    let paths = selection(cx);
    let Some(label) = labels(cx).get(index).map(|l| l.name.clone()) else {
        return;
    };
    if !paths.is_empty() {
        set_label(paths, Some(label), window, cx);
    }
}

/// `Alt+0`: takes any label off the selection.
pub fn clear_label(window: &mut Window, cx: &mut App) {
    let paths = selection(cx);
    if !paths.is_empty() {
        set_label(paths, None, window, cx);
    }
}

/// `Alt+T`: Edit tags… for the selection.
pub fn edit_tags(window: &mut Window, cx: &mut App) {
    let paths = selection(cx);
    if !paths.is_empty() {
        tags::open(window, cx, paths);
    }
}

/// `Alt+N`: Edit comment… for the selection.
pub fn edit_comment(window: &mut Window, cx: &mut App) {
    let paths = selection(cx);
    if !paths.is_empty() {
        comment::open(window, cx, paths);
    }
}

/// A label pressed in a menu or chosen in the palette (`MenuEvent::SetLabel`): `paths` get it, or lose it when
/// `label` is None or they all have it already.
pub fn set_label(paths: Vec<String>, label: Option<String>, window: &mut Window, cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    let labels = labels(cx);
    let marks = marks_of(&paths, cx);
    match label {
        Some(name) if !marks.iter().all(|m| shown(&labels, m).label.as_deref() == Some(name.as_str())) => {
            tags::set_marks(window, cx, paths, Change::Label(Some(name)));
        }
        _ => take_label_off(paths, &marks, &labels, window, cx),
    }
}

/// Takes the label off `paths`. An item whose "label" is a 0.16 color tag has no label stream: `Label(None)` would
/// change nothing, so that tag goes instead (one change for each such tag).
pub(crate) fn take_label_off(
    paths: Vec<String>,
    marks: &[Marks],
    labels: &[Label],
    window: &mut Window,
    cx: &mut App,
) {
    let mut plain = Vec::new();
    let mut by_tag: Vec<(String, Vec<String>)> = Vec::new();
    for (path, marks) in paths.into_iter().zip(marks) {
        let tag = match marks.label {
            None => marks.tags.iter().find(|tag| label_of(labels, tag).is_some()),
            Some(_) => None,
        };
        match tag {
            None => plain.push(path),
            Some(tag) => match by_tag.iter_mut().find(|(known, _)| known.to_lowercase() == tag.to_lowercase()) {
                Some((_, group)) => group.push(path),
                None => by_tag.push((tag.clone(), vec![path])),
            },
        }
    }
    if !plain.is_empty() {
        tags::set_marks(window, cx, plain, Change::Label(None));
    }
    for (tag, group) in by_tag {
        tags::set_marks(window, cx, group, Change::Tags { add: Vec::new(), remove: vec![tag] });
    }
}

/// Copy marks (`MenuEvent::CopyMarks`): the item's marks to the clipboard, as `marks::clip_text` writes them.
pub fn copy_marks(path: &str, window: &mut Window, cx: &mut App) {
    let marks = marks_of(&[path.to_string()], cx).pop().unwrap_or_default();
    if marks.is_empty() {
        let name = isolate(base_name(path).to_string());
        window.push_notification(Notification::info(t!("“{name}” has no marks to copy", name = name)), cx);
        return;
    }
    cx.write_to_clipboard(ClipboardItem::new_string(clip_text(base_name(path), &marks)));
}

/// Paste marks (`MenuEvent::PasteMarks`): the clipboard's marks to `paths`: tags are added, a label or a comment
/// replaces. One change, so one Undo step.
pub fn paste_marks(paths: Vec<String>, window: &mut Window, cx: &mut App) {
    let marks = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .and_then(|text| clip_marks(&text));
    let Some(marks) = marks else {
        window.push_notification(
            Notification::info(t!("Copy an item's marks first (More → Marks → Copy marks)."))
                .title(t!("There are no marks on the clipboard")),
            cx,
        );
        return;
    };
    if paths.is_empty() {
        return;
    }
    tags::set_marks(window, cx, paths, Change::Paste(marks));
}

/// Clear marks (`MenuEvent::ClearMarks`).
pub fn clear_marks(paths: Vec<String>, window: &mut Window, cx: &mut App) {
    if !paths.is_empty() {
        tags::set_marks(window, cx, paths, Change::Clear);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::tags::{Backend, Fut};
    use crate::explorer::fake::{file, folder, FakeDisk};
    use crate::explorer::NavOptions;
    use crate::testing;
    use glint_model::marks::{default_labels, Home, Outcome};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    const DIR: &str = "C:\\a";
    const A: &str = "C:\\a\\a.txt";
    const B: &str = "C:\\a\\b.txt";
    const C: &str = "C:\\a\\c.txt";
    const ARABIC: &str = "C:\\a\\ملف.txt";

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn marked(label: Option<&str>, tags: &[&str], comment: Option<&str>) -> Marks {
        Marks { label: label.map(String::from), tags: strings(tags), comment: comment.map(String::from) }
    }

    /// A backend that holds marks by path, applies each change, and writes down what it was asked.
    #[derive(Default)]
    struct Fake {
        held: Mutex<HashMap<String, Marks>>,
        sets: Mutex<Vec<(Vec<String>, Change)>>,
    }

    impl Fake {
        fn with(items: &[(&str, Marks)]) -> Arc<Self> {
            let fake = Fake::default();
            if let Ok(mut held) = fake.held.lock() {
                for (path, marks) in items {
                    held.insert((*path).to_string(), marks.clone());
                }
            }
            Arc::new(fake)
        }

        fn asked(&self) -> Vec<(Vec<String>, Change)> {
            self.sets.lock().map(|s| s.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
            let held = self.held.lock().map(|m| m.clone()).unwrap_or_default();
            Box::pin(async move { Ok(paths.iter().map(|p| held.get(p).map(|m| m.tags.clone()).unwrap_or_default()).collect()) })
        }

        fn set(&self, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Fut<Vec<(String, String)>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
            let held = self.held.lock().map(|m| m.clone()).unwrap_or_default();
            Box::pin(async move { Ok(paths.iter().map(|p| held.get(p).cloned().unwrap_or_default()).collect()) })
        }

        fn set_marks(&self, paths: Vec<String>, change: Change) -> Fut<Vec<Outcome>> {
            if let Ok(mut sets) = self.sets.lock() {
                sets.push((paths.clone(), change.clone()));
            }
            let mut held = self.held.lock();
            let outcomes = paths
                .into_iter()
                .map(|path| {
                    let before = held.as_ref().ok().and_then(|m| m.get(&path).cloned()).unwrap_or_default();
                    let now = change.apply(&before);
                    if let Ok(held) = held.as_mut() {
                        held.insert(path.clone(), now.clone());
                    }
                    Outcome { path, before, after: Ok((now, Home::Stream)) }
                })
                .collect();
            Box::pin(async move { Ok(outcomes) })
        }
    }

    struct Rig {
        visual: VisualTestContext,
        _disk: FakeDisk,
    }

    /// A window on `fake`, with an explorer on a fake disk showing C:\a (or none) and the labels Glint starts with.
    fn rig(cx: &mut TestAppContext, fake: Arc<Fake>, with_explorer: bool) -> Rig {
        cx.update(testing::init);
        cx.update(|cx| {
            state::update(cx, |s| s.labels = default_labels());
            let backend: Arc<dyn Backend> = fake.clone();
            tags::init_with(cx, backend);
        });
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![folder("C:\\", "a")]);
        disk.folder(
            DIR,
            ["a.txt", "b.txt", "c.txt", "ملف.txt"].iter().map(|n| file(DIR, n, 1, 1)).collect(),
        );
        if with_explorer {
            cx.update(|cx| explorer::init_with(cx, disk.source()));
            let explorer = cx.update(|cx| explorer::explorer(cx));
            let task = explorer.update(cx, |e, cx| e.navigate_task(DIR, NavOptions::default(), cx));
            cx.foreground_executor().block_test(task);
            cx.run_until_parked();
        }
        let visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        Rig { visual, _disk: disk }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    impl Rig {
        fn select(&mut self, paths: &[&str]) {
            let explorer = self.visual.update(|_, cx| explorer::explorer(cx));
            explorer.update(&mut self.visual, |e, cx| e.set_selection(paths.iter().copied(), None, None, cx));
        }

        /// Reads the marks of `paths` into the rows' facts and waits until `done` holds of them.
        fn read_until(&mut self, paths: &[&str], done: impl Fn(&[Marks]) -> bool) {
            let paths = strings(paths);
            let facts = self.visual.update(|_, cx| row_facts::shared(cx));
            for _ in 0..400 {
                facts.update(&mut self.visual, |f, cx| f.ask_tags_of(DIR, paths.iter().map(String::as_str), cx));
                self.visual.run_until_parked();
                let now = self.visual.update(|_, cx| marks_of(&paths, cx));
                let known = facts.read_with(&self.visual, |f, _| f.marks_among(paths.iter().map(String::as_str)).len());
                if known == paths.len() && done(&now) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            panic!("the marks never reached the state the test waits for");
        }

        fn read(&mut self, paths: &[&str]) {
            self.read_until(paths, |_| true);
        }

        fn press(&mut self, press: impl FnOnce(&mut Window, &mut App)) {
            self.visual.update(|window, cx| press(window, cx));
            self.visual.run_until_parked();
        }

        fn notices(&mut self) -> usize {
            self.visual.update(|window, cx| window.notifications(cx).len())
        }

        fn clipboard(&mut self) -> Option<String> {
            self.visual.update(|_, cx| cx.read_from_clipboard().and_then(|i| i.text()))
        }
    }

    fn with_label(name: &str) -> impl Fn(&[Marks]) -> bool + '_ {
        move |marks| marks.iter().all(|m| m.label.as_deref() == Some(name))
    }

    #[gpui_kit::test]
    fn alt_1_on_three_items_sets_red_and_pressed_again_once_they_all_have_it_takes_it_off(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::with(&[]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.select(&[A, B, C]);
        rig.read(&[A, B, C]);
        rig.press(|w, cx| label_selection(0, w, cx));
        assert_eq!(fake.asked(), [(strings(&[A, B, C]), Change::Label(Some("Red".into())))]);
        rig.read_until(&[A, B, C], with_label("Red"));
        rig.press(|w, cx| label_selection(0, w, cx));
        assert_eq!(fake.asked()[1], (strings(&[A, B, C]), Change::Label(None)));
    }

    #[gpui_kit::test]
    fn with_one_of_three_already_red_alt_1_labels_all_three(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::with(&[(B, marked(Some("Red"), &[], None))]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.select(&[A, B, C]);
        rig.read(&[A, B, C]);
        rig.press(|w, cx| label_selection(0, w, cx));
        assert_eq!(fake.asked(), [(strings(&[A, B, C]), Change::Label(Some("Red".into())))]);
    }

    #[gpui_kit::test]
    fn a_renamed_label_is_set_by_its_new_name_an_index_past_the_labels_does_nothing_and_alt_0_clears(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let fake = Fake::with(&[]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.visual.update(|_, cx| state::update(cx, |s| s.labels[0].name = "Important".into()));
        rig.select(&[A]);
        rig.read(&[A]);
        rig.press(|w, cx| label_selection(0, w, cx));
        assert_eq!(fake.asked(), [(strings(&[A]), Change::Label(Some("Important".into())))]);
        rig.press(|w, cx| label_selection(8, w, cx));
        assert_eq!(fake.asked().len(), 1);
        rig.press(clear_label);
        assert_eq!(fake.asked()[1], (strings(&[A]), Change::Label(None)));
    }

    #[gpui_kit::test]
    fn pressing_red_on_a_016_file_takes_the_tag_red_off_and_leaves_work(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::with(&[(A, marked(None, &["Work", "Red"], None))]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.select(&[A]);
        rig.read(&[A]);
        rig.press(|w, cx| label_selection(0, w, cx));
        assert_eq!(
            fake.asked(),
            [(strings(&[A]), Change::Tags { add: Vec::new(), remove: strings(&["Red"]) })]
        );
        assert_eq!(fake.held.lock().map(|m| m[A].tags.clone()).unwrap_or_default(), ["Work"]);
    }

    #[gpui_kit::test]
    fn with_no_selection_or_no_explorer_the_four_keys_ask_for_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        for with_explorer in [true, false] {
            let fake = Fake::with(&[]);
            let mut rig = rig(cx, fake.clone(), with_explorer);
            rig.press(|w, cx| label_selection(0, w, cx));
            rig.press(clear_label);
            rig.press(edit_tags);
            rig.press(edit_comment);
            assert_eq!(fake.asked(), []);
            let dialog = rig.visual.update(|window, cx| window.has_active_dialog(cx));
            assert!(!dialog);
        }
    }

    #[gpui_kit::test]
    fn copy_marks_puts_the_sidecars_line_on_the_clipboard_and_an_item_with_none_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::with(&[(A, marked(Some("Red"), &["Work"], Some("hi")))]);
        let mut rig = rig(cx, fake, true);
        rig.read(&[A, B]);
        rig.press(|w, cx| copy_marks(A, w, cx));
        let text = rig.clipboard().unwrap_or_default();
        assert!(text.starts_with(r#"{"name":"a.txt","label":"Red""#), "{text}");
        assert_eq!(rig.notices(), 0);
        rig.visual.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("before".into())));
        rig.press(|w, cx| copy_marks(B, w, cx));
        assert_eq!(rig.clipboard().as_deref(), Some("before"));
        assert_eq!(rig.notices(), 1);
    }

    #[gpui_kit::test]
    fn paste_marks_adds_the_tags_sets_the_label_and_the_comment_on_both_items(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let marks = marked(Some("Red"), &["Work"], Some("hi"));
        let fake = Fake::with(&[]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.visual.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(clip_text("x.txt", &marks))));
        rig.press(|w, cx| paste_marks(strings(&[A, B]), w, cx));
        assert_eq!(fake.asked(), [(strings(&[A, B]), Change::Paste(marks.clone()))], "one change, one Undo step");
        // Marks with only tags paste only tags.
        rig.visual.update(|_, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(clip_text("x.txt", &marked(None, &["Only"], None))))
        });
        rig.press(|w, cx| paste_marks(strings(&[A]), w, cx));
        assert_eq!(fake.asked().len(), 2);
        assert_eq!(fake.asked()[1].1, Change::Paste(marked(None, &["Only"], None)));
    }

    #[gpui_kit::test]
    fn paste_marks_of_other_text_asks_for_nothing_and_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::with(&[]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.visual.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string("just words".into())));
        rig.press(|w, cx| paste_marks(strings(&[A]), w, cx));
        assert_eq!(fake.asked(), []);
        assert_eq!(rig.notices(), 1);
    }

    #[gpui_kit::test]
    fn clear_marks_asks_clear_and_the_edit_keys_open_their_dialogs_for_the_selection(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::with(&[]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.press(|w, cx| clear_marks(strings(&[A, B]), w, cx));
        assert_eq!(fake.asked(), [(strings(&[A, B]), Change::Clear)]);
        rig.select(&[A, B]);
        rig.press(edit_tags);
        let dialog = rig.visual.update(|window, cx| window.has_active_dialog(cx));
        assert!(dialog);
        rig.press(edit_comment);
    }

    #[gpui_kit::test]
    fn arabic_names_and_tags_go_through_copy_and_paste_unchanged(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let marks = marked(Some("Red"), &["عمل", "مهم"], Some("ملاحظة\nثانية"));
        let fake = Fake::with(&[(ARABIC, marks.clone())]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.read(&[ARABIC]);
        rig.press(|w, cx| copy_marks(ARABIC, w, cx));
        let text = rig.clipboard().unwrap_or_default();
        assert!(text.contains("ملف.txt"), "{text}");
        rig.press(|w, cx| paste_marks(strings(&[A]), w, cx));
        assert_eq!(fake.asked(), [(strings(&[A]), Change::Paste(marks))], "the marks as they were copied");
    }
}
