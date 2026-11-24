# Card F1b — Import: applying, the journal, taking it back, twice is nothing, following, waiting marks wake

Wave 4 · crate `glint-core`

**You own:** `crates/glint-core/src/marks/apply.rs`, `crates/glint-core/src/marks/wake.rs` (both new, with bodies that do nothing; their module lines and re-exports are there) and `crates/glint-core/src/marks/ledger.rs` (additions only: every function it has keeps its signature and what it answers; `mod.rs` and `carry.rs` call them). Tests at the bottom of each.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M3, M5, M6, M7, M10), §5.3, §5.4, §7.2 (F29 to F31), §7.3 ("Meeting Glint's own marks", "Applying", "Following"), §8.1. Then the code: `marks/import.rs` whole (the types; `import_apply` calls your `apply::apply` on a blocking thread; `import_plan` is card F1a's and **answers an error until that card is in, at the same time as you work**); `marks/mod.rs` (`World` 50, `real` 71, `find` 101, `work` 153: the worker of `marks_put`, which a child module may call, as `carry.rs` does; `marks_put` 270; the tests' `world`, `ledger_in` and `folder` from 344); `marks/ledger.rs` whole; glint-model's `marks::meet` (`mod.rs:180`), `lines.rs` (`Line`, `Why`, `write_ledger`, `read_ledger`); `tools.rs:894` (`crate::tools::data_dir()`: Glint's data folder); `aware/pool.rs:41` (how a thread is put at background priority); `fsx.rs` `read_listing` (the one call of `wake::listed`, on the thread that lists).
**Check with:** `cargo test -p glint-core marks::apply`, `cargo test -p glint-core marks::wake`, `cargo test -p glint-core marks::ledger`, once at the end `cargo test -p glint-core marks::`, and `cargo clippy -p glint-core --all-targets -- -D warnings`.

**No test of yours may need `import_plan`**: they build an `ImportPlan` by hand. Everything that touches Glint's data folder takes the folder as an argument in a private function, so tests use their own; the public functions pass `data_dir()`.

## `apply(plan, progress)`

1. **In batches of 200**, on a thread at background priority (as `aware/pool.rs` sets it; put it back when done), `progress.send(Progress { done, total })` after each batch (a failed send is ignored).
2. **For each item**: `Here` is marked at `path`, `Moved { now }` at `now`. What it gets is **`meet(what it has now, &gets, plan.options.meet)`**, with what it has read then (not the plan's `has`, which may be old), written through the homes with the worker `marks_put` uses: one write a folder for marks kept beside the files, never one a file. A `true` from `meet` counts in `kept_comments`. `Away` and `Missing` go to the ledger with **`Why::Away`** (the worker would say `Shut`: write the ledger yourself, once a batch) and count in `waiting` and `missing`.
3. **Twice is nothing.** `xyplorer.json` in the data folder remembers, a source (its path in lower case): its path, modified time, size, the options applied, and **a hash an item** (blake3, in the build) of the marks it was given. An item whose hash is what is remembered and whose marks now are still those is not written again. Written once at the end, to a temporary file that takes its place (`crate::replace_contents`, `lib.rs:134`). A damaged `xyplorer.json` reads as empty.
4. **The journal**: `imports\<Unix ms>.jsonl` in the data folder: a first line `{"glint-import":1,"source":…,"at":…}`, then one line an item that changed, its path and its marks **before**, in the ledger's line format (`write_ledger`'s lines). Written as the batches go, so an import that is stopped by a crash can still be taken back. Journals older than 30 days are removed when an import starts. `report.journal` is its path; `report.befores` the same items.
5. **The report**: `marked` (items whose marks changed), `waiting`, `missing`, `kept_comments`, `befores`, and `items`: every item of the plan with what happened, worded: `Marked`, `Marked where it is now: {now}`, `Already had these marks`, `Kept its own comment`, `Waiting for its drive`, `Not found: its marks wait`, `Couldn't mark: {why}`.

