//! Tests of the menus: what each holds (read off the data, in the TypeScript's order, with its conditions), and
//! headless-window tests of opening one at a point, the keyboard, choosing, submenus, Windows' own menu and the
//! Arabic interface.

use super::*;
use crate::explorer::fake::{file, folder, FakeDisk};
use crate::explorer::{self, NavOptions, NoAccess};
use crate::{actions, testing};
use glint_model::entries::{attr, with_details, RawEntry};
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{Modifiers, TestAppContext, VisualTestContext};
use std::cell::RefCell;
use std::rc::Rc;

// ---- Reading a menu --------------------------------------------------------------------------------------

/// A menu as one line per node: items by label, submenus as `label>`, the strip as `[a|b]`, a line as `-`.
fn labels(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .map(|node| match node {
            Node::Item(e) => e.label.to_string(),
            Node::Menu { label, .. } => format!("{label}>"),
            Node::Heading(h) => format!("# {h}"),
            Node::Separator => "-".into(),
            Node::Strip(entries) => format!(
                "[{}]",
                entries
                    .iter()
                    .map(|e| e.label.to_string())
                    .collect::<Vec<_>>()
                    .join("|")
            ),
        })
        .collect()
}

/// The row with this label, anywhere in the menu (the strip and submenus included).
fn find<'a>(nodes: &'a [Node], label: &str) -> Option<&'a Entry> {
    nodes.iter().find_map(|node| match node {
        Node::Item(e) if e.label == label => Some(e),
        Node::Strip(entries) => entries.iter().find(|e| e.label == label),
        Node::Menu { items, .. } => find(items, label),
        _ => None,
    })
}

