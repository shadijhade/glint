# GPUI migration tracker

*Kept by the coordinator. "Model" is the agent tier used: Sonnet for logic with judgement or subtle semantics, Haiku for mechanical or table work. Status: planned, in progress, review, done, or blocked (with why).*

## Wave 1: pure logic, leaf modules (`crates/glint-model`)

| Module | From | Model | Status |
| :--- | :--- | :--- | :--- |
| `archives` + `paths` | archives.ts (49), paths.ts (148) | Sonnet | planned |
| `rank` | rank.ts (169) | Sonnet | planned |
| `plain` | plain.ts (223) | Sonnet | planned |
| `store` | store.ts (461) | Sonnet | planned |
| `grid` | grid.ts (161) | Sonnet | planned |
| `json_tree` | jsonTree.ts (124) | Sonnet | planned |
| `format` + `color` + `hex` | format.ts (88), color.ts (17), hex.ts (37) | Haiku | planned |
| `cast` + `cd` | cast.ts (59), cd.ts (52) | Haiku | planned |
| `i18n` + `i18n_ar` | i18n.ts (49), i18n-ar.ts (309) | Haiku | planned |
| `markdown` + `small_kinds` | markdown.ts (36), smallKinds.ts (21) | Haiku | planned |

## Coming waves (order follows the dependencies)

1. **Wave 2: logic that builds on wave 1.**
   - entries.ts (544): sorting, filtering, natural order, matching. Natural order needs Windows' `CompareStringEx` from the coordinator first.
   - fileTypes.ts, columns.ts, groups.ts, usage.ts (treemap layout and hit-testing), text.ts.
2. **Core (`crates/glint-core`).**
   - The host shim (Tauri-shaped `AppHandle`, `State`, `Channel`, `Response`, `async_runtime`) comes from the coordinator.
   - Then the non-window modules of src-tauri/src move in groups (archive, search and index, file operations, media, remote and devices, tools).
3. **UI skeleton (`crates/glint-ui`, `crates/glint`), by the coordinator:**
   - Window, title bar, theme from index.css, the kit layer, `t!`, the keymap.
   - The vendored, patched GPUI.
   - One exemplar dialog that sets the pattern.
4. **Components, in groups:** shell and chrome, lists, search and Ctrl+K, previews (native), about 40 dialogs, Settings. See the plan's appendix for the full list with line counts.
5. **Replacing the web previewers** (no WebView anywhere):
   - EPUB (foliate), Word (docx-preview), 3D (three.js), Mermaid and Graphviz, math (temml), email HTML, the terminal (xterm, ghostty-web), video and audio.
   - Each gets its own design note before it's ported.
6. **Platform:** single instance, tray, updater, installer, MSIX, settings import from WebView2's storage (read directly, since there's no WebView left), and accessibility and right-to-left passes.
