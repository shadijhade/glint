//! The list's tags and media facts: the dots after a name, the Taken, Dimensions, Duration, Artist and Album
//! columns, how little of a big folder is read, and Escape ending a search.

use super::*;
use crate::components::row_facts::{self, fake};
use crate::dialogs::tags;
use crate::explorer::Wider;
use crate::i18n;
use glint_model::entries::MediaFacts;
use glint_model::format::format_date;
use std::sync::Arc;

/// Frames until `done`, giving core's threads time between them.
fn until(ui: &mut Ui, what: &str, mut done: impl FnMut(&mut Ui) -> bool) {
    for _ in 0..400 {
        ui.frame();
        if done(ui) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the list never reached: {what}");
}

/// [`open`], with the tags and media reads going to fakes.
fn open_faked(
    cx: &mut TestAppContext,
    disk: &FakeDisk,
    path: &str,
    tags: &Arc<fake::Tags>,
    media: &Arc<fake::Media>,
) -> Ui {
    let (backend, source): (Arc<dyn tags::Backend>, Arc<dyn row_facts::MediaSource>) = (tags.clone(), media.clone());
    cx.update(|cx| {
        tags::init_with(cx, backend);
        row_facts::init_with(cx, source);
    });
    open(cx, disk, path)
}

/// The names of the dots drawn after rows' names ("Tags: Red, Work"), sorted.
fn dots(ui: &mut Ui) -> Vec<String> {
    ui.visual.update(|window, cx| {
        window.render_frame(cx);
        let mut found: Vec<String> = window
            .find_all_by_role(Role::Group)
            .iter()
            .filter_map(|group| group.label().map(str::to_string))
            .filter(|label| label.starts_with("Tags: "))
            .collect();
        found.sort();
        found
    })
}

#[gpui_kit::test]
fn dots_show_for_tagged_rows_and_change_after_tags_are_set(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let tags = fake::Tags::with(&[("C:\\x\\alpha.txt", &["Red", "Work"]), ("C:\\x\\docs", &["Blue"])]);
    let media = fake::Media::with(vec![]);
    let mut ui = open_faked(cx, &small_disk(), "C:\\x", &tags, &media);
    // Unread rows show nothing; once read, the tagged ones have their dots, named for a screen reader.
    until(&mut ui, "the dots", |ui| dots(ui).len() == 2);
    assert_eq!(dots(&mut ui), ["Tags: Blue", "Tags: Red, Work"]);
    assert_eq!(ui.role_of("Tags: Red, Work"), Some(Role::Group));
    assert_eq!(tags.calls().len(), 1, "the four rows were read together");
    assert_eq!(tags.paths_read(), 4);

    // Set tags on alpha.txt: Work comes off, Green goes on; the dots follow without the folder being read again.
    ui.visual.update(|window, cx| {
        tags::set_tags(
            window,
            cx,
            vec!["C:\\x\\alpha.txt".to_string()],
            vec!["Green".to_string()],
            vec!["Work".to_string()],
        )
    });
    until(&mut ui, "the new dots", |ui| {
        dots(ui).contains(&"Tags: Red, Green".to_string())
    });
    assert_eq!(dots(&mut ui), ["Tags: Blue", "Tags: Red, Green"]);
    assert_eq!(tags.paths_read(), 5, "only the tagged path was read again");

    // Taking the last tag off a row takes its dots away.
    ui.visual.update(|window, cx| {
        tags::set_tags(
            window,
            cx,
            vec!["C:\\x\\docs".to_string()],
            vec![],
            vec!["blue".to_string()],
        )
    });
    until(&mut ui, "no dots on the folder", |ui| dots(ui) == ["Tags: Red, Green"]);
}

#[gpui_kit::test]
fn a_tag_in_arabic_has_its_dot_and_its_name(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let disk = FakeDisk::new();
    disk.folder("C:\\ملفات", vec![file("C:\\ملفات", "تقرير.txt", 5, 0)]);
    let tags = fake::Tags::with(&[("C:\\ملفات\\تقرير.txt", &["مهم"])]);
    let media = fake::Media::with(vec![]);
    let mut ui = open_faked(cx, &disk, "C:\\ملفات", &tags, &media);
    until(&mut ui, "the dot", |ui| dots(ui).len() == 1);
    assert_eq!(dots(&mut ui), ["Tags: مهم"]);
}

#[gpui_kit::test]
fn a_big_folder_reads_only_the_rows_shown_and_a_little_ahead(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let tags = fake::Tags::with(&[]);
    let media = fake::Media::with(vec![]);
    let mut ui = open_faked(cx, &big_disk(10_000), "C:\\big", &tags, &media);
    until(&mut ui, "the first rows read", |_| tags.paths_read() > 0);
    ui.settle();
    let first = tags.paths_read();
    assert!((15..=100).contains(&first), "{first} paths read for a screen of rows");
    let paths: Vec<String> = tags.calls().concat();
    assert!(paths.contains(&"C:\\big\\file000000.txt".to_string()));
    assert!(
        !paths.contains(&"C:\\big\\file005000.txt".to_string()),
        "the middle is not read"
    );
    assert!(tags.calls().iter().all(|call| call.len() <= 200));

    // Frames that draw the same rows ask nothing more.
    for _ in 0..4 {
        ui.frame();
    }
    std::thread::sleep(std::time::Duration::from_millis(30));
    ui.frame();
    assert_eq!(tags.paths_read(), first);

    // The end of the folder: the rows there are read, and the rest of the folder still is not.
    ui.press("end");
    until(&mut ui, "the last rows read", |_| {
        tags.calls().concat().contains(&"C:\\big\\file009999.txt".to_string())
    });
    ui.settle();
    let total = tags.paths_read();
    assert!(total <= first + 100, "{total} paths read after going to the end");
    assert_eq!(media.paths_read(), 0, "no media column is shown: no media is read");
}

/// `C:\m` (mostly text files, so its columns are the usual ones) with a photo that says when it was taken, a video, a song, a photo that says nothing and a text file.
fn media_disk() -> (FakeDisk, Arc<fake::Media>) {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\m",
        vec![
            file("C:\\m", "clip.mp4", 900, 1_650_000_000_000),
            file("C:\\m", "notes.txt", 10, 1_650_000_000_000),
            file("C:\\m", "notes2.txt", 10, 1_650_000_000_000),
            file("C:\\m", "notes3.txt", 10, 1_650_000_000_000),
            file("C:\\m", "notes4.txt", 10, 1_650_000_000_000),
            file("C:\\m", "photo.jpg", 500, 1_660_000_000_000),
            file("C:\\m", "plain.jpg", 500, 1_500_000_000_000),
            file("C:\\m", "song.mp3", 300, 1_640_000_000_000),
        ],
    );
    let media = fake::Media::with(vec![
        (
            "C:\\m\\clip.mp4",
            MediaFacts {
                width: Some(1920),
                height: Some(1080),
                duration: Some(3723.),
                ..MediaFacts::default()
            },
        ),
        (
            "C:\\m\\photo.jpg",
            MediaFacts {
                taken: Some(1_600_000_000_000),
                width: Some(4000),
                height: Some(3000),
                ..MediaFacts::default()
            },
        ),
        (
            "C:\\m\\song.mp3",
            MediaFacts {
                duration: Some(185.),
                artist: Some("Nour".into()),
                album: Some("Dawn".into()),
                ..MediaFacts::default()
            },
        ),
    ]);
    (disk, media)
}

