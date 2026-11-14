# The GPUI Glint installer

An NSIS script for the GPUI build's `Glint.exe` (`crates/glint`). It is the installer the Tauri build had
(`src-tauri/installer` until 0.10, itself Tauri's NSIS template with Glint's pages) turned into a plain script, so
`makensis` is all that builds it: no Tauri bundler, no `tauri-cli`, no plugin beyond NSIS's own. It installs
over the Tauri build in place: same folder, same registry keys, same shortcuts, same switches.

## Build it

```
npm run installer                           # cargo build --release -p glint, gather, makensis
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
| `rg.exe`, `fd.exe`, `pdu.exe` | next to the built `Glint.exe` (`crates/glint/build.rs` puts them there from `crates/glint/binaries`), else `crates/glint/binaries` |
| `LICENSE.txt`, `THIRD_PARTY_LICENSES.md`, `THIRD_PARTY_CRATES.md` | the repository root |
| `shadui-index.exe`, `shadui-index.exe.sig` | the instant search service (`cargo build --release -p shadui-index`) and its update signature, **when found** |
| `uninstall.exe` | made by the installer |

Pages, as before: Welcome, the reinstall question (only when Glint is installed), the folder, the Extra tools
page (`tools.nsh`), then the install and a finish page with "Create desktop shortcut" and "Run Glint". The
look is `look.nsh` and `art/` (the pictures `scripts/installer-art.mjs` draws from `logo.svg`).

Switches (all as in the Tauri build, which is what the updater relies on): `/S` silent, `/P` passive (no pages,
closes itself), `/UPDATE` (install over the running version without asking, keep the shortcuts as they are),
`/NS` (no shortcuts), `/R` and `/ARGS <args>` (start Glint when a silent or passive install is done),
`/D=<folder>` (install folder), `/TOOLS=jaq,uv` and `/INSTANTSEARCH` (tools.nsh). tauri-plugin-updater's
passive mode passes `/P /UPDATE /R /ARGS <the arguments Glint was started with>` (checked against
`tauri-plugin-updater` 2.14's `updater_parameters`: the install mode's `/P`, then `/UPDATE`, then, when it restarts
the app, `/R` and `/ARGS`, in that order); `updater.rs` builds and tests the same line, and `installer.nsi` reads each of the
flags with `GetOptions`, so it finds them wherever they are. `/S` is NSIS's own silent switch (winget's `Silent`; `/P` is
its `SilentWithProgress`). Nothing else has to agree.

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
| `tools.nsh` | the Extra tools page (a test in `crates/glint-core/src/tools.rs` checks it against the tool list) |
| `logo.svg` | the logo `scripts/installer-art.mjs` draws the pictures from |
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

## What the release pipeline does around it

`npm run release` ([docs/releasing.md](../../../docs/releasing.md)) builds Glint.exe and the service, signs a copy of
the service for the instant search self-update, runs this script with the service and its signature
(`--service-signed` when the service was code-signed first), then signs the setup for the in-app updater, writes
`latest.json` and the winget and Scoop manifests. `npm run msix` packs the Store's package from the same build
(`crates/glint/msix`). The artwork comes from `npm run installer-art`.

## What is left

- **Not tried for real:** the setup itself was never run (by rule): it compiles with no warnings, and its
  contents were listed, but the pages, the in-place update over a Tauri 0.9.x install, silent and passive
  installs, the Extra tools page and the uninstaller need a test run on a spare account or machine.
