# Card F2b — The suggestion for a `tag.dat` never brought in; following XYplorer when Glint comes to the front

Wave 4 · crate `glint-ui` · two small models

**You own:** `crates/glint-ui/src/suggestions.rs`, `crates/glint-ui/src/marks_follow.rs` (new: `init(cx)` is there, does nothing, and `startup.rs` calls it after `explorer::marked::init`), and **one marked place** in `crates/glint-ui/src/components/settings/overview.rs`: where it gathers the suggestions' facts (about 357), if the new fact must be asked there too. Tests at the bottom of your two files.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M7), §7.2 (F27, F30), §7.3 "Following". Then the code: `suggestions.rs` whole (`Kind` 35 with `id`, `may_notify`, `icon`; `Suggestion` 79; `Facts` 98 and `from_settings` 120; `all` 142; `dismissed` 255; `act` 294; `offer` 323 and how it asks facts off the UI thread, about 346); `settings/overview.rs` `suggestions_card` (258: it draws whatever `all` answers); `workspace.rs:299` (`observe_window_activation`: how a view hears the window come to the front); glint-core's `marks::xyplorer_found` (`xy_find.rs`: `Found { path, items, changed }`), `marks::apply::brought_in`, `marks::follow` and `Followed` (`apply.rs`); `dialogs/tags.rs` (`hub`, `TagsChanged`); `dialogs/marks_import.rs` (`open(window, cx, Some(path))`: card F2a's dialog).
**Check with:** `cargo test -p glint-ui suggestions::`, `cargo test -p glint-ui marks_follow::`, `cargo test -p glint-ui components::settings` once at the end, and `cargo clippy -p glint-ui --all-targets -- -D warnings`.

**What is there for you:** core's `xyplorer_found`, `brought_in` and `follow` **answer nothing until cards F1a and F1b are in, at the same time as you work**: test against fakes.

## The suggestion (F27)

- `Kind::XyplorerMarks`: id `"xyplorer-marks"`, an icon the kit already has (say which), **may notify** (it comes up once, as the suggestions do), placed after `ToolUpdates` in `all`'s order.
- `Facts` gains `xyplorer: Option<(String, u64)>`: the `tag.dat` found and how many items, **`None` while unknown, when none is found, or when it was brought in already**. It is asked off the UI thread where the other slow facts are (`xyplorer_found()` then `brought_in(&path)`, both blocking: on `core_bridge::run` or the thread the drives are asked on), through something a test replaces. Reading only (M7).
- The suggestion: title `t!("Bring your XYplorer labels, tags and comments")`, body `t!("{n} marked in XYplorer, kept as they are there")` with `n` as `plural(items, "item")` through `format_count`, button `t!("Import marks…")`. `act` opens `dialogs::marks_import::open(window, cx, Some(path))` (the path rides in the suggestion: `Suggestion.drive` holds a drive today; add a field or reuse it, and say which).
- Dismissed, it stays away as the others do.

## Following (F30): marks_follow.rs

`init(cx)` and `init_with(cx, source)`, the source being a boxed async fn answering `Result<Option<Followed>, String>` (in the app `glint_core::marks::follow()` on `core_bridge::run`).

- **Only while `settings.follow_xyplorer`** is on; turned on later, it starts then.
- Asked **once when Glint starts and each time the main window comes to the front**, never twice at once, and not again within five seconds of the last answer (the clock behind something a test moves).
- When the answer has `came + gone > 0`: `TagsChanged { paths }` on the hub, and **one quiet line**: a plain notification that goes by itself, `t!("{n} came from XYplorer")` with `n` as `plural(came + gone, "mark")`. Nothing when nothing changed; an error says nothing (it is tried again next time).
- Nothing here reads a file on the UI thread.

## Tests (STANDARDS §5.1 "Tests")

1. With `xyplorer: Some((path, 1284))` the suggestion is in `all`, worded with `1,284 items`; with `None` it isn't; dismissed, `current` leaves it out.
2. Its button opens the import for that path (assert through whatever `act` can be seen to do in a test: say how).
3. `Kind::XyplorerMarks` has an id no other kind has, and may notify.
4. Following off: the source is never asked. On: asked once at `init`; asked again when the window comes to the front; not twice within five seconds; turned on later, asked then.
5. An answer of `came: 2, gone: 1` says `TagsChanged` for its paths and one notification reading `3 marks came from XYplorer`; `None` and an error say nothing.
6. The Overview's tests there are pass unchanged.

## Done when

The tests pass, clippy is clean, and nothing outside your files changed but the one place in `overview.rs`, if any. Say in your report every new string, the icon, where the path rides, and how the window's coming to the front is heard without `workspace.rs`.
