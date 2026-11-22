//! The list's header: a button per column that sorts by it, the edges that resize the columns, and the menu
//! on a right-click (`Header`, `ColumnEdge` and `ResizeHandle` in FileList.tsx and ResizeHandle.tsx, the
//! header's menu `onHeaderMenu` in Explorer.tsx).

use super::{layout, FileList, FileListEvent, Frame, Resize, TabView, Track};
use crate::components::menus::{visible_items, Entry, MenuView, MenuViewEvent, Node, Run, Task};
use crate::components::preview;
use crate::kit::prelude::*;
use crate::theme::{self, Tokens};
use crate::{i18n, state, t};
use glint_model::columns::{ColumnId, COLUMN_ORDER, DESCENDING};
use glint_model::entries::{Sort, SortKey};
use glint_model::groups::{GroupBy, GROUP_BYS};
use glint_model::store::Column;
use gpui_kit::{anchored, deferred, Anchor};
use std::time::{Duration, Instant};

/// A press on the header that closed its menu is the same press as the click that would open it again.
const SAME_PRESS: Duration = Duration::from_millis(300);

/// The strip along a column's start edge that takes the drag (`w-2`), and how far it sits outside the column
/// (`-start-2.5`), in spacing units.
const EDGE_WIDTH: f32 = 2.;
const EDGE_OUTSET: f32 = 2.5;

