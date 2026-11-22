//! Headless tests of the peek card: when it opens and closes, what it says from what is known, and that it never
//! takes the keyboard.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{self, NavOptions};
use crate::testing;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};

const ARABIC: &str = "تقارير";

fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\",
        vec![folder("C:\\", "a"), folder("C:\\", "b"), folder("C:\\", ARABIC)],
    );
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "inner"),
            file("C:\\a", "one.txt", 1_000, 1),
            file("C:\\a", "two.jpg", 2_000, 2),
        ],
    );
    disk.folder("C:\\b", vec![file("C:\\b", "x.txt", 1, 1)]);
    disk.folder("C:\\تقارير", vec![file("C:\\تقارير", "ملف.txt", 10, 1)]);
    disk
}

struct Host {
    peek: Entity<PeekHost>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.peek.clone())
    }
}

struct Rig {
    visual: VisualTestContext,
    peek: Entity<PeekHost>,
    explorer: Entity<Explorer>,
}

fn rig(cx: &mut TestAppContext) -> Rig {
    cx.update(testing::init);
    let disk = disk();
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task("C:\\b", NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let (visual, host) = testing::open_window(cx, {
        let explorer = explorer.clone();
        move |_, cx| {
            let peek = cx.new(|cx| PeekHost::new(explorer, cx));
            cx.new(|_| Host { peek })
        }
    });
    let peek = host.read_with(&visual, |host, _| host.peek.clone());
    Rig { visual, peek, explorer }
}

fn target(path: &str, name: &str, element: &str) -> Target {
    Target {
        key: SharedString::from(format!("card-{path}")),
        element: SharedString::from(element.to_string()),
        path: path.to_string(),
        name: SharedString::from(name.to_string()),
        hint: None,
        read_ahead: true,
    }
}

impl Rig {
    fn enter(&mut self, target: Target) {
        let peek = self.peek.clone();
        self.visual
            .update(|window, cx| peek.update(cx, |peek, cx| peek.enter(target, window, cx)));
    }

    fn leave(&mut self, element: &str) {
        let peek = self.peek.clone();
        self.visual
            .update(|_, cx| peek.update(cx, |peek, cx| peek.leave(element, cx)));
    }

    fn pass(&mut self, ms: u64) {
        self.visual.executor().advance_clock(Duration::from_millis(ms));
        self.visual.run_until_parked();
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    fn open(&self) -> bool {
        self.peek.read_with(&self.visual, |peek, _| peek.is_open())
    }

    /// The card's name for a screen reader, when it is on screen.
    fn card(&mut self) -> Option<(Option<Role>, String)> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find("peek-card")
                .map(|card| (card.role(), card.label().unwrap_or_default().to_string()))
        })
    }
}

#[gpui_kit::test]
fn the_card_opens_after_a_rest_says_what_is_known_and_closes_when_the_pointer_leaves(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx);
    // The folder of a tab that shows it (here: the one the explorer is in) is known.
    rig.enter(target("C:\\b", "b", "tab-1"));
    rig.pass(DWELL.as_millis() as u64 - 100);
    assert!(rig.card().is_none(), "not before the pointer has rested");
    rig.pass(150);
    let (role, label) = rig.card().expect("the card is on screen");
    assert_eq!(role, Some(Role::Tooltip));
    assert!(label.starts_with("b, C:\\b, 1 file"), "{label}");
    // The pointer leaves: it goes a moment later.
    rig.leave("tab-1");
    assert!(rig.open(), "not at once");
    rig.pass(200);
    assert!(rig.card().is_none());
}

#[gpui_kit::test]
fn leaving_before_the_rest_is_over_opens_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx);
    rig.enter(target("C:\\b", "b", "tab-1"));
    rig.pass(200);
    rig.leave("tab-1");
    rig.pass(1_000);
    assert!(!rig.open());
}

