//! The Marks block at the head of the preview pane's details and Quick Look's (docs/gpui/marks, F8, F20): the
//! labels' dots, the tags as chips with a box to add one, the comment in a box that grows.
//!
//! A [MarksBlock] is made with the paths it is about (one item in an [EntryView](super::frame::EntryView), the
//! selection in the pane's deck). What it draws comes from memory: the marks the rows already hold
//! ([RowFacts::marks_among]), and for the paths they don't, one read through the tags hub's backend on core's
//! runtime. Until every path is known it draws nothing (M4). It reads again when the hub says [TagsChanged] for
//! one of its paths, and leaves a box the person is typing in alone.
//!
//! Every change goes through [set_marks], so it is one Undo step with its notice. Where the marks are kept (F20)
//! comes from a source the block takes ([init_home]; core's [home_of](glint_core::marks::home_of) on a blocking
//! thread in the app), so the UI thread never asks a volume.
//!
//! **Keys.** The block's context is [BOX_CONTEXT]; Space is switched off in its inputs (Quick Look closes on
//! Space, and a space typed in a comment is a space). Escape and Ctrl+Enter are taken in the capture phase, so
//! neither reaches Quick Look.

use crate::components::row_facts;
use crate::dialogs::tags::{self, set_marks, shared_tags, tag_color, TagsChanged};
use crate::kit::prelude::*;
use crate::kit::{Textarea, TextareaState};
use crate::{core_bridge, state, t, theme};
use glint_model::entries::{is_archived, is_recycled, is_remote, FileEntry};
use glint_model::marks::{shown, Change, Home, Marks, COMMENT_CAP};
use gpui_kit::{actions, Subscription, Task};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

actions!(
    glint_marks_block,
    [
        /// Ctrl+Enter in the comment box: save it now.
        SaveComment,
        /// Escape in either box: the comment goes back to what was there, or the tag box empties.
        PutBack
    ]
);

/// The key context of the block.
pub const BOX_CONTEXT: &str = "MarksBox";

/// Where an item's marks are kept: a path to its [Home], asked off the UI thread.
pub type HomeSource = Rc<dyn Fn(&str) -> Pin<Box<dyn Future<Output = Option<Home>>>>>;

struct ChosenHome(HomeSource);

impl Global for ChosenHome {}

/// Makes the blocks ask [source] where marks are kept (the app asks core; tests a fake).
pub fn init_home(cx: &mut App, source: HomeSource) {
    cx.set_global(ChosenHome(source));
}

/// Core's answer, on a blocking thread.
fn core_home() -> HomeSource {
    Rc::new(|path| {
        let path = path.to_string();
        let running = core_bridge::run(async move {
            glint_core::host::async_runtime::spawn_blocking(move || glint_core::marks::home_of(&path))
                .await
                .ok()
        });
        Box::pin(async move { running.await.ok().flatten() })
    })
}

/// Marks that the keys are bound in this app.
struct Bound;

impl Global for Bound {}

/// Binds the block's keys in its inputs, once per app. Their context is deeper than Quick Look's and the inputs' own,
/// so these win there: Space is switched off (Quick Look closes on it, and a space typed in a comment must stay a
/// space), and Ctrl+Enter and Escape are the block's (the comment's own Ctrl+Enter would add a line).
fn ensure_bound(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    cx.bind_keys([
        KeyBinding::new("space", NoAction, Some("MarksBox > Input")),
        KeyBinding::new("ctrl-enter", SaveComment, Some("MarksBox > Input")),
        KeyBinding::new("escape", PutBack, Some("MarksBox > Input")),
    ]);
}

/// Whether the block is shown for this item: not inside an archive, in the Recycle Bin or on another computer
/// (the rows don't read those either).
pub fn applies(entry: &FileEntry) -> bool {
    !is_archived(entry) && !is_recycled(entry) && !is_remote(entry)
}

/// What the items share, as the block draws it.
struct Facts {
    /// The label every item is shown with, by its name now.
    label: Option<String>,
    /// The tags every item has.
    tags: Vec<String>,
    /// The comment every item has, when they agree.
    comment: Option<String>,
    /// The comments differ.
    mixed: bool,
    /// Some item has a mark.
    marked: bool,
}