impl FileList {
    /// The header row: the Name button, where each item is (in a list of items from many folders), and a
    /// button for each column, in the widths the rows use.
    pub(super) fn header_row(&self, frame: &Frame, view: &TabView, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let sort = view.sort;
        let mut cells: Vec<AnyElement> = Vec::new();
        for track in &frame.tracks {
            match *track {
                Track::Name => cells.push(
                    self.sort_cell(SortKey::Name, t!("Name"), sort, None, None, t, cx)
                        .flex_1()
                        .min_w_0()
                        .test_support()
                        .into_any_element(),
                ),
                Track::Where => {
                    let label = if view.recycled {
                        t!("Deleted from")
                    } else {
                        t!("Folder")
                    };
                    cells.push(
                        super::cells::where_track(
                            div()
                                .id("file-list-where")
                                .role(Role::ColumnHeader)
                                .aria_label(label.clone()),
                        )
                        .truncate()
                        .child(label)
                        .test_support()
                        .into_any_element(),
                    )
                }
                Track::Column { id, width } => {
                    let def = id.def();
                    // The Recycle Bin's date is when each was deleted; the Gallery's, when each was taken.
                    let label = if id == ColumnId::Modified && view.recycled {
                        t!("Date deleted")
                    } else if id == ColumnId::Modified && view.gallery {
                        t!("Date")
                    } else {
                        i18n::translate(def.label)
                    };
                    cells.push(
                        self.sort_cell(def.sort, label, sort, Some((id, def.width)), Some(width), t, cx)
                            .when(def.end, |this| this.justify_end())
                            .test_support()
                            .into_any_element(),
                    );
                }
            }
        }
        row_start()
            .id("file-list-header")
            .role(Role::Row)
            .aria_label(t!("Columns"))
            .aria_description(t!("Right-click for the columns to show and what to sort by"))
            .test_support()
            .h(rems(2.))
            .flex_none()
            .gap(space(3.))
            .px(space(3.))
            .border_b_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.muted_foreground)
            .children(cells)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.open_header_menu(event.position, window, cx)
                }),
            )
            .into_any_element()
    }

    /// A header cell: a button that sorts by `key`, with the arrow when it is the sort, and the edge that
    /// resizes the column when it is one.
    #[allow(clippy::too_many_arguments)]
    fn sort_cell(
        &self,
        key: SortKey,
        label: SharedString,
        sort: Sort,
        column: Option<(ColumnId, Column)>,
        width: Option<Pixels>,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = sort.key == key;
        let arrow = if sort.desc {
            IconName::ChevronDown
        } else {
            IconName::ChevronUp
        };
        let mut cell = row_start()
            .id(SharedString::from(format!("file-list-sort-{key:?}")))
            .role(Role::Button)
            .aria_label(label.clone())
            .aria_description(t!("Sort by {name}", name = label.to_lowercase()))
            .relative()
            .h_full()
            .gap(space(1.))
            .cursor_pointer()
            .hover(|style| style.text_color(t.foreground))
            .when(active, |this| this.text_color(t.foreground))
            .tooltip({
                let tip = t!("Sort by {name}", name = label.to_lowercase());
                move |window, cx| tooltip(tip.clone(), window, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.sort_by(key, cx)))
            .child(div().min_w_0().truncate().child(isolate(label.clone())))
            .when(active, |this| this.child(icon(arrow).size(rems(0.875)).flex_none()));
        if let Some(width) = width {
            cell = cell.w(width).flex_none();
        }
        if let Some((id, name)) = column {
            cell = cell.child(self.column_edge(id, name, label, t, cx));
        }
        cell
    }

    /// The edge between a column and the one before it: dragged towards the start edge the column widens, a
    /// double-click gives it its usual width back (`ColumnEdge`).
    fn column_edge(
        &self,
        id: ColumnId,
        name: Column,
        label: SharedString,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let edge = div()
            .id(SharedString::from(format!("file-list-edge-{}", id.as_str())))
            .role(Role::Splitter)
            .aria_orientation(Orientation::Vertical)
            .aria_label(t!("{name} column width", name = label))
            .aria_description(t!("double-click: as it was"))
            .tooltip(|window, cx| tooltip(t!("double-click: as it was"), window, cx))
            .absolute()
            .top_0()
            .h_full()
            .w(space(EDGE_WIDTH))
            .cursor_col_resize()
            .hover(|style| style.bg(t.primary.opacity(0.4)));
        let edge = if i18n::is_rtl() {
            edge.right(space(-EDGE_OUTSET))
        } else {
            edge.left(space(-EDGE_OUTSET))
        };
        edge.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                // Not a click on the button it sits on: that would sort.
                cx.stop_propagation();
                if event.click_count >= 2 {
                    state::update(cx, |settings| {
                        layout::set_saved_width(&mut settings.columns, name, None)
                    });
                    return;
                }
                this.start_resize(name, event.position.x, window, cx);
            }),
        )
        .test_support()
    }

    /// A press on the edge: the drag starts from the column's width now.
    fn start_resize(&mut self, name: Column, x: Pixels, window: &mut Window, cx: &mut Context<Self>) {
        let width = self.column_width(name, window, cx);
        self.resizing = Some(Resize {
            name,
            from: x,
            start: f32::from(width),
            now: None,
        });
        cx.notify();
    }

    /// A column's width now: being dragged, else as dragged before, else the usual one.
    pub(super) fn column_width(&self, name: Column, window: &Window, cx: &App) -> Pixels {
        if let Some(resize) = self.resizing.as_ref().filter(|r| r.name == name) {
            if let Some(now) = resize.now {
                return px(now);
            }
        }
        match layout::saved_width(&state::settings(cx).columns, name) {
            Some(width) => px(width as f32),
            None => rems(layout::default_rems(name)).to_pixels(window.rem_size()),
        }
    }

    /// The pointer moved while an edge is held: the column follows it, kept between the limits.
    pub(super) fn drag_resize(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(resize) = self.resizing.as_mut() else { return };
        // Dragged towards the start edge (left in English) the column widens, away from it the column narrows.
        let travelled = if i18n::is_rtl() {
            x - resize.from
        } else {
            resize.from - x
        };
        resize.now = Some(layout::clamp_width(resize.start + f32::from(travelled)));
        cx.notify();
    }

    /// The edge is let go: the width is kept, with the other settings.
    pub(super) fn end_resize(&mut self, cx: &mut Context<Self>) {
        let Some(resize) = self.resizing.take() else { return };
        if let Some(width) = resize.now {
            state::update(cx, |settings| {
                layout::set_saved_width(&mut settings.columns, resize.name, Some(f64::from(width)));
            });
        }
        cx.notify();
    }

    /// A click on a header button (`onSort`): the first click sorts by it (the biggest, longest or newest
    /// first where that is what is wanted), the next turns the order round.
    pub(super) fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        let pane = self.pane;
        let Some(current) = self.explorer.read(cx).pane_tab(pane).map(|tab| tab.sort) else {
            return;
        };
        let next = Sort {
            key,
            desc: if current.key == key {
                !current.desc
            } else {
                DESCENDING.contains(&key)
            },
        };
        self.explorer.update(cx, |explorer, cx| {
            // Sorting acts on the pane the keyboard is in: a click on this header makes it that pane.
            explorer.focus_pane(pane, cx);
            explorer.set_sort(next, cx);
        });
    }

    /// What the header's menu is made of, taken now so the menu can be built later.
    fn header_menu_data(&self, view: &TabView) -> HeaderMenu {
        HeaderMenu {
            columns: view.columns.clone(),
            sort: view.sort,
            group: view.group,
            // A search's or the Recycle Bin's columns are fixed; so is Group by in the grid.
            fixed: view.find.is_some() || view.recycled,
            like_others: view.own_view.then(|| {
                // "Like other folders again" names the kind of folder the others are.
                let kind = view.kind;
                if kind == glint_model::columns::FolderKind::General {
                    t!("Like other folders again")
                } else {
                    t!("Like other {kind} folders again", kind = kind.def().label)
                }
            }),
        }
    }

    /// A right-click on the header: its menu opens at the pointer, taking the keyboard.
    pub(super) fn open_header_menu(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        // The press that closed this menu is this click's own: it closes the menu, not opens it again.
        if self
            .header_dismissed
            .take()
            .is_some_and(|when| when.elapsed() < SAME_PRESS)
        {
            return;
        }
        let Some(view) = self.read_tab(cx) else { return };
        self.close_header_menu(false, window, cx);
        let data = self.header_menu_data(&view);
        let (nodes, picks) = data.nodes();
        let nodes = visible_items(nodes);
        let focus = window.focused(cx);
        let menu = cx.new(|cx| MenuView::new(nodes, t!("Columns"), cx));
        menu.update(cx, |menu, cx| menu.focus(window, cx));
        let subscription = cx.subscribe_in(&menu, window, |this, _, event: &MenuViewEvent, window, cx| {
            this.on_header_menu(event.clone(), window, cx)
        });
        self.header_menu = Some(HeaderOpen {
            view: menu,
            at,
            focus,
            data,
            picks,
            _subscription: subscription,
        });
        // A native preview draws above GPUI: it steps aside while the menu covers it.
        preview::cover(cx);
        cx.notify();
    }

    /// Closes the header's menu without choosing anything; `give_back` returns the keyboard to what had it.
    pub(super) fn close_header_menu(&mut self, give_back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.header_menu.take() {
            preview::uncover(cx);
            if let (true, Some(focus)) = (give_back, open.focus) {
                focus.focus(window, cx);
            }
            cx.notify();
        }
    }

    fn on_header_menu(&mut self, event: MenuViewEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.header_menu.take() else { return };
        preview::uncover(cx);
        cx.notify();
        match event {
            MenuViewEvent::Chose(run) => {
                if let Some(focus) = &open.focus {
                    focus.focus(window, cx);
                }
                if let Some(pick) = HeaderPick::of(&run).and_then(|n| open.picks.get(n).copied()) {
                    self.header_pick(pick, &open.data, cx);
                }
            }
            MenuViewEvent::Dismissed => {
                // A press on the header closed the menu: that press's right-click is not to open it again.
                let area = self.area.get();
                let at = window.mouse_position();
                let height = rems(2.).to_pixels(window.rem_size());
                let on_header =
                    at.x >= area.left() && at.x <= area.right() && at.y < area.top() && at.y >= area.top() - height;
                if on_header {
                    self.header_dismissed = Some(Instant::now());
                }
                // Escape gives the keyboard back; a press outside lets the thing pressed have it, unless that took
                // nothing.
                if let Some(focus) = open.focus.clone() {
                    let menu = open.view.read(cx).focus_handle(cx);
                    window.defer(cx, move |window, cx| {
                        if window.focused(cx).is_none_or(|now| now == menu) {
                            focus.focus(window, cx);
                        }
                    });
                }
            }
        }
    }

    /// Does what a chosen row of the header's menu says.
    fn header_pick(&mut self, pick: HeaderPick, data: &HeaderMenu, cx: &mut Context<Self>) {
        match pick {
            HeaderPick::Column(id) => {
                let next = if data.columns.contains(&id) {
                    data.columns.iter().copied().filter(|&c| c != id).collect()
                } else {
                    data.with_column(id)
                };
                cx.emit(FileListEvent::SetColumns { columns: next });
            }
            HeaderPick::LikeOthers => cx.emit(FileListEvent::ResetFolderView),
            HeaderPick::Group(by) => {
                self.explorer.update(cx, |explorer, cx| explorer.set_group(by, cx));
            }
            HeaderPick::Sort { key, column } => {
                if data.sort.key != key {
                    self.sort_by(key, cx);
                }
                // Sorted by a column that isn't shown: it shows.
                if let Some(id) = column.filter(|id| !data.fixed && !data.columns.contains(id)) {
                    cx.emit(FileListEvent::SetColumns {
                        columns: data.with_column(id),
                    });
                }
            }
        }
    }

    /// The header's menu, drawn in a layer over the window at the pointer, outside the list's flow.
    pub(super) fn header_menu_layer(&self) -> Option<AnyElement> {
        let open = self.header_menu.as_ref()?;
        // In the Arabic interface its end edge meets the point, so it opens toward the start.
        let anchor = if i18n::is_rtl() {
            Anchor::TopRight
        } else {
            Anchor::TopLeft
        };
        Some(
            deferred(
                anchored()
                    .position(open.at)
                    .anchor(anchor)
                    .snap_to_window_with_margin(px(4.))
                    .child(open.view.clone()),
            )
            .with_priority(gpui_kit::base::POPUP_PRIORITY)
            .into_any_element(),
        )
    }

    /// The header's menu, for tests.
    #[cfg(test)]
    pub(super) fn header_menu_view(&self) -> Option<&Entity<MenuView>> {
        self.header_menu.as_ref().map(|open| &open.view)
    }
}

