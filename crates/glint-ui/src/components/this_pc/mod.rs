//! This PC: the known folders, then every drive with its space, health and quick actions.
//! Ported from src/components/explorer/ThisPC.tsx.
//!
//! What the TypeScript had, and where it is here:
//!
//! | TypeScript                                  | Here                                                           |
//! |---------------------------------------------|----------------------------------------------------------------|
//! | `ThisPC` (the root view, one per pane)      | [`ThisPc`], shown while the pane's tab is This PC               |
//! | `useExplorer` drives and known folders      | the explorer model (`explorer.drives()`, `known_folders()`)     |
//! | known-folder tiles (`KNOWN_ICONS`)          | [`KnownTile`]s, icons from `kit::known_icon`                    |
//! | `DriveTile` and `driveName`                 | [`DriveTile`], titles in [`drive_title`]                        |
//! | `useDriveHealth` and the banner             | `dialogs::drive_health` (the badge, the banner, the dialog)     |
//! | `CapacityBar`, `freeOf`, `DRIVE_ICONS`      | `kit::capacity_bar`, `kit::free_of`, `kit::drive_icon`          |
//! | `openDriveTreemap`, `inspectTimeline`, `inspectDuplicates` | [`ThisPcEvent`]s: the windows that own them show them |
//! | `ejectDrive` and `refreshDrives`            | [`ThisPc::eject`], which refreshes the drives when it worked    |
//! | the total's pill                            | the totals line above the drives (`model::totals`)              |
//!
//! **Keyboard.** Every tile is a tab stop. Tab and Shift+Tab move between them; the arrows move one tile to
//! the next or previous (reversed in Arabic) and up or down a row of its grid; Home and End go to the first
//! and last tile; Enter and Space open the tile. A tile's own buttons (eject, map, recent changes, duplicates)
//! are tab stops of their own, and their Enter and Space are theirs.
//!
//! **Mouse.** A click opens the folder or drive in this pane; a middle click opens it in a new tab; a right
//! click emits [`ThisPcEvent::ContextMenu`] for the window's menu.
//!
//! **What isn't here yet** (each marked `PORT ME` where it belongs):
//! - The instant search card above the folders (the TypeScript's `InstantSearchCard`). It needs the index
//!   state, which the explorer model doesn't hold yet.
//! - Drops onto the tiles (the TypeScript's `data-drop` targets and their highlight): the drop part.
//! - The disk-usage map, recent changes and duplicates: their windows come with their own parts; this view
//!   only says which drive was asked for.

mod model;
#[cfg(test)]
mod tests;

use crate::core_bridge::{self, Core};
use crate::dialogs::drive_health::{self, DriveHealthBadge, DriveHealthBanner, Health};
use crate::explorer::{Drive, DriveKind, ExplorerEvent, KnownId, NavOptions, TabId};
use crate::kit::prelude::*;
use crate::{i18n, t, theme, workspace};
use glint_core::drivehealth::DriveHealth;
use glint_model::format::format_size;
use gpui_kit::rems;
use model::Step;
use std::cell::Cell;
use std::rc::Rc;

/// What This PC asks the window to do: the menus and windows that belong to other parts of the port.
#[derive(Clone, Debug)]
pub enum ThisPcEvent {
    /// A right click on a folder or drive tile: the window's menu for `path`, at `position` in the window.
    ContextMenu { path: String, position: Point<Pixels> },
    /// "Recent changes" on a drive: its timeline (`inspectTimeline`).
    RecentChanges { path: String },
    /// "Find duplicate files" on a drive (`inspectDuplicates`).
    FindDuplicates { path: String },
    /// "Map disk usage" on a drive: the drive was opened in `tab`; show that tab's usage map, unless it's on.
    DiskUsage { tab: TabId, path: String },
}

impl EventEmitter<ThisPcEvent> for ThisPc {}

/// A known folder's tile: Home, Desktop, Documents…
struct KnownTile {
    id: KnownId,
    name: SharedString,
    path: String,
    focus: FocusHandle,
}

