# Card W6 — The small things left

Wave 6, round 2 · crate `glint-ui` · four small things, each cut freely

**You own:** `crates/glint-ui/src/components/sidebar/mod.rs` (the tag's menu and its two dialogs) and `sidebar/tests.rs`; `crates/glint-ui/src/dialogs/mod.rs` (`enter_runs_main`); `crates/glint-ui/src/persistence.rs` (`KEPT_RECENT_TAGS` :747) and `crates/glint-ui/src/dialogs/tags.rs` (only its recent tags' cap); `crates/glint-ui/src/components/settings/mod.rs` (`scroll_to` :433, `open_at` :832) and its tests. **Not**: where the head's label chips go, and how the export's path is cut: those are the person's to decide.
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M1 to M11), F17, §13, §10, and under §9's table "Wave 3, as it landed", "Wave 4, as it landed" and "Wave 5, as it landed".
Then the code named under each part below.
**Check with:** `cargo test -p glint-ui components::sidebar`, `cargo test -p glint-ui dialogs::` (by the narrowest filter that holds what you change), `cargo test -p glint-ui persistence`, `cargo test -p glint-ui components::settings`, `cargo clippy -p glint-ui --all-targets -- -D warnings`. glint-ui builds slowly and two other agents build it now.

## 1. *Rename everywhere…* and *Take off every file…* say they are looking

`sidebar/mod.rs`: the menu :926–:934, the dialogs from :1906, what has a tag (:160). While they find the files that have the tag (long on a drive the index doesn't cover), they say `t!("Finding what has “{tag}”…", tag = …)` where the count or the button would be, and the button that acts waits until it is known. Tests in `sidebar/tests.rs` (:1067–:1170 are the model): the words show while the finding hangs, and go when it answers.

## 2. `dialogs::enter_runs_main` goes

`dialogs/mod.rs` :122 (its doc), :163. Older, called by nobody, and it panics from a dialog's own view (wave 4's record). Remove it and its doc's mention; check with `rg` that nothing calls it.

## 3. The recent tags' cap is one constant

`persistence.rs` `KEPT_RECENT_TAGS` (:747, used :1088) and the cap `dialogs/tags.rs` uses when it remembers recent tags (around :440–:450: find it). One constant, `pub(crate)`, in `persistence.rs`; `tags.rs` uses it. (`sidebar/rows.rs`' `MOST_RECENT_TAGS = 8` is how many the sidebar *shows*: another thing; leave it. `old_settings`' 20 is the old app's: leave it.)

## 4. `open_at` reaches a section after a page's first

`settings/mod.rs`: `open_at(window, cx, Some(section))` asks `scroll_to` before the page is laid out, so a later section stays at the page's top (wave 4's record). Ask on the next frame (`window.on_next_frame` or `cx.defer_in`, as the file or the kit does elsewhere), or when the section's bounds are first known. Test: `open_at` with the last section of a long page shows that section (its scroll offset is not 0).

**This card draws**: STANDARDS §5 is the law. The kit and the theme only (no `gpui_component` in a view, no color or size of its own); a label's color from `theme::label_tone`, never the accent (M11: a label is not a state); every string through `t!`; everything that can be pressed has a role and a name, and keyboard alone works; right to left tested with an Arabic name; nothing on the UI thread but reading what is in memory; a test that lists rows changes when a row is added (say so).

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

Tests pass, clippy is clean for glint-ui. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus NEW STRINGS (every `t!` string added or changed), MEASURED (if the card asks), TESTS CHANGED (every existing test you changed, and why).
