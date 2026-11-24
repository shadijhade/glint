//! Edit comment… (docs/gpui/marks, F3): a box of several lines for the comment of one item or of several.
//!
//! The marks are read through the tags hub's backend (so tests give the hub a fake) and saved through
//! [`tags::set_marks`] (Undo, [`tags::TagsChanged`], the words of a failure). `Enter` is a new line, `Ctrl+Enter` saves, `Escape`
//! leaves without saving.

use super::tags;
use crate::kit::prelude::*;
use crate::kit::{Textarea, TextareaState};
use crate::{core_bridge, t, theme};
use glint_model::format::{format_size, plural};
use glint_model::marks::{Change, COMMENT_CAP};
use glint_model::paths::base_name;

/// From this many bytes on, the box says how much of the 64 KB is used.
const SAY_USED_FROM: usize = 60 * 1024;

/// Where reading the items' comments stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Reading them.
    Loading,
    /// Read.
    Ready,
    /// Couldn't read them, with why. Saving still sets the comment.
    Failed(SharedString),
}

/// The comments of the items being edited, read from the files.
pub struct CommentEdit {
    paths: Vec<String>,
    /// The comment every item has, when they all have the same (empty for none).
    shared: String,
    /// Whether the items' comments differ.
    differ: bool,
    state: State,
}

impl CommentEdit {
    /// Reads the marks of `paths` through the tags hub's backend, on core's runtime.
    pub fn new(paths: Vec<String>, cx: &mut Context<Self>) -> Self {
        let backend = tags::hub(cx).read(cx).backend();
        let read = backend.read_marks(paths.clone());
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(marks) => {
                        let first = marks.first().and_then(|m| m.comment.clone());
                        this.differ = marks.iter().any(|m| m.comment != first);
                        this.shared = if this.differ { String::new() } else { first.unwrap_or_default() };
                        this.state = State::Ready;
                    }
                    Err(reason) => this.state = State::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            paths,
            shared: String::new(),
            differ: false,
            state: State::Loading,
        }
    }

    /// The items being edited.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Where reading stands.
    pub fn state(&self) -> &State {
        &self.state
    }
}

/// "Edit comment…": a box of several lines.
pub struct CommentDialog {
    model: Entity<CommentEdit>,
    comment: Entity<TextareaState>,
    title: SharedString,
    /// What the box held when it was filled from the files: the box is untouched while it still holds this.
    baseline: String,
    /// The box was filled from the files once; after that the person's typing is theirs.
    filled: bool,
    /// The box's placeholder says *Several comments* (the items' comments differ).
    several: bool,
    _subscriptions: Vec<Subscription>,
}

impl CommentDialog {
    /// A dialog titled `title` for the items `model` reads.
    pub fn new(
        model: Entity<CommentEdit>,
        title: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let comment = cx.new(|cx| TextareaState::new(window, cx).auto_grow(6, 12));
        let subscriptions = vec![
            cx.observe_in(&model, window, |this, _, window, cx| {
                this.fill(window, cx);
                cx.notify();
            }),
            cx.observe(&comment, |_, _, cx| cx.notify()),
            // Ctrl+Enter saves. The box has put a line break in before it says so: that one is not the person's.
            cx.subscribe_in(&comment, window, |this, comment, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { secondary: true, .. } = event {
                    let (text, at) = (comment.read(cx).value().to_string(), comment.read(cx).cursor());
                    let cleaned = text
                        .get(..at)
                        .and_then(|before| before.strip_suffix('\n'))
                        .map(|before| format!("{before}{}", &text[at..]));
                    let text = cleaned.unwrap_or(text);
                    this.save_text(text, window, cx);
                }
            }),
        ];
        let mut this = Self {
            model,
            comment,
            title,
            baseline: String::new(),
            filled: false,
            several: false,
            _subscriptions: subscriptions,
        };
        this.fill(window, cx);
        this
    }

    /// Puts the comment the items share in the box once it is read.
    fn fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        if self.filled || *model.state() == State::Loading {
            return;
        }
        self.filled = true;
        self.several = model.differ;
        let (shared, differ) = (model.shared.clone(), model.differ);
        self.comment.update(cx, |comment, cx| {
            if !shared.is_empty() {
                comment.set_value(shared, window, cx);
            }
            if differ {
                comment.set_placeholder(t!("Several comments"), window, cx);
            }
        });
        self.baseline = self.comment.read(cx).value().to_string();
    }

    /// The box, so [`open`] can start typing at once.
    pub fn input(&self) -> &Entity<TextareaState> {
        &self.comment
    }

    /// Whether Save can be pressed: the comment is read (or couldn't be) and fits.
    fn can_save(&self, cx: &App) -> bool {
        *self.model.read(cx).state() != State::Loading && self.comment.read(cx).value().len() <= COMMENT_CAP
    }

    /// Saves: an empty box takes the comment off; a box left as it was changes nothing.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.comment.read(cx).value().to_string();
        self.save_text(text, window, cx);
    }

    /// [`save`] of `text`, which is what the box holds.
    fn save_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if *self.model.read(cx).state() == State::Loading || text.len() > COMMENT_CAP {
            return;
        }
        let paths = self.model.read(cx).paths().to_vec();
        window.close_dialog(cx);
        if text == self.baseline {
            return;
        }
        let change = if text.trim().is_empty() { None } else { Some(text) };
        tags::set_marks(window, cx, paths, Change::Comment(change));
    }
}

