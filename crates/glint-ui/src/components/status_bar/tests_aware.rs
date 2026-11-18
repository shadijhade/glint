//! Tests of the dock's words and its end (card F2 of docs/gpui/aware): what the selection weighs and holds,
//! what the end notices about the folder, and the bar drawing both. A fake disk stands behind the explorer, a
//! fake probe behind Aware, and a fake index behind the bar.

use super::chips::{insight, Insight};
use super::index::{Index, Source, State};
use super::sentence::{counts_deep, deep, deep_with, Deep, MOST};
use super::summary::{counts, Summary};
use super::{StatusBar, StatusBarEvent};
use crate::aware::{self, Sources};
use crate::core_bridge::{self, CoreEvent};
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{self, Drive, DriveKind, Explorer, NavOptions};
use crate::kit::prelude::*;
use crate::testing;
use glint_core::aware::Answer;
use glint_model::aware::{Inside, Media, Part, Probed, Text};
use glint_model::entries::FileEntry;
use glint_model::format::format_size;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Entity, TestAppContext, VisualTestContext};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

// ---- Small builders -----------------------------------------------------------------------------------------

fn a_file(name: &str) -> FileEntry {
    file("C:\\x", name, 1000, 0)
}

fn a_folder(name: &str) -> FileEntry {
    folder("C:\\x", name)
}

fn media(width: u32, height: u32, seconds: f64) -> Probed {
    Probed {
        media: Some(Media {
            width,
            height,
            seconds,
        }),
        ..Probed::default()
    }
}

fn lines(lines: u64, binary: bool) -> Probed {
    Probed {
        text: Some(Text { lines, binary }),
        ..Probed::default()
    }
}

fn pages(pages: u32) -> Probed {
    Probed {
        pages: Some(pages),
        ..Probed::default()
    }
}

fn ins(files: u64, bytes: u64, done: bool) -> Inside {
    Inside {
        files,
        folders: 0,
        bytes,
        junk: 0,
        done,
        from_index: false,
    }
}

fn inside(files: u64, bytes: u64, done: bool) -> Probed {
    Probed {
        inside: Some(ins(files, bytes, done)),
        ..Probed::default()
    }
}

/// What `deep` makes of `selected`, with `answers` given to them in order (None: not answered).
fn deep_of(selected: &[FileEntry], answers: Vec<Option<Probed>>) -> Deep {
    deep(selected, &|entry| {
        let at = selected.iter().position(|e| e.path == entry.path)?;
        answers.get(at).cloned().flatten()
    })
}

fn selected_of(n: usize, items: usize, bytes: u64, files: usize) -> Summary {
    Summary {
        shown: items,
        selected: n,
        selected_files: files,
        selected_bytes: bytes,
        ..Summary::default()
    }
}

fn drive(path: &str, kind: DriveKind, total: u64, free: u64) -> Drive {
    Drive {
        path: path.into(),
        label: String::new(),
        kind,
        fs: "NTFS".into(),
        total,
        free,
    }
}

const GB: u64 = 1 << 30;

// ---- What the selection weighs and holds ------------------------------------------------------------------

#[test]
fn one_file_is_said_to_match_a_copied_checksum_after_what_else_is_known_of_it_and_never_to_differ() {
    use glint_model::aware::{copied_hash, Sums};
    let _guard = testing::lock();
    let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    let copied = copied_hash(sha).expect("a checksum");
    let made = |sha256: &str, blake3: &str, more: Probed| Probed {
        sums: Some(Sums { sha256: Some(sha256.into()), blake3: Some(blake3.into()), ..Sums::default() }),
        ..more
    };
    let one = [a_file("setup.exe")];
    let said = |answer: Option<Probed>, copied: Option<&glint_model::aware::CopiedHash>| {
        deep_with(&one, &|_| answer.clone(), copied).detail.map(|d| d.to_string())
    };
    let zero = "0".repeat(64);
    assert_eq!(said(Some(made(sha, &zero, Probed::default())), Some(&copied)).as_deref(), Some("Matches copied SHA-256"));
    assert_eq!(said(Some(made(&zero, &zero, Probed::default())), Some(&copied)), None, "a mismatch is not said in the dock");
    // After the lines it holds, and nothing before it is known.
    let with_lines = made(sha, &zero, lines(1230, false));
    assert_eq!(said(Some(with_lines), Some(&copied)).as_deref(), Some("1,230 lines · Matches copied SHA-256"));
    assert_eq!(said(Some(Probed::default()), Some(&copied)), None, "not compared yet");
    assert_eq!(said(None, Some(&copied)), None);
    assert_eq!(said(Some(made(sha, &zero, Probed::default())), None).as_deref(), None, "nothing copied");
    // Two files, or a folder, are not compared with it.
    let two = [a_file("a.exe"), a_file("b.exe")];
    let both = |_: &FileEntry| Some(made(sha, &zero, Probed::default()));
    assert_eq!(deep_with(&two, &both, Some(&copied)).detail, None);
    assert_eq!(deep_with(&[a_folder("d")], &both, Some(&copied)).detail, None);
}

