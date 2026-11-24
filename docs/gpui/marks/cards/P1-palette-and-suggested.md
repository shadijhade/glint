# Card P1 — *Import marks…* and *Export marks…* in `Ctrl+K`, and in a marks file's own menu

Wave 4 · crate `glint-ui` · small

**You own:** `crates/glint-ui/src/components/palette/commands.rs` and `palette/tests/marks.rs`; `crates/glint-ui/src/components/menus/smart.rs`, and **the one marked place** where a menu's Suggested section is given rows of its own for the item it is about (find it from `smart::dress`'s and `smart::zone`'s `extra`: say in your report which file and lines), with the menus' tests that speak of it (`menus/tests_smart.rs`, and any that lists a menu's rows: change them, and say so).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §4 (F9, F24, F25), §7.2 (F27). Then the code: `menus/event.rs` (`MenuEvent::ImportMarks { source }` and `ExportMarks { folder }`, landed for you with their keys `do:import-marks` and `do:export-marks` in `smart.rs`'s `event_key`; `workspace.rs` hands both to the dialogs, which cards F2a and F3 fill at the same time as you work); `palette/commands.rs` (`Inputs` 238, the marks' commands about 1490 to 1540: `menu("copy-marks", …)`, how a command is a `MenuEvent`, when they are offered; `all` 1865, `all_with` 1870); `palette/tests/marks.rs`; `menus/smart.rs` whole (`zone` 385, `dress` 419); `menus/item.rs` (how a menu for one file is built, and what it knows of the file: its name, never its contents).
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

**Check with:** `cargo test -p glint-ui components::palette`, `cargo test -p glint-ui components::menus`, and `cargo clippy -p glint-ui --all-targets -- -D warnings`.

## Ctrl+K (F9's last two)

- **`t!("Import marks…")`** (id `import-marks`): always offered; `MenuEvent::ImportMarks { source: None }` (the dialog asks for a file), or `Some(path)` when exactly one file is selected and its name is a marks file's (below).
- **`t!("Export marks…")`** (id `export-marks`): offered where the marks' other commands are (not in an archive, on a remote, in the Recycle Bin, or in a search's results); `MenuEvent::ExportMarks { folder }`, the folder being the one selected folder, else the folder the tab shows.
- Each with an icon the kit has (say which) and words a search finds them by (`xyplorer`, `tag.dat`, `labels tags comments`).

## A marks file's own menu (F25)

A file is a marks file **by its name alone** (a menu reads nothing): `tag.dat`, `XYplorerTag.dat` (any case), or a name ending in `.glint-marks.jsonl` (what Export marks… names its file). Write `pub fn is_marks_file(name: &str) -> bool` in `smart.rs`; the palette uses it too.

Its menu offers **`t!("Import marks…")`** under **Suggested**, first there, with `MenuEvent::ImportMarks { source: Some(path) }`: as a row Suggested gets of its own for this item (the `extra` rows), since the menu's own rows have no such row to copy. Only for one file on this PC's disks (not in an archive, on a remote, in the Recycle Bin).

## Tests

1. The palette lists *Import marks…* with nothing selected (`source: None`); with `tag.dat` selected its event carries the path; with `notes.txt` selected, `None`.
2. *Export marks…* carries the tab's folder; with one folder selected, that folder; it is not offered in an archive, on a remote, in the Recycle Bin.
3. Both are found by typing `xyplorer`.
4. `is_marks_file`: `tag.dat`, `TAG.DAT`, `XYplorerTag.dat`, `Ops.glint-marks.jsonl` are; `tag.data`, `notes.jsonl`, `.glint-marks` are not.
5. The menu of `tag.dat` has *Import marks…* first under Suggested, with the file's path; the menu of `notes.txt` has no such row; two files selected, none; in an archive, none.
6. The row's key is `do:import-marks` and it has a role and a name.
7. The palette's and the menus' tests there are pass unchanged, but those that count Suggested's rows for a file named like a marks file (if any): say which changed.

## Done when

The tests pass, clippy is clean, and nothing outside your files and the one marked place changed. Say in your report every new string, the icons, and the marked place.
