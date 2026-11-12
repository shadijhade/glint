// Sends a version of Glint to the Microsoft Store without Partner Center's pages: the package `npm run msix`
// made, what's new in it and, with --listing, the Store listing itself (its screenshots and their captions, the
// description and the features) from crates/glint/msix/listing.json. It talks to the Microsoft Store submission
// API (scripts/lib/store-submission.mjs) as the Microsoft Entra application Partner Center has been told about
// (docs/releasing.md, The Microsoft Store: once, in Partner Center's account settings).
//
// Usage: npm run store [-- --draft | --submit | --commit | --status | --discard]
//                         [--listing] [--keep-packages] [--replace-pending] [--msix <file>] [--out <folder>]
//   (no mode)          looks only: signs in, reads the app and says what a submission would change. Changes nothing.
//   --draft            makes the submission and uploads its files, not committed: read it in Partner Center
//                      (read it, don't change it there: the API can't finish one changed by hand), then --commit
//   --submit           --draft and --commit in one: the Store takes it from there (certification, then publishing)
//   --commit           commits the submission --draft made
//   --status           where the pending submission is (the last published one when none is pending)
//   --discard          deletes the pending submission
//   --listing          the Store listing too: screenshots and captions, description and features (listing.json)
//   --keep-packages    leaves the packages the Store has; by default the new one replaces those of its architecture
//   --replace-pending  with --draft or --submit: deletes a submission that is pending first (default: stops)
//   --msix <file>      the package; default release/v<version>/Glint_<version>_x64.msix (--out names that folder)
//
// The application's ids and key come from PARTNER_CENTER_TENANT_ID, PARTNER_CENTER_CLIENT_ID and
// PARTNER_CENTER_CLIENT_SECRET (the names Microsoft's own tools use); the key is asked for when it isn't set.
// Nothing is written anywhere but the Store, and nothing is sent before the mode says so.

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";
import { root, workspaceVersion } from "./lib/project.mjs";
import {
    SCREENSHOT,
    StoreError,
    committing,
    failed,
    listingProblems,
    packageIdentity,
    revise,
    signIn,
    statusLines,
    storeClient,
    upload,
    zip,
} from "./lib/store-submission.mjs";

const args = process.argv.slice(2);
const MODES = ["--draft", "--submit", "--commit", "--status", "--discard"];
const SWITCHES = ["--listing", "--keep-packages", "--replace-pending"];
const VALUES = ["--msix", "--out"];
const has = (name) => args.includes(name);
const option = (name) => (has(name) ? args[args.indexOf(name) + 1] : undefined);

const fail = (message) => {
    console.error(`\n✗ ${message}`);
    process.exit(1);
};

for (let i = 0; i < args.length; i++) {
    if (VALUES.includes(args[i])) i++;
    else if (!MODES.includes(args[i]) && !SWITCHES.includes(args[i])) fail(`Unknown option ${args[i]} (the top of scripts/store.mjs lists them).`);
}
const chosen = MODES.filter(has);
if (chosen.length > 1) fail(`One of ${chosen.join(", ")} at a time.`);
const mode = chosen[0]?.slice(2) ?? "look";
const sends = mode === "draft" || mode === "submit";

const version = workspaceVersion();
const store = JSON.parse(readFileSync(join(root, "crates", "glint", "msix", "store.json"), "utf8"));
if (!store.storeId) fail("crates/glint/msix/store.json has no storeId (the app's Store ID, from Partner Center's Product identity).");
const partnerCenter = `https://partner.microsoft.com/dashboard/products/${store.storeId}/overview`;

// The package: read and checked before anything is asked of the Store.
const msixFile = resolve(option("--msix") ?? join(option("--out") ?? join(root, "release", `v${version}`), `Glint_${version}_x64.msix`));
let msix;
let identity = { architecture: "x64" };
if (existsSync(msixFile)) {
    msix = readFileSync(msixFile);
    identity = packageIdentity(msix);
    const wrong = [];
    if (identity.name !== store.name) wrong.push(`its name is ${identity.name}, not ${store.name} (a test package?)`);
    if (identity.publisher !== store.publisher) wrong.push(`its publisher is ${identity.publisher}, not ${store.publisher}`);
    if (identity.version !== `${version}.0`) wrong.push(`its version is ${identity.version}, not ${version}.0`);
    if (wrong.length) fail(`${msixFile} isn't this version's Store package: ${wrong.join("; ")}. npm run msix makes it.`);
} else if (sends) {
    fail(`No package at ${msixFile}: npm run msix makes it.`);
}

