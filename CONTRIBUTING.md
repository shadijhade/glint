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
| `src/components/explorer/` | The Files mode UI: panes and tabs, list and grid, disk-usage map, sidebar, address bar, preview, command bar |
| `src/lib/explorer.ts` | Explorer state and actions: tabs and panes, navigation, selection, search, file operations |
| `src/lib/fs.ts` | Typed wrappers for every Rust command |
| `src/lib/usage.ts`, `diskUsage.ts` | Disk-usage maps: decoding and the squarified treemap layout; each tab's map |
| `src/lib/thumbs.ts`, `grid.ts` | Thumbnails for the tiles and rows on screen (asking, dropping, keeping them); the grid's tile sizes, columns and keys, and keeping its place as they change |
| `src/components/Results.tsx`, `Search.tsx` | Text search (ripgrep) |
| `src-tauri/src/fsx.rs` | Folder listing (Win32 `FindFirstFileExW`, binary transfer to the UI) |
| `src-tauri/src/archive/` | Archives opened like folders (zip, 7z, RAR, cabinets, installer packages, tar, compressed files): reading their list of entries, the folder tree, extracting on demand and into staging folders, safety limits ([docs/archives.md](docs/archives.md)) |
| `src-tauri/src/find.rs`, `sizes.rs`, `usage.rs` | fd name search, pdu folder sizes and disk-usage maps (or the index when it's on) |
| `src-tauri/src/shell.rs` | Copy, move, delete, rename (`IFileOperation`), clipboard, drag and drop, Properties |
| `src-tauri/src/recycle.rs` | The Recycle Bin as a place: its items from the shell and their `$I` records, and emptying it ([docs/recycle-bin.md](docs/recycle-bin.md)) |
| `src-tauri/src/compare.rs` | Compare panes down through subfolders and by contents, and what Sync copies |
| `src-tauri/src/native_preview.rs`, `src/components/explorer/ShellPreview.tsx` | Windows' preview handlers (Office, fonts…) drawing over the preview pane, on a thread of their own |
| `src-tauri/src/quick.rs`, `src/components/QuickSearch.tsx` | Quick search from anywhere: the shortcut, its window, the tray and starting with Windows |
| `src-tauri/src/thumbs.rs` | Thumbnails from Windows' thumbnail cache (`IShellItemImageFactory`, `IThumbnailCache`) on worker threads, in a queue per tab |
| `src-tauri/src/launch.rs`, `verbs.rs` | Folders from the command line and later launches (one copy runs at a time), and File Explorer's *Open in Glint* entries ([docs/explorer-integration.md](docs/explorer-integration.md)) |
| `src-tauri/index/` | The opt-in instant-search service (NTFS MFT + USN journal) |
| `src-tauri/binaries/` | Bundled `rg`, `fd` and `pdu` executables |
| `src-tauri/src/tools.rs`, `src/lib/tools.ts`, `src/components/ToolNeeded.tsx` | Every program Glint drives: the bundled ones, and optional ones installed on demand with winget or from their GitHub release (pinned by SHA-256); Settings → Tools, and the Install card a feature shows when its tool is missing |
| `src/lib/updater.ts` | Finding and installing updates (tauri-plugin-updater) |
| `scripts/` | `npm run release` and its helpers: update signatures, winget and Scoop manifests ([docs/releasing.md](docs/releasing.md)) |

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

## Updating a bundled tool

Download the `x86_64-pc-windows-msvc` build from the tool's official GitHub release, check its SHA-256 against the digest GitHub shows for the asset, and replace the file in `src-tauri/binaries/` (keep the `-x86_64-pc-windows-msvc.exe` suffix). Then update the version in `README.md` and `THIRD_PARTY_LICENSES.md`.

## Licensing

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
