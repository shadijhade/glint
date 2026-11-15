# Card E1 — The menu's view: the head, reasons, typing, the slide

Wave 2 · crate `glint-ui` · drawing and keys

**You own:** `crates/glint-ui/src/components/menus/view.rs` and `menus/tests_view.rs`.
**Read first:** STANDARDS.md §5.1 (all of it; "Accessibility", "Right to left", "Style", "Depth", "States", "Motion", "Tests" matter most here); PLAN.md §2 (R2, R9), §3 (F1 to F4, F7), §5.4; [mock.html](../mock.html) in a browser (the three menus are the target); `menus/view.rs` as it is (you are changing it); `menus/spec.rs` (`Head`, `Badge`, `Tone`, `HeadPicture`, the new `Entry` fields: the coordinator added them); `menus/filter.rs` (`matches`, `Found`: card E3 fills it in at the same time as you; until it lands it finds nothing, so write your filter tests against a menu and expect them to pass once E3 is in; say so in your report); `crates/glint-ui/src/art.rs` (`kind`, `folder`, `page`) and how `components/file_list/cells.rs` calls `art::folder` (use the same color for a plain folder).
**Tests' harness:** `use super::tests::*;` gives you `open`, `disk`, `Ui` (`open_menu`, `press`, `click`, `nodes`, `logged`, `events`, `is_open`, `select`), `labels`, `find`, `at`, `ALPHA`, `DOCS` (see `menus/tests.rs` from line 1100).
**Check with:** `cargo test -p glint-ui components::menus` (all the menus' tests: the old ones must still pass unchanged) and clippy on the crate.

## 1. The head (`Node::Head`)

Drawn first, above the strip. It is not a stop for the arrow keys. Its height never changes while the menu is open (R2).

- A row: the picture, 40 px, then a column of three lines.
  - Picture: `HeadPicture::Kind(kind)` is `art::kind(kind, px(40.), cx)`; `Folder` is `art::folder` at 40 px in the plain folder's color; `Several` is `art::page(px(40.), t.muted_foreground, Some(IconName::Files), cx)`.
  - Line 1: `head.title`, `theme::size::SM`, medium weight, one line, cut with an ellipsis, through `isolate`.
  - Line 2: `head.line`, `theme::size::XS`, `t.muted_foreground`, one line, cut, through `rtl_line`.
  - Line 3: always 20 px high, a row: the badges, then `head.changed` (`theme::size::XXS`, muted, cut) in the room that is left. It is 20 px high even when both are empty.
- A badge: a pill 20 px high, `px(space(2.))`, fully rounded, `XXS`, medium weight, its icon at 11 px before its label. `Tone::Plain`: `t.accent` behind, `t.foreground`. `Tone::Warn`: `t.warning` text on `t.warning.opacity(0.13)`. `Tone::Good`: `t.success` on `t.success.opacity(0.13)`.
  - At most three badges are drawn. With more, the third place is a `Plain` badge `+{n}` (n = how many are not shown) whose tooltip is their labels joined with ", ".
  - A badge whose `run` is not `Run::Nothing` is pressed with the mouse: pointer cursor, `Role::Button`, `aria_label(label)`, and a click emits `MenuViewEvent::Chose(run)`. Its `tip`, when it has one, is its tooltip. The others are text.
- Under the head: a hairline (`border_b_1`, `t.border`), as the strip has.
- The head's root: `.id("menu-head")`, `Role::Group`, `aria_label(title)`, `aria_description` = the line, then each badge's label, joined with ", "; `.test_support()`.
- `pub fn set_head(&mut self, head: Head, cx: &mut Context<Self>)`: replaces the `Node::Head` of the top level, if it has one, and `cx.notify()`. Nothing else moves: where the keyboard is stays where it is.
- A menu whose top level has a head is at least `rems(17.)` wide (it is 14.5 today).

## 2. Suggested rows, and why a row is greyed

- `entry.smart`: the row's icon is `t.primary` (not the muted tint).
- `entry.reason`: the label and, under it, the reason (`XXS`, muted, one line, cut); the icon sits at the top of the row (`items_start`, a pixel or two down so it lines up with the label). The reason is also the row's `aria_description`.
- `entry.why_not`, on a disabled row: its tooltip and its `aria_description`.
- A `Node::Heading` that comes right after a `Strip` or a `Head` draws no line above itself and no top margin (the strip and the head have their own hairline); anywhere else it is drawn as today.

## 3. Typing (F3)

New state: `query: String`, `found: Vec<filter::Found>`, `found_at: usize`. The keys, in `on_key_down` (Ctrl, Alt and the Windows key still fall through untouched):

| Key | While typing (`query` not empty) | Otherwise |
| :--- | :--- | :--- |
| a key with `event.keystroke.key_char` of visible text | appended to `query` | starts a `query` |
| Space | appended | chooses, as today |
| Backspace | takes the last character off | leaves the submenu, as today |
| Escape | empties `query` | closes the menu, as today |
| Down, Up, Home, End | move through `found`, round the ends | as today |
| Enter | chooses `found[found_at]` (nothing when none) | as today |
| Left, Right | nothing | as today |

After every change to `query`: `found = filter::matches(top level's nodes, &query)` (the top level's, whichever level is showing) and `found_at = 0`. "Visible text" means `key_char` is `Some`, not empty, and has no control character.

While typing, the menu draws, in place of the head, the Back row and the rows:
- a line: the Search icon, the `query` (`SM`, medium), a caret (1.5 px wide, 15 px high, `t.primary`), and "Esc" at the end edge (`XXS`, muted); a hairline under it; `.id("menu-filter")`, `Role::SearchBox` if the kit has it, else `Role::Group`, `aria_label(t!("Filter: {query}", query = …))`;
- each found row, drawn as a plain row is, with its `path` (when it has one) before the label as `{path} › ` in the muted color; its accessible name is `{path} › {label}`, or the label alone; ids `("menu-found", i)`; the one at `found_at` has the selected fill and `aria_active_descendant`; the pointer moving onto one makes it the current one; a click chooses it;
- or, with nothing found, one muted line: `t!("Nothing in this menu matches")`.

Choosing a found row emits `MenuViewEvent::Chose(entry.run)` as any row does.

## 4. The slide between levels (F7)

- The pop the whole menu arrives with plays once, when the menu opens (its id no longer changes with the level).
- The rows (and the Back row) are in an inner column, `.id("menu-rows")`, which is what moves: on entering a submenu it arrives from 10 px toward the end edge; on Back, from 10 px toward the start edge (mirrored when `i18n::is_rtl()`), over `theme::motion::FAST` on `theme::motion::enter`, from an opacity of 0.4. Use `.paint_offset(..)` (STANDARDS: scale by painting). Key the animation by a counter that goes up on every `enter` and `leave`, so it plays once per move.
- Starting or ending typing is not a move: no slide.

## Tests (`tests_view.rs`)

Build menus by hand with `MenuView::new(nodes, "Item menu".into(), cx)` in a window (`testing::open_window`), or through `Ui` where a real menu is wanted. Hold `testing::lock()`.

1. A menu with a head draws it: `window.find_by_label` finds the title; its description holds the line and the badges' labels.
2. The head is not a stop: Down from the strip's last button reaches the first row, never the head; `selected_label()` is never the title.
3. `set_head` with other badges changes what is drawn and leaves `selected_label()` as it was.
4. Five badges draw two and `+3`.
5. A badge with a `run` chooses it when clicked (the event is `Chose(run)`); a badge with `Run::Nothing` has no button role.
6. A row with a `reason` has it as its description; a `smart` row is found by its label like any row.
7. A disabled row with `why_not` has it as its description, and Down skips it.
8. Typing "ren" shows the filter line (found by the label "Filter: ren") and no head; Escape brings the head back and the menu is still open; Escape again closes it (`Dismissed`).
9. Typing then Enter chooses the first found row; Down then Enter chooses the second (two rows whose labels both hold the letters typed).
10. Backspace takes a letter off, and with none left typing has ended (the rows are back); in a submenu with nothing typed, Backspace still goes back.
11. Space with nothing typed still chooses the row the keyboard is on; Space while typing is part of the query.
12. Typing something nothing matches shows "Nothing in this menu matches", and Enter does nothing.
13. In a submenu, typing looks through the whole menu (a top-level row is found).
14. Mirrored (`testing::set_rtl(true)`, set back at the end): the head and a filtered menu still draw, and Left enters a submenu as before.

## Done when

All the menus' tests pass (old and new), clippy is clean, and with every animation landing at once (as in tests) the menu looks the same as at rest.
