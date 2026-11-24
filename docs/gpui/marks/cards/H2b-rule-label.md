# Card H2b — A folder rule that labels

Wave 5 · crates `glint-model`, `glint-core`, `glint-ui` · a rule's action, its run, and its editor

**You own:** in `crates/glint-model/src/store.rs`, `RuleAction` (:638), `FolderRule` (:688), its `Default` (:714) and its `lenient_deserialize!` (:988), and their tests; `crates/glint-core/src/rules.rs` whole; `crates/glint-ui/src/dialogs/rules.rs` whole. Nothing else (`persistence.rs` and `a11y_tests.rs` build `FolderRule`s with `..Default::default()`: they keep compiling if every new field has a default).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), §5.1 to §5.5, §8 and §8.1 (the last item, "Landed before wave 5's cards"). Then the code: `store.rs` :635–:733 and :988–:1003, and the rules' tests (:1493); core's `rules.rs` whole (`Rule` :23, `Done` :53 with its `mark`, `plan` :202 and the `"unblock"` line :220 that words an action, `act` :337, `note` :382 and `made`, `matching` :399, `check` :423, `tick` :490, `sweep` :544, `rules_preview` :660, `rules_apply` :688, `rules_undo` :722); glint-ui's `dialogs/rules.rs` (`to_core` :75, `actions()` :193, `summary` :243, `did_verb` :305, `blank` :393, the editor `RuleEditor` :563 to its `render` :1248, the `.when(self.action == RuleAction::Move` rows :1351, the tests from :2078). glint-core's `marks::read_many` and `set_many`; glint-model's `Change`, `Marks`. Settings' labels: `state::settings(cx).labels`; a label's own color: `theme::label_tone`.
**Check with:** `cargo test -p glint-model store::`, `cargo test -p glint-core rules::`, `cargo test -p glint-ui dialogs::rules`, and `cargo clippy -p <crate> --all-targets -- -D warnings` for the three. glint-ui builds slowly and other agents wait on the same lock: build by your filter.

## glint-model

- `RuleAction::Label`, saved as `"label"` (`as_str` too).
- `FolderRule` gains `pub replace: bool` (saved as `"replace"`, left out while false; `lenient_deserialize!` reads it; `false` by default): a Label rule replaces a label a file has only when this is on. A saved rule from 0.16.1 (no `replace`, no `label`) reads unchanged (a test).
- The label is the rule's `target` (its name). Say so in `target`'s doc comment.

## glint-core's `rules.rs`

- `Rule` gains `replace: bool`; its `action` doc names `"label"`.
- `plan`: `"label"` → `Ok((format!("Label it {target}"), None))` (with `replace`: `"Label it {target}, in place of a label it has"`); an empty target: `Err("Which label? Choose one in the rule")`. `check` refuses that rule the same way.
- **Labeling goes through `marks::set_many` with `Change::Label(Some(target))` in the batches the matches come in, never one call a file**: one `read_many` of the batch, then one `set_many` of those that change. Add a function (e.g. `label_all(rule, files) -> Vec<Done>`) that `tick` (gather the files that went to label rules in this tick, by rule, and call it once a rule after the loop), `sweep` and `rules_apply` call for a Label rule in place of `act` a file.
- **Fill what is empty**: a file with a label already is left as it is unless `replace` (as the import does; a 0.16 color tag drawn as a label is no `label`: the file has none). **A rule that runs again changes nothing**: a file whose label is already the rule's is not written and not logged; no `Done` for it.
- A `Done` for each file that changed: `action: "label"`, `to: Some(from)` (so `made` keeps the rule from taking it again when the stream's write wakes the watcher), `mark`: the label it had (`None` for none). An `Outcome` with `after: Err(why)` is a `Done` with that error.
- `rules_preview`: for a Label rule, the matches that would change (one `read_many`), each with `plan`'s words.
- `rules_undo` `"label"`: puts back the label it had (`Change::Label(mark)`) with `set_many`, **only if the file's label is still the rule's** (else `Err("Its label has changed since")`); a failed outcome is its words.

## glint-ui's `dialogs/rules.rs`

- `to_core` passes `replace`.
- `actions()` gains `(RuleAction::Label, t!("Give it a label"))` (the array's length changes; put it after Rename). `summary`: a Label rule reads ` {label}` as Move reads its folder. `did_verb("label")` → `t!("labeled")`.
- The editor, while the action is Label: **the labels from Settings** as a row of choices (one a label, each a button with its name and a dot in `theme::label_tone(color)`, the chosen one checked as the dialog's other choices are, keyboard alone works), choosing one sets `target`; a switch **Replace a label a file already has** (`replace`, off by default). **With no labels in Settings**: the words `t!("There are no labels yet: Settings → Files → Labels makes them")`, and the rule can't be turned on.
- **A label is not a state** (M11): the dot is the label's color; the chosen one shows by the kit's checked state, never by the label's color or the accent standing in for it.
- The kit and the theme only (no `gpui_component` in the view, no color or size of its own); every string through `t!`; everything that can be pressed has a role and a name.

## Tests

Model and core: plain `#[test]`; core's on files in a temp folder of the test's own (labels are written to the files' streams there; check them with `read_many`). UI: STANDARDS §5.1 "Tests", the file's `Fake` backend, `testing::lock()`, never a real disk.

1. `RuleAction::Label` saves as `"label"` and reads back; `replace` is left out while false and read when there; a 0.16.1 rule reads unchanged.
2. `plan` words a Label rule, with and without `replace`; an empty target is refused by `plan` and `check`.
3. `rules_apply` (or the batch function) on a folder of five files, two of them labeled already: three get the rule's label, the two keep theirs; with `replace` all five have it; run again: nothing changes and nothing is logged.
4. The batch is one `set_many` (count the calls through a seam, or show it by a folder on a drive without streams writing its `.glint-marks` once: say which).
5. Undo of a label a rule gave puts back what was there (none, or the old label); after the person changed it, Undo refuses with words.
6. The editor: choosing Label shows Settings' labels as named buttons and the switch; choosing one sets the rule's `target`; with no labels, the words show and the rule can't be turned on. The test that lists the actions changes (say so).
7. `summary` and `did_what` word a Label rule.
8. Right to left: an Arabic label name (`مهم`) in the editor and the summary, with the mirrored layout (as `arabic_names_and_the_mirrored_layout_render_without_trouble` does).
9. **Measured** (`#[ignore]`, prints): a Label rule applied to 1,000 new files in one folder; give the time (debug build) in the report.

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no `dir /s` from a root, no `Get-ChildItem -Recurse` outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. If a build has waited ten minutes on Cargo's lock, say so in your report rather than waiting on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug, and don't trust a timing from such a window.
- **Backslashes**: edit Rust with the editing tools, never a script typed into a shell.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no end-to-end checks, no real AI agent, no installed Glint.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. If you meet it, say so and go on.

## Done when

The tests pass in the three crates, clippy is clean for them, and every other action does what it did. In your report: every string you added for `t!`, the measurement, the tests you changed, and anything in `persistence.rs` or elsewhere that would need a line for the new field (you don't edit it).
