//! The commands that do things to files, the tools, and the saved searches: each is listed exactly when it
//! applies, and choosing it gives the run its menu item would.

use super::super::commands::{actions, all, Command, Inputs, Run};
use crate::actions as keys;
use crate::components::menus::{CompressFormat, MediaTool, MenuEvent, PdfTool};
use crate::explorer::fake::{file, folder};
use glint_core::openers::Opener;
use glint_core::projects::{Task, TaskGroup};
use glint_core::scripts::Script;
use glint_model::store::SavedSearch;

const HERE: &str = "C:\\work";

/// Inputs for a writable folder with `names` selected (files, or folders when they end in `/`).
fn with_selected(names: &[&str]) -> Inputs {
    let entries: Vec<_> = names
        .iter()
        .map(|name| match name.strip_suffix('/') {
            Some(dir) => folder(HERE, dir),
            None => file(HERE, name, 10, 0),
        })
        .collect();
    Inputs {
        path: HERE.into(),
        tabs: 1,
        chosen: !entries.is_empty(),
        selected: entries.iter().map(|e| e.path.clone()).collect(),
        chosen_entries: entries,
        ..Inputs::default()
    }
}

fn find<'a>(commands: &'a [Command], id: &str) -> Option<&'a Command> {
    commands.iter().find(|c| c.id == id)
}

fn menu_event(command: &Command) -> MenuEvent {
    match &command.run {
        Run::Menu(event) => event.clone(),
        other => panic!("{} should be a menu item, not {other:?}", command.id),
    }
}

fn path(name: &str) -> String {
    format!("{HERE}\\{name}")
}

#[test]
fn saved_searches_are_places_that_run_the_search_in_their_folder() {
    let mut inputs = with_selected(&[]);
    inputs.saved_searches = vec![
        SavedSearch {
            query: "invoice".into(),
            folder: "D:\\Docs".into(),
        },
        SavedSearch {
            query: "holiday".into(),
            folder: String::new(),
        },
    ];
    let commands = all(&inputs);
    let one = find(&commands, "search:D:\\Docs\ninvoice").expect("a saved search is listed");
    assert_eq!(one.label.as_ref(), "invoice");
    assert_eq!(one.detail.as_deref(), Some("Saved search in D:\\Docs"));
    assert!(matches!(&one.run, Run::Saved(s) if s.query == "invoice" && s.folder == "D:\\Docs"));
    let everywhere = find(&commands, "search:every drive\nholiday").expect("one for every drive");
    assert_eq!(everywhere.detail.as_deref(), Some("Saved search in every drive"));
    assert!(matches!(&everywhere.run, Run::Saved(s) if s.folder.is_empty()));
}

#[test]
fn emptying_the_recycle_bin_is_always_offered_and_asks_the_window_to_confirm() {
    for path in [HERE, "", "recycle-bin:"] {
        let inputs = Inputs {
            path: path.into(),
            ..Inputs::default()
        };
        let commands = actions(&inputs);
        let empty = find(&commands, "empty-bin").expect("listed everywhere");
        assert_eq!(menu_event(empty), MenuEvent::EmptyRecycleBin);
    }
}

#[test]
fn a_video_is_offered_the_media_tools_and_a_pdf_is_not() {
    let commands = actions(&with_selected(&["clip.mp4"]));
    let p = path("clip.mp4");
    assert_eq!(
        menu_event(find(&commands, "media-convert").unwrap()),
        MenuEvent::Media {
            paths: vec![p.clone()],
            tool: MediaTool::Convert
        }
    );
    for id in [
        "media-gif",
        "media-audio",
        "media-subtitles",
        "media-volume",
        "shrink",
        "encrypt",
    ] {
        assert!(find(&commands, id).is_some(), "{id} applies to a video");
    }
    assert_eq!(
        menu_event(find(&commands, "compress").unwrap()),
        MenuEvent::Compress {
            paths: vec![p],
            format: CompressFormat::Zip
        }
    );
    for id in [
        "pdf-tools",
        "pdf-merge",
        "convert",
        "convert-data",
        "song-details",
        "decrypt",
    ] {
        assert!(find(&commands, id).is_none(), "{id} does not apply to a video");
    }
}

