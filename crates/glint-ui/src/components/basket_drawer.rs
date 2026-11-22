//! The basket's drawer: what the basket holds, small pictures and names, and what to do with it all at once.
//!
//! The basket (`explorer/basket.rs`) is a plain list of paths gathered from any folder. Its pill in the dock shows
//! how many and how big; pressing the pill (or Alt+B) opens this drawer over it: one row for each item (a small
//! picture, its name, where it is, its size, a button that takes it out), Clear, and the jobs the basket does:
//! Batch rename, Compare (with exactly two files), Zip, Move here and Copy here (into the folder the active pane
//! shows). Dragging a row drags **the whole basket** out of the window as a group, through the same drag-out the
//! file list uses.
//!
//! **Nothing here does a job itself.** Each button emits the [`MenuEvent`] the basket's menu already sends
//! (`BasketRename`, `BasketZip`, `BasketMove`, `BasketCopy`, `Compare`), which the window handles; the drawer only
//! says which one. So the menu, the palette and the drawer can never drift apart.
//!
//! **Costs.** While the basket is empty the drawer does nothing. With items it reads each one's size and date
//! once, in the background (`Explorer::basket_rows`), and asks the thumbnail service only for the rows on screen,
//! at a small size (the service keeps what it made and cancels what scrolls away).
//!
//! **Keys** (while the drawer has the keyboard): Up and Down, Home and End move through the items, Delete or
//! Backspace takes the one the keyboard is on out, Escape closes the drawer and gives the keyboard back.

use crate::components::file_grid::thumbnail::{self, Look, Thumbnails, Want};
use crate::components::menus::MenuEvent;
use crate::components::panel_header::tool_button;
use crate::explorer::{self, Explorer};
use crate::kit::prelude::*;
use crate::{t, theme};
use glint_model::entries::FileEntry;
use glint_model::file_types::{is_picture_or_video, kind_of};
use glint_model::format::{format_count, format_size, plural};
use glint_model::paths::{base_name, parent_path};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

actions!(
    glint_basket,
    [
        /// Up: the item above.
        PreviousItem,
        /// Down: the item below.
        NextItem,
        /// Home: the first item.
        FirstItem,
        /// End: the last item.
        LastItem,
        /// Delete: take the item the keyboard is on out of the basket.
        TakeOut,
        /// Escape: close the drawer.
        CloseDrawer,
    ]
);

/// The drawer's key context.
const CONTEXT: &str = "BasketDrawer";
/// A row's height, in rems of spacing (`space`).
const ROW: f32 = 11.;
/// How many rows the drawer shows before it scrolls.
const MOST_ROWS: usize = 6;
/// The size of a row's picture, in CSS pixels. Small on purpose: thumbnails the service makes for it are tiny.
const PICTURE: f32 = 32.;
/// The pointer moving this far (squared, in pixels) with the button down starts a drag out.
const DRAG_SQUARED: f32 = 36.;

/// Marks that the keys are bound in this app.
struct Bound;

impl Global for Bound {}

/// Binds the drawer's keys, once per app (they are not commands: the shortcut sheet has no use for them).
fn ensure_bound(cx: &mut App) {
    if cx.has_global::<Bound>() {
        return;
    }
    cx.set_global(Bound);
    cx.bind_keys([
        KeyBinding::new("up", PreviousItem, Some(CONTEXT)),
        KeyBinding::new("down", NextItem, Some(CONTEXT)),
        KeyBinding::new("home", FirstItem, Some(CONTEXT)),
        KeyBinding::new("end", LastItem, Some(CONTEXT)),
        KeyBinding::new("delete", TakeOut, Some(CONTEXT)),
        KeyBinding::new("backspace", TakeOut, Some(CONTEXT)),
        KeyBinding::new("escape", CloseDrawer, Some(CONTEXT)),
    ]);
}

