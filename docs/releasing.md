# Releasing

Releases are built one of two ways: by the release workflow on GitHub when a `vX.Y.Z` tag is pushed (the only way to get them signed by SignPath), or on the maintainer's PC with `npm run release`. Both make the same files. This page covers what to set up once and what to do for each release.

## How it works

1.  **`npm run release`** ([`scripts/release.mjs`](../scripts/release.mjs)) checks that `package.json`, `src-tauri/Cargo.toml` and `src-tauri/tauri.conf.json` agree on the version and that everything is committed, then builds the NSIS installer (code-signed when signing is set up) and signs it for the in-app updater. It writes `latest.json` and checks the update the way the app will: the signature must verify with the public key the app is built with and be bound to the version `latest.json` announces. It also writes the winget and Scoop manifests. Everything lands in `release/vX.Y.Z/`, and it prints the `gh release create` command that makes the draft release.
2.  **Publishing the draft** makes it the latest release. Copies of the app installed with the setup program read `https://github.com/shadijhade/glint/releases/latest/download/latest.json` at start and every 12 hours, say *Update available* once, and keep an **Update** button in the header. **Install** (in the notice or in **Settings → About**) downloads the installer, verifies its signature, waits for any copy or move to finish, closes the app, installs over it with a progress bar and opens the new version.

Copies that the setup program didn't install (Scoop, a portable copy, a dev build) don't install updates themselves: the app tells them about the new version and links to it. A release without `latest.json` gets the same treatment from every copy, through GitHub's latest-release API.

**The instant search service updates separately.** It runs from `C:\Program Files\ShadUI Ripgrep\`, which an app update doesn't touch (the in-app updater runs the installer in update mode, so its uninstall hook leaves the service alone). The app and the service share one version, `[workspace.package] version` in `src-tauri/Cargo.toml`, so after an app update the service reports an older version: the app says *Instant search has an update* once, and **Settings → Instant search** shows **Update** until it's done (one administrator approval; it runs the same `shadui-index.exe install` as turning it on).

## One-time setup

### 1. The updater key (needed for auto-update)

Updates are signed with a minisign key pair. The public key is compiled into the app; the private key signs each release, on your PC or as a secret of the release workflow. Set this up **before the first release that has the updater** (the first one after v0.2.0): every update after it must be signed with the matching private key. v0.2.0 itself predates the updater, so its users get a link to that release and install it by hand once.

1.  Generate the pair and choose a password when asked:

    ```bash
    npm run tauri signer generate -- -w ~/.tauri/shadui-ripgrep.key
    ```

    This writes `shadui-ripgrep.key` (private) and `shadui-ripgrep.key.pub` (public) in `%USERPROFILE%\.tauri`, outside the repository.
2.  Paste the contents of `shadui-ripgrep.key.pub` into `src-tauri/tauri.conf.json` as `plugins.updater.pubkey`, and commit it. It's public.
3.  Keep a copy of the private key and its password in a password manager. **If they're lost**, installed copies can't verify new updates: you'd need a new pair, and everyone would have to download the next version by hand once (the app still tells them it exists). **If they leak**, rotate the same way.

The release workflow needs the key's contents and its password as the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (**Settings → Secrets and variables → Actions**). Without them a tag push fails rather than publish a release the app can't install by itself.

`npm run release` needs `TAURI_SIGNING_PRIVATE_KEY` set to the key file's path (or its contents), and asks for the password unless `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is set. It refuses to build with only one half (a key without a public key in `tauri.conf.json`, or the other way round); with neither, it builds without auto-update files and says so.

The app sets `requireSignedVersion`: a signature must name the version it was signed for (`tauri build` records it), so a tampered `latest.json` can't pass off an older, validly signed installer as new. Updates never go backwards either.

### 2. Code signing (optional, recommended)

Unsigned installers work, but Windows SmartScreen shows *Windows protected your PC* until a file has a reputation, and the UAC prompt for instant search shows *Unknown publisher*. Signing puts a verified name on the installer, the app and the service (a build signed on your PC also signs the bundled tools and the uninstaller). Reputation still builds with downloads; since 2024 even EV certificates don't skip that.

Since June 2023 a publicly trusted code-signing key must live in hardware (a smartcard, a token or a cloud HSM), so a `.pfx` file is no longer an option for new certificates. What's left for an open-source project:

