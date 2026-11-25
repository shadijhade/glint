// The interface's text, for its translators: found in Glint's Rust code, written to locales/glint.pot (the
// template), and merged into each language's catalog, locales/<code>.po, which build.rs compiles into Glint.
// scripts/i18n.mjs is the command; docs/translating.md is the guide.
//
// What counts as interface text (the xgettext "keywords" of this code base):
//   t!("…")                       the window's text, translated where it's written (glint-ui's i18n.rs)
//   english!("…")                 text in a table, translated where it's shown (glint-model's i18n.rs)
//   keyed("…"), keyed_with("…")   a menu's text with its shortcut's place in it, "Cut ({0})" (command_bar/menus.rs)
//   bind!(keys, context, action, "…")  what a key does, as the shortcuts sheet says it (actions.rs)
//   plural(n, "file")             a count: two texts, "{n} file" for one and "{n} files" for any other count
// Code under #[cfg(test)] and #[test] is left out, and so are the test modules it declares (`mod tests;`).
// A comment that starts "For translators:" on the lines just above a text goes to the translators with it;
// just above an item (a static table, a fn), it goes with every text in the item.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, join, relative } from "node:path";

// ------------------------------------------------------------------------------------------------- Rust

/** Rust source as tokens: identifiers, punctuation, strings (with their text), comments; each with its line. */
export function tokenize(source) {
    const src = source.replace(/\r\n/g, "\n");
    const out = [];
    const n = src.length;
    const idChar = (c) => c !== undefined && (/[A-Za-z0-9_]/.test(c) || c > "\x7f");
    let i = 0;
    let line = 1;
    while (i < n) {
        const c = src[i];
        if (c === "\n") {
            line++;
            i++;
            continue;
        }
        if (c === " " || c === "\t" || c === "\r") {
            i++;
            continue;
        }
        if (c === "/" && src[i + 1] === "/") {
            const end = src.indexOf("\n", i);
            const stop = end < 0 ? n : end;
            out.push({ kind: "comment", text: src.slice(i, stop), line });
            i = stop;
            continue;
        }
        if (c === "/" && src[i + 1] === "*") {
            const start = line;
            let depth = 1;
            let j = i + 2;
            while (j < n && depth > 0) {
                if (src[j] === "\n") line++;
                if (src[j] === "/" && src[j + 1] === "*") {
                    depth++;
                    j += 2;
                } else if (src[j] === "*" && src[j + 1] === "/") {
                    depth--;
                    j += 2;
                } else j++;
            }
            out.push({ kind: "comment", text: src.slice(i, j), line: start });
            i = j;
            continue;
        }
        // Raw strings: r"…", r#"…"#, br"…".
        if (c === "r" || (c === "b" && src[i + 1] === "r")) {
            let j = c === "b" ? i + 2 : i + 1;
            let hashes = 0;
            while (src[j] === "#") {
                hashes++;
                j++;
            }
            if (src[j] === '"') {
                const close = '"' + "#".repeat(hashes);
                const end = src.indexOf(close, j + 1);
                if (end < 0) throw new Error(`line ${line}: a raw string that never ends`);
                const value = src.slice(j + 1, end);
                out.push({ kind: c === "b" ? "bytes" : "str", value, line });
                line += value.split("\n").length - 1;
                i = end + close.length;
                continue;
            }
        }
        if (c === "b" && src[i + 1] === '"') {
            const read = readString(src, i + 2, line);
            out.push({ kind: "bytes", value: read.value, line });
            line += read.lines;
            i = read.end;
            continue;
        }
        if (c === '"') {
            const read = readString(src, i + 1, line);
            out.push({ kind: "str", value: read.value, line });
            line += read.lines;
            i = read.end;
            continue;
        }
        if (c === "'" || (c === "b" && src[i + 1] === "'")) {
            const q = c === "'" ? i : i + 1;
            if (src[q + 1] === "\\") {
                let j = q + 2;
                if (src[j] === "x") j += 3;
                else if (src[j] === "u") j = src.indexOf("}", j) + 1;
                else j += 1;
                out.push({ kind: "char", line });
                i = j + 1;
                continue;
            }
            const width = (src.codePointAt(q + 1) ?? 0) > 0xffff ? 2 : 1;
            if (src[q + 1 + width] === "'" && src[q + 1] !== "\n") {
                out.push({ kind: "char", line });
                i = q + 2 + width;
                continue;
            }
            // A lifetime or a label: 'a, 'static.
            let j = q + 1;
            while (idChar(src[j])) j++;
            out.push({ kind: "lifetime", line });
            i = j;
            continue;
        }
        if (/[A-Za-z_]/.test(c) || c > "\x7f") {
            let j = i + 1;
            while (idChar(src[j])) j++;
            out.push({ kind: "ident", value: src.slice(i, j), line });
            i = j;
            continue;
        }
        if (/[0-9]/.test(c)) {
            let j = i + 1;
            while (j < n && /[0-9A-Za-z_.]/.test(src[j])) {
                // Not a range (1..2) or a method on the number (1.max(2)).
                if (src[j] === "." && (src[j + 1] === "." || /[A-Za-z_]/.test(src[j + 1] ?? ""))) break;
                j++;
            }
            out.push({ kind: "number", line });
            i = j;
            continue;
        }
        out.push({ kind: "punct", value: c, line });
        i++;
    }
    return out;
}

