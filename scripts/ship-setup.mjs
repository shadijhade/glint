// Once, so that a release needs nobody (docs/releasing.md, "A release that needs nobody"). `npm run ship` does
// a whole release by itself except for two things only the maintainer has: the update key (with its password)
// and the Partner Center application the Microsoft Store takes submissions from. This puts them where the
// release runs, in the repository's Actions secrets, and proves they work there:
//
//   1. the update key: read from its file, opened with the password you type (so a wrong one stops here), and
//      stored as TAURI_SIGNING_PRIVATE_KEY and TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
//   2. the Partner Center application: its tenant ID, client ID and key, tried against the Store first (a
//      sign-in and a read), and stored as PARTNER_CENTER_TENANT_ID, PARTNER_CENTER_CLIENT_ID and
//      PARTNER_CENTER_CLIENT_SECRET;
//   3. the Release check workflow, which signs a scrap and reads the Store with those secrets, on GitHub.
//
// It asks only for what is missing, shows nothing that is typed in secret, hands each value to `gh secret set`
// on its standard input (never as an argument, never to a file), and stores nothing on this PC. GitHub encrypts
// a secret for the repository: nobody can read it back, and the repository's workflows can use it.
//
// Usage: npm run ship -- --setup [--dry | --check] [--key <file>] [--no-key] [--no-store] [--yes]
//   --dry        say what is known to be set up and what is missing: asks nothing, starts nothing, changes nothing
//   --check      run the Release check on GitHub and say what it found: asks nothing, changes nothing
//   --key <file> the update key's file (default: TAURI_SIGNING_PRIVATE_KEY when it names a file, else
//                %USERPROFILE%\.tauri\shadui-ripgrep.key)
//   --no-key, --no-store   leave that part as it is
//   --yes        don't ask "go on?" before storing
// With no terminal to ask in, the values come from TAURI_SIGNING_PRIVATE_KEY_PASSWORD, PARTNER_CENTER_TENANT_ID,
// PARTNER_CENTER_CLIENT_ID and PARTNER_CENTER_CLIENT_SECRET (and --yes says to go on).

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { askHidden, askLine, askYes, canAsk } from "./lib/ask.mjs";
import { KEY_SAYS, KEY_SECRETS, STORE_SAYS, STORE_SECRETS, checkOnGitHub, githubState, keyState, storeState } from "./lib/hands-off.mjs";
import { isEncryptedKey } from "./lib/minisign.mjs";
import { REPO, root, updatePubkey } from "./lib/project.mjs";

const SLUG = REPO.replace("https://github.com/", "");
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const value = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
const KNOWN = ["--setup", "--dry", "--check", "--no-key", "--no-store", "--yes"];
for (let i = 0; i < args.length; i++) {
    if (args[i] === "--key") i++;
    else if (!KNOWN.includes(args[i])) fail(`Unknown option ${args[i]} (the top of scripts/ship-setup.mjs lists them).`);
}

function fail(message) {
    console.error(`\n✗ ${message}`);
    process.exit(1);
}
const say = (message = "") => console.log(message);
const done = (message) => console.log(`✓ ${message}`);

/** gh; a rehearsal names a stand-in for it (GLINT_GH, a Node script: scripts/lib/hands-off.test.mjs). */
const GH = process.env.GLINT_GH ? [process.execPath, process.env.GLINT_GH] : ["gh"];
function gh(ghArgs, { input } = {}) {
    const result = spawnSync(GH[0], [...GH.slice(1), ...ghArgs], { cwd: root, encoding: "utf8", input, maxBuffer: 256 * 1024 * 1024 });
    if (result.error) throw new Error(`gh: ${result.error.message}`);
    // What failed is named by its first words: a secret is never among gh's arguments.
    if (result.status !== 0) throw new Error(`gh ${ghArgs.slice(0, 3).join(" ")}: ${(result.stderr || result.stdout || "").trim().split("\n")[0]}`);
    return (result.stdout ?? "").trim();
}
/** How long to wait between looks at GitHub (a rehearsal doesn't wait). */
const timing = process.env.GLINT_SETUP_WAIT ? { wait: Number(process.env.GLINT_SETUP_WAIT) } : undefined;

/** Stores `text` as the repository's secret `name`: on gh's standard input, so it is in no command line. */
function putSecret(name, text) {
    gh(["secret", "set", name, "--repo", SLUG], { input: text });
}

