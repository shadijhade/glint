// Builds a release on this machine (docs/releasing.md): the NSIS installer, code-signed when signing is
// set up, its update signature and latest.json for the app's updater, checked the way the app checks
// them, and the winget and Scoop manifests. Everything lands in release/v<version>/, ready to upload.
//
// Usage: npm run release [-- --sign-config <tauri config overlay>] [--allow-dirty]
//
// Update signing: TAURI_SIGNING_PRIVATE_KEY (the key, or the path to it) and, if the key has one,
// TAURI_SIGNING_PRIVATE_KEY_PASSWORD (otherwise Tauri asks for it).
// Code signing with Azure Artifact Signing: AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and
// AZURE_SIGNING_PROFILE, signed in with `az login` or AZURE_CLIENT_ID, AZURE_CLIENT_SECRET and
// AZURE_TENANT_ID. Any other signing (a certificate in the Windows store, another tool): --sign-config.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const REPO = "https://github.com/shadijhade/glint";
const root = resolve(import.meta.dirname, "..");
const args = process.argv.slice(2);
const option = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
const signConfig = option("--sign-config");
const allowDirty = args.includes("--allow-dirty");

const fail = (message) => {
    console.error(`\n✗ ${message}`);
    process.exit(1);
};
const run = (cmd, cmdArgs, options = {}) => {
    const result = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8", ...options });
    if (result.error) fail(`${cmd}: ${result.error.message}`);
    return result;
};
const json = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));

// 1. One version everywhere, from a committed tree.
const config = json("src-tauri/tauri.conf.json");
const version = config.version;
const cargo = JSON.parse(run("cargo", ["metadata", "--no-deps", "--format-version", "1", "--manifest-path", "src-tauri/Cargo.toml"]).stdout);
const versions = { "package.json": json("package.json").version, "src-tauri/tauri.conf.json": version };
for (const p of cargo.packages) versions[`src-tauri/Cargo.toml (${p.name})`] = p.version;
const wrong = Object.entries(versions).filter(([, v]) => v !== version);
if (wrong.length) fail(`The versions differ:\n${Object.entries(versions).map(([file, v]) => `  ${file}: ${v}`).join("\n")}`);
const changes = run("git", ["status", "--porcelain", "--untracked-files=no"]).stdout.trimEnd();
if (changes && !allowDirty) {
    const tools = changes.includes("src-tauri/binaries/") ? "\n(A signed build that was cut short leaves rg, fd and pdu signed: git checkout -- src-tauri/binaries)" : "";
    fail(`Commit your changes first, so the release matches a commit:\n${changes}${tools}`);
}
const commit = run("git", ["rev-parse", "HEAD"]).stdout.trim();
const pushed = run("git", ["branch", "-r", "--contains", commit]).stdout.includes("origin/master");
console.log(`Releasing ${version} from ${commit.slice(0, 7)}${pushed ? "" : " (not on origin/master yet: push it before publishing)"}`);

const work = mkdtempSync(join(tmpdir(), "glint-release-"));
process.on("exit", () => rmSync(work, { recursive: true, force: true }));
const overlays = [];
const overlay = (name, value) => {
    const path = join(work, name);
    writeFileSync(path, JSON.stringify(value, null, 2));
    overlays.push("--config", path);
};

// 2. Update signing: the public key in tauri.conf.json and the private key work only together.
const pubkey = config.plugins.updater.pubkey;
const hasKey = !!process.env.TAURI_SIGNING_PRIVATE_KEY;
if (hasKey && !pubkey) fail("TAURI_SIGNING_PRIVATE_KEY is set, but plugins.updater.pubkey in src-tauri/tauri.conf.json is empty: paste the matching public key there.");
if (pubkey && !hasKey) fail("Set TAURI_SIGNING_PRIVATE_KEY (the private key or its path): without it the app couldn't install this release.");
if (pubkey) overlay("updater.json", { bundle: { createUpdaterArtifacts: true } });
else console.warn("! No updater key is set up yet, so the app can't install this release by itself.");

// 3. Code signing: Azure Artifact Signing, your own overlay, or none.
const azure = ["AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_SIGNING_PROFILE"].map((name) => process.env[name]);
if (signConfig) {
    overlays.push("--config", resolve(signConfig));
} else if (azure.some(Boolean)) {
    if (!azure.every(Boolean)) fail("Set all of AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and AZURE_SIGNING_PROFILE.");
    overlay("codesign.json", { bundle: { windows: { signCommand: await artifactSigning(...azure) } } });
} else {
    console.warn("! Not code-signed, so SmartScreen warns about this installer.");
}
const codeSigned = !!signConfig || azure.every(Boolean);

