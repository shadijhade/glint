//! The disk usage map: a squarified treemap of what fills a tab's folder, like WizTree or WinDirStat. Folders nest
//! a few levels deep, files are colored by type, age or growth. Hover for details, click to go in, right-click
//! for the menu. Ported from src/components/explorer/DiskUsage.tsx and src/lib/diskUsage.ts.
//!
//! | TypeScript                                  | Here                                                               |
//! |---------------------------------------------|--------------------------------------------------------------------|
//! | `useUsage`, `setUsage`, `toggleUsage`, `mapUsage`, `refreshUsage`, `stopUsage` | [`model::Usage`] (one entity, a [`model::Run`] per tab) |
//! | `DiskUsage`, `UsageMapView`                 | [`DiskUsage`], one per pane                                         |
//! | `paint`, `paintOverlay`, `layoutUsage` memo | [`plan::Plan`] (made off the UI thread) and [`plan::paint`] (a canvas) |
//! | `THEMES`, `AGE_BANDS`, `GROWTH_BANDS`, `tileColor` | [`colors`]                                                   |
//! | `GrowthStrip`, `growthLine`, `markDate`, `Tooltip`, `share` | the strip, the tooltip and their words, in `words.rs` and `view.rs` |
//! | `UsageWork` (the status bar's right side)   | [`status`] and [`StatusLine`]                                       |
//! | `useMapRows`                                | [`model::Shown`]: the map's items as file-list entries              |
//!
//! # How the coordinator wires it
//!
//! **Making a tab show its map, or its list again.** The state is per tab, in [`model::Usage`] (the TypeScript's
//! `useUsage`), reached with plain functions that need only the app:
//!
//! - [`toggle`]`(cx, tab)` is `toggleUsage`: the command bar's "Disk usage" button, the palette's "Disk usage" and
//!   "Back to the file list", the Explorer action list's "Show/Hide disk usage".
//! - [`set`]`(cx, tab, on)` is `setUsage`. [`set`]`(cx, tab, false)` is also what `chooseView` did first: the list
//!   and grid actions (Ctrl+Shift+6, Ctrl+Shift+2), the view switch and the palette's view commands call it before
//!   changing the view.
//! - [`shows`]`(cx, tab)` is `showsUsage`, for the buttons' pressed state and for the workspace: a pane shows its
//!   [`DiskUsage`] instead of the list or the grid when the tab [`shows`] and its folder is a real folder
//!   ([`visible`] says both; This PC, the Recycle Bin and search results keep their own views, as in the web app).
//! - [`ThisPcEvent::DiskUsage { tab, path }`](crate::components::this_pc::ThisPcEvent::DiskUsage) (the "Map disk usage"
//!   button on a drive tile; This PC already opened the drive in `tab`): `if !shows(cx, tab) { toggle(cx, tab) }`.
//!   That is `openDriveTreemap`.
//! - **`MenuEvent::FolderSizes` is not this.** The folder menu's "Folder sizes" is `calcSizes` (measuring the
//!   subfolders for the list's Size column), which belongs to the sizes part. The disk usage map has no entry in
//!   the folder menu; its triggers are the ones above.
//!
//! **Placing it.** One [`DiskUsage::new`]`(pane, window, cx)` per pane, made once like the pane's other views. It
//! shows the map of `pane`'s tab, so the workspace draws it where it draws the list, and gives it the keyboard with
//! [`DiskUsage::focus_handle`]. It makes the shared model itself on first use; calling [`init`] at start-up (after
//! `core_bridge::init` and `explorer::init`) does the same earlier.
//!
//! **What it asks of the window** is the [`DiskUsageEvent`]s: `ItemMenu` (open the item menu for those entries at
//! the position: the pane's list menu needs the entries, since an item deeper than the folder's own is not among
//! the tab's rows), `FolderMenu` (the folder's own menu), `FreeUpSpace` (open Clean up, `dialogs::cleanup::open`)
//! and `EnableInstantSearch` (the status bar's index model `enable`).
//!
//! **Keys.** The view has the `FileList` key context, so Enter (`OpenItem`), the menu key (`ShowItemMenu`),
//! Ctrl+C, Delete and the other file keys reach the workspace as they do from the list, and act on the tab's
//! selection, which clicking an item sets. Backspace goes up one folder and F5 maps again, as in the TypeScript
//! (an interceptor takes them while the map has the keyboard, before `Back` and `Refresh` see them); T goes round the
//! color modes; Escape clears the selection.
//!
//! **The status bar** shows [`status`]`(cx, tab)` on its right while the tab [`shows`] its map (and has no search
//! running): a spinner and "pdu · measuring…" or "index · mapping…" while it works, then a bolt, "index · mapped
//! 12 GB in 3.2 ms · shown in 41 ms", with a longer tooltip. [`StatusLine::element`] draws it the way the bar's other
//! work lines are drawn.
//!
//! **Files dragged in** (from File Explorer or the other pane) drop into the folder mapped, as `data-drop` made
//! them: the map outlines itself, the drag's picture says "Copy to <folder>" (Shift: move), and the drop goes through
//! `Explorer::drop_files`. Nothing is taken in an archive, a closed folder, or for files already in the folder.
//!
//! **Focus.** A click on the map gives it the keyboard and makes its pane the active one
//! (`Explorer::focus_pane`), which emits `PaneFocused`; the workspace's `focus_files` must then hand the keyboard to
//! [`DiskUsage::focus_handle`] for a pane that shows its map, not to the list or grid underneath.
//!
//! # Differences from the TypeScript
//!
//! - The map and its labels mirror in Arabic (the biggest item at the top right, the names start at the right edge).
//! - Keyboard-only use: the arrow keys, Home and End step through the folder's own items (the TypeScript swallowed
//!   them; the map had nothing to move to). Enter opens, the menu key opens the menu.
//! - Dates in the growth strip use the app's date formatter ("Today 9:02 AM") instead of the TypeScript's
//!   "today at 9:02".
//! - While the next map comes the last one stays dimmed at once; the TypeScript waited 150 ms before dimming.
//! - There is no error boundary: a Rust draw cannot throw, so "Couldn't draw this map" has nothing to catch.

