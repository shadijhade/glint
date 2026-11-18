//! Tests of the head and Suggested as data. Card E2 of docs/gpui/aware writes these.

use super::head::{folder_head, folder_head_with, head_of, Extras};
use super::smart::{dress, keys, reason, row_for, zone};
use super::tests::{ctx, english, find, labels, text};
use super::*;
use crate::explorer::fake::{file, folder};
use glint_model::aware::{Algo, Inside, Media, Origin, Pick, Probed, Shape, Signer, Text, Verdict, Why};
use glint_model::entries::attr;

fn pick(key: &str, why: Why) -> Pick {
    Pick {
        key: key.into(),
        why,
        score: 1.0,
    }
}

fn plain_item(label: &str, run: Run) -> Node {
    Node::Item(Entry::new(label.to_string().into(), IconName::File, run))
}

fn extract() -> Run {
    Run::Ask(MenuEvent::ExtractHere { path: "C:\\x\\a.zip".into() })
}

/// Every row of a menu that can be chosen: strip buttons, items, and the rows of submenus that aren't disabled.
fn chosen_rows(nodes: &[Node], out: &mut Vec<Entry>) {
    for node in nodes {
        match node {
            Node::Item(e) if !e.disabled => out.push(e.clone()),
            Node::Strip(entries) => out.extend(entries.iter().filter(|e| !e.disabled).cloned()),
            Node::Menu {
                items, disabled: false, ..
            } => chosen_rows(items, out),
            _ => {}
        }
    }
}

// ---- Run::key and keys ----------------------------------------------------------------------------------

#[test]
fn a_rows_key_is_its_command_task_or_event_in_kebab_case() {
    assert_eq!(Run::Command(Command::Copy).key().as_deref(), Some("cmd:copy"));
    let open_in = Run::Do(Task::OpenIn {
        opener: "code".into(),
        name: "VS Code".into(),
        path: "C:\\x".into(),
    });
    assert_eq!(open_in.key().as_deref(), Some("task:open-in:code"));
    let compress = Run::Ask(MenuEvent::Compress {
        paths: vec![],
        format: CompressFormat::SevenZ,
    });
    assert_eq!(compress.key().as_deref(), Some("do:compress:7z"));
    let merge = Run::Ask(MenuEvent::PdfTools {
        paths: vec![],
        tool: PdfTool::Merge,
    });
    assert_eq!(merge.key().as_deref(), Some("do:pdf:merge"));
    let move_other = Run::Ask(MenuEvent::CopyToOtherPane {
        paths: vec![],
        moving: true,
    });
    assert_eq!(move_other.key().as_deref(), Some("do:move-other"));
    assert_eq!(Run::Nothing.key(), None);
    assert_eq!(Run::Ask(MenuEvent::Shown).key(), None);
    assert_eq!(Run::Ask(MenuEvent::Closed).key(), None);
    let new_of = Run::Ask(MenuEvent::NewOfKind { ext: ".DOCX".into() });
    assert_eq!(new_of.key().as_deref(), Some("do:new:.docx"));
    let script = Run::Ask(MenuEvent::RunScript {
        script: "C:\\Scripts\\Tidy.PS1".into(),
        cwd: "C:\\x".into(),
        paths: vec![],
    });
    assert_eq!(script.key().as_deref(), Some("do:script:tidy.ps1"));
}

#[test]
fn every_row_that_can_be_chosen_has_a_key_and_two_rows_share_one_only_when_they_are_the_same_action() {
    let _guard = english();
    let menus = [
        item_menu(&[text("alpha.txt")], &ctx()),
        item_menu(&[text("pack.zip")], &ctx()),
        item_menu(&[folder("C:\\x", "docs")], &ctx()),
        item_menu(&[text("a.txt"), text("b.txt")], &ctx()),
    ];
    for menu in &menus {
        let mut rows = Vec::new();
        chosen_rows(menu, &mut rows);
        let mut seen: Vec<(String, Run)> = Vec::new();
        for row in &rows {
            let key = row.run.key().unwrap_or_else(|| panic!("{:?} has no key", row.label));
            if let Some((_, other)) = seen.iter().find(|(k, _)| *k == key) {
                // The same key twice is the same action (a row in the strip and in the list, say), or one kind of
                // action on different things: PLAN.md 5.5 gives every tag colour `do:tags`.
                if key != "do:tags" && key != "do:open-with-editor" {
                    assert_eq!(*other, row.run, "{key} is two different actions");
                }
            } else {
                seen.push((key, row.run.clone()));
            }
        }
        assert_eq!(keys(menu).len(), seen.len());
    }
}

