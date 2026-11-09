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

## Coming waves (order follows the dependencies)

1. **Platform services:** done in wave 3, except the shell menu's window plumbing (with the file list's context menu) and the updater (last mile).
2. **Components, in groups:** shell and chrome, lists, search and Ctrl+K, previews (native), about 40 dialogs, Settings. See the plan's appendix for the full list with line counts. The explorer model's parts B (file operations, undo, clipboard, rename, new items, recycle bin), C (find, sizes, index) and D (dual pane, compare, sync, archive actions) go with the components that drive them.
3. **Replacing the web previewers** (no WebView anywhere): EPUB (foliate), Word (docx-preview), 3D (three.js), Mermaid and Graphviz, math (temml), email HTML, the terminal (xterm/ghostty-web), video and audio, and Export as PDF (was WebView2's PrintToPdf). Each gets a design note before it's ported.
4. **Platform last mile:** updater, installer (NSIS without Tauri's bundler), MSIX, settings import read directly from WebView2's localStorage LevelDB, and accessibility and right-to-left passes.
