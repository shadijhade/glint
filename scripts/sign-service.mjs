// Signs the instant search service, so an installed one takes it in place of itself without administrator
// approval (crates/shadui-index/src/update.rs): the update key's minisign signature of the exe, the version
// sealed in the signature, then checked by the service executable itself, as an installed service checks an
// update. release.mjs signs the service it packs in the installer; msix.mjs the Store package's copy.
//
// Usage: node scripts/sign-service.mjs [path\to\shadui-index.exe] [--copy-to path\to\shadui-index.exe.sig]
//   (by default the build's, in the Cargo target folder: target\ui\release, or debug with --debug)
//   --copy-to: the signature goes there too (where the installer takes it from)
// The key and its password: TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD (update-key.mjs).
// With GLINT_UPDATE_PUBKEY set (a dry run with a throwaway key) the service, which trusts the committed key only,
// can't check the signature itself: it is checked here with the key that made it, and the result says so.

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { checkSignature } from "./lib/minisign.mjs";
import { targetDir, updatePubkey, usesCommittedPubkey, workspaceVersion } from "./lib/project.mjs";
import { signWithUpdateKey } from "./update-key.mjs";

/** Signs `exe` (a build of shadui-index.exe) into `exe`.sig and checks the signature; returns what the check said, throws why not. */
export async function signService(exe) {
    if (!existsSync(exe)) throw new Error(`${exe} wasn't built`);
    const version = workspaceVersion();
    rmSync(`${exe}.sig`, { force: true });
    const signature = await signWithUpdateKey(exe, version);
    if (!usesCommittedPubkey()) {
        // The service trusts the committed key, so only the signature itself can be checked, with the key given.
        const result = checkSignature(readFileSync(exe), readFileSync(signature, "utf8"), updatePubkey());
        if (result.problems.length || result.comment.version !== version || result.comment.file !== "shadui-index.exe") {
            rmSync(signature, { force: true });
            throw new Error(`The service's signature doesn't hold: ${result.problems.join("; ") || result.trusted}`);
        }
        return `${exe} is signed with GLINT_UPDATE_PUBKEY's key for ${version}. (The service itself trusts crates/shadui-index/update-pubkey.txt, so no installed service takes this one: a dry run.)`;
    }
    // The key that signed it must be the one the service trusts (crates/shadui-index/update-pubkey.txt).
    const check = spawnSync(exe, ["verify-update", exe, signature], { encoding: "utf8" });
    if (check.error || check.status !== 0) {
        rmSync(signature, { force: true });
        throw new Error(`The service doesn't take its signature: ${(check.stderr || check.stdout || check.error?.message || "").trim()}`);
    }
    return check.stdout.trim();
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
    const args = process.argv.slice(2);
    const copyTo = args.includes("--copy-to") ? resolve(args.splice(args.indexOf("--copy-to"), 2)[1]) : undefined;
    const debug = args.includes("--debug");
    const exe = resolve(args.find((a) => !a.startsWith("--")) ?? join(targetDir(), debug ? "debug" : "release", "shadui-index.exe"));
    try {
        console.log(`✓ ${await signService(exe)}`);
        if (copyTo) copyFileSync(`${exe}.sig`, copyTo);
    } catch (e) {
        console.error(`\n✗ Couldn't sign the instant search service: ${e.message}`);
        process.exit(1);
    }
}
