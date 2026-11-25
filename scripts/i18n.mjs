// Glint's interface text, for its translators (docs/translating.md): finds every text in the code, writes the
// template, brings each language's catalog up to date with it, and says how far each has come. build.rs
// compiles the catalogs (locales/<code>.po) into Glint; scripts/lib/i18n.mjs says what counts as a text.
//
// Usage: node scripts/i18n.mjs <command> [code ...] [--verbose]
//   extract            write locales/glint.pot: every text in the code, with the files it's in
//   update [code ...]  extract, then bring locales/<code>.po up to date: new texts come in empty, a text whose
//                      English changed keeps its old translation as a fuzzy guess (Glint doesn't use it until
//                      someone checks it and takes the mark off), a text the code dropped goes to the end as
//                      obsolete (#~). Every catalog when no code is given; a new code starts a catalog.
//   status [code ...]  how much of each catalog is translated, and the translations Glint can't use. Exits 1
//                      when there are any (the build leaves them out, and a test fails on them).
//   --verbose          with extract or update: the calls that translate a text from a variable, whose text
//                      needs english!("…") where it's written for this script to find it

import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { catalogStatus, extractAll, merge, parsePo, template, writePo } from "./lib/i18n.mjs";
import { root } from "./lib/project.mjs";

const args = process.argv.slice(2);
const verbose = args.includes("--verbose");
const [command, ...codes] = args.filter((a) => a !== "--verbose");
const locales = join(root, "locales");
const potFile = join(locales, "glint.pot");
const catalogs = () => readdirSync(locales).filter((f) => f.endsWith(".po")).map((f) => f.slice(0, -3)).sort();
const write = (file, text) => {
    const before = existsSync(file) ? readFileSync(file, "utf8") : null;
    if (before !== text) writeFileSync(file, text);
    return before !== text;
};

function extract() {
    const { entries, variables, files } = extractAll(root);
    const tmpl = template(entries);
    const changed = write(potFile, writePo(tmpl));
    console.log(`${entries.length} texts in ${files} files: locales/glint.pot${changed ? "" : " (unchanged)"}`);
    if (verbose && variables.length) {
        console.log(`\n${variables.length} calls translate a text from a variable: where that text is written, it needs english!("…") (a table's) or t!:`);
        for (const v of variables) console.log(`  ${v.file}:${v.line}  ${v.call}`);
    }
    return tmpl;
}

function update(tmpl) {
    for (const code of codes.length ? codes : catalogs()) {
        if (!/^[a-z]{2,3}(-[A-Za-z0-9]+)*$/.test(code)) throw new Error(`"${code}" isn't a language code like zh-CN`);
        const file = join(locales, `${code}.po`);
        const catalog = existsSync(file) ? parsePo(readFileSync(file, "utf8")) : null;
        const merged = merge(tmpl, catalog, code);
        const changed = write(file, writePo(merged));
        const s = catalogStatus(tmpl, parsePo(writePo(merged)));
        console.log(`locales/${code}.po: ${catalog ? (changed ? "brought up to date" : "already up to date") : "started"}; ${summary(s)}`);
    }
}

function summary(s) {
    const pct = s.total ? Math.floor((s.translated * 100) / s.total) : 0;
    const parts = [`${s.translated} of ${s.total} translated (${pct}%)`];
    if (s.fuzzy) parts.push(`${s.fuzzy} fuzzy`);
    if (s.obsolete) parts.push(`${s.obsolete} obsolete`);
    return parts.join(", ");
}

function status() {
    const tmpl = existsSync(potFile) ? parsePo(readFileSync(potFile, "utf8")) : template(extractAll(root).entries);
    let problems = 0;
    for (const code of codes.length ? codes : catalogs()) {
        const file = join(locales, `${code}.po`);
        if (!existsSync(file)) {
            console.log(`locales/${code}.po: there is none (node scripts/i18n.mjs update ${code} starts it)`);
            continue;
        }
        const s = catalogStatus(tmpl, parsePo(readFileSync(file, "utf8")));
        console.log(`locales/${code}.po: ${summary(s)}`);
        if (s.missing || s.stale) console.log(`  ${s.missing} new texts and ${s.stale} dropped ones since its last update: node scripts/i18n.mjs update ${code}`);
        for (const p of s.problems) console.log(`  line ${p.line}: ${JSON.stringify(p.msgid)}: ${p.why}`);
        problems += s.problems.length;
    }
    if (problems) {
        console.log(`\n${problems} translations can't be used as they are; Glint shows their English until they're fixed.`);
        process.exitCode = 1;
    }
}

try {
    if (command === "extract") extract();
    else if (command === "update") update(extract());
    else if (command === "status") status();
    else {
        console.error("Usage: node scripts/i18n.mjs extract | update [code ...] | status [code ...] [--verbose]");
        process.exitCode = 2;
    }
} catch (e) {
    console.error(`✗ ${e.message}`);
    process.exitCode = 1;
}
