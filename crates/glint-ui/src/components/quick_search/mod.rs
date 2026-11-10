//! The Quick Search window: the shortcut's and the tray's search (QuickSearch.tsx, and the window of quick.rs).
//!
//! Names on every drive as you type, from the instant search index where it runs (fd where it doesn't). Enter
//! opens, Alt+Enter shows the item in Glint, Esc or a click elsewhere puts it away. Before anything is typed it
//! offers the folders used most.
//!
//! | TypeScript / Rust before                | Here                                                                     |
//! |-----------------------------------------|--------------------------------------------------------------------------|
//! | `QuickSearch()` (the page)              | [`QuickSearch`], a GPUI view, in a window of its own                     |
//! | `quick.rs` window, `summon`, `quick_hide` | [`window`]: [`prepare`], [`warm`], [`summon`], [`toggle`], [`hide`], [`close`] |
//! | `findFiles`, `cancelFind`, `openItem`…  | [`Source`] (core in the app, a fake in tests); see [`source`]            |
//! | `wordsOf`, `rank`, `Name`, status text  | [`search`]                                                               |
//! | `usedFolders`, `useZoxide`              | [`search::used_folders`], the zoxide list the view keeps                 |
//! | `quickReveal(path, dir)`                | [`QuickSearchEvent::Reveal`]                                              |
//!
//! # What start-up wires
//!
//! 1. After `actions::init`: `let hub = quick_search::init(cx);` and subscribe to it for
//!    [`QuickSearchEvent::Reveal`]: open `path` in a new tab (a folder), or its folder with the file selected
//!    (`dir == false`), through `Explorer::open_requested`, then bring the main window forward
//!    (`quick_reveal` did exactly that).
//! 2. The shortcut's callback, the tray menu's "Quick search" and a `--quick-search` launch call
//!    [`toggle`] (shortcut, tray: it also puts the window away when it is up and in front) or [`summon`]
//!    (always shows).
//! 3. Once the main window has shown its first folder: [`warm`], which makes the hidden window a second later
//!    if the shortcut or the tray is on, so the first press is instant.
//! 4. Closing the main window without the tray keeping Glint running: [`close`] first, and have the "last window
//!    closed" check leave out [`is_quick_window`], or the hidden window keeps Glint alive.
//!
//! # How the keys work
//!
//! The text field keeps the keyboard the whole time; the arrows, Enter, Tab, Ctrl+C and Escape are bound in the
//! key context `QuickSearch > Input` (deeper than the field's own `Input` bindings, so these win), as the
//! address bar's are. Nothing is bound in `actions::BINDINGS`: they mean something only in this window.

use crate::components::file_grid::thumbnail::kind_icon_filled;
use crate::kit::prelude::*;
use crate::{core_bridge, persistence, state, t, theme};
use glint_model::entries::FileEntry;
use glint_model::file_types::kind_of;
use glint_model::format::format_size;
use glint_model::paths::parent_path;
use glint_model::plain::{plain_query, Plain};
use glint_model::rank::Visit;
use search::{folder_entry, mark_range, used_folders, words_of, USED_FOLDERS};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

pub mod search;
pub mod source;
mod window;

#[cfg(test)]
mod tests;

pub use source::{CoreSource, Found, Fut, Options, Source};
pub use window::{close, hide, init, is_quick_window, is_shown, prepare, summon, toggle, warm, QuickSearchHub, LABEL};

/// How long after the last key a search starts (the TypeScript's 120 ms timer).
const DEBOUNCE: Duration = Duration::from_millis(120);
/// How long "Copied path" stays in the footer.
const COPIED_FOR: Duration = Duration::from_millis(1400);
/// How far the pointer moves with the button down on a result before it is dragged out, in pixels.
const DRAG_AFTER: f32 = 6.;
/// `text-lg`: the query's size. The type scale in `theme::size` stops at `BASE`.
const LG: Rems = rems(1.125);

