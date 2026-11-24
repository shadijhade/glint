# Card G1c — The settings file's part: Labels

Wave 4 · crate `glint-ui` · small

**You own:** `crates/glint-ui/src/components/settings/transfer.rs` and **marked places** in `crates/glint-ui/src/persistence.rs`: `ALL_PARTS` and `PARTS` (713, 725), the struct `export_settings` writes and the function (798), `SettingsFile` with `part_value` and `count_of` (862, 878), `known_settings` (912) and `import_settings` (966), and their tests. Nothing else of `persistence.rs`.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §4 (F26), §5.5, §8.1 ("Settings", "Columns": `Part::Labels` exists and is in neither list). Then the code: `persistence.rs` from 690 to 1000 and the tests of the settings file below it; `recent_tags` and `set_recent_tags` (639, 644); `transfer.rs` whole (`part_label` 17, `part_note` 32, `slot` 46, `offer` 184, `take` 193: `Part::Labels` is in all three matches already); `settings/about.rs` (where the transfer is drawn); glint-model's `marks::{Label, MAX_LABELS}` and `label_of`.
**Check with:** `cargo test -p glint-ui persistence::`, `cargo test -p glint-ui components::settings` and `cargo clippy -p glint-ui --all-targets -- -D warnings`.

## What to build

- **`Part::Labels` is a part like the others**: in `ALL_PARTS` and `PARTS` (`"Labels"`, `Some("with the tags used lately")`), last.
- **Written**: `"labels": { "labels": [ … ], "recentTags": [ … ] }`: the settings' labels as they are (name, color, `was` when not empty) and the tags used lately.
- **The Settings part no longer carries `labels`** when it is written (8.1 left that to G1): one home in the file. Read, a Settings part that has `labels` (a file from before this card) still applies them when the file has no Labels part.
- **Read** (`import_settings`), everything from the file being data (M10): a label with an empty name, a name over 64 characters or with a line break is left out; a color is masked to 24 bits; at most `MAX_LABELS`; names alike in any case are one. **Replace** puts the file's labels in place of the settings'; **merge** keeps the settings' labels and adds the file's that no label is or was called, while there is room. The recent tags are added to the front of the PC's, one of each in any case, as many as the PC keeps.
- `count_of(Part::Labels)` is the number of labels in the file, so the part is offered only when there are some.
- The words `import_settings` answers gain a line for the part, in the way the others have theirs.
- `transfer.rs`: whatever the part needs to be ticked, written and taken as the others are (most is there).

## Tests

1. An export with the Labels part ticked has the labels (a renamed one with its `was`) and the recent tags, and its Settings part has no `labels` key; without the part ticked the file has neither.
2. Import, replace: the settings' labels are the file's. Merge: the PC's stay, a new one is added, one the PC has (or had: `was`) is not added twice, and nothing is added past 32.
3. A file from before this card (labels inside its Settings part, no Labels part) still brings its labels with the Settings part.
4. Damaged: a label with no name, a name of 10,000 characters, a color of `4294967295`, `labels` that is a string, 200 labels: nothing panics and what is kept is valid.
5. The recent tags come in at the front, one of each.
6. The transfer offers the part when the file has labels and not otherwise; the tests the two files have pass unchanged, but those that count the parts (8 becomes 9): change them, and say so.

## Done when

The tests pass, clippy is clean, and only the marked places of `persistence.rs` changed. Say in your report every new string and every test you changed.
