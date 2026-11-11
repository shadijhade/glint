//! The tabs in the title bar. Ported from src/components/explorer/TabStrip.tsx and the tab handling in
//! Explorer.tsx (`tabMenu`, `paneMenu`).
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                  | Here                                                        |
//! |---------------------------------------------|-------------------------------------------------------------|
//! | `TabStrip` (one per pane)                   | [`TabStrip`]: one entity drawing a strip for each shown pane |
//! | `Tab` (icon, name, busy pulse, close)       | `TabStrip::tab`                                             |
//! | `dragTab` / `dropAt` / `placeTab`           | GPUI's `on_drag` with [`DraggedTab`]; `slot_at`             |
//! | `PaneDropZone` over the files               | [`PaneDropZone`]                                            |
//! | `tearOffTab`                                | `TabStrip::released`, `TabStrip::tear_off`                  |
//! | `tabMenu(id)`                               | `tab_menu` (in-app menu on a right-click)                   |
//! | `paneMenu(pane)` and the pane button        | `pane_menu` (opened by the pane button)                     |
//!
//! The strip keeps no copy of what the explorer holds: it reads the tabs and panes, acts through the
//! explorer's methods and redraws when the explorer changes. Its own state is the scroll position of each
//! strip, the slot a dragged tab would drop into, the open pane menu and the keyboard focus of the tabs.
//!
//! **A tab dragged out of the window** (`tearOffTab`) opens its folder in a window of its own where it was
//! dropped, and closes, unless it is the last tab. The drop is "outside" when the pointer is released beyond the
//! window's edge (GPUI keeps the mouse captured while a button is down, so the release still arrives), or well
//! away (90 px) from where the drag began, over nothing that took the tab. Nothing takes the tab but the strips
//! and the [`PaneDropZone`]s, which say so with `drop_taken`.
//!
//! **The title bar's drag area.** The strip's empty space takes no mouse (it has no handlers that block it), so
//! the window can still be moved by it. Tabs and buttons `occlude` the mouse, so pressing one never starts a
//! window drag.

use crate::components::command_bar::keyed;
use crate::explorer::{self, Drive, DriveKind, Explorer, Pane, Tab, TabId};
use crate::kit::prelude::*;
use crate::platform::dragdrop::{Answer, DropEffect, Under};
use crate::platform::secondary;
use crate::theme::Tokens;
use crate::{i18n, state, t, theme};
use glint_model::paths::{
    in_archive_path, is_gallery, is_recent, is_recycle_bin, is_remote_path, is_root, is_shadow_path, same_path, THIS_PC,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;

/// How long files held over a tab that isn't showing wait before it opens (`springTimer`, 450 ms).
const SPRING_DELAY: Duration = Duration::from_millis(450);
/// A tab's height: 30 px at the usual size, a pill centred in the title bar.
const TAB_HEIGHT: Rems = rems(1.875);

/// How far from where a tab's drag began a release counts as pulling it out of the strip (`> 90` in `dragTab`).
const PULL_AWAY: Pixels = px(90.);

thread_local! {
    /// Set by whatever took the tab that was just dropped (a strip, a pane's drop zone); read after the drop
    /// to tell a tab dropped on nothing.
    static DROP_TAKEN: Cell<bool> = const { Cell::new(false) };
}

/// Says that the dropped tab was taken, so it isn't torn off into a window.
fn drop_taken() {
    DROP_TAKEN.with(|taken| taken.set(true));
}

/// What a tab released at some place does (`dragTab`'s end).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TearOff {
    /// Nothing: it was dropped in the strip or near it.
    No,
    /// A window of its own, at once: past the window's edge no strip or zone can take it.
    Now,
    /// A window of its own, unless the drop that follows is taken by a strip or a pane's zone.
    IfNotTaken,
}

/// Whether a tab released inside the window (or not) and `away` from where its drag began is torn off.
fn tear_off_for(inside: bool, away: Pixels) -> TearOff {
    if !inside {
        TearOff::Now
    } else if away > PULL_AWAY {
        TearOff::IfNotTaken
    } else {
        TearOff::No
    }
}

// ---- What is drawn ----------------------------------------------------------------------------------

/// The icon a tab gets (`Icon` in TabStrip.tsx).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    Search,
    ThisPc,
    RecycleBin,
    Recent,
    Gallery,
    Cloud,
    Folder,
    Drive(DriveKind),
}

impl Glyph {
    fn icon(self) -> IconName {
        match self {
            Glyph::Search => IconName::Search,
            Glyph::ThisPc => IconName::Monitor,
            Glyph::RecycleBin => IconName::Trash,
            Glyph::Recent => IconName::Clock,
            Glyph::Gallery => IconName::Images,
            Glyph::Cloud => IconName::Cloud,
            Glyph::Folder => IconName::Folder,
            Glyph::Drive(kind) => drive_icon(kind),
        }
    }
}

/// A tab as drawn, worked out once per render from the explorer.
struct TabData {
    id: TabId,
    /// The name, for the accessibility tree.
    name: SharedString,
    /// The name, isolated for display (an Arabic file name in an English strip).
    shown: SharedString,
    /// The tooltip: the path, or what the special place is.
    tooltip: SharedString,
    glyph: Glyph,
    /// A loading folder or a running search: the icon pulses.
    busy: bool,
    /// The folder files dropped on the tab go into: not a search, an archive, a closed folder, a remote
    /// (read-only), This PC, Recent or the Gallery (`dropInto`).
    drop_into: Option<String>,
}

