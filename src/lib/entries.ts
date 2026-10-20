import { joinPath } from "./paths.ts";

/** Windows FILE_ATTRIBUTE_* bits the explorer cares about, and two of Glint's own for archives (Windows doesn't use them). */
export const ATTR = {
    readonly: 0x1,
    hidden: 0x2,
    system: 0x4,
    reparsePoint: 0x400,
    offline: 0x1000,
    /** In an archive: password-protected. */
    encrypted: 0x4000,
    recallOnOpen: 0x40000,
    recallOnDataAccess: 0x400000,
    /** Compressed with a method Glint can't read. */
    unreadable: 0x10000000,
    /** In a zip or 7z archive: read-only, and extracted to be opened (see archive/tree.rs). */
    inArchive: 0x40000000,
};

export interface FileEntry {
    name: string;
    path: string;
    dir: boolean;
    size: number;
    /** Last write time, ms since the Unix epoch (0 when unknown). */
    modified: number;
    attrs: number;
    /** Lowercased name, for sorting and filtering. */
    lower: string;
    /** Lowercased extension without the dot; "" for folders and names without one. */
    ext: string;
}

/** An entry as the backend sends it; listings leave out the path. */
export type RawEntry = Omit<FileEntry, "path" | "lower" | "ext"> & { path?: string };

/** Fills in path, lower and ext in place (no copies: listings can hold 100k entries). */
export function withDetails(raw: RawEntry, dir = ""): FileEntry {
    const entry = raw as FileEntry;
    entry.path = raw.path ?? joinPath(dir, raw.name);
    entry.lower = raw.name.toLowerCase();
    const dot = entry.lower.lastIndexOf(".");
    entry.ext = !raw.dir && dot > 0 ? entry.lower.slice(dot + 1) : "";
    return entry;
}

/** Where a folder in an archive is: the archive opened like a folder. */
export interface InArchive {
    /** The archive's own path (an archive in an archive included), the start of the folder's. */
    path: string;
    /** The archive file on disk everything is in (the same, unless archives nest). */
    outer: string;
    format: "zip" | "7z";
    /** Files and folders in the whole archive. */
    entries: number;
    /** Its list of entries was already in memory; otherwise it took `readMs` to read. */
    cached: boolean;
    readMs: number;
}

export interface Listing {
    path: string;
    entries: FileEntry[];
    /** Milliseconds Rust spent enumerating and sorting (or reading the archive's list of entries). */
    elapsed: number;
    /** Set for a folder in a zip or 7z archive (read-only). */
    archive?: InArchive;
}

const utf8 = new TextDecoder();

/** The header before a listing's arrays: counts and timings, and where an archive is in its path. */
const HEADER = 40;

/**
 * Reads the binary listing from `list_dir` (layout documented on `encode_listing` in fsx.rs):
 * numbers are viewed in place as typed arrays, and all names decode in one go.
 */
export function decodeListing(buffer: ArrayBuffer): Listing {
    const view = new DataView(buffer);
    const count = view.getUint32(0, true);
    const pathBytes = view.getUint32(4, true);
    const elapsed = view.getFloat64(8, true);
    const sizes = new Float64Array(buffer, HEADER, count);
    const modified = new Float64Array(buffer, HEADER + 8 * count, count);
    const attrs = new Uint32Array(buffer, HEADER + 16 * count, count);
    const lengths = new Uint32Array(buffer, HEADER + 20 * count, count);
    const text = HEADER + 24 * count;
    const path = utf8.decode(new Uint8Array(buffer, text, pathBytes));
    const names = utf8.decode(new Uint8Array(buffer, text + pathBytes));
    const entries = new Array<FileEntry>(count);
    for (let i = 0, at = 0; i < count; i++) {
        const name = names.slice(at, (at += lengths[i]));
        entries[i] = withDetails({ name, dir: (attrs[i] & 0x10) !== 0, size: sizes[i], modified: modified[i], attrs: attrs[i] }, path);
    }
    const listing: Listing = { path, entries, elapsed };
    const archiveUnits = view.getUint32(16, true);
    if (archiveUnits) {
        const archive = path.slice(0, archiveUnits);
        listing.archive = {
            path: archive,
            outer: path.slice(0, view.getUint32(20, true)),
            format: archiveFormat(archive) ?? "zip",
            entries: view.getUint32(24, true),
            cached: (view.getUint32(28, true) & 1) !== 0,
            readMs: view.getFloat64(32, true),
        };
    }
    return listing;
}

