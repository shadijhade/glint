// The Microsoft Store submission API, as much of it as sending a version of Glint needs (scripts/store.mjs):
// signing in as the Microsoft Entra application Partner Center knows, reading the app and its submissions,
// turning a copy of the last published submission into the new one (the package swapped, and the listing's
// screenshots and words when asked), the ZIP the new files travel in, and its upload to the storage address the
// Store hands out. Nothing here is from npm: Node's fetch, zlib and crypto.
//
// The API: https://learn.microsoft.com/windows/uwp/monetize/create-and-manage-submissions-using-windows-store-services
// and .../manage-app-submissions (the resources and their values). What matters, in its words:
// - a new submission is a copy of the last published one; what it has to say is PUT back whole;
// - a file is named in the submission with fileStatus PendingUpload and travels in one ZIP, by that name;
//   what is to go is marked PendingDelete; everything else stays as it came (Uploaded);
// - a submission made here is finished here: one changed by hand in Partner Center can no longer be changed
//   or committed through the API.

import { crc32, inflateRawSync } from "node:zlib";

/** Where the API and the sign-in are (the variables are for the tests, which stand in for both). */
export const API = () => (process.env.GLINT_STORE_API ?? "https://manage.devcenter.microsoft.com").replace(/\/$/, "");
export const LOGIN = () => (process.env.GLINT_STORE_LOGIN ?? "https://login.microsoftonline.com").replace(/\/$/, "");
/** What the token is for (the API's own name for itself, the same whoever stands in for it). */
const RESOURCE = "https://manage.devcenter.microsoft.com";

/** The Store's limits on a listing's words (Partner Center refuses longer ones at the commit, a day later). */
export const LIMITS = { caption: 200, releaseNotes: 1500, description: 10000, feature: 200, features: 20, screenshots: 10 };

/** A desktop screenshot's type in a listing's images. */
export const SCREENSHOT = "Screenshot";

/** An API error: what was asked, the status, and what the Store said. */
export class StoreError extends Error {
    constructor(what, status, body) {
        const said = typeof body === "string" ? body : (body?.message ?? JSON.stringify(body));
        super(`${what}: ${status}${said ? ` ${said}` : ""}`);
        this.status = status;
        this.body = body;
    }
}

/** Signs in with the application's tenant, id and key; gives the token (good for an hour). */
export async function signIn({ tenantId, clientId, clientSecret }) {
    const body = new URLSearchParams({ grant_type: "client_credentials", client_id: clientId, client_secret: clientSecret, resource: RESOURCE });
    const res = await fetch(`${LOGIN()}/${encodeURIComponent(tenantId)}/oauth2/token`, {
        method: "POST",
        headers: { "Content-Type": "application/x-www-form-urlencoded; charset=utf-8" },
        body,
    });
    const text = await res.text();
    let json;
    try {
        json = JSON.parse(text);
    } catch {
        json = undefined;
    }
    if (!res.ok || !json?.access_token) {
        // Entra says why in error_description ("AADSTS7000215: Invalid client secret provided…").
        throw new StoreError("Signing in to Microsoft Entra", res.status, json?.error_description?.split("\r\n")[0] ?? text.slice(0, 300));
    }
    return json.access_token;
}

/** The API's methods on one app, with a token. */
export function storeClient(token, appId) {
    const app = `${API()}/v1.0/my/applications/${encodeURIComponent(appId)}`;
    const call = async (what, method, url, body) => {
        const res = await fetch(url, {
            method,
            headers: { Authorization: `Bearer ${token}`, ...(body === undefined ? {} : { "Content-Type": "application/json; charset=utf-8" }) },
            body: body === undefined ? undefined : JSON.stringify(body),
        });
        const text = await res.text();
        let json;
        try {
            json = text ? JSON.parse(text) : undefined;
        } catch {
            json = undefined;
        }
        if (!res.ok) throw new StoreError(what, res.status, json ?? text.slice(0, 500));
        return json;
    };
    return {
        app: () => call("Reading the app", "GET", app),
        submission: (id) => call("Reading the submission", "GET", `${app}/submissions/${id}`),
        status: (id) => call("Reading the submission's status", "GET", `${app}/submissions/${id}/status`),
        create: () => call("Creating the submission", "POST", `${app}/submissions`),
        update: (id, submission) => call("Updating the submission", "PUT", `${app}/submissions/${id}`, submission),
        commit: (id) => call("Committing the submission", "POST", `${app}/submissions/${id}/commit`),
        remove: (id) => call("Deleting the submission", "DELETE", `${app}/submissions/${id}`),
    };
}

