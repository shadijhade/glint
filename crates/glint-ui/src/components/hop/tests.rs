//! Hop mode: the registry, the order hints are handed out in, and the keys, in a small window of rows to hop to.

use super::*;
use crate::testing;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::cell::RefCell;

/// A window of rows (the first column "files", the second "more") and a tab, with the layer over it.
struct Probe {
    overlay: Entity<HopOverlay>,
    hits: Rc<RefCell<Vec<String>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let names = ["alpha", "beta", "gamma", "delta"];
        let row = |kind: Kind, group: u64, name: &'static str, at: usize| {
            let hits = self.hits.clone();
            mark(div().id((name, at)).w(px(200.)).h(px(30.)).child(name), move || {
                Mark::new(kind, group, name, move |_, _| hits.borrow_mut().push(name.to_string()))
            })
        };
        div()
            .id("probe")
            .relative()
            .size_full()
            .flex()
            .flex_row()
            .child(
                col()
                    .w(px(300.))
                    .children(names.iter().enumerate().map(|(i, name)| row(Kind::Row, 1, name, i))),
            )
            .child(
                col()
                    .w(px(300.))
                    .child(row(Kind::Tab, 2, "tab", 9))
                    .child(row(Kind::Row, 3, "other", 10)),
            )
            .child(self.overlay.clone())
            .on_action(cx.listener(|this, _: &crate::actions::HopMode, window, cx| toggle(&this.overlay, window, cx)))
            .key_context("Workspace")
    }
}

fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<Probe>, Rc<RefCell<Vec<String>>>) {
    let hits = Rc::new(RefCell::new(Vec::new()));
    cx.update(testing::init);
    let seen = hits.clone();
    let (mut visual, probe) = testing::open_window(cx, move |window, cx| {
        let overlay = cx.new(|cx| HopOverlay::new(window, cx));
        cx.new(|_| Probe { overlay, hits: seen })
    });
    visual.update(|window, cx| window.render_frame(cx));
    (visual, probe, hits)
}

/// Turns the mode on and brings it to the frozen state, as the frame after the first one does in the app.
fn start(visual: &mut VisualTestContext, probe: &Entity<Probe>) {
    visual.update(|window, cx| {
        let overlay = probe.read(cx).overlay.clone();
        toggle(&overlay, window, cx);
        window.render_frame(cx);
        overlay.update(cx, |overlay, cx| overlay.freeze(window, cx));
        window.render_frame(cx);
    });
}

fn type_keys(visual: &mut VisualTestContext, keys: &str) {
    for key in keys.chars() {
        visual.update(|window, cx| window.press(&key.to_string(), cx));
    }
}

fn on(visual: &mut VisualTestContext) -> bool {
    visual.update(|_, cx| active() && !matches!(cx.global::<Hop>().phase, Phase::Off))
}

#[gpui_kit::test]
fn nothing_is_marked_and_nothing_is_drawn_while_the_mode_is_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, _probe, _) = open(cx);
    assert!(!active());
    visual.update(|window, cx| window.render_frame(cx));
    assert!(visual.update(|_, cx| cx.global::<Hop>().spots.is_empty()));
    assert!(offered(&mut visual).is_empty());
    visual.update(|window, _| assert!(window.try_find_by_label("Hop mode").is_none()));
}

#[gpui_kit::test]
fn the_mode_offers_every_target_on_screen_tabs_first_then_the_rows_pane_by_pane(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, probe, _) = open(cx);
    start(&mut visual, &probe);
    assert!(on(&mut visual));
    let shown = offered(&mut visual);
    let names: Vec<_> = shown.iter().map(|(_, _, name)| name.as_str()).collect();
    assert_eq!(names, ["tab", "alpha", "beta", "gamma", "delta", "other"]);
    let hints: Vec<_> = shown.iter().map(|(hint, _, _)| hint.as_str()).collect();
    assert_eq!(hints, ["a", "s", "d", "f", "g", "h"], "the home row, in order");
}

#[gpui_kit::test]
fn the_layer_has_a_role_a_name_and_says_how_to_use_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, probe, _) = open(cx);
    start(&mut visual, &probe);
    visual.update(|window, _| {
        let layer = window.try_find_by_label("Hop mode").expect("the layer is named");
        assert_eq!(layer.role(), Some(Role::Group));
        assert!(window
            .try_find_by_label("Type the letters on a badge to go there. Backspace takes one back. Escape cancels.")
            .is_some_and(|usage| usage.role() == Some(Role::Status)));
        let badge = window
            .try_find_by_label("S: alpha")
            .expect("every badge says its letters and its target");
        assert_eq!(badge.role(), Some(Role::Button));
    });
}

#[gpui_kit::test]
fn typing_a_hint_acts_like_a_click_and_ends_the_mode(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, probe, hits) = open(cx);
    start(&mut visual, &probe);
    type_keys(&mut visual, "d");
    assert_eq!(*hits.borrow(), ["beta"]);
    assert!(!on(&mut visual));
    assert!(!active(), "the views stop marking");
}

