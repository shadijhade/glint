import { ARCHIVE_FORMATS, type ArchiveFormat } from "./archives.ts";
import { type FileEntry, withDetails } from "./entries.ts";
import { joinPath } from "./paths.ts";

/** The folder mapped has no parent. */
export const NO_PARENT = 0xffffffff;

/** Who drew a map: the index, pdu, or (a folder in an archive) the archive's own list of entries, by its format. */
export type UsageEngine = "index" | "pdu" | ArchiveFormat;
/** By the number usage.rs's encode_map writes: 2 and up are archives, by their format's code. */
const ENGINES: UsageEngine[] = ["index", "pdu", ...ARCHIVE_FORMATS];

/**
 * A disk-usage map of a folder: its biggest files and folders a few levels down. Node 0 is the folder
 * itself; every folder's items are stored together, biggest first, after the folder.
 */
export interface UsageMap {
    path: string;
    /** The index, pdu, or (a folder in an archive) the archive's own list of entries. */
    engine: UsageEngine;
    /** Milliseconds Rust spent: the index's answer, or pdu's whole run. */
    elapsed: number;
    /** Smaller items were left out; they still count in their folders' sizes. */
    limited: boolean;
    /** pdu drew it because the instant search service runs, but is a version from before maps. */
    serviceTooOld: boolean;
    /** What grew was asked for, but the instant search service running is a version from before it. */
    growthTooOld: boolean;
    /** What each item held at the index's daily mark closest to the time asked (index maps only). */
    growth: UsageGrowth | null;
    count: number;
    size: Float64Array;
    modified: Float64Array;
    /** FILE_ATTRIBUTE_* bits; folders have the directory bit. */
    attrs: Uint32Array;
    parent: Uint32Array;
    names: string[];
    paths: string[];
    /** A folder's items are first[i] up to (not including) end[i]. */
    first: Uint32Array;
    end: Uint32Array;
}

/** What the items of a map held at a daily mark of the index's. */
export interface UsageGrowth {
    /** When the mark was taken (ms). */
    since: number;
    /** The oldest mark there is: how far back growth goes. */
    oldest: number;
    /** The whole drive's total then and now. */
    driveThen: number;
    driveNow: number;
    /** Each item's size at the mark, or WAS_NOT_THERE, or WRITTEN_SINCE (a file; what it held isn't kept). */
    previous: Float64Array;
}

export const WAS_NOT_THERE = -1;
export const WRITTEN_SINCE = -2;

export const isFolder = (map: UsageMap, i: number) => (map.attrs[i] & 0x10) !== 0;

const utf8 = new TextDecoder();

/** Reads `disk_usage`'s buffer (layout documented on `encode_map` in usage.rs). */
export function decodeUsage(buffer: ArrayBuffer): UsageMap {
    const view = new DataView(buffer);
    const count = view.getUint32(0, true);
    const pathBytes = view.getUint32(4, true);
    const elapsed = view.getFloat64(8, true);
    const engine = ENGINES[view.getUint32(16, true)] ?? "pdu";
    const flags = view.getUint32(20, true);
    const [limited, serviceTooOld, grew, growthTooOld] = [(flags & 1) !== 0, (flags & 2) !== 0, (flags & 4) !== 0, (flags & 8) !== 0];
    // With what grew, the mark's numbers come first and each item's earlier size after its date.
    let at = grew ? 56 : 24;
    const size = new Float64Array(buffer, at, count);
    const modified = new Float64Array(buffer, at + 8 * count, count);
    at += 16 * count;
    let growth: UsageGrowth | null = null;
    if (grew) {
        const [since, oldest, driveThen, driveNow] = [24, 32, 40, 48].map((k) => view.getFloat64(k, true));
        growth = { since, oldest, driveThen, driveNow, previous: new Float64Array(buffer, at, count) };
        at += 8 * count;
    }
    const attrs = new Uint32Array(buffer, at, count);
    const parent = new Uint32Array(buffer, at + 4 * count, count);
    const lengths = new Uint32Array(buffer, at + 8 * count, count);
    const text = at + 12 * count;
    const path = utf8.decode(new Uint8Array(buffer, text, pathBytes));
    const all = utf8.decode(new Uint8Array(buffer, text + pathBytes));
    const names = new Array<string>(count);
    const paths = new Array<string>(count);
    const first = new Uint32Array(count);
    const end = new Uint32Array(count);
    for (let i = 0, at = 0; i < count; i++) {
        names[i] = all.slice(at, (at += lengths[i]));
        if (i === 0) {
            paths[0] = path;
            continue;
        }
        const p = parent[i];
        paths[i] = joinPath(paths[p], names[i]);
        if (end[p] === 0) first[p] = i;
        end[p] = i + 1;
    }
    return { path, engine, elapsed, limited, serviceTooOld, growthTooOld, growth, count, size, modified, attrs, parent, names, paths, first, end };
}

