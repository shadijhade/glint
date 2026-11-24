# Card W8 — An end-to-end check `marks` (11.7), written and not run

Wave 6, round 1 · PowerShell · no Rust

**You own:** `tests/e2e/checks/Checks.ps1` (one new check, written as the others there are) and, if the README lists the checks, its line in `tests/e2e/README.md`.
**Read first:** `tests/e2e/README.md` whole; `tests/e2e/checks/Checks.ps1` whole; `tests/e2e/lib/` (what a check may call: the arena, keys, typing, reading the UI tree); `tests/e2e/run.ps1` (**read it; do not run it**); PLAN.md F6, F12, F18, §11 item 7; glint-ui's `actions.rs` for `Alt+1`.
**Check with:** PowerShell's parser only: `powershell -NoProfile -Command "[System.Management.Automation.Language.Parser]::ParseFile('tests/e2e/checks/Checks.ps1', [ref]$null, [ref]$errors); $errors"` (an empty answer is good). And `run.ps1 -List` **only if** reading `run.ps1` shows that `-List` starts nothing (no Glint, no mouse or keyboard, no profile): say what you read that told you.

## The check `marks`

1. In the arena (as the other checks make their files), a file `marks-e2e.txt`.
2. Select it, press `Alt+1` (the first label: Red by default; a fresh profile has the defaults).
3. The row shows its label (read from the UI tree as the other checks read a row: its accessible name or its Label cell; find what the row exposes, e.g. the Label column's cell or the wash's description, and say which).
4. Type `label:red` in the search box: the file is found.
5. Rename it with `ren` in a terminal (`cmd /c ren …`, as a program outside Glint): after the folder watcher has read the folder again, the row with the new name still has its label.
6. Clean up what it made, as the other checks do.

Name it in the list the runner reads, with words like the others'. Every wait has a timeout and says what it waited for when it fails.

## Not run

**Never run `run.ps1` itself** (it takes the real mouse, keyboard and profile). Say in the report what you couldn't try: whether the row's label is in the UI tree where you read it, how long the watcher takes, whether `Alt+1` reaches the list with the selection.

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

The check parses and reads like its neighbours. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus NOT TRIED (each step you could not check without running it).