actions!(
    glint_quick_search,
    [
        /// Down: the next result.
        NextResult,
        /// Up: the previous result.
        PreviousResult,
        /// Enter: open the highlighted result (a folder: in Glint, or in the dialog Quick Search was summoned over).
        OpenResult,
        /// Alt+Enter: show the highlighted result in Glint.
        ShowInGlint,
        /// Ctrl+C with nothing selected in the box: copy the highlighted result's path.
        CopyPath,
        /// Tab: take the filters read from plain words into the box.
        TakeFilters,
        /// Escape: put the window away.
        Dismiss,
    ]
);

/// Marks that the keys are bound in this app.
struct Bound;

impl Global for Bound {}

/// Binds the keys, once per app.
pub(crate) fn bind_keys(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    let field = "QuickSearch > Input";
    cx.bind_keys([
        KeyBinding::new("down", NextResult, Some(field)),
        KeyBinding::new("up", PreviousResult, Some(field)),
        KeyBinding::new("enter", OpenResult, Some(field)),
        KeyBinding::new("alt-enter", ShowInGlint, Some(field)),
        KeyBinding::new("ctrl-c", CopyPath, Some(field)),
        KeyBinding::new("tab", TakeFilters, Some(field)),
        KeyBinding::new("escape", Dismiss, Some(field)),
    ]);
}

/// What the window asks of the rest of Glint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuickSearchEvent {
    /// Put the window away. The window's plumbing does it; nothing else needs to.
    Hide,
    /// Show `path` in Glint (`quickReveal`): a folder (`dir`) opens in a new tab; a file's folder opens with the
    /// file selected. The window has been put away. Heard through the hub [`init`] returns.
    Reveal { path: String, dir: bool },
}

/// A press on a result that may become a drag out of the window.
struct Drag {
    origin: Point<Pixels>,
    path: String,
    started: bool,
}

/// The Quick Search window's content.
pub struct QuickSearch {
    source: Rc<dyn Source>,
    input: Entity<InputState>,
    /// The text in the box as last read, to tell a change from a repeat.
    query: String,
    /// Filters read from plain words ("photos from 2024"), offered under the box: Tab takes them.
    plain: Option<Plain>,
    /// Summoned over an Open or Save As dialog: a folder chosen goes there.
    over_dialog: bool,
    /// The best results of the current query, best first.
    found: Vec<FileEntry>,
    /// The folders used most, shown before anything is typed.
    recent: Vec<FileEntry>,
    /// zoxide's folders with their scores (empty while history isn't shared with it).
    zoxide: Vec<(String, f64)>,
    /// The drives searched, read when summoned.
    roots: Vec<String>,
    status: SharedString,
    busy: bool,
    active: usize,
    copied: bool,
    /// Numbers the searches, so an answer to an older one is dropped.
    serial: u64,
    search: Option<Task<()>>,
    copied_timer: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    drag: Option<Drag>,
    /// The window has been the active one since it was last shown: losing that puts it away.
    was_active: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<QuickSearchEvent> for QuickSearch {}

impl QuickSearch {
    /// The view, asking glint-core.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let source: Rc<dyn Source> = Rc::new(CoreSource::new(cx.global::<core_bridge::Core>().app.clone()));
        Self::with_source(source, window, cx)
    }

