//! Headless tests of the address bar: a fake disk behind the explorer and behind the bar's own listings, the
//! real window, real clicks and keys.

use super::*;
use crate::explorer::fake::{file, folder, hidden, FakeDisk};
use crate::explorer::{Drive, DriveKind};
use crate::testing;
use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};

/// The window's content: the bar in a box of a chosen width, so a narrow bar can be tried.
struct Host {
    bar: Entity<AddressBar>,
    width: Pixels,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(self.width).h(px(40.)).child(self.bar.clone())
    }
}

struct Rig {
    visual: VisualTestContext,
    bar: Entity<AddressBar>,
    explorer: Entity<Explorer>,
    disk: FakeDisk,
    events: Rc<RefCell<Vec<AddressBarEvent>>>,
}

/// A fake PC: `C:\` with Users (me, other, a hidden folder, a file) and Documents under me.
fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.set_drives(vec![Drive {
        path: "C:\\".into(),
        label: String::new(),
        kind: DriveKind::Fixed,
        fs: "NTFS".into(),
        total: 1000,
        free: 400,
    }]);
    disk.folder("C:\\", vec![folder("C:\\", "Users"), folder("C:\\", "Windows")]);
    disk.folder(
        "C:\\Users",
        vec![
            folder("C:\\Users", "me"),
            folder("C:\\Users", "other"),
            hidden("C:\\Users", ".cache"),
            file("C:\\Users", "readme.txt", 1, 1),
            file("C:\\Users", "backup.zip", 1, 1),
        ],
    );
    disk.folder(
        "C:\\Users\\me",
        vec![
            folder("C:\\Users\\me", "Documents"),
            folder("C:\\Users\\me", "Downloads"),
        ],
    );
    disk.folder(
        "C:\\Users\\me\\Documents",
        vec![file("C:\\Users\\me\\Documents", "a.txt", 1, 1)],
    );
    disk.folder("C:\\Users\\me\\Downloads", vec![]);
    for place in ["shell:RecycleBinFolder", "shell:Recent", "shell:Gallery"] {
        disk.folder(place, vec![]);
    }
    disk.folder("C:\\Users\\other", vec![]);
    disk.folder("C:\\Windows", vec![]);
    disk.folder("D:\\", vec![]);
    disk
}

fn rig(cx: &mut TestAppContext, rtl: bool, width: f32, start: &str) -> Rig {
    cx.update(testing::init);
    // After init, which starts every test in English.
    testing::set_rtl(rtl);
    let disk = disk();
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let source = disk.source();
    let width = px(width);
    let (mut visual, host) = testing::open_window(cx, move |window, cx| {
        let bar = cx.new(|cx| AddressBar::with_source(source, window, cx));
        cx.new(|_| Host { bar, width })
    });
    // Focus events (and so blur, which closes the address box) only reach an active window.
    visual.update(|window, _| window.activate_window());
    let bar = host.read_with(&visual, |host, _| host.bar.clone());
    let explorer = visual.update(|_, cx| explorer::explorer(cx));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&bar, move |_, event: &AddressBarEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    let mut rig = Rig {
        visual,
        bar,
        explorer,
        disk,
        events,
    };
    rig.visual.update(|_, cx| {
        rig.explorer.update(cx, |e, cx| e.refresh_drives(cx).detach());
    });
    rig.go(start);
    rig
}

impl Rig {
    fn go(&mut self, path: &str) {
        let path = path.to_string();
        self.visual.update(|_, cx| {
            self.explorer
                .update(cx, |e, cx| e.navigate(path, NavOptions::default(), cx))
        });
        self.settle();
    }