/// The header's menu on screen.
pub(super) struct HeaderOpen {
    view: Entity<MenuView>,
    at: Point<Pixels>,
    /// What had the keyboard before it.
    focus: Option<FocusHandle>,
    data: HeaderMenu,
    /// What each row chosen does, by the number its [`Run`] carries.
    picks: Vec<HeaderPick>,
    _subscription: Subscription,
}

/// What a row of the header's menu does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum HeaderPick {
    /// Shows or hides a column.
    Column(ColumnId),
    /// "Like other folders again".
    LikeOthers,
    Group(GroupBy),
    /// Sorts by `key`; `column` is the column that sort is of, shown if it wasn't.
    Sort {
        key: SortKey,
        column: Option<ColumnId>,
    },
}

/// The prefix that marks a `Task::Navigate` as one of the header's picks (the menu hands back only a `Run`).
const SCHEME: &str = "file-list-header:";

impl HeaderPick {
    /// The row that carries pick number `n`.
    fn run(n: usize) -> Run {
        Run::Do(Task::Navigate(format!("{SCHEME}{n}")))
    }

    /// The number of the pick a chosen row carries.
    fn of(run: &Run) -> Option<usize> {
        let Run::Do(Task::Navigate(text)) = run else {
            return None;
        };
        text.strip_prefix(SCHEME)?.parse().ok()
    }
}

