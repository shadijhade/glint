// Drafts the notes for the release of package.json's version: what's new by area, what's faster and
// what's fixed since the release before it, from the commits' subjects by git-cliff (cliff.toml), into
// release/v<version>/notes.md, which `npm run release` hands to `gh release create`. It's a draft:
// read it and edit it (docs/releasing.md).
//
// Usage: npm run notes    (needs git-cliff: winget install orhun.git-cliff)

import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const REPO = "https://github.com/shadijhade/glint";
const root = resolve(import.meta.dirname, "..");
const fail = (message) => {
    console.error(`✗ ${message}`);
    process.exit(1);
};
const git = (...args) => spawnSync("git", args, { cwd: root, encoding: "utf8" });

const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const tag = `v${version}`;
// A version already released behind HEAD hasn't been bumped for the next one yet.
const tagged = git("rev-parse", "--verify", "--quiet", `refs/tags/${tag}^{commit}`).stdout.trim();
if (tagged && tagged !== git("rev-parse", "HEAD").stdout.trim()) fail(`${tag} is already released: bump the version first (docs/releasing.md, Every release).`);
// The release before this one: the newest version tag behind HEAD, other than this version's own.
const previous = git("describe", "--tags", "--abbrev=0", "--match", "v[0-9]*", "--exclude", tag, "HEAD").stdout.trim();
if (!previous) fail(`No release before ${tag} was found behind HEAD.`);

const cliff = spawnSync("git-cliff", ["--config", join(root, "cliff.toml"), "--tag", tag, `${previous}..HEAD`], { cwd: root, encoding: "utf8" });
if (cliff.error) fail(cliff.error.code === "ENOENT" ? "git-cliff isn't installed: winget install orhun.git-cliff" : cliff.error.message);
if (cliff.status) fail(cliff.stderr.trim());
const notes = `${cliff.stdout.trim()}\n\n**Every change**: ${REPO}/compare/${previous}...${tag}\n`;
const out = join(root, "release", tag);
mkdirSync(out, { recursive: true });
writeFileSync(join(out, "notes.md"), notes);
console.log(notes);
console.log(`✓ release/${tag}/notes.md drafts what changed since ${previous}. Edit it: npm run release hands it to gh release create.`);