    /// Lets the fake disk answer, the explorer notify and the bar redraw, twice (a measurement redraws once more).
    fn settle(&mut self) {
        for _ in 0..3 {
            self.visual.run_until_parked();
            self.visual.update(|window, cx| window.render_frame(cx));
        }
        self.visual.run_until_parked();
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    fn path(&mut self) -> String {
        self.explorer
            .read_with(&self.visual, |e, _| e.current_path().to_string())
    }

    fn element(&mut self, label: &str) -> Option<gpui_kit::test::ElementSnapshot> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label)
        })
    }

    fn role(&mut self, label: &str) -> Option<Role> {
        self.element(label).and_then(|e| e.role())
    }

    fn has(&mut self, label: &str) -> bool {
        self.element(label).is_some()
    }

    fn x(&mut self, label: &str) -> Pixels {
        self.element(label)
            .unwrap_or_else(|| panic!("no element labelled {label:?}"))
            .bounds()
            .origin
            .x
    }

    fn click(&mut self, id: impl Into<ElementId>) {
        let id = id.into();
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.settle();
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.settle();
    }

    fn type_text(&mut self, text: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        });
        self.settle();
    }

    fn focus_address(&mut self) {
        self.visual
            .update(|window, cx| self.bar.update(cx, |bar, cx| bar.focus(window, cx)));
        self.settle();
    }

    fn focus_search(&mut self) {
        self.visual
            .update(|window, cx| self.bar.update(cx, |bar, cx| bar.focus_search(window, cx)));
        self.settle();
    }

    fn address_text(&mut self) -> String {
        let edit = self.bar.read_with(&self.visual, |bar, _| bar.edit.clone());
        edit.read_with(&self.visual, |input, _| input.value().to_string())
    }

    fn filter(&mut self) -> String {
        self.explorer.read_with(&self.visual, |e, _| {
            e.active_tab().map(|t| t.filter.clone()).unwrap_or_default()
        })
    }

    fn events(&self) -> Vec<AddressBarEvent> {
        self.events.borrow().clone()
    }

    fn roles(&mut self, role: Role) -> Vec<String> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window
                .find_all_by_role(role)
                .into_iter()
                .filter_map(|e| e.label().map(str::to_string))
                .collect()
        })
    }
}

#[gpui_kit::test]
fn the_path_is_crumbs_each_a_named_button_and_the_buttons_are_named(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me\\Documents");
    assert_eq!(rig.role("Address bar"), Some(Role::Group));
    // A drive is called by its name, the folders by theirs, and every one is a button.
    for crumb in ["This PC", "Local Disk (C:)", "Users", "me", "Documents"] {
        assert_eq!(rig.role(crumb), Some(Role::Button), "{crumb}");
    }
    for chevron in ["Drives and places", "Subfolders of Users", "Subfolders of Documents"] {
        assert_eq!(rig.role(chevron), Some(Role::Button), "{chevron}");
    }
    for control in [
        "Back (Alt+Left)",
        "Forward (Alt+Right)",
        "Up (Alt+Up)",
        "Refresh (F5)",
        "Type a path (Ctrl+L)",
        "Copy folder path",
    ] {
        assert_eq!(rig.role(control), Some(Role::Button), "{control}");
    }
    assert_eq!(rig.role("Folders"), Some(Role::List));
    // The leaf of the path is the last crumb: it comes after the ones above it, in reading order.
    assert!(rig.x("Users") < rig.x("me") && rig.x("me") < rig.x("Documents"));
    assert!(rig.x("This PC") < rig.x("Local Disk (C:)"));
    // The buttons, the field and the search box sit side by side inside the 900 px they were given.
    let (back, field, search) = rig.visual.update(|window, _| {
        (
            window.find("address-back").bounds(),
            window.find("address-field").bounds(),
            window.find("address-search").bounds(),
        )
    });
    assert!(back.right() <= field.left() && field.right() <= search.left());
    assert!(search.right() <= px(900.) + px(0.5));
    assert!(field.size.width > px(200.), "the field takes the room that is left");
}

#[gpui_kit::test]
fn a_click_on_a_crumb_goes_there_and_the_places_have_no_chevron(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me\\Documents");
    rig.click(("address-crumb", 1_usize));
    assert_eq!(rig.path(), "C:\\Users");
    rig.click("address-this-pc");
    assert_eq!(rig.path(), "");
    // On This PC the button says its name, and there is nothing to go up to.
    assert!(rig.has("This PC"));
    assert!(rig.visual.update(|window, _| window.find("address-up").disabled()) == Some(true));
    // The Recycle Bin is a place: one crumb, no chevron after it.
    rig.go("shell:RecycleBinFolder");
    assert_eq!(rig.role("Recycle Bin"), Some(Role::Button));
    assert!(!rig.has("Subfolders of Recycle Bin"));
}

