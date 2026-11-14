//! Tests of the sections that talk to the PC through the backend: instant search, the content index, photos and
//! recordings, devices, tools, clouds, agents, the settings file, File Explorer's switches and About.

use super::backend::{IndexState, Known};
use super::tests::*;
use super::*;
use crate::testing;
use glint_core::rclone;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ClipboardItem, TestAppContext, VisualTestContext};
use std::sync::Arc;

fn text_of(item: Option<ClipboardItem>) -> Option<String> {
    item.and_then(|item| item.text())
}

fn label_of(cx: &mut VisualTestContext, id: &str) -> Option<String> {
    let id = SharedString::from(id.to_string());
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).and_then(|e| e.label().map(str::to_string))
    })
}

fn saved_remotes(cx: &mut VisualTestContext) -> Vec<glint_model::store::RemoteConnection> {
    cx.update(|_, cx| persistence::app(cx).map(|app| app.remotes.clone()).unwrap_or_default())
}

#[gpui_kit::test]
fn instant_search_turns_on_says_so_and_turns_off(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "instant-search");
    // Off: it offers to turn on, and says what it is.
    assert!(has(&mut visual, "Index file names and folder sizes"));
    assert!(!has(&mut visual, "On"));
    click(&mut visual, "settings-button-instant-on");
    settle(&mut visual, |_| fake.calls().contains(&"index_enable".to_string()));
    let instant = visual.update(|_, cx| view.read(cx).instant.clone());
    settle(&mut visual, |cx| {
        cx.update(|_, cx| instant.read(cx).info().is_some_and(|i| i.state == IndexState::On))
    });
    // On: the title says so, with a line for the drive, and the button offering to turn it on has gone.
    assert!(has(&mut visual, "On"));
    let described = visual.update(|window, cx| {
        window.render_frame(cx);
        window
            .find("settings-instant-search-state")
            .description()
            .map(str::to_string)
    });
    assert_eq!(described.as_deref(), Some("C: 1,000 items"));
    assert!(label_of(&mut visual, "settings-button-instant-on").is_none());
    // Turning it off asks the service again and goes back to offering.
    click(&mut visual, "settings-button-instant-off");
    settle(&mut visual, |_| fake.calls().contains(&"index_disable".to_string()));
    settle(&mut visual, |cx| label_of(cx, "settings-button-instant-on").is_some());
    assert!(has(&mut visual, "Index file names and folder sizes"));
}

#[gpui_kit::test]
fn instant_search_failing_to_start_keeps_offering_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "instant-search");
    *lock(&fake.fail_next) = Some("access denied".into());
    click(&mut visual, "settings-button-instant-on");
    settle(&mut visual, |_| fake.calls().contains(&"index_enable".to_string()));
    // The reason is said in a notice, and the service stays off, with the spinner gone.
    let instant = visual.update(|_, cx| view.read(cx).instant.clone());
    settle(&mut visual, |cx| !cx.update(|_, cx| instant.read(cx).working()));
    assert_eq!(
        visual.update(|_, cx| instant.read(cx).info().map(|i| i.state)),
        Some(IndexState::Off)
    );
    // A declined approval prompt says nothing at all.
    *lock(&fake.fail_next) = Some("cancelled".into());
    click(&mut visual, "settings-button-instant-on");
    settle(&mut visual, |_| {
        fake.calls().iter().filter(|c| *c == "index_enable").count() == 2
    });
    settle(&mut visual, |cx| !cx.update(|_, cx| instant.read(cx).working()));
}