#[test]
fn keys_skip_disabled_rows_and_disabled_submenus_and_list_a_submenus_rows() {
    let mut off = Entry::task("Off".into(), IconName::File, Task::RefreshDrives);
    off.disabled = true;
    let nodes = vec![
        plain_item("Refresh", Run::Do(Task::RefreshDrives)),
        Node::Item(off),
        Node::Menu {
            label: "Open in".into(),
            icon: None,
            disabled: false,
            items: vec![plain_item("Code", Run::Do(Task::OpenTerminal("C:\\x".into())))],
        },
        Node::Menu {
            label: "Closed".into(),
            icon: None,
            disabled: true,
            items: vec![plain_item("Mount", Run::Ask(MenuEvent::Mount { path: "C:\\x".into() }))],
        },
        Node::Heading("H".into()),
        Node::Separator,
        plain_item("Again", Run::Do(Task::RefreshDrives)),
        plain_item("Note", Run::Nothing),
    ];
    assert_eq!(keys(&nodes), ["task:refresh-drives", "task:open-terminal"]);
}

#[test]
fn a_row_for_a_key_stands_alone_and_says_which_menu_it_is_from() {
    let _guard = english();
    let menu = item_menu(&[text("pack.zip")], &ctx());
    let extract = row_for(&menu, "do:extract-here").expect("extract here");
    assert_eq!(extract.label, "Extract here");
    assert!(extract.smart);
    assert_eq!(extract.check, None);
    let zip = row_for(&menu, "do:compress:zip").expect("zip");
    assert_eq!(zip.label, "Compress to › Zip file");
    assert!(zip.smart && zip.reason.is_none());
    assert!(row_for(&menu, "do:not-a-row").is_none());
    // A row nested two deep says both menus.
    let deep = vec![Node::menu(
        "A".into(),
        IconName::File,
        vec![Node::menu(
            "B".into(),
            IconName::File,
            vec![plain_item("C", Run::Do(Task::RefreshDrives)).into()],
        )
        .into()],
    )];
    assert_eq!(row_for(&deep, "task:refresh-drives").map(|r| r.label.to_string()).as_deref(), Some("A › B › C"));
    // Its check mark is dropped, its warning color and key text are kept.
    let mut checked = Entry::task("On".into(), IconName::File, Task::RefreshDrives).checked(true).danger(true);
    checked.hint = Some("Ctrl+K".into());
    let row = row_for(&[Node::Item(checked)], "task:refresh-drives").expect("row");
    assert_eq!((row.check, row.danger, row.hint.as_deref()), (None, true, Some("Ctrl+K")));
}

// ---- reason and zone ------------------------------------------------------------------------------------

#[test]
fn a_reason_is_said_in_the_words_of_the_table() {
    let _guard = english();
    let say = |why: Why, shape: Shape| reason(&why, shape).map(|s| s.to_string());
    assert_eq!(say(Why::Plain, Shape::File), None);
    assert_eq!(say(Why::UsualPlace, Shape::File), None);
    assert_eq!(say(Why::InUse("Word".into()), Shape::File).as_deref(), Some("In use by Word"));
    assert_eq!(
        say(Why::FromInternet("github.com".into()), Shape::File).as_deref(),
        Some("It came from github.com")
    );
    assert_eq!(
        say(Why::FromInternet(String::new()), Shape::File).as_deref(),
        Some("It came from the internet")
    );
    assert_eq!(say(Why::Unsigned, Shape::Program).as_deref(), Some("It isn't signed"));
    assert_eq!(
        say(Why::BuildOutput(1_932_735_283), Shape::Folder).as_deref(),
        Some("1.8 GB of build output inside")
    );
    assert_eq!(
        say(Why::Usual, Shape::Pictures).as_deref(),
        Some("You often do this with pictures")
    );
    assert_eq!(say(Why::Usual, Shape::Nothing).as_deref(), Some("You often do this here"));
    assert_eq!(say(Why::Usual, Shape::Pdf).as_deref(), Some("You often do this with PDFs"));
    assert_eq!(say(Why::Usual, Shape::Mixed).as_deref(), Some("You often do this with files like these"));
}

