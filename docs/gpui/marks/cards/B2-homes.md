# Card B2 — The three homes: `marks_read`, `marks_set`, `marks_put`, `read`, `matches`

Wave 2 · crate `glint-core` · I/O, called on worker threads

**You own:** `crates/glint-core/src/marks/mod.rs`, `crates/glint-core/src/marks/beside.rs`, `crates/glint-core/src/marks/ledger.rs` (tests at the bottom of each), and in `crates/glint-core/src/tags.rs` the two functions `tags_read` (86) and `tags_set` (92) and nothing else of that file. In `mod.rs` the `pub mod` and `pub use` lines of `carry`, `export`, `import` and `xy_find` are other cards': leave them.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M3, M8, M10), F18 to F20 and F23 in §4, §5.1 to §5.4, §6.1, §6.2, §8, §8.1, and "Wave 1, as it landed" under §9's table. Then cards [A1](A1-marks.md) (`tidy`, `Change::apply`), [A2](A2-lines.md) (the lines) and [B1](B1-streams.md) (the streams), which you call. Then the code: `marks/mod.rs`, `beside.rs` and `ledger.rs` as they are; `marks/streams.rs` lines 1 to 95 (`read`, `write` and their words) and `marks/caps.rs`; `glint-model/src/marks/mod.rs` (`Marks`, `Change`, `tidy`, `Home`, `Outcome`, the caps) and `lines.rs` (`Line`, `Why`, `Beside`, the six functions and their caps); `tags.rs` whole; `lib.rs` `replace_contents` (134); `actions.rs` `hide` (312); `tools.rs` `data_dir` (894); `fsx.rs` `blocking` (24).
**Check with:** `cargo test -p glint-core marks::`, `cargo test -p glint-core tags::` and `cargo clippy -p glint-core --all-targets -- -D warnings`.

**Landed for you:** `shadui_index::client::marks()` exists and answers `Err` until card C3 fills it at the same time as you work: `marks_all` calls it and takes an `Err` as no counts.

## What to build

An item's marks are in one of three homes (PLAN §5.4), asked in this order:

1. **The stream**: the item's own `glint.*` streams (`streams::read`, `streams::write`).
2. **Beside**: its line in `.glint-marks` in its folder (`beside.rs`, the text is `lines::read_beside` and `write_beside`).
3. **The ledger**: its line in `marks.jsonl` in Glint's data folder (`ledger.rs`, `lines::read_ledger` and `write_ledger`).

**Reading takes the first home that has anything for the item. Writing to a home takes the item out of the homes after it**, so a mark is in one place. What is kept is always `Change::apply`'s or `tidy`'s; what is read is returned through `tidy` too.

### ledger.rs

```rust
pub struct Ledger { /* where it is kept (None: in memory only), the lines by path in lower case, whether it changed */ }
impl Ledger {
    /// Reads the file (at most `LEDGER_MAX_BYTES`, `read_ledger`). No file, or one that doesn't read: empty.
    pub fn open(path: Option<PathBuf>) -> Ledger;
    pub fn get(&self, path: &str) -> Option<&Line>;
    /// Gives `path` these marks (`at` in Unix seconds); empty marks take its line away.
    pub fn put(&mut self, path: &str, marks: &Marks, why: Why, at: i64);
    pub fn take(&mut self, path: &str) -> Option<Line>;
    pub fn lines(&self) -> impl Iterator<Item = &Line>;
    pub fn is_empty(&self) -> bool;
    /// Writes it if it changed (`write_ledger`, `crate::replace_contents`); with no lines the file is removed.
    pub fn save(&mut self) -> Result<(), String>;
}
/// The one ledger of this PC: `marks.jsonl` in `crate::tools::data_dir()`, read the first time it is asked.
pub fn shared() -> &'static std::sync::Mutex<Ledger>;
```

A path's key is the path without a trailing `\` or `/`, in `to_lowercase()`; the line keeps the path as given. A poisoned lock is taken all the same (`unwrap_or_else(|e| e.into_inner())`): nothing here panics. It is saved **once at the end of each call that changed it** (`marks_set`, `marks_put`), which is the plan's "a moment after it changes".

### beside.rs