/// The items of the submenu called `label`.
fn submenu<'a>(nodes: &'a [Node], label: &str) -> &'a [Node] {
    nodes
        .iter()
        .find_map(|node| match node {
            Node::Menu { label: l, items, .. } if l == label => Some(items.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no submenu {label:?} in {:?}", labels(nodes)))
}

/// Asserts that `wanted` appear in `got` in that order (other lines may lie between).
fn in_order(got: &[String], wanted: &[&str]) {
    let mut at = 0;
    for want in wanted {
        match got[at..].iter().position(|g| g == want) {
            Some(found) => at += found + 1,
            None => panic!("{want:?} is missing or out of order in {got:?}"),
        }
    }
}

fn text(name: &str) -> FileEntry {
    file("C:\\x", name, 100, 0)
}

fn with_attrs(mut entry: FileEntry, attrs: u32) -> FileEntry {
    entry.attrs |= attrs;
    entry
}

fn english() -> std::sync::MutexGuard<'static, ()> {
    let guard = testing::lock();
    testing::set_rtl(false);
    guard
}

fn ctx() -> ItemCtx {
    ItemCtx {
        folder: "C:\\x".into(),
        ..ItemCtx::default()
    }
}

fn an_archive() -> glint_model::entries::InArchive {
    glint_model::entries::InArchive {
        path: "C:\\x\\pack.zip".into(),
        outer: "C:\\x\\pack.zip".into(),
        format: "zip",
        entries: 3,
        cached: true,
        read_ms: 0.,
    }
}

#[test]
fn a_file_gets_the_typescripts_items_in_its_order() {
    let _guard = english();
    let menu = item_menu(&[text("alpha.txt")], &ctx());
    assert_eq!(
        labels(&menu),
        [
            "[Cut|Copy|Rename|Copy path|Delete]",
            "Open",
            "Quick Look",
            "Open with…",
            "Run in Windows Sandbox",
            "Previous versions…",
            "Scripts>",
            "Show in File Explorer",
            "-",
            "Tags>",
            "Encrypt…",
            "Compress to>",
            "Rename items in this folder…",
            "-",
            "Inspect & check>",
            "Clean up & share>",
            "-",
            "Properties",
            "Show more options",
        ]
    );
    // The keys are the ones the bindings table gives, shown beside the commands.
    assert_eq!(find(&menu, "Cut").and_then(|e| e.hint.as_deref()), Some("Ctrl+X"));
    assert_eq!(find(&menu, "Rename").and_then(|e| e.hint.as_deref()), Some("F2"));
    assert_eq!(find(&menu, "Delete").and_then(|e| e.hint.as_deref()), Some("Del"));
    assert_eq!(find(&menu, "Open").and_then(|e| e.hint.as_deref()), Some("Enter"));
    assert_eq!(find(&menu, "Quick Look").and_then(|e| e.hint.as_deref()), Some("Space"));
    assert_eq!(
        find(&menu, "Properties").and_then(|e| e.hint.as_deref()),
        Some("Alt+Enter")
    );
    assert_eq!(
        find(&menu, "Show more options").and_then(|e| e.hint.as_deref()),
        Some("Shift+right-click")
    );
    assert_eq!(find(&menu, "Delete").map(|e| e.danger), Some(true));
    assert_eq!(
        labels(submenu(&menu, "Inspect & check")),
        [
            "File X-Ray",
            "What's locking this?",
            "-",
            "Scan with Microsoft Defender",
            "Look for passwords and keys",
            "Check with YARA rules",
        ]
    );
    assert_eq!(
        labels(submenu(&menu, "Compress to")),
        [
            "Zip file",
            "7z file",
            "tar.zst file",
            "In place (still opens normally)…",
            "-",
            "With a password…"
        ]
    );
    assert_eq!(
        labels(submenu(&menu, "Clean up & share")),
        [
            "Send to a phone nearby…",
            "Send to another PC…",
            "Share on this network…",
            "Share with Windows…"
        ]
    );
}

#[test]
fn a_folder_can_be_pasted_into_pinned_opened_beside_and_in_a_terminal() {
    let _guard = english();
    let docs = folder("C:\\x", "docs");
    let menu = item_menu(std::slice::from_ref(&docs), &ctx());
    let all = labels(&menu);
    assert_eq!(
        all[0],
        "[Cut|Copy|Paste into folder|Rename|Copy path|Pin to sidebar|Delete]"
    );
    in_order(
        &all,
        &[
            "Open",
            "Open in new tab",
            "Open side by side",
            "Quick Look",
            "Open in Terminal",
            "Open in new window",
            "Scripts>",
            "Show in File Explorer",
            "Back up…",
            "Search here",
            "Inspect & check>",
            "Properties",
        ],
    );
    assert_eq!(
        find(&menu, "Open in new tab").and_then(|e| e.hint.as_deref()),
        Some("Ctrl+Enter")
    );
    assert_eq!(
        find(&menu, "Open in new window").and_then(|e| e.hint.as_deref()),
        Some("Ctrl+N")
    );
    // A folder is not run, and a terminal and folder tools are its own.
    assert!(find(&menu, "Open with…").is_none() && find(&menu, "Run in Windows Sandbox").is_none());
    assert!(find(&menu, "File X-Ray").is_none());
    assert!(find(&menu, "Find duplicate files").is_some() && find(&menu, "Recent changes").is_some());
    // Pinned folders offer to be unpinned, in the accent color.
    let pinned = ItemCtx {
        pinned: vec![docs.path.clone()],
        ..ctx()
    };
    let menu = item_menu(&[docs], &pinned);
    let pin = find(&menu, "Unpin from sidebar").expect("an unpin button");
    assert!(pin.active);
    assert!(find(&menu, "Pin to sidebar").is_none());
}

#[test]
fn several_items_get_plural_labels_and_only_what_works_on_many() {
    let _guard = english();
    let docs = folder("C:\\x", "docs");
    let more = folder("C:\\x", "more");
    let menu = item_menu(&[docs.clone(), more], &ctx());
    let all = labels(&menu);
    assert_eq!(all[0], "[Cut|Copy|Rename 2 items…|Copy paths|Delete]");
    assert!(all.contains(&"Open 2 folders in new tabs".to_string()), "{all:?}");
    assert!(find(&menu, "Quick Look").is_none() && find(&menu, "Properties").is_none());
    assert!(find(&menu, "Rename items in this folder…").is_none());
    assert_eq!(
        find(&menu, "Rename 2 items…").map(|e| (e.run.clone(), e.hint.clone())),
        Some((Run::Command(Command::BatchRename), Some("F2".into())))
    );
    // Two files can be compared; Windows' own menu still has them when they share a folder.
    let menu = item_menu(&[text("alpha.txt"), text("beta.md")], &ctx());
    assert!(find(&menu, "Open 2 items").is_some());
    let compare = find(&menu, "Compare “alpha.txt” and “beta.md”").expect("compare two files");
    assert_eq!(compare.hint.as_deref(), Some("Ctrl+M"));
    assert!(find(&menu, "Show more options").is_some());
    // …but not when they are in different folders.
    let elsewhere = file("C:\\y", "gamma.txt", 1, 0);
    let menu = item_menu(&[text("alpha.txt"), elsewhere], &ctx());
    assert!(find(&menu, "Show more options").is_none());
    // What only some of them can do is offered for those.
    let menu = item_menu(&[text("alpha.txt"), text("beta.md")], &ctx());
    match find(&menu, "Export as PDF").map(|e| e.run.clone()) {
        Some(Run::Ask(MenuEvent::ExportPdf { paths, typst })) => {
            assert_eq!((paths, typst), (vec!["C:\\x\\beta.md".to_string()], false))
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_compare_label_keeps_an_arabic_name_whole() {
    let _guard = english();
    let menu = item_menu(&[text("تقرير الربع 1.txt"), text("b.txt")], &ctx());
    assert!(find(&menu, "Compare “تقرير الربع 1.txt” and “b.txt”").is_some());
}

#[test]
fn files_get_the_tools_that_read_them() {
    let _guard = english();
    let pdf = item_menu(&[text("a.pdf"), text("b.pdf")], &ctx());
    assert_eq!(
        labels(submenu(&pdf, "PDF tools")),
        [
            "Merge 2 PDFs…",
            "Split…",
            "Keep or take out pages…",
            "Turn pages…",
            "Make smaller",
            "Save its pictures…",
            "-",
            "Add a password…",
            "Take the password off…",
            "Stamp…",
        ]
    );
    assert!(find(&pdf, "Make smaller…").is_some(), "Make smaller works on PDFs too");
    let video = item_menu(&[text("a.mp4")], &ctx());
    assert_eq!(
        labels(submenu(&video, "Media")),
        [
            "Convert or shrink…",
            "Take the sound out…",
            "Make a GIF…",
            "Make subtitles…",
            "Even out the volume…",
        ]
    );
    let song = item_menu(&[text("a.mp3")], &ctx());
    let media = labels(submenu(&song, "Media"));
    assert!(!media.contains(&"Make a GIF…".to_string()) && media.contains(&"Edit song details…".to_string()));
    let picture = item_menu(&[text("a.jpg")], &ctx());
    for want in [
        "Convert picture…",
        "Set as desktop background",
        "Rotate right",
        "Rotate left",
        "Change when it was taken…",
    ] {
        assert!(find(&picture, want).is_some(), "{want}");
    }
    let data = item_menu(&[text("a.csv"), text("b.parquet")], &ctx());
    assert!(find(&data, "Query 2 files together…").is_some() && find(&data, "Convert data…").is_some());
    let age = item_menu(&[text("a.txt.age")], &ctx());
    assert!(find(&age, "Decrypt…").is_some() && find(&age, "Encrypt…").is_none());
    let program = item_menu(&[text("setup.exe")], &ctx());
    assert!(find(&program, "Run as administrator").is_some());
    let disk_image = item_menu(&[text("os.iso")], &ctx());
    assert!(find(&disk_image, "Mount").is_some());
    let docx = item_menu(&[text("report.docx")], &ctx());
    assert!(find(&docx, "Open as archive").is_some() && find(&docx, "Extract here").is_some());
    let zip = item_menu(&[text("pack.zip")], &ctx());
    for want in ["Open with its app", "Extract here", "Open in new tab"] {
        assert!(find(&zip, want).is_some(), "{want}");
    }
    assert!(
        find(&zip, "Open as archive").is_none(),
        "a zip opens like a folder already"
    );
}

#[test]
fn editors_scripts_and_devices_come_from_what_was_read() {
    let _guard = english();
    let mut c = ctx();
    c.editor_path = "C:\\Tools\\Code.exe".into();
    c.editor_name = "Code".into();
    c.data.openers = vec![glint_core::openers::Opener {
        id: "vscode",
        name: "VS Code",
        terminal: false,
        path: "C:\\Code.exe".into(),
    }];
    c.data.scripts = vec![
        glint_core::scripts::Script {
            name: "Hash".into(),
            path: "C:\\scripts\\hash.ps1".into(),
            program: "pwsh".into(),
            args: vec![],
            missing: None,
        },
        glint_core::scripts::Script {
            name: "Lint".into(),
            path: "C:\\scripts\\lint.sh".into(),
            program: "sh".into(),
            args: vec![],
            missing: Some("shellcheck".into()),
        },
    ];
    c.devices = vec![Device {
        id: "pc1".into(),
        name: "Laptop".into(),
    }];
    let menu = item_menu(&[text("alpha.txt")], &c);
    assert!(find(&menu, "Open with Code").is_some());
    assert_eq!(labels(submenu(&menu, "Open in")), ["VS Code"]);
    assert_eq!(
        labels(submenu(&menu, "Scripts")),
        [
            "Hash",
            "Lint: install shellcheck to run it",
            "-",
            "Open the Scripts folder"
        ]
    );
    assert_eq!(
        labels(submenu(submenu(&menu, "Clean up & share"), "Send to my PC")),
        ["Laptop"]
    );
    // With no editor found there is no "Open in" at all.
    assert!(find(&item_menu(&[text("alpha.txt")], &ctx()), "Open in").is_none());
}

#[test]
fn tags_are_checked_when_every_item_has_them_and_toggle_off_then() {
    let _guard = english();
    let mut c = ctx();
    c.data.tags.insert("c:\\x\\alpha.txt".into(), vec!["red".into()]);
    let menu = item_menu(&[text("alpha.txt")], &c);
    let tags = submenu(&menu, "Tags");
    let red = find(tags, "Red").expect("red");
    assert_eq!(red.check, Some(true));
    assert_eq!(
        red.run,
        Run::Ask(MenuEvent::SetTags {
            paths: vec!["C:\\x\\alpha.txt".into()],
            add: vec![],
            remove: vec!["Red".into()],
        })
    );
    let blue = find(tags, "Blue").expect("blue");
    assert_eq!(blue.check, Some(false));
    assert!(matches!(&blue.run, Run::Ask(MenuEvent::SetTags { add, .. }) if add == &vec!["Blue".to_string()]));
    assert_eq!(labels(tags).last().map(String::as_str), Some("Edit tags…"));
}

#[test]
fn a_tag_only_some_of_the_items_have_goes_on_them_all_and_one_all_have_comes_off_them_all() {
    let _guard = english();
    let mut c = ctx();
    c.data
        .tags
        .insert("c:\\x\\alpha.txt".into(), vec!["Red".into(), "blue".into()]);
    c.data.tags.insert("c:\\x\\beta.md".into(), vec!["RED".into()]);
    let both = [text("alpha.txt"), text("beta.md")];
    let paths = vec!["C:\\x\\alpha.txt".to_string(), "C:\\x\\beta.md".to_string()];
    let menu = item_menu(&both, &c);
    let tags = submenu(&menu, "Tags");
    // Every item has Red, in any case: checked, and choosing it takes it off both.
    let red = find(tags, "Red").expect("red");
    assert_eq!(red.check, Some(true));
    assert_eq!(
        red.run,
        Run::Ask(MenuEvent::SetTags {
            paths: paths.clone(),
            add: vec![],
            remove: vec!["Red".into()],
        })
    );
    // Only one has Blue: not checked, and choosing it puts it on both.
    let blue = find(tags, "Blue").expect("blue");
    assert_eq!(blue.check, Some(false));
    assert_eq!(
        blue.run,
        Run::Ask(MenuEvent::SetTags {
            paths: paths.clone(),
            add: vec!["Blue".into()],
            remove: vec![],
        })
    );
    // None has Green.
    assert_eq!(find(tags, "Green").and_then(|e| e.check), Some(false));
    // A third item whose tags were not read yet counts as having none: nothing is all-checked any more.
    let three = [text("alpha.txt"), text("beta.md"), text("gamma.txt")];
    let menu = item_menu(&three, &c);
    assert_eq!(find(submenu(&menu, "Tags"), "Red").and_then(|e| e.check), Some(false));
    // The strip of colors and "Edit tags…" are for the whole selection.
    let last = labels(submenu(&menu, "Tags")).last().cloned();
    assert_eq!(last.as_deref(), Some("Edit tags…"));
    let edit = find(submenu(&menu, "Tags"), "Edit tags…").expect("edit");
    assert!(matches!(&edit.run, Run::Ask(MenuEvent::EditTags { paths }) if paths.len() == 3));
}

#[test]
fn in_the_other_pane_items_copy_move_and_compare() {
    let _guard = english();
    let mut c = ctx();
    c.dual = true;
    c.other_folder = Some("Docs".into());
    c.other_selected = vec![file("C:\\y", "alpha.txt", 1, 0)];
    c.other_listing = Some((
        "C:\\y".into(),
        vec![file("C:\\y", "alpha.txt", 1, 0), file("C:\\y", "beta.md", 1, 0)].into(),
    ));
    let menu = item_menu(&[text("alpha.txt")], &c);
    let all = labels(&menu);
    in_order(
        &all,
        &[
            "Copy to “Docs” in the other pane",
            "Move to “Docs” in the other pane",
            "Compare with “alpha.txt” in the other pane",
        ],
    );
    assert_eq!(
        find(&menu, "Copy to “Docs” in the other pane").map(|e| e.disabled),
        Some(false)
    );
    // The left pane's file is the left side of the comparison; the right pane's, the right.
    let compare = find(&menu, "Compare with “alpha.txt” in the other pane").expect("compare");
    assert_eq!(
        compare.run,
        Run::Ask(MenuEvent::Compare {
            left: "C:\\x\\alpha.txt".into(),
            right: "C:\\y\\alpha.txt".into()
        })
    );
    c.pane = 1;
    let menu = item_menu(&[text("alpha.txt")], &c);
    assert!(matches!(
        &find(&menu, "Compare with “alpha.txt” in the other pane").map(|e| e.run.clone()),
        Some(Run::Ask(MenuEvent::Compare { left, .. })) if left == "C:\\y\\alpha.txt"
    ));
    // With nothing to copy into, the commands are greyed out and keep their plain words.
    let mut none = ctx();
    none.dual = true;
    let menu = item_menu(&[text("alpha.txt")], &none);
    assert_eq!(find(&menu, "Copy to the other pane").map(|e| e.disabled), Some(true));
    assert_eq!(find(&menu, "Move to the other pane").map(|e| e.disabled), Some(true));
    // Opening a folder there is "in the other pane" when two panes are shown, "side by side" when not.
    let menu = item_menu(&[folder("C:\\x", "docs")], &c);
    assert!(find(&menu, "Open in the other pane").is_some() && find(&menu, "Open side by side").is_none());
    // A file with a namesake in the other pane compares with it, and Ctrl+M is its key when it is the only way.
    let mut namesake = ctx();
    namesake.dual = true;
    namesake.other_listing = Some(("C:\\y".into(), vec![file("C:\\y", "alpha.txt", 1, 0)].into()));
    let menu = item_menu(&[text("alpha.txt")], &namesake);
    assert_eq!(
        find(&menu, "Compare with “alpha.txt” in the other pane").and_then(|e| e.hint.as_deref()),
        Some("Ctrl+M")
    );
}

#[test]
fn inside_an_archive_changes_are_greyed_out_and_a_note_says_why() {
    let _guard = english();
    let inside = with_attrs(text("a.txt"), attr::IN_ARCHIVE);
    let mut c = ctx();
    c.archive = Some(an_archive());
    let menu = item_menu(&[inside], &c);
    let all = labels(&menu);
    assert_eq!(all.last().map(String::as_str), Some("Read-only: inside an archive"));
    assert_eq!(find(&menu, "Cut").map(|e| e.disabled), Some(true));
    assert_eq!(
        find(&menu, "Delete").map(|e| (e.disabled, e.danger)),
        Some((true, false))
    );
    assert_eq!(find(&menu, "Rename").map(|e| e.disabled), Some(true));
    assert_eq!(
        find(&menu, "Copy").map(|e| e.disabled),
        Some(false),
        "copying out still works"
    );
    assert!(find(&menu, "Show the archive in File Explorer").is_some());
    for gone in [
        "Properties",
        "Show more options",
        "Show in File Explorer",
        "Scripts",
        "Tags",
        "Encrypt…",
        "Compress to",
    ] {
        assert!(
            find(&menu, gone).is_none() && !all.contains(&format!("{gone}>")),
            "{gone}"
        );
    }
    assert_eq!(
        find(&menu, "Read-only: inside an archive").map(|e| e.disabled),
        Some(true)
    );
}

#[test]
fn on_a_remote_connection_nothing_changes_and_a_note_says_so() {
    let _guard = english();
    let there = with_attrs(text("a.txt"), attr::REMOTE);
    let menu = item_menu(&[there], &ctx());
    let all = labels(&menu);
    assert_eq!(
        all.last().map(String::as_str),
        Some("Read-only: on a remote connection")
    );
    assert_eq!(find(&menu, "Cut").map(|e| e.disabled), Some(true));
    assert!(find(&menu, "Show in File Explorer").is_none() && find(&menu, "Properties").is_none());
    assert!(find(&menu, "Show more options").is_none());
    assert!(
        find(&menu, "Open with…").is_some(),
        "its file is downloaded to be opened"
    );
}

#[test]
fn in_a_folder_windows_keeps_closed_only_folders_open_and_a_note_says_so() {
    let _guard = english();
    let listed = with_attrs(text("a.txt"), attr::CLOSED);
    let menu = item_menu(&[listed], &ctx());
    assert_eq!(
        labels(&menu).last().map(String::as_str),
        Some("Read-only: Windows keeps this folder closed")
    );
    assert!(find(&menu, "Open with…").is_none() && find(&menu, "Show more options").is_none());
    assert_eq!(find(&menu, "Cut").map(|e| e.disabled), Some(true));
}

#[test]
fn in_the_recycle_bin_items_go_back_or_for_good() {
    let _guard = english();
    let mut gone = with_details(
        RawEntry {
            name: "a.txt".into(),
            attrs: attr::RECYCLED,
            origin: Some("C:\\Users\\me".into()),
            ..Default::default()
        },
        "$Recycle.Bin",
    );
    gone.path = "$Recycle.Bin\\a.txt".into();
    let menu = item_menu(std::slice::from_ref(&gone), &ctx());
    assert_eq!(
        labels(&menu),
        [
            "Restore",
            "Quick Look",
            "Open the folder it was deleted from",
            "-",
            "Copy original path",
            "-",
            "Delete for good",
        ]
    );
    assert_eq!(
        find(&menu, "Delete for good").map(|e| (e.danger, e.hint.clone())),
        Some((true, Some("Del".into())))
    );
    assert_eq!(
        find(&menu, "Copy original path").map(|e| e.run.clone()),
        Some(Run::Do(Task::CopyText {
            text: "C:\\Users\\me\\a.txt".into(),
            paths: 1
        }))
    );
    let mut other = gone.clone();
    other.name = "b.txt".into();
    other.path = "$Recycle.Bin\\b.txt".into();
    let menu = item_menu(&[gone, other], &ctx());
    assert_eq!(
        labels(&menu),
        ["Restore 2 items", "-", "Copy original paths", "-", "Delete for good"]
    );
}

#[test]
fn a_menu_has_no_stray_lines_whatever_the_items() {
    let _guard = english();
    let samples = [
        vec![text("a.txt")],
        vec![folder("C:\\x", "docs")],
        vec![text("a.mp4"), text("b.mp3")],
        vec![with_attrs(text("a.txt"), attr::IN_ARCHIVE)],
        vec![with_attrs(text("a.txt"), attr::REMOTE)],
        vec![with_attrs(text("a.txt"), attr::CLOSED)],
    ];
    for targets in samples {
        let menu = item_menu(&targets, &ctx());
        let all = labels(&menu);
        assert_ne!(all.first().map(String::as_str), Some("-"), "{all:?}");
        assert_ne!(all.last().map(String::as_str), Some("-"), "{all:?}");
        assert!(!all.windows(2).any(|w| w[0] == "-" && w[1] == "-"), "{all:?}");
        for node in &menu {
            if let Node::Menu { items, .. } = node {
                assert!(!items.is_empty());
                let inner = labels(items);
                assert_ne!(inner.first().map(String::as_str), Some("-"), "{inner:?}");
                assert_ne!(inner.last().map(String::as_str), Some("-"), "{inner:?}");
            }
        }
    }
}

#[test]
fn windows_own_menu_fits_files_on_disk_in_one_folder() {
    assert!(shell_menu_fits(&[text("a.txt"), text("b.txt")]));
    assert!(!shell_menu_fits(&[]));
    assert!(!shell_menu_fits(&[text("a.txt"), file("C:\\y", "b.txt", 1, 0)]));
    assert!(!shell_menu_fits(&[with_attrs(text("a.txt"), attr::IN_ARCHIVE)]));
    assert!(!shell_menu_fits(&[with_attrs(text("a.txt"), attr::REMOTE)]));
    assert!(!shell_menu_fits(&[with_attrs(text("a.txt"), attr::CLOSED)]));
    assert!(!shell_menu_fits(&[with_attrs(text("a.txt"), attr::RECYCLED)]));
}

// ---- The folder menu -------------------------------------------------------------------------------------

fn shown_folder() -> FolderCtx {
    FolderCtx {
        view: Some(FolderView::default()),
        ..FolderCtx::default()
    }
}

#[test]
fn the_folder_shown_has_paste_undo_new_and_the_folders_tools() {
    let _guard = english();
    let menu = folder_menu("C:\\x", true, &shown_folder());
    assert_eq!(
        labels(&menu),
        [
            "[Paste|Undo|Copy path|Pin to sidebar|Refresh]",
            "New folder",
            "New>",
            "Paste with the copy queue",
            "Select all",
            "-",
            "Folder sizes",
            "Search here",
            "Inspect & check>",
            "Clean up & share>",
            "-",
            "Open in File Explorer",
            "Open in Terminal",
            "Scripts>",
            "-",
            "Properties",
        ]
    );
    assert_eq!(find(&menu, "Paste").and_then(|e| e.hint.as_deref()), Some("Ctrl+V"));
    assert_eq!(find(&menu, "Refresh").and_then(|e| e.hint.as_deref()), Some("F5"));
    assert_eq!(
        find(&menu, "New folder").and_then(|e| e.hint.as_deref()),
        Some("Ctrl+Shift+N")
    );
    assert_eq!(
        find(&menu, "Select all").and_then(|e| e.hint.as_deref()),
        Some("Ctrl+A")
    );
    assert_eq!(
        find(&menu, "Properties").and_then(|e| e.hint.as_deref()),
        Some("Alt+Enter")
    );
    // Nothing to undo: greyed out, plain. Something: it says what it takes back.
    assert_eq!(find(&menu, "Undo").map(|e| e.disabled), Some(true));
    let mut with_undo = shown_folder();
    with_undo.data.undo = Some("copy".into());
    let menu = folder_menu("C:\\x", true, &with_undo);
    assert_eq!(find(&menu, "Undo the copy").map(|e| e.disabled), Some(false));
    assert_eq!(
        labels(submenu(&menu, "Clean up & share")),
        [
            "Find duplicate files",
            "Clean up…",
            "A rule for this folder…",
            "Export as ncdu JSON…",
            "-",
            "Send this folder to another PC…",
            "Share this folder on this network…",
            "Receive from another PC here…",
        ]
    );
    assert_eq!(
        labels(submenu(&menu, "Inspect & check")),
        [
            "Recent changes",
            "Moved and deleted here",
            "Previous versions of this folder",
            "What's locking this?",
            "-",
            "Scan with Microsoft Defender",
            "Look for passwords and keys",
            "Check with YARA rules",
        ]
    );
}

#[test]
fn new_lists_a_text_document_then_the_kinds_windows_has() {
    let _guard = english();
    let mut c = shown_folder();
    c.data.new_kinds = vec![glint_core::newmenu::NewKind {
        ext: ".docx".into(),
        name: "Microsoft Word Document".into(),
        icon: None,
        by_app: true,
    }];
    let menu = folder_menu("C:\\x", true, &c);
    assert_eq!(
        labels(submenu(&menu, "New")),
        ["Text document", "-", "Microsoft Word Document"]
    );
    assert_eq!(
        find(&menu, "Microsoft Word Document").map(|e| e.run.clone()),
        Some(Run::Ask(MenuEvent::NewOfKind { ext: ".docx".into() }))
    );
}

#[test]
fn a_place_or_tile_opens_pastes_into_and_pins_but_has_no_new_or_select_all() {
    let _guard = english();
    let menu = folder_menu("C:\\x\\docs", false, &FolderCtx::default());
    let all = labels(&menu);
    assert_eq!(all[0], "[Paste into folder|Copy path|Pin to sidebar]");
    in_order(
        &all,
        &[
            "Open",
            "Open in new tab",
            "Search here",
            "Open in File Explorer",
            "Properties",
        ],
    );
    for gone in [
        "New folder",
        "Select all",
        "Folder sizes",
        "Paste with the copy queue",
        "New>",
    ] {
        assert!(!all.contains(&gone.to_string()), "{gone}");
    }
    assert_eq!(find(&menu, "Properties").and_then(|e| e.hint.clone()), None);
    assert!(
        matches!(find(&menu, "Paste into folder").map(|e| &e.run), Some(Run::Ask(MenuEvent::PasteInto { folder })) if folder == "C:\\x\\docs")
    );
    // Pinned: the strip says unpin. And the folder's own "Send to another PC…" has the short words.
    let pinned = FolderCtx {
        pinned: vec!["C:\\x\\docs".into()],
        ..FolderCtx::default()
    };
    let menu = folder_menu("C:\\x\\docs", false, &pinned);
    assert_eq!(find(&menu, "Unpin from sidebar").map(|e| e.active), Some(true));
    assert!(find(&menu, "Send to another PC…").is_some());
}

#[test]
fn a_folder_windows_keeps_closed_leads_with_get_access_and_greys_out_changes() {
    let _guard = english();
    let mut c = shown_folder();
    if let Some(view) = &mut c.view {
        view.no_access = Some(NoAccess::default());
    }
    let menu = folder_menu("C:\\x", true, &c);
    let all = labels(&menu);
    assert_eq!(all[1], "Get access");
    assert_eq!(all[2], "-");
    assert_eq!(find(&menu, "Paste").map(|e| e.disabled), Some(true));
    assert_eq!(find(&menu, "New folder").map(|e| e.disabled), Some(true));
    assert_eq!(
        find(&menu, "Select all").map(|e| e.disabled),
        Some(true),
        "nothing was listed"
    );
    assert!(find(&menu, "Search here").is_none() && find(&menu, "Open in Terminal").is_none());
    assert_eq!(labels(submenu(&menu, "Inspect & check")), ["What's locking this?"]);
    // Windows refused outright (even administrators): no Get access to offer.
    if let Some(view) = &mut c.view {
        view.no_access = Some(NoAccess {
            refused: true,
            ..NoAccess::default()
        });
    }
    assert!(find(&folder_menu("C:\\x", true, &c), "Get access").is_none());
    // The index listed it: select all works, and the menu ends with why nothing opens.
    if let Some(view) = &mut c.view {
        view.no_access = Some(NoAccess::default());
        view.from_index = true;
    }
    let menu = folder_menu("C:\\x", true, &c);
    assert_eq!(find(&menu, "Select all").map(|e| e.disabled), Some(false));
    assert_eq!(
        labels(&menu).last().map(String::as_str),
        Some("Read-only: Windows keeps this folder closed")
    );
}

#[test]
fn in_an_archive_the_folder_menu_is_read_only() {
    let _guard = english();
    let mut c = shown_folder();
    if let Some(view) = &mut c.view {
        view.archive = Some(an_archive());
    }
    let menu = folder_menu("C:\\x\\pack.zip", true, &c);
    let all = labels(&menu);
    assert_eq!(all.last().map(String::as_str), Some("Read-only: inside an archive"));
    assert_eq!(find(&menu, "Paste").map(|e| e.disabled), Some(true));
    assert_eq!(find(&menu, "New folder").map(|e| e.disabled), Some(true));
    assert!(find(&menu, "Show the archive in File Explorer").is_some());
    assert!(find(&menu, "Archive properties").is_some() && find(&menu, "Properties").is_none());
    assert!(find(&menu, "Search here").is_none() && find(&menu, "Open in Terminal").is_none());
    // A pinned folder inside an archive is read-only too, though only the path says so.
    let menu = folder_menu("C:\\x\\pack.zip\\inner", false, &FolderCtx::default());
    assert_eq!(find(&menu, "Paste into folder").map(|e| e.disabled), Some(true));
}

#[test]
fn this_pc_has_only_refresh_and_the_recycle_bin_has_its_own_menu() {
    let _guard = english();
    let menu = folder_menu(glint_model::paths::THIS_PC, false, &FolderCtx::default());
    assert_eq!(labels(&menu), ["Refresh"]);
    assert_eq!(
        find(&menu, "Refresh").map(|e| e.run.clone()),
        Some(Run::Do(Task::RefreshDrives))
    );
    let bin = glint_model::paths::RECYCLE_BIN;
    let mut c = shown_folder();
    if let Some(view) = &mut c.view {
        view.restore_shown = Some(("Restore all items".into(), vec!["a".into()]));
    }
    let menu = folder_menu(bin, true, &c);
    assert_eq!(
        labels(&menu),
        [
            "Restore all items",
            "Select all",
            "Undo",
            "Refresh",
            "-",
            "Empty the Recycle Bin"
        ]
    );
    assert_eq!(find(&menu, "Empty the Recycle Bin").map(|e| e.danger), Some(true));
    let menu = folder_menu(bin, false, &FolderCtx::default());
    assert_eq!(labels(&menu), ["Open", "Open in new tab", "-", "Empty the Recycle Bin"]);
}

#[test]
fn a_remote_connections_top_can_be_edited_and_shown_in_file_explorer() {
    let _guard = english();
    let top = "remote:cloud".to_string();
    let mut c = FolderCtx {
        remote: Some(Connection {
            id: "cloud".into(),
            name: "My cloud".into(),
            device: false,
        }),
        ..FolderCtx::default()
    };
    let menu = folder_menu(&top, false, &c);
    assert_eq!(
        labels(&menu),
        [
            "Open",
            "Open in new tab",
            "-",
            "Copy path",
            "-",
            "Show in File Explorer, on demand",
            "Edit the connection…",
            "Remove the connection…",
            "-",
            "Read-only: on a remote connection",
        ]
    );
    // Mounted: it says how to stop. A paired PC's connection is changed in Settings, not here.
    c.data.on_demand = vec!["cloud".into()];
    c.remote = Some(Connection {
        id: "cloud".into(),
        name: "My cloud".into(),
        device: true,
    });
    let menu = folder_menu(&top, false, &c);
    assert!(find(&menu, "Stop showing in File Explorer").is_some());
    assert!(find(&menu, "Edit the connection…").is_none() && find(&menu, "Remove the connection…").is_some());
    // A folder inside a remote can be pinned.
    let inside = "remote:cloud\\docs".to_string();
    let menu = folder_menu(&inside, false, &FolderCtx::default());
    assert!(find(&menu, "Pin to sidebar").is_some());
}

#[test]
fn every_icon_the_menus_draw_is_in_the_embedded_assets() {
    use gpui_kit::assets::AllAssets;
    use gpui_kit::AssetSource as _;
    let _guard = english();
    fn icons(nodes: &[Node], into: &mut Vec<IconName>) {
        for node in nodes {
            match node {
                Node::Item(e) => into.extend(e.icon),
                Node::Strip(entries) => into.extend(entries.iter().filter_map(|e| e.icon)),
                Node::Menu { icon, items, .. } => {
                    into.extend(*icon);
                    icons(items, into);
                }
                _ => {}
            }
        }
    }
    let mut c = ctx();
    c.dual = true;
    c.other_folder = Some("Docs".into());
    c.editor_name = "Code".into();
    c.devices = vec![Device {
        id: "a".into(),
        name: "b".into(),
    }];
    c.data.openers = vec![
        glint_core::openers::Opener {
            id: "vscode",
            name: "VS Code",
            terminal: false,
            path: String::new(),
        },
        glint_core::openers::Opener {
            id: "wt",
            name: "Windows Terminal",
            terminal: true,
            path: String::new(),
        },
    ];
    c.data.scripts = vec![glint_core::scripts::Script {
        name: "x".into(),
        path: String::new(),
        program: String::new(),
        args: vec![],
        missing: Some("t".into()),
    }];
    let mut found: Vec<IconName> = vec![
        IconName::Check,
        IconName::ChevronLeft,
        IconName::ChevronRight,
        IconName::ArrowLeft,
        IconName::ArrowRight,
    ];
    let cloud = with_attrs(text("a.jpg"), attr::CLOUD | attr::OFFLINE);
    for targets in [
        vec![text("a.pdf"), text("b.pdf")],
        vec![text("a.mp4"), text("b.mp3")],
        vec![text("a.jpg")],
        vec![text("a.csv"), text("b.csv")],
        vec![text("a.md"), text("c.typ"), text("d.docx"), text("e.txt.age")],
        vec![text("pack.zip")],
        vec![text("os.iso")],
        vec![folder("C:\\x", "docs")],
        vec![cloud],
        vec![with_attrs(text("a.txt"), attr::IN_ARCHIVE)],
        vec![with_attrs(text("a.txt"), attr::REMOTE)],
        vec![with_attrs(text("a.txt"), attr::CLOSED)],
        vec![with_attrs(text("a.txt"), attr::RECYCLED)],
    ] {
        icons(&item_menu(&targets, &c), &mut found);
    }
    let mut folder_ctx = shown_folder();
    folder_ctx.data = c.data.clone();
    folder_ctx.data.new_kinds = vec![glint_core::newmenu::NewKind {
        ext: ".x".into(),
        name: "x".into(),
        icon: None,
        by_app: false,
    }];
    folder_ctx.remote = Some(Connection {
        id: "r".into(),
        name: "r".into(),
        device: false,
    });
    for (path, current) in [
        ("C:\\x", true),
        ("C:\\x", false),
        (glint_model::paths::THIS_PC, false),
        (glint_model::paths::RECYCLE_BIN, true),
        ("C:\\x\\pack.zip", true),
    ] {
        icons(&folder_menu(path, current, &folder_ctx), &mut found);
    }
    icons(&folder_menu("remote:r", false, &folder_ctx), &mut found);
    assert!(found.len() > 100);
    for name in found {
        let path = name.path();
        assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
    }
}

// ---- In a window -----------------------------------------------------------------------------------------

/// The window: a list that has the keyboard and records the commands dispatched to it, and the host drawn
/// after it.
struct Shell {
    menus: Entity<MenuHost>,
    list: FocusHandle,
    log: Rc<RefCell<Vec<&'static str>>>,
}

macro_rules! record {
    ($div:expr, $cx:expr, $($action:ident),* $(,)?) => {{
        let mut div = $div;
        $( div = div.on_action($cx.listener(|this: &mut Shell, _: &actions::$action, _, _| {
            this.log.borrow_mut().push(stringify!($action))
        })); )*
        div
    }};
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(record!(
                div()
                    .id("list")
                    .size_full()
                    .key_context("FileList")
                    .track_focus(&self.list),
                cx,
                Copy,
                Cut,
                Paste,
                Rename,
                BatchRename,
                Delete,
                DeletePermanently,
                OpenItem,
                QuickLook,
                CopyPath,
                Properties,
                Undo,
                SelectAll,
                Refresh,
                NewFolder,
                OpenInNewTab,
                CompareFiles,
            ))
            .child(self.menus.clone())
    }
}

struct Ui {
    visual: VisualTestContext,
    menus: Entity<MenuHost>,
    explorer: Entity<explorer::Explorer>,
    shell: Entity<Shell>,
    log: Rc<RefCell<Vec<&'static str>>>,
    events: Rc<RefCell<Vec<MenuEvent>>>,
}

fn disk() -> FakeDisk {
    let disk = FakeDisk::new();
    disk.folder(
        "C:\\x",
        vec![
            folder("C:\\x", "docs"),
            file("C:\\x", "alpha.txt", 100, 0),
            file("C:\\x", "beta.md", 3000, 0),
            file("C:\\x", "pack.zip", 5000, 0),
            file("C:\\x", "تقرير الربع 1.pdf", 7000, 0),
        ],
    );
    disk.folder("C:\\x\\docs", vec![file("C:\\x\\docs", "inner.txt", 1, 0)]);
    disk
}

fn open(cx: &mut TestAppContext, disk: &FakeDisk, path: &str) -> Ui {
    cx.update(testing::init);
    cx.update(|cx| explorer::init_with(cx, disk.source()));
    let explorer = cx.update(|cx| explorer::explorer(cx));
    let task = explorer.update(cx, |e, cx| e.navigate_task(path, NavOptions::default(), cx));
    cx.foreground_executor().block_test(task);
    let log = Rc::new(RefCell::new(Vec::new()));
    let sink = log.clone();
    let (mut visual, shell) = testing::open_window(cx, move |window, cx| {
        let menus = cx.new(|cx| MenuHost::with_sources(Sources::empty(), cx));
        let list = cx.focus_handle();
        list.focus(window, cx);
        cx.new(|_| Shell { menus, list, log: sink })
    });
    let menus = shell.read_with(&visual, |shell, _| shell.menus.clone());
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    visual.update(|_, cx| {
        cx.subscribe(&menus, move |_, event: &MenuEvent, _| {
            sink.borrow_mut().push(event.clone())
        })
        .detach();
    });
    visual.update(|window, _| window.activate_window());
    let mut ui = Ui {
        visual,
        menus,
        explorer,
        shell,
        log,
        events,
    };
    ui.frame();
    ui
}

impl Ui {
    fn frame(&mut self) {
        self.visual.run_until_parked();
        self.visual.update(|window, cx| window.render_frame(cx));
    }

    fn select(&mut self, paths: &[&str]) {
        let explorer = self.explorer.clone();
        self.visual.update(|_, cx| {
            explorer.update(cx, |e, cx| {
                e.select_only(paths.first().copied(), cx);
                for path in paths.iter().skip(1) {
                    e.toggle_selected(path, cx);
                }
            })
        });
    }

    /// Asks for the list's menu the way the list's event does.
    fn open_menu(&mut self, target: Option<&str>, at: Option<Point<Pixels>>, shift: bool) {
        let menus = self.menus.clone();
        self.visual.update(|window, cx| {
            menus.update(cx, |m, cx| m.open_list_menu(0, target, at, shift, window, cx));
        });
        self.frame();
    }

    fn open_background(&mut self, at: Point<Pixels>) {
        let menus = self.menus.clone();
        self.visual.update(|window, cx| {
            menus.update(cx, |m, cx| m.open_background_menu(0, Some(at), window, cx));
        });
        self.frame();
    }

    fn open_folder(&mut self, path: &str, at: Point<Pixels>) {
        let menus = self.menus.clone();
        self.visual.update(|window, cx| {
            menus.update(cx, |m, cx| m.open_folder_menu(path, at, window, cx));
        });
        self.frame();
    }

    fn is_open(&mut self) -> bool {
        self.menus.read_with(&self.visual, |m, _| m.is_open())
    }

    fn view(&mut self) -> Entity<MenuView> {
        self.menus
            .read_with(&self.visual, |m, _| m.view().cloned())
            .expect("a menu is open")
    }

    fn nodes(&mut self) -> Vec<Node> {
        let view = self.view();
        view.read_with(&self.visual, |v, _| v.nodes().to_vec())
    }

    fn selected(&mut self) -> Option<String> {
        let view = self.view();
        view.read_with(&self.visual, |v, _| v.selected_label().map(|s| s.to_string()))
    }

    fn depth(&mut self) -> usize {
        let view = self.view();
        view.read_with(&self.visual, |v, _| v.depth())
    }

    fn press(&mut self, key: &str) {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        self.frame();
    }

    /// Clicks the row, submenu or strip button with this label.
    fn click(&mut self, label: &str) {
        let nodes = self.nodes();
        let id: ElementId = nodes
            .iter()
            .enumerate()
            .find_map(|(i, node)| match node {
                Node::Item(e) if e.label == label => Some(ElementId::from(("menu-row", i))),
                Node::Menu { label: l, .. } if l == label => Some(ElementId::from(("menu-row", i))),
                Node::Strip(entries) => entries
                    .iter()
                    .position(|e| e.label == label)
                    .map(|j| ElementId::from(("menu-strip-button", i * 32 + j))),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no {label:?} in {:?}", labels(&nodes)));
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        });
        self.frame();
    }

    fn logged(&self) -> Vec<&'static str> {
        self.log.borrow().clone()
    }

    /// What the window was asked to do (not the menu appearing and going).
    fn events(&self) -> Vec<MenuEvent> {
        self.events
            .borrow()
            .iter()
            .filter(|e| !matches!(e, MenuEvent::Shown | MenuEvent::Closed))
            .cloned()
            .collect()
    }

    /// Whether the menu covered the window after each change: true when shown, false when closed.
    fn covers(&self) -> Vec<bool> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e {
                MenuEvent::Shown => Some(true),
                MenuEvent::Closed => Some(false),
                _ => None,
            })
            .collect()
    }

    fn menu_bounds(&mut self, name: &str) -> Bounds<Pixels> {
        self.visual.update(|window, cx| {
            window.render_frame(cx);
            window.find_by_label(name).bounds()
        })
    }

    fn list_has_keyboard(&mut self) -> bool {
        let shell = self.shell.clone();
        self.visual.update(|window, cx| {
            let list = shell.read(cx).list.clone();
            list.is_focused(window)
        })
    }
}

