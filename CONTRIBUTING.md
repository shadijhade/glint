# Contributing

Thanks for helping make ShadUI Ripgrep faster and better! Bug reports, ideas, docs fixes and code are all welcome.

## Before you start

-   **Bugs:** open an issue with the bug template. The app version is in **Settings → About**.
-   **Features:** open an issue first for anything bigger than a small fix, so we can agree on the approach before you spend time on it.
-   **Good first issues** are labelled [`good first issue`](https://github.com/shadijhade/shadui-ripgrep/labels/good%20first%20issue).

## Set up

You need Windows 10 or 11, [Node.js](https://nodejs.org/) 22.18 or newer, and [Rust](https://www.rust-lang.org/tools/install) (stable, MSVC toolchain).

```bash
git clone https://github.com/shadijhade/shadui-ripgrep.git
cd shadui-ripgrep
npm install
npm run tauri dev
```

The first build compiles all Rust dependencies and takes a few minutes; later ones take seconds.

## Where things are

| Path | What it is |
| :--- | :--- |
| `src/components/explorer/` | The Files mode UI: panes and tabs, list, sidebar, address bar, preview, command bar |
| `src/lib/explorer.ts` | Explorer state and actions: tabs and panes, navigation, selection, search, file operations |
| `src/lib/fs.ts` | Typed wrappers for every Rust command |
| `src/components/Results.tsx`, `Search.tsx` | Text search (ripgrep) |
| `src-tauri/src/fsx.rs` | Folder listing (Win32 `FindFirstFileExW`, binary transfer to the UI) |
| `src-tauri/src/find.rs`, `sizes.rs` | fd name search and dust folder sizes (or the index when it's on) |
| `src-tauri/src/shell.rs` | Copy, move, delete, rename (`IFileOperation`), clipboard, drag and drop, Properties |
| `src-tauri/index/` | The opt-in instant-search service (NTFS MFT + USN journal) |
| `src-tauri/binaries/` | Bundled `rg`, `fd` and `dust` executables |

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

Try UI changes in the app with `npm run tauri dev`, and add a screenshot or GIF to the pull request.

## Updating a bundled tool

Download the `x86_64-pc-windows-msvc` build from the tool's official GitHub release, check its SHA-256 against the digest GitHub shows for the asset, and replace the file in `src-tauri/binaries/` (keep the `-x86_64-pc-windows-msvc.exe` suffix). Then update the version in `README.md` and `THIRD_PARTY_LICENSES.md`.

## Licensing

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
