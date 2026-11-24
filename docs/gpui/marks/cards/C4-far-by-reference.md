# Card C4 — The far reads open an item by its file reference, never by its path

Wave 3 · crate `shadui-index` · I/O, in a service that runs as LocalSystem

**You own:** `crates/shadui-index/src/far.rs` (tests at its bottom), and in `crates/shadui-index/src/engine.rs` the constant `FAR_READS` (338) with its comment, `far_round` (501) and the thread that calls it. Nothing else of `engine.rs`. The words for `docs/instant-search.md` are the coordinator's to write: give them in your report.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M10), §5.1, §6.4, §10 ("The service's promises"), §14 items 5 and 6, and "Wave 2, as it landed" under §9's table. Then [card C3](C3-far.md) (what `drain` does stays). Then the code: `far.rs` whole (`read` 50, `drain_with` 73, the tests from 123); `tree.rs` `make_ref` (155) and `Rec::seq`; `record.rs`'s three readers (`label_text`, `tags_text`, `comment_text`, `COMMENT_KEPT`); `win.rs` (`Handle`, `wide`, `os_error`); and **`crates/glint-core/src/marks/streams.rs` `listing` and `walk` (145, 173)**: the listing of a file's streams you write again here, bounds checked, no pointer cast.
**Check with:** `cargo test -p shadui-index far::`, `cargo test -p shadui-index` once at the end, and `cargo clippy -p shadui-index --all-targets -- -D warnings`.

## Why

`far::read(path)` opens an item by its path, and the service is LocalSystem: an account that can write a folder can swap part of that path for a link while the service looks, and so send it to open somewhere else, another computer included. So the read **walks no path**.

## What to build

### `read_ref(root, reference) -> Option<Marked>` (in place of `read(path)`)

`root` is a handle of the drive's root (`X:\`), opened once a pass of `drain` (for reading attributes, with backup semantics and every share mode). `reference` is the record's number with its sequence, as `tree::make_ref(n, rec.seq())` puts them together.

1. **Open the item by its reference**: `OpenFileById` on `root` with a `FILE_ID_DESCRIPTOR` of type `FileIdType`, asking for `FILE_READ_ATTRIBUTES` alone, every share mode, `FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT` (a link is not followed). A reference that no longer opens (the file is gone, the record is another file's now: the sequence differs) is `None`, and the record is left as it is.
2. **List its streams on that handle**: `GetFileInformationByHandleEx(.., FileStreamInfo, ..)` as glint-core's `listing` does it (a buffer aligned to 8 bytes, four times bigger on `ERROR_MORE_DATA` up to 256 KB, `ERROR_HANDLE_EOF` is no streams), walked with every offset and length checked against the buffer. **No `glint.*` stream is `Some(Marked::default())`, and you are done**: that is nearly every far record, and nothing was held open that another program could trip over.
3. **Only an item that has one is opened again by its reference, for reading** (`GENERIC_READ` and no more, the same flags and shares), and its streams are walked with **`BackupRead`**: a `WIN32_STREAM_ID` header (its id, attributes, size, name size), then its name, then its data. Read the data of a stream named `:glint.label:$DATA`, `:glint.tags:$DATA` or `:glint.comment:$DATA` (any ASCII case) up to its cap (256 bytes, 4,096, `COMMENT_KEPT`) and `BackupSeek` over everything else, the file's own data first of all and what is left of a capped stream too. **The last call frees the context whatever happened** (`BackupRead` with `bAbort` true), on every way out. A file another program holds with no sharing can't be opened so: `None`, and its record is left.
4. The bytes go to `record`'s three readers; the answer is `Some(Marked)`.

Write the walk over the bytes of a header as a function of its own that takes a slice (`fn header(bytes: &[u8]) -> Option<(…)>`): a test hands it headers that lie about their sizes. Sizes are `u64`s from outside: no `as` cast that can truncate, no index that can go out of bounds, no loop a lying size can keep running (a stream's size past what `BackupSeek` moved ends the walk).

### `drain` and `drain_with`

As they are, with two changes: step 1 takes each far record's **reference** (`make_ref(n, rec.seq())`) in place of its path, and the reader is `&dyn Fn(u64) -> Option<Marked>`. `drain` opens the root's handle once (no handle: nothing is taken, the answer is 0) and hands `drain_with` a reader over `read_ref`. **`far::read(path)` goes**: nothing in the service's code may open an item by a path any more. `drain`'s signature (`tree, letter, stop`) stays: `engine.rs` calls it.

### engine.rs

Turn `FAR_READS` on, and rewrite its comment to say what is true now (the items are opened by their reference; why a path is never walked). The thread and `far_round` stay as they are unless they have to change for the above.

## Tests (each a `#[test]`, named as a sentence)

A folder of your own under `std::env::temp_dir()`, removed at the end; a test that writes a stream first tries one and returns when the drive has none. **The tests open their files by reference too**: a file's reference comes from `GetFileInformationByHandle` (`nFileIndexHigh`, `nFileIndexLow`), and the root's handle is of the drive the temp folder is on.

1. A file with the three streams reads back by its reference as its label, its tags and its comment; a **folder** with them too.
2. A file with no `glint.*` stream is `Some(Marked::default())`, and is not opened for reading. Prove it: while another handle holds the file with no sharing at all (`share_mode(0)`), the read still answers `Some(Marked::default())` (asking for attributes alone needs no sharing; a read would be refused).
3. **A stale reference**: delete the file and ask again by the reference it had: `None`. A reference made up of rubbish (`0`, `u64::MAX`): `None`.
4. **Eighty other streams** of 60-character names beside the three (more than 4 KB of listing) still read the three.
5. **A marked file held open with no sharing** by another handle: `None`, no panic, and after the handle is dropped it reads.
6. The caps: a comment of 5,000 bytes of `é` comes back at most `COMMENT_KEPT` bytes on a whole character; a file with 3 MB of its own data before its streams reads its marks (the data is sought over, not read: say how long the read took).
7. A stream named `Glint.Label` is the label; `glint.other` and `Zone.Identifier` are not marks.
8. **Headers that lie**, given straight to the header walker and to the loop that follows it over a byte slice or a fake reader: a name size past the buffer, a stream size of `u64::MAX`, a header cut short, a name that isn't UTF-16: no panic, and what was read before the lie stands.
9. A link is not followed: when a symbolic link or a junction can be made (try `std::os::windows::fs::symlink_file`, then `mklink /J` for a folder through `cmd`), its own reference reads as no marks though its target is marked. **Say in your report whether this test really ran.**
10. `drain_with`'s tests (C3's 7 to 11) hold with references in place of paths: the reader is asked for `make_ref(n, seq)`.
11. No `read(path)` is left: `far.rs` has no function that takes a path of an item.

And every test the crate has passes, `engine.rs`'s far test included.

## Done when

The tests pass, clippy is clean, every `unsafe` block has its `// SAFETY:` line, the `BackupRead` context is freed on every way out, and `FAR_READS` is on. Say in your report: **what one read of an item with no marks costs** (time 10,000 of them by reference, files in Windows' cache), whether test 9 ran, and the sentences `docs/instant-search.md`'s security model should gain (the service holds every account's labels, tags and the first 4 KB of their comments; whoever may ask it can learn which files match a word; it never sends a comment's text; it opens marked files by their reference to read those three streams, never a file's own data and never by a path), in the words that are true of what you built.
