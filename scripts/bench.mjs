// `npm run bench [-- <Glint.exe bench options>]`: runs `Glint.exe bench` (glint-core's bench) from the
// newest build there is (release, else debug), saves the run in bench-results/, and compares it with the
// run before it, measure by measure. Options go through to Glint: `--copy 5000` adds the copy measures,
// `--runs 15` runs each measure more often, and folders (a 100,000-file test folder, say) are listed too.
//
// Cold start: `npm run bench -- --start` launches Glint itself a few times and reports the time from
// launch to the first folder painted (the app writes it when GLINT_BENCH_START names a file).

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

const root = resolve(import.meta.dirname, "..");
const target = process.env.CARGO_TARGET_DIR ?? join(root, "target", "ui");
const builds = ["release", "debug"].map((b) => join(target, b, "Glint.exe")).filter(existsSync);
if (!builds.length) {
    console.error("No Glint.exe built yet: run `cargo build --release -p glint` (or a debug build) first.");
    process.exit(1);
}
const exe = builds.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)[0];
const results = join(root, "bench-results");
mkdirSync(results, { recursive: true });

let args = process.argv.slice(2);
const start = args.includes("--start");
args = args.filter((a) => a !== "--start");

const stamp = new Date().toISOString().replace(/[:.]/g, "-");
const out = join(results, `${stamp}.json`);
console.log(`${exe}\n`);
const ran = spawnSync(exe, ["bench", "--out", out, ...args], { stdio: ["ignore", "inherit", "inherit"] });
if (ran.status !== 0 || !existsSync(out)) process.exit(ran.status || 1);
const now = JSON.parse(readFileSync(out, "utf8"));

if (start) {
    now.measures.push(await coldStart(5));
    writeFileSync(out, JSON.stringify(now, null, 2));
}

// The run before this one, on this PC.
const earlier = readdirSync(results)
    .filter((f) => f.endsWith(".json") && join(results, f) !== out)
    .sort()
    .reverse()
    .map((f) => JSON.parse(readFileSync(join(results, f), "utf8")))
    .find((r) => r.cpu === now.cpu);
if (!earlier) {
    console.log(`\nSaved as ${out}. The next run will be compared with it.`);
    process.exit(0);
}
const before = new Map(earlier.measures.map((m) => [m.name, m]));
console.log(`\nCompared with the run of ${new Date(earlier.when).toLocaleString()} (version ${earlier.version}):\n`);
for (const m of now.measures) {
    const b = before.get(m.name);
    const delta = b ? ((m.medianMs - b.medianMs) / b.medianMs) * 100 : null;
    const mark = delta === null ? "  new" : `${delta >= 0 ? "+" : ""}${delta.toFixed(1)}%`;
    console.log(`${m.name.padEnd(46)} ${m.medianMs.toFixed(2).padStart(10)} ms  ${b ? `was ${b.medianMs.toFixed(2).padStart(10)} ms` : "".padStart(17)}  ${mark}`);
}

/** Launches Glint `times` times, each until it reports its first folder painted: the median, in ms. */
async function coldStart(times) {
    const file = join(tmpdir(), `glint-bench-start-${process.pid}.txt`);
    const ms = [];
    for (let i = 0; i < times; i++) {
        rmSync(file, { force: true });
        const child = spawn(exe, [], { env: { ...process.env, GLINT_BENCH_START: file }, stdio: "ignore" });
        const until = Date.now() + 30_000;
        while (!existsSync(file) && Date.now() < until) await new Promise((r) => setTimeout(r, 25));
        // The app quits itself once it has written the time; make sure.
        await new Promise((r) => setTimeout(r, 300));
        child.kill();
        if (existsSync(file)) ms.push(Number(readFileSync(file, "utf8")));
        await new Promise((r) => setTimeout(r, 500));
    }
    rmSync(file, { force: true });
    ms.sort((a, b) => a - b);
    const median = ms.length ? ms[Math.floor(ms.length / 2)] : 0;
    console.log(`${"start: launch to first folder painted".padEnd(44)} ${median.toFixed(2).padStart(10)} ms  (best ${(ms[0] ?? 0).toFixed(2).padStart(9)})  ${ms.length} launches`);
    return { name: "start: launch to first folder painted", medianMs: median, bestMs: ms[0] ?? 0, items: ms.length };
}
