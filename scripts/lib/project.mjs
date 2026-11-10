// What every release script needs to agree on, in one place: where the repository is, the version (the root
// Cargo.toml's [workspace.package] version, the one Glint.exe, the instant search service and the updater
// compare), the update key's public half and the Cargo target folder.

import { existsSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";

/** The repository's root folder. */
export const root = resolve(import.meta.dirname, "..", "..");

/** Where releases are published. */
export const REPO = "https://github.com/shadijhade/glint";

/** The version in the root Cargo.toml's [workspace.package], or throws. */
export function workspaceVersion() {
    const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
    const version = manifest.match(/\[workspace\.package\][^[]*?\nversion\s*=\s*"(\d+\.\d+\.\d+)"/)?.[1];
    if (!version) throw new Error("Couldn't read [workspace.package] version (x.y.z) from Cargo.toml.");
    return version;
}

/** package.json's version (kept equal to the workspace's: release.mjs checks). */
export const packageVersion = () => JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;

/** Where Cargo builds go: CARGO_TARGET_DIR, else target/ui (the folder this repository's builds use). */
export const targetDir = () => resolve(process.env.CARGO_TARGET_DIR ?? join(root, "target", "ui"));

/** The committed public key the releases are signed with: the one the instant search service is built to trust. */
export const COMMITTED_PUBKEY_FILE = join(root, "crates", "shadui-index", "update-pubkey.txt");

/**
 * The update key's public half, as the minisign box base64-encoded (what tauri.conf.json's `plugins.updater.pubkey`
 * held). The committed one, unless GLINT_UPDATE_PUBKEY names another (a file, or the key itself): that is for dry
 * runs with a throwaway key, and what it signs is not an update any installed Glint accepts.
 */
export function updatePubkey() {
    const given = process.env.GLINT_UPDATE_PUBKEY?.trim();
    if (given) return (existsSync(given) ? readFileSync(given, "utf8") : given).trim();
    return readFileSync(COMMITTED_PUBKEY_FILE, "utf8").trim();
}

/** Whether the public key in use is the committed one. */
export const usesCommittedPubkey = () => !process.env.GLINT_UPDATE_PUBKEY?.trim();