#[gpui_kit::test]
fn the_content_index_starts_in_documents_and_desktop_and_takes_folders_in_and_out(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "instant-search");
    reveal(&mut visual, &view, "settings-switch-content-index");
    click(&mut visual, "settings-switch-content-index");
    settle(&mut visual, |_| {
        fake.calls()
            .iter()
            .any(|c| c.starts_with("content_configure true") && c.contains("Documents") && c.contains("Desktop"))
    });
    settle(&mut visual, |cx| has(cx, "The text in pictures too"));
    // A folder picked is added once; cancelling adds nothing.
    reveal(&mut visual, &view, "settings-button-content-add");
    lock(&fake.picks).push_back(Some("D:\\Notes".into()));
    click(&mut visual, "settings-button-content-add");
    settle(&mut visual, |cx| {
        has(cx, "D:\\Notes") && cx.update(|_, _| lock(&fake.content).folders.len() == 3)
    });
    reveal(&mut visual, &view, "settings-button-content-add");
    lock(&fake.picks).push_back(None);
    click(&mut visual, "settings-button-content-add");
    settle(&mut visual, |_| lock(&fake.picks).is_empty());
    assert_eq!(lock(&fake.content).folders.len(), 3);
    // Remove takes one out, named for it.
    assert_eq!(
        label_of(&mut visual, "settings-button-content-folder-2").as_deref(),
        Some("Remove D:\\Notes")
    );
    click(&mut visual, "settings-button-content-folder-2");
    settle(&mut visual, |_| lock(&fake.content).folders.len() == 2);
    // The pictures switch writes `pictures`; the main switch off keeps the folders but turns it off.
    reveal(&mut visual, &view, "settings-switch-content-pictures");
    click(&mut visual, "settings-switch-content-pictures");
    settle(&mut visual, |_| !lock(&fake.content).pictures);
    reveal(&mut visual, &view, "settings-switch-content-index");
    click(&mut visual, "settings-switch-content-index");
    settle(&mut visual, |_| !lock(&fake.content).enabled);
    assert_eq!(lock(&fake.content).folders.len(), 2);
}

#[gpui_kit::test]
fn photo_search_picks_pictures_and_downloads_its_model_when_turned_on(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "instant-search");
    reveal(&mut visual, &view, "settings-switch-photos");
    assert!(has(&mut visual, "Photo search (shows:)"));
    click(&mut visual, "settings-switch-photos");
    settle(&mut visual, |_| {
        fake.calls().iter().any(|c| c == "media_install photos")
            && fake
                .calls()
                .iter()
                .any(|c| c.starts_with("media_configure photos=true") && c.contains("Pictures"))
    });
    // The folders chosen are listed, named for removal.
    settle(&mut visual, |cx| {
        label_of(cx, "settings-button-photos-folder-0").is_some()
    });
    assert!(label_of(&mut visual, "settings-button-photos-folder-0")
        .unwrap_or_default()
        .starts_with("Remove "));
}

#[gpui_kit::test]
fn tools_install_with_progress_and_clouds_wait_for_rclone(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    lock(&fake.remotes).push(rclone::Remote {
        name: "سحابتي".into(),
        kind: "drive".into(),
        provider: String::new(),
    });
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "clouds");
    // Without rclone the Clouds card says why it's wanted and offers to install it.
    assert!(has(
        &mut visual,
        "Google Drive, OneDrive, Dropbox and 60 more clouds open like folders with rclone, and Folder rules can keep folders in sync with them."
    ));
    click(&mut visual, "settings-button-needed-rclone-install");
    settle(&mut visual, |_| {
        fake.calls().contains(&"tool_install rclone".to_string())
    });
    // Once it's there the card reads rclone: its version, its configuration and the remotes found.
    settle(&mut visual, |cx| has(cx, "rclone 1.68.0"));
    assert!(has(&mut visual, "سحابتي"));
    assert!(label_of(&mut visual, "settings-button-clouds-connect").is_some());
    // Show in the sidebar saves a connection for the remote, named for it.
    click(&mut visual, "settings-button-clouds-show-0");
    let saved = saved_remotes(&mut visual);
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].rclone.as_deref(), Some("سحابتي"));
    assert_eq!(saved[0].service.as_deref(), Some("drive"));
    assert_eq!(saved[0].kind, glint_model::store::RemoteKind::Rclone);
    settle(&mut visual, |cx| {
        label_of(cx, "settings-button-clouds-show-0").is_none()
    });
    assert!(has(&mut visual, "Sign out of “سحابتي”"));
    // Sign out asks first; Cancel changes nothing; OK deletes it from rclone and the sidebar.
    click(&mut visual, "settings-button-clouds-sign-out-0");
    click(&mut visual, "settings-confirm-cancel");
    assert!(!fake.calls().iter().any(|c| c.starts_with("rclone_delete")));
    click(&mut visual, "settings-button-clouds-sign-out-0");
    click(&mut visual, "settings-confirm-ok");
    settle(&mut visual, |_| {
        fake.calls().contains(&"rclone_delete سحابتي".to_string())
    });
    settle(&mut visual, |cx| saved_remotes(cx).is_empty());
    settle(&mut visual, |cx| {
        has(cx, "No clouds yet: Connect a cloud signs in to one.")
    });
    // Connect a cloud closes Settings and asks the app for the connect dialog.
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let heard = asked.clone();
    visual.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &SettingsEvent, _| {
            heard.borrow_mut().push(*event)
        })
        .detach();
    });
    click(&mut visual, "settings-button-clouds-connect");
    visual.run_until_parked();
    assert_eq!(*asked.borrow(), vec![SettingsEvent::ConnectCloud]);
    visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Settings goes first"));
}

