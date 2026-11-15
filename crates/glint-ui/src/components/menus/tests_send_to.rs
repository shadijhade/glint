//! Tests of Move or copy to. Card G1 of docs/gpui/aware writes these.

use super::tests::*;
use super::*;
use crate::explorer::fake::file;
use glint_core::aware::Answer;
use glint_model::aware::{Clip, Place};
use glint_model::entries::attr;
use gpui_kit::TestAppContext;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

const INVOICES: &str = r"D:\Papers\Invoices";
const PDF: &str = "C:\\x\\تقرير الربع 1.pdf";

fn place(folder: &str, uses: u32, share: f64) -> Place {
    Place {
        folder: folder.into(),
        uses,
        weight: f64::from(uses),
        share,
    }
}

fn paths() -> Vec<String> {
    vec![r"C:\x\a.pdf".into()]
}

fn menu_items(places: &[Place], folder: &str) -> Vec<Node> {
    match send_to::menu(&paths(), places, folder) {
        Node::Menu { items, .. } => items,
        other => panic!("not a submenu: {other:?}"),
    }
}

/// The rows (not the headings) of one section of the submenu: `0` is Move to, `1` is Copy to.
fn section(items: &[Node], which: usize) -> Vec<&Entry> {
    let mut at = 0;
    let mut rows = Vec::new();
    for node in items {
        match node {
            Node::Heading(_) => at += 1,
            Node::Item(entry) if at == which + 1 => rows.push(entry),
            _ => {}
        }
    }
    rows
}

fn move_event(folder: &str, moving: bool) -> Run {
    Run::Ask(MenuEvent::SendTo {
        paths: paths(),
        folder: folder.into(),
        moving,
    })
}

// ---- The submenu as data -------------------------------------------------------------------------------------

#[test]
fn with_no_places_each_heading_has_only_choose_a_folder() {
    let _guard = english();
    let items = menu_items(&[], r"C:\x");
    assert_eq!(
        labels(&items),
        ["# Move to", "Choose a folder…", "# Copy to", "Choose a folder…"]
    );
    assert_eq!(
        section(&items, 0)[0].run,
        Run::Ask(MenuEvent::ChooseSendTo { paths: paths(), moving: true })
    );
    assert_eq!(
        section(&items, 1)[0].run,
        Run::Ask(MenuEvent::ChooseSendTo { paths: paths(), moving: false })
    );
}

#[test]
fn two_places_give_two_rows_under_each_heading_heaviest_first_with_their_events() {
    let _guard = english();
    let places = [place(INVOICES, 4, 0.8), place(r"D:\Papers\Taxes", 1, 0.2)];
    let items = menu_items(&places, r"C:\x");
    assert_eq!(
        labels(&items),
        [
            "# Move to",
            "Invoices",
            "Taxes",
            "Choose a folder…",
            "# Copy to",
            "Invoices",
            "Taxes",
            "Choose a folder…"
        ]
    );
    let moving = section(&items, 0);
    assert_eq!(moving[0].run, move_event(INVOICES, true));
    assert_eq!(moving[1].run, move_event(r"D:\Papers\Taxes", true));
    assert_eq!(moving[0].icon, Some(IconName::FolderOutput));
    let copying = section(&items, 1);
    assert_eq!(copying[0].run, move_event(INVOICES, false));
    assert_eq!(copying[1].run, move_event(r"D:\Papers\Taxes", false));
    assert_eq!(copying[0].icon, Some(IconName::CopyPlus));
}

#[test]
fn seven_places_list_five_and_the_folder_the_items_are_in_is_left_out() {
    let _guard = english();
    let places: Vec<Place> = (0..7).map(|i| place(&format!(r"D:\f{i}"), 1, 0.1)).collect();
    let items = menu_items(&places, r"C:\x");
    assert_eq!(section(&items, 0).len(), send_to::MOST + 1, "five places and Choose a folder…");
    assert_eq!(section(&items, 1).len(), send_to::MOST + 1);
    // The folder they are in is not offered, in any case, and the next place takes its turn.
    let items = menu_items(&places, r"d:\F0");
    let moving: Vec<String> = section(&items, 0).iter().map(|e| e.label.to_string()).collect();
    assert_eq!(moving, ["f1", "f2", "f3", "f4", "f5", "Choose a folder…"]);
}

