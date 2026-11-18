# Tools

Glint gets much of its speed and reach from command-line tools, most of them from the Rust world. Three come with it. Most of the rest install when a feature first needs one: that feature shows an **Install** card, and **Settings → Tools** lists every tool by what it's for, with its version, what it does in Glint, where it came from, a progress bar while it installs, and **Install**, **Update** (or **Update all**) or **Remove**.

## Bundled

| Tool | In Glint |
| :--- | :--- |
| [ripgrep](https://github.com/BurntSushi/ripgrep) (`rg.exe`) | Searching inside files (the search box, on Enter) |
| [fd](https://github.com/sharkdp/fd) (`fd.exe`) | Finding files by name where instant search doesn't |
| [pdu](https://github.com/KSXGitHub/parallel-disk-usage) (built in) | Folder sizes and disk-usage maps where instant search doesn't |

ripgrep and fd are the official Windows x64 releases, unmodified, installed next to `Glint.exe`. pdu's engine (the parallel-disk-usage crate) is built into `Glint.exe`, which runs it in a copy of itself so a measurement no longer wanted stops at once. If one is missing (a security tool quarantined it, a portable copy), Glint uses one installed on the PC instead, and **Install** puts it back from its official release, the very build Glint ships. The installer checks for them once it has copied them, and puts back one that's gone.

## Installed when a feature needs them

| Tool | In Glint | From (winget only when the release can't be downloaded) |
| :--- | :--- | :--- |
| [Czkawka](https://github.com/qarmin/czkawka) CLI | Clean up's look-alike pictures, videos and music, broken files and wrong extensions | winget `qarmin.czkawka.cli`, else its release, 12.0.2 |
| [FFmpeg](https://ffmpeg.org/) | **Media**: trim, convert and shrink (hardware encoders when the PC has them), the sound alone, GIFs; videos Windows can't play get a copy that plays; Clean up's look-alike videos | winget `Gyan.FFmpeg.Essentials`, else its release, 9.0.2 (with ffprobe) |
| [Difftastic](https://github.com/Wilfred/difftastic) | Compare two files, and a changed file's **Changes**: code compared by its structure | winget `Wilfred.difftastic`, else its release, 0.72.0 |
| [jaq](https://github.com/01mf02/jaq) | jq filters over JSON, YAML and TOML in their preview | its GitHub release, v3.1.2 |
| [rimage](https://github.com/vlad-salone/rimage) | **Convert pictures** to JPEG, PNG, WebP or AVIF | its GitHub release, v0.14.0 |
| [whisper.cpp](https://github.com/ggml-org/whisper.cpp) | **Make subtitles** (an .srt beside a video, in its language or in English) | its GitHub release, b5454, with Whisper's base model (downloaded the first time subtitles are made) |
| [pdfcpu](https://github.com/pdfcpu/pdfcpu) | **PDF tools**: merge, split, keep or take out pages, turn pages, make smaller, save its pictures, passwords (AES-256), stamps. Run offline (`--offline`), its settings folder off | its GitHub release, v0.16.4 |
| [DuckDB](https://duckdb.org) | **Query** over CSV, JSON and Parquet files of any size, read where they are; **Profile**; **Query together**; **Convert data**. Locked down first: no extensions downloaded or loaded, only the chosen files readable, one SELECT at a time | its GitHub release, v2.0.0 |
| [ExifTool](https://exiftool.org) | **Remove personal info** from photos, videos and PDFs (Office files are cleaned by Glint itself), and **Change when it was taken** | its release from SourceForge (which keeps every version), 13.61, with its exiftool_files folder |
| [7-Zip](https://7-zip.org) | Disk images, installers and packages opened like folders: ISO, IMG, VHD(X), VMDK, DMG, WIM, ext4/APFS images, NSIS installers, DEB, RPM | its release's installer, 26.06, whose 7z payload Glint unpacks itself (nothing in it runs); a 7-Zip installed in Program Files is used too |
| [pandoc](https://pandoc.org) | **Convert document**: Word, OpenDocument, Markdown, HTML, EPUB, LaTeX, RTF, reStructuredText, Org and notebooks into one another, and Markdown into PowerPoint. Run with `--sandbox`, so it reads only the document it's given | winget `JohnMacFarlane.Pandoc`, else its release, 3.13 |
| [Typst](https://typst.app) | PDFs from **Convert document**, without LaTeX or Word | winget `Typst.Typst`, else its release, 0.15.1 |
| [restic](https://restic.net) | **Back up**: folders backed up to a drive, a network folder or a cloud (through rclone), encrypted, each backup adding only what changed, and shown in **Previous versions** | winget `restic.restic`, else its release, 0.19.1 |
| [rclone](https://rclone.org) | Google Drive, OneDrive, Dropbox and 60 more clouds under **Remote**, signed in to from Glint, and folder rules that keep a folder in sync with one ([how](remote.md#clouds-through-rclone)) | winget `Rclone.Rclone`, else its release, 1.76.0 |

**Or all at once, while installing Glint.** The installer's **Extra tools** page lists them with instant search, grouped by what they're for, each with what it is and what Glint uses it for. The ones you tick install once Glint is in place, the same way as below: Glint installs them (`Glint.exe tools install <id>`), with how each is going on the installer's status line, and the last page says if one didn't. One that's on the PC already is greyed out and left as it is. A silent install picks them with `/TOOLS=difftastic,jaq` (the ids in [`tools.rs`](../crates/glint-core/src/tools.rs)), and turns on instant search with `/INSTANTSEARCH`.

## Where they go

Every tool installs on every PC, with winget or without it: each has its own release pinned, and that comes first.

-   **From its own release** (since 0.13, first for every tool): downloaded over HTTPS from the URL pinned in [`crates/glint-core/src/tools.rs`](../crates/glint-core/src/tools.rs), through the PC's proxy if it has one (a download that can't connect, or breaks off, is tried again over IPv4 alone, which gets through on networks whose IPv6 is broken), checked against the SHA-256 pinned beside it (a download that doesn't match is thrown away), and only the program (and anything it needs beside it) taken out, into `%LOCALAPPDATA%\com.shado.shadui-ripgrep\tools\<tool>\`, for your account alone, with no administrator prompt. Beside it Glint writes `glint-version.txt`, the version it put there, so the version is known without running the program (restic and pdfcpu answer `--version` with "unknown flag", ExifTool prints its manual: those are asked `version`, `version` and `-ver` when Glint must ask). **Update** in Settings → Tools appears only for a copy Glint downloaded whose version is known and older than the one pinned now; it replaces the copy in place, even while it runs (its files are moved aside, a running program's file can be renamed, and swept at the next start). `cargo test --lib -- --ignored pinned_releases` downloads and unpacks every pinned release, to check new pins.
-   **With winget**, only when the release can't be downloaded (a network that blocks GitHub) and never over a copy Glint downloaded: `winget install --id <id> --exact --source winget --scope user`, silently, with its progress in the card and a **Stop**. Until 0.13 winget came first, which made most of the trouble people wrote about: slow source updates, a prompt now and then, a newer version than Glint knew, and an Update button that never went away when the copy Glint downloaded earlier stood in front of winget's.
-   **Already on the PC**: Glint uses a copy it finds on `PATH`, in winget's links and packages, Scoop's shims, Cargo's `bin` or Chocolatey's `bin`, and installs nothing.

Glint builds each tool's arguments itself, from typed options (the UI never passes flags through), runs it without a shell or a window, reads its JSON where it has one, and stops it with **Stop** or when the tab moves on to something else.

## Removing them

-   **Downloaded by Glint**: **Remove** in Settings → Tools deletes its folder; the model's row has its own **Remove**.
-   **Installed with winget**: Glint leaves those alone, since you may use them outside Glint too. Remove one with `winget uninstall --id <id>`, or in **Settings → Apps → Installed apps**.
-   **Everything Glint keeps**: its uninstaller's *Delete the application data* removes `%LOCALAPPDATA%\com.shado.shadui-ripgrep`: the tools it downloaded, the model, your Scripts folder, the folder rules' log and SFTP host keys you trusted in Glint.

## Found on your PATH, not downloaded

Five more programs are used when they're already on the PC, and are never downloaded, updated or removed by Glint (they used to be in Settings → Tools). Without one, the feature that wants it says so, with the `winget install` that gets it, and the rest works as before. Install them yourself, with winget or however you like, and Glint finds them on `PATH`, in winget's links and packages, Scoop's shims, Cargo's `bin` or Chocolatey's `bin`.

| Program | In Glint | winget |
| :--- | :--- | :--- |
| [zoxide](https://github.com/ajeetdsouza/zoxide) | Folder history shared with your terminals (Settings → History) | `ajeetdsouza.zoxide` |
| [just](https://github.com/casey/just) | A justfile's recipes in the **Run** menu | `Casey.Just` |
| [uv](https://github.com/astral-sh/uv) | pyproject.toml's scripts in **Run**, and Python scripts in **Scripts** (Python itself works too) | `astral-sh.uv` |
| [Bun](https://bun.sh/) | TypeScript and JavaScript scripts in **Scripts** (Node.js works too, for JavaScript) | `Oven-sh.Bun` |
| [Nushell](https://www.nushell.sh/) | A shell for the terminal, and Nushell scripts in **Scripts** | `Nushell.Nushell` |
