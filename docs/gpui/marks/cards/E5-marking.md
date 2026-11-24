# Card E5 — Marking: the keys' and the menus' handlers

Wave 3 · crate `glint-ui` · one module, no view

**You own:** `crates/glint-ui/src/marking.rs` (its tests at the bottom). The workspace already calls its eight functions: for the four actions `actions.rs` binds (`Alt+1`…`Alt+9`, `Alt+0`, `Alt+T`, `Alt+N`, in the `FileList` context, so they do nothing while a text box has the keys) and for the menus' five events. The palette (card E2b) calls them too. Keep the eight signatures.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M6), F6, F7, F9, F10 and F11 in §4, §5.5, §5.6, §6.2, and "Wave 2, as it landed" under §9's table. Then [card D1](D1-hub.md). Then the code: `marking.rs` (the stubs); `workspace.rs` 1550 and 2997 (who calls you); `actions.rs` 262 (the keys); `dialogs/tags.rs` `set_marks` (414) and `open` (726); `dialogs/comment.rs` `open` (card E3 fills it); `components/row_facts.rs` `marks_among` (627) and its tests' `fake::MarksBackend` (879); the explorer: how the pane the keyboard is in, its tab and its selection are read (`explorer::try_explorer`, `active_tab`, the tab's selection and focused item: find how `workspace.rs`'s other handlers that act on the selection get its paths, and do the same); glint-model's `marks::{Change, Marks, shown, clip_text, clip_marks}`.
**Check with:** `cargo test -p glint-ui marking::` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient.)

## What to build

Every function works out **which items** and **which change**, and hands it to `dialogs::tags::set_marks(window, cx, paths, change)`, which remembers Undo, says `TagsChanged` and words a failure. Nothing here reads the disk: what an item has now comes from `RowFacts::marks_among` (an item not read counts as having no marks), shown with the settings' labels (`shown`).

| Function | What it does |
| :--- | :--- |
| `label_selection(index, ..)` | the label at `index` of `state::settings(cx).labels` (none there: nothing happens) for the selection of the pane the keyboard is in: `set_label(paths, Some(name), ..)` |
| `clear_label(..)` | `set_label(paths, None, ..)` for that selection |
| `edit_tags(..)` | `dialogs::tags::open(window, cx, paths)` |
| `edit_comment(..)` | `dialogs::comment::open(window, cx, paths)` |
| `set_label(paths, label, ..)` | `Some(name)`: when **every** item is shown with that label already (`shown`, so a 0.16 color tag counts), `Change::Label(None)`; otherwise `Change::Label(Some(name))` with the label's name as the settings have it. `None`: `Change::Label(None)` |
| `copy_marks(path, ..)` | the item's marks to the clipboard as `clip_text(base_name(path), &marks)`; an item with no marks known copies nothing and says `t!("“{name}” has no marks to copy")` in a notice |
| `paste_marks(paths, ..)` | `clip_marks` of the clipboard's text; none: a notice `t!("There are no marks on the clipboard")` with `t!("Copy an item's marks first (More → Marks → Copy marks).")`. Otherwise: **tags are added, a label or a comment replaces** (F10): `Change::Tags { add, remove: [] }` when it has tags, then `Change::Label(Some(..))` when it has a label, then `Change::Comment(Some(..))` when it has a comment |
| `clear_marks(paths, ..)` | `Change::Clear` |

- With no selection (or no explorer) the four key functions do nothing, quietly.
- When a label is taken off items whose "label" is a 0.16 color tag (no label stream, a tag that names the label), `Change::Label(None)` would change nothing: take the tag off instead (`Change::Tags { add: [], remove: [name] }` for those items, `Label(None)` for the others), so that the dot pressed again really takes the color off.
- Paste may be up to three changes, so up to three Undo steps: that is accepted for this wave; say so in your report.
- The clipboard is GPUI's (`cx.write_to_clipboard`, `cx.read_from_clipboard`), text only.

## Tests (STANDARDS §5.1 "Tests": `testing::init`, `let _guard = testing::lock();`; the tags hub with `row_facts`'s `fake::MarksBackend`, the explorer's fakes for a selection)

1. `Alt+1` on three selected items asks the backend for `Change::Label(Some("Red"))` on those three; pressed again once they all have it (the fake's marks read into `RowFacts`), `Change::Label(None)`.
2. With one of the three already Red, it labels (it does not take off).
3. A label renamed *Important* in the settings: `Alt+1` sets `Important`; an index past the labels does nothing; `Alt+0` asks `Label(None)`.
4. A 0.16 file (tags `Work`, `Red`): the Red dot pressed takes the tag `Red` off and leaves `Work`.
5. No selection: none of the four key functions asks the backend for anything, and nothing panics.
6. `copy_marks` puts the sidecar's line on the clipboard (`{"name":"a.txt","label":"Red",…}`); an item with none copies nothing and says so.
7. `paste_marks` of that line onto two items asks for the tags to be added, the label and the comment to be set, on both; a clipboard of other text asks for nothing and says so; marks with only tags ask only for tags.
8. `clear_marks` asks `Change::Clear`; `edit_tags` and `edit_comment` don't panic with or without a selection (their dialogs are other cards': assert only what your code decides, the paths).
9. Arabic names and tags go through copy and paste unchanged.

## Done when

The tests pass, clippy is clean, the eight signatures are as they were, and nothing here calls glint-core or reads a file. Say in your report every new string, how you read the selection, and what a person would see for each key (nobody has tried them in the real app yet).
