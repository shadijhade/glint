// One command for a whole release (docs/releasing.md, "One command: npm run ship"): the checks, the version,
// the build and its signatures on GitHub, the release, Scoop, winget and the Microsoft Store.
//
// The update key never has to be on this PC: the release workflow holds it as a secret, builds and signs, and
// this script takes the files from the run, checks them the way the app will, and publishes them. Every step
// looks at what is already true and does only what is missing, so a run that stopped (a failed check, a build
// that broke, the network) is continued by running it again.
//
// Usage: npm run ship -- [<x.y.z>] [options]
//   <x.y.z>            the version to release; left out, the one in Cargo.toml when it has no release yet
//   --only <step,…>    just these steps      --from <step>   this step and the ones after it
//   --draft            stop before publishing: the draft release is there to read and try
//   --run <id>         take the build from this run of the Release workflow (one started before later commits)
//   --local            build and sign on this PC instead (npm run release: it asks for the update key's password)
//   --full             run glint-ui's tests and the end-to-end checks first (they take the mouse and keyboard)
//   --notes-as-drafted take git-cliff's draft of the notes as they are, when no notes were written
//   --no-scoop, --no-winget, --no-store   leave that one out
//   --listing          the Store's screenshots and words too (by itself when they changed since the last release)
//   --dry              say what each step would do, and change nothing
//
// Steps, in order: ready, checks, version, build, verify, release, scoop, winget, store.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { REPO, root, workspaceVersion } from "./lib/project.mjs";

const STEPS = ["ready", "checks", "version", "build", "verify", "release", "scoop", "winget", "store"];
const SLUG = REPO.replace("https://github.com/", "");
const BUCKET = "shadijhade/scoop-bucket";
const WINGET = "microsoft/winget-pkgs";
const WINGET_FORK = "shadijhade/winget-pkgs";
const PACKAGE = "shadijhade.Glint";
const WORKFLOW = "release.yml";

const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const value = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
const dry = flag("--dry");

class Stop extends Error {}
const stop = (message) => {
    throw new Stop(message);
};
const say = (message) => console.log(message);
const done = (message) => console.log(`✓ ${message}`);
const would = (message) => console.log(`  would: ${message}`);

/** Runs a program and returns what it printed; a failure stops the run with what it said. */
function run(cmd, cmdArgs, options = {}) {
    const result = spawnSync(cmd, cmdArgs, { cwd: root, encoding: "utf8", maxBuffer: 256 * 1024 * 1024, shell: false, ...options });
    if (result.error) stop(`${cmd}: ${result.error.message}`);
    if (result.status !== 0 && !options.allowFailure) {
        stop(`${cmd} ${cmdArgs.join(" ")} failed:\n${`${result.stdout ?? ""}\n${result.stderr ?? ""}`.trim()}`);
    }
    return { ok: result.status === 0, out: (result.stdout ?? "").trim(), err: (result.stderr ?? "").trim() };
}
const git = (...a) => run("git", a).out;
const gh = (...a) => run("gh", a).out;
const ghJson = (...a) => JSON.parse(gh(...a) || "null");
const tryGh = (...a) => run("gh", a, { allowFailure: true });
/** npm and winget are found through the shell on Windows (npm.cmd, an app alias). */
const shell = (line, options = {}) => run(line, [], { shell: true, ...options });
const has = (cmd, cmdArgs) => spawnSync([cmd, ...cmdArgs].join(" "), { cwd: root, encoding: "utf8", shell: true }).status === 0;
const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
const sleep = (seconds) => new Promise((resolve) => setTimeout(resolve, seconds * 1000));
const read = (path) => readFileSync(join(root, path), "utf8");

// ---- What is being released ----------------------------------------------------------------------------------

