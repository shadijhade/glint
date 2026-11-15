# Card F1 — The island's next step, and Paste that says what it would paste

Wave 4 · crate `glint-ui` · the command bar

**You own:** `crates/glint-ui/src/components/command_bar/smart.rs`, `command_bar/mod.rs`, `command_bar/tests_smart.rs`.
**Read first:** STANDARDS.md §5.1 ("The dock", "States", "Motion", "Measuring during a draw", "Tests"); PLAN.md §2 (R8, R9), §3 (F8, F9), §8 (the buttons' accessible names must not change); [mock.html](../mock.html) (section 2); `command_bar/mod.rs` as it is (`refresh` at line 316, `signature` at 360, `dispatch` at 386, the helpers `ghost`, `tool`, `word` from 622, `anchored` and `anchor_probe` from 688, `edit_buttons` at 916); `command_bar/facts.rs` (`Folding`: why the signature matters); `crates/glint-ui/src/components/menus/mod.rs` (`host(cx)`, `MenuHost::suggestions`, `Suggested`, `MenuHost::run`: card E4); `crates/glint-ui/src/aware/mod.rs` (`shared`, `Aware::clip`, `note_action`); `crates/glint-ui/src/components/viz.rs` (`glint_ring`: card C1).
**Check with:** `cargo test -p glint-ui components::command_bar` (the old tests must pass unchanged) and clippy on the crate.

## smart.rs — the words

```rust
/// What Paste would do with what is on the clipboard: "Paste 3 files", "Paste 1 file", "Move 3 files here",
/// "Move 1 file here". None when the clipboard holds no files.
pub fn paste_words(clip: &Clip) -> Option<SharedString>;
/// The Paste button's tooltip while files are on the clipboard. `keys` is Paste's key as text ("Ctrl+V").
pub fn paste_tip(clip: &Clip, keys: &str) -> SharedString;
/// The first suggestion for what is selected in `pane`, if the menus have one.
pub fn next_step(cx: &App, pane: usize) -> Option<Suggested>;
```

- `paste_words`: for `Clip::Files { count, cut, .. }`: copy is `Paste {n} files` / `Paste 1 file`; cut is `Move {n} files here` / `Move 1 file here` (`format_count` for n). The clipboard may hold folders too; "files" is said for both, as Windows does.
- `paste_tip`: the names are the first three of `names` joined with `, `; when `count` is more than three: `{names} and {n} more` (n = count − 3). Then one of, each a whole `t!` sentence:
  - copy, with `from`: `Paste ({keys}): {names}, from {folder}` · copy, without: `Paste ({keys}): {names}`
  - cut, with `from`: `Move here ({keys}): {names}, from {folder}` · cut, without: `Move here ({keys}): {names}`
  where `{folder}` is the base name of `from` (`glint_model::paths::base_name`; the whole of it for a drive's root). For any other clip: `Paste ({keys})`.
- `next_step`: `menus::host(cx)?.read(cx).suggestions(pane, 1, cx).into_iter().next()`.

## mod.rs — the bar

New state: `step: Option<Suggested>`, `step_turn: u64`, `clip: Clip`, `clip_turn: u64`.

1. **Following.** In `with_sources`, observe `aware::try_shared(cx)` when it is there: mark the bar `dirty` and notify (the clipboard changed, or an answer arrived that may change the suggestion).
2. **In `refresh`**, after the facts are made:
   - `clip` = the entity's `clip()` (cloned; `Clip::Empty` without the entity). If it differs from the one kept and is now `Clip::Files`, `clip_turn += 1`.
   - `step` = `smart::next_step(cx, explorer.active_pane())` when `facts.selected` is between 1 and 200, else `None`. If its `key` differs from the kept step's key (or one of them is `None` and the new one isn't), `step_turn += 1`.
3. **The signature** (`signature`): also hash whether there is a step and its label, and `smart::paste_words(&self.clip)`, so the bar measures again when either changes its width.
4. **The step's button** (in `edit_buttons`, only on the branch with a selection, after Delete; not drawn when `fold >= 3`):
   - a `divider`, then a `div().relative().flex_none()` holding the button and `viz::glint_ring("command-bar-step", self.step_turn, theme::size::RADIUS, cx)`, with an `anchor_probe(&self.anchors, "step")` so its place is known;
   - the button: as `word` makes one (id `"step"`), its label the entry's `label`, shown while `fold < 1`; its icon the entry's (`IconName::Sparkles` if it has none) in `t.primary`; its tooltip the label, and ` · {reason}` after it when the entry has a `reason`; its accessible name the label;
   - pressed: `aware::note_action(cx, step.shape, &step.key)`, then run it: a `Run::Command(command)` goes through `self.dispatch(command.action(), window, cx)`; anything else through the host: `host.update(cx, |host, cx| host.run(run, pane, at, window, cx))`, where `at` is the top start corner of the `"step"` anchor (the window's origin when it isn't known yet).
5. **Paste (F9).** Where the Paste button is made: when `smart::paste_words(&self.clip)` gives words, Paste is not disabled, and `fold < 2`, make it as `word` does with those words as its visible label and `smart::paste_tip` as its tooltip, **and then set its accessible name back to exactly what it is today** (`keyed("Paste ({0})", &[&actions::Paste])`: the end-to-end checks press the button by that name). Wrap it like the step's button, with `viz::glint_ring("command-bar-paste", self.clip_turn, theme::size::RADIUS, cx)`. In every other case the button is exactly as it is today.
6. With `settings.suggest` off there is no step (the host gives none). Paste's words are a fact, not a suggestion: they stay.

## Tests (`tests_smart.rs`; the bar's harness is in `command_bar/tests.rs`: read how it opens a bar over a fake explorer; add a `MenuHost` made with `menus::Sources::empty()` so `menus::host(cx)` answers, and `aware::init_with(cx, sources)` whose `clip` you set)

1. `paste_words`: 3 copied files, 1 copied file, 3 cut, 1 cut, each word for word; a picture, text and an empty clipboard give `None`.
2. `paste_tip`: two names from `C:\Users\me\Downloads` is "Paste (Ctrl+V): a.txt, b.txt, from Downloads"; five files name three "and 2 more"; cut says "Move here"; no `from` leaves the folder out; an empty clipboard is "Paste (Ctrl+V)".
3. With a zip selected, the island has a button named "Extract here" after Delete; with nothing selected it has none; with 201 items selected it has none.
4. Pressing it tells the window `MenuEvent::ExtractHere` (subscribe to the host) and notes `do:extract-here` in the saved habits.
5. With `settings.suggest` off there is no such button.
6. Selecting a different kind of thing (a zip, then two text files that can be compared) changes the button and `step_turn` goes up by one each time; the same selection again doesn't turn it.
7. With three files on the clipboard and a folder that can take them, the Paste button's accessible name is still "Paste (Ctrl+V)" and its visible words are "Paste 3 files" (read the button's label through the test registry, as the bar's own tests read labels; if only the accessible name can be read, assert the name and assert `smart::paste_words` separately, and say so in your report).
8. In a read-only place (inside an archive) Paste is as today: no words.
9. Folded far (set the bar's side room and width as the old folding tests do), the step is not drawn at fold 3 and has no label at fold 1.
10. The bar's signature differs with and without a step, and with and without files on the clipboard.

## Done when

Every test under `components::command_bar` passes (old and new), clippy is clean, and the names in PLAN.md §8 are all still the accessible names of their buttons.
