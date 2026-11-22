# Releasing

Glint is built, signed and laid out by `npm run release` on the maintainer's PC, or by the release workflow on GitHub (started by hand, or by a pushed `vX.Y.Z` tag; the only way to get the binaries signed by SignPath). Both make the same files, and [`npm run ship`](#one-command-npm-run-ship) does a whole release with the second: the checks, the version, the build, the release, Scoop, winget and the Microsoft Store. This page covers what to set up once and what each step is.

The build is the GPUI one: `Glint.exe` (`crates/glint`) and the instant search service (`shadui-index.exe`, `crates/shadui-index`), built together by `cargo build --release -p glint -p shadui-index` into `target/ui/release` (`CARGO_TARGET_DIR` changes the folder), and packed by `makensis` into a plain NSIS setup program ([`crates/glint/installer`](../crates/glint/installer/README.md)). There is no Tauri in the pipeline: no `tauri build`, no bundler, no WebView2 inside the installer. What every installed Glint (v0.9.x, built with Tauri) relies on to update itself is kept as it was; [what stays fixed](#what-stays-fixed-because-installed-copies-update-through-it) lists it.

## How it works

1.  **`npm run release`** ([`scripts/release.mjs`](../scripts/release.mjs)) does these, in order, and stops at the first that fails:
    1.  checks that the root `Cargo.toml`'s `[workspace.package] version`, every crate's version and `package.json` agree, and that everything is committed (`--allow-dirty` for a trial);
    2.  runs the crates' checks ([below](#5-release-notes-and-the-crates-checks)): `cargo deny check` and `npm run licenses -- --check`. A check whose tool isn't installed is skipped, with a warning that says what it leaves unchecked;
    3.  checks the update key: the private key and its password (asked once, now, not after the build) must be the pair of `crates/shadui-index/update-pubkey.txt`, which must be the same text as `PUBLIC_KEY` in `crates/glint-ui/src/updater.rs`;
    4.  works out the code signing ([below](#2-code-signing-optional-recommended));
    5.  builds `Glint.exe` and the service, then signs a copy of the service (code signing first when set up, then the update key; the service itself checks the signature with `shadui-index.exe verify-update`), and has [`scripts/gpui-installer.mjs`](../scripts/gpui-installer.mjs) pack the installer with `makensis`, once it has checked that [Windows 10 can start what it packs](#windows-10-can-start-it), code-signing `Glint.exe`, rg, fd, the uninstaller and the setup when it's set up, and checking those signatures;
    6.  signs the installer for the in-app updater ([`scripts/lib/minisign.mjs`](../scripts/lib/minisign.mjs): minisign, written with Node's own crypto), writes `latest.json` and checks the update the way the app will ([`scripts/verify-update.mjs`](../scripts/verify-update.mjs)): the signature must verify with the public key the app is built with and be bound to the version `latest.json` announces. It also writes the winget and Scoop manifests.

    Everything lands in `release/vX.Y.Z/` (`--out <folder>` puts it elsewhere), with the installer a second time as `Glint-setup.exe` ([below](#what-stays-fixed-because-installed-copies-update-through-it)), and it prints the `gh release create` command that makes the draft release.
2.  **Publishing the draft** makes it the latest release. Copies of the app read `https://github.com/shadijhade/glint/releases/latest/download/latest.json` at start and every hour by the clock (sooner after a failed look, and when the window comes back after a while). With **Install updates automatically** on (Settings → About, the default), a copy installed with the setup program downloads the installer in the background (picking up an interrupted download, not on a metered connection), verifies its signature, says *Glint X is ready* once (Windows' own notification while the window is hidden) and keeps **Restart to update** in the header; it installs silently when the app quits, or right away after a start with Windows that nobody has opened yet. **Restart now** waits for any copy or move to finish, closes the app, installs over it with a progress bar and opens the new version. A copy Scoop installed updates through Scoop (`scoop update glint` once it has quit, then it starts again), and a Scoop copy beside the setup program's copy is brought along. crates/glint-ui/src/updater.rs has the details.

Copies that the setup program didn't install (Scoop, a portable copy, a dev build) don't install updates themselves: the app tells them about the new version and links to it. A release without `latest.json` gets the same treatment from every copy, through GitHub's latest-release API.

### What stays fixed, because installed copies update through it

Every Glint already installed (the Tauri 0.9.x ones, and the GPUI ones after them) finds and installs its updates through these. Change none of them without a release that both old and new copies can read:

-   **`latest.json`**: its format (`version`, `notes`, `pub_date` and `platforms."windows-x86_64"` with a `signature` and a `url`), its address (`https://github.com/shadijhade/glint/releases/latest/download/latest.json`; `MANIFEST_URL` in `crates/glint-ui/src/updater.rs`, `plugins.updater.endpoints` of the old `tauri.conf.json`) and the platform key it lists. The GPUI updater also reads `windows-x86_64-nsis` first, which `release.mjs` doesn't write.
-   **The installer's names**: `Glint_<version>_x64-setup.exe` (what `latest.json`'s `url` ends with, and what the winget and Scoop manifests download) and `Glint-setup.exe` (the same file, so `…/releases/latest/download/Glint-setup.exe` always downloads the newest one: the README's and the [website's](#the-website) Download links point at it, so a release without it breaks them until the next release).
-   **The update signature**: minisign's format as Tauri writes it (the `.sig` is the base64 of the minisign box, which is also what `latest.json`'s `signature` holds), signed with the same key, with the version in the trusted comment (`timestamp:…<TAB>file:Glint_X.Y.Z_x64-setup.exe<TAB>version:X.Y.Z`). The app requires the signed version to be the announced one, so a tampered `latest.json` can't pass off an older, validly signed installer as new. Updates never go backwards either. The public key is `crates/shadui-index/update-pubkey.txt` (the service's) and `PUBLIC_KEY` in `updater.rs` (the app's): the same text, which `release.mjs` checks.
-   **The installer's command line**: the in-app updater runs the setup as `Glint_X.Y.Z_x64-setup.exe /P /UPDATE /R /ARGS <the arguments Glint was started with>`. That is what `tauri-plugin-updater` 2.14 runs in `passive` mode (`updater_parameters` in its `updater.rs`: the install mode's flags `/P`, then `/UPDATE`, then, when it restarts the app, `/R` and `/ARGS` with the current arguments), and what `updater.rs` builds and tests (`/P /UPDATE /R /ARGS …`). The installer ([`installer.nsi`](../crates/glint/installer/installer.nsi)) reads exactly those: `/P` and `/UPDATE` in `.onInit` (passive: no pages, closes itself; update: install over a running version without asking, keep the shortcuts as they are), `/S` as NSIS's own silent switch (what the winget manifest's `Silent` uses; `/P` is its `SilentWithProgress`), and `/R` with `/ARGS` in `.onInstSuccess`, to start Glint again with the arguments it had. `/NS` (no shortcuts), `/D=<folder>` and, from Glint's own pages, `/TOOLS=jaq,uv` and `/INSTANTSEARCH` are the installer's too. A different order or spelling in a future updater would still work, as `GetOptions` finds each flag wherever it is.

**The instant search service updates itself.** It runs from `C:\Program Files\ShadUI Ripgrep\`, which an app update doesn't touch (the in-app updater runs the installer in update mode, so its uninstall hook leaves the service alone). The app and the service share one version, `[workspace.package] version` in the root `Cargo.toml`, so after an app update the service reports an older version, and the app hands it the build that came with the update. Each release signs `shadui-index.exe` with the update key, the version sealed in the signature ([`scripts/sign-service.mjs`](../scripts/sign-service.mjs)), and the installer carries the signature next to it as `shadui-index.exe.sig`; a service from after 0.9 takes a later build signed that way without administrator approval ([how, and why it's safe](instant-search.md#security-model)). A service from 0.9 or before, or a release without the signature, is updated the old way: the app says *Instant search has an update* once, and **Settings → Instant search** shows **Update** until it's done (one administrator approval; it runs the same `shadui-index.exe install` as turning it on).

Code signing happens to the service first, and its update signature is made over the code-signed file, so a code-signed release carries the signature too (the Tauri pipeline couldn't: it signed after).

### Windows 10 can start it

Windows loads every DLL a program imports, and every function it takes from them, before any of the program's code runs, and refuses to start it when one is missing. Glint 0.10 to 0.12 imported Windows 11's I/O ring (`api-ms-win-core-ioring-l1-1-0.dll`) and didn't start on Windows 10 at all ([issue #6](https://github.com/shadijhade/glint/issues/6)). So `scripts/gpui-installer.mjs` and `scripts/msix.mjs` read the import tables of what they pack ([`scripts/lib/windows-imports.mjs`](../scripts/lib/windows-imports.mjs)), and stop when `Glint.exe` or the service imports anything that isn't in [`scripts/windows10-imports.json`](../scripts/windows10-imports.json): the imports checked to exist on Windows 10 1809 (10.0.17763, the MSIX's MinVersion) in every edition. Three kinds are named with what to do instead:

-   **Windows 11's DLLs**, like the I/O ring's: glint-core's `ioring.rs` looks its functions up at run time (`GetProcAddress`) and reads files the usual way where there's none.
-   **The Visual C++ runtime** (`VCRUNTIME140.dll`, `MSVCP140.dll`), which Windows doesn't come with: [`.cargo/config.toml`](../.cargo/config.toml) builds the C runtime into both programs (`+crt-static`).
-   **Media Foundation** (`mfplat.dll`, `mfreadwrite.dll`), which Windows' N editions don't have until the Media Feature Pack is added: glint-core's `mfapi.rs` looks it up at run time, so there the player, video frames and waveforms say Windows can't play the file. Call any Media Foundation function through it.

Any other import that isn't listed fails until someone has checked it on its Microsoft Learn page (Requirements, Minimum supported client). One newer than Windows 10 1809 is looked up at run time instead; one that's there goes in with `npm run check-imports -- --update`. `npm run check-imports` checks a build of your own the same way (`-- <program.exe>` names others). The bundled rg and fd are their projects' official builds, so what they lack is a warning, not a stop. (pdu's official build needed `VCRUNTIME140.dll`, one reason its engine is now built into `Glint.exe` instead.)

## One-time setup

### 1. The updater key (needed for auto-update)

Updates are signed with a minisign key pair. The public key is compiled into the app and the service; the private key signs each release, on your PC or as a secret of the release workflow. **It is the key already in use**: every installed copy trusts it, so a new release must be signed with the same private key, and there is nothing to set up again unless it's lost.

-   The key was made with the Tauri CLI, `npx @tauri-apps/cli signer generate -w ~/.tauri/shadui-ripgrep.key` (choose a password when asked), which writes `shadui-ripgrep.key` (private) and `shadui-ripgrep.key.pub` (public) in `%USERPROFILE%\.tauri`, outside the repository. Signing needs no Tauri CLI (it is not a dependency of this repository any more, and `package.json` has no packages); only making a new key does, and `npx` fetches it for that one command.
-   The public half is committed in two places that must hold the same text: `crates/shadui-index/update-pubkey.txt` (the service trusts it) and `PUBLIC_KEY` in `crates/glint-ui/src/updater.rs` (the app does). It's public.
-   Keep a copy of the private key and its password in a password manager. **If they're lost**, installed copies can't verify new updates: you'd need a new pair, and everyone would have to download the next version by hand once (the app still tells them it exists). **If they leak**, rotate the same way.

The release workflow needs the key's contents and its password as the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. `npm run ship -- --setup` puts them there and proves GitHub signs with them ([below](#a-release-that-needs-nobody)); by hand they are under **Settings → Secrets and variables → Actions**, where nothing says whether what was pasted is right. Without them a tag push fails rather than publish a release the app can't install by itself.

`npm run release` needs `TAURI_SIGNING_PRIVATE_KEY` set to the key file's path (or its contents), and asks for the password once, before building, unless `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is set (the variable names are Tauri's, kept so the secrets and habits stay): it unlocks the key right away, so a wrong password or a key that isn't the one for the committed public key stops it then rather than after the build.

The signing is `scripts/lib/minisign.mjs`, minisign's format with Node's built-in `crypto` (scrypt to unlock the key, BLAKE2b-512 of the file, Ed25519). `npm run test:signing` proves it against what the real `tauri signer sign` wrote, from stored vectors (`scripts/lib/test-vectors/`: two throwaway test keys that nothing trusts, a small stand-in file and the `.sig` Tauri made for it under each key, made once with the Tauri CLI that no longer has to be installed). It signs the same file with `minisign.mjs` under the same trusted comment, compares the two `.sig` files field by field (the same layout, algorithm, key id, comments and lengths), checks ours is byte-identical to the stored `ours` signature, and checks both hold under the app's check (`checkSignature`, which `verify-update.mjs` runs). Tauri's signature bytes themselves can't be reproduced: its signer adds random noise to Ed25519's nonce, so two of its own signatures of one file differ too; ours is the plain, repeatable RFC 8032 signature, which every verifier (the app's `minisign-verify` included) accepts.

### 2. Code signing (optional, recommended)

Unsigned installers work, but Windows SmartScreen shows *Windows protected your PC* until a file has a reputation, and the UAC prompt for instant search shows *Unknown publisher*. Signing puts a verified name on the installer, the app and the service (a build signed on your PC also signs the bundled tools and the uninstaller). Reputation still builds with downloads; since 2024 even EV certificates don't skip that.

Since June 2023 a publicly trusted code-signing key must live in hardware (a smartcard, a token or a cloud HSM), so a `.pfx` file is no longer an option for new certificates. What's left for an open-source project:

| Option | Cost | Who can get it | Name on the signature | With `npm run release` |
| :--- | :--- | :--- | :--- | :--- |
| [Azure Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/quickstart) (formerly Trusted Signing) | About $10 a month (Basic) | Organizations in the US, Canada, the EU, the UK, Australia, New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel; individual developers in the US and Canada | Your verified name | Built in: three environment variables |
| [Certum](https://shop.certum.eu/code-signing.html): *Open Source Code Signing* (with a smartcard and reader) or *Standard Code Signing* in the cloud (SimplySign) | About €69 or from €49 | Individuals and companies Certum validates, in most countries | Your name | `--sign-command` with the certificate's thumbprint |
| Another CA with a hardware or cloud key (SSL.com eSigner, DigiCert KeyLocker, Sectigo…) | Roughly €200 to €500 a year | Anyone the CA validates | Your name | `--sign-command` with its signing tool |
| [SignPath Foundation](https://signpath.org/) | Free for open source | OSI-licensed projects with no proprietary parts | SignPath Foundation | Applied in October 2026 and declined until Glint is better known (stars, forks, articles, discussions); they invite a new application. It only signs builds made on GitHub-hosted runners, so it's the release workflow's (below) |

The signing is one command line with `%1` where a file's path goes, run on `Glint.exe`, `rg.exe`, `fd.exe`, the service, the uninstaller and the finished setup (`GLINT_SIGN_COMMAND`, which `--sign-command` sets for one run). Afterwards the script checks every one of them with `Get-AuthenticodeSignature`. It signs copies: the official tools in `crates/glint/binaries` stay as they are, so only the installer carries the signed ones.

**Azure Artifact Signing.** `npm run release` fetches Microsoft's Artifact Signing client for `signtool` (pinned by its SHA-256) into `release/.tools` and builds the command from it.

1.  In an Azure subscription, register the `Microsoft.CodeSigning` resource provider, create an Artifact Signing account (note its region's endpoint, such as `https://weu.codesigning.azure.net`), complete a **Public** identity validation, and create a **Public Trust** certificate profile. Microsoft's [quickstart](https://learn.microsoft.com/azure/artifact-signing/quickstart) walks through it; validation takes from a few hours to a few weeks.
2.  Assign your account the **Artifact Signing Certificate Profile Signer** role on the certificate profile.
3.  On your PC: the [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli-windows) (then `az login`), the .NET 8 runtime and the Windows SDK (`signtool` 10.0.26100 or newer; Visual Studio's C++ tools include it).
4.  Before `npm run release`, set `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT` and `AZURE_SIGNING_PROFILE`. (An app registration works too: set `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` and `AZURE_TENANT_ID` instead of `az login`.)

**A certificate in the Windows certificate store** (Certum's card, SimplySign's virtual card, most CAs' tokens), with `signtool`:

```powershell
npm run release -- --sign-command '"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe" sign /fd SHA256 /sha1 <SHA-1 thumbprint> /tr http://time.certum.pl /td SHA256 "%1"'
```

For a provider whose tool isn't `signtool` (SSL.com's CodeSignTool, DigiCert's `smctl`), put its command line there instead, with `"%1"` for the file.

**SignPath Foundation** (the release workflow). Once the project is approved, set the repository secrets `SIGNPATH_API_TOKEN` and `SIGNPATH_ORGANIZATION_ID`. The workflow submits `Glint.exe` and the service (artifact configuration `zip`), then the installer (`exe`), to project `glint` with signing policy `release-signing`, and waits up to an hour for each request to be approved on SignPath. It checks the signatures before anything is uploaded. Without the secrets it builds unsigned. (SignPath signs `Glint.exe` and the service, not rg and fd, which the installer carries as they are.)

Once releases are signed, take the SmartScreen note out of the README's Install section (and the website's question about it), and Glint can go into [the Microsoft Store](#the-microsoft-store).

### 3. The GitHub CLI

`npm run release` prints the command that creates the draft release; it needs [gh](https://cli.github.com/) (`winget install GitHub.cli`, then `gh auth login`). The web page for a new release works too: upload the files from `release/vX.Y.Z/`.

### 4. winget and Scoop (from the first release with the updater)

`npm run release` writes `release/vX.Y.Z/package-manifests/`: `winget/manifests/s/shadijhade/Glint/X.Y.Z/` (three YAML files) and `scoop/glint.json`. Submit them once the release is published, since they point at its download. For a release that's already out, `node scripts/package-manifests.mjs X.Y.Z` makes them from GitHub's copy of the installer. The package id stays `shadijhade.Glint`; the winget installer entry is `nullsoft`, per user, with the setup's own switches (`/S` silent, `/P` silent with progress).

**winget**:

1.  Read the manifests: check the publisher name, description and tags in the `.locale.en-US.yaml`.
2.  `winget validate --manifest <folder>`. To try the install, enable local manifests once from an administrator prompt (`winget settings --enable LocalManifestFiles`) and run `winget install --manifest <folder>`.
3.  Install [wingetcreate](https://github.com/microsoft/winget-create) (`winget install Microsoft.WingetCreate`) and create a classic GitHub token with the `public_repo` scope.
4.  `wingetcreate submit --token <token> <folder>` opens a pull request to [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). Its bots validate and test-install it, and a moderator merges it; then `winget install shadijhade.Glint` works.

For later versions, submit the new folder the same way (or let `wingetcreate update shadijhade.Glint --version X.Y.Z --urls <installer url> --submit --token <token>` write it).

**Scoop**: the manifest extracts the installer with 7-Zip instead of running it, so Scoop owns updates: the app runs `scoop update glint` itself once it has quit, and Scoop's record of the version stays right. `scoop install .\glint.json` tries it locally (on a PC where instant search is on, `scoop uninstall` then asks to remove the service too, and it takes out File Explorer's *Open in Glint* entries if that copy turned them on). To publish it, either:

-   open a pull request adding `bucket/glint.json` to [ScoopInstaller/Extras](https://github.com/ScoopInstaller/Extras) after reading its contributing guide (its bot then picks up new releases by itself through `checkver` and `autoupdate`), or
-   keep it in a bucket of your own: a repository with the file at `bucket/glint.json`, added with `scoop bucket add shadijhade https://github.com/shadijhade/<repository>` and installed with `scoop install shadijhade/glint`. Update the file for each release.

Once a package is live, add its install command to the README's Install section.

### 5. Release notes and the crates' checks

Four tools help with each release, and `npm run release` skips (with a warning that says what that leaves unchecked) the checks whose tool isn't installed:

-   **[git-cliff](https://git-cliff.org/)** drafts the release notes from the commits' subjects (`feat(preview): Markdown as it reads`): what's new by area, then what's faster and what's fixed. [`cliff.toml`](../cliff.toml) names the areas; give a new scope a name there. Install it with `winget install orhun.git-cliff`.
-   **[cargo-deny](https://github.com/EmbarkStudios/cargo-deny)** checks every crate in the Windows build against [RustSec's advisory database](https://rustsec.org/) (known vulnerabilities, and unmaintained crates Glint depends on directly), the licenses in [`deny.toml`](../deny.toml), and that crates come only from crates.io. Run it any time with `cargo deny check` from the repository root. For a vulnerability, update the crate (`cargo update -p <crate>`), or, if Glint doesn't use what's affected, add its ID with the reason to `ignore` under `[advisories]` in `deny.toml`. A crate under a license that isn't listed there needs a decision before it goes in. (The vendored GPUI crates under `vendor/` are path dependencies of the workspace's `[patch.crates-io]`, so they're checked as the crates they are.)
-   **[cargo-about](https://github.com/EmbarkStudios/cargo-about)** writes [`THIRD_PARTY_CRATES.md`](../THIRD_PARTY_CRATES.md), every crate in the app with its license's text, which the installer and the Store package carry (`npm run licenses`, from [`about.toml`](../about.toml) and [`about.hbs`](../about.hbs), over the whole workspace). Run it when crates change and commit the file: `npm run release` stops while it's out of date.
-   **What cargo-about and cargo-deny can't see**: fonts, icons and data that crates embed under licenses of their own. [`scripts/lib/embedded-notices.mjs`](../scripts/lib/embedded-notices.mjs) writes them after the crate list, from the crates' sources at the versions in `Cargo.lock` (so no tool needs installing for it): the Lucide icons (ISC, through `gpui-kit-assets`), `two-face`'s syntax definitions and themes (each with its own license), Typst's assets (Foxit PDF fonts, ICC profiles, W3C-derived tables), the citation styles `hayagriva` embeds (CC BY-SA 3.0), `hypher`'s hyphenation patterns and the time zone database. `npm run licenses -- --embedded` rewrites just that section, with no cargo-about; when you add a crate that embeds something, add it to `SOURCES` in that file. The file is the notices the installer and the package carry; nothing in the app's windows opens it (Settings → About shows the version, updates and diagnostics, not the notices), so it is read from the install folder.

Install the two cargo tools with `cargo install --locked cargo-deny cargo-about`, or put the `x86_64-pc-windows-msvc` builds from their GitHub releases on `PATH`. **Until cargo-about has run on this workspace, `THIRD_PARTY_CRATES.md`'s crate list is the old Tauri build's**: run `npm run licenses` and commit it before releasing.

## One command: `npm run ship`

`npm run ship -- 0.14.0` ([`scripts/ship.mjs`](../scripts/ship.mjs)) does a whole release, in order, and stops at the first thing that fails. Every step looks at what is already true and does only what is missing, so after a stop (a failed check, a build that broke, the network) the same command carries on from there.

| Step | What it does | What it needs |
| :--- | :--- | :--- |
| `ready` | master, nothing uncommitted, in step with GitHub, a version after the latest release | `gh auth login` |
| `checks` | `cargo deny check`, the notices (`npm run licenses -- --check`), the signing's own test; with `--full`, glint-ui's tests and the end-to-end checks first | cargo-deny and cargo-about for the first two (skipped with a warning without them: the build on GitHub runs both in any case, a `--local` build only when they are installed) |
| `version` | sets the version in `Cargo.toml`, `package.json` and the bug report's placeholder, drafts `release/vX.Y.Z/notes.md` when there are none, commits `Release vX.Y.Z` and pushes | the notes written (it stops once for you to rewrite the draft; `--notes-as-drafted` takes it as it is) |
| `build` | starts the release workflow on master, waits for it (about 20 minutes while its Rust cache is warm, three quarters of an hour without), and downloads what it built and signed into `release/vX.Y.Z` | the repository's `TAURI_SIGNING_PRIVATE_KEY` secrets (`npm run ship -- --setup`, [below](#a-release-that-needs-nobody)). When nobody knows whether they work, it asks GitHub first (the Release check, a minute) and doesn't start a build GitHub can't sign |
| `verify` | checks the installer the way the app will before installing it (`verify-update.mjs`), and that `Glint-setup.exe` is the same file | |
| `release` | creates the draft release on the commit that was built, with the four files and the notes, compares GitHub's copies with the built ones, publishes it as the latest, waits until installed copies are offered it, and puts the day it came out in the README's *What's new* | `--draft` stops the run before publishing: Scoop, winget and the Store wait for the published release |
| `scoop` | puts the new `bucket/glint.json` in [shadijhade/scoop-bucket](https://github.com/shadijhade/scoop-bucket) | |
| `winget` | `winget validate`, a branch on the fork with the three manifests, and the pull request to microsoft/winget-pkgs | the fork `shadijhade/winget-pkgs` |
| `store` | sends the Store's package, with `--listing` when the listing itself (description, features, screenshots, captions) changed since the last release. With a Partner Center application on this PC (`PARTNER_CENTER_TENANT_ID` and `PARTNER_CENTER_CLIENT_ID` set) it packs the MSIX (the workflow's `msix-vX.Y.Z` artifact, else from the released setup program's own files with 7-Zip) and runs `npm run store -- --submit`; otherwise, when the repository holds the application, it starts the **Store** workflow, which packs and sends it there | the Partner Center application, set up once ([below](#a-release-that-needs-nobody)); without it, it says where the package is, for Partner Center's pages |

```powershell
npm run ship -- 0.14.0            # everything
npm run ship -- --dry             # what each step would do, changing nothing
npm run ship -- --draft           # up to the draft release: read it and try Glint-setup.exe, then run it again
npm run ship -- --only store      # one step (or several: --only scoop,winget); --from release for it and the rest
npm run ship -- --no-store        # leave one out (--no-scoop, --no-winget)
npm run ship -- --run 43758944263 # take the build from a run of the workflow started earlier
npm run ship -- --local           # build and sign here instead of on GitHub (asks for the update key's password)
npm run ship -- --setup           # once: the update key and the Store's application as the repository's secrets
```

**The update key is never needed on this PC**, once the repository's `TAURI_SIGNING_PRIVATE_KEY` secret holds it. The workflow builds, signs the service and the installer and writes `latest.json`; a run started by hand (which is what `ship` does) makes no release, only the `release-vX.Y.Z` artifact. `ship` takes that, checks it against the public key the app is built with, and makes the release from it. Publishing the release creates its tag, and the workflow, started again by the tag, finds the release made and builds nothing.

`ship` says at its start what a run will need from a person (the key's password with `--local`; the Store's package left for Partner Center's pages). When nobody knows whether GitHub can sign (no Release check since the secrets were set), the build step asks it first, and with a key secret that is empty or wrong it stops there, a minute in. A run that built without the key anyway (it says `HAS_UPDATER_KEY: false`) is never released: `ship` keeps what it built in `release/vX.Y.Z/from-run` to look at, and won't take that run again. `npm run ship -- --local` builds and signs on this PC instead, with `npm run release`, which asks for the key's password.

### A release that needs nobody

`npm run ship -- X.Y.Z` runs to its end by itself once two things only the maintainer has are where the release runs, as the repository's Actions secrets. **`npm run ship -- --setup`** ([`scripts/ship-setup.mjs`](../scripts/ship-setup.mjs)) puts them there, once, in a terminal:

```powershell
npm run ship -- --setup           # asks for what is missing, stores it, and proves it on GitHub
npm run ship -- --setup --dry     # what is known to be set up and what is missing: asks, starts and changes nothing
npm run ship -- --setup --check   # asks GitHub what its secrets can do (a minute): changes nothing
```

1.  **The update key.** It reads the key's file (`%USERPROFILE%\.tauri\shadui-ripgrep.key`, or `--key <file>`), asks for its password without showing it, opens the key with it here and checks it is the key installed copies trust, and only then stores the two as `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. A wrong password or another key stores nothing.
2.  **The Partner Center application** the Microsoft Store takes submissions from. Making it is a few clicks of yours in Partner Center ([below](#sending-it-with-npm-run-store)); the setup asks for its tenant ID, client ID and key (the key without showing it), tries them against the Store (a sign-in and a read: nothing changes there), and stores them as `PARTNER_CENTER_TENANT_ID`, `PARTNER_CENTER_CLIENT_ID` and `PARTNER_CENTER_CLIENT_SECRET`. Say no there and it sets the key up alone (`--no-store` too); run it again when you have the values.
3.  **The proof.** It starts the **Release check** workflow ([`release-check.yml`](../.github/workflows/release-check.yml), [`scripts/release-check.mjs`](../scripts/release-check.mjs)), which opens the key with its password, signs a scrap and checks the signature the way the app checks an update, signs in to the Store and reads the app, with the secrets themselves, and says what it found on the run's page. GitHub tells nobody whether a secret is empty or what it holds (the key's secret was empty from 2026-10-22 until this was written, and every run built happily without it), so this is the only way to know before a release finds out. Run it whenever in doubt: **Actions → Release check → Run workflow**, or `--setup --check`.

Each value goes to `gh secret set` on its standard input: it is in no command line, no file and nothing the script prints, and nothing is stored on this PC. GitHub encrypts a secret for the repository: nobody can read it back, and only this repository's workflows, which only its owner can change, use it (their actions are pinned to commits for that reason). The key's file and your password manager stay the copies that matter: a secret can only be replaced, never read.

After it, a release is the one command, from any PC with `gh` signed in, and it asks for nothing:

| When | What happens | Where |
| :--- | :--- | :--- |
| at once | the checks, the version's commit, the push | this PC |
| about 20 minutes | the Release workflow runs the crates' checks, clippy and the tests, builds, signs the service, packs the installer and signs the update | GitHub |
| a minute | the files come down and are checked the way the app checks them; the draft release, GitHub's copies compared, published; the README's day | this PC |
| a minute | Scoop's bucket and the winget pull request | this PC |
| about 5 minutes | the **Store** workflow ([`store.yml`](../.github/workflows/store.yml)) takes the released setup program, checks its signature, packs the MSIX from its files and submits it | GitHub |

What still takes a person, none of it at a keyboard during the run: the words (the release's notes, the README's *What's new* and the Store's note, written before it), a moderator's merge of the winget pull request, the Store's certification, and a new key for the Partner Center application when the old one expires (Partner Center says when; the Release check says so too: *Microsoft refuses the Partner Center application*).

`npm run test:setup` proves the setup against stand-ins: throwaway keys, a gh that keeps the secrets it is given and runs the real check with them, and a Store that signs one application in. It shows that a wrong password, another key or a refused application stores nothing, and that no secret is ever among gh's arguments or in what is printed. It has no terminal, so it can't show a person typing: `node scripts/dev/setup-rehearsal/rehearse.mjs`, run in a terminal, is the same rehearsal with a helper typing the answers into the console (yes, a wrong password, the right one, no to the Store). Run it after changing how the setup asks: its first real run took an empty password, because what was typed never reached the prompt.

**Before it:** write what's new in the README (`## What's new`), in `docs/features.md` when a feature is new, and in `crates/glint/msix/listing.json` (`whatsNew`, with the version) for the Store; `ship` says so when the README or the listing still tell about the last release, and goes on.

**What it doesn't do:** the website ([below](#the-website): deploy it when its words or pictures change), and anything that needs a person: a moderator merges the winget pull request, and the Store certifies the submission.

## Every release, by hand

What `npm run ship` does, step by step, for when a step has to be done by hand.

1.  **Bump the version** to the same `X.Y.Z` in two places:
    -   `Cargo.toml`: `[workspace.package] version`, which the app, the installer and the instant search service share; then `cargo check` to update `Cargo.lock`
    -   `package.json`: `npm version X.Y.Z --no-git-tag-version` (updates `package-lock.json` too)

    `npm run release` and the workflow check that the two agree (and every crate's).
2.  **Commit** (`Release vX.Y.Z`) and push to `master`.
3.  **Draft the notes**: `npm run notes` writes `release/vX.Y.Z/notes.md` with what changed since the last release. Read it and edit it: it lists commits, and the notes are for people using Glint.
4.  **Build the release** (the build is most of it: a clean release build takes tens of minutes, an incremental one a few):

    ```powershell
    $env:TAURI_SIGNING_PRIVATE_KEY = "$HOME\.tauri\shadui-ripgrep.key"
    npm run release
    ```

    with the code-signing variables or `--sign-command` from above if you sign. It checks the crates first and stops on a known vulnerability, a license `deny.toml` doesn't allow, or an out-of-date `THIRD_PARTY_CRATES.md` ([what to do](#5-release-notes-and-the-crates-checks)).
5.  **Create the draft** with the `gh release create … --draft` command it prints. It uploads `Glint-setup.exe`, the `-setup.exe`, its `.sig` and `latest.json`, with your notes, and the tag is made at your commit when you publish.
6.  **On GitHub**, read the release notes (the app's *What's new* opens this page), try the installer, and **publish**. Running copies find the update within the hour (by the clock, so a PC that slept counts) or at their next start.
7.  **Submit** `release/vX.Y.Z/package-manifests` to winget and Scoop.
8.  **The Microsoft Store**, when it should have this version: `npm run msix`, then `npm run store` to see what would change and `npm run store -- --submit` to send it (add `--listing` when the screenshots or the listing's words changed), or upload the package in Partner Center ([below](#as-an-msix-package)). Write what's new for the version in `crates/glint/msix/listing.json` first.

### A trial with a throwaway key

To run the whole pipeline without the real key, and without touching `release/`:

```powershell
npx @tauri-apps/cli signer generate -w D:\trial\test.key        # a password of your choice
$env:TAURI_SIGNING_PRIVATE_KEY = "D:\trial\test.key"; $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = "<it>"
npm run release -- --allow-dirty --out D:\trial\release --pubkey D:\trial\test.key.pub
```

Everything is made and checked, with the throwaway key standing in for the app's: `verify-update.mjs` is run with its public key, and the service's signature is checked by `sign-service.mjs` with it (the service itself trusts only the committed key, so no installed Glint or service would take what a trial makes). It never publishes, uploads or installs anything.

### With the release workflow

Step 1 and 2 as above, then:

3.  **Try it** (optional): **Actions → Release → Run workflow** on `master` runs every check and builds (and signs) the installer without making a release; its files are in the run's `release-vX.Y.Z` artifact, and the Store's package in `msix-vX.Y.Z`. (`npm run ship` makes its release from exactly such a run.)
4.  **Tag and push**: `git tag vX.Y.Z` and `git push origin vX.Y.Z`. The workflow checks that the versions and the tag agree, runs the checks, installs NSIS (`choco install nsis`), builds with `cargo build --release -p glint -p shadui-index`, signs `Glint.exe` and the service with SignPath when it's set up (approve the requests there), signs the service's update and packs the installer with `scripts/gpui-installer.mjs`, signs the installer with SignPath, signs the update, and creates a draft release with `Glint-setup.exe`, the `-setup.exe`, its `.sig` and `latest.json`. It runs the crates' checks itself (`cargo deny check` and `npm run licenses -- --check`, with cargo-deny and cargo-about taken from their own releases and pinned by SHA-256), so a build on GitHub is checked whether or not the PC that asked for it has the two tools.
5.  **Notes**: `npm run notes`, edit `release/vX.Y.Z/notes.md`, and put it on the draft with `gh release edit vX.Y.Z --notes-file release/vX.Y.Z/notes.md`.
6.  **On GitHub**, as in step 6 above.
7.  **Submit** the manifests from the run's `release-vX.Y.Z` artifact (`package-manifests/`) to winget and Scoop.

Use one way per release: publishing a draft made by `npm run release` creates its tag, and the workflow then leaves that release as it is.

If a release turns out broken, publish a fixed one with a higher version: the updater never installs an older version.

## The installer

The NSIS installer is Glint's own script in [`crates/glint/installer/`](../crates/glint/installer/README.md), made from the Tauri build's (itself Tauri's template at 2.12.2, with Glint's pages), so `makensis` is all that builds it. [`scripts/gpui-installer.mjs`](../scripts/gpui-installer.mjs) gathers what it carries into a folder and runs `makensis` (found through `MAKENSIS`, the `PATH`, `%LOCALAPPDATA%\tauri\NSIS`, where Tauri's bundler kept one, or `Program Files\NSIS`; `winget install NSIS.NSIS` brings one). It installs over the Tauri build's in place: the same folder (`%LOCALAPPDATA%\Glint`), registry keys, shortcuts and switches. It carries no WebView2: the GPUI app draws everything itself.

-   **`installer.nsi`**: the script, with the switches above, the pages (Welcome, the reinstall question, the folder, the **Extra tools** page, the install, the finish page) and the uninstaller.
-   **`hooks.nsh`**: what runs before and after installing and uninstalling (ShadUI Ripgrep's leftovers, File Explorer's entries, the instant search service, the tools picked on the Extra tools page), and it brings in the two below.
-   **`look.nsh`**: Segoe UI, the welcome and finish pages' words, a question before **Cancel** quits, and the artwork for the screen's scaling.
-   **`tools.nsh`**: the **Extra tools** page: instant search and every tool Settings → Tools installs, grouped, each with what it is and what Glint uses it for; one already on the PC is greyed out. What's ticked installs after Glint itself, by Glint (`Glint.exe tools install <id>`, Settings → Tools' own code), with its progress on the installer's status line; the finish page says what didn't install. A test in `crates/glint-core/src/tools.rs` fails when a tool joins `TOOLS` without a row here. Silent installs pick them with `/TOOLS=difftastic,jaq` ([docs/tools.md](tools.md)).
-   **`art/`**: the pictures beside the welcome and finish pages and in the header, for 100% to 300% scaling, drawn by `npm run installer-art` from `crates/glint/installer/logo.svg` (run it again when the logo or their words change; it needs Microsoft Edge, and gives the committed files byte for byte).

## The Microsoft Store

Glint has been in the Store since 2026-10-31, as an MSIX package: [its listing](https://apps.microsoft.com/detail/9NQLRJ3HF1C9) (Store ID 9NQLRJ3HF1C9) is where the README's badge and the website's main buttons go, and `winget install 9NQLRJ3HF1C9 --source msstore` installs it from there too.

The Store takes Glint two ways. **As an MSIX package**, which `npm run msix` builds: Partner Center takes the file itself and the Store signs it, free, so no code-signing certificate is needed. **As an EXE app**, it downloads the setup program and runs it, and then that has to be code-signed ([below](#as-an-exe-app)).

### As an MSIX package

`npm run msix` ([`scripts/msix.mjs`](../scripts/msix.mjs)) builds Glint (`cargo build --release -p glint -p shadui-index`; `--no-build` packages what `target/ui/release` holds), lays it out with the instant search service, rg and fd, the notices, [`AppxManifest.xml`](../crates/glint/msix/AppxManifest.xml) and the Store's logos at every scale (drawn from `crates/glint/msix/icon.png` by [`scripts/msix-assets.ps1`](../scripts/msix-assets.ps1), with `resources.pri` to tell them apart), and packs it with the Windows SDK's makeappx into `release/vX.Y.Z/Glint_X.Y.Z_x64.msix`. It needs the Windows SDK, which Visual Studio's C++ tools bring. The service's update signature is made the same way as the setup program's (`TAURI_SIGNING_PRIVATE_KEY`).

The manifest matches what the app checks: the executable is `Glint.exe` (`store_copy()` and `package_family()` in `crates/glint-core/src/lib.rs` ask Windows which package the process is in, so they need no name), the execution alias is `glint.exe` (`verbs.rs` and the Jump List start it by it) and the startup task is `GlintTray` with `--tray` (`crates/glint-ui/src/platform/tray.rs`).

How the Store's copy differs from the setup program's:

-   **The Store updates it.** It doesn't look for updates itself, and **Settings → About** says the Store keeps it up to date.
-   **No File Explorer integration.** Windows keeps a packaged app's registry writes to itself, so *Open in Glint*, *Open folders in Glint by default* and *Win+E opens Glint* say they need the setup program's Glint.
-   **Start with Windows** is the package's startup task (`GlintTray`, which starts it with `--tray`), which Windows' **Startup apps** can turn off; then only they can turn it on again.
-   **AI agents and the Jump List** start it by its `glint.exe` alias, `%LOCALAPPDATA%\Microsoft\WindowsApps\<package family>\glint.exe`, which stays put as updates come (the package's own folder has its version in its name).
-   **Instant search** turns on as it does elsewhere, with one administrator approval. Uninstalling a Store app runs nothing of Glint's, so the service learns which package turned it on, checks for it every half hour, and removes itself within the hour once it's gone for every account, unless a copy from the setup program, Scoop or a portable one uses it too ([how](instant-search.md#security-model)). A test package installed with `--try` doesn't name itself, so taking it out leaves the service where it is.
-   **Its data folder.** The Store's copy keeps its data in `com.shado.glint.store` (`glint_core::host::STORE_IDENTIFIER`, chosen when `package_family()` is set), never in `com.shado.shadui-ripgrep`, which the setup copy uses, so the two on one PC don't share settings. Windows lets a packaged app read the real `%APPDATA%` and `%LOCALAPPDATA%` and keeps what it writes there in an overlay of its own (`%LOCALAPPDATA%\Packages\<family>\LocalCache`, removed with the package), which is why the folders must differ. The first run of a Store copy fills both of its folders once from the shared ones as it sees them (`crates/glint-core/src/host/store_data.rs`): files it doesn't have yet are copied, never over one it has, the old web view (`EBWebView`), `updates` and `content` are left out, and a folder over 256 MB isn't copied; the shared folder is only read, never changed or removed. A `.filled-from-shared` file marks a folder done (a copy that couldn't finish a file tries again at the next start). Settings travel between the two with **Settings → About → Settings file**.

**Once:**

1.  Sign up as an individual developer at [storedeveloper.microsoft.com](https://storedeveloper.microsoft.com/) (free; a government ID and a selfie), which opens Partner Center.
2.  **Apps and games → New product → MSIX or PWA app**, and reserve the name (*Glint*, or *Glint File Explorer* if that's taken).
3.  The app's **Product management → Product identity** has three values the package must carry: put `Package/Identity/Name`, `Package/Identity/Publisher` and `Package/Properties/PublisherDisplayName` in [`crates/glint/msix/store.json`](../crates/glint/msix/store.json) as `name`, `publisher` and `publisherDisplayName`, the name you reserved as `displayName` and the Store ID as `storeId` (for links to the listing), and commit it. It's there already (the Store ID is 9NQLRJ3HF1C9); until it is, `npm run msix` makes a test package (`…_test.msix`), which the Store won't take.
4.  Fill in the Store listing: the description from the README, screenshots from `screenshots/`, the privacy policy ([SECURITY.md](../SECURITY.md#code-signing-and-privacy-policy)), the category and the age rating questions. The package declares `runFullTrust`, as every desktop app does; when Partner Center asks why, it's a desktop file explorer that reads and changes files anywhere the user can.

**For a release the Store should offer:** on the release's commit (after `npm run release`, or instead of it), `npm run msix`, then `npm run store` ([below](#sending-it-with-npm-run-store)), or by hand in Partner Center: a new submission with `release/vX.Y.Z/Glint_X.Y.Z_x64.msix` under **Packages**, and **Submit for certification**. The Store signs it; certification takes from hours to a few days. Its version can only go up, and the Store keeps the fourth number (`X.Y.Z.0`) for itself.

#### Sending it with `npm run store`

[`scripts/store.mjs`](../scripts/store.mjs) sends a version through the [Microsoft Store submission API](https://learn.microsoft.com/windows/uwp/monetize/create-and-manage-submissions-using-windows-store-services) instead of Partner Center's pages: the package, what's new in it and, when asked, the listing itself (screenshots and captions, description, features). What the listing says is in [`crates/glint/msix/listing.json`](../crates/glint/msix/listing.json): `whatsNew` with the `version` it was written for (a note for another version is never put on this one), and for each language of the listing its `description`, `features` and `screenshots` (files in `screenshots/`, in the order the Store shows them, each with its caption).

```powershell
npm run store                        # looks only: what the Store has, and what a submission would change
npm run store -- --draft --listing   # makes the submission and uploads its files, not committed
npm run store -- --commit            # commits it: certification, then the Store publishes it by itself
npm run store -- --submit            # the two in one, for a release whose listing stays as it is
npm run store -- --status            # where the pending submission is, and the certification report's address
npm run store -- --discard           # deletes the pending submission
```

-   **It says what it will do first.** With no mode it changes nothing. `--draft` leaves the submission uncommitted so it can be read in Partner Center: read it there, don't change it there, because the API can't change or commit a submission that was edited by hand (delete it and start over if that happens: `--discard`).
-   **The package is checked before anything is sent**: it must carry the Store identity of `store.json` and this version, so a test package or last version's file stops there. The new package replaces the ones of its architecture (`--keep-packages` adds it beside them). A version the Store already has is refused.
-   **`--listing` replaces every screenshot** of the languages in `listing.json` with the listed ones (at most 10, PNG, 1366 x 768 or larger, captions of at most 200 characters) and sets the description and features. Without it, only the package and what's new change. The logos and everything else in the listing stay.
-   **A submission that is already pending stops it** (one started in Partner Center, or an earlier `--draft`): finish that one, or `--replace-pending` deletes it first.

**Once**, so that the script may act for the account ([Microsoft's page](https://learn.microsoft.com/windows/uwp/monetize/create-and-manage-submissions-using-windows-store-services#how-to-associate-an-azure-ad-application-with-your-partner-center-account) has the clicks):

1.  In Partner Center's **Account settings**, associate a Microsoft Entra directory with the account (Partner Center can make one, free, for an account that has none).
2.  On the account settings' **Users** page, add a **Microsoft Entra application** (a new one is fine: it only names the script) with the **Manager** role.
3.  Open the application there and copy its **Tenant ID** and **Client ID**, then **Add new key** and copy the key: it is shown once.
4.  **`npm run ship -- --setup`** ([above](#a-release-that-needs-nobody)) asks for the three, tries them against the Store and stores them as the repository's secrets, where `npm run ship` and the **Store** workflow use them. To use `npm run store` from this PC as well, set `PARTNER_CENTER_TENANT_ID` and `PARTNER_CENTER_CLIENT_ID` for your account (they aren't secrets; `setx` keeps them): it asks for the key without showing it, or takes it from `PARTNER_CENTER_CLIENT_SECRET` for that run. Keep the key in a password manager. A key expires (Partner Center says when) and a new one is made the same way, then `npm run ship -- --setup` again.

The pages, as Partner Center has them (2026-11): **Account settings → Tenants** (`https://partner.microsoft.com/dashboard/account/v3/tenantmanagement`) has *Create Microsoft Entra ID* and *Associate Microsoft Entra ID*; **Account settings → User management** (`…/usermanagement`) asks you to sign in with that directory's account before it shows users and applications.

**The Store workflow** ([`store.yml`](../.github/workflows/store.yml)) does on GitHub what `npm run ship`'s store step does on a PC, for a version that is released: **Actions → Store → Run workflow** with the version. Its mode is *look* unless told otherwise: it packs the package from the released setup program, keeps it as the run's artifact (`store-vX.Y.Z`) and says what a submission would change, sending nothing. *submit* sends it; *draft*, *commit*, *status* and *discard* are `npm run store`'s. Without the secrets it ends after packing, with the package for Partner Center's pages.

The key acts for the whole Partner Center account, as a Manager: it is never committed, and the script sends it only to Microsoft's sign-in. The API doesn't work for an app that uses mandatory updates or has never had a submission (the first one is made in Partner Center, [above](#as-an-msix-package)). `npm run test:store` runs the script against a stand-in for the Store that checks every request against the API's pages.

**To try it here** (with Developer Mode on): `npm run msix -- --try` installs a test package from its folder, *Glint (test)* in Start, under its own identity so it can never stand in the Store copy's way; `npm run msix -- --untry` takes it out, with everything it kept. `--from <folder>` packages a build made elsewhere instead of building, such as a debug build.

### As an EXE app

The Store can also list Glint as an **EXE app**: it downloads the installer from a link you give it and runs it silently, and a copy installed that way updates itself like any other. Microsoft's [requirements](https://learn.microsoft.com/windows/apps/publish/publish-your-app/msi/app-package-requirements) for that installer:

-   It and every program in it are code-signed with a certificate from a CA in Microsoft's Trusted Root Program ([code signing](#2-code-signing-optional-recommended)), so the Store needs signed releases.
-   It downloads nothing while it runs. The Tauri build's installer fetched WebView2 on a PC that didn't have it, so the Store got a separate **offline installer** of 220 MB. The GPUI installer carries no WebView2 and downloads nothing, so **the normal `Glint_X.Y.Z_x64-setup.exe` is the one the Store takes**, and `npm run release` no longer makes another (its old `--store` option is gone).
-   It installs without showing anything when run with `/S`. The NSIS installer does, and the installer hooks only ask their question when the install isn't silent.
-   Its address names the version, and the file there never changes: a release's download link, as long as nobody replaces the file.

Sign up as above, but make the product an **EXE or MSI app**. For a release the Store should offer (not every release needs it: copies from the Store update themselves from GitHub, so one installed from an older Store version is offered the newest at its first start): build the release code-signed (`npm run release` with the Azure variables or `--sign-command`), publish it, give Partner Center the installer's link (`https://github.com/shadijhade/glint/releases/download/vX.Y.Z/Glint_X.Y.Z_x64-setup.exe`) for x64, with `/S` where it asks how to install silently, and **Submit for certification**. Certification takes a few days.

## The website

[glint.rafd.tech](https://glint.rafd.tech/) is Glint's home page, one of Rafd's free innovations, built the way [rafd.tech](https://rafd.tech/) is: [`site/`](../site) is an Astro project of its own (GSAP and Lenis for the motion, a hand-written WebGL2 field in `site/src/scripts/field.ts`), published from this PC as the Cloudflare Worker `glint-site`, static assets only. `site/wrangler.toml` declares the domain, so every deploy keeps glint.rafd.tech attached (DNS record and certificate) on the rafd.tech zone.

```powershell
cd site
npm install
npm run dev      # http://localhost:4321
npm run preview  # build + the real Workers runtime (headers and CSP included) on :8787
npm run deploy   # build + wrangler deploy (signs in to Cloudflare in a browser the first time)
```

`npm run site` at the root runs its dev server. All the words and numbers are in `site/src/content/site.ts`, every time and count there is the README's, and the screenshots come from `screenshots/`, made into AVIF and WebP when it builds (the cards under *More* name them in `MORE.cards`). The clip above those cards is `site/public/media/glint-demo.mp4` with its poster `glint-demo.webp`: the README's `screenshots/demo.gif` as a video, which plays while it is on screen and waits to be asked with reduced motion. The search demo searches `site/public/data/names.txt`: 9,474 of Windows' own file names (programs whose version information names Microsoft; no fonts, drivers or other files a third party installed), worth regenerating only if Windows changes a lot.

Its main buttons go to the Microsoft Store's listing (`SITE.store`) and its installer links to `releases/latest/download/Glint-setup.exe`, like the README's; the page asks GitHub's API for the latest release to show its version and the installer's size. Nothing on it changes for a release; deploy it again when its words, numbers or screenshots change.

## Trying the updater

What can be checked without the real key, the real install or GitHub:

-   **The signing and the release's checks**: `npm run test:signing` (the signature is what Tauri wrote, checked against stored vectors) and [a trial with a throwaway key](#a-trial-with-a-throwaway-key) (the whole pipeline, with `verify-update.mjs` run on the result).
-   **The app's side**: the updater's own tests in `crates/glint-ui/src/updater.rs` run the feed, the version rules, the signature checks (a wrong key, a changed byte, another version's signature, a signature without a version) and the setup program's command line (`/P /UPDATE /R /ARGS …`) with a throwaway key and fake servers.
-   **The whole update on a PC**: the app reads one address and one key (`MANIFEST_URL` and `PUBLIC_KEY`, both constants in `updater.rs`) and refuses an `http:` address, so there is no override to point a build at a local server. Watching a real install replace an older one means a build of an older version (a throwaway checkout with a lower `[workspace.package] version`) whose two constants (and `crates/shadui-index/update-pubkey.txt`) name a test key and an `https:` address you control, a newer build signed by `release.mjs --pubkey`, and that address serving its `latest.json` and installer. The test copy must not be the real one: install it to its own folder (`/D=<folder>`, with `/NS`), never run the setup on the PC's real Glint, and its uninstaller removes the instant search service, so delete the folder instead.
