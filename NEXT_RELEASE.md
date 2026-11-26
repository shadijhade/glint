# Glint: after 0.13

*0.13.0 came out on 2026-11-16 ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.0)),
0.13.1, a fix for its zoom, the same day ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.1)),
and 0.13.2, updates that need nobody, that evening ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.2)).
0.14.0 is the release of what's under **In 0.14.0** and of **Planned for 0.14** (audited 2026-11-18 from `5630c16`), done the same day, every item of it: each says where (its commit on master), and the M0 table has the numbers ([its release](https://github.com/shadijhade/glint/releases/tag/v0.14.0)).
The plan for 0.12 that was here is in git history (`git show df1245a:NEXT_RELEASE.md`), the plans for 0.9 to 0.11
before it (`git show 47c33fa:NEXT_RELEASE.md`), and 0.13's own plan, the menu and the dock that know the selection,
is [docs/gpui/aware/PLAN.md](docs/gpui/aware/PLAN.md) with its task cards beside it.*

## 0.18.1: a tag's search shows what it found, and network shares below the Recycle Bin ([#11](https://github.com/shadijhade/glint/issues/11), [#10](https://github.com/shadijhade/glint/issues/10)) (released 2026-11-26)

*Released 2026-11-26 from `174efd4` ([its release](https://github.com/shadijhade/glint/releases/tag/v0.18.1)) by
`ship`, in one build (run 45843462820, its crate checks passing on GitHub). Scoop has it, winget's pull request is
microsoft/winget-pkgs#490384, and the Store's package was left for Partner Center's pages.*

Both reported by Helius hours after 0.18.0 came out, both answered on their issues.

- **#11, a tag's chip or a label's dot showed This PC** (`5853bcc`). A search of every drive keeps This PC as the
  tab's path, and the pane chose the This PC page by the path alone, so the status bar said "5 found" over the folders
  and drives. `Tab::shows_this_pc` (This PC with no search) now decides it for the pane, the This PC page and the
  Color Wheel; `a_tag_label_or_saved_search_of_every_drive_shows_its_results_and_not_the_this_pc_page`.
- **#10, a NAS share in the sidebar as "nShare"** (`3a2e529`). A pinned place on another computer
  (`paths::is_network_path`: `\\server\…`, not WSL's or `\\?\`) sits with the drives, below the Recycle Bin, as a
  network drive; a share is called "share (\\server)" until it has a name. **Rename in sidebar…** on a pinned place's
  menu keeps a name in `AppState::pin_names` ("pinNames"), shown only in the rail: the tooltip keeps the path,
  unpinning forgets the name, and the settings file carries it. Checked in a debug build on `\\localhost\D$\glint-look`
  (pin, rename, tooltip, one click, a second share, a restart, the Chinese menu). Not tried: a real NAS that asks for
  a password (Glint uses the connection Windows keeps; it has no sign-in box of its own).
- **A photo's Place link** in Quick Look and the preview pane starts at the edge of its column (`c229150`).

## In 0.18.0: Glint in Chinese, German and Arabic ([#9](https://github.com/shadijhade/glint/issues/9)) (released 2026-11-25)

*Released 2026-11-25 from `4fe038f` ([its release](https://github.com/shadijhade/glint/releases/tag/v0.18.0)) by
`ship`, with the speed fixes below, in one build: no new advisory, and the Windows 10 import check passed on a
release build first. Helius's Chinese catalog went in as their commit (`45a3d75`), with labels made 颜色标签 apart
from tags and the confirmations' blank lines put back (`e4d421b`). Scoop has it, winget's pull request is
microsoft/winget-pkgs#489316, and the Store's package was left for Partner Center's pages.*

A contributor offered a Chinese translation (2026-11-25). The interface already went through `t!("English")` (the
hook 0.13 kept when Arabic went), so the work was everything around it. [docs/translating.md](docs/translating.md)
is the guide. The same day German and Arabic followed, translated here: `locales/de.po` and `locales/ar.po`,
complete first drafts of all 3,851 texts.

- **Arabic is right to left again:** `i18n::is_rtl()` is true for it, so the window mirrors itself with the
  start/end layout that 0.13 kept. Its counts take CLDR's six forms: "{n} files" is followed by entries of the
  same English with `msgctxt` zero, two, few and many (`ملفان`, `{n} ملفات`, `{n} ملفًا`), which
  `format::plural` picks by the number (`i18n::count_form`). `update ar` gives every count those entries, and the
  text for one may leave `{n}` out ("ملف واحد").
- **Which language**: German for any `de-…`, Arabic for any `ar-…`, as Windows lists them.
- **Found by looking at them:**
  - A Quick access tile's name of two words wraps onto a second line instead of losing its end ("Zuletzt verwendet", "مقاطع الفيديو"). Tiles sit a little tighter, so "Dokumente" fits; "Benutzerordner" is still cut, with its tooltip.
  - The folder line ("4 folders · 13 files · 158 KB") and the status bar's counts go through `plural`, so Arabic says "13 ملفًا".
  - In Arabic, a size or time keeps its order ("158 KB", not "KB 158") through `kit::ltr_unit` (LRE…PDF: the window's text ignores LRI…PDI). The status bar's timing is set as a right-to-left line.
  - The preview's size line said "bytes" in English, and is now translated.
  - The trim handles' screen-reader names became "Start of the part" and "End of the part": "Start" and "End" were also the instant search button and ending a process.
  - The Time Machine's arrow tooltips in Arabic name the keys it swaps when mirrored.

- **Catalogs:** gettext files in `locales/`, `zh-CN.po` first, compiled into Glint by glint-model's build.rs
  (sorted tables, a binary search over static text, nothing read at start). A translation whose placeholders
  differ from the English is left out with a warning, and `every_translation_in_locales_is_used` fails on it.
- **Which language:** Windows' display languages, the first with a catalog (English counts). Traditional Chinese
  shows English. There's still no choice in Settings. `GLINT_LANGUAGE` overrides it: `en`, `zh-CN`, `qps-ploc`
  (the pseudo-language) or a catalog file (`GLINT_LANGUAGE=D:\zh-CN.po`), which lets a translator see their work
  in Glint as installed.
- **Every text found:** `node scripts/i18n.mjs extract | update | status` writes `locales/glint.pot` (3,851
  texts) and merges it into each catalog, keeping changed texts as fuzzy guesses. Table texts are marked
  `english!("…")` (columns, file kinds, group headings, tile sizes, the search box's hints, the sidebar, Settings'
  index, the shortcuts sheet).
- **English-only bits made translatable:** `plural(n, "file")` is "{n} file" and "{n} files" in the catalog. The
  Type column ("PNG image"), group headings ("Today", "Folders"), the menus' key templates ("Cut ({0})") and the
  preview's item count now go through the catalog. Known folders and the fixed places are named in the
  interface's language in tabs, crumbs and drags ("桌面"), and a folder that only happens to be called "Music"
  keeps its name.
- **Left:**
  - Messages made by glint-core (an archive that can't be read, a backup that failed) are still English inside
    the translated sentence around them.
  - The installer, the Store listing and the website are English too.
  - In Arabic, the window's own minimize, maximize and close buttons stay at the top right.
  - Key names stay as the code writes them (Ctrl, not German's Strg).
  - Dates follow Windows' format, not the interface's language.
  - Counts written straight into a sentence with `t!` (not `plural`) have one form in Arabic. The translators
    worded those to read right for any number.
  - action_run.rs fills an English noun into "Next to each {noun}…".

### Checks

`cargo test -p glint-model` and `-p glint-ui` pass, and `node scripts/lib/i18n.test.mjs` passes. The debug build
was looked at in `qps-ploc` and with a 37-text Chinese file: Chinese draws in a Chinese font on an English
Windows, mixed "JPG 图像" lines sit on one baseline, and a broken entry is reported on stderr. The e2e now pins
`GLINT_LANGUAGE=en`: 22/22 on 2026-11-25.

German and Arabic, the same evening:
- `cargo test -p glint-model` passes (442), and so does `-p glint-ui` (2,901), with both catalogs built in.
- `cargo clippy -p glint-model -p glint-ui --all-targets -D warnings` is clean, and `node scripts/lib/i18n.test.mjs`
  passes.
- `node scripts/i18n.mjs status` reports none left out.
- The debug build was looked at with `GLINT_LANGUAGE=de` and `ar`: the main window, the right-click menu and the
  preview. In Arabic the window mirrors: the sidebar is on the right, the crumbs and arrows point the other way,
  and the columns run from the right.
- The e2e wasn't run again: it pins English.

## In 0.18.0: speed, from an outside list of 30 (checked against the code 2026-11-25; released 2026-11-25)

The user's Antigravity agent wrote a ranked list of 30 speed fixes. Each was read against the code before
anything changed; several premises were wrong, so the numbers below are the list's own unless they say
"measured".

**Built:**
- **Start (1):** the last session's folders are read ahead before GPUI and the GPU are made
  (`glint_core::startup::begin` from `glint_ui::run`), not after; a secondary window still doesn't.
- **Dates (2):** a row's short date is cached by the minute (it has no seconds), so a folder of files written
  milliseconds apart makes one label a minute, not one a file and a cache wiped at 4,096. Windows' date, time
  and locale calls write into a stack buffer first: one call instead of two.
- **Index service (4, 11, 12):** a snapshot is written under the tree's read lock but flushed to the disk
  (`FlushFileBuffers`) and renamed after it, so a change waiting for the lock no longer holds every search behind
  the disk. `marks_in` (asked on every folder opened) goes through the marked records' names instead of all of the
  drive's. A frame's length and kind are read in one call and the payload straight into its buffer.
- **Video (6):** Media Foundation is started once for the run (`mfapi::keep_started`); a scrubbed tile's start and
  stop are counts after that.
- **Lists (8, vendored GPUI):** a uniform list measures its sizing row once a frame, not twice (docs/gpui/VENDOR.md).
- **Git (10):** changes inside `target`, `node_modules`, `__pycache__`, `.venv`, `.next`, `.nuxt` and `.turbo` no
  longer count, so a build in a watched repository doesn't work out its whole status every half second.
- **Marks (14):** a stream listing is compared as UTF-16, with no strings made per stream.
- **Ctrl+K (15):** the folders' usage scores are looked up in maps built once, not by scanning the visits and
  zoxide lists (lowercasing each entry) for every candidate.
- **Duplicates (16):** the dialog no longer copies the whole report (every duplicate's path) on each draw; its rows
  are shared.
- **Bulk work and selections (18):** restoring or purging a big Recycle Bin selection and the file engine's report
  match through sets, not a scan per item; the menu, Enter, the palette, Copy path and the preview pane's
  several-items card use `Rows::chosen` (it stops at the last selected row) instead of walking every row.
- **Color Wheel (19):** the cached palettes are read off the UI thread (the first time, from disk), and a warming
  stops once the tab shows another folder or another warming starts.
- **Documents (20):** a deck's slides and a book's chapters are read from one opening of the zip, not one each.
- **Quick Look (22):** a held arrow key shows an item each 70 ms instead of starting a full decode for every row it
  passes; a single press still shows at once.
- **Memory (29):** the picture a closed Quick Look showed leaves memory and the GPU atlas on the next frame
  (the workspace drains it too), not when a preview is next drawn.

**Left, and why:**
- **Wrong premise:** 13 (core's thumbnail cache isn't a duplicate: the list, columns, preview, photo map and more
  read it, not only the grid) and 17 (a splitter's `on_resize` fires once, on mouse up, not per pixel).
- **No measured problem, a cost either way:** 3 (thin LTO already inlines across a crate's units, and 0.16 measured
  no gain from one unit for glint-ui and glint-core), 5 (background mode is what keeps the quiet lane's disk reads
  from slowing the window's own), 7 (a 2048² atlas page is 16 MB of VRAM for the first picture), 9 (the service's
  memory is a few huge arrays; the heap barely sees it).
- **Small, on paths already measured fast:** 21, 23, 24, 26, 27; and the ColorWheelBar half of 16.
- **Bigger, worth their own pass:** 25 (index queries: rank before select, subtree bitset for scoped search), 28
  (a find box filter of more than 5,000 rows off the UI thread; Batch Rename's pattern compiled once), 30 (LIFO
  thumbnails, a pipe pool, an STA thread for Windows' previewers), 22's decode cancel, and 11's question: the head
  reads every row's marks (up to 5,000 files) to count label chips in a folder the index doesn't cover.

### Checks

`cargo test` for glint-core (536), glint-model (440, one new: a minute's moments share a label), glint-ui (2,901;
Quick Look's arrow test now waits out the settle) and shadui-index (160) pass, and `cargo clippy --workspace
--all-targets -- -D warnings` is clean (2026-11-25). Not measured in the app yet (no start bench, no frame log), no
release build, and the e2e wasn't run.

## In 0.17.0: a label, tags and a comment that stay with the file (released 2026-11-24)

*[Issue #8](https://github.com/shadijhade/glint/issues/8), Label, Tags and Comment as in XYplorer, and XYplorer's own brought along.
Built as the Marks wave, [its plan](docs/gpui/marks/PLAN.md) and task cards beside it: the plan and its contracts (`f107938`),
waves 1 to 3 (`e26c229`), 4 (`ff4f387`), 5 (`6c537c6`) and 6 (`c6b9e6a`: the docs, the last fixes and the checks). The waves'
reports name the commits of the branch they were built on (`00cf486`, `12a6e5e`, `c6a4c8b`), which master doesn't have.
[docs/marks.md](docs/marks.md) has the formats, PowerShell and what follows a file.
Released 2026-11-24 from `84612e8` ([its release](https://github.com/shadijhade/glint/releases/tag/v0.17.0)) by `ship`,
after two fixes the release build found: `parquet` 60.2.0 for a new advisory, RUSTSEC-2026-0595 (`a9610a1`), and the
three Win32 calls the marks brought added to the Windows 10 import list (`84612e8`). Scoop has it, winget's pull request
is microsoft/winget-pkgs#488505, and the Store's package was left for Partner Center's pages.*

### What it is

1. **Three marks on any file or folder.** A **label** (one named color that says a status), **tags** (several words,
   as before) and a **comment** (free text, 64 KB at most). Label, Tags and Comment are columns that sort, **Group by
   → Label** groups, a labeled row is washed in its color, a tile's name sits on a pill of it, and a row with a comment
   has a small note mark whose tooltip is the comment.
2. **One key.** `Alt+1`…`Alt+9` give the selection the first nine labels (again on items that all have it: takes it
   off), `Alt+0` takes the label off, `Alt+T` opens Edit tags… (which now suggests tags as you type) and `Alt+N` Edit
   comment…. The menu has a row of the labels' dots; **More → Marks** has *Tags…*, *Comment…*, *Copy marks*, *Paste
   marks* and *Clear marks*; the preview pane and Quick Look start with a **Marks** block; `Ctrl+K` has all of it.
   Every change is one **Undo**, tagging included (it had none).
3. **Labels are yours.** **Settings → Files → Labels** renames, recolors, adds (32 at most) and reorders them. They start
   as the seven color tags of 0.16, which become labels: a file tagged *Red* is shown with the label *Red*, `tag:red`
   still finds it, and its color tag moves into the label the next time its marks are written.
4. **Found at once.** `label:important` (`label:red;orange` for either), `tag:`, `comment:lacp`, `has:label`,
   `has:tag`, `has:comment`, `has:mark` and `!` for not, with XYplorer's `lbl:`, `tags:` and `cmt:`. They work in the
   open folder as you type (`tag:` there showed every row before), in instant search and on fd's drives. A plain word
   also finds comments and tags, under **In comments**; typos in a tag or label are forgiven; `tagged netplan`,
   `with comments` and `labeled pending from last week` are understood. The sidebar lists **Labels** above **Tags**, and a
   tag's menu renames it everywhere or takes it off every file. The pane's head shows the labels in the folder, and
   pressing one filters to it.

### Where the marks live

5. **In the file.** On NTFS and ReFS the three marks are streams of the file itself (`glint.label`, `glint.tags`,
   `glint.comment`, plain text), so renaming, moving or copying it with any program takes them along, into the Recycle
   Bin and back. PowerShell reads and writes them: `Set-Content bond0.conf -Stream glint.label -Value Important`
   (PowerShell moves the file's Date modified; Glint never does). 0.16's `glint.tags` is unchanged, so 0.16 and 0.17
   read each other's tags.
6. **On a drive without streams** (FAT, exFAT, a card, some NAS shares) they are in a hidden `.glint-marks` text file in
   the same folder, which travels with the folder; where neither can be written they are kept on this PC. Glint's own
   copies and moves write them where they belong on the other side, a folder's own marks included (Windows never
   copies a folder's streams); a tool that makes a new file in the old one's place (Make smaller, the PDF tools,
   Convert document…) gives it the old one's marks; a zip Glint makes holds a `.glint-marks` in each marked folder.
7. **Instant search reads them** in the pass that reads the names. A mark too big for the file's record is read through
   the file system by the service, by the file's reference and never by a path. The service never sends a comment's
   text, and holds the first 4 KB of every account's comments, so whoever may ask it can learn which files have a word
   in theirs ([instant-search.md](docs/instant-search.md)).

### XYplorer, agents and rules

8. **XYplorer's marks come in.** Glint finds `tag.dat` by itself (a suggestion offers it), or **Import marks…** takes any
   `tag.dat` or `XYplorerTag.dat`. One preview says where everything is (labels and what each becomes, extra columns,
   items here, moved since, waiting for a drive, not found), *Fill what is empty* is the default, and one **Undo** (or
   **Settings → Files → Marks → Take back the last import**) takes it back. Items on a drive that isn't there wait and
   take their marks when it is. **Keep following XYplorer** is off until you choose it. XYplorer's file is only read.
   **Export marks…** writes Glint's own file or an `XYplorerTag.dat` that XYplorer's `tagload` merges.
9. **AI agents** get `get_marks` and `set_marks` (asked first in a Windows dialog, written down, undone from Settings →
   AI agents). **Folder rules** can give a file a label, and **Batch rename** has `{label}` and `{tags}`.

### After the waves: what comes to you, and other keyboards

10. **Suggestions at their moment** (`glint-ui/src/suggestions`). Most people never open Settings, so what is worth
    turning on there comes as a notification when it would have helped, with one button that does it:
    - three renames in a row in one folder: **Batch rename** for those three;
    - a copy or a move of eight seconds and more: **Glint's copy queue**; a deletion taken back: **Ask before deleting**;
    - a folder of photos the Gallery doesn't show, or the Gallery opened while photos lie outside it (instant search
      says where: the folder that really holds them, never a drive or the profile): **Add to the Gallery**;
    - one kind of file moved from one folder to another for the third time: a **folder rule**, filled in and off
      until looked at; copies back and forth between two folders: **two panes**, with the other folder beside;
    - four files of a folder opened one after another: **Quick Look**; labels used with the names they came with:
      **Settings → Files → Labels**; a tag or a comment written: how to **find it again**, typed into the box for you;
    - Quick search's shortcut used while Glint quits with its window: **the tray**; Windows going dark or light while a
      theme is set by hand: **Follow Windows**; zoxide or an AI agent found on the PC: sharing history, connecting it.
    One on screen at a time, four minutes apart at least, three in a run of Glint, never over a dialog; in the
    background it waits for the window. Each has **Don't suggest this again**; one that came up is quiet for a week,
    and after three times with no press for good (but a full drive, which can be full again); one whose button was
    pressed is settled where pressing it settles it. A setting it turned on says so with **Undo**
    and where it lives. **Settings → Overview** lists them all, with the switch **Suggest these as I work**
    (`settings.tips`). No model and nothing sent: rules and counts.
11. **XYplorer in one press.** The suggestion's button brings `tag.dat` in without the preview (*Look first…* still
    opens it), with the same progress, Undo and *What happened*. An XYplorer that marks nothing yet (just installed) is
    no longer offered as "0 items": it is offered to **follow**, and following starts from there.
12. **Any keyboard** (`glint_model::keyboard`). A Chinese input method's full-width `：`, `；`, `，`, `！`, `“ ”`, `》`
    and `《` are read as the syntax they stand for, only where the syntax is (after a filter's key and in its value):
    `label：重要`, `size：》10mb`. A name with `：` in it is still searched as typed, and the box keeps what was typed.
    Tags part at `，`, `、` and Arabic's `،` as well as `,`. A label is found by its color's name in either language:
    `label:红色` is the label called Red, `label:red` the one a Chinese XYplorer's import named 红色.
13. **Fixes found on the way.** Settings opened at a section (Labels, from a menu or a suggestion) landed in the
    middle of the page above it: the page's header wasn't counted, and the page moved while it settled. The Labels
    rows' *Move up*, *Move down* and *Remove* are quiet signs with tooltips. Edit tags… had an empty band where no
    tag was used lately. Export marks…' two formats cut their own descriptions short. Edit comment… says that
    `Ctrl+Enter` saves. The import's notice had *What happened* floating in its middle.
14. **A sidebar that is all in sight** (`glint-ui/src/components/sidebar`). On a new PC it was some 29 rows of 36
    pixels, half of them the seven labels and things few reach for, and the drives were under the fold. Now:
    - **Quick access is tiles**, two to a line, nine places in five lines. The switch at its title (or **Settings →
      Files → Quick access as tiles**) makes it a list again, each place with the tree's arrow: a tile has no room to
      hang folders from.
    - **The drives come before what finds things**, so they are in sight; a wide rail says how much is free on each.
    - **The labels are one line of dots** after their title, **the tags are chips** after theirs, and a saved search
      is one line (what, then where).
    - **A section folds to its title** (a press on it, or its arrow; kept in the settings), and Remote has no title
      until there is a connection: the row that adds one is all there is of it.
    - **Rows are 30 pixels** (they were 36), as the tabs above them always were.
    - **The keyboard**: the arrows move by lines, Left and Right along a line of tiles, dots or chips; a title that
      folds is a stop, and Left and Right fold and open it.
    On this PC's default profile the whole rail is 600 pixels tall where it was about 1,040, in a window of 713.
    Nothing a row does changed: a row is still a row to the keyboard, a click, a drop, its menu and Hop, wherever
    it is drawn (`rows::lines`). Checked: `cargo test --workspace --no-fail-fast`, none failed (core 536, model 428,
    ui 2,900, index 160; the sidebar's 76, eight of them new, the others as they were but for the order, Remote's
    title and the keys along a line), clippy clean, and looked at in a debug copy in dark and light, as tiles and as
    a list, folded and open, with pinned folders, tags and saved searches. The tests caught one thing the look had
    not yet: a drive's line read the list's width while the list drew it, which panics as 0.16.0 did on a scroll;
    the width is read in `render` now. Then the end-to-end checks on the commit (`43a7bce`), no Glint running and
    the version at 0.17.0 for the run: **22 of 22**, `tabs-sidebar` (a place opened from the tiles) and `settings`
    among them.

### Not tried for real

- **Against a real XYplorer.** The reader was built from its manual and forum and read the one file a user sent for the
  issue; each storage mode, following a change and `tagload` of an export are untried.
- **On an exFAT drive and on a NAS.** This PC has neither; carrying marks between homes, `.glint-marks` written through
  `replace_contents` there, what Windows asks when streams would be lost, and whether a share that says it keeps
  streams does, are untried.
- Instant search's side with the service that came with this build: the installed 0.16.1 service knows no marks, so
  the index's answers were tested against the code and not seen in the window. ReFS drives keep marks in the file, but
  the index holds none there. The once-only move of 0.16's color tags runs only in a release build with the new
  service, so it has run in tests alone.

### Keys added

Alt+1 to Alt+9 (give a label), Alt+0 (no label), Alt+T (Edit tags…), Alt+N (Edit comment…).

### Checks

After wave 6, `cargo test --workspace --no-fail-fast` 3,984 passed, 0 failed, 62 ignored, and `cargo clippy
--workspace --all-targets -- -D warnings` clean (wave 5's review: 3,947, 0, 60). Looked at in a debug copy on a scratch profile in both themes
(`GLINT_SELFTEST=fly:`): the wash, the columns, the grid, the menu's dots, the Marks block, Edit comment…, the import's
preview and notice with issue 8's own file, Export marks…, Settings' Labels, Batch rename with `{label}`, the rule's
label. Wave 6 measured release builds ([the plan's 6.6](docs/gpui/marks/PLAN.md)): listing 100,000 files 26.8 ms, a
zip of 10,000 unmarked files 1,327 ms without reading marks and 1,379 ms with, `set_marks` over 1,000 files 1.7 s, a
`tag.dat` of 10,000 lines read in 21 ms. The marks of 200 unmarked rows took 11.6 ms on the PC that built it; on
another, the same test takes 3.1–3.4 ms on C: and 1.8–1.9 ms on D:, 1.0–1.4 ms over 0.16's read of the same files (the
plan wanted 1 ms at most: one more open a file). Looked at again there at 125% in a 900-pixel window: the grid's *Sort by*
bar was cut off at the pane's edge (Label, Tags, Comment and Size out of reach, a sort by one of them in force out of
sight); it now scrolls sideways and keeps the sort in force in view. Then the end-to-end checks, 22 of 22 with the version
at 0.17.0, `marks` the first time: it found that a search's results weren't read for their marks as they came, so
`label:red` left in the box hid what the search had found (fixed). A dev build still numbered 0.16.1 sends `label:` to the
installed 0.16.1 service, which finds nothing: the check passes once the version is bumped. After those fixes `cargo test
--workspace --no-fail-fast` 3,986 passed, 0 failed, 62 ignored, and clippy is clean.

After items 10 to 13: `cargo test --workspace --no-fail-fast` passed with none failed (core 536, model 428, ui 2,891, index
160), then 20 more in glint-ui for what was added last (2,911), and `cargo clippy --workspace --all-targets -- -D
warnings` is clean. Looked at in a debug copy (1,382 by 891 at 125%, dark and light): every suggestion's notification, the
one-press import of a made-up XYplorer of twelve items in English and in Chinese (*Marked 11 · 1 not found*, its labels
renamed 红色…), following one that marks nothing, `label：红色` typed into the box, the Gallery naming a real folder of
photos outside it, and Settings opening at Labels. Then the end-to-end checks on the commit (`77bd2b6`), no Glint
running and the version at 0.17.0 for the run: **22 of 22**, `marks` among them (the user's own Glint files untouched, the
clipboard put back). Not looked at: the list a search typed with a full-width colon answers in the live app (a dev
build numbered 0.16.1 sends it to the installed service, as above; the address bar's test holds what reaches the
explorer).

## 0.16.1: scrolling the sidebar no longer closes Glint

- 0.16.0 closed with no message (Windows' Application log: `Glint.exe`, exception `0xc0000409`, `FAST_FAIL_FATAL_APP_EXIT`)
  at the first wheel notch or touchpad swipe over the sidebar. `panics.log` had it: *RefCell already mutably borrowed*
  at `vendor/gpui-pre-0.3.12/src/elements/list.rs:798`, then *panic in a function that cannot unwind* (the window
  procedure), so Rust aborted. GPUI's list runs its scroll handler while it holds its own state (`StateInner::scroll`,
  from the wheel and from the wheel's glide), and the bottom edge's fade (`3c072ed`) read the list from that handler
  (`Sidebar::more_below`). The handler now defers the read to the end of the effect cycle, as gpui-component's
  `message_scroller` does; `scrolling_the_rail_moves_its_fade_and_never_panics` scrolls the rail both ways (`924c154`).

## In 0.16.0 (built 2026-11-22, released 2026-11-23)

*[Its release](https://github.com/shadijhade/glint/releases/tag/v0.16.0): tag `v0.16.0` at `702cb2c`, built and
signed on GitHub (run 45070565181). Glint.exe is 82,951,680 bytes (0.15.1: 85,088,768); the setup program 22.8 MB.
On the way out: Freeze order's key moved to Alt+F (`5eaad06`), every window test starts with Hop mode off
(`a8d5cb4`), and `url` went to 2.5.9 for RUSTSEC-2026-0592, which the release build's crates check stopped on
(`702cb2c`).*

*The user's list of eighteen points (speed and size first, then the interface: "do all the points below … any
other ui enhancement are welcome"), built the same day by twelve agents, each in a worktree of its own from
`a6738c0` (0.15.1). Every branch was rebased onto master, checked (its crates' tests and clippy) and merged; the
whole workspace was checked at the end (**Checks**). 36 commits on master, not pushed.*

### Speed and size

1. **Color surfing.** Color matches are ranked by a distance worked out once per photo, sorted with no allocation
   per comparison (the old sort lowercased two paths per comparison): 100,000 photos, 42,382 kept, 377–455 ms →
   46–51 ms in a debug build (`668ed45`). The wheel is one anti-aliased picture made once per lightness step
   (0.6 ms at 112 px, 3.2 ms at 256 px), not 576 squares a frame (`c2121bc`). A thumbnail's palette is taken
   after its tile has its picture, on a thread of its own (40 µs on the thumbnail thread → 0.4 µs), and photos
   without one are warmed on four threads that hand matches over as they come: 256 photos 1,630 → 532 ms
   (`3322ac1`, `5b5908e`).
2. **Photo World Map and the media cache.** The map draws from one shared set of photos, clustered once per
   camera: a pan or zoom frame over 5,000 photos 3.57 → 0.12 ms, over 50,000 43.6 → 1.13 ms (`d3b2c4e`,
   `32fe440`). `gallery-dates.json` and `gallery-geo.json` became one `gallery-meta.bin`, filled by one read
   per photo: 200,000 records save 633 → 64 ms, load 480 → 146 ms, 42.9 MB in two files → 19.3 MB in one; the
   old files are read once and removed (`616c2ee`). Cold scans read 64 KB headers through the I/O ring
   (Windows 10 keeps plain reads) and the map gets its pins about ten times a second while a scan runs
   (`b0f1881`); over 3,000 just-written photos the scan is no faster (the files were in the cache), and the
   Gallery still gets its dates at the end of a scan.
3. **Time Machine.** Shadow copies, Git and the journal are asked at once, restic beside them, and stops show as
   each source answers: first stops in a 20-commit repository 91.7 → 7.2 ms, all local stops 134 → 42 ms
   (`fda50e7`). The stops either side of the one shown are read ahead into a per-tab cache (16 stops, 60,000
   rows, trusted 60 s; backups aren't read ahead, each read starts restic), so arrow keys step without a read
   (`14ace20`).
4. **Listings.** `FileEntry` keeps its extension as an offset into its lowercased name (`ext()`), not a String:
   100,000 entries 290,000 → 200,000 allocations, 144 → 120 bytes an entry, 44.9 → 30.0 ms to fill them
   (`5bfb444`). An all-ASCII name is copied from its UTF-16 units: 100,000 names 7.7 → 3.7 ms, a 100,000-file
   folder 34.5 → 30.2 ms, System32 1.6 → 1.4 ms (release, `c60f51e`). The list's third point (a bigger
   directory read) was already there: `FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH`.
5. **Copy queue.** Small files are counted with atomics and each worker keeps its own list of what it made, so a
   file takes no shared lock: 50,000 4 KB files on 16 workers 13.9 → 12.2 s; the copy is bound by making files
   (and Defender), not locks (`0020964`). Block cloning: Windows 11's own copy (`CopyFileExW`) clones on ReFS and
   Dev Drive since its Moment 5 update, and nothing in the queue's calls stops it; what did stop a big clone onto
   a nearly full Dev Drive was the up-front "Not enough space there", which counted every byte as new. It's
   skipped where every source is on a volume that clones (`855de55`).
6. **Thumbnails ahead of the scroll.** The thumbnail service has a third, lowest lane for prefetches, behind
   every lookup and make, promoted when asked for for real and dropped per tab (`303a87f`); the grid asks for
   one screenful ahead in the direction it moves, up to 160 tiles, dropped when it turns back. Tiles still on
   their icon on the first frame after a scroll step: 8 → 0 (`bb19a18`).
7. **The instant search service waits for changes.** Each drive's journal is waited on (`FSCTL_READ_USN_JOURNAL`
   with `BytesToWaitFor` 1) instead of read every 250 ms: an idle service wakes no more (it was 4 times a second
   a drive), and a change reaches the index in about 0.1 ms plus a 5 ms settle that gathers a burst. A drive
   that can't be waited on polls as before; stopping cancels the wait (`CancelIoEx`, given up after 2 s so a
   stop or an update never hangs) (`3786c21`).
8. **Glint.exe 84.9 → 80.3 MB.** More crates nobody waits on are compiled for size (gitoxide, the SVG crates,
   rustls/ring, lopdf/pdf-extract, jaq, unrar…: the Git column, history and icons measured no slower), and
   glint-ui and glint-core get one codegen unit (2.7 MB of it; listing and sorting the same). Left at full
   speed: syntect (highlighting 6% slower at "s"), the archive crates (Defender's machine learning flagged the
   test programs built with them at "s" as malware, a false positive not worth 0.2 MB) and the terminal
   (`bd944d4`). `strip` changes nothing on this target (debug information is in the PDB). The release build
   takes about two minutes longer. With everything 0.16 adds, Glint.exe is 82,047,488 bytes (0.15.1:
   84,923,904); no dependency was added (`Cargo.lock` is unchanged).

### The interface

1. **Photo World Map.** Pins show their lead photo in a circle with a count; the drawer of a cluster is a strip of
   thumbnails, named "42 photos near Tokyo · Japan" from an offline table of about 800 cities and 190 countries
   (24 KB of source; city names show when zoomed in). Double-click a pin (or Enter on it) to fly in; a scrubber
   of years and months filters the pins, and Arcs draws dashed travel arcs between stops in time order
   (`0eb4837`, `73a2f62`, `bdb6fc6`).
2. **Color Wheel.** Lightness and Tolerance are sliders (arrows, Page Up/Down, Home/End; the mouse wheel over the
   wheel is lightness, Shift+wheel tolerance), `+` adds a second and third color with Any/All, and a photo tile
   shows its three colors as dots while the wheel is open: click one to aim the wheel at it (`96266c8`,
   `008147a`, `b01d8be`).
3. **Time Machine.** A timeline rail replaces the grid of buttons: ticks colored by source (Git blue, Windows
   Restore purple, backups amber, journal gray) under a sparkline of changes, a playhead that snaps to stops (a
   haptic detent at each), chips to show one source, and Space on a row shows that version in Quick Look without
   leaving (`14ace20`).
4. **Portals in the sidebar.** Live portals have a section of their own while any is mounted: archive, search,
   Git commit or backup icon, open on click, and Open, Copy path, Refresh and Unmount on the right-click. Unmount
   now removes only a folder right under `Glint portals` (a hand-edited portals.json could have pointed it
   anywhere) (`9fcaaa5`).
5. **Drag to transform, File Lego, rename by example.** Pictures or videos dragged out of a pane get a strip at
   the window's edge (As they are, Clean EXIF, WebP; Under 10 MB, Extract MP3): hover a choice or press Tab
   during the drag (`88b3884`; once a transform is chosen Glint's own panes refuse the drop). File Lego's dwell
   fills a ring, snaps a haptic when it arms and names the real output ("Mux subtitles → clip_muxed.mp4"); muxes
   were always written as "… (2).mp4" before, fixed (`55e9683`). Batch Rename's By example strikes deleted
   characters in red and shows inserted ones in green, and marks rows a second reading would name differently
   ("Edit this row to disambiguate") (`81d97ee`).
6. **Named workspaces.** Save the window's tabs, panes, split, views and scroll places under a name from the
   sidebar's Workspaces button or Ctrl+K, and switch, rename or delete them there; tabs already showing a wanted
   folder keep it, without reading it again (`10a506e`).
7. **Hop mode.** Alt+J puts letters on every tab, crumb, sidebar place and visible row or tile in both panes;
   typing one acts as a click there (Backspace takes a letter back, Escape cancels) (`15cbc39`, `d4e3485`: in
   the real window every row's letter had shown on the row below it).
8. **What's changing, and Freeze order.** The status bar's "Changing now" opens a live list of the files being
   written (size growing, time since the last write; click to select one). Freeze order (in that list, Alt+F
   or the palette) keeps the folder's rows where they are while files change: new ones go at the end, gone ones
   dim, sizes update in place; turning it off sorts once (`fdbcef6`, `9c88c98`).
9. **The basket's drawer.** The dock's basket pill (Alt+B) opens a drawer: thumbnails, sizes, remove and Clear,
   Batch rename, Compare (two), Zip, Move here and Copy here, and dragging any row takes the whole basket out
   (`13154fa`).
10. **Hover cards.** A background tab, a parent crumb or its arrow shows a card after 400 ms: the folder's name,
    path, counts, size and make-up band, from what's already known (`2de403f`, `ad6b5e2`).

### Polished before release (the same evening)

*The user's words: "a huge ux\ui polishing all around … remove useless icons from the header of each opened
directory and make them smart … reimagine the right click menu … sorted on how important they are and no right
click menu should take the whole page height". Nine agents, each in a worktree from `f3b0505`; then checked in the
real window (debug build, generated photos with places, a Git repository, a program, a PDF) and fixed where it fell
short. 34 commits on master with this note, not pushed.*

1. **A smart head.** Color Wheel, Photo map and Time Machine no longer sit on every folder as three icons that open
   empty panels. Labelled pills appear only when they would show something: **Map N** for photos with a place (GPS
   read off the UI thread, the first 400 pictures, kept per listing), **Colors** from 12 pictures (always in the
   Gallery), **History** when Git, a backup or a shadow copy has the folder (`0ac1952`). The kind chips are there on
   the first frame, the arrival plays only on a real change of place, and hidden items the list leaves out aren't
   counted (`527d810`). The search bar's buttons are the same pills and don't jump (`91b5fd8`).
2. **Right-click menus by importance.** Head, strip (Cut, Copy, Rename, Copy path, Share, Delete), what opens it,
   Suggested (at most two, never repeating a row above), the kind's own short section ("Picture": Rotate right,
   Rotate left, Set as desktop background, More picture tools ›), Move or copy to ›, Compress to ›, More ›
   (everything rare; nothing was lost), Properties, Show more options. A picture's top level went from 22 rows to
   12, a folder's from 19 to 9, a program's from 16 to 10; Run in Windows Sandbox only for programs, scripts and
   installers (`b1f2d1f`, `cebcc74`). The folder's own menu gained View ›, Sort by › and Group by ›.
3. **Menus that fit.** Submenus open beside the menu on hover (a safe triangle, the keys, mirrored, flipping at the
   window's edges) instead of replacing it. No menu is taller than three quarters of the window (600 px at most), and
   a file's menu shows whole in a normal window; near the bottom a menu opens upward; menus pop in from the pointer;
   tooltips never cover a menu (`d0b07e6`, `9b4fca8`, `c8b7d64`, `956aa74`). Every context menu has the one look:
   tabs, panes, portals, workspaces, the column header (Columns, then Sort by › and Group by ›), the terminal's
   shells, the text preview's encodings (`41058e4`, `bf57986`, `02952e1`, `e2255d1`, `19f41c4`).
4. **The dock** shows only buttons that can act here (no New folder or Paste in search results, Folder sizes only
   with subfolders, the pane buttons with two panes), its menus drop rows that can't apply, and its width eases
   instead of jumping (`6b79139`); the status bar's time fades in once per folder (`3c3d71c`).
5. **The rest.** The preview pane keeps the last file up until the next is ready (no empty frame between two files),
   puts rare header actions under "…" and keeps a picture's chips on one row (`dc9dd24`, `3f84269`, `761c1a2`). The
   sidebar fades at its bottom edge and shows "+" on hover or focus; the address bar's flyouts obey the height rule
   (`3c072ed`, `6d402e5`, `528ae73`). A selection's fill eases in, empty states are alike, This PC's cards fill the
   row with whole names (`bf57986`, `02952e1`). Enter runs a dialog's main button, never one that destroys, the
   palette's rows match the menus, Settings' rows line up (`1874d4a`, `b6a3877`, `e11ba91`, `f6fef2c`). The 0.16
   panels share one header, close with Esc and use icon buttons (`9cec9c6`, `385cd9b`).

Not seen in the real window: Enter in each dialog, the motion (the island's width easing, the preview's hold, the
fades), the flyouts' hover timing under a person's hand, History from a real shadow copy.

### A quieter look (later the same night)

*The user's words, with a screenshot of Downloads: "fix and polish the ui as it looks ugly and the gradients gets
clipped out of the frame instead of blending in and the selected item has the ai … start border and the inner window
looks floating instead of blending in, de slop the ui and make it more pro".*

What the screenshot showed, measured: the window's light was four lamps at the corners at up to 44 % strength; the
sheet hid 70 % of it and stood 8 px off the window's end and bottom edges, so the two lower lamps were seen only as
bright strips in that gap, cut by the frame, and the rail's bottom fade painted a dark band over one of them. The
current place had the accent's bar at its start edge and, after a click, the accent's ring round it.

1. **The sheet is part of the window.** It runs to the window's end and bottom edges; a hairline and one rounded
   corner where it meets the bar and the rail. No gap, no shadow, no gradient hairline on top.
2. **The light is a wash, not lamps.** One color (the place's), from the top of the window, fullest over the rail
   (16 %), gone before the foot: nothing of it ends at the frame. The sheet is a veil (3.4 % white in the dark, 58 %
   in the light) instead of a cover, so the wash runs on under it unbroken. The grid painted its own opaque
   background (since wave 4): removed, it lies on the sheet as the list does.
3. **A state is one even fill.** No accent bar at the start edge (the rail's place and tab, the list's current row),
   no stroke round the current place, no gradient across a selected row (`theme::selected_fill` is flat). Where you
   are in the rail is `t.current`, a gray a step past hover, with the name a weight heavier. A row of the rail shows
   the ring only after a key was pressed, never after a click.
4. **The accent only for what is chosen or on.** The glint (pink to fuchsia) is left for what is loading. The
   rail's drive bars and a row's size bar are gray; a drive's own card keeps the accent, flat. The head's band is a
   2 px hairline at 60 %; the address field's underline while typing and Settings' gradient tile are gone; "shown
   in 47 ms" is no longer pink.
5. **Tighter corners.** Rows 8 → 6 px, the sheet's corner, the address flyouts, menus and the island 14 → 10 px.

Seen in the real window (debug build beside the user's, its own profile): dark and light, Mica and solid, the list,
the grid, columns, two panes, This PC, the app's menu and a file's menu. Not seen: right to left (the rounded corner
and the hairline swap sides in code), Windows' accent color in place of the pink.

### Not tried for real

- The journal wait on a real volume (it needs the service, elevated): tested against a fake journal through the
  same code. Check after the service updates itself: an idle service doesn't wake, a new file is found at once,
  and stopping or updating the service is quick.
- A real drag out with the transform strip (does Glint repaint during Windows' drag loop, is a quick Tab caught),
  a real File Lego mux, block cloning on ReFS (no ReFS volume here), the I/O ring on a cold disk, haptics.
- Seen in the real window (debug build, generated photos): hop mode, the hover card on a crumb, the Color Wheel
  and a chord's matches, Time Machine's rail and chips on this repository, the basket drawer, the photo map's
  pins, scrubber, arcs and "5 photos near Tokyo · Japan".

### Keys added

Alt+J hop mode, Alt+B the basket's drawer, Alt+F Freeze order (it was Ctrl+Alt+F until the release: that is
Quick search's shortcut, taken from every app, so the key never reached the window).

### Checks

`cargo test --workspace` (core 385, model 348, ui 2,544, index 103) and `cargo clippy --workspace --all-targets
-- -D warnings` clean at `32fe440`; the release build's imports all exist on Windows 10 1809
(`scripts/check-imports.mjs`); e2e (`tests\e2e\run.ps1`) 21/21 with the debug build of `32fe440`.

After the polish: `cargo test --workspace --no-fail-fast` (core 386, model 348, ui 2,630, index 103) and `cargo
clippy --workspace --all-targets -- -D warnings` clean at `16ebac1`. The polish adds no Windows API calls and no
dependencies (the release build's import check wasn't run again). e2e 21/21 with the debug build of `f55dfa4` (its
`smart-menu` check follows the new order, `16ebac1`).

After the quieter look: `cargo clippy --workspace --all-targets -- -D warnings` clean and `cargo test --workspace
--no-fail-fast` (core 386, model 348, ui 2,635, index 103). Eight of the hover cards' tests (`peek_card`, the tab
strip's card) failed in one run and passed in the next: three of Hop mode's tests end with the mode on, its switch
is the process's, and cards stay away while it is on, so whichever card test ran next found none. Every window
test now starts with the mode off (`testing::init`). No Windows API calls and no dependencies added. e2e 21/21 with the debug
build of that working tree (23:45, 2026-11-22).

## 0.15.1: updates that finish, and Space while renaming

- People couldn't update to 0.15.0: the setup stopped at "Error opening file for writing: …\Glint\rg.exe". An rg (or fd)
  that a closed or killed Glint had left running (a search still going, or one an AI agent's `Glint.exe mcp` asked for)
  held the file. The in-app update stopped the app's own searches first, but nothing ended processes Glint no longer
  tracked. Now the setup and the uninstaller close exactly the processes using `$INSTDIR\rg.exe` and `fd.exe` with
  Restart Manager, after Glint itself is closed (`GLINT_CLOSE_TOOLS` in installer/hooks.nsh); another program's rg.exe
  stays. And Glint starts rg and fd in a job that ends them when it ends (`proc::spawn_tool`); a winget install is
  left out of it, so closing Glint doesn't cut one off.
- Space in the rename box opened Quick Look: the list binds Space, and the box is inside the list. The box's context
  binds Space and Shift+Delete to `NoAction`, which outranks the list's bindings, so Space types a space and
  Shift+Delete deletes no file (`file_list/rename.rs`, for the list's and the grid's boxes).

## In 0.15.0

Built on 2026-11-21, in the five commits from `d792e5b` to `77e7d69`: Time Machine (`Ctrl+Shift+H`), live portals
(Cloud Files folders for archives, saved searches, Git commits and restic snapshots), `glint://` permalinks, rename by
example, File Lego, the Photo World Map (`Ctrl+Shift+G`), the Color Wheel and Find similar colors, frames and clips
from a video's tile in the grid, Drag clean and touchpad haptics; `docs/features.md` describes each. Fixed before the release:

- The `glint://` handler was written by every copy at every start, so a dev build or an e2e run took the links from
  the installed Glint. Only the copy the setup program or Scoop installed points it at itself now, writing only when
  it runs something else; other copies only remove a handler whose program is gone (`permalinks::claim_protocol`).
- The Microsoft Store's copy couldn't take the links at all (Windows keeps a packaged copy's registry writes to
  itself): its manifest declares `glint://` now.
- Uninstalling left the handler and the portals' sync roots behind: the setup's and Scoop's uninstallers take them
  out (`Glint.exe portals remove` also takes out the remotes shown in File Explorer).
- A link's saved path was looked at wherever it pointed, so a link could make Windows sign in to any server
  (`\\server\share`). Links are only made for, and only ever open, files on this PC's own drives.

## In 0.14.0

### Back and Forward that remember

The user's report: a search ("Personal Stuff") listed files and folders, a folder opened from the results, and
Backspace could not get back to them (history held folders only, and opening anything ended the search). Back and
Forward now walk places, each kept as it was left (`crates/glint-ui/src/explorer/history.rs`):

- A search is a place after the folder it began in. Back from a folder opened in its results brings them back at
  once, without searching again, the folder selected and the list where it was; Back once more is the folder as it
  was before the search. Typing on refines the same place; Escape, the clear button, emptying the box or Up leave
  it for its folder, and Forward brings it back.
- Every place keeps its selection, its focused item and its scroll (a tab used to keep one scroll place).
- Only the six searches nearest the tab keep their results (200,000 rows at most together); an older one, or one
  whose results were still coming in when it was left, runs again. Results deleted, moved or renamed meanwhile are
  taken out of what comes back (a quiet look in the background).
- A folder gone from the disk is skipped with "is no longer available"; one that fails for another reason stays.
- Backspace goes up when there is nothing to go back to (`BackOrUp`). Held down, it keeps stepping back.
- The toolbar's arrows say where they go ("Back to the results for “Personal Stuff”"), read that folder ahead
  while the pointer is on them, and a right-click lists the places to jump straight to.

Tests: `explorer/history_tests.rs`, the address bar's and the window's (`Backspace`, the mouse's side buttons),
and `tests/e2e`'s new `search-history` check with the real keyboard and side buttons.

### Faster where it is felt

- Sizes and counts in the list, the tiles and the status bar are written without asking Windows: the digits are
  grouped here, with the separator and the grouping Windows gives the locale (read once a run), and toFixed and
  Math.round are worked out on integers (`crates/glint-model/src/format.rs`). A size took eight calls into Windows
  and eleven allocations, on every row of every frame: 38 ns now against 552 ns (`format::tests::sizes_cost_little`),
  the same text over 200,000 sizes, and the grouping the same as Windows' own in seven locales. Region settings'
  "123456789" (no grouping) is honoured now; a change to the region settings shows from Glint's next start.
- A folder's hover card gets its counts from the instant search service with the folder's sizes: the service
  counts what is below it (`Tree::count_below`: one pass over the drive's names) and sends two numbers. Before, the
  app searched the folder for its files and the service sent up to 20,000 paths to be thrown away: 14 to 21 ms a
  hover on this PC, and no count at all for a folder with more files (`C:\Repos`, `System32`). Now every folder gets
  its count of files, and of folders too: 0.3 ms for a folder of 10,000 files and 0.9 ms for a whole 2.2-million-file
  drive in the service's code (`tree::tests::counting_below_a_folder_costs_little`). A service from before answers
  without counts, and the hover searches as it did until the service updates itself.
- A search with a typo (close matches) takes half the service's time or less: each name is looked at in one pass
  without branches for its letters and where its words start, ASCII names are read where they are rather than laid
  out, and an edit table stops at the first column past the typos allowed (`crates/shadui-index/src/query.rs`). Over
  this PC's 3.35 million real names: `kernal32` 13.6 → 6.2 ms, `reprot` 18 → 7.6 ms, `screenshto` 23.6 → 10.7 ms,
  the same matches. A search with a typo first looks as typed (about 3 ms here), and the service's own figures
  will only show once it updates itself.

Not tried end to end: the hover's counts and the close matches through the installed service, which stays 0.13.2
until a release updates it (the tests run the service's code in a test and through a pipe of its own).

---

## Planned for 0.14

*Done 2026-11-18, not pushed when written. Ten agents worked the items in worktrees of their own, from `5630c16`; each branch was rebased onto master, checked (its crates' tests and clippy) and merged one at a time, and the whole workspace was checked at the end (see **Final verification**). Each item below ends with **Done** and its commit.*

### Ground rules

1. **Measure first, keep the numbers.** Run M0 before any change and again after each part; record the numbers in the Results table. A performance change that is not measurably faster (or smaller) and is not a fix or removal is reverted.
2. **Where to work.** Branch `master` (`git pull` first). Commit locally only when asked; **never push** without the user's word.
3. **Build folder.** `$env:CARGO_TARGET_DIR = "E:/glint-gpui-target/ui"`.
4. **The user's own Glint.** Before launching anything, check for a running `Glint*` process. Leave the user's running copy alone: do not kill it, overwrite its exe, or run the e2e harness while it is running.
5. **Builds to use.** Size: `cargo build --release -p glint -p shadui-index`. Speed: `cargo build --release -p glint --config "profile.release.package.glint-ui.debug-assertions=true"`. e2e: `cargo build -p glint`.
6. **Checks before saying an item is done.** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, and `npm run check-imports`. Run `powershell -STA -File tests\e2e\run.ps1` after each part (ask before taking the mouse/keyboard).
7. **Rules of the code.** Follow [docs/gpui/STANDARDS.md](docs/gpui/STANDARDS.md) (§3 Rust rules, §5 UI rules, §7 Done means).

### M0. Baseline

| Measure | How | 0.13.2 Baseline | After A | After B | After D | Final (0.14) |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| `Glint.exe`, `.text` / `.rdata` | file sizes & `python scripts/dev/size-by-crate.py --json <file>` | 93,770,752 B; 68.4 / 21.8 MB (0.12: 93,284,352 B; 68.2 / 21.6 MB) | — | — | — | **82,779,648 B; 60.5 / 19.1 MB** (−11.7%) |
| Crates compiled | `cargo tree --locked --offline -p glint --target x86_64-pc-windows-msvc -e normal --prefix none \| sort -u \| wc -l` | 1,015 (0.12: 724) | — | — | — | **836** (−179) |
| Cold start | `npm run bench -- --start` | not measured: it starts the release `Glint.exe`, which shares the installed copy's single-instance name, Run key and taskbar jump list, so it waits for 0.14 to be installed (or another PC) | — | — | — | |
| Engine: list System32 (all) | `npm run bench` | 1.35 ms (warm) | — | — | — | 1.36 ms: the same (nothing in 0.14 lists folders differently; a cold first run gave 1.72, which wasn't 0.13.2's) |
| Engine: index search & typo search | `npm run bench` | 8.1 / 27–36 ms | — | — | — | the same: both runs ask the installed 0.13.2 service, so the service's gains (the hover's counts, close matches, all cores) show once it updates. In its code: typo `kernal32` 13.6 → 6.2 ms over 3.35 million names |
| Engine: copy 5000 small files (1 / 16) | `npm run bench -- --copy 5000` | 1,443–1,448 / 902–959 ms | — | — | — | 1,384–1,471 / 964–1,114 ms: within noise (two rounds each, turn about; 0.14 didn't touch copying) |
| Frame spans (scroll, folder hover, zoom) | `GLINT_FRAME_LOG=1` | not measured (the release build on screen: see Cold start) | — | — | — | per row, in tests: a size 552 → 38 ns, a row's dates 107 → 14 ns, read-ahead peeks 114 → 1 ns a frame; 20,000 rows sort in 2.94 ms, not 4.35 |
| Idle CPU over 120 s (shown / tray) | `TotalProcessorTime` delta | not measured (the release build running two minutes: see Cold start) | — | — | — | |

### Part A. Performance: Remaining Hot Paths, Index Service, & Startup

*(Builds on `49574e3`, `706cc60`, and `5630c16` already in **In the next release**.)*

| # | What | Where | Problem | Change | Done when |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **A1** | Zero-allocation thumbnail lookup per frame | `glint-ui/src/components/file_grid/thumbnail.rs:163, 910-916`, `glint-model/src/entries.rs:32` | `Thumbnails::look_at` allocates `entry.path.to_lowercase()` every frame for every visible tile/row even though `FileEntry` already holds `pub path_lower: String`. | Pass `path_lower: &'a str` in `EntryRef` and look up by `&str` without allocating a `String` per frame. | Zero `to_lowercase()` allocations in `Thumbnails::look_at` per frame. |
| **A2** | Lazy row tooltips & `SharedString` date cache in `FileList` | `glint-ui/src/components/file_list/cells.rs:374-387, 523-535, 930`, `glint-model/src/format.rs:98-170, 352-375` | `FileList::row_element` eagerly computes `format_full_date`, `name_tooltip`, `type_label` (even when Type column is hidden), and `tags.to_vec()` on every visible row every frame; `DATE_CACHE` locks a `Mutex`, calls `SystemTime::now()`, and `.clone()`s a heap `String` twice per row; `short_picture` isn't cached across clears. | Move `format_full_date` and `name_tooltip` into `.tooltip(...)` closures; borrow `&[String]` for tags; store `Arc<str>` in `DateCache` and cache `short_picture`. | Eliminates 2 mutex locks, 1 syscall, and ~12 heap allocations per visible row per frame. |
| **A3** | Skip hidden `peek_cell` subtrees & per-frame `Peeks::read` allocations | `glint-ui/src/components/file_list/mod.rs:351-372`, `cells.rs:485-494`, `explorer/nav.rs:931-944` | `Peeks::read` allocates and sorts 3 `Vec`s every frame, and `row_element` builds the full `peek_cell` GPUI subtree with `.opacity(0.)` for up to 8 prefetched folders even when `show_peek == false`. | Guard `Peeks::read` by a prefetch generation counter and only build `peek_cell` when `show_peek` is true. | Zero `Peeks::read` allocations when prefetch cache hasn't changed; zero hidden `peek_cell` subtrees built. |
| **A4** | Filter `NAMESPACE_DOS` before allocating UTF-16 strings during MFT scan | `shadui-index/src/record.rs:262-266, 311-315` | `record::file_name` allocates `Vec<u16>` and `String` on the heap before checking `value[0x41] != NAMESPACE_DOS`, causing ~6–9M wasted heap allocations per drive scan. | Check `value.get(0x41) == Some(&NAMESPACE_DOS)` at the top of `file_name` and decode UTF-16 using a stack `[u16; 255]` buffer. | MFT scan tests pass with zero heap allocations for ignored DOS 8.3 aliases. |
| **A5** | Restrict `twins::same_size` to single-item selection/hover | `glint-core/src/aware/twins.rs:48-94`, `glint-ui/src/aware/mod.rs:437` | Selecting up to 200 files (`Ctrl+A`) sets `want.twins = true` on all 200, firing up to 400 pipe calls (`status()` + full-volume scan) and saturating `MAX_CLIENTS = 16`. | Only set `want.twins = true` when probing a single item (`entries.len() == 1`), and reuse cached drive roots instead of calling `client::status()` on every `twins` check. | Bulk selection (`Ctrl+A` in large folders) triggers zero `twins` pipe requests. |
| **A6** | Precomputed 32-byte natural sort keys in `explorer/rows.rs` & background threshold | `glint-ui/src/explorer/model.rs:30`, `rows.rs:451-456`, `glint-model/src/entries.rs:275-343` | `ROWS_IN_BACKGROUND = 20_000` runs non-plain sort/group/filter synchronously on the UI thread using uncached `natural_compare`. | Lower `ROWS_IN_BACKGROUND` to `5_000` and use the 32-byte `[u64; 4]` natural sort head key (`glint-core/src/fsx.rs:456-516`) in `rows.rs`. | Sorting/filtering 5k–20k item folders never stalls the UI thread. |
| **A7** | Cold start single-pass `state.json` parse, Git `ours == theirs` short-circuit, & `aware::file::pages` thread | `glint-ui/src/persistence.rs:156-160`, `glint-core/src/git.rs:211-213`, `glint-core/src/aware/file.rs:141` | `persistence::load_from` parses `state.json` into `serde_json::Value` twice (`is_saved_state` then `load_persisted`); `git::view_in` runs two `rev_walk` traversals even when `ours == theirs`; `aware::file::pages` spawns a redundant OS thread per PDF even though `glint-aware` pool threads already run in COM MTA. | Parse `state.json` once; skip `rev_walk` when `ours == theirs`; run `PdfDocument::LoadFromFileAsync` directly on the `glint-aware` MTA thread. | Faster cold start, zero redundant rev walks on synced branches, no per-PDF OS thread spawns. |

**Done:**

- **A1** `8d69414`. `FileEntry` has no `path_lower` (the brief's premise): the lookup lowers the path into a buffer the thread keeps and finds the picture by a borrowed key, so a frame allocates nothing; the key and the lookup share one lowering.
- **A2** `855f017`. The full date and the name tooltip are made when the tooltip shows, tags are borrowed, the Type column's text only while it's shown; dates come from a per-thread cache of shared text (a row's date 107 → 14 ns, no allocation, no lock), the short-date picture is read once a run.
- **A3** `4665135`. `Peeks::read` is kept until a prefetch generation moves (a frame 114 → 1 ns, no allocation); a folder's words are built only for the row under the pointer or the keyboard (there was no fade, so nothing else needed the hidden cell). Scrolled under a still pointer, they show at the next mouse move.
- **A4** `ae4baac`. A DOS alias is skipped before anything is decoded, a name goes through a 255-unit stack buffer, every read is bounds-checked: 260 → 118–132 ns a record (300,000 records), an alias with no allocation.
- **A5** `56b9dbf`. Identical copies are asked only for one item, and the ready drives are kept 20 s, not asked of `status()` each time.
- **A6** `c486938`. The key moved to `glint-model/src/natural.rs` (no copy left in `fsx.rs`); rows sort by keys built once a sort, from 5,000 rows off the UI thread: 20,072 rows 4.35 → 2.94 ms. The old key could misorder digit runs over 254 digits: fixed.
- **A7** `2018fd7`. `state.json` is parsed once (a 348 KB state: 47.1 → 36.8 ms in a debug build), synced branches skip the two Git walks, and a PDF's pages are counted on the lane's own thread where it is in the MTA (a scoped thread stays for any other).
- **Also:** `9ba1465`, the index service's search threads get every core (high QoS: as a service Windows kept them on the 12 efficient ones, 8.7 ms where all cores take 4.1); a probe as a temporary service couldn't show the change, so it shows once the installed service updates.

### Part B. UX Improvements & 0.13 Polish

- **B1. Promote in-flight `Lane::Quiet` probes when right-clicking** (`glint-ui/src/aware/mod.rs:288-319`, `aware/model.rs:25-46`):
  When `look_now` is called on an item whose `look_ahead` (`Lane::Quiet`) is still in flight (`kept.asked` set, `kept.probed` field still `None`), re-request the unanswered fields on `Lane::Now` and return `waiting = true` so `MenuHost::open_list_menu` waits up to `LOOK_FIRST` (`40 ms`).
  **Done** `2191187`. A right-click asks again, on the menu's lane, what hovering hasn't heard back yet, and waits for it like an item menu.
- **B2. Wake `open_list_menu` immediately when probes finish & remove the 16 ms sleep in `Aware::listen`** (`glint-ui/src/aware/mod.rs:333-355`, `components/menus/mod.rs:455-468`):
  Coalesce `Aware::listen` notifications without sleeping a blind 16 ms after the first fast `Origin` answer, and open the pending right-click menu as soon as the item's requested `Want` bits are answered instead of always waiting the full 40 ms timer.
  **Done** `13d558bb`. No blind 16 ms: answers are kept as they come and only the redraw is throttled; a waiting menu opens the moment its answers are in (an `Answered` event), with 40 ms still the most it waits.
- **B3. Enable `Aware` hover probing on all files in Grid View** (`glint-ui/src/components/file_grid/tile.rs:435-437`):
  Remove the `.when(entry.dir || super::scrub::scrubbable(entry), ...)` gate on `.on_hover` so non-video files (`.exe`, `.zip`, `.pdf`, `.png`) in Grid View call `aware::rest_on` just as `FileList` rows do.
  **Done** `e9e9aeb`. Every tile takes part in Aware's resting; folders still read ahead and videos still scrub.
- **B4. Pass `Aware` folder facts to the background context menu** (`glint-ui/src/components/menus/mod.rs:389, 578`):
  Pass the open folder's `Probed` facts into `open_background_menu` and `refresh_open` so right-clicking empty space inside a project folder shows its total size and suggests **Clean up `<size>` of build output…**.
  **Done** `e6a6943`. The open folder's menu probes the folder and waits like an item menu: its head says `N items · size`, and a project folder suggests Clean up with what build output is inside. (`refresh_open` doesn't exist: `follow_answers` carries it.)
- **B5. Clipboard picture/text paste discoverability & Undo** (`glint-ui/src/components/command_bar/smart.rs:19-57`, `explorer/clipboard.rs:251-271`):
  Show **Paste picture** on the dock button when `Clip::Picture` is on the clipboard, explain both `Clip::Picture` and `Clip::Text` in `paste_tip` (*"Paste picture as a new PNG file (Ctrl+V)"* / *"Paste text as a new .txt file (Ctrl+V)"*), and push `UndoAction::Recycle(vec![created])` in `paste_as_file` so `Ctrl+Z` undoes a pasted screenshot or text file.
  **Done** `d8be6e2`. The dock says Paste picture for a picture; text gets the tooltip only (a Paste text button would show on every copy). Undo of a pasted file already worked; it has a test now.
- **B6. Grid View parity: `Shift`+right-click, tag dots, keyboard folder prefetch, and drop target validation** (`glint-ui/src/components/file_grid/mod.rs:532-542, 851-879, 1051-1070`, `tile.rs:318-339`, `workspace.rs:1043-1045`):
  Forward `event.modifiers.shift` in `FileGridEvent::ItemMenu` so `Shift`+right-click opens Windows' native menu; render `self.tags` color dots on tiles; call `explorer.prefetch_folder` on keyboard focus in `FileGrid::move_by`; reject drops onto Gallery, Recent, and Remote paths in `FileGrid::drop_under`.
  **Done** `cdeb57a`. The brief's `ItemMenu`, `move_by` and `self.tags` don't exist; the same done in the grid's own terms: Shift+right-click opens Windows' menu, tiles draw tag dots, keyboard focus reads a folder ahead after 80 ms, and drops onto the Gallery, Recent and remote places are refused. (The grid doesn't read off-screen tags for the Tags submenu, as the list does.)
- **B7. Content search matching lines: jump to line & expand** (`glint-ui/src/components/file_list/cells.rs:149-267`):
  Clicking a matched line under a file scrolls the Preview Pane / Quick Look to `hit.line` (and double-clicking opens the configured editor at `{path}:{line}`); clicking `"N more matching lines"` (`cells.rs:256`) expands the remaining matches inline.
  **Done** `2a1af7e`. A click already scrolled the preview to the line; a double-click opens the editor at the line in each editor's own syntax (the VS Code family, Notepad++, Sublime, Zed, Helix, Vim, JetBrains; another gets the file alone), and "N more" lists the rest inline.
- **B8. Inline rename: `Tab` / `Shift+Tab` to next/previous item & compound extensions** (`glint-ui/src/components/file_list/rename.rs:17-22, 116-137`, `file_grid/rename.rs:207-220`, `dialogs/batch_rename.rs:285-293`):
  Pressing `Tab` / `Shift+Tab` while renaming commits the current rename and begins inline rename on the next/previous item; stem selection strips compound archive extensions (`.tar.gz`, `.tar.zst`, `.tar.xz`, `.tar.bz2`, `.tar.lz4`).
  **Done** `8baa524`. In the list and the grid, in the order shown; a failed rename stays with its error; `glint_model::paths::extension_start` is the one rule for the box and batch rename.
- **B9. Address bar `~` / `%ENV%` autocomplete & keyboard navigation in search hints** (`glint-ui/src/components/address_bar/completion.rs:28-61`, `find.rs:444-567`):
  Expand `~` and `%ENV%` prefixes in `completion::split_typed`; allow `Down`/`Up` arrow keys to step through Recent Searches and filter chips when the empty search box's hints popup is open, with an `×` button to remove a recent search.
  **Done** `bd986b8`. An unknown `%NAME%` stays as typed; Enter takes the hint, Delete or × forgets a recent search; the current hint is marked selected for screen readers.
- **B10. Terminal idle cursor blink & file drop** (`glint-ui/src/components/terminal/view.rs:452-466`, `terminal/mod.rs:520`):
  Stop the 530 ms cursor-blink timer after 10 s of inactivity (and when `self.exited` is true or Windows animations are off) to keep idle CPU at 0%; accept file/folder drops onto the terminal by writing their quoted paths to the PTY; update the terminal tab title on `TerminalEvent::Title`.
  **Done** `2c80304`, and `7b6cff9` took out the playback code the `.cast` player left. The blink rests after 10 s, or at once when the shell has ended or animations are off; dropped paths are pasted quoted for the shell (no Enter); the tab shows a task, then the program's title, then the folder.
- **B11. Columns view (`Ctrl+Shift+9`) & flaky test cleanup** (`glint-ui/src/components/columns_view/mod.rs:424-505, 645-664`, **Left over from 0.13**):
  Add right-click menu, middle-click new tab, and drag-and-drop to the `Parent` and `Next` columns, and render thumbnail + `Aware` `Probed` facts in the `Next` file inspector; fix the three intermittently flaky tests noted below (`dialogs::timeline::tests::another_time_window_scans_again_and_forgets_the_subfolder`, `core_bridge::tests::jobs_posted_from_any_thread_run_on_the_ui_thread_from_the_message_pump`, and `core_bridge::tests::a_panicking_job_does_not_take_the_ui_down`).
  **Done** `3a623a2` (the columns: menus through `open_entries_menu`, middle-click, drag out and drops, the inspector with the shared thumbnail and the head's facts) and `264739d`: the three, and This PC's arrow-key test, failed for causes found in the code (a message window replaced by the next test's, drive health answering at random, a race in the timeline's fake) and now wait on what they mean; twenty busy runs passed. `70a9ff3` did the same for X-Ray's partial-hash test, which failed once in the final run.

### Part C. New Features

- **C1. Flat View / Branch View (`Ctrl+B`)**:
  Toggle flattening all files in the current folder and its subfolders into a single live list or grid (`Ctrl+B` in `actions.rs` and `Ctrl+K`), streaming from `shadui-index` in milliseconds (or `fd` fallback) with the existing `Track::Where` (`Folder`) column (`file_list/header.rs:34-43`). Supports sorting an entire subtree by size/date, viewing all nested photos in Grid View, and batch-renaming across subfolders.
  **Done** `0e4d275`. A search with no words through the same run, stream and rows (the index answers `files:`, fd where it can't), so the Folder column, sorting, the grid and batch rename work as on results; a place in Back and Forward; the toolbar's toggle and the View menu besides Ctrl+B and Ctrl+K. Capped at Settings' result limit (50,000 by default, "Show all"); not live (F5 lists again); not offered on This PC, Recent, the Gallery, the Recycle Bin, remote places or inside archives.
- **C2. Automatic Clipboard Checksum Match & `.sha256` Sidecar Verification in `Aware`**:
  Detect 32/40/64-hex-character hashes on the clipboard in `glint-ui/src/aware/clip.rs`; when a single file is hovered or selected, verify its hash on `Lane::Quiet` and display **`Matches copied SHA-256`** (or warning **`Checksum mismatch`**) in the menu head (`menus/head.rs`) and dock. When right-clicking a `.sha256` / `.sha512` / `.md5` / `.b3sum` file, offer **Check files in this folder** under **Suggested**.
  **Done** `fc91693` and `82f0544`. SHA-256, SHA-512 and BLAKE3 with the crates Glint has, MD5 and SHA-1 through Windows' CNG (its six functions added to the Windows 10 list); one pass a file, 512 MiB at most for a file the pointer rests on. A match shows in the head and the dock; a mismatch only in a single file's right-click head ("Doesn't match the copied SHA-256"), so a hash on the clipboard doesn't flag every file passed over. Check files in this folder for `.sha256`, `.sha512`, `.sha1`, `.md5`, `.b3sum` and their `sum` forms.
- **C3. "Already extracted" Archive Awareness** (`glint-model/src/aware/rank.rs`, `glint-core/src/aware/probe.rs`):
  When probing an archive (`foo.zip`) whose extracted target folder (`foo\`) already exists beside it, show **`Already extracted`** in the menu head and rank **Open extracted folder** ahead of **Extract here**.
  **Done** `dfc1efb`. The badge opens the folder; Open extracted folder ranks 9.2, Extract here 9.0.
- **C4. Paste as Link / Directory Junction / Hardlink**:
  When `Clip::Files` is on the clipboard, add **Paste as link** in the folder menu (`menus/folder.rs`) and `Ctrl+K`: creates a Directory Junction (`mklink /J`, no admin rights needed) for folders, a hardlink (or block clone on ReFS/Dev Drive) for same-drive files, or a `.lnk` shortcut across drives, with `Ctrl+Z` Undo.
  **Done** `750c9e9` and `dce6751`. Junctions through `mklink /J`, hardlinks, `.lnk` across drives, and a block clone (reflink-copy, as Find duplicates' Clone) on a drive that keeps no hardlinks but clones blocks. Undo removes a link and nothing it points to (a test undoes a junction to a folder of files and finds every file there); a clone, a file of its own, goes to the Recycle Bin. Untried here: a real ReFS clone.
- **C5. Transient Drop Basket for Cross-Folder Gathering**:
  A zero-idle-cost **Basket** pill in the dock that appears only when items are dragged to the dock's basket target or added via `Ctrl+Shift+B` (*Add to basket*), holding files across folder navigation to **Move here**, **Copy here**, **Zip**, or **Batch rename** in one step.
  **Done** `6a18321`. Ctrl+Shift+B, the palette or a drop on the dock; the pill exists only while it holds something, its menu has Move here, Copy here, Zip, Batch rename, the list and Empty; moved or deleted items drop out when it is used.
- **C6. Separate Data Folder for the Microsoft Store Package**:
  In `crates/glint-core/src/host.rs`, select `com.shado.glint.store` when running as a packaged MSIX app so Store and installer copies on the same PC keep separate data folders as documented in [docs/releasing.md](docs/releasing.md#as-an-msix-package).
  **Done** `40dd0ab`. `host::identifier()` already chose `com.shado.glint.store` when packaged: what was missing is filling the Store's folders once from the shared ones (no overwrite, links and anything over 256 MB skipped, the shared folders only read), so Store users keep their settings. Untried inside a real package.

### Part D. Removals & Simplifications

Each removal is an isolated commit (delete the code, its tests, its menu/palette/Settings entries, its MCP tools, its docs, and sweep any leftover user downloads in `retired::sweep()` where applicable):

| # | Candidate | Exact Files & Line Counts | Binary / Crate Savings | Change & Trade-off |
| :--- | :--- | :--- | :--- | :--- |
| **D1** | **E-Book Reader + Kindle MOBI + TTS "Read Aloud"** | `preview/book.rs` (4,426), `preview/mobi.rs` (3,505), `platform/speech.rs` (594), `book_places.rs` (495) — **9,020 lines** | Drops WinRT Speech/MediaPlayback imports & large `glint_ui` `.text` slice | Drop untested reverse-engineered Kindle `mobi.rs`, WinRT TTS `speech.rs`, `book_places.rs`, and either drop `book.rs` (falling back to `Body::Shell`) or keep a minimal EPUB viewer. |
| **D2** | **Custom CPU 3D Engine** | `preview/model3d*.rs` (5 files: 948 + 1,111 + 801 + 715 + 322) — **3,897 lines** | Large `glint_ui` `.text` reduction | Drop the software triangle rasterizer and glTF/3MF/STL/OBJ/PLY scene loader; fall back to `Body::Shell`. |
| **D3** | **Custom MIME/HTML Mail & Outlook `.msg` CFB/LZFu Parser** | `preview/small.rs` (5,742), `small_kinds.rs` (85), `attachments.rs` (80) — **5,907 lines** | Removes custom CFB/LZFu/MIME parsers & preview network image fetching | Drop the custom `.msg` CFB/LZFu parser and HTML/MIME email client (or keep only lightweight vCard/iCal/Design thumbnail preview). |
| **D4** | **Graphviz (`layout-rs`) & LaTeX Math (`ratex-*`) in Previews** | `preview/diagram.rs` (949), `preview/math.rs` (517), `preview/svg_image.rs` (676) — **2,142 lines** | **~1.12 MB `.text` + KaTeX fonts in `.rdata` (16 crates)** | Remove `layout-rs` and `ratex-*` (just as 0.12 removed Mermaid); `.dot` and math blocks render as highlighted source/text. |
| **D5** | **Tantivy Background Content Index** | `core/src/content.rs` (813), `settings/content.rs` (206) — **1,019 lines** | **2.53 MB `.text` (39 exclusive crates: `tantivy`)** | Optional removal if bundled `ripgrep` + `doctext` is sufficient; `tantivy` is off by default and is the largest non-UI crate in `Glint.exe`. |
| **D6** | **asciinema `.cast` Player** | `preview/cast.rs` (1,097), `model/src/cast.rs` (290) — **1,387 lines** | `glint_ui` `.text` reduction | Preview `.cast` files as text. |
| **D7** | **`ncdu` JSON Export & Virtual Archive** | `core/src/archive/ncdu.rs` (510) + UI wiring — **~600 lines** | Removes `LIST_ONLY` special cases across core & UI | Remove `ncdu` export/import; Glint has its own instant Disk Usage map on Windows. |
| **D8** | **`Vault` `.age` Encryption** | `core/src/vault.rs` (137), `dialogs/vault.rs` (1,426) — **1,563 lines** | **353 KB `.text` (48 crates: `age`, `fluent`, `ml-kem`, etc.)** | Consolidate on Glint's built-in AES-256 `.7z` and `.zip` password-protected archives (which open as folders in Glint). |
| **D9** | **Embedded `rusqlite` (`sql.rs`)** | `core/src/sql.rs` (506) — **506 lines** | **904 KB `.text` (4 crates: `rusqlite`, `libsqlite3-sys`)** | Route SQL `Query` through on-demand `DuckDB` (`duck.rs`) while keeping zero-dep schema/table preview in `data_lens.rs` and `sheet.rs`. |
| **D10** | **Consolidate File Sharing (`ShareDialog` vs `NearbyDialog`)** | `core/src/share.rs` (418), `lan.rs` (574), `dialogs/share.rs` (2,961) vs `localsend.rs` (510), `dialogs/nearby.rs` (1,135) | **1,600–3,900 lines** + drops `sendme`/`dufs`, `qrcode`, Firewall COM APIs (or `hyper` server) | Unify sharing instead of maintaining 4 protocols (`sendme`, `dufs` + Firewall COM inspector, `LocalSend`, Windows Share) across 2 separate dialogs. |
| **D11** | **Prune Niche Optional CLI Tools & Dead Helpers** | `tools.rs`, `repo.rs` (201), `git/cards.rs` (630), `codesearch.rs` (23), `preview/choose.rs:139-151` | **~1,000+ lines**; simplifies installer & Settings from 22 tools to ~14 | Remove `ast-grep`, `onefetch`, and `tokei`; detect developer shells/runtimes (`zoxide`, `just`, `uv`, `bun`, `nushell`) on `PATH` without managing private downloads in `tools.rs`; remove `Choice::later` scaffolding and mark test-only `pub fn` helpers `#[cfg(test)]`. |

**Done, all eleven (the user chose every one):** 1,015 crates became 836 and `Glint.exe` 82.8 MB, not 93.8 (the M0 table); `THIRD_PARTY_CRATES.md` and `THIRD_PARTY_LICENSES.md` written again, `cargo deny check` run (`d613940`).

- **D1** `0f9762b`. The whole reader went (EPUB, MOBI and the rest go to Windows' previewer); the sweep deletes `books.json`.
- **D2** `8f26863`. A text model (ASCII OBJ, PLY, glTF) shows as text, a binary one goes to Windows' previewer.
- **D3** `f5cfabc`. vCard, iCal and Design previews stay (`small.rs` trimmed); `.eml` shows as text and a binary `.msg` goes to Windows' previewer (Windows' own mail previewers stay skipped: they load remote content); the sweep deletes `%TEMP%\Glint attachments`.
- **D4** `c8e1ffc`. 28 crates out; `svg_image.rs` only served diagrams and math, and ordinary `.svg` previews keep working.
- **D5** `c402c1e`. 54 crates out. Typing searches names now; Enter reads the files (ripgrep and the document reader), as with the index off. The sweep deletes `content` and `content.json`.
- **D6** `ac219b5`. `.cast` is a text type.
- **D7** `a66db44`. `LIST_ONLY` served only ncdu: gone with every special case.
- **D8** `1f909a2`. 89 crates out. Glint kept no keys or passphrases (Encrypt took a passphrase each time), so nothing was deleted: an existing `.age` file opens with `age --decrypt file.age` and the same passphrase.
- **D9** `e50ca24`. SQL over a SQLite file goes through DuckDB, whose SQLite extension is fetched once (a clear message offline); schemas show without DuckDB, rows need it. Untried against a real DuckDB (none installed here). Not carried over: the old 10-second limit on a query, and SQLite's immutable open (a WAL database may get its journal beside it).
- **D10** `d7b76c6`. One Share dialog: Nearby (LocalSend, which needed none of the removed code) and Windows' Share; sendme, dufs, the QR code and the firewall code went, and the sweep deletes their downloads.
- **D11** `c51be83`. 14 downloadable tools; the five shells and runtimes come from `PATH` (a missing one gives its `winget install` line); the repository card went with them (the README card stays), `Choice::later` and three unused helpers too, and the sweep deletes the eight tools' downloads.

### Order of work

1. **M0 baseline** (record sizes, crates, `npm run bench`, frame spans, idle CPU).
2. **Part A (`A1`–`A7`)**: remaining hot-path, index service, and startup speedups; re-measure M0.
3. **Part B (`B1`–`B11`)**: `Aware` timing fixes, Grid/Columns parity, clipboard/rename/terminal/address-bar polish, flaky test fixes.
4. **Part D (`D1`–`D11`)**: execute chosen removals commit-by-commit, update `THIRD_PARTY_CRATES.md` (`npm run licenses`), run `cargo deny check`, re-measure M0.
5. **Part C (`C1`–`C6`)**: implement new features (`Ctrl+B` Flat View, clipboard hash verification, extracted-archive awareness, Paste as link, Drop Basket, Store data folder).
6. **Final verification**: full M0 table, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`, `npm run check-imports`, and `tests\e2e\run.ps1`.

**Final verification (2026-11-18, master at `bd192bb`):**

- `cargo test --workspace --no-fail-fast`: shadui-index 98, glint-core 337, glint-model 295, glint-ui 2,416, no failures (X-Ray's flaky test, which failed once in the run before, fixed in `70a9ff3`); `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `npm run check-imports`: Windows 10 has every import of the release `Glint.exe` and `shadui-index.exe`.
- `npm run licenses -- --check`: current. `cargo deny check`: bans, licenses and sources ok; advisories report RUSTSEC-2026-0543, chrono unmaintained (no fixed version; glint-core, glint-model and the vendored GPUI use it), which is the maintainer's to ignore or move off before a release.
- `cargo fmt --check` fails on master in files 0.14 didn't touch, as it did before; files that were clean are still clean.
- `tests\e2e\run.ps1`: 21 of 21, twice (the debug build at `bd192bb`), the user's own Glint files untouched. `command-bar` had failed on this PC, on the build from before 0.14 too: Paste found the clipboard held by another app (PowerToys' Advanced Paste reads it after every copy; `0bfa640` waits up to 1.2 s for it, off the UI thread), and the harness clicked where the dock's buttons had been a moment before (`bd192bb` waits for them to settle).
- M0's cold start, frame spans and idle CPU weren't measured: they need the release `Glint.exe` running, and it shares the installed copy's single-instance name, Run key and taskbar jump list.

---

## 0.13.2: updates that need nobody

1884328. Two bugs the user met: a Glint in the tray showed a new version only after a restart (a 12-hour timer
Windows doesn't run while the PC sleeps, a check that stopped once it found a version, and a notice that faded
unseen in a hidden window), and Scoop kept listing the old version after an update from the app (only the running
copy was replaced; this PC had a Scoop copy beside the setup's). Around them, the update flow of
`crates/glint-ui/src/updater.rs`: hourly looks by the wall clock, a quiet download picked up where it broke off
(not on a metered connection) and kept across restarts, installing on quit or after a start with Windows that
nobody opened, Restart to update, Scoop copies updated through Scoop (`updater/scoop.rs`), notices that wait for
the window, Windows' own notification, and Settings → About's **Install updates automatically**.

Not tried: Windows' own notification from an installed copy, and an update installed silently on quit over a real
installed Glint (both from the dev build's dry run only: `GLINT_UPDATE_DRY_RUN`). Copies before 0.13.2 install it
the old way once (Update, then Install).

## 0.13.1: the zoom

Out on 2026-11-16, a few hours after 0.13.0: one fix on top of it (cb669ef, made on the other PC). `Ctrl` and the wheel, or a pinch, opened the folder under
the pointer and went up to the one above; that was never what the zoom was for. It changes the layout now, as one
ladder (the list, then the grid's tiles from the smallest to the biggest), and never the folder. The rest of the
motion is quieter: opening a folder is a small step, not a flight. `tests/e2e`'s `zoom-layout` holds it with the
real wheel.

Not tried: a pinch on a real touchpad (made-up events only).

## Left over from 0.13

Known when it shipped, none of it holding the release:

- **Windows 10 was not tried on a real PC.** The cause of [issue #6](https://github.com/shadijhade/glint/issues/6)
  is gone from the import tables, and `npm run check-imports` holds every release to what Windows 10 1809 has, but
  nobody has started 0.13 on Windows 10 yet.
- **The Store's copy shares its data folder with a setup copy** on the same PC, until `glint_core::host` picks
  `com.shado.glint.store` when Glint runs as a package ([docs/releasing.md](docs/releasing.md#as-an-msix-package)).
- **Suggested depends on what is known when the menu opens.** A signer's check can take longer than the 40 ms the
  menu waits, so a program right-clicked the instant the pointer arrives may get one suggestion where a moment
  later it gets two. The head catches up; the rows don't move (rule R2 of the plan).
- **Paste's words are for files only.** A picture or text on the clipboard leaves Paste a plain button.
- **Three tests fail once in a while when the whole suite runs** and pass alone:
  `dialogs::timeline::tests::another_time_window_scans_again_and_forgets_the_subfolder`,
  `core_bridge::tests::jobs_posted_from_any_thread_run_on_the_ui_thread_from_the_message_pump` and
  `core_bridge::tests::a_panicking_job_does_not_take_the_ui_down` (seen 2026-11-18; any test's `testing::init`
  replaces the process's message window).
- **Never tried for real:** the start-up note that offers to index every drive (instant search is on here), and a
  tool installed through winget's fallback.

## How a release is made now

`npm run ship -- <x.y.z>` ([docs/releasing.md](docs/releasing.md#one-command-npm-run-ship)): the checks, the version,
the build and its signatures on GitHub, the release, Scoop, winget and the Microsoft Store. 0.13.0 was the first
made this way, and 0.13.1 the second: built and signed on this PC (`--local`, one password), then the release,
Scoop and winget by `ship`, and the Store's package put in Partner Center by hand.

**Since 2026-11-16 the build needs nobody.** The update key and its password are the repository's secrets, put
there by `npm run ship -- --setup` ([docs/releasing.md](docs/releasing.md#a-release-that-needs-nobody)), which
checked them here first. The **Release check** workflow proved GitHub signs with them, and a try run of the
Release workflow built and signed 0.13.1 there in 22 minutes: its update signature and its service's signature
hold here, checked the way the app checks them. So `npm run ship -- <x.y.z>` asks for no password, from any PC
with `gh` signed in, and `--local` stays for a build on this PC.

One thing is left for a release to need nobody at all, once:

- **The Store's application.** No Microsoft Entra directory is associated with the Partner Center account yet
  (seen 2026-11-16), so first, in Partner Center: associate one, add a Microsoft Entra application with the
  Manager role, and make its key. `npm run ship -- --setup` then asks for its tenant ID, client ID and key, tries
  them against the Store and stores them; from then on `ship` sends the package through the **Store** workflow.
  Until then the package is uploaded in Partner Center's pages. `npm run store` has only ever run against a
  stand-in (`npm run test:store`): the first real run should be a look (**Actions → Store → Run workflow**, mode
  *look*), which sends nothing. The workflow itself was tried without the application: it packed 0.13.1's
  package from the released setup program, the same programs as the one packed here.

Never tried for real: a submission through the Store's API, and a release made from a build signed on GitHub
(the try run's files were checked, not released; `ship` checks every build the same way before it publishes).
