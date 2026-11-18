//! Tests of the sections that are settings and little else: switches that depend on each other, the shortcut
//! field, the pattern lists, text boxes and pickers, Reset, History, the Arabic interface.

use super::tests::*;
use super::*;
use crate::testing;
use glint_model::store::Theme;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{TestAppContext, VisualTestContext};
use std::sync::Arc;

#[gpui_kit::test]
fn a_dependent_switch_is_disabled_until_what_it_needs_is_on(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "files");
    // The tree follows the folder shown only while the sidebar shows folders.
    reveal(&mut visual, &view, "settings-switch-tree-follows");
    click(&mut visual, "settings-switch-folder-tree");
    assert!(!settings(&mut visual).folder_tree);
    click(&mut visual, "settings-switch-tree-follows");
    assert!(!settings(&mut visual).tree_follows, "a disabled switch changes nothing");
    click(&mut visual, "settings-label-tree-follows");
    assert!(!settings(&mut visual).tree_follows, "nor does its label");
    click(&mut visual, "settings-switch-folder-tree");
    click(&mut visual, "settings-switch-tree-follows");
    assert!(settings(&mut visual).tree_follows, "it works once the tree is back");
    // Start with Windows needs the tray, and shows off without it.
    go_to(&mut visual, "quick-search");
    click(&mut visual, "settings-switch-start-with-windows");
    assert!(!settings(&mut visual).start_with_windows);
    click(&mut visual, "settings-switch-keep-in-tray");
    assert!(settings(&mut visual).keep_in_tray);
    click(&mut visual, "settings-switch-start-with-windows");
    assert!(settings(&mut visual).start_with_windows);
    // Turning the tray off turns it off with Windows, as far as anyone can see.
    visual.update(|_, cx| state::update(cx, |s| s.start_with_windows = true));
}

#[gpui_kit::test]
fn the_quick_search_shortcut_is_checked_with_the_trays_own_parser(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "quick-search");
    assert_eq!(settings(&mut visual).quick_shortcut.as_deref(), Some("Ctrl+Alt+F"));
    // The presets are choices: Off, then the three.
    click(&mut visual, "settings-choice-quick-shortcut-2");
    assert_eq!(settings(&mut visual).quick_shortcut.as_deref(), Some("Ctrl+Alt+Space"));
    click(&mut visual, "settings-choice-quick-shortcut-0");
    assert_eq!(settings(&mut visual).quick_shortcut, None);

    let shortcut_box = visual.update(|_, cx| view.read(cx).shortcut_box.clone());
    // Something the tray couldn't register is refused, in the tray's words, and the setting stays.
    type_into(&mut visual, &shortcut_box, "banana");
    click(&mut visual, "settings-button-quick-shortcut-set");
    assert_eq!(
        role_of(&mut visual, "“banana” isn't a shortcut Glint knows"),
        Some(Role::Alert)
    );
    assert_eq!(settings(&mut visual).quick_shortcut, None);
    // A key alone would be swallowed everywhere: refused too.
    type_into(&mut visual, &shortcut_box, "F");
    click(&mut visual, "settings-button-quick-shortcut-set");
    assert!(has(&mut visual, "“F” isn't a shortcut Glint knows"));
    // Typing again clears the complaint.
    type_into(&mut visual, &shortcut_box, "ctrl + shift + k");
    assert!(!has(&mut visual, "“F” isn't a shortcut Glint knows"));
    // A good one is written the usual way, becomes the chosen option, and the box empties.
    click(&mut visual, "settings-button-quick-shortcut-set");
    assert_eq!(settings(&mut visual).quick_shortcut.as_deref(), Some("Ctrl+Shift+K"));
    let chosen = visual.update(|window, cx| {
        window.render_frame(cx);
        // Off, the three presets, then the shortcut set otherwise.
        window.find("settings-choice-quick-shortcut-4").checked()
    });
    assert_eq!(chosen, Some(true));
    assert_eq!(visual.update(|_, cx| shortcut_box.read(cx).value().to_string()), "");
    // Another app holding the shortcut is said under the label, only while it's the one chosen.
    visual.update(|_, cx| view.update(cx, |view, cx| view.set_quick_taken(Some("Ctrl+Shift+K".into()), cx)));
    visual.run_until_parked();
    assert_eq!(
        role_of(
            &mut visual,
            "Another app already uses Ctrl+Shift+K, so it doesn't open quick search: pick another"
        ),
        None,
        "the hint is a description of the choice, not an element of its own"
    );
    let description = visual.update(|window, cx| {
        window.render_frame(cx);
        window
            .find("settings-choice-quick-shortcut")
            .label()
            .map(str::to_string)
    });
    assert_eq!(description.as_deref(), Some("Shortcut"));
}

