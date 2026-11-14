//! The folders used most, above Recent, as File Explorer's Home has them (FrequentFolders.tsx).
//!
//! | TypeScript                               | Here                                                              |
//! |------------------------------------------|-------------------------------------------------------------------|
//! | `MOST`, the `useMemo` that ranks `visits` | [`MOST`] and [`frequent`] (pure, tested)                          |
//! | `FrequentFolders`                        | [`FrequentFolders`], one per pane                                 |
//! | `onContextMenu(e, path)` prop            | [`FrequentFoldersEvent::ContextMenu`]                             |
//! | `navigate(path)`, `middleClickOpens`     | the explorer, in the pane's tab; a middle click opens a new tab   |
//! | `data-drop={path}`                       | each tile takes dropped files (copy, Shift moves)                 |
//!
//! # How the coordinator wires it
//!
//! - **Make it once per pane**, like the pane's other views: `cx.new(|cx| FrequentFolders::new(pane, window, cx))`.
//! - **Place it above the list or grid**, as `Explorer.tsx` did (`recentHere && <FrequentFolders/>`): where the pane's
//!   tab shows Recent and has no search. [`shown_for`]`(&tab)` says exactly that, and [`FrequentFolders::is_empty`]
//!   says there is nothing to draw (the view then draws nothing at all, as the TypeScript's `return null`, so it
//!   may be placed whenever `shown_for` is true). It is a `shrink-0` block; the list below takes the rest.
//! - **Subscribe** to [`FrequentFoldersEvent::ContextMenu`] `{ path, position }` and open the shared place menu
//!   for the folder at the window position (`onPlaceContextMenu`, the one This PC's tiles use).
//!
//! The visits come from the explorer's memory (`Explorer::memory().visits()`); the view rebuilds whenever the explorer
//! changes (the ranking is cheap and an unchanged list redraws nothing).
//!
//! # Differences from the TypeScript
//!
//! - Keyboard: the tiles are tab stops; the arrows (reading direction for left and right), Home and End move
//!   between them and Enter or Space opens. The TypeScript's were plain buttons, reached with Tab.
//! - The grid wraps by the tile's minimum width (11 rem) as the CSS grid's `auto-fill` did, but the tiles of the
//!   last row do not stretch as wide as the ones above (flex wrapping, not a grid).

#[cfg(test)]
mod tests;

use crate::components::tab_strip::drop_words;
use crate::explorer::{explorer, NavOptions, Tab};
use crate::kit::prelude::*;
use crate::platform::dragdrop::{Answer, DropEffect};
use crate::{drops, theme};
use crate::{i18n, t};
use glint_model::paths::{
    base_name, droppable, in_archive_path, is_folder_path, is_recent, is_remote_path, parent_path,
};
use glint_model::rank::{frecency, Visit};
use glint_model::store::OrderedMap;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

/// The most folders shown.
pub const MOST: usize = 8;

/// What the view asks the window to do.
#[derive(Clone, Debug, PartialEq)]
pub enum FrequentFoldersEvent {
    /// A right click on a folder: the window's place menu for `path`, at `position` in the window.
    ContextMenu { path: String, position: Point<Pixels> },
}

impl EventEmitter<FrequentFoldersEvent> for FrequentFolders {}

/// Whether a tab gets the frequent folders above its files: it shows Recent, and no search runs there
/// (`recentHere = isRecent(path) && !find`).
pub fn shown_for(tab: &Tab) -> bool {
    is_recent(&tab.path) && tab.find.is_none()
}

/// The folders opened most, and most lately: the folders among `visits`, best frecency first (ties keep the
/// order the visits are kept in, as the stable sort of the TypeScript did), at most [`MOST`]. `now` is Unix
/// milliseconds.
pub fn frequent(visits: &OrderedMap<Visit>, now: i64) -> Vec<String> {
    let mut ranked: Vec<(&str, f64)> = visits
        .iter()
        .filter(|(path, _)| is_folder_path(path))
        .map(|(path, visit)| (path, frecency(visit, now)))
        .collect();
    // `sort_by` is stable. A NaN can't come out of frecency (a count times a constant), so `Equal` is only a
    // fallback that keeps the sort total.
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked
        .into_iter()
        .take(MOST)
        .map(|(path, _)| path.to_string())
        .collect()
}

/// A tile's text, made when the visits change and not per frame.
struct Tile {
    path: String,
    /// The folder's name, or the whole path for a root ("C:\").
    name: SharedString,
    /// The folder it is in, or the path again for a root.
    parent: SharedString,
    focus: FocusHandle,
}