fn menu_with_rows() -> Vec<Node> {
    vec![
        plain_item("Extract here", extract()),
        plain_item("Mount", Run::Ask(MenuEvent::Mount { path: "C:\\x".into() })),
        plain_item("Refresh", Run::Do(Task::RefreshDrives)),
        plain_item("Terminal", Run::Do(Task::OpenTerminal("C:\\x".into()))),
    ]
}

fn row_labels(section: &[Node]) -> Vec<String> {
    labels(section)
}

#[test]
fn a_zone_is_a_heading_and_the_picks_rows_with_their_reasons_and_skips_a_pick_with_no_row() {
    let _guard = english();
    let nodes = menu_with_rows();
    let picks = [
        pick("do:extract-here", Why::InUse("Word".into())),
        pick("do:nope", Why::Plain),
        pick("do:mount", Why::Plain),
    ];
    let section = zone(&nodes, &picks, Shape::Archive, Vec::new());
    assert_eq!(row_labels(&section), ["# Suggested", "Extract here", "Mount"]);
    match (&section[1], &section[2]) {
        (Node::Item(a), Node::Item(b)) => {
            assert_eq!(a.reason.as_deref(), Some("In use by Word"));
            assert!(a.smart && b.smart);
            assert_eq!(b.reason, None);
        }
        other => panic!("{other:?}"),
    }
    assert!(zone(&nodes, &[], Shape::Archive, Vec::new()).is_empty());
    assert!(zone(&nodes, &[pick("do:nope", Why::Plain)], Shape::Archive, Vec::new()).is_empty());
}

#[test]
fn a_zone_holds_three_rows_at_most_and_the_extra_row_takes_a_place() {
    let _guard = english();
    let nodes = menu_with_rows();
    let four: Vec<Pick> = ["do:extract-here", "do:mount", "task:refresh-drives", "task:open-terminal"]
        .iter()
        .map(|key| pick(key, Why::Plain))
        .collect();
    assert_eq!(zone(&nodes, &four, Shape::File, Vec::new()).len(), 4, "a heading and three rows");
    let extra = vec![plain_item("Move to Invoices", Run::Do(Task::RefreshDrives))];
    let section = zone(&nodes, &four[..3], Shape::File, extra);
    assert_eq!(
        row_labels(&section),
        ["# Suggested", "Extract here", "Mount", "Move to Invoices"]
    );
}

// ---- dress ----------------------------------------------------------------------------------------------

#[test]
fn a_dressed_menu_has_its_head_then_its_strip_then_suggested_and_keeps_its_own_rows_in_order() {
    let _guard = english();
    let menu = item_menu(&[text("pack.zip")], &ctx());
    let head = head_of(&[text("pack.zip")], None, &Extras::default());
    let picks = [pick("do:extract-here", Why::Plain), pick("do:compress:zip", Why::Usual)];
    let dressed = dress(menu.clone(), head, &picks, Shape::Archive, Vec::new());
    let shown = labels(&dressed);
    assert_eq!(shown[0], "<pack.zip>");
    assert!(shown[1].starts_with('['), "{shown:?}");
    assert_eq!(&shown[2..5], ["# Suggested", "Extract here", "Compress to › Zip file"]);
    assert_eq!(shown[5], "-");
    assert_eq!(shown[6..], labels(&menu)[1..], "the menu's own rows follow, none moved");
    let Node::Item(zip) = &dressed[4] else { panic!("a row") };
    assert_eq!(zip.reason.as_deref(), Some("You often do this with archives"));
}

#[test]
fn with_no_picks_a_dressed_menu_has_a_head_and_nothing_else_added() {
    let _guard = english();
    let menu = item_menu(&[text("alpha.txt")], &ctx());
    let head = head_of(&[text("alpha.txt")], None, &Extras::default());
    let dressed = dress(menu.clone(), head, &[], Shape::File, Vec::new());
    let shown = labels(&dressed);
    assert_eq!(shown[0], "<alpha.txt>");
    assert_eq!(shown[1..], labels(&menu)[..]);
    assert!(!shown.iter().any(|l| l == "# Suggested"));
}

