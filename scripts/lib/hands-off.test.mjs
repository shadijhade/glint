// Proves the parts of "a release that needs nobody" that can be proved without the real key, the real Store or
// GitHub: what scripts/lib/hands-off.mjs says of a key and of a Partner Center application in each state, how
// the answer travels through a workflow's log, what is taken as known of the repository's secrets, and a whole
// rehearsal of `npm run ship -- --setup` (scripts/ship-setup.mjs) against stand-ins: a gh that keeps the secrets
// it is given and runs the real scripts/release-check.mjs with them, and a Store that signs one application in.
// The rehearsal shows what matters about secrets: each reaches gh on its standard input and nowhere else, a
// wrong one stores nothing, and what is stored is what the check then signs and signs in with.
//
// Usage: node scripts/lib/hands-off.test.mjs      (npm run test:setup)
// The keys are the throwaway ones of scripts/lib/test-vectors (nothing trusts them). A scrypt run takes a second
// or two and a gigabyte of memory, and there are several.

import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { checkOnGitHub, githubState, keyState, markLine, readMark, sound, startWorkflow, storeState } from "./hands-off.mjs";
import { root } from "./project.mjs";

const vectors = join(import.meta.dirname, "test-vectors");
const keyFile = join(vectors, "test-pw.key");
const keyText = readFileSync(keyFile, "utf8");
const pubFile = join(vectors, "test-pw.key.pub");
const pub = readFileSync(pubFile, "utf8").trim();
const otherPub = readFileSync(join(vectors, "test-empty.key.pub"), "utf8").trim();
const PASSWORD = "w14-test-password";
const work = mkdtempSync(join(tmpdir(), "glint-hands-off-test-"));
let checks = 0;
const check = async (name, fn) => {
    await fn();
    checks++;
    console.log(`  ok  ${name}`);
};

// ---- A key ------------------------------------------------------------------------------------------------------

console.log("the update key:");
await check("the key with its password signs: as its text, and as the path to its file", async () => {
    assert.equal((await keyState({ key: keyText, password: PASSWORD, publicKey: pub })).state, "ok");
    assert.equal((await keyState({ key: keyFile, password: PASSWORD, publicKey: pub })).state, "ok");
});
await check("nothing, a wrong password, something that isn't a key and another key are each told apart", async () => {
    assert.equal((await keyState({ key: "", password: PASSWORD, publicKey: pub })).state, "empty");
    assert.equal((await keyState({ key: "  \n", publicKey: pub })).state, "empty");
    assert.equal((await keyState({ key: keyText, password: "not it", publicKey: pub })).state, "locked");
    assert.equal((await keyState({ key: "hello, I am not a key", password: PASSWORD, publicKey: pub })).state, "not-a-key");
    assert.equal((await keyState({ key: keyText, password: PASSWORD, publicKey: otherPub })).state, "other-key");
});
await check("a key without a password needs none, and what is said never holds the key or the password", async () => {
    const plain = readFileSync(join(vectors, "test-empty.key"), "utf8");
    assert.equal((await keyState({ key: plain, publicKey: otherPub })).state, "ok");
    for (const given of [{ key: keyText, password: PASSWORD, publicKey: pub }, { key: keyText, password: "not it", publicKey: pub }, { key: keyText, password: PASSWORD, publicKey: otherPub }]) {
        const { detail } = await keyState(given);
        assert.ok(!detail.includes(given.password) && !detail.includes(keyText.trim()), detail);
    }
});

// ---- A Store that knows one application -------------------------------------------------------------------------

const TENANT = "11111111-2222-3333-4444-555555555555";
const CLIENT = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const SECRET = "stand-in~Key.Of_The-Application";
const store = JSON.parse(readFileSync(join(root, "crates", "glint", "msix", "store.json"), "utf8"));
const seen = { tokens: 0, reads: 0, writes: 0 };
const server = createServer((req, res) => {
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
        const answer = (status, json) => {
            res.writeHead(status, { "Content-Type": "application/json" });
            res.end(JSON.stringify(json));
        };
        if (req.method === "POST" && req.url === `/${TENANT}/oauth2/token`) {
            const form = new URLSearchParams(body);
            if (form.get("client_id") === CLIENT && form.get("client_secret") === SECRET) {
                seen.tokens++;
                return answer(200, { access_token: "stand-in-token" });
            }
            return answer(401, { error: "invalid_client", error_description: "AADSTS7000215: Invalid client secret provided.\r\nTrace ID: 0" });
        }
        if (req.method === "GET" && req.url === `/v1.0/my/applications/${store.storeId}` && req.headers.authorization === "Bearer stand-in-token") {
            seen.reads++;
            return answer(200, { id: store.storeId, primaryName: "Glint - File Explorer", lastPublishedApplicationSubmission: { id: "1" } });
        }
        // Anything else would be a change, or a request nobody should make.
        if (req.method !== "GET") seen.writes++;
        return answer(404, { code: "NotFound", message: `${req.method} ${req.url}` });
    });
});
const address = await new Promise((ready) => server.listen(0, "127.0.0.1", () => ready(`http://127.0.0.1:${server.address().port}`)));
process.env.GLINT_STORE_API = address;
process.env.GLINT_STORE_LOGIN = address;

