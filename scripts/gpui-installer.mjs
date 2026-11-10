// Builds the GPUI Glint's setup program: crates/glint/installer/installer.nsi packed by makensis, written to
// <target>/release/bundle/Glint-setup.exe (target/ui/release/bundle/Glint-setup.exe with the usual
// CARGO_TARGET_DIR). It replaces Tauri's bundler for the GPUI build; docs/gpui/migration-plan.md, phase 6.
//
// It builds Glint.exe (cargo build --release -p glint), gathers what the installer carries into one folder
// (Glint.exe, rg.exe, fd.exe and pdu.exe beside it, the license and the notices, and the instant search
// service when there is one), and runs makensis. It does not sign, publish or install anything: code signing
// and the updater's signature belong to the release pipeline (see "Signing" below and
// crates/glint/installer/README.md).
//
// Usage: node scripts/gpui-installer.mjs [--dry-run] [--exe <Glint.exe>] [--no-build] [--out <file>]
//                                        [--service <shadui-index.exe> | --no-service] [--service-sig <file>]
//
//   --dry-run     checks that everything the installer packs is there (and where makensis is), prints what it
//                 would build and the makensis command, and builds nothing. Exit code 1 if something's missing.
//   --exe <path>  packs this Glint.exe instead of building one (a debug build, to try the installer without a
//                 release build). The installer gets the workspace version either way.
//   --no-build    packs target/<profile>/Glint.exe as it is, without running cargo.
//   --out <path>  where to write the setup (default <target>/release/bundle/Glint-setup.exe).
//   --service     the instant search service to carry (default: shadui-index.exe in <target>/release, then in
//                 src-tauri/target/release); --no-service leaves it out. --service-sig is its update signature
//                 (default: the .sig next to it, then src-tauri/target/service-signature/); without one an
//                 installed service updates only with administrator approval.
//
// Environment: CARGO_TARGET_DIR (default <repo>/target/ui), MAKENSIS (a makensis.exe to use).
//
// Signing: set GLINT_SIGN_COMMAND to a command line with %1 where a file's path goes, such as
// `signtool sign /fd SHA256 /a "%1"`; it is then run on Glint.exe, rg.exe, fd.exe, pdu.exe and the service
// before they are packed, on the uninstaller (makensis's !uninstfinalize) and on the finished setup. Left
// unset, nothing is signed and the setup says so. The script never reads a key: the command does whatever the
// person who runs the release has set it up to do. Signing the service changes it after its update signature
// was made, so the script drops that signature when it signs the service.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { delimiter, join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const option = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
const dryRun = flag("--dry-run");

const problems = [];
const notes = [];
const problem = (message) => problems.push(message);
const note = (message) => notes.push(message);
const fail = (message) => {
    console.error(`\n✗ ${message}`);
    process.exit(1);
};

// 1. Where things are.
const installerDir = join(root, "crates", "glint", "installer");
const script = join(installerDir, "installer.nsi");
const targetDir = resolve(process.env.CARGO_TARGET_DIR ?? join(root, "target", "ui"));
const releaseDir = join(targetDir, "release");
const bundleDir = join(releaseDir, "bundle");
const stage = join(bundleDir, "stage");
const out = resolve(option("--out") ?? join(bundleDir, "Glint-setup.exe"));
const signCommand = process.env.GLINT_SIGN_COMMAND?.trim() || "";

