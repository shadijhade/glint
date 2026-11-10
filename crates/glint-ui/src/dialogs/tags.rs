//! Tags for files and folders: the dots a row shows, and the "Edit tags…" dialog.
//! Ported from src/components/explorer/TagsDialog.tsx and the parts of src/lib/tags.ts it uses.
//!
//! | TypeScript                        | Here                                                                  |
//! |-----------------------------------|-----------------------------------------------------------------------|
//! | `COLOR_TAGS`, `tagColor`          | [`COLOR_TAGS`], [`tag_color`]                                         |
//! | `TagDots`                         | [`tag_dots`] (an element for the file list to put in a row)           |
//! | `useTags().recent`                | [`Tags::recent`], kept for the session (see the note on [`Tags`])     |
//! | `setTags(paths, add, remove)`     | [`set_tags`]: tags the items, words a failure, tells [`TagsChanged`]  |
//! | `useTagsEdit` + `TagsDialog`      | [`open`] and [`TagsDialog`]                                           |
//!
//! Where core is called: [`Backend`] has the two calls (`tags_read`, `tags_set`) so tests inject a fake.
//!
//! **How the coordinator wires it.** The menus emit `MenuEvent::EditTags { paths }`: call
//! `dialogs::tags::open(window, cx, paths)`. They emit `MenuEvent::SetTags { paths, add, remove }` for a color
//! tag toggled: call `dialogs::tags::set_tags(window, cx, paths, add, remove)`. Whoever draws tag dots in the
//! file list reads the tags itself (`glint_core::tags::tags_read`, a batch of the rows on screen) and
//! subscribes to the [`TagsChanged`] event of [`hub`] to read the changed paths again (`forgetTags`).

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_model::format::plural;
use glint_model::paths::base_name;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// The color tags, in the order the dialog offers them, with their colors (Tailwind's 500s, as the TypeScript
/// had them). These are the colors of the tags themselves, the same in both themes, so they aren't tokens.
pub const COLOR_TAGS: [(&str, u32); 7] = [
    ("Red", 0xef4444),
    ("Orange", 0xf97316),
    ("Yellow", 0xeab308),
    ("Green", 0x22c55e),
    ("Blue", 0x3b82f6),
    ("Purple", 0xa855f7),
    ("Gray", 0x9ca3af),
];

/// The colors other tags get, picked by a hash of the name.
const OTHER_COLORS: [u32; 8] = [
    0x0ea5e9, 0x14b8a6, 0xec4899, 0x84cc16, 0xf59e0b, 0x6366f1, 0x10b981, 0xe11d48,
];

/// The most dots a row shows.
const MOST_DOTS: usize = 5;
/// The most tags the dialog offers as chips.
const MOST_OFFERED: usize = 16;
/// The most recent tags kept.
const MOST_RECENT: usize = 20;

/// A tag's color: a color tag's own, any other's from its name (`tagColor`).
pub fn tag_color(tag: &str) -> Hsla {
    let lower = tag.to_lowercase();
    if let Some((_, hex)) = COLOR_TAGS.iter().find(|(name, _)| name.to_lowercase() == lower) {
        return gpui_kit::rgb(*hex).into();
    }
    // JS: `hash = (hash * 31 + ch.charCodeAt(0)) | 0` over the code points. charCodeAt(0) is the first UTF-16
    // unit (the high surrogate for a character outside the BMP), and `| 0` wraps at 32 bits.
    let mut hash: i32 = 0;
    for ch in lower.chars() {
        let mut units = [0u16; 2];
        let unit = ch.encode_utf16(&mut units)[0];
        hash = hash.wrapping_mul(31).wrapping_add(i32::from(unit));
    }
    let ix = usize::try_from(i64::from(hash).unsigned_abs() % OTHER_COLORS.len() as u64).unwrap_or(0);
    gpui_kit::rgb(OTHER_COLORS[ix]).into()
}

