//! The last commands of the TypeScript palette: Get access, New <kind>, the Recycle Bin's Restore and Delete for
//! good, Calculate folder sizes, New window, the panes and tabs, Invert selection, Extract all, Archive
//! properties, and the Sending / Shared rows. Each is listed exactly when it applies and yields what its menu item
//! or button does.

use super::super::commands::{actions, ArchiveTop, Change, Command, Inputs, Run, ShareView};
use crate::actions as keys;
use crate::components::menus::MenuEvent;
use crate::explorer::TabId;
use gpui_kit::Action;

const HERE: &str = "C:\\work";

/// A change to the inputs, to make a place where something does not apply.
type Adjust = fn(&mut Inputs);

fn in_folder() -> Inputs {
    Inputs {
        path: HERE.into(),
        tab: Some(TabId(7)),
        tabs: 1,
        ..Inputs::default()
    }
}

fn find<'a>(commands: &'a [Command], id: &str) -> Option<&'a Command> {
    commands.iter().find(|c| c.id == id)
}

fn listed(inputs: &Inputs, id: &str) -> bool {
    find(&actions(inputs), id).is_some()
}

fn label(inputs: &Inputs, id: &str) -> Option<String> {
    find(&actions(inputs), id).map(|c| c.label.to_string())
}

fn run_of(inputs: &Inputs, id: &str) -> Option<Run> {
    find(&actions(inputs), id).map(|c| c.run.clone())
}

fn menu_event(inputs: &Inputs, id: &str) -> MenuEvent {
    match run_of(inputs, id) {
        Some(Run::Menu(event)) => event,
        other => panic!("{id} should be a menu item, not {other:?}"),
    }
}

fn change(inputs: &Inputs, id: &str) -> Change {
    match run_of(inputs, id) {
        Some(Run::Change(change)) => change,
        other => panic!("{id} should be a change, not {other:?}"),
    }
}

#[test]
fn get_access_is_offered_for_a_closed_folder_windows_can_open() {
    let mut inputs = in_folder();
    assert!(!listed(&inputs, "get-access"));
    inputs.shut = true;
    inputs.can_get_access = true;
    assert!(matches!(change(&inputs, "get-access"), Change::GetAccess(TabId(7))));
    assert_eq!(actions(&inputs).iter().filter(|c| c.id == "get-access").count(), 1);
    assert!(!listed(&inputs, "never"));
    // On another computer, or refused: nothing Glint can do.
    inputs.can_get_access = false;
    assert!(!listed(&inputs, "get-access"));
}

#[test]
fn new_text_document_and_each_kind_are_offered_in_a_writable_folder() {
    let mut inputs = in_folder();
    inputs.new_kinds = vec![(".docx".into(), "Microsoft Word Document".into())];
    assert_eq!(menu_event(&inputs, "new-file"), MenuEvent::NewTextDocument);
    assert_eq!(
        menu_event(&inputs, "new-.docx"),
        MenuEvent::NewOfKind { ext: ".docx".into() }
    );
    assert_eq!(
        label(&inputs, "new-.docx").as_deref(),
        Some("New Microsoft Word Document")
    );
    let places: [(&str, Adjust); 4] = [
        ("a search", |i| i.finding = true),
        ("a closed folder", |i| i.shut = true),
        ("an archive", |i| i.in_archive = true),
        ("This PC", |i| i.path.clear()),
    ];
    for (name, adjust) in places {
        let mut inputs = inputs.clone();
        adjust(&mut inputs);
        assert!(!listed(&inputs, "new-file"), "no new text document in {name}");
        assert!(!listed(&inputs, "new-.docx"), "no new kind in {name}");
    }
}

#[test]
fn the_recycle_bin_restores_and_deletes_the_selection_for_good() {
    let mut inputs = Inputs {
        path: glint_model::paths::RECYCLE_BIN.into(),
        tab: Some(TabId(1)),
        ..Inputs::default()
    };
    assert!(
        !listed(&inputs, "restore") && !listed(&inputs, "purge"),
        "nothing chosen"
    );
    inputs.chosen = true;
    inputs.selected = vec!["C:\\$Recycle.Bin\\a".into(), "C:\\$Recycle.Bin\\b".into()];
    assert_eq!(
        menu_event(&inputs, "restore"),
        MenuEvent::Restore {
            paths: inputs.selected.clone()
        }
    );
    let Some(Run::Action(make)) = run_of(&inputs, "purge") else {
        panic!("Delete for good is the action Shift+Delete runs");
    };
    assert_eq!(make().name(), keys::DeletePermanently.name());
    // A folder, even with a selection, has neither.
    let folder = Inputs {
        chosen: true,
        ..in_folder()
    };
    assert!(!listed(&folder, "restore") && !listed(&folder, "purge"));
}