pub mod colors;
pub mod model;
pub mod plan;
mod view;
mod words;

#[cfg(test)]
mod tests;

use crate::explorer::{explorer, Explorer, NavOptions, TabId};
use crate::kit::prelude::*;
use crate::platform::dragdrop::{Answer, DropEffect, Under};
use crate::{drops, i18n, t};
use colors::{ColorMode, MapColors};
use glint_model::entries::FileEntry;
use glint_model::format::{format_duration, format_size};
use glint_model::paths::{droppable, is_folder_path, is_recycle_bin, same_path};
use model::{Shown, Usage};
use plan::{LabelCache, Plan, Want};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

pub use model::{init, init_with, shared, try_shared, Source};

// ---- Toggling a tab's map: plain functions for the coordinator --------------------------------------------

/// Whether the tab shows its disk-usage map (`showsUsage`).
pub fn shows(cx: &App, tab: TabId) -> bool {
    try_shared(cx).is_some_and(|usage| usage.read(cx).shows(tab))
}

/// Whether the tab's pane should draw its map: the tab shows it and its folder is a real folder (not This PC, the
/// Recycle Bin, Recent, a remote, or a search's results).
pub fn visible(cx: &App, tab: TabId) -> bool {
    shows(cx, tab)
        && explorer(cx)
            .read(cx)
            .tab(tab)
            .is_some_and(|t| t.find.is_none() && is_folder_path(&t.path))
}

/// Shows the tab's folder as a map (`on`), or as a list again (`setUsage`).
pub fn set(cx: &mut App, tab: TabId, on: bool) {
    let usage = shared(cx);
    usage.update(cx, |usage, cx| usage.set(tab, on, cx));
}