/// The tags in `text` as typed: commas between them, spaces trimmed, empty ones dropped.
pub fn parse_typed(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `tags` has `tag`, in any case.
fn has_tag(tags: &[String], tag: &str) -> bool {
    let lower = tag.to_lowercase();
    tags.iter().any(|t| t.to_lowercase() == lower)
}

/// `tags` with `tag` added, or taken off if it is there (the chips' toggle).
pub fn toggled(tags: &[String], tag: &str) -> Vec<String> {
    if has_tag(tags, tag) {
        let lower = tag.to_lowercase();
        tags.iter().filter(|t| t.to_lowercase() != lower).cloned().collect()
    } else {
        let mut out = tags.to_vec();
        out.push(tag.to_string());
        out
    }
}

/// The tags every one of `lists` has (the first list's, in its order and spelling): what "Edit tags…" starts from.
pub fn shared_tags(lists: &[Vec<String>]) -> Vec<String> {
    let Some((first, rest)) = lists.split_first() else {
        return Vec::new();
    };
    first
        .iter()
        .filter(|tag| rest.iter().all(|list| has_tag(list, tag)))
        .cloned()
        .collect()
}

/// The tags the dialog offers: the color tags, then those used lately, `MOST_OFFERED` in all.
pub fn offered(recent: &[String]) -> Vec<String> {
    COLOR_TAGS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .chain(recent.iter().filter(|t| !has_tag_name(t)).cloned())
        .take(MOST_OFFERED)
        .collect()
}

/// Whether `tag` is one of the color tags.
fn has_tag_name(tag: &str) -> bool {
    let lower = tag.to_lowercase();
    COLOR_TAGS.iter().any(|(name, _)| name.to_lowercase() == lower)
}

// ---- The dots a row shows -----------------------------------------------------------------------------

/// A row's tags as small colored dots (`TagDots`): up to five, named for a screen reader and in a tooltip.
/// `None` for no tags. `id` must be unique among the elements of the window (a row's index, say).
pub fn tag_dots(id: impl Into<ElementId>, tags: &[String], cx: &App) -> Option<impl IntoElement> {
    if tags.is_empty() {
        return None;
    }
    let t = theme::tokens(cx);
    let text: SharedString = t!("Tags: {tags}", tags = tags.join(", "));
    let tip = text.clone();
    Some(
        row()
            .id(id)
            .role(Role::Group)
            .aria_label(text)
            .test_support()
            .flex_none()
            .gap(space(0.5))
            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
            .children(tags.iter().take(MOST_DOTS).map(|tag| {
                div()
                    .size(space(2.))
                    .rounded_full()
                    .bg(tag_color(tag))
                    .border_1()
                    .border_color(t.background)
            })),
    )
}

// ---- Core, behind a trait ---------------------------------------------------------------------------------

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The two calls into glint-core's tags.rs. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Each path's tags, in order (`tags_read`).
    fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>>;
    /// Adds `add` and takes `remove` off every path; the paths that couldn't be tagged, each with why (`tags_set`).
    fn set(&self, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Fut<Vec<(String, String)>>;
}

/// glint-core's tags.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
        Box::pin(glint_core::tags::tags_read(paths))
    }

    fn set(&self, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Fut<Vec<(String, String)>> {
        Box::pin(glint_core::tags::tags_set(paths, add, remove))
    }
}

// ---- What the app shares: recent tags and "tags changed" ---------------------------------------------------

/// Said by [`hub`] after tags were set on `paths` (or tried): the file list reads their tags again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagsChanged {
    /// The paths `set_tags` was asked to tag.
    pub paths: Vec<String>,
}

/// The tags shared by the dialog, the menus and the file list: the call into core, the tags used lately and the
/// event that says tags changed.
///
/// NOTE(port): the TypeScript kept the recent tags in `localStorage` (`glint-recent-tags`), so they outlived
/// the session. They are kept for the session here; PORT ME(coordinator): `persistence::AppState` needs a
/// `recent_tags: Vec<String>` (camelCase on disk) for them to outlive it.
pub struct Tags {
    backend: Arc<dyn Backend>,
    recent: Vec<String>,
}

impl EventEmitter<TagsChanged> for Tags {}

impl Tags {
    /// A model calling `backend`.
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self {
            backend,
            recent: Vec::new(),
        }
    }

    /// The tags used lately, most recent first.
    pub fn recent(&self) -> &[String] {
        &self.recent
    }

    /// Puts `added` at the front of the recent tags (each once, any case), keeping `MOST_RECENT`.
    fn remember(&mut self, added: &[String]) {
        let mut recent: Vec<String> = Vec::new();
        for tag in added.iter().chain(self.recent.iter()) {
            if !has_tag(&recent, tag) {
                recent.push(tag.clone());
            }
        }
        recent.truncate(MOST_RECENT);
        self.recent = recent;
    }
}

struct SharedTags(Entity<Tags>);

impl Global for SharedTags {}

