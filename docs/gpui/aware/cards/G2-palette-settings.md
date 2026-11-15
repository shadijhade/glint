# Card G2 — The palette learns; the switches in Settings

Wave 4 · crate `glint-ui` · the command palette and one Settings section

**You own:** `crates/glint-ui/src/components/palette/commands.rs`, `palette/mod.rs`, `palette/tests/habits.rs`; `crates/glint-ui/src/components/settings/general.rs` (the History section only), `settings/pages.rs` (three lines of `SEARCH`), `settings/tests_aware.rs`.
**Read first:** STANDARDS.md §5.1; PLAN.md §2 (R8), §3 (F12, F13), §5.5; `palette/commands.rs` (`Command` at line 142 and its `weight`, `Run` at 89, `Inputs` at 231, `actions` at 457, `rank` at 1692); `palette/mod.rs` (`read_inputs` at 712, and where a chosen command is run); `settings/general.rs` lines 780 to 886 (the History section, its rows and the zoxide switch); `settings/widgets.rs` (`switch_row` at 161, `setting_row` at 124, `button` at 201, `good` at 303); `settings/pages.rs` (`SEARCH` and its tests); `settings/tests_general.rs` (the test that checks every `SEARCH` entry against a drawn row); `crates/glint-ui/src/aware/mod.rs` (`note_action`, `forget_habits`).
**Check with:** `cargo test -p glint-ui components::palette` and `cargo test -p glint-ui components::settings` (old tests unchanged), then clippy on the crate.

## 1. The palette (F12)

A palette command whose `run` is `Run::Menu(event)` is the same action as a menu row, and has the same key: `crate::components::menus::Run::Ask(event.clone()).key()`.

- **`Inputs`** gains `pub used: std::collections::HashMap<String, f64>`: each such key's weight over every kind of thing, today. `read_inputs` fills it: empty when `settings.learn` or `settings.suggest` is off or there is no saved state; else, for every action key in `persistence::app(cx).habits.actions` (walk the map), `habits.overall_weight(key, today)` (today is `glint_model::aware::habits::day_of` of now).
- **`actions(inputs)`**: when a command with `Run::Menu(event)` is made, its `weight` becomes `used[key].min(8.0)` (zero when not there). Do this in one place (where the list is finished, walk it once), not at every `push`.
- **`rank` with nothing typed**: the folders as today; then the actions whose weight is above zero, heaviest first (a stable sort: equal weights keep the order they were declared in); then the rest in the order they were declared. With something typed nothing changes: `rank` already adds a command's weight to its score.
- **Running one counts**: where the palette runs a `Run::Menu(event)` command, first `aware::note_action(cx, Shape::of(&inputs.chosen_entries), &key)`.

## 2. Settings: History (F13)

After the zoxide switch, three rows:

1. `widgets::switch_row("suggest", t!("Suggest the next step"), Some(t!("Menus and the dock put forward what fits the selection: Extract here for an archive, Compare for two files")), s.suggest, false, |on, _, cx| state::update(cx, |s| s.suggest = on), t)`
2. `widgets::switch_row("learn", t!("Learn from what I do"), Some(t!("Glint counts the actions you choose and where you move things, on this PC only, to offer the likely one first")), s.learn, false, |on, _, cx| state::update(cx, |s| s.learn = on), t)`
3. A `setting_row` whose label is `Nothing learned yet` when the saved habits are empty, else `{n} habits learned` (`1 habit learned`), n being how many action keys and places are kept in all (count the inner maps' entries); no description; one button `widgets::button("habits-forget", t!("Forget what Glint learned"), None)`, disabled when n is 0, whose click calls `aware::forget_habits(cx)` and `widgets::good(window, cx, t!("Forgotten"), None)`.

`pages.rs`, three entries on the History page, in `SEARCH`'s existing shape (label word for word as drawn, then extra words to find it by): "Suggest the next step" (`smart suggestions suggested menu dock`), "Learn from what I do" (`habits learning usual`), "Forget what Glint learned" (`habits clear reset privacy`). Read the test that ties `SEARCH` to the drawn rows before choosing which label the third entry carries: it must be text the section really draws.

## Tests

`palette/tests/habits.rs` (the palette's tests build `Inputs` by hand: do as `palette/tests/files_and_tools.rs` does):
1. With `used` empty the list and its order are what they are today (compare the ids with a list built without the field).
2. With a weight for `do:compress:zip`, "Compress to a zip" (or whatever that command is called: find the command whose `run` is `Run::Menu(MenuEvent::Compress { format: Zip, .. })`) has that weight, capped at 8.0, and with nothing typed comes before every action without a weight and after the folders.
3. Two weighted actions come heaviest first; equal weights keep their declared order.
4. With something typed, a weighted command outranks an unweighted one that matches equally.
5. In a window: running such a command notes its key in the saved habits; with `settings.learn` off it notes nothing and `read_inputs` gives an empty `used`.

`settings/tests_aware.rs` (do as `settings/tests_general.rs` does to open Settings on a page):
6. The History page draws both switches, on by default; turning "Suggest the next step" off sets `settings.suggest` to false, and "Learn from what I do" likewise.
7. With nothing learned the third row says "Nothing learned yet" and its button is disabled; after two `aware::note_action` calls with different keys and one `aware::note_place`, it says "3 habits learned"; pressing the button empties the saved habits.
8. Searching Settings for "habits" finds the History page's rows (the test that checks `SEARCH` against drawn rows still passes with the three new entries).

## Done when

The palette's and the Settings' tests pass (old and new), clippy is clean.
