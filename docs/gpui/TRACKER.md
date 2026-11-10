# GPUI migration tracker

*Kept by the coordinator. "Model" is the agent tier used: Sonnet for logic with judgement or subtle semantics, Haiku for mechanical or table work. Status: planned, in progress, review, done, or blocked (with why).*

## Wave 1: pure logic, leaf modules (`crates/glint-model`): done 2026-11-09

*20 agents, ~7 minutes: 10 ports, 10 independent reviews, no must-fix problems. 123 tests pass with every module together; clippy is clean. Open coordinator decisions are in the cleanup pass below.*

| Module | From | Model | Status |
| :--- | :--- | :--- | :--- |
| `archives` + `paths` | archives.ts (49), paths.ts (148) | Sonnet | done |
| `rank` | rank.ts (169) | Sonnet | done |
| `plain` | plain.ts (223) | Sonnet | done |
| `store` | store.ts (461) | Sonnet | done |
| `grid` | grid.ts (161) | Sonnet | done |
| `json_tree` | jsonTree.ts (124) | Sonnet | done |
| `format` + `color` + `hex` | format.ts (88), color.ts (17), hex.ts (37) | Haiku | done |
| `cast` + `cd` | cast.ts (59), cd.ts (52) | Haiku | done |
| `i18n` + `i18n_ar` | i18n.ts (49), i18n-ar.ts (309) | Haiku | done |
| `markdown` + `small_kinds` | markdown.ts (36), smallKinds.ts (21) | Haiku | done |

## In progress (2026-11-09)

| Work | Who | Status |
| :--- | :--- | :--- |
| **glint-core**: src-tauri's engine without Tauri (host layer) | coordinator | done: builds, clippy clean, 300 tests pass, 17 ignored (commit 50f2db7) |
| **Wave 2**: cleanups of wave 1 (format, store, js helpers, docs) + entries, fileTypes, columns, groups, usage, text | 5 cleanup + 6 port agents (Haiku/Sonnet) with reviewers | done: 24 agents, ~21 min, all reviews pass; glint-model at 229 tests, 13.5k lines, clippy clean |
| **UI skeleton**: glint-ui + glint per UI-SKELETON.md, Drive health exemplar | 1 Sonnet (high) + reviewer | done: passed review first time; 34 tests, clippy clean, starts and mirrors in Arabic (commit f0dd101) |
| **Wave 2b**: settings use the shared types; review follow-ups | 1 Sonnet + 1 Haiku + reviewers | done (commit 46c7d3e) |
| **Native previews research** | 1 Sonnet | done: docs/gpui/previews-native.md (commit e603149) |
| **Wave 3**: explorer model part A; persistence; single instance; tray and hotkey; drag and drop; preview host; window state and jump list; Glint.exe resources | 7 Sonnet + 1 Haiku + reviewers | done 2026-11-10: 18 agents, ~29 min, every review passed (one must-fix, fixed in its round) |
| **Wave 3 integration**: startup.rs wires the services; review should-fixes; manifest and placement vendor changes (VENDOR.md) | coordinator | done 2026-11-10: 146 tests pass, 2 ignored, clippy clean; tried for real (below) |

### Wave 3 integration: what was tried for real, and what wasn't

