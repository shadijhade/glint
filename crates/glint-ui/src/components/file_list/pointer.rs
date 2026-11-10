//! What the pointer does on the list besides clicking: a press on an item that turns into dragging the
//! selection out of the window, and a press on the empty part that turns into a rectangle selecting the rows it
//! crosses (`onRowMouseDown`'s press, `startMarquee` and the window-level `mousemove` and `mouseup` listeners in
//! Explorer.tsx).
//!
//! The web listened on `window` for both. Here a canvas under the list registers window-level mouse listeners
//! while a press or a marquee is under way ([`FileList::tracking`] says so), so the drag follows the pointer
//! wherever it goes in the window, and the button going up anywhere ends it.

use super::{DragFromArchive, FileList, Press, ROW_HEIGHT};
use crate::explorer::Rows;
use crate::kit::prelude::*;
use crate::t;
use glint_model::entries::{is_archived, is_recycled, FileEntry};
use glint_model::grid::{marquee_hits, Point as ContentPoint};
use std::cell::Cell;
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// A press that moves this far (pixels) with the button down is a drag (`Math.hypot(…) >= 6`).
const DRAG_AFTER: f32 = 6.;
/// A marquee shows only once the pointer has moved this far on either axis (`< 4` on both: nothing yet).
const MARQUEE_AFTER: f32 = 4.;
/// How often the list scrolls while the pointer is held at its edge (`setInterval(…, 30)`).
const SCROLL_EVERY: Duration = Duration::from_millis(30);

/// A rectangle being dragged over the list's empty part.
pub(super) struct Marquee {
    /// Where the button went down, in the list's content pixels (the scrolled part included).
    start: ContentPoint,
    /// Where the pointer is now, in window pixels.
    pointer: Point<Pixels>,
    /// What was selected when the button went down with Ctrl held: it stays selected.
    base: Vec<String>,
    /// The pointer has moved far enough for the rectangle to show.
    drawn: bool,
    /// How many rows the rectangle covered last, and the first and last of them: a move that changes none of
    /// them changes nothing.
    last: Option<(usize, Option<usize>, Option<usize>)>,
    /// The task scrolling the list while the pointer is held beyond its top or bottom.
    _scroller: Option<Task<()>>,
}

/// The window-level mouse listeners a canvas under the list registers each frame: they do nothing unless a
/// press or a marquee is being tracked.
#[derive(Clone)]
pub(super) struct Tracker {
    this: WeakEntity<FileList>,
    tracking: Rc<Cell<bool>>,
}

impl Tracker {
    /// Registers the listeners for this frame (call it while painting).
    pub(super) fn track(&self, window: &mut Window) {
        let (this, tracking) = (self.this.clone(), self.tracking.clone());
        window.on_mouse_event({
            let (this, tracking) = (this.clone(), tracking.clone());
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && tracking.get() {
                    this.update(cx, |this, cx| this.pointer_moved(event, window, cx)).ok();
                }
            }
        });
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture && tracking.get() && event.button == MouseButton::Left {
                this.update(cx, |this, cx| this.pointer_released(window, cx)).ok();
            }
        });
    }
}

/// The future of a drag that ends when the shell's drag loop does.
type Dragging = Pin<Box<dyn Future<Output = Result<String, String>>>>;

/// Starts dragging `paths` out of the window (`startDrag`). `platform::dragdrop` posts the modal loop to the
/// window's message pump itself.
#[cfg(not(test))]
fn start_drag(cx: &App, paths: Vec<String>, copy_only: bool) -> Dragging {
    Box::pin(crate::platform::dragdrop::drag_out(
        cx,
        crate::workspace::MAIN,
        paths,
        copy_only,
    ))
}