/// Makes the shared model with `backend` (the app passes [`CoreBackend`]; tests a fake). [`hub`] makes one on
/// core's tags if this was never called.
pub fn init_with(cx: &mut App, backend: Arc<dyn Backend>) {
    let tags = cx.new(|_| Tags::new(backend));
    cx.set_global(SharedTags(tags));
}

/// The shared model.
pub fn hub(cx: &mut App) -> Entity<Tags> {
    if let Some(shared) = cx.try_global::<SharedTags>() {
        return shared.0.clone();
    }
    init_with(cx, Arc::new(CoreBackend));
    cx.global::<SharedTags>().0.clone()
}

/// Adds `add` to and takes `remove` off the tags of `paths` (`setTags`). It runs on core's runtime; what
/// couldn't be tagged is said in a notification, the recent tags are updated, and [`TagsChanged`] is emitted.
pub fn set_tags(window: &mut Window, cx: &mut App, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) {
    let tags = hub(cx);
    let backend = tags.read(cx).backend.clone();
    let call = backend.set(paths.clone(), add.clone(), remove);
    window
        .spawn(cx, async move |cx| {
            let result = core_bridge::run(call).await.and_then(|inner| inner);
            cx.update(|window, cx| match result {
                Ok(failed) => {
                    tags.update(cx, |tags, cx| {
                        if !add.is_empty() {
                            tags.remember(&add);
                        }
                        cx.emit(TagsChanged { paths });
                    });
                    if let Some((first_path, why)) = failed.first() {
                        let title = if failed.len() == 1 {
                            t!("Couldn't tag “{name}”", name = base_name(first_path))
                        } else {
                            t!("Couldn't tag {n}", n = plural(failed.len(), "item"))
                        };
                        window.push_notification(Notification::error(why.clone()).title(title), cx);
                    }
                }
                Err(reason) => {
                    // Whatever got written still shows.
                    tags.update(cx, |_, cx| cx.emit(TagsChanged { paths }));
                    window.push_notification(Notification::error(reason).title(t!("Couldn't change the tags")), cx);
                }
            })
            .ok();
        })
        .detach();
}

/// Shows `view` in the window's dialog layer as [`super::show`] does, except that Enter does not close it: the
/// boxes in these dialogs answer Enter themselves (Save, Open, Encrypt…), and the dialog layer's own Enter
/// would close the dialog on top of that, even when the answer was "not yet" (two passwords that differ).
fn show<V: Render>(window: &mut Window, cx: &mut App, title: SharedString, width: Pixels, view: Entity<V>) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        dialog
            .title(title.clone())
            .w(width)
            .on_ok(|_, _, _| false)
            .child(view.clone())
    });
}

// ---- The dialog's model -----------------------------------------------------------------------------------

/// Where reading the items' tags stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Reading them.
    Loading,
    /// Read: `shared` holds what every item has.
    Ready,
    /// Couldn't read them, with why. Saving still adds tags; it just can't take any off.
    Failed(SharedString),
}

/// The tags of the items being edited, read from the files.
pub struct TagsEdit {
    paths: Vec<String>,
    shared: Vec<String>,
    state: State,
}

impl TagsEdit {
    /// Reads the tags of `paths` through `backend`, on core's runtime.
    pub fn new(paths: Vec<String>, backend: Arc<dyn Backend>, cx: &mut Context<Self>) -> Self {
        let read = backend.read(paths.clone());
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(lists) => {
                        this.shared = shared_tags(&lists);
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
            shared: Vec::new(),
            state: State::Loading,
        }
    }

    /// The items being edited.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The tags every item has (empty until read).
    pub fn shared(&self) -> &[String] {
        &self.shared
    }

    /// Where reading stands.
    pub fn state(&self) -> &State {
        &self.state
    }
}

// ---- The dialog -------------------------------------------------------------------------------------------

/// "Edit tags…": the tags the items share, as text to change (commas between them), with the color tags and
/// the ones used lately a click away. What is taken off comes off every item; what is added goes on all.
pub struct TagsDialog {
    model: Entity<TagsEdit>,
    tags: Entity<Tags>,
    input: Entity<InputState>,
    /// The box was filled from the files once; after that the person's typing is theirs.
    filled: bool,
    _subscriptions: Vec<Subscription>,
}

