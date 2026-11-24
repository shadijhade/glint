//! Settings → Files → Labels and Marks (docs/gpui/marks, F1, F29, F30): the labels renamed, recolored, added,
//! reordered and taken out; following XYplorer; taking back the last import.
//!
//! The edits are pure functions of the list ([rename], [recolor], [add], [move_by], [remove]), so they are tested
//! without a window. The two sections are views of their own (made when the Files page first draws them, one pair
//! for each Settings window: [panels]), because a name box needs a window to be made and a section is built
//! without one. What the PC knows about the last import comes through a [Source], which tests replace.

use super::backend::Fut;
use super::widgets;
use crate::dialogs::tags::{self, TagsChanged};
use crate::kit::prelude::*;
use crate::{core_bridge, i18n, state, t, theme};
use glint_core::marks::LastImport;
use glint_model::format::{format_count, format_date};
use glint_model::marks::{default_labels, label_of, Label, MAX_LABELS};
use std::collections::HashMap;
use std::sync::Arc;

actions!(
    glint_settings_labels,
    [
        /// Escape in a name box: the name goes back to what the label is called.
        PutNameBack
    ]
);

/// Set once the key is bound.
struct Bound;
impl Global for Bound {}

/// Binds Escape in the name boxes, once per app (the box would take it for itself, as the marks block's do).
fn ensure_bound(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    cx.bind_keys([KeyBinding::new("escape", PutNameBack, Some("LabelRow > Input"))]);
}

/// The longest a label's name can be, in characters.
const MAX_NAME: usize = 64;

/// The twelve colors a label can be given: today's seven (Tailwind's 500s, as the color tags are) and five more.
/// A fixed palette: there is no free color.
pub const PALETTE: [u32; 12] = [
    0xef4444, 0xf97316, 0xeab308, 0x22c55e, 0x3b82f6, 0xa855f7, 0x9ca3af, 0x14b8a6, 0x0ea5e9, 0xec4899, 0x84cc16,
    0xb45309,
];

/// What a palette color is called, for the button that chooses it.
pub fn color_name(color: u32) -> SharedString {
    match color {
        0xef4444 => t!("Red"),
        0xf97316 => t!("Orange"),
        0xeab308 => t!("Yellow"),
        0x22c55e => t!("Green"),
        0x3b82f6 => t!("Blue"),
        0xa855f7 => t!("Purple"),
        0x9ca3af => t!("Gray"),
        0x14b8a6 => t!("Teal"),
        0x0ea5e9 => t!("Sky"),
        0xec4899 => t!("Pink"),
        0x84cc16 => t!("Lime"),
        0xb45309 => t!("Brown"),
        _ => t!("Another color"),
    }
}

// ---- The edits ---------------------------------------------------------------------------------------------

/// Label `at` called `name`: the old name goes to `was` (nothing on disk is rewritten: files that say it still show
/// this label), and a name that was in its own `was` comes out of it (going back to a name empties it). Refused, in
/// words: empty, over 64 characters or with a line break, or a name another label has or had (any case).
pub fn rename(labels: &[Label], at: usize, name: &str) -> Result<Vec<Label>, SharedString> {
    let name = name.trim();
    if name.is_empty() {
        return Err(t!("Give the label a name"));
    }
    if name.chars().count() > MAX_NAME || name.contains(['\n', '\r']) {
        return Err(t!("A label's name is one line of 64 characters at most"));
    }
    let Some(old) = labels.get(at) else { return Ok(labels.to_vec()) };
    let others: Vec<Label> = labels.iter().enumerate().filter(|(i, _)| *i != at).map(|(_, l)| l.clone()).collect();
    if label_of(&others, name).is_some() {
        return Err(t!("Another label is or was called that"));
    }
    if old.name == name {
        return Ok(labels.to_vec());
    }
    let lower = name.to_lowercase();
    let mut next = labels.to_vec();
    if let Some(label) = next.get_mut(at) {
        let came_back = label.was.iter().any(|w| w.to_lowercase() == lower);
        label.was.retain(|w| w.to_lowercase() != lower);
        // Only a different name is remembered: "red" for "Red" is the same label by any case.
        if !came_back && old.name.to_lowercase() != lower {
            label.was.push(old.name.clone());
        }
        label.name = name.to_string();
    }
    Ok(next)
}

