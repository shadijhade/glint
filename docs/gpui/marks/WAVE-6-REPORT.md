# Wave 6 of the Marks wave: report

*Written 2026-11-24 by the session that wrote and ran wave 6 ([what it was asked](RUN-WAVE-6.md)), against `c6a4c8b`. Eight cards, one Sonnet agent a card, in two rounds (W1, W3, W5, W7, W8; then W2, W4, W6). Nothing is committed.*

## The whole

- `cargo test --workspace --no-fail-fast`: **3,983 passed, 0 failed, 61 ignored** after the cards (wave 5's review left 3,947 / 0 / 60). The coordinator then added one ignored measurement (`marks::tests::measure_reads_for_rows_and_a_whole_folder`), so the tree now has **62 ignored**; it compiled and ran in release, and the whole wasn't run again for it.
- `cargo clippy --workspace --all-targets -- -D warnings`: **clean** (and `-p glint-core` again after the measurement was added).
- Six cards came back done, **two partial**: W1 (built and tested; the once-only move has never run against a real index, since this PC's service is 0.16.1's) and W6 (its fourth part, `open_at`, could not be reproduced: a test that passes was added, no code changed). Nothing was sent again.
- **Nobody has seen any of it on a screen**: not the tag menu's *Finding what has…*, not Undo of an import putting labels back, not a sort from the index.

## Landed before the cards, and why

Nothing: no two cards share code. W4 adds `UndoAction::Labels`; the only exhaustive matches on `UndoAction` and `UndoRun` are in its own two files (checked before writing it). PLAN §8.1 says so.

## The cards

### W1 — The color tags' move (5.6) — partial

- **Files**: `glint-core/src/marks/mod.rs`, `glint-core/src/marks/settle.rs` (new), `glint-ui/src/startup.rs`.
- **(a) the next write**: `work` writes `settled(labels, after)`. Core keeps the PC's labels (`set_labels`, `labels`, the seven when none were set). `startup.rs` hands them over at start and on each change of Settings' labels, only when they differ. `Change::Label` and `Change::Paste` start from the marks as *shown*, so taking the label off a file tagged *Red* doesn't bring *Red* back as its label. Every other change starts from the raw marks, so `take_label_off`'s `Tags { remove: ["Red"] }` still takes the color off.
- **(b) once on indexed drives**: `settle::start` waits 30 s, then runs on core's quiet lane. It lists `tag:` the seven, plus the PC's labels and their old names, on each drive, and writes `Put(settled)` in batches of 200 only where something changes. Its marker is `marks-settled.json` in Glint's data folder: the version, and per drive the last path done and whether it is done. `on_app_quit` stops it; it takes up where it stopped. It does nothing while the service isn't this version's, so **on this PC it has never run against an index**.
- **Tests**: `marks::` 110 passed, 3 ignored; the bulk move against a fake index.
- **Left**: the labels are not in `World`. Four files build `World` with a struct literal, so the agent added `work_with(items, w, labels)` and marked it `TODO(coordinator)`. It works as it is; folding it into `World` is a tidy-up for later. Old label names with a space, `;` or `"` are left out of the `tag:` query (the filter can't carry them whole), so their files move under (a) alone.
- **For the person**: `tags_set_and_tags_read_work_as_they_did_through_the_homes` changed. Tagging a file *Work* and *Red* now gives the label *Red* and the tag *Work*. That is 5.6's point, but PLAN §10 says `tags_set` answers as it did. Say which wins.

### W2 — A sort or a group by a mark from the index — done

- **Files**: `components/row_facts.rs`, `explorer/marked.rs`, and `dialogs/tags.rs` (`Backend::marks_in`, default `Ok(None)`, which **the card asked for though it gave the agent no claim on that file**: the card's mistake).
- `ask_all_with(folder, entries, comments, cx)`: one `marks_in` for the folder; comments read from the files only for the items the index says have one, and only when the sort is Comment or the filter holds `comment:`; else every entry from the files as before. Until a comment is read, an item the index says has one carries `Some("")`, so its note mark draws with an empty tooltip, and a row on screen reads the whole text in its batch.
- The folder asked is the entries' common parent; search results across folders read the files.
- **Tests**: `row_facts` 29 and 1 ignored; `explorer::marked` 5.
- **Measured** (debug, fakes): 50 ms either way for 10,000 rows. It shows only that the index's path reads no file; the real index's answer was not measured (below).

### W3 — Far reads an oplock can't stall — done

- **Files**: `shadui-index/src/far.rs`, the oplock sentence of `docs/instant-search.md`.
- `open_ref` is `NtCreateFile` with `FILE_OPEN_BY_FILE_ID` relative to the root, and the options `FILE_COMPLETE_IF_OPLOCKED | FILE_OPEN_REPARSE_POINT | FILE_OPEN_NO_RECALL | FILE_OPEN_FOR_BACKUP_INTENT | FILE_SYNCHRONOUS_IO_NONALERT`. `STATUS_OPLOCK_BREAK_IN_PROGRESS` makes a file busy: closed, left `far`, read next round, and not counted by `drain`.
- **Tests**: `far::` 23. Among them a real read-handle oplock taken without an administrator (the read comes back at once with nothing, then gives the marks once the oplock is let go), one open for attributes alone, and none for a reference whose sequence is 0. The link and renumbered-record tests pass unchanged.
- **Only in tests**: the installed service is 0.16.1's and was not replaced.

### W4 — The import's Undo and following, with their labels — done

- **Files**: `marks/apply.rs`, `dialogs/marks_import.rs`, `marks_follow.rs`, `explorer/ops_types.rs`, `explorer/undo.rs`, `settings/labels.rs`, `settings/tests_labels.rs`.
- **Undo**: `UndoAction::Labels { before, made }` and `UndoRun::Labels`. The card said `Labels(Vec<Label>)`; `made` is what lets Undo check the labels are still what the import left. The step is `Sequence([Labels, Marks])` when an import changed labels, and `Marks` as before when it didn't. Undo puts the labels back only while Settings still equal `made`.
- **Take back**: the journal's first line keeps `labels` (from `ImportOptions.have`) and `made`. `LastImport` carries both (`None` for an old journal); *Take back the last import* follows the same rule, after the marks.
- **Following**: `apply::follow_with(have)` plans with Settings' labels now. `Followed.labels` names the plan's labels; `marks_follow` adds the new ones with `apply_labels` (moved to core, `pub`). `follow()` stays.
- **Tests**: apply 13 and 1 ignored; marks_import 20; marks_follow 6; explorer::undo 16; settings::tests_labels 13.
- **Unsure**: a follow's journal can be picked by *Take back the last import* (the same rule holds). The Arabic case runs through the helpers, not a drawn window.

### W5 — The ledger between processes and under a renamed folder — done

- **Files**: `marks/ledger.rs`, `marks/carry.rs`.
- **Between processes**: a ledger remembers the file's time and size and what it put and took since it read. `save` reads again first if another process wrote, lays its own changes over, and writes whole.
  - **The app reads again from a thread of the ledger's own** (`marks-ledger`): once a second, a `try_lock` and one `metadata`, for as long as the process runs, in the MCP process too. Listings never wait.
  - **Left**: a write by the other process between this one's read and its write is still lost (a `ponytail:` comment says a lock file would close it).
- **Under a folder**: `Ledger::carry_under` moves or copies every line under a renamed, moved or copied folder (a key prefix with `\\`, so `C:\a\b` never takes `C:\a\bc`'s), keeping `rest`, `why` and `at`. `carry_in` does a call's folders under one lock and one save. A rename that changes only case is skipped (the keys are equal).
- **Tests**: ledger 7, carry 12.

### W6 — The small things left — partial

- **Files**: `sidebar/mod.rs`, `sidebar/tests.rs`, `dialogs/mod.rs`, `dialogs/tags.rs`, `persistence.rs`, `settings/tests.rs`.
- **1. Done**: *Rename everywhere…* and *Take off every file…* open at once and say *Finding what has “{tag}”…* (role Status). Their button and Enter wait for the files; with none, the old notice. Take off is now a view of its own (`TakeOffTag`), with the same strings.
- **2. Done**: `enter_runs_main` is gone; nothing called it.
- **3. Done**: one cap, `persistence::KEPT_RECENT_TAGS`, `pub(crate)`; `dialogs/tags.rs` uses it.
- **4. Not reproduced**: `open_at` to the Files page's last section already scrolled there in the test harness. Moving the scroll to the next frame made the test fail, so it was put back. `open_at_a_section_after_a_pages_first_scrolls_to_it` passes without a change: it doesn't prove the real app's case (wave 4's record), which still needs a look in the real app.
- **Tests**: sidebar 68, dialogs:: 478, persistence 28, settings 73.

### W7 — The docs — done

- **Files**: `docs/marks.md` (new), `docs/features.md`, `docs/instant-search.md` (a marks bullet under How it works; the security bullets were already there from an earlier wave), `docs/gpui/STANDARDS.md` (§5.2 Marks: M1 to M11), `NEXT_RELEASE.md` (*In 0.17.0*).
- It wrote what the code does where the plan differs:
  - *In comments* is the index's alone.
  - Typo forgiveness is the index's.
  - A failed stream write goes beside the file before the ledger.
  - Glint carries a folder's own marks itself.
  - An export's paths start from the file's own folder.
  - The suggestion for a `tag.dat` comes through `suggestions.rs`.
- *Not yet tried against a real XYplorer / on an exFAT drive / on a NAS* are said in those words.
- **The coordinator finished three things**:
  - `marks.md`'s sentence on the background move, now W1's code: half a minute after start, background priority, taken up where it stopped, waits for this version's service.
  - `NEXT_RELEASE.md`'s Checks: this wave's totals and speed, and the e2e check written but not run.
  - The ledger's line: one W7 left plain while W5 was mid-edit; no change needed.
- **To check by eye**: the ledger's path. W7 wrote "Glint's data folder"; the plan says `%APPDATA%\com.shado.shadui-ripgrep`.

### W8 — The end-to-end check `marks` — done, written, not run

- **Files**: `tests/e2e/checks/Checks.ps1` (`marks` and a helper `Get-RowSaid`, between `search-history` and `settings`), `tests/e2e/README.md` (its row).
- What it does: `Alt+1` on `marks-e2e.txt`; waits for the row to say *label Red*; `label:red` finds it; `cmd /c ren`; the renamed row still says *label Red*; cleans up under `Assert-InScratch`. It parses with no errors.
- **Not run**, nor `run.ps1 -List`: the permission check refused that call, though reading `run.ps1` shows `-List` starts nothing.
- **Not known until it runs**: where UI Automation exposes *label Red* (`Get-RowSaid` joins the row's Name, HelpText and children's; `dump-ui.ps1 -Steps 'alt+1'` narrows it), whether `Alt+1` reaches the list after `Select-Row`, and whether the watcher answers within the 20 s it waits.

## Tests that were there and changed

- `marks::tests::tags_set_and_tags_read_work_as_they_did_through_the_homes` (W1): a *Red* tag now becomes the label (see W1, for the person).
- `sidebar/tests.rs`' rename and take-off tests (W6): they wait for the dialog *and* for *Finding…* to go, since the dialog now opens first.
- `settings/tests_labels.rs`' `an_import()` (W4): `..LastImport::default()`.
- `marks_follow.rs`' fake source and `Followed` literal (W4).

## Every new string

Through `t!`:

- `Finding what has “{tag}”…` (W6)
- `import of labels` (W4: an Undo step's words when an import changed labels and no marks)

Not `t!`: W1, W3 and W5 add no words. The docs are W7's.

## Speed (6.6, release builds; written into PLAN §6.6 with how)

| | Wanted | Wave 6 |
| :--- | :--- | :--- |
| Listing 100,000 | 24 ms | **26.8 ms** (enumeration 22.9, sort 6.6) |
| Marks of 200 rows, none marked | within 1 ms of 2.1 ms | **11.6 ms: not met** |
| Every mark of 10,000, from the files | — | 205 ms none marked, 984 ms all three each |
| The same from the index (W2) | under 60 ms | **not measured** |
| `tag:work` in the service | 8 ms | **not measured** |
| The right-click menu | under 1 ms | **not measured** |
| A `tag.dat` of 10,000 lines | read under 50 ms | read 21 ms; the whole plan 274 ms; applied 37.6 s with streams, 1.1 s beside |
| `set_marks` over 1,000 | — | 1.68 s (`get_marks` 88 ms) |
| A zip of 10,000 unmarked | the same | 1,327 ms without the marks' step, 1,379 ms with it |
| A label rule on 1,000 new files | — | 375 ms |

How: 50 writes of 4 KB took 0.8 ms each first, so the PC was not in its locked-maintenance state. `Glint.exe bench <folder> --runs 9`; the cards' ignored tests run from the release test binary.

- **Not measured**:
  - The index rows need the new service. This PC's is 0.16.1's, which was not replaced, and `shadui-index bench` needs an administrator.
  - The menu needs the real app.
- **The 200 rows**: four times S0's harness. The read is waves 2 to 5's, unchanged by wave 6 (`git diff c6a4c8b` doesn't touch `find` or `streams.rs`). `find` does more than S0's harness: it looks once for the folder's `.glint-marks`, and it tidies the marks. The files were new in %TEMP%, where Defender may scan on open. Which of these costs the time is not found.
- **Applying an import with streams**: 3.8 ms an item, about the same as wave 4's guess.

## Left between the cards, and what to wire

- W1's `work_with` → labels into `World` (a tidy-up).
- W5's timer thread runs in every process for good. The other way, a `refresh` where `work` locks the ledger, is in `marks/mod.rs`.
- `marks/mod.rs` could re-export `apply::follow_with` and `apply::apply_labels` (W4); nothing needs it.
- W2's `Some("")` for a comment the index knows of: a hover before the row's batch lands shows an empty tooltip.

## What a card's wording got wrong

- **W2** told the agent to add `marks_in` to the `Backend`, which lives in `dialogs/tags.rs`, a file the card didn't give it. (W6 owned that file's cap; the two edits don't meet.)
- **W4**: `UndoAction::Labels(Vec<Label>)` can't tell "still what the import made" without `made`.
- **W6**: part 4's bug doesn't show in the test harness.
- **W1**: the labels in `World` would have broken four files' struct literals.

## Still the person's

- A real XYplorer (11.6): mark files there in each storage mode, bring them in, follow, export back and `tagload` it.
- The real drives of 11.3: an exFAT stick, a NAS, a ReFS Dev Drive, OneDrive, two hard links, a file whose record fills.
- Running the end-to-end checks (`tests\e2e\run.ps1`), `marks` the first time; updating the instant search service, after which W1's background move and W2's sort from the index can be seen and timed.
- The reply on issue 8, and the README's What's new.
- **The two choices W6 left**: where the head's label chips go (today only in the room the kinds leave), and how the export's path is cut.
- **Two more choices**: whether tagging a file *Red* now gives it the label (W1, §10), and whether W5's once-a-second thread is acceptable in every process.

## Every new file

- `crates/glint-core/src/marks/settle.rs` (W1)
- `docs/marks.md` (W7)
- `docs/gpui/marks/cards/W1-color-tags-move.md`, `W2-sort-from-index.md`, `W3-far-oplocks.md`, `W4-import-undo-labels.md`, `W5-ledger.md`, `W6-small-things.md`, `W7-docs.md`, `W8-e2e-marks.md`
- `docs/gpui/marks/WAVE-6-REPORT.md` (this file)
- `docs/gpui/marks/RUN-WAVE-6.md` was there, untracked, before the wave.

Every other change is in `git diff c6a4c8b`: 27 files. Among them are `PLAN.md` (§6.6's measured column, §8.1, §9's rows) and the coordinator's measurement in `marks/mod.rs`'s tests.