// ---- head_of --------------------------------------------------------------------------------------------

fn line(head: &Head) -> String {
    head.line.to_string()
}

#[test]
fn a_file_with_no_answers_says_its_type_and_size_and_when_it_changed() {
    let _guard = english();
    let entry = file("C:\\x", "notes.txt", 2048, 1_700_000_000_000);
    let head = head_of(std::slice::from_ref(&entry), None, &Extras { now: 1_700_100_000_000, git: None, checksum: None });
    assert_eq!(head.title, "notes.txt");
    assert_eq!(line(&head), format!("{} · 2.0 KB", glint_model::file_types::type_label(&entry)));
    assert!(head.badges.is_empty());
    assert!(head.changed.as_deref().is_some_and(|c| c.starts_with("Changed ")));
    assert!(matches!(head.picture, HeadPicture::Kind(_)));
    let undated = head_of(&[text("a.txt")], None, &Extras::default());
    assert_eq!(undated.changed, None);
}

#[test]
fn answers_add_dimensions_length_lines_and_pages_in_that_order_and_a_binary_file_has_no_lines() {
    let _guard = english();
    let entry = text("clip.dat");
    let all = Probed {
        media: Some(Media {
            width: 1920,
            height: 1080,
            seconds: 151.0,
        }),
        text: Some(Text {
            lines: 1230,
            binary: false,
        }),
        pages: Some(1),
        ..Probed::default()
    };
    let head = head_of(std::slice::from_ref(&entry), Some(&all), &Extras::default());
    let parts: Vec<String> = line(&head).split(" · ").map(String::from).collect();
    assert_eq!(parts.len(), 6, "{parts:?}");
    assert_eq!(parts[2], "1920 × 1080");
    assert_eq!(parts[3], "2 min 31 s");
    assert_eq!(parts[4], "1,230 lines");
    assert_eq!(parts[5], "1 page");
    let binary = Probed {
        text: Some(Text { lines: 9, binary: true }),
        pages: Some(48),
        ..Probed::default()
    };
    let line_of_binary = line(&head_of(std::slice::from_ref(&entry), Some(&binary), &Extras::default()));
    assert!(!line_of_binary.contains("lines") && line_of_binary.ends_with("48 pages"), "{line_of_binary}");
    let one_line = Probed {
        text: Some(Text { lines: 1, binary: false }),
        ..Probed::default()
    };
    assert!(line(&head_of(&[entry], Some(&one_line), &Extras::default())).ends_with("1 line"));
}

fn locked(names: &[&str]) -> Probed {
    Probed {
        locked_by: Some(names.iter().map(|n| n.to_string()).collect()),
        ..Probed::default()
    }
}

