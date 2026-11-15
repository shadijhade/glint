// Whether Windows 10 can start a program at all: Windows loads every DLL a program imports, and every
// function it takes from them, before any of the program's code runs, and refuses to start it when one is
// missing ("The code execution cannot proceed because api-ms-win-core-ioring-l1-1-0.dll was not found":
// issue #6, Glint 0.10 to 0.12 on Windows 10). So every import of Glint.exe, the instant search service and the
// bundled tools is checked against scripts/windows10-imports.json, the imports known to exist on Windows 10
// 1809 (10.0.17763, the oldest Glint supports: the MSIX's MinVersion) in every edition, N editions too. A
// known troublemaker is named with what to do instead; anything else not listed yet fails until someone has
// checked it (its Microsoft Learn page's "Minimum supported client") and added it with
// `npm run check-imports -- --update`. A Windows 11 function then gets looked up at run time instead
// (GetProcAddress), as glint-core's ioring.rs and mfapi.rs do.

import { readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { peImports } from "./pe-imports.mjs";
import { root } from "./project.mjs";

export const BASELINE = join(root, "scripts", "windows10-imports.json");

/** DLLs some supported Windows lacks, and what to do instead. Never added to the baseline. */
const MISSING_SOMEWHERE = [
    {
        test: (dll) => /^api-ms-win-core-ioring-/.test(dll),
        why: "the I/O ring's DLL is Windows 11's: Windows 10 refuses to start the program (issue #6). Look its functions up at run time, as glint-core's ioring.rs does.",
    },
    {
        // vcruntime140.dll, vcruntime140_1.dll, msvcp140.dll, msvcp140_atomic_wait.dll… and the debug ones.
        test: (dll) => /^((vcruntime|msvcp|vcomp|concrt|vccorlib)\d+(_\w+)?d?|ucrtbased)\.dll$/.test(dll),
        why: "the Visual C++ runtime, which Windows doesn't come with (only the Visual C++ Redistributable brings it): a PC without it can't start the program. Glint's own programs build it in (.cargo/config.toml, +crt-static); a bundled tool has to be built that way too.",
    },
    {
        test: (dll) => /^(mfplat|mfreadwrite|mf|mfcore|evr|wmvcore|mfsensorgroup|mfsrcsnk|msmpeg2vdec)\.dll$/.test(dll),
        why: "Media Foundation, which Windows' N editions don't have until the Media Feature Pack is added: Windows refuses to start the program there. Call it through glint-core's mfapi.rs, which looks the functions up at run time.",
    },
];

/** rg and fd, as the build puts them beside Glint.exe or as crates/glint/binaries keeps them. */
export const isBundledTool = (file) => /^(rg|fd)(-x86_64-pc-windows-msvc)?\.exe$/i.test(basename(file));

function readBaseline() {
    const baseline = JSON.parse(readFileSync(BASELINE, "utf8"));
    return new Map(Object.entries(baseline.imports).map(([dll, functions]) => [dll.toLowerCase(), new Set(functions)]));
}

/**
 * The imports of `files` that Windows 10 might not have.
 * @returns {{ problems: string[], unknown: { dll: string, fn: string }[] }} problems to show (empty when all is
 * well), and the imports that are only missing from the baseline (what --update would add).
 */
export function checkWindows10(files) {
    const known = readBaseline();
    const problems = [];
    const unknown = [];
    for (const file of files) {
        const name = basename(file);
        for (const { dll: rawDll, delayed, functions } of peImports(file)) {
            const dll = rawDll.toLowerCase();
            const how = delayed ? " (delay-loaded: it fails the first time it's called)" : "";
            const missing = MISSING_SOMEWHERE.find((m) => m.test(dll));
            if (missing) {
                problems.push(`${name} imports ${rawDll}${how} (${functions.join(", ")}): ${missing.why}`);
                continue;
            }
            const listed = known.get(dll);
            const fresh = functions.filter((fn) => !listed?.has(fn));
            if (!fresh.length) continue;
            problems.push(`${name} imports ${fresh.join(", ")} from ${rawDll}${how}, which scripts/windows10-imports.json doesn't list.`);
            for (const fn of fresh) if (!unknown.some((u) => u.dll === dll && u.fn === fn)) unknown.push({ dll, fn });
        }
    }
    if (unknown.length) {
        problems.push(
            "Windows refuses to start a program when one of its imports is missing (issue #6). Check that Windows 10 1809 has each one above " +
                "(its Microsoft Learn page, Requirements, Minimum supported client). If one is newer, look it up at run time instead (GetProcAddress, as " +
                "glint-core's ioring.rs does); if they're all there, add them with `npm run check-imports -- --update`.",
        );
    }
    return { problems, unknown };
}

/**
 * The check as the installer and MSIX scripts make it on what they pack: Glint's own programs have to pass
 * (`problems`), while the bundled tools are their projects' official builds, which Glint starts without, so
 * what they lack is only said (`warnings`).
 */
export function windows10Report(files) {
    return {
        problems: checkWindows10(files.filter((f) => !isBundledTool(f))).problems,
        warnings: checkWindows10(files.filter(isBundledTool)).problems,
    };
}

/** Adds imports (checked by someone first) to scripts/windows10-imports.json. */
export function addToBaseline(imports) {
    const baseline = JSON.parse(readFileSync(BASELINE, "utf8"));
    const lists = new Map(Object.entries(baseline.imports).map(([dll, functions]) => [dll.toLowerCase(), new Set(functions)]));
    for (const { dll, fn } of imports) {
        if (MISSING_SOMEWHERE.some((m) => m.test(dll))) throw new Error(`${dll} is missing on some Windows: it can't be added.`);
        if (!lists.has(dll)) lists.set(dll, new Set());
        lists.get(dll).add(fn);
    }
    const sorted = [...lists.keys()].sort();
    baseline.imports = Object.fromEntries(sorted.map((dll) => [dll, [...lists.get(dll)].sort()]));
    writeFileSync(BASELINE, `${JSON.stringify(baseline, null, 4)}\n`);
}
