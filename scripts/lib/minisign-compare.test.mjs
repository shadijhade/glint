// Proves scripts/lib/minisign.mjs signs as `tauri signer sign` does: makes throwaway keys with
// `tauri signer generate`, signs a file with Tauri and with minisign.mjs under the same trusted comment, and
// compares the two .sig files field by field: the same text around the signature, the same algorithm and key
// id, the same lengths and the same base64 envelope, and both hold under the check the app runs
// (checkSignature, which verify-update.mjs uses). The signature bytes themselves cannot be compared:
// Tauri's signer (the Rust minisign crate) mixes fresh random noise into Ed25519's nonce, so two of its own
// signatures of one file under one trusted comment differ from each other too (this test prints that); ours
// uses the plain RFC 8032 nonce, which every verifier accepts the same, and is the same every time. Also
// checks the public key text, the default trusted comment, and that a wrong password and a changed byte are
// refused.
//
// Usage: node scripts/lib/minisign-compare.test.mjs      (npm run test:signing)
// Needs @tauri-apps/cli in node_modules (the repository's, or a parent checkout's); a scrypt run takes a
// second or two (minisign's key encryption uses 1 GiB of memory). GLINT_TEST_DIR names a folder for the
// throwaway keys and files (kept afterwards); otherwise a temporary one is used and removed. TAURI_CLI names
// tauri.js if it can't be found. The keys here are made for this test and signed with nothing else.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { checkSignature, publicKeyText, signatureOf, trustedComment, unlockSecretKey } from "./minisign.mjs";

/** tauri.js of @tauri-apps/cli: TAURI_CLI, else node_modules here or in a parent folder (a worktree sits inside its checkout). */
function findTauri() {
    if (process.env.TAURI_CLI) return process.env.TAURI_CLI;
    for (let dir = resolve(import.meta.dirname, "..", ".."); ; dir = dirname(dir)) {
        const candidate = join(dir, "node_modules", "@tauri-apps", "cli", "tauri.js");
        if (existsSync(candidate)) return candidate;
        if (dirname(dir) === dir) throw new Error("@tauri-apps/cli isn't in any node_modules above here: set TAURI_CLI to its tauri.js");
    }
}

const tauri = findTauri();
const given = process.env.GLINT_TEST_DIR;
const dir = given ? resolve(given) : mkdtempSync(join(tmpdir(), "glint-minisign-"));
mkdirSync(dir, { recursive: true });
if (!given) process.on("exit", () => rmSync(dir, { recursive: true, force: true }));

const tauriCli = (args, env = {}) => {
    const result = spawnSync(process.execPath, [tauri, ...args], { encoding: "utf8", env: { ...process.env, ...env } });
    assert.equal(result.status, 0, `tauri ${args[0]} ${args[1]} failed:\n${result.stdout}\n${result.stderr}`);
    return result;
};

const file = join(dir, "installer-stand-in.exe");
// A few MB, so hashing runs over more than one read.
writeFileSync(file, Buffer.concat([randomBytes(3_000_000), Buffer.from("Glint")]));
const version = "7.8.9";
let checks = 0;
const check = (name, fn) => {
    fn();
    checks++;
    console.log(`  ok  ${name}`);
};

