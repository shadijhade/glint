# Card G1b — Settings → Files → Labels, and the marks' two rows

Wave 4 · crate `glint-ui`

**You own:** `crates/glint-ui/src/components/settings/labels.rs` (new: an empty module whose line is in `settings/mod.rs`), a new `settings/tests_labels.rs` with **its one `mod` line in `settings/mod.rs`** (beside `tests_general`), and **marked places**: in `settings/pages.rs` the sections of `Page::Files` (170) and the `SEARCH` entries for your rows; in `settings/general.rs` the arms of `section_of` (212) that draw your two sections. Nothing else of those files; not `transfer.rs` or `persistence.rs` (card G1c's).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M6, M11), §4 (F1, F26), §5.5, §7.2 (F29, F30), §8.1 ("Settings"). Then the code: `settings/general.rs` (`section_of` 212, `files` 305: how a section of rows is made, `flag` 167, `clear_with_undo` 127); `settings/widgets.rs` whole (`section`, the rows, `id`); `settings/pages.rs` (`sections` about 165, `of_section`, `SEARCH` and the test `the_search_index_names_rows_that_are_drawn`); `settings/tests_general.rs` (how a section is tested); `state.rs` (`settings`, `update`); glint-model's `marks/labels.rs` whole (`Label { name, color, was }`, `default_labels`, `label_of`) and `MAX_LABELS`; `theme.rs` (`label_tone` 665); glint-core's `marks::{last_import, import_take_back, LastImport}` (`marks/apply.rs`); `dialogs/tags.rs` (`hub`, `TagsChanged`).
## The rules for a card that draws (STANDARDS §5 is the law; these are where such cards went wrong before)

- **The kit and the theme only.** No `gpui_component` in a view, no color or size of its own. A label's own color is `theme::label_tone(label.color)`, a wash `theme::label_wash(label.color, cx)`.
- **A label is not a state** (M11): never in the accent, never the whole line's text in color. Anything with a selected state shows its selection alone.
- **Every string through `t!`**, as PLAN §4 and §7.2 word it where they word it. List every new string in your report.
- **Everything that can be pressed has a role and a name**; keyboard alone works (focus order, Enter, Escape).
- **Right to left:** start and end helpers only, and a test with an Arabic name, tag and comment in everything that lays text out (`testing::set_rtl`).
- **Nothing on the UI thread but reading what is in memory.** Marks come from `RowFacts` (`components/row_facts.rs`); changes go through `dialogs::tags::set_marks(window, cx, paths, change)` (Undo, `TagsChanged`), never straight to glint-core from a view. Core is reached through a source or backend the view takes (a boxed async fn, on `core_bridge::run`), so tests inject a fake.
- **Only what is known is drawn** (M4): no placeholder, no skeleton.
- A list's test window must be wide enough for its columns (`file_list::layout::leave_room` takes optional columns out while the name would have less than 14 rem).
- A test that lists a menu's rows or walks it with the arrows changes when a row is added: change it, and say so.

**Check with:** `cargo test -p glint-ui components::settings` and `cargo clippy -p glint-ui --all-targets -- -D warnings`.

**What is there for you:** core's `last_import` and `import_take_back` **answer nothing until card F1b is in, at the same time as you work**: they are reached through a source your tests replace.

## The edits, as pure functions first (in labels.rs, tested plainly)

Each takes the labels and answers the new list or the words of a refusal:

- **`rename(labels, at, name)`**: the old name goes to `was` (PLAN 5.5: **nothing on disk is rewritten**); a name that is in the label's own `was` comes out of it; white space trimmed. Refused: empty (`t!("Give the label a name")`), over 64 characters or with a line break, or a name another label has or had (any case: `t!("Another label is or was called that")`).
- **`recolor(labels, at, color)`**.
- **`add(labels)`**: a label `t!("New label")` (with a number while one is called that), in the first of the palette's colors no label has; refused at `MAX_LABELS`.
- **`move_by(labels, at, -1 | 1)`** and **`remove(labels, at)`** (the items that carry it keep its name and are drawn as a tag's color is: nothing is rewritten).

## The section `Labels` (slot `labels`, after `files` on the Files page)

One row a label: its dot (`label_tone`), pressed it opens a small popup of **twelve colors** to choose from (a fixed palette in this file: today's seven and five more; no free color), each a button named by its color; the name in a box that renames when the focus leaves it or on Enter (Escape puts it back; a refusal shows its words under the box and changes nothing); **Move up** and **Move down** buttons (off at the ends); **Remove**. Under the rows: `t!("Add a label")` (off at 32, with `t!("32 labels at most")`) and `t!("Back to the seven colors")`, which puts `default_labels()` back. Under the heading, one quiet line: `t!("A label is kept in the file by its name. Renaming one changes nothing on disk: files that say the old name show the new one.")`.

Every change is `state::update(cx, |s| s.labels = …)`, and each can be taken back the way `general.rs`'s `clear_with_undo` offers its own (a notice with **Undo** that puts the list back).

## The section `Marks` (slot `marks`, after `labels`)

- `t!("Keep following XYplorer")`: a switch on `settings.follow_xyplorer`, with `t!("When its tag.dat changes, what is new in it comes in as Glint starts or comes to the front")`.
- `t!("Take back the last import")`: a row with a button `t!("Take back")`. What it says comes from a source (`last_import()`, asked off the UI thread when the page opens): `t!("{n} from {name}, {when}")`, or `t!("Nothing was imported in the last 30 days")` with the button off. Pressed: the source's `import_take_back(journal)`, then `TagsChanged` for the paths that changed and a notice `t!("Took back the import of {n}")`; an error is its words.

Both sections are in `pages.rs`'s sections of the Files page and every row that can be searched is in `SEARCH` (the test there says which).

## Tests (`tests_labels.rs`, and the pure ones in labels.rs)

1. `rename`: Red to Important leaves `was == ["Red"]`; back to Red empties it; to another label's name or old name is refused; empty is refused; trimmed.
2. `add` gives a ninth label a name and a color no other has, and is refused at 32; `move_by` at the ends changes nothing; `remove` takes the one out.
3. The section draws a row a label in order, each with its name; renaming in the box and leaving it changes the settings; a refusal shows its words and the settings stay.
4. A color pressed in the popup recolors that label alone; the popup's twelve are buttons with names.
5. Move down on the first puts it second; Remove takes it out; each change's notice has **Undo**, which puts the list back.
6. *Add a label* adds one; at 32 it is off and says why; *Back to the seven colors* gives `default_labels()`.
7. The switch turns `follow_xyplorer` on and off.
8. With a last import the row says its numbers and the button asks the source once with its journal, then says the notice and `TagsChanged`; with none the button is off.
9. Both sections are found by the page's search; keyboard alone reaches every control; a label with an Arabic name mirrors and nothing overlaps.
10. The settings' tests there are pass unchanged (but any that lists the Files page's sections: change it for exactly the two, and say so).

## Done when

The tests pass, clippy is clean, and outside `labels.rs` and `tests_labels.rs` only the marked places changed. Say in your report every new string, the twelve colors, and that nobody has seen it on a screen.
