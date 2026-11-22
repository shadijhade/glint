//! The tabs: a list of the open folders at the top of the side rail, one under the other (they were a strip in
//! the title bar; a list shows every name whole and leaves the title bar to the address). Ported from
//! src/components/explorer/TabStrip.tsx and the tab handling in Explorer.tsx (`tabMenu`, `paneMenu`).
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
//! **In the rail.** Each pane on screen has its list (two in dual-pane view, the first pane's first), under a
//! small header that names it and holds its `+` and, in dual-pane view, its pane menu. A list grows with its
//! tabs up to [`MOST_SHOWN`] rows, then scrolls.

use crate::components::command_bar::keyed;
use crate::components::menus::{Entry, Node};
use crate::components::peek_card::{self, PeekHost};
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
use std::rc::Rc;
use std::time::Duration;

/// How long files held over a tab that isn't showing wait before it opens (`springTimer`, 450 ms).
const SPRING_DELAY: Duration = Duration::from_millis(450);
/// A tab's height: 30 px at the usual size, as a place's row under it.
const TAB_HEIGHT: Rems = rems(1.875);
/// How many tabs a pane's list shows before it scrolls.
const MOST_SHOWN: f32 = 8.;

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

/// The icon a tab gets (`Icon` in TabStrip.tsx), and the head of a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    Search,
    ThisPc,
    RecycleBin,
    Recent,
    Gallery,
    Cloud,
    Folder,
    /// Home, Documents…: the folder in the place's own color.
    Known(explorer::KnownId),
    Drive(DriveKind),
}

