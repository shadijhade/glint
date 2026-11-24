# Writing and running wave 4 of the Marks wave

*For the session that writes wave 4's cards and then runs them, as another session did waves 2 and 3 ([what wave 3 was asked](RUN-WAVE-3.md), [its report](WAVE-3-REPORT.md)). You have none of that session's memory: everything you need is in this file and in [PLAN.md](PLAN.md). Wave 4 is XYplorer's marks brought in, marks out and in as files, and the places that list labels: the sidebar, Settings, the pane's head. The session that wrote the plan reviews what comes back, looks at it in the real app, and commits.*

## Where things stand

- Repository `C:\Repo\VisualStudioRepos\glint`, branch `marks-wave`. **Waves 1 to 3 are committed: `00cf486`.** The working tree is clean but for `dist/` and `src-tauri/`, which are untracked and not ours. So `git diff 00cf486` shows wave 4 alone; a new file is untracked and not in that diff: the report lists every new file.
- `cargo test --workspace --no-fail-fast`: **3,756 pass, 1 fails**, the one that has nothing to do with this work (`thumbs::tests::draws_a_pdfs_first_page`). `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation whenever they run *alone* (0.16.1 does the same); inside the whole run they pass. An agent that meets it says so and goes on.
- Read [PLAN.md](PLAN.md) whole first. Section 4 is what the person sees (F1 to F35), section 7 is XYplorer, section 8 the contracts (8.1: what is in the tree), and under section 9's table **"Wave 3, as it landed"** says what the review changed, what was seen in the real app, and what was left for this wave.

## Part 1: write the cards

Wave 4 in section 9's table: **F1**, **F2**, **F3**, **G1**, **G2**. Write them as the earlier ones were (their shape is [A4-filter.md](cards/A4-filter.md)'s, [D1-hub.md](cards/D1-hub.md)'s and [E4-preview.md](cards/E4-preview.md)'s), from the code as it is today, with `file:line`. One card is one agent's one run: cut a card in two that own different files when it is too big (F1 and G1 are the likely ones).

**The table leaves these to nobody. Each gets a card, or a place in one, and the report says where each went:**

- **F23**: a folder on a drive with streams that holds a `.glint-marks` says *12 marks here are beside the files* in its head, with **Move them into the files** (core: read the folder's lines, write the streams through `marks_put`, which takes each line out as its stream is written; one Undo step).
- **F31**: marks that waited for an item (the ledger's lines with `why: away`) move into the file the first time Glint lists it.
- **F9's last two commands**, *Import marks…* and *Export marks…*, in `Ctrl+K` (`palette/commands.rs`), and **F25's**: a `tag.dat`, an `XYplorerTag.dat` or a Glint export offers *Import marks…* in its own menu under Suggested (`menus/smart.rs`).
- **From wave 3's review** (section 9): `sidebar/rows.rs`'s `reads_files` still says `tag:` reads the files; no request Glint builds sets `SearchRequest.marks` (G2's *In comments* needs it, with the two bits of a hit's flags); nothing calls `marks_all` for the sidebar or `client::marks_in`.
- **B4, if it fits one run** (say in the report if it was cut): the folders *inside* a folder that Windows' engine copies or moves to another drive lose their own marks, since Windows copies a file's streams and never a folder's. The queue carries them already (`queue.rs`: `plan` gives each folder with the one it is a copy of, and `marks::carry` takes the pairs). In `shell::imp::file_op` a copy can walk the source's folders afterwards; a move must read them first, and only when the two ends are on different volumes (on one volume a move is a rename and nothing is lost).
- **E6, small things the look found** (section 9, "Looked at in the real app"), each cut freely: the grid's sort bar offers no mark; Edit tags… still offers the seven color names as chips, which are labels now; a sort by label works out what each row is shown as at every comparison (318 ms for 10,000 marked rows in a test build: a key a row, made once, as names have in `explorer/rows.rs`'s `sort_order`).

**No card may need another wave-4 card's code to compile or to pass its tests.** Land what two cards share first, as code that compiles and does nothing (never `todo!()`), and say so in both. The contracts of import and export are in the tree already (`glint-core/src/marks/import.rs`, `export.rs`, `xy_find.rs`: types and signatures with empty bodies). Never change a signature that code outside a card's files already calls.

**What each card must hold to:**

- **F1, the import** (PLAN 7.3; F27 to F31). One pipeline for a `tag.dat`, an `XYplorerTag.dat` and a Glint export: read (`glint_model::marks::xyplorer::read`, as it is: see "The real `tag.dat`" below) → find each item by the storage line (7.3: all five kinds; a server that does not answer is given two seconds once, not once an item) → a plan the preview shows → apply. **XYplorer's files are only read** (M7). Applying goes through the homes in batches of 200 (`marks_put`: one write a folder for marks kept beside the files, never one a file), at background priority, the befores into `imports\<time>.jsonl` in Glint's data folder and into one Undo step; running the same import twice changes nothing. *Fill what is empty* is the default (`marks::meet`). Moved since: `client::history` (30 days) where instant search covers the drive. Following (F30) is off until chosen: one `stat` of the source when Glint starts or comes to the front, and what changed in it since. **Everything from a file someone else wrote is untrusted** (M10): sizes capped, a path that leaves its folder refused, nothing panics on a damaged file (the fixtures in `tests/fixtures/marks/` have each kind).
- **F2, the import's dialog** (F27, F28, F29). The preview before anything is written, as 7.2 words it: the source's line, the labels each with what it becomes (and a way to point one at another), the extra columns (7.4) with their switch, and where the items are, by outcome. Then how far, in the dock, as other long runs show it; then the notice with **Undo** and **What happened** (every item with its outcome, to copy). The suggestion in Settings' Overview for a `tag.dat` never brought in. It opens out of nothing in particular (a plain dialog), reads nothing on the UI thread, and its numbers come from the plan core answers.
- **F3, export** (F24, F33). A folder's marks below it into one `.jsonl` (A2's lines, paths relative to the folder, the labels in the first line) or XYplorer's format (`xyplorer::write`: version 5, Storage 3, which `tagload` merges). It reads the three homes as `marks::read` does, on core's threads, and can be stopped. The dialog asks where and which format, and says how many items when it is done.
- **G1, where labels are listed and changed** (F1, F17, F26). The sidebar's **Labels** above **Tags**, in the labels' order with counts (`marks_all`, which already merges the index's counts and the ledger's); a tag's row menu: *Rename everywhere…*, *Take off every file…* (both through `set_marks` over what a search finds, with Undo and words that say how many). **Settings → Files → Labels**: rename (the old name goes to `was`, and nothing on disk is rewritten: PLAN 5.5), recolor, add up to 32, reorder, take out. The settings file's part *Labels* (`settings/transfer.rs`). Two cards, most likely.
- **G2, the head and the results** (F5, F15, F23). The pane's head: a chip a label in the folder, `Important 4`, each in its color, beside the chips for kinds; pressing one puts `label:"Important"` in the search box, again takes it out; shown only when known (at once from `client::marks_in` where instant search covers the drive, else once the rows are read for folders of up to 5,000 items). **In comments**: a plain word also finds what has it in its comment or tags (`SearchRequest.marks`), listed under its own heading with the matching line under each, as *Inside files* is. The service never sends a comment's text: the app reads the line from the file, with the person's own rights.

**For every card that draws**, STANDARDS §5 is the law, and these are where such cards went wrong before:

- The kit and the theme only: no `gpui_component` in a view, no color or size of its own. A label's own color comes from `theme::label_tone`, a wash from `theme::label_wash`.
- **A label is not a state** (M11): never in the accent, never the whole line's text in color. In the list a selected row shows its selection alone (`file_list/cells.rs`), which is the rule for anything new that has a selected state.
- Every string through `t!`, as PLAN section 4 and 7.2 word it where they word it; the report lists every new string.
- Every thing that can be pressed has a role and a name; keyboard alone works (focus order, Enter, Escape).
- Right to left: start and end helpers, and a test with an Arabic name, tag and comment in everything that lays text out.
- Nothing on the UI thread but reading what is in memory. Marks come from `RowFacts`; changes go through `dialogs::tags::set_marks` (Undo, `TagsChanged`), never straight to core.
- Only what is known is drawn (M4).
- A list's test window must be wide enough for its columns: optional columns go while the name would have less than 14 rem (`file_list::layout::leave_room`).
- A test that lists a menu's rows or walks it with the arrows changes when a row is added: change it, and say so.

**The real `tag.dat`.** The issue's author attached a sample to issue 8 on November 23. **Do not fetch it**: the session that wrote the plan checks `glint_model::marks::xyplorer::read` against it and changes that reader and its fixtures if they differ (PLAN 14, items 1 and 2). F1 calls `xyplorer::read` as it is and keeps to its types.

## Part 2: run them

One agent for each card, all at the same time, on a cheaper model than your own (Sonnet), in this tree; each given `docs/gpui/STANDARDS.md`, section 6 of `docs/gpui/aware/PLAN.md` (with the card's own commands for its rule 4), the sections of `docs/gpui/marks/PLAN.md` its card lists, and its card; told that others are editing other files in the same tree, and that its last message is its section of the report. While they work, change nothing. When all have reported, read each report against its card and finish or send again what is partial. Then check the whole once: `cargo test --workspace --no-fail-fast` and `cargo clippy --workspace --all-targets -- -D warnings`.

**Every agent is told these, which the last wave learned the hard way:**

- **No command that walks a drive.** Last time an agent ran `find / -name …` and two of those were still running nine hours later on two cores. To find a file use the search tools, inside the repository; for a crate's source, `cargo metadata`. Before its last message an agent stops every process it started.
- **A build of one agent can meet another's half-written file.** Wait and build again; don't touch the other's file.
- **File-heavy tests that seem to hang** may be the PC: when it is locked, Windows' maintenance makes every new file wait on the antivirus (a 4 KB write takes 10 to 40 ms where it takes 0.3). Time a few dozen small writes before calling it a bug, and don't trust a timing failure from such a window.
- A test is run by its own filter (`cargo test -p <crate> <module>::`), not the whole workspace, until the end.

## Part 3: the report

`docs/gpui/marks/WAVE-4-REPORT.md`, in the shape of wave 3's: what you landed first; each card's status, files, tests, what it didn't do, what it needs, what it was unsure of; the totals; every new string; what was measured (an import of 10,000 lines: reading, finding, applying; an export of 10,000 marked items; the sidebar's counts; the head's chips on a folder of 5,000); what is left between the cards; what a card's wording got wrong; every new file. Say plainly what is partial, and what nobody has seen on a screen.

## What not to do

- **No commit, no push, no branch change**, and none of `git stash`, `checkout`, `restore` or `add`. The agents run no `git` command that changes anything, and never `cargo fmt` on a file that isn't theirs.
- **Nothing of wave 5** (the AI tools, Batch rename's `{label}`, a rule that labels, zips) **and nothing of wave 6** but E6's three: not the color tags' move (5.6), not the docs.
- **Not the end-to-end checks** (`tests\e2e\run.ps1`), and not the installed Glint: they take the real mouse, keyboard and profile. How it looks on a screen is checked afterwards.
- **No new crates.**
- **XYplorer's files are never written**, and nothing is downloaded.
- **`PLAN.md` is changed only** in section 9's table (the cards' links and rows) and in 8.1 (what you landed first).

When the report is written, say so, with the two totals.
