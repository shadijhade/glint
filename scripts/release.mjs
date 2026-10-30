// Builds a release on this machine (docs/releasing.md): the crates checked first (cargo-deny, and
// THIRD_PARTY_CRATES.md current), then the NSIS installer, code-signed when signing is set up, its update
// signature and latest.json for the app's updater, checked the way the app checks them, and the winget
// and Scoop manifests. Everything lands in release/v<version>/, ready to upload, with the installer also
// as Glint-setup.exe, the name the Download links ask the latest release for. With --store, also the
// offline installer (WebView2 inside), the one the Microsoft Store runs.
//
// Usage: npm run release [-- --sign-config <tauri config overlay>] [--store] [--allow-dirty]
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
const store = args.includes("--store");
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

// The crates: no known vulnerability and licenses that can ship (cargo-deny), and THIRD_PARTY_CRATES.md,
// which the installer carries, current (cargo-about). A check whose tool isn't installed is skipped.
const deny = run("cargo", ["deny", "--manifest-path", "src-tauri/Cargo.toml", "check"]);
if (/no such (command|subcommand)/.test(deny.stderr)) console.warn("! cargo-deny isn't installed (cargo install --locked cargo-deny), so the crates weren't checked.");
else if (deny.status) fail(`cargo deny found a problem with the crates (src-tauri/deny.toml):\n${deny.stderr.trim()}`);
const licenses = run(process.execPath, [join(root, "scripts", "licenses.mjs"), "--check"]);
if (licenses.status === 2) console.warn(`! ${licenses.stderr.trim()}`);
else if (licenses.status) fail(licenses.stderr.trim());

// The installer is Tauri's template with Glint's pages (src-tauri/installer/installer.nsi): taken from an
// older @tauri-apps/cli, it lacks what Tauri has fixed in its own since.
const template = readFileSync(join(root, "src-tauri", "installer", "installer.nsi"), "utf8").match(/^; Tauri template: (\S+)/m)?.[1];
const tauriCli = json("node_modules/@tauri-apps/cli/package.json").version;
if (template !== tauriCli) console.warn(`! The installer's template is Tauri ${template}'s, but @tauri-apps/cli is ${tauriCli}: compare the two (docs/releasing.md#the-installer).`);

const work = mkdtempSync(join(tmpdir(), "glint-release-"));
process.on("exit", () => rmSync(work, { recursive: true, force: true }));
/** A Tauri config overlay in the work folder, as the arguments that hand it to `tauri build`. */
const overlay = (name, value) => {
    const path = join(work, name);
    writeFileSync(path, JSON.stringify(value, null, 2));
    return ["--config", path];
};

// 2. Update signing: the public key in tauri.conf.json and the private key work only together.
const pubkey = config.plugins.updater.pubkey;
const hasKey = !!process.env.TAURI_SIGNING_PRIVATE_KEY;
if (hasKey && !pubkey) fail("TAURI_SIGNING_PRIVATE_KEY is set, but plugins.updater.pubkey in src-tauri/tauri.conf.json is empty: paste the matching public key there.");
if (pubkey && !hasKey) fail("Set TAURI_SIGNING_PRIVATE_KEY (the private key or its path): without it the app couldn't install this release.");
const updater = pubkey ? overlay("updater.json", { bundle: { createUpdaterArtifacts: true } }) : [];
if (!pubkey) console.warn("! No updater key is set up yet, so the app can't install this release by itself.");

// 3. Code signing: Azure Artifact Signing, your own overlay, or none.
const azure = ["AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_SIGNING_PROFILE"].map((name) => process.env[name]);
let signing = [];
if (signConfig) {
    signing = ["--config", resolve(signConfig)];
} else if (azure.some(Boolean)) {
    if (!azure.every(Boolean)) fail("Set all of AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and AZURE_SIGNING_PROFILE.");
    signing = overlay("codesign.json", { bundle: { windows: { signCommand: await artifactSigning(...azure) } } });
} else {
    console.warn("! Not code-signed, so SmartScreen warns about this installer.");
}
const codeSigned = !!signConfig || azure.every(Boolean);
if (store && !codeSigned) console.warn("! The Microsoft Store only takes a code-signed installer, so the offline installer is only for trying out.");

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
const nsis = join(root, "src-tauri", "target", "release", "bundle", "nsis");
const name = `${config.productName}_${version}_x64-setup.exe`;
const installer = join(nsis, name);

