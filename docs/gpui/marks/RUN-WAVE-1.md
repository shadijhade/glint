# Running wave 1 of the Marks wave

*For the session that runs the six cards. The session that wrote the plan does what comes after (the wiring, the review, the commit), so this one stops when the cards are in and the workspace builds.*

## Where things stand

- Repository `C:\Repo\VisualStudioRepos\glint`, branch `marks-wave`, at `13660c7` (the plan, the contracts as code that does nothing yet, wave 1's cards). `git status` shows only `dist/` and `src-tauri/` as untracked: leave both alone.
- The plan is [PLAN.md](PLAN.md); the cards are in [cards/](cards/): A1, A2, A3 (glint-model), A4 and C1 (shadui-index), B1 (glint-core).
- `cargo test --workspace` has one test that fails on this PC and has nothing to do with this work: `thumbs::tests::draws_a_pdfs_first_page` (Windows' PDF renderer answers 134 by 67 where 96 by 48 is asked). Don't chase it.

## What to do

1. **Start six agents at the same time, one for each card**, on a cheaper model than your own (Sonnet): the plan has lower-tier agents do the cards and the coordinator check them. They work in this tree, not in worktrees: each card names the files it owns, and no two cards own the same file.
2. **Give each agent these and nothing else**: `docs/gpui/STANDARDS.md`; section 6 of `docs/gpui/aware/PLAN.md` (the rules for every agent, with the card's own commands in place of its rule 4's); `docs/gpui/marks/PLAN.md`, the sections its card lists; and its card. Tell it that other agents are editing other files in the same tree at the same moment, and that its last message is the report of that section 6, item 9.
3. **While they work, change nothing yourself.** The build folder is shared and Cargo locks it, so their builds wait for one another; nobody sets `CARGO_TARGET_DIR`.
4. **When all six have reported**, read each report against its card. A card that came back `partial`, or whose tests don't cover what the card lists, is finished by you or sent again with its report attached.
5. **Then check the whole**, once: `cargo test --workspace --no-fail-fast` and `cargo clippy --workspace --all-targets -- -D warnings`. Fix what the cards broke between them; a failure in a file no card owned is said in the report, not patched around.
6. **Write `docs/gpui/marks/WAVE-1-REPORT.md`**: for each card its status, the files it changed, its tests (the command and the counts), what it didn't do, what it needs from the coordinator, and what it was unsure of; then the two commands of step 5 with their totals; then anything a card's wording got wrong. Say plainly what is partial.

## What not to do

- **No commit, no push, no branch change**, and none of `git stash`, `checkout`, `restore` or `add`. The agents run no `git` command that changes anything, and never `cargo fmt` on a file that isn't theirs.
- **Not the list under "What wave 1's cards leave for the coordinator"** in PLAN.md §9 (the search asking the new filter, `keep_times` made one function): that is done after this, by the session that wrote the plan.
- **Not the end-to-end checks** (`tests\e2e\run.ps1`): they take the real mouse and keyboard.
- **No new crates**, and nothing outside the files the cards name, apart from what step 5 needs.

When the report is written, say so, with the two totals.
