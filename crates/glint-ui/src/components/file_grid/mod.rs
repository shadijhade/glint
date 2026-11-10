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
//! | `RenameInput` in a tile (F2)              | [`rename`]: a box in place of the name, following `Tab::renaming` |
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
//! # Marquee, drops, drag out
//!
//! - **Marquee:** a press on the empty part draws a rectangle that selects the tiles it covers
//!   (`glint_model::grid::marquee_hits`, applied with `Explorer::set_selection`), added to the selection with Ctrl, and
//!   scrolls while the pointer is held near the top or bottom of the body (`startMarquee`).
//! - **Files dropped in** (from File Explorer, the desktop, the other pane): GPUI's own file drops, answered through
//!   [`crate::drops`]. A folder tile takes them into it (not one in an archive, one Windows keeps closed or one in
//!   the Recycle Bin); anywhere else they go into the folder shown, unless that is This PC, search results, an
//!   archive or a closed folder. A drop that would do nothing (the files are already there, or it is one of them)
//!   is refused. The drag's words are "Copy to %1" / "Move to %1" (the Recycle Bin always moves). The folder tile
//!   (or the whole grid) under the pointer is lit. The drop is `Explorer::drop_files`; a link (Alt) copies, as the
//!   web app's did.
//! - **Dragging out:** [`FileGridEvent::DragStart`]; the dragged tiles are dimmed until the drag is over (the first
//!   pointer move with no button down, or [`FileGrid::end_drag`]).
//! - **Cut** tiles are dimmed from `Explorer::cut_paths`.
//!
//! # Right to left
//!
//! Tiles run from the right in Arabic. Left and Right move to the tile at the left or right *of the screen* (not
//! the next or previous one in reading order): this is kept on purpose, as File Explorer does it in a mirrored
//! window, and the tests pin it.
//!
//! # Not here yet
//!
//! Each is marked `PORT ME(<part>)` where it belongs: the search's, Gallery's and Recent's empty states. A
//! folder's tile says its size once "Folder sizes" measured it, and "…" while that runs (`Explorer::size_run`).
//! The context menu's content is the workspace's: the grid only emits [`FileGridEvent::ContextMenu`] (from the keyboard at the focused tile's bottom-start corner).

mod empty;
mod keys;
mod rename;
mod scrub;
mod tile;

pub mod thumbnail;

#[cfg(test)]
mod tests;

use crate::actions::{GridLarger, GridReset, GridSmaller, OpenInNewTab, OpenItem, SelectAll, ShowItemMenu};
use crate::explorer::{self, Explorer, ExplorerEvent, Rows, Selection, TabId};
use crate::kit::prelude::*;
use crate::platform::dragdrop::{Answer, DropEffect, Under};
use crate::{drops, i18n, state, t, theme};
use glint_model::columns::DESCENDING;
use glint_model::entries::{is_archive_file, is_archived, is_closed, is_recycled, FileEntry, Sort, SortKey};
use glint_model::grid::{
    grid_columns, grid_move, keep_place, marquee_hits, place_of, tile_layout, top_of, GridLayout, Point as ContentPoint,
};
use glint_model::paths::{droppable, is_gallery, is_recycle_bin};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
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

/// How far a marquee's pointer moves, in pixels, before it is drawn (the TypeScript's 4).
const MARQUEE_AFTER: f32 = 4.0;

/// Held within this many rems of the body's top or bottom, a marquee scrolls (the TypeScript's 24 px).
const MARQUEE_EDGE: f32 = 1.5;

/// How often a marquee at the edge scrolls.
const MARQUEE_TICK: Duration = Duration::from_millis(30);

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

/// A marquee being dragged out over the empty part (`startMarquee`).
struct Marquee {
    /// Where it started: x from the body's left edge, y from the top of the content (scrolled part included).
    start: ContentPoint,
    /// Where the pointer is now, in the window.
    pointer: Point<Pixels>,
    /// What was selected when it began, kept with Ctrl held.
    base: Vec<String>,
    /// Whether the pointer has moved far enough to draw it.
    shown: bool,
    /// How many tiles it covered last, and the first and last of them: nothing is applied when they don't change.
    last: Option<(usize, Option<usize>, Option<usize>)>,
}