impl Glyph {
    fn icon(self) -> IconName {
        match self {
            Glyph::Known(id) => known_icon(id),
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

    /// The tab's picture: a folder (a known one in its own color, with its sign where there is room), or the
    /// place's chip in its own color.
    pub(crate) fn picture(self, across: Pixels, cx: &App) -> Div {
        use theme::Place;
        let tone = match self {
            Glyph::Folder => return crate::art::folder(across, theme::folder_tone(cx), None, cx),
            Glyph::Known(id) => {
                let tone = theme::place_tone(Place::Known(id), cx);
                return crate::art::folder(across, tone, Some(known_icon(id)), cx);
            }
            Glyph::Search | Glyph::ThisPc => theme::tokens(cx).primary,
            Glyph::RecycleBin => theme::place_tone(Place::RecycleBin, cx),
            Glyph::Recent => theme::place_tone(Place::Recent, cx),
            Glyph::Gallery => theme::place_tone(Place::Gallery, cx),
            Glyph::Cloud => theme::place_tone(Place::Remote, cx),
            Glyph::Drive(DriveKind::Wsl) => theme::place_tone(Place::Wsl, cx),
            Glyph::Drive(_) => theme::place_tone(Place::Drive, cx),
        };
        crate::art::chip(across, tone, self.icon(), cx)
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
    /// The folder the tab's peek card is about, when it has one: a tab on a folder, not a search or a place that
    /// is no folder (This PC, the Recycle Bin, Recent, the Gallery).
    peek: Option<String>,
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
pub(crate) fn name_of(explorer: &Explorer, tab: Option<&Tab>) -> String {
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

/// The picture for what a tab shows: a search, a special place, a known folder, a drive, else a folder.
pub(crate) fn glyph_of(explorer: &Explorer, tab: Option<&Tab>) -> Glyph {
    let path = tab.map_or(THIS_PC, |t| t.path.as_str());
    if tab.is_some_and(|t| t.find.is_some()) {
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
    } else if let Some(known) = explorer.known_folders().iter().find(|k| same_path(&k.path, path)) {
        Glyph::Known(known.id)
    } else if !is_root(path) {
        Glyph::Folder
    } else {
        let kind = explorer
            .drives()
            .iter()
            .find(|d| same_path(&d.path, path))
            .map_or(DriveKind::Fixed, |d| d.kind);
        Glyph::Drive(kind)
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
    let glyph = glyph_of(explorer, tab);
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
        peek: tab
            .filter(|t| {
                t.find.is_none()
                    && t.path != THIS_PC
                    && !is_recycle_bin(&t.path)
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
            .bg(t.popover)
            .text_color(t.foreground)
            .text_size(theme::size::SM)
            .shadow(theme::elevation::floating(cx))
            .child(self.glyph.picture(theme::rem_size(cx) * 1.125, cx))
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

/// Which edge of a tab the drop marker is drawn on: above it, or below the last one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Start,
    End,
}

/// The gap a tab dropped at height `y` would take: before the first tab whose middle is below `y`, else at the
/// end (`dropAt`). `tabs` are the tabs' bounds in order, top to bottom.
fn slot_at(y: Pixels, tabs: &[Bounds<Pixels>]) -> usize {
    tabs.iter()
        .position(|b| y < b.origin.y + b.size.height / 2.)
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
            let reduce = cx.reduce_motion();
            let card = div()
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
                .m(space(2.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::size::RADIUS_SHEET)
                .border_1()
                .border_color(t.stroke)
                .bg(t.primary.opacity(0.08))
                .shadow(theme::elevation::raised(cx))
                .child(
                    row_start()
                        .gap(space(1.5))
                        .px(space(3.))
                        .py(space(1.5))
                        .rounded(theme::size::RADIUS_LG)
                        .border_1()
                        .border_color(t.stroke)
                        .bg(t.popover.opacity(0.95))
                        .text_size(theme::size::SM)
                        .text_color(t.primary)
                        .shadow(theme::elevation::floating(cx))
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
                });
            if reduce {
                card.into_any_element()
            } else {
                card.with_animation(
                    ("pane-drop-in", pane),
                    Animation::new(theme::motion::FAST).with_easing(theme::motion::enter),
                    |el, delta| el.opacity(0.35 + 0.65 * delta),
                )
                .into_any_element()
            }
        };
        if dual {
            if from_len_ok {
                zone(false)
            } else {
                div().into_any_element()
            }
        } else {
            zone(true)
        }
    }
}

// ---- The view ---------------------------------------------------------------------------------------

/// The tabs of the title bar: one strip for each pane on screen (two in dual-pane view), each with its `+`
/// button and, in dual-pane view, its pane menu button.
pub struct TabStrip {
    explorer: Entity<Explorer>,
    /// Each pane's strip scroll, by pane.
    scrolls: Vec<ScrollHandle>,
    /// Around each pane's + button, to show it while the keyboard is on it (it is hidden otherwise).
    plus_focus: Vec<FocusHandle>,
    /// Where each pane's menu button sits, so keyboard invocation opens its menu below it.
    pane_menu_bounds: Vec<Rc<Cell<Bounds<Pixels>>>>,
    /// The tab last scrolled into view in each pane, to scroll again only when another becomes active.
    scrolled_to: Vec<Option<TabId>>,
    /// Where a dragged tab would drop.
    slot: Option<Slot>,
    /// The tab being dragged and the pointer where the drag began, for pulling it out of the window.
    drag_from: Option<(TabId, Point<Pixels>)>,
    /// The tab's menu or the pane's, while one is open (one at a time).
    popup: Option<menu::Popup>,
    /// What had the keyboard before the menu took it.
    popup_focus: Option<FocusHandle>,
    /// The tab files are held over, lit while they are (`springTab`).
    spring: Option<TabId>,
    /// Opens the sprung tab when its delay passes.
    spring_timer: Option<Task<()>>,
    /// The keyboard focus of each tab, for Tab, Enter and the arrows.
    focus: HashMap<TabId, FocusHandle>,
    /// Around each tab's × button, so Tab focus keeps it visible.
    close_focus: HashMap<TabId, FocusHandle>,
    /// The card a background tab shows when the pointer rests on it: what its folder holds.
    peek: Entity<PeekHost>,
    _subscriptions: Vec<Subscription>,
}

impl TabStrip {
    /// The strip, reading the app's explorer (`explorer::explorer(cx)`, so `explorer::init` must have run).
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let subscriptions = vec![cx.observe(&explorer, |_, _, cx| cx.notify())];
        let peek = {
            let explorer = explorer.clone();
            cx.new(|cx| PeekHost::new(explorer, cx))
        };
        Self {
            explorer,
            scrolls: Vec::new(),
            plus_focus: Vec::new(),
            pane_menu_bounds: Vec::new(),
            scrolled_to: Vec::new(),
            slot: None,
            drag_from: None,
            popup: None,
            popup_focus: None,
            spring: None,
            spring_timer: None,
            focus: HashMap::new(),
            close_focus: HashMap::new(),
            peek,
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
            self.plus_focus.push(cx.focus_handle());
            self.pane_menu_bounds.push(Rc::default());
            self.scrolled_to.push(None);
        }
        self.focus.retain(|id, _| data.contains_key(id));
        self.close_focus.retain(|id, _| data.contains_key(id));
        for id in data.keys() {
            self.focus.entry(*id).or_insert_with(|| cx.focus_handle());
            self.close_focus.entry(*id).or_insert_with(|| cx.focus_handle());
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

    /// Opens `nodes` as the strip's menu at `at`; what is chosen comes back to [`Self::chose`].
    fn open_popup(
        &mut self,
        nodes: Vec<Node>,
        label: SharedString,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focus = self.popup_focus.take().or_else(|| window.focused(cx));
        let popup = menu::Popup::open(nodes, label, at, window, cx, |this, chose, window, cx| {
            this.close_popup(window, cx);
            if let Some(chose) = chose {
                this.chose(chose, cx);
            }
        });
        if popup.is_some() {
            self.popup_focus = focus;
        }
        self.popup = popup;
        cx.notify();
    }

    /// Closes the menu and gives the keyboard back to what had it.
    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = None;
        if let Some(focus) = self.popup_focus.take() {
            window.defer(cx, move |window, cx| {
                // Only when the choice (a new tab, a dialog) didn't take the keyboard itself.
                if window.focused(cx).is_none() {
                    focus.focus(window, cx);
                }
            });
        }
        cx.notify();
    }

    /// Opens a tab's menu at the pointer.
    fn open_tab_menu(&mut self, id: TabId, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = tab_menu(self.explorer.read(cx), id);
        self.open_popup(nodes, t!("Tab"), at, window, cx);
    }

    /// Opens the pane menu below the click (the pane button).
    fn open_pane_menu(&mut self, pane: usize, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = pane_menu(self.explorer.read(cx), pane, state::settings(cx).dual_split.is_none());
        self.open_popup(nodes, t!("Pane"), at, window, cx);
    }

    /// Does what a row of the tab's or the pane's menu says.
    fn chose(&mut self, chose: menu::Chose, cx: &mut Context<Self>) {
        let tab = || chose.arg.parse::<u32>().ok().map(TabId);
        let pane = || chose.arg.parse::<usize>().ok();
        let explorer = self.explorer.clone();
        match chose.tag.as_str() {
            "duplicate" => {
                if let Some(id) = tab() {
                    explorer.update(cx, |e, cx| e.duplicate_tab(id, cx));
                }
            }
            "side" => {
                if let Some(id) = tab() {
                    explorer.update(cx, |e, cx| {
                        let path = e.tab(id).map(|t| t.path.clone());
                        let from = e.pane_of(id);
                        if let Some(path) = path {
                            e.open_in_other_pane(&path, from, cx).detach();
                        }
                    });
                }
            }
            "move" => {
                if let Some(id) = tab() {
                    explorer.update(cx, |e, cx| e.move_tab_to_other_pane(id, cx));
                }
            }
            "close" => {
                if let Some(id) = tab() {
                    explorer.update(cx, |e, cx| e.close_tab(Some(id), cx));
                }
            }
            "others" => {
                if let Some(id) = tab() {
                    explorer.update(cx, |e, cx| e.close_other_tabs(id, cx));
                }
            }
            "reopen" => explorer.update(cx, |e, cx| e.reopen_closed_tab(cx)),
            "swap" => explorer.update(cx, |e, cx| e.swap_panes(cx)),
            // The arg is the pane whose folder is shown in the other pane.
            "show" => {
                if let Some(of) = pane() {
                    explorer.update(cx, |e, cx| {
                        let path = e.pane_tab(of).map(|t| t.path.clone()).filter(|p| p != THIS_PC);
                        if let Some(path) = path {
                            e.open_in_other_pane(&path, Some(of), cx).detach();
                        }
                    });
                }
            }
            "even" => state::update(cx, |settings| settings.dual_split = None),
            "close-pane" => {
                if let Some(pane) = pane() {
                    explorer.update(cx, |e, cx| e.close_pane(pane, cx));
                }
            }
            _ => {}
        }
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
        // The list runs down the rail: pulled sideways is pulled out of it.
        let away = (position.x - from.x).abs();
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
    /// (Down and Up; Right and Left too, in the reading direction, as when the tabs were a strip), Delete closes it.
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
            "down" => Some((here + 1).min(tabs.len() - 1)),
            "up" => Some(here.saturating_sub(1)),
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
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rtl = i18n::is_rtl();
        let id = d.id;
        let group = SharedString::from(format!("tab-group-{}", id.0));
        let busy_id = SharedString::from(format!("tab-busy-{}", id.0));
        let tip = d.tooltip.clone();
        let glyph = div()
            .id(busy_id.clone())
            .test_support()
            .flex_shrink_0()
            .child(d.glyph.picture(theme::rem_size(cx) * 1.125, cx));
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
                .track_focus(&self.close_focus[&id])
                .opacity(if selected { 1. } else { 0. })
                .group_hover(close_group, |s| s.opacity(1.))
                .in_focus(|s| s.opacity(1.))
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
        let glass = t.glass;
        let accent = t.accent;
        let pressed = t.pressed;
        let stroke = t.stroke;
        let glint = theme::glint_fill(cx);
        let ring = t.ring;
        let primary = t.primary;
        let passive_selected = selected && !current;
        let sprung = self.spring.filter(|_| cx.has_active_drag()) == Some(id);
        let lit = sprung && (!selected || d.drop_into.is_some());
        let loading_bar = sprung && !selected;
        let drop_into = d.drop_into.clone();
        let over_name = d.name.to_string();
        let drop_dest = d.drop_into.clone();
        // A tab that isn't the one shown says what its folder holds when the pointer rests on it, in a card that
        // has its path too (so it has no tooltip).
        let peek_target = d.peek.clone().filter(|_| !selected).map(|path| peek_card::Target {
            key: SharedString::from(format!("tab-{}", id.0)),
            element: SharedString::from(format!("tab-{}", id.0)),
            path,
            name: d.name.clone(),
            hint: None,
            read_ahead: false,
        });
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
            // A row of the rail: as wide as it, a pixel of air above and below its fill.
            .w_full()
            .flex_none()
            .h(TAB_HEIGHT)
            .py(px(1.))
            .child({
                let fill = row_start()
                    .id(SharedString::from(format!("tab-fill-{}", id.0)))
                    .relative()
                    .size_full()
                    .gap(space(1.5))
                    .ps(2.5)
                    .pe(1.)
                    .rounded(theme::size::RADIUS_ROW)
                    .border_1()
                    .border_color(if current { stroke } else { gpui_kit::transparent_black() })
                    .text_size(theme::size::SM)
                    .text_color(if selected { t.foreground } else { t.muted_foreground })
                    // Active pane's selected tab is glass + stroke; passive pane's selected tab sits quietly in accent.
                    .when(current, move |d| d.bg(glass))
                    .when(passive_selected, move |d| d.bg(accent))
                    .when(!selected, {
                        let foreground = t.foreground;
                        move |d| {
                            d.hover(move |s| s.bg(accent).text_color(foreground))
                                .active(move |s| s.bg(pressed))
                        }
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
                                .map(|bar| {
                                    if rtl {
                                        bar.right(space(1.5))
                                    } else {
                                        bar.left(space(1.5))
                                    }
                                })
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
                    // Where a dragged tab would land: a line above this one, or below the last.
                    .when_some(mark, |d, edge| {
                        d.child(
                            div()
                                .absolute()
                                .left(space(1.))
                                .right(space(1.))
                                .h(space(0.5))
                                .rounded_full()
                                .bg(primary)
                                .map(|m| {
                                    if edge == Edge::Start {
                                        m.top(px(-2.))
                                    } else {
                                        m.bottom(px(-2.))
                                    }
                                }),
                        )
                    })
                    // The tab with the keyboard's pane: the glint at its start edge.
                    .when(current, move |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(rems(0.4375))
                                .bottom(rems(0.4375))
                                .w(px(3.))
                                .rounded_full()
                                .bg(glint)
                                .map(|bar| if rtl { bar.right(px(-1.)) } else { bar.left(px(-1.)) }),
                        )
                    })
                    .child(glyph)
                    .child(div().flex_1().min_w_0().truncate().child(d.shown.clone()))
                    .children(close);
                if cx.reduce_motion() {
                    fill.into_any_element()
                } else {
                    // A tab that has just been made slides into its place from the start edge.
                    fill.with_animation(
                        SharedString::from(format!("tab-enter-{}", id.0)),
                        Animation::new(theme::motion::BASE).with_easing(theme::motion::enter),
                        move |fill, delta| {
                            let away = px(10. * (1. - delta));
                            let dx = if rtl { away } else { -away };
                            fill.opacity(0.35 + 0.65 * delta).paint_offset(dx, px(0.))
                        },
                    )
                    .into_any_element()
                }
            })
            // Hop mode (Alt+J): a tab is a target, and a hint on it shows it, as a press does.
            .map(|tab| {
                crate::components::hop::mark(tab, || {
                    let explorer = self.explorer.clone();
                    crate::components::hop::Mark::new(
                        crate::components::hop::Kind::Tab,
                        pane as u64,
                        d.name.clone(),
                        move |_, cx| explorer.update(cx, |e, cx| e.activate_tab(id, cx)),
                    )
                })
            })
            .track_focus(&self.focus[&id])
            .tab_stop(true)
            .focus_visible(move |s| s.rounded(theme::size::RADIUS_ROW).border_1().border_color(ring))
            .when(peek_target.is_none(), |tab| tab.tooltip(menu::tip(tip.clone())))
            .when_some(peek_target, |tab, target| {
                tab.on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    let target = target.clone();
                    this.peek.update(cx, |peek, cx| {
                        if *hovered {
                            peek.enter(target, window, cx);
                        } else {
                            peek.leave(&target.element, cx);
                        }
                    });
                }))
            })
            // Pressing a tab shows it, as the TypeScript's mousedown did; the keyboard focus stays where it was.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    this.peek.update(cx, |peek, cx| peek.hide(cx));
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
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    this.peek.update(cx, |peek, cx| peek.hide(cx));
                    this.open_tab_menu(id, event.position, window, cx);
                }),
            )
    }
}

