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
//! - **Pointer**: a right-click on a row opens the item menu for it (`MenuHost::open_entries_menu`, which takes
//!   items the tab's rows don't hold), and on the empty part of a column the menu of its folder; a middle-click on
//!   a folder or an archive opens it in a new tab, as in the list; a press that moves 6 px drags the item out of
//!   the window, and files dropped on a column go into its folder, or into the folder row under the pointer
//!   (copied, or moved with Shift), as in the list. Resting the pointer on a row finds out what isn't known
//!   about its item, so its menu opens knowing it.
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
//! - The last column, for a file, is its inspector: the thumbnail, the facts the menu's head says (type, size,
//!   size in pixels, length, lines, pages) as they are found out, and the badges the menu would show.

#[cfg(test)]
mod tests;

use crate::components::file_grid::thumbnail::{self, Thumbnails};
use crate::components::file_list::calm;
use crate::components::file_list::drop_path;
use crate::components::file_list::pointer::{start_drag, DRAG_AFTER};
use crate::components::menus::head::{head_of, Extras};
use crate::components::menus::{self, Tone};
use crate::explorer::{explorer, CoreSource, Explorer, Fut, ListFailure, Listed, NavOptions, Source, Tab, ViewMode};
use crate::kit::prelude::*;
use crate::platform::dragdrop::{Answer, DropEffect, Under};
use crate::{aware, drops, i18n, state, t, theme};
use glint_model::entries::{is_archive_file, is_archived, is_hidden, is_recycled, FileEntry};
use glint_model::file_types::kind_of;
use glint_model::format::format_count;
use glint_model::paths::{
    base_name, droppable, in_archive_path, is_folder_path, is_recycle_bin, is_remote_path, is_root, parent_path,
    same_path, THIS_PC,
};
use gpui_kit::{ScrollStrategy, UniformListScrollHandle};
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// How a folder is listed: glint-core's in the app, a fake disk's in tests.
pub type Lister = Rc<dyn Fn(&str) -> Fut<Result<Listed, ListFailure>>>;

/// The parent column's width (`w-56`) and the next column's (`w-64`), in rems.
const PARENT_WIDTH: f32 = 14.;
const NEXT_WIDTH: f32 = 16.;

/// The inspector's thumbnail square, in CSS pixels.
const THUMB: f32 = 96.;

/// The row the pointer rests on: the wait before its item is looked at ahead (`aware::rest_on`).
type Resting = Option<Task<()>>;

/// A press on a row that may turn into dragging the item out of the window.
struct Press {
    which: Which,
    index: usize,
    at: Point<Pixels>,
}

/// Where a drop of files would land.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DropOver {
    /// The column's own folder (the empty part, or a row that is not a folder).
    Column(Which),
    /// A folder row of a column.
    Row(Which, usize),
}

/// The folder above a tab's path, or none at This PC and at a root (`ParentColumn`'s `parent`).
pub fn parent_of(path: &str) -> Option<String> {
    if path == THIS_PC || is_root(path) {
        None
    } else {
        Some(parent_path(path))
    }
}

