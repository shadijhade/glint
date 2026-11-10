// Writes THIRD_PARTY_CRATES.md: the crates in Glint's Windows build, each license's text with the crates
// it covers, by cargo-about from the root Cargo.lock with about.toml and about.hbs, then (scripts/lib/
// embedded-notices.mjs) the fonts, icons and data those crates embed under licenses cargo-about can't see:
// the KaTeX fonts, the Lucide icons, two-face's syntaxes and themes, Typst's assets, CSL styles... The
// installer and the Store package ship it, so run this when crates change.
//
// Usage: npm run licenses [-- --check | --embedded]
//   (no option)  writes the file; needs cargo-about (cargo install --locked cargo-about)
//   --check      only says whether the file is current (exit 1 when it isn't, 2 when cargo-about isn't
//                installed, which leaves the crate list unchecked: the embedded section is still checked and
//                the message says so), for `npm run release`
//   --embedded   rewrites only the embedded-assets section of the existing file, without cargo-about; the crate
//                list above it is left as it is, so it isn't current until cargo-about has run
//   --allowed    without cargo-deny: whether every crate of the Windows build has a license expression that
//                deny.toml's allow list meets (exit 1 and the crates that don't, if not). It reads each crate's
//                `license` field only: not advisories, not sources, not license files (what cargo-deny adds)

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { embeddedNotices, MARKER } from "./lib/embedded-notices.mjs";
import { windowsPackages } from "./lib/metadata.mjs";
import { root } from "./lib/project.mjs";
import { allowedLicenses, satisfied } from "./lib/spdx.mjs";

const file = join(root, "THIRD_PARTY_CRATES.md");
const check = process.argv.includes("--check");
const embeddedOnly = process.argv.includes("--embedded");

/** A crate with `license-file` and no `license`: MIT or Apache-2.0 when its file's text is plainly one of them (cargo-deny reads files the same way, with a confidence threshold). */
function fromLicenseFile(p) {
    if (!p.licenseFile) return undefined;
    try {
        const text = readFileSync(join(p.dir, p.licenseFile), "utf8").slice(0, 4000);
        if (/Permission is hereby granted, free of charge/.test(text)) return "MIT";
        if (/Apache Licenses+Version 2.0/.test(text)) return "Apache-2.0";
    } catch {
        // Unreadable: reported below as having no usable license.
    }
    return undefined;
}

if (process.argv.includes("--allowed")) {
    const allowed = allowedLicenses(readFileSync(join(root, "deny.toml"), "utf8"));
    const unmet = windowsPackages()
        // Glint's own crates (MIT, like the app) are ignored by deny.toml's `private`; the vendored GPUI crates are checked.
        .filter((p) => !(p.publish && p.publish.length === 0 && !p.source))
        .filter((p) => !satisfied(p.license ?? fromLicenseFile(p), allowed))
        .map((p) => `${p.name} ${p.version}: ${p.license ?? (p.licenseFile ? `license file ${p.licenseFile}` : "no license field")}`);
    if (unmet.length) {
        console.error(`${unmet.length} crate${unmet.length === 1 ? "" : "s"} under a license deny.toml's allow list doesn't meet (a decision, then deny.toml and about.toml):\n  ${unmet.join("\n  ")}`);
        process.exit(1);
    }
    console.log(`Every crate's license expression is met by deny.toml's allow list (${allowed.size} licenses).`);
    process.exit(0);
}

// Some crates' license files have CRLF line ends, and Git may check the file out with them: compare and
// write LF only.
const lf = (s) => s.replace(/\r\n/g, "\n");
const current = existsSync(file) ? lf(readFileSync(file, "utf8")) : "";
const head = (text) => (text.includes(MARKER) ? text.slice(0, text.indexOf(MARKER)) : text.trimEnd() + "\n\n");

let embedded;
try {
    embedded = lf(embeddedNotices());
} catch (e) {
    console.error(`The embedded fonts, icons and data couldn't be read: ${e.message}`);
    process.exit(1);
}

if (embeddedOnly) {
    const text = head(current) + embedded;
    if (text === current) console.log("THIRD_PARTY_CRATES.md's embedded-assets section is current.");
    else {
        writeFileSync(file, text);
        console.log("✓ THIRD_PARTY_CRATES.md's embedded-assets section written (the crate list above it was not regenerated: run npm run licenses with cargo-about).");
    }
    process.exit(0);
}

const work = mkdtempSync(join(tmpdir(), "glint-licenses-"));
const made = join(work, "THIRD_PARTY_CRATES.md");
const about = spawnSync("cargo", ["about", "generate", "--workspace", "--manifest-path", "Cargo.toml", "-c", "about.toml", "-o", made, "about.hbs"], { cwd: root, encoding: "utf8" });
const generated = existsSync(made) ? lf(readFileSync(made, "utf8")) : null;
rmSync(work, { recursive: true, force: true });
if (/no such (command|subcommand)/.test(about.stderr ?? "")) {
    const embeddedCurrent = current.endsWith(embedded) && current.includes(MARKER);
    console.error(`cargo-about isn't installed (cargo install --locked cargo-about), so THIRD_PARTY_CRATES.md's crate list wasn't ${check ? "checked" : "written"}; its embedded fonts, icons and data section is ${embeddedCurrent ? "current" : "out of date: npm run licenses -- --embedded"}.`);
    process.exit(2);
}
if (about.error || about.status || generated === null) {
    console.error(`cargo about failed:\n${(about.stderr || about.error?.message || "").trim()}`);
    process.exit(1);
}

const text = `${generated.trimEnd()}\n\n${embedded}`;
if (check) {
    if (current !== text) {
        console.error("THIRD_PARTY_CRATES.md doesn't match the crates: npm run licenses, then commit it.");
        process.exit(1);
    }
    console.log("THIRD_PARTY_CRATES.md matches the crates.");
} else if (current === text) {
    console.log("THIRD_PARTY_CRATES.md already matches the crates.");
} else {
    writeFileSync(file, text);
    console.log("✓ THIRD_PARTY_CRATES.md written: commit it.");
}
