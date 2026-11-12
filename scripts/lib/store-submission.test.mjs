// Proves scripts/store.mjs and scripts/lib/store-submission.mjs without the Store: the pure parts on their own
// (what a submission has to become, the listing's limits, the ZIP written and read back, a package's identity),
// then the whole run against a stand-in for the Store that answers as the Microsoft Store submission API's pages
// say it does (the sign-in, the app, a submission copied from the last published one, the update, the upload in
// blocks to a storage address, the commit and its status) and checks every request it gets: the look that changes
// nothing, a submission with the listing, a pending submission that stops it, and one replaced.
//
// It can't prove the Store agrees with those pages: the first real run should be a look, then --draft.
//
// Usage: node scripts/lib/store-submission.test.mjs      (npm run test:store)

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deflateRawSync } from "node:zlib";
import { root, workspaceVersion } from "./project.mjs";
import { LIMITS, SCREENSHOT, listingProblems, packageIdentity, revise, screenshotName, statusLines, unzipEntry, zip } from "./store-submission.mjs";

let checks = 0;
const check = async (name, fn) => {
    await fn();
    checks++;
    console.log(`  ok  ${name}`);
};

const version = workspaceVersion();
const store = JSON.parse(readFileSync(join(root, "crates", "glint", "msix", "store.json"), "utf8"));
const listing = JSON.parse(readFileSync(join(root, "crates", "glint", "msix", "listing.json"), "utf8"));
const work = mkdtempSync(join(tmpdir(), "glint-store-test-"));
process.on("exit", () => rmSync(work, { recursive: true, force: true }));

/** The last published submission the stand-in Store starts from: an older package, two screenshots, a logo. */
const published = () => ({
    id: "100",
    applicationCategory: "UtilitiesAndTools_FileManagers",
    pricing: { trialPeriod: "NoFreeTrial", marketSpecificPricings: {}, sales: [], priceId: "Free", isAdvancedPricingModel: true },
    visibility: "Public",
    targetPublishMode: "Immediate",
    listings: {
        "en-us": {
            baseListing: {
                description: "The old description.",
                features: ["Search inside files with ripgrep, and replace"],
                releaseNotes: "New in 0.8.",
                images: [
                    { fileName: "old-1.png", fileStatus: "Uploaded", id: "1", description: "Old one", imageType: SCREENSHOT },
                    { fileName: "old-2.png", fileStatus: "Uploaded", id: "2", description: "Old two", imageType: SCREENSHOT },
                    { fileName: "logo.png", fileStatus: "Uploaded", id: "3", description: "", imageType: "Icon" },
                ],
                title: "Glint - File Explorer",
            },
            platformOverrides: {},
        },
    },
    notesForCertification: "",
    status: "Published",
    statusDetails: { errors: [], warnings: [], certificationReports: [] },
    applicationPackages: [
        { fileName: "Glint_0.9.5_x64.msix", fileStatus: "Uploaded", id: "10", version: "0.9.5.0", architecture: "x64", minimumDirectXVersion: "None", minimumSystemRam: "None" },
    ],
});

console.log("The parts:");

await check("a ZIP written here is read back entry for entry, and by Windows' own reader", () => {
    const entries = [
        { name: "Glint_1.2.3_x64.msix", data: Buffer.from("not really a package") },
        { name: "en-us-01-a picture.png", data: Buffer.alloc(70000, 7) },
        { name: "empty.txt", data: Buffer.alloc(0) },
    ];
    const archive = zip(entries);
    for (const e of entries) assert.deepEqual(unzipEntry(archive, e.name), e.data);
    assert.equal(unzipEntry(archive, "missing"), undefined);
    const file = join(work, "written.zip");
    writeFileSync(file, archive);
    const read = spawnSync(
        "powershell",
        ["-NoProfile", "-Command", `Add-Type -AssemblyName System.IO.Compression.FileSystem; $z = [IO.Compression.ZipFile]::OpenRead('${file}'); $z.Entries | ForEach-Object { '{0}|{1}' -f $_.FullName, $_.Length }; $z.Dispose()`],
        { encoding: "utf8" },
    );
    assert.equal(read.status, 0, read.stderr);
    assert.deepEqual(read.stdout.trim().split(/\r?\n/), entries.map((e) => `${e.name}|${e.data.length}`));
});