// The workspace version (the one Glint.exe's version info and the updater's comparison use).
const manifest = readFileSync(join(root, "Cargo.toml"), "utf8");
const version = manifest.match(/\[workspace\.package\][^[]*?\nversion\s*=\s*"(\d+\.\d+\.\d+)"/)?.[1];
if (!version) fail("Couldn't read [workspace.package] version (x.y.z) from Cargo.toml.");

// 2. makensis: MAKENSIS, the PATH, where Tauri's bundler keeps one, NSIS's own folders.
function findMakensis() {
    const candidates = [];
    if (process.env.MAKENSIS) candidates.push(process.env.MAKENSIS);
    for (const dir of (process.env.PATH ?? "").split(delimiter)) if (dir) candidates.push(join(dir, "makensis.exe"));
    if (process.env.LOCALAPPDATA) candidates.push(join(process.env.LOCALAPPDATA, "tauri", "NSIS", "makensis.exe"));
    for (const v of ["ProgramFiles(x86)", "ProgramFiles"]) if (process.env[v]) candidates.push(join(process.env[v], "NSIS", "makensis.exe"));
    return candidates.find((c) => existsSync(c));
}
const makensis = findMakensis();
if (!makensis) {
    problem("makensis.exe wasn't found (not on the PATH, not in %LOCALAPPDATA%\\tauri\\NSIS, not in Program Files\\NSIS): install NSIS 3 (winget install NSIS.NSIS) or set MAKENSIS.");
}
const nsisHome = makensis ? resolve(makensis, "..") : undefined;
if (nsisHome) {
    // What the script includes from NSIS's own Include folder.
    for (const include of ["MUI2.nsh", "FileFunc.nsh", "x64.nsh", "WordFunc.nsh", "StrFunc.nsh", "LogicLib.nsh", "nsDialogs.nsh",
        "Win/COM.nsh", "Win/Propkey.nsh", "Win/RestartManager.nsh"]) {
        if (!existsSync(join(nsisHome, "Include", include))) problem(`${join(nsisHome, "Include", include)} is missing: that NSIS is too old or incomplete (3.x is needed).`);
    }
}

// 3. What the installer is made of: its own files, the art, the icon.
const own = ["installer.nsi", "hooks.nsh", "look.nsh", "tools.nsh", "utils.nsh", "strings.nsh"];
for (const file of own) if (!existsSync(join(installerDir, file))) problem(`crates/glint/installer/${file} is missing.`);
const art = [];
for (const kind of ["header", "sidebar"]) for (const scale of [100, 125, 150, 175, 200, 225, 250, 300]) art.push(`${kind}-${scale}.bmp`);
for (const file of art) if (!existsSync(join(installerDir, "art", file))) problem(`crates/glint/installer/art/${file} is missing (scripts/installer-art.mjs draws them).`);
if (!existsSync(join(root, "crates", "glint", "resources", "icon.ico"))) problem("crates/glint/resources/icon.ico is missing.");

// The Extra tools page lists the tools Glint can install. While the old app is still in the tree its copy
// of the page is the one glint-core's test checks against its tool list, so the two must not drift.
const oldTools = join(root, "src-tauri", "installer", "tools.nsh");
if (existsSync(oldTools) && existsSync(join(installerDir, "tools.nsh"))) {
    if (!readFileSync(oldTools).equals(readFileSync(join(installerDir, "tools.nsh")))) {
        note("crates/glint/installer/tools.nsh differs from src-tauri/installer/tools.nsh: copy the newer one over the other (glint-core's tools test reads the old one until src-tauri goes).");
    }
}

// 4. What the installer carries.
const exeOption = option("--exe");
const glintExe = exeOption ? resolve(exeOption) : join(releaseDir, "Glint.exe");
const willBuild = !exeOption && !flag("--no-build");
/** rg.exe, fd.exe and pdu.exe: next to the built Glint.exe (crates/glint/build.rs puts them there), else the official ones in src-tauri/binaries. */
function tool(name) {
    const beside = join(resolve(glintExe, ".."), `${name}.exe`);
    if (existsSync(beside)) return beside;
    const official = join(root, "src-tauri", "binaries", `${name}-x86_64-pc-windows-msvc.exe`);
    return existsSync(official) ? official : undefined;
}
const files = [
    { name: "Glint.exe", from: glintExe, built: willBuild },
    ...["rg", "fd", "pdu"].map((t) => ({ name: `${t}.exe`, from: tool(t) ?? join(resolve(glintExe, ".."), `${t}.exe`), built: willBuild && !tool(t) })),
    { name: "LICENSE.txt", from: join(root, "LICENSE") },
    { name: "THIRD_PARTY_LICENSES.md", from: join(root, "THIRD_PARTY_LICENSES.md") },
    { name: "THIRD_PARTY_CRATES.md", from: join(root, "THIRD_PARTY_CRATES.md") },
];
for (const file of files) {
    if (!file.built && !existsSync(file.from)) problem(`${file.from} is missing (it goes in the installer as ${file.name}).`);
}

// The service: optional. Glint.exe finds it beside itself (glint-core's index.rs); without it the Extra tools
// page's Instant search row and Settings can't turn the index on.
let service;
let serviceSig;
if (!flag("--no-service")) {
    const given = option("--service");
    service = [given && resolve(given), join(releaseDir, "shadui-index.exe"), join(root, "src-tauri", "target", "release", "shadui-index.exe")]
        .find((p) => p && existsSync(p));
    if (given && !service) problem(`--service ${given} doesn't exist.`);
    if (service) {
        const givenSig = option("--service-sig");
        serviceSig = [givenSig && resolve(givenSig), `${service}.sig`, join(root, "src-tauri", "target", "service-signature", "shadui-index.exe.sig")]
            .find((p) => p && existsSync(p));
        if (givenSig && !serviceSig) problem(`--service-sig ${givenSig} doesn't exist.`);
        if (serviceSig && statSync(serviceSig).size < 64) serviceSig = undefined; // release.mjs's placeholder text, not a signature
        if (!serviceSig) note("No update signature for shadui-index.exe: an installed service will update only with administrator approval.");
    } else {
        note("No shadui-index.exe found (build it, or pass --service): this installer leaves out the instant search service, so turning instant search on from it fails with \"missing next to the app\".");
    }
}

// 5. The commands.
const cargoArgs = ["build", "--release", "-p", "glint"];
const nsisArgs = (setup) => [
    "/V3",
    `/DSTAGE=${stage}`,
    `/DOUTFILE=${setup}`,
    `/DVERSION=${version}`,
    `/DVERSIONWITHBUILD=${version}.0`,
    ...(service ? ["/DHAVE_SERVICE"] : []),
    ...(service && serviceSig && !signCommand ? ["/DHAVE_SERVICE_SIG"] : []),
    ...(signCommand ? [`/DUNINSTALLERSIGNCOMMAND=${signCommand}`] : []),
    script,
];
const quote = (a) => (/[\s"]/.test(a) ? `"${a.replace(/"/g, '\\"')}"` : a);

console.log(`Glint ${version} installer (${dryRun ? "dry run" : "build"})`);
console.log(`  makensis : ${makensis ?? "NOT FOUND"}`);
console.log(`  target   : ${targetDir}`);
console.log(`  packs    :`);
for (const file of files) console.log(`    ${file.name.padEnd(26)} ${existsSync(file.from) ? file.from : file.built ? "(built by cargo)" : "MISSING"}`);
console.log(`    ${"shadui-index.exe".padEnd(26)} ${service ?? "(left out)"}${service && serviceSig ? ` + signature ${serviceSig}` : ""}`);
console.log(`  art      : ${art.length} bitmaps in crates/glint/installer/art`);
console.log(`  signing  : ${signCommand ? `GLINT_SIGN_COMMAND (${signCommand})` : "none (set GLINT_SIGN_COMMAND; the release pipeline signs with the maintainer's keys)"}`);
console.log(`  output   : ${out}`);
if (willBuild) console.log(`\n  1. CARGO_TARGET_DIR=${targetDir}  cargo ${cargoArgs.join(" ")}`);
console.log(`  ${willBuild ? "2" : "1"}. stage the files in ${stage}`);
console.log(`  ${willBuild ? "3" : "2"}. ${quote(makensis ?? "makensis")} ${nsisArgs(out).map(quote).join(" ")}`);
for (const n of notes) console.log(`\n! ${n}`);
if (problems.length) {
    console.error(`\n✗ ${problems.length} problem${problems.length === 1 ? "" : "s"}:\n${problems.map((p) => `  - ${p}`).join("\n")}`);
    process.exit(1);
}
if (dryRun) {
    console.log("\n✓ Everything the installer packs is there. (--dry-run: nothing was built.)");
    process.exit(0);
}

// 6. Build.
const run = (cmd, cmdArgs, options = {}) => {
    const result = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8", ...options });
    if (result.error) fail(`${cmd}: ${result.error.message}`);
    return result;
};
if (willBuild) {
    console.log("\nBuilding Glint.exe (this takes a while)...");
    const build = run("cargo", cargoArgs, { stdio: "inherit", env: { ...process.env, CARGO_TARGET_DIR: targetDir } });
    if (build.status) fail("cargo build --release -p glint failed.");
    for (const file of files) if (!existsSync(file.from)) fail(`${file.from} wasn't built.`);
}

// Stage: the folder as it installs. Files are signed here, before they are packed.
rmSync(stage, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });
const sign = (path) => {
    if (!signCommand) return;
    const result = run(signCommand.replaceAll("%1", path), [], { shell: true, stdio: "inherit" });
    if (result.status) fail(`The signing command failed on ${path}.`);
};
for (const file of files) {
    copyFileSync(file.from, join(stage, file.name));
    if (file.name.endsWith(".exe")) sign(join(stage, file.name));
}
if (service) {
    copyFileSync(service, join(stage, "shadui-index.exe"));
    if (signCommand) {
        sign(join(stage, "shadui-index.exe"));
        if (serviceSig) note("The service was code-signed after its update signature was made, so the signature is left out (docs/releasing.md).");
    } else if (serviceSig) {
        copyFileSync(serviceSig, join(stage, "shadui-index.exe.sig"));
    }
}

mkdirSync(resolve(out, ".."), { recursive: true });
rmSync(out, { force: true });
console.log("\nPacking with makensis...");
const pack = run(makensis, nsisArgs(out), { cwd: installerDir });
const log = `${pack.stdout ?? ""}${pack.stderr ?? ""}`.trim();
if (pack.status || !existsSync(out)) {
    console.error(log);
    fail(`makensis failed (exit ${pack.status}).`);
}
// makensis's warnings (an unused variable, a function nothing calls), then the tail of its log: the size it wrote.
const lines = log.split(/\r?\n/);
const warnings = lines.filter((l) => /warning/i.test(l));
if (warnings.length) console.log(`makensis warned ${warnings.length} time${warnings.length === 1 ? "" : "s"}:\n${warnings.map((w) => `  ${w}`).join("\n")}`);
console.log(lines.slice(-14).join("\n"));
sign(out);
rmSync(stage, { recursive: true, force: true });

const size = statSync(out).size;
const sha = createHash("sha256").update(readFileSync(out)).digest("hex");
console.log(`\n✓ ${out}\n  ${(size / 1048576).toFixed(1)} MB, SHA-256 ${sha}${signCommand ? "" : "\n  Not code-signed, and no update signature: both are the release pipeline's (docs/releasing.md)."}`);
console.log("  Nothing was installed or run.");
