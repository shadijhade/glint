// What a release needs so that nobody has to be there for it (docs/releasing.md, "A release that needs
// nobody"): the update key and its password where the build is, and the Partner Center application the
// Microsoft Store takes submissions from. This file says whether each works, without ever printing one:
// release-check.mjs runs it on GitHub against the repository's secrets, and ship-setup.mjs on the maintainer's
// PC against what was typed, before anything is stored.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { checkSignature, publicKeyText, secretKeyText, signatureOf, unlockSecretKey } from "./minisign.mjs";
import { signIn, storeClient } from "./store-submission.mjs";

/** The secrets of the repository a release on GitHub uses, by what they are for. */
export const KEY_SECRETS = ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"];
export const STORE_SECRETS = ["PARTNER_CENTER_TENANT_ID", "PARTNER_CENTER_CLIENT_ID", "PARTNER_CENTER_CLIENT_SECRET"];

/** The workflows that go with them. */
export const CHECK_WORKFLOW = "release-check.yml";
export const STORE_WORKFLOW = "store.yml";

/**
 * Whether `key` (the update key's text, or the path to it) with `password` signs updates the app accepts:
 * `publicKey` is the app's (crates/shadui-index/update-pubkey.txt).
 * `{ state, detail }`: "ok", or "empty" (nothing there), "not-a-key", "locked" (the password doesn't open it),
 * "other-key" (a key, but not the app's) or "broken" (it opened and its signature doesn't hold). The detail is a
 * sentence about what was found, and never holds the key or the password.
 */
export async function keyState({ key, password = "", publicKey }) {
    if (!String(key ?? "").trim()) return { state: "empty", detail: "there is no key" };
    let unlocked;
    try {
        unlocked = unlockSecretKey(secretKeyText(key), password ?? "");
    } catch (error) {
        return /password/i.test(error.message)
            ? { state: "locked", detail: "the password doesn't open the key" }
            : { state: "not-a-key", detail: "what is there isn't a minisign key (the key file's whole text is what is wanted)" };
    }
    if (publicKeyText(unlocked) !== String(publicKey ?? "").trim()) {
        return { state: "other-key", detail: "it is a key, but not the one installed copies trust (crates/shadui-index/update-pubkey.txt)" };
    }
    // And it signs: a scrap, signed and checked the way the app checks an update.
    const work = mkdtempSync(join(tmpdir(), "glint-key-check-"));
    try {
        const scrap = join(work, "scrap.bin");
        const data = randomBytes(4096);
        writeFileSync(scrap, data);
        const signature = await signatureOf(scrap, unlocked, { version: "0.0.0" });
        const { problems, comment } = checkSignature(data, signature, publicKey);
        if (problems.length || comment.version !== "0.0.0") return { state: "broken", detail: `its signature doesn't hold: ${problems.join("; ") || "the version isn't sealed in"}` };
    } finally {
        rmSync(work, { recursive: true, force: true });
    }
    return { state: "ok", detail: "the key and its password sign an update the app accepts" };
}

/**
 * Whether the Partner Center application may manage the app `appId` in the Microsoft Store: signs in and reads
 * the app, and changes nothing.
 * `{ state, detail }`: "ok", "unset" (none of the three is there), "incomplete" (some are), or "refused"
 * (Microsoft's answer is the detail). Never holds the application's key.
 */
export async function storeState({ tenantId, clientId, clientSecret, appId }) {
    const given = [tenantId, clientId, clientSecret].map((value) => String(value ?? "").trim());
    if (given.every((value) => !value)) return { state: "unset", detail: "the Partner Center application isn't set" };
    if (given.some((value) => !value)) {
        const missing = ["the tenant ID", "the client ID", "the key"].filter((_, i) => !given[i]);
        return { state: "incomplete", detail: `${missing.join(" and ")} ${missing.length > 1 ? "are" : "is"} missing` };
    }
    try {
        const token = await signIn({ tenantId: given[0], clientId: given[1], clientSecret: given[2] });
        const app = await storeClient(token, appId).app();
        const pending = app.pendingApplicationSubmission?.id;
        return { state: "ok", detail: `Microsoft signs it in, and it may manage ${app.primaryName ?? appId}${pending ? " (a submission is pending there)" : ""}` };
    } catch (error) {
        return { state: "refused", detail: redact(String(error.message ?? error), given[2]).split("\n")[0].slice(0, 300) };
    }
}

/** `text` without `secret` in it, should an answer ever repeat it. */
function redact(text, secret) {
    return secret ? text.split(secret).join("***") : text;
}

/** Whether every part that is set works: the key must, and the Store's application when any of it is there. */
export const sound = ({ key, store }) => key.state === "ok" && ["ok", "unset"].includes(store.state);

// ---- The answer, carried through a workflow's log -------------------------------------------------------------

const MARK = "release-check ";

/** The line release-check.mjs prints for whoever reads the run's log: the two states and nothing else. */
export const markLine = ({ key, store }) => `${MARK}${JSON.stringify({ key: key.state, store: store.state })}`;

/** The states in a run's log (`gh run view --log`), or undefined when the check never got to say. */
export function readMark(log) {
    const found = [...String(log ?? "").matchAll(/release-check (\{"key":"[a-z-]+","store":"[a-z-]+"\})/g)].pop();
    return found ? JSON.parse(found[1]) : undefined;
}