/// Label `at` in `color`.
pub fn recolor(labels: &[Label], at: usize, color: u32) -> Vec<Label> {
    let mut next = labels.to_vec();
    if let Some(label) = next.get_mut(at) {
        label.color = color;
    }
    next
}

/// A new label at the end: "New label" (with a number while one is called that), in the first color of the
/// palette no label has. Refused at [MAX_LABELS].
pub fn add(labels: &[Label]) -> Result<Vec<Label>, SharedString> {
    if labels.len() >= MAX_LABELS {
        return Err(t!("32 labels at most"));
    }
    let base = t!("New label").to_string();
    let mut name = base.clone();
    let mut n = 2;
    while label_of(labels, &name).is_some() {
        name = format!("{base} {n}");
        n += 1;
    }
    // Past twelve labels colors repeat, round the palette.
    let color = PALETTE
        .iter()
        .copied()
        .find(|c| !labels.iter().any(|l| l.color == *c))
        .unwrap_or(PALETTE[labels.len() % PALETTE.len()]);
    let mut next = labels.to_vec();
    next.push(Label { name, color, was: Vec::new() });
    Ok(next)
}

/// Label `at` one place earlier (-1) or later (1); at the ends nothing changes.
pub fn move_by(labels: &[Label], at: usize, by: i32) -> Vec<Label> {
    let mut next = labels.to_vec();
    let to = if by < 0 { at.checked_sub(1) } else { at.checked_add(1) };
    if let Some(to) = to.filter(|to| *to < next.len() && at < next.len()) {
        next.swap(at, to);
    }
    next
}

/// The list without label `at`. The items that carry it keep its name and are drawn as a tag's color is: nothing is
/// rewritten.
pub fn remove(labels: &[Label], at: usize) -> Vec<Label> {
    let mut next = labels.to_vec();
    if at < next.len() {
        next.remove(at);
    }
    next
}

// ---- What the PC knows of the last import -------------------------------------------------------------------

/// What the Marks section asks of the PC: the last import that can be taken back, and taking it back. Tests give
/// the section a fake (the real one is [CoreSource]).
pub trait Source: Send + Sync {
    /// The last import whose journal is still kept.
    fn last_import(&self) -> Fut<Option<LastImport>>;
    /// Puts back what the import with this journal changed; answers the paths whose marks changed.
    fn take_back(&self, journal: String) -> Fut<Vec<String>>;
}

/// Core's answer.
struct CoreSource;

impl Source for CoreSource {
    fn last_import(&self) -> Fut<Option<LastImport>> {
        Box::pin(async { core_bridge::run(async { glint_core::marks::last_import() }).await })
    }

    fn take_back(&self, journal: String) -> Fut<Vec<String>> {
        Box::pin(async move {
            let outcomes = core_bridge::run(glint_core::marks::import_take_back(journal)).await.and_then(|r| r)?;
            Ok(outcomes.into_iter().filter(|o| o.after.is_ok()).map(|o| o.path).collect())
        })
    }
}

/// The source in use, when a test (or anything else) replaced the real one.
struct Sourced(Arc<dyn Source>);
impl Global for Sourced {}

/// Replaces the source the Marks section asks.
#[cfg_attr(not(test), allow(dead_code))] // only tests replace it today
pub fn set_source(cx: &mut App, source: Arc<dyn Source>) {
    cx.set_global(Sourced(source));
}

fn source(cx: &App) -> Arc<dyn Source> {
    cx.try_global::<Sourced>().map_or_else(|| Arc::new(CoreSource) as Arc<dyn Source>, |s| s.0.clone())
}

// ---- The two sections --------------------------------------------------------------------------------------

/// The pair of views a Settings window draws its two sections with, by the window's view.
#[derive(Default)]
struct Panels(HashMap<EntityId, (Entity<LabelRows>, Entity<MarksRows>)>);
impl Global for Panels {}

/// The views of the two sections of `this` Settings window, made the first time they are asked for.
pub(super) fn panels<V: 'static>(cx: &mut Context<V>) -> (Entity<LabelRows>, Entity<MarksRows>) {
    let id = cx.entity_id();
    if let Some(found) = cx.try_global::<Panels>().and_then(|p| p.0.get(&id)) {
        return found.clone();
    }
    let made = (cx.new(LabelRows::new), cx.new(MarksRows::new));
    if !cx.has_global::<Panels>() {
        cx.set_global(Panels::default());
    }
    cx.global_mut::<Panels>().0.insert(id, made.clone());
    // Gone with the window.
    cx.on_release(move |_, cx| {
        cx.global_mut::<Panels>().0.remove(&id);
    })
    .detach();
    made
}

