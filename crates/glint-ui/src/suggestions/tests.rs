//! What is suggested from the PC's setup, what each moment brings, how often they may come up, and what their
//! buttons do.

use super::moments::{after, noticed, still_holds, Moment, Seen};
use super::*;
use crate::testing;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{TestAppContext, VisualTestContext};
use std::cell::RefCell;

fn kinds(facts: &Facts) -> Vec<Kind> {
    all(facts).into_iter().map(|s| s.kind).collect()
}

// ---- From how the PC is set up ----

#[test]
fn instant_search_off_comes_first() {
    let facts = Facts {
        index: Some(IndexState::Off),
        tool_updates: Some(2),
        ..Facts::default()
    };
    assert_eq!(kinds(&facts), [Kind::IndexAllDrives, Kind::ToolUpdates]);
    let on = Facts {
        index: Some(IndexState::On),
        ..facts.clone()
    };
    assert_eq!(kinds(&on), [Kind::ToolUpdates]);
    // A PC whose drives can't be indexed isn't asked.
    let unsupported = Facts {
        index: Some(IndexState::Unsupported),
        ..Facts::default()
    };
    assert!(kinds(&unsupported).is_empty());
    assert_eq!(
        kinds(&Facts::default()),
        Vec::<Kind>::new(),
        "nothing known, nothing said"
    );
}

#[test]
fn a_drive_is_nearly_full_by_share_and_by_size() {
    const GB: u64 = 1 << 30;
    let room = |root: &str, free: u64, total: u64| Room {
        root: root.into(),
        free,
        total,
    };
    let facts = |drives| Facts {
        drives: Some(drives),
        ..Facts::default()
    };
    // 8 GB of 256: full. 100 GB of 2 TB is 5% but plenty: not.
    let found = all(&facts(vec![
        room("C:\\", 8 * GB, 256 * GB),
        room("D:\\", 100 * GB, 2048 * GB),
    ]));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].about, About::Drive("C:\\".into()));
    assert_eq!(found[0].title, "C: is nearly full");
    assert!(all(&facts(vec![room("C:\\", 60 * GB, 256 * GB)])).is_empty());
    // The fullest is the one named.
    let two = all(&facts(vec![
        room("C:\\", 9 * GB, 256 * GB),
        room("E:\\", 2 * GB, 128 * GB),
    ]));
    assert_eq!(two[0].about, About::Drive("E:\\".into()));
}

#[test]
fn settings_suggest_what_goes_with_them() {
    let shortcut_alone = Facts {
        quick_shortcut: true,
        ..Facts::default()
    };
    assert_eq!(kinds(&shortcut_alone), [Kind::ShortcutNeedsTray]);
    let tray = Facts {
        quick_shortcut: true,
        keep_in_tray: true,
        ..Facts::default()
    };
    assert_eq!(kinds(&tray), [Kind::StartWithWindows]);
    let all_set = Facts {
        start_with_windows: true,
        ..tray
    };
    assert!(kinds(&all_set).is_empty());
    let light_on_dark = Facts {
        theme: Some(Theme::Light),
        windows_dark: true,
        ..Facts::default()
    };
    assert_eq!(kinds(&light_on_dark), [Kind::MatchWindowsTheme]);
    let follows = Facts {
        theme: Some(Theme::System),
        windows_dark: true,
        ..Facts::default()
    };
    assert!(kinds(&follows).is_empty());
    let cloudless = Facts {
        rclone: Some(true),
        clouds: Some(0),
        ..Facts::default()
    };
    assert_eq!(kinds(&cloudless), [Kind::ConnectCloud]);
}

#[test]
fn zoxide_and_an_agent_are_suggested_only_while_they_are_there_and_not_set_up() {
    let zoxide = Facts {
        zoxide: Some(true),
        ..Facts::default()
    };
    assert_eq!(kinds(&zoxide), [Kind::ShareWithZoxide]);
    let shared = Facts {
        zoxide_shared: true,
        ..zoxide
    };
    assert!(kinds(&shared).is_empty());
    assert!(kinds(&Facts {
        zoxide: Some(false),
        ..Facts::default()
    })
    .is_empty());
    let agent = Facts {
        agent: Some("Claude".into()),
        ..Facts::default()
    };
    let found = all(&agent);
    assert_eq!(found[0].kind, Kind::ConnectAgent);
    assert_eq!(found[0].title, "Let Claude find your files");
}

