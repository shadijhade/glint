// Code signing for the release scripts: one command line with %1 where a file's path goes, which
// gpui-installer.mjs runs on Glint.exe, the tools, the service, the uninstaller and the finished setup
// (GLINT_SIGN_COMMAND). Where it comes from:
//   * --sign-command "<command line with %1>" (or GLINT_SIGN_COMMAND already set): any signing tool, such as
//     `signtool sign /fd SHA256 /sha1 <thumbprint> /tr http://time.certum.pl /td SHA256 "%1"` for a certificate
//     in the Windows store, or SSL.com's CodeSignTool or DigiCert's smctl;
//   * AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and AZURE_SIGNING_PROFILE: Microsoft's Artifact Signing
//     through signtool and its client, fetched once into release/.tools and checked by its SHA-256, signed in
//     with `az login` or AZURE_CLIENT_ID, AZURE_CLIENT_SECRET and AZURE_TENANT_ID.
// Nothing here reads a key: the command does whatever the person who runs the release has set up.

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { root } from "./project.mjs";

const CLIENT = "1.0.128";
const CLIENT_SHA256 = "74bd7d27e6ce1051409c38d9b46bc8df0400ecd643d51ffbf2ac00869061e40b";

/** signtool.exe of the newest Windows SDK that has one, or undefined. */
export function findSigntool() {
    const kits = join(process.env["ProgramFiles(x86)"] ?? "C:\\Program Files (x86)", "Windows Kits", "10", "bin");
    const sdk = existsSync(kits) ? readdirSync(kits).filter((d) => /^10\.\d+\.\d+\.\d+$/.test(d) && existsSync(join(kits, d, "x64", "signtool.exe"))) : [];
    const newest = sdk.sort((a, b) => a.localeCompare(b, undefined, { numeric: true })).pop();
    return newest ? join(kits, newest, "x64", "signtool.exe") : undefined;
}

/** Fetches Microsoft's Artifact Signing client into release/.tools (once), checked against its SHA-256; returns its folder. Throws why not. */
async function artifactSigningClient() {
    const client = join(root, "release", ".tools", `artifact-signing-${CLIENT}`);
    if (existsSync(join(client, "bin", "x64", "Azure.CodeSigning.Dlib.dll"))) return client;
    const response = await fetch(`https://www.nuget.org/api/v2/package/Microsoft.ArtifactSigning.Client/${CLIENT}`);
    if (!response.ok) throw new Error(`Couldn't download the Artifact Signing client: ${response.status} ${response.statusText}`);
    const data = Buffer.from(await response.arrayBuffer());
    if (createHash("sha256").update(data).digest("hex") !== CLIENT_SHA256) {
        throw new Error(`Microsoft.ArtifactSigning.Client ${CLIENT} doesn't match the SHA-256 recorded in scripts/lib/codesign.mjs`);
    }
    mkdirSync(client, { recursive: true });
    writeFileSync(join(client, "package.zip"), data);
    // Windows' own tar reads zip files (Git Bash's doesn't).
    const tar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
    if (spawnSync(tar, ["-xf", "package.zip"], { cwd: client }).status) throw new Error("Couldn't unpack the Artifact Signing client");
    return client;
}

/**
 * The signing command line for this release, or null when it isn't code-signed. `work` is a folder for the
 * metadata file Artifact Signing reads. Throws with what to fix when the setup is half there.
 */
export async function signCommandFor({ signCommand, work }) {
    if (signCommand) return signCommand;
    if (process.env.GLINT_SIGN_COMMAND?.trim()) return process.env.GLINT_SIGN_COMMAND.trim();
    const azure = ["AZURE_SIGNING_ENDPOINT", "AZURE_SIGNING_ACCOUNT", "AZURE_SIGNING_PROFILE"].map((name) => process.env[name]);
    if (!azure.some(Boolean)) return null;
    if (!azure.every(Boolean)) throw new Error("Set all of AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and AZURE_SIGNING_PROFILE.");
    const [endpoint, account, profile] = azure;
    const signtool = findSigntool();
    if (!signtool) throw new Error("signtool.exe wasn't found: install the Windows SDK (10.0.26100 or newer).");
    const client = await artifactSigningClient();
    // Signs in with the AZURE_CLIENT_* service principal if those are set, else with `az login`.
    const others = ["ManagedIdentityCredential", "WorkloadIdentityCredential", "SharedTokenCacheCredential", "VisualStudioCredential",
        "VisualStudioCodeCredential", "AzurePowerShellCredential", "AzureDeveloperCliCredential", "InteractiveBrowserCredential"];
    const metadata = join(work, "metadata.json");
    writeFileSync(metadata, JSON.stringify({ Endpoint: endpoint, CodeSigningAccountName: account, CertificateProfileName: profile, ExcludeCredentials: others }));
    return [`"${signtool}"`, "sign", "/fd", "SHA256", "/tr", "http://timestamp.acs.microsoft.com", "/td", "SHA256", "/d", "Glint",
        "/dlib", `"${join(client, "bin", "x64", "Azure.CodeSigning.Dlib.dll")}"`, "/dmdf", `"${metadata}"`, '"%1"'].join(" ");
}

/** Runs the signing command on `file` (the command has %1 where its path goes); throws if it fails. */
export function signFileWith(command, file) {
    const result = spawnSync(command.replaceAll("%1", file), [], { shell: true, stdio: "inherit" });
    if (result.error || result.status) throw new Error(`The signing command failed on ${file}.`);
}

/** Checks that each file carries a valid Authenticode signature; returns the report's lines, throws when one doesn't. */
export function checkSignatures(files, work) {
    const script = join(work, "check-signatures.ps1");
    writeFileSync(script, "$bad = 0\nforeach ($f in $args) {\n  $s = Get-AuthenticodeSignature $f\n  \"$($s.Status)  $(Split-Path $f -Leaf)  $($s.SignerCertificate.Subject)\"\n  if ($s.Status -ne 'Valid') { $bad++ }\n}\nexit $bad\n");
    const result = spawnSync("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...files], { encoding: "utf8" });
    const report = (result.stdout ?? "").trim();
    if (result.status) throw new Error(`Not everything is validly code-signed:\n${report}`);
    return report;
}
