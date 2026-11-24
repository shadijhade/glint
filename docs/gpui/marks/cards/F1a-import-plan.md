# Card F1a — Import: finding XYplorer, reading a source, finding the items, the plan

Wave 4 · crate `glint-core` · reads only, writes nothing

**You own:** `crates/glint-core/src/marks/import.rs` (everything but `import_apply`, which stays the one line it is: card F1b's `apply.rs` does the applying) and `crates/glint-core/src/marks/xy_find.rs`; tests at the bottom of each. Nothing else: not `mod.rs`, not `apply.rs`, not glint-model.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M7, M9, M10), §5.5, §7.1 to §7.4 (7.3 changed on November 24: read it as it is now), §8.1. Then the code: glint-model's `marks/xyplorer.rs` (`XY_MAX_BYTES` 14, the types 18 to 80, `read` 145, `marks_of` 246), `marks/lines.rs` (`Export` 58, `read_export` 250), `marks/labels.rs` (`Label` 10, `default_labels` 19, `label_of` 39), `marks/mod.rs` (`tidy` 122, `MAX_LABELS` 43); glint-core's `marks/mod.rs` (`read` 275, `read_all` 119 and `real` 71: a child module may call its parent's private functions, as `carry.rs` does); `permalinks.rs:320` (`crate::permalinks::win::find_volume_root_by_serial`, Windows only); shadui-index's `client::history` (`client.rs:264`) with `proto::HistoryRequest` and `HistoryEvent` (`proto.rs:78`, `:101`: `path`, `now`); `find.rs:408` (`same_service`). Then `tests/fixtures/marks/README.md` and its files: **`real-v5-zh.dat` is a file XYplorer itself wrote**.
**Check with:** `cargo test -p glint-core marks::import`, `cargo test -p glint-core marks::xy_find` and `cargo clippy -p glint-core --all-targets -- -D warnings`.

**What is there for you:** the types of `import.rs` are the contract; three fields were added before you started: `ImportOptions.have` (Glint's labels as they are now, handed in by the interface: core has no settings), `LabelPlan.takes_name`, `ImportPlan.needs_base`, and `LabelPlan.to` may be empty (the label comes in as a tag). Don't change a field's name or type. `xyplorer::read` is called as it is; keep to its types.

## xy_find.rs

`xyplorer_found() -> Option<Found>`: the first `tag.dat` that exists, looking in (7.1) `%APPDATA%\XYplorer`; where a `Startup.ini` beside `XYplorer.exe` says `[Appdata] Path=…`; `Data` beside `XYplorer.exe` (a portable copy). `XYplorer.exe` is found from Windows' *App Paths* and the uninstall entries (the registry: `Win32_System_Registry` is in the crate's features), read only. `items` is how many data lines it reads (`xyplorer::read`), `changed` its modified time in Unix ms. A file over `XY_MAX_BYTES`, unreadable or not a `tag.dat` is not found. Write it as a pure `found_in(appdata: Option<&Path>, programs: &[PathBuf]) -> Option<Found>` that tests call on folders of their own, and a thin function that gathers the real places.

Also `pub fn base_of(tag_dat: &Path) -> Option<PathBuf>`: XYplorer's own folder for storage 1 and 2 (7.3): two folders above a portable copy's `Data\tag.dat` when `XYplorer.exe` is there; else from the registry; else `None`.

**XYplorer's files are only read** (M7): opened for reading with sharing that lets XYplorer write, never held open, never written, moved or locked.

## import.rs: `import_plan(source, options)`

On a blocking thread (`crate::fsx::blocking`). Nothing in it writes anything, anywhere.

1. **Read.** At most `XY_MAX_BYTES`; more is `Err("This file is too big to be a tag database (64 MB at most)")`. A file whose first line is a Glint export's (`{"glint-export":…`, UTF-8) is read with `lines::read_export`: `version` 0, its lines' names are paths under the source's folder. Anything else goes to `xyplorer::read`; its error is the answer's. `changed` is the file's modified time.
2. **Labels** (7.3, "What an item gets"), one `LabelPlan` a label of the source, in its order (`color` is XYplorer's own, `back`):
   - pointed by the person: `options.labels` has `(source name, Glint name)`; that wins. An empty Glint name means *as a tag*.
   - **by name**, any case, through `label_of(&options.have, name)` (it knows `was`); *Grey* is *Gray*.
   - else **by XYplorer's own color**: `FC7268`, `F6AB46`, `EFDC4A`, `B5D74A`, `5DA4FE`, `E29CDC`, `B5B5B5` are its Red, Orange, Yellow, Green, Blue, Purple, Grey, whatever the language. The label becomes the one of `options.have` that is (by name or `was`) `default_labels()`'s label at that place; **`takes_name` is true when that label's name is still the default one** (the interface then renames it to the source's name). When `have` has no such label, go on to the next rule.
   - else **a new label**: `to` its own name, `new: true`, in XYplorer's color, or in its text's color where its own is too pale to tell a row by (`DDF4FF` with text `0969DA` gives `0969DA`: say in a comment what "too pale" is, one function). When `have` and the new ones already make `MAX_LABELS`, it comes in as a tag: `to` empty, `new: false`.
   - A Glint export's labels (its first line) go the same way by name, else new with their own color.
