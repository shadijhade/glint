// minisign signatures with nothing but Node's built-in crypto (scrypt, blake2b512, ed25519), in the formats
// `tauri signer sign` reads and writes, so the release pipeline needs no Tauri CLI to sign:
//
//   * The private key is what `tauri signer generate` writes: base64 of a minisign secret key box
//     ("untrusted comment: rsign encrypted secret key" and the key on the next line), the key itself
//     scrypt-encrypted with the password. TAURI_SIGNING_PRIVATE_KEY holds it (or the path to it),
//     TAURI_SIGNING_PRIVATE_KEY_PASSWORD its password: update-key.mjs reads both.
//   * A signature is base64 of a minisign signature box: an untrusted comment, the Ed25519 signature of the
//     file's BLAKE2b-512 hash ("ED", the prehashed form), a trusted comment and the signature over the
//     signature and that comment. The trusted comment binds `timestamp`, `file` and `version`, which is how
//     the app's updater (requireSignedVersion) and the instant search service know what a signature is for.
//
// scripts/lib/minisign-compare.test.mjs proves the result is what `tauri signer sign` writes. The signature
// bytes themselves differ from run to run in Tauri's (its Rust signer adds random noise to Ed25519's nonce);
// ours uses the plain RFC 8032 nonce, the same every time, and any verifier accepts both alike.
//
// Layout of the secret key (minisign's, 158 bytes): "Ed" | kdf "Sc" | checksum "B2" | salt (32) |
// opslimit (8, little endian) | memlimit (8, little endian) | the encrypted part (104): key id (8),
// Ed25519 secret key (64: the seed, then the public key) and a BLAKE2b-256 checksum (32).

