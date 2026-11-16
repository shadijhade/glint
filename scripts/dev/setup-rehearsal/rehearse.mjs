// A dress rehearsal of `npm run ship -- --setup` (scripts/ship-setup.mjs) in a real terminal, with nobody at it:
// a throwaway test key (scripts/lib/test-vectors), a stand-in for gh that keeps what it is given, and a helper
// that types into this console as a person would: yes, a wrong password, the right one, and no to the Store.
// Run from the repository, in a terminal: node scripts\dev\setup-rehearsal\rehearse.mjs   (about 40 seconds)
// It ends with PASS or FAIL. Nothing real is read, stored or sent: the key is a test key nothing trusts, and
// the gh it talks to is gh-stand-in.mjs. Why it exists: the setup's first real run took an empty password,
// because its yes was asked through Node's readline and the password through Read-Host, and what was typed
// for the second never reached it. No test without a terminal can see that (scripts/lib/ask.mjs).

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const here = import.meta.dirname;
const root = join(here, "..", "..", "..");
const vectors = join(root, "scripts", "lib", "test-vectors");
const work = mkdtempSync(join(tmpdir(), "glint-setup-rehearsal-"));
const world = join(work, "world.json");
writeFileSync(
    world,
    JSON.stringify({
        calls: [],
        runs: [],
        secrets: [
            { name: "TAURI_SIGNING_PRIVATE_KEY", updatedAt: "2026-10-22T13:23:27Z", value: "" },
            { name: "TAURI_SIGNING_PRIVATE_KEY_PASSWORD", updatedAt: "2026-10-22T13:23:37Z", value: "an old password" },
        ],
    }),
);
const type = (text, delay) =>
    spawn("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(here, "conin.ps1"), "-DelayMs", String(delay), "-Text", text], { stdio: "ignore" });
type("y", 6000);
type("not-it", 12000);
type("w14-test-password", 20000);
type("n", 30000);

const env = { ...process.env, GLINT_GH: join(here, "gh-stand-in.mjs"), GLINT_FAKE_GH: world, GLINT_ROOT: root, GLINT_SETUP_WAIT: "0.05", GLINT_UPDATE_PUBKEY: join(vectors, "test-pw.key.pub"), TAURI_SIGNING_PRIVATE_KEY: "" };
for (const name of ["TAURI_SIGNING_PRIVATE_KEY_PASSWORD", "PARTNER_CENTER_TENANT_ID", "PARTNER_CENTER_CLIENT_ID", "PARTNER_CENTER_CLIENT_SECRET"]) delete env[name];
const child = spawn(process.execPath, [join(root, "scripts", "ship-setup.mjs"), "--key", join(vectors, "test-pw.key")], { cwd: root, stdio: "inherit", env });
child.on("close", (status) => {
    const after = JSON.parse(readFileSync(world, "utf8"));
    rmSync(work, { recursive: true, force: true });
    const password = after.calls.find((call) => call.args[2] === "TAURI_SIGNING_PRIVATE_KEY_PASSWORD")?.value;
    const said = (after.runs[0]?.log ?? "").split("\n").find((line) => line.startsWith("release-check"));
    console.log("\n==== dress rehearsal ====");
    console.log(`exit code: ${status}`);
    console.log(`stored: ${after.calls.map((call) => call.args.slice(0, 3).join(" ")).join("; ") || "nothing"}`);
    console.log(`the password stored is the right one (the second typed): ${password === "w14-test-password"}`);
    console.log(`the check, with what was stored: ${said}`);
    console.log(status === 0 && password === "w14-test-password" && /"key":"ok"/.test(said ?? "") ? "PASS" : "FAIL");
});
