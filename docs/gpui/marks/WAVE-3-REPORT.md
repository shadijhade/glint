# Wave 3 of the Marks wave: report

*November 23, 2026 · branch `marks-wave` at `13660c7`, nothing committed · the tree before this wave is `6ef80cf` (`git diff 6ef80cf` shows this wave alone, but for the files that are new and untracked, listed below) · ten cards written here, then ten agents on Sonnet (claude-sonnet-5-5), one a card, in the one tree.*

**Seven cards came back done and three partial**; the three were finished here, each on a small point that lay in a file only the coordinator may touch. So all ten are in. **Nobody has seen any of it on a screen**: every card that draws was tested headless, and the look, in both themes, is still to be checked in the real app. One thing is not this wave's and is said plainly below: the test binary that `cargo test -p glint-ui` builds dies with an access violation in the Color Wheel's tests, while the one `cargo test --workspace` builds passes them.

## The whole

| Command | Result |
| :--- | :--- |
| `cargo test --workspace --no-fail-fast` | **3,751 passed, 1 failed, 51 ignored.** The one is `thumbs::tests::draws_a_pdfs_first_page`, known before this work. |
| `cargo clippy --workspace --all-targets -- -D warnings` | **clean** |

By crate: glint-core 432 passed, 1 failed, 15 ignored · glint-model 418 passed, 2 ignored · glint-ui 2,747 passed, 16 ignored · shadui-index 154 passed, 7 ignored · one more target 11 ignored.

The first run of the whole had one more failure, `file_grid::tests::parity::tiles_show_their_tags_as_dots_and_ask_for_each_tile_once`: it expected Red and Blue as dots, and they are labels now (a tile shows them as its name's pill). Its two expectations were changed here and the whole was run again; the totals above are the second run's.

**The access violation, which is not in the totals.** `cargo test -p glint-ui <filter>` builds another binary than `cargo test --workspace` does (the workspace's features are unified; a single package's are its own). In that binary the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with `0xc0000005` **after the test's body has finished** (found by printing at its last line), each also when run alone. Card E1c's agent met it and reported it. It was looked for in this wave's code by putting files back to `6ef80cf` one group at a time and running the test again (everything was put back afterwards and checked byte for byte): E1a's three files, B3's five, E4's three, E1b's, E1c's and E2a's seventeen together, E3's and E5's three. **The crash stayed every time**, so it is not any one of those; E2b's files and what was landed first were not tried. Every other group of glint-ui's 2,763 tests passes in that binary too (154 groups run one by one). Whether a `-p glint-ui` build of the tree before this wave crashes the same way could not be tried here (no `stash`, no `checkout`). For the session that reviews: build `6ef80cf` with `cargo test -p glint-ui components::color_wheel`; if it crashes there too it is older than this wave.

## Landed before the cards, and why

PLAN §8.1 lists them. Each compiled and did nothing, or did its whole small job, before an agent started.

- **glint-ui `theme.rs`**: `label_tone(color)` and `label_wash(color, cx)`. The wash's strength (an alpha of 0.18, the same in both themes) is this one function's: the list, the grid and the menu's dots all go through it. It has not been tuned on a screen.
- **glint-model**: the marks' three columns in `COLUMN_ORDER` (before Size) and Label in `GROUP_BYS`, so both can be chosen. One test of the list's header menu counted its rows (12 at most): it allows 15 now. `marks::clip_text(name, marks)` and `marks::clip_marks(text)`, written whole with a test: one item's marks as the line a `.glint-marks` would hold, for Copy marks and Paste marks.
- **glint-ui `marking.rs`** (new): eight functions for the four key actions and the menus' five events, and **the workspace wired to them** (`workspace.rs`: the events' arms and four `on_action`s). The table had left the handlers to nobody: they became card E5.
- **glint-ui**: `dialogs/comment.rs` with `open(window, cx, paths)` (E3); the module `preview/marks.rs` (E4); `explorer::RowFileMarks` and the field `RowSpec::marked`; `explorer/marked.rs` with `init(cx)`, which `startup.rs` calls after Git's (E1a).

