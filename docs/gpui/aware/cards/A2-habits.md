# Card A2 — What the person did before

Wave 1 · crate `glint-model` · pure logic, no I/O

**You own:** `crates/glint-model/src/aware/habits.rs` (its tests at the bottom of the file).
**Read first:** PLAN.md §5.1 (habits.rs) and §5.5; `crates/glint-model/src/rank.rs` lines 16 to 60 (`Visit`, `frecency`: the same idea for folders, for comparison only).
**Check with:** `cargo test -p glint-model aware::habits` and clippy on the crate.

## What to build

Counts of what the person chose, kept small, that fade with time. Nothing here knows about menus: an action is a string, a folder is a string.

Constants (public): `HALF_LIFE_DAYS: f64 = 30.`, `MOST_ACTIONS: usize = 48` (per shape), `MOST_PLACES: usize = 16` (per extension).

- `weight(used, today)`: `used.n as f64 * 0.5_f64.powf(days / HALF_LIFE_DAYS)`, where `days` is `today.saturating_sub(used.day) as f64` (a day in the future counts as today).
- `day_of(unix_ms)`: whole days since 1970-01-01: `(unix_ms.max(0) / 86_400_000) as u32` (clamp so it can't overflow).
- `note_action(shape, action, today)`: under `shape.key()`, the entry for `action` gets `n` one higher (`saturating_add`) and `day = today`. An empty `action` is ignored. If the shape then holds more than `MOST_ACTIONS`, drop the one with the lowest `weight` (ties: the key that sorts last).
- `note_place(ext, folder, today)`: `ext` is lower-cased. `folder` loses one trailing backslash unless it is a drive's root (`C:\`). An empty folder is ignored. If a folder already kept equals it ignoring case (compare `to_lowercase()`), that one is counted (its spelling stays); else it is added as given. Over `MOST_PLACES`: drop the lowest weight, as above.
- `action_weight(shape, action, today)`: the weight, `0.` when never done. `action_uses`: its `n`, `0` when never done.
- `overall_weight(action, today)`: the sum of the action's weight over every shape.
- `places_for(ext, today)`: every folder kept for `ext` (lower-cased), as `Place { folder, uses: n, weight, share }`, heaviest first (ties: the later `day` first, then the folder's name). `share` is its weight over the sum of all the weights for that extension (`0.` when the sum is zero).
- `forget()`: empties both maps. `is_empty()`: both are empty.

**The JSON shape is part of the contract** (it is written into the person's `state.json`):

```json
{"actions":{"pictures":{"do:convert-pictures":{"n":3,"day":20407}}},"places":{"pdf":{"D:\\Papers\\Invoices":{"n":4,"day":20407}}}}
```

`#[serde(default)]` is on the struct already: a file from before this wave, or with one map missing, loads.

## Tests

1. A fresh `Habits` is empty and writes `{"actions":{},"places":{}}`; the JSON above reads back to the same value that wrote it.
2. Noting an action three times on one day gives `uses == 3` and `weight == 3.0` that day, `1.5` thirty days later, and `0.75` sixty days later (compare with a tolerance of `1e-9`).
3. An action noted again later carries the new day: two uses, the second 30 days after the first, weigh `2.0` on the second day.
4. A day in the past of `today` never makes the weight grow; a `day` after `today` weighs as today.
5. Actions are kept per shape: noted for `Picture`, the weight for `Pictures` is `0.`; `overall_weight` sums both.
6. The 49th different action of a shape pushes out the lightest one, and the newest stays.
7. `note_place("PDF", r"D:\Papers\Invoices\", d)` then `note_place("pdf", r"d:\papers\invoices", d)` is one folder used twice, spelled as first given; `C:\` keeps its backslash.
8. `places_for` is heaviest first, and the shares add up to `1.0`; an extension never used gives an empty list.
9. `forget` empties everything.
10. `day_of(0) == 0`, `day_of(86_400_000) == 1`, a negative time is day 0.

## Done when

The tests pass, clippy is clean, nothing reads the clock (`today` is always given).