3. **Each item's path made whole**, by the storage line: 0 as written; 3 joined to the source's folder (a path that leaves that folder with `..` is refused: M10; so is one in a Glint export); 4 the drive whose serial it is (`find_volume_root_by_serial`; none connected is `At::Away`); 1 and 2 against XYplorer's folder: `options.base`, else `xy_find::base_of(source)`, else for storage 1 a `?:` path that exists on exactly one drive; when none tells, **`needs_base: true`** and those items are `At::Missing`.
4. **Where each is** (`At`), one call an item, any case: there is `Here`; its drive or server isn't there is `Away` (**a server that does not answer is given two seconds once**, not once an item: ask each `\\server` once, on a thread you stop waiting for after 2 s, and call everything on it `Away`); not there on a drive that is: `Moved { now }` when the index's history has an event whose `path` is the old path and whose `now` exists (only when `same_service()`; 30 days; ask in as few requests as you can and say how many in your report), else `Missing`. A refused path is `Missing` with the path as written. The history is behind a function your tests replace (`&dyn Fn(&str) -> Option<String>`).
5. **What each gets**: `xyplorer::marks_of(&file, item, options.extras)` with its label replaced by the plan's `to` (or moved among the tags when `to` is empty; a label number past the list is no label), then `tidy`. **`has`**: what the item has in Glint now where it is (`Here` or `Moved`), `None` when nothing; read on every core for more than 512 items, as `read_all` does.

Everything from the file is untrusted (M10): no index, slice or `unwrap` that a damaged file can break; sizes capped; nothing in it run.

## Tests (each a `#[test]`, named as a sentence; a folder of the test's own under `std::env::temp_dir()`, removed at the end)

1. `found_in` finds a `tag.dat` in an app-data folder; in a portable copy's `Data`; where a `Startup.ini` points; none anywhere is `None`; a file that is not a tag database is `None`. Reading it leaves its modified time and bytes as they were.
2. Every fixture plans without an error: storage 0 as written; storage 3 (`XYplorerTag-storage3.dat` copied into the test's folder with its two files made there) gives both `Here`; storage 4 with an unknown serial gives `Away`; storage 1 and 2 with no base say `needs_base`, and with `options.base` find the files made under it.
3. `real-v5-zh.dat`, with `have = default_labels()`: *红色* becomes *Red* with `takes_name`, and so the six in XYplorer's colors; *天蓝色* is new, in `0969DA`. With `have`'s Red renamed *Important* (Red in `was`): *红色* becomes *Important* and `takes_name` is false.
4. Labels by name: *Grey* is *Gray*; any case; a name in `was`; a pointing in `options.labels` wins; an empty pointing makes it a tag on the items; with 32 labels in `have` an unknown one comes in as a tag.
5. An item that is there is `Here` with what it has (a label set through `marks_set` first); one whose file was moved is `Moved` to where the fake history says; one that is nowhere is `Missing`; a path that leaves the folder (storage 3 with `..\..\x`) is `Missing` and nothing outside the folder is looked at.
6. A Glint export (written with `lines::write_export`) plans with `version` 0 and its labels.
7. 7.4's extras are in `gets` when `options.extras`, and not otherwise (`v5-storage0-bom-crlf.dat`'s `review.docx`).
8. Damaged files don't panic: empty, one byte, a cut line, 1,000 `|` in a line, a label number of `4294967295`, a file over the cap (make it sparse or skip the test's read by a private cap you pass in).
9. `#[ignore]`: a file of 10,000 lines made from the fixtures over 10,000 small files: print the milliseconds of reading and of finding (the report wants both).

## Done when

The tests pass, clippy is clean, nothing in your two files writes a file or the registry, and `import_apply` is untouched. Say in your report how the history is asked, what "too pale" is, the two times of test 9, and what F2 (the dialog) must know that the types don't say.
