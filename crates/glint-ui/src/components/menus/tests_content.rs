//! Tests of what the right-click menus hold and in which order (0.16: the most important first, a short section
//! for each kind of file, the rest under More, nothing that can't apply).

use super::smart::{keys, top_rows};
use super::tests::{an_archive, ctx, english, find, labels, submenu, text, with_attrs};
use super::*;
use crate::explorer::fake::folder;
use glint_model::aware::{Origin, Probed};
use glint_model::entries::attr;

/// The rows a menu has at its top level (not the strip's buttons, headings or lines).
fn top(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Node::Item(e) => Some(e.label.to_string()),
            Node::Menu { label, .. } => Some(format!("{label}>")),
            _ => None,
        })
        .collect()
}

#[test]
fn a_single_items_top_level_has_at_most_twelve_rows() {
    let _guard = english();
    let kinds = [
        text("a.jpg"),
        text("a.mp4"),
        text("a.mp3"),
        text("a.pdf"),
        text("setup.exe"),
        text("a.txt"),
        text("a.docx"),
        text("a.csv"),
        text("pack.zip"),
        text("os.iso"),
        folder("C:\\x", "docs"),
    ];
    for entry in kinds {
        let menu = item_menu(std::slice::from_ref(&entry), &ctx());
        let rows = top(&menu);
        assert_eq!(top_rows(&menu), rows.len());
        assert!(rows.len() <= 12, "{}: {} rows {rows:?}", entry.name, rows.len());
    }
}

#[test]
fn suggested_never_repeats_a_row_the_menu_leads_with_and_holds_two_at_most() {
    let _guard = english();
    let menu = item_menu(&[text("pack.zip")], &ctx());
    let pick = |key: &str| glint_model::aware::Pick {
        key: key.into(),
        why: glint_model::aware::Why::Plain,
        score: 1.0,
    };
    // Extract here and Open with its app are already at the top: the next best are taken instead.
    let picks = [
        pick("do:extract-here"),
        pick("do:open-with-its-app"),
        pick("do:pin-portal"),
        pick("do:compress:zip"),
        pick("do:defender"),
    ];
    let head = Head {
        title: "pack.zip".into(),
        picture: HeadPicture::Several,
        line: "".into(),
        badges: Vec::new(),
        changed: None,
    };
    let dressed = smart::dress(menu, head, &picks, glint_model::aware::Shape::Archive, Vec::new());
    let all = labels(&dressed);
    let at = all
        .iter()
        .position(|l| l == "# Suggested")
        .expect("a Suggested section");
    assert_eq!(&all[at + 1..at + 3], ["Mount as live folder", "Compress to › Zip file"]);
    assert_eq!(all.iter().filter(|l| *l == "Extract here").count(), 1, "{all:?}");
    // It sits under the rows the menu leads with, above the kind's section.
    let open = all.iter().position(|l| l == "Open").unwrap();
    assert!(open < at && at < all.iter().position(|l| l == "# Archive").unwrap());
}

#[test]
fn a_picture_leads_with_open_then_its_own_section_then_organize_then_more() {
    let _guard = english();
    let menu = item_menu(&[text("a.jpg")], &ctx());
    assert_eq!(
        labels(&menu),
        [
            "[Cut|Copy|Rename|Copy path|Share…|Delete]",
            "Open",
            "Open with…",
            "Quick Look",
            "# Picture",
            "Rotate right",
            "Rotate left",
            "Set as desktop background",
            "More picture tools>",
            "-",
            "Move or copy to>",
            "Compress to>",
            "More>",
            "-",
            "Properties",
            "Show more options",
        ]
    );
    let more = labels(submenu(&menu, "More picture tools"));
    for want in [
        "Convert picture…",
        "Copy the text in it",
        "Make smaller…",
        "Find similar colors",
        "Remove personal info…",
        "Change when it was taken…",
    ] {
        assert!(more.contains(&want.to_string()), "{want} in {more:?}");
    }
}

#[test]
fn the_editor_settings_name_comes_right_after_open_with() {
    let _guard = english();
    let mut c = ctx();
    c.editor_path = "C:\\Tools\\Code.exe".into();
    c.editor_name = "Code".into();
    let menu = item_menu(&[text("a.txt")], &c);
    assert_eq!(&top(&menu)[..4], ["Open", "Open with…", "Open with Code", "Quick Look"]);
}

