//! The grid of tiles with thumbnails and tile sizes. Ported from src/components/explorer/FileGrid.tsx, with the
//! keys, clicks and zoom that Explorer.tsx drove it with, and Thumbnail.tsx and src/lib/thumbs.ts in
//! [`thumbnail`].
//!
//! # What it is
//!
//! [`FileGrid`] is a GPUI entity that shows `explorer.pane_tab(pane)` as tiles in rows: Windows' thumbnail of each
//! file (its kind icon until the picture is in, or when it has none), the name under it, and its size. It
//! keeps no copy of what the explorer holds: it reads the active tab's rows and selection each frame, acts
//! through the explorer's methods, and redraws when the explorer, the thumbnail service or the settings change.
//!
//! | TypeScript                                | Here                                                           |
//! |-------------------------------------------|----------------------------------------------------------------|
//! | `FileGrid` (rows of tiles, virtualized)   | [`FileGrid`], a `uniform_list` of rows of tiles                |
//! | `gridColumns`, `tileLayout`               | `glint_model::grid`, from the window's rem and the tile-size setting |
//! | `lastWidth` (first frame has its columns) | a per-pane memory of the last width                            |
//! | `VirtualList` `scrollToIndex`, `anchor`   | `scroll_to_item` when the focus moves; `keep_place` when the layout changes |
//! | `initialScrollTop`, `onScrollTop`         | `Explorer::saved_scroll` / `save_scroll`, as a place in tiles (`place_of`) |
//! | Ctrl+wheel `onZoom`                       | a capture-phase wheel listener, `Explorer::zoom_grid`          |
//! | `gridMove`, type-ahead (Explorer.tsx)     | [`keys`] and `glint_model::grid::grid_move`                    |
//! | `onRowMouseDown`                          | [`FileGrid`]'s tile handlers (select, range, toggle, open)      |
//! | `Thumbnail`, `useThumbnail`               | [`thumbnail`]                                                  |
//!
//! # Placing it
//!
//! ```ignore
//! let grid = cx.new(|cx| FileGrid::new(pane, window, cx));       // one per pane
//! cx.subscribe(&grid, |this, _, event: &FileGridEvent, cx| match event { ... }).detach();
//! ```
//!
//! Give it the pane's whole body (it has its own "Sort by" header, as the TypeScript's grid did). It is a flex
//! column of `size_full()`. Its root has the `FileList` key context and handles the actions listed on
//! [`FileGrid`].
//!
//! # Not here yet
//!
//! Each is marked `PORT ME(<part>)` where it belongs: drag out and drop in (the grid emits
//! [`FileGridEvent::DragStart`] and shows no drop highlight), the context menu's content (it emits
//! [`FileGridEvent::ContextMenu`]), renaming in place, the marquee, the diff and Git badges, folders' measured sizes,
//! SVG pictures drawn by the grid itself, and the search's, Gallery's and Recent's empty states.

mod compat;
mod empty;
mod keys;
mod tile;

pub mod thumbnail;

#[cfg(test)]
mod tests;

use crate::actions::{GridLarger, GridReset, GridSmaller, OpenInNewTab, OpenItem, SelectAll, ShowItemMenu};
use crate::explorer::{self, Explorer, ExplorerEvent, Rows, Selection, TabId};
use crate::kit::prelude::*;
use crate::{i18n, state, t, theme};
use compat::ScrollableElement as _;
use glint_model::columns::DESCENDING;
use glint_model::entries::{is_archive_file, is_archived, is_recycled, FileEntry, Sort, SortKey};
use glint_model::grid::{grid_columns, grid_move, keep_place, place_of, tile_layout, top_of, GridLayout};
use glint_model::paths::is_gallery;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use thumbnail::Thumbnails;

/// Wheel movement for one tile size: a mouse wheel's notch. Touchpads and fine-grained wheels add up to it.
const WHEEL_STEP: f32 = 100.0;

/// A pause longer than this starts the wheel's count over.
const WHEEL_PAUSE: Duration = Duration::from_millis(400);

/// How far a press on a tile moves, in pixels, before it becomes a drag.
const DRAG_AFTER: f32 = 6.0;

/// Opening more files than this at once asks first (`openSelected`).
const OPEN_AT_MOST: usize = 10;

thread_local! {
    /// How wide each pane's grid was last, so the next folder's grid has its columns in its first frame
    /// (`lastWidth`).
    static LAST_WIDTH: RefCell<HashMap<usize, f32>> = RefCell::new(HashMap::new());
}

