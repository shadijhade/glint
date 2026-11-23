# Card A1 — Marks, the labels, and what an import does with what is there

Wave 1 · crate `glint-model` · pure logic, no I/O

**You own:** `crates/glint-model/src/marks/mod.rs` and `crates/glint-model/src/marks/labels.rs` (tests at the bottom of each). In `mod.rs`, the `pub mod` and `pub use` lines for `lines`, `wanted` and `xyplorer` are other cards': leave them as they are.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3, §5.1, §5.5, §5.6, §7.3 ("Meeting Glint's own marks"), §8 and §8.1. Then `crates/glint-core/src/tags.rs` lines 89 to 109 (`tags_set`: what adding and taking off a tag does in 0.16, which `Change::Tags` keeps doing).
**Check with:** `cargo test -p glint-model marks::` and `cargo clippy -p glint-model --all-targets -- -D warnings`.

## What to build

Fill in `Change::apply`, `meet`, `label_of` and `shown`; their signatures are in the files. Add `tidy` (in `mod.rs`) and `settled` (in `labels.rs`, re-exported beside `shown`). Nothing here knows about files or drives.

### `pub fn tidy(marks: &Marks) -> Marks` (new, `mod.rs`)

What any marks become before they are kept. The other cards' readers return what a file says; whoever keeps marks calls this, and every function below returns tidy marks.

- **Label**: `\r` and `\n` become spaces, then trimmed. Empty is `None`. Longer than `LABEL_CAP` bytes: cut at the last character boundary at or before `LABEL_CAP`, then trimmed again.
- **Tags**: each one has `\r` and `\n` made spaces and is trimmed; empty ones go; one of each in any case (compare `to_lowercase()`), the first spelling kept, the order kept.
- **Comment**: `\r\n` and a lone `\r` become `\n`; `\0` is taken out; trailing white space is trimmed (`trim_end`). Nothing but white space is `None`. A comment is never cut here: core refuses one over `COMMENT_CAP` with words.

### `Change::apply(&self, marks) -> Marks`

| Change | What `marks` become |
| :--- | :--- |
| `Label(Some(name))` | the same with that label |
| `Label(None)` | the same with no label |
| `Tags { add, remove }` | first every tag that is in `remove` (any case) goes, then each tag of `add` that isn't there yet (any case) is put at the end, in `add`'s order |
| `Comment(Some(text))` | the same with that comment |
| `Comment(None)` | the same with no comment |
| `Put(m)` | `m` |
| `Clear` | `Marks::default()` |

The result is always `tidy`.

### `meet(has, incoming, how) -> (Marks, bool)`

Both are tidied first.

- `Meet::Replace`: `(incoming, false)`.
- `Meet::Fill`: the label is `has`'s, else `incoming`'s. The tags are `has`'s, then `incoming`'s that `has` lacks (any case). The comment is `has`'s, else `incoming`'s. The `bool` is true when both have a comment and the two differ: Glint's was kept, and the import's report says so.

### `label_of(labels, name) -> Option<(usize, &Label)>`

Names are compared folded: trimmed, `to_lowercase()`, and a whole name `grey` reads as `gray` (XYplorer's seventh label is *Grey*). An empty name is `None`. First the label whose own name is it; if none, the first label one of whose `was` is it.

### `shown(labels, marks) -> Shown`

1. `marks.label` is `Some(l)`: when `label_of(labels, l)` finds `(i, label)`, draw `label.name` with `place: Some(i)`; else draw `l` as the file says it, `place: None`. The tags are all of `marks.tags`.
2. No label: the first tag for which `label_of` finds `(i, label)` is drawn as the label (`label.name`, `Some(i)`), and the tags are the others (that one tag left out, the rest in order). This is a 0.16 color tag (§5.6).
3. Else no label, and all the tags.

### `pub fn settled(labels: &[Label], marks: &Marks) -> Marks` (new, `labels.rs`)

What a file's marks are written as the next time they are written (§5.5, §5.6): `tidy(marks)`, with

- a 0.16 color tag made the label: when there is no label and `shown` drew a tag as one, the label becomes that label's name and the tag leaves the tags;
- an old name made the new one: when the label is one a label `was`, it becomes that label's name.

Marks that need neither come back tidy and otherwise as they were. A label this PC doesn't have stays as the file says it.

## Tests (each a `#[test]`, named as a sentence)

1. `tidy`: a label `"  In\nreview "` is `In review`; a label of 300 `é` is at most `LABEL_CAP` bytes and still valid text; tags `[" a ", "", "A", "b\nc"]` are `["a", "b c"]`; a comment `"one\r\ntwo\r  \n\n"` is `"one\ntwo"`; a comment of spaces is `None`; a comment of 70,000 bytes is not cut; tidying twice changes nothing.
2. Every row of the `Change` table, starting from marks that have all three. `Tags { add: ["New", "work"], remove: ["OLD"] }` on `["Work", "old"]` gives `["Work", "New"]`.
3. `Change::Put` and `Change::Label` tidy what they are given (a label with a line break, a duplicate tag).
4. `Clear` gives marks for which `is_empty()` holds.
5. `meet` with `Fill`: a label is taken only when there is none; tags are added without doubles in any case; a comment is taken only when there is none; two different comments keep Glint's and say `true`; two equal comments say `false`.
6. `meet` with `Replace` gives the incoming marks and `false`, even when they are empty.
7. `label_of` over `default_labels()`: `red`, ` RED ` and `Red` are place 0; `grey` and `Gray` are place 6; `Mauve` and an empty name are `None`. With the first label renamed `Important` and `was: ["Red"]`: `red` and `important` are both place 0; a label named `Red` added at the end wins over the old name (`red` is its place).
8. `shown`: a label this PC has is drawn by its name now, with its place (a file saying `red` after the rename is drawn `Important`); a label it doesn't have is drawn as written with no place; no label and tags `["Work", "Red", "Blue"]` draw `Red` with tags `["Work", "Blue"]`; no label and no color tag draw no label.
9. `settled`: `{ label: None, tags: ["Work", "red"] }` becomes `{ label: Some("Red"), tags: ["Work"] }`; after the rename, `{ label: Some("Red") }` becomes `Important`; marks with a label this PC lacks, or with nothing to move, come back equal to `tidy(marks)`.
10. `default_labels()` is the seven, Red to Gray, in that order, each with no `was`.
11. The JSON a `Label` is saved as (it is in the person's `state.json`): `{"name":"Red","color":15680580}` when `was` is empty, `"was":["Red"]` after it when not, and `{"name":"X","color":1}` reads with an empty `was`.

## Done when

The tests pass, clippy is clean, and nothing in the two files reads the disk or the clock. Say in your report that `tidy` and `settled` are new and where they are exported.
