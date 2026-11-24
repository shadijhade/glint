# Card D1 — The hub takes marks; `RowFacts` keeps them; Undo

Wave 2 · crate `glint-ui` · models and their tests, nothing drawn

**You own:** `crates/glint-ui/src/dialogs/tags.rs` **from its top to the line `// ---- The dialog's model` (328)**, and that file's tests; `crates/glint-ui/src/components/row_facts.rs`; `crates/glint-ui/src/explorer/ops_types.rs`; `crates/glint-ui/src/explorer/undo.rs` (tests at the bottom of each). The dialog itself (`TagsEdit`, `TagsDialog`, `open`) is card E3's, next wave: leave it as it is. No menu, no key's handler, nothing drawn in the list: those are wave 3.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4 to M6), F11 in §4, §6.1, §6.2, §8 (the glint-ui part), §8.1 ("glint-ui"), §10. STANDARDS §5.1 "Tests" matters here. Then the code: `dialogs/tags.rs` lines 1 to 328 (`Backend` at 164, `CoreBackend` at 184, `TagsChanged` at 206, `set_tags` at 282, `set_marks` at 323) and its tests (604 on; their `Fake` at 616); `row_facts.rs` whole (`TagsRead` 199, `Store` 214, `ask` 318, `queue` 369, `tags_arrived` 419, `listed_again` 484, `tags_changed` 495, `tags_of` to `tags_among` 513 to 540, `ask_tags` 559, the tests' `fake::Tags` 709); `explorer/ops_types.rs` `UndoAction` (129), `UndoRunner` (261) and `FakeUndoRunner` (273); `explorer/undo.rs` whole (`CoreRunner` 23, the run of a `Marks` step that does nothing at 160, the tests' `explorer_in_a` 321); `dialogs/compress_in_place.rs` `remember_undo` and `offer_undo` (204, 211): how a dialog outside the explorer remembers a step and offers Undo in a notice. glint-model's `marks/mod.rs` for `Marks`, `Change`, `Home` and `Outcome`.
**Check with:** `cargo test -p glint-ui dialogs::tags`, `cargo test -p glint-ui components::row_facts`, `cargo test -p glint-ui explorer::undo` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (This crate builds slowly, and other agents' builds wait on the same lock: be patient, and don't run the whole crate's tests more than once at the end, if at all.)

**What is there for you:** glint-core's `marks_read`, `marks_set` and `marks_put` exist and **answer nothing until card B2 is in**, at the same time as you work. So everything here is tested against fakes; `CoreBackend` and `CoreRunner` only pass the call on.

## dialogs/tags.rs: the hub

The hub stays here, with its one `Backend` and its one event.

### `Backend`

Other files' fakes implement only `read` and `set` (`sidebar/tests.rs`, this file's `Fake`, `row_facts`'s `fake::Tags`), and they must compile and behave unchanged. So the marks calls keep **default bodies**, which now stand on the tags calls:

| Call | Default body | `CoreBackend` |
| :--- | :--- | :--- |
| `read_marks(paths)` | `self.read(paths)`, each list as `Marks { tags, ..Default::default() }` | `glint_core::marks::marks_read` |
| `set_marks(paths, change)` | for `Change::Tags { add, remove }`: `self.set(paths, add, remove)`, then one `Outcome` a path with an empty `before` and, for a path `set` said failed, `after: Err(why)`, else `after: Ok((Marks::default(), Home::Stream))`. For any other change: `Ok(Vec::new())`, as now | `glint_core::marks::marks_set` |
| `put_marks(items)` (new: `Vec<(String, Marks)>` to `Fut<Vec<Outcome>>`) | `Ok(Vec::new())` | `glint_core::marks::marks_put` |

A backend that only knows tags therefore answers outcomes whose `before` and `after` are both empty: there is nothing to undo, and the hub remembers nothing (below).

### `set_marks(window, cx, paths, change)`

On core's runtime (`core_bridge::run`, as `set_tags` does it today):

1. Call `backend.set_marks(paths.clone(), change.clone())`.
2. **Undo** (M6): the items to put back are, for every outcome whose `after` is `Ok((marks, _))` with `marks != before`, `(path, before)`. When there is at least one and `explorer::try_explorer(cx)` gives the explorer: `remember_undo(label, UndoAction::Marks(items), cx)`, one step. The label completes "Undid the …":

   | Change | Label | The notice |
   | :--- | :--- | :--- |
   | `Label(_)` | `t!("label of {what}")` | `t!("Changed the label of {what}")` |
   | `Tags { .. }` | `t!("tags of {what}")` | `t!("Changed the tags of {what}")` |
   | `Comment(_)` | `t!("comment of {what}")` | `t!("Changed the comment of {what}")` |
   | `Put(_)`, `Clear` | `t!("marks of {what}")` | `t!("Changed the marks of {what}")` |

   `what` is `plural(n, "item")`, `n` being how many items changed. Then the notice, a success with an **Undo** button that calls `explorer.undo(Some(step), cx)` and goes after 8 s, built as `compress_in_place.rs`'s `offer_undo` builds its own (a notification type of your own for its id).
3. For `Change::Tags` with an `add` that isn't empty: the recent tags are remembered and saved, exactly as `set_tags` does now.
4. `TagsChanged { paths }` is emitted, **always** (it now means any mark; whatever got written still shows). Say so in its comment.
5. **A failure is words.** The outcomes whose `after` is `Err(why)`: one notification, an error with the first one's `why` and a title:

   | | One item | Several | The call itself failed |
   | :--- | :--- | :--- | :--- |
   | `Change::Tags` | `Couldn't tag “{name}”` | `Couldn't tag {n}` | `Couldn't change the tags` |
   | any other | `Couldn't mark “{name}”` | `Couldn't mark {n}` | `Couldn't change the marks` |

   (`name` is `base_name` of the path, `n` is `plural(count, "item")`; the tags' three are today's strings, untouched.)

**`set_tags(window, cx, paths, add, remove)` keeps its signature and becomes `set_marks` with `Change::Tags { add, remove }`**: tags get Undo (G5) wherever the backend knows marks. Every test this file has passes unchanged.

## explorer/ops_types.rs and explorer/undo.rs: taking a marks step back

- `UndoRunner` gains `fn put_marks(&self, items: Vec<(String, Marks)>) -> UndoFut<Result<(), String>>`. `CoreRunner`: `glint_core::marks::marks_put(items)` through `core_bridge::run`; when an outcome's `after` is an `Err`, the answer is `Err` with the first one's item and words (`“{name}”: {why}`), else `Ok`. `FakeUndoRunner` writes the items down (a `marks` list with a `marks_put()` reader, like `renamed()`) and fails once when told, as its other calls do.
- In `Explorer::undo`, `UndoRun::Marks(items)` runs `runner.put_marks(items)`. The rest of the function stays: the folder is read again afterwards, which reads the rows' marks again.
- `UndoAction::Marks` is no file operation (`ops()` leaves it out, `op()` is `None`) and names no renamed folder: check that both hold and keep them so.

## components/row_facts.rs: the rows' marks

`RowFacts` reads **marks** where it read tags, and `tags_of` goes on answering.

- What is kept for a path is its `Marks` (in place of its tags alone). `tags_of` answers `marks.tags`; `marks_of(folder, path)` answers the marks, `None` while unread (M4: draw nothing). Add `marks_among(paths)` beside `tags_among`, the same way (by path in lower case, unread paths left out).
- A batch is read with `backend.read_marks` in place of `backend.read`: one call a batch, the same paths, the same `TAGS_BATCH`. With a backend that only knows tags the default body calls its `read` once with those paths, so the tests there are see what they saw.
- Everything that holds for tags holds for marks: a failed or short answer says nothing for the paths it lacks; a change named by `TagsChanged` is read again while the old marks keep showing; a folder listed again reads again.
- **`ask_all(folder, entries, cx)`** (M5, §6.1), for a list sorted, grouped or filtered by a mark: every entry of the folder that `ask` would read tags for (on this disk, not on another computer) and whose marks are neither fresh nor on their way is asked for **in one `read_marks` call** (core reads a long list on every core), not in batches of 200. When it lands the marks are kept and observers are told once. Add **`pub fn has_all(&self, folder: &str) -> bool`**: true once a whole read of the folder has landed, false again when the folder is listed again or a mark in it changes (until the next `ask_all` lands). Asking again while a whole read is on its way, or when everything is fresh, calls nothing.
- A folder read whole is not let go of by `Store::trim` while `has_all` holds (a sort needs every row): `MOST_PER_FOLDER` is for the reads of rows on screen.

## Tests

As STANDARDS §5.1 "Tests" and the tests these files have: `testing::init`, `let _guard = testing::lock();`, fakes, never a real disk.

A fake backend that knows marks (in each file's tests, or `row_facts`'s `fake` module beside `fake::Tags`): it holds marks by path, answers `read_marks`, applies `change.apply` in `set_marks` and answers real befores and afters (`Home::Stream`), can be told to fail a path with words or the whole call, and writes down what it was asked.

dialogs/tags.rs:
1. `set_marks` with `Change::Label(Some("Red"))` on three paths: the backend is asked once with those paths and that change; `TagsChanged` names the three; the explorer (`explorer::fake`, as `undo.rs`'s `explorer_in_a` makes one) has one more Undo step, labeled `label of 3 items`, whose action is `UndoAction::Marks` of the three paths with what each had.
2. Only what changed is put back: with one of the three already labeled Red, the step holds two items and says `label of 2 items`; when nothing changed there is no step and no notice, and `TagsChanged` is still said.
3. The labels of the four kinds of change: `label of …`, `tags of …`, `comment of …`, `marks of …` (`Put` and `Clear`).
4. A failure is said: one path failing gives the title `Couldn't mark “a.txt”` with its words; two give `Couldn't mark 2 items`; the call failing gives `Couldn't change the marks`; the paths that worked are still in the Undo step, and `TagsChanged` is said every time.
5. `set_tags` goes through marks: with the backend that knows marks, it is asked `set_marks` with `Change::Tags { add, remove }` (and never `set`), the added tags are remembered as recent, and the step says `tags of 1 item`. With a backend that only knows tags, `set` is asked as before and no step is remembered.
6. The default bodies: `read_marks` of a tags-only backend gives each path's tags as marks; its `set_marks` with a label answers nothing; its `put_marks` answers nothing.
7. The notice offers Undo: after test 1's change, taking the step back (`explorer.undo(Some(step), cx)`, with a `FakeUndoRunner` installed) asks the runner to put the three befores back.
8. With no explorer in the app (`try_explorer` is `None`), a change still reaches the backend and says `TagsChanged`, and nothing panics.

explorer/undo.rs and ops_types.rs:
9. Ctrl+Z on a marks step asks the runner's `put_marks` with the step's items and says `Undid { label }`; when the runner fails, `UndoFailed` with its words, and the step is used up.
10. A marks step is not a file operation, runs as one `UndoRun::Marks`, names no renamed folder, and takes its turn in a `Sequence`.

components/row_facts.rs:
11. `marks_of` is `None` until read, then the item's marks (a label, tags and a comment); `tags_of` is those marks' tags; an item with none reads as empty marks, not `None`.
12. A frame's rows are read with one `read_marks` call and `read` is never asked (the marks fake); with `fake::Tags` (tags only) the same rows are read with one `read` call, as before.
13. A mark changed (`TagsChanged`) is read again, and the old marks show until the new ones arrive.
14. `ask_all` on 1,000 entries asks once, with all of them; `has_all` is false before it lands and true after; asked again it calls nothing; after the folder is listed again `has_all` is false and `ask_all` reads again; entries in an archive, in the Recycle Bin or on another computer are not asked for.
15. A folder of `MOST_PER_FOLDER + 100` entries read whole keeps every one (`marks_of` of the first and of the last).
16. `marks_among` gives the marks read for the paths asked, by path in lower case, and leaves out the unread.

And every test of the four files passes unchanged, as do `sidebar/tests.rs`'s and `file_list/tests/facts.rs`'s (their fakes are not yours to touch: if one stops compiling, a default body is wrong).

## Done when

The tests pass, clippy is clean for the crate, the dialog's part of `tags.rs` is untouched, and `set_tags`, `tags_of`, `tags_among`, `ask` and `ask_tags_of` keep their signatures. Say in your report what the coordinator must wire (who calls `set_marks`, `ask_all` and `has_all`, and where the rows' marks are handed to `Reads::marks` and `matcher_marks`), and every string you added for `t!`.
