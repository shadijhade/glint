# Card E2 — The head's contents and the Suggested section, as data

Wave 3 · crate `glint-ui` · pure builders over the menu's data (no window needed)

**You own:** `crates/glint-ui/src/components/menus/head.rs`, `menus/smart.rs`, `menus/tests_smart.rs`.
**Read first:** PLAN.md §2 (R1, R3, R7), §3 (F1, F2), §5.1, §5.4, §5.5; [mock.html](../mock.html); `menus/spec.rs` (all); `menus/event.rs` (every `MenuEvent` variant: you give each a key); `menus/item.rs` lines 56 to 130 (how a menu is built as data); `crates/glint-model/src/format.rs` (`format_size` at 37, `format_duration` at 79, `format_date` at 94, `format_count`: read what each takes); `crates/glint-model/src/file_types.rs` (`kind_of`, `type_label`); `crates/glint-model/src/entries.rs` (`cloud_state`, `CloudState`, `is_hidden`, `attr`).
**Check with:** `cargo test -p glint-ui components::menus::tests_smart` and clippy on the crate.

Card E4 calls these functions from the menu host at the same time as you write them; the signatures below are in the tree already. Don't change them.

## smart.rs

### `impl Run { pub fn key(&self) -> Option<String> }`

Exactly the table in PLAN.md §5.5. Write it as `match`es with one arm per variant and **no `_` arm**, each returning a literal (so adding a variant later fails to compile until it is given a key). `Task::OpenIn { opener, .. }` is `format!("task:open-in:{opener}")`; `NewOfKind { ext }` is `format!("do:new:{}", ext.to_lowercase())`; `RunScript { script, .. }` is `do:script:` and the script's file name in lower case (`glint_model::paths::base_name`).

### `pub fn keys(nodes: &[Node]) -> Vec<String>`

The key of every row that can be chosen, in the menu's order: `Node::Item`s and a strip's buttons that are not `disabled` and have a key; the rows of every `Node::Menu` that is not `disabled`, at any depth. Each key once (the first time it is met). `Head`, `Heading`, `Separator` give nothing.

### `pub fn row_for(nodes: &[Node], key: &str) -> Option<Entry>`

The first such row with that key, as a copy made to stand alone:
- its `label` is its own at the top level and in the strip; under submenus it is `{menu} › {label}` (two deep: `{outer} › {inner} › {label}`), the `›` with a space each side;
- `smart = true`; `check = None`; `danger`, `active` as they were; `hint` as it was; `reason = None`.

### `pub fn reason(why: &Why, shape: Shape) -> Option<SharedString>`

| `why` | Words |
| :--- | :--- |
| `Plain` | none |
| `InUse(app)` | `In use by {app}` |
| `FromInternet(host)` | `It came from {host}`; with an empty host, `It came from the internet` |
| `Unsigned` | `It isn't signed` |
| `BuildOutput(bytes)` | `{size} of build output inside` (`format_size`) |
| `Usual` | `You often do this with {kind}`; for `Shape::Nothing`, `You often do this here` |
| `UsualPlace` | none here (card G1 words it) |

`{kind}` by shape: Folder, Folders: `folders` · Archive, Archives: `archives` · DiskImage: `disk images` · Program: `programs` · Installer: `installers` · Script: `scripts` · Shortcut: `shortcuts` · Picture, Pictures: `pictures` · Video, Videos: `videos` · Song, Songs: `songs` · Pdf, Pdfs: `PDFs` · Document, Documents: `documents` · Data: `data files` · Code: `code` · Font: `fonts` · File, Files, Mixed: `files like these`. Each whole sentence is one `t!` (a `match` on the shape with a `t!("You often do this with pictures")` per arm: translators need whole sentences).

### `pub fn zone(nodes: &[Node], picks: &[Pick], shape: Shape, extra: Vec<Node>) -> Vec<Node>`

