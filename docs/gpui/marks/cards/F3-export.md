# Card F3 — Export marks…, both formats

Wave 4 · crates `glint-core` and `glint-ui`

**You own:** `crates/glint-core/src/marks/export.rs` and `crates/glint-ui/src/dialogs/marks_export.rs` (new: `open(window, cx, folder)` is there and does nothing; the workspace calls it for `MenuEvent::ExportMarks`). Tests at the bottom of each.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M2, M7, M10), §4 (F24, F33), §5.1 to §5.4, §7.1. Then the code: `marks/export.rs` (`Format`, `marks_export`); `marks/mod.rs` (`find` 101 and `Sidecars` 81: an item's marks from the first home that has any, a folder's sidecar read once; `read_all` 119; `real` 71; a child module may call them, as `carry.rs` does); `marks/ledger.rs` (`shared`, `lines`); glint-model's `lines.rs` (`Export` 58, `Line` 24, `write_export` 272, `read_export` 250), `xyplorer.rs` (`XyItem` 54, `write` 212), `labels.rs` (`Label`, `label_of` 39); for the dialog `dialogs/comment.rs` (a small dialog with a backend, `open` 285), `dialogs/mod.rs` (`show`, `main_action`), `dialogs/backup.rs:64` and `:118` (asking Windows for a place), `core_bridge::run`.
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

**Check with:** `cargo test -p glint-core marks::export`, `cargo test -p glint-ui dialogs::marks_export`, and `cargo clippy -p glint-core -p glint-ui --all-targets -- -D warnings`.

## export.rs

`marks_export(root, to, format)` keeps its signature and calls a new
`pub async fn marks_export_with(root: String, to: String, format: Format, labels: Vec<Label>, stop: Arc<AtomicBool>) -> Result<u64, String>`
with `default_labels()` and a flag nobody sets (core has no settings: the interface hands the labels in).

- **Everything marked below `root`**, `root`'s own children and deeper: walked on core's threads (`blocking`, `rayon` for the reads), **never following a link or a junction**, never leaving `root`. An item's marks are read as `marks::read` reads them: its streams, else its line beside it (each folder's `.glint-marks` read once), else the ledger's; and the ledger's lines under `root` whose item isn't there are exported too (they wait). A `.glint-marks` itself is not an item.
- **`Format::Glint`**: `lines::write_export(&Export { labels, lines })`: the labels in the first line, then a line an item, its name its path under `root` with `\`, sorted by path.
- **`Format::Xyplorer`**: `xyplorer::write(&items, &labels)`: version 5, storage 3. An item's `label` is its label's place in `labels`, from 1, by `label_of` (so `was` counts); a label the list lacks is added at its end for this file (gray `B5B5B5`), up to what the format takes, and say what that is in a comment.
- **Written whole or not at all**: to a temporary file beside `to` that then takes its name. `stop` set ends it with `Err("Stopped")` and leaves nothing behind. `to` inside `root` is fine.
- **XYplorer's files are never written** (M7): a `to` whose name is `tag.dat` (any case) is refused: `Err("Glint never writes XYplorer's own tag.dat: choose another name, and load it there with tagload")`.
- The answer is how many items were written. A folder that can't be read is skipped, not an error.

## The dialog

`open(window, cx, folder)`, out of nothing in particular (`dialogs::show`), title `t!("Export marks")`. A `Backend` trait in the file (`pick_save(owner, folder, name) -> Option<String>`, `export(root, to, format, labels, stop) -> Result<u64, String>`), `CoreBackend` through `core_bridge::run`, `open_with` for tests.

1. One line: `t!("Everything marked in “{name}” and below it, into one file.")`.
2. **The format**, two choices in a radio group: `t!("Glint")` with `t!("Text, one line an item: read by Import marks… on any PC")`, and `t!("XYplorer")` with `t!("An XYplorerTag.dat that XYplorer's tagload merges")`.
3. **Where**: the file's path with `t!("Choose…")`. It starts as `<folder>\<folder's name>.glint-marks.jsonl`, or `<folder>\XYplorerTag.dat` for XYplorer's; changing the format changes the name while the person hasn't chosen one. (`.glint-marks.jsonl` is the name a menu knows an export by: card P1.)
4. **`t!("Export")`** (the main action). While it runs: `t!("Exporting…")` and **`t!("Stop")`**, which sets the flag. When done the dialog says `t!("Exported the marks of {n}")` (`plural(n, "item")`), or `t!("Nothing below “{name}” is marked")` for 0 (and no file is left), with **Close**; an error is its words.
5. The labels handed to core are `state::settings(cx).labels`.

## Tests

export.rs (folders of the test's own; a drive without streams is told by a `World` as `mod.rs`'s tests do, or skip the stream half where the temp folder keeps none):
1. A tree of three folders with a label, tags and a comment on files and on a folder exports every one, by its path under the root with `\`, sorted; reading the file back with `read_export` gives the same marks and the labels handed in.
2. All three homes: an item marked in its streams, one in a `.glint-marks`, one in the ledger (not there on disk) are all in the export; the `.glint-marks` is not an item.
3. XYplorer's format reads back with `xyplorer::read`: version 5, storage 3, the same paths, each label's number its place; a label the list lacks is added at the end; a comment's line break is `¶`.
4. A junction or symbolic link inside the root is not walked into (skip the test where one can't be made, and say so).
5. `stop` set before it starts answers `Stopped` and leaves no file and no temporary file.
6. `to` named `TAG.DAT` is refused and nothing is written.
7. Nothing marked: `Ok(0)` and no file.
8. An Arabic name, tag and comment come through both formats.
9. `#[ignore]`: 10,000 marked files in 100 folders: print the milliseconds of each format (the report wants it).

The dialog (a fake backend):
10. It opens on Glint's format with the `.glint-marks.jsonl` name; choosing XYplorer changes the name to `XYplorerTag.dat`; a name the person chose stays.
11. *Export* asks the backend once with the folder, the path, the format and the settings' labels; the answer 12 shows `Exported the marks of 12 items`; 0 shows the other line; an error its words.
12. *Stop* sets the flag the backend was given.
13. Everything pressable has a role and a name; Enter exports, Escape closes; an Arabic folder name mirrors.

## Done when

The tests pass, clippy is clean for both crates, `marks_export` keeps its signature, and your two files are the only ones changed. Say in your report every new string, test 9's times, how many labels XYplorer's format takes, and that nobody has seen the dialog on a screen.
