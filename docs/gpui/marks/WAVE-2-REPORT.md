# Wave 2 of the Marks wave: report

*November 23, 2026 · branch `marks-wave` at `13660c7`, nothing committed · the tree before this wave is `4dd2162` (`git diff 4dd2162` shows this wave alone) · five cards written here, then five agents on Sonnet (claude-sonnet-5-5), one a card, in the one tree.*

All five cards came back **done**; none is partial. Against `4dd2162`: 19 files changed and one new (`far.rs`), 3,118 lines added and 194 removed, each file a card's own or one of the signatures landed first. **One thing is not clean**: a test in a file no card owned, `aware::probe::tests::a_picture_asked_for_its_media_says_its_size`, now fails in some full runs of glint-core (below). It was not patched.

## The whole

| Command | Result |
| :--- | :--- |
| `cargo test --workspace --no-fail-fast` | **3,643 passed, 2 failed, 50 ignored.** The two: `thumbs::tests::draws_a_pdfs_first_page` (known before this work) and `aware::probe::tests::a_picture_asked_for_its_media_says_its_size` (new, intermittent). |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** |

By crate: glint-core 419 passed, 2 failed, 15 ignored · glint-model 417 passed, 2 ignored · glint-ui 2,659 passed, 15 ignored · shadui-index 148 passed, 7 ignored · one more target 11 ignored.

