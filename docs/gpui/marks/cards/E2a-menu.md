# Card E2a — The menu: the row of the labels' dots, and Marks under More

Wave 3 · crate `glint-ui` · the context menus

**You own:** `crates/glint-ui/src/components/menus/item.rs`, `spec.rs`, `view.rs`, `smart.rs` and `data.rs`, and the menus' tests (`menus/tests.rs`, `tests_content.rs`, `tests_view.rs`, `tests_smart.rs`). Not `menus/mod.rs` (the host), not `event.rs` (the five events are there), not the workspace (it already hands the events on).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M11), F7 and F10 in §4, §5.5, §5.6, §8 (the menus' part), §10 (the two tests that change), and "Wave 2, as it landed" under §9's table. Then the code: `item.rs` around 950 (More) and `tags_menu` (1355); `spec.rs` `Node` (20; `Swatches` at 43) and `Entry` (119); `view.rs` `render_strip` (1248) and where `Swatches` draws nothing (1374), with how the strip takes the keyboard; `smart.rs`'s keys (129) and its `walk` (266); `data.rs` `MenuData` and `read_tags`; `event.rs` `SetLabel` to `ClearMarks` (144); the tests PLAN §10 names (`tests.rs` from 394, `tests_content.rs` around its Tags rows); `components/row_facts.rs` `marks_among` (627); glint-model's `marks::{shown, Label}`.
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

**Check with:** `cargo test -p glint-ui components::menus`, `cargo test -p glint-ui components::command_bar` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient.)

**What is there for you:** the five events (`SetLabel { paths, label }`, `EditComment { paths }`, `CopyMarks { path }`, `PasteMarks { paths }`, `ClearMarks { paths }`) with their keys in `smart.rs` (`do:label`, `do:comment`, `do:copy-marks`, `do:paste-marks`, `do:clear-marks`), which the workspace already hands to `crate::marking` (card E5 fills it) and to Edit comment… (card E3). **You only build the rows and emit the events.** A row's key is never renamed: `do:tags` and `do:edit-tags` stay in `smart.rs` even where no row uses them now.

## What to build

### data.rs: the menu knows the items' marks

`MenuData` gains `marks: HashMap<String, Marks>` (by path in lower case, from `RowFacts::marks_among`) and `labels: Vec<Label>` (`state::settings(cx).labels`), both filled by `read_tags`, which keeps its signature and still fills `tags`. **Nothing is read from the disk to open a menu**: an item not read yet is missing from the map and counts as having no marks. Opening a menu stays under a millisecond.

### The row of dots (F7)

In an item's menu, **right under the strip of icon buttons**, `Node::Swatches`: one entry a label, in the labels' order, then one for **No label**.

- A label's entry is named by the label (`label.name`), runs `MenuEvent::SetLabel { paths, label: Some(name) }`, and is checked when **every** item of the menu is shown with that label (`shown(labels, marks)`, so a 0.16 color tag counts). The handler takes the label off when they all have it: the menu only says which label was pressed.
- **No label** runs `SetLabel { paths, label: None }`, named `t!("No label")`.
- `view.rs` draws it: a row of round dots in `theme::label_tone(label.color)` (the last one a hollow ring in the muted text color), a checked one with the kit's check mark or ring that says "on" without the accent filling the dot, each a button with its label's name as its accessible name and tooltip, reached and pressed from the keyboard exactly as the strip's buttons are (arrows along it, Enter). If `Entry` has no place for a color, give it one in a way that breaks no caller (a field with a default, set through a method).
- `smart.rs`: `walk` goes on skipping `Swatches` (Suggested never copies a dot), and the row never moves.
- A menu for items that can't be marked (whatever the Tags submenu is left out for today) has no row of dots either.

### Marks, under More (F7)

In place of the Tags submenu with its seven color rows: a submenu **Marks** (`t!("Marks")`, the tag icon) with, in this order:

| Row | Event |
| :--- | :--- |
| `Tags…` | `EditTags { paths }` |
| `Comment…` | `EditComment { paths }` |
| (separator) | |
| `Copy marks` | `CopyMarks { path }`: the item the menu was opened on (the first of `paths`) |
| `Paste marks` | `PasteMarks { paths }` |
| `Clear marks` | `ClearMarks { paths }` |

The rows are always there (a menu's rows never move once it is on screen, and nothing here waits for the clipboard): *Copy marks* and *Clear marks* are disabled when no item of the menu has a mark that is known.

### The tests that change

PLAN §10 allows exactly these to change: the tests that list the Tags submenu's rows (`tests.rs` from 394 on, `tests_content.rs`'s rows) now list Marks' rows and the row of dots. Every other test of the menus passes unchanged.

## Tests (STANDARDS §5.1 "Tests")

1. An item's menu has the row of dots right after the strip: seven labels and No label, in the settings' order, each named.
2. The dot of a label every item has is checked; with one item lacking it, none is; a 0.16 file tagged `red` checks Red; an item not read checks nothing.
3. Pressing a dot asks `SetLabel` with the menu's paths and that label; No label asks it with `None`.
4. More → Marks has the five rows in order, each asking its event; Copy marks names the first path; Copy and Clear are disabled when nothing is marked.
5. The dots take the keyboard as the strip does (the arrows move along the row, Enter presses), and each has a role and a name.
6. Suggested never copies a dot, and `do:label`, `do:comment`, `do:copy-marks`, `do:paste-marks`, `do:clear-marks`, `do:tags` and `do:edit-tags` are still the events' keys.
7. Right to left, with a label named in Arabic: the dots run from the start edge and the names are theirs.
8. `read_tags` fills tags, marks and labels from what `RowFacts` holds and calls nothing.
9. The command bar's tests (`command_bar/tests.rs` matches `Node::Swatches`) still pass.

## Done when

The tests pass, clippy is clean, and the only tests changed are the ones that listed the Tags submenu. Say in your report every new string, every test you changed and why, and that nobody has seen the dots on a screen yet.