/// What the drawer tells the dock.
#[derive(Clone, Debug, PartialEq)]
pub enum BasketDrawerEvent {
    /// A job was chosen: the window does it (`Workspace::on_menu_event`).
    Menu(MenuEvent),
    /// The drawer closed itself (Escape, its close button, the last item leaving): give the keyboard back.
    Closed,
    /// A press outside the drawer closed it: a press on the pill is the same press and must not open it again.
    Dismissed,
}

/// The drawer's state.
pub struct BasketDrawer {
    explorer: Entity<Explorer>,
    thumbs: Entity<Thumbnails>,
    focus: FocusHandle,
    open: bool,
    /// What the basket holds, as last seen (its order).
    paths: Vec<String>,
    /// What is known of each item: its size, date and kind.
    entries: HashMap<String, FileEntry>,
    /// Items being read now, so none is asked for twice.
    reading: HashSet<String>,
    /// Whether the folder shown can take Move here and Copy here.
    here: bool,
    /// The item the keyboard is on.
    cursor: usize,
    scroll: UniformListScrollHandle,
    /// The row pressed, and where: it becomes a drag of the whole basket once the pointer has moved.
    press: Option<Point<Pixels>>,
    /// What the thumbnails shown were asked for, so they are asked again only when it changes.
    asked: u64,
    /// When a press outside the drawer last closed it: a press on the pill is that same press.
    dismissed_at: Option<std::time::Instant>,
    /// The groups dragged out (tests: the real drag is the shell's modal loop).
    #[cfg(test)]
    dragged: Vec<Vec<String>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<BasketDrawerEvent> for BasketDrawer {}

impl BasketDrawer {
    /// The drawer for the explorer's basket.
    pub fn new(cx: &mut Context<Self>) -> Self {
        ensure_bound(cx);
        let explorer = explorer::explorer(cx);
        let thumbs = thumbnail::thumbnails(cx);
        let subscriptions = vec![
            cx.observe(&explorer, |this: &mut Self, _, cx| this.sync(cx)),
            cx.observe(&thumbs, |this: &mut Self, _, cx| {
                if this.open {
                    cx.notify();
                }
            }),
        ];
        let mut drawer = BasketDrawer {
            explorer,
            thumbs,
            focus: cx.focus_handle(),
            open: false,
            paths: Vec::new(),
            entries: HashMap::new(),
            reading: HashSet::new(),
            here: false,
            cursor: 0,
            scroll: UniformListScrollHandle::new(),
            press: None,
            asked: 0,
            dismissed_at: None,
            #[cfg(test)]
            dragged: Vec::new(),
            _subscriptions: subscriptions,
        };
        drawer.sync(cx);
        drawer
    }

    /// The groups dragged out so far (tests).
    #[cfg(test)]
    pub fn dragged(&self) -> &[Vec<String>] {
        &self.dragged
    }

    /// Whether a press outside the drawer closed it a moment ago (taken: it answers once).
    pub fn take_just_dismissed(&mut self) -> bool {
        self.dismissed_at
            .take()
            .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(300))
    }

    /// Whether the drawer is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Where the keyboard goes while the drawer is open.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// What the basket holds, as the drawer shows it.
    pub fn items(&self) -> &[String] {
        &self.paths
    }

    /// The item the keyboard is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// What the files in the basket add up to, when every item's size is known. Folders count for nothing (their
    /// contents aren't measured): [`BasketDrawer::folders`] says whether there are any.
    pub fn total_size(&self) -> Option<u64> {
        if self.paths.is_empty() || self.paths.iter().any(|path| !self.entries.contains_key(path)) {
            return None;
        }
        Some(self.entries.values().filter(|e| !e.dir).map(|e| e.size).sum())
    }

    /// How many of the items are folders.
    pub fn folders(&self) -> usize {
        self.entries.values().filter(|e| e.dir).count()
    }

    /// What the pill says besides how many: "12.4 MB", or nothing until every size is known.
    pub fn size_words(&self) -> Option<String> {
        self.total_size().map(format_size)
    }