const ESCAPES = { n: "\n", r: "\r", t: "\t", "\\": "\\", 0: "\0", "'": "'", '"': '"' };

/** A Rust string's text from just after its opening quote: escapes read, `\` at a line's end joining lines. */
function readString(src, start, line) {
    let value = "";
    let i = start;
    let lines = 0;
    for (;;) {
        const c = src[i];
        if (c === undefined) throw new Error(`line ${line}: a string that never ends`);
        if (c === '"') return { value, end: i + 1, lines };
        if (c === "\n") lines++;
        if (c !== "\\") {
            value += c;
            i++;
            continue;
        }
        const e = src[i + 1];
        if (e === "\n") {
            // A line that goes on: the line break and the next line's leading spaces aren't in the text.
            i += 1;
            while (src[i] === "\n" || src[i] === " " || src[i] === "\t" || src[i] === "\r") {
                if (src[i] === "\n") lines++;
                i++;
            }
        } else if (e in ESCAPES) {
            value += ESCAPES[e];
            i += 2;
        } else if (e === "x") {
            value += String.fromCharCode(parseInt(src.slice(i + 2, i + 4), 16));
            i += 4;
        } else if (e === "u") {
            const close = src.indexOf("}", i);
            value += String.fromCodePoint(parseInt(src.slice(i + 3, close), 16));
            i = close + 1;
        } else {
            throw new Error(`line ${line + lines}: \\${e} isn't a Rust escape`);
        }
    }
}

const ITEM_WORDS = new Set(["fn", "mod", "impl", "struct", "enum", "trait", "const", "static", "type", "use", "macro_rules", "union", "extern"]);

/** The length of a `#[test]` or `#[cfg(test)]` attribute at `i`, or 0. */
function testAttribute(tokens, i) {
    const is = (k, kind, value) => tokens[k]?.kind === kind && (value === undefined || tokens[k].value === value);
    if (!is(i, "punct", "#") || !is(i + 1, "punct", "[")) return 0;
    if (is(i + 2, "ident", "test") && is(i + 3, "punct", "]")) return 4;
    if (is(i + 2, "ident", "cfg") && is(i + 3, "punct", "(") && is(i + 4, "ident", "test") && is(i + 5, "punct", ")") && is(i + 6, "punct", "]")) return 7;
    return 0;
}

