// The update key, for the scripts that sign with it (release.mjs, msix.mjs, sign-service.mjs): the key in
// TAURI_SIGNING_PRIVATE_KEY (the key itself, or the path to it; TAURI_SIGNING_PRIVATE_KEY_PATH works too) and
// its password in TAURI_SIGNING_PRIVATE_KEY_PASSWORD, exactly as `tauri signer sign` took them, so the key and
// the variables the maintainer already has keep working. The signing itself is scripts/lib/minisign.mjs, Node's
// own crypto: no Tauri CLI is needed any more. unlockUpdateKey() asks for the password once (when the key has
// one and the variable isn't set) and checks the two go together, before anything is built with them.

import { spawnSync } from "node:child_process";
import { isEncryptedKey, publicKeyText, secretKeyText, signFile, unlockSecretKey } from "./lib/minisign.mjs";

/** The key (or its path) the environment names. */
const keyValue = () => process.env.TAURI_SIGNING_PRIVATE_KEY || process.env.TAURI_SIGNING_PRIVATE_KEY_PATH;

/** Whether the update key is set. */
export const hasUpdateKey = () => !!keyValue();

let unlocked;

/** Reads the password without echoing it, from the person running the release. */
function askPassword() {
    const script = [
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8",
        "$p = Read-Host -AsSecureString 'Password of the update key (Enter if it has none)'",
        "$b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($p)",
        "try { [Runtime.InteropServices.Marshal]::PtrToStringBSTR($b) } finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b) }",
    ].join("; ");
    const asked = spawnSync("powershell", ["-NoProfile", "-Command", script], { stdio: ["inherit", "pipe", "inherit"], encoding: "utf8" });
    if (asked.error || asked.status !== 0) throw new Error("Couldn't ask for the update key's password: set TAURI_SIGNING_PRIVATE_KEY_PASSWORD");
    return asked.stdout.replace(/\r?\n$/, "");
}

/**
 * Unlocks the update key (once per run: the key's scrypt takes a second or two and a gigabyte of memory) and
 * returns it. With `expectedPublicKey` (the app's public key), throws unless this private key is the one that
 * goes with it. Throws with Tauri's kind of words when the key is missing, unreadable or the password is wrong.
 */
export function unlockUpdateKey({ expectedPublicKey } = {}) {
    if (!unlocked) {
        const value = keyValue();
        if (!value) throw new Error("TAURI_SIGNING_PRIVATE_KEY isn't set (the key, or the path to it)");
        const text = secretKeyText(value);
        if (process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD === undefined && isEncryptedKey(text)) {
            process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = askPassword();
        }
        unlocked = unlockSecretKey(text, process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "");
    }
    if (expectedPublicKey && publicKeyText(unlocked) !== expectedPublicKey.trim()) {
        throw new Error("this private key isn't the one that goes with the app's public key (crates/shadui-index/update-pubkey.txt)");
    }
    return unlocked;
}

/** Signs `file` with the update key into `file`.sig, with `version` sealed in the signature when given. Returns the signature's path; throws why not. */
export async function signWithUpdateKey(file, version) {
    return signFile(file, unlockUpdateKey(), { version });
}