impl TagsDialog {
    /// A dialog for the items `model` reads.
    pub fn new(model: Entity<TagsEdit>, tags: Entity<Tags>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Work, Taxes 2026, Red")));
        let on_input = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => this.save(window, cx),
            InputEvent::Change => cx.notify(),
            _ => {}
        });
        let on_model = cx.observe_in(&model, window, |this, _, window, cx| {
            this.fill(window, cx);
            cx.notify();
        });
        let on_tags = cx.observe(&tags, |_, _, cx| cx.notify());
        let mut this = Self {
            model,
            tags,
            input,
            filled: false,
            _subscriptions: vec![on_input, on_model, on_tags],
        };
        this.fill(window, cx);
        this
    }

    /// Puts the shared tags in the box once they are read, unless the person has typed already.
    fn fill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.filled || *self.model.read(cx).state() == State::Loading {
            return;
        }
        self.filled = true;
        let shared = self.model.read(cx).shared().join(", ");
        if !shared.is_empty() && self.input.read(cx).value().is_empty() {
            self.input.update(cx, |input, cx| input.set_value(shared, window, cx));
        }
    }

    /// The box's focus handle, so [`open`] can start typing at once.
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    fn typed(&self, cx: &App) -> Vec<String> {
        parse_typed(&self.input.read(cx).value())
    }

    /// Saves: what is typed is added to every item, what the items shared and is no longer typed comes off.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        if *model.state() == State::Loading {
            return;
        }
        let typed = self.typed(cx);
        let removed: Vec<String> = model
            .shared()
            .iter()
            .filter(|tag| !has_tag(&typed, tag))
            .cloned()
            .collect();
        let paths = model.paths().to_vec();
        window.close_dialog(cx);
        set_tags(window, cx, paths, typed, removed);
    }

    fn toggle(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = toggled(&self.typed(cx), tag).join(", ");
        self.input.update(cx, |input, cx| input.set_value(text, window, cx));
    }
}

