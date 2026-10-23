// Self-check for the explorer's pure helpers. Run: node src/lib/explorer.check.ts
import assert from "node:assert/strict";
import { crumbs, isRoot, joinPath, linkedPath, parentPath, samePath, THIS_PC, volumeOf } from "./paths.ts";
import { ATTR, compareEntries, decodeItems, decodeListing, type FileEntry, hasPropFilter, isArchived, isArchiveFile, isBrowsable, isClosed, isListOnly, isLocked, matcher, naturalCompare, propTest, sortEntries, terms, withDetails } from "./entries.ts";
import type { MediaFacts } from "./media.ts";
import { inArchivePath } from "./paths.ts";
import { cdCommand, isTyping } from "./cd.ts";
import { formatDate, formatDuration, formatSize } from "./format.ts";
import { decodeThumb, gridColumns, gridMove, keepPlace, placeOf, thumbStep, tileLayout, topOf, zoomTiles } from "./grid.ts";
import { editsAllowed, frecency, fuzzyScore, typoDistance } from "./rank.ts";
import { markdownTitle, safeHref } from "./markdown.ts";
import { initiallyOpen, isContainer, JsonNumber, jqPath, PAGE, parseJson, preview, treeRows } from "./jsonTree.ts";
import { byteChar, byteClass, hex, parseOffset } from "./hex.ts";
import { clock, outputUntil, parseCast } from "./cast.ts";
import {
    BLOCK,
    decodeUsage,
    FRAME,
    grewBy,
    growthLeaders,
    hitTest,
    layoutUsage,
    NO_PARENT,
    pruneUsage,
    squarify,
    subUsage,
    usageEntries,
    type UsageMap,
    WAS_NOT_THERE,
    WRITTEN_SINCE,
} from "./usage.ts";

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

// Links in Markdown: relative to its folder, / to the repository, never to another scheme or a share by name
const repo = "C:\\src\\app";
const docs = "C:\\src\\app\\docs";
assert.equal(linkedPath("guide.md", docs, repo), "C:\\src\\app\\docs\\guide.md");
assert.equal(linkedPath("./img/a%20b.png?raw=true#x", docs, repo), "C:\\src\\app\\docs\\img\\a b.png");
assert.equal(linkedPath("../../../../x.md", docs, repo), "C:\\x.md");
assert.equal(linkedPath("/LICENSE", docs, repo), "C:\\src\\app\\LICENSE");
assert.equal(linkedPath("\\\\evil\\share\\pic.png", docs, repo), "C:\\src\\app\\evil\\share\\pic.png");
assert.equal(linkedPath("D:/notes/todo.md", docs, repo), "D:\\notes\\todo.md");
assert.equal(linkedPath("https://example.com/a.png", docs, repo), null);
assert.equal(linkedPath("mailto:me@example.com", docs, repo), null);
assert.equal(linkedPath("#heading", docs, repo), null);
assert.equal(linkedPath("100%.md", docs, repo), "C:\\src\\app\\docs\\100%.md");
assert.equal(linkedPath("pic.png", "\\\\nas\\media\\docs", "\\\\nas\\media\\"), "\\\\nas\\media\\docs\\pic.png");
assert.equal(volumeOf("\\\\nas\\media\\docs\\a"), "\\\\nas\\media");
assert.equal(volumeOf("c:\\x"), "c:");
assert.equal(safeHref("javascript:alert(1)"), null);
assert.equal(safeHref(" JavaScript:alert(1)"), null);
assert.equal(safeHref("data:text/html,<b>"), null);
assert.equal(safeHref("file:///C:/x"), null);
assert.equal(safeHref("https://example.com"), "https://example.com");
assert.equal(safeHref("#install"), "#install");
assert.equal(safeHref("docs/a.md"), "docs/a.md");
assert.equal(safeHref("C:\\notes\\a.md"), "C:\\notes\\a.md");
assert.equal(safeHref(""), null);

