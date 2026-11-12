// Builds a release on this machine (docs/releasing.md): the version checked everywhere, the crates checked
// (cargo-deny, and THIRD_PARTY_CRATES.md current), Glint.exe and the instant search service built, signed
// where signing is set up, packed into the NSIS installer by makensis (scripts/gpui-installer.mjs), the
// installer's update signature and latest.json for the app's updater checked the way the app checks them,
// and the winget and Scoop manifests. Everything lands in release/v<version>/, ready to upload, with the
// installer also as Glint-setup.exe, the name the Download links ask the latest release for.
//
// Usage: npm run release [-- --allow-dirty] [--out <folder>] [--sign-command "<command with %1>"]
//                         [--pubkey <file>] [--installer <setup.exe>] [--no-build]
//   --allow-dirty    release from a tree with uncommitted changes (a trial; a real release matches a commit)
//   --out <folder>   where the release goes (default release/v<version>)
//   --sign-command   code signing: a command line with %1 where a file's path goes (docs/releasing.md); the
//                    Azure variables below are the other way, and with neither nothing is code-signed
//   --pubkey <file>  a trial with a throwaway key: this public key (the .pub that `tauri signer generate`
//                    writes) in place of crates/shadui-index/update-pubkey.txt. The result is signed with a key
//                    no installed Glint trusts; verify-update.mjs checks it with this key. (GLINT_UPDATE_PUBKEY)
//   --installer      sign and lay out this setup program in place of building one: the release workflow, after
//                    SignPath has signed the installer. The checks and the build are then the workflow's.
//   --no-build       use the Glint.exe and shadui-index.exe already in <target>/release (a build you made
//                    yourself, such as one with other cargo options) instead of running cargo
//
// Update signing: TAURI_SIGNING_PRIVATE_KEY (the key, or the path to it) and, if the key has one,
// TAURI_SIGNING_PRIVATE_KEY_PASSWORD (otherwise this asks for it once, before building). The same key signs
// the instant search service, so an installed one updates itself to the release (scripts/sign-service.mjs).
// Code signing with Azure Artifact Signing: AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and
// AZURE_SIGNING_PROFILE, signed in with `az login` or AZURE_CLIENT_ID, AZURE_CLIENT_SECRET and
// AZURE_TENANT_ID. Any other signing (a certificate in the Windows store, another tool): --sign-command.

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join, relative, resolve } from "node:path";
import { signCommandFor, signFileWith } from "./lib/codesign.mjs";
import { parsePublicKey } from "./lib/minisign.mjs";
import { COMMITTED_PUBKEY_FILE, packageVersion, REPO, root, targetDir, updatePubkey, usesCommittedPubkey, workspaceVersion } from "./lib/project.mjs";
import { signService } from "./sign-service.mjs";
import { hasUpdateKey, signWithUpdateKey, unlockUpdateKey } from "./update-key.mjs";

const args = process.argv.slice(2);
const option = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
const allowDirty = args.includes("--allow-dirty");
const prebuilt = option("--installer") ? resolve(option("--installer")) : undefined;

const fail = (message) => {
    console.error(`\n✗ ${message}`);
    process.exit(1);
};
const run = (cmd, cmdArgs, options = {}) => {
    const result = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8", ...options });
    if (result.error) fail(`${cmd}: ${result.error.message}`);
    return result;
};
if (args.includes("--store")) {
    console.warn("! --store is gone: the installer no longer carries WebView2, so it downloads nothing and the Store's EXE route takes the normal Glint_<version>_x64-setup.exe (docs/releasing.md#the-microsoft-store).");
}
if (option("--pubkey")) process.env.GLINT_UPDATE_PUBKEY = resolve(option("--pubkey"));

// 1. One version everywhere, from a committed tree.
const version = workspaceVersion();
const cargo = JSON.parse(run("cargo", ["metadata", "--no-deps", "--format-version", "1"], { maxBuffer: 1 << 28 }).stdout || "{}");
const versions = { "Cargo.toml ([workspace.package])": version, "package.json": packageVersion() };
for (const p of cargo.packages ?? []) versions[`${p.name} (Cargo.toml)`] = p.version;
const wrong = Object.entries(versions).filter(([, v]) => v !== version);
if (wrong.length || !cargo.packages?.length) {
    fail(`The versions differ:\n${Object.entries(versions).map(([file, v]) => `  ${file}: ${v}`).join("\n")}`);
}
const changes = run("git", ["status", "--porcelain", "--untracked-files=no"]).stdout.trimEnd();
if (changes && !allowDirty) fail(`Commit your changes first, so the release matches a commit (or --allow-dirty for a trial):\n${changes}`);
const commit = run("git", ["rev-parse", "HEAD"]).stdout.trim();
const pushed = run("git", ["branch", "-r", "--contains", commit]).stdout.includes("origin/master");
console.log(`Releasing ${version} from ${commit.slice(0, 7)}${changes ? " (with uncommitted changes)" : ""}${pushed ? "" : " (not on origin/master yet: push it before publishing)"}`);

