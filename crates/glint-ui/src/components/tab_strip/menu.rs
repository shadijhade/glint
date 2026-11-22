//! The chrome's own menus (a tab's, a pane's, a portal's, the workspaces'), drawn as `MenuView`s like every other
//! menu in the window.
//!
//! These menus run closures of their own (they act on a tab, a portal, a workspace), while a `MenuView` hands back
//! plain data (a [`Run`]). So each row carries a [`pick`]: a tag and an argument wrapped as a `Run`, and whoever
//! opened the menu reads it back with [`Chose::tag`] and does the work. Nothing outside the chrome ever sees such a
//! run, because the menu's events come to the chrome alone (never to the window's menu host).
//!
//! The same module keeps count of the menus it has open, so that the rows the menus open over (a tab, a place, a
//! breadcrumb) show no tooltip while one is up: [`tip`] is the tooltip builder those rows use.

use crate::components::menus::{Entry, MenuView, MenuViewEvent, Node, Run, Task};
use crate::kit::prelude::*;
use gpui_kit::AnyView;

/// What a chrome row's [`Run`] is marked with, so that it can't be taken for anything a menu host does.
const OPENER: &str = "chrome";

/// The run of a chrome row: `tag` says what to do, `arg` which tab, portal or workspace it is for.
pub(crate) fn pick(tag: &str, arg: impl Into<String>) -> Run {
    Run::Do(Task::OpenIn {
        opener: OPENER.to_string(),
        name: tag.to_string(),
        path: arg.into(),
    })
}

/// A row that does `pick(tag, arg)`.
pub(crate) fn row(label: SharedString, icon: IconName, tag: &str, arg: impl Into<String>) -> Entry {
    Entry::new(label, icon, pick(tag, arg))
}

/// What a chrome menu's choice was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Chose {
    pub tag: String,
    pub arg: String,
}

impl Chose {
    /// What `run` says, when it is a chrome row's.
    pub(crate) fn of(run: &Run) -> Option<Chose> {
        match run {
            Run::Do(Task::OpenIn { opener, name, path }) if opener == OPENER => Some(Chose {
                tag: name.clone(),
                arg: path.clone(),
            }),
            _ => None,
        }
    }
}

/// A chrome menu on screen: the view, and where it hangs.
pub(crate) struct Popup {
    at: Point<Pixels>,
    view: Entity<MenuView>,
    _subscription: Subscription,
}

/// The menus of this module that are on screen, to tell [`tip`] to hold back.
#[derive(Default)]
struct Open(Vec<WeakEntity<MenuView>>);

impl Global for Open {}

/// Whether one of the chrome's menus is on screen.
pub(crate) fn is_open(cx: &App) -> bool {
    cx.try_global::<Open>()
        .is_some_and(|open| open.0.iter().any(|view| view.upgrade().is_some()))
}

impl Popup {
    /// Shows `nodes` at `at`, taking the keyboard. `done` hears what was chosen (`None`: Escape or a press outside);
    /// it drops the popup (and gives the keyboard back) itself.
    pub(crate) fn open<T: 'static>(
        nodes: Vec<Node>,
        label: SharedString,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<T>,
        done: impl Fn(&mut T, Option<Chose>, &mut Window, &mut Context<T>) + 'static,
    ) -> Option<Popup> {
        let nodes = crate::components::menus::visible_items(nodes);
        if nodes.is_empty() {
            return None;
        }
        let view = cx.new(|cx| MenuView::new(nodes, label, cx));
        view.update(cx, |view, cx| view.focus(window, cx));
        let subscription = cx.subscribe_in(&view, window, move |this, _, event: &MenuViewEvent, window, cx| {
            let chose = match event {
                MenuViewEvent::Chose(run) => Chose::of(run),
                MenuViewEvent::Dismissed => None,
            };
            done(this, chose, window, cx);
        });
        let weak = view.downgrade();
        init(cx);
        cx.update_global::<Open, _>(|open, _| {
            open.0.retain(|view| view.upgrade().is_some());
            open.0.push(weak);
        });
        Some(Popup {
            at,
            view,
            _subscription: subscription,
        })
    }

    /// The menu, to put among the owner's children (outside anything that clips).
    pub(crate) fn element(&self) -> impl IntoElement {
        let rtl = crate::i18n::is_rtl();
        deferred(
            anchored()
                .position(self.at)
                .anchor(if rtl { Anchor::TopRight } else { Anchor::TopLeft })
                .snap_to_window_with_margin(px(8.))
                .child(self.view.clone()),
        )
        .with_priority(gpui_kit::base::POPUP_PRIORITY)
    }

    /// The view, for tests.
    #[cfg(test)]
    pub(crate) fn view(&self) -> &Entity<MenuView> {
        &self.view
    }
}

/// Makes the global the count lives in; called once as the chrome's first menu opens.
pub(crate) fn init(cx: &mut App) {
    if cx.try_global::<Open>().is_none() {
        cx.set_global(Open::default());
    }
}

/// A tooltip that draws nothing.
struct Quiet;

impl Render for Quiet {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// The tooltip of a row that a menu opens over: the kit's, and nothing while any menu is on screen or about to be
/// (the pointer rests where the right-click was, so the row's name would open over the menu's rows).
pub(crate) fn tip(text: SharedString) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        if is_open(cx) {
            return cx.new(|_| Quiet).into();
        }
        crate::kit::tooltip_under(text.clone(), window, cx)
    }
}