## Taking it back

`last_import() -> Option<LastImport>`: the newest journal there is. `import_take_back(journal)`: gives every item of the journal its marks before (through `marks_put`'s worker; an item that waited is taken out of the ledger), forgets those items in `xyplorer.json`, removes the journal, and answers the outcomes. A journal that is damaged gives back what it can read; one outside the data folder's `imports` is refused (M10). `brought_in(source)`: whether `xyplorer.json` remembers that source.

## Following (F30)

`follow()`: for the source `xyplorer.json` remembers that is an XYplorer file (the newest, if several): **one `stat`**; when its modified time and size are what is remembered, `Ok(None)`. Else `import_plan(source, the remembered options)`, then, in a function your tests call with a plan made by hand: items new or changed since (their hash differs) are applied as above; **an item remembered whose line is gone has its marks taken off only where they are still what XYplorer had** (its hash). `Followed { source, came, gone, paths }`. Following writes no journal's Undo step, but it does write a journal, so *Take back the last import* works on it. Whether to follow at all is the interface's setting: `follow()` does what it is asked.

## Waiting marks wake (F31): wake.rs and ledger.rs

`wake::listed(folder)` is called on the thread that lists a folder, for every real folder listed. **While no mark waits it is one atomic read** (M5: a listing costs what it costs today). ledger.rs gains what that needs: a count of `Why::Away` lines kept right by `open`, `put`, `take` and `save`, and `away_in(folder)` (the waiting lines whose item is right in that folder, any case). When some wait in this folder, a thread of its own (never the listing's) gives each one that is there now its marks through the worker, which takes the line out of the ledger; `meet` with `Fill`, in case the item was marked since. One folder is woken by one thread at a time.

## Tests

apply.rs (folders of the test's own; a `World` with a ledger of its own, as `mod.rs`'s tests make):
1. A plan of three items `Here` marks them, answers `marked: 3`, their befores, a journal with three lines, and sends progress that ends at `3 of 3`.
2. *Fill what is empty* keeps a different comment and counts it; *Replace* replaces all three marks.
3. `Moved` marks the file where it is now; `Away` and `Missing` are in the ledger with `"why":"away"` and counted.
4. **The same plan applied twice changes nothing the second time**: `marked: 0`, no file's modified time moves, no new journal line.
5. 450 items in one folder with no streams write its `.glint-marks` three times (three batches), not 450.
6. `import_take_back` puts every item back as it was (a label it had, none where it had none), takes the waiting ones out of the ledger and removes the journal; `last_import` is then `None`. A journal outside the folder is refused; a journal cut mid-line gives back the lines before the cut.
7. Marking leaves a file's dates and contents (M3).
8. Following: a plan with one new item, one changed and one gone, against what is remembered: the new and the changed come in, the gone one loses its marks, **and a gone one whose marks the person changed since keeps them**. A source that did not change is `None` with one `stat` and nothing read.
9. `#[ignore]`: 10,000 items over 10,000 small files in 100 folders: print the milliseconds of applying (the report wants it), with streams and without.

wake.rs and ledger.rs:
10. With no waiting line `listed` returns at once (the count is 0) and starts nothing.
11. A waiting line whose file now exists: after `listed(its folder)` and a short wait, the file has the marks and the ledger no longer has the line; a line of another folder stays; a line whose file is still not there stays.
12. The count is right after `put`, `take`, `open` of a file with waiting lines, and a line put as `Shut` doesn't count.

## Done when

The tests pass (and `cargo test -p glint-core marks::` whole), clippy is clean, `ledger.rs`'s old functions are as they were, and nothing here reads or writes XYplorer's file but through `import_plan`. Say in your report the layout of `xyplorer.json` and of a journal, test 9's times, and what the interface must call (`follow`, `last_import`, `import_take_back`, `brought_in`).
