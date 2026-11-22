//! The "…" menus of the preview: the header's rare actions, the picture's spare chips, the text's encodings.
//!
//! Each is a [`MenuView`] drawn from [`Node`]s, as every other menu of commands is. The rows are told apart by
//! the id they carry ([`pick`]): the owner that opened the menu is the one that hears the choice, so nothing
//! here goes through the menus' host.

use crate::components::menus::{visible_items, Entry, MenuView, MenuViewEvent, Node, Run, Task};
use crate::components::preview::cover;
use crate::kit::prelude::*;
use gpui_kit::{anchored, deferred, Anchor, Subscription};

/// The opener of the rows made here: no host matches on it, the owner of the menu reads the id.
const OWNER: &str = "preview";

/// What a row of an owner's menu does: `id` says which.
pub fn pick(id: &str) -> Run {
    Run::Do(Task::OpenIn {
        opener: OWNER.into(),
        name: id.into(),
        path: String::new(),
    })
}

/// The id a chosen row carries.
pub fn id_of(run: &Run) -> Option<&str> {
    match run {
        Run::Do(Task::OpenIn { opener, name, .. }) if opener == OWNER => Some(name),
        _ => None,
    }
}

/// A row of such a menu.
pub fn row_of(id: &str, label: SharedString, glyph: IconName) -> Entry {
    Entry::new(label, glyph, pick(id))
}

/// A menu on screen: where it opens, and the listening for what is chosen.
pub struct OpenMore {
    view: Entity<MenuView>,
    at: Point<Pixels>,
    _subscription: Subscription,
}

impl OpenMore {
    /// The menu, for tests.
    #[cfg(test)]
    pub fn view(&self) -> &Entity<MenuView> {
        &self.view
    }
}

/// Opens a menu of `nodes` at `at`, taking the keyboard. `heard` is told the id of the row chosen, or None when
/// the menu was closed with nothing chosen; the owner drops its [`OpenMore`] then. None when nothing is left to
/// show.
pub fn open<T: 'static>(
    nodes: Vec<Node>,
    label: SharedString,
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut Context<T>,
    heard: impl Fn(&mut T, Option<String>, &mut Window, &mut Context<T>) + 'static,
) -> Option<OpenMore> {
    let nodes = visible_items(nodes);
    if nodes.is_empty() {
        return None;
    }
    let view = cx.new(|cx| MenuView::new(nodes, label, cx));
    view.update(cx, |view, cx| view.focus(window, cx));
    let subscription = cx.subscribe_in(&view, window, move |this, _, event: &MenuViewEvent, window, cx| {
        // A native preview draws above GPUI: it comes back now that the menu is gone.
        crate::components::preview::uncover(cx);
        match event {
            MenuViewEvent::Chose(run) => heard(this, id_of(run).map(str::to_string), window, cx),
            MenuViewEvent::Dismissed => heard(this, None, window, cx),
        }
    });
    // A native preview draws above GPUI: it steps aside while the menu covers it.
    cover(cx);
    Some(OpenMore {
        view,
        at,
        _subscription: subscription,
    })
}

/// The menu as an element, outside whatever clips its owner. It opens below `at`, toward the start edge (the
/// end edge in Arabic: these buttons sit at the end of their line), and stays inside the window.
pub fn element(open: &OpenMore) -> impl IntoElement {
    let rtl = crate::i18n::is_rtl();
    deferred(
        anchored()
            .position(open.at)
            .anchor(if rtl { Anchor::TopLeft } else { Anchor::TopRight })
            .snap_to_window_with_margin(px(8.))
            .child(open.view.clone()),
    )
    .with_priority(gpui_kit::base::POPUP_PRIORITY)
}

/// Where a menu opens for a click on a button: just under the click.
pub fn below(event: &ClickEvent) -> Point<Pixels> {
    event.position() + point(px(0.), px(16.))
}
