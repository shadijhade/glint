# Card B1 — Streams: a file's own marks, and what a volume can do

Wave 1 · crate `glint-core` · I/O, called on worker threads

**You own:** `crates/glint-core/src/marks/streams.rs` and `crates/glint-core/src/marks/caps.rs` (tests at the bottom of each).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M3, M8), §5.1, §5.4, §14 items 4 and 5 (what was measured, and why the read is `FileStreamInfo`). Then `crates/glint-core/src/tags.rs`, all of it (153 lines: the stream's name, `read`, `write`, `keep_times` and its test are what you extend to three streams), `crates/glint-model/src/marks/mod.rs` (the stream names and the caps), and `crates/glint-core/src/wof.rs` line 570 on (`GetVolumePathNameW` as it is called here).
**Check with:** `cargo test -p glint-core marks::` and `cargo clippy -p glint-core --all-targets -- -D warnings`.

## streams.rs

Marks in the file itself: `glint.tags` (one tag a line, 0.16's), `glint.label` (one line, the label's name) and `glint.comment` (the comment). The path of a stream is the item's path without a trailing `\` or `/`, a colon and the stream's name, as `tags::stream_of` makes it. A drive's root (`C:\`, `\\server\share\`) is not marked: `read` gives `None` for one and `write` gives `Err("A drive itself can't be marked")`.

### `read(path) -> Option<Marks>`

One listing, then one open for each stream of ours it shows, so a file with no marks costs one open and one query (15.5 µs measured; a failed open of each of the three would be 37):

1. Open the item for its attributes only, so that folders open too and nothing is locked: `std::fs::OpenOptions` with `access_mode(0x80)` (`FILE_READ_ATTRIBUTES`) and `custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)`, as `keep_times` opens it. No open: `None`.
2. `GetFileInformationByHandleEx(handle, FileStreamInfo, …)` into a buffer of 4 KB aligned to 8 bytes (a `Vec<u64>`); on `ERROR_MORE_DATA` four times bigger, up to 256 KB, then give up (`None`). An item with no streams at all answers `ERROR_HANDLE_EOF`: `None`.
3. Walk the entries (`FILE_STREAM_INFO`: the next entry's offset, the name's length in bytes, two sizes, then the name in UTF-16). Read the fields from the bytes with bounds checked (`get(..)`, `from_le_bytes`): an offset or a length that points outside the buffer ends the walk; no pointer is cast. A stream's name is `:glint.tags:$DATA`; compare without regard to ASCII case.
4. For each of the three found, read it with `std::fs`:
   - tags: `crate::tags::read(path)` (it is the 0.16 reader);
   - label: at most `LABEL_CAP` bytes, as text (`from_utf8_lossy`); the first line with anything in it, trimmed;
   - comment: at most `COMMENT_CAP` bytes, as text; when the stream was longer and the cut fell inside a character, the broken end is dropped; `\r\n` becomes `\n`; trailing white space trimmed.
5. `Some(marks)` when any of the three came out with something, else `None`.

### `write(path, marks) -> Result<(), String>`

1. Refuse what a file can't hold, before anything is opened: a label over `LABEL_CAP` bytes (`"A label's name can be 256 bytes at most"`), tags whose text (below) is over `TAGS_CAP` (`"These are more tags than a file can hold (4 KB)"`), a comment over `COMMENT_CAP` (`"A comment can be 64 KB at most"`).
2. Read what the item has now (`read(path).unwrap_or_default()`). **Only a stream whose mark differs is touched**; when nothing differs, return `Ok(())` without opening anything for writing.
3. Keep the item's dates as `tags::write` does. `tags::keep_times` is private: copy it into this file with the line `// TODO(coordinator): from tags.rs`. Take the dates once before the first stream is written and put them back once after the last.
4. For each stream to change: a mark that is none (or no tags) removes the stream (`remove_file`; not there is fine); otherwise the stream is created anew and written:

   | Stream | Its text |
   | :--- | :--- |
   | `glint.tags` | the tags, one a line, a `\n` after each (as 0.16 writes it) |
   | `glint.label` | the name, then `\n` |
   | `glint.comment` | the comment, then `\n` |

   The marks given are tidy (the caller ran `marks::tidy`); all the same, write a label's line breaks as spaces and leave out an empty tag, so that what is written always reads back.
5. The first error is the answer (`e.to_string()`), after the dates are back; what was written before it stays written.

Without Windows (`#[cfg(not(windows))]`): `read` is `None`, `write` is `Err("Marks in the file need Windows")`.

## caps.rs

`has_streams(path) -> bool`: `GetVolumePathNameW` for the root of the volume `path` is on (a drive or a share), then `GetVolumeInformationW` for its flags: true when `FILE_NAMED_STREAMS` (`0x0004_0000`) is among them. An empty or relative path, or either call failing, is `false`.

Answers are remembered by the volume's root in lower case (a `static` `Mutex<HashMap<…>>`): for good on a fixed drive (`GetDriveTypeW` says 3), for ten seconds on any other (a stick may be swapped, a share may come back). A failure is never remembered. Without Windows: `false`.

This replaces guessing from an error's text (`tags.rs:102`, which stays as it is: card B2 calls you instead).

## Tests

Use a folder of your own under `std::env::temp_dir()` (unique with the process id and a counter; removed at the end). Every test that writes a stream starts with `if !caps::has_streams(&dir) { return; }`, so a temp folder on a drive without streams skips rather than fails.

1. All three marks written and read back are the same; the file's modified date is what it was (sleep 20 ms before writing, as `tags.rs`'s test does); its contents are untouched; and the three streams hold exactly `"Work\nRed\n"`, `"Important\n"` and `"mode 4\nLACP rate fast\n"` (read them with `std::fs::read_to_string`).
2. A file with no marks reads as `None`, and so does a path that isn't there.
3. Writing `Marks::default()` over a marked file takes all three streams away (each open fails with not found) and `read` is `None`.
4. Only what differs is written: with the file made read-only after a first write, writing the same marks again is `Ok`, and writing a different label is `Err`. (Take the read-only attribute off before removing the folder.)
5. Changing the label leaves the comment's stream byte for byte as it was.
6. A folder takes marks too.
7. What a script wrote reads tidy: `glint.comment` holding `"one\r\ntwo"` reads as `one\ntwo`; `glint.label` holding `"  Pending  \r\nmore\r\n"` reads as `Pending`.
8. Tags are 0.16's both ways: after `write` with the tags `a`, `b`, `crate::tags::read(path)` gives them; a `glint.tags` written as `"x\ny\n"` reads through `read` as `x`, `y`.
9. Too much is refused with words: a comment of `COMMENT_CAP + 1` bytes is `Err` holding `64 KB`, and the file has no stream afterwards.
10. A comment of 70,000 bytes written by something else, with a two-byte character across the cap, reads as text of at most `COMMENT_CAP` bytes that ends on a whole character.
11. A stream named in another case (`Glint.Label`) is read as the label.
12. An Arabic file name works, and so does a path of more than 260 characters (skip with `return` when the folders can't be made).
13. An item with 80 other streams of 60-character names (more than 4 KB of listing) still reads its marks.
14. A drive itself is not marked: `read("C:\\")` is `None` and `write` to it is `Err`.
15. `caps`: for the temp folder, `has_streams` says what trying says (writing `probe.txt:glint.probe` there works or not); asked twice it says the same; an empty path and a relative one are `false`; a drive letter nothing is on is `false` (find one from `Z` down for which `Path::new("Z:\\").exists()` is false; skip when all are taken). Don't ask a server that isn't there: that waits for the network.

## Done when

The tests pass, clippy is clean, every `unsafe` block has its `// SAFETY:` line, and reading an item with no marks makes no call beyond the one open and the one query. Say in your report what the three reads cost on your run if you timed them, and that `keep_times` is a copy for the coordinator to fold.
