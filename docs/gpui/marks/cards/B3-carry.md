# Card B3 — Marks follow what Glint copies and moves; fd's drives

Wave 3 · crate `glint-core` · I/O, on worker threads

**You own:** `crates/glint-core/src/marks/carry.rs` (tests at its bottom); `crates/glint-core/src/find.rs`; and **marked places only** in `queue.rs` (right after each file is in place: the move at 748, the two copies at 776 and 832, or their callers where a step is counted done), `actions.rs` (`run`, 145: around the recycling at 188) and `convert.rs` (`convert`, 218: around 266). `marks/mod.rs`, `beside.rs` and `ledger.rs` are not yours: call them. If you need one more `pub(crate)` function there, write it in `carry.rs` on top of what they export and say so.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1, M3, M5, M10), F18 to F22 in §4, §5.2 to §5.4, §6.3, §6.5, and "Wave 2, as it landed" under §9's table. Then [card B2](B2-homes.md). Then the code: `marks/mod.rs` whole (`home_of` 141, `read` 275, `matches` 280, the worker `marks_put` stands on, and its tests' way of saying a drive has no streams), `beside.rs`, `ledger.rs`, `caps.rs`; `find.rs` whole (`Out` 202 and its `push` 227, the four threads that read media filters from 423, the tags handed to fd's results at 493); `shadui-index/src/query.rs` `Filters::marks`, `MarksFilter` and `without_marks`; `glint-model/src/marks/wanted.rs` (`Wanted::parse`).
**Check with:** `cargo test -p glint-core marks::`, `cargo test -p glint-core find::`, `cargo test -p glint-core queue::` and `cargo clippy -p glint-core --all-targets -- -D warnings`.

## carry.rs

### `carry(pairs, moved)`

After Glint copied (`moved` false) or moved, renamed, recycled or restored (`moved` true) each `(source, destination)` pair. Where both ends keep streams there is **nothing to do** (Windows took them along) and nothing may be read: decide that from `caps::has_streams` of the two paths before anything else, so a copy between two NTFS folders costs two answers from memory a pair and no open.

Otherwise, for each pair whose destination is there:

1. The marks the source had: for a copy, `marks::read(source)`; for a move the source is gone, so they are its line in the source folder's `.glint-marks` or in the ledger (ask those two by the source's path; its streams went with it or were dropped by the move).
2. Marks that aren't empty are written at the destination through the homes (what `marks_put` does for one item, without the async: call the same worker), so they land in the first home that takes them. A destination that already has marks in its streams keeps them (Windows carried them).
3. For a move, the source's line is taken out of its folder's sidecar (one `beside::change` a folder, not one an item) and out of the ledger.

It never fails and never panics: a pair that can't be read or written is left. Pairs are grouped by folder so that a sidecar is read and written once a folder a call.

### `pub fn carry_one(source, destination, moved)` and `pub fn taken(original) -> Marks` / `pub fn given(made, marks)`

Small faces for the callers below (name them as you like; say the names in your report): one pair at a time for the queue, and "read before it is recycled, write to what takes its place" for the tools.

## The marked places

- **`queue.rs`**: after each file is in place (a copy, a move across drives, a rename on one drive), its pair is carried. The cost when nothing is marked and both ends have streams must be nothing a person could measure: say what you measured (a queue test that copies 2,000 small files, timed with the call and with it commented out, or the two `has_streams` answers timed alone).
- **`actions.rs` `run` with `Output::Replace`** and **`convert.rs` `convert`**: each original's marks are read **before** the originals are recycled, and written to the file that takes its place once it is there (G6). An original with no marks costs one `marks::read`.

## find.rs: fd's drives

Today `Out::push` asks `crate::tags::matches` of each path fd found, on the one thread that reads fd.

- Read `Filters::marks` where `Filters::tags` is read, and take the marks terms out of fd's query with `without_marks` where `without_tags` is used (493). The filter asked of each item is glint-model's `Wanted`, parsed from the same query's terms (`Wanted::parse(&glint_model::entries::terms(&query.to_lowercase()))`): the two copies agree by the fixture.
- **The test moves to the four threads that read the media filters** (423): a search with a marks filter and no property filter uses them too.
- **One `.glint-marks` a folder for the length of a search**: a cache by folder (lower case) shared by the four threads, so `marks::matches`'s read of the sidecar is not repeated for every item of a folder. Write the read here on top of `streams::read`, `beside::read`, `beside::line` and the ledger, in the order `marks::read` has them.
- **The ledger's matches under the folders searched are added** to what fd found (items fd can't see: inside an archive, on a remote), each once, when they pass the filter and the query's other terms can't refuse them (a query of marks terms alone; with name terms, the name must hold them).
- `Filters::tags` is no longer read here. What the index answers is not yours to change.

## Tests (each a `#[test]`, named as a sentence)

A folder of your own under `std::env::temp_dir()` and a ledger of your own, as `marks/mod.rs`'s tests do; no test may touch the shared ledger's file. Tests that write streams skip when the drive has none.

1. Between two folders that both keep streams `carry` reads and writes nothing: a sidecar planted in the source folder is byte for byte what it was, and no sidecar appears at the destination.
2. A copy from a folder whose marks are beside the files (streams said absent for it) to one with streams: the copy has the marks in its streams, the source's line is still there.
3. A move the same way: the destination has them, the source's line is gone, and a sidecar left with no lines is removed.
4. A move from streams to a folder without: the line is beside the destination.
5. A rename inside a folder without streams (`a.txt` to `b.txt`, `moved` true) renames the line; the other lines and unknown keys stay.
6. An item whose marks are in the ledger, moved: the ledger has them under the new path and not the old.
7. Twenty pairs of one folder cost one write of its sidecar (count the writes by the file's modified time or by a counter of your own).
8. Pairs that can't be carried (a source that never existed, a destination that isn't there, an empty list, a path of rubbish) change nothing and don't panic.
9. A file made in another's place gets its marks: the faces the tools call, on a file with all three marks, with the original gone in between.
10. `find.rs`: a query's marks terms are taken out of what fd is asked (`report label:red !tag:old` asks fd for `report`), and the filter kept is the `Wanted` of those terms.
11. The folder cache: 200 items of one folder asked about read its `.glint-marks` once.
12. The ledger's lines under a searched folder that pass the filter are added once each, and one outside the folders isn't.
13. The tests `find.rs` and `queue.rs` have pass unchanged.

## Done when

The tests pass, clippy is clean, a copy between two drives with streams reads no mark, and nothing a path or a sidecar holds can panic. Say in your report: the names of the faces you added, what a copy costs when nothing is marked (measured), and **what the interface must call** (glint-ui's `explorer/ops.rs` `after_file_op` does not call `carry` yet: say exactly which call with which pairs for a copy, a move, a rename, a delete to the Recycle Bin and a restore).