fn at(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

const ALPHA: &str = "C:\\x\\alpha.txt";
const DOCS: &str = "C:\\x\\docs";

#[gpui_kit::test]
fn a_right_click_opens_the_item_menu_at_the_pointer_and_escape_closes_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA]);
    ui.open_menu(Some(ALPHA), Some(at(200., 10.)), false);
    assert!(ui.is_open());
    assert_eq!(labels(&ui.nodes())[1], "Open");
    let bounds = ui.menu_bounds("Item menu");
    assert_eq!(bounds.origin.x, px(200.), "its start edge is at the pointer");
    // Every row is a named menu item for a screen reader.
    let named = ui
        .visual
        .update(|window, _| window.find_all_by_role(Role::MenuItem).len());
    assert!(named > 10, "{named}");
    assert_eq!(
        ui.visual.update(|window, _| window.find_all_by_role(Role::Menu).len()),
        1
    );
    assert_eq!(
        ui.covers(),
        [true],
        "the window hides a native preview while the menu is over it"
    );
    ui.press("escape");
    assert!(!ui.is_open());
    assert_eq!(ui.covers(), [true, false]);
    assert!(ui.list_has_keyboard(), "the keyboard goes back to the list");
    assert!(ui.logged().is_empty());
}

#[gpui_kit::test]
fn a_press_outside_the_menu_closes_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA]);
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    assert!(ui.is_open());
    ui.visual.simulate_click(at(950., 650.), Modifiers::default());
    ui.frame();
    assert!(!ui.is_open());
    assert!(ui.logged().is_empty(), "nothing was chosen");
}

