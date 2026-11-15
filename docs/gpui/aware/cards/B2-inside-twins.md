# Card B2 — A folder's inside, and identical copies

Wave 1 · crate `glint-core` · I/O on worker threads

**You own:** `crates/glint-core/src/aware/inside.rs`, `aware/twins.rs` (tests at the bottom of each).
**Read first:** PLAN.md §2 (R1, R5), §5.1 (`Inside`), §5.2; `crates/shadui-index/src/client.rs` (`sizes` at line 150, `search` at 313, `Event` at 302) and `crates/shadui-index/src/proto.rs` (`Sizes` at 194, `SearchRequest` at 99, `Hit` at 259); `crates/glint-core/src/index.rs` lines 76 to 98 (how a search is asked whether the index covers a folder: the `Plan` event). Card B1 writes `super::pool` at the same time as you: use `super::pool::install(lane, || …)`, which is in the tree already (until B1 lands it just calls the closure).
**Check with:** `cargo test -p glint-core aware::inside aware::twins` (two runs, one per filter) and clippy on the crate.

## inside.rs — `inside(path, lane, stop, on)`

What is in a folder, all the way down: `Inside { files, folders, bytes, junk, done, from_index }`.

**Junk** is what a build leaves that can be made again: the bytes of the folders *directly inside* `path` whose name, ignoring case, is one of `target`, `node_modules`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo`, `.parcel-cache`, `.angular`, `.gradle`, `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.venv`, `venv`, `obj` (a public `const JUNK: &[&str]`). `target` counts only when `Cargo.toml` is beside it, `node_modules` only beside `package.json`, `obj` only beside a `*.csproj` or `*.vcxproj` file, `.venv` and `venv` only when they hold `pyvenv.cfg`; the others always. Write this as `fn is_junk(folder: &Path, name: &str) -> bool`.

**From the index, when it covers the folder** (one answer):
1. `shadui_index::client::sizes(path)`. An error means no index, or not this drive: go to the walk.
2. `bytes = sizes.total`; `junk` = the sum over `sizes.sizes` of the entries whose name `is_junk`.
3. The number of files: `client::search` with `SearchRequest { query: "file:".into(), roots: vec![path.into()], hidden: true, exclude: vec![], max_results: Some(20_000), fuzzy: false, rank: false }`. Stop listening (return `false`) if the `Plan` event doesn't cover the root (as `index.rs` does). From `Done { count, limited, .. }`: `files = count` when not `limited`, else `0` (more than can be counted cheaply: not said, R1). Ignore `Hits`.
4. `on(Inside { files, folders: 0, bytes, junk, done: true, from_index: true })`.

**Else a walk**, inside `pool::install(lane, …)`:
- Go down with `rayon::scope`: one task per folder, each reading its folder with `std::fs::read_dir` and adding to shared `AtomicU64`s (`files`, `folders`, `bytes`, `junk`). A file's size is `entry.metadata()?.len()`. A folder directly inside `path` that `is_junk` has everything under it added to `junk` as well as to `bytes` (pass a flag down).
- **Never follow a link:** skip any entry whose `file_type()` is a symlink, and on Windows any whose attributes have `FILE_ATTRIBUTE_REPARSE_POINT` (`std::os::windows::fs::MetadataExt::file_attributes() & 0x400 != 0`): junctions loop, and cloud placeholders must not be read.
- A folder that can't be read is skipped, silently.
- Check `stop` at the top of every task; once it is set, tasks return at once.
- Report as it goes: the calling thread, while the walk runs, calls `on(Inside { …, done: false, from_index: false })` about every 150 ms with the counts so far (run the walk in `rayon::scope` from a spawned task and poll with `std::thread::sleep(Duration::from_millis(150))` until a shared `AtomicBool` says it has finished; or structure it your way, as long as `on` is called from one thread only).
- At the end, if `stop` isn't set: `on(Inside { …, done: true, from_index: false })`. If `stop` is set: no final call.
- A walk that has run for 3 seconds gives up as if stopped (a constant `LONGEST: Duration`): what it had counted has been reported with `done: false`, and nothing more is said.

A `path` that isn't a folder calls `on` never.

## twins.rs — `twins(path, size, stop)`

Other files on this PC with exactly the same bytes.

1. `size < 1024` or `size > 256 * 1024 * 1024`: `None`.
2. Ask the index for files of exactly that size: `client::search` with `query: format!("file: size:{size}")`, `roots`: the drives' roots the index has (`shadui_index::client::status()`'s `volumes`, each `volume` as its root, e.g. `C:\`; an error or no volumes: `None`), `hidden: true`, `max_results: Some(64)`. Collect the `Hit`s' paths except `path` itself (compare ignoring case). `limited` (more than 64 files of that size): `None`: too common a size to say anything about.
3. No candidate: `Some(vec![])`.
4. Hash `path` and every candidate with BLAKE3 (`blake3::Hasher`, a 1 MB buffer, checking `stop` between reads; a file that can't be read, or whose length on disk now differs from `size`, is not a twin). Do the candidates at once with `rayon::iter` inside the pool the caller is on (`par_iter`). Stopped: `None`.
5. `Some(paths of the candidates whose hash equals path's)`, in the index's order.

## Tests

Use a folder of your own under `std::env::temp_dir()`; remove it at the end. The index service may or may not run on the machine the tests run on, so test the walk through a private `fn walk(path, lane, stop, on)` and the index branch only through its pure parts.

1. `is_junk`: `target` beside a `Cargo.toml` is junk and alone is not; `node_modules` beside `package.json`; `__pycache__` anywhere; `Target` (capital) counts; `src` never.
2. The walk of a small tree (3 files of known sizes in the root, 2 in `sub`, 1 in `target` beside a `Cargo.toml`) ends with `done: true`, `files == 6`, `folders == 2`, `bytes` the sum, `junk` the one file's size, `from_index == false`.
3. The walk's last call is the only one with `done: true`, and the counts never go down between calls.
4. A stop flag set before the walk: `on` is never called with `done: true`.
5. A path that is a file, or doesn't exist: `on` is never called.
6. The walk does not follow a junction: make one with `std::process::Command::new("cmd").args(["/c", "mklink", "/J", link, target])` pointing back at the tree's root; if making it fails, `return` (skip); else the walk still ends, with the same counts as test 2.
7. `twins`: sizes under 1 KB and over 256 MB give `None` without touching the disk (a path that doesn't exist, no panic).
8. A private `fn same_bytes(a: &Path, b: &Path, size: u64, stop: &AtomicBool) -> Option<bool>` (step 4 for one pair): two files with the same 5,000 bytes are the same; one byte changed is not; a missing file is `Some(false)`; stopped is `None`.

## Done when

The tests pass, clippy is clean, and nothing here reads a file's contents except in `twins`.
