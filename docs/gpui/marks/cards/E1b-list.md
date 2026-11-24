# Card E1b — The list: the wash, the note mark, three columns

Wave 3 · crate `glint-ui` · the file list's rows

**You own:** `crates/glint-ui/src/components/file_list/cells.rs`, `layout.rs` and `header.rs`, and the list's tests that are about them (`file_list/tests.rs`: add yours at its end, or in a new `file_list/tests/marks.rs` if that folder is how the tests are laid out; say which). Not `file_list/mod.rs`, not the grid (card E1c), not `explorer/` (card E1a makes the sort, the group and the filter work; you draw).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M5, M11), F1, F3 and F4 in §4, §5.5, §5.6, §13, and "Wave 2, as it landed" under §9's table. Then the code: `cells.rs` whole (the row and its fills around 517; the name's cell with the tag dots, read at 379 and drawn at 449; the columns' cells, the marks' three drawing nothing at 559; `peek_cell` 710; `describe_row` 787; `text_cell` 892); `layout.rs` whole (the three widths are there); `header.rs` (the columns' menu at 503, Group by at 524); `dialogs/tags.rs` `tag_color` and `tag_dots` (56, 132); `components/row_facts.rs` `marks_of` (578); glint-model's `marks::{shown, Shown, Label}`.
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

**Check with:** `cargo test -p glint-ui components::file_list` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient; the whole crate's tests at most once.)

**Landed for you:** `theme::label_tone` and `theme::label_wash`; the three columns in `COLUMN_ORDER` and Label in `GROUP_BYS` (so the header's menus list them already: check that their rows read **Label**, **Tags**, **Comment** and Group by → **Label**, and fix only what is in your files).

## What to build

Each row reads its item's marks once, where it reads its tags today (`facts.marks_of(&frame.facts_key, &entry.path)`, in place of `tags_of`), and works out `shown(labels, marks)` with the settings' labels (read once a frame, not once a row).

### The wash (F1)

A row whose item has a label this PC has is washed in `theme::label_wash(label.color, cx)`: a rounded fill **inside** the row's line, with the same inset and radius the selection's fill has (`theme::size::RADIUS_ROW`), drawn **under** the hover and selection fills, so a selected or hovered labeled row shows its state as every row does, over the wash. A label this PC doesn't have (`place: None`) washes in `tag_color(name)` through the same function's strength (give `label_wash` the color's `0xRRGGBB`, or add nothing to the theme and say what you needed). No bar, no outline, no colored text.

### The note mark (F3)

A row whose item has a comment has a small note mark after its name (a stroke icon in the muted text color, `IconName::StickyNote` or the nearest the kit has: add it to the kit's icon test list **in your report**, kit.rs is not yours), with the comment as its tooltip (the whole comment, lines kept) and an accessible name `t!("Comment")`. It gives way before the name does in a narrow column, as the peek does.

### The dots

The name's cell draws the dots of the **tags left** (`Shown::tags`: a color tag drawn as the label is not a dot too). **While the Tags column shows, the dots leave the name's cell.**

### Three columns (F4)

| Column | Its cell |
| :--- | :--- |
| **Label** | a pill: the label's name on `label_wash` of its color, a dot of `label_tone` at its start; a label this PC lacks the same way in `tag_color`; none: empty |
| **Tags** | chips, one a tag (a dot in `tag_color(tag)` and its name), as many as fit, then `+N`; the tooltip is all of them |
| **Comment** | the comment's first line, cut with an ellipsis; the tooltip is the whole comment |

Each sorts from its header as every column does (card E1a makes the order; the header only has to offer it). Widths are `layout.rs`'s; the cells never grow a row's height.

### `describe_row`

Says the item's label, tags and comment after what it says today, when it has them: `t!("label {name}")`, `t!("tags {list}")` (the tags joined with `, `), `t!("comment: {first}")` (the first line). Nothing for what it lacks or what isn't read.

## Tests (STANDARDS §5.1 "Tests": `testing::init`, `let _guard = testing::lock();`, `window.render_frame(cx)`; marks from `row_facts`'s `fake::MarksBackend`)

1. A row labeled *Red* has the wash's element and an unlabeled one has none; a row whose marks aren't read yet has none (M4).
2. A labeled row that is selected still has the selection's fill, and its wash is under it (the order of the two elements); the wash's color is `theme::label_wash`'s and not the accent.
3. A row with a comment has the note mark, named *Comment*, whose tooltip is the comment; one without has none.
4. A 0.16 file (no label, tags `Work`, `Red`) is washed as Red and draws one dot (Work).
5. The Label, Tags and Comment cells: the label's name; three tags as chips, twelve as what fits and `+N`; a comment of three lines as its first line.
6. With the Tags column shown the name's cell has no dots; with it hidden they are back.
7. `describe_row` says label, tags and comment for an item with all three and nothing more for one with none.
8. Right to left, with an Arabic name, tag and comment: the note mark is at the name's end, the pill's dot at its start, and nothing overlaps (`testing::set_rtl`, bounds compared as the file's other RTL tests do).
9. The header's column menu lists Label, Tags and Comment, and Group by lists Label.
10. Every test the list has passes unchanged.

## Done when

The tests pass, clippy is clean, a folder with no marks draws exactly what it drew (no element added to a row without marks), and nothing is allocated per row per frame that a row without marks didn't allocate before. Say in your report every new string, every icon you used, and **what nobody has seen on a screen** (all of it: the look is checked afterwards in the real app, in both themes).