/// Which side column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Where the list was when files were last dragged over it: what a drop is placed by.
    list_at: Bounds<Pixels>,
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
            list_at: Bounds::default(),
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
    /// A press on a row that may become a drag out of the window.
    press: Option<Press>,
    /// Whether a press is under way: the window-level listeners painted each frame do nothing otherwise.
    tracking: Rc<Cell<bool>>,
    /// Where files dragged over the columns would land (lit while they are over it).
    drop_over: Option<DropOver>,
    /// The pointer resting on a row finds out what isn't known about its item.
    resting: Resting,
    /// The thumbnail service, and the file whose thumbnail the inspector asked it for.
    thumbs: Entity<Thumbnails>,
    thumb_of: Option<(String, i64)>,
    /// Redraws when a picture or an answer about the inspector's file arrives (kept only while a file shows).
    _inspector_watch: Vec<Subscription>,
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
            press: None,
            tracking: Rc::new(Cell::new(false)),
            drop_over: None,
            resting: None,
            thumbs: thumbnail::thumbnails(cx),
            thumb_of: None,
            _inspector_watch: Vec::new(),
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

    /// The folder the first column lists, if any.
    pub fn parent_path(&self) -> Option<&str> {
        self.parent.path.as_deref()
    }

    /// The folder the last column lists, if any (the list's focused item, when a folder).
    #[cfg(test)]
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

    /// The pane shows another view: the columns let go of what they listed and read nothing more.
    ///
    /// A pane has its columns view from the start, whichever view it shows. Left to follow the explorer all the
    /// while, each one listed the folder above and every folder the keyboard came to (a second and a third read
    /// of what the list had read ahead), kept those listings, and freed them on the UI thread when the next
    /// folder opened: 12 ms for a folder of 100,000 items, in each pane.
    fn rest(&mut self, cx: &mut Context<Self>) {
        if self.tab_path.is_empty()
            && self.parent.path.is_none()
            && self.next.path.is_none()
            && self.focused.is_none()
            && self.press.is_none()
        {
            return;
        }
        self.tab_path.clear();
        self.focused = None;
        self.leave_inspector(cx);
        self.press = None;
        self.tracking.set(false);
        self.drop_over = None;
        self.resting = None;
        for which in [Which::Parent, Which::Next] {
            let side = self.side_mut(which);
            side.path = None;
            side.seq += 1;
            side.shown.clear();
            side.cursor = 0;
            let raw = side.raw.take();
            crate::explorer::let_go(raw, cx);
        }
        cx.notify();
    }

    /// Reads the pane's tab and the setting, and lists again what changed.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (path, focused) = {
            let explorer = explorer(cx);
            let explorer = explorer.read(cx);
            match self.tab(explorer).filter(|tab| tab.view == ViewMode::Columns) {
                Some(tab) => (
                    tab.path.clone(),
                    tab.selection
                        .focus()
                        .and_then(|focus| tab.rows.position_of(focus))
                        .and_then(|at| tab.rows.get(at))
                        .cloned(),
                ),
                None => {
                    self.rest(cx);
                    return;
                }
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
            self.watch_inspector(cx);
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// The last column shows a file: what isn't known about it is found out at once (it is on screen), and a
    /// picture or an answer that arrives redraws it. A folder or nothing there lets all that go.
    fn watch_inspector(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.focused.clone().filter(|entry| !entry.dir) else {
            self.leave_inspector(cx);
            return;
        };
        if let Some(aware) = aware::try_shared(cx) {
            aware.update(cx, |aware, cx| aware.look_now(std::slice::from_ref(&entry), cx));
            if self._inspector_watch.is_empty() {
                self._inspector_watch.push(cx.observe(&aware, |_, _, cx| cx.notify()));
            }
        }
        if self._inspector_watch.len() < 2 {
            self._inspector_watch.push(cx.observe(&self.thumbs, |_, _, cx| cx.notify()));
        }
    }

    /// No file in the last column: its thumbnail isn't wanted, and nothing more is heard from the sources.
    fn leave_inspector(&mut self, cx: &mut Context<Self>) {
        self._inspector_watch.clear();
        if self.thumb_of.take().is_some() {
            let viewer = cx.entity_id();
            self.thumbs.update(cx, |thumbs, cx| thumbs.release(viewer, cx));
        }
    }

    /// Asks for the inspector file's thumbnail, once per version of the file (at the window's scale).
    fn ask_for_thumbnail(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(entry) = self.focused.clone().filter(|entry| !entry.dir) else {
            return;
        };
        let key = (entry.path.clone(), entry.modified);
        if self.thumb_of.as_ref() == Some(&key) {
            return;
        }
        let wants: Vec<_> = thumbnail::Want::for_entry(&entry, THUMB, window.scale_factor()).into_iter().collect();
        let viewer = cx.entity_id();
        self.thumbs.update(cx, |thumbs, cx| thumbs.show(viewer, "columns-inspector", wants, cx));
        self.thumb_of = Some(key);
    }

    /// Lists `path` into a side column in the background (`useListing`): the column is empty until it arrives, a
    /// failure leaves it empty, and a path that is not a real folder lists nothing.
    fn load(&mut self, which: Which, path: Option<String>, cx: &mut Context<Self>) {
        let side = self.side_mut(which);
        side.path = path.clone();
        side.seq += 1;
        let raw = side.raw.take();
        side.shown.clear();
        side.cursor = 0;
        crate::explorer::let_go(raw, cx);
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

    // ---- The pointer ------------------------------------------------------------------------------------

    /// The pointer came to a row, or left it: resting there finds out what isn't known about its item, so its
    /// menu opens knowing it (docs/gpui/aware). Leaving before the wait is over asks nothing.
    fn rest_on_row(&mut self, which: Which, index: usize, hovered: bool, cx: &mut Context<Self>) {
        self.resting = None;
        if hovered {
            if let Some(entry) = self.side(which).entry(index).cloned() {
                self.resting = aware::rest_on(entry, cx);
            }
        }
    }

    /// The right button on a row: the item's menu, the way the disk usage map asks for the menu of an item the
    /// tab's rows don't hold.
    fn row_menu(&mut self, which: Which, index: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let side = self.side_mut(which);
        side.cursor = index;
        side.focus.focus(window, cx);
        let Some(entry) = self.side(which).entry(index).cloned() else {
            return;
        };
        let (pane, Some(host)) = (self.pane, menus::host(cx)) else {
            return;
        };
        explorer(cx).update(cx, |explorer, cx| explorer.focus_pane(pane, cx));
        host.update(cx, |menus, cx| menus.open_entries_menu(pane, vec![entry], at, window, cx));
    }

    /// The right button on the empty part of a column: the menu of the folder it lists.
    fn column_menu(&mut self, which: Which, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(folder) = self.side(which).path.clone().filter(|path| is_folder_path(path)) else {
            return;
        };
        let Some(host) = menus::host(cx) else {
            return;
        };
        host.update(cx, |menus, cx| menus.open_folder_menu(&folder, at, window, cx));
    }

    /// The middle button on a row: a folder or an archive opens in a new tab, as in the list.
    fn row_in_new_tab(&mut self, which: Which, index: usize, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let Some(entry) = self.side(which).entry(index) else {
            return;
        };
        if (entry.dir || is_archive_file(entry)) && !is_recycled(entry) {
            let path = entry.path.clone();
            explorer(cx).update(cx, |explorer, cx| explorer.open_in_new_tab(&path, cx));
        }
    }

    /// The left button went down on a row: it may be the start of a drag out of the window.
    fn row_pressed(&mut self, which: Which, index: usize, at: Point<Pixels>) {
        self.press = Some(Press { which, index, at });
        self.tracking.set(true);
    }

    /// The pointer moved while a press is tracked: far enough, and the pressed item is dragged out of the window
    /// (into File Explorer, the desktop, a mail). What is in an archive or in the Recycle Bin isn't: the list
    /// extracts the first, and Restore is for the second.
    fn pointer_moved(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = &self.press else {
            self.tracking.set(false);
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.pointer_released();
            return;
        }
        let (dx, dy) = (f32::from(event.position.x - press.at.x), f32::from(event.position.y - press.at.y));
        if dx.hypot(dy) < DRAG_AFTER {
            return;
        }
        let (which, index) = (press.which, press.index);
        self.pointer_released();
        let Some(entry) = self.side(which).entry(index) else {
            return;
        };
        if is_archived(entry) || is_recycled(entry) {
            return;
        }
        let ended = start_drag(cx, vec![entry.path.clone()], false);
        cx.spawn_in(window, async move |this, cx| {
            let result = ended.await;
            this.update_in(cx, |this, window, cx| match result {
                // Moved away: what the columns list is out of date.
                Ok(effect) if effect == "move" || effect == "none" => {
                    for which in [Which::Parent, Which::Next] {
                        let path = this.side(which).path.clone();
                        this.load(which, path, cx);
                    }
                }
                Ok(_) => {}
                Err(reason) => {
                    window.push_notification(Notification::error(reason).title(t!("Couldn't drag these items")), cx)
                }
            })
            .ok();
        })
        .detach();
    }

    /// No press is under way any more (the button went up, anywhere).
    fn pointer_released(&mut self) {
        self.press = None;
        self.tracking.set(false);
    }

    /// Where a drop of `dragged` at `at` would go in the column whose list is at `list` (window pixels): a folder
    /// row under the pointer takes it, else the folder the column lists. With the folder it goes into. None where
    /// nothing can be dropped (what the list refuses: a read-only place, where the files already are).
    fn drop_target(
        &self,
        which: Which,
        at: Point<Pixels>,
        list: Bounds<Pixels>,
        dragged: &[String],
        window: &Window,
    ) -> Option<(DropOver, String)> {
        let side = self.side(which);
        let height = f32::from(space(7.).to_pixels(window.rem_size()));
        let scrolled = -f32::from(side.scroll.0.borrow().base_handle.offset().y);
        let line = (f32::from(at.y - list.origin.y) + scrolled) / height;
        let row = (line >= 0.).then_some(line as usize);
        let over_folder = row.and_then(|row| side.entry(row).and_then(drop_path).map(|path| (row, path.to_string())));
        let (place, dest) = match over_folder {
            Some((row, path)) => (DropOver::Row(which, row), path),
            None => {
                let folder = side.path.clone().filter(|path| is_folder_path(path))?;
                (DropOver::Column(which), folder)
            }
        };
        if is_remote_path(&dest) || in_archive_path(&dest) {
            return None;
        }
        // Where they all are already, or into one of them: nothing to do there.
        if !is_recycle_bin(&dest) && droppable(dragged, &dest).is_empty() {
            return None;
        }
        Some((place, dest))
    }

    /// What a folder is called on the dragged picture, with the words for copying and moving into it.
    fn drop_words(&self, dest: &str, cx: &App) -> Under {
        Under {
            name: explorer(cx).read(cx).tab_name(dest),
            copy: t!("Copy to %1").to_string(),
            moves: t!("Move to %1").to_string(),
            always_move: is_recycle_bin(dest),
        }
    }

    /// Files are dragged over the window (every listener hears every move): when the pointer is over this column's
    /// list, says what a drop there would do, and lights the place.
    fn files_over(&mut self, which: Which, event: &DragMoveEvent<ExternalPaths>, window: &Window, cx: &mut Context<Self>) {
        self.side_mut(which).list_at = event.bounds;
        let found = if event.bounds.contains(&event.event.position) {
            let dragged = drops::paths(event.drag(cx));
            self.drop_target(which, event.event.position, event.bounds, &dragged, window)
        } else {
            None
        };
        if let Some((_, dest)) = &found {
            drops::answer(cx, Answer::Accept(self.drop_words(dest, cx)));
        }
        let place = found.map(|(place, _)| place);
        // Only this column's own light is put out: the other column answers for itself.
        let mine = matches!(&self.drop_over, Some(DropOver::Column(w) | DropOver::Row(w, _)) if *w == which);
        if (place.is_some() || mine) && self.drop_over != place {
            self.drop_over = place;
            cx.notify();
        }
    }

    /// Files were dropped on a column: copied into the folder under the pointer, or moved with Shift (as the drag's
    /// picture said); on the Recycle Bin, recycled.
    fn files_dropped(
        &mut self,
        which: Which,
        dropped: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.drop_over = None;
        cx.notify();
        let moves = match drops::effect(cx) {
            DropEffect::None => return,
            DropEffect::Move => true,
            DropEffect::Copy | DropEffect::Link => false,
        };
        let paths = drops::paths(dropped);
        let list = self.side(which).list_at;
        let Some((_, dest)) = self.drop_target(which, window.mouse_position(), list, &paths, window) else {
            return;
        };
        let pane = self.pane;
        explorer(cx).update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            explorer.drop_files(paths, &dest, moves, cx).detach();
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
        let art_px = (theme::rem_size(cx) * 1.125).round();
        let rtl = i18n::is_rtl();
        range
            .filter_map(|index| {
                let entry = side.entry(index)?;
                let current = which == Which::Parent && same_path(&entry.path, &self.tab_path);
                let under_cursor = has_keyboard && side.cursor == index;
                let lit = self.drop_over == Some(DropOver::Row(which, index));
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
                let glyph = crate::art::kind(glint_model::file_types::kind_of(entry), art_px, cx);
                let (accent, ring, primary, muted) = (t.accent, t.ring, t.primary, t.muted_foreground);
                Some(
                    row_start()
                        .id(id)
                        .role(Role::ListBoxOption)
                        .aria_selected(current)
                        .aria_label(label)
                        .aria_description(SharedString::from(entry.path.clone()))
                        .tooltip(move |window, cx| crate::kit::tooltip_under(tip.clone(), window, cx))
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
                        .when(lit, |row| row.bg(primary.opacity(0.3)).border_color(primary))
                        .when(!entry.dir, |row| row.text_color(muted))
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            let side = this.side_mut(which);
                            side.cursor = index;
                            side.focus.focus(window, cx);
                            this.open_row(which, index, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                this.row_pressed(which, index, event.position)
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                                this.row_menu(which, index, event.position, window, cx)
                            }),
                        )
                        .on_mouse_down(
                            MouseButton::Middle,
                            cx.listener(move |this, _: &MouseDownEvent, _, cx| this.row_in_new_tab(which, index, cx)),
                        )
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            this.rest_on_row(which, index, *hovered, cx)
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
        let (ring, lit) = (theme::tokens(cx).ring, self.drop_over == Some(DropOver::Column(which)));
        // Files dragged over the list, and dropped on it: the folder under the pointer takes them, else the column's.
        let drop_id = match which {
            Which::Parent => "columns-parent-drop",
            Which::Next => "columns-next-drop",
        };
        let list = div()
            .id(drop_id)
            .test_support()
            .size_full()
            .on_drag_move::<ExternalPaths>(cx.listener(move |this, event: &DragMoveEvent<ExternalPaths>, window, cx| {
                this.files_over(which, event, window, cx)
            }))
            .on_drop::<ExternalPaths>(cx.listener(move |this, dropped: &ExternalPaths, window, cx| {
                this.files_dropped(which, dropped, window, cx)
            }))
            .child(list);
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.column_menu(which, event.position, window, cx)
                }),
            )
            .border_1()
            .border_color(if lit { ring } else { gpui_kit::transparent_black() })
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
            .tooltip(move |window, cx| crate::kit::tooltip_under(tip.clone(), window, cx))
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

    fn next_column(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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
                        // The same look as the list's and the grid's message: a picture, one line, centred.
                        column.child(
                            div().flex_1().min_h_0().child(
                                calm::centered(&t, "columns-empty")
                                    .role(Role::Status)
                                    .aria_label(t!("It's empty"))
                                    .test_support()
                                    .child(calm::faded(&t, IconName::Folder))
                                    .child(calm::title(&t, t!("It's empty"))),
                            ),
                        )
                    })
                    .when(!empty, |column| column.child(list))
                    .into_any_element()
            }
            Some(entry) => self.inspector(entry, window, cx),
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