    /// The view, asking `source` (tests give a fake).
    pub fn with_source(source: Rc<dyn Source>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search every drive…")));
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.query_changed(cx);
            }
        }));
        // A click elsewhere (the window loses the foreground) puts it away; gaining it gives the box the keyboard.
        subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| this.activation_changed(window, cx)));
        // A folder opened in the main window shows in the list at once.
        subscriptions.push(cx.observe_global::<persistence::Persistence>(|this, cx| this.refresh_recent(cx)));
        let mut view = Self {
            source,
            input,
            query: String::new(),
            plain: None,
            over_dialog: false,
            found: Vec::new(),
            recent: Vec::new(),
            zoxide: Vec::new(),
            roots: Vec::new(),
            status: SharedString::default(),
            busy: false,
            active: 0,
            copied: false,
            serial: 0,
            search: None,
            copied_timer: None,
            scroll: UniformListScrollHandle::new(),
            drag: None,
            was_active: false,
            _subscriptions: subscriptions,
        };
        view.refresh_recent(cx);
        view
    }

    // ---- What is shown --------------------------------------------------------------------------------

    /// What the list holds: the results of the query, or before anything is typed the folders used most.
    fn shown_entries(&self) -> &[FileEntry] {
        if self.query.trim().is_empty() {
            &self.recent
        } else {
            &self.found
        }
    }

    /// The highlighted row, kept inside the list.
    fn active_row(&self) -> usize {
        self.active.min(self.shown_entries().len().saturating_sub(1))
    }

    // ---- Being summoned and put away ------------------------------------------------------------------

    /// The window has been brought up (each time): the settings and folders as the main window left them, the
    /// drives as they are now, and the last query selected, to type over (`ready()`).
    pub fn shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.over_dialog = self.source.over_dialog();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(t!("Search every drive…"), window, cx);
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        if state::settings(cx).zoxide {
            let asked = self.source.zoxide();
            cx.spawn(async move |this, cx| {
                let folders = asked.await;
                this.update(cx, |this, cx| {
                    this.zoxide = folders;
                    this.refresh_recent(cx);
                })
                .ok();
            })
            .detach();
        } else if !self.zoxide.is_empty() {
            self.zoxide.clear();
        }
        let asked = self.source.drives();
        cx.spawn(async move |this, cx| {
            let drives = asked.await;
            this.update(cx, |this, _| {
                if !drives.is_empty() {
                    this.roots = drives;
                }
            })
            .ok();
        })
        .detach();
        self.refresh_recent(cx);
    }

    fn activation_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_window_active() {
            self.was_active = true;
            self.input.update(cx, |input, cx| input.focus(window, cx));
        } else if self.was_active {
            self.hide(cx);
        }
    }

    /// Asks for the window to be put away (`quickHide`).
    fn hide(&mut self, cx: &mut Context<Self>) {
        self.was_active = false;
        cx.emit(QuickSearchEvent::Hide);
    }

    /// Shows `path` in Glint and puts the window away (`quickReveal`).
    fn reveal(&mut self, path: String, dir: bool, cx: &mut Context<Self>) {
        self.was_active = false;
        cx.emit(QuickSearchEvent::Reveal { path, dir });
    }

    // ---- The folders used most -------------------------------------------------------------------------

    /// Recomputes the folders shown before anything is typed: Glint's visits and zoxide's folders together.
    fn refresh_recent(&mut self, cx: &mut Context<Self>) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let visits: Vec<(&str, Visit)> = persistence::app(cx)
            .map(|saved| saved.visits.iter().map(|(path, visit)| (path, *visit)).collect())
            .unwrap_or_default();
        let folders = used_folders(visits, &self.zoxide, USED_FOLDERS, now);
        let recent: Vec<FileEntry> = folders.iter().map(|path| folder_entry(path)).collect();
        if recent != self.recent {
            self.recent = recent;
            cx.notify();
        }
    }

    // ---- Typing and searching --------------------------------------------------------------------------

    /// The box's text changed: read it, offer the filters its plain words make, and search after a pause.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();
        if value == self.query {
            return;
        }
        self.query = value;
        self.plain = plain_query(&self.query);
        self.search_after_a_pause(cx);
        cx.notify();
    }

    /// Searches for the box's text 120 ms after the last key; with nothing typed, ends the search and clears the
    /// results.
    fn search_after_a_pause(&mut self, cx: &mut Context<Self>) {
        self.serial += 1;
        let serial = self.serial;
        let text = self.query.trim().to_string();
        if text.is_empty() {
            self.search = None;
            self.source.cancel();
            self.found.clear();
            self.status = SharedString::default();
            self.busy = false;
            self.active = 0;
            return;
        }
        let source = self.source.clone();
        self.search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let started = this.update(cx, |this, cx| {
                this.busy = true;
                this.active = 0;
                cx.notify();
                let settings = state::settings(cx);
                let options = Options {
                    hidden: settings.show_hidden,
                    exclude: settings.find_exclusions.clone(),
                };
                (this.roots.clone(), options)
            });
            let Ok((known, options)) = started else { return };
            let roots = if known.is_empty() {
                let drives = source.drives().await;
                let kept = drives.clone();
                if this.update(cx, |this, _| this.roots = kept).is_err() {
                    return;
                }
                drives
            } else {
                known
            };
            let receiver = match source.find(roots, text, options).await {
                Ok(receiver) => receiver,
                Err(message) => {
                    this.update(cx, |this, cx| {
                        if this.serial == serial {
                            this.busy = false;
                            this.status = message.into();
                            cx.notify();
                        }
                    })
                    .ok();
                    return;
                }
            };
            let mut engine = String::new();
            while let Ok(first) = receiver.recv().await {
                // Everything that arrived meanwhile is applied together: one redraw a frame at most.
                let mut batch = vec![first];
                while let Ok(more) = receiver.try_recv() {
                    batch.push(more);
                }
                let current = this.update(cx, |this, cx| {
                    if this.serial != serial {
                        return false;
                    }
                    for found in batch {
                        this.apply(found, &mut engine);
                    }
                    cx.notify();
                    true
                });
                if !matches!(current, Ok(true)) {
                    return;
                }
            }
            // The search ended without saying so (core went away): don't leave the spinner turning.
            this.update(cx, |this, cx| {
                if this.serial == serial && this.busy {
                    this.busy = false;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// One thing the search said.
    fn apply(&mut self, found: Found, engine: &mut String) {
        match found {
            Found::Started { engine: who } => *engine = who,
            Found::Results(results) => self.found = results,
            Found::Error(message) => {
                self.busy = false;
                self.status = message.into();
            }
            // NOTE(port): the TypeScript did the same: a search that ended in an error says "0 found" over the
            // error's text, since `finished` follows `error`. Its message only shows if nothing follows.
            Found::Finished {
                count,
                elapsed,
                limited,
                fuzzy,
            } => {
                self.busy = false;
                self.status = search::status_line(engine, fuzzy, count, limited, elapsed);
            }
        }
    }

    // ---- Keys ---------------------------------------------------------------------------------------------

    /// Up and Down (clamped at the ends, the TypeScript's `Math.min`/`Math.max`).
    fn move_active(&mut self, by: isize, cx: &mut Context<Self>) {
        let len = self.shown_entries().len();
        if len == 0 {
            return;
        }
        let next = self.active_row().saturating_add_signed(by).min(len - 1);
        self.set_active(next, cx);
        self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
    }

    fn set_active(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.active != row {
            self.active = row;
            cx.notify();
        }
    }

    /// Enter and Alt+Enter on the highlighted row.
    fn open_active(&mut self, in_glint: bool, cx: &mut Context<Self>) {
        if let Some(entry) = self.shown_entries().get(self.active_row()).cloned() {
            self.open(entry, in_glint, cx);
        }
    }

    /// Folders open in Glint (or, summoned over an Open or Save As dialog, in that dialog); files with their app,
    /// or in Glint with Alt.
    fn open(&mut self, entry: FileEntry, in_glint: bool, cx: &mut Context<Self>) {
        if entry.dir && !in_glint && self.over_dialog {
            let jumping = self.source.jump(entry.path.clone());
            cx.spawn(async move |this, cx| {
                let jumped = jumping.await;
                this.update(cx, |this, cx| {
                    if jumped {
                        this.hide(cx);
                    } else {
                        this.reveal(entry.path, true, cx);
                    }
                })
                .ok();
            })
            .detach();
            return;
        }
        if in_glint || entry.dir {
            self.reveal(entry.path, entry.dir, cx);
            return;
        }
        self.hide(cx);
        let opening = self.source.open(entry.path.clone());
        cx.spawn(async move |this, cx| match opening.await {
            // A shortcut to a folder opens the folder in Glint.
            Ok(Some(folder)) => {
                this.update(cx, |this, cx| this.reveal(folder, true, cx)).ok();
            }
            Ok(None) => {}
            // It wouldn't open: show where it is.
            Err(_) => {
                this.update(cx, |this, cx| this.reveal(entry.path, false, cx)).ok();
            }
        })
        .detach();
    }

    /// Tab: the filters read from plain words go in the box.
    fn take_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(query) = self.plain.as_ref().map(|plain| plain.query.clone()) else {
            // Nothing to take: Tab does what it does in the box.
            cx.propagate();
            return;
        };
        self.input.update(cx, |input, cx| {
            input.set_value(query, window, cx);
            input.focus(window, cx);
        });
        self.query_changed(cx);
    }

    /// Ctrl+C with nothing selected in the box: the highlighted result's path goes to the clipboard.
    fn copy_path(&mut self, cx: &mut Context<Self>) {
        let nothing_selected = self.input.read(cx).selected_value().is_empty();
        let Some(entry) = self.shown_entries().get(self.active_row()).filter(|_| nothing_selected) else {
            // A selection in the box is copied as usual.
            cx.propagate();
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(entry.path.clone()));
        self.copied = true;
        self.copied_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPIED_FOR).await;
            this.update(cx, |this, cx| {
                this.copied = false;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    // ---- The pointer ----------------------------------------------------------------------------------

    /// The pointer moved over row `row`: it is the highlighted one, and a press that has moved far enough is
    /// dragged out of the window (`startDrag`).
    fn row_moved(&mut self, row: usize, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.set_active(row, cx);
        if event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let Some(drag) = self.drag.as_mut().filter(|drag| !drag.started) else {
            return;
        };
        let (dx, dy) = (
            f32::from(event.position.x - drag.origin.x),
            f32::from(event.position.y - drag.origin.y),
        );
        if dx.hypot(dy) < DRAG_AFTER {
            return;
        }
        drag.started = true;
        // The drag's loop runs from the message pump (core posts it); this resolves when it ends.
        let dragging = self.source.drag(cx, vec![drag.path.clone()]);
        cx.spawn(async move |this, cx| {
            let _ = dragging.await;
            this.update(cx, |this, _| this.drag = None).ok();
        })
        .detach();
    }

    // ---- Drawing ----------------------------------------------------------------------------------------

    /// The rows in view: `row` of each, in `range`.
    fn rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme::tokens(cx).clone();
        let words = words_of(&self.query);
        let active = self.active_row();
        let entries: Vec<(usize, FileEntry)> = range
            .filter_map(|row| Some((row, self.shown_entries().get(row)?.clone())))
            .collect();
        entries
            .into_iter()
            .map(|(row, entry)| self.row(row, entry, row == active, &words, &t, cx))
            .collect()
    }

    /// A result: its kind's icon, its name with the part that matched marked, the folder it is in, and its size.
    fn row(
        &self,
        row: usize,
        entry: FileEntry,
        active: bool,
        words: &[String],
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = kind_of(&entry);
        let name = isolate(entry.name.clone());
        // The isolate puts a mark before the name (and one after it): the marked range moves past the first.
        let shift = if name.len() == entry.name.len() {
            0
        } else {
            '\u{2067}'.len_utf8()
        };
        let marked = mark_range(&entry.name, words).map(|marked| {
            (
                marked.start + shift..marked.end + shift,
                HighlightStyle {
                    color: Some(t.primary),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                },
            )
        });
        let folder = parent_path(&entry.path);
        let (path, down_path) = (entry.clone(), entry.path.clone());
        row_start()
            .id(("quick-search-row", row))
            .role(Role::ListBoxOption)
            .aria_label(entry.name.clone())
            .aria_description(folder.clone())
            .aria_selected(active)
            .when(active, |row| row.aria_active_descendant())
            .test_support()
            .gap(space(3.))
            .h(space(12.))
            .w_full()
            .px(space(3.))
            .rounded(theme::size::RADIUS)
            .cursor_pointer()
            .when(active, |row| row.bg(t.accent))
            // The box keeps the keyboard when a result is pressed; the press may become a drag out.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, _| {
                    window.prevent_default();
                    this.drag = Some(Drag {
                        origin: event.position,
                        path: down_path.clone(),
                        started: false,
                    });
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| this.row_moved(row, event, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.drag = None;
                }),
            )
            .on_click(
                cx.listener(move |this, event: &ClickEvent, _, cx| this.open(path.clone(), event.modifiers().alt, cx)),
            )
            .child(kind_icon_filled(kind, theme::rem_size(cx), cx))
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_size(theme::size::SM)
                            .child(StyledText::new(name).with_highlights(marked)),
                    )
                    .child(
                        div()
                            .max_w_full()
                            .truncate()
                            .text_size(theme::size::XS)
                            .text_color(t.muted_foreground)
                            .child(isolate(folder)),
                    ),
            )
            .when(!entry.dir, |row| {
                row.child(
                    div()
                        .flex_none()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(format_size(entry.size)),
                )
            })
            .into_any_element()
    }

    /// A key and what it does, in the footer: the key's cap, then the words. `keys` is the keystroke; `name` is
    /// how a screen reader says it.
    fn hint(n: usize, keys: &'static str, name: &'static str, what: SharedString) -> impl IntoElement {
        row_start()
            .id(("quick-search-hint", n))
            .role(Role::Group)
            .aria_label(format!("{name}: {what}"))
            .test_support()
            .flex_none()
            .gap(space(1.))
            .children(Keystroke::parse(keys).ok().map(Kbd::new))
            .child(what)
    }
}