/**
 * Checks a listing file's words against the Store's limits and gives the problems, as sentences (none: []).
 * `listing` is crates/glint/msix/listing.json, read; `fileExists(path)` says whether a screenshot is there.
 */
export function listingProblems(listing, fileExists = () => true) {
    const problems = [];
    const languages = Object.keys(listing).filter((key) => key !== "whatsNew");
    for (const lang of languages) {
        const l = listing[lang];
        if (typeof l.description === "string" && l.description.length > LIMITS.description) problems.push(`${lang}: the description is ${l.description.length} characters (at most ${LIMITS.description}).`);
        if (l.features) {
            if (l.features.length > LIMITS.features) problems.push(`${lang}: ${l.features.length} features (at most ${LIMITS.features}).`);
            for (const f of l.features) if (f.length > LIMITS.feature) problems.push(`${lang}: a feature is ${f.length} characters (at most ${LIMITS.feature}): "${f.slice(0, 40)}…"`);
        }
        if (l.screenshots) {
            if (!l.screenshots.length) problems.push(`${lang}: no screenshots (a listing needs one).`);
            if (l.screenshots.length > LIMITS.screenshots) problems.push(`${lang}: ${l.screenshots.length} screenshots (at most ${LIMITS.screenshots}).`);
            for (const s of l.screenshots) {
                if (!/\.png$/i.test(s.file)) problems.push(`${lang}: ${s.file} isn't a .png.`);
                else if (!fileExists(s.file)) problems.push(`${lang}: ${s.file} isn't there.`);
                if ((s.caption ?? "").length > LIMITS.caption) problems.push(`${lang}: the caption of ${s.file} is ${s.caption.length} characters (at most ${LIMITS.caption}).`);
            }
        }
    }
    for (const [lang, text] of Object.entries(listing.whatsNew ?? {})) {
        if (lang !== "version" && text.length > LIMITS.releaseNotes) problems.push(`${lang}: what's new is ${text.length} characters (at most ${LIMITS.releaseNotes}).`);
    }
    return problems;
}

/** The name a screenshot travels under in the ZIP: its place in the listing, its language and its own name. */
export const screenshotName = (lang, index, file) => `${lang}-${String(index + 1).padStart(2, "0")}-${file.split(/[\\/]/).pop()}`;

const live = (item) => item.fileStatus !== "PendingDelete";
const sameVersion = (a, b) => {
    const parts = (v) => `${v}`.split(".").map(Number).concat(0, 0, 0, 0).slice(0, 4).join(".");
    return parts(a) === parts(b);
};

/**
 * What a copy of the last published submission has to become to carry this version.
 *
 * - The package: `packageName` is added (PendingUpload), and the packages it replaces (the same architecture)
 *   are marked PendingDelete, unless `keepPackages`.
 * - What's new: `listing.whatsNew[lang]` when its `version` is this one (a note written for another version is
 *   not put on this one: the old note stays, and the answer says so).
 * - With `withListing`, for each language in `listing`: the description and the features when given, and the
 *   screenshots: the ones there go (PendingDelete) and the listed ones come, in order, with their captions.
 *
 * Returns the revised submission (the input is not touched), the lines that say what changes, the files the ZIP
 * must hold as { name, source } and warnings. Throws when the Store already has this version, or the listing
 * names a language the Store's listing doesn't have.
 */
