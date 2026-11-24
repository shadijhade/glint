# Card E1c — The grid: a label's pill under a tile's name, the note mark

Wave 3 · crate `glint-ui` · the grid's tiles

**You own:** `crates/glint-ui/src/components/file_grid/tile.rs`, and the grid's tests about it (add them at the end of `file_grid/tests.rs`). Not `file_grid/mod.rs`, not the list (card E1b).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M11), F1 and F3 in §4, §5.5, §5.6, §13, and "Wave 2, as it landed" under §9's table. Then the code: `tile.rs` whole (the tile's fills; its name and the name's accessible label at 506; the tag dots at 402); `dialogs/tags.rs` `tag_color` and `tag_dots` (56, 132); `components/row_facts.rs` `marks_of` (578); glint-model's `marks::{shown, Shown, Label}`.
## The rules for a card that draws (STANDARDS §5 is the law; these are where such cards go wrong)

- **The kit and the theme only.** No `gpui_component` in a view, no color or size of its own. A label's color is the label's own, the same in both themes: `theme::label_tone(label.color)` (`theme.rs:665`). How strongly it washes a row or a tile's name is **one function**, `theme::label_wash(label.color, cx)` (`theme.rs:672`), landed for you: use it and never an opacity of your own.
- **A label is not a state** (M11): selected, hovered and dropped-on rows keep their one fill, drawn **over** the label's; a label is never in the accent, never a bar at the row's edge, never the color of the line's text.
- **Every string through `t!`**, exactly as PLAN §4 words it where it words it. List every new string in your report.
- **Everything that can be pressed has a role and a name**; keyboard alone works (focus order, Enter, Escape).
- **Right to left:** start and end helpers only, and a test with an Arabic name, tag and comment in everything that lays text out (`testing::set_rtl`).
- **Nothing on the UI thread but reading what is in memory.** Marks come from `RowFacts` (`components/row_facts.rs`: `marks_of` 578, `marks_among` 627); changes go through `dialogs::tags::set_marks(window, cx, paths, change)` (`dialogs/tags.rs:414`), which remembers Undo and says `TagsChanged`. Never straight to glint-core from a view.
- **Only what is known is drawn** (M4): an item whose marks aren't read yet draws none, no placeholder.
- **What is drawn for an item** is `glint_model::marks::shown(labels, marks)` with the labels of `state::settings(cx).labels`: the label by its name now with its place (a 0.16 color tag reads as its label, and is then not among the tags), and the tags left.
- **The tests the files have pass unchanged**, except the ones this card names.

**Check with:** `cargo test -p glint-ui components::file_grid` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient.)

**Landed for you:** `theme::label_tone` and `theme::label_wash`.

## What to build

A tile reads its item's marks where it reads its tags today (`marks_of` in place of `tags_of`) and works out `shown(labels, marks)` with the settings' labels.

- **The pill (F1).** A tile whose item has a label: its **name sits on a pill** of `theme::label_wash(label.color, cx)` (rounded, hugging the name's lines, inside the tile), a label this PC lacks in `tag_color(name)` the same way. The tile's own states (hover, selected, dropped on) keep their one fill, over or around the pill as they are today; the name's text keeps its color. No pill for no label, and none while the marks aren't read.
- **The dots** are the tags left (`Shown::tags`), where they are today.
- **The note mark (F3).** A tile whose item has a comment has the small note mark beside its dots (the same icon card E1b uses: `IconName::StickyNote` or the nearest the kit has; say which in your report), its tooltip the comment, its accessible name `t!("Comment")`.
- **The tile's accessible name** says the label, the tags and the comment's first line after the name, when it has them: `t!("label {name}")`, `t!("tags {list}")`, `t!("comment: {first}")` (the same three strings card E1b uses in the list).
- At the smallest tile sizes the pill stays (it is the name's ground); the note mark and the dots give way as the dots do today.

## Tests (STANDARDS §5.1 "Tests"; marks from `row_facts`'s `fake::MarksBackend`)

1. A tile labeled *Red* has the pill and an unlabeled one has none; a tile whose marks aren't read has none.
2. A labeled tile that is selected keeps the selection's fill, and the pill's color is `label_wash`'s, not the accent.
3. A 0.16 item (tags `Work`, `Red`, no label) has Red's pill and one dot.
4. A tile with a comment has the note mark named *Comment* with the comment as its tooltip.
5. The tile's accessible name says label, tags and comment for an item with all three, and only the name for one with none.
6. Right to left with an Arabic name, tag and comment: the pill is under the name and nothing overlaps.
7. Every test the grid has passes unchanged.

## Done when

The tests pass, clippy is clean, and a tile without marks draws what it drew. Say in your report every new string, the icon, and that nobody has seen it on a screen yet.