/// A pane's strip as drawn.
struct PaneData {
    index: usize,
    tabs: Vec<TabData>,
    active: TabId,
    /// The pane the keyboard acts on.
    current: bool,
}

/// A drive's name in the interface language ("Local Disk (C:)", "Ubuntu (WSL)"): `explorer::drive_name` is
/// English, so the default labels are translated here.
fn drive_title(drive: &Drive) -> String {
    let label: String = if drive.label.is_empty() {
        match drive.kind {
            DriveKind::Fixed => t!("Local Disk"),
            DriveKind::Removable => t!("USB Drive"),
            DriveKind::Network => t!("Network Drive"),
            DriveKind::Cdrom => t!("DVD Drive"),
            DriveKind::Ram => t!("RAM Disk"),
            DriveKind::Wsl => t!("Linux"),
            DriveKind::Unknown => t!("Drive"),
        }
        .to_string()
    } else {
        drive.label.clone()
    };
    if drive.kind == DriveKind::Wsl {
        format!("{label} (WSL)")
    } else {
        format!("{label} ({})", drive.path.strip_suffix('\\').unwrap_or(&drive.path))
    }
}

/// What a tab is called: a search's query, the fixed names translated, a drive's name, a folder's name
/// (`tabName`).
fn name_of(explorer: &Explorer, tab: Option<&Tab>) -> String {
    let path = tab.map_or(THIS_PC, |t| t.path.as_str());
    if let Some(find) = tab.and_then(|t| t.find.as_ref()) {
        return find.query.clone();
    }
    if path == THIS_PC {
        t!("This PC").to_string()
    } else if is_recycle_bin(path) {
        t!("Recycle Bin").to_string()
    } else if is_recent(path) {
        t!("Recent").to_string()
    } else if is_gallery(path) {
        t!("Gallery").to_string()
    } else if is_root(path) && !is_remote_path(path) && !is_shadow_path(path) {
        match explorer.drives().iter().find(|d| same_path(&d.path, path)) {
            Some(drive) => drive_title(drive),
            None => path.to_string(),
        }
    } else {
        explorer.tab_name(path)
    }
}

/// The tab as drawn.
fn tab_data(explorer: &Explorer, id: TabId) -> TabData {
    let tab = explorer.tab(id);
    let path = tab.map_or(THIS_PC, |t| t.path.as_str());
    let query = tab.and_then(|t| t.find.as_ref()).map(|f| f.query.as_str());
    let name = name_of(explorer, tab);
    let place: String = if path == THIS_PC {
        t!("This PC").to_string()
    } else if is_recycle_bin(path) {
        t!("Recycle Bin").to_string()
    } else if is_recent(path) {
        t!("Recent: the files opened lately").to_string()
    } else if is_gallery(path) {
        t!("Gallery: your photos and videos").to_string()
    } else {
        path.to_string()
    };
    let tooltip = match query {
        Some(query) => t!("Search results for “{query}” in {place}", query = query, place = place).to_string(),
        None => place,
    };
    let glyph = if query.is_some() {
        Glyph::Search
    } else if path == THIS_PC {
        Glyph::ThisPc
    } else if is_recycle_bin(path) {
        Glyph::RecycleBin
    } else if is_recent(path) {
        Glyph::Recent
    } else if is_gallery(path) {
        Glyph::Gallery
    } else if is_remote_path(path) && is_root(path) {
        Glyph::Cloud
    } else if !is_root(path) {
        Glyph::Folder
    } else {
        let kind = explorer
            .drives()
            .iter()
            .find(|d| same_path(&d.path, path))
            .map_or(DriveKind::Fixed, |d| d.kind);
        Glyph::Drive(kind)
    };
    TabData {
        id,
        shown: isolate(name.clone()),
        name: name.into(),
        tooltip: tooltip.into(),
        glyph,
        busy: tab.is_some_and(|t| t.loading || t.find.as_ref().is_some_and(|f| f.running)),
        drop_into: tab
            .filter(|t| {
                t.find.is_none()
                    && t.path != THIS_PC
                    && !t.listing.as_ref().is_some_and(|l| l.archive.is_some())
                    && !t.is_closed()
                    && !in_archive_path(&t.path)
                    && !is_remote_path(&t.path)
                    && !is_recent(&t.path)
                    && !is_gallery(&t.path)
            })
            .map(|t| t.path.clone()),
    }
}

/// What the drag's picture says a drop on a folder does ("Copy to Documents", "Move to Documents"): the words
/// are Glint's, `%1` stands for the name. The address bar's crumbs use it too.
pub(crate) fn drop_words(name: &str) -> Under {
    Under {
        name: name.to_string(),
        copy: t!("Copy to %1").to_string(),
        moves: t!("Move to %1").to_string(),
        always_move: false,
    }
}

// ---- Dragging a tab ---------------------------------------------------------------------------------

/// A tab being dragged (GPUI's drag payload). The strips take it to reorder or to move it to the other pane;
/// the file panes take it too, with [`PaneDropZone`], to move it into the pane or open it side by side.
#[derive(Clone, Debug)]
pub struct DraggedTab {
    pub id: TabId,
    /// The pane it came from.
    pub from_pane: usize,
    name: SharedString,
    glyph: Glyph,
}

