//! Tests of the menu's view: the head, reasons, typing, the slide. Card E1 of docs/gpui/aware writes these.
//!
//! The menus here are built by hand and shown alone in a window, so what is checked is the view and nothing
//! about what the menus hold (`tests.rs` does that).

use super::event::MenuEvent;
use super::spec::{Badge, Entry, Head, HeadPicture, Node, Run, Task, Tone};
use super::view::{MenuView, MenuViewEvent};
use crate::kit::prelude::*;
use crate::{t, testing};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;

/// A row that does something of its own (so no two rows are the same thing), named `label`.
pub(super) fn entry(label: &str) -> Entry {
    Entry::task(label.to_string().into(), IconName::File, Task::OpenTerminal(label.to_string()))
}

pub(super) fn item(label: &str) -> Node {
    Node::Item(entry(label))
}

/// What choosing the row `label` says.
pub(super) fn chose(label: &str) -> MenuViewEvent {
    MenuViewEvent::Chose(Run::Do(Task::OpenTerminal(label.to_string())))
}

pub(super) fn badge(label: &str, tone: Tone, run: Run) -> Badge {
    Badge {
        label: label.to_string().into(),
        icon: IconName::Info,
        tone,
        tip: None,
        run,
    }
}

pub(super) fn head(badges: Vec<Badge>) -> Head {
    Head {
        title: "setup.exe".into(),
        picture: HeadPicture::Folder,
        line: "Program · 84.6 MB".into(),
        badges,
        changed: Some("Changed yesterday".into()),
    }
}

/// A head, a strip of two buttons, then rows.
pub(super) fn menu_with(head: Head) -> Vec<Node> {
    vec![
        Node::Head(head),
        Node::Strip(vec![entry("Cut"), entry("Copy")]),
        item("Open"),
        item("Rename"),
        item("Delete"),
    ]
}

pub(super) struct Rig {
    pub(super) visual: VisualTestContext,
    pub(super) view: Entity<MenuView>,
    pub(super) events: Rc<RefCell<Vec<MenuViewEvent>>>,
}

pub(super) fn rig(cx: &mut TestAppContext, nodes: Vec<Node>) -> Rig {
    cx.update(testing::init);
    let (mut visual, view) = testing::open_window(cx, move |window, cx| {
        let view = cx.new(|cx| MenuView::new(nodes, "Item menu".into(), cx));
        let focus = view.read(cx).focus_handle(cx);
        focus.focus(window, cx);
        view
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|window, cx| {
        window.activate_window();
        cx.subscribe(&view, move |_, event: &MenuViewEvent, _| sink.borrow_mut().push(event.clone()))
            .detach();
    });
    let mut rig = Rig { visual, view, events };
    rig.frame();
    rig
}

impl Rig {
    pub(super) fn frame(&mut self) {
        self.visual.run_until_parked();
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    pub(super) fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.frame();
    }

    /// Types `text`, a key for each letter.
    pub(super) fn type_text(&mut self, text: &str) {
        for letter in text.chars() {
            self.press(&letter.to_string());
        }
    }

    pub(super) fn click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.frame();
    }

    pub(super) fn selected(&mut self) -> Option<String> {
        self.view
            .read_with(&self.visual, |v, _| v.selected_label().map(|s| s.to_string()))
    }

    pub(super) fn depth(&mut self) -> usize {
        self.view.read_with(&self.visual, |v, _| v.depth())
    }

    pub(super) fn found(&mut self) -> Vec<String> {
        self.view
            .read_with(&self.visual, |v, _| v.found_labels().iter().map(|l| l.to_string()).collect())
    }

    pub(super) fn drawn(&mut self, label: &str) -> bool {
        self.visual
            .update(|window, _| window.try_find_by_label(label).is_some())
    }

    pub(super) fn description(&mut self, label: &str) -> Option<String> {
        self.visual
            .update(|window, _| window.find_by_label(label).description().map(str::to_string))
    }

    pub(super) fn events(&self) -> Vec<MenuViewEvent> {
        self.events.borrow().clone()
    }
}