// A README's title, for its folded card
assert.equal(markdownTitle('<div align="center">\n\n<img src="logo.svg" width="96" alt="Glint logo">\n\n# Glint\n\n**A file explorer**'), "Glint");
assert.equal(markdownTitle("Project *Name*\n=====\n\nText"), "Project Name");
assert.equal(markdownTitle('<h1 align="center">My <b>App</b></h1>'), "My App");
assert.equal(markdownTitle("[![build](https://x/badge.svg)](https://x)\n\nA tool that does `things`.\n\n## Install"), "A tool that does things.");
assert.equal(markdownTitle("# [Docs](https://example.com) #\n"), "Docs");
assert.equal(markdownTitle("\n\n"), "");

// The JSON tree: exact numbers, jq paths, rows a page at a time
const doc = parseJson('{"name":"glint","big":123456789012345678901234567890,"price":1.50,"n":3,"tools":["fd","rg"],"odd key":{"x":[]}}');
assert.ok(isContainer(doc) && !Array.isArray(doc));
assert.ok(doc.big instanceof JsonNumber && String(doc.big) === "123456789012345678901234567890");
assert.ok(doc.price instanceof JsonNumber && String(doc.price) === "1.50");
assert.equal(doc.n, 3);
assert.equal(jqPath(".", "name"), ".name");
assert.equal(jqPath(".", 2), ".[2]");
assert.equal(jqPath(".tools", 1), ".tools[1]");
assert.equal(jqPath(".", "odd key"), '.["odd key"]');
assert.equal(jqPath('.["odd key"]', "x"), '.["odd key"].x');
const opened = initiallyOpen([doc]);
assert.deepEqual([...opened.keys()], ["0:.", "0:.tools", '0:.["odd key"]']);
assert.deepEqual(
    treeRows([doc], opened).map((r) => `${"  ".repeat(r.depth)}${r.key ?? "."}`),
    [".", "  name", "  big", "  price", "  n", "  tools", "    0", "    1", "  odd key", "    x"]
);
assert.equal(preview(doc, 40), '{name: "glint", …}');
assert.equal(preview(["a", 1, null, { b: 2 }]), '["a", 1, null, {…}]');
assert.equal(preview({ "a b": "x".repeat(100) }, 30), '{…}');
const long = Array.from({ length: PAGE + 5 }, (_, i) => i);
const paged = treeRows([long], new Map([["0:.", PAGE]]));
assert.equal(paged.length, PAGE + 2);
assert.equal(paged[paged.length - 1].more, 5);
assert.equal(treeRows([long], new Map([["0:.", PAGE * 2]])).length, PAGE + 6);

// The hex view: hexyl's byte classes and characters, and offsets to go to
assert.deepEqual([0, 0x20, 0x0a, 0x41, 0x7f, 0x01, 0x80, 0xff].map(byteClass), ["null", "space", "space", "text", "control", "control", "high", "high"]);
assert.equal([0, 0x20, 0x09, 0x41, 0x1b, 0xe9].map(byteChar).join(""), "⋄ _A•×");
assert.equal(hex(255, 2), "FF");
assert.equal(hex(0x1f00, 8), "00001F00");
assert.equal(parseOffset("0x1F00"), 0x1f00);
assert.equal(parseOffset("1f00h"), 0x1f00);
assert.equal(parseOffset("1F00"), 0x1f00);
assert.equal(parseOffset("8192"), 8192);
assert.equal(parseOffset(" 1_000 "), 1000);
assert.equal(parseOffset("12g"), null);
assert.equal(parseOffset(""), null);