// The listing file: what's new for this version, and the listing's own content for --listing.
const listingFile = join(root, "crates", "glint", "msix", "listing.json");
const listing = existsSync(listingFile) ? JSON.parse(readFileSync(listingFile, "utf8")) : {};
const problems = listingProblems(listing, (file) => existsSync(join(root, file)));
if (problems.length) fail(`crates/glint/msix/listing.json:\n  ${problems.join("\n  ")}`);
if (has("--listing") && !existsSync(listingFile)) fail("--listing needs crates/glint/msix/listing.json.");

/** Reads a line without echoing it, from the person running this. */
function askHidden(prompt) {
    const script = [
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8",
        `$p = Read-Host -AsSecureString '${prompt}'`,
        "$b = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($p)",
        "try { [Runtime.InteropServices.Marshal]::PtrToStringBSTR($b) } finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($b) }",
    ].join("; ");
    const asked = spawnSync("powershell", ["-NoProfile", "-Command", script], { stdio: ["inherit", "pipe", "inherit"], encoding: "utf8" });
    if (asked.error || asked.status !== 0) fail("Couldn't ask for the application's key: set PARTNER_CENTER_CLIENT_SECRET.");
    return asked.stdout.replace(/\r?\n$/, "");
}

function credentials() {
    const tenantId = process.env.PARTNER_CENTER_TENANT_ID?.trim();
    const clientId = process.env.PARTNER_CENTER_CLIENT_ID?.trim();
    if (!tenantId || !clientId) {
        fail(
            "Set PARTNER_CENTER_TENANT_ID and PARTNER_CENTER_CLIENT_ID: the Microsoft Entra application Partner Center knows\n" +
                "  (Account settings → User management → Microsoft Entra applications, with the Manager role).\n" +
                "  docs/releasing.md, The Microsoft Store, says how to make it, once.",
        );
    }
    const clientSecret = process.env.PARTNER_CENTER_CLIENT_SECRET?.trim() || askHidden("Key of the Partner Center application (its client secret)");
    if (!clientSecret) fail("The application's key is empty.");
    return { tenantId, clientId, clientSecret };
}

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const POLL = Number(process.env.GLINT_STORE_POLL_MS ?? 15000);

/** Tells what a submission holds: its packages and, per language, its screenshots. */
function describe(submission) {
    for (const p of submission.applicationPackages ?? []) console.log(`  package ${p.fileName} ${p.version ?? ""} ${p.architecture ?? ""} (${p.fileStatus})`);
    for (const [lang, l] of Object.entries(submission.listings ?? {})) {
        const shots = (l.baseListing?.images ?? []).filter((i) => i.imageType === SCREENSHOT && i.fileStatus !== "PendingDelete");
        console.log(`  ${lang}: ${shots.length} screenshot${shots.length === 1 ? "" : "s"}, ${l.baseListing?.features?.length ?? 0} features, a description of ${l.baseListing?.description?.length ?? 0} characters`);
    }
}

/** Commits a submission and waits until the Store has taken the commit (or refused it). */
async function commit(api, id) {
    await api.commit(id);
    console.log("Committed. Waiting for the Store to take it…");
    let state;
    for (let waited = 0; waited < 20 * 60 * 1000; waited += POLL) {
        state = await api.status(id);
        if (!committing(state.status)) break;
        await sleep(POLL);
    }
    for (const line of statusLines(state.statusDetails)) console.log(line);
    if (failed(state.status)) fail(`The Store refused the submission: ${state.status}. It is still there to look at (${partnerCenter}); npm run store -- --discard removes it.`);
    if (committing(state.status)) console.log("! The Store is still taking the commit: npm run store -- --status says where it is.");
    else console.log(`\n✓ The Store has version ${version}: ${state.status}. Certification comes next (hours, sometimes days), then it publishes by itself.`);
    console.log(`  npm run store -- --status says where it is; so does ${partnerCenter}`);
}