#[test]
fn each_badge_alone_has_its_label_tone_and_run() {
    let _guard = english();
    let entry = file("C:\\x", "setup.exe", 10, 0);
    let path = entry.path.clone();
    let badge_of = |entry: &FileEntry, probed: Probed, git: Option<&str>| -> Vec<Badge> {
        let extras = Extras {
            now: 0,
            git: git.map(|g| g.to_string().into()),
            checksum: None,
        };
        head_of(std::slice::from_ref(entry), Some(&probed), &extras).badges
    };

    let b = badge_of(&entry, locked(&["Word", "Excel"]), None);
    assert_eq!(b.len(), 1);
    assert_eq!((b[0].label.as_ref(), b[0].icon, b[0].tone), ("In use by Word", IconName::Lock, Tone::Warn));
    assert_eq!(b[0].tip.as_deref(), Some("Word, Excel"));
    assert_eq!(b[0].run, Run::Ask(MenuEvent::Locks { paths: vec![path.clone()] }));
    assert!(badge_of(&entry, locked(&[]), None).is_empty(), "nobody has it open: nothing to say");

    let from = |host: &str, blocked: bool| Probed {
        origin: Some(Origin {
            host: host.into(),
            blocked,
        }),
        ..Probed::default()
    };
    let b = badge_of(&entry, from("github.com", true), None);
    assert_eq!((b[0].label.as_ref(), b[0].icon, b[0].tone), ("From github.com", IconName::Download, Tone::Warn));
    assert_eq!(b[0].tip.as_deref(), Some("Windows marked it as downloaded"));
    assert_eq!(b[0].run, Run::Ask(MenuEvent::XRay { path: path.clone() }));
    assert_eq!(badge_of(&entry, from("", true), None)[0].label, "From the internet");
    assert!(badge_of(&entry, from("x.com", false), None).is_empty());

    let signed = |signer: Signer| Probed {
        signer: Some(signer),
        ..Probed::default()
    };
    let b = badge_of(&entry, signed(Signer::Unsigned), None);
    assert_eq!((b[0].label.as_ref(), b[0].icon, b[0].tone), ("Not signed", IconName::ShieldAlert, Tone::Warn));
    assert_eq!(b[0].run, Run::Ask(MenuEvent::XRay { path: path.clone() }));
    let b = badge_of(&entry, signed(Signer::Signed("Acme Ltd".into())), None);
    assert_eq!(
        (b[0].label.as_ref(), b[0].icon, b[0].tone),
        ("Signed by Acme Ltd", IconName::ShieldCheck, Tone::Good)
    );

    let twins = |n: usize| Probed {
        twins: Some((0..n).map(|i| format!("C:\\copy{i}\\setup.exe")).collect()),
        ..Probed::default()
    };
    let b = badge_of(&entry, twins(7), None);
    assert_eq!((b[0].label.as_ref(), b[0].icon, b[0].tone), ("7 identical copies", IconName::Files, Tone::Plain));
    assert_eq!(b[0].tip.as_deref().map(|t| t.lines().count()), Some(5));
    assert_eq!(
        b[0].run,
        Run::Ask(MenuEvent::OpenFileLocation {
            path: "C:\\copy0\\setup.exe".into()
        })
    );
    assert_eq!(badge_of(&entry, twins(1), None)[0].label, "1 identical copy");
    assert!(badge_of(&entry, twins(0), None).is_empty());

    let b = badge_of(&entry, Probed::default(), Some("Changed"));
    assert_eq!((b[0].label.as_ref(), b[0].icon, b[0].run.clone()), ("Changed", IconName::GitBranch, Run::Nothing));

    let link = Probed {
        target: Some("C:\\Program Files\\App\\app.exe".into()),
        ..Probed::default()
    };
    let b = badge_of(&entry, link, None);
    assert_eq!((b[0].label.as_ref(), b[0].icon), ("Points to app.exe", IconName::ExternalLink));
    assert_eq!(b[0].tip.as_deref(), Some("C:\\Program Files\\App\\app.exe"));
    assert_eq!(
        b[0].run,
        Run::Ask(MenuEvent::OpenFileLocation {
            path: "C:\\Program Files\\App\\app.exe".into()
        })
    );
}

#[test]
fn badges_for_what_the_entry_itself_says_are_cloud_read_only_and_hidden() {
    let _guard = english();
    let none = Extras::default();
    let word = |entry: FileEntry| -> Vec<String> {
        head_of(&[entry], None, &none).badges.iter().map(|b| b.label.to_string()).collect()
    };
    let online = {
        let mut e = text("a.txt");
        e.attrs |= attr::OFFLINE;
        e
    };
    assert_eq!(word(online), ["Online only"]);
    let kept = {
        let mut e = text("a.txt");
        e.attrs |= attr::PINNED;
        e
    };
    assert_eq!(word(kept), ["Always on this PC"]);
    let ro = {
        let mut e = text("a.txt");
        e.attrs |= attr::READONLY;
        e
    };
    let b = head_of(&[ro], None, &none).badges;
    assert_eq!((b[0].label.as_ref(), b[0].icon, b[0].tone), ("Read-only", IconName::PencilOff, Tone::Plain));
    let hidden = {
        let mut e = text("a.txt");
        e.attrs |= attr::HIDDEN;
        e
    };
    assert_eq!(word(hidden), ["Hidden"]);
    // A folder gets only cloud, hidden and Git: never read-only.
    let mut dir = folder("C:\\x", "docs");
    dir.attrs |= attr::READONLY | attr::HIDDEN;
    assert_eq!(word(dir), ["Hidden"]);
}

