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
//! Where core is called: [`Backend`] has the calls (`tags_read`, `tags_set`, and for marks `marks_read`, `marks_set`)
//! so tests inject a fake.
//!
//! **How the coordinator wires it.** The menus emit `MenuEvent::EditTags { paths }`: call
//! `dialogs::tags::open(window, cx, paths)`. They emit `MenuEvent::SetTags { paths, add, remove }` for a color
//! tag toggled: call `dialogs::tags::set_tags(window, cx, paths, add, remove)`. Whoever draws tag dots in the
//! file list reads the tags itself (`glint_core::tags::tags_read`, a batch of the rows on screen) and
//! subscribes to the [`TagsChanged`] event of [`hub`] to read the changed paths again (`forgetTags`).

use crate::kit::prelude::*;
use crate::explorer::{self, UndoAction, UndoId};
use crate::{core_bridge, t, theme};
use glint_model::format::plural;
use glint_model::marks::{Change, Home, Marks, Outcome};
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

/// The tags the dialog offers: those used lately that are not a label's name (colors are labels now, 5.6: the
/// labels have their own place in the dialog), `MOST_OFFERED` at most.
pub fn offered(recent: &[String], labels: &[glint_model::marks::Label]) -> Vec<String> {
    recent
        .iter()
        .filter(|t| glint_model::marks::label_of(labels, t).is_none())
        .take(MOST_OFFERED)
        .cloned()
        .collect()
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

/// The calls into glint-core's tags.rs and marks (docs/gpui/marks). The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// Each path's tags, in order (`tags_read`).
    fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>>;
    /// Adds `add` and takes `remove` off every path; the paths that couldn't be tagged, each with why (`tags_set`).
    fn set(&self, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) -> Fut<Vec<(String, String)>>;
    /// Each path's marks, in order (`marks_read`). A backend that only knows tags answers each path's tags as
    /// marks.
    fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
        let call = self.read(paths);
        Box::pin(async move {
            let lists = call.await?;
            Ok(lists
                .into_iter()
                .map(|tags| Marks {
                    tags,
                    ..Marks::default()
                })
                .collect())
        })
    }
    /// Changes every path's marks: each one's before and after (`marks_set`). A backend that only knows tags
    /// does a tags change through `set` and answers outcomes with nothing before and nothing after, so there is
    /// nothing to undo; any other change it does not do.
    fn set_marks(&self, paths: Vec<String>, change: Change) -> Fut<Vec<Outcome>> {
        let Change::Tags { add, remove } = change else {
            return Box::pin(async { Ok(Vec::new()) });
        };
        let call = self.set(paths.clone(), add, remove);
        Box::pin(async move {
            let failed = call.await?;
            Ok(paths
                .into_iter()
                .map(|path| {
                    let after = match failed.iter().find(|(bad, _)| *bad == path) {
                        Some((_, why)) => Err(why.clone()),
                        None => Ok((Marks::default(), Home::Stream)),
                    };
                    Outcome {
                        path,
                        before: Marks::default(),
                        after,
                    }
                })
                .collect())
        })
    }
    /// Puts these marks on these paths, whatever they have now (`marks_put`: Undo, paste). Nothing for a backend
    /// that only knows tags.
    fn put_marks(&self, items: Vec<(String, Marks)>) -> Fut<Vec<Outcome>> {
        let _ = items;
        Box::pin(async { Ok(Vec::new()) })
    }
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

    fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
        Box::pin(glint_core::marks::marks_read(paths))
    }

    fn set_marks(&self, paths: Vec<String>, change: Change) -> Fut<Vec<Outcome>> {
        Box::pin(glint_core::marks::marks_set(paths, change))
    }

    fn put_marks(&self, items: Vec<(String, Marks)>) -> Fut<Vec<Outcome>> {
        Box::pin(glint_core::marks::marks_put(items))
    }
}

// ---- What the app shares: recent tags and "tags changed" ---------------------------------------------------

/// Said by [`hub`] after marks (tags, a label, a comment) were changed on `paths` (or tried): whoever shows marks
/// reads them again. Said every time, whatever got written still shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagsChanged {
    /// The paths `set_tags` or `set_marks` was asked to change.
    pub paths: Vec<String>,
}

/// The tags shared by the dialog, the menus and the file list: the call into core, the tags used lately and the
/// event that says tags changed.
///
/// The recent tags are kept in the saved state (`AppState.recent_tags`), so they outlive the session as the
/// TypeScript's `localStorage` key `glint-recent-tags` did; at the first start after the old app they are its own
/// (`persistence::init_at` takes them from `old_settings::imported`).
pub struct Tags {
    backend: Arc<dyn Backend>,
    recent: Vec<String>,
}

impl EventEmitter<TagsChanged> for Tags {}

impl Tags {
    /// A model calling `backend`, with no recent tags.
    pub fn new(backend: Arc<dyn Backend>) -> Self {
        Self::with_recent(backend, Vec::new())
    }

    /// A model calling `backend`, starting from the recent tags kept last time (at most `MOST_RECENT`).
    pub fn with_recent(backend: Arc<dyn Backend>, mut recent: Vec<String>) -> Self {
        recent.truncate(MOST_RECENT);
        Self { backend, recent }
    }

    /// The calls into core's tags, for whoever reads tags for the rows (`components::row_facts`).
    pub fn backend(&self) -> Arc<dyn Backend> {
        self.backend.clone()
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
    let recent = crate::persistence::recent_tags(cx);
    let tags = cx.new(|_| Tags::with_recent(backend, recent));
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

/// Adds `add` to and takes `remove` off the tags of `paths` (`setTags`): [`set_marks`] with a tags change, so a
/// backend that knows marks gives it an Undo step too.
pub fn set_tags(window: &mut Window, cx: &mut App, paths: Vec<String>, add: Vec<String>, remove: Vec<String>) {
    set_marks(window, cx, paths, Change::Tags { add, remove });
}

/// The notification that offers Undo for a change of marks, keyed by its step.
struct MarksToast;

/// How long the Undo offer stays, as the web app's 8 s toast.
const UNDO_FOR: std::time::Duration = std::time::Duration::from_secs(8);

/// What a change of marks is called in "Undid the {label}" and in the notice that offers Undo, for `n` items.
fn change_words(change: &Change, n: usize) -> (String, SharedString) {
    let what = plural(n, "item");
    match change {
        Change::Label(_) => (
            t!("label of {what}", what = what.clone()).to_string(),
            t!("Changed the label of {what}", what = what),
        ),
        Change::Tags { .. } => (
            t!("tags of {what}", what = what.clone()).to_string(),
            t!("Changed the tags of {what}", what = what),
        ),
        Change::Comment(_) => (
            t!("comment of {what}", what = what.clone()).to_string(),
            t!("Changed the comment of {what}", what = what),
        ),
        Change::Put(_) | Change::Paste(_) | Change::Clear => (
            t!("marks of {what}", what = what.clone()).to_string(),
            t!("Changed the marks of {what}", what = what),
        ),
    }
}

/// Says what changed, with an Undo button that takes the step back as Ctrl+Z would (as compress in place does).
fn offer_undo(said: SharedString, step: UndoId, window: &mut Window, cx: &mut App) {
    let key = step.0 as usize;
    let note = Notification::success(said)
        .id1::<MarksToast>(key)
        .action(move |_, _, _| {
            Button::new("marks-undo-toast")
                .label(t!("Undo"))
                .on_click(move |_, window, cx| {
                    window.remove_notification1::<MarksToast>(key, cx);
                    if let Some(explorer) = explorer::try_explorer(cx) {
                        explorer.update(cx, |explorer, cx| explorer.undo(Some(step), cx).detach());
                    }
                })
        });
    window.push_notification(note, cx);
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(UNDO_FOR).await;
        let _ = handle.update(cx, |_, window, cx| window.remove_notification1::<MarksToast>(key, cx));
    })
    .detach();
}

