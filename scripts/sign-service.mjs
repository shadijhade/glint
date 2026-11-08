// Signs the instant search service, so an installed one takes it in place of itself without administrator
// approval (src-tauri/index/src/update.rs): `tauri signer sign` with the update key, the version sealed in
// the signature, then checked by the service executable itself, as an installed service checks an update.
// release.mjs runs it as Tauri's beforeBundleCommand, so shadui-index.exe.sig goes into the installer next
// to shadui-index.exe; msix.mjs runs it on the Store package's copy.
//
// Usage: node scripts/sign-service.mjs [path\to\shadui-index.exe] [--copy-to path\to\shadui-index.exe.sig]
//   (by default the build's, in the Cargo target folder: debug when Tauri builds a debug build)
//   --copy-to: the signature goes there too (where the installer takes it from)
// The key and its password: TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD (update-key.mjs).

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { signWithUpdateKey } from "./update-key.mjs";

const root = resolve(import.meta.dirname, "..");

/** Signs `exe` (a build of shadui-index.exe) into `exe`.sig and checks the signature with it; throws why not. */
export function signService(exe) {
    if (!existsSync(exe)) throw new Error(`${exe} wasn't built`);
    const version = JSON.parse(readFileSync(join(root, "src-tauri", "tauri.conf.json"), "utf8")).version;
    rmSync(`${exe}.sig`, { force: true });
    const signature = signWithUpdateKey(exe, version);
    // The key that signed it must be the one the service trusts (tauri.conf.json's updater key).
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
    const target = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(root, "src-tauri", "target");
    const exe = resolve(args[0] ?? join(target, process.env.TAURI_ENV_DEBUG === "true" ? "debug" : "release", "shadui-index.exe"));
    try {
        console.log(`✓ ${signService(exe)}`);
        if (copyTo) copyFileSync(`${exe}.sig`, copyTo);
    } catch (e) {
        console.error(`\n✗ Couldn't sign the instant search service: ${e.message}`);
        process.exit(1);
    }
}
