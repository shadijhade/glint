# Glint 0.12: lighter, quicker, tidier

*Planned 2026-11-14 from master at `47c33fa` and approved the same day; nothing is built yet. This file is the brief
for whoever implements it: start at M0, keep the numbers in the table at the end. The plans for 0.9 to 0.11 that
were here are in git history (`git show 47c33fa:NEXT_RELEASE.md`).*

## Context

0.10 and 0.11 moved Glint to a native window and added a lot. This release adds nothing: it removes what is dead or
not worth its weight, makes what stays cheaper, and repairs what the move dropped. The list comes from an audit of
`47c33fa`: the 0.11.0 release build read symbol by symbol (`scripts/dev/size-by-crate.py`), every function traced
from the app's entry points (`scripts/dev/unreached-fns.py`), and two read-only sweeps of the UI and the engine.
Items marked **V** were confirmed by reading the code; **S** items come from the sweeps and must be confirmed (and
measured) before they are changed.

**What the audit found**
- `Glint.exe` is **174.1 MB** (setup 45.75 MB, MSIX 68.1 MB; the Tauri exe was 53 MB): 124.5 MB of code, 43.2 MB of
  read-only data, 964 crates. Biggest crates in the code: glint-ui 14.6 MB, gpui 10.2 MB, glint-core 6.6 MB, tokio
  5.9 MB (one task harness per call site, 11,564 symbols). What a dependency takes with it (it and every crate only
  it brings in, estimates): **Typst 18.5 MB** (81 crates), **Mermaid (merman) 15.9 MB** (21), **iroh 4.5 MB** (48),
  tantivy 2.6 MB, gix 2.4 MB, rusqlite 1.4 MB, RaTeX 1.2 MB. Their data and unwind tables come on top.
- **Six background services the old app started once its page had painted are never started since 0.10** (the old
  `app_ready`: `git show b55e18b^:src-tauri/src/lib.rs`, lines 222-242): `content::start`, `media_ai::start`,
  `devices::start`, `ondemand::start`, `backup::start_scheduler`, `dialogs::listen`. So the content index does not
  catch up after a restart, daily and weekly backups never run, remotes shown in File Explorer are not connected
  again, and `Ctrl+Alt+G` does nothing. The README promises all four.
- Hot paths still do per-key and per-folder work nobody sees (part D), the copy queue's progress event is quadratic,
  and the index service repeats per-hit work and never cancels a stale scan (parts E, F).

**Decisions made (2026-11-14)**
| Feature | Decision |
| :--- | :--- |
| Mermaid diagrams | **Remove** (a mermaid block shows as code) |
| Built-in Typst | **Remove**; `.typ` preview and Export as PDF are **dropped entirely** (`.typ` shows as text). Convert document keeps its PDF route through the pandoc and typst *tools* |
| Paired devices (iroh) | **Remove** |
| Photo, speech and meaning search (local AI) | **Remove all three** |
| Calculator in Ctrl+K, everything else | Keep |

**Intended outcome**: a 0.12.0 whose exe is roughly 40-55 MB smaller, whose idle and per-key costs are measurably
lower, with the dropped services running again, and with no dead code or migration scaffolding left. Expected, not
promised: the table in M0 decides.

## Ground rules

1. **Measure first, keep the numbers.** Run M0 before any change and again after each part; put both in the table at
   the end of this file. A change that is not measurably better and is not a removal or a fix is reverted. Never
   state a cause or a saving that was not measured: say "estimate".
2. **Where to work.** The main checkout `C:\Repos\VisualStudioRepos\shadui-ripgrep`, branch `master` (no branches, no
   PRs; `git pull` first, the user also works from a second PC). Commit locally, one commit per item id, only once the
   user has said to commit as you go; **never push** without their word.
3. **Build folder.** `$env:CARGO_TARGET_DIR = "E:/glint-gpui-target/ui"` (the existing cache; the checkout has no
   `target/`). E: had ~31 GB free on 2026-11-14: when it drops under 10 GB delete old folders under
   `debug\incremental` only, and say so. Never delete `E:\glint-gpui-target` itself, and touch nothing in
   `D:\glint-build`, `D:\glint-head`, `D:\glint-target`.
4. **The user's own Glint.** Before launching anything, check for a running `Glint*` process. A copy the user started
   is left alone: do not kill it, overwrite its exe or run the e2e harness (it swaps the profile under it). Build to
   another name and wait. Never click the "Instant search has an update" notice's Update button (UAC), and never run
   an uninstaller of a test copy.
5. **Builds to use.** Size: plain `cargo build --release -p glint -p shadui-index`. Speed: the windowed,
   dev-behaving optimized build `cargo build --release -p glint --config "profile.release.package.glint-ui.debug-assertions=true"`
   (the flag on glint-ui only, or it becomes a console program and every start measures 170 ms of console), copied to
   `E:\glint-gpui-target\ui\perf\Glint-next.exe`. e2e: the debug build `cargo build -p glint`.
6. **Checks before saying an item is done**: `cargo test --workspace` (write the output to a file and grep it; piping
   into `head` cuts the run short), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. After
   each *part*: e2e `powershell -STA -File tests\e2e\run.ps1` (18 checks, ~100 s, takes the real mouse and keyboard:
   **ask the user first**). A check that fails once is a fault until the app's log says otherwise
   (`GLINT_FRAME_LOG=1`, `GLINT_E2E_STDERR=<file>`). Run the whole set again on the very last state.