/// What a failed change says: a title and the first reason, or nothing when nothing failed. `tags`: it was a change
/// of tags, which has its own words.
fn failure_words(tags: bool, result: &Result<Vec<Outcome>, String>) -> Option<(SharedString, String)> {
    let outcomes = match result {
        Ok(outcomes) => outcomes,
        Err(reason) => {
            let title = if tags {
                t!("Couldn't change the tags")
            } else {
                t!("Couldn't change the marks")
            };
            return Some((title, reason.clone()));
        }
    };
    let failed: Vec<(&str, &str)> = outcomes
        .iter()
        .filter_map(|o| o.after.as_ref().err().map(|why| (o.path.as_str(), why.as_str())))
        .collect();
    let (first_path, why) = *failed.first()?;
    let title = match (tags, failed.len() == 1) {
        (true, true) => t!("Couldn't tag “{name}”", name = base_name(first_path)),
        (true, false) => t!("Couldn't tag {n}", n = plural(failed.len(), "item")),
        (false, true) => t!("Couldn't mark “{name}”", name = base_name(first_path)),
        (false, false) => t!("Couldn't mark {n}", n = plural(failed.len(), "item")),
    };
    Some((title, why.to_string()))
}

/// Changes the marks of `paths` (docs/gpui/marks, 6.2), on core's runtime. What changed is remembered as one Undo
/// step, with a notice that offers it; what couldn't be changed is said in one notification; the tags added are
/// remembered as recent; and [`TagsChanged`] is emitted every time (it means any mark, and whatever got written
/// still shows).
pub fn set_marks(window: &mut Window, cx: &mut App, paths: Vec<String>, change: Change) {
    let tags = hub(cx);
    let backend = tags.read(cx).backend.clone();
    let call = backend.set_marks(paths.clone(), change.clone());
    window
        .spawn(cx, async move |cx| {
            let result = core_bridge::run(call).await.and_then(|inner| inner);
            cx.update(|window, cx| {
                let tags_change = matches!(change, Change::Tags { .. });
                let outcomes = match &result {
                    Ok(outcomes) => outcomes.as_slice(),
                    Err(_) => &[],
                };
                // Undo (M6): what changed goes back as it was, in one step.
                let items: Vec<(String, Marks)> = outcomes
                    .iter()
                    .filter_map(|o| match &o.after {
                        Ok((marks, _)) if *marks != o.before => Some((o.path.clone(), o.before.clone())),
                        _ => None,
                    })
                    .collect();
                if !items.is_empty() {
                    let (label, said) = change_words(&change, items.len());
                    let step = explorer::try_explorer(cx)
                        .map(|e| e.update(cx, |e, cx| e.remember_undo(label, UndoAction::Marks(items), cx)));
                    if let Some(step) = step {
                        offer_undo(said, step, window, cx);
                    }
                }
                let added = match (&change, &result) {
                    (Change::Tags { add, .. }, Ok(_)) if !add.is_empty() => add.clone(),
                    _ => Vec::new(),
                };
                tags.update(cx, |tags, cx| {
                    if !added.is_empty() {
                        tags.remember(&added);
                    }
                    cx.emit(TagsChanged { paths });
                });
                if !added.is_empty() {
                    let recent = tags.read(cx).recent().to_vec();
                    crate::persistence::set_recent_tags(cx, recent);
                }
                // A failure is words.
                if let Some((title, why)) = failure_words(tags_change, &result) {
                    window.push_notification(Notification::error(why).title(title), cx);
                }
            })
            .ok();
        })
        .detach();
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

/// Every tag in use, each with how many items have it (core's `marks_all`).
pub type Known = Vec<(String, u64)>;

/// Where the known tags come from: a boxed async fn, so tests inject one.
pub type Source = Box<dyn Fn() -> Fut<Known> + Send + Sync>;

actions!(tags_dialog, [TakeSuggestion]);

/// Marks that the dialog's keys are bound in this app.
struct SuggestKeysBound;

impl Global for SuggestKeysBound {}

/// Binds Tab in the dialog's box to taking a suggestion, once per app (the box's own, deeper binding is outranked).
fn bind_suggest_keys(cx: &mut App) {
    if cx.has_global::<SuggestKeysBound>() {
        return;
    }
    cx.set_global(SuggestKeysBound);
    cx.bind_keys([KeyBinding::new("tab", TakeSuggestion, Some("TagsDialog > Input"))]);
}

/// The most tags suggested under the box.
const MOST_SUGGESTED: usize = 8;
/// The most rows of the tab whose marks say which tags are used in this folder.
const MOST_ROWS_LOOKED_AT: usize = 2000;

/// The app's source: every tag in use on this PC, from the index and the ledger.
pub fn core_source() -> Source {
    Box::new(|| Box::pin(async { glint_core::marks::marks_all().await.map(|counts| counts.tags) }))
}

/// The tags used by the rows of the tab the keyboard is in (lower case), from what `RowFacts` holds: nothing is read.
fn tags_here(cx: &App) -> std::collections::HashSet<String> {
    let rows = explorer::try_explorer(cx).zip(crate::components::row_facts::try_shared(cx));
    let Some((explorer, facts)) = rows else {
        return Default::default();
    };
    let explorer = explorer.read(cx);
    let paths = explorer
        .active_tab()
        .map(|tab| tab.entries())
        .unwrap_or_default()
        .iter()
        .take(MOST_ROWS_LOOKED_AT)
        .map(|entry| entry.path.as_str());
    facts
        .read(cx)
        .marks_among(paths)
        .values()
        .flat_map(|marks| marks.tags.iter().map(|tag| tag.to_lowercase()))
        .collect()
}

/// The text before the tag being typed (its tags), and the tag being typed: what follows the last comma.
fn split_typing(text: &str) -> (Vec<String>, &str) {
    match text.rsplit_once(',') {
        Some((done, typing)) => (parse_typed(done), typing.trim()),
        None => (Vec::new(), text.trim()),
    }
}

/// The tags to suggest for the tag being typed in `text`: those that start with it, then those that contain it
/// (any case); within each, those `here` (lower case) first, then the most used first. Tags in `have` (what the items
/// have) and tags already typed are left out; at most [`MOST_SUGGESTED`]. Nothing typed, nothing suggested.
pub fn suggestions(
    text: &str,
    known: &[(String, u64)],
    here: &std::collections::HashSet<String>,
    have: &[String],
) -> Vec<String> {
    let (done, typing) = split_typing(text);
    let needle = typing.to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, bool, std::cmp::Reverse<u64>, &String)> = known
        .iter()
        .filter_map(|(tag, count)| {
            let lower = tag.to_lowercase();
            let tier = if lower.starts_with(&needle) {
                0
            } else if lower.contains(&needle) {
                1
            } else {
                return None;
            };
            if has_tag(have, tag) || has_tag(&done, tag) {
                return None;
            }
            Some((tier, !here.contains(&lower), std::cmp::Reverse(*count), tag))
        })
        .collect();
    // Stable: tags that tie (the recent ones, all with no count) keep their order.
    hits.sort_by_key(|a| (a.0, a.1, a.2));
    hits.into_iter().take(MOST_SUGGESTED).map(|hit| hit.3.clone()).collect()
}

