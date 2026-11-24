# Card B4 — The folders inside a folder Windows copies or moves away; marks beside files moved into them

Wave 4 · crate `glint-core`

**You own:** `crates/glint-core/src/marks/beside.rs` (the two functions at its end, `waiting_in` and `move_in`, which do nothing; and its tests) and **one marked place** in `crates/glint-core/src/shell.rs`: `imp::file_op` (615 to 650), with tests beside the ones the file has for it or in a small module at the file's end. Nothing else: not `carry.rs`, not `queue.rs`, not `mod.rs`.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1, M3, M5), §4 (F18, F21, F23), §5.2, §5.4, §6.3, and under §9's table "Wave 3, as it landed" ("A folder's own marks are carried, always"; "Not carried"). Then the code: `shell.rs` `imp::file_op` (615: what it does today for the items themselves, and `own` for a moved folder's own marks); `marks/carry.rs` whole (`carry` 54, `taken` 69, `given` 74: read an item's marks, give an item marks); `queue.rs` `plan` (332: how the queue pairs every folder with the one it is a copy of) and 624; `marks/beside.rs` whole (`read` 14, `line` 31, `change` 36); `marks/streams.rs` (`read` 10, `write` 25); `marks/caps.rs` (`has_streams`); `marks/mod.rs` (`marks_beside` and `marks_move_in`, which call your two functions; `work` 153, to see why `marks_put` can't do this move: an item whose marks don't change is not written).
**Check with:** `cargo test -p glint-core marks::beside`, `cargo test -p glint-core shell::` (or your module's own filter), `cargo test -p glint-core marks::` once at the end, and `cargo clippy -p glint-core --all-targets -- -D warnings`.

## Part 1 (do this first; it is small): beside.rs, F23

- **`waiting_in(folder) -> u64`**: 0 unless the folder's drive keeps streams (`caps::has_streams`) and it has a `.glint-marks`. Else how many of its lines name an item that is there, hold marks, and whose own streams hold none (a stream wins over a line: such a line is not waiting, it is stale).
- **`move_in(folder) -> Result<u64, String>`**: for each waiting line, the marks are written to the item's streams (`streams::write`, which keeps the file's dates: M3), and **only the lines whose stream was written are taken out**, in one write of the sidecar (`change`), which removes the file when no line is left. An item that can't be written (read-only, in use) keeps its line and isn't counted. The answer is how many moved. A line later versions' keys ride on is lost with its line: say so in a comment.
- M10 holds as it does in `read`: a name that leaves the folder was refused when the file was read; nothing here joins a path from anything else.

## Part 2: shell.rs, the folders inside (B4)

Windows' engine copies a file's streams and never a folder's. `file_op` already carries the items it was given and a moved folder's own marks; **the folders inside a folder it copies or moves to another drive still lose theirs**.

- **A copy** (`Copy`, `CopyInto`): once done, for each pair `(from, to)` where `from` is a folder: walk `from`'s folders (never following a link or a junction), and each one that has marks of its own (`carry::taken`) gives them to the folder at the same place under `to`, when that one is there and has none.
- **A move** (`Move`, `MoveBack`): **only when the two ends are on different volumes** (on one volume a move is a rename and nothing is lost; M5: it must cost nothing there). The inner folders' marks are read **before** the engine runs (afterwards the source is gone) and given afterwards under where each source went. Whether two paths are on one volume: write one small function and say in its comment what it does about mount points and shares.
- **Bounded**: the walk stops at 50,000 folders a source and is not started for a source on another computer; say so in a comment (a `ponytail:` one, with the ceiling).
- What `file_op` does today for the items themselves stays exactly as it is.

Write the carrying as a function that takes pairs and the marks read before (so tests call it with folders of their own and never run Windows' engine), and keep `file_op`'s part to calling it.

## Tests

beside.rs (folders of the test's own; where the temp folder keeps no streams the tests that need them return early, as `mod.rs`'s do):
1. A folder with a `.glint-marks` of three lines, one for a file that is gone, one for a file whose stream already has marks: `waiting_in` is 1.
2. `move_in` writes that one's streams (its label, tags and comment read back with `streams::read`), leaves its dates and contents, takes its line out, and answers 1; the stale and the gone lines are as `change` leaves them.
3. When every line moved the `.glint-marks` is gone.
4. A read-only file keeps its line and isn't counted, and the others still move.
5. A folder with no `.glint-marks` is 0 and `move_in` writes nothing.

shell.rs:
6. A tree `a\b\c` with a label on `b` and a comment on `c`, copied by the test itself (plain `fs` calls) to another folder: the carrying gives `b` and `c` under the copy their marks; a folder under the copy that already has marks keeps its own.
7. A move: the marks read before, the source removed by the test, the carrying gives them under the destination.
8. Two paths on one volume are told so, and for them the move reads nothing (count the calls through a function the test passes, or assert the read list is empty).
9. A junction inside the source is not walked into (skip where one can't be made, and say so).

## Done when

The tests pass (and `cargo test -p glint-core marks::` whole), clippy is clean, `file_op` does for the items themselves what it did, and nothing outside your two places changed. **If part 2 doesn't fit your run, finish part 1, say "partial", and say exactly what of part 2 is there.**