for (const [label, password] of [["a key with a password", "w14-test-password"], ["a key with an empty password", ""]]) {
    console.log(`${label}:`);
    const keyFile = join(dir, `test-${password ? "pw" : "empty"}.key`);
    rmSync(keyFile, { force: true });
    rmSync(`${keyFile}.pub`, { force: true });
    tauriCli(["signer", "generate", "--ci", "-p", password, "-w", keyFile]);
    const keyText = readFileSync(keyFile, "utf8");
    const pubText = readFileSync(`${keyFile}.pub`, "utf8").trim();

    // Tauri signs: the key reaches it as a path, as update-key.mjs and the docs do.
    rmSync(`${file}.sig`, { force: true });
    tauriCli(["signer", "sign", "--app-version", version, file], { TAURI_SIGNING_PRIVATE_KEY_PATH: keyFile, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: password });
    const theirs = readFileSync(`${file}.sig`);
    const theirText = Buffer.from(theirs.toString(), "base64").toString("utf8");
    const trusted = theirText.split("\n")[2].replace(/^trusted comment: /, "");
    const untrusted = theirText.split("\n")[0].replace(/^untrusted comment: /, "");

    const unlocked = unlockSecretKey(keyText, password);
    check("the public key text is Tauri's", () => assert.equal(publicKeyText(unlocked), pubText));
    check("the trusted comment has the shape Tauri writes", () => assert.match(trusted, /^timestamp:\d+\tfile:installer-stand-in\.exe\tversion:7\.8\.9$/));
    check("our default trusted comment has that shape too", () => assert.match(trustedComment(file, version), /^timestamp:\d+\tfile:installer-stand-in\.exe\tversion:7\.8\.9$/));

    const ours = Buffer.from(await signatureOf(file, unlocked, { trusted, untrusted }));
    const parts = (sig) => {
        const text = Buffer.from(sig.toString(), "base64").toString("utf8");
        const lines = text.split("\n");
        return { text, lines, line: Buffer.from(lines[1], "base64"), global: Buffer.from(lines[3], "base64") };
    };
    const [a, b] = [parts(theirs), parts(ours)];
    check(`the .sig has Tauri's layout (${theirs.length} bytes, base64 of four lines and a closing newline)`, () => {
        assert.equal(ours.length, theirs.length);
        assert.equal(b.lines.length, a.lines.length);
        assert.ok(b.text.endsWith("\n") && a.text.endsWith("\n"));
        assert.equal(b.lines[0], a.lines[0]);
        assert.equal(b.lines[2], a.lines[2]);
    });
    check("the same algorithm (ED, BLAKE2b-prehashed) and key id, signatures of 64 bytes", () => {
        assert.equal(b.line.length, 74);
        assert.deepEqual(b.line.subarray(0, 10), a.line.subarray(0, 10));
        assert.equal(b.global.length, a.global.length);
        assert.equal(b.global.length, 64);
    });
    const repeated = Buffer.from(await signatureOf(file, unlocked, { trusted, untrusted }));
    check("our signing is repeatable", () => assert.deepEqual(repeated, ours));
    rmSync(`${file}.sig`, { force: true });
    tauriCli(["signer", "sign", "--app-version", version, file], { TAURI_SIGNING_PRIVATE_KEY_PATH: keyFile, TAURI_SIGNING_PRIVATE_KEY_PASSWORD: password });
    const again = readFileSync(`${file}.sig`);
    console.log(`  --  Tauri's own second signature of the file ${again.equals(theirs) ? "is" : "is not"} byte-identical to its first (random nonce): bytes can't be compared, structure and validity are`);
    const data = readFileSync(file);
    check("Tauri's signature holds with the public key", () => assert.deepEqual(checkSignature(data, theirs.toString(), pubText).problems, []));
    check("ours holds the same way, bound to the version", () => {
        const result = checkSignature(data, ours.toString(), pubText);
        assert.deepEqual(result.problems, []);
        assert.equal(result.comment.version, version);
        assert.equal(result.comment.file, "installer-stand-in.exe");
    });
    check("a changed byte is refused", () => {
        const changed = Buffer.from(data);
        changed[10] ^= 1;
        assert.ok(checkSignature(changed, ours.toString(), pubText).problems.length > 0);
    });
    check("another key is refused", () => {
        const other = publicKeyText({ keyId: Buffer.alloc(8, 1), publicKey: unlocked.publicKey });
        assert.ok(checkSignature(data, ours.toString(), other).problems.length > 0);
    });
    if (password) {
        check("a wrong password is refused", () => assert.throws(() => unlockSecretKey(keyText, `${password}x`), /incorrect updater private key password/));
        check("no password is refused for a key that has one", () => assert.throws(() => unlockSecretKey(keyText, ""), /incorrect updater private key password/));
    }
}
check("text that isn't a key is refused", () => assert.throws(() => unlockSecretKey("not a key", ""), /isn't a minisign key/));
console.log(`\n${checks} checks passed: minisign.mjs writes what tauri signer sign writes, and what the app verifies.`);