#[test]
fn nothing_selected_offers_none_of_the_selection_commands() {
    let commands = actions(&with_selected(&[]));
    for id in ["compress", "shrink", "encrypt", "convert-doc", "copy-other"] {
        assert!(find(&commands, id).is_none(), "{id}");
    }
    // With nothing chosen the folder is the subject.
    assert_eq!(
        menu_event(find(&commands, "who-locks").unwrap()),
        MenuEvent::Locks {
            paths: vec![HERE.into()]
        }
    );
    assert_eq!(
        find(&commands, "defender").unwrap().label.as_ref(),
        "Scan this folder with Microsoft Defender"
    );
}

#[test]
fn merging_pdfs_needs_two_and_data_queries_need_two() {
    let one = actions(&with_selected(&["a.pdf", "a.csv"]));
    assert!(find(&one, "pdf-tools").is_some() && find(&one, "convert-data").is_some());
    assert!(find(&one, "pdf-merge").is_none() && find(&one, "query-together").is_none());
    let two = actions(&with_selected(&["a.pdf", "b.pdf", "a.csv", "b.csv"]));
    assert_eq!(
        menu_event(find(&two, "pdf-merge").unwrap()),
        MenuEvent::PdfTools {
            paths: ["a.pdf", "b.pdf", "a.csv", "b.csv"].map(path).to_vec(),
            tool: PdfTool::Merge
        }
    );
    assert!(find(&two, "query-together").is_some());
}

#[test]
fn an_archive_extracts_by_name_and_inside_one_nothing_is_compressed() {
    let mut inputs = with_selected(&["bundle.zip"]);
    let commands = actions(&inputs);
    let extract = find(&commands, "extract").expect("one archive selected");
    assert_eq!(extract.label.as_ref(), "Extract “bundle.zip” here");
    assert_eq!(
        menu_event(extract),
        MenuEvent::ExtractHere {
            path: path("bundle.zip")
        }
    );
    inputs.in_archive = true;
    inputs.archive_outer = Some("C:\\work\\bundle.zip".into());
    let inside = actions(&inputs);
    assert!(find(&inside, "extract").is_none() && find(&inside, "compress").is_none());
    assert_eq!(
        menu_event(find(&inside, "explorer-archive").unwrap()),
        MenuEvent::RevealArchive {
            archive: "C:\\work\\bundle.zip".into()
        }
    );
    assert!(
        find(&inside, "explorer").is_none(),
        "an archive is not a folder on disk"
    );
}

#[test]
fn the_other_pane_gets_copies_and_moves_of_the_selection() {
    let mut inputs = with_selected(&["a.txt", "b.txt"]);
    assert!(find(&actions(&inputs), "copy-other").is_none(), "one pane only");
    inputs.dual = true;
    inputs.other_pane = Some("D:\\Elsewhere".into());
    let commands = actions(&inputs);
    let both = vec![path("a.txt"), path("b.txt")];
    assert_eq!(
        menu_event(find(&commands, "copy-other").unwrap()),
        MenuEvent::CopyToOtherPane {
            paths: both.clone(),
            moving: false
        }
    );
    assert_eq!(
        menu_event(find(&commands, "move-other").unwrap()),
        MenuEvent::CopyToOtherPane {
            paths: both,
            moving: true
        }
    );
    assert!(matches!(&find(&commands, "open-here").unwrap().run, Run::Navigate(p) if p == "D:\\Elsewhere"));
    // Nothing is moved out of an archive or a closed folder.
    inputs.in_archive = true;
    let commands = actions(&inputs);
    assert!(find(&commands, "copy-other").is_some() && find(&commands, "move-other").is_none());
    inputs.in_archive = false;
    inputs.shut = true;
    assert!(find(&actions(&inputs), "move-other").is_none());
    inputs.shut = false;
    inputs.chosen = false;
    assert!(find(&actions(&inputs), "copy-other").is_none(), "nothing chosen");
}

