// The crates of the Windows build, from `cargo metadata` (offline: Cargo.lock decides what they are), for the
// scripts that read their licenses and sources (licenses.mjs, embedded-notices.mjs).

import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { root } from "./project.mjs";

/** Every package in the dependency graph of the x86_64-pc-windows-msvc build: { name, version, license, dir, source, publish, repository }. */
export function windowsPackages() {
    const result = spawnSync("cargo", ["metadata", "--format-version", "1", "--offline", "--filter-platform", "x86_64-pc-windows-msvc", "--manifest-path", join(root, "Cargo.toml")],
        { encoding: "utf8", maxBuffer: 1 << 30 });
    if (result.error || result.status) throw new Error(`cargo metadata failed: ${(result.stderr || result.error?.message || "").trim().split("\n").slice(-3).join(" ")}`);
    const metadata = JSON.parse(result.stdout);
    const used = new Set(metadata.resolve.nodes.map((node) => node.id));
    return metadata.packages
        .filter((p) => used.has(p.id))
        .map((p) => ({
            name: p.name,
            version: p.version,
            license: p.license,
            licenseFile: p.license_file,
            dir: dirname(p.manifest_path),
            source: p.source,
            publish: p.publish,
            repository: p.repository,
        }));
}