/** signtool with Microsoft's Artifact Signing plugin (fetched once into release/.tools, checked by its SHA-256). */
async function artifactSigning(endpoint, account, profile) {
    const client = join(root, "release", ".tools", "artifact-signing-1.0.128");
    if (!existsSync(join(client, "bin", "x64", "Azure.CodeSigning.Dlib.dll"))) {
        const response = await fetch("https://www.nuget.org/api/v2/package/Microsoft.ArtifactSigning.Client/1.0.128").catch((e) => fail(String(e)));
        if (!response.ok) fail(`Couldn't download the Artifact Signing client: ${response.status} ${response.statusText}`);
        const data = Buffer.from(await response.arrayBuffer());
        if (createHash("sha256").update(data).digest("hex") !== "74bd7d27e6ce1051409c38d9b46bc8df0400ecd643d51ffbf2ac00869061e40b") {
            fail("Microsoft.ArtifactSigning.Client 1.0.128 doesn't match the SHA-256 recorded in scripts/release.mjs");
        }
        mkdirSync(client, { recursive: true });
        writeFileSync(join(client, "package.zip"), data);
        // Windows' own tar reads zip files (Git Bash's doesn't).
        const tar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
        if (run(tar, ["-xf", "package.zip"], { cwd: client }).status) fail("Couldn't unpack the Artifact Signing client");
    }
    const metadata = join(work, "metadata.json");
    // Signs in with the AZURE_CLIENT_* service principal if those are set, else with `az login`.
    const others = ["ManagedIdentityCredential", "WorkloadIdentityCredential", "SharedTokenCacheCredential", "VisualStudioCredential",
        "VisualStudioCodeCredential", "AzurePowerShellCredential", "AzureDeveloperCliCredential", "InteractiveBrowserCredential"];
    writeFileSync(metadata, JSON.stringify({ Endpoint: endpoint, CodeSigningAccountName: account, CertificateProfileName: profile, ExcludeCredentials: others }));
    const kits = join(process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Windows Kits", "10", "bin");
    const sdk = existsSync(kits) ? readdirSync(kits).filter((d) => /^10\.\d+\.\d+\.\d+$/.test(d) && existsSync(join(kits, d, "x64", "signtool.exe"))) : [];
    const newest = sdk.sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).pop();
    if (!newest) fail("signtool.exe wasn't found: install the Windows SDK (10.0.26100 or newer).");
    return {
        cmd: join(kits, newest, "x64", "signtool.exe"),
        args: ["sign", "/fd", "SHA256", "/tr", "http://timestamp.acs.microsoft.com", "/td", "SHA256", "/d", config.productName,
            "/dlib", join(client, "bin", "x64", "Azure.CodeSigning.Dlib.dll"), "/dmdf", metadata, "%1"],
    };
}

// 4. Build. Tauri signs every bundled executable, then the installer, then makes the update signature.
// It signs rg, fd and pdu where they are, in src-tauri/binaries, which keeps the official files: the
// signed copies only go into the installer.
const cli = join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");
const tools = join(root, "src-tauri", "binaries");
if (codeSigned) cpSync(tools, join(work, "binaries"), { recursive: true });
const build = spawnSync(process.execPath, [cli, "build", "--bundles", "nsis", ...overlays], { cwd: root, stdio: "inherit" });
if (codeSigned) cpSync(join(work, "binaries"), tools, { recursive: true });
if (build.error || build.status) fail("The build failed");

const nsis = join(root, "src-tauri", "target", "release", "bundle", "nsis");
const name = `${config.productName}_${version}_x64-setup.exe`;
const installer = join(nsis, name);
if (!existsSync(installer)) fail(`${installer} wasn't built`);

if (codeSigned) {
    const files = [installer, join(root, "src-tauri", "target", "release", `${config.mainBinaryName}.exe`), join(root, "src-tauri", "target", "release", "shadui-index.exe")];
    const check = join(work, "check-signatures.ps1");
    writeFileSync(check, "$bad = 0\nforeach ($f in $args) {\n  $s = Get-AuthenticodeSignature $f\n  \"$($s.Status)  $(Split-Path $f -Leaf)  $($s.SignerCertificate.Subject)\"\n  if ($s.Status -ne 'Valid') { $bad++ }\n}\nexit $bad\n");
    const result = run("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", check, ...files]);
    console.log(result.stdout.trim());
    if (result.status) fail("Not everything is validly code-signed");
}

// 5. The update the app downloads, checked the way the app checks it, and the package manifests.
const out = join(root, "release", `v${version}`);
mkdirSync(out, { recursive: true });
copyFileSync(installer, join(out, name));
if (pubkey) {
    copyFileSync(`${installer}.sig`, join(out, `${name}.sig`));
    const manifest = {
        version,
        notes: `${REPO}/releases/tag/v${version}`,
        pub_date: new Date().toISOString(),
        platforms: { "windows-x86_64": { signature: readFileSync(`${installer}.sig`, "utf8").trim(), url: `${REPO}/releases/download/v${version}/${name}` } },
    };
    writeFileSync(join(out, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
    if (run(process.execPath, [join(root, "scripts", "verify-update.mjs"), join(out, "latest.json"), join(out, name)], { stdio: "inherit" }).status) {
        fail("The update doesn't verify with the app's public key: is TAURI_SIGNING_PRIVATE_KEY the key that goes with it?");
    }
}
const manifests = [join(root, "scripts", "package-manifests.mjs"), version, join(out, name), "--out", join(out, "package-manifests")];
if (run(process.execPath, manifests, { stdio: "inherit" }).status) fail("Couldn't write the winget and Scoop manifests");

const assets = [name, ...(pubkey ? [`${name}.sig`, "latest.json"] : [])].map((f) => `release/v${version}/${f}`).join(" ");
console.log(`
✓ release/v${version} is ready. Next (docs/releasing.md):
  1. Create the draft release:
       gh release create v${version} ${assets} --draft --target ${commit} --title "${config.productName} v${version}" --generate-notes
  2. Read it on GitHub, write the notes, try the installer, and publish it.
  3. Submit release/v${version}/package-manifests to winget and Scoop.`);