#[gpui_kit::test]
fn patterns_are_added_trimmed_once_and_taken_out_with_their_cross(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "find-files-by-name");
    let editor = visual.update(|_, cx| view.read(cx).find_exclusions.clone());
    assert_eq!(settings(&mut visual).find_exclusions, ["node_modules", ".git"]);
    let input = visual.update(|_, cx| editor.read(cx).input().clone());
    type_into(&mut visual, &input, "  *.tmp ");
    click(&mut visual, "settings-button-find-exclusions-add");
    assert_eq!(settings(&mut visual).find_exclusions, ["node_modules", ".git", "*.tmp"]);
    // The box empties, and the same pattern isn't added twice.
    assert_eq!(visual.update(|_, cx| editor.read(cx).typed(cx)), "");
    let input = visual.update(|_, cx| editor.read(cx).input().clone());
    type_into(&mut visual, &input, "*.tmp");
    click(&mut visual, "settings-button-find-exclusions-add");
    assert_eq!(settings(&mut visual).find_exclusions.len(), 3);
    // Each chip has a named cross.
    let cross = visual.update(|window, cx| {
        window.render_frame(cx);
        window
            .find("settings-chip-find-exclusions-0")
            .label()
            .map(str::to_string)
    });
    assert_eq!(cross.as_deref(), Some("Stop excluding node_modules"));
    click(&mut visual, "settings-chip-find-exclusions-0");
    assert_eq!(settings(&mut visual).find_exclusions, [".git", "*.tmp"]);
    // A name in Arabic goes in and comes out the same.
    let input = visual.update(|_, cx| editor.read(cx).input().clone());
    type_into(&mut visual, &input, "مجلد_قديم");
    click(&mut visual, "settings-button-find-exclusions-add");
    assert!(settings(&mut visual).find_exclusions.contains(&"مجلد_قديم".to_string()));
    assert!(has(&mut visual, "Stop excluding مجلد_قديم"));
}

#[gpui_kit::test]
fn the_text_search_presets_toggle_their_patterns_and_remove_all_empties_the_list(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    go_to(&mut visual, "text-search-skips");
    // The defaults have some archives (5 of 8), so pressing Archives adds the rest...
    click(&mut visual, "settings-preset-archives");
    let skipped = settings(&mut visual).exclusions;
    for item in ["*.zip", "*.rar", "*.7z", "*.tar", "*.gz", "*.bz2", "*.xz", "*.iso"] {
        assert!(skipped.contains(&item.to_string()), "{item}");
    }
    // ...and pressing it again takes all eight out, and only them.
    click(&mut visual, "settings-preset-archives");
    let skipped = settings(&mut visual).exclusions;
    assert!(!skipped.contains(&"*.zip".to_string()) && skipped.contains(&"*.png".to_string()));
    click(&mut visual, "settings-button-exclusions-remove-all");
    assert!(settings(&mut visual).exclusions.is_empty());
    assert!(has(&mut visual, "Nothing is excluded."));
}

#[gpui_kit::test]
fn text_boxes_write_the_settings_as_they_are_typed_and_follow_a_change_from_elsewhere(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "search-text-ripgrep");
    let editor = visual.update(|_, cx| view.read(cx).editor_path.clone());
    type_into(&mut visual, &editor, "code");
    assert_eq!(settings(&mut visual).editor_path, "code");
    // An editor found on this PC fills in its path (the terminal found with it is not offered).
    go_to(&mut visual, "search-text-ripgrep");
    assert!(has(&mut visual, "An editor found on this PC"));
    assert!(!has(&mut visual, "Windows Terminal"));
    click(&mut visual, "settings-choice-editor-picker-0");
    assert_eq!(settings(&mut visual).editor_path, "C:\\Code.exe");
    assert_eq!(
        visual.update(|_, cx| editor.read(cx).value().to_string()),
        "C:\\Code.exe"
    );
    // Changing a setting from elsewhere (Reset, an import) puts its text back in the box.
    visual.update(|_, cx| state::update(cx, |s| s.editor_path = "E:\\Else.exe".into()));
    visual.run_until_parked();
    assert_eq!(visual.update(|_, cx| editor.read(cx).value().to_string()), "E:\\Else.exe");
}

