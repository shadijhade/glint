//! Columns: the folder above, the list, and what is in the focused folder, side by side (ColumnsView.tsx).
//!
//! | TypeScript                                 | Here                                                               |
//! |--------------------------------------------|--------------------------------------------------------------------|
//! | `useListing(path, showHidden)`             | [`Side`]'s `load` (the folder listed in the background, hidden items left out unless shown) |
//! | `Row`                                      | the rows [`ColumnsView`] draws (virtualized, one `uniform_list` a column) |
//! | `ParentColumn`                             | the first column, from the tab's parent folder                     |
//! | `NextColumn`                               | the last column, from the list's focused item                      |
//! | the `<div className="flex ...">` in Explorer.tsx | [`ColumnsView`]: the two columns around a slot for the pane's list |
//!
//! # How the coordinator wires it
//!
//! - **Make it once per pane**: `cx.new(|cx| ColumnsView::new(pane, window, cx))`, then give it the pane's file list
//!   (the TypeScript put `FileList` between the columns) with [`ColumnsView::set_center`]`(file_list.clone().into())`.
//!   Place the `ColumnsView` where the pane draws its list while the tab's view is `ViewMode::Columns`; it draws
//!   the three side by side and sizes the middle one to the rest. The list keeps its own key handling, which
//!   already does Left (`go_up`) and Right (into the focused folder) in this view.
//! - **It needs nothing else**: it reads the pane's tab (path, rows, focus) and the show-hidden setting, lists the
//!   two folders itself through glint-core (a second [`CoreSource`], made from the `Core` global), and opens
//!   things through the explorer in the pane's tab. It emits no events.
//! - **Keyboard**: each side column is a tab stop. Up, Down, Home, End, PageUp and PageDown move its cursor;
//!   Enter or Space opens the row under it; Left goes to the folder above (as the TypeScript's global handler);
//!   Right opens the row under the cursor if it is a folder.
//!
//! # Differences from the TypeScript
//!
//! - The task text for this wave described the columns as running from the root down to the open folder. The
//!   TypeScript has exactly two side columns (the folder above and the focused folder), so this follows the
//!   TypeScript.
//! - The folder above is scrolled to the open folder when it arrives (the TypeScript left it at the top).
//! - The side columns' rows are named with their column ("name, in the folder above") so a name that appears in
//!   two columns, or in the list, can be told apart; the full path is the description.
//! - The listing is filtered again when "show hidden" changes rather than listed again.

#[cfg(test)]
mod tests;

use crate::explorer::{explorer, CoreSource, Explorer, Fut, ListFailure, Listed, NavOptions, Source, Tab};
use crate::kit::prelude::*;
use crate::{i18n, state, t, theme};
use glint_model::entries::{is_hidden, FileEntry};
use glint_model::format::{format_count, format_size};
use glint_model::paths::{base_name, is_folder_path, is_root, parent_path, same_path, THIS_PC};
use gpui_kit::{ScrollStrategy, UniformListScrollHandle};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// How a folder is listed: glint-core's in the app, a fake disk's in tests.
pub type Lister = Rc<dyn Fn(&str) -> Fut<Result<Listed, ListFailure>>>;

/// The parent column's width (`w-56`) and the next column's (`w-64`), in rems.
const PARENT_WIDTH: f32 = 14.;
const NEXT_WIDTH: f32 = 16.;

/// The folder above a tab's path, or none at This PC and at a root (`ParentColumn`'s `parent`).
pub fn parent_of(path: &str) -> Option<String> {
    if path == THIS_PC || is_root(path) {
        None
    } else {
        Some(parent_path(path))
    }
}

/// Which side column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Parent,
    Next,
}

/// One side column's folder and what it lists.
struct Side {
    /// The folder shown (not necessarily a real one: a search or the Recycle Bin lists nothing).
    path: Option<String>,
    /// As listed, until the next listing replaces it. `None` while it is read.
    raw: Option<Arc<[FileEntry]>>,
    /// The indices into `raw` that show: hidden items only when the setting says so.
    shown: Vec<usize>,
    /// Each listing is numbered; a result that comes back with an older number is dropped.
    seq: u64,
    scroll: UniformListScrollHandle,
    /// The row the keyboard is on.
    cursor: usize,
    focus: FocusHandle,
}