    /// The folder shown can (not) take Move here and Copy here; the dock says so as its facts change.
    pub fn set_here(&mut self, here: bool, cx: &mut Context<Self>) {
        if self.here != here {
            self.here = here;
            if self.open {
                cx.notify();
            }
        }
    }

    /// The two files Compare would take: exactly two items, neither a folder.
    pub fn compare_pair(&self) -> Option<(String, String)> {
        let [left, right] = self.paths.as_slice() else {
            return None;
        };
        let file = |path: &String| self.entries.get(path).is_some_and(|entry| !entry.dir);
        (file(left) && file(right)).then(|| (left.clone(), right.clone()))
    }

    /// Opens the drawer (it keeps the keyboard) or closes it. With nothing in the basket it stays shut.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
        } else {
            self.open(window, cx);
        }
    }

    /// Opens the drawer.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync(cx);
        if self.paths.is_empty() || self.open {
            return;
        }
        self.open = true;
        self.cursor = self.cursor.min(self.paths.len() - 1);
        self.asked = 0;
        self.focus.focus(window, cx);
        // What was moved or deleted since it was added is dropped, so the drawer says what is true.
        self.explorer.update(cx, |e, cx| e.prune_basket(cx)).detach();
        cx.notify();
    }

    /// Closes the drawer. The dock gives the keyboard back.
    pub fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.release(cx);
            cx.emit(BasketDrawerEvent::Closed);
            cx.notify();
        }
    }

    /// Lets go of the thumbnails the drawer asked for.
    fn release(&mut self, cx: &mut Context<Self>) {
        let id = cx.entity_id();
        self.thumbs.update(cx, |thumbs, cx| thumbs.release(id, cx));
        self.asked = 0;
        self.press = None;
    }

    /// Takes in what the basket holds now: reads the new items' sizes, forgets the gone ones.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let held = self.explorer.read(cx).basket();
        if held == self.paths.as_slice() {
            return;
        }
        self.paths = held.to_vec();
        self.entries.retain(|path, _| self.paths.contains(path));
        self.reading.retain(|path| self.paths.contains(path));
        self.cursor = self.cursor.min(self.paths.len().saturating_sub(1));
        let unknown: Vec<String> = self
            .paths
            .iter()
            .filter(|path| !self.entries.contains_key(*path) && !self.reading.contains(*path))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            self.reading.extend(unknown.iter().cloned());
            let looking = self.explorer.read(cx).basket_rows(unknown);
            cx.spawn(async move |this, cx| {
                let rows = looking.await;
                this.update(cx, |this, cx| {
                    for row in rows {
                        this.reading.remove(&row.path);
                        if this.paths.contains(&row.path) {
                            this.entries.insert(row.path.clone(), row);
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        if self.paths.is_empty() && self.open {
            self.open = false;
            self.release(cx);
            cx.emit(BasketDrawerEvent::Closed);
        }
        cx.notify();
    }

    /// Takes one item out (the row's button, Delete).
    pub fn remove(&mut self, path: &str, cx: &mut Context<Self>) {
        let path = path.to_string();
        self.explorer.update(cx, |e, cx| e.remove_from_basket(&path, cx));
    }

    /// Takes everything out. The files stay where they are.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.explorer.update(cx, |e, cx| e.empty_basket(cx));
    }

    /// Asks the window to do one of the basket's jobs.
    fn job(&mut self, event: MenuEvent, cx: &mut Context<Self>) {
        cx.emit(BasketDrawerEvent::Menu(event));
    }

    /// Batch rename.
    pub fn rename(&mut self, cx: &mut Context<Self>) {
        self.job(MenuEvent::BasketRename, cx);
    }

    /// Compare, with exactly two files in the basket.
    pub fn compare(&mut self, cx: &mut Context<Self>) {
        if let Some((left, right)) = self.compare_pair() {
            self.job(MenuEvent::Compare { left, right }, cx);
        }
    }

    /// Zip.
    pub fn zip(&mut self, cx: &mut Context<Self>) {
        self.job(MenuEvent::BasketZip, cx);
    }

    /// Move here (the folder the active pane shows).
    pub fn move_here(&mut self, cx: &mut Context<Self>) {
        if self.here {
            self.job(MenuEvent::BasketMove, cx);
        }
    }

    /// Copy here.
    pub fn copy_here(&mut self, cx: &mut Context<Self>) {
        if self.here {
            self.job(MenuEvent::BasketCopy, cx);
        }
    }

    /// Drags the whole basket out of the window, to a folder in File Explorer, the desktop, a mail.
    pub fn drag_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.press = None;
        if self.paths.is_empty() {
            return;
        }
        let paths = self.paths.clone();
        #[cfg(test)]
        {
            let _ = (window, cx);
            self.dragged.push(paths);
        }
        #[cfg(not(test))]
        {
            let drag = crate::platform::dragdrop::drag_out(cx, crate::workspace::MAIN, paths, false);
            window
                .spawn(cx, async move |cx| {
                    if let Err(err) = drag.await {
                        let _ = cx.update(|window, cx| {
                            window.push_notification(Notification::error(err).title(t!("Couldn't start drag")), cx);
                        });
                    }
                })
                .detach();
        }
    }

    fn step(&mut self, to: usize, cx: &mut Context<Self>) {
        if self.paths.is_empty() {
            return;
        }
        self.cursor = to.min(self.paths.len() - 1);
        self.scroll.scroll_to_item(self.cursor, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Delete: the item the keyboard is on leaves; the keyboard stays at the same place in the list.
    fn take_out_cursor(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.paths.get(self.cursor).cloned() {
            self.remove(&path, cx);
        }
    }

    /// Tells the thumbnail service which pictures the rows in `range` need, if that changed since last time.
    fn show_thumbnails(&mut self, range: &Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let scale = window.scale_factor();
        let wanted: Vec<&FileEntry> = range
            .clone()
            .filter_map(|i| self.paths.get(i))
            .filter_map(|path| self.entries.get(path))
            .filter(|entry| is_picture_or_video(entry))
            .collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash, Hasher};
        (scale.to_bits(), PICTURE.to_bits()).hash(&mut hasher);
        for entry in &wanted {
            (&entry.path, entry.modified, entry.size).hash(&mut hasher);
        }
        let fingerprint = hasher.finish();
        if fingerprint == self.asked {
            return;
        }
        self.asked = fingerprint;
        let wants: Vec<Want> = wanted
            .into_iter()
            .filter_map(|entry| Want::for_entry(entry, PICTURE, scale))
            .collect();
        let id = cx.entity_id();
        self.thumbs
            .update(cx, |thumbs, cx| thumbs.show(id, "basket", wants, cx));
        for freed in self.thumbs.update(cx, |thumbs, _| thumbs.take_evicted()) {
            window.drop_image(freed).ok();
        }
    }

    /// The accessible name of a row: the name, and what it is.
    fn row_name(&self, path: &str) -> SharedString {
        match self.entries.get(path) {
            Some(entry) if entry.dir => t!("{name}, folder", name = entry.name.clone()),
            Some(entry) => t!(
                "{name}, {size}",
                name = entry.name.clone(),
                size = format_size(entry.size)
            ),
            None => base_name(path).to_string().into(),
        }
    }

    /// A row's picture: the thumbnail when there is one, else the kind's own.
    fn picture(&self, path: &str, window: &Window, cx: &App) -> AnyElement {
        let across = px(PICTURE);
        let Some(entry) = self.entries.get(path) else {
            return div().size(across).flex_none().into_any_element();
        };
        let kind = kind_of(entry);
        if is_picture_or_video(entry) {
            let look = self.thumbs.read(cx).look(entry, PICTURE, window.scale_factor());
            if !matches!(look, Look::Icon) {
                return thumbnail::thumbnail_square(&look, kind, PICTURE, PICTURE * 0.8, false, cx).into_any_element();
            }
        }
        div()
            .size(across)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(crate::art::kind(kind, across * 0.9, cx))
            .into_any_element()
    }

    fn rows(&mut self, range: Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.show_thumbnails(&range, window, cx);
        let t = theme::tokens(cx).clone();
        let total = self.paths.len();
        range
            .filter_map(|i| self.paths.get(i).cloned().map(|path| (i, path)))
            .map(|(i, path)| {
                let on_it = i == self.cursor;
                let name = self.row_name(&path);
                let shown = self
                    .entries
                    .get(&path)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| base_name(&path).to_string());
                let place = parent_path(&path);
                let facts = match self.entries.get(&path) {
                    Some(e) if !e.dir => format!("{} · {}", format_size(e.size), place),
                    _ => place,
                };
                let taken = path.clone();
                let remove_name = t!("Take out “{name}”", name = base_name(&path));
                row_start()
                    .id(("basket-item", i))
                    .role(Role::ListBoxOption)
                    .aria_label(name)
                    .aria_selected(on_it)
                    .aria_position_in_set(i + 1)
                    .aria_size_of_set(total)
                    .test_support()
                    .h(space(ROW))
                    .w_full()
                    .px(space(2.))
                    .gap(space(2.))
                    .items_center()
                    .rounded(theme::size::RADIUS_ROW)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.accent))
                    .when(on_it, |row| row.bg(t.selected))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cursor = i;
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, _| {
                            this.press = Some(event.position);
                        }),
                    )
                    .child(self.picture(&path, window, cx))
                    .child(
                        col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(theme::size::SM)
                                    .text_color(t.foreground)
                                    .child(isolate(shown)),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(theme::size::XS)
                                    .text_color(t.muted_foreground)
                                    .child(isolate(facts)),
                            ),
                    )
                    .child(
                        Button::new(("basket-take-out", i))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .accessibility_label(remove_name.clone())
                            .tooltip(remove_name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove(&taken, cx);
                            })),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// One of the jobs as a button: a picture and a word.
    fn job_button(
        &self,
        id: &'static str,
        picture: IconName,
        label: SharedString,
        why: Option<SharedString>,
        cx: &mut Context<Self>,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> impl IntoElement {
        let off = why.is_some();
        let tip = why.unwrap_or_else(|| label.clone());
        Button::new(id)
            .ghost()
            .small()
            .icon(picture)
            .label(label.clone())
            .accessibility_label(label)
            .tooltip(tip)
            .disabled(off)
            .on_click(cx.listener(move |this, _, _, cx| run(this, cx)))
    }
}