#[test]
fn what_may_come_up_at_start_and_what_a_press_settles() {
    for kind in [
        Kind::IndexAllDrives,
        Kind::FullDrive,
        Kind::ToolUpdates,
        Kind::XyplorerMarks,
        Kind::XyplorerFollow,
    ] {
        assert!(kind.may_notify(), "{kind:?}");
    }
    // These wait for their own moment, or in the Overview.
    for kind in [
        Kind::StartWithWindows,
        Kind::MatchWindowsTheme,
        Kind::ConnectCloud,
        Kind::ShortcutNeedsTray,
    ] {
        assert!(!kind.may_notify(), "{kind:?}");
    }
    // A full drive can be full again and another folder can hold photos: pressing their button settles nothing.
    for kind in [
        Kind::FullDrive,
        Kind::ToolUpdates,
        Kind::GalleryFolder,
        Kind::FolderRule,
        Kind::XyplorerMarks,
    ] {
        assert!(!kind.teaches(), "{kind:?}");
    }
    for kind in [
        Kind::CopyQueue,
        Kind::AskBeforeDelete,
        Kind::BatchRename,
        Kind::QuickLook,
        Kind::NameLabels,
    ] {
        assert!(kind.teaches(), "{kind:?}");
    }
}

#[test]
fn every_kind_has_an_id_of_its_own() {
    let mut ids: Vec<&str> = Kind::ALL.iter().map(|kind| kind.id()).collect();
    assert_eq!(
        Kind::XyplorerMarks.id(),
        "xyplorer-marks",
        "kept from 0.17's first build: what was put away stays put away"
    );
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), Kind::ALL.len(), "no two kinds share an id");
}

// ---- XYplorer (F27) ----

const TAG_DAT: &str = "C:\\Users\\me\\AppData\\Roaming\\XYplorer\\tag.dat";

fn xy(items: u64, brought_in: bool) -> Facts {
    Facts {
        xyplorer: Some(XyFound {
            path: TAG_DAT.to_string(),
            items,
            brought_in,
        }),
        ..Facts::default()
    }
}

#[test]
fn a_tag_dat_with_marks_is_brought_in_with_one_press_or_looked_at_first() {
    let found = all(&xy(1284, false));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].kind, Kind::XyplorerMarks);
    assert_eq!(found[0].title, "Bring your XYplorer labels, tags and comments");
    assert!(
        found[0].body.starts_with("1,284 items marked in XYplorer."),
        "{}",
        found[0].body
    );
    assert_eq!(found[0].button, "Bring them in");
    assert_eq!(found[0].also.as_deref(), Some("Look first…"));
    assert_eq!(
        found[0].about,
        About::TagDat {
            path: TAG_DAT.to_string(),
            brought_in: false
        }
    );
    assert!(kinds(&Facts::default()).is_empty(), "unknown or none found: nothing");
    let order = Facts {
        tool_updates: Some(1),
        quick_shortcut: true,
        ..xy(3, false)
    };
    assert_eq!(
        kinds(&order),
        [Kind::ToolUpdates, Kind::XyplorerMarks, Kind::ShortcutNeedsTray]
    );
}

#[test]
fn an_xyplorer_with_nothing_to_bring_is_one_to_follow_until_it_is_followed() {
    // Just installed, nothing marked yet: no "0 items" to bring, but something to follow.
    let fresh = all(&xy(0, false));
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].kind, Kind::XyplorerFollow);
    assert!(
        fresh[0].body.starts_with("XYplorer is on this PC."),
        "{}",
        fresh[0].body
    );
    assert_eq!(fresh[0].button, "Follow XYplorer");
    assert!(fresh[0].also.is_none());
    // Brought in before: what it gains since is what following is for.
    let brought = all(&xy(40, true));
    assert_eq!(brought[0].kind, Kind::XyplorerFollow);
    assert!(brought[0].body.contains("from now on"), "{}", brought[0].body);
    // Followed: nothing more to say.
    let followed = Facts {
        follow_xyplorer: true,
        ..xy(40, true)
    };
    assert!(kinds(&followed).is_empty());
    assert!(kinds(&Facts {
        follow_xyplorer: true,
        ..xy(0, false)
    })
    .is_empty());
}