import { createHash, createPrivateKey, createPublicKey, scryptSync, sign, verify } from "node:crypto";
import { createReadStream, existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";

const PKCS8_SEED = Buffer.from("302e020100300506032b657004220420", "hex");
const SPKI_KEY = Buffer.from("302a300506032b6570032100", "hex");

/** The text of a minisign box from what Tauri stores: the box base64-encoded, or the box itself. */
const boxText = (text) => {
    const trimmed = String(text).trim();
    return trimmed.startsWith("untrusted comment:") ? trimmed : Buffer.from(trimmed.replace(/\s+/g, ""), "base64").toString("utf8");
};
const boxLines = (text) => boxText(text).split(/\r?\n/);

/**
 * The scrypt parameters libsodium's crypto_pwhash_scryptsalsa208sha256 picks for minisign's opslimit and
 * memlimit (pickparams in libsodium), which is what the key was encrypted with.
 */
export function scryptParams(opslimit, memlimit) {
    const r = 8;
    opslimit = Math.max(opslimit, 32768);
    let p;
    let log2;
    if (opslimit < memlimit / 32) {
        p = 1;
        const maxN = Math.floor(opslimit / (r * 4));
        for (log2 = 1; log2 < 63; log2++) if (2 ** log2 > maxN / 2) break;
    } else {
        const maxN = Math.floor(memlimit / (r * 128));
        for (log2 = 1; log2 < 63; log2++) if (2 ** log2 > maxN / 2) break;
        const maxrp = Math.min(Math.floor(opslimit / 4 / 2 ** log2), 0x3fffffff);
        p = Math.floor(maxrp / r);
    }
    return { N: 2 ** log2, r, p };
}

/** The 32-byte public key of an Ed25519 seed. */
function publicKeyOfSeed(seed) {
    const key = createPrivateKey({ key: Buffer.concat([PKCS8_SEED, seed]), format: "der", type: "pkcs8" });
    return createPublicKey(key).export({ format: "der", type: "spki" }).subarray(-32);
}

/**
 * Decrypts a minisign secret key (what TAURI_SIGNING_PRIVATE_KEY holds, as text) with its password.
 * Returns { keyId, publicKey, privateKey } (a Node KeyObject), or throws with Tauri's kind of words when the
 * text isn't a key or the password is wrong.
 */
export function unlockSecretKey(keyText, password = "") {
    const bytes = Buffer.from(boxLines(keyText)[1] ?? "", "base64");
    if (bytes.length !== 158 || bytes.subarray(0, 2).toString() !== "Ed") throw new Error("failed to decode the secret key: it isn't a minisign key");
    let plain;
    if (bytes[2] === 0 && bytes[3] === 0) {
        // A key written without a password: the secret part is stored as it is.
        plain = bytes.subarray(54);
    } else {
        if (bytes.subarray(2, 4).toString() !== "Sc" || bytes.subarray(4, 6).toString() !== "B2") throw new Error("failed to decode the secret key: unsupported key derivation");
        const opslimit = Number(bytes.readBigUInt64LE(38));
        const memlimit = Number(bytes.readBigUInt64LE(46));
        const { N, r, p } = scryptParams(opslimit, memlimit);
        // 128 * N * r bytes of working memory (1 GiB for minisign's own settings), plus room.
        const stream = scryptSync(Buffer.from(password, "utf8"), bytes.subarray(6, 38), 104, { N, r, p, maxmem: 128 * N * r * (p + 1) + (1 << 20) });
        plain = Buffer.from(bytes.subarray(54).map((b, i) => b ^ stream[i]));
    }
    const keyId = plain.subarray(0, 8);
    const seed = plain.subarray(8, 40);
    const publicKey = plain.subarray(40, 72);
    // A wrong password gives noise: the public key the seed makes isn't the one stored beside it.
    let derived;
    try {
        derived = publicKeyOfSeed(seed);
    } catch {
        derived = Buffer.alloc(0);
    }
    if (!derived.equals(publicKey)) throw new Error("incorrect updater private key password: the key can't be decrypted");
    return { keyId: Buffer.from(keyId), publicKey: Buffer.from(publicKey), privateKey: createPrivateKey({ key: Buffer.concat([PKCS8_SEED, seed]), format: "der", type: "pkcs8" }) };
}

/** Whether a secret key needs a password (a key made without one has no key derivation). */
export function isEncryptedKey(keyText) {
    const bytes = Buffer.from(boxLines(keyText)[1] ?? "", "base64");
    return !(bytes[2] === 0 && bytes[3] === 0);
}

/** The key text from what TAURI_SIGNING_PRIVATE_KEY holds: the key itself, or the path to its file. */
export function secretKeyText(value) {
    const trimmed = String(value ?? "").trim();
    if (!trimmed) throw new Error("no private key");
    return existsSync(trimmed) ? readFileSync(trimmed, "utf8") : trimmed;
}

/** A public key as Tauri stores it (base64 of the minisign box, or the box): { keyId, key } or null. */
export function parsePublicKey(text) {
    const bytes = Buffer.from(boxLines(text ?? "")[1] ?? "", "base64");
    return bytes.length === 42 && bytes.subarray(0, 2).toString() === "Ed" ? { keyId: bytes.subarray(2, 10), key: bytes.subarray(10) } : null;
}

/** The public key of an unlocked key, in the form tauri.conf.json's plugins.updater.pubkey (and update-pubkey.txt) holds it. */
export function publicKeyText(unlocked) {
    const bytes = Buffer.concat([Buffer.from("Ed"), unlocked.keyId, unlocked.publicKey]);
    const id = Buffer.from(unlocked.keyId).reverse().toString("hex").toUpperCase();
    const box = `untrusted comment: minisign public key: ${id}\n${bytes.toString("base64")}\n`;
    return Buffer.from(box).toString("base64");
}

/** BLAKE2b-512 of a file, read in pieces. */
async function hashFile(file) {
    const hash = createHash("blake2b512");
    for await (const chunk of createReadStream(file)) hash.update(chunk);
    return hash.digest();
}

/** The trusted comment Tauri writes: when, for which file and (with --app-version) for which version. */
export function trustedComment(file, version, now = Date.now()) {
    return `timestamp:${Math.floor(now / 1000)}\tfile:${basename(file)}${version ? `\tversion:${version}` : ""}`;
}

/**
 * The signature of `file` as text to put in a .sig file and in latest.json's `signature`: base64 of the
 * minisign box. `unlocked` is unlockSecretKey's result; `trusted` replaces the trusted comment Tauri would write.
 */
export async function signatureOf(file, unlocked, { version, trusted, untrusted = "signature from tauri secret key" } = {}) {
    const comment = trusted ?? trustedComment(file, version);
    const signature = sign(null, await hashFile(file), unlocked.privateKey);
    const line = Buffer.concat([Buffer.from("ED"), unlocked.keyId, signature]);
    const global = sign(null, Buffer.concat([signature, Buffer.from(comment)]), unlocked.privateKey);
    const box = `untrusted comment: ${untrusted}\n${line.toString("base64")}\ntrusted comment: ${comment}\n${global.toString("base64")}\n`;
    return Buffer.from(box).toString("base64");
}

/** Signs `file` into `file`.sig as `tauri signer sign` does; returns the signature's path. */
export async function signFile(file, unlocked, options) {
    writeFileSync(`${file}.sig`, await signatureOf(file, unlocked, options));
    return `${file}.sig`;
}

/** Ed25519 verification that answers false for malformed input instead of throwing. */
function ed25519(key, message, signature) {
    try {
        return verify(null, message, createPublicKey({ key: Buffer.concat([SPKI_KEY, key]), format: "der", type: "spki" }), signature);
    } catch {
        return false;
    }
}

/**
 * Checks a signature the way the app's updater does: made with this public key, for these very bytes, and the
 * trusted comment (bound to the version) really signed. Returns { problems: [..], trusted, comment: {key: value} };
 * `problems` is empty when it holds. `data` is the file's bytes (a Buffer).
 */
export function checkSignature(data, signatureText, publicKeyBase64) {
    const problems = [];
    const pub = parsePublicKey(publicKeyBase64);
    if (!pub) return { problems: ["no usable public key"], trusted: "", comment: {} };
    // Lines: untrusted comment, "ED" (BLAKE2b-prehashed) or "Ed" | key id | signature, trusted comment, its signature.
    const [, sigLine = "", trustedLine = "", globalLine = ""] = boxLines(signatureText ?? "");
    const sig = Buffer.from(sigLine, "base64");
    const trusted = trustedLine.replace(/^trusted comment: /, "");
    if (!(sig.length === 74 && sig.subarray(2, 10).equals(pub.keyId))) problems.push("not signed with this key");
    const prehashed = sig.subarray(0, 2).toString() === "ED";
    if (!ed25519(pub.key, prehashed ? createHash("blake2b512").update(data).digest() : data, sig.subarray(10))) problems.push("the signature doesn't match the file");
    if (!ed25519(pub.key, Buffer.concat([sig.subarray(10), Buffer.from(trusted)]), Buffer.from(globalLine, "base64"))) problems.push("the signature's trusted comment doesn't verify");
    const comment = Object.fromEntries(trusted.split("\t").map((part) => [part.slice(0, part.indexOf(":")), part.slice(part.indexOf(":") + 1)]));
    return { problems, trusted, comment };
}