#[test]
fn a_folder_leads_with_open_a_tab_the_side_by_side_view_and_a_terminal() {
    let _guard = english();
    let menu = item_menu(&[folder("C:\\x", "docs")], &ctx());
    assert_eq!(
        top(&menu),
        [
            "Open",
            "Open in new tab",
            "Open side by side",
            "Open in Terminal",
            "Move or copy to>",
            "Compress to>",
            "More>",
            "Properties",
            "Show more options",
        ]
    );
    let more = labels(submenu(&menu, "More"));
    for want in [
        "Quick Look",
        "Open in new window",
        "Show in File Explorer",
        "Back up…",
        "Search here",
    ] {
        assert!(more.contains(&want.to_string()), "{want} in {more:?}");
    }
}

#[test]
fn an_archive_has_extract_here_right_after_open_and_its_app_after_that() {
    let _guard = english();
    let menu = item_menu(&[text("pack.zip")], &ctx());
    let rows = top(&menu);
    assert_eq!(&rows[..4], ["Open", "Extract here", "Open with its app", "Quick Look"]);
    assert!(labels(&menu).contains(&"# Archive".to_string()));
    assert!(find(&menu, "Mount as live folder").is_some());
    // The tab and the app picker are one More away.
    let more = labels(submenu(&menu, "More"));
    assert!(more.contains(&"Open in new tab".to_string()) && more.contains(&"Open with…".to_string()));
    // Once extracted, its folder is next to Open.
    let mut c = ctx();
    c.probed = Some(Probed {
        extracted: Some(Some("C:\\x\\pack".into())),
        ..Probed::default()
    });
    let rows = top(&item_menu(&[text("pack.zip")], &c));
    assert_eq!(&rows[..3], ["Open", "Open extracted folder", "Extract here"]);
}

#[test]
fn a_program_has_its_own_section_and_only_programs_run_in_the_sandbox() {
    let _guard = english();
    let exe = item_menu(&[text("setup.exe")], &ctx());
    assert!(labels(&exe).contains(&"# Program".to_string()));
    assert!(find(&exe, "Run as administrator").is_some() && find(&exe, "Run in Windows Sandbox").is_some());
    for name in [
        "a.msi", "a.ps1", "a.bat", "a.vbs", "a.js", "a.jar", "a.appx", "a.msix", "a.cmd",
    ] {
        let menu = item_menu(&[text(name)], &ctx());
        assert!(find(&menu, "Run in Windows Sandbox").is_some(), "{name}");
    }
    for name in ["a.jpg", "a.txt", "a.png", "a.mp4", "a.pdf", "a.csv", "pack.zip"] {
        let menu = item_menu(&[text(name)], &ctx());
        assert!(find(&menu, "Run in Windows Sandbox").is_none(), "{name}");
    }
    assert!(find(&item_menu(&[folder("C:\\x", "d")], &ctx()), "Run in Windows Sandbox").is_none());
    // Windows marked a document as downloaded: it may be tried out and unblocked; a picture still may not.
    let mut c = ctx();
    c.probed = Some(Probed {
        origin: Some(Origin {
            host: "example.com".into(),
            blocked: true,
        }),
        ..Probed::default()
    });
    let doc = item_menu(&[text("report.docx")], &c);
    assert!(find(&doc, "Run in Windows Sandbox").is_some() && find(&doc, "Unblock").is_some());
    let picture = item_menu(&[text("a.jpg")], &c);
    assert!(find(&picture, "Run in Windows Sandbox").is_none() && find(&picture, "Unblock").is_some());
}

