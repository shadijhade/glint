// The part of THIRD_PARTY_CRATES.md that cargo-about can't write: fonts, icons and data that crates embed in
// Glint.exe under licenses of their own, which the crate's own SPDX license (all cargo-about and cargo-deny
// read) doesn't cover. The texts are read from the crates' sources in Cargo's registry (found with
// `cargo metadata`), at the versions Cargo.lock names, so a crate update changes them with it:
//
//   * the Lucide icons (ISC, and MIT for the ones from Feather), embedded by gpui-kit-assets for gpui-kit
//   * two-face's syntax definitions and themes (each with its own license), embedded for code highlighting
//   * jiff-tzdb's time zone database (public domain)
//
// (Typst's assets, hayagriva's citation styles and hypher's hyphenation patterns left with the built-in Typst in 0.12,
// and the KaTeX fonts with the math previews in 0.14.)
//
// Anything else that embeds a font, icon set or data and isn't listed here: add it to SOURCES below.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { inflateSync } from "node:zlib";
import { windowsPackages } from "./metadata.mjs";

/** Marks where this section starts in THIRD_PARTY_CRATES.md, so it can be rewritten without cargo-about. */
export const MARKER = "<!-- embedded-assets: scripts/lib/embedded-notices.mjs writes everything below this line -->";

/** `crate name -> { dir, version }` of the crates in the Windows build. */
function crateFolders() {
    const folders = new Map();
    for (const p of windowsPackages()) if (!folders.has(p.name)) folders.set(p.name, { dir: p.dir, version: p.version });
    return folders;
}

const read = (path) => readFileSync(path, "utf8").replace(/\r\n/g, "\n").trimEnd();
/** A text in a fence that no license text can end early (it uses five backticks). */
const fence = (text) => `\`\`\`\`\`text\n${text}\n\`\`\`\`\`\n`;

/** two-face's acknowledgements: a zlib-compressed bincode list of { license type, text, path }, syntaxes then themes. */
function twoFaceLicenses(dir) {
    const data = inflateSync(readFileSync(join(dir, "generated", "acknowledgements_full.bin")));
    let at = 0;
    const u64 = () => {
        const value = Number(data.readBigUInt64LE(at));
        at += 8;
        return value;
    };
    const text = () => {
        const length = u64();
        const value = data.toString("utf8", at, at + length);
        at += length;
        return value;
    };
    const out = [];
    for (const kind of ["syntax definitions", "themes"]) {
        const count = u64();
        for (let i = 0; i < count; i++) {
            at += 4; // the license type
            const licenseText = text();
            out.push({ kind, text: licenseText.replace(/\r\n/g, "\n").trim(), path: text() });
        }
    }
    if (at !== data.length) throw new Error(`two-face's acknowledgements didn't read to the end (${at} of ${data.length} bytes): its format changed`);
    return out;
}

/** Each section: [title, what it is, (folders) => the license texts as markdown]. */
const SOURCES = [
    ["Lucide icons", "Glint's icons are Lucide's, embedded in Glint.exe by gpui-kit-assets (ISC; the icons that come from Feather are MIT).", (get) => fence(read(join(get("gpui-kit-assets").dir, "LICENSE-LUCIDE")))],
    ["Syntax definitions and themes (two-face)", "Code and text previews are highlighted with the syntax definitions and themes two-face embeds (the sets of the `bat` program). Each keeps its own license, reproduced here once with every file it covers.", (get) => {
        const groups = new Map();
        for (const entry of twoFaceLicenses(get("two-face").dir)) {
            const group = groups.get(entry.text) ?? { paths: [], text: entry.text };
            group.paths.push(`${entry.kind === "themes" ? "theme" : "syntax"}: ${entry.path.replace(/\\/g, "/").replace(/^(syntaxes|themes)\//, "").replace(/\/(LICENSE|LICENCE|COPYING)(\.[a-z]+)?$/i, "")}`);
            groups.set(entry.text, group);
        }
        return [...groups.values()].map((g) => `${g.paths.sort().join(", ")}\n\n${fence(g.text)}`).join("\n");
    }],
    ["Time zone database (jiff-tzdb)", "Dates in time zones use the IANA time zone database jiff-tzdb embeds, which is in the public domain (https://www.iana.org/time-zones). jiff-tzdb's own code is dual-licensed under the Unlicense and MIT (listed above).", () => ""],
];

/** The section for THIRD_PARTY_CRATES.md, from the marker down, as text. Throws if a crate's sources aren't in the registry. */
export function embeddedNotices() {
    const folders = crateFolders();
    const get = (name) => {
        const found = folders.get(name);
        if (!found || !existsSync(found.dir)) throw new Error(`${name} isn't in the build (or its sources aren't downloaded): scripts/lib/embedded-notices.mjs lists it`);
        return found;
    };
    const parts = [MARKER, "", "# Fonts, icons and data embedded in the app", "",
        "cargo-about lists the crates above by their own licenses. These crates also carry fonts, icons and data that are not theirs, under licenses of their own; they ship inside Glint.exe, so their notices are reproduced here. (Written by `npm run licenses`, from the crates' sources at the versions in Cargo.lock.)", ""];
    for (const [title, intro, texts] of SOURCES) {
        parts.push(`## ${title}`, "", intro, "", texts(get).trim(), "");
    }
    return `${parts.join("\n").trimEnd()}\n`;
}
