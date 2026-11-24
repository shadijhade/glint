# Card F2a — Import marks…: the preview, how far, what happened

Wave 4 · crate `glint-ui` · one new dialog

**You own:** `crates/glint-ui/src/dialogs/marks_import.rs` (new: `open(window, cx, source)` is there and does nothing; the workspace calls it for `MenuEvent::ImportMarks`; tests at its bottom). Nothing else.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M6, M7, M11), §5.5, §7.2 (F27 to F30) and §7.3 as they are now (they changed on November 24), §7.4. STANDARDS §5.1 "Models" and "Tests" matter here. Then the code: glint-core's `marks/import.rs` whole (the types you show: `ImportOptions` with `have`, `ImportPlan` with `needs_base`, `LabelPlan` with `takes_name` and an empty `to` for *as a tag*, `Planned`, `At`, `Progress`, `ImportReport`); `dialogs/comment.rs` (a small dialog with a backend: `open` 285, how it is shown); `dialogs/mod.rs` (`show` 185, `main_action` 119, `enter_runs_main` 159, `confirm` 264); `dialogs/compress_in_place.rs` (`remember_undo` and `offer_undo`, about 204 and 211: a step remembered from a dialog, and a notice with **Undo**; and how a long run of a dialog shows how far it is); `dialogs/backup.rs:64` and `:118` (a backend that asks Windows for a file or folder); `core_bridge.rs` (`run` 335, `stream` 344: a `Channel` whose messages arrive on a receiver); `explorer/ops_types.rs` (`UndoAction::Marks` and `Sequence`, 129 to 160); `dialogs/tags.rs` (`hub`, `TagsChanged`); `state.rs` (`settings`, `update`).
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

**Check with:** `cargo test -p glint-ui dialogs::marks_import` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient.)

**What is there for you:** glint-core's `import_plan` and `import_apply` **answer an error until cards F1a and F1b are in, at the same time as you work**. Everything here is tested against a fake backend; the real one only passes the call on.

## What to build

A `Backend` trait in your file (boxed futures, as `dialogs/tags.rs`'s): `pick(owner) -> Option<String>` (Windows' open-file dialog, for `open(.., None)`), `pick_folder(owner)`, `plan(source, options) -> Result<ImportPlan, String>`, `apply(plan, progress: Channel<Progress>) -> Result<ImportReport, String>`. `CoreBackend` calls core through `core_bridge::run`. `open_with(window, cx, source, backend)` for tests.

**It opens out of nothing in particular** (`dialogs::show`), reads nothing on the UI thread, and **every number it shows comes from the plan core answers**.

1. **While the plan is read**: the title and nothing else but a quiet `t!("Reading…")` (no numbers yet: M4). An error is its words, with **Close**.
2. **The source's line** (F28): `t!("{name}, version {v}, changed {when} · {items} · {labels} · {tags} · {comments} · {extras}")`, each part made with `plural` and `format_count` (`1,284 items`, `7 labels`, `93 tags`, `412 comments`, `3 extra columns`); the last part only when there are extras; `version {v}` is left out for a Glint export (version 0); a version above 5 adds `t!("A newer format than Glint knows: read by the same rules")`.
3. **Labels**: a row a label of the source: its dot in its own color (`label_tone`), its name, and what it becomes: `t!("is {label}")` by name; `t!("is {label}, which Glint will then call {name}")` when `takes_name`; `t!("New label")` when `new`; `t!("comes in as a tag")` when `to` is empty. **Each can be pointed at another**: a menu of Glint's labels, *New label* and *As a tag*; choosing puts `(name, to)` in `options.labels` and plans again.
4. **Extra columns**, when there are any: each with what it becomes (7.4: `t!("becomes the tag {caption}")`, `t!("becomes tags like {caption}: 4")`, `t!("becomes a line of the comment")`, `t!("left out")` for an icon), and one switch `t!("Bring extra columns in")` (on) that plans again.
5. **Where the items are**, by outcome, each a line with its count, only those that have any: `t!("Here")`, `t!("Moved since: found where they are now")`, `t!("On a drive that is not connected: they wait")` (with the drives' names), `t!("Not found: they wait")`, and `t!("Already marked in Glint")` with the choice `t!("Fill what is empty")` (the default) or `t!("The file's marks replace Glint's")`. When `needs_base`: a line `t!("Glint couldn't tell where XYplorer is")` with `t!("Choose its folder…")`, which asks for a folder and plans again with `options.base`.
6. **`t!("Keep following this file")`**, a switch, only for an XYplorer file named `tag.dat`, off until chosen: applied to `settings.follow_xyplorer` when the import is applied.
7. **The buttons**: `t!("Import")` (the main action; off while nothing would change) and **Cancel**. Escape cancels.
8. **Applying**: the dialog closes and how far it is shows as other long runs show it (look at how `compress_in_place.rs` does it and do the same; say which you used), fed by the `Progress` channel.
9. **When it ends**: labels first, **in the settings**: a `LabelPlan` with `new` is added (name, color; never past `MAX_LABELS`), one with `takes_name` renames Glint's label `to` to the source's name with the old name pushed to `was` (PLAN 5.5). Then `TagsChanged` for the befores' paths, **one Undo step** (`remember_undo(t!("import of {what}"), UndoAction::Marks(report.befores))`; if a `Sequence` can hold the labels' change too, use it, else say in your report that Undo leaves the labels), and the notice: `t!("Marked {marked} · {waiting} waiting · {missing} not found")` (parts with 0 left out; `t!("Nothing to bring in: everything is marked already")` when all are 0), with **Undo** and **`t!("What happened")`**.
10. **What happened**: a dialog listing `report.items` (every item with its outcome), in a list that draws only what is on screen, with `t!("Copy")` (the lines, tab between path and outcome, to the clipboard) and **Close**.

## Tests (STANDARDS §5.1 "Tests"; a fake backend that answers plans made by hand, writes down what it was asked, and can fail)

1. With a plan of 1,284 items the source's line reads as F28's, with the numbers of the plan; a Glint export's has no version.
2. The labels' rows: by name, `takes_name`, new, as a tag: each with its words. Pointing *红色* at *Blue* plans again with `options.labels == [("红色", "Blue")]`.
3. The extras' switch plans again with `options.extras == false`; a plan without extras draws no such section.
4. Only the outcomes that have items are drawn; choosing *The file's marks replace Glint's* plans again with `Meet::Replace`.
5. `needs_base` shows its line; choosing a folder (the fake's) plans again with `options.base`.
6. *Import* asks `apply` once with the plan shown; when it ends the hub says `TagsChanged` for the befores' paths, the explorer has one more Undo step whose action holds the befores, and the notice has the three numbers, **Undo** and **What happened**.
7. Labels: a new label is in the settings afterwards; `takes_name` leaves *红色* with `was == ["Red"]`; with 32 labels none is added.
8. The follow switch shows only for `tag.dat`, and on, sets `follow_xyplorer` when the import is applied, not before.
9. A plan that fails shows its words; an apply that fails says `t!("Couldn't import the marks")` with the reason, and no step is remembered.
10. *What happened* lists every item; **Copy** puts the lines on the (test) clipboard.
11. Everything pressable has a role and a name; Escape closes; with Arabic labels and paths the dialog mirrors and nothing overlaps.
12. `open(.., None)` asks the backend for a file first, and does nothing when none is chosen.

## Done when

The tests pass, clippy is clean, your file is the only one changed, and the dialog never calls glint-core from the UI thread. Say in your report every new string, how progress is shown, whether Undo takes the labels back, and that nobody has seen the dialog on a screen.
