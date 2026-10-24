// Writes THIRD_PARTY_CRATES.md: the crates in Glint's Windows build, each license's text with the crates
// it covers, by cargo-about from src-tauri/Cargo.lock with src-tauri/about.toml and about.hbs. The
// installer ships it, so run this when crates change. With --check it only says whether the file is
// current (exit 1 when it isn't, 2 when cargo-about isn't installed), for `npm run release`.
//
// Usage: npm run licenses [-- --check]    (needs cargo-about: cargo install --locked cargo-about)

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const file = join(root, "THIRD_PARTY_CRATES.md");
const check = process.argv.includes("--check");

const work = mkdtempSync(join(tmpdir(), "glint-licenses-"));
const made = join(work, "THIRD_PARTY_CRATES.md");
const about = spawnSync(
    "cargo",
    ["about", "generate", "--manifest-path", "src-tauri/Cargo.toml", "-c", "src-tauri/about.toml", "-o", made, "src-tauri/about.hbs"],
    { cwd: root, encoding: "utf8" },
);
// Some crates' license files have CRLF line ends, and Git may check the file out with them: compare and
// write LF only.
const lf = (s) => s.replace(/\r\n/g, "\n");
const text = existsSync(made) ? lf(readFileSync(made, "utf8")) : null;
rmSync(work, { recursive: true, force: true });
if (/no such (command|subcommand)/.test(about.stderr ?? "")) {
    console.error("cargo-about isn't installed (cargo install --locked cargo-about), so THIRD_PARTY_CRATES.md wasn't checked.");
    process.exit(2);
}
if (about.error || about.status || text === null) {
    console.error(`cargo about failed:\n${(about.stderr || about.error?.message || "").trim()}`);
    process.exit(1);
}

const current = existsSync(file) ? lf(readFileSync(file, "utf8")) : "";
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