/// What follows the pointer while a tab is dragged: the tab's chip.
struct DragChip {
    name: SharedString,
    glyph: Glyph,
}

impl Render for DragChip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        row_start()
            .gap(space(1.5))
            .h(TAB_HEIGHT)
            .px(space(2.5))
            .max_w(space(52.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.stroke)
            .bg(t.card)
            .text_color(t.foreground)
            .text_size(theme::size::SM)
            .shadow(theme::elevation::floating(cx))
            .child(icon(self.glyph.icon()).size_4().text_color(t.muted_foreground))
            .child(div().min_w_0().truncate().child(isolate(self.name.clone())))
    }
}

/// Where a dragged tab would land: the gap before tab `index` of pane `pane` (counted with the dragged tab
/// still where it was, as [`Explorer::place_tab`] takes it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Slot {
    pane: usize,
    index: usize,
}

/// Which edge of a tab the drop marker is drawn on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Start,
    End,
}

/// The gap a tab dropped at `x` would take: before the first tab whose middle is past `x` in the reading
/// direction, else at the end (`dropAt`). `tabs` are the tabs' bounds in order.
fn slot_at(x: Pixels, tabs: &[Bounds<Pixels>], rtl: bool) -> usize {
    tabs.iter()
        .position(|b| {
            let middle = b.origin.x + b.size.width / 2.;
            if rtl {
                x > middle
            } else {
                x < middle
            }
        })
        .unwrap_or(tabs.len())
}

/// The overlay shown over a pane's files while a tab is dragged: dropping there moves the tab into that
/// pane (not its pane's only tab), or, with one pane, from its far half, opens a second pane beside it
/// (`PaneDropZone` and `zoneAt`). Put it last in the pane's container (which must be `relative`); it draws
/// nothing, and takes no mouse, unless a tab is being dragged.
#[derive(IntoElement)]
pub struct PaneDropZone {
    pane: usize,
}

impl PaneDropZone {
    pub fn new(pane: usize) -> Self {
        Self { pane }
    }
}

impl RenderOnce for PaneDropZone {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let dragged = cx.has_active_drag();
        if !dragged {
            return div().into_any_element();
        }
        let pane = self.pane;
        let explorer = explorer::explorer(cx);
        let (dual, from_len_ok) = {
            let e = explorer.read(cx);
            (e.is_dual(), e.panes().iter().any(|p| p.tabs.len() > 1))
        };
        let t = theme::tokens(cx).clone();
        let rtl = i18n::is_rtl();
        // With two panes: the whole pane (not for a pane's only tab). With one: the far half.
        let zone = |split: bool| {
            let label = if split {
                t!("Open side by side")
            } else {
                t!("Move the tab here")
            };
            div()
                .id(SharedString::from(format!("pane-drop-{pane}")))
                .role(Role::Group)
                .aria_label(label.clone())
                .absolute()
                .top_0()
                .bottom_0()
                .map(|d| {
                    if split {
                        // The far half: the right in English, the left in Arabic.
                        let d = d.w_1_2();
                        if rtl {
                            d.left_0()
                        } else {
                            d.right_0()
                        }
                    } else {
                        d.left_0().right_0()
                    }
                })
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::size::RADIUS)
                .border_2()
                .border_dashed()
                .border_color(t.primary.opacity(0.7))
                .bg(t.primary.opacity(0.1))
                .child(
                    row_start()
                        .gap(space(1.5))
                        .px(space(2.5))
                        .py(space(1.))
                        .rounded(theme::size::RADIUS)
                        .bg(t.background.opacity(0.95))
                        .text_size(theme::size::SM)
                        .text_color(t.primary)
                        .child(icon(IconName::Columns2).size_4())
                        .child(label),
                )
                .on_drop({
                    let explorer = explorer.clone();
                    move |dragged: &DraggedTab, _, cx| {
                        drop_taken();
                        let id = dragged.id;
                        explorer.update(cx, |e, cx| {
                            if split {
                                e.split_with_tab(id, cx);
                            } else {
                                e.move_tab_to_pane(id, pane, cx);
                            }
                        });
                    }
                })
        };
        if dual {
            if from_len_ok {
                zone(false).into_any_element()
            } else {
                div().into_any_element()
            }
        } else {
            zone(true).into_any_element()
        }
    }
}

// ---- The view ---------------------------------------------------------------------------------------

/// The pane menu while it is open.
struct OpenMenu {
    at: Point<Pixels>,
    menu: Entity<PopupMenu>,
    _dismissed: Subscription,
}

