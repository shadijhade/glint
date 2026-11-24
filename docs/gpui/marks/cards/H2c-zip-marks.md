# Card H2c — The zips Glint makes keep the marks

Wave 5 · crate `glint-core` · the archive writer

**You own:** `crates/glint-core/src/archive/compress.rs` and its tests. Nothing else. Not extracting (a zip's `.glint-marks` comes out as a file, which is enough).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), §5.1 to §5.5, §8 and §8.1 (the last item, "Landed before wave 5's cards"). F19 and F23 in §4. Then the code: `compress.rs` whole (`Item` :25, `gather` :34, `compress_as` :93, the zip loop :108–:155, `seven_z` :192, `tar_zst` :260, the tests); glint-model's `marks/lines.rs` (`Line` :24, `Beside` :47, `read_beside` :173, `write_beside` :222, `BESIDE_NAME` in `marks/mod.rs`); glint-core's `marks::read_many`.
**Check with:** `cargo test -p glint-core archive::compress` and `cargo clippy -p glint-core --all-targets -- -D warnings`.

**What is there for you:** `glint_core::marks::read_many(&[String]) -> Vec<Marks>` (`crate::marks::read_many` from here), synchronous, whole: an item's streams, else its line beside it, else the ledger. One call reads each folder's `.glint-marks` once.

## What a zip holds

- An archive of marked items holds a `.glint-marks` **in each of its folders that has a marked item**, in 5.2's lines (`write_beside`, `{"glint-marks":1}` first, sorted by name), names relative to that folder. **A folder's own marks go in its parent's `.glint-marks` under its name**; an item at the archive's top (one of `paths`, or a folder's name there) goes in a `.glint-marks` at the archive's top.
- **Hidden** where the format can say so: for the zip, the MS-DOS hidden attribute if the `zip` crate (9.0.4: read its source through `cargo metadata`, never download) lets an entry carry it; else say so in the report. Its date: now.
- A `.glint-marks` already on disk in a folder being zipped is **not** copied as a file (the marks it holds come through `read_many` for the items that are there; a line for an item no longer there is left out). Compare its name in any case.
- **Reading the marks must not slow a zip of unmarked files**: one `read_many` a folder's items (gather the paths by folder as `gather` walks), nothing written for a folder with no marked item, nothing at all for an archive with none.
- A `.glint-marks` counts nothing in the progress's bytes (`total` stays the files' sizes, so the tests' totals hold).
- **7z and tar.zst**: the same few lines if they are (an entry from bytes: `sevenz_rust2`'s `push_archive_entry` with a reader, `tar`'s `append_data`). If either is more than that, leave it and say why.
- With a password, the `.glint-marks` is encrypted like the other entries.

## Tests (plain `#[test]`, a temp folder of the test's own, removed at the end)

The marks are written with `crate::marks::set_many` on the test's files (they go to the streams on NTFS; that is fine).

1. A zip of a folder with a labeled file, a tagged file and a bare one holds `folder/.glint-marks` whose lines (`read_beside`) are the two marked files' marks and nothing for the bare one.
2. A folder's own marks are in its parent's `.glint-marks` (at the zip's top for a folder zipped by name), under its name.
3. Two levels: a marked file in `photos/2026/` gives `photos/2026/.glint-marks` and no `photos/.glint-marks` when nothing in `photos/` itself is marked.
4. A zip of unmarked files has no `.glint-marks` anywhere; its entries are exactly as before (the existing test's names hold).
5. A `.glint-marks` on disk in the folder is not copied as a file; the zip's own is made from the items there.
6. A comment with a line break and an Arabic tag come out of the zip's `.glint-marks` as they went in.
7. 7z and tar.zst do the same (or the report says why not).
8. **Measured** (`#[ignore]`, prints): a zip of 10,000 unmarked files of 64 bytes in one folder, **before and after**: before by a run with the marks' step skipped (a flag only the test sets, or the time `read_many` takes over those paths alone, subtracted: say which), five runs each, the medians, debug build. Give them in the report.

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no `dir /s` from a root, no `Get-ChildItem -Recurse` outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. If a build has waited ten minutes on Cargo's lock, say so in your report rather than waiting on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug, and don't trust a timing from such a window.
- **Backslashes**: edit Rust with the editing tools, never a script typed into a shell.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no end-to-end checks, no real AI agent, no installed Glint.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. If you meet it, say so and go on.

## Done when

The tests pass, clippy is clean for glint-core, `compress_as` and `compress` keep their signatures. In your report: the measurement, whether the zip's `.glint-marks` is hidden, and what 7z and tar.zst do.