/** Where the item after a test attribute ends (the index after it), noting a `mod name;` it declares. */
function skipTestItem(tokens, i, testModules) {
    let k = i;
    // More attributes.
    while (tokens[k]?.value === "#" && tokens[k + 1]?.value === "[") {
        let depth = 0;
        do {
            if (tokens[k].value === "[") depth++;
            if (tokens[k].value === "]") depth--;
            k++;
        } while (k < tokens.length && depth > 0);
    }
    // What kind of thing it is: an item (fn, mod…) ends at its block or its `;`; a field, a variant or a
    // statement may end at a comma too.
    let p = k;
    while (["pub", "async", "unsafe", "crate", "in", "super", "self"].includes(tokens[p]?.value) || tokens[p]?.value === "(" || tokens[p]?.value === ")") p++;
    const item = ITEM_WORDS.has(tokens[p]?.value);
    if (tokens[p]?.value === "mod" && tokens[p + 1]?.kind === "ident" && tokens[p + 2]?.value === ";") {
        testModules.push(tokens[p + 1].value);
        return p + 3;
    }
    let depth = 0;
    for (; k < tokens.length; k++) {
        const v = tokens[k].kind === "punct" ? tokens[k].value : null;
        if (v === "(" || v === "[") depth++;
        else if (v === ")" || v === "]") depth--;
        else if (v === "{" && depth === 0) {
            let braces = 0;
            for (; k < tokens.length; k++) {
                if (tokens[k].value === "{" && tokens[k].kind === "punct") braces++;
                if (tokens[k].value === "}" && tokens[k].kind === "punct" && --braces === 0) return k + 1;
            }
            return k;
        } else if (v === ";" && depth === 0) return k + 1;
        else if (v === "," && depth === 0 && !item) return k + 1;
        else if (v === "}" && depth === 0 && !item) return k;
    }
    return k;
}

/** The tokens of each argument of the call whose `(` is at `open`, and the index after its `)`. */
function callArguments(tokens, open) {
    const args = [[]];
    let depth = 0;
    let k = open;
    for (; k < tokens.length; k++) {
        const t = tokens[k];
        const v = t.kind === "punct" ? t.value : null;
        if (v === "(" || v === "[" || v === "{") {
            depth++;
            if (depth === 1) continue;
        } else if (v === ")" || v === "]" || v === "}") {
            depth--;
            if (depth === 0) break;
        } else if (v === "," && depth === 1) {
            args.push([]);
            continue;
        }
        args[args.length - 1].push(t);
    }
    return { args, end: k + 1 };
}

/**
 * The interface text in one Rust file: `{ texts, testModules, variables }`. Each text is `{ text, line, note }`;
 * `testModules` are the modules it declares under #[cfg(test)]; `variables` are the calls that translate text
 * from a variable (`{ line, call }`), whose text needs `english!("…")` where it's written.
 */