// The crates: no known vulnerability and licenses that can ship (cargo-deny, deny.toml), and
// THIRD_PARTY_CRATES.md, which the installer carries, current (cargo-about). A check whose tool isn't
// installed is skipped, and says what that leaves unchecked. A build from the release workflow runs them
// in the workflow's own steps.
if (!prebuilt) {
    const deny = run("cargo", ["deny", "check"]);
    if (/no such (command|subcommand)/.test(deny.stderr)) {
        console.warn("! cargo-deny isn't installed (cargo install --locked cargo-deny), so SKIPPED: the crates' known vulnerabilities (RustSec) and where they come from.");
        // The licenses can still be checked against deny.toml's allow list, from the crates' Cargo.toml fields.
        const allowed = run(process.execPath, [join(root, "scripts", "licenses.mjs"), "--allowed"]);
        if (allowed.status) fail(allowed.stderr.trim());
        console.log(`  ${allowed.stdout.trim()} (licenses.mjs --allowed: the license fields only)`);
    } else if (deny.status) fail(`cargo deny found a problem with the crates (deny.toml):\n${deny.stderr.trim()}`);
    const licenses = run(process.execPath, [join(root, "scripts", "licenses.mjs"), "--check"]);
    if (licenses.status === 2) console.warn(`! SKIPPED: ${licenses.stderr.trim()}`);
    else if (licenses.status) fail(licenses.stderr.trim());
}

// 2. Update signing: the public key committed with the service (crates/shadui-index/update-pubkey.txt, also
// compiled into the app's updater) and the private key work only together.
const pubkey = updatePubkey();
if (!parsePublicKey(pubkey)) fail("The update key's public half isn't usable (crates/shadui-index/update-pubkey.txt, or GLINT_UPDATE_PUBKEY).");
// The app's updater has its own copy of the key (crates/glint-ui/src/updater.rs PUBLIC_KEY): the two must be
// the same text, or the app would refuse what the service accepts, or the other way round.
{
    const copy = readFileSync(join(root, "crates", "glint-ui", "src", "updater.rs"), "utf8").match(/pub const PUBLIC_KEY: &str = "([^"]+)"/)?.[1];
    const committed = readFileSync(COMMITTED_PUBKEY_FILE, "utf8").trim();
    if (copy !== committed) fail("PUBLIC_KEY in crates/glint-ui/src/updater.rs isn't the key in crates/shadui-index/update-pubkey.txt: the app and the service must trust the same one.");
}
if (!hasUpdateKey()) fail("Set TAURI_SIGNING_PRIVATE_KEY (the private key or its path): without it the app couldn't install this release.");
// The password, asked once and checked now rather than after the build.
try {
    unlockUpdateKey({ expectedPublicKey: pubkey });
} catch (e) {
    fail(`The update key can't sign: ${e.message}`);
}
if (!usesCommittedPubkey()) {
    console.warn("! A trial: signed with the key of GLINT_UPDATE_PUBKEY, not the committed one. No installed Glint or instant search service would take this update.");
}

// 3. Code signing: Azure Artifact Signing, your own command, or none.
const work = mkdtempSync(join(tmpdir(), "glint-release-"));
process.on("exit", () => rmSync(work, { recursive: true, force: true }));
let signCommand;
try {
    signCommand = await signCommandFor({ signCommand: option("--sign-command"), work });
} catch (e) {
    fail(e.message);
}
if (!signCommand && !prebuilt) console.warn("! Not code-signed, so SmartScreen warns about this installer.");