impl Render for QuickSearch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let typed = !self.query.trim().is_empty();
        let count = self.shown_entries().len();

        // The box. The icon beside it is the window's drag handle: the box itself is for typing.
        let lead = div()
            .id("quick-search-lead")
            .test_support()
            .window_control_area(WindowControlArea::Drag)
            .flex_none()
            .child(if self.busy {
                Spinner::new().large().color(t.primary).into_any_element()
            } else {
                icon(IconName::Search)
                    .size_5()
                    .text_color(t.muted_foreground)
                    .into_any_element()
            });
        let header = row_start()
            .flex_none()
            .gap(space(3.))
            .px(space(4.))
            .border_b_1()
            .border_color(t.border)
            .child(lead)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(space(14.))
                    .flex()
                    .items_center()
                    .text_size(LG)
                    .child(
                        Input::new(&self.input)
                            .appearance(false)
                            .large()
                            .aria_label(t!("Search file names on every drive")),
                    ),
            );

        // Plain words read as filters ("photos from 2024"), offered under the box: Tab takes them.
        let plain = self.plain.as_ref().map(|plain| {
            let chips = plain.chips.clone();
            row_start()
                .id("quick-search-plain")
                .role(Role::Button)
                .aria_label(t!("Use these filters: {filters}", filters = chips.join(", ")))
                .test_support()
                .flex_none()
                .gap(space(2.))
                .px(space(4.))
                .py(space(2.))
                .border_b_1()
                .border_color(t.border)
                .text_size(theme::size::XS)
                .cursor_pointer()
                .hover(|s| s.bg(t.accent))
                .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                .on_click(cx.listener(|this, _, window, cx| this.take_filters(window, cx)))
                .children(Keystroke::parse("tab").ok().map(Kbd::new))
                .child(
                    row_start()
                        .min_w_0()
                        .flex_wrap()
                        .gap(space(1.))
                        .children(chips.into_iter().map(|chip| {
                            div()
                                .rounded(theme::size::RADIUS_SM)
                                .bg(t.primary.opacity(0.12))
                                .px(space(1.5))
                                .py(space(0.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.primary)
                                .child(chip)
                        })),
                )
        });

        // The results, or what to do when there are none.
        let list = if count > 0 {
            div()
                .flex_1()
                .min_h_0()
                .relative()
                .child(
                    uniform_list(
                        "quick-search-rows",
                        count,
                        cx.processor(|this, range: Range<usize>, _, cx| this.rows(range, cx)),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .vertical_scrollbar(&self.scroll)
                .into_any_element()
        } else {
            let said = if !typed {
                t!("Type part of a name. Filters work too: ext:pdf, size:>10mb, dm:7d")
            } else if self.busy {
                t!("Searching…")
            } else if self.status.is_empty() {
                t!("Nothing found")
            } else {
                self.status.clone()
            };
            div()
                .id("quick-search-empty")
                .role(Role::Status)
                .aria_label(said.clone())
                .test_support()
                .px(space(3.))
                .py(space(8.))
                .text_center()
                .text_size(theme::size::SM)
                .text_color(t.muted_foreground)
                .child(said)
                .into_any_element()
        };
        // The heading is outside the list, which holds only results.
        let results = col()
            .flex_1()
            .min_h_0()
            .p(space(1.5))
            .when(!typed && !self.recent.is_empty(), |results| {
                results.child(
                    div()
                        .id("quick-search-heading")
                        .role(Role::Heading)
                        .aria_label(t!("Folders you use most"))
                        .test_support()
                        .flex_none()
                        .px(space(3.))
                        .pt(space(1.5))
                        .pb(space(1.))
                        .text_size(theme::size::XS)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.muted_foreground)
                        .child(t!("Folders you use most")),
                )
            })
            .child(
                col()
                    .id("quick-search-results")
                    .role(Role::ListBox)
                    .aria_label(t!("Results"))
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .child(list),
            );

        // The footer: how the search went (or that the path was copied), then the keys.
        let note: AnyElement = if self.copied {
            row_start()
                .id("quick-search-copied")
                .role(Role::Status)
                .aria_label(t!("Copied path"))
                .test_support()
                .gap(space(1.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.success)
                .child(icon(IconName::Check).size_3p5())
                .child(t!("Copied path"))
                .into_any_element()
        } else if !self.found.is_empty() {
            div()
                .id("quick-search-status")
                .role(Role::Status)
                .aria_label(self.status.clone())
                .test_support()
                .truncate()
                .child(self.status.clone())
                .into_any_element()
        } else {
            div().into_any_element()
        };
        let footer = row_start()
            .flex_none()
            .h(space(8.))
            .gap(space(4.))
            .px(space(4.))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(div().flex_1().min_w_0().child(note))
            .child(Self::hint(
                0,
                "enter",
                "Enter",
                if self.over_dialog {
                    t!("open (a folder: in the dialog)")
                } else {
                    t!("open")
                },
            ))
            .child(Self::hint(1, "alt-enter", "Alt+Enter", t!("show in Glint")))
            .child(Self::hint(2, "ctrl-c", "Ctrl+C", t!("copy path")))
            .child(Self::hint(3, "escape", "Esc", t!("close")));

        col()
            .id("quick-search")
            .role(Role::Group)
            .aria_label(t!("Glint quick search"))
            .test_support()
            .key_context("QuickSearch")
            .size_full()
            .overflow_hidden()
            .border_1()
            .border_color(t.border)
            .bg(t.background)
            .text_color(t.foreground)
            .text_start()
            .on_action(cx.listener(|this, _: &NextResult, _, cx| this.move_active(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousResult, _, cx| this.move_active(-1, cx)))
            .on_action(cx.listener(|this, _: &OpenResult, _, cx| this.open_active(false, cx)))
            .on_action(cx.listener(|this, _: &ShowInGlint, _, cx| this.open_active(true, cx)))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| this.copy_path(cx)))
            .on_action(cx.listener(|this, _: &TakeFilters, window, cx| this.take_filters(window, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| this.hide(cx)))
            .child(header)
            .children(plain)
            .child(results)
            .child(footer)
    }
}