/// The tabs of the title bar: one strip for each pane on screen (two in dual-pane view), each with its `+`
/// button and, in dual-pane view, its pane menu button.
pub struct TabStrip {
    explorer: Entity<Explorer>,
    /// Each pane's strip scroll, by pane.
    scrolls: Vec<ScrollHandle>,
    /// The tab last scrolled into view in each pane, to scroll again only when another becomes active.
    scrolled_to: Vec<Option<TabId>>,
    /// Where a dragged tab would drop.
    slot: Option<Slot>,
    /// The tab being dragged and the pointer where the drag began, for pulling it out of the window.
    drag_from: Option<(TabId, Point<Pixels>)>,
    pane_menu: Option<OpenMenu>,
    /// The tab files are held over, lit while they are (`springTab`).
    spring: Option<TabId>,
    /// Opens the sprung tab when its delay passes.
    spring_timer: Option<Task<()>>,
    /// The keyboard focus of each tab, for Tab, Enter and the arrows.
    focus: HashMap<TabId, FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl TabStrip {
    /// The strip, reading the app's explorer (`explorer::explorer(cx)`, so `explorer::init` must have run).
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let subscriptions = vec![cx.observe(&explorer, |_, _, cx| cx.notify())];
        Self {
            explorer,
            scrolls: Vec::new(),
            scrolled_to: Vec::new(),
            slot: None,
            drag_from: None,
            pane_menu: None,
            spring: None,
            spring_timer: None,
            focus: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    /// What to draw, from the explorer; also makes the scroll handles and focus handles it needs.
    fn collect(&mut self, cx: &mut Context<Self>) -> Vec<PaneData> {
        let explorer = self.explorer.clone();
        let (panes, active_pane, data): (Vec<Pane>, usize, HashMap<TabId, TabData>) = {
            let e = explorer.read(cx);
            let panes = e.shown_panes().to_vec();
            let data = panes
                .iter()
                .flat_map(|p| p.tabs.iter().copied())
                .map(|id| (id, tab_data(e, id)))
                .collect();
            (panes, e.active_pane(), data)
        };
        while self.scrolls.len() < panes.len() {
            self.scrolls.push(ScrollHandle::new());
            self.scrolled_to.push(None);
        }
        self.focus.retain(|id, _| data.contains_key(id));
        for id in data.keys() {
            self.focus.entry(*id).or_insert_with(|| cx.focus_handle());
        }
        let mut data = data;
        panes
            .into_iter()
            .enumerate()
            .map(|(index, pane)| PaneData {
                index,
                active: pane.active,
                current: index == active_pane,
                tabs: pane.tabs.iter().filter_map(|id| data.remove(id)).collect(),
            })
            .collect()
    }

    /// Opens the pane menu below the click (the pane button).
    fn open_pane_menu(&mut self, pane: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let explorer = self.explorer.clone();
        let menu = PopupMenu::build(window, cx, move |menu, _, cx| pane_menu(menu, &explorer, pane, cx));
        window.focus(&menu.focus_handle(cx), cx);
        let dismissed = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.pane_menu = None;
            cx.notify();
        });
        self.pane_menu = Some(OpenMenu {
            at,
            menu,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    /// Files are held over `tab` (or over no tab: `None`): it lights, and one that isn't showing opens after
    /// [`SPRING_DELAY`], so the files can go into its folder (`hoverTab`).
    fn spring_to(&mut self, tab: Option<TabId>, cx: &mut Context<Self>) {
        if self.spring == tab {
            return;
        }
        self.spring = tab;
        self.spring_timer = None;
        cx.notify();
        let Some(id) = tab else {
            return;
        };
        let hidden = !self.explorer.read(cx).shown_panes().iter().any(|p| p.active == id);
        if !hidden {
            return;
        }
        self.spring_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SPRING_DELAY).await;
            this.update(cx, |this, cx| {
                // The drag may have ended, or moved on, while it waited.
                if this.spring == Some(id) && cx.has_active_drag() {
                    this.explorer.update(cx, |e, cx| e.activate_tab(id, cx));
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Files dropped on a tab go into its folder: copied, or moved when the drop moves.
    fn drop_files_on(&mut self, dest: Option<&str>, paths: Vec<String>, cx: &mut Context<Self>) {
        self.spring_to(None, cx);
        let Some(dest) = dest else {
            return;
        };
        let moves = crate::drops::effect(cx) == DropEffect::Move;
        self.explorer
            .update(cx, |e, cx| e.drop_files(paths, dest, moves, cx).detach());
    }

    /// A tab dropped on pane `pane`'s strip (`count` tabs): into the gap it was shown for, else at the end.
    fn drop_tab(&mut self, pane: usize, count: usize, id: TabId, cx: &mut Context<Self>) {
        drop_taken();
        let to = self.slot.filter(|s| s.pane == pane).map_or(count, |s| s.index);
        self.slot = None;
        self.explorer.update(cx, |e, cx| e.place_tab(id, pane, to, cx));
        cx.notify();
    }

    /// The mouse button went up outside the strip while a tab was dragged: a tab pulled out of the window opens
    /// its folder in a window of its own where it was dropped (`dragTab`'s end and `tearOffTab`).
    fn released(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((id, from)) = self.drag_from.take() else {
            return;
        };
        let viewport = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let away = (position.y - from.y).abs();
        match tear_off_for(viewport.contains(&position), away) {
            TearOff::No => {}
            TearOff::Now => self.tear_off(id, position, window, cx),
            TearOff::IfNotTaken => {
                DROP_TAKEN.with(|taken| taken.set(false));
                // The drop handlers run after this (the capture phase); the deferred call sees what they did.
                let strip = cx.entity();
                window.defer(cx, move |window, cx| {
                    if !DROP_TAKEN.with(Cell::get) {
                        strip.update(cx, |this, cx| this.tear_off(id, position, window, cx));
                    }
                });
            }
        }
    }

    /// Opens tab `id`'s folder in a window of its own with its corner at `position` (in this window), then closes
    /// the tab unless it is the last one. NOTE(port): the web app closed it whether or not the window opened; here
    /// a window that can't start leaves the tab where it was.
    fn tear_off(&mut self, id: TabId, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let explorer = self.explorer.clone();
        let folder = {
            let e = explorer.read(cx);
            e.tab(id).map(|tab| e.new_window_folder(Some(&tab.path)))
        };
        let Some(folder) = folder else {
            return;
        };
        let at = secondary::screen_point(window, position);
        secondary::open_window(window, cx, &folder, Some(at), move |_, cx| {
            explorer.update(cx, |e, cx| {
                let tabs: usize = e.panes().iter().map(|pane| pane.tabs.len()).sum();
                if tabs > 1 {
                    e.close_tab(Some(id), cx);
                }
            });
        });
    }

    /// A key on a focused tab: Enter and Space show it, the arrows (Home, End) move to and show the next tab
    /// in the reading direction, Delete closes it.
    fn key(&mut self, pane: usize, id: TabId, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let rtl = i18n::is_rtl();
        let tabs = self
            .explorer
            .read(cx)
            .panes()
            .get(pane)
            .map(|p| p.tabs.clone())
            .unwrap_or_default();
        let here = tabs.iter().position(|&t| t == id).unwrap_or(0);
        let go_to = match event.keystroke.key.as_str() {
            "enter" | "space" => Some(here),
            "right" => Some(if rtl {
                here.saturating_sub(1)
            } else {
                (here + 1).min(tabs.len() - 1)
            }),
            "left" => Some(if rtl {
                (here + 1).min(tabs.len() - 1)
            } else {
                here.saturating_sub(1)
            }),
            "home" => Some(0),
            "end" => Some(tabs.len() - 1),
            "delete" => {
                let can_close = tabs.len() > 1 || self.explorer.read(cx).is_dual();
                if can_close {
                    self.explorer.update(cx, |e, cx| e.close_tab(Some(id), cx));
                }
                cx.stop_propagation();
                return;
            }
            _ => None,
        };
        let Some(target) = go_to.and_then(|i| tabs.get(i).copied()) else {
            return;
        };
        cx.stop_propagation();
        self.explorer.update(cx, |e, cx| e.activate_tab(target, cx));
        if let Some(handle) = self.focus.get(&target) {
            window.focus(handle, cx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tab(
        &self,
        pane: usize,
        index: usize,
        d: &TabData,
        selected: bool,
        current: bool,
        can_close: bool,
        mark: Option<Edge>,
        count: usize,
        scroll: &ScrollHandle,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rtl = i18n::is_rtl();
        let id = d.id;
        let group = SharedString::from(format!("tab-group-{}", id.0));
        let busy_id = SharedString::from(format!("tab-busy-{}", id.0));
        let tip = d.tooltip.clone();
        let glyph_color = if d.glyph == Glyph::Folder {
            t.warning
        } else {
            t.muted_foreground
        };
        let glyph = div()
            .id(busy_id.clone())
            .flex_shrink_0()
            .child(icon(d.glyph.icon()).size_4().text_color(glyph_color));
        let glyph = if d.busy {
            // The TypeScript's `animate-pulse`.
            glyph
                .with_animation(
                    busy_id,
                    Animation::new(std::time::Duration::from_secs(2))
                        .repeat()
                        .with_easing(pulsating_between(0.4, 1.0)),
                    |d, delta| d.opacity(delta),
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
        };
        let close = can_close.then(|| {
            let close_group = group.clone();
            div()
                .id(SharedString::from(format!("tab-close-wrap-{}", id.0)))
                .flex_shrink_0()
                .opacity(if selected { 1. } else { 0. })
                .group_hover(close_group, |s| s.opacity(1.))
                // Pressing × must not activate (and so re-read) the tab that is about to go.
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .child(
                    Button::new(SharedString::from(format!("tab-close-{}", id.0)))
                        .ghost()
                        .xsmall()
                        .icon(IconName::X)
                        .accessibility_label(t!("Close {name}", name = d.name))
                        .tooltip(keyed("Close tab ({0})", &[&crate::actions::CloseTab]))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.explorer.update(cx, |e, cx| e.close_tab(Some(id), cx));
                        })),
                )
        });
        let background = t.background;
        let accent = t.accent;
        let stroke = t.stroke;
        let raised = theme::elevation::raised(cx);
        let glint = theme::glint_fill(cx);
        let ring = t.ring;
        let primary = t.primary;
        let wheel_scroll = scroll.clone();
        let menu_explorer = self.explorer.clone();
        let sprung = self.spring.filter(|_| cx.has_active_drag()) == Some(id);
        let lit = sprung && (!selected || d.drop_into.is_some());
        let loading_bar = sprung && !selected;
        let drop_into = d.drop_into.clone();
        let over_name = d.name.to_string();
        let drop_dest = d.drop_into.clone();
        let drag = DraggedTab {
            id,
            from_pane: pane,
            name: d.name.clone(),
            glyph: d.glyph,
        };
        div()
            .id(SharedString::from(format!("tab-{}", id.0)))
            .test_support()
            .role(Role::Tab)
            .aria_label(d.name.clone())
            .aria_description(d.tooltip.clone())
            .aria_selected(selected)
            .aria_position_in_set(index + 1)
            .group(group)
            .relative()
            .occlude()
            // Tabs are as wide as they can be, and give way together when the strip is full.
            .flex_grow_0()
            .flex_shrink_1()
            .flex_basis(space(52.))
            .min_w(space(24.))
            .max_w(space(52.))
            .h(TAB_HEIGHT)
            .child(
                row_start()
                    .id(SharedString::from(format!("tab-fill-{}", id.0)))
                    .size_full()
                    .gap(space(1.5))
                    .ps(2.5)
                    .pe(1.)
                    .rounded(theme::size::RADIUS_LG)
                    .border_1()
                    .border_color(if selected {
                        stroke
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .text_size(theme::size::SM)
                    .text_color(if selected { t.foreground } else { t.muted_foreground })
                    .when(selected, move |d| d.bg(background).shadow(raised))
                    .when(!selected, {
                        let foreground = t.foreground;
                        move |d| d.hover(move |s| s.bg(accent).text_color(foreground))
                    })
                    // Files held over it: lit, and shown soon when it isn't.
                    .when(lit, move |d| {
                        d.bg(primary.opacity(0.15)).text_color(primary).border_color(primary)
                    })
                    .when(loading_bar, move |d| {
                        d.child(
                            div()
                                .id(SharedString::from(format!("tab-spring-{}", id.0)))
                                .absolute()
                                .bottom(space(0.5))
                                .left(space(1.5))
                                .h(space(0.5))
                                .rounded_full()
                                .bg(primary)
                                .with_animation(
                                    SharedString::from(format!("tab-spring-bar-{}", id.0)),
                                    Animation::new(SPRING_DELAY),
                                    |bar, delta| bar.w(relative(delta * 0.9)),
                                ),
                        )
                    })
                    .when_some(mark, |d, edge| {
                        let left = (edge == Edge::Start) != rtl;
                        d.child(
                            div()
                                .absolute()
                                .top(space(0.5))
                                .bottom(space(0.5))
                                .w(space(0.5))
                                .rounded_full()
                                .bg(primary)
                                .map(|m| if left { m.left(px(-1.)) } else { m.right(px(-1.)) }),
                        )
                    })
                    .when(current, move |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(-1.))
                                .left(space(3.))
                                .right(space(3.))
                                .h(px(2.))
                                .rounded_full()
                                .bg(glint),
                        )
                    })
                    .child(glyph)
                    .child(div().flex_1().min_w_0().truncate().child(d.shown.clone()))
                    .children(close)
                    // A tab that has just been made rises into its place.
                    .with_animation(
                        SharedString::from(format!("tab-enter-{}", id.0)),
                        Animation::new(theme::motion::BASE).with_easing(theme::motion::enter),
                        |fill, delta| fill.opacity(delta).top(px(6. * (1. - delta))),
                    ),
            )
            .track_focus(&self.focus[&id])
            .tab_stop(true)
            .focus_visible(move |s| s.border_1().border_color(ring))
            .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
            // Pressing a tab shows it, as the TypeScript's mousedown did; the keyboard focus stays where it was.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    this.explorer.update(cx, |e, cx| e.activate_tab(id, cx));
                }),
            )
            // A middle-click closes the tab.
            .on_aux_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.is_middle_click() && can_close {
                    this.explorer.update(cx, |e, cx| e.close_tab(Some(id), cx));
                }
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                this.key(pane, id, event, window, cx);
            }))
            .on_scroll(move |event, window, _| wheel(&wheel_scroll, event, rtl, window))
            .on_drag(drag, |drag, _, _, cx| {
                cx.new(|_| DragChip {
                    name: drag.name.clone(),
                    glyph: drag.glyph,
                })
            })
            // Files dragged over it: the tab under the pointer lights and, if it isn't showing, opens after a
            // moment; a drop there goes into its folder (the drag's words say so). Every tab hears every move.
            .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                let inside = event.bounds.contains(&event.event.position);
                if inside {
                    this.spring_to(Some(id), cx);
                    if drop_into.is_some() {
                        crate::drops::answer(cx, Answer::Accept(drop_words(&over_name)));
                    }
                } else if this.spring == Some(id) {
                    this.spring_to(None, cx);
                }
            }))
            .on_drop(cx.listener(move |this, drop: &ExternalPaths, _, cx| {
                this.drop_files_on(drop_dest.as_deref(), crate::drops::paths(drop), cx);
            }))
            // Tabs occlude the strip, so a tab dropped on a tab is dropped here.
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.drop_tab(pane, count, dragged.id, cx);
            }))
            .context_menu(move |menu, _, cx| tab_menu(menu, &menu_explorer, id, cx))
    }
}

