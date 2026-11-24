# Card W2 — A sort or a group by a mark, from the index (6.6's third row)

Wave 6, round 2 · crate `glint-ui` · `RowFacts` and the rows

**You own:** `crates/glint-ui/src/components/row_facts.rs` and its tests; the marked places in `crates/glint-ui/src/explorer/rows.rs` and `crates/glint-ui/src/explorer/marked.rs` that ask `ask_all`/`has_all` (find them: `rg -n "ask_all|has_all" crates/glint-ui/src/explorer`). Not `pane_head` (its `marks_in` is the model of the call, :261, :277, :513).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), **§6.1**, **§6.6** (the third row, and "The third row can't come from the files"), §10, and under §9's table "Wave 3, as it landed", "Wave 4, as it landed" and "Wave 5, as it landed".
Then the code: `row_facts.rs` whole (`MOST_PER_FOLDER` :60, `marks_of` :578, `has_all` :584, `ask_all` :593, the fakes :923); glint-core's `marks::marks_in(folder) -> Result<Option<Vec<MarkedIn>>, String>` (`marks/mod.rs` :337–:364: `None` where the index can't say; `MarkedIn { name, label, tags, comment: bool }`, never a comment's text) and `marks::comment_brief`; `explorer/rows.rs` and `explorer/marked.rs` where `ask_all` is asked and the sort keys are made.
**Check with:** `cargo test -p glint-ui components::row_facts`, `cargo test -p glint-ui explorer::` (by its filter; the module is big: run the marked tests by name if you can), `cargo clippy -p glint-ui --all-targets -- -D warnings`. glint-ui builds slowly and two other agents build it now: build by your filter.

## What changes

- `ask_all` on a folder **instant search covers**: one `marks_in(folder)` call (through the `Backend`: add `marks_in` with a default body answering `Ok(None)`, so every fake there is compiles unchanged, and `CoreBackend` calling core) fills the folder's marks from memory: label and tags as the index has them; an item absent from the answer has no marks.
- **A comment's text**: only when the sort or group is by **Comment** (the rows tell `ask_all` what the sort is: add what you need to its arguments only if no caller outside your files calls it; else a second method), the items whose `comment` is true are read from the files with `read_marks` (one call, for those alone), for the first line a sort needs. Otherwise a comment is known as "has one" (`Marks.comment` stays `None` until read? decide, and make sure the Comment column and the note mark still draw what they drew: a row on screen is still read by the batches of 200, which give the whole marks).
- Where `marks_in` answers `None` (not covered, no service, a service older than marks, a remote, an archive), it reads the files as it does today.
- `has_all` holds once the index's answer (and any comment reads) are in.
- **What the index says can be older than a change Glint just made**: a `TagsChanged` path is read from the file afterwards as today and wins over the index's.

## Tests (STANDARDS §5.1 "Tests", fakes, never a real disk or index)

1. With a backend whose `marks_in` answers for the folder, `ask_all` on 10,000 entries asks `marks_in` once and `read_marks` never (sort by label); `has_all` holds after; each row's label and tags are the index's.
2. Sorted by comment: `read_marks` is asked once, for the items the index says have a comment, and no others.
3. `marks_in` answering `None`: `read_marks` once with every entry, as before.
4. A path named by `TagsChanged` after the index answered is read from the file and its marks win.
5. The tests that are there pass unchanged (the default body).
6. **Measured** (`#[ignore]`, prints): a folder of 10,000 marked entries sorted by label, the fake's answers in hand: from `ask_all` to `has_all` and the rows built, before (the files' path: `marks_in` answering `None`, the fake's `read_marks` answering at once) and after. Say what it measures and what it can't (the real index's answer time is measured by the coordinator in release).

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

Tests pass, clippy is clean for glint-ui, `ask_all`'s and `has_all`'s callers outside your files compile unchanged. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus NEW STRINGS (every `t!` string added or changed), MEASURED (if the card asks), TESTS CHANGED (every existing test you changed, and why).
