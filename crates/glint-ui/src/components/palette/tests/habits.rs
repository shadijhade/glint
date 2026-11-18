//! Tests of the palette learning from use. Card G2 of docs/gpui/aware writes these.
//!
//! A command that is a menu row's twin has the row's weight over every kind of thing, lists first when nothing is
//! typed, counts when it is run, and all of it stops when learning is off.

use super::super::commands::{self, Command, Inputs, Kind, Run};
use super::{world, World};
use crate::components::menus::{CompressFormat, MenuEvent};
use crate::explorer::fake::file;
use crate::{persistence, state, testing};
use glint_model::rank::Visit;
use gpui_kit::TestAppContext;

const HERE: &str = r"C:\work";

/// Inputs for a writable folder with two files selected, and a folder that has been opened often.
fn inputs() -> Inputs {
    let entries = vec![file(HERE, "a.txt", 10, 0), file(HERE, "b.txt", 10, 0)];
    Inputs {
        path: HERE.into(),
        tabs: 1,
        chosen: true,
        selected: entries.iter().map(|e| e.path.clone()).collect(),
        chosen_entries: entries,
        visits: vec![(
            r"C:\work\project".into(),
            Visit {
                count: 10,
                last: 1_800_000_000_000,
            },
        )],
        now: 1_800_000_000_000,
        ..Inputs::default()
    }
}

fn ids(list: &[Command]) -> Vec<String> {
    list.iter().map(|c| c.id.clone()).collect()
}

/// The key a menu row of the same action has, as habits keep it.
fn key_of(command: &Command) -> Option<String> {
    match &command.run {
        Run::Menu(event) => crate::components::menus::Run::Ask(event.clone()).key(),
        _ => None,
    }
}

fn zip(list: &[Command]) -> &Command {
    list.iter()
        .find(|c| {
            matches!(
                &c.run,
                Run::Menu(MenuEvent::Compress {
                    format: CompressFormat::Zip,
                    ..
                })
            )
        })
        .expect("a zip command for a selection on disk")
}

/// The menu commands of a list, in the order declared.
fn menu_ones(list: &[Command]) -> Vec<&Command> {
    list.iter().filter(|c| key_of(c).is_some()).collect()
}

#[test]
fn with_nothing_used_the_list_and_its_order_are_what_they_were() {
    let _guard = testing::lock();
    let plain = commands::all(&inputs());
    let with_field = commands::all(&Inputs {
        used: std::collections::HashMap::new(),
        ..inputs()
    });
    assert_eq!(ids(&plain), ids(&with_field));
    assert!(plain.iter().all(|c| c.weight == 0. || c.kind == Kind::Folder));
    assert_eq!(ids(&commands::rank(&plain, "")), ids(&commands::rank(&with_field, "")));
}

#[test]
fn a_used_menu_action_has_its_weight_capped_and_comes_first_after_the_folders() {
    let _guard = testing::lock();
    let base = commands::all(&inputs());
    let key = key_of(zip(&base)).expect("a key");
    assert_eq!(key, "do:compress:zip");
    let mut used = std::collections::HashMap::new();
    used.insert(key, 20.0);
    let all = commands::all(&Inputs { used, ..inputs() });
    assert_eq!(zip(&all).weight, 8.0, "capped");
    let shown = commands::rank(&all, "");
    let first_action = shown.iter().position(|c| c.kind == Kind::Action).expect("actions");
    assert!(first_action > 0, "the folders come first");
    assert!(shown[..first_action].iter().all(|c| c.kind == Kind::Folder));
    assert_eq!(shown[first_action].id, zip(&all).id, "then the action used");
    // The rest keep the order they were declared in.
    let rest: Vec<_> = shown[first_action + 1..].iter().map(|c| c.id.clone()).collect();
    let declared: Vec<_> = commands::rank(&base, "")[first_action..]
        .iter()
        .map(|c| c.id.clone())
        .filter(|id| *id != zip(&all).id)
        .collect();
    assert_eq!(rest, declared);
}

