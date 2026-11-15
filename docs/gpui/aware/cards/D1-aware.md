# Card D1 — The Aware entity

Wave 2 · crate `glint-ui` · one model, no drawing

**You own:** everything in `crates/glint-ui/src/aware/`: `mod.rs` (the signatures are there; keep them), and the files you add beside it, which are declared already: `model.rs`, `clip.rs`, `pulse.rs`, `tests.rs`.
**Read first:** STANDARDS.md §5.1 ("Calling core", "Models", "The app's services", "A view that is not on screen does nothing", "Tests"); PLAN.md §2, §4, §5; `crates/glint-ui/src/components/row_facts.rs` lines 1 to 60 (a shared model that reads in batches and keeps answers: the nearest thing to this one); `crates/glint-ui/src/core_bridge.rs` (`stream` at 320, `subscribe` at 369, `CoreEvent`); `crates/glint-ui/src/explorer/types.rs` (`ExplorerEvent` at 489, `Drive` at 135, `DriveKind` at 95); `crates/glint-ui/src/persistence.rs` (`app` at 604, `update_app` at 610); `crates/glint-ui/src/state.rs` (`settings`); `crates/glint-ui/src/components/menus/mod.rs:646` (`main_window`: copy it privately, marked `// TODO(coordinator): from core_bridge`).
**Check with:** `cargo test -p glint-ui aware::` and clippy on the crate.

## What it is

One `Entity<Aware>` for the app (a `Global` holds it, as `row_facts::shared` does). It keeps what has been found out about files, asks for what hasn't, watches the clipboard and the folder's changes, and is the only code that reads or writes `AppState.habits`. Views observe it and redraw when it changes. It draws nothing.

## Sources (so tests never touch the engine)

```rust
#[derive(Clone)]
pub struct Sources {
    /// Starts probing; the answers arrive on the receiver.
    pub probe: Rc<dyn Fn(Vec<Ask>, Lane, Arc<AtomicBool>) -> async_channel::Receiver<Answer>>,
    /// Windows' count of clipboard changes.
    pub clip_seq: Rc<dyn Fn() -> u32>,
    /// What is on the clipboard now.
    pub clip: Rc<dyn Fn(&mut App) -> Task<Clip>>,
    /// Today, in days since 1970-01-01.
    pub today: Rc<dyn Fn() -> u32>,
}
impl Sources {
    pub fn core() -> Sources;     // the engine and Windows
    pub fn still() -> Sources;    // tests: no probe ever answers, the clipboard is empty and never changes, day 20000
}
```

`Sources::core()`:
- `probe`: `let (channel, receiver) = core_bridge::stream::<Answer>(); glint_core::aware::probe(asks, lane, stop, channel); receiver`.
- `clip_seq`: `windows::Win32::System::DataExchange::GetClipboardSequenceNumber()` (safe to call from any thread; wrap in `unsafe` with its `// SAFETY:` line).
- `clip` (in `clip.rs`): `core_bridge::run(glint_core::shell::clipboard_get_files(main_window))`. `Ok(Ok(Some(files)))` with paths: `Clip::Files { count, cut: files.cut, from, names }`, where `names` are the first five base names (`glint_model::paths::base_name`) and `from` is the folder they all share (`glint_model::paths::parent_path` of each; `None` when they differ). Otherwise look at `cx.read_from_clipboard()`: an `Image` entry is `Clip::Picture`; a `String` entry that isn't empty is `Clip::Text`; else `Clip::Empty`. No main window: only the second part.
- `today`: `glint_model::aware::habits::day_of(now in Unix milliseconds)`.

## The cache and looking

Constants: `MOST_KEPT: usize = 512`, `MOST_TO_LOOK_AT: usize = 200`, `INSIDE_FRESH: Duration = 30 s`, `SELECTION_SETTLES: Duration = 150 ms`.

- The model has what the bookkeeping needs: `Want::minus(&probed)` (what is still unknown), `Want::without(asked)` (what hasn't been asked yet), `Want::and(other)` (both), `Want::is_empty()`, and `Probed::take(part)`. The two function types in `Sources` have names in the code: `Probe` and `ReadClip`.
- **Kept per path** (lower case): the entry's size and modified time when asked, the `Probed` so far, the `Want` already asked for, and when. A look at an entry whose size or modified time differ from what is kept forgets what is kept first.
- **`look_ahead(entries, cx)`** and **`look_now(entries, cx)`**: take the first `MOST_TO_LOOK_AT` entries; for each, `want = want_for(entry)` minus what is kept and minus what was already asked; skip entries with nothing left to ask, and entries on a drive that isn't `DriveKind::Fixed` (find the drive whose `path` the entry's path starts with, ignoring case, in `explorer.read(cx).drives()`; no drive found: skip). A folder's `inside` is asked again when its answer is older than `INSIDE_FRESH` (the old answer stays until the new one arrives). If anything is left, call `sources.probe` with `Lane::Quiet` or `Lane::Now` and a new stop flag.
  - `look_ahead` first sets the stop flag of the last quiet run (the pointer has moved on); what that run already answered stays.
  - `look_now` first sets the stop flag of the last now run.
  - `pub fn rest(&mut self)`: sets both (a menu closed). Add this method.
- **Answers.** One task per run reads its receiver: wait for an answer, then take every answer already waiting (`try_recv` until empty), apply them all (`Probed::take`), `cx.notify()` once, then wait 16 ms before reading again. So views redraw at most once a frame, however many answers arrive (R4). A run whose stop flag is set drops what still arrives.
- Over `MOST_KEPT` paths: forget the 64 least recently asked.
- **`probed(entry)`**: what is kept for its path, if the kept size and modified time are the entry's. Reading only.
- **`forget(path)`**: drops what is kept for it (ignoring case).

