# Card A2 — The lines: beside the files, the ledger, an export

Wave 1 · crate `glint-model` · pure logic, no I/O

**You own:** `crates/glint-model/src/marks/lines.rs` (its tests at the bottom of the file).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M2, M10), §5.2, §5.3, F24 in §4, §8.1. Then `tests/fixtures/marks/README.md` ("`.glint-marks`") and the three `.glint-marks` files beside it.
**Check with:** `cargo test -p glint-model marks::lines` and `cargo clippy -p glint-model --all-targets -- -D warnings`.

## What to build

Fill in `read_beside`, `write_beside`, `read_ledger`, `write_ledger`, `read_export` and `write_export`. All three files are JSON Lines: UTF-8, one JSON object a line, lines ended by `\n` (a `\r` before it is ignored when reading). What they hold comes from outside (a USB stick, another PC): no input may panic, and nothing is trusted (M10).

These functions return what a file says. Tidying marks is card A1's `marks::tidy`, which the callers use; don't copy it here.

### One item's line

Written with its keys in this order, each left out when it has nothing:

| Key | From | Left out when |
| :--- | :--- | :--- |
| `"name"` (beside) or `"path"` (the ledger, an export) | `Line::name` | never |
| `"label"` | `marks.label` | `None` |
| `"tags"` | `marks.tags`, an array | empty |
| `"comment"` | `marks.comment` | `None` |
| `"at"` | `Line::at`, Unix seconds | `0` |
| `"why"` | `"away"` or `"shut"` | `None` |
| every key of `rest` | as it was read | — |

`rest`'s keys are written **sorted by key**, after the known ones. Put the line together by hand (`"key":` then `serde_json::to_string` of the value): a `serde_json::Map`'s own order is sorted when this crate is built alone and as inserted when the workspace is, so it can't be relied on.

Read with `serde_json::from_str::<serde_json::Value>`:

- Not an object, or no string under `"name"` (beside) or `"path"` (the others): the line is **refused**.
- `"label"` and `"comment"`: a string that isn't empty, else none. `"tags"`: the strings of an array that aren't empty, else none. `"at"`: an integer that isn't negative, else `0`. `"why"`: `"away"` or `"shut"`, else none.
- Every other key goes to `rest` with its value untouched.
- A line with no marks and an empty `rest` is dropped (not counted as refused), and is not written either.
- Two lines for the same item (the names equal in `to_lowercase()`): the later one stands.

### Names and paths

- **Beside, `"name"`**: an item of the file's own folder, by its name. Refused when it is empty, `.` or `..`, has any of `\ / : * ? " < > |` or a character below U+0020, or is longer than 255 UTF-16 units.
- **The ledger, `"path"`**: a full path as Glint has it. Refused only when empty or holding `\0`.
- **An export, `"path"`**: a path under the folder exported. A `/` reads as `\`. Refused when it is empty, starts with `\`, or any of its parts (split on `\`) would be refused as a name above (so `C:\x`, `\\server\share\x`, `..\x` and `a\..\b` all are).

### The three files

| | First line | Read by | Caps |
| :--- | :--- | :--- | :--- |
| Beside | `{"glint-marks":1}` | `read_beside` | `BESIDE_MAX_BYTES`, `BESIDE_MAX_LINES` |
| The ledger | `{"glint-ledger":1}` | `read_ledger` | `LEDGER_MAX_BYTES` (new: `64 << 20`) |
| An export | `{"glint-export":1,"labels":[…]}` | `read_export` | `LEDGER_MAX_BYTES` |

- **`read_beside`**: when the first line that isn't empty is an object with an integer `"glint-marks"` of 1 or more, that is `Beside::version` and the line is not an item's (its other keys are ignored). Otherwise `version` is `0` and the line is read as an item's. `refused` counts every refused line.
- **`write_beside`**: the first line with `b.version` (1 when it is 0), then the lines sorted by name (`to_lowercase()`, then the name as written), each ended by `\n`. With no line to write it returns an empty string: the caller removes the file.
- **`read_ledger`**: an object line with a `"glint-ledger"` key is the first line and is skipped; refused lines are left out. **`write_ledger`**: the first line, then the lines sorted by path the same way; an empty string when there are none.
- **`read_export`**: `Err("Not a Glint export")` unless the first line that isn't empty is an object with an integer `"glint-export"` of 1 or more. Its `"labels"` is an array of labels as `Label` is saved (`serde_json::from_value::<Label>`; one that doesn't read is skipped; at most `MAX_LABELS`). **`write_export`**: the first line (with the labels, an empty array when there are none), then the lines sorted by path; the first line is there even with no lines.

**Caps.** Text past the byte cap isn't read: cut it at the last `\n` at or before the cap (never inside a character), and count one refused line for what was cut. In `read_beside`, item lines past `BESIDE_MAX_LINES` are refused, each counted.

## Tests (each a `#[test]`, named as a sentence)

Read the fixtures with `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks/<file>"))`.

1. `later-version.glint-marks`: version 2; two lines; `a.txt` has its label, tag, comment and `at`, and `rest` holds `rating` (4) and `color`; `b.txt`'s `rest` holds `future` with its nested value. Written back, the first line is `{"glint-marks":2}` and `a.txt`'s line is exactly `{"name":"a.txt","label":"Important","tags":["x"],"comment":"c","at":1795436917,"color":"#ff8800","rating":4}`.
2. `leaves-folder.glint-marks`: `ok.txt` alone, six refused.
3. `cut.glint-marks`: `whole.txt` alone, one refused.
4. What is read and written again reads the same (`read_beside(&write_beside(&b)) == b` for a `Beside` whose lines are already sorted), with an Arabic name, a comment of several lines, a quote and a backslash in a tag.
5. Lines are written sorted without regard to case: `b.txt`, `C.txt`, `a.txt` come out `a.txt`, `b.txt`, `C.txt`; no lines write an empty string; a line with no marks and no `rest` is not written.
6. A name with `:`, `*` or a tab is refused; a name of 256 characters is refused and one of 255 is not; the same name twice in two cases keeps the later line.
7. A file with no first line of its own is read with version 0 and loses no item.
8. Nothing panics and everything is refused for: an empty text, `null`, `42`, `"x"`, `[]`, `{`, a line of 1,000 `{`, `{"name":7}`.
9. The caps: 100,001 item lines keep 100,000 and refuse one (make each line shorter than 35 bytes, so that the byte cap isn't what stops it); a text of 5 MB that is one line keeps nothing and refuses one.
10. The ledger: a line with `"why":"away"` and one with `"why":"shut"` read and write back; `"why":"other"` reads as none; a line with `"name"` and no `"path"` is left out; two paths equal in another case keep the later.
11. An export: labels and lines read back as written; a text whose first line is a `.glint-marks`'s is `Err`; the paths `C:\x`, `\\server\share\x`, `..\x`, `a\..\b`, `\abs` and `a:b` are left out, and `sub/notes.md` reads as `sub\notes.md`.

## Done when

The tests pass, clippy is clean, and no function in the file reads the disk or the clock. Say in your report that `LEDGER_MAX_BYTES` is new.
