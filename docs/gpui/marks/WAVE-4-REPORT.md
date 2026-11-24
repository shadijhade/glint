# Wave 4 of the Marks wave: report

*November 24, 2026 · branch `marks-wave` at `00cf486`, nothing committed · `git diff 00cf486` shows this wave (47 files changed, 5,298 lines added, 180 taken out), but for the files that are new and untracked, listed at the end, and for three files another session changed while this one worked (below) · thirteen cards written here, then thirteen agents on Sonnet (claude-sonnet-5-5), one a card, in the one tree.*

**Eleven cards came back done. Two agents (G1b, G2b) stalled before their last message and wrote no report**; their code was in the tree, compiles, and its tests pass, and their sections below are written here from the code, which is said in each. P1 called itself partial only because clippy stopped in another card's file. Three clippy errors were fixed here afterwards. **Nobody has seen any of it on a screen**: every card that draws was tested headless.

**Not this session's edits, and in the same diff**: `crates/glint-model/src/marks/xyplorer.rs` (a test), `tests/fixtures/marks/README.md`, `tests/fixtures/marks/real-v5-zh.dat` (new) and PLAN.md's 7.1, 7.2, 7.3 and 14: the session that wrote the plan put the real `tag.dat` in while the cards were being written, and F1a's and F2a's cards were written to 7.3 as it then read (labels by name, else by XYplorer's own color, else new).

## The whole

| Command | Result |
| :--- | :--- |
| `cargo test --workspace --no-fail-fast` | **3,907 passed, 0 failed, 56 ignored.** `thumbs::tests::draws_a_pdfs_first_page`, which failed before this wave, passed in this run; nothing here touched it. |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** |

By crate: glint-core 490 passed, 18 ignored · glint-model 422 passed, 3 ignored · glint-ui 2,840 passed, 17 ignored · shadui-index 155 passed, 7 ignored · one more target 11 ignored. One run of each, after the last edit. The access violation of `components::color_wheel` and the grid's photo-dots test came up once more when E6 ran `components::file_grid` alone, as RUN-WAVE-4 said it would; inside the whole run they pass.

## Landed before the cards, and why

PLAN §8.1 lists them ("Landed before wave 4's cards"). Each compiled and did nothing before an agent started, but `marks_in`, which is whole.

- **glint-core `marks/apply.rs`** (new): `apply`, which `import_apply` now calls on a blocking thread, `last_import`, `import_take_back`, `brought_in`, `follow`, with `LastImport` and `Followed`. F1 was two runs' work, and the two halves had to own different files: the plan stayed in `import.rs` (F1a), the applying went here (F1b).
- **glint-core `marks/wake.rs`** (new) with `listed(folder)`, and **the one call of it in `fsx::read_listing`**, for every real folder listed (F31).
- **glint-core `beside::waiting_in` and `beside::move_in`**, behind `marks_beside(folder)` and `marks_move_in(folder)` in `marks/mod.rs` (F23's core, B4), so the head's card (G2a) had something to call.
- **glint-core `marks_in(folder) -> Option<Vec<MarkedIn>>`**, written whole: `client::marks_in`'s answer, `None` where the index can't say. It is the call nothing made before; G2a's chips make it.
- **Three fields of the import's types**: `ImportOptions.have` (Glint's labels: core has no settings, so the interface hands them in), `LabelPlan.takes_name` (7.3 as it now reads), `ImportPlan.needs_base`; and `LabelPlan.to` may be empty (the label comes in as a tag). `Followed.paths` too.
- **glint-ui**: `dialogs/marks_import.rs` with `open(window, cx, source)` and `dialogs/marks_export.rs` with `open(window, cx, folder)`; `MenuEvent::ImportMarks { source }` and `ExportMarks { folder }` with their keys `do:import-marks` and `do:export-marks` and the workspace's two arms; `marks_follow.rs` with `init(cx)`, which `startup.rs` calls after `explorer::marked::init`; the module `settings/labels.rs`.

## The cards, and where what the table left to nobody went

F1 became two cards, F2 two, G1 three, G2 two; B4, E6 and P1 were added. PLAN §9's table has the thirteen rows.

| Left to nobody | Went to |
| :--- | :--- |
| F23 (*marks here are beside the files*, **Move them into the files**) | core: [B4](cards/B4-inner-folders.md) part 1; the head's line and button: [G2a](cards/G2a-head-chips.md). **No Undo step** (see "What a card's wording got wrong") |
| F31 (waiting marks wake) | [F1b](cards/F1b-import-apply.md): `wake.rs`, `ledger.rs` |
| F9's *Import marks…* and *Export marks…*; F25's row under Suggested | [P1](cards/P1-palette-and-suggested.md) |
| `reads_files` still listing `tag:` | [G1a](cards/G1a-sidebar.md) |
| No request sets `SearchRequest.marks`; a hit's two bits | [G2b](cards/G2b-in-comments.md) |
| Nothing calls `marks_all` for the sidebar | [G1a](cards/G1a-sidebar.md) |
| Nothing calls `client::marks_in` | landed first as `marks::marks_in`; called by [G2a](cards/G2a-head-chips.md). **A sort from the index (§6.6) still doesn't use it**: left |
| B4 (the folders inside a folder Windows' engine copies or moves away) | [B4](cards/B4-inner-folders.md) part 2: **it fitted the run, nothing cut** |
| E6's three | [E6](cards/E6-small-things.md): all three done, nothing cut |
| F29's *Take back the last import* and F30's switch in Settings | [G1b](cards/G1b-settings-labels.md) (the section Marks), on F1b's `last_import` and `import_take_back` |

### F1a — Import: finding XYplorer, the plan — done
- **Files:** `glint-core/src/marks/import.rs` (`import_apply` untouched), `marks/xy_find.rs`.
- **Tests:** `marks::import` 15 passed, 1 ignored (the timing); `marks::xy_find` 7 passed.
- **How the history is asked:** one request for the 30 days (10,000 events), made on the first item that is not where the source says, only when `same_service()`; if that answer is limited, one more a distinct parent folder it didn't answer, 20 at most. 1 request usually, 21 at worst. `now` must exist and differ.
- **"Too pale":** brightness over 225 of 255 (Rec. 709 weights; XYplorer's Yellow `EFDC4A` is 214, `DDF4FF` 240). A new label then takes its text's color only when that is between 40 and 225, since XYplorer's default text is black.
- **Not run for real:** the registry reads (App Paths, uninstall entries: the tests hand folders in), and the history against a live service (the tests replace it).
- **Unsure:** `found_in`'s `appdata` is `%APPDATA%` itself; storage 1 with a base uses only the base's drive letter; a storage 1 or 2 path that names its own drive is taken as written; XYplorer's seven colors are matched on the label's own color exactly, and only for XYplorer's files; an export's line whose label is not in the export's list goes by name against Glint's labels, else as written.
- **For the dialog:** `needs_base` items are `Missing` until planned again with a base; `LabelPlan.color` is XYplorer's own but for a new label, where it is already the pale rule's answer; `has` is `None` for `Away` and `Missing`; `extras` is filled even with the switch off; planning waits up to 2 s for a server or mapped drive that doesn't answer (all asked at once).

### F1b — Import: applying, the journal, taking back, following, waking — done
- **Files:** `marks/apply.rs`, `marks/wake.rs`, `marks/ledger.rs` (additions: a count of waiting lines, `away()`, `away_in(folder)`, `shared_away()`, `shared_if_open()`; the shared ledger's static moved to module level; `put` and `take` as they were).
- **Tests:** `marks::apply` 10 passed, 1 ignored; `marks::wake` 3; `marks::ledger` 4 (1 new); `marks::` whole 94 passed, 3 ignored.
- **`xyplorer.json`:** `{"sources":{"<source, lower case>":{path, modified (ms), size, at, xy, options:{meet, extras, labels, base, have}, items:{"<item, lower case>":{hash, at}}}}}`; the hash is blake3 of the marks given, cut to 128 bits. **A journal:** `imports\<Unix ms>.jsonl`, a first line `{"glint-import":1,"source":…,"at":…}`, then a ledger-format line an item that changed with its marks before (`"none":true` where it had none). It is made when the first batch changes something: an import that changes nothing writes none.
- **Not done:** `follow()` itself has no test (it calls `import_plan`, which was another card's): the one `stat` and the function that applies a plan made by hand are tested.
- **Unsure:**
  - `follow()` plans with the labels remembered at the last import; labels renamed since are stale to it (a `follow_with(have)` would be small).
  - To wake a waiting mark the ledger's line is taken out first, then written through the worker: for a few milliseconds a reader sees no marks on that item. A write that fails puts the line back.
  - The first listing of a session can't read the ledger's file on its own thread: a thread opens it and wakes that folder; other folders listed in that moment wake at their next listing.
  - Taking back a waiting item that had marks in the ledger before restores them as `shut`, not `away`.
  - `follow` answers `None` when the source changed and nothing came or went.
  - The report's outcomes (`Marked`, `Waiting for its drive`…) are plain strings in core, not `t!`.

### F2a — Import marks…: the preview, how far, what happened — done
- **Files:** `dialogs/marks_import.rs`.
- **Tests:** `dialogs::marks_import` 16 passed (the card's twelve and four more).
- **How far is shown:** the dialog closes; a notice `Importing marks… 40%`, pushed again only when the percent changes, and the taskbar button's bar: the archive work's way (`notices.rs`), not `compress_in_place.rs`'s, which keeps its dialog open.
- **Undo does not take the labels back**: a step holds marks only (`UndoAction` has no settings variant). A label renamed or added by an import stays after Undo.
- **Differs from the card:** the source's line is parts joined with ` · `, not one `t!` string (one string left a trailing separator with no extras). The notice stays 20 s, not 8, so *What happened* can be reached.
- **Unsure:** pointing a label at *New label* sends `(name, name)`, by F1a's wording, **not run against core** (each card was tested against fakes: see "Left between the cards"); *Import* is on when any item would change (waiting items always count); an unknown kind of extra column shows *left out*; it ran `rustfmt` on its own file alone.

### F2b — The suggestion; following when Glint comes to the front — done
- **Files:** `suggestions.rs`, `marks_follow.rs`, one place in `settings/overview.rs` (`load_drives` also asks the `tag.dat` fact).
- **Tests:** `suggestions::` 9 passed (5 new); `marks_follow::` 4 passed.
- **How:** `Kind::XyplorerMarks` (id `xyplorer-marks`, may notify, after `ToolUpdates`, icon `Tag`); the path rides in `Suggestion.drive`; the fact is a kept global the Overview and `offer` fill. The window coming to the front is heard through `cx.observe_new::<Workspace>` and that window's activation, so `workspace.rs` is untouched.
- **Not done as worded:** *3 marks came from XYplorer* is tested through the function that words it, not read off the notification; the suggestion's button is tested by the path it hands on.
- **Unsure:** **if the workspace is not made inside its window, the following quietly never starts** (true in the test's host, not checked against the real main window); an error counts as an answer for the five seconds; the Overview now waits for the `tag.dat` lookup before it reads the drives.

### F3 — Export marks…, both formats — done
- **Files:** `marks/export.rs` (`marks_export` keeps its signature and calls the new `marks_export_with(root, to, format, labels, stop)`), `dialogs/marks_export.rs`.
- **Tests:** `marks::export` 9 passed with the timing; `dialogs::marks_export` 5 passed. The junction test made a real junction; the streams half ran.
- **Unsure:** XYplorer's own limit of labels is not known: 32 is used; a label the list lacks is added at its end in gray until then, and past it the item is written with no label. A `.glint-marks` line for an item that isn't there is not exported (the ledger's are). Closing the dialog while it runs stops the work. The save dialog has no file-type filter.

### G1a — The sidebar's Labels, and a tag's own menu — done
- **Files:** `sidebar/rows.rs`, `sidebar/mod.rs`, `sidebar/tests.rs`.
- **Tests:** `components::sidebar` 67 passed (10 new). Clippy was not run by the agent after its last two small edits; the whole run here is clean.
- **What finds a tag's items:** `find_files` with `tag:"name"` over every fixed and removable drive, hidden items too, 100,000 at most.
- **Unsure:** renaming *work* to *Work* is refused (adding and taking off alike in any case could cancel); a tag with a comma is not refused; right-click on a tag's row was added (it had none); **`dialogs::enter_runs_main` panics when used from the dialog's own view** ("already being updated"), and nothing else calls it: the dialog listens for Enter itself. That helper is worth a look.

### G1b — Settings → Files → Labels, and Marks' two rows — the agent stalled; written here from the code
- **Files:** `settings/labels.rs` (692 lines), `settings/tests_labels.rs` (new), and the marked places: `pages.rs` (the Files page's sections are `files`, `labels`, `marks`; four `SEARCH` entries), `general.rs` (two arms of `section_of`), `settings/mod.rs` (the test module's line).
- **Tests:** `components::settings` 70 passed, 0 failed, run here: eleven in `tests_labels.rs`, six pure ones in `labels.rs`. While the agent worked, others saw three to six of its tests fail; all pass now.
- **What is there:** the five edits as pure functions (`rename` with `was`, `recolor`, `add`, `move_by`, `remove`); a row a label with its dot, a popup of twelve colors (today's seven and Teal, Sky, Pink, Lime, Brown), a name box, Move up, Move down, Remove; *Add a label*, *Back to the seven colors*; a notice *Labels changed* with Undo; the section Marks with the switch and *Take back the last import* on a source tests replace.
- **Finished here:** two clippy errors (`set_source` is used only by tests; a needless clone in a test).
- **Not known, since there is no report:** what the agent was unsure of, and whether it changed a test that was there (the settings' other tests pass, and `git diff` shows none of them changed). **Read this card's code before trusting it more than the others.**

### G1c — The settings file's part Labels — done
- **Files:** marked places of `persistence.rs`; one test in `settings/transfer.rs`.
- **Tests:** `persistence::` 28 passed, 1 ignored; the transfer's 2.
- **Unsure:** a file whose labels are all invalid leaves the PC's labels alone even on Replace; **a Settings part no longer touches the PC's labels at all** (before, Replace reset them to the seven), but a file from before this card, with labels in its Settings part and no Labels part, still brings them; the 20 recent tags kept is the tags dialog's private number written again here, so the two can drift; a color is masked to 24 bits through a float.

### G2a — The head's label chips; *marks here are beside the files* — done
- **Files:** `pane_head/mod.rs`, `pane_head/tests.rs`.
- **Tests:** `components::pane_head` 22 passed, 1 ignored (the timing).
- **Unsure:** pressing a label's chip **replaces the whole search box**, exactly as a kind's chip does; hidden items are counted (the index can't tell them apart); two `marks_in` answers in flight are not ordered, the last to land wins; a lit chip uses the kit's selected fill, the label's color on its dot alone.

### G2b — *In comments* — the agent stalled; written here from the code
- **Files:** glint-core `find.rs` (`FindOptions.marks`, `FindEvent::Marked`, `index_request` and `add_hits` split out of `search_index`); glint-ui `explorer/in_comments.rs` (new, 280 lines, with its line in `explorer/mod.rs`), `explorer/find.rs`, `find_types.rs`, `find_tests.rs`, the parts' placing in `explorer/rows.rs`, `explorer/source.rs` (two lines), `file_list/cells.rs` (the header). **Outside its files, the one field or arm the card allowed:** `mcp.rs:366` (`marks: false`), `quick_search/source.rs` (`marks: false`, and an arm for `Marked` that changes nothing).
- **Tests:** in the whole run, all passing: two new in glint-core's `find::`, seven new in `find_tests.rs` (found by a comment with the line that holds the word; by a tag alone; three parts in order; no line until read, none when the marks no longer hold the word; only flagged paths read, one call a batch; grouped by something else; Arabic), four in `in_comments.rs`. One line of an older test in `find_tests.rs` changed (a `FindOptions` literal).
- **How:** a search asks `marks` when it has plain words and is not a flat view, and only of a service of the app's version. A file found by a comment is kept where a file found inside is (`inside.only`, `inside.hits`) plus a mark of its own (its path after a `\u{1}`), and `rows.rs` puts those under *In comments*, between *Names* and *Inside files*. Its marks are read through the tags hub's backend, 2,000 files a search at most; the rest stay under the header with no line.
- **Not known, since there is no report:** what the agent was unsure of. Its last words were "fix warnings, then run the find tests"; the tests pass and clippy is clean. **Not checked by anyone: what a real service sends** (whether a hit found by its name *and* its comment comes flagged, which decides whether it stays under *Names*).

### B4 — The folders inside; marks beside files moved into them — done, both parts
- **Files:** `marks/beside.rs`; `shell.rs` (a private `mod inner` before `mod imp`, and two additions in `imp::file_op`).
- **Tests:** `marks::beside` 5 passed; `shell::` 10 passed (5 new).
- **How:** a copy reads the inner folders' marks after the engine and gives them under the copy; a move reads them before, only when the two ends are on different volumes (the volume's serial, asked of a handle on each path), and gives them after. The walk stops at 50,000 folders a source, is not started on another computer, and skips every link and junction.
- **Unsure:** a source whose volume can't be asked counts as another volume (one walk too many); the agent did not check whether its junction test made a junction or returned early.

### E6 — The grid's sort bar, Edit tags…' chips, a sort by label — done, nothing cut
- **Files:** marked places of `glint-model/src/entries.rs`, `file_grid/mod.rs`, `file_grid/tests.rs`, `dialogs/tags.rs`.
- **Tests:** `entries::` 34 passed, 1 ignored; `components::file_grid::tests` 71; `dialogs::tags` 27.
- **Unsure:** the key a row is kept **by the entry's path, not its address** as the card suggested (a sort moves its items, so an address would name another row); the bar offers the marks in a search's results and in Recent too, where rows read marks.

### P1 — *Import marks…* and *Export marks…* in Ctrl+K and in a marks file's menu — done (it said partial for clippy alone)
- **Files:** `palette/commands.rs`, `palette/tests/marks.rs`, `menus/smart.rs` (`is_marks_file`, `import_marks_row`; `zone_of` puts the row first under Suggested), `menus/tests_smart.rs`, and **the marked place: `menus/mod.rs`, `MenuHost::item_nodes`**.
- **Tests:** `components::palette` 62 passed; `components::menus` 201 passed.
- **Finished here:** one clippy error in its test.
- **Not done:** no test opens `tag.dat`'s menu in a window, and the condition "one file, on this PC's disks" in `menus/mod.rs` has no test (the row and its place under Suggested are tested as data).
- **Unsure:** the palette has no field for search words, so they are in each command's dimmed line of detail; *Export marks…*'s detail holds "comments", so it can come before *Edit comment…* for that word.

## Tests that were there and changed

- **G1a**, `sidebar/tests.rs`: the test world clears the settings' seven labels (else every older list test would show a Labels section) and its fake writes down what it is asked; `with_no_tags_from_instant_search…` waits for the answer instead of asserting at once (tags are asked only after marks failed). `rows.rs`: the two `reads_files` tests, as the card named.
- **G1c**, `persistence.rs`: `the_export_has_settings_file_ts_shape` (a key more), `export_then_import_gives_the_same_parts_and_counts` (9 parts; the Settings part counts one option fewer).
- **E6**: `file_grid/tests.rs`, `the_header_names_the_sorts_and_a_click_sorts_by_it` (three more buttons; Size is the eighth) and `a_tile_with_a_comment_has_the_note_named_comment…` (finds the note by its id, since *Comment* now also names a button of the bar); `dialogs/tags.rs`, `typed_text_shared_tags_toggles_and_offers_read_as_the_typescript_did` and `the_box_starts_with_what_the_items_share_and_save_adds_and_removes` (they pressed color chips, which are gone).
- **P1**, `palette/tests/marks.rs`: `a_label_renamed_in_the_settings_is_listed_and_found_under_its_new_name` asserts *Edit comment…* is among the results, not first.
- **G2a**, `pane_head/tests.rs`: no test's body; the fake probes gained three fields.
- **G2b**, `find_tests.rs`: one `FindOptions` literal.

## Every new string

- **Import (F2a):** `Import marks`, `Marks files`, `All files`, `XYplorer's folder`, `Reading…`, `version {v}`, `changed {when}`, `Labels`, `Extra columns`, `Where the items are`, `is {label}`, `is {label}, which Glint will then call {name}`, `New label`, `As a tag`, `comes in as a tag`, `becomes the tag {caption}`, `becomes tags like {caption}: 4`, `becomes tags like {caption}: value`, `becomes a line of the comment`, `left out`, `Bring extra columns in`, `Here`, `Moved since: found where they are now`, `On a drive that is not connected: they wait`, `Not found: they wait`, `Already marked in Glint`, `Fill what is empty`, `The file's marks replace Glint's`, `Glint couldn't tell where XYplorer is`, `Choose its folder…`, `Keep following this file`, `Import`, `Importing marks…`, `Importing marks… {percent}%`, `Marked {marked}`, `{waiting} waiting`, `{missing} not found`, `Nothing to bring in: everything is marked already`, `import of {what}`, `What happened`, `Items`, `Copy`, `Couldn't import the marks`, `A newer format than Glint knows: read by the same rules`.
- **The suggestion and following (F2b):** `Bring your XYplorer labels, tags and comments`, `{n} marked in XYplorer, kept as they are there`, `Import marks…`, `{n} came from XYplorer`.
- **Export (F3):** `Export marks`, `Everything marked in “{name}” and below it, into one file.`, `Format`, `Glint`, `Text, one line an item: read by Import marks… on any PC`, `XYplorer`, `An XYplorerTag.dat that XYplorer's tagload merges`, `Choose…`, `Export`, `Exporting…`, `Stop`, `Exported the marks of {n}`, `Nothing below “{name}” is marked`.
- **The sidebar (G1a):** `Label {name}, {n}`, `Label {name}`, `What's labeled “{label}”`, `Rename everywhere…`, `Take off every file…`, `Tag`, `Nothing has the tag “{tag}” where Glint can search`, `Rename the tag “{tag}”`, `New name`, `Rename`, `Give the tag another name`, `Take off “{tag}”`, `Take “{tag}” off {n}?`, `Take off`.
- **Settings (G1b, read from the code):** `Labels`, `Marks`, `Teal`, `Sky`, `Pink`, `Lime`, `Brown`, `Another color`, `Give the label a name`, `A label's name is one line of 64 characters at most`, `Another label is or was called that`, `32 labels at most`, `New label`, `Labels changed`, `Color of {name}`, `Colors`, `Name of the label`, `Move up`, `Move down`, `Remove`, `A label is kept in the file by its name. Renaming one changes nothing on disk: files that say the old name show the new one.`, `Add a label`, `Back to the seven colors`, `Keep following XYplorer`, `When its tag.dat changes, what is new in it comes in as Glint starts or comes to the front`, `Take back the last import`, `Take back`, `{n} from {name}, {when}`, `Nothing was imported in the last 30 days`, `Took back the import of {n}`.
- **The head (G2a):** `Label {name}: {n}`, `{n} here are beside the files`, `Move them into the files`, `Moved the marks of {n} into the files`.
- **In comments (G2b):** `In comments`, `tags: {list}`.
- **The palette (P1):** `Import marks…`, `Export marks…`, `From XYplorer's tag.dat or a Glint marks file`, `Labels, tags and comments into one file, for XYplorer too`.
- **Not `t!`:** core's outcomes and refusals (F1a, F1b, F3), and the settings file's line `{n} labels` (built as its other lines are).
- **None new:** B4, E6, G1c.

## Measured

All on this PC, in **debug or test builds**, each a single run by the card's agent while twelve other agents built and tested beside it. None is a release number, and none was measured twice but E6's.

| What | Result |
| :--- | :--- |
| **Import of 10,000 lines over 10,000 small files: reading** (`xyplorer::read`, F1a) | 141 ms (§6.6 wants under 50: a debug build) |
| **finding** (F1a: each item looked for, and what it has) | about 640 ms (the whole plan 778 ms) |
| **applying** (F1b, 100 folders, at background priority) | **40.7 s where the drive keeps streams**; 2.0 s where it doesn't (one write a folder) |
| **Export of 10,000 marked items** in 100 folders (F3) | Glint's format 285 ms; XYplorer's 321 ms |
| **The head's chips on a folder of 5,000**, 500 marked (G2a) | 2.5 ms from every mark being in to the chips worked out. The read of the 5,000 itself is `RowFacts`', not measured here |
| **The sidebar's counts** | **not measured**: G1a measured nothing. It is one `marks_all` call |
| **A sort by label**, 10,000 marked rows (E6, its own test data) | before 723 ms; after 148 and 123 ms |

The number to look at is the 40.7 s: about 4 ms an item to write three streams at background priority in a debug build with the PC busy. It is in the background and says how far it is, but it is twenty times the sidecar's cost and worth a release-build run on a quiet PC.

## Left between the cards

- **The import has never run end to end.** F1a's plan, F1b's applying, F2a's dialog and F2b's following were each tested against fakes or plans made by hand, as the rule that no card needs another's code required. Nothing has called `import_plan` and handed its answer to `apply`, and `follow()` has no test. The first real run is the look in the app, with `real-v5-zh.dat` and the fixtures.
- **Undo of an import leaves the labels** it renamed or added (F2a); *Take back the last import* in Settings does too.
- **Following plans with the labels as they were at the import** (F1b), and its start depends on the workspace being made inside its window (F2b).
- **A sort or a group from the index** (§6.6's third row): `marks::marks_in` is there and only the head calls it.
- **F15 in the open folder**: *In comments* is the index's; filtering the open folder as you type does not look in comments, and fd's drives send no such hits.
- **The palette's two commands and the menu's row open dialogs** that were stubs while P1 worked: the wiring is the workspace's two arms, landed first, untried in a window.
- **`dialogs::enter_runs_main`** panics when called from the dialog's own view (G1a).
- **The recent tags' cap** is written in two places (G1c).
- Still as wave 3 left them: a `.glint-marks` goes with its folder as a file; ledger lines under a renamed folder keep its old path; a line that moves loses a later version's keys (B4's `move_in` too).

## What a card's wording got wrong, and what to look at

- **F23's "through `marks_put`", and its "one Undo step"** (RUN-WAVE-4). `marks_put` writes nothing for an item whose marks don't change, and moving a line into the file changes no mark, only its home. So B4 writes the streams itself, and **there is no Undo step**: a marks step can only put marks, which would write them into the files again. To have one, Undo needs a step that puts lines back beside the files.
- **F2a, "one Undo step" with the labels.** The card allowed for it; `Sequence` holds only file and marks steps.
- **F2a, the source's line as one string.** It can't leave parts out; the agent joined parts.
- **F2a, "as other long runs show it".** The card pointed at `compress_in_place.rs`, which keeps its dialog open; the archive work's notice was the right pattern.
- **E6, "by the entry's address".** Wrong for a sort that moves its items; the agent used the path.
- **G1a, "the tests pass unchanged".** A Labels section from the default settings shows in every older list test; the test world had to clear the labels.
- **G1c and "8.1 left that to G1".** The card took `labels` out of the Settings part; that also means Replace no longer resets labels.
- **P1, "words a search finds them by".** The palette has no such field.
- **Thirteen agents on one build folder.** Two agents stalled after about two hours, both in glint-ui, the crate every build waits on; the ones that finished took 28 to 90 minutes, most of it waiting for the lock or for another's half-written file (`xy_find.rs`, `marks_import.rs`, `find_tests.rs` and `entries.rs` each stopped other agents' builds or clippy for minutes). Nobody changed a file that wasn't theirs. Fewer cards a wave in glint-ui, or two rounds, would have been quicker.
- **Line ends**, as before: the files the agents wrote have LF.

## Not run, and not seen

- **Nothing was seen on a screen**: the import's preview and its report, the export's dialog, the sidebar's Labels and the tag's menu, Settings' two sections and the color popup, the head's chips and the line under them, *In comments*, the grid's sort bar, the palette's two commands, in either theme.
- Not against a real XYplorer, a real registry entry for it, a live index service (the history, `marks` in a search, `marks_in`), or a drive without streams.
- The end-to-end checks (`tests\e2e\run.ps1`) and the installed Glint.
- `cargo fmt`: by the rules (one agent ran `rustfmt` on its own new file).
- Nothing of waves 5 and 6 but E6's three.

## Every new file

- `crates/glint-core/src/marks/apply.rs`, `crates/glint-core/src/marks/wake.rs`
- `crates/glint-ui/src/dialogs/marks_import.rs`, `crates/glint-ui/src/dialogs/marks_export.rs`
- `crates/glint-ui/src/marks_follow.rs`
- `crates/glint-ui/src/components/settings/labels.rs`, `crates/glint-ui/src/components/settings/tests_labels.rs`
- `crates/glint-ui/src/explorer/in_comments.rs`
- `docs/gpui/marks/cards/`: `F1a-import-plan.md`, `F1b-import-apply.md`, `F2a-import-dialog.md`, `F2b-suggestion-and-following.md`, `F3-export.md`, `G1a-sidebar.md`, `G1b-settings-labels.md`, `G1c-settings-file-part.md`, `G2a-head-chips.md`, `G2b-in-comments.md`, `B4-inner-folders.md`, `E6-small-things.md`, `P1-palette-and-suggested.md`
- `docs/gpui/marks/WAVE-4-REPORT.md` (this file)
- Not this session's: `docs/gpui/marks/RUN-WAVE-4.md`, `tests/fixtures/marks/real-v5-zh.dat`