#[gpui_kit::test]
fn the_pointer_moving_between_elements_of_one_card_keeps_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx);
    let mut crumb = target("C:\\b", "b", "crumb-b");
    let mut arrow = target("C:\\b", "b", "arrow-b");
    arrow.hint = Some("Subfolders of b".into());
    rig.enter(crumb.clone());
    rig.pass(450);
    assert!(rig.open());
    // Entered the arrow before the crumb is left (either order happens), then the crumb is left.
    rig.enter(arrow.clone());
    rig.leave("crumb-b");
    rig.pass(300);
    assert!(rig.open(), "still on the arrow");
    // Leaving the arrow ends it.
    rig.leave("arrow-b");
    rig.pass(300);
    assert!(!rig.open());
    // And the other order: the crumb is left before the arrow is entered, within the grace.
    crumb.element = "crumb-b".into();
    rig.enter(crumb);
    rig.pass(450);
    rig.leave("crumb-b");
    rig.pass(30);
    rig.enter(arrow);
    rig.pass(300);
    assert!(rig.open());
}

#[gpui_kit::test]
fn escape_and_a_press_close_it_and_another_card_replaces_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx);
    rig.enter(target("C:\\b", "b", "tab-1"));
    rig.pass(450);
    assert!(rig.open());
    rig.visual.update(|window, cx| window.press("escape", cx));
    rig.visual.run_until_parked();
    assert!(!rig.open(), "Escape closed it");

    rig.enter(target("C:\\b", "b", "tab-1"));
    rig.pass(450);
    assert!(rig.open());
    // The pointer goes straight to another tab: the old card goes at once, the new one comes after its rest.
    rig.enter(target("C:\\a", "a", "tab-2"));
    assert!(!rig.open());
    rig.pass(450);
    let (_, label) = rig.card().expect("the other card");
    assert!(label.starts_with("a, C:\\a"), "{label}");
    // A press (the tab was clicked) closes it.
    let peek = rig.peek.clone();
    rig.visual.update(|_, cx| peek.update(cx, |peek, cx| peek.hide(cx)));
    assert!(!rig.open());
    rig.pass(1_000);
    assert!(!rig.open(), "and nothing is waiting to open it again");
}

#[gpui_kit::test]
fn a_folder_nobody_shows_is_read_ahead_and_the_card_fills_in_when_it_arrives(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx);
    rig.enter(target("C:\\a", "a", "crumb-a"));
    rig.pass(DWELL.as_millis() as u64 + 10);
    // The card is up at once with the name and path; what it holds comes after, without a wait for it.
    assert!(rig.open());
    for _ in 0..200 {
        rig.visual.run_until_parked();
        let Some((_, label)) = rig.card() else {
            panic!("the card closed");
        };
        if label.contains("1 folder") {
            assert!(label.contains("2 files"), "{label}");
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the folder's contents never arrived: {:?}", rig.card());
}

#[gpui_kit::test]
fn an_arabic_folder_has_a_card_too_and_the_card_never_takes_the_keyboard(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx);
    let focused_before = rig.visual.update(|window, cx| window.focused(cx));
    rig.enter(target(&format!("C:\\{ARABIC}"), ARABIC, "tab-3"));
    rig.pass(450);
    let (_, label) = rig.card().expect("the card");
    assert!(label.starts_with(&format!("{ARABIC}, C:\\{ARABIC}")), "{label}");
    let focused_after = rig.visual.update(|window, cx| window.focused(cx));
    assert!(focused_before == focused_after, "the card took the keyboard");
    // The explorer is where it was: looking at a folder doesn't go there.
    let path = rig.explorer.read_with(&rig.visual, |e, _| e.current_path().to_string());
    assert_eq!(path, "C:\\b");
}

#[test]
fn hidden_items_are_excluded_when_show_hidden_is_false() {
    use glint_model::entries::{attr, with_details, RawEntry};
    let shown = with_details(
        RawEntry {
            name: "visible.txt".into(),
            size: 100,
            ..Default::default()
        },
        "C:\\b\\visible.txt",
    );
    let hidden = with_details(
        RawEntry {
            name: ".secret".into(),
            size: 200,
            attrs: attr::HIDDEN,
            ..Default::default()
        },
        "C:\\b\\.secret",
    );
    let listed = Listed {
        path: "C:\\b".into(),
        entries: vec![shown, hidden].into(),
        archive: None,
        remote: false,
        elapsed: 0.,
        from_index: false,
    };
    let filtered = Facts::of(&listed, false);
    assert!(filtered.words.contains("1 file"), "{}", filtered.words);
    assert!(!filtered.words.contains("2 files"), "{}", filtered.words);
    let all = Facts::of(&listed, true);
    assert!(all.words.contains("2 files"), "{}", all.words);
}
