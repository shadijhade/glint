// Self-check for the explorer's pure helpers. Run: node src/lib/explorer.check.ts
import assert from "node:assert/strict";
import { crumbs, isRoot, joinPath, parentPath, samePath, THIS_PC } from "./paths.ts";
import { decodeListing, type FileEntry, matcher, naturalCompare, sortEntries, terms, withDetails } from "./entries.ts";
import { formatDate, formatDuration, formatSize } from "./format.ts";
import { decodeThumb, gridColumns, gridMove, thumbStep } from "./grid.ts";
import { frecency, fuzzyScore } from "./rank.ts";
import { BLOCK, decodeUsage, FRAME, hitTest, layoutUsage, NO_PARENT, pruneUsage, squarify, subUsage, usageEntries, type UsageMap } from "./usage.ts";

// Paths
assert.equal(joinPath("C:\\", "Users"), "C:\\Users");
assert.equal(joinPath("C:\\Users", "me"), "C:\\Users\\me");
assert.equal(joinPath("/home", "me"), "/home/me");
assert.equal(parentPath("C:\\Users\\me"), "C:\\Users");
assert.equal(parentPath("C:\\Users"), "C:\\");
assert.equal(parentPath("C:\\"), THIS_PC);
assert.equal(parentPath("\\\\server\\share\\docs"), "\\\\server\\share");
assert.equal(parentPath("\\\\server\\share"), THIS_PC);
assert.ok(isRoot("D:") && isRoot("D:\\") && isRoot("\\\\nas\\media") && !isRoot("C:\\Users"));
assert.ok(samePath("c:\\users\\", "C:\\Users"));
assert.deepEqual(crumbs("C:\\Users\\me").map((c) => c.path), ["C:\\", "C:\\Users", "C:\\Users\\me"]);
assert.deepEqual(crumbs("\\\\nas\\media\\tv").map((c) => c.name), ["\\\\nas\\media", "tv"]);
assert.deepEqual(crumbs(THIS_PC), []);

// Natural order and sorting
const entry = (name: string, extra: Partial<FileEntry> = {}) =>
    withDetails({ name, dir: name.endsWith("/"), size: 0, modified: 0, attrs: 0, ...extra, ...(name.endsWith("/") ? { name: name.slice(0, -1) } : {}) }, "C:\\x");
assert.ok(naturalCompare("img9", "img10") < 0);
assert.ok(naturalCompare("v1.10", "v1.9") > 0);
assert.ok(naturalCompare("file02", "file2") === 0);
const listing = [entry("b/"), entry("a.txt", { size: 30, modified: 3 }), entry("c.md", { size: 10, modified: 1 }), entry("B.txt", { size: 20, modified: 2 })];
assert.deepEqual(listing.map((e) => e.path), ["C:\\x\\b", "C:\\x\\a.txt", "C:\\x\\c.md", "C:\\x\\B.txt"]);
assert.equal(listing[1].ext, "txt");
assert.equal(listing[0].ext, "");
const names = (list: FileEntry[]) => list.map((e) => e.name).join(",");
const presorted = sortEntries(listing, { key: "name", desc: false }, true, false);
assert.equal(names(presorted), "b,a.txt,B.txt,c.md");
assert.equal(sortEntries(presorted, { key: "name", desc: false }, true, true), presorted, "default view must not copy");
assert.equal(names(sortEntries(presorted, { key: "name", desc: true }, true, true)), "b,c.md,B.txt,a.txt");
assert.equal(names(sortEntries(presorted, { key: "size", desc: true }, true, true)), "b,a.txt,B.txt,c.md");
assert.equal(names(sortEntries(presorted, { key: "modified", desc: false }, false, true)), "b,c.md,B.txt,a.txt");
assert.equal(names(sortEntries(presorted, { key: "type", desc: false }, true, true)), "b,c.md,a.txt,B.txt");
assert.equal(names(sortEntries(presorted, { key: "size", desc: true }, true, true, (e) => (e.dir ? 100 : e.size))), "b,a.txt,B.txt,c.md");

