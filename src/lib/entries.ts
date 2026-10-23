import { ARCHIVE_FORMATS, type ArchiveFormat, BROWSED, ON_REQUEST } from "./archives.ts";
import type { GitMark } from "./fs.ts";
import type { MediaFacts } from "./media.ts";
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
    /** In the Recycle Bin: its path is its file there, its `origin` the folder it was deleted from (see recycle.rs). */
    recycled: 0x08000000,
    /** Compressed with a method Glint can't read. */
    unreadable: 0x10000000,
    /** In an ncdu export (with `inArchive`): listed with its size and date, but its contents aren't there (see archive/ncdu.rs). */
    listOnly: 0x20000000,
    /** In a zip or 7z archive: read-only, and extracted to be opened (see archive/tree.rs). */
    inArchive: 0x40000000,
    /** Listed from instant search's index, in a folder Windows keeps closed to this account: read-only, and it can't be opened (see index.rs). */
    closed: 0x80000000,
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
    /** In the Recycle Bin: the folder it was deleted from (`modified` is when). */
    origin?: string;
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
    format: ArchiveFormat;
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
    /** Listed from instant search's index: Windows keeps the folder closed to this account, so it's read-only and nothing in it opens. */
    fromIndex?: boolean;
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
    // Items each with a path of their own (flag 4): the Recycle Bin's.
    if (view.getUint32(28, true) & 4) {
        const { location, entries, elapsed } = decodeItems(buffer);
        return { path: location, entries, elapsed };
    }
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
    if (view.getUint32(28, true) & 2) listing.fromIndex = true;
    const archiveUnits = view.getUint32(16, true);
    if (archiveUnits) {
        const flags = view.getUint32(28, true);
        listing.archive = {
            path: path.slice(0, archiveUnits),
            outer: path.slice(0, view.getUint32(20, true)),
            // The format's code plus one, in bits 8 to 15.
            format: ARCHIVE_FORMATS[((flags >> 8) & 0xff) - 1] ?? "zip",
            entries: view.getUint32(24, true),
            cached: (flags & 1) !== 0,
            readMs: view.getFloat64(32, true),
        };
    }
    return listing;
}

/**
 * Reads items that each carry their own path, such as search results (layout documented on
 * `encode_items` in fsx.rs): a name is the end of its path unless the names follow the paths, and
 * where each came from follows them when the Recycle Bin says.
 */
export function decodeItems(buffer: ArrayBuffer): { location: string; entries: FileEntry[]; elapsed: number } {
    const view = new DataView(buffer);
    const count = view.getUint32(0, true);
    const locationBytes = view.getUint32(4, true);
    const elapsed = view.getFloat64(8, true);
    const columns = view.getUint32(24, true);
    const withNames = (columns & 1) !== 0;
    const withOrigins = (columns & 2) !== 0;
    const sizes = new Float64Array(buffer, HEADER, count);
    const modified = new Float64Array(buffer, HEADER + 8 * count, count);
    const attrs = new Uint32Array(buffer, HEADER + 16 * count, count);
    const pathLengths = new Uint32Array(buffer, HEADER + 20 * count, count);
    const nameLengths = new Uint32Array(buffer, HEADER + 24 * count, count);
    const originLengths = withOrigins ? new Uint32Array(buffer, HEADER + 28 * count, count) : null;
    const textAt = HEADER + (withOrigins ? 32 : 28) * count;
    const location = utf8.decode(new Uint8Array(buffer, textAt, locationBytes));
    // Every path (and name, and origin) decodes in one go, then is cut up by its length in UTF-16 units.
    const text = utf8.decode(new Uint8Array(buffer, textAt + locationBytes));
    let names = 0;
    for (let i = 0; i < count; i++) names += pathLengths[i];
    let origins = names;
    if (withNames) for (let i = 0; i < count; i++) origins += nameLengths[i];
    const entries = new Array<FileEntry>(count);
    for (let i = 0, at = 0; i < count; i++) {
        const path = text.slice(at, (at += pathLengths[i]));
        const name = withNames ? text.slice(names, (names += nameLengths[i])) : path.slice(path.length - nameLengths[i]);
        const entry = withDetails({ name, path, dir: (attrs[i] & 0x10) !== 0, size: sizes[i], modified: modified[i], attrs: attrs[i] });
        if (originLengths) entry.origin = text.slice(origins, (origins += originLengths[i]));
        entries[i] = entry;
    }
    return { location, entries, elapsed };
}

/** An archive file (on disk or in another archive) Enter opens like a folder: a zip, a 7z… */
export const isArchiveFile = (e: FileEntry) => !e.dir && BROWSED.includes(e.ext);

