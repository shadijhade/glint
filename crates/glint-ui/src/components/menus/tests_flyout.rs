//! Tests of the menu's height, its flyouts (submenus beside the menu), how it opens, and the tooltips under it.
//! Card `menuview` of the 0.16 polish wave writes these; `tests_view.rs` has the rig they use.

use super::spec::{Node, Run, Tone};
use super::tests_view::{badge, chose, head, item, menu_with, rig, Rig};
use super::view::{inside_triangle, MenuView, MenuViewEvent, GRACE, OPEN_AFTER};
use crate::kit::prelude::*;
use crate::testing;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

/// A menu placed at `left`, `top` in the window (as the host places it), to see where its flyouts go.
struct Placed {
    view: Entity<MenuView>,
    left: f32,
    top: f32,
}

impl Render for Placed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .absolute()
                .left(px(self.left))
                .top(px(self.top))
                .child(self.view.clone()),
        )
    }
}

fn rig_at(cx: &mut TestAppContext, nodes: Vec<Node>, left: f32, top: f32) -> Rig {
    cx.update(testing::init);
    let (mut visual, placed) = testing::open_window(cx, move |window, cx| {
        let view = cx.new(|cx| MenuView::new(nodes, "Item menu".into(), cx));
        let focus = view.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        cx.new(|_| Placed { view, left, top })
    });
    let view = placed.read_with(&visual, |placed, _| placed.view.clone());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|window, cx| {
        window.activate_window();
        cx.subscribe(&view, move |_, event: &MenuViewEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let mut rig = Rig { visual, view, events };
    rig.frame();
    rig
}

impl Rig {
    fn bounds(&mut self, label: &str) -> Bounds<Pixels> {
        self.visual.update(|window, _| window.find_by_label(label).bounds())
    }

    /// Moves the pointer to the middle of the row `label`.
    fn hover(&mut self, label: &str) {
        let at = self.bounds(label).center();
        self.point(at);
    }

    fn point(&mut self, at: Point<Pixels>) {
        self.visual.simulate_mouse_move(at, None, Default::default());
        self.frame();
    }

    /// Lets `ms` milliseconds go by.
    fn wait(&mut self, ms: u64) {
        self.visual.executor().advance_clock(Duration::from_millis(ms));
        self.frame();
        self.frame();
    }

    fn active_level(&mut self) -> usize {
        self.view.read_with(&self.visual, |v, _| v.active_level())
    }

    fn expanded(&mut self, label: &str) -> Option<bool> {
        self.visual.update(|window, _| window.find_by_label(label).expanded())
    }
}

/// Open, a submenu of `inner` rows named "Inner 0", "Inner 1" …, and Delete.
fn with_flyout(inner: usize) -> Vec<Node> {
    vec![
        item("Open"),
        Node::Menu {
            label: "More".into(),
            icon: None,
            disabled: false,
            items: (0..inner).map(|i| item(&format!("Inner {i}"))).collect(),
        },
        item("Delete"),
    ]
}

#[gpui_kit::test]
fn a_long_menu_stays_within_the_cap_and_scrolls_inside(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let nodes: Vec<Node> = (0..40).map(|i| item(&format!("Row {i}"))).collect();
    let mut rig = rig(cx, nodes);
    // 70 % of the 700 px window.
    let menu = rig.bounds("Item menu");
    assert!(menu.size.height <= px(491.), "{:?}", menu.size);
    assert!(
        menu.size.height > px(300.),
        "it is as tall as the cap lets it be: {:?}",
        menu.size
    );
    rig.press("end");
    rig.frame();
    let menu = rig.bounds("Item menu");
    let last = rig.bounds("Row 39");
    assert!(
        last.bottom() <= menu.bottom() && last.top() >= menu.top(),
        "End shows the last row"
    );
    let first = rig.bounds("Row 0");
    assert!(
        first.bottom() <= menu.top() + px(2.),
        "the first row scrolled out above"
    );
    // A shorter window makes it shorter: 70 % of 300 px.
    rig.visual.simulate_resize(size(px(800.), px(300.)));
    rig.frame();
    rig.frame();
    let short = rig.bounds("Item menu").size.height;
    assert!(short <= px(211.), "{short:?}");
}

#[gpui_kit::test]
fn the_head_and_the_strip_stay_put_while_the_rows_scroll(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut nodes = menu_with(head(vec![]));
    nodes.extend((0..40).map(|i| item(&format!("Row {i}"))));
    let mut rig = rig(cx, nodes);
    let (head_at, strip_at) = (rig.bounds("setup.exe").origin, rig.bounds("Cut").origin);
    rig.press("end");
    rig.frame();
    assert_eq!(rig.bounds("setup.exe").origin, head_at);
    assert_eq!(rig.bounds("Cut").origin, strip_at);
    assert!(rig.bounds("Item menu").size.height <= px(491.));
}

#[gpui_kit::test]
fn the_pointer_resting_on_a_submenus_row_opens_its_flyout_after_a_moment(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    rig.hover("More");
    assert_eq!(rig.depth(), 1, "not at once");
    rig.wait(OPEN_AFTER.as_millis() as u64 - 50);
    assert_eq!(rig.depth(), 1);
    rig.wait(60);
    assert_eq!(rig.depth(), 2);
    assert!(rig.drawn("Inner 0"));
    assert_eq!(rig.active_level(), 0, "the keyboard stays where it was");
    assert_eq!(rig.selected().as_deref(), Some("More"));
    // Resting on another row (not on the way to the flyout) closes it at once.
    rig.hover("Delete");
    assert_eq!(rig.depth(), 1);
    assert!(!rig.drawn("Inner 0"));
}

#[gpui_kit::test]
fn the_pointer_leaving_a_row_before_the_moment_is_up_opens_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    rig.hover("More");
    rig.wait(100);
    rig.hover("Open");
    rig.wait(300);
    assert_eq!(rig.depth(), 1);
}

#[gpui_kit::test]
fn moving_toward_the_flyout_across_another_row_keeps_it_for_a_grace_period(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(8), 100., 20.);
    rig.hover("More");
    rig.wait(160);
    assert_eq!(rig.depth(), 2);
    // The pointer moves along the row, then down onto Delete at its end, toward the flyout.
    let more = rig.bounds("More");
    rig.point(point(more.left() + px(30.), more.center().y));
    let delete = rig.bounds("Delete");
    rig.point(point(delete.right() - px(4.), delete.center().y));
    assert_eq!(rig.depth(), 2, "on its way");
    rig.wait(GRACE.as_millis() as u64 - 100);
    assert_eq!(rig.depth(), 2);
    rig.wait(150);
    assert_eq!(rig.depth(), 1, "resting on Delete closes it");
}