impl ColumnsView {
    /// The last column for a file: its thumbnail, what is known about it (the menu's head says it: type, size, size
    /// in pixels, length, lines, pages, each once it is found out), the badges the menu would show, and the keys.
    fn inspector(&mut self, entry: FileEntry, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let t = theme::tokens(cx).clone();
        let look = self.thumbs.read(cx).look(&entry, THUMB, window.scale_factor());
        let probed = aware::try_shared(cx).and_then(|aware| aware.read(cx).probed(&entry).cloned());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
        // No checksum verdict: a mismatch is said only in the right-click menu's head, never in passing.
        let head = head_of(std::slice::from_ref(&entry), probed.as_ref(), &Extras { now, git: None, checksum: None });
        let badges = head.badges.iter().take(3).enumerate().map(|(ix, badge)| {
            let (fg, bg) = match badge.tone {
                Tone::Plain => (t.foreground, t.accent),
                Tone::Warn => (t.warning, t.warning.opacity(0.13)),
                Tone::Good => (t.success, t.success.opacity(0.13)),
            };
            let pill = row_start()
                .id(("columns-file-badge", ix))
                .role(Role::Label)
                .aria_label(badge.label.clone())
                .test_support()
                .flex_none()
                .h(px(20.))
                .px(space(2.))
                .gap(space(1.))
                .rounded_full()
                .bg(bg)
                .text_color(fg)
                .text_size(theme::size::XXS)
                .font_weight(FontWeight::MEDIUM)
                .child(icon(badge.icon).size(px(11.)).flex_none())
                .child(div().whitespace_nowrap().child(isolate(badge.label.clone())));
            match badge.tip.clone() {
                Some(tip) => pill.tooltip(move |window, cx| crate::kit::tooltip_under(tip.clone(), window, cx)),
                None => pill,
            }
        });
        let picture = thumbnail::thumbnail_square(&look, kind_of(&entry), THUMB, 56., false, cx);
        col()
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
                    .id("columns-file-picture")
                    .role(Role::Image)
                    .aria_label(entry.name.clone())
                    .test_support()
                    .flex()
                    .justify_center()
                    .pb(space(2.))
                    .child(picture),
            )
            .child(
                div()
                    .truncate()
                    .text_size(theme::size::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.foreground)
                    .child(isolate(entry.name.clone())),
            )
            .child(
                div()
                    .id("columns-file-facts")
                    .role(Role::Label)
                    .aria_label(head.line.clone())
                    .test_support()
                    .child(isolate(head.line.clone())),
            )
            .child(
                row_start()
                    .flex_wrap()
                    .gap(space(1.))
                    .py(space(1.))
                    .children(badges)
                    .children(head.changed.clone().map(|changed| div().child(isolate(changed)))),
            )
            .child(div().child(t!("Space shows it full size; Alt+P the preview pane.")))
            .into_any_element()
    }
}