const asked = args.find((a) => /^\d+\.\d+\.\d+$/.test(a));
const version = asked ?? workspaceVersion();
const tag = `v${version}`;
const out = join(root, "release", tag);
const setup = `Glint_${version}_x64-setup.exe`;
const files = () => [join(out, "Glint-setup.exe"), join(out, setup), join(out, `${setup}.sig`), join(out, "latest.json")];
const notes = join(out, "notes.md");
const manifests = join(out, "package-manifests");
/** The commit the files in `out` were built from (the build step writes it): what the release's tag is put on. */
const builtFrom = join(out, "built-from.txt");
/** Runs of the workflow that built without the update key: not taken again. */
const unsignedRuns = join(out, "unsigned-runs.txt");
const UNSIGNED = [
    "The workflow built Glint but could not sign the update: the repository's TAURI_SIGNING_PRIVATE_KEY secret is empty",
    "(its runs say HAS_UPDATER_KEY: false), so there is no latest.json and no service signature, and the app couldn't",
    "install this release by itself. Either",
    '  - put the key there once:   Get-Content "$HOME\\.tauri\\shadui-ripgrep.key" -Raw | gh secret set TAURI_SIGNING_PRIVATE_KEY',
    "    then:                     npm run ship",
    "  - or build and sign here:   npm run ship -- --local     (asks for the key's password)",
].join("\n");
const releaseCommit = () => (existsSync(builtFrom) ? readFileSync(builtFrom, "utf8").trim() : git("rev-parse", "HEAD"));

/** The release on GitHub, or undefined: `{ isDraft, assets: [{ name, digest }], targetCommitish }`. */
function release() {
    const found = tryGh("release", "view", tag, "--repo", SLUG, "--json", "isDraft,assets,targetCommitish,tagName");
    return found.ok ? JSON.parse(found.out) : undefined;
}
/** The newest published release's version ("0.12.0"), for "what changed since". */
const lastReleased = () => ghJson("release", "list", "--repo", SLUG, "--exclude-drafts", "--limit", "20", "--json", "tagName,isLatest").find((r) => r.isLatest)?.tagName.replace(/^v/, "");

// ---- The steps -----------------------------------------------------------------------------------------------