/// Where the tiles are, as last drawn: what the marquee, the drops and the keyboard menu need to turn a pointer
/// into a tile and a tile into a corner.
#[derive(Clone, Copy, Debug)]
struct Metrics {
    cols: usize,
    row_height: f32,
    /// The space before the first tile and after the last one in a row.
    pad: f32,
    col_w: f32,
    /// The width the rows are laid out in: the body less the scroll bar's gutter.
    content_w: f32,
    /// The gap around a tile in its cell (`m-0.5`).
    inset: f32,
    /// The window's rem, for the distances that follow the text size.
    rem: f32,
}

/// What files dragged over the grid would be dropped into.
#[derive(Clone, Debug, PartialEq)]
enum DropOver {
    /// The folder tile under the pointer.
    Tile(String),
    /// The folder the grid shows.
    Pane,
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
/// **Keys it handles itself:** the arrows, Page Up/Down, Home and End (Shift extends, Ctrl only moves the
/// cursor), Ctrl+Space, Escape, and type-ahead.
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
    /// Where the tiles were drawn last.
    metrics: Option<Metrics>,
    /// The tab and view whose scroll position was restored last.
    placed_for: Option<(TabId, String)>,
    /// The last place saved for it, so it is saved when it changes.
    saved_place: Option<f32>,
    /// The focused item the list was last scrolled to.
    scrolled_to: Option<String>,
    press: Option<Press>,
    marquee: Option<Marquee>,
    /// Scrolls the body while a marquee is held at its edge; dropped with the marquee.
    marquee_scroll: Option<Task<()>>,
    /// The items being dragged out of the window, dimmed until it is over.
    dragging: Option<Arc<HashSet<String>>>,
    /// What a drop at the pointer would go into, while files are dragged over the grid.
    drop_over: Option<DropOver>,
    wheel: Wheel,
    type_ahead: keys::TypeAhead,
    /// The scrub overlay of the video tile the pointer is on.
    scrub: scrub::ScrubSlot,
    /// The box that renames a tile in place (F2): it follows `Tab::renaming`.
    rename: Option<rename::RenameBox>,
    /// The thumbnail size drawn last, logical pixels.
    thumb: f32,
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
                    this.stop_pointer();
                    cx.notify();
                }
            }),
        ];
        // Git's marks arrived or changed: the pills on the tiles, and the faded ignored ones, are drawn again.
        if let Some(model) = crate::components::git::try_shared(cx) {
            subscriptions.push(cx.observe(&model, |_, _, cx| cx.notify()));
        }
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
            metrics: None,
            placed_for: None,
            saved_place: None,
            scrolled_to: None,
            press: None,
            marquee: None,
            marquee_scroll: None,
            dragging: None,
            drop_over: None,
            wheel: Wheel::default(),
            type_ahead: keys::TypeAhead::default(),
            scrub: scrub::ScrubSlot::default(),
            rename: None,
            thumb: 0.,
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
        self.stop_pointer();
        self.dragging = None;
        self.drop_over = None;
    }

    /// Lets go of a press or a marquee in progress.
    fn stop_pointer(&mut self) {
        self.press = None;
        self.marquee = None;
        self.marquee_scroll = None;
        self.scrub.clear();
        self.sync_busy();
    }

    /// The window-level pointer listeners only do anything while something needs them.
    fn sync_busy(&self) {
        self.busy
            .set(self.press.is_some() || self.marquee.is_some() || self.dragging.is_some());
    }

    /// The drag out of the window is over: the dragged tiles are drawn as they were. Called by itself on the first
    /// pointer move with no button down; call it when the drag's future finishes to be sure.
    pub fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.dragging.take().is_some() {
            self.sync_busy();
            cx.notify();
        }
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
        self.dragging = None;
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
                    } else if keep {
                        // The others stay selected until release (so all of them can be dragged); the focus and
                        // the anchor move to this one at once (`updateTab({ focus, anchor })`).
                        explorer.set_focus(Some(&entry.path), Some(&entry.path), cx);
                    } else {
                        explorer.select_only(Some(&entry.path), cx);
                    }
                });
                self.press = Some(Press {
                    at: event.position,
                    narrow_to: keep.then(|| entry.path.clone()),
                });
                self.sync_busy();
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

    /// Hovering a folder tile reads it ahead; leaving it lets go (`hoverFolder`, `leaveFolder`). A video tile gets
    /// its scrub overlay while the pointer is on it, and only then.
    fn tile_hover(&mut self, index: usize, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        let Some((rows, _)) = self.snapshot(cx) else {
            return;
        };
        let Some(entry) = rows.get(index) else {
            return;
        };
        let path = entry.path.clone();
        if scrub::scrubbable(entry) {
            if hovered {
                let thumb = self.thumb;
                self.scrub.enter(&path, thumb, window.scale_factor(), cx);
                cx.notify();
            } else if self.scrub.leave(&path) {
                cx.notify();
            }
            return;
        }
        self.explorer.update(cx, |explorer, cx| {
            if hovered {
                explorer.hover_folder(&path, cx);
            } else {
                explorer.leave_folder();
            }
        });
    }

    /// A press on the empty part: without Ctrl or Shift it clears the selection, and dragging draws a marquee that
    /// selects what it covers, added to what was selected with Ctrl (`onBackgroundMouseDown`, `startMarquee`).
    fn background_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.dragging = None;
        window.focus(&self.focus, cx);
        self.activate_pane(cx);
        if event.button != MouseButton::Left {
            return;
        }
        if !event.modifiers.control && !event.modifiers.shift {
            self.explorer.update(cx, |explorer, cx| explorer.select_only(None, cx));
        }
        let base = if event.modifiers.control {
            self.snapshot(cx)
                .map(|(_, selection)| selection.iter().map(str::to_string).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.marquee = Some(Marquee {
            start: self.content_point(event.position),
            pointer: event.position,
            base,
            shown: false,
            last: None,
        });
        // Held near the top or bottom, the body scrolls, and what comes into view is covered too.
        self.marquee_scroll = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(MARQUEE_TICK).await;
            if this.update(cx, |this, cx| this.marquee_scroll_tick(cx)).is_err() {
                break;
            }
        }));
        self.sync_busy();
    }

    /// A window position as a point in the content: x from the body's left edge, y from the content's top.
    fn content_point(&self, at: Point<Pixels>) -> ContentPoint {
        let body = self.body.get();
        ContentPoint {
            x: f32::from(at.x - body.origin.x),
            y: f32::from(at.y - body.origin.y) + self.scroll_top(),
        }
    }

    /// The pointer moved with a marquee under way.
    fn marquee_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            self.stop_pointer();
            cx.notify();
            return;
        }
        let here = self.content_point(event.position);
        let Some(marquee) = &mut self.marquee else {
            return;
        };
        marquee.pointer = event.position;
        // Small movements are a click on the empty part, not a marquee.
        let moved =
            (here.x - marquee.start.x).abs() >= MARQUEE_AFTER || (here.y - marquee.start.y).abs() >= MARQUEE_AFTER;
        if moved {
            marquee.shown = true;
        }
        if marquee.shown {
            self.marquee_apply(cx);
            cx.notify();
        }
    }

    /// Selects what the marquee covers now (`commitHits`): the tiles under its rectangle, with what was selected
    /// at the start when Ctrl was held. The first tile it covers is the anchor, the last the focus.
    fn marquee_apply(&mut self, cx: &mut Context<Self>) {
        let (Some(metrics), Some((rows, _))) = (self.metrics, self.snapshot(cx)) else {
            return;
        };
        let Some(marquee) = &self.marquee else {
            return;
        };
        let (a, b) = (marquee.start, self.content_point(marquee.pointer));
        // Tiles run from the right in Arabic: the columns are counted from there.
        let rtl = i18n::is_rtl();
        let mirrored = |p: ContentPoint| ContentPoint {
            x: if rtl { metrics.content_w - p.x } else { p.x },
            y: p.y,
        };
        let hits = marquee_hits(
            mirrored(a),
            mirrored(b),
            rows.len(),
            metrics.row_height,
            metrics.cols,
            metrics.content_w,
            metrics.pad,
        );
        let (first, last) = (hits.first().copied(), hits.last().copied());
        let summary = (hits.len(), first, last);
        if marquee.last == Some(summary) {
            return;
        }
        let mut chosen = marquee.base.clone();
        chosen.extend(hits.iter().filter_map(|&i| rows.get(i)).map(|entry| entry.path.clone()));
        let path_of = |i: Option<usize>| i.and_then(|i| rows.get(i)).map(|entry| entry.path.clone());
        let (anchor, focus) = (path_of(first), path_of(last));
        if let Some(marquee) = &mut self.marquee {
            marquee.last = Some(summary);
        }
        self.explorer.update(cx, |explorer, cx| {
            explorer.set_selection(chosen, anchor.as_deref(), focus.as_deref(), cx)
        });
    }

    /// Every so often while a marquee is held near the top or bottom: scroll that way by half a row.
    fn marquee_scroll_tick(&mut self, cx: &mut Context<Self>) {
        let (Some(metrics), Some(marquee)) = (self.metrics, &self.marquee) else {
            return;
        };
        if !marquee.shown {
            return;
        }
        let body = self.body.get();
        let (top, bottom) = (f32::from(body.origin.y), f32::from(body.origin.y + body.size.height));
        let y = f32::from(marquee.pointer.y);
        let edge = metrics.rem * MARQUEE_EDGE;
        let step = if y < top + edge {
            -1.0
        } else if y > bottom - edge {
            1.0
        } else {
            return;
        };
        let rows = self.snapshot(cx).map_or(0, |(rows, _)| rows.len());
        let content = rows.div_ceil(metrics.cols.max(1)) as f32 * metrics.row_height;
        let most = (content - f32::from(body.size.height)).max(0.0);
        let to = (self.scroll_top() + step * metrics.row_height / 2.0).clamp(0.0, most);
        if (to - self.scroll_top()).abs() > f32::EPSILON {
            self.set_scroll_top(to);
            self.marquee_apply(cx);
            cx.notify();
        }
    }

    /// The pointer moved with a press, a marquee or a drag out under way: far enough, and a press is a drag.
    fn pointer_moved(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.marquee.is_some() {
            self.marquee_moved(event, cx);
            return;
        }
        // The drag out is over once the pointer moves with the button up (nothing moves while Windows has it).
        if self.press.is_none() && self.dragging.is_some() {
            if event.pressed_button.is_none() {
                self.end_drag(cx);
            }
            return;
        }
        let Some(press) = &self.press else {
            self.sync_busy();
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.press = None;
            self.sync_busy();
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
        self.sync_busy();
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };
        let chosen: Vec<&FileEntry> = rows.iter().filter(|entry| selection.contains(&entry.path)).collect();
        // Out of the Recycle Bin, an item would land named as Windows keeps it there: Restore it instead.
        if chosen.is_empty() || chosen.iter().any(|entry| is_recycled(entry)) {
            return;
        }
        let paths: Vec<String> = chosen.iter().map(|entry| entry.path.clone()).collect();
        let from_archive = chosen.iter().any(|entry| is_archived(entry));
        // The dragged items show dim (`dragging`) until the drag is over; out of an archive they are extracted
        // first, so those wait for their copies and aren't dimmed.
        if !from_archive {
            self.dragging = Some(Arc::new(paths.iter().cloned().collect()));
            self.sync_busy();
            cx.notify();
        }
        cx.emit(FileGridEvent::DragStart { paths, from_archive });
    }

    /// A button went up anywhere: a press on a multi-selection that wasn't a drag narrows to its item, and a
    /// marquee ends.
    fn pointer_released(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left {
            return;
        }
        if self.marquee.as_ref().is_some_and(|marquee| marquee.shown) {
            self.marquee_apply(cx);
        }
        let narrow = self.press.take().and_then(|press| press.narrow_to);
        let had_marquee = self.marquee.is_some();
        self.marquee = None;
        self.marquee_scroll = None;
        self.sync_busy();
        if had_marquee {
            cx.notify();
        }
        if let Some(path) = narrow {
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
        // The rename box has its keys: the arrows and letters typed there move no tile.
        if self.is_renaming() {
            return;
        }
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
                        // Only the cursor moves (`updateTab({ focus })`); the view follows it when it is drawn.
                        self.explorer
                            .update(cx, |explorer, cx| explorer.set_focus(Some(&path), None, cx));
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
                let searching = self
                    .explorer
                    .read(cx)
                    .pane_tab(self.pane)
                    .filter(|tab| tab.find.is_some() || !tab.filter.is_empty())
                    .map(|tab| tab.id);
                if let Some(tab) = searching {
                    cx.stop_propagation();
                    // `clearFind`: the filter goes, and a search's results end with it.
                    self.explorer.update(cx, |explorer, cx| explorer.clear_find_in(tab, cx));
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

    /// Shift+F10 or the menu key: the menu of what is selected, at the focused tile's bottom-start corner (the
    /// bottom left in English, the bottom right in Arabic), kept inside the body.
    fn keyboard_menu(&mut self, cx: &mut Context<Self>) {
        let Some((rows, selection)) = self.snapshot(cx) else {
            return;
        };
        let Some(at) = usize::try_from(Self::focus_index(&rows, &selection)).ok() else {
            return;
        };
        let (Some(entry), Some(metrics)) = (rows.get(at), self.metrics) else {
            return;
        };
        let cols = metrics.cols.max(1);
        let (row, col) = (at / cols, at % cols);
        let body = self.body.get();
        // The tile's start edge, from the body's start edge (its left, or its right in Arabic).
        let along = metrics.pad + col as f32 * metrics.col_w + metrics.inset;
        let x = if i18n::is_rtl() {
            f32::from(body.origin.x) + metrics.content_w - along
        } else {
            f32::from(body.origin.x) + along
        };
        let (top, bottom) = (f32::from(body.origin.y), f32::from(body.origin.y + body.size.height));
        let y = (top + (row + 1) as f32 * metrics.row_height - metrics.inset - self.scroll_top()).clamp(top, bottom);
        cx.emit(FileGridEvent::ContextMenu {
            path: entry.path.clone(),
            position: point(px(x), px(y)),
        });
    }

    // ---- Drops ------------------------------------------------------------------------------------

    /// The tile under a window position, if the pointer is over one (not between them, not in the gutter).
    fn tile_at(&self, at: Point<Pixels>, count: usize) -> Option<usize> {
        let metrics = self.metrics?;
        let body = self.body.get();
        if !body.contains(&at) || metrics.col_w <= 0.0 || metrics.row_height <= 0.0 {
            return None;
        }
        let point = self.content_point(at);
        let x = if i18n::is_rtl() {
            metrics.content_w - point.x
        } else {
            point.x
        };
        let col = ((x - metrics.pad) / metrics.col_w).floor();
        let row = (point.y / metrics.row_height).floor();
        if !(col >= 0.0 && row >= 0.0 && col < metrics.cols as f32) {
            return None;
        }
        // Both are whole and non-negative, and the column is below the column count: the casts are exact (a row
        // far beyond the rows is cut off by the count below).
        let index = (row as usize).checked_mul(metrics.cols)?.checked_add(col as usize)?;
        (index < count).then_some(index)
    }

    /// What a drop of `dragged` at `at` would go into: the folder tile under the pointer, else the folder shown,
    /// as the web app's `targetAt` decided. `None` where nothing can be dropped, or where it would do nothing.
    fn drop_under(&self, at: Point<Pixels>, dragged: &[String], cx: &App) -> Option<(String, DropOver)> {
        let explorer = self.explorer.read(cx);
        let tab = explorer.pane_tab(self.pane)?;
        let over = self
            .tile_at(at, tab.rows.len())
            .and_then(|i| tab.rows.get(i))
            .and_then(drop_folder);
        let (dest, over) = match over {
            Some(path) => (path.to_string(), DropOver::Tile(path.to_string())),
            // Nothing can be dropped into an archive (it is read-only), into a folder Windows keeps closed, or
            // into This PC or a search's results, which are no folder.
            None if !tab.is_this_pc() && tab.find.is_none() && tab.archive().is_none() && !tab.is_closed() => {
                (tab.path.clone(), DropOver::Pane)
            }
            None => return None,
        };
        // Where they all are already, or into one of them: nothing to do there.
        let nothing = !dragged.is_empty() && !is_recycle_bin(&dest) && droppable(dragged, &dest).is_empty();
        (!nothing).then_some((dest, over))
    }

    /// The words on the drag's picture for a drop into `dest`, and whether it always moves.
    fn drop_words(&self, dest: &str, cx: &App) -> Under {
        // The fixed names ("This PC", "Recycle Bin"...) are translated; a folder's name is as it is.
        let name = self.explorer.read(cx).tab_name(dest);
        Under {
            name: glint_model::i18n::translate(i18n::language(), &name).to_string(),
            copy: t!("Copy to %1").to_string(),
            moves: t!("Move to %1").to_string(),
            always_move: is_recycle_bin(dest),
        }
    }

    /// Files dragged over the grid moved (every grid hears every move): answer what a drop at the pointer would
    /// do, and light the place it would go.
    fn files_over(&mut self, event: &DragMoveEvent<ExternalPaths>, cx: &mut Context<Self>) {
        let dragged = event
            .dragged_item()
            .downcast_ref::<ExternalPaths>()
            .map(drops::paths)
            .unwrap_or_default();
        let target = if event.bounds.contains(&event.event.position) {
            self.drop_under(event.event.position, &dragged, cx)
        } else {
            None
        };
        if let Some((dest, _)) = &target {
            drops::answer(cx, Answer::Accept(self.drop_words(dest, cx)));
        }
        let over = target.map(|(_, over)| over);
        if self.drop_over != over {
            self.drop_over = over;
            cx.notify();
        }
    }

    /// Files were dropped at `at`: into the folder under the pointer (`dropFiles`), copied, or moved when the drop
    /// moves (Shift, or the Recycle Bin: recycled). A link copies, as it did on the web.
    fn files_dropped(&mut self, drop: &ExternalPaths, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.drop_over = None;
        cx.notify();
        let paths = drops::paths(drop);
        let Some((dest, _)) = self.drop_under(at, &paths, cx) else {
            return;
        };
        let moves = drops::effect(cx) == DropEffect::Move;
        self.explorer
            .update(cx, |explorer, cx| explorer.drop_files(paths, &dest, moves, cx).detach());
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

/// A folder that files dragged in can be dropped on: not one in an archive (read-only), nor one Windows keeps
/// closed, nor one in the Recycle Bin (`dropPath`).
// TODO(coordinator): from file_list::drop_path (the list's copy), once the views share it.
fn drop_folder(entry: &FileEntry) -> Option<&str> {
    (entry.dir && !is_archived(entry) && !is_closed(entry) && !is_recycled(entry)).then_some(entry.path.as_str())
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
        let cut = explorer.cut_paths().clone();
        let saved = explorer.saved_scroll(tab_id, &format!("{view_key}:grid"));
        let located = search || recycled;
        // The sizes "Folder sizes" measured for this folder (a search's results come from many folders).
        let run = explorer.size_run(&tab.path).filter(|_| !search);
        let (sizes, measuring) = (run.map(|run| run.sizes.clone()), run.is_some_and(|run| run.running));
        // What Git says of the folder's items, and how they differ from the other pane's (search results have neither).
        let git = super::file_list::marks_in(cx, tab_id, &tab.path).filter(|_| !search);
        let diffs = explorer.comparison_of(tab_id).filter(|_| !search);
        let diff_same = diffs
            .as_ref()
            .filter(|_| explorer.diff_only)
            .map_or(0, |diffs| diffs.same);
        // The rename box follows the tab's `renaming`: the item it names, as the grid has it.
        let renaming = tab.renaming.clone();
        let find_renamed = |path: &str| {
            let entry = rows.iter().find(|entry| entry.path == path)?;
            Some((entry.path.clone(), entry.name.clone(), entry.dir))
        };
        self.sync_rename(renaming.as_deref(), find_renamed, window, cx);

        // The layout: tiles from the tile-size setting and the text size, as many to a row as the width holds.
        let tile = tile_layout(rem, state::settings(cx).tile_size.as_str());
        self.thumb = tile.thumb;
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
        self.metrics = Some(Metrics {
            cols,
            row_height: layout.row_height,
            pad: tile.pad,
            col_w,
            content_w: (width - gutter).max(0.0),
            inset: rem * 0.125,
            rem,
        });

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

        // Files dragged over the grid light the folder tile they would go into, or the whole grid; a drag that has
        // ended (or left the window) lights nothing, whatever was last seen.
        let drop_over = self.drop_over.clone().filter(|_| cx.has_active_drag());
        let frame = Rc::new(tile::Frame {
            rows: rows.clone(),
            selection: selection.clone(),
            focus,
            inactive,
            flashed,
            cut,
            dragging: self.dragging.clone(),
            drop_tile: match &drop_over {
                Some(DropOver::Tile(path)) => Some(path.clone()),
                _ => None,
            },
            tile,
            cols,
            lines: line_count,
            col_w,
            located,
            sizes,
            measuring,
            git,
            diffs,
            recycled,
            gallery,
            scope,
            lane,
            scale,
            tokens: tokens.clone(),
            thumbs: self.thumbs.clone(),
            viewer: cx.entity_id(),
            shown: self.shown.clone(),
            scrub: self.scrub.current(),
            on_down: handler(cx, |this, (i, event): &(usize, MouseDownEvent), window, cx| {
                this.tile_down(*i, event, window, cx)
            }),
            on_right_up: handler(cx, |this, (i, event): &(usize, MouseUpEvent), _, cx| {
                this.tile_context_menu(*i, event, cx)
            }),
            on_hover: handler(cx, |this, (i, hovered): &(usize, bool), window, cx| {
                this.tile_hover(*i, *hovered, window, cx)
            }),
            rename: self.rename_view(cx),
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
        let empty =
            (rows.is_empty() && has_listing).then(|| empty::Empty::of(&filter, recycled, listed, archive, diff_same));

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
            .children(empty.map(|empty| {
                let tab = self.explorer.read(cx).pane_tab_id(self.pane);
                div().absolute().inset_0().child(empty::view(&empty, tab, cx))
            }))
            .children(self.marquee_rect(&tokens))
            .child(self.taps(width, body_height, cx));

        col()
            .id(("file-grid-pane", self.pane))
            .relative()
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
            // Files dropped in (from File Explorer, the desktop, the other pane): every grid hears every move and
            // answers when the pointer is over it.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| this.files_over(event, cx)))
            .on_drop(cx.listener(|this, drop: &ExternalPaths, window, cx| {
                this.files_dropped(drop, window.mouse_position(), cx)
            }))
            .child(self.header(sort, recycled, located, gallery, cx))
            .child(body)
            .children((drop_over == Some(DropOver::Pane) && !search).then(|| {
                // `bg-primary/5 outline-2 outline-dashed outline-primary/60`
                div()
                    .id("file-grid-drop")
                    .test_support()
                    .absolute()
                    .inset_0()
                    .border_2()
                    .border_dashed()
                    .border_color(tokens.primary.opacity(0.6))
                    .bg(tokens.primary.opacity(0.05))
            }))
            .into_any_element()
    }
}

impl FileGrid {
    /// The marquee being dragged, as a rectangle over the body (clipped to it): from where it started in the
    /// content, so it stays with the tiles as the body scrolls, to the pointer.
    fn marquee_rect(&self, tokens: &theme::Tokens) -> Option<impl IntoElement> {
        let marquee = self.marquee.as_ref().filter(|marquee| marquee.shown)?;
        let body = self.body.get();
        let (width, height) = (f32::from(body.size.width), f32::from(body.size.height));
        let end = self.content_point(marquee.pointer);
        let (left, right) = (marquee.start.x.min(end.x), marquee.start.x.max(end.x));
        let top = marquee.start.y.min(end.y) - self.scroll_top();
        let bottom = marquee.start.y.max(end.y) - self.scroll_top();
        // Kept to the body, so it neither draws over the header nor grows past the window.
        let (left, right) = (left.clamp(0.0, width), right.clamp(0.0, width));
        let (top, bottom) = (top.clamp(0.0, height), bottom.clamp(0.0, height));
        // `border border-primary/80 bg-primary/15 rounded-[4px]`, a faint ring around.
        let mark = div()
            .id("file-grid-marquee")
            .test_support()
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(right - left))
            .h(px(bottom - top))
            .rounded(theme::size::RADIUS_SM)
            .border_1()
            .border_color(tokens.primary.opacity(0.8))
            .bg(tokens.primary.opacity(0.15));
        Some(div().absolute().inset_0().overflow_hidden().child(mark))
    }

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