fn tools_inputs() -> Inputs {
    let mut inputs = with_selected(&["notes.txt"]);
    inputs.scripts = vec![
        Script {
            name: "Tidy".into(),
            path: "C:\\scripts\\tidy.ps1".into(),
            program: "pwsh".into(),
            args: Vec::new(),
            missing: None,
        },
        Script {
            name: "Fetch".into(),
            path: "C:\\scripts\\fetch.py".into(),
            program: "python".into(),
            args: Vec::new(),
            missing: Some("python".into()),
        },
    ];
    inputs.openers = vec![
        Opener {
            id: "vscode",
            name: "VS Code",
            terminal: false,
            path: "code.exe".into(),
        },
        Opener {
            id: "wt",
            name: "Windows Terminal",
            terminal: true,
            path: "wt.exe".into(),
        },
    ];
    inputs.tasks = vec![
        TaskGroup {
            folder: HERE.into(),
            source: "npm".into(),
            tasks: vec![Task {
                name: "build".into(),
                command: "npm run build".into(),
                detail: String::new(),
            }],
            missing: None,
        },
        TaskGroup {
            folder: HERE.into(),
            source: "bun".into(),
            tasks: vec![Task {
                name: "dev".into(),
                command: "bun run dev".into(),
                detail: String::new(),
            }],
            missing: Some("bun".into()),
        },
    ];
    inputs
}

#[test]
fn the_tools_of_a_folder_you_can_change_are_offered() {
    let commands = actions(&tools_inputs());
    assert!(matches!(&find(&commands, "terminal").unwrap().run, Run::Terminal(p) if p == HERE));
    assert!(matches!(&find(&commands, "explorer").unwrap().run, Run::FileExplorer(p) if p == HERE));
    let task = find(&commands, &format!("run:{HERE}:npm run build")).expect("a task of the folder");
    assert_eq!(task.label.as_ref(), "Run npm run build");
    assert!(matches!(&task.run, Run::Task { folder, command } if folder == HERE && command == "npm run build"));
    assert!(
        find(&commands, &format!("run:{HERE}:bun run dev")).is_none(),
        "a task whose tool is missing is left out"
    );
    let editor = find(&commands, "open-in-vscode").unwrap();
    assert_eq!(editor.label.as_ref(), "Open this folder in VS Code");
    assert!(matches!(
        &editor.run,
        Run::OpenIn { opener, name, path } if opener == "vscode" && name == "VS Code" && path == HERE
    ));
    assert!(find(&commands, "open-in-wt").is_some());
    assert_eq!(
        menu_event(find(&commands, "scripts-folder").unwrap()),
        MenuEvent::OpenScriptsFolder
    );
}

#[test]
fn scripts_run_on_the_selection_in_the_folder_and_say_what_they_run_on() {
    let commands = actions(&tools_inputs());
    let tidy = find(&commands, "script:C:\\scripts\\tidy.ps1").unwrap();
    assert_eq!(tidy.label.as_ref(), "Script: Tidy");
    assert_eq!(tidy.detail.as_deref(), Some("on notes.txt"));
    assert_eq!(
        menu_event(tidy),
        MenuEvent::RunScript {
            script: "C:\\scripts\\tidy.ps1".into(),
            cwd: HERE.into(),
            paths: vec![path("notes.txt")]
        }
    );
    let fetch = find(&commands, "script:C:\\scripts\\fetch.py").unwrap();
    assert_eq!(fetch.detail.as_deref(), Some("installs what it needs first"));
    // Nothing selected: the folder; several: their number.
    let mut inputs = tools_inputs();
    inputs.chosen_entries.clear();
    inputs.selected.clear();
    inputs.chosen = false;
    let commands = actions(&inputs);
    assert_eq!(
        find(&commands, "script:C:\\scripts\\tidy.ps1")
            .unwrap()
            .detail
            .as_deref(),
        Some("on this folder")
    );
    let mut inputs = tools_inputs();
    inputs.chosen_entries.push(file(HERE, "more.txt", 1, 0));
    inputs.selected.push(path("more.txt"));
    let commands = actions(&inputs);
    assert_eq!(
        find(&commands, "script:C:\\scripts\\tidy.ps1")
            .unwrap()
            .detail
            .as_deref(),
        Some("on the 2 selected items")
    );
}