#[test]
fn all_the_badges_come_in_the_tables_order() {
    let _guard = english();
    let mut entry = text("link.lnk");
    entry.attrs |= attr::READONLY | attr::HIDDEN | attr::PINNED;
    let probed = Probed {
        locked_by: Some(vec!["Word".into()]),
        origin: Some(Origin {
            host: "a.com".into(),
            blocked: true,
        }),
        signer: Some(Signer::Unsigned),
        twins: Some(vec!["C:\\o\\link.lnk".into()]),
        target: Some("C:\\t\\real.txt".into()),
        ..Probed::default()
    };
    let extras = Extras {
        now: 0,
        git: Some("New".into()),
        checksum: None,
    };
    let head = head_of(&[entry], Some(&probed), &extras);
    let got: Vec<&str> = head.badges.iter().map(|b| b.label.as_ref()).collect();
    assert_eq!(
        got,
        [
            "In use by Word",
            "From a.com",
            "Not signed",
            "1 identical copy",
            "Always on this PC",
            "Read-only",
            "Hidden",
            "Points to real.txt",
            "New"
        ]
    );
}

#[test]
fn a_folder_says_folder_and_what_is_inside_only_once_it_is_counted() {
    let _guard = english();
    let dir = folder("C:\\x", "docs");
    let none = Extras::default();
    let head = head_of(std::slice::from_ref(&dir), None, &none);
    assert_eq!((line(&head), head.picture.clone()), ("Folder".to_string(), HeadPicture::Folder));
    let inside = |done: bool| Probed {
        inside: Some(Inside {
            files: 1204,
            folders: 3,
            bytes: 3_435_973_837,
            junk: 0,
            done,
            from_index: true,
        }),
        ..Probed::default()
    };
    assert_eq!(line(&head_of(std::slice::from_ref(&dir), Some(&inside(true)), &none)), "Folder · 1,204 files · 3.2 GB");
    assert_eq!(line(&head_of(&[dir], Some(&inside(false)), &none)), "Folder");
}

#[test]
fn several_items_are_counted_and_weighed_by_their_files() {
    let _guard = english();
    let none = Extras::default();
    let targets = [
        file("C:\\x", "a.txt", 1024, 0),
        file("C:\\x", "b.txt", 1024, 0),
        file("C:\\x", "c.txt", 2048, 0),
        folder("C:\\x", "docs"),
    ];
    let head = head_of(&targets, None, &none);
    assert_eq!(head.title, "4 items");
    assert_eq!(line(&head), "3 files, 1 folder · 4.0 KB");
    assert_eq!(head.picture, HeadPicture::Several);
    assert!(head.badges.is_empty() && head.changed.is_none());
    let two = head_of(&[folder("C:\\x", "a"), folder("C:\\x", "b")], None, &none);
    assert_eq!((two.title.to_string(), line(&two)), ("2 items".to_string(), "2 folders".to_string()));
}

#[test]
fn an_archives_menu_has_open_extracted_folder_before_extract_here_only_once_it_was_extracted() {
    let _guard = english();
    let zip = file("C:\\x", "pack.zip", 5000, 0);
    let mut c = ctx();
    let plain = labels(&item_menu(std::slice::from_ref(&zip), &c));
    assert!(!plain.iter().any(|l| l == "Open extracted folder"), "{plain:?}");
    c.probed = Some(Probed { extracted: Some(Some("C:\\x\\pack".into())), ..Probed::default() });
    let with = labels(&item_menu(std::slice::from_ref(&zip), &c));
    let place = |label: &str| with.iter().position(|l| l == label).unwrap_or_else(|| panic!("no {label:?} in {with:?}"));
    assert!(place("Open extracted folder") < place("Extract here"));
    c.probed = Some(Probed { extracted: Some(None), ..Probed::default() });
    assert!(!labels(&item_menu(std::slice::from_ref(&zip), &c)).iter().any(|l| l == "Open extracted folder"));
}