thread_local! {
    /// Tests only: the drags that were started, in place of the shell's drag loop.
    #[cfg(test)]
    pub(super) static STARTED: std::cell::RefCell<Vec<(Vec<String>, bool)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn start_drag(_: &App, paths: Vec<String>, copy_only: bool) -> Dragging {
    STARTED.with(|started| started.borrow_mut().push((paths, copy_only)));
    Box::pin(async { Ok("copy".to_string()) })
}

impl FileList {
    /// The list's scroll, in pixels from the top.
    pub(super) fn scroll_top(&self) -> f32 {
        -f32::from(self.scroll.0.borrow().base_handle.offset().y)
    }

    /// A window position in the list's content pixels: from its left edge, from its top with the scrolled
    /// part included (`at` in `startMarquee`).
    fn content_point(&self, at: Point<Pixels>) -> ContentPoint {
        let area = self.area.get();
        ContentPoint {
            x: f32::from(at.x - area.origin.x),
            y: f32::from(at.y - area.origin.y) + self.scroll_top(),
        }
    }

    /// What the canvas under the list needs to register the window-level mouse listeners each frame.
    pub(super) fn tracker(&self) -> Tracker {
        Tracker {
            this: self.this.clone(),
            tracking: self.tracking.clone(),
        }
    }

    /// Nothing is pressed any more: the window listeners can rest.
    fn stop_tracking_if_idle(&self) {
        if self.press.is_none() && self.marquee.is_none() {
            self.tracking.set(false);
        }
    }

    /// A press on an item may turn into a drag: remembered with where it began (`press.current`).
    pub(super) fn begin_press(&mut self, narrow_to: Option<String>, at: Point<Pixels>) {
        self.press = Some(Press { narrow_to, at });
        self.tracking.set(true);
    }

    /// The pointer moved while a press or a marquee is tracked.
    pub(super) fn pointer_moved(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.pressed_button != Some(MouseButton::Left) {
            // The button went up where this window didn't hear it (outside it, or while a menu had the mouse).
            self.press = None;
            self.marquee = None;
            self.stop_tracking_if_idle();
            cx.notify();
            return;
        }
        if let Some(marquee) = self.marquee.as_mut() {
            marquee.pointer = event.position;
            self.update_marquee(window, cx);
            return;
        }
        let Some(press) = &self.press else {
            self.stop_tracking_if_idle();
            return;
        };
        let (dx, dy) = (
            f32::from(event.position.x - press.at.x),
            f32::from(event.position.y - press.at.y),
        );
        if dx.hypot(dy) >= DRAG_AFTER {
            self.press = None;
            self.stop_tracking_if_idle();
            self.drag_out(window, cx);
        }
    }

    /// The left button went up: a marquee ends, a press kept on one item of a selection narrows to it.
    pub(super) fn pointer_released(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.marquee.is_some() {
            self.commit_marquee(window, cx);
            self.marquee = None;
            cx.notify();
        }
        self.release(cx);
        self.stop_tracking_if_idle();
    }

    // ---- Dragging the selection out ---------------------------------------------------------------------

    /// The selection is dragged out of the window, into File Explorer, the desktop, a mail (or the other pane).
    /// Items in an archive are extracted first, which is the archive part's: the list only says so. Items in
    /// the Recycle Bin can't be dragged (they would land named as Windows keeps them there: Restore instead).
    fn drag_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, rows, selection)) = self
            .explorer
            .read(cx)
            .pane_tab(self.pane)
            .map(|tab| (tab.id, tab.rows.clone(), tab.selection.clone()))
        else {
            return;
        };
        let chosen: Vec<&FileEntry> = rows.iter().filter(|entry| selection.contains(&entry.path)).collect();
        if chosen.is_empty() || chosen.iter().any(|entry| is_recycled(entry)) {
            return;
        }
        let paths: Vec<String> = chosen.iter().map(|entry| entry.path.clone()).collect();
        if chosen.iter().any(|entry| is_archived(entry)) {
            cx.emit(DragFromArchive { paths });
            return;
        }
        self.dragging = Arc::new(paths.iter().cloned().collect::<HashSet<_>>());
        cx.notify();
        let ended = start_drag(cx, paths, false);
        cx.spawn_in(window, async move |this, cx| {
            let result = ended.await;
            this.update_in(cx, |this, window, cx| {
                this.dragging = Arc::new(HashSet::new());
                match result {
                    // Moved away (into another drive's folder in File Explorer, say): show what is left. A move
                    // into a folder on the same drive comes back as "none", File Explorer or Glint moved them itself.
                    Ok(effect) if effect == "move" || effect == "none" => {
                        this.explorer
                            .update(cx, |explorer, cx| explorer.reload(Some(id), cx).detach());
                    }
                    Ok(_) => {}
                    Err(reason) => {
                        window.push_notification(Notification::error(reason).title(t!("Couldn't drag these items")), cx)
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- The marquee ------------------------------------------------------------------------------------

    /// A press on the empty part starts a marquee. Nothing shows until the pointer has moved; with Ctrl held
    /// what was selected stays selected.
    pub(super) fn begin_marquee(&mut self, at: Point<Pixels>, additive: bool, cx: &mut Context<Self>) {
        let base = match (additive, self.explorer.read(cx).pane_tab(self.pane)) {
            (true, Some(tab)) => tab.selection.iter().map(str::to_string).collect(),
            _ => Vec::new(),
        };
        self.marquee = Some(Marquee {
            start: self.content_point(at),
            pointer: at,
            base,
            drawn: false,
            last: None,
            _scroller: None,
        });
        self.tracking.set(true);
    }

    /// The pointer or the scroll changed: the rectangle follows, and the rows it covers are selected.
    fn update_marquee(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pointer) = self.marquee.as_ref().map(|marquee| marquee.pointer) else {
            return;
        };
        let end = self.content_point(pointer);
        let newly_drawn = {
            let Some(marquee) = self.marquee.as_mut() else { return };
            if !marquee.drawn
                && (end.x - marquee.start.x).abs() < MARQUEE_AFTER
                && (end.y - marquee.start.y).abs() < MARQUEE_AFTER
            {
                return;
            }
            !std::mem::replace(&mut marquee.drawn, true)
        };
        if newly_drawn {
            // Held beyond the top or bottom, the list scrolls, and what comes into view is covered too.
            let scroller = cx.spawn_in(window, async move |this, cx| loop {
                cx.background_executor().timer(SCROLL_EVERY).await;
                if !matches!(
                    this.update_in(cx, |this, window, cx| this.scroll_tick(window, cx)),
                    Ok(true)
                ) {
                    break;
                }
            });
            if let Some(marquee) = self.marquee.as_mut() {
                marquee._scroller = Some(scroller);
            }
        }
        cx.notify();
        self.commit_marquee(window, cx);
    }

    /// One step of the list scrolling under a held marquee: half a row towards the edge the pointer is at or
    /// beyond (`list.scrollTop += rowHeight / 2`). False once there is no marquee.
    pub(super) fn scroll_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(marquee) = self.marquee.as_ref().filter(|marquee| marquee.drawn) else {
            return self.marquee.is_some();
        };
        let area = self.area.get();
        let edge = space(6.).to_pixels(window.rem_size());
        let step = if marquee.pointer.y < area.origin.y + edge {
            -1.
        } else if marquee.pointer.y > area.bottom() - edge {
            1.
        } else {
            0.
        };
        if step != 0. {
            let row = f32::from(ROW_HEIGHT.to_pixels(window.rem_size()));
            let lines = self
                .explorer
                .read(cx)
                .pane_tab(self.pane)
                .map_or(0, |tab| layout_rows::line_count(&tab.rows));
            let most = (lines as f32 * row - f32::from(area.size.height)).max(0.);
            let top = (self.scroll_top() + step * row / 2.).clamp(0., most);
            self.scroll.0.borrow().base_handle.set_offset(point(px(0.), px(-top)));
            self.update_marquee(window, cx);
        }
        true
    }

    /// Selects what the rectangle covers now, if that differs from last time (`commitHits`): the rows it
    /// crosses, added to what was selected with Ctrl; the first of them is the anchor, the last has the focus.
    fn commit_marquee(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(marquee) = self.marquee.as_ref().filter(|marquee| marquee.drawn) else {
            return;
        };
        let end = self.content_point(marquee.pointer);
        let start = marquee.start;
        let Some(rows) = self.explorer.read(cx).pane_tab(self.pane).map(|tab| tab.rows.clone()) else {
            return;
        };
        let height = f32::from(ROW_HEIGHT.to_pixels(window.rem_size()));
        let covered = marquee_hits(start, end, layout_rows::line_count(&rows), height, 1, 0., 0.);
        // Grouped, the list's lines are headers and rows: what is covered is the rows among them.
        let hits: Vec<usize> = covered
            .into_iter()
            .filter_map(|line| layout_rows::row_of_line(&rows, line))
            .collect();
        let (first, last) = (hits.first().copied(), hits.last().copied());
        let key = (hits.len(), first, last);
        let Some(marquee) = self.marquee.as_mut() else { return };
        if marquee.last == Some(key) {
            return;
        }
        marquee.last = Some(key);
        let path_at = |i: Option<usize>| i.and_then(|i| rows.get(i)).map(|entry| entry.path.as_str());
        let paths = marquee
            .base
            .iter()
            .map(String::as_str)
            .chain(hits.iter().filter_map(|&i| path_at(Some(i))));
        let (anchor, focus) = (path_at(first), path_at(last));
        self.explorer
            .update(cx, |explorer, cx| explorer.set_selection(paths, anchor, focus, cx));
    }

    /// The rectangle, as a box in the list's own pixels (its top is the list's scroll-less top), or None.
    pub(super) fn marquee_bounds(&self) -> Option<Bounds<Pixels>> {
        let marquee = self.marquee.as_ref().filter(|marquee| marquee.drawn)?;
        let end = self.content_point(marquee.pointer);
        let top = self.scroll_top();
        let (left, right) = (marquee.start.x.min(end.x), marquee.start.x.max(end.x));
        let (above, below) = (marquee.start.y.min(end.y) - top, marquee.start.y.max(end.y) - top);
        Some(Bounds::new(
            point(px(left), px(above)),
            size(px(right - left), px(below - above)),
        ))
    }
}

/// Lines and rows of the list's rows: a grouped list has header lines between the rows.
pub(super) mod layout_rows {
    use super::Rows;

    /// How many lines the list has: its rows, and a header for each group.
    pub fn line_count(rows: &Rows) -> usize {
        rows.groups().map_or(rows.len(), |groups| groups.lines.len())
    }

    /// The row on line `line`, or None on a group's header (and past the end).
    pub fn row_of_line(rows: &Rows, line: usize) -> Option<usize> {
        match rows.groups() {
            Some(groups) => groups
                .lines
                .get(line)
                .copied()
                .filter(|&at| at >= 0)
                .map(|at| at as usize),
            None => (line < rows.len()).then_some(line),
        }
    }

    /// The line a row is on.
    pub fn line_of_row(rows: &Rows, row: usize) -> usize {
        match rows.groups() {
            Some(groups) => groups.line_of.get(row).map_or(row, |&line| line.max(0) as usize),
            None => row,
        }
    }
}
