# Wave 1 of the Marks wave: report

*November 23, 2026 · branch `marks-wave` at `13660c7`, nothing committed · six agents on Sonnet (claude-sonnet-5-5), one a card, in the one tree.*

All six cards came back **done**; none is partial. Twelve files changed (2,409 lines added, 93 removed), each owned by a card; nothing else was touched. The coordinator of this run changed no source file: step 5 found nothing to fix.

## The whole

| Command | Result |
| :--- | :--- |
| `cargo test --workspace --no-fail-fast` | **3,561 passed, 1 failed, 50 ignored.** The one is `thumbs::tests::draws_a_pdfs_first_page`, the failure known before this work. |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** |

By crate: glint-core 400 passed, 1 failed, 15 ignored · glint-model 401 passed, 2 ignored · glint-ui 2,638 passed, 15 ignored · shadui-index 122 passed, 7 ignored · one more target 11 ignored.

## The cards

### A1 — Marks, the labels, and what an import does — done
- **Files:** `crates/glint-model/src/marks/mod.rs`, `crates/glint-model/src/marks/labels.rs`.
- **Tests:** `cargo test -p glint-model marks::` → 30 passed, 0 failed (the count holds A2's and A3's too, as they were then). 17 tests in `mod.rs`, 13 in `labels.rs`; the card's 11 items are all there.
- **Clippy:** stopped on A3's file when A1 ran it; clean in the workspace run.
- **Not done:** nothing.
- **Needs:** nothing now.
- **New:** `tidy` is `glint_model::marks::tidy` (`mod.rs`); `settled` is in `labels.rs`, re-exported beside `shown` as `glint_model::marks::settled` (A1 added it to the `pub use labels::{…}` line).
- **Unsure:** `settled` renames a label only when it matched through a `was` name. A label that differs from the label's own name only in case, or Grey for Gray, stays as written (`red` stays `red`).

### A2 — The lines — done
- **Files:** `crates/glint-model/src/marks/lines.rs`. `LEDGER_MAX_BYTES` (`64 << 20`) is new.
- **Tests:** `cargo test -p glint-model marks::lines` → 11 passed, 0 failed. The card's items 5, 6 and 7 share tests with their neighbours.
- **Clippy:** nothing in `lines.rs`; clean in the workspace run.
- **Not done:** nothing.
- **Needs:** nothing.
- **Unsure:** a later line with no marks for the same item replaces the earlier one, so the item is dropped; a `"glint-ledger"` line is skipped only as the first line that isn't empty; `"at"` counts only when it is an integer that fits `i64` (a float or a larger number reads as 0).