/// Things the grid tells whoever places it. The grid has done its own part (the selection) already.
#[derive(Clone, Debug, PartialEq)]
pub enum FileGridEvent {
    /// A right-click (or the menu key) on a tile: show the item menu for the selection at `position` (window
    /// coordinates). The tile is selected.
    ContextMenu { path: String, position: Point<Pixels> },
    /// A right-click on the empty part: the folder's own menu.
    BackgroundContextMenu { position: Point<Pixels> },
    /// A press on a tile moved far enough to be a drag: drag these out of the window (or into the other pane).
    /// `from_archive` says they are inside an archive and must be extracted first, and dragged as copies.
    DragStart { paths: Vec<String>, from_archive: bool },
    /// Enter on more than ten files: ask "Open N files?", then `Explorer::open_file` each.
    OpenMany { paths: Vec<String> },
}

impl EventEmitter<FileGridEvent> for FileGrid {}

/// A press on a tile that may turn into a drag, or (on a multi-selection) into a plain click on release.
struct Press {
    at: Point<Pixels>,
    /// Pressing an item of a multi-selection keeps the selection until release, so all of it can be dragged;
    /// released without dragging, the selection narrows to this item.
    narrow_to: Option<String>,
}

/// The Ctrl+wheel zoom's count: a notch is one step; finer wheels add up to it.
#[derive(Default)]
struct Wheel {
    turned: f32,
    last: Option<Instant>,
}

/// The tiles of one pane's active tab.
///
/// **Actions it handles** (in its `FileList` key context): `OpenItem`, `OpenInNewTab`, `SelectAll`,
/// `GridLarger`, `GridSmaller`, `GridReset` and `ShowItemMenu` (as [`FileGridEvent::ContextMenu`]). Everything
/// else bound in that context (Back, Rename, Delete, Copy…) it leaves to propagate to the workspace.
/// **Keys it handles itself:** the arrows, Page Up/Down, Home and End (Shift extends, Ctrl only moves, which
/// waits for `Explorer::set_focus`), Ctrl+Space, Escape, and type-ahead.
pub struct FileGrid {
    pane: usize,
    explorer: Entity<Explorer>,
    thumbs: Entity<Thumbnails>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    /// Where the body (below the header) was drawn last frame, shared with the paint callbacks.
    body: Rc<Cell<Bounds<Pixels>>>,
    /// Whether a press is under way (the window-level pointer listeners only do anything then).
    busy: Rc<Cell<bool>>,
    /// A fingerprint of the thumbnails last handed to the service (see `tile::show_thumbnails`).
    shown: Rc<Cell<u64>>,
    /// The layout drawn last, to keep the place when it changes under the scroll position.
    layout: Option<GridLayout>,
    /// The tab and view whose scroll position was restored last.
    placed_for: Option<(TabId, String)>,
    /// The last place saved for it, so it is saved when it changes.
    saved_place: Option<f32>,
    /// The focused item the list was last scrolled to.
    scrolled_to: Option<String>,
    press: Option<Press>,
    wheel: Wheel,
    type_ahead: keys::TypeAhead,
    _subscriptions: Vec<Subscription>,
}