#[test]
fn a_closed_folder_a_search_and_an_archive_take_the_tools_away() {
    let mut shut = tools_inputs();
    shut.shut = true;
    let commands = actions(&shut);
    for id in [
        "terminal",
        "open-in-vscode",
        "timeline",
        "duplicates",
        "cleanup",
        "rule-here",
        "receive-pc",
    ] {
        assert!(find(&commands, id).is_none(), "{id} in a closed folder");
    }
    assert!(find(&commands, "scripts-folder").is_some() && find(&commands, "history").is_some());
    let mut finding = tools_inputs();
    finding.finding = true;
    let commands = actions(&finding);
    assert!(find(&commands, &format!("run:{HERE}:npm run build")).is_none());
    assert!(find(&commands, "receive-pc").is_none() && find(&commands, "paste-queued").is_none());
    let mut archive = tools_inputs();
    archive.in_archive = true;
    let commands = actions(&archive);
    for id in [
        "terminal",
        "open-in-vscode",
        "explorer",
        "who-locks",
        "defender",
        "secrets",
        "send-pc",
    ] {
        assert!(find(&commands, id).is_none(), "{id} in an archive");
    }
}

#[test]
fn the_folders_own_tools_ask_about_the_folder() {
    let commands = actions(&with_selected(&[]));
    let folder_event = |id: &str| menu_event(find(&commands, id).unwrap_or_else(|| panic!("{id} is listed")));
    assert_eq!(folder_event("timeline"), MenuEvent::Timeline { folder: HERE.into() });
    assert_eq!(
        folder_event("duplicates"),
        MenuEvent::Duplicates { folder: HERE.into() }
    );
    assert_eq!(folder_event("cleanup"), MenuEvent::Cleanup { folder: HERE.into() });
    assert_eq!(
        folder_event("export-ncdu"),
        MenuEvent::ExportNcdu { folder: HERE.into() }
    );
    assert_eq!(
        folder_event("receive-pc"),
        MenuEvent::ReceiveHere { folder: HERE.into() }
    );
    assert_eq!(folder_event("rule-here"), MenuEvent::Rule { folder: HERE.into() });
    assert_eq!(
        folder_event("paste-queued"),
        MenuEvent::PasteQueued { folder: HERE.into() }
    );
    assert_eq!(folder_event("send-pc"), MenuEvent::SendToPc { path: HERE.into() });
    assert_eq!(
        folder_event("share-network"),
        MenuEvent::ShareOnNetwork { path: HERE.into() }
    );
    assert_eq!(folder_event("secrets"), MenuEvent::Secrets { path: HERE.into() });
    assert_eq!(folder_event("yara"), MenuEvent::Yara { path: HERE.into() });
    assert_eq!(
        find(&commands, "send-pc").unwrap().label.as_ref(),
        "Send this folder to another PC"
    );
}

#[test]
fn one_selected_item_is_what_x_ray_sends_and_shares_name() {
    let mut inputs = with_selected(&["a.txt"]);
    inputs.focus_path = Some(path("a.txt"));
    let commands = actions(&inputs);
    assert_eq!(
        menu_event(find(&commands, "file-xray").unwrap()),
        MenuEvent::XRay { path: path("a.txt") }
    );
    assert_eq!(
        find(&commands, "send-pc").unwrap().label.as_ref(),
        "Send “a.txt” to another PC"
    );
    assert_eq!(
        menu_event(find(&commands, "share-network").unwrap()),
        MenuEvent::ShareOnNetwork { path: path("a.txt") }
    );
    assert_eq!(
        menu_event(find(&commands, "secrets").unwrap()),
        MenuEvent::Secrets { path: path("a.txt") }
    );
    // Two items: no single subject to send or share; the folder is still checked as a whole.
    let commands = actions(&with_selected(&["a.txt", "b.txt"]));
    assert!(find(&commands, "send-pc").is_none() && find(&commands, "share-network").is_none());
    assert_eq!(
        menu_event(find(&commands, "secrets").unwrap()),
        MenuEvent::Secrets { path: HERE.into() }
    );
    // Nothing on the keyboard or selected: nothing to X-Ray.
    assert!(find(&actions(&with_selected(&[])), "file-xray").is_none());
}