#[gpui_kit::test]
fn a_locked_rclone_asks_for_its_password_and_a_wrong_one_says_so(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    *lock(&fake.rclone_installed) = true;
    *lock(&fake.rclone_locked) = true;
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "clouds");
    settle(&mut visual, |cx| has(cx, "Its configuration is encrypted"));
    let clouds = visual.update(|_, cx| view.read(cx).clouds.clone());
    // The password box is masked and named; Unlock waits for something typed.
    assert_eq!(
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("settings-button-clouds-unlock").disabled()
        }),
        Some(true)
    );
    let password = visual.update(|_, cx| clouds.read(cx).password_input().clone());
    type_into(&mut visual, &password, "wrong");
    click(&mut visual, "settings-button-clouds-unlock");
    settle(&mut visual, |cx| has(cx, "That isn't its password"));
    type_into(&mut visual, &password, "right");
    click(&mut visual, "settings-button-clouds-unlock");
    settle(&mut visual, |cx| !has(cx, "Its configuration is encrypted"));
}

#[gpui_kit::test]
fn agents_copy_their_setup_and_undo_what_they_did(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "ai-agents");
    settle(&mut visual, |cx| {
        label_of(cx, "settings-agent-text-claude-code").is_some()
    });
    reveal(&mut visual, &view, "settings-button-agents-copy-claude-code");
    click(&mut visual, "settings-button-agents-copy-claude-code");
    assert_eq!(
        text_of(visual.update(|_, cx| cx.read_from_clipboard())).as_deref(),
        Some("claude mcp add glint -- \"C:\\Glint.exe\" mcp")
    );
    // What agents did is listed with Undo, and Windows' registry is offered.
    assert!(has(&mut visual, "What agents did"));
    reveal(&mut visual, &view, "settings-button-agents-undo-7");
    click(&mut visual, "settings-button-agents-undo-7");
    settle(&mut visual, |_| fake.calls().contains(&"agent_undo 7".to_string()));
    reveal(&mut visual, &view, "settings-button-agents-register");
    click(&mut visual, "settings-button-agents-register");
    settle(&mut visual, |_| fake.calls().contains(&"agents_register".to_string()));
}

#[gpui_kit::test]
fn file_explorer_switches_change_together_the_way_the_typescript_did(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "files");
    reveal(&mut visual, &view, "settings-switch-file-explorer-menu");
    click(&mut visual, "settings-switch-file-explorer-menu");
    settle(&mut visual, |_| {
        fake.calls()
            .contains(&"set_explorer_integration true false Some(false)".to_string())
    });
    // "By default" turns the menu on with it.
    reveal(&mut visual, &view, "settings-switch-file-explorer-default");
    click(&mut visual, "settings-switch-file-explorer-default");
    settle(&mut visual, |_| {
        fake.calls()
            .contains(&"set_explorer_integration true true Some(false)".to_string())
    });
}

