# Card C2 — The engine: `Marks`, `MarksIn`, a search that looks in marks, the far records' thread

Wave 2 · crate `shadui-index` · the service's side

**You own:** `crates/shadui-index/src/engine.rs` and `crates/shadui-index/src/proto.rs` (tests at the bottom of each), and in `crates/shadui-index/src/tree.rs` **one new method**, `Tree::search_marks`, right after `search_until`, with its tests in that file's tests. The rest of `tree.rs` stays as it is, `search_until` included (you may move its checks into a private helper both use, if its tests pass unchanged). `far.rs` and `client.rs` are card C3's; `pipe.rs`, `query.rs` and `cli.rs` are nobody's this wave: don't touch them.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M10), F12, F14 and F15 in §4, §6.4, §8, §8.1, §10 ("The service's promises"), §14 item 6, and "Wave 1, as it landed" under §9's table. Then cards [C1](C1-index.md) (`Marked`, `Tree::marks` and `far`) and [A4](A4-filter.md) (`MarksFilter`, `nearest`). Then the code: `proto.rs` `SearchRequest` (122), `MarkCounts` and `MarkedChild` (327), the flags (341), `Hit`, `encode_hit` and `decode_hits` (347 to 379); `tree.rs` `Tree` and `Marked` (199 to 232), `Search` (270), `links_of` (377), `marked`, `label_counts`, `comments`, `tag_counts` (642 on), `list` (865), `search_until` (1125) and `reachable` (1315); `engine.rs` `Engine::start` (442), `with_tree` (470), `stop` (503), `worker` (538), `tags` (981), `marks` and `marks_in` (1001, 1007), `search_until` (1036; the close matches at 1139), `sizes` and `list` (1161, 1181: their words for a drive that isn't indexed and a folder that isn't there), `send_ranked` to `encode_hits` (1385 to 1496); `far.rs` whole (30 lines: what you call); `query.rs` `MarksFilter::nearest` (718); `pipe.rs` line 351 (it already sends what you answer).
**Check with:** `cargo test -p shadui-index` and `cargo clippy -p shadui-index --all-targets -- -D warnings`.

**Landed for you:** `crate::far::drain(tree: &RwLock<Tree>, letter, stop) -> usize` exists and answers 0 until card C3 fills it, at the same time as you work. Call it; don't write it.

## What to build

### `Engine::marks` and `Engine::marks_in`

- **`marks()`**: over the ready drives, every label and every tag with how many items have it, merged as `tags()` merges them (names alike in any case are one, shown as first met; most used first, then by name), and `comments` summed (`Tree::label_counts`, `tag_counts`, `comments`).
- **`marks_in(path)`**: the marked items right inside the folder. The drive and the folder are found as `sizes` and `list` find them, with their words when the drive isn't indexed or the folder isn't there. For each item `Tree::list` gives whose record has marks: its name, its label (`""` for none), its tags (the lines of `Marked::tags`), and **whether** it has a comment. **Never a comment's text**: `MarkedChild` has no place for one, and none is to be added.

### proto.rs: a hit says how it was found

`Hit` gains `pub by: u8`: 0 for a hit found by its name, else `HIT_BY_COMMENT`, `HIT_BY_TAG` or both. `encode_hit` writes the first byte as `dir | by`; `decode_hits` reads `by` from those two bits and `dir` from bit 0 as now, so an app from before reads what it read. Every place that makes a `Hit` (they are all in your two files) says `by: 0` unless it is a hit of `search_marks`.

### tree.rs: `Tree::search_marks`

```rust
/// The names of marked records whose **marks** hold every one of `words` (folded, plain: no wildcard, no path),
/// and which `search` would let through but for their names: each with how it was found (`HIT_BY_COMMENT`,
/// `HIT_BY_TAG`, or both). A name `search.matcher` itself matches is left out: `search_until` found it.
pub fn search_marks(&self, search: &Search, words: &[String], gone: &(dyn Fn() -> bool + Sync)) -> Vec<(u32, u8)>;
```

- It walks `self.marks` and nothing else (only marked records take time), skipping records no longer in use. With no words it finds nothing.
- A record is found when **every** word is inside its comment (folded) or inside one of its tags or its label (folded): a word matches inside, as a name does. `HIT_BY_COMMENT` is set when a word was found in the comment, `HIT_BY_TAG` when one was found in a tag or the label.
- For each name of a found record (`links_of`): everything `search_until` checks after the name's own match is checked the same way and in the same order: `search.filters` (with the record's marks asked of `filters.marks`), hidden, the excluded names, whether its folder is reachable from `search.scope`, the excluded paths. `search.matcher` is asked only to leave out a name it matches.
- The answer is in index order (by link id). `gone` is asked now and then; what is found then is incomplete and goes unused.