impl Side {
    fn new(cx: &mut Context<ColumnsView>) -> Self {
        Side {
            path: None,
            raw: None,
            shown: Vec::new(),
            seq: 0,
            scroll: UniformListScrollHandle::new(),
            cursor: 0,
            focus: cx.focus_handle().tab_stop(true),
        }
    }

    fn entry(&self, shown_index: usize) -> Option<&FileEntry> {
        self.shown.get(shown_index).and_then(|&i| self.raw.as_ref()?.get(i))
    }

    /// Which items show, for `show_hidden`.
    fn filter(&mut self, show_hidden: bool) {
        self.shown = match &self.raw {
            Some(raw) => (0..raw.len())
                .filter(|&i| show_hidden || raw.get(i).is_some_and(|e| !is_hidden(e)))
                .collect(),
            None => Vec::new(),
        };
    }
}

/// The columns view of one pane.
pub struct ColumnsView {
    pane: usize,
    lister: Lister,
    /// The pane's list (or anything) between the two columns.
    center: Option<AnyView>,
    parent: Side,
    next: Side,
    /// The tab's path, to mark the open folder in the folder above.
    tab_path: String,
    /// The list's focused item: what the last column shows (`NextColumn`'s `entry`).
    focused: Option<FileEntry>,
    show_hidden: bool,
    _subscriptions: Vec<Subscription>,
}

impl ColumnsView {
    /// The columns for `pane`, listing folders through glint-core.
    pub fn new(pane: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let lister: Lister = match cx.try_global::<crate::core_bridge::Core>() {
            Some(core) => {
                let source = CoreSource::new(core.app.clone());
                Rc::new(move |path: &str| source.list(path))
            }
            None => Rc::new(|path: &str| {
                let failure = ListFailure::other(path, "Glint's engine isn't running");
                Box::pin(async move { Err(failure) })
            }),
        };
        Self::with_lister(pane, lister, window, cx)
    }

    /// The columns listing folders from `source` (tests, tools).
    pub fn with_source(pane: usize, source: Rc<dyn Source>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::with_lister(pane, Rc::new(move |path: &str| source.list(path)), window, cx)
    }

    /// The columns listing folders with `lister`.
    pub fn with_lister(pane: usize, lister: Lister, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        let subscriptions = vec![
            cx.observe(&explorer, |this, _, cx| this.sync(cx)),
            cx.observe_global::<state::UiState>(|this, cx| this.sync(cx)),
        ];
        let mut this = ColumnsView {
            pane,
            lister,
            center: None,
            parent: Side::new(cx),
            next: Side::new(cx),
            tab_path: String::new(),
            focused: None,
            show_hidden: state::settings(cx).show_hidden,
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    /// The view drawn between the two columns: the pane's file list.
    pub fn set_center(&mut self, view: AnyView, cx: &mut Context<Self>) {
        self.center = Some(view);
        cx.notify();
    }

    /// The first column's focus handle (the next column's is [`ColumnsView::next_focus`]).
    pub fn parent_focus(&self) -> FocusHandle {
        self.parent.focus.clone()
    }

    /// The last column's focus handle.
    pub fn next_focus(&self) -> FocusHandle {
        self.next.focus.clone()
    }

    /// The folder the first column lists, if any.
    pub fn parent_path(&self) -> Option<&str> {
        self.parent.path.as_deref()
    }

    /// The folder the last column lists, if any (the list's focused item, when a folder).
    pub fn next_path(&self) -> Option<&str> {
        self.next.path.as_deref()
    }

    fn side(&self, which: Which) -> &Side {
        match which {
            Which::Parent => &self.parent,
            Which::Next => &self.next,
        }
    }

    fn side_mut(&mut self, which: Which) -> &mut Side {
        match which {
            Which::Parent => &mut self.parent,
            Which::Next => &mut self.next,
        }
    }

    /// The pane's tab, if there is one.
    fn tab<'a>(&self, explorer: &'a Explorer) -> Option<&'a Tab> {
        explorer.pane_tab(self.pane)
    }