#[gpui_kit::test]
fn the_menu_key_opens_the_selections_menu_and_the_arrows_enter_and_escape_work_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA]);
    // The keyboard gives the item it is on and no point.
    ui.open_menu(Some(ALPHA), None, false);
    assert!(ui.is_open());
    assert_eq!(ui.selected().as_deref(), Some("Open"), "the first row past the strip");
    ui.press("down");
    assert_eq!(ui.selected().as_deref(), Some("Quick Look"));
    ui.press("up");
    ui.press("up");
    assert_eq!(
        ui.selected().as_deref(),
        Some("Delete"),
        "up goes into the strip and round the end"
    );
    ui.press("left");
    assert_eq!(
        ui.selected().as_deref(),
        Some("Copy path"),
        "the arrows move along the strip"
    );
    ui.press("right");
    assert_eq!(ui.selected().as_deref(), Some("Delete"));
    ui.press("end");
    assert_eq!(ui.selected().as_deref(), Some("Show more options"));
    ui.press("home");
    assert_eq!(ui.selected().as_deref(), Some("Cut"));
    ui.press("down");
    ui.press("down");
    ui.press("down");
    ui.press("down");
    ui.press("down");
    assert_eq!(ui.selected().as_deref(), Some("Open"));
    ui.press("enter");
    assert!(!ui.is_open());
    assert_eq!(
        ui.logged(),
        ["OpenItem"],
        "the command goes to the list that had the keyboard"
    );
    assert!(ui.list_has_keyboard());
}