/// `text` with the tag being typed replaced by `tag` and a comma.
fn with_chosen(text: &str, tag: &str) -> String {
    let (mut done, _) = split_typing(text);
    done.push(tag.to_string());
    format!("{}, ", done.join(", "))
}

/// The tags of the items being edited, read from the files.
pub struct TagsEdit {
    paths: Vec<String>,
    shared: Vec<String>,
    /// Every tag any of the items has.
    have: Vec<String>,
    state: State,
    /// The tags in use and their counts, once the source answered something; until then (or when it fails or
    /// has nothing) the recent tags stand in.
    known: Option<Known>,
    /// The tags the rows of this folder use (lower case).
    here: std::collections::HashSet<String>,
}

impl TagsEdit {
    /// Reads the tags of `paths` through `backend`, on core's runtime. It suggests the recent tags.
    pub fn new(paths: Vec<String>, backend: Arc<dyn Backend>, cx: &mut Context<Self>) -> Self {
        Self::start(paths, backend, None, Default::default(), cx)
    }

    /// [`new`], asking `source` once for the tags in use; `here` are the ones this folder uses (lower case).
    pub fn with_source(
        paths: Vec<String>,
        backend: Arc<dyn Backend>,
        source: Source,
        here: std::collections::HashSet<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::start(paths, backend, Some(source), here, cx)
    }

