// Writes the winget and Scoop manifests for a release, from its installer's SHA-256. scripts/release.mjs
// runs it; by hand, it can also make them for a published release.
//
// Usage: node scripts/package-manifests.mjs <version> [installer-setup.exe] [--out <dir>]
// Without an installer it downloads the release's setup.exe from GitHub. The manifests land in
// <out>/winget/manifests/s/shadijhade/ShadUIRipgrep/<version>/ (winget-pkgs' layout, ready for
// `wingetcreate submit`) and <out>/scoop/shadui-ripgrep.json; <out> defaults to
// release/v<version>/package-manifests. docs/releasing.md says how to submit them.

import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const outIndex = args.indexOf("--out");
const outArg = outIndex >= 0 ? args.splice(outIndex, 2)[1] : undefined;
const [version, installerPath] = args;
const out = outArg ?? join("release", `v${version}`, "package-manifests");
if (!/^\d+\.\d+\.\d+$/.test(version ?? "") || (outIndex >= 0 && !outArg)) {
    console.error("Usage: node scripts/package-manifests.mjs <version> [installer-setup.exe] [--out <dir>]");
    process.exit(2);
}

const REPO = "https://github.com/shadijhade/shadui-ripgrep";
const ID = "shadijhade.ShadUIRipgrep";
const SHORT = "A fast file explorer for Windows: every folder's size, instant search across NTFS drives, and ripgrep search inside files.";
const installerName = `shadui-ripgrep_${version}_x64-setup.exe`;
const installerUrl = `${REPO}/releases/download/v${version}/${installerName}`;

const installer = installerPath ? readFileSync(installerPath) : await download(installerUrl);
const sha256 = createHash("sha256").update(installer).digest("hex").toUpperCase();

async function download(url) {
    const response = await fetch(url);
    if (!response.ok) throw new Error(`${url}: ${response.status} ${response.statusText}`);
    return Buffer.from(await response.arrayBuffer());
}

// winget: what `wingetcreate new` writes, as a version, an installer and a default-locale manifest.
// The installer is Tauri's NSIS setup (per user, /S for silent); AppsAndFeaturesEntries match what it
// registers (the uninstall key is named after productName, the publisher comes from the identifier).
const SCHEMA = "1.10.0";
const header = (type) => `# yaml-language-server: $schema=https://aka.ms/winget-manifest.${type}.${SCHEMA}.schema.json\n\nPackageIdentifier: ${ID}\nPackageVersion: ${version}\n`;
const footer = (type) => `ManifestType: ${type}\nManifestVersion: ${SCHEMA}\n`;
const winget = {
    [`${ID}.yaml`]: `${header("version")}DefaultLocale: en-US\n${footer("version")}`,
    [`${ID}.installer.yaml`]: `${header("installer")}InstallerType: nullsoft
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
AppsAndFeaturesEntries:
- DisplayName: shadui-ripgrep
  Publisher: shado
  ProductCode: shadui-ripgrep
Installers:
- Architecture: x64
  InstallerUrl: ${installerUrl}
  InstallerSha256: ${sha256}
${footer("installer")}`,
    [`${ID}.locale.en-US.yaml`]: `${header("defaultLocale")}PackageLocale: en-US
Publisher: Shadi Sadi
PublisherUrl: https://github.com/shadijhade
PublisherSupportUrl: ${REPO}/issues
Author: Shadi Sadi
PackageName: ShadUI Ripgrep
PackageUrl: ${REPO}
License: MIT
LicenseUrl: ${REPO}/blob/master/LICENSE
ShortDescription: "${SHORT}"
Description: |-
  ShadUI Ripgrep is a file explorer for Windows that opens folders in milliseconds and shows the size of every folder.
  Name searches use fd, or an opt-in NTFS index service (like Everything's) that finds any file on any drive in milliseconds.
  Text searches inside files use ripgrep, with regular expressions, replace and a syntax-highlighted preview.
  File operations go through Windows' own shell engine, so the Recycle Bin, conflicts and permissions work as in File Explorer.
Moniker: shadui-ripgrep
Tags:
- disk-usage
- everything
- explorer
- fd
- file-explorer
- file-manager
- folder-size
- ripgrep
- search
ReleaseNotesUrl: ${REPO}/releases/tag/v${version}
${footer("defaultLocale")}`,
};

// Scoop: extracts the NSIS setup with 7-Zip instead of running it. Without the setup's uninstall.exe the
// app knows it didn't come from the installer and leaves updating to `scoop update` (see lib.rs).
const scoop = {
    version,
    description: SHORT,
    homepage: REPO,
    license: "MIT",
    notes: [
        "Update with 'scoop update shadui-ripgrep'; the app links to new versions but doesn't install them itself.",
        "Instant search, once turned on in Settings, runs as a Windows service; 'scoop uninstall' removes it too.",
    ],
    architecture: {
        "64bit": {
            url: `${installerUrl}#/dl.7z`,
            hash: sha256.toLowerCase(),
        },
    },
    pre_install: "Remove-Item \"$dir\\`$PLUGINSDIR\", \"$dir\\uninstall.exe\" -Recurse -Force -ErrorAction SilentlyContinue",
    uninstaller: {
        script: [
            "# Instant search's service runs from Program Files; uninstalling (not updating) removes it, as the setup's uninstaller does.",
            "if ($cmd -eq 'uninstall' -and (Get-Service ShadUIRipgrepIndex -ErrorAction SilentlyContinue)) {",
            "    Start-Process \"$dir\\shadui-index.exe\" -ArgumentList uninstall -Verb RunAs -Wait",
            "}",
        ],
    },
    shortcuts: [["shadui-ripgrep.exe", "ShadUI Ripgrep"]],
    checkver: "github",
    autoupdate: {
        architecture: {
            "64bit": {
                url: `${REPO}/releases/download/v$version/shadui-ripgrep_$version_x64-setup.exe#/dl.7z`,
            },
        },
    },
};

const wingetDir = join(out, "winget", "manifests", "s", "shadijhade", "ShadUIRipgrep", version);
const scoopDir = join(out, "scoop");
mkdirSync(wingetDir, { recursive: true });
mkdirSync(scoopDir, { recursive: true });
for (const [name, text] of Object.entries(winget)) writeFileSync(join(wingetDir, name), text);
writeFileSync(join(scoopDir, "shadui-ripgrep.json"), `${JSON.stringify(scoop, null, 4)}\n`);
console.log(`${installerName} SHA-256 ${sha256}\nwinget: ${wingetDir}\nScoop:  ${join(scoopDir, "shadui-ripgrep.json")}`);
