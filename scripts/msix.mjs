// Builds Glint for the Microsoft Store as an MSIX package (docs/releasing.md, The Microsoft Store): the
// app with a data folder of its own (src-tauri/tauri.msix.conf.json), laid out with the instant search
// service, the tools that come with Glint, the notices, the Store's logos at every scale and
// AppxManifest.xml (src-tauri/msix), and packed by the Windows SDK's makeappx into
// release/v<version>/Glint_<version>_x64.msix. Nothing here signs it: the Store does, once Partner Center
// has it.
//
// The package's identity is the one Partner Center gives the app (Product identity), in
// src-tauri/msix/store.json. Until it's there, the package has a test identity, to try here and not for
// the Store (its name ends in _test).
//
// Usage: npm run msix [-- --no-build | --from <folder>] [--try | --untry]
//   --no-build  package what the release build's folder holds (built with tauri.msix.conf.json already)
//   --from      package the app built in another folder (a test build), without building
//   --try       a test package (its own identity and folder), installed here from its folder: needs
//               Developer Mode, and starts from Start like the Store's would
//   --untry     uninstall that test package

import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const args = process.argv.slice(2);
const tryIt = args.includes("--try");
const from = args.includes("--from") ? resolve(args[args.indexOf("--from") + 1]) : undefined;

const fail = (message) => {
    console.error(`\n✗ ${message}`);
    process.exit(1);
};
const run = (cmd, cmdArgs, options = {}) => {
    const result = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8", ...options });
    if (result.error) fail(`${cmd}: ${result.error.message}`);
    if (result.status !== 0) fail(`${cmd} ${cmdArgs.join(" ")} failed:\n${(result.stdout ?? "").trim()}\n${(result.stderr ?? "").trim()}`.trim());
    return result;
};
const powershell = (script) => run("powershell", ["-NoProfile", "-NonInteractive", "-Command", script]);
const json = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));

// A test package's identity: never the Store's, so a copy tried here can't stand in its way.
const TEST = { name: "Glint.Test", publisher: "CN=Glint Test", publisherDisplayName: "Glint (test)", displayName: "Glint (test)" };

if (args.includes("--untry")) {
    powershell(`Get-AppxPackage -Name '${TEST.name}' | Remove-AppxPackage`);
    console.log(`✓ The test package ${TEST.name} is uninstalled.`);
    process.exit(0);
}

const config = json("src-tauri/tauri.conf.json");
const version = json("package.json").version;
if (!/^\d+\.\d+\.\d+$/.test(version)) fail(`An MSIX version has four numbers, so ${version} can't be one`);
const store = json("src-tauri/msix/store.json");
const fromPartnerCenter = ["name", "publisher", "publisherDisplayName", "displayName"].every((key) => store[key]);
const identity = fromPartnerCenter && !tryIt ? store : TEST;
if (identity === TEST && !tryIt) console.warn("! src-tauri/msix/store.json has no identity from Partner Center yet, so this is a test package, not one for the Store.");

// makeappx and makepri, from the newest Windows SDK.
const kits = "C:\\Program Files (x86)\\Windows Kits\\10\\bin";
const sdk = existsSync(kits)
    ? readdirSync(kits)
          .filter((v) => /^10\./.test(v) && existsSync(join(kits, v, "x64", "makeappx.exe")))
          .sort((a, b) => a.localeCompare(b, undefined, { numeric: true }))
          .pop()
    : undefined;
if (!sdk) fail("The Windows SDK (makeappx) isn't installed: Visual Studio's C++ tools bring it, or winget install Microsoft.WindowsSDK.10.0.26100");
const tool = (name) => join(kits, sdk, "x64", name);

if (!args.includes("--no-build") && !from) {
    console.log("Building Glint for the Store's package (its own data folder)…");
    const cli = join(root, "node_modules", "@tauri-apps", "cli", "tauri.js");
    run(process.execPath, [cli, "build", "--no-bundle", "--config", "src-tauri/tauri.msix.conf.json"], { stdio: "inherit" });
}