console.log("the Partner Center application:");
await check("nothing set, some set, the right three and a wrong key are each told apart", async () => {
    assert.equal((await storeState({ appId: store.storeId })).state, "unset");
    const some = await storeState({ tenantId: TENANT, appId: store.storeId });
    assert.equal(some.state, "incomplete");
    assert.match(some.detail, /the client ID and the key are missing/);
    const right = await storeState({ tenantId: TENANT, clientId: CLIENT, clientSecret: SECRET, appId: store.storeId });
    assert.equal(right.state, "ok");
    assert.match(right.detail, /Glint - File Explorer/);
    const wrong = await storeState({ tenantId: TENANT, clientId: CLIENT, clientSecret: "an old key", appId: store.storeId });
    assert.equal(wrong.state, "refused");
    assert.match(wrong.detail, /AADSTS7000215/);
    assert.ok(!wrong.detail.includes("an old key"));
});
await check("it signs in and reads, and asks the Store to change nothing", () => {
    assert.ok(seen.tokens >= 1 && seen.reads >= 1);
    assert.equal(seen.writes, 0);
});
await check("a release needs the key; the Store's application only when some of it is there", () => {
    const state = (key, storeIs) => ({ key: { state: key }, store: { state: storeIs } });
    assert.ok(sound(state("ok", "ok")) && sound(state("ok", "unset")));
    for (const [key, storeIs] of [["empty", "ok"], ["locked", "unset"], ["ok", "refused"], ["ok", "incomplete"]]) assert.ok(!sound(state(key, storeIs)));
});

// ---- The answer in a log ----------------------------------------------------------------------------------------

console.log("the answer, through a workflow's log:");
await check("what the check prints is read back from a run's log, the last of them when there are several", () => {
    const line = markLine({ key: { state: "ok" }, store: { state: "unset" } });
    assert.deepEqual(readMark(`check\tCan GitHub sign\t2026-11-16T10:00:00.1Z ${line}\ncheck\tPost\t2026-11-16T10:00:01Z bye`), { key: "ok", store: "unset" });
    assert.deepEqual(readMark(`${markLine({ key: { state: "empty" }, store: { state: "unset" } })}\n${line}`), { key: "ok", store: "unset" });
    assert.equal(readMark("a log in which the check never got to say"), undefined);
    assert.equal(readMark(undefined), undefined);
});

// ---- What is known of the repository ------------------------------------------------------------------------------

/** A gh that answers from `world`: its secrets, its Release check runs and their logs. */
const fakeGh = (world) => (args) => {
    const line = args.join(" ");
    world.asked.push(line);
    if (line.startsWith("secret list")) return JSON.stringify(world.secrets);
    if (line.startsWith("run list")) return JSON.stringify(world.runs.map(({ log, ...run }) => run));
    if (line.startsWith("run view") && line.includes("--log")) {
        const run = world.runs.find((r) => String(r.databaseId) === args[2]);
        if (!run?.log) throw new Error("log not found");
        return run.log;
    }
    if (line.startsWith("run view")) return JSON.stringify(world.runs.find((r) => String(r.databaseId) === args[2]));
    if (line.startsWith("workflow run")) {
        const id = 100 + world.runs.length;
        world.runs.unshift({ databaseId: id, status: "completed", conclusion: "success", createdAt: new Date().toISOString(), log: world.says });
        return "";
    }
    throw new Error(`the stand-in gh doesn't know: ${line}`);
};
const secret = (name, updatedAt) => ({ name, updatedAt });
const said = (key, storeIs) => `x\ty\tz ${markLine({ key: { state: key }, store: { state: storeIs } })}`;