#[gpui_kit::test]
fn the_pointer_going_into_the_flyout_keeps_it_and_choosing_there_chooses(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    rig.hover("More");
    rig.wait(160);
    rig.hover("Inner 1");
    rig.wait(500);
    assert_eq!(rig.depth(), 2);
    assert_eq!(rig.active_level(), 1);
    assert_eq!(rig.selected().as_deref(), Some("Inner 1"));
    rig.click(("menu-row", 1001usize));
    assert_eq!(
        rig.events(),
        vec![chose("Inner 1")],
        "a press inside the flyout is not outside the menu"
    );
}

#[gpui_kit::test]
fn the_arrows_and_escape_go_into_a_flyout_and_back_to_its_row(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some("More"));
    rig.press("right");
    assert_eq!((rig.depth(), rig.active_level()), (2, 1));
    assert_eq!(rig.selected().as_deref(), Some("Inner 0"));
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some("Inner 1"));
    rig.press("left");
    assert_eq!((rig.depth(), rig.active_level()), (1, 0));
    assert_eq!(
        rig.selected().as_deref(),
        Some("More"),
        "the keyboard is back on the row"
    );
    rig.press("enter");
    assert_eq!(rig.depth(), 2);
    // Escape goes back to the row first, and closes the menu from the top.
    rig.press("escape");
    assert_eq!(rig.depth(), 1);
    assert_eq!(rig.events(), vec![]);
    rig.press("escape");
    assert_eq!(rig.events(), vec![MenuViewEvent::Dismissed]);
}

#[gpui_kit::test]
fn the_parent_row_is_marked_open_while_its_flyout_is_open(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    assert_eq!(rig.expanded("More"), Some(false));
    rig.press("down");
    rig.press("right");
    assert_eq!(rig.expanded("More"), Some(true));
    rig.press("down");
    assert_eq!(rig.expanded("More"), Some(true));
    rig.press("left");
    assert_eq!(rig.expanded("More"), Some(false));
}

