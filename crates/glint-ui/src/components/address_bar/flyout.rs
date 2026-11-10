//! The menus the chevrons open (`openFlyout` and the popover in AddressBar.tsx): the subfolders of the folder
//! before the chevron, the drives and places after "This PC", or the folders the "…" stands for.

use super::*;
use glint_model::entries::{is_archive_file, is_hidden};
use glint_model::paths::REMOTE;
use std::ops::Range;

/// A menu with more items than this has a box to filter them.
const FILTER_FROM: usize = 8;

/// Most rows a menu shows before it scrolls (`max-h-72`).
const MOST_ROWS: usize = 10;

/// Where a menu hangs: under which chevron or button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum At {
    /// The chevron after "This PC": drives and places.
    Roots,
    /// The chevron after the crumb at this position: its subfolders.
    Crumb(usize),
    /// The "…": the folders folded away.
    Folded,
}

/// The menu that is open.
pub(super) struct Flyout {
    pub at: At,
    /// The next folder along the path, marked in the list.
    pub active_child: Option<String>,
    pub items: Vec<Suggestion>,
    pub loading: bool,
    /// The box that narrows the items, for a long menu.
    pub filter: Entity<InputState>,
    /// Indices into `items` that the filter lets through.
    pub shown: Vec<usize>,
    /// The item the keyboard is on, as a position in `shown`.
    pub cursor: Option<usize>,
    pub focus: FocusHandle,
    pub scroll: UniformListScrollHandle,
    /// Where the keyboard was before the menu took it.
    pub prior: Option<FocusHandle>,
    pub task: Option<Task<()>>,
    _filter: Subscription,
}

/// A crumb as an item of the "…" menu: a drive by its icon, an archive marked, else a folder.
pub(super) fn crumb_item(crumb: &CrumbView) -> Suggestion {
    Suggestion {
        name: crumb.name.clone(),
        detail: None,
        path: crumb.path.clone(),
        dir: true,
        archive: crumb.archive,
        look: match crumb.drive {
            Some(kind) => Look::Drive(kind),
            None if crumb.archive => Look::Archive,
            None => Look::Folder,
        },
    }
}

/// The subfolders and archives of a listing (`toSuggestions`).
fn folder_items(entries: &[FileEntry], show_hidden: bool) -> Vec<Suggestion> {
    entries
        .iter()
        .filter(|e| (e.dir || is_archive_file(e)) && (show_hidden || !is_hidden(e)))
        .map(|e| Suggestion {
            name: e.name.clone(),
            detail: None,
            path: e.path.clone(),
            dir: true,
            archive: !e.dir,
            look: if e.dir { Look::Folder } else { Look::Archive },
        })
        .collect()
}

impl AddressBar {
    /// What the menu after "This PC" lists: every drive, the known folders, Recent, Gallery, the Recycle Bin
    /// and the remote connections.
    fn roots(&self, cx: &App) -> Vec<Suggestion> {
        let place = |name: SharedString, path: &str, look| Suggestion {
            name: name.to_string(),
            detail: None,
            path: path.to_string(),
            dir: true,
            archive: false,
            look,
        };
        let mut items: Vec<Suggestion> = self
            .seen
            .drives
            .iter()
            .map(|d| Suggestion {
                name: explorer::drive_name(d),
                detail: Some(d.fs.clone()),
                path: d.path.clone(),
                dir: true,
                archive: false,
                look: Look::Drive(d.kind),
            })
            .collect();
        items.extend(self.seen.known.iter().map(|k| Suggestion {
            name: i18n::translate(k.name).to_string(),
            detail: None,
            path: k.path.clone(),
            dir: true,
            archive: false,
            look: Look::Known(k.id),
        }));
        items.push(place(t!("Recent"), glint_model::paths::RECENT, Look::Recent));
        items.push(place(t!("Gallery"), glint_model::paths::GALLERY, Look::Gallery));
        items.push(place(
            t!("Recycle Bin"),
            glint_model::paths::RECYCLE_BIN,
            Look::RecycleBin,
        ));
        if let Some(app) = crate::persistence::app(cx) {
            items.extend(
                app.remotes
                    .iter()
                    .map(|r| place(r.name.clone().into(), &format!("{REMOTE}{}", r.id), Look::Remote)),
            );
        }
        items
    }