/// The mouse wheel scrolls the strip sideways (`onWheel`): down shows the later tabs.
fn wheel(scroll: &ScrollHandle, event: &ScrollEvent, rtl: bool, window: &Window) {
    let delta = event.delta.pixel_delta(window.line_height());
    if delta.y == px(0.) || delta.x != px(0.) {
        return;
    }
    let offset = scroll.offset();
    let max = scroll.max_offset().x;
    // Offsets run from 0 (the strip's left end) to minus the overflow. English's later tabs are to the right,
    // Arabic's to the left.
    let moved = if rtl { offset.x - delta.y } else { offset.x + delta.y };
    scroll.set_offset(point(moved.clamp(-max, px(0.)), offset.y));
}

impl Render for TabStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let rtl = i18n::is_rtl();
        let panes = self.collect(cx);
        let dual = panes.len() > 1;
        let dragging = cx.has_active_drag();
        let slot = self.slot.filter(|_| dragging);
        if !dragging {
            self.drag_from = None;
        }
        let _ = window;

        let mut strips = Vec::with_capacity(panes.len());
        for pane in &panes {
            let index = pane.index;
            let count = pane.tabs.len();
            let can_close = count > 1 || dual;
            let scroll = self.scrolls[index].clone();
            // Another tab became active: keep it in view (a tab brought into view stops clear of the edge).
            if self.scrolled_to[index] != Some(pane.active) {
                self.scrolled_to[index] = Some(pane.active);
                if let Some(at) = pane.tabs.iter().position(|d| d.id == pane.active) {
                    scroll.scroll_to_item(if rtl { count - at } else { at });
                }
            }
            // The tabs in reading order. Arabic runs right to left: the scroll is laid out left to right with the
            // tabs reversed after a spacer that keeps them against the right edge while they all fit.
            let mut tabs: Vec<AnyElement> = pane
                .tabs
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    let mark = slot.and_then(|s| {
                        if s.pane != index {
                            None
                        } else if s.index == i {
                            Some(Edge::Start)
                        } else if s.index == count && i + 1 == count {
                            Some(Edge::End)
                        } else {
                            None
                        }
                    });
                    self.tab(
                        index,
                        i,
                        d,
                        d.id == pane.active,
                        d.id == pane.active && pane.current,
                        can_close,
                        mark,
                        count,
                        &scroll,
                        &t,
                        cx,
                    )
                    .into_any_element()
                })
                .collect();
            if rtl {
                tabs.reverse();
                tabs.insert(0, div().flex_1().min_w_0().into_any_element());
            }
            let list_label = if !dual {
                t!("Tabs")
            } else if index == 0 {
                t!("Left pane tabs")
            } else {
                t!("Right pane tabs")
            };
            let drop_scroll = scroll.clone();
            let wheel_scroll = scroll.clone();
            let list = div()
                .id(SharedString::from(format!("tablist-{index}")))
                .test_support()
                .role(Role::TabList)
                .aria_label(list_label)
                .flex()
                .flex_row()
                // As wide as its tabs, and no wider: the + sits right after the last one, until they fill the strip.
                .flex_initial()
                .min_w_0()
                .h_full()
                .items_center()
                .gap(space(1.))
                .overflow_x_scroll()
                .track_scroll(&scroll)
                // Double-clicking the empty strip makes a tab (the tabs themselves occlude it).
                .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                    if event.click_count() == 2 {
                        this.explorer.update(cx, |e, cx| e.new_tab(None, cx));
                    }
                }))
                .on_scroll(move |event, window, _| wheel(&wheel_scroll, event, rtl, window))
                // A tab dragged over the strip: the gap it would take.
                .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                    let over = event.bounds.contains(&event.event.position);
                    let skip = usize::from(rtl);
                    let new = if over {
                        let mut bounds: Vec<Bounds<Pixels>> = (0..count)
                            .filter_map(|i| drop_scroll.bounds_for_item(skip + if rtl { count - 1 - i } else { i }))
                            .collect();
                        bounds.truncate(count);
                        Some(Slot {
                            pane: index,
                            index: slot_at(event.event.position.x, &bounds, rtl),
                        })
                    } else if this.slot.is_some_and(|s| s.pane == index) {
                        None
                    } else {
                        this.slot
                    };
                    if new != this.slot {
                        this.slot = new;
                        cx.notify();
                    }
                }))
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                    this.drop_tab(index, count, dragged.id, cx);
                }))
                .children(tabs);

            // The empty room after the buttons: takes no mouse except a dragged tab, so the title bar still
            // moves the window by it, and a tab dropped there goes to the end.
            let filler = div()
                .id(SharedString::from(format!("tab-filler-{index}")))
                .flex_1()
                .min_w_0()
                .h_full()
                .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                    if event.bounds.contains(&event.event.position) {
                        let end = Some(Slot {
                            pane: index,
                            index: count,
                        });
                        if this.slot != end {
                            this.slot = end;
                            cx.notify();
                        }
                    }
                }))
                .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                    this.drop_tab(index, count, dragged.id, cx);
                }));

            let new_tab = Button::new(SharedString::from(format!("new-tab-{index}")))
                .ghost()
                .small()
                .icon(IconName::Plus)
                .accessibility_label(t!("New tab"))
                .tooltip(keyed("New tab ({0})", &[&crate::actions::NewTab]))
                .on_click(cx.listener(move |this, _, _, cx| {
                    // A new tab goes where the keyboard is: this pane's button acts on this pane.
                    this.explorer.update(cx, |e, cx| {
                        e.focus_pane(index, cx);
                        e.new_tab(None, cx);
                    });
                }));
            let pane_button = dual.then(|| {
                Button::new(SharedString::from(format!("pane-menu-{index}")))
                    .ghost()
                    .small()
                    .icon(IconName::Columns2)
                    .accessibility_label(t!("Pane menu"))
                    .tooltip(t!("This pane: swap, show a folder in the other pane, even out, close"))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.open_pane_menu(index, event.position(), window, cx);
                    }))
            });
            strips.push(
                row_start()
                    .id(SharedString::from(format!("tab-strip-{index}")))
                    .flex_1()
                    .flex_basis(px(0.))
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap(space(1.))
                    .child(list)
                    // The buttons take the mouse (the title bar's drag area stops at them).
                    .child(
                        row_start()
                            .occlude()
                            .flex_shrink_0()
                            .items_center()
                            .self_center()
                            .child(new_tab),
                    )
                    .child(filler)
                    .children(
                        pane_button.map(|button| row_start().occlude().flex_shrink_0().self_center().child(button)),
                    ),
            );
        }

        let menu = self.pane_menu.as_ref().map(|open| {
            deferred(
                anchored()
                    .position(open.at)
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.))
                    .child(open.menu.clone()),
            )
            .with_priority(1)
        });
        row_start()
            .id("tab-strip")
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .gap(space(4.))
            // Where a tab's drag began, and where it ended if that was outside the strip.
            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                if this.drag_from.is_none() {
                    this.drag_from = Some((event.drag(cx).id, event.event.position));
                }
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| this.released(event.position, window, cx)),
            )
            .children(strips)
            .children(menu)
    }
}