export function extractFromRust(source) {
    const all = tokenize(source);
    const notes = [];
    const tokens = [];
    for (const t of all) {
        if (t.kind !== "comment") {
            tokens.push(t);
            continue;
        }
        const said = /^\/\/\s*For translators:\s*(.*)$/.exec(t.text);
        const last = notes[notes.length - 1];
        if (said) notes.push({ text: said[1].trim(), from: t.line, to: t.line });
        else if (last && t.line === last.to + 1 && /^\/\/[^/!]/.test(t.text)) {
            last.text = `${last.text} ${t.text.replace(/^\/\/\s*/, "").trim()}`.trim();
            last.to = t.line;
        }
    }
    // A note just above a text is for that text only.
    const noteFor = (line) => {
        const note = notes.find((n) => !n.used && line > n.to && line - n.to <= 2);
        if (!note) return null;
        note.used = true;
        return note.text;
    };
    // A note just above an item (a static table, a fn) is for every text in it.
    const blockStarts = new Map();
    for (const note of notes) {
        const at = tokens.findIndex((t) => t.line > note.to);
        if (at >= 0 && tokens[at].line - note.to <= 2 && ITEM_WORDS.has(tokens[at].value === "pub" ? tokens[at + 1]?.value : tokens[at].value)) {
            blockStarts.set(at, note.text);
            note.used = true;
        }
    }
    let block = null;
    let depth = 0;

    const texts = [];
    const testModules = [];
    const variables = [];
    const add = (text, line, note = null) => texts.push({ text, line, note: note ?? noteFor(line) ?? block?.text ?? null });
    for (let i = 0; i < tokens.length; i++) {
        const skip = testAttribute(tokens, i);
        if (skip) {
            i = skipTestItem(tokens, i + skip, testModules) - 1;
            continue;
        }
        const t = tokens[i];
        if (blockStarts.has(i)) block = { text: blockStarts.get(i), depth };
        if (t.kind === "punct") {
            if ("([{".includes(t.value)) depth++;
            else if (")]}".includes(t.value)) {
                depth--;
                if (block && t.value === "}" && depth === block.depth) block = null;
            } else if (block && t.value === ";" && depth === block.depth) block = null;
        }
        if (t.kind !== "ident") continue;
        // bind!(keys, context, action, "What it does"): the shortcuts sheet's text (actions.rs).
        if (t.value === "bind" && tokens[i + 1]?.value === "!" && tokens[i + 2]?.value === "(") {
            const what = callArguments(tokens, i + 2).args[3]?.find((a) => a.kind === "str");
            if (what) add(what.value, what.line);
            continue;
        }
        const definition = tokens[i - 1]?.value === "fn";
        const next = tokens[i + 1];
        const macro = next?.value === "!" && tokens[i + 2]?.value === "(";
        if ((t.value === "t" || t.value === "english") && macro) {
            const first = tokens[i + 3];
            if (first?.kind === "str") add(first.value, first.line);
            else if (first?.value !== "$") variables.push({ line: t.line, call: `${t.value}!(…)` });
            continue;
        }
        if (definition || next?.value !== "(") continue;
        if (t.value === "keyed" || t.value === "keyed_with") {
            const first = tokens[i + 2];
            if (first?.kind === "str") add(first.value, first.line, "{0}, {1}: the keys that do it (Ctrl+X), which Glint fills in");
        } else if (t.value === "plural") {
            const { args } = callArguments(tokens, i + 1);
            const nouns = (args[1] ?? []).filter((a) => a.kind === "str");
            if (!nouns.length) variables.push({ line: t.line, call: "plural(n, …)" });
            for (const noun of nouns) {
                add(`{n} ${noun.value}`, noun.line, "a count of one: {n} is 1");
                add(`{n} ${noun.value}s`, noun.line, "a count other than one: {n} is the number");
            }
        } else if (t.value === "translate" && tokens[i - 1]?.value === ":") {
            // i18n::translate(text) in the window, glint_model::i18n::translate(language, text).
            const { args } = callArguments(tokens, i + 1);
            const text = args[args.length - 1] ?? [];
            if (!text.some((a) => a.kind === "str" || a.value === "english" || a.value === "$")) variables.push({ line: t.line, call: "translate(…)" });
        }
    }
    return { texts, testModules, variables };
}

/** The Rust files under each crate's src/, as paths relative to `root` with forward slashes. */
export function rustFiles(root) {
    const files = [];
    const walk = (dir) => {
        for (const name of readdirSync(dir).sort()) {
            const path = join(dir, name);
            if (statSync(path).isDirectory()) walk(path);
            else if (name.endsWith(".rs")) files.push(relative(root, path).replaceAll("\\", "/"));
        }
    };
    const crates = join(root, "crates");
    for (const crate of readdirSync(crates).sort()) {
        const src = join(crates, crate, "src");
        try {
            if (statSync(src).isDirectory()) walk(src);
        } catch {
            // A crate without src/.
        }
    }
    return files;
}

/** Where a module declared in `file` as `mod name;` lives: `dir/name.rs` or `dir/name/`. */
function moduleDir(file) {
    const name = basename(file);
    return ["mod.rs", "lib.rs", "main.rs"].includes(name) ? dirname(file) : join(dirname(file), name.slice(0, -3)).replaceAll("\\", "/");
}

/**
 * Every interface text in the code: `{ entries, variables, files }`. Entries are in the order they're first met
 * (files in path order), each `{ msgid, refs (files), notes }`.
 */