// ---- Moments ----

fn defaults() -> Settings {
    Settings::default()
}

fn renamed(to: &str) -> Moment {
    Moment::Renamed { to: to.to_string() }
}

#[test]
fn three_renames_in_one_folder_offer_batch_rename_for_those_three() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    assert!(after(&mut seen, &renamed("D:\\a\\one.txt"), now, &s, false).is_none());
    assert!(after(&mut seen, &renamed("D:\\a\\two.txt"), now, &s, false).is_none());
    let third = after(&mut seen, &renamed("D:\\a\\three.txt"), now, &s, false).expect("the third");
    assert_eq!(third.kind, Kind::BatchRename);
    assert_eq!(third.button, "Rename these 3…");
    assert_eq!(
        third.about,
        About::Files(vec![
            "D:\\a\\one.txt".into(),
            "D:\\a\\two.txt".into(),
            "D:\\a\\three.txt".into()
        ])
    );
    // It starts over: the next rename is the first of three again.
    assert!(after(&mut seen, &renamed("D:\\a\\four.txt"), now, &s, false).is_none());
}

#[test]
fn renames_in_other_folders_or_long_ago_do_not_count() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    assert!(after(&mut seen, &renamed("D:\\a\\one.txt"), now, &s, false).is_none());
    assert!(after(&mut seen, &renamed("D:\\a\\two.txt"), now, &s, false).is_none());
    // Another folder: the two before it are another job.
    assert!(after(&mut seen, &renamed("D:\\b\\x.txt"), now, &s, false).is_none());
    assert!(after(&mut seen, &renamed("D:\\b\\y.txt"), now, &s, false).is_none());
    // Ten minutes on, the third in that folder is the first again.
    let later = now + Duration::from_secs(600);
    assert!(after(&mut seen, &renamed("D:\\b\\z.txt"), later, &s, false).is_none());
    // The same item renamed twice is one item.
    let mut seen = Seen::default();
    for _ in 0..4 {
        assert!(after(&mut seen, &renamed("D:\\a\\same.txt"), now, &s, false).is_none());
    }
}

fn placed(from: &str, dest: &str, ext: &str, moved: bool) -> Moment {
    Moment::Placed {
        from: from.to_string(),
        dest: dest.to_string(),
        exts: vec![ext.to_string()],
        moved,
    }
}

#[test]
fn one_kind_moved_the_same_way_three_times_offers_a_rule_for_it() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    let moved = placed("C:\\Users\\me\\Downloads", "D:\\Papers\\Invoices", "pdf", true);
    // Two panes are showing, so only the rule can come of it.
    assert!(after(&mut seen, &moved, now, &s, true).is_none());
    assert!(after(&mut seen, &moved, now, &s, true).is_none());
    let third = after(&mut seen, &moved, now, &s, true).expect("the third move");
    assert_eq!(third.kind, Kind::FolderRule);
    assert_eq!(third.title, "Always move PDF files to “Invoices”?");
    assert!(
        third.body.starts_with("That is 3 times from “Downloads”."),
        "{}",
        third.body
    );
    assert_eq!(
        third.about,
        About::Rule {
            from: "C:\\Users\\me\\Downloads".into(),
            names: "*.pdf".into(),
            to: "D:\\Papers\\Invoices".into()
        }
    );
    // Copies, folders and mixed kinds are no habit a rule could have.
    let mut seen = Seen::default();
    for _ in 0..4 {
        assert!(after(&mut seen, &placed("D:\\a", "D:\\b", "pdf", false), now, &s, true).is_none());
        assert!(after(&mut seen, &placed("D:\\a", "D:\\b", "", true), now, &s, true).is_none());
    }
}