## The selection, looked at ahead

Subscribe to the explorer (`cx.subscribe(&explorer::explorer(cx), …)`): on `ExplorerEvent::SelectionChanged(tab)` for the active tab, start (replacing any pending one) a task that waits `SELECTION_SETTLES` and then calls `look_ahead` with the tab's selected entries (`tab.rows.chosen(&tab.selection)`, cloned), when there are between 1 and `MOST_TO_LOOK_AT` of them. More than that: nothing (section 8 of the plan: a huge selection costs nothing).

## The clipboard (`clip.rs`)

A task that runs for the life of the entity: every 700 ms, if a window of the app is active (`cx.active_window().is_some()`), compare `(sources.clip_seq)()` with the last seen; when it differs, await `(sources.clip)(cx)`, and if the `Clip` is not the one kept, keep it and `cx.notify()`. `clip()` returns the kept one.

## The folder's pulse (`pulse.rs`)

- `core_bridge::subscribe(cx, …)`: on `CoreEvent::DirChanged(folder)`, push `Instant::now()` for that folder (lower case) onto its queue; drop instants older than 60 s and keep at most 256 per folder and at most 16 folders (drop the folder least recently beaten). If the folder is the active tab's (`explorer.read(cx).active_tab()`'s `path`, ignoring case): `cx.notify()`, and start (replacing any pending one) a task that waits 10.5 s and notifies again, so "Changing now" goes away when the changes stop.
- `pulse(folder)`: `None` when nothing is kept for it in the last 60 s; else thirty counts, two seconds each, the newest last, each capped at 255.
- `changing(folder)`: three or more instants in the last 10 s.
- Keep the arithmetic pure and testable: `pub(crate) fn buckets(beats: &[Duration]) -> [u8; 30]`, given how long ago each beat was.

## Habits and suggesting (free functions in `mod.rs`)

- `facts(cx, targets)`: `shape`, `count`, `files`, `folders`, `bytes` (the files' sizes), `ext` (`shared_ext`), `first_name` (the first target's `lower`), `fixed` (any target `is_archived`, `is_remote`, `is_closed` or `is_recycled`), `clip` (the entity's, `Clip::Empty` when the entity isn't there).
- `suggest(cx, facts, probed, available, most)`: an empty list when `state::settings(cx).suggest` is false. Else `rank::suggest` with `persistence::app(cx)`'s `habits` when `settings.learn` is true (an empty `Habits` when false, or when there is no saved state), and today from the entity's sources.
- `note_action(cx, shape, key)`: nothing when `settings.learn` is false, the key is empty or `rank::is_basic(key)`. Else `persistence::update_app(cx, |app| app.habits.note_action(shape, key, today))`.
- `note_place(cx, exts, folder)`: nothing when `learn` is false. Else for each extension (at most eight; an empty slice counts as one `""`): `app.habits.note_place(ext, folder, today)`.
- `places_for(cx, ext)`: empty when `learn` or `suggest` is false; else `habits.places_for(ext, today)`.
- `forget_habits(cx)`: `app.habits.forget()`.

## Tests (`tests.rs`)

Use `testing::init(cx)`, `explorer::init` with a `FakeDisk` (see `crates/glint-ui/src/components/menus/tests.rs` lines 1 to 20 for the imports and how a fake explorer is opened), and `aware::init_with(cx, sources)` with a `Sources` whose `probe` records every call in an `Rc<RefCell<Vec<(Vec<String> /* paths */, Lane)>>>` and returns a receiver the test holds the sender of.

1. A look at a file asks the probe once with that path on the quiet lane; a second look at the same file asks nothing.
2. An answer sent on the channel is in `probed(entry)` after `cx.run_until_parked()`, and observers were notified once for three answers sent together (count with `cx.observe`).
3. A file whose size changed is asked about again, and `probed` no longer returns the old answer.
4. `look_ahead` again sets the first run's stop flag; `look_now` doesn't stop a quiet run; `rest` stops both.
5. Entries on a network drive (give the fake explorer a drive of `DriveKind::Network`) are never asked about; nor are more than 200 entries (201 looked at: 200 asked).
6. An entry in an archive is never asked about (`want_for` is empty).
7. Selecting two files in the active tab asks about them after the wait (advance the executor's clock by 200 ms: `cx.executor().advance_clock(..)`), and selecting 300 asks nothing.
8. `buckets`: no beats is all zeros; a beat 1 s ago is in the last count; one 59 s ago in the first; 300 beats 1 s ago is 255.
9. `changing` needs three beats within 10 s; a `DirChanged` for the active tab's folder notifies observers.
10. The clipboard: a `clip_seq` that returns a new number and a `clip` that returns `Clip::Files { count: 3, .. }` make `clip()` say so after 700 ms of executor time, and observers are told once; the same sequence number again asks nothing.
11. `suggest` is empty with `settings.suggest` off (`state::update`); with `learn` off a habit of ten uses adds nothing; `note_action` writes nothing with `learn` off and never writes a basic key.
12. `facts`: two pictures give `Pictures`, `count == 2`, their summed `bytes` and `ext == Some("png")`; an archived entry makes it `fixed`.

## Done when

The tests pass, clippy is clean, and no function here blocks: every wait is a task.
