# Card E6 — Small things the look found: the grid's sort bar, Edit tags…' chips, a sort by label

Wave 4 · crates `glint-ui` and `glint-model` · three small things, each cut freely: say what you cut

**You own:** **marked places** only: in `crates/glint-ui/src/components/file_grid/mod.rs` the sort bar's columns (1426 to 1445) and the tests of it in `file_grid/tests.rs`; in `crates/glint-ui/src/dialogs/tags.rs` `offered` and `has_tag_name` (112 to 126) and the tests that speak of what the dialog offers; in `crates/glint-model/src/entries.rs` `entry_order_by` (428 to about 520) and its tests. Nothing else of those files.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §4 (F2, F4), §5.6, and under §9's table "Wave 3, as it landed" ("Found, and left for wave 6"; "Wave 6, speed"). Then the code named below.
**Check with:** `cargo test -p glint-model entries::`, `cargo test -p glint-ui components::file_grid`, `cargo test -p glint-ui dialogs::tags`, and `cargo clippy -p glint-model -p glint-ui --all-targets -- -D warnings`. (The seven tests of `components::color_wheel` and one of `file_grid::tile` end with an access violation when run **alone**; that is known and not yours: say so and go on.)

## 1. A sort by label makes each row's key once (glint-model)

`entry_order_by` (`entries.rs:428`) works out what a row is **shown** as (`shown(labels, marks)`, line 449) at every comparison: 318 ms for 10,000 marked rows in a test build. Make it once a row: the comparison it returns keeps what it worked out for an entry the first time it is asked (by the entry's address, in a map behind a `RefCell`, or however fits the function's bounds), and compares those. **The function's signature and what it orders stay exactly as they are**: glint-ui's `explorer/rows.rs` calls it and is not yours.

Tests: the ones the file has for the three marks' sorts pass unchanged (`:2124`, `:2145`); a new one counts how often a row's marks are asked for in a sort of 1,000 rows by label (once a row, not once a comparison: count through the `of` closure); an `#[ignore]` one prints the milliseconds of sorting 10,000 marked rows by label, before (say what you measured before your change) and after.

## 2. The grid's sort bar offers the marks (glint-ui)

`file_grid/mod.rs:1434` lists Name, the date, Date taken, Type and Size. Add **Label**, **Tags** and **Comment** (`glint_model::columns::MARK_SORTS`; the words the list's header uses for those columns: find them and use the same `t!` strings), after Type and before Size, and not for the Recycle Bin, the Gallery, an archive or a remote (where rows read no marks: see how the bar already leaves columns out). Tests: the bar lists them in a folder and not in the Gallery; pressing Label sorts by label; the bar's tests there are change only where they list every column, and say so.

## 3. Edit tags… no longer offers the seven colors as tags (glint-ui)

Colors are labels now (5.6). `offered` (`dialogs/tags.rs:113`) gives the color tags first: it gives **the tags used lately that are not a label's name** (`state::settings(cx).labels`, by name and `was`; if the function can't reach the settings from where it is called, take the labels as an argument and say who must pass them), `MOST_OFFERED` at most. `COLOR_TAGS` and `tag_color` stay as they are (other code reads them). Tests: with recent tags `work`, `Red`, `netplan` the dialog offers `work` and `netplan`; with none it offers none; the file's tests that expected the colors among the chips change for exactly that, and say which.

## Done when

The tests pass, clippy is clean for both crates, and only the marked places changed. Say in your report what you cut, the two times of part 1, and every test you changed.