// asciinema recordings: version 2 (times from the start) and 3 (since the last event), long pauses cut
const v2 = parseCast('{"version": 2, "width": 100, "height": 30, "title": "demo"}\n[0.5, "o", "$ "]\n[1.0, "i", "l"]\n[1.5, "o", "ls\\r\\n"]\n[60.0, "o", "done"]\nnot json\n');
assert.ok(v2);
assert.deepEqual([v2.cols, v2.rows, v2.title], [100, 30, "demo"]);
assert.deepEqual(v2.events.map((e) => e.t), [0.5, 1.5, 4.5], "the 58.5 s pause plays as 3");
assert.equal(v2.duration, 4.5);
assert.deepEqual(outputUntil(v2, 1.5), { data: "$ ls\r\n", count: 2 });
const v3 = parseCast('{"version": 3, "term": {"cols": 80, "rows": 24}, "idle_time_limit": 1}\n[0.2, "o", "a"]\n[5, "o", "b"]\n[0.3, "r", "90x30"]\n[0.1, "o", "c"]\n');
assert.ok(v3);
assert.deepEqual(v3.events.map((e) => [Math.round(e.t * 1000) / 1000, e.data]), [[0.2, "a"], [1.2, "b"], [1.6, "c"]]);
assert.equal(parseCast('{"version": 1}'), null);
assert.equal(parseCast("plain text"), null);
assert.equal(clock(65.4), "1:05");
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
// Date taken: a photo's when it says, else the date modified.
const shots = [entry("a.jpg", { modified: 50 }), entry("b.jpg", { modified: 10 }), entry("c.txt", { modified: 30 })];
assert.equal(names(sortEntries(shots, { key: "taken", desc: false }, true, false, undefined, (e) => (e.name === "a.jpg" ? 5 : undefined))), "a.jpg,b.jpg,c.txt");
assert.equal(names(sortEntries(shots, { key: "taken", desc: true }, true, false, undefined, (e) => (e.name === "a.jpg" ? 5 : null))), "c.txt,b.jpg,a.jpg");

// Dual-pane comparison
assert.equal(compareEntries(entry("a.txt", { size: 10, modified: 5000 }), undefined), "unique");
assert.equal(compareEntries(entry("a.txt", { size: 10, modified: 8000 }), entry("a.txt", { size: 10, modified: 5000 })), "newer");
assert.equal(compareEntries(entry("a.txt", { size: 10, modified: 5000 }), entry("a.txt", { size: 10, modified: 8000 })), "older");
assert.equal(compareEntries(entry("a.txt", { size: 20, modified: 5000 }), entry("a.txt", { size: 10, modified: 5000 })), "differ");
assert.equal(compareEntries(entry("a.txt", { size: 10, modified: 5000 }), entry("a.txt", { size: 10, modified: 5500 })), "same");
assert.equal(compareEntries(entry("sub/"), entry("sub/"), 100, 200), "differ");

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
// Filters, as the index and fd read them: a.txt is 30 B, B.txt 20 B, c.md 10 B; b is a folder.
assert.equal(match("ext:TXT"), "a.txt,B.txt");
assert.equal(match("size:>=20 ext:txt;md"), "a.txt,B.txt");
assert.equal(match("size:<20"), "c.md");
assert.equal(match("folder:"), "b");
assert.equal(match("file: b"), "B.txt");
assert.equal(match("dm:>1y"), "b,a.txt,c.md,B.txt");
assert.equal(match("dm:7d"), "");
assert.equal(match("size:big"), "");
// git: asks the folder's Git marks (by lowercase name); without them (not a repository) it matches nothing.
const marks = { "a.txt": "modified", "b.txt": "untracked", b: "staged", "c.md": "ignored" } as const;
const gitMatch = (query: string) => names(listing.filter(matcher(query, marks) ?? (() => true)));
assert.equal(gitMatch("git:changed"), "b,a.txt,B.txt");
assert.equal(gitMatch("git:new"), "B.txt");
assert.equal(gitMatch("git:staged"), "b");
assert.equal(gitMatch("git:ignored"), "c.md");
assert.equal(gitMatch("git:changed ext:txt"), "a.txt,B.txt");
assert.equal(gitMatch("git:nonsense"), "");
assert.equal(match("git:changed"), "");

// What photos, videos and songs say, as media.rs checks it (query.rs's and media.rs's cases)
const day = (y: number, m: number, d: number) => new Date(y, m - 1, d).getTime();
const photo: MediaFacts = { taken: day(2024, 6, 15) + 3_600_000, width: 4032, height: 3024, duration: null, camera: "canon eos r5", artist: null, album: null };
const song: MediaFacts = { taken: null, width: null, height: null, duration: 187.4, camera: null, artist: "the beatles", album: "abbey road" };
const today = day(2026, 10, 23);
const holdAll = (facts: MediaFacts, query: string) =>
    terms(query.toLowerCase()).every((t) => {
        const [, key, value] = /^(\w+):(.*)$/.exec(t)!;
        const test = propTest(key, value, today);
        return !!test && test(facts);
    });