#[test]
fn each_kind_gets_a_heading_and_its_most_used_rows() {
    let _guard = english();
    let video = item_menu(&[text("a.mp4")], &ctx());
    assert!(labels(&video).contains(&"# Video".to_string()));
    for want in ["Convert or shrink…", "Make a GIF…", "Take the sound out…"] {
        assert!(top(&video).contains(&want.to_string()), "{want}");
    }
    let more = labels(submenu(&video, "More video tools"));
    assert!(more.contains(&"Make subtitles…".to_string()) && more.contains(&"Even out the volume…".to_string()));
    let song = item_menu(&[text("a.mp3")], &ctx());
    assert!(labels(&song).contains(&"# Audio".to_string()));
    assert!(top(&song).contains(&"Edit song details…".to_string()));
    assert!(find(&song, "Make a GIF…").is_none() && find(&song, "Take the sound out…").is_none());
    let pdf = item_menu(&[text("a.pdf"), text("b.pdf")], &ctx());
    assert!(labels(&pdf).contains(&"# PDF".to_string()));
    assert!(top(&pdf).contains(&"Merge 2 PDFs…".to_string()));
    assert!(find(submenu(&pdf, "More PDF tools"), "Stamp…").is_some());
    let docx = item_menu(&[text("report.docx")], &ctx());
    assert!(labels(&docx).contains(&"# Document".to_string()));
    assert!(top(&docx).contains(&"Convert document…".to_string()));
    let data = item_menu(&[text("a.csv"), text("b.parquet")], &ctx());
    assert!(labels(&data).contains(&"# Data".to_string()));
    assert!(top(&data).contains(&"Query 2 files together…".to_string()));
    let iso = item_menu(&[text("os.iso")], &ctx());
    assert!(labels(&iso).contains(&"# Disk image".to_string()));
    // A text file has no kind section at all.
    let plain = labels(&item_menu(&[text("a.txt")], &ctx()));
    assert!(!plain.iter().any(|l| l.starts_with("# ")), "{plain:?}");
}

#[test]
fn nothing_the_old_menus_had_for_these_kinds_is_lost() {
    let _guard = english();
    let common = [
        "cmd:cut",
        "cmd:copy",
        "cmd:rename",
        "cmd:copy-path",
        "cmd:delete",
        "do:share",
        "cmd:open",
        "cmd:quick-look",
        "do:copy-to-choose",
        "do:move-to-choose",
        "do:tags",
        "do:edit-tags",
        "do:compress:zip",
        "do:compress:7z",
        "do:compress:tar-zst",
        "do:compress-with-password",
        "do:reveal-in-file-explorer",
        "do:previous-versions",
        "do:locks",
        "do:defender",
        "do:batch-rename-folder",
        "task:windows-menu",
        "cmd:properties",
    ];
    let wanted: Vec<(Vec<FileEntry>, Vec<&str>)> = vec![
        (
            vec![text("a.jpg")],
            vec![
                "task:open-with",
                "do:x-ray",
                "do:copy-permalink",
                "do:make-smaller",
                "do:convert-pictures",
                "do:copy-text-in",
                "do:set-wallpaper",
                "do:match-color",
                "do:rotate:right",
                "do:rotate:left",
                "do:remove-personal-info",
                "do:change-date-taken",
            ],
        ),
        (
            vec![text("a.mp4")],
            vec![
                "do:media:convert",
                "do:media:gif",
                "do:media:audio",
                "do:media:subtitles",
                "do:media:volume",
                "do:make-smaller",
            ],
        ),
        // A PDF's own "Make smaller" (do:pdf:optimize) is the one: the generic do:make-smaller is dropped there on
        // purpose, as two near-identical rows in one menu confuse.
        (
            vec![text("a.pdf")],
            vec![
                "do:pdf:split",
                "do:pdf:keep",
                "do:pdf:rotate",
                "do:pdf:optimize",
                "do:pdf:images",
                "do:pdf:encrypt",
                "do:pdf:decrypt",
                "do:pdf:stamp",
            ],
        ),
        (
            vec![text("setup.exe")],
            vec!["task:run-as-admin", "do:run-in-sandbox", "do:x-ray"],
        ),
        (
            vec![text("a.txt")],
            vec!["task:open-with", "do:x-ray", "do:copy-permalink"],
        ),
        (
            vec![text("pack.zip")],
            vec![
                "do:extract-here",
                "do:open-with-its-app",
                "do:pin-portal",
                "task:open-in-new-tabs",
                "task:open-with",
            ],
        ),
        (
            vec![folder("C:\\x", "docs")],
            vec![
                "do:paste-into",
                "task:toggle-pin",
                "task:open-in-new-tabs",
                "task:open-in-other-pane",
                "task:open-terminal",
                "do:open-in-new-window",
                "do:back-up",
                "do:search-text",
                "do:timeline",
                "do:duplicates",
                "do:cleanup",
            ],
        ),
    ];
    for (targets, extra) in wanted {
        let menu = item_menu(&targets, &ctx());
        let have = keys(&menu);
        for key in common.iter().chain(extra.iter()) {
            // Paste into is a folder's; the rest are every item's.
            if *key == "do:paste-into" && !targets[0].dir {
                continue;
            }
            assert!(
                have.iter().any(|k| k == key),
                "{} lost {key}: {have:?}",
                targets[0].name
            );
        }
    }
}