impl Focusable for BasketDrawer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BasketDrawer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open || self.paths.is_empty() {
            return div().into_any_element();
        }
        let t = theme::tokens(cx).clone();
        let count = self.paths.len();
        let folders = self.folders();
        let mut summary = plural(count, "item");
        if let Some(size) = self.size_words() {
            summary = format!("{summary} · {size}");
        }
        if folders > 0 {
            summary = format!("{summary} · {}", t!("folders not measured"));
        }
        let heading = t!("Basket: {summary}", summary = summary.clone());
        let two = self.compare_pair().is_some();
        let compare_why = (!two).then(|| t!("Compare needs exactly two files in the basket"));
        let here_why = (!self.here).then(|| t!("Open a folder here first: it takes what the basket holds"));
        let rows = count.min(MOST_ROWS);

        let header = row_start()
            .items_center()
            .gap(space(2.))
            .px(space(2.))
            .child(icon(IconName::Package).size_4().text_color(t.primary))
            .child(
                div()
                    .id("basket-summary")
                    .role(Role::Status)
                    .aria_label(heading)
                    .test_support()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::SM)
                    .font_weight(FontWeight::MEDIUM)
                    .child(isolate(summary)),
            )
            .child(
                Button::new("basket-clear")
                    .ghost()
                    .small()
                    .icon(IconName::Trash)
                    .label(t!("Clear"))
                    .accessibility_label(t!("Clear"))
                    .tooltip(t!("Empty the basket: the files stay where they are"))
                    .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
            )
            .child(tool_button(
                "basket-close",
                IconName::X,
                t!("Close the basket"),
                t!("Close the basket (Esc)"),
                cx.listener(|this, _, window, cx| this.close(window, cx)),
            ))
            .pe(1.);

        let jobs = row_start()
            .flex_wrap()
            .gap(space(1.))
            .px(space(1.))
            .child(self.job_button(
                "basket-rename",
                IconName::PenLine,
                t!("Batch rename"),
                None,
                cx,
                |this, cx| this.rename(cx),
            ))
            .child(self.job_button(
                "basket-compare",
                IconName::GitCompareArrows,
                t!("Compare"),
                compare_why,
                cx,
                |this, cx| this.compare(cx),
            ))
            .child(
                self.job_button("basket-zip", IconName::Package, t!("Zip"), None, cx, |this, cx| {
                    this.zip(cx)
                }),
            )
            .child(self.job_button(
                "basket-move",
                IconName::FolderInput,
                t!("Move here"),
                here_why.clone(),
                cx,
                |this, cx| this.move_here(cx),
            ))
            .child(self.job_button(
                "basket-copy",
                IconName::Copy,
                t!("Copy here"),
                here_why,
                cx,
                |this, cx| this.copy_here(cx),
            ));

        let list = uniform_list(
            "basket-items",
            count,
            cx.processor(|this, range: Range<usize>, window, cx| this.rows(range, window, cx)),
        )
        .track_scroll(&self.scroll)
        .h(space(ROW) * rows as f32);

        let hint = div()
            .px(space(2.))
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(t!("Drag any row to take all of it out of the window"));

        col()
            .id("basket-drawer")
            .role(Role::Group)
            .aria_label(t!("Basket"))
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .occlude()
            .w(space(96.))
            .max_w(relative(1.))
            .gap(space(1.5))
            .py(space(2.))
            .rounded(theme::size::RADIUS_SHEET)
            .border_1()
            .border_color(t.stroke)
            .bg(t.popover)
            .text_color(t.popover_foreground)
            .shadow(theme::elevation::floating(cx))
            .text_start()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.open {
                    this.open = false;
                    this.dismissed_at = Some(std::time::Instant::now());
                    this.release(cx);
                    cx.emit(BasketDrawerEvent::Dismissed);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PreviousItem, _, cx| this.step(this.cursor.saturating_sub(1), cx)))
            .on_action(cx.listener(|this, _: &NextItem, _, cx| this.step(this.cursor + 1, cx)))
            .on_action(cx.listener(|this, _: &FirstItem, _, cx| this.step(0, cx)))
            .on_action(cx.listener(|this, _: &LastItem, _, cx| this.step(usize::MAX, cx)))
            .on_action(cx.listener(|this, _: &TakeOut, _, cx| this.take_out_cursor(cx)))
            .on_action(cx.listener(|this, _: &CloseDrawer, window, cx| this.close(window, cx)))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                let Some(origin) = this.press else { return };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.press = None;
                    return;
                }
                let (dx, dy) = (
                    f32::from(event.position.x - origin.x),
                    f32::from(event.position.y - origin.y),
                );
                if dx * dx + dy * dy >= DRAG_SQUARED {
                    this.drag_out(window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.press = None;
                }),
            )
            .child(header)
            .child(jobs)
            .child(
                div()
                    .id("basket-list")
                    .role(Role::ListBox)
                    .aria_label(t!("In the basket ({n})", n = format_count(count)))
                    .test_support()
                    .px(space(1.))
                    .child(list),
            )
            .child(hint)
            .into_any_element()
    }
}