assert.ok(holdAll(photo, "width:>4000 height:>=3024 camera:eos"));
assert.ok(!holdAll(photo, "width:>4032"));
assert.ok(holdAll(photo, "taken:2024 taken:2024-06 taken:2024-06-15 taken:>2023 taken:<2025-01 taken:>1y"));
assert.ok(!holdAll(photo, "taken:2024-06-16") && !holdAll(photo, "taken:>2024") && !holdAll(photo, "taken:30d"));
assert.ok(holdAll(song, `duration:>3m duration:<=3:07 artist:beatles album:"abbey road"`));
assert.ok(!holdAll(song, "duration:>3:07") && !holdAll(song, "width:>1") && !holdAll(photo, "artist:"));
assert.ok(holdAll(song, "album:"), "a word left out: any album");
for (const bad of ["width:wide", "width:<0", "height:>", "duration:5x", "duration:1:300", "taken:2023-02-29", "taken:2024-13", "taken:24", "taken:yesterday"]) {
    const [, key, value] = /^(\w+):(.*)$/.exec(bad)!;
    assert.equal(propTest(key, value), null, bad);
}
assert.ok(hasPropFilter("holiday Width:>10") && !hasPropFilter("holiday ext:jpg"));
// In a folder, a file shows once what it says is read; in search results (no facts) the search checked them.
const pictures = [entry("big.png", { size: 1 }), entry("small.png", { size: 1 }), entry("unread.png", { size: 1 }), entry("notes.txt", { size: 1 })];
const said = new Map<string, MediaFacts | null>([["big.png", { ...photo, width: 3000 }], ["small.png", { ...photo, width: 800 }], ["notes.txt", null]]);
assert.equal(names(pictures.filter(matcher("width:>1920", undefined, (e) => said.get(e.name))!)), "big.png");
assert.equal(names(pictures.filter(matcher("width:>1920 big")!)), "big.png", "without facts, left to the search");

// The terminal's cd, as each shell takes it; its own reports aren't typing
assert.equal(cdCommand("pwsh", "C:\\It's mine"), "Set-Location -LiteralPath 'C:\\It''s mine'\r");
assert.equal(cdCommand("cmd", "D:\\Work Files"), 'cd /d "D:\\Work Files"\r');
assert.equal(cdCommand("bash", "C:\\it's"), `cd "$(cygpath -u 'C:\\it'\\''s')"\r`);
assert.equal(cdCommand("nu", "C:\\a b"), 'cd "C:\\\\a b"\r');
assert.ok(isTyping("ls\r") && isTyping("\x1b[A") && !isTyping("\x1b[I") && !isTyping("\x1b[O") && !isTyping("\x1b[12;40R"));