impl Render for ColumnsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.thumbs.update(cx, |thumbs, _| thumbs.take_evicted()) {
            window.drop_image(image).ok();
        }
        self.ask_for_thumbnail(window, cx);
        let parent = self.parent_column(cx);
        let next = self.next_column(window, cx);
        let center = div().h_full().min_w_0().flex_1().children(self.center.clone());
        // Window-level listeners for a press that may become a drag out; they do nothing unless one is under way.
        let (view, tracking) = (cx.entity().downgrade(), self.tracking.clone());
        let taps = gpui_kit::canvas(
            |bounds, _, _| bounds,
            move |_, _, window, _| {
                let (moved, moving) = (view.clone(), tracking.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && moving.get() {
                        moved.update(cx, |this, cx| this.pointer_moved(event, window, cx)).ok();
                    }
                });
                let (released, pressing) = (view.clone(), tracking.clone());
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && pressing.get() && event.button == MouseButton::Left {
                        released.update(cx, |this, _| this.pointer_released()).ok();
                    }
                });
            },
        )
        .absolute()
        .inset_0();
        row_start()
            .id("columns-view")
            .relative()
            .size_full()
            .min_w_0()
            .items_stretch()
            .text_start()
            .child(taps)
            .children(parent)
            .child(center)
            .child(next)
    }
}
