# The GPUI Glint installer

An NSIS script for the GPUI build's `Glint.exe` (`crates/glint`). It is the installer the Tauri build had
(`src-tauri/installer`, itself Tauri's NSIS template with Glint's pages) turned into a plain script, so
`makensis` is all that builds it: no Tauri bundler, no `tauri-cli`, no plugin beyond NSIS's own. It installs
over the Tauri build in place: same folder, same registry keys, same shortcuts, same switches.

## Build it

```
npm run gpui:installer                      # cargo build --release -p glint, gather, makensis
node scripts/gpui-installer.mjs --dry-run   # check what it packs and print the makensis command; builds nothing
```

The setup is written to `<target>/release/bundle/Glint-setup.exe` (`target/ui/release/bundle` with
`CARGO_TARGET_DIR=target/ui`, which is the default here). The script needs `makensis.exe` (NSIS 3): it looks
at `MAKENSIS`, the `PATH`, `%LOCALAPPDATA%\tauri\NSIS` (where Tauri's bundler keeps one) and
`Program Files\NSIS`. Options are listed at the top of the script: `--exe <Glint.exe>` packs a Glint.exe you
already have (a debug build, say) instead of building one; `--service <shadui-index.exe>` and
`--service-sig <file>` say which instant search service to carry; `--out <file>` moves the output.

Running the script never installs, runs or signs anything. The setup is not run by it either.

## What it installs

Per user, with no administrator rights, into `%LOCALAPPDATA%\Glint` (or the folder an earlier install used):

| File | Where it comes from |
| :--- | :--- |
| `Glint.exe` | `cargo build --release -p glint` |
| `rg.exe`, `fd.exe`, `pdu.exe` | next to the built `Glint.exe` (`crates/glint/build.rs` puts them there), else `src-tauri/binaries` |
| `LICENSE.txt`, `THIRD_PARTY_LICENSES.md`, `THIRD_PARTY_CRATES.md` | the repository root |
| `shadui-index.exe`, `shadui-index.exe.sig` | the instant search service and its update signature, **when found** (see "What is left") |
| `uninstall.exe` | made by the installer |

Pages, as before: Welcome, the reinstall question (only when Glint is installed), the folder, the Extra tools
page (`tools.nsh`), then the install and a finish page with "Create desktop shortcut" and "Run Glint". The
look is `look.nsh` and `art/` (the pictures `scripts/installer-art.mjs` draws, a copy of
`src-tauri/installer/art`).

Switches (all as in the Tauri build, which is what the updater relies on): `/S` silent, `/P` passive (no pages,
closes itself), `/UPDATE` (install over the running version without asking, keep the shortcuts as they are),
`/NS` (no shortcuts), `/R` and `/ARGS <args>` (start Glint when a silent or passive install is done),
`/D=<folder>` (install folder), `/TOOLS=jaq,uv` and `/INSTANTSEARCH` (tools.nsh). tauri-plugin-updater's
passive mode passes `/P /UPDATE /R` (plus `/ARGS`); the GPUI updater has to pass the same, which is the one thing
this script and `updater.rs` must agree on.

## What it changes in the registry and the Start menu

All under the current user:

- `HKCU\Software\shado\Glint` (default value = the install folder; `Installer Language`).
- `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Glint`: `DisplayName`, `DisplayIcon`,
  `DisplayVersion`, `Publisher` (`shado`), `InstallLocation`, `UninstallString`, `NoModify`, `NoRepair`,
  `EstimatedSize`, `MainBinaryName` (`Glint.exe`).
- `Glint.lnk` in the Start menu's Programs (not when updating or with `/NS`), and on the desktop (silent and
  passive installs, or the finish page's check box), both with the app's user model id
  `com.shado.shadui-ripgrep`.
- The Extra tools page and `/INSTANTSEARCH` install what was picked after Glint is in place, with
  `Glint.exe tools install <id>` (and the service's own `install` verb, which asks Windows for approval).
- First install over an app still called `shadui-ripgrep` (before 0.3.1) removes that app's files, shortcuts
  and Uninstall entry, as `hooks.nsh` did.
- The uninstaller (not when updating) removes the files above, the shortcuts, Glint's "Open in Glint" entries
  in File Explorer (`HKCU\Software\Classes\Directory\shell\Glint` and the Background and Drive ones, and Win+E's
  `CLSID\{52205fd8-...}` key) where they run this copy, the `Run` value that starts Glint with Windows, and the
  instant search service. "Delete the application data" also removes `%APPDATA%\com.shado.shadui-ripgrep`,
  `%LOCALAPPDATA%\com.shado.shadui-ripgrep` and the keys under `HKCU\Software\shado`.

No file associations or deep links are registered at install time: the Tauri build registered none either.
File Explorer's entries and the default-folder-handler are written by the app from its settings
(`glint-core/src/verbs.rs`).

What is **not** done any more: no WebView2 step (the GPUI build draws everything itself), no
`nsis_tauri_utils` plugin (`GlintSemverCompare` uses NSIS's `VersionCompare`, and `Exec` starts Glint, which is
right because the installer runs as the user), no per-machine or "both" install mode, no license page (as
before), and no language chooser (English only, as before).

## Files

| File | What it is |
| :--- | :--- |
| `installer.nsi` | the script (Tauri's template without the handlebars, the WebView2 section or the plugin) |
| `hooks.nsh` | Glint's install and uninstall steps: the old-name migration, Explorer entries, the service, the tools |
| `look.nsh` | font, welcome and finish words, artwork picked for the screen's scaling |
| `tools.nsh` | the Extra tools page (identical to `src-tauri/installer/tools.nsh` until that folder goes; the script warns if they differ) |
| `utils.nsh` | Tauri's helper macros (shortcuts, the "close it first" check) minus the plugin call |
| `strings.nsh` | the installer's English words |
| `art/` | 16 bitmaps: `header-` and `sidebar-` at 100% to 300% |

## Signing (a hook, not done here)

The script never signs. Set `GLINT_SIGN_COMMAND` to a command line with `%1` for a file's path (for example
`signtool sign /fd SHA256 /a "%1"`, or the release pipeline's Artifact Signing command): the script then runs
it on `Glint.exe`, `rg.exe`, `fd.exe`, `pdu.exe` and the service before packing, on the uninstaller
(`!uninstfinalize`) and on the finished setup. The updater's signature (`Glint-setup.exe.sig`, `latest.json`)
and the service's (`shadui-index.exe.sig`) are made by the release pipeline with the maintainer's key
(`scripts/release.mjs`, `scripts/sign-service.mjs`); signing the service after its signature was made
invalidates that signature, so the script then leaves it out.

## What is left

- **MSIX** (`scripts/msix.mjs`) still packs the Tauri build; it has to be pointed at the GPUI build's folder
  (the identity 9NQLRJ3HF1C9 stays). The Store's offline installer no longer needs WebView2 inside.
- **The service.** `shadui-index.exe` is a binary of the old `src-tauri` package; the GPUI workspace doesn't
  build one yet, so unless one is passed with `--service` (or is in `src-tauri/target/release`), the setup has no
  instant search service. Glint.exe looks for it beside itself (`glint-core/src/index.rs`).
- **Release plumbing.** `release.mjs` still builds the Tauri installer; it has to call this script, sign what
  it makes, write `latest.json` and the winget and Scoop manifests (`package-manifests.mjs`) from
  `Glint-setup.exe`.
- **`scripts/installer-art.mjs`** writes the pictures into `src-tauri/installer/art`; point it at `art/` here
  when `src-tauri` goes. The same goes for glint-core's test that checks `tools.nsh` against its tool list
  (`include_str!("../../../src-tauri/installer/tools.nsh")` in `tools.rs`).
- **Not tried for real:** the setup itself was never run (by rule): it compiles with no warnings, and its
  contents were listed, but the pages, the in-place update over a Tauri 0.9.x install, silent and passive
  installs, the Extra tools page and the uninstaller need a test run on a spare account or machine.
