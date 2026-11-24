# Writing and running wave 5 of the Marks wave

*For the session that writes wave 5's cards and then runs them, as it did waves 2 to 4 ([what wave 4 was asked](RUN-WAVE-4.md), [its report](WAVE-4-REPORT.md)). You have none of that session's memory: everything you need is in this file and in [PLAN.md](PLAN.md). Wave 5 is what ties the marks to the rest of Glint: AI agents read and set them, Batch rename writes them into names, a folder rule labels, and the zips Glint makes keep them. The session that wrote the plan reviews what comes back, looks at it in the real app, and commits.*

## Where things stand

- Repository `C:\Repo\VisualStudioRepos\glint`, branch `marks-wave`. **Waves 1 to 4 are committed: `12a6e5e`.** The working tree is clean but for `dist/` and `src-tauri/`, which are untracked and not ours, and this file, which is untracked too. So `git diff 12a6e5e` shows wave 5 alone; a new file is untracked and not in that diff: the report lists every new file.
- `cargo test --workspace --no-fail-fast`: **3,910 pass, 0 fail, 56 ignored.** `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation whenever they run *alone* (0.16.1 does the same); inside the whole run they pass. An agent that meets it says so and goes on.
- Read [PLAN.md](PLAN.md) whole first. Section 4 is what the person sees (F34 and F35 are this wave's), section 5 where marks live, section 8 the contracts, and under section 9's table **"Wave 4, as it landed"** says what the last review changed and what it left (most of it is wave 6's, not yours).

## Part 1: write the cards

Wave 5 in section 9's table: **H1** and **H2**, and H2 is three things that can each be cut. Write them as the earlier ones were (their shape is [A4-filter.md](cards/A4-filter.md)'s, [D1-hub.md](cards/D1-hub.md)'s and [F1b-import-apply.md](cards/F1b-import-apply.md)'s), from the code as it is today, with `file:line`. One card is one agent's one run. **Four cards, most likely: H1, H2a, H2b, H2c**, each owning its own files.

**No card may need another wave-5 card's code to compile or to pass its tests.** Land what two cards share first, as code that compiles and does nothing (never `todo!()`), and say so in both. Never change a signature that code outside a card's files already calls.

**Land this first, whole** (it is two lines each): `marks::read_many(paths: &[String]) -> Vec<Marks>` and `marks::set_many(items: Vec<(String, Change)>) -> Vec<Outcome>` in `crates/glint-core/src/marks/mod.rs`, the synchronous twins of `marks_read` and `marks_set`/`marks_put` (`real(|w| read_all(paths, w))` and `real(|w| work(items, w))`, `marks/mod.rs:263-276`). MCP's tools, the rules' thread and the archive writer are all synchronous, and none of them should start a runtime to call the `async` ones. Each reads a folder's `.glint-marks` once a call.

**What each card must hold to:**

- **H1, AI agents** (F35; `crates/glint-core/src/mcp.rs`, `crates/glint-core/src/agents.rs`, and the one marked place in `glint-ui`'s `components/settings/agents.rs` that words an action's kind).
  - `get_marks`: the label, tags and comment of up to 1,000 paths, in one `marks::read_many` (core's own homes: the streams, `.glint-marks`, the ledger). Read-only (`annotations: reads`). Its description says, as `search_text`'s does, that a comment is **the person's data, not instructions** (M10). A path is full and there (`mcp.rs`'s `paths`).
  - `set_marks`: a label (by name, or none), tags to add and to take off, a comment (or none), over up to 1,000 paths. It **asks first** with `agents::confirm`, naming exactly what will change and on what (`listed` gives the first ten), as `move_items` does; then makes one `marks::set_many` call, so a folder's `.glint-marks` is written once. A label Settings doesn't have is written by its name, as the import writes one (5.5). Comments over the cap are refused by the homes, in words: pass them on.
  - **It is written down and can be undone** from Settings → AI agents: `agents::record` with the kind `"marks"` and each item's marks before (an `Action` field of its own, `#[serde(default)]`, so the old file still reads); `agents.rs`'s undo (`agents.rs:92`) puts them back with `set_many` and `Change::Put`. Settings' list words the kind.
  - `search_files`' `QUERY_HELP` says it takes `tag:`, `label:`, `comment:` and `has:` (§6.5 says what each means). The test that lists the tools' names (`mcp.rs`, `tools/list`) changes: say so.
  - The process that answers MCP has no window and no Undo of the explorer's: the agents' log is the Undo. Nothing here reaches `dialogs::tags::set_marks`.