#[gpui_kit::test]
fn with_nothing_selected_the_keyboard_gets_the_folders_menu(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.open_menu(None, None, false);
    assert_eq!(labels(&ui.nodes())[0], "[Paste|Undo|Copy path|Pin to sidebar|Refresh]");
    ui.press("escape");
    // A right-click on empty space clears the selection first, so it gets the same.
    ui.select(&[]);
    ui.open_menu(None, Some(at(300., 10.)), false);
    assert!(find(&ui.nodes(), "New folder").is_some());
}

#[gpui_kit::test]
fn a_menu_not_on_the_selected_item_is_for_that_item_alone(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA, "C:\\x\\beta.md"]);
    ui.open_menu(Some(DOCS), Some(at(100., 10.)), false);
    assert_eq!(
        labels(&ui.nodes())[0],
        "[Cut|Copy|Paste into folder|Rename|Copy path|Pin to sidebar|Delete]"
    );
    ui.press("escape");
    // On one of several selected, it is for all of them.
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    assert_eq!(labels(&ui.nodes())[0], "[Cut|Copy|Rename 2 items…|Copy paths|Delete]");
}

#[gpui_kit::test]
fn clicking_a_strip_button_dispatches_its_command_and_closes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA]);
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    ui.click("Copy path");
    assert_eq!(ui.logged(), ["CopyPath"]);
    assert!(!ui.is_open());
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    ui.click("Delete");
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    ui.click("Rename");
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    ui.click("Cut");
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    ui.click("Copy");
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    ui.click("Properties");
    assert_eq!(
        ui.logged(),
        ["CopyPath", "Delete", "Rename", "Cut", "Copy", "Properties"]
    );
    assert!(
        ui.events().is_empty(),
        "file operations are the workspace's own commands, not events"
    );
}

