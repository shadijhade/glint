// Self-check for the explorer's pure helpers. Run: node src/lib/explorer.check.ts
import assert from "node:assert/strict";
import { crumbs, isRoot, joinPath, parentPath, samePath, THIS_PC } from "./paths.ts";
import { decodeListing, type FileEntry, matcher, naturalCompare, sortEntries, terms, withDetails } from "./entries.ts";
import { formatDate, formatDuration, formatSize } from "./format.ts";

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