- **Tried on this PC (debug build):** the window opens hidden and shows in its saved place, maximized as kept (1.1 s to visible in debug); a second launch hands `C:\Windows` over in 99 ms and exits, and the running copy opens it as a new tab (saved session has both tabs); closing quits with code 0 and writes `state.json` and `window.json`; with Keep in tray on, closing hides the window and a launch brings it back maximized; a `--tray` start stays hidden until a launch reveals it, maximized. Glint.exe has one manifest (with longPathAware), the icon and the version info.
- **Not tried for real yet:** drops from File Explorer, drag-out, the preview host pop-up with a real handler (text and Office), the thumbnail toolbar's buttons and the Jump List, taskbar progress, the tray menu's own items and the shortcut summoning, an elevated copy receiving a launch (UIPI blocks it; `ChangeWindowMessageFilterEx` would fix it), and release-build start time and memory with the explorer running.
- **Debug builds stay off what an installed Glint owns:** their own single-instance name and they never touch the Run key or the Store startup task. They do share the data folders: `window.json` (the same file the web app keeps its place in; the test scripts restore it) and `state.json` in `%APPDATA%\com.shado.shadui-ripgrep` (new; the web app doesn't read it).
- **Left for the parts that own them:** `mayQuit` before closing (copy queue, Compare), the notices' buttons (Get access, Install 7-Zip) and the archive password question, the Quick Search window (the shortcut and the tray bring the main window forward and say it isn't built), the old WebView2 settings import, drop answers per view.
- **Small follow-ups noted by reviewers, not done:** preview host pane colors from theme tokens (hard-coded dark and light now); `window_state` and `tray` keep private copies of glint-core's `launch_exe`, `in_sta` and `installed_exe` (make them pub in core); `on_screen` compares workspace coordinates with monitor work areas (only wrong at the edge with a top or left taskbar); `CompanyName` "Shadi Saadi" vs LICENSE's "Shadi Sadi" (the user's call).

## Wave 4: the browsing views (`crates/glint-ui/src/components`): done 2026-11-10

*14 agents, ~53 minutes, 3.6M tokens: 7 ports (5 Sonnet, 2 Haiku), 7 independent reviews, every one passed first time (no must-fix). Integrated by the coordinator: the workspace places the views and wires their events and the window's actions. 362 tests pass, clippy clean.*

| View | From | Model | Status |
| :--- | :--- | :--- | :--- |
| `file_list` | FileList, VirtualList, ResizeHandle, Explorer.tsx's list keys | Sonnet | done: 52 tests, 100k rows |
| `file_grid` (+ `thumbnail.rs`) | FileGrid, Thumbnail, thumbs.ts | Sonnet | done (partial: SVG tiles, marquee): 38 tests |
| `tab_strip` | TabStrip | Sonnet | done: 9 tests |
| `address_bar` | AddressBar | Sonnet | done: 45 tests |
| `sidebar` | Sidebar, FolderTree | Sonnet | done: 26 tests |
| `status_bar` | StatusBar | Haiku | done: 24 tests |
| `this_pc` | ThisPC | Haiku | done (partial: instant search card): tests |

**Tried for real (debug build):** restored tabs, breadcrumbs, sidebar with drives and capacity bars, the sorted list of C:\Windows ("110 items · 4 hidden", read in 0.4 ms, shown in 54 ms), the grid with real Windows thumbnails, This PC with live drive temperatures and totals, a second launch adding a tab, clean quit. Visible after ~0.5 s warm (1.1 s cold) in debug; 112–118 MB working set, 234–247 MB private in debug (release not measured yet).

**Wired to "comes with its part" notices until their parts exist:** renaming (the list emits CommitRename/CancelRename), the item menu (ContextMenu events from the list, the grid and This PC), choosing columns (SetColumns/ResetFolderView: needs `Explorer::set_columns`), searching subfolders (the bar's RunFind/RerunFind, the list's SearchSubfolders), saved searches, remote connections and text search (the sidebar's events), recent changes, duplicates and the disk usage map (This PC's events), drops on the window, Rename/Delete/Quick Look/preview pane keys, Settings, the palette, the shortcuts sheet. Wired for real: tab and pane actions, Back/Forward/Up/Refresh, Ctrl+L/Ctrl+F, list/grid view, grid zoom, hidden items, drag-out from the grid (not from archives), Properties and "Open in File Explorer" on closed folders, opening many files (without the "Open N files?" question yet).

**Explorer API the views asked for** (next wave): `set_focus(path, anchor)` (Ctrl+arrows, press on a multi-selection), `set_selection(paths, anchor, focus)` (marquee), `set_columns` and resetting a folder's view, `set_editing_address`, clearing `Tab::renaming`, index state shared by the status bar and This PC's instant search card, eject through the `Source`, thumbnail and sizes runs per tab for the status bar.

**Kit and theme asks:** Tooltip and Scrollbar exports (three views carry private tooltips, two reach `gpui_kit::base::Scrollbar`), a programmatic PopupMenu (menu key, Shift+F10), `theme::mono_font`, a sky token for cloud marks, filled folder icons (GPUI's svg is single-colour: needs Glint's own assets), the icons the views use in the kit's asset test.

**Polish seen in the real app:** the tab strip's + sits at the window's far end rather than after the last tab; a few literal px sizes in five views; keyboard gaps the reviewers flagged (the sidebar's menu and eject by keyboard, the status bar's breakdown segments); grid and This PC swap Left/Right in Arabic (arrows follow the screen, which the coordinator accepts as right for a mirrored UI).

**Process lessons:** test binaries with full debug info (~1 GB PDB each, one per feature set and agent) filled drive C: mid-wave; dependencies are now built without debug info (`[profile.dev.package."*"]`), and the next waves give agents an absolute `CARGO_TARGET_DIR` (one agent built into `crates/target` from a relative path).

## Wave 5: acting on files, and the first windows of their own: done 2026-11-10

*18 agents, ~56 minutes, 4.0M tokens: 8 ports (7 Sonnet, 1 Haiku), 8 reviews, one fix round (the Haiku dialogs). Integrated by the coordinator: notices.rs words the operations (Undo buttons, the slow-work notice, the taskbar), dialogs::confirm asks before deleting for good, the workspace wires the menus, the copy queue, Settings, Ctrl+K, the dialogs and the file-operation actions, start-up wires Quick Search. 600 tests pass, clippy clean.*

| Part | From | Model | Status |
| :--- | :--- | :--- | :--- |
| explorer part B (`ops.rs`, `undo.rs`, `clipboard.rs`, `queue.rs`, `recycle.rs`, `rename.rs`, `create.rs`, `view.rs`) | explorer.ts's file operations, undo, clipboard, recycle bin, new items; the views' asks | Sonnet | done: 30 new tests |
| `menus` | Explorer.tsx's item, background and folder menus, Menu.tsx, Windows' own menu | Sonnet | done |
| `copy_queue` | CopyQueue.tsx | Sonnet | done |
| `palette` | CommandPalette.tsx | Sonnet | done |
| `settings` | SettingsView.tsx (1,595 lines) | Sonnet | done (updater and logo pending) |
| `quick_search` | QuickSearch.tsx, quick.rs | Sonnet | done |
| dialogs `shortcuts`, `history` | ShortcutsDialog, HistoryDialog | Haiku | done after a fix round (partial: day grouping in UTC) |
| dialogs `locks`, `versions` | LocksDialog, VersionsDialog | Sonnet | done |

**Tried for real (debug build):** `GLINT_SELFTEST=ops:<folder>` (debug builds only, selftest.rs) drives the real app in a scratch folder without synthetic input: open, copy to the clipboard, paste (Windows names the copies "a - Copy.txt"), the rename box, rename, Undo of the rename. All pass; the clipboard and the test state are put back. Quick Search summoned by a second launch shows in 61 ms with the folders used most. The main window still restores its saved place. A first self-test run pressed Undo before the rename's step was recorded (a race in the test, not the app) and sent two test copies to the Recycle Bin; they were removed.

**Vendored GPUI:** a window opened hidden now gets its requested size at once (Quick Search came up 1920 x 1010 before): VENDOR.md.

**Wired to notices until their parts exist:** batch rename, copying out of archives, the many tool and sharing items of the menus (named by the menu's own word), comparing, connecting a cloud, folder rules, the "Open N files?" question (they open), the keyboard menu placed at the focused row (it opens a third of the way down).

**Next for these parts:** the views use the explorer's new APIs (cut items dimmed, set_focus for Ctrl+arrows, set_selection for the marquee, set_editing_address, eject through the explorer, the focused row's bounds for the keyboard menu); Settings' "shortcut taken" hint (set_quick_taken from the tray's error); local dates in the history and versions dialogs (glint-model needs day, time and date formats); the quit path waiting for `busy_count()`; `reload_recycle_bins` when the window regains focus.