fn set_columns(ui: &mut Ui, columns: Vec<ColumnId>) {
    let explorer = ui.explorer.clone();
    ui.visual
        .update(|_, cx| explorer.update(cx, |e, cx| e.set_columns(columns, cx)));
    ui.frame();
}

const MEDIA_COLUMNS: [ColumnId; 7] = [
    ColumnId::Modified,
    ColumnId::Taken,
    ColumnId::Dimensions,
    ColumnId::Duration,
    ColumnId::Artist,
    ColumnId::Album,
    ColumnId::Size,
];

#[gpui_kit::test]
fn the_media_columns_read_the_files_only_while_they_are_shown_and_say_what_the_files_say(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (disk, media) = media_disk();
    let tags = fake::Tags::with(&[]);
    let mut ui = open_faked(cx, &disk, "C:\\m", &tags, &media);
    // The usual columns: tags are read for the eight rows, no file is asked what it says of itself.
    until(&mut ui, "the tags read", |_| tags.paths_read() == 8);
    ui.settle();
    assert_eq!(media.paths_read(), 0);
    assert!(!ui.said(1).unwrap_or_default().contains("1920"));

    // Show the columns: the three media files and the unreadable photo are read (not the text file), together.
    set_columns(&mut ui, MEDIA_COLUMNS.to_vec());
    until(&mut ui, "the media read", |_| media.paths_read() == 4);
    ui.settle();
    assert_eq!(media.calls().len(), 1);
    assert!(!media.calls()[0].contains(&"C:\\m\\notes.txt".to_string()));
    let language = i18n::language();
    let said = |ui: &mut Ui, i: usize| ui.said(i).unwrap_or_default();
    // The video: its size on screen and its length as a clock.
    let clip = said(&mut ui, 0);
    assert!(clip.contains("1920 × 1080") && clip.contains("1:02:03"), "{clip}");
    // The text file says nothing of this kind.
    assert_eq!(
        said(&mut ui, 1),
        format!("TXT file, 10 B, {}", format_date(1_650_000_000_000, None, language))
    );
    // The photo that says when it was taken shows that date, not the modified one, and its dimensions.
    let photo = said(&mut ui, 5);
    assert!(
        photo.contains(&format_date(1_600_000_000_000, None, language)),
        "{photo}"
    );
    assert!(photo.contains("4000 × 3000"), "{photo}");
    // The photo that says nothing shows its modified date again, as the Taken column, and nothing else.
    let plain = said(&mut ui, 6);
    let modified = format_date(1_500_000_000_000, None, language);
    assert_eq!(
        plain.matches(&modified).count(),
        2,
        "Date modified and the stand-in: {plain}"
    );
    assert!(!plain.contains(" × "), "{plain}");
    // The song: its length, artist and album.
    let song = said(&mut ui, 7);
    assert!(
        song.contains("3:05") && song.contains("Nour") && song.contains("Dawn"),
        "{song}"
    );

    // Taking the columns away stops the reading and empties the cells; showing them again needs no new reads.
    set_columns(&mut ui, vec![ColumnId::Modified, ColumnId::Size]);
    assert!(!said(&mut ui, 0).contains("1920"));
    set_columns(&mut ui, MEDIA_COLUMNS.to_vec());
    ui.settle();
    assert!(said(&mut ui, 0).contains("1920 × 1080"));
    assert_eq!(media.paths_read(), 4, "what was read is kept");
}