/** How much item `i` grew since the mark (all of it, if it wasn't there); 0 for a file written since. */
export function grewBy(map: UsageMap, i: number) {
    const then = map.growth?.previous[i] ?? map.size[i];
    return then === WAS_NOT_THERE ? map.size[i] : then === WRITTEN_SINCE ? 0 : map.size[i] - then;
}

/**
 * Where most of the growth went: the deepest folders that grew by at least 5% of what the folder
 * mapped grew and that no folder in them holds 70% of. Biggest first, at most `max`.
 */
export function growthLeaders(map: UsageMap, max = 3): number[] {
    const total = grewBy(map, 0);
    if (!map.growth || total <= 0) return [];
    const leaders = new Set<number>();
    for (let i = 1; i < map.count; i++) {
        const grew = grewBy(map, i);
        if (!isFolder(map, i) || grew < total * 0.05) continue;
        let concentrated = false;
        for (let c = map.first[i]; c < map.end[i] && !concentrated; c++) concentrated = isFolder(map, c) && grewBy(map, c) >= grew * 0.7;
        if (!concentrated) leaders.add(i);
    }
    // A folder with one of them inside is told by that one.
    for (const i of [...leaders]) for (let p = map.parent[i]; p !== NO_PARENT; p = map.parent[p]) leaders.delete(p);
    return [...leaders].sort((a, b) => grewBy(map, b) - grewBy(map, a)).slice(0, max);
}

/** Every node as a file-list entry (the folder mapped first), for selection, menus and Quick Look. */
export function usageEntries(map: UsageMap): FileEntry[] {
    const entries = new Array<FileEntry>(map.count);
    for (let i = 0; i < map.count; i++) {
        entries[i] = withDetails({ name: map.names[i], path: map.paths[i], dir: isFolder(map, i), size: map.size[i], modified: map.modified[i], attrs: map.attrs[i] });
    }
    return entries;
}

/** How a node is drawn. */
export const HIDDEN = 0;
/** A file, or a folder drawn whole (nothing to show inside, or too small to). */
export const BLOCK = 1;
/** A folder drawn as a frame (with a header when there's room) around its items. */
export const FRAME = 2;

/** Sizes in CSS pixels. */
export const HEADER = 17;
const PAD = 2;
const MIN_FRAME = 22;
const MIN_HEADER = { w: 60, h: 42 };

export interface UsageLayout {
    width: number;
    height: number;
    kind: Uint8Array;
    x: Float32Array;
    y: Float32Array;
    w: Float32Array;
    h: Float32Array;
    /** A frame's header height, 0 when it has none. */
    header: Float32Array;
    /** Each frame's room for everything too small to list ("smaller items"); width 0 when none. */
    restX: Float32Array;
    restY: Float32Array;
    restW: Float32Array;
    restH: Float32Array;
    /** Frame depth: 0 for the folder mapped. */
    depth: Uint8Array;
    /** Nodes in drawing order, every frame before what's inside it. */
    order: Uint32Array;
}

/**
 * Lays `values` (largest first) out in the box as a squarified treemap (Bruls, Huizing and van Wijk):
 * rows along the shorter side, each grown while that keeps its rectangles closer to square.
 */
