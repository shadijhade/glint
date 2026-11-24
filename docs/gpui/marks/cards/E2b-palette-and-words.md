# Card E2b — Ctrl+K's commands, plain words that know the labels, the lists of filter keys

Wave 3 · crate `glint-ui` · the palette and the search boxes

**You own:** `crates/glint-ui/src/components/palette/commands.rs` (and the palette's tests, `palette/tests.rs`); `components/address_bar/find.rs` and `address_bar/search.rs`; `components/quick_search/mod.rs` (the plain words' line, 337, and what it needs); and **only the hand-kept lists of filter keys** in `explorer/inside.rs` (around 77) and `components/sidebar/rows.rs` (`reads_files`).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) F9, F12, F13 and F16 in §4, §5.5, §6.5, and "Wave 2, as it landed" under §9's table. Then the code: `palette/commands.rs` (how a command is listed and what it runs; how commands that act on the selection find it and when they are offered; Group by's commands at 797); `crate::marking` (`marking.rs`: the eight functions you call; card E5 fills them at the same time); `address_bar/find.rs` around 48; `address_bar/search.rs` around 65 (`KEYS`); `quick_search/mod.rs` 337; `explorer/inside.rs` 60 to 135; glint-model's `plain::plain_query_with` and `marks::KEYS`.
**Check with:** `cargo test -p glint-ui components::palette`, `components::address_bar`, `components::quick_search`, `explorer::inside` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. (Slow crate, shared build lock: be patient.)

## What to build

### The commands of F9 (`palette/commands.rs`)

Listed with the commands that act on what is selected, offered when they are (a selection in the pane the keyboard is in), each through `t!`:

| Command | Runs |
| :--- | :--- |
| `Label: {name}`, one for each label of `state::settings(cx).labels`, in their order | `marking::set_label(paths, Some(name), window, cx)` |
| `No label` | `marking::set_label(paths, None, window, cx)` |
| `Edit tags…` | `marking::edit_tags(window, cx)` |
| `Edit comment…` | `marking::edit_comment(window, cx)` |
| `Copy marks` | `marking::copy_marks(path, window, cx)`: the focused item, else the first selected |
| `Paste marks` | `marking::paste_marks(paths, window, cx)` |
| `Clear marks` | `marking::clear_marks(paths, window, cx)` |

Run them the way the file's other commands run theirs (if a command is data that the palette or the workspace executes, add what that needs **in your file**, and say in your report what the coordinator must wire outside it). Each shows its key from `actions::BINDINGS` where it has one (`Alt+1`…, `Alt+0`, `Alt+T`, `Alt+N`), as other commands show theirs. *Import marks…* and *Export marks…* are wave 4's: not now. The words a person types find them: "label", "tag", "comment", "mark", a label's own name.

### Plain words know the labels (F16)

`address_bar/find.rs` and `quick_search/mod.rs` call `plain_query_with(text, &names)` in place of `plain_query(text)`, the names being the settings' labels' names. Read them where the settings are at hand without a new read a keystroke that allocates more than the names themselves; quick search runs in a window of its own: use what it can reach (`state::settings`), and if it can't reach the settings, pass no labels there and say so.

### The lists of filter keys read `marks::KEYS` (§6.5)

Each place that keeps the filter keys by hand takes the marks' keys from `glint_model::marks::KEYS` (`label`, `lbl`, `tag`, `tags`, `comment`, `cmt`, `has`), with `!` before one read as the same key where the place reads a term's key:

- `address_bar/search.rs`'s `KEYS` (what the box knows as a filter while typing);
- `explorer/inside.rs`'s lists (which terms are filters and not words to look for inside files: a marks term is never searched for as text);
- `sidebar/rows.rs` `reads_files`: leave it as it is unless a marks key belongs there (the index answers marks, so they don't make a search "read the files"); say what you decided.
- `quick_search/search.rs` keeps no list any more (every `key:` word is left out of the words): nothing to do there; say so.

## Tests (STANDARDS §5.1 "Tests")

1. With something selected the palette lists `Label: Red` … `Label: Gray` in order, `No label`, `Edit tags…`, `Edit comment…`, `Copy marks`, `Paste marks`, `Clear marks`; with nothing selected, none of them (or as the file's other selection commands behave: say which).
2. A label renamed in the settings shows under its new name; typing `important` finds `Label: Important`, typing `comment` finds `Edit comment…`.
3. `Label: Red` shows `Alt+1`, `No label` `Alt+0`, `Edit tags…` `Alt+T`, `Edit comment…` `Alt+N`.
4. Each command runs what the table says (test what can be tested while `marking`'s bodies are empty: that the command is there, is the right kind, and names the right paths and label).
5. The address bar offers `label:important ext:pdf` for `important pdfs` when a label is named Important, and `important ext:pdf` when none is; `tagged netplan` offers `tag:netplan`.
6. `label:red`, `LBL:red`, `!tag:old`, `has:comment` and `cmt:x` are filters to the address bar's box and are not words to look for inside files; `ext:pdf` and a plain word are what they were.
7. An Arabic label name is listed, found by typing it, and mirrors (`testing::set_rtl`) as the palette's other rows do.
8. Every test of the palette, the address bar, quick search and `explorer::inside` passes unchanged.

## Done when

The tests pass, clippy is clean, and nothing typed in a search box costs a read of the disk. Say in your report every new string, what you did with each of the four lists, and what the coordinator must wire.