console.log("what is known of the repository:");
await check("no check yet: the key is unknown, and the Store unset or unknown by whether its secrets are there", () => {
    const none = githubState(fakeGh({ asked: [], secrets: [secret("TAURI_SIGNING_PRIVATE_KEY", "2026-10-22T13:23:27Z")], runs: [] }), "o/r");
    assert.deepEqual([none.key, none.store], ["unknown", "unset"]);
    const some = githubState(fakeGh({ asked: [], secrets: [secret("PARTNER_CENTER_CLIENT_ID", "2026-11-17T00:00:00Z")], runs: [] }), "o/r");
    assert.deepEqual([some.key, some.store], ["unknown", "unknown"]);
});
await check("the last check is believed, until a secret of that part is set after it", () => {
    const run = { databaseId: 7, status: "completed", createdAt: "2026-11-17T10:00:00Z", log: said("ok", "ok") };
    const before = [secret("TAURI_SIGNING_PRIVATE_KEY", "2026-11-17T09:00:00Z"), secret("PARTNER_CENTER_CLIENT_SECRET", "2026-11-17T09:00:00Z")];
    const believed = githubState(fakeGh({ asked: [], secrets: before, runs: [run] }), "o/r");
    assert.deepEqual([believed.key, believed.store, believed.checked], ["ok", "ok", "2026-11-17T10:00:00Z"]);
    const newKey = githubState(fakeGh({ asked: [], secrets: [secret("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "2026-11-17T11:00:00Z"), before[1]], runs: [run] }), "o/r");
    assert.deepEqual([newKey.key, newKey.store], ["unknown", "ok"]);
    const newStore = githubState(fakeGh({ asked: [], secrets: [before[0], secret("PARTNER_CENTER_CLIENT_SECRET", "2026-11-17T11:00:00Z")], runs: [run] }), "o/r");
    assert.deepEqual([newStore.key, newStore.store], ["ok", "unknown"]);
});
await check("a run still going is not an answer, and a log that is gone leaves it unknown", () => {
    const going = { databaseId: 9, status: "in_progress", createdAt: "2026-11-17T12:00:00Z" };
    const done = { databaseId: 8, status: "completed", createdAt: "2026-11-17T10:00:00Z", log: said("empty", "unset") };
    assert.equal(githubState(fakeGh({ asked: [], secrets: [], runs: [going, done] }), "o/r").key, "empty");
    assert.equal(githubState(fakeGh({ asked: [], secrets: [], runs: [{ ...done, log: undefined }] }), "o/r").key, "unknown");
});
await check("a workflow that is started is found as the run that wasn't there before, and the check's answer is its log's", async () => {
    const world = { asked: [], secrets: [], runs: [{ databaseId: 1, status: "completed", createdAt: "2026-11-01T00:00:00Z", log: said("empty", "unset") }], says: said("ok", "unset") };
    const quick = { wait: 0.01 };
    assert.equal(await startWorkflow(fakeGh(world), "o/r", "store.yml", { version: "1.2.3", mode: "submit" }, quick), 101);
    assert.ok(world.asked.includes("workflow run store.yml --repo o/r --ref master -f version=1.2.3 -f mode=submit"));
    assert.deepEqual(await checkOnGitHub(fakeGh(world), "o/r", quick), { key: "ok", store: "unset", run: 102 });
});

// ---- A whole rehearsal of the setup ---------------------------------------------------------------------------------

// A gh for the setup script: it keeps its world in a file, takes secrets on its standard input, and its "Release
// check" is the real scripts/release-check.mjs run with the secrets it holds, as the workflow runs it on GitHub.
const ghProgram = join(work, "gh-stand-in.mjs");
writeFileSync(
    ghProgram,
    `import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
const file = process.env.GLINT_FAKE_GH;
const world = JSON.parse(readFileSync(file, "utf8"));
const args = process.argv.slice(2);
const line = args.join(" ");
const save = () => writeFileSync(file, JSON.stringify(world));
if (line === "auth status") process.exit(0);
else if (line.startsWith("secret list")) console.log(JSON.stringify(world.secrets.map(({ name, updatedAt }) => ({ name, updatedAt }))));
else if (line.startsWith("secret set")) {
    const value = readFileSync(0, "utf8");
    world.calls.push({ args, value });
    world.secrets = world.secrets.filter((s) => s.name !== args[2]).concat({ name: args[2], updatedAt: new Date().toISOString(), value });
    save();
} else if (line.startsWith("secret delete")) {
    world.calls.push({ args });
    world.secrets = world.secrets.filter((s) => s.name !== args[2]);
    save();
} else if (line.startsWith("run list")) console.log(JSON.stringify(world.runs.map(({ log, ...run }) => run)));
else if (line.startsWith("workflow run")) {
    const held = Object.fromEntries(world.secrets.map((s) => [s.name, s.value ?? ""]));
    const env = { ...process.env, TAURI_SIGNING_PRIVATE_KEY: "", TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "", PARTNER_CENTER_TENANT_ID: "", PARTNER_CENTER_CLIENT_ID: "", PARTNER_CENTER_CLIENT_SECRET: "", ...held };
    const ran = spawnSync(process.execPath, ["scripts/release-check.mjs"], { cwd: process.env.GLINT_ROOT, env, encoding: "utf8" });
    world.runs.unshift({ databaseId: 500 + world.runs.length, status: "completed", conclusion: ran.status === 0 ? "success" : "failure", createdAt: new Date().toISOString(), log: ran.stdout });
    save();
} else if (line.startsWith("run view") && line.includes("--log")) console.log(world.runs.find((r) => String(r.databaseId) === args[2]).log);
else if (line.startsWith("run view")) console.log(JSON.stringify(world.runs.find((r) => String(r.databaseId) === args[2])));
else {
    console.error("the stand-in gh doesn't know: " + line);
    process.exit(1);
}
`,
);

/** Runs the setup against a world (a file) and returns how it ended, what it printed and the world after. */
function setup(worldFile, args, env = {}) {
    return new Promise((ended) => {
        const child = spawn(process.execPath, [join(root, "scripts", "ship-setup.mjs"), ...args], {
            cwd: root,
            // No terminal: what it would ask for comes from the variables.
            stdio: ["ignore", "pipe", "pipe"],
            env: {
                ...process.env,
                GLINT_GH: ghProgram,
                GLINT_FAKE_GH: worldFile,
                GLINT_ROOT: root,
                GLINT_SETUP_WAIT: "0.02",
                GLINT_UPDATE_PUBKEY: pubFile,
                TAURI_SIGNING_PRIVATE_KEY: "",
                TAURI_SIGNING_PRIVATE_KEY_PASSWORD: PASSWORD,
                PARTNER_CENTER_TENANT_ID: TENANT,
                PARTNER_CENTER_CLIENT_ID: CLIENT,
                PARTNER_CENTER_CLIENT_SECRET: SECRET,
                ...env,
            },
        });
        let printed = "";
        child.stdout.on("data", (chunk) => (printed += chunk));
        child.stderr.on("data", (chunk) => (printed += chunk));
        child.on("close", (status) => ended({ status, printed, world: JSON.parse(readFileSync(worldFile, "utf8")) }));
    });
}
/** A repository as Glint's is today: the key's secret there and empty, its password set, no Store application. */
function repository(name) {
    const file = join(work, `${name}.json`);
    const world = { calls: [], runs: [], secrets: [{ name: "TAURI_SIGNING_PRIVATE_KEY", updatedAt: "2026-10-22T13:23:27Z", value: "" }, { name: "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", updatedAt: "2026-10-22T13:23:37Z", value: "an old password" }] };
    writeFileSync(file, JSON.stringify(world));
    return file;
}
const stored = (world) => Object.fromEntries(world.calls.map((call) => [call.args[2], call.value]));
const SECRETS = [PASSWORD, SECRET, keyText.trim(), keyText.trim().split(/\r?\n/).pop()];

console.log("npm run ship -- --setup, rehearsed:");
const first = repository("first");
await check("--dry says what is missing, asks nothing and stores nothing", async () => {
    const { status, printed, world } = await setup(first, ["--dry"]);
    assert.equal(status, 0, printed);
    assert.match(printed, /The update key: nobody knows yet/);
    assert.match(printed, /The Microsoft Store: the Partner Center application isn't in the repository's secrets/);
    assert.deepEqual([world.calls.length, world.runs.length], [0, 0]);
});
await check("--check asks GitHub, finds the key's secret empty, and stores nothing", async () => {
    const { status, printed, world } = await setup(first, ["--check"]);
    assert.equal(status, 1, printed);
    assert.match(printed, /the TAURI_SIGNING_PRIVATE_KEY secret is empty/);
    assert.deepEqual([world.calls.length, world.runs.length], [0, 1]);
});
await check("the setup stores the key, its password and the application, and GitHub then signs and signs in with them", async () => {
    const { status, printed, world } = await setup(first, ["--key", keyFile, "--yes"]);
    assert.equal(status, 0, printed);
    assert.match(printed, /GitHub can sign Glint's updates and send it to the Microsoft Store/);
    assert.deepEqual(stored(world), {
        TAURI_SIGNING_PRIVATE_KEY: keyText.trim(),
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD: PASSWORD,
        PARTNER_CENTER_TENANT_ID: TENANT,
        PARTNER_CENTER_CLIENT_ID: CLIENT,
        PARTNER_CENTER_CLIENT_SECRET: SECRET,
    });
    // The real check, run with what was stored, is what said so.
    assert.deepEqual(readMark(world.runs[0].log), { key: "ok", store: "ok" });
    assert.equal(world.runs[0].conclusion, "success");
});
await check("every secret went to gh on its standard input: none is among its arguments, and none was printed", async () => {
    const { printed, world } = await setup(first, ["--dry"]);
    const second = await setup(repository("printed"), ["--key", keyFile, "--yes"]);
    for (const value of SECRETS) {
        for (const call of world.calls) assert.ok(!call.args.join(" ").includes(value));
        assert.ok(!printed.includes(value) && !second.printed.includes(value));
    }
    for (const call of world.calls) assert.deepEqual(call.args.slice(0, 2).concat(call.args.slice(3)), ["secret", "set", "--repo", "shadijhade/glint"]);
    assert.equal(world.calls.length, 5);
});
await check("run again, it finds nothing missing and stores nothing", async () => {
    const before = JSON.parse(readFileSync(first, "utf8")).calls.length;
    const { status, printed, world } = await setup(first, ["--key", keyFile, "--yes"]);
    assert.equal(status, 0, printed);
    assert.match(printed, /Nothing is missing/);
    assert.equal(world.calls.length, before);
});
await check("a wrong password stores nothing at all", async () => {
    const { status, printed, world } = await setup(repository("wrong-password"), ["--key", keyFile, "--yes"], { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "not it" });
    assert.equal(status, 1, printed);
    assert.match(printed, /Nothing was stored: the password doesn't open the key/);
    assert.equal(world.calls.length, 0);
});
await check("a key that isn't the app's stores nothing", async () => {
    const { status, printed, world } = await setup(repository("other-key"), ["--key", join(vectors, "test-empty.key"), "--yes"], { TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "" });
    assert.equal(status, 1, printed);
    assert.match(printed, /Nothing was stored: it is a key, but not the one installed copies trust/);
    assert.equal(world.calls.length, 0);
});
await check("an application Microsoft refuses isn't stored; the key is, and GitHub signs with it", async () => {
    const { status, printed, world } = await setup(repository("old-application"), ["--key", keyFile, "--yes"], { PARTNER_CENTER_CLIENT_SECRET: "an old key" });
    assert.equal(status, 1, printed);
    assert.match(printed, /The Store's application wasn't stored: .*AADSTS7000215/);
    assert.deepEqual(Object.keys(stored(world)), ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"]);
    assert.deepEqual(readMark(world.runs[0].log), { key: "ok", store: "unset" });
    assert.ok(!printed.includes("an old key"));
});
await check("a key whose password is empty takes the old password away, and GitHub signs with it", async () => {
    const { status, printed, world } = await setup(repository("empty-password"), ["--key", join(vectors, "test-empty.key"), "--yes", "--no-store"], {
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
        GLINT_UPDATE_PUBKEY: join(vectors, "test-empty.key.pub"),
    });
    assert.equal(status, 0, printed);
    assert.deepEqual(world.calls.map((call) => call.args.slice(0, 3).join(" ")), ["secret set TAURI_SIGNING_PRIVATE_KEY", "secret delete TAURI_SIGNING_PRIVATE_KEY_PASSWORD"]);
    assert.deepEqual(readMark(world.runs[0].log), { key: "ok", store: "unset" });
});
await check("--no-store sets the key up alone, and a release then needs nobody but for the Store", async () => {
    const { status, printed, world } = await setup(repository("key-only"), ["--key", keyFile, "--yes", "--no-store"]);
    assert.equal(status, 0, printed);
    assert.match(printed, /GitHub can sign Glint's updates\. From now on a release is one command/);
    assert.deepEqual(Object.keys(stored(world)), ["TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"]);
});
await check("with nobody to ask and no --yes, it stops before reading the key", async () => {
    const { status, printed, world } = await setup(repository("nobody"), ["--key", keyFile]);
    assert.equal(status, 1, printed);
    assert.match(printed, /There is no terminal to ask in/);
    assert.equal(world.calls.length, 0);
});
await check("the Store was only ever signed in to and read", () => assert.equal(seen.writes, 0));

server.close();
rmSync(work, { recursive: true, force: true });
console.log(`\n${checks} checks passed: a key and an application are told apart in every state, and the setup stores only what works, where only gh sees it.`);
