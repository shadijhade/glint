# Card C1 — The index reads three streams

Wave 1 · crate `shadui-index` · the service's own data, no new I/O

**You own:** in `crates/shadui-index/src/`: `record.rs`, `tree.rs`, and four marked places elsewhere:
- `volume.rs`: the `RecordState` made from a record (line 435), the merge of an extension record (459), and what the first read hands the builder (622);
- `refs.rs`: the `RecordState` at line 376;
- `engine.rs`: `save_tags` and `load_tags` (1507, 1521) and the two lines that call them (805, 829). The rest of `engine.rs` is card C2's.

`query.rs` is card A4's: `Tree::search` goes on asking `filters.tags` and `tagged`, and the coordinator puts A4's filter there after the wave. Don't touch `Tree::search`.

**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §5.1, §6.4, §8, §8.1, §14 item 6 (what was seen of records that fill up). Then, in the crate: `record.rs` lines 165 to 345 (`Attribute`, `TAGS_STREAM`, `tags_text`, `Parsed`, `parse`) and its `RecordBuilder` (382); `tree.rs` `RecordState` (130), `Tree` (193), `Marked` (213, there since S0 and unused), `set_record` (528), `remove_record` (595), `tagged` (608), `tag_counts` (616) and `Builder::tags` (1515).
**Check with:** `cargo test -p shadui-index` and `cargo clippy -p shadui-index --all-targets -- -D warnings`.

## What to build

Today a file's tags reach the index from a resident `glint.tags` stream and are kept as `Tree::tags`. Make that the three marks, kept as `Tree::marks`, and note which records have marks the record itself doesn't show (*far*), for card C2 to read through the file system.

### record.rs