/// A drive's tile, with what its numbers say. Built when the drives or the health change, not per frame.
struct DriveTile {
    path: String,
    kind: DriveKind,
    title: SharedString,
    /// The free space and size as text, or the path for a WSL distribution, or the file system, or "Not ready".
    subtitle: SharedString,
    /// The file system, shown as a badge (not for WSL).
    fs: String,
    total: u64,
    free: u64,
    percent: u32,
    health: Option<DriveHealth>,
    focus: FocusHandle,
}

impl DriveTile {
    fn new(drive: &Drive, health: Option<DriveHealth>, focus: FocusHandle) -> Self {
        let wsl = drive.kind == DriveKind::Wsl;
        let subtitle: SharedString = if wsl {
            drive.path.clone().into()
        } else {
            let sized = free_of(drive.total, drive.free);
            if !sized.is_empty() {
                sized
            } else if !drive.fs.is_empty() {
                drive.fs.clone().into()
            } else {
                t!("Not ready")
            }
        };
        DriveTile {
            path: drive.path.clone(),
            kind: drive.kind,
            title: drive_title(drive).into(),
            subtitle,
            fs: drive.fs.clone(),
            total: drive.total,
            free: drive.free,
            percent: model::used_percent(drive.total, drive.free),
            health,
            focus,
        }
    }
}

/// This PC's view of the explorer. One per pane: the pane shows This PC while its tab is This PC.
pub struct ThisPc {
    /// The pane (0 or 1) this view is in: its tab is where clicks navigate.
    pane: usize,
    known: Vec<KnownTile>,
    drives: Vec<DriveTile>,
    /// Disks whose health isn't good, for the banner.
    failing: Vec<DriveHealth>,
    totals: Option<model::Totals>,
    /// The width of the content, measured while painting: the arrow keys need the columns of each grid.
    width: Rc<Cell<Pixels>>,
    health: Entity<Health>,
    _subscriptions: Vec<Subscription>,
}

impl ThisPc {
    /// This PC for `pane`. It reads the drives and the health the explorer and the drive health model already
    /// hold, and asks the drives for their health if the reading is stale (the TypeScript's `useDriveHealth`).
    pub fn new(pane: usize, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = crate::explorer::explorer(cx);
        let health = drive_health::shared(cx);
        let subscriptions = vec![
            cx.observe(&explorer, |_, _, cx| cx.notify()),
            cx.subscribe(&explorer, |this, _, event: &ExplorerEvent, cx| {
                if matches!(event, ExplorerEvent::DrivesChanged | ExplorerEvent::KnownFoldersChanged) {
                    this.rebuild(cx);
                }
            }),
            cx.observe(&health, |this, _, cx| this.rebuild(cx)),
        ];
        health.update(cx, |health, cx| health.refresh(false, cx));
        let mut this = ThisPc {
            pane,
            known: Vec::new(),
            drives: Vec::new(),
            failing: Vec::new(),
            totals: None,
            width: Rc::new(Cell::new(px(0.))),
            health,
            _subscriptions: subscriptions,
        };
        this.rebuild(cx);
        this
    }