#[gpui_kit::test]
fn a_menu_with_a_head_draws_it_and_describes_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let badges = vec![
        badge("From github.com", Tone::Warn, Run::Nothing),
        badge("Not signed", Tone::Warn, Run::Nothing),
    ];
    let mut rig = rig(cx, menu_with(head(badges)));
    assert!(rig.drawn("setup.exe"));
    let said = rig.description("setup.exe").expect("the head has a description");
    assert_eq!(said, "Program · 84.6 MB, From github.com, Not signed");
    assert!(rig.drawn("From github.com"));
    assert!(rig.drawn("Not signed"));
}

#[gpui_kit::test]
fn the_head_is_not_a_stop_for_the_arrow_keys(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, menu_with(head(vec![])));
    assert_eq!(rig.selected().as_deref(), Some("Open"));
    // Up from the first row goes to the strip's last button, and Down comes back to the first row.
    rig.press("up");
    assert_eq!(rig.selected().as_deref(), Some("Copy"));
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some("Open"));
    // A whole round, and the head is never where the keyboard is.
    for _ in 0..8 {
        rig.press("down");
        assert_ne!(rig.selected().as_deref(), Some("setup.exe"));
    }
    rig.press("home");
    assert_eq!(rig.selected().as_deref(), Some("Cut"));
    rig.press("end");
    assert_eq!(rig.selected().as_deref(), Some("Delete"));
}

#[gpui_kit::test]
fn a_new_head_changes_what_is_drawn_and_leaves_the_keyboard_where_it_was(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, menu_with(head(vec![badge("In use by Word", Tone::Warn, Run::Nothing)])));
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some("Rename"));
    assert!(rig.drawn("In use by Word"));
    let later = head(vec![
        badge("2 identical copies", Tone::Plain, Run::Nothing),
        badge("Signed by Acme", Tone::Good, Run::Nothing),
    ]);
    rig.view.update(&mut rig.visual, |view, cx| view.set_head(later, cx));
    rig.frame();
    assert!(!rig.drawn("In use by Word"));
    assert!(rig.drawn("2 identical copies"));
    assert!(rig.drawn("Signed by Acme"));
    assert_eq!(rig.selected().as_deref(), Some("Rename"), "the keyboard did not move");
}

#[gpui_kit::test]
fn five_badges_draw_two_and_a_plus_three(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let badges = ["One", "Two", "Three", "Four", "Five"]
        .into_iter()
        .map(|label| badge(label, Tone::Plain, Run::Nothing))
        .collect();
    let mut rig = rig(cx, menu_with(head(badges)));
    assert!(rig.drawn("One"));
    assert!(rig.drawn("Two"));
    assert!(!rig.drawn("Three"));
    assert!(!rig.drawn("Four"));
    assert!(!rig.drawn("Five"));
    assert!(rig.drawn("+3"));
    // All five are still in what a screen reader is told of the head.
    let said = rig.description("setup.exe").unwrap_or_default();
    assert!(said.contains("Three, Four, Five"), "{said}");
}

#[gpui_kit::test]
fn exactly_three_badges_are_all_drawn_with_no_plus(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let three = ["One", "Two", "Three"]
        .into_iter()
        .map(|label| badge(label, Tone::Plain, Run::Nothing))
        .collect();
    let mut rig = rig(cx, menu_with(head(three)));
    assert!(rig.drawn("One"));
    assert!(rig.drawn("Three"));
    assert!(!rig.drawn("+1"));
}

#[gpui_kit::test]
fn a_badge_with_something_behind_it_is_a_button_and_the_others_are_text(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let run = Run::Do(Task::OpenTerminal("locks".into()));
    let badges = vec![
        badge("In use by Word", Tone::Warn, run.clone()),
        badge("Hidden", Tone::Plain, Run::Nothing),
    ];
    let mut rig = rig(cx, menu_with(head(badges)));
    let role_of = |rig: &mut Rig, label: &str| rig.visual.update(|window, _| window.find_by_label(label).role());
    assert_eq!(role_of(&mut rig, "In use by Word"), Some(Role::Button));
    assert_ne!(role_of(&mut rig, "Hidden"), Some(Role::Button));
    // Pressing the one that does something chooses it; pressing the other does nothing.
    rig.click(("menu-badge", 1usize));
    assert_eq!(rig.events(), vec![]);
    rig.click(("menu-badge", 0usize));
    assert_eq!(rig.events(), vec![MenuViewEvent::Chose(run)]);
}

