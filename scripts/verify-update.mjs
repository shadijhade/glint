// Checks a release's update the way the app does before installing it: latest.json points at this
// installer, and the installer's signature verifies with the app's public key and is bound to the
// version latest.json announces (the app sets requireSignedVersion, so anything else is refused).
//
// Usage: node scripts/verify-update.mjs <latest.json> <installer-setup.exe> [public key]
// The public key defaults to plugins.updater.pubkey in src-tauri/tauri.conf.json, the one the app is
// built with. `npm run release` runs this on every release, before anything is uploaded.

import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";
import { basename } from "node:path";

const [manifestPath, installerPath, pubkeyArg] = process.argv.slice(2);
if (!manifestPath || !installerPath) {
    console.error("Usage: node scripts/verify-update.mjs <latest.json> <installer-setup.exe> [public key]");
    process.exit(2);
}

const config = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const pubkey = pubkeyArg ?? config.plugins.updater.pubkey;
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const installer = readFileSync(installerPath);
const name = basename(installerPath);

const problems = [];
const check = (ok, problem) => ok || problems.push(problem);

/** Tauri stores minisign keys and signatures base64-encoded, comment lines included. */
const minisign = (text) => Buffer.from(String(text).trim(), "base64").toString("utf8").split(/\r?\n/);
/** Ed25519 verification that answers false for malformed input instead of throwing. */
function ed25519(key, message, signature) {
    try {
        const der = Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), key]);
        return verify(null, message, createPublicKey({ key: der, format: "der", type: "spki" }), signature);
    } catch {
        return false;
    }
}

const platforms = Object.entries(manifest.platforms ?? {}).filter(([target]) => target.startsWith("windows-x86_64"));
check(/^\d+\.\d+\.\d+/.test(String(manifest.version)), `latest.json has no valid version (${manifest.version})`);
check(platforms.length > 0, "latest.json has no windows-x86_64 entry");

// A public key is "Ed" | key id (8 bytes) | Ed25519 key (32 bytes).
const key = Buffer.from(minisign(pubkey ?? "")[1] ?? "", "base64");
check(key.length === 42, "no usable public key (plugins.updater.pubkey in src-tauri/tauri.conf.json)");

for (const [target, { url, signature }] of key.length === 42 ? platforms : []) {
    check(String(url).endsWith(`/releases/download/v${manifest.version}/${name}`), `${target}: ${url} isn't ${name} from release v${manifest.version}`);
    // Lines: untrusted comment, "ED" (BLAKE2b-prehashed) | key id | signature, trusted comment, its signature.
    const [, sigLine = "", trustedLine = "", globalLine = ""] = minisign(signature ?? "");
    const sig = Buffer.from(sigLine, "base64");
    const trusted = trustedLine.replace(/^trusted comment: /, "");
    check(sig.length === 74 && sig.subarray(2, 10).equals(key.subarray(2, 10)), `${target}: not signed with the app's key`);
    const prehashed = sig.subarray(0, 2).toString() === "ED";
    check(ed25519(key.subarray(10), prehashed ? createHash("blake2b512").update(installer).digest() : installer, sig.subarray(10)), `${target}: the signature doesn't match the installer`);
    check(ed25519(key.subarray(10), Buffer.concat([sig.subarray(10), Buffer.from(trusted)]), Buffer.from(globalLine, "base64")), `${target}: the signature's trusted comment doesn't verify`);
    check(trusted.split("\t").includes(`version:${manifest.version}`), `${target}: the signature isn't bound to version ${manifest.version} (trusted comment: ${trusted})`);
}

if (problems.length) {
    for (const problem of problems) console.error(`✗ ${problem}`);
    process.exit(1);
}
console.log(`✓ ${name} is version ${manifest.version}, signed with the app's key (${platforms.map(([target]) => target).join(", ")})`);