/** Builds the NSIS installer with these config overlays and, when it's code-signed, checks the signatures. */
function bundle(configs) {
    if (codeSigned) cpSync(tools, join(work, "binaries"), { recursive: true });
    const build = spawnSync(process.execPath, [cli, "build", "--bundles", "nsis", ...configs], { cwd: root, stdio: "inherit" });
    if (codeSigned) cpSync(join(work, "binaries"), tools, { recursive: true });
    if (build.error || build.status) fail("The build failed");
    if (!existsSync(installer)) fail(`${installer} wasn't built`);
    if (!codeSigned) return;
    const files = [installer, join(root, "src-tauri", "target", "release", `${config.mainBinaryName}.exe`), join(root, "src-tauri", "target", "release", "shadui-index.exe")];
    const check = join(work, "check-signatures.ps1");
    writeFileSync(check, "$bad = 0\nforeach ($f in $args) {\n  $s = Get-AuthenticodeSignature $f\n  \"$($s.Status)  $(Split-Path $f -Leaf)  $($s.SignerCertificate.Subject)\"\n  if ($s.Status -ne 'Valid') { $bad++ }\n}\nexit $bad\n");
    const result = run("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", check, ...files]);
    console.log(result.stdout.trim());
    if (result.status) fail("Not everything is validly code-signed");
}
bundle([...updater, ...signing]);

// 5. The update the app downloads, checked the way the app checks it, and the package manifests.
const out = join(root, "release", `v${version}`);
mkdirSync(out, { recursive: true });
copyFileSync(installer, join(out, name));
// The same installer without the version in its name. The Download links ask GitHub for the latest
// release's Glint-setup.exe (…/releases/latest/download/Glint-setup.exe), so every release needs one.
const latest = `${config.productName}-setup.exe`;
copyFileSync(installer, join(out, latest));
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

// 6. With --store, the offline installer: the same app with WebView2 inside, so setup downloads nothing,
// as the Microsoft Store requires of an installer it runs (docs/releasing.md#the-microsoft-store). It's
// built last because it takes the other installer's place in target/, and has no update signature: the
// Store's copies update with the normal installer, like every other copy.
const offline = `${config.productName}_${version}_x64_offline-setup.exe`;
if (store) {
    bundle([...signing, "--config", join(root, "src-tauri", "tauri.microsoftstore.conf.json")]);
    copyFileSync(installer, join(out, offline));
}

// The offline installer goes up only when it's signed: it's for the Store.
const files = [latest, name, ...(pubkey ? [`${name}.sig`, "latest.json"] : []), ...(store && codeSigned ? [offline] : [])];
const assets = files.map((f) => `release/v${version}/${f}`).join(" ");
// The notes `npm run notes` drafted, or GitHub's list of the changes.
const drafted = existsSync(join(out, "notes.md"));
const notes = drafted ? `--notes-file release/v${version}/notes.md` : "--generate-notes";
const storeStep = !store ? "" : codeSigned
    ? `\n  4. Point the Microsoft Store at the offline installer in Partner Center and submit it (docs/releasing.md#the-microsoft-store):\n       ${REPO}/releases/download/v${version}/${offline}`
    : `\n  The offline installer (release/v${version}/${offline}) isn't code-signed, so it's left out: the Store wouldn't take it.`;
console.log(`
✓ release/v${version} is ready. Next (docs/releasing.md):
  1. Create the draft release:
       gh release create v${version} ${assets} --draft --target ${commit} --title "${config.productName} v${version}" ${notes}
  2. Read it on GitHub, ${drafted ? "check the notes" : "write the notes (npm run notes drafts them)"}, try the installer, and publish it.
  3. Submit release/v${version}/package-manifests to winget and Scoop.${storeStep}`);
