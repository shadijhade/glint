# Glint: moving the UI from WebView2 to GPUI

*Planned 2026-11-09. Part of phase 0 ran the same day ([results](#phase-0-results-2026-11-09)); nothing else is built. Facts about GPUI were checked on 2026-11-09 in its source code (Zed at d2a4fb6, the snapshot that gpui-component 0.9.1 pins) and on crates.io. Glint's numbers come from this PC.*

## In short

- **Goal:** Glint's own UI (the explorer, its dialogs, Settings, Quick Search, Ctrl+K) is drawn by GPUI in the same process as Glint's Rust code. That means no WebView2 at startup, no IPC, and no TypeScript. Today's 41,000 lines of TS/TSX/CSS become Rust.
- **End state (your decision, 2026-11-09): no WebView at all.**
  - No WebView2 island, no wry, no HTML or JS engine.
  - Every preview that used a web library gets a native replacement (see "Previews" below, where island rows are now work to do).
- **Where the work lives:** on the local branch `gpui-migration` (worktree `.claude/worktrees/gpui-migration`). Nothing is pushed until Glint runs fully on GPUI.
- **Who does it:** Haiku and Sonnet agents port code in waves under [STANDARDS.md](STANDARDS.md). Each wave is checked by an independent reviewer, then reviewed and committed by the coordinator. [TRACKER.md](TRACKER.md) shows where each piece stands.
- **Order:**
  1. A measured spike with go/no-go gates.
  2. Glint's Rust taken out of Tauri, and the app's state moved into Rust. This happens while the current app keeps shipping.
  3. The TypeScript logic ported to Rust.
  4. A GPUI explorer you can use every day.
  5. Everything else.
  6. A beta channel, then the switch.
- **Why the order:** steps 2 and 3 pay off even if GPUI later disappoints, and they shrink the rewrite before it starts. Releases on the current UI continue until the switch.
- **Why GPUI over Slint, Qt or Flutter (the 2026-11-07 shortlist):**
  - gpui-component is shadcn-style with Lucide icons, the same design language Glint's Tailwind UI already uses.
  - It's all Rust and runs in-process.
  - Its Windows layer already does what Glint needs: DirectComposition (child windows stay visible), Mica, snap layouts, AccessKit, and a window handle.
  - It has a virtualized table, a tree-sitter code view, Markdown, a dock layout and a WebView crate.
  - Zed ships its own Windows build on this layer.
- **What hasn't changed since 2026-11-07's "not mature yet":**
  - There's no stable GPUI release.
  - The weekly `gpui-pre` snapshots can each change the API.
  - Right-to-left support is incomplete.
  - Accessibility is new.

  The plan contains these risks rather than ignoring them: exact pins, a thin wrapper layer, and gates in phase 0.

## Phase 0 results (2026-11-09)

**What ran:** a first spike on this PC with gpui-pre 0.3.12 (the snapshot gpui-component 0.9.1 pins). It's a 900×600 window with Mica, a header, and a 100,000-row `uniform_list` of English, Arabic and Hebrew names, built in release with thin LTO. The rest of phase 0 is still to do: G5–G8, gpui-component itself, and Quick Search.

| Gate | Result | Notes |
| :--- | :--- | :--- |
| G1 Startup | **Pass with a fix.** The first frame comes at 281 ms median (7 runs) as is, and at 230 ms when the GPU driver is loaded on a thread at start | Where the time goes: process to `main` 9 ms, COM and OLE 8 ms, DirectWrite 52 ms, the rest of the platform 12 ms, the D3D11 device 147 ms, then the window and first frame 35 ms. The device and DirectWrite are made one after the other on the main thread; GPUI could make the device on a thread instead |
| G2 Memory | **Working set passes, private bytes don't:** 63 MB working set, 140 MB private | Today's Glint, measured the same day: 528 MB working set summed over its 8 processes (Glint.exe itself 52 MB) and 301 MB private. Of GPUI's 140 MB private, 54 MB is committed the moment COM starts (only 2 MB of it in RAM), and 53 MB comes with NVIDIA's D3D11 driver. RivaTuner's hook DLL is injected into the spike, so re-measure with it closed |
| G3 Scrolling | **Pass.** 144 fps held (the display's rate), with 0–2 long frames in 578, scrolling 60 px or 1,200 px (50 new rows) per frame, windowed and maximized. CPU was 0.5–1.8 ms per frame | One maximized page-jump run of the four had 31 frames of ~35 ms |
| G4 Text | **Fails as is; passes with a 20-line fix to GPUI.** Stock GPUI draws Arabic and Hebrew left to right in logical order, exactly like Windows' forced left-to-right rendering, so Arabic names come out reversed. The cause is `DrawGlyphRun` in gpui_windows' direct_write.rs: it ignores DirectWrite's baseline origin and bidi level and lays every glyph rightward. Using both ([the fix](#appendix-the-right-to-left-fix): a right-to-left run advances leftward from its origin) made the spike's names match Windows' own rendering exactly, numbers and paths included | The caret, clicks and selection in inputs were broken too; [the full fix](#appendix-the-right-to-left-fix) (2026-11-09) corrects them. Still open: arrow keys move in text order, the one-spot-per-index limit, wrapped lines, an RTL UI |
| G5 Windows | **Mostly passes** (second run, on gpui-component 0.9.1). Details below | Real drags by hand are still to try |
| G6 Accessibility | **The tree passes.** UI Automation shows the address input (Edit, Value), the Go button, and a DataGrid whose rows read "Quarterly report 0.xlsx, 0 KB, 2026-11-09 21:14", with their cells. A header has "Sort by Name" | The window and the input have no names yet; that's Glint's job. Not tried: Narrator's speech, and keyboard navigation through a long list |
| G7 No GPU | **Passes on WARP** (Microsoft Basic Render Driver forced; Windows Sandbox isn't installed here). First frame in ~135 ms with 43 MB working set and 89 MB private. Scrolling ran at 91–144 fps with some ~35 ms frames, using 2–10 ms CPU per frame | WARP's first frame beats this PC's NVIDIA driver (281 ms), because making the hardware device is the slow part. RDP not tried |
| G8 Build | **Passes.** gpui-component added ~75 s to a clean build; rebuilds take 23–26 s. The exe is 25.8 MB with gpui-component (10.7 MB with GPUI alone), both thin LTO. The 462 crates in that build all fall within deny.toml's allowed licenses | Checked with a script against deny.toml's list (cargo-deny isn't installed). Not built with Glint's fat LTO |

**G5 in detail** (second run, on gpui-component 0.9.1):

| Check | Result |
| :--- | :--- |
| Startup | The same window built on gpui-component (its title bar, a 100k-row DataTable, an Input) paints its first frame at 297 ms median (5 runs) and settles at 71 MB working set, 141 MB private. The icons need `.with_assets(gpui_kit::assets::Assets)`, without which the title bar's buttons are invisible |
| Custom title bar | **Pass.** With the pointer really on it, gpui-component's title bar answers caption, minimize, maximize and close, and resting on maximize shows Windows 11's snap layouts. GPUI answers `WM_NCHITTEST` from the hovered element, not from the point asked |
| Drop target | **Pass.** `RevokeDragDrop`, then registering Glint's own `IDropTarget`, both succeed. Fed a real shell data object for two files (one with an Arabic name), it reads both paths and answers copy by default and move with Shift |
| Shell context menu | **Pass, with a rule.** Explorer's own menu for a file (26 items: Open with, Send to, 7-Zip, PowerRename, Properties…) is built in 326 ms, shows, and closes cleanly. Rule: modal Win32 loops (`TrackPopupMenuEx`, `SHDoDragDrop`, and anything else that pumps messages) run from a posted message in a window subclass, never inside a GPUI callback, so GPUI isn't borrowed while they pump |
| Drag-out | The call path works from the subclass (`SHDoDragDrop` returns at once with no button held). A real drag needs a person |
| WebView2 island | **Pass.** As a child of the GPUI window it draws on top. The first page shows 200–260 ms after starting it. While open it adds 6 processes and ~200 MB private, so the island has to close when idle |
| Preview handlers | **WebView2-based ones pass, GDI-drawn ones need a host window.** PowerToys' PDF previewer (out of process, WebView2 inside) shows as a child. Windows' own text previewer (RichEdit, GDI) runs and is in place but invisible: a GPUI window is a DirectComposition window (`WS_EX_NOREDIRECTIONBITMAP`), so GDI children have nothing to draw into. GPUI's flip-model mode hides them too, and so does a layered child. Office's previewers draw the same way. Hosting the handler in an owned, never-activated pop-up kept over the pane works: the text previewer renders there and is the window on top at that spot |
| Quick Search | **Pass.** A second window (`WindowKind::PopUp`, with an input and results) opens to its first frame in 11–13 ms warm, 6 times out of 6. Asking the instant search service in-process (`shadui_index::client::search`): the service took 10 ms, the answer reached the window after 10–12 ms, and was on screen 21 ms after the question |

**Caret positions in Arabic text** (the RTL display fix in place): GPUI's `LineLayout` assumed x grows with the text index.
- **The caret was one letter off:** `x_for_index` returned each Arabic letter's left edge, where the caret belongs on its right edge.
- **Clicks landed on the wrong letter:** for any x inside the Arabic run, `index_for_x` returned the index of the "1" that follows it.

Clicking, selecting and moving the caret in Arabic were wrong in every input (address bar, rename, search). The fix in GPUI's core, the Windows layer and gpui-base's Input was written and tested the same day; see the [appendix](#appendix-the-right-to-left-fix).

**Also learned:**

- **Which GPU:**
  - GPUI took DXGI's adapter 0, which on this PC is the NVIDIA RTX 3070. The PC also has Intel graphics and a SudoMaker virtual display.
  - Forced onto the Intel GPU (through Windows' per-app graphics preference), the first frame came at ~375 ms, with 131 MB working set and 197 MB private. Intel's device is quicker to make (97 ms), but its first frame then takes ~190 ms.
  - Which GPU Glint asks for is open, including whether laptops mind waking their discrete GPU.
- **DirectComposition:** `GPUI_DISABLE_DIRECT_COMPOSITION=1` changed neither time nor memory.
- **Build:** GPUI compiled from clean in about a minute, and the spike rebuilds in 9–14 s. The exe is 10.7 MB (thin LTO, without gpui-component yet). The target folder is 1.1 GB.
- **The fork:** a spike-only copy of gpui-pre-windows carried the timing prints and the RTL fix through `[patch.crates-io]`. Glint would carry the RTL fix the same way until it's upstream.

**What it means:**

- **Nothing found so far rules GPUI out, but two things now need work before phase 1:**
  1. Right-to-left: the display, caret, click and selection fix is written and tested ([patch](rtl-fix.patch)). It still has to go upstream (Zed and gpui-kit), and arrow keys still move in text order.
  2. GDI preview handlers (Office's) need the owned pop-up host, which brings the airspace rules with it.
- **Speed and memory hold up so far.** The window with a 100k list paints at about 60% of today's time (281 ms against 441 ms, or 230 ms with the warm-up). Its working set is about an eighth of today's summed one, and scrolling stays smooth at 144 Hz.
- **Caveat:** the 441 ms is the whole app, so the real app adds its own start-up work (state, the first listing). That work can run while the GPU device is made.
- **Arabic works only with the fix,** so the patch goes upstream first and is carried here until then.

## Where Glint starts

| | Today |
| :--- | :--- |
| Launch to first folder painted | **441 ms** median (best 436 ms, 5 runs, `npm run bench -- --start`, 0.9.5, 2026-11-09) |
| Memory | **323 MB** private for Glint plus its 7 WebView2 processes (measured 2026-11-07) |
| UI code | 94 TSX and 59 TS files: 41,028 lines of TS/TSX/CSS (`src/lib` is 11,009 of them), plus 5,886 lines of vendored foliate |
| Rust ↔ UI surface | 290 `#[tauri::command]`s (278 registered). 95 of the 122 files in `src-tauri/src` mention Tauri: `AppHandle` 72 times, `ipc::Channel` 53, `State` 28, `Emitter` 18, `ipc::Response` 15. 14 event names |
| Windows | The main window (and more with Ctrl+N), Quick Search, and a hidden print window (Export as PDF) |
| Tauri pieces in use | Single instance, updater, dialog, opener, tray, window effects (Mica), asset protocol (`convertFileSrc`, 10+ places), and the NSIS and MSIX bundling |
| Web-only libraries | foliate (EPUB), docx-preview, three.js (3D), mermaid, Graphviz (wasm), temml (math), postal-mime and msgreader (email), xterm and ghostty-web (terminal), Prism (react-syntax-highlighter), sonner (toasts) |

The IPC isn't free either. 50,000 instant-search results take about 170 ms to reach the screen, against about 33 ms inside the service (explorer-vision notes). With GPUI those results never leave Rust.

## GPUI on Windows today (checked 2026-11-09)

**Which GPUI:**

- `gpui` 0.2.2 on crates.io dates from 2025-10-22 and is a year behind Zed.
- Current GPUI reaches crates.io as weekly **`gpui-pre`** snapshots of Zed's split crates (`gpui-pre`, `gpui-pre-platform`, `gpui-pre-windows`…). The gpui-component maintainer publishes them (0.3.12 = zed@d2a4fb6, 2026-11-09).
- **gpui-component 0.9.1** (2026-11-09; the repo is now longbridge/gpui-kit) pins exactly `gpui-pre =0.3.12`. It releases weekly: 0.7.2 on 2026-10-12, 0.9.1 four weeks later.
- `gpui-fast` (0.1.14, Longbridge) is an optional fork with rendering and layout optimizations.
- All of these are Apache-2.0.
- Zed's own `ui`, `markdown`, `terminal` and `terminal_view` crates are **GPL-3.0-or-later**. Glint must not copy them.

**What fits** (from `crates/gpui_windows` and `crates/gpui` at d2a4fb6):

| Need | GPUI |
| :--- | :--- |
| Drawing | Direct3D 11 through DirectComposition (`CreateTargetForHwnd(hwnd, false)`, `WS_EX_NOREDIRECTIONBITMAP`). Phase 0 showed child windows that compose themselves (WebView2, and WebView2-based preview handlers) **draw above GPUI**, while GDI-drawn children stay invisible and need an owned pop-up |
| Text | DirectWrite (`IDWriteTextLayout` per run), with font fallback and IME composition through IMM32 |
| Mica | `WindowBackgroundAppearance::MicaBackdrop` / `MicaAltBackdrop` (`DWMWA_SYSTEMBACKDROP_TYPE`, Windows 11 22621+) |
| Custom title bar with snap layouts | `WindowControlArea::{Drag, Min, Max, Close}` answer `WM_NCHITTEST` with `HTCAPTION`/`HTMAXBUTTON`/… |
| HWND for Glint's Win32 code | `Window` implements `HasWindowHandle`, which serves the shell context menu, SHDoDragDrop, preview handlers and Restart Manager dialogs |
| Accessibility | AccessKit → UI Automation (`accesskit_windows`). Elements take `.role(accesskit::Role)`, `aria_*`, `on_a11y_action`. The tree is built only while a screen reader is connected |
| Touchpads | DirectManipulation (smooth precision-touchpad scrolling) |
| Also | Jump lists (`destination_list.rs`), system notifications, file dialogs, several windows |
| Components (gpui-kit) | 75+, including a virtualized data table (resizable columns, sorting), a virtual list, a tree-sitter code editor (200K lines), Markdown and HTML, charts, a dock layout, Lucide icons, `gpui-webview` (wry) |

**Gaps Glint has to cover itself:**

| Gap | Plan |
| :--- | :--- |
| Drops: GPUI's `IDropTarget` gives paths only and always answers `DROPEFFECT_COPY` | `RevokeDragDrop`, then register Glint's own target from dragdrop.rs. That keeps copy, move and link, Shift, opening tabs on hover, and virtual files. Hover and drop go into the UI through a channel |
| Drag-out | There's none in GPUI. Glint's own SHDoDragDrop starts when a drag passes the threshold in the file list (as today) |
| Tray | `tray-icon` 0.26.1 (MIT/Apache, the crate under Tauri's tray), over GPUI's message loop |
| Right to left: on Windows, stock GPUI draws right-to-left runs left to right, so Arabic and Hebrew come out reversed (phase 0, G4). There's no layout direction (taffy), and text layouts use DirectWrite's default left-to-right reading direction | Carry [the fix](#appendix-the-right-to-left-fix) (display, caret, clicks, selection; written and tested 2026-11-09) and upstream it. Arrow keys moving on screen are still to do. An RTL UI is mirrored by Glint's own row helper, and RTL strings are wrapped in RLI…PDI so DirectWrite orders them as RTL paragraphs. A reading direction upstream comes later |
| No explicit WARP fallback: adapters come from DXGI enumeration, which includes the Basic Render Driver as a software adapter | Test in Windows Sandbox and over RDP in phase 0 |
| Video, audio, PDF, terminal, EPUB, Word, 3D | See [Previews](#previews-native-or-island) |
| Native windows: WebView2 children (island, video) draw above GPUI's drawing, menus included. GDI children (Office's preview handlers) don't show at all | GDI previewers go in an owned pop-up over the pane (works in phase 0). While an overlay intersects a native window, hide it and show a `PrintWindow` snapshot instead. Today's WebView2 build has the same constraint with preview handlers |
| `gpui-component` pins `windows` 0.58 (Glint uses 0.62) | Two versions in the build cost compile time, not correctness |

## Target architecture

```
crates/
  core/    glint-core   today's src-tauri/src minus Tauri: fsx, shell, thumbs, archive, search, queue,
                        rules, git, remote, devices, tools, mcp… Its own tokio runtime (not Tauri's).
  model/   glint-model  the logic now in src/lib/*.ts: tabs and panes, navigation history, selection,
                        sort/group/filter, columns, frecency (rank.ts), plain-words queries (plain.ts),
                        settings schema and persistence, i18n tables. Pure Rust, unit-tested.
  ui/      glint-ui     GPUI views over gpui-component, behind Glint's own thin component layer;
                        the island host.
  app/     Glint.exe    platform services (single instance, tray, hotkey, updater), CLI subcommands
                        (bench, tools install, finish-update…), main().
  index/   shadui-index unchanged (the instant search service).
island/                 the web previewers that remain (Vite), loaded into WebView2 on demand.
src-tauri/              shrinks to adapters over glint-core until the switch, then is deleted.
```

**Principles:**

- **Glint's code calls functions, not commands.** A Tauri command becomes a plain `glint_core` function.
  - Streams: `ipc::Channel<T>` becomes a `Sink<T>` (a closure or an async channel). The GPUI side drains it in a `cx.spawn` task that updates an entity at most once a frame.
  - Events: `Emitter` becomes an event-bus trait.
- **Big data stays in Rust.** A listing is an `Arc<[Entry]>` that the view reads directly. Sort, group and filter run in `glint-model`.
- **Thumbnails and icons skip PNG encoding.** thumbs.rs's BGRA bitmaps become GPUI `RenderImage`s directly, in an LRU keyed by path, mtime and size. Shell icons go the same way. UI icons are the Lucide SVGs from gpui-kit's assets.
- **Lists are Glint's own:**
  - The file list sits on `uniform_list`, or on `list` when grouped (Group by has variable-height headers), with its own selection model (anchor and extent, Ctrl/Shift, marquee, type-ahead, inline rename).
  - The grid is rows of tiles, as grid.ts lays it out today.
  - gpui-component supplies the chrome: inputs, menus, dialogs, popovers, tabs, tooltips, notifications and resizable panels.
- **Modal Win32 loops run outside GPUI.** This covers the shell menu, drag-out, and anything else that pumps messages. The UI posts a message to its own window, and a window subclass runs the loop, so GPUI is never borrowed while Windows pumps messages. Phase 0 ran Explorer's menu this way.
- **Native content goes in two places:**
  - WebView2 (the island, and WebView2-based preview handlers) can be a child window.
  - GDI-drawn preview handlers (Office's, Windows' text previewer) go in an owned, never-activated pop-up kept over the pane. A DirectComposition window gives GDI children nothing to draw into.
- **A thin wrapper layer.** Glint's views use `glint_ui::kit::{Button, Input, Menu, Dialog…}` re-exports and wrappers rather than gpui-component directly. A breaking snapshot then means fixing one module.
- **Keyboard:** GPUI actions and context-scoped keymaps (`Explorer`, `FileList`, `AddressBar`…) replace React's keydown handlers. ShortcutsDialog's list is generated from the keymap.
- **Theme:** the tokens in index.css map to a gpui-component `Theme`: light and dark, Windows' accent color and its shades (appearance.rs), and Mica.
- **Persistence:** settings, session (tabs), pinned folders, history, visits, folder views and rules are JSON files written atomically under `%APPDATA%\com.shado.shadui-ripgrep\state\`. They're the same shape as the settings export (settingsFile.ts).

## Previews: native or island

| Preview | At the switch | Later (optional) |
| :--- | :--- | :--- |
| Images (incl. HEIC/AVIF/RAW through WIC) | Native: WIC → RGBA → `RenderImage` | |
| SVG, Typst | Native (resvg) | |
| Text and code | Native: gpui-component's editor, read-only, tree-sitter (replaces Prism) | |
| Hex, binary, PE, data tables (CSV, Excel, Parquet, SQLite, DuckDB), JSON/YAML/TOML tree | Native (lists and tables) | |
| PDF | Native pages from Windows.Data.Pdf, already used for thumbnails. The island (Edge's viewer) is the fallback for selecting and finding text | |
| Video, audio | Native: Media Foundation (`IMFMediaEngine`) in a child window, with GPUI controls beside it. The island plays what Media Foundation can't | |
| Fonts (H10) | Native: DirectWrite loads the file and GPUI draws the samples | |
| Windows preview handlers (Office…) | Native, as today (native_preview.rs) | |
| Plain Markdown, README cards, notebooks | Native (Markdown view, code view, PNG outputs) | |
| Markdown with Mermaid, Graphviz or math; Export as PDF | Island (print.html's renderer, then `PrintToPdf`) | Graphviz SVG through resvg |
| EPUB (foliate), Word (docx-preview), 3D models (three.js) | Island | Probably never: each is a whole engine |
| Email (.eml/.msg) | Island | Headers and attachments native (mail-parser); island only for HTML bodies |
| Terminal pane, asciinema casts | Island (ghostty-web, as today) | Native on alacritty_terminal (Apache-2.0), written for Glint. Zed's terminal_view is GPL |
| Disk usage map, waveform, video scrub | Native (`canvas`) | |

**The island:**

- It's one WebView2 environment, shared by the island previewers. Each one gets its own controller, as a child window placed from the GPUI element's bounds every frame.
- Local files arrive through a virtual host mapping instead of Tauri's asset protocol.
- It's created on first use, suspended while hidden (what `rest_webview` does today), and closed after a few idle minutes.
- `webview2-com` is already a dependency. `gpui-webview` (wry) is the alternative, settled in phase 0.

## Phases

Sizes are rough, in focused weeks. Each phase ends at a gate. Phases 1 and 2 land on master in small commits while the current app keeps shipping.

### Phase 0: spike and go/no-go (2–3 days)

**What it builds:** a GPUI window on gpui-component 0.9.1 that has:

- Mica and a custom title bar.
- A 100,000-row list with English, Arabic, Hebrew and mixed names.
- Glint's drop target and SHDoDragDrop.
- The shell context menu, using a window subclass for `IContextMenu3` messages.
- A preview handler and a WebView2 child above GPUI.
- A Quick Search window that queries the index service.

Measure it the same way Rune and Slint were measured (first frame, working set, private bytes), with the same 640×400 and 900×600 windows.

| Gate | Pass |
| :--- | :--- |
| G1 Startup | The spike's first frame is ≤ 250 ms warm on this PC. Rune took ~1,240 ms, Slint ~435 ms. Today's whole app paints its first folder at 441 ms |
| G2 Memory | The spike is ≤ 100 MB private with the 100k list. Today: 323 MB |
| G3 Scrolling | The 100k list flings at full resolution with no visible stutter, under 8 ms of CPU per frame; also checked at 150% and 4K |
| G4 Text | Arabic, Hebrew and mixed names show in the right order. An Arabic UI mirrored with the RLI…PDI wrapping reads right. Text is as sharp as WebView2's at 100%, 125% and 150% |
| G5 Windows | Snap layouts on the custom title bar. Glint's drop target replaces GPUI's (copy, move, Shift). Drag-out to File Explorer. The shell menu with its submenus. A preview handler and WebView2 drawing above GPUI. Quick Search reopens in ≤ 50 ms with the app warm |
| G6 Accessibility | Narrator and NVDA read list rows (name, size, date), buttons and inputs. Keyboard-only use works |
| G7 No GPU | It runs in Windows Sandbox (Basic Render Driver) and over RDP |
| G8 Build | Clean and incremental build times, Glint.exe's size change, and `cargo deny` passing with the new crates |

**Stop if** G1, G2 or G4 fail by a wide margin: speed, memory and Arabic file names are the reason for the move. Try `gpui-fast` against the pinned snapshot on G1–G3 before deciding.

**Status:** G1–G8 ran on 2026-11-09 ([results](#phase-0-results-2026-11-09)), the second half on gpui-component 0.9.1, with Quick Search against the index. Still open:
- **Right-to-left:** the fix is written ([patch](rtl-fix.patch)). Left: send it upstream, arrow keys moving on screen, and an RTL-mirrored UI.
- **By hand:**
  - real drags in and out;
  - Narrator's speech and keyboard navigation;
  - memory with RivaTuner closed;
  - Windows Sandbox and RDP;
  - the pop-up host with a real Office previewer.

### Phase 1: Rust out of Tauri, state into Rust (~1 week, in the shipping app)

- Move the workspace root to the repository root. Add `crates/core` and move `src-tauri/src` into it module by module.
  - `ipc::Channel<T>` becomes `glint_core::Sink<T>`.
  - `AppHandle` and `Emitter` become an `Events` trait.
  - `tauri::State<T>` becomes plain state on a `Core` struct.
  - `tauri::async_runtime` becomes core's own tokio runtime.
- Tauri commands become one-line adapters in `src-tauri/src/commands/`. Crate names, the service, the pipe and the bundle identifier stay (see explorer-vision).
- **State moves to Rust files.** Settings, session, pinned folders, history, visits, folder views and rules get written by Rust (`state/*.json`). The React store loads them at start and saves through a command.
  - Ship this in the next release.
  - Every user who updates through it carries their set-up across without anything reading WebView2's localStorage.
- **Gate:** cargo test, clippy, the node checks, `npm run bench` unchanged (within noise), and a release test copy behaving the same.

### Phase 2: the TypeScript logic in Rust (~1 week)

- Port into `glint-model`:
  - explorer.ts: tabs, panes, navigation, clipboard and paste plans.
  - entries.ts, columns.ts and groups.ts: sort, filter, group.
  - rank.ts (frecency) and plain.ts (plain-words queries).
  - store.ts: settings types and defaults.
  - i18n.ts and i18n-ar.ts: as Rust tables. gpui-component already uses `rust-i18n`.
  - Also fileTypes.ts, paths.ts, format.ts, grid.ts (tile sizes), the scheduling of visible thumbnails in thumbs.ts, and the squarified layout from diskUsage.ts and usage.ts.
- Port explorer.check.ts and text.check.ts into Rust unit tests.
- **Gate:** the ported tests pass, and the model's sort and group results match the TS output on the bench folders (100,000 files, WinSxS, System32).

### Phase 3: a GPUI explorer for daily use (3–4 weeks)

The new app (`crates/ui`, `crates/app`) builds from the same workspace into a separate `Glint-next.exe`.

**In order:**

1. Window, title bar, tab strip, Mica, theme, and the keymap.
2. The file list (columns, sort, Group by, selection, rename, type-ahead) on a real listing from `glint-core`.
3. The address bar with completion, breadcrumbs and drops.
4. The sidebar and folder tree, This PC, and the status bar.
5. The grid with thumbnails and tile sizes, zoomed with Ctrl+wheel.
6. File operations, Undo, the copy queue, and drag and drop both ways.
7. The shell context menu, New menu and Open with.
8. Search: names through the index or fd, contents through ripgrep and the content index, plain-words queries, history.
9. The preview pane and Quick Look for the native kinds, plus preview handlers.
10. Dual pane, session restore, and file watching.

**Gate:** you can use it as your file explorer for a week. `Glint.exe bench --start` reports launch to first folder painted ≤ 300 ms median (target 200 ms), and memory is ≤ 160 MB private with a 100k folder open.

### Phase 4: everything else (3–4 weeks)

**Settings and the other windows:**

- Settings (SettingsView, 1,595 lines).
- Ctrl+K: the palette, the calculator, and folders ranked by frecency.
- Quick Search: tray, global hotkey and Start with Windows. The `RegisterHotKey` thread in quick.rs is kept.
- More windows (Ctrl+N), History and Shortcuts.

**About 40 dialogs.** Each is a self-contained port, so they can be done in any order or in parallel sessions:

- **Tools:** Cleanup, Duplicates, X-Ray, Locks, Batch rename, Rules, Timeline, Versions, Compress in place, Drive health, Check.
- **Media and data:** Media and MediaTools, Convert, Doc convert, Shrink, PDF tools, Metadata, Song, DataTools, Query.
- **Network and sharing:** Share, Nearby, Remote, CloudSetup, Backup, Vault, Password, Tags.
- **Larger views:** Compare files (1,574 lines, with similar's diff), Disk usage (914 lines, on `canvas`), Git chips, Repo and README cards, Gallery months, Frequent folders.

**Gate:** every feature in the README works in Glint-next. Track this as a checklist made from the component list (appendix).

### Phase 5: previews (~2 weeks)

- Native: PDF pages (Windows.Data.Pdf), Media Foundation playback, fonts, notebooks, Typst, and the code view.
- The island: an `island/` Vite build holding only the remaining previewers, plus the hidden-window Export as PDF from print.rs.
- **Gate:** every preview kind in fileTypes.ts opens. WebView2 doesn't start until an island kind is opened.

### Phase 6: platform and shipping (1–2 weeks)

- **Single instance:** a named mutex, plus a named pipe that passes the arguments on. This replaces `tauri-plugin-single-instance`; launch.rs's `second_launch` logic stays.
- **Updater:**
  - Read the **same** latest.json and verify its minisign signatures with the **same** public key (`minisign-verify` 0.3).
  - Run the NSIS setup with the arguments `tauri-plugin-updater` uses today. Copy them from its Windows install code so hooks.nsh's update mode keeps working.
  - Because the feed and key don't change, the last Tauri release updates into the GPUI release on its own.
- **Installer:** release.mjs renders the existing custom NSIS template itself and runs makensis. That replaces Tauri's bundler. cargo-packager was last released 2025-11-27, so it isn't relied on. Signing (updater and SignPath) stays as it is. The WebView2 bootstrapper stays, for the island only.
- **MSIX:** scripts/msix.mjs repointed at the new build. The identity (9NQLRJ3HF1C9) is unchanged.
- **One-time import:** for anyone who skips the phase 1 release, a first run that finds no `state/` files but finds WebView2 data reads `shadui-ripgrep-storage` through a hidden island page. It uses the same user data folder and the same origin, which still has to be confirmed (Tauri 2 on Windows uses `http://tauri.localhost` unless `useHttpsScheme` is set).
- **Right to left:**
  - The Arabic UI is mirrored by the row helper, and RTL strings are wrapped in RLI…PDI.
  - Check the address bar and rename boxes with Arabic input: caret movement and selection.
- **Accessibility:** roles on Glint's own list, grid, tree and chips, then a pass with Narrator and NVDA.
- **Docs:** README ("built on…", the benchmarks), CONTRIBUTING (no npm except the island), SECURITY (CSP only for the island), docs/releasing.md, and the license notices (cargo-about picks up the new crates; the npm notices shrink to the island's).
- **Gate:** a release test copy installs over 0.9.x, updates from the previous Tauri build through the real updater, keeps every setting, and passes the accessibility and Arabic checks.

### Phase 7: beta, switch, delete (2–4 weeks of calendar time)

- **Beta:** Glint-next ships side by side from its own feed (`latest-next.json`) to you and testers.
  - On its first run it copies the stable app's `state/` files, one way.
  - It accepts the installed service at the same minor version, so it doesn't ask to update the service on every beta.
- **The switch:** the next release *is* the GPUI app (Glint.exe), delivered through the normal updater.
- **One release later:** delete `src-tauri/`, React, Tailwind and zustand, and the Tauri npm and cargo dependencies.
- **Freeze:** once phase 3 starts, the Tauri UI takes fixes only. New features land in `glint-core` and the GPUI UI.

### Phase 8 (optional): no WebView at all

Replace the island previewers one by one: email headers native, Graphviz through resvg, then a native terminal on alacritty_terminal. EPUB, Word and 3D would each be a project of its own. Do this only if the island turns out to cost more in practice than it's worth.

## Testing

- **View tests:** GPUI's `#[gpui::test]` with `TestAppContext`, headless. It simulates keys and clicks and reads entity state, for the file list, selection, keymaps and dialogs.
- **End-to-end:** **UI Automation** replaces the CDP harness (tauri-app-driving notes). Glint's AccessKit tree is the handle to drive the real app, so these tests double as the accessibility check.
- **Bench:** `Glint.exe bench` is unchanged. `--start` reads the GPUI app's first-folder time from `GLINT_BENCH_START`, as today.
- **CI checks per commit:** cargo test, clippy `-D warnings`, cargo deny. The node checks remain only for the island.

## Risks

| Risk | Mitigation |
| :--- | :--- |
| GPUI's API changes with every snapshot; gpui-component is 0.x and released weekly | Exact pins (`=`), upgraded together once a month. The thin `glint_ui::kit` layer. Everything is Apache-2.0, so a snapshot can be vendored into the repo if the snapshots stop |
| Few hands: one maintainer publishes `gpui-pre`, and Zed builds GPUI for Zed | The same mitigations. Zed and Longbridge Pro both ship on it, so the Windows layer is maintained for their sake |
| Right to left: stock GPUI draws it reversed on Windows and places carets and clicks wrongly; there's no layout direction | [The fix](rtl-fix.patch) carried as a patch and sent upstream. Arabic stays a gate in phase 6. Caret affinity and a reading direction come later, upstream |
| Accessibility is new in GPUI; WebView2 gave it for free | Roles on Glint's own elements, gate G6, UIA tests |
| Airspace with child windows | Snapshot and hide while overlays are open. The island and video stay inside their own pane |
| Compile times (GPUI, gpui-component, tree-sitter grammars, two `windows` versions) | Only the grammars Glint shows, crate boundaries that keep rebuilds small, dev-profile tuning. Measured at G8 |
| A rewrite this size stalls feature work | Phases 1 and 2 ship on their own. The Tauri UI is frozen from phase 3, with fixes only. The dialogs port in parallel |
| Users lose their set-up | Phase 1 moves state to files months before the switch; the one-time island import covers the rest |

## Open decisions for you

1. **The island:** keep WebView2 as an on-demand viewer (recommended), or aim for no WebView at the switch (adds EPUB, Word, 3D, Mermaid and the terminal to the rewrite).
2. **The beta:** side-by-side Glint-next on its own feed (recommended), or an opt-in switch inside one install.
3. **The freeze:** the Tauri UI gets fixes only from phase 3 on (recommended).
4. **The Arabic UI:** mirrored at the switch (recommended), or Arabic file names right at the switch and the mirrored UI during the beta.
5. **The RTL fix:** send [the patch](rtl-fix.patch) upstream (the GPUI parts to Zed, the Input part to gpui-kit) and carry it here meanwhile (recommended: Arabic is the one gate that failed).
6. **Which GPU:** DXGI's default (the discrete GPU here: the faster first frame, but more power on laptops), or ask for the low-power one.

## Appendix: what to port

**Shell and chrome:**

| File | Lines |
| :--- | ---: |
| Explorer | 2,768 |
| AddressBar | 803 |
| CommandBar | 572 |
| StatusBar | 404 |
| Sidebar | 382 |
| TabStrip | 294 |
| ThisPC | 264 |
| FolderTree | 209 |
| WindowControls | 77 |
| ResizeHandle | 74 |
| App | 614 |
| Menu | 241 |
| Modal | 169 |

**Lists:**

| File | Lines |
| :--- | ---: |
| FileList | 473 |
| FileGrid | 223 |
| VirtualList | 119 |
| Thumbnail | 108 |
| ColumnsView | 94 |
| ChangesView | 127 |
| GalleryMonths | 65 |
| FrequentFolders | 48 |

**Search:**

| File | Lines |
| :--- | ---: |
| Results | 414 |
| QuickSearch | 323 |
| Search | 254 |
| CommandPalette | 240 |
| History | 148 |

**Previews:**

| File | Lines |
| :--- | ---: |
| FilePreview | 1,263 |
| DataPreview | 546 |
| SmallPreviews | 390 |
| MarkdownView | 308 |
| BinaryPreview | 307 |
| DataTree | 305 |
| BookView | 230 |
| HexView | 219 |
| CastPreview | 180 |
| Preview | 154 |
| QueryView | 136 |
| DocxView | 128 |
| ModelView | 125 |
| ShellPreview | 114 |
| NotebookView | 102 |
| Waveform | 99 |
| VideoScrub | 81 |
| TypstView | 47 |
| PreviewState | 45 |
| QuickLook | 34 |

**Dialogs and tools:**

| File | Lines |
| :--- | ---: |
| CompareFiles | 1,574 |
| DiskUsage | 914 |
| Cleanup | 707 |
| CloudSetup | 614 |
| XRay | 574 |
| BatchRename | 512 |
| Share | 501 |
| Rules | 481 |
| Timeline | 442 |
| Duplicates | 412 |
| Remote | 390 |
| Media | 367 |
| Backup | 345 |
| TerminalPanel | 337 |
| Convert | 317 |
| HistoryDialog | 315 |
| CompressInPlace | 291 |
| Pdf | 286 |
| Locks | 279 |
| Check | 267 |
| Versions | 257 |
| Shrink | 252 |
| ActionParts | 249 |
| GitChip | 231 |
| DataTools | 218 |
| MediaTools | 217 |
| Nearby | 190 |
| Metadata | 189 |
| Song | 178 |
| CopyQueue | 169 |
| DriveHealth | 168 |
| Vault | 139 |
| RepoCard | 133 |
| Password | 131 |
| ShortcutsDialog | 130 |
| DocConvert | 123 |
| Tags | 85 |
| SignatureChip | 81 |
| ReadmeCard | 76 |
| ToolNeeded | 39 |

**Settings:**

| File | Lines |
| :--- | ---: |
| SettingsView | 1,595 |

**Logic for glint-model (`src/lib`, 11,009 lines):**

| File | Lines |
| :--- | ---: |
| explorer.ts | 2,529 |
| fs.ts | 1,460 |
| entries.ts | 544 |
| store.ts | 461 |
| usage.ts | 351 |
| i18n-ar.ts | 309 |
| thumbs.ts | 247 |
| diskUsage.ts | 236 |
| plain.ts | 223 |
| terminal.ts | 218 |
| share.ts | 196 |
| rank.ts | 169 |
| settingsFile.ts | 167 |
| grid.ts | 161 |
| fileTypes.ts | 151 |
| paths.ts | 148 |

Most of fs.ts is `invoke` wrappers and binary decoding, which disappear rather than get ported. The rest of `src/lib` is files under 150 lines each.

## Appendix: the right-to-left fix

**Where it is:** [rtl-fix.patch](rtl-fix.patch), written and tested on 2026-11-09. It's about 340 added lines against the crates.io sources: gpui-pre-windows 0.3.12, gpui-pre 0.3.12 (Zed d2a4fb6) and gpui-base 0.9.1. Glint would carry it through `[patch.crates-io]`, with copies of those three crates, until it's upstream. The Zed parts go to zed-industries/zed (`crates/gpui_windows/src/direct_write.rs`, `crates/gpui/src/text_system/line_layout.rs`); the gpui-base part goes to longbridge/gpui-kit.

**What it changes:**

1. **Display (gpui_windows, `DrawGlyphRun`):**
   - Each glyph goes where DirectWrite's layout put it: from the run's baseline origin, leftward for an odd bidi level.
   - A right-to-left run's glyphs are handed over in visual order, with their text indices going down. CoreText already gives GPUI that on macOS, and GPUI's line-wrapping code expects it.
2. **Caret and clicks (gpui, `LineLayout`):** when a line has right-to-left text (a run whose indices go down), `x_for_index`, `index_for_x` and `closest_index_for_x` work from glyph clusters: the text each covers, where it is, and its run's direction.
   - **A caret** goes on the leading edge of the character after it. The exception is a caret right after a right-to-left character: it goes on that character's trailing (left) edge, where typing goes on, so the caret follows the letters as they're typed.
   - **A click** takes the nearest caret, so the caret shows where you clicked. On a tie, it takes the glyph under the pointer.
   - **Left-to-right-only lines** take the old code path unchanged.
3. **Selection:**
   - **GPUI:** `LineLayout::x_ranges_for_index_range` is new. It gives the stretches of a line that show a range of text, because a selection across right-to-left text can show in several places.
   - **gpui-base:** the Input paints one rectangle per stretch, instead of one from caret to caret, and builds outlines per rectangle when a row has several.

**Tested:**

- **Unit tests:** in the patch for upstream, and run in the spike as `tests/rtl_caret.rs`, because the published gpui-pre can't build its own tests (it leaves out its test dependencies and fonts). 4 of 4 pass: carets, hit-testing, selection stretches, and left-to-right lines unchanged.
- **With real DirectWrite text** ("تقرير الربع 1.pdf" at 20 px): carets sit on the letters' edges in the right order, and clicks map to the nearest caret.
- **In gpui-component's Input, with real mouse events:**
  - Clicking across the name from left to right moves the cursor back through the Arabic letter by letter (after ع, ب, ل, the space, ي, ق, ت), then into ".pdf". Before the fix, every click in the Arabic landed on the "1".
  - Selecting the first word highlights exactly "تقرير".
  - The caret after it sits at the word's left end.
- **Applying it:** the patch applies cleanly to the pristine crates.io sources (`git apply --check`).

**Still not right:**

- **Arrow keys move in text order.** In Arabic, Right moves the caret leftward on screen. Moving on screen needs a change to gpui-base's movement code.
- **One spot per index.** At the edge between right-to-left text and the left-to-right text after it, one index has two possible spots, and the fix shows one. Example: clicking just before ".pdf" after an Arabic name gives the start of the name. Doing it fully needs caret affinity in GPUI's API.
- **One unreachable edge.** A digit that the bidi algorithm places on the far side of the Arabic leaves one glyph edge no index reaches.
- **Wrapped lines** with right-to-left text aren't handled; Glint's inputs are single-line.
- **An RTL-mirrored UI** isn't covered.
