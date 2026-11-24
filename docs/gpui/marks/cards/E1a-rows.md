# Card E1a — The rows' marks reach the rows: filter, sort and group by a mark

Wave 3 · crate `glint-ui` · models, nothing drawn

**You own:** `crates/glint-ui/src/explorer/rows.rs`, `crates/glint-ui/src/explorer/marked.rs` (new: a stub is there), and **marked places** in `crates/glint-ui/src/explorer/model.rs`: `row_spec`'s line `marked: None` (808), one new method beside `set_git_marks` (823), and one field of the explorer or of `Tab` (`explorer/tab.rs`, that one field) to keep a tab's marks in. Cards E1b and E1c draw; you draw nothing.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M5), F4, F12 and F13 in §4, §6.1, §6.6, §8.1, and "Wave 2, as it landed" under §9's table. Then [card A5](A5-open-folder.md) (what you call) and [card D1](D1-hub.md) (`RowFacts`). Then the code: `explorer/rows.rs` `RowMarks`, `RowFileMarks` (63) and `RowSpec`, `reads_git` (440), `build_rows` (448: `matcher` at 452, `group_order` at 565), `sort_order` and its `Reads` (630), the test `sorting_twenty_thousand_rows…` (1267: how a cost is measured here); `explorer/model.rs` `row_spec` (757), `set_git_marks` (823) and `rebuild_rows` (850); `components/git/mod.rs` lines 30 to 60 (how Git's marks are handed to the explorer: the pattern you follow); `components/row_facts.rs` `marks_of`, `has_all`, `ask_all` (578, 584, 593) and its tests' `fake::MarksBackend` (879); glint-model's `entries::{MarksOf, MarkReads, matcher_marks, has_marks_filter}`, `groups::group_order_marked`, `columns::MARK_SORTS`.
**Check with:** `cargo test -p glint-ui explorer::rows`, `cargo test -p glint-ui explorer::marked`, `cargo test -p glint-ui explorer::tests` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (This crate builds slowly and other agents' builds wait on the same lock: be patient, and run the whole crate's tests at most once.)

**Landed for you:** `RowFileMarks { marks: Arc<HashMap<String, Marks>>, labels: Arc<[Label]> }` and the field `RowSpec::marked` (`None` by default and in `row_spec`); `explorer::marked::init(cx)`, a stub that start-up already calls after the explorer and the rows' facts are there; the Label, Tags and Comment columns and Group by → Label are listed now (`COLUMN_ORDER`, `GROUP_BYS`), so a person can choose them.

## rows.rs: `build_rows` reads `spec.marked`

With `spec.marked` set, a reader over its map (an entry's marks by `entry.path`; an entry not in the map has none known) and its labels go to:

- the filter: `matcher_marks(&spec.filter, git, None, Some(&reader), now)` in place of `matcher`;
- the sort: `Reads { marks: Some(MarkReads { of, labels }) }` in `sort_order`;
- the groups: `group_order_marked(.., Some(MarkReads { .. }))` in place of `group_order`.

Without it everything is called as today, so **a spec with no marks builds the rows it built, at the cost it had** (M5). Add `pub fn reads_marks(filter: &str, sort: Sort, group: GroupBy) -> bool` beside `reads_git`: true when `has_marks_filter(filter)`, or the sort's key is one of `MARK_SORTS`, or the group is `GroupBy::Label`.

## model.rs: a tab's marks

Beside `set_git_marks`, **`pub fn set_row_marks(&mut self, id: TabId, marks: Option<RowFileMarks>, cx)`**: kept for the tab; when they are the same as before nothing happens; otherwise the tab's rows are rebuilt (`rebuild_rows`). `row_spec` gives them as `marked` **only while `reads_marks` holds for the tab's filter, sort and group** (as `git` is given only while something reads it), so a change of marks rebuilds only the rows that read them. Also **`pub fn wants_row_marks(&self, id: TabId) -> bool`**: whether `reads_marks` holds for the tab now.

## marked.rs: who reads, and when

`init(cx)` follows the explorer and `RowFacts` from then on (subscriptions kept in a global of this file):

- For the tab each pane shows (and only for a tab a pane shows: "a view that is not on screen does nothing"), when `wants_row_marks`: ask `RowFacts::ask_all(view_key, entries, cx)` with the key the views use for that tab (the tab's `view_key()`, the one `FileList` passes as its `facts_key`: find it and use the same), and **when `has_all` says every row is read**, make one map of `marks_of` for the tab's entries, with `state::settings(cx).labels`, and hand it to `set_row_marks`. The rows are then built once.
- While `has_all` is false the tab keeps what it had (the rows show as they are, the marks terms left to the search), and this is asked again whenever `RowFacts` tells its observers: no timer, no loop of your own.
- When the tab stops wanting marks, `set_row_marks(id, None)`: the map is let go.
- A mark changed (`TagsChanged` makes `has_all` false until it is read again), a folder listed again, the labels changed in settings: the same path brings a new map, and the rows are built again once.
- Nothing here blocks: every call is a read of what is in memory, or an ask that returns at once.

## Measure (M5), and put both numbers in your report

A folder of 10,000 rows built by `build_rows` with the default spec, a name sort descending and a size sort: **before** your change and **after** it (the medians of 11 runs each, in a test written as `sorting_twenty_thousand_rows_costs_less_with_name_keys` is, `#[ignore]`d if it is slow, run by hand with `--ignored --nocapture` in a release or a test build: say which). With no marks in the spec the two must be the same within the noise. Then the same folder sorted by label with every row's marks in the map: say what that costs.

## Tests (each a `#[test]` or `#[gpui_kit::test]`, named as a sentence; STANDARDS §5.1 "Tests")

rows.rs:
1. With marks: `tag:work` keeps the tagged rows and the one labeled *Work*; `!has:label` keeps the unlabeled; `report tag:work` needs both; without marks in the spec `tag:work` keeps every row (as A5 has it).
2. Sorted by label: this PC's labels in their order, then unknown ones by name, then none; descending the reverse; folders first still holds.
3. Grouped by label: headers in the labels' order with **No label** last, counts right, rows in their order inside a group.
4. Two specs with the very same marks are equal and with other maps are not (as `specs_with_the_same_marks…` has it for Git).
5. `reads_marks`: true for `x label:red`, a sort by `Tags`, a group by `Label`; false for `x ext:pdf` with a name sort and no group.
6. Every test the file has passes unchanged.

model.rs / marked.rs (the explorer's fakes, `explorer::fake`, and `row_facts`'s `fake::MarksBackend`):
7. A tab sorted by label asks for every row's marks once, and when they are in its rows are in the labels' order; a tab sorted by name asks for nothing and `RowSpec::marked` is `None`.
8. Typing `tag:work` in the filter of a folder: before the marks are in, every row shows; once they are, only the tagged; the filter cleared, the map is let go.
9. A mark changed and said (`TagsChanged`) brings the rows up to date once read again; a folder listed again too.
10. `set_row_marks` with the same map twice rebuilds nothing (count `ExplorerEvent`s, or compare the rows' identity).
11. A tab no pane shows is never asked about.

## Done when

The tests pass, clippy is clean, a tab that shows, sorts by and filters on no mark reads nothing and builds its rows as before (the numbers say so), and nothing waits on the UI thread. Say in your report the key you used for `RowFacts` and where you found it, what you kept the tab's marks in, and anything the workspace must still call.