#[gpui_kit::test]
fn a_suggested_row_with_a_reason_is_found_by_its_label_and_says_why(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut why = entry("Scan with Defender");
    why.smart = true;
    why.reason = Some("It came from github.com".into());
    let mut plain = entry("Extract here");
    plain.smart = true;
    let mut rig = rig(cx, vec![Node::Heading("Suggested".into()), Node::Item(why), Node::Item(plain), item("Open")]);
    assert_eq!(rig.description("Scan with Defender").as_deref(), Some("It came from github.com"));
    assert_eq!(rig.description("Extract here"), None);
    assert!(rig.drawn("Open"));
    // It is a row like any: chosen with Enter.
    rig.press("enter");
    assert_eq!(rig.events(), vec![chose("Scan with Defender")]);
}

#[gpui_kit::test]
fn a_greyed_row_says_why_and_the_keyboard_passes_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let grey = entry("Paste").disabled_because(true, &Some("Nothing changes inside an archive".into()));
    let mut rig = rig(cx, vec![item("Open"), Node::Item(grey), item("Delete")]);
    assert_eq!(
        rig.description("Paste").as_deref(),
        Some("Nothing changes inside an archive")
    );
    assert_eq!(rig.selected().as_deref(), Some("Open"));
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some("Delete"));
    // Clicking it does nothing.
    rig.click(("menu-row", 1usize));
    assert_eq!(rig.events(), vec![]);
}

#[gpui_kit::test]
fn typing_shows_the_filter_in_place_of_the_head_and_escape_takes_it_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, menu_with(head(vec![])));
    assert!(rig.drawn("setup.exe"));
    rig.type_text("ren");
    assert!(rig.drawn("Filter: ren"));
    assert!(!rig.drawn("setup.exe"), "the head gives way to the filter");
    assert!(!rig.drawn("Cut"), "and so does the strip");
    // Escape takes the typing back: the head is there again and the menu is still open.
    rig.press("escape");
    assert!(rig.drawn("setup.exe"));
    assert!(!rig.drawn("Filter: ren"));
    assert_eq!(rig.events(), vec![]);
    // Escape with nothing typed closes it.
    rig.press("escape");
    assert_eq!(rig.events(), vec![MenuViewEvent::Dismissed]);
}

fn rows_with_ren() -> Vec<Node> {
    vec![item("Open"), item("Rename"), item("Batch rename"), item("Delete")]
}

#[gpui_kit::test]
fn enter_chooses_the_first_found_row(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, rows_with_ren());
    rig.type_text("ren");
    let found = rig.found();
    assert!(found.len() >= 2, "both rows that hold the letters are found: {found:?}");
    assert!(!found.contains(&"Delete".to_string()));
    assert_eq!(rig.selected().as_deref(), Some(found[0].as_str()));
    rig.press("enter");
    assert_eq!(rig.events(), vec![chose(&found[0])]);
}

#[gpui_kit::test]
fn down_then_enter_chooses_the_second_found_row(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, rows_with_ren());
    rig.type_text("ren");
    let found = rig.found();
    assert!(found.len() >= 2, "{found:?}");
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some(found[1].as_str()));
    rig.press("enter");
    assert_eq!(rig.events(), vec![chose(&found[1])]);
}

#[gpui_kit::test]
fn up_from_the_first_found_row_goes_round_to_the_last(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, rows_with_ren());
    rig.type_text("ren");
    let all = rig.found();
    rig.press("up");
    assert_eq!(rig.selected().as_deref(), all.last().map(String::as_str));
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), all.first().map(String::as_str));
}

#[gpui_kit::test]
fn a_found_row_is_named_with_where_it_is_and_chosen_by_a_click(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let nodes = vec![
        item("Open"),
        Node::Menu {
            label: "Compress to".into(),
            icon: None,
            disabled: false,
            items: vec![item("With a password")],
        },
    ];
    let mut rig = rig(cx, nodes);
    rig.type_text("pass");
    assert!(rig.drawn("Compress to › With a password"));
    rig.click(("menu-found", 0usize));
    assert_eq!(rig.events(), vec![chose("With a password")]);
}