export function extractAll(root, read = (path) => readFileSync(join(root, path), "utf8")) {
    const files = rustFiles(root);
    const found = new Map();
    const excluded = [];
    for (const file of files) {
        const { texts, testModules, variables } = extractFromRust(read(file));
        const dir = moduleDir(file);
        for (const name of testModules) excluded.push(`${dir}/${name}.rs`, `${dir}/${name}/`);
        found.set(file, { texts, variables });
    }
    const isTest = (file) => excluded.some((path) => (path.endsWith("/") ? file.startsWith(path) : file === path));
    const entries = new Map();
    const variables = [];
    let used = 0;
    for (const file of files) {
        if (isTest(file)) continue;
        used++;
        const { texts, variables: v } = found.get(file);
        // i18n.rs is the lookup itself: its variables are the texts the rest of the code hands it.
        if (!file.endsWith("/i18n.rs")) variables.push(...v.map((x) => ({ file, ...x })));
        for (const { text, note } of texts) {
            if (!text.trim()) continue;
            let entry = entries.get(text);
            if (!entry) entries.set(text, (entry = { msgid: text, refs: [], notes: [] }));
            if (!entry.refs.includes(file)) entry.refs.push(file);
            if (note && !entry.notes.includes(note)) entry.notes.push(note);
        }
    }
    return { entries: [...entries.values()], variables, files: used };
}

// --------------------------------------------------------------------------------------------------- PO

/** The `{name}`, `%1` and `%` placeholders in a text, each once, in order: as src/po.rs's `placeholders`. */
export function placeholders(text) {
    const found = [];
    for (const m of text.matchAll(/\{[A-Za-z0-9_]+\}|%[0-9]|%/g)) if (!found.includes(m[0])) found.push(m[0]);
    return found;
}

/** Why a translation can't stand in for its English, or null: as src/po.rs's `placeholders_differ`. */
export function placeholdersDiffer(english, translation) {
    const want = placeholders(english);
    const have = placeholders(translation);
    const missing = want.filter((p) => !have.includes(p));
    const extra = have.filter((p) => !want.includes(p));
    if (!missing.length && !extra.length) return null;
    if (!extra.length) return `the translation lacks ${missing.join(" ")}`;
    if (!missing.length) return `the translation has ${extra.join(" ")}, which the English doesn't`;
    return `the translation lacks ${missing.join(" ")} and has ${extra.join(" ")}, which the English doesn't`;
}

function unquote(quoted, line) {
    if (quoted.length < 2 || !quoted.startsWith('"') || !quoted.endsWith('"')) throw new Error(`line ${line}: expected a string in quotes, found ${quoted}`);
    const inner = quoted.slice(1, -1);
    let out = "";
    for (let i = 0; i < inner.length; i++) {
        const c = inner[i];
        if (c === '"') throw new Error(`line ${line}: a " inside ${quoted} needs a \\ before it`);
        if (c !== "\\") {
            out += c;
            continue;
        }
        const e = inner[++i];
        const map = { n: "\n", t: "\t", r: "\r", '"': '"', "\\": "\\" };
        if (!(e in map)) throw new Error(`line ${line}: \\${e ?? ""} isn't an escape Glint's catalogs use`);
        out += map[e];
    }
    return out;
}

/**
 * A catalog: `{ header, headerComments, entries, obsolete }`. The header is the msgstr of the entry whose
 * msgid is empty. Each entry is `{ msgid, msgstr, comments (the translator's "# …"), notes ("#."), refs
 * ("#:"), flags ("#,"), previous ("#| msgid"), context, plural, line }`.
 */