7. **Crates.** No new crate without the user's OK. After any change to dependencies: `npm run licenses`, commit
   `THIRD_PARTY_CRATES.md`, `cargo deny check`. cargo-deny 0.20.2 and cargo-about 0.9.3 are not on PATH here (0.11 used
   copies in a session's scratchpad): ask before installing them.
8. **Vendored GPUI.** Every change under `vendor/` gets a row in `docs/gpui/VENDOR.md`.
9. **Rules of the code** stay those of `docs/gpui/STANDARDS.md` (§3 Rust rules, §5 UI rules, §7 "Done means"):
   never run modal loops or Win32 calls that message a GPUI window inside GPUI callbacks (`core_bridge::post_to_ui`),
   `notify` during a draw is dropped (`window.on_next_frame`), test helpers must not return a window from under
   `testing::lock()`.
10. **Do not touch** (looks like clean-up, is not): the names `shadui-index`, `com.shado.shadui-ripgrep`,
    `com.shado.glint.store`, `TAURI_SIGNING_PRIVATE_KEY*` (installed copies, user data and secrets depend on them);
    `latest.json`'s format and the installer's command line (docs/releasing.md, "Change none of them"); the
    `old_settings/` import (0.9.x users still arrive); the pre-made Quick Search window; `panic = "unwind"`
    (`core_bridge::run` turns a panicking task into an `Err`); the "Ported from …tsx" tables in module docs (history,
    not worth the churn).

## M0. Baseline (before anything, then after each part)

| Measure | How | 0.11.0 |
| :--- | :--- | :--- |
| `Glint.exe`, setup, code and data | file sizes; `python scripts/dev/size-by-crate.py --json <file>` for sections, crates and what each dependency takes; `--compare <file>` against the last run | 174,127,104 B; 45,752,959 B; .text 124.5 MB, .rdata 43.2 MB |
| Crates compiled | `cargo tree --locked --offline -p glint --target x86_64-pc-windows-msvc -e normal --prefix none \| sort -u \| wc -l` | 964 |
| Release build time | wall time of the release build after `cargo clean -p glint-ui -p glint-core -p glint` | record |
| Cold start | `npm run bench -- --start` (launch to first folder painted, 5 runs) | record (231 ms on 2026-11-11) |
| Engine | `npm run bench` (and `-- --copy 5000`) | record |
| Key to frame, folder to screen | `GLINT_FRAME_LOG=1` with scratch scenarios on `tests/e2e/lib/Harness.ps1` (arrow keys and Ctrl+A in a folder of 100,000 made in a scratch folder, never at a drive's top; Ctrl+wheel zoom; open 20 folders in a row): read the `input … ms=`, `nav shown` and `span name=` lines | record |
| Memory at rest | 15 s after the first folder: `Get-Process Glint-next \| select PrivateMemorySize64, WorkingSet64` (start without `GLINT_BENCH_START`, which quits) | record (~316 MB private, ~123 MB working set on 2026-11-11) |
| Idle CPU | `TotalProcessorTime` delta over 120 s with the window resting, and again hidden in the tray | record |

New spans are added with `crate::perf::span("name")` (`crates/glint-ui/src/perf.rs`). To find an unknown slow
handler, time each observer call in vendored `gpui-pre` `app.rs` (`apply_notify_effect`, `apply_emit_effect`) in a
debug build, then revert.

## Part A. Repair what the move dropped (do first; small)

**A1. Start the four services again (V).** In `crates/glint-ui/src/startup.rs`, where `quick_search::warm(cx)` runs
after the first folder (the `services.*` block around line 180), add a `services.background` flag (off in tests, off in
a secondary window, as `core_bridge::init_with`'s `housekeeping`) that, once per process and at least 1 s after the
first folder is on screen, calls from a background thread:
`glint_core::content::start(&app)`, `glint_core::ondemand::start()`, `glint_core::backup::start_scheduler(app.clone())`,
`glint_core::dialogs::listen()`. (`devices::start` and `media_ai::start` are deleted in part B.) Make
`start_scheduler` return without spawning its thread when no plan has a schedule other than `Manual`, and start it
when a plan gets one (`backup_schedule`, `backup_setup`). The UI already listens for `backup-finished`
(`startup.rs:503`) and reports the folder shown for `Ctrl+Alt+G` (`explorer/start.rs:271`).
*Done when*: a headless test proves each is called once and not in tests' default services; by hand in a scratch
profile: the content index finds a file changed while Glint was closed; `Ctrl+Alt+G` in Notepad's Open dialog jumps
to the folder shown; a plan set to daily with its last run a day old backs up within about a minute of start; start
time (M0) unchanged.

## Part B. Removals

Each removal is one commit: delete the code, its tests, its strings (`crates/glint-model/src/i18n_ar.rs` too), its
menu and palette entries, its Settings section, its MCP tools (`crates/glint-core/src/mcp.rs`), its README, docs and
website mentions, then drop the dependency and let `cargo check --workspace --all-targets` name what is now unused
(follow the `dead_code` warnings until clean). Find every touchpoint with the grep given; the files listed are where
most of it is. **Read B6 before deleting B3's and B4's modules** (it needs their paths).

**B1. Mermaid.** `rg -i "mermaid|merman" crates`: `components/preview/diagram.rs` (keep its Graphviz half and
`layout-rs`), `preview/markdown.rs`, `preview/small.rs`, `preview/svg_image.rs`, `preview/choose.rs`,
`glint-model/src/small_kinds.rs`. Remove `merman` from `crates/glint-ui/Cargo.toml`. A ```` ```mermaid ```` block and
a `.mmd` file show as code and text. Math (RaTeX) and Graphviz stay.

**B2. Built-in Typst, `.typ` preview, Export as PDF.** `rg -i typst crates`: delete
`crates/glint-ui/src/typst_engine.rs`, `components/preview/typst_view.rs`, the `export_pdf` half of
`dialogs/doc_convert.rs` and `MenuEvent::ExportPdf` (`components/menus/event.rs`, `item.rs`, `kinds.rs`), the Typst
kind in `preview/choose.rs` and `preview/frame.rs`, `typst_engine::warm_fonts()` at `startup.rs:78` (a full system
font scan at every start goes with it), the `wave11` self-test, math's and diagrams' PDF-export code. Remove `typst`,
`typst-pdf`, `typst-render`, `typst-kit` from glint-ui. In glint-core delete `docconvert::typst_preview`; **keep**
`compile_typst` and the `typst` entry in `tools.rs` (Convert document → PDF uses the tool). `.typ` previews as text.

**B3. Paired devices.** `rg "glint_core::devices|devices::devices_|RemoteKind::Device|remote::device|iroh" crates`:
delete `crates/glint-core/src/devices.rs`, `remote/device.rs` and the `"device"` arm in `remote/mod.rs:220`,
`components/settings/devices.rs` and its calls in `settings/backend.rs:322-338`, `RemoteKind::Device` in
glint-model's store with its uses (`components/menus/mod.rs:465` and `:671`, the Send-to-device entries;
`sidebar/drop.rs`, `sidebar/mod.rs:1021`, `sidebar/rows.rs:305`), the device-files listener in `startup.rs`
(~line 497). **First move `settings::devices::remote_id`** to where its other users are (`settings/clouds.rs:6`,
`dialogs/cloud_setup.rs:196` and `:963`). Remove `iroh` from glint-core. LocalSend (Nearby), the sendme tool, and
on-demand remotes for clouds, S3, WebDAV and SFTP stay. ("device" in `metadata.rs` and "paired" in the compare code
are other things.)

**B4. Photo, speech and meaning search.** `rg -i "media_ai|photos::|meaning::|multilingual|speech::|shows:|says:|whisper" crates`:
delete `crates/glint-core/src/photos.rs`, `media_ai.rs`, `speech.rs`, `meaning.rs`, `multilingual.rs`, their
Settings rows (`components/settings/media.rs`, `tools.rs`, `backend.rs`), the `shows:`/`says:` filters
(`glint-model/src/entries.rs`, `components/address_bar/search.rs`; typed afterwards they are plain words), the
Meaning mode of the documents search (`doctext.rs:830-844`, `lib.rs:427`), the hand-off in `find.rs:380-388`, their
MCP tools (`mcp.rs:509-517`), `tools.rs:1292`. Remove `ort` and `unicode-properties` from glint-core.
**Subtitles must keep working (V):** Media's Subtitles task runs whisper.cpp with speech search's model
(`media_tools.rs:247`, `:566`: `speech::installed()`, `speech::model_path()`). Before deleting `speech.rs`, move the
Whisper model's path, installed check and download into `media_tools.rs` (or a small `whisper.rs`), let the
Subtitles task offer the download itself, and keep the `whisper` tool entry. Keep Windows' OCR (`ocr.rs`) and Read
aloud.

**B5. ast-grep leftovers (V).** The Text screen went in 0.11 (README line 331) but its engine stayed: delete
`run_ast_grep`, `ast_grep_search`, `ast_grep_apply`, `read_file_content`, `read_file_chunk`, `doc_lines` in
`crates/glint-core/src/lib.rs`. *Corrected while building:* the MCP server's code search still runs ast-grep, so
`codesearch.rs` keeps the one function that words its command line (`args`) and the `ast-grep` tool stays, worded
as the MCP server's (tools.rs, docs/tools.md, the installer's row).

**B6. What stays on users' PCs, and old state (V: needed; the design is yours).** Before deleting B3 and B4, list from
their code every file and folder they wrote under Glint's data folders (`tools::data_dir()`: the CLIP model and ONNX
Runtime ~240 MB, Model2Vec 30/128 MB, the photo and speech libraries (JSONL), device keys and pairings; **not**
Whisper's model, which Subtitles keeps). Add `glint_core::retired::sweep()`: run once per start on a background thread
(with A1's block), it deletes exactly those paths if present and nothing else, and is covered by a test with a temp
data folder. Loading must tolerate old files: a `state.json` with the removed settings and with remotes of kind
"device" loads (those remotes and their `ondemand.json` mounts are dropped, the placeholders unmounted); add a test
with such a fixture. The release notes say the downloads were removed.

**B7. Unreached code (V for the ones named).** `python scripts/dev/unreached-fns.py` lists it (2026-11-14: glint-core
29 functions, glint-model 11, glint-ui 151). Delete, then follow the warnings:
- glint-core: `markdown.rs` (`markdown_html`, `render`; then the `comrak` dependency); `host.rs` (`app_config_dir`,
  `app_cache_dir`, `app_log_dir`, `resource_dir`, `home_dir`, `is_visible`, `headless`, `async_runtime::handle`);
  `shell::clipboard_text` (all three); `sandbox::sandbox_available`, `textenc::text_cancel`,
  `versions::shadow_copies`; `meaning_status` goes with B4. (`sheet::load_parquet` looks unused but `sql.rs:164`
  calls it: keep it.)
- glint-model: `entries::sort_entries`, `merge_sorted` and `prop_test_now`, `color::hsl_of`, `columns::grid_templates`
  and `drop_class` (CSS for the web page), `columns::folder_kind`, `groups::group_of`, `i18n::language_of` (tests
  only), `cast::output_until`, `json_tree::to_json`, `file_types::preview_kind`, `paths::clear_shadow_names`,
  `store::remove_from_history`.
- glint-ui: about 150 `pub` accessors named only by tests (`run_state`, `again_input`, `shown_cells`,
  `drawn_pages`…): mark them `#[cfg(test)]` or delete them. The script's second list adds `svg_image::clear`,
  `held`, `peek` (test helpers), `platform/tray::configure`, `dialogs/backup::can_back_up`, `preview/sheet::is_workbook`,
  `dialogs/media_tools::name_of`, `preview/quick_look::current`.

The script matches by name, so check each hit with `rg` before deleting. Its second list also names the A1 starters
(`content::start`, `ondemand::start`, `dialogs::listen`): those are wired up, not deleted. Ignore the test functions it
lists in `wof.rs`, `ioring.rs`, `tags.rs` and `drivehealth.rs` (real-PC checks run by hand).

## Part C. A smaller, faster-building exe

**C1. One task harness, not thousands (V).** Every engine call is `core_bridge::run(future)` →
`host::async_runtime::spawn::<F>` and, inside, `fsx::blocking(closure)` → `spawn_blocking::<F, R>`: tokio's harness is
compiled once per future and per closure type (190 + 190 call sites). In `crates/glint-core/src/host.rs:470-482` keep
the signatures and box inside:
`RUNTIME.spawn(Box::pin(future) as Pin<Box<dyn Future<Output = F::Output> + Send>>)` and
`RUNTIME.spawn_blocking(Box::new(run) as Box<dyn FnOnce() -> R + Send>)`. That leaves one instantiation per *result*
type. If the size script still shows tokio above ~2 MB, erase the result too: a non-generic
`spawn_boxed(Pin<Box<dyn Future<Output = ()> + Send>>)` with the answer sent through a one-shot channel in
`core_bridge::run` (`crates/glint-ui/src/core_bridge.rs:322`) and `fsx::blocking` (`fsx.rs:26`); a panic must still
come back as `Err` (the test in `core_bridge.rs` near line 428 says so). *Done when*: tokio's share is measured
before and after, tests pass, bench unchanged.

**C2. Compile cold code for size (measure, then keep or drop).** In the root `Cargo.toml` add
`[profile.release.package.<name>] opt-level = "s"` for the big crates that never sit on a hot path, then for
`glint-ui` itself (most of its 14.6 MB, and the gpui and tokio code made for its types, follows its setting). Order:
(a) cold dependencies: `gix`, `parquet`, `calamine`, `lofty`, `rusqlite`/`libsqlite3-sys`, `age`,
`alacritty_terminal`, `sevenz-rust2`, `zip`, `pdf-extract`, `lopdf`, `hyper`, `reqwest`, `rustls`, `syntect`,
`markdown`, `layout-rs`, `ratex-*`, `jaq-*`, `fend-core`, `nom-exif`, `msi`, `resvg`, `usvg`; (b) `glint-ui` at
`"s"`, else `2`. Leave at 3: `glint-core`, `glint-model`, `shadui-index`, `gpui-*`, `rayon*`, `tantivy`, `regex*`,
`image` and its codecs. *Gate*: each step is kept only if the exe shrinks by 1 MB or more and M0's key-to-frame,
100,000-file open, scroll frame times and bench stay within 5%. One optional experiment at the end: `lto = "fat"`;
adopt only if the release build stays under about 12 minutes.

**C3. One copy of each big dependency (V: who pulls what).** `resvg`/`usvg` 0.45.1 come from vendored
`gpui-component`, 0.46.0 from vendored `gpui-pre` (0.47 leaves with Typst): move `vendor/gpui-component-0.9.1` to
0.46.0 (and `tiny-skia` with it). `windows` 0.58 comes only from vendored `gpui-base` and `gpui-component` (the rest
is 0.62): try moving both to 0.62; if it is more than mechanical, stop and leave a note. Vendored `gpui-pre`'s
`image` features include `exr`, `tiff`, `dds`, `hdr`, `ff`: keep only the formats Glint shows through GPUI's `img`
(check `components/preview` and `file_grid/thumbnail.rs` first). VENDOR.md rows for all three.

**C4. Dependencies nobody uses (V).** `comrak` (B7); `unicode-segmentation` and `unicode-normalization` in
`crates/glint-model/Cargo.toml` (no use in its sources); `mimalloc` in `crates/glint-core/Cargo.toml` (only
`crates/glint/src/main.rs` uses it). After B and C, prune `windows` features no file uses any more in `glint-core`
and `glint-ui` (remove one, `cargo check`, repeat), and confirm with `cargo tree -d --target x86_64-pc-windows-msvc`.

**C5. serde derives that serialize nothing (S, low priority).** 153 `derive(Serialize…)` in glint-core date from
the JSON bridge. Where a type no longer crosses JSON (not emitted, not saved, not sent to a worker process or MCP),
drop the derive. Only if C1-C4 leave time; the gain is about 1 MB and build time.

## Part D. The window: work nobody sees (all V unless marked)

Every arrow key ends in `apply_selection` → `cx.notify()` on the explorer, and every view that observes the explorer
runs, both panes' hidden views included. Measure each with a span first.

| # | What | Where | Change | Done when |
| :--- | :--- | :--- | :--- | :--- |
| D1 | Any settings change re-applies the whole theme: `state::update` always calls `theme::apply`, which enumerates every installed font family twice (`installed_family` → `all_font_names()`), rebuilds the tokens and refreshes every window. 68 callers, among them each Ctrl+wheel zoom notch, sort and group clicks, column and preview resizes, show hidden | `state.rs:57`, `theme.rs:681`, `theme.rs:757` | Re-apply the theme only when theme, accent, backdrop, font size or language changed (compare before and after); resolve the two font families once (cached until `theme::init` runs again) | Ctrl+wheel zoom: no `all_font_names` in the frame log's spans; zoom key-to-frame drops |
| D2 | A folder opened emits `MemoryChanged` twice (`remember_last_folder`, then after `record_visit`), each cloning visits (300), folder views (500) and the session, then `update_app` deep-clones the whole `AppState` for the saver; `Persistence`'s observers then rebuild (sidebar, Quick Search's recents while its window is hidden, the jump list: observers V, their cost S) | `explorer/nav.rs:497-502`, `nav.rs:569`, `startup.rs:687-700`, `persistence.rs:520`, `quick_search/mod.rs:201`, `startup.rs:1099` | One emit per navigation; the saver takes its snapshot when it writes (a dirty flag), not per change; observers compare the part they show (visits, pinned) and return early; Quick Search refreshes its recents when it is shown | `nav shown` for 20 folders in a row; one `MemoryChanged` per open in a test |
| D3 | The sidebar rebuilds all its rows on every explorer notify (clones pinned, saved searches, remotes, known folders, drives; `build_rows`; `key_of` lowercases per row) and its tree never closes the ancestors it revealed, so it slows over a session (S for the growth) | `sidebar/mod.rs:218`, `mod.rs:279-376`, `sidebar/rows.rs:355`, `sidebar/tree.rs:56`, `mod.rs:424` | Refresh only when its inputs changed (path, listing pointer, the three lists, settings); cap or prune the revealed tree | arrow-key spans show no sidebar work; rows identical in tests |
| D4 | 3-5 linear `position_of` scans per arrow key (each compares paths over the whole list) | `explorer/rows.rs:279`; callers `file_list/mod.rs:710`, `:788`, `status_bar/summary.rs:268`, `explorer/select.rs:78`, `file_grid/mod.rs:359` | Keep the focused row's index beside the selection (checked against the row before use), or a lazy path → index map on `Rows` | arrow key in 100,000: key-to-frame |
| D5 | With several items selected the status bar hashes every row's path and clones every selected path on each notify (Ctrl+A in 100,000 = 100,000 `String`s per key); the status bar and each pane head recompute a 4,000-entry spectrum per notify | `status_bar/summary.rs:262-280`, `summary.rs:254`, `pane_head/mod.rs:117` | Use `Rows::chosen` (written for exactly this, `rows.rs:303`) and return counts and bytes without cloning paths unless asked; cache summary and spectrum by listing pointer + sizes version + selection version | Ctrl+A then arrows in 100,000 |
| D6 | `sync_readmes` scans the whole listing for a README on every explorer notify, for both panes | `workspace.rs:435-451`, `components/git/words.rs:398` | Remember the answer per listing (`Arc` pointer) | span gone on arrow keys |
| D7 | Start: each pane's `ThisPc::new` asks every physical drive for its health (SMART/NVMe IOCTLs, can wake a sleeping disk) and makes its own `IndexModel`, before the window shows; all ten views of the second pane are built (and observe the explorer) though one pane is shown | `this_pc/mod.rs:162-178`, `workspace.rs:184` | Ask for drive health when This PC is first shown; one shared `IndexModel`; (larger, optional) build pane 1's views when the second pane opens | start time and memory at rest (M0) |
| D8 | Hidden views that work anyway: `FrequentFolders::rebuild` ×2 panes per notify; the address bar's `Seen::of` and `t!("Search {folder}")` per notify; the preview pane's `.git` scan of every new listing on the UI thread; each arriving thumbnail notifies four observers (S) | `frequent_folders/mod.rs:110-132`, `address_bar/mod.rs:283-386`, `preview/pane.rs:59-66`, `file_grid/thumbnail.rs:850` | Rebuild only while shown or when visits changed; guard by inputs; take `repo` from the reading thread (the listing already knows its entries); coalesce thumbnail notifies to one per frame | spans; first view of a folder of 500 pictures |
| D9 | Whole-list copies: grouping deep-clones every row (then `group_rows` copies again); `match_selection` runs `encode_utf16().count()` over every entry on the UI thread when a listing arrives with something to select; a search inside files clones the whole hit map and rebuilds entries per event batch (quadratic) | `explorer/rows.rs:422`, `explorer/nav.rs:185-196`, `explorer/inside.rs:586-631` | Group over indices or `Arc`ed rows; match on the reading thread; append batches (throttle as `find.rs`'s `worth_waiting` does) | grouped filter keystroke in 20,000; a search with 50,000 hits |
| D10 | `Prefs::read` clones the column list and both exclusion lists to read one bool (S) | `nav.rs:505`, `:548`, `model.rs:453`, `:468` | Read the field | - |

## Part E. The engine

| # | What | Where | Change | Done when |
| :--- | :--- | :--- | :--- | :--- |
| E1 (V) | The `copy-queue` event clones every job with `made` (a from/to pair per file copied so far) and `errors`, turns it into a JSON `Value`, and the UI clones and decodes it, every 150 ms: quadratic over a big copy, and finished jobs are sent again and again | `glint-core/src/queue.rs:34-59`, `:126-141`, `host.rs:247`, `glint-ui/src/explorer/queue.rs:87` | Send progress numbers per tick; `made` and `errors` once, when a job finishes (or as deltas); nothing for jobs that did not change | `npm run bench -- --copy 5000` and a copy of 50,000 small files: UI frame times flat |
| E2 (V) | Text search: `rg --json` lines (begin, end, context too) are batched into a JSON event, deep-cloned and parsed line by line on the UI thread; no `--max-columns`, so a hit in a minified file carries its whole line | `lib.rs:337-398`, `:518-542`, `explorer/model.rs:824`, `explorer/inside.rs:255` | Forward only `match` and `summary` lines; cap line length (`--max-columns` with a preview; check how the UI shows a cut line); deliver through a typed `core_bridge::stream` channel and parse off the UI thread | a search with 100,000 matches; a hit in a 5 MB one-line file |
| E3 (V) | Selecting a folder counts its contents through `fsx::list_dir`: read, natural sort, encode, decode, to count three numbers | `components/preview/sources.rs:150-174` | A `fsx::count_dir` that only reads (`read_dir`, no sort, no encode) | arrowing over folders of 100,000 |
| E4 (V) | Thumbnails travel as BMP bytes: encoded, copied whole again into the answer (`Thumb::encode`, memory hits included), decoded by the UI; every request first hops to the blocking pool for `archive::thumbnail_source` | `thumbs.rs:72-80`, `:399-419`, `file_grid/thumbnail.rs:128` | Hand BGRA pixels over typed (an `Arc`), as `list_folder` hands entries; skip the archive hop for paths that are not inside an archive | first view of 500 pictures; allocations |
| E5 (S) | Song preview reads tags twice and base64-encodes a cover it throws away; `media_details_many` formats display strings its callers drop and opens each picture twice | `media.rs:79`, `:145`, `:209-286`, `:328-334`, `preview/sources.rs:204`, `player/sources.rs:97-143` | A `cover` flag off for rows; the cover as bytes once; a numbers-only variant for row facts | - |
| E6 (V timers; S rest) | Idle wakeups: folder rules start three threads (500 ms, 2 s, 1 h) the first time a rule is on and never stop them; the rules tick holds the engine lock across `metadata()` | `rules.rs:565-582`, `:454-502` | Run the timers only while a rule is on; stat outside the lock | idle CPU (M0) with no rule on equals a profile that never had one |
| E7 (S) | The Git watcher is recursive over the whole repository (`target/`, `node_modules/`); each event lowercases and replaces its path and takes a global lock even for noise; `git_view` discovers the repository twice, lowercases every index entry at the root, and polls a watchdog every 10 ms | `git.rs:189-339`, `:531-567` | Filter noise without allocating and before the lock; build `tracked` only when needed; one discover | a `cargo build` in an open repository: Glint's CPU |
| E8 (S) | `proc::locate` scans PATH and four package managers on every call, also for availability checks; `tags::read` opens `path:glint.tags` for every visible row, on any drive | `proc.rs:148-191`, `tags.rs:21-28`, `row_facts.rs` | Cache locations (forget on install and uninstall); skip tags on remote and non-NTFS listings | a network folder's rows |
| E9 (S) | Read-ahead listings for tabs never opened stay encoded in `startup::WARM` for the process's life | `glint-core/src/startup.rs:17-60` | Drop what was not taken after a minute | memory at rest |
| E10 (S, if time) | Still encoded then decoded: index and fd results (`find.rs:240-286`, `explorer/source.rs:1012`), listings of archives, Recycle Bin, Recent, Gallery, remotes (`fsx.rs:79-135`), the jq worker's answer, QR as SVG text | as given | Hand them over typed, as `list_folder` does | search of 500,000 hits |

## Part F. The index service (`crates/shadui-index`)

No change to the pipe protocol: 0.12's app must work with a 0.9-0.11 service and the reverse. The installed service
here is 0.9.0 and replacing it needs the user's OK (UAC): prove these with unit tests and an `#[ignore]`d bench test
that builds a synthetic tree of about 2 million names and times `Tree::search` and `rank` before and after.

| # | What | Where | Change |
| :--- | :--- | :--- | :--- |
| F1 (V) | Every matching name walks all its ancestors to the scope, testing hidden and every name exclusion (~40 globs from Quick Search's defaults) at each; only the path-term branch remembers the last parent | `tree.rs:905-910`, `:1008-1025`, `query.rs:920` | Remember the answer per parent inside each chunk (the last parent, then a small map): siblings cost one walk |
| F2 (V) | A newer key press does not stop the scan before it: `tree.search` and ranking run to the end under the read lock, and the closed pipe is noticed only at the first write; fast typing stacks full scans up to the 16-client cap, then the app falls back to fd | `engine.rs:629-653`, `:799-843`, `pipe.rs:151-200`, `find.rs:270-295` | A stop flag per connection, set when the client has gone (peek the pipe between chunks, or a watcher), checked per chunk in `search` and before ranking |
| F3 (V) | Any query with `ext:`, `size:` or a path asks the service for its status first (`same_service`), and `status` counts every record of every drive under the lock | `find.rs:301-315`, `engine.rs:573-587`, `tree.rs:1105-1124` | App: remember the service's version for the session (forget it on a pipe error and after a service update). Service: keep file and folder counts as it applies changes, or cache them per tree generation |
| F4 (S) | A ranked search collects every match (three copies of the ids), ranks them all, then keeps `max` | `tree.rs:875-1000` | Only if 1-2 letter queries are still slow after F1-F3: keep the best `max` per chunk |
| F5 (S) | Idle: the journal is polled every 250 ms per drive, drives found again every 60 s; totals are recomputed hourly under the write lock and the snapshot saved under the read lock | `engine.rs:28`, `:172`, `:397-469`, `volume.rs:519-529` | A blocking journal read with a timeout instead of polling (stop must stay quick); the rest only noted |

## Part G. Clean-ups

- **G1 Manifests.** Root `Cargo.toml`: the first line still says "local branch gpui-migration; never pushed".
  `crates/glint-model/Cargo.toml`: drop the per-module `[features]` and the 21 `#[cfg(feature = …)]` in its `lib.rs`
  (the waves are over), fix "ported from the TypeScript in src/lib". `crates/glint-ui/Cargo.toml`: drop the empty
  `[features]` block and the wave notes. `crates/glint-core/Cargo.toml`: comments that say "already in the build
  through Tauri", the empty `[target.'cfg(not(any(android, ios)))']` comment about Tauri's updater, the description.
- **G2 Docs.** In `docs/gpui/`: keep `STANDARDS.md` (cut §4 "Porting TypeScript" and §6 "src-tauri to glint-core"),
  `VENDOR.md`, `rtl-fix.patch`; delete `migration-plan.md`, `TRACKER.md`, `parity-strings.md`, `previews-native.md`,
  `UI-SKELETON.md` if nothing links to them, and `reference/spike-*.rs`; fix links (`scripts/gpui-installer.mjs`
  cites the migration plan). README, `docs/tools.md`, CONTRIBUTING's code map and `site/` for everything part B
  removed; a "New in 0.12" section with a plain **Removed** list. Leave the website undeployed (the user's word).
- **G3 Notes in the code.** Settle the 15 `TODO(coordinator)` (`rg "TODO\(coordinator\)" crates`: private copies of a
  helper "until it is exported": export it once and delete the copy, e.g. `verbs::launch_exe`, `shell::in_sta`,
  `sftp::UNKNOWN_HOST`, the two rankers in `quick_search/search.rs` and the palette, `file_list::drop_path`) and the
  7 `PORT ME` (`rg "PORT ME" crates`: do it or delete the note; `workspace.rs:1010` hides the native preview under a
  menu). Reword comments about the web app only in files already being edited.
- **G4 Scripts.** `package.json`: `gpui:installer` → `installer` (update docs/releasing.md, release.yml and
  `scripts/release.mjs` callers); `.gitignore`'s web-era lines.
- **G5 Self-test hooks.** `crates/glint-ui/src/selftest.rs` (660 lines, debug only): delete the wave scripts the e2e
  harness now covers (`wave8`, `wave9`, `wave11`, `look13`, `look14`); keep `ops:` and `show:` if still used for
  captures.
- **G6 Vendor.** Delete `vendor/gpui-pre-0.3.12/examples` (56 files, 5 MB) and its 44 `[[example]]` entries, and the
  vendored crates' own `Cargo.lock`, `tests/`, `benches/` if the build and `cargo metadata` do not need them; a
  VENDOR.md row.
- **G7 Housekeeping, only on the user's yes (destructive):** worktrees `.claude/worktrees/intelligent-benz-a0d24e`,
  `objective-wiles-12a035`, `next-release`, `D:/glint-build/next`; branches `next`, `worktree-next-release`,
  `gpui-migration` (its worktree holds the build-cache junction: keep it until every build uses
  `CARGO_TARGET_DIR`); the remote branch `origin/imgbot`.

## Order and size

1. **M0 baseline** (half a day with the scenario scripts).
2. **A1** (S). 3. **B5, B7, C4** (S each: pure deletions). 4. **B1, B2** (M each), then **B6 + B3, B4** (M each).
5. **C1** (S), **C3** (S-M), **C2** (M: mostly build and measure time). Run M0 again.
6. **D1, D2, D5, D4** (the per-key and per-folder ones; S-M each), then **D3, D6, D7, D8, D9, D10**. Run M0 again.
7. **E1, E2, E3, E4** (M each), then **E6, E8, E9**, then E5, E7, E10 as time allows.
8. **F1, F3** (S), **F2** (M), F4 and F5 only if measured as needed.
9. **G1-G6**, README and notes, the final M0, the full check set (rule 6), then stop and report: releasing (the
   version bump to 0.12.0, `npm run release`, Scoop, winget, MSIX, the website) happens only on the user's word.

## Verification (end to end)

- The M0 table, before and after, below, with `size-by-crate.py --compare` against the 0.11.0 run.
- `cargo test --workspace`, clippy, fmt, `npm run test:signing`, `npm run licenses -- --check`, `cargo deny check`;
  e2e 18/18 on the debug build (ask before taking the mouse), each failing check run again alone.
- By hand in a scratch profile (the optimized dev-behaving build, a made-up folder in a scratch folder): A1's four
  services; a Markdown file with mermaid, math and Graphviz blocks (code, drawn, drawn); a `.typ` file (text);
  Convert document → PDF still offers the tools; Settings shows no Devices, photo, speech or meaning rows; an old
  `state.json` with those settings and a device remote loads; `retired::sweep` removes a planted model folder and
  nothing else; Subtitles on a video still runs; a copy of 50,000 files; a text search with 100,000 matches; Quick
  Search typed fast.
- Not verifiable here without the user: the real index service (UAC), Narrator, a second PC, the Store copy.

## The audit scripts (`scripts/dev/`)

- **`size-by-crate.py [Glint.exe]`** reads the exe's PDB (beside the exe) with Windows' dbghelp.dll and credits each
  symbol's code to the crate its mangled name names (generic code counts for the crate that defines it unless that
  is the standard library). It prints the sections, the biggest crates, and what removing each direct dependency
  would take out (it and every crate only it brings in, from `cargo metadata`); `--group NAME=dep,dep` counts a set
  together, `--json` saves a run, `--compare` prints the change. Thin LTO hides private symbols, so shares are a few
  percent off: for a decision, build with and without the thing and compare the files.
- **`unreached-fns.py`** lists functions nothing in the shipping app reaches by name (test code set aside), and free
  functions never named with their module (`content::start`). It matches by name, so check each hit with `rg`.

## Results

Measured on this PC (2026-11-14), release builds (`cargo build --release -p glint -p shadui-index`), sizes with
`scripts/dev/size-by-crate.py`. The app was not run for timings (the user's own Glint was open, and they asked for no
app runs): the rows that need the window are empty, and so is the setup's size (makensis isn't on this PC's PATH).

| Measure | 0.11.0 | after A-B | after C | 0.12.0 (A-G) |
| :--- | :--- | :--- | :--- | :--- |
| `Glint.exe` | 174,127,104 B | 100,507,648 B | 93,347,328 B | 93,284,352 B (-46%) |
| setup | 45,752,959 B | | | not built |
| .text / .rdata | 124.5 / 43.2 MB | 73.3 / 23.2 MB | 68.2 / 21.6 MB | 68.2 / 21.6 MB |
| tokio's code | 5.87 MB | 4.62 MB | 0.76 MB | 0.92 MB |
| crates compiled | 964 | 746 | 724 | 724 |
| release build of Glint's crates (after `cargo clean -p glint-ui -p glint-core -p glint`) | 269 s | | | 128 s |
| engine bench: list System32 (all) | 1.49 ms | | 1.48 ms | 1.50-1.52 ms |
| engine bench: copy 5000 small files, one at a time / 16 at a time (single runs, Defender's share varies) | 1,861 / 1,078 ms | | 1,515 / 1,234 ms | 1,449 / 968 ms |
| engine bench: index search and typo search (the installed 0.9 service answers: not this build's code) | 7.2 / 28.3 ms | | 5.5 / 30.0 ms | 8.2-8.5 / 36.5-37.8 ms |
| index service, synthetic 1.8 million files, "report" under 40 exclusions (F1's ignored bench) | | | | 11.3 to 7.9 ms |
| cold start | | | | |
| arrow key in 100,000 (key to frame) | | | | |
| Ctrl+wheel zoom (key to frame) | | | | |
| 20 folders opened (`nav shown`) | | | | |
| memory at rest (private / working set) | | | | |
| idle CPU over 120 s (shown / in the tray) | | | | |

Per part (sizes): B took 73.6 MB out of the exe (Mermaid 14.5, Typst and its crates ~15, iroh and QUIC ~3.4, the
rest their dependencies and the code that used them); C1 2.7 MB (tokio 4.62 to 0.94 MB); C3 and C4 2.7 MB; C2 1.8 MB.

**e2e** (`tests\e2e\run.ps1` on the debug build, 2026-11-14, with the user's OK): 17 of 18 pass. It found two
real bugs, both fixed and committed: a line the content index found showed no line number (an empty label won), and
the index, started at launch again by A1, answered before it had caught up (a new file wasn't found). `quit` fails:
closing the window hides it to the tray instead of quitting, and the 0.11.0 build fails the same way on this PC, so it
isn't from 0.12. The harness's "fresh" profile isn't fully fresh here (the content index's settings, and what its
processes write to the registry, are shared with the user's own copy). Debug links now hit the PDB limit at times
(LNK1140): delete `debug\Glint.pdb` and `debug\deps\Glint.pdb`, or link with `-C link-arg=/DEBUG:NONE`.

**Done**: A1; B1-B7 (B5 corrected: ast-grep stays for the MCP server); C1-C4 (C2's cold crates; glint-ui at "s" and
fat LTO weren't tried, since judging them needs app timings); D1-D10; E1, E2 (with a fix: ripgrep 15's summary was
never read), E3, E4 (the archive hop only), E6, E7 (the noise filter only), E8 (program locations only), E9;
F1, F3; G1-G6; notices regenerated (cargo-about 0.9.3) and cargo-deny 0.20.2 passing.

**Not done, and why**: D2's deferred save snapshot, D3's pruning of the revealed tree, D7's lazy second-pane views and
D8's thumbnail coalescing (small, and their worth needs app timings); E5, E7's discover and tracked-set work, E8's
tags on non-NTFS listings (larger changes, S items); F2 (a stop flag per pipe connection, which helps only once the
installed 0.9 service is updated, with UAC), F4 and F5 (not shown to be needed). The app-run measures above: not run,
at the user's word. E4's typed pixels, E10 and C5 were done afterwards, in the wave below.

### Wave: no more encoding between the engine and the window

The user's ask (2026-11-14): "cleaning and eliminate Tauri-Era, e.g Encoding / JSON Round-Trips Running In-Process".
The engine and the window are one process since 0.10, but much of what crossed between them was still packed for the
web page and unpacked again. Nine commits after `f7dbdc0`, each with the whole test suite passing:

| What crossed | Before | Now |
| :--- | :--- | :--- |
| Every engine event (folder changed, copy queue, rules, search inside files, backups, files from nearby devices, rclone's sign-in page, Gallery dates, Git changes) | `serde_json::Value`, matched by name in each subscriber and decoded field by field or into a mirror struct | one `host::Event` enum, matched by variant |
| Name search results | JSON text events plus `encode_items` bytes (sizes as doubles, name lengths in UTF-16 units) | `FindEvent` values with the entries as read |
| Listings of archives, remotes, the Recycle Bin, Recent, the Gallery, folders read ahead at start | the same binary layout, decoded by glint-model | `fsx::Listing` values (folders keep the archive's or remote's details; items keep their own paths) |
| Matches of a search inside files | ripgrep's JSON lines parsed in the window; Documents and the content index wrote ripgrep-style JSON (and a made-up summary) only for that | read once in the engine (a long line still cut around its first hit), sent as `TextMatch` values |
| Disk-usage maps | packed like a listing, the archive format as a number | a `Measured` value, made into the map from its items |
| Thumbnails | each a BMP, copied again behind an 8-byte header for every answer, decoded by GPUI's BMP reader | the BGRA pixels themselves, an `Arc` shared with the engine's memory; a PDF's page decoded to pixels by Windows |
| Video scrub frames, jq preview answers, the hex views' bytes | headers of numbers before the data (the frames in RGBA, swapped back) | `VideoFrames` in BGRA, `Queried`, `BytesWindow`, `(size, bytes)` |
| Song covers, New-menu icons | base64 data URLs (icons were encoded as PNG first) | the cover's own bytes; the icon's pixels |
| The terminal's folder reports and exit | JSON text beside the shell's bytes on an untyped channel | `TermOutput` |
| Staging results and progress, archive peeks and reads, tool status and install progress, MCP's setup, the text-search request, the compress format, the index's history, every saved remote and folder rule | turned into JSON and read back, because core's fields were private (or "the same JSON shape") | public fields, read as they are; saved choices handed over by the word they're saved as (pinned to their serde names by a test) |

What went with them: `list_dir`, `encode_listing`, `encode_items`, `encode_map`, `encode_bmp`, glint-model's
`decode_listing`, `decode_items` and `decode_usage`, `ARCHIVE_FORMATS` and the format codes, the `Emitter` trait,
`host::ipc::InvokeResponseBody` and `Response`, the window's copies of `JobView`, `NewKind` and the history types, its
base64 reader, the `get_webview_window`/`webview_windows` names, and 131 of glint-core's 164 serde derives (the 33
left are what the MCP server answers with, what worker processes and the index service exchange, and what's saved).

Measured: the code shrank by about 2,000 lines (140 files, +2,097 / -4,077); `Glint.exe` 93,284,352 -> 92,810,752 B
(-0.47 MB: glint_ui -0.22 MB and gpui -0.30 MB of code, glint_core +0.37 MB, the parsing that moved into it); the
release build of the changed crates took 133 s. Tests: core 293, model 231, window 2,330, index 92, all passing;
clippy `-D warnings` clean. Per-event costs (CPU, allocations) weren't timed: the window wasn't run.
