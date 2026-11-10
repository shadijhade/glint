//! Quick Look: Space on the focused item shows it large over the window; the arrows flip through the list
//! underneath; Space or Escape closes it (QuickLook.tsx, and Explorer.tsx's keys for it).
//!
//! It is a dialog of the kit's dialog layer (the window behind is covered, focus returns where it was), holding
//! the same previewer the pane would show for the item ([`EntryView`] in [`Place::QuickLook`]). The item is the
//! one the keyboard is on in the active tab, else the first selected (`rows[focusIndex] ?? selected[0]`); it
//! follows that, so moving through the list moves the preview.
//!
//! **Keys** (context "QuickLook", in `actions::BINDINGS`): Space and Escape close it; the arrows move to the
//! next or previous item of the list (`explorer.select_only`: the list underneath moves with it); Page Down
//! and Page Up scroll the preview. The dialog's own box has the focus, so a previewer's text box never takes the
//! arrows.
//!
//! **The explorer's flag.** `Explorer::quick_look` says Quick Look is open (the TypeScript's `quickLook`); the
//! model clears it when the tab changes. Opening here sets it, and the view clears it when it goes. The
//! workspace calls [`sync`] after explorer changes, which opens Quick Look when the model set the flag itself
//! (Enter on a Recycle Bin item looks at it) and closes it when the model cleared it.

use super::frame::{EntryEvent, EntryView, Place};
use crate::explorer::{self, Explorer};
use crate::kit::prelude::*;
use crate::{actions, t, theme};
use glint_model::entries::FileEntry;
use std::cell::Cell;
use std::rc::Rc;

/// The dialog's size in the usual state: at most `max-w-[80rem]` by `max-h-[56rem]`, with a margin of `p-8`.
const MAX_WIDTH: f32 = 1280.;
const MAX_HEIGHT: f32 = 896.;
const MARGIN: f32 = 32.;
/// Maximized (`p-2.5`).
const MAXIMIZED_MARGIN: f32 = 10.;

/// The Quick Look that is open, if one is.
struct Current(WeakEntity<QuickLook>);

impl Global for Current {}

/// Quick Look's dialog content.
pub struct QuickLook {
    explorer: Entity<Explorer>,
    view: Entity<EntryView>,
    focus: FocusHandle,
    /// Read by the dialog's builder, which runs again whenever the window draws.
    maximized: Rc<Cell<bool>>,
    _subscriptions: Vec<Subscription>,
}

/// The item the keyboard is on in the active tab, else the first selected: Quick Look's item (`looked`).
pub fn looked(explorer: &Explorer) -> Option<FileEntry> {
    let tab = explorer.active_tab()?;
    let by_path = |path: &str| tab.rows.position_of(path).and_then(|i| tab.rows.get(i)).cloned();
    tab.selection
        .focus()
        .and_then(by_path)
        .or_else(|| tab.selection.iter().next().and_then(by_path))
}

/// The item `step` rows from the focused one, kept inside the list (no wrap-around, as the arrows in the list).
fn neighbour(explorer: &Explorer, step: isize) -> Option<FileEntry> {
    let tab = explorer.active_tab()?;
    let current = looked(explorer)?;
    let at = tab.rows.position_of(&current.path)?;
    let next = at.checked_add_signed(step)?;
    tab.rows.get(next).cloned()
}

impl QuickLook {
    fn new(entry: FileEntry, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let explorer = explorer::explorer(cx);
        let maximized = Rc::new(Cell::new(false));
        let view = cx.new(|cx| EntryView::new(entry, Place::QuickLook, window, cx));
        let subscriptions = vec![
            // Quick Look follows the cursor, so arrowing through the list flips through previews.
            cx.observe_in(&explorer, window, |this, explorer, window, cx| {
                this.follow(&explorer, window, cx)
            }),
            cx.subscribe_in(&view, window, |this, _, event: &EntryEvent, window, cx| {
                this.on_header(event, window, cx)
            }),
        ];
        // The model's flag stays set only while this is open.
        cx.on_release(|_, cx| {
            if let Some(explorer) = explorer::try_explorer(cx) {
                explorer.update(cx, |explorer, _| explorer.quick_look = false);
            }
        })
        .detach();
        QuickLook {
            explorer,
            view,
            focus: cx.focus_handle(),
            maximized,
            _subscriptions: subscriptions,
        }
    }

    /// The item shown.
    pub fn entry(&self, cx: &App) -> FileEntry {
        self.view.read(cx).entry().clone()
    }

    /// The preview in the dialog.
    pub fn view(&self) -> &Entity<EntryView> {
        &self.view
    }

    /// Whether it is maximized.
    pub fn is_maximized(&self) -> bool {
        self.maximized.get()
    }

