# Card W5 — The ledger, between processes and under a renamed folder

Wave 6, round 1 · crate `glint-core` · the ledger and carrying

**You own:** `crates/glint-core/src/marks/ledger.rs` and `crates/glint-core/src/marks/carry.rs`, and their tests. Not `marks/mod.rs` (W1's this round): if you need a line there, say so.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), §5.3, §5.4, §6.3, §10, and under §9's table "Wave 3, as it landed", "Wave 4, as it landed" and "Wave 5, as it landed".
Then the code: `ledger.rs` whole (`Ledger` :21, `open` :53, `waits_in` :88, `get` :116, `put` :121, `take` :141, `save` :162, `shared` :187, the empty flag `shared_empty`); `carry.rs` whole (`carry` :54, `carry_files` :59, `carry_one` :64, `carry_in` :85: what is done for a folder today); glint-model's `marks/lines.rs` (`Line` :24 with `rest`, `read_ledger`, `write_ledger`); `marks/mod.rs` `World` :53 and how `work` locks the ledger (:242–:258); `marks/wake.rs` (a listing asks `waits_in`, and must never wait on a lock or a read).
**Check with:** `cargo test -p glint-core marks::ledger`, `cargo test -p glint-core marks::carry`, `cargo clippy -p glint-core --all-targets -- -D warnings`.

## Between processes

The MCP server (`Glint.exe mcp`) is a process of its own with its own `shared()` ledger, so a line one process writes can be lost when the other saves (wave 5's record).

- `Ledger` remembers the file's modified time and size when it read it (or wrote it last).
- **A save** reads the file again when it changed since (another process wrote it), and puts **its own changes** over what is there: the lines this process put or took since its read are applied to what the file holds now, the rest is the other's. Keep what this process changed as a list of puts and takes (by key) since the last read, emptied on a save. Written as now (whole, to a temporary file that takes its place).
- **The app reads the file again** when it is newer than what it holds: checked at most once a second, by a cheap `metadata` call, and **never on the thread that lists a folder** (`waits_in` only tries the lock, as now; a reload happens in `get`/`put`/`take`/`save`, or on a timer of the ledger's own, your choice: say which). `shared_empty` stays true only while the file holds nothing.
- Never panics on a file another process cut short (a line cut off is left out, as `read_ledger` does).

## Under a renamed or moved folder

Ledger lines under a folder that Glint renames or moves keep the old path today (wave 3's record).

- `carry` (and `carry_in` for a folder pair): the ledger's lines **under** the source folder (any depth, by key prefix in lower case with `\\`) move to the same place under the destination when `moved`; are copied when not (a copy). One lock, one save.
- **A line that moves keeps what a later version wrote** (`Line::rest`), and its `why` and `at`.
- Its `waits_in` counts follow.

## Tests (plain `#[test]`; ledgers of the test's own in a temp folder: never `shared()`)

1. Two `Ledger`s open on one file (two processes): each puts a different line and saves; both lines are in the file, and each reads the other's after its next read.
2. One takes a line the other then saves without touching: the line stays taken.
3. Both change the same line: the last to save wins, and nothing else is lost.
4. A ledger whose file didn't change since it read it doesn't read it again (count reads through a seam).
5. A file cut short by another process reads without panicking; its whole lines are kept.
6. `carry` of a renamed folder moves the lines under it (two levels deep), keeps `rest`, `why` and `at`, and `waits_in` answers for the new folder and not the old; a copy keeps both.
7. A folder whose name is a prefix of another's (`C:\a\b` and `C:\a\bc`) moves only its own lines.

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

Tests pass, clippy is clean for glint-core, no signature outside these two files changes. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus NEW STRINGS (every `t!` string added or changed), MEASURED (if the card asks), TESTS CHANGED (every existing test you changed, and why).