#[gpui_kit::test]
fn the_settings_file_exports_everything_and_imports_the_parts_ticked(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let dir = std::env::temp_dir().join(format!("glint-settings-file-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("out.json");
    let _ = std::fs::remove_file(&path);
    visual.update(|_, cx| {
        state::update(cx, |s| s.show_hidden = true);
        persistence::update_app(cx, |app| app.pinned.push("C:\\Projects".into()));
    });
    go_to(&mut visual, "about");
    reveal(&mut visual, &view, "settings-button-transfer-export");
    lock(&fake.picks).push_back(Some(path.display().to_string()));
    click(&mut visual, "settings-button-transfer-export");
    settle(&mut visual, |_| path.exists());
    let name = fake
        .calls()
        .into_iter()
        .find(|c| c.starts_with("save_settings_file "))
        .unwrap_or_default();
    assert!(
        name.starts_with("save_settings_file Glint settings 20") && name.ends_with(".json"),
        "{name}"
    );
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        written.contains("\"glint\": \"settings\"") && written.contains("C:\\\\Projects"),
        "{written}"
    );

    // Change things, then import: the pinned folder part only, replacing.
    visual.update(|_, cx| {
        state::update(cx, |s| s.show_hidden = false);
        persistence::update_app(cx, |app| app.pinned = vec!["D:\\Else".into()]);
    });
    lock(&fake.picks).push_back(Some(path.display().to_string()));
    reveal(&mut visual, &view, "settings-button-transfer-import");
    click(&mut visual, "settings-button-transfer-import");
    settle(&mut visual, |cx| label_of(cx, "settings-incoming").is_some());
    assert_eq!(label_of(&mut visual, "settings-incoming").as_deref(), Some("out.json"));
    // Everything in it is ticked except history and the folders Ctrl+K ranks (which it has none of).
    let transfer = visual.update(|_, cx| view.read(cx).about.read(cx).transfer().clone());
    let ticked = visual.update(|_, cx| transfer.read(cx).ticked());
    assert!(ticked.contains(&persistence::Part::Settings) && ticked.contains(&persistence::Part::Pinned));
    // Take only the pinned folders: untick Settings.
    reveal(&mut visual, &view, "settings-part-settings");
    click(&mut visual, "settings-part-settings");
    reveal(&mut visual, &view, "settings-button-transfer-replace");
    click(&mut visual, "settings-button-transfer-replace");
    settle(&mut visual, |cx| {
        cx.update(|_, cx| persistence::app(cx).is_some_and(|a| a.pinned == vec!["C:\\Projects".to_string()]))
    });
    assert!(!settings(&mut visual).show_hidden, "the Settings part wasn't ticked");
    // And a file that isn't one says so.
    let bad = dir.join("bad.json");
    std::fs::write(&bad, "{\"hello\": 1}").ok();
    lock(&fake.picks).push_back(Some(bad.display().to_string()));
    reveal(&mut visual, &view, "settings-button-transfer-import");
    click(&mut visual, "settings-button-transfer-import");
    settle(&mut visual, |_| lock(&fake.picks).is_empty());
    visual.run_until_parked();
    // Nothing is offered from a file that isn't one.
    let offering = visual.update(|_, cx| view.read(cx).about.read(cx).transfer().read(cx).offering());
    assert!(!offering);
    // Importing the whole thing, adding, brings the setting back.
    lock(&fake.picks).push_back(Some(path.display().to_string()));
    reveal(&mut visual, &view, "settings-button-transfer-import");
    click(&mut visual, "settings-button-transfer-import");
    settle(&mut visual, |cx| label_of(cx, "settings-incoming").is_some());
    reveal(&mut visual, &view, "settings-button-transfer-add");
    click(&mut visual, "settings-button-transfer-add");
    settle(&mut visual, |cx| settings(cx).show_hidden);
}

#[gpui_kit::test]
fn about_copies_diagnostics_and_closes_itself_for_the_shortcuts_sheet(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    go_to(&mut visual, "about");
    reveal(&mut visual, &view, "settings-button-about-diagnostics");
    click(&mut visual, "settings-button-about-diagnostics");
    settle(&mut visual, |cx| {
        text_of(cx.update(|_, cx| cx.read_from_clipboard())).as_deref() == Some("Glint 0.10.0\nrg 14")
    });
    // Without an update model (these windows are made without the app's start-up) the check is drawn, and waits.
    assert_eq!(
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("settings-button-about-update").disabled()
        }),
        Some(true)
    );
    reveal(&mut visual, &view, "settings-button-about-shortcuts");
    click(&mut visual, "settings-button-about-shortcuts");
    visual.run_until_parked();
    visual.update(|window, cx| assert!(!window.has_active_dialog(cx), "Settings goes first"));
}