## Wave 6: search, drops and the views' follow-ups: done 2026-11-10

*16 agents, ~38 minutes, 3.3M tokens: 8 changes (7 Sonnet, 1 Haiku), all passed review first time. Integrated by the coordinator: search events wired to the explorer's find, the sidebar opens the shared folder menu, batch rename, tags, vault and the archive password are opened from the explorer's and the menus' events, the instant-search offer is worded with its Turn on button. 738 tests pass, clippy clean.*

| Change | Model | Status |
| :--- | :--- | :--- |
| explorer part C: `find.rs` (run_find, as-typed, saved searches, cancelling everywhere the TypeScript did) | Sonnet | done: 24 tests |
| file list: Ctrl+arrows and the marquee through the explorer, cut items dimmed, thumbnails, drops, drag-out, the keyboard menu at the focused row | Sonnet | done |
| file grid: the same, plus SVG tiles drawn by Glint | Sonnet | done |
| sidebar: the shared folder menu, drops, eject through the explorer | Sonnet | done |
| tab strip and address bar: drops, spring-loaded tabs, the + after the last tab, crumbs take drops and drag out | Sonnet | done |
| status bar and This PC: keyboard segments, the thumbnail line, tile drops, eject | Haiku | done |
| dialog: batch rename | Sonnet | done (Ctrl+Z for a batch needs an UndoAction) |
| dialogs: tags, password, vault | Sonnet | done (recent tags not kept across runs yet) |