**E1 became three cards and E2 two**, as RUN-WAVE-3 expected, and **E5 was added**: [E1a](cards/E1a-rows.md) (the rows' marks reach the rows), [E1b](cards/E1b-list.md) (the list), [E1c](cards/E1c-grid.md) (the grid), [E2a](cards/E2a-menu.md) (the menu), [E2b](cards/E2b-palette-and-words.md) (`Ctrl+K`, plain words, the key lists), [E5](cards/E5-marking.md) (the handlers). PLAN §9's table has the rows.

## Finished here after the cards

- **Both dialogs open out of the row** (E3 came back partial on this alone): `dialogs/mod.rs` gained `show_out_of(window, cx, title, width, view, path)` (`Dialog::opens_from` with `stage::spot_of`, as Quick Look does; plainly when the item isn't on screen), and Edit tags… and Edit comment… call it with their first path. Not seen on a screen.
- **The palette knows the labels** (E2b came back partial on this alone): `palette/mod.rs` keeps the labels' names as they are when it opens and builds its commands with `commands::all_with` at all three places, so `Label: Red` … are listed.
- **The grid's parity test** (E1c came back partial on this): its expectations, above.
- **The icons**: `StickyNote`, `MessageSquare` and `Eraser` are in the kit's list of icons checked against the embedded assets, and `StickyNote` in the grid's.
- **`docs/instant-search.md`**: the security model's new point, *Labels, tags and comments*, from C4's sentences.

## The cards

### B3 — Marks follow what Glint copies and moves; fd's drives — done
- **Files:** `glint-core/src/marks/carry.rs`, `find.rs`, three places in `queue.rs`, `actions.rs`, `convert.rs`.
- **Tests:** `marks::` 44 passed (9 new), `find::` 6 passed (3 new), `queue::` 5 passed (none new). In the whole: glint-core 432 passed, 1 failed.
- **Clippy:** clean.
- **New names:** `carry_one(source, destination, moved)`, `taken(original) -> Marks`, `given(made, &marks)`.
- **Not done:** a **folder's** own marks are not carried by the queue (only files have steps there); ledger lines under a renamed folder's old path are not rewritten; no new test in `queue.rs`.
- **Needs:** the interface's call (below, "Left").
- **Unsure:** it reaches `marks/mod.rs`'s private helpers from the child module rather than through anything exported; a line that moves loses its own unknown keys (a later version's); in a move across drives the source's line is taken out when the copy is made, not when the original is deleted, so a copy that then fails to verify leaves the original without its line; the test that a folder's sidecar is written once counts by the file's modified time, which is weak.

### C4 — The far reads by file reference — done
- **Files:** `shadui-index/src/far.rs` (rewritten: 726 lines where 353 were; it is untracked, so `git diff 6ef80cf` does not show it), `engine.rs` (`FAR_READS` is **on**, with its comment).
- **What changed in `far.rs`:** `read(path)` and every function that took an item's path are gone. `read_ref(root, reference)` opens by `OpenFileById` on a handle of the drive's root for attributes alone, lists the streams with `FileStreamInfo`, and only for an item with a `glint.*` stream opens again for reading and walks `BackupRead` (a `Backup` trait with `header` and `collect`, so the walk is tested on bytes that lie); `drain` opens the root once a pass and takes references in place of paths. Seven `unsafe` blocks, seven `// SAFETY:` lines.
- **Tests:** `far::` 18 passed; the crate 154 passed, 7 ignored.
- **Clippy:** clean.
- **The link test really ran** this time: a symbolic link and a junction were both made and both read as no marks. It would not fail if neither could be made.
- **Unsure:** a reference whose sequence is 0 is refused outright (NTFS reads 0 as "any file with this number"); `drain` now reads a far record that has no name (it opens by reference; the old code dropped it).

### E1a — The rows' marks reach the rows — done
- **Files:** `explorer/rows.rs`, `explorer/marked.rs`, `explorer/model.rs` (`row_spec`, `wants_row_marks`, `set_row_marks`), `explorer/tab.rs` (one field, `marked`).
- **Tests:** `explorer::rows` 22 passed, 2 ignored; `explorer::marked` 5; `explorer::tests` 55.
- **Clippy:** clean.
- **How:** the key for `RowFacts` is the tab's `view_key()`, the one the list and the grid pass as `facts_key`. `marked::init` follows the explorer, the facts and the settings; it asks `ask_all` only for a tab a pane shows and only while `wants_row_marks`; when `has_all`, one map is made and handed to `set_row_marks`. No timer, no loop.
- **Not done:** **no "before" number** (below, "Measured").
- **Unsure:** a tab that comes back on screen has its folder listed again, so its marks are read a second time.

### E1b — The list — done
- **Files:** `file_list/cells.rs`, `header.rs` (a test), `tests.rs` (the line `mod marks;`), `tests/marks.rs` (new), `tests/facts.rs` (one test changed); `layout.rs` untouched.
- **Tests:** `components::file_list` 134 passed, 1 ignored.
- **Clippy:** clean.
- **Not done as worded:** the order the wash and the selection's fill are painted in can't be read in a test: it checks that the wash is the line's first child, its bounds, and that its color is `label_wash`'s and not the accent. Tooltips can't be read either: the note mark and the Comment cell carry the comment as their description, and that is tested.
- **A test changed:** `dots_show_for_tagged_rows_and_change_after_tags_are_set` used Red, Blue and Green as tags; they are labels now, so it uses Plan, Docs and Misc.
- **Unsure:** how many chips fit in the Tags cell is **estimated from the letters' count** (nothing is measured on the UI thread); the labels are read from the settings once a row, not once a frame (a reference, nothing allocated); a label this PC lacks is washed in `tag_color(name)` at `label_wash`'s strength.

### E1c — The grid — partial, finished here
- **Files:** `file_grid/tile.rs`, `file_grid/tests.rs` (a module `marked_tiles` at its end).
- **Tests:** its 6 new tests pass; the parity test was changed here.
- **Clippy:** clean.
- **Not done:** nothing for the smallest tile sizes (the note sits with the dots, which had no rule either).
- **It met the access violation** in the photo's dots test, in its own file, and said it could not tell whether its change caused it. Putting `tile.rs` back did not stop it.

### E2a — The menu — done
- **Files:** `menus/item.rs`, `spec.rs`, `view.rs`, `smart.rs`, `data.rs`, and the tests `tests.rs`, `tests_view.rs`, `tests_smart.rs`, `tests_content.rs`, `tests_host.rs` (the last was not on the card).
- **Tests:** `components::menus` 199 passed; `components::command_bar` 72.
- **Clippy:** clean.
- **Tests changed**, beyond the two PLAN §10 names: a test that took the second label of a menu takes the third (the row of dots is under the strip); the menu key's test goes Up from Open to *No label* and counts 14 Downs from Cut to Open; `do:tags` is no longer among the keys every menu must have (it stays in `smart.rs`; no row carries it); the zip menu's test leaves the dots out when it compares with a menu built from a bare context.
- **Unsure:** with no labels in the settings there is no row of dots at all, not even *No label*; a dot that is on has a check mark and a ring in the popover's text color, and how that reads on yellow or gray is unjudged; the row's height in the menu's height cap is a guess (40).

### E2b — Ctrl+K, plain words, the key lists — partial, finished here
- **Files:** `palette/commands.rs`, `palette/tests.rs` (the line `mod marks;`), `palette/tests/marks.rs` (new), `address_bar/find.rs`, `address_bar/search.rs`, `quick_search/mod.rs`, `explorer/inside.rs`.
- **Tests:** `components::palette` 59 passed (6 new), `address_bar` 65 (2 new), `quick_search` 34 (none new), `explorer::inside` 14 (1 new).
- **Clippy:** clean.
- **How:** the commands are `MenuEvent`s (`SetLabel`, `EditTags`…) that the workspace already hands on, so they go through `marking` as the menu's do. They are offered when something is selected, and not in an archive, on a remote or in the Recycle Bin.
- **The four lists:** `address_bar/search.rs` reads `marks::KEYS` (a `!` before one read as the same key; a marks key with nothing after it is left out of the search as you type, **except `tag:` alone**, which an older test lists as finished); `explorer/inside.rs` never looks inside files for a marks term; `sidebar/rows.rs`'s `reads_files` was **left with its `tag:`** (a test there expects it; the index answers `tag:` now, so it may want to go); `quick_search/search.rs` keeps no list.
- **Not done:** no test that the open palette lists the labels from the real settings (its test hands them in); no test of quick search's plain words with labels.

### E3 — The dialogs — partial, finished here
- **Files:** `dialogs/tags.rs` (the dialog's part), `dialogs/comment.rs`.
- **Tests:** `dialogs::comment` and `dialogs::tags` 36 passed.
- **Clippy:** clean.
- **How:** Edit tags… takes its tags from a source (`marks_all` once when it opens; the recent ones when that answers nothing) and those of this folder from `marks_among` over the tab's first 2,000 rows; Up and Down walk the suggestions, Enter takes the one lit (else saves), Tab takes it or the first. `TagsEdit::new` and `TagsDialog::new` keep their signatures; `TagsEdit::with_source` is new.
- **Unsure:**
  - `Ctrl+Enter` in Edit comment… saves through the box's own second Enter, which **puts a line break in first**; it is taken out before saving, but when a save is refused (over 64 KB) the break stays in the box;
  - the dialog's own key handler for `Ctrl+Enter` never fired in the tests, so what a real window does is unchecked;
  - one item's title is `Comment of “name”` (the card said `Comment`);
  - the suggestions wrap as chips do rather than standing as rows;
  - *Several comments* is tested through a flag beside the call, not as text (the box gives no way to read its placeholder);
  - the source that asks the index is never run in a test.

### E4 — The Marks block — done
- **Files:** `preview/marks.rs` (new), marked places in `preview/frame.rs` (a field, the block made in `EntryView::new`, drawn right before the details, and an accessor for tests that the card didn't ask for) and `preview/pane.rs` (a field, made in `Shown::Many` when the items can be written, drawn at the deck's end).
- **Tests:** `components::preview::marks` 19 passed; `components::preview` 292 passed, 6 ignored.
- **Clippy:** clean.
- **Quick Look's keys:** three keys are bound in `MarksBox > Input` (Space does nothing there, `Ctrl+Enter` saves, Escape puts back); a test types a space in the comment and Quick Look stays open.
- **Unsure:**
  - **a 0.16 color tag drawn as the label can't be taken off from the block**: its dot asks `Change::Label(None)`, which changes nothing for such an item (`marking::set_label` takes the tag off instead; the block doesn't go through it);
  - with several items, *Kept beside the file* speaks for the first marked one only;
  - a comment typed and not yet saved is lost if the block goes away without the focus leaving it first (untested).

### E5 — Marking — done
- **Files:** `marking.rs`.
- **Tests:** `marking::` 10 passed.
- **Clippy:** clean for its file.
- **How:** the selection is the active tab's selected rows in the rows' order (as `Workspace::selection`); the focused item alone is not a selection. Marks come from `marks_among`; an item not read counts as unmarked.
- **Unsure:** taking a label off a selection that mixes labels and 0.16 color tags is two changes, so **two Undo steps**; Paste marks is up to **three** (tags, label, comment), as the card allowed.

## Every new string

- **The list and the grid:** `label {name}`, `tags {list}`, `comment: {first}`, `+{n}`, `Comment` (the note mark's name).
- **The menu:** `Label` (the row of dots' name), `No label`, `Marks`, `Tags…`, `Comment…`, `Copy marks`, `Paste marks`, `Clear marks`, `Nothing here is marked`.
- **The palette:** `Label: {name}`, `No label`, `Edit tags…`, `Edit comment…`, `Copy marks`, `Paste marks`, `Clear marks`.
- **Marking:** `“{name}” has no marks to copy`, `There are no marks on the clipboard`, `Copy an item's marks first (More → Marks → Copy marks).`
- **Edit tags…:** `Suggested tags`.
- **Edit comment…:** `Comment of “{name}”`, `Comment of {n}`, `Several comments`, `{used} of 64 KB`, `A comment can be 64 KB at most`, `Couldn't read the comment: {reason}`.
- **The Marks block:** `Add a tag`, `Add a comment`, `Comment`, `Several comments`, `Remove tag {tag}`, `A comment can be 64 KB at most`, `Kept beside the file`, `Kept on this PC`.
- **Group by:** `Label` (glint-model's `GROUP_BYS`).

## Measured

All on this PC, in **debug or test builds**, not release.

| What | Result |
| :--- | :--- |
| **E1a**, 10,000 rows built (medians of 11, test build, names that are hard to sort) | the default spec 0.28 ms; a name sort descending 14.75 ms; a size sort 15.02 ms; **a sort by label with every row marked 317.88 ms** |
| **E1a, before** | **not measured.** The agent could not run the old code (no stash). Without marks in the spec `build_rows` makes the calls it made, after one check for `None`; that is reasoning, not a number. To have it: `cargo test -p glint-ui explorer::rows::tests::building_ten -- --ignored --nocapture` on `6ef80cf` and on this tree. |
| **B3**, a copy when nothing is marked | 2,000 `carry_one` calls between two NTFS folders: 6.7 ms, about 3 µs a file. From a test the agent ran and removed, not from the queue. |
| **B3**, `caps::has_streams` | **about 1.2 ms a call** on this PC, 2.4 s for 2,000 pairs, which is why `carry.rs` keeps its own answer a folder for ten seconds. B1's cache in `caps.rs` was meant to make this free: worth a look. |
| **C4**, one read of an item with no marks | 10,000 reads by reference: 540 ms, about 54 µs each (one `OpenFileById` for attributes, one listing, one close). |
| **C4**, 3 MB of the file's own data before its marks | one read: 421 µs (the data is sought over). |

The sort by label is the number to look at: 318 ms for 10,000 rows in a test build. A5 left a note that what is shown is worked out again at every comparison; making each row's key once would take most of it away.

## Left between the cards

- **`explorer/ops.rs` does not call `carry`.** So a copy, a move or a rename done through Windows' engine (not the copy queue) leaves marks kept beside the files or on this PC where they were. The explorer's tests run on a fake disk, so the call belongs behind its `Source`, not straight to glint-core: that is why it was not put in here. B3 says what to call: `carry(pairs, false)` for a copy Windows' dialog did, `carry(pairs, true)` for a move and for a rename, nothing for a delete to the Recycle Bin, and for a restore `carry([(bin, restored)], true)` when the bin's path is known. Folders too: the queue carries files only.
- **The block's label dot and a 0.16 color tag** (E4, above): it should go through `marking::set_label`, or the hub should do what that does.
- **`sidebar/rows.rs`'s `reads_files`** still says `tag:` reads the files.
- **Undo steps**: Paste marks up to three, a mixed label-off two. One step needs the hub to take a change an item (`Backend::put_marks` is there; no hub function calls it).
- **The sort by label's cost**, above.
- **Kept by hand in two places**: the three strings `label {name}`, `tags {list}`, `comment: {first}` are built in `cells.rs` and in `tile.rs`; the note mark too.
- **Still wave 4's**: `marks_all` for the sidebar, `client::marks_in` for a sort from the index, `SearchRequest.marks`.

## What a card's wording got wrong, and what to look at

- **E1b and E1c, tests of what is painted.** Both cards asked for a test that the wash is drawn under the selection's fill and for the tooltip's text. The test harness shows neither. The agents tested the order of the elements and the description.
- **E1b, E1c and the old tests of dots.** Two tests outside or inside the cards' files used color names as tags. The cards said "the tests the files have pass unchanged" and should have said that a color tag is no longer a dot.
- **E2a, "the only tests changed are the ones that listed the Tags submenu".** A row of dots under the strip moves every test that counts rows or walks the menu with the arrows. Five test files changed, one not on the card.
- **E2b, `Inputs`.** The card had the palette's commands read the labels; `Inputs` is built in `palette/mod.rs`, which was nobody's. The agent added functions that take the names and left the wiring, rightly.
- **E3, `opens_from`.** The card said to say what `dialogs/mod.rs` needs if it can't be done; it couldn't, so the card came back partial by design.
- **E3, the title.** PLAN doesn't word Edit comment…'s title; the card's `Comment` and the agent's `Comment of “name”` differ. The agent's is the one in the tree.
- **E4, the color tag.** The card sent every change through `set_marks` and never said what taking a label off a 0.16 file means. E5's card did.
- **B3, where the copies are carried.** The card named the three places in `queue.rs` where a file is in place; a folder is never "in place" there.
- **C4, "by a path".** Nothing in the card's wording was wrong; the agent went one further and refuses a sequence of 0.
- **Line ends**, as before: the files the agents wrote have LF.
- **While they ran:** builds of one agent met another's half-written file several times (`far.rs`, `comment.rs`, `preview/marks.rs`, the palette's tests); each waited and built again. Nobody changed a file that wasn't theirs, but for the test files named above.

## Not run, and not seen

- **Nothing was seen on a screen.** The wash's strength against hover and selection, the pill, the chips, the note mark, the menu's dots, both dialogs, the block, right to left: all untested by eye, in either theme.
- The end-to-end checks (`tests\e2e\run.ps1`) and the installed Glint: they take the real mouse, keyboard and profile.
- `cargo fmt`: by the rules.
- Nothing of waves 4 and 5.