#[test]
fn going_between_two_folders_three_times_offers_two_panes_unless_they_show() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    assert!(after(&mut seen, &placed("D:\\a", "D:\\b", "txt", false), now, &s, false).is_none());
    // Either way round is the same two folders.
    assert!(after(&mut seen, &placed("D:\\b", "D:\\a", "jpg", false), now, &s, false).is_none());
    let third = after(&mut seen, &placed("D:\\a", "D:\\b", "", false), now, &s, false).expect("the third");
    assert_eq!(third.kind, Kind::TwoPanes);
    assert_eq!(third.about, About::Folder("D:\\b".into()));
    assert!(third.body.contains("“a” and “b”"), "{}", third.body);
    // With two panes on screen there is nothing to offer.
    let mut seen = Seen::default();
    for _ in 0..5 {
        assert!(after(&mut seen, &placed("D:\\a", "D:\\b", "", false), now, &s, true).is_none());
    }
    // Into the folder it came from is no going between two.
    assert!(after(&mut seen, &placed("D:\\a", "D:\\a", "txt", false), now, &s, false).is_none());
}

#[test]
fn a_slow_copy_and_an_undone_deletion_offer_their_settings_while_those_are_off() {
    let (mut seen, now) = (Seen::default(), Instant::now());
    let off = defaults();
    assert_eq!(
        after(&mut seen, &Moment::SlowTransfer, now, &off, false).map(|s| s.kind),
        Some(Kind::CopyQueue)
    );
    assert_eq!(
        after(&mut seen, &Moment::UndidDeletion, now, &off, false).map(|s| s.kind),
        Some(Kind::AskBeforeDelete)
    );
    let on = Settings {
        copy_queue: true,
        confirm_delete: true,
        ..defaults()
    };
    assert!(after(&mut seen, &Moment::SlowTransfer, now, &on, false).is_none());
    assert!(after(&mut seen, &Moment::UndidDeletion, now, &on, false).is_none());
}

#[test]
fn four_files_of_a_folder_opened_in_a_row_offer_quick_look() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    let opened = |name: &str| Moment::Opened {
        path: format!("D:\\Photos\\{name}"),
    };
    for name in ["a.jpg", "b.JPG", "c.png"] {
        assert!(after(&mut seen, &opened(name), now, &s, false).is_none());
    }
    // A program isn't something to look at.
    assert!(after(&mut seen, &opened("setup.exe"), now, &s, false).is_none());
    assert_eq!(
        after(&mut seen, &opened("d.pdf"), now, &s, false).map(|s| s.kind),
        Some(Kind::QuickLook)
    );
    // Spread over an afternoon it is no look through a folder.
    let mut seen = Seen::default();
    for (i, name) in ["a.jpg", "b.jpg", "c.jpg", "d.jpg", "e.jpg"].into_iter().enumerate() {
        let when = now + Duration::from_secs(600 * i as u64);
        assert!(after(&mut seen, &opened(name), when, &s, false).is_none());
    }
}

#[test]
fn labels_used_with_the_names_they_came_with_offer_naming_them() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    assert!(after(&mut seen, &Moment::Labeled, now, &s, false).is_none());
    assert!(after(&mut seen, &Moment::Labeled, now, &s, false).is_none());
    assert_eq!(
        after(&mut seen, &Moment::Labeled, now, &s, false).map(|s| s.kind),
        Some(Kind::NameLabels)
    );
    // Someone who named one already knows where that is.
    let mut named = defaults();
    named.labels[0].name = "Urgent".into();
    let mut seen = Seen::default();
    for _ in 0..5 {
        assert!(after(&mut seen, &Moment::Labeled, now, &named, false).is_none());
    }
}

