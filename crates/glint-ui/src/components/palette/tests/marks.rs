//! Tests of the palette's marks commands (docs/gpui/marks, F9): a label, tags, a comment, copied, pasted or
//! cleared, for what is selected.

use super::super::commands::{self, Command, Inputs, Run};
use super::super::{Current, Palette, Sources};
use super::{world, World};
use crate::components::menus::MenuEvent;
use crate::explorer::fake::file;
use crate::testing;
use crate::kit::prelude::*;
use gpui_kit::TestAppContext;

const HERE: &str = r"C:\work";

fn names(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|l| l.to_string()).collect()
}

/// Inputs for a writable folder with two files selected, and the labels Glint starts with.
fn red_to_gray() -> Vec<String> {
    glint_model::marks::default_labels().into_iter().map(|l| l.name).collect()
}

fn inputs() -> Inputs {
    let entries = vec![file(HERE, "a.txt", 10, 0), file(HERE, "b.txt", 10, 0)];
    Inputs {
        path: HERE.into(),
        tabs: 1,
        chosen: true,
        selected: entries.iter().map(|e| e.path.clone()).collect(),
        chosen_entries: entries,
        ..Inputs::default()
    }
}

fn find<'a>(commands: &'a [Command], id: &str) -> &'a Command {
    commands.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id} is listed"))
}

fn event(command: &Command) -> MenuEvent {
    match &command.run {
        Run::Menu(event) => event.clone(),
        other => panic!("{} should be a menu item, not {other:?}", command.id),
    }
}

fn marks_labels(commands: &[Command]) -> Vec<String> {
    let ids = ["edit-tags", "edit-comment", "copy-marks", "paste-marks", "clear-marks", "label-none"];
    commands
        .iter()
        .filter(|c| c.id.starts_with("label-") || ids.contains(&c.id.as_str()))
        .map(|c| c.label.to_string())
        .collect()
}

#[test]
fn with_something_selected_the_palette_lists_every_label_then_the_marks_commands() {
    let _guard = testing::lock();
    let listed = marks_labels(&commands::actions_with(&inputs(), &red_to_gray()));
    assert_eq!(
        listed,
        [
            "Label: Red",
            "Label: Orange",
            "Label: Yellow",
            "Label: Green",
            "Label: Blue",
            "Label: Purple",
            "Label: Gray",
            "No label",
            "Edit tags…",
            "Edit comment…",
            "Copy marks",
            "Paste marks",
            "Clear marks",
        ]
    );
}

#[test]
fn with_nothing_selected_none_of_the_marks_commands_is_listed() {
    let _guard = testing::lock();
    let none = Inputs {
        chosen: false,
        selected: Vec::new(),
        chosen_entries: Vec::new(),
        ..inputs()
    };
    assert!(marks_labels(&commands::actions_with(&none, &red_to_gray())).is_empty());
}

#[test]
fn a_label_renamed_in_the_settings_is_listed_and_found_under_its_new_name() {
    let _guard = testing::lock();
    let all = commands::all_with(&inputs(), &names(&["Important", "Orange"]));
    assert!(!all.iter().any(|c| c.label.as_ref() == "Label: Red"));
    let found = commands::rank(&all, "important");
    assert_eq!(found[0].label.as_ref(), "Label: Important");
    // (Export marks… says "comments" in its words, so it may come first: Edit comment… is among them.)
    let found = commands::rank(&all, "comment");
    assert!(found.iter().any(|c| c.label.as_ref() == "Edit comment…"));
    let found = commands::rank(&all, "tag");
    assert!(found.iter().any(|c| c.label.as_ref() == "Edit tags…"));
}

#[test]
fn the_commands_show_the_keys_that_do_the_same() {
    let _guard = testing::lock();
    let all = commands::actions_with(&inputs(), &red_to_gray());
    let key = |id: &str| find(&all, id).shortcut.as_ref().map(|k| k.to_string());
    assert_eq!(key("label-0").as_deref(), Some("Alt+1"));
    assert_eq!(key("label-6").as_deref(), Some("Alt+7"));
    assert_eq!(key("label-none").as_deref(), Some("Alt+0"));
    assert_eq!(key("edit-tags").as_deref(), Some("Alt+T"));
    assert_eq!(key("edit-comment").as_deref(), Some("Alt+N"));
    assert_eq!(key("copy-marks"), None);
}

#[test]
fn each_command_runs_what_it_names_on_the_selected_paths() {
    let _guard = testing::lock();
    let inputs = Inputs {
        focus_path: Some(format!("{HERE}\\b.txt")),
        ..inputs()
    };
    let all = commands::actions_with(&inputs, &red_to_gray());
    let paths = inputs.selected.clone();
    assert!(matches!(
        event(find(&all, "label-1")),
        MenuEvent::SetLabel { paths: p, label: Some(l) } if p == paths && l == "Orange"
    ));
    assert!(matches!(
        event(find(&all, "label-none")),
        MenuEvent::SetLabel { paths: p, label: None } if p == paths
    ));
    assert!(matches!(event(find(&all, "edit-tags")), MenuEvent::EditTags { paths: p } if p == paths));
    assert!(matches!(event(find(&all, "edit-comment")), MenuEvent::EditComment { paths: p } if p == paths));
    assert!(matches!(event(find(&all, "paste-marks")), MenuEvent::PasteMarks { paths: p } if p == paths));
    assert!(matches!(event(find(&all, "clear-marks")), MenuEvent::ClearMarks { paths: p } if p == paths));
    // Copy marks takes the focused item, else the first selected.
    assert!(matches!(
        event(find(&all, "copy-marks")),
        MenuEvent::CopyMarks { path } if path.ends_with("b.txt")
    ));
    let unfocused = commands::actions_with(&Inputs { focus_path: None, ..inputs }, &red_to_gray());
    assert!(matches!(
        event(find(&unfocused, "copy-marks")),
        MenuEvent::CopyMarks { path } if path.ends_with("a.txt")
    ));
}