// ---- The menus --------------------------------------------------------------------------------------

/// The tab's menu (`tabMenu`): duplicate, show beside, move, close, close the others, reopen. Built when it
/// opens, so what is disabled is right at that moment.
fn tab_menu(menu: PopupMenu, explorer: &Entity<Explorer>, id: TabId, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let (dual, tabs, path, reopen) = {
        let e = explorer.read(cx);
        let pane = e.pane_of(id);
        (
            e.is_dual(),
            pane.and_then(|p| e.panes().get(p)).map_or(0, |p| p.tabs.len()),
            e.tab(id).map(|t| t.path.clone()),
            e.can_reopen_tab(),
        )
    };
    let pane_of = explorer.read(cx).pane_of(id);
    let act = |f: fn(&mut Explorer, TabId, &mut Context<Explorer>)| {
        let explorer = explorer.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| explorer.update(cx, |e, cx| f(e, id, cx))
    };
    let side = if dual {
        t!("Open in the other pane")
    } else {
        t!("Open side by side")
    };
    let menu = menu
        .item(
            PopupMenuItem::new(t!("Duplicate tab"))
                .icon(IconName::Copy)
                .on_click(act(|e, id, cx| e.duplicate_tab(id, cx))),
        )
        .item(PopupMenuItem::new(side).icon(IconName::Columns2).on_click({
            let explorer = explorer.clone();
            move |_, _, cx| {
                if let Some(path) = path.clone() {
                    explorer.update(cx, |e, cx| e.open_in_other_pane(&path, pane_of, cx).detach());
                }
            }
        }));
    let menu = if dual {
        menu.item(
            PopupMenuItem::new(t!("Move to the other pane"))
                .icon(IconName::ArrowRightLeft)
                .disabled(tabs < 2)
                .on_click(act(|e, id, cx| e.move_tab_to_other_pane(id, cx))),
        )
    } else {
        menu
    };
    menu.separator()
        .item(
            PopupMenuItem::new(t!("Close tab"))
                .icon(IconName::X)
                .disabled(tabs < 2 && !dual)
                .on_click(act(|e, id, cx| e.close_tab(Some(id), cx))),
        )
        .item(
            PopupMenuItem::new(t!("Close other tabs"))
                .icon(IconName::X)
                .disabled(tabs < 2)
                .on_click(act(|e, id, cx| e.close_other_tabs(id, cx))),
        )
        .item(
            PopupMenuItem::new(t!("Reopen closed tab"))
                .icon(IconName::RotateCcw)
                .disabled(!reopen)
                .on_click(act(|e, _, cx| e.reopen_closed_tab(cx))),
        )
}

