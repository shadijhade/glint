// Checks that Windows 10 can start Glint.exe, the instant search service and the bundled rg and fd: that
// every DLL and function they import is one Windows 10 1809 has, in every edition (scripts/lib/windows-imports.mjs
// says why: issue #6). The installer and MSIX scripts run the same check on what they pack; this is for a
// build of your own, and for adding imports you've checked to scripts/windows10-imports.json.
//
// Usage: npm run check-imports [-- <program.exe> ...] [--update]
//   <program.exe>  the programs to check (default: Glint.exe, shadui-index.exe, rg.exe and fd.exe in
//                  <target>/release, the tools from crates/glint/binaries where the build hasn't put them there)
//   --update       add every import that isn't listed yet to scripts/windows10-imports.json: only once you've
//                  checked that Windows 10 1809 has each one. The DLLs known to be missing somewhere never go in.

import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { addToBaseline, checkWindows10, isBundledTool, windows10Report } from "./lib/windows-imports.mjs";
import { root, targetDir } from "./lib/project.mjs";

const args = process.argv.slice(2);
const update = args.includes("--update");
const given = args.filter((a) => a !== "--update").map((a) => resolve(a));

const release = join(targetDir(), "release");
const files = given.length
    ? given
    : [
          join(release, "Glint.exe"),
          join(release, "shadui-index.exe"),
          ...["rg", "fd"].map((t) => [join(release, `${t}.exe`), join(root, "crates", "glint", "binaries", `${t}-x86_64-pc-windows-msvc.exe`)].find(existsSync) ?? join(release, `${t}.exe`)),
      ];
const absent = files.filter((f) => !existsSync(f));
if (absent.length) {
    console.error(`✗ Not built: ${absent.join(", ")} (cargo build --release -p glint -p shadui-index, or name the programs to check).`);
    process.exit(1);
}

if (update) {
    const { unknown } = checkWindows10(files);
    if (unknown.length) {
        addToBaseline(unknown);
        console.log(`Added to scripts/windows10-imports.json:\n${unknown.map((u) => `  ${u.dll}: ${u.fn}`).join("\n")}\n`);
    }
}
// Glint's own programs have to pass. The bundled tools are their projects' official builds, which Glint starts
// without (only what runs one fails), so what they lack is said, not failed on.
const { problems, warnings } = windows10Report(files);
if (warnings.length) console.log(`! Bundled tools that might not run on every Windows 10 (Glint still starts):\n${warnings.map((w) => `  - ${w}`).join("\n")}\n`);
if (problems.length) {
    console.error(`✗ Windows 10 might not start Glint:\n${problems.map((p) => `  - ${p}`).join("\n")}`);
    process.exit(1);
}
const passed = files.filter((f) => !isBundledTool(f));
if (passed.length || !warnings.length) console.log(`✓ Windows 10 has every DLL and function these import:\n${(passed.length ? passed : files).map((f) => `  ${f}`).join("\n")}`);