#[gpui_kit::test]
fn the_icons_settings_draws_are_in_the_embedded_assets(_cx: &mut TestAppContext) {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    for name in [
        IconName::Archive,
        IconName::Binary,
        IconName::ClipboardCopy,
        IconName::Cloud,
        IconName::Copy,
        IconName::Download,
        IconName::ExternalLink,
        IconName::FileDown,
        IconName::FileUp,
        IconName::Folder,
        IconName::Heart,
        IconName::Image,
        IconName::Keyboard,
        IconName::Lock,
        IconName::LockOpen,
        IconName::LogOut,
        IconName::Plus,
        IconName::RefreshCw,
        IconName::RotateCcw,
        IconName::ShieldCheck,
        IconName::Sparkles,
        IconName::Type,
        IconName::WandSparkles,
        IconName::X,
        IconName::Zap,
    ] {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
    // The known folders the sections suggest are asked of the backend by name.
    let fake = Fake::new();
    assert!(Backend::known_folder(&fake, Known::Pictures).is_some());
}

// ---- About: updates ----------------------------------------------------------------------------------

/// The update model of `updater`'s tests, shared with the Settings that was already open.
fn with_updates(
    cx: &mut VisualTestContext,
    policy: crate::updater::Policy,
    current: &str,
) -> crate::updater::tests::Rig {
    cx.update(|_, cx| {
        let rig = crate::updater::tests::build_rig(cx, policy, current, false);
        crate::updater::set_shared(cx, rig.updater.clone());
        rig
    })
}

/// The hint under the row called `row` (what a screen reader reads after its name).
fn hint_of(cx: &mut VisualTestContext, row: &str) -> Option<String> {
    let id = SharedString::from(format!("settings-row-{row}"));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).and_then(|e| e.description().map(str::to_string))
    })
}

#[gpui_kit::test]
fn about_checks_for_updates_and_says_how_it_went(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let rig = with_updates(&mut visual, crate::updater::Policy::SelfUpdating, "0.9.5");
    go_to(&mut visual, "about");
    reveal(&mut visual, &view, "settings-button-about-update");
    assert_eq!(hint_of(&mut visual, "Updates"), None);
    // Nothing newer: it says so, and the button is there to ask again.
    click(&mut visual, "settings-button-about-update");
    settle(&mut visual, |cx| {
        hint_of(cx, "Updates").as_deref() == Some("You have the latest version")
    });
    assert_ne!(
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.find("settings-button-about-update").disabled()
        }),
        Some(true)
    );
    // Offline it says that plainly, not in the library's words.
    rig.fake.fail("error sending request for url (https://github.com/)");
    click(&mut visual, "settings-button-about-update");
    settle(&mut visual, |cx| {
        hint_of(cx, "Updates").as_deref() == Some("Couldn't reach GitHub to check. Are you online?")
    });
    // Any other failure is said as it came.
    rig.fake.fail("GitHub answered 403 Forbidden");
    click(&mut visual, "settings-button-about-update");
    settle(&mut visual, |cx| {
        hint_of(cx, "Updates").as_deref() == Some("Couldn't check: GitHub answered 403 Forbidden")
    });
}

#[gpui_kit::test]
fn about_offers_a_newer_version_and_installs_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let rig = with_updates(&mut visual, crate::updater::Policy::SelfUpdating, "0.9.5");
    rig.fake.serve("0.99.0", crate::updater::tests::SIG_GOOD);
    go_to(&mut visual, "about");
    reveal(&mut visual, &view, "settings-button-about-update");
    click(&mut visual, "settings-button-about-update");
    settle(&mut visual, |cx| has(cx, "Version 0.99.0 is available"));
    // The check has given way to What's new and Install, and the row says what installing does.
    assert!(label_of(&mut visual, "settings-button-about-update").is_none());
    assert_eq!(
        label_of(&mut visual, "settings-button-about-install").as_deref(),
        Some("Install")
    );
    assert!(label_of(&mut visual, "settings-button-about-whats-new").is_some());
    assert_eq!(
        hint_of(&mut visual, "Version 0.99.0 is available").as_deref(),
        Some("Installing it restarts the app")
    );
    // Install downloads, checks the signature, and (the questions answered) runs it and quits.
    click(&mut visual, "settings-button-about-install");
    settle(&mut visual, |_| *rig.quit.borrow() == 1);
}