/// Map or list for the tab (`toggleUsage`).
pub fn toggle(cx: &mut App, tab: TabId) {
    let usage = shared(cx);
    usage.update(cx, |usage, cx| usage.toggle(tab, cx));
}

/// Maps the tab's folder again, reading it again first (F5, `refreshUsage`).
pub fn refresh(cx: &mut App, tab: TabId) {
    let usage = shared(cx);
    usage.update(cx, |usage, cx| usage.refresh(tab, cx));
}

// ---- The status bar -------------------------------------------------------------------------------------

/// What the status bar's right side says while a tab shows its map (`UsageWork`).
#[derive(Clone, Debug, PartialEq)]
pub struct StatusLine {
    /// The work is under way: a spinner rather than the bolt.
    pub busy: bool,
    /// "pdu · measuring…", "index · mapped 12 GB in 3.2 ms · shown in 41 ms".
    pub text: SharedString,
    /// Who mapped it and how long it took, for the hover.
    pub tooltip: Option<SharedString>,
}

impl StatusLine {
    /// The line as the status bar draws its work: a spinner or a bolt in the primary color, then the text.
    pub fn element(&self, cx: &App) -> AnyElement {
        let t = crate::theme::tokens(cx);
        let lead = if self.busy {
            Spinner::new().color(t.primary).into_any_element()
        } else {
            icon(IconName::Zap)
                .size_3()
                .text_color(t.primary)
                .flex_none()
                .into_any_element()
        };
        let line = row_start()
            .id("disk-usage-status")
            .role(Role::Status)
            .aria_label(self.text.clone())
            .test_support()
            .items_center()
            .gap(space(1.))
            .child(lead)
            .child(div().child(self.text.clone()));
        match self.tooltip.clone() {
            Some(tip) => line
                .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                .into_any_element(),
            None => line.into_any_element(),
        }
    }
}

/// The status bar's line for the tab's map, or nothing when the tab doesn't show one or has nothing to say yet
/// (`UsageWork`).
pub fn status(cx: &App, tab: TabId) -> Option<StatusLine> {
    let usage = try_shared(cx)?;
    let usage = usage.read(cx);
    let run = usage.run(tab)?;
    let current = run.current();
    if run.running && (run.preview || current.is_none()) {
        let text = if run.engine == "pdu" {
            t!("pdu · measuring…")
        } else {
            t!("{engine} · mapping…", engine = run.engine)
        };
        return Some(StatusLine {
            busy: true,
            text,
            tooltip: None,
        });
    }
    let shown = current?;
    let map = &shown.map;
    let took = format_duration(map.elapsed);
    let mut text = t!(
        "{engine} · mapped {size} in {duration}",
        engine = map.engine,
        size = format_size(map.size.first().copied().unwrap_or(0.) as u64),
        duration = took
    )
    .to_string();
    if run.shown_in > 0. {
        text.push_str(&t!(" · shown in {duration}", duration = format_duration(run.shown_in)));
    }
    let who = match map.engine {
        "index" => t!("The instant search index"),
        "pdu" => SharedString::from("pdu"),
        _ => t!("The archive's own list of entries"),
    };
    let tooltip = t!(
        "{who} mapped this folder in {duration}; it was on screen {shown} after the click.",
        who = who,
        duration = took,
        shown = format_duration(run.shown_in)
    );
    Some(StatusLine {
        busy: false,
        text: text.into(),
        tooltip: Some(tooltip),
    })
}

// ---- The view -------------------------------------------------------------------------------------------