#[gpui_kit::test]
fn escape_a_click_a_stray_key_and_the_chord_again_each_cancel(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, probe, hits) = open(cx);
    for cancel in ["escape", "1", "enter", "ctrl-a", "backspace"] {
        start(&mut visual, &probe);
        assert!(on(&mut visual), "{cancel}");
        visual.update(|window, cx| window.press(cancel, cx));
        assert!(!on(&mut visual), "{cancel} cancels");
    }
    // A click anywhere.
    start(&mut visual, &probe);
    visual.update(|window, cx| window.click("hop-overlay", cx));
    assert!(!on(&mut visual), "a click cancels");
    // Alt+J again.
    start(&mut visual, &probe);
    visual.update(|window, cx| window.press("alt-j", cx));
    assert!(!on(&mut visual), "the chord toggles");
    assert!(hits.borrow().is_empty(), "nothing was acted on");
}

#[gpui_kit::test]
fn many_targets_get_two_letters_that_narrow_as_they_are_typed_and_backspace_takes_one_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let hits = Rc::new(RefCell::new(Vec::new()));
    cx.update(testing::init);
    let seen = hits.clone();
    let (mut visual, probe) = testing::open_window(cx, move |window, cx| {
        let overlay = cx.new(|cx| HopOverlay::new(window, cx));
        cx.new(|_| Many { overlay, hits: seen })
    });
    visual.update(|window, cx| {
        let overlay = probe.read(cx).overlay.clone();
        toggle(&overlay, window, cx);
        window.render_frame(cx);
        overlay.update(cx, |overlay, cx| overlay.freeze(window, cx));
        window.render_frame(cx);
    });
    let shown = offered(&mut visual);
    assert_eq!(shown.len(), 40);
    let double = shown
        .iter()
        .find(|(hint, _, _)| hint.len() == 2)
        .cloned()
        .expect("two-letter hints");
    // The first letter narrows: the others fade, nothing has acted yet.
    let first = &double.0[..1];
    type_keys(&mut visual, first);
    assert!(on(&mut visual));
    assert!(hits.borrow().is_empty());
    // Backspace takes it back, and the mode stays.
    visual.update(|window, cx| window.press("backspace", cx));
    assert!(on(&mut visual));
    assert_eq!(visual.update(|_, cx| cx.global::<Hop>().typed.clone()), "");
    type_keys(&mut visual, &double.0);
    assert_eq!(hits.borrow().as_slice(), std::slice::from_ref(&double.2));
    assert!(!on(&mut visual));
}

/// Forty rows in one column.
struct Many {
    overlay: Entity<HopOverlay>,
    hits: Rc<RefCell<Vec<String>>>,
}

impl Render for Many {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(col().children((0..40usize).map(|i| {
                let hits = self.hits.clone();
                let name = format!("row {i}");
                let shown = name.clone();
                mark(div().id(("many", i)).h(px(16.)).child(shown), move || {
                    Mark::new(Kind::Row, 1, name.clone(), move |_, _| {
                        hits.borrow_mut().push(name.clone())
                    })
                })
            })))
            .child(self.overlay.clone())
    }
}

#[test]
fn hints_go_to_tabs_then_crumbs_then_places_then_rows_each_group_nearest_the_start_edge_first() {
    fn spot(kind: Kind, group: u64, x: f32, y: f32) -> Spot {
        Spot {
            mark: Mark::new(kind, group, format!("{kind:?} {group} {x} {y}"), |_, _| {}),
            bounds: Bounds::new(point(px(x), px(y)), size(px(10.), px(10.))),
        }
    }
    let spots = [
        spot(Kind::Row, 8, 500., 10.),
        spot(Kind::Row, 7, 300., 40.),
        spot(Kind::Row, 7, 300., 10.),
        spot(Kind::Tile, 8, 520., 10.),
        spot(Kind::Place, 1, 0., 80.),
        spot(Kind::Crumb, 2, 100., 0.),
        spot(Kind::Tab, 3, 0., 30.),
    ];
    let names = |rtl| {
        ordered(
            spots
                .iter()
                .map(|s| Spot {
                    mark: s.mark.clone(),
                    bounds: s.bounds,
                })
                .collect(),
            rtl,
        )
        .iter()
        .map(|s| s.mark.name.to_string())
        .collect::<Vec<_>>()
    };
    assert_eq!(
        names(false),
        [
            "Tab 3 0 30",
            "Crumb 2 100 0",
            "Place 1 0 80",
            "Row 7 300 10",
            "Row 7 300 40",
            "Row 8 500 10",
            "Tile 8 520 10"
        ]
    );
    // Mirrored, the pane at the right is the first, and a row reads from its right.
    assert_eq!(names(true)[3], "Tile 8 520 10");
}

#[gpui_kit::test]
fn a_target_mostly_cut_off_by_its_list_is_not_offered(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, probe, _) = open(cx);
    visual.update(|window, cx| {
        let overlay = probe.read(cx).overlay.clone();
        toggle(&overlay, window, cx);
        // Marks recorded outside the window's mask, as a row scrolled off the end of its list is.
        let whole = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        let id = window.window_handle().window_id();
        let below = Bounds::new(point(px(0.), whole.size.height - px(4.)), size(px(100.), px(30.)));
        note(Mark::new(Kind::Row, 5, "cut", |_, _| {}), below, whole, id, cx);
        let in_view = Bounds::new(point(px(0.), whole.size.height - px(20.)), size(px(100.), px(30.)));
        note(Mark::new(Kind::Row, 5, "most of it", |_, _| {}), in_view, whole, id, cx);
        let names: Vec<_> = cx
            .global::<Hop>()
            .spots
            .iter()
            .map(|s| s.mark.name.to_string())
            .collect();
        assert_eq!(names, ["most of it"]);
    });
}