// Matching mirrors fd's: all terms, case-insensitive, globs anchor to the whole name
assert.deepEqual(terms(`report "q3 final"  pdf`), ["report", "q3 final", "pdf"]);
const match = (query: string) => names(listing.filter(matcher(query) ?? (() => true)));
assert.equal(match("TXT"), "a.txt,B.txt");
assert.equal(match("*.md"), "c.md");
assert.equal(match("b txt"), "B.txt");
assert.equal(match("b"), "b,B.txt");
assert.equal(match("?.txt"), "a.txt,B.txt");
assert.equal(match("*.t"), "");
assert.equal(matcher("   "), null);

// Binary listings: built here with the layout fsx.rs's encode_listing writes (its test checks the offsets)
type Item = { name: string; dir: boolean; size: number; modified: number; attrs: number };
function encodeListing(path: string, items: Item[], elapsed: number) {
    const encoder = new TextEncoder();
    const pathBytes = encoder.encode(path);
    const names = encoder.encode(items.map((e) => e.name).join(""));
    const n = items.length;
    const buffer = new ArrayBuffer(16 + 24 * n + pathBytes.length + names.length);
    const view = new DataView(buffer);
    view.setUint32(0, n, true);
    view.setUint32(4, pathBytes.length, true);
    view.setFloat64(8, elapsed, true);
    items.forEach((e, i) => {
        view.setFloat64(16 + 8 * i, e.size, true);
        view.setFloat64(16 + 8 * n + 8 * i, e.modified, true);
        view.setUint32(16 + 16 * n + 4 * i, e.attrs | (e.dir ? 0x10 : 0), true);
        view.setUint32(16 + 20 * n + 4 * i, e.name.length, true);
    });
    new Uint8Array(buffer).set(pathBytes, 16 + 24 * n);
    new Uint8Array(buffer).set(names, 16 + 24 * n + pathBytes.length);
    return buffer;
}
const decoded = decodeListing(
    encodeListing("C:\\x", [{ name: "sub", dir: true, size: 0, modified: 5, attrs: 0 }, { name: "é😀.txt", dir: false, size: 2 ** 40, modified: 1.7e12, attrs: 2 }], 1.5)
);
assert.equal(decoded.path, "C:\\x");
assert.equal(decoded.elapsed, 1.5);
assert.deepEqual(
    decoded.entries.map((e) => [e.name, e.path, e.dir, e.size, e.modified, e.attrs, e.ext]),
    [
        ["sub", "C:\\x\\sub", true, 0, 5, 0x10, ""],
        ["é😀.txt", "C:\\x\\é😀.txt", false, 2 ** 40, 1.7e12, 2, "txt"],
    ]
);
assert.deepEqual(decodeListing(encodeListing("D:\\", [], 0)).entries, []);

