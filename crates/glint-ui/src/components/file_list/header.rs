//! The list's header: a button per column that sorts by it, the edges that resize the columns, and the menu
//! on a right-click (`Header`, `ColumnEdge` and `ResizeHandle` in FileList.tsx and ResizeHandle.tsx, the
//! header's menu `onHeaderMenu` in Explorer.tsx).

use super::{layout, FileList, FileListEvent, Frame, Resize, TabView, Track};
use crate::kit::prelude::*;
use crate::theme::{self, Tokens};
use crate::{i18n, state, t};
use glint_model::columns::{ColumnId, COLUMN_ORDER, DESCENDING};
use glint_model::entries::{Sort, SortKey};
use glint_model::groups::GROUP_BYS;
use glint_model::store::Column;

/// The strip along a column's start edge that takes the drag, in pixels, and how far it sits outside the
/// column (`-start-2.5`).
const EDGE_WIDTH: f32 = 8.;
const EDGE_OUTSET: f32 = 10.;

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
        let menu = self.header_menu_data(view);
        let this = cx.weak_entity();
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
            .context_menu(move |popup, _, _| menu.build(popup, this.clone()))
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
            .absolute()
            .top_0()
            .h_full()
            .w(px(EDGE_WIDTH))
            .cursor_col_resize()
            .hover(|style| style.bg(t.primary.opacity(0.4)));
        let edge = if i18n::is_rtl() {
            edge.right(px(-EDGE_OUTSET))
        } else {
            edge.left(px(-EDGE_OUTSET))
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
}

/// The header's right-click menu: the columns to show, Group by, Sort by.
struct HeaderMenu {
    columns: Vec<ColumnId>,
    sort: Sort,
    group: glint_model::groups::GroupBy,
    fixed: bool,
    like_others: Option<SharedString>,
}

impl HeaderMenu {
    fn build(&self, menu: PopupMenu, this: WeakEntity<FileList>) -> PopupMenu {
        let mut menu = menu;
        let shown = |id: ColumnId| self.columns.contains(&id);
        if !self.fixed {
            menu = menu.item(PopupMenuItem::label(t!("Columns")));
            for id in COLUMN_ORDER {
                let this = this.clone();
                let columns = self.columns.clone();
                menu = menu.item(
                    PopupMenuItem::new(i18n::translate(id.def().label))
                        .checked(shown(id))
                        .on_click(move |_, _, cx| {
                            let next = if columns.contains(&id) {
                                columns.iter().copied().filter(|&c| c != id).collect()
                            } else {
                                let mut all: Vec<&str> = columns.iter().map(|c| c.as_str()).collect();
                                all.push(id.as_str());
                                glint_model::columns::in_order(&all)
                            };
                            this.update(cx, |_, cx| cx.emit(FileListEvent::SetColumns { columns: next }))
                                .ok();
                        }),
                );
            }
        }
        if let Some(label) = &self.like_others {
            let this = this.clone();
            menu = menu.item(
                PopupMenuItem::new(label.clone())
                    .icon(IconName::RotateCcw)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |_, cx| cx.emit(FileListEvent::ResetFolderView)).ok();
                    }),
            );
        }
        menu = menu.item(PopupMenuItem::label(t!("Group by")));
        for option in GROUP_BYS {
            let this = this.clone();
            let by = option.by;
            menu = menu.item(
                PopupMenuItem::new(i18n::translate(option.label))
                    .checked(self.group == by)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            this.explorer.update(cx, |explorer, cx| explorer.set_group(by, cx));
                        })
                        .ok();
                    }),
            );
        }
        menu = menu.item(PopupMenuItem::label(t!("Sort by")));
        let mut keys: Vec<(SortKey, SharedString, Option<ColumnId>)> = vec![(SortKey::Name, t!("Name"), None)];
        keys.extend(
            COLUMN_ORDER
                .iter()
                .filter(|&&id| !self.fixed || shown(id))
                .map(|&id| (id.def().sort, i18n::translate(id.def().label), Some(id))),
        );
        for (key, label, column) in keys {
            let this = this.clone();
            let columns = self.columns.clone();
            let fixed = self.fixed;
            let current = self.sort.key;
            menu = menu.item(
                PopupMenuItem::new(label)
                    .checked(current == key)
                    .on_click(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            if current != key {
                                this.sort_by(key, cx);
                            }
                            // Sorted by a column that isn't shown: it shows.
                            if let Some(id) = column.filter(|id| !fixed && !columns.contains(id)) {
                                let mut all: Vec<&str> = columns.iter().map(|c| c.as_str()).collect();
                                all.push(id.as_str());
                                cx.emit(FileListEvent::SetColumns {
                                    columns: glint_model::columns::in_order(&all),
                                });
                            }
                        })
                        .ok();
                    }),
            );
        }
        menu
    }
}