/// The views of the one Settings window there is, for tests.
#[cfg(test)]
pub(super) fn open_panels(cx: &App) -> Option<(Entity<LabelRows>, Entity<MarksRows>)> {
    cx.try_global::<Panels>()?.0.values().next().cloned()
}

/// Puts the labels back as they were (what Undo does).
pub(super) fn restore(cx: &mut App, before: Vec<Label>) {
    state::update(cx, move |s| s.labels = before);
}

/// Changes the labels, and says so with Undo (as Settings' Clear history does).
fn apply(window: &mut Window, cx: &mut App, before: Vec<Label>, after: Vec<Label>) {
    state::update(cx, |s| s.labels = after);
    window.push_notification(
        Notification::success(t!("Labels changed")).action(move |_, _, cx| {
            let before = before.clone();
            Button::new("settings-undo-labels")
                .label(t!("Undo"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    restore(cx, before.clone());
                    this.dismiss(window, cx);
                }))
        }),
        cx,
    );
}

/// Settings → Files → Labels: a row a label.
pub struct LabelRows {
    /// A name box a label, in the labels' order; each with what it is subscribed to and the name it was made for.
    boxes: Vec<(Entity<InputState>, Subscription, String)>,
    /// The words of the last refusal, under the box of the label at its place.
    error: Option<(usize, SharedString)>,
    /// The label whose colors are shown.
    picking: Option<usize>,
    _subscription: Subscription,
}

impl LabelRows {
    fn new(cx: &mut Context<Self>) -> Self {
        ensure_bound(cx);
        Self {
            boxes: Vec::new(),
            error: None,
            picking: None,
            _subscription: cx.observe_global::<state::UiState>(|_, cx| cx.notify()),
        }
    }

    /// The name box of the label at `i`, for tests.
    #[cfg(test)]
    pub(super) fn input(&self, i: usize) -> Option<Entity<InputState>> {
        self.boxes.get(i).map(|(b, _, _)| b.clone())
    }