/** A file that's an archive by another name (a .docx, a .jar…): Enter opens it in its app, "Open as archive" looks inside. */
export const isBrowsable = (e: FileEntry) => !e.dir && ON_REQUEST.includes(e.ext);

/** Something inside an archive: read-only, and extracted to be opened or copied. */
export const isArchived = (e: FileEntry) => (e.attrs & ATTR.inArchive) !== 0;

/** Listed from the index, in a folder Windows keeps closed to this account: read-only, and it can't be opened. */
export const isClosed = (e: FileEntry) => (e.attrs & ATTR.closed) !== 0;

/** In the Recycle Bin: restored, or deleted for good, but not opened or changed where it is. */
export const isRecycled = (e: FileEntry) => (e.attrs & ATTR.recycled) !== 0;

/** In an archive, but can't be extracted: password-protected, or compressed in a way Glint can't read. */
export const isLocked = (e: FileEntry) => (e.attrs & (ATTR.encrypted | ATTR.unreadable)) !== 0;
/** A file in an ncdu export: a name, a size and a date, with nothing to open or preview. */
export const isListOnly = (e: FileEntry) => (e.attrs & (ATTR.inArchive | ATTR.listOnly)) === (ATTR.inArchive | ATTR.listOnly);

export const isHidden = (e: FileEntry) => (e.attrs & ATTR.hidden) !== 0;

/** OneDrive-style placeholders: reading them would start a download. */
export const isOnlineOnly = (e: FileEntry) => (e.attrs & (ATTR.offline | ATTR.recallOnOpen | ATTR.recallOnDataAccess)) !== 0;

