# Security

## Reporting a vulnerability

Please report security problems privately through [GitHub's vulnerability reporting](https://github.com/shadijhade/glint/security/advisories/new), not in a public issue. Include the app version, what an attacker could do, and the steps to reproduce it. You'll get an answer as soon as possible, and credit in the release notes if you'd like.

Only the latest release gets security fixes.

## What's in scope

The parts of the app worth a closer look:

-   **Instant search service** (opt-in). It runs as LocalSystem to read NTFS volumes and answers name searches over a named pipe. Its full security model (reduced privileges, where it's installed, the pipe's ACL and owner check, what the index exposes) is in [docs/instant-search.md](docs/instant-search.md#security-model).
-   **File operations.** Copy, move, delete and rename go through Windows' own shell engine (`IFileOperation`) with the signed-in user's rights; the app doesn't elevate for them.
-   **Updates.** The app installs an update only if its minisign signature verifies with the public key built into the app and names the version being offered (`requireSignedVersion`), so neither a changed installer nor an older one passed off as new gets in; it never installs an older version. The private key never enters the repository; it signs releases on the maintainer's PC. Copies the setup program didn't install don't update themselves. The release process is in [docs/releasing.md](docs/releasing.md).
-   **Bundled tools.** `rg`, `fd` and `dust` are unmodified official releases, checked against their published SHA-256 digests (see [CONTRIBUTING](CONTRIBUTING.md#updating-a-bundled-tool)); a code-signed release adds its signature to the copies in the installer, nothing else. The app builds their arguments itself, so the UI can't pass arbitrary flags to them.
-   **Previews.** Images, video, audio and PDFs are shown through the app's local asset protocol. HTML files are shown as text, never rendered.