#[test]
fn nothing_selected_and_too_many_selected_are_known_about_not_at_all() {
    let asked = Cell::new(0);
    let one = [a_file("a.txt")];
    assert_eq!(deep(&[], &|_| None), Deep::default());
    // Nothing is looked up for an empty selection, nor for more than `MOST`.
    let many: Vec<FileEntry> = (0..=MOST).map(|n| a_file(&format!("f{n}.txt"))).collect();
    assert_eq!(many.len(), 65);
    let found = deep(&many, &|_| {
        asked.set(asked.get() + 1);
        Some(lines(10, false))
    });
    assert_eq!(found, Deep::default());
    assert_eq!(asked.get(), 0, "65 files ask nothing");
    // Sixty-four are looked into.
    let sixty_four = &many[..MOST];
    let found = deep(sixty_four, &|_| Some(lines(10, false)));
    assert_eq!(found.detail.as_deref(), Some("640 lines"));
    assert_eq!(deep_of(&one, vec![None]), Deep::default());
}

#[test]
fn a_folder_with_a_finished_inside_adds_its_bytes_and_files_to_the_files_beside_it() {
    let selected = [a_folder("src"), a_file("a.zip")];
    let found = deep_of(&selected, vec![Some(inside(1204, 3 * GB, true)), None]);
    assert_eq!(found.bytes, Some(3 * GB + 1000));
    assert_eq!(found.inside, Some(1204));
    assert!(found.done);
    assert_eq!(found.detail, None);
}

#[test]
fn a_folder_whose_inside_is_still_growing_is_not_done() {
    let selected = [a_folder("src")];
    let found = deep_of(&selected, vec![Some(inside(40, 5000, false))]);
    assert_eq!((found.bytes, found.inside, found.done), (Some(5000), Some(40), false));
}

#[test]
fn with_two_folders_and_one_unanswered_nothing_about_them_is_said() {
    let selected = [a_folder("one"), a_folder("two")];
    let found = deep_of(&selected, vec![Some(inside(10, 100, true)), None]);
    assert_eq!((found.bytes, found.inside), (None, None));
    assert!(!found.done);
    assert_eq!(found, Deep::default());
}

#[test]
fn a_picture_says_its_size_and_a_video_its_size_and_length() {
    let one = [a_file("a.png")];
    assert_eq!(
        deep_of(&one, vec![Some(media(1920, 1080, 0.))]).detail.as_deref(),
        Some("1920 × 1080")
    );
    assert_eq!(
        deep_of(&one, vec![Some(media(1920, 1080, 151.))]).detail.as_deref(),
        Some("1920 × 1080 · 2 min 31 s")
    );
}

#[test]
fn songs_add_up_their_lengths_and_text_files_their_lines() {
    let songs = [a_file("a.mp3"), a_file("b.mp3")];
    let total = deep_of(&songs, vec![Some(media(0, 0, 180.)), Some(media(0, 0, 91.))]);
    assert_eq!(total.detail.as_deref(), Some("4 min 31 s"));
    let hour = deep_of(&songs, vec![Some(media(0, 0, 3000.)), Some(media(0, 0, 700.))]);
    assert_eq!(hour.detail.as_deref(), Some("1 h 1 min"));
    let one_missing = deep_of(&songs, vec![Some(media(0, 0, 180.)), None]);
    assert_eq!(one_missing.detail, None);

    let texts = [a_file("a.txt"), a_file("b.txt")];
    let sum = deep_of(&texts, vec![Some(lines(1000, false)), Some(lines(230, false))]);
    assert_eq!(sum.detail.as_deref(), Some("1,230 lines"));
    let binary = deep_of(&texts, vec![Some(lines(1000, false)), Some(lines(230, true))]);
    assert_eq!(binary.detail, None, "a binary file has no lines");
    let unanswered = deep_of(&texts, vec![Some(lines(1000, false)), None]);
    assert_eq!(unanswered.detail, None);
    let single = deep_of(&texts[..1], vec![Some(lines(1, false))]);
    assert_eq!(single.detail.as_deref(), Some("1 line"));
}