#[gpui_kit::test]
fn backspace_takes_a_letter_off_and_with_none_left_the_rows_are_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let nodes = vec![
        item("Open"),
        Node::Menu {
            label: "More".into(),
            icon: None,
            disabled: false,
            items: vec![item("Inner")],
        },
    ];
    let mut rig = rig(cx, nodes);
    rig.type_text("op");
    assert!(rig.drawn("Filter: op"));
    rig.press("backspace");
    assert!(rig.drawn("Filter: o"));
    rig.press("backspace");
    assert!(!rig.drawn("Filter: o"));
    assert!(rig.drawn("Open"), "the rows are back");
    assert!(rig.drawn("More"));
    // With nothing typed, Backspace still goes back out of a submenu.
    rig.press("down");
    rig.press("right");
    assert_eq!(rig.depth(), 2);
    rig.press("backspace");
    assert_eq!(rig.depth(), 1);
}

#[gpui_kit::test]
fn space_chooses_when_nothing_is_typed_and_is_part_of_the_query_when_something_is(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, vec![item("Open"), item("Rename")]);
    rig.press("space");
    assert_eq!(rig.events(), vec![chose("Open")]);
}

#[gpui_kit::test]
fn space_while_typing_is_part_of_the_query(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, vec![item("Open"), item("Rename")]);
    rig.type_text("r");
    rig.press("space");
    assert!(rig.drawn("Filter: r "));
    assert_eq!(rig.events(), vec![], "nothing was chosen");
}

#[gpui_kit::test]
fn something_nothing_matches_says_so_and_enter_does_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, vec![item("Open"), item("Rename")]);
    rig.type_text("zzqx");
    assert!(rig.drawn("Nothing in this menu matches"));
    assert!(!rig.drawn("Open"));
    rig.press("enter");
    assert_eq!(rig.events(), vec![]);
    // Keys that move through nothing do nothing either.
    rig.press("down");
    rig.press("up");
    assert_eq!(rig.events(), vec![]);
}

#[gpui_kit::test]
fn typing_in_a_submenu_looks_through_the_whole_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let nodes = vec![
        item("Rename"),
        Node::Menu {
            label: "More".into(),
            icon: None,
            disabled: false,
            items: vec![item("Inner")],
        },
    ];
    let mut rig = rig(cx, nodes);
    rig.press("down");
    rig.press("right");
    assert_eq!(rig.depth(), 2);
    rig.type_text("ren");
    assert_eq!(rig.found().first().map(String::as_str), Some("Rename"));
    rig.press("enter");
    assert_eq!(rig.events(), vec![chose("Rename")]);
}

#[gpui_kit::test]
fn taking_the_typing_back_puts_the_submenu_back_as_it_was(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let nodes = vec![
        item("Rename"),
        Node::Menu {
            label: "More".into(),
            icon: None,
            disabled: false,
            items: vec![item("Inner")],
        },
    ];
    let mut rig = rig(cx, nodes);
    rig.press("down");
    rig.press("right");
    rig.type_text("ren");
    rig.press("escape");
    assert_eq!(rig.depth(), 2);
    assert!(rig.drawn("Inner"));
    assert_eq!(rig.events(), vec![]);
}

#[gpui_kit::test]
fn mirrored_the_head_and_a_filtered_menu_still_draw_and_left_enters_a_submenu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut nodes = menu_with(head(vec![badge("In use by Word", Tone::Warn, Run::Nothing)]));
    nodes.push(Node::Menu {
        label: "More".into(),
        icon: None,
        disabled: false,
        items: vec![item("Inner")],
    });
    let mut rig = rig(cx, nodes);
    // The window's start-up sets English, so the direction is turned after it.
    testing::set_rtl(true);
    rig.frame();
    assert!(rig.drawn("setup.exe"));
    assert!(rig.drawn("In use by Word"));
    // The head's picture is at the start edge, which is the right in Arabic.
    rig.type_text("ren");
    assert!(rig.drawn(&t!("Filter: {query}", query = "ren")));
    assert!(!rig.drawn("setup.exe"));
    rig.press("escape");
    assert!(rig.drawn("setup.exe"));
    // Toward the end of the line is Left: it enters a submenu, and Right goes back.
    rig.press("end");
    assert_eq!(rig.selected().as_deref(), Some("More"));
    rig.press("left");
    assert_eq!(rig.depth(), 2);
    rig.press("right");
    assert_eq!(rig.depth(), 1);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn the_head_keeps_its_height_when_what_it_says_changes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut empty = head(vec![]);
    empty.changed = None;
    let mut rig = rig(cx, menu_with(empty));
    let height = |rig: &mut Rig| rig.visual.update(|window, _| window.find_by_label("setup.exe").bounds().size.height);
    let before = height(&mut rig);
    let full = head(vec![badge("Hidden", Tone::Plain, Run::Nothing)]);
    rig.view.update(&mut rig.visual, |view, cx| view.set_head(full, cx));
    rig.frame();
    assert_eq!(before, height(&mut rig), "a head never changes height");
}