/** What each state means for a release, in a sentence. */
export const KEY_SAYS = {
    ok: "GitHub can sign updates",
    empty: "the TAURI_SIGNING_PRIVATE_KEY secret is empty: GitHub builds Glint but can't sign it",
    "not-a-key": "the TAURI_SIGNING_PRIVATE_KEY secret isn't the key file's text",
    locked: "the TAURI_SIGNING_PRIVATE_KEY_PASSWORD secret doesn't open the key",
    "other-key": "the key in the secret isn't the one installed copies trust",
    broken: "the key in the secret signs, but its signature doesn't hold",
};
export const STORE_SAYS = {
    ok: "GitHub can send Glint to the Microsoft Store",
    unset: "the Partner Center application isn't in the repository's secrets: the Store's package waits for Partner Center's pages",
    incomplete: "only some of the Partner Center application's three secrets are set",
    refused: "Microsoft refuses the Partner Center application in the secrets (a key expires: Partner Center says when)",
};

// ---- What GitHub holds and can do, through the gh program -----------------------------------------------------
//
// `gh` here is a function: (arguments, { input }?) => what gh printed, throwing when gh fails. ship.mjs and
// ship-setup.mjs pass the real one, the tests a stand-in.

const sleep = (seconds) => new Promise((resolve) => setTimeout(resolve, seconds * 1000));
const json = (text, otherwise) => {
    try {
        return JSON.parse(text || "null") ?? otherwise;
    } catch {
        return otherwise;
    }
};

/**
 * What is known of the repository `slug`: the names of its secrets, and the key's and the Store's state as the
 * last Release check found them. A state is "unknown" when no check has run, when its log is gone, or when one
 * of that part's secrets was set after it ran: only a new check knows what a secret holds now.
 * `{ secrets: [name], key, store, checked }` (`checked`: when the check that says so ran, or undefined).
 */
export function githubState(gh, slug) {
    const secrets = json(gh(["secret", "list", "--repo", slug, "--json", "name,updatedAt"]), []);
    const names = secrets.map((secret) => secret.name);
    const unknown = { secrets: names, key: "unknown", store: STORE_SECRETS.some((name) => names.includes(name)) ? "unknown" : "unset", checked: undefined };
    let last;
    let said;
    try {
        const runs = json(gh(["run", "list", "--repo", slug, "--workflow", CHECK_WORKFLOW, "--limit", "5", "--json", "databaseId,status,createdAt"]), []);
        last = runs.find((run) => run.status === "completed");
        if (last) said = readMark(gh(["run", "view", String(last.databaseId), "--repo", slug, "--log"]));
    } catch {
        // The workflow isn't on GitHub yet, or the run's log is gone.
    }
    if (!last || !said) return unknown;
    const setSince = (wanted) => secrets.some((secret) => wanted.includes(secret.name) && Date.parse(secret.updatedAt) > Date.parse(last.createdAt));
    return {
        secrets: names,
        key: setSince(KEY_SECRETS) ? "unknown" : said.key,
        store: setSince(STORE_SECRETS) ? "unknown" : said.store,
        checked: last.createdAt,
    };
}

/** Starts `workflow` on master with `fields` (its inputs) and returns its run's id, once GitHub has made the run. */
export async function startWorkflow(gh, slug, workflow, fields = {}, { wait = 3, tries = 40 } = {}) {
    const list = () => json(gh(["run", "list", "--repo", slug, "--workflow", workflow, "--event", "workflow_dispatch", "--limit", "10", "--json", "databaseId"]), []).map((run) => run.databaseId);
    // The run that wasn't there before is ours: clocks here and at GitHub needn't agree.
    const before = new Set(list());
    gh(["workflow", "run", workflow, "--repo", slug, "--ref", "master", ...Object.entries(fields).flatMap(([name, value]) => ["-f", `${name}=${value}`])]);
    for (let i = 0; i < tries; i++) {
        await sleep(wait);
        const mine = list().find((id) => !before.has(id));
        if (mine) return mine;
    }
    throw new Error(`The ${workflow} workflow didn't start: look at the repository's Actions page.`);
}

/** Waits for run `id` to end and returns its conclusion ("success", "failure", "cancelled"…). */
export async function waitForRun(gh, slug, id, { wait = 10 } = {}) {
    for (;;) {
        const run = json(gh(["run", "view", String(id), "--repo", slug, "--json", "status,conclusion"]), {});
        if (run.status === "completed") return run.conclusion;
        await sleep(wait);
    }
}

/**
 * Runs the Release check on GitHub and returns what it found: `{ key, store, run }`, each state as
 * release-check.mjs names it ("unknown" when the run ended without saying).
 */
export async function checkOnGitHub(gh, slug, timing) {
    const run = await startWorkflow(gh, slug, CHECK_WORKFLOW, {}, timing);
    await waitForRun(gh, slug, run, timing);
    let said;
    // The log is there a moment after the run ends.
    for (let i = 0; i < 6 && !said; i++) {
        try {
            said = readMark(gh(["run", "view", String(run), "--repo", slug, "--log"]));
        } catch {
            said = undefined;
        }
        if (!said) await sleep(timing?.wait ?? 5);
    }
    return { key: said?.key ?? "unknown", store: said?.store ?? "unknown", run };
}
