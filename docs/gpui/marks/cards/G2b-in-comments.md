# Card G2b — *In comments*: a plain word also finds what has it in its comment or tags

Wave 4 · crates `glint-core` and `glint-ui` · the hardest card of the wave: an honest "partial" is fine

**You own:** in glint-core, `crates/glint-core/src/find.rs`; in glint-ui, `crates/glint-ui/src/explorer/find.rs`, `explorer/find_types.rs`, `explorer/find_tests.rs`, `explorer/rows.rs` **from `with_hits` to the end of what places a search's parts** (about 676 to 800: not `sort_order`, not `build_rows`' head), and **marked places**: in `explorer/source.rs` `found_from` (937) and where it builds core's `FindOptions` (661); in `components/file_list/cells.rs` the header of a search's part (272 to 300). A new `explorer/in_comments.rs` is yours if you want one (its `mod` line in `explorer/mod.rs` is the one line you may add there). **Every other place that builds core's `FindOptions`** (`mcp.rs:366`, `quick_search/source.rs:162`) or matches core's `FindEvent` (`quick_search/source.rs:92`): you may add the one field or the one arm that keeps it compiling and doing what it did, and you list each in your report.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M5), §4 (F15), §6.4 (`proto.rs`, and "The service never sends a comment's text"), and under §9's table "Wave 2, as it landed" (a search with `marks`). Then the code: shadui-index's `proto.rs` (`SearchRequest.marks`, `HIT_BY_COMMENT`, `HIT_BY_TAG`, `Hit` and how its flags are read: `decode_hits` about 337); glint-core's `find.rs` (`FindOptions` 23, `FindEvent` 39, `Out` about 300, `search_index` 361 where the request says `marks: false` and each hit becomes an `Entry`, `same_service` 408); glint-ui's `explorer/inside.rs` (the head's notes from line 1, `inside_words`: **`Inside files` is the pattern you copy**), `explorer/find.rs` (`run_find_with` 282, `Inside` at 353, `found` 547, `add_pending` 733), `explorer/rows.rs` (`with_hits` and the `Names` / `Inside files` sections about 700 to 760, `Sections`, `HitLine`), `file_list/cells.rs:272`; `components/row_facts.rs` and `dialogs/tags.rs`' `Backend::read_marks` (how the app reads an item's marks off the UI thread); `explorer/inside_tests.rs` (how the parts are tested: `r.lines(cx)`).
**Check with:** `cargo test -p glint-core find::`, `cargo test -p glint-ui explorer::find`, `cargo test -p glint-ui explorer::rows`, `cargo test -p glint-ui explorer::inside`, `cargo test -p glint-ui components::file_list` once at the end, and `cargo clippy -p glint-core -p glint-ui --all-targets -- -D warnings`.

## Core: find.rs

- `FindOptions` gains `pub marks: bool`: plain words also look in tags and comments. `search_index` sends it as `SearchRequest.marks` **only when `same_service()`** (an older service doesn't know it).
- A hit's two bits reach the app: `FindEvent` gains `Marked(Vec<(String, u8)>)`, the paths of a batch found by a comment or a tag with their bits (`HIT_BY_COMMENT`, `HIT_BY_TAG`), sent right before the `Found` batch that holds them. A hit found by its name too is as the service flags it. fd's drives send none (the open folder and fd are not this card).
- **Nothing else changes for a search with `marks: false`**: the same events, in the same order.

## The app

- The explorer's searches ask with `marks: true` whenever the search has plain words (what `inside_words` calls words), **as it is typed too**: the index answers from memory. A search with no plain word asks without. Quick search stays `false`.
- A run keeps, beside what it keeps for `Inside files`, **which of its rows were found by a comment or a tag** and, for each, **the matching line**.
- **The service never sends a comment's text.** The app reads it: the marks of the flagged paths through the tags hub's backend (`read_marks`, off the UI thread, in batches, only for rows found this way, at most 2,000 a search), and from them the line to show: for a comment, **the first line of it that holds one of the search's words** (any case), cut to what a hit line shows; for a tag alone, `t!("tags: {list}")`. Until an item's marks are read its row shows no line (M4); if they no longer hold the word (changed since), the row stays without a line.
- **Listed under its own heading**: `t!("In comments")`, with how many, after `Names` and before `Inside files`; each row with its matching line under it, **as `Inside files` lists a file's lines**. A row found by its name too stays under `Names`. Grouped by something else, there are no such parts and the line still follows its row (as `Inside files` does: `inside_tests.rs:397`).
- The header in `cells.rs` words the new part as it words the two it has.

## Tests

glint-core `find::`:
1. `FindOptions { marks: false }` builds the request it built; with `true` the request says `marks` (test what builds the request: split it out of `search_index` if you must).
2. A batch of hits where one has `HIT_BY_COMMENT` sends `Marked` with that path and its bits, then `Found` with all of them; a batch with no such hit sends only `Found`.

glint-ui (the explorer's fake source; a fake marks backend):
3. A search for `lacp` that finds `notes.txt` by name and `bond0.conf` by its comment lists `# Names (1)`, `notes.txt`, `# In comments (1)`, `bond0.conf`, and under it the comment's line that holds `lacp` (its second line, not its first).
4. An item found by a tag alone shows `tags: bond, lacp`.
5. Found by comment and by what is inside files: the three parts in order, Names, In comments, Inside files.
6. Until the marks are read the row has no line; once read it has; marks that no longer hold the word leave it without one.
7. Only the flagged paths are read, in one call a batch, never from the UI thread, and a search with no flagged hit reads nothing.
8. Grouped by type there are no parts and the line follows its row.
9. A comment in Arabic matches and its line is shown.
10. The tests of `find`, `inside`, `rows` and the list pass unchanged (but any that lists every part's header: change it, and say so).

## Done when

The tests pass, clippy is clean for both crates, a search with no plain word asks what it asked, and `sort_order` and the rest of `rows.rs` above `with_hits` are untouched (card E6 is not there either; nobody else edits this file). Say in your report every new string, every place outside your files you added a field or an arm to, what is partial, and that nobody has seen it on a screen.
