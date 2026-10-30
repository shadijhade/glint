# Contributing

Thanks for helping make Glint faster and better! Bug reports, ideas, docs fixes and code are all welcome.

## Before you start

-   **Bugs:** open an issue with the bug template. The app version is in **Settings → About**.
-   **Features:** open an issue first for anything bigger than a small fix, so we can agree on the approach before you spend time on it.
-   **Good first issues** are labelled [`good first issue`](https://github.com/shadijhade/glint/labels/good%20first%20issue).

## Set up

You need Windows 10 or 11, [Node.js](https://nodejs.org/) 22.18 or newer, and [Rust](https://www.rust-lang.org/tools/install) (stable, MSVC toolchain).

```bash
git clone https://github.com/shadijhade/glint.git
cd glint
npm install
npm run tauri dev
```

The first build compiles all Rust dependencies and takes a few minutes; later ones take seconds.

## Where things are

| Path | What it is |
| :--- | :--- |
| `src/components/explorer/` | The Files mode UI: panes and tabs, list and grid, disk-usage map, sidebar (and its folder tree, `FolderTree.tsx`), address bar, preview, command bar, terminal, and the dialogs (Clean up, Find duplicates, X-Ray, Compare two files, Send, Remote, Rules…) |
| `src/lib/explorer.ts` | Explorer state and actions: tabs and panes, navigation, selection, search, file operations |
| `src/lib/fs.ts` | Typed wrappers for every Rust command |
| `src/lib/store.ts`, `paths.ts`, `entries.ts` | Settings, the session and saved things (in localStorage); paths and places (`shell:RecycleBinFolder`, `remote:<id>\…`); listings decoded from Rust's binary buffers |
| `src/lib/<feature>.ts` beside `src-tauri/src/<feature>.rs` | That feature's state in the UI and its calls into Rust (`share.ts`, `remotes.ts`, `meaning.ts`, `projects.ts`, `scripts.ts`…) |
| `src/lib/preview.ts`, `markdown.ts`, `jsonTree.ts`, `hex.ts`, `cast.ts`, `highlight.tsx`, `fileTypes.ts` | The preview's views (Markdown's links, the tree, hex rows, asciinema playback, syntax highlighting) and what kind of file is what |
| `src/lib/ripgrep.ts`, `text.ts`, `rank.ts`, `format.ts` | Text search's state and Replace all, ranking folders by how often and recently they're opened, and how sizes, counts and times read |
| `src/lib/usage.ts`, `diskUsage.ts` | Disk-usage maps: decoding and the squarified treemap layout; each tab's map |
| `src/lib/thumbs.ts`, `grid.ts` | Thumbnails for the tiles and rows on screen (asking, dropping, keeping them); the grid's tile sizes, columns and keys, and keeping its place as they change |
| `src/components/Results.tsx`, `Search.tsx` | Text search (ripgrep) |
| `src-tauri/src/fsx.rs` | Folder listing (Win32 `FindFirstFileExW`, binary transfer to the UI), with cloud files' (OneDrive's) states |
| `src-tauri/src/gallery.rs` | Gallery, a place listed like a folder: the photos and videos below its folders (from the index, else walked), dated by when each was taken, kept in gallery-dates.json |
| `src-tauri/src/recent.rs` | Recent, a place listed like a folder: Windows' Recent Items, each shortcut's file at its own path |
| `src-tauri/src/newmenu.rs` | Windows' New menu: the kinds of file the PC's apps registered (`ShellNew`), made from their templates, bytes or commands |
| `src-tauri/src/archive/` | Archives opened like folders (zip, 7z, RAR, cabinets, installer packages, tar, compressed files): reading their list of entries, the folder tree, extracting on demand and into staging folders, safety limits ([docs/archives.md](docs/archives.md)) |
| `src-tauri/src/find.rs`, `sizes.rs`, `usage.rs` | fd name search, pdu folder sizes and disk-usage maps (or the index when it's on) |
| `src-tauri/src/shell.rs` | Copy, move, delete, rename (`IFileOperation`), clipboard, drag and drop (with its drag picture), Properties, and Windows' own Share, Eject and Mount, turning pictures and the desktop background, OneDrive's keep and free up |
| `src-tauri/src/jumplist.rs` | The taskbar button: its Jump List (tasks, pinned and most-used folders), its progress bar, and Back, Forward and Up under its preview |
| `src-tauri/src/recycle.rs` | The Recycle Bin as a place: its items from the shell and their `$I` records, and emptying it ([docs/recycle-bin.md](docs/recycle-bin.md)) |
| `src-tauri/src/compare.rs` | Compare panes down through subfolders and by contents, and what Sync copies |
| `src-tauri/src/filediff.rs` | Compare two files: text side by side (Difftastic's structural diff, or line by line), pictures, and bytes |
| `src-tauri/src/native_preview.rs`, `src/components/explorer/ShellPreview.tsx` | Windows' preview handlers (Office, fonts…) drawing over the preview pane, on a thread of their own |
| `src-tauri/src/markdown.rs`, `jq.rs`, `sheet.rs`, `data_lens.rs`, `sql.rs`, `pe.rs`, `media.rs` | Glint's own previews: Markdown (comrak), JSON, YAML and TOML as a tree with jq filters (jaq, in a process of its own), Excel, OpenDocument and Parquet tables, CSV and SQLite, Query (SQL, read-only), programs and DLLs, and photo, video and music details |
| `src-tauri/src/git.rs`, `repo.rs`, `src/lib/git.ts` | Git status in folders, the branch, `git:` filters and a changed file's changes, read with gitoxide (nothing written); a repository's card (onefetch, tokei) |
| `src-tauri/src/term.rs`, `projects.rs`, `scripts.rs`, `src/lib/terminal.ts` | The terminal under the files (ConPTY through portable-pty, the folder each prompt reports), the Run menu (package.json, justfile, Cargo, uv) and script actions from the Scripts folder |
| `src-tauri/src/codesearch.rs`, `doctext.rs`, `ocr.rs`, `meaning.rs` | Text mode's code patterns (ast-grep), documents (PDF, Office, OpenDocument, EPUB, read in worker processes), text in pictures (Windows' OCR) and meaning search (Model2Vec) |
| `src-tauri/src/cleanup.rs`, `dedup.rs`, `convert.rs`, `batch_rename.rs` | Clean up (Czkawka's modes too), Find duplicates (hard links, block clones), Convert pictures (rimage) and batch rename |
| `src-tauri/src/xray.rs`, `sign.rs`, `check.rs`, `defender.rs`, `locks.rs`, `timeline.rs` | The File X-Ray (checksums, streams, signatures), YARA-X and Kingfisher checks, Defender scans, what's locking a file (Restart Manager) and recent changes |
| `src-tauri/src/share.rs`, `lan.rs` | Send to another PC (sendme) and Share on this network (dufs) |
| `src-tauri/src/remote/` | Remote connections as read-only folders: S3 (signed requests) and WebDAV over HTTPS, SFTP through Windows' own `sftp.exe`, rclone's remotes, secrets in Credential Manager ([docs/remote.md](docs/remote.md)) |
| `src-tauri/src/rclone.rs`, `src/lib/rclone.ts`, `src/components/explorer/CloudSetup.tsx` | Clouds through rclone: its remote control (started when first needed), signing in step by step and in the browser, its remotes, how full they are, and sync runs |
| `src-tauri/src/rules.rs`, `src/lib/rules.ts` | Folder rules: watching, telling when a file is complete, the actions, the log with Undo, and sync rules' runs |
| `src-tauri/src/quick.rs`, `src/components/QuickSearch.tsx` | Quick search from anywhere: the shortcut, its window, the tray and starting with Windows |
| `src-tauri/src/thumbs.rs` | Thumbnails from Windows' thumbnail cache (`IShellItemImageFactory`, `IThumbnailCache`) on worker threads, in a queue per tab |
| `src-tauri/src/watch.rs` | Live folders: notify watches the folders on screen |
| `src-tauri/src/launch.rs`, `verbs.rs`, `access.rs` | Folders from the command line and later launches (one copy runs at a time), File Explorer's *Open in Glint* entries and Win+E ([docs/explorer-integration.md](docs/explorer-integration.md)), and Get access for folders Windows keeps you out of |
| `src-tauri/src/mcp.rs` | The Model Context Protocol server AI agents start (`Glint.exe mcp`): its tools over the index, fd, pdu and ripgrep, and Settings → AI agents' setups |
| `src-tauri/src/shellmenu.rs` | Windows' own context menu (Show more options): the shell's `IContextMenu`, with the messages its submenus need while it's open |
| `src-tauri/src/appearance.rs`, `placement.rs`, `src/lib/color.ts` | Windows 11's Mica and the accent color; the window's place, kept and put back; colors as the theme's tokens |
| `src/lib/plain.ts` | Plain words in the search box read as its filters |
| `src/lib/highlight.tsx`, `highlighter.ts` | Code highlighting: the languages, and Prism with its grammars, loaded the first time code is shown |
| `quick.html`, `src/quick.tsx` | The quick search window's own page |
| `src-tauri/src/openers.rs`, `calc.rs`, `zoxide.rs`, `snap.rs` | Open in… (the editors and terminals found), the calculator in `Ctrl+K` (fend), folder history shared with zoxide, and Windows 11's snap layouts on the maximize button |
| `src-tauri/index/`, `src-tauri/src/index.rs` | The opt-in instant-search service (NTFS MFT + USN journal): names, folder sizes, maps, what grew and what moved ([docs/instant-search.md](docs/instant-search.md)); turning it on and off |
| `src-tauri/binaries/` | Bundled `rg`, `fd` and `pdu` executables |
| `src-tauri/installer/` | The NSIS installer: Tauri's template with Glint's pages, its hooks, its look and artwork, and the **Extra tools** page ([docs/releasing.md](docs/releasing.md#the-installer)) |
| `src-tauri/src/webview.rs` | Before the window opens: on a PC without WebView2, offering to install it (Microsoft's installer, checked for Microsoft's signature) |
| `src-tauri/src/tools.rs`, `proc.rs`, `src/lib/tools.ts`, `src/components/ToolNeeded.tsx` | Every program Glint drives: the bundled ones, and optional ones installed on demand with winget or from their GitHub release (pinned by SHA-256) ([docs/tools.md](docs/tools.md)); finding and running them, a lane per tab; Settings → Tools, and the Install card a feature shows when its tool is missing |
| `src/lib/updater.ts` | Finding and installing updates (tauri-plugin-updater) |
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
cd src-tauri && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
npx tsc -p tsconfig.json
node src/lib/explorer.check.ts
node src/lib/text.check.ts
```

Try UI changes in the app with `npm run tauri dev`, and add a screenshot or GIF to the pull request. Release builds run once per app identifier and hand their folders to a Glint that's already running, so try a release build under its own name and identifier ([how](docs/explorer-integration.md#trying-it-without-touching-your-glint)).

## Adding or updating a crate

Prefer crates that are light and pure Rust, and say what one adds to `Glint.exe`'s size. After changing crates, run `npm run licenses` and commit `THIRD_PARTY_CRATES.md` with them, and run `cargo deny --manifest-path src-tauri/Cargo.toml check` (both tools: `cargo install --locked cargo-deny cargo-about`). A crate under a license that `src-tauri/deny.toml` doesn't list needs an issue first.

## Updating a bundled tool

Download the `x86_64-pc-windows-msvc` build from the tool's official GitHub release, check its SHA-256 against the digest GitHub shows for the asset, and replace the file in `src-tauri/binaries/` (keep the `-x86_64-pc-windows-msvc.exe` suffix). Then update the version in `README.md` and `THIRD_PARTY_LICENSES.md`.

## Licensing

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