    /// Reads the pane's tab and the setting, and lists again what changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (path, focused) = {
            let explorer = explorer(cx);
            let explorer = explorer.read(cx);
            match self.tab(explorer) {
                Some(tab) => (
                    tab.path.clone(),
                    tab.selection
                        .focus()
                        .and_then(|focus| tab.rows.position_of(focus))
                        .and_then(|at| tab.rows.get(at))
                        .cloned(),
                ),
                None => (String::new(), None),
            }
        };
        let show_hidden = state::settings(cx).show_hidden;
        let mut changed = false;

        if self.tab_path != path {
            self.tab_path = path.clone();
            changed = true;
        }
        if self.show_hidden != show_hidden {
            self.show_hidden = show_hidden;
            for which in [Which::Parent, Which::Next] {
                let side = self.side_mut(which);
                side.filter(show_hidden);
                side.cursor = side.cursor.min(side.shown.len().saturating_sub(1));
            }
            changed = true;
        }
        let parent = parent_of(&path);
        if self.parent.path != parent {
            self.load(Which::Parent, parent, cx);
            changed = true;
        }
        let next = focused.as_ref().filter(|e| e.dir).map(|e| e.path.clone());
        if self.next.path != next {
            self.load(Which::Next, next, cx);
            changed = true;
        }
        let same = match (&self.focused, &focused) {
            (Some(a), Some(b)) => a.path == b.path && a.size == b.size && a.dir == b.dir && a.name == b.name,
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.focused = focused;
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Lists `path` into a side column in the background (`useListing`): the column is empty until it arrives, a
    /// failure leaves it empty, and a path that is not a real folder lists nothing.
    fn load(&mut self, which: Which, path: Option<String>, cx: &mut Context<Self>) {
        let side = self.side_mut(which);
        side.path = path.clone();
        side.seq += 1;
        side.raw = None;
        side.shown.clear();
        side.cursor = 0;
        let Some(path) = path.filter(|p| is_folder_path(p)) else {
            return;
        };
        let seq = side.seq;
        let listing = (self.lister)(&path);
        cx.spawn(async move |this, cx| {
            let entries: Arc<[FileEntry]> = match listing.await {
                Ok(listed) => listed.entries,
                Err(_) => Arc::from(Vec::new()),
            };
            this.update(cx, |this, cx| this.loaded(which, seq, entries, cx)).ok();
        })
        .detach();
    }

    /// A listing arrived (unless a newer one was asked for meanwhile).
    fn loaded(&mut self, which: Which, seq: u64, entries: Arc<[FileEntry]>, cx: &mut Context<Self>) {
        let show_hidden = self.show_hidden;
        let tab_path = self.tab_path.clone();
        let side = self.side_mut(which);
        if side.seq != seq {
            return;
        }
        side.raw = Some(entries);
        side.filter(show_hidden);
        // The folder above starts on the open folder, and shows it.
        if which == Which::Parent {
            let at = (0..side.shown.len()).find(|&i| side.entry(i).is_some_and(|e| same_path(&e.path, &tab_path)));
            if let Some(at) = at {
                side.cursor = at;
                side.scroll.scroll_to_item(at, ScrollStrategy::Center);
            }
        }
        cx.notify();
    }

    /// Opens a row, as a click on it does: a folder opens; a file opens its folder with the file selected.
    fn open_row(&mut self, which: Which, index: usize, cx: &mut Context<Self>) {
        let side = self.side(which);
        let Some(entry) = side.entry(index) else {
            return;
        };
        let (target, select) = if entry.dir {
            (entry.path.clone(), None)
        } else {
            match &side.path {
                Some(folder) => (folder.clone(), Some(entry.path.clone())),
                None => return,
            }
        };
        let pane = self.pane;
        explorer(cx).update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            let tab = explorer.pane_tab_id(pane);
            let mut options = NavOptions::in_tab(tab);
            if let Some(select) = select {
                options = options.selecting(select);
            }
            explorer.navigate(target, options, cx);
        });
    }

    /// Keys on a side column.
    fn on_key(&mut self, which: Which, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let side = self.side(which);
        let count = side.shown.len();
        let last = count.saturating_sub(1);
        let page = 8;
        let key = event.keystroke.key.as_str();
        let moved = match key {
            "up" => Some(side.cursor.saturating_sub(1)),
            "down" => Some((side.cursor + 1).min(last)),
            "home" => Some(0),
            "end" => Some(last),
            "pageup" => Some(side.cursor.saturating_sub(page)),
            "pagedown" => Some((side.cursor + page).min(last)),
            _ => None,
        };
        if let Some(to) = moved {
            if count > 0 {
                let side = self.side_mut(which);
                side.cursor = to;
                side.scroll.scroll_to_item(to, ScrollStrategy::Nearest);
                cx.notify();
            }
            cx.stop_propagation();
            return;
        }
        match key {
            "enter" | "space" => {
                let at = side.cursor;
                self.open_row(which, at, cx);
            }
            // Right goes into a folder; a file has nothing to go into.
            "right" => {
                let at = side.cursor;
                if side.entry(at).is_some_and(|e| e.dir) {
                    self.open_row(which, at, cx);
                }
            }
            // Left goes to the folder above, as the list's Left does in this view.
            "left" => {
                let pane = self.pane;
                explorer(cx).update(cx, |explorer, cx| {
                    explorer.focus_pane(pane, cx);
                    explorer.go_up(cx).detach();
                });
            }
            _ => return,
        }
        let _ = window;
        cx.stop_propagation();
    }

    /// The rows `range` of a side column.
    fn rows(
        &mut self,
        which: Which,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = theme::tokens(cx).clone();
        let side = self.side(which);
        let has_keyboard = side.focus.is_focused(window);
        let folder_tone = theme::tailwind("amber-500").unwrap_or(t.warning);
        let rtl = i18n::is_rtl();
        range
            .filter_map(|index| {
                let entry = side.entry(index)?;
                let current = which == Which::Parent && same_path(&entry.path, &self.tab_path);
                let under_cursor = has_keyboard && side.cursor == index;
                let name = entry.name.clone();
                let label = match which {
                    Which::Parent => t!("{name}, in the folder above", name = name),
                    Which::Next => t!("{name}, in the focused folder", name = name),
                };
                let tip = isolate(name.clone());
                let id = match which {
                    Which::Parent => ("columns-parent-row", index),
                    Which::Next => ("columns-next-row", index),
                };
                let glyph = if entry.dir {
                    icon(IconName::Folder)
                        .size(rems(1.))
                        .text_color(folder_tone)
                        .flex_none()
                } else {
                    icon(IconName::File).size(rems(1.)).flex_none()
                };
                let (accent, ring, primary, muted) = (t.accent, t.ring, t.primary, t.muted_foreground);
                Some(
                    row_start()
                        .id(id)
                        .role(Role::ListBoxOption)
                        .aria_selected(current)
                        .aria_label(label)
                        .aria_description(SharedString::from(entry.path.clone()))
                        .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                        .test_support()
                        .h(space(7.))
                        .w_full()
                        .flex_none()
                        .gap(space(2.))
                        .px(space(2.))
                        .rounded(theme::size::RADIUS_SM)
                        .border_1()
                        .border_color(if under_cursor {
                            ring
                        } else {
                            gpui_kit::transparent_black()
                        })
                        .text_size(theme::size::SM)
                        .cursor_pointer()
                        .hover(move |style| style.bg(accent))
                        .when(current, |row| {
                            row.bg(primary.opacity(0.15)).font_weight(FontWeight::MEDIUM)
                        })
                        .when(!entry.dir, |row| row.text_color(muted))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            let side = this.side_mut(which);
                            side.cursor = index;
                            side.focus.focus(window, cx);
                            this.open_row(which, index, cx);
                        }))
                        .child(glyph)
                        .child(div().min_w_0().flex_1().truncate().child(isolate(name)))
                        .when(entry.dir, |row| {
                            row.child(
                                icon(if rtl {
                                    IconName::ChevronLeft
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(rems(0.875))
                                .opacity(0.5)
                                .flex_none(),
                            )
                        })
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// A side column's list: virtualized, with its scrollbar, and the cursor keys.
    fn list(&mut self, which: Which, label: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let side = self.side(which);
        let count = side.shown.len();
        let scroll = side.scroll.clone();
        let (id, list_id) = match which {
            Which::Parent => ("columns-parent-box", "columns-parent-list"),
            Which::Next => ("columns-next-box", "columns-next-list"),
        };
        let list = uniform_list(
            list_id,
            count,
            cx.processor(move |this, range, window, cx| this.rows(which, range, window, cx)),
        )
        .track_scroll(&scroll)
        .size_full();
        let focus = side.focus.clone();
        div()
            .id(id)
            .role(Role::ListBox)
            .aria_label(label)
            .test_support()
            .track_focus(&focus)
            .key_context("ColumnsColumn")
            .on_key_down(
                cx.listener(move |this, event: &KeyDownEvent, window, cx| this.on_key(which, event, window, cx)),
            )
            .relative()
            .min_h_0()
            .flex_1()
            .p(space(1.))
            .child(list)
            .child(Scrollbar::vertical(&scroll).viewport_from_layout())
            .into_any_element()
    }

    /// A column's frame: its width, and a border on its end edge (`faces_end`, the parent's `border-e`) or its start
    /// edge (the next column's `border-s`).
    fn frame(&self, id: &'static str, label: SharedString, width: f32, faces_end: bool, cx: &App) -> Stateful<Div> {
        let t = theme::tokens(cx);
        let mut frame = col()
            .id(id)
            .role(Role::Group)
            .aria_label(label)
            .h_full()
            .w(rems(width))
            .flex_none()
            .border_color(t.border);
        // `border-e` / `border-s`: the right edge in English for the end, the left in Arabic.
        frame = if faces_end != i18n::is_rtl() {
            frame.border_r_1()
        } else {
            frame.border_l_1()
        };
        frame
    }

    /// A column's title: `px-3 py-1.5 text-xs font-semibold`, truncated.
    fn header(&self, id: &'static str, text: String, tip: String, cx: &App) -> Stateful<Div> {
        let t = theme::tokens(cx);
        let (label, tip) = (SharedString::from(text.clone()), isolate(tip));
        div()
            .id(id)
            .role(Role::Heading)
            .aria_label(label)
            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
            .flex_none()
            .border_b_1()
            .border_color(t.border)
            .px(space(3.))
            .py(space(1.5))
            .truncate()
            .text_size(theme::size::XS)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.muted_foreground)
            .child(isolate(text))
    }

    fn parent_column(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let parent = self.parent.path.clone()?;
        let name = match base_name(&parent) {
            "" => parent.clone(),
            name => name.to_string(),
        };
        let header = self
            .header("columns-parent-header", name.clone(), parent, cx)
            .test_support();
        let list = self.list(Which::Parent, t!("Items in the folder above"), cx);
        Some(
            self.frame(
                "columns-parent",
                t!("The folder above: {name}", name = name),
                PARENT_WIDTH,
                true,
                cx,
            )
            .child(header)
            .child(list)
            .test_support()
            .into_any_element(),
        )
    }

    fn next_column(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let label = t!("What's in the focused folder");
        let body: AnyElement = match self.focused.clone() {
            Some(entry) if entry.dir => {
                let count = self.next.raw.as_ref().map(|_| self.next.shown.len());
                let mut title = entry.name.clone();
                if let Some(count) = count {
                    title = format!("{title} ({})", format_count(count));
                }
                let header = self
                    .header("columns-next-header", title, entry.path.clone(), cx)
                    .test_support();
                let empty = count == Some(0);
                let list = self.list(Which::Next, t!("Items in the focused folder"), cx);
                col()
                    .size_full()
                    .child(header)
                    .when(empty, |column| {
                        column.child(
                            div()
                                .id("columns-empty")
                                .role(Role::Status)
                                .aria_label(t!("It's empty"))
                                .test_support()
                                .px(space(2.))
                                .py(space(3.))
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(t!("It's empty")),
                        )
                    })
                    .when(!empty, |column| column.child(list))
                    .into_any_element()
            }
            Some(entry) => col()
                .id("columns-file")
                .role(Role::Group)
                .aria_label(t!("{name}, details", name = entry.name.clone()))
                .test_support()
                .gap(space(1.))
                .p(space(3.))
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(
                    div()
                        .truncate()
                        .text_size(theme::size::SM)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.foreground)
                        .child(isolate(entry.name)),
                )
                .child(div().child(format_size(entry.size)))
                .child(div().child(t!("Space shows it full size; Alt+P the preview pane.")))
                .into_any_element(),
            None => div()
                .id("columns-hint")
                .role(Role::Status)
                .aria_label(t!("← and → move between the columns."))
                .test_support()
                .p(space(3.))
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(t!("← and → move between the columns."))
                .into_any_element(),
        };
        self.frame("columns-next", label, NEXT_WIDTH, false, cx)
            .child(body)
            .test_support()
            .into_any_element()
    }
}

impl Render for ColumnsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let parent = self.parent_column(cx);
        let next = self.next_column(cx);
        let center = div().h_full().min_w_0().flex_1().children(self.center.clone());
        row_start()
            .id("columns-view")
            .size_full()
            .min_w_0()
            .items_stretch()
            .text_start()
            .children(parent)
            .child(center)
            .child(next)
    }
}