// 4. Build: Glint.exe and the instant search service, the service's signatures, then the installer.
let installer = prebuilt;
if (prebuilt) {
    if (!existsSync(prebuilt)) fail(`${prebuilt} doesn't exist.`);
} else {
    const target = targetDir();
    if (args.includes("--no-build")) {
        for (const exe of ["Glint.exe", "shadui-index.exe"]) if (!existsSync(join(target, "release", exe))) fail(`${join(target, "release", exe)} isn't built (--no-build)`);
        console.log(`\nUsing the build in ${join(target, "release")} (--no-build).`);
    } else {
        console.log("\nBuilding Glint and the instant search service (cargo build --release -p glint -p shadui-index)…");
        const build = run("cargo", ["build", "--release", "-p", "glint", "-p", "shadui-index"], { stdio: "inherit", env: { ...process.env, CARGO_TARGET_DIR: target } });
        if (build.status) fail("The build failed");
    }

    // The service's update signature is made over the very bytes that get packed: a copy of the service, code-signed
    // first when signing is set up, then signed with the update key and checked by the service itself.
    const serviceDir = join(work, "service");
    mkdirSync(serviceDir, { recursive: true });
    const service = join(serviceDir, "shadui-index.exe");
    copyFileSync(join(target, "release", "shadui-index.exe"), service);
    try {
        if (signCommand) signFileWith(signCommand, service);
        console.log(`✓ ${await signService(service)}`);
    } catch (e) {
        fail(`Couldn't sign the instant search service: ${e.message}`);
    }

    installer = join(work, "Glint-setup.exe");
    const pack = run(process.execPath, [join(root, "scripts", "gpui-installer.mjs"), "--no-build", "--service", service, "--service-sig", `${service}.sig`, "--service-signed", "--out", installer], {
        stdio: "inherit",
        env: { ...process.env, CARGO_TARGET_DIR: target, GLINT_SIGN_COMMAND: signCommand ?? "" },
    });
    if (pack.status || !existsSync(installer)) fail("The installer wasn't built");
}

// 5. The update the app downloads, checked the way the app checks it, and the package manifests.
const out = resolve(option("--out") ?? join(root, "release", `v${version}`));
mkdirSync(out, { recursive: true });
const name = `Glint_${version}_x64-setup.exe`;
copyFileSync(installer, join(out, name));
// The same installer without the version in its name. The Download links ask GitHub for the latest
// release's Glint-setup.exe (…/releases/latest/download/Glint-setup.exe), so every release needs one.
const latest = "Glint-setup.exe";
copyFileSync(installer, join(out, latest));
const signature = await signWithUpdateKey(join(out, name), version);
const manifest = {
    version,
    notes: `${REPO}/releases/tag/v${version}`,
    pub_date: new Date().toISOString(),
    platforms: { "windows-x86_64": { signature: readFileSync(signature, "utf8").trim(), url: `${REPO}/releases/download/v${version}/${name}` } },
};
writeFileSync(join(out, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
if (run(process.execPath, [join(root, "scripts", "verify-update.mjs"), join(out, "latest.json"), join(out, name)], { stdio: "inherit" }).status) {
    fail("The update doesn't verify with the app's public key: is TAURI_SIGNING_PRIVATE_KEY the key that goes with crates/shadui-index/update-pubkey.txt?");
}
const manifests = [join(root, "scripts", "package-manifests.mjs"), version, join(out, name), "--out", join(out, "package-manifests")];
if (run(process.execPath, manifests, { stdio: "inherit" }).status) fail("Couldn't write the winget and Scoop manifests");

const shown = relative(root, out).replaceAll("\\", "/");
const where = isAbsolute(shown) || shown.startsWith("..") ? out.replaceAll("\\", "/") : shown;
const files = [latest, name, `${name}.sig`, "latest.json"];
const assets = files.map((f) => `${where}/${f}`).join(" ");
// The notes `npm run notes` drafted, or GitHub's list of the changes.
const drafted = existsSync(join(out, "notes.md"));
const notes = drafted ? `--notes-file ${where}/notes.md` : "--generate-notes";
console.log(`
✓ ${where} is ready. Next (docs/releasing.md):
  1. Create the draft release:
       gh release create v${version} ${assets} --draft --target ${commit} --title "Glint v${version}" ${notes}
  2. Read it on GitHub, ${drafted ? "check the notes" : "write the notes (npm run notes drafts them)"}, try the installer, and publish it.
  3. Submit ${where}/package-manifests to winget and Scoop.
  4. The Microsoft Store, when it should have this version: npm run msix, then npm run store (or upload the package in Partner Center).`);