- Two more stream names beside `TAGS_STREAM`: `glint.label` and `glint.comment`. A stream's name is compared **without regard to ASCII case** (NTFS opens `Glint.Tags` as `glint.tags`, and keeps the case it was made with): one private helper for all three, over the UTF-16LE bytes.
- `pub fn label_text(value: &[u8]) -> Option<String>`: as text (`from_utf8_lossy`), the first line with anything in it, trimmed, cut to 256 bytes at a character boundary; `None` when nothing is left.
- `pub fn comment_text(value: &[u8]) -> Option<String>`: the first 4,096 bytes (`pub const COMMENT_KEPT: usize = 4096`) as text, a character broken by the cut dropped; `\r\n` made `\n`; trailing white space trimmed; `None` when nothing is left.
- `Parsed` gains `pub label: Option<String>`, `pub comment: Option<String>` and `pub far: bool`.
- In `parse`: a resident `glint.label` or `glint.comment` fills its field, as `glint.tags` does. `far` is set when any of the three streams is **not** resident, and when the record's attribute list is not resident (read from the disk's clusters it can be older than what Windows holds: §14 item 6).
- `RecordBuilder` (tests): add `pub fn stream(&mut self, name: &str, value: &[u8]) -> &mut Self` (a resident named data stream) and `pub fn far_stream(&mut self, name: &str, size: u64) -> &mut Self` (one that isn't), over its private `attribute`.

### tree.rs

- `RecordState` gains `pub label: Option<String>`, `pub comment: Option<String>`, `pub far: bool`. The test helpers `dir` and `file` (1629, 1641) and every other literal that lists all its fields get `..Default::default()`.
- `Tree::tags` goes. In its place:

```rust
/// What the marked records are marked with: only they take room.
pub marks: HashMap<u32, Marked>,
/// Records whose marks the record itself doesn't show (a stream that left it, an attribute list that did):
/// to be read through the file system (far.rs).
pub far: HashSet<u32>,

/// Record `n`'s marks, if it has any.
pub fn marked(&self, n: u32) -> Option<&Marked>;
/// Gives record `n` exactly these marks, as read through the file system: all three empty takes them
/// away. `n` is then no longer far.
pub fn set_marks(&mut self, n: u32, marks: Marked);
/// Every label in the index with how many items have it, most used first.
pub fn label_counts(&self) -> Vec<(String, u64)>;
/// How many items have a comment.
pub fn comments(&self) -> u64;
```

- `tagged` and `tag_counts` keep their signatures and what they answer, reading `marks`. `label_counts` counts as `tag_counts` does: names alike in any case are one, shown as first met; most used first, then by name; a record no longer in use is not counted. `comments` skips those too.
- **`set_record`**, where it sets or removes the tags today:
  - a state that is **not far**: the record's marks become exactly what the state says (a `Marked` with `""` for what it lacks), or go when it says none; `n` leaves `far`;
  - a state that **is far**: what the state says replaces that field, and a field it doesn't say **keeps what was there** (it was read through the file system, and will be again); `n` joins `far`.
- `remove_record` takes `n` out of both.
- `Builder` (the first read of a drive): beside `tags(n, String)`, which now fills that record's `Marked`, add `label(n, String)`, `comment(n, String)` and `far(n)`. A record and its extension records may each bring one field: they add up in one `Marked`.

### volume.rs, refs.rs

- Line 435: the `RecordState` takes `label`, `comment` and `far` from `parsed`.
- Line 459: an extension record's `label` and `comment` are merged as its `tags` are, and `state.far` becomes true when the extension's is.
- Line 622: the builder is handed `label`, `comment` and `far` beside `tags`.
- `refs.rs:376`: `tags: None` becomes `..Default::default()` (a ReFS drive brings no marks: PLAN §12).

### engine.rs: the file beside the snapshot

Tags are saved today as `<snapshot>.tags` (`"GLINTTAG"`, a count, then each record's number, length and text). Replace the pair with `save_marks(path, &HashMap<u32, Marked>, &HashSet<u32>)` and `load_marks(path) -> (HashMap<u32, Marked>, HashSet<u32>)`, on `<snapshot>.marks`, everything little-endian:

```
"GLINTMRK"   version: u32 = 1   records: u64
then for each record:   n: u32   label: (len: u32, bytes)   tags: (len: u32, bytes)   comment: (len: u32, bytes)
then   far: u64   and that many n: u32
```

- Written to `.marks.tmp` and renamed, as `save_tags` does. After a save that worked, a `.tags` file beside it is removed (ignore an error): it is stale from then on.
- Loading never panics and never takes a damaged file in part: a wrong name or version, a file that ends early, a length past the end or over 64 KB, or a count the file's size can't hold, each give no marks at all.
- At line 805: load `.marks` when it is there; else `.tags` with the old reader (keep it, private) into marks that have only tags. At line 829: save the marks (the log line says *marks*).

## Tests (each a `#[test]`, named as a sentence)

record.rs, with `RecordBuilder`:
1. A record with the three streams resident says its label, tags and comment, and is not far.
2. A stream named `Glint.Label` is the label.
3. A `glint.comment` that left the record makes it far, and its resident tags are still read.
4. An attribute list that left the record makes it far.
5. `label_text`: `"  Important \r\nsecond\n"` is `Important`; 300 bytes of `é` come out at most 256 bytes and valid; nothing but line ends is `None`. `comment_text`: 5,000 bytes of `é` come out at most 4,096 bytes ending on a whole character; `"a\r\nb\r\n"` is `a\nb`; bytes that aren't UTF-8 don't panic.
6. Other streams are not marks: `Zone.Identifier` and `glint.other` leave all three empty and the record not far.

tree.rs:
7. A record's marks come and go with it: set with marks, `marked(n)` has them; set again with none, it is `None`; set with marks and removed, it is `None`.
8. `tagged` answers as it did (groups, any case), from `marks`; `tag_counts` and `label_counts` merge cases, show the first spelling, put the most used first, and leave out a record that was removed; `comments()` counts the records with one.
9. A far record keeps what was read for it: after `set_marks(n, …)` with a comment, `set_record(n, …)` with `far` and only tags leaves the comment and has the new tags, and `n` is in `far`; the same state without `far` takes the comment away and `n` out of `far`.
10. `set_marks` with three empty fields removes the record's marks, and takes `n` out of `far`.
11. The builder adds up a record's tags and an extension's comment into one `Marked`, and remembers `far`.

engine.rs:
12. Marks saved beside a snapshot come back the same, the far ones too, with an Arabic label and a comment of several lines (a folder of your own under `std::env::temp_dir()`, removed at the end).
13. A `.tags` file as 0.16 wrote it (make its bytes by hand) is read when there is no `.marks`, and gives marks with only tags; once `.marks` is saved the `.tags` is gone.
14. A damaged `.marks` is no marks and no panic: a good file cut at every length; a wrong first eight bytes; version 2; a record count of `u64::MAX`; a length of `u32::MAX`.

And every test the crate has passes unchanged.

## Done when

The tests pass, clippy is clean, `Tree::search` and `query.rs` are untouched, and a drive with nothing marked takes no more memory than before (two empty maps). Say in your report whether you found any other place that read `Tree::tags`, and anything in `volume.rs` you had to change beyond the three places.