**Tried for real:** the self-test now also searches: "Copy" in the scratch folder, 2 found by the instant-search index. Copy, paste, rename and undo pass again; nothing reaches the Recycle Bin.

**Process:** one agent ran `git rm --cached` on the file it was retiring and another `git diff --stat` (both against the rule, neither harmful); a stray `crates/target` appeared again, so `.cargo/config.toml` now pins every build to `target/ui`.

**Not tried for real yet:** drops from File Explorer onto each view and drag-out (needs a real OLE drag: a UI Automation harness), the instant-search offer's Turn on.

## Wave 7: native previews and seven dialogs: done 2026-11-10

*12 agents, ~52 minutes, 3.0M tokens: 6 ports (all Sonnet), all passed review first time. Integrated by the coordinator: the preview pane at the end edge (made and dropped with its setting, Alt+P), Quick Look on Space, menus covering Windows' previewers, the seven dialogs opened from the menus, This PC and Settings, saved folder rules handed to core at start. 951 tests pass, clippy clean.*

| Part | From | Status |
| :--- | :--- | :--- |
| `preview` frame: pane, Quick Look, pictures (zoom, pan, colors, OCR, rotate, wallpaper), Windows' preview handlers, the details | FilePreview, Preview, PreviewState, QuickLook | done: 80 tests |
| previewers: text and code (highlighted), Markdown, hex and PE headers, JSON/YAML/TOML trees, CSV/TSV tables | the text, Markdown, hex, binary and data views | done |
| dialogs: duplicates, recent changes | DuplicatesDialog, TimelineDialog | done |
| dialog: clean up | CleanupDialog | done |
| dialogs: X-Ray, checking files (secrets, YARA, Defender) | XRayDialog, CheckDialog | done |
| dialogs: folder rules, compress in place | RulesDialog, CompressInPlaceDialog | done |

**Tried for real:** a debug-only `GLINT_SELFTEST=show:<file>` opens a file's folder with the preview pane on. Highlighted Rust with line numbers, Markdown with an Arabic heading and a highlighted code block, and a 3840 x 2400 JPEG all render. The picture first stayed blank: it measured its area in a canvas and called `notify` during the draw, which GPUI drops; it now asks after the frame (`on_next_frame`), and STANDARDS has the rule.

