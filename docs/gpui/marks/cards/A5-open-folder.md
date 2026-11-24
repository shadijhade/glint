# Card A5 — The filter in the open folder; sort and group by a mark; plain words

Wave 2 · crate `glint-model` · pure logic, no I/O

**You own:** `crates/glint-model/src/marks/wanted.rs`, `crates/glint-model/src/entries.rs`, `crates/glint-model/src/groups.rs` and `crates/glint-model/src/plain.rs` (tests at the bottom of each). Nothing in glint-ui or glint-core: every function other crates call today keeps its signature, and what is new goes **beside** it.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M5), F4, F12, F13 and F16 in §4, §5.5, §5.6, §6.1, §6.5, §8.1, and "Wave 1, as it landed" under §9's table. Then [card A4](A4-filter.md), all of it: your filter is the second copy of the one it describes. Then `crates/shadui-index/src/query.rs`: `MarksFilter::accepts` (690) and the marks part of `parse` (881 on), which are the rules to mirror; `tests/fixtures/marks-filter.txt`, whose first lines say what was decided for both copies; `marks/labels.rs` (`label_of`, `shown`). Then your files: `wanted.rs` whole; in `entries.rs` `MarksOf` and `MarkReads` (340), `Reads` (417), `entry_order_by` (428; the three sorts that do nothing are at 483), `terms` (497), `split_prop` and `has_prop_filter` (749, 762), `filter_test` (869), `matcher` and `matcher_at` (1023, 1032); in `groups.rs` `group_in` (197; `GroupBy::Label` at 253), `group_of` (259) and `group_order` (336); in `plain.rs` `FILLER` (114) and `plain_query` (274 to its end).
**Check with:** `cargo test -p glint-model` (the whole crate: its tests must pass unchanged) and `cargo clippy -p glint-model --all-targets -- -D warnings`.

**Landed for you** (it compiles and does nothing yet): `entries::MarksOf`, `entries::MarkReads { of, labels }`, and the field `Reads::marks: Option<MarkReads>`, which `entry_order` and glint-ui's `explorer/rows.rs` fill with `None`. Don't change `Reads` again: glint-ui builds it.

## wanted.rs: `Wanted::parse` and `Wanted::accepts`

The open folder's copy of the marks filter. It must answer every case of the fixture as `MarksFilter` does; where the two would differ, that is a bug here.

`parse(terms)` takes a search's terms as `entries::terms` splits them (quotes already off) and returns the filter and the terms that are not about marks, in their order and untouched.

- A term is about marks when the text before its first `:`, with one leading `!` taken off, is one of `KEYS` in any ASCII case. The `!` sends it to `no`, else to `yes`. A `!` before any other key is not a filter: the term stays among the rest.
- Every name, word and phrase is kept folded (`to_lowercase()`), so `accepts` folds only the item's side.

| Key | The value |
| :--- | :--- |
| `label`, `lbl` | split on `;`, each trimmed, quotes (`"`) trimmed off, empty ones dropped: one group of `labels` |
| `tag`, `tags` | split on `;` or `,`, the same way: one group of `tags` |
| `comment`, `cmt` | the whole value, trimmed: one of `words` |
| `has` | `label`; `tag` or `tags`; `comment`; `mark` or `marks` |

- **A term that doesn't read right** (no value left, or a `has:` that is none of those): the service answers with an error; here, as every filter of `entries.rs` does, it matches nothing. Push an **empty group** onto `yes.labels` (whatever the term's side): no item satisfies an empty group. Say so in `parse`'s comment.

`accepts(marks)`: an empty filter accepts everything. Otherwise, with the label trimmed and folded, each tag trimmed and folded (empty ones dropped) and the comment folded:

- every group of `yes.labels` holds the label (an item with no label fails them all);
- every group of `yes.tags` holds one of the tags **or the label**;
- every one of `yes.words` is somewhere in the comment;
- every `yes.has` holds: `Label` a label, `Tag` at least one tag, `Comment` a comment, `Mark` any of the three;
- and not one term of `no` holds, each read the same way.

A label and a tag match whole; only a comment matches inside. `has:label` is the label alone (a 0.16 color tag is a tag until it is moved).

## entries.rs: the filter of the open folder (G2)