// Binary listings: built here with the layout fsx.rs's encode_listing writes (its test checks the offsets)
type Item = { name: string; dir: boolean; size: number; modified: number; attrs: number };
type InArchiveHeader = { archive: number; outer: number; entries: number; cached: boolean; readMs: number; format: number };
function encodeListing(path: string, items: Item[], elapsed: number, archive?: InArchiveHeader, fromIndex = false) {
    const encoder = new TextEncoder();
    const pathBytes = encoder.encode(path);
    const names = encoder.encode(items.map((e) => e.name).join(""));
    const n = items.length;
    const buffer = new ArrayBuffer(40 + 24 * n + pathBytes.length + names.length);
    const view = new DataView(buffer);
    view.setUint32(0, n, true);
    view.setUint32(4, pathBytes.length, true);
    view.setFloat64(8, elapsed, true);
    if (archive) {
        view.setUint32(16, archive.archive, true);
        view.setUint32(20, archive.outer, true);
        view.setUint32(24, archive.entries, true);
        view.setFloat64(32, archive.readMs, true);
    }
    view.setUint32(28, (archive?.cached ? 1 : 0) | (fromIndex ? 2 : 0) | (archive ? (archive.format + 1) << 8 : 0), true);
    items.forEach((e, i) => {
        view.setFloat64(40 + 8 * i, e.size, true);
        view.setFloat64(40 + 8 * n + 8 * i, e.modified, true);
        view.setUint32(40 + 16 * n + 4 * i, e.attrs | (e.dir ? 0x10 : 0), true);
        view.setUint32(40 + 20 * n + 4 * i, e.name.length, true);
    });
    new Uint8Array(buffer).set(pathBytes, 40 + 24 * n);
    new Uint8Array(buffer).set(names, 40 + 24 * n + pathBytes.length);
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
assert.equal(decoded.archive, undefined, "a folder on disk");
assert.equal(decoded.fromIndex, undefined, "a folder on disk");

// A folder Windows keeps closed, listed from instant search's index: the listing says so, and so does each item
const closed = decodeListing(
    encodeListing("C:\\PerfLogs", [{ name: "System", dir: true, size: 0, modified: 5, attrs: ATTR.closed }, { name: "trace.etl", dir: false, size: 7, modified: 6, attrs: ATTR.closed | 0x20 }], 0.4, undefined, true)
);
assert.equal(closed.fromIndex, true);
assert.deepEqual(closed.entries.map((e) => [e.path, e.dir, isClosed(e)]), [["C:\\PerfLogs\\System", true, true], ["C:\\PerfLogs\\trace.etl", false, true]]);
assert.ok(!isClosed(entry("plain.txt")));

// A folder in an archive (here one in an archive in an archive): its listing says where the archives are
const inside = "C:\\Dö\\photos.zip\\2024\\old.7z\\summer";
const nested = decodeListing(
    encodeListing(inside, [{ name: "beach.jpg", dir: false, size: 300, modified: 1.7e12, attrs: ATTR.inArchive }, { name: "secret.txt", dir: false, size: 9, modified: 0, attrs: ATTR.inArchive | ATTR.encrypted }], 0.25, {
        archive: "C:\\Dö\\photos.zip\\2024\\old.7z".length,
        outer: "C:\\Dö\\photos.zip".length,
        entries: 54_001,
        cached: false,
        readMs: 13.5,
        format: 1,
    })
);
assert.deepEqual(nested.archive, { path: "C:\\Dö\\photos.zip\\2024\\old.7z", outer: "C:\\Dö\\photos.zip", format: "7z", entries: 54_001, cached: false, readMs: 13.5 });
assert.deepEqual(nested.entries.map((e) => [e.path, isArchived(e), isLocked(e)]), [
    [`${inside}\\beach.jpg`, true, false],
    [`${inside}\\secret.txt`, true, true],
]);
assert.ok(isArchiveFile(entry("x.Zip")) && isArchiveFile(entry("y.7z")) && !isArchiveFile(entry("z.zip/")) && !isArchiveFile(entry("w.docx")));
assert.ok(isArchiveFile(entry("backup.RAR")) && isArchiveFile(entry("drivers.cab")) && isArchiveFile(entry("disk.ncdu")) && isBrowsable(entry("setup.msi")) && !isArchiveFile(entry("setup.msi")));
// A file in an ncdu export has a name, a size and a date, and nothing to open; the bit means that only inside an archive.
assert.ok(isListOnly(entry("a.jpg", { attrs: ATTR.inArchive | ATTR.listOnly })) && !isListOnly(entry("b.jpg", { attrs: ATTR.listOnly })) && !isLocked(entry("c.jpg", { attrs: ATTR.inArchive | ATTR.listOnly })));
// A .docx or .jar is a zip that Enter opens in its app: "Open as archive" looks inside, and a path into it is an archive's.
assert.ok(isBrowsable(entry("report.DOCX")) && isBrowsable(entry("lib.jar")) && !isBrowsable(entry("x.zip")) && !isBrowsable(entry("dir.docx/")));
assert.ok(inArchivePath("C:\\a\\report.docx\\word") && inArchivePath("C:\\a\\b.7z") && !inArchivePath("C:\\a\\docx\\b") && !inArchivePath("C:\\a\\b.zipper"));
assert.ok(!isArchived(entry("plain.txt")));
// The archive's own top folder, and the crumbs into it: an archive's path is a folder path like any other.
assert.deepEqual(crumbs(inside).map((c) => c.name), ["C:\\", "Dö", "photos.zip", "2024", "old.7z", "summer"]);
assert.equal(parentPath("C:\\Dö\\photos.zip"), "C:\\Dö");

// Search results: items that each carry their own path, built with the layout fsx.rs's encode_items writes (its test checks the offsets)
function encodeItems(items: (Item & { path: string })[], withNames: boolean) {
    const text = new TextEncoder().encode(items.map((e) => e.path).join("") + (withNames ? items.map((e) => e.name).join("") : ""));
    const n = items.length;
    const buffer = new ArrayBuffer(40 + 28 * n + text.length);
    const view = new DataView(buffer);
    view.setUint32(0, n, true);
    view.setFloat64(8, 2.5, true);
    view.setUint32(24, withNames ? 1 : 0, true);
    view.setUint32(28, 4, true);
    items.forEach((e, i) => {
        view.setFloat64(40 + 8 * i, e.size, true);
        view.setFloat64(40 + 8 * n + 8 * i, e.modified, true);
        view.setUint32(40 + 16 * n + 4 * i, e.attrs | (e.dir ? 0x10 : 0), true);
        view.setUint32(40 + 20 * n + 4 * i, e.path.length, true);
        view.setUint32(40 + 24 * n + 4 * i, e.name.length, true);
    });
    new Uint8Array(buffer).set(text, 40 + 28 * n);
    return buffer;
}
const results = decodeItems(
    encodeItems(
        [
            { name: "😀 report.pdf", path: "C:\\Dö\\😀 report.pdf", dir: false, size: 5, modified: 1.7e12, attrs: 0x20 },
            { name: "Windows", path: "C:\\Windows", dir: true, size: 0, modified: 4, attrs: 0 },
            { name: "D:\\", path: "D:\\", dir: true, size: 0, modified: 0, attrs: 0 },
        ],
        false
    )
);
assert.equal(results.elapsed, 2.5);
assert.deepEqual(
    results.entries.map((e) => [e.name, e.path, e.dir, e.size, e.ext]),
    [
        ["😀 report.pdf", "C:\\Dö\\😀 report.pdf", false, 5, "pdf"],
        ["Windows", "C:\\Windows", true, 0, ""],
        ["D:\\", "D:\\", true, 0, ""],
    ]
);
// Names that aren't the end of their path (a Recycle Bin item's) follow the paths.
const renamed = decodeItems(encodeItems([{ name: "budget.xlsx", path: "C:\\$Recycle.Bin\\S-1-5\\$RQ2Z1.xlsx", dir: false, size: 9, modified: 1, attrs: 0 }], true));
assert.deepEqual(renamed.entries.map((e) => [e.name, e.path, e.ext]), [["budget.xlsx", "C:\\$Recycle.Bin\\S-1-5\\$RQ2Z1.xlsx", "xlsx"]]);
assert.deepEqual(decodeItems(encodeItems([], false)).entries, []);

// Disk-usage maps: built here with the layout usage.rs's encode_map writes (its test checks the offsets)
type Node = { name: string; parent: number; dir: boolean; size: number; modified?: number; attrs?: number };
/** `growth`: the mark's since, oldest, drive then and now, and each item's size then. */
function encodeMap(path: string, nodes: Node[], elapsed: number, engine: number, flags: number, growth?: { block: number[]; previous: number[] }) {
    const encoder = new TextEncoder();
    const pathBytes = encoder.encode(path);
    const names = encoder.encode(nodes.map((n) => n.name).join(""));
    const n = nodes.length;
    const buffer = new ArrayBuffer(24 + 28 * n + (growth ? 32 + 8 * n : 0) + pathBytes.length + names.length);
    const view = new DataView(buffer);
    view.setUint32(0, n, true);
    view.setUint32(4, pathBytes.length, true);
    view.setFloat64(8, elapsed, true);
    view.setUint32(16, engine, true);
    view.setUint32(20, flags | (growth ? 4 : 0), true);
    growth?.block.forEach((v, k) => view.setFloat64(24 + 8 * k, v, true));
    let at = growth ? 56 : 24;
    nodes.forEach((node, i) => {
        view.setFloat64(at + 8 * i, node.size, true);
        view.setFloat64(at + 8 * n + 8 * i, node.modified ?? 0, true);
    });
    at += 16 * n;
    if (growth) {
        growth.previous.forEach((v, i) => view.setFloat64(at + 8 * i, v, true));
        at += 8 * n;
    }
    nodes.forEach((node, i) => {
        view.setUint32(at + 4 * i, (node.attrs ?? 0) | (node.dir ? 0x10 : 0), true);
        view.setUint32(at + 4 * n + 4 * i, node.parent, true);
        view.setUint32(at + 8 * n + 4 * i, node.name.length, true);
    });
    at += 12 * n;
    new Uint8Array(buffer).set(pathBytes, at);
    new Uint8Array(buffer).set(names, at + pathBytes.length);
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
// A folder in an archive is mapped from the archive's own list of entries.
assert.deepEqual([2, 3].map((engine) => decodeUsage(encodeMap("C:\\a.zip", sample.slice(0, 1), 0, engine, 0)).engine), ["zip", "7z"]);
const mapped = usageEntries(usage);
assert.deepEqual([mapped[3].name, mapped[3].ext, mapped[3].attrs, mapped[4].dir, mapped[5].modified, mapped[6].ext], ["pagefile.sys", "sys", 0x26, true, 1.7e12, "mkv"]);
assert.equal(usage.growth, null);
// What grew since a mark comes with the dates: each item's size then (or that it wasn't there, or a
// file written since). Windows grew 400 from 200, all of it System32 (new); Users shrank by 50.
const grown = decodeUsage(encodeMap("C:\\", sample, 1, 0, 0, { block: [1e12, 9e11, 5000, 5300], previous: [700, 200, 350, 60, WAS_NOT_THERE, WRITTEN_SINCE, 250, 300] }));
assert.deepEqual([grown.growth?.since, grown.growth?.oldest, grown.growth?.driveThen, grown.growth?.driveNow], [1e12, 9e11, 5000, 5300]);
assert.deepEqual([grown.paths, [...grown.size]], [usage.paths, [...usage.size]]);
assert.deepEqual([0, 1, 2, 4, 5].map((i) => grewBy(grown, i)), [300, 400, -50, 400, 0]);
assert.deepEqual(growthLeaders(grown), [4], "System32 tells where Windows' growth went");
assert.deepEqual([...subUsage(grown, 1).growth!.previous], [200, WAS_NOT_THERE, WRITTEN_SINCE, 300]);
assert.deepEqual([...pruneUsage(grown, new Set(["C:\\Users"])).growth!.previous], [700, 200, 60, WAS_NOT_THERE, WRITTEN_SINCE, 300]);
assert.equal(decodeUsage(encodeMap("C:\\", sample, 1, 0, 8)).growthTooOld, true);

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
// Tile sizes: Ctrl+= and Ctrl+- step through them and stop at the ends; one this version doesn't know is medium.
assert.deepEqual([zoomTiles("medium", 1), zoomTiles("medium", -1), zoomTiles("small", -1), zoomTiles("huge", 1), zoomTiles("small", 3), zoomTiles("huge", -9)], ["large", "small", "small", "huge", "xlarge", "small"]);
assert.deepEqual([zoomTiles("enormous", 0), zoomTiles("enormous", 1)], ["medium", "large"]);
// Medium is the tile the grid always had (120 × 164 at 16 px text); names and margins keep their room at every size.
assert.deepEqual(tileLayout(16, "medium"), { width: 120, height: 164, thumb: 96, pad: 8 });
assert.deepEqual([tileLayout(14, "medium").width, tileLayout(14, "medium").height, tileLayout(18, "medium").height], [105, 144, 185]);
assert.deepEqual([tileLayout(16, "small"), tileLayout(16, "huge"), tileLayout(16, "?")], [{ width: 88, height: 132, thumb: 64, pad: 8 }, { width: 280, height: 324, thumb: 256, pad: 8 }, tileLayout(16, "medium")]);
// Keeping its place as the tiles zoom: 4 to a row of 100 px, the view 300 px tall, scrolled to the top.
const small = { cols: 4, rowHeight: 100 };
const big = { cols: 2, rowHeight: 200 };
// The focused tile (5, in the second row: its middle 150 px down) stays where it was.
assert.equal(keepPlace(small, big, 0, 300, 10, 5), 2 * 200 - 50);
// Zoomed out again, it stays put too.
assert.equal(keepPlace(big, small, 350, 300, 3, 5), 0);
// Off screen (or none focused): the tiles at the top stay at the top, as far into their row as before.
assert.equal(keepPlace(small, big, 250, 300, 10, 40), (4 + 0.5) * 200);
assert.equal(keepPlace(small, big, 250, 300, 10, -1), 900);
// A tile taller than the view goes to its top; the view never goes past either end.
assert.equal(keepPlace(small, { cols: 1, rowHeight: 500 }, 0, 300, 40, 2), 2 * 500);
assert.equal(keepPlace(small, big, 0, 300, 2, 5), 100);
assert.equal(keepPlace(big, small, 900, 300, 3, -1), 0);
// A tab's place in its grid outlasts a zoom: the tiles at the top, and how far into their row.
assert.deepEqual([placeOf(250, small), topOf(placeOf(250, small), small), topOf(8.5, big), topOf(8.5, { cols: 5, rowHeight: 100 }), placeOf(0, big)], [8.5, 250, 900, 150, 0]);
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
// A typo: a close match, below every in-order one.
assert.deepEqual(best("stetings", ["Settings", "Set as wallpaper", "Stop"]), ["Settings"]);
assert.deepEqual(best("setings", ["Reset tiles", "Settings"]), ["Settings"], "a letter left out is still in order: no typo needed");
assert.ok(fuzzyScore("setings", "Settings")! > 0);
assert.ok(fuzzyScore("stetings", "Settings")! < fuzzyScore("sett", "Reset tiles")!);
// In a path, letters far apart aren't a match; close together they are.
const longPath = "C:\\Users\\me\\AppData\\Local\\Temp\\claude\\visualstudio\\scratchpad\\kingfisher-test";
assert.ok(fuzzyScore("stetings", longPath) !== null && fuzzyScore("stetings", longPath, true) === null);
assert.ok(fuzzyScore("scratch", longPath, true)! > 0 && fuzzyScore("scrpad", longPath, true)! > 0);

// Close matches, as the instant search service finds them (query.rs's cases)
const typo = typoDistance;
assert.equal(typo("reprot", "Q3 Report.pdf"), 1, "two letters swapped are one edit");
assert.equal(typo("dowloads", "Downloads"), 1, "a letter missing");
assert.equal(typo("donwloads", "Downloads"), 1);
assert.equal(typo("downlaods", "my-downloads-old"), 1);
assert.equal(typo("invoise", "Invoices 2026"), 1);
assert.equal(typo("buget", "budget.xlsx"), 1);
assert.equal(typo("reprot", "MyReportFinal.docx"), 1, "a word starting at a capital");
assert.equal(typo("reprot", "report2026.docx"), 1, "letters then digits");
assert.equal(typo("photgraphs", "Photographs"), 1);
assert.equal(typo("phtograhps", "Photographs"), 2, "two edits from 8 letters");
assert.equal(typo("erport", "report"), 1, "the first two letters swapped");
assert.equal(typo("eport", "report"), null, "not a typo in the first letter");
assert.equal(typo("report", "the report"), 0);
assert.equal(typo("reprot", "freport.txt"), null, "only at a word's start");
assert.equal(typo("reprot", "Replay.mp4"), null);
assert.equal(typo("bdget 2026", "budget-2025.xlsx"), null, "a number as typed");
assert.equal(typo("bdget 2026", "budget-2026.xlsx"), 1);
assert.equal(typo("résumé", "Mon résume.pdf"), 1);
assert.equal(typo("café", "CAFÉ menu"), 0);
assert.deepEqual([editsAllowed("cat"), editsAllowed("2026"), editsAllowed("report"), editsAllowed("download")], [0, 0, 1, 2]);
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