    /// A header button was pressed.
    fn on_header(&mut self, event: &EntryEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            EntryEvent::Close => close(window, cx),
            EntryEvent::ToggleMaximize => self.toggle_maximize(cx),
        }
    }

    fn toggle_maximize(&mut self, cx: &mut Context<Self>) {
        self.maximized.set(!self.maximized.get());
        let maximized = self.maximized.get();
        self.view.update(cx, |view, cx| view.set_maximized(maximized, cx));
        cx.notify();
    }

    /// The explorer changed: show the item the keyboard is on now, or close when there is none.
    fn follow(&mut self, explorer: &Entity<Explorer>, window: &mut Window, cx: &mut Context<Self>) {
        // Anything that changes the tab clears the flag (the model's rule).
        if !explorer.read(cx).quick_look {
            close(window, cx);
            return;
        }
        match looked(explorer.read(cx)) {
            None => close(window, cx),
            Some(entry) => {
                let shown = self.view.read(cx).entry();
                if shown.path != entry.path || shown.modified != entry.modified {
                    let maximized = self.maximized.get();
                    let view = cx.new(|cx| EntryView::new(entry, Place::QuickLook, window, cx));
                    view.update(cx, |view, cx| view.set_maximized(maximized, cx));
                    self._subscriptions.truncate(1);
                    self._subscriptions.push(cx.subscribe_in(
                        &view,
                        window,
                        |this, _, event: &EntryEvent, window, cx| this.on_header(event, window, cx),
                    ));
                    self.view = view;
                    cx.notify();
                }
            }
        }
    }

    /// Next or previous item of the list: the list's own selection moves, and Quick Look follows.
    fn step(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(next) = neighbour(self.explorer.read(cx), step) else {
            return;
        };
        self.explorer
            .update(cx, |explorer, cx| explorer.select_only(Some(&next.path), cx));
    }

    // PORT ME(previewers): Page Down and Page Up scroll what the frame scrolls itself (cards, a tall body). The
    // text, Markdown, hex and data views scroll inside themselves; they need a `page(down)` the frame can call
    // (the TypeScript scrolled the deepest scroller in the overlay).
    fn scroll(&mut self, down: bool, cx: &mut Context<Self>) {
        self.view.update(cx, |view, cx| {
            view.page(down, cx);
        });
    }
}

impl Render for QuickLook {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let name = self.view.read(cx).entry().name.clone();
        div()
            .id("quick-look")
            .role(Role::Dialog)
            .aria_label(t!("Quick Look: {name}", name = name))
            .test_support()
            .key_context("QuickLook")
            .track_focus(&self.focus)
            .size_full()
            .overflow_hidden()
            .rounded(theme::size::RADIUS_XL)
            .bg(t.card)
            .text_start()
            .on_action(cx.listener(|_, _: &actions::CloseQuickLook, window, cx| close(window, cx)))
            .on_action(cx.listener(|this, _: &actions::QuickLookNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &actions::QuickLookPrevious, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollDown, _, cx| this.scroll(true, cx)))
            .on_action(cx.listener(|this, _: &actions::QuickLookScrollUp, _, cx| this.scroll(false, cx)))
            .child(self.view.clone())
    }
}

// ---- Opening it ---------------------------------------------------------------------------------------------

/// Whether Quick Look is showing.
pub fn is_open(window: &mut Window, cx: &mut App) -> bool {
    cx.try_global::<Current>()
        .is_some_and(|current| current.0.upgrade().is_some())
        && window.has_active_dialog(cx)
}

/// The open Quick Look, if one is.
pub fn current(cx: &App) -> Option<Entity<QuickLook>> {
    cx.try_global::<Current>().and_then(|current| current.0.upgrade())
}

/// Opens Quick Look on the item the keyboard is on in the active tab; nothing happens without one, or when
/// it is open already.
pub fn open(window: &mut Window, cx: &mut App) {
    if is_open(window, cx) {
        return;
    }
    let explorer = explorer::explorer(cx);
    let Some(entry) = looked(explorer.read(cx)) else {
        return;
    };
    explorer.update(cx, |explorer, _| explorer.quick_look = true);
    let view = cx.new(|cx| QuickLook::new(entry, window, cx));
    cx.set_global(Current(view.downgrade()));
    let maximized = view.read(cx).maximized.clone();
    let focus = view.read(cx).focus.clone();
    window.open_dialog(cx, move |dialog, window, _cx| {
        let viewport = window.viewport_size();
        let margin = if maximized.get() { MAXIMIZED_MARGIN } else { MARGIN };
        let (cap_w, cap_h) = if maximized.get() {
            (f32::MAX, f32::MAX)
        } else {
            (MAX_WIDTH, MAX_HEIGHT)
        };
        let width = (f32::from(viewport.width) - margin * 2.).min(cap_w).max(0.);
        let height = (f32::from(viewport.height) - margin * 2.).min(cap_h).max(0.);
        dialog
            .w(px(width))
            .h(px(height))
            .centered()
            .close_button(false)
            .p_0()
            .on_ok(|_, _, _| false)
            .child(view.clone())
    });
    // The dialog layer takes the focus when it opens; Quick Look's box takes it from there, so Space, Escape
    // and the arrows reach it.
    window.defer(cx, move |window, cx| window.focus(&focus, cx));
}

/// Closes Quick Look.
pub fn close(window: &mut Window, cx: &mut App) {
    if is_open(window, cx) {
        window.close_dialog(cx);
    }
    if let Some(explorer) = explorer::try_explorer(cx) {
        explorer.update(cx, |explorer, _| explorer.quick_look = false);
    }
}

/// Space: opens Quick Look when it is closed, closes it when it is open (the `QuickLook` action).
pub fn toggle(window: &mut Window, cx: &mut App) {
    if is_open(window, cx) {
        close(window, cx);
    } else {
        open(window, cx);
    }
}

/// Makes Quick Look match the explorer's flag: opens it when the model asked for it (a Recycle Bin item is
/// looked at rather than opened), closes it when the model cleared it (another tab came forward). The
/// workspace calls this after the explorer changes.
pub fn sync(window: &mut Window, cx: &mut App) {
    let wanted = explorer::try_explorer(cx).is_some_and(|e| e.read(cx).quick_look);
    let open_now = is_open(window, cx);
    if wanted && !open_now {
        open(window, cx);
    } else if !wanted && open_now {
        close(window, cx);
    }
}