export function squarify(values: number[], x: number, y: number, w: number, h: number, place: (k: number, x: number, y: number, w: number, h: number) => void) {
    let total = 0;
    for (const v of values) total += v;
    if (total <= 0 || w <= 0 || h <= 0) return;
    const scale = (w * h) / total;
    for (let i = 0; i < values.length; ) {
        const wide = w >= h;
        const side = wide ? h : w;
        let sum = 0;
        let min = Infinity;
        let max = 0;
        let worst = Infinity;
        let j = i;
        while (j < values.length) {
            const area = values[j] * scale;
            const s = sum + area;
            const lo = Math.min(min, area);
            const hi = Math.max(max, area);
            const ratio = Math.max((side * side * hi) / (s * s), (s * s) / (side * side * lo));
            if (j > i && ratio > worst) break;
            [sum, min, max, worst] = [s, lo, hi, ratio];
            j++;
        }
        // The last row takes whatever is left, so rounding never leaves a gap.
        const thick = j === values.length ? (wide ? w : h) : Math.min(sum / side, wide ? w : h);
        let at = 0;
        for (let k = i; k < j; k++) {
            const len = k === j - 1 ? side - at : (values[k] * scale) / thick;
            if (wide) place(k, x, y + at, thick, len);
            else place(k, x + at, y, len, thick);
            at += len;
        }
        if (wide) [x, w] = [x + thick, w - thick];
        else [y, h] = [y + thick, h - thick];
        i = j;
    }
}

/** Lays the map out in a `width` × `height` box (CSS pixels). */
export function layoutUsage(map: UsageMap, width: number, height: number): UsageLayout {
    const n = map.count;
    const layout: UsageLayout = {
        width,
        height,
        kind: new Uint8Array(n),
        x: new Float32Array(n),
        y: new Float32Array(n),
        w: new Float32Array(n),
        h: new Float32Array(n),
        header: new Float32Array(n),
        restX: new Float32Array(n),
        restY: new Float32Array(n),
        restW: new Float32Array(n),
        restH: new Float32Array(n),
        depth: new Uint8Array(n),
        order: new Uint32Array(n),
    };
    let drawn = 0;
    const place = (i: number, x: number, y: number, w: number, h: number, depth: number) => {
        if (w < 0.5 || h < 0.5) return;
        [layout.x[i], layout.y[i], layout.w[i], layout.h[i], layout.depth[i]] = [x, y, w, h, depth];
        layout.order[drawn++] = i;
        layout.kind[i] = BLOCK;
        const [first, end] = [map.first[i], map.end[i]];
        if (first === end || (i !== 0 && (w < MIN_FRAME || h < MIN_FRAME))) return;
        // The folder mapped fills the box; others get a frame, and a header when there's room.
        const header = i !== 0 && w >= MIN_HEADER.w && h >= MIN_HEADER.h ? HEADER : 0;
        const pad = i === 0 ? 0 : PAD;
        const [ix, iy, iw, ih] = [x + pad, y + (header || pad), w - 2 * pad, h - (header || pad) - pad];
        if (iw < 2 || ih < 2) return;
        layout.kind[i] = FRAME;
        layout.header[i] = header;
        // What the listed items don't add up to is drawn as one block of smaller items, in size order.
        const values: number[] = [];
        let listed = 0;
        for (let c = first; c < end; c++) listed += map.size[c];
        const rest = map.size[i] - listed > listed * 1e-4 ? map.size[i] - listed : 0;
        let restAt = -1;
        for (let c = first; c < end; c++) {
            if (restAt < 0 && rest > map.size[c]) restAt = values.push(rest) - 1;
            values.push(Math.max(map.size[c], 0));
        }
        if (restAt < 0 && rest > 0) restAt = values.push(rest) - 1;
        squarify(values, ix, iy, iw, ih, (k, cx, cy, cw, ch) => {
            if (k === restAt) [layout.restX[i], layout.restY[i], layout.restW[i], layout.restH[i]] = [cx, cy, cw, ch];
            else place(first + k - (restAt >= 0 && k > restAt ? 1 : 0), cx, cy, cw, ch, depth + 1);
        });
    };
    if (n && width > 0 && height > 0) place(0, 0, 0, width, height, 0);
    layout.order = layout.order.slice(0, drawn);
    return layout;
}