const steps = {
    /** The tree is clean and in step with GitHub, gh is signed in, and the version is one that isn't out yet. */
    ready() {
        if (git("rev-parse", "--abbrev-ref", "HEAD") !== "master") stop("Releases are made from master.");
        if (git("status", "--porcelain") && !flag("--allow-dirty")) stop("There are uncommitted changes: commit them (or put them aside) first.");
        git("fetch", "origin", "master", "--tags", "--quiet");
        if (git("rev-list", "--count", "HEAD..origin/master") !== "0") stop("origin/master has commits this checkout doesn't: git pull first.");
        if (!tryGh("auth", "status").ok) stop("gh isn't signed in: gh auth login.");
        const made = release();
        if (made && !made.isDraft && !flag("--only") && !flag("--from")) stop(`${tag} is released already. For the next one: npm run ship -- <x.y.z>`);
        const latest = lastReleased();
        const newer = (a, b) => {
            const [x, y] = [a, b].map((v) => v.split(".").map(Number));
            const at = x.findIndex((n, i) => n !== y[i]);
            return at >= 0 && x[at] > y[at];
        };
        if (latest && !made && !newer(version, latest)) stop(`${version} isn't after the latest release (${latest}): updates never go backwards.`);
        done(`ready to release ${version}${latest ? ` (the latest is ${latest})` : ""}`);
    },

    /** The crates' checks and the signing's own test; with --full, glint-ui's tests and the end-to-end checks. */
    checks() {
        if (dry) return would("cargo deny check, the notices check, the signing test" + (flag("--full") ? ", glint-ui's tests and the end-to-end checks" : ""));
        if (has("cargo", ["deny", "--version"])) {
            run("cargo", ["deny", "check"]);
            done("cargo deny: advisories, bans, licenses, sources");
        } else say("! cargo-deny isn't installed: known vulnerabilities and licenses weren't checked (cargo install --locked cargo-deny)");
        if (has("cargo", ["about", "--version"])) {
            run("node", ["scripts/licenses.mjs", "--check"]);
            done("THIRD_PARTY_CRATES.md matches the crates");
        } else say("! cargo-about isn't installed: THIRD_PARTY_CRATES.md wasn't checked (cargo install --locked cargo-about)");
        run("node", ["scripts/lib/minisign-compare.test.mjs"]);
        done("the update signature is what the app verifies");
        if (flag("--full")) {
            say("glint-ui's tests (a few minutes)…");
            run("cargo", ["test", "-p", "glint-ui", "--lib"], { stdio: "inherit" });
            say("The end-to-end checks: leave the mouse and keyboard alone…");
            run("cargo", ["build", "-p", "glint"], { stdio: "inherit" });
            run("powershell", ["-STA", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "tests\\e2e\\run.ps1"], { stdio: "inherit" });
        }
    },

    /** The version in its three places, the notes, and the "Release vX.Y.Z" commit, pushed. */
    version() {
        const have = workspaceVersion();
        if (have !== version) {
            if (dry) return would(`set the version to ${version} (Cargo.toml, package.json, the bug report's placeholder), commit "Release ${tag}" and push`);
            const set = (path, from, to) => {
                const text = read(path);
                if (!text.includes(from)) stop(`${path} has no ${from}`);
                writeFileSync(join(root, path), text.replace(from, to));
            };
            set("Cargo.toml", `version = "${have}"`, `version = "${version}"`);
            set(".github/ISSUE_TEMPLATE/bug_report.yml", `placeholder: "${have}"`, `placeholder: "${version}"`);
            shell(`npm version ${version} --no-git-tag-version --allow-same-version`);
            // Only the workspace's own entries in Cargo.lock change.
            run("cargo", ["update", "--workspace", "--offline"]);
        }
        if (!existsSync(notes)) {
            if (dry) return would(`draft ${notes} from the commits`);
            run("node", ["scripts/notes.mjs"]);
            if (!flag("--notes-as-drafted")) stop(`The notes are drafted from the commits in ${notes}.\nRead and rewrite them for people who use Glint, then run npm run ship again (--notes-as-drafted takes them as they are).`);
        }
        if (!read("README.md").includes(`**${version}**`)) say(`! The README's "What's new" doesn't name ${version}: it still tells about the last release.`);
        if (JSON.parse(read("crates/glint/msix/listing.json")).whatsNew?.version !== version) say(`! crates/glint/msix/listing.json's whatsNew is for another version: the Store keeps its old note.`);
        if (git("status", "--porcelain")) {
            if (dry) return would(`commit "Release ${tag}" and push`);
            git("add", "Cargo.toml", "Cargo.lock", "package.json", "package-lock.json", ".github/ISSUE_TEMPLATE/bug_report.yml");
            git("commit", "--quiet", "-m", `Release ${tag}`);
        }
        if (git("rev-list", "--count", "origin/master..HEAD") !== "0") {
            if (dry) return would("git push origin master");
            git("push", "--quiet", "origin", "master");
        }
        done(`master is at ${git("rev-parse", "--short", "HEAD")}, version ${version}`);
    },

    /** The release workflow builds and signs this commit; its files come down into release/vX.Y.Z. */
    async build() {
        if (files().every(existsSync)) return done(`the build's files are in ${out}`);
        if (flag("--local")) {
            if (dry) return would("npm run release: build, sign (it asks for the update key's password) and lay out the files here");
            // The key's path, not the key: release.mjs unlocks it with the password it asks for.
            const key = process.env.TAURI_SIGNING_PRIVATE_KEY ?? join(process.env.USERPROFILE ?? "", ".tauri", "shadui-ripgrep.key");
            run("node", ["scripts/release.mjs"], { stdio: "inherit", env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: key } });
            writeFileSync(builtFrom, `${git("rev-parse", "HEAD")}\n`);
            return done(`built and signed here; its files are in ${out}`);
        }
        const head = git("rev-parse", "HEAD");
        const unsigned = existsSync(unsignedRuns) ? readFileSync(unsignedRuns, "utf8").split(/\s+/).filter(Boolean) : [];
        const runsOf = () => ghJson("run", "list", "--repo", SLUG, "--workflow", WORKFLOW, "--limit", "20", "--json", "databaseId,headSha,status,conclusion,event").filter((r) => r.headSha === head);
        let build = value("--run")
            ? ghJson("run", "view", value("--run"), "--repo", SLUG, "--json", "databaseId,status,conclusion,headSha")
            : runsOf().find((r) => (r.conclusion === "success" || r.status !== "completed") && !unsigned.includes(String(r.databaseId)));
        if (!build) {
            if (dry) return would(`start the Release workflow on master (${head.slice(0, 7)}), wait for it, and download its files`);
            gh("workflow", "run", WORKFLOW, "--repo", SLUG, "--ref", "master");
            for (let i = 0; i < 20 && !build; i++) {
                await sleep(6);
                build = runsOf().find((r) => r.status !== "completed" && !unsigned.includes(String(r.databaseId)));
            }
            if (!build) stop("The Release workflow didn't start: look at the repository's Actions page.");
            say(`The Release workflow builds and signs ${tag}: ${REPO}/actions/runs/${build.databaseId} (about three quarters of an hour).`);
        } else if (dry) return would(`wait for run ${build.databaseId} and download its files`);
        if (build.status !== "completed") say(`Waiting for run ${build.databaseId}: ${REPO}/actions/runs/${build.databaseId}`);
        while (build.status !== "completed") {
            await sleep(60);
            build = ghJson("run", "view", String(build.databaseId), "--repo", SLUG, "--json", "databaseId,status,conclusion,headSha");
        }
        if (build.conclusion !== "success") stop(`The build failed: gh run view ${build.databaseId} --log-failed says where. Fix it, push, and run npm run ship again.`);
        mkdirSync(out, { recursive: true });
        // Into a folder of their own first: a build without the update signature is not a release's files.
        const down = join(out, "from-run");
        rmSync(down, { recursive: true, force: true });
        for (const name of [`release-${tag}`, `msix-${tag}`]) {
            const got = tryGh("run", "download", String(build.databaseId), "--repo", SLUG, "--name", name, "--dir", down);
            if (!got.ok && name.startsWith("release-")) stop(`The run has no ${name} artifact:\n${got.err}`);
        }
        if (!existsSync(join(down, "latest.json")) || !existsSync(join(down, `${setup}.sig`))) {
            writeFileSync(unsignedRuns, `${[...unsigned, String(build.databaseId)].join("\n")}\n`);
            stop(`${UNSIGNED}\n\n(What run ${build.databaseId} built is in ${down}: the installer and the Store's package, without those signatures.)`);
        }
        for (const name of readdirSync(down)) {
            rmSync(join(out, name), { recursive: true, force: true });
            renameSync(join(down, name), join(out, name));
        }
        rmSync(down, { recursive: true, force: true });
        writeFileSync(builtFrom, `${build.headSha}
`);
        done(`run ${build.databaseId} built ${tag} from ${build.headSha.slice(0, 7)}; its files are in ${out}`);
    },

    /** What came down is what the app will accept: the signature, the version it is bound to, the two names. */
    verify() {
        const missing = files().filter((path) => !existsSync(path));
        if (missing.length) stop(`Not built yet: ${missing.join(", ")}`);
        say(run("node", ["scripts/verify-update.mjs", join(out, "latest.json"), join(out, setup)]).out);
        if (sha256(join(out, "Glint-setup.exe")) !== sha256(join(out, setup))) stop("Glint-setup.exe isn't the same file as the versioned setup.");
        if (JSON.parse(readFileSync(join(out, "latest.json"), "utf8")).version !== version) stop("latest.json announces another version.");
        done(`${setup}: ${(readFileSync(join(out, setup)).length / 1048576).toFixed(1)} MB, SHA-256 ${sha256(join(out, setup))}`);
    },

    /** The draft release with its four files and the notes, checked against what GitHub holds, then published. */
    async release() {
        let made = release();
        if (!made) {
            if (!existsSync(notes)) stop(`No notes: ${notes}`);
            if (dry) return would(`create the draft release ${tag} with ${files().length} files, compare GitHub's copies, and publish it`);
            gh("release", "create", tag, ...files(), "--repo", SLUG, "--draft", "--target", releaseCommit(), "--title", `Glint ${tag}`, "--notes-file", notes);
            made = release();
        }
        for (const path of files()) {
            const name = path.split(/[\\/]/).pop();
            const asset = made.assets.find((a) => a.name === name);
            if (!asset) stop(`The release has no ${name}.`);
            if (asset.digest && asset.digest !== `sha256:${sha256(path)}`) stop(`GitHub's ${name} isn't the file that was built (${asset.digest}).`);
        }
        if (made.isDraft) {
            if (flag("--draft")) return done(`${tag} is a draft: read it, try Glint-setup.exe, then npm run ship again without --draft. ${REPO}/releases`);
            if (dry) return would(`publish ${tag} as the latest release`);
            gh("release", "edit", tag, "--repo", SLUG, "--draft=false", "--latest");
        }
        // GitHub keeps the old address of "latest" for a moment; a query string gets the new one at once.
        for (let i = 0; i < 30; i++) {
            const answer = await fetch(`${REPO}/releases/latest/download/latest.json?${Date.now()}`).then((r) => (r.ok ? r.json() : undefined), () => undefined);
            if (answer?.version === version) {
                stampReadme();
                return done(`${tag} is published, and installed copies are offered it: ${REPO}/releases/tag/${tag}`);
            }
            await sleep(10);
        }
        stop(`${tag} is published, but ${REPO}/releases/latest/download/latest.json doesn't say ${version} yet.`);
    },

    /** The Scoop bucket's manifest, replaced by this release's. */
    scoop() {
        if (flag("--no-scoop")) return;
        const ours = readFileSync(join(manifests, "scoop", "glint.json"), "utf8");
        const theirs = ghJson("api", `repos/${BUCKET}/contents/bucket/glint.json`);
        if (JSON.parse(Buffer.from(theirs.content, "base64").toString("utf8")).version === version) return done(`Scoop's bucket has ${version}`);
        if (dry) return would(`put bucket/glint.json ${version} in ${BUCKET}`);
        const body = JSON.stringify({ message: `glint: Update to version ${version}`, content: Buffer.from(ours.replace(/\r\n/g, "\n")).toString("base64"), sha: theirs.sha });
        run("gh", ["api", "--method", "PUT", `repos/${BUCKET}/contents/bucket/glint.json`, "--input", "-"], { input: body });
        done(`Scoop's bucket has ${version}: scoop update glint`);
    },

    /** The winget manifests, on a branch of the fork, and the pull request to microsoft/winget-pkgs. */
    winget() {
        if (flag("--no-winget")) return;
        const title = `Update: ${PACKAGE} to ${version}`;
        const open = ghJson("pr", "list", "--repo", WINGET, "--author", "@me", "--state", "all", "--search", `"${title}" in:title`, "--json", "number,title,state,url").find((pr) => pr.title === title);
        if (open) return done(`winget: ${open.url} (${open.state.toLowerCase()})`);
        const folder = join(manifests, "winget", "manifests", "s", "shadijhade", "Glint", version);
        const yaml = readdirSync(folder).filter((name) => name.endsWith(".yaml"));
        if (yaml.length !== 3) stop(`${folder} should hold three manifests.`);
        const validated = has("winget", ["validate", "--manifest", `"${folder}"`]);
        if (!validated) stop(`winget validate refuses the manifests in ${folder} (or winget isn't here).`);
        if (dry) return would(`branch ${PACKAGE}-${version} on ${WINGET_FORK} with ${yaml.length} manifests, and the pull request to ${WINGET}`);
        // The fork's master is brought up to date, and the branch is one commit on it.
        gh("repo", "sync", WINGET_FORK, "--source", WINGET, "--branch", "master");
        const base = ghJson("api", `repos/${WINGET_FORK}/git/ref/heads/master`).object.sha;
        const baseTree = ghJson("api", `repos/${WINGET_FORK}/git/commits/${base}`).tree.sha;
        const api = (path, body) => JSON.parse(run("gh", ["api", "--method", "POST", `repos/${WINGET_FORK}/${path}`, "--input", "-"], { input: JSON.stringify(body) }).out);
        const tree = api("git/trees", {
            base_tree: baseTree,
            tree: yaml.map((name) => ({ path: `manifests/s/shadijhade/Glint/${version}/${name}`, mode: "100644", type: "blob", content: readFileSync(join(folder, name), "utf8") })),
        });
        const commit = api("git/commits", { message: title, tree: tree.sha, parents: [base] });
        const branch = `${PACKAGE}-${version}`;
        const ref = run("gh", ["api", "--method", "POST", `repos/${WINGET_FORK}/git/refs`, "--input", "-"], { input: JSON.stringify({ ref: `refs/heads/${branch}`, sha: commit.sha }), allowFailure: true });
        if (!ref.ok && !/already exists/i.test(`${ref.out}${ref.err}`)) stop(`Couldn't make the branch ${branch}:\n${ref.err}`);
        const before = lastReleasedBefore();
        const body = [
            "## 📖 Description",
            "",
            `${PACKAGE} version ${version}, Glint's new release ([release notes](${REPO}/releases/tag/${tag})). The manifests are ${before ?? "the last version"}'s with the new version, installer URL, SHA-256 and release notes link.`,
            "",
            "## ✅ Checklist",
            "",
            "- [x] Signed the [Contributor License Agreement](https://cla.opensource.microsoft.com)",
            "- [ ] Linked to an issue (if applicable)",
            "",
            "## 📦 Manifest Checklist",
            "",
            "- [x] Checked that there aren't other open [pull requests](https://github.com/microsoft/winget-pkgs/pulls) for the same manifest update/change",
            "- [x] This PR only modifies one (1) manifest",
            "- [x] Validated manifest locally with `winget validate --manifest <path>` ([validation guide](https://github.com/microsoft/winget-pkgs/blob/master/doc/ValidationFailureGuide.md))",
            "- [ ] Tested manifest locally with `winget install --manifest <path>`",
            `- [ ] Manifest conforms to the [1.28 schema](https://github.com/microsoft/winget-pkgs/tree/master/doc/manifest/schema/1.28.0) (manifest version 1.10.0, as ${before ?? "the last version"}'s)`,
            "",
            "> **Note:** `<path>` is the directory containing the manifest you're submitting.",
        ].join("\n");
        const url = gh("pr", "create", "--repo", WINGET, "--head", `shadijhade:${branch}`, "--base", "master", "--title", title, "--body", body);
        done(`winget: ${url} (Microsoft's checks and a moderator merge it)`);
    },

    /** The Store's package, from the very files the setup program installs, and its submission. */
    store() {
        if (flag("--no-store")) return;
        const msix = join(out, `Glint_${version}_x64.msix`);
        if (!existsSync(msix)) {
            if (dry) return would(`pack ${msix} from the released setup program's files`);
            // The setup program is a 7-Zip archive of what it installs: the released Glint.exe, the service and
            // the service's update signature, byte for byte, with no key needed here.
            const sevenZip = ["C:\\Program Files\\7-Zip\\7z.exe", "C:\\Program Files (x86)\\7-Zip\\7z.exe"].find(existsSync);
            if (!sevenZip) stop("7-Zip isn't installed (winget install 7zip.7zip): it takes the released files out of the setup program for the Store's package.");
            const unpacked = join(out, "setup-files");
            rmSync(unpacked, { recursive: true, force: true });
            run(sevenZip, ["x", join(out, setup), `-o${unpacked}`, "-y", "-bso0", "-bsp0"]);
            run("node", ["scripts/msix.mjs", "--from", unpacked, "--out", out], { stdio: "inherit" });
            rmSync(unpacked, { recursive: true, force: true });
        }
        if (!process.env.PARTNER_CENTER_TENANT_ID || !process.env.PARTNER_CENTER_CLIENT_ID) {
            say(`! The Store's package is ${msix}.`);
            say("  The Store API isn't set up on this PC (docs/releasing.md, \"Sending it with npm run store\": once, in Partner Center),");
            say("  so upload it in Partner Center → Glint → Packages, or set it up and run: npm run ship -- --only store");
            return;
        }
        const before = lastReleasedBefore();
        const listingChanged = flag("--listing") || (before && !run("git", ["diff", "--quiet", `v${before}`, "HEAD", "--", "crates/glint/msix/listing.json", "screenshots"], { allowFailure: true }).ok);
        if (dry) return would(`npm run store -- --submit${listingChanged ? " --listing" : ""}`);
        run("node", ["scripts/store.mjs", "--submit", "--msix", msix, ...(listingChanged ? ["--listing"] : [])], { stdio: "inherit" });
        done("the Store has the submission: certification, then it publishes by itself (npm run store -- --status)");
    },
};

