# Card A3 — XYplorer's tag database, read and written

Wave 1 · crate `glint-model` · pure logic, no I/O

**You own:** `crates/glint-model/src/marks/xyplorer.rs` (its tests at the bottom of the file).
**Read first:** [STANDARDS.md](../../STANDARDS.md); the rules for every agent in [the Aware plan](../../aware/PLAN.md), section 6 (with this card's commands for its rule 4); this wave's [PLAN.md](../PLAN.md) §3 (M7, M10), §7.1, §7.3 ("Reading", "What an item gets"), §7.4, §8.1, §14 items 1 and 2. Then `tests/fixtures/marks/README.md` ("XYplorer's `tag.dat`"): it says what every fixture must read as.
**Check with:** `cargo test -p glint-model marks::xyplorer` and `cargo clippy -p glint-model --all-targets -- -D warnings`.

No real `tag.dat` has been seen yet: the format here is XYplorer's manual as quoted, and the fixtures are made from it. Read leniently and keep each rule in one place, so that a real file can correct it later.

## `read(bytes) -> Result<XyFile, String>`

**Text.** More than `XY_MAX_BYTES`: `Err("This file is too big to be a tag database")`. Bytes starting `FF FE` are UTF-16LE after them; `FE FF`, UTF-16BE; `EF BB BF`, UTF-8; with none of these, UTF-16LE when the second byte is `0`, else UTF-8. A unit that isn't valid becomes U+FFFD; an odd last byte is ignored. Lines end at `\n`, and a `\r` before it is dropped.

**The first line** that isn't empty must start with `XYplorer File Tags v` and a number, which is `version`. Otherwise `Err("Not an XYplorer tag database")`. A version above 5 is read by the same rules.

**Sections**, found by their names wherever they are (never by line numbers):

| Line | Then |
| :--- | :--- |
| `Labels:` | the next line that isn't empty is the labels |
| `Extra Tags:` | each line up to the next empty one is an extra field's definition |
| starts with `Storage:` | the number after it is `storage` (`0` when there is none or it doesn't read) |
| `Data:` | every line after it that isn't empty is an item |

Any other line outside a section is ignored. A file with no `Data:` has no items and is not an error.

- **Labels**: split on `;`, each on `|` into name, text color, color. Colors are six hex digits; an empty or unreadable text color is `0x000000`, an empty or unreadable color `0xFC7268` (what XYplorer's version 3 notes give). A label with an empty name **keeps its place**: an item's label is a place in this list.
- **A definition**: split on `|`; the first field is the caption, the second the type: `0` Text, `1` Number, `2` Date, `3` Checkbox, `4` Rating, `5` List, `6` Location, `7` Icon, `8` Image, `9` Script, any other number `Other(n)`, unreadable `Text`. An unused one (`|0|0|0|||||`) still counts: the number of definitions is the number of extra fields in every item.
- **An item**: split on `|` into at most `extras.len() + 4` parts (`splitn`), so the comment, last, keeps its own `|`: the path, the label, the tags, one value for each extra field, the comment. Parts that are missing are empty, so `XyItem::extras` always has `extras.len()` values. A line with an empty path is skipped.
  - `label`: the number, `0` when it doesn't read.
  - `tags`: split on `,`, each trimmed, empty ones dropped, one of each in any case (`to_lowercase()`), the first spelling kept.
  - `comment` and the extras' values: as written, `¶` included.

## `marks_of(file, item, extras) -> Marks`

- **Label**: the name of the label at place `item.label` (from 1) in `file.labels`. `0`, a place past the list, or an empty name: none. The name is XYplorer's as written (*Grey* stays *Grey*: `label_of` knows it).
- **Tags**: the item's.
- **Comment**: the item's with every `¶` made `\n`, trailing white space trimmed; none when nothing is left.
- With `extras` true, each extra field whose value isn't empty adds, by its type (§7.4). `Caption` is the definition's caption, or `Extra 3` (its place, from 1) when that is empty; values are trimmed, and `¶` in one is `\n`.

| Type | Adds |
| :--- | :--- |
| Checkbox | the tag `Caption` |
| Rating, List | the tag `Caption: value` |
| Text, Number, Date, Location, Image, Script, `Other` | the line `Caption: value` to the comment |
| Icon | nothing |

  The lines go at the end of the comment in the fields' order, after one empty line when there is a comment before them. A tag an item already has (any case) is not added twice.

## `write(items, labels) -> Vec<u8>`

An `XYplorerTag.dat` XYplorer's `tagload` merges: the bytes `FF FE`, then UTF-16LE, lines ended by `\r\n`:

```
XYplorer File Tags v5

Labels:
Red|000000|EF4444;Orange|000000|F97316
                                              (an empty line)
Extra Tags:
|0|0|0|||||                                   (sixteen of these)

Storage: 3 (Relative to tags database)

Data:
bond0.conf|1|bond, netplan|||||||||||||||||mode 4¶LACP rate fast
```

- **Labels**: `Name|000000|RRGGBB` for each of `labels`, joined by `;`; a `|` or `;` in a name is written as a space. No labels: an empty line.
- **Items**: sorted by path, byte by byte (XYplorer keeps its file in that order). `path|label|tags`, then seventeen `|`, then the comment. `label` is written as `0` when it is past `labels`. Tags are joined by `, `, a `|` or `,` inside one written as a space. In the comment `\r\n` and `\n` are `¶`. `item.extras` is not written.

## Tests (each a `#[test]`, named as a sentence)

Read the fixtures with `include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/marks/<file>"))`.

1. `v5-storage0-bom-crlf.dat`: version 5, storage 0, seven labels (the first `Red`, `0x000000`, `0xFC7268`), sixteen extras of which the first four are `Status` List, `Stars` Rating, `Reviewed` Checkbox, `Due` Date, and the seven items of the README's table, each field as written there (the tags of `spaces.txt` are `lead`, `trail`, `dup`; the comment of `pipes.txt` is `left | right || end`).
2. `marks_of` on those items: `bond0.conf` is `Red`, tags `bond`, `netplan`, comment `mode 4\nLACP rate fast`; `archive` is `Grey`; `past.txt` has no label; the Arabic item's name, tags and comment come through. `review.docx` with extras has the label `Blue`, the tags `Status: In review`, `Stars: 4`, `Reviewed` and the comment `Due: 2026-11-30`; without extras it has only the label.
3. `v5-storage1-nobom-lf.dat` (no byte-order mark, `\n`): storage 1, both items as the README says.
4. `v5-storage2.dat`, `XYplorerTag-storage3.dat` and `v5-storage4.dat`: storage 2, 3 and 4, and their paths as written.
5. `v3-storage0.dat`: version 3, nine labels, five extras; `old.txt` has label 8 (`Work`), tags `legacy`, `v3`, the first extra `netplan`, and with extras its comment is `kept\nsince v3`, an empty line, `Topic: netplan`; `plain.txt`'s comment is `a | b`.
6. What is written reads back: `read(&write(&items, &labels))` is version 5, storage 3, sixteen extras, the labels' names and colors, and the items sorted by path with their labels, tags and comments. The bytes start `FF FE`, and every line ends `\r\n`.
7. Writing protects the format: a label named `a|b;c`, a tag `x,y|z` and a comment `one\ntwo | three` read back as `a b c`, `x y z` and `one¶two | three`.
8. Not a database: an empty slice, `hello`, a `.glint-marks` file's bytes, and a first line `XYplorer File Tags v` with no number are each `Err`.
9. Nothing panics on: the first fixture cut at every length from 0 to its size (step 7), a file of `|` only after `Data:`, an odd number of bytes, a label number `99999999999`.
10. A later version (`XYplorer File Tags v6`, the rest as version 5) reads as version 6 with its items.
11. A file with definitions but no `Data:` has no items. A UTF-8 copy of a fixture (with and without `EF BB BF`) reads the same as its UTF-16 one.
12. Ten thousand lines (the first fixture's seven item lines repeated with a number put in each path, encoded as it is) read in under half a second in this test build, and give ten thousand items. (The 50 ms of §6.6 is the release build's, which the coordinator measures.)

## Done when

The tests pass, clippy is clean, and nothing in the file reads the disk or the clock. Say in your report every place where the README and the fixtures disagree with this card, and anything about the format you had to guess.