/** A small .msix stand-in: a ZIP whose AppxManifest.xml is deflated, as makeappx writes it. */
function fakeMsix({ name = store.name, publisher = store.publisher, packageVersion = `${version}.0` } = {}) {
    const manifest = Buffer.from(`<?xml version="1.0"?><Package><Identity Name="${name}" Publisher="${publisher}" Version="${packageVersion}" ProcessorArchitecture="x64" /></Package>`);
    const packed = deflateRawSync(manifest);
    const stored = zip([{ name: "AppxManifest.xml", data: packed }, { name: "Glint.exe", data: Buffer.alloc(5 * 1024 * 1024, 3) }]);
    // Mark the first entry deflated, with its real size, in both of its headers (the ZIP above stores everything).
    const central = stored.indexOf(Buffer.from([0x50, 0x4b, 0x01, 0x02]));
    stored.writeUInt16LE(8, 8);
    stored.writeUInt32LE(manifest.length, 22);
    stored.writeUInt16LE(8, central + 10);
    stored.writeUInt32LE(manifest.length, central + 24);
    return stored;
}

await check("a package says who it is: name, publisher, version, architecture", () => {
    assert.deepEqual(packageIdentity(fakeMsix()), { name: store.name, publisher: store.publisher, version: `${version}.0`, architecture: "x64" });
    assert.throws(() => packageIdentity(zip([{ name: "other.txt", data: Buffer.from("x") }])), /AppxManifest/);
    assert.throws(() => packageIdentity(Buffer.from("not a zip at all, just some bytes here")), /Not a ZIP/);
});

const real = join(root, "release", `v${version}`, `Glint_${version}_x64.msix`);
if (existsSync(real)) {
    await check(`this version's real package reads the same way (${version})`, () => {
        const id = packageIdentity(readFileSync(real));
        assert.equal(id.name, store.name);
        assert.equal(id.version, `${version}.0`);
    });
}

await check("the new package comes and the one it replaces goes; nothing else is touched, nor the input", () => {
    const before = published();
    const plan = revise(before, { packageName: "Glint_9.9.9_x64.msix", version: "9.9.9", listing });
    assert.deepEqual(before, published(), "the submission given is left as it was");
    const [old, added] = plan.submission.applicationPackages;
    assert.equal(old.fileStatus, "PendingDelete");
    assert.deepEqual(added, { fileName: "Glint_9.9.9_x64.msix", fileStatus: "PendingUpload", minimumDirectXVersion: "None", minimumSystemRam: "None" });
    assert.deepEqual(plan.files, [{ name: "Glint_9.9.9_x64.msix", source: "package" }]);
    // Without --listing the listing's pictures and words stay; what's new is for another version, so it stays too.
    assert.deepEqual(plan.submission.listings, before.listings);
    assert.match(plan.warnings.join(" "), /written for .*, not 9\.9\.9/);
    assert.deepEqual(plan.submission.pricing, before.pricing);
});

await check("a version the Store already has is refused; --keep-packages and another architecture's package stay", () => {
    assert.throws(() => revise(published(), { packageName: "x.msix", version: "0.9.5" }), /already has version 0\.9\.5/);
    const kept = revise(published(), { packageName: "new.msix", version: "1.0.0", keepPackages: true });
    assert.deepEqual(kept.submission.applicationPackages.map((p) => p.fileStatus), ["Uploaded", "PendingUpload"]);
    const arm = published();
    arm.applicationPackages[0].architecture = "arm64";
    assert.equal(revise(arm, { packageName: "new.msix", version: "1.0.0" }).submission.applicationPackages[0].fileStatus, "Uploaded");
});

