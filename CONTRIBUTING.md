# Contributing

Thanks for helping make Glint faster and better! Bug reports, ideas, docs fixes and code are all welcome.

## Before you start

-   **Bugs:** open an issue with the bug template. The app version is in **Settings → About**.
-   **Features:** open an issue first for anything bigger than a small fix, so we can agree on the approach before you spend time on it.
-   **Good first issues** are labelled [`good first issue`](https://github.com/shadijhade/glint/labels/good%20first%20issue).

## Set up

You need Windows 10 or 11 and [Rust](https://www.rust-lang.org/tools/install) (stable, MSVC toolchain, with the Visual Studio C++ build tools). [Node.js](https://nodejs.org/) 22 or newer is needed only for the release scripts in `scripts/` (they use Node's own modules, so there is no `npm install`), and [NSIS](https://nsis.sourceforge.io/) 3 only to pack an installer.

```bash
git clone https://github.com/shadijhade/glint.git
cd glint
cargo run -p glint
```

The first build compiles GPUI and all the other dependencies and takes several minutes; later ones take seconds. Every build goes to `target\ui` (set in `.cargo/config.toml`). Until 0.10 Glint's interface was a web page (Tauri, React); it is now Rust all the way down, on [GPUI](https://www.gpui.rs/) with a patched copy in `vendor/` ([docs/gpui/VENDOR.md](docs/gpui/VENDOR.md)).

## Where things are

Five crates make a workspace (`Cargo.toml`): `crates/glint` is `Glint.exe` (a thin `main.rs`, its resources, the bundled tools and the installer), `crates/glint-ui` is everything on screen, `crates/glint-core` is the engine (files, search, archives, media, devices… with no window), `crates/glint-model` is pure logic with nothing to link against (sorting, formatting, paths, settings, text), and `crates/shadui-index` is the instant search service. The rules for UI code are in [docs/gpui/STANDARDS.md](docs/gpui/STANDARDS.md), and `crates/glint-ui/src/lib.rs` says in what order to read it.

| Path | What it is |
| :--- | :--- |
| `crates/glint-ui/src/workspace.rs`, `startup.rs`, `state.rs`, `persistence.rs` | The main window and what it holds; start-up, where the services meet; shared state; settings and the session saved to disk |
| `crates/glint-ui/src/explorer/` | The Files mode's state and actions: tabs and panes (`tabs.rs`, `tab.rs`), navigation (`nav.rs`), selection (`select.rs`), search (`find.rs`), file operations and their undo (`ops.rs`, `undo.rs`, `clipboard.rs`, `rename.rs`), archives and the Recycle Bin as places (`archive.rs`, `recycle.rs`), comparing panes (`compare.rs`) |
| `crates/glint-ui/src/components/file_list/`, `crates/glint-ui/src/components/file_grid/`, `crates/glint-ui/src/components/columns_view/` | The list (details), the grid of tiles and thumbnails, and Miller columns |
| `crates/glint-ui/src/components/` (`tab_strip`, `sidebar`, `address_bar`, `command_bar`, `status_bar`, `pane_bars`, `compare_bar`, `palette`, `menus`, `copy_queue`, `this_pc`, `frequent_folders`, `gallery_months`, `git`) | The chrome around the files: tabs, the sidebar and its folder tree, the address bar, the command bar, the status bar, the compare bar, `Ctrl+K`, context menus, the copy queue, This PC, and Git's chip and cards |
| `crates/glint-ui/src/components/disk_usage/`, `crates/glint-model/src/usage.rs` | Disk-usage maps: the squarified treemap layout, painting, colors by type, age or growth |
| `crates/glint-ui/src/explorer/inside.rs`, `crates/glint-model/src/text.rs` | Searching inside files as part of the one search (ripgrep, the content index, documents): the files found and their matching lines |
| `crates/glint-ui/src/components/preview/`, `crates/glint-model/src/hex.rs`, `crates/glint-model/src/cast.rs`, `crates/glint-model/src/json_tree.rs`, `crates/glint-model/src/markdown.rs`, `crates/glint-model/src/file_types.rs` | The preview pane and Quick Look: pictures, the Media Foundation player, PDFs and books, Markdown with math (RaTeX) and diagrams (Mermaid, Graphviz), JSON trees, tables, hex, asciinema playback, 3D models, programs and DLLs; and what kind of file is what |
| `crates/glint-ui/src/components/terminal/` | The terminal under the files: `alacritty_terminal`'s emulator, the shell's session (`session.rs`, ConPTY through `crates/glint-core/src/term.rs`), keys and links |
| `crates/glint-ui/src/components/settings/` | Settings, one file per page |
| `crates/glint-ui/src/dialogs/` | The dialogs: Clean up, Find duplicates, X-Ray, Compare two files, Send, Remote, Rules, Convert, Media, PDF tools, Backups, Drive health… (`drive_health.rs` is the exemplar) |
| `crates/glint-ui/src/core_bridge.rs`, `crates/glint-ui/src/actions.rs`, `crates/glint-ui/src/kit.rs`, `crates/glint-ui/src/theme.rs`, `crates/glint-ui/src/i18n.rs` | How a view calls the engine without waiting; commands and their keys; the controls every view uses (the only place gpui-component is touched); colors and sizes as tokens; every string (`t!`), right to left |
| `crates/glint-ui/src/platform/` | Windows-only code behind the window: one copy at a time and later launches (`launch.rs`), the tray, the window's place, drag and drop, file pickers, Windows' preview handlers (`preview_host.rs`), read-aloud |
| `crates/glint-ui/src/updater.rs`, `crates/glint-ui/src/old_settings/` | Finding and installing updates (latest.json, the signed installer); reading the old WebView app's settings at the first start |
| `crates/glint-model/src/store.rs`, `crates/glint-model/src/paths.rs`, `crates/glint-model/src/entries.rs`, `crates/glint-model/src/collate.rs`, `crates/glint-model/src/format.rs`, `crates/glint-model/src/rank.rs`, `crates/glint-model/src/plain.rs`, `crates/glint-model/src/grid.rs` | Pure logic: settings and saved things, paths and places (`shell:RecycleBinFolder`, `remote:<id>\…`), listings, natural sort, how sizes and times read, ranking folders by how often and recently they're opened, plain words in the search box read as filters, the grid's tile sizes |
| `crates/glint-core/src/fsx.rs` | Folder listing (Win32 `FindFirstFileExW`), with cloud files' (OneDrive's) states |
| `crates/glint-core/src/queue.rs`, `crates/glint-ui/src/components/copy_queue/` | The copy queue: one job at a time with CopyFileEx, paused in its progress routine, names taken asked about, each copy read back past the cache and compared (BLAKE3), moves that rename or copy and then delete |
| `crates/glint-core/src/gallery.rs` | Gallery, a place listed like a folder: the photos and videos below its folders (from the index, else walked), dated by when each was taken, kept in gallery-dates.json |
| `crates/glint-core/src/recent.rs` | Recent, a place listed like a folder: Windows' Recent Items, each shortcut's file at its own path |
| `crates/glint-core/src/newmenu.rs` | Windows' New menu: the kinds of file the PC's apps registered (`ShellNew`), made from their templates, bytes or commands |
| `crates/glint-core/src/archive/` | Archives opened like folders (zip, 7z, RAR, cabinets, installer packages, tar, compressed files): reading their list of entries, the folder tree, extracting on demand and into staging folders, safety limits ([docs/archives.md](docs/archives.md)) |
| `crates/glint-core/src/find.rs`, `crates/glint-core/src/sizes.rs`, `crates/glint-core/src/usage.rs` | fd name search, pdu folder sizes and disk-usage maps (or the index when it's on) |
| `crates/glint-core/src/shell.rs` | Copy, move, delete, rename (`IFileOperation`), clipboard, drag and drop (with its drag picture), Properties, and Windows' own Share, Eject and Mount, turning pictures and the desktop background, OneDrive's keep and free up |
| `crates/glint-ui/src/platform/window_state.rs` | The window's place, kept and put back; the taskbar button's Jump List (tasks, pinned and most-used folders), its progress bar, and Back, Forward and Up under its preview |
| `crates/glint-core/src/recycle.rs` | The Recycle Bin as a place: its items from the shell and their `$I` records, and emptying it ([docs/recycle-bin.md](docs/recycle-bin.md)) |
| `crates/glint-core/src/compare.rs`, `crates/glint-ui/src/components/compare_bar/` | Compare panes down through subfolders and by contents, and what Sync copies |
| `crates/glint-core/src/filediff.rs`, `crates/glint-ui/src/dialogs/compare_files.rs` | Compare two files: text side by side (Difftastic's structural diff, or line by line), pictures, and bytes |
| `crates/glint-core/src/markdown.rs`, `crates/glint-core/src/jq.rs`, `crates/glint-core/src/sheet.rs`, `crates/glint-core/src/data_lens.rs`, `crates/glint-core/src/sql.rs`, `crates/glint-core/src/pe.rs`, `crates/glint-core/src/media.rs`, `crates/glint-core/src/metadata.rs` | Glint's own previews' engines: Markdown (comrak), JSON, YAML and TOML as a tree with jq filters (jaq, in a process of its own), Excel, OpenDocument and Parquet tables, CSV and SQLite, Query (SQL, read-only), programs and DLLs, and photo, video and music details |
| `crates/glint-core/src/git.rs`, `crates/glint-core/src/repo.rs`, `crates/glint-ui/src/components/git/` | Git status in folders, the branch, `git:` filters and a changed file's changes, read with gitoxide (nothing written); a repository's card (onefetch, tokei) |
| `crates/glint-core/src/term.rs`, `crates/glint-core/src/projects.rs`, `crates/glint-core/src/scripts.rs` | ConPTY through portable-pty and the folder each prompt reports, the Run menu (package.json, justfile, Cargo, uv) and script actions from the Scripts folder |
| `crates/glint-core/src/codesearch.rs`, `crates/glint-core/src/doctext.rs`, `crates/glint-core/src/ocr.rs`, `crates/glint-core/src/meaning.rs` | The search inside files: documents (PDF, Office, OpenDocument, EPUB, read in worker processes), text in pictures (Windows' OCR) and meaning search (Model2Vec); code patterns (ast-grep) are still in core, with no place in the window since the one search |
| `crates/glint-core/src/cleanup.rs`, `crates/glint-core/src/dedup.rs`, `crates/glint-core/src/convert.rs`, `crates/glint-core/src/batch_rename.rs` | Clean up (Czkawka's modes too), Find duplicates (hard links, block clones), Convert pictures (rimage) and batch rename |
| `crates/glint-core/src/media_tools.rs`, `crates/glint-core/src/pdf_tools.rs`, `crates/glint-core/src/docconvert.rs`, `crates/glint-core/src/shrink.rs`, `crates/glint-core/src/wof.rs`, `crates/glint-core/src/backup.rs`, `crates/glint-core/src/vault.rs`, `crates/glint-core/src/songtags.rs`, `crates/glint-core/src/drivehealth.rs` | The things done to files: FFmpeg's Media menu, PDF tools (pdfcpu), Convert documents, Make smaller, compress in place (WOF), restic backups, age vaults, song tags, drive health |
| `crates/glint-core/src/xray.rs`, `crates/glint-core/src/sign.rs`, `crates/glint-core/src/check.rs`, `crates/glint-core/src/defender.rs`, `crates/glint-core/src/locks.rs`, `crates/glint-core/src/timeline.rs` | The File X-Ray (checksums, streams, signatures), YARA-X and Kingfisher checks, Defender scans, what's locking a file (Restart Manager) and recent changes |
| `crates/glint-core/src/share.rs`, `crates/glint-core/src/lan.rs`, `crates/glint-core/src/localsend.rs` | Send to another PC (sendme), Share on this network (dufs) and LocalSend |
| `crates/glint-core/src/remote/` | Remote connections as read-only folders: S3 (signed requests) and WebDAV over HTTPS, SFTP through Windows' own `sftp.exe`, rclone's remotes, secrets in Credential Manager ([docs/remote.md](docs/remote.md)) |
| `crates/glint-core/src/rclone.rs`, `crates/glint-ui/src/dialogs/cloud_setup.rs` | Clouds through rclone: its remote control (started when first needed), signing in step by step and in the browser, its remotes, how full they are, and sync runs |
| `crates/glint-core/src/rules.rs`, `crates/glint-ui/src/dialogs/rules.rs` | Folder rules: watching, telling when a file is complete, the actions, the log with Undo, and sync rules' runs |
| `crates/glint-ui/src/components/quick_search/`, `crates/glint-ui/src/platform/tray.rs` | Quick search from anywhere: the shortcut, its window, the tray and starting with Windows |
| `crates/glint-core/src/thumbs.rs` | Thumbnails from Windows' thumbnail cache (`IShellItemImageFactory`, `IThumbnailCache`) on worker threads, in a queue per tab |
| `crates/glint-core/src/watch.rs` | Live folders: notify watches the folders on screen |
| `crates/glint-core/src/verbs.rs`, `crates/glint-core/src/access.rs`, `crates/glint-ui/src/platform/launch.rs` | Folders from the command line and later launches (one copy runs at a time), File Explorer's *Open in Glint* entries and Win+E ([docs/explorer-integration.md](docs/explorer-integration.md)), and Get access for folders Windows keeps you out of |
| `crates/glint-core/src/mcp.rs`, `crates/glint-core/src/agents.rs` | The Model Context Protocol server AI agents start (`Glint.exe mcp`): its tools over the index, fd, pdu and ripgrep, and Settings → AI agents' setups |
| `crates/glint-core/src/shellmenu.rs` | Windows' own context menu (Show more options): the shell's `IContextMenu`, with the messages its submenus need while it's open |
| `crates/glint-ui/src/theme.rs`, `crates/glint-model/src/color.rs` | Windows 11's Mica and the accent color; colors as the theme's tokens |
| `crates/glint-core/src/openers.rs`, `crates/glint-core/src/calc.rs`, `crates/glint-core/src/zoxide.rs` | Open in… (the editors and terminals found), the calculator in `Ctrl+K` (fend), and folder history shared with zoxide |
| `crates/shadui-index/`, `crates/glint-core/src/index.rs` | The opt-in instant-search service (NTFS MFT + USN journal): names, folder sizes, maps, what grew and what moved ([docs/instant-search.md](docs/instant-search.md)); turning it on and off from the app |
| `crates/glint/binaries/` | Bundled `rg`, `fd` and `pdu` executables |
| `crates/glint/installer/` | The NSIS installer: its pages, hooks, look and artwork, and the **Extra tools** page ([crates/glint/installer/README.md](crates/glint/installer/README.md), [docs/releasing.md](docs/releasing.md#the-installer)) |
| `crates/glint-core/src/tools.rs`, `crates/glint-core/src/proc.rs`, `crates/glint-ui/src/components/settings/tools.rs` | Every program Glint drives: the bundled ones, and optional ones installed on demand with winget or from their GitHub release (pinned by SHA-256) ([docs/tools.md](docs/tools.md)); finding and running them, a lane per tab; Settings → Tools, and the install card a feature shows when its tool is missing |
| `vendor/` | The patched GPUI crates ([docs/gpui/VENDOR.md](docs/gpui/VENDOR.md)) |
| `tests/e2e/` | End-to-end checks that drive the real debug `Glint.exe` with clicks and keys ([tests/e2e/README.md](tests/e2e/README.md)) |
| `scripts/` | `npm run release` and its helpers (update signatures, winget and Scoop manifests), `npm run notes` (a release's notes, by git-cliff from `cliff.toml`) and `npm run licenses` (`THIRD_PARTY_CRATES.md`, by cargo-about) ([docs/releasing.md](docs/releasing.md)) |
| `site/` | The website, [glint.rafd.tech](https://glint.rafd.tech/): an Astro project of its own (GSAP, a WebGL field), published as a Cloudflare Worker ([docs/releasing.md](docs/releasing.md#the-website)) |

## Principles

-   **Speed is the feature.** Measure before and after, and keep the timings the status bar shows honest.
-   **Wrap the best tools.** Fast Rust CLIs run as sidecars, and their arguments are always built in Rust from typed options, never passed through from the UI.
-   **Behave like Windows.** File operations go through the shell, so Recycle Bin, conflicts and permissions work the way people expect.
-   **Nothing elevated by default.** Anything that needs administrator rights is opt-in and documented.

## Checks

Run these before you commit or open a pull request:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm run test:signing                       # if you touched scripts/lib/minisign.mjs
powershell -STA -File tests\e2e\run.ps1    # the real app, driven by real clicks: leave the mouse and keyboard alone while it runs
```

Try UI changes in the app with `cargo run -p glint`, and add a screenshot or GIF to the pull request. Release builds run once per app identifier and hand their folders to a Glint that's already running, so try a release build under its own name and identifier ([how](docs/explorer-integration.md#trying-it-without-touching-your-glint)).

## Adding or updating a crate

Prefer crates that are light and pure Rust, and say what one adds to `Glint.exe`'s size. After changing crates, run `npm run licenses` and commit `THIRD_PARTY_CRATES.md` with them, and run `cargo deny check` (both tools: `cargo install --locked cargo-deny cargo-about`). A crate under a license that `deny.toml` doesn't list needs an issue first. So does a new crate in the workspace itself.

## Updating a bundled tool

Download the `x86_64-pc-windows-msvc` build from the tool's official GitHub release, check its SHA-256 against the digest GitHub shows for the asset, and replace the file in `crates/glint/binaries/` (keep the `-x86_64-pc-windows-msvc.exe` suffix). Then update the version in `README.md` and `THIRD_PARTY_LICENSES.md`.

## Licensing

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