impl FileGrid {
    /// A grid for pane `pane` (0 is the left or only pane). Its thumbnails come from the app's thumbnail service
    /// ([`thumbnail::thumbnails`], made on first use).
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let thumbs = thumbnail::thumbnails(cx);
        let id = cx.entity_id();
        let mut subscriptions = vec![
            cx.observe(&explorer, |_, _, cx| cx.notify()),
            // A picture arrived, or one was let go of.
            cx.observe(&thumbs, |_, _, cx| cx.notify()),
            // The tile size, the text size.
            cx.observe_global::<state::UiState>(|_, cx| cx.notify()),
            cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| {
                if matches!(event, ExplorerEvent::TabsChanged | ExplorerEvent::PaneFocused) {
                    this.press = None;
                    this.busy.set(false);
                    cx.notify();
                }
            }),
        ];
        let service = thumbs.clone();
        subscriptions.push(cx.on_release(move |_, cx| {
            service.update(cx, |thumbs, cx| thumbs.release(id, cx));
        }));
        Self {
            pane,
            explorer,
            thumbs,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            body: Rc::new(Cell::new(Bounds::default())),
            busy: Rc::new(Cell::new(false)),
            shown: Rc::new(Cell::new(0)),
            layout: None,
            placed_for: None,
            saved_place: None,
            scrolled_to: None,
            press: None,
            wheel: Wheel::default(),
            type_ahead: keys::TypeAhead::default(),
            _subscriptions: subscriptions,
        }
    }

    /// The pane this grid shows.
    pub fn pane(&self) -> usize {
        self.pane
    }

    /// The grid's focus: give it to the grid when its pane takes the keyboard (`window.focus(&grid.focus_handle(), cx)`).
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Call when the grid is swapped out (the tab went to the list) but the entity is kept: its thumbnails are let
    /// go of, and asked for again when it is drawn. Dropping the entity does the same.
    pub fn pause(&mut self, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        self.thumbs.update(cx, |thumbs, cx| thumbs.release(id, cx));
        self.shown.set(0);
        self.press = None;
        self.busy.set(false);
    }

    // ---- What the explorer holds ------------------------------------------------------------------

    /// The pane's rows and selection, shared (cheap to clone).
    fn snapshot(&self, cx: &App) -> Option<(Rows, Selection)> {
        let tab = self.explorer.read(cx).pane_tab(self.pane)?;
        Some((tab.rows.clone(), tab.selection.clone()))
    }

    /// The row the keyboard is on, or -1.
    fn focus_index(rows: &Rows, selection: &Selection) -> i64 {
        selection
            .focus()
            .and_then(|focus| rows.position_of(focus))
            .and_then(|i| i64::try_from(i).ok())
            .unwrap_or(-1)
    }

    /// The tiles per row and the row's height as last drawn (one tile and 1 px before the first frame).
    fn layout_now(&self) -> GridLayout {
        self.layout.unwrap_or(GridLayout {
            cols: 1,
            row_height: 1.0,
        })
    }

    /// Clicking in a pane makes it the one the keyboard acts on.
    fn activate_pane(&mut self, cx: &mut Context<Self>) {
        if self.explorer.read(cx).active_pane() != self.pane {
            let pane = self.pane;
            self.explorer.update(cx, |explorer, cx| explorer.focus_pane(pane, cx));
        }
    }

    // ---- Mouse ----------------------------------------------------------------------------------------

    /// A press on tile `index` (`onRowMouseDown`, and the double-click that opens).
    fn tile_down(&mut self, index: usize, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.activate_pane(cx);
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };
        let Some(entry) = rows.get(index).cloned() else {
            return;
        };
        let additive = event.modifiers.control || event.modifiers.platform;
        match event.button {
            // Middle-click: the folder (or archive) in a new tab.
            MouseButton::Middle => {
                if (entry.dir || is_archive_file(&entry)) && !is_recycled(&entry) {
                    self.explorer
                        .update(cx, |explorer, cx| explorer.open_in_new_tab(&entry.path, cx));
                }
            }
            // Right-click keeps a selection the item is in, else selects it alone.
            MouseButton::Right => {
                if !selection.contains(&entry.path) {
                    self.explorer
                        .update(cx, |explorer, cx| explorer.select_only(Some(&entry.path), cx));
                }
            }
            MouseButton::Left => {
                let keep =
                    !event.modifiers.shift && !additive && selection.contains(&entry.path) && selection.len() > 1;
                self.explorer.update(cx, |explorer, cx| {
                    // The first click of what may be a double-click reads the folder ahead.
                    if entry.dir {
                        explorer.prefetch_folder(&entry.path, cx);
                    }
                    if event.modifiers.shift {
                        explorer.select_range(&rows, index, additive, cx);
                    } else if additive {
                        explorer.toggle_selected(&entry.path, cx);
                    } else if !keep {
                        explorer.select_only(Some(&entry.path), cx);
                    }
                    // PORT ME(coordinator): `Explorer::set_focus(path, anchor)` for the `keep` case: the focus and
                    // the anchor move to the pressed item at once (`updateTab({ focus, anchor })`). Until then they
                    // move on release, with the narrowing.
                });
                self.press = Some(Press {
                    at: event.position,
                    narrow_to: keep.then(|| entry.path.clone()),
                });
                self.busy.set(true);
                if event.click_count == 2 {
                    self.explorer
                        .update(cx, |explorer, cx| explorer.open_entry(&entry, cx).detach());
                }
            }
            _ => {}
        }
    }

    /// The button went up on tile `index`: a right-click asks for the menu (`onContextMenu`).
    fn tile_context_menu(&mut self, index: usize, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some((rows, _)) = self.snapshot(cx) else {
            return;
        };
        if let Some(entry) = rows.get(index) {
            cx.emit(FileGridEvent::ContextMenu {
                path: entry.path.clone(),
                position: event.position,
            });
        }
    }

    /// Hovering a folder tile reads it ahead; leaving it lets go (`hoverFolder`, `leaveFolder`).
    fn tile_hover(&mut self, index: usize, hovered: bool, cx: &mut Context<Self>) {
        let Some((rows, _)) = self.snapshot(cx) else {
            return;
        };
        let Some(entry) = rows.get(index) else {
            return;
        };
        let path = entry.path.clone();
        self.explorer.update(cx, |explorer, cx| {
            if hovered {
                explorer.hover_folder(&path, cx);
            } else {
                explorer.leave_folder();
            }
        });
    }

    /// A press on the empty part: without Ctrl or Shift it clears the selection (`onBackgroundMouseDown`).
    fn background_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.activate_pane(cx);
        if event.button == MouseButton::Left && !event.modifiers.control && !event.modifiers.shift {
            self.explorer.update(cx, |explorer, cx| explorer.select_only(None, cx));
        }
        // PORT ME(coordinator): the marquee (`startMarquee`): dragging from here draws a rectangle that selects what
        // it covers, added to what was selected with Ctrl, scrolling while the pointer is held near the top or
        // bottom. `glint_model::grid::marquee_hits` is ready; it needs `Explorer::set_selection(paths, anchor,
        // focus)` to apply the hits at once.
    }

    /// The pointer moved with a press under way: far enough, and it is a drag.
    fn pointer_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(press) = &self.press else {
            self.busy.set(false);
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.press = None;
            self.busy.set(false);
            return;
        }
        let (dx, dy) = (
            f32::from(event.position.x - press.at.x),
            f32::from(event.position.y - press.at.y),
        );
        if dx.hypot(dy) < DRAG_AFTER {
            return;
        }
        self.press = None;
        self.busy.set(false);
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };
        let chosen: Vec<&FileEntry> = rows.iter().filter(|entry| selection.contains(&entry.path)).collect();
        // Out of the Recycle Bin, an item would land named as Windows keeps it there: Restore it instead.
        if chosen.is_empty() || chosen.iter().any(|entry| is_recycled(entry)) {
            return;
        }
        cx.emit(FileGridEvent::DragStart {
            paths: chosen.iter().map(|entry| entry.path.clone()).collect(),
            from_archive: chosen.iter().any(|entry| is_archived(entry)),
        });
        // PORT ME(drag): the dragged items show dim (`dragging`), and the folder under the pointer highlights as
        // the drop target (`dropTarget`).
    }

    /// A button went up anywhere: a press on a multi-selection that wasn't a drag narrows to its item.
    fn pointer_released(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        self.busy.set(false);
        if let Some(Press {
            narrow_to: Some(path), ..
        }) = self.press.take()
        {
            self.explorer
                .update(cx, |explorer, cx| explorer.select_only(Some(&path), cx));
        }
    }

    /// Ctrl+wheel over the grid: bigger tiles (wheel up) or smaller ones.
    fn ctrl_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        // Tiles move to other rows as they zoom, which would make the rename box anew.
        if self
            .explorer
            .read(cx)
            .pane_tab(self.pane)
            .is_some_and(|tab| tab.renaming.is_some())
        {
            return;
        }
        let delta = match event.delta {
            // A notch is three lines; multiplied before dividing so that a whole notch is exactly one step.
            ScrollDelta::Lines(lines) => lines.y * WHEEL_STEP / 3.0,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y),
        };
        let now = Instant::now();
        let paused = self
            .wheel
            .last
            .is_none_or(|last| now.duration_since(last) > WHEEL_PAUSE);
        let turned_back = self.wheel.turned != 0.0 && (delta > 0.0) != (self.wheel.turned > 0.0);
        if paused || turned_back {
            self.wheel.turned = 0.0;
        }
        self.wheel.last = Some(now);
        self.wheel.turned += delta;
        let steps = (self.wheel.turned / WHEEL_STEP).trunc();
        if steps == 0.0 {
            return;
        }
        self.wheel.turned -= steps * WHEEL_STEP;
        // Away from you (up) for bigger tiles, as in Explorer. Whole notches only, so the cast is exact.
        self.zoom(steps as i64, cx);
    }

    fn zoom(&mut self, steps: i64, cx: &mut Context<Self>) {
        self.explorer.update(cx, |explorer, cx| explorer.zoom_grid(steps, cx));
    }

    // ---- Keyboard ---------------------------------------------------------------------------------

    /// A key that no binding took (`onKey` in Explorer.tsx, the parts the grid and the list share).
    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let ctrl = modifiers.control || modifiers.platform;
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };

        // The arrows, the pages, Home and End.
        if !modifiers.alt {
            if let Some(key) = keys::move_key(&keystroke.key, i18n::is_rtl()) {
                let layout = self.layout_now();
                let at = Self::focus_index(&rows, &selection);
                let rows_on_screen = (f32::from(self.body.get().size.height) / layout.row_height.max(1.0)).floor();
                // One row of tiles less than the screen holds (at least one).
                let page = (rows_on_screen as usize).saturating_sub(1).max(1);
                if let Some(to) = grid_move(key, at, rows.len(), layout.cols, page) {
                    cx.stop_propagation();
                    let Some(path) = rows.get(to).map(|entry| entry.path.clone()) else {
                        return;
                    };
                    // Arrows select; with Shift they extend from the anchor; with Ctrl they only move the cursor.
                    if modifiers.shift {
                        self.explorer
                            .update(cx, |explorer, cx| explorer.select_range(&rows, to, ctrl, cx));
                    } else if ctrl {
                        // PORT ME(coordinator): `Explorer::set_focus(path, None)`: Ctrl+arrow moves the cursor without
                        // changing the selection (`updateTab({ focus })`). Until then the list scrolls there only.
                        self.scroll_to_row(to / layout.cols.max(1));
                    } else {
                        self.explorer
                            .update(cx, |explorer, cx| explorer.select_only(Some(&path), cx));
                    }
                }
                return;
            }
        }

        match (keystroke.key.as_str(), ctrl, modifiers.alt) {
            // Ctrl+Space: the focused item in or out of the selection.
            ("space", true, false) => {
                if let Some(focus) = selection.focus().map(str::to_string) {
                    cx.stop_propagation();
                    self.explorer
                        .update(cx, |explorer, cx| explorer.toggle_selected(&focus, cx));
                }
            }
            // Escape clears the filter, else the selection.
            ("escape", false, false) => {
                let filter = self
                    .explorer
                    .read(cx)
                    .pane_tab(self.pane)
                    .is_some_and(|tab| !tab.filter.is_empty());
                if filter {
                    cx.stop_propagation();
                    // PORT ME(find): `clearFind` also ends a search's results.
                    self.explorer
                        .update(cx, |explorer, cx| explorer.set_filter("", None, cx));
                } else if !selection.is_empty() {
                    cx.stop_propagation();
                    self.explorer.update(cx, |explorer, cx| explorer.select_only(None, cx));
                }
            }
            // Type-ahead: jump to the next name starting with what was typed; one letter repeated cycles.
            _ if !ctrl && !modifiers.alt => {
                let typed = keystroke
                    .key_char
                    .as_deref()
                    .filter(|text| text.chars().count() == 1 && *text != " ");
                if let Some(typed) = typed {
                    let at = Self::focus_index(&rows, &selection);
                    if let Some(found) = self.type_ahead.find(typed, Instant::now(), at, &rows) {
                        cx.stop_propagation();
                        if let Some(path) = rows.get(found).map(|entry| entry.path.clone()) {
                            self.explorer
                                .update(cx, |explorer, cx| explorer.select_only(Some(&path), cx));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Enter: open what is selected (`openSelected`). One item opens as a click would; several open the files among
    /// them (or the first folder, if there are none), and many ask first.
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };
        let chosen: Vec<FileEntry> = if selection.len() == 1 {
            selection
                .iter()
                .next()
                .and_then(|path| rows.position_of(path))
                .and_then(|i| rows.get(i).cloned())
                .into_iter()
                .collect()
        } else {
            rows.iter()
                .filter(|entry| selection.contains(&entry.path))
                .cloned()
                .collect()
        };
        if chosen.len() == 1 {
            self.open(&chosen[0], cx);
            return;
        }
        let files: Vec<&FileEntry> = chosen.iter().filter(|entry| !entry.dir).collect();
        if files.is_empty() {
            if let Some(first) = chosen.first() {
                self.open(first, cx);
            }
            return;
        }
        if files.len() > OPEN_AT_MOST {
            cx.emit(FileGridEvent::OpenMany {
                paths: files.iter().map(|entry| entry.path.clone()).collect(),
            });
            return;
        }
        for entry in files {
            self.open(entry, cx);
        }
    }

    fn open(&mut self, entry: &FileEntry, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.open_entry(entry, cx).detach());
    }

    /// Ctrl+Enter: each selected folder (and archive) in a new tab (`openSelectedInTabs`).
    fn open_selected_in_tabs(&mut self, cx: &mut Context<Self>) {
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };
        let paths: Vec<String> = rows
            .iter()
            .filter(|entry| selection.contains(&entry.path))
            .filter(|entry| (entry.dir || is_archive_file(entry)) && !is_recycled(entry))
            .map(|entry| entry.path.clone())
            .collect();
        self.explorer.update(cx, |explorer, cx| {
            for path in paths {
                explorer.open_in_new_tab(&path, cx);
            }
        });
    }

    /// Shift+F10 or the menu key: the menu of what is selected, at the focused tile.
    fn keyboard_menu(&mut self, cx: &mut Context<Self>) {
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };
        let at = Self::focus_index(&rows, &selection);
        let Some(entry) = usize::try_from(at).ok().and_then(|i| rows.get(i)) else {
            return;
        };
        let layout = self.layout_now();
        let cols = layout.cols.max(1);
        let (row, col) = (
            usize::try_from(at).unwrap_or(0) / cols,
            usize::try_from(at).unwrap_or(0) % cols,
        );
        let body = self.body.get();
        let top = self.scroll_top();
        let col_w = (f32::from(body.size.width) / cols as f32).max(1.0);
        let col_x = if i18n::is_rtl() { cols - 1 - col } else { col };
        // The middle of the tile, kept inside the body.
        let x = f32::from(body.origin.x) + (col_x as f32 + 0.5) * col_w;
        let y = (f32::from(body.origin.y) + (row as f32 + 0.5) * layout.row_height - top)
            .clamp(f32::from(body.origin.y), f32::from(body.origin.y + body.size.height));
        cx.emit(FileGridEvent::ContextMenu {
            path: entry.path.clone(),
            position: point(px(x), px(y)),
        });
    }

    // ---- Scrolling --------------------------------------------------------------------------------

    /// How far the grid is scrolled, pixels from the top.
    fn scroll_top(&self) -> f32 {
        let state = self.scroll.0.borrow();
        (-f32::from(state.base_handle.offset().y)).max(0.0)
    }

    fn set_scroll_top(&self, top: f32) {
        self.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-top.max(0.0))));
    }

    fn scroll_to_row(&self, row: usize) {
        self.scroll.scroll_to_item(row, ScrollStrategy::Nearest);
    }

    // ---- Drawing ----------------------------------------------------------------------------------

    /// The "Sort by" header (`Header`): the sort in force, and a click sorts by that.
    fn header(
        &self,
        sort: Sort,
        recycled: bool,
        located: bool,
        gallery: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let date = if recycled {
            t!("Date deleted")
        } else if gallery {
            t!("Date")
        } else {
            t!("Date modified")
        };
        let mut columns: Vec<(SortKey, SharedString)> = vec![(SortKey::Name, t!("Name")), (SortKey::Modified, date)];
        if !gallery {
            columns.push((SortKey::Taken, t!("Date taken")));
        }
        if !located {
            columns.push((SortKey::Type, t!("Type")));
        }
        columns.push((SortKey::Size, t!("Size")));

        row_start()
            .id("file-grid-header")
            .role(Role::Toolbar)
            .aria_label(t!("Sort by"))
            .test_support()
            .flex_none()
            .h(space(8.))
            .px(space(3.))
            .gap(space(4.))
            .overflow_hidden()
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.muted_foreground)
            .child(div().flex_none().child(t!("Sort by")))
            .children(columns.into_iter().enumerate().map(|(n, (key, label))| {
                let active = sort.key == key;
                let arrow = if sort.desc {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronUp
                };
                Button::new(("file-grid-sort", n))
                    .ghost()
                    .xsmall()
                    .selected(active)
                    .label(label.clone())
                    .tooltip(t!("Sort by {name}", name = label.to_lowercase()))
                    .when(active, |button| button.child(icon(arrow).size_3p5()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let desc = if sort.key == key {
                            !sort.desc
                        } else {
                            DESCENDING.contains(&key)
                        };
                        this.explorer
                            .update(cx, |explorer, cx| explorer.set_sort(Sort { key, desc }, cx));
                    }))
            }))
    }
}

