// Self-check for the search/replace text helpers. Run: node src/lib/text.check.ts
import assert from "node:assert/strict";
import { replaceText, segments, toRegExp } from "./text.ts";
import type { RgMatch } from "./ripgrep";

const opts = (o: Partial<{ caseSensitive: boolean; wholeWord: boolean; regex: boolean }> = {}) =>
    ({ caseSensitive: false, wholeWord: false, regex: false, code: false, ...o });
const replace = (text: string, query: string, o = opts()) => text.replace(toRegExp(query, o), "X");

// Smart case, like rg: lowercase ignores case, any uppercase matches exactly.
assert.equal(replace("foo Foo FOO", "foo"), "X X X");
assert.equal(replace("foo Foo FOO", "Foo"), "foo X FOO");
assert.equal(replace("foo Foo", "foo", opts({ caseSensitive: true })), "X Foo");
// Literal queries are escaped; whole word respects word boundaries.
assert.equal(replace("a.b axb", "a.b"), "X axb");
assert.equal(replace("a.b axb", "a.b", opts({ regex: true })), "X X");
assert.equal(replace("cat concat", "cat", opts({ wholeWord: true })), "X concat");
// Multi-line literal queries also match CRLF files.
assert.equal(replace("one\r\ntwo", "one\ntwo"), "X");
// Replace All works line by line, as rg searched: $ ends each line, \s never eats a line end.
assert.equal(replaceText("a  \r\n\r\nb \n", "\\s+$", opts({ regex: true }), ""), "a\r\n\r\nb\n");
assert.equal(replaceText("x1\nx2", "^x", opts({ regex: true }), "y"), "y1\ny2");
assert.equal(replaceText("a$1b", "$1", opts(), "[$&]"), "a[$&]b");

// rg reports UTF-8 byte offsets: "é" is 2 bytes, so "foo" sits at bytes 9..12.
const match = (text: string, spans: [number, number][]) =>
    ({ type: "match", data: { lines: { text }, submatches: spans.map(([start, end]) => ({ match: { text: "" }, start, end })) } }) as RgMatch;
const parts = (m: RgMatch) => segments(m).map((p) => (p.hit ? `[${p.text}]` : p.text)).join("");
assert.equal(parts(match("  héllo foo wörld\n", [[9, 12]])), "héllo [foo] wörld");
assert.equal(parts(match("plain line\r\n", [])), "plain line");
assert.equal(parts(match(`${"x".repeat(100)}foo`, [[100, 103]])), `…${"x".repeat(30)}[foo]`);

console.log("text.check: all assertions passed");