/** "zip" or "7z" for a name Glint opens like a folder (whatever's before it), else null. */
export function archiveFormat(name: string): InArchive["format"] | null {
    const lower = name.toLowerCase();
    return lower.endsWith(".zip") ? "zip" : lower.endsWith(".7z") ? "7z" : null;
}

/** A zip or 7z file (on disk or in another archive): Enter opens it like a folder. */
export const isArchiveFile = (e: FileEntry) => !e.dir && (e.ext === "zip" || e.ext === "7z");

/** Something inside an archive: read-only, and extracted to be opened or copied. */
export const isArchived = (e: FileEntry) => (e.attrs & ATTR.inArchive) !== 0;

/** In an archive, but can't be extracted: password-protected, or compressed in a way Glint can't read. */
export const isLocked = (e: FileEntry) => (e.attrs & (ATTR.encrypted | ATTR.unreadable)) !== 0;

export const isHidden = (e: FileEntry) => (e.attrs & ATTR.hidden) !== 0;

/** OneDrive-style placeholders: reading them would start a download. */
export const isOnlineOnly = (e: FileEntry) => (e.attrs & (ATTR.offline | ATTR.recallOnOpen | ATTR.recallOnDataAccess)) !== 0;

export type SortKey = "name" | "modified" | "type" | "size";
export interface Sort {
    key: SortKey;
    desc: boolean;
}

const isDigit = (c: number) => c >= 48 && c <= 57;

/** Natural order for lowercased names: "file2" before "file10" (matches the Rust side). */
export function naturalCompare(a: string, b: string) {
    let i = 0;
    let j = 0;
    while (i < a.length && j < b.length) {
        const ca = a.charCodeAt(i);
        const cb = b.charCodeAt(j);
        if (isDigit(ca) && isDigit(cb)) {
            let si = i;
            let sj = j;
            while (i < a.length && isDigit(a.charCodeAt(i))) i++;
            while (j < b.length && isDigit(b.charCodeAt(j))) j++;
            while (si < i - 1 && a.charCodeAt(si) === 48) si++;
            while (sj < j - 1 && b.charCodeAt(sj) === 48) sj++;
            if (i - si !== j - sj) return i - si - (j - sj);
            for (let k = 0; k < i - si; k++) {
                const d = a.charCodeAt(si + k) - b.charCodeAt(sj + k);
                if (d) return d;
            }
        } else {
            if (ca !== cb) return ca - cb;
            i++;
            j++;
        }
    }
    return a.length - i - (b.length - j);
}

const byName = (a: FileEntry, b: FileEntry) => naturalCompare(a.lower, b.lower) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);

/**
 * Sorts for display. `presorted` entries already come folders-first in name order (listings sort in
 * Rust), so the default view costs nothing and ties keep name order. `sizeOf` lets folders sort by
 * their calculated size.
 */
export function sortEntries(entries: FileEntry[], sort: Sort, foldersFirst: boolean, presorted: boolean, sizeOf = (e: FileEntry) => e.size) {
    if (presorted && foldersFirst && sort.key === "name" && !sort.desc) return entries;
    const direction = sort.desc ? -1 : 1;
    const group = (e: FileEntry) => (foldersFirst && e.dir ? 0 : 1);
    const compare: Record<SortKey, (a: FileEntry, b: FileEntry) => number> = {
        name: byName,
        modified: (a, b) => a.modified - b.modified,
        size: (a, b) => sizeOf(a) - sizeOf(b),
        type: (a, b) => Number(b.dir) - Number(a.dir) || (a.ext < b.ext ? -1 : a.ext > b.ext ? 1 : 0),
    };
    const by = compare[sort.key];
    return [...entries].sort((a, b) => group(a) - group(b) || direction * by(a, b) || (presorted ? 0 : byName(a, b)));
}

/** Splits a query into terms; "double quotes" keep spaces in one term (matches the Rust side). */
export function terms(query: string) {
    const found: string[] = [];
    let current = "";
    let quoted = false;
    for (const c of query) {
        if (c === '"') quoted = !quoted;
        else if (!quoted && /\s/.test(c)) {
            if (current) found.push(current);
            current = "";
        } else current += c;
    }
    if (current) found.push(current);
    return found;
}