const storeIds = JSON.parse(readFileSync(join(root, "crates", "glint", "msix", "store.json"), "utf8"));
const ACCOUNT = "https://partner.microsoft.com/dashboard/account/v3";

/** One line for each part: what is known of it. */
function report(state) {
    const sign = (s) => (s === "ok" ? "✓" : s === "unset" || s === "unknown" ? "!" : "✗");
    const unknown = "nobody knows yet: no Release check has run since its secrets were set";
    say(`  ${sign(state.key)} The update key: ${state.key === "unknown" ? unknown : KEY_SAYS[state.key]}`);
    say(`  ${sign(state.store)} The Microsoft Store: ${state.store === "unknown" ? unknown : STORE_SAYS[state.store]}`);
    if (state.checked) say(`    (the Release check of ${state.checked.slice(0, 10)} says so)`);
}

const ready = (state) => state.key === "ok" && state.store === "ok";
const NEXT = "From now on a release is one command: npm run ship -- <x.y.z>";

// ---- What is there ---------------------------------------------------------------------------------------------

say("Glint: a release that needs nobody\n");
try {
    gh(["auth", "status"]);
} catch {
    fail("gh isn't signed in: gh auth login.");
}
let state;
try {
    state = githubState(gh, SLUG);
} catch (error) {
    fail(`Couldn't read ${SLUG}'s secrets (it takes the repository's owner): ${error.message}`);
}

if (flag("--dry")) {
    report(state);
    say(ready(state) ? `\n${NEXT}` : "\nnpm run ship -- --setup sets up what is missing (--check asks GitHub what its secrets can do, in a minute).");
    process.exit(0);
}

/** Runs the Release check and takes its answer as the state. */
async function check(why) {
    say(`${why} (the Release check, a minute)…`);
    let found;
    try {
        found = await checkOnGitHub(gh, SLUG, timing);
    } catch (error) {
        fail(`${error.message}\n  The Release check has to be on GitHub's master first: push, then run this again.`);
    }
    state = { ...state, key: found.key, store: found.store, checked: new Date().toISOString() };
    say(`  ${REPO}/actions/runs/${found.run}`);
    report(state);
    say();
}

if (flag("--check") || state.key === "unknown" || state.store === "unknown") await check("Asking GitHub what its secrets can do");
else {
    report(state);
    say();
}
if (flag("--check")) process.exit(ready(state) || (state.key === "ok" && state.store === "unset") ? 0 : 1);
if (ready(state)) {
    done(`Nothing is missing. ${NEXT}`);
    process.exit(0);
}

const interactive = canAsk();
/** "Go on?", unless --yes; with nobody to ask and no --yes, no. */
async function goOn(question) {
    if (flag("--yes")) return true;
    if (!interactive) fail("There is no terminal to ask in: run this in one, or pass --yes with the values in their variables (the top of scripts/ship-setup.mjs).");
    return askYes(question);
}
let stored = false;
/** A part that was refused and left as it was: said again at the end, after what did work was proved. */
let trouble;

// ---- 1. The update key -----------------------------------------------------------------------------------------

if (!flag("--no-key") && state.key !== "ok") {
    say("1. The update key");
    const named = process.env.TAURI_SIGNING_PRIVATE_KEY?.trim();
    const file = resolve(value("--key") ?? (named && existsSync(named) ? named : join(homedir(), ".tauri", "shadui-ripgrep.key")));
    if (!existsSync(file)) fail(`There is no key at ${file}: --key <file> says where it is.`);
    say(`   ${file} and its password go into ${SLUG}'s Actions secrets (${KEY_SECRETS.join(", ")}),`);
    say("   so that the Release workflow signs what it builds. GitHub keeps them encrypted: nobody can read them back,");
    say("   and only this repository's workflows, which only you can change, use them. The file here stays as it is.");
    if (await goOn("   Go on?")) {
        const text = readFileSync(file, "utf8").trim();
        const fromVariable = process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
        let password = "";
        if (isEncryptedKey(text)) {
            if (fromVariable !== undefined) password = fromVariable;
            else if (interactive) password = askHidden("   Password of the update key");
            else fail("The key has a password and there is no terminal to ask in: set TAURI_SIGNING_PRIVATE_KEY_PASSWORD.");
        }
        // Here first: a wrong password or another key stops before anything is stored.
        const here = await keyState({ key: text, password, publicKey: updatePubkey() });
        if (here.state !== "ok") fail(`Nothing was stored: ${here.detail}.`);
        done("the password opens the key, and it is the key installed copies trust");
        putSecret(KEY_SECRETS[0], text);
        // A key whose password is empty must find no other password there: GitHub holds no empty secret, so an
        // old one is taken away.
        if (password) putSecret(KEY_SECRETS[1], password);
        else if (state.secrets.includes(KEY_SECRETS[1])) gh(["secret", "delete", KEY_SECRETS[1], "--repo", SLUG]);
        done(`${password ? KEY_SECRETS.join(" and ") : KEY_SECRETS[0]} ${password ? "are" : "is"} stored`);
        stored = true;
    } else say("   Left as it is.");
    say();
}

