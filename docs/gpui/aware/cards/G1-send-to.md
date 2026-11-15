# Card G1 — Move or copy to, and the usual place

Wave 4 · crate `glint-ui` · a submenu, built as data

**You own:** `crates/glint-ui/src/components/menus/send_to.rs`, `menus/tests_send_to.rs`; one marked place in `menus/item.rs` (the comment `// G1: "Move or copy to" goes here.`), one marked place in `menus/mod.rs` (the comment `// G1: the usual place goes in this Vec`); and in `menus/tests.rs` only the expected lists that gain the one line named below.
**Read first:** PLAN.md §2 (R3, R8), §3 (F5), §5.1 (`Place`, `usual_place`), §5.5; [mock.html](../mock.html) (the second menu); `menus/spec.rs`; `menus/item.rs` (how a submenu is added: "Compress to" around line 522); `menus/smart.rs` (`dress`'s `extra`); `crates/glint-ui/src/aware/mod.rs` (`places_for`); `crates/glint-model/src/aware/shape.rs` (`shared_ext`).
**Tests' harness:** `use super::tests::*;`.
**Check with:** `cargo test -p glint-ui components::menus` and clippy on the crate.

`ItemCtx` has a field `places: Vec<Place>` (the coordinator added it; empty by default): the folders this kind of thing has gone to before, heaviest first. The window already handles the two events you emit (`MenuEvent::SendTo { paths, folder, moving }` moves or copies there and counts the place; `MenuEvent::ChooseSendTo { paths, moving }` asks for a folder first).

## send_to.rs

```rust
/// The most places listed under each heading.
pub const MOST: usize = 5;
/// The "Move or copy to" submenu for `paths`, now in `folder`.
pub fn menu(paths: &[String], places: &[Place], folder: &str) -> Node;
/// The row for Suggested when one place is where this kind of thing usually goes.
pub fn usual_row(paths: &[String], places: &[Place], ext: Option<&str>, folders_only: bool, folder: &str) -> Option<Node>;
```

**`menu`**: `Node::menu(t!("Move or copy to"), IconName::FolderOutput, …)` holding, in order:
1. `Node::Heading(t!("Move to"))`;
2. a row for each of the first `MOST` places that aren't `folder` itself (compare ignoring case): `Entry::ask(name, IconName::FolderOutput, MenuEvent::SendTo { paths, folder: place.folder, moving: true })`, where `name` is the place's base name (the whole path for a drive's root); its `hint` is the place's parent folder (`glint_model::paths::parent_path`), so two folders of one name can be told apart;
3. `Entry::ask(t!("Choose a folder…"), IconName::FolderSearch, MenuEvent::ChooseSendTo { paths, moving: true })`;
4. `Node::Heading(t!("Copy to"))`, the same places with `IconName::CopyPlus` and `moving: false`, and its own `Choose a folder…` (`moving: false`).

With no places each heading has only its "Choose a folder…".

**`usual_row`**: `rank::usual_place(places)`; `None` when there is none or it is `folder` itself. Else a row `Entry::ask(t!("Move to “{name}”", name = base name), IconName::FolderOutput, MenuEvent::SendTo { …, moving: true })` with `smart = true` and `reason`:
- `ext` given: `Where you usually put {ext} files` (the extension as it is, lower case: "zip files");
- `folders_only`: `Where you usually put folders`;
- else: `Where you usually put files like these`.

## The two marked places

- **`item.rs`**, at the comment: `if !fixed { m.push(Some(send_to::menu(&paths, &c.places, &c.folder))); }`. Not in the Recycle Bin's menu, not for `fixed` items.
- **`mod.rs`**: the host builds an item menu in `item_nodes` and dresses it in `dressed`, which ends with the marked line `smart::dress(nodes, head(), &picks, facts.shape, Vec::new())`. Give `dressed` one more parameter, `extra: Vec<Node>`, passed on to `smart::dress` in place of that `Vec::new()` (the folder menus' calls pass `Vec::new()`). In `item_nodes`, before `item_menu` is called, set `ctx.places = aware::places_for(cx, &ext)` where `ext` is `shared_ext(&chosen)`, or `""` when every target is a folder, and leave `places` empty for anything else (a mix has no one place); then pass `send_to::usual_row(&paths, &ctx.places, ext, folders_only, &ctx.folder).into_iter().collect()` as `dress`'s `extra`. `MenuHost::suggestions` (the island) passes no extra: the island offers only what a rule or a habit of actions puts first.

## What changes in the old tests (`menus/tests.rs`)

Every expected list of a whole item menu for items that aren't `fixed` gains `"Move or copy to>"`: after `"Move to the other pane"`-style lines when two panes show, and otherwise as the first line of the section that begins after "Show in File Explorer" (that is: right after that section's `"-"`). Nothing else changes.

## Tests (`tests_send_to.rs`)

Places by hand: `Place { folder: r"D:\Papers\Invoices".into(), uses: 4, weight: 4.0, share: 0.8 }`.

1. With no places the submenu is `# Move to`, `Choose a folder…`, `# Copy to`, `Choose a folder…`, and the two rows carry `moving: true` and `moving: false`.
2. Two places give two rows under each heading, heaviest first, named by their base names, each row's event carrying the paths, the place's whole folder and the right `moving`.
3. Seven places list five; the folder the items are in is left out.
4. A row's `hint` is its parent folder.
5. `usual_row`: a place with 4 uses and a share of 0.8 gives "Move to “Invoices”" with "Where you usually put pdf files"; for folders, "Where you usually put folders"; with no extension, "…files like these"; a share of 0.5, or the folder the items are in, gives nothing.
6. In a window with a saved habit (three `aware::note_place(cx, &["pdf".into()], r"D:\Papers\Invoices")` calls), a PDF's menu has "Move to “Invoices”" under "Suggested" and "Invoices" inside "Move or copy to"; with `settings.learn` off it has neither, and the submenu still has its two "Choose a folder…" rows.
7. Choosing "Invoices" under "Move to" tells the window `MenuEvent::SendTo { moving: true, .. }` with that folder.
8. An item inside an archive has no "Move or copy to".

## Done when

Every test under `components::menus` passes, clippy is clean.
