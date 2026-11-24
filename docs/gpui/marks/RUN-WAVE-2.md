# Writing and running wave 2 of the Marks wave

*For the session that writes wave 2's four cards and then runs them. Wave 1 was run the same way ([what was asked](RUN-WAVE-1.md), [its report](WAVE-1-REPORT.md)); the difference is that this time the cards are yours to write too. The session that wrote the plan reviews what comes back, does what is left between the cards, and commits.*

## Where things stand

- Repository `C:\Repo\VisualStudioRepos\glint`, branch `marks-wave`. The last commit is `13660c7` (the plan, the contracts, wave 1's cards). **Wave 1 and its integration are in the working tree, not committed**: 13 source files and `PLAN.md` are modified, and that is as it should be. Leave it so. `dist/` and `src-tauri/` are untracked and not ours.
- The tree as it is now is kept as `4dd21620c4ed2173c8a519aa35e7e1d542f2cda7` (made with `git stash create`; it is no branch and no stash entry). `git diff 4dd2162` shows what wave 2 changed, and nothing of wave 1.
- `cargo test --workspace --no-fail-fast`: 3,562 pass, 1 fails, and that one has nothing to do with this work (`thumbs::tests::draws_a_pdfs_first_page`: Windows' PDF renderer answers 134 by 67 where 96 by 48 is asked). Clippy is clean.
- Read [PLAN.md](PLAN.md) whole before anything else. Section 9 has the cards' table and, under it, **"Wave 1, as it landed"**: what exists now, what is made and not called yet, and what the wave's agents decided.

## Part 1: write the cards

The four cards of wave 2, from PLAN §9's table: **A5** (the filter in the open folder; sort and group by a mark; plain words), **B2** (the three homes; `marks_read`, `marks_set`, `marks_put`, `read`, `matches`), **C2** (streams that left the record; `Marks`, `MarksIn`; a search that looks in marks), **D1** (the hub takes marks; `RowFacts` keeps them; Undo). Put them in [cards/](cards/), named as wave 1's are.

**How a card is written.** As wave 1's six are: read [A4-filter.md](cards/A4-filter.md) and [B1-streams.md](cards/B1-streams.md) and keep their shape exactly (what it owns, what to read first with lines, the command that checks it, what to build with its rules, exact strings and tables, numbered tests named as sentences, when it is done). Write each from the code as it is today: open every file the card will own, and say where things are by `file:line`. A card is one agent's one run: when one is too big for that (C2 may be), make it two cards that own different files.

**No card may need another wave-2 card's code to compile or to pass its own tests.** They run at the same time in one tree. Where two cards meet (one calls what another fills, a type crosses a crate), land that signature yourself first, as code that compiles and does nothing (never `todo!()`), as S0 did (PLAN §8 and §8.1), and say in both cards that it is there. Never change a signature that code outside the card's files already calls: add beside it.

**What each card must hold to** (the plan's sections say why):

- **A5.** `glint_model::marks::Wanted` is the second copy of the filter (§6.5): it must answer every case of `tests/fixtures/marks-filter.txt` as `shadui_index::query::MarksFilter` does, so read card A4 and its code and mirror the rules; where the two would differ, that is a bug in the card. The open folder's filter learns the marks keys (today `tag:` there shows nothing: G2), taking the rows' marks from a reader passed in, as Git's marks and the media facts are. Sorting and grouping by a mark read the same way (`Reads`, `group_in`); a row whose marks aren't known yet sorts as one with none. Plain words (§4, F16) need the labels' names, so they are passed in. The four key lists glint-ui keeps by hand are not this card's.
- **B2.** The three homes of §5.2 to §5.4, in that order: reading takes the first home that has anything for the item; writing to a home takes the item out of the homes after it. A batch of `marks_read` costs one stream listing an item (B1's `streams::read`) and one look for `.glint-marks` a **folder**, never one an item. What is kept is always `Change::apply`'s or `tidy`'s (A1); the text of `.glint-marks` and of the ledger is `lines`' (A2), written with `crate::replace_contents`, the sidecar hidden. The ledger is one file in Glint's data folder, held in memory behind one lock. Every path gets its `Outcome` (before, after, which home), and a failure is words a person can read. `tags_read` and `tags_set` answer exactly as they do today, whatever they go through. Core has no labels: `settled` is the interface's to call.
- **C2.** A far record (§6.4, and §14 item 6 for why) is read **through the file system**, by its path with backup semantics, never from the volume's clusters: the three streams, a comment's first `record::COMMENT_KEPT` bytes, then `Tree::set_marks`. After a drive's first read and as the journal brings records, on a thread of its own: a search never waits for it. `Engine::marks` and `Engine::marks_in` (the pipe sends what they answer already); `MarksIn` never carries a comment's text. A search with `marks: true` lets plain words match tags and comments, such hits flagged `HIT_BY_COMMENT` and `HIT_BY_TAG`; with `fuzzy`, a tag or label asked for that nothing has is replaced through `MarksFilter::nearest`. `client.rs` gets the calls the app needs. The service must never panic on what a stream holds.
- **D1.** The hub stays in `dialogs/tags.rs`, with its one `Backend` and its one event: `set_marks` calls core, remembers one Undo step (`UndoAction::Marks`, run by `marks_put`), says `TagsChanged`, and words a failure; `set_tags` keeps its signature and goes through it, so tags get Undo (G5). `RowFacts` keeps the marks of the rows it reads (`marks_of`, `ask_all`) and `tags_of` goes on answering. The tests the files have pass unchanged and their fakes still compile. D1 is tested against the fake backend: core's three calls answer nothing until B2 is in.

Then link the cards from PLAN §9's table, as wave 1's are, and add to §8.1 whatever signatures you landed for them.

## Part 2: run them

As wave 1 was run:

1. **One agent for each card, all at the same time**, on a cheaper model than your own (Sonnet), in this tree and not in worktrees.
2. **Each agent is given these and nothing else**: `docs/gpui/STANDARDS.md`; section 6 of `docs/gpui/aware/PLAN.md` (the rules for every agent, with the card's own commands in place of its rule 4's); `docs/gpui/marks/PLAN.md`, the sections its card lists; and its card. It is told that other agents are editing other files in the same tree at the same moment, and that its last message is the report of that section 6, item 9.
3. **While they work, change nothing yourself.** Their builds wait for one another on Cargo's lock; nobody sets `CARGO_TARGET_DIR`.
4. **When all have reported**, read each report against its card. A card that came back `partial`, or whose tests don't cover what it lists, is finished by you or sent again with its report attached.
5. **Then check the whole**, once: `cargo test --workspace --no-fail-fast` and `cargo clippy --workspace --all-targets -- -D warnings`. Fix what the cards broke between them; a failure in a file no card owned is said in the report, not patched around.

## Part 3: the report

Write `docs/gpui/marks/WAVE-2-REPORT.md` in the shape of wave 1's: what you landed before the cards and why; for each card its status, files, tests (the command and the counts), what it didn't do, what it needs, what it was unsure of; the two commands of step 5 with their totals; what is left between the cards for afterwards (calls nobody makes yet, lists kept by hand, anything a card had to leave); and what a card's wording got wrong. Say plainly what is partial.

## What not to do

- **No commit, no push, no branch change**, and none of `git stash`, `checkout`, `restore` or `add` (reading with `git diff`, `git status` and `git log` is fine). The agents run no `git` command that changes anything, and never `cargo fmt` on a file that isn't theirs.
- **Nothing of waves 3 to 5**: no drawing in the list, no menu, no dialog, no import.
- **Not the end-to-end checks** (`tests\e2e\run.ps1`): they take the real mouse and keyboard.
- **No new crates.** And nothing outside the files the cards name, apart from the signatures of Part 1 and what step 5 needs.
- **`PLAN.md` is changed only** where Part 1 says (the table's links, §8.1).

When the report is written, say so, with the two totals.