    /// The name box of each label, made (or made again when the label's name is not the one the box was made for:
    /// a label moved, or changed from elsewhere) with what each is called now. A box with something typed in it
    /// that was not kept (a refusal) is left alone.
    fn sync_boxes(&mut self, labels: &[Label], window: &mut Window, cx: &mut Context<Self>) {
        self.boxes.truncate(labels.len());
        for (i, label) in labels.iter().enumerate() {
            if self.boxes.get(i).is_some_and(|(_, _, built)| *built == label.name) {
                continue;
            }
            let name = label.name.clone();
            let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
            let subscription = cx.subscribe_in(&input, window, |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.commit(input.entity_id(), window, cx);
                }
            });
            let made = (input, subscription, label.name.clone());
            match self.boxes.get_mut(i) {
                Some(slot) => *slot = made,
                None => self.boxes.push(made),
            }
        }
    }

    /// The name typed in a box becomes the label's name, or its refusal shows under it.
    fn commit(&mut self, input: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(at) = self.boxes.iter().position(|(b, _, _)| b.entity_id() == input) else { return };
        let before = state::settings(cx).labels.clone();
        let typed = self.boxes[at].0.read(cx).value().to_string();
        if before.get(at).is_none_or(|l| l.name == typed) {
            return;
        }
        match rename(&before, at, &typed) {
            Ok(after) => {
                self.error = None;
                // What was typed is the name now: the box stays (and keeps the keyboard).
                if let (Some(label), Some(slot)) = (after.get(at), self.boxes.get_mut(at)) {
                    if label.name == typed {
                        slot.2 = typed;
                    }
                }
                if after != before {
                    apply(window, cx, before, after);
                }
            }
            Err(words) => self.error = Some((at, words)),
        }
        cx.notify();
    }

    /// Escape in a box puts its name back, and closes the colors when they are open. True when it did something.
    fn escape(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let name = state::settings(cx).labels.get(at).map(|l| l.name.clone());
        let typing = self.boxes.get(at).map(|(b, _, _)| b.clone()).filter(|b| b.read(cx).focus_handle(cx).is_focused(window));
        let mut did = false;
        if let (Some(input), Some(name)) = (typing, name) {
            if input.read(cx).value().as_ref() != name.as_str() || self.error.is_some() {
                input.update(cx, |input, cx| input.set_value(name, window, cx));
                self.error = None;
                did = true;
            }
        }
        if self.picking.take().is_some() {
            did = true;
        }
        if did {
            cx.notify();
        }
        did
    }

    /// Runs one of the edits that can't be refused here, with Undo.
    fn edit(&mut self, window: &mut Window, cx: &mut Context<Self>, edit: impl FnOnce(&[Label]) -> Vec<Label>) {
        let before = state::settings(cx).labels.clone();
        let after = edit(&before);
        self.error = None;
        self.picking = None;
        if after != before {
            apply(window, cx, before, after);
        }
        cx.notify();
    }

    fn row(&self, i: usize, label: &Label, last: usize, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let open = self.picking == Some(i);
        let dot = Button::new(widgets::id(&["settings-label-dot", &i.to_string()]))
            .ghost()
            .small()
            .accessibility_label(t!("Color of {name}", name = label.name.clone()))
            .child(div().size(space(3.)).rounded_full().bg(theme::label_tone(label.color)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.picking = if this.picking == Some(i) { None } else { Some(i) };
                cx.notify();
            }));
        let name_box = self.boxes.get(i).map(|(input, _, _)| {
            col()
                .flex_1()
                .min_w(space(24.))
                .gap(space(1.))
                .child(Input::new(input).aria_label(t!("Name of the label")))
                .children(self.error.clone().filter(|(at, _)| *at == i).map(|(_, words)| {
                    div()
                        .id(widgets::id(&["settings-label-error", &i.to_string()]))
                        .role(Role::Alert)
                        .aria_label(words.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_start()
                        .text_color(t.destructive)
                        .child(words)
                }))
        });
        let mut line = row_start()
            .id(widgets::id(&["settings-labelrow", &i.to_string()]))
            .role(Role::Group)
            .aria_label(label.name.clone())
            .test_support()
            .items_start()
            .gap(space(2.))
            .px(space(4.))
            .py(space(2.))
            .key_context("LabelRow")
            .on_action(cx.listener(move |this, _: &PutNameBack, window, cx| {
                // Nothing to put back: Escape is the dialog's.
                if !this.escape(i, window, cx) {
                    cx.propagate();
                }
            }))
            .child(dot)
            .children(name_box)
            .child(
                widgets::button(&format!("label-up-{i}"), t!("Move up"), Some(IconName::ChevronUp))
                    .disabled(i == 0)
                    .accessibility_label(t!("Move up"))
                    .on_click(cx.listener(move |this, _, window, cx| this.edit(window, cx, |l| move_by(l, i, -1)))),
            )
            .child(
                widgets::button(&format!("label-down-{i}"), t!("Move down"), Some(IconName::ChevronDown))
                    .disabled(i >= last)
                    .accessibility_label(t!("Move down"))
                    .on_click(cx.listener(move |this, _, window, cx| this.edit(window, cx, |l| move_by(l, i, 1)))),
            )
            .child(
                widgets::quiet_button(&format!("label-remove-{i}"), t!("Remove"))
                    .on_click(cx.listener(move |this, _, window, cx| this.edit(window, cx, |l| remove(l, i)))),
            );
        if open {
            line = line.child(self.colors(i, label.color, t, cx));
        }
        // The colors open under the row, across its width.
        div().flex_wrap().child(line).into_any_element()
    }

    /// The twelve colors, each a button named by its color; the label's own has a ring.
    fn colors(&self, i: usize, current: u32, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        row_start()
            .id(widgets::id(&["settings-label-colors", &i.to_string()]))
            .role(Role::Group)
            .aria_label(t!("Colors"))
            .test_support()
            .w_full()
            .flex_wrap()
            .gap(space(1.))
            .children(PALETTE.iter().enumerate().map(|(k, color)| {
                let color = *color;
                let dot = div()
                    .size(space(4.))
                    .rounded_full()
                    .border_2()
                    .border_color(if color == current { t.foreground } else { t.card })
                    .child(div().size_full().rounded_full().bg(theme::label_tone(color)));
                Button::new(widgets::id(&["settings-label-color", &i.to_string(), &k.to_string()]))
                    .ghost()
                    .small()
                    .accessibility_label(color_name(color))
                    .child(dot)
                    .on_click(cx.listener(move |this, _, window, cx| this.edit(window, cx, |l| recolor(l, i, color))))
            }))
            .into_any_element()
    }
}