impl Render for TagsDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let typed = self.typed(cx);
        let state = self.model.read(cx).state().clone();
        let chips = offered(self.tags.read(cx).recent());

        col()
            .id("tags-dialog")
            .role(Role::Group)
            .aria_label(t!("Tags"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!(
                        "Kept in the files themselves: they go where the files go, on NTFS and ReFS drives."
                    )),
            )
            .child(Input::new(&self.input).aria_label(t!("Tags, with commas between them")))
            .when_some(
                match &state {
                    State::Failed(reason) => Some(reason.clone()),
                    _ => None,
                },
                |this, reason| {
                    let line = t!("Couldn't read the tags: {reason}", reason = reason);
                    this.child(
                        div()
                            .id("tags-error")
                            .role(Role::Alert)
                            .aria_label(line.clone())
                            .test_support()
                            .text_size(theme::size::XS)
                            .text_color(t.destructive)
                            .child(line),
                    )
                },
            )
            .child(
                row_start()
                    .id("tags-chips")
                    .role(Role::Group)
                    .aria_label(t!("Tags to add or take off"))
                    .test_support()
                    .flex_wrap()
                    .gap(space(1.5))
                    .children(chips.into_iter().enumerate().map(|(ix, tag)| {
                        let on = has_tag(&typed, &tag);
                        let label = SharedString::from(tag.clone());
                        Button::new(("tags-chip", ix))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(if on {
                                        t.primary.opacity(0.1)
                                    } else {
                                        t.background.opacity(0.)
                                    })
                                    .foreground(t.foreground)
                                    .hover(t.accent)
                                    .active(t.accent),
                            )
                            .compact()
                            .rounded_full()
                            .border_1()
                            .border_color(if on { t.primary } else { t.border })
                            .role(Role::Button)
                            .toggled(on)
                            .accessibility_label(label.clone())
                            .on_click(cx.listener(move |this, _, window, cx| this.toggle(&tag, window, cx)))
                            .child(
                                align_row(1.5)
                                    .text_size(theme::size::XS)
                                    .child(div().size(space(2.)).rounded_full().bg(tag_color(&label)))
                                    .child(isolate(label)),
                            )
                    })),
            )
            .child(
                row_start()
                    .justify_end()
                    .gap(space(2.))
                    .child(
                        Button::new("tags-cancel")
                            .ghost()
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("tags-save")
                            .primary()
                            .label(t!("Save"))
                            .disabled(state == State::Loading)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

/// The dialog's title: `Tags of “name”` for one item, `Tags of 3 items` for more.
pub fn title(paths: &[String]) -> SharedString {
    match paths {
        [one] => t!("Tags of “{name}”", name = isolate(base_name(one).to_string())),
        _ => t!("Tags of {n} items", n = glint_model::format::format_count(paths.len())),
    }
}

/// Opens "Edit tags…" for `paths` in `window` (`useTagsEdit.setState({ paths })`). Nothing opens for no paths.
pub fn open(window: &mut Window, cx: &mut App, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let tags = hub(cx);
    let backend = tags.read(cx).backend.clone();
    let shown = title(&paths);
    let model = cx.new(|cx| TagsEdit::new(paths, backend, cx));
    let view = cx.new(|cx| TagsDialog::new(model, tags, window, cx));
    let input = view.read(cx).input().clone();
    show(window, cx, shown, px(448.), view);
    input.update(cx, |input, cx| input.focus(window, cx));
}

// ---- Tests -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::Mutex;
    use std::time::Duration;

    /// What the fake was asked to set.
    type SetCall = (Vec<String>, Vec<String>, Vec<String>);

    /// A backend with tags per path, recording what was set.
    struct Fake {
        tags: Vec<(String, Vec<String>)>,
        read_fails: bool,
        failed: Vec<(String, String)>,
        set_calls: Mutex<Vec<SetCall>>,
    }

    impl Fake {
        fn new(tags: &[(&str, &[&str])]) -> Arc<Self> {
            Arc::new(Self {
                tags: tags
                    .iter()
                    .map(|(p, t)| ((*p).to_string(), t.iter().map(|s| (*s).to_string()).collect()))
                    .collect(),
                read_fails: false,
                failed: Vec::new(),
                set_calls: Mutex::new(Vec::new()),
            })
        }
    }

    impl Backend for Fake {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
            let fails = self.read_fails;
            let known = self.tags.clone();
            Box::pin(async move {
                if fails {
                    return Err("access denied".to_string());
                }
                Ok(paths
                    .iter()
                    .map(|p| {
                        known
                            .iter()
                            .find(|(k, _)| k == p)
                            .map(|(_, t)| t.clone())
                            .unwrap_or_default()
                    })
                    .collect())
            })
        }

        fn set(&self, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Fut<Vec<(String, String)>> {
            if let Ok(mut calls) = self.set_calls.lock() {
                calls.push((paths, add, remove));
            }
            let failed = self.failed.clone();
            Box::pin(async move { Ok(failed) })
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
        panic!("the tags dialog never reached the state the test waits for");
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    /// A window with the dialog layer, the hub on `fake`.
    fn window(cx: &mut TestAppContext, fake: &Arc<Fake>) -> VisualTestContext {
        cx.update(testing::init);
        let backend: Arc<dyn Backend> = fake.clone();
        cx.update(|cx| init_with(cx, backend));
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn box_text(cx: &mut VisualTestContext) -> String {
        cx.update(|window, cx| {
            window.render_frame(cx);
        });
        let view = dialog_input(cx);
        view.read_with(cx, |input, _| input.value().to_string())
    }

    /// The dialog's text box, found through the open dialog's view.
    fn dialog_input(cx: &mut VisualTestContext) -> Entity<InputState> {
        cx.update(|_, cx| {
            let found = DIALOG.with(|d| d.borrow().clone());
            found
                .expect("the dialog was opened by open_dialog_for_test")
                .read(cx)
                .input()
                .clone()
        })
    }

    thread_local! {
        static DIALOG: std::cell::RefCell<Option<Entity<TagsDialog>>> = const { std::cell::RefCell::new(None) };
    }

    /// [`open`], keeping the view so the test can reach its box.
    fn open_dialog_for_test(cx: &mut VisualTestContext, paths: Vec<String>) {
        cx.update(|window, cx| {
            let tags = hub(cx);
            let backend = tags.read(cx).backend.clone();
            let shown = title(&paths);
            let model = cx.new(|cx| TagsEdit::new(paths, backend, cx));
            let view = cx.new(|cx| TagsDialog::new(model, tags, window, cx));
            DIALOG.with(|d| *d.borrow_mut() = Some(view.clone()));
            show(window, cx, shown, px(448.), view);
        });
        cx.run_until_parked();
    }

    fn type_into(cx: &mut VisualTestContext, text: &str) {
        let input = dialog_input(cx);
        cx.update(|window, cx| {
            input.update(cx, |input, cx| input.focus(window, cx));
            window.press("ctrl-a", cx);
            window.input(text, cx);
        });
        cx.run_until_parked();
    }

    #[test]
    fn colors_follow_the_typescript() {
        let _guard = testing::lock();
        // A color tag has its own color, in any case; the rest are picked from the name's hash.
        assert_eq!(tag_color("red"), tag_color("RED"));
        assert_eq!(tag_color("Red"), gpui_kit::rgb(0xef4444).into());
        // JS: hash of "work" = ((119*31+111)*31+114)*31+107 = 3_636_433 -> 3636433 % 8 = 1 -> teal.
        assert_eq!(tag_color("Work"), gpui_kit::rgb(0x14b8a6).into());
        // Arabic and emoji names get a color without trouble.
        let _ = tag_color("ضريبة 2026");
        let _ = tag_color("🙂");
        assert_eq!(tag_color("ضريبة"), tag_color("ضريبة"));
    }

    #[test]
    fn typed_text_shared_tags_toggles_and_offers_read_as_the_typescript_did() {
        assert_eq!(parse_typed(" Work ,, Taxes 2026 ,Red,"), ["Work", "Taxes 2026", "Red"]);
        assert!(parse_typed("  , ").is_empty());
        let lists = vec![
            strings(&["Work", "Red", "A"]),
            strings(&["red", "work"]),
            strings(&["WORK", "Red"]),
        ];
        assert_eq!(shared_tags(&lists), ["Work", "Red"]);
        assert!(shared_tags(&[]).is_empty());
        assert_eq!(toggled(&strings(&["Red"]), "red"), Vec::<String>::new());
        assert_eq!(toggled(&strings(&["Red"]), "Blue"), ["Red", "Blue"]);
        // The color tags first, then recent ones that aren't color tags, at most 16.
        let recent: Vec<String> = (0..30).map(|i| format!("t{i}")).chain(strings(&["green"])).collect();
        let offer = offered(&recent);
        assert_eq!(offer.len(), 16);
        assert_eq!(&offer[..2], ["Red", "Orange"]);
        assert_eq!(offer[7], "t0");
        assert!(!offered(&strings(&["green"])).contains(&"green".to_string()));
    }

    #[test]
    fn remembering_keeps_each_tag_once_newest_first() {
        let mut tags = Tags::new(Fake::new(&[]));
        tags.remember(&strings(&["a", "b"]));
        tags.remember(&strings(&["B", "c"]));
        assert_eq!(tags.recent(), ["B", "c", "a"]);
        let many: Vec<String> = (0..30).map(|i| format!("x{i}")).collect();
        tags.remember(&many);
        assert_eq!(tags.recent().len(), 20);
    }

    #[gpui_kit::test]
    fn the_box_starts_with_what_the_items_share_and_save_adds_and_removes(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[
            ("C:\\a\\one.txt", &["Work", "Red"]),
            ("C:\\a\\two.txt", &["red", "work", "Only two"]),
        ]);
        let mut visual = window(cx, &fake);
        open_dialog_for_test(&mut visual, strings(&["C:\\a\\one.txt", "C:\\a\\two.txt"]));
        // Loading: saving waits until the files have said what they have.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            assert_eq!(window.find("tags-save").disabled(), Some(true));
        });
        settle(&mut visual, |cx| box_text(cx) == "Work, Red");
        assert_eq!(role_of(&mut visual, "Tags"), Some(Role::Group));
        assert_eq!(
            role_of(&mut visual, "Tags, with commas between them"),
            Some(Role::TextInput)
        );
        // Red is on (pressed), Blue is not; clicking Blue adds it, clicking Red takes it off.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("tags-chip", 4_usize), cx);
            window.click(("tags-chip", 0_usize), cx);
        });
        visual.run_until_parked();
        assert_eq!(box_text(&mut visual), "Work, Blue");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("tags-save", cx);
        });
        settle(&mut visual, |_| {
            !fake.set_calls.lock().map(|c| c.is_empty()).unwrap_or(true)
        });
        let calls = fake.set_calls.lock().map(|c| c.clone()).unwrap_or_default();
        assert_eq!(
            calls,
            vec![(
                strings(&["C:\\a\\one.txt", "C:\\a\\two.txt"]),
                strings(&["Work", "Blue"]),
                strings(&["Red"])
            )]
        );
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Save closes the dialog"));
        // What was added is recent now.
        settle(&mut visual, |cx| {
            let tags = cx.update(|_, cx| hub(cx));
            tags.read_with(cx, |t, _| t.recent() == ["Work", "Blue"])
        });
    }

    #[gpui_kit::test]
    fn enter_saves_escape_closes_and_a_failure_is_said(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut fake_value = Fake::new(&[("C:\\a\\one.txt", &[])]);
        // A fake that can't tag: built before sharing.
        if let Some(inner) = Arc::get_mut(&mut fake_value) {
            inner.failed = vec![(
                "C:\\a\\one.txt".into(),
                "This drive keeps no tags (FAT or exFAT)".into(),
            )];
        }
        let fake = fake_value;
        let mut visual = window(cx, &fake);
        // Opened the way the app does, and Escape closes it.
        visual.update(|window, cx| open(window, cx, strings(&["C:\\a\\one.txt"])));
        visual.run_until_parked();
        visual.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        // Typing then Enter saves.
        let changed: std::rc::Rc<std::cell::RefCell<Vec<TagsChanged>>> = Default::default();
        let heard = changed.clone();
        let _subscription = visual.update(|_, cx| {
            let tags = hub(cx);
            cx.subscribe(&tags, move |_, event: &TagsChanged, _| {
                heard.borrow_mut().push(event.clone())
            })
        });
        open_dialog_for_test(&mut visual, strings(&["C:\\a\\one.txt"]));
        settle(&mut visual, |cx| has(cx, "Tags, with commas between them"));
        settle(&mut visual, |cx| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window.find("tags-save").disabled() != Some(true)
            })
        });
        type_into(&mut visual, "Taxes 2026");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.press("enter", cx);
        });
        settle(&mut visual, |_| {
            !fake.set_calls.lock().map(|c| c.is_empty()).unwrap_or(true)
        });
        // Whoever shows tags is told to read these paths again, failed or not.
        settle(&mut visual, |_| !changed.borrow().is_empty());
        assert_eq!(changed.borrow()[0].paths, strings(&[r"C:\a\one.txt"]));
    }

    #[gpui_kit::test]
    fn a_read_that_fails_is_shown_and_saving_still_adds(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut fake_value = Fake::new(&[]);
        if let Some(inner) = Arc::get_mut(&mut fake_value) {
            inner.read_fails = true;
        }
        let fake = fake_value;
        let mut visual = window(cx, &fake);
        open_dialog_for_test(&mut visual, strings(&["C:\\a\\one.txt"]));
        settle(&mut visual, |cx| has(cx, "Couldn't read the tags: access denied"));
        assert_eq!(
            role_of(&mut visual, "Couldn't read the tags: access denied"),
            Some(Role::Alert)
        );
        type_into(&mut visual, "Blue");
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("tags-save", cx);
        });
        settle(&mut visual, |_| {
            !fake.set_calls.lock().map(|c| c.is_empty()).unwrap_or(true)
        });
        let calls = fake.set_calls.lock().map(|c| c.clone()).unwrap_or_default();
        assert_eq!(calls[0].1, ["Blue"]);
        assert!(calls[0].2.is_empty(), "nothing is known to take off");
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render_and_title_the_dialog(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\ملفات\\تقرير.txt", &["مهم", "Red"])]);
        let mut visual = window(cx, &fake);
        testing::set_rtl(true);
        assert_eq!(
            title(&strings(&["C:\\ملفات\\تقرير.txt"])),
            "Tags of “\u{2067}تقرير.txt\u{2069}”"
        );
        open_dialog_for_test(&mut visual, strings(&["C:\\ملفات\\تقرير.txt"]));
        settle(&mut visual, |cx| box_text(cx) == "مهم, Red");
        // The Arabic tag is a chip once it is recent, and the chips keep their names unisolated for readers.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("tags-cancel", cx);
        });
        visual.run_until_parked();
        // Dots for a row: named by their tags, one dot a tag.
        visual.update(|_, cx| {
            assert!(tag_dots("dots", &[], cx).is_none());
        });
        testing::set_rtl(false);
    }

    struct Dots;

    impl Render for Dots {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let tags = strings(&["Red", "مهم", "c", "d", "e", "f"]);
            div().size_full().children(tag_dots("row-dots", &tags, cx))
        }
    }

    #[gpui_kit::test]
    fn a_rows_dots_are_a_named_group_of_at_most_five(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Dots));
        assert_eq!(role_of(&mut visual, "Tags: Red, مهم, c, d, e, f"), Some(Role::Group));
    }
}
