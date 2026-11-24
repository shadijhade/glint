# Card C3 — Streams that left the record, read through the file system; the app's two calls

Wave 2 · crate `shadui-index` · I/O (the service opens a file for the first time here)

**You own:** `crates/shadui-index/src/far.rs` and `crates/shadui-index/src/client.rs` (tests at the bottom of each). `engine.rs`, `proto.rs` and `tree.rs` are card C2's: the engine starts the thread that calls your `drain`, at the same time as you work.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M10), §5.1, §6.4, §8.1, §10 ("The service's promises"), §14 item 6 (what was seen of records that fill up, and why the read is through the file system), and "Wave 1, as it landed" under §9's table. Then [card C1](C1-index.md) (`Marked`, `Tree::far`, `set_record` and `set_marks`). Then the code: `far.rs` whole (the two signatures you fill); `record.rs` `COMMENT_KEPT` (182), `label_text`, `comment_text` and `tags_text` (206 to 229: the three readers you hand bytes to); `tree.rs` `Tree` and `Marked` (199 to 232), `rec`, `links_of` (377), `set_record` (537), `marked` and `set_marks` (642, 648), `path` (810), `Builder` (1549); `client.rs` `connect_to` (78), `history_on` (268), `tags` (284) and the two stubs after it (300); `pipe.rs` line 351 (what the service answers), and its tests' `sample` and `answers_over_a_real_pipe` (417, 528: how a tree is built and a pipe is served in a test).
**Check with:** `cargo test -p shadui-index far::`, `cargo test -p shadui-index client::` and `cargo clippy -p shadui-index --all-targets -- -D warnings`.

## far.rs

### `read(path) -> Option<Marked>`

The marks of the item at `path` (a full path as the index makes it, `C:\dir\name`), **through the file system and never from the volume's clusters**.

1. A trailing `\` or `/` is taken off; an empty path or a drive's root (`C:`, `C:\`) is `None`.
2. Open the item itself for its attributes only (`access_mode(0x80)`, `FILE_READ_ATTRIBUTES`), with `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT` and every share mode: folders open too, nothing is locked, and **a link is not followed** (the record's own streams are wanted, and the service must not be led to another file). No open: `None` (the item is gone, or kept from the service): the caller leaves its record as it is.
3. For each of `glint.label`, `glint.tags` and `glint.comment`: open `{path}:{stream}` for reading with the same two flags and every share mode. Not there: that field is empty. Read **at most** 256 bytes of a label, 4,096 of the tags, `record::COMMENT_KEPT` of a comment, and hand them to `record::label_text`, `tags_text` and `comment_text`: the fields are then exactly what the record's own parse would have given.
4. `Some(Marked)`, each field empty when it has none (so an item whose streams are gone answers `Some(Marked::default())`, which takes its marks away).

Only those three streams are ever opened, never the item's own data, and nothing a stream holds can panic (the three readers take any bytes).

### `drain(tree, letter, stop) -> usize`

One pass over the far records of drive `letter`. `drain` is `drain_with` given `read`; write the private

```rust
fn drain_with(tree: &RwLock<Tree>, letter: char, stop: &(dyn Fn() -> bool + Sync), read: &dyn Fn(&str) -> Option<Marked>) -> usize;
```

so that tests hand in a reader of their own.

1. **Take**, under the write lock: up to `BATCH` numbers out of `tree.far` (removed from it), each with its path: the record's first name (`links_of(n).next()`, `Tree::path`) after `{letter}:\`. A record no longer in use, or with no name, is dropped from `far` and not read.
2. **Read**, with **no lock held** (a search never waits for the disk): `read(path)` for each, asking `stop` before each one.
3. **Give**, under the write lock again, for each that was read: skip a record no longer in use; note whether `n` is in `tree.far` **now** (the journal brought it again while you read); `set_marks(n, marks)`; and when it was, put `n` back in `far`, so that it is read once more. A read that answered `None` gives nothing and stays out of `far` (the journal brings the record back when it changes).
4. When `stop` ended the reads early, the numbers taken and not read go back into `far`.
5. The answer is how many records were taken in step 1 (0: none is far, and the caller sleeps).

A poisoned lock is taken all the same (`unwrap_or_else(|e| e.into_inner())`): no `unwrap` on what another thread did.

## client.rs: `marks` and `marks_in`

Fill the two stubs in the shape of `tags` and `history`, each with its `pub(crate)` twin that takes the pipe (`marks_on(pipe)`, `marks_in_on(pipe, path)`):

- `marks()` asks `Request::Marks` and answers `Message::Marks(counts)`;
- `marks_in(path)` asks `Request::MarksIn { path }` and answers `Message::MarksIn { items }`;
- any other message is `unexpected()`, a closed pipe the `UnexpectedEof` the others give, and the service's own error comes through `message` as `Error::Service`.

## Tests (each a `#[test]`, named as a sentence)

far.rs, `read`: a folder of your own under `std::env::temp_dir()` (unique with the process id and a counter, removed at the end). Each test that writes a stream first tries one (`std::fs::write(dir.join("probe.txt:glint.probe"), b"x")`) and returns when the drive has no streams.

1. A file with the three streams reads as its label, its tags one a line, and its comment; a folder with them does too.
2. A file with none is `Some(Marked::default())`; a path that isn't there, an empty path and `C:\` are `None`.
3. A comment of 5,000 bytes of `é` comes out at most `COMMENT_KEPT` bytes, ending on a whole character; `"a\r\nb\r\n"` is `a\nb`; a label stream `"  Important \r\nsecond\n"` is `Important`.
4. A stream made as `Glint.Label` is the label.
5. Streams holding bytes that aren't UTF-8, and a tags stream of 100 KB, don't panic and are capped.
6. Other streams and the file's own contents are not marks: a file holding `glint.label` **as its contents**, with a `Zone.Identifier` and a `glint.other` stream, reads as no marks.

far.rs, `drain_with` (trees built with `Tree::set_record` or `Builder`, as `pipe.rs`'s `sample` does):
7. Two far records get what the reader says, `far` is empty afterwards, the answer is 2, and the reader was asked for `X:\dir\name` paths (the drive's letter, the folders, the name).
8. No lock is held while reading: a reader that itself takes the tree's write lock (and marks the record far again with `set_record`) doesn't deadlock; afterwards the record has the marks that were read **and is still far**.
9. A reader that answers `None` leaves the record's marks as they were and the record not far; a record removed while it was read gets nothing.
10. `stop` set after the first read: the rest are back in `far`.
11. With `BATCH + 10` far records one pass takes `BATCH` and the next takes 10; with none it takes 0 and the reader is never asked.

client.rs (a pipe of its own name, served by `pipe::serve_on` over `Engine::with_tree`, as `answers_over_a_real_pipe` does it):
12. `marks_on` answers `Ok` (the counts are the engine's: empty until card C2 is in, so assert only that they decode).
13. `marks_in_on` for a folder of the tree answers `Ok` or the service's own error (`Error::Service`), never `unexpected`; and against a server that isn't there `marks()`'s twin on a pipe name nobody serves is the error `connect_to` gives.

And the crate's other tests pass unchanged.

## Done when

The tests pass, clippy is clean, every `unsafe` block (if you need one) has its `// SAFETY:` line, `read` opens nothing but the item for its attributes and those three streams, and no lock is held across a read. Say in your report what one `read` of an unmarked file costs if you timed it, and whether `FILE_FLAG_OPEN_REPARSE_POINT` kept a link from being followed when you tried one (a junction or a symlink needs no administrator to try: skip it and say so if it can't be made).
