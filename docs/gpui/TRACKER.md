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

## Wave 11: diagrams, math, terminal, Typst (new crates the user approved): done 2026-11-10

*3 ports (Sonnet; 6 agents, ~125 min, 2.0M tokens), all passed review first time. Crates added with the user's OK (2026-11-10): merman 0.9.3 without ELK (EPL-2.0), layout-rs 0.1.3, RaTeX 0.1.15 (KaTeX fonts, SIL OFL: their notice must ship), alacritty_terminal 0.26, Typst 0.15.1 (system fonts only, no packages, no OpenSSL), markdown 1.0; all 1,422 packages' licenses are on the allow list. Integrated by the coordinator: features dropped, Typst and cast bodies in the preview pane, the terminal pane under the panes (Ctrl+`, Run task, scripts), Typst's fonts warmed at start. glint-ui 1,933 tests pass; clippy clean.*

| Part | From | Status |
| :--- | :--- | :--- |
| `svg_image`, `diagram`, `math`: Mermaid and Graphviz in Markdown and as files, inline and display math | MarkdownView (mermaid, math), SmallPreviews (Graphviz, MermaidFile) | done |
| `terminal`: tabs of shells over core's PTY, alacritty's grid painted natively; `cast`: recordings played | TerminalPanel, CastPreview | done |
| `typst_engine`, `typst_view`; Markdown and Typst to PDF in process | TypstView, the WebView print | done |

**Tried for real** (`GLINT_SELFTEST=wave11:`, `term:` and show: captures): notes.md exported to a 46 KB PDF and doc.typ to a 28 KB PDF in process; a Mermaid flowchart, inline and display math beside Arabic; a Typst page typeset with its math and an Arabic line; a .cast recording played with its colours; the terminal pane open in the folder with a command typed.

**Found:** `range.contains(&&usize)` stops compiling once alacritty_terminal adds `PartialOrd<Column> for usize`: pdf.rs and typst_view.rs take `contains(*p)`.

**Left for later:** math and diagrams in Markdown's PDF export (left as source), the Convert document dialog's PDF target for other formats (still the Typst tool), Arabic inside a formula (RaTeX doesn't shape it), Graphviz's fidelity (layout-rs's subset), Arabic strings, the terminal's IME and Arabic in the grid (not checked).

## Wave 12A: installer, updater, end-to-end checks, scrub and book places: done 2026-11-10

*4 tasks (Sonnet; 8 agents, ~54 min, 1.5M tokens), all passed review.* The NSIS installer for the GPUI build (`crates/glint/installer/`, `npm run gpui:installer`; packaging tried with the debug build, never run); the updater (`updater.rs`, behind `updater` until wired: latest.json, minisign-checked installer, passive install); `tests/e2e/run.ps1` (UI Automation against a debug Glint.exe); video scrubbing on grid tiles; books' places and text size in books.json. **The end-to-end checks found three app bugs** (next wave): Enter in the rename box also opens the item under its old name; after the rename box closes nothing has the keyboard; a file dropped from File Explorer is ignored after a key press until the mouse moves over the window.

## Wave 12B: wrapped right-to-left text and the three end-to-end bugs: done 2026-11-10

*2 tasks (Sonnet), both passed review; committed with the updater's wiring as 2a7c94a.* The vendored GPUI wraps a right-to-left paragraph at spaces in reading order and shapes each row alone (13 tests on the real DirectWrite); Enter in the rename box no longer opens the item; the list takes the keyboard back after a rename; drops from other apps work after a key press (file drags count as mouse input). End-to-end: 18 passed. The updater runs from start-up, asks before quitting to install, and has the toolbar's Update button. **Tried for real:** a long Arabic paragraph and a mixed Arabic and English one in the Markdown preview wrap at spaces, rows top to bottom, Latin words at the right ends. **Left:** in an English window those paragraphs align left. The Markdown TextView (gpui-kit) needs each block's direction from its first strong letter, as `preview::rich` already does for Word and books.

## Wave 13: known gaps: done 2026-11-10

*5 tasks (Sonnet; 12 agents, ~87 min, 2.1M tokens), all passed review.*

- **find-buttons:** find as you type (the bar asks the explorer itself; `AddressBarEvent::FindAsTyped` is gone), Search subfolders too, Try again, the three search-wider buttons, the Gallery's Add a folder, Recent's note.
- **grid-rename:** the rename box on tiles (`file_grid/rename.rs`); Escape ends a search as well as a filter.
- **list-facts:** `components/row_facts.rs` (tags and media facts read for the rows drawn, kept per folder), tag dots in the list, the Tags submenu's checks, and the Taken, Dimensions, Duration, Artist and Album columns.
- **markdown-dir:** each Markdown block takes its direction from its first strong letter (vendored gpui-base, VENDOR.md row); lists, quotes and inline flows mirror; code stays left to right.
- **palette:** saved searches, Empty the Recycle Bin, the file actions and tools (`Run::Menu` through `workspace::run_menu_command`), and Run <task> (through `workspace::run_task`).

**Integration:**
- The grid's Search subfolders too searches its own pane.
- The Tags submenu reads the chosen items' tags.
- The palette uses the menus' `kinds`; its copy is deleted.
- The kit's icon test lists all 216 icons the code names (a script made the list).
- The Arabic Markdown test now really runs in an Arabic window.
- New grid test: Escape ends a search.
- **Bug found and fixed:** the Gallery was never given its folders. `gallery_set_folders` went uncalled, so it listed nothing. The explorer now hands over Pictures, Videos and the added folders at start and whenever they change. Tested headless only.

Full suite: 2,075 + 300 + 233 passed; clippy clean.

**Tried for real** (debug build, `GLINT_SELFTEST=look13:` and `show:`, settings put back):
- F2 on a grid tile shows the box.
- Tag dots are read from the files' `glint.tags` streams.
- Ctrl+K "run" lists the folder's npm scripts.
- Arabic paragraphs align right in an English window.

**Left:**
- The grid's box text isn't centred like the label.
- Tag dots on tiles: the TypeScript app had none, so this isn't parity.
- Mirrored table columns, and the ordered-list marker's bidi order.
- Sort and filter by media facts.
- The palette's remaining TS commands: Convert to UTF-8, Get access, Restore and Delete for good, Even out the panes, Invert selection, Extract all, Archive properties, New <kind>, Calculate folder sizes, Move tab to other pane, New window.
- The shared rename-box helper (the list and the grid duplicate it).
- An RTL underline overshoot in gpui-pre's `line.rs`.

**Build folder:** since 2026-11-10, `target/ui` is a junction to `E:\glint-gpui-target\ui`. E: is on the same NVMe drive and has 104 GB free, so C: keeps its room. Paths didn't change, so nothing was rebuilt.

## Wave 14: the service crate, the release pipeline, compare bar, last commands: done 2026-11-10

*5 tasks and a sweep (Sonnet; 15 agents, ~2 h, 2.6M tokens), all passed review (compare-bar and palette-rest after a fix round).*

- **service-crate:** the instant search service is now `crates/shadui-index`, moved with `git mv`.
  - The package and exe names are unchanged; the version follows the workspace (0.10.0).
  - The update key it trusts is now in `crates/shadui-index/update-pubkey.txt`. It is identical to the key in Tauri's config.
  - Profile: src-tauri built the service with fat LTO and one codegen unit; the root profile has neither. The service now gets `codegen-units = 1`; LTO is still open (below).
- **release:** `npm run release` builds the GPUI app:
  - `cargo build --release -p glint -p shadui-index`, then signing of the service, then the NSIS setup.
  - `scripts/lib/minisign.mjs` signs without the Tauri CLI. Its signatures were byte-identical to `tauri signer`'s with a throwaway key.
  - The version comes from the workspace.
  - MSIX, the binaries, `deny.toml` and the cargo-about files moved out of src-tauri. The notices gained the embedded fonts, icons and grammars.
  - `docs/releasing.md` and `release.yml` are rewritten.
  - The dry run on D: passed `verify-update.mjs`. No setup was run and nothing was published.
- **compare-bar:** the bar over each pane while comparing:
  - the only-here, newer, different and older badges;
  - the options: Subfolders too, File contents, Only the differences;
  - Select differences, Sync with the deeper `compare_folders` and `cancel_compare`, and Stop;
  - the bar for two files selected across the panes.
- **palette-rest:** Ctrl+K has the TS palette's remaining commands, except Convert to UTF-8 and the Ctrl+N hint, which need workspace hooks.
- **notices-sidebar:**
  - The archive password question and the notices' Install 7-Zip and Get access buttons.
  - A big archive read whole shows its progress, with Stop (`cancel_archive_read`).
  - The service updates itself quietly after the app does (`index_update`), else offers the admin route.
  - The sidebar's Tags section (`tags_all`).
- **port-me sweep:** 87 notes: 34 stale (22 removed; the diff is comment lines only), 47 open, 6 partial. The list is in the wave 15 plan below.

**Integration:**
- Release builds compiled the self-test's call to a debug-only palette helper. The helper is no longer debug-only.
- The compare bar is wired over each pane.
- **The Store copy gets its own data folder:** under MSIX it uses `com.shado.glint.store`, as Tauri's MSIX build did. Before, it shared `com.shado.shadui-ripgrep` with a setup copy.
- glint-core's tools test reads `crates/glint/installer/tools.nsh`. The updater's key test compares with the service's key file. `bench.mjs` uses `target/ui`.
- `package-lock.json` is at 0.10.0.
- Previous versions' Compare folders opens the two folders in compare panes (f3858a3).

**Parity list re-checked:**
- Snap layouts, the accent colour, launch handoff, shared-memory listings, the clipboard and Markdown-to-HTML are all replaced.
- `open_main_settings` waits on second windows.

**Left:**
- **Needs the user:**
  - cargo-about and cargo-deny aren't installed. `THIRD_PARTY_CRATES.md`'s crate list is still the Tauri build's, and the root workspace was never checked by cargo-deny.
  - `D:\glint-w14-temp` (throwaway key and dry-run output) couldn't be deleted from here.
  - The setup and the MSIX were never run.
- **Release profile:** no LTO for either exe; the Tauri builds had fat LTO. Decide before the first GPUI release.
- **Missing pane bars:** the search bar while searching (Results for…, Stopped at N, Show all, Save, Stop) and the ClosedBar (listing from the index, Get access).

## Wave 15: pane bars, second window, MOBI, Read aloud, batch undo, UTF-8: done 2026-11-10

*6 tasks (Sonnet; 12 agents, ~1.7 h, 2.4M tokens), all passed review on the first try.*

- **pane-bars:**
  - The search bar over a pane: spinner or icon, "Results" or "Close matches" for the query, "Stopped at N" with Show all, then Save, Stop and Close.
  - The closed-folder bar for a listing from the index, with Get access.
- **second-window:** Open in new window, Ctrl+N, and a tab dragged out of its strip all open a second Glint process (`--window <folder> --size WxH --main <pid> [--at x,y]`).
  - It keeps nothing of its own: no single instance, tray, updater, session, placement or state writes.
  - It follows `state.json` and sends Settings to the main window.
  - It closes when the main instance exits.
- **mobi:** a MOBI, AZW and AZW3 reader written in Rust (`preview/mobi.rs`):
  - PalmDOC and HUFF/CDIC compression, EXTH, old MOBI HTML, and KF8's FDST, skeleton and fragments.
  - A DRM-protected book says it can't be read.
- **read-aloud:** the book view reads the chapter aloud with Windows' speech (`platform/speech.rs`). The voice comes from the text's script.
- **batch-undo:** Ctrl+Z undoes batch renames, Cleanup's give-extensions, and compress in place (`UndoAction::Wof`).
- **utf8:** Convert to UTF-8 in the text preview's encoding menu and in Ctrl+K, with Undo.

**Integration:**
- Second windows no longer write `books.json` or run core's housekeeping.
- The Markdown preview's `data:` checks no longer slice inside a multi-byte character (a possible crash).
- Ctrl+N shows beside New window in Ctrl+K.
- The icon test lists all 219 icons.

Tests: 2,258 + 300 + 233 + 92 passed; clippy clean.

**Tried for real** (`look15:` self-tests, settings put back):
- The secondary process opens on the folder at 85% size and exits after the main one.
- The search bar, with Save and Close.
- A generated MOBI book shows its chapters.

**Not tried:**
- Read aloud with sound (left for the user).
- A real store MOBI or KF8 book.
- Undo and UTF-8 against real core (fakes only).

**Release blocker found:** the old app kept everything in WebView2's localStorage (LevelDB):
- the store `shadui-ripgrep-storage`;
- book places and text size;
- recent tags;
- `last-version`;
- the first-notice flags.

The GPUI app imports none of it yet. That is wave 16 (`old_settings/`, without a new crate).

## Antigravity (2026-11-10): closed before it started

The user offered Antigravity to share the work. It hit its rate limit before doing anything; it left only an empty report. Its lane is closed and its worktree and branch are removed.

Its brief is in commit 86c5c9b (`docs/gpui/ANTIGRAVITY.md`). Those tasks come back here:

- **AG-2:** move the service into `crates/shadui-index`. Goes to the swarm as wave 14.
- **AG-3:** the release pipeline for the GPUI build, the notices and the MSIX. Also wave 14.
- **AG-1, the parity audit:** done here instead, by scripts first. They list the strings and commands the TypeScript app has that the GPUI app lacks.
- **Arabic:** still waits on the user.

**Parity, first pass by script (2026-11-10):** of the 271 Tauri commands the TypeScript app invokes, glint-ui names all but 29.

- **Replaced on purpose (16):** the native preview's placing, `drag_under`, `app_ready`, `print_ready`, `snap_place`, `accent_colors`, `take_launch_targets`, `list_dir_shared`, `open_main_settings`, `clipboard_text`, `markdown_html`, and the queue commands (the copy queue calls `queue::` by other names).
- **Still to wire:**
  - Compare folders: `compare_folders` and `cancel_compare`, from Previous versions and the dual pane.
  - `index_update`: the service's update button.
  - `tags_all`: the tag suggestions.
  - `cancel_archive_read`.
  - ~~`gallery_set_folders`~~: wired in wave 13's integration. The Gallery was empty without it.
- **Not settled by this pass:** the ~99 `PORT ME` notes in glint-ui. Each needs a look to tell stale from open; that's a wave 14 task.

## Coming waves (order follows the dependencies)

1. **Platform services:** done in wave 3, except the shell menu's window plumbing (with the file list's context menu) and the updater (last mile).
2. **Components, in groups:** shell and chrome, lists, search and Ctrl+K, previews (native), about 40 dialogs, Settings. See the plan's appendix for the full list with line counts. The explorer model's parts B (file operations, undo, clipboard, rename, new items, recycle bin), C (find, sizes, index) and D (dual pane, compare, sync, archive actions) go with the components that drive them.
3. **Replacing the web previewers** (no WebView anywhere): EPUB (foliate), Word (docx-preview), 3D (three.js), Mermaid and Graphviz, math (temml), email HTML, the terminal (xterm/ghostty-web), video and audio, and Export as PDF (was WebView2's PrintToPdf). Each gets a design note before it's ported.
4. **Platform last mile:** updater, installer (NSIS without Tauri's bundler), MSIX, settings import read directly from WebView2's localStorage LevelDB, and accessibility and right-to-left passes.