Add, beside `matcher_at` (which keeps its signature, as `matcher` does, and calls this with `None`):

```rust
/// `matcher_at`, with the rows' marks for `label:`, `tag:`, `comment:` and `has:` (and `!` before them).
pub fn matcher_marks<'a, 'f: 'a>(
    query: &str,
    git: Option<&'a GitMarks>,
    facts: Option<&'a FactsOf<'f>>,
    marks: Option<&'a MarksOf<'f>>,
    now: i64,
) -> Option<EntryTest<'a>>;

/// Whether a search holds a marks filter, which needs every row's marks read to apply (`RowFacts::ask_all`).
pub fn has_marks_filter(query: &str) -> bool;
```

- The terms are split as now (`terms(&query.to_lowercase())`); `Wanted::parse` takes the marks terms out, and the rest go through the tests there are, unchanged.
- With a filter that isn't empty: **with** a reader, an entry passes when `wanted.accepts` its marks, and an entry the reader has no marks for yet (`None`) is asked as one with none (`Marks::default()`: so `!has:label` keeps it, `label:red` doesn't). **Without** a reader the marks terms accept every entry: they are left to the search, as a property filter without `facts` is (`filter_test`, 869).
- A query of only marks terms is a filter (`Some`), and an empty query is `None` as now.
- `has_marks_filter` is true when `Wanted::parse` of the query's terms isn't empty.

Today `tag:work` typed over an open folder becomes a name and shows nothing; after this it shows what is tagged.

## entries.rs: sorting by a mark (F4)

In `entry_order_by`, `SortKey::Label`, `Tags` and `Comment` read `reads.marks` (the destructuring at the top ignores it today). Without it (`None`) they stay `Ordering::Equal`, as now. With it, each entry's marks are `shown(labels, marks)` of what the reader gives, an entry not read yet being one with no marks:

| Key | Ascending |
| :--- | :--- |
| `Label` | labels this PC has, in the labels' order (`Shown::place`); then labels it doesn't have, by name (`compare_base_numeric`); then no label |
| `Tags` | by the tags drawn (`Shown::tags`) joined with `, `, compared with `compare_base_numeric`; no tags last |
| `Comment` | by the comment's first line, `compare_base_numeric`; no comment last |

Descending reverses all of it, as every other key does, and ties fall to the name as they do now.

## groups.rs: grouping by label (F4)

Add beside `group_order` (which keeps its signature and calls this with `None`):

```rust
/// `group_order`, with the rows' marks for `GroupBy::Label`.
pub fn group_order_marked<E: std::borrow::Borrow<FileEntry>>(
    rows: &[E],
    by: GroupBy,
    now: i64,
    size_of: Option<&SizeOf>,
    git: Option<&GitMarks>,
    marks: Option<MarkReads>,
) -> GroupedOrder;
```

`group_in` (private) takes the marks too; `group_of` and `group_rows` keep their signatures and pass `None`. For `GroupBy::Label` with marks, from `shown(labels, marks)`: a label this PC has is the group ranked by its place, named by the label's name now; a label it doesn't have is ranked right after them (`labels.len()`) and named as the file says it (such groups order by name, as groups of one rank do); none, or an entry not read yet, is the last group, **No label**. Without marks `GroupBy::Label` stays one nameless group, as now.

## plain.rs: plain words (F16)

Add beside `plain_query` (which keeps its signature and calls this with no labels):

```rust
/// `plain_query`, knowing this PC's labels by name.
pub fn plain_query_with(text: &str, labels: &[&str]) -> Option<Plain>;
```

New steps in the loop, each with its filter and its chip. The first three come **before** the step for `with` / `on` (477); the last comes right before a word is taken as a name (the last lines of the loop), so every word that means something today still means it.

| Words | Filter | Chip |
| :--- | :--- | :--- |
| `tagged X` | `tag:x` (`X` folded) | `tagged X` (as typed) |
| `labeled X`, `labelled X` | `label:x`; `X` is one word, or the words of a known label that follow (`labeled in review` with a label *In review*: `label:"in review"`) | `labeled Name`: the label's own spelling when this PC has it, else as typed |
| `with comments`, `with comment`, `with a comment`, `commented` | `has:comment` | `with a comment` |
| `with tags`, `with tag`, `with a tag` | `has:tag` | `with tags` |
| `with labels`, `with label`, `with a label` | `has:label` | `with a label` |
| a known label's name alone (its words in a row, any case) | `label:name` (quoted when it has a space) | `labeled Name` |