/// Opens the palette over `inputs` (as `open_with` does over what it reads).
fn open_over(w: &mut World, inputs: Inputs, labels: Vec<String>) {
    let sources: Sources = w.sources(Vec::new());
    w.cx.update(|window, cx| {
        super::super::keys::ensure_bound(cx);
        let view = cx.new(|cx| Palette::new(inputs, sources, window, cx));
        cx.set_global(Current(view.downgrade()));
        window.open_dialog(cx, move |dialog, _window, _cx| dialog.p_0().child(view.clone()));
    });
    w.cx.run_until_parked();
    // What came back from the background (tools, zoxide) listed the commands again, without the labels.
    w.give_labels(&labels);
}

impl World {
    /// Lists the labels in the open palette (what `read_inputs` will hand `all_with`).
    fn give_labels(&mut self, labels: &[String]) {
        let palette = self.palette();
        self.cx.update(|_, cx| {
            palette.update(cx, |p, cx| {
                p.all = commands::all_with(&p.inputs, labels);
                p.typed(cx);
            })
        });
        self.cx.update(|window, _| window.refresh());
        self.cx.run_until_parked();
    }
}

#[gpui_kit::test]
fn an_arabic_label_is_listed_found_by_typing_it_and_mirrors_like_the_other_rows(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w = world(cx, "marks-arabic");
    testing::set_rtl(true);
    open_over(&mut w, inputs(), names(&["عاجل", "Orange"]));
    let data = commands::rank(&commands::all_with(&inputs(), &names(&["عاجل", "Orange"])), "عاجل");
    assert_eq!(data[0].label.as_ref(), "Label: عاجل", "data level");
    w.give_labels(&names(&["عاجل", "Orange"]));
    w.cx.update(|window, cx| super::super::type_query("عاجل", window, cx));
    w.cx.run_until_parked();
    let results = w.results();
    assert_eq!(results[0], "Label: عاجل", "{results:?}");
    assert!(w.has("Label: عاجل"));
    w.press("escape");
    testing::set_rtl(false);
}

fn selected_in(folder: &str, entries: Vec<glint_model::entries::FileEntry>) -> Inputs {
    Inputs {
        path: folder.into(),
        tabs: 1,
        chosen: !entries.is_empty(),
        selected: entries.iter().map(|e| e.path.clone()).collect(),
        chosen_entries: entries,
        ..Inputs::default()
    }
}

fn import_source(inputs: &Inputs) -> Option<String> {
    match event(find(&commands::actions_with(inputs, &[]), "import-marks")) {
        MenuEvent::ImportMarks { source } => source,
        other => panic!("{other:?}"),
    }
}

#[test]
fn import_marks_is_always_listed_and_carries_the_file_only_when_one_marks_file_is_selected() {
    let _guard = testing::lock();
    let nothing = selected_in(HERE, Vec::new());
    assert_eq!(import_source(&nothing), None);
    let dat = selected_in(HERE, vec![file(HERE, "tag.dat", 10, 0)]);
    assert_eq!(import_source(&dat).as_deref(), Some(r"C:\work\tag.dat"));
    assert_eq!(import_source(&selected_in(HERE, vec![file(HERE, "notes.txt", 10, 0)])), None);
    assert_eq!(import_source(&selected_in(HERE, vec![file(HERE, "tag.dat", 1, 0), file(HERE, "b.txt", 1, 0)])), None);
    assert!(import_source(&selected_in(HERE, vec![file(HERE, "تقرير.glint-marks.jsonl", 1, 0)])).is_some());
    let all = commands::actions_with(&dat, &[]);
    assert_eq!(find(&all, "import-marks").label.as_ref(), "Import marks…");
    assert_eq!(find(&all, "export-marks").label.as_ref(), "Export marks…");
}

#[test]
fn export_marks_takes_the_tabs_folder_or_the_one_selected_folder_and_not_where_marks_cannot_be() {
    let _guard = testing::lock();
    let folder_of = |inputs: &Inputs| match event(find(&commands::actions_with(inputs, &[]), "export-marks")) {
        MenuEvent::ExportMarks { folder } => folder,
        other => panic!("{other:?}"),
    };
    assert_eq!(folder_of(&selected_in(HERE, Vec::new())), HERE);
    assert_eq!(folder_of(&selected_in(HERE, vec![file(HERE, "a.txt", 1, 0)])), HERE);
    let sub = crate::explorer::fake::folder(HERE, "مشروع");
    assert_eq!(folder_of(&selected_in(HERE, vec![sub.clone()])), sub.path);
    let missing = |inputs: Inputs| !commands::actions_with(&inputs, &[]).iter().any(|c| c.id == "export-marks");
    assert!(missing(Inputs { in_archive: true, ..selected_in(HERE, Vec::new()) }));
    assert!(missing(Inputs { finding: true, ..selected_in(HERE, Vec::new()) }));
    assert!(missing(selected_in(r"remote:abc\x", Vec::new())));
    assert!(missing(selected_in("", Vec::new())));
}

#[test]
fn xyplorer_finds_both_commands() {
    let _guard = testing::lock();
    let all = commands::all(&selected_in(HERE, Vec::new()));
    let found: Vec<String> = commands::rank(&all, "xyplorer").iter().map(|c| c.label.to_string()).collect();
    assert!(found.contains(&"Import marks…".to_string()) && found.contains(&"Export marks…".to_string()), "{found:?}");
    let found = commands::rank(&all, "tag.dat");
    assert!(found.iter().any(|c| c.id == "import-marks"));
}