**The intermittent failure.** `aware/probe.rs`'s test asks the quiet lane for a 1×1 PNG's size and waits five seconds for the answer (`wait_for`, line 171). Alone it passes (run twice), and `cargo test -p glint-core aware::probe` passes. In full runs of glint-core after this wave it failed in the workspace run and in one of three more runs of `cargo test -p glint-core` (2 of 4 in all; the other two had the PDF test alone, 420 passed). Before this wave it had not failed in the runs on record (wave 1's, and the 3,562 of the integration). Nothing in this wave touches `aware/` or the lanes; the likeliest cause is the load of the 20 new tests of `marks/` beside it (they write streams and sidecars and read 600 paths on every core), which makes the quiet lane's answer late. That is a guess: the failing run's message was not captured. For the session that reviews this: either the test's wait is too short for a loaded machine, or B2's tests should be lighter.

## Landed before the cards, and why

Each compiles and did nothing until its card filled it; PLAN §8.1 lists them.

- **glint-model, `entries.rs`**: `MarksOf` (an entry's marks as read, `None` while unread), `MarkReads { of, labels }` and the field `Reads::marks`. `Reads` is built in glint-ui (`explorer/rows.rs:611`), which is not A5's: a field A5 added itself would have broken D1's crate while both were working. `entry_order` and `rows.rs` pass `None`.
- **shadui-index, `far.rs`** (new, `pub mod far` in `lib.rs`): `far::read(path)`, `far::drain(tree, letter, stop)`, `far::BATCH`. C2's thread calls `drain` and C3 writes it; this is where the card that was too big was cut in two.
- **shadui-index, `client.rs`**: `marks()` and `marks_in(path)`, answering `Err` until C3. B2's `marks_all` calls the first.

Nothing was needed between B2 and D1 (S0's `marks_read`, `marks_set` and `marks_put` were there), nor for D1's `UndoRunner` and `Backend` (both traits and all the impls it had to change are in its own files; the fakes elsewhere stand on default bodies).

**C2 became two cards**, as RUN-WAVE-2 allowed: [C2](cards/C2-engine.md) (the engine: `engine.rs`, `proto.rs`, one new method in `tree.rs`) and [C3](cards/C3-far.md) (`far.rs`, `client.rs`). PLAN §9's table has both rows.

## The cards

### A5 — The filter in the open folder; sort and group by a mark; plain words — done
- **Files:** `glint-model/src/marks/wanted.rs`, `entries.rs`, `groups.rs`, `plain.rs`.
- **Tests:** `cargo test -p glint-model` → 417 passed, 0 failed, 2 ignored; 16 new (5 in `wanted.rs`, 6 in `entries.rs`, 2 in `groups.rs`, 3 in `plain.rs`), the card's 14 items. The fixture's 69 cases all hold: the two copies of the filter agree.
- **Clippy:** clean.
- **Not done:** nothing.
- **New names:** `entries::matcher_marks`, `entries::has_marks_filter`, `groups::group_order_marked`, `plain::plain_query_with`. Every function other crates call kept its signature.
- **Needs:** the wiring (below).
- **Unsure:** in a descending sort, entries with equal marks stay in ascending name order (as every other key does); in plain words a known label's name is matched before the filler words, so *In review* works though `in` is filler; a label whose name holds `"`, `;` or `:` is left out of plain words; the sort works out what is shown at every comparison (a `ponytail:` comment says to make the keys once if a large folder sorted by label is slow).

### B2 — The three homes — done
- **Files:** `glint-core/src/marks/mod.rs`, `beside.rs`, `ledger.rs`, and `tags.rs` (`tags_read`, `tags_set`, and two lines beyond them: see "wording").
- **Tests:** `cargo test -p glint-core marks::` → 35 passed, 0 failed (17 new in `mod.rs`, 3 in `ledger.rs`, with B1's 15); `cargo test -p glint-core tags::` → 4 passed. The card's 18 items are there. `beside.rs` has no tests of its own: it is tested through `mod.rs`.
- **Clippy:** clean. One `#[allow(clippy::permissions_set_readonly_false)]` in a test.
- **Not done:** nothing.
- **New name:** `marks::home_of`.
- **What it costs:** 200 unmarked paths are 200 stream listings and one look for `.glint-marks` a distinct folder; the ledger is not locked while it is empty. The ledger is saved at the end of each call that changed it, not on a timer.
- **Needs:** nothing.
- **Unsure, and decided by the agent:**
  - a change to an item that isn't there goes straight to the ledger (beside, its line would be dropped at once as an item that is gone);
  - the first line's other keys (the fixture's `"writer"`) are not kept on a rewrite: `Beside` has no field for them, only the version;
  - `beside::line` is a scan (`ponytail:` comment: a map when a folder of 100,000 lines is measured);
  - `ledger::shared()` takes its path the first time it is asked: asked before the data folder is set, the ledger stays in memory for the process;
  - a relative path is refused with the words for a drive's root;
  - `marks_all`'s index part has no test (the call into the service can't be faked).

### C2 — The engine — done
- **Files:** `shadui-index/src/engine.rs`, `proto.rs`, `tree.rs` (`search_marks` and its tests; `search_until` untouched).
- **Tests:** `cargo test -p shadui-index` → 148 passed, 0 failed, 7 ignored; 10 new (6 in `engine.rs`, 1 in `proto.rs`, 3 in `tree.rs`), the card's ten.
- **Clippy:** clean.
- **Not done:** nothing. `search_marks` was not timed.
- **Needs:** nothing in `pipe.rs`, `cli.rs` or `client.rs` for `Hit::by` (none builds a `Hit`). Whatever in the app reads hits gets the new field through `decode_hits`.
- **Unsure:**
  - both flags are set when words were found in the comment and in a tag;
  - marks are looked for as typed and with the nearer filter, not beside the names' own close matches;
  - `Done` says `fuzzy: true` when the nearer filter found something or the names' close matches ran;
  - the "first spelling" of a label merged across records can't be tested in `marks()` (a `HashMap`'s order decides which is met first), so that test compares in lower case;
  - `marks_in` says "Not a folder" for a path that is a file, as `sizes` does;
  - `search_until`'s checks were not moved into a helper: `search_marks` repeats them in the same order, so the two must be kept alike by hand.
- **Also:** a private `merged()` in `engine.rs` that `tags()` uses too, answering as before.

### C3 — The far reads; the app's two calls — done
- **Files:** `shadui-index/src/far.rs`, `client.rs`.
- **Tests:** `cargo test -p shadui-index far::` → 12 passed; `client::` → 5 passed (3 new). The card's 13 items, and one more: a link is not followed.
- **Clippy:** clean. No `unsafe`.
- **Not done:** one `read` of an unmarked file was not timed (it is one open for attributes and three opens that fail). **The link was not proved**: the test returns early when a symlink can't be made, and the agent could not tell whether it was made on this PC; no junction was tried. So `FILE_FLAG_OPEN_REPARSE_POINT` is in the code and its effect is unverified.
- **Needs:** nothing.
- **Unsure:** nothing beyond the link.

### D1 — The hub takes marks; RowFacts keeps them; Undo — done
- **Files:** `glint-ui/src/dialogs/tags.rs` (the hub's part and the tests; the dialog's part untouched), `components/row_facts.rs`, `explorer/ops_types.rs`, `explorer/undo.rs`.
- **Tests:** `cargo test -p glint-ui dialogs::tags` → 20 passed (10 new); `components::row_facts` → 24 passed (8 new); `explorer::undo` → 14 passed (3 new); `components::sidebar` 55 and `file_list::tests::facts` 6, unchanged. The whole crate ran in step 5: 2,659 passed.
- **Clippy:** clean.
- **Not done, said by the agent:** two of the card's tests could not be written as worded (see "wording"): the failure's title is tested through a function `failure_words`, not by reading the notification; the Undo step is checked by taking it back and reading what the fake runner was asked to put, not by reading `UndoAction::Marks` out of the explorer.
- **New names:** `Backend::put_marks`, `UndoRunner::put_marks`, `RowFacts::has_all`, `RowFacts::marks_among`, `fake::MarksBackend` (tests).
- **Strings for `t!`:** `label of {what}`, `tags of {what}`, `comment of {what}`, `marks of {what}`; `Changed the label of {what}`, `Changed the tags of {what}`, `Changed the comment of {what}`, `Changed the marks of {what}`; `Couldn't mark “{name}”`, `Couldn't mark {n}`, `Couldn't change the marks`.
- **Unsure:** a whole read that fails or comes back short is kept as empty marks and counts as landed (`has_all` is true), as a failed batch does; when every entry is already fresh `ask_all` says `has_all` without calling anything; while rows are still on batched reads `ask_all` waits for them, so the caller asks again each frame until `has_all`; the Undo offer and a failure are two notices, so a change that half worked shows both.

## Left between the cards (for the session that integrates)

Calls nobody makes yet:

- **The rows' marks into the model.** `explorer/rows.rs` still passes `Reads { marks: None }` and calls `matcher` and `group_order`: it has to hand `RowFacts::marks_of` to `Reads::marks`, `matcher_marks` and `group_order_marked`, and call `ask_all` (each frame until `has_all`) when `has_marks_filter` holds or the sort or group is by a mark. Until then `tag:` typed over an open folder shows **every** row (the marks terms are left to the search), where it showed none.
- **`plain_query_with`**: `address_bar/find.rs` and `quick_search/mod.rs` still call `plain_query`, with no labels.
- **`set_marks`**: nothing calls it but `set_tags` (the menu events, the keys and the dialogs are wave 3).
- **`marks_among`**, `marks::home_of`, `marks::matches` (fd's drives: B3), `marks_all` (the sidebar: G1), `client::marks_in` (the sort from the index, PLAN §6.6).
- **`SearchRequest.marks`**: every request Glint builds still says `false`, so `search_marks` and `Hit::by` run only from the command line's `--marks`.
- **The far thread** runs in the service from this wave on: it opens files for the first time. `docs/instant-search.md`'s security model does not say so yet.

Kept by hand, in two places:

- `search_until` and `search_marks` make the same checks after the name's (C2).
- The four lists of filter keys in glint-ui (PLAN §6.5) still don't read `marks::KEYS`.

## What a card's wording got wrong, and what to look at

- **All five cards, a slip of mine.** They were written with a stand-in for the backtick that was also the section sign, so "§3" reached the agents as a backtick before the number for the first seconds of the run. The files were corrected right after the agents started; the numbers read the same either way, and no report shows an agent misled by it.
- **D1, tests 1 and 4.** The card asks to read the step's `UndoAction` and the notification's title. `Explorer::ops` is private to `explorer` and a `Notification`'s title is private to the kit, so neither can be read from `dialogs/tags.rs`'s tests. The agent tested the same facts another way.
- **B2, "nothing else of `tags.rs`".** With `tags_set` going through the homes, `tags::write` has no caller left but the file's test, and clippy refuses dead code: the agent put `#[cfg(test)]` on `write` and moved its `use std::io::Write` inside. The words `This drive keeps no tags (FAT or exFAT)` are gone with the old body (such a drive now takes the marks beside the files). Worth a decision: delete `write` and have the test use `streams::write`, or leave it.
- **B2, an item that isn't there.** The card sends every item through the homes in order; a line beside a missing item would be dropped by the same write (§5.2). The agent sends it to the ledger. The card should have said so.
- **B2, the first line's other keys.** PLAN §5.2 says "keys it does not know are kept"; `Beside` (A2) keeps them for item lines and only the version for the first line. The card's test 6 asks for no more. If a later version's first-line keys must survive, `Beside` needs a field.
- **C2, `marks()`'s first spelling.** The card's test 5 asks for it; across records it depends on a `HashMap`'s order, so it can't be asserted.
- **C3, the link.** The card asked the agent to try one; it couldn't be sure it had. To try by hand: `mklink /J` a junction to a marked folder and read it with `far::read`.
- **Line ends**, as in wave 1: the files the agents wrote have LF, and Git says it will make them CRLF.
- **While they ran:** C3's first build met C2's half-written `engine.rs`, waited, and passed. Nobody touched a file that wasn't theirs, except B2's two lines in `tags.rs` above.

## Not run

- The end-to-end checks (`tests\e2e\run.ps1`): they take the real mouse and keyboard.
- `cargo fmt`: by the rules.
- Nothing of waves 3 to 5.