#[test]
fn a_tag_or_a_comment_written_says_how_to_find_it_again() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    let tagged = after(&mut seen, &Moment::Tagged { tag: "work".into() }, now, &s, false).expect("a tag");
    assert_eq!(tagged.kind, Kind::FindMarks);
    assert_eq!(tagged.about, About::Search("tag:work".into()));
    assert!(tagged.body.starts_with("tag:work in the search box"), "{}", tagged.body);
    // A tag of two words is quoted, as the search box wants it; a Chinese one needs nothing.
    let two = after(&mut seen, &Moment::Tagged { tag: "to read".into() }, now, &s, false).expect("a tag");
    assert_eq!(two.about, About::Search("tag:\"to read\"".into()));
    let zh = after(&mut seen, &Moment::Tagged { tag: "工作".into() }, now, &s, false).expect("a tag");
    assert_eq!(zh.about, About::Search("tag:工作".into()));
    let commented = after(&mut seen, &Moment::Commented, now, &s, false).expect("a comment");
    assert_eq!(commented.about, About::Search("has:comment".into()));
}

#[test]
fn a_folder_of_photos_is_offered_to_the_gallery_by_its_name_and_count() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    let here = Moment::PhotoFolder {
        folder: "D:\\Trips\\Lisbon 2024".into(),
        files: 312,
    };
    let offered = after(&mut seen, &here, now, &s, false).expect("offered");
    assert_eq!(offered.kind, Kind::GalleryFolder);
    assert_eq!(offered.title, "Show “Lisbon 2024” in the Gallery?");
    assert!(
        offered.body.starts_with("312 photos and videos here."),
        "{}",
        offered.body
    );
    assert_eq!(offered.about, About::Folder("D:\\Trips\\Lisbon 2024".into()));
    let elsewhere = Moment::PhotosElsewhere {
        folder: "D:\\Camera".into(),
        files: 2340,
    };
    let offered = after(&mut seen, &elsewhere, now, &s, false).expect("offered");
    assert_eq!(offered.title, "“Camera” isn't in your Gallery");
    assert!(
        offered.body.starts_with("2,340 photos and videos are in"),
        "{}",
        offered.body
    );
    // In the Gallery since: it no longer holds.
    assert!(still_holds(&offered, &s));
    let added = Settings {
        gallery_folders: vec!["d:\\camera".into()],
        ..defaults()
    };
    assert!(!still_holds(&offered, &added));
}

#[test]
fn windows_changing_its_mode_offers_following_it_only_to_a_theme_set_by_hand() {
    let (mut seen, now) = (Seen::default(), Instant::now());
    let went_dark = Moment::WindowsThemeChanged { dark: true };
    let light = Settings {
        theme: Theme::Light,
        ..defaults()
    };
    let offered = after(&mut seen, &went_dark, now, &light, false).expect("light by hand, Windows dark");
    assert_eq!(offered.kind, Kind::MatchWindowsTheme);
    assert_eq!(offered.title, "Windows went dark");
    // Dark by hand and Windows dark: they agree. Following already: nothing to offer.
    let dark = Settings {
        theme: Theme::Dark,
        ..defaults()
    };
    assert!(after(&mut seen, &went_dark, now, &dark, false).is_none());
    assert!(after(&mut seen, &went_dark, now, &defaults(), false).is_none());
}

#[test]
fn the_shortcut_used_again_and_again_offers_the_tray_while_glint_quits_with_its_window() {
    let (mut seen, now, s) = (Seen::default(), Instant::now(), defaults());
    assert!(after(&mut seen, &Moment::ShortcutUsed, now, &s, false).is_none());
    assert!(after(&mut seen, &Moment::ShortcutUsed, now, &s, false).is_none());
    assert_eq!(
        after(&mut seen, &Moment::ShortcutUsed, now, &s, false).map(|s| s.kind),
        Some(Kind::ShortcutNeedsTray)
    );
    let kept = Settings {
        keep_in_tray: true,
        ..defaults()
    };
    let mut seen = Seen::default();
    for _ in 0..5 {
        assert!(after(&mut seen, &Moment::ShortcutUsed, now, &kept, false).is_none());
    }
}

#[test]
fn the_pace_is_a_few_a_run_and_never_two_close_together() {
    let now = Instant::now();
    assert!(pace_allows(0, None, now), "the first may come");
    assert!(!pace_allows(1, Some(now), now), "not right after another");
    assert!(!pace_allows(1, Some(now), now + GAP - Duration::from_secs(1)));
    assert!(pace_allows(1, Some(now), now + GAP));
    assert!(
        !pace_allows(MOST_A_RUN, Some(now), now + GAP * 10),
        "and no more than a few in a run"
    );
}

