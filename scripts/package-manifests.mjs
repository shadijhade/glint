// Writes the winget and Scoop manifests for a release, from its installer's SHA-256. scripts/release.mjs
// runs it; by hand, it can also make them for a published release.
//
// Usage: node scripts/package-manifests.mjs <version> [installer-setup.exe] [--out <dir>]
// Without an installer it downloads the release's setup.exe from GitHub. The manifests land in
// <out>/winget/manifests/s/shadijhade/Glint/<version>/ (winget-pkgs' layout, ready for
// `wingetcreate submit`) and <out>/scoop/glint.json; <out> defaults to
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

const REPO = "https://github.com/shadijhade/glint";
const ID = "shadijhade.Glint";
const SHORT = "A fast file explorer for Windows: every folder's size, instant search across NTFS drives, and ripgrep search inside files.";
const installerName = `Glint_${version}_x64-setup.exe`;
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
- DisplayName: Glint
  Publisher: shado
  ProductCode: Glint
Installers:
- Architecture: x64
  InstallerUrl: ${installerUrl}
  InstallerSha256: ${sha256}
${footer("installer")}`,
    [`${ID}.locale.en-US.yaml`]: `${header("defaultLocale")}PackageLocale: en-US
Publisher: Shadi Saadi
PublisherUrl: https://github.com/shadijhade
PublisherSupportUrl: ${REPO}/issues
Author: Shadi Saadi
PackageName: Glint
PackageUrl: ${REPO}
License: MIT
LicenseUrl: ${REPO}/blob/master/LICENSE
ShortDescription: "${SHORT}"
Description: |-
  Glint is a file explorer for Windows that opens folders in milliseconds and shows the size of every folder.
  Name searches use fd, or an opt-in NTFS index service (like Everything's) that finds any file on any drive in milliseconds.
  Text searches inside files use ripgrep, with regular expressions, replace and a syntax-highlighted preview.
  File operations go through Windows' own shell engine, so the Recycle Bin, conflicts and permissions work as in File Explorer.
Moniker: glint
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
// Uninstalling it (not updating: Scoop's $cmd tells them apart) does what the setup's uninstaller does.
//
// File Explorer's entries: a Scoop copy adds them running Glint through the app's `current` link, which
// updates keep (src-tauri/src/verbs.rs). Uninstalling takes them out as GLINT_REMOVE_EXPLORER_VERBS does
// for the setup's copies (src-tauri/installer/hooks.nsh); verbs.rs tests it against what turning the
// options on adds.
const removeExplorerVerbs = String.raw`
# File Explorer's 'Open in Glint', opening folders in Glint by default and Win+E (Settings > Files) run Glint through 'current'.
# Uninstalling (not updating) removes them where they run this copy, puts back what folders opened with before, and
# removes the keys above them if that leaves them empty, as the setup's uninstaller does. If that fails, the uninstall
# stops there, leaving Glint where they can still run it.
if ($cmd -eq 'uninstall') {
    $ErrorActionPreference = 'Stop'
    $verb = 'Glint'
    $exe = '"' + (Split-Path $dir) + '\current\Glint.exe"'
    $classes = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes', $true)
    $runs = {
        param($place)
        $command = $classes.OpenSubKey("$place\$verb\command")
        if ($command) { "$($command.GetValue(''))".StartsWith($exe, 'OrdinalIgnoreCase'); $command.Close() }
    }
    $changed = $false
    if (& $runs 'Directory\shell') {
        $folders = $classes.OpenSubKey('Directory\shell', $true)
        if ($folders.GetValue('') -eq $verb) {
            $kept = $folders.OpenSubKey($verb)
            if ($kept.GetValueNames() -contains 'PreviousDefault') {
                $folders.SetValue('', $kept.GetValue('PreviousDefault', $null, 'DoNotExpandEnvironmentNames'), $kept.GetValueKind('PreviousDefault'))
            } else {
                $folders.DeleteValue('')
            }
            $kept.Close()
        }
        $folders.Close()
    }
    foreach ($place in 'Directory\shell', 'Directory\Background\shell', 'Drive\shell') {
        if (& $runs $place) {
            $classes.DeleteSubKeyTree("$place\$verb")
            $changed = $true
        }
    }
    # Win+E opening Glint: Windows' own key for it goes, so Win+E opens File Explorer again.
    $winE = 'CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}'
    $command = $classes.OpenSubKey("$winE\shell\opennewwindow\command")
    if ($command) {
        $ours = "$($command.GetValue(''))".StartsWith($exe, 'OrdinalIgnoreCase')
        $command.Close()
        if ($ours) { $classes.DeleteSubKeyTree($winE); $changed = $true }
    }
    if ($changed) {
        foreach ($parent in 'Directory\Background\shell', 'Directory\Background', 'Directory\shell', 'Directory', 'Drive\shell', 'Drive') {
            $key = $classes.OpenSubKey($parent)
            if ($key) {
                $empty = $key.SubKeyCount -eq 0 -and $key.ValueCount -eq 0
                $key.Close()
                if ($empty) { $classes.DeleteSubKey($parent) }
            }
        }
        # SHCNE_ASSOCCHANGED: File Explorer reads them again.
        Add-Type -Namespace GlintScoop -Name Shell -MemberDefinition '[DllImport("shell32.dll")] public static extern void SHChangeNotify(int eventId, uint flags, IntPtr item1, IntPtr item2);'
        [GlintScoop.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
    }
    $classes.Close()
    # Starting with Windows (Settings > Quick search), where it starts this copy.
    $run = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Run', $true)
    if ($run) {
        if ("$($run.GetValue($verb))".StartsWith($exe, 'OrdinalIgnoreCase')) { $run.DeleteValue($verb) }
        $run.Close()
    }
}`;
const scoop = {
    version,
    description: SHORT,
    homepage: REPO,
    license: "MIT",
    notes: [
        "Update with 'scoop update glint'; the app links to new versions but doesn't install them itself.",
        "Instant search, opening folders from File Explorer and starting with Windows, once turned on in Settings, are taken out by 'scoop uninstall' too.",
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
            ...removeExplorerVerbs.trim().split("\n"),
        ],
    },
    shortcuts: [["Glint.exe", "Glint"]],
    checkver: "github",
    autoupdate: {
        architecture: {
            "64bit": {
                url: `${REPO}/releases/download/v$version/Glint_$version_x64-setup.exe#/dl.7z`,
            },
        },
    },
};

const wingetDir = join(out, "winget", "manifests", "s", "shadijhade", "Glint", version);
const scoopDir = join(out, "scoop");
mkdirSync(wingetDir, { recursive: true });
mkdirSync(scoopDir, { recursive: true });
for (const [name, text] of Object.entries(winget)) writeFileSync(join(wingetDir, name), text);
writeFileSync(join(scoopDir, "glint.json"), `${JSON.stringify(scoop, null, 4)}\n`);
console.log(`${installerName} SHA-256 ${sha256}\nwinget: ${wingetDir}\nScoop:  ${join(scoopDir, "glint.json")}`);