// Disk-usage maps: built here with the layout usage.rs's encode_map writes (its test checks the offsets)
type Node = { name: string; parent: number; dir: boolean; size: number; modified?: number; attrs?: number };
function encodeMap(path: string, nodes: Node[], elapsed: number, engine: number, flags: number) {
    const encoder = new TextEncoder();
    const pathBytes = encoder.encode(path);
    const names = encoder.encode(nodes.map((n) => n.name).join(""));
    const n = nodes.length;
    const buffer = new ArrayBuffer(24 + 28 * n + pathBytes.length + names.length);
    const view = new DataView(buffer);
    view.setUint32(0, n, true);
    view.setUint32(4, pathBytes.length, true);
    view.setFloat64(8, elapsed, true);
    view.setUint32(16, engine, true);
    view.setUint32(20, flags, true);
    nodes.forEach((node, i) => {
        view.setFloat64(24 + 8 * i, node.size, true);
        view.setFloat64(24 + 8 * n + 8 * i, node.modified ?? 0, true);
        view.setUint32(24 + 16 * n + 4 * i, (node.attrs ?? 0) | (node.dir ? 0x10 : 0), true);
        view.setUint32(24 + 20 * n + 4 * i, node.parent, true);
        view.setUint32(24 + 24 * n + 4 * i, node.name.length, true);
    });
    new Uint8Array(buffer).set(pathBytes, 24 + 28 * n);
    new Uint8Array(buffer).set(names, 24 + 28 * n + pathBytes.length);
    return buffer;
}
// C:\ 1000: Windows 600 (System32 400 (big.dll 300), notes.txt 50), Users 300 (é😀.mkv 250), pagefile.sys 60;
// 40 bytes of smaller items directly in C:\, 150 in Windows, 100 in System32, 50 in Users.
const sample: Node[] = [
    { name: "", parent: NO_PARENT, dir: true, size: 1000 },
    { name: "Windows", parent: 0, dir: true, size: 600 },
    { name: "Users", parent: 0, dir: true, size: 300 },
    { name: "pagefile.sys", parent: 0, dir: false, size: 60, attrs: 0x26 },
    { name: "System32", parent: 1, dir: true, size: 400 },
    { name: "notes.txt", parent: 1, dir: false, size: 50, modified: 1.7e12 },
    { name: "é😀.mkv", parent: 2, dir: false, size: 250 },
    { name: "big.dll", parent: 4, dir: false, size: 300 },
];
const usage = decodeUsage(encodeMap("C:\\", sample, 3.5, 0, 1));
assert.deepEqual([usage.path, usage.engine, usage.elapsed, usage.limited, usage.serviceTooOld, usage.count], ["C:\\", "index", 3.5, true, false, 8]);
assert.deepEqual(usage.paths, ["C:\\", "C:\\Windows", "C:\\Users", "C:\\pagefile.sys", "C:\\Windows\\System32", "C:\\Windows\\notes.txt", "C:\\Users\\é😀.mkv", "C:\\Windows\\System32\\big.dll"]);
assert.deepEqual([...usage.first], [1, 4, 6, 0, 7, 0, 0, 0]);
assert.deepEqual([...usage.end], [4, 6, 7, 0, 8, 0, 0, 0]);
// dust drew it, because the index service is too old to map.
const fallback = decodeUsage(encodeMap("D:\\x", sample.slice(0, 1), 0, 1, 2));
assert.deepEqual([fallback.engine, fallback.limited, fallback.serviceTooOld], ["dust", false, true]);
const mapped = usageEntries(usage);
assert.deepEqual([mapped[3].name, mapped[3].ext, mapped[3].attrs, mapped[4].dir, mapped[5].modified, mapped[6].ext], ["pagefile.sys", "sys", 0x26, true, 1.7e12, "mkv"]);

// Squarified layout: every value gets its share of the box, nothing overlaps, nothing is left over
const boxes: number[][] = [];
squarify([6, 6, 4, 3, 2, 2, 1], 0, 0, 6, 4, (k, x, y, w, h) => (boxes[k] = [x, y, w, h]));
const near = (a: number, b: number) => Math.abs(a - b) < 1e-6;
boxes.forEach(([, , w, h], k) => assert.ok(near(w * h, [6, 6, 4, 3, 2, 2, 1][k]), `area of ${k}`));
for (let a = 0; a < boxes.length; a++) {
    for (let b = a + 1; b < boxes.length; b++) {
        const [ax, ay, aw, ah] = boxes[a];
        const [bx, by, bw, bh] = boxes[b];
        assert.ok(ax + aw <= bx + 1e-6 || bx + bw <= ax + 1e-6 || ay + ah <= by + 1e-6 || by + bh <= ay + 1e-6, `${a} and ${b} overlap`);
    }
}
// The first two (the biggest) share the first row along the short side, as squares would.
assert.deepEqual(boxes[0].map((v) => +v.toFixed(6)), [0, 0, 3, 2]);
assert.deepEqual(boxes[1].map((v) => +v.toFixed(6)), [0, 2, 3, 2]);

const layout = layoutUsage(usage, 800, 500);
const within = (i: number, p: number) =>
    layout.x[i] >= layout.x[p] - 1e-3 && layout.y[i] >= layout.y[p] - 1e-3 && layout.x[i] + layout.w[i] <= layout.x[p] + layout.w[p] + 1e-3 && layout.y[i] + layout.h[i] <= layout.y[p] + layout.h[p] + 1e-3;