// ---- In a window ----

thread_local! {
    /// What `act` and `act_also` asked the import for ("bring <path>", "follow <path>", "look <path>"): the
    /// import itself is another card's.
    pub(super) static OPENED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

struct Host;

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn state_file(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("glint-suggestions-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("state.json")
}

/// A window in front, on a saved state of its own.
fn window(cx: &mut TestAppContext, name: &str) -> (VisualTestContext, std::path::PathBuf) {
    let file = state_file(name);
    cx.update(|cx| {
        testing::init(cx);
        crate::persistence::init_at(cx, file.clone());
    });
    let (mut visual, _) = testing::open_window(cx, |_, cx| cx.new(|_| Host));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    (visual, file)
}

fn done(file: std::path::PathBuf) {
    let _ = std::fs::remove_dir_all(file.parent().unwrap());
}

/// Lets what was started end: a notification taken down has left.
fn settle(visual: &mut VisualTestContext) {
    visual.run_until_parked();
    visual.executor().advance_clock(Duration::from_millis(600));
    visual.run_until_parked();
}

fn shown(visual: &mut VisualTestContext, kind: Kind) -> bool {
    settle(visual);
    visual.update(|window, cx| toast::is_shown(kind, window, cx))
}

fn press(visual: &mut VisualTestContext, id: &str) {
    visual.update(|window, cx| {
        window.render_frame(cx);
        window.click(SharedString::from(id.to_string()), cx);
    });
    settle(visual);
}

fn copy_queue() -> Suggestion {
    after(
        &mut Seen::default(),
        &Moment::SlowTransfer,
        Instant::now(),
        &Settings::default(),
        false,
    )
    .expect("the queue")
}

#[gpui_kit::test]
fn dismissed_the_xyplorer_suggestion_stays_away(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "dismissed");
    visual.update(|_, cx| {
        let found = xy(1284, false);
        assert_eq!(current(&found, cx).len(), 1);
        dismiss(cx, Kind::XyplorerMarks);
        assert!(dismissed(cx, Kind::XyplorerMarks));
        assert!(current(&found, cx).is_empty(), "put away for good");
        assert!(to_offer(&found, cx, 20_000).is_none());
    });
    done(file);
}

#[gpui_kit::test]
fn the_xyplorer_buttons_bring_it_in_look_first_or_follow(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "xyplorer");
    OPENED.with(|opened| opened.borrow_mut().clear());
    let marks = all(&xy(1284, false)).remove(0);
    let fresh = all(&xy(0, false)).remove(0);
    let brought = all(&xy(9, true)).remove(0);
    visual.update(|window, cx| {
        act(&marks, window, cx);
        act_also(&marks, window, cx);
        act(&fresh, window, cx);
        assert!(
            !crate::state::settings(cx).follow_xyplorer,
            "the import turns it on when it ends"
        );
        // Brought in before: the switch alone.
        act(&brought, window, cx);
        assert!(crate::state::settings(cx).follow_xyplorer);
    });
    visual.run_until_parked();
    OPENED.with(|opened| {
        assert_eq!(
            *opened.borrow(),
            [
                format!("bring {TAG_DAT}"),
                format!("look {TAG_DAT}"),
                format!("follow {TAG_DAT}")
            ]
        );
    });
    done(file);
}