await check("what's new goes on its own version only, and --listing swaps the screenshots and the words", () => {
    const plan = revise(published(), { packageName: "p.msix", version: listing.whatsNew.version, listing, withListing: true });
    const base = plan.submission.listings["en-us"].baseListing;
    assert.equal(base.releaseNotes, listing.whatsNew["en-us"]);
    assert.equal(base.description, listing["en-us"].description);
    assert.deepEqual(base.features, listing["en-us"].features);
    const shots = base.images.filter((i) => i.imageType === SCREENSHOT);
    assert.deepEqual(shots.slice(0, 2).map((i) => i.fileStatus), ["PendingDelete", "PendingDelete"]);
    const added = shots.slice(2);
    assert.equal(added.length, listing["en-us"].screenshots.length);
    added.forEach((image, i) => {
        const wanted = listing["en-us"].screenshots[i];
        assert.deepEqual(image, { fileName: screenshotName("en-us", i, wanted.file), fileStatus: "PendingUpload", description: wanted.caption, imageType: SCREENSHOT });
    });
    assert.equal(base.images.find((i) => i.imageType === "Icon").fileStatus, "Uploaded", "the logo is not a screenshot");
    assert.equal(plan.files.length, 1 + added.length);
    assert.equal(new Set(plan.files.map((f) => f.name)).size, plan.files.length, "every file has a name of its own");
    assert.throws(() => revise(published(), { packageName: "p.msix", version: "1.0.0", listing: { "fr-fr": { features: ["x"] } }, withListing: true }), /no "fr-fr"/);
});

await check("the listing file in the repository is within the Store's limits, and its screenshots are there", () => {
    assert.deepEqual(listingProblems(listing, (file) => existsSync(join(root, file))), []);
    assert.ok(listing["en-us"].screenshots.length <= LIMITS.screenshots);
    const long = { "en-us": { features: ["x".repeat(201)], screenshots: [{ file: "a.jpg", caption: "y".repeat(201) }] }, whatsNew: { version: "1.0.0", "en-us": "z".repeat(1501) } };
    assert.equal(listingProblems(long).length, 4);
    assert.deepEqual(statusLines({ errors: [{ code: "InvalidState", details: "No." }], warnings: [], certificationReports: [] }), ["  ✗ InvalidState: No."]);
});

// ---- The whole run, against a stand-in Store --------------------------------------------------------------

/** A Store that answers as the API's pages say, and keeps what it was asked. */
function standIn({ pending = false } = {}) {
    const state = { requests: [], submissions: { 100: published() }, pendingId: pending ? "150" : undefined, blocks: new Map(), blob: undefined, statusReads: 0 };
    if (pending) state.submissions[150] = { ...published(), id: "150", status: "PendingCommit" };
    const server = createServer((req, res) => {
        const chunks = [];
        req.on("data", (c) => chunks.push(c));
        req.on("end", () => {
            const body = Buffer.concat(chunks);
            const url = new URL(req.url, "http://stand-in");
            const path = url.pathname;
            const send = (status, json) => {
                res.writeHead(status, { "Content-Type": "application/json" });
                res.end(json === undefined ? "" : JSON.stringify(json));
            };
            const note = (what) => state.requests.push(what);
            if (req.method === "POST" && path === "/tenant-1/oauth2/token") {
                const form = new URLSearchParams(body.toString());
                note("sign in");
                if (form.get("client_secret") !== "the-key") return send(401, { error: "invalid_client", error_description: "AADSTS7000215: Invalid client secret provided.\r\nTrace ID: x" });
                assert.equal(form.get("grant_type"), "client_credentials");
                assert.equal(form.get("client_id"), "client-1");
                assert.equal(form.get("resource"), "https://manage.devcenter.microsoft.com");
                return send(200, { token_type: "Bearer", access_token: "token-1" });
            }
            if (path.startsWith("/blob/")) {
                // The storage address: blocks, then the list that makes them the blob. No Authorization here.
                assert.equal(req.headers.authorization, undefined);
                assert.equal(url.searchParams.get("sig"), "a+b/c=", "the address's own signature arrives as it was given");
                if (url.searchParams.get("comp") === "block") {
                    state.blocks.set(url.searchParams.get("blockid"), body);
                    note("block");
                    return send(201);
                }
                if (url.searchParams.get("comp") === "blocklist") {
                    const ids = [...body.toString().matchAll(/<Latest>([^<]+)<\/Latest>/g)].map((m) => m[1]);
                    state.blob = Buffer.concat(ids.map((id) => state.blocks.get(id)));
                    note("block list");
                    return send(201);
                }
            }
            if (!path.startsWith(`/v1.0/my/applications/${store.storeId}`)) return send(404, { code: "NotFound", message: path });
            if (req.headers.authorization !== "Bearer token-1") return send(401, { code: "Unauthorized", message: "no token" });
            const rest = path.slice(`/v1.0/my/applications/${store.storeId}`.length);
            const match = rest.match(/^\/submissions\/(\d+)(\/status|\/commit)?$/);
            if (req.method === "GET" && rest === "") {
                note("app");
                return send(200, {
                    id: store.storeId,
                    primaryName: "Glint - File Explorer",
                    lastPublishedApplicationSubmission: { id: "100" },
                    ...(state.pendingId ? { pendingApplicationSubmission: { id: state.pendingId } } : {}),
                });
            }
            if (req.method === "POST" && rest === "/submissions") {
                note("create");
                if (state.pendingId) return send(409, { code: "InvalidOperation", message: "A submission is already pending." });
                state.pendingId = "200";
                state.submissions[200] = { ...published(), id: "200", status: "PendingCommit", fileUploadUrl: `http://127.0.0.1:${server.address().port}/blob/200?sv=2014-02-14&sr=b&sig=a%2Bb%2Fc%3D&se=2099-01-01T00:00:00Z&sp=rwl` };
                return send(201, state.submissions[200]);
            }
            if (match && state.submissions[match[1]]) {
                const [, id, sub] = match;
                if (req.method === "GET" && !sub) return note(`get ${id}`), send(200, state.submissions[id]);
                if (req.method === "GET" && sub === "/status") {
                    note(`status ${id}`);
                    const s = state.submissions[id];
                    if (s.status === "CommitStarted" && ++state.statusReads >= 2) s.status = "PreProcessing";
                    return send(200, { status: s.status, statusDetails: s.statusDetails });
                }
                if (req.method === "PUT" && !sub) {
                    note(`update ${id}`);
                    state.submissions[id] = { ...JSON.parse(body.toString()), status: "PendingCommit" };
                    state.updated = JSON.parse(body.toString());
                    return send(200, state.submissions[id]);
                }
                if (req.method === "POST" && sub === "/commit") {
                    note(`commit ${id}`);
                    assert.ok(state.blob, "the files are uploaded before the commit");
                    state.submissions[id].status = "CommitStarted";
                    return send(202, { status: "CommitStarted" });
                }
                if (req.method === "DELETE" && !sub) {
                    note(`delete ${id}`);
                    delete state.submissions[id];
                    if (state.pendingId === id) state.pendingId = undefined;
                    return send(204);
                }
            }
            return send(404, { code: "NotFound", message: `${req.method} ${path}` });
        });
    });
    return new Promise((ready) => server.listen(0, "127.0.0.1", () => ready({ state, server, address: `http://127.0.0.1:${server.address().port}` })));
}