#[gpui_kit::test]
fn the_media_cells_have_their_own_tooltips_and_leave_a_narrow_list(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let (disk, media) = media_disk();
    let tags = fake::Tags::with(&[]);
    let mut ui = open_faked(cx, &disk, "C:\\m", &tags, &media);
    set_columns(&mut ui, MEDIA_COLUMNS.to_vec());
    until(&mut ui, "the media read", |_| media.paths_read() == 4);
    ui.settle();
    ui.visual.update(|window, _| {
        assert!(window.try_find(("file-list-taken", 5usize)).is_some());
        assert!(window.try_find(("file-list-artist", 7usize)).is_some());
        assert!(window.try_find(("file-list-album", 7usize)).is_some());
    });
    // At 400 px the list shows no media column, so what they say is not in the rows either.
    ui.visual.simulate_resize(size(px(400.), px(700.)));
    ui.frame();
    ui.frame();
    ui.visual.update(|window, _| {
        assert!(window.try_find(("file-list-taken", 5usize)).is_none());
        assert!(window.try_find(("file-list-artist", 7usize)).is_none());
    });
    assert!(!ui.said(0).unwrap_or_default().contains("1920"));
}

#[gpui_kit::test]
fn escape_in_a_search_ends_the_search_and_its_results(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &small_disk(), "C:\\x");
    let task = ui
        .explorer
        .update(&mut ui.visual, |e, cx| e.run_find("be", Wider::default(), cx));
    cx.foreground_executor().block_test(task);
    ui.frame();
    let searching = |ui: &mut Ui| {
        ui.explorer
            .read_with(&ui.visual, |e, _| e.active_tab().is_some_and(|t| t.find.is_some()))
    };
    assert!(searching(&mut ui), "the search ran");
    assert_eq!(ui.role_of("Search results"), Some(Role::ListBox));
    ui.press("escape");
    ui.settle();
    assert!(!searching(&mut ui), "Escape ended the search");
    assert_eq!(ui.role_of("Files"), Some(Role::ListBox), "the folder is shown again");
    assert_eq!(ui.labels(), ["docs", "alpha.txt", "berry.png", "beta.md"]);
    // A plain filter is cleared by Escape as before.
    ui.explorer.update(&mut ui.visual, |e, cx| e.set_filter("al", None, cx));
    ui.frame();
    assert_eq!(ui.labels(), ["alpha.txt"]);
    ui.press("escape");
    assert_eq!(ui.labels().len(), 4);
}