#[gpui_kit::test]
fn a_submenu_replaces_the_menu_with_a_back_row_and_choosing_in_it_tells_the_window(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA]);
    ui.open_menu(Some(ALPHA), Some(at(100., 10.)), false);
    ui.click("Compress to");
    assert_eq!(ui.depth(), 2);
    assert_eq!(labels(&ui.nodes())[0], "Zip file");
    assert_eq!(ui.selected().as_deref(), Some("Zip file"), "the first row, not Back");
    ui.press("up");
    assert_eq!(
        ui.selected().as_deref(),
        Some("Compress to"),
        "the Back row is named for the menu it leaves"
    );
    ui.press("down");
    assert_eq!(ui.selected().as_deref(), Some("Zip file"));
    // The arrow toward the start goes back; toward the end goes in again.
    ui.press("left");
    assert_eq!(ui.depth(), 1);
    assert_eq!(ui.selected().as_deref(), Some("Compress to"));
    ui.press("right");
    assert_eq!(ui.depth(), 2);
    ui.press("backspace");
    assert_eq!(ui.depth(), 1);
    ui.click("Compress to");
    ui.click("7z file");
    assert!(!ui.is_open());
    assert_eq!(
        ui.events(),
        [MenuEvent::Compress {
            paths: vec![ALPHA.to_string()],
            format: CompressFormat::SevenZ,
        }]
    );
}