**Left for later parts:** video and audio, PDF pages drawn natively (Windows' PDF previewer stands in), EPUB, Word, 3D, email, notebooks, Mermaid and math, the terminal; the toasts of what a folder rule did (core's "rule-did"); saving a dragged preview width; Ctrl+Z for batch renames and WOF compression (an UndoAction each); a shared Tools model for the install cards; the licenses of the grammars two-face bundles (check before a release).

## Wave 8: text search, the disk usage map, archives, fifteen dialogs: done 2026-11-10

*8 ports (Sonnet), all passed review first time. Integrated by the coordinator: the features dropped (`dialogs::action_run` declared once), text mode (Files | Text switch, Ctrl+Shift+F / Ctrl+Shift+E; tabs, sidebar, status bar and the files' keys are off while it shows), the map as a pane view with its status line, toolbar button and palette command (`actions::DiskUsage`), archive notices with Stop and the taskbar, compress (secrets check first) and compress with a password, extract, mount, drags and copies out of archives, archive handoffs, Ctrl+M (`compare_pair`), every menu, sidebar and Settings event of the new dialogs, the Sharing button, quitting asks about running copies and Compare's edits, remote connections handed to core at start. glint-ui 1,253 tests, glint-core 300, glint-model 233 pass; clippy clean.*

| Part | From | Status |
| :--- | :--- | :--- |
| `text_search`: query, switches, results, history, replace, preview | Search, Results, History | done |
| `disk_usage`: the map (index or pdu), colors by type/age/growth, its words | DiskUsage, diskUsage.ts | done |
| explorer part D, archives: compress, extract, mount, stage out, drags, handoffs | archive actions in explorer.ts | done |
| dialogs: compare files | CompareFilesDialog | done |
| dialogs: media, media tools (trim bar, can't play), song tags, metadata | MediaDialog, MediaTools, SongDialog, MetadataDialog | done |
| dialogs: convert pictures, convert documents, make smaller, PDF tools, data tools | ConvertDialog, DocConvertDialog, ShrinkDialog, PdfDialog, DataTools | done |
| dialogs: share, nearby | ShareDialog, NearbyDialog | done |
| dialogs: remote connection, cloud setup, backup | RemoteDialog, CloudSetup, BackupDialog | done |

**Tried for real** (`GLINT_SELFTEST=wave8:<folder>`, plus `text:` and `map:` captures): text search found the needle (ripgrep, 1 match; Arabic file names and mixed lines render in order), back to the files; the map came up from the instant-search index ("index · mapped 10 MB in 1.8 ms") and went again; Compare opened; two files compressed into `a.zip` and extracted beside it.

**Found and fixed while trying:** a dev build had no rg.exe beside it and Windows' App Paths entry for the Store Glint puts its WindowsApps folder first on PATH for any Glint.exe the shell starts, whose rg.exe other processes may not run ("Access is denied"): `crates/glint/build.rs` now puts rg, fd and pdu next to Glint.exe, and `proc::locate` skips WindowsApps folders (the shipping Tauri app has the same PATH fallback for optional tools: worth the same skip on master). A view taking a pane's place (the list to its map, This PC to a folder) now takes the keyboard when the old one had it. glint-core asks for serde_json's `preserve_order` itself, so its tests see the key order Glint.exe has (two tests assumed sorted keys). Pdu maps kept: 4, not 8 (about 10 MB each when big).

**Left for later:** the CommandBar (New, Sort, View, Disk usage... the toolbar has only Disk usage and the mode switch), comparing folders with an earlier version, Open with for files not in archives (its app, an editor, Windows' picker), the remaining menu events (`other =>` in `on_menu_event`: run in sandbox, new window, open file location, forget recent, reveal, folder sizes, batch rename folder, wallpaper, rotate, cloud pin, ncdu export, scripts, tools, on-demand), the video/audio player with the trim bar, the preview pane's Query and Profile views (`data_tools::QueryView`/`ProfileView` exist), Arabic catalog entries for all wave 8 strings, moving text search's own keys into `actions::BINDINGS`, a shared save picker (CSV/JSON filters) and file picker, a kit Slider, one-step Undo for runs that replaced files, Remote's Save guarded against a double press, Cloud setup's Other clouds waiting for the delete, virtualizing Cloud setup's service list, `glint_core::remote::sftp::UNKNOWN_HOST` exported, the two action-parts modules (`media::parts`, `action_run`) merged.

## Wave 9: command bar, git, views, previews, menu actions, pickers: done 2026-11-10

*7 ports (Sonnet; 16 agents with reviews, ~54 min, 3.4M tokens), all passed review (views polish after one fix round), then 3 integration agents (8 with reviews, ~66 min, 1.5M tokens) wired the component files while the coordinator wired the window: the command bar above the panes (it took over Disk usage; Folder sizes says Stop while measuring), the Columns view, the Gallery's months, frequent folders above Recent, the README card above a folder's files, Git's marks to the explorer's rows (`set_git_marks` on `GitEvent::Changed`), every remaining menu event (open with, sandbox, reveal, Recent, folder sizes, wallpaper, rotate, cloud pin, ncdu export through Windows' save picker, scripts, tools, on-demand folders), the explorer's Action/Export/Request events with their notices. glint-ui 1,540 tests, glint-core 300, glint-model 233 pass; clippy clean.*

| Part | From | Status |
| :--- | :--- | :--- |
| `command_bar`: buttons, fold levels, Tools/Run/View/New menus | CommandBar.tsx | done |
| `git`: model, row marks, status line, chip, repository and README cards, Changes view | git.ts, GitChip, RepoCard, ReadmeCard, ChangesView | done |
| `columns_view`, `gallery_months`, `frequent_folders` | ColumnsView, GalleryMonths, FrequentFolders | done |
| previews: SQLite, workbooks and Parquet, notebooks, program headers | DataPreview (SqlitePreview, SheetPreview), NotebookView, BinaryPreview | done |
| previews: contacts, calendars, mail, design files; the signature chip | SmallPreviews, SignatureChip | done |
| explorer: remaining menu actions, folder sizes; `platform::pickers` | explorer.ts, the menus' actions | done |
| wave 8 fixes: one action-parts module, guarded Save, cloud setup order, no-core notice; one-press Undo (`UndoAction::Sequence`) | review findings | done |

**Tried for real** (`GLINT_SELFTEST=wave9:<folder>` in a scratch Git repository, plus `show:` captures): folder sizes from the instant-search index ("index sized 3.2 MB in 0.6 ms", the photos folder 1.6 MB); Git's line ("git · 77 ms") and a.txt's Changed badge; the command bar; SQLite schema with Rows and Query; a notebook (Markdown with Arabic, highlighted code, output); notepad.exe's sections and imports with "Signed by Microsoft Windows" from the real signature check; contacts with an Arabic name; a mail with its headers and an Arabic line. Wave 8's and the ops self-tests pass again.

**Found while trying:** extracting right after compressing in the same tab cancelled the compress (core runs a tab's archive jobs in one lane, as the TypeScript did) and the half-written zip was removed; the self-test now waits for the compress to end. Worth a look on master too: starting archive work in a tab stops what that tab was compressing. The pickers' futures are `Send` now, so Settings needs no unsafe wrapper.

**Left for later:** Git marks in the grid (FileGrid.tsx drew them), the status bar's sized hover built from joined pieces (translate as whole sentences), a second window for Open in new window (a new tab meanwhile), the terminal pane (Run task, scripts in a terminal, Terminal here), Compare panes (no handler for Ctrl+Shift+M, nothing draws the highlights), a Columns view key (Ctrl+Shift+9), design files checked against real .kra/.ora/.procreate/.sketch files, Quick Look's Page keys for the small, text and Markdown previews, `dialogs/vault.rs` merged into action_run (done) and `batch_rename`/`gallery_months` sharing one local-date helper, two tests that fail now and then under a full parallel run (core_bridge's message-pump job test, the preview data tree test).

## Wave 10: native previews with no new crate, compare panes: done 2026-11-10

*6 ports (Sonnet; 12 agents with reviews, ~71 min, 2.8M tokens), all passed review first time. Integrated by the coordinator: the preview chooser's Player, Pdf, Book, Docx and Model bodies (video, sound and PDFs in archives are shown from a copy the view extracts), Quick Look's Page keys for each, the atlas released for frames and drawings, a Word document Glint can't lay out falling back to Windows' previewer, Ctrl+Shift+M and the command bar through `Explorer::toggle_compare_panes`. glint-ui 1,729 tests pass; clippy clean. `image`, `zip` and `roxmltree` became glint-ui dependencies (already in the build).*

| Part | From | Status |
| :--- | :--- | :--- |
| `player`: Media Foundation's media engine (frames through a D3D11 texture), Trim, waveform, transcript, scrubbing | VideoPreview, AudioPreview, Waveform, VideoScrub | done |
| `pdf`: Windows.Data.Pdf pages, zoom, page jump, copy text, find | PdfPreview (was WebView2's viewer) | done |
| `rich`, `book`, `docx`: EPUB, FB2, comics; Word documents | BookView, DocxView | done (no MOBI) |
| mail: sanitised HTML bodies, remote content off, cid pictures, Outlook RTF | SmallPreviews (Email) | done |
| `model3d`: STL, OBJ, PLY, glTF/GLB, 3MF; software renderer, orbit | ModelView | done |
| compare panes and diff-only in the explorer and list; Git marks in the grid | Explorer.tsx, FileGrid.tsx | done |

**Tried for real** (show: captures): a 640 x 360 H.264 clip's first frame with the controls and Trim; Alarm01.wav with its waveform from a real decode; a two-page PDF drawn by Windows.Data.Pdf; a Word document's heading, bold and italic runs and a table; an STL cube shaded.

**Found while trying, top of the next wave:** a right-to-left paragraph that wraps breaks inside a word and its second line runs past the start edge (the Word preview's Arabic paragraph). GPUI's wrapped lines don't reorder each line on its own (the RTL patch covered single lines: "wrapped lines" was its open item), so every wrapped Arabic paragraph is affected (Markdown, notebooks, mail, books, Word).

**Left for later:** MOBI/AZW (a reader of its own), Read aloud, a book's last place and text size kept between runs, video scrub thumbnails on grid tiles (`player::Scrub` exists), the compare bar (counts, Sync, deep compare: no core command), HTML mail's right-to-left alignment, a plain/HTML switch and a "load remote pictures" switch, tiled PDF rendering past 8 MP, Arabic strings for all of the above.

## Coming waves (order follows the dependencies)

1. **Platform services:** done in wave 3, except the shell menu's window plumbing (with the file list's context menu) and the updater (last mile).
2. **Components, in groups:** shell and chrome, lists, search and Ctrl+K, previews (native), about 40 dialogs, Settings. See the plan's appendix for the full list with line counts. The explorer model's parts B (file operations, undo, clipboard, rename, new items, recycle bin), C (find, sizes, index) and D (dual pane, compare, sync, archive actions) go with the components that drive them.
3. **Replacing the web previewers** (no WebView anywhere): EPUB (foliate), Word (docx-preview), 3D (three.js), Mermaid and Graphviz, math (temml), email HTML, the terminal (xterm/ghostty-web), video and audio, and Export as PDF (was WebView2's PrintToPdf). Each gets a design note before it's ported.
4. **Platform last mile:** updater, installer (NSIS without Tauri's bundler), MSIX, settings import read directly from WebView2's localStorage LevelDB, and accessibility and right-to-left passes.