    /// Makes the tiles from the explorer's folders and drives and the health's readings. Tiles keep their focus
    /// handles across a rebuild, so the keyboard stays where it was when a reading arrives.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let (known, drives) = {
            let explorer = crate::explorer::explorer(cx);
            let explorer = explorer.read(cx);
            (explorer.known_folders().clone(), explorer.drives().clone())
        };
        let health = self.health.read(cx);
        let failing: Vec<DriveHealth> = health.failing().into_iter().cloned().collect();

        let old_known: Vec<FocusHandle> = self.known.iter().map(|k| k.focus.clone()).collect();
        let known_tiles: Vec<KnownTile> = known
            .iter()
            .enumerate()
            .map(|(i, k)| KnownTile {
                id: k.id,
                name: known_name(k.id),
                path: k.path.clone(),
                focus: old_known
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| cx.focus_handle().tab_stop(true)),
            })
            .collect();
        let old_drives: Vec<FocusHandle> = self.drives.iter().map(|d| d.focus.clone()).collect();
        let drive_tiles: Vec<DriveTile> = drives
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let focus = old_drives
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| cx.focus_handle().tab_stop(true));
                DriveTile::new(d, health.for_drive(&d.path).cloned(), focus)
            })
            .collect();

        self.known = known_tiles;
        self.drives = drive_tiles;
        self.failing = failing;
        self.totals = model::totals(&drives);
        cx.notify();
    }

    /// Every tile's focus handle, folders first.
    fn focus_handles(&self) -> impl Iterator<Item = &FocusHandle> {
        self.known
            .iter()
            .map(|k| &k.focus)
            .chain(self.drives.iter().map(|d| &d.focus))
    }

    /// The tile that has the keyboard, by its index (folders first).
    fn focused_tile(&self, window: &Window) -> Option<usize> {
        self.focus_handles().position(|focus| focus.is_focused(window))
    }

    /// The focus handle of the tile at `index` (folders first).
    fn focus_at(&self, index: usize) -> Option<&FocusHandle> {
        let known = self.known.len();
        if index < known {
            self.known.get(index).map(|k| &k.focus)
        } else {
            self.drives.get(index - known).map(|d| &d.focus)
        }
    }

    /// The path a tile opens, by its index (folders first).
    fn path_at(&self, index: usize) -> Option<String> {
        let known = self.known.len();
        if index < known {
            self.known.get(index).map(|k| k.path.clone())
        } else {
            self.drives.get(index - known).map(|d| d.path.clone())
        }
    }

    /// The number of columns of the folders' grid and of the drives' grid, at the width last painted.
    fn columns(&self, window: &Window) -> (usize, usize) {
        let rem = f32::from(window.rem_size());
        let width = f32::from(self.width.get());
        (
            model::grid_columns(width, model::FOLDER_TILE_REM * rem, model::FOLDER_GAP_REM * rem),
            model::grid_columns(width, model::DRIVE_TILE_REM * rem, model::DRIVE_GAP_REM * rem),
        )
    }

    /// Opens a folder or drive in this pane (`navigate`): the pane is made the active one first, as a click in
    /// it does.
    fn open(&self, path: &str, cx: &mut Context<Self>) {
        let pane = self.pane;
        let explorer = crate::explorer::explorer(cx);
        explorer.update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            let tab = explorer.pane_tab_id(pane);
            explorer.navigate(path, NavOptions::in_tab(tab), cx);
        });
    }

    /// A middle click: the folder opens in a new tab beside this pane's (`openInNewTab`).
    fn open_in_new_tab(&self, path: &str, cx: &mut Context<Self>) {
        let pane = self.pane;
        let explorer = crate::explorer::explorer(cx);
        explorer.update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            explorer.open_in_new_tab(path, cx);
        });
    }

    /// "Map disk usage": opens the drive in this pane's tab, then asks the window to show its usage map.
    fn map_usage(&self, path: String, cx: &mut Context<Self>) {
        let pane = self.pane;
        let explorer = crate::explorer::explorer(cx);
        let tab = explorer.update(cx, |explorer, cx| {
            explorer.focus_pane(pane, cx);
            let tab = explorer.pane_tab_id(pane);
            explorer.navigate(path.clone(), NavOptions::in_tab(tab), cx);
            tab
        });
        cx.emit(ThisPcEvent::DiskUsage { tab, path });
    }

    /// "Safely eject": asks Windows to eject the drive. On success the drives are read again and a notice says
    /// so; on failure the notice says why (`ejectDrive` with its toasts).
    fn eject(&mut self, path: String, name: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let app = cx.global::<Core>().app.clone();
        let running = core_bridge::run(async move {
            // The shell asks for the window that owns it: the main window, which core knows by its label.
            use glint_core::host::Manager as _;
            let Some(core_window) = app.get_webview_window(workspace::MAIN) else {
                return Err("Glint has no window to eject from".to_string());
            };
            glint_core::shell::eject_drive(core_window, path).await
        });
        let explorer = crate::explorer::explorer(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = running.await.and_then(|inner| inner);
            let ejected = result.is_ok();
            let _ = this.update_in(cx, |_, window, cx| match result {
                Ok(()) => window.push_notification(
                    Notification::success(t!("Safely ejected {name}", name = name.clone())),
                    cx,
                ),
                Err(error) => window.push_notification(
                    Notification::error(error).title(t!("Couldn't eject {name}", name = name.clone())),
                    cx,
                ),
            });
            if ejected {
                // The drives are read again, so the ejected one leaves the list.
                explorer.update(cx, |explorer, cx| explorer.refresh_drives(cx).detach());
            }
        })
        .detach();
    }

    /// Keys on the tiles: the arrows move, Home and End jump, Enter and Space open the tile under the focus.
    /// Keys with a modifier are the shortcuts' (Ctrl+T and the like), so they pass by.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let Some(current) = self.focused_tile(window) else {
            return;
        };
        let rtl = i18n::is_rtl();
        let step = match event.keystroke.key.as_str() {
            // "Next" is the arrow that points along the reading direction, so it swaps in Arabic.
            "right" if rtl => Step::Previous,
            "right" => Step::Next,
            "left" if rtl => Step::Next,
            "left" => Step::Previous,
            "up" => Step::Up,
            "down" => Step::Down,
            "home" => Step::First,
            "end" => Step::Last,
            "enter" | "space" => {
                if let Some(path) = self.path_at(current) {
                    self.open(&path, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        let (columns_known, columns_drives) = self.columns(window);
        let target = model::step(
            current,
            step,
            self.known.len(),
            self.drives.len(),
            columns_known,
            columns_drives,
        );
        if let Some(target) = target {
            if let Some(focus) = self.focus_at(target) {
                focus.focus(window, cx);
            }
            cx.stop_propagation();
        }
    }

    /// A folder tile: a click opens it, a middle click opens it in a new tab, a right click asks for the menu.
    fn render_folder(&self, index: usize, tile: &KnownTile, t: &theme::Tokens, cx: &Context<Self>) -> impl IntoElement {
        let open_path = tile.path.clone();
        let middle_path = tile.path.clone();
        let menu_path = tile.path.clone();
        let (primary, accent, ring, border, card) = (t.primary, t.accent, t.ring, t.border, t.card);
        // The folder's icon: a tinted square, as the TypeScript's.
        let glyph = div()
            .flex()
            .items_center()
            .justify_center()
            .w(space(8.))
            .h(space(8.))
            .flex_none()
            .rounded(theme::size::RADIUS)
            .bg(primary.opacity(0.1))
            .text_color(primary)
            .child(icon(known_icon(tile.id)).size_4());

        div()
            .id(("this-pc-folder", index))
            .role(Role::Button)
            .aria_label(tile.name.clone())
            .aria_description(SharedString::from(tile.path.clone()))
            .test_support()
            .track_focus(&tile.focus)
            .focus(|style| style.border_color(ring))
            .flex()
            .items_center()
            .gap(space(3.))
            .w(rems(model::FOLDER_TILE_REM))
            .min_w_0()
            .flex_grow(1.)
            .flex_shrink(1.)
            .p(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(border)
            .bg(card)
            .cursor_pointer()
            .hover(|style| style.border_color(primary.opacity(0.4)).bg(accent.opacity(0.6)))
            // PORT ME(drop): a file dragged over a folder tile highlights it and opens it on a drop (data-drop).
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open(&open_path, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.open_in_new_tab(&middle_path, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                    cx.emit(ThisPcEvent::ContextMenu {
                        path: menu_path.clone(),
                        position: event.position,
                    })
                }),
            )
            .child(glyph)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .child(tile.name.clone()),
            )
    }

    /// A drive tile: its icon, name, file system, health, used share, capacity bar, free space and quick actions.
    fn render_drive(&self, index: usize, tile: &DriveTile, t: &theme::Tokens, cx: &Context<Self>) -> impl IntoElement {
        let open_path = tile.path.clone();
        let middle_path = tile.path.clone();
        let menu_path = tile.path.clone();
        let (primary, accent, ring, border, card, muted, muted_fg, destructive, warning, foreground) = (
            t.primary,
            t.accent,
            t.ring,
            t.border,
            t.card,
            t.muted,
            t.muted_foreground,
            t.destructive,
            t.warning,
            t.foreground,
        );
        let nearly = model::nearly_full(tile.percent);
        let tone = if nearly { destructive } else { muted_fg };

        let glyph = div()
            .mt(space(0.5))
            .flex()
            .items_center()
            .justify_center()
            .w(space(9.))
            .h(space(9.))
            .flex_none()
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(if nearly { destructive.opacity(0.4) } else { border })
            .bg(if nearly {
                destructive.opacity(0.1)
            } else {
                muted.opacity(0.4)
            })
            .text_color(tone)
            .child(icon(drive_icon(tile.kind)).size_5());

        // The name line: the name, the badges (WSL, file system, health), and the used share at the end.
        let linux = (tile.kind == DriveKind::Wsl).then(|| {
            div()
                .flex_none()
                .rounded(theme::size::RADIUS_SM)
                .px(space(1.5))
                .bg(warning.opacity(0.15))
                .text_color(warning)
                .text_size(theme::size::XXS)
                .child(t!("Linux"))
        });
        let file_system = (tile.kind != DriveKind::Wsl && !tile.fs.is_empty()).then(|| {
            div()
                .flex_none()
                .rounded(theme::size::RADIUS_SM)
                .px(space(1.5))
                .bg(muted)
                .text_color(muted_fg)
                .text_size(theme::size::XXS)
                .child(SharedString::from(tile.fs.clone()))
        });
        let health = tile
            .health
            .clone()
            .map(|reading| DriveHealthBadge::new(reading, open_health));
        let used = (tile.total > 0).then(|| {
            div()
                .flex_none()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(theme::size::XXS)
                .text_color(tone)
                .child(SharedString::from(format!("{}%", tile.percent)))
        });
        let name_line = row_start()
            .w_full()
            .items_center()
            .justify_between()
            .gap(space(1.5))
            .child(
                row_start()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(space(1.5))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::size::SM)
                            .font_weight(FontWeight::MEDIUM)
                            .child(tile.title.clone()),
                    )
                    .children(linux)
                    .children(file_system)
                    .children(health),
            )
            .children(used);

        let actions = model::has_actions(tile.kind, tile.total).then(|| self.tile_actions(index, tile, cx));

        let subtitle = div()
            .min_w_0()
            .truncate()
            .text_size(theme::size::XS)
            .text_color(muted_fg)
            .child(tile.subtitle.clone());

        div()
            .id(("this-pc-drive", index))
            .role(Role::Button)
            .aria_label(tile.title.clone())
            .aria_description(tile.subtitle.clone())
            .test_support()
            .track_focus(&tile.focus)
            .focus(|style| style.border_color(ring))
            .flex()
            .items_start()
            .gap(space(3.))
            .w(rems(model::DRIVE_TILE_REM))
            .min_w_0()
            .flex_grow(1.)
            .flex_shrink(1.)
            .p(space(3.))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(border)
            .bg(card)
            .cursor_pointer()
            .hover(|style| style.border_color(primary.opacity(0.4)).bg(accent.opacity(0.6)))
            // PORT ME(drop): a file dragged over a drive tile highlights it and opens it on a drop (data-drop).
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.open(&open_path, cx)))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.open_in_new_tab(&middle_path, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, event: &MouseDownEvent, _, cx| {
                    cx.emit(ThisPcEvent::ContextMenu {
                        path: menu_path.clone(),
                        position: event.position,
                    })
                }),
            )
            .child(glyph)
            .child(
                col()
                    .flex_1()
                    .min_w_0()
                    .align_start()
                    .text_start()
                    .child(name_line)
                    .child(
                        div()
                            .my(space(2.))
                            .w_full()
                            .children(capacity_bar(tile.total, tile.free, space(1.5), cx)),
                    )
                    .child(
                        row_start()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap(space(2.))
                            .child(subtitle)
                            .children(actions),
                    ),
            )
            .text_color(foreground)
    }

    /// The quick actions of a drive: eject (removable and disc drives), then map, recent changes and duplicates
    /// (drives with a size). Each is its own tab stop, and a click on one doesn't open the drive.
    fn tile_actions(&self, index: usize, tile: &DriveTile, cx: &Context<Self>) -> impl IntoElement {
        let name = tile.title.clone();
        let eject_path = tile.path.clone();
        let eject_name = name.clone();
        let usage_path = tile.path.clone();
        let recent_path = tile.path.clone();
        let duplicates_path = tile.path.clone();

        let eject = model::is_ejectable(tile.kind).then(|| {
            let label = t!("Safely eject {name}", name = name.clone());
            Button::new(("this-pc-eject", index))
                .ghost()
                .xsmall()
                .icon(IconName::Unplug)
                .accessibility_label(label.clone())
                .tooltip(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.eject(eject_path.clone(), eject_name.clone(), window, cx);
                }))
        });
        let (map, recent, duplicates) = if tile.total > 0 {
            let map = Button::new(("this-pc-map", index))
                .ghost()
                .xsmall()
                .icon(IconName::LayoutDashboard)
                .accessibility_label(t!("Map disk usage"))
                .tooltip(t!("Map disk usage"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.map_usage(usage_path.clone(), cx);
                }));
            let recent = Button::new(("this-pc-recent", index))
                .ghost()
                .xsmall()
                .icon(IconName::FileClock)
                .accessibility_label(t!("Recent changes"))
                .tooltip(t!("Recent changes"))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(ThisPcEvent::RecentChanges {
                        path: recent_path.clone(),
                    });
                }));
            let duplicates = Button::new(("this-pc-duplicates", index))
                .ghost()
                .xsmall()
                .icon(IconName::Files)
                .accessibility_label(t!("Find duplicate files"))
                .tooltip(t!("Find duplicate files"))
                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(ThisPcEvent::FindDuplicates {
                        path: duplicates_path.clone(),
                    });
                }));
            (Some(map), Some(recent), Some(duplicates))
        } else {
            (None, None, None)
        };

        row_start()
            .flex_none()
            .items_center()
            .gap(space(0.5))
            .children(eject)
            .children(map)
            .children(recent)
            .children(duplicates)
    }
}

