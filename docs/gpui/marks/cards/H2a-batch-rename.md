# Card H2a — `{label}` and `{tags}` in Batch rename

Wave 5 · crate `glint-ui` · a dialog's model and its tokens

**You own:** `crates/glint-ui/src/dialogs/batch_rename.rs` and its tests. Nothing else (not `a11y_tests.rs`, which builds this dialog's `Source` with its two fields: **don't add a field to `Source`**).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), §5.1 to §5.5, §8 and §8.1 (the last item, "Landed before wave 5's cards"). Then the code: `batch_rename.rs` `TOKENS` :183, `DETAIL_TOKENS` :189, `wants_details` :192, `token_help` :197, `validate_target_name` :408, `safe` :423, `simulate` :496 (the chain of `replace`s :532–:546), `Source` :606, `Details` :687, `BatchRename` :746, `reading` :817, `can_apply` :829, `resimulate` :875, `update_rules` :968, `read_details` :999, the token chips :1613, and the tests (`details_are_read_once_when_the_pattern_asks_and_renaming_waits_for_them` :3271 is the model). The tags hub: `dialogs/tags.rs` `Backend::read_marks` :167, `hub` :310, `Tags::backend` :275, `init_with` :303 (how a test gives the hub a fake). glint-model's `marks/labels.rs` `shown` :74. Settings' labels: `state::try_settings(cx).map(|s| s.labels.clone())` (as `explorer/marked.rs:93` reads them), else `glint_model::marks::default_labels()`.
**Check with:** `cargo test -p glint-ui dialogs::batch_rename` and `cargo clippy -p glint-ui --all-targets -- -D warnings`. glint-ui builds slowly and other agents wait on the same lock: build by your filter; run the whole crate's tests once at most, at the end, if at all.

## The two tokens

- `TOKENS` gains `"{label}"` and `"{tags}"` (after `"{parent}"`, before the photo and song tokens; the array's length changes). `token_help`: `"{label}"` → `t!("The item's label")`, `"{tags}"` → `t!("The item's tags, with a space between")`.
- **`{label}`** is the name the item is shown with: `glint_model::marks::shown(&labels, &marks).label` (a 0.16 color tag reads as its label; a renamed label by its name now). **`{tags}`** is `shown(..).tags` joined with one space, in the order the item has them (so a 0.16 color tag drawn as the label isn't repeated among the tags). Empty where the item has none.
- **Cleaned as any part of a name is, by `validate_target_name`**, **not** by `safe`: a tag with `/` or `:` gives a name the row refuses with its conflict's words (`InvalidCharacters`), never a path into a subfolder. Say so in a comment where they are filled.

## Read once, off the UI thread, only when asked for

- `pub fn wants_marks(pattern) -> bool` beside `wants_details`.
- A state like `Details` (Idle, Reading, Ready) for the marks, read **once** the first time the pattern holds `{label}` or `{tags}` (in `update_rules`, as `read_details` is started), through the tags hub's backend: `crate::dialogs::tags::hub(cx).read(cx).backend().read_marks(paths)` on core's runtime (`core_bridge::run`), as the rows read them. A failure leaves the map empty (empty tokens), as details do. Work out each item's `(label, tags)` text with the labels **when the marks land** and keep that map, so `simulate` does no lookup a row.
- `simulate` takes the map as one more argument (it has no caller outside this file); with no map the two tokens are empty.
- `reading()` is true while either the details or the marks the pattern wants are not in, so **Rename waits for them** (`can_apply`).
- Nothing on the UI thread but what is in memory: no read of a file, no lock held across an await.

## Tests

As STANDARDS §5.1 "Tests" and this file's tests: `testing::init`, `let _guard = testing::lock();`, the hub given a fake backend with `tags::init_with` (a fake that answers `read_marks` from a map and counts its calls), never a real disk.

1. `simulate` pure: `{label} {name}` with an item labeled Important gives `Important report.pdf`; an item with none gives ` report.pdf`'s trimmed form as the chain gives it today (say what it gives); `{tags}` gives the tags with a space, in their order.
2. A 0.16 color tag (`tags: ["Red", "work"]`, no label) reads as `{label}` Red and `{tags}` `work`; a label renamed in Settings (`was`) reads by its new name.
3. A tag with `/` or `:` makes the row's conflict `InvalidCharacters` and the batch can't be applied; its `to` stays in the item's folder.
4. Marks are read once, when `{label}` or `{tags}` first appears, with every item's path, and never for a pattern without them; Rename waits until they are in.
5. Right to left: an Arabic name, label and tag (`تقرير`, `مهم`, `عمل`) fill the pattern and the dialog draws with the mirrored layout without trouble (as this file's Arabic test does).
6. The token chips list the two new tokens, each a button with its name and help (the test that lists the chips, if there is one, changes: say so).
7. **Measured** (`#[ignore]`, prints): `simulate` over 1,000 items with `{label}` and a map of their marks; give the time (debug build) in the report.

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no `dir /s` from a root, no `Get-ChildItem -Recurse` outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. If a build has waited ten minutes on Cargo's lock, say so in your report rather than waiting on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug, and don't trust a timing from such a window.
- **Backslashes**: edit Rust with the editing tools, never a script typed into a shell.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no end-to-end checks, no real AI agent, no installed Glint.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. If you meet it, say so and go on.

## Done when

The tests pass, clippy is clean for glint-ui, `Source`'s fields and `open_with`'s signature are unchanged. In your report: every string you added for `t!`, the measurement, and every test you changed.