const inside = (l: UsageLayout, i: number, x: number, y: number) => l.w[i] > 0 && x >= l.x[i] && x < l.x[i] + l.w[i] && y >= l.y[i] && y < l.y[i] + l.h[i];

/** What's under a point: the innermost item, and whether the point is on its frame's smaller items. */
export function hitTest(map: UsageMap, layout: UsageLayout, x: number, y: number): { node: number; rest: boolean } | null {
    if (!map.count || !inside(layout, 0, x, y)) return null;
    let node = 0;
    descend: while (layout.kind[node] === FRAME) {
        for (let c = map.first[node]; c < map.end[node]; c++) {
            if (inside(layout, c, x, y)) {
                node = c;
                continue descend;
            }
        }
        break;
    }
    const rest = layout.restW[node] > 0 && x >= layout.restX[node] && x < layout.restX[node] + layout.restW[node] && y >= layout.restY[node] && y < layout.restY[node] + layout.restH[node];
    return { node, rest };
}

/**
 * The nodes of `map` that `place` gives a new place (-1 for the others), in their order, which keeps
 * a map's: a kept node's folder must be kept, and come first. `size` replaces the sizes.
 */
function pick(map: UsageMap, place: Int32Array, kept: number, size = map.size): UsageMap {
    const index = Array.from({ length: map.count }, (_, i) => i).filter((i) => place[i] >= 0);
    const take = <T extends Float64Array | Uint32Array>(from: T, make: (n: number) => T) => {
        const out = make(kept);
        index.forEach((i, k) => (out[k] = from[i]));
        return out;
    };
    const parent = new Uint32Array(kept);
    index.forEach((i, k) => (parent[k] = k === 0 ? NO_PARENT : place[map.parent[i]]));
    const first = new Uint32Array(kept);
    const end = new Uint32Array(kept);
    for (let k = 1; k < kept; k++) {
        if (end[parent[k]] === 0) first[parent[k]] = k;
        end[parent[k]] = k + 1;
    }
    return {
        ...map,
        count: kept,
        size: take(size, (n) => new Float64Array(n)),
        modified: take(map.modified, (n) => new Float64Array(n)),
        growth: map.growth && { ...map.growth, previous: take(map.growth.previous, (n) => new Float64Array(n)) },
        attrs: take(map.attrs, (n) => new Uint32Array(n)),
        parent,
        names: index.map((i) => map.names[i]),
        paths: index.map((i) => map.paths[i]),
        first,
        end,
    };
}

/** The map without the items at `gone` (and what's in them), its folders' sizes reduced to match. */
export function pruneUsage(map: UsageMap, gone: ReadonlySet<string>): UsageMap {
    const place = new Int32Array(map.count).fill(-1);
    const size = Float64Array.from(map.size);
    let kept = 0;
    for (let i = 0; i < map.count; i++) {
        if (i > 0 && (place[map.parent[i]] < 0 || gone.has(map.paths[i]))) {
            // Only the topmost item removed takes its size off the folders above it.
            if (place[map.parent[i]] >= 0) for (let p = map.parent[i]; p !== NO_PARENT; p = map.parent[p]) size[p] -= map.size[i];
            continue;
        }
        place[i] = kept++;
    }
    return kept === map.count ? map : pick(map, place, kept, size);
}

/** The part of `map` below its folder `node`, as a map of that folder (one level shallower). */
export function subUsage(map: UsageMap, node: number): UsageMap {
    const place = new Int32Array(map.count).fill(-1);
    let kept = 0;
    place[node] = kept++;
    // Everything below a node comes after it.
    for (let i = node + 1; i < map.count; i++) if (place[map.parent[i]] >= 0) place[i] = kept++;
    return { ...pick(map, place, kept), path: map.paths[node] };
}