impl Render for CommentDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let state = self.model.read(cx).state().clone();
        let used = self.comment.read(cx).value().len();
        let can_save = self.can_save(cx);
        let said = if used > COMMENT_CAP {
            Some((t!("A comment can be 64 KB at most"), t.destructive, Role::Alert))
        } else if used >= SAY_USED_FROM {
            let used = format_size(u64::try_from(used).unwrap_or(u64::MAX));
            Some((t!("{used} of 64 KB", used = used), t.muted_foreground, Role::Status))
        } else {
            None
        };
        super::main_action(cx, can_save, |this, window, cx| this.save(window, cx));

        col()
            .id("comment-dialog")
            .role(Role::Group)
            .aria_label(self.title.clone())
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if key == "escape" {
                    window.close_dialog(cx);
                    cx.stop_propagation();
                } else if key == "enter" && event.keystroke.modifiers.control {
                    this.save(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(Textarea::new(&self.comment).aria_label(t!("Comment")))
            .when_some(
                match &state {
                    State::Failed(reason) => Some(reason.clone()),
                    _ => None,
                },
                |this, reason| {
                    let line = t!("Couldn't read the comment: {reason}", reason = reason);
                    this.child(
                        div()
                            .id("comment-error")
                            .role(Role::Alert)
                            .aria_label(line.clone())
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(t.destructive)
                            .child(line),
                    )
                },
            )
            .when_some(said, |this, (line, color, role)| {
                this.child(
                    row_start().justify_end().child(
                        div()
                            .id("comment-size")
                            .role(role)
                            .aria_label(line.clone())
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(color)
                            .child(line),
                    ),
                )
            })
            .child(
                row_start()
                    .items_center()
                    .gap(space(2.))
                    // Enter is a new line here, so what saves is said where the eye goes for the buttons.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!("Ctrl+Enter saves")),
                    )
                    .child(
                        Button::new("comment-cancel")
                            .ghost()
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("comment-save")
                            .primary()
                            .label(t!("Save"))
                            .disabled(!can_save)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

/// The dialog's title: `Comment of “name”` for one item, `Comment of 3 items` for more.
pub fn title(paths: &[String]) -> SharedString {
    match paths {
        [one] => t!("Comment of “{name}”", name = isolate(base_name(one).to_string())),
        _ => t!("Comment of {n}", n = plural(paths.len(), "item")),
    }
}

/// Opens Edit comment… for `paths`, out of the first one's row or tile. Nothing opens for no paths.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let shown = title(&paths);
    let first = paths.first().cloned().unwrap_or_default();
    let model = cx.new(|cx| CommentEdit::new(paths, cx));
    let view = cx.new({
        let shown = shown.clone();
        |cx| CommentDialog::new(model, shown, window, cx)
    });
    let comment = view.read(cx).input().clone();
    super::show_out_of(window, cx, shown, px(520.), view, first);
    comment.update(cx, |comment, cx| comment.focus(window, cx));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogs::tags::{Backend, Fut};
    use crate::testing;
    use glint_model::marks::{Marks, Outcome};
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A backend with a comment per path, recording the changes it was asked for.
    struct Fake {
        comments: Vec<(String, Option<String>)>,
        read_fails: bool,
        sets: Mutex<Vec<(Vec<String>, Change)>>,
    }

    impl Fake {
        fn new(comments: &[(&str, Option<&str>)], read_fails: bool) -> Arc<Self> {
            Arc::new(Self {
                comments: comments
                    .iter()
                    .map(|(p, c)| ((*p).to_string(), c.map(String::from)))
                    .collect(),
                read_fails,
                sets: Mutex::new(Vec::new()),
            })
        }

        fn asked(&self) -> Vec<(Vec<String>, Change)> {
            self.sets.lock().map(|s| s.clone()).unwrap_or_default()
        }
    }

    impl Backend for Fake {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
            Box::pin(async move { Ok(paths.iter().map(|_| Vec::new()).collect()) })
        }

        fn set(&self, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Fut<Vec<(String, String)>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
            let fails = self.read_fails;
            let known = self.comments.clone();
            Box::pin(async move {
                if fails {
                    return Err("access denied".to_string());
                }
                Ok(paths
                    .iter()
                    .map(|p| Marks {
                        comment: known.iter().find(|(k, _)| k == p).and_then(|(_, c)| c.clone()),
                        ..Marks::default()
                    })
                    .collect())
            })
        }

        fn set_marks(&self, paths: Vec<String>, change: Change) -> Fut<Vec<Outcome>> {
            if let Ok(mut sets) = self.sets.lock() {
                sets.push((paths, change));
            }
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    fn settle(cx: &mut VisualTestContext, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the comment dialog never reached the state the test waits for");
    }

    /// A window with the dialog layer and the hub on `fake`, the dialog open on `paths` and read.
    fn opened(
        cx: &mut TestAppContext,
        fake: &Arc<Fake>,
        paths: &[&str],
    ) -> (VisualTestContext, Entity<CommentDialog>) {
        cx.update(testing::init);
        let backend: Arc<dyn Backend> = fake.clone();
        cx.update(|cx| tags::init_with(cx, backend));
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let paths = strings(paths);
        let view = visual.update(|window, cx| {
            let shown = title(&paths);
            let model = cx.new(|cx| CommentEdit::new(paths, cx));
            let view = cx.new(|cx| CommentDialog::new(model, shown.clone(), window, cx));
            crate::dialogs::show(window, cx, shown, px(520.), view.clone());
            let comment = view.read(cx).input().clone();
            comment.update(cx, |comment, cx| comment.focus(window, cx));
            view
        });
        settle(&mut visual, |cx| {
            view.read_with(cx, |view, cx| *view.model.read(cx).state() != State::Loading)
        });
        (visual, view)
    }

    fn text(cx: &mut VisualTestContext, view: &Entity<CommentDialog>) -> String {
        view.read_with(cx, |view, cx| view.comment.read(cx).value().to_string())
    }

    fn is_open(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| window.has_active_dialog(cx))
    }

    fn press(cx: &mut VisualTestContext, key: &str) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    }

    fn type_text(cx: &mut VisualTestContext, text: &str) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.press("ctrl-a", cx);
            window.input(text, cx);
        });
        cx.run_until_parked();
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    const A: &str = "C:\\a\\a.txt";
    const B: &str = "C:\\a\\b.txt";
    const C: &str = "C:\\a\\c.txt";

    #[gpui_kit::test]
    fn a_comment_of_three_lines_is_in_the_box_and_ctrl_enter_saves_the_edit_and_closes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, Some("one\ntwo\nthree"))], false);
        let (mut visual, view) = opened(cx, &fake, &[A]);
        assert_eq!(text(&mut visual, &view), "one\ntwo\nthree");
        type_text(&mut visual, "new words");
        press(&mut visual, "ctrl-enter");
        settle(&mut visual, |_| !fake.asked().is_empty());
        assert_eq!(fake.asked(), [(strings(&[A]), Change::Comment(Some("new words".into())))]);
        assert!(!is_open(&mut visual), "Save closes the dialog");
    }

    #[gpui_kit::test]
    fn enter_adds_a_line_and_does_not_save_and_escape_leaves_asking_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, None)], false);
        let (mut visual, view) = opened(cx, &fake, &[A]);
        type_text(&mut visual, "first");
        press(&mut visual, "enter");
        assert_eq!(text(&mut visual, &view), "first\n");
        assert!(is_open(&mut visual));
        assert!(fake.asked().is_empty());
        press(&mut visual, "escape");
        assert!(!is_open(&mut visual));
        std::thread::sleep(Duration::from_millis(20));
        visual.run_until_parked();
        assert!(fake.asked().is_empty());
    }

    #[gpui_kit::test]
    fn an_emptied_box_saved_takes_the_comment_off(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, Some("old"))], false);
        let (mut visual, _view) = opened(cx, &fake, &[A]);
        press(&mut visual, "ctrl-a");
        press(&mut visual, "backspace");
        press(&mut visual, "ctrl-enter");
        settle(&mut visual, |_| !fake.asked().is_empty());
        assert_eq!(fake.asked(), [(strings(&[A]), Change::Comment(None))]);
    }

    #[gpui_kit::test]
    fn three_items_with_one_comment_show_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, Some("same")), (B, Some("same")), (C, Some("same"))], false);
        let (mut visual, view) = opened(cx, &fake, &[A, B, C]);
        assert_eq!(text(&mut visual, &view), "same");
        assert!(has(&mut visual, "Comment of 3 items"));
    }

    #[gpui_kit::test]
    fn three_items_with_different_comments_show_several_comments_and_an_untouched_save_asks_nothing(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, Some("x")), (B, Some("y")), (C, None)], false);
        let (mut visual, view) = opened(cx, &fake, &[A, B, C]);
        assert_eq!(text(&mut visual, &view), "");
        assert!(view.read_with(&visual, |view, _| view.several), "the placeholder says Several comments");
        press(&mut visual, "ctrl-enter");
        assert!(!is_open(&mut visual), "an untouched save just closes");
        std::thread::sleep(Duration::from_millis(20));
        visual.run_until_parked();
        assert!(fake.asked().is_empty());
    }

    #[gpui_kit::test]
    fn a_comment_typed_over_three_different_ones_is_set_on_all_three(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, Some("x")), (B, Some("y")), (C, None)], false);
        let (mut visual, _view) = opened(cx, &fake, &[A, B, C]);
        type_text(&mut visual, "for all");
        press(&mut visual, "ctrl-enter");
        settle(&mut visual, |_| !fake.asked().is_empty());
        assert_eq!(fake.asked(), [(strings(&[A, B, C]), Change::Comment(Some("for all".into())))]);
    }

    #[gpui_kit::test]
    fn a_comment_over_64_kb_cannot_be_saved_and_says_so_and_at_64_kb_it_saves(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, None)], false);
        let (mut visual, view) = opened(cx, &fake, &[A]);
        let line = format!("{}\n", "x".repeat(1023));
        let big = line.repeat(64);
        assert_eq!(big.len(), COMMENT_CAP);
        let set = |visual: &mut VisualTestContext, text: String| {
            let comment = view.read_with(visual, |view, _| view.comment.clone());
            visual.update(|window, cx| comment.update(cx, |comment, cx| comment.set_value(text, window, cx)));
            visual.update(|window, cx| window.render_frame(cx));
        };
        set(&mut visual, format!("{big}y"));
        assert!(has(&mut visual, "A comment can be 64 KB at most"));
        visual.update(|window, _| assert_eq!(window.find("comment-save").disabled(), Some(true)));
        press(&mut visual, "ctrl-enter");
        assert!(is_open(&mut visual), "a refused save leaves the dialog as it is");
        assert!(fake.asked().is_empty());
        set(&mut visual, big.clone());
        assert!(has(&mut visual, "64 KB of 64 KB"));
        visual.update(|window, _| assert_ne!(window.find("comment-save").disabled(), Some(true)));
        press(&mut visual, "ctrl-enter");
        settle(&mut visual, |_| !fake.asked().is_empty());
        assert_eq!(fake.asked(), [(strings(&[A]), Change::Comment(Some(big)))]);
    }

    #[gpui_kit::test]
    fn a_read_that_fails_is_shown_and_a_save_still_sets_the_comment(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[], true);
        let (mut visual, _view) = opened(cx, &fake, &[A]);
        assert!(has(&mut visual, "Couldn't read the comment: access denied"));
        type_text(&mut visual, "still");
        press(&mut visual, "ctrl-enter");
        settle(&mut visual, |_| !fake.asked().is_empty());
        assert_eq!(fake.asked(), [(strings(&[A]), Change::Comment(Some("still".into())))]);
    }

    #[gpui_kit::test]
    fn the_box_and_the_buttons_have_names_and_an_arabic_comment_renders_mirrored_in_a_dialog_with_an_arabic_title(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let path = "C:\\ملفات\\تقرير.txt";
        let fake = Fake::new(&[(path, Some("سطر أول\nسطر ثان\nسطر ثالث"))], false);
        testing::set_rtl(true);
        let (mut visual, view) = opened(cx, &fake, &[path]);
        assert_eq!(text(&mut visual, &view), "سطر أول\nسطر ثان\nسطر ثالث");
        let shown = "Comment of “\u{2067}تقرير.txt\u{2069}”";
        assert_eq!(title(&strings(&[path])), shown);
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(window.try_find_by_label("Comment").and_then(|e| e.role()), Some(Role::MultilineTextInput));
            assert!(window.try_find_by_label(shown).is_some());
            // Both buttons are there (find panics for one that isn't).
            let _ = window.find("comment-save");
            let _ = window.find("comment-cancel");
        });
        testing::set_rtl(false);
    }
}
