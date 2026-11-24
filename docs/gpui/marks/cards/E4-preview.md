# Card E4 — The Marks block in the preview pane and Quick Look

Wave 3 · crate `glint-ui` · one new view

**You own:** `crates/glint-ui/src/components/preview/marks.rs` (new: the module line is there; tests at its bottom) and **marked places**: in `preview/frame.rs` the one place the block goes in (right before the details, `.child(self.details(&t))` at 795, with the field that holds its entity), and in `preview/pane.rs` the one place several selected items are shown (`Shown::Many`), where the block goes in for all of them. Nothing else of those two files.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M6, M11), F8 and F20 in §4, §5.4, §5.5, §5.6, §6.2, and "Wave 2, as it landed" under §9's table. STANDARDS §5.1 "Models" and "Tests" matter here. Then the code: `preview/frame.rs` `EntryView` (124), its `new`, its `render` and `details` (802); `preview/pane.rs` (`Many`, `Shown`, how the pane follows the selection); `preview/quick_look.rs` (it shows an `EntryView`; which keys its context binds); `dialogs/tags.rs` `tag_color` (56), the hub (`hub`, `TagsChanged`, `set_marks` 414, `Backend::read_marks`); `components/row_facts.rs` `marks_among` (627); glint-core's `marks::home_of` (141 of `glint-core/src/marks/mod.rs`: it asks the volume, so never on the UI thread); glint-model's `marks::{Change, Marks, Home, shown, COMMENT_CAP}`.
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

**Check with:** `cargo test -p glint-ui components::preview::marks`, `cargo test -p glint-ui components::preview` once at the end, and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient.)

## What to build

`pub struct MarksBlock`, an entity made with the paths it is about (one item in an `EntryView`, the selection in the pane's deck), shown at the head of the details for **every kind of item** (files, folders; not for an item in an archive, in the Recycle Bin or on another computer, which the rows don't read either: no block there).

**What it shows comes from memory**: the items' marks from `RowFacts::marks_among`; for paths it doesn't hold yet, one read through the tags hub's backend (`read_marks`, on `core_bridge::run`), never the disk from the view. It follows `TagsChanged` (the hub's event) for its paths and reads them again; while nothing is known it draws **nothing** (M4), not a skeleton.

1. **The labels' dots**: one a label of the settings, in `theme::label_tone`, the one the items are shown with marked as on (not with the accent filling it); pressing a dot sets that label, pressing the one that is on takes it off: `set_marks(.., Change::Label(..))`. Each dot is a button named by its label.
2. **The tags as chips** (a dot in `tag_color` and the name, each with a small remove button named `t!("Remove tag {tag}")`), then **a box to add one** (placeholder `t!("Add a tag")`): Enter or a comma adds it (`Change::Tags { add, remove }`).
3. **The comment in a box that grows from one line to six** (placeholder `t!("Add a comment")`, accessible name `t!("Comment")`). **It saves when the focus leaves it, or on `Ctrl+Enter`; `Escape` puts back what was there** and keeps the focus in the pane. An unchanged box saves nothing. More than 64 KB is not sent: the box says `t!("A comment can be 64 KB at most")` and keeps the text.
4. **Several items**: the label they all share (else none is on), the tags they **all** have, and the comment they all share, else the placeholder `t!("Several comments")` (typing there and leaving sets it on all; untouched, nothing is saved).
5. **Where the marks are kept, said quietly** (F20), one line in the muted text under the block, only when the item has marks and they are not in the file: `t!("Kept beside the file")` for `Home::Beside`, `t!("Kept on this PC")` for `Home::Ledger`. The home comes from a **source** the block takes (a boxed async fn from a path to its `Home`; in the app `glint_core::marks::home_of` on a background thread, asked once per item shown and again after a change), so tests inject it and the UI thread never asks a volume.

Every change goes through `dialogs::tags::set_marks`, so each is one Undo step with its notice.

**In Quick Look the keys it binds must not fire while one of the block's boxes has the keys** (Space closes Quick Look; a space typed in a comment must be a space). Check how its context is set and make sure a focused input is outside it; test it.

## Tests (STANDARDS §5.1 "Tests"; the hub with `row_facts`'s `fake::MarksBackend`, a fake home source)

1. An item labeled Red, tagged `bond`, `netplan`, with a comment: the Red dot is on, two chips, the box holds the comment; an item with no marks shows the dots, no chip, the two empty boxes; an item whose marks aren't known yet draws nothing.
2. Pressing Green asks `Change::Label(Some("Green"))`; pressing Red (on) asks `Label(None)`.
3. Typing `work` and Enter in the tag box asks `Tags { add: ["work"], remove: [] }` and empties the box; a chip's remove button asks for that tag's removal.
4. The comment: edited and the focus leaving it asks `Change::Comment(Some(new))` once; `Ctrl+Enter` too; `Escape` puts the old text back and asks nothing; unchanged and left, nothing; emptied, `Comment(None)`; over 64 KB, nothing is asked and the words show.
5. Three items: the shared label is on, only the tags all three have are chips, different comments show *Several comments*; a comment typed is set on all three.
6. `Kept beside the file` shows for a marked item whose home is `Beside`, `Kept on this PC` for `Ledger`, nothing for `Stream` or for an item with no marks.
7. A mark changed elsewhere (`TagsChanged` for the path) shows in the block once read again, without touching a box the person is typing in.
8. In Quick Look, a space typed in the comment box is a space and Quick Look stays open.
9. Everything pressable has a role and a name; with an Arabic name, tag and comment the block mirrors and nothing overlaps.
10. The preview's tests there are pass unchanged; an item in an archive has no block.

## Done when

The tests pass, clippy is clean, the two marked places are the only lines changed outside `marks.rs`, and the block never reads the disk from the UI thread. Say in your report every new string, the two places you changed, and that nobody has seen the block on a screen yet.