#[gpui_kit::test]
fn about_says_plainly_when_an_update_was_refused(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let rig = with_updates(&mut visual, crate::updater::Policy::SelfUpdating, "0.9.5");
    // The file's signature is for another version than the feed announces.
    rig.fake.serve("1.0.0", crate::updater::tests::SIG_GOOD);
    go_to(&mut visual, "about");
    reveal(&mut visual, &view, "settings-button-about-update");
    click(&mut visual, "settings-button-about-update");
    settle(&mut visual, |cx| has(cx, "Version 1.0.0 is available"));
    click(&mut visual, "settings-button-about-install");
    settle(&mut visual, |cx| {
        hint_of(cx, "Version 1.0.0 is available").is_some_and(|hint| hint.starts_with("Couldn't install: "))
    });
    let hint = hint_of(&mut visual, "Version 1.0.0 is available").unwrap_or_default();
    assert!(
        hint.contains("signed as version 0.99.0 but announced as 1.0.0"),
        "{hint}"
    );
    assert_eq!(*rig.quit.borrow(), 0);
    // Install is there to try again.
    assert_eq!(
        label_of(&mut visual, "settings-button-about-install").as_deref(),
        Some("Install")
    );
}

/// About in a copy that never updates itself: the reason, and no check.
fn about_without_updates(cx: &mut TestAppContext, policy: crate::updater::Policy, hint: &str) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, _) = open_settings(cx, &fake, None);
    let rig = with_updates(&mut visual, policy, "0.9.5");
    rig.fake.serve("0.99.0", crate::updater::tests::SIG_GOOD);
    go_to(&mut visual, "about");
    frame(&mut visual);
    assert_eq!(hint_of(&mut visual, "Updates").as_deref(), Some(hint));
    assert!(label_of(&mut visual, "settings-button-about-update").is_none());
    assert!(rig.fake.asked().is_empty());
}

#[gpui_kit::test]
fn about_has_no_check_in_the_stores_copy(cx: &mut TestAppContext) {
    about_without_updates(
        cx,
        crate::updater::Policy::Store,
        "The Microsoft Store keeps Glint up to date",
    );
}

#[gpui_kit::test]
fn about_has_no_check_in_a_debug_build(cx: &mut TestAppContext) {
    about_without_updates(cx, crate::updater::Policy::Off, "Updates are off in a debug build");
}

#[gpui_kit::test]
fn about_updates_row_mirrors_in_arabic(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let fake = Arc::new(Fake::new());
    let (mut visual, view) = open_settings(cx, &fake, None);
    let rig = with_updates(&mut visual, crate::updater::Policy::SelfUpdating, "0.9.5");
    rig.fake.serve("0.99.0", crate::updater::tests::SIG_GOOD);
    go_to(&mut visual, "about");
    reveal(&mut visual, &view, "settings-button-about-update");
    click(&mut visual, "settings-button-about-update");
    settle(&mut visual, |cx| has(cx, "Version 0.99.0 is available"));
    reveal(&mut visual, &view, "settings-button-about-install");
    let place = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            let row = window.find("settings-row-Version 0.99.0 is available").bounds();
            let button = window.find("settings-button-about-install").bounds();
            (row.center().x, button.center().x)
        })
    };
    let (row, button) = place(&mut visual);
    assert!(button > row, "left to right: Install is at the end, on the right");
    visual.update(|_, cx| state::update(cx, |s| s.language = glint_model::store::Language::Ar));
    visual.run_until_parked();
    assert!(crate::i18n::is_rtl());
    reveal(&mut visual, &view, "settings-button-about-install");
    let (row, button) = place(&mut visual);
    assert!(button < row, "right to left: Install is at the end, on the left");
    assert!(label_of(&mut visual, "settings-button-about-install").is_some());
}
