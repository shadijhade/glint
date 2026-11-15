# Card F2 — The dock's words and its end

Wave 3 · crate `glint-ui` · the status bar

**You own:** `crates/glint-ui/src/components/status_bar/sentence.rs`, `status_bar/chips.rs`, `status_bar/mod.rs`, `status_bar/tests_aware.rs`.
**Read first:** STANDARDS.md §5.1 ("The dock", "States", "Motion", "A view that is not on screen does nothing", "Tests"); PLAN.md §2 (R1, R4, R9), §3 (F10, F11), §8; [mock.html](../mock.html) (section 2: the words at the dock's two ends); `status_bar/mod.rs` as it is (`left` at line 481, `right` at 566, `Wide` at 325, `refresh_summary` at 158); `status_bar/summary.rs` (`counts` at 372, `selected_paths` at 284); `crates/glint-ui/src/components/viz.rs` (`roll`, `spark`: card C1); `crates/glint-ui/src/aware/mod.rs` (`Aware::probed`, `pulse`, `changing`); `crates/glint-ui/src/suggestions.rs` lines 150 to 175 (the rule and the words for a drive that is nearly full: use the same thresholds and the same words).
**Check with:** `cargo test -p glint-ui components::status_bar` (the old tests must pass unchanged) and clippy on the crate.

## 1. What the selection weighs and holds (`sentence.rs`, F10)

```rust
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Deep {
    /// Everything selected, folders' insides included; None when no folder is selected or one isn't known yet.
    pub bytes: Option<u64>,
    /// The files inside the selected folders; None when not known.
    pub inside: Option<u64>,
    /// "1,230 lines", "12 min 31 s", "1920 × 1080", "48 pages".
    pub detail: Option<SharedString>,
    /// Every folder's inside is finished (the numbers have stopped growing).
    pub done: bool,
}
/// The most selected items looked into.
pub const MOST: usize = 64;
pub fn deep(selected: &[FileEntry], probed: &dyn Fn(&FileEntry) -> Option<Probed>) -> Deep;
/// The dock's start words: `counts(s)`, with what `deep` knows.
pub fn counts_deep(s: &Summary, deep: &Deep) -> SharedString;
```

`deep`:
- Nothing selected, or more than `MOST`: `Deep::default()`.
- **With folders selected:** if every selected folder has an `inside` answer (finished or not): `bytes` = the selected files' sizes plus every `inside.bytes`; `inside` = the sum of `inside.files` when above zero; `done` = every one is `done`. If any folder has no answer yet: both `None`. No `detail`.
- **With only files selected**, `detail` is the first of these that holds, and each needs the answer for *every* selected file (R1):
  1. one file whose `media` has a width and a height: `{w} × {h}`, and ` · {length}` after it when it has seconds;
  2. every file's `media.seconds` is above zero: their total length (`format_duration`; mind its unit);
  3. every file has `text` and none is `binary`: `{n} lines` (`1 line`), the sum;
  4. every file has `pages`: `{n} pages` (`1 page`), the sum.

`counts_deep(s, deep)`: exactly `counts(s)` when `deep` is `Deep::default()`. Otherwise the same parts in the same order (items, hidden, selected), then:
- the size: `format_size(deep.bytes)` when `deep.bytes` is known, else `format_size(s.selected_bytes)` when `s.selected_files > 0` (as today);
- `{n} files inside` (`1 file inside`) when `deep.inside` is known;
- the `detail` when there is one;
each joined with ` · `.

## 2. The bar uses it (`mod.rs`)

- The bar observes `aware::shared(cx)` (when it is there: `try_shared`) and redraws when it changes.
- In `refresh_summary`, when `summary.selected` is between 1 and `sentence::MOST`: read the selected entries (`tab.rows.chosen(&tab.selection)`, cloned) and keep them in the bar (`selected: Vec<FileEntry>`); otherwise keep none. On every render work `deep` out from the kept entries and `aware.read(cx).probed(entry).cloned()` (64 lookups at most: cheap).
- `left` draws `counts_deep(s, &deep)` where it drew `counts(s)`. The element's accessible name is that whole text, as today.
- **The numbers roll (R9).** While `deep.bytes` is known and not `done`, or has just changed, the size is drawn with `viz::roll("status-size", from, to, |v| format_size(v).into(), cx)` in place of plain text, and the files inside with `viz::roll("status-inside", …)`: keep the last drawn `bytes` and `inside` in the bar as the `from`. Build the line as a row of pieces (text, roll, text) only in that case; otherwise it stays one piece of text, as today.
- The X-Ray, Rename, Zip and Copy paths buttons stay exactly as they are.

## 3. The dock's end (`chips.rs`, F11)

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Insight {
    /// Files are being written or removed in the folder shown: its last minute.
    Changing([u8; 30]),
    /// The folder's drive is nearly full: its root, as "C:".
    NearlyFull(String),
}
pub fn insight(folder: &str, changing: bool, pulse: Option<[u8; 30]>, drives: &[Drive]) -> Option<Insight>;
```

- `Changing(pulse)` when `changing` and there is a pulse. Else `NearlyFull(letter)` when the drive whose `path` the folder starts with (ignoring case) is `DriveKind::Fixed`, has under 10% free and under 15 GB free (the same numbers as `suggestions.rs`; if it has a function for this that you can call, call it). Else `None`. No insight on This PC, in a search's results, in an archive or on a remote (the caller passes those as an empty `folder`).
- The chip (a method of the bar, in `mod.rs`): a pill 22 px high, fully rounded, `theme::size::XXS`, medium weight, drawn at the start of the right end's row, before the work line, when `wide.large` holds.
  - `Changing`: `viz::spark(&pulse, t.primary)` then `t!("Changing now")`; `t.accent` behind, `t.foreground`. Tooltip `t!("Files in this folder are changing. Press to see what.")`. Pressed: `cx.emit(StatusBarEvent::Insight(insight))`.
  - `NearlyFull`: the `HardDrive` icon then `t!("{drive} is nearly full", drive = letter)`; `t.warning` on `t.warning.opacity(0.13)`. Tooltip `t!("See what takes the space")`. Pressed: the same event.
  - `.id("status-insight")`, `Role::Button`, `aria_label` its words, `.test_support()`.
- `StatusBar` becomes an `EventEmitter<StatusBarEvent>`: `pub enum StatusBarEvent { Insight(Insight) }` (declare it in `mod.rs`, re-export `Insight`). The window handles it (the coordinator wires it: Recent changes for `Changing`, Disk usage for `NearlyFull`).

## Tests (`tests_aware.rs`; the bar's own harness is in `status_bar/tests.rs`: read how it opens a bar over a fake explorer and do the same, with `aware::init_with(cx, sources)` as card D1's tests do)

1. `deep` of nothing, and of 65 files, is the default.
2. One folder with a finished inside of 1,204 files and 3 GB, beside a 1 MB file: `bytes` is their sum, `inside` 1,204, `done`.
3. Two folders, one without an answer: `bytes` and `inside` are `None`.
4. One picture with dimensions: `1920 × 1080`; with a length too: `1920 × 1080 · {length}`.
5. Two songs with lengths: their total; two text files of 1,000 and 230 lines: `1,230 lines`; one of them binary: no detail; one of them unanswered: no detail.
6. Three PDFs of 10, 20 and 18 pages: `48 pages`; one page: `1 page`.
7. `counts_deep` with the default `Deep` equals `counts(s)` for several summaries (items only; hidden; selected files with bytes).
8. `counts_deep`: "25 items · 1 selected · 3.2 GB · 1,204 files inside"; "25 items · 2 selected · 48 KB · 1,230 lines" (build `Summary` by hand; the sizes as `format_size` words them).
9. In a window: selecting a folder whose inside the fake probe then answers changes the bar's accessible text to hold "files inside"; selecting 100 files asks nothing of `probed` (count the calls through a probe that records).
10. `insight`: changing with a pulse is `Changing`; a fixed drive at 5% and 10 GB free is `NearlyFull("C:")`; at 5% and 100 GB free, or 20% free, nothing; a network drive never; `Changing` wins over `NearlyFull`; an empty folder gives nothing.
11. In a window wide enough, a pulse for the active folder (send `CoreEvent::DirChanged` three times through `core_bridge::emit`) shows the chip named "Changing now", and pressing it emits `StatusBarEvent::Insight(Insight::Changing(_))`.
12. Mirrored (`testing::set_rtl(true)`, back to false at the end), the bar with a chip still draws.

## Done when

Every test under `components::status_bar` passes (old and new), clippy is clean, and selecting everything in a 10,000-row folder makes no `probed` lookup (test 9's counter proves it for 100).