#[test]
fn folder_sizes_need_a_listed_folder_that_is_not_a_search() {
    let mut inputs = in_folder();
    assert!(matches!(menu_event(&inputs, "sizes"), MenuEvent::FolderSizes));
    inputs.finding = true;
    assert!(!listed(&inputs, "sizes"));
    inputs.finding = false;
    inputs.unlisted = true;
    assert!(!listed(&inputs, "sizes"));
    assert!(!listed(&Inputs::default(), "sizes"), "This PC has no folder to measure");
}

#[test]
fn new_window_opens_the_folder_shown() {
    assert_eq!(
        menu_event(&in_folder(), "new-window"),
        MenuEvent::OpenInNewWindow { path: HERE.into() }
    );
    assert!(listed(&Inputs::default(), "new-window"), "also from This PC");
}

#[test]
fn moving_a_tab_and_evening_out_panes_need_two_panes() {
    let mut inputs = in_folder();
    inputs.tabs = 2;
    assert!(!listed(&inputs, "move-tab"), "one pane");
    inputs.dual = true;
    assert!(matches!(change(&inputs, "move-tab"), Change::MoveTab(TabId(7))));
    inputs.tabs = 1;
    assert!(!listed(&inputs, "move-tab"), "a pane's only tab stays");
    assert!(!listed(&inputs, "even-panes"), "they are even");
    inputs.uneven = true;
    assert!(matches!(change(&inputs, "even-panes"), Change::EvenPanes));
    inputs.dual = false;
    assert!(!listed(&inputs, "even-panes"), "one pane has nothing to even out");
}

#[test]
fn invert_selection_is_for_folders_and_searches() {
    assert!(matches!(
        change(&in_folder(), "invert-selection"),
        Change::InvertSelection
    ));
    let search = Inputs {
        path: String::new(),
        finding: true,
        ..in_folder()
    };
    assert!(listed(&search, "invert-selection"));
    assert!(
        !listed(&Inputs::default(), "invert-selection"),
        "This PC lists no files"
    );
}

#[test]
fn extract_all_and_archive_properties_are_for_the_archive_shown() {
    let mut inputs = Inputs {
        path: "C:\\a\\pack.zip".into(),
        in_archive: true,
        ..in_folder()
    };
    assert!(!listed(&inputs, "extract-all") && !listed(&inputs, "archive-properties"));
    inputs.archive_outer = Some("C:\\a\\pack.zip".into());
    inputs.archive_top = Some(ArchiveTop {
        outer: "C:\\a\\pack.zip".into(),
        single_file: false,
    });
    assert_eq!(
        menu_event(&inputs, "extract-all"),
        MenuEvent::ExtractHere {
            path: "C:\\a\\pack.zip".into()
        }
    );
    assert_eq!(
        label(&inputs, "extract-all").as_deref(),
        Some("Extract all: everything into a folder next to the archive")
    );
    assert!(matches!(
        run_of(&inputs, "archive-properties"),
        Some(Run::Properties(path)) if path == "C:\\a\\pack.zip"
    ));
    // A single-file format words it differently.
    inputs.archive_top = Some(ArchiveTop {
        outer: "C:\\a\\notes.txt.gz".into(),
        single_file: true,
    });
    assert_eq!(
        label(&inputs, "extract-all").as_deref(),
        Some("Extract the file next to the archive")
    );
    // Inside the archive's folders only the properties stay.
    inputs.archive_top = None;
    assert!(!listed(&inputs, "extract-all") && listed(&inputs, "archive-properties"));
}

#[test]
fn sending_and_sharing_rows_show_only_while_there_is_something() {
    let mut inputs = in_folder();
    assert!(!listed(&inputs, "sending") && !listed(&inputs, "serving"));
    inputs.sending = true;
    assert!(matches!(run_of(&inputs, "sending"), Some(Run::Share(ShareView::Send))));
    inputs.serving = true;
    assert!(matches!(
        run_of(&inputs, "serving"),
        Some(Run::Share(ShareView::Network))
    ));
}
