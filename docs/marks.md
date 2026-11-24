# Marks: a label, tags and a comment that stay with the file

A **label** (one named color that says a status), **tags** (several words) and a **comment** (free text) can be put on any file or folder. Together they are called *marks*. They are kept **in the file itself** wherever the drive allows, so a file renamed in a terminal, moved by another program or copied to another PC keeps them. [features.md](features.md) says how they are set and shown; this page says where they live, what each format is, what follows a file and what doesn't, how to read and write them from a script, and how XYplorer's come along.

Written for 0.17 from the code as it is. Three things have **not yet been tried**, and the page says where they matter: *not yet tried against a real XYplorer*, *not yet tried on an exFAT drive*, *not yet tried on a NAS*.

## Where marks live

Glint keeps a file's marks in the first of three homes that works:

1. **The file's own streams**, when its drive keeps named streams (NTFS and ReFS do). Glint asks the drive (`FILE_NAMED_STREAMS`), not the error a write gives.
2. **`.glint-marks`**, a hidden text file in the same folder, when the drive has no streams (FAT, exFAT, a camera's card, some NAS shares) and the folder can be written.
3. **The ledger on this PC**, when neither can be written (a read-only share, a file inside an archive, a drive that isn't there).

Reading takes the first home that has anything for the item. Writing to a home takes the item out of the homes after it, so a mark is in one place, and **a file's own stream wins** over a line beside it or in the ledger. A change to an item that isn't there goes to the ledger (a line beside it would be dropped as an item that is gone). The preview pane's Marks block says *Kept beside the file* or *Kept on this PC* when that is where they are going.

Marking never changes a file's contents or its dates (Date modified, Date created and Date accessed are put back after a stream is written), and a read of marks never opens a file's own data.

### The three streams

| Stream | Holds | Read at most |
| :--- | :--- | ---: |
| `glint.label` | one line: the label's **name** | 256 bytes |
| `glint.tags` | one tag a line (0.16's format and name) | 4 KB |
| `glint.comment` | the comment: UTF-8, lines ended by `\n` | 64 KB |

All three are plain text with no format around them, so a script needs no parser. Each is written alone: setting a label never rewrites a comment. Before anything is written Glint tidies it: a label is one trimmed line, tags are trimmed with one of each (in any case), a comment has `\n` line ends and no NUL characters, and a label or comment of only white space is no mark. Glint refuses, with words, more tags than fit in 4 KB and a comment over 64 KB; it never cuts one.

The label is stored by its **name**, not a number (XYplorer stores a number, so renaming a label there changes what files say). A PC that has no label by that name still shows it, in a color worked out from the name, as it does for any tag.

A small mark fits in the file's own record in the Master File Table and costs no disk space of its own: a label, a few tags and a comment of up to about 500 bytes on a short file name stay in the record, and a longer comment, or a comment on a very long name, leaves it. Instant search reads a mark that left the record too; see [instant-search.md](instant-search.md).

### PowerShell, as it is

The streams are ordinary NTFS streams, so PowerShell reads and writes them with no help from Glint:

```powershell
Set-Content bond0.conf -Stream glint.label -Value Important
Get-Content bond0.conf -Stream glint.comment
Remove-Item bond0.conf -Stream glint.tags
```

A row shows the change at once, with no `F5`: writing a stream wakes Glint's folder watcher. **PowerShell moves the file's Date modified when it writes a stream; Glint doesn't.** If the date matters, put it back (`(Get-Item bond0.conf).LastWriteTime = $before`) or set marks in Glint.

### `.glint-marks`

A hidden file in the folder, JSON Lines, UTF-8, `\n` between lines, sorted by name (without regard to case) so it reads well in a diff:

```
{"glint-marks":1}
{"name":"bond0.conf","label":"Important","tags":["bond","netplan"],"comment":"mode 4 (802.3ad)\nLACP rate fast","at":1795436917}
```

- The first line holds the format's version. Each other line is one item of the folder, by `name`; `at` is when it was set, in Unix seconds. A line with nothing in it isn't kept.
- It is written whole, to a temporary file that takes the old one's place, and hidden. A line whose item is no longer in the folder goes the next time the file is written; a file with no lines left is removed. Copy the folder to any system and the marks are in it.
- It is read wherever it is found, on NTFS too: a folder copied back from a stick, or unpacked from a zip, shows its marks, and the pane's head says *12 here are beside the files* with **Move them into the files** (*1 mark here is beside its file*, **Move it into the file** for one). Where the file has its own marks, its stream wins and that line is stale rather than waiting. Keys Glint doesn't know (a later version's) leave with a line when it moves into a stream, since a stream has no place for them.
- What comes from outside is data: at most 4 MB and 100,000 lines are read, a line that isn't JSON or is cut off is left out, and **a name with `\`, `/` or `..` in it is refused**, so a `.glint-marks` can only speak of its own folder. Nothing in it is run.

Glint's *Move* and *Rename* keep the lines with the names. Moving a folder takes its `.glint-marks` with it, as a file.

### The ledger

`marks.jsonl` in Glint's data folder (`%LOCALAPPDATA%\com.shado.shadui-ripgrep`): the same lines with `"path"` in place of `"name"`, and `"why"`:

- `"away"`: an import waiting for an item that wasn't there (a drive that isn't connected, a file not found). The first time Glint lists the folder the item is in, its marks move from the ledger into the file.
- `"shut"`: the marks can't be written where the item is.

It is kept in memory by path (any case) and written the way `.glint-marks` is, a moment after it changes. Glint and its MCP server each read it again when the other wrote it, so neither loses the other's lines, and its lines follow a folder Glint renames or moves. The ledger is Glint's alone: it is not a place to look for a file's marks from outside.

## What follows the file

- **Rename, move, copy with any program** (on a drive with streams, and between two such drives): a file's streams go along. They follow it into the Recycle Bin and back. A folder's own marks stay with it when it is renamed, or moved on its drive.
- **Windows never copies a folder's own streams** (it copies a file's streams, not a folder's), so a copy of a folder made by another program is bare. **Glint's own copies and moves carry the folder's marks** too: through Windows' file engine, the copy queue, a drop and the other pane alike.
- **Between drives that keep different homes**, Glint's copy or move writes the marks where they belong: from NTFS to exFAT it writes the lines beside the copies, from exFAT to NTFS it writes the streams. A move takes an original's line only once the original is deleted, so a move that stopped leaves the original marked. One line says where the marks went (*Marks went beside the copies, in .glint-marks*). **Not yet tried on an exFAT drive**: the code is tested with a drive that says it keeps no streams, and the questions Windows asks when it copies a file with streams to a drive without them are not seen. A copy made by another program to such a drive loses the streams, as with any other stream.
- **A tool that makes a new file in the old one's place** (Make smaller, Remove personal info, the PDF tools, Convert document and the others) gives it the old one's marks.
- **A zip Glint makes** holds a `.glint-marks` in each folder with marked items; unpacked, the folder shows those marks (see `.glint-marks` above). Windows' own zip folders ignore the hidden bit, so someone who opens the zip in Explorer sees that small file, once for each marked folder.
- **A cloud's sync** (OneDrive, Dropbox) doesn't carry streams, so marks on files there stay on this PC. Export and import move them.
- **Not carried**: marks follow a file only where Glint or the drive does it. A file copied by another program onto a drive without streams, or sent by e-mail, is bare. **Not yet tried on a NAS**: whether a share that says it keeps named streams keeps them is the share's own answer.

## Labels

Settings → Files → Labels holds the list: it starts as Red, Orange, Yellow, Green, Blue, Purple and Gray (the same seven as XYplorer's), and can be renamed, recolored, added to (32 at most) and reordered. `Alt+1` to `Alt+9` give the selection the first nine.

- A label is **stored by name**. When one is renamed ("Red" to "Important"), its old names are kept with it, so a file that still says *Red* is drawn as *Important*, `label:important` finds it, and the file gets the new name the next time its marks are written. Nothing is rewritten in bulk.
- A label **this PC lacks** (a file from another PC) is drawn, washed in a color worked out from its name.
- **0.16's color tags become labels.** A file tagged *Red* in 0.16 has no `glint.label`. From 0.17 it is *drawn* with that label, and the tag is not drawn again among its tags; `tag:red` still finds it; and the color tag moves from `glint.tags` to `glint.label` the next time the file's marks are written, and once, in the background, for files on the drives instant search covers (half a minute after Glint starts, at background priority, taken up where it stopped if Glint closes; it waits until the instant search service is this version's). A 0.16 Glint that meets such a file after the move sees the label stream and no color tag.

## The filter's words

Typed in the search box, in the sidebar's saved searches, in Quick search and in `search_files` for an AI agent:

| Filter | Finds |
| :--- | :--- |
| `label:important` | files with that label; `label:red;orange` for either |
| `tag:netplan` | files with that tag; a label answers too, so `tag:red` finds what it always found |
| `comment:lacp` | every word must be in the comment; `comment:"LACP rate"` keeps a phrase |
| `has:label`, `has:tag`, `has:comment`, `has:mark` | having any |
| `!label:archived`, `!has:comment` | `!` before any of them: not |

A space is *and*; `;` is *or* within one filter. XYplorer's names work too: `lbl:`, `tags:` and `cmt:`. Plain words are understood as well: `tagged netplan`, `with comments`, `labeled pending from last week`.

Where they work:

- **The open folder, as you type.** It reads the marks of the folder's items to answer, and shows them as it goes.
- **The index**, for the drives instant search covers: answered in milliseconds, with the names. When no tag or label has the name typed, the nearest ones stand in (*Close matches for…*), by the rule names have.
- **fd's drives**, the ones the index doesn't cover: each file's marks are read as the search goes, one folder's `.glint-marks` once a search, and the ledger's matches under the folders searched are added.
- **In comments**: a plain word typed in the search box also finds files whose comment or tags hold it, listed under **In comments** with the matching line, as **Inside files** does. This comes from the index alone: the open folder's filter and fd's drives don't look in comments.

The index and the open folder's filter are two parsers that run one table of cases, so they answer alike.

## XYplorer

*Not yet tried against a real XYplorer.* What follows is built from XYplorer's manual, its forum and one real `tag.dat` (from XYplorer 19.70 and later, in Chinese, which a user sent for the issue), read by Glint's tests.

XYplorer keeps all marks in one file, `tag.dat` (UTF-16 text), in its data folder: `%APPDATA%\XYplorer`; a portable copy's `Data` folder; or where its `Startup.ini` says. Glint **only reads** it: never writes, moves or holds it open (it is read with sharing that lets XYplorer write meanwhile), and it never writes XYplorer's own `tag.dat`.

- **Found by itself.** When there is a `tag.dat` that was never brought in, a suggestion offers *Bring your XYplorer labels, tags and comments*, with how many items it marks. A `tag.dat` or `XYplorerTag.dat` has *Import marks…* in its own menu under Suggested, and `Ctrl+K` has it too.
- **Import marks…** reads the file (version 3 and 5, and a later version by the same rules, saying so) and shows a **preview** before anything is written: the version and date, how many items, labels, tags and comments, and then
  - **Labels**: each XYplorer label and the Glint label it becomes. By name (*Grey* is *Gray*); else by XYplorer's own color, for an XYplorer in another language (*红色* is *Red*, which Glint then calls *红色*, keeping the old name for files that say it); else *New label*, in XYplorer's color (it comes in as a tag past 32 labels). Any can be pointed elsewhere.
  - **Extra columns**, each with what it becomes, and a switch to leave them out.
  - **Where the items are**: here; moved since (found where they are now, from instant search's history of the last 30 days: XYplorer's orphans come back); on a drive that isn't connected (they wait, in the ledger); not found (they wait, or are left out); already marked in Glint.
- **Fill what is empty** (the default) gives an item the label if it has none, adds the tags, and gives the comment if it has none (a different comment keeps Glint's, and the report says so). The other choice, *The file's marks replace Glint's*, replaces all three.
- **Applying** runs in the background with its progress in the dock, then says *Marked 1,190 items from XYplorer · 61 waiting · 33 not found*, with **Undo** and **What happened** (every item with its outcome, to copy). It is one Undo step, and **Settings → Files → Marks → Take back the last import** does it later. Running an import twice changes nothing.
- **Following** (off until chosen: *Keep following this file* in the preview, or *Keep following XYplorer* in Settings → Files → Marks). Each time Glint starts or comes to the front, if `tag.dat`'s date changed, what is new or changed in it comes in, and a line gone from XYplorer takes its marks off only where they are still what XYplorer had. One quiet line says so: *3 marks came from XYplorer*. Glint remembers what it applied item by item in `xyplorer.json`, and each import's marks before it in a journal under `imports` in its data folder (kept 30 days).
- **What the extra columns become** (Glint has no custom columns): a checkbox that is on becomes the tag `Caption`; a rating the tag `Caption: 4`; a list the tag `Caption: value`; text, number, date, location, image and script become a line `Caption: value` at the end of the comment, after an empty line; an icon is left out, and the preview says so. An unused extra column isn't listed.
- **Export marks…** (on a folder, or `Ctrl+K`) writes everything marked below it into one file: Glint's own JSON Lines (the sidecar's lines with paths relative to the file's folder, the labels in the first line), or, as **XYplorer**, an `XYplorerTag.dat` (version 5, storage 3) that XYplorer's `tagload` merges. The file must be saved in the folder exported, or one above it, since its paths start from its own folder. Glint can import its own export too.
- **Not done**: writing XYplorer's `tag.dat` while XYplorer runs. XYplorer keeps it in memory and writes it itself, so a second writer would lose one side's changes. Following is one way; going back is an export you run.

## Scripts and AI agents

- **PowerShell** reads and writes the streams as shown above.
- **AI agents** get two tools in Glint's MCP server: `get_marks` reads the label, tags and comment of files and folders (it only reads), and `set_marks` gives files a label, adds or takes off tags, or sets a comment. `set_marks` **asks you first**, in a Windows dialog that names the files and what will be written (one line each: a label or tag with a line break in it is shown and kept as one line), and every change is written down: **Settings → AI agents** lists what agents did, each with an **Undo** that puts back only what the agent gave (what you changed after it stays). `search_files` takes `tag:`, `label:`, `comment:` and `has:`. What a comment says is the person's data, and agents are told not to take it as instructions. The MCP server is a separate process with its own copy of the ledger; it reads the file again before it writes, so only two changes to the ledger in the same instant can lose one.
- **Folder rules** can *Give it a label* to what they move or find (and *Replace a label a file already has* or leave it); the rules' log is their Undo.
- **Batch rename** has `{label}` and `{tags}`; one that is empty takes the space beside it along, so `{label} {name}` gives `report.pdf` for a file with no label.

## Limits, in one place

| | |
| :--- | ---: |
| A label's name | 256 bytes |
| Tags on one item | 4 KB in all |
| A comment | 64 KB (the index keeps the first 4 KB) |
| Labels in Settings | 32 |
| `.glint-marks` read | 4 MB, 100,000 lines |
| A `tag.dat` read | 64 MB |
| Tags an agent may set in one call | 100 |
| Items whose marks the preview pane's block reads | 1,000 |

## Not yet tried

- **Against a real XYplorer**: everything under XYplorer above was built from its documentation and the one file a user sent. Each storage mode, bringing in after renames in a terminal, following a change, and `tagload` of an export are to be tried with its trial.
- **On an exFAT drive**: the carry between homes, `.glint-marks` through `replace_contents`, and what Windows asks when streams would be lost.
- **On a NAS**: whether a share that says it keeps streams does.
- ReFS drives keep marks in the file as NTFS does, but instant search's index holds none there.