const msixFile = join(work, `Glint_${version}_x64.msix`);
const msixBytes = fakeMsix();
writeFileSync(msixFile, msixBytes);

/** Runs scripts/store.mjs against a stand-in, and gives its exit code and what it printed. */
function run(address, args, env = {}) {
    return new Promise((done) => {
        const child = spawn(process.execPath, [join(root, "scripts", "store.mjs"), ...args, "--msix", msixFile], {
            env: {
                ...process.env,
                GLINT_STORE_API: address,
                GLINT_STORE_LOGIN: address,
                GLINT_STORE_POLL_MS: "20",
                PARTNER_CENTER_TENANT_ID: "tenant-1",
                PARTNER_CENTER_CLIENT_ID: "client-1",
                PARTNER_CENTER_CLIENT_SECRET: "the-key",
                ...env,
            },
        });
        let out = "";
        child.stdout.on("data", (d) => (out += d));
        child.stderr.on("data", (d) => (out += d));
        child.on("close", (code) => done({ code, out }));
    });
}

console.log("The whole run, against a stand-in Store:");
const notesAreForThisVersion = listing.whatsNew.version === version;

await check("a look signs in, reads, says what would change, and changes nothing", async () => {
    const { state, server, address } = await standIn();
    const { code, out } = await run(address, ["--listing"]);
    server.close();
    assert.equal(code, 0, out);
    assert.deepEqual(state.requests, ["sign in", "app", "get 100"]);
    assert.match(out, /- package Glint_0\.9\.5_x64\.msix/);
    assert.match(out, new RegExp(`\\+ package Glint_${version.replaceAll(".", "\\.")}_x64\\.msix`));
    assert.match(out, /\+ en-us: screenshot "Any file on any drive, in milliseconds"/);
    assert.match(out, /Nothing was changed/);
});