assert.deepEqual([layout.x[0], layout.y[0], layout.w[0], layout.h[0]], [0, 0, 800, 500]);
for (let i = 1; i < usage.count; i++) assert.ok(within(i, usage.parent[i]), `${usage.names[i]} inside its folder`);
assert.deepEqual([...layout.kind], [FRAME, FRAME, FRAME, BLOCK, FRAME, BLOCK, BLOCK, BLOCK]);
assert.deepEqual([...layout.order], [0, 1, 4, 7, 5, 2, 6, 3], "frames before what's inside them");
// The folder mapped is split by size exactly: Windows 60%, Users 30%, pagefile.sys 6%, smaller items 4%.
const area = (i: number) => layout.w[i] * layout.h[i];
assert.ok(near(area(1) / 400_000, 0.6) && near(area(2) / 400_000, 0.3) && near(area(3) / 400_000, 0.06));
assert.ok(near((layout.restW[0] * layout.restH[0]) / 400_000, 0.04));
assert.ok(layout.header[1] > 0, "big frames have a header");
// Smaller items fill what's left of a frame: System32's 100 of 400 is a quarter of its inside.
assert.ok(near((layout.restW[4] * layout.restH[4]) / (area(7) + layout.restW[4] * layout.restH[4]), 0.25));

// What's under the pointer: the innermost item; on a frame's header, the frame; on smaller items, its folder
const centre = (i: number) => [layout.x[i] + layout.w[i] / 2, layout.y[i] + layout.h[i] / 2] as const;
assert.deepEqual(hitTest(usage, layout, ...centre(7)), { node: 7, rest: false });
assert.deepEqual(hitTest(usage, layout, ...centre(3)), { node: 3, rest: false });
assert.deepEqual(hitTest(usage, layout, layout.x[1] + 5, layout.y[1] + 5), { node: 1, rest: false });
assert.deepEqual(hitTest(usage, layout, layout.restX[4] + layout.restW[4] / 2, layout.restY[4] + layout.restH[4] / 2), { node: 4, rest: true });
assert.equal(hitTest(usage, layout, 900, 10), null);
// A box too small for anything inside draws its folder whole.
const tiny = layoutUsage(usage, 30, 20);
assert.ok(tiny.kind[1] === BLOCK || tiny.kind[1] === 0);
assert.equal(tiny.kind[7], 0);
assert.equal(layoutUsage(usage, 0, 0).order.length, 0);

// Deleting from the map: the item and what's in it go, and every folder above it shrinks
const pruned: UsageMap = pruneUsage(usage, new Set(["C:\\Windows\\System32"]));
assert.deepEqual(pruned.names, ["", "Windows", "Users", "pagefile.sys", "notes.txt", "é😀.mkv"]);
assert.deepEqual([...pruned.size], [600, 200, 300, 60, 50, 250]);
assert.deepEqual([...pruned.parent], [NO_PARENT, 0, 0, 0, 1, 2]);
assert.deepEqual([...pruned.first], [1, 4, 5, 0, 0, 0]);
assert.deepEqual([...pruned.end], [4, 5, 6, 0, 0, 0]);
assert.equal(pruneUsage(usage, new Set(["C:\\nothing"])), usage);

// Going into a folder of a map: its part of the map, as a map of that folder
const sub = subUsage(usage, 1);
assert.deepEqual([sub.path, sub.count, sub.names], ["C:\\Windows", 4, ["Windows", "System32", "notes.txt", "big.dll"]]);
assert.deepEqual([...sub.parent], [NO_PARENT, 0, 0, 1]);
assert.deepEqual([...sub.size], [600, 400, 50, 300]);
assert.deepEqual([[...sub.first], [...sub.end]], [[1, 3, 0, 0], [3, 4, 0, 0]]);
assert.deepEqual(subUsage(usage, 3).names, ["pagefile.sys"]);