| Option | Cost | Who can get it | Name on the signature | With `npm run release` |
| :--- | :--- | :--- | :--- | :--- |
| [Azure Artifact Signing](https://learn.microsoft.com/azure/artifact-signing/quickstart) (formerly Trusted Signing) | About $10 a month (Basic) | Organizations in the US, Canada, the EU, the UK, Australia, New Zealand, Japan, South Korea, Singapore, Switzerland, Norway and Israel; individual developers in the US and Canada | Your verified name | Built in: three environment variables |
| [Certum](https://shop.certum.eu/code-signing.html): *Open Source Code Signing* (with a smartcard and reader) or *Standard Code Signing* in the cloud (SimplySign) | About €69 or from €49 | Individuals and companies Certum validates, in most countries | Your name | `--sign-config` with the certificate's thumbprint |
| Another CA with a hardware or cloud key (SSL.com eSigner, DigiCert KeyLocker, Sectigo…) | Roughly €200 to €500 a year | Anyone the CA validates | Your name | `--sign-config` with its signing tool |
| [SignPath Foundation](https://signpath.org/) | Free for open source | OSI-licensed projects with no proprietary parts | SignPath Foundation | Applied in October 2026 and declined until Glint is better known (stars, forks, articles, discussions); they invite a new application. It only signs builds made on GitHub-hosted runners, so it's the release workflow's (below) |

**Azure Artifact Signing.** `npm run release` fetches Microsoft's Artifact Signing client for `signtool` (pinned by its SHA-256) into `release/.tools`, has Tauri run it on every executable it bundles and then on the installer (before the update signature, since signing changes the file), and checks the signatures afterwards.

1.  In an Azure subscription, register the `Microsoft.CodeSigning` resource provider, create an Artifact Signing account (note its region's endpoint, such as `https://weu.codesigning.azure.net`), complete a **Public** identity validation, and create a **Public Trust** certificate profile. Microsoft's [quickstart](https://learn.microsoft.com/azure/artifact-signing/quickstart) walks through it; validation takes from a few hours to a few weeks.
2.  Assign your account the **Artifact Signing Certificate Profile Signer** role on the certificate profile.
3.  On your PC: the [Azure CLI](https://learn.microsoft.com/cli/azure/install-azure-cli-windows) (then `az login`), the .NET 8 runtime and the Windows SDK (`signtool` 10.0.26100 or newer; Visual Studio's C++ tools include it).
4.  Before `npm run release`, set `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT` and `AZURE_SIGNING_PROFILE`. (An app registration works too: set `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` and `AZURE_TENANT_ID` instead of `az login`.)

**A certificate in the Windows certificate store** (Certum's card, SimplySign's virtual card, most CAs' tokens): write a Tauri config overlay such as `%USERPROFILE%\.tauri\sign.json`

```json
{ "bundle": { "windows": { "certificateThumbprint": "<SHA-1 thumbprint>", "digestAlgorithm": "sha256", "timestampUrl": "http://time.certum.pl" } } }
```

and run `npm run release -- --sign-config %USERPROFILE%\.tauri\sign.json`. For a provider whose tool isn't `signtool` (SSL.com's CodeSignTool, DigiCert's `smctl`), put a `signCommand` there instead: `{ "cmd": "<tool>", "args": [..., "%1"] }`, where `%1` is the file to sign.

Tauri signs rg, fd and pdu where they are, in `src-tauri/binaries`; `npm run release` puts the official files back after the build, so only the installer carries the signed copies. (If a signed build is cut short and `git status` shows them changed, `git checkout -- src-tauri/binaries`.)

**SignPath Foundation** (the release workflow). Once the project is approved, set the repository secrets `SIGNPATH_API_TOKEN` and `SIGNPATH_ORGANIZATION_ID`. The workflow submits the app and the service (artifact configuration `zip`), then the installer (`exe`), to project `glint` with signing policy `release-signing`, and waits up to an hour for each request to be approved on SignPath. It checks the signatures before anything is uploaded. Without the secrets it builds unsigned.

Once releases are signed, take the SmartScreen note out of the README's Install section.

### 3. The GitHub CLI

`npm run release` prints the command that creates the draft release; it needs [gh](https://cli.github.com/) (`winget install GitHub.cli`, then `gh auth login`). The web page for a new release works too: upload the files from `release/vX.Y.Z/`.

### 4. winget and Scoop (from the first release with the updater)

`npm run release` writes `release/vX.Y.Z/package-manifests/`: `winget/manifests/s/shadijhade/Glint/X.Y.Z/` (three YAML files) and `scoop/glint.json`. Submit them once the release is published, since they point at its download. For a release that's already out, `node scripts/package-manifests.mjs X.Y.Z` makes them from GitHub's copy of the installer.

**winget**:

1.  Read the manifests: check the publisher name, description and tags in the `.locale.en-US.yaml`.
2.  `winget validate --manifest <folder>`. To try the install, enable local manifests once from an administrator prompt (`winget settings --enable LocalManifestFiles`) and run `winget install --manifest <folder>`.
3.  Install [wingetcreate](https://github.com/microsoft/winget-create) (`winget install Microsoft.WingetCreate`) and create a classic GitHub token with the `public_repo` scope.
4.  `wingetcreate submit --token <token> <folder>` opens a pull request to [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs). Its bots validate and test-install it, and a moderator merges it; then `winget install shadijhade.Glint` works.

For later versions, submit the new folder the same way (or let `wingetcreate update shadijhade.Glint --version X.Y.Z --urls <installer url> --submit --token <token>` write it).

**Scoop**: the manifest extracts the installer with 7-Zip instead of running it, so Scoop owns updates (`scoop update`) and the app only links to new versions. `scoop install .\glint.json` tries it locally (on a PC where instant search is on, `scoop uninstall` then asks to remove the service too, and it takes out File Explorer's *Open in Glint* entries if that copy turned them on). To publish it, either:

-   open a pull request adding `bucket/glint.json` to [ScoopInstaller/Extras](https://github.com/ScoopInstaller/Extras) after reading its contributing guide (its bot then picks up new releases by itself through `checkver` and `autoupdate`), or
-   keep it in a bucket of your own: a repository with the file at `bucket/glint.json`, added with `scoop bucket add shadijhade https://github.com/shadijhade/<repository>` and installed with `scoop install shadijhade/glint`. Update the file for each release.

Once a package is live, add its install command to the README's Install section.

## Every release

1.  **Bump the version** to the same `X.Y.Z` in three places:
    -   `package.json`: `npm version X.Y.Z --no-git-tag-version` (updates `package-lock.json` too)
    -   `src-tauri/Cargo.toml`: `[workspace.package] version`, which the app and the instant search service share; then `cargo check --manifest-path src-tauri/Cargo.toml` to update `Cargo.lock`
    -   `src-tauri/tauri.conf.json`: `version`
2.  **Commit** (`Release vX.Y.Z`) and push to `master`.
3.  **Build the release** (5 to 10 minutes):

    ```powershell
    $env:TAURI_SIGNING_PRIVATE_KEY = "$HOME\.tauri\shadui-ripgrep.key"
    npm run release
    ```

    with the code-signing variables or `--sign-config` from above if you sign.
4.  **Create the draft** with the `gh release create … --draft` command it prints. It uploads the `-setup.exe`, its `.sig` and `latest.json`, and the tag is made at your commit when you publish.
5.  **On GitHub**, write the release notes (the app's *What's new* opens this page), try the installer, and **publish**. Running copies find the update within 12 hours or at their next start.
6.  **Submit** `release/vX.Y.Z/package-manifests` to winget and Scoop.

### With the release workflow

Steps 1 and 2 as above, then:

3.  **Try it** (optional): **Actions → Release → Run workflow** on `master` runs every check and builds (and signs) the installer without making a release; its files are in the run's `release-vX.Y.Z` artifact.
4.  **Tag and push**: `git tag vX.Y.Z` and `git push origin vX.Y.Z`. The workflow checks that the three versions and the tag agree, runs the checks, signs with SignPath when it's set up (approve the requests there), signs the update, and creates a draft release with the `-setup.exe`, its `.sig` and `latest.json`.
5.  **On GitHub**, as in step 5 above.
6.  **Submit** the manifests from the run's `release-vX.Y.Z` artifact (`package-manifests/`) to winget and Scoop.

Use one way per release: publishing a draft made by `npm run release` creates its tag, and the workflow then leaves that release as it is.

If a release turns out broken, publish a fixed one with a higher version: the updater never installs an older version.

## Trying the updater locally

The flow can be tested end to end without the real key, the real install or GitHub, with two builds that differ only in version, a test key and a local `latest.json`:

1.  `npm run tauri signer generate -- --ci -p "" -w <scratch>/test.key`.
2.  Write a config overlay, `<scratch>/test.json`, that gives the test builds their own name (so they install next to the real app, not over it), drops the installer hooks (so uninstalling the test copy can't remove the real instant search service), and points the updater at a local server:

    ```json
    {
      "productName": "glint-updatetest",
      "identifier": "com.shado.shadui-ripgrep.updatetest",
      "bundle": { "createUpdaterArtifacts": true, "windows": { "nsis": { "installerHooks": null } } },
      "plugins": {
        "updater": {
          "pubkey": "<contents of test.key.pub>",
          "endpoints": ["http://127.0.0.1:8765/latest.json"],
          "dangerousInsecureTransportProtocol": true
        }
      }
    }
    ```
3.  Build the old and the new version, with `TAURI_SIGNING_PRIVATE_KEY=<scratch>/test.key` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD=` set: `npm run tauri build -- --bundles nsis --config <scratch>/test.json --config <scratch>/old.json`, where `old.json` is just `{ "version": "0.2.0" }`, then the same with a `new.json` (a file rather than inline JSON, which npm's Windows shell mangles). For the new build, also bump `[workspace.package] version` for the moment, to see the instant search update offer, and put it back afterwards.
4.  Serve the new installer at `http://127.0.0.1:8765/releases/download/v<new>/<installer>` next to a `latest.json`:

    ```json
    {
      "version": "<new>",
      "pub_date": "2026-10-16T12:00:00Z",
      "platforms": {
        "windows-x86_64": {
          "url": "http://127.0.0.1:8765/releases/download/v<new>/glint-updatetest_<new>_x64-setup.exe",
          "signature": "<contents of the installer's .sig>"
        }
      }
    }
    ```

    and check it: `node scripts/verify-update.mjs latest.json <installer> "<contents of test.key.pub>"`.
5.  Install the old build with `/S /NS` (silent, no shortcuts), start it from `%LOCALAPPDATA%\glint-updatetest`, and install the update from the notice or **Settings → About**: the app closes, the installer's progress bar shows, and the new version opens. To see a refusal, announce a different version in `latest.json` or change a byte of the installer.
6.  Afterwards, uninstall the test copy with its `uninstall.exe /S` and delete `%LOCALAPPDATA%\com.shado.shadui-ripgrep.updatetest`.