#[gpui_kit::test]
fn a_flyout_opens_toward_the_end_and_flips_where_there_is_no_room(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    rig.press("down");
    rig.press("right");
    rig.frame();
    let (menu, flyout) = (rig.bounds("Item menu"), rig.bounds("More ›"));
    assert!(flyout.left() >= menu.right() - px(6.), "{menu:?} {flyout:?}");
    assert!(flyout.right() <= px(1000.));
}

#[gpui_kit::test]
fn a_flyout_near_the_right_edge_opens_on_the_start_side(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 760., 20.);
    rig.press("down");
    rig.press("right");
    rig.frame();
    let (menu, flyout) = (rig.bounds("Item menu"), rig.bounds("More ›"));
    assert!(flyout.right() <= menu.left() + px(6.), "{menu:?} {flyout:?}");
    assert!(flyout.left() >= px(0.));
}

#[gpui_kit::test]
fn mirrored_the_flyout_opens_toward_the_left(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 500., 20.);
    testing::set_rtl(true);
    rig.frame();
    // In Arabic the arrow toward the end of the line is Left.
    rig.press("down");
    rig.press("left");
    rig.frame();
    let (menu, flyout) = (rig.bounds("Item menu"), rig.bounds("More ›"));
    assert!(flyout.right() <= menu.left() + px(6.), "{menu:?} {flyout:?}");
    drop(rig);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn mirrored_at_the_left_edge_the_flyout_flips_to_the_right(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 10., 20.);
    testing::set_rtl(true);
    rig.frame();
    rig.press("down");
    rig.press("left");
    rig.frame();
    let (menu, flyout) = (rig.bounds("Item menu"), rig.bounds("More ›"));
    assert!(flyout.left() >= menu.right() - px(6.), "{menu:?} {flyout:?}");
    drop(rig);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_flyout_is_capped_like_the_menu_and_scrolls(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(40), 100., 20.);
    rig.press("down");
    rig.press("right");
    rig.frame();
    let flyout = rig.bounds("More ›");
    assert!(flyout.size.height <= px(491.), "{:?}", flyout.size);
    assert!(flyout.size.height > px(300.));
    rig.press("end");
    rig.frame();
    let last = rig.bounds("Inner 39");
    assert!(last.bottom() <= rig.bounds("More ›").bottom());
}

#[gpui_kit::test]
fn typing_hides_the_flyouts_and_finds_rows_inside_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    rig.press("down");
    rig.press("right");
    assert!(rig.drawn("Inner 1"));
    rig.type_text("inner 2");
    assert!(!rig.drawn("More ›"), "no flyout beside the filtered list");
    assert_eq!(rig.found().first().map(String::as_str), Some("Inner 2"));
    rig.press("enter");
    assert_eq!(rig.events(), vec![chose("Inner 2")]);
}

#[gpui_kit::test]
fn a_press_outside_every_column_dismisses_the_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig_at(cx, with_flyout(3), 100., 20.);
    rig.press("down");
    rig.press("right");
    rig.frame();
    rig.visual.simulate_click(point(px(900.), px(600.)), Default::default());
    rig.frame();
    assert_eq!(rig.events(), vec![MenuViewEvent::Dismissed]);
}

#[gpui_kit::test]
fn the_head_has_a_line_of_badges_only_when_there_are_badges(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut plain = head(vec![]);
    plain.changed = None;
    let mut rig_plain = rig(cx, menu_with(plain));
    let without = rig_plain.bounds("setup.exe").size.height;
    drop(rig_plain);
    let mut flagged = head(vec![badge("In use by Word", Tone::Warn, Run::Nothing)]);
    flagged.changed = None;
    let mut rig_flagged = rig(cx, menu_with(flagged));
    let with = rig_flagged.bounds("setup.exe").size.height;
    assert!(with > without + px(10.), "{with:?} against {without:?}");
    assert!(
        without <= px(62.),
        "a head with no badges is two lines and a picture: {without:?}"
    );
}

#[gpui_kit::test]
fn badges_that_come_late_do_not_change_a_head_with_no_line_for_them(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut empty = head(vec![]);
    empty.changed = None;
    let mut rig = rig(cx, menu_with(empty));
    let before = rig.bounds("setup.exe").size;
    rig.view.update(&mut rig.visual, |view, cx| {
        view.set_head(head(vec![badge("In use by Word", Tone::Warn, Run::Nothing)]), cx)
    });
    rig.frame();
    assert_eq!(rig.bounds("setup.exe").size, before);
}