/// The Marks block: one item's, or the selection's.
pub struct MarksBlock {
    prefix: SharedString,
    paths: Vec<String>,
    /// The marks read, by path in lower case.
    known: HashMap<String, Marks>,
    home: Option<Home>,
    home_of: HomeSource,
    /// What the comment box was given: what Escape puts back and what "unchanged" means.
    baseline: String,
    /// The comment is over what a file can hold: said under the box, and not sent.
    too_big: bool,
    /// The comment box's placeholder: *Several comments* where the items differ.
    placeholder: SharedString,
    tag_box: Entity<InputState>,
    comment: Entity<TextareaState>,
    /// Where Escape leaves the keyboard: in the block, so it stays in the pane.
    focus: FocusHandle,
    _reading: Option<Task<()>>,
    _homing: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl MarksBlock {
    /// The block for [paths]; its ids start with [prefix] (so the pane's and Quick Look's can be on screen together).
    pub fn new(paths: Vec<String>, prefix: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        ensure_bound(cx);
        let tag_box = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Add a tag")));
        let comment = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6).placeholder(t!("Add a comment")));
        let hub = tags::hub(cx);
        let subscriptions = vec![
            cx.subscribe_in(&hub, window, |this, _, event: &TagsChanged, window, cx| {
                this.changed(&event.paths, window, cx)
            }),
            cx.subscribe_in(&tag_box, window, |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.add_typed(window, cx),
                InputEvent::Change if input.read(cx).value().contains(',') => this.add_typed(window, cx),
                _ => {}
            }),
            cx.subscribe_in(&comment, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Blur) {
                    this.save_comment(window, cx);
                }
            }),
        ];
        let home_of = cx
            .try_global::<ChosenHome>()
            .map_or_else(core_home, |chosen| chosen.0.clone());
        let mut block = MarksBlock {
            prefix: SharedString::from(prefix.to_string()),
            paths,
            known: HashMap::new(),
            home: None,
            home_of,
            baseline: String::new(),
            too_big: false,
            placeholder: t!("Add a comment"),
            tag_box,
            comment,
            focus: cx.focus_handle(),
            _reading: None,
            _homing: None,
            _subscriptions: subscriptions,
        };
        block.refresh(false, window, cx);
        block
    }

    // ---- Knowing ----------------------------------------------------------------------------------------

    /// Takes the marks the rows hold, and reads the rest (all of them when [force]: something changed).
    fn refresh(&mut self, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let held = if force {
            HashMap::new()
        } else {
            row_facts::try_shared(cx)
                .map(|facts| facts.read(cx).marks_among(self.paths.iter().map(String::as_str)))
                .unwrap_or_default()
        };
        let mut missing = Vec::new();
        for path in &self.paths {
            match held.get(&path.to_lowercase()) {
                Some(marks) => {
                    self.known.insert(path.to_lowercase(), marks.clone());
                }
                None => missing.push(path.clone()),
            }
        }
        if missing.is_empty() {
            self._reading = None;
            self.settled(window, cx);
            return;
        }
        let backend = tags::hub(cx).read(cx).backend();
        let call = backend.read_marks(missing.clone());
        self._reading = Some(cx.spawn_in(window, async move |this, cx| {
            let result = core_bridge::run(call).await.and_then(|inner| inner);
            this.update_in(cx, |this, window, cx| {
                if let Ok(list) = result {
                    for (path, marks) in missing.iter().zip(list) {
                        this.known.insert(path.to_lowercase(), marks);
                    }
                }
                this.settled(window, cx);
            })
            .ok();
        }));
    }

    /// The hub said marks changed on [paths]: read ours again if any is one of them.
    fn changed(&mut self, paths: &[String], window: &mut Window, cx: &mut Context<Self>) {
        let own: std::collections::HashSet<String> = self.paths.iter().map(|p| p.to_lowercase()).collect();
        if paths.iter().any(|p| own.contains(&p.to_lowercase())) {
            self.refresh(true, window, cx);
        }
    }

    /// What the items share, once all are known.
    fn facts(&self, cx: &App) -> Option<Facts> {
        let labels = &state::settings(cx).labels;
        let marks: Vec<&Marks> = self
            .paths
            .iter()
            .map(|p| self.known.get(&p.to_lowercase()))
            .collect::<Option<_>>()?;
        let first = marks.first()?;
        let drawn: Vec<_> = marks.iter().map(|m| shown(labels, m)).collect();
        let label = drawn[0].label.clone().filter(|name| {
            drawn
                .iter()
                .all(|d| d.label.as_deref().is_some_and(|other| other.eq_ignore_ascii_case(name)))
        });
        let tags = shared_tags(&drawn.iter().map(|d| d.tags.clone()).collect::<Vec<_>>());
        let mixed = marks.iter().any(|m| m.comment != first.comment);
        Some(Facts {
            label,
            tags,
            comment: if mixed { None } else { first.comment.clone() },
            mixed,
            marked: marks.iter().any(|m| !m.is_empty()),
        })
    }

    /// Marks are in: puts them in the boxes (not into the comment being typed), and asks where they are kept.
    fn settled(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(facts) = self.facts(cx) else { return };
        let placeholder = if facts.mixed {
            t!("Several comments")
        } else {
            t!("Add a comment")
        };
        self.placeholder = placeholder.clone();
        let text = facts.comment.clone().unwrap_or_default();
        let typing = self.comment.read(cx).focus_handle(cx).is_focused(window)
            && self.comment.read(cx).value().as_ref() != self.baseline;
        self.comment.update(cx, |comment, cx| {
            comment.set_placeholder(placeholder, window, cx);
            if !typing {
                comment.set_value(text.clone(), window, cx);
            }
        });
        if !typing {
            self.baseline = text;
            self.too_big = false;
        }
        // One ask for the first marked item, again after each change (ponytail: a selection of many says the
        // first one's home; ask each if that ever misleads).
        let first = self
            .paths
            .iter()
            .find(|p| self.known.get(&p.to_lowercase()).is_some_and(|m| !m.is_empty()));
        match first {
            None => self.home = None,
            Some(path) => {
                let ask = (self.home_of)(path);
                self._homing = Some(cx.spawn(async move |this, cx| {
                    let home = ask.await;
                    this.update(cx, |this, cx| {
                        this.home = home;
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
        cx.notify();
    }

    // ---- Doing ------------------------------------------------------------------------------------------

    fn press_label(&mut self, name: &str, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !on {
            set_marks(window, cx, self.paths.clone(), Change::Label(Some(name.to_string())));
            return;
        }
        // Off: a label that is a 0.16 color tag goes as the tag it is (`marking::take_label_off`).
        let marks: Vec<Marks> = self
            .paths
            .iter()
            .map(|p| self.known.get(&p.to_lowercase()).cloned().unwrap_or_default())
            .collect();
        let labels = state::settings(cx).labels.clone();
        crate::marking::take_label_off(self.paths.clone(), &marks, &labels, window, cx);
    }

    fn remove_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        let change = Change::Tags {
            add: Vec::new(),
            remove: vec![tag.to_string()],
        };
        set_marks(window, cx, self.paths.clone(), change);
    }

    /// Adds what is typed in the tag box (Enter, or a comma in it) and empties the box.
    fn add_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = tags::parse_typed(self.tag_box.read(cx).value().as_ref());
        self.tag_box.update(cx, |input, cx| input.set_value("", window, cx));
        let have = self.facts(cx).map(|facts| facts.tags).unwrap_or_default();
        let mut add: Vec<String> = Vec::new();
        for tag in typed {
            let lower = tag.to_lowercase();
            if !have.iter().chain(add.iter()).any(|t| t.to_lowercase() == lower) {
                add.push(tag);
            }
        }
        if !add.is_empty() {
            set_marks(window, cx, self.paths.clone(), Change::Tags { add, remove: Vec::new() });
        }
    }

    /// The focus left the comment, or Ctrl+Enter: saves it once when it changed; over 64 KB it says so and sends
    /// nothing.
    fn save_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.facts(cx).is_none() {
            return;
        }
        let text = self.comment.read(cx).value().to_string();
        if text == self.baseline {
            return;
        }
        if text.len() > COMMENT_CAP {
            self.too_big = true;
            cx.notify();
            return;
        }
        self.too_big = false;
        self.baseline = text.clone();
        let change = Change::Comment((!text.trim().is_empty()).then_some(text));
        set_marks(window, cx, self.paths.clone(), change);
    }

    fn comment_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.comment.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Escape: the comment goes back to what was there (or the tag box is emptied), and the keyboard stays in the
    /// block.
    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.comment_has_focus(window, cx) {
            let before = self.baseline.clone();
            self.comment.update(cx, |comment, cx| comment.set_value(before, window, cx));
            self.too_big = false;
        } else {
            self.tag_box.update(cx, |input, cx| input.set_value("", window, cx));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }
}

impl Render for MarksBlock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(facts) = self.facts(cx) else {
            return div().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let prefix = self.prefix.clone();
        let id = |part: &str| SharedString::from(format!("{prefix}-marks-{part}"));
        let labels = state::settings(cx).labels.clone();

        let dots = row_start()
            .id(id("labels"))
            .role(Role::Group)
            .aria_label(t!("Label"))
            .test_support()
            .flex_wrap()
            .gap(space(0.5))
            .children(labels.iter().enumerate().map(|(i, label)| {
                let on = facts
                    .label
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&label.name));
                let name = label.name.clone();
                let press = label.name.clone();
                let ring = if on { t.foreground } else { t.background.opacity(0.) };
                Button::new((id("label"), i))
                    .custom(
                        ButtonCustomVariant::new(cx)
                            .color(t.background.opacity(0.))
                            .foreground(t.foreground)
                            .hover(t.accent)
                            .active(t.accent),
                    )
                    .xsmall()
                    .compact()
                    .rounded_full()
                    .role(Role::Button)
                    .toggled(on)
                    .accessibility_label(name.clone())
                    .tooltip(SharedString::from(name))
                    .on_click(cx.listener(move |this, _, window, cx| this.press_label(&press, on, window, cx)))
                    .child(
                        div()
                            .size(space(3.5))
                            .rounded_full()
                            .bg(theme::label_tone(label.color))
                            .border_2()
                            .border_color(ring),
                    )
            }));

        let chips = facts.tags.iter().enumerate().map(|(i, tag)| {
            let remove = tag.clone();
            align_row(1.)
                .id((id("chip"), i))
                .role(Role::Group)
                .aria_label(SharedString::from(tag.clone()))
                .test_support()
                .items_center()
                .rounded_full()
                .border_1()
                .border_color(t.border)
                .ps(2.)
                .pe(0.5)
                .child(div().size(space(2.)).rounded_full().bg(tag_color(tag)))
                .child(isolate(tag.clone()))
                .child(
                    Button::new((id("remove"), i))
                        .ghost()
                        .compact()
                        .rounded_full()
                        .icon(IconName::X)
                        .accessibility_label(t!("Remove tag {tag}", tag = tag.clone()))
                        .on_click(cx.listener(move |this, _, window, cx| this.remove_tag(&remove, window, cx))),
                )
        });
        let tags = row_start()
            .id(id("tags"))
            .role(Role::Group)
            .aria_label(t!("Tags"))
            .test_support()
            .flex_wrap()
            .items_center()
            .gap(space(1.5))
            .children(chips)
            .child(
                div()
                    .flex_1()
                    .min_w(space(24.))
                    .child(Input::new(&self.tag_box).small().aria_label(t!("Add a tag"))),
            );

        let big = self.too_big.then(|| {
            let said = t!("A comment can be 64 KB at most");
            div()
                .id(id("too-big"))
                .role(Role::Alert)
                .aria_label(said.clone())
                .test_support()
                .text_color(t.destructive)
                .child(said)
        });
        let kept = facts
            .marked
            .then(|| match self.home {
                Some(Home::Beside) => Some(t!("Kept beside the file")),
                Some(Home::Ledger) => Some(t!("Kept on this PC")),
                _ => None,
            })
            .flatten()
            .map(|said| {
                div()
                    .id(id("kept"))
                    .role(Role::Status)
                    .aria_label(said.clone())
                    .test_support()
                    .text_color(t.muted_foreground)
                    .child(said)
            });

        col()
            .id(id("block"))
            .role(Role::Group)
            .aria_label(t!("Marks"))
            .test_support()
            .key_context(BOX_CONTEXT)
            .track_focus(&self.focus)
            .flex_none()
            .gap(space(2.))
            .border_t_1()
            .border_color(t.border)
            .px(space(4.))
            .py(space(3.))
            .text_size(theme::size::XS)
            .on_action(cx.listener(|this, _: &SaveComment, window, cx| {
                if this.comment_has_focus(window, cx) {
                    this.save_comment(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PutBack, window, cx| this.escape(window, cx)))
            .child(dots)
            .child(tags)
            .child(div().w_full().child(Textarea::new(&self.comment).aria_label(t!("Comment"))))
            .children(big)
            .children(kept)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::preview::quick_look;
    use crate::components::preview::tests::{setup as pane_setup, Env as PaneEnv};
    use crate::dialogs::tags::{Backend, Fut};
    use crate::testing;
    use glint_model::marks::Outcome;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::{Arc, Mutex};

    /// A marks backend over a map (by path in lower case): it logs what it is asked and applies each change.
    #[derive(Default)]
    struct Recorder {
        marks: Mutex<HashMap<String, Marks>>,
        reads: Mutex<Vec<Vec<String>>>,
        sets: Mutex<Vec<(Vec<String>, Change)>>,
        /// Reads never answer.
        stall: bool,
    }

    impl Recorder {
        fn with(items: &[(&str, Marks)], stall: bool) -> Arc<Self> {
            let fake = Recorder {
                stall,
                ..Recorder::default()
            };
            for (path, marks) in items {
                fake.put(path, marks.clone());
            }
            Arc::new(fake)
        }

        fn put(&self, path: &str, marks: Marks) {
            if let Ok(mut map) = self.marks.lock() {
                map.insert(path.to_lowercase(), marks);
            }
        }

        fn sets(&self) -> Vec<(Vec<String>, Change)> {
            self.sets.lock().map(|s| s.clone()).unwrap_or_default()
        }

        fn reads(&self) -> usize {
            self.reads.lock().map(|r| r.iter().map(Vec::len).sum()).unwrap_or_default()
        }
    }

    impl Backend for Recorder {
        fn read(&self, _: Vec<String>) -> Fut<Vec<Vec<String>>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn set(&self, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Fut<Vec<(String, String)>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
            if let Ok(mut reads) = self.reads.lock() {
                reads.push(paths.clone());
            }
            if self.stall {
                return Box::pin(std::future::pending());
            }
            let known = self.marks.lock().map(|m| m.clone()).unwrap_or_default();
            Box::pin(async move {
                Ok(paths
                    .iter()
                    .map(|p| known.get(&p.to_lowercase()).cloned().unwrap_or_default())
                    .collect())
            })
        }

        fn set_marks(&self, paths: Vec<String>, change: Change) -> Fut<Vec<Outcome>> {
            if let Ok(mut sets) = self.sets.lock() {
                sets.push((paths.clone(), change.clone()));
            }
            let mut out = Vec::new();
            if let Ok(mut map) = self.marks.lock() {
                for path in &paths {
                    let before = map.get(&path.to_lowercase()).cloned().unwrap_or_default();
                    let after = change.apply(&before);
                    map.insert(path.to_lowercase(), after.clone());
                    out.push(Outcome {
                        path: path.clone(),
                        before,
                        after: Ok((after, Home::Stream)),
                    });
                }
            }
            Box::pin(async move { Ok(out) })
        }
    }

    const A: &str = "C:\\x\\a.conf";
    const B: &str = "C:\\x\\b.conf";
    const C: &str = "C:\\x\\c.conf";

    fn marked(label: Option<&str>, tags: &[&str], comment: Option<&str>) -> Marks {
        Marks {
            label: label.map(str::to_string),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            comment: comment.map(str::to_string),
        }
    }

    struct Env {
        visual: VisualTestContext,
        block: Entity<MarksBlock>,
        rec: Arc<Recorder>,
    }

    impl Env {
        fn has(&mut self, label: &str) -> bool {
            self.visual.update(|window, cx| {
                window.render_frame(cx);
                window.try_find_by_label(label).is_some()
            })
        }

        fn click(&mut self, id: impl Into<ElementId>) {
            let id = id.into();
            self.visual.update(|window, cx| {
                window.render_frame(cx);
                window.click(id, cx);
            });
            self.visual.run_until_parked();
        }

        fn press(&mut self, key: &str) {
            self.visual.update(|window, cx| {
                window.render_frame(cx);
                window.press(key, cx);
            });
            self.visual.run_until_parked();
        }

        /// Focuses the comment box and types [text] after what is there.
        fn type_comment(&mut self, text: &str) {
            let comment = self.block.read_with(&self.visual, |b, _| b.comment.clone());
            self.visual.update(|window, cx| {
                comment.update(cx, |input, cx| input.focus(window, cx));
                window.render_frame(cx);
                window.press("ctrl-end", cx);
                window.input(text, cx);
            });
            self.visual.run_until_parked();
        }

        fn type_tag(&mut self, text: &str) {
            let tag_box = self.block.read_with(&self.visual, |b, _| b.tag_box.clone());
            self.visual.update(|window, cx| {
                tag_box.update(cx, |input, cx| input.focus(window, cx));
                window.render_frame(cx);
                window.input(text, cx);
            });
            self.visual.run_until_parked();
        }

        /// The focus leaves the boxes for the block (a click elsewhere in the pane).
        fn leave(&mut self) {
            let focus = self.block.read_with(&self.visual, |b, _| b.focus.clone());
            self.visual.update(|window, cx| {
                window.focus(&focus, cx);
                window.render_frame(cx);
            });
            self.visual.run_until_parked();
        }

        fn comment(&mut self) -> String {
            let comment = self.block.read_with(&self.visual, |b, _| b.comment.clone());
            comment.read_with(&self.visual, |c, _| c.value().to_string())
        }

        fn set_comment(&mut self, text: &str) {
            let comment = self.block.read_with(&self.visual, |b, _| b.comment.clone());
            let text = text.to_string();
            self.visual.update(|window, cx| {
                comment.update(cx, |input, cx| {
                    input.focus(window, cx);
                    input.set_value(text, window, cx);
                });
                window.render_frame(cx);
            });
            self.visual.run_until_parked();
        }

        fn tag_text(&mut self) -> String {
            let tag_box = self.block.read_with(&self.visual, |b, _| b.tag_box.clone());
            tag_box.read_with(&self.visual, |c, _| c.value().to_string())
        }

        fn known(&mut self) -> bool {
            self.block.read_with(&self.visual, |b, cx| b.facts(cx).is_some())
        }

        fn facts(&mut self) -> (Option<String>, Vec<String>, bool) {
            self.block.read_with(&self.visual, |b, cx| {
                let facts = b.facts(cx).expect("the marks are known");
                (facts.label, facts.tags, facts.mixed)
            })
        }

        /// Waits for the backend to have been asked [n] changes, and for what follows (the read again).
        fn sets_reach(&mut self, n: usize) {
            for _ in 0..400 {
                self.visual.run_until_parked();
                if self.rec.sets().len() >= n {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            self.visual.run_until_parked();
        }
    }

    /// A block over [paths] (a window of its own), with the backend knowing [items] and the home source saying [home].
    fn block(cx: &mut TestAppContext, paths: &[&str], items: &[(&str, Marks)], home: Option<Home>, stall: bool) -> Env {
        cx.update(testing::init);
        let rec = Recorder::with(items, stall);
        let backend: Arc<dyn Backend> = rec.clone();
        cx.update(|cx| {
            tags::init_with(cx, backend);
            init_home(cx, Rc::new(move |_| Box::pin(async move { home })));
        });
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        let (visual, block) = testing::open_window(cx, move |window, cx| {
            cx.new(|cx| MarksBlock::new(paths, "t", window, cx))
        });
        let mut env = Env { visual, block, rec };
        // Focus events (and so blur) only reach an active window.
        env.visual.update(|window, _| window.activate_window());
        if !stall {
            for _ in 0..200 {
                env.visual.run_until_parked();
                if env.known() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            env.visual.run_until_parked();
        }
        env
    }

    #[gpui_kit::test]
    fn a_marked_item_shows_its_label_its_chips_and_its_comment(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(
            cx,
            &[A],
            &[(A, marked(Some("Red"), &["bond", "netplan"], Some("LACP, mode 4")))],
            None,
            false,
        );
        let (label, tags, mixed) = env.facts();
        assert_eq!(label.as_deref(), Some("Red"), "the Red dot is on");
        assert_eq!(tags, ["bond", "netplan"]);
        assert!(!mixed);
        assert!(env.has("Red") && env.has("Green"), "one dot a label, named by it");
        assert!(env.has("Remove tag bond") && env.has("Remove tag netplan"), "two chips");
        assert_eq!(env.comment(), "LACP, mode 4");
        assert!(env.has("Add a tag") && env.has("Comment"));
    }

    #[gpui_kit::test]
    fn an_item_with_no_marks_shows_the_dots_and_two_empty_boxes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[], None, false);
        let (label, tags, _) = env.facts();
        assert_eq!((label, tags.len()), (None, 0));
        for name in ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"] {
            assert!(env.has(name), "the {name} dot");
        }
        assert!(!env.has("Remove tag bond"));
        assert_eq!(env.comment(), "");
        assert_eq!(env.tag_text(), "");
        assert!(!env.has("Kept beside the file") && !env.has("Kept on this PC"));
    }

    #[gpui_kit::test]
    fn an_item_whose_marks_are_not_known_yet_draws_nothing(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(Some("Red"), &[], None))], None, true);
        assert!(env.rec.reads() > 0, "it asked");
        assert!(!env.known());
        assert!(!env.has("Marks") && !env.has("Red") && !env.has("Comment"), "no skeleton");
    }

    #[gpui_kit::test]
    fn pressing_green_asks_for_green_and_pressing_the_label_that_is_on_takes_it_off(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(Some("Red"), &[], None))], None, false);
        env.click(("t-marks-label", 3_usize));
        env.sets_reach(1);
        assert_eq!(env.rec.sets(), [(vec![A.to_string()], Change::Label(Some("Green".into())))]);
        let mut env = block(cx, &[A], &[(A, marked(Some("Red"), &[], None))], None, false);
        env.click(("t-marks-label", 0_usize));
        env.sets_reach(1);
        assert_eq!(env.rec.sets(), [(vec![A.to_string()], Change::Label(None))]);
    }

    #[gpui_kit::test]
    fn typing_a_tag_and_enter_adds_it_and_empties_the_box(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[], None, false);
        env.type_tag("work");
        assert_eq!(env.tag_text(), "work");
        env.press("enter");
        env.sets_reach(1);
        let add = Change::Tags {
            add: vec!["work".into()],
            remove: vec![],
        };
        assert_eq!(env.rec.sets(), [(vec![A.to_string()], add)]);
        assert_eq!(env.tag_text(), "");
    }

    #[gpui_kit::test]
    fn a_comma_adds_the_tag_typed_before_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[], None, false);
        env.type_tag("lab,");
        env.sets_reach(1);
        let add = Change::Tags {
            add: vec!["lab".into()],
            remove: vec![],
        };
        assert_eq!(env.rec.sets(), [(vec![A.to_string()], add)]);
        assert_eq!(env.tag_text(), "");
    }

    #[gpui_kit::test]
    fn a_chips_remove_button_asks_for_that_tags_removal(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(None, &["bond", "netplan"], None))], None, false);
        env.click(("t-marks-remove", 1_usize));
        env.sets_reach(1);
        let remove = Change::Tags {
            add: vec![],
            remove: vec!["netplan".into()],
        };
        assert_eq!(env.rec.sets(), [(vec![A.to_string()], remove)]);
    }

    #[gpui_kit::test]
    fn an_edited_comment_is_saved_once_when_the_focus_leaves(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(None, &[], Some("LACP")))], None, false);
        env.type_comment(" mode 4");
        assert_eq!(env.comment(), "LACP mode 4");
        assert!(env.rec.sets().is_empty(), "nothing is saved while typing");
        env.leave();
        env.sets_reach(1);
        assert_eq!(
            env.rec.sets(),
            [(vec![A.to_string()], Change::Comment(Some("LACP mode 4".into())))]
        );
        env.type_comment("");
        env.leave();
        assert_eq!(env.rec.sets().len(), 1, "leaving again saves nothing more");
    }

    #[gpui_kit::test]
    fn ctrl_enter_saves_the_comment_and_leaving_afterwards_does_not_save_it_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(None, &[], Some("LACP")))], None, false);
        env.type_comment("!");
        env.press("ctrl-enter");
        env.sets_reach(1);
        env.leave();
        assert_eq!(
            env.rec.sets(),
            [(vec![A.to_string()], Change::Comment(Some("LACP!".into())))]
        );
        assert_eq!(env.comment(), "LACP!", "no line break came in with the keys");
    }

    #[gpui_kit::test]
    fn escape_puts_back_the_old_comment_asks_nothing_and_keeps_the_focus_in_the_block(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(None, &[], Some("LACP")))], None, false);
        env.type_comment(" oops");
        env.press("escape");
        assert_eq!(env.comment(), "LACP");
        env.leave();
        assert!(env.rec.sets().is_empty());
        let focused = env.visual.update(|window, cx| {
            let block = env.block.read(cx);
            block.focus.is_focused(window) || block.focus.contains_focused(window, cx)
        });
        assert!(focused, "the focus stays in the block");
    }

    #[gpui_kit::test]
    fn an_unchanged_comment_that_is_left_saves_nothing_and_an_emptied_one_clears(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(None, &[], Some("LACP")))], None, false);
        env.type_comment("");
        env.leave();
        assert!(env.rec.sets().is_empty(), "unchanged: nothing");
        env.set_comment("");
        env.leave();
        env.sets_reach(1);
        assert_eq!(env.rec.sets(), [(vec![A.to_string()], Change::Comment(None))]);
    }

    #[gpui_kit::test]
    fn a_comment_over_64_kb_is_not_sent_and_the_box_says_so_and_keeps_the_text(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[], None, false);
        let long = "x".repeat(COMMENT_CAP + 1);
        env.set_comment(&long);
        env.leave();
        assert!(env.rec.sets().is_empty());
        assert!(env.has("A comment can be 64 KB at most"));
        assert_eq!(env.comment().len(), long.len(), "the text is kept");
    }

    #[gpui_kit::test]
    fn three_items_show_what_they_share_and_a_typed_comment_is_set_on_all_three(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(
            cx,
            &[A, B, C],
            &[
                (A, marked(Some("Red"), &["bond", "netplan"], Some("one"))),
                (B, marked(Some("Red"), &["bond", "dns"], Some("two"))),
                (C, marked(Some("Red"), &["bond"], None)),
            ],
            None,
            false,
        );
        let (label, tags, mixed) = env.facts();
        assert_eq!(label.as_deref(), Some("Red"), "the shared label is on");
        assert_eq!(tags, ["bond"], "only the tags all three have are chips");
        assert!(mixed, "different comments");
        assert_eq!(env.comment(), "");
        let said = env.block.read_with(&env.visual, |b, _| b.placeholder.to_string());
        assert_eq!(said, "Several comments");
        // Untouched, left: nothing is saved.
        env.type_comment("");
        env.leave();
        assert!(env.rec.sets().is_empty());
        env.type_comment("same");
        env.leave();
        env.sets_reach(1);
        let all: Vec<String> = [A, B, C].iter().map(|p| p.to_string()).collect();
        assert_eq!(env.rec.sets(), [(all, Change::Comment(Some("same".into())))]);
    }

    #[gpui_kit::test]
    fn three_items_with_different_labels_have_no_dot_on(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(
            cx,
            &[A, B],
            &[(A, marked(Some("Red"), &[], None)), (B, marked(Some("Blue"), &[], None))],
            None,
            false,
        );
        assert_eq!(env.facts().0, None);
    }

    #[gpui_kit::test]
    fn where_the_marks_are_kept_is_said_only_for_beside_and_this_pc(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let said = |cx: &mut TestAppContext, home: Home, marks: Marks| -> (bool, bool) {
            let mut env = block(cx, &[A], &[(A, marks)], Some(home), false);
            for _ in 0..100 {
                env.visual.run_until_parked();
                if env.block.read_with(&env.visual, |b, _| b.home.is_some()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            (env.has("Kept beside the file"), env.has("Kept on this PC"))
        };
        let tagged = marked(None, &["bond"], None);
        assert_eq!(said(cx, Home::Beside, tagged.clone()), (true, false));
        assert_eq!(said(cx, Home::Ledger, tagged.clone()), (false, true));
        assert_eq!(said(cx, Home::Stream, tagged), (false, false));
        assert_eq!(said(cx, Home::Beside, Marks::default()), (false, false), "no marks, nothing said");
    }

    #[gpui_kit::test]
    fn a_mark_changed_elsewhere_shows_once_read_again_without_touching_a_box_being_typed_in(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(cx, &[A], &[(A, marked(None, &["bond"], Some("LACP")))], None, false);
        env.type_comment(" typed");
        env.rec.put(A, marked(Some("Blue"), &["bond", "vlan"], Some("elsewhere")));
        env.visual.update(|_, cx| {
            let hub = tags::hub(cx);
            hub.update(cx, |_, cx| cx.emit(TagsChanged { paths: vec![A.to_string()] }));
        });
        for _ in 0..200 {
            env.visual.run_until_parked();
            if env.facts().1.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let (label, tags, _) = env.facts();
        assert_eq!(label.as_deref(), Some("Blue"));
        assert_eq!(tags, ["bond", "vlan"]);
        assert!(env.has("Remove tag vlan"));
        assert_eq!(env.comment(), "LACP typed", "what is being typed is left alone");
    }

    #[gpui_kit::test]
    fn an_arabic_name_tag_and_comment_mirror_with_everything_named_and_nothing_overlapping(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = block(
            cx,
            &[A],
            &[(A, marked(Some("Red"), &["شبكة", "bond"], Some("ملاحظة عن الربط")))],
            None,
            false,
        );
        // After the window: opening one starts from the English interface.
        testing::set_rtl(true);
        assert!(env.has("Remove tag شبكة"));
        assert_eq!(env.comment(), "ملاحظة عن الربط");
        let (unnamed, pressable, rows) = env.visual.update(|window, cx| {
            window.render_frame(cx);
            let elements = window.elements();
            let named = |e: &gpui_kit::test::ElementSnapshot| e.label().is_some_and(|l| !l.trim().is_empty());
            let pressable: Vec<_> = elements
                .iter()
                .filter(|e| {
                    e.role().is_some_and(|r| {
                        [Role::Button, Role::TextInput, Role::MultilineTextInput].contains(&r)
                    })
                })
                .collect();
            let unnamed = pressable.iter().filter(|e| !named(e)).count();
            let find = |label: &str| window.try_find_by_label(label).map(|e| e.bounds());
            (unnamed, pressable.len(), (find("Label"), find("Tags"), find("Comment"), find("شبكة"), find("Add a tag")))
        });
        assert_eq!(unnamed, 0, "every pressable thing has a name");
        assert!(pressable >= 7 + 2 + 2, "dots, remove buttons and the boxes were seen: {pressable}");
        let (dots, tags, comment, chip, add) = rows;
        let (dots, tags, comment) = (dots.expect("dots"), tags.expect("tags"), comment.expect("comment"));
        assert!(dots.bottom() <= tags.top() + px(0.5) && tags.bottom() <= comment.top() + px(0.5), "stacked");
        let (chip, add) = (chip.expect("chip"), add.expect("box"));
        assert!(
            chip.right() >= tags.right() - px(2.),
            "right to left: the chips start at the end edge, on the right: chip {chip:?} row {tags:?}"
        );
        assert!(add.top() >= chip.bottom() - px(0.5) || add.right() <= chip.left() + px(0.5), "the box does not overlap the chip");
        testing::set_rtl(false);
    }

    /// Selects a file of the pane's folder and lets the pane settle on it.
    fn select(env: &mut PaneEnv, name: &str) {
        env.select(name);
    }

    #[gpui_kit::test]
    fn in_quick_look_a_space_typed_in_the_comment_is_a_space_and_quick_look_stays_open(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = pane_setup(cx);
        let rec = Recorder::with(&[], false);
        let backend: Arc<dyn Backend> = rec.clone();
        env.visual.update(|window, cx| {
            window.activate_window();
            tags::init_with(cx, backend)
        });
        select(&mut env, "notes.txt");
        env.visual.update(quick_look::toggle);
        env.visual.run_until_parked();
        assert!(env.dialog_open());
        let ql = env.visual.update(|_, cx| quick_look::current(cx)).expect("it is open");
        let view = ql.read_with(&env.visual, |q, _| q.view().clone());
        let block = view.read_with(&env.visual, |v, _| v.marks().cloned()).expect("the block is there");
        for _ in 0..200 {
            env.visual.run_until_parked();
            if block.read_with(&env.visual, |b, cx| b.facts(cx).is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let comment = block.read_with(&env.visual, |b, _| b.comment.clone());
        env.visual.update(|window, cx| {
            comment.update(cx, |input, cx| input.focus(window, cx));
            window.render_frame(cx);
            window.input("a", cx);
            window.press("space", cx);
            window.input("b", cx);
        });
        env.visual.run_until_parked();
        assert!(env.dialog_open(), "Space did not close Quick Look");
        assert_eq!(comment.read_with(&env.visual, |c, _| c.value().to_string()), "a b");
        // Escape in the box is the box's: it does not close Quick Look either.
        env.press("escape");
        assert!(env.dialog_open(), "Escape in the box leaves Quick Look open");
        env.press("escape");
        assert!(!env.dialog_open(), "Escape outside the box closes it");
    }

    #[gpui_kit::test]
    fn the_pane_shows_the_block_for_a_file_and_not_for_an_item_in_an_archive(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut env = pane_setup(cx);
        let rec = Recorder::with(&[], false);
        let backend: Arc<dyn Backend> = rec.clone();
        env.visual.update(|_, cx| tags::init_with(cx, backend));
        select(&mut env, "notes.txt");
        let view = env.entry_view().expect("one file is shown");
        assert!(view.read_with(&env.visual, |v, _| v.marks().is_some()));
        assert!(env.has("Marks"));
        select(&mut env, "inside.txt");
        let view = env.entry_view().expect("one item is shown");
        assert!(view.read_with(&env.visual, |v, _| v.marks().is_none()), "an item in an archive has none");
        select(&mut env, "gone.txt");
        let view = env.entry_view().expect("one item is shown");
        assert!(view.read_with(&env.visual, |v, _| v.marks().is_none()), "nor one in the Recycle Bin");
        select(&mut env, "sub");
        let view = env.entry_view().expect("a folder is shown");
        assert!(view.read_with(&env.visual, |v, _| v.marks().is_some()), "a folder has one");
    }
}