#[gpui_kit::test]
fn open_in_new_tab_opens_a_tab(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[DOCS]);
    let tabs = |ui: &mut Ui| ui.explorer.read_with(&ui.visual, |e, _| e.panes()[0].tabs.len());
    assert_eq!(tabs(&mut ui), 1);
    ui.open_menu(Some(DOCS), Some(at(100., 10.)), false);
    ui.click("Open in new tab");
    assert_eq!(tabs(&mut ui), 2, "a tab was opened");
    assert!(!ui.is_open());
}

#[gpui_kit::test]
fn open_side_by_side_turns_on_two_panes(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[DOCS]);
    ui.open_menu(Some(DOCS), Some(at(100., 10.)), false);
    ui.click("Open side by side");
    ui.frame();
    let dual = ui.explorer.read_with(&ui.visual, |e, _| e.is_dual());
    assert!(dual);
}

#[gpui_kit::test]
fn a_folders_menu_from_a_tile_opens_it_here(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.open_folder(DOCS, at(100., 10.));
    assert_eq!(labels(&ui.nodes())[0], "[Paste into folder|Copy path|Pin to sidebar]");
    ui.click("Open");
    ui.frame();
    let path = ui
        .explorer
        .read_with(&ui.visual, |e, _| e.active_tab().map(|t| t.path.clone()));
    assert_eq!(path.as_deref(), Some(DOCS));
}