/// The header's right-click menu: the columns to show, Group by, Sort by.
#[derive(Clone)]
struct HeaderMenu {
    columns: Vec<ColumnId>,
    sort: Sort,
    group: GroupBy,
    fixed: bool,
    like_others: Option<SharedString>,
}

impl HeaderMenu {
    /// The columns shown with `id` added, in their usual order.
    fn with_column(&self, id: ColumnId) -> Vec<ColumnId> {
        let mut all: Vec<&str> = self.columns.iter().map(|c| c.as_str()).collect();
        all.push(id.as_str());
        glint_model::columns::in_order(&all)
    }

    /// The rows, and what choosing each does (a row's `Run` carries the number of its pick).
    fn nodes(&self) -> (Vec<Node>, Vec<HeaderPick>) {
        let mut nodes = Vec::new();
        let mut picks = Vec::new();
        let mut row =
            |nodes: &mut Vec<Node>, label: SharedString, icon: IconName, check: Option<bool>, pick: HeaderPick| {
                let mut entry = Entry::new(label, icon, HeaderPick::run(picks.len()));
                entry.check = check;
                picks.push(pick);
                nodes.push(Node::Item(entry));
            };
        let shown = |id: ColumnId| self.columns.contains(&id);
        if !self.fixed {
            nodes.push(Node::Heading(t!("Columns")));
            for id in COLUMN_ORDER {
                row(
                    &mut nodes,
                    i18n::translate(id.def().label),
                    IconName::Check,
                    Some(shown(id)),
                    HeaderPick::Column(id),
                );
            }
        }
        if let Some(label) = &self.like_others {
            row(
                &mut nodes,
                label.clone(),
                IconName::RotateCcw,
                None,
                HeaderPick::LikeOthers,
            );
        }
        // Sort by and Group by are submenus: the columns are what a right-click on the header is for.
        let mut grouping = Vec::new();
        for option in GROUP_BYS {
            row(
                &mut grouping,
                i18n::translate(option.label),
                IconName::Check,
                Some(self.group == option.by),
                HeaderPick::Group(option.by),
            );
        }
        let mut sorting = Vec::new();
        let mut keys: Vec<(SortKey, SharedString, Option<ColumnId>)> = vec![(SortKey::Name, t!("Name"), None)];
        keys.extend(
            COLUMN_ORDER
                .iter()
                .filter(|&&id| !self.fixed || shown(id))
                .map(|&id| (id.def().sort, i18n::translate(id.def().label), Some(id))),
        );
        for (key, label, column) in keys {
            row(
                &mut sorting,
                label,
                IconName::Check,
                Some(self.sort.key == key),
                HeaderPick::Sort { key, column },
            );
        }
        let submenu = |label: SharedString, icon: IconName, items: Vec<Node>| Node::Menu {
            label,
            icon: Some(icon),
            disabled: false,
            items,
        };
        nodes.push(submenu(t!("Sort by"), IconName::ArrowUpDown, sorting));
        nodes.push(submenu(t!("Group by"), IconName::Layers, grouping));
        (nodes, picks)
    }
}