#[gpui_kit::test]
fn the_fact_is_asked_through_what_a_test_replaces_and_kept_for_the_settings(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "asked");
    let there = XyFound {
        path: "D:\\tag.dat".to_string(),
        items: 3,
        brought_in: false,
    };
    let answer_with = there.clone();
    visual.update(|_, cx| {
        ask_xyplorer_with(cx, move || {
            let found = answer_with.clone();
            Box::pin(async move { Some(found) })
        });
    });
    let answer = Rc::new(RefCell::new(None));
    let sink = answer.clone();
    visual.update(|_, cx| {
        let asked = ask_xyplorer(cx);
        cx.spawn(async move |_| *sink.borrow_mut() = Some(asked.await)).detach();
    });
    visual.run_until_parked();
    let answer = answer.borrow().clone().flatten();
    assert_eq!(answer, Some(there));
    visual.update(|_, cx| {
        assert!(Facts::from_settings(cx).xyplorer.is_none(), "unknown until it is kept");
        know_xyplorer(cx, answer);
        assert_eq!(Facts::from_settings(cx).xyplorer.map(|found| found.items), Some(3));
    });
    done(file);
}

#[gpui_kit::test]
fn a_moment_comes_up_once_with_its_button_and_a_quiet_no(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "raise");
    assert!(visual.update(|window, cx| raise(copy_queue(), window, cx)));
    assert!(shown(&mut visual, Kind::CopyQueue));
    visual.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.try_find_by_label("Use the copy queue").is_some());
        assert!(window.try_find_by_label("Don't suggest this again").is_some());
        // Kept for the Overview too, while the setting is still off.
        assert_eq!(
            current(&Facts::default(), cx)
                .iter()
                .map(|s| s.kind)
                .collect::<Vec<_>>(),
            [Kind::CopyQueue]
        );
    });
    // Its button turns the setting on, says so with Undo, and it is settled: never offered again.
    press(&mut visual, "suggestion-copy-queue");
    visual.update(|window, cx| {
        assert!(crate::state::settings(cx).copy_queue);
        assert!(settled(cx, Kind::CopyQueue));
        assert!(current(&Facts::default(), cx).is_empty());
        window.render_frame(cx);
        assert!(window.try_find_by_label("Undo").is_some());
    });
    assert!(!shown(&mut visual, Kind::CopyQueue));
    press(&mut visual, "suggestion-undo");
    visual.update(|window, cx| {
        assert!(!crate::state::settings(cx).copy_queue, "Undo turns it off again");
        assert!(!raise(copy_queue(), window, cx), "and it isn't suggested a second time");
    });
    done(file);
}

#[gpui_kit::test]
fn dont_suggest_this_again_puts_it_away_for_good(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "never");
    assert!(visual.update(|window, cx| raise(copy_queue(), window, cx)));
    press(&mut visual, "suggestion-copy-queue-never");
    assert!(!shown(&mut visual, Kind::CopyQueue));
    visual.update(|window, cx| {
        assert!(dismissed(cx, Kind::CopyQueue));
        assert!(!crate::state::settings(cx).copy_queue, "nothing was changed");
        assert!(!raise(copy_queue(), window, cx));
        assert!(current(&Facts::default(), cx).is_empty());
    });
    done(file);
}

#[gpui_kit::test]
fn suggestions_keep_their_distance_and_obey_the_switch(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "pace");
    let ask_first = || {
        after(
            &mut Seen::default(),
            &Moment::UndidDeletion,
            Instant::now(),
            &Settings::default(),
            false,
        )
        .expect("ask first")
    };
    assert!(visual.update(|window, cx| raise(copy_queue(), window, cx)));
    // Another one right after waits in the Overview instead of coming up.
    assert!(!visual.update(|window, cx| raise(ask_first(), window, cx)));
    assert!(!shown(&mut visual, Kind::AskBeforeDelete));
    visual.update(|_, cx| {
        let listed: Vec<Kind> = current(&Facts::default(), cx).iter().map(|s| s.kind).collect();
        assert_eq!(listed, [Kind::CopyQueue, Kind::AskBeforeDelete]);
    });
    // The one that came up is not shown again for a week, whatever the pace.
    visual.update(|_, cx| cx.default_global::<Run>().last = None);
    assert!(
        !visual.update(|window, cx| raise(copy_queue(), window, cx)),
        "shown today"
    );
    assert!(
        visual.update(|window, cx| raise(ask_first(), window, cx)),
        "its turn has come"
    );
    done(file);
}