#[test]
fn pdfs_add_up_their_pages_and_one_page_is_singular() {
    let pdfs = [a_file("a.pdf"), a_file("b.pdf"), a_file("c.pdf")];
    let found = deep_of(&pdfs, vec![Some(pages(10)), Some(pages(20)), Some(pages(18))]);
    assert_eq!(found.detail.as_deref(), Some("48 pages"));
    assert_eq!(deep_of(&pdfs[..1], vec![Some(pages(1))]).detail.as_deref(), Some("1 page"));
    let one_missing = deep_of(&pdfs, vec![Some(pages(10)), None, Some(pages(18))]);
    assert_eq!(one_missing.detail, None);
}

#[test]
fn counts_deep_with_nothing_known_is_exactly_the_counts() {
    let none = Deep::default();
    let items_only = Summary {
        shown: 25,
        ..Summary::default()
    };
    let hidden = Summary {
        shown: 25,
        hidden: 3,
        ..Summary::default()
    };
    let with_bytes = selected_of(2, 25, 48 * 1024, 2);
    for s in [&items_only, &hidden, &with_bytes, &Summary::default()] {
        assert_eq!(counts_deep(s, &none), counts(s));
    }
    assert_eq!(counts_deep(&with_bytes, &none), "25 items · 2 selected · 48 KB");
}

#[test]
fn counts_deep_says_the_folders_weight_and_the_files_inside_or_the_files_detail() {
    let folder_known = Deep {
        bytes: Some(3_435_973_837),
        inside: Some(1204),
        detail: None,
        done: true,
    };
    assert_eq!(
        counts_deep(&selected_of(1, 25, 0, 0), &folder_known),
        "25 items · 1 selected · 3.2 GB · 1,204 files inside"
    );
    let lines_known = Deep {
        detail: Some("1,230 lines".into()),
        ..Deep::default()
    };
    assert_eq!(
        counts_deep(&selected_of(2, 25, 48 * 1024, 2), &lines_known),
        "25 items · 2 selected · 48 KB · 1,230 lines"
    );
    let one_file = Deep {
        inside: Some(1),
        bytes: Some(10),
        ..Deep::default()
    };
    assert_eq!(
        counts_deep(&selected_of(1, 5, 0, 0), &one_file),
        "5 items · 1 selected · 10 B · 1 file inside"
    );
}

// ---- What the end notices ---------------------------------------------------------------------------------

#[test]
fn the_end_notices_a_changing_folder_before_a_nearly_full_drive_and_otherwise_nothing() {
    let full = drive("C:\\", DriveKind::Fixed, 200 * GB, 10 * GB);
    let pulse = [1u8; 30];
    assert_eq!(
        insight("C:\\a", true, Some(pulse), std::slice::from_ref(&full)),
        Some(Insight::Changing(pulse))
    );
    assert_eq!(
        insight("c:\\A\\b", false, Some(pulse), std::slice::from_ref(&full)),
        Some(Insight::NearlyFull("C:".into()))
    );
    // Changing, but with no pulse to draw: not changing, so the drive is what is said.
    assert_eq!(
        insight("C:\\a", true, None, std::slice::from_ref(&full)),
        Some(Insight::NearlyFull("C:".into()))
    );
    assert_eq!(insight("", true, Some(pulse), std::slice::from_ref(&full)), None);
}

