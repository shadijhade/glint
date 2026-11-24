# The Marks wave: a label, tags and a comment that stay with the file

*Plan for 0.17, for [issue #8](https://github.com/shadijhade/glint/issues/8) (Label, Tags and Comment as in XYplorer, and XYplorer's own brought along). Written 2026-11-23 against `44679b2`. Read [STANDARDS.md](../STANDARDS.md) first. The shape follows [the Aware wave's plan](../aware/PLAN.md): what the person sees, how it is built, contracts, cards in waves. Wave 1's cards are in [cards/](cards/); each later wave's are written when the wave before it is in.*

## 1. Why

The issue asks for three things on every file and folder: a **Label** (one color that says a status), **Tags** (several words), and a **Comment** (free text), each a column, edited from the menu, a panel or a key, searched with `label:`, `tag:` and `comment:`, following the file when it is copied or moved, and exported and imported.

XYplorer keeps all of this in one file, `tag.dat`, a list of paths. That is why a file renamed in a terminal loses its tags there, and why its forum has threads about orphans, portable drives and syncing. Glint already does the hard part differently: a file's tags are **in the file** (an NTFS stream), so any program can rename or move it, and the instant search service reads them out of the Master File Table in the pass that reads the names. This wave finishes that idea rather than copying XYplorer's:

1. **The mark is in the file.** Label, tags and comment are three small plain-text streams of the file itself. Where a drive has no streams, they are in a plain-text file beside it. A list of paths is the last resort, never the truth.
2. **One key.** `Alt+1` labels what is selected. Nothing opens.
3. **Found at once.** `label:pending`, `tag:netplan`, `comment:lacp` answer from the index as names do, and a plain word typed in the search box finds a file by its comment too.
4. **Nothing is lost coming from XYplorer, and nothing is taken from it.** Glint finds `tag.dat`, shows what it would do, does it with one Undo, can keep following XYplorer while both are in use, and can write XYplorer's format back.

## 2. What is there today

Verified in the tree; every later section builds on these.

- **Tags are a stream.** `file:glint.tags`, UTF-8, one tag per line, at most 4 KB read; writing keeps the file's dates (`crates/glint-core/src/tags.rs:12`, `:47`, `:67`). Folders take them too.
- **The index reads them for free.** A resident `glint.tags` is parsed with the record (`shadui-index/src/record.rs:177`, `:336`), kept per record (`tree.rs:207`), saved beside the snapshot as `.tags` (`engine.rs:1493`), searched by `tag:` (`query.rs:755`, `tree.rs:596`, `:1087`) and counted for the sidebar (`Request::Tags`, `proto.rs:55`). Every journal record re-reads its file's record (`usn.rs:85`), so a changed stream is in the index within about 0.3 s.
- **The interface.** Seven color tags, Red to Gray (`glint-ui/src/dialogs/tags.rs:30`); dots after the name in a row (`file_list/cells.rs:449`) and on a tile (`file_grid/tile.rs:399`); a Tags submenu under More (`menus/item.rs:1355`); the Edit tags… dialog (`dialogs/tags.rs:366`); Tags in the sidebar (`sidebar/rows.rs:137`); rows on screen read through `RowFacts` (`components/row_facts.rs:512`).

What is missing or wrong, each closed by a card in section 9:

| | Gap | Where |
| :--- | :--- | :--- |
| G1 | No label, no comment, no column, sort or group for any of the three | `glint-model/src/columns.rs:18` |
| G2 | Filtering the open folder as you type does not know `tag:`: it becomes a name match and shows nothing | `glint-model/src/entries.rs:844`, `:1040` |
| G3 | A tags stream that is not resident in the record is not read: the file counts as untagged in the index | `shadui-index/src/record.rs:336` |
| G4 | Sorting by a column read from the file is not wired: the rows are built with no reads | `glint-ui/src/explorer/rows.rs:611` |
| G5 | Tagging cannot be undone | `glint-ui/src/dialogs/tags.rs:262` |
| G6 | Tools that write a new file and recycle the old one lose its streams (Make smaller, Remove personal info, PDF tools, Convert document, Media, Convert to UTF-8, Convert pictures in place) | `glint-core/src/actions.rs:145`, `convert.rs:218` |
| G7 | "This drive keeps no tags" is guessed from an error's text; nothing asks the volume | `glint-core/src/tags.rs:102` |
| G8 | Edit tags… suggests nothing as you type; a tag in the sidebar has no menu; `Ctrl+K` and AI agents have no tag commands | `dialogs/tags.rs:366`, `sidebar/mod.rs:783`, `glint-core/src/mcp.rs:86` |
| G9 | ReFS drives: the index holds no tags | `shadui-index/src/refs.rs:383` (stays, section 12) |

## 3. Rules of the wave (they outrank a card's wording)

- **M1. The file is the home.** Where the drive keeps streams, a mark is in the file's own stream and nowhere else.
- **M2. Plain text everywhere.** Every home is UTF-8 text a person can read and a script can write: `Get-Content notes.conf -Stream glint.comment`. No database, no binary format outside the index's own snapshot.
- **M3. Marking never changes a file's dates or contents** (`tags::keep_times`), in every home.
- **M4. Only what is known is drawn** (the Aware wave's R1). A row whose marks are not read yet draws none: no placeholder.
- **M5. A listing costs what it costs today.** No mark is read to list a folder. Rows on screen are read in batches off the UI thread, as tags are. Every row is read only when a marks column sorts, groups or filters.
- **M6. Every change can be taken back** with `Ctrl+Z`, an import included.
- **M7. XYplorer's files are only read.** Never written, moved or held open.
- **M8. What 0.16 wrote keeps working.** `glint.tags` keeps its name and format. A 0.16 app or service meeting a 0.17 file sees its tags as before.
- **M9. No new crates.** `serde_json`, `windows`, `rayon`, `notify` and `blake3` are in the build.
- **M10. What comes from outside is data.** A sidecar from a USB stick, a `tag.dat`, an imported file: sizes are capped, a name with `\`, `/` or `..` in a sidecar is refused (it can only speak of its own folder), nothing in them is run, and none of it can panic (STANDARDS §3).
- **M11. A label is not a state.** Selection, hover and drop keep their one fill (STANDARDS, "A state is a fill"); a label is drawn under them and never in the accent.

## 4. What the person sees

The numbers are how cards and tests refer to features.

### The three marks

- **F1. Label.** One per item, from a short list of named colors. It starts as today's seven (Red, Orange, Yellow, Green, Blue, Purple, Gray: also XYplorer's seven). **Settings → Files → Labels** renames one ("Red" to "Important"), recolors, adds (32 at most) and reorders. In the list a labeled row is washed in its color, inside the row's rounded line (a selected row shows its selection alone); a tile's name sits on a pill of it.
- **F2. Tags.** As today. Edit tags… now suggests as you type: tags that start with what is typed, then tags that contain it, the most used first, with the ones used in this folder ahead.
- **F3. Comment.** Free text, several lines, 64 KB at most. A row with one has a small note mark after its name; the mark's tooltip is the comment.

### In the list and the grid

- **F4. Three columns**: **Label** (a pill with its name), **Tags** (chips; the dots leave the name's cell while this column shows), **Comment** (its first line, the rest in the tooltip). They are chosen, sized and remembered per folder as every column is, and each sorts. **Group by → Label** groups in the labels' order, *No label* last.
- **F5. The pane's head knows the labels in it.** Beside the chips for kinds: `Important 4`, `Pending 2`, each with its color. Pressing one puts `label:"Important"` in the search box; again takes it out. Shown when known: at once with instant search, else for folders of up to 5,000 items once they are read.

### Setting them

- **F6. Keys**, in the files: `Alt+1`…`Alt+9` give the selection the first nine labels (pressed again on items that all have it: takes it off), `Alt+0` takes any label off, `Alt+T` opens Edit tags…, `Alt+N` opens Edit comment…. All are free today (`actions.rs`).
- **F7. The menu.** A row of the labels' dots right under the strip of icon buttons: one press labels. Under **More**, the Tags submenu becomes **Marks**: *Tags…*, *Comment…*, *Copy marks*, *Paste marks*, *Clear marks*.
- **F8. The preview pane and Quick Look** start with a **Marks** block for every kind of item: the labels' dots, the tags as chips with a box to add one, and the comment in a box that grows from one line to six. It saves when the focus leaves it or on `Ctrl+Enter`; `Esc` puts back what was there. With several items selected it shows what they share, and *Several comments* where they differ.
- **F9. `Ctrl+K`**: *Label: Important* (one for each label), *No label*, *Edit tags…*, *Edit comment…*, *Copy marks*, *Paste marks*, *Clear marks*, *Import marks…*, *Export marks…*.
- **F10. Copy marks, Paste marks.** Copy takes the focused item's marks (as text: the sidecar's line); Paste gives them to the selection: tags are added, a label or a comment replaces.
- **F11. Undo.** Every change is one step with its words ("label of 3 items") and a notice with **Undo**.

### Finding them

- **F12. Filters.** `label:important` (`label:red;orange` for either), `tag:` as today, `comment:lacp` (every word must be in it; quotes keep a phrase), `has:label`, `has:tag`, `has:comment`, `has:mark`. `!` before any of them means not: `!label:archived`, `!has:comment`. XYplorer's names are understood: `lbl:`, `tags:`, `cmt:`. A label answers to `tag:` too, so `tag:red` finds what it found before.
- **F13. In all three places a search runs**: filtering the open folder as you type (new, G2), the index, and fd's drives.
- **F14. Typos forgiven.** When no tag or label has the name typed, the nearest ones stand in (the rule names have: four letters or more, one edit, two from eight), under **Close matches for…**.
- **F15. One search finds comments.** A plain word in the search box also finds the files whose comment or tags hold it, listed under **In comments** with the matching line under each, as **Inside files** does. Where instant search covers the drive, and in the open folder.
- **F16. Plain words.** `important pdfs`, `tagged netplan`, `with comments`, `labeled pending from last week`.
- **F17. The sidebar** lists **Labels** (in their order, with counts) above **Tags**. A tag's row gets a menu: *Rename everywhere…*, *Take off every file…*.

### Staying with the file

- **F18. On NTFS and ReFS** the marks are in the file. Rename it, move it, copy it with any program: they go along. They follow it into the Recycle Bin and back. A folder's own marks stay with it when it is renamed, or moved on its drive; Windows never copies them (it copies a file's streams, not a folder's), so a copy of a folder made by another program is bare. Glint's own copies carry them (F21).
- **F19. On a drive without streams** (FAT, exFAT, a camera's card, some NAS shares) they are in `.glint-marks`, a hidden text file in the same folder. Copy the folder to any system and the marks are in it.
- **F20. Where neither can be written** (a read-only share, inside an archive, a remote, a file your account may not change) they are kept on this PC, by path. The Marks block says *Kept on this PC*.
- **F21. Glint's copies carry them across.** A copy or move from NTFS to exFAT writes the lines beside the copies; from exFAT to NTFS it writes the streams. Through Windows' file engine, the copy queue, a drop and the other pane alike.
- **F22. A tool that makes a new file in the old one's place gives it the old one's marks** (G6).
- **F23. A folder on NTFS that holds a `.glint-marks`** (copied back from a stick, or unpacked) shows its marks, and its head says *12 marks here are beside the files* with **Move them into the files**.

### Out and in

- **F24. Export marks…** on a folder: everything marked below it into one `.jsonl` file (the sidecar's lines with paths relative to the folder, the labels in the first line), or into XYplorer's format.
- **F25. Import marks…**: a Glint export, an XYplorer `tag.dat` or `XYplorerTag.dat`. One preview, one Undo. Such a file's own menu offers it under Suggested.
- **F26. The settings file** (Settings → About) gains a part: **Labels** (with the tags used lately).

### XYplorer

Section 7: found by itself (F27), a preview that says where everything is (F28), brought in with one Undo (F29), followed while both are used (F30), marks that wait for a drive (F31), the same words (F32), a way back (F33).

### Scripts and agents

- **F34. PowerShell works on them as they are**: `Set-Content bond0.conf -Stream glint.label -Value Important`, `Get-Content bond0.conf -Stream glint.comment`. `docs/marks.md` says so with the formats.
- **F35. AI agents** get `get_marks` and `set_marks` (asked about in a dialog, as the other tools that change something are), and `search_files` says it takes `tag:`, `label:` and `comment:`.

## 5. Where marks live

### 5.1 In the file: three streams

| Stream | Holds | Read at most |
| :--- | :--- | :--- |
| `glint.tags` | one tag per line (as today) | 4 KB |
| `glint.label` | one line: the label's **name** | 256 bytes |
| `glint.comment` | the comment, UTF-8, lines ended by `\n` | 64 KB (the index keeps the first 4 KB) |

**Why three, not one.** `glint.tags` is in people's files and in installed services (M8). Text with no format needs no parser in a script (M2). Each mark is written alone, so a label by key never rewrites a comment.

**Why the name, not a number.** XYplorer stores a label's number, so renaming a label, or opening the files on another PC, changes what they say. A name means the same everywhere; a PC that has no label by that name shows it with a color worked out from the name (`tag_color`, as other tags get).

**What it costs.** A file's record in the MFT is 1 KB; a label, a few tags and a short comment fit in what is free of it, take no cluster, and are read by the index in the pass that reads the names. Measured at S0 (section 14, item 6): beside a label and two tags, a comment stays in the record up to about 500 bytes when the file's name is short, and only about 40 when it is 200 characters long. A longer comment leaves the record; the index then reads its first 4 KB through the file system (section 6.4). Reading for rows is one open of the file for its attributes and `GetFileInformationByHandleEx(FileStreamInfo)`, then one open for each `glint.*` stream it lists: a file with no marks costs that one call. S0 measured it against `FindFirstStreamW`, today's one open per file and three opens (section 14, item 5): 15.5 µs a file with no marks, against 18.0, 10.7 and 37.4.

### 5.2 Beside the file: `.glint-marks`

Hidden, in the folder, JSON Lines (UTF-8, `\n`), sorted by name so it reads well in a diff:

```
{"glint-marks":1}
{"name":"bond0.conf","label":"Important","tags":["bond","netplan"],"comment":"mode 4 (802.3ad)\nLACP rate fast","at":1795436917}
```

- Written whole, to a temporary file that takes its place (core's `replace_contents`, `glint-core/src/lib.rs:132`; tried on exFAT in section 11). Keys it does not know are kept (a later version's).
- A line whose name is no longer in the folder goes at the next write. A file with no lines left is removed.
- Read wherever it is found, on NTFS too (F23); a file's own stream wins over its line.
- Read at most 4 MB and 100,000 lines; a name with `\`, `/` or `..` is refused (M10).

### 5.3 On this PC: the ledger

`%APPDATA%\com.shado.shadui-ripgrep\marks.jsonl`: the same lines with `"path"` in place of `"name"`, and `"why"`: `"away"` (an import waiting for a file that was not there) or `"shut"` (it cannot be written where it is). Held in memory by path in lower case; written the way the sidecar is, a moment after it changes.

### 5.4 Which home

`marks::home_of(path)`, the first that holds:

1. **The stream**, when the volume says it has named streams (`GetVolumeInformationW`'s `FILE_NAMED_STREAMS`, asked once per volume: closes G7) and the write succeeds.
2. **Beside**, when the item's folder is a real folder that can be written.
3. **The ledger.**

Reading takes the first home that has anything for the item: stream, then beside, then ledger. Writing to a home takes the item out of the homes after it, so a mark is in one place.

### 5.5 The labels

`Settings.labels: Vec<Label { name, color, was }>` in `state.json`, starting as `COLOR_TAGS`. Renaming a label keeps its old names in `was`: a file that still says *Red* is drawn as *Important*, `label:important` is asked as `label:important;red`, and the file gets the new name the next time its marks are written. Nothing is rewritten in bulk.

### 5.6 Today's color tags become labels

A file tagged *Red* in 0.16 has no `glint.label`. From 0.17:

- **Drawn**: a file without a label stream whose tags name a label is shown with the first such tag as its label, and that tag is not drawn again among its tags.
- **Moved**: on the drives instant search covers, once after the update and on the quiet lane, each such file's first color tag moves from `glint.tags` to `glint.label` (dates kept; the list comes from `tag:red;orange;yellow;green;blue;purple;gray`). Elsewhere it moves the next time the file's marks are written.
- **Found**: `tag:red` matches a tag or the label (F12), so nothing saved or learned stops working.

## 6. How it is built

```
glint-model/src/marks/     pure: Marks, the labels, the lines of 5.2 and 5.3 (read and write), XYplorer's
                           format (read and write), what an import does with what is there, the filter
glint-core/src/marks/      I/O: streams, the three homes, carrying across copies, import, export, finding XYplorer
glint-core/src/tags.rs     stays: the tags stream's reader and writer, called from marks::
shadui-index/src/          record.rs and tree.rs (three streams), far.rs (a stream that left the record),
                           query.rs (label:, comment:, has:, !), proto.rs (Marks, MarksIn, a search that
                           looks in marks)
glint-ui/src/dialogs/      tags.rs (the hub stays here: one Backend, one event), comment.rs, marks_import.rs
glint-ui/src/components/   row_facts.rs, file_list/, file_grid/, preview/marks.rs, menus/, sidebar/,
                           pane_head/, settings/labels.rs
```

### 6.1 Reading

`FileList::ask_facts` asks `RowFacts` for the rows on screen and half a screen either side, as now; the batch (200) is answered by `marks_read` instead of `tags_read`, so `tags_of` keeps working and `marks_of` is new. While the list is sorted or grouped by a mark, or filtered by one as you type, `RowFacts::ask_all` reads every entry of the folder on the now lane (`rayon`), and the rows are built once when it is in: `Reads` gains `marks_of` and `build_rows` passes it (closes G4 for marks; the same door would fix Date taken's sort, which is not this wave). A menu still reads only what `RowFacts` has in memory.

### 6.2 Writing

A view calls `dialogs::tags::set_marks(window, cx, paths, change)`. Core's `marks_set` answers each path's marks before and after, and its home. The hub remembers the befores as one Undo step (`UndoAction::Marks`, run by `marks_put`), says `TagsChanged { paths }` (which now means any mark: `RowFacts` reads them again and the sidebar counts again), and shows the notice. `set_tags` becomes `set_marks` with `Change::Tags`, so tags get Undo too (G5).

### 6.3 Following

- **Nothing to do** for a rename or a move on a drive with streams, or a copy between two such drives: Windows' engine and `CopyFileExW` take streams along (`shell.rs:760`, `queue.rs:769`, `:824`). **A file's.** Neither copies a folder's own streams, so `carry` reads and writes a folder's own marks whatever the drives keep (found in wave 3's review: section 9).
- **`marks::carry(pairs, moved)`** runs where an operation Glint ran is known to be done (`explorer/ops.rs:186`, the copy queue after `queue.rs:503` and `:565`): for each pair whose homes differ, the marks are read at the source and written at the destination; for a move, taken out of the source's sidecar or the ledger. Items in a sidecar or the ledger that Glint renames or deletes have their line renamed or dropped the same way.
- **A file made in another's place**: `actions::run` with `Output::Replace` and `convert::convert` read the original's marks before it is recycled and write them to the file that takes its place (G6).

### 6.4 The index

- **`record.rs`** parses `glint.label` and `glint.comment` beside `glint.tags`. A `glint.*` stream that is not resident sets `far`.
- **`far.rs`**: a file marked `far` is opened by its path, with backup rights, and the first 4 KB of each `glint.*` stream are read. Through the file system and not from the volume's clusters, because a stream just written may still be in Windows' cache. This closes G3. It is the first time the service opens a file rather than a volume: only these three streams, never the file's own data, and [instant-search.md](../../instant-search.md)'s security model says so. A record whose attribute list is not resident is `far` too: read from the volume's clusters it can be older than what Windows has in its cache, which is most likely how S0 saw a tag go missing when its record filled up (section 14, item 6).
- **`tree.rs`**: `tags: HashMap<u32, Box<str>>` becomes `marks: HashMap<u32, Marked>`. Only marked records take room. The snapshot's side file is `.marks`; a `.tags` from 0.16 is read when there is no `.marks` yet.
- **`query.rs`**: the keys of F12. `comment:` and the plain words of F15 look only at marked records, so they cost a walk of that map.
- **`proto.rs`**: `Request::Marks` (labels and tags with their counts, how many comments), `Request::MarksIn { path }` (a folder's marked children: name, label, tags, whether there is a comment), `SearchRequest.marks` (plain words look in tags and comments too), and two bits of a hit's flags that today only carry *folder*: found by its comment, found by a tag. `decode_hits` reads bit 0 alone (`proto.rs:337`), so an older app is unharmed. `Request::Tags` stays for older apps.
- **The service never sends a comment's text.** It says which files match; the app reads the comment from the file, with the person's own rights. It does hold the first 4 KB of every account's comments, and whoever may ask it can learn which files have a word in theirs: the security model gains that sentence.
- **Versions**: filters need the service to be the app's version (`find::same_service`), as now; until the service has updated itself, `label:` and `comment:` are answered the way `tag:` is on fd's drives.

### 6.5 The filter, written twice on purpose

`shadui-index` does not depend on `glint-model` (a SYSTEM service keeps few dependencies), and `glint-model` filters the open folder with its own parser (`entries.rs:844`; `terms` is already a copy). So the marks filter exists in both: `shadui_index::query::MarksFilter` and `glint_model::marks::Wanted`. One table of cases, `tests/fixtures/marks-filter.txt` (`query | label | tags | comment | yes or no`), is run by both crates' tests, so they cannot drift. The four lists of filter keys the interface keeps by hand (`explorer/inside.rs:70`, `address_bar/search.rs:61`, `sidebar/rows.rs:365`, `quick_search/search.rs:59`) read one list, `glint_model::marks::KEYS`, for the new keys.

On fd's drives the test moves from `Out::push` (one thread, `find.rs:228`) to the four threads that read media filters (`find.rs:422`): `marks::matches(path, &wanted)`. The ledger's matches under the folders searched are added by `find_files` itself.

### 6.6 What it may cost (S0 measured the first column; section 11 holds the second)

| | Today | Wanted |
| :--- | :--- | :--- |
| Listing a folder of 100,000 | 24 ms: Windows' enumeration 19–20 ms, the sort 5–7 ms | the same |
| Marks of 200 rows, none marked | tags: 2.1 ms (10.7 µs a file: one open, which fails) | within a millisecond of it |
| Every mark in a folder of 10,000 (a sort) | not done | under 60 ms on every core (see below) |
| The index: C: read from scratch | 2.3–2.5 s | the same with no marks; measured with 10,000 marked files |
| `tag:work` inside the service | 8.0 ms (best 7.3), C: and D: (1.4 million items, none tagged); a name, `report`, 8.2 ms | the same; `label:` and `comment:` like it |
| The right-click menu for one item | under 1 ms | the same |
| A `tag.dat` of 10,000 lines: read, find the files, mark them | not done | read under 50 ms; the rest in the background, said as it goes |

How S0 measured (2026-11-23, this PC: 16 threads, NVMe SSD, Windows 11, release builds, files in Windows' cache, nothing else running): listing with `Glint.exe bench <folder> --runs 9` on 100,000 files of 64 bytes, twice (medians 24.2 and 24.4 ms); the marks of rows with a harness that copies `tags.rs`'s reads exactly and times four ways in turns, 11 runs after a warm-up (section 14, item 5); the service with its own `search` command, 11 fresh runs, the time it reports for itself. The harness and its notes are in the coordinator's notes folder (`marks-s0-harness/`).

**The third row can't come from the files.** Reading every mark of 10,000 files takes 95 ms on 16 threads when none is marked (182 ms on one), and 0.66 s when every file has all three. Under 60 ms needs the index: on a drive instant search covers, a sort or a group takes the folder's marks from `Request::MarksIn` (from memory) and reads from the files only what a row shows that the index doesn't send (a comment's first line). C2 makes `MarksIn` in the same wave as B2's `marks_read`, so the coordinator wires that path after wave 2; on other drives the sort costs what the files cost. The second row is met, just: the read of 5.1 takes 3.1 ms for those 200 rows, 0.96 ms more than today's.

## 7. Bringing XYplorer along

### 7.1 What XYplorer keeps

From its manual as quoted in Everything's forum, its FAQ and its own forum (sources at the end). What is marked *to verify* is checked against a real file before S0 (section 14).

- **One file, `tag.dat`**, in its data folder: `%APPDATA%\XYplorer` by default; `Data` beside `XYplorer.exe` for a portable copy with no `Startup.ini`; else where `Startup.ini` (beside the program) says, `[Appdata] Path=…`.
- **UTF-16LE text.** Version 5 (XYplorer 19.70 and later):

```
XYplorer File Tags v5

Labels:
Red|000000|FC7268;Orange|000000|F6AB46;Yellow|000000|EFDC4A;Green|000000|B5D74A;Blue|000000|5DA4FE;Purple|000000|E29CDC;Grey|000000|B5B5B5

Extra Tags:
Caption|TypeID|0|0|||||          (16 lines)

Storage: 0 (Absolute paths)

Data:
C:\ops\bond0.conf|1|bond, netplan|||||||||||||||||mode 4¶LACP rate fast
```

- **A line of data** is `Full filename|LabelID|Tags|Extra1…Extra16|Comment`. `LabelID` is 0 for none, else the label's place in the list. Tags have `, ` between them (`,` alone is seen too). A line break in a comment is `¶`. Only the comment, the last field, may hold `|`.
- **Version 3** (older): five extra fields instead of sixteen, 15 labels at most, the comment still last.
- **Extra fields** have a caption and a type: 0 text, 1 number, 2 date, 3 checkbox (empty is off, anything else on), 4 rating (1 to 5), 5 list, 6 location, 7 icon, 8 image, 9 script.
- **Storage** says how paths are written: 0 absolute; 1 `?:\dir\file` for files on XYplorer's own drive; 2 relative to XYplorer's folder (`..\..\dir\file`); 3 relative to the folder `tag.dat` is in; 4 the volume's serial for the drive letter (`B2D60DFE:\dir\file`).
- **`XYplorerTag.dat`** is the same format for one folder and what is below it, made by XYplorer's `tagexport` and merged by `tagload("XYplorerTag.dat", 1);`.
- **Its own behaviour**: it reads `tag.dat` again when the file's date changes, looks paths up by exact case, and calls a line whose file is gone an orphan.
- **Its search** names the fields `lbl:`, `tags:` and `cmt:`.

### 7.2 What the person sees

- **F27. Found by itself.** Glint looks (reading only) in the three places of 7.1. When there is a `tag.dat` that was never brought in, Settings' Overview has a suggestion, *Bring your XYplorer labels, tags and comments · 1,284 items*, and it may come up once as the suggestions do (`settings/overview.rs:258`). **Import marks…** takes any `tag.dat` or `XYplorerTag.dat`.
- **F28. The preview says what would happen**, before anything is written:
  - *tag.dat, version 5, changed today 14:02 · 1,284 items · 7 labels · 93 tags · 412 comments · 3 extra columns*
  - **Labels**, each with the Glint label it becomes: by name (*Grey* is *Gray*), else *New label* in XYplorer's color; any of them can be pointed at another.
  - **Extra columns**, each with what it becomes (7.4), and a switch to leave them out.
  - **Where the items are**: *here* (marked now); *moved since* (found where they are now); *on a drive that is not connected*, by drive (they wait); *not found* (listed; they wait, or are left out); *already marked in Glint* (*Fill what is empty*, or *XYplorer's replace Glint's*).
- **F29. Bring them in.** On background threads, with how far in the dock. Then: *Marked 1,190 items from XYplorer · 61 waiting · 33 not found*, with **Undo** and **What happened** (every item with its outcome, to copy). **Settings → Files → Marks** has *Take back the last import* for later.
- **F30. Keep following XYplorer** (a switch in the preview and in Settings, off until chosen). Each time Glint starts or comes to the front, if `tag.dat`'s date changed, what is new or changed in it comes in; a line gone from XYplorer takes its marks off only where they are still what XYplorer had. One quiet line says so: *3 marks came from XYplorer*.
- **F31. Waiting marks wake.** An item that was away gets its marks the first time Glint lists it: they move from the ledger into the file.
- **F32. The same words.** The columns are named Label, Tags and Comment; `lbl:`, `tags:` and `cmt:` work; the seven labels have XYplorer's names.
- **F33. A way back.** Export marks… → *XYplorer* writes an `XYplorerTag.dat` (version 5, Storage 3) that `tagload` merges.

### 7.3 How

One pipeline for every source (XYplorer's files and Glint's own export):

```
read the file → Incoming { where, label, tags, comment, extras }
             → find each item → Planned { path, what it gets, outcome }      (the preview shows this)
             → apply in batches (marks_set), befores into a journal            (Undo reads this)
             → remember what was applied, item by item                        (following compares with this)
```

- **Reading** (`glint-model/src/marks/xyplorer.rs`, pure). From bytes: a byte-order mark or none, `\r\n` or `\n`. Sections are found by their names, not their line numbers; the number of extra fields is the number of definition lines; a data line is split on `|` at most that many times plus three, so the comment keeps its own. A version above 5 is read by the same rules and the preview says so. Capped at 64 MB.
- **Finding the items** (core). By the storage line:
  - 0: as written.
  - 4: the drive whose serial it is (`permalinks`' `find_volume_root_by_serial`).
  - 3: joined to the folder of `tag.dat`.
  - 1 and 2: XYplorer's drive or folder. For a portable copy that is two folders above `tag.dat`; otherwise from Windows' *App Paths* and uninstall entries; failing both, a `?:` path that exists on exactly one drive is taken, and the rest are asked about once in the preview.
  - Each path is then looked for with one call, any case. A server that does not answer is given two seconds once, not per item.
- **Moved since.** For what is not where XYplorer left it, on drives instant search covers, the service's history is asked (`client::history`, which keeps 30 days): an event whose `path` is the old path gives `now`, where the file is today (`proto.rs:93`). XYplorer cannot do this; it is where its orphans come back.
- **What an item gets.** Labels by name, any case, with `was` and *grey* for *gray*; an unknown one is a new label until there are 32, then a tag. Tags trimmed, one of each in any case. `¶` becomes a line break. Extras by 7.4.
- **Meeting Glint's own marks.** *Fill what is empty* (the default): the label if it has none, the tags added, the comment if it has none; a different comment keeps Glint's and the report says so. *XYplorer's replace Glint's*: all three.
- **Applying.** In batches of 200 at background priority. The befores go to `imports\<time>.jsonl` in Glint's data folder (kept 30 days) and to one Undo step. An item whose line has not changed since it was applied is not applied again, so running an import twice does nothing.
- **Following.** `xyplorer.json` holds the source's path, date and size and a hash per item of what was applied. On start and on coming to the front: one `stat`; if it changed, read (milliseconds) and apply the difference.

### 7.4 Extra columns

Glint has no custom columns (section 12), so nothing is dropped silently:

| XYplorer's type | Becomes |
| :--- | :--- |
| checkbox, on | the tag `Caption` |
| rating | the tag `Caption: 4` |
| list | the tag `Caption: value` |
| text, number, date, location, image, script | a line `Caption: value` at the end of the comment, after an empty line |
| icon | left out, and the preview says so |

### 7.5 Not done: writing `tag.dat` while XYplorer runs

XYplorer holds its database in memory and writes the file itself; a second writer would lose one side's changes. Following is one way (M7), and going back is an export the person runs.

## 8. Contracts

The coordinator lands these as code before the first card (S0): every type, signature and module line, with bodies that compile and do nothing (never `todo!()`). If the tree and this section disagree, the tree wins.

```rust
// glint-model/src/marks/mod.rs
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marks { pub label: Option<String>, pub tags: Vec<String>, pub comment: Option<String> }

pub enum Change {
    Label(Option<String>),
    Tags { add: Vec<String>, remove: Vec<String> },
    Comment(Option<String>),
    Put(Marks),          // paste, undo, import: these and nothing else
    Clear,
}
pub enum Home { Stream, Beside, Ledger }
pub struct Outcome { pub path: String, pub before: Marks, pub after: Result<(Marks, Home), String> }

pub struct Label { pub name: String, pub color: u32, #[serde(default)] pub was: Vec<String> }
pub fn default_labels() -> Vec<Label>;                                  // today's COLOR_TAGS
pub fn label_of<'a>(labels: &'a [Label], name: &str) -> Option<(usize, &'a Label)>;
pub fn shown(labels: &[Label], marks: &Marks) -> Shown;                 // 5.6: the label drawn, the tags drawn

pub const KEYS: [&str; 7] = ["label", "lbl", "tag", "tags", "comment", "cmt", "has"];
pub struct Wanted { /* groups of labels, groups of tags, words, has, and the same again for "not" */ }
impl Wanted { pub fn parse(terms: &[String]) -> (Wanted, Vec<String>); pub fn accepts(&self, marks: &Marks) -> bool; }

// lines.rs: 5.2, 5.3 and the export          xyplorer.rs: 7.1, read and write
pub fn read_beside(text: &str) -> Beside;     pub fn read(bytes: &[u8]) -> Result<XyFile, String>;
pub fn write_beside(b: &Beside) -> String;    pub fn write(items: &[XyItem], labels: &[Label]) -> Vec<u8>;
```

```rust
// glint-core/src/marks/mod.rs
pub async fn marks_read(paths: Vec<String>) -> Result<Vec<Marks>, String>;
pub async fn marks_set(paths: Vec<String>, change: Change) -> Result<Vec<Outcome>, String>;
pub async fn marks_put(items: Vec<(String, Marks)>) -> Result<Vec<Outcome>, String>;
pub fn read(path: &str) -> Marks;                              // stream, else beside, else ledger
pub fn matches(path: &str, wanted: &Wanted) -> bool;           // fd's drives
pub fn carry(pairs: &[(String, String)], moved: bool);
pub async fn marks_all() -> Result<Counts, String>;            // the index's, with the ledger's
pub async fn marks_export(root: String, to: String, format: Format) -> Result<u64, String>;
pub async fn import_plan(source: String, options: ImportOptions) -> Result<ImportPlan, String>;
pub async fn import_apply(plan: ImportPlan, progress: Channel<Progress>) -> Result<ImportReport, String>;
pub fn xyplorer_found() -> Option<Found>;                      // where tag.dat is, how many items, when it changed
```

```rust
// shadui-index
pub struct Marked { pub label: Box<str>, pub tags: Box<str>, pub comment: Box<str> }     // empty: none
Request::Marks                 → Message::Marks { labels: Vec<(String, u64)>, tags: Vec<(String, u64)>, comments: u64 }
Request::MarksIn { path }      → Message::MarksIn { items: Vec<MarkedChild> }            // no comment text
SearchRequest { .., #[serde(default)] marks: bool }
pub const HIT_BY_COMMENT: u8 = 2;   pub const HIT_BY_TAG: u8 = 4;                        // beside bit 0, "a folder"
```

```rust
// glint-ui
// dialogs/tags.rs: the Backend gains  read_marks(paths) -> Fut<Vec<Marks>>,  set_marks(paths, change) -> Fut<Vec<Outcome>>
pub fn set_marks(window: &mut Window, cx: &mut App, paths: Vec<String>, change: Change);
// row_facts.rs
pub fn marks_of(&self, folder: &str, path: &str) -> Option<&Marks>;
pub fn ask_all(&mut self, folder: &str, entries: &[FileEntry], cx: &mut Context<Self>);
// explorer/ops_types.rs
UndoAction::Marks(Vec<(String, Marks)>)        UndoRun::Marks(Vec<(String, Marks)>)
// glint-model
ColumnId::{Label, Tags, Comment}    SortKey::{Label, Tags, Comment}    GroupBy::Label    Part::Labels
// menus
MenuEvent::SetLabel { paths, label: Option<String> }    "do:label"
MenuEvent::EditComment { paths }                        "do:comment"
MenuEvent::CopyMarks { path }                           "do:copy-marks"
MenuEvent::PasteMarks { paths }                         "do:paste-marks"
MenuEvent::ClearMarks { paths }                         "do:clear-marks"
Node::Swatches(Vec<Entry>)                              // the row of labels' dots
```

`MenuEvent::SetTags` and `EditTags` keep their keys (`do:tags`, `do:edit-tags`): a key is never renamed.

### 8.1 What S0 put in the tree, where it differs from the sketch above

The tree wins; this is what a card finds there.

- **Files.** A function's body is in the file of the card that fills it, and `mod.rs` re-exports it. glint-model: `marks/mod.rs` and `labels.rs` (A1), `lines.rs` (A2), `xyplorer.rs` (A3), `wanted.rs` (A5, which holds `KEYS` and `Wanted`). glint-core: `marks/mod.rs`, `beside.rs`, `ledger.rs` (B2; the last two are empty modules), `streams.rs` and `caps.rs` (B1), `carry.rs` (B3), `import.rs` and `xy_find.rs` (F1), `export.rs` (F3).
- **glint-model adds** the stream names, their caps, `BESIDE_NAME` and `MAX_LABELS`; `Marks::is_empty`; `Change::apply` (what a change makes of an item's marks, which `marks_set` needs; A1); `Meet { Fill, Replace }` and `meet(has, incoming, how) -> (Marks, bool)` (7.3, the bool saying a different comment was kept; A1); `Shown { label, place, tags }` (`place` is the label's index when this PC has it); `Wanted { yes: Terms, no: Terms }` with `Terms { labels, tags, words, has }`, `Has { Label, Tag, Comment, Mark }` and `Wanted::is_empty`; `Line { name, marks, at, why, rest }`, `Why { Away, Shut }`, `Beside { version, lines, refused }`, `Export { labels, lines }`, `read_ledger`, `write_ledger`, `read_export`, `write_export`, `BESIDE_MAX_BYTES`, `BESIDE_MAX_LINES`; `XyFile { version, labels, extras, storage, items }`, `XyLabel { name, text, back }`, `XyExtra { caption, kind }`, `XyKind`, `XyItem { path, label, tags, extras, comment }`, `XY_MAX_BYTES`, and `xyplorer::marks_of(file, item, extras) -> Marks` (an item's marks with 7.4's extras; A3).
- **`default_labels()` holds the seven colors itself**: glint-model can't read glint-ui's `COLOR_TAGS`. A glint-ui test keeps the two one list.
- **Settings**: `labels` and `follow_xyplorer` are written like every other setting, so the default-JSON test gained them: the settings file keeps only the keys the defaults have (`persistence::known_settings`), and a setting left out of them would be dropped on import. `Label.was` is not written while it is empty. The Settings part of a settings file therefore carries the labels; G1 decides whether `Part::Labels` takes them out of it.
- **glint-core adds** `streams::read(path) -> Option<Marks>`, `streams::write(path, &Marks)`, `caps::has_streams(path) -> bool`; `Counts { labels, tags, comments }`; `Format { Glint, Xyplorer }`; `ImportOptions { meet, extras, labels, base }`, `ImportPlan { source, version, changed, options, labels, extras, items }`, `Planned { path, at, gets, has }`, `At { Here, Moved { now }, Away, Missing }`, `LabelPlan { name, color, to, new }`, `Progress { done, total }`, `ImportReport { marked, waiting, missing, kept_comments, befores, items, journal }`, `Found { path, items, changed }`.
- **shadui-index**: `Message::Marks(MarkCounts)`, a struct for the counts (clippy refuses the tuple), the same JSON on the wire as the struct variant above; `MarkedChild { name, label, tags, comment }`. `Engine::marks` and `Engine::marks_in` answer empty and `pipe.rs` already sends what they answer, so C2 fills the engine and nothing else. `SearchRequest.marks` is left out of the JSON while false, as `fuzzy` and `rank` are; every request Glint builds says `marks: false`, and the command line's `search` takes `--marks`. `Marked` is in `tree.rs`.
- **glint-ui**: the `Backend`'s two calls have bodies that answer nothing, so the tests' fakes compile unchanged, and `CoreBackend` calls core; `set_marks` does nothing yet. `UndoAction::Marks` runs as `UndoRun::Marks`, which does nothing until D1 calls `marks_put`. The keys are actions `LabelSelection { index }` (Alt+1…9, one row of the shortcut sheet), `ClearLabel`, `EditTags` and `EditComment`; nothing handles them yet. The five menu events are matched and do nothing in `workspace.rs`; `Node::Swatches` draws nothing and `smart::walk` skips it (E2 decides whether Suggested may copy a dot).
- **Columns**: the `ColumnId`, `SortKey` and `GroupBy` variants have their `COLUMNS` rows, `Column::{Label, Tags, Comment}` its widths (7, 9 and 14 rem), and `columns::MARK_SORTS` lists the three sorts. `COLUMN_ORDER` and `GROUP_BYS` don't list them, so nothing can show or choose them until E1 adds them; until then the cells draw nothing, the sort keeps the name's order and `GroupBy::Label` is one group, as `None` is. `persistence::Part::Labels` exists but is not in `ALL_PARTS` or `PARTS` until G1 writes and reads it.
- **Landed before wave 2's cards** (signatures two cards or two crates share, each compiling and doing nothing until its card fills it): glint-model's `entries::MarksOf` (an entry's marks as read, `None` while unread), `entries::MarkReads { of, labels }` and the field `Reads::marks: Option<MarkReads>`, which `entry_order` and glint-ui's `explorer/rows.rs` fill with `None` (A5 reads it; the coordinator hands the rows' marks in afterwards); shadui-index's `far.rs`, a public module with `far::read(path) -> Option<Marked>`, `far::drain(tree: &RwLock<Tree>, letter, stop) -> usize` and `far::BATCH` (C3 fills them, C2's thread calls `drain`); `client::marks() -> Result<MarkCounts, Error>` and `client::marks_in(path) -> Result<Vec<MarkedChild>, Error>`, which answer `Err` until C3 (B2's `marks_all` calls the first).
- **Landed before wave 3's cards**: glint-ui's `theme::label_tone(color)` and `theme::label_wash(color, cx)` (the one place a label's strength in a row is set); glint-model's `COLUMN_ORDER` with the marks' three columns and `GROUP_BYS` with Label, so both can be chosen; `marks::clip_text(name, marks)` and `marks::clip_marks(text)` (one item's marks as the sidecar's line, for Copy marks and Paste marks); glint-ui's `marking.rs` (eight functions the workspace calls for the four key actions and the menus' five events, filled by E5), `dialogs/comment.rs` with `open(window, cx, paths)` (E3), the module `preview/marks.rs` (E4), `explorer::RowFileMarks` with the field `RowSpec::marked`, and `explorer::marked::init`, which start-up calls (E1a).
- **Added by wave 3's review**: `marks::carry::carry_files` (pairs known to be files: the queue's), `Change::Paste`, `marks::comment_brief`, the queue's `Copies` (what a job copied, carried 512 at a time and when it ends), `marking::take_label_off` as `pub(crate)`; `queue::plan` gives each folder with the one it is a copy of; glint-ui's `file_list::layout::leave_room` with `NAME_ROOM`.
- **Engine helpers**: `xray::list_streams` is `pub(crate)`; `permalinks::win` is `pub(crate)`, so `find_volume_root_by_serial` is `crate::permalinks::win::find_volume_root_by_serial` (Windows only).

## 9. The waves

Cards in a wave run at the same time; a wave starts when the one before it is integrated. Rules for agents are the Aware plan's section 6.

| Wave | Card | What | Crate | Owns |
| :--- | :--- | :--- | :--- | :--- |
| S0 | — | **Coordinator**: section 8 as code; the settings (`labels`, `follow_xyplorer`); the keys of F6 in `actions::BINDINGS`; the new menu events and their keys; the column, sort and group variants with their table rows; `xray::list_streams` and `permalinks`' `find_volume_root_by_serial` made `pub(crate)`; the fixtures of section 11; **the measurements of 6.6's first column**; the answers to section 14. *Landed 2026-11-23 (`13660c7`): 8.1, 6.6, 14.* | all | — |
| 1 | [A1](cards/A1-marks.md) | Marks, the labels, what is drawn for old color tags, what an import does with what is there | model | `marks/mod.rs`, `marks/labels.rs` |
| 1 | [A2](cards/A2-lines.md) | The lines: beside, the ledger, the export | model | `marks/lines.rs` |
| 1 | [A3](cards/A3-xyplorer.md) | XYplorer's format, read and written | model | `marks/xyplorer.rs` |
| 1 | [A4](cards/A4-filter.md) | The filter in the service: `label:`, `comment:`, `has:`, `!`, the nearest names | index | `query.rs` |
| 1 | [B1](cards/B1-streams.md) | Streams: one listing per file, writing with the dates kept; what a volume can do | core | `marks/streams.rs`, `marks/caps.rs` |
| 1 | [C1](cards/C1-index.md) | The index reads three streams; `Marked`; the `.marks` side file | index | `record.rs`, `tree.rs`, marked places in `volume.rs`, `refs.rs` and `engine.rs` |
| 2 | [A5](cards/A5-open-folder.md) | The filter in the open folder; sort and group by a mark; plain words | model | `entries.rs`, `groups.rs`, `plain.rs`, `marks/wanted.rs` |
| 2 | [B2](cards/B2-homes.md) | The three homes; `marks_read`, `marks_set`, `marks_put`, `read`, `matches` | core | `marks/mod.rs`, `marks/beside.rs`, `marks/ledger.rs`, `tags_read` and `tags_set` in `tags.rs` |
| 2 | [C2](cards/C2-engine.md) | The engine: `Marks`, `MarksIn`; a search that looks in marks; the far records' thread | index | `engine.rs`, `proto.rs`, one new method in `tree.rs` |
| 2 | [C3](cards/C3-far.md) | Streams that left the record, read through the file system; the app's two calls (C2's other half: it was too big for one run) | index | `far.rs`, `client.rs` |
| 2 | [D1](cards/D1-hub.md) | The hub takes marks; `RowFacts` keeps them; Undo | ui | `dialogs/tags.rs` (the hub's part), `row_facts.rs`, `explorer/ops_types.rs`, `explorer/undo.rs` |
| 3 | [B3](cards/B3-carry.md) | Carrying: copies, moves, files made in another's place; fd's drives; the ledger in results | core | `marks/carry.rs`, `find.rs`, marked places in `queue.rs`, `actions.rs`, `convert.rs` |
| 3 | [C4](cards/C4-far-by-reference.md) | The far reads open an item by its file reference, never by its path; then the thread is turned on (see "Wave 2, as it landed") | index | `far.rs`, and `FAR_READS` with `far_round` in `engine.rs` |
| 3 | [E1a](cards/E1a-rows.md) | The rows' marks reach the rows: filter, sort and group by a mark (E1 was three runs' work) | ui | `explorer/rows.rs`, `explorer/marked.rs`, marked places in `explorer/model.rs` |
| 3 | [E1b](cards/E1b-list.md) | The list: the wash, the note mark, three columns | ui | `file_list/{cells,layout,header}.rs` |
| 3 | [E1c](cards/E1c-grid.md) | The grid: a label's pill under a tile's name, the note mark | ui | `file_grid/tile.rs` |
| 3 | [E2a](cards/E2a-menu.md) | The menu's row of dots, Marks under More | ui | `menus/{item,spec,view,smart,data}.rs` |
| 3 | [E2b](cards/E2b-palette-and-words.md) | The commands in `Ctrl+K`; plain words that know the labels; the lists of filter keys | ui | `palette/commands.rs`, `address_bar/{find,search}.rs`, `quick_search/mod.rs`, the key lists in `explorer/inside.rs` and `sidebar/rows.rs` |
| 3 | [E5](cards/E5-marking.md) | Marking: what the keys and the menus' events do (the table left the handlers to nobody) | ui | `marking.rs` |
| 3 | [E3](cards/E3-dialogs.md) | Edit tags… suggests; Edit comment… | ui | `dialogs/tags.rs` (the dialog's part), `dialogs/comment.rs` |
| 3 | [E4](cards/E4-preview.md) | The Marks block in the preview pane and Quick Look | ui | `preview/marks.rs`, one marked place in `preview/frame.rs` and one in `preview/pane.rs` |
| 4 | F1 | Import: finding XYplorer, finding the items, moved since, the plan, applying, the journal, following | core | `marks/import.rs`, `marks/xy_find.rs` |
| 4 | F2 | The import's preview and report; the suggestion; following when Glint comes to the front | ui | `dialogs/marks_import.rs`, marked places in `settings/overview.rs` and `startup.rs` |
| 4 | F3 | Export, both formats | core, ui | `marks/export.rs`, `dialogs/marks_export.rs` |
| 4 | G1 | The sidebar's Labels and the tags' menu; Settings' Labels; the settings file's part | ui | `sidebar/{mod,rows}.rs`, `settings/labels.rs`, `settings/transfer.rs` |
| 4 | G2 | The head's label chips; **In comments** in a search's results | ui | `pane_head/mod.rs`, `explorer/find.rs`, the results' grouping |
| 5 | H1 | AI agents: `get_marks`, `set_marks`, the help's words | core | `mcp.rs` |
| 5 | H2 | Ties, each one cut freely: `{label}` and `{tags}` in Batch rename; a folder rule that labels (`RuleAction::Label`, `store.rs:621`); zips Glint makes carry a `.glint-marks` in each folder | model, core | named in the card |
| 6 | — | **Coordinator**: the color tags' move (5.6); every wiring call; the look in the real app in both themes; speed; e2e; docs (section 11) | all | — |

Why this order: wave 1 has no card that reads another's code. B2 needs A1, A2 and B1; C2 needs C1 and A4; D1 needs only section 8. Wave 3 is the interface on D1, and carrying on B2. Wave 4 is XYplorer and what stands on a working feature. Wave 5 can be left for 0.17.1 without anyone missing a promise of the issue.

**Wave 1, as it landed** (2026-11-23: six agents on Sonnet, one a card, run from another session; [its report](WAVE-1-REPORT.md), and [what that session was asked](RUN-WAVE-1.md)). Every card came back done, in twelve files, each a card's own. The cards were written so that none of the six read another's code, which left two things to do afterwards:

- **The search asks the new filter.** `Tree::search` asks `filters.marks.accepts` of each record's `Marked` (what the filter says of a record with no marks is worked out once a search), so `label:`, `comment:`, `has:` and `!` narrow what the index answers, and a label answers to `tag:`. `Filters::tags` and `Tree::tagged` stay until glint-core's `find.rs` reads `marks` too (B3).
- **`keep_times` is one function**: `tags::keep_times`, `pub(crate)`, which `marks::streams` writes under.

What the later cards should know:

- **Tidying is A1's alone** (`marks::tidy`). The readers of A2, A3 and B1 return what a file says; whoever keeps marks calls it (B2's `marks_set` and `marks_put`, F1's import). `marks::settled` is what 5.6's move and B2's writes use; it gives a label its new name only through `was`, and never changes a name's case.
- **Made, and not called by anything yet**: `MarksFilter::nearest` (the engine asks it in C2); `Tree::far`, which fills and is saved in `.marks` but is read by C2's `far.rs`; `streams::read`, `streams::write` and `caps::has_streams` (B2).
- **Decided by the cards' agents, and kept**: in a `.glint-marks`, a later line with no marks for the same item drops the item; a `.marks` file that is damaged gives no marks, and the old `.tags` is not tried then; `streams::write` takes a label or a comment of only white space as none; XYplorer's checkbox is on for any value, `0` too (to check against a real `tag.dat`: 14.1); `label:` splits on `;` alone, `tag:` on `,` too (the fixture has both).
- **New names the cards added**: `marks::tidy`, `marks::settled`, `lines::LEDGER_MAX_BYTES`, the ledger's and the export's first lines (`{"glint-ledger":1}`, `{"glint-export":1,"labels":[…]}`), `query::MarksFilter` with `nearest` and `without_marks`, `record::COMMENT_KEPT`, `Tree::far` with `set_marks`, and the `.marks` file's layout (C1's card).

**Wave 2, as it landed** (2026-11-23: the other session wrote the five cards and ran them, one Sonnet agent a card; [its report](WAVE-2-REPORT.md), and [what it was asked](RUN-WAVE-2.md)). Every card came back done. C2 was too big for one run and became C2 (the engine) and C3 (the far reads and the app's two calls). The signatures the cards share were landed first (§8.1).

What the review changed:

- **The far reads are off** (`FAR_READS` in `engine.rs`). `far::read` opens an item by its path, and the service runs as LocalSystem: an account that can write a folder can swap part of a path for a link while the service looks, and so send it to open somewhere else, another computer included. Card C4 makes the read open the item by its file reference on the volume (`OpenFileById`: no path is walked); list its streams with `FileStreamInfo` on a handle that asks for attributes alone, so nothing is held open for a file with no marks, which is nearly every far record; and only for an item that has a `glint.*` stream read them through `BackupRead`. Until then a far record keeps what its record shows, and G3 stays open.
- **A test that began to fail now and then is given time.** `aware::probe`'s tests wait for the quiet lane, whose threads run at background priority; beside the crate's new tests an answer took more than the five seconds they allowed. They wait up to thirty now, and still return the moment the answer is in.

What wave 3's cards must do, since nothing does it yet:

- **E1: the rows' marks reach the model.** `explorer/rows.rs` still passes `Reads { marks: None }` and calls `matcher` and `group_order`. It hands `RowFacts::marks_of` to `Reads::marks`, `matcher_marks` and `group_order_marked`, and asks `ask_all` each frame until `has_all` while `has_marks_filter` holds or the sort or the group is by a mark. Until it does, `tag:` typed over an open folder shows every row.
- **E2: plain words know the labels.** `address_bar/find.rs` and `quick_search/mod.rs` call `plain_query`; `plain_query_with` takes the labels. The four lists of filter keys (§6.5) read `marks::KEYS`.
- **E2, E3 and E4: `set_marks` gets its callers** (the menu's events, the keys, the dialogs, the preview's block). Today only `set_tags` reaches it.
- **B3: fd's drives.** `marks::matches` reads a folder's `.glint-marks` again for every item it is asked about: B3 keeps one for the length of a search. `Filters::tags` and `Tree::tagged` go when `find.rs` reads `marks`.
- **Wave 4 (G1, G2)**: `marks_all` for the sidebar; `client::marks_in` for a sort from the index (§6.6); and `SearchRequest.marks`, which every request Glint builds still leaves false.

Decided by the cards' agents, and kept:

- A change to an item that isn't there goes to the ledger (a line beside it would be dropped as an item that is gone).
- A `.glint-marks`'s first line keeps its version on a rewrite, and not its other keys.
- `tags_read` and `tags_set` go through the homes, so a drive without streams no longer refuses a tag: it keeps it beside the file. `tags::write` is left for its own test.
- A search with `marks` looks for the words in tags and comments as typed, and sets both flags on a hit found in both. `search_until` and `search_marks` make the same checks and are kept alike by hand.
- A read of every row that fails counts as read, with no marks (`RowFacts::has_all`).
- Not proved: that `far::read` leaves a link unfollowed (its test may have returned early). C4 replaces that read.

**Wave 3, as it landed** (2026-11-23 and 24: the other session wrote ten cards and ran them, one Sonnet agent a card; [its report](WAVE-3-REPORT.md), and [what it was asked](RUN-WAVE-3.md)). E1 became three cards and E2 two, and E5 (what the keys and the menus' events do) was added. Seven came back done and three partial, which that session finished. Nobody had seen any of it on a screen; the review looked (below).

What the review changed:

- **The far reads are on, by file reference, and three gaps in them are closed.** What was read is given to a record only while its number is still that file's (a file deleted while it was read, its number taken by another, gave the new file the old one's marks). The drive's root is opened without following a link, and an item with `FILE_FLAG_OPEN_NO_RECALL`, so a cloud folder's file is never fetched for its marks. The thread is not one of the engine's `workers`: another program can keep an open of its own marked file waiting for as long as it likes (an oplock), and `stop` must not wait for that. **Left:** such a program still stalls the far reads (never a search) until the service starts again. An open that comes back at once from an oplocked file ends that (`NtCreateFile` with `FILE_COMPLETE_IF_OPLOCKED`; the crate has the `Wdk` features already): wave 6. `instant-search.md` no longer says this is the one time the service opens anything but a volume: on ReFS it has always walked the folders.
- **A folder's own marks are carried, always.** Windows copies a file's streams and never a folder's (tried: the shell's copy and a plain one both left a labeled folder's copy bare). So `marks::carry` reads and writes a folder's own marks whatever the drives keep; `carry_files` is for pairs known to be files (the queue's: none is looked at to tell). The queue hands `carry` every folder it made (`plan` gives each with the one it is a copy of), and Windows' engine reads a moved folder's marks before it goes, since a move to another drive is a copy and a delete. F18 and 6.3 say so.
- **Windows' engine carries**, in `shell::imp::file_op`, which the explorer, an AI agent's moves and a folder rule's all go through: a copy and a sync as copies; a move, a rename and the Undo of a move as moves. An item sent to the Recycle Bin keeps its line where it was, for when it comes back. This was what the report left "between the cards"; `explorer/ops.rs` needed nothing.
- **The queue carries 512 pairs at a time and when a job ends, however it ends** (`Copies`), not once a file: a file at a time wrote a folder's `.glint-marks` again for every marked file in it. **And a move takes an original's line only once the original is deleted.** The cards' version took it when the copy was made, so a move that was stopped, or a copy that failed its check, left the original without its marks.
- **`caps::has_streams` asks a handle** (`GetVolumeInformationByHandleW` on the item, or on its folder when it isn't there): 0.02 ms where finding the volume's root first took 1.2 ms a call (2,000 calls each, the same answers). Every write of a mark asks it, and every file the queue copies.
- **Paste marks is one change**, `Change::Paste`: one Undo step and one write a file, where there were up to three of each.
- **The Marks block**: a label that is a 0.16 color tag can be taken off from its dot (`marking::take_label_off`, as the menu and the keys do); the pane makes no block for more than 1,000 items, since a block reads the marks of every item it is about; and a change the hub says is matched against the block's paths through a set, not each path against each.
- **A row holds the start of a long comment** (`marks::comment_brief`, one kilobyte): a row is made again on every frame it shows, and a comment can be 64 KB.

**Looked at in the real app** (2026-11-24: a debug copy on a scratch profile, driven by `GLINT_SELFTEST=fly:` and captured with `PrintWindow`, in both themes; not in Arabic, which the interface no longer speaks, so right to left is only tested). What it showed, and what changed for it:

- **You couldn't tell which labeled row was selected.** The wash (18% of the label's color) lay under the selection's tint, which is fainter (11% of the accent in light, 17% in dark). A selected row and the folder dropped on now show their one fill alone; the label stays in the Label column, the tile's pill and the preview. The pointer's fill still lies over a wash.
- **Three marks columns squeezed the name to nothing**: with the date and the size they are 45 rems, and the list's breakpoints never counted widths. `layout::leave_room` takes optional columns out, those a narrow list leaves out first and the last first, while the name would have less than 14 rems (`NAME_ROOM`). The media columns' test now gives its window room for its seven.
- **The preview pane's own preview was a sliver**: the block (about 230 px) and the details took the rest. The two now take at most half the frame and scroll; the block's label dots sit on one line at the pane's usual width.
- **Kept as it is**: the wash's strength (0.18 in both themes). It reads as a label in both, and no longer hides a state. A Red label and a red or pink accent are close in hue: on such a PC a red-labeled row looks like a selected one.
- **Seen working**: the wash; Group by → Label; `tag:work` typed over the folder; the three columns (the pill, the chips and their "+5", a comment's first line, Arabic tags and comments); a 0.16 color tag drawn as its label; the grid's pills, dots and note marks; the menu's row of dots with the label checked; the Marks block in the pane and Quick Look; Edit comment… and Edit tags….
- **Found, and left for wave 6**: Edit tags… still offers the seven color names as tags, 0.16's chips, though colors are labels now (5.6's move); the grid's sort bar offers no mark to sort by; *Kept beside the file* shows only where the marks would be kept beside, not for a line beside a file on NTFS (it says where they go, as `home_of` answers).
- The tour (`selftest.rs`, debug builds) gained `menu`, `tags`, `comment`, `filter:`, `sort:` and `group:`, so the next look needs no hands.

**The access violation the report found is older than the marks.** The seven tests of `components::color_wheel` and the grid's `a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with `0xc0000005` whenever they run alone, in the workspace's test binary too, and pass inside the whole run. 0.16.1's own sources (`44679b2`) crash the same way, three runs of three. It is the Color Wheel's, for its own fix, and not this wave's.

The whole, after the review: `cargo test --workspace --no-fail-fast` 3,756 passed, 1 failed (`thumbs::tests::draws_a_pdfs_first_page`, as before), 51 ignored; clippy clean.

What is left, and whose it is:

- **Wave 4 (G1)**: `sidebar/rows.rs`'s `reads_files` still lists `tag:`; and, as before, `marks_all` for the sidebar, `client::marks_in`, `SearchRequest.marks`.
- **Wave 6, speed**: a sort by label works out what each row is shown as at every comparison (318 ms for 10,000 marked rows in a test build): a key a row, made once, as names have. With no mark read, `build_rows` makes the calls it made before after one `None` (0.28 ms for 10,000 rows in the default order); nobody measured the tree before it.
- **Wave 6, the look**: what keys do in a real window was not tried (the tour presses none): `Enter`, `Ctrl+Enter` and `Escape` in Edit comment… (its tests never saw the dialog's own key handler fire), and the block's in Quick Look.
- **Not carried** (each a card, or a line of section 12): the folders *inside* a folder that Windows' engine copies or moves to another drive (the queue carries them: it knows each one); a `.glint-marks` goes with its folder as a file, so merging into a folder that has one and answering Replace replaces that folder's lines; ledger lines under a folder that was renamed keep its old path; a line that moves loses a later version's keys.
- **Not run on a real drive without streams**: on NTFS the queue's and Windows' engine's calls have nothing to do for a file. `carry`'s own tests tell it a drive keeps none. Section 14, item 3.

Decided by the cards' agents, and kept:

- Edit comment…'s title for one item is `Comment of “name”`.
- A file reference whose sequence is 0 is never opened (NTFS reads 0 as any file with that number), and a far record with no name is read like any other.
- A label this PC lacks is washed in the color its name gets as a tag; with no labels in Settings a menu has no row of dots.
- The keys act on the selected rows: the item the keyboard is on is not a selection by itself.
- Taking a label off a selection that mixes labels and 0.16 color tags is two changes (two Undo steps).
- The Tags column works out how many chips fit from the letters' count, not by measuring text.

**A cut that still keeps the issue's word**: S0 and waves 1 to 3 are the three marks, their columns, keys, menu, panel, search and following the file. Wave 4's F1 to F3 are import and export, XYplorer's included. Everything else is what makes it Glint's.

## 10. What must not break

- **`glint.tags`**: its name, its format, `tags::read` and `tags_set` as they answer today; a 0.16 file read by 0.17 and a 0.17 file read by 0.16.
- **The tests there are** pass unchanged: `dialogs/tags.rs` (`:733`–`:989`), the sidebar's, `row_facts.rs`'s, `file_list/tests/facts.rs`. The two that list the Tags submenu's rows (`menus/tests.rs:395`–`:451`, `tests_content.rs:260`) change for exactly the rows of F7.
- **The end-to-end names** the Aware plan's section 8 lists.
- **Speed**: 6.6's second column, in the real app with `GLINT_FRAME_LOG=1`.
- **The index**: no more memory for a drive with nothing marked; a `.tags` snapshot from 0.16 loads; an app from 0.16 gets the answers it got.
- **The service's promises**: it still never reads or sends a file's own data, and never sends a comment's text.
- **A file is never worse off for being marked**: its dates, its contents, its hash and its signature are as they were (`tags_live_in_the_file_and_leave_its_dates` grows to all three streams and to a signed program).

## 11. Checking it (the coordinator, after wave 4)

1. `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`.
2. **Fixtures**, in `tests/fixtures/`: `tag.dat` in version 3 and version 5; each storage mode; `¶`; a comment with `|` in it; tags with `,` and with `, `; with and without a byte-order mark; `\r\n` and `\n`; a label number past the list; a file of 10,000 lines; a `.glint-marks` with a later version's keys, with a name that tries to leave its folder, and cut off mid-line; `marks-filter.txt` (6.5). *Written at S0*: `tests/fixtures/marks/`, whose README says what each file must read as, and `tests/fixtures/marks-filter.txt`, whose header settles what the two filters do alike. The file of 10,000 lines is made by A3's test from the others (1.4 MB of UTF-16 that would only repeat them).
3. **Real drives**: NTFS; an exFAT stick (beside); a share with streams and one without; a ReFS Dev Drive; a OneDrive folder; a read-only file; a file another program holds open; a path over 260 characters; a folder's own marks; two hard links to one file (the marks are the file's, so both names show them); a file whose name fills its record, to see a comment leave it.
4. **The measurements** of 6.6, before and after, in the commit that lands each.
5. **The look**, in a debug copy on a scratch profile (`GLINT_SELFTEST=fly:…`, `glint-ui/src/selftest.rs`, and frames from `PrintWindow`), in both themes at 100% and 125%, in a 900 px window: a row of every label, labeled and selected, labeled under the pointer, an Arabic name, a long comment, twelve tags; the tiles; the Marks block for one item and for several; the menu's row of dots; the import's preview with the fixtures. With Windows' animation effects off.
6. **A real XYplorer** (its trial): mark files there in each storage mode, bring them in, rename some in a terminal first, follow a change, export back and `tagload` it. Until this is done the docs say *not yet tried against a real XYplorer*.
7. **e2e**: a new check `marks`: `Alt+1` on a file in the arena, `label:red` finds it, it is renamed with `ren` in a terminal, the row still has its label.
8. **Docs**: `docs/marks.md` (the formats, PowerShell, XYplorer); `docs/features.md` (Tags becomes Label, tags and comment); `docs/instant-search.md` (6.4's sentences); STANDARDS §5.1 gains "Marks" (M1 to M11 in the repository's words); `NEXT_RELEASE.md`; the README's What's new; a reply on the issue.

## 12. Left out, on purpose

- **Writing XYplorer's `tag.dat` live** (7.5).
- **Custom columns** as XYplorer's sixteen extra fields are. They come in as tags and comment lines (7.4).
- **OR between different filters, and brackets.** A space is and, `;` is or within one filter, `!` is not: what the issue names. XYplorer's boolean syntax is its own and is not copied.
- **Group by tag**: a file with three tags would stand in three groups, which the rows cannot do.
- **Marks through a cloud's sync.** OneDrive and Dropbox do not sync streams, so marks on files there stay on this PC; export and import move them. The way to it is known (beside the files in a synced folder, and the service reading those files) and is its own wave.
- **Sidecars in the index.** On a drive instant search covers, marks beside the files are found in the open folder and by fd, not by the index; F23 offers to move them into the files.
- **Tags in the index on ReFS** (G9), as today.
- **Other programs' marks**: Directory Opus, Total Commander's `descript.ion`, TagSpaces, Windows' own keywords. The pipeline of 7.3 takes a new source each.
- **A comment's history**, and **the sort of Date taken and the other media columns** (the door 6.1 opens fixes them in a few lines; not this wave's promise).

## 13. Decided for you (say if you want otherwise)

- **The words**: Label, Tags, Comment, as the issue and XYplorer have them; *marks* for the three together.
- **The keys**: `Alt+1`…`Alt+9`, `Alt+0`, `Alt+T`, `Alt+N`. In Edit comment…, `Enter` is a new line and `Ctrl+Enter` saves.
- **A label is a wash in the row**, not a bar at its edge and not the whole line's text in color (M11); its strength is one function in the theme, tuned in both themes in the real app.
- **Today's color tags become labels** (5.6), moved once on indexed drives.
- **A label is stored by its name**, and renaming one rewrites nothing (5.5).
- **`.glint-marks`** is the name, JSON Lines the format, one file per folder.
- **The import fills what is empty** by default, keeps what it cannot place waiting, and follows XYplorer only when asked to.
- **Extra columns** come in as 7.4 says.
- **The service holds the first 4 KB of comments and never sends their text.**
- **Lower-tier agents do the cards; the coordinator does S0, the wiring and the checking.** Twenty-two cards over five waves.

## 14. To find out before S0

Each is a question the research left open; none changes the shape, each changes a card's wording. S0's answers follow each one; 1, 2 and 8 need the person (a real XYplorer, a NAS).

1. **A real `tag.dat`.** Whether it starts with a byte-order mark, its line ends, how many labels version 5 allows, and what version 3 has after the comment (a sample in XYplorer's forum shows four more `|`). XYplorer's trial answers it; the issue's author could be asked for a file with the names scrubbed.
   *Open: for the person.* The fixtures follow 7.1 until then (their README says so).
2. **Storage 3's paths**: the manual's quote has no example.
   *Open: for the person*, with item 1. The fixture writes them relative without `.\` (`sub\notes.md`).
3. **A copy to a drive without streams.** Expected: `CopyFileExW` copies the file and drops the streams silently, and Windows' engine asks *copy without its properties?* for a file with `glint.*` streams, as it must do for tagged files today. If it asks, what `marks::carry` should say afterwards.
   *Not tried*: this PC has no FAT or exFAT drive, no WSL share, and making one (a virtual disk) needs an administrator. From the code: the copy queue calls `CopyFileExW` with `COPY_FILE_COPY_SYMLINK` alone (`queue.rs:776`, `:832`); Windows' engine copies and moves with `FOF_ALLOWUNDO | FOF_NOCONFIRMMKDIR` (`shell.rs:785`–`:841`), without `FOF_NOCONFIRMATION`, so the expected question would be asked. Either way `carry` can't learn of the loss from an error: for each pair whose destination exists and can't hold streams (`caps::has_streams`), it writes the line beside the copy once the copy is done, and a pair Windows' question skipped has no destination and is left alone. Afterwards one line says where the marks went (*Marks went beside the copies, in .glint-marks*). To try on the exFAT stick of 11.3; whether to keep Windows' question (it says the streams don't go, and the marks still do) is the person's choice.
4. **Whether writing a stream and putting the dates back wakes the folder watcher** (`notify` asks for no stream changes), so a mark set by a script shows without `F5`.
   *Yes, measured.* With notify's filter (`FILE_NOTIFY_CHANGE_FILE_NAME | DIR_NAME | ATTRIBUTES | SIZE | LAST_WRITE | CREATION | SECURITY`), `tags::write` with the dates put back gives the file two `MODIFIED` within 2 ms (the write, then `SetFileTime`); so does a write that leaves the dates moved, PowerShell's `Set-Content f -Stream glint.label -Value X` (212 ms, PowerShell's start) and a stream removed. notify 8.2.2 hands them on as `Modify(Any)` and `watch.rs` reads the folder again on any event, so the row is read again with no `F5` and nothing changes in `watch.rs`. PowerShell moves Date modified (it doesn't put it back): `docs/marks.md` says so.
5. **`FindFirstStreamW` against opening each stream** (5.1), on 200 and on 10,000 files.
   *Measured; the read is `FileStreamInfo`.* Medians of 11 runs, in turns, files in Windows' cache, each way reading all three streams except today's (the tags alone):

   | Files | Today: one open | Three opens | `FindFirstStreamW`, then opens | `FileStreamInfo`, then opens |
   | :--- | ---: | ---: | ---: | ---: |
   | 200, none marked | 2.1 ms | 7.5 ms | 3.6 ms | **3.1 ms** |
   | 200, all three marks each | 8.6 ms (tags only) | 26.1 ms | 31.8 ms | 31.0 ms |
   | 10,000, none marked, 1 thread / 16 | 122 / 91 ms | 350 / 270 ms | 196 / 95 ms | 182 / 95 ms |
   | 10,000, all marked, 1 thread / 16 | 508 / 210 ms (tags only) | 1,428 / 659 ms | 1,773 / 857 ms | 1,701 / 844 ms |

   Unmarked files are most of any folder, and there listing first wins: one open for the file's attributes and `GetFileInformationByHandleEx(FileStreamInfo)`, then an open per `glint.*` stream listed (B1). Three opens are 15% quicker on a file that has all three marks; that is not worth a second way. Opening a stream that is there costs about 40 µs, a failed open 11.
6. **How long a comment stays in the record**, with a short and a long file name, and that a stream in an extension record is still read (`volume.rs:459` merges tags from one today).
   *Measured without an administrator*, from each stream's allocation (`FileStreamInfo`: a resident stream's is its size rounded to 8 bytes, a non-resident one's whole clusters of 4,096; checked against `FSCTL_GET_RETRIEVAL_POINTERS` on known cases; records are 1,024 bytes). Beside a 13-byte `glint.tags` and a 10-byte `glint.label`, the longest comment that stays resident is 552 bytes for `a.txt` when it is empty and 504 when it holds 10 KB; for a 200-character name (which has an 8.3 alias too) 40 bytes empty and 384 with 10 KB (that one most likely moved to an extension record; which record holds it can only be read by an administrator, `marks-s0 ext` run elevated). So a comment is in the record when it is short and the name is short; most comments on long names are `far` (C2).
   The running 0.16.1 service and extension records: it found a tag written to a file whose record was already full (30 hard links with long names, 16 streams with long names, 150 short links), but **not one written before 30 long hard links filled the record**, even 12 s later. A record whose names spill over also shows only some of its names (1 of 31 paths, 6 of 151): the same cause, most likely, and older than marks. The likely cause: the attribute list became non-resident, and `volume.rs` reads a non-resident attribute list from the volume's clusters (`read_runs`), which Windows may not have written yet; so the extension records aren't found. For C1 and C2: read a non-resident attribute list through the file system, as `far.rs` reads streams, or treat its record as `far`. A 2.3 KB `glint.tags` (not resident) isn't found either: that is G3.
7. **That `ReplaceFileW` keeps the old file's streams** (`lib.rs:129` says so) and that lofty's save does (song tags).
   *Yes, both, tried.* `replace_contents` (copied from `lib.rs:129`–`:178`) on a file with all three streams: new contents, the three streams kept, no temporary file left. Song details (`songtags.rs:107`–`:121`, `actions.rs:116`–`:196`): the song is copied into a work folder (`fs::copy` is `CopyFileExW`, which takes the streams), lofty saves the copy in place, the original goes to the Recycle Bin and the copy takes its name; the three streams are there afterwards and the title is set (a WAV). G6 stands for the tools that make a new file rather than a copy.
8. **Shares**: whether NAS boxes that say they have named streams keep them.
   *Open: for the person* (a NAS); 11.3's real drives.

## Sources for XYplorer's format

- The manual's pages on the tag database (345–347, and 66 on storage), quoted in Everything's forum: [XYplorer Tag Support](https://www.voidtools.com/forum/viewtopic.php?p=56412).
- XYplorer's tour: [Tag-Based File Management](https://www.xyplorer.com/tour.php?page=tags) (UTF-16LE text).
- XYplorer's forum: [tag.dat documentation](https://www.xyplorer.com/xyfc/viewtopic.php?p=116386) (version 3), [Tag File Format](https://www.xyplorer.com/xyfc/viewtopic.php?p=196843), [XYplorer File Tags Sort Order?](https://www.xyplorer.com/xyfc/viewtopic.php?p=138700), [Syncing Tags across systems](https://www.xyplorer.com/xyfc/viewtopic.php?p=115308), [How to export tags to a different pc?](https://www.xyplorer.com/xyfc/viewtopic.php?p=141315) (`tagexport`, `tagload`), [Global v.s. local tag storage](https://www.xyplorer.com/xyfc/viewtopic.php?p=167736) (`XYplorerTag.dat`), [the search's field names](https://www.xyplorer.com/xyfc/viewtopic.php?p=70786).
- Where its data folder is: XYplorer's FAQ, [application data](https://xyplorer.com/faq-topic.php?id=appdata), and [What is correct File structure for No-Install Package?](https://www.xyplorer.com/xyfc/viewtopic.php?p=132646).