export function revise(submission, { packageName, version, architecture = "x64", keepPackages = false, listing = {}, withListing = false }) {
    const revised = structuredClone(submission);
    const changes = [];
    const warnings = [];
    const files = [{ name: packageName, source: "package" }];

    const packages = (revised.applicationPackages ??= []);
    const already = packages.find((p) => live(p) && sameVersion(p.version ?? "", `${version}.0`));
    if (already) throw new Error(`The Store's last submission already has version ${version} (${already.fileName}): a new one needs a higher version.`);
    for (const p of packages.filter(live)) {
        const same = `${p.architecture ?? ""}`.toLowerCase() === architecture.toLowerCase();
        if (keepPackages || !same) {
            changes.push(`  = package ${p.fileName} ${p.version ?? ""} ${p.architecture ?? ""} stays`);
        } else {
            p.fileStatus = "PendingDelete";
            changes.push(`  - package ${p.fileName} ${p.version ?? ""} ${p.architecture ?? ""}`);
        }
    }
    packages.push({ fileName: packageName, fileStatus: "PendingUpload", minimumDirectXVersion: "None", minimumSystemRam: "None" });
    changes.push(`  + package ${packageName} ${version}.0 ${architecture}`);

    const listed = revised.listings ?? {};
    const notes = listing.whatsNew ?? {};
    for (const [lang, text] of Object.entries(notes)) {
        if (lang === "version") continue;
        const base = listed[lang]?.baseListing;
        if (!base) throw new Error(`The Store's listing has no "${lang}" (it has ${Object.keys(listed).join(", ") || "none"}): what's new can't be put there.`);
        if (notes.version !== version) {
            warnings.push(`What's new in the listing file is written for ${notes.version ?? "no version"}, not ${version}: the Store keeps the note it has.`);
            break;
        }
        if (base.releaseNotes !== text) {
            base.releaseNotes = text;
            changes.push(`  ~ ${lang}: what's new in this version (${text.length} characters)`);
        }
    }

    if (withListing) {
        const languages = Object.keys(listing).filter((key) => key !== "whatsNew");
        for (const lang of languages) {
            const base = listed[lang]?.baseListing;
            if (!base) throw new Error(`The Store's listing has no "${lang}" (it has ${Object.keys(listed).join(", ") || "none"}).`);
            const l = listing[lang];
            if (typeof l.description === "string" && base.description !== l.description) {
                base.description = l.description;
                changes.push(`  ~ ${lang}: the description (${l.description.length} characters)`);
            }
            if (l.features && JSON.stringify(base.features ?? []) !== JSON.stringify(l.features)) {
                base.features = [...l.features];
                changes.push(`  ~ ${lang}: the features (${l.features.length})`);
            }
            if (l.screenshots) {
                const images = (base.images ??= []);
                for (const image of images.filter((i) => i.imageType === SCREENSHOT && live(i))) {
                    image.fileStatus = "PendingDelete";
                    changes.push(`  - ${lang}: screenshot "${image.description ?? ""}" (${image.fileName})`);
                }
                l.screenshots.forEach((s, index) => {
                    const name = screenshotName(lang, index, s.file);
                    images.push({ fileName: name, fileStatus: "PendingUpload", description: s.caption ?? "", imageType: SCREENSHOT });
                    files.push({ name, source: s.file });
                    changes.push(`  + ${lang}: screenshot "${s.caption ?? ""}" (${s.file})`);
                });
            }
        }
        for (const lang of Object.keys(listed).filter((lang) => !languages.includes(lang))) {
            warnings.push(`The Store's listing in "${lang}" is not in the listing file: it stays as it is.`);
        }
    }
    return { submission: revised, changes, files, warnings };
}

// ---- The ZIP ---------------------------------------------------------------------------------------------

/**
 * A ZIP of `entries` ([{ name, data: Buffer }]), stored, not compressed: an .msix and PNGs are compressed
 * already. Names go in as UTF-8, with forward slashes. Throws beyond what a plain (not ZIP64) archive holds.
 */