// ---- 2. The Partner Center application ---------------------------------------------------------------------------

if (!flag("--no-store") && state.store !== "ok") {
    say("2. The Microsoft Store");
    say("   The Store takes submissions from a Microsoft Entra application that Partner Center has been told about.");
    say("   Making it is yours to do, once, in Partner Center (docs/releasing.md has the same steps):");
    say(`     a. ${ACCOUNT}/tenantmanagement`);
    say("        Associate a Microsoft Entra directory with the account (Create Microsoft Entra ID makes one, free).");
    say(`     b. ${ACCOUNT}/usermanagement#apps  (signed in with that directory's account)`);
    say("        Add a Microsoft Entra application, a new one, with the Manager role.");
    say("     c. Open the application there: copy its Tenant ID and Client ID, then Add new key and copy the key.");
    say("        The key is shown once and acts for the whole account: keep it in your password manager.");
    const fromVariables = STORE_SECRETS.every((name) => process.env[name]?.trim());
    if (fromVariables || (await goOn("   Do you have the three values?"))) {
        const id = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
        const ask = async (name, prompt, hidden) => {
            const given = process.env[name]?.trim();
            if (given) return given;
            if (!interactive) fail(`There is no terminal to ask in: set ${name}.`);
            return (hidden ? askHidden(`   ${prompt}`) : await askLine(`   ${prompt}`)).trim();
        };
        const tenantId = await ask(STORE_SECRETS[0], "Tenant ID");
        const clientId = await ask(STORE_SECRETS[1], "Client ID");
        const shaped = [["tenant ID", tenantId], ["client ID", clientId]].find(([, given]) => !id.test(given));
        const clientSecret = shaped ? "" : await ask(STORE_SECRETS[2], "Key of the application (its client secret)", true);
        // Against the Store first: a sign-in and a read, which change nothing there.
        const here = shaped
            ? { state: "refused", detail: `the ${shaped[0]} should look like 00000000-0000-0000-0000-000000000000` }
            : await storeState({ tenantId, clientId, clientSecret, appId: storeIds.storeId });
        if (here.state === "ok") {
            done(here.detail);
            putSecret(STORE_SECRETS[0], tenantId);
            putSecret(STORE_SECRETS[1], clientId);
            putSecret(STORE_SECRETS[2], clientSecret);
            done(`${STORE_SECRETS.join(", ")} are stored`);
            stored = true;
        } else {
            trouble = `The Store's application wasn't stored: ${here.detail}.`;
            say(`   ✗ ${trouble}`);
        }
    } else say("   Left for later: run this again when you have them. Until then the Store's package waits for Partner Center's pages.");
    say();
}

// ---- 3. On GitHub ------------------------------------------------------------------------------------------------

if (stored) await check("Trying them where a release runs");
if (trouble) fail(`${trouble}
  Run this again with the right values.${state.key === "ok" ? " GitHub can sign Glint's updates meanwhile, and the Store's package waits for Partner Center's pages." : ""}`);
if (ready(state)) done(`GitHub can sign Glint's updates and send it to the Microsoft Store. ${NEXT}`);
else if (state.key === "ok") done(`GitHub can sign Glint's updates. ${NEXT}\n  The Store's package still waits for Partner Center's pages, until part 2 is done.`);
else fail(`A release still needs you: ${state.key === "unknown" ? "GitHub didn't say whether it can sign" : KEY_SAYS[state.key]}. Until then: npm run ship -- --local (it asks for the key's password).`);
