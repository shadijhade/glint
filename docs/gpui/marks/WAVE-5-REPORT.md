# Wave 5 of the Marks wave: report

*Written 2026-11-24 by the session that wrote and ran wave 5 ([what it was asked](RUN-WAVE-5.md)), against `12a6e5e`. Four cards, one Sonnet agent a card, all at once in this tree. Nothing is committed.*

## The whole

- `cargo test --workspace --no-fail-fast`: **3,945 passed, 0 failed, 60 ignored** (wave 4 left 3,910 / 0 / 56; the four new ignored tests are the cards' measurements). The access violation of `color_wheel` alone did not come up.
- `cargo clippy --workspace --all-targets -- -D warnings`: **clean**.
- Every card came back done but H1, which said partial for one reason: `settings/tests.rs` builds an `agents::Action` literal, which H1's new field broke. The coordinator added `marks: Vec::new()` there (`settings/tests.rs:257`) while H2a and H2b were still building, since it stopped glint-ui's tests compiling for them too. Nothing else was finished or sent again.
- **Nobody has seen any of it on a screen**: not the two tokens in Batch rename, not the rule editor's labels, not the agents' dialog, not a zip's `.glint-marks` opened by Explorer or another tool.

## Landed before the cards, and why

- `marks::read_many(paths: &[String]) -> Vec<Marks>` and `marks::set_many(items: Vec<(String, Change)>) -> Vec<Outcome>` in `crates/glint-core/src/marks/mod.rs`: `real(|w| read_all(paths, w))` and `real(|w| work(items, w))`, the synchronous twins of `marks_read` and `marks_set`/`marks_put`. MCP's process, the rules' thread and the archive writer are synchronous and have no runtime. Both are whole (two lines each), so no card needed another's code. PLAN §8.1 says so.

## The cards

### H1 — AI agents: `get_marks`, `set_marks`, written down and undone — done (it said partial for the literal above)

- **Files**: `glint-core/src/mcp.rs`, `glint-core/src/agents.rs`, the intro's two sentences of `glint-ui/src/components/settings/agents.rs`.
- `get_marks`: at most 1,000 full paths that exist, one `read_many`, `{ items: [{ path, label, tags, comment }] }`, read-only. Its description says what a comment says is the person's data, not instructions.
- `set_marks`: `label` (or `null`), `addTags`, `removeTags`, `comment` (or `null`); nothing to change is refused before anything is asked. It asks first through a seam, `set_marks_with(args, confirm, log)`, so no test shows a dialog or writes the real log. Then one `read_many`, afters by `Change::apply`, and one `set_many` of `Change::Put` for the items that change. Failures come back in `failed` with the homes' words.
- **The log**: `Action.marks` (`#[serde(default, skip_serializing_if)]`) holds the befores; an `agent-actions.json` from 0.16.1 reads (tested). `record` keeps its signature; `record_marks` is new, and the log's functions now take a path (`actions_at`, `save_at`, `record_at`, `undo_at`) for the tests. Undo of `"marks"` puts the befores back with one `set_many`.
- `QUERY_HELP` names `tag:`, `label:`, `comment:`, `has:` and `!`. The MCP server's `INSTRUCTIONS` and Windows' agent registry manifest name the two tools.
- **Tests**: `mcp::` 7 passed, 1 ignored; `agents::` 2 passed. The card's ten cases.
- **Unsure**: a comment over 64 KB fails every item of the call, not just one. The comment is part of each item's one `Put`, so the card's "the others in the call still change" can't hold. The test shows it with a drive root, which fails while a file in the same call changes. The question always starts *change the marks of N items:* and then lists each change.

### H2a — `{label}` and `{tags}` in Batch rename — done

- **Files**: `glint-ui/src/dialogs/batch_rename.rs` alone; `Source` and `open_with` are unchanged.
- `TOKENS` is 15 long. `wants_marks` sits beside `wants_details`. The marks are read once, through the tags hub's backend (`read_marks`), the first time the pattern holds a marks token. The labels are read when the marks land. Each item's text is worked out then, and `simulate` takes the map as a fifth argument. Rename waits for the marks (`reading()`).
- `{label}` is `shown(..).label`, so a 0.16 color tag reads as its label and a renamed label by its new name. `{tags}` is `shown(..).tags` with a space between, so the color tag isn't repeated. They go through `validate_target_name`, not `safe`: a tag with `/` or `:` gives an `InvalidCharacters` row that can't be applied.
- **Tests**: `dialogs::batch_rename` 45 passed, 1 ignored; eight new, Arabic included.
- **Unsure**: `{label}` and `{tags}` are filled last in the chain, so a tag holding `{n}` is not expanded. **With no label, `{label} {name}` gives ` report.pdf`, with a leading space the row accepts**, as other empty tokens do today. The person may want a token that leaves no gap. Marks are not read again while the dialog is open.

### H2b — A folder rule that labels — done

- **Files**: `glint-model/src/store.rs` (`RuleAction::Label` saved as `"label"`, `FolderRule.replace` saved as `"replace"` and left out while false), `glint-core/src/rules.rs`, `glint-ui/src/dialogs/rules.rs`.
- Core: `label_with` does one `read_many` and one `set_many` (`Change::Label`) for the files that change. `tick` gathers a tick's files by rule; `sweep` and `rules_apply` pass their batches. It fills what is empty unless `replace` is set. A file whose label is already the rule's is neither written nor logged, so a rule that runs again changes nothing. Undo puts back what was there, but only while the file's label is still the rule's.
- UI: *Give it a label* among the actions, after Rename. Settings' labels show as named choices with their dot in `label_tone`, the chosen one checked, and a *Replace a label a file already has* switch. With no labels it shows the words below, and *What would it do?* is disabled.
- **Tests**: `store::` 28, `rules::` (core) 10 and 1 ignored, `dialogs::rules` 26, all passing. One test counts one read and one set a batch.
- **Decided by the agent, to look at**:
  - A label rule never takes a `.glint-marks`, since writing a sidecar wakes the watcher.
  - Changing the action between Label and another clears `target`, so a typed folder is lost.
  - A label is compared with the rule's in any case, not through `was`.
  - The log's `Done` keeps the label it gave in `summary` and the one it had in `mark`.
  - A Label rule saved as on before its labels were removed stays on until switched off.
  - The labels' choices sit below the fold of the editor's scrolling fields, as the sync choices do.

### H2c — The zips Glint makes keep the marks — done

- **Files**: `glint-core/src/archive/compress.rs` alone; `compress_as` and `compress` keep their signatures.
- **Reads**: one `read_many` over every item gathered, not one a folder as the card said. It reads each folder's `.glint-marks` once either way.
- **What it writes**:
  - A `.glint-marks` (`write_beside`) goes in each archive folder that has a marked item.
  - A folder's own marks go in its parent's, and items at the top in one at the archive's top.
  - A `.glint-marks` on disk is not copied as a file.
  - Nothing is written for unmarked items.
- **Each format**:
  - Zip: hidden (DOS attributes `0x22`).
  - 7z: hidden too (`windows_attributes`).
  - tar.zst: carries the file, but tar has no hidden attribute.
  - With a password, the `.glint-marks` is encrypted like the rest.
- **Tests**: `archive::compress` 10 passed, 1 ignored.
- **Unsure**: each line's `at` is 0 (unknown), where 5.2's lines carry when the mark was set. A `.glint-marks` named in `paths` itself is dropped too.

## Tests that were there and changed

- `mcp::tests::speaks_the_protocol`: the tool list gains `get_marks` and `set_marks` after `search_text`.
- `store::tests::choices_are_handed_to_core_as_the_words_they_are_saved_as`: gains `RuleAction::Label`.
- `dialogs::rules`' `the_icons_this_file_draws_are_embedded`: gains `IconName::Tag`, which the no-labels note draws.
- `batch_rename`'s callers of `simulate` pass `None` as the new last argument; no assertion changed.
- `settings/tests.rs`' fake `agent_actions`: `marks: Vec::new()` (the coordinator).

## Every new string

Through `t!`:

- `The item's label`, `The item's tags, with a space between` (H2a)
- `Give it a label`, `labeled`, `There are no labels yet: Settings → Files → Labels makes them`, `Replace a label a file already has`, `Label` (H2b; `Label` may exist already)
- Changed, H1, Settings → AI agents' intro:
  - `AI agents that speak the Model Context Protocol can ask Glint to find files by name on every drive in milliseconds (with instant search on), measure folders (as a map, in agents that show apps), list the biggest files, say what was moved or deleted lately, search text inside files and read the labels, tags and comments you gave files.`
  - `They can also ask to move, rename, make folders, recycle and label, tag or comment files: Glint asks you first each time, in a Windows dialog naming exactly what will happen, and lists what was done below, with Undo. Once told how, the agent starts Glint for itself, and sees the names and text it asks for.`

Core's own words, not through `t!`, as core's other strings are:

- H1: the tools' titles and descriptions; *At most 1,000 paths at a time*; *Say what to change: label, addTags, removeTags or comment*; *The person said no: no marks were changed*; the question *change the marks of N items: …*; the summaries *Changed the marks of N item(s)* and *Labeled N item(s) “…”*.
- H2b: *Label it {target}*; *Label it {target}, in place of a label it has*; *Which label? Choose one in the rule*; *Its label has changed since*; *The label that was given isn't known*; *It wasn't labeled*.

## Measured

Debug builds, one PC, with the other agents building beside them, so every number is high and noisy. None was taken in a quiet release build.

| What | Time |
| :--- | ---: |
| `get_marks`, 1,000 files in one folder | 128 ms |
| `set_marks` (a label, a tag and a comment), 1,000 files on NTFS | 2.76 s |
| Batch rename's `simulate`, 1,000 items with `{label}` and their marks in hand | 9.3 ms (one run) |
| A Label rule applied to 1,000 new files (`rules_apply`) | 530 ms (one run) |
| A zip of 10,000 unmarked 64-byte files, marks' step skipped, median of 5 | 2,983 ms |
| The same, with the marks' step, median of 5 | 3,165 ms |

The zip's runs overlap heavily: before 2,368 to 3,229 ms, after 2,574 to 3,504 ms. The marks' step costs about 0.02 ms a file, a few percent. A test-only flag (`SKIP_MARKS`) skips the marks' step for the "before" runs. `set_marks` writes three streams a file, and wave 4 measured about 4 ms an item for an import's writes on a busy PC; a quiet release build is wave 6's.

## Left between the cards

Nothing one card needs from another. To wire or decide:

- H1's `set_marks` changes files the explorer may show, from another process. The rows learn of it through the folder watcher (PLAN §14 item 4), not through `TagsChanged`, and Ctrl+Z in the explorer doesn't know of it: the agents' log is its Undo, as the card said.
- H2b's rule labels through core, so neither the hub's `TagsChanged` nor the explorer's Undo hear of it. The watcher shows it, and the rules' log is its Undo, as every rule's is.
- H2c: extracting is untouched. A zip's `.glint-marks` comes out as a file, and F23's *Move them into the files* takes it from there.

## What a card's wording got wrong, and what to look at

- **H1**: `RUN-WAVE-5.md` and the card speak of "the one marked place in `settings/agents.rs` that words an action's kind". There is none: the list shows an action's `summary`, so only the intro changed. The card's "the others in the call still change" for a comment over the cap can't hold with one `Put` an item (above).
- **H2a**: the card asked what an empty `{label}` gives. It gives a leading space, kept as it is.
- **H2b**: the card put the label given in no field. The agent used `summary` for it and `mark` for the label the file had.
- **H2c**: the card said one `read_many` a folder's items; one for all is the same reads.

## Not run, and not seen

- Nothing on a screen; both themes, 125%, keyboard alone in the rule editor's label choices, and the Arabic layouts are only tested.
- No real AI agent against the MCP server, and no real `confirm` dialog (the tests go through the seam).
- No drive without streams. H2b's batch shows one read and one set by a count, not by a sidecar written once on exFAT.
- A zip's `.glint-marks` opened by Windows' own zip folders or by 7-Zip: whether either hides it is unchecked.
- No end-to-end checks.

## Every new file

- `docs/gpui/marks/cards/H1-agents.md`, `H2a-batch-rename.md`, `H2b-rule-label.md`, `H2c-zip-marks.md`
- `docs/gpui/marks/WAVE-5-REPORT.md` (this file)
- `docs/gpui/marks/RUN-WAVE-5.md` was there, untracked, before this wave began.

No card made a new source file. Every change is in `git diff 12a6e5e`: 11 files, the cards' own plus `marks/mod.rs` (landed first), `settings/tests.rs` (one line) and `PLAN.md` (§8.1 and §9's rows).
