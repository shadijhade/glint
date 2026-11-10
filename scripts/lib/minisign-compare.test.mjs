// Proves scripts/lib/minisign.mjs signs as the Tauri CLI's `tauri signer sign` did, from stored vectors (the CLI
// is no longer a dependency): scripts/lib/test-vectors/ holds throwaway test keys, a small stand-in file and the
// .sig Tauri wrote for it under each key. The test signs the same file with minisign.mjs under the same trusted
// comment and checks the two .sig files field by field (the same text around the signature, the same algorithm
// and key id, the same lengths and the same base64 envelope), that ours is byte-identical to the stored
// `ours` .sig, and that both hold under the check the app runs (checkSignature, which verify-update.mjs uses).
// Tauri's signature bytes can't be reproduced: its signer (the Rust minisign crate) mixes fresh random noise
// into Ed25519's nonce; ours uses the plain RFC 8032 nonce, which every verifier accepts the same, and is the
// same every time. Also checks the public key text and that a wrong password and a changed byte are refused.
//
// Usage: node scripts/lib/minisign-compare.test.mjs      (npm run test:signing)
// A scrypt run takes a second or two (minisign's key encryption uses 1 GiB of memory). UPDATE_VECTORS=1
// rewrites the stored `ours` .sig files. The keys are made for this test and sign nothing else.

import assert from "node:assert/strict";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { checkSignature, publicKeyText, signatureOf, trustedComment, unlockSecretKey } from "./minisign.mjs";

const vectors = join(import.meta.dirname, "test-vectors");
const file = join(vectors, "stand-in.exe");
const version = "7.8.9";
let checks = 0;
const check = (name, fn) => {
    fn();
    checks++;
    console.log(`  ok  ${name}`);
};

for (const [label, name, password] of [["a key with a password", "pw", "w14-test-password"], ["a key with an empty password", "empty", ""]]) {
    console.log(`${label}:`);
    const keyText = readFileSync(join(vectors, `test-${name}.key`), "utf8");
    const pubText = readFileSync(join(vectors, `test-${name}.key.pub`), "utf8").trim();
    const theirs = readFileSync(join(vectors, `stand-in.${name}.tauri.sig`));
    const theirText = Buffer.from(theirs.toString(), "base64").toString("utf8");
    const trusted = theirText.split("\n")[2].replace(/^trusted comment: /, "");
    const untrusted = theirText.split("\n")[0].replace(/^untrusted comment: /, "");

    const unlocked = unlockSecretKey(keyText, password);
    check("the public key text is Tauri's", () => assert.equal(publicKeyText(unlocked), pubText));
    check("the trusted comment has the shape Tauri writes", () => assert.match(trusted, /^timestamp:\d+\tfile:stand-in\.exe\tversion:7\.8\.9$/));
    check("our default trusted comment has that shape too", () => assert.match(trustedComment(file, version), /^timestamp:\d+\tfile:stand-in\.exe\tversion:7\.8\.9$/));

    const ours = Buffer.from(await signatureOf(file, unlocked, { trusted, untrusted }));
    const oursPath = join(vectors, `stand-in.${name}.ours.sig`);
    if (process.env.UPDATE_VECTORS) writeFileSync(oursPath, ours);
    check("our signature is byte-identical to the stored one", () => assert.deepEqual(ours, readFileSync(oursPath)));
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
    const data = readFileSync(file);
    check("Tauri's signature holds with the public key", () => assert.deepEqual(checkSignature(data, theirs.toString(), pubText).problems, []));
    check("ours holds the same way, bound to the version", () => {
        const result = checkSignature(data, ours.toString(), pubText);
        assert.deepEqual(result.problems, []);
        assert.equal(result.comment.version, version);
        assert.equal(result.comment.file, "stand-in.exe");
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