function globToRegExp(glob: string) {
    const source = glob.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".");
    return new RegExp(`^${source}$`);
}

const SIZE_UNITS: Record<string, number> = { "": 1, b: 1, k: 2 ** 10, kb: 2 ** 10, m: 2 ** 20, mb: 2 ** 20, g: 2 ** 30, gb: 2 ** 30, t: 2 ** 40, tb: 2 ** 40 };
const AGE_UNITS: Record<string, number> = { h: 3600, d: 86400, w: 7 * 86400, y: 365 * 86400 };

/**
 * A filter term (ext:, size:, dm:, file:, folder:) as the index and fd read it (shadui_index::query::parse),
 * for what's on screen; undefined for a name term. One that doesn't read right matches nothing here
 * (the search says what's wrong with it).
 */
function filterTest(term: string): ((e: FileEntry) => boolean) | undefined {
    const [, key, value] = /^(ext|size|dm|files?|folders?):(.*)$/.exec(term) ?? [];
    if (!key) return undefined;
    if (key === "ext") {
        const exts = value.split(/[;,]/).map((x) => x.trim().replace(/^\.+/, "")).filter(Boolean);
        return (e) => !exts.length || exts.some((x) => e.lower.endsWith(`.${x}`));
    }
    if (key.startsWith("file")) return value ? undefined : (e) => !e.dir;
    if (key.startsWith("folder")) return value ? undefined : (e) => e.dir;
    const [, op, amount] = /^([<>=]*)(.*)$/.exec(value)!;
    if (key === "size") {
        const [, number, unit] = /^([\d.]+)([a-z]*)$/.exec(amount) ?? [];
        const bytes = Math.floor(Number(number) * SIZE_UNITS[unit]);
        if (!number || Number.isNaN(bytes)) return () => false;
        const tests: Record<string, (size: number) => boolean> = { ">": (s) => s > bytes, ">=": (s) => s >= bytes, "<": (s) => s < bytes, "<=": (s) => s <= bytes };
        const fits = tests[op] ?? ((s: number) => s === bytes);
        // Only files have a size of their own.
        return (e) => !e.dir && fits(e.size);
    }
    const [, count, unit] = /^(\d+)([a-z]+)$/.exec(amount) ?? [];
    if (!count || !AGE_UNITS[unit]) return () => false;
    const ms = Number(count) * AGE_UNITS[unit] * 1000;
    const now = Date.now();
    return op.startsWith(">") ? (e) => now - e.modified > ms : (e) => now - e.modified < ms;
}

/**
 * The same matching fd does for a search, applied to what's on screen: every term must match the
 * name, case-insensitively; terms with * or ? match the whole name, others any part of it. Filters
 * (ext:pdf, size:>10mb, dm:7d, file:, folder:) narrow it as the index does.
 */
export function matcher(query: string): ((e: FileEntry) => boolean) | null {
    const parts = terms(query.toLowerCase());
    if (!parts.length) return null;
    const tests = parts.map((term) => {
        const filter = filterTest(term);
        if (filter) return filter;
        if (!/[*?]/.test(term)) return (e: FileEntry) => e.lower.includes(term);
        const re = globToRegExp(term);
        return (e: FileEntry) => re.test(e.lower);
    });
    return (e) => tests.every((test) => test(e));
}

export type DiffStatus = "unique" | "newer" | "older" | "differ" | "same";

/**
 * Compares an item in one pane with the matching name in the other pane (`other`).
 * Allows a 2-second timestamp tolerance for FAT/archive rounding when sizes match.
 */
export function compareEntries(own: FileEntry, other: FileEntry | undefined, ownDirSize?: number, otherDirSize?: number): DiffStatus {
    if (!other || other.dir !== own.dir) return "unique";
    if (own.dir) {
        if (ownDirSize !== undefined && otherDirSize !== undefined && ownDirSize !== otherDirSize) return "differ";
        return "same";
    }
    if (own.modified > 0 && other.modified > 0 && Math.abs(own.modified - other.modified) > 2000) {
        return own.modified > other.modified ? "newer" : "older";
    }
    if (own.size !== other.size) return "differ";
    return "same";
}