#[test]
fn two_files_of_one_kind_and_a_mix_show_what_applies_to_them() {
    let _guard = english();
    let menu = item_menu(&[text("a.jpg"), text("b.png")], &ctx());
    let all = labels(&menu);
    assert!(all.contains(&"# Pictures".to_string()));
    assert!(find(&menu, "Convert 2 pictures…").is_some());
    assert!(find(&menu, "Set as desktop background").is_none());
    assert!(find(&menu, "Rename 2 items…").is_some());
    let mixed = item_menu(&[text("a.jpg"), text("b.mp4")], &ctx());
    assert!(find(&mixed, "Convert picture…").is_some() && find(&mixed, "Convert or shrink…").is_some());
}

#[test]
fn in_search_results_recent_and_the_gallery_open_file_location_follows_open() {
    let _guard = english();
    let mut c = ctx();
    c.find = true;
    let rows = top(&item_menu(&[text("a.txt")], &c));
    assert_eq!(&rows[..2], ["Open", "Open file location"]);
    let mut c = ctx();
    c.folder = glint_model::paths::RECENT.into();
    let rows = top(&item_menu(&[text("a.txt")], &c));
    assert_eq!(&rows[..3], ["Open", "Open file location", "Remove from Recent"]);
}

#[test]
fn inside_an_archive_rows_that_cant_apply_are_absent_and_read_only_ones_are_greyed() {
    let _guard = english();
    let inside = with_attrs(text("a.jpg"), attr::IN_ARCHIVE);
    let mut c = ctx();
    c.archive = Some(an_archive());
    let menu = item_menu(&[inside], &c);
    for gone in [
        "Rotate right",
        "Convert picture…",
        "Share…",
        "Run in Windows Sandbox",
        "Move or copy to>",
    ] {
        assert!(
            !labels(&menu).contains(&gone.to_string()) && find(&menu, gone).is_none(),
            "{gone}"
        );
    }
    assert_eq!(find(&menu, "Cut").map(|e| e.disabled), Some(true));
}

#[test]
fn the_folders_view_sort_and_group_rows_dispatch_what_the_dock_and_the_header_do() {
    let _guard = english();
    let c = FolderCtx {
        view: Some(FolderView::default()),
        ..FolderCtx::default()
    };
    let menu = folder_menu("C:\\x", true, &c);
    let all = top(&menu);
    let at = |label: &str| {
        all.iter()
            .position(|l| l == label)
            .unwrap_or_else(|| panic!("{label} in {all:?}"))
    };
    assert!(at("New folder") < at("New>") && at("New>") < at("Select all"));
    assert!(at("Select all") < at("View>") && at("View>") < at("Sort by>") && at("Sort by>") < at("Group by>"));
    assert!(at("Group by>") < at("Open in Terminal") && at("Open in Terminal") < at("Search here"));
    assert!(at("Folder sizes") < at("More>") && at("More>") < at("Properties"));
    let view = submenu(&menu, "View");
    let run = |nodes: &[Node], label: &str| find(nodes, label).map(|e| e.run.clone());
    assert_eq!(run(view, "List"), Some(Run::Command(Command::ListView)));
    assert_eq!(
        run(view, "Grid, with thumbnails"),
        Some(Run::Command(Command::GridView))
    );
    assert_eq!(run(view, "Columns"), Some(Run::Command(Command::ColumnsView)));
    assert_eq!(
        run(view, "Show hidden files"),
        Some(Run::Command(Command::ToggleHidden))
    );
    assert_eq!(run(view, "Preview pane"), Some(Run::Command(Command::TogglePreview)));
    assert_eq!(find(view, "List").and_then(|e| e.check), Some(true));
    assert_eq!(find(view, "Preview pane").and_then(|e| e.check), Some(true));
    // Tile sizes only matter in the grid.
    assert!(find(view, "Larger tiles").is_none());
    let sort = submenu(&menu, "Sort by");
    assert_eq!(
        run(sort, "Size"),
        Some(Run::Do(Task::SortBy(glint_model::entries::SortKey::Size)))
    );
    assert_eq!(run(sort, "Descending"), Some(Run::Do(Task::SortOrder { desc: true })));
    assert_eq!(find(sort, "Name").and_then(|e| e.check), Some(true));
    assert_eq!(find(sort, "Ascending").and_then(|e| e.check), Some(true));
    let group = submenu(&menu, "Group by");
    assert_eq!(
        run(group, "Kind"),
        Some(Run::Do(Task::GroupBy(glint_model::groups::GroupBy::Kind)))
    );
    // The grid adds its tile sizes.
    let mut grid = c.clone();
    if let Some(v) = &mut grid.view {
        v.look.view = glint_model::store::ViewMode::Grid;
    }
    let menu = folder_menu("C:\\x", true, &grid);
    let view = submenu(&menu, "View");
    assert_eq!(run(view, "Larger tiles"), Some(Run::Command(Command::GridLarger)));
    assert_eq!(
        run(view, "Small"),
        Some(Run::Do(Task::TileSize(glint_model::grid::TileSize::Small)))
    );
}