export function zip(entries) {
    const parts = [];
    const central = [];
    let offset = 0;
    for (const { name, data } of entries) {
        const file = Buffer.from(name.replaceAll("\\", "/"), "utf8");
        if (data.length >= 0xffffffff) throw new Error(`${name} is too big for the upload's ZIP.`);
        const sum = crc32(data);
        const local = Buffer.alloc(30);
        local.writeUInt32LE(0x04034b50, 0);
        local.writeUInt16LE(20, 4); // version needed: 2.0
        local.writeUInt16LE(0x0800, 6); // names are UTF-8
        local.writeUInt16LE(0, 8); // stored
        local.writeUInt16LE(0, 10); // time
        local.writeUInt16LE(0x21, 12); // date: 1980-01-01
        local.writeUInt32LE(sum, 14);
        local.writeUInt32LE(data.length, 18);
        local.writeUInt32LE(data.length, 22);
        local.writeUInt16LE(file.length, 26);
        local.writeUInt16LE(0, 28);
        const header = Buffer.alloc(46);
        header.writeUInt32LE(0x02014b50, 0);
        header.writeUInt16LE(20, 4);
        header.writeUInt16LE(20, 6);
        header.writeUInt16LE(0x0800, 8);
        header.writeUInt16LE(0, 10);
        header.writeUInt16LE(0, 12);
        header.writeUInt16LE(0x21, 14);
        header.writeUInt32LE(sum, 16);
        header.writeUInt32LE(data.length, 20);
        header.writeUInt32LE(data.length, 24);
        header.writeUInt16LE(file.length, 28);
        header.writeUInt32LE(offset, 42);
        central.push(header, file);
        parts.push(local, file, data);
        offset += 30 + file.length + data.length;
        if (offset >= 0xffffffff) throw new Error("The upload's ZIP would pass 4 GB.");
    }
    const directory = Buffer.concat(central);
    const end = Buffer.alloc(22);
    end.writeUInt32LE(0x06054b50, 0);
    end.writeUInt16LE(entries.length, 8);
    end.writeUInt16LE(entries.length, 10);
    end.writeUInt32LE(directory.length, 12);
    end.writeUInt32LE(offset, 16);
    return Buffer.concat([...parts, directory, end]);
}

/**
 * One entry of a ZIP (an .msix is one), read: `name`'s bytes, or undefined when it has no such entry. Reads the
 * central directory (ZIP64's too, which makeappx writes for a package of any size), stored and deflated entries.
 */
export function unzipEntry(archive, name) {
    let end = -1;
    for (let i = archive.length - 22; i >= Math.max(0, archive.length - 22 - 0xffff); i--) {
        if (archive.readUInt32LE(i) === 0x06054b50) {
            end = i;
            break;
        }
    }
    if (end < 0) throw new Error("Not a ZIP archive (no end of central directory).");
    let count = archive.readUInt16LE(end + 10);
    let at = archive.readUInt32LE(end + 16);
    // ZIP64: the real numbers are in a record of their own, whose place a locator just before this one gives.
    if (end >= 20 && archive.readUInt32LE(end - 20) === 0x07064b50) {
        const record = Number(archive.readBigUInt64LE(end - 20 + 8));
        if (archive.readUInt32LE(record) === 0x06064b50) {
            count = Number(archive.readBigUInt64LE(record + 32));
            at = Number(archive.readBigUInt64LE(record + 48));
        }
    }
    for (let i = 0; i < count; i++) {
        if (archive.readUInt32LE(at) !== 0x02014b50) throw new Error("The ZIP's central directory is damaged.");
        const method = archive.readUInt16LE(at + 10);
        let packed = archive.readUInt32LE(at + 20);
        let size = archive.readUInt32LE(at + 24);
        const nameLength = archive.readUInt16LE(at + 28);
        const extraLength = archive.readUInt16LE(at + 30);
        const commentLength = archive.readUInt16LE(at + 32);
        let local = archive.readUInt32LE(at + 42);
        const entry = archive.toString("utf8", at + 46, at + 46 + nameLength);
        // ZIP64's extra field holds whichever of size, packed size and offset didn't fit, in that order.
        let extra = at + 46 + nameLength;
        const extraEnd = extra + extraLength;
        while (extra + 4 <= extraEnd) {
            const id = archive.readUInt16LE(extra);
            const length = archive.readUInt16LE(extra + 2);
            if (id === 1) {
                let field = extra + 4;
                if (size === 0xffffffff) (size = Number(archive.readBigUInt64LE(field))), (field += 8);
                if (packed === 0xffffffff) (packed = Number(archive.readBigUInt64LE(field))), (field += 8);
                if (local === 0xffffffff) local = Number(archive.readBigUInt64LE(field));
            }
            extra += 4 + length;
        }
        if (entry === name) {
            const data = local + 30 + archive.readUInt16LE(local + 26) + archive.readUInt16LE(local + 28);
            const bytes = archive.subarray(data, data + packed);
            if (method === 0) return Buffer.from(bytes);
            if (method === 8) return inflateRawSync(bytes);
            throw new Error(`${name} is packed in a way this doesn't read (method ${method}).`);
        }
        at += 46 + nameLength + extraLength + commentLength;
    }
    return undefined;
}