#[gpui_kit::test]
fn a_tooltip_asked_for_before_a_menu_opened_is_not_shown_and_one_asked_for_later_is(cx: &mut TestAppContext) {
    let _guard = testing::lock();

    struct Tip;
    impl Render for Tip {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("tip")
                .role(Role::Label)
                .aria_label("TIP")
                .test_support()
                .child("TIP")
        }
    }
    struct Under;
    impl Render for Under {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(
                div()
                    .id("under")
                    .test_support()
                    .w(px(300.))
                    .h(px(40.))
                    .tooltip(move |_, cx| cx.new(|_| Tip).into()),
            )
        }
    }
    cx.update(testing::init);
    let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Under));
    fn frame(visual: &mut VisualTestContext) {
        visual.run_until_parked();
        visual.update(|window, cx| window.render_frame(cx));
    }
    fn shown(visual: &mut VisualTestContext) -> bool {
        visual.update(|window, _| window.try_find_by_label("TIP").is_some())
    }
    fn rest(visual: &mut VisualTestContext, ms: u64) {
        visual.executor().advance_clock(Duration::from_millis(ms));
        frame(visual);
        frame(visual);
    }
    frame(&mut visual);
    visual.simulate_mouse_move(point(px(50.), px(20.)), None, Default::default());
    frame(&mut visual);
    rest(&mut visual, 700);
    assert!(shown(&mut visual), "the tooltip comes after its delay");
    // A menu opens: what is up is gone.
    visual.update(|window, _| window.dismiss_tooltips());
    frame(&mut visual);
    assert!(!shown(&mut visual), "dismissed");
    // One whose delay was running when the menu opened never shows.
    visual.simulate_mouse_move(point(px(400.), px(300.)), None, Default::default());
    frame(&mut visual);
    visual.simulate_mouse_move(point(px(60.), px(20.)), None, Default::default());
    frame(&mut visual);
    visual.update(|window, _| window.dismiss_tooltips());
    rest(&mut visual, 700);
    assert!(!shown(&mut visual), "asked for before the menu, so not shown");
    // Leaving and coming back asks again, under the new count: it works as usual.
    visual.simulate_mouse_move(point(px(400.), px(300.)), None, Default::default());
    frame(&mut visual);
    visual.simulate_mouse_move(point(px(70.), px(20.)), None, Default::default());
    frame(&mut visual);
    rest(&mut visual, 700);
    assert!(shown(&mut visual));
}

#[gpui_kit::test]
fn a_menu_dismisses_the_tooltips_when_it_opens(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, vec![item("Open")]);
    rig.frame();
    let counted = rig.visual.update(|window, _| window.tooltip_epoch());
    assert!(counted >= 1, "the menu counted its opening: {counted}");
}

#[gpui_kit::test]
fn the_safe_triangle_holds_what_is_between_the_pointer_and_the_flyout(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let p = |x: f32, y: f32| point(px(x), px(y));
    let (apex, a, b) = (p(0., 50.), p(100., 0.), p(100., 200.));
    assert!(inside_triangle(p(90., 60.), apex, a, b));
    assert!(inside_triangle(p(50., 50.), apex, a, b));
    assert!(
        !inside_triangle(p(10., 150.), apex, a, b),
        "too steep for the way there"
    );
    assert!(!inside_triangle(p(-10., 50.), apex, a, b), "behind the apex");
    let _ = cx;
}

#[gpui_kit::test]
fn a_kit_popup_menu_with_no_height_of_its_own_is_capped_and_scrolls(cx: &mut TestAppContext) {
    use crate::kit::{PopupMenu, PopupMenuItem};
    let _guard = testing::lock();

    struct Wrap(Entity<PopupMenu>);
    impl Render for Wrap {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.0.clone())
        }
    }
    cx.update(testing::init);
    let (mut visual, _) = testing::open_window(cx, |window, cx| {
        let menu = PopupMenu::build(window, cx, |mut menu, _, _| {
            for i in 0..60 {
                menu = menu.item(PopupMenuItem::new(format!("Entry {i}")));
            }
            menu
        });
        cx.new(|_| Wrap(menu))
    });
    visual.run_until_parked();
    visual.update(|window, cx| window.render_frame(cx));
    visual.update(|window, cx| window.render_frame(cx));
    let height = visual.update(|window, _| window.find("popup-menu").bounds().size.height);
    // 70 % of the 700 px window, never the whole of it.
    assert!(height <= px(491.), "{height:?}");
    assert!(height > px(300.), "{height:?}");
}
