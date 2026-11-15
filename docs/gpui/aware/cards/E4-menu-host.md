# Card E4 — The menus use what Glint knows

Wave 3 · crate `glint-ui` · the menu host and the menus' rows

**You own:** `crates/glint-ui/src/components/menus/mod.rs`, `menus/item.rs`, `menus/folder.rs`, `menus/tests_host.rs`; and in `menus/tests.rs` only the expected lists named in "What changes in the old tests" below.
**Read first:** STANDARDS.md §5.1; PLAN.md §2 (R1, R2, R3, R8), §3 (F1, F2, F4, F6), §5.3, §5.4, §8; `menus/mod.rs`, `menus/item.rs`, `menus/folder.rs` as they are; `crates/glint-ui/src/aware/mod.rs` (the functions you call; card D1 has filled them in); `menus/smart.rs` and `menus/head.rs` (card E2 fills them in at the same time as you: their signatures are in the tree; until E2 lands `dress` returns the menu unchanged and `head_of` an empty head, so some of your tests pass only once E2 is in: say which in your report); `crates/glint-ui/src/components/git/model.rs` lines 565 to 610 (`view_of`, `mark_of`).
**Tests' harness:** `use super::tests::*;` (see `menus/tests.rs` from line 1100: `open`, `disk`, `Ui`).
**Check with:** `cargo test -p glint-ui components::menus` and clippy on the crate.

## 1. The host dresses every menu (`mod.rs`)

**Only when the app has its Aware entity** (`aware::try_shared(cx)` is `Some`, as it always is in the running app). Without it the menu is shown exactly as `item_menu` or `folder_menu` made it: that is what keeps every old test in `menus/tests.rs` passing unchanged (they start no entity), and your new tests start one (`aware::init_with`). Write this as one early check in a private `fn dressed(…)` that the three `open_…` functions call.

In `open_list_menu` and `open_entries_menu`, after `item_menu(&chosen, &ctx)`:

```rust
let facts = aware::facts(cx, &chosen);
let one = (chosen.len() == 1).then(|| &chosen[0]);
let probed = one.and_then(|e| aware::try_shared(cx).and_then(|a| a.read(cx).probed(e).cloned()));
let keys = smart::keys(&nodes);
let picks = aware::suggest(cx, &facts, probed.as_ref(), &keys.iter().map(String::as_str).collect::<Vec<_>>(), 3);
let head = head::head_of(&chosen, probed.as_ref(), &self.extras(one, cx));
let nodes = smart::dress(nodes, head, &picks, facts.shape, Vec::new());   // G1: the usual place goes in this Vec
```