#[gpui_kit::test]
fn back_forward_and_up_walk_the_tabs_history(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    rig.go("C:\\Users\\me");
    // Nothing forward yet, so Forward is dimmed; Back is not.
    assert_eq!(
        rig.visual.update(|w, _| w.find("address-forward").disabled()),
        Some(true)
    );
    assert_ne!(rig.visual.update(|w, _| w.find("address-back").disabled()), Some(true));
    rig.click("address-back");
    assert_eq!(rig.path(), "C:\\Users");
    assert_ne!(
        rig.visual.update(|w, _| w.find("address-forward").disabled()),
        Some(true)
    );
    rig.click("address-forward");
    assert_eq!(rig.path(), "C:\\Users\\me");
    rig.click("address-up");
    assert_eq!(rig.path(), "C:\\Users");
    // Refresh reads the folder again.
    let before = rig.disk.listed().len();
    rig.click("address-refresh");
    assert!(rig.disk.listed().len() > before);
}

#[gpui_kit::test]
fn a_chevron_lists_the_subfolders_in_the_background_and_a_click_opens_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    // The menu after "Users" lists C:\Users: folders (and archives), not files or hidden items.
    rig.click(("address-chevron", 1_usize));
    assert_eq!(rig.roles(Role::Menu), ["Subfolders of Users"]);
    assert_eq!(rig.roles(Role::MenuItem), ["me", "other", "backup.zip"]);
    rig.click(("address-flyout-item", 1_usize));
    assert_eq!(rig.path(), "C:\\Users\\other");
    // The menu closed with the choice.
    assert!(rig.roles(Role::MenuItem).is_empty());
}

#[gpui_kit::test]
fn the_menu_after_this_pc_lists_drives_and_places_and_works_from_the_keyboard(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    rig.click("address-roots");
    let items = rig.roles(Role::MenuItem);
    assert_eq!(&items[0], "Local Disk (C:)");
    for place in ["Recent", "Gallery", "Recycle Bin"] {
        assert!(items.iter().any(|i| i == place), "{place}");
    }
    // Down, Down, Enter: the second item. Home, the first.
    rig.press("down");
    rig.press("down");
    rig.press("enter");
    // The second item is the first place after the one drive (this PC has no known folders in the fake).
    assert_eq!(rig.path(), glint_model::paths::RECENT);
    // Escape closes a menu and gives the keyboard back.
    rig.click("address-roots");
    assert!(!rig.roles(Role::MenuItem).is_empty());
    rig.press("escape");
    assert!(rig.roles(Role::MenuItem).is_empty());
}

#[gpui_kit::test]
fn a_long_menu_has_a_filter_box_that_narrows_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    let many: Vec<_> = (0..20).map(|i| folder("C:\\Big", &format!("project {i:02}"))).collect();
    rig.disk.folder("C:\\Big", many);
    rig.disk
        .folder("C:\\", vec![folder("C:\\", "Big"), folder("C:\\", "Users")]);
    rig.go("C:\\Big");
    rig.click(("address-chevron", 1_usize));
    assert_eq!(rig.role("Filter folders…"), Some(Role::TextInput));
    // The keyboard is in the filter box: typing narrows the list.
    rig.type_text("project 1");
    let items = rig.roles(Role::MenuItem);
    assert_eq!(items.len(), 10, "{items:?}");
    assert!(items.iter().all(|i| i.starts_with("project 1")));
    rig.type_text("7");
    assert_eq!(rig.roles(Role::MenuItem), ["project 17"]);
    rig.type_text("zzz");
    assert!(rig.has("No matching folders"));
}