```rust
/// The folder's `.glint-marks` as it reads; None when there is none (or it can't be opened).
pub fn read(folder: &Path) -> Option<Beside>;
/// The line for the item named `name` (names alike in `to_lowercase()` are one).
pub fn line<'a>(beside: &'a Beside, name: &str) -> Option<&'a Line>;
/// Gives each named item of `folder` these marks, in one read and one write: empty marks take the line away.
pub fn change(folder: &Path, changes: &[(String, Marks)], at: i64) -> Result<(), String>;
```

- `read` takes at most `BESIDE_MAX_BYTES` of the file (`from_utf8_lossy`) and hands them to `read_beside`.
- `change`: the lines there are, with each changed name's line replaced (its `rest` kept, `at` set) or dropped; **a line whose item is no longer in the folder goes** (`symlink_metadata` of the name fails); the `version` and unknown keys stay as read. When that leaves what was read, nothing is written. With no line left the file is removed. Otherwise the text of `write_beside` goes through `crate::replace_contents`, and the file is hidden (`crate::actions::hide`). Writes of one process don't interleave: one `static Mutex<()>` held from the read to the write.
- A folder that is no real folder (`folder.is_dir()` is false: an archive, a remote, a path that isn't there) is `Err` without anything tried.

### mod.rs

**`read(path) -> Marks`**: `tidy` of `streams::read(path)`; else of the item's line in its folder's `.glint-marks`; else of its line in the ledger; else `Marks::default()`. A path with no folder above it or no name (a drive's root) has none.

**`marks_read(paths)`** (on `blocking`): each path's marks in order, one `Marks` a path whatever happens (a path that isn't there has none). **It costs one stream listing an item and one look for `.glint-marks` a folder, never one an item**: the sidecar of each distinct folder is read once and kept for the batch, and only looked for when an item of that folder had nothing in its streams. The ledger is asked from memory, and not locked at all while it is empty. Over 512 paths the items are read with `rayon` (`par_iter`), still in order.

**`marks_set(paths, change)`** and **`marks_put(items)`** (`marks_put` is `Change::Put` of each item's own marks) share one private worker. For each path:

1. `before` is what `read` gives; `after` is `change.apply(&before)`.
2. Refused with words, before anything is written, `Outcome::after` being the `Err`: a path with no folder above it or no name: `"A drive itself can't be marked"`; tags whose text (each then `\n`) is over `TAGS_CAP`: `"These are more tags than a file can hold (4 KB)"`; a comment over `COMMENT_CAP`: `"A comment can be 64 KB at most"` (B1's words).
3. `after == before`: nothing is written; the outcome is `Ok((after, home))` with the home the marks are in, or `home_of(path)` when there are none.
4. **The stream**, when `caps::has_streams(path)`: `streams::write(path, &after)`. When it works the home is `Stream`, and the item's line goes from its folder's sidecar (when there is one with a line for it) and from the ledger. When it fails **and the item's streams hold marks** (`streams::read` is `Some`), the outcome is that `Err`: a later home would be hidden behind them. When it fails and they hold none, go on.
5. **Beside**: the items of one folder that got here are written **together, in one `beside::change`**. When it works the home is `Beside` and their ledger lines go.
6. **The ledger**: `put(path, &after, Why::Shut, now)`; the home is `Ledger`. It is saved once at the end; when the save fails, those outcomes become `Err("Couldn't keep the marks on this PC: {e}")`.

The outcomes come back one a path, in the paths' order, each with its `before`. Add **`pub fn home_of(path: &str) -> Home`** (PLAN §5.4): `Stream` when the item is there and its volume has streams; else `Beside` when its folder is a real folder; else `Ledger`. Re-export nothing else new.

So that the homes can be tested on a drive that has streams, the worker and `read`'s insides take what they ask about the world as arguments (a private struct: whether a path's volume has streams, the ledger to use, the time now); the `pub` functions pass `caps::has_streams`, `ledger::shared()` and the clock.

**`matches(path, wanted)`**: true at once for an empty `wanted` (nothing is read); else `wanted.accepts(&read(path))`. (`Wanted` is card A5's, filled at the same time: test only the empty one.)

**`marks_all()`**: the index's counts (`shadui_index::client::marks()` on `blocking`, Windows only; an `Err` is no counts) with the ledger's lines added: labels and tags alike in any case are one, shown as first met, most used first and then by name; `comments` summed.

### tags.rs: `tags_read` and `tags_set`

They answer exactly as they do today, through the homes: `tags_read(paths)` is each path's `marks_read` tags; `tags_set(paths, add, remove)` is `marks_set` with `Change::Tags { add, remove }`, answering the paths whose outcome is an `Err`, each with its words. `read`, `write`, `matches`, `keep_times`, `tags_all` and the file's test stay as they are.

## Tests (each a `#[test]`, named as a sentence)

Use a folder of your own under `std::env::temp_dir()` (unique with the process id and a counter; removed at the end), and a ledger of your own in it (`Ledger::open(Some(dir.join("marks.jsonl")))`): **no test may touch `ledger::shared()`'s file**. Every test that writes a stream starts with `if !caps::has_streams(&dir) { return; }`. Call the `async` functions with `crate::host::async_runtime::block_on`, as `backup.rs`'s tests do.

1. On a drive with streams, a label set on a file: its outcome has an empty `before` and `Ok((marks, Home::Stream))`; `streams::read` has it; `read` has it; no `.glint-marks` appears; the file's modified date and contents are what they were.
2. With streams said to be absent: the home is `Beside`; `.glint-marks` is there, hidden, and holds exactly `{"glint-marks":1}` and the item's line (`"name"`, `"label"`, `"at"`); `read` gives the marks; the item has no stream.
3. Three items of one folder marked in one call come back in the paths' order, and the sidecar holds their three lines sorted by name; `marks_read` of them (and of a fourth, unmarked, and a fifth that isn't there) gives five answers in order.
4. Taking the marks off the last marked item removes `.glint-marks`.
5. A line whose file was deleted is gone after the next write to that folder's sidecar.
6. A sidecar from a later version (`tests/fixtures/marks/later-version.glint-marks` copied in, with `a.txt` and `b.txt` made beside it) keeps its `{"glint-marks":2}` and `a.txt`'s `rating` and `color` after `b.txt`'s label is changed.
7. A folder whose sidecar can't be written (make a **folder** named `.glint-marks` in it), with streams said to be absent: the home is `Ledger`, the ledger's file holds `{"glint-ledger":1}` and a line with the item's `"path"` and `"why":"shut"`, and `read` gives the marks.
8. One place: an item with a line beside it and a line in the ledger, written again where streams work, has its marks in the stream, and neither line is left. And reading takes the stream's marks over a line that says otherwise.
9. An item whose streams hold marks and can't be written (make the file read-only after marking it; take the attribute off before removing the folder) answers `Err` for a new label, and nothing is written beside it.
10. `tags_set` and `tags_read` are 0.16's: after `tags_set` with `Work`, `Red`, `crate::tags::read` gives them and nothing failed; `remove` takes `work` off in any case; a `glint.tags` written by hand reads through `tags_read`.
11. A comment of `COMMENT_CAP + 1` bytes is an `Err` holding `64 KB`, and the item has no marks anywhere afterwards.
12. A drive's root (`C:\`) is an `Err` holding `A drive itself can't be marked`, and `read` of it is empty.
13. `marks_put` gives exactly the marks named (a label with a line break comes out tidy), its `before` is what was there, and putting the `before` back makes `read` what it was at the start (this is Undo).
14. A change that changes nothing (the same label again) writes nothing: the file's stream is byte for byte what it was, and the outcome is `Ok` with `before == after`.
15. The ledger: put, get in another case, take, save and open again give the same lines; an empty ledger's file is removed by `save`; a file of rubbish opens empty; a ledger with no path saves without an error.
16. `marks_read` of 600 paths, most of which aren't there, answers 600 in order.
17. `matches` with an empty `Wanted` is true for a path that isn't there; `home_of` says `Stream` for a file in the temp folder (where it has streams), and `Ledger` for a path whose folder isn't there.
18. `marks_all` with a ledger of three lines counts its labels and tags in any case as one, most used first, and its comments.

## Done when

The tests pass, clippy is clean, `tags.rs`'s own test passes unchanged, every `unsafe` block (if you need one) has its `// SAFETY:` line, and no path, name or file content can make any of this panic. Say in your report: what a batch of 200 unmarked paths costs in calls (one listing each, and how many looks for a sidecar), that the ledger is saved at the end of a call and not on a timer, and anything of §5.2 to §5.4 you could not hold to.