#[gpui_kit::test]
fn browse_buttons_pick_a_folder_into_the_setting_and_cancel_changes_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    // The gallery's folders: Cancel adds nothing; one is added once, each taken out with its cross.
    go_to(&mut visual, "files");
    reveal(&mut visual, &view, "settings-button-gallery-add");
    lock(&fake.picks).push_back(None);
    click(&mut visual, "settings-button-gallery-add");
    settle(&mut visual, |_| fake.calls().iter().any(|c| c.starts_with("pick_folder")));
    assert!(settings(&mut visual).gallery_folders.is_empty());
    lock(&fake.picks).push_back(Some("D:\\Photos".into()));
    click(&mut visual, "settings-button-gallery-add");
    settle(&mut visual, |cx| settings(cx).gallery_folders == ["D:\\Photos"]);
    lock(&fake.picks).push_back(Some("d:\\photos\\".into()));
    click(&mut visual, "settings-button-gallery-add");
    settle(&mut visual, |_| lock(&fake.picks).is_empty());
    visual.run_until_parked();
    assert_eq!(
        settings(&mut visual).gallery_folders,
        ["D:\\Photos"],
        "the same folder isn't added twice"
    );
    assert_eq!(role_of(&mut visual, "D:\\Photos"), Some(Role::ListItem));
    assert_eq!(
        role_of(&mut visual, "Take D:\\Photos out of the gallery"),
        Some(Role::Button)
    );
    click(&mut visual, "settings-gallery-remove-0");
    assert!(settings(&mut visual).gallery_folders.is_empty());
}

#[gpui_kit::test]
fn terminal_shells_found_are_the_choices_and_the_first_is_chosen_until_one_is(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    go_to(&mut visual, "terminal");
    let checked = |cx: &mut VisualTestContext, i: usize| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(format!("settings-choice-terminal-shell-{i}")).checked()
        })
    };
    // Nothing chosen yet (""), so the first shell found shows as the one.
    assert_eq!(settings(&mut visual).terminal_shell, "");
    assert_eq!(
        (checked(&mut visual, 0), checked(&mut visual, 1)),
        (Some(true), Some(false))
    );
    click(&mut visual, "settings-choice-terminal-shell-1");
    assert_eq!(settings(&mut visual).terminal_shell, "cmd");
    assert_eq!(
        (checked(&mut visual, 0), checked(&mut visual, 1)),
        (Some(false), Some(true))
    );
    click(&mut visual, "settings-switch-terminal-follow");
    assert!(!settings(&mut visual).terminal_follow);
}

#[gpui_kit::test]
fn reset_asks_first_and_only_a_yes_restores_the_defaults(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    visual.update(|_, cx| {
        state::update(cx, |s| {
            s.theme = Theme::Dark;
            s.show_hidden = true;
        })
    });
    go_to(&mut visual, "about");
    click(&mut visual, "settings-button-about-reset");
    let asked = visual.update(|window, cx| {
        window.render_frame(cx);
        let question = window.find("settings-confirm");
        (
            question.role(),
            question.label().map(str::to_string),
            question.description().map(str::to_string),
        )
    });
    assert_eq!(asked.0, Some(Role::Group));
    assert_eq!(asked.1.as_deref(), Some("Reset settings"));
    assert_eq!(
        asked.2.as_deref(),
        Some("Reset all settings to their defaults? Your search history is kept.")
    );
    // Cancel changes nothing.
    click(&mut visual, "settings-confirm-cancel");
    assert_eq!(settings(&mut visual).theme, Theme::Dark);
    click(&mut visual, "settings-button-about-reset");
    click(&mut visual, "settings-confirm-ok");
    let s = settings(&mut visual);
    assert_eq!((s.theme, s.show_hidden), (Theme::System, false));
    // The dialog under it is still Settings.
    visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
}