#[gpui_kit::test]
fn typing_a_path_offers_folders_and_tab_completes_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    rig.disk.on_resolve("C:\\Users\\other\\", "C:\\Users\\other", true);
    rig.disk.on_resolve("C:\\Users\\other", "C:\\Users\\other", true);
    // Ctrl+L: the whole path in the box, selected, the keyboard in it.
    rig.focus_address();
    assert!(rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
    assert_eq!(rig.address_text(), "C:\\Users\\me");
    assert_eq!(rig.role("Address"), Some(Role::TextInput));
    // Typing replaces the selection.
    rig.type_text("C:\\Users\\o");
    assert_eq!(rig.address_text(), "C:\\Users\\o");
    assert_eq!(rig.role("Path suggestions"), Some(Role::ListBox));
    assert_eq!(rig.roles(Role::ListBoxOption), ["other"]);
    // Tab takes it, a folder with a separator after it.
    rig.press("tab");
    assert_eq!(rig.address_text(), "C:\\Users\\other\\");
    // Enter opens what's typed, and the box closes.
    rig.press("enter");
    assert_eq!(rig.path(), "C:\\Users\\other");
    assert!(!rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
    assert!(rig.events().contains(&AddressBarEvent::FocusFiles { first: false }));
}

#[gpui_kit::test]
fn the_arrows_pick_a_suggestion_and_enter_opens_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    rig.disk
        .on_resolve("C:\\Users\\me\\Downloads", "C:\\Users\\me\\Downloads", true);
    rig.focus_address();
    rig.type_text("C:\\Users\\me\\do");
    // Folders sorted as they came: Documents, Downloads.
    assert_eq!(rig.roles(Role::ListBoxOption), ["Documents", "Downloads"]);
    // Down goes to the first, Down again to the second, Up back; Down from the last wraps round.
    rig.press("down");
    rig.press("down");
    let selected = rig
        .visual
        .update(|window, _| window.find(("address-suggestion", 1_usize)).selected());
    assert_eq!(selected, Some(true));
    rig.press("enter");
    assert_eq!(rig.path(), "C:\\Users\\me\\Downloads");
}