    fn start(
        paths: Vec<String>,
        backend: Arc<dyn Backend>,
        source: Option<Source>,
        here: std::collections::HashSet<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let read = backend.read(paths.clone());
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(read).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(lists) => {
                        this.shared = shared_tags(&lists);
                        for tag in lists.iter().flatten() {
                            if !has_tag(&this.have, tag) {
                                this.have.push(tag.clone());
                            }
                        }
                        this.state = State::Ready;
                    }
                    Err(reason) => this.state = State::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
        if let Some(source) = source {
            let ask = source();
            cx.spawn(async move |this, cx| {
                let known = core_bridge::run(ask).await.and_then(|inner| inner);
                let _ = this.update(cx, |this, cx| {
                    this.known = known.ok().filter(|known| !known.is_empty());
                    cx.notify();
                });
            })
            .detach();
        }
        Self {
            paths,
            shared: Vec::new(),
            have: Vec::new(),
            state: State::Loading,
            known: None,
            here,
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

    /// The tags to suggest for what is typed in the box: from what is in memory, nothing is asked. `recent` stand
    /// in while the tags in use are not known.
    pub fn suggest(&self, text: &str, recent: &[String]) -> Vec<String> {
        match &self.known {
            Some(known) => suggestions(text, known, &self.here, &self.have),
            None => {
                let known: Known = recent.iter().map(|tag| (tag.clone(), 0)).collect();
                suggestions(text, &known, &self.here, &self.have)
            }
        }
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
    /// The suggestion the arrows are on, if they moved at all.
    picked: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl TagsDialog {
    /// A dialog for the items `model` reads.
    pub fn new(model: Entity<TagsEdit>, tags: Entity<Tags>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        bind_suggest_keys(cx);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Work, Taxes 2026, Red")));
        let on_input = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| match event {
            // Enter on a suggestion the arrows chose takes it; otherwise it saves.
            InputEvent::PressEnter { .. } => match this.picked_tag(cx) {
                Some(tag) => this.choose(&tag, window, cx),
                None => this.save(window, cx),
            },
            InputEvent::Change => {
                this.picked = None;
                cx.notify();
            }
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
            picked: None,
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

    /// What is suggested for the tag being typed (worked out from what is in memory).
    fn suggestions(&self, cx: &App) -> Vec<String> {
        let text = self.input.read(cx).value();
        self.model.read(cx).suggest(&text, self.tags.read(cx).recent())
    }

    /// The suggestion the arrows are on.
    fn picked_tag(&self, cx: &App) -> Option<String> {
        self.suggestions(cx).get(self.picked?).cloned()
    }

    /// Puts `tag` in the box in place of the tag being typed, with a comma, and keeps the focus there.
    fn choose(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = with_chosen(&self.input.read(cx).value(), tag);
        self.picked = None;
        self.input.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// Tab takes a suggestion (the one the arrows are on, else the first); with none, Tab is the box's own.
    fn take(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = self.suggestions(cx);
        match list.get(self.picked.unwrap_or(0).min(list.len().saturating_sub(1))) {
            Some(tag) => self.choose(tag, window, cx),
            None => cx.propagate(),
        }
    }

    /// The arrows walk the suggestions.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let list = self.suggestions(cx);
        let last = list.len().saturating_sub(1);
        let key = event.keystroke.key.as_str();
        match key {
            "down" | "up" if !list.is_empty() => {
                self.picked = Some(match (key, self.picked) {
                    ("down", Some(ix)) => (ix + 1).min(last),
                    ("down", None) => 0,
                    (_, Some(ix)) => ix.saturating_sub(1),
                    (_, None) => last,
                });
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
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
        let chips = offered(self.tags.read(cx).recent(), &crate::state::settings(cx).labels);
        let suggested = self.suggestions(cx);
        let picked = self.picked;
        super::main_action(cx, state != State::Loading, |this, window, cx| this.save(window, cx));

        col()
            .id("tags-dialog")
            .key_context("TagsDialog")
            .on_action(cx.listener(|this, _: &TakeSuggestion, window, cx| this.take(window, cx)))
            .role(Role::Group)
            .aria_label(t!("Tags"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            // The dialog layer is outside the workspace's tree, so the start-aligned text is set here too.
            .text_start()
            .pt(space(1.))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
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
            .when(!suggested.is_empty(), |this| {
                this.child(
                    row_start()
                        .id("tags-suggestions")
                        .role(Role::Group)
                        .aria_label(t!("Suggested tags"))
                        .test_support()
                        .flex_wrap()
                        .gap(space(1.))
                        .children(suggested.into_iter().enumerate().map(|(ix, tag)| {
                            let label = SharedString::from(tag.clone());
                            Button::new(("tags-suggestion", ix))
                                .ghost()
                                .compact()
                                .role(Role::Button)
                                .toggled(picked == Some(ix))
                                .accessibility_label(label.clone())
                                .on_click(cx.listener(move |this, _, window, cx| this.choose(&tag, window, cx)))
                                .child(
                                    align_row(1.5)
                                        .text_size(theme::size::XS)
                                        .child(div().size(space(2.)).rounded_full().bg(tag_color(&label)))
                                        .child(isolate(label)),
                                )
                        })),
                )
            })
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
    let first = paths.first().cloned().unwrap_or_default();
    let here = tags_here(cx);
    let model = cx.new(|cx| TagsEdit::with_source(paths, backend, core_source(), here, cx));
    let view = cx.new(|cx| TagsDialog::new(model, tags, window, cx));
    let input = view.read(cx).input().clone();
    super::show_out_of(window, cx, shown, px(448.), view, first);
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
            crate::dialogs::show(window, cx, shown, px(448.), view);
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
        // Recent tags that aren't a label's name (or one it was), at most 16; the colors are labels, not offered.
        let labels = glint_model::marks::default_labels();
        let recent: Vec<String> = (0..30).map(|i| format!("t{i}")).chain(strings(&["green"])).collect();
        let offer = offered(&recent, &labels);
        assert_eq!(offer.len(), 16);
        assert_eq!(&offer[..2], ["t0", "t1"]);
        assert!(!offered(&strings(&["green"]), &labels).contains(&"green".to_string()));
        assert_eq!(offered(&strings(&["work", "Red", "netplan"]), &labels), ["work", "netplan"]);
        assert!(offered(&[], &labels).is_empty());
        let renamed = vec![glint_model::marks::Label {
            name: "Urgent".into(),
            color: 0xef4444,
            was: strings(&["Red"]),
        }];
        assert_eq!(offered(&strings(&["Red", "Urgent", "x"]), &renamed), ["x"]);
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
            ("C:\\a\\one.txt", &["Work", "Old"]),
            ("C:\\a\\two.txt", &["old", "work", "Only two"]),
        ]);
        let mut visual = window(cx, &fake);
        // The chips are the tags used lately (the colors are labels, not offered): Old, then Later.
        visual.update(|_, cx| {
            let tags = hub(cx);
            tags.update(cx, |t, _| t.remember(&strings(&["Later", "Old"])));
        });
        open_dialog_for_test(&mut visual, strings(&["C:\\a\\one.txt", "C:\\a\\two.txt"]));
        // Loading: saving waits until the files have said what they have.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            assert_eq!(window.find("tags-save").disabled(), Some(true));
        });
        settle(&mut visual, |cx| box_text(cx) == "Work, Old");
        assert_eq!(role_of(&mut visual, "Tags"), Some(Role::Group));
        assert_eq!(
            role_of(&mut visual, "Tags, with commas between them"),
            Some(Role::TextInput)
        );
        // Old is on (pressed), Later is not; clicking Later adds it, clicking Old takes it off.
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(("tags-chip", 0_usize), cx);
            window.click(("tags-chip", 1_usize), cx);
        });
        visual.run_until_parked();
        assert_eq!(box_text(&mut visual), "Work, Later");
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
                strings(&["Work", "Later"]),
                strings(&["Old"])
            )]
        );
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Save closes the dialog"));
        // What was added is recent now.
        settle(&mut visual, |cx| {
            let tags = cx.update(|_, cx| hub(cx));
            tags.read_with(cx, |t, _| t.recent().starts_with(&strings(&["Work", "Later"])))
        });
    }

    #[gpui_kit::test]
    fn recent_tags_are_saved_with_the_state_and_come_back_after_a_restart(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let dir = std::env::temp_dir().join(format!("glint-recent-tags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("makes the folder");
        let path = dir.join("state.json");
        let fake = Fake::new(&[(r"C:\a\one.txt", &[])]);
        cx.update(testing::init);
        cx.update(|cx| crate::persistence::init_at(cx, path.clone()));
        let mut visual = window(cx, &fake);
        visual.update(|window, cx| {
            set_tags(
                window,
                cx,
                strings(&[r"C:\a\one.txt"]),
                strings(&["Work", "Blue"]),
                Vec::new(),
            );
        });
        settle(&mut visual, |cx| {
            let tags = cx.update(|_, cx| hub(cx));
            tags.read_with(cx, |t, _| t.recent() == ["Work", "Blue"])
        });
        cx.update(crate::persistence::save_now);
        assert_eq!(crate::persistence::load_from(&path).state.recent_tags, ["Work", "Blue"]);
        // A restart: the state is read again and the shared model starts from it.
        cx.update(|cx| crate::persistence::init_at(cx, path.clone()));
        let backend: Arc<dyn Backend> = fake.clone();
        cx.update(|cx| init_with(cx, backend));
        let tags = cx.update(hub);
        assert_eq!(tags.read_with(cx, |t, _| t.recent().to_vec()), ["Work", "Blue"]);
        let _ = std::fs::remove_dir_all(&dir);
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

    // ---- Suggestions as you type (F2) -----------------------------------------------------------------------------

    fn known(list: &[(&str, u64)]) -> Known {
        list.iter().map(|(tag, n)| ((*tag).to_string(), *n)).collect()
    }

    /// [`open`] with a fake source for the tags in use (or its failure), and `here` the tags this folder uses.
    fn open_suggesting(
        cx: &mut VisualTestContext,
        paths: Vec<String>,
        answer: Result<Known, String>,
        here: &[&str],
    ) {
        cx.update(|window, cx| {
            let tags = hub(cx);
            let backend = tags.read(cx).backend.clone();
            let shown = title(&paths);
            let source: Source = Box::new(move || {
                let answer = answer.clone();
                Box::pin(async move { answer })
            });
            let here = here.iter().map(|tag| tag.to_lowercase()).collect();
            let model = cx.new(|cx| TagsEdit::with_source(paths, backend, source, here, cx));
            let view = cx.new(|cx| TagsDialog::new(model, tags, window, cx));
            DIALOG.with(|d| *d.borrow_mut() = Some(view.clone()));
            crate::dialogs::show(window, cx, shown, px(448.), view);
        });
        cx.run_until_parked();
    }

    /// What the dialog suggests for what is in its box now.
    fn suggested(cx: &mut VisualTestContext) -> Vec<String> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            let view = DIALOG.with(|d| d.borrow().clone()).expect("the dialog was opened by a test helper");
            view.read(cx).suggestions(cx)
        })
    }

    fn press(cx: &mut VisualTestContext, key: &str) {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    }

    /// Waits for the dialog to have read its items and the source to have answered.
    fn wait_for(cx: &mut VisualTestContext, text: &str, wanted: &[&str]) {
        type_into(cx, text);
        settle(cx, |cx| suggested(cx) == wanted);
    }

    const NET: &[(&str, u64)] = &[("netplan", 9), ("network", 3), ("internet", 20), ("work", 50)];

    #[gpui_kit::test]
    fn typing_ne_suggests_the_tags_that_start_with_it_then_those_that_contain_it_the_most_used_first(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\a\\one.txt", &[])]);
        let mut visual = window(cx, &fake);
        open_suggesting(&mut visual, strings(&["C:\\a\\one.txt"]), Ok(known(NET)), &[]);
        wait_for(&mut visual, "ne", &["netplan", "network", "internet"]);
        assert!(has(&mut visual, "Suggested tags"));
        assert!(has(&mut visual, "netplan"));
    }

    #[gpui_kit::test]
    fn a_tag_used_in_this_folder_comes_ahead_of_the_more_used_ones(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\a\\one.txt", &[])]);
        let mut visual = window(cx, &fake);
        open_suggesting(&mut visual, strings(&["C:\\a\\one.txt"]), Ok(known(NET)), &["Network"]);
        wait_for(&mut visual, "ne", &["network", "netplan", "internet"]);
    }

    #[gpui_kit::test]
    fn tags_the_items_have_and_tags_in_the_box_are_not_suggested_and_at_most_eight_show(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\a\\one.txt", &["netmask"])]);
        let mut visual = window(cx, &fake);
        let mut all = known(&[("netmask", 100), ("netbox", 100)]);
        all.extend((1..=10).map(|n| (format!("net{n}"), n)));
        open_suggesting(&mut visual, strings(&["C:\\a\\one.txt"]), Ok(all), &[]);
        settle(&mut visual, |cx| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window.find("tags-save").disabled() != Some(true)
            })
        });
        type_into(&mut visual, "netbox, ne");
        settle(&mut visual, |cx| suggested(cx).len() == 8);
        let list = suggested(&mut visual);
        assert!(!list.iter().any(|tag| tag == "netmask" || tag == "netbox"), "{list:?}");
        assert_eq!(list[0], "net10", "the most used of the rest first");
    }

    #[gpui_kit::test]
    fn choosing_a_suggestion_puts_it_in_the_box_with_a_comma_keeps_the_other_tags_and_typing_goes_on(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\a\\one.txt", &[])]);
        let mut visual = window(cx, &fake);
        open_suggesting(&mut visual, strings(&["C:\\a\\one.txt"]), Ok(known(NET)), &[]);
        wait_for(&mut visual, "Work, ne", &["netplan", "network", "internet"]);
        visual.update(|window, cx| window.click(("tags-suggestion", 0_usize), cx));
        visual.run_until_parked();
        assert_eq!(box_text(&mut visual), "Work, netplan, ");
        // The box keeps the focus: typing goes on at its end.
        visual.update(|window, cx| window.input("x", cx));
        visual.run_until_parked();
        assert_eq!(box_text(&mut visual), "Work, netplan, x");
    }

    #[gpui_kit::test]
    fn the_arrows_and_enter_choose_a_suggestion_without_saving_and_tab_takes_the_first(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\a\\one.txt", &[])]);
        let mut visual = window(cx, &fake);
        open_suggesting(&mut visual, strings(&["C:\\a\\one.txt"]), Ok(known(NET)), &[]);
        wait_for(&mut visual, "ne", &["netplan", "network", "internet"]);
        press(&mut visual, "down");
        press(&mut visual, "down");
        press(&mut visual, "enter");
        assert_eq!(box_text(&mut visual), "network, ");
        assert!(fake.set_calls.lock().map(|c| c.is_empty()).unwrap_or(false), "Enter chose, it didn't save");
        type_into(&mut visual, "ne");
        press(&mut visual, "tab");
        assert_eq!(box_text(&mut visual), "netplan, ");
        // With nothing suggested, Enter saves as it did.
        type_into(&mut visual, "zzz");
        press(&mut visual, "enter");
        settle(&mut visual, |_| !fake.set_calls.lock().map(|c| c.is_empty()).unwrap_or(true));
    }

    #[gpui_kit::test]
    fn a_source_that_fails_leaves_the_recent_tags_to_suggest_and_nothing_typed_offers_what_it_did(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\a\\one.txt", &[])]);
        let mut visual = window(cx, &fake);
        let tags = visual.update(|_, cx| hub(cx));
        tags.update(&mut visual, |tags, _| tags.remember(&strings(&["netflix", "work"])));
        open_suggesting(&mut visual, strings(&["C:\\a\\one.txt"]), Err("instant search is off".into()), &[]);
        wait_for(&mut visual, "net", &["netflix"]);
        // Nothing typed: no suggestions, and the chips are what they were (the color tags, then the recent ones).
        press(&mut visual, "ctrl-a");
        press(&mut visual, "backspace");
        assert!(suggested(&mut visual).is_empty());
        assert!(!has(&mut visual, "Suggested tags"));
        assert!(has(&mut visual, "Tags to add or take off"));
        assert!(has(&mut visual, "netflix"));
    }

    #[gpui_kit::test]
    fn suggestions_have_a_role_and_names_and_an_arabic_tag_is_suggested_and_mirrors(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[("C:\\ملفات\\تقرير.txt", &[])]);
        let mut visual = window(cx, &fake);
        testing::set_rtl(true);
        open_suggesting(
            &mut visual,
            strings(&["C:\\ملفات\\تقرير.txt"]),
            Ok(known(&[("شبكة", 5), ("شبكة عمل", 2)])),
            &[],
        );
        wait_for(&mut visual, "شب", &["شبكة", "شبكة عمل"]);
        assert_eq!(role_of(&mut visual, "Suggested tags"), Some(Role::Group));
        assert_eq!(role_of(&mut visual, "شبكة"), Some(Role::Button));
        assert_eq!(role_of(&mut visual, "شبكة عمل"), Some(Role::Button));
        // Mirrored: the first suggestion sits at the group's end edge, on the right.
        let (group, first) = visual.update(|window, _| {
            (
                window.find("tags-suggestions").bounds(),
                window.find(("tags-suggestion", 0_usize)).bounds(),
            )
        });
        assert!(first.center().x > group.center().x, "{first:?} in {group:?}");
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

    #[test]
    fn the_labels_glint_starts_with_are_the_color_tags() {
        // glint-model can't read this crate's COLOR_TAGS, so it keeps a copy: the two must stay one list.
        let labels: Vec<(String, u32)> = glint_model::marks::default_labels()
            .into_iter()
            .map(|l| (l.name, l.color))
            .collect();
        let tags: Vec<(String, u32)> = COLOR_TAGS.iter().map(|(name, hex)| (name.to_string(), *hex)).collect();
        assert_eq!(labels, tags);
    }

    // ---- Marks: the hub's set_marks, on a backend that knows them ------------------------------------------------

    use crate::explorer::fake::{folder, FakeDisk};
    use crate::explorer::{Explorer, FakeUndoRunner, NavOptions};
    use gpui_kit::AppContext as _;
    use std::collections::HashMap;

    const A: &str = "C:\\a\\a.txt";
    const B: &str = "C:\\a\\b.txt";
    const C: &str = "C:\\a\\c.txt";

    fn marked(label: Option<&str>, tags: &[&str], comment: Option<&str>) -> Marks {
        Marks {
            label: label.map(String::from),
            tags: strings(tags),
            comment: comment.map(String::from),
        }
    }

    /// A backend that knows marks: it holds them by path, applies each change and answers real befores and
    /// afters, can be told to fail a path (with words) or the whole call, and writes down what it was asked.
    #[derive(Default)]
    struct MarksFake {
        marks: Mutex<HashMap<String, Marks>>,
        failing: Mutex<Vec<(String, String)>>,
        call_fails: Mutex<Option<String>>,
        sets: Mutex<Vec<(Vec<String>, Change)>>,
        /// How many times the tags-only `set` was asked: never, for a backend that knows marks.
        tag_sets: Mutex<usize>,
    }

    impl MarksFake {
        fn with(items: &[(&str, Marks)]) -> Arc<Self> {
            let fake = MarksFake::default();
            if let Ok(mut map) = fake.marks.lock() {
                for (path, marks) in items {
                    map.insert((*path).to_string(), marks.clone());
                }
            }
            Arc::new(fake)
        }

        fn fail_path(&self, path: &str, why: &str) {
            if let Ok(mut failing) = self.failing.lock() {
                failing.push((path.to_string(), why.to_string()));
            }
        }

        fn asked(&self) -> Vec<(Vec<String>, Change)> {
            self.sets.lock().map(|s| s.clone()).unwrap_or_default()
        }
    }

    impl Backend for MarksFake {
        fn read(&self, paths: Vec<String>) -> Fut<Vec<Vec<String>>> {
            let known = self.marks.lock().map(|m| m.clone()).unwrap_or_default();
            Box::pin(async move {
                Ok(paths
                    .iter()
                    .map(|p| known.get(p).map(|m| m.tags.clone()).unwrap_or_default())
                    .collect())
            })
        }

        fn set(&self, _: Vec<String>, _: Vec<String>, _: Vec<String>) -> Fut<Vec<(String, String)>> {
            if let Ok(mut n) = self.tag_sets.lock() {
                *n += 1;
            }
            Box::pin(async { Ok(Vec::new()) })
        }

        fn read_marks(&self, paths: Vec<String>) -> Fut<Vec<Marks>> {
            let known = self.marks.lock().map(|m| m.clone()).unwrap_or_default();
            Box::pin(async move { Ok(paths.iter().map(|p| known.get(p).cloned().unwrap_or_default()).collect()) })
        }

        fn set_marks(&self, paths: Vec<String>, change: Change) -> Fut<Vec<Outcome>> {
            if let Ok(mut sets) = self.sets.lock() {
                sets.push((paths.clone(), change.clone()));
            }
            if let Some(why) = self.call_fails.lock().ok().and_then(|f| f.clone()) {
                return Box::pin(async move { Err(why) });
            }
            let failing = self.failing.lock().map(|f| f.clone()).unwrap_or_default();
            let mut held = self.marks.lock();
            let outcomes = paths
                .into_iter()
                .map(|path| {
                    let before = held.as_ref().ok().and_then(|m| m.get(&path).cloned()).unwrap_or_default();
                    let after = match failing.iter().find(|(bad, _)| *bad == path) {
                        Some((_, why)) => Err(why.clone()),
                        None => {
                            let now = change.apply(&before);
                            if let Ok(held) = held.as_mut() {
                                held.insert(path.clone(), now.clone());
                            }
                            Ok((now, Home::Stream))
                        }
                    };
                    Outcome { path, before, after }
                })
                .collect();
            Box::pin(async move { Ok(outcomes) })
        }
    }

    struct Rig {
        visual: VisualTestContext,
        explorer: Option<Entity<Explorer>>,
        runner: Arc<FakeUndoRunner>,
        changed: std::rc::Rc<std::cell::RefCell<Vec<TagsChanged>>>,
        _heard: Subscription,
        _disk: FakeDisk,
    }

    /// A window on `backend`, with an explorer on a fake disk showing C:\\a (or none at all), a fake undo runner,
    /// and a listener for [`TagsChanged`].
    fn rig(cx: &mut TestAppContext, backend: Arc<dyn Backend>, with_explorer: bool) -> Rig {
        cx.update(testing::init);
        cx.update(|cx| init_with(cx, backend));
        let disk = FakeDisk::new();
        disk.folder("C:\\", vec![folder("C:\\", "a")]);
        disk.folder("C:\\a", vec![]);
        let runner = cx.update(FakeUndoRunner::install);
        let explorer = with_explorer.then(|| {
            cx.update(|cx| explorer::init_with(cx, disk.source()));
            let explorer = cx.update(|cx| explorer::explorer(cx));
            let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\a", NavOptions::default(), cx));
            cx.foreground_executor().block_test(task);
            cx.run_until_parked();
            explorer
        });
        let mut visual = testing::open_window(cx, |_, cx| cx.new(|_| Host)).0;
        let changed: std::rc::Rc<std::cell::RefCell<Vec<TagsChanged>>> = Default::default();
        let heard = changed.clone();
        let subscription = visual.update(|_, cx| {
            let tags = hub(cx);
            cx.subscribe(&tags, move |_, event: &TagsChanged, _| heard.borrow_mut().push(event.clone()))
        });
        Rig {
            visual,
            explorer,
            runner,
            changed,
            _heard: subscription,
            _disk: disk,
        }
    }

    impl Rig {
        /// Asks `set_marks` and waits until `TagsChanged` was said `times` times in all.
        fn set(&mut self, paths: &[&str], change: Change, times: usize) {
            let paths = strings(paths);
            self.visual.update(|window, cx| set_marks(window, cx, paths, change));
            let changed = self.changed.clone();
            settle(&mut self.visual, |_| changed.borrow().len() >= times);
        }

        fn next(&mut self) -> Option<(UndoId, String)> {
            let explorer = self.explorer.clone()?;
            explorer.read_with(&self.visual, |e, _| e.next_undo().map(|(id, label)| (id, label.to_string())))
        }

        fn notices(&mut self) -> usize {
            self.visual.update(|window, cx| window.notifications(cx).len())
        }

        /// Takes the newest step back (Ctrl+Z) and waits for the runner to be asked.
        fn undo(&mut self) -> Vec<Vec<(String, Marks)>> {
            let Some(explorer) = self.explorer.clone() else {
                return Vec::new();
            };
            explorer.update(&mut self.visual, |e, cx| e.undo(None, cx).detach());
            let runner = self.runner.clone();
            settle(&mut self.visual, |_| !runner.marks_put().is_empty());
            self.runner.marks_put()
        }
    }

    #[gpui_kit::test]
    fn a_label_on_three_items_is_one_call_one_event_and_one_undo_step_that_puts_each_back(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = MarksFake::with(&[(A, marked(None, &["x"], None)), (B, marked(Some("Blue"), &[], None))]);
        let mut rig = rig(cx, fake.clone(), true);
        assert_eq!(rig.next(), None);
        rig.set(&[A, B, C], Change::Label(Some("Red".into())), 1);
        assert_eq!(fake.asked(), [(strings(&[A, B, C]), Change::Label(Some("Red".into())))]);
        assert_eq!(rig.changed.borrow().as_slice(), [TagsChanged { paths: strings(&[A, B, C]) }]);
        assert_eq!(rig.next().map(|(_, label)| label).as_deref(), Some("label of 3 items"));
        // The step, taken back, puts the three as they were.
        let put = rig.undo();
        assert_eq!(
            put,
            [vec![
                (A.to_string(), marked(None, &["x"], None)),
                (B.to_string(), marked(Some("Blue"), &[], None)),
                (C.to_string(), Marks::default()),
            ]]
        );
        assert_eq!(rig.next(), None, "one step, now used up");
    }

    #[gpui_kit::test]
    fn only_what_changed_is_put_back_and_a_change_that_changed_nothing_leaves_no_step_and_no_notice(
        cx: &mut TestAppContext,
    ) {
        let _guard = testing::lock();
        let fake = MarksFake::with(&[(A, marked(Some("Red"), &[], None))]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.set(&[A, B, C], Change::Label(Some("Red".into())), 1);
        let (step, label) = rig.next().expect("a step");
        assert_eq!(label, "label of 2 items");
        assert_eq!(rig.notices(), 1);
        // Again: all three are Red now. No new step, no new notice, and the event is still said.
        rig.set(&[A, B, C], Change::Label(Some("Red".into())), 2);
        assert_eq!(rig.next(), Some((step, label)));
        assert_eq!(rig.notices(), 1);
        assert_eq!(rig.changed.borrow().len(), 2);
        let put = rig.undo();
        assert_eq!(
            put,
            [vec![(B.to_string(), Marks::default()), (C.to_string(), Marks::default())]]
        );
    }

    #[gpui_kit::test]
    fn each_kind_of_change_names_its_step_in_its_own_words(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = MarksFake::with(&[]);
        let mut rig = rig(cx, fake, true);
        let changes = [
            (Change::Label(Some("Red".into())), "label of 1 item"),
            (
                Change::Tags {
                    add: strings(&["x"]),
                    remove: Vec::new(),
                },
                "tags of 1 item",
            ),
            (Change::Comment(Some("hi".into())), "comment of 1 item"),
            (Change::Put(marked(Some("Blue"), &[], None)), "marks of 1 item"),
            (Change::Clear, "marks of 1 item"),
        ];
        for (n, (change, label)) in changes.into_iter().enumerate() {
            rig.set(&[A], change, n + 1);
            assert_eq!(rig.next().map(|(_, l)| l).as_deref(), Some(label));
        }
    }

    fn outcome(path: &str, after: Result<(), &str>) -> Outcome {
        Outcome {
            path: path.to_string(),
            before: Marks::default(),
            after: after.map(|()| (Marks::default(), Home::Stream)).map_err(String::from),
        }
    }

    fn said(tags: bool, outcomes: Vec<Outcome>) -> Option<(String, String)> {
        failure_words(tags, &Ok(outcomes)).map(|(title, why)| (title.to_string(), why))
    }

    #[test]
    fn a_failure_has_a_title_for_one_item_for_several_and_for_the_call_and_the_first_reason() {
        let _guard = testing::lock();
        let one = vec![outcome(A, Ok(())), outcome(B, Err("This drive keeps no marks"))];
        let two = vec![outcome(A, Err("Access is denied")), outcome(B, Err("Other")), outcome(C, Ok(()))];
        let pair = |a: &str, b: &str| Some((a.to_string(), b.to_string()));
        assert_eq!(said(false, one.clone()), pair("Couldn't mark “b.txt”", "This drive keeps no marks"));
        assert_eq!(said(false, two.clone()), pair("Couldn't mark 2 items", "Access is denied"));
        assert_eq!(said(true, one), pair("Couldn't tag “b.txt”", "This drive keeps no marks"));
        assert_eq!(said(true, two), pair("Couldn't tag 2 items", "Access is denied"));
        assert_eq!(said(false, vec![outcome(A, Ok(()))]), None);
        let gone = Err("The service is gone".to_string());
        let words = |tags| failure_words(tags, &gone).map(|(t, why)| (t.to_string(), why));
        assert_eq!(words(false), pair("Couldn't change the marks", "The service is gone"));
        assert_eq!(words(true), pair("Couldn't change the tags", "The service is gone"));
    }

    #[gpui_kit::test]
    fn a_failure_is_said_in_words_and_what_worked_is_still_undoable(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = MarksFake::with(&[]);
        fake.fail_path(B, "This drive keeps no marks");
        let mut rig = rig(cx, fake.clone(), true);
        rig.set(&[A, B, C], Change::Label(Some("Red".into())), 1);
        assert_eq!(rig.notices(), 2, "the Undo offer and the failure");
        assert_eq!(rig.next().map(|(_, l)| l).as_deref(), Some("label of 2 items"), "the two that worked");
        // Two failing.
        fake.fail_path(C, "Access is denied");
        rig.set(&[A, B, C], Change::Label(Some("Blue".into())), 2);
        assert_eq!(rig.notices(), 4);
        // The call itself failing.
        if let Ok(mut fails) = fake.call_fails.lock() {
            *fails = Some("The service is gone".into());
        }
        rig.set(&[A], Change::Comment(Some("x".into())), 3);
        assert_eq!(rig.notices(), 5, "the call failing says so, and there is nothing to undo");
        // Tags keep their own words.
        let tags = Change::Tags {
            add: strings(&["t"]),
            remove: Vec::new(),
        };
        rig.set(&[A], tags, 4);
        assert_eq!(rig.notices(), 6);
    }

    #[gpui_kit::test]
    fn set_tags_goes_through_marks_with_undo_and_remembers_what_it_added(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = MarksFake::with(&[]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.visual
            .update(|window, cx| set_tags(window, cx, strings(&[A]), strings(&["Work", "Blue"]), strings(&["Red"])));
        let changed = rig.changed.clone();
        settle(&mut rig.visual, |_| !changed.borrow().is_empty());
        assert_eq!(
            fake.asked(),
            [(
                strings(&[A]),
                Change::Tags {
                    add: strings(&["Work", "Blue"]),
                    remove: strings(&["Red"])
                }
            )]
        );
        assert_eq!(*fake.tag_sets.lock().unwrap(), 0, "never the tags-only call");
        let tags = rig.visual.update(|_, cx| hub(cx));
        assert_eq!(tags.read_with(&rig.visual, |t, _| t.recent().to_vec()), ["Work", "Blue"]);
        assert_eq!(rig.next().map(|(_, l)| l).as_deref(), Some("tags of 1 item"));
    }

    #[gpui_kit::test]
    fn set_tags_on_a_backend_that_only_knows_tags_asks_set_and_remembers_no_step(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, &[])]);
        let mut rig = rig(cx, fake.clone(), true);
        rig.visual
            .update(|window, cx| set_tags(window, cx, strings(&[A]), strings(&["Work"]), Vec::new()));
        let changed = rig.changed.clone();
        settle(&mut rig.visual, |_| !changed.borrow().is_empty());
        let calls = fake.set_calls.lock().map(|c| c.clone()).unwrap_or_default();
        assert_eq!(calls, vec![(strings(&[A]), strings(&["Work"]), Vec::new())]);
        assert_eq!(rig.next(), None);
        assert_eq!(rig.notices(), 0, "nothing to offer Undo for");
    }

    #[gpui_kit::test]
    fn a_backend_that_only_knows_tags_answers_marks_from_its_tags_and_nothing_else(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = Fake::new(&[(A, &["Work"])]);
        let backend: &dyn Backend = fake.as_ref();
        fn wait<T>(cx: &TestAppContext, fut: Fut<T>) -> Result<T, String> {
            cx.foreground_executor().block_test(fut)
        }
        let read = wait(cx, backend.read_marks(strings(&[A, B])));
        assert_eq!(read, Ok(vec![marked(None, &["Work"], None), Marks::default()]));
        assert_eq!(wait(cx, backend.set_marks(strings(&[A]), Change::Label(Some("Red".into())))), Ok(Vec::new()));
        assert_eq!(wait(cx, backend.put_marks(vec![(A.to_string(), Marks::default())])), Ok(Vec::new()));
        // A tags change is done through `set`, and the answer has nothing before and nothing after.
        let tags = Change::Tags {
            add: strings(&["t"]),
            remove: Vec::new(),
        };
        let outcomes = wait(cx, backend.set_marks(strings(&[A]), tags)).unwrap_or_default();
        assert_eq!(
            outcomes,
            [Outcome {
                path: A.to_string(),
                before: Marks::default(),
                after: Ok((Marks::default(), Home::Stream))
            }]
        );
        assert_eq!(fake.set_calls.lock().map(|c| c.len()).unwrap_or(0), 1);
    }

    #[gpui_kit::test]
    fn the_notice_offers_undo_and_taking_the_step_back_puts_the_three_befores_back(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = MarksFake::with(&[(A, marked(Some("Blue"), &[], None))]);
        let mut rig = rig(cx, fake, true);
        rig.set(&[A, B, C], Change::Label(Some("Red".into())), 1);
        assert_eq!(rig.notices(), 1);
        let (step, _) = rig.next().expect("a step");
        let explorer = rig.explorer.clone().expect("an explorer");
        explorer.update(&mut rig.visual, |e, cx| e.undo(Some(step), cx).detach());
        let runner = rig.runner.clone();
        settle(&mut rig.visual, |_| !runner.marks_put().is_empty());
        assert_eq!(
            runner.marks_put(),
            [vec![
                (A.to_string(), marked(Some("Blue"), &[], None)),
                (B.to_string(), Marks::default()),
                (C.to_string(), Marks::default()),
            ]]
        );
    }

    #[gpui_kit::test]
    fn with_no_explorer_a_change_still_reaches_the_backend_and_says_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let fake = MarksFake::with(&[]);
        let mut rig = rig(cx, fake.clone(), false);
        rig.set(&[A, B], Change::Clear, 1);
        rig.set(&[A, B], Change::Label(Some("Red".into())), 2);
        assert_eq!(fake.asked().len(), 2);
        assert_eq!(rig.changed.borrow().len(), 2);
        assert_eq!(rig.notices(), 0);
    }
}
