# Glint: after 0.13

*0.13.0 came out on 2026-11-16 ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.0)),
0.13.1, a fix for its zoom, the same day ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.1)),
and 0.13.2, updates that need nobody, that evening ([its release](https://github.com/shadijhade/glint/releases/tag/v0.13.2)).
0.14.0 is the release of what's under **In 0.14.0** and of **Planned for 0.14** (audited 2026-11-18 from `5630c16`), done the same day, every item of it: each says where (its commit on master), and the M0 table has the numbers ([its release](https://github.com/shadijhade/glint/releases/tag/v0.14.0)).
The plan for 0.12 that was here is in git history (`git show df1245a:NEXT_RELEASE.md`), the plans for 0.9 to 0.11
before it (`git show 47c33fa:NEXT_RELEASE.md`), and 0.13's own plan, the menu and the dock that know the selection,
is [docs/gpui/aware/PLAN.md](docs/gpui/aware/PLAN.md) with its task cards beside it.*

## Built for 0.16 (2026-11-22, not released)

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
   written (size growing, time since the last write; click to select one). Freeze order (in that list, Ctrl+Alt+F
   or the palette) keeps the folder's rows where they are while files change: new ones go at the end, gone ones
   dim, sizes update in place; turning it off sorts once (`fdbcef6`, `9c88c98`).
9. **The basket's drawer.** The dock's basket pill (Alt+B) opens a drawer: thumbnails, sizes, remove and Clear,
   Batch rename, Compare (two), Zip, Move here and Copy here, and dragging any row takes the whole basket out
   (`13154fa`).
10. **Hover cards.** A background tab, a parent crumb or its arrow shows a card after 400 ms: the folder's name,
    path, counts, size and make-up band, from what's already known (`2de403f`, `ad6b5e2`).

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

Alt+J hop mode, Alt+B the basket's drawer, Ctrl+Alt+F Freeze order.

### Checks

`cargo test --workspace` (core 385, model 348, ui 2,544, index 103) and `cargo clippy --workspace --all-targets
-- -D warnings` clean at `32fe440`; the release build's imports all exist on Windows 10 1809
(`scripts/check-imports.mjs`); e2e (`tests\e2e\run.ps1`) 21/21 with the debug build of `32fe440`.

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