/// A pane's menu (`paneMenu`): swap, show this folder in the other pane, bring the other pane's folder
/// here, even out the panes, close the pane.
fn pane_menu(menu: PopupMenu, explorer: &Entity<Explorer>, pane: usize, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let folder = |path: Option<&str>| path.filter(|p| *p != THIS_PC).map(str::to_string);
    let (here, there) = {
        let e = explorer.read(cx);
        (
            folder(e.pane_tab(pane).map(|t| t.path.as_str())),
            folder(e.pane_tab(1 - pane.min(1)).map(|t| t.path.as_str())),
        )
    };
    let even = state::settings(cx).dual_split.is_none();
    let show = |path: Option<String>, from: usize, label: SharedString, glyph: IconName| {
        let explorer = explorer.clone();
        PopupMenuItem::new(label)
            .icon(glyph)
            .disabled(path.is_none())
            .on_click(move |_, _, cx| {
                if let Some(path) = &path {
                    explorer.update(cx, |e, cx| e.open_in_other_pane(path, Some(from), cx).detach());
                }
            })
    };
    menu.item(
        PopupMenuItem::new(t!("Swap panes"))
            .icon(IconName::ArrowLeftRight)
            .on_click({
                let explorer = explorer.clone();
                move |_, _, cx| explorer.update(cx, |e, cx| e.swap_panes(cx))
            }),
    )
    .item(show(
        here,
        pane,
        t!("Show this folder in the other pane"),
        IconName::Columns2,
    ))
    .item(show(
        there,
        1 - pane.min(1),
        t!("Open the other pane's folder here"),
        IconName::FolderInput,
    ))
    .separator()
    .item(
        PopupMenuItem::new(t!("Even out the panes"))
            .icon(IconName::Columns2)
            .disabled(even)
            .on_click(|_, _, cx| state::update(cx, |settings| settings.dual_split = None)),
    )
    .item(PopupMenuItem::new(t!("Close this pane")).icon(IconName::X).on_click({
        let explorer = explorer.clone();
        move |_, _, cx| explorer.update(cx, |e, cx| e.close_pane(pane, cx))
    }))
}

#[cfg(test)]
mod tests;