#[test]
fn the_folder_menu_keeps_the_rows_the_old_one_had_somewhere() {
    let _guard = english();
    let mut c = FolderCtx {
        view: Some(FolderView::default()),
        files_on_clipboard: true,
        ..FolderCtx::default()
    };
    c.data.undo = Some("copy".into());
    let menu = folder_menu("C:\\x", true, &c);
    let have = keys(&menu);
    for key in [
        "cmd:paste",
        "cmd:undo",
        "task:toggle-pin",
        "cmd:refresh",
        "cmd:new-folder",
        "do:new-text-document",
        "do:paste-queued",
        "do:paste-as-link",
        "cmd:select-all",
        "do:folder-sizes",
        "do:search-text",
        "do:timeline",
        "do:time-machine",
        "do:history",
        "do:locks",
        "do:defender",
        "do:duplicates",
        "do:cleanup",
        "do:rule",
        "do:share",
        "task:open-in-file-explorer",
        "task:open-terminal",
        "task:show-properties",
    ] {
        assert!(have.iter().any(|k| k == key), "lost {key}: {have:?}");
    }
    // Paste is a row of its own only while there is something to paste.
    assert!(top(&menu).contains(&"Paste".to_string()));
    let empty = folder_menu(
        "C:\\x",
        true,
        &FolderCtx {
            view: Some(FolderView::default()),
            ..FolderCtx::default()
        },
    );
    assert!(!top(&empty).contains(&"Paste".to_string()));
    // A sidebar place follows the same order, without the view's choices.
    let place = top(&folder_menu("C:\\x\\docs", false, &FolderCtx::default()));
    assert_eq!(
        place,
        [
            "Open",
            "Open in new tab",
            "Open in Terminal",
            "Search here",
            "More>",
            "Properties"
        ]
    );
}

#[test]
fn typing_finds_rows_inside_more_and_ren_finds_rename_first() {
    let _guard = english();
    let menu = item_menu(&[text("a.jpg")], &ctx());
    let found = filter::matches(&menu, "ren");
    assert_eq!(
        found.first().map(|f| f.entry.label.to_string()).as_deref(),
        Some("Rename"),
        "{found:?}"
    );
    let found = filter::matches(&menu, "x-ray");
    assert!(
        found.iter().any(|f| f.entry.label == "File X-Ray"),
        "a row under More › Inspect & check is found"
    );
    let found = filter::matches(&menu, "rotate left");
    assert!(found.iter().any(|f| f.entry.label == "Rotate left"));
}

#[test]
fn a_pdf_has_one_make_smaller_and_a_mixed_selection_keeps_the_generic_one() {
    let _guard = english();
    let pdf = item_menu(&[text("a.pdf")], &ctx());
    assert!(find(&pdf, "Make smaller").is_some() && find(&pdf, "Make smaller…").is_none());
    let mixed = item_menu(&[text("a.pdf"), text("b.jpg")], &ctx());
    assert!(find(&mixed, "Make smaller…").is_some());
    let picture = item_menu(&[text("a.jpg")], &ctx());
    assert!(top(&picture).len() <= 12);
}