impl Render for TabStrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = crate::perf::span("tab strip");
        let t = theme::tokens(cx).clone();
        let panes = self.collect(cx);
        let dual = panes.len() > 1;
        let dragging = cx.has_active_drag();
        let rtl = i18n::is_rtl();
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
            // Another tab became active: keep it in view.
            if self.scrolled_to[index] != Some(pane.active) {
                self.scrolled_to[index] = Some(pane.active);
                if let Some(at) = pane.tabs.iter().position(|d| d.id == pane.active) {
                    scroll.scroll_to_item(at);
                }
            }
            let tabs: Vec<AnyElement> = pane
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
                        &t,
                        cx,
                    )
                    .into_any_element()
                })
                .collect();
            let list_label = if !dual {
                t!("Tabs")
            } else if (index == 0) != rtl {
                t!("Left pane tabs")
            } else {
                t!("Right pane tabs")
            };
            let drop_scroll = scroll.clone();
            let list = div()
                .id(SharedString::from(format!("tablist-{index}")))
                .test_support()
                .role(Role::TabList)
                .aria_label(list_label)
                .flex()
                .flex_col()
                .w_full()
                // As tall as its tabs, up to a few; then it scrolls.
                .max_h(TAB_HEIGHT * MOST_SHOWN)
                .overflow_y_scroll()
                .track_scroll(&scroll)
                // A tab dragged over the list: the gap it would take.
                .on_drag_move(cx.listener(move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                    let over = event.bounds.contains(&event.event.position);
                    let new = if over {
                        let bounds: Vec<Bounds<Pixels>> =
                            (0..count).filter_map(|i| drop_scroll.bounds_for_item(i)).collect();
                        Some(Slot {
                            pane: index,
                            index: slot_at(event.event.position.y, &bounds),
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
            let list = div()
                .relative()
                .w_full()
                .when(crate::components::hop::active(), |wrap| {
                    wrap.child(
                        canvas(
                            move |bounds, window, cx| {
                                crate::components::hop::clip_tabs(index as u64, bounds, window, cx);
                            },
                            |_, _, _, _| (),
                        )
                        .absolute()
                        .inset_0(),
                    )
                })
                .child(list)
                .when(count as f32 > MOST_SHOWN, |wrap| {
                    wrap.child(Scrollbar::vertical(&scroll).viewport_from_layout())
                });

            let new_tab = Button::new(SharedString::from(format!("new-tab-{index}")))
                .ghost()
                .xsmall()
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
                let menu_bounds = self.pane_menu_bounds[index].clone();
                let click_bounds = menu_bounds.clone();
                let measure = canvas(move |b, _, _| menu_bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .inset_0();
                let btn = Button::new(SharedString::from(format!("pane-menu-{index}")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Columns2)
                    .accessibility_label(t!("Pane menu"))
                    .tooltip(t!("This pane: swap, show a folder in the other pane, even out, close"))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        let pos = event.position();
                        let at = if pos.x == px(0.) && pos.y == px(0.) {
                            let b = click_bounds.get();
                            if b.size.width > px(0.) && b.size.height > px(0.) {
                                point(if rtl { b.right() } else { b.left() }, b.bottom() + px(2.))
                            } else {
                                pos
                            }
                        } else {
                            pos
                        };
                        this.open_pane_menu(index, at, window, cx);
                    }));
                div().relative().flex_none().child(measure).child(btn)
            });
            // What the list is, over it: "Tabs" (or which pane's), then its buttons at the end edge. A tab dropped
            // on the header goes to the end of the list, and a double-click on it makes a tab.
            let heading: SharedString = if !dual {
                t!("Open")
            } else if (index == 0) != rtl {
                t!("Left pane")
            } else {
                t!("Right pane")
            };
            let header_group = SharedString::from(format!("tab-section-{index}"));
            let header = row_start()
                .id(SharedString::from(format!("tab-header-{index}")))
                .w_full()
                .h(space(7.))
                .ps(2.)
                .pe(0.5)
                .gap(space(0.5))
                .text_size(theme::size::XS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(if pane.current && dual {
                    t.foreground
                } else {
                    t.muted_foreground
                })
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    this.explorer.update(cx, |e, cx| {
                        e.focus_pane(index, cx);
                        if event.click_count() == 2 {
                            e.new_tab(None, cx);
                        }
                    });
                }))
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
                }))
                .child(div().flex_1().min_w_0().truncate().child(heading))
                .children(pane_button)
                // The + shows while the section is under the pointer (or a tab is being dragged over it).
                .child(
                    div()
                        .id(SharedString::from(format!("new-tab-wrap-{index}")))
                        .flex_none()
                        .track_focus(&self.plus_focus[index])
                        .opacity(if dragging { 1. } else { 0. })
                        .group_hover(header_group.clone(), |s| s.opacity(1.))
                        // Never an invisible button with the keyboard on it.
                        .in_focus(|s| s.opacity(1.))
                        .child(new_tab),
                );
            strips.push(
                col()
                    .id(SharedString::from(format!("tab-strip-{index}")))
                    .group(header_group)
                    .w_full()
                    .child(header)
                    .child(list),
            );
        }

        let menu = self.popup.as_ref().map(menu::Popup::element);
        col()
            .id("tab-strip")
            .w_full()
            .flex_none()
            .gap(space(1.))
            // Where a tab's drag began, and where it ended if that was outside the lists.
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
            .child(self.peek.clone())
    }
}