/** An .msix's identity from its AppxManifest.xml: { name, publisher, version, architecture }. Throws when it has none. */
export function packageIdentity(msix) {
    const manifest = unzipEntry(msix, "AppxManifest.xml")?.toString("utf8");
    const identity = manifest?.match(/<Identity\b[^>]*>/)?.[0];
    if (!identity) throw new Error("The package has no AppxManifest.xml with an Identity: is it an .msix?");
    const value = (attribute) => identity.match(new RegExp(`\\b${attribute}="([^"]*)"`))?.[1];
    return { name: value("Name"), publisher: value("Publisher"), version: value("Version"), architecture: value("ProcessorArchitecture") };
}

// ---- The upload ------------------------------------------------------------------------------------------

/** Blocks of this size are taken by every version of the storage service a Store address may be signed for. */
const BLOCK = 4 * 1024 * 1024;

/**
 * Uploads `data` to the storage address (a shared access signature) a submission came with: in blocks, then the
 * list that makes them the blob. `progress(sent, total)` is told after each block.
 */
export async function upload(address, data, progress = () => {}) {
    const ids = [];
    for (let start = 0, index = 0; start < data.length || index === 0; start += BLOCK, index++) {
        const id = Buffer.from(`block-${String(index).padStart(6, "0")}`).toString("base64");
        const res = await fetch(`${address}&comp=block&blockid=${encodeURIComponent(id)}`, { method: "PUT", body: data.subarray(start, start + BLOCK) });
        if (!res.ok) throw new StoreError("Uploading the files", res.status, (await res.text()).slice(0, 300));
        ids.push(id);
        progress(Math.min(start + BLOCK, data.length), data.length);
    }
    const list = `<?xml version="1.0" encoding="utf-8"?><BlockList>${ids.map((id) => `<Latest>${id}</Latest>`).join("")}</BlockList>`;
    const res = await fetch(`${address}&comp=blocklist`, { method: "PUT", headers: { "x-ms-blob-content-type": "application/zip" }, body: list });
    if (!res.ok) throw new StoreError("Finishing the upload", res.status, (await res.text()).slice(0, 300));
}

/** A status that will change by itself (the commit is being taken); every other one is where it rests, or an end. */
export const committing = (status) => status === "CommitStarted";

/** Whether a status is a failure. */
export const failed = (status) => /Failed$/.test(status ?? "") || status === "Canceled";

/** The lines of a status's details: errors, warnings and the certification reports' addresses. */
export function statusLines(details = {}) {
    const lines = [];
    for (const e of details.errors ?? []) lines.push(`  ✗ ${e.code ?? "error"}: ${e.details ?? ""}`);
    for (const w of details.warnings ?? []) lines.push(`  ! ${w.code ?? "warning"}: ${w.details ?? ""}`);
    for (const r of details.certificationReports ?? []) lines.push(`  report (${r.date ?? ""}): ${r.reportUrl ?? ""}`);
    return lines;
}