impl Render for LabelRows {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let labels = state::settings(cx).labels.clone();
        self.sync_boxes(&labels, window, cx);
        let last = labels.len().saturating_sub(1);
        let full = labels.len() >= MAX_LABELS;
        let mut rows: Vec<AnyElement> = vec![widgets::note(
            t!("A label is kept in the file by its name. Renaming one changes nothing on disk: files that say the old name show the new one."),
            &t,
        )
        .into_any_element()];
        for (i, label) in labels.iter().enumerate() {
            rows.push(self.row(i, label, last, &t, cx));
        }
        rows.push(
            row_start()
                .px(space(4.))
                .py(space(3.))
                .gap(space(2.))
                .flex_wrap()
                .items_center()
                .child(
                    widgets::button("label-add", t!("Add a label"), Some(IconName::Plus))
                        .disabled(full)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.edit(window, cx, |l| add(l).unwrap_or_else(|_| l.to_vec()))
                        })),
                )
                .when(full, |this| {
                    this.child(
                        div()
                            .id("settings-label-full")
                            .role(Role::Label)
                            .aria_label(t!("32 labels at most"))
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(t!("32 labels at most")),
                    )
                })
                .child(
                    widgets::button("label-defaults", t!("Back to the seven colors"), None)
                        .on_click(cx.listener(|this, _, window, cx| this.edit(window, cx, |_| default_labels()))),
                )
                .into_any_element(),
        );
        widgets::section("labels", t!("Labels"), None, rows, &t)
    }
}

/// Settings → Files → Marks: following XYplorer, and taking back the last import.
pub struct MarksRows {
    /// The last import that can be taken back; None until asked, or when there is none.
    last: Option<LastImport>,
    /// Whether the source has answered (until then the row says nothing, M4).
    known: bool,
    /// A take back is under way.
    busy: bool,
}

impl MarksRows {
    /// What the last-import row says, when it is known.
    pub(super) fn said(&self) -> Option<SharedString> {
        match (&self.last, self.known) {
            (Some(last), _) => {
                let source = glint_model::paths::base_name(&last.source).to_string();
                let when = format_date(last.at, None, i18n::language());
                Some(t!("{n} from {name}, {when}", n = format_count(last.items as usize), name = source, when = when))
            }
            (None, true) => Some(t!("Nothing was imported in the last 30 days")),
            (None, false) => None,
        }
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let this = Self { last: None, known: false, busy: false };
        this.ask(cx);
        this
    }

