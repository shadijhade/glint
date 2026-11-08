// The update key, for the scripts that sign with it (release.mjs, msix.mjs, sign-service.mjs): the key in
// TAURI_SIGNING_PRIVATE_KEY (the key itself, or the path to it) and its password in
// TAURI_SIGNING_PRIVATE_KEY_PASSWORD. `tauri signer sign` doesn't ask for a password that isn't set (it
// takes it as empty, and fails), so unlockUpdateKey() asks once and checks the two go together, before
// anything is built with them.

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const cli = join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");

/** Whether the update key is set. */
export const hasUpdateKey = () => !!(process.env.TAURI_SIGNING_PRIVATE_KEY || process.env.TAURI_SIGNING_PRIVATE_KEY_PATH);

/** The environment `tauri signer sign` reads the key from: a path in TAURI_SIGNING_PRIVATE_KEY goes in TAURI_SIGNING_PRIVATE_KEY_PATH. */
function signerEnv() {
    const env = { ...process.env };
    if (env.TAURI_SIGNING_PRIVATE_KEY && existsSync(env.TAURI_SIGNING_PRIVATE_KEY)) {
        env.TAURI_SIGNING_PRIVATE_KEY_PATH = env.TAURI_SIGNING_PRIVATE_KEY;
        delete env.TAURI_SIGNING_PRIVATE_KEY;
    }
    return env;
}

/** Signs `file` with the update key into `file`.sig, with `version` sealed in the signature when given. Throws with Tauri's words when it can't. */
export function signWithUpdateKey(file, version) {
    const args = [cli, "signer", "sign", ...(version ? ["--app-version", version] : []), file];
    const result = spawnSync(process.execPath, args, { cwd: root, env: signerEnv(), encoding: "utf8" });
    if (result.error || result.status !== 0 || !existsSync(`${file}.sig`)) {
        const said = `${result.stdout ?? ""}\n${result.stderr ?? ""}`.split("\n").filter((line) => /error|wrong|incorrect|invalid|failed/i.test(line));
        throw new Error(said.map((line) => line.trim()).join("\n") || result.error?.message || `tauri signer sign exited with ${result.status}`);
    }
    return `${file}.sig`;
}

/** Asks for the key's password once (unless TAURI_SIGNING_PRIVATE_KEY_PASSWORD is set) and checks it by signing a scratch file. Throws when the two don't go together. */
export function unlockUpdateKey() {
    if (process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === undefined) {
        const script = [
            "[Console]::OutputEncoding = [Text.Encoding]::UTF8",
            "$p = Read-Host -AsSecureString 'Password of the update key (Enter if it has none)'",
            "$b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($p)",
            "try { [Runtime.InteropServices.Marshal]::PtrToStringBSTR($b) } finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b) }",
        ].join("; ");
        const asked = spawnSync("powershell", ["-NoProfile", "-Command", script], { stdio: ["inherit", "pipe", "inherit"], encoding: "utf8" });
        if (asked.error || asked.status !== 0) throw new Error("Couldn't ask for the update key's password: set TAURI_SIGNING_PRIVATE_KEY_PASSWORD");
        process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = asked.stdout.replace(/\r?\n$/, "");
    }
    const scratch = mkdtempSync(join(tmpdir(), "glint-update-key-"));
    try {
        const file = join(scratch, "check.txt");
        writeFileSync(file, "Glint");
        signWithUpdateKey(file);
    } finally {
        rmSync(scratch, { recursive: true, force: true });
    }
}