### A3 — XYplorer — done
- **Files:** `crates/glint-model/src/marks/xyplorer.rs`.
- **Tests:** `cargo test -p glint-model marks::xyplorer` → 12 passed, 0 failed, the card's twelve. The 10,000-line read is under 500 ms in the test build; the release build was not timed.
- **Clippy:** clean (the two lints A1 and A2 saw in this file were fixed by A3 before it reported).
- **Not done:** nothing.
- **Needs:** nothing. The file's long test lines were formatted by hand, not by rustfmt.
- **Unsure:** no real `tag.dat` has been seen, so these follow the card and the fixtures only:
  - a checkbox extra is on for any value that isn't empty, so `0` is on;
  - a color that isn't exactly six hex digits falls back to the default;
  - a label number past `u32` reads as 0;
  - a trailing `;` in the labels line would add a label with an empty name;
  - section lines (`Labels:`, `Extra Tags:`, `Data:`) are matched whole, untrimmed;
  - `write` leaves out items with an empty path and writes a line break inside a name, tag or path as a space (the card doesn't say).

### A4 — The filter — done
- **Files:** `crates/shadui-index/src/query.rs` (`Has`, `MarkTerms`, `MarksFilter`, `Filters::marks`, the marks keys in `parse`, `without_marks`, `nearest`).
- **Tests:** `cargo test -p shadui-index query::` → 19 passed, 0 failed: the 14 there before, unchanged, and 5 new (the fixture with 60 cases or more, `parse`, `without_marks`, `nearest`, odd input).
- **Clippy:** clean.
- **Not done:** nothing. No case of the fixture looked wrong.
- **Needs:** the coordinator's own item: `Tree::search` asking `filters.marks`. Until then `Filters::tags` is filled by `tag:` without `!`.
- **Unsure:** in `nearest`, a name with nothing near it inside a group that is being replaced stays as typed; `!` before a marks key goes to `marks.no` whatever the key's case; `label:` splits on `;` only.

### B1 — The streams — done
- **Files:** `crates/glint-core/src/marks/streams.rs`, `crates/glint-core/src/marks/caps.rs`.
- **Tests:** `cargo test -p glint-core marks::` → 15 passed, 0 failed (14 and 1), the card's fifteen. They ran for real: the temp drive is NTFS.
- **Clippy:** clean. One `#[allow(clippy::permissions_set_readonly_false)]` in a test, on a temp file about to be removed.
- **Not done:** the three reads were not timed (the card asks for it only "if you timed them").
- **Needs:** `keep_times` folded into one function: `streams.rs` has a private copy marked `// TODO(coordinator): from tags.rs`. This is PLAN §9's item, left for after.
- **Unsure:** `write` also takes a label or comment of only white space as none and trims a comment's trailing white space, which the card doesn't ask, so that what is written reads back; the long-path test skips silently when the deep folders can't be made.

### C1 — The index — done
- **Files:** `crates/shadui-index/src/record.rs`, `tree.rs`, `volume.rs` (the three places, 15 lines), `refs.rs` (the one line), `engine.rs` (`save_marks`, `load_marks`, the old `.tags` reader kept private, the two calling lines).
- **Tests:** `cargo test -p shadui-index` → 122 passed, 0 failed, 7 ignored (ignored before too). The card's fourteen: 6 in `record.rs`, 5 in `tree.rs` (item 8 is one test), 3 in `engine.rs`.
- **Clippy:** stopped on A4's file when C1 ran it; clean in the workspace run.
- **Not done:** nothing. `Tree::search` is untouched.
- **Needs:** nothing beyond A4's filter going into `Tree::search`.
- **Asked by the card:** no other place read `Tree::tags` (`engine.rs`'s `tags()` goes through `tag_counts()`, the search through `tagged()`); nothing in `volume.rs` beyond the three places; a drive with nothing marked holds two empty maps.
- **Unsure:**
  - `far` is set for an attribute list that left the record even when its runs don't parse;
  - a `.marks` that is there but damaged gives no marks, and `.tags` is not tried then;
  - `save_marks` cuts a field at 64 KB so that `load_marks` takes it back;
  - test 4 makes the far attribute list through `RecordBuilder`'s private `attribute`, since the card adds no method for it.

## Left for the coordinator (PLAN §9, not done here on purpose)

- `Tree::search` asking A4's `filters.marks`.
- `keep_times` made one function (B1's copy in `streams.rs`, the original in `tags.rs`).

## What a card's wording got wrong, and what to look at

- **A2, the fixtures.** The card says to read them with `include_str!`. A2 reported reading them through `std::fs` in its tests instead; the tests pass, and no function outside `#[cfg(test)]` reads the disk.
- **A1, `settled`.** "When the label is one a label `was`" was read to the letter: a label written `red` is not made `Red`. If a file's label should settle to the name's spelling now, the card needs to say so.
- **C1, a damaged `.marks`.** The card says "load `.marks` when it is there; else `.tags`"; it doesn't say what a damaged `.marks` beside a good `.tags` gives. C1 gives no marks.
- **A3, the checkbox.** "Any value that isn't empty is on" makes `0` on. To be checked against a real `tag.dat`.
- **B1, tidying on write.** Not asked by the card; it overlaps A1's `tidy`, which the callers are meant to use.
- **Line ends.** Eight of the changed files were written with LF; Git warns it will make them CRLF the next time it touches them. Nothing to do unless the diff looks odd.
- **While they ran:** A4 and B1 each met a build failure in `engine.rs` (C1's half-written work), waited, and it cleared; A3 met one `LNK1104` at link time from a build beside it, and its next run passed. Nobody touched a file that wasn't theirs.

## Not run

- The end-to-end checks (`tests\e2e\run.ps1`): they take the real mouse and keyboard.
- `cargo fmt`: by the rules.