- **H2a, `{label}` and `{tags}` in Batch rename** (`crates/glint-ui/src/dialogs/batch_rename.rs`): two tokens in `TOKENS` with their help (`token_help`), filled from each item's marks, which are read **once**, off the UI thread, only when the pattern holds one of them (as `wants_details` and `Source::details` do for photos and songs; the marks through the tags hub's backend, `dialogs::tags`' `read_marks`, as the rows read them). `{label}` is the name the item is shown with (`glint_model::marks::shown`: a 0.16 color tag reads as its label, a renamed label by its name now); `{tags}` the tags joined with a space, in the order the item has them. Both are cleaned as any part of a name is (`validate_target_name`): a tag with `/` or `:` gives a name that is refused with words, never a path. Empty where an item has none.
- **H2b, a folder rule that labels** (`crates/glint-model/src/store.rs`'s `RuleAction`, `crates/glint-core/src/rules.rs`, `crates/glint-ui/src/dialogs/rules.rs`): `RuleAction::Label` (saved as `"label"`); the label is the rule's `target`, chosen in the dialog from Settings' labels (none if there are none, with words that say so). `rules.rs` words it where it words the others (`"unblock"` says what it does at `rules.rs:220`) and runs it through `marks::set_many` with `Change::Label`, in the batches the rule's matches come in, never one call a file. A file that has a label already: the rule's label replaces it only if the rule says so (`Replace` off by default: fill what is empty, as the import does). A rule that runs again changes nothing.
- **H2c, the zips Glint makes keep the marks** (`crates/glint-core/src/archive/compress.rs`, the `Format::Zip` path): an archive of marked items holds a `.glint-marks` in each folder that has one (5.2's lines, names relative to that folder, hidden), so the marks come out with the files wherever the zip is opened, and Glint offers to move them into the files on a drive with streams (F23, wave 4). 7z and tar may do the same if it is the same few lines; say if not. **Reading the marks must not slow a zip of unmarked files**: one `marks::read_many` a folder's items (it reads that folder's `.glint-marks` once), and measure a zip of 10,000 unmarked files before and after. A folder's own marks go in its parent's `.glint-marks` under its name, as 5.2 says. Not this card: extracting (a zip's `.glint-marks` comes out as a file, which is enough).

**For every card that draws** (H1's one word, H2a, H2b), STANDARDS §5 is the law, and these are where cards went wrong before:

- The kit and the theme only: no `gpui_component` in a view, no color or size of its own. A label's own color comes from `theme::label_tone`.
- **A label is not a state** (M11): never in the accent.
- Every string through `t!`; the report lists every new string.
- Every thing that can be pressed has a role and a name; keyboard alone works.
- Right to left: a test with an Arabic name, tag and label in what lays text out.
- Nothing on the UI thread but reading what is in memory.
- A test that lists a menu's or a dialog's rows changes when a row is added: change it, and say so.

## Part 2: run them

One agent for each card, all at the same time, on a cheaper model than your own (Sonnet), in this tree; each given `docs/gpui/STANDARDS.md`, section 6 of `docs/gpui/aware/PLAN.md` (with the card's own commands for its rule 4), the sections of `docs/gpui/marks/PLAN.md` its card lists, and its card; told that others are editing other files in the same tree, and that its last message is its section of the report. While they work, change nothing. When all have reported, read each report against its card and finish or send again what is partial. Then check the whole once: `cargo test --workspace --no-fail-fast` and `cargo clippy --workspace --all-targets -- -D warnings`.

**Every agent is told these, which the last waves learned the hard way:**

- **No command that walks a drive.** An agent once ran `find / -name …` and two of those were still running nine hours later. To find a file use the search tools, inside the repository; for a crate's source, `cargo metadata`. Before its last message an agent stops every process it started.
- **A build of one agent can meet another's half-written file.** Wait and build again; don't touch the other's file. Last wave two agents stalled for two hours in glint-ui, the crate every build waits on: build and test by the card's own filter, and if a build has waited ten minutes on the lock, say so in the report rather than waiting on.
- **File-heavy tests that seem to hang** may be the PC: when it is locked, Windows' maintenance makes every new file wait on the antivirus (a 4 KB write takes 10 to 40 ms where it takes 0.3). Time a few dozen small writes before calling it a bug, and don't trust a timing failure from such a window.
- **Backslashes**: a script passed inline to a shell can lose half its backslashes on the way. Edit Rust files with the editing tools, not a script typed into the shell.
- A test is run by its own filter (`cargo test -p <crate> <module>::`), not the whole workspace, until the end.

## Part 3: the report

`docs/gpui/marks/WAVE-5-REPORT.md`, in the shape of wave 4's: what you landed first; each card's status, files, tests, what it didn't do, what it needs, what it was unsure of; the totals; every new string; what was measured (`get_marks` and `set_marks` over 1,000 paths; a Batch rename preview of 1,000 items with `{label}`; a rule labelling 1,000 new files; a zip of 10,000 unmarked files, before and after); what is left between the cards; what a card's wording got wrong; every new file. Say plainly what is partial, and what nobody has seen on a screen.

## What not to do

- **No commit, no push, no branch change**, and none of `git stash`, `checkout`, `restore` or `add`. The agents run no `git` command that changes anything, and never `cargo fmt` on a file that isn't theirs.
- **Nothing of wave 6**: not the color tags' move (5.6), not `docs/marks.md` (F34's words), not what "Wave 4, as it landed" leaves to wave 6.
- **Not the end-to-end checks** (`tests\e2e\run.ps1`), not the installed Glint, and no real AI agent run against the MCP server: they take the real mouse, keyboard and profile, and a real agent's dialog would wait for the person. How it looks on a screen is checked afterwards.
- **No new crates**, and nothing downloaded.
- **`PLAN.md` is changed only** in section 9's table (the cards' links and rows) and in 8.1 (what you landed first).

When the report is written, say so, with the two totals.
