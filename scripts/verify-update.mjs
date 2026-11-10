// Checks a release's update the way the app does before installing it: latest.json points at this
// installer, and the installer's signature verifies with the app's public key and is bound to the
// version latest.json announces (the app requires a signed version, so anything else is refused).
//
// Usage: node scripts/verify-update.mjs <latest.json> <installer-setup.exe> [public key]
// The public key (the minisign box, base64-encoded, or the path to a file holding it) defaults to the one the
// releases are signed with, crates/shadui-index/update-pubkey.txt (GLINT_UPDATE_PUBKEY names another, for a
// dry run with a throwaway key). `npm run release` runs this on every release, before anything is uploaded.

import { existsSync, readFileSync } from "node:fs";
import { basename } from "node:path";
import { checkSignature, parsePublicKey } from "./lib/minisign.mjs";
import { updatePubkey } from "./lib/project.mjs";

const [manifestPath, installerPath, pubkeyArg] = process.argv.slice(2);
if (!manifestPath || !installerPath) {
    console.error("Usage: node scripts/verify-update.mjs <latest.json> <installer-setup.exe> [public key]");
    process.exit(2);
}

const pubkey = pubkeyArg ? (existsSync(pubkeyArg) ? readFileSync(pubkeyArg, "utf8").trim() : pubkeyArg) : updatePubkey();
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const installer = readFileSync(installerPath);
const name = basename(installerPath);

const problems = [];
const check = (ok, problem) => ok || problems.push(problem);

const platforms = Object.entries(manifest.platforms ?? {}).filter(([target]) => target.startsWith("windows-x86_64"));
check(/^\d+\.\d+\.\d+/.test(String(manifest.version)), `latest.json has no valid version (${manifest.version})`);
check(platforms.length > 0, "latest.json has no windows-x86_64 entry");
check(!!parsePublicKey(pubkey), "no usable public key (crates/shadui-index/update-pubkey.txt)");

for (const [target, { url, signature }] of parsePublicKey(pubkey) ? platforms : []) {
    check(String(url).endsWith(`/releases/download/v${manifest.version}/${name}`), `${target}: ${url} isn't ${name} from release v${manifest.version}`);
    const { problems: bad, trusted } = checkSignature(installer, signature, pubkey);
    for (const problem of bad) check(false, `${target}: ${problem.replace("the file", "the installer").replace("this key", "the app's key")}`);
    check(trusted.split("\t").includes(`version:${manifest.version}`), `${target}: the signature isn't bound to version ${manifest.version} (trusted comment: ${trusted})`);
}

if (problems.length) {
    for (const problem of problems) console.error(`✗ ${problem}`);
    process.exit(1);
}
console.log(`✓ ${name} is version ${manifest.version}, signed with the app's key (${platforms.map(([target]) => target).join(", ")})`);