#[gpui_kit::test]
fn one_let_go_by_three_times_is_not_offered_a_fourth_unless_it_can_be_true_anew(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "let-go");
    const GB: u64 = 1 << 30;
    let facts = Facts {
        drives: Some(vec![Room {
            root: "C:\\".into(),
            free: 2 * GB,
            total: 256 * GB,
        }]),
        ..xy(40, false)
    };
    visual.update(|_, cx| {
        // 0.16 wrote the day alone: that is once.
        persistence::update_app(cx, |app| {
            app.notices.insert("suggest-xyplorer-marks".into(), "20000".into());
        });
        assert_eq!(super::shown(cx, Kind::XyplorerMarks), Some((20_000, 1)));
        note_shown(cx, Kind::XyplorerMarks, 20_010);
        dismiss(cx, Kind::FullDrive);
        assert_eq!(
            to_offer(&facts, cx, 20_020).map(|s| s.kind),
            Some(Kind::XyplorerMarks),
            "twice so far"
        );
        note_shown(cx, Kind::XyplorerMarks, 20_020);
        assert_eq!(super::shown(cx, Kind::XyplorerMarks), Some((20_020, 3)));
        assert!(
            to_offer(&facts, cx, 20_100).is_none(),
            "three times and never pressed: not again"
        );
        // The Overview still lists it.
        assert!(current(&facts, cx).iter().any(|s| s.kind == Kind::XyplorerMarks));
    });
    done(file);
    // A drive that is full again is said again, however often it was let go by.
    let (mut visual, file) = window(cx, "let-go-drive");
    visual.update(|_, cx| {
        for day in [20_000, 20_010, 20_020, 20_030] {
            note_shown(cx, Kind::FullDrive, day);
        }
        assert_eq!(to_offer(&facts, cx, 20_100).map(|s| s.kind), Some(Kind::FullDrive));
    });
    done(file);
}

#[gpui_kit::test]
fn with_the_switch_off_nothing_comes_up_and_the_overview_still_lists_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "tips-off");
    visual.update(|_, cx| crate::state::update(cx, |s| s.tips = false));
    assert!(!visual.update(|window, cx| raise(copy_queue(), window, cx)));
    assert!(!shown(&mut visual, Kind::CopyQueue));
    visual.update(|_, cx| assert_eq!(current(&Facts::default(), cx).len(), 1));
    done(file);
}

#[gpui_kit::test]
fn one_that_comes_while_glint_is_in_the_background_waits_for_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "held");
    visual.deactivate_window();
    visual.run_until_parked();
    assert!(!visual.update(|window, cx| raise(copy_queue(), window, cx)));
    assert!(!shown(&mut visual, Kind::CopyQueue));
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    visual.update(window_active);
    assert!(shown(&mut visual, Kind::CopyQueue), "in front again: it comes up");
    done(file);
}

#[gpui_kit::test]
fn a_folder_is_offered_to_the_gallery_once_and_its_button_adds_it_with_undo(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (mut visual, file) = window(cx, "gallery");
    let here = || Moment::PhotoFolder {
        folder: "D:\\Trips\\Lisbon".into(),
        files: 80,
    };
    visual.update(|window, cx| noticed(here(), window, cx));
    assert!(shown(&mut visual, Kind::GalleryFolder));
    press(&mut visual, "suggestion-gallery-folder");
    visual.update(|window, cx| {
        assert_eq!(crate::state::settings(cx).gallery_folders, ["D:\\Trips\\Lisbon"]);
        assert!(
            !settled(cx, Kind::GalleryFolder),
            "another folder may be worth offering one day"
        );
        window.render_frame(cx);
        assert!(window.try_find_by_label("Undo").is_some());
    });
    press(&mut visual, "suggestion-undo");
    visual.update(|window, cx| {
        assert!(crate::state::settings(cx).gallery_folders.is_empty());
        // Offered once: opening the folder again says nothing, whatever the pace.
        cx.default_global::<Run>().last = None;
        persistence::update_app(cx, |app| {
            app.notices.remove("suggest-gallery-folder");
        });
        noticed(here(), window, cx);
    });
    assert!(!shown(&mut visual, Kind::GalleryFolder));
    done(file);
}