    /// Opens the menu at `at`: for `folder`'s subfolders, or for `given` items (the folded crumbs).
    pub(super) fn open_flyout(
        &mut self,
        at: At,
        folder: String,
        active_child: Option<String>,
        given: Option<Vec<Suggestion>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The menu opened last wins: a slow folder's list mustn't replace the one asked for after it.
        self.flyout_seq += 1;
        let seq = self.flyout_seq;
        let prior = self
            .flyout
            .as_ref()
            .and_then(|f| f.prior.clone())
            .or_else(|| window.focused(cx));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Filter folders…")));
        let subscription = cx.subscribe_in(&filter, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.flyout_filter_changed(cx);
            }
        });
        let show_hidden = state::settings(cx).show_hidden;

        let mut loading = false;
        let mut reading = None;
        let items = if let Some(given) = given {
            given
        } else if folder == THIS_PC {
            self.roots(cx)
        } else {
            // The folder open now is already listed: no trip to the disk.
            let listed = if same_path(&folder, &self.seen.path) {
                self.explorer
                    .read(cx)
                    .active_tab()
                    .and_then(|tab| tab.listing.as_ref().map(|l| l.entries.clone()))
            } else {
                None
            };
            match listed {
                Some(entries) => folder_items(&entries, show_hidden),
                None => {
                    loading = true;
                    reading = Some(self.source.list(&folder));
                    Vec::new()
                }
            }
        };
        let shown = (0..items.len()).collect();
        self.flyout = Some(Flyout {
            at,
            active_child,
            items,
            loading,
            filter,
            shown,
            cursor: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            prior,
            task: None,
            _filter: subscription,
        });
        if let Some(reading) = reading {
            let task = cx.spawn_in(window, async move |this, cx| {
                let items = reading
                    .await
                    .map(|listed| folder_items(&listed.entries, show_hidden))
                    .unwrap_or_default();
                this.update_in(cx, |this, window, cx| {
                    if this.flyout_seq != seq {
                        return;
                    }
                    if let Some(flyout) = this.flyout.as_mut() {
                        flyout.shown = (0..items.len()).collect();
                        flyout.items = items;
                        flyout.loading = false;
                        flyout.task = None;
                    }
                    this.focus_flyout(window, cx);
                    cx.notify();
                })
                .ok();
            });
            if let Some(flyout) = self.flyout.as_mut() {
                flyout.task = Some(task);
            }
        }
        self.focus_flyout(window, cx);
        cx.notify();
    }

    /// The keyboard goes into the menu: its filter box when it has one, else the menu itself.
    fn focus_flyout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flyout) = self.flyout.as_ref() else {
            return;
        };
        if flyout.items.len() > FILTER_FROM {
            let filter = flyout.filter.clone();
            filter.update(cx, |input, cx| input.focus(window, cx));
        } else {
            window.focus(&flyout.focus, cx);
        }
    }

    /// Closes the menu. With `restore`, the keyboard goes back to where it was.
    pub(super) fn close_flyout(&mut self, restore: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.flyout_seq += 1;
        let Some(flyout) = self.flyout.take() else {
            return;
        };
        if restore {
            match flyout.prior {
                Some(prior) => window.focus(&prior, cx),
                None => window.focus(&self.focus, cx),
            }
        }
        cx.notify();
    }

    /// The filter box changed: the items that contain what's typed (case-insensitively), the first at the top.
    fn flyout_filter_changed(&mut self, cx: &mut Context<Self>) {
        let Some(flyout) = self.flyout.as_mut() else {
            return;
        };
        let q = flyout
            .filter
            .read(cx)
            .value()
            .trim_matches(glint_model::js::is_js_space)
            .to_lowercase();
        flyout.shown = flyout
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| q.is_empty() || item.name.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        flyout.cursor = None;
        flyout.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Moves the keyboard's place in the menu: `step` rows round the ends, or to `to`.
    fn flyout_move(&mut self, to: FlyoutMove, cx: &mut Context<Self>) {
        let Some(flyout) = self.flyout.as_mut() else {
            return;
        };
        let n = flyout.shown.len();
        if n == 0 {
            return;
        }
        let next = match to {
            FlyoutMove::Down => flyout.cursor.map_or(0, |i| (i + 1) % n),
            FlyoutMove::Up => match flyout.cursor {
                Some(i) if i > 0 => i - 1,
                _ => n - 1,
            },
            FlyoutMove::First => 0,
            FlyoutMove::Last => n - 1,
        };
        flyout.cursor = Some(next);
        flyout.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Enter: opens the item the keyboard is on (the first, when it isn't on one).
    fn flyout_accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(flyout) = self.flyout.as_ref() else {
            return;
        };
        let at = flyout.cursor.unwrap_or(0);
        self.flyout_open_item(at, window, cx);
    }

    /// Opens the item at this position in the shown items, and closes the menu.
    fn flyout_open_item(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .flyout
            .as_ref()
            .and_then(|f| f.shown.get(at).and_then(|&i| f.items.get(i)))
            .map(|item| item.path.clone())
        else {
            return;
        };
        self.close_flyout(false, window, cx);
        self.open(path, window, cx);
    }

    /// A chevron, with the menu under it when it's the one open.
    #[allow(clippy::too_many_arguments)] // What one chevron is: where it hangs, what it opens, how it looks.
    pub(super) fn chevron(
        &mut self,
        id: impl Into<ElementId>,
        label: SharedString,
        at: At,
        folder: String,
        active_child: Option<String>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.flyout.as_ref().is_some_and(|f| f.at == at);
        let menu = self.flyout_menu(at, cx);
        div()
            .relative()
            .flex_none()
            .child(
                div()
                    .id(id)
                    .role(Role::Button)
                    .aria_label(label.clone())
                    .aria_expanded(open)
                    .test_support()
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(space(5.))
                    .rounded(theme::size::RADIUS_SM)
                    .text_color(t.muted_foreground.opacity(0.7))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.accent).text_color(t.foreground))
                    .focus_visible(|s| s.bg(t.accent))
                    .tooltip(tip(label))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_flyout(at, folder.clone(), active_child.clone(), None, window, cx)
                    }))
                    // It points along the path: right in English, left in Arabic.
                    .child(
                        icon(if i18n::is_rtl() {
                            IconName::ChevronLeft
                        } else {
                            IconName::ChevronRight
                        })
                        .size_3p5(),
                    ),
            )
            .children(menu)
    }

    /// What the menu is called.
    fn flyout_name(&self, at: At) -> SharedString {
        match at {
            At::Roots => t!("Drives and places"),
            At::Folded => t!("Folders above"),
            At::Crumb(i) => t!(
                "Subfolders of {name}",
                name = self.seen.crumbs.get(i).map(|c| c.name.as_str()).unwrap_or_default()
            ),
        }
    }

    /// The menu itself, when it hangs at `at`: under its button, kept inside the window, over everything.
    pub(super) fn flyout_menu(&mut self, at: At, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.flyout.as_ref()?.at != at {
            return None;
        }
        let t = theme::tokens(cx).clone();
        let name = self.flyout_name(at);
        let flyout = self.flyout.as_ref()?;
        let rtl = i18n::is_rtl();
        let rows = flyout.shown.len();
        let filtering = flyout.items.len() > FILTER_FROM;

        let body = if flyout.loading {
            align_row(2.)
                .id("address-flyout-loading")
                .role(Role::Status)
                .aria_label(t!("Loading folders…"))
                .test_support()
                .justify_center()
                .py(space(4.))
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(Spinner::new().xsmall())
                .child(t!("Loading folders…"))
                .into_any_element()
        } else if rows == 0 {
            let empty = if flyout.filter.read(cx).value().trim().is_empty() {
                t!("No subfolders")
            } else {
                t!("No matching folders")
            };
            div()
                .id("address-flyout-empty")
                .role(Role::Status)
                .aria_label(empty.clone())
                .test_support()
                .px(space(2.))
                .py(space(3.))
                .text_center()
                .text_size(theme::size::XS)
                .text_color(t.muted_foreground)
                .child(empty)
                .into_any_element()
        } else {
            uniform_list(
                "address-flyout-items",
                rows,
                cx.processor(|this, range: Range<usize>, _, cx| this.flyout_rows(range, cx)),
            )
            .track_scroll(&flyout.scroll)
            .h(space(7.) * rows.min(MOST_ROWS) as f32)
            .into_any_element()
        };

        let menu = col()
            .id("address-flyout")
            .role(Role::Menu)
            .aria_label(name)
            .test_support()
            .key_context(keys::FLYOUT)
            .track_focus(&flyout.focus)
            .occlude()
            .w(space(64.))
            .p(space(1.))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow_lg()
            .text_start()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_flyout(false, window, cx)))
            .on_action(cx.listener(|this, _: &ListDown, _, cx| this.flyout_move(FlyoutMove::Down, cx)))
            .on_action(cx.listener(|this, _: &ListUp, _, cx| this.flyout_move(FlyoutMove::Up, cx)))
            .on_action(cx.listener(|this, _: &ListFirst, _, cx| this.flyout_move(FlyoutMove::First, cx)))
            .on_action(cx.listener(|this, _: &ListLast, _, cx| this.flyout_move(FlyoutMove::Last, cx)))
            .on_action(cx.listener(|this, _: &ListAccept, window, cx| this.flyout_accept(window, cx)))
            .on_action(cx.listener(|this, _: &ListCancel, window, cx| this.close_flyout(true, window, cx)))
            .when(filtering, |menu| {
                menu.child(
                    div()
                        .mb(space(1.))
                        .pb(space(1.))
                        .border_b_1()
                        .border_color(t.border)
                        .child(Input::new(&flyout.filter).aria_label(t!("Filter folders…"))),
                )
            })
            .child(body);

        Some(
            deferred(
                div()
                    .absolute()
                    .top_full()
                    .mt(space(1.))
                    .when(rtl, |d| d.right_0())
                    .when(!rtl, |d| d.left_0())
                    .child(
                        anchored()
                            .anchor(if rtl { Anchor::TopRight } else { Anchor::TopLeft })
                            .snap_to_window_with_margin(px(4.))
                            .child(menu),
                    ),
            )
            .with_priority(20),
        )
    }

    /// The rows of the menu in view.
    fn flyout_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = theme::tokens(cx).clone();
        let Some(flyout) = self.flyout.as_ref() else {
            return Vec::new();
        };
        let rows: Vec<(usize, Suggestion, bool, bool)> = range
            .filter_map(|pos| {
                let item = flyout.items.get(*flyout.shown.get(pos)?)?;
                let active = flyout
                    .active_child
                    .as_deref()
                    .is_some_and(|child| same_path(&item.path, child));
                Some((pos, item.clone(), active, flyout.cursor == Some(pos)))
            })
            .collect();
        rows.into_iter()
            .map(|(pos, item, active, on_it)| {
                let name = suggestion_name(&item);
                let (path, middle) = (item.path.clone(), item.path.clone());
                align_row(2.)
                    .id(("address-flyout-item", pos))
                    .role(Role::MenuItem)
                    .aria_label(name.clone())
                    .aria_selected(on_it)
                    .test_support()
                    .h(space(7.))
                    .w_full()
                    .px(space(2.))
                    .rounded(theme::size::RADIUS_SM)
                    .text_size(theme::size::SM)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.accent))
                    .when(on_it, |row| row.bg(t.accent))
                    .when(active, |row| {
                        row.bg(t.primary.opacity(0.1))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.primary)
                    })
                    // The keyboard stays in the menu when a row is pressed.
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            window.prevent_default();
                            this.open_in_new_tab(&middle, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_flyout(false, window, cx);
                        this.open(path.clone(), window, cx);
                    }))
                    .child(look_icon(item.look, cx))
                    .child(div().flex_1().min_w_0().truncate().child(isolate(name)))
                    .children(item.detail.as_ref().map(|detail| {
                        div()
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(detail.clone())
                    }))
                    .children(active.then(|| icon(IconName::Check).size_3p5().flex_none().text_color(t.primary)))
                    .into_any_element()
            })
            .collect()
    }
}

/// Where the keyboard goes in a menu.
#[derive(Clone, Copy)]
enum FlyoutMove {
    Down,
    Up,
    First,
    Last,
}
