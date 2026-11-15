# Card A3 — What to put forward

Wave 1 · crate `glint-model` · pure logic, no I/O

**You own:** `crates/glint-model/src/aware/rank.rs` (its tests at the bottom of the file).
**Read first:** PLAN.md §2 (R1, R3), §5.1 (`Facts`, `Probed`, `Habits`, rank.rs) and §5.5 (the keys). You call `Habits::action_uses` and `Habits::action_weight`; card A2 writes them at the same time as you write this, so in your tests build `Habits` by filling its public maps directly (`habits.actions.entry("pictures".into()).or_default().insert(key.into(), Use { n, day })`) and, until A2 lands, expect those two methods to return zero: write the habit tests anyway, they must pass once A2 is in (say so in your report if they can't be run yet).
**Check with:** `cargo test -p glint-model aware::rank` and clippy on the crate.

## What to build

`suggest`, `is_basic`, `usual_place`.

### `is_basic(key)`

True for every key that starts with `cmd:`, and for exactly these: `task:toggle-pin`, `task:windows-menu`, `task:copy-text`, `task:show-properties`, `task:navigate`, `task:refresh-drives`, `task:get-access`, `do:tags`, `do:edit-tags`, `do:paste-into`, `do:paste-queued`, `do:open-scripts-folder`, `do:reveal-in-file-explorer`, `do:reveal-archive`, `do:forget-recent`, `do:restore`, `do:empty-recycle-bin`, `do:install-tool`, `do:batch-rename-folder`, `do:copy-to-choose`, `do:move-to-choose`.

### The rules

Each rule that holds gives one key a score and a reason. `MB` is 1024 × 1024 bytes. "probed" is the `probed` argument (rules that need it don't hold when it is `None`, or when the field they read is `None`).

| # | Holds when | Key | Score | Why |
| :--- | :--- | :--- | :--- | :--- |
| 1 | `probed.locked_by` has at least one name | `do:locks` | 9.5 | `InUse(first name)` |
| 2 | shape is `Archive` and not `facts.fixed` | `do:extract-here` | 9.0 | `Plain` |
| 3 | shape is `DiskImage` and not `facts.fixed` | `do:mount` | 9.0 | `Plain` |
| 4 | shape is `Program`, `Installer` or `Script`, and `probed.origin` is blocked | `do:defender` | 8.5 | `FromInternet(host)` |
| 5 | always: the menu has a Compare row only when there are two files to compare, so being in `available` is the condition | `do:compare` | 8.0 | `Plain` |
| 6 | shape is `Program` or `Installer`, and `probed.signer` is `Unsigned` | `do:run-in-sandbox` | 7.5 | `Unsigned` |
| 7 | shape is `Folder`, `probed.inside` is `done` and its `junk >= 500 MB` | `do:cleanup` | 7.5 | `BuildOutput(junk)` |
| 8 | shape is `Pdfs` | `do:pdf:merge` | 7.0 | `Plain` |
| 9 | shape is `Picture` and `facts.first_name` starts with `screenshot` | `do:copy-text-in` | 6.5 | `Plain` |
| 10 | shape is `Video` and `facts.bytes >= 200 MB` | `do:make-smaller` | 6.0 | `Plain` |
| 11 | shape is `Data` and `facts.count >= 2` | `do:query-together` | 6.0 | `Plain` |
| 12 | shape is `Pictures` | `do:convert-pictures` | 5.0 | `Plain` |
| 13 | `probed.origin` is blocked and the shape is not `Program`, `Installer` or `Script` | `do:unblock` | 5.0 | `FromInternet(host)` |
| 14 | shape is `Song` or `Songs` | `do:edit-songs` | 4.0 | `Plain` |
| 15 | shape is `Document` or `Documents` | `do:convert-document` | 3.0 | `Plain` |
| 16 | not `facts.fixed` (the row is there only with two panes, and can be chosen only when the other can take items) | `do:copy-other` | 2.5 | `Plain` |

Write the rules as one private function that returns `Vec<(&'static str, f64, Why)>`, one `if` per row in this order, so the table can be read off the code.

### `suggest(facts, probed, available, habits, today, most)`

For every key in `available` (in order, skipping a key seen before and every basic key):

1. `rule` = the rule for that key, if one holds (no key has two rules).
2. `uses = habits.action_uses(facts.shape, key)`, `w = habits.action_weight(facts.shape, key, today)`. `bonus = if uses >= 3 { w.min(8.0) } else { 0.0 }`.
3. `score = rule's score (0.0 without one) + bonus`.
4. `why` = the rule's reason when there is a rule and its reason isn't `Plain`; otherwise `Usual` when `bonus >= 2.0`; otherwise `Plain`.
5. Keep it when `score >= 2.5`.

Sort what is kept by score, highest first; equal scores keep the order of `available` (use a stable sort). Return the first `most`.

A key that a rule names but that is not in `available` is never returned: the menu has no such row to copy (R3).

### `usual_place(places)`

The first place with `uses >= 3` and `share >= 0.6` (the list is heaviest first already).

## Tests

Write a helper `facts(shape)` giving a `Facts` with that shape, `count: 1` and the rest default, and a helper `all()` returning a `Vec<&str>` of every key the rules name plus `cmd:open`, `cmd:delete`, `do:x-ray`, `do:compress:zip`.

1. One test per rule row: with the facts and answers that make it hold, its key comes first with its score and reason; with them one step short (not locked, `fixed`, 499 MB of junk, a 199 MB video, one data file, the host unblocked), the key is not returned. Rules 5 and 16 are tested by the key being in `available` or not.
2. A locked archive offers `do:locks` before `do:extract-here` (9.5 before 9.0).
3. A blocked unsigned program offers `do:defender` then `do:run-in-sandbox`, and not `do:unblock`; a blocked picture offers `do:unblock`.
4. Nothing basic is ever returned, however often it was done (give `cmd:delete` a hundred uses today).
5. A key that isn't in `available` isn't returned even when its rule holds.
6. Two uses give no bonus; three uses today add 3.0 and the reason is `Usual`; the bonus stops at 8.0.
7. A rule's reason wins over `Usual` when it isn't `Plain`; a `Plain` rule with a bonus of 2.0 or more says `Usual`.
8. Old habits fade: three uses 90 days ago (weight 0.375) don't reach 2.5 alone.
9. Equal scores keep the menu's order; `most` cuts the list; `most == 0` gives nothing.
10. `Nothing` (the folder's own menu) has no rule, and offers a habit (`task:open-terminal`, five uses today).
11. `usual_place`: three uses with a share of 0.6 is one; two uses isn't; a share of 0.5 isn't; an empty list gives `None`.

## Done when

The tests pass (the habit ones too once A2 is in), clippy is clean.