let made;
try {
    const api = storeClient(await signIn(credentials()), store.storeId);
    const app = await api.app();
    const pending = app.pendingApplicationSubmission?.id;
    const last = app.lastPublishedApplicationSubmission?.id;
    console.log(`${app.primaryName ?? store.displayName} (${app.id ?? store.storeId})${pending ? ", with a submission pending" : ""}`);

    if (mode === "status") {
        const id = pending ?? last;
        if (!id) fail("The app has no submission yet: the first one is made in Partner Center.");
        const state = await api.status(id);
        console.log(`${pending ? "The pending submission" : "The last published submission"} (${id}): ${state.status}`);
        for (const line of statusLines(state.statusDetails)) console.log(line);
        describe(await api.submission(id));
    } else if (mode === "discard") {
        if (!pending) fail("No submission is pending.");
        describe(await api.submission(pending));
        await api.remove(pending);
        console.log(`✓ Deleted the pending submission ${pending}.`);
    } else if (mode === "commit") {
        if (!pending) fail("No submission is pending: --draft makes one.");
        const state = await api.status(pending);
        if (state.status !== "PendingCommit") fail(`The pending submission is ${state.status}, not waiting for a commit.`);
        await commit(api, pending);
    } else {
        if (!last) fail("The app has no published submission to start from: the first one is made in Partner Center.");
        const options = {
            packageName: basename(msixFile),
            version,
            architecture: identity.architecture ?? "x64",
            keepPackages: has("--keep-packages"),
            listing,
            withListing: has("--listing"),
        };
        if (mode === "look") {
            const published = await api.submission(last);
            console.log("The Store has:");
            describe(published);
            const plan = revise(published, options);
            console.log(`\nA submission of ${version} would change:`);
            for (const line of plan.changes) console.log(line);
            for (const warning of plan.warnings) console.log(`! ${warning}`);
            if (!msix) console.log(`! No package at ${msixFile} yet: npm run msix makes it.`);
            if (!has("--listing") && existsSync(listingFile)) console.log("  (the listing's screenshots, description and features only with --listing)");
            if (pending) console.log(`! A submission is pending (${(await api.status(pending)).status}): --draft and --submit stop at it unless --replace-pending.`);
            console.log("\nNothing was changed. --draft makes it without committing; --submit sends it.");
        } else {
            if (pending) {
                const state = await api.status(pending);
                if (!has("--replace-pending")) fail(`A submission is already pending (${state.status}): finish it, or run again with --replace-pending to delete it and start over.`);
                await api.remove(pending);
                console.log(`Deleted the pending submission ${pending} (${state.status}).`);
            }
            const created = await api.create();
            made = created.id;
            const plan = revise(created, options);
            console.log(`Submission ${created.id}, a copy of the last published one, becomes:`);
            for (const line of plan.changes) console.log(line);
            for (const warning of plan.warnings) console.log(`! ${warning}`);
            await api.update(created.id, plan.submission);
            const archive = zip(plan.files.map((f) => ({ name: f.name, data: f.source === "package" ? msix : readFileSync(join(root, f.source)) })));
            let said = 0;
            await upload(created.fileUploadUrl, archive, (sent, total) => {
                const percent = Math.floor((sent / total) * 100);
                if (percent >= said + 25 || sent === total) console.log(`  uploaded ${(sent / 1048576).toFixed(0)} of ${(total / 1048576).toFixed(0)} MB`), (said = percent);
            });
            if (mode === "draft") {
                console.log(`\n✓ Submission ${created.id} is in Partner Center, not committed: ${partnerCenter}`);
                console.log("  Read it there without changing it, then: npm run store -- --commit   (or --discard)");
            } else {
                await commit(api, created.id);
            }
        }
    }
} catch (error) {
    const hint = made ? `\n  Submission ${made} was made and is still pending: npm run store -- --discard removes it.` : "";
    if (error instanceof StoreError && error.status === 401) fail(`${error.message}\n  The application's ids or key aren't right (or the key has expired: Partner Center makes a new one).${hint}`);
    if (error instanceof StoreError && error.status === 403) fail(`${error.message}\n  The application needs the Manager role in Partner Center (Account settings → User management).${hint}`);
    fail(`${error.message}${hint}`);
}
