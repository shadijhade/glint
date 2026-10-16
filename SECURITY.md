# Security

## Reporting a vulnerability

Please report security problems privately through [GitHub's vulnerability reporting](https://github.com/shadijhade/shadui-ripgrep/security/advisories/new), not in a public issue. Include the app version, what an attacker could do, and the steps to reproduce it. You'll get an answer as soon as possible, and credit in the release notes if you'd like.

Only the latest release gets security fixes.

## What's in scope

The parts of the app worth a closer look:

-   **Instant search service** (opt-in). It runs as LocalSystem to read NTFS volumes and answers name searches over a named pipe. Its full security model (reduced privileges, where it's installed, the pipe's ACL and owner check, what the index exposes) is in [docs/instant-search.md](docs/instant-search.md#security-model).
-   **File operations.** Copy, move, delete and rename go through Windows' own shell engine (`IFileOperation`) with the signed-in user's rights; the app doesn't elevate for them.
-   **Bundled tools.** `rg`, `fd` and `dust` are unmodified official releases, checked against their published SHA-256 digests (see [CONTRIBUTING](CONTRIBUTING.md#updating-a-bundled-tool)). The app builds their arguments itself, so the UI can't pass arbitrary flags to them.
-   **Previews.** Images, video, audio and PDFs are shown through the app's local asset protocol. HTML files are shown as text, never rendered.
