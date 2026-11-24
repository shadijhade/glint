# Card W3 — Far reads that an oplock can't stall

Wave 6, round 1 · crate `shadui-index` · the service's far reads

**You own:** `crates/shadui-index/src/far.rs` and its tests; in `docs/instant-search.md`, the sentence(s) on the far reads and oplocks (find them: search for "oplock" and "far"). Nothing else (not `engine.rs`: `FAR_READS` :338 and `far_round` :502 stay as they are).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), **§6.4**, §10, and under §9's table "Wave 3, as it landed", "Wave 4, as it landed" and "Wave 5, as it landed".
Then the code: `far.rs` whole (the module comment :1–:20; `root_handle` :40; `open_ref` :52, which uses `OpenFileById` with `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL` :64–:70; `listing` :79 with `FileStreamInfo`; `read_ref` :268; `drain` :283 and `drain_with` :288; the tests from about :500, among them `a_record_that_became_another_file_while_it_was_read_gets_nothing` :707). The crate's `windows` dependency and its `Wdk` features in `crates/shadui-index/Cargo.toml` (read it; don't change it: if a feature you need is missing, say so and stop there).
**Check with:** `cargo test -p shadui-index far::`, `cargo clippy -p shadui-index --all-targets -- -D warnings`.

## What changes

- **Today** another program that holds a marked file with an oplock keeps `OpenFileById` waiting until the oplock is broken, and the far reads' thread waits with it (until the service restarts, wave 3's record).
- **Open by file reference with `NtCreateFile`**: `FILE_OPEN_BY_FILE_ID` relative to the drive's root handle (the reference as the 8-byte name), with `FILE_COMPLETE_IF_OPLOCKED`, so the open comes back at once. When it answers `STATUS_OPLOCK_BREAK_IN_PROGRESS` the file is **skipped this round** (closed, left `far`) and read on the next.
- **Everything the far reads promise stays** (the service is LocalSystem): never by a path; a link never followed (`FILE_OPEN_REPARSE_POINT`); a cloud file never recalled (`FILE_OPEN_NO_RECALL`, or the `FILE_FLAG_OPEN_NO_RECALL` equivalent in `NtCreateFile`'s options); backup intent (`FILE_OPEN_FOR_BACKUP_INTENT`); attributes only (`FILE_READ_ATTRIBUTES` and `SYNCHRONIZE`) before a `glint.*` stream is known; the second open, for reading, only for an item that has one, and only `glint.*` streams read; marks given only while the record's number is still that file's. A reference whose sequence is 0 is never opened (wave 3).
- `FILE_SYNCHRONOUS_IO_NONALERT` so the handle works with `GetFileInformationByHandleEx` and `BackupRead` as now.
- `docs/instant-search.md`: the sentence that says such a program stalls the far reads becomes what is true now (skipped this round, read on the next).

## Tests

The tests that are there pass unchanged (say if one had to change, and why). Add:

1. A file held open by the test with an oplock (`FSCTL_REQUEST_OPLOCK` on an overlapped handle, as Windows' own samples do; if that can't be done without an administrator, say so and test the status path through a seam instead): `read_ref` comes back within a second and gives nothing; with the oplock let go, the next read gives its marks.
2. A link (a junction or a symbolic link the test can make without an administrator; if none can, say so) is not followed: its target's marks are not given.
3. An item whose number now names another file gets nothing (the existing test still passes).
4. A file with no `glint.*` stream is opened once, for attributes alone (through the seam that counts opens, if there is one; else say how you checked).

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

Tests pass, clippy is clean for shadui-index, the module comment says how the open is made now. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus NEW STRINGS (every `t!` string added or changed), MEASURED (if the card asks), TESTS CHANGED (every existing test you changed, and why). Say plainly that the installed service is 0.16.1's and was not replaced, so this has run only in tests.
