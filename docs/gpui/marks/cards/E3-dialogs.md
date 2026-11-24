# Card E3 — Edit tags… suggests as you type; Edit comment…

Wave 3 · crate `glint-ui` · two dialogs

**You own:** `crates/glint-ui/src/dialogs/tags.rs` **from the line `// ---- The dialog's model` (467) down to its tests**, and the tests of the dialog in that file's test module (the hub's tests above them are card D1's: leave them); and `crates/glint-ui/src/dialogs/comment.rs` (a stub with `open(window, cx, paths)` is there: keep that signature, the workspace and `marking` call it). The hub's part of `tags.rs` (everything above that line) is not yours: call it.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M6, M10), F2, F3 and F8 in §4, §5.1 (the caps), §6.2, §13 (the keys), and "Wave 2, as it landed" under §9's table. STANDARDS §5.1 "Dialogs", "Models" and "Tests" matter here; `dialogs/drive_health.rs` is the reference dialog. Then the code: `dialogs/tags.rs` from 467 (`TagsEdit`, `TagsDialog` 532, `open` 726), `offered` (113) and the dialog's tests (743 on); the hub above it: `Backend` (`read_marks`), `set_marks` (414), `Tags::recent`; `dialogs/mod.rs` `show` (185); `components/preview/quick_look.rs` around 240 (how a dialog opens out of an item: `Dialog::opens_from` with `stage::spot_of`); `components/row_facts.rs` `marks_among` (627); glint-core's `marks::marks_all` and `Counts`; glint-model's `marks::{Change, Marks, COMMENT_CAP}`.
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

**Check with:** `cargo test -p glint-ui dialogs::tags`, `cargo test -p glint-ui dialogs::comment` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient.)

## Edit tags… suggests (F2)

Under the box, as the person types the tag being typed (the text after the last comma), up to eight suggestions, each a row that can be pressed or chosen with the arrows and Enter (Tab takes the first):

1. tags that **start with** what is typed, then tags that **contain** it (any case);
2. within each, **those used in this folder first** (the tags among what `RowFacts` holds for the rows of the tab the keyboard is in: `marks_among` over that tab's entries, at most the first 2,000), then the rest **the most used first**;
3. tags the items already have, and tags already typed in the box, are left out.

Where the tags and their counts come from is a **source** the dialog's model takes (a boxed async fn, so tests inject one): in the app `glint_core::marks::marks_all()` through `core_bridge::run`, asked once when the dialog opens; **when it answers nothing or fails (instant search is off) the recent tags stand in** (`Tags::recent`), in their order. Nothing is asked per keystroke: suggestions are worked out from what is in memory. Choosing one replaces the tag being typed with it and a comma, and the box keeps the focus. With nothing typed the offered tags are what they are today.

Saving changes tags through `set_tags` as it does now (which goes through `set_marks` and remembers Undo). Every test the dialog has passes unchanged.

## Edit comment… (F3), `dialogs/comment.rs`

`open(window, cx, paths)` shows a dialog titled `t!("Comment")` for one item (its name in the title as Edit tags… does it) or `t!("Comment of {n}")` with `plural(n, "item")` for several.

- **A model** (`State { Loading, Ready, Failed }`) reads the items' marks through the tags hub's backend (`tags::hub(cx).read(cx).backend().read_marks(paths)`, on `core_bridge::run`: tests give the hub a fake). One item: its comment. Several: the comment they **all** share, else an empty box with the placeholder `t!("Several comments")`.
- **A box of several lines** (the kit's multi-line input; about six lines tall, growing to twelve), with the accessible name `t!("Comment")`. **`Enter` is a new line, `Ctrl+Enter` saves, `Escape` leaves** without saving. A **Save** button (the main one) and **Cancel**.
- Under the box, at its end, how much is used once it matters: from 60 KB on, `t!("{used} of 64 KB")` (`format_size`).
- **More than 64 KB (`COMMENT_CAP` bytes of UTF-8) is refused with words before anything is asked of core**: Save is disabled and the line under the box says `t!("A comment can be 64 KB at most")`.
- **Save**: an empty box (or only white space) is `Change::Comment(None)`, else `Change::Comment(Some(text))`, through `dialogs::tags::set_marks(window, cx, paths, change)` (Undo, `TagsChanged`, a failure's words are the hub's). For several items whose comments differ, saving an **untouched** empty box changes nothing and just closes. Then the dialog closes.
- A read that failed says why (as Edit tags… does) and saving still sets the comment.

## Both open out of the row

Both dialogs open out of the item they are about (`Dialog::opens_from(bounds)`, the bounds from `stage::spot_of` for the first path, as Quick Look does it). `dialogs::show` is not yours: if it can't take the bounds, open with it as today and **say in your report what `dialogs/mod.rs` needs**.

## Tests (STANDARDS §5.1 "Tests"; fakes only)

Edit tags…:
1. Typing `ne` with the known tags `netplan` (9), `network` (3), `internet` (20), `work` (50) suggests `netplan`, `network`, then `internet`; with `network` used in this folder it comes first.
2. Tags the items have and tags already in the box are not suggested; at most eight show.
3. Choosing a suggestion (pressed, or arrows and Enter) puts it in the box with a comma and keeps typing possible; the box's other tags stay.
4. With a source that fails, the recent tags are suggested; with nothing typed the dialog offers what it offered before.
5. The suggestions have a role and names, and an Arabic tag is suggested and mirrors.
6. The dialog's tests there are pass unchanged.

Edit comment…:
7. One item with a comment of three lines: the box holds it; edited and saved with `Ctrl+Enter`, the backend is asked `Change::Comment(Some(new))` for that path, and the dialog is closed.
8. `Enter` adds a line and does not save; `Escape` closes and asks nothing.
9. Emptied and saved: `Change::Comment(None)`.
10. Three items with the same comment show it; with different ones the box is empty with *Several comments*, an untouched save asks nothing, and a typed one sets it on all three.
11. A comment over 64 KB: Save is disabled, the words are shown, and the backend is never asked; at 64 KB exactly it saves.
12. A read that fails shows why and a save still sets the comment.
13. The box and the buttons have names; an Arabic comment of several lines renders, mirrored, in a dialog titled with an Arabic file name.

## Done when

The tests pass, clippy is clean, `open(window, cx, paths)` keeps its signature in both files, the hub's part of `tags.rs` is untouched, and nothing is read or written on the UI thread. Say in your report every new string, what `dialogs/mod.rs` needs for `opens_from`, and that nobody has seen either dialog on a screen yet.