    /// Asks the source for the last import, off the UI thread.
    fn ask(&self, cx: &mut Context<Self>) {
        let asked = source(cx).last_import();
        cx.spawn(async move |this, cx| {
            let last = asked.await.ok().flatten();
            this.update(cx, |this, cx| {
                this.last = last;
                this.known = true;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn take_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(last) = self.last.clone().filter(|_| !self.busy) else { return };
        self.busy = true;
        let labels = last.labels.zip(last.made);
        let done = source(cx).take_back(last.journal);
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = done.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(paths) => {
                        // After the marks: the labels the import made go back too, while they are still as it
                        // left them (the same rule as Undo's); said nothing more of either way.
                        if let Some((before, made)) = labels {
                            crate::dialogs::marks_import::put_labels_back(cx, &before, &made);
                        }
                        let n = paths.len();
                        tags::hub(cx).update(cx, |_, cx| cx.emit(TagsChanged { paths }));
                        window.push_notification(
                            Notification::success(t!("Took back the import of {n}", n = format_count(n))),
                            cx,
                        );
                        this.ask(cx);
                    }
                    Err(reason) => window.push_notification(Notification::error(reason), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for MarksRows {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let follow = state::settings(cx).follow_xyplorer;
        let said = self.said();
        let button = widgets::button("take-back-import", t!("Take back"), None)
            .disabled(self.last.is_none() || self.busy)
            .on_click(cx.listener(|this, _, window, cx| this.take_back(window, cx)));
        let rows = vec![
            widgets::switch_row(
                "follow-xyplorer",
                t!("Keep following XYplorer"),
                Some(t!("When its tag.dat changes, what is new in it comes in as Glint starts or comes to the front")),
                follow,
                false,
                |on, _, cx| state::update(cx, |s| s.follow_xyplorer = on),
                &t,
            )
            .into_any_element(),
            widgets::setting_row(t!("Take back the last import"), said, vec![button.into_any_element()], &t),
        ];
        widgets::section("marks", t!("Marks"), None, rows, &t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(labels: &[Label]) -> Vec<&str> {
        labels.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn renaming_red_to_important_remembers_red_and_going_back_forgets_it() {
        let seven = default_labels();
        let renamed = rename(&seven, 0, "Important").unwrap();
        assert_eq!((renamed[0].name.as_str(), renamed[0].was.clone()), ("Important", vec!["Red".to_string()]));
        let back = rename(&renamed, 0, "Red").unwrap();
        assert_eq!((back[0].name.as_str(), back[0].was.is_empty()), ("Red", true));
        // Only the case: still the one label, and nothing to remember.
        assert!(rename(&seven, 0, "red").unwrap()[0].was.is_empty());
    }

    #[test]
    fn a_name_another_label_has_or_had_is_refused_in_any_case() {
        let renamed = rename(&default_labels(), 0, "Important").unwrap();
        assert!(rename(&renamed, 2, "ORANGE").is_err());
        assert!(rename(&renamed, 1, "green").is_err());
        assert!(rename(&renamed, 1, "red").is_err(), "Red is a name label 0 had");
        assert!(rename(&renamed, 1, "Grey").is_err(), "Grey is Gray");
        assert!(rename(&renamed, 0, "important").is_ok(), "its own name is its own");
    }

    #[test]
    fn an_empty_long_or_two_line_name_is_refused_and_one_is_trimmed() {
        let seven = default_labels();
        assert_eq!(rename(&seven, 0, "   ").unwrap_err().as_ref(), "Give the label a name");
        assert!(rename(&seven, 0, &"x".repeat(65)).is_err());
        assert!(rename(&seven, 0, &"x".repeat(64)).is_ok());
        assert!(rename(&seven, 0, "two\nlines").is_err());
        assert_eq!(rename(&seven, 0, "  Urgent \n").unwrap()[0].name, "Urgent");
    }

    #[test]
    fn a_ninth_label_has_a_name_and_a_color_no_other_has_and_the_33rd_is_refused() {
        let mut labels = default_labels();
        labels.push(Label { name: "Eighth".into(), color: PALETTE[7], was: Vec::new() });
        let nine = add(&labels).unwrap();
        assert_eq!(nine[8].name, "New label");
        assert!(labels.iter().all(|l| l.color != nine[8].color));
        let ten = add(&nine).unwrap();
        assert_eq!(ten[9].name, "New label 2");
        assert!(nine.iter().all(|l| l.color != ten[9].color));
        let mut many = default_labels();
        while many.len() < MAX_LABELS {
            many = add(&many).unwrap();
        }
        assert_eq!(add(&many).unwrap_err().as_ref(), "32 labels at most");
    }

    #[test]
    fn moving_at_the_ends_changes_nothing_and_remove_takes_the_one_out() {
        let seven = default_labels();
        assert_eq!(move_by(&seven, 0, -1), seven);
        assert_eq!(move_by(&seven, 6, 1), seven);
        assert_eq!(move_by(&seven, 99, 1), seven);
        assert_eq!(names(&move_by(&seven, 0, 1))[..2], ["Orange", "Red"]);
        assert_eq!(names(&remove(&seven, 2)), ["Red", "Orange", "Green", "Blue", "Purple", "Gray"]);
        assert_eq!(remove(&seven, 9), seven);
        assert_eq!(recolor(&seven, 1, PALETTE[9])[1].color, PALETTE[9]);
        assert_eq!(recolor(&seven, 1, PALETTE[9])[0], seven[0]);
    }

    #[test]
    fn the_palette_is_twelve_different_colors_with_todays_seven_first() {
        let mut sorted = PALETTE.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 12);
        let today: Vec<u32> = default_labels().iter().map(|l| l.color).collect();
        assert_eq!(PALETTE[..7], today[..]);
        assert!(PALETTE.iter().all(|c| color_name(*c).as_ref() != "Another color"));
    }
}