export type SortKey = "name" | "modified" | "taken" | "type" | "size";
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
export function sortEntries(
    entries: FileEntry[],
    sort: Sort,
    foldersFirst: boolean,
    presorted: boolean,
    sizeOf = (e: FileEntry) => e.size,
    takenOf: (e: FileEntry) => number | null | undefined = () => undefined
) {
    if (presorted && foldersFirst && sort.key === "name" && !sort.desc) return entries;
    const direction = sort.desc ? -1 : 1;
    const group = (e: FileEntry) => (foldersFirst && e.dir ? 0 : 1);
    const compare: Record<SortKey, (a: FileEntry, b: FileEntry) => number> = {
        name: byName,
        modified: (a, b) => a.modified - b.modified,
        // When a photo or video was taken, else when the file was modified.
        taken: (a, b) => (takenOf(a) ?? a.modified) - (takenOf(b) ?? b.modified),
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

/** What `git:` takes, and the marks each stands for. */
const GIT_FILTERS: Record<string, GitMark[]> = {
    changed: ["modified", "staged", "untracked", "conflicted"],
    modified: ["modified"],
    staged: ["staged"],
    new: ["untracked"],
    untracked: ["untracked"],
    conflict: ["conflicted"],
    conflicted: ["conflicted"],
    ignored: ["ignored"],
};

/** At least and at most, both included (query.rs's Range). */
type Range = { min: number | null; max: number | null };
const inRange = (r: Range, n: number) => (r.min === null || n >= r.min) && (r.max === null || n <= r.max);

/** `>1920`, `<=1080` or `1080` (exactly), each number read by `number`. */
function range(value: string, number: (text: string) => number | null): Range | null {
    const [, op, amount] = /^([<>=]*)(.*)$/.exec(value)!;
    const n = number(amount);
    if (n === null) return null;
    if (op === ">") return { min: n + 1, max: null };
    if (op === ">=") return { min: n, max: null };
    if (op === "<") return n > 0 ? { min: null, max: n - 1 } : null;
    if (op === "<=") return { min: null, max: n };
    return op === "" || op === "=" ? { min: n, max: n } : null;
}

const whole = (text: string) => (/^\d+$/.test(text) ? Number(text) : null);

/** "90" or "90s", "5m", "1.5h", "1:30" or "1:02:03", in seconds. */
function seconds(text: string): number | null {
    if (text.includes(":")) {
        const parts = text.split(":");
        if (parts.some((p, i) => !/^\d+$/.test(p) || (i > 0 && p.length > 2))) return null;
        return parts.reduce((total, p) => total * 60 + Number(p), 0);
    }
    const [, number, unit] = /^([\d.]+)([a-z]*)$/i.exec(text) ?? [];
    const scale: Record<string, number> = { "": 1, s: 1, sec: 1, m: 60, min: 60, h: 3600, hr: 3600 };
    if (!number || Number.isNaN(Number(number)) || !(unit.toLowerCase() in scale)) return null;
    return Math.round(Number(number) * scale[unit.toLowerCase()]);
}

/** `2024`, `2024-06` or `2024-06-15`: midnight here of its first day, and of the first day after. */
function period(text: string): [number, number] | null {
    const [, y, m, d] = /^(\d{4})(?:-(\d{2})(?:-(\d{2}))?)?$/.exec(text) ?? [];
    if (!y) return null;
    const year = Number(y);
    if (!m) return [new Date(year, 0, 1).getTime(), new Date(year + 1, 0, 1).getTime()];
    const month = Number(m);
    if (month < 1 || month > 12) return null;
    if (!d) return [new Date(year, month - 1, 1).getTime(), new Date(year, month, 1).getTime()];
    const day = Number(d);
    if (day < 1 || day > new Date(year, month, 0).getDate()) return null;
    return [new Date(year, month - 1, day).getTime(), new Date(year, month - 1, day + 1).getTime()];
}

/** The keys of what photos, videos and songs say of themselves (query.rs's Prop). */
const PROPS = /^(width|height|duration|taken|camera|artist|album):(.*)$/;

/** Whether a search holds a property filter (width:, taken:…), which needs files read to apply. */
export const hasPropFilter = (query: string) => terms(query.toLowerCase()).some((t) => PROPS.test(t));

/**
 * A property filter's test of what a file says (media.rs, as the search applies it), or null when it
 * doesn't read right: `width:>1920`, `height:`, `duration:>5m`, `taken:2024` (2024-06, 2024-06-15, after
 * `>` or `<`, or 7d and >1y as dm: goes), `camera:`, `artist:` and `album:` with some of their text.
 */
export function propTest(key: string, value: string, now = Date.now()): ((f: MediaFacts) => boolean) | null {
    if (key === "width" || key === "height") {
        const r = range(value, whole);
        return r && ((f) => f[key] !== null && inRange(r, f[key]!));
    }
    if (key === "duration") {
        const r = range(value, seconds);
        return r && ((f) => f.duration !== null && inRange(r, Math.floor(f.duration)));
    }
    if (key === "taken") {
        const [, op, amount] = /^([<>=]*)(.*)$/.exec(value)!;
        const [, count, unit] = /^(\d+)([hdwy])$/.exec(amount) ?? [];
        if (count) {
            const since = now - Number(count) * AGE_UNITS[unit] * 1000;
            return op.startsWith(">") ? (f) => f.taken !== null && f.taken < since : (f) => f.taken !== null && f.taken >= since;
        }
        const days = period(amount);
        if (!days) return null;
        const [start, end] = days;
        const bounds: Record<string, [number | null, number | null]> = { "": [start, end], "=": [start, end], ">": [end, null], ">=": [start, null], "<": [null, start], "<=": [null, end] };
        const [from, to] = bounds[op] ?? [];
        if (from === undefined) return null;
        return (f) => f.taken !== null && (from === null || f.taken >= from) && (to === null || f.taken < to);
    }
    const wanted = value.trim().toLowerCase();
    const field = key as "camera" | "artist" | "album";
    return (f) => f[field] !== null && f[field]!.includes(wanted);
}

/**
 * A filter term (ext:, size:, dm:, file:, folder:) as the index and fd read it (shadui_index::query::parse),
 * for what's on screen; undefined for a name term. One that doesn't read right matches nothing here
 * (the search says what's wrong with it). `git:changed` (modified, staged, new, conflict, ignored) asks
 * the folder's Git marks, so it filters what's on screen only. A property filter (width:, taken:…) asks
 * what the file says (`facts`, undefined until it's read): without them it's left to the search.
 */
function filterTest(term: string, git?: Readonly<Record<string, GitMark>>, facts?: (e: FileEntry) => MediaFacts | null | undefined): ((e: FileEntry) => boolean) | undefined {
    const [, prop, wanted] = PROPS.exec(term) ?? [];
    if (prop) {
        if (!facts) return () => true;
        const test = propTest(prop, wanted);
        return (e) => {
            const f = !e.dir && test ? facts(e) : null;
            return !!f && test!(f);
        };
    }
    const [, key, value] = /^(ext|size|dm|files?|folders?|git):(.*)$/.exec(term) ?? [];
    if (!key) return undefined;
    if (key === "git") {
        const wanted = GIT_FILTERS[value];
        return (e) => !!wanted && !!git && wanted.includes(git[e.lower]);
    }
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
 * (ext:pdf, size:>10mb, dm:7d, file:, folder:) narrow it as the index does; git:changed by `git`,
 * the folder's Git marks by lowercase name; width:, taken: and the like by `facts`, what files say.
 */
export function matcher(query: string, git?: Readonly<Record<string, GitMark>>, facts?: (e: FileEntry) => MediaFacts | null | undefined): ((e: FileEntry) => boolean) | null {
    const parts = terms(query.toLowerCase());
    if (!parts.length) return null;
    const tests = parts.map((term) => {
        const filter = filterTest(term, git, facts);
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