#[test]
fn a_drive_is_nearly_full_only_when_both_the_share_and_the_room_are_small() {
    let big_room = drive("C:\\", DriveKind::Fixed, 2000 * GB, 100 * GB);
    assert_eq!(insight("C:\\a", false, None, &[big_room]), None, "5% but 100 GB free");
    let roomy = drive("C:\\", DriveKind::Fixed, 100 * GB, 20 * GB);
    assert_eq!(insight("C:\\a", false, None, &[roomy]), None, "20% free");
    let network = drive("Z:\\", DriveKind::Network, 100 * GB, GB);
    assert_eq!(insight("Z:\\a", false, None, &[network]), None, "never a network drive");
    let other = drive("D:\\", DriveKind::Fixed, 100 * GB, GB);
    assert_eq!(insight("C:\\a", false, None, &[other]), None, "another drive's space says nothing");
    let unknown_size = drive("C:\\", DriveKind::Fixed, 0, 0);
    assert_eq!(insight("C:\\a", false, None, &[unknown_size]), None);
}

// ---- The bar, in a window -----------------------------------------------------------------------------------

/// Every probe Aware starts, and how to answer it.
type Senders = Rc<RefCell<Vec<async_channel::Sender<Answer>>>>;

struct Win {
    visual: VisualTestContext,
    bar: Entity<StatusBar>,
    explorer: Entity<Explorer>,
    senders: Senders,
    events: Rc<RefCell<Vec<StatusBarEvent>>>,
}

struct Frame {
    bar: Entity<StatusBar>,
}

impl Render for Frame {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        col().size_full().justify_end().child(self.bar.clone())
    }
}

fn a_quiet_index() -> Source {
    let answer = Index {
        state: State::Off,
        version: None,
        volumes: vec![],
        error: None,
    };
    Source {
        status: Arc::new(move || {
            let now = answer.clone();
            Box::pin(async move { Ok(now) })
        }),
        enable: Arc::new(|| Box::pin(async { Err("not in a test".to_string()) })),
    }
}

/// `C:\a` (a folder, three files), `C:\big` (a hundred files); a bar over them with Aware behind it, whose probe
/// answers only what the test sends.
fn window(cx: &mut TestAppContext) -> Win {
    cx.update(testing::init);
    let disk = FakeDisk::new();
    disk.folder("C:\\", vec![folder("C:\\", "a"), folder("C:\\", "big")]);
    disk.folder(
        "C:\\a",
        vec![
            folder("C:\\a", "sub"),
            file("C:\\a", "one.txt", 100, 10),
            file("C:\\a", "two.txt", 300, 30),
        ],
    );
    disk.folder("C:\\a\\sub", vec![file("C:\\a\\sub", "deep.txt", 1, 1)]);
    disk.folder(
        "C:\\big",
        (0..100).map(|n| file("C:\\big", &format!("f{n:03}.txt"), 10, 5)).collect(),
    );
    disk.set_drives(vec![drive("C:\\", DriveKind::Fixed, 500 * GB, 200 * GB)]);
    let senders: Senders = Rc::default();
    let recorded = senders.clone();
    let sources = Sources {
        probe: Rc::new(move |_, _, _| {
            let (sender, receiver) = async_channel::unbounded();
            recorded.borrow_mut().push(sender);
            receiver
        }),
        ..Sources::still()
    };
    cx.update(|cx| {
        explorer::init_with(cx, disk.source());
        super::init_with(cx, a_quiet_index());
    });
    let explorer = cx.update(|cx| explorer::explorer(cx));
    // The drives are read before Aware starts: it asks only about files on fixed drives.
    let task = explorer.update(cx, |e, cx| e.refresh_drives(cx));
    cx.foreground_executor().block_test(task);
    cx.update(|cx| aware::init_with(cx, sources));
    let (mut visual, frame) = testing::open_window(cx, |window, cx| {
        let bar = cx.new(|cx| StatusBar::new(window, cx));
        cx.new(|_| Frame { bar })
    });
    let bar = visual.update(|_, cx| frame.read(cx).bar.clone());
    let events: Rc<RefCell<Vec<StatusBarEvent>>> = Rc::default();
    let heard = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&bar, move |_, event: &StatusBarEvent, _| heard.borrow_mut().push(event.clone()))
            .detach();
    });
    Win {
        visual,
        bar,
        explorer,
        senders,
        events,
    }
}

fn go(win: &mut Win, path: &str) {
    let explorer = win.explorer.clone();
    let path = path.to_string();
    win.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| e.navigate(path.clone(), NavOptions::default(), cx));
    });
    win.visual.run_until_parked();
}