/** The README's "What's new" says the day this version came out: the day it was published, whatever was written before
 * (a day, or "coming soon" while a release waited). */
function stampReadme() {
    const published = ghJson("release", "view", tag, "--repo", SLUG, "--json", "publishedAt").publishedAt;
    const day = new Date(published).toLocaleDateString("en-US", { year: "numeric", month: "long", day: "numeric" });
    const text = read("README.md");
    const dated = text.replace(new RegExp(`(\\*\\*${version.replace(/\./g, "\\.")}\\*\\* \\()(?:[A-Z][a-z]+ \\d{1,2}, \\d{4}|coming soon)(\\))`), `$1${day}$2`);
    if (dated === text || git("status", "--porcelain", "README.md")) return;
    writeFileSync(join(root, "README.md"), dated);
    git("add", "README.md");
    git("commit", "--quiet", "-m", `docs: ${version} came out on ${day}`);
    git("push", "--quiet", "origin", "master");
    done(`the README says ${version} came out on ${day}`);
}

/** The version released before this one, for "the manifests are X's with…". */
function lastReleasedBefore() {
    const all = ghJson("release", "list", "--repo", SLUG, "--exclude-drafts", "--limit", "30", "--json", "tagName").map((r) => r.tagName.replace(/^v/, ""));
    return all.find((v) => v !== version);
}

// ---- The run -------------------------------------------------------------------------------------------------

const only = value("--only")?.split(",");
const from = value("--from");
for (const name of [...(only ?? []), ...(from ? [from] : [])]) if (!STEPS.includes(name)) throw new Error(`No step "${name}": ${STEPS.join(", ")}`);
const chosen = STEPS.filter((name) => (only ? only.includes(name) : from ? STEPS.indexOf(name) >= STEPS.indexOf(from) : true));

try {
    for (const name of chosen) {
        console.log(`\n— ${name} —`);
        await steps[name]();
    }
    console.log(`\n✓ ${dry ? "Nothing was changed." : `Glint ${version}: ${chosen.join(", ")} done.`}`);
} catch (error) {
    if (!(error instanceof Stop)) throw error;
    console.error(`\n✗ ${error.message}`);
    process.exit(1);
}