impl Render for ThisPc {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        // The view belongs to a pane whose tab is This PC; when that tab shows a folder, nothing is drawn here.
        let shown = crate::explorer::explorer(cx)
            .read(cx)
            .pane_tab(self.pane)
            .is_some_and(|tab| tab.is_this_pc());
        if !shown {
            return div().size_full().into_any_element();
        }

        // Measured at paint, so the arrow keys know how many tiles a row holds.
        let width = self.width.clone();
        let measure = gpui_kit::canvas(move |bounds, _, _| width.set(bounds.size.width), |_, _, _, _| {})
            .w_full()
            .h(px(0.));

        let folders = self
            .known
            .iter()
            .enumerate()
            .map(|(i, tile)| self.render_folder(i, tile, &t, cx));
        let folder_grid = row_start()
            .id("this-pc-folders")
            .role(Role::Group)
            .aria_label(t!("Folders"))
            .test_support()
            .flex_wrap()
            .gap(space(2.))
            .children(folders);

        let totals = self.totals.map(|totals| {
            let sentence = t!(
                "Total: {used} used of {capacity}",
                used = format_size(totals.used),
                capacity = format_size(totals.capacity)
            );
            let free = t!("{free} free", free = format_size(totals.free));
            let percent = format!("{}%", totals.percent);
            let line = format!("{sentence} · {free} · {percent}");
            row_start()
                .id("this-pc-totals")
                .items_center()
                .gap(space(2.5))
                .whitespace_nowrap()
                .role(Role::Group)
                .aria_label(SharedString::from(line))
                .test_support()
                .rounded_full()
                .border_1()
                .border_color(t.border)
                .bg(t.muted.opacity(0.3))
                .px(space(3.))
                .py(space(1.))
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(sentence)
                .child(div().text_color(t.border).child("·"))
                .child(div().text_color(t.success).child(free))
                .child(
                    div()
                        .rounded(theme::size::RADIUS_SM)
                        .bg(t.primary.opacity(0.15))
                        .px(space(1.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.primary)
                        .child(percent),
                )
        });
        let devices_header = row_start()
            .flex_wrap()
            .justify_between()
            .items_center()
            .gap(space(2.))
            .mt(space(8.))
            .mb(space(3.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(theme::size::SM)
                    .child(t!("Devices and drives")),
            )
            .children(totals);

        let drives = self
            .drives
            .iter()
            .enumerate()
            .map(|(i, tile)| self.render_drive(self.known.len() + i, tile, &t, cx));
        let drive_grid = row_start()
            .id("this-pc-drives")
            .role(Role::Group)
            .aria_label(t!("Devices and drives"))
            .test_support()
            .flex_wrap()
            .gap(space(3.))
            .children(drives);

        let body = col()
            .w_full()
            .px(space(6.))
            .py(space(5.))
            .child(measure)
            .child(
                div()
                    .mb(space(2.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(theme::size::SM)
                    .child(t!("Folders")),
            )
            .child(folder_grid)
            // PORT ME(coordinator): the instant search card (InstantSearchCard) goes here: the index state
            // (glint-core's index_status and its volumes) and Turn on / Retry, which the explorer model doesn't hold yet.
            .child(DriveHealthBanner::new(self.failing.clone(), open_health))
            .child(devices_header)
            .child(drive_grid);

        div()
            .id("this-pc")
            .role(Role::Group)
            .aria_label(t!("This PC"))
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .text_start()
            .text_size(theme::size::SM)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| this.on_key(event, window, cx)))
            .child(body)
            .into_any_element()
    }
}

/// Opens the drive-health dialog for one disk (a badge or a banner entry was clicked).
fn open_health(disk: u32, window: &mut Window, cx: &mut App) {
    drive_health::open(window, cx, Some(disk));
}

/// A known folder's name as the sidebar shows it: translated (the explorer keeps the English).
fn known_name(id: KnownId) -> SharedString {
    match id {
        KnownId::Home => t!("Home"),
        KnownId::Desktop => t!("Desktop"),
        KnownId::Documents => t!("Documents"),
        KnownId::Downloads => t!("Downloads"),
        KnownId::Pictures => t!("Pictures"),
        KnownId::Music => t!("Music"),
        KnownId::Videos => t!("Videos"),
    }
}

/// What a drive without a label is called, translated (`DEFAULT_LABEL` in fs.ts).
fn default_label(kind: DriveKind) -> SharedString {
    match kind {
        DriveKind::Fixed => t!("Local Disk"),
        DriveKind::Removable => t!("USB Drive"),
        DriveKind::Network => t!("Network Drive"),
        DriveKind::Cdrom => t!("DVD Drive"),
        DriveKind::Ram => t!("RAM Disk"),
        DriveKind::Wsl => t!("Linux"),
        DriveKind::Unknown => t!("Drive"),
    }
}

/// A drive's name as This PC shows it: "Local Disk (C:)", "Ubuntu (WSL)" (`driveName`), with the default label
/// translated. The explorer's own `drive_name` is English; this is the same shape in the user's language.
pub fn drive_title(drive: &Drive) -> String {
    let label = if drive.label.is_empty() {
        default_label(drive.kind).to_string()
    } else {
        drive.label.clone()
    };
    if drive.kind == DriveKind::Wsl {
        format!("{label} (WSL)")
    } else {
        format!("{label} ({})", model::letters(&drive.path))
    }
}