#[test]
fn history_comparing_and_undo_use_actions_when_they_have_them() {
    let none = actions(&with_selected(&[]));
    assert!(find(&none, "compare-files").is_none() && find(&none, "undo").is_none());
    let history = find(&none, "history").unwrap();
    let shows_history = keys::ShowHistory { query: String::new() };
    assert!(matches!(&history.run, Run::Action(make) if make().name() == gpui_kit::Action::name(&shows_history)));
    let mut inputs = with_selected(&["a.txt", "b.txt"]);
    inputs.pair = Some(((path("a.txt"), "a.txt".into()), (path("b.txt"), "b.txt".into())));
    inputs.undo = Some("copy".into());
    let commands = actions(&inputs);
    let compare = find(&commands, "compare-files").unwrap();
    assert_eq!(compare.label.as_ref(), "Compare “a.txt” and “b.txt”");
    assert!(matches!(&compare.run, Run::Action(make) if make().name() == gpui_kit::Action::name(&keys::CompareFiles)));
    let undo = find(&commands, "undo").unwrap();
    assert_eq!(undo.label.as_ref(), "Undo the copy");
    assert!(matches!(&undo.run, Run::Action(make) if make().name() == gpui_kit::Action::name(&keys::Undo)));
}

#[test]
fn locks_and_scans_name_the_selection_and_remote_places_get_none() {
    let commands = actions(&with_selected(&["a.txt"]));
    assert_eq!(
        menu_event(find(&commands, "who-locks").unwrap()),
        MenuEvent::Locks {
            paths: vec![path("a.txt")]
        }
    );
    assert_eq!(
        find(&commands, "who-locks").unwrap().label.as_ref(),
        "What's locking the selection?"
    );
    assert_eq!(
        menu_event(find(&commands, "defender").unwrap()),
        MenuEvent::Defender {
            paths: vec![path("a.txt")]
        }
    );
    let remote = Inputs {
        path: glint_model::paths::REMOTE.to_string() + "work",
        ..with_selected(&["a.txt"])
    };
    let commands = actions(&remote);
    for id in [
        "who-locks",
        "defender",
        "secrets",
        "yara",
        "send-pc",
        "compress",
        "terminal",
    ] {
        assert!(find(&commands, id).is_none(), "{id} on a remote");
    }
}

#[test]
fn connections_and_rules_are_always_there_and_nearby_follows_the_selection() {
    let commands = actions(&Inputs::default());
    assert!(matches!(&find(&commands, "add-remote").unwrap().run, Run::AddRemote));
    assert!(matches!(&find(&commands, "rules").unwrap().run, Run::Rules(None)));
    assert_eq!(
        find(&commands, "nearby").unwrap().label.as_ref(),
        "Phones and computers nearby (LocalSend): send or receive"
    );
    let commands = actions(&with_selected(&["a.txt"]));
    let nearby = find(&commands, "nearby").unwrap();
    assert_eq!(
        nearby.label.as_ref(),
        "Send the selection to a phone nearby (LocalSend)"
    );
    assert_eq!(
        menu_event(nearby),
        MenuEvent::SendNearby {
            paths: vec![path("a.txt")]
        }
    );
}

#[test]
fn converting_to_utf8_is_offered_for_a_selection_of_text_files_only() {
    // Text files: one or many, each its own file.
    let one = actions(&with_selected(&["old.txt"]));
    let convert = find(&one, "to-utf8").expect("a text file is offered it");
    assert_eq!(
        convert.label.as_ref(),
        "Convert the selected text files to UTF-8 (old Arabic, Cyrillic and other encodings)"
    );
    match &convert.run {
        Run::ToUtf8(paths) => assert_eq!(paths, &vec![path("old.txt")]),
        other => panic!("it should convert the selection, not {other:?}"),
    }
    let many = actions(&with_selected(&["a.txt", "b.csv", "c.md"]));
    match &find(&many, "to-utf8").expect("text files").run {
        Run::ToUtf8(paths) => assert_eq!(paths, &["a.txt", "b.csv", "c.md"].map(path).to_vec()),
        other => panic!("{other:?}"),
    }
    // Nothing selected, a folder, a picture, or one of several that is not text: not offered.
    for names in [&[][..], &["sub/"], &["a.png"], &["a.txt", "b.png"], &["a.txt", "sub/"]] {
        let commands = actions(&with_selected(names));
        assert!(find(&commands, "to-utf8").is_none(), "{names:?}");
    }
    // A file inside an archive is not rewritten.
    let mut inside = with_selected(&[]);
    let mut entry = file(r"C:\work\bundle.zip", "a.txt", 10, 0);
    entry.attrs |= glint_model::entries::attr::IN_ARCHIVE;
    inside.selected = vec![entry.path.clone()];
    inside.chosen_entries = vec![entry];
    inside.chosen = true;
    assert!(find(&actions(&inside), "to-utf8").is_none());
}
