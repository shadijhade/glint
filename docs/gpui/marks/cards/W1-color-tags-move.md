# Card W1 — 0.16's color tags become labels on disk (5.6)

Wave 6, round 1 · crate `glint-core` (and one marked place in glint-ui's `startup.rs`) · no window

**You own:** `crates/glint-core/src/marks/mod.rs` (`work` and what it needs; nothing of `read_many`'s or `set_many`'s signatures), a new `crates/glint-core/src/marks/settle.rs` (add its `pub mod settle;` line to `marks/mod.rs`, which is yours), and in `crates/glint-ui/src/startup.rs` the lines beside `explorer::marked::init(cx)` (:82) and `crate::marks_follow::init(cx)` (:84). Not `ledger.rs`, `carry.rs` (W5), `apply.rs` (W4).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), §5.4, §5.5, **§5.6**, §6.2, §10, and under §9's table "Wave 3, as it landed", "Wave 4, as it landed" and "Wave 5, as it landed".
Then the code: glint-model's `marks/labels.rs` (`shown` :74, `settled` :95 and its tests :196–:209) and `marks/mod.rs` (`tidy` :122); glint-core's `marks/mod.rs` (`World` :53, `real` :74, `find` :104, `work` :156, `read_many`/`set_many` :279–:288); `marks/import.rs` (`ImportOptions.have` :29: how the interface hands core the PC's labels today); glint-ui's `state.rs` (`settings` :47, `try_settings` :52; observers of `UiState`, :13), `startup.rs` :70–:90, `marking.rs` (`take_label_off`: what takes a 0.16 color tag off today). How core asks the index: `shadui_index::client` (`search`, the `tag:` filter; `find.rs`'s `same_service`).
**Check with:** `cargo test -p glint-core marks::`, `cargo clippy -p glint-core --all-targets -- -D warnings`; for the two lines in glint-ui, `cargo check -p glint-ui` (no glint-ui tests of yours: keep the UI part to the two calls).

## (a) The next write moves it

- Core keeps the PC's labels: `pub fn set_labels(labels: Vec<Label>)` and `pub fn labels() -> Vec<Label>` in `marks/mod.rs` (a `RwLock` static), **the seven of `default_labels()` when never set or set empty**.
- In `work`: what is written for an item is `settled(&labels, &after)` (5.5, 5.6): a file tagged *Red* in 0.16 is written with the label it is shown with, and *Red* leaves its tags. Compare the settled after with the before to decide whether to write, and answer the settled marks in the `Outcome`. Think through, and test, every change: **`Change::Label(None)` on such a file must not make the color tag come back as the label** (today's `marking::take_label_off` takes a 0.16 color tag off as a tag: check it still does what it says); `Change::Tags { add: ["Red"] }` on a file with no label gives it the label Red (5.6's "drawn" made true on disk); a label this PC lacks stays as the file says it (`settled`'s rule).
- `World` gains the labels (tests give their own; `real` reads `labels()`), so no test reads or sets the global.
- **The interface hands them over**: at start and whenever `state::settings(cx).labels` changes, `glint_core::marks::set_labels(labels)` (an observer of the settings global in startup.rs's marked place; compare with what was handed last, so it is called only on a change).

## (b) Once after the update, on the drives instant search covers

- `settle::run(stop: &dyn Fn() -> bool)` (sync), started by the interface once at start on a thread of its own at background priority (`crate::proc` or the thread helpers glint-core already uses for the quiet lane: find them, don't add a crate), after a delay (30 s) so start-up isn't slowed.
- It asks the index for `tag:red;orange;yellow;green;blue;purple;gray` (the seven *default* names; also the PC's labels' names and `was`, so a renamed Red is found), on every drive the index covers. Where the service isn't there or is older than marks (`same_service`), it does nothing and tries again at the next start.
- The files go through `set_many` in batches of 200 with `Change::Put(settled)` of what each has (read with `read_many`), only where that changes something; **a file with a label already keeps it** (`settled` does so). Dates are kept (the homes do it).
- **Never twice**: a marker in Glint's data folder (`crate::tools::data_dir()`, e.g. `marks-settled.json` with the version and the last path done per drive). **Stopped when Glint closes** (`stop`) between batches, and taken up where it stopped. Done for every drive: the marker says so and it never runs again. A drive the index doesn't cover moves file by file under (a).
- A 0.16 copy that reads such a file afterwards sees a `glint.label` it doesn't know and its tags less the color (5.6, §10): nothing else to do, but a test shows the tags stream is the 0.16 format.

## Tests (plain `#[test]`, files in a temp folder of the test's own, a ledger of its own: never the shared one)

1. `set_labels`/`labels`: the seven when unset or set empty. (Set it only in a test that restores it, or test through `World`.)
2. A file tagged `Red, work` (written as 0.16 wrote it: `glint.tags` alone) given `Change::Tags { add: ["x"] }` is written with label *Red* and tags `work, x`; its `glint.tags` is one tag a line and has no *Red*.
3. With a label renamed (*Red* → *Important*, `was: ["Red"]`), the same file is written with *Important*.
4. `Change::Label(None)` on a file tagged *Red* with no label: what is written (say what, and why it's right); `take_label_off`'s path still takes the color off.
5. A file with a label already and a *Red* tag keeps its label; the tag stays a tag.
6. `settle`'s batch work, given a list of paths (the index is behind a seam: no real service in a test): settles each once, writes nothing for a file that needs nothing, stops between batches when told, writes its marker, and a second run with the marker does nothing.
7. The dates of a settled file are as before (`tags_live_in_the_file_and_leave_its_dates` is the model).

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

Tests pass, clippy is clean for glint-core, `cargo check -p glint-ui` builds. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus NEW STRINGS (every `t!` string added or changed), MEASURED (if the card asks), TESTS CHANGED (every existing test you changed, and why). Say what the interface calls and where, and what happens on a PC whose service is 0.16.1's (this one's is: so (b) has never run against a real index here).