#[gpui_kit::test]
fn history_counts_clear_and_come_back_with_undo(cx: &mut TestAppContext) {
    use glint_model::store::HistoryItem;
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    go_to(&mut visual, "history");
    assert!(has(&mut visual, "0 searches in the history"));
    assert_eq!(
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("settings-button-history-clear").disabled()
        }),
        Some(true)
    );
    visual.update(|_, cx| {
        persistence::update_app(cx, |app| {
            app.history.push(HistoryItem {
                query: "invoice".into(),
                path: "C:\\".into(),
                timestamp: 1,
            })
        })
    });
    visual.run_until_parked();
    assert!(has(&mut visual, "1 search in the history"));
    click(&mut visual, "settings-button-history-clear");
    assert!(has(&mut visual, "0 searches in the history"));
    assert_eq!(
        visual.update(|_, cx| persistence::app(cx).map(|a| a.history.len())),
        Some(0)
    );
    // The notice has Undo, which brings it back.
    assert!(has(&mut visual, "Undo"));
    // PORT NOTE: pressing the toast's Undo button never reached its handler under the headless pointer, in this
    // window's dialog layer, so what it does is driven here: it puts back exactly what was cleared.
    visual.update(|_, cx| {
        super::general::restore(
            super::general::Clear::History,
            vec![glint_model::store::HistoryItem {
                query: "invoice".into(),
                path: "C:\\".into(),
                timestamp: 1,
            }],
            Default::default(),
            cx,
        )
    });
    assert_eq!(
        visual.update(|_, cx| persistence::app(cx).map(|a| a.history.len())),
        Some(1)
    );
    // Max items and the zoxide switch, which waits for zoxide on the PATH.
    click(&mut visual, "settings-choice-max-history-items-1");
    assert_eq!(settings(&mut visual).max_history_items, 500);
    click(&mut visual, "settings-switch-zoxide");
    assert!(
        !settings(&mut visual).zoxide,
        "zoxide isn't installed, so the switch waits"
    );
}

#[gpui_kit::test]
fn a_mirrored_layout_and_arabic_names_carry_through(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    testing::set_rtl(true);
    visual.update(|_, cx| cx.refresh_windows());
    visual.run_until_parked();
    // Mirrored, the section list moves to the start edge (the right), and the page to its left.
    let (nav, body) = visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.find("settings-sections").bounds(),
            window.find("settings-body").bounds(),
        )
    });
    assert!(
        nav.origin.x > body.origin.x,
        "the names are on the right when mirrored: {nav:?} vs {body:?}"
    );
    // Arabic text goes through a list, a chip and the name of a button, untouched.
    go_to(&mut visual, "files");
    reveal(&mut visual, &view, "settings-button-gallery-add");
    lock(&fake.picks).push_back(Some("D:\\صور العائلة".into()));
    click(&mut visual, "settings-button-gallery-add");
    settle(&mut visual, |cx| settings(cx).gallery_folders == ["D:\\صور العائلة"]);
    assert_eq!(role_of(&mut visual, "D:\\صور العائلة"), Some(Role::ListItem));
    go_to(&mut visual, "find-files-by-name");
    let editor = visual.update(|_, cx| view.read(cx).find_exclusions.clone());
    let input = visual.update(|_, cx| editor.read(cx).input().clone());
    type_into(&mut visual, &input, "ملفات_مؤقتة");
    click(&mut visual, "settings-button-find-exclusions-add");
    assert!(settings(&mut visual)
        .find_exclusions
        .contains(&"ملفات_مؤقتة".to_string()));
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn opening_at_about_shows_its_page_and_marks_it_on_the_left(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, Some(ABOUT));
    frame(&mut visual);
    visual.run_until_parked();
    frame(&mut visual);
    assert_eq!(visual.update(|_, cx| view.read(cx).current_section_name()), "about");
    assert_eq!(role_of(&mut visual, "About"), Some(Role::Heading), "the page's header names it");
    let marked = visual.update(|window, cx| {
        window.render_frame(cx);
        window.find("settings-nav-about").selected()
    });
    assert_eq!(marked, Some(true));
    // A section that shares a page opens that page.
    visual.update(|_, cx| view.update(cx, |view, cx| view.scroll_to("text-search-skips", cx)));
    frame(&mut visual);
    assert_eq!(visual.update(|_, cx| view.read(cx).page()), Page::Inside);
    // A second request while it's open doesn't stack another dialog on it.
    let again = visual.update(|window, cx| {
        let backend: Arc<dyn Backend> = fake.clone();
        open_with(window, cx, backend, false, Some("settings-appearance"))
    });
    assert_eq!(again.entity_id(), view.entity_id());
}

