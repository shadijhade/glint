# Card A1 — What a selection is, and what to ask about it

Wave 1 · crate `glint-model` · pure logic, no I/O

**You own:** `crates/glint-model/src/aware/shape.rs` (its tests at the bottom of the file).
**Read first:** PLAN.md §5.1; `crates/glint-model/src/entries.rs` (`FileEntry` at line 88, `is_archive_file`, `is_archived`, `is_remote`, `is_closed`, `is_recycled`, `is_online_only`); `crates/glint-model/src/file_types.rs` (`type_group`, `TYPE_GROUPS`).
**Check with:** `cargo test -p glint-model aware::shape` and `cargo clippy -p glint-model --all-targets -- -D warnings`.

## What to build

Fill in `Shape::of`, `Shape::key`, `Shape::is_many`, `shared_ext` and `want_for`. The signatures are in the file already.

### One file's class (a private enum `One`), first match wins

| # | When | Class |
| :--- | :--- | :--- |
| 1 | `is_archive_file(e)` | Archive |
| 2 | `e.ext` is `iso`, `img`, `vhd` or `vhdx` | DiskImage |
| 3 | `e.ext` is `exe`, `com`, `scr` or `msc` | Program |
| 4 | `e.ext` is `msi`, `msix`, `msixbundle`, `appx` or `appxbundle` | Installer |
| 5 | `e.ext` is `bat`, `cmd`, `ps1`, `vbs` or `wsf` | Script |
| 6 | `e.ext` is `lnk` or `url` | Shortcut |
| 7 | `e.ext` is `pdf` | Pdf |
| 8 | `e.ext` is `csv`, `tsv`, `tab`, `psv`, `json`, `jsonl`, `ndjson` or `parquet` | Data |
| 9 | `*type_group(e)` is `TYPE_GROUPS.images` / `.video` / `.audio` / `.documents` / `.code` / `.fonts` | Picture / Video / Song / Document / Code / Font |
| 10 | anything else | Other |

`e.ext` is already lower case.

### `Shape::of(targets)`

- No targets: `Nothing`.
- Every target is a folder (`e.dir`): `Folder` for one, `Folders` for more.
- Folders and files together: `Mixed`.
- Only files, all of one class:
  - one file: `Archive`, `DiskImage`, `Program`, `Installer`, `Script`, `Shortcut`, `Picture`, `Video`, `Song`, `Pdf`, `Document`, `Data`, `Code`, `Font`, and `File` for Other;
  - several: `Archives`, `Pictures`, `Videos`, `Songs`, `Pdfs`, `Documents`; `Data` and `Code` stay `Data` and `Code`; every other class (DiskImage, Program, Installer, Script, Shortcut, Font, Other) is `Files`.
- Only files, of different classes: `Files`.

### `Shape::key()`

Exactly these, never to change (they are saved to disk): `nothing`, `folder`, `folders`, `archive`, `archives`, `disk-image`, `program`, `installer`, `script`, `shortcut`, `picture`, `pictures`, `video`, `videos`, `song`, `songs`, `pdf`, `pdfs`, `document`, `documents`, `data`, `code`, `font`, `file`, `files`, `mixed`.

### `Shape::is_many()`

True for `Folders`, `Archives`, `Pictures`, `Videos`, `Songs`, `Pdfs`, `Documents`, `Files`, `Mixed`.

### `shared_ext(targets)`

`None` when there are no targets, a folder is among them, the extensions differ, or the shared extension is empty. Otherwise `Some(ext)`.

### `want_for(entry)`

Returns `Want::default()` (ask nothing) when any of these holds: `is_archived`, `is_remote`, `is_recycled`, `is_closed`, `is_online_only`, or the path does not start with a drive letter, a colon and a backslash (`C:\…`; write a small private `on_a_drive(path: &str) -> bool`, as `crates/glint-ui/src/components/menus/kinds.rs:72` does).

Otherwise:
- a folder: `inside` only;
- a file: `origin`, `locked_by` and `opens_with` always, plus
  - `signer` for class Program or Installer;
  - `media` for Picture, Video or Song;
  - `text` for Code, and for Data other than `parquet`, when `entry.size <= 64 * 1024 * 1024`;
  - `pages` for Pdf;
  - `target` when `entry.ext == "lnk"`;
  - `twins` when `1024 <= entry.size <= 256 * 1024 * 1024`.

## Tests (each a `#[test]`, named as a sentence)

Build entries by hand: `FileEntry { name, path, dir, size, modified: 0, attrs: 0, lower: name.to_lowercase(), ext, origin: None }` (look at the struct for the exact field types; write one private helper `file(name, size)` and one `folder(name)` that put them in `C:\x`).

1. Nothing selected is `Nothing`; one folder `Folder`; two `Folders`; a folder and a file `Mixed`.
2. Each row of the class table gives its singular shape for one file (one assert per row, with a real name: `a.zip`, `a.iso`, `a.exe`, `a.msi`, `a.ps1`, `a.lnk`, `a.pdf`, `a.csv`, `a.png`, `a.mp4`, `a.mp3`, `a.docx`, `a.rs`, `a.ttf`, `a.xyz`).
3. A `.json` file is `Data`, not `Code` (rule 8 comes before rule 9).
4. Two zips are `Archives`; two pictures `Pictures`; two programs `Files`; a picture and a PDF `Files`; two `.rs` files `Code`.
5. Every shape's key is unique, lower case, and has no space (loop over all the variants in an array).
6. `shared_ext`: two `.png` give `png`; a `.png` and a `.jpg` give `None`; a folder among them gives `None`.
7. `want_for`: a folder asks only `inside`; a `.exe` asks `signer`; a `.png` asks `media` and not `text`; a 100 MB `.rs` file does not ask `text`; a 100-byte file does not ask `twins`; a `.lnk` asks `target`.
8. `want_for` asks nothing for an entry inside an archive, on a remote, in the Recycle Bin, online-only, or on `\\server\share\a.txt` (set the bits from `glint_model::entries::attr`).

## Done when

The tests pass, clippy is clean, and no function in the file reads the disk or the clock.
