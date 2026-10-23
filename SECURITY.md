# Security

## Reporting a vulnerability

Please report security problems privately through [GitHub's vulnerability reporting](https://github.com/shadijhade/glint/security/advisories/new), not in a public issue. Include the app version, what an attacker could do, and the steps to reproduce it. You'll get an answer as soon as possible, and credit in the release notes if you'd like.

Only the latest release gets security fixes.

## What's in scope

The parts of the app worth a closer look:

-   **Instant search service** (opt-in). It runs as LocalSystem to read NTFS volumes and answers name searches over a named pipe. Its full security model (reduced privileges, where it's installed, the pipe's ACL and owner check, what the index exposes) is in [docs/instant-search.md](docs/instant-search.md#security-model).
-   **File operations.** Copy, move, delete and rename go through Windows' own shell engine (`IFileOperation`) with the signed-in user's rights; the app doesn't elevate for them.
-   **Updates.** The app installs an update only if its minisign signature verifies with the public key built into the app and names the version being offered (`requireSignedVersion`), so neither a changed installer nor an older one passed off as new gets in; it never installs an older version. The private key never enters the repository; it signs releases on the maintainer's PC or, for releases the release workflow builds, from an encrypted GitHub Actions secret. Copies the setup program didn't install don't update themselves. The release process is in [docs/releasing.md](docs/releasing.md).
-   **Bundled tools.** `rg`, `fd` and `dust` are unmodified official releases, checked against their published SHA-256 digests (see [CONTRIBUTING](CONTRIBUTING.md#updating-a-bundled-tool)); a release signed on the maintainer's PC adds its signature to the copies in the installer, nothing else. The app runs the copies next to it; only when one is missing (a security tool quarantined it, a portable copy) does it run one installed on the PC, from winget, Scoop, Cargo, Chocolatey or an absolute `PATH` folder, and Text mode then offers to install ripgrep with winget. It builds their arguments itself from typed options, so the UI can't pass arbitrary flags to them.
-   **Previews.** Images, video, audio and PDFs are shown through the app's local asset protocol. HTML files are shown as text, never rendered, and a PDF is shown only if its bytes are a PDF's (a web page named `.pdf` isn't).
-   **Archives.** Zip, 7z and tar files and compressed streams are parsed in-process (pure-Rust readers, every read under `catch_unwind`). Extraction keeps every path inside its target folder, cuts each entry off at its declared size, refuses likely zip bombs and checks free space first; nothing is ever written into an archive. The limits are in [docs/archives.md](docs/archives.md#safety).

## Code signing and privacy policy

Releases aren't code-signed yet: the SignPath Foundation, which signs open-source projects for free, asked for a new application once Glint is better known. Until then, check a download against the SHA-256 GitHub shows next to it on the release page (Glint's File X-Ray compares a pasted one), or install with Scoop, which checks it for you.

-   **Team roles (committers, reviewers and approvers):** [Shadi Saadi (`@shadijhade`)](https://github.com/shadijhade) is the sole maintainer, reviewer and release approver.
-   **Privacy policy:** Glint does not collect telemetry, analytics or personal data, and will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it. Its only network requests are checking GitHub Releases (`github.com/shadijhade/glint`) for application updates and downloading an update when the user chooses to install one.