#[gpui_kit::test]
fn the_folder_shown_dispatches_new_folder_and_select_all_and_tells_the_window_the_rest(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.open_background(at(100., 10.));
    ui.click("New folder");
    ui.open_background(at(100., 10.));
    ui.click("Select all");
    ui.open_background(at(100., 10.));
    ui.click("Paste");
    assert_eq!(ui.logged(), ["NewFolder", "SelectAll", "Paste"]);
    ui.open_background(at(100., 10.));
    ui.click("Folder sizes");
    ui.open_background(at(100., 10.));
    ui.click("Paste with the copy queue");
    ui.open_background(at(100., 10.));
    ui.click("Clean up & share");
    ui.click("Find duplicate files");
    assert_eq!(
        ui.events(),
        [
            MenuEvent::FolderSizes,
            MenuEvent::PasteQueued { folder: "C:\\x".into() },
            MenuEvent::Duplicates { folder: "C:\\x".into() },
        ]
    );
}

#[gpui_kit::test]
fn shift_right_click_shows_windows_own_menu_at_once_and_show_more_options_does_too(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA]);
    ui.open_menu(Some(ALPHA), Some(at(210., 40.)), true);
    assert!(!ui.is_open(), "no menu of ours");
    let calls = ui.menus.read_with(&ui.visual, |m, _| m.shell_calls.borrow().clone());
    assert_eq!(calls, [(vec![ALPHA.to_string()], at(210., 40.))]);
    // From the menu: the same, at the point the menu was opened.
    ui.open_menu(Some(ALPHA), Some(at(120., 30.)), false);
    ui.click("Show more options");
    assert!(!ui.is_open());
    let calls = ui.menus.read_with(&ui.visual, |m, _| m.shell_calls.borrow().clone());
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1], (vec![ALPHA.to_string()], at(120., 30.)));
    // A zip on disk is an ordinary file for Windows.
    ui.select(&["C:\\x\\pack.zip"]);
    ui.open_menu(Some("C:\\x\\pack.zip"), Some(at(10., 10.)), true);
    assert!(!ui.is_open());
    assert_eq!(ui.menus.read_with(&ui.visual, |m, _| m.shell_calls.borrow().len()), 3);
}

#[gpui_kit::test]
fn a_menu_asks_the_window_for_what_it_cant_do_itself(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&["C:\\x\\pack.zip"]);
    ui.open_menu(Some("C:\\x\\pack.zip"), Some(at(100., 10.)), false);
    ui.click("Extract here");
    assert_eq!(
        ui.events(),
        [MenuEvent::ExtractHere {
            path: "C:\\x\\pack.zip".into()
        }]
    );
}

#[gpui_kit::test]
fn the_arabic_menu_opens_toward_the_start_and_mirrors_its_rows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    testing::set_rtl(false);
    let mut ui = open(cx, &disk(), "C:\\x");
    let pdf = "C:\\x\\تقرير الربع 1.pdf";
    ui.select(&[pdf]);
    // English: the start edge is at the pointer, Cut is before Delete along the strip.
    ui.open_menu(Some(pdf), Some(at(500., 10.)), false);
    let menu = ui.menu_bounds("Item menu");
    assert_eq!(menu.origin.x, px(500.));
    let (cut, delete) = ui.visual.update(|window, _| {
        (
            window.find_by_label("Cut").bounds().origin.x,
            window.find_by_label("Delete").bounds().origin.x,
        )
    });
    assert!(cut < delete);
    ui.press("escape");

    testing::set_rtl(true);
    ui.open_menu(Some(pdf), Some(at(500., 10.)), false);
    let menu = ui.menu_bounds("Item menu");
    // The Arabic menu's end edge meets the pointer: it opens toward the start.
    let right = menu.origin.x + menu.size.width;
    assert!((right - px(500.)).abs() < px(1.), "right edge at {right:?}");
    let (cut, delete) = ui.visual.update(|window, _| {
        (
            window.find_by_label(&t!("Cut")).bounds().origin.x,
            window.find_by_label(&t!("Delete")).bounds().origin.x,
        )
    });
    assert!(cut > delete, "in Arabic the strip starts at the right");
    // The rows are in Arabic, and keys work as they do in English: toward the end is Left.
    assert_eq!(ui.selected().as_deref(), Some("فتح"));
    ui.press("down");
    assert_eq!(ui.selected().as_deref(), Some(t!("Quick Look").as_ref()));
    ui.visual.update(|window, _| {
        assert!(window.try_find_by_label("فتح").is_some());
    });
    // The submenu arrow mirrors: Right goes back, Left goes in.
    ui.click(&t!("Compress to"));
    assert_eq!(ui.depth(), 2);
    ui.press("right");
    assert_eq!(ui.depth(), 1);
    ui.press("left");
    assert_eq!(ui.depth(), 2);
    testing::set_rtl(false);
}

#[gpui_kit::test]
fn a_menu_taller_than_the_window_still_fits_in_it(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.select(&[ALPHA]);
    ui.open_menu(Some(ALPHA), Some(at(100., 600.)), false);
    let bounds = ui.menu_bounds("Item menu");
    assert!(
        bounds.origin.y >= px(0.) && bounds.origin.y + bounds.size.height <= px(700.) + px(1.),
        "{bounds:?}"
    );
}

#[gpui_kit::test]
fn the_folder_menus_undo_says_what_the_explorer_would_take_back(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut ui = open(cx, &disk(), "C:\\x");
    ui.open_background(at(100., 10.));
    assert_eq!(
        find(&ui.nodes(), "Undo").map(|e| e.disabled),
        Some(true),
        "nothing to take back yet"
    );
    ui.press("escape");
    let explorer = ui.explorer.clone();
    ui.visual.update(|_, cx| {
        explorer.update(cx, |e, cx| {
            e.remember_undo("copy of 2 items", explorer::UndoAction::Recycle(Vec::new()), cx);
        })
    });
    ui.open_background(at(100., 10.));
    assert_eq!(
        find(&ui.nodes(), "Undo the copy of 2 items").map(|e| e.disabled),
        Some(false)
    );
    ui.click("Undo the copy of 2 items");
    assert_eq!(ui.logged(), ["Undo"], "Ctrl+Z's own command does it, with its notice");
}
