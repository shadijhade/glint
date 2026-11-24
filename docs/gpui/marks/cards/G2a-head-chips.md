# Card G2a — The pane's head: a chip a label, and *marks here are beside the files*

Wave 4 · crate `glint-ui`

**You own:** `crates/glint-ui/src/components/pane_head/mod.rs` and `pane_head/tests.rs`. Not `lenses.rs`, `pill.rs`, or the status bar's `summary.rs`.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M5, M11), §4 (F5, F23), §5.2, §5.6, §6.6 (its last paragraph: marks from the index's memory). Then the code: `pane_head/mod.rs` whole (`Head` 43, `Head::of` 125, `Probes` 195 and `Probes::core` 204, `PaneHead` 213, `refresh` 276, `ask` 297, `toggle` 463, `chip` 473, `fitting` 516, `render` 540 with the chips' row at 608); `status_bar/summary.rs` (`chip_active`, `chip_title`: how a kind's chip knows it is lit by the search box's text); `components/row_facts.rs` (`marks_of`, `marks_among`, `ask_all`, `has_all`; its `fake::MarksBackend`); `explorer/marked.rs` (who asks `ask_all` today, and the key it uses: the tab's `view_key()`); `dialogs/tags.rs` (`hub`, `TagsChanged`); glint-core's `marks::{marks_in, MarkedIn, marks_beside, marks_move_in}` (`marks/mod.rs`: the first is whole, the other two answer 0 until card B4 is in, at the same time as you work); glint-model's `marks::{shown, Label, Marks}`; `theme::label_tone`.
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

**Check with:** `cargo test -p glint-ui components::pane_head` and `cargo clippy -p glint-ui --all-targets -- -D warnings`.

## A chip a label (F5)

- `Head` gains `labels: Vec<LabelChip { name, color, count }>`: the labels the folder's items are **shown** with (`marks::shown(settings' labels, marks).label`: a renamed label by its name now, a 0.16 color tag as its label), in the settings' order, then labels this PC lacks by name, each with how many items.
- **Shown only when known** (M4), from the first of:
  1. **the index's memory**: `Probes` gains `marks_in` (a boxed async fn from a folder to `Option<Vec<MarkedIn>>`), asked when the head's folder changes and when `TagsChanged` names something in it. `Some` is the whole answer, at once.
  2. **else the rows' own marks**, for a folder of **up to 5,000 items**: `RowFacts::ask_all` for the tab (as `explorer/marked.rs` asks it, with the same key), and the counts once `has_all`. Over 5,000 and not indexed: no label chips.
  A search's results, the Recycle Bin, an archive, a remote: no label chips.
- **The chip**: the label's dot in `label_tone`, its name and its count (`Important 4`), after the chips for kinds, in the same row and by the same `fitting` (kinds first; labels take what room is left). Its accessible name is `t!("Label {name}: {n}")`. Lit or not is the kit's own state for a chip, as a kind's: **the label's color is the dot's alone**.
- **Pressed**, it puts `label:"Name"` in the search box; pressed again, takes it out (`toggle`, as a kind's chip; lit while the box holds exactly that). Other words in the box stay as a kind's chip leaves them.

## *12 marks here are beside the files* (F23)

- `Probes` gains `beside` (folder to how many: `marks_beside`) and `move_in` (folder to how many moved: `marks_move_in`). Asked for a real folder when it is shown and again after `TagsChanged`; 0 draws nothing.
- Under the chips, one quiet line: `t!("{n} here are beside the files")` with `n` as `plural(count, "mark")`, and a button **`t!("Move them into the files")`**. Pressed: `move_in`, then `TagsChanged` on the hub for the folder's items, and a notice `t!("Moved the marks of {n} into the files")` (`plural(n, "item")`); an error is its words. While it runs the button is off.
- **No Undo step**: the marks are what they were, only where they are kept changed, and a marks step can only put marks, which would write them into the files again. Say so in your report.

## Tests (`pane_head/tests.rs`; fake probes, `row_facts`'s `fake::MarksBackend`)

1. With `marks_in` answering four *Important* and two *Pending*, the head shows `Important 4` and `Pending 2` after the kinds' chips, in the settings' order, each with a role and its name; `ask_all` is never asked.
2. With `marks_in` answering `None` and a folder of 100 items, `ask_all` is asked and the chips show once it lands; with 5,001 items nothing is asked and no label chip shows.
3. Until an answer is in, no label chip is drawn (and no placeholder).
4. A file tagged Red with no label counts under Red (5.6); a file labeled *Red* where Red was renamed *Important* counts under Important.
5. Pressing `Important 4` puts `label:"Important"` in the box and the chip is lit; again takes it out; a kind's chip beside it still works.
6. `TagsChanged` for an item of the folder asks again and the count changes.
7. In a pane too narrow, kinds keep their chips and labels take what is left (`fitting`).
8. `beside` answering 12 shows the line and the button; pressing asks `move_in` once, says the notice with what it answered and `TagsChanged`; 0 shows nothing; a search's results never ask.
9. With an Arabic label name the chips mirror and nothing overlaps.
10. The head's tests there are pass unchanged.
11. `#[ignore]`: a folder of 5,000 items, 500 marked, through `has_all`: print the milliseconds from the marks being in to the chips being worked out (the report wants it).

## Done when

The tests pass, clippy is clean, your two files are the only ones changed, and the head reads nothing from the UI thread. Say in your report every new string, test 11's time, and that nobody has seen it on a screen.