export function parsePo(text) {
    const result = { header: null, headerComments: [], entries: [], obsolete: [] };
    const lines = text.replace(/\r\n/g, "\n").split("\n");
    let entry = null;
    let field = null;
    const fresh = (obsolete) => ({ msgid: null, msgstr: null, comments: [], notes: [], refs: [], flags: [], previous: null, context: null, plural: null, line: 0, obsolete });
    const finish = () => {
        if (!entry) return;
        if (entry.msgid === null) {
            // Comments with no entry: the header's, at the top.
            if (!result.entries.length && result.header === null) result.headerComments.push(...entry.comments);
        } else {
            if (entry.msgstr === null) throw new Error(`line ${entry.line}: msgid without a msgstr`);
            if (entry.msgid === "" && entry.context === null && !entry.obsolete) {
                result.header = entry.msgstr;
                result.headerComments.push(...entry.comments);
            } else (entry.obsolete ? result.obsolete : result.entries).push(entry);
        }
        entry = null;
        field = null;
    };
    for (let index = 0; index < lines.length; index++) {
        const number = index + 1;
        let line = lines[index].trim();
        const obsolete = line.startsWith("#~");
        if (obsolete) line = line.slice(2).trim();
        if (entry && entry.msgstr !== null && (line === "" || (line.startsWith("#") && !obsolete) || line.startsWith("msgctxt") || /^msgid\s/.test(line))) finish();
        if (line === "") {
            if (entry && entry.msgid === null && !obsolete) finish();
            continue;
        }
        entry ??= fresh(obsolete);
        if (obsolete && entry.msgid === null) entry.obsolete = true;
        if (line.startsWith("#,")) entry.flags.push(...line.slice(2).split(",").map((f) => f.trim()).filter(Boolean));
        else if (line.startsWith("#|")) {
            const m = /^#\|\s*msgid\s+(".*")$/.exec(line);
            if (m) entry.previous = unquote(m[1], number);
        } else if (line.startsWith("#.")) entry.notes.push(line.slice(2).trim());
        else if (line.startsWith("#:")) entry.refs.push(...line.slice(2).trim().split(/\s+/).filter(Boolean));
        else if (line.startsWith("#")) entry.comments.push(line.replace(/^#\s?/, ""));
        else if (line.startsWith("msgctxt")) {
            entry.context = unquote(line.slice(7).trim(), number);
            field = "context";
        } else if (line.startsWith("msgid_plural")) {
            entry.plural = unquote(line.slice(12).trim(), number);
            field = "plural";
        } else if (line.startsWith("msgid")) {
            entry.msgid = unquote(line.slice(5).trim(), number);
            entry.line = number;
            field = "msgid";
        } else if (line.startsWith("msgstr[")) {
            const m = /^msgstr\[(\d+)\]\s*(".*")$/.exec(line);
            if (!m) throw new Error(`line ${number}: can't read ${line}`);
            if (m[1] === "0") {
                entry.msgstr = unquote(m[2], number);
                field = "msgstr";
            } else field = "other";
        } else if (line.startsWith("msgstr")) {
            entry.msgstr = unquote(line.slice(6).trim(), number);
            field = "msgstr";
        } else if (line.startsWith('"')) {
            const more = unquote(line, number);
            if (!field) throw new Error(`line ${number}: a string that goes on from nothing`);
            if (field !== "other") entry[field] += more;
        } else throw new Error(`line ${number}: can't read ${line}`);
    }
    finish();
    return result;
}

const WIDTH = 79;

function escapePo(text) {
    return text.replaceAll("\\", "\\\\").replaceAll('"', '\\"').replaceAll("\n", "\\n").replaceAll("\t", "\\t").replaceAll("\r", "\\r");
}

/** An escaped string cut after each \n, and at spaces where a line would pass the width: as gettext writes it. */
function wrapped(escaped, room) {
    const segments = [];
    let current = "";
    for (let i = 0; i < escaped.length; i++) {
        if (escaped[i] === "\\") {
            current += escaped.slice(i, i + 2);
            if (escaped[i + 1] === "n" && i + 2 < escaped.length) {
                segments.push(current);
                current = "";
            }
            i++;
        } else current += escaped[i];
    }
    if (current) segments.push(current);
    const pieces = [];
    for (let rest of segments) {
        while (rest.length > room) {
            let cut = rest.lastIndexOf(" ", room - 1);
            if (cut <= 0) cut = rest.indexOf(" ", room);
            if (cut < 0 || cut === rest.length - 1) break;
            pieces.push(rest.slice(0, cut + 1));
            rest = rest.slice(cut + 1);
        }
        pieces.push(rest);
    }
    return pieces;
}

function poField(keyword, text, prefix = "") {
    const escaped = escapePo(text);
    const inner = escaped.endsWith("\\n") ? escaped.slice(0, -2) : escaped;
    const oneLine = `${prefix}${keyword} "${escaped}"`;
    if (oneLine.length <= WIDTH && wrapped(inner, Infinity).length === 1) return [oneLine];
    return [`${prefix}${keyword} ""`, ...wrapped(escaped, WIDTH - 2 - prefix.length).map((piece) => `${prefix}"${piece}"`)];
}

/** A catalog's text, from what `parsePo` gives (or the template's entries). */
export function writePo({ headerComments = [], header = "", entries = [], obsolete = [] }) {
    const out = [];
    for (const c of headerComments) out.push(c ? `# ${c}` : "#");
    out.push('msgid ""', ...poField("msgstr", header));
    for (const entry of entries) {
        out.push("");
        for (const c of entry.comments ?? []) out.push(c ? `# ${c}` : "#");
        for (const note of entry.notes ?? []) out.push(`#. ${note}`);
        if (entry.refs?.length) out.push(...refLines(entry.refs));
        if (entry.flags?.length) out.push(`#, ${entry.flags.join(", ")}`);
        if (entry.previous !== null && entry.previous !== undefined) out.push(...poField("msgid", entry.previous, "#| "));
        out.push(...poField("msgid", entry.msgid), ...poField("msgstr", entry.msgstr ?? ""));
    }
    for (const entry of obsolete) {
        out.push("");
        for (const c of entry.comments ?? []) out.push(c ? `# ${c}` : "#");
        out.push(...poField("msgid", entry.msgid, "#~ "), ...poField("msgstr", entry.msgstr ?? "", "#~ "));
    }
    return out.join("\n") + "\n";
}

function refLines(refs) {
    const lines = [];
    let line = "#:";
    for (const ref of refs) {
        if (line.length > 2 && line.length + 1 + ref.length > WIDTH) {
            lines.push(line);
            line = "#:";
        }
        line += ` ${ref}`;
    }
    lines.push(line);
    return lines;
}

/** The template's header. */
export const TEMPLATE_HEADER =
    "Project-Id-Version: Glint\n" +
    "Report-Msgid-Bugs-To: https://github.com/shadijhade/glint/issues\n" +
    "MIME-Version: 1.0\n" +
    "Content-Type: text/plain; charset=UTF-8\n" +
    "Content-Transfer-Encoding: 8bit\n";

/** A new catalog's header for `code` ("zh-CN"). */
export function catalogHeader(code) {
    return `${TEMPLATE_HEADER}Language: ${code.replace("-", "_")}\nX-Generator: scripts/i18n.mjs\n`;
}

/** The template (locales/glint.pot) for the extracted entries. */
export function template(entries) {
    return {
        headerComments: [
            "Glint's interface text, made from the code by `node scripts/i18n.mjs extract`: don't edit it.",
            "Each language's translation is locales/<code>.po; docs/translating.md says how.",
        ],
        header: TEMPLATE_HEADER,
        entries: entries.map((e) => ({ msgid: e.msgid, msgstr: "", notes: e.notes, refs: e.refs })),
    };
}

/** How alike two texts are, 0 to 1 (1 less the edit distance over the longer length). */
export function likeness(a, b) {
    if (a === b) return 1;
    const longer = Math.max(a.length, b.length);
    if (!longer || Math.min(a.length, b.length) / longer < 0.6) return 0;
    let prev = Array.from({ length: b.length + 1 }, (_, j) => j);
    for (let i = 1; i <= a.length; i++) {
        const row = [i];
        for (let j = 1; j <= b.length; j++) row[j] = Math.min(prev[j] + 1, row[j - 1] + 1, prev[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1));
        prev = row;
    }
    return 1 - prev[b.length] / longer;
}

/**
 * A catalog brought up to date with the template: each text the code has, with the translation the catalog
 * had for it; a new text empty, or, where the catalog translated one much like it that the code no longer has
 * (an English that changed), that translation marked fuzzy (a guess Glint doesn't use until someone checks it);
 * the translations of texts the code dropped, at the end as obsolete (`#~`).
 */
export function merge(tmpl, catalog, code) {
    const old = new Map((catalog?.entries ?? []).filter((e) => e.context === null && e.plural === null).map((e) => [e.msgid, e]));
    const wanted = new Set(tmpl.entries.map((e) => e.msgid));
    const entries = tmpl.entries.map((t) => {
        const had = old.get(t.msgid);
        const fuzzy = had?.flags.includes("fuzzy") ?? false;
        return {
            msgid: t.msgid,
            msgstr: had?.msgstr ?? "",
            comments: had?.comments ?? [],
            notes: t.notes ?? [],
            refs: t.refs ?? [],
            flags: fuzzy ? ["fuzzy"] : [],
            previous: fuzzy ? (had.previous ?? null) : null,
        };
    });
    const gone = [...old.values()].filter((e) => !wanted.has(e.msgid) && e.msgstr);
    const before = [...gone, ...(catalog?.obsolete ?? []).filter((e) => e.msgstr && !wanted.has(e.msgid))];
    for (const entry of entries) {
        if (entry.msgstr || !before.length) continue;
        let best = null;
        let score = 0.7;
        for (const candidate of before) {
            const s = likeness(entry.msgid, candidate.msgid);
            if (s >= score && !placeholdersDiffer(entry.msgid, candidate.msgstr)) [best, score] = [candidate, s];
        }
        if (best) Object.assign(entry, { msgstr: best.msgstr, flags: ["fuzzy"], previous: best.msgid });
    }
    const seen = new Set();
    const obsolete = before.filter((e) => !seen.has(e.msgid) && seen.add(e.msgid)).map((e) => ({ msgid: e.msgid, msgstr: e.msgstr, comments: e.comments ?? [] }));
    return {
        headerComments: catalog?.headerComments?.length ? catalog.headerComments : [`Glint's interface in ${code}: docs/translating.md says how to translate it.`],
        header: catalog?.header ?? catalogHeader(code),
        entries,
        obsolete,
    };
}

/** How far a catalog has come against the template, and the entries Glint can't use (as build.rs judges them). */
export function catalogStatus(tmpl, catalog) {
    const wanted = new Set(tmpl.entries.map((e) => e.msgid));
    const have = new Map(catalog.entries.map((e) => [e.msgid, e]));
    let translated = 0;
    let fuzzy = 0;
    for (const msgid of wanted) {
        const e = have.get(msgid);
        if (!e?.msgstr) continue;
        if (e.flags.includes("fuzzy")) fuzzy++;
        else translated++;
    }
    const problems = [];
    const seen = new Set();
    for (const e of catalog.entries) {
        let why = null;
        if (e.context !== null) why = "it has a msgctxt, and Glint's texts have no context";
        else if (e.plural !== null) why = 'it has plural forms: Glint\'s counts are two texts, "{n} file" and "{n} files"';
        else if (e.msgstr && !e.flags.includes("fuzzy")) why = placeholdersDiffer(e.msgid, e.msgstr);
        if (!why && seen.has(e.msgid)) why = "the same English is in the file twice";
        seen.add(e.msgid);
        if (why) problems.push({ line: e.line, msgid: e.msgid, why });
    }
    const missing = [...wanted].filter((msgid) => !have.has(msgid)).length;
    const stale = catalog.entries.filter((e) => !wanted.has(e.msgid)).length;
    return { total: wanted.size, translated, fuzzy, untranslated: wanted.size - translated - fuzzy, missing, stale, obsolete: catalog.obsolete.length, problems };
}