#[test]
fn a_row_tells_its_folder_from_another_of_the_same_name_by_its_parent() {
    let _guard = english();
    let places = [place(INVOICES, 2, 0.5), place(r"E:\Old\Invoices", 2, 0.5)];
    let items = menu_items(&places, r"C:\x");
    let rows = section(&items, 0);
    assert_eq!(rows[0].hint.as_deref(), Some(r"D:\Papers"));
    assert_eq!(rows[1].hint.as_deref(), Some(r"E:\Old"));
    assert_eq!(rows[0].label, rows[1].label);
}

#[test]
fn a_drive_root_is_named_by_its_whole_path() {
    let _guard = english();
    let items = menu_items(&[place(r"D:\", 1, 0.5)], r"C:\x");
    assert_eq!(section(&items, 0)[0].label, r"D:\");
}

// ---- The usual place -----------------------------------------------------------------------------------------

#[test]
fn the_usual_place_is_offered_as_a_suggested_row_with_its_reason() {
    let _guard = english();
    let places = [place(INVOICES, 4, 0.8)];
    let row = |ext, folders_only, folder| match send_to::usual_row(&paths(), &places, ext, folders_only, folder) {
        Some(Node::Item(entry)) => entry,
        other => panic!("not a row: {other:?}"),
    };
    let pdf = row(Some("pdf"), false, r"C:\x");
    assert_eq!(pdf.label, "Move to “Invoices”");
    assert_eq!(pdf.reason.as_deref(), Some("Where you usually put pdf files"));
    assert!(pdf.smart);
    assert_eq!(pdf.run, move_event(INVOICES, true));
    assert_eq!(
        row(None, true, r"C:\x").reason.as_deref(),
        Some("Where you usually put folders")
    );
    assert_eq!(
        row(None, false, r"C:\x").reason.as_deref(),
        Some("Where you usually put files like these")
    );
}

#[test]
fn a_place_that_is_not_clearly_the_usual_one_or_is_where_they_are_is_not_offered() {
    let _guard = english();
    let half = [place(INVOICES, 4, 0.5)];
    assert_eq!(send_to::usual_row(&paths(), &half, Some("pdf"), false, r"C:\x"), None);
    let clear = [place(INVOICES, 4, 0.8)];
    assert_eq!(send_to::usual_row(&paths(), &clear, Some("pdf"), false, r"d:\papers\invoices"), None);
    assert_eq!(send_to::usual_row(&paths(), &[], Some("pdf"), false, r"C:\x"), None);
}

// ---- In the item menu ----------------------------------------------------------------------------------------

#[test]
fn an_item_menu_has_the_submenu_with_the_places_the_context_holds() {
    let _guard = english();
    let mut c = ctx();
    c.places = vec![place(INVOICES, 4, 0.8)];
    let menu = item_menu(&[text("a.pdf")], &c);
    let moving = labels(submenu(&menu, "Move or copy to"));
    assert_eq!(moving[..2], ["# Move to", "Invoices"]);
}

#[test]
fn an_item_inside_an_archive_has_no_move_or_copy_to() {
    let _guard = english();
    let inside = with_attrs(text("a.txt"), attr::IN_ARCHIVE);
    let mut c = ctx();
    c.archive = Some(an_archive());
    let menu = item_menu(&[inside], &c);
    let all = labels(&menu);
    assert!(!all.contains(&"Move or copy to>".to_string()), "{all:?}");
    // …nor an item on a remote connection.
    let there = with_attrs(text("a.txt"), attr::REMOTE);
    let all = labels(&item_menu(&[there], &ctx()));
    assert!(!all.contains(&"Move or copy to>".to_string()), "{all:?}");
}

// ---- In a window, with what was learned ----------------------------------------------------------------------

static RIGS: AtomicUsize = AtomicUsize::new(0);

/// A window on `C:\x` with saved state in a folder of its own and an Aware entity whose probe never answers.
struct Rig {
    ui: Ui,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig(cx: &mut TestAppContext) -> Rig {
    let disk = disk();
    let ui = open(cx, &disk, "C:\\x");
    let dir = std::env::temp_dir().join(format!(
        "glint-menus-send-to-{}-{}",
        std::process::id(),
        RIGS.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::create_dir_all(&dir);
    let state_file = dir.join("state.json");
    let sources = aware::Sources {
        probe: Rc::new(|_asks, _lane, _stop| async_channel::unbounded::<Answer>().1),
        clip_seq: Rc::new(|| 0),
        clip: Rc::new(|_| gpui_kit::Task::ready(Clip::Empty)),
        today: Rc::new(|| 20_000),
    };
    cx.update(|cx| {
        persistence::init_at(cx, state_file);
        aware::init_with(cx, sources);
    });
    Rig { ui, dir }
}

impl Rig {
    /// The person has put PDFs into Invoices three times.
    fn learn(&mut self) {
        self.ui.visual.update(|_, cx| {
            for _ in 0..3 {
                aware::note_place(cx, &["pdf".into()], INVOICES);
            }
        });
    }

    fn open_pdf(&mut self) -> Vec<Node> {
        self.ui.select(&[PDF]);
        self.ui.open_menu(Some(PDF), Some(at(100., 10.)), false);
        self.ui.nodes()
    }
}

#[gpui_kit::test]
fn a_learned_place_is_suggested_and_listed_in_the_submenu(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx);
    rig.learn();
    let nodes = rig.open_pdf();
    let heading = nodes
        .iter()
        .position(|node| matches!(node, Node::Heading(h) if h == "Suggested"))
        .unwrap_or_else(|| panic!("no Suggested: {:?}", labels(&nodes)));
    let row = nodes
        .iter()
        .position(|node| matches!(node, Node::Item(e) if e.label == "Move to “Invoices”" && e.smart))
        .unwrap_or_else(|| panic!("no usual place: {:?}", labels(&nodes)));
    assert!(row > heading, "{:?}", labels(&nodes));
    if let Node::Item(entry) = &nodes[row] {
        assert_eq!(entry.reason.as_deref(), Some("Where you usually put pdf files"));
    }
    let sub = submenu(&nodes, "Move or copy to");
    assert!(find(sub, "Invoices").is_some(), "{:?}", labels(sub));
}

#[gpui_kit::test]
fn with_learning_off_there_is_no_usual_place_and_the_submenu_keeps_its_choose_rows(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx);
    rig.learn();
    rig.ui.visual.update(|_, cx| state::update(cx, |settings| settings.learn = false));
    let nodes = rig.open_pdf();
    assert!(find(&nodes, "Move to “Invoices”").is_none(), "{:?}", labels(&nodes));
    let sub = submenu(&nodes, "Move or copy to");
    assert!(find(sub, "Invoices").is_none());
    assert_eq!(
        labels(sub),
        ["# Move to", "Choose a folder…", "# Copy to", "Choose a folder…"]
    );
}

#[gpui_kit::test]
fn choosing_a_place_under_move_to_tells_the_window_to_move_there(cx: &mut TestAppContext) {
    let _guard = english();
    let mut rig = rig(cx);
    rig.learn();
    let nodes = rig.open_pdf();
    let run = find(submenu(&nodes, "Move or copy to"), "Invoices")
        .expect("the place")
        .run
        .clone();
    let view = rig.ui.view();
    rig.ui.visual.update(|_, cx| {
        view.update(cx, |_, cx| cx.emit(MenuViewEvent::Chose(run)));
    });
    rig.ui.frame();
    assert_eq!(
        rig.ui.events(),
        [MenuEvent::SendTo {
            paths: vec![file("C:\\x", "تقرير الربع 1.pdf", 7000, 0).path],
            folder: INVOICES.into(),
            moving: true,
        }]
    );
}