### engine.rs: `search_until` with `request.marks`

When `request.marks` is set and there is at least one name term and **every** name term is a plain word (no `*`, `?`, `\`, `/` or `:`): after the names' hits are sent (both ways: in index order and ranked), each target's `search_marks` hits are sent, folded words being the name terms, encoded with their `by`, as long as `max` allows; they count in `count` and `limited` as any hit. Without `request.marks` nothing changes, on the wire or in time.

### engine.rs: close matches for a tag or a label (F14)

When nothing was found as typed (names and marks together), `request.fuzzy` is set and `filters.marks` isn't empty: ask `filters.marks.nearest(labels, tags)` with every label and every tag the targets' trees have (`label_counts`, `tag_counts`). When it gives a filter, search again with it in place of `filters.marks` (the names as typed). If that finds something, `Done` says `fuzzy: true`. If it still finds nothing, the names' own close matches run as they do today, with the nearer filter when there was one.

### engine.rs: the far records' thread

A record whose marks its own record doesn't show (`Tree::far`) is read through the file system by `far::drain`. It runs **on a thread of its own, so that a search never waits for it**:

- One thread for the engine, started in `Engine::start` and joined where the workers are (`stop`). `with_tree` (tests) starts none.
- It goes round the ready drives calling `far::drain(&slot.tree, slot.letter, &|| stopped)`. When a whole round took nothing it sleeps a second (`self.stop.sleep`, which ends at once on stop); otherwise it goes round again.
- Put one round in a method of its own (`fn far_round(&self) -> usize`), so that a test can run it without the thread.
- It holds no lock of its own across `drain`, and never the slots' lock while a drive is drained (clone the `Arc<Slot>`s first, as `status` does).

## Tests (each a `#[test]`, named as a sentence)

proto.rs:
1. A hit found by its comment and one found by a tag come back from `decode_hits` with their `by` and their `dir`; a hit with `by: 0` encodes to the bytes it encoded to before (compare with the bytes written by hand); the first byte of a folder found by a tag is `1 | HIT_BY_TAG`.

tree.rs, `search_marks` (build the tree as that file's tests do):
2. A word in a comment finds the file with `HIT_BY_COMMENT`; a word in a tag, or in the label, with `HIT_BY_TAG`; two words, one in each, with both; a word in neither finds nothing; words match inside and in any case, Arabic too.
3. A file whose **name** holds the word is left out (the name's search has it), and no words find nothing.
4. The search's other rules hold: a file outside `scope`, a hidden one without `hidden`, one under an excluded name, one that `ext:` or `label:` in the filters refuses, are not found; a record removed since is not found; a file with two names (hard links) is found by each name in scope.

engine.rs (with `Engine::with_tree` and a tree that has marked records):
5. `marks()` merges cases, shows the first spelling, puts the most used first, and counts the comments.
6. `marks_in` lists the marked children of a folder and only them, with a label, the tags as a list, and `comment: true` without any text; a folder with none marked is an empty list; a drive that isn't indexed and a folder that isn't there are the errors `sizes` gives.
7. A search for `lacp` with `marks: true` sends the file named `lacp.txt` with `by: 0` and the file whose comment says *LACP rate fast* with `HIT_BY_COMMENT`, each once; `Done` counts both. The same search with `marks: false` sends the first alone. With `rank: true` the hits by marks come after the ranked ones.
8. With `marks: true`, `net*` and `src\net` look at names only; `max_results: 1` sends one hit and says `limited`.
9. `tag:netpaln` with `fuzzy: true` and a file tagged `netplan` finds it and `Done` says `fuzzy: true`; without `fuzzy` nothing is found; `tag:netplan` as typed says `fuzzy: false`.
10. `far_round` over a tree with nothing far takes nothing and changes no marks; the thread, started and stopped, ends (start an engine as the tests of `stop` do, or test the loop's function with a stop that is already set).

And every test the crate has passes unchanged (`pipe.rs`'s too: it reads `Hit`s you now give a `by`).

## Done when

The tests pass, clippy is clean, a search without `marks` sends byte for byte what it sent, `marks_in` and every hit carry no comment's text, and nothing a stream holds can make the service panic. Say in your report what `search_marks` costs on a tree of your test's size if you timed it, whether you moved `search_until`'s checks into a helper, and anything in `pipe.rs` or `cli.rs` that the coordinator must change for `Hit::by`.
