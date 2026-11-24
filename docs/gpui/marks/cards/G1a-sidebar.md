# Card G1a — The sidebar's Labels, and a tag's own menu

Wave 4 · crate `glint-ui`

**You own:** `crates/glint-ui/src/components/sidebar/mod.rs`, `sidebar/rows.rs` and `sidebar/tests.rs`. Not `portals.rs`, `tree.rs`, `workspaces.rs` or `drop.rs`.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M4, M6, M11), §4 (F1, F12, F17), §5.5, §5.6, §6.2, and under §9's table "Wave 3, as it landed" (what is left for G1). Then the code: `sidebar/rows.rs` (`Tag` 124, `shown_tags` 137, `tag_search` 159, `Row` 168, `Inputs` 255, `reads_files` 365, `build_rows` 382 with the Tags section at 462, the tests from 814); `sidebar/mod.rs` (`TagsSource` 124, `Sources` and `Sources::core` 150 with `tags` at 181, the hub's subscription 282, `read_tags` 663, `open_menu` 783, `keyboard_menu` 809, `draw_tag` 1542); `sidebar/portals.rs:107` (`open_portal_menu`: a row's own popup menu, the pattern for a tag's); `sidebar/tests.rs` (how its fakes build `Sources`); `dialogs/tags.rs` (`tag_color` 56, `hub`, `TagsChanged`, `set_tags` and `set_marks`); `dialogs/mod.rs` (`show` 185, `confirm` 264); glint-core's `marks::marks_all` and `Counts` (`marks/mod.rs:40`, `:321`: the index's counts merged with the ledger's, labels and tags apart); `index.rs:74` (`index_count`: how a search is asked of the index from core) and `find.rs` (`find_files`: how paths are found); glint-model's `marks::{Label, label_of}`; `theme::label_tone`.
## The rules for a card that draws (STANDARDS §5 is the law; these are where such cards went wrong before)

- **The kit and the theme only.** No `gpui_component` in a view, no color or size of its own. A label's own color is `theme::label_tone(label.color)`, a wash `theme::label_wash(label.color, cx)`.
- **A label is not a state** (M11): never in the accent, never the whole line's text in color. Anything with a selected state shows its selection alone.
- **Every string through `t!`**, as PLAN §4 and §7.2 word it where they word it. List every new string in your report.
- **Everything that can be pressed has a role and a name**; keyboard alone works (focus order, Enter, Escape).
- **Right to left:** start and end helpers only, and a test with an Arabic name, tag and comment in everything that lays text out (`testing::set_rtl`).
- **Nothing on the UI thread but reading what is in memory.** Marks come from `RowFacts` (`components/row_facts.rs`); changes go through `dialogs::tags::set_marks(window, cx, paths, change)` (Undo, `TagsChanged`), never straight to glint-core from a view. Core is reached through a source or backend the view takes (a boxed async fn, on `core_bridge::run`), so tests inject a fake.
- **Only what is known is drawn** (M4): no placeholder, no skeleton.
- A list's test window must be wide enough for its columns (`file_list::layout::leave_room` takes optional columns out while the name would have less than 14 rem).
- A test that lists a menu's rows or walks it with the arrows changes when a row is added: change it, and say so.

**Check with:** `cargo test -p glint-ui components::sidebar` and `cargo clippy -p glint-ui --all-targets -- -D warnings`.

## Labels above Tags (F17)

- `Sources` gains `marks`: a boxed async fn answering `(labels: Vec<(String, u64)>, tags: Vec<(String, u64)>)`, in the app `glint_core::marks::marks_all()`. `read_tags` asks it in place of `tags` (which stays in `Sources` for the fakes that set it: when `marks` answers an error, `tags` is asked as before). **`sidebar/tests.rs`'s tests pass unchanged** but the ones this card names, so give `marks` a default that fails where the fakes build `Sources` by hand.
- **A section `Labels`**, above `Tags`: one row a label of `state::settings(cx).labels`, **in their order**, each with its dot in `theme::label_tone(label.color)`, its name, and its count when the index says one (its own count, plus the counts of its `was` names, plus a tag of its name: a 0.16 color tag is that label, §5.6). A label nothing has shows no count (M4: only what is known). No label in the settings, no section.
- A press on a label finds what has it on every drive: `label:"Name"` at This PC (`label_search`, beside `tag_search`).
- **Tags** no longer lists a tag that is a label's name (it is in Labels).
- A label's row is a `Row::Label` of its own: focusable, with a key, in the keyboard's walk, a role and a name (`t!("Label {name}, {n}")`), as a tag's row has.
- **`reads_files` loses `tag:`** (the index answers it now), so a saved search by tag or label is counted. Its two tests (`:857`, `:1061`) change for exactly that.

## A tag's row gets a menu (F17)

Right-click, the menu key and Shift+F10 on a `Row::Tag` open a popup of two rows (as `open_portal_menu` opens a portal's):

- **`t!("Rename everywhere…")`**: a small dialog (`dialogs::show`), title `t!("Rename the tag “{tag}”")`, one box with the name, `t!("Rename")` and Cancel. Renaming is `dialogs::tags::set_tags(window, cx, paths, vec![new], vec![old])` over every item that has the tag: **one Undo step**, the hub's own notice.
- **`t!("Take off every file…")`**: `dialogs::confirm` asking `t!("Take “{tag}” off {n}?")` (`plural(n, "item")`), then `set_tags(.., vec![], vec![tag])`.
- The items come from a source, `Sources.tagged`: a boxed async fn from a tag to the paths that have it (in the app a search for `tag:"name"` on every drive, at most 100,000: write it with what glint-core has, and say what you used). It answers before the dialog shows its number. None found: a notice `t!("Nothing has the tag “{tag}” where Glint can search")`.
- A new name that is empty, the same, or has a line break is refused in the dialog (`t!("Give the tag another name")`).

## Tests (in `sidebar/tests.rs` and `rows.rs`'s own)

1. Labels come above Tags, in the settings' order, each with its count; a renamed label (`was`) takes its old name's count; a tag named as a label is counted with the label and not listed under Tags.
2. With no counts known the labels are listed without numbers; with no labels in the settings there is no section.
3. A press on a label runs the search `label:"Important"` at This PC.
4. `reads_files` is false for `tag:"Work"` and `label:red`, still true for `width:>1000`.
5. The menu key on a tag's row opens the two rows; *Rename everywhere…* with a new name asks `set_tags` (the hub's fake backend) once with the paths the source gave, adding the new and removing the old; an empty or unchanged name asks nothing.
6. *Take off every file…* asks with the number, and on yes removes the tag from those paths; on no, nothing.
7. The source answering no paths says the notice and opens no dialog.
8. The keyboard walks label rows as it walks tag rows; every row has a role and a name; with Arabic label and tag names the rows mirror.
9. `TagsChanged` reads the counts again.

## Done when

The tests pass, clippy is clean, the three files are the only ones changed, and no test outside this card's list changed. Say in your report every new string, what the `tagged` source uses, and that nobody has seen it on a screen.