#[gpui_kit::test]
fn a_click_on_a_suggestion_opens_it_without_the_box_giving_up_first(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    rig.disk.on_resolve("C:\\Users\\other", "C:\\Users\\other", true);
    rig.focus_address();
    rig.type_text("C:\\Users\\ot");
    rig.click(("address-suggestion", 0_usize));
    assert_eq!(rig.path(), "C:\\Users\\other");
    assert!(!rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
}

#[gpui_kit::test]
fn up_from_nothing_goes_to_the_last_suggestion_and_down_from_the_last_wraps(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    rig.focus_address();
    rig.type_text("C:\\Users\\me\\do");
    assert_eq!(rig.roles(Role::ListBoxOption), ["Documents", "Downloads"]);
    let selected = |rig: &mut Rig, ix: usize| {
        rig.visual
            .update(|window, _| window.find(("address-suggestion", ix)).selected())
    };
    rig.press("up");
    assert_eq!(selected(&mut rig, 1), Some(true));
    rig.press("down");
    assert_eq!(selected(&mut rig, 0), Some(true));
    // Typing again forgets the highlight.
    rig.type_text("w");
    assert_ne!(selected(&mut rig, 0), Some(true));
}

#[gpui_kit::test]
fn a_name_typed_with_a_typo_is_still_offered(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    rig.focus_address();
    rig.type_text("C:\\Users\\me\\Dowloads");
    assert_eq!(rig.roles(Role::ListBoxOption), ["Downloads"]);
}

#[gpui_kit::test]
fn escape_gives_up_and_the_crumbs_come_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    // A click on the empty part of the bar types a path.
    rig.click("address-type");
    assert!(rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
    assert!(rig.explorer.read_with(&rig.visual, |e, _| e.editing_address));
    rig.press("escape");
    assert!(!rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
    assert!(!rig.explorer.read_with(&rig.visual, |e, _| e.editing_address));
    assert_eq!(rig.path(), "C:\\Users\\me", "nothing was opened");
    assert!(rig.has("Documents") || rig.has("me"), "the crumbs are back");
    assert!(rig.events().contains(&AddressBarEvent::FocusFiles { first: false }));
}

#[gpui_kit::test]
fn the_explorer_asking_for_a_path_opens_the_box_and_a_navigation_closes_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    // Ctrl+L, F4 and Ctrl+D just set the explorer's flag: the bar follows.
    rig.visual.update(|_, cx| {
        rig.explorer.update(cx, |e, cx| {
            e.editing_address = true;
            cx.notify();
        })
    });
    rig.settle();
    assert!(rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
    // Anything that changes the folder ends it.
    rig.go("C:\\Users");
    assert!(!rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
}

#[gpui_kit::test]
fn the_search_box_filters_as_you_type_and_tells_the_find_part_the_rest(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    assert_eq!(rig.role("Search Users and its subfolders."), Some(Role::TextInput));
    rig.focus_search();
    rig.type_text("read");
    // The folder is filtered straight away, by the explorer.
    assert_eq!(rig.filter(), "read");
    let rows = rig.explorer.read_with(&rig.visual, |e, _| {
        e.active_tab()
            .map(|t| t.rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    assert_eq!(rows, ["readme.txt"]);
    // After a pause the find part is offered the query, to run if the index answers at once.
    rig.visual.executor().advance_clock(Duration::from_millis(150));
    rig.settle();
    assert!(rig
        .events()
        .contains(&AddressBarEvent::FindAsTyped { query: "read".into() }));
    // Enter searches the subfolders too.
    rig.press("enter");
    assert!(rig
        .events()
        .contains(&AddressBarEvent::RunFind { query: "read".into() }));
    // The clear button empties the filter and says so.
    assert_eq!(rig.role("Clear search"), Some(Role::Button));
    rig.click("address-search-clear");
    assert_eq!(rig.filter(), "");
    assert!(rig.events().contains(&AddressBarEvent::ClearFind));
}

#[gpui_kit::test]
fn escape_clears_the_search_and_down_hands_the_keyboard_to_the_list(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    rig.focus_search();
    rig.type_text("zip");
    rig.press("down");
    assert!(rig.events().contains(&AddressBarEvent::FocusFiles { first: true }));
    rig.focus_search();
    rig.press("escape");
    assert_eq!(rig.filter(), "");
    assert!(rig.events().contains(&AddressBarEvent::ClearFind));
}

#[gpui_kit::test]
fn an_empty_focused_search_box_offers_its_filters_and_a_click_types_one(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    rig.focus_search();
    assert_eq!(rig.role("Search filters"), Some(Role::Group));
    assert_eq!(rig.role("ext:pdf"), Some(Role::Button));
    // The Git filter is only offered in a repository.
    assert!(!rig.has("git:changed"));
    rig.bar
        .update(&mut rig.visual, |bar, cx| bar.set_in_repository(true, cx));
    rig.settle();
    assert!(rig.has("git:changed"));
    rig.click(("address-offer", 0_usize));
    assert_eq!(rig.filter(), "ext:");
}

#[gpui_kit::test]
fn plain_words_read_as_filters_are_offered_under_the_box(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    rig.focus_search();
    rig.type_text("pdfs from last week");
    let plain = rig.bar.read_with(&rig.visual, |bar, _| bar.plain.clone());
    let plain = plain.expect("the words read as filters");
    assert!(plain.query.contains("ext:pdf"));
    // Tab takes them into the box.
    rig.press("tab");
    assert_eq!(rig.filter(), plain.query);
}

#[gpui_kit::test]
fn a_narrow_bar_folds_the_first_folders_into_a_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let deep =
        "C:\\Users\\me\\Documents\\an extremely long folder name to fill the bar\\and another very long name here";
    let mut rig = rig(cx, false, 700., "C:\\Users");
    let mut path = String::from("C:\\Users");
    for part in deep.split('\\').skip(2) {
        let parent = path.clone();
        rig.disk.folder(&parent, vec![folder(&parent, part)]);
        path = format!("{parent}\\{part}");
        rig.disk.folder(&path, vec![]);
    }
    rig.go(&path);
    // Everything that doesn't fit has gone into the "…" menu: the last folder is still there.
    assert!(rig.has("Folders above"), "something was folded");
    assert!(rig.has("and another very long name here"));
    rig.click("address-folded");
    let folded = rig.roles(Role::MenuItem);
    assert!(folded.iter().any(|name| name == "Local Disk (C:)"), "{folded:?}");
    assert!(!folded.iter().any(|name| name == "and another very long name here"));
    // Choosing one goes there.
    rig.click(("address-flyout-item", 0_usize));
    assert_eq!(rig.path(), "C:\\");
}

#[gpui_kit::test]
fn dual_pane_view_shows_which_pane_the_bar_follows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    assert!(!rig.has("The left pane: F6 for the right one"));
    rig.visual
        .update(|_, cx| rig.explorer.update(cx, |e, cx| e.toggle_dual(cx)));
    rig.settle();
    // The new pane has the keyboard.
    assert!(rig.has("The right pane: F6 for the left one"));
    rig.click("address-pane");
    assert!(rig.has("The left pane: F6 for the right one"));
}

#[gpui_kit::test]
fn the_icons_the_bar_draws_are_embedded(_cx: &mut TestAppContext) {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    // A missing icon draws nothing and says nothing, so each is checked here (kit.rs's list is the
    // coordinator's: it adds these when it integrates the bar).
    for name in [
        IconName::ArrowLeft,
        IconName::ArrowRight,
        IconName::ArrowUp,
        IconName::Check,
        IconName::ChevronLeft,
        IconName::ChevronRight,
        IconName::Clock,
        IconName::Cloud,
        IconName::Copy,
        IconName::Ellipsis,
        IconName::File,
        IconName::FileArchive,
        IconName::Folder,
        IconName::Images,
        IconName::LoaderCircle,
        IconName::Monitor,
        IconName::PanelLeft,
        IconName::PanelRight,
        IconName::RotateCw,
        IconName::Search,
        IconName::Trash,
        IconName::X,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

#[gpui_kit::test]
fn an_arabic_name_is_a_crumb_a_suggestion_and_a_menu_item(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk_path = "C:\\ملفات";
    let mut rig = rig(cx, false, 900., "C:\\");
    rig.disk.folder(
        "C:\\",
        vec![
            folder("C:\\", "ملفات"),
            folder("C:\\", "Users"),
            folder("C:\\", "Windows"),
        ],
    );
    rig.disk.folder(disk_path, vec![folder(disk_path, "التقارير")]);
    rig.disk.folder("C:\\ملفات\\التقارير", vec![]);
    rig.go("C:\\ملفات\\التقارير");
    // The crumb's name is the file's, without the layout-only isolates.
    assert_eq!(rig.role("ملفات"), Some(Role::Button));
    assert_eq!(rig.role("التقارير"), Some(Role::Button));
    rig.click(("address-chevron", 1_usize));
    assert_eq!(rig.roles(Role::MenuItem), ["التقارير"]);
    rig.press("escape");
    rig.focus_address();
    rig.type_text("C:\\ملفات\\الت");
    assert_eq!(rig.roles(Role::ListBoxOption), ["التقارير"]);
}

#[gpui_kit::test]
fn the_arabic_interface_mirrors_the_bar(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, true, 900., "C:\\Users\\me");
    // Back is to the right of Forward, the path reads from the right, and the search box is at the far left.
    assert!(rig.x("Back (Alt+Left)") > rig.x("Forward (Alt+Right)"));
    // "This PC" is called by its Arabic name.
    let this_pc = t!("This PC").to_string();
    assert_ne!(this_pc, "This PC");
    assert!(rig.x(&this_pc) > rig.x("Users"));
    assert!(rig.x("Users") > rig.x("me"));
    let search = rig.x("Search me and its subfolders.");
    assert!(
        search < rig.x("Users"),
        "the search box is at the end, which is the left"
    );
    // The menu opens, and still names everything.
    rig.click(("address-chevron", 1_usize));
    assert_eq!(rig.roles(Role::MenuItem), ["me", "other", "backup.zip"]);
    rig.press("escape");
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_narrow_arabic_bar_folds_the_first_folders_at_the_right(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, true, 700., "C:\\Users");
    let mut path = String::from("C:\\Users");
    for part in ["اسم مجلد طويل جدا ليملأ الشريط", "واسم آخر طويل جدا أيضا هنا"]
    {
        let parent = path.clone();
        rig.disk.folder(&parent, vec![folder(&parent, part)]);
        path = format!("{parent}\\{part}");
        rig.disk.folder(&path, vec![]);
    }
    rig.go(&path);
    assert!(rig.has("Folders above"), "the first folders were folded away");
    assert!(rig.has("واسم آخر طويل جدا أيضا هنا"));
    rig.click("address-folded");
    assert_eq!(
        rig.roles(Role::MenuItem).first().map(String::as_str),
        Some("Local Disk (C:)")
    );
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_drive_or_known_folder_is_offered_while_no_folder_is_typed_yet(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    rig.focus_address();
    rig.type_text("loc");
    assert_eq!(rig.roles(Role::ListBoxOption), ["Local Disk (C:)"]);
    // The drive's file system sits at the end of the row; Tab completes to the drive.
    rig.press("tab");
    assert_eq!(rig.address_text(), "C:\\");
}

#[gpui_kit::test]
fn the_box_starts_with_the_places_by_name(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "");
    rig.focus_address();
    assert_eq!(rig.address_text(), "This PC");
    // Nothing offered for "This PC"; Enter goes there (it is where we are).
    assert!(rig.roles(Role::ListBoxOption).is_empty());
    rig.press("escape");
    rig.go("shell:Recent");
    rig.focus_address();
    assert_eq!(rig.address_text(), "Recent");
}

#[gpui_kit::test]
fn tab_with_nothing_to_complete_does_not_type_a_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    rig.focus_address();
    rig.type_text("C:\\Nowhere\\x");
    assert!(rig.roles(Role::ListBoxOption).is_empty());
    rig.press("tab");
    assert_eq!(rig.address_text(), "C:\\Nowhere\\x");
}

#[gpui_kit::test]
fn the_box_closes_when_the_keyboard_goes_elsewhere(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users");
    rig.focus_address();
    assert!(rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
    // Ctrl+F: the keyboard goes to the search box and the path box gives up, as a blur did in the browser.
    rig.focus_search();
    assert!(!rig.bar.read_with(&rig.visual, |bar, _| bar.is_editing()));
    assert!(!rig.explorer.read_with(&rig.visual, |e, _| e.editing_address));
    // The window's keys still work: the keyboard stayed inside the bar's tree after the box went.
    rig.focus_address();
    rig.press("escape");
    let focused = rig.visual.update(|window, cx| window.focused(cx).is_some());
    assert!(focused, "something has the keyboard");
}

#[gpui_kit::test]
fn escape_in_a_menu_gives_the_keyboard_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    rig.focus_search();
    rig.click(("address-chevron", 1_usize));
    assert!(!rig.roles(Role::MenuItem).is_empty());
    rig.press("escape");
    assert!(rig.roles(Role::MenuItem).is_empty());
    // The keyboard went back to the chevron that opened the menu (the button has it once pressed), inside the bar.
    let in_bar = rig
        .visual
        .update(|window, cx| rig.bar.read(cx).focus.contains_focused(window, cx));
    assert!(in_bar);
}

#[gpui_kit::test]
fn a_middle_click_opens_a_crumb_in_a_new_tab_and_stays(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    let tabs = |rig: &mut Rig| rig.explorer.read_with(&rig.visual, |e, _| e.panes()[0].tabs.len());
    assert_eq!(tabs(&mut rig), 1);
    let at = rig.element("Users").expect("the crumb").bounds().center();
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.pointer_down(at, MouseButton::Middle, cx);
        window.pointer_up(at, MouseButton::Middle, cx);
    });
    rig.settle();
    assert_eq!(tabs(&mut rig), 2);
    assert_eq!(rig.path(), "C:\\Users\\me", "you stay where you are");
}

#[gpui_kit::test]
fn the_copy_button_puts_the_folders_path_on_the_clipboard_and_shows_a_check(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "C:\\Users\\me");
    // The button shows when the pointer is over the bar.
    rig.visual.update(|window, cx| {
        window.render_frame(cx);
        window.hover("address-crumb-bar", cx);
    });
    rig.settle();
    rig.click("address-copy");
    let copied = rig
        .visual
        .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("C:\\Users\\me"));
    assert!(rig.bar.read_with(&rig.visual, |bar, _| bar.copied));
    // It goes back after a moment.
    rig.visual.executor().advance_clock(Duration::from_millis(1500));
    rig.settle();
    assert!(!rig.bar.read_with(&rig.visual, |bar, _| bar.copied));
}

#[gpui_kit::test]
fn this_pc_has_no_copy_button(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut rig = rig(cx, false, 900., "");
    assert!(!rig.has("Copy folder path"));
    assert_eq!(rig.role("This PC"), Some(Role::Button));
    assert!(rig.has("Drives and places"));
}