fn with_explorer(win: &mut Win, f: impl FnOnce(&mut Explorer, &mut gpui_kit::Context<Explorer>)) {
    let explorer = win.explorer.clone();
    win.visual.update(|_, cx| explorer.update(cx, f));
    win.visual.run_until_parked();
}

/// Lets time pass: the selection settles, and what Aware was told is applied.
fn wait(win: &mut Win, ms: u64) {
    win.visual.executor().advance_clock(Duration::from_millis(ms));
    win.visual.run_until_parked();
}

fn has(win: &mut Win, label: &str) -> bool {
    win.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find_by_label(label).is_some()
    })
}

fn answer(win: &mut Win, call: usize, path: &str, part: Part) {
    let sender = win.senders.borrow()[call].clone();
    assert!(sender
        .try_send(Answer {
            path: path.into(),
            part,
        })
        .is_ok());
    wait(win, 50);
}

#[gpui_kit::test]
fn a_selected_folders_inside_is_said_in_the_dock_once_it_is_known_and_again_when_it_is_done(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut win = window(cx);
    go(&mut win, "C:\\a");
    with_explorer(&mut win, |e, cx| e.select_only(Some("C:\\a\\sub"), cx));
    assert!(has(&mut win, "3 items · 1 selected"), "nothing is said that isn't known");
    // The selection settles, and Aware asks about the folder.
    wait(&mut win, 200);
    assert_eq!(win.senders.borrow().len(), 1, "one look at the one selected folder");
    answer(&mut win, 0, "C:\\a\\sub", Part::Inside(ins(40, 5000, false)));
    let growing = format!("3 items · 1 selected · {} · 40 files inside", format_size(5000));
    assert!(has(&mut win, &growing), "{growing}");
    answer(&mut win, 0, "C:\\a\\sub", Part::Inside(ins(1204, 3 * GB, true)));
    let done = format!("3 items · 1 selected · {} · 1,204 files inside", format_size(3 * GB));
    assert!(has(&mut win, &done), "{done}");
}

#[gpui_kit::test]
fn a_selection_of_a_hundred_files_is_not_looked_into_by_the_bar(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut win = window(cx);
    go(&mut win, "C:\\big");
    let rows = win
        .explorer
        .read_with(&win.visual, |e, _| e.active_tab().map(|tab| tab.rows.clone()))
        .unwrap_or_default();
    with_explorer(&mut win, |e, cx| e.select_all(&rows, cx));
    assert!(has(&mut win, "100 items · 100 selected · 1000 B"));
    assert!(win.bar.read_with(&win.visual, |bar, _| bar.selected.is_empty()), "the bar keeps none to look at");
    with_explorer(&mut win, |e, cx| e.select_only(Some("C:\\big\\f001.txt"), cx));
    assert_eq!(win.bar.read_with(&win.visual, |bar, _| bar.selected.len()), 1);
}

#[gpui_kit::test]
fn the_chip_says_changing_now_when_the_folder_is_written_to_and_pressing_it_tells_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut win = window(cx);
    go(&mut win, "C:\\a");
    assert!(!has(&mut win, "Changing now"), "nothing is changing");
    for _ in 0..3 {
        win.visual.update(|_, cx| core_bridge::emit(cx, CoreEvent::DirChanged("C:\\a".into())));
    }
    win.visual.run_until_parked();
    assert!(has(&mut win, "Changing now"));
    assert_eq!(
        win.visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label("Changing now").and_then(|element| element.role())
        }),
        Some(Role::Button)
    );
    win.visual.update(|window, cx| {
        window.render_frame(cx);
        window.click("status-insight", cx);
    });
    win.visual.run_until_parked();
    let events = win.events.borrow().clone();
    assert!(
        matches!(events.as_slice(), [StatusBarEvent::Insight(Insight::Changing(_))]),
        "{events:?}"
    );
}

