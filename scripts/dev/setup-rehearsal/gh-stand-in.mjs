// A stand-in for gh, for rehearsing `npm run ship -- --setup` (scripts/lib/hands-off.test.mjs without a terminal,
// rehearse.mjs in one). It keeps its world in a file (GLINT_FAKE_GH), takes secrets on its standard input, and
// its "Release check" is the real scripts/release-check.mjs run with the secrets it holds, as the workflow runs
// it on GitHub (GLINT_ROOT is the repository). Throwaway values only: it writes what it is given to that file.
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
const file = process.env.GLINT_FAKE_GH;
const world = JSON.parse(readFileSync(file, "utf8"));
const args = process.argv.slice(2);
const line = args.join(" ");
const save = () => writeFileSync(file, JSON.stringify(world));
if (line === "auth status") process.exit(0);
else if (line.startsWith("secret list")) console.log(JSON.stringify(world.secrets.map(({ name, updatedAt }) => ({ name, updatedAt }))));
else if (line.startsWith("secret set")) {
    const value = readFileSync(0, "utf8");
    world.calls.push({ args, value });
    world.secrets = world.secrets.filter((s) => s.name !== args[2]).concat({ name: args[2], updatedAt: new Date().toISOString(), value });
    save();
} else if (line.startsWith("secret delete")) {
    world.calls.push({ args });
    world.secrets = world.secrets.filter((s) => s.name !== args[2]);
    save();
} else if (line.startsWith("run list")) console.log(JSON.stringify(world.runs.map(({ log, ...run }) => run)));
else if (line.startsWith("workflow run")) {
    const held = Object.fromEntries(world.secrets.map((s) => [s.name, s.value ?? ""]));
    const env = { ...process.env, TAURI_SIGNING_PRIVATE_KEY: "", TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "", PARTNER_CENTER_TENANT_ID: "", PARTNER_CENTER_CLIENT_ID: "", PARTNER_CENTER_CLIENT_SECRET: "", ...held };
    const ran = spawnSync(process.execPath, ["scripts/release-check.mjs"], { cwd: process.env.GLINT_ROOT, env, encoding: "utf8" });
    world.runs.unshift({ databaseId: 500 + world.runs.length, status: "completed", conclusion: ran.status === 0 ? "success" : "failure", createdAt: new Date().toISOString(), log: ran.stdout });
    save();
} else if (line.startsWith("run view") && line.includes("--log")) console.log(world.runs.find((r) => String(r.databaseId) === args[2]).log);
else if (line.startsWith("run view")) console.log(JSON.stringify(world.runs.find((r) => String(r.databaseId) === args[2])));
else {
    console.error("the stand-in gh doesn't know: " + line);
    process.exit(1);
}
