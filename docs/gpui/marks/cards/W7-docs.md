# Card W7 — The docs (11.8)

Wave 6, round 1 · docs only · no code

**You own:** `docs/marks.md` (new), `docs/features.md` (the part on tags), `docs/instant-search.md` (the marks sentences of §6.4: **not** the sentence on oplocks, which is card W3's this round), `docs/gpui/STANDARDS.md` (a new part "Marks" in §5, beside §5.1, not inside it), `NEXT_RELEASE.md`. Nothing else. **Not** the README's What's new, and **no reply on the issue**: those go out with the release.
**Read first:** `docs/gpui/marks/PLAN.md` whole; `docs/gpui/STANDARDS.md` §5; each of your files as it is now (`NEXT_RELEASE.md` has its own shape: keep it); two or three other files in `docs/` to match the repository's voice.
**Check with:** reading each sentence against the code. **Every sentence true of the code as it is: read the code, not the plan, where they differ** (the reports' "as it landed" sections say where they differ).

## docs/marks.md

- **The three streams** and their formats (5.1): `glint.label` (one line: the label's name, 256 bytes read), `glint.tags` (one tag a line, 4 KB, 0.16's format), `glint.comment` (UTF-8, lines ended by `\n`, 64 KB; the index keeps the first 4 KB). Check the caps against `glint_model::marks`'s constants.
- **PowerShell as it is** (F34): `Set-Content bond0.conf -Stream glint.label -Value Important`, `Get-Content bond0.conf -Stream glint.comment`, `Remove-Item bond0.conf -Stream glint.tags`; that PowerShell moves Date modified and Glint doesn't (PLAN §14 item 4).
- **`.glint-marks`** (5.2: its lines, hidden, sorted, the caps, a name with `\`, `/` or `..` refused) and **the ledger** (5.3: where, `why`). Which home is used when (5.4), and that a file's own stream wins.
- **What follows the file** (F18 to F23), as the code does it: a folder's own marks and Windows' copies; Glint's copies; a zip Glint makes holds a `.glint-marks` (and Windows' zip folders show it).
- **Labels**: stored by name, renamed through `was`, a label this PC lacks drawn with a color from its name; 0.16's color tags become labels (5.6) as card W1 makes them (read `crates/glint-core/src/marks/` for what is there when you write; if W1's code isn't in yet, write what 5.6 says and mark the sentence for the coordinator to check).
- **The filter's words** (6.5, F12): `label:`, `tag:`, `comment:`, `has:`, `!`, `;` for or, XYplorer's `lbl:`, `tags:`, `cmt:`; where they work (the open folder, the index, fd's drives; *In comments* from the index alone).
- **XYplorer** (section 7): what Import marks… reads, the preview, *fill what is empty*, following (off until chosen), what the extra columns become (7.4), Export marks… → XYplorer.
- **AI agents**: `get_marks` and `set_marks`, asked first, undone from Settings → AI agents. **Folder rules** that label; Batch rename's `{label}` and `{tags}`.
- **Not tried** — say so in these words: *not yet tried against a real XYplorer*, *not yet tried on an exFAT drive*, *not yet tried on a NAS*.

## The others

- `features.md`: Tags becomes **Label, tags and comment**, in its shape, linking `marks.md`.
- `instant-search.md`: 6.4's sentences: the index reads the three streams; a stream that left the record is read through the file system by file reference (the far reads); the service never sends a comment's text, and holds the first 4 KB of every account's comments, so whoever may ask it can learn which files have a word in theirs. Leave the oplock sentence to W3.
- STANDARDS §5: a part **Marks**: M1 to M11 (PLAN §3) in the repository's words, as rules a later card can follow.
- `NEXT_RELEASE.md`: the Marks wave in its own shape, plain, for the person who reads release notes.

## What the last waves learned (hold to these)

- **No command that walks a drive.** No `find /`, no recursive listing outside the repository. Find files with the search tools inside the repository; a crate's source with `cargo metadata`. Before your last message, stop every process you started.
- **Others are editing other files in this tree now.** A build can meet another agent's half-written file: wait and build again; never touch their file. Build and test by your own filter. A build that has waited ten minutes on Cargo's lock is said in your report rather than waited on.
- **A file-heavy test that seems to hang** may be the PC: while it is locked, Windows' maintenance makes every new file wait on the antivirus. Time a few dozen small writes before calling it a bug; don't trust a timing from such a window.
- **Backslashes**: edit files with the editing tools, never a script typed into a shell.
- **Tests never touch the PC's own Glint data**: a test that writes marks uses a folder of its own and a ledger of its own (as `marks::apply`'s tests do with `World`), never the shared ledger, journals or `xyplorer.json` in Glint's data folder.
- No `git` command that changes anything, no `cargo fmt`, no new crate, nothing downloaded, no `tests\e2e\run.ps1`, no installed Glint, no real AI agent, and **never install, stop or replace the index service**.
- **Known, and not yours**: the seven tests of `components::color_wheel` and `file_grid::tile::tests::a_photos_dots_name_their_colors_and_press_one_to_set_the_wheel_on_it` end with an access violation when run alone. Say so and go on.

## Done when

Every file is written and every sentence has been checked against the code. Your last message is your section of the report: rule 9's shape (CARD, STATUS, FILES, TESTS, CLIPPY, NOT DONE, NEEDS FROM COORDINATOR, UNSURE), plus WHERE THE PLAN AND THE CODE DIFFERED (each place you wrote what the code does rather than what the plan says).