#[gpui_kit::test]
fn the_chip_and_the_words_still_draw_mirrored(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut win = window(cx);
    go(&mut win, "C:\\a");
    testing::set_rtl(true);
    for _ in 0..3 {
        win.visual.update(|_, cx| core_bridge::emit(cx, CoreEvent::DirChanged("C:\\a".into())));
    }
    win.visual.run_until_parked();
    with_explorer(&mut win, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    let drawn = has(&mut win, "Changing now") && has(&mut win, "3 items · 1 selected · 100 B");
    testing::set_rtl(false);
    assert!(drawn);
}

#[gpui_kit::test]
fn without_aware_the_bar_says_what_it_says_today_and_has_no_chip(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    cx.update(testing::init);
    let disk = FakeDisk::new();
    disk.folder("C:\\", vec![file("C:\\", "a.txt", 7, 1)]);
    cx.update(|cx| {
        explorer::init_with(cx, disk.source());
        super::init_with(cx, a_quiet_index());
    });
    let (mut visual, frame) = testing::open_window(cx, |window, cx| {
        let bar = cx.new(|cx| StatusBar::new(window, cx));
        cx.new(|_| Frame { bar })
    });
    let _ = &frame;
    let explorer = cx.update(|cx| explorer::explorer(cx));
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.navigate("C:\\", NavOptions::default(), cx)));
    visual.run_until_parked();
    visual.update(|_, cx| explorer.update(cx, |e, cx| e.select_only(Some("C:\\a.txt"), cx)));
    visual.run_until_parked();
    let found = visual.update(|window, cx| {
        window.render_frame(cx);
        (
            window.try_find_by_label("1 items · 1 selected · 7 B").is_some()
                || window.try_find_by_label("1 item · 1 selected · 7 B").is_some(),
            window.try_find_by_label("Changing now").is_some(),
        )
    });
    assert_eq!(found, (true, false));
}

/// Puts the bar in the dock with `side` pixels at each end beside the island, as the window tells it.
fn beside(win: &mut Win, side: f32) {
    let bar = win.bar.clone();
    win.visual.update(|_, cx| {
        bar.read(cx).beside().set(side);
        bar.update(cx, |_, cx| cx.notify());
    });
    win.visual.run_until_parked();
}

fn bounds(win: &mut Win, id: &'static str) -> Option<Bounds<Pixels>> {
    win.visual.update(|window, cx| {
        window.render_frame(cx);
        window.try_find(id).map(|element| element.bounds())
    })
}

#[gpui_kit::test]
fn beside_the_island_the_words_fold_to_two_lines_then_to_none_and_the_chip_to_its_picture(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut win = window(cx);
    go(&mut win, "C:\\a");
    with_explorer(&mut win, |e, cx| e.select_only(Some("C:\\a\\one.txt"), cx));
    for _ in 0..3 {
        win.visual.update(|_, cx| core_bridge::emit(cx, CoreEvent::DirChanged("C:\\a".into())));
    }
    win.visual.run_until_parked();

    // Wide ends: the whole sentence on one line, and the chip's words.
    beside(&mut win, 400.);
    assert!(has(&mut win, "3 items · 1 selected · 100 B"));
    let one_line = bounds(&mut win, "status-counts").expect("the words").size.height;
    let whole = bounds(&mut win, "status-insight").expect("the chip").size.width;

    // Tight ends: the words start at the selection, on two lines no wider than the end; the chip says less.
    beside(&mut win, 100.);
    assert!(has(&mut win, "1 selected · 100 B"), "the words start at the selection");
    let counts = bounds(&mut win, "status-counts").expect("the words");
    assert!(counts.size.height > one_line, "two lines: {counts:?} over {one_line:?}");
    assert!(counts.size.width <= px(100.), "{counts:?}");
    let short = bounds(&mut win, "status-insight").expect("the chip").size.width;
    assert!(short < whole, "fewer words: {short:?} < {whole:?}");
    assert!(has(&mut win, "Changing now"), "a screen reader is told the same");

    // Less still: the chip is its picture alone.
    beside(&mut win, 60.);
    let picture = bounds(&mut win, "status-insight").expect("the chip").size.width;
    assert!(picture < short, "{picture:?} < {short:?}");
    assert!(has(&mut win, "Changing now"));

    // No room for a word: none is drawn, and the end is gone.
    beside(&mut win, 40.);
    let counts = bounds(&mut win, "status-counts");
    assert!(counts.is_none_or(|counts| counts.size.width == px(0.)), "{counts:?}");
    assert!(bounds(&mut win, "status-insight").is_none(), "no chip without room");
}