// ---- The menus --------------------------------------------------------------------------------------

/// The key an action has, as menus word it ("Ctrl+W"), for a row that does the same.
fn key_of(action: &dyn gpui_kit::Action) -> Option<SharedString> {
    crate::actions::BINDINGS
        .iter()
        .find(|binding| action.partial_eq(&*(binding.action)()))
        .map(|binding| crate::components::menus::key_text(binding.keys).into())
}

/// The tab's menu (`tabMenu`): duplicate, show beside, move, close, close the others, reopen. Built when it
/// opens, so what can't apply is left out, and what can't now (the last tab's close) is grey.
fn tab_menu(e: &Explorer, id: TabId) -> Vec<Node> {
    let dual = e.is_dual();
    let pane = e.pane_of(id);
    let tabs = pane.and_then(|p| e.panes().get(p)).map_or(0, |p| p.tabs.len());
    let has_path = e.tab(id).is_some();
    let arg = id.0.to_string();
    let side = if dual {
        t!("Open in the other pane")
    } else {
        t!("Open side by side")
    };
    let mut close = menu::row(t!("Close tab"), IconName::X, "close", arg.clone()).disabled(tabs < 2 && !dual);
    close.hint = key_of(&crate::actions::CloseTab);
    let mut reopen =
        menu::row(t!("Reopen closed tab"), IconName::RotateCcw, "reopen", arg.clone()).disabled(!e.can_reopen_tab());
    reopen.hint = key_of(&crate::actions::ReopenClosedTab);
    vec![
        menu::row(t!("Duplicate tab"), IconName::Copy, "duplicate", arg.clone()).node(),
        has_path
            .then(|| menu::row(side, IconName::Columns2, "side", arg.clone()))
            .and_then(Entry::node),
        // Moving to the other pane is for two panes (and a pane with another tab to keep).
        dual.then(|| {
            menu::row(
                t!("Move to the other pane"),
                IconName::ArrowRightLeft,
                "move",
                arg.clone(),
            )
            .disabled(tabs < 2)
        })
        .and_then(Entry::node),
        Some(Node::Separator),
        close.node(),
        // Closing the others needs others.
        (tabs > 1)
            .then(|| menu::row(t!("Close other tabs"), IconName::X, "others", arg.clone()))
            .and_then(Entry::node),
        reopen.node(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// A pane's menu (`paneMenu`): swap, show this folder in the other pane, bring the other pane's folder
/// here, even out the panes, close the pane.
fn pane_menu(e: &Explorer, pane: usize, even: bool) -> Vec<Node> {
    let other = 1 - pane.min(1);
    let folder = |of: usize| {
        e.pane_tab(of)
            .map(|t| t.path.as_str())
            .filter(|p| *p != THIS_PC)
            .is_some()
    };
    vec![
        menu::row(t!("Swap panes"), IconName::ArrowLeftRight, "swap", "").node(),
        folder(pane)
            .then(|| {
                menu::row(
                    t!("Show this folder in the other pane"),
                    IconName::Columns2,
                    "show",
                    pane.to_string(),
                )
            })
            .and_then(Entry::node),
        folder(other)
            .then(|| {
                menu::row(
                    t!("Open the other pane's folder here"),
                    IconName::FolderInput,
                    "show",
                    other.to_string(),
                )
            })
            .and_then(Entry::node),
        Some(Node::Separator),
        // Already even: nothing to do, so no row.
        (!even)
            .then(|| menu::row(t!("Even out the panes"), IconName::Columns2, "even", ""))
            .and_then(Entry::node),
        menu::row(t!("Close this pane"), IconName::X, "close-pane", pane.to_string()).node(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) mod menu;

#[cfg(test)]
mod tests;