// ---- The labels' dots ------------------------------------------------------------------------------------

/// A strip, then the labels' dots (the third checked, the last "No label"), then a row.
fn dotted(names: &[&str]) -> Vec<Node> {
    let mut dots: Vec<Entry> = names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            Entry::ask(
                (*name).into(),
                IconName::Circle,
                MenuEvent::SetLabel {
                    paths: vec!["C:\\x\\a.txt".into()],
                    label: Some((*name).into()),
                },
            )
            .toned(0xef4444 + i as u32)
            .checked(i == 2)
        })
        .collect();
    dots.push(Entry::ask(
        "No label".into(),
        IconName::Circle,
        MenuEvent::SetLabel {
            paths: vec!["C:\\x\\a.txt".into()],
            label: None,
        },
    ));
    vec![Node::Strip(vec![entry("Cut")]), Node::Swatches(dots), item("Open")]
}

#[gpui_kit::test]
fn the_dots_take_the_keyboard_like_the_strip_arrows_along_enter_presses_each_has_a_name(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, dotted(&["Red", "Orange", "Yellow"]));
    for name in ["Red", "Orange", "Yellow", "No label"] {
        assert!(rig.drawn(name), "{name} is drawn with its name");
        let role = rig.visual.update(|window, _| window.find_by_label(name).role());
        assert_eq!(role, Some(Role::MenuItem), "{name}");
    }
    // The menu starts on its first row; Up goes round to the last stop, Home to the first: the strip's button.
    rig.press("home");
    assert_eq!(rig.selected().as_deref(), Some("Cut"));
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some("Red"));
    rig.press("right");
    rig.press("right");
    assert_eq!(rig.selected().as_deref(), Some("Yellow"));
    rig.press("right");
    assert_eq!(rig.selected().as_deref(), Some("No label"));
    rig.press("right");
    assert_eq!(rig.selected().as_deref(), Some("No label"), "the row ends there");
    rig.press("left");
    rig.press("left");
    rig.press("left");
    assert_eq!(rig.selected().as_deref(), Some("Red"));
    rig.press("enter");
    assert!(matches!(
        rig.events().as_slice(),
        [MenuViewEvent::Chose(Run::Ask(MenuEvent::SetLabel { label: Some(l), .. }))] if l == "Red"
    ));
    // A click presses a dot as well.
    rig.click(("menu-swatch", 35usize));
    assert!(matches!(
        rig.events().last(),
        Some(MenuViewEvent::Chose(Run::Ask(MenuEvent::SetLabel { label: None, .. })))
    ));
}

#[gpui_kit::test]
fn with_the_interface_mirrored_and_a_label_named_in_arabic_the_dots_run_from_the_start_edge(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, dotted(&["أحمر", "Orange", "Yellow"]));
    testing::set_rtl(true);
    rig.frame();
    assert!(rig.drawn("أحمر"));
    let x = |rig: &mut Rig, name: &str| rig.visual.update(|window, _| f32::from(window.find_by_label(name).bounds().origin.x));
    let (first, last) = (x(&mut rig, "أحمر"), x(&mut rig, "No label"));
    assert!(first > last, "the first dot is at the right, the start edge: {first} {last}");
    // Toward the end of the line is Left.
    rig.press("home");
    rig.press("down");
    assert_eq!(rig.selected().as_deref(), Some("أحمر"));
    rig.press("left");
    assert_eq!(rig.selected().as_deref(), Some("Orange"));
    testing::set_rtl(false);
}