#[test]
fn weighted_actions_come_heaviest_first_and_equal_ones_keep_their_declared_order() {
    let _guard = testing::lock();
    let base = commands::all(&inputs());
    let menus = menu_ones(&base);
    assert!(menus.len() >= 4, "enough menu commands to compare");
    let key = |ix: usize| key_of(menus[ix]).expect("a key");
    let mut used = std::collections::HashMap::new();
    // Declared order: 0, 1, 2, 3. Heaviest is 3; 0 and 1 weigh the same; 2 is not used.
    used.insert(key(3), 5.0);
    used.insert(key(0), 2.0);
    used.insert(key(1), 2.0);
    let all = commands::all(&Inputs { used, ..inputs() });
    let shown = commands::rank(&all, "");
    let at = |ix: usize| shown.iter().position(|c| c.id == menus[ix].id).expect("listed");
    assert!(at(3) < at(0), "heaviest first");
    assert!(at(0) < at(1), "equal weights keep their declared order");
    assert!(at(1) < at(2), "an unused one after the used");
}

#[test]
fn with_something_typed_a_used_command_outranks_an_unused_one_that_matches_equally() {
    let _guard = testing::lock();
    let all = commands::all(&inputs());
    let mut plain: Command = zip(&all).clone();
    plain.id = "plain".into();
    let mut used = plain.clone();
    used.id = "used".into();
    used.weight = 4.0;
    assert_eq!(ids(&commands::rank(&[plain.clone(), used.clone()], "compress"))[0], "used");
    assert_eq!(ids(&commands::rank(&[used, plain], "compress"))[0], "used");
}

#[gpui_kit::test]
fn running_a_menu_command_counts_it_and_with_learning_off_counts_nothing(cx: &mut TestAppContext) {
    let _guard = testing::lock();
    let mut w: World = world(cx, "habits");
    let counted = |w: &mut World| {
        w.cx.update(|_, cx| {
            persistence::app(cx)
                .map(|app| app.habits.actions.values().any(|kept| kept.contains_key("do:rule")))
                .unwrap_or(false)
        })
    };
    // Learning off: it runs and nothing is counted; and what was counted is not read.
    w.cx.update(|_, cx| {
        state::update(cx, |s| s.learn = false);
        persistence::update_app(cx, |app| {
            app.habits
                .note_action(glint_model::aware::Shape::Nothing, "do:something", 20_000);
        });
    });
    let used = w.cx.update(|_, cx| super::super::read_inputs(cx).used);
    assert!(used.is_empty(), "learning off: nothing is read");
    w.open();
    w.type_text("A rule for this folder");
    w.press("enter");
    assert!(!counted(&mut w), "learning off: nothing is counted");
    // Learning on: running it counts its key.
    w.cx.update(|_, cx| state::update(cx, |s| s.learn = true));
    let used = w.cx.update(|_, cx| super::super::read_inputs(cx).used);
    assert!(used.contains_key("do:something"), "what was counted is read");
    w.open();
    w.type_text("A rule for this folder");
    w.press("enter");
    assert!(counted(&mut w), "learning on: its key is counted");
    // And with suggestions off it is not read, though it is kept.
    w.cx.update(|_, cx| state::update(cx, |s| s.suggest = false));
    let used = w.cx.update(|_, cx| super::super::read_inputs(cx).used);
    assert!(used.is_empty());
}

#[test]
fn the_palette_offers_the_flat_view_in_a_folder_and_the_way_out_of_it() {
    let _guard = testing::lock();
    let flat = |list: &[Command]| list.iter().find(|c| c.id == "flat-view").map(|c| c.label.to_string());
    // A folder that can be shown flat offers it, wherever the keyboard is.
    let folder = Inputs {
        can_flat: true,
        ..inputs()
    };
    assert_eq!(
        flat(&commands::all(&folder)).as_deref(),
        Some("Flat view: every file in this folder and its subfolders as one list")
    );
    // Showing it: the same command is the way out.
    let showing = Inputs {
        flat: true,
        finding: true,
        ..inputs()
    };
    assert_eq!(
        flat(&commands::all(&showing)).as_deref(),
        Some("Leave the flat view, back to the folder")
    );
    // Where it can't be shown (This PC, the Recycle Bin, an archive) it isn't offered.
    assert_eq!(flat(&commands::all(&inputs())), None);
}