`Found::add` keeps one filter of each key, as now. With no labels the last row never fires, so `plain_query` answers what it did for every text that has none of the new words.

## Tests (each a `#[test]`, named as a sentence)

wanted.rs:
1. **The fixture**, exactly as card A4's test 1 reads it (`include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks-filter.txt"))`; five fields split on `|`, trimmed; `#` lines and empty ones skipped): `Wanted::parse(&terms(query))` leaves no other term, and `accepts` of the marks (label `Some` unless empty; tags split on commas; a written `\n` in the comment a real one) is what the last field says. The line's number is in the assert's message. At least 60 cases ran.
2. `parse`: `!ext:pdf` and `report` come back among the rest, in order, untouched; `LABEL:Red` reads as `label:red`; `lbl:`, `tags:` and `cmt:` are the same keys; `label:`, `!comment:` and `has:size` each give a filter that accepts nothing, whatever the marks; `label:"in review";red` is one group of two.
3. `accepts` holds for an empty filter whatever the marks, and doesn't panic on empty strings, a comment of 70,000 bytes, or names in Arabic.

entries.rs:
4. `matcher_marks` with a reader: `tag:work` keeps the entries tagged *Work* and the one labeled *Work*; `label:red;blue` either; `comment:lacp` inside a comment; `!has:label` keeps the unlabeled **and the entries not read yet**; `label:red` drops those; `report tag:work` needs both; a query of only `has:mark` is `Some`.
5. Without a reader, `tag:work` accepts every entry and `report tag:work` is the name alone; `matcher` and `matcher_at` answer as they did for every query without a marks key.
6. `has_marks_filter`: true for `holiday TAG:work`, `!has:comment`, `lbl:red`; false for `holiday ext:jpg`, `!ext:pdf`, `label` (no colon).
7. Sorting by `Label` with the seven labels: Red, Orange, an unknown *Zeta*, an unknown *Alpha*, none, come out Red, Orange, Alpha, Zeta, none; a file with no label and the tag `red` sorts as Red (`shown`); descending is the reverse; an entry not read sorts with none.
8. Sorting by `Tags` and by `Comment`: by the text, none last; a comment sorts by its first line; a color tag drawn as the label is not among the tags compared.
9. Without `Reads::marks` the three keys keep the name's order (as today).

groups.rs:
10. `group_order_marked` by `Label`: groups in the labels' order, then unknown labels by name, then **No label**; the headers' labels are the labels' names now (after a rename, a file saying `Red` is in *Important*); an entry not read is in **No label**; rows keep their order inside a group.
11. `group_order` by `Label` is one nameless group, and `group_order_marked` with `None` equals `group_order` for every `GroupBy`.

plain.rs:
12. With the labels `Important`, `Pending`, `In review`: `important pdfs` is `label:important ext:pdf` with the chips `labeled Important`, `PDF files`; `tagged netplan` is `tag:netplan`; `with comments` is `has:comment`; `labeled pending from last week` is `label:pending dm:7d`; `labeled in review` is `label:"in review"`; `labeled mauve` is `label:mauve` with the chip `labeled mauve`; `photos with tags` ends with ` has:tag`; `budget tagged work` is `budget tag:work`.
13. With no labels: `important pdfs` is `important ext:pdf` as today, and every assert of `plain_words_read_as_filters` holds through `plain_query_with(text, &[])` too. `photos with pixel` still ends with ` camera:pixel`.
14. Nothing panics on `tagged`, `labeled` or `with` as the last word, or on a label whose name is empty.

And every test the crate has passes unchanged.

## Done when

The tests pass, clippy is clean, nothing in the four files reads the disk or the clock beyond what they read today, and `matcher`, `matcher_at`, `sort_entries`, `entry_order`, `entry_order_by`, `group_order`, `group_rows`, `group_of` and `plain_query` keep their signatures. Say in your report every new `pub` name, and any case where you found `MarksFilter` and the fixture disagree (don't change the fixture).