await check("--submit --listing: the copy is revised, its files travel in one ZIP in blocks, then the commit is taken", async () => {
    const { state, server, address } = await standIn();
    const { code, out } = await run(address, ["--submit", "--listing"]);
    server.close();
    assert.equal(code, 0, out);
    const blocks = state.requests.filter((r) => r === "block").length;
    assert.ok(blocks >= 2, `the upload went in ${blocks} block(s)`);
    assert.deepEqual(
        state.requests.filter((r) => r !== "block"),
        ["sign in", "app", "create", "update 200", "block list", "commit 200", "status 200", "status 200"],
    );
    const sent = state.updated;
    assert.deepEqual(sent.applicationPackages.map((p) => [p.fileName, p.fileStatus]), [["Glint_0.9.5_x64.msix", "PendingDelete"], [`Glint_${version}_x64.msix`, "PendingUpload"]]);
    const base = sent.listings["en-us"].baseListing;
    const coming = base.images.filter((i) => i.fileStatus === "PendingUpload");
    assert.equal(coming.length, listing["en-us"].screenshots.length);
    assert.equal(base.images.filter((i) => i.fileStatus === "PendingDelete").length, 2);
    if (notesAreForThisVersion) assert.equal(base.releaseNotes, listing.whatsNew["en-us"]);
    assert.equal(sent.pricing.priceId, "Free", "what the copy came with goes back as it came");
    // The ZIP the stand-in was given holds the package and every screenshot, byte for byte, under the names sent.
    assert.deepEqual(unzipEntry(state.blob, `Glint_${version}_x64.msix`), msixBytes);
    coming.forEach((image, i) => {
        assert.deepEqual(unzipEntry(state.blob, image.fileName), readFileSync(join(root, listing["en-us"].screenshots[i].file)), image.fileName);
    });
    assert.match(out, /The Store has version .*: PreProcessing/);
});

await check("a pending submission stops --draft; --replace-pending deletes it and starts over, uncommitted", async () => {
    const first = await standIn({ pending: true });
    const stopped = await run(first.address, ["--draft"]);
    first.server.close();
    assert.equal(stopped.code, 1);
    assert.match(stopped.out, /already pending \(PendingCommit\)/);
    assert.deepEqual(first.state.requests, ["sign in", "app", "status 150"]);

    const second = await standIn({ pending: true });
    const replaced = await run(second.address, ["--draft", "--replace-pending"]);
    second.server.close();
    assert.equal(replaced.code, 0, replaced.out);
    assert.deepEqual(second.state.requests.filter((r) => r !== "block"), ["sign in", "app", "status 150", "delete 150", "create", "update 200", "block list"]);
    assert.match(replaced.out, /not committed/);
    assert.equal(second.state.submissions[200].status, "PendingCommit");
});

await check("--commit, --status and --discard act on the pending submission; a wrong key and a wrong package say why", async () => {
    const a = await standIn({ pending: true });
    a.state.blob = Buffer.alloc(1);
    const committed = await run(a.address, ["--commit"]);
    assert.equal(committed.code, 0, committed.out);
    assert.deepEqual(a.state.requests, ["sign in", "app", "status 150", "commit 150", "status 150", "status 150"]);
    const status = await run(a.address, ["--status"]);
    assert.match(status.out, /The pending submission \(150\): PreProcessing/);
    a.server.close();

    const b = await standIn({ pending: true });
    const discarded = await run(b.address, ["--discard"]);
    assert.equal(discarded.code, 0, discarded.out);
    assert.equal(b.state.pendingId, undefined);
    const wrongKey = await run(b.address, [], { PARTNER_CENTER_CLIENT_SECRET: "another" });
    assert.equal(wrongKey.code, 1);
    assert.match(wrongKey.out, /Invalid client secret/);
    b.server.close();

    const test = join(work, "test-package.msix");
    writeFileSync(test, fakeMsix({ name: `${store.name}_test` }));
    const wrongPackage = spawnSync(process.execPath, [join(root, "scripts", "store.mjs"), "--submit", "--msix", test], { encoding: "utf8" });
    assert.equal(wrongPackage.status, 1);
    assert.match(wrongPackage.stderr, /isn't this version's Store package/);
});

console.log(`\n${checks} checks passed: store.mjs asks the Store what the submission API's pages say to ask, in their order.`);