#[test]
fn a_file_that_matches_the_copied_checksum_says_so_and_one_that_does_not_warns() {
    let _guard = english();
    let entry = file("C:\\x", "setup.exe", 5000, 0);
    let badges = |verdict: Option<Verdict>| {
        let extras = Extras { now: 0, git: None, checksum: verdict };
        head_of(std::slice::from_ref(&entry), Some(&Probed::default()), &extras).badges
    };
    let matches = badges(Some(Verdict::Match(Algo::Sha256)));
    assert_eq!(matches.len(), 1);
    assert_eq!((matches[0].label.as_ref(), matches[0].tone), ("Matches copied SHA-256", Tone::Good));
    assert_eq!(badges(Some(Verdict::Match(Algo::Blake3)))[0].label, "Matches copied BLAKE3");
    let wrong = badges(Some(Verdict::Mismatch(Algo::Sha256)));
    assert_eq!((wrong[0].label.as_ref(), wrong[0].tone), ("Doesn't match the copied SHA-256", Tone::Warn));
    assert_eq!(badges(Some(Verdict::Mismatch(Algo::Md5)))[0].label, "Doesn't match the copied MD5");
    assert!(badges(None).is_empty(), "unknown is nothing at all");
}

#[test]
fn a_checksum_files_menu_has_check_files_in_this_folder_only_for_the_kinds_that_can_be_checked() {
    let _guard = english();
    let has = |name: &str| keys(&item_menu(&[text(name)], &ctx())).iter().any(|k| k == "do:check-sums");
    assert!(has("pack.sha256") && has("PACK.SHA512") && has("sums.b3sum"));
    assert!(has("pack.md5") && has("pack.sha1") && has("pack.md5sum") && has("pack.sha1sum"));
    assert!(!has("notes.txt") && !has("SHA256SUMS"));
    let mut remote = text("pack.sha256");
    remote.attrs |= attr::REMOTE;
    assert!(!keys(&item_menu(&[remote], &ctx())).iter().any(|k| k == "do:check-sums"), "not on a remote");
}

#[test]
fn an_archive_that_was_extracted_says_so_in_its_head_and_the_badge_opens_the_folder() {
    let _guard = english();
    let zip = file("C:\\x", "pack.zip", 5000, 0);
    let extras = Extras { now: 0, git: None, checksum: None };
    let known = |extracted| Probed { extracted: Some(extracted), ..Probed::default() };
    let folder = "C:\\x\\pack".to_string();
    let head = head_of(std::slice::from_ref(&zip), Some(&known(Some(folder.clone()))), &extras);
    let badge = head.badges.iter().find(|b| b.label == "Already extracted").expect("a badge");
    assert_eq!(badge.run, Run::Do(Task::OpenExtracted(folder.clone())));
    assert_eq!(badge.tip.as_deref(), Some(folder.as_str()));
    assert!(head_of(std::slice::from_ref(&zip), Some(&known(None)), &extras).badges.is_empty());
    assert!(head_of(std::slice::from_ref(&zip), None, &extras).badges.is_empty());
}

#[test]
fn a_folders_own_head_adds_its_whole_size_once_it_is_counted() {
    let _guard = english();
    let inside = |bytes, done| Probed {
        inside: Some(Inside { files: 9, folders: 2, bytes, junk: 0, done, from_index: false }),
        ..Probed::default()
    };
    let head = |probed: Option<&Probed>| folder_head_with("P".into(), Some(25), None, probed);
    assert_eq!(line(&head(None)), "25 items");
    assert_eq!(line(&head(Some(&inside(3 * 1024 * 1024, false)))), "25 items", "not while it is still counting");
    assert_eq!(line(&head(Some(&inside(3 * 1024 * 1024, true)))), "25 items · 3.0 MB");
    assert_eq!(line(&head(Some(&inside(0, true)))), "25 items");
    assert_eq!(line(&folder_head_with("P".into(), None, None, Some(&inside(1024, true)))), "1.0 KB");
}

#[test]
fn a_folders_own_head_has_its_name_its_item_count_and_its_branch() {
    let _guard = english();
    let head = folder_head("Projects".into(), Some(25), None);
    assert_eq!((head.title.as_ref(), line(&head).as_str()), ("Projects", "25 items"));
    assert_eq!(head.picture, HeadPicture::Folder);
    assert!(head.badges.is_empty());
    assert_eq!(line(&folder_head("P".into(), Some(1), None)), "1 item");
    assert_eq!(line(&folder_head("P".into(), None, None)), "");
    let branch = folder_head("P".into(), Some(2), Some("main".into()));
    assert_eq!(branch.badges.len(), 1);
    let b = &branch.badges[0];
    assert_eq!((b.label.as_ref(), b.icon, b.tone, b.run.clone()), ("main", IconName::GitBranch, Tone::Plain, Run::Nothing));
    let _ = find(&[], "x");
}