/// The folders used most, in a grid of tiles. One per pane.
pub struct FrequentFolders {
    pane: usize,
    tiles: Vec<Tile>,
    /// The tile (by index) files dragged over would drop into, while the drop is taken; the tiles light from it.
    drop_over: Rc<Cell<Option<usize>>>,
    /// The visits ranked last ([`crate::explorer::Memory::visits_version`]): the explorer changes on every key
    /// press, its visits once per folder opened.
    ranked: Option<u64>,
    _subscriptions: Vec<Subscription>,
}

impl FrequentFolders {
    /// The frequent folders for `pane`: a click opens a folder in that pane's tab.
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        // Observing (not only `MemoryChanged`): `set_memory` hands over the saved visits without an event.
        let subscriptions = vec![cx.observe(&explorer, |this, _, cx| this.rebuild(cx))];
        let mut this = FrequentFolders {
            pane,
            tiles: Vec::new(),
            drop_over: Rc::new(Cell::new(None)),
            ranked: None,
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this
    }

    /// There are no folders to show (the view then draws nothing).
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// The paths of the folders shown, best first.
    pub fn paths(&self) -> Vec<String> {
        self.tiles.iter().map(|tile| tile.path.clone()).collect()
    }

    /// Ranks the visits again. Tiles keep their focus handles by position, so the keyboard stays put.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let version = explorer(cx).read(cx).memory().visits_version();
        if self.ranked == Some(version) {
            return;
        }
        self.ranked = Some(version);
        let now = now_ms();
        let folders = explorer(cx).read_with(cx, |explorer, _| frequent(explorer.memory().visits(), now));
        if folders == self.paths() {
            return;
        }
        let old: Vec<FocusHandle> = self.tiles.iter().map(|tile| tile.focus.clone()).collect();
        self.tiles = folders
            .into_iter()
            .enumerate()
            .map(|(i, path)| {
                let name = match base_name(&path) {
                    "" => path.clone(),
                    name => name.to_string(),
                };
                let parent = match parent_path(&path) {
                    parent if parent.is_empty() => path.clone(),
                    parent => parent,
                };
                Tile {
                    name: name.into(),
                    parent: parent.into(),
                    path,
                    focus: old.get(i).cloned().unwrap_or_else(|| cx.focus_handle().tab_stop(true)),
                }
            })
            .collect();
        self.drop_over.set(None);
        cx.notify();
    }

    /// Opens a folder in this pane's tab (`navigate(path)`).
    fn open(&self, path: &str, cx: &mut Context<Self>) {
        let pane = self.pane;
        explorer(cx).update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            let tab = explorer.pane_tab_id(pane);
            explorer.navigate(path, NavOptions::in_tab(tab), cx);
        });
    }

    /// A middle click: the folder opens in a new tab beside this pane's (`middleClickOpens`).
    fn open_in_new_tab(&self, path: &str, cx: &mut Context<Self>) {
        let pane = self.pane;
        explorer(cx).update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            explorer.open_in_new_tab(path, cx);
        });
    }

    /// Whether a drop of `paths` on the folder is taken: not into a read-only place (a remote connection, an
    /// archive), and not where every file already is.
    fn drop_accepted(paths: &[String], dest: &str) -> bool {
        !is_remote_path(dest) && !in_archive_path(dest) && !droppable(paths, dest).is_empty()
    }

    /// Files dragged over tile `index`: says what a drop would do, and lights the tile while it is taken.
    fn drag_over_tile(&mut self, index: usize, event: &DragMoveEvent<ExternalPaths>, cx: &mut Context<Self>) {
        let Some(tile) = self.tiles.get(index) else {
            return;
        };
        if !event.bounds.contains(&event.event.position) {
            self.leave_tile(index, cx);
            return;
        }
        let paths = drops::paths(event.drag(cx));
        if Self::drop_accepted(&paths, &tile.path) {
            drops::answer(cx, Answer::Accept(drop_words(&tile.name)));
            if self.drop_over.get() != Some(index) {
                self.drop_over.set(Some(index));
                cx.notify();
            }
        } else {
            drops::answer(cx, Answer::Refuse);
            self.leave_tile(index, cx);
        }
    }

    fn leave_tile(&self, index: usize, cx: &mut Context<Self>) {
        if self.drop_over.get() == Some(index) {
            self.drop_over.set(None);
            cx.notify();
        }
    }

    /// Files dropped on a tile go into its folder: copied, or moved when Shift was held (`dropFiles`).
    fn drop_on_tile(&mut self, index: usize, dropped: &ExternalPaths, cx: &mut Context<Self>) {
        self.drop_over.set(None);
        cx.notify();
        let Some(dest) = self.tiles.get(index).map(|tile| tile.path.clone()) else {
            return;
        };
        let paths = drops::paths(dropped);
        if !Self::drop_accepted(&paths, &dest) {
            return;
        }
        let moves = drops::effect(cx) == DropEffect::Move;
        explorer(cx).update(cx, |explorer, cx| explorer.drop_files(paths, &dest, moves, cx).detach());
    }

    /// Keys on the tiles: the arrows move (left and right along the reading direction), Home and End jump, Enter
    /// and Space open. Keys with a modifier are the shortcuts' and pass by.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let Some(current) = self.tiles.iter().position(|tile| tile.focus.is_focused(window)) else {
            return;
        };
        let last = self.tiles.len().saturating_sub(1);
        let rtl = i18n::is_rtl();
        let target = match event.keystroke.key.as_str() {
            "right" if rtl => current.saturating_sub(1),
            "left" if rtl => (current + 1).min(last),
            "right" | "down" => (current + 1).min(last),
            "left" | "up" => current.saturating_sub(1),
            "home" => 0,
            "end" => last,
            "enter" | "space" => {
                if let Some(path) = self.tiles.get(current).map(|tile| tile.path.clone()) {
                    self.open(&path, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if let Some(tile) = self.tiles.get(target) {
            tile.focus.focus(window, cx);
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn tile(&self, index: usize, tile: &Tile, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let lit = self.drop_over.clone();
        let folder = theme::tailwind("amber-500").unwrap_or(t.warning);
        let (open_path, middle_path, menu_path, tip) = (
            tile.path.clone(),
            tile.path.clone(),
            tile.path.clone(),
            SharedString::from(tile.path.clone()),
        );
        div()
            .id(("frequent-folder", index))
            .role(Role::Button)
            .aria_label(tile.name.clone())
            .aria_description(SharedString::from(tile.path.clone()))
            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
            .test_support()
            .track_focus(&tile.focus)
            .focus(|style| style.border_color(t.ring))
            .flex()
            .items_center()
            .gap(space(2.))
            .min_w(rems(11.))
            .flex_basis(rems(11.))
            .flex_grow(1.)
            .flex_shrink(1.)
            .px(space(2.))
            .py(space(1.5))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .cursor_pointer()
            .hover(|style| style.bg(t.accent))
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                if lit.get() == Some(index) {
                    style.border_color(t.primary).bg(t.primary.opacity(0.15))
                } else {
                    style
                }
            })
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                this.drag_over_tile(index, event, cx)
            }))
            .on_drop(cx.listener(move |this, dropped: &ExternalPaths, _, cx| this.drop_on_tile(index, dropped, cx)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open(&open_path, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.open_in_new_tab(&middle_path, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                    cx.emit(FrequentFoldersEvent::ContextMenu {
                        path: menu_path.clone(),
                        position: event.position,
                    })
                }),
            )
            .child(icon(IconName::Folder).size(rems(1.25)).text_color(folder).flex_none())
            .child(
                col()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::SM)
                            .child(isolate(tile.name.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(isolate(tile.parent.clone())),
                    ),
            )
    }
}

/// Now, in Unix milliseconds.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

impl Render for FrequentFolders {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tiles.is_empty() {
            // `return null`: nothing at all.
            return div().into_any_element();
        }
        let t = theme::tokens(cx).clone();
        let tiles: Vec<AnyElement> = self
            .tiles
            .iter()
            .enumerate()
            .map(|(i, tile)| self.tile(i, tile, cx).into_any_element())
            .collect();
        div()
            .id("frequent-folders")
            .role(Role::Group)
            .aria_label(t!("Frequent folders"))
            .test_support()
            .flex_none()
            .w_full()
            .border_b_1()
            .border_color(t.border)
            .px(space(3.))
            .py(space(2.5))
            .text_start()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
            .child(
                div()
                    .mb(space(1.5))
                    .text_size(theme::size::XS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.muted_foreground)
                    .child(t!("Frequent folders")),
            )
            .child(row_start().flex_wrap().gap(space(1.)).children(tiles))
            .into_any_element()
    }
}