/// What the map asks the window to do.
#[derive(Clone, Debug, PartialEq)]
pub enum DiskUsageEvent {
    /// A right-click (or the menu key) on an item: show the item menu for `entries` (what is selected; the item
    /// pressed alone when it wasn't part of the selection) at `position` in the window.
    ItemMenu {
        entries: Vec<FileEntry>,
        position: Point<Pixels>,
    },
    /// A right-click on the folder mapped itself, where none of its items is: the folder's own menu.
    FolderMenu { folder: String, position: Point<Pixels> },
    /// "Free up space": Clean up for the folder mapped (`openCleanup`).
    FreeUpSpace { folder: String },
    /// "Update instant search" or "Turn on instant search" (`enableIndex`; Windows asks for administrator approval).
    EnableInstantSearch,
}

impl EventEmitter<DiskUsageEvent> for DiskUsage {}

/// A map and a plan made from it: what is on screen. A newer map keeps this one drawn until its own plan is
/// ready, so the map never blanks.
struct Drawn {
    plan: Arc<Plan>,
    shown: Arc<Shown>,
}

/// What is under the pointer.
#[derive(Clone, Copy, Debug)]
struct Hover {
    node: usize,
    rest: bool,
}

/// The disk usage map of a pane's tab. See the module docs for how the window uses it.
pub struct DiskUsage {
    pane: usize,
    explorer: Entity<Explorer>,
    usage: Entity<Usage>,
    focus: FocusHandle,
    mode: ColorMode,
    /// The legend entry clicked (stays lit) and the one the pointer is on (`pinnedSpot`, `hoverSpot`).
    pinned_spot: Option<SharedString>,
    hover_spot: Option<SharedString>,
    /// The mode and folder the spots belong to: they reset when either changes.
    spot_for: Option<(ColorMode, String)>,
    drawn: Option<Drawn>,
    /// What a background job is making a plan for.
    building: Option<Want>,
    /// When the map on screen arrived (Unix ms): ages count from it.
    arrived: Option<(u64, f64)>,
    /// Where the map is on screen, measured by a canvas while painting.
    body: Rc<Cell<Bounds<Pixels>>>,
    /// The pointer over the map, in window pixels.
    pointer: Option<Point<Pixels>>,
    /// The file a click selected, which a double-click may then open (`picked`).
    picked: Option<String>,
    /// Files dragged in could be dropped on the map (into the folder mapped): it is outlined.
    drop_over: bool,
    labels: Rc<RefCell<LabelCache>>,
    /// Redraws once a second-ish while pdu measures, so the seconds tick.
    ticker: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for DiskUsage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl DiskUsage {
    /// The map for `pane`'s tab. It follows the pane's tab and the shared model; it draws nothing until the
    /// tab shows its map ([`set`]).
    pub fn new(pane: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer(cx);
        let usage = shared(cx);
        let focus = cx.focus_handle().tab_stop(true);
        let mut subscriptions = vec![
            cx.observe(&explorer, |_, _, cx| cx.notify()),
            cx.observe(&usage, |_, _, cx| cx.notify()),
        ];
        // Keys only a map has: Backspace goes back out, as Alt+Up does; F5 maps again. They are taken before the
        // key bindings (`Back`, `Refresh`) see them, and only while the map has the keyboard.
        let this = cx.weak_entity();
        let intercept = cx.intercept_keystrokes(move |event, window, cx| {
            let modifiers = event.keystroke.modifiers;
            if modifiers.control || modifiers.alt || modifiers.shift || modifiers.platform {
                return;
            }
            let key = event.keystroke.key.as_str();
            if key != "backspace" && key != "f5" {
                return;
            }
            let wanted = this
                .upgrade()
                .is_some_and(|view| view.read(cx).focus.is_focused(window) && view.read(cx).showing(cx));
            if wanted {
                cx.stop_propagation();
                let go_up = key == "backspace";
                this.update(cx, |this, cx| if go_up { this.go_up(cx) } else { this.refresh(cx) })
                    .ok();
            }
        });
        subscriptions.push(intercept);
        let _ = window;
        Self {
            pane,
            explorer,
            usage,
            focus,
            mode: ColorMode::Type,
            pinned_spot: None,
            hover_spot: None,
            spot_for: None,
            drawn: None,
            building: None,
            arrived: None,
            body: Rc::new(Cell::new(Bounds::default())),
            pointer: None,
            picked: None,
            drop_over: false,
            labels: Rc::new(RefCell::new(LabelCache::default())),
            ticker: None,
            _subscriptions: subscriptions,
        }
    }

    /// The handle the window gives the keyboard to.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The pane this shows.
    pub fn pane(&self) -> usize {
        self.pane
    }

    /// The color mode in use (T goes round them).
    #[cfg(test)]
    pub fn color_mode(&self) -> ColorMode {
        self.mode
    }

    /// The tab this pane shows now.
    fn tab(&self, cx: &App) -> TabId {
        self.explorer.read(cx).pane_tab_id(self.pane)
    }

    /// Whether the pane's tab shows its map.
    fn showing(&self, cx: &App) -> bool {
        visible(cx, self.tab(cx))
    }

    /// The map on screen, when it is the folder mapped now (not an earlier one shown meanwhile).
    fn live(&self, cx: &App) -> Option<&Drawn> {
        let drawn = self.drawn.as_ref()?;
        let usage = self.usage.read(cx);
        let run = usage.run(self.tab(cx))?;
        same_path(&drawn.shown.map.path, &run.path).then_some(drawn)
    }

    /// What is under `position` (window pixels), if the map on screen is live.
    fn hit_at(&self, position: Point<Pixels>, cx: &App) -> Option<Hover> {
        let drawn = self.live(cx)?;
        let local = position - self.body.get().origin;
        let hit = drawn.plan.hit(&drawn.shown, f32::from(local.x), f32::from(local.y))?;
        Some(Hover {
            node: hit.node,
            rest: hit.rest,
        })
    }

    /// The pane has the keyboard and the explorer's active pane follows: a click on this map makes this pane
    /// the active one.
    fn take_pane(&self, cx: &mut Context<Self>) {
        let pane = self.pane;
        if self.explorer.read(cx).active_pane() != pane {
            self.explorer.update(cx, |e, cx| e.focus_pane(pane, cx));
        }
    }

    // ---- What the keys do ------------------------------------------------------------------------------------

    /// Backspace: up one folder, as Alt+Up does (`goUp`).
    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        self.take_pane(cx);
        self.explorer.update(cx, |e, cx| e.go_up(cx).detach());
    }