#[gpui_kit::test]
fn the_overview_says_how_glint_is_set_up_and_suggests_indexing_every_drive(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let said = |cx: &mut VisualTestContext, tile: &str| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window
                .try_find(format!("settings-tile-{tile}"))
                .and_then(|e| e.description().map(str::to_string))
        })
    };
    settle(&mut visual, |cx| {
        said(cx, "instant-search").as_deref() == Some("Off: names are searched drive by drive")
    });
    assert_eq!(view.read_with(&visual, |v, _| v.page()), Page::Overview);
    for tile in ["instant-search", "tools", "clouds", "appearance"] {
        assert!(
            visual.update(|window, cx| {
                window.render_frame(cx);
                window.try_find(format!("settings-tile-{tile}")).is_some()
            }),
            "the {tile} tile"
        );
    }
    // Instant search is off on the fake: the suggestion to index every drive leads the list.
    assert_eq!(
        role_of(&mut visual, "Find any file on every drive, instantly"),
        Some(Role::Group)
    );
    // Its button turns it on (Windows would ask for approval).
    click(&mut visual, "settings-button-suggestion-index-all-drives");
    settle(&mut visual, |_| fake.calls().iter().any(|c| c == "index_enable"));
    settle(&mut visual, |cx| !has(cx, "Find any file on every drive, instantly"));
    // A tile's name opens its page.
    click(&mut visual, "settings-tile-open-tools");
    assert_eq!(view.read_with(&visual, |v, _| v.page()), Page::Tools);
}

#[gpui_kit::test]
fn a_suggestion_put_away_stays_away(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    visual.update(|_, cx| state::update(cx, |s| s.quick_shortcut = Some("Ctrl+Alt+F".into())));
    settle(&mut visual, |cx| has(cx, "Keep Quick search's shortcut working"));
    click(&mut visual, "settings-suggestion-never-shortcut-needs-tray");
    assert!(!has(&mut visual, "Keep Quick search's shortcut working"));
    let put_away = visual.update(|_, cx| crate::suggestions::dismissed(cx, crate::suggestions::Kind::ShortcutNeedsTray));
    assert!(put_away, "kept in the saved notices");
    // Its button does what it says: keep running in the tray.
    visual.update(|_, cx| state::update(cx, |s| s.keep_in_tray = false));
    settle(&mut visual, |cx| has(cx, "Find any file on every drive, instantly"));
    click(&mut visual, "settings-button-suggestion-index-all-drives");
    settle(&mut visual, |_| fake.calls().iter().any(|c| c == "index_enable"));
}

#[gpui_kit::test]
fn search_finds_a_setting_by_other_words_and_opens_its_page_with_the_row_marked(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let search = visual.update(|_, cx| view.read(cx).search.clone());
    type_into(&mut visual, &search, "tray");
    assert_eq!(
        role_of(&mut visual, "Quick search › Keep running in the tray"),
        Some(Role::Button)
    );
    type_into(&mut visual, &search, "zzzz");
    assert!(has(&mut visual, "No setting matches “zzzz”"));
    type_into(&mut visual, &search, "dark mode");
    click(&mut visual, "settings-found-0");
    assert_eq!(view.read_with(&visual, |v, _| v.page()), Page::Appearance);
    assert!(visual.update(|_, cx| search.read(cx).value().is_empty()), "the search is put away");
    assert!(has(&mut visual, "Theme"));
    // Enter opens the first one found.
    type_into(&mut visual, &search, "start with");
    visual.update(|window, cx| window.press("enter", cx));
    visual.run_until_parked();
    assert_eq!(view.read_with(&visual, |v, _| v.page()), Page::Quick);
}

#[gpui_kit::test]
fn the_search_index_names_rows_that_are_drawn(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    for entry in pages::SEARCH {
        visual.update(|window, cx| view.update(cx, |v, cx| v.show_page(entry.page, window, cx)));
        settle(&mut visual, |_| true);
        assert!(
            has(&mut visual, entry.label) || entry.label == entry.page.name().as_ref(),
            "search names “{}” on {:?}, which doesn't draw it",
            entry.label,
            entry.page
        );
    }
}

#[gpui_kit::test]
fn the_zoxide_switch_works_once_zoxide_is_on_the_path(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    *lock(&fake.zoxide_installed) = true;
    let (mut visual, _) = open_settings(cx, &fake, None);
    go_to(&mut visual, "history");
    visual.run_until_parked();
    click(&mut visual, "settings-switch-zoxide");
    assert!(settings(&mut visual).zoxide, "zoxide is there, so the switch turns on");
}