// The files the setup program installs, but its uninstaller: the app, the service, the tools Glint
// comes with and the notices.
const out = join(root, "release", `v${version}`);
// A package tried here runs from its folder, so that one is its own.
const layout = join(out, tryIt ? "msix-try" : identity === TEST ? "msix-test" : "msix");
rmSync(layout, { recursive: true, force: true });
mkdirSync(join(layout, "Assets"), { recursive: true });
const built = from ?? join(process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(root, "src-tauri", "target"), "release");
// (cargo build names the app after its crate; tauri build, after mainBinaryName.)
const crate = readFileSync(join(root, "src-tauri", "Cargo.toml"), "utf8").match(/^\[package\][^[]*?^name = "([^"]+)"/m)?.[1];
const app = `${config.mainBinaryName}.exe`;
for (const exe of [app, "shadui-index.exe", ...config.bundle.externalBin.map((bin) => `${bin.split("/").pop()}.exe`)]) {
    const source = [exe, exe === app && `${crate}.exe`].filter(Boolean).map((name) => join(built, name)).find(existsSync);
    if (!source) fail(`${exe} isn't in ${built}: build first (without --no-build)`);
    copyFileSync(source, join(layout, exe));
}
for (const [from, to] of Object.entries(config.bundle.resources)) copyFileSync(join(root, "src-tauri", from), join(layout, to));

console.log("Drawing the logos…");
run("powershell", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", join(root, "scripts", "msix-assets.ps1"), "-Source", join(root, "src-tauri", "icons", "icon.png"), "-Out", join(layout, "Assets")]);

// Four numbers, the last 0: the Store keeps that one for itself.
const xml = (text) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
let manifest = readFileSync(join(root, "src-tauri", "msix", "AppxManifest.xml"), "utf8");
for (const [key, value] of Object.entries({ ...identity, version: `${version}.0` })) manifest = manifest.replaceAll(`{{${key}}}`, xml(value));
if (/\{\{\w+\}\}/.test(manifest)) fail(`AppxManifest.xml still has ${manifest.match(/\{\{\w+\}\}/)[0]}`);
writeFileSync(join(layout, "AppxManifest.xml"), manifest);

// resources.pri tells Windows which logo is for which scale and size.
const priConfig = join(out, "msix-priconfig.xml");
run(tool("makepri.exe"), ["createconfig", "/cf", priConfig, "/dq", "en-US", "/pv", "10.0.0", "/o"]);
// One index with every scale: makepri's config would split the scales out for a bundle's resource
// packages, which a single package doesn't have, so Windows would only find the 100% logos.
writeFileSync(priConfig, readFileSync(priConfig, "utf8").replace(/<packaging>[\s\S]*?<\/packaging>/, ""));
run(tool("makepri.exe"), ["new", "/pr", layout, "/cf", priConfig, "/mn", join(layout, "AppxManifest.xml"), "/of", join(layout, "resources.pri"), "/o"]);
rmSync(priConfig, { force: true });

const msix = join(out, `${config.productName}_${version}_x64${identity === TEST ? "_test" : ""}.msix`);
run(tool("makeappx.exe"), ["pack", "/d", layout, "/p", msix, "/o"]);
console.log(`✓ ${msix}`);

if (tryIt) {
    // From its folder, as Developer Mode allows, so nothing has to sign it.
    powershell(`Add-AppxPackage -Register '${join(layout, "AppxManifest.xml")}' -ForceUpdateFromAnyVersion`);
    const family = powershell(`(Get-AppxPackage -Name '${TEST.name}').PackageFamilyName`).stdout.trim();
    console.log(`✓ Installed for trying: Glint (test) in Start, or explorer shell:AppsFolder\\${family}!Glint. npm run msix -- --untry takes it out.`);
} else if (identity !== TEST) {
    console.log("Next: Partner Center → the app → Packages: upload it. The Store signs it.");
}