    /// F5: reads the folder again and maps it again (`refreshUsage`).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let tab = self.tab(cx);
        self.usage.update(cx, |usage, cx| usage.refresh(tab, cx));
    }

    /// T: the next way to color the tiles.
    pub fn next_mode(&mut self, cx: &mut Context<Self>) {
        self.set_mode(self.mode.next(), cx);
    }

    /// Colors the tiles by `mode`.
    pub fn set_mode(&mut self, mode: ColorMode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }

    /// Enter: opens what is selected.
    fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(drawn) = self.drawn.as_ref() else { return };
        let tab = self.tab(cx);
        let entries: Vec<FileEntry> = self
            .explorer
            .read(cx)
            .tab(tab)
            .map(|t| {
                t.selection
                    .iter()
                    .filter_map(|p| drawn.shown.node_of(p).and_then(|n| drawn.shown.entry(n)).cloned())
                    .collect()
            })
            .unwrap_or_default();
        self.take_pane(cx);
        for entry in entries {
            self.explorer.update(cx, |e, cx| e.open_entry(&entry, cx).detach());
        }
    }

    /// The items of the folder mapped, biggest first: what the arrow keys step through.
    fn top_items(&self) -> Vec<usize> {
        let Some(drawn) = self.drawn.as_ref() else {
            return Vec::new();
        };
        let map = &drawn.shown.map;
        let first = map.first.first().map_or(0, |&v| v as usize).max(1);
        let end = map.end.first().map_or(0, |&v| v as usize).min(map.count);
        (first..end).collect()
    }

    /// An arrow key, Home or End: selects another of the folder's own items. Left and Up go to the previous one,
    /// Right and Down to the next (the other way round in Arabic for Left and Right).
    fn step(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let items = self.top_items();
        let (Some(drawn), false) = (self.drawn.as_ref(), items.is_empty()) else {
            return false;
        };
        let rtl = i18n::is_rtl();
        let tab = self.tab(cx);
        let at = self
            .explorer
            .read(cx)
            .tab(tab)
            .and_then(|t| t.selection.focus().map(str::to_string))
            .and_then(|p| drawn.shown.node_of(&p))
            .and_then(|n| items.iter().position(|&i| i == n));
        let last = items.len() - 1;
        let to = match (key, rtl) {
            ("home", _) => 0,
            ("end", _) => last,
            ("up", _) | ("left", false) | ("right", true) => at.map_or(last, |a| a.saturating_sub(1)),
            ("down", _) | ("right", false) | ("left", true) => at.map_or(0, |a| (a + 1).min(last)),
            _ => return false,
        };
        let Some(path) = drawn.shown.map.paths.get(items[to]).cloned() else {
            return false;
        };
        self.take_pane(cx);
        self.explorer.update(cx, |e, cx| e.select_only(Some(&path), cx));
        true
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = keystroke.modifiers;
        let plain = !m.control && !m.alt && !m.platform;
        let key = keystroke.key.as_str();
        if plain && !m.shift && key == "t" {
            cx.stop_propagation();
            self.next_mode(cx);
        } else if plain && !m.shift && self.step(key, cx) {
            cx.stop_propagation();
        } else if plain && key == "escape" {
            let tab = self.tab(cx);
            if self.explorer.read(cx).tab(tab).is_some_and(|t| !t.selection.is_empty()) {
                cx.stop_propagation();
                self.take_pane(cx);
                self.explorer.update(cx, |e, cx| e.select_only(None, cx));
            }
        }
    }

    /// The menu key (or Shift+F10): the menu of what is selected at its corner, else the folder's.
    fn keyboard_menu(&mut self, cx: &mut Context<Self>) {
        let Some(drawn) = self.drawn.as_ref() else { return };
        let tab = self.tab(cx);
        let body = self.body.get();
        let selected: Vec<(usize, FileEntry)> = self
            .explorer
            .read(cx)
            .tab(tab)
            .map(|t| {
                t.selection
                    .iter()
                    .filter_map(|p| {
                        let n = drawn.shown.node_of(p)?;
                        Some((n, drawn.shown.entry(n)?.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        match selected.first() {
            Some(&(node, _)) => {
                let (x, y, w, h) = drawn.plan.rect(node);
                // The bottom start corner of the item.
                let corner_x = if drawn.plan.rtl { x + w } else { x };
                let position = body.origin + point(px(corner_x), px(y + h));
                cx.emit(DiskUsageEvent::ItemMenu {
                    entries: selected.into_iter().map(|(_, e)| e).collect(),
                    position,
                });
            }
            None => {
                let folder = drawn.shown.map.path.clone();
                cx.emit(DiskUsageEvent::FolderMenu {
                    folder,
                    position: body.origin,
                });
            }
        }
    }

    // ---- What the pointer does -------------------------------------------------------------------------------

    /// Click: a folder (or its smaller items) is mapped; a file is selected, in its own folder if it's deeper.
    /// A second click on a file the first one selected opens it (`onClick`, `onDoubleClick`).
    fn clicked(&mut self, position: Point<Pixels>, count: usize, cx: &mut Context<Self>) {
        let Some(hover) = self.hit_at(position, cx) else { return };
        let Some(drawn) = self.live(cx) else { return };
        let shown = drawn.shown.clone();
        let map = &shown.map;
        let tab = self.tab(cx);
        self.picked = None;
        self.take_pane(cx);
        let node = hover.node;
        if node == 0 {
            self.explorer.update(cx, |e, cx| e.select_only(None, cx));
            return;
        }
        let Some(path) = map.paths.get(node).cloned() else {
            return;
        };
        if hover.rest || glint_model::usage::is_folder(map, node) {
            self.explorer
                .update(cx, |e, cx| e.navigate(path, NavOptions::in_tab(tab), cx));
        } else {
            let folder = map.parent.get(node).copied().unwrap_or(0) as usize;
            if folder == 0 {
                self.picked = Some(path.clone());
                self.explorer.update(cx, |e, cx| e.select_only(Some(&path), cx));
            } else if let Some(folder_path) = map.paths.get(folder).cloned() {
                self.explorer.update(cx, |e, cx| {
                    e.navigate(folder_path, NavOptions::in_tab(tab).selecting(path.clone()), cx)
                });
            }
        }
        // Double-click opens a file the first click selected (not one that a click took you to).
        if count >= 2 && !hover.rest && self.picked.as_deref() == map.paths.get(node).map(String::as_str) {
            if let Some(entry) = shown.entry(node).cloned() {
                self.explorer.update(cx, |e, cx| e.open_entry(&entry, cx).detach());
            }
        }
    }

    /// Right-click: the folder mapped has the folder's menu; any other item is selected (unless it is already)
    /// and gets the item menu (`onContextMenu`).
    fn context_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(hover) = self.hit_at(position, cx) else { return };
        let Some(drawn) = self.live(cx) else { return };
        let shown = drawn.shown.clone();
        let tab = self.tab(cx);
        self.take_pane(cx);
        if hover.node == 0 {
            cx.emit(DiskUsageEvent::FolderMenu {
                folder: shown.map.path.clone(),
                position,
            });
            return;
        }
        let Some(path) = shown.map.paths.get(hover.node).cloned() else {
            return;
        };
        let selected = self
            .explorer
            .read(cx)
            .tab(tab)
            .is_some_and(|t| t.selection.contains(&path));
        if !selected {
            self.explorer.update(cx, |e, cx| e.select_only(Some(&path), cx));
        }
        let entries: Vec<FileEntry> = self
            .explorer
            .read(cx)
            .tab(tab)
            .map(|t| {
                t.selection
                    .iter()
                    .filter_map(|p| shown.node_of(p).and_then(|n| shown.entry(n)).cloned())
                    .collect()
            })
            .unwrap_or_default();
        cx.emit(DiskUsageEvent::ItemMenu { entries, position });
    }

    /// The pointer moved over the map.
    fn pointer_moved(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.pointer != Some(position) {
            self.pointer = Some(position);
            cx.notify();
        }
    }

    /// The pointer left the map.
    fn pointer_left(&mut self, cx: &mut Context<Self>) {
        if self.pointer.take().is_some() {
            cx.notify();
        }
    }

    // ---- Files dropped on the map -------------------------------------------------------------------------

    /// What a drop of `dragged` on the map would go into: the folder mapped (`data-drop={run.path}`). `None`
    /// where nothing can be dropped (an archive is read-only, a folder Windows keeps closed takes nothing) or
    /// where it would do nothing (they are in it already).
    fn drop_target(&self, dragged: &[String], cx: &App) -> Option<String> {
        let explorer = self.explorer.read(cx);
        let tab = explorer.pane_tab(self.pane)?;
        if tab.is_this_pc() || tab.find.is_some() || tab.archive().is_some() || tab.is_closed() {
            return None;
        }
        let dest = tab.path.clone();
        let nothing = !dragged.is_empty() && !is_recycle_bin(&dest) && droppable(dragged, &dest).is_empty();
        (!nothing).then_some(dest)
    }

    /// The words on the drag's picture for a drop into `dest`.
    fn drop_words(&self, dest: &str, cx: &App) -> Under {
        let name = self.explorer.read(cx).tab_name(dest);
        Under {
            name: glint_model::i18n::translate(i18n::language(), &name).to_string(),
            copy: t!("Copy to %1").to_string(),
            moves: t!("Move to %1").to_string(),
            always_move: is_recycle_bin(dest),
        }
    }

    /// Files dragged over the window moved (every listener hears every move): when the pointer is over the map,
    /// answer what a drop would do and outline the map.
    fn files_over(&mut self, event: &DragMoveEvent<ExternalPaths>, cx: &mut Context<Self>) {
        let dragged = event
            .dragged_item()
            .downcast_ref::<ExternalPaths>()
            .map(drops::paths)
            .unwrap_or_default();
        let target = if event.bounds.contains(&event.event.position) && self.live(cx).is_some() {
            self.drop_target(&dragged, cx)
        } else {
            None
        };
        if let Some(dest) = &target {
            drops::answer(cx, Answer::Accept(self.drop_words(dest, cx)));
        }
        let over = target.is_some();
        if self.drop_over != over {
            self.drop_over = over;
            cx.notify();
        }
    }

    /// Files were dropped on the map: copied into the folder mapped, or moved when the drop moves (Shift). A
    /// link copies, as it did on the web (`dropFiles`).
    fn files_dropped(&mut self, drop: &ExternalPaths, cx: &mut Context<Self>) {
        self.drop_over = false;
        cx.notify();
        let paths = drops::paths(drop);
        let Some(dest) = self.drop_target(&paths, cx) else {
            return;
        };
        let moves = drops::effect(cx) == DropEffect::Move;
        self.explorer
            .update(cx, |explorer, cx| explorer.drop_files(paths, &dest, moves, cx).detach());
    }

    // ---- The plan ------------------------------------------------------------------------------------------

    /// Makes sure the plan for the map and size now is being made or is there (`useMemo` of the layout). Cheap
    /// when nothing changed. The work runs on a background thread; the old plan stays on screen meanwhile.
    fn ensure_plan(&mut self, shown: &Arc<Shown>, cx: &mut Context<Self>) {
        let size = self.body.get().size;
        let (width, height) = (f32::from(size.width), f32::from(size.height));
        if width <= 0. || height <= 0. {
            return;
        }
        let now_ms = match self.arrived {
            Some((id, now)) if id == shown.id => now,
            _ => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0., |d| d.as_secs_f64() * 1000.);
                self.arrived = Some((shown.id, now));
                now
            }
        };
        let want = Want {
            shown: shown.clone(),
            width,
            height,
            mode: self.mode,
            colors: MapColors::new(crate::theme::tokens(cx)),
            now_ms,
            rtl: i18n::is_rtl(),
        };
        if self.drawn.as_ref().is_some_and(|d| want.is_met_by(&d.plan)) {
            return;
        }
        if self.building.as_ref().is_some_and(|b| b.same_as(&want)) {
            return;
        }
        self.building = Some(want.clone());
        cx.spawn(async move |this, cx| {
            let for_plan = want.clone();
            let plan = cx
                .background_executor()
                .spawn(async move { Plan::build(&for_plan) })
                .await;
            this.update(cx, |this, cx| this.plan_ready(want, plan, cx)).ok();
        })
        .detach();
    }

    /// A plan came back. It's used unless something newer is wanted meanwhile.
    fn plan_ready(&mut self, want: Want, plan: Plan, cx: &mut Context<Self>) {
        let wanted_still = self.building.as_ref().is_some_and(|b| b.same_as(&want));
        if !wanted_still {
            return;
        }
        self.building = None;
        let tab = self.tab(cx);
        let run = self
            .usage
            .read(cx)
            .run(tab)
            .map(|r| (r.id, r.current().is_some_and(|s| s.id == want.shown.id)));
        self.drawn = Some(Drawn {
            plan: Arc::new(plan),
            shown: want.shown.clone(),
        });
        // Timed when this folder's own map is drawn (not a part of an earlier one shown meanwhile).
        if let Some((id, true)) = run {
            self.usage.update(cx, |usage, cx| usage.mark_shown(tab, id, cx));
        }
        cx.notify();
    }
}
