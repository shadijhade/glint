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
| **Wave 3**: explorer model part A; persistence; single instance; tray and hotkey; drag and drop; preview host; window state and jump list; Glint.exe resources | 7 Sonnet + 1 Haiku + reviewers | running |

## Coming waves (order follows the dependencies)

1. **Platform services** (each its own agent, after the skeleton): single instance and argument forwarding (launch.rs), tray and global hotkey (quick.rs), Glint's own drop target and drag-out (dragdrop.rs), the shell menu's window plumbing, the preview host pop-up (native_preview.rs), the jump list, window placement and snapping.
2. **Components, in groups:** shell and chrome, lists, search and Ctrl+K, previews (native), about 40 dialogs, Settings. See the plan's appendix for the full list with line counts.
3. **Replacing the web previewers** (no WebView anywhere): EPUB (foliate), Word (docx-preview), 3D (three.js), Mermaid and Graphviz, math (temml), email HTML, the terminal (xterm/ghostty-web), video and audio, and Export as PDF (was WebView2's PrintToPdf). Each gets a design note before it's ported.
4. **Platform last mile:** updater, installer (NSIS without Tauri's bundler), MSIX, settings import read directly from WebView2's localStorage LevelDB, and accessibility and right-to-left passes.