// Grid view: arrows move by tiles and rows (Left and Right only in a grid), never off the items
// 0  1  2  3
// 4  5  6  7
// 8  9
const move = (key: string, at: number, cols = 4, count = 10) => gridMove(key, at, count, cols, 2);
assert.deepEqual([move("ArrowRight", 3), move("ArrowLeft", 4), move("ArrowRight", 9), move("ArrowLeft", 0)], [4, 3, 9, 0]);
assert.deepEqual([move("ArrowDown", 1), move("ArrowDown", 5), move("ArrowDown", 6), move("ArrowDown", 9)], [5, 9, 9, 9], "into the short last row: its last item");
assert.deepEqual([move("ArrowUp", 9), move("ArrowUp", 2), move("ArrowDown", -1), move("ArrowRight", -1)], [5, 2, 0, 0]);
assert.deepEqual([move("PageDown", 1), move("PageDown", 4), move("PageUp", 9), move("Home", 6), move("End", 2)], [9, 9, 1, 0, 9]);
assert.equal(move("Enter", 2), null);
assert.deepEqual([move("ArrowRight", 2, 1), move("ArrowDown", 2, 1), move("ArrowUp", 0, 1), move("PageDown", -1, 1)], [null, 3, 0, 1], "the list: one column");
assert.equal(gridMove("ArrowDown", -1, 0, 4, 2), null);
assert.deepEqual([gridColumns(1000, 120), gridColumns(100, 120), gridColumns(0, 120)], [8, 1, 1]);
// Thumbnails are asked for in device pixels, one to one: 96 CSS pixels at 125% asks for 120.
assert.deepEqual([thumbStep(96, 1), thumbStep(96, 1.25), thumbStep(96, 1.5), thumbStep(16, 1.25), thumbStep(14, 1.75), thumbStep(900, 2), thumbStep(4, 1)], [96, 120, 144, 20, 28, 1024, 16]);
// Answers from thumbs.rs: an 8-byte header, then the BMP
const answer = new Uint8Array([0, 2, 0, 0, ...new Uint8Array(new Float32Array([2.5]).buffer), 66, 77]);
const thumb = decodeThumb(answer.buffer);
assert.deepEqual([thumb.source, thumb.ms, [...thumb.bmp!]], ["made", 2.5, [66, 77]]);
const none = decodeThumb(new Uint8Array([1, 1, 1, 0, 0, 0, 0, 0]).buffer);
assert.deepEqual([none.bmp, none.source, none.lasting], [null, "cache", true]);
assert.equal(decodeThumb(new Uint8Array([1, 1, 0, 0, 0, 0, 0, 0]).buffer).lasting, false, "none yet: asked again later");

// Command palette ranking: in-order letters, word starts and runs first; visits weighted by recency
const best = (query: string, options: string[]) =>
    options.map((o) => ({ o, s: fuzzyScore(query, o) })).filter((x) => x.s !== null).sort((a, b) => b.s! - a.s!).map((x) => x.o);
assert.equal(fuzzyScore("xyz", "Downloads"), null);
assert.equal(fuzzyScore("", "anything"), 0);
assert.deepEqual(best("dl", ["Desktop", "Downloads", "Videos"]), ["Downloads"]);
assert.deepEqual(best("doc", ["Dropbox config", "Documents", "src\\docs-old"]), ["Documents", "src\\docs-old", "Dropbox config"]);
assert.deepEqual(best("nf", ["New folder", "Info notes"]), ["New folder", "Info notes"]);
assert.deepEqual(best("sh", ["Refresh", "Search here", "Show hidden items"]), ["Show hidden items", "Search here", "Refresh"]);
const now = 1_000_000_000_000;
assert.ok(frecency({ count: 3, last: now - 60_000 }, now) > frecency({ count: 3, last: now - 3 * 86_400_000 }, now));
assert.ok(frecency({ count: 10, last: now - 30 * 86_400_000 }, now) > frecency({ count: 1, last: now - 2 * 3_600_000 }, now));

// Formatting
assert.equal(formatSize(0), "0 B");
assert.equal(formatSize(1536), "1.5 KB");
assert.equal(formatSize(5 * 1024 ** 3), "5.0 GB");
assert.equal(formatSize(245_127_203_117), "228 GB");
assert.equal(formatDuration(0.42), "0.4 ms");
assert.equal(formatDuration(1234), "1.23 s");
const noon = new Date(2026, 9, 14, 12, 0).getTime();
assert.match(formatDate(noon - 3_600_000, noon), /^Today /);
assert.match(formatDate(noon - 86_400_000, noon), /^Yesterday /);
assert.match(formatDate(new Date(2026, 0, 2, 9, 5).getTime(), noon), /2026/);
assert.equal(formatDate(0), "");

console.log("explorer.check: all assertions passed");