/// A listener that tiles call with their index and the event.
fn handler<E: 'static>(
    cx: &mut Context<FileGrid>,
    run: impl Fn(&mut FileGrid, &E, &mut Window, &mut Context<FileGrid>) + 'static,
) -> tile::Handler<E> {
    Rc::new(cx.listener(run))
}

impl Render for FileGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx).clone();
        let scale = window.scale_factor();
        let rem = f32::from(window.rem_size());

        // What the active tab of the pane holds. All of it is shared or small to copy.
        let explorer = self.explorer.read(cx);
        let Some(tab) = explorer.pane_tab(self.pane) else {
            return div().size_full().into_any_element();
        };
        let tab_id = tab.id;
        let rows = tab.rows.clone();
        let selection = tab.selection.clone();
        let sort = tab.sort;
        let view_key = tab.view_key();
        let lane = tab.id.to_string();
        let recycled = tab.is_recycle_bin();
        let gallery = is_gallery(&tab.path);
        let search = tab.find.is_some();
        let scope = tab.find.as_ref().map(|find| find.scope.clone()).unwrap_or_default();
        let filter = tab.filter.clone();
        let listed = tab.entries().len();
        let archive = tab.archive().is_some();
        let has_listing = tab.listing.is_some();
        let inactive = explorer.is_dual() && explorer.active_pane() != self.pane;
        let flashed = explorer.flashed().clone();
        let saved = explorer.saved_scroll(tab_id, &format!("{view_key}:grid"));
        let located = search || recycled;

        // The layout: tiles from the tile-size setting and the text size, as many to a row as the width holds.
        let tile = tile_layout(rem, state::settings(cx).tile_size.as_str());
        let measured = self.body.get();
        let measured_width = f32::from(measured.size.width);
        let width = if measured_width > 0.0 {
            LAST_WIDTH.with(|last| last.borrow_mut().insert(self.pane, measured_width));
            measured_width
        } else {
            LAST_WIDTH
                .with(|last| last.borrow().get(&self.pane).copied())
                .unwrap_or_else(|| f32::from(window.viewport_size().width))
        };
        // The scroll bar sits over a gutter at the end of the list, so the tiles never run under it.
        let gutter = rem * 0.75;
        let inner = (width - gutter - 2.0 * tile.pad).max(0.0);
        let cols = grid_columns(inner, tile.width);
        let col_w = inner / cols as f32;
        let layout = GridLayout {
            cols,
            row_height: tile.height,
        };
        let line_count = rows.len().div_ceil(cols);
        let body_height = f32::from(measured.size.height);

        // Scroll position: restored for a folder just opened; kept (around the focused tile) when the layout
        // changes under it (a zoom, a resize); remembered for the next visit.
        let place_key = (tab_id, format!("{view_key}:grid"));
        // Looking the focused item up is a scan of the rows, so it is done only when it is needed.
        let focus_row = || Self::focus_index(&rows, &selection);
        let wanted_top = |top: f32| {
            let most = (line_count as f32 * layout.row_height - body_height).max(0.0);
            top.clamp(0.0, if body_height > 0.0 { most } else { f32::MAX })
        };
        if self.placed_for.as_ref() != Some(&place_key) {
            self.placed_for = Some(place_key.clone());
            self.type_ahead.reset();
            self.scrolled_to = None;
            self.shown.set(0);
            self.set_scroll_top(if saved > 0.0 {
                wanted_top(top_of(saved, layout))
            } else {
                0.0
            });
            self.saved_place = Some(saved);
            let (thumbs, scope_key) = (self.thumbs.clone(), format!("{view_key}:grid"));
            let lane = lane.clone();
            // The tab's folder changed: what it waited for is dropped, and the burst is timed from here.
            thumbs.update(cx, |thumbs, cx| {
                thumbs.forget_lane(&lane, cx);
                thumbs.time(&lane, &scope_key, cx);
            });
        } else if let Some(was) = self.layout.filter(|was| *was != layout) {
            let next = keep_place(was, layout, self.scroll_top(), body_height, line_count, focus_row());
            self.set_scroll_top(next);
        }
        self.layout = Some(layout);

        // The focused item stays in view as the keyboard moves it.
        let focus = selection.focus().map(str::to_string);
        if focus != self.scrolled_to {
            if let Ok(row) = usize::try_from(focus_row()) {
                self.scroll_to_row(row / cols);
            }
            self.scrolled_to = focus.clone();
        }

        let place = place_of(self.scroll_top(), layout);
        if self.saved_place != Some(place) {
            self.saved_place = Some(place);
            self.explorer.update(cx, |explorer, _| {
                explorer.save_scroll(tab_id, &format!("{view_key}:grid"), place)
            });
        }

        let frame = Rc::new(tile::Frame {
            rows: rows.clone(),
            selection: selection.clone(),
            focus,
            inactive,
            flashed,
            tile,
            cols,
            lines: line_count,
            col_w,
            located,
            recycled,
            gallery,
            scope,
            lane,
            scale,
            tokens: tokens.clone(),
            thumbs: self.thumbs.clone(),
            viewer: cx.entity_id(),
            shown: self.shown.clone(),
            on_down: handler(cx, |this, (i, event): &(usize, MouseDownEvent), window, cx| {
                this.tile_down(*i, event, window, cx)
            }),
            on_right_up: handler(cx, |this, (i, event): &(usize, MouseUpEvent), _, cx| {
                this.tile_context_menu(*i, event, cx)
            }),
            on_hover: handler(cx, |this, (i, hovered): &(usize, bool), _, cx| {
                this.tile_hover(*i, *hovered, cx)
            }),
        });

        let list = uniform_list("file-grid-rows", line_count, {
            let frame = frame.clone();
            move |range, window, cx| tile::rows_in(&frame, range, window, cx)
        })
        .track_scroll(&self.scroll)
        .size_full()
        .pr(px(gutter));

        let name = if search {
            t!("Search results")
        } else if recycled {
            t!("Recycle Bin")
        } else {
            t!("Files")
        };
        let empty = (rows.is_empty() && has_listing).then(|| empty::Empty::of(&filter, recycled, listed, archive));

        let body = div()
            .id("file-grid-body")
            .test_support()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("file-grid")
                    .role(Role::ListBox)
                    .aria_label(name)
                    .test_support()
                    .size_full()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| this.background_down(event, window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event, window, cx| this.background_down(event, window, cx)),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|_, event: &MouseUpEvent, _, cx| {
                            cx.emit(FileGridEvent::BackgroundContextMenu {
                                position: event.position,
                            })
                        }),
                    )
                    .child(list),
            )
            .vertical_scrollbar(&self.scroll)
            .children(empty.map(|empty| div().absolute().inset_0().child(empty::view(&empty, cx))))
            .child(self.taps(width, body_height, cx));

        col()
            .id(("file-grid-pane", self.pane))
            .key_context("FileList")
            .track_focus(&self.focus)
            .size_full()
            .min_h_0()
            .items_stretch()
            .bg(tokens.background)
            .on_key_down(cx.listener(|this, event, window, cx| this.key_down(event, window, cx)))
            .on_action(cx.listener(|this, _: &OpenItem, _, cx| this.open_selected(cx)))
            .on_action(cx.listener(|this, _: &OpenInNewTab, _, cx| this.open_selected_in_tabs(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if let Some((rows, _)) = this.snapshot(cx) {
                    this.explorer.update(cx, |explorer, cx| explorer.select_all(&rows, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &GridLarger, _, cx| this.zoom(1, cx)))
            .on_action(cx.listener(|this, _: &GridSmaller, _, cx| this.zoom(-1, cx)))
            .on_action(cx.listener(|this, _: &GridReset, _, cx| this.zoom(0, cx)))
            .on_action(cx.listener(|this, _: &ShowItemMenu, _, cx| this.keyboard_menu(cx)))
            .child(self.header(sort, recycled, located, gallery, cx))
            .child(body)
            .into_any_element()
    }
}

impl FileGrid {
    /// An invisible element over the body that measures it and listens to the window's pointer: the width and
    /// height for the layout, the Ctrl+wheel (which must be taken before the list scrolls with it), and the
    /// button going up anywhere after a press.
    fn taps(&self, drawn_width: f32, drawn_height: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let body = self.body.clone();
        let busy = self.busy.clone();
        canvas(
            |bounds, _, _| bounds,
            move |bounds, _, window, cx| {
                let was = body.replace(bounds);
                let size_changed = (f32::from(bounds.size.width) - drawn_width).abs() > 0.5
                    || (f32::from(bounds.size.height) - drawn_height).abs() > 0.5;
                if size_changed || was.origin != bounds.origin {
                    // The grid was laid out for another size: draw it again for this one.
                    let view = view.clone();
                    window.defer(cx, move |_, cx| {
                        view.update(cx, |_, cx| cx.notify()).ok();
                    });
                }
                let moved = view.clone();
                let moving = busy.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && moving.get() {
                        moved.update(cx, |this, cx| this.pointer_moved(event, cx)).ok();
                    }
                });
                let released = view.clone();
                let pressing = busy.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && pressing.get() {
                        released.update(cx, |this, cx| this.pointer_released(event, cx)).ok();
                    }
                });
                let wheeled = view.clone();
                window.on_mouse_event(move |event: &ScrollEvent, phase, _, cx| {
                    // Only an active listener can stop the list scrolling too: the capture phase comes first.
                    if phase == DispatchPhase::Capture && event.modifiers.control && bounds.contains(&event.position) {
                        wheeled.update(cx, |this, cx| this.ctrl_wheel(event, cx)).ok();
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}