- `extras`: `now` is the time in Unix milliseconds; `git` for one target is `Some(t!("Changed"))` when `git::has_changes_view(cx, entry)` says so (look at `git::mark_of` for a new file: `Some(t!("New"))` when its mark says untracked), else `None`.
- With one target on screen, ask for what isn't known, at once: `aware.update(cx, |a, cx| a.look_now(std::slice::from_ref(entry), cx))`.
- `ItemCtx` has a new field, `probed: Option<Probed>` (the coordinator added it): set it from `probed` before calling `item_menu` (section 3 reads it).
- **The folder's own menu** (`open_background_menu`): `head::folder_head(name, Some(tab.rows.len()), branch)` where `name` is the tab's display name (`ex.tab_name(&tab.path)`, as `item_ctx` does for the other pane) and `branch` is `git::view_of(cx, tab.id)`'s branch when there is one; `facts` is `aware::facts(cx, &[])` (shape `Nothing`); picks as above. **A place's or a tile's menu** (`open_folder_menu`): the same with `items: None` and no branch.
- **Not dressed:** a menu with fewer than four nodes (This PC's one row), so it stays as it is; and any menu while there is no Aware entity (above).
- `Open` keeps two more things: `shape: Shape` and `targets: Vec<FileEntry>`.

## 2. While the menu is open

- **The head follows the answers.** In `with_sources`, if `aware::try_shared(cx)` is there, `cx.observe` it (keep the subscription in the host): when it changes and a menu is open for exactly one target, build the head again (`head_of` with what `probed` says now) and `open.view.update(cx, |view, cx| view.set_head(head, cx))`. Nothing else is rebuilt (R2): not the rows, not Suggested.
- **Choosing counts.** In `on_view`, for `MenuViewEvent::Chose(run)`: `if let Some(key) = run.key() { aware::note_action(cx, open.shape, &key) }`, then run it as today.
- **Closing rests.** Whenever the menu closes (`close`, `on_view`): `aware.update(cx, |a, _| a.rest())` if the entity is there.

## 3. For the island (card F1 calls this)

```rust
/// A row put forward for what is selected in `pane`.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggested { pub entry: Entry, pub key: String, pub shape: Shape }
impl MenuHost {
    /// The rows Suggested would hold for the selection of `pane`, best first, at most `most`. None for no
    /// selection, or one of more than 200 items.
    pub fn suggestions(&self, pane: usize, most: usize, cx: &App) -> Vec<Suggested>;
}
```

`chosen = self.chosen(pane, None, cx)`; empty or more than 200: an empty `Vec`. Else build the menu as section 1 does (without reading tags, without `look_now`), take the picks, and give `smart::row_for` of each (with its reason set, as `smart::zone` does) with its key and the shape.

## 4. Why a row is greyed (`item.rs`, `folder.rs`; F4)

Every `.disabled(condition)` on a row or a strip button becomes `.disabled_because(condition, &why)`, where `why: Option<SharedString>` says which of the condition's causes holds. The words, exactly:

| Cause | Words |
| :--- | :--- |
| inside an archive (`archived.is_some()`, `in_archive`, `archive.is_some()`) | `Nothing changes inside an archive` |
| on a remote connection | `Read-only: on a remote connection` |
| a folder Windows keeps closed (`closed`, `shut`, `unlisted`) | `Windows keeps this folder closed` |
| a search's results are shown (`find`) | `Not while a search's results are shown` |
| the other pane can't take items (`!other`) | `The other pane shows nothing that can take items` |
| nothing to undo | `Nothing to undo` |
| already online only (`freed`) | `It is online only already` |

In `item_menu`, work `why_fixed` out once, beside `fixed` (archive first, then remote, then closed), and use it wherever `fixed` is the condition. Where a condition has two causes (`!other || fixed`), the first that holds gives the words. The `disabled` field itself is set exactly as before: no row becomes enabled or disabled by this card.

## 5. Two new rows (`item.rs`; F6)

- **Unblock.** For one file, when `c.probed` says `origin` is `blocked`, and not `fixed`: `Entry::ask(t!("Unblock"), IconName::LockOpen, MenuEvent::Unblock { paths: vec![path] })`, right after "Run in Windows Sandbox" (after "Run as administrator" when that is the last of the two; when neither is there, after "Open with…").
- **Copy the text in it.** For one file that is on disk (`on_disk`), not online only, and whose extension is one of `png jpg jpeg jpe bmp gif tif tiff webp` (a private `const` in `item.rs`): `Entry::ask(t!("Copy the text in it"), IconName::ScanText, MenuEvent::CopyTextIn { path })`, right before "Set as desktop background" (which every such picture has).

Both events exist already and the window handles them. Leave this comment line right after the block that adds "Move to the other pane" (card G1 puts its submenu there): `// G1: "Move or copy to" goes here.`

## What changes in the old tests (`menus/tests.rs`)

Nothing but this: tests that list the whole menu of **a picture** gain the line "Copy the text in it" before "Set as desktop background". No other expected list changes; if another old test fails, the change that made it fail is wrong (PLAN.md §8).

## Tests (`tests_host.rs`)

Start the app's pieces as `menus/tests.rs`'s `open` does, plus `aware::init_with(cx, sources)` with a probe you hold the sender of (card D1's tests show how).

1. A file's menu starts with a head whose title is the file's name (`ui.nodes()[0]` is `Node::Head`), then the strip.
2. This PC's menu has no head.
3. A zip's menu has "Suggested" with "Extract here" right after the strip, and everything after the section's separator equals `item_menu`'s own list from its second node on.
4. With `settings.suggest` off there is no "Suggested", and the head is still there.
5. An answer that arrives while the menu is open (send `Part::LockedBy(vec!["Word".into()])` for the file) puts "In use by Word" in the head, and the rows are the same nodes as before it arrived (compare `labels` before and after, ignoring node 0).
6. Opening a menu for one file asks the probe on `Lane::Now`; closing it sets that run's stop flag.
7. Choosing "Extract here" notes `do:extract-here` under `archive` (read `persistence::app(cx).habits`); choosing "Copy" notes nothing; with `settings.learn` off nothing is noted.
8. After three such choices on three days' worth of sources (or three calls of `aware::note_action`), a row that no rule suggests (give a text file three uses of `do:compress:zip`) is in "Suggested" with "You often do this with documents" or the words for its shape.
9. A file whose answers say `origin.blocked` has "Unblock"; without the answer it hasn't.
10. A PNG has "Copy the text in it"; a text file hasn't; a PNG inside an archive hasn't.
11. Inside an archive, Cut's `why_not` is "Nothing changes inside an archive", and on a remote "Read-only: on a remote connection"; in the folder's menu with nothing to undo, Undo's is "Nothing to undo".
12. `suggestions(pane, 1, cx)`: one zip selected gives "Extract here" with key `do:extract-here` and shape `Archive`; nothing selected gives nothing; 201 selected give nothing.
13. The folder's own menu has a head with "{n} items", and a branch badge when the fake Git view has a branch (see how `components/git`'s tests give one; if that needs more than a few lines, leave the branch out of this test and say so).

## Done when

Every test under `components::menus` passes (old and new), clippy is clean, and `item_menu` and `folder_menu` return what they returned before for every input that has no `probed` and isn't a picture.
