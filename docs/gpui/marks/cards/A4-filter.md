# Card A4 — The filter in the service: `label:`, `comment:`, `has:`, `!`, the nearest names

Wave 1 · crate `shadui-index` · pure logic, no I/O

**You own:** `crates/shadui-index/src/query.rs` (its tests at the bottom of the file). `tree.rs` is card C1's: it goes on asking `Filters::tags` until the coordinator puts your filter there, so don't touch it.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) F12 to F14 in §4, §6.4, §6.5. Then, in your file: `terms` and `fold` (lines 12 to 36), `edits` (156), `Filters` and `parse` (630 to 776), `without_tags` (896). Then `tests/fixtures/marks-filter.txt`, whose first lines say what was decided for both copies of the filter.
**Check with:** `cargo test -p shadui-index query::` and `cargo clippy -p shadui-index --all-targets -- -D warnings`.

## What to build

The service's copy of the marks filter (glint-model has the other, card A5; the fixture keeps them alike). Add these beside `Filters`, and one field to it:

```rust
/// What `has:` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Has { Label, Tag, Comment, Mark }

/// One side of a marks filter: every term of it must hold.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarkTerms {
    /// `label:`: groups of names, any one of which will do.
    pub labels: Vec<Vec<String>>,
    /// `tag:`: groups the same way; a label answers too.
    pub tags: Vec<Vec<String>>,
    /// `comment:`: words or phrases, every one in the comment.
    pub words: Vec<String>,
    pub has: Vec<Has>,
}

/// `label:`, `tag:`, `comment:` and `has:` (`yes`), and the same after `!` (`no`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarksFilter { pub yes: MarkTerms, pub no: MarkTerms }

impl MarksFilter {
    pub fn is_empty(&self) -> bool;
    /// Whether an item with these marks is wanted: `label` and `comment` are "" for none, `tags` one a line.
    pub fn accepts(&self, label: &str, tags: &str, comment: &str) -> bool;
    /// This filter with the names nothing has replaced by the nearest that something has; None when none is.
    pub fn nearest(&self, labels: &[&str], tags: &[&str]) -> Option<MarksFilter>;
}

/// The query without its marks terms, for what can't read marks (fd).
pub fn without_marks(query: &str) -> String;

pub struct Filters { /* as now */ pub marks: MarksFilter }
```

Every name, word and phrase in a `MarksFilter` is kept folded (`fold`), so `accepts` folds only the item's side.

### In `parse`

The marks keys are `label`, `lbl`, `tag`, `tags`, `comment`, `cmt` and `has`, in any case. A `!` right before one of them means not, and the term goes to `marks.no`; without it, to `marks.yes`. A `!` before any other key is not a filter: the term stays a name term, as it is today.

| Key | The value | When it has nothing |
| :--- | :--- | :--- |
| `label`, `lbl` | split on `;`, each trimmed and with its quotes off: one group | `Err("“label:”: give a label, like label:important or label:red;orange")` |
| `tag`, `tags` | split on `;` or `,`, as today: one group | today's error |
| `comment`, `cmt` | the whole value, trimmed: one word or phrase | `Err("“comment:”: give a word, like comment:invoice or comment:\"second draft\"")` |
| `has` | `label`; `tag` or `tags`; `comment`; `mark` or `marks` | `Err(format!("“{term}”: has: takes label, tag, comment or mark"))`, for any other value too |

`tag:` without `!` also goes on filling `Filters::tags` exactly as it does now (card C1's tree and glint-core read it until they move to `marks`). `!tag:` fills only `marks.no`. `Filters::accepts` stays about names, sizes and dates: say in its comment that marks are asked of `marks`, with the record's.

`terms` has taken the quotes off already, so `comment:"rate fast"` arrives as the value `rate fast`: a value with a space in it is a phrase, and nothing more is needed for one.

### `accepts(label, tags, comment)`

An empty filter accepts everything. Otherwise, with the item's label, each of its tags and its comment folded:

- every group of `yes.labels` holds the label (an item with no label fails them all);
- every group of `yes.tags` holds one of the tags, **or the label**;
- every one of `yes.words` is somewhere in the comment;
- every `yes.has` holds: `Label` a label, `Tag` at least one tag, `Comment` a comment, `Mark` any of the three;
- and not one term of `no` holds, each read the same way (`!tag:red` refuses an item labeled Red).

A label and a tag match whole; only a comment matches inside.

### `nearest(labels, tags)`

For typos (F14). `labels` and `tags` are every name the index has. For each group of `yes.labels`: when none of its names is a known label (folded), each of its names is replaced by the known labels nearest to it, and the group is those; when none is near, the group stays. The same for each group of `yes.tags`, against the tags **and** the labels. `no` is left alone. `Some(filter)` when any group changed, else `None`.

*Near* is the rule names have (`edits(letters)`: none under four letters, one from four, two from eight), over the **whole** name: the fewest of insert a letter, drop one, change one, or swap two neighbours (write a private `name_distance`), and the first letters must be the same unless the first two are swapped. Of the names within the allowance, only those at the smallest distance stand in.

### `without_marks(query)`

As `without_tags`, for every marks key with or without its `!`: those terms go, the rest stay in order, a term with a space gets its quotes back. `without_tags` itself stays as it is.

## Tests (each a `#[test]`, named as a sentence)

1. **The fixture.** Read it with `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks-filter.txt"))`. For each case (five fields split on `|`, trimmed; skip `#` lines and empty ones): `parse(query)` is `Ok` with no name terms, and `filters.marks.accepts(label, tags, comment)` is what the last field says, where the tags field's commas become line ends and a written `\n` in the comment field becomes a real one. Say the line's number in the assert's message. Assert that at least 60 cases ran.
2. `parse`: `!ext:pdf` stays a name term and leaves the filters empty; `LABEL:Red` reads as `label:red`; `label:` and `!comment:` and `has:size` are errors that name the term; `tag:work` fills `Filters::tags` as before and `!tag:work` doesn't; a query of only `has:comment` makes `Filters::is_empty()` false.
3. `without_marks("report label:red !tag:old \"my file\" has:comment ext:pdf")` is `report "my file" ext:pdf`, and a query with no marks terms comes back as it was (with its quotes).
4. `nearest`, with the labels `Important`, `Pending` and the tags `work`, `workshop`, `netplan`: `tag:wrok` becomes the group `work` (not `workshop`); `tag:netpaln` becomes `netplan`; `label:importnat` becomes `important`; `tag:importnat` becomes `important` (a label answers to `tag:`); `tag:work`, `tag:abc`, `tag:xyzw` and `!tag:wrok` change nothing (`None`); `tag:wrok;netplan` is left alone, since `netplan` is there as typed.
5. `accepts` holds for an empty filter whatever the marks, and doesn't panic on empty strings, a comment of 4 KB, or names in Arabic.
6. The tests the file has pass unchanged.

## Done when

The tests pass, clippy is clean, nothing in the file reads the disk or the clock, and `tree.rs` is untouched. If a case of the fixture seems wrong, say so in your report; don't change the fixture.