`Node::Heading(t!("Suggested"))`, then for each pick, in order, `Node::Item(row)` where `row = row_for(nodes, &pick.key)` with `reason = reason(&pick.why, shape)`; a pick with no row is skipped. Then the `extra` nodes (card G1's "usual place" row; empty until then). At most three rows in all: when `extra` has rows, picks are cut so the total is three. With no row at all: an empty `Vec` (no heading).

### `pub fn dress(nodes: Vec<Node>, head: Head, picks: &[Pick], shape: Shape, extra: Vec<Node>) -> Vec<Node>`

`[Node::Head(head)]`, then the menu's `Node::Strip` if that is its first node, then the zone, then a `Node::Separator` if the zone isn't empty and the next node isn't a `Heading` or a `Separator`, then the rest of the menu exactly as it was. The menu's own rows are never moved, changed or removed (R3).

## head.rs

```rust
/// What the head needs that isn't in the entries or the answers.
#[derive(Clone, Debug, Default)]
pub struct Extras {
    /// Now, in Unix milliseconds (for "Changed …").
    pub now: i64,
    /// Git's word for the one target: "Changed" or "New".
    pub git: Option<SharedString>,
}
pub fn head_of(targets: &[FileEntry], probed: Option<&Probed>, extras: &Extras) -> Head;
pub fn folder_head(name: SharedString, items: Option<usize>, branch: Option<SharedString>) -> Head;
```

`probed` is used only when there is exactly one target.

**One file.** `title`: its name. `picture`: `HeadPicture::Kind(kind_of(entry))`. `line`: these parts joined with ` · `, each only when known (R1):
1. `type_label(entry)`;
2. `format_size(entry.size)`;
3. `{w} × {h}` when `probed.media` has both above zero;
4. the length (`format_duration`; mind its unit) when `probed.media.seconds > 0`;
5. `{n} lines` (`1 line`) when `probed.text` is there and not `binary`;
6. `{n} pages` (`1 page`) when `probed.pages` is there.
`changed`: `Changed {when}` with `format_date` of `entry.modified` (none when `modified` is 0).

**One folder.** `title`: its name; `picture`: `Folder`; `line`: `Folder`, then, only when `probed.inside` is there and `done`: `{n} files` (`1 file`) when `files > 0`, and `format_size(bytes)`. `changed` as for a file.

**Several.** `title`: `{n} items`; `picture`: `Several`; `line`: `{f} files` (`1 file`) and `{d} folders` (`1 folder`), the zero one left out, joined with `, `; then ` · {size}` of the files when there are files. No badges, no `changed`.

**No targets:** `folder_head` is used instead. `title`: `name`; `picture`: `Folder`; `line`: `{n} items` (`1 item`) when `items` is given, else empty; one badge when `branch` is given (its label the branch, `IconName::GitBranch`, `Tone::Plain`, `Run::Nothing`).

**Badges of one target**, in this order (the view draws the first three):

| # | When | Label | Icon | Tone | Tip | Run |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | `locked_by` has names | `In use by {first}` | `Lock` | Warn | the names joined with `, ` | `Ask(Locks { paths: [path] })` |
| 2 | `origin.blocked` | `From {host}`, or `From the internet` | `Download` | Warn | `Windows marked it as downloaded` | `Ask(XRay { path })` |
| 3 | `signer` is `Unsigned` | `Not signed` | `ShieldAlert` | Warn | none | `Ask(XRay { path })` |
| 3 | `signer` is `Signed(name)` | `Signed by {name}` | `ShieldCheck` | Good | none | `Ask(XRay { path })` |
| 4 | `twins` has paths | `{n} identical copies` (`1 identical copy`) | `Files` | Plain | the first five paths, one to a line | `Ask(OpenFileLocation { path: first })` |
| 5 | `cloud_state` is `Online` | `Online only` | `CloudOff` | Plain | none | `Nothing` |
| 5 | `cloud_state` is `Kept` | `Always on this PC` | `CloudDownload` | Plain | none | `Nothing` |
| 6 | the read-only attribute is set (a file) | `Read-only` | `PencilOff` | Plain | none | `Nothing` |
| 7 | `is_hidden` | `Hidden` | `EyeOff` | Plain | none | `Nothing` |
| 8 | `target` is there | `Points to {name}` (the target's base name) | `ExternalLink` | Plain | the target's path | `Ask(OpenFileLocation { path: target })` |
| 9 | `extras.git` is there | that word | `GitBranch` | Plain | none | `Nothing` |

A folder gets only rows 5, 7 and 9. Every string through `t!`. List the `IconName`s you used that `kit.rs`'s icon test doesn't have yet under NEEDS FROM COORDINATOR.

## Tests (`tests_smart.rs`; plain `#[test]`s, holding `testing::lock()` and `testing::set_rtl(false)` as `menus/tests.rs`'s `english()` does)

Build entries with `crate::explorer::fake::{file, folder}` and menus with `item_menu(&targets, &ItemCtx { folder: "C:\\x".into(), ..Default::default() })`.

1. `Run::key`: `Command::Copy` is `cmd:copy`; `Task::OpenIn` with opener `code` is `task:open-in:code`; `Compress` with `SevenZ` is `do:compress:7z`; `PdfTools` with `Merge` is `do:pdf:merge`; `CopyToOtherPane { moving: true }` is `do:move-other`; `Run::Nothing` and `MenuEvent::Shown` have none.
2. Every row of a real file's menu and a real folder's menu that can be chosen has a key, and no two different rows of one menu share one (`keys` has as many entries as rows found by walking the menu yourself, except rows that really are the same action).
3. `keys` skips disabled rows and the rows of a disabled submenu, and lists a submenu's rows.
4. `row_for` a zip's `do:extract-here` is "Extract here"; `do:compress:zip` is "Compress to › Zip file"; a key the menu doesn't have is `None`; the copy is `smart` with no check mark.
5. `reason`: each row of the table, word for word; `Usual` for `Pictures` is "You often do this with pictures".
6. `zone`: two picks give a heading and two rows with their reasons; a pick with no row is skipped; no rows give nothing at all; four picks give three rows; one `extra` row and three picks give two picks' rows and the extra.
7. `dress` puts the head first, the strip second, then "Suggested"; the menu's own nodes follow in their order, none missing (compare `labels` of the tail with `labels` of the undressed menu's tail); with no picks there is no heading and no extra separator.
8. `head_of` one file with no answers: the title is its name, the line is "{type} · {size}", no badges, `changed` is there.
9. With answers: dimensions, the length, "1,230 lines", "1 page" appear in that order; a binary file says no lines.
10. Each badge row of the table, alone, gives its label, tone and run; all of them at once come in the table's order.
11. A folder: "Folder" alone without answers; "Folder · 1,204 files · 3.2 GB" with a finished inside; nothing of an inside that isn't `done`.
12. Three files and a folder: title "4 items", line "3 files, 1 folder · {size}"; two folders: "2 folders".
13. `folder_head`: "25 items", "1 item", and a branch badge.

## Done when

The tests pass, clippy is clean, and nothing in these two files needs a window or the app's state: everything they read is given to them.
