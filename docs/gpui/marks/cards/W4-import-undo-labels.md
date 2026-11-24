# Card W4 — The import's Undo and following, with their labels

Wave 6, round 2 · crates `glint-core` and `glint-ui` · the import's afterwards

**You own:** `crates/glint-core/src/marks/apply.rs`; `crates/glint-ui/src/dialogs/marks_import.rs`; `crates/glint-ui/src/marks_follow.rs`; `crates/glint-ui/src/explorer/ops_types.rs` (`UndoAction`, `UndoRun`); `crates/glint-ui/src/explorer/undo.rs` (running the new run); in `crates/glint-ui/src/components/settings/labels.rs` the take back (`Source::take_back` :166/:177, `take_back` :559) and `settings/tests_labels.rs`. Their tests. Exhaustive matches on `UndoAction`/`UndoRun` are only in `ops_types.rs` and `undo.rs` (checked): a new variant is yours to add.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), **§5.5**, **§7.3** (Applying, Following), F29, F30, §10, and under §9's table "Wave 3, as it landed", "Wave 4, as it landed" and "Wave 5, as it landed".
Then the code: `apply.rs` whole (`LastImport` :33, `Memory`/`Source`/`Opts` :58–:96 with `have`, `journal_line` :249, `apply_in` :294, `apply` :448, `last_import_in` :470 and the journal's first line, `take_back_in` :497, `changed_source` :535, `follow` :550, `follow_plan` :560, the tests); `marks_import.rs` (`apply_labels` :237, where the plan's labels go into Settings :561, the Undo step :576, the module comment :16 that says labels stay); `marks_follow.rs` whole (`init` :43, `ask` :95, `answered` :122); `ops_types.rs` `UndoAction` :129–:240; `undo.rs` (the run of `UndoRun::Marks` :173, the tests' `marks_step` :538); `settings/labels.rs` :160–:600; `state.rs` (how Settings are changed: `state::update`).
**Check with:** `cargo test -p glint-core marks::apply`, `cargo test -p glint-ui dialogs::marks_import`, `cargo test -p glint-ui marks_follow`, `cargo test -p glint-ui explorer::undo`, `cargo test -p glint-ui settings::tests_labels`, clippy for both crates. glint-ui builds slowly and two other agents build it now.

## Undo of an import puts the labels back too

- Today the step holds marks alone (`UndoAction::Marks`), so the labels an import renamed or added in Settings stay (wave 4's record).
- `UndoAction::Labels(Vec<Label>)` (the whole list as it was before the import) with `UndoRun::Labels`, run in the interface (`Explorer::undo`: Settings' labels become that list, through `state::update`); not a file operation (`op()` None, `ops()` leaves it out), names no renamed folder. The import's step becomes `UndoAction::Sequence(vec![Labels(before), Marks(befores)])` when the import changed the labels, else `Marks` as now.
- Undo puts back the labels **only while they are still what the import made them** (else the person changed them since: leave them, and say nothing more than the Undo's own words). Decide and say.

## *Take back the last import* puts them back too

- The journal's first line (`last_import_in` reads `source` and `at` from it) also keeps `labels`: the labels as they were before the import (written by `apply`; the interface hands them in: an `ImportOptions` field already carries `have`, the PC's labels when the plan was made; use it if it is the same list, and say so).
- `LastImport` gains `labels: Option<Vec<Label>>` (None for a journal written before). Settings' take back puts them back as Undo does (same rule), after the marks.

## Following plans with the labels as they are now

- Today following plans with `have` as it was at the import (`Opts.have`), and doesn't add to Settings a label XYplorer gained since.
- `follow_with(have: Vec<Label>)` (keep `follow()` if anything outside your files calls it: check, else replace it): plans with the PC's labels now. `Followed` gains the plan's `labels: Vec<LabelPlan>`; `marks_follow.rs` hands Settings' labels in and adds a new one as the import does (`apply_labels`, which you may move or make `pub(crate)` since both files are yours).

**This card draws**: STANDARDS §5 is the law. The kit and the theme only (no `gpui_component` in a view, no color or size of its own); a label's color from `theme::label_tone`, never the accent (M11: a label is not a state); every string through `t!`; everything that can be pressed has a role and a name, and keyboard alone works; right to left tested with an Arabic name; nothing on the UI thread but reading what is in memory; a test that lists rows changes when a row is added (say so).

## Tests

Core: plain `#[test]` with the `World` and data folder of the test's own (as the file's tests do). UI: STANDARDS §5.1 "Tests", the fakes there are, never a real disk.

1. An import that renamed Red to 红色 and added 天蓝色: Undo puts the files' marks and Settings' labels back as before (one step, one notice).
2. Changed in Settings after the import: Undo leaves the labels as the person made them, and still puts the marks back.
3. An import that changed no label makes a `Marks` step as before (the tests that are there pass).
4. The journal's first line keeps the labels before; `last_import` reads them; a journal written before reads with `labels: None`.
5. *Take back the last import* puts back marks and labels (Settings' row); a journal without labels puts back the marks alone.
6. Following with a label XYplorer gained since: the plan names it new, `marks_follow` adds it to Settings, the files get it.
7. Following plans with Settings' labels now: a label renamed in Glint since the import is matched by its new name and `was`.
8. `UndoAction::Labels` is not a file operation, runs as one `UndoRun::Labels`, names no renamed folder, takes its turn in a `Sequence`.
9. Right to left: an Arabic label (`مهم`) through Undo and the Settings row.

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

Tests pass in both crates, clippy is clean for them, signatures outside your files unchanged. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus NEW STRINGS (every `t!` string added or changed), MEASURED (if the card asks), TESTS CHANGED (every existing test you changed, and why).
